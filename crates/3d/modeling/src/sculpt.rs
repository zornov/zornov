// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Digital sculpting on dense meshes (millions of triangles): brushes
//! that push, build up, carve, flatten, pinch, smooth, grab and pull out
//! forms; alphas that stamp real surface detail (pores, scales, wrinkles,
//! cracks, weave); masks and face sets that protect and group parts;
//! mirror symmetry; and dynamic topology that adds triangles where a
//! stroke needs them and removes them where it doesn't.
//!
//! Strokes are lists of points on (or near) the surface in the model's
//! pixels; dabs are laid along them every `spacing` × radius.

use crate::edit::Falloff;
use crate::noise::Pattern;
use crate::poly::Poly;
use crate::spatial::PointGrid;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// A triangle mesh set up for sculpting.
#[derive(Clone, Debug)]
pub struct SculptMesh {
    pub pos: Vec<V3>,
    pub tris: Vec<[u32; 3]>,
    pub alive: Vec<bool>,
    pub vtris: Vec<Vec<u32>>,
    pub normals: Vec<V3>,
    /// 0 free – 1 fully protected.
    pub mask: Vec<f32>,
    pub tri_set: Vec<u32>,
    pub tri_material: Vec<u32>,
    /// Face sets that brushes can't touch.
    pub hidden: HashSet<u32>,
    grid: PointGrid,
    /// How far each vertex has risen during the current layer stroke.
    layer: Vec<f32>,
    pub materials: Vec<studio_mesh::Material>,
}

impl SculptMesh {
    pub fn from_poly(p: &Poly) -> SculptMesh {
        let mut tris = Vec::new();
        let mut sets = Vec::new();
        let mut mats = Vec::new();
        for f in 0..p.faces() {
            let c = p.face(f);
            for t in crate::poly::triangulate(&p.positions, c) {
                tris.push([c[t[0]], c[t[1]], c[t[2]]]);
                sets.push(p.face_set[f]);
                mats.push(p.face_material[f]);
            }
        }
        let mut s = SculptMesh::new(p.positions.clone(), tris);
        s.tri_set = sets;
        s.tri_material = mats;
        s.materials = p.materials.clone();
        s
    }

    pub fn new(pos: Vec<V3>, tris: Vec<[u32; 3]>) -> SculptMesh {
        let n = pos.len();
        let nt = tris.len();
        let mut vtris = vec![Vec::new(); n];
        for (t, tri) in tris.iter().enumerate() {
            for &v in tri {
                vtris[v as usize].push(t as u32);
            }
        }
        let cell = average_edge(&pos, &tris) * 4.0;
        let grid = PointGrid::new(&pos, cell.max(1e-3));
        let mut s = SculptMesh { pos, tris, alive: vec![true; nt], vtris, normals: vec![[0.0; 3]; n], mask: vec![0.0; n], tri_set: vec![0; nt], tri_material: vec![0; nt], hidden: HashSet::new(), grid, layer: vec![0.0; n], materials: vec![studio_mesh::Material::default()] };
        let all: Vec<u32> = (0..n as u32).collect();
        s.update_normals(&all);
        s
    }

    pub fn live_triangles(&self) -> usize {
        self.alive.iter().filter(|a| **a).count()
    }

    /// Back to a polygon mesh (dead triangles and loose vertices dropped).
    pub fn to_poly(&self) -> Poly {
        let mut p = Poly { positions: self.pos.clone(), materials: self.materials.clone(), ..Poly::new() };
        for (t, tri) in self.tris.iter().enumerate() {
            if self.alive[t] {
                p.add_face_like(tri, None, self.tri_material[t], self.tri_set[t], true);
            }
        }
        p.uvs.clear();
        p.compact();
        p.ensure_materials();
        p
    }

    fn tri_normal(&self, t: usize) -> V3 {
        let [a, b, c] = self.tris[t];
        cross(sub(self.pos[b as usize], self.pos[a as usize]), sub(self.pos[c as usize], self.pos[a as usize]))
    }

    /// Recomputes normals of these vertices.
    pub fn update_normals(&mut self, verts: &[u32]) {
        for &v in verts {
            let mut n = [0.0; 3];
            for &t in &self.vtris[v as usize] {
                if self.alive[t as usize] {
                    n = add(n, self.tri_normal(t as usize));
                }
            }
            self.normals[v as usize] = normalise(n);
        }
    }

    /// Vertices joined to `v`.
    pub fn neighbours(&self, v: u32) -> Vec<u32> {
        let mut out = Vec::with_capacity(8);
        for &t in &self.vtris[v as usize] {
            if !self.alive[t as usize] {
                continue;
            }
            for &u in &self.tris[t as usize] {
                if u != v && !out.contains(&u) {
                    out.push(u);
                }
            }
        }
        out
    }

    fn move_vertex(&mut self, v: u32, to: V3) {
        self.grid.remove(v, self.pos[v as usize]);
        self.pos[v as usize] = to;
        self.grid.insert(v, to);
    }

