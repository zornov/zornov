// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pose readers: numbers worked out from a pose, to drive corrective
//! shapes (the elbow's crease when the arm bends, the shoulder's bulge
//! when it lifts), drivers and swaps.
//!
//! - [`cone`]: 1 when a bone points a given way, fading to 0 at the edge
//!   of a cone around it.
//! - [`range`]: 0 to 1 as an angle (or any value) goes from one number to
//!   another.
//! - [`Rbf`]: radial basis interpolation between sculpted example poses
//!   (each example's own shape at its own pose, smooth in between).

use crate::v::{self, P};

/// How close a bone pointing `now` is to pointing `target`: 1 on it, 0 at
/// `cone` radians away or further, smooth between.
pub fn cone<const N: usize>(now: P<N>, target: P<N>, cone: f64) -> f64 {
    1.0 - v::smooth(0.0, cone.max(1e-9), v::angle(now, target))
}

/// 0 at `from`, 1 at `to`, smooth between (either order).
pub fn range(x: f64, from: f64, to: f64) -> f64 {
    if to >= from { v::smooth(from, to, x) } else { 1.0 - v::smooth(to, from, x) }
}

/// Radial basis interpolation through example poses.
#[derive(Clone, Debug, PartialEq)]
pub struct Rbf {
    centres: Vec<Vec<f64>>,
    weights: Vec<Vec<f64>>,
    sigma: f64,
}

impl Rbf {
    /// Fits `examples` (`(pose, outputs)`; every pose and every output as
    /// long as the first). `sigma` is the reach of each example (0: from
    /// the spacing of the examples).
    pub fn fit(examples: &[(Vec<f64>, Vec<f64>)], sigma: f64) -> Option<Rbf> {
        let n = examples.len();
        let dim = examples.first()?.0.len();
        let outs = examples[0].1.len();
        if examples.iter().any(|e| e.0.len() != dim || e.1.len() != outs) {
            return None;
        }
        let d = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>().sqrt();
        let sigma = if sigma > 0.0 {
            sigma
        } else {
            // The mean distance to the nearest other example.
            let near: Vec<f64> = (0..n).map(|i| (0..n).filter(|&j| j != i).map(|j| d(&examples[i].0, &examples[j].0)).fold(f64::INFINITY, f64::min)).filter(|x| x.is_finite()).collect();
            if near.is_empty() { 1.0 } else { (near.iter().sum::<f64>() / near.len() as f64).max(1e-6) }
        };
        let phi = |r: f64| (-(r / sigma).powi(2)).exp();
        // (Φ + λI) W = Y, solved by Gaussian elimination per output.
        let mut a: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| phi(d(&examples[i].0, &examples[j].0)) + if i == j { 1e-9 } else { 0.0 }).collect()).collect();
        let mut y: Vec<Vec<f64>> = examples.iter().map(|e| e.1.clone()).collect();
        for c in 0..n {
            let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
            if a[p][c].abs() < 1e-12 {
                return None;
            }
            a.swap(c, p);
            y.swap(c, p);
            for r in 0..n {
                if r != c {
                    let k = a[r][c] / a[c][c];
                    if k != 0.0 {
                        for x in c..n {
                            a[r][x] -= k * a[c][x];
                        }
                        for o in 0..outs {
                            y[r][o] -= k * y[c][o];
                        }
                    }
                }
            }
        }
        let weights = (0..n).map(|i| y[i].iter().map(|v| v / a[i][i]).collect()).collect();
        Some(Rbf { centres: examples.iter().map(|e| e.0.clone()).collect(), weights, sigma })
    }

    /// The outputs at pose `q`.
    pub fn eval(&self, q: &[f64]) -> Vec<f64> {
        let outs = self.weights.first().map_or(0, Vec::len);
        let mut out = vec![0.0; outs];
        for (c, w) in self.centres.iter().zip(&self.weights) {
            let r = c.iter().zip(q).map(|(x, y)| (x - y).powi(2)).sum::<f64>().sqrt();
            let k = (-(r / self.sigma).powi(2)).exp();
            for (o, wv) in out.iter_mut().zip(w) {
                *o += wv * k;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_fade_with_the_pose() {
        assert!((cone([0.0, 1.0], [0.0, 1.0], 1.0) - 1.0).abs() < 1e-12);
        assert_eq!(cone([1.0, 0.0], [0.0, 1.0], 1.0), 0.0);
        let half = cone([0.5f64.sin(), 0.5f64.cos()], [0.0, 1.0], 1.0);
        assert!((half - 0.5).abs() < 1e-9, "{half}");
        assert_eq!(range(90.0, 0.0, 90.0), 1.0);
        assert_eq!(range(0.0, 90.0, 0.0), 1.0);
        assert_eq!(range(45.0, 0.0, 90.0), 0.5);
    }

    #[test]
    fn rbf_goes_through_every_example() {
        let ex = vec![(vec![0.0, 0.0], vec![0.0]), (vec![90.0, 0.0], vec![1.0]), (vec![0.0, 90.0], vec![0.5])];
        let r = Rbf::fit(&ex, 0.0).unwrap();
        for (p, o) in &ex {
            assert!((r.eval(p)[0] - o[0]).abs() < 1e-6);
        }
        let mid = r.eval(&[45.0, 0.0])[0];
        assert!(mid > 0.0 && mid < 1.0);
        assert!(Rbf::fit(&[(vec![0.0], vec![1.0]), (vec![1.0, 2.0], vec![0.0])], 1.0).is_none());
    }
}
