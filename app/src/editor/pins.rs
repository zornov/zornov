// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Puppet pins, pinned masks and corner pins on the canvas.
//!
//! Pins live in a layer's own 0..1 coordinates (as masks do: across the
//! picture after crop), so they ride along when the layer moves. This
//! module maps them to canvas pixels and back through the layer's
//! placement (`xform::Geo`) and its corner pin, for the canvas tools here
//! and for the AI's `puppet`, `mask_puppet` and `corner_pin` ops.
//!
//! The canvas tool ([`interact`]) shows the selected layer's pins (yellow
//! dots; a ring for bend, a square for starch, a diamond for overlap) and
//! its corner-pin handles, and the path points and pins of a mask chosen
//! in the Inspector. With the pin tool on, a click on the layer places a
//! pin, a drag moves one (a key at the playhead when it is animated),
//! alt-click removes one; corner handles drag the corners (shift for
//! skew: the opposite side moves along).

use crate::app::StudioApp;
use crate::editor::xform::{self, Geo};
use crate::theme;
use crate::kit::FieldRow;
use super::physics::cloud_feedback as feedback;
use crate::view::{Project, SEC, Time};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;

/// A layer's picture on the canvas at one moment: 0..1 layer coordinates
/// to canvas pixels from the centre, with or without its corner pin.
#[derive(Clone, Debug)]
pub struct LayerMap {
    pub geo: Geo,
    /// Crop fractions: left, top, right, bottom.
    crop: [f32; 4],
    corner: Option<m::CornerPin>,
}

fn mv(p: Vec2) -> m::Vec2 {
    m::Vec2::new(p.x as f64, p.y as f64)
}

impl LayerMap {
    /// Layer `id` at timeline time `t`.
    pub fn at(proj: &Project, model: &m::Project, id: u32, t: Time) -> Option<LayerMap> {
        let geo = xform::geo(proj, model, id, t)?;
        let vc = proj.clip(id)?;
        let mc = model.sequences.iter().find_map(|s| s.clip(m::ClipId(id as u64)))?;
        let local = (t - vc.start).clamp(0, vc.dur.max(0));
        let c = mc.transform_at(local).crop;
        let corner = mc.corner_pin_at(local).filter(|c| c.enabled);
        Some(LayerMap { geo, crop: [c.left as f32, c.top as f32, c.right as f32, c.bottom as f32].map(|v| v.clamp(0.0, 1.0)), corner })
    }

    fn to_source(&self, p: m::Vec2) -> Vec2 {
        let s = self.geo.source();
        let [l, t, r, b] = self.crop;
        Vec2::new((l + p.x as f32 * (1.0 - l - r)) * s.x, (t + p.y as f32 * (1.0 - t - b)) * s.y)
    }

    fn from_source(&self, q: Vec2) -> m::Vec2 {
        let s = self.geo.source();
        let [l, t, r, b] = self.crop;
        let (w, h) = ((1.0 - l - r).max(1e-6), (1.0 - t - b).max(1e-6));
        mv(Vec2::new((q.x / s.x.max(1e-6) - l) / w, (q.y / s.y.max(1e-6) - t) / h))
    }

    /// Flat picture (no corner pin) → canvas pixels from the centre.
    pub fn flat_canvas(&self, p: m::Vec2) -> Vec2 {
        self.geo.to_canvas(self.to_source(p))
    }

    /// Canvas pixels → the flat picture's coordinates.
    pub fn flat_layer(&self, c: Vec2) -> Option<m::Vec2> {
        self.geo.to_source(c).map(|q| self.from_source(q))
    }

    /// Where a point of the layer (as the puppet sees it) shows.
    pub fn canvas(&self, p: m::Vec2) -> Vec2 {
        self.flat_canvas(self.corner.as_ref().map_or(p, |c| c.map(p)))
    }

    /// The point of the layer showing at canvas `c`.
    pub fn layer(&self, c: Vec2) -> Option<m::Vec2> {
        let q = self.flat_layer(c)?;
        match &self.corner {
            Some(cp) => cp.unmap(q),
            None => Some(q),
        }
    }

    /// A point of the layer's drawn box (`u`, `v` from -1 to 1), in layer
    /// coordinates.
    pub fn content(&self, u: f32, v: f32) -> m::Vec2 {
        self.from_source(self.geo.content_point(u, v))
    }

    /// A point given in the drawing's own pixels (its top-left corner is
    /// 0, 0; before the crop), in layer coordinates.
    pub fn drawing(&self, x: f32, y: f32) -> m::Vec2 {
        self.from_source(Vec2::new(x, y))
    }

    /// The drawing's own size in pixels (before the crop).
    pub fn drawing_size(&self) -> Vec2 {
        self.geo.source()
    }

    /// The layer's picture size in source pixels (after crop): what the
    /// renderer bends in.
    pub fn pixels(&self) -> Vec2 {
        let s = self.geo.source();
        let [l, t, r, b] = self.crop;
        Vec2::new(s.x * (1.0 - l - r), s.y * (1.0 - t - b))
    }
}

/// What the pin tool is working on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PinTool {
    /// No pin tool: pins are shown, the transform handles work.
    #[default]
    Off,
    /// Place and move the layer's puppet pins.
    Puppet,
    /// Drag the corner pin's corners.
    Corners,
    /// Place pins on, and move the points of, mask `n` (0-based).
    Mask(usize),
    /// Draw, move and delete the cuts that split the puppet's mesh
    /// (`editor::scissor`).
    Scissor,
}

/// Which pin kind a click places.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PinChoice(pub m::PinKind);

