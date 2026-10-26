// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A bounding volume hierarchy over triangles: fast ray casts (what the
//! path tracer, skin-weight visibility and scattering on a surface use)
//! and nearest-point queries (what cloth and particles collide with).
//!
//! Built top down with the surface area heuristic over 12 bins per axis,
//! leaves of up to four triangles, nodes flattened depth first so the
//! left child follows its parent (the layout the GPU tracer reads too).

use crate::{V3, add, cross, dot, scale, sub};

/// A node: its box, and either two children (`count == 0`: the left one is
/// next, the right one at `index`) or `count` triangles from `index` in
/// [`Bvh::order`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub lo: V3,
    pub hi: V3,
    pub index: u32,
    pub count: u32,
}

/// Triangles in world space and the tree over them.
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    pub tris: Vec<[V3; 3]>,
    pub nodes: Vec<Node>,
    /// Triangle numbers in leaf order.
    pub order: Vec<u32>,
}

/// Where a ray met a triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub t: f32,
    pub tri: u32,
    /// Barycentrics of corners 1 and 2.
    pub u: f32,
    pub v: f32,
}

fn tri_box(t: &[V3; 3]) -> (V3, V3) {
    let lo = std::array::from_fn(|i| t[0][i].min(t[1][i]).min(t[2][i]));
    let hi = std::array::from_fn(|i| t[0][i].max(t[1][i]).max(t[2][i]));
    (lo, hi)
}

fn grow(b: &mut (V3, V3), o: (V3, V3)) {
    for i in 0..3 {
        b.0[i] = b.0[i].min(o.0[i]);
        b.1[i] = b.1[i].max(o.1[i]);
    }
}

fn area(b: &(V3, V3)) -> f32 {
    let d: V3 = std::array::from_fn(|i| (b.1[i] - b.0[i]).max(0.0));
    2.0 * (d[0] * d[1] + d[1] * d[2] + d[2] * d[0])
}

const EMPTY_BOX: (V3, V3) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);

impl Bvh {
    pub fn new(tris: Vec<[V3; 3]>) -> Bvh {
        let boxes: Vec<(V3, V3)> = tris.iter().map(tri_box).collect();
        Bvh::with_boxes(tris, boxes)
    }

    /// A tree over triangles that move between two places (motion blur):
    /// each box holds the triangle at both ends of its move.
    pub fn swept(start: Vec<[V3; 3]>, end: &[[V3; 3]]) -> Bvh {
        let boxes: Vec<(V3, V3)> = start
            .iter()
            .zip(end)
            .map(|(a, b)| {
                let mut x = tri_box(a);
                grow(&mut x, tri_box(b));
                x
            })
            .collect();
        Bvh::with_boxes(start, boxes)
    }

    fn with_boxes(tris: Vec<[V3; 3]>, boxes: Vec<(V3, V3)>) -> Bvh {
        let n = tris.len();
        let centres: Vec<V3> = boxes.iter().map(|b| scale(add(b.0, b.1), 0.5)).collect();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes = Vec::with_capacity(2 * n.max(1));
        if n > 0 {
            build(&boxes, &centres, &mut order, 0, n, &mut nodes);
        } else {
            nodes.push(Node { lo: [0.0; 3], hi: [0.0; 3], index: 0, count: 0 });
        }
        Bvh { tris, nodes, order }
    }

    /// The tree over a mesh's triangles as they are.
    pub fn of_mesh(m: &crate::Mesh) -> Bvh {
        Bvh::new((0..m.triangles()).map(|t| m.tri(t).map(|v| m.positions[v])).collect())
    }

