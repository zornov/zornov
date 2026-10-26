// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The open model's cage over the 3D workspace: vertices, edges and faces
//! (picked ones in orange, the active one brighter), seams, sharp edges
//! and creases, normals and face orientation when asked, and what is
//! behind the surface left out unless the view is see-through.

use super::{Session, with};
use crate::app::StudioApp;
use crate::editor::viewport::ui::Pane;
use crate::editor::viewport::{Shade, Workspace};
use eframe::egui::{self, Color32, Pos2, Stroke};
use std::cell::RefCell;
use studio_mesh::V3;
use studio_mesh::bvh::Bvh;
use studio_model as m;
use studio_model::space as sp;
use studio_modeling::poly::{Poly, triangulate};
use studio_modeling::selection::{Elem, Mode};

pub const PICKED: Color32 = Color32::from_rgb(255, 160, 40);
pub const ACTIVE: Color32 = Color32::from_rgb(255, 236, 190);
const WIRE: Color32 = Color32::from_rgb(20, 20, 24);
const WIRE_XRAY: Color32 = Color32::from_rgb(210, 210, 215);
const SEAM: Color32 = Color32::from_rgb(235, 70, 60);
const SHARP: Color32 = Color32::from_rgb(60, 200, 235);
const CREASE: Color32 = Color32::from_rgb(220, 70, 220);

/// The open part's place in the world: part → world and back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    pub to_world: [[f64; 4]; 3],
    pub from_world: [[f64; 4]; 3],
}

fn mul(a: &[[f64; 4]; 3], b: &[[f64; 4]; 3]) -> [[f64; 4]; 3] {
    std::array::from_fn(|r| {
        let mut row = [0.0; 4];
        for c in 0..4 {
            row[c] = (0..3).map(|k| a[r][k] * b[k][c]).sum::<f64>() + if c == 3 { a[r][3] } else { 0.0 };
        }
        row
    })
}

pub fn invert(m: &[[f64; 4]; 3]) -> Option<[[f64; 4]; 3]> {
    let a = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-18 {
        return None;
    }
    let inv: [[f64; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let (i1, i2) = ((j + 1) % 3, (j + 2) % 3);
            let (j1, j2) = ((i + 1) % 3, (i + 2) % 3);
            (a[i1][j1] * a[i2][j2] - a[i1][j2] * a[i2][j1]) / det
        })
    });
    let t = [m[0][3], m[1][3], m[2][3]];
    Some(std::array::from_fn(|r| [inv[r][0], inv[r][1], inv[r][2], -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2])]))
}

/// The time the workspace shows.
pub fn time_of(app: &StudioApp) -> i64 {
    app.editor.hover_time.unwrap_or(app.editor.playhead)
}

impl Space {
    /// Where the open part is now.
    pub fn of(app: &StudioApp, s: &Session) -> Option<Space> {
        let seq = app.doc.model.active();
        let mc = seq.clip(m::ClipId(s.clip))?;
        let w = app.doc.project.width as f64;
        let t = time_of(app).clamp(mc.start, mc.start + mc.duration.max(1) - 1);
        let tr = sp::oriented_transform(seq, mc, t, w);
        let clip_mx = crate::editor::viewport::model_matrix(&tr);
        let part = s.part();
        let r = studio_mesh::ops::rotation(part.rotation);
        let k = part.scale;
        let place: [[f64; 4]; 3] = std::array::from_fn(|i| [r[i][0] as f64 * k[0] as f64, r[i][1] as f64 * k[1] as f64, r[i][2] as f64 * k[2] as f64, part.position[i] as f64]);
        let to_world = mul(&clip_mx, &place);
        Some(Space { to_world, from_world: invert(&to_world)? })
    }

    pub fn world(&self, p: V3) -> [f64; 3] {
        let m = &self.to_world;
        std::array::from_fn(|r| m[r][0] * p[0] as f64 + m[r][1] * p[1] as f64 + m[r][2] * p[2] as f64 + m[r][3])
    }

    pub fn to_local(&self, w: [f64; 3]) -> V3 {
        let m = &self.from_world;
        std::array::from_fn(|r| (m[r][0] * w[0] + m[r][1] * w[1] + m[r][2] * w[2] + m[r][3]) as f32)
    }

