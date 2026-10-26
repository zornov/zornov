// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Polygon meshes: faces of any number of corners (quads above all), with
//! the edge data modeling needs — creases for subdivision, sharp edges for
//! shading, seams for unwrapping — and the conversion to the triangles
//! Studio draws.

use std::collections::{HashMap, HashSet};
use studio_mesh::{Material, Mesh, V3, add, cross, dot, length, normalise, scale, sub};

/// An edge by its two vertices, the smaller first.
pub type Edge = (u32, u32);

pub fn ekey(a: u32, b: u32) -> Edge {
    if a < b { (a, b) } else { (b, a) }
}

/// A polygon mesh. Faces list their corners counter-clockwise seen from
/// outside (so `cross(b - a, c - a)` points out, as in [`Mesh`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Poly {
    pub positions: Vec<V3>,
    /// Every face's corner vertices, one face after another.
    pub corners: Vec<u32>,
    /// Where each face starts in `corners`; one longer than the faces.
    pub starts: Vec<u32>,
    /// Texture coordinates per corner (as long as `corners`), or empty.
    pub uvs: Vec<[f32; 2]>,
    pub face_material: Vec<u32>,
    /// Face sets: groups of faces a sculptor or modeler names and hides,
    /// masks or picks together.
    pub face_set: Vec<u32>,
    /// Smooth-shaded faces (flat ones show their facets).
    pub face_smooth: Vec<bool>,
    /// Subdivision creases: 0 smooth – 1 a sharp edge (between: how many
    /// levels stay sharp, 0.25 a level).
    pub creases: HashMap<Edge, f32>,
    /// Edges where shading breaks (hard edges).
    pub sharp: HashSet<Edge>,
    /// Edges where the texture layout is cut.
    pub seams: HashSet<Edge>,
    pub materials: Vec<Material>,
}

impl Poly {
    pub fn new() -> Poly {
        Poly { starts: vec![0], ..Default::default() }
    }

    /// A mesh from positions and faces (corner lists).
    pub fn from_faces(positions: Vec<V3>, faces: &[Vec<u32>]) -> Poly {
        let mut p = Poly { positions, ..Poly::new() };
        for f in faces {
            p.add_face(f, 0);
        }
        p
    }

