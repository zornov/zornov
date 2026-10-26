// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Retopology: any model rebuilt as one closed, manifold, all-quad
//! surface with about the number of faces asked for.
//!
//! 1. A grid sized so the surface crosses about the target number of grid
//!    edges; the distance to the original is measured in a band round it,
//!    and inside / outside comes from flooding the grid from its border
//!    (so self-crossing parts, doubled walls and small holes in the
//!    original all melt into one solid).
//! 2. Dual contouring: a vertex in every cell the surface passes through,
//!    placed where the original's planes there meet (a quadric fit to the
//!    crossing points and the original's normals), so hard edges and
//!    corners stay sharp; a quad across every grid edge the surface
//!    crosses, so every face is a quad.
//! 3. Cells where two sheets meet (a thin part narrower than a cell) get
//!    one vertex per sheet, so the result stays manifold.
//! 4. Vertices on smooth parts are relaxed along the surface (even quads)
//!    and put back on the original; corners and creases stay put.
//! 5. Where the original is mirror-symmetric, the grid is centred on the
//!    mirror plane and the result is made exactly symmetric, so rigs and
//!    sculpting with mirror work on it.
//!
//! Faces take the material of the original part they lie on.

use crate::geo::{Nearest, surface_area};
use std::collections::HashMap;
use studio_mesh::{Group, Mesh, V3, add, cross, dot, length, normalise, scale, sub};

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Quads wanted.
    pub quads: usize,
    /// Make the result mirror-symmetric in x (about the model's centre)
    /// when the original is.
    pub symmetry: bool,
    /// Relaxation rounds on smooth parts.
    pub relax: usize,
    /// Normals: faces meeting at more than this many degrees get a hard edge.
    pub crease: f32,
}

impl Default for Params {
    fn default() -> Params {
        Params { quads: 8_000, symmetry: true, relax: 4, crease: 50.0 }
    }
}

struct Grid {
    origin: V3,
    h: f32,
    n: [usize; 3],
}

impl Grid {
    fn at(&self, i: usize, j: usize, k: usize) -> V3 {
        [self.origin[0] + i as f32 * self.h, self.origin[1] + j as f32 * self.h, self.origin[2] + k as f32 * self.h]
    }
    fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.n[1] + j) * self.n[0] + i
    }
}

/// Is the original symmetric about x = `cx`? (Mirrored samples land on it.)
pub fn is_symmetric(near: &Nearest, cx: f32) -> bool {
    let m = near.mesh;
    let diag = crate::geo::diagonal(m);
    let samples = crate::geo::sample_surface(m, 400, 0x3c6e_f372);
    let mut far = 0;
    for (p, _) in &samples {
        let q = [2.0 * cx - p[0], p[1], p[2]];
        if near.find(q).is_none_or(|h| h.distance > diag * 0.004) {
            far += 1;
        }
    }
    far * 100 <= samples.len() * 2
}

/// Solves a 3×3 system (symmetric), returning None if singular.
fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let x = (b[0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (b[1] * a[2][2] - a[1][2] * b[2]) + a[0][2] * (b[1] * a[2][1] - a[1][1] * b[2])) * inv;
    let y = (a[0][0] * (b[1] * a[2][2] - a[1][2] * b[2]) - b[0] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * b[2] - b[1] * a[2][0])) * inv;
    let z = (a[0][0] * (a[1][1] * b[2] - b[1] * a[2][1]) - a[0][1] * (a[1][0] * b[2] - b[1] * a[2][0]) + b[0] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])) * inv;
    Some([x, y, z])
}

/// Where planes (points + normals) meet, pulled towards `mass` where they
/// leave a direction free (a flat area, a crease).
fn qef(points: &[V3], normals: &[V3], mass: V3) -> V3 {
    let mut a = [[0f64; 3]; 3];
    let mut b = [0f64; 3];
    // A small pull to the mass point keeps flat and creased cells stable.
    let reg = 0.05f64;
    for (p, n) in points.iter().zip(normals) {
        let d = dot(*n, sub(*p, mass)) as f64;
        for r in 0..3 {
            for c in 0..3 {
                a[r][c] += n[r] as f64 * n[c] as f64;
            }
            b[r] += n[r] as f64 * d;
        }
    }
    for (r, row) in a.iter_mut().enumerate() {
        row[r] += reg;
    }
    match solve3(a, b) {
        Some(x) => [mass[0] + x[0] as f32, mass[1] + x[1] as f32, mass[2] + x[2] as f32],
        None => mass,
    }
}

const CORNERS: [[usize; 3]; 8] = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];
/// Cube edges as corner pairs.
const EDGES: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];

