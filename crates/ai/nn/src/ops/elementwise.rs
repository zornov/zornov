// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Element-wise operators with numpy broadcasting: activations, arithmetic,
//! comparisons, logic, `Where`, `Clip`, `Cast`.

use super::{OpCtx, inp, opt};
use crate::error::{Error, Result};
use crate::kernels::par::par_chunks_mut;
use crate::onnx::Node;
use crate::tensor::{DType, Data, Tensor, numel};

// ------------------------------------------------------------ broadcasting

pub fn broadcast_shape(a: &[usize], b: &[usize]) -> Result<Vec<usize>> {
    let r = a.len().max(b.len());
    let mut out = vec![0; r];
    for i in 0..r {
        let da = if i + a.len() >= r { a[i + a.len() - r] } else { 1 };
        let db = if i + b.len() >= r { b[i + b.len() - r] } else { 1 };
        out[i] = if da == db {
            da
        } else if da == 1 {
            db
        } else if db == 1 {
            da
        } else {
            return Err(Error(format!("cannot broadcast shapes {a:?} and {b:?}")));
        };
    }
    Ok(out)
}

/// Strides of `shape` viewed as broadcast to `out` (0 on broadcast dims).
pub fn bstrides(shape: &[usize], out: &[usize]) -> Vec<usize> {
    let r = out.len();
    let st = crate::tensor::strides(shape);
    let mut s = vec![0; r];
    for i in 0..shape.len() {
        let o = r - shape.len() + i;
        s[o] = if shape[i] == 1 && out[o] != 1 { 0 } else { st[i] };
    }
    s
}

/// Merges adjacent dims that are contiguous for every operand.
fn collapse(out: &[usize], strides: &mut [Vec<usize>]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut dims: Vec<usize> = Vec::new();
    let mut st: Vec<Vec<usize>> = vec![Vec::new(); strides.len()];
    for i in 0..out.len() {
        if out[i] == 1 {
            continue;
        }
        if let Some(&last) = dims.last() {
            let _ = last;
            let mergeable = strides.iter().enumerate().all(|(k, s)| *st[k].last().unwrap() == s[i] * out[i]);
            if mergeable {
                let l = dims.len() - 1;
                dims[l] *= out[i];
                for (k, s) in strides.iter().enumerate() {
                    let ll = st[k].len() - 1;
                    st[k][ll] = s[i];
                }
                continue;
            }
        }
        dims.push(out[i]);
        for (k, s) in strides.iter().enumerate() {
            st[k].push(s[i]);
        }
    }
    if dims.is_empty() {
        dims.push(1);
        for s in st.iter_mut() {
            s.push(0);
        }
    }
    (dims, st)
}

