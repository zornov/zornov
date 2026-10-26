// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Resize / Upsample (separable nearest, n-linear and cubic interpolation with
//! every ONNX coordinate-transformation mode, antialias, exclude_outside,
//! tf_crop_and_resize) and GridSample (2-D, bilinear / nearest / bicubic,
//! zeros / border / reflection padding, align_corners).

use super::{OpCtx, inp, opt};
use crate::error::{Error, Result};
use crate::kernels::par::{SendPtr, par_dynamic};
use crate::onnx::Node;
use crate::tensor::Tensor;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Nearest,
    Linear,
    Cubic,
}

#[derive(Clone, Debug)]
pub struct ResizeParams {
    pub mode: Mode,
    pub coord: String,
    pub nearest: String,
    pub cubic_a: f32,
    pub exclude_outside: bool,
    pub extrapolation: f32,
    pub antialias: bool,
}

impl ResizeParams {
    pub fn simple(mode: Mode, coord: &str) -> Self {
        ResizeParams { mode, coord: coord.into(), nearest: "round_prefer_floor".into(), cubic_a: -0.75, exclude_outside: false, extrapolation: 0.0, antialias: false }
    }
}

/// Interpolation taps for one axis.
struct AxisTaps {
    taps: usize,
    idx: Vec<usize>,
    w: Vec<f32>,
    /// false where tf_crop_and_resize extrapolates.
    valid: Vec<bool>,
}

fn cubic_coeff(x: f32, a: f32) -> f32 {
    let x = x.abs();
    let (x2, x3) = (x * x, x * x * x);
    if x <= 1.0 {
        (a + 2.0) * x3 - (a + 3.0) * x2 + 1.0
    } else if x < 2.0 {
        a * x3 - 5.0 * a * x2 + 8.0 * a * x - 4.0 * a
    } else {
        0.0
    }
}

fn original_coord(p: &ResizeParams, x: f32, scale: f32, in_len: usize, out_len: usize, roi: (f32, f32)) -> f32 {
    let (inl, outl) = (in_len as f32, out_len as f32);
    match p.coord.as_str() {
        "asymmetric" => x / scale,
        "align_corners" => {
            if out_len == 1 {
                0.0
            } else {
                x * (inl - 1.0) / (outl - 1.0)
            }
        }
        "pytorch_half_pixel" => {
            if out_len > 1 {
                (x + 0.5) / scale - 0.5
            } else {
                0.0
            }
        }
        "tf_half_pixel_for_nearest" => (x + 0.5) / scale,
        "half_pixel_symmetric" => {
            let adj = outl / (scale * inl);
            let center = inl / 2.0;
            let offset = center * (1.0 - adj);
            offset + (x + 0.5) / scale - 0.5
        }
        "tf_crop_and_resize" => {
            let (s, e) = roi;
            if out_len > 1 {
                s * (inl - 1.0) + x * (e - s) * (inl - 1.0) / (outl - 1.0)
            } else {
                0.5 * (s + e) * (inl - 1.0)
            }
        }
        _ => (x + 0.5) / scale - 0.5, // half_pixel
    }
}

fn nearest_index(p: &ResizeParams, x: f32, scale: f32) -> i64 {
    match p.nearest.as_str() {
        "floor" => x.floor() as i64,
        "ceil" => x.ceil() as i64,
        "round_prefer_ceil" => x.round() as i64,
        "simple" => {
            if scale < 1.0 {
                x.ceil() as i64
            } else {
                x.floor() as i64
            }
        }
        _ => {
            // round_prefer_floor (exact half test, like onnxruntime)
            if x == x.floor() + 0.5 { x.floor() as i64 } else { x.round() as i64 }
        }
    }
}

