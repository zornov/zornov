// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sculpt mode in the 3D workspace: the open model shaped with brushes
//! where the pointer drags on it, seen from the workspace's eye. Dynamic
//! detail splits edges longer than a size under the brush as it goes, so
//! a coarse shape gains the faces fine strokes need; Remesh rebuilds the
//! surface from voxels into even quads. A stroke is one undo step.

use super::draw::{self, Space};
use super::{Stage, with};
use crate::app::StudioApp;
use crate::editor::viewport::ui::Pane;
use crate::icons::Icon;
use crate::kit::{self, IconButton};
use eframe::egui::{self, Key, Modifiers, Stroke};
use std::cell::RefCell;
use std::collections::HashSet;
use studio_mesh::ops::Brush;
use studio_mesh::{Mesh, V3};
use studio_modeling::poly::{Poly, triangulate};

pub const BRUSHES: [&str; 7] = ["Draw", "Carve", "Grab", "Smooth", "Flatten", "Pinch", "Crease"];

/// Sculpt mode's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brushes {
    pub brush: usize,
    /// Screen points.
    pub radius: f32,
    /// 0…1.
    pub strength: f32,
    pub symmetry: bool,
    /// Split edges longer than `detail` px (screen) under the brush.
    pub dyntopo: bool,
    pub detail: f32,
    /// Voxel size of Remesh, as a share of the model's size.
    pub voxel: f32,
}

impl Default for Brushes {
    fn default() -> Brushes {
        Brushes { brush: 0, radius: 50.0, strength: 0.5, symmetry: true, dyntopo: false, detail: 8.0, voxel: 0.02 }
    }
}

thread_local! {
    static BRUSH: RefCell<Brushes> = RefCell::new(Brushes::default());
    static STROKE: RefCell<Option<Stroke3>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug)]
struct Stroke3 {
    last: Option<V3>,
    dabs: u32,
}

pub fn brushes() -> Brushes {
    BRUSH.with(|b| *b.borrow())
}

pub fn set_brushes(b: Brushes) {
    BRUSH.with(|x| *x.borrow_mut() = b);
}

pub fn begin(ctx: &egui::Context) {
    ctx.request_repaint();
}

pub fn end(_ctx: &egui::Context) {
    STROKE.with(|s| *s.borrow_mut() = None);
}

/// Sculpt mode's keys: [ ] brush size, Shift+[ ] strength, 1–7 brushes,
/// Esc back to object mode.
pub fn keys(app: &mut StudioApp, ctx: &egui::Context) -> bool {
    let mut b = brushes();
    let before = b;
    let shift = ctx.input(|i| i.modifiers.shift);
    let (smaller, bigger) = ctx.input(|i| (i.key_pressed(Key::OpenBracket), i.key_pressed(Key::CloseBracket)));
    if smaller || bigger {
        if shift {
            b.strength = (b.strength + if bigger { 0.05 } else { -0.05 }).clamp(0.01, 1.0);
        } else {
            b.radius = (b.radius * if bigger { 1.2 } else { 1.0 / 1.2 }).clamp(2.0, 800.0);
        }
    }
    for (i, k) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7].iter().enumerate() {
        if ctx.input_mut(|inp| inp.consume_key(Modifiers::NONE, *k)) {
            b.brush = i;
        }
    }
    if b != before {
        set_brushes(b);
        return true;
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        let _ = super::enter(app, ctx, Stage::Edit);
        return true;
    }
    false
}