struct Built {
    grid: Grid,
    /// Grid points inside the solid.
    inside: Vec<bool>,
    pos: Vec<V3>,
    nrm: Vec<V3>,
    quads: Vec<[u32; 4]>,
    /// Grid cell of each vertex (for symmetry).
    cell: Vec<[usize; 3]>,
    /// Corners and creases, which relaxation leaves alone.
    feature: Vec<bool>,
}

fn build(src: &Mesh, near: &Nearest, h: f32, mirror_x: Option<f32>) -> Built {
    let (lo, hi) = src.bounds();
    let pad = 2.0 * h;
    let mut n: [usize; 3] = std::array::from_fn(|i| (((hi[i] - lo[i]) + 2.0 * pad) / h).ceil() as usize + 1);
    let mut origin: V3 = std::array::from_fn(|i| lo[i] - pad);
    if let Some(cx) = mirror_x {
        // Grid points at cx ± (k + ½)h: the mirror maps the grid onto itself.
        if n[0] % 2 == 1 {
            n[0] += 1;
        }
        origin[0] = cx - (n[0] as f32 / 2.0 - 0.5) * h;
    }
    let g = Grid { origin, h, n };
    let total = n[0] * n[1] * n[2];
    // Unsigned distance in a band round the triangles.
    let mut dist = vec![f32::INFINITY; total];
    let mut sign_hint = vec![0i8; total];
    {
        let mut band = vec![false; total];
        for t in 0..src.triangles() {
            let [a, b, c] = src.tri(t);
            let (pa, pb, pc) = (src.positions[a], src.positions[b], src.positions[c]);
            let lo_t: [usize; 3] = std::array::from_fn(|i| ((((pa[i].min(pb[i]).min(pc[i])) - g.origin[i]) / h).floor() as isize - 1).max(0) as usize);
            let hi_t: [usize; 3] = std::array::from_fn(|i| ((((pa[i].max(pb[i]).max(pc[i])) - g.origin[i]) / h).ceil() as usize + 1).min(n[i] - 1));
            let nrm = normalise(cross(sub(pb, pa), sub(pc, pa)));
            for k in lo_t[2]..=hi_t[2] {
                for j in lo_t[1]..=hi_t[1] {
                    for i in lo_t[0]..=hi_t[0] {
                        // Only points near the triangle's plane.
                        let p = g.at(i, j, k);
                        if dot(sub(p, pa), nrm).abs() <= h * 1.8 {
                            band[g.idx(i, j, k)] = true;
                        }
                    }
                }
            }
        }
        let list: Vec<usize> = (0..total).filter(|&i| band[i]).collect();
        let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 12);
        let chunk = list.len().div_ceil(threads).max(1);
        let results: Vec<Vec<(usize, f32, i8)>> = std::thread::scope(|s| {
            let hs: Vec<_> = list
                .chunks(chunk)
                .map(|part| {
                    let g = &g;
                    s.spawn(move || {
                        part.iter()
                            .map(|&id| {
                                let (i, j, k) = (id % n[0], (id / n[0]) % n[1], id / (n[0] * n[1]));
                                let p = g.at(i, j, k);
                                match near.find(p) {
                                    Some(hit) => {
                                        let fnrm = near.mesh.face_normal(hit.triangle);
                                        let side = dot(sub(p, hit.point), fnrm);
                                        (id, hit.distance, if side > 0.0 { 1 } else if side < 0.0 { -1 } else { 0 })
                                    }
                                    None => (id, f32::INFINITY, 0),
                                }
                            })
                            .collect()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        for part in results {
            for (id, d, s) in part {
                dist[id] = d;
                sign_hint[id] = s;
            }
        }
    }
    // Outside: flood from the border through points not right at the surface.
    let wall = h * 0.75;
    let mut outside = vec![false; total];
    let mut stack: Vec<usize> = Vec::new();
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                if (i == 0 || j == 0 || k == 0 || i == n[0] - 1 || j == n[1] - 1 || k == n[2] - 1) && dist[g.idx(i, j, k)] > wall {
                    let id = g.idx(i, j, k);
                    if !outside[id] {
                        outside[id] = true;
                        stack.push(id);
                    }
                }
            }
        }
    }
    while let Some(id) = stack.pop() {
        let (i, j, k) = (id % n[0], (id / n[0]) % n[1], id / (n[0] * n[1]));
        let mut push = |ii: usize, jj: usize, kk: usize| {
            let o = g.idx(ii, jj, kk);
            if !outside[o] && dist[o] > wall {
                outside[o] = true;
                stack.push(o);
            }
        };
        if i > 0 {
            push(i - 1, j, k);
        }
        if i + 1 < n[0] {
            push(i + 1, j, k);
        }
        if j > 0 {
            push(i, j - 1, k);
        }
        if j + 1 < n[1] {
            push(i, j + 1, k);
        }
        if k > 0 {
            push(i, j, k - 1);
        }
        if k + 1 < n[2] {
            push(i, j, k + 1);
        }
    }
    // Wall points the nearest face puts outside, next to the flooded
    // outside, are outside too (a second flood through them); wall points
    // deep inside (round faces buried in another part) stay inside.
    for id in 0..total {
        if outside[id] {
            stack.push(id);
        }
    }
    while let Some(id) = stack.pop() {
        let (i, j, k) = (id % n[0], (id / n[0]) % n[1], id / (n[0] * n[1]));
        let around = [(i > 0).then(|| g.idx(i - 1, j, k)), (i + 1 < n[0]).then(|| g.idx(i + 1, j, k)), (j > 0).then(|| g.idx(i, j - 1, k)), (j + 1 < n[1]).then(|| g.idx(i, j + 1, k)), (k > 0).then(|| g.idx(i, j, k - 1)), (k + 1 < n[2]).then(|| g.idx(i, j, k + 1))];
        for o in around.into_iter().flatten() {
            if !outside[o] && dist[o] <= wall && sign_hint[o] > 0 {
                outside[o] = true;
                stack.push(o);
            }
        }
    }
    // Signed values.
    let inside: Vec<bool> = (0..total)
        .map(|id| {
            if outside[id] {
                false
            } else if dist[id] <= wall {
                // At the surface and not reached from outside.
                true
            } else {
                true
            }
        })
        .collect();
    let val = |i: usize, j: usize, k: usize| -> f32 {
        let id = g.idx(i, j, k);
        let d = if dist[id].is_finite() { dist[id] } else { h * 3.0 };
        if inside[id] { -d.max(1e-6) } else { d.max(1e-6) }
    };
    // A vertex per surface sheet in each crossed cell (dual contouring).
    let cells = (n[0] - 1) * (n[1] - 1) * (n[2] - 1);
    let cid = |i: usize, j: usize, k: usize| (k * (n[1] - 1) + j) * (n[0] - 1) + i;
    // Cell → (first vertex, how many) and which vertex each crossed edge uses.
    let mut cell_first = vec![u32::MAX; cells];
    let mut edge_vertex: HashMap<(usize, u8), u32> = HashMap::new();
    let mut out = Built { grid: Grid { origin: g.origin, h: g.h, n: g.n }, inside: Vec::new(), pos: Vec::new(), nrm: Vec::new(), quads: Vec::new(), cell: Vec::new(), feature: Vec::new() };
    for k in 0..n[2] - 1 {
        for j in 0..n[1] - 1 {
            for i in 0..n[0] - 1 {
                let corner_in: [bool; 8] = std::array::from_fn(|c| val(i + CORNERS[c][0], j + CORNERS[c][1], k + CORNERS[c][2]) < 0.0);
                if corner_in.iter().all(|&b| b) || corner_in.iter().all(|&b| !b) {
                    continue;
                }
                // Sheets: inside corners joined along cube edges (one vertex per
                // connected inside group keeps two touching parts apart).
                let mut group = [usize::MAX; 8];
                let mut groups = 0usize;
                for c in 0..8 {
                    if !corner_in[c] || group[c] != usize::MAX {
                        continue;
                    }
                    let mut st = vec![c];
                    group[c] = groups;
                    while let Some(x) = st.pop() {
                        for &(a, b) in &EDGES {
                            let o = if a == x { b } else if b == x { a } else { continue };
                            if corner_in[o] && group[o] == usize::MAX {
                                group[o] = groups;
                                st.push(o);
                            }
                        }
                    }
                    groups += 1;
                }
                // Outside corners can also split into groups; split by the side
                // with fewer groups would be ideal, the inside groups suffice
                // for thin parts (the common case).
                let first = out.pos.len() as u32;
                cell_first[cid(i, j, k)] = first;
                for gi in 0..groups {
                    let mut pts = Vec::new();
                    let mut nrms = Vec::new();
                    for (e, &(a, b)) in EDGES.iter().enumerate() {
                        if corner_in[a] == corner_in[b] {
                            continue;
                        }
                        let ins = if corner_in[a] { a } else { b };
                        if group[ins] != gi {
                            continue;
                        }
                        let (ca, cb) = (CORNERS[a], CORNERS[b]);
                        let (va, vb) = (val(i + ca[0], j + ca[1], k + ca[2]), val(i + cb[0], j + cb[1], k + cb[2]));
                        let t = (va / (va - vb)).clamp(0.0, 1.0);
                        let pa = g.at(i + ca[0], j + ca[1], k + ca[2]);
                        let pb = g.at(i + cb[0], j + cb[1], k + cb[2]);
                        let guess = studio_mesh::lerp(pa, pb, t);
                        let (p, nn) = match near.find(guess) {
                            Some(hit) if hit.distance < h * 1.5 => (hit.point, near.mesh.face_normal(hit.triangle)),
                            _ => (guess, normalise(sub(pb, pa))),
                        };
                        pts.push(p);
                        nrms.push(nn);
                        edge_vertex.insert((cid(i, j, k), e as u8), first + gi as u32);
                    }
                    let mass = scale(pts.iter().fold([0.0; 3], |s, p| add(s, *p)), 1.0 / pts.len().max(1) as f32);
                    let mut v = qef(&pts, &nrms, mass);
                    // Keep it in (or very near) its cell.
                    let cmin = g.at(i, j, k);
                    for d in 0..3 {
                        v[d] = v[d].clamp(cmin[d] - h * 0.25, cmin[d] + h * 1.25);
                    }
                    let spread = nrms.iter().map(|a| nrms.iter().map(|b| dot(*a, *b)).fold(1.0, f32::min)).fold(1.0, f32::min);
                    out.pos.push(v);
                    out.nrm.push(normalise(nrms.iter().fold([0.0; 3], |s, x| add(s, *x))));
                    out.cell.push([i, j, k]);
                    out.feature.push(spread < 0.6);
                }
            }
        }
    }
    // A quad across every crossed grid edge, from the four cells round it.
    let mut quads: Vec<[u32; 4]> = Vec::new();
    for k in 1..n[2] - 1 {
        for j in 1..n[1] - 1 {
            for i in 1..n[0] - 1 {
                let here = val(i, j, k) < 0.0;
                for axis in 0..3 {
                    let (di, dj, dk) = [(1, 0, 0), (0, 1, 0), (0, 0, 1)][axis];
                    if i + di >= n[0] || j + dj >= n[1] || k + dk >= n[2] {
                        continue;
                    }
                    if here == (val(i + di, j + dj, k + dk) < 0.0) {
                        continue;
                    }
                    // The four cells round edge (i,j,k)→+axis and, in each, which
                    // cube edge this grid edge is.
                    let ring: [([usize; 3], usize); 4] = match axis {
                        // x edges: cube edges 0..4 by (y, z) corner offset.
                        0 => [([i, j - 1, k - 1], 3), ([i, j, k - 1], 2), ([i, j, k], 0), ([i, j - 1, k], 1)],
                        1 => [([i - 1, j, k - 1], 7), ([i - 1, j, k], 5), ([i, j, k], 4), ([i, j, k - 1], 6)],
                        _ => [([i - 1, j - 1, k], 11), ([i, j - 1, k], 10), ([i, j, k], 8), ([i - 1, j, k], 9)],
                    };
                    let mut q = [u32::MAX; 4];
                    for (s, (c, e)) in ring.iter().enumerate() {
                        if let Some(&v) = edge_vertex.get(&(cid(c[0], c[1], c[2]), *e as u8)) {
                            q[s] = v;
                        }
                    }
                    if q.contains(&u32::MAX) {
                        continue;
                    }
                    quads.push(if here { q } else { [q[3], q[2], q[1], q[0]] });
                }
            }
        }
    }
    out.quads = quads;
    out.inside = inside;
    out
}

/// The original without its buried faces (inside another part, where
/// parts overlap): what vertices may be put back onto.
fn shell(src: &Mesh, b: &Built) -> Mesh {
    let g = &b.grid;
    let mut keep = Vec::new();
    let mats = src.tri_materials();
    let mut km = Vec::new();
    for t in 0..src.triangles() {
        let p = add(src.centroid(t), scale(src.face_normal(t), g.h * 0.6));
        let ijk: [usize; 3] = std::array::from_fn(|i| (((p[i] - g.origin[i]) / g.h).round().max(0.0) as usize).min(g.n[i] - 1));
        if !b.inside[g.idx(ijk[0], ijk[1], ijk[2])] {
            keep.extend_from_slice(&src.indices[3 * t..3 * t + 3]);
            km.push(mats[t]);
        }
    }
    let mut m = src.clone();
    m.indices = keep;
    m.regroup(&km);
    m
}

/// Quad neighbours (along quad edges, not diagonals) and vertex normals.
fn neighbours(pos: &[V3], quads: &[[u32; 4]]) -> (Vec<Vec<u32>>, Vec<V3>) {
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    let mut nrm = vec![[0.0f32; 3]; pos.len()];
    for q in quads {
        let fnrm = cross(sub(pos[q[2] as usize], pos[q[0] as usize]), sub(pos[q[3] as usize], pos[q[1] as usize]));
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            if !nb[a as usize].contains(&b) {
                nb[a as usize].push(b);
            }
            if !nb[b as usize].contains(&a) {
                nb[b as usize].push(a);
            }
            nrm[a as usize] = add(nrm[a as usize], fnrm);
        }
    }
    (nb, nrm.into_iter().map(normalise).collect())
}

/// Even quads on smooth parts: each vertex moved towards its neighbours'
/// centre along the surface, then put back on the original.
fn relax(pos: &mut [V3], quads: &[[u32; 4]], feature: &[bool], near: &Nearest, rounds: usize, project_all: bool) {
    for round in 0..rounds.max(usize::from(project_all)) {
        let (nb, nrm) = neighbours(pos, quads);
        let next: Vec<V3> = (0..pos.len())
            .map(|v| {
                let p = pos[v];
                let moved = if round < rounds && !feature[v] && !nb[v].is_empty() {
                    let c = scale(nb[v].iter().fold([0.0; 3], |s, &o| add(s, pos[o as usize])), 1.0 / nb[v].len() as f32);
                    let d = sub(c, p);
                    add(p, scale(sub(d, scale(nrm[v], dot(d, nrm[v]))), 0.6))
                } else {
                    p
                };
                near.find(moved).map_or(moved, |h| h.point)
            })
            .collect();
        pos.copy_from_slice(&next);
    }
}

/// One Catmull–Clark step over quads. Sharp edges (marked in `sharp`, a
/// set of vertex pairs) stay creases; vertices with three or more sharp
/// edges stay corners.
fn catmull_clark(pos: &[V3], quads: &[[u32; 4]], feature: &[bool], sharp: &std::collections::HashSet<(u32, u32)>) -> (Vec<V3>, Vec<[u32; 4]>, Vec<bool>, std::collections::HashSet<(u32, u32)>) {
    let nv = pos.len();
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (f, q) in quads.iter().enumerate() {
        for k in 0..4 {
            edge_faces.entry(key(q[k], q[(k + 1) % 4])).or_default().push(f);
        }
    }
    let face_pt: Vec<V3> = quads.iter().map(|q| scale(q.iter().fold([0.0; 3], |s, &v| add(s, pos[v as usize])), 0.25)).collect();
    let mut out_pos: Vec<V3> = Vec::with_capacity(nv + edge_faces.len() + quads.len());
    out_pos.extend_from_slice(pos);
    let mut out_feat: Vec<bool> = feature.to_vec();
    let mut edge_id: HashMap<(u32, u32), u32> = HashMap::new();
    let mut edges: Vec<(u32, u32)> = edge_faces.keys().copied().collect();
    edges.sort();
    for &e in &edges {
        let fs = &edge_faces[&e];
        let mid = scale(add(pos[e.0 as usize], pos[e.1 as usize]), 0.5);
        let p = if sharp.contains(&e) || fs.len() != 2 {
            mid
        } else {
            scale(add(add(pos[e.0 as usize], pos[e.1 as usize]), add(face_pt[fs[0]], face_pt[fs[1]])), 0.25)
        };
        edge_id.insert(e, out_pos.len() as u32);
        out_pos.push(p);
        out_feat.push(sharp.contains(&e));
    }
    let face_base = out_pos.len() as u32;
    out_pos.extend_from_slice(&face_pt);
    out_feat.extend(std::iter::repeat_n(false, quads.len()));
    // Moved original vertices.
    let mut faces_at: Vec<Vec<usize>> = vec![Vec::new(); nv];
    let mut edges_at: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nv];
    for (f, q) in quads.iter().enumerate() {
        for &v in q {
            faces_at[v as usize].push(f);
        }
    }
    for &e in &edges {
        edges_at[e.0 as usize].push(e);
        edges_at[e.1 as usize].push(e);
    }
    for v in 0..nv {
        let p = pos[v];
        let sharp_here: Vec<(u32, u32)> = edges_at[v].iter().copied().filter(|e| sharp.contains(e) || edge_faces[e].len() != 2).collect();
        out_pos[v] = match sharp_here.len() {
            0 => {
                let n = faces_at[v].len() as f32;
                if n == 0.0 {
                    p
                } else {
                    let f = scale(faces_at[v].iter().fold([0.0; 3], |s, &fi| add(s, face_pt[fi])), 1.0 / n);
                    let r = scale(edges_at[v].iter().fold([0.0; 3], |s, e| add(s, scale(add(pos[e.0 as usize], pos[e.1 as usize]), 0.5))), 1.0 / edges_at[v].len().max(1) as f32);
                    scale(add(add(f, scale(r, 2.0)), scale(p, n - 3.0)), 1.0 / n)
                }
            }
            2 => {
                let o: Vec<V3> = sharp_here.iter().map(|e| pos[if e.0 as usize == v { e.1 } else { e.0 } as usize]).collect();
                add(scale(p, 0.75), scale(add(o[0], o[1]), 0.125))
            }
            _ => p,
        };
    }
    let mut out_quads = Vec::with_capacity(quads.len() * 4);
    let mut out_sharp = std::collections::HashSet::new();
    for (f, q) in quads.iter().enumerate() {
        let fc = face_base + f as u32;
        for k in 0..4 {
            let (prev, v, next) = (q[(k + 3) % 4], q[k], q[(k + 1) % 4]);
            let e_in = edge_id[&key(prev, v)];
            let e_out = edge_id[&key(v, next)];
            out_quads.push([v, e_out, fc, e_in]);
        }
    }
    for &e in &edges {
        if sharp.contains(&e) {
            let m = edge_id[&e];
            out_sharp.insert(key(e.0, m));
            out_sharp.insert(key(m, e.1));
        }
    }
    (out_pos, out_quads, out_feat, out_sharp)
}