fn tool_id() -> egui::Id {
    egui::Id::unique("canvas-pin-tool")
}

/// The pin tool in use.
pub fn tool(ctx: &egui::Context) -> PinTool {
    ctx.data(|d| d.get_temp(tool_id())).unwrap_or_default()
}

pub fn set_tool(ctx: &egui::Context, t: PinTool) {
    ctx.data_mut(|d| d.insert_temp(tool_id(), t));
}

/// The kind of pin a click places.
pub fn kind(ctx: &egui::Context) -> m::PinKind {
    ctx.data(|d| d.get_temp::<PinChoice>(tool_id().with("kind"))).map_or(m::PinKind::Position, |k| k.0)
}

pub fn set_kind(ctx: &egui::Context, k: m::PinKind) {
    ctx.data_mut(|d| d.insert_temp(tool_id().with("kind"), PinChoice(k)));
}

/// Writes `value` to `path` at `local`: a key when the property is
/// animated, the value otherwise (as every canvas drag does).
pub fn put(mc: &mut m::Clip, path: &str, value: m::Value, local: Time, frame: Time) {
    xform::put(mc, path, value, local, frame);
}

/// A colour per pin kind.
fn pin_colour(k: m::PinKind) -> Color32 {
    match k {
        m::PinKind::Position => Color32::from_rgb(250, 204, 21),
        m::PinKind::Bend => Color32::from_rgb(56, 189, 248),
        m::PinKind::Starch => Color32::from_rgb(244, 114, 182),
        m::PinKind::Overlap => Color32::from_rgb(163, 230, 53),
    }
}

/// Draws one pin.
pub(super) fn draw_pin(p: &egui::Painter, at: Pos2, kind: m::PinKind, hot: bool, rotation: f32) {
    let c = pin_colour(kind);
    let r = if hot { 6.5 } else { 5.0 };
    let edge = Stroke::new(1.5, Color32::from_black_alpha(200));
    match kind {
        m::PinKind::Position => {
            p.circle_filled(at, r, c);
            p.circle_stroke(at, r, edge);
        }
        m::PinKind::Bend => {
            p.circle_stroke(at, r + 1.0, Stroke::new(2.5, c));
            // A tick shows how far it turns.
            let a = (rotation - 90.0).to_radians();
            p.line_segment([at, at + Vec2::new(a.cos(), a.sin()) * (r + 5.0)], Stroke::new(2.0, c));
        }
        m::PinKind::Starch => {
            p.rect(Rect::from_center_size(at, Vec2::splat(r * 2.0)), 1, c, edge, egui::StrokeKind::Inside);
        }
        m::PinKind::Overlap => {
            let pts = vec![at + Vec2::new(0.0, -r - 1.0), at + Vec2::new(r + 1.0, 0.0), at + Vec2::new(0.0, r + 1.0), at + Vec2::new(-r - 1.0, 0.0)];
            p.add(egui::Shape::convex_polygon(pts, c, edge));
        }
    }
}

/// A grabbed handle of the pin tool.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Pin(u32),
    Corner(usize),
    MaskPin(u32),
    MaskPoint(usize),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    id: u32,
    grab: Grab,
    local: Time,
}

