// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cubic Hermite splines for curves.
//!
//! * [`Spline::monotone`] — Fritsch–Carlson monotone cubic. Input points are
//!   sorted and their y values forced non-decreasing, so the curve can never
//!   invert. Used for master / R / G / B curves.
//! * [`Spline::shape_preserving`] — the same tangent rule without forcing
//!   monotonicity: no overshoot between points, flat at local extrema. Used
//!   for luminance-vs-saturation and saturation-vs-saturation.
//! * [`Spline::periodic`] — shape-preserving on a circular axis 0..1 (hue),
//!   built by adding wrapped copies of the neighbouring points.
//!
//! All variants evaluate the same way, which keeps the WGSL evaluator
//! (one loop over at most [`MAX_KNOTS`] knots) identical to the CPU one.

/// Maximum user points per curve (the spec allows up to 16).
pub const MAX_POINTS: usize = 16;
/// Maximum stored knots, including the two wrapped knots of a periodic
/// spline.
pub const MAX_KNOTS: usize = MAX_POINTS + 2;

/// What happens outside the first and last knot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extrapolate {
    /// Continue the end tangent in a straight line.
    Linear,
    /// Hold the end value.
    Clamp,
    /// The x axis wraps at 1 (hue).
    Periodic,
}

/// A cubic Hermite spline: knots with positions, values and tangents.
#[derive(Clone, Debug, PartialEq)]
pub struct Spline {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    pub ms: Vec<f32>,
    pub extrapolate: Extrapolate,
}

fn clean(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> =
        points.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).take(MAX_POINTS).collect();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // Points closer than this in x would make a near-vertical segment.
    p.dedup_by(|b, a| (b[0] - a[0]).abs() < 1e-4);
    p
}

/// Fritsch–Carlson tangents for strictly increasing `xs`.
fn fc_tangents(xs: &[f32], ys: &[f32]) -> Vec<f32> {
    let n = xs.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
    let mut m = vec![0.0; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) * 0.5 };
    }
    // Limit tangents so each segment stays within its end values
    // (Fritsch & Carlson 1980, the α² + β² ≤ 9 circle).
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let a = m[i] / d[i];
        let b = m[i + 1] / d[i];
        if a < 0.0 {
            m[i] = 0.0;
        }
        if b < 0.0 {
            m[i + 1] = 0.0;
        }
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * d[i];
            m[i + 1] = t * b * d[i];
        }
    }
    m
}

impl Spline {
    /// A spline that is constant `y`.
    pub fn constant(y: f32) -> Spline {
        Spline { xs: vec![0.0], ys: vec![y], ms: vec![0.0], extrapolate: Extrapolate::Clamp }
    }

    /// The identity line through (0,0) and (1,1).
    pub fn identity() -> Spline {
        Spline::monotone(&[[0.0, 0.0], [1.0, 1.0]])
    }

    /// Monotone non-decreasing curve, extrapolated linearly with its end
    /// slopes (so highlights above 1 keep flowing through a curve).
    pub fn monotone(points: &[[f32; 2]]) -> Spline {
        let mut p = clean(points);
        if p.is_empty() {
            return Spline::identity();
        }
        let mut max = f32::MIN;
        for q in p.iter_mut() {
            max = max.max(q[1]);
            q[1] = max;
        }
        let xs: Vec<f32> = p.iter().map(|q| q[0]).collect();
        let ys: Vec<f32> = p.iter().map(|q| q[1]).collect();
        let ms = fc_tangents(&xs, &ys);
        Spline { xs, ys, ms, extrapolate: Extrapolate::Linear }
    }

    /// Smooth curve without overshoot, held constant outside its points.
    pub fn shape_preserving(points: &[[f32; 2]]) -> Spline {
        let p = clean(points);
        match p.len() {
            0 => Spline::constant(0.0),
            1 => Spline::constant(p[0][1]),
            _ => {
                let xs: Vec<f32> = p.iter().map(|q| q[0]).collect();
                let ys: Vec<f32> = p.iter().map(|q| q[1]).collect();
                let ms = fc_tangents(&xs, &ys);
                Spline { xs, ys, ms, extrapolate: Extrapolate::Clamp }
            }
        }
    }

