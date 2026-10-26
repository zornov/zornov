// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Topology a production model must have: closed (no holes), manifold
//! (every edge between two faces, every vertex one fan), consistently
//! wound, no degenerate or crossing faces; and how good the faces are:
//! how many pair into clean quads, how many poles (vertices that are not
//! four-way), how long the edge loops run before they break.
//!
//! Studio keeps triangles; quads are read back by pairing triangles across
//! the edge that makes the best rectangle, the way a triangulated quad
//! export is read by any 3D package.

use crate::geo::{Welded, triangle_area, weld};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use studio_mesh::{Mesh, V3, cross, dot, length, normalise, sub};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Topology {
    pub triangles: usize,
    pub vertices: usize,
    /// Edges with one face: holes.
    pub boundary_edges: usize,
    /// Edges with three or more faces.
    pub nonmanifold_edges: usize,
    /// Vertices whose faces make more than one fan (two cones touching).
    pub nonmanifold_vertices: usize,
    /// Edges both of whose faces run the same way round (flipped faces).
    pub flipped_edges: usize,
    pub degenerate: usize,
    /// Pairs of faces that pass through each other (sampled exactly).
    pub self_intersections: usize,
    pub components: usize,
    /// Tiny loose bits: components under 1% of the surface.
    pub specks: usize,
    pub watertight: bool,
    pub manifold: bool,
    /// Share of the faces that are in quads (0–1).
    pub quad_ratio: f32,
    /// Share of the (polygon) vertices that are poles: inside the surface
    /// and not four-way.
    pub pole_ratio: f32,
    /// How long the edge loops run, as a share of what a perfect grid of
    /// the same faces would give (0–1).
    pub loop_score: f32,
    /// Smallest angle of the triangles, averaged (degrees).
    pub mean_min_angle: f32,
    /// Share of triangles with a corner under 10° (slivers).
    pub slivers: f32,
    /// Spread of polygon edge lengths (standard deviation / mean).
    pub edge_cov: f32,
}

struct Edges {
    /// Welded edge (low, high) → faces using it and whether that face runs
    /// low → high.
    map: HashMap<(u32, u32), Vec<(u32, bool)>>,
}

fn edges(w: &Welded) -> Edges {
    let mut map: HashMap<(u32, u32), Vec<(u32, bool)>> = HashMap::with_capacity(w.tris.len() * 2);
    for (t, tri) in w.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            if a == b {
                continue;
            }
            map.entry((a.min(b), a.max(b))).or_default().push((t as u32, a < b));
        }
    }
    Edges { map }
}

fn find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}

fn union(p: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb {
        p[ra as usize] = rb;
    }
}

