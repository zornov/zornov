// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plane and geometry helpers shared by the compositing nodes: resampling
//! single-channel planes, blurs and min/max filters on mattes, and 3×3
//! homographies.

use crate::image::{Edge, Image};
use crate::ops::gaussian_blur;

/// Bilinear resample of a `sw`×`sh` plane to `w`×`h`.
pub fn resample_plane(p: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    let (kx, ky) = (sw as f32 / w.max(1) as f32, sh as f32 / h.max(1) as f32);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(sample_plane(p, sw, sh, (x as f32 + 0.5) * kx, (y as f32 + 0.5) * ky));
        }
    }
    out
}

/// Nearest-neighbour resample (for ids, which must not blend).
pub fn nearest_plane(p: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = ((y as f32 + 0.5) * sh as f32 / h.max(1) as f32) as usize;
        for x in 0..w {
            let sx = ((x as f32 + 0.5) * sw as f32 / w.max(1) as f32) as usize;
            out.push(p[sy.min(sh - 1) * sw + sx.min(sw - 1)]);
        }
    }
    out
}

/// Bilinear sample of a plane at a pixel position (centres at +0.5),
/// edges clamped.
pub fn sample_plane(p: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    if w == 0 || h == 0 || !x.is_finite() || !y.is_finite() {
        return 0.0;
    }
    let x = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (y - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = p[y0 * w + x0] + (p[y0 * w + x1] - p[y0 * w + x0]) * fx;
    let bot = p[y1 * w + x0] + (p[y1 * w + x1] - p[y1 * w + x0]) * fx;
    top + (bot - top) * fy
}

/// Gaussian blur of a plane.
pub fn blur_plane(p: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.25 {
        return p.to_vec();
    }
    let img = Image { width: w, height: h, pixels: p.iter().map(|&v| [v, 0.0, 0.0, 0.0]).collect() };
    gaussian_blur(&img, sigma, sigma, Edge::Clamp).pixels.iter().map(|q| q[0]).collect()
}

/// Min filter (erode, `r > 0`) or max filter (dilate, `r < 0`) over a
/// round-ish window of radius `|r|` pixels: a square pass then a diamond
/// pass, separable and cheap at any radius.
pub fn erode_plane(p: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let n = r.abs().round() as usize;
    if n == 0 || w == 0 || h == 0 {
        return p.to_vec();
    }
    let shrink = r > 0.0;
    let pick = |a: f32, b: f32| if shrink { a.min(b) } else { a.max(b) };
    // Square of side 2·(0.7r) then diamond of radius 0.6r is close to a disc.
    let sq = ((n as f32) * 0.7).round() as usize;
    let dia = n.saturating_sub(sq).max(if sq == 0 { n } else { 0 });
    let mut out = p.to_vec();
    if sq > 0 {
        out = minmax_line(&out, w, h, sq, true, &pick);
        out = minmax_line(&out, w, h, sq, false, &pick);
    }
    for _ in 0..dia {
        let src = out.clone();
        for y in 0..h {
            for x in 0..w {
                let mut v = src[y * w + x];
                if x > 0 {
                    v = pick(v, src[y * w + x - 1]);
                }
                if x + 1 < w {
                    v = pick(v, src[y * w + x + 1]);
                }
                if y > 0 {
                    v = pick(v, src[(y - 1) * w + x]);
                }
                if y + 1 < h {
                    v = pick(v, src[(y + 1) * w + x]);
                }
                out[y * w + x] = v;
            }
        }
    }
    out
}

/// Running min/max along rows or columns (van Herk/Gil-Werman would be
/// O(1) per pixel; radii here are small enough for the direct form with a
/// monotonic deque).
fn minmax_line(p: &[f32], w: usize, h: usize, r: usize, horizontal: bool, pick: &impl Fn(f32, f32) -> f32) -> Vec<f32> {
    let mut out = vec![0.0; p.len()];
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for line in 0..lines {
        dq.clear();
        // Window [i - r, i + r]; push as the right edge advances.
        let mut next = 0;
        for i in 0..len {
            while next < len && next <= i + r {
                let v = p[idx(line, next)];
                while let Some(&b) = dq.back() {
                    let bv = p[idx(line, b)];
                    if pick(v, bv) == v {
                        dq.pop_back();
                    } else {
                        break;
                    }
                }
                dq.push_back(next);
                next += 1;
            }
            while let Some(&f) = dq.front() {
                if f + r < i {
                    dq.pop_front();
                } else {
                    break;
                }
            }
            out[idx(line, i)] = dq.front().map_or(p[idx(line, i)], |&f| p[idx(line, f)]);
        }
    }
    out
}

/// A 3×3 projective transform, rows; maps `[x, y, 1]`.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn apply_h(m: &Mat3, p: [f64; 2]) -> [f64; 2] {
    let x = m[0][0] * p[0] + m[0][1] * p[1] + m[0][2];
    let y = m[1][0] * p[0] + m[1][1] * p[1] + m[1][2];
    let z = m[2][0] * p[0] + m[2][1] * p[1] + m[2][2];
    if z.abs() < 1e-12 { [f64::NAN, f64::NAN] } else { [x / z, y / z] }
}

