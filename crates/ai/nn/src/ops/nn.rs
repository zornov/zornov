// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Neural-network operators: Conv / ConvTranspose, pooling, normalisation,
//! softmax family, MatMul and Gemm.

use super::{OpCtx, Prepacked, inp, opt};
use crate::error::{Error, Result};
use crate::kernels::conv::{ConvGeom, conv_forward, conv_transpose, pack_conv_weights};
use crate::kernels::gemm::{self, PackedB, gemm_packed, gemv_packed, pack_a, pack_b, sgemm};
use crate::kernels::par::{SendPtr, par_dynamic, par_range};
use crate::onnx::Node;
use crate::tensor::{Tensor, numel};

// ------------------------------------------------------------ geometry

fn ints_or(node: &Node, name: &str, n: usize, default: i64) -> Vec<i64> {
    match node.attr_ints(name) {
        Some(v) if v.len() == n => v,
        Some(v) if v.len() == 1 => vec![v[0]; n],
        _ => vec![default; n],
    }
}

fn to3(v: &[usize], fill: usize) -> [usize; 3] {
    let mut o = [fill; 3];
    let off = 3 - v.len();
    for (i, &x) in v.iter().enumerate() {
        o[off + i] = x;
    }
    o
}

/// Resolved spatial geometry shared by conv and pooling.
pub struct Spatial {
    pub out: Vec<usize>,
    pub pad_begin: Vec<usize>,
}

/// Output size and leading padding for windows over `inp` (non-transposed).
pub fn window_geometry(node: &Node, inp: &[usize], k: &[usize], stride: &[usize], dil: &[usize], ceil_mode: bool) -> Result<Spatial> {
    let n = inp.len();
    let auto = node.attr_s_or("auto_pad", "NOTSET");
    let pads = ints_or(node, "pads", 2 * n, 0);
    let mut out = vec![0; n];
    let mut pb = vec![0; n];
    for i in 0..n {
        let eff = (k[i] - 1) * dil[i] + 1;
        match auto.as_str() {
            "SAME_UPPER" | "SAME_LOWER" => {
                let o = inp[i].div_ceil(stride[i]);
                let total = ((o - 1) * stride[i] + eff).saturating_sub(inp[i]);
                pb[i] = if auto == "SAME_UPPER" { total / 2 } else { total - total / 2 };
                out[i] = o;
            }
            "VALID" => {
                if inp[i] < eff {
                    return Err(Error(format!("{}: window {eff} larger than input {}", node.op_type, inp[i])));
                }
                out[i] = (inp[i] - eff) / stride[i] + 1;
            }
            _ => {
                let (b, e) = (pads[i].max(0) as usize, pads[i + n].max(0) as usize);
                let span = inp[i] + b + e;
                if span < eff {
                    return Err(Error(format!("{}: window {eff} larger than padded input {span}", node.op_type)));
                }
                let mut o = if ceil_mode { (span - eff).div_ceil(stride[i]) + 1 } else { (span - eff) / stride[i] + 1 };
                if ceil_mode && (o - 1) * stride[i] >= inp[i] + b {
                    o -= 1;
                }
                out[i] = o;
                pb[i] = b;
            }
        }
    }
    Ok(Spatial { out, pad_begin: pb })
}

pub fn conv_geom(node: &Node, x: &[usize], w: &[usize]) -> Result<(ConvGeom, Vec<usize>)> {
    if x.len() < 3 || x.len() > 5 || w.len() != x.len() {
        return Err(Error(format!("Conv: unsupported shapes x={x:?} w={w:?}")));
    }
    let sr = x.len() - 2;
    let group = node.attr_i("group", 1).max(1) as usize;
    let k: Vec<usize> = w[2..].to_vec();
    let stride: Vec<usize> = ints_or(node, "strides", sr, 1).iter().map(|&v| v.max(1) as usize).collect();
    let dil: Vec<usize> = ints_or(node, "dilations", sr, 1).iter().map(|&v| v.max(1) as usize).collect();
    if x[1] != w[1] * group {
        return Err(Error(format!("Conv: input channels {} != weight {} × group {group}", x[1], w[1])));
    }
    let sp = window_geometry(node, &x[2..], &k, &stride, &dil, false)?;
    let g = ConvGeom {
        n: x[0],
        c_in: x[1],
        c_out: w[0],
        group,
        inp: to3(&x[2..], 1),
        out: to3(&sp.out, 1),
        k: to3(&k, 1),
        stride: to3(&stride, 1),
        dil: to3(&dil, 1),
        pad: to3(&sp.pad_begin, 0),
    };
    let mut oshape = vec![x[0], w[0]];
    oshape.extend_from_slice(&sp.out);
    Ok((g, oshape))
}