/// The pins and pin handles of the selected layer, and the gestures on
/// them. Returns `true` when the pin tool took the pointer (the transform
/// handles then stay out of the way).
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, area: Rect, scale: f32) -> bool {
    let mut tool = tool(ui.ctx());
    // Escape (or nothing selected) puts the pin tool away.
    if tool != PinTool::Off && (ui.input(|i| i.key_pressed(egui::Key::Escape)) || app.editor.selection.is_empty()) {
        set_tool(ui.ctx(), PinTool::Off);
        tool = PinTool::Off;
    }
    let Some(id) = app.editor.selection.first().copied() else { return false };
    let now = app.editor.hover_time.unwrap_or(app.editor.playhead);
    let Some(map) = LayerMap::at(&app.doc.project, &app.doc.model, id, now) else { return false };
    let Some(vc) = app.doc.project.clip(id).cloned() else { return false };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return false };
    if app.doc.project.lane(vc.lane).is_some_and(|l| l.locked) {
        return false;
    }
    let local = (app.editor.playhead - vc.start).clamp(0, vc.dur.max(0));
    let shown = (now - vc.start).clamp(0, vc.dur.max(0));
    if tool == PinTool::Scissor {
        return super::scissor::interact(app, ui, canvas, area, scale, id, &map, &mc, shown);
    }
    let frame = (SEC as f64 / (app.doc.project.fps as f64).max(1.0)) as Time;
    let t = theme::tokens(ui.ctx());
    let screen = |c: Vec2| canvas.center() + c * scale;
    let from_screen = |p: Pos2| (p - canvas.center()) / scale;
    let p = ui.painter().with_clip_rect(area.expand(8.0));
    let drag_id = egui::Id::unique("canvas-pin-drag");
    let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
    let mut took = false;

    // What shows: the puppet's pins always (when there are any), the
    // corners and a mask's points with their tools.
    let puppet = mc.puppet_at(shown);
    let mut handles: Vec<(Grab, Pos2)> = Vec::new();
    if let Some(pp) = &puppet {
        for pin in &pp.pins {
            handles.push((Grab::Pin(pin.id), screen(map.canvas(pin.position))));
        }
    }
    if tool == PinTool::Corners {
        if let Some(cp) = mc.corner_pin_at(shown) {
            let q: Vec<Pos2> = cp.corners().iter().map(|c| screen(map.flat_canvas(*c))).collect();
            for k in 0..4 {
                p.line_segment([q[k], q[(k + 1) % 4]], Stroke::new(1.0, t.select));
            }
            for (k, at) in q.into_iter().enumerate() {
                handles.push((Grab::Corner(k), at));
            }
        }
    }
    let mask = match tool {
        PinTool::Mask(n) => mc.masks.get(n).map(|mk| mc.mask_at(mk, shown)),
        _ => None,
    };
    if let Some(mk) = &mask {
        // The outline as it is drawn (bent by its pins).
        let pts: Vec<m::PathPoint> = mk.outline().unwrap_or_default();
        let mut poly: Vec<[f32; 2]> = flatten(&pts);
        bend(mk, &mut poly, &map);
        let line: Vec<Pos2> = poly.iter().map(|q| screen(map.canvas(m::Vec2::new(q[0] as f64, q[1] as f64)))).collect();
        if line.len() > 1 {
            let mut closed = line.clone();
            closed.push(line[0]);
            p.add(egui::Shape::line(closed, Stroke::new(1.5, Color32::from_rgb(250, 204, 21))));
        }
        if mk.shape == m::MaskShape::Path {
            for (i, pt) in mk.path.iter().enumerate() {
                handles.push((Grab::MaskPoint(i), screen(map.canvas(pt.point))));
            }
        }
        for pin in &mk.pins {
            handles.push((Grab::MaskPin(pin.id), screen(map.canvas(pin.position))));
        }
    }
    if handles.is_empty() && tool == PinTool::Off {
        return false;
    }

    // The layer under the handles (made first, so the handles made after
    // it get the pointer where they are).
    let body = (matches!(tool, PinTool::Puppet | PinTool::Mask(_)) && drag.is_none()).then(|| ui.interact(area, egui::Id::unique("canvas-pin-body"), Sense::click()));
    let mut on_handle = false;

    // The handles: hover, drag, alt-click to remove.
    let alt = ui.input(|i| i.modifiers.alt);
    for (g, at) in &handles {
        let resp = ui.interact(Rect::from_center_size(*at, Vec2::splat(16.0)), egui::Id::unique(("canvas-pin", id, format!("{g:?}"))), if tool == PinTool::Off { Sense::hover() } else { Sense::click_and_drag() });
        let hot = resp.hovered() || resp.dragged();
        match g {
            Grab::Pin(pid) => {
                if let Some(pin) = puppet.as_ref().and_then(|pp| pp.pin(*pid)) {
                    draw_pin(&p, *at, pin.kind, hot, pin.rotation as f32);
                    if hot && !pin.name.is_empty() {
                        resp.clone().on_hover_text(&pin.name);
                    }
                }
            }
            Grab::MaskPin(pid) => {
                if let Some(pin) = mask.as_ref().and_then(|mk| mk.pins.iter().find(|q| q.id == *pid)) {
                    draw_pin(&p, *at, pin.kind, hot, pin.rotation as f32);
                }
            }
            Grab::Corner(_) => {
                let r = Rect::from_center_size(*at, Vec2::splat(if hot { 10.0 } else { 8.0 }));
                p.rect(r, 1, Color32::WHITE, Stroke::new(1.0, t.select), egui::StrokeKind::Inside);
            }
            Grab::MaskPoint(_) => {
                let r = Rect::from_center_size(*at, Vec2::splat(if hot { 8.0 } else { 6.0 }));
                p.rect(r, 0, Color32::WHITE, Stroke::new(1.0, Color32::from_rgb(250, 204, 21)), egui::StrokeKind::Inside);
            }
        }
        if tool == PinTool::Off {
            continue;
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::Crosshair);
        if hot {
            took = true;
            on_handle = true;
        }
        if resp.clicked() && alt {
            remove(app, id, *g);
            return true;
        }
        if resp.drag_started() {
            drag = Some(Drag { id, grab: *g, local });
        }
    }

    // Click on the layer with the puppet or mask tool: a new pin there.
    if let Some(body) = body {
        took = true;
        if body.clicked() && !alt && !on_handle {
            if let Some(pos) = body.interact_pointer_pos() {
                if let Some(at) = map.layer(from_screen(pos)) {
                    add_pin(app, ui.ctx(), id, tool, at, kind(ui.ctx()));
                }
            }
        }
    }

    if let (Some(d), Some(pos)) = (drag, ui.input(|i| i.pointer.interact_pos())) {
        took = true;
        let c = from_screen(pos);
        if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(d.id as u64)).cloned() {
            let shift = ui.input(|i| i.modifiers.shift);
            let label = match d.grab {
                Grab::Pin(pid) => {
                    if let Some(at) = map.layer(c) {
                        put(&mut mc, &format!("puppet.{pid}.position"), m::Value::Point(at), d.local, frame);
                    }
                    "Move pin"
                }
                Grab::MaskPin(pid) => {
                    if let (PinTool::Mask(n), Some(at)) = (tool, map.layer(c)) {
                        if let Some(mid) = mc.masks.get(n).map(|k| k.id.0) {
                            put(&mut mc, &format!("mask.{mid}.pin.{pid}.position"), m::Value::Point(at), d.local, frame);
                        }
                    }
                    "Move mask pin"
                }
                Grab::MaskPoint(i) => {
                    if let (PinTool::Mask(n), Some(at)) = (tool, map.layer(c)) {
                        if let Some(mid) = mc.masks.get(n).map(|k| k.id.0) {
                            put(&mut mc, &format!("mask.{mid}.point.{i}"), m::Value::Point(at), d.local, frame);
                        }
                    }
                    "Move mask point"
                }
                Grab::Corner(k) => {
                    if let Some(at) = map.flat_layer(c) {
                        let names = m::CornerPin::CORNERS;
                        let cur = mc.corner_pin_at(d.local).map(|cp| cp.corners()).unwrap_or_default();
                        let delta = m::Vec2::new(at.x - cur[k].x, at.y - cur[k].y);
                        put(&mut mc, &format!("corner_pin.{}", names[k]), m::Value::Point(at), d.local, frame);
                        // Shift: skew, the neighbour on the same side
                        // moves along.
                        if shift {
                            let n = if k % 2 == 0 { (k + 1) % 4 } else { (k + 3) % 4 };
                            let moved = m::Vec2::new(cur[n].x + delta.x, cur[n].y + delta.y);
                            put(&mut mc, &format!("corner_pin.{}", names[n]), m::Value::Point(moved), d.local, frame);
                        }
                    }
                    "Corner pin"
                }
            };
            let seq = app.doc.seq();
            app.doc.apply_gesture(&format!("canvas-pin-{}-{:?}", d.id, d.grab), label, m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
    }
    if ui.input(|i| i.pointer.any_released()) {
        ui.data_mut(|m| m.remove::<Drag>(drag_id));
    } else if let Some(d) = drag {
        ui.data_mut(|m| m.insert_temp(drag_id, d));
    }
    took
}

