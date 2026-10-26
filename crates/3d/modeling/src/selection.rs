// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a person has picked of a mesh in edit mode: vertices, edges or
//! faces (the select mode says which leads), kept in step with each other
//! the way modelers expect — a face is picked when all its corners are,
//! an edge when both ends are — plus what is hidden.
//!
//! Everything here works on vertex, edge and face numbers of one
//! [`Poly`]; operations that change the mesh hand back what they made so
//! the pick can follow the new geometry.

use crate::poly::{Edge, Poly, Topology, edge_slot, ekey, other_face};
use crate::select::edge_ring;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, dot, length, normalise, sub};

/// Which kind of element clicks pick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Vertex,
    Edge,
    Face,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Vertex, Mode::Edge, Mode::Face];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Vertex => "vertex",
            Mode::Edge => "edge",
            Mode::Face => "face",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Vertex => "Vertex",
            Mode::Edge => "Edge",
            Mode::Face => "Face",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s.trim().to_lowercase().as_str() {
            "vertex" | "vertices" | "verts" | "vert" | "points" | "point" => Mode::Vertex,
            "edge" | "edges" => Mode::Edge,
            "face" | "faces" | "polygons" | "polygon" => Mode::Face,
            _ => return None,
        })
    }
}

/// One element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Elem {
    Vertex(u32),
    Edge(Edge),
    Face(u32),
}

/// How a pick combines with what was picked before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Combine {
    /// Replace.
    #[default]
    Set,
    Add,
    Subtract,
    /// Flip each element's state.
    Toggle,
}

/// The pick and what is hidden.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub mode: Mode,
    pub verts: Vec<bool>,
    pub edges: HashSet<Edge>,
    pub faces: Vec<bool>,
    /// Vertices hidden by hand (faces using them hide too).
    pub hide_v: Vec<bool>,
    /// Faces hidden by hand.
    pub hide_f: Vec<bool>,
    /// The element picked last (the "active" one tools measure from).
    pub active: Option<Elem>,
    /// Vertices in the order they were picked (merge at first / last).
    pub order: Vec<u32>,
}

/// What can be seen (not hidden), per element.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Visible {
    pub verts: Vec<bool>,
    pub faces: Vec<bool>,
}

impl Visible {
    pub fn edge(&self, e: Edge) -> bool {
        self.verts.get(e.0 as usize).copied().unwrap_or(false) && self.verts.get(e.1 as usize).copied().unwrap_or(false)
    }
}

impl Selection {
    pub fn new(p: &Poly, mode: Mode) -> Selection {
        let mut s = Selection { mode, ..Default::default() };
        s.fit(p);
        s
    }

    /// Makes every list the mesh's size (after the mesh changed), keeping
    /// what still fits.
    pub fn fit(&mut self, p: &Poly) {
        let (nv, nf) = (p.positions.len(), p.faces());
        self.verts.resize(nv, false);
        self.faces.resize(nf, false);
        self.hide_v.resize(nv, false);
        self.hide_f.resize(nf, false);
        let live: HashSet<Edge> = p.edges().into_iter().collect();
        self.edges.retain(|e| live.contains(e));
        self.order.retain(|&v| (v as usize) < nv && self.verts[v as usize]);
        let ok = match self.active {
            Some(Elem::Vertex(v)) => (v as usize) < nv,
            Some(Elem::Face(f)) => (f as usize) < nf,
            Some(Elem::Edge(e)) => live.contains(&e),
            None => true,
        };
        if !ok {
            self.active = None;
        }
    }

    /// What is not hidden.
    pub fn visible(&self, p: &Poly) -> Visible {
        let nv = p.positions.len();
        let faces: Vec<bool> = (0..p.faces()).map(|f| !self.hide_f.get(f).copied().unwrap_or(false) && p.face(f).iter().all(|&v| !self.hide_v.get(v as usize).copied().unwrap_or(false))).collect();
        // A vertex shows when it isn't hidden itself and some face of it
        // shows (or it has no faces at all).
        let mut used = vec![false; nv];
        let mut shown = vec![false; nv];
        for f in 0..p.faces() {
            for &v in p.face(f) {
                used[v as usize] = true;
                if faces[f] {
                    shown[v as usize] = true;
                }
            }
        }
        let verts = (0..nv).map(|v| !self.hide_v.get(v).copied().unwrap_or(false) && (shown[v] || !used[v])).collect();
        Visible { verts, faces }
    }

    pub fn count(&self) -> (usize, usize, usize) {
        (self.verts.iter().filter(|b| **b).count(), self.edges.len(), self.faces.iter().filter(|b| **b).count())
    }

    pub fn is_empty(&self) -> bool {
        match self.mode {
            Mode::Vertex => !self.verts.iter().any(|b| *b),
            Mode::Edge => self.edges.is_empty(),
            Mode::Face => !self.faces.iter().any(|b| *b),
        }
    }

