// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Curves and surfaces: Bézier, Catmull–Rom and NURBS curves; sweeping a
//! profile along a path (pipes, rails, mouldings, cables, horns, tails),
//! lofting through sections (hulls, bottles, wings), NURBS patches, and
//! extruding flat outlines with holes (logos, floor plans, cut panels),
//! with a bevel on their rims.

use crate::poly::Poly;
use serde::{Deserialize, Serialize};
use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// A curve in space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Curve {
    /// Straight segments through the points.
    Polyline {
        points: Vec<V3>,
        #[serde(default)]
        closed: bool,
    },
    /// A smooth curve through every point.
    Smooth {
        points: Vec<V3>,
        #[serde(default)]
        closed: bool,
    },
    /// Cubic Bézier segments: each knot with the handles before and after
    /// it (missing handles make smooth ones).
    Bezier {
        knots: Vec<BezierKnot>,
        #[serde(default)]
        closed: bool,
    },
    /// A NURBS curve: control points with weights, of a degree; knots are
    /// spread evenly (clamped at the ends) when not given.
    Nurbs {
        points: Vec<V3>,
        #[serde(default)]
        weights: Vec<f32>,
        #[serde(default = "three")]
        degree: usize,
        #[serde(default)]
        knots: Vec<f32>,
    },
    /// A circle (or arc) of `radius` round `axis` (0 x, 1 y, 2 z).
    Circle {
        radius: f32,
        #[serde(default = "y_axis")]
        axis: usize,
        #[serde(default)]
        centre: V3,
        #[serde(default = "full")]
        degrees: f32,
    },
    /// A coil: `turns` round `axis`, rising `height`.
    Helix {
        radius: f32,
        height: f32,
        turns: f32,
        #[serde(default = "y_axis")]
        axis: usize,
    },
}

fn three() -> usize {
    3
}
fn y_axis() -> usize {
    1
}
fn full() -> f32 {
    360.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BezierKnot {
    pub at: V3,
    #[serde(default)]
    pub before: Option<V3>,
    #[serde(default)]
    pub after: Option<V3>,
}

impl Curve {
    pub fn closed(&self) -> bool {
        match self {
            Curve::Polyline { closed, .. } | Curve::Smooth { closed, .. } | Curve::Bezier { closed, .. } => *closed,
            Curve::Circle { degrees, .. } => (degrees.abs() - 360.0).abs() < 1e-3,
            _ => false,
        }
    }

    /// Points along the curve, dense enough to look smooth (`n` per
    /// segment for curved kinds).
    pub fn sample(&self, n: usize) -> Vec<V3> {
        let n = n.max(1);
        match self {
            Curve::Polyline { points, .. } => points.clone(),
            Curve::Smooth { points, closed } => catmull_rom(points, *closed, n),
            Curve::Bezier { knots, closed } => bezier(knots, *closed, n),
            Curve::Nurbs { points, weights, degree, knots } => {
                let count = (points.len() * n).max(2);
                (0..=count).map(|i| nurbs_point(points, weights, *degree, knots, i as f32 / count as f32)).collect()
            }
            Curve::Circle { radius, axis, centre, degrees } => {
                let full = (degrees.abs() - 360.0).abs() < 1e-3;
                let steps = (n * 8).max(8);
                let m = if full { steps } else { steps + 1 };
                (0..m)
                    .map(|i| {
                        let a = degrees.to_radians() * i as f32 / steps as f32;
                        add(*centre, on_axis(*axis, radius * a.cos(), radius * a.sin(), 0.0))
                    })
                    .collect()
            }
            Curve::Helix { radius, height, turns, axis } => {
                let steps = ((turns.abs() * 16.0 * n as f32).ceil() as usize).max(8);
                (0..=steps)
                    .map(|i| {
                        let t = i as f32 / steps as f32;
                        let a = std::f32::consts::TAU * turns * t;
                        on_axis(*axis, radius * a.cos(), radius * a.sin(), height * (t - 0.5))
                    })
                    .collect()
            }
        }
    }

    /// Points spread evenly by length along the curve.
    pub fn even(&self, count: usize) -> Vec<V3> {
        resample(&self.sample(16), count.max(2), self.closed())
    }
}

/// (a, b) in the plane round `axis`, `h` along it.
fn on_axis(axis: usize, a: f32, b: f32, h: f32) -> V3 {
    match axis {
        0 => [h, a, b],
        2 => [a, b, h],
        _ => [a, h, b],
    }
}

fn catmull_rom(p: &[V3], closed: bool, n: usize) -> Vec<V3> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let m = p.len();
    let at = |i: isize| -> V3 {
        if closed {
            p[i.rem_euclid(m as isize) as usize]
        } else if i < 0 {
            sub(scale(p[0], 2.0), p[1])
        } else if i as usize >= m {
            sub(scale(p[m - 1], 2.0), p[m - 2])
        } else {
            p[i as usize]
        }
    };
    let segs = if closed { m } else { m - 1 };
    let mut out = Vec::new();
    for s in 0..segs {
        let (p0, p1, p2, p3) = (at(s as isize - 1), at(s as isize), at(s as isize + 1), at(s as isize + 2));
        for k in 0..n {
            let t = k as f32 / n as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(std::array::from_fn(|i| 0.5 * (2.0 * p1[i] + (-p0[i] + p2[i]) * t + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * t2 + (-p0[i] + 3.0 * p1[i] - 3.0 * p2[i] + p3[i]) * t3)));
        }
    }
    if !closed {
        out.push(p[m - 1]);
    }
    out
}