/// A mask outline flattened to a polyline (0..1 layer coordinates).
fn flatten(path: &[m::PathPoint]) -> Vec<[f32; 2]> {
    let n = path.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b) = (path[i], path[(i + 1) % n]);
        let p = |v: m::Vec2| [v.x as f32, v.y as f32];
        let (p0, p3) = (p(a.point), p(b.point));
        let p1 = [p0[0] + a.handle_out.x as f32, p0[1] + a.handle_out.y as f32];
        let p2 = [p3[0] + b.handle_in.x as f32, p3[1] + b.handle_in.y as f32];
        for s in 0..12 {
            let t = s as f32 / 12.0;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            out.push([w[0] * p0[0] + w[1] * p1[0] + w[2] * p2[0] + w[3] * p3[0], w[0] * p0[1] + w[1] * p1[1] + w[2] * p2[1] + w[3] * p3[1]]);
        }
    }
    out
}

/// Bends a mask outline by the mask's pins, as the renderer does.
#[cfg(feature = "render")]
fn bend(mk: &m::Mask, poly: &mut [[f32; 2]], map: &LayerMap) {
    if mk.shape != m::MaskShape::Path || mk.pins.is_empty() {
        return;
    }
    // In layer pixels, as the renderer bends it.
    let s = map.pixels();
    let (w, h) = (s.x.max(1.0), s.y.max(1.0));
    let pins: Vec<studio_render::puppet::PinDesc> = mk
        .pins
        .iter()
        .map(|p| studio_render::puppet::PinDesc {
            kind: match p.kind {
                m::PinKind::Position => studio_render::puppet::PinKind::Position,
                m::PinKind::Bend => studio_render::puppet::PinKind::Bend,
                m::PinKind::Starch => studio_render::puppet::PinKind::Starch,
                m::PinKind::Overlap => studio_render::puppet::PinKind::Overlap,
            },
            rest: [p.rest.x as f32, p.rest.y as f32],
            at: [p.position.x as f32, p.position.y as f32],
            rotation: p.rotation as f32,
            scale: p.scale as f32,
            amount: p.amount as f32,
            extent: p.extent as f32,
        })
        .collect();
    for q in poly.iter_mut() {
        *q = [q[0] * w, q[1] * h];
    }
    studio_render::puppet::bend_points(&pins, poly, w, h);
    for q in poly.iter_mut() {
        *q = [q[0] / w, q[1] / h];
    }
}

#[cfg(not(feature = "render"))]
fn bend(_: &m::Mask, _: &mut [[f32; 2]], _: &LayerMap) {}

/// Adds a pin of `kind` at `at` (layer coordinates) with the tool's
/// target: the puppet, or a mask (turned into a path first).
fn add_pin(app: &mut StudioApp, ctx: &egui::Context, id: u32, tool: PinTool, at: m::Vec2, kind: m::PinKind) {
    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let label = match tool {
        PinTool::Mask(n) => {
            let Some(mk) = mc.masks.get(n).cloned() else { return };
            let Some(mut path_mask) = as_path(&mut mc, &mk) else {
                feedback::hint(ctx, "A line or mirror mask has no outline to pin");
                return;
            };
            let pid = path_mask.pins.iter().map(|p| p.id).max().map_or(1, |x| x + 1);
            path_mask.pins.push(m::Pin::new(pid, kind, at));
            mc.masks[n] = path_mask;
            "Mask pin"
        }
        _ => {
            mc.puppet.get_or_insert_with(m::Puppet::default).add(kind, at);
            "Puppet pin"
        }
    };
    let seq = app.doc.seq();
    app.doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(mc) });
}

/// `mk` as a path mask (its shape traced, its rotation baked in and its
/// shape keys dropped); `None` for a line or mirror, which has no outline.
pub fn as_path(mc: &mut m::Clip, mk: &m::Mask) -> Option<m::Mask> {
    if mk.shape == m::MaskShape::Path {
        return Some(mk.clone());
    }
    let mut out = mk.clone();
    out.path = mk.outline()?;
    out.shape = m::MaskShape::Path;
    out.params.remove("rotation");
    let prefix = format!("mask.{}.", mk.id.0);
    for k in ["rotation", "size"] {
        mc.keyframes.remove(&format!("{prefix}{k}"));
    }
    Some(out)
}