pub fn conv(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let w = inp(inputs, 1)?;
    let b = opt(inputs, 2);
    let (g, oshape) = conv_geom(node, &x.shape, &w.shape)?;
    let bias = b.map(|t| t.to_f32_vec());
    let (wv, packed_cache) = match ctx.cache {
        Some(Prepacked::Conv { w, packed }) => (std::borrow::Cow::Borrowed(w.as_slice()), Some(packed)),
        _ => (w.f32s(), None),
    };
    let depthwise = g.icg() == 1 && g.ocg() == 1 && g.k[0] == 1 && g.inp[0] == 1;
    let owned;
    let packed = match packed_cache {
        Some(p) => p.as_slice(),
        None if depthwise => &[][..],
        None => {
            owned = pack_conv_weights(&wv, &g);
            owned.as_slice()
        }
    };
    let out = conv_forward(&x.f32s(), &g, &wv, packed, bias.as_deref(), ctx.threads);
    Ok(vec![Tensor::f32(&oshape, out)])
}

pub fn conv_transpose_op(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let w = inp(inputs, 1)?;
    let b = opt(inputs, 2);
    let xs = &x.shape;
    if xs.len() < 3 || xs.len() > 5 || w.rank() != xs.len() {
        return Err(Error(format!("ConvTranspose: unsupported shapes {xs:?} {:?}", w.shape)));
    }
    let sr = xs.len() - 2;
    let group = node.attr_i("group", 1).max(1) as usize;
    let k: Vec<usize> = w.shape[2..].to_vec();
    let stride: Vec<usize> = ints_or(node, "strides", sr, 1).iter().map(|&v| v.max(1) as usize).collect();
    let dil: Vec<usize> = ints_or(node, "dilations", sr, 1).iter().map(|&v| v.max(1) as usize).collect();
    let opad: Vec<usize> = ints_or(node, "output_padding", sr, 0).iter().map(|&v| v.max(0) as usize).collect();
    let pads = ints_or(node, "pads", 2 * sr, 0);
    let auto = node.attr_s_or("auto_pad", "NOTSET");
    let out_shape_attr = node.attr_ints("output_shape");
    let mut out = vec![0usize; sr];
    let mut pb = vec![0usize; sr];
    for i in 0..sr {
        let full = stride[i] * (xs[2 + i] - 1) + opad[i] + (k[i] - 1) * dil[i] + 1;
        if let Some(os) = &out_shape_attr {
            let o = os[os.len() - sr + i] as usize;
            let total = full.saturating_sub(o);
            pb[i] = if auto == "SAME_UPPER" { total / 2 } else { total - total / 2 };
            out[i] = o;
        } else if auto == "SAME_UPPER" || auto == "SAME_LOWER" {
            let o = xs[2 + i] * stride[i];
            let total = full.saturating_sub(o);
            pb[i] = if auto == "SAME_UPPER" { total / 2 } else { total - total / 2 };
            out[i] = o;
        } else {
            let (b0, e0) = (pads[i].max(0) as usize, pads[i + sr].max(0) as usize);
            out[i] = full.saturating_sub(b0 + e0);
            pb[i] = b0;
        }
    }
    let c_out = w.shape[1] * group;
    let g = ConvGeom { n: xs[0], c_in: xs[1], c_out, group, inp: to3(&xs[2..], 1), out: to3(&out, 1), k: to3(&k, 1), stride: to3(&stride, 1), dil: to3(&dil, 1), pad: to3(&pb, 0) };
    let bias = b.map(|t| t.to_f32_vec());
    let res = conv_transpose(&x.f32s(), &g, &w.f32s(), bias.as_deref(), ctx.threads);
    let mut oshape = vec![xs[0], c_out];
    oshape.extend_from_slice(&out);
    Ok(vec![Tensor::f32(&oshape, res)])
}