/// Sculpt mode's part of the bar.
pub fn bar(app: &mut StudioApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut b = brushes();
    let before = b;
    if let Some(i) = kit::menu_button(ui, "sculpt3d-brush", b.brush, &BRUSHES, 96.0, None) {
        b.brush = i;
    }
    kit::NumberField::new("Brush size", &mut b.radius).range(2.0..=800.0).unit("pt").width(70.0).show(ui).on_hover_text(crate::i18n::t("Brush size ([ and ])"));
    let mut pct = b.strength * 100.0;
    if kit::NumberField::new("Strength", &mut pct).range(1.0..=100.0).unit("%").width(64.0).show(ui).on_hover_text(crate::i18n::t("Strength (Shift+[ and ])")).changed() {
        b.strength = pct / 100.0;
    }
    if IconButton::new(Icon::ArrowsHorizontal, "Symmetry across X").active(b.symmetry).compact().show(ui).clicked() {
        b.symmetry = !b.symmetry;
    }
    let dy = if b.dyntopo { kit::Button::secondary("Dynamic detail") } else { kit::Button::ghost("Dynamic detail") };
    if dy.compact().show(ui).on_hover_text(crate::i18n::t("Splits long edges under the brush so strokes have the faces they need")).clicked() {
        b.dyntopo = !b.dyntopo;
    }
    if b.dyntopo {
        kit::NumberField::new("Detail", &mut b.detail).range(1.0..=100.0).unit("pt").width(64.0).show(ui).on_hover_text(crate::i18n::t("Edges longer than this on screen are split"));
    }
    let mut vox = b.voxel * 100.0;
    if kit::NumberField::new("Voxel size", &mut vox).range(0.2..=20.0).decimals(1).unit("%").width(64.0).show(ui).on_hover_text(crate::i18n::t("Remesh voxel size, a share of the model's size")).changed() {
        b.voxel = vox / 100.0;
    }
    if kit::Button::ghost("Remesh").compact().show(ui).on_hover_text(crate::i18n::t("Rebuilds the surface as even quads (closes holes, joins pieces)")).clicked() {
        if let Err(e) = remesh(app, b.voxel) {
            kit::push(&ctx, e, None, Some(Icon::Warning));
        }
    }
    if b != before {
        set_brushes(b);
    }
}

/// Rebuilds the open part from voxels `share` × its size across.
pub fn remesh(app: &mut StudioApp, share: f32) -> Result<(), String> {
    let Some(mut s) = with(|s| s.clone()) else { return Err("no model is open".into()) };
    let p = s.poly();
    let (lo, hi) = p.bounds();
    let size = studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0) * share.clamp(0.002, 0.2);
    let mats = p.materials.clone();
    let mut out = studio_modeling::remesh::voxel(p, size)?;
    out.materials = mats;
    out.ensure_materials();
    for f in &mut out.face_smooth {
        *f = true;
    }
    let part = s.part;
    s.asset.parts[part].mesh = out;
    s.sel = studio_modeling::selection::Selection::new(s.poly(), s.sel.mode);
    s.last = None;
    s.changed();
    with(|cur| *cur = Some(s));
    super::commit(app, "Remesh")
}

/// The part's own triangles over its vertices (same numbering).
fn cage_mesh(p: &Poly) -> Mesh {
    let mut m = Mesh { positions: p.positions.clone(), ..Mesh::default() };
    for f in 0..p.faces() {
        let c = p.face(f);
        for t in triangulate(&p.positions, c) {
            m.indices.extend_from_slice(&[c[t[0]], c[t[1]], c[t[2]]]);
        }
    }
    studio_mesh::ops::smooth_normals_in_place(&mut m);
    m
}

/// Splits edges longer than `limit` within `radius` of `at` (part space).
fn refine(p: &mut Poly, at: V3, radius: f32, limit: f32) -> bool {
    let long: HashSet<studio_modeling::Edge> = p
        .edges()
        .into_iter()
        .filter(|e| {
            let (a, b) = (p.positions[e.0 as usize], p.positions[e.1 as usize]);
            let mid = studio_mesh::lerp(a, b, 0.5);
            studio_mesh::length(studio_mesh::sub(mid, at)) < radius && studio_mesh::length(studio_mesh::sub(b, a)) > limit
        })
        .collect();
    if long.is_empty() || p.faces() > 400_000 {
        return false;
    }
    studio_modeling::ops::subdivide_edges(p, &long, 1);
    // Fresh faces from split outlines shade and sculpt as triangles.
    let big: Vec<bool> = (0..p.faces()).map(|f| p.face(f).len() > 4).collect();
    studio_modeling::edit::triangulate_faces(p, &big);
    true
}