    pub fn vert_list(&self) -> Vec<u32> {
        self.verts.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i as u32).collect()
    }

    pub fn face_list(&self) -> Vec<u32> {
        self.faces.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i as u32).collect()
    }

    pub fn edge_list(&self) -> Vec<Edge> {
        let mut v: Vec<Edge> = self.edges.iter().copied().collect();
        v.sort_unstable();
        v
    }

    /// Brings the other two kinds in line with the one the mode leads
    /// with (and drops anything hidden).
    pub fn sync(&mut self, p: &Poly) {
        self.fit(p);
        let vis = self.visible(p);
        match self.mode {
            Mode::Vertex => {
                for v in 0..self.verts.len() {
                    self.verts[v] &= vis.verts[v];
                }
                self.from_verts(p);
            }
            Mode::Edge => {
                self.edges.retain(|e| vis.edge(*e));
                self.from_edges(p, &vis);
            }
            Mode::Face => {
                for f in 0..self.faces.len() {
                    self.faces[f] &= vis.faces[f];
                }
                self.from_faces(p);
            }
        }
        let verts = &self.verts;
        self.order.retain(|&v| verts[v as usize]);
    }

    fn from_verts(&mut self, p: &Poly) {
        self.edges = p.edges().into_iter().filter(|e| self.verts[e.0 as usize] && self.verts[e.1 as usize]).collect();
        self.faces = (0..p.faces()).map(|f| p.face(f).iter().all(|&v| self.verts[v as usize])).collect();
    }

    fn from_edges(&mut self, p: &Poly, vis: &Visible) {
        self.verts = vec![false; p.positions.len()];
        for e in &self.edges {
            self.verts[e.0 as usize] = true;
            self.verts[e.1 as usize] = true;
        }
        self.faces = (0..p.faces())
            .map(|f| {
                let c = p.face(f);
                vis.faces[f] && (0..c.len()).all(|i| self.edges.contains(&ekey(c[i], c[(i + 1) % c.len()])))
            })
            .collect();
    }

    fn from_faces(&mut self, p: &Poly) {
        self.verts = vec![false; p.positions.len()];
        self.edges.clear();
        for f in 0..p.faces() {
            if self.faces[f] {
                let c = p.face(f);
                for i in 0..c.len() {
                    self.verts[c[i] as usize] = true;
                    self.edges.insert(ekey(c[i], c[(i + 1) % c.len()]));
                }
            }
        }
    }

    /// Switches select mode, carrying the pick over.
    pub fn set_mode(&mut self, p: &Poly, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.fit(p);
        let old = self.mode;
        self.mode = mode;
        match (old, mode) {
            // Fewer, bigger elements: only those wholly picked.
            (Mode::Vertex, _) | (Mode::Edge, Mode::Face) => {}
            // More, smaller elements: everything the bigger ones used.
            (Mode::Face, _) => self.from_faces(p),
            (Mode::Edge, Mode::Vertex) => {}
            _ => {}
        }
        self.sync(p);
    }

    pub fn clear(&mut self) {
        self.verts.iter_mut().for_each(|b| *b = false);
        self.faces.iter_mut().for_each(|b| *b = false);
        self.edges.clear();
        self.active = None;
        self.order.clear();
    }

    pub fn all(&mut self, p: &Poly) {
        self.fit(p);
        let vis = self.visible(p);
        self.verts = vis.verts.clone();
        self.faces = vis.faces.clone();
        self.edges = p.edges().into_iter().filter(|e| vis.edge(*e)).collect();
        self.sync(p);
    }

    pub fn invert(&mut self, p: &Poly) {
        self.fit(p);
        let vis = self.visible(p);
        match self.mode {
            Mode::Vertex => {
                for v in 0..self.verts.len() {
                    self.verts[v] = vis.verts[v] && !self.verts[v];
                }
            }
            Mode::Edge => {
                let all: HashSet<Edge> = p.edges().into_iter().filter(|e| vis.edge(*e)).collect();
                self.edges = all.difference(&self.edges).copied().collect();
            }
            Mode::Face => {
                for f in 0..self.faces.len() {
                    self.faces[f] = vis.faces[f] && !self.faces[f];
                }
            }
        }
        self.active = None;
        self.sync(p);
    }

    /// Picks one element (of the current mode's kind or any kind: an edge
    /// in vertex mode picks both its ends).
    pub fn pick(&mut self, p: &Poly, e: Elem, how: Combine) {
        self.fit(p);
        if how == Combine::Set {
            self.clear();
        }
        let on = |was: bool| match how {
            Combine::Set | Combine::Add => true,
            Combine::Subtract => false,
            Combine::Toggle => !was,
        };
        match e {
            Elem::Vertex(v) if (v as usize) < self.verts.len() => {
                let to = on(self.verts[v as usize]);
                self.set_vertex(p, v, to);
            }
            Elem::Edge(ed) => {
                let to = on(self.edges.contains(&ed));
                self.set_edge(p, ed, to);
            }
            Elem::Face(f) if (f as usize) < self.faces.len() => {
                let to = on(self.faces[f as usize]);
                self.set_face(p, f, to);
            }
            _ => return,
        }
        let picked = match e {
            Elem::Vertex(v) => self.verts.get(v as usize).copied().unwrap_or(false),
            Elem::Edge(ed) => self.edges.contains(&ed) || (self.verts[ed.0 as usize] && self.verts[ed.1 as usize]),
            Elem::Face(f) => self.faces.get(f as usize).copied().unwrap_or(false),
        };
        self.active = if picked { Some(e) } else { None };
        self.sync(p);
    }

    fn set_vertex(&mut self, p: &Poly, v: u32, on: bool) {
        match self.mode {
            Mode::Face => {
                // A vertex in face mode picks the faces round it.
                for f in 0..p.faces() {
                    if p.face(f).contains(&v) {
                        self.faces[f] = on;
                    }
                }
            }
            Mode::Edge => {
                for e in p.edges() {
                    if e.0 == v || e.1 == v {
                        if on {
                            self.edges.insert(e);
                        } else {
                            self.edges.remove(&e);
                        }
                    }
                }
            }
            Mode::Vertex => {
                self.verts[v as usize] = on;
                if on {
                    self.order.retain(|&x| x != v);
                    self.order.push(v);
                }
            }
        }
    }

    fn set_edge(&mut self, p: &Poly, e: Edge, on: bool) {
        match self.mode {
            Mode::Vertex => {
                self.set_vertex(p, e.0, on);
                self.set_vertex(p, e.1, on);
            }
            Mode::Edge => {
                if on {
                    self.edges.insert(e);
                } else {
                    self.edges.remove(&e);
                }
            }
            Mode::Face => {
                let t = Topology::of(p);
                for &f in t.edge_faces.get(&e).map(Vec::as_slice).unwrap_or(&[]) {
                    self.faces[f as usize] = on;
                }
            }
        }
    }

    fn set_face(&mut self, p: &Poly, f: u32, on: bool) {
        match self.mode {
            Mode::Face => self.faces[f as usize] = on,
            Mode::Vertex => {
                for &v in p.face(f as usize).to_vec().iter() {
                    self.set_vertex(p, v, on);
                }
            }
            Mode::Edge => {
                let c = p.face(f as usize);
                for i in 0..c.len() {
                    let e = ekey(c[i], c[(i + 1) % c.len()]);
                    if on {
                        self.edges.insert(e);
                    } else {
                        self.edges.remove(&e);
                    }
                }
            }
        }
    }

    /// Picks by a test on positions (a box or lasso on screen, a paint
    /// brush): vertices whose point passes; in edge mode edges with both
    /// ends passing; in face mode faces whose centre passes. Only what
    /// `reach` allows (not hidden, and not behind the surface when the
    /// view isn't see-through).
    pub fn region(&mut self, p: &Poly, inside: &dyn Fn(V3) -> bool, reach: Option<&[bool]>, how: Combine) {
        self.fit(p);
        if how == Combine::Set {
            self.clear();
        }
        let vis = self.visible(p);
        let ok_v = |v: usize| vis.verts[v] && reach.is_none_or(|r| r.get(v).copied().unwrap_or(false));
        let hit_v: Vec<bool> = (0..p.positions.len()).map(|v| ok_v(v) && inside(p.positions[v])).collect();
        let to = |was: bool| match how {
            Combine::Set | Combine::Add => true,
            Combine::Subtract => false,
            Combine::Toggle => !was,
        };
        match self.mode {
            Mode::Vertex => {
                for v in 0..hit_v.len() {
                    if hit_v[v] {
                        self.verts[v] = to(self.verts[v]);
                    }
                }
            }
            Mode::Edge => {
                for e in p.edges() {
                    if hit_v[e.0 as usize] && hit_v[e.1 as usize] {
                        if to(self.edges.contains(&e)) {
                            self.edges.insert(e);
                        } else {
                            self.edges.remove(&e);
                        }
                    }
                }
            }
            Mode::Face => {
                for f in 0..p.faces() {
                    let c = p.face(f);
                    let seen = c.iter().any(|&v| reach.is_none_or(|r| r.get(v as usize).copied().unwrap_or(false)));
                    if vis.faces[f] && seen && inside(p.face_centre(f)) {
                        self.faces[f] = to(self.faces[f]);
                    }
                }
            }
        }
        self.sync(p);
    }

    /// The edge loop through `e` (face mode: the loop of faces across it).
    pub fn pick_loop(&mut self, p: &Poly, e: Edge, how: Combine) {
        let t = Topology::of(p);
        if how == Combine::Set {
            self.clear();
        }
        match self.mode {
            Mode::Face => {
                // Faces of the ring through the edge: the ring of edges
                // across the faces meeting at `e` runs round them.
                for f in face_loop(p, &t, e) {
                    self.set_face(p, f, how != Combine::Subtract);
                }
            }
            _ => {
                for l in crate::select::edge_loop(p, &t, e) {
                    self.set_edge(p, l, how != Combine::Subtract);
                }
            }
        }
        self.active = Some(Elem::Edge(e));
        self.sync(p);
    }

    /// The edge ring through `e` (face mode: the faces it crosses).
    pub fn pick_ring(&mut self, p: &Poly, e: Edge, how: Combine) {
        let t = Topology::of(p);
        if how == Combine::Set {
            self.clear();
        }
        let ring = edge_ring(p, &t, e);
        match self.mode {
            Mode::Face => {
                for (_, f) in &ring {
                    if let Some(f) = f {
                        self.set_face(p, *f, how != Combine::Subtract);
                    }
                }
                for &f in t.edge_faces.get(&e).map(Vec::as_slice).unwrap_or(&[]) {
                    if p.face(f as usize).len() == 4 {
                        self.set_face(p, f, how != Combine::Subtract);
                    }
                }
            }
            _ => {
                for (r, _) in ring {
                    self.set_edge(p, r, how != Combine::Subtract);
                }
            }
        }
        self.active = Some(Elem::Edge(e));
        self.sync(p);
    }

    /// Everything joined to the pick (stopping at seams when `seams`).
    pub fn linked(&mut self, p: &Poly, seams: bool) {
        self.fit(p);
        let vis = self.visible(p);
        let t = Topology::of(p);
        let mut face_on = self.faces.clone();
        for f in 0..p.faces() {
            if p.face(f).iter().any(|&v| self.verts[v as usize]) {
                face_on[f] = true;
            }
        }
        let mut stack: Vec<usize> = (0..p.faces()).filter(|&f| face_on[f] && vis.faces[f]).collect();
        while let Some(f) = stack.pop() {
            let c = p.face(f).to_vec();
            for i in 0..c.len() {
                let e = ekey(c[i], c[(i + 1) % c.len()]);
                if seams && p.seams.contains(&e) {
                    continue;
                }
                let neighbours: Vec<u32> = if seams { t.edge_faces.get(&e).cloned().unwrap_or_default() } else { t.vertex_faces[c[i] as usize].clone() };
                for g in neighbours {
                    let g = g as usize;
                    if !face_on[g] && vis.faces[g] {
                        face_on[g] = true;
                        stack.push(g);
                    }
                }
            }
        }
        self.faces = face_on;
        let mode = self.mode;
        self.mode = Mode::Face;
        self.from_faces(p);
        self.mode = mode;
        if mode == Mode::Edge {
            // from_faces filled the edges already.
        }
        self.sync(p);
    }

    /// One ring more.
    pub fn grow(&mut self, p: &Poly) {
        self.fit(p);
        let t = Topology::of(p);
        match self.mode {
            Mode::Face => {
                let m = crate::select::grow(p, self.faces.clone(), 1);
                self.faces = m;
            }
            _ => {
                let mut next = self.verts.clone();
                for v in 0..p.positions.len() {
                    if self.verts[v] {
                        for u in t.neighbours(p, v as u32) {
                            next[u as usize] = true;
                        }
                    }
                }
                self.verts = next;
                if self.mode == Mode::Edge {
                    self.edges = p.edges().into_iter().filter(|e| self.verts[e.0 as usize] && self.verts[e.1 as usize]).collect();
                }
            }
        }
        self.sync(p);
    }

    /// One ring less (the rim of the pick goes).
    pub fn shrink(&mut self, p: &Poly) {
        self.fit(p);
        let t = Topology::of(p);
        match self.mode {
            Mode::Face => self.faces = crate::select::grow(p, self.faces.clone(), -1),
            _ => {
                let mut next = self.verts.clone();
                for v in 0..p.positions.len() {
                    if self.verts[v] && (t.neighbours(p, v as u32).iter().any(|&u| !self.verts[u as usize]) || t.vertex_faces[v].is_empty()) {
                        next[v] = false;
                    }
                }
                self.verts = next;
                if self.mode == Mode::Edge {
                    self.edges = p.edges().into_iter().filter(|e| self.verts[e.0 as usize] && self.verts[e.1 as usize]).collect();
                }
            }
        }
        self.sync(p);
    }

    /// Hides the pick (or everything else when `unpicked`).
    pub fn hide(&mut self, p: &Poly, unpicked: bool) {
        self.fit(p);
        match self.mode {
            Mode::Face => {
                for f in 0..p.faces() {
                    if self.faces[f] != unpicked {
                        self.hide_f[f] = true;
                    }
                }
            }
            _ => {
                for v in 0..p.positions.len() {
                    if self.verts[v] != unpicked {
                        self.hide_v[v] = true;
                    }
                }
            }
        }
        self.clear();
        self.sync(p);
    }

    /// Shows everything again; what was hidden comes back picked.
    pub fn reveal(&mut self, p: &Poly) {
        self.fit(p);
        let before = self.visible(p);
        self.hide_v.iter_mut().for_each(|b| *b = false);
        self.hide_f.iter_mut().for_each(|b| *b = false);
        match self.mode {
            Mode::Face => {
                for f in 0..p.faces() {
                    if !before.faces[f] {
                        self.faces[f] = true;
                    }
                }
            }
            Mode::Edge => {
                for e in p.edges() {
                    if !before.edge(e) {
                        self.edges.insert(e);
                    }
                }
            }
            Mode::Vertex => {
                for v in 0..p.positions.len() {
                    if !before.verts[v] {
                        self.verts[v] = true;
                    }
                }
            }
        }
        self.sync(p);
    }

    /// Picks the elements like the picked ones.
    pub fn similar(&mut self, p: &Poly, by: Similar, threshold: f32) -> Result<usize, String> {
        self.fit(p);
        let vis = self.visible(p);
        let t = Topology::of(p);
        let before = self.count();
        match self.mode {
            Mode::Face => {
                let picked = self.face_list();
                if picked.is_empty() {
                    return Err("pick a face first".into());
                }
                let key = |f: usize| -> Vec<f32> {
                    match by {
                        Similar::Normal => p.face_normal(f).to_vec(),
                        Similar::Area => vec![p.face_area(f)],
                        Similar::Material => vec![p.face_material[f] as f32],
                        Similar::Sides => vec![p.face(f).len() as f32],
                        Similar::Coplanar => {
                            let n = p.face_normal(f);
                            vec![n[0], n[1], n[2], dot(n, p.face_centre(f))]
                        }
                        Similar::Smooth => vec![p.face_smooth[f] as u8 as f32],
                        Similar::FaceSet => vec![p.face_set[f] as f32],
                        _ => vec![0.0],
                    }
                };
                let keys: Vec<Vec<f32>> = picked.iter().map(|&f| key(f as usize)).collect();
                for f in 0..p.faces() {
                    if !vis.faces[f] || self.faces[f] {
                        continue;
                    }
                    let k = key(f);
                    if keys.iter().any(|q| close(by, q, &k, threshold, p)) {
                        self.faces[f] = true;
                    }
                }
            }
            Mode::Edge => {
                let picked = self.edge_list();
                if picked.is_empty() {
                    return Err("pick an edge first".into());
                }
                let key = |e: Edge| -> Vec<f32> {
                    let d = sub(p.positions[e.1 as usize], p.positions[e.0 as usize]);
                    match by {
                        Similar::Length => vec![length(d)],
                        Similar::Direction => {
                            let n = normalise(d);
                            vec![n[0].abs(), n[1].abs(), n[2].abs()]
                        }
                        Similar::FaceAngle => {
                            let fs = t.edge_faces.get(&e).cloned().unwrap_or_default();
                            match fs[..] {
                                [a, b] => vec![dot(p.face_normal(a as usize), p.face_normal(b as usize)).clamp(-1.0, 1.0).acos().to_degrees()],
                                _ => vec![-1.0],
                            }
                        }
                        Similar::Seam => vec![p.seams.contains(&e) as u8 as f32],
                        Similar::Sharp => vec![p.sharp.contains(&e) as u8 as f32],
                        Similar::Crease => vec![p.creases.get(&e).copied().unwrap_or(0.0)],
                        _ => vec![t.edge_faces.get(&e).map_or(0, Vec::len) as f32],
                    }
                };
                let keys: Vec<Vec<f32>> = picked.iter().map(|e| key(*e)).collect();
                for e in p.edges() {
                    if !vis.edge(e) || self.edges.contains(&e) {
                        continue;
                    }
                    let k = key(e);
                    if keys.iter().any(|q| close(by, q, &k, threshold, p)) {
                        self.edges.insert(e);
                    }
                }
            }
            Mode::Vertex => {
                let picked = self.vert_list();
                if picked.is_empty() {
                    return Err("pick a vertex first".into());
                }
                let normals = crate::modifiers::vertex_normals(p);
                let key = |v: usize| -> Vec<f32> {
                    match by {
                        Similar::Normal => normals[v].to_vec(),
                        _ => vec![t.neighbours(p, v as u32).len() as f32],
                    }
                };
                let keys: Vec<Vec<f32>> = picked.iter().map(|&v| key(v as usize)).collect();
                for v in 0..p.positions.len() {
                    if !vis.verts[v] || self.verts[v] {
                        continue;
                    }
                    let k = key(v);
                    let by_v = if by == Similar::Normal { Similar::Normal } else { Similar::Sides };
                    if keys.iter().any(|q| close(by_v, q, &k, threshold, p)) {
                        self.verts[v] = true;
                    }
                }
            }
        }
        self.sync(p);
        let after = self.count();
        Ok(match self.mode {
            Mode::Vertex => after.0 - before.0,
            Mode::Edge => after.1 - before.1,
            Mode::Face => after.2 - before.2,
        })
    }

    /// Changes the pick by a list of vertices, edges and faces.
    pub fn with_vertices(&mut self, p: &Poly, verts: &[u32]) {
        let mode = self.mode;
        self.mode = Mode::Vertex;
        self.clear();
        for &v in verts {
            if (v as usize) < self.verts.len() {
                self.verts[v as usize] = true;
                self.order.push(v);
            }
        }
        self.sync(p);
        self.mode = mode;
        if mode != Mode::Vertex {
            self.sync(p);
        }
    }

    pub fn with_edges(&mut self, p: &Poly, edges: &[Edge]) {
        let mode = self.mode;
        self.mode = Mode::Edge;
        self.clear();
        self.fit(p);
        let live: HashSet<Edge> = p.edges().into_iter().collect();
        self.edges = edges.iter().map(|e| ekey(e.0, e.1)).filter(|e| live.contains(e)).collect();
        let vis = self.visible(p);
        self.from_edges(p, &vis);
        self.mode = mode;
        if mode == Mode::Face {
            // Faces stay only when all their edges came.
        }
        self.sync(p);
    }

    pub fn with_faces(&mut self, p: &Poly, faces: &[u32]) {
        let mode = self.mode;
        self.mode = Mode::Face;
        self.clear();
        self.fit(p);
        for &f in faces {
            if (f as usize) < self.faces.len() {
                self.faces[f as usize] = true;
            }
        }
        self.from_faces(p);
        self.mode = mode;
        self.sync(p);
    }

    /// The vertices tools move: the picked ones.
    pub fn moving(&self) -> Vec<bool> {
        self.verts.clone()
    }

    /// Faces a face tool works on: picked faces.
    pub fn face_pick(&self) -> Vec<bool> {
        self.faces.clone()
    }

    /// After vertices were renumbered (`map[old] = new`, `u32::MAX` gone).
    pub fn remap_vertices(&mut self, p: &Poly, map: &[u32]) {
        let mut verts = vec![false; p.positions.len()];
        let mut hide_v = vec![false; p.positions.len()];
        for (old, &new) in map.iter().enumerate() {
            if new != u32::MAX && (new as usize) < verts.len() {
                verts[new as usize] |= self.verts.get(old).copied().unwrap_or(false);
                hide_v[new as usize] |= self.hide_v.get(old).copied().unwrap_or(false);
            }
        }
        let re = |v: u32| map.get(v as usize).copied().filter(|&n| n != u32::MAX);
        self.edges = self.edges.iter().filter_map(|e| Some(ekey(re(e.0)?, re(e.1)?))).filter(|e| e.0 != e.1).collect();
        self.order = self.order.iter().filter_map(|&v| re(v)).collect();
        self.active = match self.active {
            Some(Elem::Vertex(v)) => re(v).map(Elem::Vertex),
            Some(Elem::Edge(e)) => re(e.0).zip(re(e.1)).map(|(a, b)| Elem::Edge(ekey(a, b))),
            other => other,
        };
        self.verts = verts;
        self.hide_v = hide_v;
    }

    /// The point tools turn and scale about: the middle of the picked
    /// vertices (their average).
    pub fn median(&self, p: &Poly) -> Option<V3> {
        let mut s = [0.0f64; 3];
        let mut n = 0usize;
        for (v, on) in self.verts.iter().enumerate() {
            if *on {
                for k in 0..3 {
                    s[k] += p.positions[v][k] as f64;
                }
                n += 1;
            }
        }
        (n > 0).then(|| [(s[0] / n as f64) as f32, (s[1] / n as f64) as f32, (s[2] / n as f64) as f32])
    }

    /// The middle of the picked vertices' box.
    pub fn bounds_centre(&self, p: &Poly) -> Option<V3> {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let mut any = false;
        for (v, on) in self.verts.iter().enumerate() {
            if *on {
                any = true;
                for k in 0..3 {
                    lo[k] = lo[k].min(p.positions[v][k]);
                    hi[k] = hi[k].max(p.positions[v][k]);
                }
            }
        }
        any.then(|| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0])
    }

    /// The active element's middle.
    pub fn active_point(&self, p: &Poly) -> Option<V3> {
        match self.active? {
            Elem::Vertex(v) => p.positions.get(v as usize).copied(),
            Elem::Edge(e) => Some(studio_mesh::lerp(*p.positions.get(e.0 as usize)?, *p.positions.get(e.1 as usize)?, 0.5)),
            Elem::Face(f) => ((f as usize) < p.faces()).then(|| p.face_centre(f as usize)),
        }
    }

    /// The pick's own axes: x along the active edge (or the face's first
    /// edge), z out of the surface (the picked faces' average normal).
    pub fn normal_axes(&self, p: &Poly) -> Option<[V3; 3]> {
        let mut n = [0.0; 3];
        for f in 0..p.faces() {
            if self.faces.get(f).copied().unwrap_or(false) {
                n = studio_mesh::add(n, p.face_area_vector(f));
            }
        }
        if length(n) < 1e-9 {
            let normals = crate::modifiers::vertex_normals(p);
            for (v, on) in self.verts.iter().enumerate() {
                if *on {
                    n = studio_mesh::add(n, normals[v]);
                }
            }
        }
        if length(n) < 1e-9 {
            return None;
        }
        let z = normalise(n);
        let hint = match self.active {
            Some(Elem::Edge(e)) => sub(p.positions[e.1 as usize], p.positions[e.0 as usize]),
            _ => {
                let vs = self.vert_list();
                if vs.len() >= 2 { sub(p.positions[vs[1] as usize], p.positions[vs[0] as usize]) } else if z[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] }
            }
        };
        let x0 = sub(hint, studio_mesh::scale(z, dot(hint, z)));
        let x = if length(x0) < 1e-6 { normalise(studio_mesh::cross(if z[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] }, z)) } else { normalise(x0) };
        let y = studio_mesh::cross(z, x);
        Some([x, y, z])
    }
}