    pub fn faces(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    pub fn face(&self, f: usize) -> &[u32] {
        &self.corners[self.starts[f] as usize..self.starts[f + 1] as usize]
    }

    pub fn face_range(&self, f: usize) -> std::ops::Range<usize> {
        self.starts[f] as usize..self.starts[f + 1] as usize
    }

    pub fn has_uvs(&self) -> bool {
        !self.uvs.is_empty() && self.uvs.len() == self.corners.len()
    }

    pub fn face_uvs(&self, f: usize) -> Option<&[[f32; 2]]> {
        self.has_uvs().then(|| &self.uvs[self.face_range(f)])
    }

    /// Adds a face; returns its number.
    pub fn add_face(&mut self, verts: &[u32], material: u32) -> usize {
        self.add_face_like(verts, None, material, 0, true)
    }

    /// Adds a face with its texture coordinates, material, face set and
    /// shading.
    pub fn add_face_like(&mut self, verts: &[u32], uvs: Option<&[[f32; 2]]>, material: u32, set: u32, smooth: bool) -> usize {
        if self.starts.is_empty() {
            self.starts.push(0);
        }
        let had_uvs = self.has_uvs();
        self.corners.extend_from_slice(verts);
        if had_uvs || (self.faces() == 0 && uvs.is_some()) {
            match uvs {
                Some(u) if u.len() == verts.len() => self.uvs.extend_from_slice(u),
                _ => self.uvs.extend(std::iter::repeat_n([0.0, 0.0], verts.len())),
            }
        }
        self.starts.push(self.corners.len() as u32);
        self.face_material.push(material);
        self.face_set.push(set);
        self.face_smooth.push(smooth);
        self.faces() - 1
    }

    /// Adds a vertex; returns its number.
    pub fn add_vertex(&mut self, p: V3) -> u32 {
        self.positions.push(p);
        (self.positions.len() - 1) as u32
    }

    /// Newell's normal of face `f` (unit; works for bent and concave faces).
    pub fn face_normal(&self, f: usize) -> V3 {
        normalise(self.face_area_vector(f))
    }

    /// The face's normal scaled by twice its area.
    pub fn face_area_vector(&self, f: usize) -> V3 {
        let v = self.face(f);
        let mut n = [0.0f32; 3];
        for i in 0..v.len() {
            let a = self.positions[v[i] as usize];
            let b = self.positions[v[(i + 1) % v.len()] as usize];
            n[0] += (a[1] - b[1]) * (a[2] + b[2]);
            n[1] += (a[2] - b[2]) * (a[0] + b[0]);
            n[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
        n
    }

    pub fn face_area(&self, f: usize) -> f32 {
        0.5 * length(self.face_area_vector(f))
    }

    pub fn face_centre(&self, f: usize) -> V3 {
        let v = self.face(f);
        let mut c = [0.0; 3];
        for &i in v {
            c = add(c, self.positions[i as usize]);
        }
        scale(c, 1.0 / v.len().max(1) as f32)
    }

    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if self.positions.is_empty() { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }

    /// Every edge of every face (each once).
    pub fn edges(&self) -> Vec<Edge> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for f in 0..self.faces() {
            let v = self.face(f);
            for i in 0..v.len() {
                let e = ekey(v[i], v[(i + 1) % v.len()]);
                if seen.insert(e) {
                    out.push(e);
                }
            }
        }
        out
    }

    /// Keeps only the faces `keep` says yes to (vertices stay; see
    /// [`Poly::compact`]).
    pub fn retain_faces(&mut self, keep: impl Fn(usize) -> bool) {
        let mut out = Poly { positions: std::mem::take(&mut self.positions), creases: std::mem::take(&mut self.creases), sharp: std::mem::take(&mut self.sharp), seams: std::mem::take(&mut self.seams), materials: std::mem::take(&mut self.materials), ..Poly::new() };
        let uvs = self.has_uvs();
        if uvs {
            out.uvs = Vec::new();
        }
        for f in 0..self.faces() {
            if keep(f) {
                let r = self.face_range(f);
                out.corners.extend_from_slice(&self.corners[r.clone()]);
                if uvs {
                    out.uvs.extend_from_slice(&self.uvs[r]);
                }
                out.starts.push(out.corners.len() as u32);
                out.face_material.push(self.face_material[f]);
                out.face_set.push(self.face_set[f]);
                out.face_smooth.push(self.face_smooth[f]);
            }
        }
        *self = out;
    }

    /// Drops vertices no face uses and edge flags on edges that are gone.
    pub fn compact(&mut self) {
        let mut map = vec![u32::MAX; self.positions.len()];
        let mut pos = Vec::new();
        for c in &mut self.corners {
            let i = *c as usize;
            if map[i] == u32::MAX {
                map[i] = pos.len() as u32;
                pos.push(self.positions[i]);
            }
            *c = map[i];
        }
        self.positions = pos;
        let remap = |e: &Edge| -> Option<Edge> {
            let (a, b) = (map.get(e.0 as usize)?, map.get(e.1 as usize)?);
            (*a != u32::MAX && *b != u32::MAX).then(|| ekey(*a, *b))
        };
        let live: HashSet<Edge> = self.edges().into_iter().collect();
        self.creases = self.creases.iter().filter_map(|(e, w)| remap(e).map(|k| (k, *w))).filter(|(k, _)| live.contains(k)).collect();
        self.sharp = self.sharp.iter().filter_map(remap).filter(|k| live.contains(k)).collect();
        self.seams = self.seams.iter().filter_map(remap).filter(|k| live.contains(k)).collect();
    }

    /// Faces with a repeated corner in a row lose it; faces left with
    /// fewer than three corners go.
    pub fn clean(&mut self) {
        let uvs = self.has_uvs();
        let mut out = Poly { positions: std::mem::take(&mut self.positions), creases: std::mem::take(&mut self.creases), sharp: std::mem::take(&mut self.sharp), seams: std::mem::take(&mut self.seams), materials: std::mem::take(&mut self.materials), ..Poly::new() };
        for f in 0..self.faces() {
            let r = self.face_range(f);
            let mut vs: Vec<u32> = Vec::new();
            let mut us: Vec<[f32; 2]> = Vec::new();
            for k in r.clone() {
                let v = self.corners[k];
                if vs.last() != Some(&v) {
                    vs.push(v);
                    if uvs {
                        us.push(self.uvs[k]);
                    }
                }
            }
            while vs.len() > 1 && vs.first() == vs.last() {
                vs.pop();
                us.pop();
            }
            if vs.len() >= 3 {
                out.add_face_like(&vs, uvs.then_some(&us[..]), self.face_material[f], self.face_set[f], self.face_smooth[f]);
            }
        }
        if !uvs {
            out.uvs.clear();
        }
        *self = out;
    }

    /// Adds another mesh's faces (its materials appended after these).
    pub fn merge(&mut self, other: &Poly) {
        let base = self.positions.len() as u32;
        let mbase = self.materials.len() as u32;
        let had = self.has_uvs() || self.faces() == 0;
        if !other.has_uvs() && self.has_uvs() {
            // keep ours; theirs get zeros
        } else if other.has_uvs() && !self.has_uvs() && self.faces() > 0 {
            self.uvs = vec![[0.0, 0.0]; self.corners.len()];
        }
        self.positions.extend_from_slice(&other.positions);
        for f in 0..other.faces() {
            let v: Vec<u32> = other.face(f).iter().map(|i| i + base).collect();
            let uv = other.face_uvs(f);
            self.add_face_like(&v, uv, other.face_material[f] + mbase, other.face_set[f], other.face_smooth[f]);
        }
        let _ = had;
        for (e, w) in &other.creases {
            self.creases.insert((e.0 + base, e.1 + base), *w);
        }
        self.sharp.extend(other.sharp.iter().map(|e| (e.0 + base, e.1 + base)));
        self.seams.extend(other.seams.iter().map(|e| (e.0 + base, e.1 + base)));
        self.materials.extend(other.materials.iter().cloned());
    }

    /// Moves every vertex.
    pub fn map_positions(&mut self, f: impl Fn(V3) -> V3) {
        for p in &mut self.positions {
            *p = f(*p);
        }
    }

    /// Reverses face `f` (its normal flips).
    pub fn flip_face(&mut self, f: usize) {
        let r = self.face_range(f);
        self.corners[r.clone()].reverse();
        if self.has_uvs() {
            self.uvs[r].reverse();
        }
    }

    /// The material list, made to cover every face's material number.
    pub fn ensure_materials(&mut self) {
        let need = self.face_material.iter().copied().max().map_or(1, |m| m as usize + 1);
        if self.materials.len() < need {
            self.materials.resize_with(need, Material::default);
        }
    }

    /// Every face on material slot 0, one material kept (after merging
    /// pieces that are all one thing).
    pub fn single_material(&mut self) {
        for m in &mut self.face_material {
            *m = 0;
        }
        self.materials.truncate(1);
        self.ensure_materials();
    }

    /// Statistics in words.
    pub fn stats(&self) -> String {
        let quads = (0..self.faces()).filter(|&f| self.face(f).len() == 4).count();
        let tris = (0..self.faces()).filter(|&f| self.face(f).len() == 3).count();
        let (lo, hi) = self.bounds();
        format!(
            "{} faces ({} quads, {} triangles, {} n-gons), {} vertices, {:.0}×{:.0}×{:.0} px",
            self.faces(),
            quads,
            tris,
            self.faces() - quads - tris,
            self.positions.len(),
            hi[0] - lo[0],
            hi[1] - lo[1],
            hi[2] - lo[2]
        )
    }

    /// A polygon mesh from triangles (positions welded; texture
    /// coordinates kept per corner; materials by group).
    pub fn from_mesh(m: &Mesh) -> Poly {
        let (ids, unique) = m.position_ids();
        let mats = m.tri_materials();
        let mut p = Poly { positions: unique, materials: m.materials.clone(), ..Poly::new() };
        let has_uv = m.uvs.len() == m.positions.len();
        for t in 0..m.triangles() {
            let c = m.tri(t);
            let v = [ids[c[0]], ids[c[1]], ids[c[2]]];
            if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
                continue;
            }
            let uv = [m.uvs.get(c[0]).copied().unwrap_or_default(), m.uvs.get(c[1]).copied().unwrap_or_default(), m.uvs.get(c[2]).copied().unwrap_or_default()];
            p.add_face_like(&v, has_uv.then_some(&uv[..]), mats[t], 0, true);
        }
        // Where triangles disagree on normals across an edge, it was hard.
        p.ensure_materials();
        p
    }

    /// Triangles for drawing: n-gons cut into triangles, normals per
    /// corner (smooth across edges below `smooth_angle` degrees unless
    /// marked sharp; flat faces show facets), vertices split where normals
    /// or texture coordinates differ.
    pub fn to_mesh(&self, smooth_angle: f32, mode: NormalMode) -> Mesh {
        let normals = self.corner_normals(smooth_angle, mode);
        let uvs = self.has_uvs();
        let mut out = Mesh::default();
        let mut index: HashMap<(u32, [i32; 3], [i32; 2]), u32> = HashMap::new();
        let mut tri_mats = Vec::new();
        let q = |v: f32, k: f32| (v * k).round() as i32;
        for f in 0..self.faces() {
            let r = self.face_range(f);
            let mut local = Vec::with_capacity(r.len());
            for k in r.clone() {
                let v = self.corners[k];
                let n = normals[k];
                let uv = if uvs { self.uvs[k] } else { [0.0, 0.0] };
                let key = (v, [q(n[0], 4096.0), q(n[1], 4096.0), q(n[2], 4096.0)], [q(uv[0], 65536.0), q(uv[1], 65536.0)]);
                let id = *index.entry(key).or_insert_with(|| {
                    out.positions.push(self.positions[v as usize]);
                    out.normals.push(n);
                    out.uvs.push(uv);
                    (out.positions.len() - 1) as u32
                });
                local.push(id);
            }
            for t in triangulate(&self.positions, self.face(f)) {
                out.indices.extend_from_slice(&[local[t[0]], local[t[1]], local[t[2]]]);
                tri_mats.push(self.face_material[f]);
            }
        }
        out.materials = self.materials.clone();
        if out.materials.is_empty() {
            out.materials.push(Material::default());
        }
        out.regroup(&tri_mats);
        out
    }

    /// The normal at every corner.
    pub fn corner_normals(&self, smooth_angle: f32, mode: NormalMode) -> Vec<V3> {
        let nf = self.faces();
        let fnorm: Vec<V3> = (0..nf).map(|f| self.face_normal(f)).collect();
        let farea: Vec<f32> = (0..nf).map(|f| self.face_area(f)).collect();
        let cos_limit = smooth_angle.clamp(0.0, 180.0).to_radians().cos();
        // Faces around each edge, to see which neighbours share a fan.
        let mut edge_faces: HashMap<Edge, Vec<u32>> = HashMap::new();
        for f in 0..nf {
            let v = self.face(f);
            for i in 0..v.len() {
                edge_faces.entry(ekey(v[i], v[(i + 1) % v.len()])).or_default().push(f as u32);
            }
        }
        let smooth_edge = |e: Edge| -> bool {
            if self.sharp.contains(&e) {
                return false;
            }
            match edge_faces.get(&e).map(Vec::as_slice) {
                Some([a, b]) => self.face_smooth[*a as usize] && self.face_smooth[*b as usize] && dot(fnorm[*a as usize], fnorm[*b as usize]) >= cos_limit,
                _ => false,
            }
        };
        // Corner -> union-find over corners at one vertex joined across
        // smooth edges.
        let mut parent: Vec<u32> = (0..self.corners.len() as u32).collect();
        fn find(p: &mut [u32], mut x: u32) -> u32 {
            while p[x as usize] != x {
                p[x as usize] = p[p[x as usize] as usize];
                x = p[x as usize];
            }
            x
        }
        // The corner of face f at vertex v.
        let corner_at = |f: usize, v: u32| -> Option<usize> { self.face_range(f).find(|&k| self.corners[k] == v) };
        for (e, fs) in &edge_faces {
            if fs.len() == 2 && smooth_edge(*e) {
                for v in [e.0, e.1] {
                    if let (Some(a), Some(b)) = (corner_at(fs[0] as usize, v), corner_at(fs[1] as usize, v)) {
                        let (ra, rb) = (find(&mut parent, a as u32), find(&mut parent, b as u32));
                        if ra != rb {
                            parent[ra as usize] = rb;
                        }
                    }
                }
            }
        }
        let mut sums: Vec<V3> = vec![[0.0; 3]; self.corners.len()];
        let mut corner_face = vec![0u32; self.corners.len()];
        for f in 0..nf {
            let r = self.face_range(f);
            let v = self.face(f);
            for (i, k) in r.clone().enumerate() {
                corner_face[k] = f as u32;
                let p = self.positions[v[i] as usize];
                let a = normalise(sub(self.positions[v[(i + v.len() - 1) % v.len()] as usize], p));
                let b = normalise(sub(self.positions[v[(i + 1) % v.len()] as usize], p));
                let angle = dot(a, b).clamp(-1.0, 1.0).acos();
                let w = match mode {
                    NormalMode::Angle => angle,
                    // Large faces win: flat panels stay flat with soft bevels.
                    NormalMode::Area => angle * farea[f] * farea[f],
                };
                let root = find(&mut parent, k as u32) as usize;
                sums[root] = add(sums[root], scale(fnorm[f], w));
            }
        }
        (0..self.corners.len())
            .map(|k| {
                let f = corner_face[k] as usize;
                if !self.face_smooth[f] {
                    return fnorm[f];
                }
                let root = find(&mut parent, k as u32) as usize;
                let n = normalise(sums[root]);
                if n == [0.0; 3] { fnorm[f] } else { n }
            })
            .collect()
    }

    /// Faces turned so that neighbours agree and the whole points outward
    /// (each connected piece is checked by its signed volume).
    pub fn orient_outward(&mut self) {
        let nf = self.faces();
        let mut edge_faces: HashMap<Edge, Vec<u32>> = HashMap::new();
        for f in 0..nf {
            let v = self.face(f);
            for i in 0..v.len() {
                edge_faces.entry(ekey(v[i], v[(i + 1) % v.len()])).or_default().push(f as u32);
            }
        }
        let dir = |p: &Poly, f: usize, a: u32, b: u32| -> bool {
            let v = p.face(f);
            (0..v.len()).any(|i| v[i] == a && v[(i + 1) % v.len()] == b)
        };
        let mut done = vec![false; nf];
        for seed in 0..nf {
            if done[seed] {
                continue;
            }
            let mut piece = vec![seed];
            done[seed] = true;
            let mut stack = vec![seed];
            while let Some(f) = stack.pop() {
                let v: Vec<u32> = self.face(f).to_vec();
                for i in 0..v.len() {
                    let (a, b) = (v[i], v[(i + 1) % v.len()]);
                    for &g in edge_faces.get(&ekey(a, b)).map(Vec::as_slice).unwrap_or(&[]) {
                        let g = g as usize;
                        if g == f || done[g] {
                            continue;
                        }
                        // A neighbour runs the shared edge the other way.
                        if dir(self, g, a, b) {
                            self.flip_face(g);
                        }
                        done[g] = true;
                        piece.push(g);
                        stack.push(g);
                    }
                }
            }
            let mut vol = 0.0f64;
            for &f in &piece {
                let v = self.face(f);
                let p0 = self.positions[v[0] as usize];
                for i in 1..v.len() - 1 {
                    let (p1, p2) = (self.positions[v[i] as usize], self.positions[v[i + 1] as usize]);
                    vol += dot(p0, cross(p1, p2)) as f64;
                }
            }
            if vol < 0.0 {
                for &f in &piece {
                    self.flip_face(f);
                }
            }
        }
    }
}

/// How smooth normals weigh the faces around a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalMode {
    /// By the angle each face makes at the corner (the usual).
    Angle,
    /// Big faces win (weighted normals): flat panels of a hard-surface
    /// model shade flat right up to their bevels.
    Area,
}

