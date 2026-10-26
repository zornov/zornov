// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Building blocks for stabilisation and tracking: FAST corners, pyramidal
//! Lucas-Kanade (KLT) point tracking, and a RANSAC similarity fit.

use crate::math::Rng;
use crate::ops::Gray;

/// FAST-9 corner detector. `threshold` is the brightness difference (in
/// the 0..1 units of [`Gray`]) a ring pixel must exceed. Returns
/// `(x, y, score)` with non-maximum suppression applied; coordinates are
/// pixel centres (`+ 0.5`).
pub fn fast_corners(img: &Gray, threshold: f32) -> Vec<(f32, f32, f32)> {
    const RING: [(isize, isize); 16] = [
        (0, -3), (1, -3), (2, -2), (3, -1), (3, 0), (3, 1), (2, 2), (1, 3),
        (0, 3), (-1, 3), (-2, 2), (-3, 1), (-3, 0), (-3, -1), (-2, -2), (-1, -3),
    ];
    let (w, h) = (img.width, img.height);
    if w < 8 || h < 8 {
        return Vec::new();
    }
    let mut score = vec![0.0f32; w * h];
    for y in 3..h - 3 {
        for x in 3..w - 3 {
            let c = img.get(x, y);
            let ring: [f32; 16] = std::array::from_fn(|i| img.get((x as isize + RING[i].0) as usize, (y as isize + RING[i].1) as usize));
            // Quick reject: of the four compass points at least three must
            // be clearly brighter or clearly darker for a 9-arc to exist.
            let bright = [0, 4, 8, 12].iter().filter(|&&i| ring[i] > c + threshold).count();
            let dark = [0, 4, 8, 12].iter().filter(|&&i| ring[i] < c - threshold).count();
            if bright < 2 && dark < 2 {
                continue;
            }
            let arc = |pred: &dyn Fn(f32) -> bool| {
                let mut run = 0;
                for i in 0..32 {
                    if pred(ring[i % 16]) {
                        run += 1;
                        if run >= 9 {
                            return true;
                        }
                    } else {
                        run = 0;
                    }
                }
                false
            };
            if arc(&|v| v > c + threshold) || arc(&|v| v < c - threshold) {
                score[y * w + x] = ring.iter().map(|&v| ((v - c).abs() - threshold).max(0.0)).sum();
            }
        }
    }
    let mut out = Vec::new();
    for y in 3..h - 3 {
        for x in 3..w - 3 {
            let s = score[y * w + x];
            if s <= 0.0 {
                continue;
            }
            let mut is_max = true;
            'n: for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if (dx, dy) != (0, 0) && score[(y as isize + dy) as usize * w + (x as isize + dx) as usize] > s {
                        is_max = false;
                        break 'n;
                    }
                }
            }
            if is_max {
                out.push((x as f32 + 0.5, y as f32 + 0.5, s));
            }
        }
    }
    out
}

/// Picks up to `max` corners spread over the frame: the strongest in each
/// cell of a grid first, then the next strongest, and so on.
pub fn spread_corners(mut corners: Vec<(f32, f32, f32)>, w: usize, h: usize, max: usize) -> Vec<[f32; 2]> {
    corners.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let grid = ((max as f32).sqrt().ceil() as usize).max(1);
    let cell = |x: f32, y: f32| {
        let cx = ((x / w as f32) * grid as f32) as usize;
        let cy = ((y / h as f32) * grid as f32) as usize;
        cy.min(grid - 1) * grid + cx.min(grid - 1)
    };
    let mut taken = vec![0usize; grid * grid];
    let mut out = Vec::new();
    for round in 1.. {
        let before = out.len();
        for &(x, y, _) in &corners {
            if out.len() >= max {
                return out;
            }
            let c = cell(x, y);
            if taken[c] < round && !out.contains(&[x, y]) {
                taken[c] += 1;
                out.push([x, y]);
            }
        }
        if out.len() == before {
            break;
        }
    }
    out
}

/// An image pyramid with gradients, ready for KLT.
#[derive(Clone, Debug)]
pub struct Pyramid {
    pub levels: Vec<Gray>,
    gx: Vec<Gray>,
    gy: Vec<Gray>,
}

impl Pyramid {
    /// Builds `levels` levels (at least one), each half the previous size.
    pub fn new(img: &Gray, levels: usize) -> Self {
        let mut lv = vec![img.blurred(0.7)];
        while lv.len() < levels.max(1) && lv.last().unwrap().width >= 16 && lv.last().unwrap().height >= 16 {
            let next = lv.last().unwrap().half();
            lv.push(next);
        }
        let grad = |g: &Gray, dx: isize, dy: isize| {
            let mut out = Gray::new(g.width, g.height);
            for y in 0..g.height {
                for x in 0..g.width {
                    let (xi, yi) = (x as isize, y as isize);
                    out.data[y * g.width + x] = 0.5 * (g.at(xi + dx, yi + dy) - g.at(xi - dx, yi - dy));
                }
            }
            out
        };
        let gx = lv.iter().map(|g| grad(g, 1, 0)).collect();
        let gy = lv.iter().map(|g| grad(g, 0, 1)).collect();
        Pyramid { levels: lv, gx, gy }
    }
}