/// Removes a pin, a corner pin (a corner handle) or a mask point.
fn remove(app: &mut StudioApp, id: u32, g: Grab) {
    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let label = match g {
        Grab::Pin(pid) => {
            remove_pin(&mut mc, pid);
            "Remove pin"
        }
        Grab::Corner(_) => {
            mc.corner_pin = None;
            mc.keyframes.retain(|k, _| !k.starts_with("corner_pin."));
            "Remove corner pin"
        }
        Grab::MaskPin(pid) => {
            for mk in &mut mc.masks {
                if let Some(i) = mk.pins.iter().position(|p| p.id == pid) {
                    mk.pins.remove(i);
                    let prefix = format!("mask.{}.pin.{pid}.", mk.id.0);
                    mc.keyframes.retain(|k, _| !k.starts_with(&prefix));
                    break;
                }
            }
            "Remove mask pin"
        }
        Grab::MaskPoint(_) => return,
    };
    let seq = app.doc.seq();
    app.doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(mc) });
}

/// Removes puppet pin `pid` and its keys (and the puppet with its last pin).
pub fn remove_pin(mc: &mut m::Clip, pid: u32) {
    if let Some(pp) = &mut mc.puppet {
        pp.pins.retain(|p| p.id != pid);
        if pp.pins.is_empty() {
            mc.puppet = None;
        }
    }
    let prefix = format!("puppet.{pid}.");
    mc.keyframes.retain(|k, _| !k.starts_with(&prefix));
}

/// A corner pin over what layer `id` draws (its corners on the box).
pub fn new_corner_pin(map: &LayerMap) -> m::CornerPin {
    m::CornerPin::new([map.content(-1.0, -1.0), map.content(1.0, 1.0)])
}

// ------------------------------------------------------------ Inspector

/// A field row bound to a model property of clip `id` (one axis of a
/// point when `axis` is set), shown times `k`: the value at the playhead,
/// a key there when it is animated, and a keyframe diamond.
#[allow(clippy::too_many_arguments)]
pub(crate) fn model_row(app: &mut StudioApp, ui: &mut egui::Ui, id: u32, path: &str, axis: Option<usize>, label: &str, range: std::ops::RangeInclusive<f32>, default: f32, unit: &str, k: f32) {
    let Some(vc) = app.doc.project.clip(id).cloned() else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let local = (app.editor.playhead - vc.start).clamp(0, vc.dur.max(0));
    let frame = (SEC as f64 / (app.doc.project.fps as f64).max(1.0)) as Time;
    let Some(cur) = mc.property_at(path, local) else { return };
    let read = |v: &m::Value| -> f32 {
        match (v, axis) {
            (m::Value::Point(p), Some(0)) => p.x as f32,
            (m::Value::Point(p), Some(_)) => p.y as f32,
            (m::Value::Scalar(x) | m::Value::Angle(x), _) => *x as f32,
            _ => 0.0,
        }
    };
    let write = |v: &m::Value, x: f32| -> m::Value {
        match (v, axis) {
            (m::Value::Point(p), Some(0)) => m::Value::Point(m::Vec2::new(x as f64, p.y)),
            (m::Value::Point(p), Some(_)) => m::Value::Point(m::Vec2::new(p.x, x as f64)),
            (m::Value::Angle(_), _) => m::Value::Angle(x as f64),
            _ => m::Value::Scalar(x as f64),
        }
    };
    let mut v = read(&cur) * k;
    let key_here = mc.keyframes.get(path).and_then(|tr| tr.keys.iter().find(|kf| (kf.time - local).abs() <= frame / 2).map(|kf| kf.time));
    let mut keyed = key_here.is_some();
    let out = FieldRow::new(label, &mut v, range.clone()).default(default * k).unit(unit).decimals(if k >= 100.0 { 1 } else { 2 }).scrub(((range.end() - range.start()) / 400.0).max(0.001)).keyframe(&mut keyed).show(ui);
    let seq = app.doc.seq();
    if out.changed {
        let mut c = mc.clone();
        put(&mut c, path, write(&cur, v / k), local, frame);
        app.doc.apply_gesture(&format!("insp-pin-{id}-{path}-{axis:?}"), label, m::Edit::SetClip { seq, clip: Box::new(c) });
    }
    if out.keyframe_toggled {
        let mut c = mc.clone();
        match key_here {
            Some(t) => {
                // The last key gone: the property keeps the value it had.
                if let Some(tr) = c.keyframes.get_mut(path) {
                    tr.remove_key(t);
                    if tr.keys.is_empty() {
                        c.keyframes.remove(path);
                        c.set_property(path, cur.clone());
                    }
                }
            }
            None => {
                let key = m::Keyframe::new(local, cur.clone(), m::Interp::Ease);
                match c.keyframes.get_mut(path) {
                    Some(tr) => tr.set_key(key),
                    None => {
                        c.keyframes.insert(path.to_owned(), m::Track::new(vec![key]));
                    }
                }
            }
        }
        app.doc.apply(if key_here.is_some() { "Remove keyframe" } else { "Add keyframe" }, m::Edit::SetClip { seq, clip: Box::new(c) });
    }
}

/// The notes under the pin tool.
const PUPPET_HOW: &str = "Click the layer to add a pin. Drag to move, ⌥-click to remove";
const CORNERS_HOW: &str = "Drag a corner. Shift-drag to skew";

/// A pin's row title: its name, or its place in the list.
fn pin_title(what: &str, n: usize, pin: &m::Pin) -> String {
    if pin.name.is_empty() { crate::i18n::tf(format!("{what} {} · {}", n + 1, kind_label(pin.kind))) } else { format!("{} · {}", feedback::shown_name(&pin.name), kind_label(pin.kind)) }
}