// ------------------------------------------------------------ pooling

pub fn pool(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let xs = &x.shape;
    if xs.len() < 3 || xs.len() > 5 {
        return Err(Error(format!("{}: unsupported rank {}", node.op_type, xs.len())));
    }
    let sr = xs.len() - 2;
    let k: Vec<usize> = node.attr_ints("kernel_shape").ok_or_else(|| Error("pool: kernel_shape required".into()))?.iter().map(|&v| v as usize).collect();
    let stride: Vec<usize> = ints_or(node, "strides", sr, 1).iter().map(|&v| v.max(1) as usize).collect();
    let dil: Vec<usize> = ints_or(node, "dilations", sr, 1).iter().map(|&v| v.max(1) as usize).collect();
    let ceil = node.attr_i("ceil_mode", 0) != 0;
    let sp = window_geometry(node, &xs[2..], &k, &stride, &dil, ceil)?;
    let pads = ints_or(node, "pads", 2 * sr, 0);
    let pad_end: Vec<usize> = (0..sr).map(|i| if node.attr_s_or("auto_pad", "NOTSET") == "NOTSET" { pads[i + sr].max(0) as usize } else { 0 }).collect();
    let (i3, o3, k3, s3, d3, p3, pe3) = (to3(&xs[2..], 1), to3(&sp.out, 1), to3(&k, 1), to3(&stride, 1), to3(&dil, 1), to3(&sp.pad_begin, 0), to3(&pad_end, 0));
    let kind = node.op_type.as_str();
    let include_pad = node.attr_i("count_include_pad", 0) != 0;
    let p_norm = node.attr_i("p", 2) as f32;
    let planes = xs[0] * xs[1];
    let isz: usize = i3.iter().product();
    let osz: usize = o3.iter().product();
    let xv = x.f32s();
    let mut out = vec![0.0f32; planes * osz];
    let want_idx = kind == "MaxPool" && node.outputs.len() > 1 && !node.outputs[1].is_empty();
    let mut idx_out = if want_idx { vec![0i64; planes * osz] } else { Vec::new() };
    let op = SendPtr(out.as_mut_ptr());
    let ip = SendPtr(idx_out.as_mut_ptr());
    let storage_col = node.attr_i("storage_order", 0) != 0;
    par_dynamic(planes, if planes * osz > 4096 { ctx.threads } else { 1 }, |pl| {
        let xin = &xv[pl * isz..(pl + 1) * isz];
        for oz in 0..o3[0] {
            for oy in 0..o3[1] {
                for ox in 0..o3[2] {
                    let mut acc = if kind == "MaxPool" { f32::NEG_INFINITY } else { 0.0 };
                    let mut best = 0usize;
                    let mut cnt = 0usize;
                    let mut cnt_pad = 0usize;
                    for kz in 0..k3[0] {
                        let iz = (oz * s3[0] + kz * d3[0]) as isize - p3[0] as isize;
                        for ky in 0..k3[1] {
                            let iy = (oy * s3[1] + ky * d3[1]) as isize - p3[1] as isize;
                            for kx in 0..k3[2] {
                                let ix = (ox * s3[2] + kx * d3[2]) as isize - p3[2] as isize;
                                let inside_pad = iz < (i3[0] + pe3[0]) as isize && iy < (i3[1] + pe3[1]) as isize && ix < (i3[2] + pe3[2]) as isize;
                                if inside_pad {
                                    cnt_pad += 1;
                                }
                                if iz < 0 || iy < 0 || ix < 0 || iz as usize >= i3[0] || iy as usize >= i3[1] || ix as usize >= i3[2] {
                                    continue;
                                }
                                let pos = (iz as usize * i3[1] + iy as usize) * i3[2] + ix as usize;
                                let v = xin[pos];
                                cnt += 1;
                                match kind {
                                    "MaxPool" => {
                                        if v > acc {
                                            acc = v;
                                            best = pos;
                                        }
                                    }
                                    "LpPool" => acc += v.abs().powf(p_norm),
                                    _ => acc += v,
                                }
                            }
                        }
                    }
                    let o = (oz * o3[1] + oy) * o3[2] + ox;
                    let val = match kind {
                        "MaxPool" => acc,
                        "LpPool" => acc.powf(1.0 / p_norm),
                        _ => {
                            let c = if include_pad { cnt_pad } else { cnt };
                            if c > 0 { acc / c as f32 } else { 0.0 }
                        }
                    };
                    // SAFETY: plane `pl` is written by this item only.
                    unsafe { *op.add(pl * osz + o) = val };
                    if want_idx {
                        let flat = if storage_col {
                            // column-major index within the plane
                            let (z, rem) = (best / (i3[1] * i3[2]), best % (i3[1] * i3[2]));
                            let (y, xx) = (rem / i3[2], rem % i3[2]);
                            (xx * i3[1] + y) * i3[0] + z
                        } else {
                            best
                        };
                        unsafe { *ip.add(pl * osz + o) = (pl * isz + flat) as i64 };
                    }
                }
            }
        }
    });
    let mut oshape = vec![xs[0], xs[1]];
    oshape.extend_from_slice(&sp.out);
    let mut res = vec![Tensor::f32(&oshape, out)];
    if want_idx {
        res.push(Tensor::i64(&oshape, idx_out));
    }
    Ok(res)
}