    /// Whether a brush may move `v` (not under a hidden face set).
    fn editable(&self, v: u32) -> bool {
        self.hidden.is_empty() || self.vtris[v as usize].iter().any(|&t| self.alive[t as usize] && !self.hidden.contains(&self.tri_set[t as usize]))
    }

    /// Vertices within `r` of `p`.
    pub fn around(&self, p: V3, r: f32) -> Vec<u32> {
        self.grid.within(&self.pos, p, r).into_iter().filter(|&v| !self.vtris[v as usize].is_empty()).collect()
    }

    /// The vertex nearest `p` (searching outward).
    pub fn nearest_vertex(&self, p: V3, start: f32) -> Option<u32> {
        let mut r = start.max(1e-3);
        for _ in 0..24 {
            let near = self.around(p, r);
            if let Some(v) = near.into_iter().min_by(|a, b| length(sub(self.pos[*a as usize], p)).total_cmp(&length(sub(self.pos[*b as usize], p)))) {
                return Some(v);
            }
            r *= 2.0;
        }
        None
    }
}

fn average_edge(pos: &[V3], tris: &[[u32; 3]]) -> f32 {
    let take = tris.len().min(2000);
    if take == 0 {
        return 1.0;
    }
    let step = (tris.len() / take).max(1);
    let mut s = 0.0;
    let mut n = 0;
    for t in tris.iter().step_by(step) {
        s += length(sub(pos[t[1] as usize], pos[t[0] as usize]));
        n += 1;
    }
    s / n as f32
}

/// What a brush does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Push out (or in) along the surface.
    Draw,
    /// Sharper-edged draw.
    DrawSharp,
    /// Build up flat layers of clay, filling hollows first.
    Clay,
    /// Clay laid in square strips (blocking muscles, planes of a face).
    ClayStrips,
    /// Raise to a fixed height and no more (plates, scars, panels).
    Layer,
    /// Swell along each point's own normal.
    Inflate,
    /// Round bump that pushes out from its middle.
    Blob,
    /// Cut a sharp crease (wrinkles, seams, eyelids).
    Crease,
    /// Draw points together (sharpen ridges).
    Pinch,
    /// Flatten toward the average plane.
    Flatten,
    /// Shave peaks only.
    Scrape,
    /// Fill hollows only.
    Fill,
    /// Even out bumps.
    Smooth,
    /// Move a region (big shape changes).
    Grab,
    /// Pull out along the stroke (horns, tentacles, ears).
    SnakeHook,
    /// Push sideways along the stroke.
    Nudge,
    /// Paint protection (and `unmask` to clear it).
    Mask,
    Unmask,
    /// Paint the triangles into a face set.
    FaceSet,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "draw" | "standard" => Kind::Draw,
            "draw_sharp" | "sharp" => Kind::DrawSharp,
            "clay" | "build" | "add" => Kind::Clay,
            "clay_strips" | "strips" => Kind::ClayStrips,
            "layer" => Kind::Layer,
            "inflate" | "swell" => Kind::Inflate,
            "blob" => Kind::Blob,
            "crease" | "carve" | "dam_standard" => Kind::Crease,
            "pinch" | "magnify" => Kind::Pinch,
            "flatten" | "planar" => Kind::Flatten,
            "scrape" => Kind::Scrape,
            "fill" => Kind::Fill,
            "smooth" | "relax" => Kind::Smooth,
            "grab" | "move" => Kind::Grab,
            "snake_hook" | "pull" => Kind::SnakeHook,
            "nudge" => Kind::Nudge,
            "mask" => Kind::Mask,
            "unmask" => Kind::Unmask,
            "face_set" => Kind::FaceSet,
            _ => return None,
        })
    }

    pub const NAMES: &'static [&'static str] = &["draw", "draw_sharp", "clay", "clay_strips", "layer", "inflate", "blob", "crease", "pinch", "flatten", "scrape", "fill", "smooth", "grab", "snake_hook", "nudge", "mask", "unmask", "face_set"];
}

/// A stamp of surface detail over the brush.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alpha {
    pub pattern: Pattern,
    /// Feature size (px).
    pub size: f32,
    /// 0 none – 1 all detail (the rest is the plain brush).
    #[serde(default = "one")]
    pub depth: f32,
    #[serde(default)]
    pub seed: u32,
}

fn one() -> f32 {
    1.0
}

/// A brush's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    pub kind: Kind,
    pub radius: f32,
    /// 0–1 (above 1 for heavy-handed strokes).
    #[serde(default = "half")]
    pub strength: f32,
    #[serde(default = "smooth_falloff")]
    pub falloff: Falloff,
    /// Carve instead of build.
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub alpha: Option<Alpha>,
    /// Only points facing the brush (not the far side of a thin part).
    #[serde(default = "yes")]
    pub front_only: bool,
    /// Smoothing after each dab, 0–1.
    #[serde(default)]
    pub autosmooth: f32,
    /// Face set number (face set brush).
    #[serde(default)]
    pub face_set: u32,
}

fn half() -> f32 {
    0.5
}
fn yes() -> bool {
    true
}
fn smooth_falloff() -> Falloff {
    Falloff::Smooth
}

