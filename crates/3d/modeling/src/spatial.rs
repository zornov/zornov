// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finding things on a surface fast: a tree of boxes over triangles for
//! ray casts (baking, picking, visibility) and nearest-point queries
//! (shrink-wrapping, projecting a retopology cage, transferring detail),
//! and a hash grid over points (sculpt brushes, welding).

use std::collections::HashMap;
use studio_mesh::{V3, add, cross, dot, length, scale, sub};

#[derive(Clone, Copy, Debug)]
struct Node {
    lo: V3,
    hi: V3,
    /// Leaf: first triangle in `order` and how many; inner: right child
    /// (left is next) and count 0.
    index: u32,
    count: u32,
}

/// Triangles and a box tree over them.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub tris: Vec<[V3; 3]>,
    nodes: Vec<Node>,
    order: Vec<u32>,
}

/// Where a ray met the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub tri: u32,
    /// Barycentric weights of the triangle's second and third corners.
    pub uv: [f32; 2],
}

fn tri_box(t: &[V3; 3]) -> (V3, V3) {
    let mut lo = t[0];
    let mut hi = t[0];
    for p in &t[1..] {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

fn grow(lo: &mut V3, hi: &mut V3, b: (V3, V3)) {
    for i in 0..3 {
        lo[i] = lo[i].min(b.0[i]);
        hi[i] = hi[i].max(b.1[i]);
    }
}

fn area(lo: V3, hi: V3) -> f32 {
    let d = sub(hi, lo);
    if d[0] < 0.0 {
        return 0.0;
    }
    2.0 * (d[0] * d[1] + d[1] * d[2] + d[2] * d[0])
}

impl Tree {
    pub fn new(tris: Vec<[V3; 3]>) -> Tree {
        let n = tris.len();
        let boxes: Vec<(V3, V3)> = tris.iter().map(tri_box).collect();
        let centres: Vec<V3> = boxes.iter().map(|b| scale(add(b.0, b.1), 0.5)).collect();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes = Vec::with_capacity(2 * n.max(1));
        if n > 0 {
            build(&mut nodes, &mut order, &boxes, &centres, 0, n);
        }
        Tree { tris, nodes, order }
    }

    /// The triangles of a mesh.
    pub fn of_mesh(m: &studio_mesh::Mesh) -> Tree {
        Tree::new((0..m.triangles()).map(|t| m.tri(t).map(|v| m.positions[v])).collect())
    }

    /// The nearest hit along a ray within `max_t` (direction need not be
    /// unit; `t` is in its units).
    pub fn ray(&self, origin: V3, dir: V3, max_t: f32) -> Option<Hit> {
        self.ray_impl(origin, dir, max_t, false)
    }

    /// Whether anything lies along the ray within `max_t`.
    pub fn occluded(&self, origin: V3, dir: V3, max_t: f32) -> bool {
        self.ray_impl(origin, dir, max_t, true).is_some()
    }

    fn ray_impl(&self, o: V3, d: V3, max_t: f32, any: bool) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut best: Option<Hit> = None;
        let mut limit = max_t;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp] as usize;
            let nd = &self.nodes[ni];
            if !slab(nd.lo, nd.hi, o, inv, limit) {
                continue;
            }
            if nd.count > 0 {
                for k in nd.index..nd.index + nd.count {
                    let ti = self.order[k as usize];
                    if let Some((t, u, v)) = ray_tri(o, d, &self.tris[ti as usize])
                        && t > 0.0
                        && t < limit
                    {
                        limit = t;
                        best = Some(Hit { t, tri: ti, uv: [u, v] });
                        if any {
                            return best;
                        }
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = nd.index;
                stack[sp + 1] = (ni + 1) as u32;
                sp += 2;
            }
        }
        best
    }

    /// Every crossing along a ray: (t, +1 leaving a surface, −1 entering),
    /// by the triangle's facing; nearest first.
    pub fn crossings(&self, o: V3, d: V3) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            return out;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp] as usize;
            let nd = &self.nodes[ni];
            if !slab(nd.lo, nd.hi, o, inv, f32::INFINITY) {
                continue;
            }
            if nd.count > 0 {
                for k in nd.index..nd.index + nd.count {
                    let tr = &self.tris[self.order[k as usize] as usize];
                    if let Some((t, _, _)) = ray_tri(o, d, tr)
                        && t > 0.0
                    {
                        let n = cross(sub(tr[1], tr[0]), sub(tr[2], tr[0]));
                        out.push((t, dot(n, d).signum()));
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = nd.index;
                stack[sp + 1] = (ni + 1) as u32;
                sp += 2;
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// The nearest point on the surface to `p` within `max_d`: the point,
    /// its triangle and the distance.
    pub fn nearest(&self, p: V3, max_d: f32) -> Option<(V3, u32, f32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(V3, u32, f32)> = None;
        let mut limit = max_d;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp] as usize;
            let nd = &self.nodes[ni];
            if box_distance(nd.lo, nd.hi, p) > limit {
                continue;
            }
            if nd.count > 0 {
                for k in nd.index..nd.index + nd.count {
                    let ti = self.order[k as usize];
                    let q = closest_on_tri(p, &self.tris[ti as usize]);
                    let d = length(sub(q, p));
                    if d < limit {
                        limit = d;
                        best = Some((q, ti, d));
                    }
                }
            } else if sp + 2 <= stack.len() {
                // Nearer child last (popped first).
                let (l, r) = ((ni + 1) as u32, nd.index);
                let dl = box_distance(self.nodes[l as usize].lo, self.nodes[l as usize].hi, p);
                let dr = box_distance(self.nodes[r as usize].lo, self.nodes[r as usize].hi, p);
                if dl < dr {
                    stack[sp] = r;
                    stack[sp + 1] = l;
                } else {
                    stack[sp] = l;
                    stack[sp + 1] = r;
                }
                sp += 2;
            }
        }
        best
    }

    /// Barycentric weights of a point on triangle `tri`.
    pub fn barycentric(&self, tri: u32, p: V3) -> [f32; 3] {
        barycentric(&self.tris[tri as usize], p)
    }
}

fn build(nodes: &mut Vec<Node>, order: &mut [u32], boxes: &[(V3, V3)], centres: &[V3], start: usize, end: usize) -> usize {
    let me = nodes.len();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    let mut clo = [f32::INFINITY; 3];
    let mut chi = [f32::NEG_INFINITY; 3];
    for &t in &order[start..end] {
        grow(&mut lo, &mut hi, boxes[t as usize]);
        grow(&mut clo, &mut chi, (centres[t as usize], centres[t as usize]));
    }
    nodes.push(Node { lo, hi, index: start as u32, count: (end - start) as u32 });
    let n = end - start;
    if n <= 4 {
        return me;
    }
    // Binned surface-area split on the widest axis of the centres.
    let ext = sub(chi, clo);
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
    if ext[axis] <= 1e-12 {
        return me;
    }
    const BINS: usize = 12;
    let mut bin_box = [([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]); BINS];
    let mut bin_n = [0usize; BINS];
    let bin_of = |c: V3| (((c[axis] - clo[axis]) / ext[axis] * BINS as f32) as usize).min(BINS - 1);
    for &t in &order[start..end] {
        let b = bin_of(centres[t as usize]);
        bin_n[b] += 1;
        let (l, h) = &mut bin_box[b];
        grow(l, h, boxes[t as usize]);
    }
    let mut best = (f32::INFINITY, 0usize);
    for split in 1..BINS {
        let (mut l1, mut h1, mut n1) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3], 0);
        let (mut l2, mut h2, mut n2) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3], 0);
        for b in 0..BINS {
            if bin_n[b] == 0 {
                continue;
            }
            if b < split {
                grow(&mut l1, &mut h1, bin_box[b]);
                n1 += bin_n[b];
            } else {
                grow(&mut l2, &mut h2, bin_box[b]);
                n2 += bin_n[b];
            }
        }
        if n1 == 0 || n2 == 0 {
            continue;
        }
        let cost = area(l1, h1) * n1 as f32 + area(l2, h2) * n2 as f32;
        if cost < best.0 {
            best = (cost, split);
        }
    }
    let mid = if best.0.is_finite() {
        let mut i = start;
        for j in start..end {
            if bin_of(centres[order[j] as usize]) < best.1 {
                order.swap(i, j);
                i += 1;
            }
        }
        i
    } else {
        start + n / 2
    };
    let mid = if mid == start || mid == end { start + n / 2 } else { mid };
    nodes[me].count = 0;
    build(nodes, order, boxes, centres, start, mid);
    let right = build(nodes, order, boxes, centres, mid, end);
    nodes[me].index = right as u32;
    me
}

