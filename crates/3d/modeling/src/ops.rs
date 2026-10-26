// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The rest of the edit-mode toolbox, next to [`crate::edit`]: duplicate,
//! split and separate; delete and dissolve by vertex, edge or face; merges
//! at a point, in groups and by distance; vertex bevels; edge and vertex
//! slides; subdivision of picked edges and faces; connecting vertices and
//! making faces; filling and bridging edge loops; giving faces thickness;
//! normals; spin and screw from an edge chain; bisect within a pick; and
//! packing texture layouts.
//!
//! Every function keeps faces wound outward and says what it returns
//! (numbers valid in the mesh as it is left).

use crate::edit::{self, Plane};
use crate::poly::{Edge, Poly, Topology, ekey, triangulate};
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

const MARK: u32 = 1 << 30;

/// Runs `f` with the picked faces tagged, and returns which faces carry
/// the tag afterwards: the pieces the picked faces became, however `f`
/// rebuilt the face list (materials, sets and shading ride along with
/// every face a tool makes from another).
pub fn keep_face_pick(p: &mut Poly, pick: &[bool], f: impl FnOnce(&mut Poly)) -> Vec<bool> {
    for (i, s) in p.face_set.iter_mut().enumerate() {
        if pick.get(i).copied().unwrap_or(false) {
            *s |= MARK;
        }
    }
    f(p);
    let out: Vec<bool> = p.face_set.iter().map(|s| s & MARK != 0).collect();
    for s in &mut p.face_set {
        *s &= !MARK;
    }
    out
}

/// The signed volume a mesh encloses (negative when it faces inward).
pub fn volume(p: &Poly) -> f32 {
    let mut v = 0.0f64;
    for f in 0..p.faces() {
        let c = p.face(f);
        for t in triangulate(&p.positions, c) {
            let (a, b, cc) = (p.positions[c[t[0]] as usize], p.positions[c[t[1]] as usize], p.positions[c[t[2]] as usize]);
            v += dot(a, cross(b, cc)) as f64 / 6.0;
        }
    }
    v as f32
}

/// The faces tagged by [`keep_face_pick`] (inside its closure).
pub fn marked(p: &Poly) -> Vec<bool> {
    p.face_set.iter().map(|s| s & MARK != 0).collect()
}

/// Copies the picked faces (with their own vertices); returns the copies'
/// face numbers.
pub fn duplicate(p: &mut Poly, pick: &[bool]) -> Vec<u32> {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut made = Vec::new();
    let nf = p.faces();
    for f in 0..nf {
        if !pick.get(f).copied().unwrap_or(false) {
            continue;
        }
        let c: Vec<u32> = p.face(f).to_vec();
        let vs: Vec<u32> = c
            .iter()
            .map(|&v| {
                *map.entry(v).or_insert_with(|| {
                    let q = p.positions[v as usize];
                    p.add_vertex(q)
                })
            })
            .collect();
        let uv = p.face_uvs(f).map(|u| u.to_vec());
        let (m, s, sm) = (p.face_material[f], p.face_set[f], p.face_smooth[f]);
        made.push(p.add_face_like(&vs, uv.as_deref(), m, s, sm) as u32);
    }
    copy_marks(p, &map);
    made
}

fn copy_marks(p: &mut Poly, map: &HashMap<u32, u32>) {
    let re = |e: &Edge| Some(ekey(*map.get(&e.0)?, *map.get(&e.1)?));
    let creases: Vec<(Edge, f32)> = p.creases.iter().filter_map(|(e, w)| re(e).map(|k| (k, *w))).collect();
    let sharp: Vec<Edge> = p.sharp.iter().filter_map(re).collect();
    let seams: Vec<Edge> = p.seams.iter().filter_map(re).collect();
    p.creases.extend(creases);
    p.sharp.extend(sharp);
    p.seams.extend(seams);
}

/// Disconnects the picked faces from the rest: where they meet other
/// faces they get their own vertices. Returns how many vertices were
/// split off.
pub fn split(p: &mut Poly, pick: &[bool]) -> usize {
    let t = Topology::of(p);
    let mut map: HashMap<u32, u32> = HashMap::new();
    for f in 0..p.faces() {
        if !pick.get(f).copied().unwrap_or(false) {
            continue;
        }
        for &v in p.face(f).to_vec().iter() {
            let shared = t.vertex_faces[v as usize].iter().any(|&g| !pick.get(g as usize).copied().unwrap_or(false));
            if shared && !map.contains_key(&v) {
                let q = p.positions[v as usize];
                map.insert(v, p.add_vertex(q));
            }
        }
    }
    for f in 0..p.faces() {
        if pick.get(f).copied().unwrap_or(false) {
            for k in p.face_range(f) {
                if let Some(&n) = map.get(&p.corners[k]) {
                    p.corners[k] = n;
                }
            }
        }
    }
    copy_marks(p, &map);
    map.len()
}

/// Takes the picked faces out into a mesh of their own (same material
/// list); both are compacted.
pub fn separate(p: &mut Poly, pick: &[bool]) -> Poly {
    let mut out = p.clone();
    out.retain_faces(|f| pick.get(f).copied().unwrap_or(false));
    out.compact();
    p.retain_faces(|f| !pick.get(f).copied().unwrap_or(false));
    p.compact();
    out
}

/// Face picks of each connected piece.
pub fn loose_parts(p: &Poly) -> Vec<Vec<bool>> {
    let t = Topology::of(p);
    let mut piece = vec![usize::MAX; p.faces()];
    let mut out = Vec::new();
    for seed in 0..p.faces() {
        if piece[seed] != usize::MAX {
            continue;
        }
        let id = out.len();
        let mut m = vec![false; p.faces()];
        piece[seed] = id;
        m[seed] = true;
        let mut stack = vec![seed];
        while let Some(f) = stack.pop() {
            for &v in p.face(f) {
                for &g in &t.vertex_faces[v as usize] {
                    if piece[g as usize] == usize::MAX {
                        piece[g as usize] = id;
                        m[g as usize] = true;
                        stack.push(g as usize);
                    }
                }
            }
        }
        out.push(m);
    }
    out
}

/// Deletes faces that use any picked vertex; returns faces deleted.
pub fn delete_vertices(p: &mut Poly, verts: &[bool]) -> usize {
    let pick: Vec<bool> = (0..p.faces()).map(|f| p.face(f).iter().any(|&v| verts.get(v as usize).copied().unwrap_or(false))).collect();
    edit::delete_faces(p, &pick)
}

/// Deletes faces that use any of the edges; returns faces deleted.
pub fn delete_edges(p: &mut Poly, edges: &HashSet<Edge>) -> usize {
    let pick: Vec<bool> = (0..p.faces())
        .map(|f| {
            let c = p.face(f);
            (0..c.len()).any(|i| edges.contains(&ekey(c[i], c[(i + 1) % c.len()])))
        })
        .collect();
    edit::delete_faces(p, &pick)
}

