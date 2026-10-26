// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Polygon modeling: extrude, inset, loop cut, knife and bisect, bridge,
//! merge, dissolve, fill, delete, symmetry, soft-selection moves and the
//! small tools between them. Each works on a [`Poly`] in place and keeps
//! faces wound outward.

use crate::poly::{Edge, Poly, Topology, edge_slot, ekey, triangulate};
use crate::select::edge_ring;
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// For each vertex of a face pick, a copy; the picked faces moved onto the
/// copies and side quads stitched along the pick's rim. Returns the
/// old → new vertex map.
fn split_region(p: &mut Poly, pick: &[bool]) -> HashMap<u32, u32> {
    let t = Topology::of(p);
    let mut map: HashMap<u32, u32> = HashMap::new();
    for f in 0..p.faces() {
        if pick[f] {
            for v in p.face(f).to_vec() {
                if !map.contains_key(&v) {
                    let q = p.positions[v as usize];
                    map.insert(v, p.add_vertex(q));
                }
            }
        }
    }
    // Rim edges in the direction the picked face runs them.
    let mut rim: Vec<(u32, u32, usize)> = Vec::new();
    for f in 0..p.faces() {
        if !pick[f] {
            continue;
        }
        let c = p.face(f).to_vec();
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            let faces = &t.edge_faces[&ekey(a, b)];
            let picked = faces.iter().filter(|&&g| pick[g as usize]).count();
            if picked == 1 {
                rim.push((a, b, f));
            }
        }
    }
    for f in 0..p.faces() {
        if pick[f] {
            let r = p.face_range(f);
            for k in r {
                p.corners[k] = map[&p.corners[k]];
            }
        }
    }
    for (a, b, f) in rim {
        let (m, s, sm) = (p.face_material[f], p.face_set[f], p.face_smooth[f]);
        let uv = p.face_uvs(f).map(|_| [[0.0f32; 2]; 4]);
        p.add_face_like(&[a, b, map[&b], map[&a]], uv.as_ref().map(|u| &u[..]), m, s, sm);
        // Keep the edge's crease on the new rim of the cap.
        if let Some(&w) = p.creases.get(&ekey(a, b)) {
            p.creases.insert(ekey(map[&a], map[&b]), w);
        }
    }
    map
}

/// Area-weighted normal of the picked faces around each vertex.
fn pick_normals(p: &Poly, pick: &[bool]) -> HashMap<u32, (V3, f32)> {
    let mut acc: HashMap<u32, (V3, Vec<V3>)> = HashMap::new();
    for f in 0..p.faces() {
        if pick[f] {
            let n = p.face_normal(f);
            for &v in p.face(f) {
                let e = acc.entry(v).or_insert(([0.0; 3], Vec::new()));
                e.0 = add(e.0, n);
                e.1.push(n);
            }
        }
    }
    acc.into_iter()
        .map(|(v, (s, ns))| {
            let n = normalise(s);
            // Even thickness: a vertex between faces at an angle moves
            // further so each face moves by the full distance.
            let worst = ns.iter().map(|m| dot(*m, n)).fold(1.0f32, f32::min).max(0.25);
            (v, (n, 1.0 / worst))
        })
        .collect()
}

/// Extrudes the picked faces `distance` along their normals (or along
/// `direction` when given), as one region or each face on its own.
/// Returns the moved (cap) faces' numbers.
pub fn extrude(p: &mut Poly, pick: &[bool], distance: f32, direction: Option<V3>, individual: bool) -> Vec<u32> {
    let caps: Vec<u32> = (0..p.faces()).filter(|&f| pick[f]).map(|f| f as u32).collect();
    if caps.is_empty() {
        return caps;
    }
    let groups: Vec<Vec<bool>> = if individual {
        caps.iter().map(|&f| (0..p.faces()).map(|g| g == f as usize).collect()).collect()
    } else {
        vec![pick.to_vec()]
    };
    for g in groups {
        let mut g = g;
        g.resize(p.faces(), false);
        let normals = pick_normals(p, &g);
        let map = split_region(p, &g);
        for (old, new) in map {
            let off = match direction {
                Some(d) => scale(normalise(d), distance),
                None => {
                    let (n, k) = normals[&old];
                    scale(n, distance * k)
                }
            };
            p.positions[new as usize] = add(p.positions[old as usize], off);
        }
    }
    caps
}

/// Insets the picked faces: a new rim `thickness` wide inside their
/// outline, the inner part raised by `depth` (negative sinks it).
pub fn inset(p: &mut Poly, pick: &[bool], thickness: f32, depth: f32, individual: bool) -> Vec<u32> {
    let caps: Vec<u32> = (0..p.faces()).filter(|&f| pick[f]).map(|f| f as u32).collect();
    let groups: Vec<Vec<bool>> = if individual {
        caps.iter().map(|&f| (0..p.faces()).map(|g| g == f as usize).collect()).collect()
    } else {
        vec![pick.to_vec()]
    };
    for g in groups {
        let mut g = g;
        g.resize(p.faces(), false);
        let t = Topology::of(p);
        // Rim edges (as the picked faces run them) and each rim vertex's
        // inward direction.
        let mut inward: HashMap<u32, Vec<(V3, V3)>> = HashMap::new();
        for f in 0..p.faces() {
            if !g[f] {
                continue;
            }
            let n = p.face_normal(f);
            let c = p.face(f);
            for i in 0..c.len() {
                let (a, b) = (c[i], c[(i + 1) % c.len()]);
                let picked = t.edge_faces[&ekey(a, b)].iter().filter(|&&h| g[h as usize]).count();
                if picked == 1 {
                    let d = normalise(sub(p.positions[b as usize], p.positions[a as usize]));
                    let inn = normalise(cross(n, d));
                    inward.entry(a).or_default().push((inn, d));
                    inward.entry(b).or_default().push((inn, d));
                }
            }
        }
        let normals = pick_normals(p, &g);
        let map = split_region(p, &g);
        for (old, new) in map {
            let base = p.positions[old as usize];
            let mut q = base;
            if let Some(list) = inward.get(&old) {
                let mut s = [0.0; 3];
                for (i, _) in list {
                    s = add(s, *i);
                }
                let dir = normalise(s);
                let k = list.iter().map(|(i, _)| dot(*i, dir)).fold(1.0f32, f32::min).max(0.2);
                q = add(q, scale(dir, thickness / k));
            }
            if depth != 0.0 {
                let (n, _) = normals[&old];
                q = add(q, scale(n, depth));
            }
            p.positions[new as usize] = q;
        }
    }
    caps
}

/// Splits the given edges: each gets `cuts` new vertices (at `ts` along
/// it from the edge's first to its second vertex), threaded into every
/// face that uses it. Returns the new vertices per edge.
fn split_edges(p: &mut Poly, ts: &HashMap<Edge, Vec<f32>>) -> HashMap<Edge, Vec<u32>> {
    let mut made: HashMap<Edge, Vec<u32>> = HashMap::new();
    for (e, list) in ts {
        let (a, b) = (p.positions[e.0 as usize], p.positions[e.1 as usize]);
        let ids = list.iter().map(|&t| p.add_vertex(lerp(a, b, t))).collect();
        made.insert(*e, ids);
    }
    made
}

