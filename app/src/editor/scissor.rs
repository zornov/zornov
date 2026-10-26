// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The scissor tool on the canvas: cut lines that split the mesh a
//! layer's puppet pins and bones bend, so parts move apart (see
//! `studio_model::Cut`; the AI's `scissor` op works on the same cuts).
//!
//! With the tool on, a drag across the layer draws a cut (hold shift for a
//! straight line); drag a cut's dot to move that point, or the diamond on
//! one of its segments to move the whole cut; alt-click a dot to take that
//! point away (the last two points take the cut) and alt-click a diamond to
//! delete the cut. "Show mesh" draws the deformation mesh as it is bent now,
//! so you can see where it is split and where a gap opens.

use crate::app::StudioApp;
use crate::editor::pins::{self, LayerMap};
use crate::theme;
use crate::view::Time;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;

fn show_id() -> egui::Id {
    egui::Id::unique("canvas-scissor-mesh")
}

/// Whether the mesh is drawn over the layer.
pub fn show_mesh(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(show_id())).unwrap_or(false)
}

pub fn set_show_mesh(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(show_id(), on));
}

fn fill_id() -> egui::Id {
    egui::Id::unique("canvas-scissor-fill")
}

/// What a new cut fills its gap with: 0 nothing, 1 a mouth's dark inside,
/// 2 generated from the picture.
pub fn new_fill(ctx: &egui::Context) -> usize {
    ctx.data(|d| d.get_temp(fill_id())).unwrap_or(0)
}

pub fn set_new_fill(ctx: &egui::Context, k: usize) {
    ctx.data_mut(|d| d.insert_temp(fill_id(), k));
}

fn fill_of(k: usize) -> m::CutFill {
    match k {
        1 => {
            let [r, g, b, a] = studio_rig::scissor::MOUTH_INSIDE;
            m::CutFill::Colour(m::Rgba::new(r, g, b, a))
        }
        2 => m::CutFill::Auto,
        _ => m::CutFill::None,
    }
}

fn cut_colour(c: &m::Cut) -> Color32 {
    if c.enabled { Color32::from_rgb(244, 63, 94) } else { Color32::from_gray(140) }
}

/// A grabbed handle.
#[derive(Clone, Debug, PartialEq)]
enum Grab {
    /// Point `i` of cut `id`.
    Point(u32, usize),
    /// The whole of cut `id`, held where the pointer started.
    Whole(u32, usize),
}

#[derive(Clone, Debug)]
struct Drag {
    grab: Grab,
    /// The cut's points when the drag began, and the pointer's place in
    /// layer coordinates then.
    from: Vec<m::Vec2>,
    start: m::Vec2,
}

fn drag_id() -> egui::Id {
    egui::Id::unique("canvas-scissor-drag")
}

fn draw_id() -> egui::Id {
    egui::Id::unique("canvas-scissor-draw")
}

/// Fewest screen pixels between points of a cut being drawn.
const STEP: f32 = 4.0;