/// Takes vertices out of the faces round them where the face keeps at
/// least three corners (`only`: just these vertices). Vertices on an
/// open rim go only when their two edges run straight on. Returns how
/// many went.
pub fn dissolve_two_edge_vertices(p: &mut Poly, only: Option<&[bool]>) -> usize {
    let t = Topology::of(p);
    let mut gone = vec![false; p.positions.len()];
    let mut n = 0;
    for v in 0..p.positions.len() {
        if only.is_some_and(|o| !o.get(v).copied().unwrap_or(false)) {
            continue;
        }
        let nb = t.neighbours(p, v as u32);
        if nb.len() != 2 || t.vertex_faces[v].is_empty() {
            continue;
        }
        if t.vertex_faces[v].iter().any(|&f| p.face(f as usize).len() <= 3) {
            continue;
        }
        let boundary = t.is_boundary(ekey(v as u32, nb[0])) || t.is_boundary(ekey(v as u32, nb[1]));
        if boundary {
            let (a, b) = (normalise(sub(p.positions[nb[0] as usize], p.positions[v])), normalise(sub(p.positions[nb[1] as usize], p.positions[v])));
            if dot(a, b) > -0.999 {
                continue;
            }
        }
        // Neighbours of a vertex being taken out keep theirs.
        if nb.iter().any(|&u| gone[u as usize]) {
            continue;
        }
        gone[v] = true;
        n += 1;
    }
    if n == 0 {
        return 0;
    }
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    let uvs = p.has_uvs();
    for f in 0..p.faces() {
        let r = p.face_range(f);
        let mut vs = Vec::new();
        let mut us = Vec::new();
        for k in r {
            let v = p.corners[k];
            if !gone[v as usize] {
                vs.push(v);
                if uvs {
                    us.push(p.uvs[k]);
                }
            }
        }
        out.add_face_like(&vs, uvs.then_some(&us[..]), p.face_material[f], p.face_set[f], p.face_smooth[f]);
    }
    if !uvs {
        out.uvs.clear();
    }
    // Marks on the edges either side of a gone vertex join up.
    let mut joined: Vec<(Edge, Edge, Edge)> = Vec::new();
    for v in 0..gone.len() {
        if gone[v] {
            let nb = t.neighbours(p, v as u32);
            joined.push((ekey(v as u32, nb[0]), ekey(v as u32, nb[1]), ekey(nb[0], nb[1])));
        }
    }
    for (a, b, j) in joined {
        if out.sharp.remove(&a) | out.sharp.remove(&b) {
            out.sharp.insert(j);
        }
        if out.seams.remove(&a) | out.seams.remove(&b) {
            out.seams.insert(j);
        }
        let w = out.creases.remove(&a).into_iter().chain(out.creases.remove(&b)).fold(0.0f32, f32::max);
        if w > 0.0 {
            out.creases.insert(j, w);
        }
    }
    *p = out;
    p.compact();
    n
}

/// Removes picked vertices, joining the faces round each into one.
/// Returns faces removed.
pub fn dissolve_vertices(p: &mut Poly, verts: &[bool]) -> usize {
    let t = Topology::of(p);
    let before = p.faces();
    let edges: Vec<Edge> = t.edge_faces.iter().filter(|(e, fs)| fs.len() == 2 && (verts.get(e.0 as usize).copied().unwrap_or(false) || verts.get(e.1 as usize).copied().unwrap_or(false))).map(|(e, _)| *e).collect();
    let keep: Vec<V3> = p.positions.clone();
    edit::dissolve_edges(p, &edges);
    // Rim vertices that are left between two edges go too.
    let only: Vec<bool> = {
        let set: HashSet<[u32; 3]> = verts.iter().enumerate().filter(|(_, b)| **b).map(|(v, _)| keep[v].map(f32::to_bits)).collect();
        p.positions.iter().map(|q| set.contains(&q.map(f32::to_bits))).collect()
    };
    dissolve_two_edge_vertices(p, Some(&only));
    before.saturating_sub(p.faces())
}

/// Joins picked faces that touch into one face per group (the edges
/// between them go). Returns faces removed.
pub fn dissolve_faces(p: &mut Poly, pick: &[bool]) -> usize {
    let t = Topology::of(p);
    let before = p.faces();
    let inner: Vec<Edge> = t.edge_faces.iter().filter(|(_, fs)| fs.len() == 2 && fs.iter().all(|&f| pick.get(f as usize).copied().unwrap_or(false))).map(|(e, _)| *e).collect();
    let region: Vec<V3> = inner.iter().flat_map(|e| [p.positions[e.0 as usize], p.positions[e.1 as usize]]).collect();
    edit::dissolve_edges(p, &inner);
    let set: HashSet<[u32; 3]> = region.iter().map(|q| q.map(f32::to_bits)).collect();
    let only: Vec<bool> = p.positions.iter().map(|q| set.contains(&q.map(f32::to_bits))).collect();
    dissolve_two_edge_vertices(p, Some(&only));
    before.saturating_sub(p.faces())
}

/// Removes edges (joining the faces either side) and the vertices left
/// between two edges at their ends. Returns faces removed.
pub fn dissolve_edges_clean(p: &mut Poly, edges: &[Edge]) -> usize {
    let before = p.faces();
    let ends: Vec<V3> = edges.iter().flat_map(|e| [p.positions[e.0 as usize], p.positions[e.1 as usize]]).collect();
    edit::dissolve_edges(p, edges);
    let set: HashSet<[u32; 3]> = ends.iter().map(|q| q.map(f32::to_bits)).collect();
    let only: Vec<bool> = p.positions.iter().map(|q| set.contains(&q.map(f32::to_bits))).collect();
    dissolve_two_edge_vertices(p, Some(&only));
    before.saturating_sub(p.faces())
}

/// Merges each group of vertices into one at its point. Returns the
/// merged vertices' new numbers (one per group).
pub fn merge_groups(p: &mut Poly, groups: &[(Vec<u32>, V3)]) -> Vec<u32> {
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    let mut keeps = Vec::new();
    for (vs, at) in groups {
        let Some(&k) = vs.first() else { continue };
        p.positions[k as usize] = *at;
        for &v in vs {
            map[v as usize] = k;
        }
        keeps.push(k);
    }
    // Where the kept vertices end up after compaction: mark them by
    // position first.
    let marks: Vec<[u32; 3]> = keeps.iter().map(|&k| p.positions[k as usize].map(f32::to_bits)).collect();
    edit::remap_vertices(p, &map);
    marks.iter().filter_map(|m| p.positions.iter().position(|q| q.map(f32::to_bits) == *m).map(|i| i as u32)).collect()
}