/// Broadcasting binary map. Returns `(out_shape, values)`. Large outputs
/// are split across threads.
pub fn binary_map<A: Copy + Sync, B: Copy + Sync, O: Copy + Send>(a: &[A], ash: &[usize], b: &[B], bsh: &[usize], f: impl Fn(A, B) -> O + Sync) -> Result<(Vec<usize>, Vec<O>)> {
    let out = broadcast_shape(ash, bsh)?;
    let n = numel(&out);
    let threads = if n >= 1 << 16 { crate::kernels::par::resolve_threads(0) } else { 1 };
    let mut res: Vec<O> = Vec::with_capacity(n);
    // SAFETY: every element is written exactly once below.
    #[allow(clippy::uninit_vec)]
    unsafe {
        res.set_len(n)
    };
    let dst = crate::kernels::par::SendPtr(res.as_mut_ptr());
    if ash == bsh || (b.len() == 1 && a.len() == n) || (a.len() == 1 && b.len() == n) {
        let (sa, sb) = (if a.len() == 1 && n > 1 { 0 } else { 1 }, if b.len() == 1 && n > 1 { 0 } else { 1 });
        crate::kernels::par::par_range(n, threads, |lo, hi| {
            for i in lo..hi {
                // SAFETY: disjoint ranges per worker.
                unsafe { *dst.add(i) = f(a[i * sa], b[i * sb]) };
            }
        });
        return Ok((out, res));
    }
    let mut ss = vec![bstrides(ash, &out), bstrides(bsh, &out)];
    let (dims, st) = collapse(&out, &mut ss);
    let r = dims.len();
    let inner = dims[r - 1];
    let (sa, sb) = (st[0][r - 1], st[1][r - 1]);
    let outer: usize = dims[..r - 1].iter().product();
    let threads = if outer > 1 { threads } else { 1 };
    crate::kernels::par::par_range(outer, threads, |lo, hi| {
        for o in lo..hi {
            let (mut rem, mut ia, mut ib) = (o, 0usize, 0usize);
            for d in (0..r - 1).rev() {
                let i = rem % dims[d];
                rem /= dims[d];
                ia += i * st[0][d];
                ib += i * st[1][d];
            }
            let base = o * inner;
            // SAFETY: row `o` belongs to this worker only.
            let row = unsafe { std::slice::from_raw_parts_mut(dst.add(base), inner) };
            match (sa, sb) {
                (1, 1) => row.iter_mut().enumerate().for_each(|(i, v)| *v = f(a[ia + i], b[ib + i])),
                (1, 0) => {
                    let y = b[ib];
                    row.iter_mut().enumerate().for_each(|(i, v)| *v = f(a[ia + i], y))
                }
                (0, 1) => {
                    let x = a[ia];
                    row.iter_mut().enumerate().for_each(|(i, v)| *v = f(x, b[ib + i]))
                }
                _ => row.iter_mut().enumerate().for_each(|(i, v)| *v = f(a[ia + i * sa], b[ib + i * sb])),
            }
        }
    });
    Ok((out, res))
}

/// Calls `f(out_index, [operand offsets])` for every element of the broadcast output.
pub fn for_each_bcast(out: &[usize], shapes: &[&[usize]], mut f: impl FnMut(usize, &[usize])) {
    let st: Vec<Vec<usize>> = shapes.iter().map(|s| bstrides(s, out)).collect();
    let n = numel(out);
    let r = out.len();
    let mut idx = vec![0usize; r];
    let mut offs = vec![0usize; shapes.len()];
    for i in 0..n {
        f(i, &offs);
        for d in (0..r).rev() {
            idx[d] += 1;
            for (k, s) in st.iter().enumerate() {
                offs[k] += s[d];
            }
            if idx[d] < out[d] {
                break;
            }
            for (k, s) in st.iter().enumerate() {
                offs[k] -= s[d] * out[d];
            }
            idx[d] = 0;
        }
    }
}

// ------------------------------------------------------------ math helpers

/// erf with ~1e-7 relative accuracy (Taylor series near 0, Chebyshev erfc elsewhere).
pub fn erf(x: f32) -> f32 {
    erf64(x as f64) as f32
}

pub fn erf64(x: f64) -> f64 {
    let z = x.abs();
    if z < 0.5 {
        let x2 = x * x;
        let mut term = x;
        let mut sum = x;
        for n in 1..12 {
            term *= -x2 / n as f64;
            sum += term / (2 * n + 1) as f64;
        }
        return sum * std::f64::consts::FRAC_2_SQRT_PI;
    }
    let t = 1.0 / (1.0 + 0.5 * z);
    let ans = t * (-z * z - 1.26551223 + t * (1.00002368 + t * (0.37409196 + t * (0.09678418 + t * (-0.18628806 + t * (0.27886807 + t * (-1.13520398 + t * (1.48851587 + t * (-0.82215223 + t * 0.17087277))))))))).exp();
    if x >= 0.0 { 1.0 - ans } else { ans - 1.0 }
}

#[inline]
pub fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

#[inline]
pub fn softplus(x: f32) -> f32 {
    if x > 20.0 { x } else { x.exp().ln_1p() }
}

pub fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + erf(x * std::f32::consts::FRAC_1_SQRT_2))
}