/// Edges where the surface turns sharply (faces meeting at more than
/// `degrees`) between two corner/crease vertices.
fn sharp_edges(pos: &[V3], quads: &[[u32; 4]], feature: &[bool], degrees: f32) -> std::collections::HashSet<(u32, u32)> {
    let cos = degrees.to_radians().cos();
    let mut by_edge: HashMap<(u32, u32), Vec<V3>> = HashMap::new();
    for q in quads {
        let n = normalise(cross(sub(pos[q[2] as usize], pos[q[0] as usize]), sub(pos[q[3] as usize], pos[q[1] as usize])));
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(n);
        }
    }
    by_edge.into_iter().filter(|(e, ns)| ns.len() == 2 && feature[e.0 as usize] && feature[e.1 as usize] && dot(ns[0], ns[1]) < cos).map(|(e, _)| e).collect()
}

/// A quad as two triangles, split along its shorter diagonal.
fn split(pos: &[V3], q: &[u32; 4]) -> [[u32; 3]; 2] {
    let p = |v: u32| pos[v as usize];
    if length(sub(p(q[0]), p(q[2]))) <= length(sub(p(q[1]), p(q[3]))) { [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] } else { [[q[0], q[1], q[3]], [q[1], q[2], q[3]]] }
}

/// Faces that pass through each other (where parts meet in a tight
/// crease): the vertices round them smoothed in place until they clear.
fn untangle(pos: &mut [V3], quads: &[[u32; 4]]) {
    for _ in 0..12 {
        let tris: Vec<[u32; 3]> = quads.iter().flat_map(|q| split(pos, q)).collect();
        let (lo, hi) = pos.iter().fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(l, h), p| (std::array::from_fn(|i| l[i].min(p[i])), std::array::from_fn(|i| h[i].max(p[i]))));
        let w = crate::geo::Welded { tris: tris.clone(), points: pos.to_vec(), ids: (0..pos.len() as u32).collect() };
        let bad = crate::topo::crossing_faces(&w, length(sub(hi, lo)));
        if bad.is_empty() {
            return;
        }
        let mut hot = vec![false; pos.len()];
        for (a, b) in bad {
            for v in tris[a as usize].iter().chain(&tris[b as usize]) {
                hot[*v as usize] = true;
            }
        }
        let (nb, _) = neighbours(pos, quads);
        // And their neighbours, so the fold relaxes over a little area.
        let ring: Vec<bool> = (0..pos.len()).map(|v| hot[v] || nb[v].iter().any(|&o| hot[o as usize])).collect();
        for _ in 0..3 {
            let old = pos.to_vec();
            for v in 0..pos.len() {
                if ring[v] && !nb[v].is_empty() {
                    let c = scale(nb[v].iter().fold([0.0; 3], |s, &o| add(s, old[o as usize])), 1.0 / nb[v].len() as f32);
                    pos[v] = studio_mesh::lerp(old[v], c, 0.5);
                }
            }
        }
    }
}

