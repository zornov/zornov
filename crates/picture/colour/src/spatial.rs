// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tools that look at neighbouring pixels or pixel position: clarity,
//! dehaze, sharpen, vignette and grain.
//!
//! All of them work on straight (un-premultiplied) linear RGB in place.
//! Radii that describe the *picture* (clarity, dehaze) scale with the
//! image size so a preview and an export look the same; sharpen and grain
//! are specified in pixels of the image being processed.

use crate::basic::{Grain, Sharpen, Vignette};
use crate::math::{Rgb, dot3, smoothstep};
use crate::space::cct_to_n;
use crate::transfer::lin_to_acescct;

// ------------------------------------------------------------- blurring

/// Box blur of one plane with radius `r` (window 2r+1), edges clamped.
fn box_blur_1d(src: &[f32], dst: &mut [f32], n: usize, stride: usize, r: usize) {
    if n == 0 {
        return;
    }
    let inv = 1.0 / (2 * r + 1) as f32;
    let get = |i: isize| src[(i.clamp(0, n as isize - 1) as usize) * stride];
    let mut sum = 0.0f64;
    for i in -(r as isize)..=(r as isize) {
        sum += get(i) as f64;
    }
    for i in 0..n {
        dst[i * stride] = sum as f32 * inv;
        sum += get(i as isize + r as isize + 1) as f64 - get(i as isize - r as isize) as f64;
    }
}

/// Approximate Gaussian blur of a `w×h` plane with standard deviation
/// `sigma`, as three successive box blurs (the classic sizes from
/// Wells 1986 / Kovesi 2010). Cost is independent of the radius.
pub fn gaussian_blur(plane: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 || w == 0 || h == 0 {
        return;
    }
    let passes = 3.0f32;
    let ideal = (12.0 * sigma * sigma / passes + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - passes * (wl * wl) as f32 - 4.0 * passes * wl as f32 - 3.0 * passes)
        / (-4.0 * wl as f32 - 4.0))
        .round() as i32;
    let mut tmp = vec![0.0f32; plane.len()];
    for pass in 0..3 {
        let size = if pass < m { wl } else { wu };
        let r = ((size - 1) / 2).max(0) as usize;
        if r == 0 {
            continue;
        }
        for y in 0..h {
            box_blur_1d(&plane[y * w..], &mut tmp[y * w..], w, 1, r);
        }
        for x in 0..w {
            box_blur_1d(&tmp[x..], &mut plane[x..], h, w, r);
        }
    }
}

/// Sliding-window minimum along one line (monotonic deque, O(n)).
fn min_filter_1d(src: &[f32], dst: &mut [f32], n: usize, stride: usize, r: usize) {
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let get = |i: usize| src[i * stride];
    let mut next = 0;
    for i in 0..n {
        let hi = (i + r).min(n - 1);
        while next <= hi {
            while let Some(&b) = dq.back() {
                if get(b) >= get(next) {
                    dq.pop_back();
                } else {
                    break;
                }
            }
            dq.push_back(next);
            next += 1;
        }
        let lo = i.saturating_sub(r);
        while let Some(&f) = dq.front() {
            if f < lo {
                dq.pop_front();
            } else {
                break;
            }
        }
        dst[i * stride] = get(*dq.front().unwrap());
    }
}

/// Square minimum filter of radius `r`.
pub fn min_filter(plane: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let mut tmp = vec![0.0f32; plane.len()];
    for y in 0..h {
        min_filter_1d(&plane[y * w..], &mut tmp[y * w..], w, 1, r);
    }
    for x in 0..w {
        min_filter_1d(&tmp[x..], &mut plane[x..], h, w, r);
    }
}

// --------------------------------------------------- local contrast tools