pub fn global_pool(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    if x.rank() < 2 {
        return Err(Error("global pool: rank < 2".into()));
    }
    let planes = x.shape[0] * x.shape[1];
    let sz: usize = x.shape[2..].iter().product();
    let v = x.f32s();
    let p = node.attr_i("p", 2) as f32;
    let out: Vec<f32> = (0..planes)
        .map(|i| {
            let s = &v[i * sz..(i + 1) * sz];
            match node.op_type.as_str() {
                "GlobalMaxPool" => s.iter().copied().fold(f32::NEG_INFINITY, f32::max),
                "GlobalLpPool" => s.iter().map(|x| x.abs().powf(p)).sum::<f32>().powf(1.0 / p),
                _ => s.iter().map(|&x| x as f64).sum::<f64>() as f32 / sz.max(1) as f32,
            }
        })
        .collect();
    let mut shape = x.shape.clone();
    for d in shape[2..].iter_mut() {
        *d = 1;
    }
    Ok(vec![Tensor::f32(&shape, out)])
}

// ------------------------------------------------------------ normalisation

pub fn batch_norm(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let (scale, bias, mean, var) = (inp(inputs, 1)?.f32s(), inp(inputs, 2)?.f32s(), inp(inputs, 3)?.f32s(), inp(inputs, 4)?.f32s());
    let eps = node.attr_f("epsilon", 1e-5);
    let c = if x.rank() > 1 { x.shape[1] } else { 1 };
    let inner: usize = x.shape.iter().skip(2).product();
    let mut v = x.to_f32_vec();
    let coef: Vec<(f32, f32)> = (0..c)
        .map(|i| {
            let s = scale[i] / (var[i] + eps).sqrt();
            (s, bias[i] - mean[i] * s)
        })
        .collect();
    for (p, ch) in v.chunks_mut(inner.max(1)).enumerate() {
        let (s, b) = coef[p % c];
        for e in ch {
            *e = *e * s + b;
        }
    }
    Ok(vec![Tensor::f32(&x.shape, v)])
}

pub fn instance_norm(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let (scale, bias) = (inp(inputs, 1)?.f32s(), inp(inputs, 2)?.f32s());
    let eps = node.attr_f("epsilon", 1e-5);
    let c = x.shape[1];
    let inner: usize = x.shape.iter().skip(2).product();
    let mut v = x.to_f32_vec();
    for (p, ch) in v.chunks_mut(inner.max(1)).enumerate() {
        let (m, var) = mean_var(ch);
        let s = scale[p % c] / (var + eps).sqrt();
        let b = bias[p % c];
        for e in ch {
            *e = (*e - m) * s + b;
        }
    }
    Ok(vec![Tensor::f32(&x.shape, v)])
}

/// Mean and (biased) variance with f64 accumulation.
pub fn mean_var(v: &[f32]) -> (f32, f32) {
    let n = v.len().max(1) as f64;
    let m = v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = v.iter().map(|&x| (x as f64 - m) * (x as f64 - m)).sum::<f64>() / n;
    (m as f32, var as f32)
}