/// Exact mirror symmetry about x = `cx`: each vertex averaged with the
/// vertex nearest its mirror image; vertices on the plane put on it.
fn symmetrise(pos: &mut [V3], cx: f32, tol: f32) {
    let cell = tol.max(1e-4);
    let mut grid: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    let key = |p: V3| -> [i32; 3] { std::array::from_fn(|i| (p[i] / cell).floor() as i32) };
    for (v, p) in pos.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(v as u32);
    }
    let old = pos.to_vec();
    for (v, p) in old.iter().enumerate() {
        let m = [2.0 * cx - p[0], p[1], p[2]];
        let k = key(m);
        let mut best: Option<(f32, u32)> = None;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    for &o in grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]).into_iter().flatten() {
                        let d = length(sub(old[o as usize], m));
                        if d < tol && best.is_none_or(|b| d < b.0) {
                            best = Some((d, o));
                        }
                    }
                }
            }
        }
        if let Some((_, o)) = best {
            let q = old[o as usize];
            pos[v] = if o as usize == v { [cx, p[1], p[2]] } else { [(p[0] + 2.0 * cx - q[0]) * 0.5, (p[1] + q[1]) * 0.5, (p[2] + q[2]) * 0.5] };
        }
    }
}

/// Rebuilds `src` as a closed all-quad model of about `p.quads` faces:
/// a cage of `p.quads / 4^levels` quads from the voxel field, subdivided
/// `levels` times (Catmull–Clark, creases kept) and put back on the
/// original after every step. Subdividing makes every new vertex
/// four-way, so the result has few poles and long, clean edge loops.
pub fn remesh_levels(src: &Mesh, p: &Params, levels: usize) -> Mesh {
    if src.triangles() == 0 {
        return src.clone();
    }
    let near = Nearest::new(src);
    let (lo, hi) = src.bounds();
    let cx = (lo[0] + hi[0]) * 0.5;
    let mirror = (p.symmetry && is_symmetric(&near, cx)).then_some(cx);
    let area = surface_area(src).max(1e-6);
    let cage_quads = (p.quads.max(8) as f32 / 4f32.powi(levels as i32)).max(6.0);
    let mut h = (area / cage_quads).sqrt();
    let mut best: Option<Built> = None;
    for _ in 0..4 {
        let b = build(src, &near, h, mirror);
        let quads = b.quads.len();
        let ratio = quads as f32 / cage_quads;
        best = Some(b);
        if (0.8..=1.15).contains(&ratio) || quads == 0 {
            break;
        }
        h *= ratio.sqrt();
    }
    let Some(b) = best else { return src.clone() };
    let outer = shell(src, &b);
    let on = if outer.triangles() > 0 { Nearest::new(&outer) } else { Nearest::new(src) };
    let near = on;
    let (mut pos, mut quads, mut feature) = (b.pos, b.quads, b.feature);
    relax(&mut pos, &quads, &feature, &near, p.relax, true);
    let mut sharp = sharp_edges(&pos, &quads, &feature, p.crease);
    for _ in 0..levels {
        let (np, nq, nf, ns) = catmull_clark(&pos, &quads, &feature, &sharp);
        (pos, quads, feature, sharp) = (np, nq, nf, ns);
        relax(&mut pos, &quads, &feature, &near, (p.relax / 2).max(1), true);
    }
    untangle(&mut pos, &quads);
    if let Some(cx) = mirror {
        let edge = (area / quads.len().max(1) as f32).sqrt();
        symmetrise(&mut pos, cx, edge * 0.4);
    }
    // Triangles: each quad split along its shorter diagonal.
    let mut m = Mesh::default();
    m.positions = pos;
    for q in &quads {
        let tris = split(&m.positions, q);
        for t in tris {
            m.indices.extend_from_slice(&t);
        }
    }
    // Materials from the original part under each face.
    m.materials = if src.materials.is_empty() { vec![Default::default()] } else { src.materials.clone() };
    let src_mats = src.tri_materials();
    let mats: Vec<u32> = (0..m.triangles()).map(|t| near.find(m.centroid(t)).map_or(0, |hit| src_mats.get(hit.triangle).copied().unwrap_or(0))).collect();
    m.normals = vec![[0.0; 3]; m.positions.len()];
    m.uvs = vec![[0.0; 2]; m.positions.len()];
    m.groups = vec![Group { start: 0, count: m.triangles() as u32, material: 0 }];
    m.regroup(&mats);
    studio_mesh::ops::smooth_normals_in_place(&mut m);
    m
}