/// The pointer over a pane in sculpt mode. Returns whether it took the
/// pointer (a stroke on the model).
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, pane: &Pane, resp: &egui::Response) -> bool {
    let ctx = ui.ctx().clone();
    let Some(mut s) = with(|s| s.clone()) else { return false };
    if s.stage != Stage::Sculpt || pane.camera.is_some() {
        return false;
    }
    let b = brushes();
    let Some(space) = Space::of(app, &s) else { return false };
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let (alt, primary) = ctx.input(|i| (i.modifiers.alt, i.pointer.primary_down()));
    if let Some(p) = pointer
        && pane.rect.contains(p)
    {
        let th = crate::theme::tokens(&ctx);
        ui.painter().circle_stroke(p, b.radius, Stroke::new(1.5, th.accent));
    }
    let stroking = STROKE.with(|st| st.borrow().is_some());
    if resp.drag_started() && primary && !alt {
        STROKE.with(|st| *st.borrow_mut() = Some(Stroke3 { last: None, dabs: 0 }));
        if !app.doc.in_batch() {
            app.doc.begin_batch("Sculpt");
        }
    }
    if (stroking || resp.drag_started()) && primary
        && let Some(at) = pointer
    {
        let Some((o, d)) = draw::ray_local(&space, pane, at) else { return true };
        let hit = draw::with_hits(&s, |h| h.bvh.ray(o, d, 0.0, f32::INFINITY).map(|r| r.t));
        let last = STROKE.with(|st| st.borrow().as_ref().and_then(|x| x.last));
        // World size of the brush at the hit, in part space.
        let target = match (b.brush, last, hit) {
            (2, Some(l), _) => {
                // Grab follows the pointer at the depth it started.
                let wl = space.world(l);
                let depth = pane.view.view(wl)[2];
                let c = pane.to_canvas(at);
                let ray = pane.view.ray(studio_model::Vec2::new(c[0], c[1]));
                let wp: [f64; 3] = std::array::from_fn(|j| pane.view.position[j] + ray[j] * depth);
                Some(space.to_local(wp))
            }
            (_, _, Some(t)) => Some(studio_mesh::add(o, studio_mesh::scale(d, t))),
            _ => None,
        };
        if let Some(q) = target {
            let wq = space.world(q);
            let depth = pane.view.view(wq)[2].max(1.0);
            let wpp = depth / pane.view.zoom / pane.scale as f64;
            let right = pane.view.axes[0];
            let edge = space.to_local([wq[0] + right[0] * b.radius as f64 * wpp, wq[1] + right[1] * b.radius as f64 * wpp, wq[2] + right[2] * b.radius as f64 * wpp]);
            let radius = studio_mesh::length(studio_mesh::sub(edge, q)).max(1e-4);
            let spacing = radius * 0.25;
            let far = last.is_none_or(|l| studio_mesh::length(studio_mesh::sub(q, l)) >= spacing);
            if far {
                let part = s.part;
                if b.dyntopo {
                    let limit = radius * b.detail / b.radius.max(1.0);
                    let poly = s.poly_mut();
                    if refine(poly, q, radius, limit) {
                        let pp = s.poly().clone();
                        s.sel = studio_modeling::selection::Selection::new(&pp, s.sel.mode);
                    }
                }
                let mut mesh = cage_mesh(s.poly());
                let k = b.strength.clamp(0.0, 1.0);
                let (brush, amount) = match b.brush {
                    0 => (Brush::Inflate, radius * 0.08 * k),
                    1 => (Brush::Inflate, -radius * 0.08 * k),
                    2 => (Brush::Grab { by: last.map_or([0.0; 3], |l| studio_mesh::sub(q, l)) }, 0.0),
                    3 => (Brush::Smooth, k),
                    4 => (Brush::Flatten, k * 0.5),
                    5 => (Brush::Pinch, radius * 0.1 * k),
                    _ => (Brush::Crease, radius * 0.06 * k),
                };
                let centre = if b.brush == 2 { last.unwrap_or(q) } else { q };
                if b.brush != 2 || last.is_some() {
                    studio_mesh::ops::sculpt(&mut mesh, brush, centre, radius, amount, b.symmetry);
                }
                s.asset.parts[part].mesh.positions = mesh.positions;
                s.changed();
                STROKE.with(|st| {
                    if let Some(x) = st.borrow_mut().as_mut() {
                        x.last = Some(q);
                        x.dabs += 1;
                    }
                });
                with(|cur| *cur = Some(s.clone()));
                super::preview(app);
            }
        }
        ctx.request_repaint();
    }
    if stroking && ctx.input(|i| i.pointer.any_released()) {
        let dabs = STROKE.with(|st| st.borrow_mut().take()).map_or(0, |x| x.dabs);
        if dabs > 0
            && let Err(e) = super::commit(app, "Sculpt")
        {
            kit::push(&ctx, e, None, Some(Icon::Warning));
        }
        if app.doc.in_batch() {
            app.doc.commit_batch();
        }
    }
    stroking || resp.drag_started() && primary && !alt
}