pub fn layer_norm(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let scale = inp(inputs, 1)?.to_f32_vec();
    let bias = opt(inputs, 2).map(|t| t.to_f32_vec());
    let eps = node.attr_f("epsilon", 1e-5);
    let ax = super::shape_ops::norm_axis(node.attr_i("axis", -1), x.rank())?;
    let inner: usize = x.shape[ax..].iter().product();
    let rows = x.len() / inner.max(1);
    let mut v = x.to_f32_vec();
    let mut means = vec![0.0f32; rows];
    let mut invs = vec![0.0f32; rows];
    let (vp, mp, ip) = (SendPtr(v.as_mut_ptr()), SendPtr(means.as_mut_ptr()), SendPtr(invs.as_mut_ptr()));
    let sl = scale.len();
    let bl = bias.as_ref().map(|b| b.len()).unwrap_or(0);
    par_range(rows, if x.len() > 1 << 15 { ctx.threads } else { 1 }, |r0, r1| {
        for r in r0..r1 {
            // SAFETY: rows are disjoint.
            let row = unsafe { std::slice::from_raw_parts_mut(vp.add(r * inner), inner) };
            let (m, var) = mean_var(row);
            let inv = 1.0 / (var + eps).sqrt();
            for (j, e) in row.iter_mut().enumerate() {
                let mut y = (*e - m) * inv * scale[j % sl];
                if let Some(b) = &bias {
                    y += b[j % bl];
                }
                *e = y;
            }
            unsafe {
                *mp.add(r) = m;
                *ip.add(r) = inv;
            }
        }
    });
    let mut stat_shape = x.shape.clone();
    for d in stat_shape[ax..].iter_mut() {
        *d = 1;
    }
    Ok(vec![Tensor::f32(&x.shape, v), Tensor::f32(&stat_shape, means), Tensor::f32(&stat_shape, invs)])
}

pub fn group_norm(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let scale = inp(inputs, 1)?.to_f32_vec();
    let bias = inp(inputs, 2)?.to_f32_vec();
    let groups = node.attr_i("num_groups", 1).max(1) as usize;
    let eps = node.attr_f("epsilon", 1e-5);
    let (n, c) = (x.shape[0], x.shape[1]);
    if c % groups != 0 {
        return Err(Error("GroupNormalization: channels not divisible by groups".into()));
    }
    let inner: usize = x.shape.iter().skip(2).product();
    let cpg = c / groups;
    let per_channel = scale.len() == c;
    let mut v = x.to_f32_vec();
    for b in 0..n {
        for g in 0..groups {
            let s = &mut v[(b * c + g * cpg) * inner..(b * c + (g + 1) * cpg) * inner];
            let (m, var) = mean_var(s);
            let inv = 1.0 / (var + eps).sqrt();
            for (i, e) in s.iter_mut().enumerate() {
                let ch = g * cpg + i / inner.max(1);
                let (sc, bi) = if per_channel { (scale[ch], bias[ch]) } else { (scale[g], bias[g]) };
                *e = (*e - m) * inv * sc + bi;
            }
        }
    }
    Ok(vec![Tensor::f32(&x.shape, v)])
}

pub fn lp_norm(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let ax = super::shape_ops::norm_axis(node.attr_i("axis", -1), x.rank())?;
    let p = node.attr_i("p", 2);
    let (outer, d, inner) = split3(&x.shape, ax);
    let mut v = x.to_f32_vec();
    for o in 0..outer {
        for i in 0..inner {
            let mut s = 0.0f32;
            for k in 0..d {
                let e = v[(o * d + k) * inner + i];
                s += if p == 1 { e.abs() } else { e * e };
            }
            let nrm = if p == 1 { s } else { s.sqrt() };
            if nrm > 0.0 {
                for k in 0..d {
                    v[(o * d + k) * inner + i] /= nrm;
                }
            }
        }
    }
    Ok(vec![Tensor::f32(&x.shape, v)])
}

pub fn split3(shape: &[usize], ax: usize) -> (usize, usize, usize) {
    (shape[..ax].iter().product(), shape[ax], shape[ax + 1..].iter().product())
}

