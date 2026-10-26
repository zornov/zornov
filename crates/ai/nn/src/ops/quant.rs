// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Quantization operators: QuantizeLinear, DequantizeLinear,
//! DynamicQuantizeLinear, MatMulInteger, ConvInteger, QLinearMatMul,
//! QLinearConv and the onnxruntime contrib ops MatMulIntegerToFloat and
//! DynamicQuantizeMatMul (dynamic-quantised transformer exports).

use super::nn::conv_geom;
use super::{OpCtx, inp, opt};
use crate::error::{Error, Result};
use crate::kernels::conv::ConvGeom;
use crate::kernels::gemm::gemm_i32;
use crate::onnx::Node;
use crate::tensor::{DType, Data, Tensor};

fn ints_i32(t: &Tensor) -> Vec<i32> {
    match &t.data {
        Data::U8(v) => v.iter().map(|&x| x as i32).collect(),
        Data::I8(v) => v.iter().map(|&x| x as i32).collect(),
        Data::I64(v) => v.iter().map(|&x| x as i32).collect(),
        _ => t.f32s().iter().map(|&x| x as i32).collect(),
    }
}

fn saturate(v: f32, dt: DType) -> f32 {
    match dt {
        DType::I8 => v.clamp(-128.0, 127.0),
        DType::U8 => v.clamp(0.0, 255.0),
        _ => v,
    }
}

/// Per-element parameter lookup for per-tensor or per-axis quantization params.
fn per_axis<'a>(param: &'a [f32], shape: &'a [usize], axis: usize) -> impl Fn(usize) -> f32 + 'a {
    let inner: usize = shape[axis.min(shape.len())..].iter().skip(1).product();
    let d = shape.get(axis).copied().unwrap_or(1);
    move |i| if param.len() == 1 { param[0] } else { param[(i / inner.max(1)) % d] }
}

pub fn quantize_linear(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let scale = inp(inputs, 1)?.to_f32_vec();
    let zp_t = opt(inputs, 2);
    let dt = zp_t.map(|t| t.dtype()).unwrap_or(DType::U8);
    let zp = zp_t.map(|t| t.to_f32_vec()).unwrap_or_else(|| vec![0.0]);
    let axis = super::shape_ops::norm_axis(node.attr_i("axis", 1), x.rank().max(1)).unwrap_or(0);
    let sf = per_axis(&scale, &x.shape, axis);
    let zf = per_axis(&zp, &x.shape, axis);
    let v: Vec<f32> = x.f32s().iter().enumerate().map(|(i, &e)| saturate((e / sf(i)).round_ties_even() + zf(i), dt)).collect();
    Ok(vec![Tensor::f32(&x.shape, v).cast(dt)])
}

pub fn dequantize_linear(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let scale = inp(inputs, 1)?.to_f32_vec();
    let zp = opt(inputs, 2).map(|t| t.to_f32_vec()).unwrap_or_else(|| vec![0.0]);
    let axis = super::shape_ops::norm_axis(node.attr_i("axis", 1), x.rank().max(1)).unwrap_or(0);
    let sf = per_axis(&scale, &x.shape, axis);
    let zf = per_axis(&zp, &x.shape, axis);
    let v: Vec<f32> = x.f32s().iter().enumerate().map(|(i, &e)| (e - zf(i)) * sf(i)).collect();
    Ok(vec![Tensor::f32(&x.shape, v)])
}

/// Dynamic uint8 quantization parameters (ONNX DynamicQuantizeLinear).
pub fn dynamic_params(v: &[f32]) -> (f32, u8) {
    let mn = v.iter().copied().fold(0.0f32, f32::min);
    let mx = v.iter().copied().fold(0.0f32, f32::max);
    let scale = if mx > mn { (mx - mn) / 255.0 } else { 1.0 };
    let zp = (-mn / scale).round_ties_even().clamp(0.0, 255.0) as u8;
    (scale, zp)
}

pub fn dynamic_quantize_linear(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let v = x.f32s();
    let (scale, zp) = dynamic_params(&v);
    let q: Vec<u8> = v.iter().map(|&e| ((e / scale).round_ties_even() + zp as f32).clamp(0.0, 255.0) as u8).collect();
    Ok(vec![Tensor::u8(&x.shape, q), Tensor::scalar_f32(scale), Tensor::u8(&[], vec![zp])])
}

