// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Star-shaped outlines described in polar form: for a direction `a`
//! (radians, 0 = right, y pointing down) the distance from the centre to
//! the outline of a unit-sized shape. Used by iris and shape-reveal
//! transitions and the mask reveals of motion presets.

use std::f32::consts::PI;
use std::sync::OnceLock;

use crate::math::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Circle,
    Square,
    Diamond,
    Hexagon,
    Star,
    Heart,
    Triangle,
}

impl Shape {
    /// Distance to the outline in direction `a`; the outer radius is 1.
    pub fn radius(self, a: f32) -> f32 {
        let polygon = |n: f32, rot: f32| {
            let seg = TAU / n;
            let local = (a - rot).rem_euclid(seg) - seg * 0.5;
            (PI / n).cos() / local.cos()
        };
        match self {
            Shape::Circle => 1.0,
            Shape::Square => polygon(4.0, PI / 4.0),
            Shape::Diamond => polygon(4.0, 0.0),
            Shape::Hexagon => polygon(6.0, 0.0),
            Shape::Triangle => polygon(3.0, -PI / 2.0),
            Shape::Star => {
                // Five points, the first straight up; straight edges between
                // outer and inner vertices.
                let seg = TAU / 5.0;
                let local = (a + PI / 2.0).rem_euclid(seg);
                let half = seg * 0.5;
                let (outer, inner) = (1.0f32, 0.42f32);
                // Edge from the outer vertex (angle 0) to the inner (angle half).
                let t = if local <= half { local } else { seg - local };
                let (p0, p1) = ([outer, 0.0], [inner * half.cos(), inner * half.sin()]);
                ray_segment(t, p0, p1)
            }
            Shape::Heart => heart_radius(a),
        }
    }

    /// Smallest radius over all directions, so `1 / inradius` is how much a
    /// shape must grow before it covers a circle of radius one.
    pub fn inradius(self) -> f32 {
        match self {
            Shape::Circle => 1.0,
            Shape::Square | Shape::Diamond => (PI / 4.0).cos(),
            Shape::Hexagon => (PI / 6.0).cos(),
            Shape::Triangle => 0.5,
            _ => (0..720).map(|i| self.radius(i as f32 / 720.0 * TAU)).fold(f32::INFINITY, f32::min),
        }
    }

    /// Normalised distance of a point from the centre: below 1 inside.
    pub fn distance(self, x: f32, y: f32) -> f32 {
        let r = (x * x + y * y).sqrt();
        if r < 1e-9 {
            return 0.0;
        }
        r / self.radius(y.atan2(x)).max(1e-6)
    }
}

/// Distance along a ray at angle `t` from the origin to the segment p0-p1.
fn ray_segment(t: f32, p0: [f32; 2], p1: [f32; 2]) -> f32 {
    let (dx, dy) = (t.cos(), t.sin());
    let (ex, ey) = (p1[0] - p0[0], p1[1] - p0[1]);
    // Solve s*d = p0 + u*e for s.
    let den = dx * ey - dy * ex;
    if den.abs() < 1e-9 {
        return 1.0;
    }
    (p0[0] * ey - p0[1] * ex) / den
}

/// The heart outline, tabulated once: the classic parametric heart curve
/// is sampled densely and the farthest point in each direction kept.
fn heart_radius(a: f32) -> f32 {
    const BINS: usize = 720;
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = vec![0.0f32; BINS];
        let n = 20000;
        // Centre chosen so the heart sits balanced in its circle.
        let cy = 0.1;
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            let s = i as f32 / n as f32 * TAU;
            let x = 16.0 * s.sin().powi(3) / 17.0;
            let y = -(13.0 * s.cos() - 5.0 * (2.0 * s).cos() - 2.0 * (3.0 * s).cos() - (4.0 * s).cos()) / 17.0;
            pts.push((x, y - cy));
        }
        let max_r = pts.iter().map(|&(x, y)| (x * x + y * y).sqrt()).fold(0.0f32, f32::max);
        for &(x, y) in &pts {
            let b = ((y.atan2(x).rem_euclid(TAU)) / TAU * BINS as f32) as usize % BINS;
            t[b] = t[b].max((x * x + y * y).sqrt() / max_r);
        }
        // Fill any empty bins from their neighbours.
        for _ in 0..4 {
            for i in 0..BINS {
                if t[i] == 0.0 {
                    t[i] = t[(i + BINS - 1) % BINS].max(t[(i + 1) % BINS]);
                }
            }
        }
        t
    });
    let f = a.rem_euclid(TAU) / TAU * BINS as f32;
    let i = f as usize % BINS;
    let j = (i + 1) % BINS;
    let k = f - f.floor();
    table[i] * (1.0 - k) + table[j] * k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radii_are_sane() {
        for s in [Shape::Circle, Shape::Square, Shape::Diamond, Shape::Hexagon, Shape::Star, Shape::Heart, Shape::Triangle] {
            for i in 0..360 {
                let r = s.radius(i as f32 / 360.0 * TAU);
                assert!(r > 0.1 && r <= 1.0001, "{s:?} {i} {r}");
            }
            assert!(s.inradius() > 0.1);
        }
        // Square corners at the diagonals, sides at 1/sqrt 2.
        assert!((Shape::Square.radius(0.0) - 0.7071).abs() < 1e-3);
        assert!((Shape::Square.radius(PI / 4.0) - 1.0).abs() < 1e-3);
        // Star point straight up (negative y).
        assert!((Shape::Star.radius(-PI / 2.0) - 1.0).abs() < 1e-3);
    }
}
