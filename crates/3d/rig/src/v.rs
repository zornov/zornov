// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Points and directions of any dimension.

/// A point or a direction in N dimensions.
pub type P<const N: usize> = [f64; N];

pub fn add<const N: usize>(a: P<N>, b: P<N>) -> P<N> {
    std::array::from_fn(|i| a[i] + b[i])
}

pub fn sub<const N: usize>(a: P<N>, b: P<N>) -> P<N> {
    std::array::from_fn(|i| a[i] - b[i])
}

pub fn scale<const N: usize>(a: P<N>, k: f64) -> P<N> {
    a.map(|x| x * k)
}

pub fn dot<const N: usize>(a: P<N>, b: P<N>) -> f64 {
    (0..N).map(|i| a[i] * b[i]).sum()
}

pub fn len<const N: usize>(a: P<N>) -> f64 {
    dot(a, a).sqrt()
}

pub fn dist<const N: usize>(a: P<N>, b: P<N>) -> f64 {
    len(sub(a, b))
}

/// `a` made one long; zero stays zero.
pub fn norm<const N: usize>(a: P<N>) -> P<N> {
    let l = len(a);
    if l < 1e-12 { [0.0; N] } else { scale(a, 1.0 / l) }
}

pub fn lerp<const N: usize>(a: P<N>, b: P<N>, t: f64) -> P<N> {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The part of `v` across `axis` (`axis` one long).
pub fn across<const N: usize>(v: P<N>, axis: P<N>) -> P<N> {
    sub(v, scale(axis, dot(v, axis)))
}

/// Any direction square to `a` (one long), for when nothing else says
/// which.
pub fn perpendicular<const N: usize>(a: P<N>) -> P<N> {
    let a = norm(a);
    // The axis `a` leans along least.
    let k = (0..N).min_by(|&i, &j| a[i].abs().total_cmp(&a[j].abs())).unwrap_or(0);
    let mut e = [0.0; N];
    e[k] = 1.0;
    let p = norm(across(e, a));
    if len(p) > 0.5 {
        return p;
    }
    // One dimension: nothing is square to it.
    [0.0; N]
}

/// `v` turned by the smallest rotation that takes direction `from` to
/// direction `to` (in the plane they span; what is square to that plane
/// stays). Works in any dimension.
pub fn turn<const N: usize>(v: P<N>, from: P<N>, to: P<N>) -> P<N> {
    let (a, b) = (norm(from), norm(to));
    if len(a) < 0.5 || len(b) < 0.5 {
        return v;
    }
    let c = dot(a, b).clamp(-1.0, 1.0);
    if c > 1.0 - 1e-12 {
        return v;
    }
    let w = {
        let w = across(b, a);
        if len(w) < 1e-9 { perpendicular(a) } else { norm(w) }
    };
    let (s, c) = (c.acos().sin(), c);
    let (pu, pw) = (dot(v, a), dot(v, w));
    let rest = sub(v, add(scale(a, pu), scale(w, pw)));
    add(rest, add(scale(a, pu * c - pw * s), scale(w, pu * s + pw * c)))
}

/// The angle between two directions, radians (0..π).
pub fn angle<const N: usize>(a: P<N>, b: P<N>) -> f64 {
    let (a, b) = (norm(a), norm(b));
    dot(a, b).clamp(-1.0, 1.0).acos()
}

/// The signed angle from `a` to `b` in a plane (radians, -π..π; positive
/// turns x toward y, which is clockwise on a y-down picture).
pub fn signed_angle(a: P<2>, b: P<2>) -> f64 {
    (a[0] * b[1] - a[1] * b[0]).atan2(a[0] * b[0] + a[1] * b[1])
}

/// Smoothstep from 0 at `a` to 1 at `b`.
pub fn smooth(a: f64, b: f64, x: f64) -> f64 {
    if (b - a).abs() < 1e-12 {
        return if x >= b { 1.0 } else { 0.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_takes_one_direction_to_another_in_any_dimension() {
        let v = turn([1.0, 0.0], [1.0, 0.0], [0.0, 1.0]);
        assert!(dist(v, [0.0, 1.0]) < 1e-12);
        // What lies square to the plane stays.
        let v = turn([1.0, 0.0, 5.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(dist(v, [0.0, 1.0, 5.0]) < 1e-12, "{v:?}");
        // Opposite directions: some half turn, length kept.
        let v = turn([2.0, 0.0, 0.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]);
        assert!(dist(v, [-2.0, 0.0, 0.0]) < 1e-9, "{v:?}");
        assert!((signed_angle([1.0, 0.0], [0.0, 1.0]) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!(len(perpendicular([0.3, 0.4, 0.5])) > 0.99 && dot(perpendicular([0.3, 0.4, 0.5]), [0.3, 0.4, 0.5]).abs() < 1e-12);
    }
}