/// Rebuilds face `f` with split points threaded into its edges.
fn thread(p: &Poly, f: usize, made: &HashMap<Edge, Vec<u32>>, ts: &HashMap<Edge, Vec<f32>>) -> (Vec<u32>, Option<Vec<[f32; 2]>>) {
    let c = p.face(f);
    let uv = p.face_uvs(f);
    let mut vs = Vec::new();
    let mut us = Vec::new();
    for i in 0..c.len() {
        let (a, b) = (c[i], c[(i + 1) % c.len()]);
        vs.push(a);
        if let Some(u) = uv {
            us.push(u[i]);
        }
        let e = ekey(a, b);
        if let (Some(ids), Some(t)) = (made.get(&e), ts.get(&e)) {
            let forward = a == e.0;
            let order: Vec<usize> = if forward { (0..ids.len()).collect() } else { (0..ids.len()).rev().collect() };
            for k in order {
                vs.push(ids[k]);
                if let Some(u) = uv {
                    let tt = if forward { t[k] } else { 1.0 - t[k] };
                    let (ua, ub) = (u[i], u[(i + 1) % c.len()]);
                    us.push([ua[0] + (ub[0] - ua[0]) * tt, ua[1] + (ub[1] - ua[1]) * tt]);
                }
            }
        }
    }
    (vs, uv.map(|_| us))
}

/// Cuts `cuts` loops across the ring of quads through `edge`, evenly
/// spaced (`slide` −1…1 moves them toward one side). Returns the new
/// edges running along the loops.
pub fn loop_cut(p: &mut Poly, edge: Edge, cuts: usize, slide: f32) -> Vec<Edge> {
    let cuts = cuts.max(1);
    let t = Topology::of(p);
    let ring = edge_ring(p, &t, edge);
    if ring.is_empty() {
        return Vec::new();
    }
    // Orient every ring edge: which end is "start" (one side of the cut).
    let mut start_of: HashMap<Edge, u32> = HashMap::new();
    start_of.insert(ring[0].0, ring[0].0.0);
    let ring_faces: Vec<u32> = ring.iter().filter_map(|(_, f)| *f).collect();
    let mut ring_set: HashSet<u32> = ring_faces.iter().copied().collect();
    // The faces on both sides of the first edge are in the ring too.
    for &f in t.edge_faces.get(&ring[0].0).map(Vec::as_slice).unwrap_or(&[]) {
        if p.face(f as usize).len() == 4 {
            ring_set.insert(f);
        }
    }
    // Propagate starts across quads until stable.
    let mut changed = true;
    while changed {
        changed = false;
        for &f in &ring_set {
            let c = p.face(f as usize);
            for s in 0..4 {
                let e = ekey(c[s], c[(s + 1) % 4]);
                let opp = ekey(c[(s + 2) % 4], c[(s + 3) % 4]);
                if let Some(&st) = start_of.get(&e)
                    && !start_of.contains_key(&opp)
                    && ring.iter().any(|(r, _)| *r == opp)
                {
                    // c[s] pairs with c[s+3]; c[s+1] with c[s+2].
                    let partner = if st == c[s] { c[(s + 3) % 4] } else { c[(s + 2) % 4] };
                    start_of.insert(opp, partner);
                    changed = true;
                }
            }
        }
    }
    let k = slide.clamp(-0.95, 0.95);
    let fracs: Vec<f32> = (1..=cuts)
        .map(|i| {
            let t = i as f32 / (cuts + 1) as f32;
            if k >= 0.0 { t + (1.0 - t) * k * t } else { t + t * k * (1.0 - t) }
        })
        .collect();
    let mut ts: HashMap<Edge, Vec<f32>> = HashMap::new();
    for (e, _) in &ring {
        let st = start_of.get(e).copied().unwrap_or(e.0);
        let list: Vec<f32> = if st == e.0 { fracs.clone() } else { fracs.iter().map(|f| 1.0 - f).collect() };
        // Stored from e.0 toward e.1, ascending.
        let mut l = list;
        l.sort_by(f32::total_cmp);
        ts.insert(*e, l);
    }
    let made = split_edges(p, &ts);
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    let has_uv = p.has_uvs();
    let mut new_edges = Vec::new();
    // Points along an edge from vertex `from`.
    let along = |e: Edge, from: u32| -> Vec<u32> {
        let ids = made[&e].clone();
        if from == e.0 { ids } else { ids.into_iter().rev().collect() }
    };
    for f in 0..p.faces() {
        let c = p.face(f).to_vec();
        let (m, s, sm) = (p.face_material[f], p.face_set[f], p.face_smooth[f]);
        let split_slots: Vec<usize> = (0..c.len()).filter(|&i| made.contains_key(&ekey(c[i], c[(i + 1) % c.len()]))).collect();
        if c.len() == 4 && split_slots.len() == 2 && split_slots[1] == split_slots[0] + 2 {
            let s0 = split_slots[0];
            let q = [c[s0], c[(s0 + 1) % 4], c[(s0 + 2) % 4], c[(s0 + 3) % 4]];
            let uvq = p.face_uvs(f).map(|u| [u[s0], u[(s0 + 1) % 4], u[(s0 + 2) % 4], u[(s0 + 3) % 4]]);
            let a: Vec<u32> = std::iter::once(q[0]).chain(along(ekey(q[0], q[1]), q[0])).chain(std::iter::once(q[1])).collect();
            let b: Vec<u32> = std::iter::once(q[3]).chain(along(ekey(q[3], q[2]), q[3])).chain(std::iter::once(q[2])).collect();
            let n = a.len() - 1;
            for i in 0..n {
                let quad = [a[i], a[i + 1], b[i + 1], b[i]];
                let uv = uvq.map(|u| {
                    let (t0, t1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                    let l = |x: [f32; 2], y: [f32; 2], t: f32| [x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t];
                    [l(u[0], u[1], t0), l(u[0], u[1], t1), l(u[3], u[2], t1), l(u[3], u[2], t0)]
                });
                out.add_face_like(&quad, uv.as_ref().map(|x| &x[..]), m, s, sm);
                if i + 1 < n {
                    new_edges.push(ekey(a[i + 1], b[i + 1]));
                }
            }
        } else {
            let (vs, us) = thread(p, f, &made, &ts);
            out.add_face_like(&vs, us.as_deref(), m, s, sm);
        }
    }
    if !has_uv {
        out.uvs.clear();
    }
    // Split edges pass their marks to their halves.
    for (e, ids) in &made {
        let chain: Vec<u32> = std::iter::once(e.0).chain(ids.iter().copied()).chain(std::iter::once(e.1)).collect();
        let halves: Vec<Edge> = chain.windows(2).map(|w| ekey(w[0], w[1])).collect();
        if let Some(w) = out.creases.remove(e) {
            for h in &halves {
                out.creases.insert(*h, w);
            }
        }
        if out.sharp.remove(e) {
            out.sharp.extend(halves.iter().copied());
        }
        if out.seams.remove(e) {
            out.seams.extend(halves.iter().copied());
        }
    }
    *p = out;
    new_edges.sort_unstable();
    new_edges.dedup();
    new_edges
}

/// A plane: a point on it and its normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub at: V3,
    pub normal: V3,
}