fn axis_taps(p: &ResizeParams, in_len: usize, out_len: usize, scale: f32, roi: (f32, f32)) -> AxisTaps {
    let crop = p.coord == "tf_crop_and_resize";
    let s_eff = if p.antialias && scale < 1.0 { scale } else { 1.0 };
    let (start, count) = match p.mode {
        Mode::Nearest => (0i64, 1usize),
        Mode::Linear => {
            let st = (-1.0 / s_eff).floor() as i64 + 1;
            (st, (2 - 2 * st) as usize)
        }
        Mode::Cubic => {
            let st = (-2.0 / s_eff).floor() as i64 + 1;
            (st, (2 - 2 * st) as usize)
        }
    };
    let mut t = AxisTaps { taps: count, idx: Vec::with_capacity(out_len * count), w: Vec::with_capacity(out_len * count), valid: Vec::with_capacity(out_len) };
    let last = in_len as i64 - 1;
    for o in 0..out_len {
        let x = original_coord(p, o as f32, scale, in_len, out_len, roi);
        t.valid.push(!crop || (x >= 0.0 && x <= last as f32));
        if p.mode == Mode::Nearest {
            let i = nearest_index(p, x, scale).clamp(0, last.max(0));
            t.idx.push(i as usize);
            t.w.push(1.0);
            continue;
        }
        let xf = x.floor();
        let ratio = x - xf;
        let base = xf as i64;
        let mut ws: Vec<f32> = (0..count as i64)
            .map(|k| {
                let arg = ((start + k) as f32 - ratio) * s_eff;
                match p.mode {
                    Mode::Linear => (1.0 - arg.abs()).max(0.0),
                    _ => cubic_coeff(arg, p.cubic_a),
                }
            })
            .collect();
        let ids: Vec<i64> = (0..count as i64).map(|k| base + start + k).collect();
        if p.exclude_outside {
            for (w, &i) in ws.iter_mut().zip(&ids) {
                if i < 0 || i > last {
                    *w = 0.0;
                }
            }
        }
        let sum: f32 = ws.iter().sum();
        if sum != 0.0 {
            ws.iter_mut().for_each(|w| *w /= sum);
        }
        for (w, i) in ws.into_iter().zip(ids) {
            t.idx.push(i.clamp(0, last.max(0)) as usize);
            t.w.push(w);
        }
    }
    t
}

fn apply_axis(v: &[f32], shape: &[usize], ax: usize, t: &AxisTaps, threads: usize) -> Vec<f32> {
    let outer: usize = shape[..ax].iter().product();
    let d = shape[ax];
    let inner: usize = shape[ax + 1..].iter().product();
    let ol = t.valid.len();
    let mut out = vec![0.0f32; outer * ol * inner];
    let op = SendPtr(out.as_mut_ptr());
    let nt = t.taps;
    par_dynamic(outer, if out.len() > 1 << 15 { threads } else { 1 }, |o| {
        let src = &v[o * d * inner..(o + 1) * d * inner];
        // SAFETY: each outer slice is written by one item.
        let dst = unsafe { std::slice::from_raw_parts_mut(op.add(o * ol * inner), ol * inner) };
        for j in 0..ol {
            let row = &mut dst[j * inner..(j + 1) * inner];
            for k in 0..nt {
                let w = t.w[j * nt + k];
                if w == 0.0 {
                    continue;
                }
                let s = &src[t.idx[j * nt + k] * inner..][..inner];
                if inner == 1 {
                    row[0] += w * s[0];
                } else {
                    for (r, &x) in row.iter_mut().zip(s) {
                        *r += w * x;
                    }
                }
            }
        }
    });
    out
}

/// Resizes `x` (any rank) to `out_shape`; `scales[i]` is the scale used in the
/// coordinate transform of axis i, `roi` the crop box per axis.
pub fn resize_f32(x: &[f32], shape: &[usize], out_shape: &[usize], scales: &[f32], roi: &[(f32, f32)], p: &ResizeParams, threads: usize) -> Vec<f32> {
    let mut cur = x.to_vec();
    let mut sh = shape.to_vec();
    let crop = p.coord == "tf_crop_and_resize";
    let mut invalid: Vec<(usize, Vec<bool>)> = Vec::new();
    for ax in 0..shape.len() {
        if out_shape[ax] == shape[ax] && scales[ax] == 1.0 && !crop {
            continue;
        }
        let t = axis_taps(p, shape[ax], out_shape[ax], scales[ax], roi[ax]);
        cur = apply_axis(&cur, &sh, ax, &t, threads);
        sh[ax] = out_shape[ax];
        if crop && t.valid.iter().any(|v| !v) {
            invalid.push((ax, t.valid));
        }
    }
    if !invalid.is_empty() {
        let st = crate::tensor::strides(&sh);
        for (i, v) in cur.iter_mut().enumerate() {
            if invalid.iter().any(|(ax, val)| !val[(i / st[*ax]) % sh[*ax]]) {
                *v = p.extrapolation;
            }
        }
    }
    cur
}

fn nonempty<'a>(inputs: &[Option<&'a Tensor>], i: usize) -> Option<&'a Tensor> {
    opt(inputs, i).filter(|t| !t.is_empty())
}