/// Adds `gain × (log2 Y − blur(log2 Y))` stops to each pixel, scaled by
/// `mask(Y)`. Working in log luminance makes the detail a ratio, so the
/// effect is the same in shadows and highlights and colour is preserved.
fn log_luma_detail(pixels: &mut [Rgb], w: usize, h: usize, weights: Rgb, sigma: f32, gain: f32, mask: impl Fn(f32) -> f32) {
    const FLOOR: f32 = 1.0 / 4096.0;
    let mut ly: Vec<f32> = pixels.iter().map(|c| dot3(*c, weights).max(FLOOR).log2()).collect();
    let orig = ly.clone();
    gaussian_blur(&mut ly, w, h, sigma);
    for ((c, &l), &b) in pixels.iter_mut().zip(&orig).zip(&ly) {
        // Clamp the detail so strong edges do not produce bright halos.
        let detail = (l - b).clamp(-2.0, 2.0);
        let k = (gain * detail * mask(l.exp2())).exp2();
        *c = c.map(|v| v * k);
    }
}

/// Clarity: large-radius unsharp mask on luminance, strongest in the
/// midtones. Sigma is 1.2 % of the longer image side; `amount` −1..+1
/// (negative softens local contrast).
pub fn clarity(pixels: &mut [Rgb], w: usize, h: usize, weights: Rgb, amount: f32) {
    if amount == 0.0 {
        return;
    }
    let sigma = 0.012 * w.max(h) as f32;
    log_luma_detail(pixels, w, h, weights, sigma, 0.8 * amount, |y| {
        let n = cct_to_n(lin_to_acescct(y));
        smoothstep(0.0, 0.3, n) * (1.0 - smoothstep(0.8, 1.2, n))
    });
}

/// Unsharp-mask sharpening on luminance.
pub fn sharpen(pixels: &mut [Rgb], w: usize, h: usize, weights: Rgb, s: &Sharpen) {
    if s.amount == 0.0 {
        return;
    }
    log_luma_detail(pixels, w, h, weights, s.radius.clamp(0.3, 10.0), 1.5 * s.amount, |_| 1.0);
}

/// Dehaze with a dark-channel estimate (after He, Sun & Tang 2009).
///
/// 1. Dark channel: per pixel minimum over RGB, then a minimum filter with
///    a radius of 1 % of the shorter side.
/// 2. Airlight `A`: mean colour of the 0.1 % pixels with the brightest dark
///    channel.
/// 3. Transmission `t = 1 − ω · dark(I / A)`, smoothed with a Gaussian of
///    four times the patch radius (a cheap stand-in for guided filtering),
///    with ω = 0.95 × amount.
/// 4. Recover `J = (I − A) / max(t, 0.1) + A`.
///
/// A negative amount adds haze instead: `J = I·t + A·(1 − t)` with
/// `t = 1 − 0.6·|amount|·(1 − dark)`, which fogs bright distant areas less
/// than a flat mix would.
pub fn dehaze(pixels: &mut [Rgb], w: usize, h: usize, amount: f32) {
    if amount == 0.0 || pixels.is_empty() {
        return;
    }
    let r = ((w.min(h) as f32 * 0.01).round() as usize).max(1);
    let mut dark: Vec<f32> = pixels.iter().map(|c| c[0].min(c[1]).min(c[2]).max(0.0)).collect();
    min_filter(&mut dark, w, h, r);

    let count = (pixels.len() / 1000).max(1);
    let mut order: Vec<usize> = (0..pixels.len()).collect();
    let k = pixels.len() - count;
    order.select_nth_unstable_by(k, |&a, &b| dark[a].total_cmp(&dark[b]));
    let mut a = [0.0f32; 3];
    for &i in &order[k..] {
        for ch in 0..3 {
            a[ch] += pixels[i][ch].max(0.0);
        }
    }
    let a = a.map(|v| (v / count as f32).max(0.05));

    let mut dn: Vec<f32> =
        pixels.iter().map(|c| (c[0] / a[0]).min(c[1] / a[1]).min(c[2] / a[2]).clamp(0.0, 1.0)).collect();
    min_filter(&mut dn, w, h, r);
    gaussian_blur(&mut dn, w, h, 4.0 * r as f32);

    if amount > 0.0 {
        let omega = 0.95 * amount.min(1.0);
        for (c, &d) in pixels.iter_mut().zip(&dn) {
            let t = (1.0 - omega * d).max(0.1);
            for ch in 0..3 {
                c[ch] = (c[ch] - a[ch]) / t + a[ch];
            }
        }
    } else {
        let s = 0.6 * (-amount).min(1.0);
        for (c, &d) in pixels.iter_mut().zip(&dn) {
            let t = 1.0 - s * (1.0 - d);
            for ch in 0..3 {
                c[ch] = c[ch] * t + a[ch] * (1.0 - t);
            }
        }
    }
}