/// What "similar" compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Similar {
    Normal,
    Area,
    Material,
    Sides,
    Coplanar,
    Smooth,
    FaceSet,
    Length,
    Direction,
    FaceAngle,
    Seam,
    Sharp,
    Crease,
    /// Vertices: number of edges.
    Edges,
}

impl Similar {
    pub const FACE: [(&'static str, Similar); 7] = [("normal", Similar::Normal), ("area", Similar::Area), ("material", Similar::Material), ("sides", Similar::Sides), ("coplanar", Similar::Coplanar), ("smooth", Similar::Smooth), ("face_set", Similar::FaceSet)];
    pub const EDGE: [(&'static str, Similar); 6] = [("length", Similar::Length), ("direction", Similar::Direction), ("face_angle", Similar::FaceAngle), ("seam", Similar::Seam), ("sharp", Similar::Sharp), ("crease", Similar::Crease)];
    pub const VERTEX: [(&'static str, Similar); 2] = [("normal", Similar::Normal), ("edges", Similar::Edges)];

    pub fn parse(s: &str) -> Option<Similar> {
        Similar::FACE.iter().chain(Similar::EDGE.iter()).chain(Similar::VERTEX.iter()).find(|(n, _)| *n == s).map(|(_, k)| *k)
    }
}

fn close(by: Similar, a: &[f32], b: &[f32], threshold: f32, _p: &Poly) -> bool {
    match by {
        Similar::Normal | Similar::Direction => {
            let d = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>().clamp(-1.0, 1.0);
            d.acos().to_degrees() <= threshold.max(0.01)
        }
        Similar::Coplanar => {
            let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
            d.acos().to_degrees() <= threshold.max(0.01) && (a[3] - b[3]).abs() <= threshold.max(0.01) * 0.1 * a[3].abs().max(1.0)
        }
        Similar::Area | Similar::Length => {
            let (x, y) = (a[0], b[0]);
            (x - y).abs() <= threshold.max(0.0) / 100.0 * x.abs().max(y.abs()).max(1e-9) + 1e-6
        }
        Similar::FaceAngle => (a[0] - b[0]).abs() <= threshold.max(0.01),
        Similar::Crease => (a[0] - b[0]).abs() <= threshold.max(0.0) / 100.0 + 1e-6,
        _ => a == b,
    }
}

/// Faces of the loop running across `e`: from each face at `e`, across
/// the quad to the opposite edge, and on.
pub fn face_loop(p: &Poly, t: &Topology, e: Edge) -> Vec<u32> {
    // The loop of faces across `e` is the ring of edges *along* `e`'s
    // direction: faces between consecutive edges of the edge ring through
    // an edge of the face that shares a corner with `e` but runs across.
    let mut out = Vec::new();
    let faces = t.edge_faces.get(&e).cloned().unwrap_or_default();
    let Some(&f0) = faces.first() else { return out };
    let c = p.face(f0 as usize);
    if c.len() != 4 {
        out.push(f0);
        return out;
    }
    let Some(s) = edge_slot(c, e) else { return out };
    // The side edge next to `e` in the first face: the ring through it
    // walks the faces along `e`'s loop.
    let side = ekey(c[(s + 1) % 4], c[(s + 2) % 4]);
    for (_, f) in edge_ring(p, t, side) {
        if let Some(f) = f
            && !out.contains(&f)
        {
            out.push(f);
        }
    }
    for &f in t.edge_faces.get(&side).map(Vec::as_slice).unwrap_or(&[]) {
        if p.face(f as usize).len() == 4 && !out.contains(&f) {
            out.push(f);
        }
    }
    let _ = other_face;
    out
}

/// Open edges and edges with more than two faces; vertices where the
/// surface pinches.
pub fn non_manifold(p: &Poly) -> (Vec<Edge>, Vec<u32>) {
    let t = Topology::of(p);
    let mut edges: Vec<Edge> = t.edge_faces.iter().filter(|(_, f)| f.len() != 2).map(|(e, _)| *e).collect();
    edges.sort_unstable();
    // A vertex is a pinch when its faces fall into more than one fan.
    let mut verts = Vec::new();
    for v in 0..p.positions.len() {
        let fs = &t.vertex_faces[v];
        if fs.len() < 2 {
            continue;
        }
        let mut seen: HashSet<u32> = HashSet::from([fs[0]]);
        let mut stack = vec![fs[0]];
        while let Some(f) = stack.pop() {
            let c = p.face(f as usize);
            let Some(i) = c.iter().position(|&x| x == v as u32) else { continue };
            for u in [c[(i + 1) % c.len()], c[(i + c.len() - 1) % c.len()]] {
                for &g in t.edge_faces.get(&ekey(v as u32, u)).map(Vec::as_slice).unwrap_or(&[]) {
                    if seen.insert(g) {
                        stack.push(g);
                    }
                }
            }
        }
        if seen.len() < fs.len() {
            verts.push(v as u32);
        }
    }
    (edges, verts)
}

/// For each vertex, its mirror image across `axis` = 0 (within
/// `tolerance`), for symmetric editing.
pub fn mirror_map(p: &Poly, axis: usize, tolerance: f32) -> Vec<Option<u32>> {
    let cell = tolerance.max(1e-4) * 2.0;
    let key = |q: V3| [(q[0] / cell).round() as i64, (q[1] / cell).round() as i64, (q[2] / cell).round() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    for (v, q) in p.positions.iter().enumerate() {
        grid.entry(key(*q)).or_default().push(v as u32);
    }
    p.positions
        .iter()
        .map(|q| {
            let mut m = *q;
            m[axis] = -m[axis];
            let k = key(m);
            let mut best: Option<(f32, u32)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        for &u in grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]).map(Vec::as_slice).unwrap_or(&[]) {
                            let d = length(sub(p.positions[u as usize], m));
                            if d <= tolerance && best.is_none_or(|b| d < b.0) {
                                best = Some((d, u));
                            }
                        }
                    }
                }
            }
            best.map(|b| b.1)
        })
        .collect()
}

/// How strongly each vertex follows a move of the picked ones when
/// proportional editing reaches `radius` (1 for picked ones). With
/// `connected`, distance is measured along edges, so a nearby but
/// separate piece stays put.
pub fn falloff_weights(p: &Poly, picked: &[bool], radius: f32, falloff: crate::edit::Falloff, connected: bool) -> Vec<f32> {
    let n = p.positions.len();
    let mut w = vec![0.0f32; n];
    if radius <= 0.0 {
        for v in 0..n {
            if picked.get(v).copied().unwrap_or(false) {
                w[v] = 1.0;
            }
        }
        return w;
    }
    let mut dist = vec![f32::INFINITY; n];
    if connected {
        let t = Topology::of(p);
        let nb: Vec<Vec<u32>> = (0..n as u32).map(|v| t.neighbours(p, v)).collect();
        // Dijkstra from every picked vertex at once.
        let mut heap = std::collections::BinaryHeap::new();
        #[derive(PartialEq)]
        struct Item(f32, u32);
        impl Eq for Item {}
        impl PartialOrd for Item {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Item {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        for v in 0..n {
            if picked.get(v).copied().unwrap_or(false) {
                dist[v] = 0.0;
                heap.push(Item(0.0, v as u32));
            }
        }
        while let Some(Item(d, v)) = heap.pop() {
            if d > dist[v as usize] || d > radius {
                continue;
            }
            for &u in &nb[v as usize] {
                let nd = d + length(sub(p.positions[u as usize], p.positions[v as usize]));
                if nd < dist[u as usize] {
                    dist[u as usize] = nd;
                    heap.push(Item(nd, u));
                }
            }
        }
    } else {
        let src: Vec<V3> = (0..n).filter(|&v| picked.get(v).copied().unwrap_or(false)).map(|v| p.positions[v]).collect();
        let grid = crate::spatial::PointGrid::new(&src, radius.max(1e-4));
        for v in 0..n {
            if picked.get(v).copied().unwrap_or(false) {
                dist[v] = 0.0;
                continue;
            }
            let q = p.positions[v];
            for i in grid.within(&src, q, radius) {
                dist[v] = dist[v].min(length(sub(src[i as usize], q)));
            }
        }
    }
    for v in 0..n {
        w[v] = if dist[v] == 0.0 { 1.0 } else if dist[v] < radius { falloff.weight(dist[v] / radius) } else { 0.0 };
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn modes_carry_the_pick_over() {
        let c = prim::cube([2.0; 3], 1);
        let mut s = Selection::new(&c, Mode::Face);
        s.pick(&c, Elem::Face(0), Combine::Set);
        assert_eq!(s.count(), (4, 4, 1));
        s.set_mode(&c, Mode::Vertex);
        assert_eq!(s.count(), (4, 4, 1));
        // One vertex less: the face goes, two edges stay.
        let v = s.vert_list()[0];
        s.pick(&c, Elem::Vertex(v), Combine::Subtract);
        assert_eq!(s.count(), (3, 2, 0));
        s.set_mode(&c, Mode::Face);
        assert_eq!(s.count().2, 0);
    }

    #[test]
    fn all_invert_hide_and_reveal() {
        let c = prim::cube([2.0; 3], 2);
        let mut s = Selection::new(&c, Mode::Face);
        s.all(&c);
        assert_eq!(s.count().2, 24);
        s.with_faces(&c, &[0, 1]);
        s.invert(&c);
        assert_eq!(s.count().2, 22);
        s.hide(&c, false);
        let vis = s.visible(&c);
        assert_eq!(vis.faces.iter().filter(|b| **b).count(), 2);
        s.all(&c);
        assert_eq!(s.count().2, 2, "hidden faces can't be picked");
        s.clear();
        s.reveal(&c);
        assert_eq!(s.count().2, 22, "what comes back is picked");
        assert!(s.visible(&c).faces.iter().all(|b| *b));
    }

    #[test]
    fn loops_rings_linked_grow_shrink() {
        let cyl = prim::cylinder(10.0, 10.0, 20.0, 12, 4, prim::Cap::Ngon);
        let t = Topology::of(&cyl);
        let mid = crate::select::nearest_edge(&cyl, [10.0, 0.0, 0.0]).unwrap();
        let mut s = Selection::new(&cyl, Mode::Edge);
        s.pick_loop(&cyl, mid, Combine::Set);
        let n_loop = s.count().1;
        s.pick_ring(&cyl, mid, Combine::Set);
        let n_ring = s.count().1;
        assert!(n_loop == 12 || n_ring == 12, "loop {n_loop} ring {n_ring}");
        let _ = t;
        // Face loop round the side.
        let mut f = Selection::new(&cyl, Mode::Face);
        let horizontal = {
            let mut e = mid;
            if (cyl.positions[e.0 as usize][1] - cyl.positions[e.1 as usize][1]).abs() > 1e-3 {
                // A vertical edge: its face loop goes round.
                e = mid;
            }
            e
        };
        f.pick_loop(&cyl, horizontal, Combine::Set);
        assert!(f.count().2 >= 4);
        // Linked from one face is everything.
        f.with_faces(&cyl, &[0]);
        f.linked(&cyl, false);
        assert_eq!(f.count().2, cyl.faces());
        let mut g = Selection::new(&cyl, Mode::Face);
        g.with_faces(&cyl, &[5]);
        g.grow(&cyl);
        let grown = g.count().2;
        assert!(grown > 1);
        g.shrink(&cyl);
        assert!(g.count().2 < grown);
    }

    #[test]
    fn similar_faces_by_normal_and_sides() {
        let c = prim::cube([2.0; 3], 2);
        let mut s = Selection::new(&c, Mode::Face);
        let top = crate::select::Faces::named("top").unwrap().list(&c);
        s.with_faces(&c, &top[..1]);
        assert_eq!(s.similar(&c, Similar::Normal, 1.0).unwrap(), 3);
        s.with_faces(&c, &[0]);
        s.similar(&c, Similar::Sides, 0.0).unwrap();
        assert_eq!(s.count().2, 24);
    }

    #[test]
    fn region_picks_respect_reach_and_mode() {
        let c = prim::cube([2.0; 3], 1);
        let mut s = Selection::new(&c, Mode::Vertex);
        s.region(&c, &|q| q[1] < 0.0, None, Combine::Set);
        assert_eq!(s.count(), (4, 4, 1));
        let reach: Vec<bool> = c.positions.iter().map(|q| q[2] < 0.0).collect();
        s.region(&c, &|q| q[1] < 0.0, Some(&reach), Combine::Set);
        assert_eq!(s.count().0, 2);
        s.mode = Mode::Face;
        s.region(&c, &|q| q[0] > 0.5, None, Combine::Set);
        assert_eq!(s.count().2, 1);
    }

    #[test]
    fn mirror_map_and_weights() {
        let c = prim::cube([2.0; 3], 2);
        let m = mirror_map(&c, 0, 1e-3);
        assert!(m.iter().all(|x| x.is_some()));
        for (v, w) in m.iter().enumerate() {
            let w = w.unwrap() as usize;
            assert!((c.positions[v][0] + c.positions[w][0]).abs() < 1e-4);
        }
        let mut picked = vec![false; c.positions.len()];
        let top = c.positions.iter().position(|q| q[1] < -0.9 && q[0].abs() < 0.1 && q[2].abs() < 0.1).unwrap();
        picked[top] = true;
        let w = falloff_weights(&c, &picked, 1.5, crate::edit::Falloff::Linear, false);
        assert_eq!(w[top], 1.0);
        assert!(w.iter().filter(|x| **x > 0.0 && **x < 1.0).count() >= 4);
        let wc = falloff_weights(&c, &picked, 1.5, crate::edit::Falloff::Linear, true);
        assert!(wc.iter().filter(|x| **x > 0.0).count() >= 5);
    }

    #[test]
    fn non_manifold_finds_open_rims() {
        let mut c = prim::cube([2.0; 3], 1);
        crate::edit::delete_faces(&mut c, &[true, false, false, false, false, false]);
        let (e, _) = non_manifold(&c);
        assert_eq!(e.len(), 4);
    }
}