/// Result of tracking one point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracked {
    pub position: [f32; 2],
    /// Mean absolute brightness difference over the window at the end.
    pub residual: f32,
}

/// Pyramidal Lucas-Kanade: finds where the window around `p` in `prev`
/// moved to in `next`. `half` is the window half-size in pixels. Returns
/// `None` when the window has too little texture or the solution leaves
/// the image.
pub fn klt(prev: &Pyramid, next: &Pyramid, p: [f32; 2], guess: [f32; 2], half: usize) -> Option<Tracked> {
    let levels = prev.levels.len().min(next.levels.len());
    let half = half as isize;
    let mut g = [guess[0] / (1 << (levels - 1)) as f32, guess[1] / (1 << (levels - 1)) as f32];
    let mut residual = 0.0;
    for l in (0..levels).rev() {
        let scale = (1 << l) as f32;
        let (px, py) = (p[0] / scale, p[1] / scale);
        let (i, ix, iy, j) = (&prev.levels[l], &prev.gx[l], &prev.gy[l], &next.levels[l]);
        // Structure tensor of the window in the previous frame.
        let (mut gxx, mut gxy, mut gyy) = (0.0f32, 0.0f32, 0.0f32);
        let n = ((2 * half + 1) * (2 * half + 1)) as f32;
        let mut patch = Vec::with_capacity(n as usize);
        for dy in -half..=half {
            for dx in -half..=half {
                let (x, y) = (px + dx as f32, py + dy as f32);
                let (a, b) = (ix.sample(x, y), iy.sample(x, y));
                gxx += a * a;
                gxy += a * b;
                gyy += b * b;
                patch.push((i.sample(x, y), a, b, x, y));
            }
        }
        let det = gxx * gyy - gxy * gxy;
        // Smallest eigenvalue per pixel: too small means a flat or
        // edge-only window that cannot be located in two directions.
        let min_eig = 0.5 * ((gxx + gyy) - ((gxx - gyy).powi(2) + 4.0 * gxy * gxy).sqrt()) / n;
        if det.abs() < 1e-12 || min_eig < 1e-5 {
            return None;
        }
        let mut d = [0.0f32; 2];
        for _ in 0..20 {
            let (mut bx, mut by, mut err) = (0.0f32, 0.0f32, 0.0f32);
            for &(iv, a, b, x, y) in &patch {
                let diff = iv - j.sample(x + g[0] + d[0], y + g[1] + d[1]);
                bx += diff * a;
                by += diff * b;
                err += diff.abs();
            }
            residual = err / n;
            let step = [(gyy * bx - gxy * by) / det, (gxx * by - gxy * bx) / det];
            d[0] += step[0];
            d[1] += step[1];
            if step[0].abs() + step[1].abs() < 0.01 {
                break;
            }
        }
        g = [g[0] + d[0], g[1] + d[1]];
        if l > 0 {
            g = [g[0] * 2.0, g[1] * 2.0];
        }
    }
    let q = [p[0] + g[0], p[1] + g[1]];
    let (w, h) = (prev.levels[0].width as f32, prev.levels[0].height as f32);
    if !q[0].is_finite() || !q[1].is_finite() || q[0] < 0.0 || q[1] < 0.0 || q[0] > w || q[1] > h {
        return None;
    }
    Some(Tracked { position: q, residual })
}