pub fn resize(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let upsample = node.op_type == "Upsample";
    let legacy = upsample || ctx.opset < 11;
    let mode = match node.attr_s_or("mode", "nearest").as_str() {
        "nearest" => Mode::Nearest,
        "linear" | "bilinear" => Mode::Linear,
        "cubic" | "bicubic" => Mode::Cubic,
        m => return Err(Error(format!("Resize: unknown mode {m}"))),
    };
    let p = ResizeParams {
        mode,
        coord: if legacy { "asymmetric".into() } else { node.attr_s_or("coordinate_transformation_mode", "half_pixel") },
        nearest: if legacy { "simple".into() } else { node.attr_s_or("nearest_mode", "round_prefer_floor") },
        cubic_a: node.attr_f("cubic_coeff_a", -0.75),
        exclude_outside: node.attr_i("exclude_outside", 0) != 0,
        extrapolation: node.attr_f("extrapolation_value", 0.0),
        antialias: node.attr_i("antialias", 0) != 0,
    };
    let (scales_t, sizes_t, roi_t) = if legacy {
        (nonempty(inputs, 1), None, None)
    } else {
        (nonempty(inputs, 2), nonempty(inputs, 3), nonempty(inputs, 1))
    };
    let axes: Vec<usize> = match node.attr_ints("axes") {
        Some(a) => a.iter().map(|&v| super::shape_ops::norm_axis(v, r)).collect::<Result<_>>()?,
        None => (0..r).collect(),
    };
    let mut scales = vec![1.0f32; r];
    let mut out = x.shape.clone();
    if let Some(s) = sizes_t {
        let sv = s.to_i64_vec();
        if sv.len() != axes.len() {
            return Err(Error("Resize: sizes length mismatch".into()));
        }
        let policy = node.attr_s_or("keep_aspect_ratio_policy", "stretch");
        if policy == "stretch" {
            for (i, &a) in axes.iter().enumerate() {
                out[a] = sv[i] as usize;
                scales[a] = sv[i] as f32 / x.shape[a] as f32;
            }
        } else {
            let ratios: Vec<f32> = axes.iter().enumerate().map(|(i, &a)| sv[i] as f32 / x.shape[a] as f32).collect();
            let s = if policy == "not_larger" { ratios.iter().copied().fold(f32::INFINITY, f32::min) } else { ratios.iter().copied().fold(0.0, f32::max) };
            for &a in &axes {
                out[a] = (x.shape[a] as f32 * s).round_ties_even() as usize;
                scales[a] = s;
            }
        }
    } else if let Some(s) = scales_t {
        let sv = s.to_f32_vec();
        if sv.len() != axes.len() {
            return Err(Error("Resize: scales length mismatch".into()));
        }
        for (i, &a) in axes.iter().enumerate() {
            scales[a] = sv[i];
            out[a] = (x.shape[a] as f32 * sv[i]).floor() as usize;
        }
    } else if let Some(s) = node.attr_floats("scales") {
        for a in 0..r.min(s.len()) {
            scales[a] = s[a];
            out[a] = (x.shape[a] as f32 * s[a]).floor() as usize;
        }
    } else {
        return Err(Error("Resize: neither scales nor sizes given".into()));
    }
    let mut roi = vec![(0.0f32, 1.0f32); r];
    if let Some(rt) = roi_t {
        let rv = rt.to_f32_vec();
        let n = axes.len();
        if rv.len() == 2 * n {
            for (i, &a) in axes.iter().enumerate() {
                roi[a] = (rv[i], rv[i + n]);
            }
        }
    }
    let v = resize_f32(&x.f32s(), &x.shape, &out, &scales, &roi, &p, ctx.threads);
    let t = Tensor::f32(&out, v);
    Ok(vec![if x.dtype() == crate::DType::F32 || x.dtype() == crate::DType::F16 { t } else { t.cast(x.dtype()) }])
}

// ------------------------------------------------------------ GridSample

fn reflect(x: f32, twice_low: f32, twice_high: f32) -> f32 {
    if twice_low == twice_high {
        return 0.0;
    }
    let min = twice_low / 2.0;
    let span = (twice_high - twice_low) / 2.0;
    let x = (x - min).abs();
    let extra = x % span;
    let flips = (x / span).floor() as i64;
    if flips % 2 == 0 { extra + min } else { span - extra + min }
}

#[derive(Clone, Copy, PartialEq)]
enum Pad {
    Zeros,
    Border,
    Reflection,
}

fn pad_coord(x: f32, size: usize, pad: Pad, align: bool) -> f32 {
    match pad {
        Pad::Zeros => x,
        Pad::Border => x.clamp(0.0, size as f32 - 1.0),
        Pad::Reflection => {
            let r = if align { reflect(x, 0.0, 2.0 * (size as f32 - 1.0)) } else { reflect(x, -1.0, 2.0 * size as f32 - 1.0) };
            r.clamp(0.0, size as f32 - 1.0)
        }
    }
}