// ------------------------------------------------------------ softmax family

pub fn softmax(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let legacy = ctx.opset < 13;
    let axis = super::shape_ops::norm_axis(node.attr_i("axis", if legacy { 1 } else { -1 }), r.max(1))?;
    let (outer, d, inner) = if r == 0 {
        (1, 1, 1)
    } else if legacy {
        (x.shape[..axis].iter().product(), x.shape[axis..].iter().product(), 1)
    } else {
        split3(&x.shape, axis)
    };
    let kind = node.op_type.as_str();
    let mut v = x.to_f32_vec();
    let vp = SendPtr(v.as_mut_ptr());
    let threads = if x.len() > 1 << 14 { ctx.threads } else { 1 };
    par_range(outer, threads, |o0, o1| {
        let mut tmp = vec![0.0f32; d];
        for o in o0..o1 {
            for i in 0..inner {
                // gather the strided row
                for k in 0..d {
                    tmp[k] = unsafe { *vp.add((o * d + k) * inner + i) };
                }
                let mx = tmp.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                match kind {
                    "Hardmax" => {
                        let am = tmp.iter().position(|&e| e == mx).unwrap_or(0);
                        for (k, t) in tmp.iter_mut().enumerate() {
                            *t = if k == am { 1.0 } else { 0.0 };
                        }
                    }
                    _ => {
                        let mut s = 0.0f32;
                        for t in tmp.iter_mut() {
                            *t = (*t - mx).exp();
                            s += *t;
                        }
                        if kind == "LogSoftmax" {
                            let ls = s.ln();
                            for k in 0..d {
                                tmp[k] = unsafe { *vp.add((o * d + k) * inner + i) } - mx - ls;
                            }
                        } else {
                            let inv = 1.0 / s;
                            for t in tmp.iter_mut() {
                                *t *= inv;
                            }
                        }
                    }
                }
                for k in 0..d {
                    // SAFETY: rows (o, i) are disjoint between workers.
                    unsafe { *vp.add((o * d + k) * inner + i) = tmp[k] };
                }
            }
        }
    });
    Ok(vec![Tensor::f32(&x.shape, v)])
}

// ------------------------------------------------------------ MatMul / Gemm

/// Batched matmul with numpy semantics on f32 data.
pub fn matmul_f32(a: &[f32], ash: &[usize], b: &[f32], bsh: &[usize], packed_b: Option<&PackedB>, threads: usize) -> Result<(Vec<usize>, Vec<f32>)> {
    let mut ash = ash.to_vec();
    let mut bsh = bsh.to_vec();
    let a1 = ash.len() == 1;
    let b1 = bsh.len() == 1;
    if a1 {
        ash.insert(0, 1);
    }
    if b1 {
        bsh.push(1);
    }
    let (m, k) = (ash[ash.len() - 2], ash[ash.len() - 1]);
    let (k2, n) = (bsh[bsh.len() - 2], bsh[bsh.len() - 1]);
    if k != k2 {
        return Err(Error(format!("MatMul: inner dims differ {ash:?} x {bsh:?}")));
    }
    let abatch = &ash[..ash.len() - 2];
    let bbatch = &bsh[..bsh.len() - 2];
    let batch = super::elementwise::broadcast_shape(abatch, bbatch)?;
    let nb = numel(&batch);
    let mut oshape = batch.clone();
    if !a1 {
        oshape.push(m);
    }
    if !b1 {
        oshape.push(n);
    }
    let mut out = vec![0.0f32; nb * m * n];
    // B shared by all batches: fold A's batch into M.
    if numel(bbatch) == 1 && numel(abatch) == nb {
        let mm = nb * m;
        if let Some(pb) = packed_b {
            if mm == 1 {
                gemv_packed(a, pb, &mut out, threads);
            } else {
                let pa = pack_a(mm, k, a, k, 1);
                gemm_packed(&pa, pb, &mut out, n, threads);
            }
        } else {
            sgemm(mm, n, k, a, k, 1, b, n, 1, &mut out, n, true, threads);
        }
        return Ok((oshape, out));
    }
    let ast = super::elementwise::bstrides(abatch, &batch);
    let bst = super::elementwise::bstrides(bbatch, &batch);
    let offs: Vec<(usize, usize)> = (0..nb)
        .map(|i| {
            let (mut ia, mut ib, mut rem) = (0, 0, i);
            for d in (0..batch.len()).rev() {
                let c = rem % batch[d];
                rem /= batch[d];
                ia += c * ast[d];
                ib += c * bst[d];
            }
            (ia * m * k, ib * k * n)
        })
        .collect();
    let per = 2.0 * (m * n * k) as f64;
    let op = SendPtr(out.as_mut_ptr());
    if per < 2.0e6 && nb > 1 {
        par_dynamic(nb, threads, |i| {
            let (ia, ib) = offs[i];
            // SAFETY: each batch writes its own m*n block.
            let o = unsafe { std::slice::from_raw_parts_mut(op.add(i * m * n), m * n) };
            sgemm(m, n, k, &a[ia..], k, 1, &b[ib..], n, 1, o, n, true, 1);
        });
    } else {
        for (i, &(ia, ib)) in offs.iter().enumerate() {
            sgemm(m, n, k, &a[ia..], k, 1, &b[ib..], n, 1, &mut out[i * m * n..(i + 1) * m * n], n, true, threads);
        }
    }
    Ok((oshape, out))
}

