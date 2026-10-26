// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A smooth path through points (Catmull-Rom), walked by arc length, for
//! path-follow constraints: a bone (a car on a track, a bird on its
//! flight line, a spine along a curve) placed at a fraction of the way
//! along and turned to its direction.

use crate::v::{self, P};

/// A path through points.
#[derive(Clone, Debug, PartialEq)]
pub struct Path<const N: usize> {
    closed: bool,
    /// Samples along the curve with the distance to each.
    samples: Vec<(f64, P<N>)>,
}

const STEPS: usize = 24;

impl<const N: usize> Path<N> {
    /// The curve through `points` (at least two); `closed` joins the last
    /// to the first.
    pub fn new(points: &[P<N>], closed: bool) -> Option<Path<N>> {
        if points.len() < 2 {
            return None;
        }
        let n = points.len();
        let at = |i: isize| -> P<N> {
            if closed {
                points[i.rem_euclid(n as isize) as usize]
            } else {
                // Ends continue straight.
                if i < 0 {
                    v::sub(v::scale(points[0], 2.0), points[1])
                } else if i as usize >= n {
                    v::sub(v::scale(points[n - 1], 2.0), points[n - 2])
                } else {
                    points[i as usize]
                }
            }
        };
        let spans = if closed { n } else { n - 1 };
        let mut samples = vec![(0.0, points[0])];
        let mut total = 0.0;
        for s in 0..spans as isize {
            let (p0, p1, p2, p3) = (at(s - 1), at(s), at(s + 1), at(s + 2));
            for k in 1..=STEPS {
                let t = k as f64 / STEPS as f64;
                let (t2, t3) = (t * t, t * t * t);
                let p: P<N> = std::array::from_fn(|i| 0.5 * (2.0 * p1[i] + (-p0[i] + p2[i]) * t + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * t2 + (-p0[i] + 3.0 * p1[i] - 3.0 * p2[i] + p3[i]) * t3));
                total += v::dist(samples.last().map_or(p, |l| l.1), p);
                samples.push((total, p));
            }
        }
        Some(Path { closed, samples })
    }

    /// How long it is.
    pub fn length(&self) -> f64 {
        self.samples.last().map_or(0.0, |s| s.0)
    }

    fn locate(&self, t: f64) -> (usize, f64) {
        let t = if self.closed { t.rem_euclid(1.0) } else { t.clamp(0.0, 1.0) };
        let want = t * self.length();
        let i = self.samples.partition_point(|s| s.0 < want).clamp(1, self.samples.len() - 1);
        let (a, b) = (self.samples[i - 1], self.samples[i]);
        let k = if b.0 - a.0 > 1e-12 { (want - a.0) / (b.0 - a.0) } else { 0.0 };
        (i, k)
    }

    /// The point a fraction `t` (0..1) of the way along by length;
    /// a closed path wraps, an open one stops at its ends.
    pub fn at(&self, t: f64) -> P<N> {
        let (i, k) = self.locate(t);
        v::lerp(self.samples[i - 1].1, self.samples[i].1, k)
    }

    /// Which way the path runs at `t` (one long).
    pub fn direction(&self, t: f64) -> P<N> {
        let (i, _) = self.locate(t);
        v::norm(v::sub(self.samples[i].1, self.samples[i - 1].1))
    }

    /// The fraction along the path nearest `p`.
    pub fn nearest(&self, p: P<N>) -> f64 {
        let total = self.length().max(1e-12);
        let mut best = (f64::INFINITY, 0.0);
        for w in self.samples.windows(2) {
            let (a, b) = (w[0].1, w[1].1);
            let ab = v::sub(b, a);
            let l2 = v::dot(ab, ab);
            let k = if l2 > 1e-24 { (v::dot(v::sub(p, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let q = v::add(a, v::scale(ab, k));
            let d = v::dist(p, q);
            if d < best.0 {
                best = (d, (w[0].0 + (w[1].0 - w[0].0) * k) / total);
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_goes_through_its_points_and_is_walked_by_length() {
        let p = Path::new(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]], false).unwrap();
        assert!((p.length() - 2.0).abs() < 1e-9);
        assert!(v::dist(p.at(0.5), [1.0, 0.0]) < 1e-9);
        assert!(v::dist(p.at(2.0), [2.0, 0.0]) < 1e-9, "open paths stop at the end");
        assert!(v::dist(p.direction(0.3), [1.0, 0.0]) < 1e-9);
        assert!((p.nearest([1.5, 3.0]) - 0.75).abs() < 1e-6);
        // A closed square: wraps, goes through every corner.
        let sq = Path::new(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], true).unwrap();
        assert!(v::dist(sq.at(1.25), sq.at(0.25)) < 1e-9);
        assert!(sq.samples.iter().any(|s| v::dist(s.1, [1.0, 1.0, 0.0]) < 1e-9));
        assert!(Path::<2>::new(&[[0.0, 0.0]], false).is_none());
    }
}
