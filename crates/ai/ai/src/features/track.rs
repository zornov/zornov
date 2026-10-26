// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion tracking: a point, a plane or the camera itself, followed through
//! decoded frames with classical computer vision (no model to download).
//!
//! * [`Gray`] / [`Pyramid`]: luma pictures and their half-size levels.
//! * [`RegionTracker`]: corners found inside a region (a circle around a
//!   clicked point, or the four corners of a plane) are followed frame to
//!   frame with pyramidal Lucas–Kanade, checked forwards and backwards, and
//!   a robust fit (RANSAC) of a translation, similarity or homography from
//!   the reference frame to each frame says where the region went. Every
//!   frame carries a confidence (how many corners agree); when too few do,
//!   the region is lost and searched for near where it was last seen.
//! * [`camera_motion`]: the whole picture's motion from one frame to the
//!   next (a similarity: shift, turn, zoom), for stabilizing.
//! * [`plan_stabilize`]: a smoothed (or locked) camera path from that
//!   motion, the correction that takes each frame onto it, and the zoom
//!   that hides the moving edges, with the correction eased back when the
//!   zoom would pass its limit.
//!
//! Lucas–Kanade: Bouguet, "Pyramidal implementation of the Lucas Kanade
//! feature tracker" (2001). Corners: Shi & Tomasi, "Good features to
//! track" (1994). Forward–backward error: Kalal et al. (2010).

use crate::error::{invalid, Error, Result};
use crate::jobs::JobCtx;

/// A single-channel picture, values 0..1, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn new(width: usize, height: usize, data: Vec<f32>) -> Result<Gray> {
        if width < 8 || height < 8 || data.len() != width * height {
            return invalid(format!("gray picture {width}x{height} with {} values", data.len()));
        }
        Ok(Gray { width, height, data })
    }

    /// Rec. 709 luma of an RGBA frame.
    pub fn from_frame(f: &crate::frame::Frame) -> Gray {
        Gray { width: f.width, height: f.height, data: f.luma() }
    }

    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.data[y * self.width + x]
    }

    /// Bilinear sample (edges clamped).
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (ix, iy) = (x0 as isize, y0 as isize);
        let a = self.at(ix, iy);
        let b = self.at(ix + 1, iy);
        let c = self.at(ix, iy + 1);
        let d = self.at(ix + 1, iy + 1);
        (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
    }

    /// Half size, 1-2-1 filtered (a pyramid level).
    fn half(&self) -> Gray {
        let (w, h) = (self.width.div_ceil(2).max(1), self.height.div_ceil(2).max(1));
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (2 * x as isize, 2 * y as isize);
                let mut acc = 0.0;
                for (dy, wy) in [(-1isize, 1.0f32), (0, 2.0), (1, 1.0)] {
                    for (dx, wx) in [(-1isize, 1.0f32), (0, 2.0), (1, 1.0)] {
                        acc += self.at(sx + dx, sy + dy) * wx * wy;
                    }
                }
                out[y * w + x] = acc / 16.0;
            }
        }
        Gray { width: w, height: h, data: out }
    }
}

/// A picture and its half-size levels (level 0 is full size).
#[derive(Clone, Debug)]
pub struct Pyramid {
    pub levels: Vec<Gray>,
}

impl Pyramid {
    pub fn new(g: Gray, levels: usize) -> Pyramid {
        let mut out = vec![g];
        while out.len() < levels.max(1) {
            let last = out.last().expect("one level");
            if last.width < 40 || last.height < 40 {
                break;
            }
            let next = last.half();
            out.push(next);
        }
        Pyramid { levels: out }
    }

    pub fn base(&self) -> &Gray {
        &self.levels[0]
    }
}

/// Lucas–Kanade window half-size (a 15×15 window).
const WIN: isize = 7;

/// Follows the point `p` of `a` into `b`, starting from the guess `p +
/// guess`. `None` when the point leaves the picture, has no texture to
/// follow or the iteration does not settle.
pub fn lk(a: &Pyramid, b: &Pyramid, p: (f32, f32), guess: (f32, f32), win: isize) -> Option<(f32, f32)> {
    let levels = a.levels.len().min(b.levels.len());
    let mut g = (guess.0 / (1 << (levels - 1)) as f32, guess.1 / (1 << (levels - 1)) as f32);
    let n = ((2 * win + 1) * (2 * win + 1)) as usize;
    let mut tmpl = Vec::with_capacity(n);
    let mut grads = Vec::with_capacity(n);
    for l in (0..levels).rev() {
        let (ia, ib) = (&a.levels[l], &b.levels[l]);
        let s = (1 << l) as f32;
        let (px, py) = (p.0 / s, p.1 / s);
        if px < 1.0 || py < 1.0 || px > ia.width as f32 - 2.0 || py > ia.height as f32 - 2.0 {
            return None;
        }
        tmpl.clear();
        grads.clear();
        let (mut gxx, mut gxy, mut gyy) = (0.0f32, 0.0f32, 0.0f32);
        for dy in -win..=win {
            for dx in -win..=win {
                let (x, y) = (px + dx as f32, py + dy as f32);
                let ix = (ia.sample(x + 1.0, y) - ia.sample(x - 1.0, y)) * 0.5;
                let iy = (ia.sample(x, y + 1.0) - ia.sample(x, y - 1.0)) * 0.5;
                tmpl.push(ia.sample(x, y));
                grads.push((ix, iy));
                gxx += ix * ix;
                gxy += ix * iy;
                gyy += iy * iy;
            }
        }
        let det = gxx * gyy - gxy * gxy;
        // Smallest eigenvalue of the structure tensor, per pixel.
        let tr = gxx + gyy;
        let min_eig = (tr - ((gxx - gyy).powi(2) + 4.0 * gxy * gxy).sqrt()) * 0.5 / n as f32;
        if det.abs() < 1e-12 || min_eig < 1e-6 {
            return None;
        }
        let mut v = (0.0f32, 0.0f32);
        for _ in 0..24 {
            let (mut bx, mut by) = (0.0f32, 0.0f32);
            let mut k = 0;
            for dy in -win..=win {
                for dx in -win..=win {
                    let d = tmpl[k] - ib.sample(px + g.0 + v.0 + dx as f32, py + g.1 + v.1 + dy as f32);
                    bx += d * grads[k].0;
                    by += d * grads[k].1;
                    k += 1;
                }
            }
            let dvx = (gyy * bx - gxy * by) / det;
            let dvy = (gxx * by - gxy * bx) / det;
            v.0 += dvx;
            v.1 += dvy;
            if !v.0.is_finite() || !v.1.is_finite() {
                return None;
            }
            if dvx * dvx + dvy * dvy < 1e-4 {
                break;
            }
        }
        g = (g.0 + v.0, g.1 + v.1);
        if l > 0 {
            g = (g.0 * 2.0, g.1 * 2.0);
        }
    }
    let q = (p.0 + g.0, p.1 + g.1);
    let base = b.base();
    if q.0 < 0.0 || q.1 < 0.0 || q.0 > base.width as f32 - 1.0 || q.1 > base.height as f32 - 1.0 {
        return None;
    }
    Some(q)
}