/// A 2-D similarity transform `q = [a -b; b a] p + t` (uniform scale,
/// rotation and translation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Similarity {
    pub const IDENTITY: Similarity = Similarity { a: 1.0, b: 0.0, tx: 0.0, ty: 0.0 };

    /// From scale, rotation (radians) and translation.
    pub fn from_parts(scale: f32, angle: f32, tx: f32, ty: f32) -> Self {
        Similarity { a: scale * angle.cos(), b: scale * angle.sin(), tx, ty }
    }

    pub fn scale(&self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    pub fn angle(&self) -> f32 {
        self.b.atan2(self.a)
    }

    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [self.a * p[0] - self.b * p[1] + self.tx, self.b * p[0] + self.a * p[1] + self.ty]
    }

    /// `self` after `first`: `(self ∘ first)(p) = self(first(p))`.
    pub fn compose(&self, first: &Similarity) -> Similarity {
        Similarity {
            a: self.a * first.a - self.b * first.b,
            b: self.b * first.a + self.a * first.b,
            tx: self.a * first.tx - self.b * first.ty + self.tx,
            ty: self.b * first.tx + self.a * first.ty + self.ty,
        }
    }

    pub fn inverse(&self) -> Similarity {
        let d = (self.a * self.a + self.b * self.b).max(1e-12);
        let (ia, ib) = (self.a / d, -self.b / d);
        Similarity { a: ia, b: ib, tx: -(ia * self.tx - ib * self.ty), ty: -(ib * self.tx + ia * self.ty) }
    }

    /// Least-squares fit to point pairs (`p` maps to `q`); needs two or
    /// more pairs that are not all the same point.
    pub fn fit(pairs: &[([f32; 2], [f32; 2])]) -> Option<Similarity> {
        if pairs.len() < 2 {
            return None;
        }
        let n = pairs.len() as f64;
        let (mut pmx, mut pmy, mut qmx, mut qmy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for (p, q) in pairs {
            pmx += p[0] as f64;
            pmy += p[1] as f64;
            qmx += q[0] as f64;
            qmy += q[1] as f64;
        }
        let (pmx, pmy, qmx, qmy) = (pmx / n, pmy / n, qmx / n, qmy / n);
        let (mut sa, mut sb, mut sp) = (0.0f64, 0.0f64, 0.0f64);
        for (p, q) in pairs {
            let (px, py) = (p[0] as f64 - pmx, p[1] as f64 - pmy);
            let (qx, qy) = (q[0] as f64 - qmx, q[1] as f64 - qmy);
            sa += px * qx + py * qy;
            sb += px * qy - py * qx;
            sp += px * px + py * py;
        }
        if sp < 1e-9 {
            return None;
        }
        let (a, b) = (sa / sp, sb / sp);
        Some(Similarity {
            a: a as f32,
            b: b as f32,
            tx: (qmx - (a * pmx - b * pmy)) as f32,
            ty: (qmy - (b * pmx + a * pmy)) as f32,
        })
    }
}

/// RANSAC similarity estimate: robust to points on moving objects.
/// `threshold` is the inlier distance in pixels. Returns the model refit
/// on all inliers and the inlier count.
pub fn ransac_similarity(pairs: &[([f32; 2], [f32; 2])], threshold: f32, iterations: usize, seed: u32) -> Option<(Similarity, usize)> {
    if pairs.len() < 2 {
        return None;
    }
    let mut rng = Rng::new(seed);
    let t2 = threshold * threshold;
    let count = |m: &Similarity| {
        pairs
            .iter()
            .filter(|(p, q)| {
                let r = m.apply(*p);
                (r[0] - q[0]).powi(2) + (r[1] - q[1]).powi(2) <= t2
            })
            .count()
    };
    let mut best: Option<(Similarity, usize)> = None;
    for _ in 0..iterations {
        let i = rng.below(pairs.len());
        let mut j = rng.below(pairs.len());
        if j == i {
            j = (i + 1) % pairs.len();
        }
        let Some(m) = Similarity::fit(&[pairs[i], pairs[j]]) else { continue };
        // Real camera motion between frames never scales wildly.
        if !(0.5..2.0).contains(&m.scale()) {
            continue;
        }
        let c = count(&m);
        if best.is_none_or(|(_, bc)| c > bc) {
            best = Some((m, c));
        }
    }
    let (m, _) = best?;
    let inliers: Vec<_> = pairs
        .iter()
        .copied()
        .filter(|(p, q)| {
            let r = m.apply(*p);
            (r[0] - q[0]).powi(2) + (r[1] - q[1]).powi(2) <= t2
        })
        .collect();
    let refined = Similarity::fit(&inliers).unwrap_or(m);
    Some((refined, inliers.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_algebra() {
        let s = Similarity::from_parts(1.2, 0.3, 5.0, -2.0);
        let p = [3.0, 4.0];
        let q = s.inverse().apply(s.apply(p));
        assert!((q[0] - p[0]).abs() < 1e-4 && (q[1] - p[1]).abs() < 1e-4);
        let t = Similarity::from_parts(0.9, -0.1, 1.0, 1.0);
        let c = t.compose(&s).apply(p);
        let d = t.apply(s.apply(p));
        assert!((c[0] - d[0]).abs() < 1e-4 && (c[1] - d[1]).abs() < 1e-4);
    }

    #[test]
    fn ransac_ignores_outliers() {
        let truth = Similarity::from_parts(1.01, 0.02, 3.0, -1.5);
        let mut rng = Rng::new(1);
        let mut pairs = Vec::new();
        for i in 0..100 {
            let p = [rng.next_f32() * 200.0, rng.next_f32() * 100.0];
            let q = if i % 4 == 0 { [rng.next_f32() * 200.0, rng.next_f32() * 100.0] } else { truth.apply(p) };
            pairs.push((p, q));
        }
        let (m, n) = ransac_similarity(&pairs, 1.0, 200, 5).unwrap();
        assert!(n >= 75);
        assert!((m.tx - truth.tx).abs() < 0.01 && (m.angle() - truth.angle()).abs() < 1e-4);
    }
}
