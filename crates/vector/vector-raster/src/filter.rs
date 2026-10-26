// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Raster effects on a layer's pixels (premultiplied): Gaussian blur,
//! drop and inner shadows, outer and inner glows. Distances arrive in
//! device pixels, so effects scale with the art.

use crate::blend::over;
use crate::prepare::PFilter;

pub(crate) fn apply(px: &mut [[f32; 4]], w: usize, h: usize, filters: &[PFilter], threads: usize) {
    for f in filters {
        match *f {
            PFilter::Blur { sx, sy } => blur_rgba(px, w, h, sx, sy, threads),
            PFilter::Opacity { amount } => px.iter_mut().for_each(|p| *p = p.map(|v| v * amount)),
            PFilter::DropShadow { dx, dy, sigma, color, spread } => {
                let mut a = shifted_alpha(px, w, h, dx, dy, false);
                dilate(&mut a, w, h, spread);
                blur_plane(&mut a, w, h, sigma, sigma, threads);
                for (p, &k) in px.iter_mut().zip(&a) {
                    *p = over(color.map(|c| c * k), *p);
                }
            }
            PFilter::OuterGlow { sigma, color, spread } => {
                let mut a = shifted_alpha(px, w, h, 0.0, 0.0, false);
                dilate(&mut a, w, h, spread);
                blur_plane(&mut a, w, h, sigma, sigma, threads);
                for (p, &k) in px.iter_mut().zip(&a) {
                    *p = over(color.map(|c| c * k), *p);
                }
            }
            PFilter::InnerShadow { dx, dy, sigma, color } => {
                // The outside, shifted and blurred, seen only inside the shape.
                let mut a = shifted_alpha(px, w, h, dx, dy, true);
                blur_plane(&mut a, w, h, sigma, sigma, threads);
                for (p, &k) in px.iter_mut().zip(&a) {
                    let s = color.map(|c| c * k * p[3]);
                    *p = atop(*p, s);
                }
            }
            PFilter::InnerGlow { sigma, color } => {
                let mut a = shifted_alpha(px, w, h, 0.0, 0.0, true);
                blur_plane(&mut a, w, h, sigma, sigma, threads);
                for (p, &k) in px.iter_mut().zip(&a) {
                    let s = color.map(|c| c * k * p[3]);
                    *p = atop(*p, s);
                }
            }
        }
    }
}

/// `s` over `d` keeping `d`'s alpha (`s` is already limited to `d`).
fn atop(d: [f32; 4], s: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - s[3];
    [s[0] + d[0] * k, s[1] + d[1] * k, s[2] + d[2] * k, d[3]]
}

/// The alpha plane moved by (`dx`, `dy`) with bilinear weights;
/// `inverted` gives `1 - alpha` (and 1 outside the image).
fn shifted_alpha(px: &[[f32; 4]], w: usize, h: usize, dx: f32, dy: f32, inverted: bool) -> Vec<f32> {
    let outside = if inverted { 1.0 } else { 0.0 };
    let get = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            outside
        } else {
            let a = px[y as usize * w + x as usize][3];
            if inverted { 1.0 - a } else { a }
        }
    };
    let (ix, iy) = (dx.floor(), dy.floor());
    let (fx, fy) = (dx - ix, dy - iy);
    let (ix, iy) = (ix as i64, iy as i64);
    let mut out = vec![0.0; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            // Sample at (x - dx, y - dy).
            let (sx, sy) = (x - ix, y - iy);
            let a = get(sx, sy) * (1.0 - fx) * (1.0 - fy) + get(sx - 1, sy) * fx * (1.0 - fy) + get(sx, sy - 1) * (1.0 - fx) * fy + get(sx - 1, sy - 1) * fx * fy;
            out[y as usize * w + x as usize] = a;
        }
    }
    out
}

/// Grows the plane by `r` pixels (a separable maximum filter, fractional
/// radii blended).
fn dilate(a: &mut [f32], w: usize, h: usize, r: f32) {
    if r <= 0.0 {
        return;
    }
    let ri = r.ceil().min(256.0) as usize;
    let frac = r - (ri as f32 - 1.0);
    let pass = |src: &[f32], dst: &mut [f32], len: usize, count: usize, stride_i: usize, stride_j: usize| {
        for j in 0..count {
            for i in 0..len {
                let mut m = src[j * stride_j + i * stride_i];
                let mut edge = 0.0f32;
                for k in 1..=ri {
                    let mut best = 0.0f32;
                    if i >= k {
                        best = best.max(src[j * stride_j + (i - k) * stride_i]);
                    }
                    if i + k < len {
                        best = best.max(src[j * stride_j + (i + k) * stride_i]);
                    }
                    if k < ri {
                        m = m.max(best);
                    } else {
                        edge = best;
                    }
                }
                dst[j * stride_j + i * stride_i] = m.max(edge * frac.clamp(0.0, 1.0));
            }
        }
    };
    let mut tmp = vec![0.0; a.len()];
    pass(a, &mut tmp, w, h, 1, w);
    pass(&tmp, a, h, w, w, 1);
}