/// Normalised cross-correlation of the windows around `p` in `a` and `q`
/// in `b` (−1..1; 1 is the same patch).
pub fn ncc(a: &Gray, p: (f32, f32), b: &Gray, q: (f32, f32), win: isize) -> f32 {
    let mut xs = Vec::with_capacity(((2 * win + 1) * (2 * win + 1)) as usize);
    let mut ys = Vec::with_capacity(xs.capacity());
    for dy in -win..=win {
        for dx in -win..=win {
            xs.push(a.sample(p.0 + dx as f32, p.1 + dy as f32));
            ys.push(b.sample(q.0 + dx as f32, q.1 + dy as f32));
        }
    }
    let n = xs.len() as f32;
    let (mx, my) = (xs.iter().sum::<f32>() / n, ys.iter().sum::<f32>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(&ys) {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    if sxx < 1e-8 || syy < 1e-8 {
        return if sxx < 1e-8 && syy < 1e-8 { 1.0 } else { 0.0 };
    }
    sxy / (sxx * syy).sqrt()
}

/// Strong corners (Shi–Tomasi) of `g` inside `keep`, at least `spacing`
/// pixels apart, best first, at most `max`.
pub fn corners(g: &Gray, max: usize, spacing: f32, keep: &dyn Fn(f32, f32) -> bool) -> Vec<(f32, f32)> {
    let (w, h) = (g.width, g.height);
    // Integral images of the gradient products.
    let stride = w + 1;
    let mut sxx = vec![0f64; stride * (h + 1)];
    let mut sxy = vec![0f64; stride * (h + 1)];
    let mut syy = vec![0f64; stride * (h + 1)];
    for y in 0..h {
        let (mut rxx, mut rxy, mut ryy) = (0f64, 0f64, 0f64);
        for x in 0..w {
            let ix = ((g.at(x as isize + 1, y as isize) - g.at(x as isize - 1, y as isize)) * 0.5) as f64;
            let iy = ((g.at(x as isize, y as isize + 1) - g.at(x as isize, y as isize - 1)) * 0.5) as f64;
            rxx += ix * ix;
            rxy += ix * iy;
            ryy += iy * iy;
            let i = (y + 1) * stride + x + 1;
            sxx[i] = sxx[i - stride] + rxx;
            sxy[i] = sxy[i - stride] + rxy;
            syy[i] = syy[i - stride] + ryy;
        }
    }
    let r = 3usize;
    let boxsum = |s: &[f64], x: usize, y: usize| {
        let (x0, y0, x1, y1) = (x - r, y - r, x + r + 1, y + r + 1);
        s[y1 * stride + x1] - s[y0 * stride + x1] - s[y1 * stride + x0] + s[y0 * stride + x0]
    };
    let cell = spacing.max(2.0) as usize;
    let (cw, ch) = (w.div_ceil(cell), h.div_ceil(cell));
    let mut best: Vec<(f32, usize, usize)> = vec![(0.0, 0, 0); cw * ch];
    let margin = (WIN as usize + 2).max(r + 1);
    if w <= 2 * margin || h <= 2 * margin {
        return Vec::new();
    }
    let mut top = 0f32;
    for y in margin..h - margin {
        for x in margin..w - margin {
            if !keep(x as f32, y as f32) {
                continue;
            }
            let (a, b, c) = (boxsum(&sxx, x, y), boxsum(&sxy, x, y), boxsum(&syy, x, y));
            let e = (((a + c) - ((a - c).powi(2) + 4.0 * b * b).sqrt()) * 0.5) as f32;
            let k = (y / cell) * cw + x / cell;
            if e > best[k].0 {
                best[k] = (e, x, y);
            }
            top = top.max(e);
        }
    }
    let floor = (top * 0.02).max(1e-5);
    let mut out: Vec<(f32, usize, usize)> = best.into_iter().filter(|b| b.0 > floor).collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.truncate(max);
    out.into_iter().map(|(_, x, y)| (x as f32, y as f32)).collect()
}

/// A 3×3 projective transform, row-major, acting on column vectors.
pub type Mat3 = [f64; 9];

pub const IDENTITY: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

pub fn apply(m: &Mat3, p: (f64, f64)) -> (f64, f64) {
    let w = m[6] * p.0 + m[7] * p.1 + m[8];
    let w = if w.abs() < 1e-12 { 1e-12 } else { w };
    ((m[0] * p.0 + m[1] * p.1 + m[2]) / w, (m[3] * p.0 + m[4] * p.1 + m[5]) / w)
}

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    o
}

pub fn invert(m: &Mat3) -> Option<Mat3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-14 {
        return None;
    }
    let k = 1.0 / det;
    Some([(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k, (f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k, (d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k])
}

/// What a region may do from frame to frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Only move.
    Translation,
    /// Move, turn and scale (a point with its surroundings, a face).
    Similarity,
    /// Any perspective change (a flat surface: a screen, a sign, a wall).
    Perspective,
}

impl Motion {
    fn minimum(self) -> usize {
        match self {
            Motion::Translation => 1,
            Motion::Similarity => 2,
            Motion::Perspective => 4,
        }
    }
}

/// Least-squares fit of `m` mapping `src` to `dst`.
pub fn fit(m: Motion, src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<Mat3> {
    let n = src.len();
    if n < m.minimum() || n != dst.len() {
        return None;
    }
    match m {
        Motion::Translation => {
            let (mut tx, mut ty) = (0.0, 0.0);
            for (s, d) in src.iter().zip(dst) {
                tx += d.0 - s.0;
                ty += d.1 - s.1;
            }
            Some([1.0, 0.0, tx / n as f64, 0.0, 1.0, ty / n as f64, 0.0, 0.0, 1.0])
        }
        Motion::Similarity => {
            // Umeyama-style closed form for x' = a x − b y + tx, y' = b x + a y + ty.
            let cs = mean(src);
            let cd = mean(dst);
            let (mut sab, mut sbb, mut ss) = (0.0, 0.0, 0.0);
            for (s, d) in src.iter().zip(dst) {
                let (sx, sy) = (s.0 - cs.0, s.1 - cs.1);
                let (dx, dy) = (d.0 - cd.0, d.1 - cd.1);
                sab += sx * dx + sy * dy;
                sbb += sx * dy - sy * dx;
                ss += sx * sx + sy * sy;
            }
            if ss < 1e-9 {
                return fit(Motion::Translation, src, dst);
            }
            let (a, b) = (sab / ss, sbb / ss);
            let tx = cd.0 - (a * cs.0 - b * cs.1);
            let ty = cd.1 - (b * cs.0 + a * cs.1);
            Some([a, -b, tx, b, a, ty, 0.0, 0.0, 1.0])
        }
        Motion::Perspective => homography(src, dst),
    }
}

fn mean(p: &[(f64, f64)]) -> (f64, f64) {
    let n = p.len().max(1) as f64;
    (p.iter().map(|q| q.0).sum::<f64>() / n, p.iter().map(|q| q.1).sum::<f64>() / n)
}

/// Normalisation (Hartley): centre on the mean, mean distance √2.
fn normaliser(p: &[(f64, f64)]) -> Mat3 {
    let c = mean(p);
    let d = p.iter().map(|q| ((q.0 - c.0).powi(2) + (q.1 - c.1).powi(2)).sqrt()).sum::<f64>() / p.len().max(1) as f64;
    let s = if d < 1e-9 { 1.0 } else { std::f64::consts::SQRT_2 / d };
    [s, 0.0, -s * c.0, 0.0, s, -s * c.1, 0.0, 0.0, 1.0]
}

/// Homography by normalised DLT with h33 = 1 (least squares through the
/// normal equations).
fn homography(src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<Mat3> {
    let (ts, td) = (normaliser(src), normaliser(dst));
    let mut ata = [[0.0f64; 8]; 8];
    let mut atb = [0.0f64; 8];
    for (s, d) in src.iter().zip(dst) {
        let (x, y) = apply(&ts, *s);
        let (u, v) = apply(&td, *d);
        let rows = [([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u), ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v)];
        for (r, b) in rows {
            for i in 0..8 {
                atb[i] += r[i] * b;
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
            }
        }
    }
    let h = solve8(ata, atb)?;
    let hn = [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0];
    let out = mul(&invert(&td)?, &mul(&hn, &ts));
    let k = out[8];
    if k.abs() < 1e-12 || out.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(out.map(|v| v / k))
}

/// Gaussian elimination with partial pivoting.
fn solve8(mut a: [[f64; 8]; 8], mut b: [f64; 8]) -> Option<[f64; 8]> {
    for c in 0..8 {
        let p = (c..8).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..8 {
            let k = a[r][c] / a[c][c];
            for j in c..8 {
                a[r][j] -= k * a[c][j];
            }
            b[r] -= k * b[c];
        }
    }
    let mut x = [0.0; 8];
    for r in (0..8).rev() {
        let s: f64 = (r + 1..8).map(|j| a[r][j] * x[j]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

/// A small, fast deterministic generator for RANSAC samples.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// RANSAC fit: the transform and which pairs agree with it (within
/// `tol` pixels).
pub fn ransac(m: Motion, src: &[(f64, f64)], dst: &[(f64, f64)], tol: f64) -> Option<(Mat3, Vec<bool>)> {
    let n = src.len();
    let k = m.minimum();
    if n < k {
        return None;
    }
    let score = |h: &Mat3| -> (usize, Vec<bool>) {
        let mut inl = vec![false; n];
        let mut c = 0;
        for i in 0..n {
            let q = apply(h, src[i]);
            if (q.0 - dst[i].0).powi(2) + (q.1 - dst[i].1).powi(2) <= tol * tol {
                inl[i] = true;
                c += 1;
            }
        }
        (c, inl)
    };
    let mut best: Option<(usize, Mat3, Vec<bool>)> = None;
    if n == k {
        let h = fit(m, src, dst)?;
        let (c, inl) = score(&h);
        best = Some((c, h, inl));
    } else {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ n as u64);
        let iters = match m {
            Motion::Translation => 40,
            Motion::Similarity => 120,
            Motion::Perspective => 300,
        };
        let mut pick = Vec::with_capacity(k);
        for _ in 0..iters {
            pick.clear();
            while pick.len() < k {
                let i = rng.below(n);
                if !pick.contains(&i) {
                    pick.push(i);
                }
            }
            let s: Vec<_> = pick.iter().map(|&i| src[i]).collect();
            let d: Vec<_> = pick.iter().map(|&i| dst[i]).collect();
            let Some(h) = fit(m, &s, &d) else { continue };
            let (c, inl) = score(&h);
            if best.as_ref().is_none_or(|b| c > b.0) {
                best = Some((c, h, inl));
            }
        }
    }
    let (c, h, inl) = best?;
    if c < k {
        return None;
    }
    // Refit on every inlier, then take the inliers of the refit.
    let s: Vec<_> = (0..n).filter(|&i| inl[i]).map(|i| src[i]).collect();
    let d: Vec<_> = (0..n).filter(|&i| inl[i]).map(|i| dst[i]).collect();
    let h2 = fit(m, &s, &d).unwrap_or(h);
    let (c2, inl2) = score(&h2);
    if c2 >= c { Some((h2, inl2)) } else { Some((h, inl)) }
}

/// The region a [`RegionTracker`] follows, in reference-frame pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Region {
    /// A point and the circle of picture around it that moves with it.
    Point { x: f64, y: f64, radius: f64 },
    /// A flat four-sided surface (corners in order around it).
    Plane([(f64, f64); 4]),
}

impl Region {
    /// The region's outline points (the point itself, or the four corners).
    pub fn points(&self) -> Vec<(f64, f64)> {
        match self {
            Region::Point { x, y, .. } => vec![(*x, *y)],
            Region::Plane(c) => c.to_vec(),
        }
    }

    fn contains(&self, h: &Mat3, p: (f64, f64), grow: f64) -> bool {
        match self {
            Region::Point { x, y, radius } => {
                let c = apply(h, (*x, *y));
                let r = radius * scale_of(h) * grow;
                (p.0 - c.0).powi(2) + (p.1 - c.1).powi(2) <= r * r
            }
            Region::Plane(cs) => {
                let q: Vec<(f64, f64)> = cs.iter().map(|c| apply(h, *c)).collect();
                let c = mean(&q);
                let q: Vec<(f64, f64)> = q.iter().map(|p| (c.0 + (p.0 - c.0) * grow, c.1 + (p.1 - c.1) * grow)).collect();
                inside_quad(&q, p)
            }
        }
    }

    /// A bounding box (x0, y0, x1, y1) of the region under `h`.
    fn bounds(&self, h: &Mat3) -> (f64, f64, f64, f64) {
        match self {
            Region::Point { x, y, radius } => {
                let c = apply(h, (*x, *y));
                let r = radius * scale_of(h);
                (c.0 - r, c.1 - r, c.0 + r, c.1 + r)
            }
            Region::Plane(cs) => {
                let q: Vec<(f64, f64)> = cs.iter().map(|c| apply(h, *c)).collect();
                (q.iter().map(|p| p.0).fold(f64::MAX, f64::min), q.iter().map(|p| p.1).fold(f64::MAX, f64::min), q.iter().map(|p| p.0).fold(f64::MIN, f64::max), q.iter().map(|p| p.1).fold(f64::MIN, f64::max))
            }
        }
    }
}

/// How much `h` scales lengths near the origin of its frame.
pub fn scale_of(h: &Mat3) -> f64 {
    ((h[0] * h[4] - h[1] * h[3]).abs()).sqrt().clamp(0.05, 20.0)
}

/// Inside a (convex or mildly concave) quad, by winding.
pub fn inside_quad(q: &[(f64, f64)], p: (f64, f64)) -> bool {
    let mut wn = 0i32;
    for i in 0..q.len() {
        let (a, b) = (q[i], q[(i + 1) % q.len()]);
        let cross = (b.0 - a.0) * (p.1 - a.1) - (p.0 - a.0) * (b.1 - a.1);
        if a.1 <= p.1 {
            if b.1 > p.1 && cross > 0.0 {
                wn += 1;
            }
        } else if b.1 <= p.1 && cross < 0.0 {
            wn -= 1;
        }
    }
    wn != 0
}

/// Where the region is in one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Frame index (in the frames the tracker was given).
    pub frame: usize,
    /// Reference frame → this frame.
    pub h: Mat3,
    /// 0..1: how sure the tracker is (the share of corners that agree, and
    /// whether there were enough of them).
    pub confidence: f32,
    /// The region could not be found here; `h` is where it was last seen.
    pub lost: bool,
}

impl Sample {
    /// The region's outline points in this frame.
    pub fn points(&self, region: &Region) -> Vec<(f64, f64)> {
        region.points().iter().map(|p| apply(&self.h, *p)).collect()
    }
}

/// Tracker settings.
#[derive(Clone, Copy, Debug)]
pub struct TrackSettings {
    pub motion: Motion,
    /// Corners kept alive in the region.
    pub features: usize,
    /// Forward–backward disagreement allowed (pixels at tracking size).
    pub fb_tol: f32,
    /// Below this confidence a frame counts as lost.
    pub min_confidence: f32,
}

impl Default for TrackSettings {
    fn default() -> Self {
        TrackSettings { motion: Motion::Similarity, features: 80, fb_tol: 1.0, min_confidence: 0.25 }
    }
}

/// Follows a region from its reference frame, one frame at a time.
pub struct RegionTracker {
    region: Region,
    settings: TrackSettings,
    reference: Pyramid,
    prev: Pyramid,
    /// Corners: (position in the reference frame, position in `prev`).
    feats: Vec<((f64, f64), (f64, f64))>,
    h: Mat3,
    /// Last frame-to-frame motion (prediction for the next frame).
    velocity: Mat3,
    lost_for: usize,
    /// The point region's own patch, for a region with too little texture
    /// around it to find corners.
    lone: bool,
}

impl RegionTracker {
    /// Starts on `frame` (the reference) with `region` drawn on it.
    pub fn new(frame: Pyramid, region: Region, settings: TrackSettings) -> Result<RegionTracker> {
        let mut t = RegionTracker { region, settings, reference: frame.clone(), prev: frame, feats: Vec::new(), h: IDENTITY, velocity: IDENTITY, lost_for: 0, lone: false };
        t.replenish();
        if t.feats.len() < t.settings.motion.minimum() {
            match &t.region {
                Region::Point { x, y, .. } => {
                    // Too plain to find corners: follow the point's own patch.
                    t.lone = true;
                    t.feats = vec![((*x, *y), (*x, *y))];
                }
                Region::Plane(_) => {
                    if t.feats.is_empty() {
                        return invalid("the surface has no detail to follow: pick a region with texture or edges");
                    }
                }
            }
        }
        Ok(t)
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The reference-to-current transform so far.
    pub fn current(&self) -> Mat3 {
        self.h
    }

    /// Adds corners found inside the region where it is now.
    fn replenish(&mut self) {
        let want = self.settings.features;
        if self.feats.len() >= want * 3 / 4 {
            return;
        }
        let base = self.prev.base();
        let diag = ((base.width * base.width + base.height * base.height) as f64).sqrt();
        let (x0, y0, x1, y1) = self.region.bounds(&self.h);
        let area = ((x1 - x0).max(1.0) * (y1 - y0).max(1.0)).max(1.0);
        let spacing = ((area / want as f64).sqrt() * 0.7).clamp(3.0, diag / 20.0) as f32;
        let h = self.h;
        let region = self.region.clone();
        let found = corners(base, want * 2, spacing, &|x, y| region.contains(&h, (x as f64, y as f64), 1.0));
        let Some(inv) = invert(&self.h) else { return };
        let min_gap = (spacing as f64 * 0.8).powi(2);
        for c in found {
            if self.feats.len() >= want {
                break;
            }
            let p = (c.0 as f64, c.1 as f64);
            if self.feats.iter().any(|f| (f.1 .0 - p.0).powi(2) + (f.1 .1 - p.1).powi(2) < min_gap) {
                continue;
            }
            self.feats.push((apply(&inv, p), p));
        }
    }

    /// Moves on to the next frame.
    pub fn step(&mut self, next: Pyramid, frame: usize) -> Sample {
        let tol = self.settings.fb_tol;
        let win = if self.lone { 15 } else { WIN };
        // (reference position, position in the last frame, position now)
        let mut moved = Vec::new();
        let tracked = self.feats.len();
        for &(r, p) in &self.feats {
            let pp = (p.0 as f32, p.1 as f32);
            let guess = {
                let q = apply(&self.velocity, p);
                ((q.0 - p.0) as f32, (q.1 - p.1) as f32)
            };
            let Some(q) = lk(&self.prev, &next, pp, guess, win) else { continue };
            let Some(back) = lk(&next, &self.prev, q, (pp.0 - q.0, pp.1 - q.1), win) else { continue };
            if (back.0 - pp.0).powi(2) + (back.1 - pp.1).powi(2) > tol * tol {
                continue;
            }
            // Against the reference picture too, from where this frame's
            // tracking put the corner: no slow drift while the look holds.
            let mut q = (q.0 as f64, q.1 as f64);
            if !self.lone {
                let guess_ref = ((q.0 - r.0) as f32, (q.1 - r.1) as f32);
                if let Some(qr) = lk(&self.reference, &next, (r.0 as f32, r.1 as f32), guess_ref, WIN) {
                    let qr = (qr.0 as f64, qr.1 as f64);
                    if (qr.0 - q.0).powi(2) + (qr.1 - q.1).powi(2) < 1.5 * 1.5 && ncc(self.reference.base(), (r.0 as f32, r.1 as f32), next.base(), (qr.0 as f32, qr.1 as f32), WIN) > 0.8 {
                        q = qr;
                    }
                }
            }
            moved.push((r, p, q));
        }
        let base = next.base();
        let px_tol = (base.width.max(base.height) as f64 / 400.0).clamp(1.0, 4.0);
        let motion = self.settings.motion;
        let src: Vec<(f64, f64)> = moved.iter().map(|m| m.0).collect();
        let dst: Vec<(f64, f64)> = moved.iter().map(|m| m.2).collect();
        let fitted = if self.lone {
            fit(Motion::Translation, &src, &dst).map(|h| (h, vec![true; src.len()]))
        } else if src.len() >= motion.minimum() * 2 {
            ransac(motion, &src, &dst, px_tol)
        } else {
            // Too few corners for the full motion: this frame's change as a
            // simpler motion, on top of where the region was.
            let mut m = motion;
            while m != Motion::Translation && src.len() < m.minimum() * 2 {
                m = if m == Motion::Perspective { Motion::Similarity } else { Motion::Translation };
            }
            let last: Vec<(f64, f64)> = moved.iter().map(|m| m.1).collect();
            ransac(m, &last, &dst, px_tol).map(|(d, inl)| (mul(&d, &self.h), inl))
        };
        let need = if self.lone { 1 } else { motion.minimum().max(3).min(tracked.max(1)) };
        let mut conf = 0.0f32;
        let mut lost = true;
        if let Some((h, inl)) = fitted {
            let n_in = inl.iter().filter(|&&b| b).count();
            if n_in >= need.min(src.len()).max(1) {
                let ratio = n_in as f32 / tracked.max(1) as f32;
                let enough = (n_in as f32 / (need as f32 * 3.0)).min(1.0);
                conf = (ratio * 0.6 + 0.4) * enough;
                if self.lone {
                    let p = self.region.points()[0];
                    let q = apply(&h, p);
                    conf *= ncc(self.prev.base(), (self.feats[0].1 .0 as f32, self.feats[0].1 .1 as f32), base, (q.0 as f32, q.1 as f32), 15).max(0.0);
                }
                if conf >= self.settings.min_confidence && plausible(&self.h, &h) {
                    lost = false;
                    self.velocity = invert(&self.h).map(|inv| mul(&h, &inv)).unwrap_or(IDENTITY);
                    self.h = h;
                    self.feats = moved.iter().zip(&inl).filter(|&(_, &k)| k).map(|(&(r, _, q), _)| (r, q)).collect();
                    if self.lone {
                        let p = self.region.points()[0];
                        self.feats = vec![(p, apply(&h, p))];
                    }
                }
            }
        }
        if lost {
            self.lost_for += 1;
            if let Some(h) = self.reacquire(&next) {
                self.h = h;
                self.velocity = IDENTITY;
                self.feats.clear();
                lost = false;
                conf = self.settings.min_confidence.max(0.3);
                self.lost_for = 0;
            } else {
                // Keep the corners that still sit where the region was.
                self.feats.retain(|f| f.1 .0 >= 0.0);
            }
        } else {
            self.lost_for = 0;
        }
        self.prev = next;
        if !lost && !self.lone {
            self.replenish();
        }
        if self.lone && lost {
            let p = self.region.points()[0];
            self.feats = vec![(p, apply(&self.h, p))];
        }
        Sample { frame, h: self.h, confidence: conf.clamp(0.0, 1.0), lost }
    }

    /// Looks for the reference picture of the region near where it was
    /// last seen (after an occlusion or a blur), by correlation on a
    /// small pyramid level.
    fn reacquire(&self, next: &Pyramid) -> Option<Mat3> {
        let lvl = (self.reference.levels.len() - 1).min(2).min(next.levels.len() - 1);
        let s = (1 << lvl) as f64;
        let (ra, na) = (&self.reference.levels[lvl], &next.levels[lvl]);
        let (x0, y0, x1, y1) = self.region.bounds(&IDENTITY);
        let (cx, cy) = (((x0 + x1) / 2.0) / s, ((y0 + y1) / 2.0) / s);
        let half = (((x1 - x0).max(y1 - y0) / 2.0) / s).clamp(4.0, 24.0) as isize;
        let last = apply(&self.h, ((x0 + x1) / 2.0, (y0 + y1) / 2.0));
        let (lx, ly) = (last.0 / s, last.1 / s);
        let reach = (half * 3).max(12) as f64;
        let mut best = (0.0f32, 0.0, 0.0);
        let step = 1.0;
        let mut y = ly - reach;
        while y <= ly + reach {
            let mut x = lx - reach;
            while x <= lx + reach {
                let c = ncc(ra, (cx as f32, cy as f32), na, (x as f32, y as f32), half);
                if c > best.0 {
                    best = (c, x, y);
                }
                x += step;
            }
            y += step;
        }
        if best.0 < 0.75 {
            return None;
        }
        // The last known shape, moved to where the region was found.
        let found = (best.1 * s, best.2 * s);
        let d = (found.0 - last.0, found.1 - last.1);
        Some(mul(&[1.0, 0.0, d.0, 0.0, 1.0, d.1, 0.0, 0.0, 1.0], &self.h))
    }
}

/// Rejects wild jumps: a region does not double in size or fold over in
/// one frame.
fn plausible(prev: &Mat3, h: &Mat3) -> bool {
    let (a, b) = (scale_of(prev), scale_of(h));
    let r = b / a;
    let det = h[0] * h[4] - h[1] * h[3];
    (0.6..1.6).contains(&r) && det > 0.0 && h.iter().all(|v| v.is_finite())
}

/// Tracks `region` (drawn on frame `start`) through frames `range` of
/// `frame`, forwards to the end and backwards to the beginning. `sink`
/// gets each frame's sample in the order they are worked out.
pub fn track(frame: &mut dyn FnMut(usize) -> Result<Gray>, range: std::ops::Range<usize>, start: usize, region: Region, settings: TrackSettings, backwards: bool, ctx: &JobCtx, sink: &mut dyn FnMut(Sample) -> Result<()>) -> Result<()> {
    if !range.contains(&start) {
        return invalid(format!("start frame {start} is outside {}..{}", range.start, range.end));
    }
    let total = range.len().max(1);
    let levels = 4;
    let first_gray = frame(start)?;
    let first = Pyramid::new(first_gray.clone(), levels);
    sink(Sample { frame: start, h: IDENTITY, confidence: 1.0, lost: false })?;
    let mut done = 1;
    let dirs: &[isize] = if backwards { &[1, -1] } else { &[1] };
    for &dir in dirs {
        let mut t = RegionTracker::new(first.clone(), region.clone(), settings)?;
        let mut prev = first_gray.clone();
        let mut cut = false;
        let mut i = start as isize + dir;
        while i >= range.start as isize && i < range.end as isize {
            ctx.check()?;
            ctx.set_progress(done as f32 / total as f32);
            if cut {
                // Past a cut in the footage the region is not in the shot.
                sink(Sample { frame: i as usize, h: t.current(), confidence: 0.0, lost: true })?;
            } else {
                let g = frame(i as usize)?;
                if is_cut(&prev, &g) {
                    cut = true;
                    sink(Sample { frame: i as usize, h: t.current(), confidence: 0.0, lost: true })?;
                } else {
                    prev = g.clone();
                    let s = t.step(Pyramid::new(g, levels), i as usize);
                    sink(s)?;
                }
            }
            done += 1;
            i += dir;
        }
    }
    Ok(())
}

/// A small, blurred copy of a picture for comparing shots.
fn thumb(g: &Gray) -> Vec<f32> {
    let (w, h) = (32usize, 18usize);
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x * g.width / w, ((x + 1) * g.width / w).max(x * g.width / w + 1));
            let (y0, y1) = (y * g.height / h, ((y + 1) * g.height / h).max(y * g.height / h + 1));
            let mut s = 0.0;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    s += g.data[yy * g.width + xx];
                }
            }
            out[y * w + x] = s / ((x1 - x0) * (y1 - y0)) as f32;
        }
    }
    out
}