    /// The nearest hit along `origin + t·dir` with `t` in `(t_min, t_max)`.
    pub fn ray(&self, origin: V3, dir: V3, t_min: f32, t_max: f32) -> Option<RayHit> {
        if self.tris.is_empty() {
            return None;
        }
        let inv: V3 = dir.map(|d| if d.abs() > 1e-12 { 1.0 / d } else { 1e12f32.copysign(d) });
        let mut best: Option<RayHit> = None;
        let mut far = t_max;
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let here = stack[sp];
            let node = &self.nodes[here as usize];
            if !slab(node, origin, inv, t_min, far) {
                continue;
            }
            if node.count > 0 {
                for k in node.index..node.index + node.count {
                    let ti = self.order[k as usize];
                    if let Some((t, u, v)) = ray_tri(&self.tris[ti as usize], origin, dir)
                        && t > t_min
                        && t < far
                    {
                        far = t;
                        best = Some(RayHit { t, tri: ti, u, v });
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = node.index;
                stack[sp + 1] = here + 1;
                sp += 2;
            }
        }
        best
    }

    /// Whether anything lies along the ray within `(t_min, t_max)`.
    pub fn occluded(&self, origin: V3, dir: V3, t_min: f32, t_max: f32) -> bool {
        self.ray(origin, dir, t_min, t_max).is_some()
    }

    /// The nearest point on any triangle to `p` within `radius`: (point,
    /// triangle, distance).
    pub fn nearest(&self, p: V3, radius: f32) -> Option<(V3, u32, f32)> {
        if self.tris.is_empty() {
            return None;
        }
        let mut best: Option<(V3, u32, f32)> = None;
        let mut r = radius;
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            let d2: f32 = (0..3).map(|k| (node.lo[k] - p[k]).max(0.0).max(p[k] - node.hi[k])).map(|d| d * d).sum();
            if d2 > r * r {
                continue;
            }
            if node.count > 0 {
                for k in node.index..node.index + node.count {
                    let ti = self.order[k as usize];
                    let q = closest_on_tri(&self.tris[ti as usize], p);
                    let d = crate::length(sub(q, p));
                    if d < r {
                        r = d;
                        best = Some((q, ti, d));
                    }
                }
            } else {
                stack.push(node.index);
                stack.push(i + 1);
            }
        }
        best
    }
}

fn build(boxes: &[(V3, V3)], centres: &[V3], order: &mut [u32], start: usize, end: usize, nodes: &mut Vec<Node>) -> u32 {
    let me = nodes.len() as u32;
    let mut bb = EMPTY_BOX;
    let mut cb = EMPTY_BOX;
    for &t in &order[start..end] {
        grow(&mut bb, boxes[t as usize]);
        let c = centres[t as usize];
        grow(&mut cb, (c, c));
    }
    nodes.push(Node { lo: bb.0, hi: bb.1, index: start as u32, count: (end - start) as u32 });
    let count = end - start;
    if count <= 4 {
        return me;
    }
    const BINS: usize = 12;
    let mut best = (f32::INFINITY, 0usize, 0f32);
    for axis in 0..3 {
        let (lo, hi) = (cb.0[axis], cb.1[axis]);
        if hi - lo < 1e-9 {
            continue;
        }
        let mut bins = [(EMPTY_BOX, 0usize); BINS];
        let k = BINS as f32 / (hi - lo);
        for &t in &order[start..end] {
            let b = (((centres[t as usize][axis] - lo) * k) as usize).min(BINS - 1);
            grow(&mut bins[b].0, boxes[t as usize]);
            bins[b].1 += 1;
        }
        for split in 1..BINS {
            let (mut l, mut r) = (EMPTY_BOX, EMPTY_BOX);
            let (mut nl, mut nr) = (0, 0);
            for b in &bins[..split] {
                if b.1 > 0 {
                    grow(&mut l, b.0);
                    nl += b.1;
                }
            }
            for b in &bins[split..] {
                if b.1 > 0 {
                    grow(&mut r, b.0);
                    nr += b.1;
                }
            }
            if nl == 0 || nr == 0 {
                continue;
            }
            let cost = area(&l) * nl as f32 + area(&r) * nr as f32;
            if cost < best.0 {
                best = (cost, axis, lo + split as f32 / k);
            }
        }
    }
    let leaf_cost = area(&bb) * count as f32;
    if !best.0.is_finite() || (best.0 >= leaf_cost && count <= 12) {
        if count <= 12 || !best.0.is_finite() {
            // All centres in one spot: halve the list.
            if !best.0.is_finite() && count > 12 {
                let mid = start + count / 2;
                nodes[me as usize].count = 0;
                build(boxes, centres, order, start, mid, nodes);
                let right = build(boxes, centres, order, mid, end, nodes);
                nodes[me as usize].index = right;
            }
            return me;
        }
    }
    let (axis, at) = (best.1, best.2);
    let slice = &mut order[start..end];
    let mut i = 0;
    for j in 0..slice.len() {
        if centres[slice[j] as usize][axis] < at {
            slice.swap(i, j);
            i += 1;
        }
    }
    let mid = if i == 0 || i == count { start + count / 2 } else { start + i };
    nodes[me as usize].count = 0;
    build(boxes, centres, order, start, mid, nodes);
    let right = build(boxes, centres, order, mid, end, nodes);
    nodes[me as usize].index = right;
    me
}

