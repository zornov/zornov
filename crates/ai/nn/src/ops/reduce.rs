// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reductions (ReduceMean/Sum/Max/Min/Prod/L1/L2/LogSum/LogSumExp/SumSquare)
//! and ArgMax/ArgMin.

use super::shape_ops::{norm_axis, transpose_tensor};
use super::{OpCtx, inp, opt};
use crate::error::Result;
use crate::onnx::Node;
use crate::tensor::{DType, Tensor};

/// Axes from the second input (opset 13 ReduceSum, 18 others) or the attribute.
fn reduce_axes(node: &Node, inputs: &[Option<&Tensor>], rank: usize) -> Result<Option<Vec<usize>>> {
    let raw = match opt(inputs, 1) {
        Some(t) => Some(t.to_i64_vec()),
        None => node.attr_ints("axes"),
    };
    match raw {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => {
            let mut a: Vec<usize> = v.iter().map(|&x| norm_axis(x, rank)).collect::<Result<_>>()?;
            a.sort();
            a.dedup();
            Ok(Some(a))
        }
    }
}

/// Applies `f` to each reduced row of an (outer, d, inner) view.
fn reduce_view<T: Copy, O>(v: &[T], outer: usize, d: usize, inner: usize, mut f: impl FnMut(&mut dyn Iterator<Item = T>) -> O) -> Vec<O> {
    let mut out = Vec::with_capacity(outer * inner);
    for o in 0..outer {
        for i in 0..inner {
            let mut it = (0..d).map(|k| v[(o * d + k) * inner + i]);
            out.push(f(&mut it));
        }
    }
    out
}

pub fn reduce(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let keep = node.attr_i("keepdims", 1) != 0;
    let noop = node.attr_i("noop_with_empty_axes", 0) != 0;
    let axes = match reduce_axes(node, inputs, r)? {
        Some(a) => a,
        None if noop => return Ok(vec![x.clone()]),
        None => (0..r).collect(),
    };
    // Move reduced axes to a contiguous block (transpose if needed).
    let contiguous = axes.windows(2).all(|w| w[1] == w[0] + 1);
    let (src, first, last) = if contiguous || axes.is_empty() {
        (std::borrow::Cow::Borrowed(x), axes.first().copied().unwrap_or(0), axes.last().map(|a| a + 1).unwrap_or(0))
    } else {
        let mut perm: Vec<usize> = (0..r).filter(|a| !axes.contains(a)).collect();
        let kept = perm.len();
        perm.extend(&axes);
        (std::borrow::Cow::Owned(transpose_tensor(x, &perm)), kept, r)
    };
    let sh = &src.shape;
    let outer: usize = sh[..first].iter().product();
    let d: usize = sh[first..last].iter().product();
    let inner: usize = sh[last..].iter().product();
    let mut oshape: Vec<usize> = Vec::new();
    for (i, &s) in x.shape.iter().enumerate() {
        if axes.contains(&i) {
            if keep {
                oshape.push(1);
            }
        } else {
            oshape.push(s);
        }
    }
    let op = node.op_type.as_str();
    if x.dtype() == DType::I64 && matches!(op, "ReduceSum" | "ReduceMax" | "ReduceMin" | "ReduceProd") {
        let v = src.as_i64().unwrap();
        let out = reduce_view(v, outer, d, inner, |it| match op {
            "ReduceSum" => it.sum(),
            "ReduceMax" => it.max().unwrap_or(i64::MIN),
            "ReduceMin" => it.min().unwrap_or(i64::MAX),
            _ => it.product(),
        });
        return Ok(vec![Tensor::i64(&oshape, out)]);
    }
    let v = src.f32s();
    let out: Vec<f32> = if inner == 1 && matches!(op, "ReduceMean" | "ReduceSum") {
        // contiguous rows: pairwise-friendly f64 sums
        (0..outer)
            .map(|o| {
                let s: f64 = v[o * d..(o + 1) * d].iter().map(|&e| e as f64).sum();
                if op == "ReduceMean" { (s / d.max(1) as f64) as f32 } else { s as f32 }
            })
            .collect()
    } else {
        reduce_view(&v, outer, d, inner, |it| match op {
            "ReduceMean" => (it.map(|e| e as f64).sum::<f64>() / d.max(1) as f64) as f32,
            "ReduceSum" => it.map(|e| e as f64).sum::<f64>() as f32,
            "ReduceMax" => it.fold(f32::NEG_INFINITY, |a, b| if b.is_nan() || a.is_nan() { f32::NAN } else { a.max(b) }),
            "ReduceMin" => it.fold(f32::INFINITY, |a, b| if b.is_nan() || a.is_nan() { f32::NAN } else { a.min(b) }),
            "ReduceProd" => it.product(),
            "ReduceL1" => it.map(|e| e.abs()).sum(),
            "ReduceL2" => (it.map(|e| (e as f64) * (e as f64)).sum::<f64>()).sqrt() as f32,
            "ReduceSumSquare" => it.map(|e| e * e).sum(),
            "ReduceLogSum" => it.sum::<f32>().ln(),
            _ => {
                // ReduceLogSumExp
                let vals: Vec<f32> = it.collect();
                let m = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                if m == f32::NEG_INFINITY { m } else { m + vals.iter().map(|e| (e - m).exp()).sum::<f32>().ln() }
            }
        })
    };
    Ok(vec![Tensor::f32(&oshape, out)])
}

pub fn arg_reduce(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let ax = norm_axis(node.attr_i("axis", 0), x.rank())?;
    let keep = node.attr_i("keepdims", 1) != 0;
    let last = node.attr_i("select_last_index", 0) != 0;
    let is_max = node.op_type == "ArgMax";
    let outer: usize = x.shape[..ax].iter().product();
    let d = x.shape[ax];
    let inner: usize = x.shape[ax + 1..].iter().product();
    let v = x.f32s();
    let out = reduce_view(&v, outer, d, inner, |it| {
        let mut best = 0usize;
        let mut bv = if is_max { f32::NEG_INFINITY } else { f32::INFINITY };
        for (k, e) in it.enumerate() {
            let better = k == 0 || if is_max { e > bv || (last && e == bv) } else { e < bv || (last && e == bv) };
            if better {
                best = k;
                bv = e;
            }
        }
        best as i64
    });
    let mut shape = x.shape.clone();
    if keep {
        shape[ax] = 1;
    } else {
        shape.remove(ax);
    }
    Ok(vec![Tensor::i64(&shape, out)])
}