/// Integer matmul of A [..., M, K] by B [K, N] or [..., K, N] (same batch),
/// subtracting per-tensor / per-row (A) and per-column (B) zero points.
fn int_matmul(a: &Tensor, azp: &[i32], b: &Tensor, bzp: &[i32], threads: usize) -> Result<(Vec<usize>, Vec<i32>)> {
    if a.rank() < 2 || b.rank() < 2 {
        return Err(Error("integer matmul: inputs must be at least 2-D".into()));
    }
    let (m, k) = (a.shape[a.rank() - 2], a.shape[a.rank() - 1]);
    let (k2, n) = (b.shape[b.rank() - 2], b.shape[b.rank() - 1]);
    if k != k2 {
        return Err(Error(format!("integer matmul: inner dims {k} vs {k2}")));
    }
    let abatch: usize = a.shape[..a.rank() - 2].iter().product();
    let bbatch: usize = b.shape[..b.rank() - 2].iter().product();
    if bbatch != 1 && bbatch != abatch {
        return Err(Error("integer matmul: unsupported batch broadcast".into()));
    }
    let av = ints_i32(a);
    let bv = ints_i32(b);
    let mut out = vec![0i32; abatch * m * n];
    for bi in 0..abatch {
        let ab: Vec<i32> = (0..m * k).map(|i| av[bi * m * k + i] - azp[if azp.len() == 1 { 0 } else { (i / k) % azp.len() }]).collect();
        let bo = if bbatch == 1 { 0 } else { bi * k * n };
        let mut bt = vec![0i32; n * k];
        for kk in 0..k {
            for j in 0..n {
                bt[j * k + kk] = bv[bo + kk * n + j] - bzp[if bzp.len() == 1 { 0 } else { j % bzp.len() }];
            }
        }
        gemm_i32(m, n, k, &ab, &bt, &mut out[bi * m * n..(bi + 1) * m * n], threads);
    }
    let mut shape = a.shape[..a.rank() - 2].to_vec();
    shape.extend([m, n]);
    Ok((shape, out))
}

fn zp_or_zero(t: Option<&Tensor>) -> Vec<i32> {
    t.map(ints_i32).unwrap_or_else(|| vec![0])
}

pub fn matmul_integer(ctx: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let (shape, v) = int_matmul(inp(inputs, 0)?, &zp_or_zero(opt(inputs, 2)), inp(inputs, 1)?, &zp_or_zero(opt(inputs, 3)), ctx.threads)?;
    Ok(vec![Tensor::i64(&shape, v.into_iter().map(|x| x as i64).collect())])
}

pub fn qlinear_matmul(ctx: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let a_s = inp(inputs, 1)?.f32s()[0];
    let b = inp(inputs, 3)?;
    let b_s = inp(inputs, 4)?.to_f32_vec();
    let y_s = inp(inputs, 6)?.f32s()[0];
    let y_zp_t = inp(inputs, 7)?;
    let y_zp = y_zp_t.f32s()[0];
    let (shape, acc) = int_matmul(a, &zp_or_zero(opt(inputs, 2)), b, &zp_or_zero(opt(inputs, 5)), ctx.threads)?;
    let n = *shape.last().unwrap();
    let dt = y_zp_t.dtype();
    let v: Vec<f32> = acc.iter().enumerate().map(|(i, &x)| saturate((x as f32 * a_s * b_s[if b_s.len() == 1 { 0 } else { i % n }] / y_s).round_ties_even() + y_zp, dt)).collect();
    Ok(vec![Tensor::f32(&shape, v).cast(dt)])
}

pub fn matmul_integer_to_float(ctx: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let a_s = inp(inputs, 2)?.to_f32_vec();
    let b_s = inp(inputs, 3)?.to_f32_vec();
    let (shape, acc) = int_matmul(a, &zp_or_zero(opt(inputs, 4)), b, &zp_or_zero(opt(inputs, 5)), ctx.threads)?;
    let bias = opt(inputs, 6).map(|t| t.to_f32_vec());
    let n = *shape.last().unwrap();
    let v: Vec<f32> = acc
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let j = i % n;
            x as f32 * a_s[0] * b_s[if b_s.len() == 1 { 0 } else { j }] + bias.as_ref().map(|b| b[j]).unwrap_or(0.0)
        })
        .collect();
    Ok(vec![Tensor::f32(&shape, v)])
}

pub fn dynamic_quantize_matmul(ctx: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let a = inp(inputs, 0)?;
    let b = inp(inputs, 1)?;
    let b_s = inp(inputs, 2)?.to_f32_vec();
    let av = a.f32s();
    let (sa, za) = dynamic_params(&av);
    let qa = Tensor::u8(&a.shape, av.iter().map(|&e| ((e / sa).round_ties_even() + za as f32).clamp(0.0, 255.0) as u8).collect());
    let (shape, acc) = int_matmul(&qa, &[za as i32], b, &zp_or_zero(opt(inputs, 3)), ctx.threads)?;
    let bias = opt(inputs, 4).map(|t| t.to_f32_vec());
    let n = *shape.last().unwrap();
    let v: Vec<f32> = acc
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let j = i % n;
            x as f32 * sa * b_s[if b_s.len() == 1 { 0 } else { j }] + bias.as_ref().map(|b| b[j]).unwrap_or(0.0)
        })
        .collect();
    Ok(vec![Tensor::f32(&shape, v)])
}