impl Plane {
    pub fn side(&self, p: V3) -> f32 {
        dot(sub(p, self.at), normalise(self.normal))
    }
}

/// Cuts the picked faces along a plane (new edges where it crosses);
/// `within` limits the cut to points it says yes to (a knife stroke's
/// reach). Returns the new edges.
pub fn knife(p: &mut Poly, pick: &[bool], plane: Plane, within: &dyn Fn(V3) -> bool) -> Vec<Edge> {
    let eps = 1e-4 * {
        let (lo, hi) = p.bounds();
        length(sub(hi, lo)).max(1.0)
    };
    let d: Vec<f32> = p.positions.iter().map(|&x| plane.side(x)).collect();
    // Edge crossing points (strictly inside the edge).
    let mut ts: HashMap<Edge, Vec<f32>> = HashMap::new();
    for f in 0..p.faces() {
        if !pick.get(f).copied().unwrap_or(false) {
            continue;
        }
        let c = p.face(f);
        for i in 0..c.len() {
            let e = ekey(c[i], c[(i + 1) % c.len()]);
            let (da, db) = (d[e.0 as usize], d[e.1 as usize]);
            if (da > eps && db < -eps) || (da < -eps && db > eps) {
                let t = da / (da - db);
                let x = lerp(p.positions[e.0 as usize], p.positions[e.1 as usize], t);
                if within(x) {
                    ts.insert(e, vec![t]);
                }
            }
        }
    }
    let made = split_edges(p, &ts);
    let has_uv = p.has_uvs();
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    let side = |out: &Poly, v: u32| plane.side(out.positions[v as usize]);
    let mut new_edges = Vec::new();
    for f in 0..p.faces() {
        let (vs, us) = thread(p, f, &made, &ts);
        let (m, s, sm) = (p.face_material[f], p.face_set[f], p.face_smooth[f]);
        let picked = pick.get(f).copied().unwrap_or(false);
        // Corners on the plane, in order round the face.
        let on: Vec<usize> = (0..vs.len()).filter(|&i| side(&out, vs[i]).abs() <= eps && within(out.positions[vs[i] as usize])).collect();
        let n = vs.len();
        let usable = picked && on.len() >= 2;
        if !usable {
            out.add_face_like(&vs, us.as_deref(), m, s, sm);
            continue;
        }
        // Split into pieces between consecutive on-plane corners that
        // aren't neighbours already.
        let mut pieces: Vec<(Vec<u32>, Vec<[f32; 2]>)> = vec![(vs.clone(), us.clone().unwrap_or_default())];
        let pairs: Vec<(u32, u32)> = {
            // Sort the on-plane corners along the cut line.
            let normal = {
                let mut nn = [0.0; 3];
                for i in 0..n {
                    let (a, b) = (out.positions[vs[i] as usize], out.positions[vs[(i + 1) % n] as usize]);
                    nn[0] += (a[1] - b[1]) * (a[2] + b[2]);
                    nn[1] += (a[2] - b[2]) * (a[0] + b[0]);
                    nn[2] += (a[0] - b[0]) * (a[1] + b[1]);
                }
                normalise(nn)
            };
            let dir = normalise(cross(normal, plane.normal));
            let mut sorted: Vec<u32> = on.iter().map(|&i| vs[i]).collect();
            sorted.sort_by(|a, b| dot(out.positions[*a as usize], dir).total_cmp(&dot(out.positions[*b as usize], dir)));
            sorted.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
        };
        for (a, b) in pairs {
            let mut next = Vec::new();
            for (pv, pu) in pieces {
                let (ia, ib) = (pv.iter().position(|&x| x == a), pv.iter().position(|&x| x == b));
                match (ia, ib) {
                    (Some(i), Some(j)) if (i + 1) % pv.len() != j && (j + 1) % pv.len() != i && i != j => {
                        let (i, j) = (i.min(j), i.max(j));
                        let first: Vec<u32> = pv[i..=j].to_vec();
                        let second: Vec<u32> = pv[j..].iter().chain(pv[..=i].iter()).copied().collect();
                        let (fu, su) = if pu.is_empty() { (Vec::new(), Vec::new()) } else { (pu[i..=j].to_vec(), pu[j..].iter().chain(pu[..=i].iter()).copied().collect()) };
                        new_edges.push(ekey(a, b));
                        next.push((first, fu));
                        next.push((second, su));
                    }
                    _ => next.push((pv, pu)),
                }
            }
            pieces = next;
        }
        for (pv, pu) in pieces {
            out.add_face_like(&pv, if pu.is_empty() { None } else { Some(&pu[..]) }, m, s, sm);
        }
    }
    if !has_uv {
        out.uvs.clear();
    }
    *p = out;
    new_edges
}

/// Cuts the mesh in two along a plane; `keep`: Some(true) keeps the side
/// the normal points to, Some(false) the other, None both. `fill` closes
/// the cut with faces.
pub fn bisect(p: &mut Poly, plane: Plane, keep: Option<bool>, fill: bool) {
    let all = vec![true; p.faces()];
    knife(p, &all, plane, &|_| true);
    if let Some(k) = keep {
        let eps = 1e-4 * {
            let (lo, hi) = p.bounds();
            length(sub(hi, lo)).max(1.0)
        };
        let keep_face: Vec<bool> = (0..p.faces())
            .map(|f| {
                let s: f32 = p.face(f).iter().map(|&v| plane.side(p.positions[v as usize])).sum::<f32>() / p.face(f).len() as f32;
                if k { s > -eps * 0.5 } else { s < eps * 0.5 }
            })
            .collect();
        p.retain_faces(|f| keep_face[f]);
        p.compact();
        if fill {
            fill_holes(p, usize::MAX);
        }
    }
}

/// Closed loops of open edges, each in the order its faces run the edges.
pub fn boundary_loops(p: &Poly) -> Vec<Vec<u32>> {
    let t = Topology::of(p);
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for f in 0..p.faces() {
        let c = p.face(f);
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            if t.is_boundary(ekey(a, b)) {
                next.entry(a).or_default().push(b);
            }
        }
    }
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let mut loops = Vec::new();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        for &first in next[&s].clone().iter() {
            if used.contains(&(s, first)) {
                continue;
            }
            let mut lp = vec![s];
            used.insert((s, first));
            let mut at = first;
            let mut ok = false;
            for _ in 0..p.positions.len() + 1 {
                if at == s {
                    ok = true;
                    break;
                }
                lp.push(at);
                let Some(n) = next.get(&at).and_then(|l| l.iter().copied().find(|&b| !used.contains(&(at, b)))) else { break };
                used.insert((at, n));
                at = n;
            }
            if ok && lp.len() >= 3 {
                loops.push(lp);
            }
        }
    }
    loops
}

/// Closes holes of at most `max_sides` edges with one face each.
pub fn fill_holes(p: &mut Poly, max_sides: usize) -> usize {
    let loops = boundary_loops(p);
    let mut n = 0;
    for lp in loops {
        if lp.len() <= max_sides {
            let mut f = lp.clone();
            f.reverse();
            p.add_face(&f, 0);
            n += 1;
        }
    }
    n
}