/// Whether `b` is a different shot from `a` (a hard cut): both the layout
/// and the brightness spread change at once, which a pan or a flash alone
/// does not do.
pub fn is_cut(a: &Gray, b: &Gray) -> bool {
    let (ta, tb) = (thumb(a), thumb(b));
    let n = ta.len() as f32;
    let mad = ta.iter().zip(&tb).map(|(x, y)| (x - y).abs()).sum::<f32>() / n;
    let hist = |t: &[f32]| {
        let mut h = [0f32; 16];
        for v in t {
            h[((v.clamp(0.0, 0.999)) * 16.0) as usize] += 1.0 / n;
        }
        h
    };
    let (ha, hb) = (hist(&ta), hist(&tb));
    let dist = ha.iter().zip(&hb).map(|(x, y)| (x - y).abs()).sum::<f32>() / 2.0;
    mad > 0.12 && dist > 0.35
}

/// The picture's motion from each frame to the next (`out[i]` takes frame
/// `i` to frame `i + 1`) as a similarity, with how much of the picture
/// agreed (0..1). Moving subjects are outvoted by the background.
pub fn camera_motion(frame: &mut dyn FnMut(usize) -> Result<Gray>, range: std::ops::Range<usize>, ctx: &JobCtx) -> Result<Vec<(Mat3, f32)>> {
    if range.len() < 2 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(range.len() - 1);
    let mut prev = Pyramid::new(frame(range.start)?, 4);
    let total = range.len();
    for i in range.start + 1..range.end {
        ctx.check()?;
        ctx.set_progress((i - range.start) as f32 / total as f32);
        let next = Pyramid::new(frame(i)?, 4);
        // A cut is not camera motion: the path carries straight on.
        if is_cut(prev.base(), next.base()) {
            out.push((IDENTITY, 0.0));
        } else {
            out.push(frame_motion(&prev, &next));
        }
        prev = next;
    }
    Ok(out)
}