/// Groups of picked vertices joined by picked edges.
pub fn islands(p: &Poly, verts: &[bool]) -> Vec<Vec<u32>> {
    let mut parent: Vec<u32> = (0..p.positions.len() as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for e in p.edges() {
        if verts[e.0 as usize] && verts[e.1 as usize] {
            let (a, b) = (find(&mut parent, e.0), find(&mut parent, e.1));
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
    for v in 0..p.positions.len() as u32 {
        if verts[v as usize] {
            let r = find(&mut parent, v);
            groups.entry(r).or_default().push(v);
        }
    }
    let mut out: Vec<Vec<u32>> = groups.into_values().collect();
    out.sort();
    out
}

/// The middle of some vertices.
pub fn centre_of(p: &Poly, vs: &[u32]) -> V3 {
    let mut c = [0.0; 3];
    for &v in vs {
        c = add(c, p.positions[v as usize]);
    }
    scale(c, 1.0 / vs.len().max(1) as f32)
}

/// Welds picked vertices closer than `distance` to each other (others
/// stay). Returns how many went.
pub fn merge_by_distance_in(p: &mut Poly, verts: &[bool], distance: f32) -> usize {
    let d = distance.max(1e-6);
    let cell = |x: V3| [(x[0] / d).floor() as i64, (x[1] / d).floor() as i64, (x[2] / d).floor() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    let mut gone = 0;
    for v in 0..p.positions.len() {
        if !verts.get(v).copied().unwrap_or(false) {
            continue;
        }
        let x = p.positions[v];
        let c = cell(x);
        let mut found = None;
        'look: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &u in grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).map(Vec::as_slice).unwrap_or(&[]) {
                        if length(sub(p.positions[u as usize], x)) <= d {
                            found = Some(u);
                            break 'look;
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
    if gone > 0 {
        edit::remap_vertices(p, &map);
        remove_duplicate_faces(p);
    }
    gone
}

/// Drops faces that use the same corners as an earlier one (what welding
/// two copies of a surface together leaves). Returns how many went.
pub fn remove_duplicate_faces(p: &mut Poly) -> usize {
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    let keep: Vec<bool> = (0..p.faces())
        .map(|f| {
            let mut k = p.face(f).to_vec();
            k.sort_unstable();
            seen.insert(k)
        })
        .collect();
    let n = keep.iter().filter(|k| !**k).count();
    if n > 0 {
        p.retain_faces(|f| keep[f]);
        p.compact();
    }
    n
}

/// Chamfers picked vertices: each corner is cut `width` along every edge
/// and capped (`segments` > 1 rounds the cap, `profile` 0…1 how full).
/// Returns how many vertices were bevelled.
pub fn bevel_vertices(p: &mut Poly, verts: &[bool], width: f32, segments: usize, profile: f32) -> usize {
    let t = Topology::of(p);
    let picked: Vec<u32> = (0..p.positions.len() as u32).filter(|&v| verts.get(v as usize).copied().unwrap_or(false) && !t.vertex_faces[v as usize].is_empty()).collect();
    if picked.is_empty() || width <= 0.0 {
        return 0;
    }
    let is_picked: HashSet<u32> = picked.iter().copied().collect();
    // The cut point on edge v→u near v.
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    for &v in &picked {
        for u in t.neighbours(p, v) {
            let (a, b) = (p.positions[v as usize], p.positions[u as usize]);
            let len = length(sub(b, a));
            let limit = if is_picked.contains(&u) { len * 0.45 } else { len * 0.9 };
            let w = width.min(limit);
            let q = add(a, scale(normalise(sub(b, a)), w));
            let id = p.add_vertex(q);
            cut.insert((v, u), id);
        }
    }
    let uvs = p.has_uvs();
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    // Cap edges as the faces run them (b -> a so the cap runs a -> b).
    let mut caps: HashMap<u32, HashMap<u32, u32>> = HashMap::new();
    let mut cap_look: HashMap<u32, (u32, u32, bool)> = HashMap::new();
    for f in 0..p.faces() {
        let c = p.face(f);
        let n = c.len();
        let uv = p.face_uvs(f);
        let mut vs = Vec::new();
        let mut us = Vec::new();
        for i in 0..n {
            let v = c[i];
            if is_picked.contains(&v) {
                let (prev, next) = (c[(i + n - 1) % n], c[(i + 1) % n]);
                let (a, b) = (cut[&(v, prev)], cut[&(v, next)]);
                vs.push(a);
                vs.push(b);
                if let Some(u) = uv {
                    let (up, un, uc) = (u[(i + n - 1) % n], u[(i + 1) % n], u[i]);
                    let kp = frac(&out.positions, v, prev, a);
                    let kn = frac(&out.positions, v, next, b);
                    us.push([uc[0] + (up[0] - uc[0]) * kp, uc[1] + (up[1] - uc[1]) * kp]);
                    us.push([uc[0] + (un[0] - uc[0]) * kn, uc[1] + (un[1] - uc[1]) * kn]);
                }
                caps.entry(v).or_default().insert(b, a);
                cap_look.entry(v).or_insert((p.face_material[f], p.face_set[f], p.face_smooth[f]));
            } else {
                vs.push(v);
                if let Some(u) = uv {
                    us.push(u[i]);
                }
            }
        }
        out.add_face_like(&vs, uv.map(|_| &us[..]), p.face_material[f], p.face_set[f], p.face_smooth[f]);
    }
    let mut done = 0;
    for &v in &picked {
        let Some(next) = caps.get(&v) else { continue };
        // Walk the cap ring.
        let start = *next.keys().min().unwrap();
        let mut ring = vec![start];
        let mut at = start;
        while let Some(&n) = next.get(&at) {
            if n == start || ring.len() > next.len() + 1 {
                break;
            }
            ring.push(n);
            at = n;
        }
        // An open fan (a rim corner): the chain from its free end.
        if next.get(&at) != Some(&start) {
            let targets: HashSet<u32> = next.values().copied().collect();
            if let Some(&first) = next.keys().find(|k| !targets.contains(k)) {
                ring = vec![first];
                let mut at = first;
                while let Some(&n) = next.get(&at) {
                    ring.push(n);
                    at = n;
                    if ring.len() > next.len() + 1 {
                        break;
                    }
                }
            }
        }
        if ring.len() < 3 {
            continue;
        }
        let (m, s, sm) = cap_look[&v];
        let apex = out.positions[v as usize];
        if segments <= 1 {
            out.add_face_like(&ring, None, m, s, sm);
        } else {
            let pts: Vec<V3> = ring.iter().map(|&r| out.positions[r as usize]).collect();
            let c = scale(pts.iter().fold([0.0; 3], |a, b| add(a, *b)), 1.0 / pts.len() as f32);
            let bulge = (profile.clamp(0.0, 1.0) * 2.0) * 0.35;
            let pole = add(c, scale(sub(apex, c), bulge));
            let mut rows: Vec<Vec<u32>> = vec![ring.clone()];
            for k in 1..segments {
                let tt = k as f32 / segments as f32;
                let lift = (tt * std::f32::consts::FRAC_PI_2).sin();
                let row: Vec<u32> = pts.iter().map(|q| out.add_vertex(add(lerp(*q, c, tt), scale(sub(pole, c), lift)))).collect();
                rows.push(row);
            }
            let tip = out.add_vertex(pole);
            for k in 0..segments - 1 {
                for i in 0..ring.len() {
                    let j = (i + 1) % ring.len();
                    out.add_face_like(&[rows[k][i], rows[k][j], rows[k + 1][j], rows[k + 1][i]], None, m, s, sm);
                }
            }
            let last = &rows[segments - 1];
            for i in 0..ring.len() {
                out.add_face_like(&[last[i], last[(i + 1) % ring.len()], tip], None, m, s, sm);
            }
        }
        done += 1;
    }
    if !uvs {
        out.uvs.clear();
    }
    *p = out;
    p.compact();
    done
}

fn frac(pos: &[V3], v: u32, u: u32, q: u32) -> f32 {
    let len = length(sub(pos[u as usize], pos[v as usize]));
    if len < 1e-12 { 0.0 } else { length(sub(pos[q as usize], pos[v as usize])) / len }
}

/// Ordered chains of the given edges: (vertices, closed).
pub fn edge_chains(edges: &[Edge]) -> Vec<(Vec<u32>, bool)> {
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for e in edges {
        adj.entry(e.0).or_default().push(e.1);
        adj.entry(e.1).or_default().push(e.0);
    }
    let mut used: HashSet<Edge> = HashSet::new();
    let mut out = Vec::new();
    let mut starts: Vec<u32> = adj.keys().copied().collect();
    starts.sort_unstable();
    // Open chains first (from an end), then loops.
    starts.sort_by_key(|v| if adj[v].len() == 1 { 0 } else { 1 });
    for s in starts {
        let Some(&first) = adj[&s].iter().find(|&&u| !used.contains(&ekey(s, u))) else { continue };
        let mut chain = vec![s];
        let mut prev = s;
        let mut at = first;
        used.insert(ekey(s, first));
        let mut closed = false;
        loop {
            if at == s {
                closed = true;
                break;
            }
            chain.push(at);
            let Some(&n) = adj[&at].iter().find(|&&u| u != prev && !used.contains(&ekey(at, u))) else { break };
            used.insert(ekey(at, n));
            prev = at;
            at = n;
        }
        out.push((chain, closed));
    }
    out
}

/// Slides edge loops along the edges beside them: `factor` 1 all the way
/// to one side, −1 to the other. Returns how many vertices moved.
pub fn edge_slide(p: &mut Poly, edges: &[Edge], factor: f32) -> Result<usize, String> {
    let t = Topology::of(p);
    let picked: HashSet<u32> = edges.iter().flat_map(|e| [e.0, e.1]).collect();
    let mut left: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut right: HashMap<u32, Vec<u32>> = HashMap::new();
    for (chain, closed) in edge_chains(edges) {
        let n = chain.len();
        let steps = if closed { n } else { n.saturating_sub(1) };
        for i in 0..steps {
            let (a, b) = (chain[i], chain[(i + 1) % n]);
            for &f in t.edge_faces.get(&ekey(a, b)).map(Vec::as_slice).unwrap_or(&[]) {
                let c = p.face(f as usize);
                let m = c.len();
                let Some(ia) = c.iter().position(|&x| x == a) else { continue };
                let forward = c[(ia + 1) % m] == b;
                let ib = c.iter().position(|&x| x == b).unwrap_or(ia);
                if forward {
                    // a -> b runs this face: it is on the left.
                    let pa = c[(ia + m - 1) % m];
                    let nb = c[(ib + 1) % m];
                    if !picked.contains(&pa) {
                        left.entry(a).or_default().push(pa);
                    }
                    if !picked.contains(&nb) {
                        left.entry(b).or_default().push(nb);
                    }
                } else {
                    let na = c[(ia + 1) % m];
                    let pb = c[(ib + m - 1) % m];
                    if !picked.contains(&na) {
                        right.entry(a).or_default().push(na);
                    }
                    if !picked.contains(&pb) {
                        right.entry(b).or_default().push(pb);
                    }
                }
            }
        }
    }
    if left.is_empty() && right.is_empty() {
        return Err("those edges have nothing beside them to slide along".into());
    }
    let k = factor.clamp(-1.0, 1.0);
    let rails = if k >= 0.0 { &left } else { &right };
    let base = p.positions.clone();
    let mut moved = 0;
    for &v in &picked {
        let Some(list) = rails.get(&v) else { continue };
        let target = scale(list.iter().fold([0.0; 3], |s, &u| add(s, base[u as usize])), 1.0 / list.len() as f32);
        p.positions[v as usize] = lerp(base[v as usize], target, k.abs());
        moved += 1;
    }
    Ok(moved)
}

/// Slides picked vertices along the edge of theirs that best follows
/// `toward` (a direction), `factor` 0…1 of the way.
pub fn vertex_slide(p: &mut Poly, verts: &[bool], toward: V3, factor: f32) -> usize {
    let t = Topology::of(p);
    let d = normalise(toward);
    let base = p.positions.clone();
    let mut n = 0;
    for v in 0..p.positions.len() {
        if !verts.get(v).copied().unwrap_or(false) {
            continue;
        }
        let best = t.neighbours(p, v as u32).into_iter().filter(|&u| !verts[u as usize]).max_by(|&a, &b| dot(normalise(sub(base[a as usize], base[v])), d).total_cmp(&dot(normalise(sub(base[b as usize], base[v])), d)));
        if let Some(u) = best {
            p.positions[v] = lerp(base[v], base[u as usize], factor.clamp(-1.0, 1.0).max(0.0));
            n += 1;
        }
    }
    n
}

/// Splits the given edges into `cuts + 1` pieces each; faces whose every
/// edge was split become grids (quads and triangles), quads split on two
/// opposite sides become strips, others take the new points into their
/// outline. Returns the new vertices.
pub fn subdivide_edges(p: &mut Poly, edges: &HashSet<Edge>, cuts: usize) -> Vec<u32> {
    let cuts = cuts.clamp(1, 64);
    let m = cuts + 1;
    let mut made: HashMap<Edge, Vec<u32>> = HashMap::new();
    let mut new_verts = Vec::new();
    let mut sorted: Vec<Edge> = edges.iter().copied().collect();
    sorted.sort_unstable();
    for e in &sorted {
        let (a, b) = (p.positions[e.0 as usize], p.positions[e.1 as usize]);
        let ids: Vec<u32> = (1..m).map(|k| p.add_vertex(lerp(a, b, k as f32 / m as f32))).collect();
        new_verts.extend_from_slice(&ids);
        made.insert(*e, ids);
    }
    let uvs = p.has_uvs();
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    // A side of a face: its points from corner i to corner i + 1.
    let side = |c: &[u32], i: usize| -> Vec<u32> {
        let (a, b) = (c[i], c[(i + 1) % c.len()]);
        let e = ekey(a, b);
        let mid: Vec<u32> = match made.get(&e) {
            Some(ids) if a == e.0 => ids.clone(),
            Some(ids) => ids.iter().rev().copied().collect(),
            None => Vec::new(),
        };
        std::iter::once(a).chain(mid).chain(std::iter::once(b)).collect()
    };
    let lerp2 = |x: [f32; 2], y: [f32; 2], t: f32| [x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t];
    for f in 0..p.faces() {
        let c = p.face(f).to_vec();
        let n = c.len();
        let uv: Option<Vec<[f32; 2]>> = p.face_uvs(f).map(|u| u.to_vec());
        let look = (p.face_material[f], p.face_set[f], p.face_smooth[f]);
        let split: Vec<bool> = (0..n).map(|i| made.contains_key(&ekey(c[i], c[(i + 1) % n]))).collect();
        let add = |out: &mut Poly, vs: &[u32], us: Option<Vec<[f32; 2]>>| {
            out.add_face_like(vs, us.as_deref(), look.0, look.1, look.2);
        };
        if n == 4 && split.iter().all(|b| *b) {
            let s: Vec<Vec<u32>> = (0..4).map(|i| side(&c, i)).collect();
            let mut g = vec![vec![0u32; m + 1]; m + 1];
            for i in 0..=m {
                g[i][0] = s[0][i];
                g[m][i] = s[1][i];
                g[i][m] = s[2][m - i];
                g[0][i] = s[3][m - i];
            }
            let corner: [V3; 4] = std::array::from_fn(|i| out.positions[c[i] as usize]);
            for j in 1..m {
                for i in 1..m {
                    let (u, v) = (i as f32 / m as f32, j as f32 / m as f32);
                    let pos = |x: u32| out.positions[x as usize];
                    let l = lerp(pos(g[0][j]), pos(g[m][j]), u);
                    let mm = lerp(pos(g[i][0]), pos(g[i][m]), v);
                    let bl = lerp(lerp(corner[0], corner[1], u), lerp(corner[3], corner[2], u), v);
                    let q = sub(studio_mesh::add(l, mm), bl);
                    g[i][j] = out.add_vertex(q);
                    new_verts.push(g[i][j]);
                }
            }
            for j in 0..m {
                for i in 0..m {
                    let quad = [g[i][j], g[i + 1][j], g[i + 1][j + 1], g[i][j + 1]];
                    let us = uv.as_ref().map(|u| {
                        let at = |ii: usize, jj: usize| {
                            let (a, b) = (ii as f32 / m as f32, jj as f32 / m as f32);
                            lerp2(lerp2(u[0], u[1], a), lerp2(u[3], u[2], a), b)
                        };
                        vec![at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]
                    });
                    add(&mut out, &quad, us);
                }
            }
            continue;
        }
        if n == 3 && split.iter().all(|b| *b) {
            let s: Vec<Vec<u32>> = (0..3).map(|i| side(&c, i)).collect();
            let mut g: HashMap<(usize, usize), u32> = HashMap::new();
            for k in 0..=m {
                g.insert((k, 0), s[0][k]);
                g.insert((m - k, k), s[1][k]);
                g.insert((0, m - k), s[2][k]);
            }
            let (t0, t1, t2) = (out.positions[c[0] as usize], out.positions[c[1] as usize], out.positions[c[2] as usize]);
            for j in 1..m {
                for i in 1..m - j {
                    let q = studio_mesh::add(t0, studio_mesh::add(scale(sub(t1, t0), i as f32 / m as f32), scale(sub(t2, t0), j as f32 / m as f32)));
                    let id = out.add_vertex(q);
                    g.insert((i, j), id);
                    new_verts.push(id);
                }
            }
            let tuv = |i: usize, j: usize| uv.as_ref().map(|u| {
                let (a, b) = (i as f32 / m as f32, j as f32 / m as f32);
                [u[0][0] + (u[1][0] - u[0][0]) * a + (u[2][0] - u[0][0]) * b, u[0][1] + (u[1][1] - u[0][1]) * a + (u[2][1] - u[0][1]) * b]
            });
            for j in 0..m {
                for i in 0..m - j {
                    let tri = [g[&(i, j)], g[&(i + 1, j)], g[&(i, j + 1)]];
                    let us = uv.as_ref().map(|_| vec![tuv(i, j).unwrap(), tuv(i + 1, j).unwrap(), tuv(i, j + 1).unwrap()]);
                    add(&mut out, &tri, us);
                    if i + j + 1 < m {
                        let tri = [g[&(i + 1, j)], g[&(i + 1, j + 1)], g[&(i, j + 1)]];
                        let us = uv.as_ref().map(|_| vec![tuv(i + 1, j).unwrap(), tuv(i + 1, j + 1).unwrap(), tuv(i, j + 1).unwrap()]);
                        add(&mut out, &tri, us);
                    }
                }
            }
            continue;
        }
        if n == 4 && split.iter().filter(|b| **b).count() == 2 && split[0] == split[2] {
            // A strip across the two split sides.
            let s0 = if split[0] { 0 } else { 1 };
            let a = side(&c, s0);
            let b: Vec<u32> = side(&c, s0 + 2).into_iter().rev().collect();
            for i in 0..m {
                let quad = [a[i], a[i + 1], b[i + 1], b[i]];
                let us = uv.as_ref().map(|u| {
                    let (q0, q1, q2, q3) = (u[s0], u[(s0 + 1) % 4], u[(s0 + 2) % 4], u[(s0 + 3) % 4]);
                    let (t0, t1) = (i as f32 / m as f32, (i + 1) as f32 / m as f32);
                    vec![lerp2(q0, q1, t0), lerp2(q0, q1, t1), lerp2(q3, q2, t1), lerp2(q3, q2, t0)]
                });
                add(&mut out, &quad, us);
            }
            continue;
        }
        // Points threaded into the outline.
        let mut vs = Vec::new();
        let mut us = Vec::new();
        for i in 0..n {
            let sd = side(&c, i);
            let k = sd.len() - 1;
            for (j, &v) in sd[..k].iter().enumerate() {
                vs.push(v);
                if let Some(u) = &uv {
                    us.push(lerp2(u[i], u[(i + 1) % n], j as f32 / k as f32));
                }
            }
        }
        add(&mut out, &vs, uv.as_ref().map(|_| us));
    }
    if !uvs {
        out.uvs.clear();
    }
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
    new_verts
}

/// Splits faces between picked corners that aren't neighbours. Returns
/// the new edges.
pub fn connect_vertices(p: &mut Poly, verts: &[bool]) -> Vec<Edge> {
    let uvs = p.has_uvs();
    let mut out = Poly { positions: std::mem::take(&mut p.positions), creases: std::mem::take(&mut p.creases), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    let mut new_edges = Vec::new();
    for f in 0..p.faces() {
        let c = p.face(f).to_vec();
        let n = c.len();
        let u: Vec<[f32; 2]> = p.face_uvs(f).map(|u| u.to_vec()).unwrap_or_default();
        let look = (p.face_material[f], p.face_set[f], p.face_smooth[f]);
        let on: Vec<usize> = (0..n).filter(|&i| verts.get(c[i] as usize).copied().unwrap_or(false)).collect();
        let mut chords: Vec<(u32, u32)> = Vec::new();
        if on.len() >= 2 {
            for k in 0..on.len() {
                let (i, j) = (on[k], on[(k + 1) % on.len()]);
                if i == j || (i + 1) % n == j || (j + 1) % n == i {
                    continue;
                }
                let ch = (c[i].min(c[j]), c[i].max(c[j]));
                if !chords.contains(&ch) {
                    chords.push(ch);
                }
            }
        }
        let mut pieces: Vec<(Vec<u32>, Vec<[f32; 2]>)> = vec![(c.clone(), u.clone())];
        for (a, b) in chords {
            let mut next = Vec::new();
            for (pv, pu) in pieces {
                let (ia, ib) = (pv.iter().position(|&x| x == a), pv.iter().position(|&x| x == b));
                match (ia, ib) {
                    (Some(i), Some(j)) if i != j && (i + 1) % pv.len() != j && (j + 1) % pv.len() != i => {
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
            out.add_face_like(&pv, (!pu.is_empty()).then_some(&pu[..]), look.0, look.1, look.2);
        }
    }
    if !uvs {
        out.uvs.clear();
    }
    *p = out;
    new_edges.sort_unstable();
    new_edges.dedup();
    new_edges
}

/// Loops of open edges made wholly of the given edges.
pub fn picked_holes(p: &Poly, edges: &HashSet<Edge>) -> Vec<Vec<u32>> {
    edit::boundary_loops(p)
        .into_iter()
        .filter(|l| (0..l.len()).all(|i| edges.contains(&ekey(l[i], l[(i + 1) % l.len()]))))
        .collect()
}

/// Closes holes whose rims are picked (`grid`: with a grid of quads when
/// the rim has an even count, else one face). Returns new faces.
pub fn fill(p: &mut Poly, edges: &HashSet<Edge>, grid: bool) -> Result<Vec<u32>, String> {
    let holes = picked_holes(p, edges);
    if holes.is_empty() {
        return Err("pick the edges round a hole (a closed loop of open edges)".into());
    }
    let before = p.faces();
    for h in holes {
        if grid && h.len() >= 4 && h.len() % 2 == 0 && edit::grid_fill(p, &h) {
            continue;
        }
        if grid {
            return Err(format!("a grid fill needs an even number of edges round the hole; this one has {}", h.len()));
        }
        let mut f = h.clone();
        f.reverse();
        let mat = neighbour_material(p, h[0]);
        p.add_face(&f, mat);
    }
    Ok((before as u32..p.faces() as u32).collect())
}

fn neighbour_material(p: &Poly, v: u32) -> u32 {
    (0..p.faces()).find(|&f| p.face(f).contains(&v)).map_or(0, |f| p.face_material[f])
}

/// Makes a face from picked vertices (in order round their middle), or
/// fills picked holes; two picked corners of one face split it. Returns
/// new faces.
pub fn make_face(p: &mut Poly, verts: &[bool], edges: &HashSet<Edge>) -> Result<Vec<u32>, String> {
    if !picked_holes(p, edges).is_empty() {
        return fill(p, edges, false);
    }
    let vs: Vec<u32> = (0..p.positions.len() as u32).filter(|&v| verts.get(v as usize).copied().unwrap_or(false)).collect();
    if vs.len() == 2 {
        let shared = (0..p.faces()).any(|f| vs.iter().all(|v| p.face(f).contains(v)));
        if shared {
            connect_vertices(p, verts);
            return Ok(Vec::new());
        }
        return Err("two vertices make a face only when they are corners of the same face (then the face is split)".into());
    }
    if vs.len() < 3 {
        return Err("pick three or more vertices (or the edges round a hole)".into());
    }
    let pts: Vec<V3> = vs.iter().map(|&v| p.positions[v as usize]).collect();
    let c = scale(pts.iter().fold([0.0; 3], |a, b| add(a, *b)), 1.0 / pts.len() as f32);
    // Best plane by Newell's method over the points round their middle.
    let mut nrm = [0.0; 3];
    let mut order: Vec<usize> = (0..pts.len()).collect();
    let ref_dir = normalise(sub(pts[0], c));
    let mut n_est = [0.0f32; 3];
    for i in 0..pts.len() {
        n_est = add(n_est, cross(sub(pts[i], c), sub(pts[(i + 1) % pts.len()], c)));
    }
    if length(n_est) < 1e-9 {
        n_est = [0.0, 0.0, 1.0];
    }
    let n0 = normalise(n_est);
    let bdir = cross(n0, ref_dir);
    order.sort_by(|&a, &b| {
        let ang = |i: usize| {
            let d = sub(pts[i], c);
            dot(d, bdir).atan2(dot(d, ref_dir))
        };
        ang(a).total_cmp(&ang(b))
    });
    let mut face: Vec<u32> = order.iter().map(|&i| vs[i]).collect();
    for i in 0..face.len() {
        let (a, b) = (p.positions[face[i] as usize], p.positions[face[(i + 1) % face.len()] as usize]);
        nrm = add(nrm, cross(sub(a, c), sub(b, c)));
    }
    // Wind against faces it shares an edge with; else outward from the
    // mesh's middle.
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for f in 0..p.faces() {
        let fc = p.face(f);
        for i in 0..fc.len() {
            directed.insert((fc[i], fc[(i + 1) % fc.len()]));
        }
    }
    let same = (0..face.len()).filter(|&i| directed.contains(&(face[i], face[(i + 1) % face.len()]))).count();
    let opposite = (0..face.len()).filter(|&i| directed.contains(&(face[(i + 1) % face.len()], face[i]))).count();
    let flip = if same + opposite > 0 {
        same > opposite
    } else {
        let (lo, hi) = p.bounds();
        let mid = lerp(lo, hi, 0.5);
        dot(nrm, sub(c, mid)) < 0.0
    };
    if flip {
        face.reverse();
    }
    let mat = neighbour_material(p, face[0]);
    Ok(vec![p.add_face(&face, mat) as u32])
}

/// Joins picked loops of open edges in pairs with bands of faces.
/// Returns the new faces.
pub fn bridge_loops(p: &mut Poly, edges: &HashSet<Edge>, segments: usize, twist: i32) -> Result<Vec<u32>, String> {
    let holes = picked_holes(p, edges);
    if holes.len() < 2 {
        return Err(format!("bridging needs two loops of open edges picked; {} found (pick the rims of two holes, or two face groups in face mode)", holes.len()));
    }
    let before = p.faces();
    let mut left: Vec<Vec<u32>> = holes;
    while left.len() >= 2 {
        let a = left.remove(0);
        let ca = centre_of(p, &a);
        let (k, _) = left.iter().enumerate().map(|(i, l)| (i, length(sub(centre_of(p, l), ca)))).min_by(|x, y| x.1.total_cmp(&y.1)).unwrap();
        let b = left.remove(k);
        edit::bridge(p, &a, &b, segments, twist);
    }
    Ok((before as u32..p.faces() as u32).collect())
}

/// Gives picked faces thickness: they move `thickness` along their
/// normals (negative: back), the old surface stays as the back, and the
/// rim is closed. Returns the moved faces.
pub fn solidify_faces(p: &mut Poly, pick: &[bool], thickness: f32) -> Vec<u32> {
    let backs: Vec<(Vec<u32>, u32, u32, bool)> = (0..p.faces()).filter(|&f| pick.get(f).copied().unwrap_or(false)).map(|f| (p.face(f).to_vec(), p.face_material[f], p.face_set[f], p.face_smooth[f])).collect();
    let n0 = p.faces();
    let caps = edit::extrude(p, pick, thickness, None, false);
    for (mut vs, m, s, sm) in backs {
        vs.reverse();
        p.add_face_like(&vs, None, m, s, sm);
    }
    if thickness < 0.0 {
        // Pushed back: the shell is inside out until every face of it
        // turns round (caps, sides and backs).
        for &f in &caps {
            p.flip_face(f as usize);
        }
        for f in n0..p.faces() {
            p.flip_face(f);
        }
    }
    caps
}

/// Turns picked faces to point outward (or inward) consistently.
pub fn recalc_normals(p: &mut Poly, pick: &[bool], inside: bool) -> usize {
    let mut c = p.clone();
    c.orient_outward();
    let mut n = 0;
    for f in 0..p.faces() {
        if !pick.get(f).copied().unwrap_or(false) {
            continue;
        }
        let flipped = c.face(f) != p.face(f);
        if flipped != inside {
            p.flip_face(f);
            n += 1;
        }
    }
    n
}

/// Flips picked faces.
pub fn flip(p: &mut Poly, pick: &[bool]) -> usize {
    let mut n = 0;
    for f in 0..p.faces() {
        if pick.get(f).copied().unwrap_or(false) {
            p.flip_face(f);
            n += 1;
        }
    }
    n
}

/// Turns a profile round `axis` (0 x, 1 y, 2 z) through `centre`.
pub fn spin_about(profile: &[V3], centre: V3, axis: usize, degrees: f32, steps: usize) -> Poly {
    let local: Vec<V3> = profile.iter().map(|q| sub(*q, centre)).collect();
    let mut s = edit::spin(&local, degrees, steps, axis);
    s.map_positions(|q| add(q, centre));
    s
}

/// A profile turned round `axis` through `centre` while it climbs
/// `height` per turn (threads, springs, spiral stairs).
pub fn screw(profile: &[V3], centre: V3, axis: usize, turns: f32, steps: usize, height: f32) -> Poly {
    let steps = steps.max(3);
    let total = ((turns.abs() * steps as f32).round() as usize).max(1);
    let mut p = Poly::new();
    let rot = |q: V3, a: f32| -> V3 {
        let (s, c) = a.sin_cos();
        match axis {
            0 => [q[0], q[1] * c - q[2] * s, q[1] * s + q[2] * c],
            2 => [q[0] * c - q[1] * s, q[0] * s + q[1] * c, q[2]],
            _ => [q[0] * c + q[2] * s, q[1], -q[0] * s + q[2] * c],
        }
    };
    let dir = if turns < 0.0 { -1.0 } else { 1.0 };
    let mut cols: Vec<Vec<u32>> = Vec::new();
    for k in 0..=total {
        let a = dir * std::f32::consts::TAU * k as f32 / steps as f32;
        let rise = height * k as f32 / steps as f32;
        let col: Vec<u32> = profile
            .iter()
            .map(|q| {
                let mut r = rot(sub(*q, centre), a);
                r[axis.min(2)] += rise;
                p.add_vertex(add(r, centre))
            })
            .collect();
        cols.push(col);
    }
    for k in 0..total {
        for j in 0..profile.len().saturating_sub(1) {
            p.add_face(&[cols[k][j], cols[k + 1][j], cols[k + 1][j + 1], cols[k][j + 1]], 0);
        }
    }
    p.ensure_materials();
    p
}

/// Adds `other` into `p` and welds its vertices onto `p`'s picked ones
/// where they sit on the same point (a spin's first column on the
/// profile it was made from). Returns the added faces.
pub fn merge_welded(p: &mut Poly, other: &Poly, onto: &[u32]) -> Vec<u32> {
    let base = p.positions.len() as u32;
    let first_face = p.faces() as u32;
    let mut o = other.clone();
    o.materials.clear();
    let mats = p.materials.clone();
    for m in &mut o.face_material {
        *m = 0;
    }
    if !p.has_uvs() && p.faces() > 0 {
        o.uvs.clear();
    }
    p.merge(&o);
    p.materials = mats;
    p.ensure_materials();
    let (lo, hi) = p.bounds();
    let eps = length(sub(hi, lo)).max(1.0) * 1e-5;
    let mut map: Vec<u32> = (0..p.positions.len() as u32).collect();
    for v in base..p.positions.len() as u32 {
        let q = p.positions[v as usize];
        if let Some(&u) = onto.iter().find(|&&u| length(sub(p.positions[u as usize], q)) <= eps) {
            map[v as usize] = u;
        }
    }
    let n_faces = p.faces() as u32;
    let marks: Vec<bool> = (0..p.faces()).map(|f| f as u32 >= first_face).collect();
    let after = keep_face_pick(p, &marks, |p| edit::remap_vertices(p, &map));
    let _ = n_faces;
    // Wind the new faces against the old ones they now share edges with.
    let mut old_dir: HashSet<(u32, u32)> = HashSet::new();
    for f in 0..p.faces() {
        if !after[f] {
            let c = p.face(f);
            for i in 0..c.len() {
                old_dir.insert((c[i], c[(i + 1) % c.len()]));
            }
        }
    }
    let (mut same, mut against) = (0, 0);
    for f in 0..p.faces() {
        if after[f] {
            let c = p.face(f);
            for i in 0..c.len() {
                let (a, b) = (c[i], c[(i + 1) % c.len()]);
                if old_dir.contains(&(a, b)) {
                    same += 1;
                } else if old_dir.contains(&(b, a)) {
                    against += 1;
                }
            }
        }
    }
    if same > against {
        for f in 0..p.faces() {
            if after[f] {
                p.flip_face(f);
            }
        }
    }
    after.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i as u32).collect()
}

/// Cuts picked faces along a plane; `clear`: Some(true) deletes the part
/// of the pick in front of the plane (where its normal points), Some(false)
/// behind; `fill` closes the cut. Returns the new edges' count.
pub fn bisect_in(p: &mut Poly, pick: &[bool], plane: Plane, clear: Option<bool>, fill_cut: bool) -> usize {
    let mut n = 0;
    let after = keep_face_pick(p, pick, |p| {
        let all = marked(p);
        n = edit::knife(p, &all, plane, &|_| true).len();
    });
    if let Some(front) = clear {
        let (lo, hi) = p.bounds();
        let eps = 1e-4 * length(sub(hi, lo)).max(1.0);
        let gone: Vec<bool> = (0..p.faces())
            .map(|f| {
                if !after[f] {
                    return false;
                }
                let s: f32 = p.face(f).iter().map(|&v| plane.side(p.positions[v as usize])).sum::<f32>() / p.face(f).len() as f32;
                if front { s > eps * 0.5 } else { s < -eps * 0.5 }
            })
            .collect();
        edit::delete_faces(p, &gone);
        if fill_cut {
            edit::fill_holes(p, usize::MAX);
        }
    }
    n
}

/// Moves picked vertices by a random amount up to `amount` (repeatable by
/// `seed`).
pub fn randomize(p: &mut Poly, verts: &[bool], amount: f32, seed: u32) {
    let mut s = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    };
    for v in 0..p.positions.len() {
        if verts.get(v).copied().unwrap_or(false) {
            let d = [next(), next(), next()];
            p.positions[v] = add(p.positions[v], scale(d, amount));
        }
    }
}

/// Mirrors the picked vertices in place across `axis` through `pivot`;
/// faces wholly picked are turned round so they still face out.
pub fn mirror_in_place(p: &mut Poly, verts: &[bool], axis: usize, pivot: V3) {
    let a = axis.min(2);
    for v in 0..p.positions.len() {
        if verts.get(v).copied().unwrap_or(false) {
            p.positions[v][a] = 2.0 * pivot[a] - p.positions[v][a];
        }
    }
    for f in 0..p.faces() {
        if p.face(f).iter().all(|&v| verts.get(v as usize).copied().unwrap_or(false)) {
            p.flip_face(f);
        }
    }
}

/// Packs the texture layout's pieces into the square without overlap,
/// keeping their relative sizes. Returns the pieces packed.
pub fn pack_uvs(p: &mut Poly, margin: f32) -> usize {
    if !p.has_uvs() {
        return 0;
    }
    // Pieces: faces joined by an edge whose two ends match in the layout.
    let nf = p.faces();
    let mut parent: Vec<usize> = (0..nf).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut seen: HashMap<(u32, u32), (usize, [f32; 2], [f32; 2])> = HashMap::new();
    let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
    for f in 0..nf {
        let c = p.face(f);
        let r = p.face_range(f);
        let u = &p.uvs[r];
        for i in 0..c.len() {
            let j = (i + 1) % c.len();
            let (a, b) = (c[i], c[j]);
            if let Some(&(g, ub, ua)) = seen.get(&(b, a)) {
                if close(ua, u[i]) && close(ub, u[j]) {
                    let (x, y) = (find(&mut parent, f), find(&mut parent, g));
                    if x != y {
                        parent[x] = y;
                    }
                }
            } else {
                seen.insert((a, b), (f, u[i], u[j]));
            }
        }
    }
    let mut pieces: HashMap<usize, Vec<usize>> = HashMap::new();
    for f in 0..nf {
        let r = find(&mut parent, f);
        pieces.entry(r).or_default().push(f);
    }
    let mut list: Vec<Vec<usize>> = pieces.into_values().collect();
    list.sort();
    // Each piece scaled so texture density matches surface area.
    struct Piece {
        faces: Vec<usize>,
        lo: [f32; 2],
        w: f32,
        h: f32,
        k: f32,
        at: [f32; 2],
    }
    let mut ps: Vec<Piece> = Vec::new();
    for faces in list {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        let mut uv_area = 0.0f32;
        let mut area = 0.0f32;
        for &f in &faces {
            let r = p.face_range(f);
            let u = &p.uvs[r];
            for q in u {
                lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                hi = [hi[0].max(q[0]), hi[1].max(q[1])];
            }
            for t in triangulate(&p.positions, p.face(f)) {
                let (a, b, c) = (u[t[0]], u[t[1]], u[t[2]]);
                uv_area += ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() * 0.5;
            }
            area += p.face_area(f);
        }
        let k = if uv_area > 1e-12 { (area / uv_area).sqrt() } else { 1.0 };
        ps.push(Piece { faces, lo, w: (hi[0] - lo[0]).max(1e-6) * k, h: (hi[1] - lo[1]).max(1e-6) * k, k, at: [0.0; 2] });
    }
    if ps.is_empty() {
        return 0;
    }
    let total: f32 = ps.iter().map(|q| q.w * q.h).sum();
    let gap = margin.max(0.0) * total.sqrt();
    // Shelves of a width near the square root of the total.
    let shelf = total.sqrt() * 1.15 + gap * 2.0;
    let mut order: Vec<usize> = (0..ps.len()).collect();
    order.sort_by(|&a, &b| ps[b].h.total_cmp(&ps[a].h));
    let (mut x, mut y, mut row_h, mut width) = (gap, gap, 0.0f32, 0.0f32);
    for i in order {
        let (w, h) = (ps[i].w, ps[i].h);
        if x + w + gap > shelf && x > gap {
            x = gap;
            y += row_h + gap;
            row_h = 0.0;
        }
        ps[i].at = [x, y];
        x += w + gap;
        row_h = row_h.max(h);
        width = width.max(x);
    }
    let height = y + row_h + gap;
    let fit = 1.0 / width.max(height).max(1e-9);
    for q in &ps {
        for &f in &q.faces {
            let r = p.face_range(f);
            for k in r {
                let u = p.uvs[k];
                p.uvs[k] = [((u[0] - q.lo[0]) * q.k + q.at[0]) * fit, ((u[1] - q.lo[1]) * q.k + q.at[1]) * fit];
            }
        }
    }
    ps.len()
}

/// Gives every picked face the whole square (quads corner to corner,
/// other faces fitted by their own outline).
pub fn reset_uvs(p: &mut Poly, pick: &[bool]) {
    if !p.has_uvs() {
        p.uvs = vec![[0.0, 0.0]; p.corners.len()];
    }
    for f in 0..p.faces() {
        if !pick.get(f).copied().unwrap_or(false) {
            continue;
        }
        let n = p.face(f).len();
        let r = p.face_range(f);
        for (k, i) in r.enumerate() {
            p.uvs[i] = if n == 4 {
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]][k]
            } else {
                let a = std::f32::consts::TAU * k as f32 / n as f32;
                [0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()]
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;
    use crate::select::Faces;

    pub(crate) fn closed(p: &Poly) -> bool {
        Topology::of(p).boundary_edges().is_empty() && Topology::of(p).edge_faces.values().all(|f| f.len() == 2)
    }

    pub(crate) fn volume(p: &Poly) -> f32 {
        super::volume(p)
    }

    fn outward(p: &Poly) -> bool {
        let mut q = p.clone();
        q.orient_outward();
        q == *p
    }

    #[test]
    fn duplicate_split_and_separate() {
        let mut c = prim::cube([2.0; 3], 1);
        let top = Faces::named("top").unwrap().mask(&c);
        let copies = duplicate(&mut c, &top);
        assert_eq!(copies.len(), 1);
        assert_eq!(c.faces(), 7);
        assert_eq!(c.positions.len(), 12);
        let mut d = prim::cube([2.0; 3], 1);
        let top = Faces::named("top").unwrap().mask(&d);
        assert_eq!(split(&mut d, &top), 4);
        assert_eq!(Topology::of(&d).boundary_edges().len(), 8);
        let mut e = prim::cube([2.0; 3], 2);
        let top = Faces::named("top").unwrap().mask(&e);
        let piece = separate(&mut e, &top);
        assert_eq!(piece.faces(), 4);
        assert_eq!(e.faces(), 20);
        assert_eq!(loose_parts(&e).len(), 1);
        let mut two = prim::cube([2.0; 3], 1);
        let mut other = prim::cube([2.0; 3], 1);
        other.map_positions(|q| add(q, [5.0, 0.0, 0.0]));
        two.merge(&other);
        assert_eq!(loose_parts(&two).len(), 2);
    }

    #[test]
    fn dissolving_vertices_edges_and_faces() {
        // A 2×2 grid plane: dissolving the middle vertex leaves one face.
        let mut g = prim::plane(2.0, 2.0, 2, 2);
        let mid = g.positions.iter().position(|q| q[0].abs() < 1e-5 && q[2].abs() < 1e-5).unwrap();
        let mut picked = vec![false; g.positions.len()];
        picked[mid] = true;
        dissolve_vertices(&mut g, &picked);
        assert_eq!(g.faces(), 1);
        assert_eq!(g.face(0).len(), 8, "the rim keeps its points: {:?}", g.face(0));
        let mut c = prim::cube([2.0; 3], 2);
        let top = Faces::named("top").unwrap().mask(&c);
        dissolve_faces(&mut c, &top);
        assert_eq!(c.faces(), 21);
        assert!(closed(&c));
        assert!((volume(&c) - 8.0).abs() < 1e-3);
    }

    #[test]
    fn merges() {
        let mut c = prim::cube([2.0; 3], 1);
        let top: Vec<u32> = (0..8).filter(|&v| c.positions[v as usize][1] < 0.0).collect();
        let at = centre_of(&c, &top);
        let kept = merge_groups(&mut c, &[(top.clone(), at)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(c.positions.len(), 5, "a pyramid");
        assert_eq!(c.faces(), 5);
        assert!(closed(&c));
        let mut d = prim::cube([2.0; 3], 1);
        let base = d.clone();
        d.merge(&base);
        let all = vec![true; d.positions.len()];
        assert_eq!(merge_by_distance_in(&mut d, &all, 0.01), 8);
    }

    #[test]
    fn vertex_bevel_cuts_a_corner() {
        let mut c = prim::cube([2.0; 3], 1);
        let mut picked = vec![false; 8];
        picked[0] = true;
        assert_eq!(bevel_vertices(&mut c, &picked, 0.5, 1, 0.5), 1);
        assert_eq!(c.faces(), 7);
        assert_eq!(c.positions.len(), 10);
        assert!(closed(&c));
        assert!(outward(&c));
        assert!(volume(&c) < 8.0 && volume(&c) > 7.9);
        let mut r = prim::cube([2.0; 3], 1);
        assert_eq!(bevel_vertices(&mut r, &[true; 8], 0.4, 3, 0.5), 8);
        assert!(closed(&r));
        assert!(outward(&r));
    }

    #[test]
    fn edge_slide_moves_a_loop_along_its_rails() {
        let mut c = prim::cylinder(1.0, 1.0, 2.0, 8, 2, prim::Cap::Ngon);
        let ring: Vec<Edge> = c.edges().into_iter().filter(|e| c.positions[e.0 as usize][1].abs() < 1e-4 && c.positions[e.1 as usize][1].abs() < 1e-4).collect();
        assert_eq!(ring.len(), 8);
        let moved = edge_slide(&mut c, &ring, 0.5).unwrap();
        assert_eq!(moved, 8);
        let ys: Vec<f32> = ring.iter().map(|e| c.positions[e.0 as usize][1]).collect();
        assert!(ys.iter().all(|y| (y.abs() - 0.5).abs() < 1e-4), "{ys:?}");
        let first = ys[0];
        assert!(ys.iter().all(|y| (y - first).abs() < 1e-4), "all one way: {ys:?}");
    }

    #[test]
    fn subdividing_picked_edges_and_faces() {
        let mut c = prim::cube([2.0; 3], 1);
        let all: HashSet<Edge> = c.edges().into_iter().collect();
        subdivide_edges(&mut c, &all, 1);
        assert_eq!(c.faces(), 24);
        assert_eq!(c.positions.len(), 26);
        assert!(closed(&c));
        assert!(outward(&c));
        // One face of the cube: its quads become a 3×3 grid, its
        // neighbours take the new points.
        let mut d = prim::cube([2.0; 3], 1);
        let f = 0;
        let fe: HashSet<Edge> = (0..4).map(|i| ekey(d.face(f)[i], d.face(f)[(i + 1) % 4])).collect();
        subdivide_edges(&mut d, &fe, 2);
        assert_eq!(d.faces(), 5 + 9);
        assert!(closed(&d));
        assert!(outward(&d));
        // Triangles split into a triangle grid.
        let mut t = Poly::from_faces(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[vec![0, 1, 2]]);
        let te: HashSet<Edge> = t.edges().into_iter().collect();
        subdivide_edges(&mut t, &te, 2);
        assert_eq!(t.faces(), 9);
        assert!(t.face_normal(4)[2] > 0.99);
    }

    #[test]
    fn connect_and_make_face() {
        let mut q = prim::plane(2.0, 2.0, 1, 1);
        let first = q.face(0)[0] as usize;
        let opposite = q.face(0)[2] as usize;
        let mut picked = vec![false; q.positions.len()];
        picked[first] = true;
        picked[opposite] = true;
        let e = connect_vertices(&mut q, &picked);
        assert_eq!(e.len(), 1);
        assert_eq!(q.faces(), 2);
        // A hole filled by its rim.
        let mut c = prim::cube([2.0; 3], 1);
        let top = Faces::named("top").unwrap().mask(&c);
        edit::delete_faces(&mut c, &top);
        let rim: HashSet<Edge> = Topology::of(&c).boundary_edges().into_iter().collect();
        let made = make_face(&mut c, &[false; 8], &rim).unwrap();
        assert_eq!(made.len(), 1);
        assert!(closed(&c));
        assert!(outward(&c));
        // Loose points make a face winding against its neighbours.
        let mut d = prim::cube([2.0; 3], 1);
        let top_d = Faces::named("top").unwrap().mask(&d);
        edit::delete_faces(&mut d, &top_d);
        let top_v: Vec<bool> = d.positions.iter().map(|q| q[1] < 0.0).collect();
        make_face(&mut d, &top_v, &HashSet::new()).unwrap();
        assert!(closed(&d));
        assert!(outward(&d));
    }

    #[test]
    fn bridge_and_fill_loops() {
        // A tube's two open ends bridged... into a torus-like shell; a
        // grid fill on an even rim.
        let mut c = prim::cylinder(1.0, 1.0, 2.0, 8, 1, prim::Cap::None);
        let rims: HashSet<Edge> = Topology::of(&c).boundary_edges().into_iter().collect();
        assert_eq!(rims.len(), 16);
        let mut f = c.clone();
        let made = fill(&mut f, &rims, true).unwrap();
        assert!(made.len() > 2);
        assert!(closed(&f));
        assert!(outward(&f));
        let mut cut = prim::cube([2.0; 3], 1);
        let mut other = prim::cube([2.0; 3], 1);
        other.map_positions(|q| add(q, [4.0, 0.0, 0.0]));
        cut.merge(&other);
        let right = Faces::Side { axis: 0, at: 0.9, above: true }.mask(&cut);
        let left_of_second = Faces::Side { axis: 0, at: 3.1, above: false }.mask(&cut);
        let pick: Vec<bool> = (0..cut.faces()).map(|f| right[f] && left_of_second[f]).collect();
        edit::delete_faces(&mut cut, &pick);
        let holes: HashSet<Edge> = Topology::of(&cut).boundary_edges().into_iter().collect();
        let made = bridge_loops(&mut cut, &holes, 2, 0).unwrap();
        assert_eq!(made.len(), 8);
        assert!(closed(&cut));
        assert!(outward(&cut));
        let _ = &mut c;
    }

    #[test]
    fn solidify_normals_and_spin() {
        let mut sheet = prim::plane(2.0, 2.0, 2, 2);
        let all = vec![true; sheet.faces()];
        solidify_faces(&mut sheet, &all, 0.2);
        assert!(closed(&sheet));
        assert!(outward(&sheet), "both sides face out");
        let mut c = prim::cube([2.0; 3], 1);
        c.flip_face(1);
        c.flip_face(3);
        assert_eq!(recalc_normals(&mut c, &[true; 6], false), 2);
        assert!(outward(&c));
        assert_eq!(flip(&mut c, &[true; 6]), 6);
        assert_eq!(recalc_normals(&mut c, &[true; 6], false), 6);
        // A spun profile closes into a vase.
        let prof = vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [0.0, -1.0, 0.0]];
        let v = spin_about(&prof, [0.0; 3], 1, 360.0, 12);
        assert!(closed(&v));
        assert!(volume(&v) > 5.0);
        let s = screw(&[[1.0, 0.0, 0.0], [1.5, 0.0, 0.0]], [0.0; 3], 1, 2.0, 12, 1.0);
        assert_eq!(s.faces(), 24);
        let (lo, hi) = s.bounds();
        assert!((hi[1] - lo[1] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn bisect_within_a_pick_and_pack() {
        let mut c = prim::cube([2.0; 3], 1);
        let all = vec![true; 6];
        bisect_in(&mut c, &all, Plane { at: [0.0; 3], normal: [1.0, 0.0, 0.0] }, Some(true), true);
        assert!(closed(&c));
        let (lo, hi) = c.bounds();
        assert!(hi[0].abs() < 1e-4 && (lo[0] + 1.0).abs() < 1e-4);
        assert!((volume(&c) - 4.0).abs() < 1e-3);
        let mut u = prim::cube([2.0; 3], 2);
        crate::uv::unwrap(&mut u, &crate::uv::Unwrap::default());
        let pieces = pack_uvs(&mut u, 0.01);
        assert!(pieces >= 1);
        assert!(u.uvs.iter().all(|q| q[0] >= -1e-4 && q[0] <= 1.0001 && q[1] >= -1e-4 && q[1] <= 1.0001));
        let every = vec![true; u.faces()];
        reset_uvs(&mut u, &every);
        assert_eq!(u.uvs[0], [0.0, 0.0]);
    }

    #[test]
    fn keep_face_pick_follows_rebuilt_faces() {
        let mut c = prim::cube([2.0; 3], 1);
        let top = Faces::named("top").unwrap().mask(&c);
        let after = keep_face_pick(&mut c, &top, |p| edit::triangulate_faces(p, &vec![true; 6]));
        assert_eq!(after.iter().filter(|b| **b).count(), 2);
        assert!(c.face_set.iter().all(|s| s & MARK == 0));
    }
}