fn bezier(k: &[BezierKnot], closed: bool, n: usize) -> Vec<V3> {
    if k.len() < 2 {
        return k.iter().map(|x| x.at).collect();
    }
    let m = k.len();
    // Missing handles: a third of the way to the neighbours (smooth).
    let tangent = |i: usize| -> V3 {
        let prev = if i > 0 { k[i - 1].at } else if closed { k[m - 1].at } else { k[i].at };
        let next = if i + 1 < m { k[i + 1].at } else if closed { k[0].at } else { k[i].at };
        scale(sub(next, prev), 1.0 / 6.0)
    };
    let after = |i: usize| k[i].after.unwrap_or_else(|| add(k[i].at, tangent(i)));
    let before = |i: usize| k[i].before.unwrap_or_else(|| sub(k[i].at, tangent(i)));
    let segs = if closed { m } else { m - 1 };
    let mut out = Vec::new();
    for s in 0..segs {
        let (a, b) = (s, (s + 1) % m);
        let (p0, p1, p2, p3) = (k[a].at, after(a), before(b), k[b].at);
        for j in 0..n {
            let t = j as f32 / n as f32;
            let u = 1.0 - t;
            out.push(std::array::from_fn(|i| u * u * u * p0[i] + 3.0 * u * u * t * p1[i] + 3.0 * u * t * t * p2[i] + t * t * t * p3[i]));
        }
    }
    if !closed {
        out.push(k[m - 1].at);
    }
    out
}

/// A clamped, even knot vector.
pub fn clamped_knots(count: usize, degree: usize) -> Vec<f32> {
    let p = degree.min(count.saturating_sub(1)).max(1);
    let m = count + p + 1;
    (0..m)
        .map(|i| {
            if i <= p {
                0.0
            } else if i >= m - p - 1 {
                1.0
            } else {
                (i - p) as f32 / (count - p) as f32
            }
        })
        .collect()
}