/// The tool's work on the selected layer. Returns `true` when it took the
/// pointer.
#[allow(clippy::too_many_arguments)]
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, area: Rect, scale: f32, id: u32, map: &LayerMap, mc: &m::Clip, shown: Time) -> bool {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let screen = |c: Vec2| canvas.center() + c * scale;
    let from_screen = |p: Pos2| (p - canvas.center()) / scale;
    let p = ui.painter().with_clip_rect(area.expand(8.0));

    // What the cuts split: the pins, shown but not grabbed.
    if let Some(pp) = mc.puppet_at(shown) {
        for pin in &pp.pins {
            pins::draw_pin(&p, screen(map.canvas(pin.position)), pin.kind, false, pin.rotation as f32);
        }
    }
    if show_mesh(&ctx) {
        draw_mesh(app, &p, id, map, mc, shown, &screen);
    }

    // The layer under the handles (made first, so the handles made after it
    // get the pointer where they are).
    let mut draw: Option<Vec<Pos2>> = ui.data(|d| d.get_temp(draw_id()));
    let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id()));
    let body = ui.interact(area, egui::Id::unique("canvas-scissor-body"), Sense::click_and_drag());
    let alt = ui.input(|i| i.modifiers.alt);
    let shift = ui.input(|i| i.modifiers.shift);
    let took = true;

    // The cuts.
    for c in &mc.cuts {
        let line: Vec<Pos2> = c.points.iter().map(|q| screen(map.canvas(*q))).collect();
        let stroke = Stroke::new(2.0, cut_colour(c));
        if c.enabled {
            p.add(egui::Shape::line(line.clone(), stroke));
        } else {
            p.add(egui::Shape::dashed_line(&line, stroke, 6.0, 4.0));
        }
        if !c.fill.is_none() {
            if let Some(mid) = line.get(line.len() / 2) {
                p.circle_filled(*mid + Vec2::new(0.0, -9.0), 3.0, match c.fill {
                    m::CutFill::Colour(k) => Color32::from_rgb((k.r * 255.0) as u8, (k.g * 255.0) as u8, (k.b * 255.0) as u8),
                    _ => Color32::from_gray(90),
                });
            }
        }
    }
    let mut handles: Vec<(Grab, Pos2)> = Vec::new();
    for c in &mc.cuts {
        for (i, q) in c.points.iter().enumerate() {
            handles.push((Grab::Point(c.id, i), screen(map.canvas(*q))));
        }
        for i in 0..c.points.len().saturating_sub(1) {
            let mid = m::Vec2::new((c.points[i].x + c.points[i + 1].x) * 0.5, (c.points[i].y + c.points[i + 1].y) * 0.5);
            handles.push((Grab::Whole(c.id, i), screen(map.canvas(mid))));
        }
    }
    let mut on_handle = false;
    for (g, at) in &handles {
        let resp = ui.interact(Rect::from_center_size(*at, Vec2::splat(14.0)), egui::Id::unique(("canvas-cut", id, format!("{g:?}"))), Sense::click_and_drag());
        let hot = resp.hovered() || resp.dragged();
        let cut = match g {
            Grab::Point(cid, _) | Grab::Whole(cid, _) => mc.cuts.iter().find(|c| c.id == *cid),
        };
        let colour = cut.map_or(Color32::WHITE, cut_colour);
        match g {
            Grab::Point(..) => {
                p.circle_filled(*at, if hot { 5.5 } else { 4.0 }, Color32::WHITE);
                p.circle_stroke(*at, if hot { 5.5 } else { 4.0 }, Stroke::new(1.5, colour));
            }
            Grab::Whole(..) => {
                let r = if hot { 5.0 } else { 3.5 };
                let pts = vec![*at + Vec2::new(0.0, -r), *at + Vec2::new(r, 0.0), *at + Vec2::new(0.0, r), *at + Vec2::new(-r, 0.0)];
                p.add(egui::Shape::convex_polygon(pts, colour, Stroke::new(1.0, Color32::WHITE)));
            }
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::Crosshair);
        if hot {
            on_handle = true;
        }
        if resp.clicked() && alt {
            remove(app, id, g);
            return true;
        }
        if resp.drag_started() {
            if let (Some(c), Some(pos)) = (cut, ui.input(|i| i.pointer.interact_pos())) {
                if let Some(start) = map.layer(from_screen(pos)) {
                    drag = Some(Drag { grab: g.clone(), from: c.points.clone(), start });
                }
            }
        }
    }

    // A drag on the layer draws a cut.
    if !on_handle && drag.is_none() {
        if body.drag_started() {
            // From where the button went down, not where the drag was noticed.
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()).or_else(|| body.interact_pointer_pos()) {
                draw = Some(vec![pos]);
            }
        }
        if body.dragged() {
            if let (Some(pts), Some(pos)) = (draw.as_mut(), body.interact_pointer_pos()) {
                if pts.last().is_none_or(|l| l.distance(pos) >= STEP) {
                    pts.push(pos);
                }
            }
        }
        if body.drag_stopped() {
            if let Some(mut pts) = draw.take() {
                if let Some(pos) = body.interact_pointer_pos() {
                    if pts.last().is_none_or(|l| l.distance(pos) > 1.0) {
                        pts.push(pos);
                    }
                }
                if shift && pts.len() > 2 {
                    pts = vec![pts[0], pts[pts.len() - 1]];
                }
                add(app, id, map, &pts.iter().map(|q| from_screen(*q)).collect::<Vec<_>>(), new_fill(&ctx));
            }
        }
    }
    if let Some(pts) = &draw {
        if pts.len() > 1 {
            p.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, t.select)));
        }
    }

    // Moving a point or a whole cut.
    if let (Some(d), Some(pos)) = (drag.clone(), ui.input(|i| i.pointer.interact_pos())) {
        if let Some(at) = map.layer(from_screen(pos)) {
            if let Some(mut c) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() {
                let (cid, label) = match d.grab {
                    Grab::Point(cid, i) => {
                        if let Some(cut) = c.cuts.iter_mut().find(|k| k.id == cid) {
                            if let Some(q) = cut.points.get_mut(i) {
                                *q = at;
                            }
                        }
                        (cid, "Move cut point")
                    }
                    Grab::Whole(cid, _) => {
                        let (dx, dy) = (at.x - d.start.x, at.y - d.start.y);
                        if let Some(cut) = c.cuts.iter_mut().find(|k| k.id == cid) {
                            cut.points = d.from.iter().map(|q| m::Vec2::new(q.x + dx, q.y + dy)).collect();
                        }
                        (cid, "Move cut")
                    }
                };
                let seq = app.doc.seq();
                app.doc.apply_gesture(&format!("canvas-cut-{id}-{cid}"), label, m::Edit::SetClip { seq, clip: Box::new(c) });
            }
        }
    }
    let released = ui.input(|i| i.pointer.any_released());
    ui.data_mut(|m| {
        match (&draw, released) {
            (Some(v), false) => {
                m.insert_temp(draw_id(), v.clone());
            }
            _ => {
                m.remove::<Vec<Pos2>>(draw_id());
            }
        }
        match (&drag, released) {
            (Some(v), false) => {
                m.insert_temp(drag_id(), v.clone());
            }
            _ => {
                m.remove::<Drag>(drag_id());
            }
        }
    });
    took
}

