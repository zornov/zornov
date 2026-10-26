// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Image pre/post-processing for vision models: RGBA8 ↔ normalised NCHW /
//! NHWC tensors, bilinear resizing of planes, letterboxing, padding to a
//! multiple, tiling with overlap, mask helpers.

use crate::tensor::Tensor;

/// A borrowed RGBA8 image (row-major, 4 bytes per pixel, no row padding).
#[derive(Clone, Copy, Debug)]
pub struct Rgba<'a> {
    pub w: usize,
    pub h: usize,
    pub px: &'a [u8],
}

/// Per-channel normalisation `(x * scale - mean) / std`, applied to RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Norm {
    pub scale: f32,
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

impl Norm {
    /// x / 255 (range [0, 1]).
    pub const UNIT: Norm = Norm { scale: 1.0 / 255.0, mean: [0.0; 3], std: [1.0; 3] };
    /// (x / 255 − 0.5) / 0.5 (range [−1, 1]).
    pub const SIGNED: Norm = Norm { scale: 1.0 / 255.0, mean: [0.5; 3], std: [0.5; 3] };
    /// ImageNet mean/std on x / 255.
    pub const IMAGENET: Norm = Norm { scale: 1.0 / 255.0, mean: [0.485, 0.456, 0.406], std: [0.229, 0.224, 0.225] };
    /// Raw 0–255 values.
    pub const RAW: Norm = Norm { scale: 1.0, mean: [0.0; 3], std: [1.0; 3] };

    #[inline]
    pub fn apply(&self, c: usize, v: f32) -> f32 {
        (v * self.scale - self.mean[c]) / self.std[c]
    }
}

/// Bilinear sample position (half-pixel centres, like `Resize`
/// `coordinate_transformation_mode = half_pixel` / OpenCV INTER_LINEAR).
#[inline]
fn src_coord(dst: usize, dst_len: usize, src_len: usize) -> (usize, usize, f32) {
    let s = ((dst as f32 + 0.5) * src_len as f32 / dst_len as f32 - 0.5).max(0.0);
    let i0 = (s.floor() as usize).min(src_len - 1);
    let i1 = (i0 + 1).min(src_len - 1);
    (i0, i1, s - i0 as f32)
}

/// Resizes an RGBA image to `ow × oh` and returns normalised RGB planes as
/// a `[1, 3, oh, ow]` f32 tensor.
pub fn to_nchw(img: Rgba, ow: usize, oh: usize, norm: Norm) -> Tensor {
    let mut out = vec![0.0f32; 3 * ow * oh];
    sample_rgb(img, ow, oh, |x, y, c, v| out[c * ow * oh + y * ow + x] = norm.apply(c, v));
    Tensor::f32(&[1, 3, oh, ow], out)
}

/// As [`to_nchw`] but `[1, oh, ow, 3]` (TFLite/MediaPipe layout).
pub fn to_nhwc(img: Rgba, ow: usize, oh: usize, norm: Norm) -> Tensor {
    let mut out = vec![0.0f32; 3 * ow * oh];
    sample_rgb(img, ow, oh, |x, y, c, v| out[(y * ow + x) * 3 + c] = norm.apply(c, v));
    Tensor::f32(&[1, oh, ow, 3], out)
}

fn sample_rgb(img: Rgba, ow: usize, oh: usize, mut put: impl FnMut(usize, usize, usize, f32)) {
    assert!(img.px.len() >= img.w * img.h * 4 && img.w > 0 && img.h > 0, "image buffer too small");
    for y in 0..oh {
        let (y0, y1, fy) = src_coord(y, oh, img.h);
        for x in 0..ow {
            let (x0, x1, fx) = src_coord(x, ow, img.w);
            for c in 0..3 {
                let p = |xx: usize, yy: usize| img.px[(yy * img.w + xx) * 4 + c] as f32;
                let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
                let bot = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
                put(x, y, c, top * (1.0 - fy) + bot * fy);
            }
        }
    }
}