pub fn mul3(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

pub fn inverse3(m: &Mat3) -> Option<Mat3> {
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    if det.abs() < 1e-15 {
        return None;
    }
    Some(std::array::from_fn(|r| std::array::from_fn(|k| adj[r][k] / det)))
}

/// The homography taking four `from` points to four `to` points.
pub fn homography(from: &[[f64; 2]; 4], to: &[[f64; 2]; 4]) -> Option<Mat3> {
    // 8×8 system for h00..h21 with h22 = 1.
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let ([x, y], [u, v]) = (from[i], to[i]);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    let h = solve8(a)?;
    Some([[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]])
}

fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let piv = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for r in 0..8 {
            if r != col {
                let f = a[r][col] / a[col][col];
                for k in col..9 {
                    a[r][k] -= f * a[col][k];
                }
            }
        }
    }
    Some(std::array::from_fn(|i| a[i][8] / a[i][i]))
}

/// Least-squares homography from point pairs (normalised DLT solved as
/// an 8-unknown linear system with h22 = 1). Needs four or more pairs.
pub fn fit_homography(pairs: &[([f64; 2], [f64; 2])]) -> Option<Mat3> {
    if pairs.len() < 4 {
        return None;
    }
    let norm = |pts: &mut dyn Iterator<Item = [f64; 2]>| {
        let v: Vec<[f64; 2]> = pts.collect();
        let n = v.len() as f64;
        let (cx, cy) = (v.iter().map(|p| p[0]).sum::<f64>() / n, v.iter().map(|p| p[1]).sum::<f64>() / n);
        let d = v.iter().map(|p| ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()).sum::<f64>() / n;
        let s = if d > 1e-9 { std::f64::consts::SQRT_2 / d } else { 1.0 };
        [[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]]
    };
    let ta = norm(&mut pairs.iter().map(|p| p.0));
    let tb = norm(&mut pairs.iter().map(|p| p.1));
    // Normal equations AᵀA h = Aᵀb.
    let mut ata = [[0.0f64; 8]; 8];
    let mut atb = [0.0f64; 8];
    for (a, b) in pairs {
        let [x, y] = apply_h(&ta, *a);
        let [u, v] = apply_h(&tb, *b);
        let rows = [([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u), ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v)];
        for (r, rhs) in rows {
            for i in 0..8 {
                atb[i] += r[i] * rhs;
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
            }
        }
    }
    let mut m = [[0.0f64; 9]; 8];
    for i in 0..8 {
        m[i][..8].copy_from_slice(&ata[i]);
        m[i][8] = atb[i];
    }
    let h = solve8(m)?;
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]];
    let r = mul3(&inverse3(&tb)?, &mul3(&hn, &ta));
    let s = r[2][2];
    (s.abs() > 1e-12).then(|| std::array::from_fn(|i| std::array::from_fn(|j| r[i][j] / s)))
}