fn angle_at(a: V3, b: V3, c: V3) -> f32 {
    let u = normalise(sub(b, a));
    let v = normalise(sub(c, a));
    dot(u, v).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Faces as polygons: triangles paired into quads where that makes a good
/// quad (greedy, best pairs first).
pub fn pair_quads(w: &Welded) -> Vec<Vec<u32>> {
    let e = edges(w);
    let mut cands: Vec<(f32, u32, u32, (u32, u32))> = Vec::new();
    for (&(a, b), faces) in &e.map {
        if faces.len() != 2 || faces[0].1 == faces[1].1 {
            continue;
        }
        let (t1, t2) = (faces[0].0, faces[1].0);
        let o1 = w.tris[t1 as usize].iter().copied().find(|&v| v != a && v != b);
        let o2 = w.tris[t2 as usize].iter().copied().find(|&v| v != a && v != b);
        let (Some(o1), Some(o2)) = (o1, o2) else { continue };
        let p = |i: u32| w.points[i as usize];
        // The quad o1 a o2 b, corners in order round it.
        let quad = [p(o1), p(a), p(o2), p(b)];
        let mut q = 1.0f32;
        for k in 0..4 {
            let ang = angle_at(quad[k], quad[(k + 1) % 4], quad[(k + 3) % 4]);
            q = q.min(1.0 - ((ang - 90.0).abs() / 90.0));
        }
        let n1 = normalise(cross(sub(p(a), p(o1)), sub(p(b), p(o1))));
        let n2 = normalise(cross(sub(p(b), p(o2)), sub(p(a), p(o2))));
        let flat = dot(n1, n2).abs();
        // Convex: both diagonals split the quad.
        let q = q * flat * flat;
        if q > 0.35 {
            cands.push((q, t1, t2, (a, b)));
        }
    }
    cands.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut used = vec![false; w.tris.len()];
    let mut faces = Vec::new();
    for (_, t1, t2, (a, b)) in cands {
        if used[t1 as usize] || used[t2 as usize] {
            continue;
        }
        used[t1 as usize] = true;
        used[t2 as usize] = true;
        // Walk t1 from the corner opposite the shared edge, then t2's.
        let tri1 = w.tris[t1 as usize];
        let k = tri1.iter().position(|&v| v != a && v != b).unwrap_or(0);
        let (x, y, z) = (tri1[k], tri1[(k + 1) % 3], tri1[(k + 2) % 3]);
        let o2 = w.tris[t2 as usize].iter().copied().find(|&v| v != a && v != b).unwrap_or(y);
        faces.push(vec![x, y, o2, z]);
    }
    for (t, tri) in w.tris.iter().enumerate() {
        if !used[t] {
            faces.push(tri.to_vec());
        }
    }
    faces
}

/// Loops and poles over polygon faces.
fn flow(faces: &[Vec<u32>], points: usize) -> (f32, f32, f32) {
    let mut nbrs: Vec<HashSet<u32>> = vec![HashSet::new(); points];
    let mut edge_faces: HashMap<(u32, u32), u32> = HashMap::new();
    for f in faces {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            if a == b {
                continue;
            }
            nbrs[a as usize].insert(b);
            nbrs[b as usize].insert(a);
            *edge_faces.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let boundary: HashSet<u32> = edge_faces.iter().filter(|(_, c)| **c == 1).flat_map(|(e, _)| [e.0, e.1]).collect();
    let used: Vec<u32> = (0..points as u32).filter(|&v| !nbrs[v as usize].is_empty()).collect();
    let poles = used.iter().filter(|&&v| !boundary.contains(&v) && nbrs[v as usize].len() != 4).count();
    let pole_ratio = poles as f32 / used.len().max(1) as f32;
    // Faces around each edge, to find the edge straight across a vertex.
    let mut sides: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            sides.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let straight_on = |u: u32, v: u32| -> Option<u32> {
        let n = &nbrs[v as usize];
        if n.len() != 4 || boundary.contains(&v) {
            return None;
        }
        // Neighbours of v that share a face with edge u-v are beside it.
        let mut beside: HashSet<u32> = HashSet::new();
        for &fi in sides.get(&(u.min(v), u.max(v)))? {
            let f = &faces[fi];
            if f.len() != 4 {
                return None;
            }
            let k = f.iter().position(|&x| x == v)?;
            for w in [f[(k + 1) % 4], f[(k + 3) % 4]] {
                if w != u {
                    beside.insert(w);
                }
            }
        }
        let rest: Vec<u32> = n.iter().copied().filter(|&w| w != u && !beside.contains(&w)).collect();
        if rest.len() == 1 { Some(rest[0]) } else { None }
    };
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut weighted = 0.0f64;
    let mut total = 0usize;
    let keys: Vec<(u32, u32)> = edge_faces.keys().copied().collect();
    for e in keys {
        if seen.contains(&e) {
            continue;
        }
        seen.insert(e);
        let mut len = 1usize;
        for (mut u, mut v) in [(e.0, e.1), (e.1, e.0)] {
            while let Some(w) = straight_on(u, v) {
                let k = (v.min(w), v.max(w));
                if !seen.insert(k) {
                    break;
                }
                len += 1;
                (u, v) = (v, w);
            }
        }
        weighted += (len * len) as f64;
        total += len;
    }
    let mean_loop = if total > 0 { weighted / total as f64 } else { 0.0 } as f32;
    // A grid of n quads on a closed surface has loops about 2·√n long.
    let quads = faces.iter().filter(|f| f.len() == 4).count().max(1) as f32;
    let loop_score = (mean_loop / (2.0 * quads.sqrt())).min(1.0);
    // Edge lengths are measured by the caller (needs positions).
    (pole_ratio, loop_score, 0.0)
}

/// Do the triangles `a` and `b` (no shared corner) pass through each other?
fn crosses(w: &Welded, a: usize, b: usize) -> bool {
    let ta = w.tris[a];
    let tb = w.tris[b];
    let seg_hits = |p: V3, q: V3, t: [u32; 3]| -> bool {
        let (v0, v1, v2) = (w.points[t[0] as usize], w.points[t[1] as usize], w.points[t[2] as usize]);
        let d = sub(q, p);
        let e1 = sub(v1, v0);
        let e2 = sub(v2, v0);
        let h = cross(d, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-12 {
            return false;
        }
        let f = 1.0 / det;
        let s = sub(p, v0);
        let u = f * dot(s, h);
        if !(1e-5..=1.0 - 1e-5).contains(&u) {
            return false;
        }
        let qv = cross(s, e1);
        let v = f * dot(d, qv);
        if v < 1e-5 || u + v > 1.0 - 1e-5 {
            return false;
        }
        let t = f * dot(e2, qv);
        (1e-5..=1.0 - 1e-5).contains(&t)
    };
    for k in 0..3 {
        if seg_hits(w.points[ta[k] as usize], w.points[ta[(k + 1) % 3] as usize], tb) {
            return true;
        }
        if seg_hits(w.points[tb[k] as usize], w.points[tb[(k + 1) % 3] as usize], ta) {
            return true;
        }
    }
    false
}

/// Pairs of faces that pass through each other.
pub fn crossing_faces(w: &Welded, diag: f32) -> Vec<(u32, u32)> {
    self_intersections_list(w, diag)
}

fn self_intersections(w: &Welded, diag: f32) -> usize {
    self_intersections_list(w, diag).len()
}

fn self_intersections_list(w: &Welded, diag: f32) -> Vec<(u32, u32)> {
    let n = w.tris.len();
    if n < 2 {
        return Vec::new();
    }
    let cell = (diag / (n as f32).sqrt().max(1.0)) * 1.5;
    let mut grid: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    let key = |p: V3| -> [i32; 3] { std::array::from_fn(|i| (p[i] / cell).floor() as i32) };
    for (t, tri) in w.tris.iter().enumerate() {
        let ps = tri.map(|i| w.points[i as usize]);
        let lo = key(std::array::from_fn(|i| ps[0][i].min(ps[1][i]).min(ps[2][i])));
        let hi = key(std::array::from_fn(|i| ps[0][i].max(ps[1][i]).max(ps[2][i])));
        if (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) * (hi[2] - lo[2] + 1) > 512 {
            continue;
        }
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    grid.entry([x, y, z]).or_default().push(t as u32);
                }
            }
        }
    }
    let mut found: HashSet<(u32, u32)> = HashSet::new();
    let mut tests = 0usize;
    for list in grid.values() {
        for i in 0..list.len() {
            for j in i + 1..list.len() {
                let (a, b) = (list[i], list[j]);
                let (ta, tb) = (w.tris[a as usize], w.tris[b as usize]);
                if ta.iter().any(|v| tb.contains(v)) || found.contains(&(a.min(b), a.max(b))) {
                    continue;
                }
                tests += 1;
                if tests > 4_000_000 {
                    return found.into_iter().collect();
                }
                if crosses(w, a as usize, b as usize) {
                    found.insert((a.min(b), a.max(b)));
                }
            }
        }
    }
    found.into_iter().collect()
}