pub fn gelu_tanh(x: f32) -> f32 {
    let c = (2.0f32 / std::f32::consts::PI).sqrt();
    0.5 * x * (1.0 + (c * (x + 0.044715 * x * x * x)).tanh())
}

/// Round half to even.
pub fn round_even(x: f32) -> f32 {
    x.round_ties_even()
}

// ------------------------------------------------------------ unary

/// Builds the scalar function for a unary float op.
fn unary_fn(node: &Node) -> Option<Box<dyn Fn(f32) -> f32 + Sync>> {
    let a = |n: &str, d: f32| node.attr_f(n, d);
    Some(match node.op_type.as_str() {
        "Relu" => Box::new(|x: f32| x.max(0.0)),
        "LeakyRelu" => {
            let al = a("alpha", 0.01);
            Box::new(move |x| if x >= 0.0 { x } else { al * x })
        }
        "Sigmoid" => Box::new(sigmoid),
        "Tanh" => Box::new(|x: f32| x.tanh()),
        "HardSigmoid" => {
            let (al, be) = (a("alpha", 0.2), a("beta", 0.5));
            Box::new(move |x| (al * x + be).clamp(0.0, 1.0))
        }
        "HardSwish" => Box::new(|x: f32| x * (x / 6.0 + 0.5).clamp(0.0, 1.0)),
        "Gelu" => {
            if node.attr_s_or("approximate", "none") == "tanh" {
                Box::new(gelu_tanh)
            } else {
                Box::new(gelu)
            }
        }
        "FastGelu" => Box::new(gelu_tanh),
        "QuickGelu" => {
            let al = a("alpha", 1.702);
            Box::new(move |x| x * sigmoid(al * x))
        }
        "Erf" => Box::new(erf),
        "Softplus" => Box::new(softplus),
        "Softsign" => Box::new(|x: f32| x / (1.0 + x.abs())),
        "Mish" => Box::new(|x: f32| x * softplus(x).tanh()),
        "Elu" => {
            let al = a("alpha", 1.0);
            Box::new(move |x| if x >= 0.0 { x } else { al * (x.exp() - 1.0) })
        }
        "Selu" => {
            let (al, ga) = (a("alpha", 1.673_263_2), a("gamma", 1.050_701));
            Box::new(move |x| if x > 0.0 { ga * x } else { ga * al * (x.exp() - 1.0) })
        }
        "Celu" => {
            let al = a("alpha", 1.0);
            Box::new(move |x| x.max(0.0) + (al * ((x / al).exp() - 1.0)).min(0.0))
        }
        "ThresholdedRelu" => {
            let al = a("alpha", 1.0);
            Box::new(move |x| if x > al { x } else { 0.0 })
        }
        "Sqrt" => Box::new(|x: f32| x.sqrt()),
        "Exp" => Box::new(|x: f32| x.exp()),
        "Log" => Box::new(|x: f32| x.ln()),
        "Abs" => Box::new(|x: f32| x.abs()),
        "Neg" => Box::new(|x: f32| -x),
        "Reciprocal" => Box::new(|x: f32| 1.0 / x),
        "Floor" => Box::new(|x: f32| x.floor()),
        "Ceil" => Box::new(|x: f32| x.ceil()),
        "Round" => Box::new(round_even),
        "Sign" => Box::new(|x: f32| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }),
        "Sin" => Box::new(|x: f32| x.sin()),
        "Cos" => Box::new(|x: f32| x.cos()),
        "Tan" => Box::new(|x: f32| x.tan()),
        "Asin" => Box::new(|x: f32| x.asin()),
        "Acos" => Box::new(|x: f32| x.acos()),
        "Atan" => Box::new(|x: f32| x.atan()),
        "Sinh" => Box::new(|x: f32| x.sinh()),
        "Cosh" => Box::new(|x: f32| x.cosh()),
        "Asinh" => Box::new(|x: f32| x.asinh()),
        "Acosh" => Box::new(|x: f32| x.acosh()),
        "Atanh" => Box::new(|x: f32| x.atanh()),
        _ => return None,
    })
}