// ----------------------------------------------------- position-based

/// Vignette gain (linear multiplier) at pixel centre `(x, y)`.
///
/// Distances: in *frame* mode `dx = 2·(x/w − ½)`, `dy = 2·(y/h − ½)`; in
/// *circle* mode both axes are divided by the same length so corners still
/// sit at distance √2. `roundness` blends the two. The distance is a
/// superellipse norm with exponent `2 + 6·shape`. The mask is
/// `smoothstep(r0, r0 + f, d)` with `r0 = 0.2 + midpoint` and
/// `f = 0.05 + feather`, and the gain is `2^(2·amount·mask)`.
pub fn vignette_gain(v: &Vignette, x: f32, y: f32, w: f32, h: f32) -> f32 {
    let fx = 2.0 * (x / w - 0.5);
    let fy = 2.0 * (y / h - 0.5);
    let half_diag = 0.5 * w.hypot(h);
    let unit = half_diag / std::f32::consts::SQRT_2;
    let cx = (x - 0.5 * w) / unit;
    let cy = (y - 0.5 * h) / unit;
    let t = v.roundness.clamp(0.0, 1.0);
    let dx = (fx + (cx - fx) * t).abs();
    let dy = (fy + (cy - fy) * t).abs();
    let p = 2.0 + 6.0 * v.shape.clamp(0.0, 1.0);
    let d = (dx.powf(p) + dy.powf(p)).powf(1.0 / p);
    let r0 = 0.2 + v.midpoint.clamp(0.0, 1.0);
    let m = smoothstep(r0, r0 + 0.05 + v.feather.clamp(0.0, 1.0), d);
    (2.0 * v.amount * m).exp2()
}