/// How far a rebuilt surface strays from the original under it: the
/// worst distance (99th percentile, as a share of the original's
/// diagonal) and the share of its surface turned more than 30° from the
/// original's (torn rims, folds, ripples). Measured one way, from the
/// rebuilt surface onto the original, so inner walls the rebuild melts
/// away on purpose don't count against it; facing is ignored, so an
/// original with flipped faces is judged the same.
fn stray(m: &Mesh, near: &Nearest) -> (f32, f32) {
    let diag = crate::geo::diagonal(near.mesh).max(1e-6);
    let (mut d, mut torn, mut n) = (Vec::new(), 0usize, 0usize);
    for (p, t) in crate::geo::sample_surface(m, 2000, 0x6a09_e667) {
        let Some(h) = near.find(p) else { continue };
        d.push(h.distance);
        let (a, b) = (m.face_normal(t), near.mesh.face_normal(h.triangle));
        if dot(a, a) < 0.5 || dot(b, b) < 0.5 {
            continue;
        }
        torn += usize::from(dot(a, b).abs() < 30f32.to_radians().cos());
        n += 1;
    }
    d.sort_by(f32::total_cmp);
    let p99 = d.get((d.len() as f32 * 0.99) as usize).or(d.last()).copied().unwrap_or(0.0);
    (p99 / diag, torn as f32 / n.max(1) as f32)
}