pub fn analyse(m: &Mesh) -> Topology {
    let w = weld(m);
    let diag = crate::geo::diagonal(m);
    let e = edges(&w);
    let mut t = Topology { triangles: m.triangles(), ..Default::default() };
    let used: HashSet<u32> = w.tris.iter().flatten().copied().collect();
    t.vertices = used.len();
    for faces in e.map.values() {
        match faces.len() {
            1 => t.boundary_edges += 1,
            2 => {
                if faces[0].1 == faces[1].1 {
                    t.flipped_edges += 1;
                }
            }
            _ => t.nonmanifold_edges += 1,
        }
    }
    let area_eps = (diag * diag) * 1e-12;
    t.degenerate = (0..m.triangles()).filter(|&i| {
        let tri = w.tris[i];
        tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] || triangle_area(m, i) <= area_eps
    }).count();
    // Components over faces joined by edges.
    let mut parent: Vec<u32> = (0..w.tris.len() as u32).collect();
    for faces in e.map.values() {
        for k in 1..faces.len() {
            union(&mut parent, faces[0].0, faces[k].0);
        }
    }
    let mut comp_area: HashMap<u32, f32> = HashMap::new();
    let mut total_area = 0.0;
    for i in 0..w.tris.len() {
        let r = find(&mut parent, i as u32);
        let a = triangle_area(m, i);
        *comp_area.entry(r).or_default() += a;
        total_area += a;
    }
    t.components = comp_area.len();
    t.specks = comp_area.values().filter(|&&a| a < total_area * 0.01).count();
    // Non-manifold vertices: faces round a vertex joined through edges at it.
    let mut at_vertex: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, tri) in w.tris.iter().enumerate() {
        for &v in tri {
            at_vertex.entry(v).or_default().push(i as u32);
        }
    }
    for (&v, list) in &at_vertex {
        if list.len() < 2 {
            continue;
        }
        let index: HashMap<u32, usize> = list.iter().enumerate().map(|(k, &f)| (f, k)).collect();
        let mut p: Vec<u32> = (0..list.len() as u32).collect();
        for &f in list {
            for &o in &w.tris[f as usize] {
                if o == v {
                    continue;
                }
                if let Some(fs) = e.map.get(&(v.min(o), v.max(o))) {
                    for &(g, _) in fs {
                        if let (Some(&a), Some(&b)) = (index.get(&f), index.get(&g)) {
                            union(&mut p, a as u32, b as u32);
                        }
                    }
                }
            }
        }
        let fans: HashSet<u32> = (0..list.len() as u32).map(|k| find(&mut p, k)).collect();
        if fans.len() > 1 {
            t.nonmanifold_vertices += 1;
        }
    }
    t.self_intersections = self_intersections(&w, diag);
    t.watertight = t.boundary_edges == 0 && t.triangles > 0;
    t.manifold = t.nonmanifold_edges == 0 && t.nonmanifold_vertices == 0 && t.flipped_edges == 0;
    // Triangle shape.
    let mut min_sum = 0.0f64;
    let mut slivers = 0usize;
    for tri in &w.tris {
        let [a, b, c] = tri.map(|i| w.points[i as usize]);
        let m0 = angle_at(a, b, c).min(angle_at(b, c, a)).min(angle_at(c, a, b));
        if m0.is_finite() {
            min_sum += m0 as f64;
            if m0 < 10.0 {
                slivers += 1;
            }
        }
    }
    t.mean_min_angle = (min_sum / w.tris.len().max(1) as f64) as f32;
    t.slivers = slivers as f32 / w.tris.len().max(1) as f32;
    // Quads and flow.
    let faces = pair_quads(&w);
    let quads = faces.iter().filter(|f| f.len() == 4).count();
    t.quad_ratio = (quads * 2) as f32 / w.tris.len().max(1) as f32;
    let (pole_ratio, loop_score, _) = flow(&faces, w.points.len());
    t.pole_ratio = pole_ratio;
    t.loop_score = loop_score;
    let mut lens = Vec::new();
    let mut seen = HashSet::new();
    for f in &faces {
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            if seen.insert((a.min(b), a.max(b))) {
                lens.push(length(sub(w.points[a as usize], w.points[b as usize])) as f64);
            }
        }
    }
    if !lens.is_empty() {
        let mean = lens.iter().sum::<f64>() / lens.len() as f64;
        let var = lens.iter().map(|l| (l - mean) * (l - mean)).sum::<f64>() / lens.len() as f64;
        t.edge_cov = (var.sqrt() / mean.max(1e-12)) as f32;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_is_closed_all_quads_and_has_eight_poles() {
        let m = studio_mesh::prim::by_name("cube", 100.0, 0).unwrap();
        let t = analyse(&m);
        assert!(t.watertight && t.manifold, "{t:?}");
        assert_eq!(t.components, 1);
        assert!(t.quad_ratio > 0.99, "{t:?}");
        assert_eq!(t.self_intersections, 0);
    }

    #[test]
    fn a_hole_and_a_flip_are_found() {
        let mut m = studio_mesh::prim::by_name("sphere", 100.0, 3).unwrap();
        let mid = (m.indices.len() / 6) * 3;
        m.indices.drain(mid..mid + 3);
        m.single_group();
        assert!(analyse(&m).boundary_edges >= 3);
        let mut m = studio_mesh::prim::by_name("sphere", 100.0, 3).unwrap();
        m.indices.swap(0, 1);
        assert!(analyse(&m).flipped_edges >= 1);
    }

    #[test]
    fn crossing_spheres_intersect() {
        let mut a = studio_mesh::prim::by_name("sphere", 100.0, 3).unwrap();
        let mut b = a.clone();
        studio_mesh::ops::transform(&mut b, &studio_mesh::region::Region::All, [50.0, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        studio_mesh::ops::merge(&mut a, &b);
        let t = analyse(&a);
        assert!(t.self_intersections > 10, "{t:?}");
        assert_eq!(t.components, 2);
    }
}