/// Adds a cut along screen-space `pts` (canvas pixels from the centre) of
/// layer `id`, thinned to the points that matter.
fn add(app: &mut StudioApp, id: u32, map: &LayerMap, pts: &[Vec2], fill: usize) {
    let layer: Vec<[f64; 2]> = pts.iter().filter_map(|c| map.layer(*c)).map(|q| [q.x, q.y]).collect();
    // Within a thousandth of the layer is as good as exact.
    let thin = studio_rig::scissor::simplify(&layer, 0.0015);
    if thin.len() < 2 {
        return;
    }
    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let mut cut = m::Cut::new(m::Cut::next_id(&mc.cuts), thin.iter().map(|q| m::Vec2::new(q[0], q[1])).collect());
    if !cut.is_active() {
        return;
    }
    cut.fill = fill_of(fill);
    if fill == 1 {
        cut.name = "mouth".into();
    }
    mc.cuts.push(cut);
    let seq = app.doc.seq();
    app.doc.apply("Add cut", m::Edit::SetClip { seq, clip: Box::new(mc) });
}

/// Removes a point (the cut with its last two) or a whole cut.
fn remove(app: &mut StudioApp, id: u32, g: &Grab) {
    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let label = match g {
        Grab::Point(cid, i) => {
            if let Some(c) = mc.cuts.iter_mut().find(|c| c.id == *cid) {
                if c.points.len() > 2 {
                    c.points.remove(*i);
                } else {
                    mc.cuts.retain(|c| c.id != *cid);
                }
            }
            "Remove cut point"
        }
        Grab::Whole(cid, _) => {
            mc.cuts.retain(|c| c.id != *cid);
            "Remove cut"
        }
    };
    let seq = app.doc.seq();
    app.doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(mc) });
}

// ---------------------------------------------------------------- mesh

/// Most edges drawn; a finer mesh than this is thinned out of view.
const MAX_EDGES: usize = 30_000;