fn slab(lo: V3, hi: V3, o: V3, inv: V3, max_t: f32) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = max_t;
    for i in 0..3 {
        let (a, b) = ((lo[i] - o[i]) * inv[i], (hi[i] - o[i]) * inv[i]);
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        if a.is_nan() || b.is_nan() {
            continue;
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return false;
        }
    }
    true
}

fn box_distance(lo: V3, hi: V3, p: V3) -> f32 {
    let mut d = 0.0f32;
    for i in 0..3 {
        let v = (lo[i] - p[i]).max(0.0).max(p[i] - hi[i]);
        d += v * v;
    }
    d.sqrt()
}

/// Möller–Trumbore: (t, u, v) of a ray meeting a triangle (both sides).
pub fn ray_tri(o: V3, d: V3, t: &[V3; 3]) -> Option<(f32, f32, f32)> {
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

/// The closest point of a triangle to `p` (Ericson's region test).
pub fn closest_on_tri(p: V3, t: &[V3; 3]) -> V3 {
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
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

pub fn barycentric(t: &[V3; 3], p: V3) -> [f32; 3] {
    let v0 = sub(t[1], t[0]);
    let v1 = sub(t[2], t[0]);
    let v2 = sub(p, t[0]);
    let (d00, d01, d11, d20, d21) = (dot(v0, v0), dot(v0, v1), dot(v1, v1), dot(v2, v0), dot(v2, v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-20 {
        return [1.0, 0.0, 0.0];
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    [1.0 - v - w, v, w]
}

/// A hash grid over points: what lies within a radius.
#[derive(Clone, Debug, Default)]
pub struct PointGrid {
    cell: f32,
    map: HashMap<[i32; 3], Vec<u32>>,
}

impl PointGrid {
    pub fn new(points: &[V3], cell: f32) -> PointGrid {
        let mut g = PointGrid { cell: cell.max(1e-6), map: HashMap::new() };
        for (i, p) in points.iter().enumerate() {
            g.insert(i as u32, *p);
        }
        g
    }

    fn key(&self, p: V3) -> [i32; 3] {
        [(p[0] / self.cell).floor() as i32, (p[1] / self.cell).floor() as i32, (p[2] / self.cell).floor() as i32]
    }

    pub fn insert(&mut self, i: u32, p: V3) {
        let k = self.key(p);
        self.map.entry(k).or_default().push(i);
    }

    pub fn remove(&mut self, i: u32, p: V3) {
        let k = self.key(p);
        if let Some(v) = self.map.get_mut(&k)
            && let Some(at) = v.iter().position(|&x| x == i)
        {
            v.swap_remove(at);
        }
    }

    /// Points within `r` of `p` (checking against `points`).
    pub fn within(&self, points: &[V3], p: V3, r: f32) -> Vec<u32> {
        let mut out = Vec::new();
        let (a, b) = (self.key(sub(p, [r; 3])), self.key(add(p, [r; 3])));
        let r2 = r * r;
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    if let Some(list) = self.map.get(&[x, y, z]) {
                        for &i in list {
                            let d = sub(points[i as usize], p);
                            if dot(d, d) <= r2 {
                                out.push(i);
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_and_nearest_points_on_a_sphere() {
        let m = studio_mesh::prim::ico_sphere(10.0, 3);
        let t = Tree::of_mesh(&m);
        let h = t.ray([0.0, 0.0, -50.0], [0.0, 0.0, 1.0], 1e9).unwrap();
        assert!((h.t - 40.0).abs() < 0.3, "{}", h.t);
        assert!(t.occluded([0.0, 0.0, -50.0], [0.0, 0.0, 1.0], 100.0));
        assert!(!t.occluded([0.0, 0.0, -50.0], [0.0, 1.0, 0.0], 100.0));
        let (q, _, d) = t.nearest([30.0, 0.0, 0.0], 1e9).unwrap();
        assert!((d - 20.0).abs() < 0.3 && (q[0] - 10.0).abs() < 0.3);
        // Brute force agrees.
        let p = [3.0, -14.0, 7.0];
        let brute = t.tris.iter().map(|tr| length(sub(closest_on_tri(p, tr), p))).fold(f32::INFINITY, f32::min);
        assert!((t.nearest(p, 1e9).unwrap().2 - brute).abs() < 1e-4);
    }

    #[test]
    fn grid_finds_neighbours() {
        let pts: Vec<V3> = (0..100).map(|i| [(i % 10) as f32, (i / 10) as f32, 0.0]).collect();
        let g = PointGrid::new(&pts, 2.0);
        assert_eq!(g.within(&pts, [5.0, 5.0, 0.0], 1.01).len(), 5);
    }
}
