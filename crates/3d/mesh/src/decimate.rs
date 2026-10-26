// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fewer triangles with the shape kept: edge collapses ordered by quadric
//! error (Garland and Heckbert). Open edges are held in place, collapses
//! that would turn a face over are refused, and texture coordinates stay
//! with their corners.

use crate::{Mesh, V3, cross, dot, normalise, scale, sub};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// A symmetric 4×4 error matrix, upper triangle.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: V3, d: f32, w: f64) -> Quadric {
        let (a, b, c, d) = (n[0] as f64, n[1] as f64, n[2] as f64, d as f64);
        Quadric([a * a * w, a * b * w, a * c * w, a * d * w, b * b * w, b * c * w, b * d * w, c * c * w, c * d * w, d * d * w])
    }
    fn add(&self, o: &Quadric) -> Quadric {
        Quadric(std::array::from_fn(|i| self.0[i] + o.0[i]))
    }
    fn error(&self, p: V3) -> f64 {
        let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let q = &self.0;
        q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z + 2.0 * q[6] * y + q[7] * z * z + 2.0 * q[8] * z + q[9]
    }
    /// The point of least error, when the matrix can be solved.
    fn best(&self) -> Option<V3> {
        let q = &self.0;
        let m = [[q[0], q[1], q[2]], [q[1], q[4], q[5]], [q[2], q[5], q[7]]];
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < 1e-9 {
            return None;
        }
        let b = [-q[3], -q[6], -q[8]];
        let solve = |col: usize| {
            let mut mm = m;
            for r in 0..3 {
                mm[r][col] = b[r];
            }
            (mm[0][0] * (mm[1][1] * mm[2][2] - mm[1][2] * mm[2][1]) - mm[0][1] * (mm[1][0] * mm[2][2] - mm[1][2] * mm[2][0]) + mm[0][2] * (mm[1][0] * mm[2][1] - mm[1][1] * mm[2][0])) / det
        };
        Some([solve(0) as f32, solve(1) as f32, solve(2) as f32])
    }
}

struct Candidate {
    cost: f64,
    a: u32,
    b: u32,
    stamp: (u32, u32),
    to: V3,
}