pub fn unary(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    if x.dtype() == DType::I64 {
        let v = x.as_i64().unwrap();
        let r: Vec<i64> = match node.op_type.as_str() {
            "Abs" => v.iter().map(|a| a.abs()).collect(),
            "Neg" => v.iter().map(|a| -a).collect(),
            "Sign" => v.iter().map(|a| a.signum()).collect(),
            "Relu" => v.iter().map(|a| (*a).max(0)).collect(),
            "Floor" | "Ceil" | "Round" => v.to_vec(),
            _ => return unary_float(ctx, node, x),
        };
        return Ok(vec![Tensor::i64(&x.shape, r)]);
    }
    unary_float(ctx, node, x)
}

fn unary_float(ctx: &OpCtx, node: &Node, x: &Tensor) -> Result<Vec<Tensor>> {
    let f = unary_fn(node).ok_or_else(|| Error(format!("unary op {} not implemented", node.op_type)))?;
    let mut out = x.to_f32_vec();
    par_chunks_mut(&mut out, ctx.threads, 1 << 15, |_, ch| {
        for v in ch {
            *v = f(*v);
        }
    });
    let t = Tensor::f32(&x.shape, out);
    // ONNX unary ops keep the input type; integer inputs round-trip through f32.
    Ok(vec![if x.dtype().is_float() { t } else { t.cast(x.dtype()) }])
}

pub fn is_nan(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    Ok(vec![Tensor::bool(&x.shape, x.f32s().iter().map(|v| v.is_nan()).collect())])
}

pub fn is_inf(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let (neg, pos) = (node.attr_i("detect_negative", 1) != 0, node.attr_i("detect_positive", 1) != 0);
    Ok(vec![Tensor::bool(&x.shape, x.f32s().iter().map(|&v| (v == f32::INFINITY && pos) || (v == f32::NEG_INFINITY && neg)).collect())])
}

pub fn not(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    Ok(vec![Tensor::bool(&x.shape, x.to_bool_vec().iter().map(|v| !v).collect())])
}

pub fn clip(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    if x.dtype() == DType::I64 {
        let lo = opt(inputs, 1).map(|t| t.i64s()[0]).unwrap_or(i64::MIN);
        let hi = opt(inputs, 2).map(|t| t.i64s()[0]).unwrap_or(i64::MAX);
        return Ok(vec![Tensor::i64(&x.shape, x.as_i64().unwrap().iter().map(|v| (*v).clamp(lo, hi.max(lo))).collect())]);
    }
    let lo = opt(inputs, 1).map(|t| t.f32s()[0]).unwrap_or(node.attr_f("min", f32::NEG_INFINITY));
    let hi = opt(inputs, 2).map(|t| t.f32s()[0]).unwrap_or(node.attr_f("max", f32::INFINITY));
    let v: Vec<f32> = x.f32s().iter().map(|&v| if v < lo { lo } else if v > hi { hi } else { v }).collect();
    let t = Tensor::f32(&x.shape, v);
    Ok(vec![if x.dtype().is_float() { t } else { t.cast(x.dtype()) }])
}

// ------------------------------------------------------------ binary

fn is_int(d: DType) -> bool {
    matches!(d, DType::I64 | DType::I8 | DType::U8)
}

fn arith_f32(op: &str, fmod: bool) -> Option<fn(f32, f32) -> f32> {
    Some(match op {
        "Add" | "Sum" => |a, b| a + b,
        "Sub" => |a, b| a - b,
        "Mul" => |a, b| a * b,
        "Div" => |a, b| a / b,
        "Pow" => |a: f32, b: f32| {
            if b == 2.0 {
                a * a
            } else if b == 0.5 {
                a.sqrt()
            } else {
                a.powf(b)
            }
        },
        "Max" => |a: f32, b: f32| if a.is_nan() || b.is_nan() { f32::NAN } else { a.max(b) },
        "Min" => |a: f32, b: f32| if a.is_nan() || b.is_nan() { f32::NAN } else { a.min(b) },
        "Mod" if fmod => |a: f32, b: f32| a % b,
        "Mod" => |a: f32, b: f32| {
            let r = a % b;
            if r != 0.0 && ((r < 0.0) != (b < 0.0)) { r + b } else { r }
        },
        "PRelu" => |x, s| if x < 0.0 { x * s } else { x },
        _ => return None,
    })
}