/// Fills a hole with a grid of quads (for even-sided holes: the clean way
/// to close a cylinder end or a gap that will be subdivided).
pub fn grid_fill(p: &mut Poly, hole: &[u32]) -> bool {
    let n = hole.len();
    if n < 4 || n % 2 == 1 {
        return false;
    }
    // Split the rim into four sides: two of a, two of b (a + b = n/2).
    let half = n / 2;
    let a = half / 2;
    let b = half - a;
    if a == 0 || b == 0 {
        return false;
    }
    let rim: Vec<u32> = hole.iter().rev().copied().collect();
    let side = |i: usize| rim[i % n];
    // Corners at 0, a, a+b, 2a+b.
    let bottom: Vec<u32> = (0..=a).map(side).collect();
    let right: Vec<u32> = (a..=a + b).map(side).collect();
    let top: Vec<u32> = (a + b..=2 * a + b).map(side).rev().collect();
    let left: Vec<u32> = (2 * a + b..=n).map(side).rev().collect();
    let mut grid = vec![vec![0u32; a + 1]; b + 1];
    for (i, &v) in bottom.iter().enumerate() {
        grid[0][i] = v;
    }
    for (i, &v) in top.iter().enumerate() {
        grid[b][i] = v;
    }
    for j in 0..=b {
        grid[j][0] = left[j];
        grid[j][a] = right[j];
    }
    for j in 1..b {
        for i in 1..a {
            let (u, v) = (i as f32 / a as f32, j as f32 / b as f32);
            let pos = |x: u32| p.positions[x as usize];
            // Coons patch from the four sides.
            let l = lerp(pos(left[j]), pos(right[j]), u);
            let m = lerp(pos(bottom[i]), pos(top[i]), v);
            let c = lerp(lerp(pos(grid[0][0]), pos(grid[0][a]), u), lerp(pos(grid[b][0]), pos(grid[b][a]), u), v);
            let q = sub(add(l, m), c);
            grid[j][i] = p.add_vertex(q);
        }
    }
    for j in 0..b {
        for i in 0..a {
            p.add_face(&[grid[j][i], grid[j][i + 1], grid[j + 1][i + 1], grid[j + 1][i]], 0);
        }
    }
    true
}

/// Joins two open loops (as from [`boundary_loops`]) with a band of
/// `segments` rows of faces, `twist` steps round; loops of different
/// lengths are zipped with triangles.
pub fn bridge(p: &mut Poly, a: &[u32], b: &[u32], segments: usize, twist: i32) {
    let (na, nb) = (a.len(), b.len());
    if na < 2 || nb < 2 {
        return;
    }
    let pos = |p: &Poly, v: u32| p.positions[v as usize];
    if na == nb {
        // b runs backward against a; pick the offset that keeps the band
        // shortest.
        let n = na;
        let cost = |off: usize| -> f32 { (0..n).map(|i| length(sub(pos(p, a[i]), pos(p, b[(off + n - i) % n])))).sum() };
        let best = (0..n).min_by(|x, y| cost(*x).total_cmp(&cost(*y))).unwrap_or(0);
        let off = ((best as i64 + twist as i64).rem_euclid(n as i64)) as usize;
        let bb: Vec<u32> = (0..n).map(|i| b[(off + n - i) % n]).collect();
        let segs = segments.max(1);
        let mut rows: Vec<Vec<u32>> = vec![a.to_vec()];
        for s in 1..segs {
            let t = s as f32 / segs as f32;
            rows.push((0..n).map(|i| {
                let q = lerp(pos(p, a[i]), pos(p, bb[i]), t);
                p.add_vertex(q)
            }).collect());
        }
        rows.push(bb);
        for r in 0..segs {
            for i in 0..n {
                let j = (i + 1) % n;
                p.add_face(&[rows[r][j], rows[r][i], rows[r + 1][i], rows[r + 1][j]], 0);
            }
        }
        return;
    }
    // Zip by length along each loop.
    let arc = |p: &Poly, l: &[u32]| -> Vec<f32> {
        let mut s = vec![0.0];
        for i in 0..l.len() {
            let d = length(sub(pos(p, l[(i + 1) % l.len()]), pos(p, l[i])));
            s.push(s[i] + d);
        }
        let total = *s.last().unwrap_or(&1.0);
        s.iter().map(|x| x / total.max(1e-9)).collect()
    };
    let start_b = (0..nb).min_by(|x, y| length(sub(pos(p, a[0]), pos(p, b[*x]))).total_cmp(&length(sub(pos(p, a[0]), pos(p, b[*y]))))).unwrap_or(0);
    let br: Vec<u32> = (0..nb).map(|i| b[(start_b + nb - i) % nb]).collect();
    let (sa, sb) = (arc(p, a), arc(p, &br));
    let (mut i, mut j) = (0usize, 0usize);
    while i < na || j < nb {
        let adv_a = j >= nb || (i < na && sa[i + 1] <= sb[j + 1]);
        if adv_a {
            p.add_face(&[a[(i + 1) % na], a[i % na], br[j % nb]], 0);
            i += 1;
        } else {
            p.add_face(&[a[i % na], br[j % nb], br[(j + 1) % nb]], 0);
            j += 1;
        }
    }
}

/// Bridges two picked face groups: removes them and joins the holes.
pub fn bridge_faces(p: &mut Poly, first: &[bool], second: &[bool], segments: usize, twist: i32) -> Result<(), String> {
    let both: Vec<bool> = first.iter().zip(second).map(|(a, b)| *a || *b).collect();
    let before = boundary_loops(p);
    let ca: HashSet<u32> = (0..p.faces()).filter(|&f| first[f]).flat_map(|f| p.face(f).to_vec()).collect();
    p.retain_faces(|f| !both[f]);
    let loops: Vec<Vec<u32>> = boundary_loops(p).into_iter().filter(|l| !before.iter().any(|b| b.len() == l.len() && b.contains(&l[0]))).collect();
    if loops.len() != 2 {
        return Err(format!("bridging needs two separate face groups that each leave one hole; they left {}", loops.len()));
    }
    let (a, b) = if loops[0].iter().any(|v| ca.contains(v)) { (&loops[0], &loops[1]) } else { (&loops[1], &loops[0]) };
    bridge(p, a, b, segments, twist);
    p.compact();
    Ok(())
}

/// Welds vertices closer than `distance`; returns how many went.
pub fn merge_by_distance(p: &mut Poly, distance: f32) -> usize {
    let d = distance.max(1e-6);
    let cell = |x: V3| [(x[0] / d).floor() as i64, (x[1] / d).floor() as i64, (x[2] / d).floor() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    let mut gone = 0;
    for v in 0..p.positions.len() {
        let x = p.positions[v];
        let c = cell(x);
        let mut found = None;
        'look: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &u in list {
                            if length(sub(p.positions[u as usize], x)) <= d {
                                found = Some(u);
                                break 'look;
                            }
                        }
                    }
                }
            }
        }
        match found {
            Some(u) => {
                map[v] = u;
                gone += 1;
            }
            None => grid.entry(c).or_default().push(v as u32),
        }
    }
    remap_vertices(p, &map);
    gone
}