    pub fn dir_to_local(&self, d: [f64; 3]) -> V3 {
        let m = &self.from_world;
        let v: [f64; 3] = std::array::from_fn(|r| m[r][0] * d[0] + m[r][1] * d[1] + m[r][2] * d[2]);
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        v.map(|x| (x / l) as f32)
    }

    pub fn dir_to_world(&self, d: V3) -> [f64; 3] {
        let m = &self.to_world;
        std::array::from_fn(|r| m[r][0] * d[0] as f64 + m[r][1] * d[1] as f64 + m[r][2] * d[2] as f64)
    }
}

/// The cage's triangles for hits and occlusion (part space), each with
/// its face.
pub struct Hits {
    pub version: u64,
    pub bvh: Bvh,
    pub face_of: Vec<u32>,
    /// Every edge, and what is not hidden: worked out once per change.
    pub edges: Vec<studio_modeling::Edge>,
    pub vis: studio_modeling::selection::Visible,
}

thread_local! {
    static HITS: RefCell<Option<Hits>> = const { RefCell::new(None) };
}

/// Runs `f` with the cage's triangles (built when the mesh changed).
pub fn with_hits<R>(s: &Session, f: impl FnOnce(&Hits) -> R) -> R {
    HITS.with(|h| {
        let mut h = h.borrow_mut();
        if h.as_ref().is_none_or(|x| x.version != s.version) {
            let p = s.poly();
            let mut tris = Vec::new();
            let mut face_of = Vec::new();
            let vis = s.sel.visible(p);
            for f in 0..p.faces() {
                if !vis.faces[f] {
                    continue;
                }
                let c = p.face(f);
                for t in triangulate(&p.positions, c) {
                    tris.push([p.positions[c[t[0]] as usize], p.positions[c[t[1]] as usize], p.positions[c[t[2]] as usize]]);
                    face_of.push(f as u32);
                }
            }
            *h = Some(Hits { version: s.version, bvh: Bvh::new(tris), face_of, edges: p.edges(), vis });
        }
        f(h.as_ref().unwrap())
    })
}

/// Whether the view sees through the surface.
pub fn see_through(ws: &Workspace) -> bool {
    matches!(ws.shade, Shade::Xray | Shade::Wireframe)
}

/// The eye in part space.
pub fn eye_local(space: &Space, pane: &Pane) -> V3 {
    space.to_local(pane.view.position)
}

/// Which vertices the eye can see (not behind the cage). All of them when
/// the view is see-through.
pub fn seen(s: &mut Session, space: &Space, pane: &Pane, ws: &Workspace) -> Vec<bool> {
    let n = s.poly().positions.len();
    if see_through(ws) {
        return vec![true; n];
    }
    let eye = eye_local(space, pane);
    let q = |x: f32| (x * 4.0).round() as i64;
    let tq = |x: f64| (x * 4.0).round() as i64;
    let key = (s.version, [q(eye[0]), q(eye[1]), q(eye[2]), tq(space.to_world[0][3]), tq(space.to_world[1][3]), tq(space.to_world[2][3])]);
    if s.cage.key == key && s.cage.seen.len() == n {
        return s.cage.seen.clone();
    }
    let out: Vec<bool> = with_hits(s, |h| {
        let p = s.poly();
        let (lo, hi) = p.bounds();
        let eps = studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0) * 1e-4;
        p.positions
            .iter()
            .map(|&v| {
                let d = studio_mesh::sub(v, eye);
                let len = studio_mesh::length(d);
                if len < 1e-6 {
                    return true;
                }
                let dir = studio_mesh::scale(d, 1.0 / len);
                !h.bvh.occluded(eye, dir, 0.0, len - eps * 4.0)
            })
            .collect()
    });
    s.cage = super::Cage { key, seen: out.clone() };
    out
}

/// Screen points of every vertex.
pub fn screen_points(p: &Poly, space: &Space, pane: &Pane) -> Vec<Option<Pos2>> {
    p.positions.iter().map(|&v| pane.to_screen(space.world(v))).collect()
}