/// The mesh of layer `id` as it is bent at `shown` (in layer coordinates,
/// as `(from, to)` segments), kept while nothing it depends on changes.
#[cfg(feature = "render")]
fn mesh_edges(app: &StudioApp, p: &egui::Painter, id: u32, map: &LayerMap, mc: &m::Clip, shown: Time) -> std::sync::Arc<Vec<([f32; 2], [f32; 2])>> {
    use std::sync::Arc;
    type Cached = Arc<(studio_render::puppet::WarpDesc, (u32, u32), Arc<Vec<([f32; 2], [f32; 2])>>)>;
    let mut warp = studio_render::model_scene::warp_desc(mc, shown);
    // The flat mesh: the corner pin is applied by the canvas mapping.
    warp.corner_pin = None;
    let px = map.pixels();
    // About 192 cells across is plenty to look at.
    let k = (192.0 / px.x.max(px.y).max(1.0)).min(1.0);
    let (w, h) = (((px.x * k).round() as u32).max(2), ((px.y * k).round() as u32).max(2));
    let key = egui::Id::new(("canvas-scissor-mesh-cache", id));
    if let Some(c) = p.ctx().data(|d| d.get_temp::<Cached>(key)) {
        if c.0 == warp && c.1 == (w, h) {
            return c.2.clone();
        }
    }
    let alpha = layer_alpha(app, mc, w, h);
    let frame = studio_render::Frame::from_fn(w, h, |x, y| [0.0, 0.0, 0.0, alpha(x, y)]);
    let occ = studio_render::puppet::occupancy_cpu(&frame, studio_render::puppet::OccupancyGrid::for_size(w, h));
    let mut edges: Vec<([f32; 2], [f32; 2])> = Vec::new();
    if let Some(mesh) = studio_render::puppet::shown_mesh(&warp, w, h, Some(&occ)) {
        let mut seen = std::collections::HashSet::new();
        let to_layer = |x: f32, y: f32| [(x + mesh.origin[0]) / w as f32, (y + mesh.origin[1]) / h as f32];
        for t in mesh.tris.iter().filter(|t| t[13] == 0.0) {
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (pa, pb) = (to_layer(t[2 * a], t[2 * a + 1]), to_layer(t[2 * b], t[2 * b + 1]));
                let q = |v: f32| (v * 4096.0).round() as i32;
                let (ka, kb) = ([q(pa[0]), q(pa[1])], [q(pb[0]), q(pb[1])]);
                let key = if ka <= kb { (ka, kb) } else { (kb, ka) };
                if seen.insert(key) {
                    edges.push((pa, pb));
                }
            }
        }
    }
    let out = Arc::new(edges);
    p.ctx().data_mut(|d| d.insert_temp::<Cached>(key, Arc::new((warp, (w, h), out.clone()))));
    out
}

/// The layer's alpha at `(x, y)` of a `w × h` grid over what it shows
/// (after crop): its picture's transparency when it has a still picture,
/// else everywhere.
#[cfg(feature = "render")]
fn layer_alpha(app: &StudioApp, mc: &m::Clip, w: u32, h: u32) -> Box<dyn Fn(u32, u32) -> f32> {
    use crate::services::character as ch;
    let Ok(path) = ch::picture_path(&app.doc, mc) else { return Box::new(|_, _| 1.0) };
    let Ok((pw, ph, px)) = ch::picture(&path) else { return Box::new(|_, _| 1.0) };
    let c = &mc.transform.crop;
    let (x0, y0) = (c.left.clamp(0.0, 1.0), c.top.clamp(0.0, 1.0));
    let (sx, sy) = ((1.0 - c.left - c.right).max(1e-3), (1.0 - c.top - c.bottom).max(1e-3));
    Box::new(move |x, y| {
        let u = x0 + (x as f64 + 0.5) / w as f64 * sx;
        let v = y0 + (y as f64 + 0.5) / h as f64 * sy;
        let (ix, iy) = (((u * pw as f64) as usize).min(pw - 1), ((v * ph as f64) as usize).min(ph - 1));
        px[(iy * pw + ix) * 4 + 3] as f32 / 255.0
    })
}