/// A NURBS curve's point at `t` (0–1), by de Boor's algorithm on
/// homogeneous points.
pub fn nurbs_point(points: &[V3], weights: &[f32], degree: usize, knots: &[f32], t: f32) -> V3 {
    let n = points.len();
    if n == 0 {
        return [0.0; 3];
    }
    if n == 1 {
        return points[0];
    }
    let p = degree.min(n - 1).max(1);
    let kv: Vec<f32> = if knots.len() == n + p + 1 { knots.to_vec() } else { clamped_knots(n, p) };
    let (lo, hi) = (kv[p], kv[n]);
    let u = lo + (hi - lo) * t.clamp(0.0, 1.0);
    // Knot span.
    let mut k = p;
    while k < n - 1 && u >= kv[k + 1] {
        k += 1;
    }
    let w = |i: usize| weights.get(i).copied().unwrap_or(1.0);
    let mut d: Vec<[f32; 4]> = (0..=p)
        .map(|j| {
            let i = j + k - p;
            let q = points[i];
            [q[0] * w(i), q[1] * w(i), q[2] * w(i), w(i)]
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let den = kv[i + p - r + 1] - kv[i];
            let a = if den.abs() < 1e-12 { 0.0 } else { (u - kv[i]) / den };
            d[j] = std::array::from_fn(|c| (1.0 - a) * d[j - 1][c] + a * d[j][c]);
        }
    }
    let h = d[p];
    if h[3].abs() < 1e-12 { [h[0], h[1], h[2]] } else { [h[0] / h[3], h[1] / h[3], h[2] / h[3]] }
}

/// Lengths along a polyline from its start.
fn arc(points: &[V3], closed: bool) -> Vec<f32> {
    let mut s = vec![0.0];
    let n = points.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let d = length(sub(points[(i + 1) % n], points[i]));
        s.push(s[i] + d);
    }
    s
}

/// `count` points spread evenly by length (for a closed loop, the last
/// doesn't repeat the first).
pub fn resample(points: &[V3], count: usize, closed: bool) -> Vec<V3> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let s = arc(points, closed);
    let total = *s.last().unwrap_or(&0.0);
    let n = points.len();
    let steps = if closed { count } else { count - 1 };
    let mut out = Vec::with_capacity(count);
    let mut seg = 0;
    for i in 0..count {
        let target = total * i as f32 / steps.max(1) as f32;
        while seg + 1 < s.len() - 1 && s[seg + 1] < target {
            seg += 1;
        }
        let (a, b) = (points[seg % n], points[(seg + 1) % n]);
        let len = (s[seg + 1] - s[seg]).max(1e-12);
        out.push(lerp(a, b, ((target - s[seg]) / len).clamp(0.0, 1.0)));
    }
    out
}