pub fn grid_sample(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let grid = inp(inputs, 1)?;
    if x.rank() != 4 || grid.rank() != 4 || grid.shape[3] != 2 {
        return Err(Error(format!("GridSample: only 4-D input supported (x {:?}, grid {:?})", x.shape, grid.shape)));
    }
    let mode = match node.attr_s_or("mode", "bilinear").as_str() {
        "bilinear" | "linear" => Mode::Linear,
        "nearest" => Mode::Nearest,
        "bicubic" | "cubic" => Mode::Cubic,
        m => return Err(Error(format!("GridSample: unknown mode {m}"))),
    };
    let pad = match node.attr_s_or("padding_mode", "zeros").as_str() {
        "border" => Pad::Border,
        "reflection" => Pad::Reflection,
        _ => Pad::Zeros,
    };
    let align = node.attr_i("align_corners", 0) != 0;
    let (n, c, h, w) = (x.shape[0], x.shape[1], x.shape[2], x.shape[3]);
    let (oh, ow) = (grid.shape[1], grid.shape[2]);
    let xv = x.f32s();
    let gv = grid.f32s();
    let mut out = vec![0.0f32; n * c * oh * ow];
    let op = SendPtr(out.as_mut_ptr());
    let unnorm = |g: f32, size: usize| if align { (g + 1.0) / 2.0 * (size as f32 - 1.0) } else { ((g + 1.0) * size as f32 - 1.0) / 2.0 };
    par_dynamic(n * oh, if out.len() > 4096 { ctx.threads } else { 1 }, |row| {
        let b = row / oh;
        let y = row % oh;
        for xo in 0..ow {
            let gi = ((b * oh + y) * ow + xo) * 2;
            let ix = unnorm(gv[gi], w);
            let iy = unnorm(gv[gi + 1], h);
            for ch in 0..c {
                let plane = &xv[(b * c + ch) * h * w..(b * c + ch + 1) * h * w];
                let at = |yy: i64, xx: i64| -> f32 { if yy < 0 || xx < 0 || yy >= h as i64 || xx >= w as i64 { 0.0 } else { plane[yy as usize * w + xx as usize] } };
                let val = match mode {
                    Mode::Linear => {
                        let (px, py) = (pad_coord(ix, w, pad, align), pad_coord(iy, h, pad, align));
                        let (x0, y0) = (px.floor(), py.floor());
                        let (fx, fy) = (px - x0, py - y0);
                        let (x0, y0) = (x0 as i64, y0 as i64);
                        at(y0, x0) * (1.0 - fx) * (1.0 - fy) + at(y0, x0 + 1) * fx * (1.0 - fy) + at(y0 + 1, x0) * (1.0 - fx) * fy + at(y0 + 1, x0 + 1) * fx * fy
                    }
                    Mode::Nearest => {
                        let (px, py) = (pad_coord(ix, w, pad, align), pad_coord(iy, h, pad, align));
                        at(py.round_ties_even() as i64, px.round_ties_even() as i64)
                    }
                    Mode::Cubic => {
                        let (x0, y0) = (ix.floor(), iy.floor());
                        let (fx, fy) = (ix - x0, iy - y0);
                        let wx: Vec<f32> = (-1..3).map(|k| cubic_coeff(k as f32 - fx, -0.75)).collect();
                        let wy: Vec<f32> = (-1..3).map(|k| cubic_coeff(k as f32 - fy, -0.75)).collect();
                        let fetch = |yy: f32, xx: f32| -> f32 {
                            if pad == Pad::Zeros {
                                at(yy as i64, xx as i64)
                            } else {
                                let (a, bb) = (pad_coord(xx, w, pad, align), pad_coord(yy, h, pad, align));
                                at(bb as i64, a as i64)
                            }
                        };
                        let mut s = 0.0;
                        for (j, wyv) in wy.iter().enumerate() {
                            for (i, wxv) in wx.iter().enumerate() {
                                s += wyv * wxv * fetch(y0 + j as f32 - 1.0, x0 + i as f32 - 1.0);
                            }
                        }
                        s
                    }
                };
                // SAFETY: output rows (b, y) are disjoint between items.
                unsafe { *op.add(((b * c + ch) * oh + y) * ow + xo) = val };
            }
        }
    });
    Ok(vec![Tensor::f32(&[n, c, oh, ow], out)])
}