#[cfg(feature = "render")]
fn draw_mesh(app: &StudioApp, p: &egui::Painter, id: u32, map: &LayerMap, mc: &m::Clip, shown: Time, screen: &dyn Fn(Vec2) -> Pos2) {
    let edges = mesh_edges(app, p, id, map, mc, shown);
    let stride = edges.len().div_ceil(MAX_EDGES).max(1);
    let stroke = Stroke::new(0.6, Color32::from_rgba_unmultiplied(120, 200, 255, 130));
    for (a, b) in edges.iter().step_by(stride) {
        let pa = screen(map.canvas(m::Vec2::new(a[0] as f64, a[1] as f64)));
        let pb = screen(map.canvas(m::Vec2::new(b[0] as f64, b[1] as f64)));
        p.line_segment([pa, pb], stroke);
    }
}

#[cfg(not(feature = "render"))]
fn draw_mesh(_: &StudioApp, _: &egui::Painter, _: u32, _: &LayerMap, _: &m::Clip, _: Time, _: &dyn Fn(Vec2) -> Pos2) {}

/// Whether this build can draw the mesh.
pub const CAN_SHOW_MESH: bool = cfg!(feature = "render");

// ------------------------------------------------------------ Inspector

/// The scissor rows of the Pins section: what the tool does, show mesh, the
/// fill for new cuts, finding cuts, and each cut with its fill and a
/// remove button.
pub fn rows(app: &mut StudioApp, ui: &mut egui::Ui, id: u32, mc: &m::Clip, tool_on: bool) {
    use crate::icons::Icon;
    use crate::kit::{Button, Segment};
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let w = ui.available_width();
    if tool_on {
        ui.label(egui::RichText::new(crate::i18n::shown("Drag across the layer to cut it (shift: straight); drag a dot to move a point, a diamond to move the cut; ⌥-click a dot or diamond removes it. Cut between legs, along lips, under an arm: the sides then move apart.")).font(theme::caption()).color(t.text_2));
        if CAN_SHOW_MESH {
            let mut on = show_mesh(&ctx);
            crate::kit::labeled_row(ui, "Show mesh", |ui| {
                if crate::kit::toggle(ui, &mut on, "Show mesh", None).changed() {
                    set_show_mesh(&ctx, on);
                }
            });
        }
        let mut k = new_fill(&ctx);
        if crate::kit::segmented(ui, "cut-fill", &mut k, &[Segment::text("No fill"), Segment::text("Mouth"), Segment::text("Auto")], Some(w), None).changed() {
            set_new_fill(&ctx, k);
        }
        ui.label(egui::RichText::new(crate::i18n::shown("A fill shows in the gap where the sides move apart: the dark inside of a mouth, or the picture's own colour darkened.")).font(theme::caption()).color(t.text_2));
        ui.horizontal(|ui| {
            if Button::secondary("Find gaps").icon(Icon::MagicWand).show(ui).clicked() {
                run(app, &ctx, serde_json::json!({"op": "scissor", "clip": id, "auto": if mc.skeleton.is_some() { vec!["gaps", "parts"] } else { vec!["gaps"] }}), "Find gaps");
            }
            if Button::secondary("Find mouth").icon(Icon::MagicWand).show(ui).clicked() {
                run(app, &ctx, serde_json::json!({"op": "scissor", "clip": id, "auto": "mouth"}), "Find mouth");
            }
        });
    }
    for c in &mc.cuts {
        let name = if c.name.is_empty() { crate::i18n::tf(format!("Cut {}", c.id)) } else { c.name.clone() };
        crate::kit::name_row(ui, egui::RichText::new(name).font(theme::body_medium()).color(t.text), |ui| {
            if Button::ghost("Remove").icon(Icon::Trash).show(ui).clicked() {
                let mut next = mc.clone();
                next.cuts.retain(|o| o.id != c.id);
                let seq = app.doc.seq();
                app.doc.apply("Remove cut", m::Edit::SetClip { seq, clip: Box::new(next) });
            }
        });
        let mut enabled = c.enabled;
        crate::kit::labeled_row(ui, "Cut on", |ui| {
            if crate::kit::toggle(ui, &mut enabled, "Cut on", None).changed() {
                let mut next = mc.clone();
                if let Some(o) = next.cuts.iter_mut().find(|o| o.id == c.id) {
                    o.enabled = enabled;
                }
                let seq = app.doc.seq();
                app.doc.apply("Cut", m::Edit::SetClip { seq, clip: Box::new(next) });
            }
        });
        let mut k = match c.fill {
            m::CutFill::None => 0,
            m::CutFill::Colour(_) => 1,
            m::CutFill::Auto => 2,
        };
        if crate::kit::segmented(ui, &format!("cut-fill-of-{id}-{}", c.id), &mut k, &[Segment::text("No fill"), Segment::text("Colour"), Segment::text("Auto")], Some(w), None).changed() {
            let mut next = mc.clone();
            if let Some(o) = next.cuts.iter_mut().find(|o| o.id == c.id) {
                o.fill = match (k, c.fill) {
                    (1, m::CutFill::Colour(old)) => m::CutFill::Colour(old),
                    (1, _) => fill_of(1),
                    (2, _) => m::CutFill::Auto,
                    _ => m::CutFill::None,
                };
            }
            let seq = app.doc.seq();
            app.doc.apply("Cut fill", m::Edit::SetClip { seq, clip: Box::new(next) });
        }
        if let m::CutFill::Colour(old) = c.fill {
            let mut rgb = [old.r as f32, old.g as f32, old.b as f32];
            crate::kit::labeled_row(ui, "Fill colour", |ui| {
                if ui.color_edit_button_rgb(&mut rgb).changed() {
                    let mut next = mc.clone();
                    if let Some(o) = next.cuts.iter_mut().find(|o| o.id == c.id) {
                        o.fill = m::CutFill::Colour(m::Rgba::new(rgb[0] as f64, rgb[1] as f64, rgb[2] as f64, old.a));
                    }
                    let seq = app.doc.seq();
                    app.doc.apply_gesture(&format!("cut-colour-{id}-{}", c.id), "Cut fill colour", m::Edit::SetClip { seq, clip: Box::new(next) });
                }
            });
        }
    }
}

