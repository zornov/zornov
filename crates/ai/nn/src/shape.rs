// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Static shape inference with unknown (`None`) dims for dynamic sizes.
//!
//! Covers the ops that matter for folding `Shape`-driven glue in exporter
//! graphs and for reporting: element-wise/broadcast ops, MatMul/Gemm, Conv,
//! pooling, reshape-family, Transpose, Concat, Gather, Slice (constant
//! bounds), Resize (constant scales/sizes), reductions and Shape itself.
//! Anything else yields "unknown".

use crate::onnx::Node;
use crate::ops::shape_ops::{norm_axis, reshape_target};
use crate::tensor::Tensor;
use std::collections::HashMap;

pub type Shape = Vec<Option<usize>>;

pub fn is_static(s: &Shape) -> bool {
    s.iter().all(|d| d.is_some())
}

pub fn to_static(s: &Shape) -> Option<Vec<usize>> {
    s.iter().copied().collect()
}

fn bcast(a: &Shape, b: &Shape) -> Option<Shape> {
    let r = a.len().max(b.len());
    let mut out = Vec::with_capacity(r);
    for i in 0..r {
        let da = if i + a.len() >= r { a[i + a.len() - r] } else { Some(1) };
        let db = if i + b.len() >= r { b[i + b.len() - r] } else { Some(1) };
        out.push(match (da, db) {
            (Some(1), x) | (x, Some(1)) => x,
            (Some(x), Some(y)) if x == y => Some(x),
            (Some(_), Some(_)) => return None,
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        });
    }
    Some(out)
}

const SAME_SHAPE: &[&str] = &[
    "Relu", "LeakyRelu", "Sigmoid", "Tanh", "HardSigmoid", "HardSwish", "Gelu", "Erf", "Softplus", "Softsign", "Mish", "Elu", "Selu", "Celu", "ThresholdedRelu", "Sqrt", "Exp", "Log", "Abs", "Neg", "Reciprocal", "Floor", "Ceil", "Round", "Sign", "Sin", "Cos", "Tan", "Asin", "Acos", "Atan", "Sinh", "Cosh", "Asinh", "Acosh", "Atanh", "IsNaN", "IsInf", "Not", "Clip", "Cast", "CastLike", "Identity", "Dropout", "Softmax", "LogSoftmax", "Hardmax", "BatchNormalization", "InstanceNormalization", "LayerNormalization", "GroupNormalization", "LpNormalization", "CumSum", "QuantizeLinear", "DequantizeLinear", "Trilu",
];
const BROADCAST: &[&str] = &["Add", "Sub", "Mul", "Div", "Pow", "Mod", "PRelu", "Max", "Min", "Sum", "Mean", "Equal", "Less", "Greater", "LessOrEqual", "GreaterOrEqual", "And", "Or", "Xor", "Where"];