fn kind_label(k: m::PinKind) -> &'static str {
    match k {
        m::PinKind::Position => "Position",
        m::PinKind::Bend => "Bend",
        m::PinKind::Starch => "Starch",
        m::PinKind::Overlap => "Overlap",
    }
}

const KINDS: [m::PinKind; 4] = [m::PinKind::Position, m::PinKind::Bend, m::PinKind::Starch, m::PinKind::Overlap];

/// The rows of one pin (under `prefix`, e.g. `puppet.3.`).
fn pin_rows(app: &mut StudioApp, ui: &mut egui::Ui, id: u32, prefix: &str, pin: &m::Pin) {
    match pin.kind {
        m::PinKind::Position => {
            model_row(app, ui, id, &format!("{prefix}position"), Some(0), "X", -50.0..=150.0, pin.rest.x as f32, "%", 100.0);
            model_row(app, ui, id, &format!("{prefix}position"), Some(1), "Y", -50.0..=150.0, pin.rest.y as f32, "%", 100.0);
        }
        m::PinKind::Bend => {
            model_row(app, ui, id, &format!("{prefix}rotation"), None, "Rotation", -360.0..=360.0, 0.0, "°", 1.0);
            model_row(app, ui, id, &format!("{prefix}scale"), None, "Scale", 10.0..=400.0, 1.0, "%", 100.0);
        }
        m::PinKind::Starch | m::PinKind::Overlap => {
            let range = if pin.kind == m::PinKind::Starch { 0.0..=100.0 } else { -100.0..=100.0 };
            model_row(app, ui, id, &format!("{prefix}amount"), None, if pin.kind == m::PinKind::Starch { "Amount" } else { "In front" }, range, 0.5, "%", 100.0);
            model_row(app, ui, id, &format!("{prefix}extent"), None, "Extent", 1.0..=100.0, 0.15, "%", 100.0);
        }
    }
}