/// Integer hash → −1..1 (a PCG-style permutation; quality is ample for
/// grain and it is cheap to mirror in WGSL).
#[inline]
pub fn hash_noise(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Smooth value noise at continuous position `(x, y)` in lattice units.
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (ix, iy) = (ix as i32, iy as i32);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash_noise(ix, iy, seed);
    let b = hash_noise(ix + 1, iy, seed);
    let c = hash_noise(ix, iy + 1, seed);
    let d = hash_noise(ix + 1, iy + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

/// Mixes the frame number into the grain seed so grain is animated but
/// repeatable for a given frame.
pub fn grain_seed(g: &Grain, frame: u64) -> u32 {
    let f = frame as u32 ^ ((frame >> 32) as u32).wrapping_mul(0x9e37_79b9);
    g.seed.wrapping_mul(0x85eb_ca6b) ^ f.wrapping_mul(0xc2b2_ae35).wrapping_add(0x27d4_eb2f)
}

/// Grain multiplier at pixel `(x, y)` for a pixel of luminance `y_lin`.
///
/// Two octaves of value noise: a base layer at `size` pixels and a rough
/// layer at `size / 2.5`, mixed by `roughness`. Grain is applied as an
/// exposure change of `amount × 0.5 × noise × weight` stops, where the
/// luminance weight `0.2 + 0.8·(1 − ((n − 0.45) / 0.6)²)` (n: normalised
/// grading value, clamped ≥ 0) peaks in the midtones like film grain and
/// leaves pure black black.
pub fn grain_gain(g: &Grain, seed: u32, x: f32, y: f32, y_lin: f32) -> f32 {
    let size = g.size.clamp(0.25, 16.0);
    let base = value_noise(x / size, y / size, seed);
    let fine = value_noise(x * 2.5 / size, y * 2.5 / size, seed ^ 0x5bd1_e995);
    let r = g.roughness.clamp(0.0, 1.0);
    // Value noise has a smaller spread than its lattice values; 1.7
    // roughly restores unit amplitude.
    let noise = ((1.0 - 0.5 * r) * base + r * 0.7 * fine) * 1.7;
    let n = cct_to_n(lin_to_acescct(y_lin.max(0.0)));
    let q = (n - 0.45) / 0.6;
    let weight = 0.2 + 0.8 * (1.0 - q * q).max(0.0);
    (g.amount * 0.5 * noise * weight).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_preserves_constant_and_mean() {
        let (w, h) = (40, 30);
        let mut p = vec![0.7f32; w * h];
        gaussian_blur(&mut p, w, h, 5.0);
        assert!(p.iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut imp = vec![0.0f32; w * h];
        imp[15 * w + 20] = 1.0;
        gaussian_blur(&mut imp, w, h, 2.0);
        let sum: f32 = imp.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4, "{sum}");
        assert!(imp[15 * w + 20] > imp[15 * w + 23]);
    }

    #[test]
    fn min_filter_works() {
        let (w, h) = (7, 5);
        let mut p = vec![1.0f32; w * h];
        p[2 * w + 3] = 0.0;
        min_filter(&mut p, w, h, 1);
        assert_eq!(p[w + 2], 0.0);
        assert_eq!(p[3 * w + 4], 0.0);
        assert_eq!(p[0], 1.0);
    }

    #[test]
    fn vignette_shape() {
        let v = Vignette { amount: -1.0, ..Vignette::default() };
        let centre = vignette_gain(&v, 960.0, 540.0, 1920.0, 1080.0);
        let corner = vignette_gain(&v, 0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(centre, 1.0);
        assert!(corner < 0.3, "{corner}");
        let off = Vignette::default();
        assert_eq!(vignette_gain(&off, 0.0, 0.0, 1920.0, 1080.0), 1.0);
    }

    #[test]
    fn grain_is_animated_and_neutral_at_zero() {
        let g = Grain { amount: 1.0, ..Grain::default() };
        let a = grain_gain(&g, grain_seed(&g, 0), 10.3, 20.7, 0.18);
        let b = grain_gain(&g, grain_seed(&g, 1), 10.3, 20.7, 0.18);
        assert_ne!(a, b);
        assert_eq!(a, grain_gain(&g, grain_seed(&g, 0), 10.3, 20.7, 0.18));
        let off = Grain::default();
        assert_eq!(grain_gain(&off, 1, 3.0, 4.0, 0.5), 1.0);
        // Roughly zero-mean in stops.
        let mut sum = 0.0;
        for i in 0..10000 {
            sum += grain_gain(&g, 7, (i % 100) as f32 * 1.37, (i / 100) as f32 * 1.37, 0.18).log2();
        }
        assert!((sum / 10000.0).abs() < 0.05, "{}", sum / 10000.0);
    }

    #[test]
    fn dehaze_increases_contrast_of_hazy_image() {
        let (w, h) = (64, 64);
        let haze = [0.8f32, 0.82, 0.85];
        let mut px: Vec<Rgb> = (0..w * h)
            .map(|i| {
                let scene = if (i % w) < w / 2 { 0.05 } else { 0.4 };
                [0.5 * scene + 0.5 * haze[0], 0.5 * scene + 0.5 * haze[1], 0.5 * scene + 0.5 * haze[2]]
            })
            .collect();
        let before = px[w * 32 + 40][0] - px[w * 32 + 10][0];
        dehaze(&mut px, w, h, 1.0);
        let after = px[w * 32 + 40][0] - px[w * 32 + 10][0];
        assert!(after > before * 1.3, "{before} → {after}");
    }

    #[test]
    fn clarity_adds_local_contrast() {
        // Sigma is 1.2 % of 400 = 4.8 px; sample two pixels either side of
        // the edge at x = 200.
        let (w, h) = (400, 40);
        let mut px: Vec<Rgb> = (0..w * h).map(|i| if (i % w) < w / 2 { [0.12; 3] } else { [0.25; 3] }).collect();
        let before = px[20 * w + 202][0] / px[20 * w + 197][0];
        clarity(&mut px, w, h, crate::math::REC709_LUMA, 1.0);
        let after = px[20 * w + 202][0] / px[20 * w + 197][0];
        assert!(after > before, "{before} → {after}");
    }
}