/// Integer convolution: x and w already have zero points subtracted.
pub fn conv_i32(x: &[i32], g: &ConvGeom, w: &[i32], threads: usize) -> Vec<i32> {
    let (icg, ocg) = (g.icg(), g.ocg());
    let isz = g.in_size();
    let osz = g.out_size();
    let kk = icg * g.kernel_size();
    let [id, ih, iw] = g.inp;
    let [_, oh, ow] = g.out;
    let [kd, kh, kw] = g.k;
    let mut out = vec![0i32; g.n * g.c_out * osz];
    let mut colt = vec![0i32; osz * kk];
    for b in 0..g.n {
        for grp in 0..g.group {
            let xg = &x[(b * g.c_in + grp * icg) * isz..][..icg * isz];
            for p in 0..osz {
                let (oz, oy, ox) = (p / (oh * ow), (p / ow) % oh, p % ow);
                let row = &mut colt[p * kk..(p + 1) * kk];
                let mut r = 0;
                for ci in 0..icg {
                    for kz in 0..kd {
                        let iz = (oz * g.stride[0] + kz * g.dil[0]) as isize - g.pad[0] as isize;
                        for ky in 0..kh {
                            let iy = (oy * g.stride[1] + ky * g.dil[1]) as isize - g.pad[1] as isize;
                            for kx in 0..kw {
                                let ix = (ox * g.stride[2] + kx * g.dil[2]) as isize - g.pad[2] as isize;
                                row[r] = if iz >= 0 && iy >= 0 && ix >= 0 && (iz as usize) < id && (iy as usize) < ih && (ix as usize) < iw {
                                    xg[ci * isz + (iz as usize * ih + iy as usize) * iw + ix as usize]
                                } else {
                                    0
                                };
                                r += 1;
                            }
                        }
                    }
                }
            }
            let wg = &w[grp * ocg * kk..(grp + 1) * ocg * kk];
            gemm_i32(ocg, osz, kk, wg, &colt, &mut out[(b * g.c_out + grp * ocg) * osz..][..ocg * osz], threads);
        }
    }
    out
}

fn sub_zp(t: &Tensor, zp: &[i32], axis_dim: Option<usize>) -> Vec<i32> {
    let v = ints_i32(t);
    if zp.len() <= 1 {
        let z = zp.first().copied().unwrap_or(0);
        return v.iter().map(|&x| x - z).collect();
    }
    // per output channel (dim 0)
    let per = v.len() / axis_dim.unwrap_or(zp.len()).max(1);
    v.iter().enumerate().map(|(i, &x)| x - zp[(i / per.max(1)) % zp.len()]).collect()
}

pub fn conv_integer(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let w = inp(inputs, 1)?;
    let (g, oshape) = conv_geom(node, &x.shape, &w.shape)?;
    let xi = sub_zp(x, &zp_or_zero(opt(inputs, 2)), None);
    let wi = sub_zp(w, &zp_or_zero(opt(inputs, 3)), Some(w.shape[0]));
    let out = conv_i32(&xi, &g, &wi, ctx.threads);
    Ok(vec![Tensor::i64(&oshape, out.into_iter().map(|v| v as i64).collect())])
}

pub fn qlinear_conv(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let xs = inp(inputs, 1)?.f32s()[0];
    let w = inp(inputs, 3)?;
    let ws = inp(inputs, 4)?.to_f32_vec();
    let ys = inp(inputs, 6)?.f32s()[0];
    let yzp_t = inp(inputs, 7)?;
    let yzp = yzp_t.f32s()[0];
    let bias = opt(inputs, 8).map(ints_i32);
    let (g, oshape) = conv_geom(node, &x.shape, &w.shape)?;
    let xi = sub_zp(x, &zp_or_zero(opt(inputs, 2)), None);
    let wi = sub_zp(w, &zp_or_zero(opt(inputs, 5)), Some(w.shape[0]));
    let acc = conv_i32(&xi, &g, &wi, ctx.threads);
    let osz = g.out_size();
    let dt = yzp_t.dtype();
    let v: Vec<f32> = acc
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let oc = (i / osz.max(1)) % g.c_out;
            let a = a + bias.as_ref().map(|b| b[oc]).unwrap_or(0);
            saturate((a as f32 * xs * ws[if ws.len() == 1 { 0 } else { oc }] / ys).round_ties_even() + yzp, dt)
        })
        .collect();
    Ok(vec![Tensor::f32(&oshape, v).cast(dt)])
}