/// Output shapes of one node, given input shapes and constant inputs.
pub fn infer_node(node: &Node, ins: &[Option<Shape>], consts: &[Option<&Tensor>], opset: i64) -> Vec<Option<Shape>> {
    let n_out = node.outputs.len();
    let unknown = vec![None; n_out];
    let op = node.op_type.as_str();
    let i0 = ins.first().cloned().flatten();
    let cint = |i: usize| consts.get(i).copied().flatten().map(|t| t.to_i64_vec());
    // An input that is given but not constant makes attribute fallbacks invalid.
    let dynamic_in = |i: usize| node.input(i).is_some() && consts.get(i).copied().flatten().is_none();
    if !crate::ops::normalize_domain(&node.domain).is_empty() {
        return unknown;
    }
    let one = |s: Option<Shape>| {
        let mut v = vec![None; n_out];
        if n_out > 0 {
            v[0] = s;
        }
        v
    };
    if SAME_SHAPE.contains(&op) {
        return one(i0);
    }
    if BROADCAST.contains(&op) {
        let mut acc: Option<Shape> = None;
        for s in ins {
            let Some(s) = s else { return unknown };
            acc = match acc {
                None => Some(s.clone()),
                Some(a) => bcast(&a, s),
            };
            if acc.is_none() {
                return unknown;
            }
        }
        return one(acc);
    }
    let Some(x) = i0 else { return unknown };
    let r = x.len();
    match op {
        "Shape" => {
            let rr = r as i64;
            let cl = |v: i64| (if v < 0 { v + rr } else { v }).clamp(0, rr);
            let (s, e) = (cl(node.attr_i("start", 0)), cl(node.attr_i("end", rr)));
            one(Some(vec![Some((e - s).max(0) as usize)]))
        }
        "Size" => one(Some(vec![])),
        "MatMul" => {
            let Some(Some(b)) = ins.get(1) else { return unknown };
            if r < 2 || b.len() < 2 {
                return unknown;
            }
            let Some(mut batch) = bcast(&x[..r - 2].to_vec(), &b[..b.len() - 2].to_vec()) else { return unknown };
            batch.push(x[r - 2]);
            batch.push(b[b.len() - 1]);
            one(Some(batch))
        }
        "Gemm" => {
            let Some(Some(b)) = ins.get(1) else { return unknown };
            let m = if node.attr_i("transA", 0) != 0 { x[1] } else { x[0] };
            let n = if node.attr_i("transB", 0) != 0 { b[0] } else { b[1] };
            one(Some(vec![m, n]))
        }
        "Conv" | "MaxPool" | "AveragePool" | "LpPool" => {
            let kernel: Option<Vec<usize>> = if op == "Conv" {
                consts.get(1).copied().flatten().map(|w| w.shape[2..].to_vec()).or_else(|| node.attr_ints("kernel_shape").map(|k| k.iter().map(|&v| v as usize).collect()))
            } else {
                node.attr_ints("kernel_shape").map(|k| k.iter().map(|&v| v as usize).collect())
            };
            let Some(k) = kernel else { return unknown };
            let oc = if op == "Conv" { consts.get(1).copied().flatten().map(|w| w.shape[0]).or(ins.get(1).cloned().flatten().and_then(|w| w[0])) } else { x.get(1).copied().flatten() };
            let sp: Option<Vec<usize>> = x[2..].iter().copied().collect();
            let mut out = vec![x[0], oc];
            match sp {
                Some(sp) if sp.len() == k.len() => {
                    let n = sp.len();
                    let st: Vec<usize> = node.attr_ints("strides").map(|v| v.iter().map(|&a| a as usize).collect()).unwrap_or(vec![1; n]);
                    let dl: Vec<usize> = node.attr_ints("dilations").map(|v| v.iter().map(|&a| a as usize).collect()).unwrap_or(vec![1; n]);
                    let ceil = node.attr_i("ceil_mode", 0) != 0;
                    match crate::ops::nn::window_geometry(node, &sp, &k, &st, &dl, ceil) {
                        Ok(g) => out.extend(g.out.into_iter().map(Some)),
                        Err(_) => return unknown,
                    }
                }
                _ => out.extend(std::iter::repeat_n(None, r - 2)),
            }
            one(Some(out))
        }
        "GlobalAveragePool" | "GlobalMaxPool" | "GlobalLpPool" => {
            let mut o = x.clone();
            for d in o.iter_mut().skip(2) {
                *d = Some(1);
            }
            one(Some(o))
        }
        "Reshape" => {
            let Some(t) = cint(1) else { return unknown };
            let allowzero = node.attr_i("allowzero", 0) != 0;
            if let Some(xs) = to_static(&x) {
                return one(reshape_target(&xs, &t, allowzero).ok().map(|v| v.into_iter().map(Some).collect()));
            }
            one(Some(t.iter().enumerate().map(|(i, &d)| if d > 0 { Some(d as usize) } else if d == 0 && !allowzero { x.get(i).copied().flatten() } else { None }).collect()))
        }
        "Flatten" => {
            let a = node.attr_i("axis", 1);
            let a = if a < 0 { a + r as i64 } else { a } as usize;
            let p = |s: &[Option<usize>]| s.iter().try_fold(1usize, |acc, d| d.map(|v| acc * v));
            one(Some(vec![p(&x[..a.min(r)]), p(&x[a.min(r)..])]))
        }
        "Squeeze" | "Unsqueeze" => {
            if dynamic_in(1) {
                return unknown;
            }
            let axes = cint(1).or_else(|| node.attr_ints("axes"));
            if op == "Squeeze" {
                match axes {
                    Some(ax) => {
                        let ax: Vec<usize> = ax.iter().filter_map(|&a| norm_axis(a, r).ok()).collect();
                        one(Some(x.iter().enumerate().filter(|(i, _)| !ax.contains(i)).map(|(_, d)| *d).collect()))
                    }
                    None if is_static(&x) => one(Some(x.iter().copied().filter(|d| *d != Some(1)).collect())),
                    None => unknown,
                }
            } else {
                let Some(ax) = axes else { return unknown };
                let rr = r + ax.len();
                let ax: Vec<usize> = ax.iter().filter_map(|&a| norm_axis(a, rr).ok()).collect();
                let mut it = x.iter();
                one(Some((0..rr).map(|i| if ax.contains(&i) { Some(1) } else { it.next().copied().flatten() }).collect()))
            }
        }
        "Transpose" => {
            let perm: Vec<usize> = node.attr_ints("perm").map(|p| p.iter().map(|&v| v as usize).collect()).unwrap_or_else(|| (0..r).rev().collect());
            one(Some(perm.iter().map(|&p| x.get(p).copied().flatten()).collect()))
        }
        "Concat" => {
            let Ok(ax) = norm_axis(node.attr_i("axis", 0), r) else { return unknown };
            let mut out = x.clone();
            let mut total = Some(0usize);
            for s in ins.iter() {
                let Some(s) = s else { return unknown };
                if s.len() != r {
                    return unknown; // the op skips mismatched (empty) inputs; stay conservative
                }
                total = match (total, s.get(ax).copied().flatten()) {
                    (Some(t), Some(d)) => Some(t + d),
                    _ => None,
                };
            }
            out[ax] = total;
            one(Some(out))
        }
        "Gather" => {
            let Ok(ax) = norm_axis(node.attr_i("axis", 0), r) else { return unknown };
            let Some(Some(is)) = ins.get(1) else { return unknown };
            let mut out = x[..ax].to_vec();
            out.extend(is.iter().copied());
            out.extend(x[ax + 1..].iter().copied());
            one(Some(out))
        }
        "Slice" => {
            if dynamic_in(3) || dynamic_in(4) {
                return unknown;
            }
            let (Some(st), Some(en)) = (cint(1).or_else(|| node.attr_ints("starts")), cint(2).or_else(|| node.attr_ints("ends"))) else { return unknown };
            let axes = cint(3).or_else(|| node.attr_ints("axes")).unwrap_or_else(|| (0..st.len() as i64).collect());
            let steps = cint(4).unwrap_or_else(|| vec![1; st.len()]);
            let mut out = x.clone();
            for i in 0..st.len() {
                let Ok(a) = norm_axis(axes[i], r) else { return unknown };
                let Some(dim) = x[a] else {
                    out[a] = None;
                    continue;
                };
                let dim = dim as i64;
                let step = steps[i];
                let (mut s, mut e) = (st[i], en[i]);
                if s < 0 {
                    s += dim;
                }
                if e < 0 {
                    e += dim;
                }
                out[a] = Some(if step > 0 {
                    let (s, e) = (s.clamp(0, dim), e.clamp(0, dim));
                    if e > s { ((e - s + step - 1) / step) as usize } else { 0 }
                } else {
                    let (s, e) = (s.clamp(0, dim - 1), e.clamp(-1, dim - 1));
                    if s > e { ((s - e - step - 1) / (-step)) as usize } else { 0 }
                });
            }
            one(Some(out))
        }
        "ReduceMean" | "ReduceSum" | "ReduceMax" | "ReduceMin" | "ReduceProd" | "ReduceL1" | "ReduceL2" | "ReduceLogSumExp" | "ReduceSumSquare" | "ArgMax" | "ArgMin" => {
            let keep = node.attr_i("keepdims", 1) != 0;
            let axes: Vec<usize> = if op.starts_with("Arg") {
                vec![norm_axis(node.attr_i("axis", 0), r).unwrap_or(0)]
            } else {
                if dynamic_in(1) {
                    return unknown;
                }
                match cint(1).or_else(|| node.attr_ints("axes")) {
                    Some(a) if !a.is_empty() => a.iter().filter_map(|&v| norm_axis(v, r).ok()).collect(),
                    _ if node.attr_i("noop_with_empty_axes", 0) != 0 => return one(Some(x.clone())),
                    _ => (0..r).collect(),
                }
            };
            one(Some(x.iter().enumerate().filter_map(|(i, d)| if axes.contains(&i) { if keep { Some(Some(1)) } else { None } } else { Some(*d) }).collect()))
        }
        "Resize" | "Upsample" => {
            // `axes` and keep_aspect_ratio_policy change the meaning of sizes/scales
            if node.has("axes") || node.attr_s_or("keep_aspect_ratio_policy", "stretch") != "stretch" {
                return unknown;
            }
            let legacy = op == "Upsample" || opset < 11;
            let scales = consts.get(if legacy { 1 } else { 2 }).copied().flatten().filter(|t| !t.is_empty());
            let sizes = if legacy { None } else { consts.get(3).copied().flatten().filter(|t| !t.is_empty()) };
            if let Some(s) = sizes {
                let sv = s.to_i64_vec();
                if sv.len() != r {
                    return unknown;
                }
                return one(Some(sv.iter().map(|&v| Some(v as usize)).collect()));
            }
            if let Some(s) = scales {
                let sv = s.to_f32_vec();
                if sv.len() == r {
                    return one(Some(x.iter().zip(&sv).map(|(d, &f)| d.map(|v| (v as f32 * f).floor() as usize)).collect()));
                }
            }
            unknown
        }
        "ConstantOfShape" | "Expand" | "Tile" => {
            if op == "ConstantOfShape" {
                return one(cint(0).map(|v| v.iter().map(|&d| Some(d as usize)).collect()));
            }
            let Some(s) = cint(1) else { return unknown };
            if op == "Tile" {
                return one(Some(x.iter().zip(&s).map(|(d, &rep)| d.map(|v| v * rep as usize)).collect()));
            }
            one(bcast(&x, &s.iter().map(|&d| Some(d as usize)).collect()))
        }
        _ => unknown,
    }
}

/// Infers shapes for a topologically ordered node list. `known` holds the
/// graph-input shapes on entry and gains every inferred value.
pub fn infer_graph(nodes: &[&Node], known: &mut HashMap<String, Shape>, consts: &HashMap<String, Tensor>, opset: i64) {
    for n in nodes {
        let ins: Vec<Option<Shape>> = n.inputs.iter().map(|i| if i.is_empty() { None } else { known.get(i).cloned().or_else(|| consts.get(i).map(|t| t.shape.iter().map(|&d| Some(d)).collect())) }).collect();
        let cs: Vec<Option<&Tensor>> = n.inputs.iter().map(|i| consts.get(i)).collect();
        let outs = infer_node(n, &ins, &cs, opset);
        for (name, s) in n.outputs.iter().zip(outs) {
            if let Some(s) = s
                && !name.is_empty()
            {
                known.insert(name.clone(), s);
            }
        }
    }
}