/// Draws the open model's cage on a pane.
pub fn overlay(app: &StudioApp, painter: &egui::Painter, pane: &Pane, ws: &Workspace) {
    let Some(mut s) = with(|s| s.clone()) else { return };
    if s.stage != super::Stage::Edit || pane.camera.is_some() {
        return;
    }
    let Some(space) = Space::of(app, &s) else { return };
    let seen = seen(&mut s, &space, pane, ws);
    with(|cur| {
        if let Some(cur) = cur {
            cur.cage = s.cage.clone();
        }
    });
    let p = s.poly();
    let pts = screen_points(p, &space, pane);
    let (edges, vis) = with_hits(&s, |h| (h.edges.clone(), h.vis.clone()));
    let xray = see_through(ws);
    let wire = if xray || ws.shade == Shade::Wireframe { WIRE_XRAY } else { WIRE };
    let mut shapes: Vec<egui::Shape> = Vec::new();
    let eye = eye_local(&space, pane);
    // Faces: picked ones tinted, face orientation over all.
    for f in 0..p.faces() {
        if !vis.faces[f] {
            continue;
        }
        let c = p.face(f);
        let face_seen = c.iter().any(|&v| seen[v as usize]);
        if !face_seen {
            continue;
        }
        let picked = s.sel.faces.get(f).copied().unwrap_or(false);
        let tint = if picked {
            Some(Color32::from_rgba_unmultiplied(255, 160, 40, if xray { 50 } else { 70 }))
        } else if s.show.face_orientation {
            let toward = studio_mesh::dot(p.face_normal(f), studio_mesh::sub(eye, p.face_centre(f))) > 0.0;
            Some(if toward { Color32::from_rgba_unmultiplied(60, 110, 255, 70) } else { Color32::from_rgba_unmultiplied(240, 50, 50, 90) })
        } else {
            None
        };
        if let Some(col) = tint {
            for t in triangulate(&p.positions, c) {
                if let (Some(a), Some(b), Some(cc)) = (pts[c[t[0]] as usize], pts[c[t[1]] as usize], pts[c[t[2]] as usize]) {
                    shapes.push(egui::Shape::convex_polygon(vec![a, b, cc], col, Stroke::NONE));
                }
            }
        }
    }
    // Edges.
    let active_edge = match s.sel.active {
        Some(Elem::Edge(e)) => Some(e),
        _ => None,
    };
    for &e in &edges {
        if !vis.edge(e) {
            continue;
        }
        let both_seen = seen[e.0 as usize] && seen[e.1 as usize];
        if !both_seen {
            continue;
        }
        let (Some(a), Some(b)) = (pts[e.0 as usize], pts[e.1 as usize]) else { continue };
        let picked = s.sel.edges.contains(&e);
        let (w, col) = if Some(e) == active_edge {
            (2.4, ACTIVE)
        } else if picked {
            (2.0, PICKED)
        } else if s.show.seams && p.seams.contains(&e) {
            (1.8, SEAM)
        } else if s.show.sharp && p.sharp.contains(&e) {
            (1.6, SHARP)
        } else if s.show.creases && p.creases.get(&e).is_some_and(|w| *w > 0.0) {
            (1.6, CREASE)
        } else {
            (1.0, wire)
        };
        shapes.push(egui::Shape::line_segment([a, b], Stroke::new(w, col)));
    }
    // Vertices (vertex mode) or face dots (face mode).
    let active_v = match s.sel.active {
        Some(Elem::Vertex(v)) => Some(v),
        _ => None,
    };
    match s.sel.mode {
        Mode::Vertex => {
            for v in 0..p.positions.len() {
                if !vis.verts[v] || !seen[v] {
                    continue;
                }
                let Some(at) = pts[v] else { continue };
                let picked = s.sel.verts[v];
                let col = if Some(v as u32) == active_v { ACTIVE } else if picked { PICKED } else { Color32::BLACK };
                shapes.push(egui::Shape::circle_filled(at, if picked { 3.2 } else { 2.6 }, col));
            }
        }
        Mode::Face => {
            for f in 0..p.faces() {
                if !vis.faces[f] || !p.face(f).iter().any(|&v| seen[v as usize]) {
                    continue;
                }
                if let Some(at) = pane.to_screen(space.world(p.face_centre(f))) {
                    let picked = s.sel.faces[f];
                    let col = if s.sel.active == Some(Elem::Face(f as u32)) { ACTIVE } else if picked { PICKED } else { Color32::from_black_alpha(160) };
                    shapes.push(egui::Shape::rect_filled(egui::Rect::from_center_size(at, egui::vec2(3.0, 3.0)), 0.0, col));
                }
            }
        }
        Mode::Edge => {}
    }
    // Normals.
    let (lo, hi) = p.bounds();
    let reach = studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0) * 0.06;
    if s.show.face_normals {
        for f in 0..p.faces() {
            if !vis.faces[f] || !p.face(f).iter().any(|&v| seen[v as usize]) {
                continue;
            }
            let c = p.face_centre(f);
            let tip = studio_mesh::add(c, studio_mesh::scale(p.face_normal(f), reach));
            if let (Some(a), Some(b)) = (pane.to_screen(space.world(c)), pane.to_screen(space.world(tip))) {
                shapes.push(egui::Shape::line_segment([a, b], Stroke::new(1.2, Color32::from_rgb(70, 220, 230))));
            }
        }
    }
    if s.show.vertex_normals {
        let normals = studio_modeling::modifiers::vertex_normals(p);
        for v in 0..p.positions.len() {
            if !vis.verts[v] || !seen[v] {
                continue;
            }
            let tip = studio_mesh::add(p.positions[v], studio_mesh::scale(normals[v], reach));
            if let (Some(a), Some(b)) = (pts[v], pane.to_screen(space.world(tip))) {
                shapes.push(egui::Shape::line_segment([a, b], Stroke::new(1.0, Color32::from_rgb(90, 140, 255))));
            }
        }
    }
    painter.extend(shapes);
    // Proportional editing's reach during a transform.
    if let Some(md) = &s.modal
        && s.prop.on
        && s.prop.radius > 0.0
        && let Some(c) = pane.to_screen(space.world(md.pivot))
    {
        let edge = studio_mesh::add(md.pivot, studio_mesh::scale(space.dir_to_local(pane.view.axes[0]), s.prop.radius));
        if let Some(e) = pane.to_screen(space.world(edge)) {
            painter.circle_stroke(c, c.distance(e), Stroke::new(1.0, Color32::from_white_alpha(140)));
        }
    }
    super::lattice::overlay(app, painter, pane);
    if s.show.stats
        && let Some(line) = super::status()
    {
        let th = crate::theme::tokens(painter.ctx());
        painter.text(pane.rect.left_bottom() + egui::vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, line, crate::theme::caption(), th.text);
    }
}