/// One frame-to-frame similarity and its agreement.
pub fn frame_motion(a: &Pyramid, b: &Pyramid) -> (Mat3, f32) {
    let g = a.base();
    let diag = ((g.width * g.width + g.height * g.height) as f32).sqrt();
    let pts = corners(g, 300, diag / 28.0, &|_, _| true);
    let mut src = Vec::new();
    let mut dst = Vec::new();
    for p in &pts {
        let Some(q) = lk(a, b, *p, (0.0, 0.0), WIN) else { continue };
        let Some(back) = lk(b, a, q, (p.0 - q.0, p.1 - q.1), WIN) else { continue };
        if (back.0 - p.0).powi(2) + (back.1 - p.1).powi(2) > 1.0 {
            continue;
        }
        src.push((p.0 as f64, p.1 as f64));
        dst.push((q.0 as f64, q.1 as f64));
    }
    let tol = (g.width.max(g.height) as f64 / 500.0).clamp(0.75, 3.0);
    match ransac(Motion::Similarity, &src, &dst, tol) {
        Some((h, inl)) if src.len() >= 6 => (h, inl.iter().filter(|&&b| b).count() as f32 / pts.len().max(1) as f32),
        _ => (IDENTITY, 0.0),
    }
}

/// A similarity's parts: shift (x, y), turn (radians) and log scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    pub log_scale: f64,
}