fn slab(n: &Node, o: V3, inv: V3, t_min: f32, t_max: f32) -> bool {
    let mut t0 = t_min;
    let mut t1 = t_max;
    for i in 0..3 {
        let a = (n.lo[i] - o[i]) * inv[i];
        let b = (n.hi[i] - o[i]) * inv[i];
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    t0 <= t1
}

/// Möller–Trumbore: (t, u, v), both sides.
pub fn ray_tri(t: &[V3; 3], o: V3, d: V3) -> Option<(f32, f32, f32)> {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, t[0]);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some((dot(e2, q) * inv, u, v))
}

/// The point of triangle `t` nearest `p` (Ericson's region test).
pub fn closest_on_tri(t: &[V3; 3], p: V3) -> V3 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return add(a, scale(ab, v));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return add(a, scale(ac, w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), w));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    add(a, add(scale(ab, v), scale(ac, w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_the_nearest_face_and_miss_the_rest() {
        let m = crate::prim::cube([100.0, 100.0, 100.0]);
        let b = Bvh::of_mesh(&m);
        let h = b.ray([0.0, 0.0, -500.0], [0.0, 0.0, 1.0], 0.0, f32::INFINITY).unwrap();
        assert!((h.t - 450.0).abs() < 1e-3, "{h:?}");
        assert!(b.ray([0.0, 300.0, -500.0], [0.0, 0.0, 1.0], 0.0, f32::INFINITY).is_none());
        // Many spheres: same answers as testing every triangle.
        let mut s = crate::prim::ico_sphere(50.0, 3);
        crate::ops::array(&mut s, 6, [130.0, 20.0, 15.0]);
        let b = Bvh::of_mesh(&s);
        for k in 0..200 {
            let o = [k as f32 * 4.0 - 100.0, (k as f32 * 0.37).sin() * 80.0, -400.0];
            let d = crate::normalise([((k * 7) % 13) as f32 * 0.01, 0.05, 1.0]);
            let brute = (0..s.triangles()).filter_map(|t| ray_tri(&s.tri(t).map(|v| s.positions[v]), o, d).filter(|r| r.0 > 0.0).map(|r| r.0)).fold(f32::INFINITY, f32::min);
            let got = b.ray(o, d, 0.0, f32::INFINITY).map_or(f32::INFINITY, |h| h.t);
            assert!((brute - got).abs() < 1e-2 || (brute.is_infinite() && got.is_infinite()), "{k}: {brute} {got}");
        }
        let (q, _, d) = b.nearest([0.0, 0.0, -80.0], 1000.0).unwrap();
        assert!((d - 30.0).abs() < 1.5, "{q:?} {d}");
    }
}