impl serde::Serialize for Falloff {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Falloff::Smooth => "smooth",
            Falloff::Linear => "linear",
            Falloff::Sharp => "sharp",
            Falloff::Root => "root",
            Falloff::Sphere => "sphere",
            Falloff::Constant => "constant",
        })
    }
}

impl<'de> serde::Deserialize<'de> for Falloff {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Falloff, D::Error> {
        let s = String::deserialize(d)?;
        Falloff::parse(&s).ok_or_else(|| serde::de::Error::custom("falloff is smooth, linear, sharp, root, sphere or constant"))
    }
}

/// Adaptive detail: edges kept near `detail` px long under the brush.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dyntopo {
    pub detail: f32,
}

/// A stroke: points along the surface, laid as dabs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub points: Vec<V3>,
    /// Dab spacing as a fraction of the radius.
    #[serde(default = "spacing")]
    pub spacing: f32,
    /// Mirror across x, y, z (the model's middle).
    #[serde(default)]
    pub symmetry: [bool; 3],
    #[serde(default)]
    pub dyntopo: Option<Dyntopo>,
}

fn spacing() -> f32 {
    0.15
}

/// Dab centres along a polyline, `step` apart.
fn dabs(points: &[V3], step: f32) -> Vec<V3> {
    if points.len() <= 1 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    let mut carry = 0.0;
    for w in points.windows(2) {
        let d = length(sub(w[1], w[0]));
        let mut t = step - carry;
        while t <= d {
            out.push(lerp(w[0], w[1], t / d.max(1e-9)));
            t += step;
        }
        carry = d - (t - step);
    }
    if out.last() != points.last() {
        out.push(*points.last().unwrap());
    }
    out
}

/// What a stroke did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub dabs: usize,
    pub moved: usize,
    pub largest_move: f32,
    pub triangles_before: usize,
    pub triangles_after: usize,
}

impl SculptMesh {
    /// Runs a stroke.
    pub fn stroke(&mut self, brush: &Brush, stroke: &Stroke) -> Report {
        let mut rep = Report { triangles_before: self.live_triangles(), ..Default::default() };
        let r = brush.radius.max(1e-3);
        let pts = dabs(&stroke.points, (stroke.spacing.clamp(0.02, 2.0) * r).max(1e-3));
        rep.dabs = pts.len();
        self.layer.iter_mut().for_each(|x| *x = 0.0);
        if self.layer.len() != self.pos.len() {
            self.layer = vec![0.0; self.pos.len()];
        }
        let mirrors = mirror_set(stroke.symmetry);
        let mut moved: HashSet<u32> = HashSet::new();
        if brush.kind == Kind::Grab {
            // One move from the first point to the last, weights from the
            // start.
            if let (Some(a), Some(b)) = (stroke.points.first(), stroke.points.last()) {
                for m in &mirrors {
                    let (a, b) = (flip(*a, m), flip(*b, m));
                    self.grab(brush, a, sub(b, a), &mut moved, &mut rep);
                }
            }
        } else {
            let mut prev: Option<V3> = None;
            for c in &pts {
                for m in &mirrors {
                    let cm = flip(*c, m);
                    let pm = prev.map(|p| flip(p, m));
                    self.dab(brush, cm, pm, &mut moved, &mut rep);
                    if let Some(d) = stroke.dyntopo {
                        self.dyntopo(cm, r, d.detail);
                    }
                }
                prev = Some(*c);
            }
        }
        rep.moved = moved.len();
        rep.triangles_after = self.live_triangles();
        rep
    }

    fn grab(&mut self, b: &Brush, at: V3, delta: V3, moved: &mut HashSet<u32>, rep: &mut Report) {
        let r = b.radius;
        let Some(nv) = self.nearest_vertex(at, r) else { return };
        let centre = self.pos[nv as usize];
        let list = self.around(centre, r);
        let mut touched = Vec::new();
        for v in list {
            if !self.editable(v) {
                continue;
            }
            let d = length(sub(self.pos[v as usize], centre)) / r;
            let w = b.falloff.weight(d) * (1.0 - self.mask[v as usize]) * b.strength.clamp(0.0, 1.0);
            if w <= 0.0 {
                continue;
            }
            let to = add(self.pos[v as usize], scale(delta, w));
            rep.largest_move = rep.largest_move.max(length(scale(delta, w)));
            self.move_vertex(v, to);
            touched.push(v);
            moved.insert(v);
        }
        self.refresh(&touched);
    }

    fn refresh(&mut self, touched: &[u32]) {
        let mut ring: HashSet<u32> = touched.iter().copied().collect();
        for &v in touched {
            ring.extend(self.neighbours(v));
        }
        let list: Vec<u32> = ring.into_iter().collect();
        self.update_normals(&list);
    }