/// Points every corner at `map[vertex]`, fixing faces and edge marks.
pub fn remap_vertices(p: &mut Poly, map: &[u32]) {
    for c in &mut p.corners {
        *c = map[*c as usize];
    }
    let re = |e: &Edge| ekey(map[e.0 as usize], map[e.1 as usize]);
    p.creases = p.creases.iter().map(|(e, w)| (re(e), *w)).filter(|(e, _)| e.0 != e.1).collect();
    p.sharp = p.sharp.iter().map(re).filter(|e| e.0 != e.1).collect();
    p.seams = p.seams.iter().map(re).filter(|e| e.0 != e.1).collect();
    p.clean();
    p.compact();
}

/// Stitches cracks where a vertex sits on another face's open edge
/// (T-junctions, as booleans and some imports leave): the vertex is
/// threaded into that edge. Returns how many were stitched.
pub fn fix_t_junctions(p: &mut Poly, tolerance: f32) -> usize {
    let mut total = 0;
    for _ in 0..4 {
        let t = Topology::of(p);
        let open = t.boundary_edges();
        if open.is_empty() {
            break;
        }
        let mut on_rim: Vec<u32> = open.iter().flat_map(|e| [e.0, e.1]).collect();
        on_rim.sort_unstable();
        on_rim.dedup();
        let avg = open.iter().map(|e| length(sub(p.positions[e.0 as usize], p.positions[e.1 as usize]))).sum::<f32>() / open.len() as f32;
        let mut grid = crate::spatial::PointGrid::new(&[], avg.max(1e-4));
        for &v in &on_rim {
            grid.insert(v, p.positions[v as usize]);
        }
        let mut inserts: HashMap<Edge, Vec<(f32, u32)>> = HashMap::new();
        for e in &open {
            let (a, b) = (p.positions[e.0 as usize], p.positions[e.1 as usize]);
            let len = length(sub(b, a));
            if len < 1e-9 {
                continue;
            }
            let mid = lerp(a, b, 0.5);
            for v in grid.within(&p.positions, mid, len / 2.0 + tolerance) {
                if v == e.0 || v == e.1 {
                    continue;
                }
                let q = p.positions[v as usize];
                let s = dot(sub(q, a), sub(b, a)) / (len * len);
                if s <= 1e-4 || s >= 1.0 - 1e-4 {
                    continue;
                }
                if crate::select::segment_distance(q, a, b) <= tolerance {
                    inserts.entry(*e).or_default().push((s, v));
                }
            }
        }
        if inserts.is_empty() {
            break;
        }
        let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
        for f in 0..p.faces() {
            let c = p.face(f);
            let mut vs = Vec::with_capacity(c.len());
            for i in 0..c.len() {
                let (a, b) = (c[i], c[(i + 1) % c.len()]);
                vs.push(a);
                if let Some(list) = inserts.get(&ekey(a, b)) {
                    let mut l = list.clone();
                    l.sort_by(|x, y| x.0.total_cmp(&y.0));
                    if a != ekey(a, b).0 {
                        l.reverse();
                    }
                    for (_, v) in l {
                        vs.push(v);
                        total += 1;
                    }
                }
            }
            out.add_face_like(&vs, None, p.face_material[f], p.face_set[f], p.face_smooth[f]);
        }
        out.uvs.clear();
        *p = out;
        p.clean();
    }
    total
}

/// Merges the given vertices into one at their centre.
pub fn merge_at_centre(p: &mut Poly, verts: &[u32]) {
    if verts.is_empty() {
        return;
    }
    let mut c = [0.0; 3];
    for &v in verts {
        c = add(c, p.positions[v as usize]);
    }
    let c = scale(c, 1.0 / verts.len() as f32);
    let keep = verts[0];
    p.positions[keep as usize] = c;
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    for &v in verts {
        map[v as usize] = keep;
    }
    remap_vertices(p, &map);
}

/// Collapses each edge to its middle.
pub fn collapse_edges(p: &mut Poly, edges: &[Edge]) {
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    fn root(m: &mut [u32], mut x: u32) -> u32 {
        while m[x as usize] != x {
            x = m[x as usize];
        }
        x
    }
    for e in edges {
        let (a, b) = (root(&mut map, e.0), root(&mut map, e.1));
        if a != b {
            let mid = lerp(p.positions[a as usize], p.positions[b as usize], 0.5);
            p.positions[a as usize] = mid;
            map[b as usize] = a;
        }
    }
    let full: Vec<u32> = (0..p.positions.len() as u32).map(|v| root(&mut map, v)).collect();
    remap_vertices(p, &full);
}