/// Gaussian weights for deviation `s`, radius 3s.
fn kernel(s: f32) -> Vec<f32> {
    let r = (3.0 * s).ceil().max(1.0) as usize;
    let mut k: Vec<f32> = (0..=r).map(|i| (-(i as f32 * i as f32) / (2.0 * s * s)).exp()).collect();
    let sum: f32 = k[0] + 2.0 * k[1..].iter().sum::<f32>();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// Box widths whose three passes approximate a Gaussian of deviation `s`.
fn boxes(s: f32) -> [usize; 3] {
    let ideal = (12.0 * s * s / 3.0 + 1.0).sqrt();
    let mut wl = ideal.floor() as usize;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m = ((12.0 * s * s - 3.0 * (wl * wl) as f32 - 12.0 * wl as f32 - 9.0) / (-4.0 * wl as f32 - 4.0)).round() as usize;
    [0, 1, 2].map(|i| if i < m { wl } else { wu })
}

/// Blurs one line (`len` values `stride` apart starting at `off`) into `out`.
fn blur_line(src: &[f32], out: &mut [f32], len: usize, s: f32) {
    if s < 12.0 {
        let k = kernel(s);
        let r = k.len() - 1;
        for i in 0..len {
            let mut acc = src[i] * k[0];
            for (j, kj) in k.iter().enumerate().skip(1) {
                if i >= j {
                    acc += src[i - j] * kj;
                }
                if i + j < len {
                    acc += src[i + j] * kj;
                }
            }
            let _ = r;
            out[i] = acc;
        }
    } else {
        // Large deviations: three running box averages (zero outside).
        let mut a = src[..len].to_vec();
        let mut b = vec![0.0f32; len];
        for bw in boxes(s) {
            let r = (bw / 2) as i64;
            let inv = 1.0 / bw as f32;
            let mut acc = 0.0f32;
            for i in -r..=r {
                if i >= 0 && (i as usize) < len {
                    acc += a[i as usize];
                }
            }
            for i in 0..len as i64 {
                b[i as usize] = acc * inv;
                let add = i + r + 1;
                let sub = i - r;
                if add >= 0 && (add as usize) < len {
                    acc += a[add as usize];
                }
                if sub >= 0 && (sub as usize) < len {
                    acc -= a[sub as usize];
                }
            }
            std::mem::swap(&mut a, &mut b);
        }
        out[..len].copy_from_slice(&a);
    }
}

fn par_rows(data: &mut [f32], w: usize, h: usize, threads: usize, f: impl Fn(&mut [f32]) + Sync) {
    let n = crate::threads_for(threads).min(h.max(1));
    let rows_per = h.div_ceil(n.max(1)).max(1);
    std::thread::scope(|s| {
        for chunk in data.chunks_mut(rows_per * w) {
            let f = &f;
            s.spawn(move || {
                for row in chunk.chunks_mut(w) {
                    f(row);
                }
            });
        }
    });
}

fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    const B: usize = 32;
    for by in (0..h).step_by(B) {
        for bx in (0..w).step_by(B) {
            for y in by..(by + B).min(h) {
                for x in bx..(bx + B).min(w) {
                    out[x * h + y] = src[y * w + x];
                }
            }
        }
    }
    out
}

pub(crate) fn blur_plane(a: &mut [f32], w: usize, h: usize, sx: f32, sy: f32, threads: usize) {
    if w == 0 || h == 0 {
        return;
    }
    if sx > 0.05 {
        par_rows(a, w, h, threads, |row| {
            let src = row.to_vec();
            blur_line(&src, row, row.len(), sx);
        });
    }
    if sy > 0.05 {
        let mut t = transpose(a, w, h);
        par_rows(&mut t, h, w, threads, |col| {
            let src = col.to_vec();
            blur_line(&src, col, col.len(), sy);
        });
        a.copy_from_slice(&transpose(&t, h, w));
    }
}

fn blur_rgba(px: &mut [[f32; 4]], w: usize, h: usize, sx: f32, sy: f32, threads: usize) {
    for c in 0..4 {
        let mut plane: Vec<f32> = px.iter().map(|p| p[c]).collect();
        blur_plane(&mut plane, w, h, sx, sy, threads);
        for (p, v) in px.iter_mut().zip(plane) {
            p[c] = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_energy_and_spreads() {
        let (w, h) = (64, 64);
        let mut a = vec![0.0f32; w * h];
        a[32 * w + 32] = 1.0;
        blur_plane(&mut a, w, h, 3.0, 3.0, 2);
        let sum: f32 = a.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        assert!(a[32 * w + 35] > 0.0 && a[32 * w + 32] < 0.1);
        // Symmetric.
        assert!((a[32 * w + 29] - a[32 * w + 35]).abs() < 1e-6);
        // Large deviations use box passes and still keep the energy.
        let mut b = vec![0.0f32; 256 * 256];
        b[128 * 256 + 128] = 1.0;
        blur_plane(&mut b, 256, 256, 20.0, 20.0, 2);
        let sum: f32 = b.iter().sum();
        assert!((sum - 1.0).abs() < 1e-2, "{sum}");
    }

    #[test]
    fn dilate_grows() {
        let (w, h) = (9, 1);
        let mut a = vec![0.0; 9];
        a[4] = 1.0;
        dilate(&mut a, w, h, 2.0);
        assert_eq!(&a[2..7], &[1.0; 5]);
        assert_eq!(a[1], 0.0);
    }
}