fn arith_i64(op: &str, fmod: bool) -> Option<fn(i64, i64) -> i64> {
    Some(match op {
        "Add" | "Sum" => |a: i64, b: i64| a.wrapping_add(b),
        "Sub" => |a: i64, b: i64| a.wrapping_sub(b),
        "Mul" => |a: i64, b: i64| a.wrapping_mul(b),
        "Div" => |a: i64, b: i64| if b == 0 { 0 } else { a.wrapping_div(b) },
        "Pow" => |a: i64, b: i64| if b < 0 { 0 } else { a.wrapping_pow(b.min(u32::MAX as i64) as u32) },
        "Max" => |a: i64, b: i64| a.max(b),
        "Min" => |a: i64, b: i64| a.min(b),
        "Mod" if fmod => |a: i64, b: i64| if b == 0 { 0 } else { a % b },
        "Mod" => |a: i64, b: i64| {
            if b == 0 {
                return 0;
            }
            let r = a % b;
            if r != 0 && ((r < 0) != (b < 0)) { r + b } else { r }
        },
        "PRelu" => |x: i64, s: i64| if x < 0 { x * s } else { x },
        _ => return None,
    })
}

pub fn binary_tensors(op: &str, a: &Tensor, b: &Tensor, fmod: bool) -> Result<Tensor> {
    let (da, db) = (a.dtype(), b.dtype());
    if is_int(da) && (is_int(db) || op == "Pow" && is_int(db)) {
        let f = arith_i64(op, fmod).ok_or_else(|| Error(format!("{op}: integer inputs unsupported")))?;
        let (shape, v) = binary_map(&a.i64s(), &a.shape, &b.i64s(), &b.shape, f)?;
        let t = Tensor::i64(&shape, v);
        return Ok(if da == DType::I64 { t } else { t.cast(da) });
    }
    let f = arith_f32(op, fmod).ok_or_else(|| Error(format!("binary op {op} not implemented")))?;
    let (shape, v) = binary_map(&a.f32s(), &a.shape, &b.f32s(), &b.shape, f)?;
    let t = Tensor::f32(&shape, v);
    Ok(if is_int(da) { t.cast(da) } else { t })
}

/// Same-shape float fast path, multithreaded.
fn binary_same_f32(op: &str, a: &[f32], b: &[f32], threads: usize) -> Option<Vec<f32>> {
    let f = arith_f32(op, true)?;
    let mut out = a.to_vec();
    par_chunks_mut(&mut out, threads, 1 << 15, |off, ch| {
        for (i, v) in ch.iter_mut().enumerate() {
            *v = f(*v, b[off + i]);
        }
    });
    Some(out)
}

pub fn binary(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let op = node.op_type.as_str();
    if a.shape == b.shape
        && a.len() >= 1 << 16
        && op != "Mod"
        && let (Some(x), Some(y)) = (a.as_f32(), b.as_f32())
        && let Some(v) = binary_same_f32(op, x, y, ctx.threads)
    {
        return Ok(vec![Tensor::f32(&a.shape, v)]);
    }
    let fmod = node.attr_i("fmod", 0) != 0;
    Ok(vec![binary_tensors(op, a, b, fmod)?])
}