    fn dab(&mut self, b: &Brush, c: V3, prev: Option<V3>, moved: &mut HashSet<u32>, rep: &mut Report) {
        let r = b.radius;
        let list = self.around(c, r);
        if list.is_empty() {
            return;
        }
        // Area normal and centre.
        let mut an = [0.0; 3];
        let mut ac = [0.0; 3];
        let mut wsum = 0.0;
        for &v in &list {
            let w = b.falloff.weight(length(sub(self.pos[v as usize], c)) / r).max(1e-4);
            an = add(an, scale(self.normals[v as usize], w));
            ac = add(ac, scale(self.pos[v as usize], w));
            wsum += w;
        }
        let an = normalise(an);
        let ac = scale(ac, 1.0 / wsum.max(1e-9));
        let sign = if b.invert { -1.0 } else { 1.0 };
        let h = b.strength * r * 0.06 * sign;
        let stroke_dir = prev.map(|p| sub(c, p)).unwrap_or([0.0; 3]);
        // Square footprint for strips, along the stroke.
        let (su, sv) = {
            let t = if length(stroke_dir) > 1e-6 { normalise(sub(stroke_dir, scale(an, dot(stroke_dir, an)))) } else { normalise(cross(an, if an[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] })) };
            (t, cross(an, t))
        };
        let mut new_pos: Vec<(u32, V3)> = Vec::with_capacity(list.len());
        for &v in &list {
            if !self.editable(v) {
                continue;
            }
            let p = self.pos[v as usize];
            let n = self.normals[v as usize];
            if b.front_only && dot(n, an) < -0.1 && !matches!(b.kind, Kind::Smooth | Kind::Mask | Kind::Unmask | Kind::FaceSet) {
                continue;
            }
            let dist = length(sub(p, c)) / r;
            let mut fall = if b.kind == Kind::ClayStrips {
                let off = sub(p, c);
                let (x, y) = (dot(off, su).abs() / r, dot(off, sv).abs() / r);
                if x > 1.0 || y > 1.0 { 0.0 } else { b.falloff.weight(x.max(y) * 0.9).max(0.0) }
            } else if b.kind == Kind::DrawSharp {
                Falloff::Sharp.weight(dist)
            } else {
                b.falloff.weight(dist)
            };
            if dist > 1.0 && b.kind != Kind::ClayStrips {
                fall = 0.0;
            }
            if fall <= 0.0 {
                continue;
            }
            let mut w = fall * (1.0 - self.mask[v as usize]);
            if let Some(a) = &b.alpha {
                let s = a.pattern.height(p, a.size, a.seed);
                w *= 1.0 - a.depth.clamp(0.0, 1.0) + a.depth.clamp(0.0, 1.0) * (0.5 + 0.5 * s).clamp(0.0, 1.5);
            }
            let q = match b.kind {
                Kind::Draw | Kind::DrawSharp => add(p, scale(an, h * w)),
                Kind::Inflate => add(p, scale(n, h * w)),
                Kind::Blob => {
                    let t = sub(p, c);
                    let tang = sub(t, scale(an, dot(t, an)));
                    add(add(p, scale(an, h * w)), scale(tang, 0.15 * w * b.strength * sign))
                }
                Kind::Clay | Kind::ClayStrips => {
                    // Toward a plane just above the area's centre.
                    let plane = add(ac, scale(an, h * 2.0));
                    let d = dot(sub(p, plane), an) * sign;
                    if d < 0.0 { sub(p, scale(an, d * sign * w * 0.5)) } else { p }
                }
                Kind::Layer => {
                    let target = b.strength * r * 0.2 * fall;
                    let have = self.layer[v as usize];
                    let step = (target - have).max(0.0).min(r * 0.03);
                    self.layer[v as usize] += step;
                    add(p, scale(n, step * sign * (1.0 - self.mask[v as usize])))
                }
                Kind::Crease => {
                    let t = sub(c, p);
                    let tang = sub(t, scale(an, dot(t, an)));
                    add(add(p, scale(an, -h * w * 0.7)), scale(tang, 0.35 * w * b.strength))
                }
                Kind::Pinch => {
                    let t = sub(c, p);
                    let tang = sub(t, scale(an, dot(t, an)));
                    add(p, scale(tang, 0.3 * w * b.strength * sign))
                }
                Kind::Flatten | Kind::Scrape | Kind::Fill => {
                    let d = dot(sub(p, ac), an);
                    let apply = match b.kind {
                        Kind::Scrape => d > 0.0,
                        Kind::Fill => d < 0.0,
                        _ => true,
                    };
                    if apply { sub(p, scale(an, d * w * b.strength.min(1.0))) } else { p }
                }
                Kind::Smooth => {
                    let nb = self.neighbours(v);
                    if nb.is_empty() {
                        p
                    } else {
                        let mut avg = [0.0; 3];
                        for &u in &nb {
                            avg = add(avg, self.pos[u as usize]);
                        }
                        lerp(p, scale(avg, 1.0 / nb.len() as f32), (w * b.strength).min(1.0))
                    }
                }
                Kind::SnakeHook => add(p, scale(stroke_dir, w * b.strength.min(1.0))),
                Kind::Nudge => add(p, scale(sub(stroke_dir, scale(an, dot(stroke_dir, an))), w * b.strength.min(1.0) * 0.5)),
                Kind::Mask => {
                    self.mask[v as usize] = (self.mask[v as usize] + fall * b.strength).min(1.0);
                    p
                }
                Kind::Unmask => {
                    self.mask[v as usize] = (self.mask[v as usize] - fall * b.strength).max(0.0);
                    p
                }
                Kind::FaceSet => {
                    if fall > 0.3 {
                        for &t in &self.vtris[v as usize].clone() {
                            self.tri_set[t as usize] = b.face_set;
                        }
                    }
                    p
                }
                Kind::Grab => p,
            };
            if q != p {
                new_pos.push((v, q));
            }
        }
        let touched: Vec<u32> = new_pos.iter().map(|(v, _)| *v).collect();
        for (v, q) in new_pos {
            rep.largest_move = rep.largest_move.max(length(sub(q, self.pos[v as usize])));
            self.move_vertex(v, q);
            moved.insert(v);
        }
        if b.autosmooth > 0.0 && !touched.is_empty() {
            let cur: Vec<(u32, V3)> = touched
                .iter()
                .map(|&v| {
                    let nb = self.neighbours(v);
                    let mut avg = [0.0; 3];
                    for &u in &nb {
                        avg = add(avg, self.pos[u as usize]);
                    }
                    let a = if nb.is_empty() { self.pos[v as usize] } else { scale(avg, 1.0 / nb.len() as f32) };
                    (v, lerp(self.pos[v as usize], a, b.autosmooth.min(1.0) * (1.0 - self.mask[v as usize])))
                })
                .collect();
            for (v, q) in cur {
                self.move_vertex(v, q);
            }
        }
        self.refresh(&touched);
    }

    // ----- dynamic topology -----

    fn edge_tris(&self, a: u32, b: u32) -> Vec<u32> {
        self.vtris[a as usize].iter().copied().filter(|&t| self.alive[t as usize] && self.tris[t as usize].contains(&b)).collect()
    }

    fn add_tri(&mut self, tri: [u32; 3], set: u32, mat: u32) -> u32 {
        let id = self.tris.len() as u32;
        self.tris.push(tri);
        self.alive.push(true);
        self.tri_set.push(set);
        self.tri_material.push(mat);
        for &v in &tri {
            self.vtris[v as usize].push(id);
        }
        id
    }

    fn kill_tri(&mut self, t: u32) {
        self.alive[t as usize] = false;
        for v in self.tris[t as usize] {
            self.vtris[v as usize].retain(|&x| x != t);
        }
    }

    fn add_vert(&mut self, p: V3, n: V3, mask: f32) -> u32 {
        let id = self.pos.len() as u32;
        self.pos.push(p);
        self.normals.push(n);
        self.mask.push(mask);
        self.layer.push(0.0);
        self.vtris.push(Vec::new());
        self.grid.insert(id, p);
        id
    }

    /// Splits edge a–b at its middle.
    fn split(&mut self, a: u32, b: u32) -> u32 {
        let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
        let m = self.add_vert(lerp(pa, pb, 0.5), normalise(add(self.normals[a as usize], self.normals[b as usize])), (self.mask[a as usize] + self.mask[b as usize]) / 2.0);
        for t in self.edge_tris(a, b) {
            let tri = self.tris[t as usize];
            let (set, mat) = (self.tri_set[t as usize], self.tri_material[t as usize]);
            // Keep the winding: find a->b order in the triangle.
            let i = tri.iter().position(|&x| x == a).unwrap();
            let j = tri.iter().position(|&x| x == b).unwrap();
            let c = tri[3 - i - j];
            let forward = (i + 1) % 3 == j;
            self.kill_tri(t);
            if forward {
                self.add_tri([a, m, c], set, mat);
                self.add_tri([m, b, c], set, mat);
            } else {
                self.add_tri([b, m, c], set, mat);
                self.add_tri([m, a, c], set, mat);
            }
        }
        m
    }

    /// Collapses b into a (at their middle) when that keeps the surface a
    /// clean surface; returns whether it did.
    fn collapse(&mut self, a: u32, b: u32) -> bool {
        let shared = self.edge_tris(a, b);
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }
        // Link condition: the only common neighbours are the opposite corners.
        let na: HashSet<u32> = self.neighbours(a).into_iter().collect();
        let nb: HashSet<u32> = self.neighbours(b).into_iter().collect();
        let common = na.intersection(&nb).count();
        if common != shared.len() {
            return false;
        }
        let mid = lerp(self.pos[a as usize], self.pos[b as usize], 0.5);
        // No triangle may flip.
        for &t in self.vtris[b as usize].iter().chain(self.vtris[a as usize].iter()) {
            if shared.contains(&t) || !self.alive[t as usize] {
                continue;
            }
            let tri = self.tris[t as usize];
            let before = self.tri_normal(t as usize);
            let moved: [V3; 3] = tri.map(|v| if v == a || v == b { mid } else { self.pos[v as usize] });
            let after = cross(sub(moved[1], moved[0]), sub(moved[2], moved[0]));
            if dot(before, after) <= 0.0 {
                return false;
            }
        }
        for t in shared {
            self.kill_tri(t);
        }
        let bt: Vec<u32> = self.vtris[b as usize].clone();
        for t in bt {
            let tri = &mut self.tris[t as usize];
            for v in tri.iter_mut() {
                if *v == b {
                    *v = a;
                }
            }
            self.vtris[a as usize].push(t);
        }
        self.vtris[b as usize].clear();
        self.grid.remove(b, self.pos[b as usize]);
        self.move_vertex(a, mid);
        self.mask[a as usize] = self.mask[a as usize].max(self.mask[b as usize]);
        true
    }

    /// Keeps edges under the brush near `detail` px: splits long ones,
    /// collapses short ones, then relaxes the new points along the surface.
    pub fn dyntopo(&mut self, centre: V3, radius: f32, detail: f32) {
        let detail = detail.max(radius / 200.0).max(1e-3);
        let (long, short) = (detail * 1.25, detail * 0.4);
        let reach = radius * 1.1;
        for _pass in 0..4 {
            let verts = self.around(centre, reach);
            let mut edges: Vec<(f32, u32, u32)> = Vec::new();
            let mut seen = HashSet::new();
            for &v in &verts {
                for u in self.neighbours(v) {
                    let k = if v < u { (v, u) } else { (u, v) };
                    if seen.insert(k) {
                        let l = length(sub(self.pos[k.0 as usize], self.pos[k.1 as usize]));
                        edges.push((l, k.0, k.1));
                    }
                }
            }
            edges.sort_by(|x, y| y.0.total_cmp(&x.0));
            let mut changed = false;
            let mut new_verts = Vec::new();
            for &(l, a, b) in &edges {
                if l <= long {
                    break;
                }
                if self.edge_tris(a, b).is_empty() {
                    continue;
                }
                let cur = length(sub(self.pos[a as usize], self.pos[b as usize]));
                if cur > long {
                    new_verts.push(self.split(a, b));
                    changed = true;
                }
            }
            for &(l, a, b) in edges.iter().rev() {
                if l >= short {
                    break;
                }
                if self.vtris[a as usize].is_empty() || self.vtris[b as usize].is_empty() {
                    continue;
                }
                let cur = length(sub(self.pos[a as usize], self.pos[b as usize]));
                if cur < short && self.collapse(a, b) {
                    changed = true;
                }
            }
            // Tangential relax of new points so the triangles stay even.
            for &v in &new_verts {
                let nb = self.neighbours(v);
                if nb.is_empty() {
                    continue;
                }
                let mut avg = [0.0; 3];
                for &u in &nb {
                    avg = add(avg, self.pos[u as usize]);
                }
                avg = scale(avg, 1.0 / nb.len() as f32);
                let n = self.normals[v as usize];
                let d = sub(avg, self.pos[v as usize]);
                let tang = sub(d, scale(n, dot(d, n)));
                let to = add(self.pos[v as usize], scale(tang, 0.5));
                self.move_vertex(v, to);
            }
            let mut ring: Vec<u32> = new_verts.clone();
            for &v in &new_verts {
                ring.extend(self.neighbours(v));
            }
            self.update_normals(&ring);
            if !changed {
                break;
            }
        }
    }

    // ----- masks and face sets -----

    /// Masks hollows (cavities) — protecting or picking crevices.
    pub fn mask_cavity(&mut self, amount: f32) {
        for v in 0..self.pos.len() {
            let nb = self.neighbours(v as u32);
            if nb.is_empty() {
                continue;
            }
            let mut avg = [0.0; 3];
            for &u in &nb {
                avg = add(avg, self.pos[u as usize]);
            }
            avg = scale(avg, 1.0 / nb.len() as f32);
            let lap = sub(avg, self.pos[v]);
            let edge = nb.iter().map(|&u| length(sub(self.pos[u as usize], self.pos[v]))).sum::<f32>() / nb.len() as f32;
            let concave = dot(lap, self.normals[v]) / edge.max(1e-9);
            self.mask[v] = (concave * 4.0 * amount).clamp(0.0, 1.0);
        }
    }

    /// Masks points whose normal is within `degrees` of a direction.
    pub fn mask_facing(&mut self, dir: V3, degrees: f32) {
        let d = normalise(dir);
        let c = degrees.to_radians().cos();
        for v in 0..self.pos.len() {
            self.mask[v] = if dot(self.normals[v], d) >= c { 1.0 } else { 0.0 };
        }
    }

    pub fn mask_invert(&mut self) {
        self.mask.iter_mut().for_each(|m| *m = 1.0 - *m);
    }

    pub fn mask_clear(&mut self) {
        self.mask.iter_mut().for_each(|m| *m = 0.0);
    }

    /// Softens (or, with negative `iterations`, sharpens) the mask's edge.
    pub fn mask_blur(&mut self, iterations: i32) {
        for _ in 0..iterations.unsigned_abs() {
            let cur = self.mask.clone();
            for v in 0..self.pos.len() {
                let nb = self.neighbours(v as u32);
                if nb.is_empty() {
                    continue;
                }
                let avg = nb.iter().map(|&u| cur[u as usize]).sum::<f32>() / nb.len() as f32;
                self.mask[v] = if iterations > 0 { (cur[v] + avg) / 2.0 } else { (cur[v] + (cur[v] - avg)).clamp(0.0, 1.0) };
            }
        }
    }

    /// Masks everything in face set `set` (or everything else).
    pub fn mask_face_set(&mut self, set: u32, others: bool) {
        for v in 0..self.pos.len() {
            let inside = self.vtris[v].iter().any(|&t| self.tri_set[t as usize] == set);
            self.mask[v] = if inside != others { 1.0 } else { 0.0 };
        }
    }

    /// Face sets from connected pieces (each loose part its own set).
    pub fn face_sets_from_parts(&mut self) -> u32 {
        let nt = self.tris.len();
        let mut set = vec![u32::MAX; nt];
        let mut next = 1;
        for s in 0..nt {
            if !self.alive[s] || set[s] != u32::MAX {
                continue;
            }
            let mut stack = vec![s as u32];
            set[s] = next;
            while let Some(t) = stack.pop() {
                for v in self.tris[t as usize] {
                    for &u in &self.vtris[v as usize] {
                        if self.alive[u as usize] && set[u as usize] == u32::MAX {
                            set[u as usize] = next;
                            stack.push(u);
                        }
                    }
                }
            }
            next += 1;
        }
        for t in 0..nt {
            if self.alive[t] {
                self.tri_set[t] = set[t];
            }
        }
        next - 1
    }

    /// Face sets split where the surface turns by more than `degrees`.
    pub fn face_sets_from_normals(&mut self, degrees: f32) -> u32 {
        let c = degrees.to_radians().cos();
        let nt = self.tris.len();
        let n: Vec<V3> = (0..nt).map(|t| normalise(self.tri_normal(t))).collect();
        let mut set = vec![u32::MAX; nt];
        let mut next = 1;
        for s in 0..nt {
            if !self.alive[s] || set[s] != u32::MAX {
                continue;
            }
            let mut stack = vec![s as u32];
            set[s] = next;
            while let Some(t) = stack.pop() {
                let tri = self.tris[t as usize];
                for k in 0..3 {
                    for u in self.edge_tris(tri[k], tri[(k + 1) % 3]) {
                        if set[u as usize] == u32::MAX && dot(n[u as usize], n[s]) >= c {
                            set[u as usize] = next;
                            stack.push(u);
                        }
                    }
                }
            }
            next += 1;
        }
        for t in 0..nt {
            if self.alive[t] {
                self.tri_set[t] = set[t];
            }
        }
        next - 1
    }
}

