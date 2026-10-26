// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Paint colours at a pixel: gradients, freeform blends and image
//! sampling (bilinear, trilinear over mipmaps when shrinking, bicubic when
//! enlarging). `shaders/fine.wgsl` mirrors these.

use crate::paint::{Extend, Image, ImageQuality};
use crate::prepare::{IRect, ImgSrc, PPaint};

/// A prepass image: premultiplied pixels covering `region`.
pub(crate) struct PrepImage<'a> {
    pub region: IRect,
    pub pixels: &'a [[f32; 4]],
}

#[inline]
pub(crate) fn apply(inv: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    (inv[0] * x + inv[2] * y + inv[4], inv[1] * x + inv[3] * y + inv[5])
}

#[inline]
pub(crate) fn extend_t(t: f32, e: Extend) -> f32 {
    match e {
        Extend::Pad | Extend::None => t.clamp(0.0, 1.0),
        Extend::Repeat => t - t.floor(),
        Extend::Reflect => {
            let u = t.rem_euclid(2.0);
            if u > 1.0 { 2.0 - u } else { u }
        }
    }
}

/// The two-point conical gradient offset at `p` (gradient space), or
/// `None` outside the cone.
#[inline]
pub(crate) fn radial_t(p: (f32, f32), c0: [f32; 2], r0: f32, c1: [f32; 2], r1: f32) -> Option<f32> {
    let (cdx, cdy) = (c1[0] - c0[0], c1[1] - c0[1]);
    let dr = r1 - r0;
    let (pdx, pdy) = (p.0 - c0[0], p.1 - c0[1]);
    let a = cdx * cdx + cdy * cdy - dr * dr;
    let b = pdx * cdx + pdy * cdy + r0 * dr;
    let c = pdx * pdx + pdy * pdy - r0 * r0;
    if a.abs() < 1e-6 * (cdx * cdx + cdy * cdy + dr * dr).max(1e-12) {
        if b.abs() < 1e-12 {
            return None;
        }
        let t = c / (2.0 * b);
        return (r0 + t * dr >= 0.0).then_some(t);
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let (t1, t2) = ((b + sq) / a, (b - sq) / a);
    let (hi, lo) = if t1 > t2 { (t1, t2) } else { (t2, t1) };
    if r0 + hi * dr >= 0.0 {
        Some(hi)
    } else if r0 + lo * dr >= 0.0 {
        Some(lo)
    } else {
        None
    }
}

#[inline]
fn scale4(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3] * k]
}

/// The colour of `paint` at pixel centre (`x`, `y`), premultiplied.
pub(crate) fn eval(paint: &PPaint, x: f32, y: f32, pre: &[PrepImage]) -> [f32; 4] {
    match paint {
        PPaint::Solid(c) => *c,
        PPaint::Linear { inv, lut, extend } => {
            let (u, _) = apply(inv, x, y);
            lut.sample(extend_t(u, *extend))
        }
        PPaint::Radial { inv, lut, extend, c0, r0, c1, r1 } => match radial_t(apply(inv, x, y), *c0, *r0, *c1, *r1) {
            Some(t) => lut.sample(extend_t(t, *extend)),
            None => [0.0; 4],
        },
        PPaint::Conic { inv, lut, extend } => {
            let (u, v) = apply(inv, x, y);
            let t = v.atan2(u) / std::f32::consts::TAU;
            let t = t - t.floor();
            lut.sample(extend_t(t, *extend))
        }
        PPaint::Freeform { pts } => freeform(pts, x, y),
        PPaint::Image { src, inv, extend, quality, opacity, lod } => {
            let (u, v) = apply(inv, x, y);
            let c = match src {
                ImgSrc::Prepass(i) => sample_prepass(&pre[*i], u, v, *extend, *quality),
                ImgSrc::User(img) => sample_image(img, u, v, *extend, *quality, *lod),
            };
            scale4(c, *opacity)
        }
    }
}

pub(crate) fn freeform(pts: &[([f32; 2], [f32; 4], f32)], x: f32, y: f32) -> [f32; 4] {
    let mut acc = [0.0f32; 4];
    let mut wsum = 0.0f32;
    for (p, c, s) in pts {
        let (dx, dy) = (x - p[0], y - p[1]);
        let d2 = dx * dx + dy * dy;
        if d2 < 1e-6 {
            return *c;
        }
        let w = s / (d2 * d2.sqrt().sqrt());
        wsum += w;
        for i in 0..4 {
            acc[i] += c[i] * w;
        }
    }
    scale4(acc, 1.0 / wsum.max(1e-30))
}

/// A texel coordinate after extension, or `None` when outside a
/// non-extending image.
#[inline]
fn wrap(i: i32, n: i32, e: Extend) -> Option<i32> {
    match e {
        Extend::Pad => Some(i.clamp(0, n - 1)),
        Extend::None => (0..n).contains(&i).then_some(i),
        Extend::Repeat => Some(i.rem_euclid(n)),
        Extend::Reflect => {
            let m = i.rem_euclid(2 * n);
            Some(if m >= n { 2 * n - 1 - m } else { m })
        }
    }
}