/// Max / Min / Sum / Mean over any number of inputs.
pub fn variadic(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let ins: Vec<&Tensor> = inputs.iter().flatten().copied().collect();
    if ins.is_empty() {
        return Err(Error(format!("{}: no inputs", node.op_type)));
    }
    let op = if node.op_type == "Mean" { "Sum" } else { node.op_type.as_str() };
    let mut acc = ins[0].clone();
    if ins.len() == 1 && acc.dtype() == DType::F16 {
        acc = acc.cast(DType::F32);
    }
    for t in &ins[1..] {
        acc = binary_tensors(op, &acc, t, false)?;
    }
    if node.op_type == "Mean" {
        let n = ins.len() as f32;
        let v: Vec<f32> = acc.f32s().iter().map(|x| x / n).collect();
        acc = Tensor::f32(&acc.shape, v);
    }
    Ok(vec![acc])
}

pub fn compare(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let op = node.op_type.as_str();
    let (shape, v) = if is_int(a.dtype()) && is_int(b.dtype()) || (a.dtype() == DType::Bool && b.dtype() == DType::Bool) {
        let f: fn(i64, i64) -> bool = match op {
            "Equal" => |x, y| x == y,
            "Less" => |x, y| x < y,
            "Greater" => |x, y| x > y,
            "LessOrEqual" => |x, y| x <= y,
            "GreaterOrEqual" => |x, y| x >= y,
            _ => return Err(Error(format!("compare op {op}"))),
        };
        binary_map(&a.i64s(), &a.shape, &b.i64s(), &b.shape, f)?
    } else {
        let f: fn(f32, f32) -> bool = match op {
            "Equal" => |x, y| x == y,
            "Less" => |x, y| x < y,
            "Greater" => |x, y| x > y,
            "LessOrEqual" => |x, y| x <= y,
            "GreaterOrEqual" => |x, y| x >= y,
            _ => return Err(Error(format!("compare op {op}"))),
        };
        binary_map(&a.f32s(), &a.shape, &b.f32s(), &b.shape, f)?
    };
    Ok(vec![Tensor::bool(&shape, v)])
}

pub fn logic(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let f: fn(bool, bool) -> bool = match node.op_type.as_str() {
        "And" => |x, y| x && y,
        "Or" => |x, y| x || y,
        "Xor" => |x, y| x ^ y,
        o => return Err(Error(format!("logic op {o}"))),
    };
    let (shape, v) = binary_map(&a.to_bool_vec(), &a.shape, &b.to_bool_vec(), &b.shape, f)?;
    Ok(vec![Tensor::bool(&shape, v)])
}

pub fn where_(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let c = inp(inputs, 0)?;
    let x = inp(inputs, 1)?;
    let y = inp(inputs, 2)?;
    let out = broadcast_shape(&broadcast_shape(&c.shape, &x.shape)?, &y.shape)?;
    let cv = c.to_bool_vec();
    fn go<T: crate::tensor::Elem>(out: &[usize], c: &Tensor, cv: &[bool], x: &Tensor, y: &Tensor) -> Data {
        let xv = x.elems::<T>().unwrap();
        let yv = y.elems::<T>().unwrap();
        let mut r = Vec::with_capacity(numel(out));
        for_each_bcast(out, &[&c.shape, &x.shape, &y.shape], |_, o| r.push(if cv[o[0]] { xv[o[1]] } else { yv[o[2]] }));
        T::wrap(r)
    }
    let (x, y) = if x.dtype() == y.dtype() { (x.clone(), y.clone()) } else { (x.cast(DType::F32), y.cast(DType::F32)) };
    let data = crate::dispatch_dtype!(x.dtype(), go(&out, c, &cv, &x, &y));
    Ok(vec![Tensor { shape: out, data }])
}

pub fn onnx_to_dtype(to: i64) -> Result<DType> {
    DType::from_onnx(to as i32).ok_or_else(|| Error(format!("Cast: unsupported target type {to}")))
}

pub fn cast(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let to = onnx_to_dtype(node.attr_i("to", 1))?;
    Ok(vec![x.cast(to)])
}

pub fn cast_like(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let t = inp(inputs, 1)?;
    Ok(vec![x.cast(t.dtype())])
}