impl Pose {
    /// The pose of a similarity about the picture centre `c`.
    pub fn of(h: &Mat3, c: (f64, f64)) -> Pose {
        let (a, b) = (h[0], h[3]);
        let s = (a * a + b * b).sqrt().max(1e-9);
        let q = apply(h, c);
        Pose { x: q.0 - c.0, y: q.1 - c.1, angle: b.atan2(a), log_scale: s.ln() }
    }

    /// The similarity about centre `c`.
    pub fn matrix(&self, c: (f64, f64)) -> Mat3 {
        let s = self.log_scale.exp();
        let (sn, cs) = self.angle.sin_cos();
        let (a, b) = (s * cs, s * sn);
        // q = R·s·(p − c) + c + t
        [a, -b, c.0 + self.x - (a * c.0 - b * c.1), b, a, c.1 + self.y - (b * c.0 + a * c.1), 0.0, 0.0, 1.0]
    }
}

/// How a shot is stabilized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StabilizeSettings {
    /// Lock the camera completely (a tripod shot) instead of smoothing.
    pub lock: bool,
    /// How long a sway is smoothed away, in seconds (smooth mode).
    pub smoothness: f64,
    /// Correct the turn as well as the shift.
    pub rotation: bool,
    /// Correct zooming in and out.
    pub scale: bool,
    /// Largest zoom allowed to hide the edges (1.0 = no zoom, edges may
    /// show); past it the correction is eased back.
    pub max_zoom: f64,
}