/// Bilinear resize of a single f32 plane.
pub fn resize_plane(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; dw * dh];
    if sw == 0 || sh == 0 {
        return out;
    }
    for y in 0..dh {
        let (y0, y1, fy) = src_coord(y, dh, sh);
        for x in 0..dw {
            let (x0, x1, fx) = src_coord(x, dw, sw);
            let p = |xx: usize, yy: usize| src[yy * sw + xx];
            let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
            let bot = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
            out[y * dw + x] = top * (1.0 - fy) + bot * fy;
        }
    }
    out
}

/// Bilinear resize of a sub-rectangle `(x, y, w, h)` (in source pixels,
/// fractional allowed) of a plane into `dw × dh`.
pub fn crop_resize_plane(src: &[f32], sw: usize, sh: usize, rect: (f32, f32, f32, f32), dw: usize, dh: usize) -> Vec<f32> {
    let (rx, ry, rw, rh) = rect;
    let mut out = vec![0.0f32; dw * dh];
    for y in 0..dh {
        let sy = (ry + (y as f32 + 0.5) * rh / dh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let fy = sy - y0 as f32;
        for x in 0..dw {
            let sx = (rx + (x as f32 + 0.5) * rw / dw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let fx = sx - x0 as f32;
            let p = |xx: usize, yy: usize| src[yy * sw + xx];
            out[y * dw + x] = (p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx) * (1.0 - fy) + (p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx) * fy;
        }
    }
    out
}

/// The size with the short side scaled to `short` and both sides rounded
/// down to a multiple of `multiple` (at least `multiple`).
pub fn fit_short_side(w: usize, h: usize, short: usize, multiple: usize) -> (usize, usize) {
    let s = short as f32 / w.min(h).max(1) as f32;
    let r = |v: f32| (((v / multiple as f32).round() as usize).max(1)) * multiple;
    (r(w as f32 * s), r(h as f32 * s))
}

/// The size with the long side scaled to `long`, rounded to `multiple`.
pub fn fit_long_side(w: usize, h: usize, long: usize, multiple: usize) -> (usize, usize) {
    let s = long as f32 / w.max(h).max(1) as f32;
    let r = |v: f32| (((v / multiple as f32).round() as usize).max(1)) * multiple;
    (r(w as f32 * s), r(h as f32 * s))
}

/// Letterbox geometry: the image scaled to fit `dw × dh` keeping aspect,
/// centred. Returns `(scale, pad_x, pad_y)` with `dst = src * scale + pad`.
pub fn letterbox(sw: usize, sh: usize, dw: usize, dh: usize) -> (f32, f32, f32) {
    let s = (dw as f32 / sw as f32).min(dh as f32 / sh as f32);
    (s, (dw as f32 - sw as f32 * s) / 2.0, (dh as f32 - sh as f32 * s) / 2.0)
}

/// Letterboxes an RGBA image into a `[1, 3, dh, dw]` tensor (padding value
/// `pad` after normalisation is `norm.apply(c, 0)`).
pub fn to_nchw_letterbox(img: Rgba, dw: usize, dh: usize, norm: Norm) -> (Tensor, (f32, f32, f32)) {
    let (s, px, py) = letterbox(img.w, img.h, dw, dh);
    let mut out = vec![0.0f32; 3 * dw * dh];
    for c in 0..3 {
        let v = norm.apply(c, 0.0);
        out[c * dw * dh..(c + 1) * dw * dh].fill(v);
    }
    for c in 0..3 {
        let plane: Vec<f32> = (0..img.w * img.h).map(|i| img.px[i * 4 + c] as f32).collect();
        let iw = ((img.w as f32 * s).round() as usize).clamp(1, dw);
        let ih = ((img.h as f32 * s).round() as usize).clamp(1, dh);
        let r = resize_plane(&plane, img.w, img.h, iw, ih);
        let (ox, oy) = (px.floor() as usize, py.floor() as usize);
        for y in 0..ih.min(dh - oy) {
            for x in 0..iw.min(dw - ox) {
                out[c * dw * dh + (y + oy) * dw + x + ox] = norm.apply(c, r[y * iw + x]);
            }
        }
    }
    (Tensor::f32(&[1, 3, dh, dw], out), (s, px, py))
}

/// Pads `[1, C, H, W]` on the right/bottom (edge replicate) so H and W are
/// multiples of `m`. Returns the padded tensor and the original size.
pub fn pad_to_multiple(t: &Tensor, m: usize) -> (Tensor, (usize, usize)) {
    let (c, h, w) = (t.shape[1], t.shape[2], t.shape[3]);
    let ph = h.div_ceil(m) * m;
    let pw = w.div_ceil(m) * m;
    if ph == h && pw == w {
        return (t.clone(), (h, w));
    }
    let v = t.f32s();
    let mut out = vec![0.0f32; c * ph * pw];
    for ci in 0..c {
        for y in 0..ph {
            for x in 0..pw {
                out[(ci * ph + y) * pw + x] = v[(ci * h + y.min(h - 1)) * w + x.min(w - 1)];
            }
        }
    }
    (Tensor::f32(&[1, c, ph, pw], out), (h, w))
}

/// Crops `[1, C, H, W]` to the top-left `h × w`.
pub fn crop_nchw(t: &Tensor, h: usize, w: usize) -> Tensor {
    let (c, th, tw) = (t.shape[1], t.shape[2], t.shape[3]);
    let v = t.f32s();
    let mut out = Vec::with_capacity(c * h * w);
    for ci in 0..c {
        for y in 0..h.min(th) {
            out.extend_from_slice(&v[(ci * th + y) * tw..(ci * th + y) * tw + w.min(tw)]);
        }
    }
    Tensor::f32(&[1, c, h.min(th), w.min(tw)], out)
}

/// NCHW planes (RGB, values in [0,1] after `denorm`) → RGBA8 with alpha 255.
pub fn nchw_to_rgba(t: &Tensor, denorm: impl Fn(usize, f32) -> f32) -> (Vec<u8>, usize, usize) {
    let (h, w) = (t.shape[t.rank() - 2], t.shape[t.rank() - 1]);
    let v = t.f32s();
    let mut out = vec![255u8; w * h * 4];
    for c in 0..3 {
        for i in 0..w * h {
            out[i * 4 + c] = (denorm(c, v[c * w * h + i]) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    (out, w, h)
}

/// A tile of a tiled pass: input rect (with overlap) and the core rect it
/// is responsible for, both in source pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub core_x: usize,
    pub core_y: usize,
    pub core_w: usize,
    pub core_h: usize,
}

/// Splits `w × h` into tiles of at most `tile` core pixels with `overlap`
/// context pixels on every side (clamped at the borders).
pub fn tiles(w: usize, h: usize, tile: usize, overlap: usize) -> Vec<Tile> {
    let mut v = Vec::new();
    let tile = tile.max(1);
    let mut y = 0;
    while y < h {
        let ch = tile.min(h - y);
        let mut x = 0;
        while x < w {
            let cw = tile.min(w - x);
            let x0 = x.saturating_sub(overlap);
            let y0 = y.saturating_sub(overlap);
            let x1 = (x + cw + overlap).min(w);
            let y1 = (y + ch + overlap).min(h);
            v.push(Tile { x: x0, y: y0, w: x1 - x0, h: y1 - y0, core_x: x, core_y: y, core_w: cw, core_h: ch });
            x += cw;
        }
        y += ch;
    }
    v
}

/// Extracts an RGBA sub-image.
pub fn crop_rgba(img: Rgba, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for yy in y..y + h {
        out.extend_from_slice(&img.px[(yy * img.w + x) * 4..(yy * img.w + x + w) * 4]);
    }
    out
}

pub fn sigmoid_inplace(v: &mut [f32]) {
    for x in v {
        *x = 1.0 / (1.0 + (-*x).exp());
    }
}

/// Min-max normalises to [0, 1] (constant input → zeros).
pub fn normalize_minmax(v: &mut [f32]) {
    let (mn, mx) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    let r = mx - mn;
    for x in v {
        *x = if r > 1e-12 { (*x - mn) / r } else { 0.0 };
    }
}

/// Mask in [0,1] → u8 alpha.
pub fn mask_to_u8(v: &[f32]) -> Vec<u8> {
    v.iter().map(|&x| (x * 255.0 + 0.5).clamp(0.0, 255.0) as u8).collect()
}

/// Exponential moving average for temporally smoothing masks/mattes.
pub fn ema(prev: &mut [f32], cur: &[f32], alpha: f32) {
    for (p, &c) in prev.iter_mut().zip(cur) {
        *p = *p * (1.0 - alpha) + c * alpha;
    }
}

/// Separable box blur (radius r) of a plane, used to feather masks.
pub fn box_blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return v.to_vec();
    }
    let mut tmp = vec![0.0f32; w * h];
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &v[y * w..(y + 1) * w];
        let mut acc: f32 = (0..=r).map(|i| row[i.min(w - 1)]).sum::<f32>() + r as f32 * row[0];
        for x in 0..w {
            tmp[y * w + x] = acc / (2 * r + 1) as f32;
            let add = row[(x + r + 1).min(w - 1)];
            let sub = row[x.saturating_sub(r)];
            acc += add - sub;
        }
    }
    for x in 0..w {
        let col = |y: usize| tmp[y * w + x];
        let mut acc: f32 = (0..=r).map(|i| col(i.min(h - 1))).sum::<f32>() + r as f32 * col(0);
        for y in 0..h {
            out[y * w + x] = acc / (2 * r + 1) as f32;
            acc += col((y + r + 1).min(h - 1)) - col(y.saturating_sub(r));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nchw_identity_resize_and_norm() {
        let px: Vec<u8> = (0..2 * 2).flat_map(|i| [i as u8 * 10, 100, 255, 255]).collect();
        let t = to_nchw(Rgba { w: 2, h: 2, px: &px }, 2, 2, Norm::UNIT);
        assert_eq!(t.shape, vec![1, 3, 2, 2]);
        let v = t.f32s();
        assert!((v[1] - 10.0 / 255.0).abs() < 1e-6);
        assert!((v[4] - 100.0 / 255.0).abs() < 1e-6);
        assert!((v[8] - 1.0).abs() < 1e-6);
        let n = to_nhwc(Rgba { w: 2, h: 2, px: &px }, 2, 2, Norm::SIGNED);
        assert_eq!(n.shape, vec![1, 2, 2, 3]);
        assert!((n.f32s()[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn plane_resize_constant_and_ramp() {
        let p = vec![3.0f32; 16];
        assert!(resize_plane(&p, 4, 4, 7, 3).iter().all(|&v| (v - 3.0).abs() < 1e-6));
        let ramp: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let r = resize_plane(&ramp, 8, 1, 4, 1);
        assert_eq!(r, vec![0.5, 2.5, 4.5, 6.5]);
    }

    #[test]
    fn tiles_cover_every_pixel_once() {
        let t = tiles(100, 70, 32, 8);
        let mut hits = vec![0; 100 * 70];
        for tl in &t {
            assert!(tl.x <= tl.core_x && tl.core_x + tl.core_w <= tl.x + tl.w);
            for y in tl.core_y..tl.core_y + tl.core_h {
                for x in tl.core_x..tl.core_x + tl.core_w {
                    hits[y * 100 + x] += 1;
                }
            }
        }
        assert!(hits.iter().all(|&h| h == 1));
    }

    #[test]
    fn pad_and_crop_roundtrip() {
        let t = Tensor::f32(&[1, 1, 3, 5], (0..15).map(|i| i as f32).collect());
        let (p, (h, w)) = pad_to_multiple(&t, 4);
        assert_eq!(p.shape, vec![1, 1, 4, 8]);
        assert_eq!(crop_nchw(&p, h, w), t);
    }

    #[test]
    fn fit_and_letterbox() {
        assert_eq!(fit_short_side(1920, 1080, 512, 32), (896, 512));
        let (s, px, py) = letterbox(200, 100, 100, 100);
        assert_eq!((s, px, py), (0.5, 0.0, 25.0));
    }

    #[test]
    fn blur_preserves_constant() {
        let v = vec![0.7f32; 30];
        assert!(box_blur(&v, 6, 5, 2).iter().all(|&x| (x - 0.7).abs() < 1e-5));
    }
}