/// Removes edges, joining the faces either side into one.
pub fn dissolve_edges(p: &mut Poly, edges: &[Edge]) -> usize {
    let t = Topology::of(p);
    let nf = p.faces();
    let mut parent: Vec<usize> = (0..nf).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let dissolve: HashSet<Edge> = edges.iter().map(|e| ekey(e.0, e.1)).collect();
    for e in &dissolve {
        if let Some([a, b]) = t.edge_faces.get(e).map(Vec::as_slice) {
            let (ra, rb) = (find(&mut parent, *a as usize), find(&mut parent, *b as usize));
            if ra != rb {
                parent[ra] = rb;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for f in 0..nf {
        let r = find(&mut parent, f);
        groups.entry(r).or_default().push(f);
    }
    let mut out = Poly { positions: p.positions.clone(), creases: p.creases.clone(), sharp: p.sharp.clone(), seams: p.seams.clone(), materials: p.materials.clone(), ..Poly::new() };
    let mut removed = 0;
    let mut order: Vec<_> = groups.into_iter().collect();
    order.sort_by_key(|(r, _)| *r);
    for (_, fs) in order {
        if fs.len() == 1 {
            let f = fs[0];
            out.add_face_like(p.face(f), None, p.face_material[f], p.face_set[f], p.face_smooth[f]);
            continue;
        }
        // Directed edges of the group not cancelled by their reverse.
        let mut directed: HashSet<(u32, u32)> = HashSet::new();
        for &f in &fs {
            let c = p.face(f);
            for i in 0..c.len() {
                let (a, b) = (c[i], c[(i + 1) % c.len()]);
                if !directed.remove(&(b, a)) {
                    directed.insert((a, b));
                }
            }
        }
        let mut next: HashMap<u32, u32> = HashMap::new();
        let mut ok = true;
        for (a, b) in &directed {
            if next.insert(*a, *b).is_some() {
                ok = false;
            }
        }
        let mut lp = Vec::new();
        if ok && let Some(&s) = next.keys().min() {
            let mut at = s;
            for _ in 0..=next.len() {
                lp.push(at);
                match next.get(&at) {
                    Some(&n) => at = n,
                    None => {
                        lp.clear();
                        break;
                    }
                }
                if at == s {
                    break;
                }
            }
            ok = lp.len() == next.len();
        }
        if ok && lp.len() >= 3 {
            let f0 = fs[0];
            out.add_face_like(&lp, None, p.face_material[f0], p.face_set[f0], p.face_smooth[f0]);
            removed += fs.len() - 1;
        } else {
            for &f in &fs {
                out.add_face_like(p.face(f), None, p.face_material[f], p.face_set[f], p.face_smooth[f]);
            }
        }
    }
    out.uvs.clear();
    // Vertices left with two edges in a straight line are dropped.
    *p = out;
    p.compact();
    removed
}

/// Deletes the picked faces (and vertices no longer used).
pub fn delete_faces(p: &mut Poly, pick: &[bool]) -> usize {
    let n = pick.iter().filter(|b| **b).count();
    p.retain_faces(|f| !pick[f]);
    p.compact();
    n
}

/// Mirrors the mesh across the plane `coordinate[axis] = 0`, joined to
/// the original along the middle.
pub fn mirror(p: &mut Poly, axis: usize, weld: f32) {
    let mut m = p.clone();
    m.materials.clear();
    let n = p.positions.len() as u32;
    for v in &mut m.positions {
        v[axis] = -v[axis];
    }
    for f in 0..m.faces() {
        m.flip_face(f);
    }
    let mats = p.materials.clone();
    let base_mat = p.materials.len() as u32;
    p.merge(&m);
    // The copy uses the same materials.
    for f in 0..p.faces() {
        if p.face_material[f] >= base_mat {
            p.face_material[f] -= base_mat;
        }
    }
    p.materials = mats;
    // Weld the seam only.
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    for v in 0..n {
        if p.positions[v as usize][axis].abs() <= weld {
            map[(n + v) as usize] = v;
            p.positions[v as usize][axis] = 0.0;
        }
    }
    remap_vertices(p, &map);
}

/// Makes the mesh symmetric: keeps the half on the `positive` side of
/// `axis` and mirrors it over the other.
pub fn symmetrize(p: &mut Poly, axis: usize, positive: bool, weld: f32) {
    let mut normal = [0.0; 3];
    normal[axis] = if positive { 1.0 } else { -1.0 };
    bisect(p, Plane { at: [0.0; 3], normal }, Some(true), false);
    mirror(p, axis, weld);
}

/// How a soft move fades from its centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Falloff {
    Smooth,
    Linear,
    Sharp,
    Root,
    Sphere,
    Constant,
}

impl Falloff {
    pub fn parse(s: &str) -> Option<Falloff> {
        Some(match s {
            "smooth" => Falloff::Smooth,
            "linear" => Falloff::Linear,
            "sharp" => Falloff::Sharp,
            "root" => Falloff::Root,
            "sphere" => Falloff::Sphere,
            "constant" | "none" => Falloff::Constant,
            _ => return None,
        })
    }
    /// Weight at distance `t` (0 at the centre, 1 at the edge).
    pub fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let u = 1.0 - t;
        match self {
            Falloff::Smooth => u * u * (3.0 - 2.0 * u),
            Falloff::Linear => u,
            Falloff::Sharp => u * u,
            Falloff::Root => u.sqrt(),
            Falloff::Sphere => (1.0 - t * t).max(0.0).sqrt(),
            Falloff::Constant => 1.0,
        }
    }
}

/// Moves, turns and scales picked vertices about `pivot`; with a
/// `radius`, nearby vertices follow with a fading weight (soft selection).
pub fn transform(p: &mut Poly, verts: &[bool], translate: V3, rotate_deg: V3, scale_by: V3, pivot: V3, radius: f32, falloff: Falloff) {
    let r = studio_mesh::ops::rotation(rotate_deg);
    let apply = |q: V3| -> V3 {
        let l = sub(q, pivot);
        let s = [l[0] * scale_by[0], l[1] * scale_by[1], l[2] * scale_by[2]];
        let rot = [dot(r[0], s), dot(r[1], s), dot(r[2], s)];
        add(add(rot, pivot), translate)
    };
    let picked: Vec<V3> = (0..p.positions.len()).filter(|&v| verts[v]).map(|v| p.positions[v]).collect();
    for v in 0..p.positions.len() {
        let q = p.positions[v];
        let w = if verts[v] {
            1.0
        } else if radius > 0.0 {
            let d = picked.iter().map(|x| length(sub(*x, q))).fold(f32::INFINITY, f32::min);
            if d < radius { falloff.weight(d / radius) } else { 0.0 }
        } else {
            0.0
        };
        if w > 0.0 {
            p.positions[v] = lerp(q, apply(q), w);
        }
    }
}

/// Splits n-gons and quads into triangles.
pub fn triangulate_faces(p: &mut Poly, pick: &[bool]) {
    let has_uv = p.has_uvs();
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    for f in 0..p.faces() {
        let c = p.face(f);
        let uv = p.face_uvs(f);
        if !pick[f] || c.len() == 3 {
            out.add_face_like(c, uv, p.face_material[f], p.face_set[f], p.face_smooth[f]);
            continue;
        }
        for t in triangulate(&out.positions, c) {
            let tu = uv.map(|u| [u[t[0]], u[t[1]], u[t[2]]]);
            out.add_face_like(&[c[t[0]], c[t[1]], c[t[2]]], tu.as_ref().map(|x| &x[..]), p.face_material[f], p.face_set[f], p.face_smooth[f]);
        }
    }
    if !has_uv {
        out.uvs.clear();
    }
    *p = out;
}

/// Joins pairs of triangles into quads where they lie nearly flat
/// together (within `degrees`) and make a well-shaped quad.
pub fn tris_to_quads(p: &mut Poly, degrees: f32) -> usize {
    let t = Topology::of(p);
    let c = degrees.to_radians().cos();
    let mut cands: Vec<(f32, Edge, u32, u32)> = Vec::new();
    for (e, fs) in &t.edge_faces {
        if let [a, b] = fs[..]
            && p.face(a as usize).len() == 3
            && p.face(b as usize).len() == 3
            && p.face_material[a as usize] == p.face_material[b as usize]
            && !p.sharp.contains(e)
        {
            let d = dot(p.face_normal(a as usize), p.face_normal(b as usize));
            if d < c {
                continue;
            }
            let q = quad_of(p.face(a as usize), p.face(b as usize), *e);
            let Some(q) = q else { continue };
            // Prefer square corners.
            let pts: Vec<V3> = q.iter().map(|&v| p.positions[v as usize]).collect();
            let mut worst = 0.0f32;
            let mut convex = true;
            let n = normalise(add(p.face_normal(a as usize), p.face_normal(b as usize)));
            for i in 0..4 {
                let (x, y, z) = (pts[(i + 3) % 4], pts[i], pts[(i + 1) % 4]);
                let (u, v) = (normalise(sub(x, y)), normalise(sub(z, y)));
                worst = worst.max(dot(u, v).abs());
                if dot(cross(sub(z, y), sub(x, y)), n) <= 0.0 {
                    convex = false;
                }
            }
            if convex {
                cands.push((worst + (1.0 - d), *e, a, b));
            }
        }
    }
    cands.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut used = vec![false; p.faces()];
    let mut joins: Vec<(u32, u32, [u32; 4])> = Vec::new();
    for (_, e, a, b) in cands {
        if used[a as usize] || used[b as usize] {
            continue;
        }
        if let Some(q) = quad_of(p.face(a as usize), p.face(b as usize), e) {
            used[a as usize] = true;
            used[b as usize] = true;
            joins.push((a, b, q));
        }
    }
    let n = joins.len();
    let mut add_faces: Vec<([u32; 4], u32, u32, bool)> = Vec::new();
    let gone: HashSet<u32> = joins.iter().flat_map(|(a, b, _)| [*a, *b]).collect();
    for (a, _, q) in &joins {
        add_faces.push((*q, p.face_material[*a as usize], p.face_set[*a as usize], p.face_smooth[*a as usize]));
    }
    p.retain_faces(|f| !gone.contains(&(f as u32)));
    p.uvs.clear();
    for (q, m, s, sm) in add_faces {
        p.add_face_like(&q, None, m, s, sm);
    }
    n
}

/// The quad two triangles sharing edge `e` make (wound like them).
fn quad_of(a: &[u32], b: &[u32], e: Edge) -> Option<[u32; 4]> {
    let sa = edge_slot(a, e)?;
    let opp_b = *b.iter().find(|&&v| v != e.0 && v != e.1)?;
    // a runs a[sa] -> a[sa+1]; b's far corner goes between them.
    Some([a[sa], opp_b, a[(sa + 1) % 3], a[(sa + 2) % 3]])
}

/// Adds a vertex at each picked face's centre, fanning the face to it.
pub fn poke(p: &mut Poly, pick: &[bool], offset: f32) {
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    for f in 0..p.faces() {
        let c = p.face(f).to_vec();
        if !pick[f] {
            out.add_face_like(&c, None, p.face_material[f], p.face_set[f], p.face_smooth[f]);
            continue;
        }
        let mut ctr = [0.0; 3];
        for &v in &c {
            ctr = add(ctr, out.positions[v as usize]);
        }
        ctr = scale(ctr, 1.0 / c.len() as f32);
        let n = {
            let tmp = Poly::from_faces(c.iter().map(|&v| out.positions[v as usize]).collect(), &[(0..c.len() as u32).collect()]);
            tmp.face_normal(0)
        };
        let m = out.add_vertex(add(ctr, scale(n, offset)));
        for i in 0..c.len() {
            out.add_face_like(&[c[i], c[(i + 1) % c.len()], m], None, p.face_material[f], p.face_set[f], p.face_smooth[f]);
        }
    }
    out.uvs.clear();
    *p = out;
}

/// Splits every face into quads `cuts` times over (each edge halved, a
/// point in each face's middle); the shape stays as it is.
pub fn subdivide_flat(p: &mut Poly, cuts: usize) {
    for _ in 0..cuts.max(1) {
        *p = crate::subd::once_with(p, true);
    }
}

/// Marks edges sharper than `degrees` as hard (and, optionally, as
/// creased for subdivision and as texture seams).
pub fn mark_sharp(p: &mut Poly, degrees: f32, crease: Option<f32>, seam: bool) -> usize {
    let edges = crate::select::Edges::Sharper { degrees }.list(p);
    for e in &edges {
        p.sharp.insert(*e);
        if let Some(w) = crease {
            p.creases.insert(*e, w);
        }
        if seam {
            p.seams.insert(*e);
        }
    }
    edges.len()
}

/// Extrudes open edges outward into a new band (`offset` is the move).
pub fn extrude_edges(p: &mut Poly, edges: &[Edge], offset: V3) -> Vec<Edge> {
    let t = Topology::of(p);
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut out = Vec::new();
    for e in edges {
        if !t.is_boundary(*e) {
            continue;
        }
        // Direction the edge's face runs it.
        let f = t.edge_faces[e][0] as usize;
        let c = p.face(f);
        let s = edge_slot(c, *e).unwrap_or(0);
        let (a, b) = (c[s], c[(s + 1) % c.len()]);
        let na = *map.entry(a).or_insert_with(|| {
            let q = add(p.positions[a as usize], offset);
            p.add_vertex(q)
        });
        let nb = *map.entry(b).or_insert_with(|| {
            let q = add(p.positions[b as usize], offset);
            p.add_vertex(q)
        });
        p.add_face(&[b, a, na, nb], p.face_material[f]);
        out.push(ekey(na, nb));
    }
    p.uvs.clear();
    out
}

/// Turns a profile (points in the x–y plane, x the distance from the axis)
/// round the y axis: `degrees` of a turn in `steps`, closing up when it
/// goes all the way. Vertices on the axis are shared.
pub fn spin(profile: &[V3], degrees: f32, steps: usize, axis: usize) -> Poly {
    let steps = steps.max(3);
    let full = (degrees.abs() - 360.0).abs() < 1e-3;
    let columns = if full { steps } else { steps + 1 };
    let mut p = Poly::new();
    let mut ids = vec![vec![0u32; profile.len()]; columns];
    let rot = |q: V3, a: f32| -> V3 {
        let (s, c) = a.sin_cos();
        match axis {
            0 => [q[0], q[1] * c - q[2] * s, q[1] * s + q[2] * c],
            2 => [q[0] * c - q[1] * s, q[0] * s + q[1] * c, q[2]],
            _ => [q[0] * c + q[2] * s, q[1], -q[0] * s + q[2] * c],
        }
    };
    let on_axis = |q: V3| match axis {
        0 => q[1].abs() < 1e-5 && q[2].abs() < 1e-5,
        2 => q[0].abs() < 1e-5 && q[1].abs() < 1e-5,
        _ => q[0].abs() < 1e-5 && q[2].abs() < 1e-5,
    };
    let shared: Vec<Option<u32>> = profile.iter().map(|&q| on_axis(q).then(|| p.add_vertex(q))).collect();
    for (i, col) in ids.iter_mut().enumerate() {
        let a = degrees.to_radians() * i as f32 / steps as f32;
        for (j, &q) in profile.iter().enumerate() {
            col[j] = shared[j].unwrap_or_else(|| p.add_vertex(rot(q, a)));
        }
    }
    let mut uvs_ok = true;
    let _ = &mut uvs_ok;
    for i in 0..steps {
        let i2 = if full { (i + 1) % columns } else { i + 1 };
        for j in 0..profile.len() - 1 {
            let q = [ids[i][j], ids[i2][j], ids[i2][j + 1], ids[i][j + 1]];
            let uv = [[i as f32 / steps as f32, j as f32 / (profile.len() - 1) as f32], [(i + 1) as f32 / steps as f32, j as f32 / (profile.len() - 1) as f32], [(i + 1) as f32 / steps as f32, (j + 1) as f32 / (profile.len() - 1) as f32], [i as f32 / steps as f32, (j + 1) as f32 / (profile.len() - 1) as f32]];
            let mut vs: Vec<u32> = Vec::new();
            let mut us: Vec<[f32; 2]> = Vec::new();
            for k in 0..4 {
                if !vs.contains(&q[k]) {
                    vs.push(q[k]);
                    us.push(uv[k]);
                }
            }
            if vs.len() >= 3 {
                p.add_face_like(&vs, Some(&us), 0, 0, true);
            }
        }
    }
    p.ensure_materials();
    p.orient_outward();
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;
    use crate::select::{Edges, Faces};

    fn closed(p: &Poly) -> bool {
        Topology::of(p).boundary_edges().is_empty()
    }

    fn volume(p: &Poly) -> f32 {
        let mut v = 0.0;
        for f in 0..p.faces() {
            for t in triangulate(&p.positions, p.face(f)) {
                let c = p.face(f);
                let (a, b, cc) = (p.positions[c[t[0]] as usize], p.positions[c[t[1]] as usize], p.positions[c[t[2]] as usize]);
                v += dot(a, cross(b, cc)) / 6.0;
            }
        }
        v
    }

    #[test]
    fn extruding_the_top_of_a_cube_makes_it_taller() {
        let mut c = prim::cube([2.0; 3], 1);
        let top = Faces::named("top").unwrap().mask(&c);
        extrude(&mut c, &top, 1.0, None, false);
        c.compact();
        assert_eq!(c.faces(), 10);
        assert!(closed(&c));
        assert!((volume(&c) - 12.0).abs() < 1e-3, "{}", volume(&c));
        assert!((c.bounds().0[1] + 2.0).abs() < 1e-5);
    }

    #[test]
    fn inset_then_push_in_makes_a_panel() {
        let mut c = prim::cube([2.0; 3], 1);
        let front = Faces::named("front").unwrap().mask(&c);
        inset(&mut c, &front, 0.2, -0.1, false);
        c.compact();
        assert_eq!(c.faces(), 10);
        assert!(closed(&c));
        assert!(volume(&c) < 8.0);
        // The panel is 1.6 wide.
        let f = (0..c.faces()).find(|&f| c.face_centre(f)[2] < -0.85 && c.face_area(f) > 2.0).unwrap();
        assert!((c.face_area(f) - 2.56).abs() < 1e-3, "{}", c.face_area(f));
    }

    #[test]
    fn loop_cut_round_a_cube() {
        let mut c = prim::cube([2.0; 3], 1);
        let e = crate::select::nearest_edge(&c, [1.0, 0.0, 1.0]).unwrap();
        let made = loop_cut(&mut c, e, 2, 0.0);
        assert_eq!(made.len(), 8, "two loops of four");
        assert_eq!(c.faces(), 14);
        assert!(closed(&c));
        assert!((volume(&c) - 8.0).abs() < 1e-3);
    }

    #[test]
    fn bisect_and_fill_halves_the_volume() {
        let mut c = prim::uv_sphere(1.0, 16, 8);
        let v0 = volume(&c);
        bisect(&mut c, Plane { at: [0.0; 3], normal: [1.0, 0.0, 0.0] }, Some(true), true);
        assert!(closed(&c));
        assert!((volume(&c) - v0 / 2.0).abs() < 0.02, "{} vs {}", volume(&c), v0 / 2.0);
    }

    #[test]
    fn knife_splits_faces() {
        let mut c = prim::cube([2.0; 3], 1);
        let all = vec![true; 6];
        let made = knife(&mut c, &all, Plane { at: [0.3, 0.0, 0.0], normal: [1.0, 0.0, 0.0] }, &|_| true);
        assert_eq!(made.len(), 4);
        assert_eq!(c.faces(), 10);
        assert!(closed(&c));
    }

    #[test]
    fn bridging_two_boxes_makes_one_closed_piece() {
        let mut a = prim::cube([2.0; 3], 1);
        let mut b = prim::cube([2.0; 3], 1);
        b.map_positions(|v| [v[0] + 5.0, v[1], v[2]]);
        a.merge(&b);
        let right_of_a = Faces::And(vec![Faces::Facing { direction: [1.0, 0.0, 0.0], degrees: 10.0 }, Faces::Side { axis: 0, at: 2.0, above: false }]).mask(&a);
        let left_of_b = Faces::And(vec![Faces::Facing { direction: [-1.0, 0.0, 0.0], degrees: 10.0 }, Faces::Side { axis: 0, at: 2.0, above: true }]).mask(&a);
        bridge_faces(&mut a, &right_of_a, &left_of_b, 3, 0).unwrap();
        assert!(closed(&a));
        assert!((volume(&a) - (16.0 + 12.0)).abs() < 1e-2, "{}", volume(&a));
    }

    #[test]
    fn merging_and_mirroring() {
        let mut c = prim::cube([2.0; 3], 2);
        symmetrize(&mut c, 0, true, 1e-3);
        assert!(closed(&c));
        assert!((volume(&c) - 8.0).abs() < 1e-3);
        let n = merge_by_distance(&mut c, 1e-3);
        assert_eq!(n, 0);
        let sharp = Edges::Sharper { degrees: 30.0 }.list(&c).len();
        assert_eq!(sharp, 24);
    }

    #[test]
    fn spin_makes_a_closed_vase() {
        let prof = [[0.0, -1.0, 0.0], [0.5, -0.9, 0.0], [0.8, 0.0, 0.0], [0.4, 0.9, 0.0], [0.0, 1.0, 0.0]];
        let v = spin(&prof, 360.0, 24, 1);
        assert!(closed(&v));
        assert!(volume(&v) > 0.5);
    }

    #[test]
    fn triangles_back_into_quads() {
        let c = prim::cube([2.0; 3], 2);
        let mut t = Poly::from_mesh(&c.to_mesh(180.0, crate::NormalMode::Angle));
        assert_eq!(t.faces(), 48);
        let n = tris_to_quads(&mut t, 5.0);
        assert_eq!(n, 24);
        assert!(closed(&t));
    }

    #[test]
    fn dissolving_a_loop_rejoins_the_faces() {
        let mut c = prim::cube([2.0; 3], 1);
        let e = crate::select::nearest_edge(&c, [1.0, 0.0, 1.0]).unwrap();
        let made = loop_cut(&mut c, e, 1, 0.0);
        assert_eq!(c.faces(), 10);
        let removed = dissolve_edges(&mut c, &made);
        assert_eq!(removed, 4);
        assert_eq!(c.faces(), 6);
    }

    #[test]
    fn grid_fill_closes_a_cylinder_end_with_quads() {
        let mut c = prim::cylinder(1.0, 1.0, 2.0, 12, 1, prim::Cap::None);
        let loops = boundary_loops(&c);
        assert_eq!(loops.len(), 2);
        for l in loops {
            assert!(grid_fill(&mut c, &l));
        }
        assert!(closed(&c));
        assert!((0..c.faces()).all(|f| c.face(f).len() == 4));
    }

    #[test]
    fn soft_moves_fade() {
        let mut g = prim::plane(10.0, 10.0, 10, 10);
        let centre = (0..g.positions.len()).min_by(|&a, &b| length(g.positions[a]).total_cmp(&length(g.positions[b]))).unwrap();
        let mut sel = vec![false; g.positions.len()];
        sel[centre] = true;
        transform(&mut g, &sel, [0.0, -2.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3], 3.0, Falloff::Smooth);
        assert!((g.positions[centre][1] + 2.0).abs() < 1e-5);
        let near = g.positions.iter().filter(|v| v[1] < -0.01 && v[1] > -1.99).count();
        assert!(near >= 4);
        assert!(g.positions.iter().filter(|v| length([v[0], 0.0, v[2]]) > 3.1).all(|v| v[1] == 0.0));
    }
}