/// The worst surface error and torn share a rebuild may have (the same
/// bars the critics hold a finished model to).
const WORST_ERROR: f32 = 0.02;
const TORN: f32 = 0.03;

/// [`remesh_levels`] at the subdivision depth the model's measured
/// fidelity allows. Two steps give the cleanest edge loops, but the cage
/// is then coarse and can't hold thin walls, rims or small parts; so
/// each depth's result, if closed and in one piece, is measured against
/// the original and the deepest one within the bars is kept. If none is,
/// the one closest to them is.
pub fn remesh(src: &Mesh, p: &Params) -> Mesh {
    if src.triangles() == 0 {
        return src.clone();
    }
    let near = Nearest::new(src);
    let parts = crate::topo::analyse(src).components.max(1);
    let debug = std::env::var_os("REMESH_DEBUG").is_some();
    let mut best: Option<(f32, Mesh)> = None;
    let mut fallback = None;
    for levels in [2, 1, 0] {
        let m = remesh_levels(src, p, levels);
        let t = crate::topo::analyse(&m);
        if !(t.watertight && t.manifold && t.components <= parts) {
            if debug {
                eprintln!("levels {levels}: not closed {t:?}");
            }
            fallback.get_or_insert(m);
            continue;
        }
        let (worst, torn) = stray(&m, &near);
        let over = (worst / WORST_ERROR).max(torn / TORN);
        if debug {
            eprintln!("levels {levels}: worst error {worst:.4}, torn {torn:.3}");
        }
        if over <= 1.0 {
            return m;
        }
        if best.as_ref().is_none_or(|(b, _)| over < *b) {
            best = Some((over, m));
        }
    }
    best.map(|(_, m)| m).or(fallback).unwrap_or_else(|| src.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::analyse;

    #[test]
    fn a_sphere_becomes_closed_quads_near_the_budget() {
        let s = studio_mesh::prim::by_name("sphere", 200.0, 5).unwrap();
        let m = remesh(&s, &Params { quads: 1500, ..Default::default() });
        let t = analyse(&m);
        assert!(t.watertight && t.manifold, "{t:?}");
        assert!(t.quad_ratio > 0.95, "{t:?}");
        let q = m.triangles() / 2;
        assert!((1000..=2000).contains(&q), "{q} quads");
        let f = crate::views::fidelity(&m, &s);
        assert!(f.chamfer < 0.004, "{f:?}");
    }

    #[test]
    fn a_cube_keeps_its_corners() {
        let c = studio_mesh::prim::by_name("cube", 200.0, 0).unwrap();
        let m = remesh(&c, &Params { quads: 600, ..Default::default() });
        let (lo, hi) = m.bounds();
        assert!((hi[0] - lo[0] - 200.0).abs() < 2.0 && (hi[1] - lo[1] - 200.0).abs() < 2.0, "{lo:?} {hi:?}");
        // A corner vertex sits on the cube's corner.
        let corner = m.positions.iter().map(|p| length(sub(*p, [100.0, 100.0, 100.0]))).fold(f32::INFINITY, f32::min);
        assert!(corner < 4.0, "nearest to the corner {corner}");
        let t = analyse(&m);
        assert!(t.watertight && t.manifold, "{t:?}");
    }

    #[test]
    fn a_thin_wall_keeps_its_shape() {
        // A plate thinner than a coarse cage's cells: the depth is chosen
        // so the plate isn't torn or rippled.
        let mut c = studio_mesh::prim::by_name("cube", 200.0, 0).unwrap();
        studio_mesh::ops::transform(&mut c, &studio_mesh::region::Region::All, [0.0; 3], [0.0; 3], [1.0, 1.0, 0.1], [0.0; 3]);
        let m = remesh(&c, &Params { quads: 1200, ..Default::default() });
        let t = analyse(&m);
        assert!(t.watertight && t.manifold, "{t:?}");
        let (worst, torn) = stray(&m, &Nearest::new(&c));
        assert!(worst <= WORST_ERROR && torn <= TORN, "worst {worst}, torn {torn}");
    }

    #[test]
    fn overlapping_parts_melt_into_one_surface() {
        let mut a = studio_mesh::prim::by_name("sphere", 200.0, 4).unwrap();
        let mut b = a.clone();
        studio_mesh::ops::transform(&mut b, &studio_mesh::region::Region::All, [90.0, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        studio_mesh::ops::merge(&mut a, &b);
        let m = remesh(&a, &Params { quads: 2000, ..Default::default() });
        let t = analyse(&m);
        assert_eq!(t.components, 1, "{t:?}");
        assert!(t.watertight && t.manifold && t.self_intersections == 0, "{t:?}");
        assert!(crate::views::symmetry(&m) > 0.97);
    }
}