/// Triangles covering a face (corner numbers within the face): a fan for
/// convex faces, ear clipping for concave ones.
pub fn triangulate(positions: &[V3], face: &[u32]) -> Vec<[usize; 3]> {
    let n = face.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    let p: Vec<V3> = face.iter().map(|&i| positions[i as usize]).collect();
    let mut normal = [0.0f32; 3];
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
        normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
        normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let normal = normalise(normal);
    if n == 4 {
        // Split a quad along its shorter diagonal (unless that makes it
        // fold over).
        let d02 = length(sub(p[2], p[0]));
        let d13 = length(sub(p[3], p[1]));
        let ok = |a: usize, b: usize, c: usize| dot(cross(sub(p[b], p[a]), sub(p[c], p[a])), normal) > 0.0;
        let split02 = ok(0, 1, 2) && ok(0, 2, 3);
        let split13 = ok(0, 1, 3) && ok(1, 2, 3);
        return if (d02 <= d13 && split02) || !split13 { vec![[0, 1, 2], [0, 2, 3]] } else { vec![[0, 1, 3], [1, 2, 3]] };
    }
    // Project onto the face's plane.
    let u = normalise(if normal[0].abs() < 0.9 { cross(normal, [1.0, 0.0, 0.0]) } else { cross(normal, [0.0, 1.0, 0.0]) });
    let v = cross(normal, u);
    let q: Vec<[f32; 2]> = p.iter().map(|x| [dot(*x, u), dot(*x, v)]).collect();
    let cross2 = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let convex = (0..n).all(|i| cross2(q[(i + n - 1) % n], q[i], q[(i + 1) % n]) >= -1e-6);
    if convex {
        return (1..n - 1).map(|i| [0, i, i + 1]).collect();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            if cross2(q[a], q[b], q[c]) <= 1e-9 {
                continue;
            }
            let inside = idx.iter().any(|&k| {
                if k == a || k == b || k == c {
                    return false;
                }
                let (d1, d2, d3) = (cross2(q[a], q[b], q[k]), cross2(q[b], q[c], q[k]), cross2(q[c], q[a], q[k]));
                d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
            });
            if inside {
                continue;
            }
            out.push([a, b, c]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() >= 3 {
        for i in 1..idx.len() - 1 {
            out.push([idx[0], idx[i], idx[i + 1]]);
        }
    }
    out
}

/// Which faces meet at each edge, and around each vertex.
pub struct Topology {
    pub edge_faces: HashMap<Edge, Vec<u32>>,
    pub vertex_faces: Vec<Vec<u32>>,
}

impl Topology {
    pub fn of(p: &Poly) -> Topology {
        let mut edge_faces: HashMap<Edge, Vec<u32>> = HashMap::new();
        let mut vertex_faces = vec![Vec::new(); p.positions.len()];
        for f in 0..p.faces() {
            let v = p.face(f);
            for i in 0..v.len() {
                edge_faces.entry(ekey(v[i], v[(i + 1) % v.len()])).or_default().push(f as u32);
                vertex_faces[v[i] as usize].push(f as u32);
            }
        }
        Topology { edge_faces, vertex_faces }
    }

    pub fn is_boundary(&self, e: Edge) -> bool {
        self.edge_faces.get(&e).is_none_or(|f| f.len() == 1)
    }

    /// Edges with one face: the rims of holes and open sheets.
    pub fn boundary_edges(&self) -> Vec<Edge> {
        let mut v: Vec<Edge> = self.edge_faces.iter().filter(|(_, f)| f.len() == 1).map(|(e, _)| *e).collect();
        v.sort_unstable();
        v
    }

    /// Vertices joined to `v` by an edge.
    pub fn neighbours(&self, p: &Poly, v: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for &f in &self.vertex_faces[v as usize] {
            let c = p.face(f as usize);
            let n = c.len();
            if let Some(i) = c.iter().position(|&x| x == v) {
                for u in [c[(i + 1) % n], c[(i + n - 1) % n]] {
                    if !out.contains(&u) {
                        out.push(u);
                    }
                }
            }
        }
        out
    }
}

/// The face's other neighbour across edge `e`.
pub fn other_face(t: &Topology, e: Edge, f: u32) -> Option<u32> {
    t.edge_faces.get(&e)?.iter().copied().find(|&g| g != f)
}

/// Where in face `face` (a corner list) the edge a→b or b→a sits: the
/// corner index of its first end along the face.
pub fn edge_slot(face: &[u32], e: Edge) -> Option<usize> {
    let n = face.len();
    (0..n).find(|&i| ekey(face[i], face[(i + 1) % n]) == e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Poly {
        crate::prim::cube([2.0, 2.0, 2.0], 1)
    }

    #[test]
    fn a_cube_has_six_quads_and_twelve_edges() {
        let c = cube();
        assert_eq!(c.faces(), 6);
        assert_eq!(c.edges().len(), 12);
        assert_eq!(c.positions.len(), 8);
        let t = Topology::of(&c);
        assert!(t.boundary_edges().is_empty());
        // Outward: the +x face's normal points +x.
        let f = (0..6).find(|&f| c.face_centre(f)[0] > 0.9).unwrap();
        assert!(c.face_normal(f)[0] > 0.99);
    }

    #[test]
    fn hard_cube_shades_flat_and_soft_cube_smooth() {
        let mut c = cube();
        c.uvs.clear();
        let hard = c.to_mesh(30.0, NormalMode::Angle);
        assert_eq!(hard.positions.len(), 24, "every corner split at the 90° edges");
        let soft = c.to_mesh(180.0, NormalMode::Angle);
        assert_eq!(soft.positions.len(), 8);
        assert_eq!(hard.triangles(), 12);
    }

    #[test]
    fn concave_faces_triangulate_inside() {
        // An L-shape.
        let pts = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 2.0, 0.0]];
        let tris = triangulate(&pts, &[0, 1, 2, 3, 4, 5]);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris.iter().map(|t| 0.5 * length(cross(sub(pts[t[1]], pts[t[0]]), sub(pts[t[2]], pts[t[0]])))).sum();
        assert!((area - 3.0).abs() < 1e-4, "area {area}");
    }

    #[test]
    fn orient_outward_fixes_flipped_faces() {
        let mut c = cube();
        c.flip_face(2);
        c.flip_face(4);
        for f in 0..6 {
            c.flip_face(f);
        }
        c.orient_outward();
        for f in 0..6 {
            assert!(dot(c.face_normal(f), normalise(c.face_centre(f))) > 0.9);
        }
    }

    #[test]
    fn round_trip_through_triangles() {
        let c = cube();
        let m = c.to_mesh(30.0, NormalMode::Angle);
        let back = Poly::from_mesh(&m);
        assert_eq!(back.positions.len(), 8);
        assert_eq!(back.faces(), 12);
    }
}