fn sample_prepass(img: &PrepImage, u: f32, v: f32, e: Extend, q: ImageQuality) -> [f32; 4] {
    let (w, h) = (img.region.width(), img.region.height());
    if w <= 0 || h <= 0 {
        return [0.0; 4];
    }
    let fetch = |x: i32, y: i32| -> [f32; 4] {
        match (wrap(x, w, e), wrap(y, h, e)) {
            (Some(x), Some(y)) => img.pixels[(y * w + x) as usize],
            _ => [0.0; 4],
        }
    };
    match q {
        ImageQuality::Nearest => fetch(u.floor() as i32, v.floor() as i32),
        _ => bilinear(fetch, u, v),
    }
}

#[inline]
fn bilinear(fetch: impl Fn(i32, i32) -> [f32; 4], u: f32, v: f32) -> [f32; 4] {
    let (x, y) = (u - 0.5, v - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (fetch(x0, y0), fetch(x0 + 1, y0), fetch(x0, y0 + 1), fetch(x0 + 1, y0 + 1));
    let mut o = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        o[i] = top + (bot - top) * fy;
    }
    o
}

fn catmull_rom(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

fn texel(data: &[u8], w: i32, x: i32, y: i32) -> [f32; 4] {
    let i = ((y * w + x) * 4) as usize;
    [data[i] as f32 / 255.0, data[i + 1] as f32 / 255.0, data[i + 2] as f32 / 255.0, data[i + 3] as f32 / 255.0]
}

fn sample_level(data: &[u8], w: i32, h: i32, u: f32, v: f32, e: Extend, q: ImageQuality, cubic: bool) -> [f32; 4] {
    let fetch = |x: i32, y: i32| match (wrap(x, w, e), wrap(y, h, e)) {
        (Some(x), Some(y)) => texel(data, w, x, y),
        _ => [0.0; 4],
    };
    match q {
        ImageQuality::Nearest => fetch(u.floor() as i32, v.floor() as i32),
        _ if cubic => {
            let (x, y) = (u - 0.5, v - 0.5);
            let (x0, y0) = (x.floor(), y.floor());
            let (wx, wy) = (catmull_rom(x - x0), catmull_rom(y - y0));
            let (x0, y0) = (x0 as i32 - 1, y0 as i32 - 1);
            let mut o = [0.0f32; 4];
            for (j, wyj) in wy.iter().enumerate() {
                for (i, wxi) in wx.iter().enumerate() {
                    let t = fetch(x0 + i as i32, y0 + j as i32);
                    let k = wxi * wyj;
                    for c in 0..4 {
                        o[c] += t[c] * k;
                    }
                }
            }
            // Ringing must not leave premultiplied range.
            let a = o[3].clamp(0.0, 1.0);
            [o[0].clamp(0.0, a), o[1].clamp(0.0, a), o[2].clamp(0.0, a), a]
        }
        _ => bilinear(fetch, u, v),
    }
}

/// `lod` is image pixels per device pixel: above 1 the image is shrunk and
/// mipmaps are blended (trilinear); below 1, bicubic quality enlarges with
/// Catmull-Rom.
pub(crate) fn sample_image(img: &Image, u: f32, v: f32, e: Extend, q: ImageQuality, lod: f32) -> [f32; 4] {
    let (w, h) = (img.width as i32, img.height as i32);
    if q == ImageQuality::Nearest || lod <= 1.0 {
        return sample_level(&img.data, w, h, u, v, e, q, q == ImageQuality::Bicubic && lod < 1.0);
    }
    let level = lod.log2();
    let l0 = level.floor() as usize;
    let f = level - l0 as f32;
    let mips = img.mips();
    let at = |l: usize| -> [f32; 4] {
        if l == 0 {
            return sample_level(&img.data, w, h, u, v, e, q, false);
        }
        let Some(m) = mips.get(l - 1).or(mips.last()) else {
            return sample_level(&img.data, w, h, u, v, e, q, false);
        };
        let (sx, sy) = (m.width as f32 / w as f32, m.height as f32 / h as f32);
        sample_level(&m.data, m.width as i32, m.height as i32, u * sx, v * sy, e, q, false)
    };
    let a = at(l0);
    if f < 1e-3 {
        return a;
    }
    let b = at(l0 + 1);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radial_plain_and_focal() {
        // Plain radial: t is the distance over the radius.
        let t = radial_t((3.0, 4.0), [0.0, 0.0], 0.0, [0.0, 0.0], 10.0).unwrap();
        assert!((t - 0.5).abs() < 1e-6);
        // Focal at the left: the focus itself is offset 0, the outer circle 1.
        let t = radial_t((-5.0, 0.0), [-5.0, 0.0], 0.0, [0.0, 0.0], 10.0).unwrap();
        assert!(t.abs() < 1e-6);
        let t = radial_t((10.0, 0.0), [-5.0, 0.0], 0.0, [0.0, 0.0], 10.0).unwrap();
        assert!((t - 1.0).abs() < 1e-5, "{t}");
    }

    #[test]
    fn extend_modes() {
        assert_eq!(extend_t(1.25, Extend::Repeat), 0.25);
        assert_eq!(extend_t(1.25, Extend::Reflect), 0.75);
        assert_eq!(extend_t(-0.5, Extend::Pad), 0.0);
        assert_eq!(wrap(-1, 4, Extend::Reflect), Some(0));
        assert_eq!(wrap(5, 4, Extend::Reflect), Some(2));
        assert_eq!(wrap(4, 4, Extend::None), None);
    }
}