pub fn matmul(ctx: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let pb = match ctx.cache {
        Some(Prepacked::B(p)) => Some(p),
        _ => None,
    };
    let bv = if pb.is_some() { std::borrow::Cow::Borrowed(&[][..]) } else { b.f32s() };
    let (shape, v) = matmul_f32(&a.f32s(), &a.shape, &bv, &b.shape, pb, ctx.threads)?;
    Ok(vec![Tensor::f32(&shape, v)])
}

/// Packs op(B) for Gemm (`trans` = transB).
pub fn pack_gemm_b(b: &Tensor, trans: bool, threads: usize) -> PackedB {
    let v = b.f32s();
    let (r, c) = (b.shape[0], b.shape[1]);
    if trans { pack_b(c, r, &v, 1, c, threads) } else { pack_b(r, c, &v, c, 1, threads) }
}

pub fn gemm_op(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let c = opt(inputs, 2);
    let ta = node.attr_i("transA", 0) != 0;
    let tb = node.attr_i("transB", 0) != 0;
    let alpha = node.attr_f("alpha", 1.0);
    let beta = node.attr_f("beta", 1.0);
    if a.rank() != 2 || b.rank() != 2 {
        return Err(Error(format!("Gemm: needs 2-D inputs, got {:?} {:?}", a.shape, b.shape)));
    }
    let (m, k) = if ta { (a.shape[1], a.shape[0]) } else { (a.shape[0], a.shape[1]) };
    let n = if tb { b.shape[0] } else { b.shape[1] };
    let kb = if tb { b.shape[1] } else { b.shape[0] };
    if k != kb {
        return Err(Error(format!("Gemm: inner dims differ {:?} {:?}", a.shape, b.shape)));
    }
    let mut out = vec![0.0f32; m * n];
    let av = a.f32s();
    let (ars, acs) = if ta { (1, m) } else { (k, 1) };
    match ctx.cache {
        Some(Prepacked::B(pb)) => {
            if m == 1 && !ta {
                gemv_packed(&av, pb, &mut out, ctx.threads);
            } else {
                let pa = pack_a(m, k, &av, ars, acs);
                gemm_packed(&pa, pb, &mut out, n, ctx.threads);
            }
        }
        _ => {
            let bv = b.f32s();
            let (brs, bcs) = if tb { (1, k) } else { (n, 1) };
            sgemm(m, n, k, &av, ars, acs, &bv, brs, bcs, &mut out, n, true, ctx.threads);
        }
    }
    if alpha != 1.0 {
        out.iter_mut().for_each(|v| *v *= alpha);
    }
    if let Some(c) = c
        && beta != 0.0
    {
        let cv = c.f32s();
        let (_, add) = super::elementwise::binary_map(&out, &[m, n], &cv, &c.shape, |x, y| x + beta * y)?;
        out = add;
    }
    let _ = gemm::MR;
    Ok(vec![Tensor::f32(&[m, n], out)])
}