impl Default for StabilizeSettings {
    fn default() -> Self {
        StabilizeSettings { lock: false, smoothness: 1.0, rotation: true, scale: false, max_zoom: 1.15 }
    }
}

/// A stabilization plan: per frame, the correction that moves the frame
/// onto the smooth path, and the one zoom (≥ 1) that keeps the canvas
/// covered.
#[derive(Clone, Debug, PartialEq)]
pub struct StabilizePlan {
    pub corrections: Vec<Pose>,
    pub zoom: f64,
    /// How much of the wanted correction survived the zoom limit (0..1).
    pub strength: f64,
}

/// Plans the correction for `motion` (frame-to-frame similarities, as
/// [`camera_motion`] gives them) of `w`×`h` pictures at `fps`.
///
/// Only shift, turn and uniform zoom are corrected, never shear or
/// perspective, and the turn is low-passed with the path: rolling-shutter
/// wobble is not "fixed" into a warped picture.
pub fn plan_stabilize(motion: &[Mat3], w: f64, h: f64, fps: f64, s: &StabilizeSettings) -> StabilizePlan {
    let c = (w / 2.0, h / 2.0);
    // The camera path: where each frame sits relative to the first.
    let mut path = Vec::with_capacity(motion.len() + 1);
    let mut acc = IDENTITY;
    path.push(Pose::default());
    for m in motion {
        acc = mul(m, &acc);
        path.push(Pose::of(&acc, c));
    }
    let n = path.len();
    let target: Vec<Pose> = if s.lock {
        vec![Pose::default(); n]
    } else {
        let sigma = (s.smoothness.max(0.05) * fps.max(1.0) / 2.0).max(1.0);
        gaussian(&path, sigma)
    };
    let correction = |k: f64| -> Vec<Pose> {
        (0..n)
            .map(|i| {
                let (p, t) = (path[i], target[i]);
                // Frame i maps reference → frame by P_i; the target T_i. The
                // correction takes the frame onto the target: T_i · P_i⁻¹.
                let pm = p.matrix(c);
                let tm = t.matrix(c);
                let corr = invert(&pm).map(|inv| mul(&tm, &inv)).unwrap_or(IDENTITY);
                let mut q = Pose::of(&corr, c);
                if !s.rotation {
                    q.angle = 0.0;
                }
                if !s.scale {
                    q.log_scale = 0.0;
                }
                Pose { x: q.x * k, y: q.y * k, angle: q.angle * k, log_scale: q.log_scale * k }
            })
            .collect()
    };
    let zoom_for = |cs: &[Pose]| cs.iter().map(|q| cover_zoom(q, w, h)).fold(1.0f64, f64::max);
    let mut k = 1.0;
    let mut cs = correction(1.0);
    let mut zoom = zoom_for(&cs);
    if zoom > s.max_zoom.max(1.0) + 1e-9 {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if zoom_for(&correction(mid)) <= s.max_zoom.max(1.0) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        k = lo;
        cs = correction(k);
        zoom = zoom_for(&cs).min(s.max_zoom.max(1.0));
    }
    if s.max_zoom <= 1.0 {
        zoom = 1.0;
    }
    StabilizePlan { corrections: cs, zoom, strength: k }
}