/// Homography by RANSAC over point pairs: the inlier set within
/// `threshold` pixels, refitted. Returns the transform and inlier count.
pub fn ransac_homography(pairs: &[([f64; 2], [f64; 2])], threshold: f64, iterations: usize, seed: u32) -> Option<(Mat3, usize)> {
    if pairs.len() < 4 {
        return None;
    }
    let mut rng = crate::math::Rng::new(seed);
    let mut best: Option<(Mat3, usize)> = None;
    let t2 = threshold * threshold;
    let is_in = |h: &Mat3, a: [f64; 2], b: [f64; 2]| {
        let p = apply_h(h, a);
        (p[0] - b[0]).powi(2) + (p[1] - b[1]).powi(2) < t2
    };
    let count = |h: &Mat3| pairs.iter().filter(|(a, b)| is_in(h, *a, *b)).count();
    for _ in 0..iterations {
        let mut idx = [0usize; 4];
        for k in 0..4 {
            loop {
                let i = rng.below(pairs.len());
                if !idx[..k].contains(&i) {
                    idx[k] = i;
                    break;
                }
            }
        }
        let from = idx.map(|i| pairs[i].0);
        let to = idx.map(|i| pairs[i].1);
        let Some(h) = homography(&from, &to) else { continue };
        let n = count(&h);
        if best.as_ref().is_none_or(|b| n > b.1) {
            best = Some((h, n));
        }
    }
    let (h, _) = best?;
    let set: Vec<_> = pairs.iter().filter(|(a, b)| is_in(&h, *a, *b)).copied().collect();
    let refit = fit_homography(&set).unwrap_or(h);
    let n = count(&refit);
    Some((refit, n))
}

/// Warps an image by a homography from output to source pixels
/// (`src = m · out`), with the given edge mode.
pub fn warp_h(img: &Image, m: &Mat3, edge: Edge) -> Image {
    Image::from_fn(img.width, img.height, |x, y| {
        let s = apply_h(m, [x as f64 + 0.5, y as f64 + 0.5]);
        if !s[0].is_finite() {
            return [0.0; 4];
        }
        img.sample(s[0] as f32, s[1] as f32, edge)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erode_and_dilate_move_an_edge_by_the_radius() {
        let (w, h) = (40, 9);
        let p: Vec<f32> = (0..w * h).map(|i| if i % w < 20 { 1.0 } else { 0.0 }).collect();
        let e = erode_plane(&p, w, h, 4.0);
        let d = erode_plane(&p, w, h, -4.0);
        let edge = |q: &[f32]| (0..w).filter(|&x| q[4 * w + x] > 0.5).count();
        assert_eq!(edge(&e), 16);
        assert_eq!(edge(&d), 24);
    }

    #[test]
    fn homography_round_trips() {
        let from = [[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]];
        let to = [[10.0, 5.0], [90.0, 12.0], [110.0, 70.0], [-5.0, 60.0]];
        let h = homography(&from, &to).unwrap();
        for i in 0..4 {
            let p = apply_h(&h, from[i]);
            assert!((p[0] - to[i][0]).abs() < 1e-6 && (p[1] - to[i][1]).abs() < 1e-6);
        }
        let pairs: Vec<_> = (0..30).map(|i| {
            let a = [(i * 37 % 100) as f64, (i * 53 % 50) as f64];
            (a, apply_h(&h, a))
        }).collect();
        let mut noisy = pairs.clone();
        noisy[3].1 = [500.0, 500.0];
        let (r, n) = ransac_homography(&noisy, 1.0, 200, 7).unwrap();
        assert!(n >= 29);
        let p = apply_h(&r, [50.0, 25.0]);
        let q = apply_h(&h, [50.0, 25.0]);
        assert!((p[0] - q[0]).abs() < 1e-3 && (p[1] - q[1]).abs() < 1e-3);
    }
}