    /// Smooth curve on a circular 0..1 axis. Point x values are wrapped
    /// into 0..1 first.
    pub fn periodic(points: &[[f32; 2]]) -> Spline {
        let wrapped: Vec<[f32; 2]> = points.iter().map(|p| [p[0].rem_euclid(1.0), p[1]]).collect();
        let p = clean(&wrapped);
        match p.len() {
            0 => return Spline::constant(0.0),
            1 => return Spline::constant(p[0][1]),
            _ => {}
        }
        let first = p[0];
        let last = p[p.len() - 1];
        let mut ext = Vec::with_capacity(p.len() + 2);
        ext.push([last[0] - 1.0, last[1]]);
        ext.extend_from_slice(&p);
        ext.push([first[0] + 1.0, first[1]]);
        let xs: Vec<f32> = ext.iter().map(|q| q[0]).collect();
        let ys: Vec<f32> = ext.iter().map(|q| q[1]).collect();
        let mut ms = fc_tangents(&xs, &ys);
        // The ghost knots repeat real ones, so give them the real tangents
        // and the curve joins smoothly across the wrap.
        let n = ms.len();
        ms[0] = ms[n - 2];
        ms[n - 1] = ms[1];
        Spline { xs, ys, ms, extrapolate: Extrapolate::Periodic }
    }

    /// True if the spline is the identity line (used to skip work).
    pub fn is_identity(&self) -> bool {
        self.extrapolate == Extrapolate::Linear
            && self.xs.iter().zip(&self.ys).all(|(x, y)| x == y)
            && self.ms.iter().all(|&m| m == 1.0)
    }

    /// True if the spline is zero everywhere.
    pub fn is_zero(&self) -> bool {
        self.extrapolate != Extrapolate::Linear && self.ys.iter().all(|&y| y == 0.0)
    }

    pub fn eval(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if n == 1 {
            return self.ys[0];
        }
        let x = if self.extrapolate == Extrapolate::Periodic { x.rem_euclid(1.0) } else { x };
        if x <= self.xs[0] {
            return match self.extrapolate {
                Extrapolate::Linear => self.ys[0] + (x - self.xs[0]) * self.ms[0],
                _ => self.ys[0],
            };
        }
        if x >= self.xs[n - 1] {
            return match self.extrapolate {
                Extrapolate::Linear => self.ys[n - 1] + (x - self.xs[n - 1]) * self.ms[n - 1],
                _ => self.ys[n - 1],
            };
        }
        // At most 18 knots: a linear scan is as fast as a binary search.
        let mut i = 0;
        while i + 2 < n && x >= self.xs[i + 1] {
            i += 1;
        }
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * self.ys[i] + h10 * h * self.ms[i] + h01 * self.ys[i + 1] + h11 * h * self.ms[i + 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Lcg;

    #[test]
    fn identity_line() {
        let s = Spline::identity();
        assert!(s.is_identity());
        for i in -10..=30 {
            let x = i as f32 / 10.0;
            assert!((s.eval(x) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn passes_through_points() {
        let pts = [[0.0, 0.0], [0.3, 0.2], [0.6, 0.7], [1.0, 1.0]];
        let s = Spline::monotone(&pts);
        for p in pts {
            assert!((s.eval(p[0]) - p[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn monotone_curves_never_invert() {
        let mut rng = Lcg(42);
        for _ in 0..500 {
            let n = 2 + (rng.next() * 15.0) as usize;
            // Deliberately non-monotone input: the spline must fix it up.
            let pts: Vec<[f32; 2]> = (0..n).map(|_| [rng.next(), rng.next()]).collect();
            let s = Spline::monotone(&pts);
            let mut prev = f32::MIN;
            for i in 0..=4000 {
                let x = -0.5 + i as f32 / 2000.0;
                let y = s.eval(x);
                assert!(y >= prev - 1e-6, "inverted at {x}: {y} < {prev} for {pts:?}");
                prev = y;
            }
        }
    }

    #[test]
    fn shape_preserving_has_no_overshoot() {
        let s = Spline::shape_preserving(&[[0.0, 0.0], [0.4, 1.0], [0.5, 1.0], [1.0, 0.0]]);
        for i in 0..=1000 {
            let y = s.eval(i as f32 / 1000.0);
            assert!((-1e-6..=1.0 + 1e-6).contains(&y));
        }
    }

    #[test]
    fn periodic_wraps_smoothly() {
        let s = Spline::periodic(&[[0.1, 0.5], [0.4, -0.2], [0.8, 0.3]]);
        assert!((s.eval(0.0) - s.eval(1.0)).abs() < 1e-6);
        assert!((s.eval(-0.05) - s.eval(0.95)).abs() < 1e-6);
        // Continuous slope across the wrap.
        let e = 1e-3;
        let left = (s.eval(1.0) - s.eval(1.0 - e)) / e;
        let right = (s.eval(e) - s.eval(0.0)) / e;
        assert!((left - right).abs() < 0.05, "{left} {right}");
        assert!((s.eval(0.4) + 0.2).abs() < 1e-6);
    }
}