/// The zoom (about the centre, after the correction) that keeps a `w`×`h`
/// canvas covered by the corrected picture.
pub fn cover_zoom(q: &Pose, w: f64, h: f64) -> f64 {
    let c = (w / 2.0, h / 2.0);
    let m = q.matrix(c);
    let Some(inv) = invert(&m) else { return 1.0 };
    let (mut lo, mut hi) = (1.0f64, 4.0f64);
    let covered = |z: f64| {
        // Canvas corner → (unzoom) → (uncorrect) must land in the picture.
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().all(|&(x, y)| {
            let p = (c.0 + (x - c.0) / z, c.1 + (y - c.1) / z);
            let s = apply(&inv, p);
            s.0 >= -1e-6 && s.1 >= -1e-6 && s.0 <= w + 1e-6 && s.1 <= h + 1e-6
        })
    };
    if covered(1.0) {
        return 1.0;
    }
    if !covered(hi) {
        return hi;
    }
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if covered(mid) { hi = mid } else { lo = mid }
    }
    hi
}

/// Gaussian smoothing of a pose path (ends held by reflection).
fn gaussian(p: &[Pose], sigma: f64) -> Vec<Pose> {
    let n = p.len() as isize;
    let r = (sigma * 3.0).ceil() as isize;
    let weights: Vec<f64> = (-r..=r).map(|k| (-(k * k) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let reflect = |i: isize| -> usize {
        if n == 1 {
            return 0;
        }
        let mut i = i;
        while i < 0 || i >= n {
            if i < 0 {
                i = -i;
            }
            if i >= n {
                i = 2 * (n - 1) - i;
            }
        }
        i as usize
    };
    (0..n)
        .map(|i| {
            let mut acc = Pose::default();
            let mut wsum = 0.0;
            for (j, k) in (-r..=r).enumerate() {
                let q = p[reflect(i + k)];
                let wt = weights[j];
                acc.x += q.x * wt;
                acc.y += q.y * wt;
                acc.angle += q.angle * wt;
                acc.log_scale += q.log_scale * wt;
                wsum += wt;
            }
            Pose { x: acc.x / wsum, y: acc.y / wsum, angle: acc.angle / wsum, log_scale: acc.log_scale / wsum }
        })
        .collect()
}

/// Turns an [`Error`] from a frame callback into the tracker's own words.
pub fn frame_error(index: usize, e: impl std::fmt::Display) -> Error {
    Error::Io(format!("frame {index}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured picture (value noise) of size w×h, seen through `view`:
    /// output pixel p shows texture at view⁻¹(p).
    fn textured(w: usize, h: usize, view: &Mat3) -> Gray {
        let inv = invert(view).unwrap();
        let tex = |x: f64, y: f64| -> f32 {
            // Smooth blobs at several scales.
            let f = |x: f64, y: f64, s: f64| ((x / s).sin() * (y / s * 1.3).cos() + (x / s * 0.7 + y / s * 0.4).sin()) as f32;
            0.5 + 0.18 * f(x, y, 7.0) + 0.12 * f(x + 31.0, y - 17.0, 3.1) + 0.06 * f(x * 1.7, y * 0.9, 1.9)
        };
        let mut d = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let s = apply(&inv, (x as f64, y as f64));
                d[y * w + x] = tex(s.0, s.1).clamp(0.0, 1.0);
            }
        }
        Gray::new(w, h, d).unwrap()
    }

    fn shift(x: f64, y: f64) -> Mat3 {
        [1.0, 0.0, x, 0.0, 1.0, y, 0.0, 0.0, 1.0]
    }

    #[test]
    fn lk_follows_a_shift() {
        let a = Pyramid::new(textured(160, 120, &IDENTITY), 3);
        let b = Pyramid::new(textured(160, 120, &shift(3.4, -2.2)), 3);
        let q = lk(&a, &b, (80.0, 60.0), (0.0, 0.0), WIN).unwrap();
        assert!((q.0 - 83.4).abs() < 0.1 && (q.1 - 57.8).abs() < 0.1, "{q:?}");
    }

    #[test]
    fn homography_round_trips() {
        let h: Mat3 = [1.1, 0.05, 12.0, -0.03, 0.95, -4.0, 0.0004, -0.0002, 1.0];
        let src: Vec<(f64, f64)> = [(0.0, 0.0), (100.0, 0.0), (100.0, 80.0), (0.0, 80.0), (50.0, 30.0), (20.0, 70.0)].to_vec();
        let dst: Vec<(f64, f64)> = src.iter().map(|p| apply(&h, *p)).collect();
        let g = fit(Motion::Perspective, &src, &dst).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            let q = apply(&g, *s);
            assert!((q.0 - d.0).abs() < 1e-6 && (q.1 - d.1).abs() < 1e-6);
        }
    }

    #[test]
    fn ransac_ignores_outliers() {
        let h = Pose { x: 5.0, y: -3.0, angle: 0.1, log_scale: 0.05 }.matrix((50.0, 50.0));
        let mut src = Vec::new();
        let mut dst = Vec::new();
        for i in 0..40 {
            let p = ((i * 7 % 100) as f64, (i * 13 % 100) as f64);
            src.push(p);
            dst.push(if i % 5 == 0 { (p.0 + 30.0, p.1 - 20.0) } else { apply(&h, p) });
        }
        let (g, inl) = ransac(Motion::Similarity, &src, &dst, 0.5).unwrap();
        assert_eq!(inl.iter().filter(|&&b| b).count(), 32);
        let q = apply(&g, (10.0, 20.0));
        let r = apply(&h, (10.0, 20.0));
        assert!((q.0 - r.0).abs() < 1e-6 && (q.1 - r.1).abs() < 1e-6);
    }

    #[test]
    fn point_track_follows_moving_texture() {
        // The picture slides right and down 2 px a frame and turns slowly.
        let views: Vec<Mat3> = (0..20).map(|i| Pose { x: 2.0 * i as f64, y: 1.0 * i as f64, angle: 0.004 * i as f64, log_scale: 0.0 }.matrix((100.0, 75.0))).collect();
        let mut frames = |i: usize| Ok(textured(200, 150, &views[i]));
        let mut got = Vec::new();
        let region = Region::Point { x: 90.0, y: 70.0, radius: 20.0 };
        track(&mut frames, 0..20, 0, region.clone(), TrackSettings::default(), true, &JobCtx::detached(), &mut |s| {
            got.push(s);
            Ok(())
        })
        .unwrap();
        assert_eq!(got.len(), 20);
        for s in &got {
            let want = apply(&views[s.frame], (90.0, 70.0));
            let p = s.points(&region)[0];
            assert!(!s.lost && s.confidence > 0.5, "frame {} conf {}", s.frame, s.confidence);
            assert!((p.0 - want.0).abs() < 0.5 && (p.1 - want.1).abs() < 0.5, "frame {}: {p:?} vs {want:?}", s.frame);
        }
    }

    #[test]
    fn plane_track_follows_perspective() {
        let views: Vec<Mat3> = (0..15)
            .map(|i| {
                let t = i as f64;
                [1.0 + 0.01 * t, 0.004 * t, 1.5 * t, -0.003 * t, 1.0, 0.8 * t, 0.00004 * t, 0.0, 1.0]
            })
            .collect();
        let mut frames = |i: usize| Ok(textured(220, 160, &views[i]));
        let quad = [(60.0, 40.0), (150.0, 45.0), (155.0, 120.0), (55.0, 115.0)];
        let region = Region::Plane(quad);
        let mut got = Vec::new();
        let settings = TrackSettings { motion: Motion::Perspective, ..TrackSettings::default() };
        track(&mut frames, 0..15, 0, region.clone(), settings, false, &JobCtx::detached(), &mut |s| {
            got.push(s);
            Ok(())
        })
        .unwrap();
        let last = got.last().unwrap();
        for (p, q) in last.points(&region).iter().zip(quad) {
            let want = apply(&views[14], q);
            assert!((p.0 - want.0).abs() < 1.0 && (p.1 - want.1).abs() < 1.0, "{p:?} vs {want:?}");
        }
    }

    #[test]
    fn camera_motion_and_lock_cancel_shake() {
        // A shaky camera: the picture jitters by a few pixels.
        let jitter = |i: usize| ((i as f64 * 1.7).sin() * 3.0, (i as f64 * 2.3).cos() * 2.0);
        let views: Vec<Mat3> = (0..24).map(|i| shift(jitter(i).0, jitter(i).1)).collect();
        let mut frames = |i: usize| Ok(textured(200, 150, &views[i]));
        let motion = camera_motion(&mut frames, 0..24, &JobCtx::detached()).unwrap();
        assert_eq!(motion.len(), 23);
        assert!(motion.iter().all(|m| m.1 > 0.3));
        let plan = plan_stabilize(&motion.iter().map(|m| m.0).collect::<Vec<_>>(), 200.0, 150.0, 24.0, &StabilizeSettings { lock: true, ..Default::default() });
        // Applying each correction brings every frame back onto frame 0.
        for (i, q) in plan.corrections.iter().enumerate() {
            let (jx, jy) = (jitter(i).0 - jitter(0).0, jitter(i).1 - jitter(0).1);
            assert!((q.x + jx).abs() < 0.3 && (q.y + jy).abs() < 0.3, "frame {i}: {q:?} vs {jx},{jy}");
        }
        assert!(plan.zoom > 1.0 && plan.zoom < 1.1 && plan.strength == 1.0);
    }

    #[test]
    fn smoothing_keeps_a_pan_and_drops_jitter() {
        // A steady pan of 2 px a frame plus jitter.
        let n = 60;
        let pos = |i: usize| (2.0 * i as f64 + if i % 2 == 0 { 1.5 } else { -1.5 }, 0.0);
        let motion: Vec<Mat3> = (0..n - 1).map(|i| shift(pos(i + 1).0 - pos(i).0, 0.0)).collect();
        let plan = plan_stabilize(&motion, 320.0, 180.0, 30.0, &StabilizeSettings { smoothness: 0.5, max_zoom: 2.0, ..Default::default() });
        // Mid-shot, the corrected position moves evenly.
        let corrected: Vec<f64> = (0..n).map(|i| pos(i).0 - pos(0).0 + plan.corrections[i].x).collect();
        for i in 20..40 {
            let step = corrected[i + 1] - corrected[i];
            assert!((step - 2.0).abs() < 0.3, "step {i}: {step}");
        }
    }

    #[test]
    fn a_cut_ends_the_track() {
        let a = |i: usize| -> Gray { textured(160, 120, &shift(i as f64, 0.0)) };
        // Frames 0..6 one shot, then a different, darker one.
        let mut frames = |i: usize| -> Result<Gray> {
            if i < 6 {
                Ok(a(i))
            } else {
                let mut g = textured(160, 120, &Pose { x: 40.0, y: 30.0, angle: 1.2, log_scale: 0.4 }.matrix((80.0, 60.0)));
                g.data.iter_mut().for_each(|v| *v = (*v - 0.35).max(0.0) * 0.6);
                Ok(g)
            }
        };
        let mut got = Vec::new();
        track(&mut frames, 0..10, 2, Region::Point { x: 80.0, y: 60.0, radius: 20.0 }, TrackSettings::default(), true, &JobCtx::detached(), &mut |s| {
            got.push(s);
            Ok(())
        })
        .unwrap();
        got.sort_by_key(|s| s.frame);
        assert!(got[..6].iter().all(|s| !s.lost), "{:?}", got.iter().map(|s| s.lost).collect::<Vec<_>>());
        assert!(got[6..].iter().all(|s| s.lost && s.confidence == 0.0));
        assert!(!is_cut(&a(0), &a(3)), "a pan is not a cut");
    }

    #[test]
    fn zoom_limit_eases_the_correction() {
        let motion: Vec<Mat3> = (0..30).map(|i| shift(if i % 2 == 0 { 20.0 } else { -20.0 }, 0.0)).collect();
        let plan = plan_stabilize(&motion, 200.0, 100.0, 30.0, &StabilizeSettings { lock: true, max_zoom: 1.05, ..Default::default() });
        assert!(plan.strength < 1.0 && plan.zoom <= 1.05 + 1e-9);
    }
}