/// The Inspector's "Pins" section for clip `id`: the pin tool, the
/// puppet's pins with their rows, the corner pin and the pins and points
/// of the first mask.
pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    use crate::icons::Icon;
    use crate::kit::{Button, Section, Segment};
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let ctx = ui.ctx().clone();
    let has = mc.puppet.is_some() || mc.corner_pin.is_some() || !mc.cuts.is_empty() || mc.masks.iter().any(|k| !k.pins.is_empty());
    let out = Section::new("pins", "Pins").icon(Icon::Path).resettable(has).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        // The canvas tool.
        let tool = tool(&ctx);
        let mut order = vec![PinTool::Off, PinTool::Puppet, PinTool::Corners, PinTool::Scissor];
        if !mc.masks.is_empty() {
            order.push(PinTool::Mask(0));
        }
        let mut cur = order.iter().position(|o| std::mem::discriminant(o) == std::mem::discriminant(&tool)).unwrap_or(0);
        let w = ui.available_width();
        let opts: Vec<Segment> = order
            .iter()
            .map(|o| {
                Segment::text(match o {
                    PinTool::Off => "Off",
                    PinTool::Puppet => "Puppet",
                    PinTool::Corners => "Corners",
                    PinTool::Scissor => "Scissor",
                    PinTool::Mask(_) => "Mask",
                })
            })
            .collect();
        if crate::kit::segmented(ui, "pin-tool", &mut cur, &opts, Some(w), None).changed() {
            let next = order[cur];
            // The corners tool starts a corner pin over what the layer draws.
            if next == PinTool::Corners && mc.corner_pin.is_none() {
                if let Some(map) = LayerMap::at(&app.doc.project, &app.doc.model, id, app.editor.playhead) {
                    let mut c = mc.clone();
                    c.corner_pin = Some(new_corner_pin(&map));
                    let seq = app.doc.seq();
                    app.doc.apply("Corner pin", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
            }
            set_tool(&ctx, next);
        }
        if matches!(tool, PinTool::Puppet | PinTool::Mask(_)) {
            let mut k = KINDS.iter().position(|x| *x == kind(&ctx)).unwrap_or(0);
            let segs: Vec<Segment> = KINDS.iter().map(|x| Segment::text(kind_label(*x))).collect();
            if crate::kit::segmented(ui, "pin-kind", &mut k, &segs, Some(w), None).changed() {
                set_kind(&ctx, KINDS[k]);
            }
            ui.label(egui::RichText::new(crate::i18n::shown(PUPPET_HOW)).font(theme::caption()).color(t.text_2));
        } else if tool == PinTool::Corners {
            ui.label(egui::RichText::new(crate::i18n::shown(CORNERS_HOW)).font(theme::caption()).color(t.text_2));
        }

        // The puppet's pins.
        if let Some(pp) = &mc.puppet {
            let mut enabled = pp.enabled;
            crate::kit::labeled_row(ui, "Puppet", |ui| {
                if crate::kit::toggle(ui, &mut enabled, "Puppet on", None).changed() {
                    let mut c = mc.clone();
                    if let Some(p) = &mut c.puppet {
                        p.enabled = enabled;
                    }
                    let seq = app.doc.seq();
                    app.doc.apply("Puppet", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
            });
            let mut density = pp.density as f32;
            if FieldRow::new("Mesh detail", &mut density, 4.0..=64.0).default(24.0).decimals(0).step(1.0).show(ui).changed {
                let mut c = mc.clone();
                if let Some(p) = &mut c.puppet {
                    p.density = density as f64;
                }
                let seq = app.doc.seq();
                app.doc.apply_gesture(&format!("pin-density-{id}"), "Puppet mesh", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
            for (n, pin) in pp.pins.iter().enumerate() {
                let name = pin_title("Pin", n, pin);
                crate::kit::name_row(ui, egui::RichText::new(name).font(theme::body_medium()).color(t.text), |ui| {
                    if Button::ghost("Remove").icon(Icon::Trash).show(ui).clicked() {
                        let mut c = mc.clone();
                        remove_pin(&mut c, pin.id);
                        let seq = app.doc.seq();
                        app.doc.apply("Remove pin", m::Edit::SetClip { seq, clip: Box::new(c) });
                    }
                });
                pin_rows(app, ui, id, &format!("puppet.{}.", pin.id), pin);
            }
        }

        // The cuts that split the mesh.
        if tool == PinTool::Scissor || !mc.cuts.is_empty() {
            super::scissor::rows(app, ui, id, &mc, tool == PinTool::Scissor);
        }

        // The corner pin.
        if let Some(cp) = &mc.corner_pin {
            crate::kit::name_row(ui, egui::RichText::new(crate::i18n::shown("Corner pin")).font(theme::body_medium()).color(t.text), |ui| {
                if Button::ghost("Remove").icon(Icon::Trash).show(ui).clicked() {
                    let mut c = mc.clone();
                    c.corner_pin = None;
                    c.keyframes.retain(|k, _| !k.starts_with("corner_pin."));
                    let seq = app.doc.seq();
                    app.doc.apply("Remove corner pin", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
            });
            let rest = m::CornerPin::new(cp.rest).corners();
            for (i, (name, label)) in m::CornerPin::CORNERS.iter().zip(["Top left", "Top right", "Bottom right", "Bottom left"]).enumerate() {
                model_row(app, ui, id, &format!("corner_pin.{name}"), Some(0), &format!("{label} X"), -100.0..=200.0, rest[i].x as f32, "%", 100.0);
                model_row(app, ui, id, &format!("corner_pin.{name}"), Some(1), &format!("{label} Y"), -100.0..=200.0, rest[i].y as f32, "%", 100.0);
            }
        }

        // Pins and points of the first mask (the one the Mask section
        // shapes).
        if let Some(mk) = mc.masks.first() {
            if !mk.pins.is_empty() || mk.shape == m::MaskShape::Path {
                ui.label(egui::RichText::new(crate::i18n::shown("Mask")).font(theme::body_medium()).color(t.text));
            }
            for (n, pin) in mk.pins.iter().enumerate() {
                crate::kit::name_row(ui, egui::RichText::new(pin_title("Mask pin", n, pin)).font(theme::body()).color(t.text_2), |ui| {
                    if Button::ghost("Remove").icon(Icon::Trash).show(ui).clicked() {
                        remove(app, id, Grab::MaskPin(pin.id));
                    }
                });
                pin_rows(app, ui, id, &format!("mask.{}.pin.{}.", mk.id.0, pin.id), pin);
            }
            if mk.shape == m::MaskShape::Path && mk.path.len() <= 12 {
                for (i, pt) in mk.path.iter().enumerate() {
                    model_row(app, ui, id, &format!("mask.{}.point.{i}", mk.id.0), Some(0), &crate::i18n::tf(format!("Point {} X", i + 1)), -50.0..=150.0, pt.point.x as f32, "%", 100.0);
                    model_row(app, ui, id, &format!("mask.{}.point.{i}", mk.id.0), Some(1), &crate::i18n::tf(format!("Point {} Y", i + 1)), -50.0..=150.0, pt.point.y as f32, "%", 100.0);
                }
            }
        }
    });
    if out.reset {
        let mut c = mc.clone();
        c.puppet = None;
        c.corner_pin = None;
        c.cuts.clear();
        for mk in &mut c.masks {
            mk.pins.clear();
        }
        c.keyframes.retain(|k, _| !(k.starts_with("puppet.") || k.starts_with("corner_pin.") || (k.starts_with("mask.") && k.contains(".pin."))));
        let seq = app.doc.seq();
        app.doc.apply("Remove pins", m::Edit::SetClip { seq, clip: Box::new(c) });
        set_tool(&ctx, PinTool::Off);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traced_masks_become_paths_without_their_shape_keys() {
        let mut mc = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, SEC, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        let mut mk = m::Mask::new(m::MaskId(5), m::MaskShape::Rectangle);
        mk.params.insert("rotation".into(), m::Value::Scalar(90.0));
        mc.keyframes.insert("mask.5.rotation".into(), m::Track::new(vec![m::Keyframe::new(0, m::Value::Scalar(0.0), m::Interp::Linear)]));
        let path = as_path(&mut mc, &mk).unwrap();
        assert_eq!(path.shape, m::MaskShape::Path);
        assert_eq!(path.path.len(), 4);
        assert!(!path.params.contains_key("rotation"));
        assert!(mc.keyframes.is_empty());
        let line = m::Mask::new(m::MaskId(6), m::MaskShape::Line);
        assert!(as_path(&mut mc, &line).is_none());
    }

    /// A project with a 320 × 180 card in the middle of a 640 × 360
    /// canvas, and the card's id.
    fn card_doc() -> (crate::doc::Doc, u32) {
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Pins", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2, "width": 320, "height": 180})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s.doc, id)
    }

    /// The real pin tool, headless (no window): a click on the layer
    /// places a pin where it was clicked, a drag moves it, alt-click
    /// removes it, Escape puts the tool away.
    #[test]
    fn the_pin_tool_places_drags_and_removes_pins() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let (doc, id) = card_doc();
        app.doc = doc;
        app.editor.selection = vec![id];
        app.editor.playhead = 0;
        set_tool(&ctx, PinTool::Puppet);
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        let mut t = 0.0;
        let mut frame = |app: &mut StudioApp, events: Vec<egui::Event>, mods: egui::Modifiers| {
            t += 1.0 / 60.0;
            let mut all = vec![egui::Event::ModifiersChanged(mods)];
            all.extend(events);
            let input = egui::RawInput { screen_rect: Some(canvas), time: Some(t), events: all, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| {
                interact(app, ui, canvas, canvas, 1.0);
            });
        };
        let none = egui::Modifiers::NONE;
        let press = |p: Pos2, down: bool, m: egui::Modifiers| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: m };
        let click = |app: &mut StudioApp, frame: &mut dyn FnMut(&mut StudioApp, Vec<egui::Event>, egui::Modifiers), p: Pos2, m: egui::Modifiers| {
            frame(app, vec![egui::Event::PointerMoved(p)], m);
            frame(app, vec![press(p, true, m)], m);
            frame(app, vec![press(p, false, m)], m);
            frame(app, vec![], m);
        };
        // Two clicks: two pins. A card's picture is the canvas's size, so
        // canvas x = 100 is (320 + 100) / 640 across it.
        click(&mut app, &mut frame, Pos2::new(420.0, 180.0), none);
        click(&mut app, &mut frame, Pos2::new(220.0, 180.0), none);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        let pins = &mc.puppet.as_ref().expect("pins placed").pins;
        assert_eq!(pins.len(), 2, "{pins:?}");
        assert!((pins[0].rest.x - 0.65625).abs() < 0.01 && (pins[0].rest.y - 0.5).abs() < 0.01, "{:?}", pins[0].rest);
        // Drag the first one 60 px up.
        let from = Pos2::new(420.0, 180.0);
        frame(&mut app, vec![egui::Event::PointerMoved(from)], none);
        frame(&mut app, vec![press(from, true, none)], none);
        for k in 1..=6 {
            frame(&mut app, vec![egui::Event::PointerMoved(from - Vec2::new(0.0, 10.0 * k as f32))], none);
        }
        frame(&mut app, vec![press(from - Vec2::new(0.0, 60.0), false, none)], none);
        frame(&mut app, vec![], none);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        let p = mc.puppet.as_ref().unwrap().pins[0].position;
        assert!((p.y - (0.5 - 60.0 / 360.0)).abs() < 0.02, "dragged to {p:?}");
        assert!(mc.puppet.as_ref().unwrap().bends());
        // Alt-click the other one: gone.
        let alt = egui::Modifiers { alt: true, ..Default::default() };
        click(&mut app, &mut frame, Pos2::new(220.0, 180.0), alt);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        assert_eq!(mc.puppet.as_ref().unwrap().pins.len(), 1);
        // Escape: the tool is put away.
        frame(&mut app, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: none }], none);
        assert_eq!(tool(&ctx), PinTool::Off);
    }

    /// The whole editor, headless, with a pinned, corner-pinned layer
    /// selected and each pin tool on: the Inspector's Pins section and
    /// the canvas draw without trouble.
    #[test]
    fn the_editor_draws_pins_and_their_inspector_rows() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let (doc, id) = card_doc();
        let mut s = crate::agent::Session::new(doc);
        s.edit(
            &[
                serde_json::json!({"op": "puppet", "clip": id, "space": "canvas", "pins": [[-100, 0], [100, 0], {"at": [0, 0], "kind": "bend", "rotate": 20}], "pin": 2, "keys": [[0, [100, 0]], [1, [100, -60]]]}),
                serde_json::json!({"op": "corner_pin", "clip": id, "skew": 10}),
                serde_json::json!({"op": "mask", "clip": id, "shape": "ellipse"}),
                serde_json::json!({"op": "mask_puppet", "clip": id, "pins": [[-40, 0], [40, 0]], "pin": 1, "to": [-80, 10]}),
            ],
            "pins",
            false,
        )
        .unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![id];
        let mut frame = eframe::Frame::_new_kittest();
        for (k, tool_on) in [PinTool::Off, PinTool::Puppet, PinTool::Corners, PinTool::Mask(0)].into_iter().enumerate() {
            set_tool(&ctx, tool_on);
            for i in 0..3 {
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some((k * 3 + i) as f64 / 60.0), ..Default::default() };
                crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            }
        }
        // Nothing drawn changed the project.
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap();
        assert_eq!(mc.puppet.as_ref().unwrap().pins.len(), 3);
        assert!(mc.corner_pin.is_some() && mc.masks[0].pins.len() == 2);
        // The section alone at the Inspector's width, with each tool.
        for (k, tool_on) in [PinTool::Off, PinTool::Puppet, PinTool::Corners, PinTool::Mask(0)].into_iter().enumerate() {
            set_tool(&ctx, tool_on);
            let drawn = crate::editor::physics::cloud_ui_check::draw(&ctx, 3, |ui| inspector(&mut app, ui, id));
            drawn.assert_tidy(&format!("pins-{k}"), &[]);
            assert!(drawn.shows("Pin 1 · Position") && drawn.shows("Mesh detail"), "{:?}", drawn.texts);
            match tool_on {
                PinTool::Puppet | PinTool::Mask(_) => assert!(drawn.shows(PUPPET_HOW)),
                PinTool::Corners => assert!(drawn.shows(CORNERS_HOW)),
                _ => {}
            }
        }
        set_tool(&ctx, PinTool::Off);
    }
}