/// Cuts the usual places of a character just rigged as `kind` (legs, arms,
/// mouth); a kind with none cuts nothing.
pub fn after_build(app: &mut StudioApp, ctx: &egui::Context, id: u32, kind: &str) {
    let rigged = matches!(crate::services::rigging::kind_of(kind), Some("biped" | "quadruped"));
    if rigged {
        run(app, ctx, serde_json::json!({"op": "scissor", "clip": id, "auto": true, "kind": kind}), "Scissor");
    }
}

/// Runs an agent op on the open document (one undo step).
fn run(app: &mut StudioApp, ctx: &egui::Context, op: serde_json::Value, label: &str) {
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let r = s.edit(&[op], label, false);
    app.doc = s.doc;
    if let Err(e) = r {
        crate::kit::push(ctx, format!("{label}: {e}"), None, Some(Icon::Warning));
    }
}

use crate::icons::Icon;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::pins::{PinTool, set_tool, tool};

    fn card_doc() -> (crate::doc::Doc, u32) {
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Cuts", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2, "width": 320, "height": 180})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s.doc, id)
    }

    /// The real scissor tool, headless: a drag across the layer draws a
    /// cut, a drag on its dot moves a point, alt-click on a dot takes it
    /// away, and the mesh option draws without trouble.
    #[test]
    fn the_scissor_tool_draws_moves_and_removes_cuts() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let (doc, id) = card_doc();
        app.doc = doc;
        app.editor.selection = vec![id];
        app.editor.playhead = 0;
        set_tool(&ctx, PinTool::Scissor);
        set_show_mesh(&ctx, true);
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        let mut t = 0.0;
        let mut frame = |app: &mut StudioApp, events: Vec<egui::Event>, mods: egui::Modifiers| {
            t += 1.0 / 60.0;
            let mut all = vec![egui::Event::ModifiersChanged(mods)];
            all.extend(events);
            let input = egui::RawInput { screen_rect: Some(canvas), time: Some(t), events: all, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| {
                pins::interact(app, ui, canvas, canvas, 1.0);
            });
        };
        let none = egui::Modifiers::NONE;
        let press = |p: Pos2, down: bool, m: egui::Modifiers| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: m };
        // A drag from the top of the card down through its middle.
        let from = Pos2::new(320.0, 100.0);
        frame(&mut app, vec![egui::Event::PointerMoved(from)], none);
        frame(&mut app, vec![press(from, true, none)], none);
        for k in 1..=10 {
            frame(&mut app, vec![egui::Event::PointerMoved(from + Vec2::new(0.0, 16.0 * k as f32))], none);
        }
        frame(&mut app, vec![press(from + Vec2::new(0.0, 160.0), false, none)], none);
        frame(&mut app, vec![], none);
        let cut = |app: &StudioApp| app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().cuts.clone();
        let cuts = cut(&app);
        assert_eq!(cuts.len(), 1, "{cuts:?}");
        // A straight drag thins to its two ends; the card's picture is the
        // canvas's size: canvas x = 320 is its middle.
        assert_eq!(cuts[0].points.len(), 2, "{:?}", cuts[0].points);
        assert!((cuts[0].points[0].x - 0.5).abs() < 0.02 && (cuts[0].points[0].y - 100.0 / 360.0).abs() < 0.02, "{:?}", cuts[0].points);
        assert!((cuts[0].points[1].y - 260.0 / 360.0).abs() < 0.02);
        // Drag its top dot sideways.
        let dot = from;
        frame(&mut app, vec![egui::Event::PointerMoved(dot)], none);
        frame(&mut app, vec![press(dot, true, none)], none);
        for k in 1..=5 {
            frame(&mut app, vec![egui::Event::PointerMoved(dot + Vec2::new(20.0 * k as f32, 0.0))], none);
        }
        frame(&mut app, vec![press(dot + Vec2::new(100.0, 0.0), false, none)], none);
        frame(&mut app, vec![], none);
        let cuts = cut(&app);
        assert_eq!(cuts.len(), 1, "a drag on a dot moves it, it draws no new cut: {cuts:?}");
        assert!((cuts[0].points[0].x - (320.0 + 100.0) / 640.0).abs() < 0.03, "{:?}", cuts[0].points);
        // Alt-click on the dot: the cut had two points, so it goes.
        let at = dot + Vec2::new(100.0, 0.0);
        let alt = egui::Modifiers { alt: true, ..Default::default() };
        frame(&mut app, vec![egui::Event::PointerMoved(at)], alt);
        frame(&mut app, vec![press(at, true, alt)], alt);
        frame(&mut app, vec![press(at, false, alt)], alt);
        frame(&mut app, vec![], alt);
        assert!(cut(&app).is_empty());
        frame(&mut app, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: none }], none);
        assert_eq!(tool(&ctx), PinTool::Off);
    }

    /// The whole editor with the scissor tool on and a cut selected: the
    /// canvas (with the mesh) and the Inspector's rows draw.
    #[test]
    fn the_editor_draws_cuts_and_the_mesh() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let (doc, id) = card_doc();
        let mut s = crate::agent::Session::new(doc);
        s.edit(
            &[
                serde_json::json!({"op": "puppet", "clip": id, "pins": [[-100, 0], [100, 0]], "pin": 2, "to": [100, -40]}),
                serde_json::json!({"op": "scissor", "clip": id, "cuts": [{"name": "split", "path": [[0, -90], [0, 90]], "fill": "auto"}]}),
            ],
            "cuts",
            false,
        )
        .unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![id];
        set_show_mesh(&ctx, true);
        set_tool(&ctx, PinTool::Scissor);
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..4 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(i as f64 / 60.0), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        }
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap();
        assert_eq!(mc.cuts.len(), 1);
        assert_eq!(mc.cuts[0].fill, m::CutFill::Auto);
    }
}