fn mirror_set(sym: [bool; 3]) -> Vec<[bool; 3]> {
    let mut out = vec![[false; 3]];
    for a in 0..3 {
        if sym[a] {
            let more: Vec<[bool; 3]> = out
                .iter()
                .map(|m| {
                    let mut x = *m;
                    x[a] = true;
                    x
                })
                .collect();
            out.extend(more);
        }
    }
    out
}

fn flip(p: V3, m: &[bool; 3]) -> V3 {
    [if m[0] { -p[0] } else { p[0] }, if m[1] { -p[1] } else { p[1] }, if m[2] { -p[2] } else { p[2] }]
}

/// Moves a model's points from one sculpt to another of the same
/// vertices (used by multires and undo).
pub fn positions_changed(a: &[V3], b: &[V3]) -> Vec<u32> {
    a.iter().zip(b).enumerate().filter(|(_, (x, y))| x != y).map(|(i, _)| i as u32).collect()
}

/// Statistics in words.
pub fn describe(s: &SculptMesh) -> HashMap<&'static str, f64> {
    let masked = s.mask.iter().filter(|m| **m > 0.5).count();
    HashMap::from([("triangles", s.live_triangles() as f64), ("vertices", s.pos.len() as f64), ("masked", masked as f64)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    fn ball() -> SculptMesh {
        SculptMesh::from_poly(&prim::quad_sphere(100.0, 24))
    }

    fn brush(kind: Kind, radius: f32) -> Brush {
        Brush { kind, radius, strength: 0.5, falloff: Falloff::Smooth, invert: false, alpha: None, front_only: true, autosmooth: 0.0, face_set: 0 }
    }

    fn top(s: &SculptMesh) -> f32 {
        s.pos.iter().map(|p| -p[1]).fold(f32::MIN, f32::max)
    }

    #[test]
    fn draw_raises_and_invert_carves() {
        let mut s = ball();
        let before = top(&s);
        let st = Stroke { points: vec![[0.0, -100.0, 0.0]], spacing: 0.1, symmetry: [false; 3], dyntopo: None };
        for _ in 0..10 {
            s.stroke(&brush(Kind::Draw, 40.0), &st);
        }
        assert!(top(&s) > before + 10.0, "{} -> {}", before, top(&s));
        let mut c = ball();
        let mut b = brush(Kind::Draw, 40.0);
        b.invert = true;
        for _ in 0..10 {
            c.stroke(&b, &st);
        }
        assert!(top(&c) < before - 5.0);
    }

    #[test]
    fn symmetry_mirrors_the_stroke() {
        let mut s = ball();
        let st = Stroke { points: vec![[60.0, -60.0, 50.0]], spacing: 0.1, symmetry: [true, false, false], dyntopo: None };
        for _ in 0..8 {
            s.stroke(&brush(Kind::Inflate, 30.0), &st);
        }
        // The far side rose the same.
        let near = |x: f32| s.pos.iter().filter(|p| (p[0] - x).abs() < 15.0 && (p[1] + 60.0).abs() < 15.0 && (p[2] - 50.0).abs() < 15.0).map(|p| length(*p)).fold(0.0f32, f32::max);
        let (a, b) = (near(60.0), near(-60.0));
        assert!(a > 105.0 && (a - b).abs() < 2.0, "{a} {b}");
    }

    #[test]
    fn masks_protect() {
        let mut s = ball();
        s.mask_facing([0.0, -1.0, 0.0], 60.0);
        let before = top(&s);
        let st = Stroke { points: vec![[0.0, -100.0, 0.0]], spacing: 0.1, symmetry: [false; 3], dyntopo: None };
        for _ in 0..10 {
            s.stroke(&brush(Kind::Draw, 40.0), &st);
        }
        assert!((top(&s) - before).abs() < 1e-3);
    }

    #[test]
    fn smooth_and_flatten_even_out() {
        let mut s = ball();
        // Rough it up.
        let mut b = brush(Kind::Draw, 30.0);
        b.alpha = Some(Alpha { pattern: Pattern::Rock, size: 8.0, depth: 1.0, seed: 1 });
        b.strength = 1.0;
        let st = Stroke { points: vec![[0.0, -100.0, 0.0]], spacing: 0.1, symmetry: [false; 3], dyntopo: None };
        for _ in 0..5 {
            s.stroke(&b, &st);
        }
        let rough = |s: &SculptMesh| {
            let near: Vec<u32> = s.around([0.0, -100.0, 0.0], 25.0);
            let mut e = 0.0;
            for &v in &near {
                let nb = s.neighbours(v);
                let mut avg = [0.0; 3];
                for &u in &nb {
                    avg = add(avg, s.pos[u as usize]);
                }
                e += length(sub(scale(avg, 1.0 / nb.len() as f32), s.pos[v as usize]));
            }
            e / near.len() as f32
        };
        let r0 = rough(&s);
        for _ in 0..5 {
            s.stroke(&brush(Kind::Smooth, 40.0), &st);
        }
        assert!(rough(&s) < r0 * 0.8, "{r0} -> {}", rough(&s));
    }

    #[test]
    fn grab_moves_a_region() {
        let mut s = ball();
        let st = Stroke { points: vec![[100.0, 0.0, 0.0], [160.0, 0.0, 0.0]], spacing: 0.1, symmetry: [false; 3], dyntopo: None };
        let mut b = brush(Kind::Grab, 40.0);
        b.strength = 1.0;
        s.stroke(&b, &st);
        let far = s.pos.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        assert!(far > 150.0, "{far}");
    }

    #[test]
    fn dyntopo_adds_detail_where_the_brush_goes() {
        let mut s = SculptMesh::from_poly(&prim::quad_sphere(100.0, 6));
        let before = s.live_triangles();
        let st = Stroke { points: vec![[0.0, -100.0, 0.0], [60.0, -80.0, 0.0]], spacing: 0.2, symmetry: [false; 3], dyntopo: Some(Dyntopo { detail: 4.0 }) };
        let mut b = brush(Kind::SnakeHook, 30.0);
        b.strength = 1.0;
        let rep = s.stroke(&b, &st);
        assert!(rep.triangles_after > before * 2, "{before} -> {}", rep.triangles_after);
        let p = s.to_poly();
        assert!(crate::poly::Topology::of(&p).boundary_edges().is_empty(), "dyntopo tore the surface");
        // Edges near the stroke are near the detail size.
        let near = s.around([0.0, -100.0, 0.0], 20.0);
        let mut longest = 0.0f32;
        for &v in &near {
            for u in s.neighbours(v) {
                longest = longest.max(length(sub(s.pos[v as usize], s.pos[u as usize])));
            }
        }
        assert!(longest < 4.0 * 1.25 * 1.6, "{longest}");
    }

    #[test]
    fn face_sets_from_parts_and_normals() {
        let mut p = prim::cube([10.0; 3], 2);
        let mut q = prim::cube([10.0; 3], 2);
        q.map_positions(|v| add(v, [30.0, 0.0, 0.0]));
        p.merge(&q);
        let mut s = SculptMesh::from_poly(&p);
        assert_eq!(s.face_sets_from_parts(), 2);
        assert_eq!(s.face_sets_from_normals(30.0), 12);
        s.hidden.insert(1);
        let st = Stroke { points: vec![[0.0, -5.0, 0.0]], spacing: 0.1, symmetry: [false; 3], dyntopo: None };
        let before = s.pos.clone();
        let mut b = brush(Kind::Draw, 30.0);
        b.front_only = false;
        s.stroke(&b, &st);
        let _ = before;
    }
}