/// The nearest vertex, edge or face under a screen point (what a click
/// picks in the current mode; `any` finds an edge in every mode, for loop
/// picks).
pub fn under(app: &StudioApp, s: &mut Session, pane: &Pane, ws: &Workspace, at: Pos2, want: Mode) -> Option<Elem> {
    let space = Space::of(app, s)?;
    let seen = seen(s, &space, pane, ws);
    let p = s.poly();
    let vis = s.sel.visible(p);
    let pts = screen_points(p, &space, pane);
    match want {
        Mode::Vertex => {
            let mut best: Option<(f32, u32)> = None;
            for v in 0..p.positions.len() {
                if !vis.verts[v] || !seen[v] {
                    continue;
                }
                if let Some(q) = pts[v] {
                    let d = q.distance(at);
                    if d < 14.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, v as u32));
                    }
                }
            }
            best.map(|b| Elem::Vertex(b.1))
        }
        Mode::Edge => {
            let mut best: Option<(f32, studio_modeling::Edge)> = None;
            for e in p.edges() {
                if !vis.edge(e) || !(seen[e.0 as usize] && seen[e.1 as usize]) {
                    continue;
                }
                if let (Some(a), Some(b)) = (pts[e.0 as usize], pts[e.1 as usize]) {
                    let d = seg_dist(at, a, b);
                    if d < 10.0 && best.is_none_or(|x| d < x.0) {
                        best = Some((d, e));
                    }
                }
            }
            best.map(|b| Elem::Edge(b.1))
        }
        Mode::Face => {
            // The face the ray meets first (in a see-through view, also
            // faces behind: the nearest by its centre on screen breaks ties).
            let (o, d) = ray_local(&space, pane, at)?;
            let hit = with_hits(s, |h| h.bvh.ray(o, d, 0.0, f32::INFINITY).map(|r| h.face_of[r.tri as usize]));
            if let Some(f) = hit {
                return Some(Elem::Face(f));
            }
            None
        }
    }
}

/// A screen point's ray in part space (origin, unit direction).
pub fn ray_local(space: &Space, pane: &Pane, at: Pos2) -> Option<(V3, V3)> {
    let c = pane.to_canvas(at);
    let d = pane.view.ray(m::Vec2::new(c[0], c[1]));
    let o = space.to_local(pane.view.position);
    let dl = space.dir_to_local(d);
    Some((o, dl))
}

pub fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