impl PartialEq for Candidate {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Candidate {
    // Least cost first out of the (max-)heap.
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}

/// Collapses edges until about `target` triangles remain (or nothing more
/// can go without damage).
pub fn decimate(m: &mut Mesh, target: usize) {
    if m.triangles() <= target.max(4) {
        return;
    }
    let (ids, mut pos) = m.position_ids();
    let np = pos.len();
    let mats = m.tri_materials();
    let tri_pid = |t: usize, ids: &[u32], m: &Mesh| m.tri(t).map(|v| ids[v]);
    let mut q = vec![Quadric::default(); np];
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); np];
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for t in 0..m.triangles() {
        let p = tri_pid(t, &ids, m);
        let raw = cross(sub(pos[p[1] as usize], pos[p[0] as usize]), sub(pos[p[2] as usize], pos[p[0] as usize]));
        let n = normalise(raw);
        let area = (dot(raw, raw).sqrt() * 0.5) as f64;
        let k = Quadric::plane(n, -dot(n, pos[p[0] as usize]), area.max(1e-6));
        for &v in &p {
            q[v as usize] = q[v as usize].add(&k);
            around[v as usize].push(t);
        }
        for (x, y) in [(p[0], p[1]), (p[1], p[2]), (p[2], p[0])] {
            *edges.entry((x.min(y), x.max(y))).or_default() += 1;
        }
    }
    // Open edges: a steep plane through them keeps them from wandering.
    for (&(x, y), &n) in &edges {
        if n != 1 {
            continue;
        }
        let t = around[x as usize].iter().copied().find(|&t| tri_pid(t, &ids, m).contains(&y));
        if let Some(t) = t {
            let e = sub(pos[y as usize], pos[x as usize]);
            let side = normalise(cross(e, m.face_normal(t)));
            let w = dot(e, e) as f64 * 10.0;
            let k = Quadric::plane(side, -dot(side, pos[x as usize]), w.max(1e-3));
            q[x as usize] = q[x as usize].add(&k);
            q[y as usize] = q[y as usize].add(&k);
        }
    }
    let mut rep: Vec<u32> = (0..np as u32).collect();
    let mut stamp = vec![0u32; np];
    let mut dead = vec![false; m.triangles()];
    let mut live = m.triangles();
    let candidate = |a: u32, b: u32, q: &[Quadric], pos: &[V3], stamp: &[u32]| -> Candidate {
        let s = q[a as usize].add(&q[b as usize]);
        let mid = scale(crate::add(pos[a as usize], pos[b as usize]), 0.5);
        let mut options = vec![pos[a as usize], pos[b as usize], mid];
        if let Some(p) = s.best() {
            // Only trust the solved point near the edge.
            let len = crate::length(sub(pos[a as usize], pos[b as usize]));
            if crate::length(sub(p, mid)) < len * 2.0 {
                options.push(p);
            }
        }
        let (to, cost) = options.into_iter().map(|p| (p, s.error(p))).min_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(Ordering::Equal)).expect("options");
        Candidate { cost, a, b, stamp: (stamp[a as usize], stamp[b as usize]), to }
    };
    let mut heap = BinaryHeap::new();
    for &(a, b) in edges.keys() {
        heap.push(candidate(a, b, &q, &pos, &stamp));
    }
    let find = |rep: &mut Vec<u32>, mut x: u32| {
        while rep[x as usize] != x {
            rep[x as usize] = rep[rep[x as usize] as usize];
            x = rep[x as usize];
        }
        x
    };
    while live > target {
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.a, c.b);
        if rep[a as usize] != a || rep[b as usize] != b || stamp[a as usize] != c.stamp.0 || stamp[b as usize] != c.stamp.1 {
            continue;
        }
        // Would any surviving face turn over?
        let mut flips = false;
        let current = |t: usize, rep: &mut Vec<u32>| m.tri(t).map(|v| find(rep, ids[v]));
        let mut touched: Vec<usize> = around[a as usize].iter().chain(&around[b as usize]).copied().filter(|&t| !dead[t]).collect();
        touched.sort_unstable();
        touched.dedup();
        for &t in &touched {
            let p = current(t, &mut rep);
            if p.contains(&a) && p.contains(&b) {
                continue;
            }
            let before = cross(sub(pos[p[1] as usize], pos[p[0] as usize]), sub(pos[p[2] as usize], pos[p[0] as usize]));
            let moved = p.map(|v| if v == a || v == b { c.to } else { pos[v as usize] });
            let after = cross(sub(moved[1], moved[0]), sub(moved[2], moved[0]));
            if dot(normalise(before), normalise(after)) < 0.2 {
                flips = true;
                break;
            }
        }
        if flips {
            continue;
        }
        rep[b as usize] = a;
        pos[a as usize] = c.to;
        q[a as usize] = q[a as usize].add(&q[b as usize]);
        stamp[a as usize] += 1;
        let mut keep = Vec::new();
        for &t in &touched {
            let p = current(t, &mut rep);
            if p[0] == p[1] || p[1] == p[2] || p[0] == p[2] {
                dead[t] = true;
                live -= 1;
            } else {
                keep.push(t);
            }
        }
        around[a as usize] = keep;
        around[b as usize].clear();
        let mut next: HashSet<u32> = HashSet::new();
        for &t in &around[a as usize] {
            for v in current(t, &mut rep) {
                if v != a {
                    next.insert(v);
                }
            }
        }
        for n in next {
            heap.push(candidate(a.min(n), a.max(n), &q, &pos, &stamp));
        }
    }
    let mut keep_mats = Vec::new();
    let mut idx = Vec::new();
    for t in 0..m.triangles() {
        if !dead[t] {
            idx.extend_from_slice(&m.indices[3 * t..3 * t + 3]);
            keep_mats.push(mats[t]);
        }
    }
    for v in 0..m.positions.len() {
        let r = find(&mut rep, ids[v]);
        m.positions[v] = pos[r as usize];
    }
    m.indices = idx;
    m.regroup(&keep_mats);
    crate::ops::compact(m);
    crate::ops::smooth_normals_in_place(m);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn a_sphere_keeps_its_shape_with_a_tenth_of_the_triangles() {
        let mut m = prim::ico_sphere(100.0, 5);
        let n = m.triangles();
        decimate(&mut m, n / 10);
        assert!(m.triangles() <= n / 10 + 4, "{} of {n}", m.triangles());
        for p in &m.positions {
            let r = crate::length(*p);
            assert!((r - 100.0).abs() < 6.0, "radius {r}");
        }
    }
}