/// Frames along a path that twist as little as possible (double
/// reflection): (tangent, normal, binormal) at each point.
pub fn frames(path: &[V3], closed: bool) -> Vec<[V3; 3]> {
    let n = path.len();
    if n < 2 {
        return vec![[[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; n];
    }
    let tangent = |i: usize| -> V3 {
        let (a, b) = if closed {
            (path[(i + n - 1) % n], path[(i + 1) % n])
        } else {
            (path[i.saturating_sub(1)], path[(i + 1).min(n - 1)])
        };
        normalise(sub(b, a))
    };
    let t0 = tangent(0);
    let helper = if t0[1].abs() < 0.9 { [0.0, -1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let r0 = normalise(cross(cross(t0, helper), t0));
    let mut out = vec![[t0, r0, cross(t0, r0)]];
    for i in 0..n - 1 {
        let [ti, ri, _] = out[i];
        let v1 = sub(path[i + 1], path[i]);
        let c1 = dot(v1, v1);
        if c1 < 1e-20 {
            out.push(out[i]);
            continue;
        }
        let rl = sub(ri, scale(v1, 2.0 / c1 * dot(v1, ri)));
        let tl = sub(ti, scale(v1, 2.0 / c1 * dot(v1, ti)));
        let t1 = tangent(i + 1);
        let v2 = sub(t1, tl);
        let c2 = dot(v2, v2);
        let r1 = if c2 < 1e-20 { rl } else { sub(rl, scale(v2, 2.0 / c2 * dot(v2, rl))) };
        let r1 = normalise(r1);
        out.push([t1, r1, cross(t1, r1)]);
    }
    if closed && n > 2 {
        // Spread the leftover twist so the loop closes.
        let [_, rn, _] = out[n - 1];
        let [t0, r0, b0] = out[0];
        let _ = t0;
        let ang = dot(rn, b0).atan2(dot(rn, r0));
        for (i, f) in out.iter_mut().enumerate() {
            let a = -ang * i as f32 / n as f32;
            let (s, c) = a.sin_cos();
            let (r, b) = (f[1], f[2]);
            f[1] = add(scale(r, c), scale(b, s));
            f[2] = sub(scale(b, c), scale(r, s));
        }
    }
    out
}

/// A cross-section to sweep: points in its own plane (x across, y up).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Profile {
    Circle {
        radius: f32,
        #[serde(default = "sixteen")]
        segments: usize,
    },
    /// A rectangle with rounded corners.
    Rect {
        width: f32,
        height: f32,
        #[serde(default)]
        corner: f32,
        #[serde(default = "four")]
        corner_segments: usize,
    },
    /// Any outline (x, y) pairs, closed or open (open makes a strip, as
    /// for a moulding's face).
    Points {
        points: Vec<[f32; 2]>,
        #[serde(default = "yes")]
        closed: bool,
    },
}

fn sixteen() -> usize {
    16
}
fn four() -> usize {
    4
}
fn yes() -> bool {
    true
}

impl Profile {
    pub fn points(&self) -> (Vec<[f32; 2]>, bool) {
        match self {
            Profile::Circle { radius, segments } => {
                let n = (*segments).max(3);
                ((0..n).map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / n as f32;
                    [radius * a.cos(), radius * a.sin()]
                }).collect(), true)
            }
            Profile::Rect { width, height, corner, corner_segments } => (rounded_rect(*width, *height, *corner, *corner_segments), true),
            Profile::Points { points, closed } => (points.clone(), *closed),
        }
    }
}

/// A rectangle's outline (counter-clockwise), corners rounded by `r`.
pub fn rounded_rect(w: f32, h: f32, r: f32, segs: usize) -> Vec<[f32; 2]> {
    let r = r.clamp(0.0, w.min(h) / 2.0);
    let (hw, hh) = (w / 2.0, h / 2.0);
    if r <= 1e-6 {
        return vec![[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
    }
    let s = segs.max(1);
    let mut out = Vec::new();
    for (cx, cy, a0) in [(hw - r, -hh + r, -90.0f32), (hw - r, hh - r, 0.0), (-hw + r, hh - r, 90.0), (-hw + r, -hh + r, 180.0)] {
        for k in 0..=s {
            let a = (a0 + 90.0 * k as f32 / s as f32).to_radians();
            out.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    out
}

/// How a sweep changes along its length.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Along {
    /// Scale of the profile at the start and end (and between, evenly).
    #[serde(default)]
    pub scale: Vec<f32>,
    /// Degrees the profile turns from start to end.
    #[serde(default)]
    pub twist: f32,
    /// Close the ends of a closed profile.
    #[serde(default)]
    pub caps: bool,
}

fn scale_at(list: &[f32], t: f32) -> f32 {
    match list.len() {
        0 => 1.0,
        1 => list[0],
        n => {
            let x = t * (n - 1) as f32;
            let i = (x.floor() as usize).min(n - 2);
            let f = x - i as f32;
            list[i] * (1.0 - f) + list[i + 1] * f
        }
    }
}

/// A profile swept along a path: tubes, rails, trims, horns, tails.
pub fn sweep(path: &Curve, profile: &Profile, along: &Along, samples: usize) -> Poly {
    let pts = path.sample(samples.max(2));
    let closed_path = path.closed();
    let fr = frames(&pts, closed_path);
    let (prof, closed_prof) = profile.points();
    let mut p = Poly::new();
    let n = pts.len();
    let m = prof.len();
    let mut ring_ids = Vec::with_capacity(n);
    for (i, (c, f)) in pts.iter().zip(&fr).enumerate() {
        let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
        let k = scale_at(&along.scale, t);
        let tw = (along.twist * t).to_radians();
        let (s, co) = tw.sin_cos();
        let ids: Vec<u32> = prof
            .iter()
            .map(|q| {
                let (x, y) = (q[0] * co - q[1] * s, q[0] * s + q[1] * co);
                p.add_vertex(add(*c, add(scale(f[1], x * k), scale(f[2], y * k))))
            })
            .collect();
        ring_ids.push(ids);
    }
    let rows = if closed_path { n } else { n - 1 };
    let cols = if closed_prof { m } else { m - 1 };
    for i in 0..rows {
        let i2 = (i + 1) % n;
        for j in 0..cols {
            let j2 = (j + 1) % m;
            let q = [ring_ids[i][j], ring_ids[i][j2], ring_ids[i2][j2], ring_ids[i2][j]];
            let (u0, u1) = (j as f32 / cols as f32, (j + 1) as f32 / cols as f32);
            let (v0, v1) = (i as f32 / rows as f32, (i + 1) as f32 / rows as f32);
            p.add_face_like(&q, Some(&[[u0, v0], [u1, v0], [u1, v1], [u0, v1]]), 0, 0, true);
        }
    }
    if along.caps && closed_prof && !closed_path {
        let first: Vec<u32> = ring_ids[0].iter().rev().copied().collect();
        let last: Vec<u32> = ring_ids[n - 1].clone();
        p.add_face(&first, 0);
        p.add_face(&last, 0);
    }
    p.ensure_materials();
    if closed_prof && (closed_path || along.caps) {
        p.orient_outward();
    }
    p
}

/// A surface through cross-sections in order (each a closed loop when
/// `closed`, resampled to the same count).
pub fn loft(sections: &[Vec<V3>], closed: bool, count: usize, smooth: bool) -> Poly {
    let count = count.max(3);
    let rings: Vec<Vec<V3>> = sections.iter().map(|s| resample(s, count, closed)).collect();
    // Smooth between sections: interpolate each column with Catmull–Rom.
    let rings: Vec<Vec<V3>> = if smooth && rings.len() >= 3 {
        let cols: Vec<Vec<V3>> = (0..count).map(|j| catmull_rom(&rings.iter().map(|r| r[j]).collect::<Vec<_>>(), false, 4)).collect();
        (0..cols[0].len()).map(|i| cols.iter().map(|c| c[i]).collect()).collect()
    } else {
        rings
    };
    let mut p = Poly::new();
    let ids: Vec<Vec<u32>> = rings.iter().map(|r| r.iter().map(|q| p.add_vertex(*q)).collect()).collect();
    let cols = if closed { count } else { count - 1 };
    for i in 0..ids.len().saturating_sub(1) {
        for j in 0..cols {
            let j2 = (j + 1) % count;
            let (u0, u1) = (j as f32 / cols as f32, (j + 1) as f32 / cols as f32);
            let (v0, v1) = (i as f32 / (ids.len() - 1) as f32, (i + 1) as f32 / (ids.len() - 1) as f32);
            p.add_face_like(&[ids[i][j], ids[i][j2], ids[i + 1][j2], ids[i + 1][j]], Some(&[[u0, v0], [u1, v0], [u1, v1], [u0, v1]]), 0, 0, true);
        }
    }
    p.ensure_materials();
    p
}

/// A NURBS patch: rows of control points (with weights), tessellated into
/// `res` × `res` quads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NurbsSurface {
    pub rows: Vec<Vec<V3>>,
    #[serde(default)]
    pub weights: Vec<Vec<f32>>,
    #[serde(default = "three")]
    pub degree_u: usize,
    #[serde(default = "three")]
    pub degree_v: usize,
}

impl NurbsSurface {
    pub fn point(&self, u: f32, v: f32) -> V3 {
        // Along each row at u, then down the column at v.
        let col: Vec<V3> = self.rows.iter().enumerate().map(|(r, row)| nurbs_point(row, self.weights.get(r).map(Vec::as_slice).unwrap_or(&[]), self.degree_u, &[], u)).collect();
        nurbs_point(&col, &[], self.degree_v, &[], v)
    }

    pub fn tessellate(&self, res_u: usize, res_v: usize) -> Poly {
        let (nu, nv) = (res_u.max(1), res_v.max(1));
        let mut p = Poly::new();
        for j in 0..=nv {
            for i in 0..=nu {
                p.add_vertex(self.point(i as f32 / nu as f32, j as f32 / nv as f32));
            }
        }
        let id = |i: usize, j: usize| (j * (nu + 1) + i) as u32;
        for j in 0..nv {
            for i in 0..nu {
                let uv = |a: usize, b: usize| [a as f32 / nu as f32, b as f32 / nv as f32];
                p.add_face_like(&[id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)], Some(&[uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)]), 0, 0, true);
            }
        }
        p.ensure_materials();
        p
    }
}

fn signed_area(p: &[[f32; 2]]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f32>() / 2.0
}

/// An outline with holes as one simple polygon: each hole joined to the
/// outline by a cut (so ear clipping can fill it).
pub fn join_holes(outline: &[[f32; 2]], holes: &[Vec<[f32; 2]>]) -> Vec<[f32; 2]> {
    let mut poly: Vec<[f32; 2]> = outline.to_vec();
    if signed_area(&poly) < 0.0 {
        poly.reverse();
    }
    // Rightmost holes first.
    let mut hs: Vec<Vec<[f32; 2]>> = holes.iter().filter(|h| h.len() >= 3).cloned().collect();
    for h in &mut hs {
        if signed_area(h) > 0.0 {
            h.reverse();
        }
    }
    hs.sort_by(|a, b| {
        let ma = a.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        let mb = b.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        mb.total_cmp(&ma)
    });
    for h in hs {
        let (hi, hp) = h.iter().enumerate().max_by(|a, b| a.1[0].total_cmp(&b.1[0])).map(|(i, p)| (i, *p)).unwrap();
        // The nearest outline vertex to the right that the cut can reach
        // without crossing an edge.
        let crosses = |a: [f32; 2], b: [f32; 2], poly: &[[f32; 2]]| -> bool {
            let n = poly.len();
            (0..n).any(|i| {
                let (c, d) = (poly[i], poly[(i + 1) % n]);
                if c == a || d == a || c == b || d == b {
                    return false;
                }
                let o = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
                (o(a, b, c) > 0.0) != (o(a, b, d) > 0.0) && (o(c, d, a) > 0.0) != (o(c, d, b) > 0.0)
            })
        };
        let best = (0..poly.len())
            .filter(|&i| !crosses(hp, poly[i], &poly) && !crosses(hp, poly[i], &h))
            .min_by(|&a, &b| {
                let da = (poly[a][0] - hp[0]).powi(2) + (poly[a][1] - hp[1]).powi(2) + if poly[a][0] < hp[0] { 1e9 } else { 0.0 };
                let db = (poly[b][0] - hp[0]).powi(2) + (poly[b][1] - hp[1]).powi(2) + if poly[b][0] < hp[0] { 1e9 } else { 0.0 };
                da.total_cmp(&db)
            });
        let Some(oi) = best else { continue };
        let mut joined = Vec::with_capacity(poly.len() + h.len() + 2);
        joined.extend_from_slice(&poly[..=oi]);
        for k in 0..=h.len() {
            joined.push(h[(hi + k) % h.len()]);
        }
        joined.push(poly[oi]);
        joined.extend_from_slice(&poly[oi + 1..]);
        poly = joined;
    }
    poly
}

/// A flat outline (x, z; holes allowed) pulled up into a solid `depth`
/// tall (along −y, up), its rims bevelled by `bevel` over `segments`.
pub fn extrude_outline(outline: &[[f32; 2]], holes: &[Vec<[f32; 2]>], depth: f32, bevel: f32, segments: usize) -> Poly {
    let mut p = Poly::new();
    let mut rings: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut o = outline.to_vec();
    if signed_area(&o) < 0.0 {
        o.reverse();
    }
    rings.push(o.clone());
    for h in holes {
        let mut h = h.clone();
        if signed_area(&h) > 0.0 {
            h.reverse();
        }
        rings.push(h);
    }
    let segs = if bevel > 0.0 { segments.max(1) } else { 0 };
    let b = bevel.min(depth / 2.0).max(0.0);
    // Layers from the bottom (y = 0) up: (inset, height).
    let mut layers: Vec<(f32, f32)> = Vec::new();
    for k in 0..=segs {
        let a = std::f32::consts::FRAC_PI_2 * k as f32 / segs.max(1) as f32;
        if segs == 0 {
            layers.push((0.0, 0.0));
            break;
        }
        layers.push((b * (1.0 - a.sin()), b * (1.0 - a.cos())));
    }
    let top: Vec<(f32, f32)> = layers.iter().rev().map(|(i, h)| (*i, depth - h)).collect();
    layers.extend(top);
    // Offset a ring inward by d (miter per vertex).
    let inset = |ring: &[[f32; 2]], d: f32| -> Vec<[f32; 2]> {
        let n = ring.len();
        (0..n)
            .map(|i| {
                let (a, c, b) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
                let e1 = normalise([c[0] - a[0], c[1] - a[1], 0.0]);
                let e2 = normalise([b[0] - c[0], b[1] - c[1], 0.0]);
                // Inward for counter-clockwise rings is to the left.
                let n1 = [-e1[1], e1[0]];
                let n2 = [-e2[1], e2[0]];
                let m = normalise([n1[0] + n2[0], n1[1] + n2[1], 0.0]);
                let k = (m[0] * n1[0] + m[1] * n1[1]).max(0.25);
                [c[0] + m[0] * d / k, c[1] + m[1] * d / k]
            })
            .collect()
    };
    let mut ring_layers: Vec<Vec<Vec<u32>>> = Vec::new();
    for ring in &rings {
        let mut ls = Vec::new();
        for &(d, y) in &layers {
            let r = inset(ring, d);
            ls.push(r.iter().map(|q| p.add_vertex([q[0], -y, q[1]])).collect::<Vec<u32>>());
        }
        ring_layers.push(ls);
    }
    for ls in &ring_layers {
        let n = ls[0].len();
        for l in 0..ls.len() - 1 {
            for i in 0..n {
                let j = (i + 1) % n;
                p.add_face(&[ls[l][i], ls[l][j], ls[l + 1][j], ls[l + 1][i]], 0);
            }
        }
    }
    // Caps: bottom and top from the joined outline.
    for (layer, up) in [(0usize, false), (layers.len() - 1, true)] {
        let d = layers[layer].0;
        let o2 = inset(&rings[0], d);
        let hs: Vec<Vec<[f32; 2]>> = rings[1..].iter().map(|h| inset(h, d)).collect();
        let flat = join_holes(&o2, &hs);
        let y = -layers[layer].1;
        // Find each joined point's vertex in this layer.
        let lookup: Vec<u32> = flat
            .iter()
            .map(|q| {
                let mut best = (f32::INFINITY, 0u32);
                for ls in &ring_layers {
                    for &v in &ls[layer] {
                        let pv = p.positions[v as usize];
                        let dd = (pv[0] - q[0]).powi(2) + (pv[2] - q[1]).powi(2) + (pv[1] - y).powi(2);
                        if dd < best.0 {
                            best = (dd, v);
                        }
                    }
                }
                best.1
            })
            .collect();
        let pos3: Vec<V3> = flat.iter().map(|q| [q[0], y, q[1]]).collect();
        let idx: Vec<u32> = (0..flat.len() as u32).collect();
        for t in crate::poly::triangulate(&pos3, &idx) {
            let tri = [lookup[t[0]], lookup[t[1]], lookup[t[2]]];
            if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                continue;
            }
            if up {
                p.add_face(&tri, 0);
            } else {
                p.add_face(&[tri[0], tri[2], tri[1]], 0);
            }
        }
    }
    p.ensure_materials();
    p.orient_outward();
    crate::edit::tris_to_quads(&mut p, 1.0);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poly::Topology;

    #[test]
    fn nurbs_circle_is_round() {
        // A rational quadratic circle: 9 points, weights 1 / √2.
        let w = std::f32::consts::FRAC_1_SQRT_2;
        let pts = vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, -1.0, 0.0], [0.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 0.0, 0.0]];
        let weights = vec![1.0, w, 1.0, w, 1.0, w, 1.0, w, 1.0];
        let knots = vec![0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0];
        for i in 0..=40 {
            let q = nurbs_point(&pts, &weights, 2, &knots, i as f32 / 40.0);
            assert!((length(q) - 1.0).abs() < 1e-4, "{q:?}");
        }
    }

    #[test]
    fn a_swept_pipe_and_a_capped_tube() {
        let path = Curve::Smooth { points: vec![[0.0, 0.0, 0.0], [50.0, -20.0, 0.0], [100.0, 0.0, 30.0], [150.0, 10.0, 0.0]], closed: false };
        let pipe = sweep(&path, &Profile::Circle { radius: 5.0, segments: 12 }, &Along { caps: true, ..Default::default() }, 8);
        assert!(Topology::of(&pipe).boundary_edges().is_empty());
        // Rotation-minimising: every ring stays round (equal radius).
        let ring = Curve::Circle { radius: 40.0, axis: 1, centre: [0.0; 3], degrees: 360.0 };
        let donut = sweep(&ring, &Profile::Circle { radius: 5.0, segments: 10 }, &Along::default(), 4);
        assert!(Topology::of(&donut).boundary_edges().is_empty(), "{}", donut.stats());
    }

    #[test]
    fn loft_and_patch() {
        let sec = |r: f32, y: f32| (0..12).map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 12.0;
            [r * a.cos(), y, r * a.sin()]
        }).collect::<Vec<V3>>();
        let bottle = loft(&[sec(10.0, 0.0), sec(12.0, -20.0), sec(4.0, -35.0), sec(4.0, -45.0)], true, 16, true);
        assert!(bottle.faces() > 16 * 3);
        let patch = NurbsSurface { rows: vec![vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]], vec![[0.0, 0.0, 10.0], [10.0, -10.0, 10.0], [20.0, 0.0, 10.0]], vec![[0.0, 0.0, 20.0], [10.0, 0.0, 20.0], [20.0, 0.0, 20.0]]], weights: vec![], degree_u: 2, degree_v: 2 };
        let t = patch.tessellate(8, 8);
        assert_eq!(t.faces(), 64);
        assert!(t.bounds().0[1] < -2.0);
    }

    #[test]
    fn outlines_with_holes_extrude_closed() {
        let outline = vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]];
        let hole = vec![[10.0, 10.0], [10.0, 20.0], [30.0, 20.0], [30.0, 10.0]];
        let s = extrude_outline(&outline, &[hole], 10.0, 1.5, 2);
        assert!(Topology::of(&s).boundary_edges().is_empty(), "{}", s.stats());
        let (lo, hi) = s.bounds();
        assert!((hi[1] - lo[1] - 10.0).abs() < 1e-3);
    }
}
