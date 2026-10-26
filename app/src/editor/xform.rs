// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! On-canvas transform: every layer's real hit box, and handles that move,
//! scale, rotate and re-anchor it, straight on the project model.
//!
//! The geometry is the renderer's (`studio-render`'s placement): a layer's
//! source is fitted into the canvas, its `anchor` (source pixels from the
//! source centre) lands on `position` (canvas pixels from the canvas
//! centre), and scale and rotation turn about that anchor. A title's or a
//! shape's hit box is what it draws, not the whole frame it is drawn on.
//!
//! Dragging a property that has keyframes keys it at the playhead (as
//! every motion editor does); otherwise it sets the value. The anchor can
//! be dragged without the picture jumping, and an animated position shows
//! its motion path with a handle on every key.
//!
//! A 3D layer's box is its plane seen through the camera
//! (`studio_model::space`): picking, outlines and the handles follow the
//! perspective, a move slides it parallel to the screen, and the 3D gizmo
//! (`three_d`) moves and turns it in depth.

use crate::app::StudioApp;
use crate::kit;
use crate::theme;
use crate::view::{ClipKind, Project, SEC, Time};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;

/// A layer's placement at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    /// Where the anchor lands, canvas pixels from the canvas centre.
    pub pos: Vec2,
    pub sx: f32,
    pub sy: f32,
    /// Clockwise degrees.
    pub rot: f32,
    /// Source pixels from the source centre.
    pub anchor: Vec2,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Depth (3D layers).
    pub z: f32,
}

/// A 3D layer's plane and the camera it is seen through.
#[derive(Clone, Copy, Debug)]
pub struct Space3 {
    pub plane: m::space::Plane,
    pub view: m::space::View,
    /// Where its anchor is in the world.
    pub anchor: [f64; 3],
}

/// Where a layer is: its placement, its source size, and the part of the
/// source that shows (what it draws, after crop).
#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub id: u32,
    pub xf: Xf,
    source: Vec2,
    fit: f32,
    /// Visible content in source pixels: min and max corners.
    content: [Vec2; 2],
    /// Set for a 3D layer: its source maps to the screen through this.
    pub space: Option<Space3>,
}

impl Geo {
    /// Where the anchor shows, canvas pixels from the centre.
    pub fn anchor_canvas(&self) -> Vec2 {
        match &self.space {
            Some(s) => s.view.project(s.anchor).map_or(self.xf.pos, |(c, _)| Vec2::new(c.x as f32, c.y as f32)),
            None => self.xf.pos,
        }
    }

    fn axes(&self) -> Vec2 {
        Vec2::new(self.fit * self.xf.sx * if self.xf.flip_x { -1.0 } else { 1.0 }, self.fit * self.xf.sy * if self.xf.flip_y { -1.0 } else { 1.0 })
    }

    /// Source pixels → canvas pixels from the canvas centre.
    pub fn to_canvas(&self, p: Vec2) -> Vec2 {
        if let Some(s) = &self.space {
            // Behind the lens: far off in its direction, so outlines still
            // run the right way.
            let w = s.plane.at(p.x as f64, p.y as f64);
            let v = s.view.view(w);
            let d = v[2].max(1e-3);
            return Vec2::new((s.view.zoom * v[0] / d) as f32, (s.view.zoom * v[1] / d) as f32);
        }
        let a = self.axes();
        let d = p - (self.source / 2.0 + self.xf.anchor);
        self.xf.pos + egui::emath::Rot2::from_angle(self.xf.rot.to_radians()) * Vec2::new(d.x * a.x, d.y * a.y)
    }

    /// Canvas pixels from the centre → source pixels.
    pub fn to_source(&self, c: Vec2) -> Option<Vec2> {
        if let Some(s) = &self.space {
            let (u, v, t) = s.plane.hit(s.view.position, s.view.ray(m::Vec2::new(c.x as f64, c.y as f64)))?;
            return (t > 0.0).then_some(Vec2::new(u as f32, v as f32));
        }
        let a = self.axes();
        if a.x.abs() < 1e-6 || a.y.abs() < 1e-6 {
            return None;
        }
        let d = egui::emath::Rot2::from_angle(-self.xf.rot.to_radians()) * (c - self.xf.pos);
        Some(Vec2::new(d.x / a.x, d.y / a.y) + self.source / 2.0 + self.xf.anchor)
    }

    /// The source's size in source pixels.
    pub fn source(&self) -> Vec2 {
        self.source
    }

    /// Canvas pixels per source pixel at a scale of 100 %.
    pub fn fit(&self) -> f32 {
        self.fit
    }

    /// A point of the content box: `u`, `v` from -1 (left / top) to 1.
    pub fn content_point(&self, u: f32, v: f32) -> Vec2 {
        let [a, b] = self.content;
        Vec2::new(a.x + (b.x - a.x) * (u + 1.0) / 2.0, a.y + (b.y - a.y) * (v + 1.0) / 2.0)
    }

    /// The content box's corners in canvas pixels from the centre.
    pub fn quad(&self) -> [Vec2; 4] {
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(u, v)| self.to_canvas(self.content_point(u, v)))
    }

    pub fn contains(&self, c: Vec2) -> bool {
        let Some(s) = self.to_source(c) else { return false };
        let [a, b] = self.content;
        // A few source pixels of slack so thin layers stay grabbable.
        let pad = 2.0 / self.fit.max(1e-3);
        s.x >= a.x - pad && s.x <= b.x + pad && s.y >= a.y - pad && s.y <= b.y + pad
    }
}

fn num(v: Option<m::Value>, default: f32) -> f32 {
    match v {
        Some(m::Value::Scalar(x) | m::Value::Angle(x)) => x as f32,
        _ => default,
    }
}

fn point(v: Option<m::Value>) -> Vec2 {
    match v {
        Some(m::Value::Point(p)) => Vec2::new(p.x as f32, p.y as f32),
        _ => Vec2::ZERO,
    }
}

/// The placement of `mc` at `local` (time from the clip's start).
pub fn xf_at(mc: &m::Clip, local: Time) -> Xf {
    Xf {
        pos: point(mc.property_at("transform.position", local)),
        sx: num(mc.property_at("transform.scale_x", local), 1.0),
        sy: num(mc.property_at("transform.scale_y", local), 1.0),
        rot: num(mc.property_at("transform.rotation", local), 0.0),
        anchor: point(mc.property_at("transform.anchor", local)),
        flip_x: mc.transform.flip_x,
        flip_y: mc.transform.flip_y,
        z: num(mc.property_at("transform.position_z", local), 0.0),
    }
}

/// Where layer `id` is at time `t`, as the renderer draws it.
pub fn geo(proj: &Project, model: &m::Project, id: u32, t: Time) -> Option<Geo> {
    let vc = proj.clip(id)?;
    if matches!(vc.kind, ClipKind::Audio | ClipKind::Fx) {
        return None;
    }
    let mc = model.active().clip(m::ClipId(id as u64))?;
    let driven = model.active().driven(mc, t);
    let mc: &m::Clip = &driven;
    let local = (t - vc.start).clamp(0, vc.dur.max(0));
    let canvas = Vec2::new(proj.width.max(1) as f32, proj.height.max(1) as f32);
    let (source, content) = match &mc.source {
        m::ClipSource::Media(r) => {
            let mi = proj.media(r.media.0 as u32)?;
            let s = Vec2::new(mi.width.max(1) as f32, mi.height.max(1) as f32);
            (s, [Vec2::ZERO, s])
        }
        m::ClipSource::Sequence(_) => (canvas, [Vec2::ZERO, canvas]),
        m::ClipSource::Gap => return None,
        m::ClipSource::Generator(g) => match g.kind {
            m::GeneratorKind::Adjustment | m::GeneratorKind::Camera | m::GeneratorKind::Light | m::GeneratorKind::Model => return None,
            m::GeneratorKind::Solid | m::GeneratorKind::Gradient => (canvas, [Vec2::ZERO, canvas]),
            m::GeneratorKind::Shape => {
                // Drawn at the source centre, `size` a fraction of it.
                let k = match g.params.get("size") {
                    Some(m::Value::Point(p)) => Vec2::new(p.x as f32, p.y as f32),
                    _ => Vec2::splat(0.5 * canvas.x.min(canvas.y) / canvas.x.max(1.0)) * Vec2::new(1.0, canvas.x / canvas.y),
                };
                let half = canvas * k / 2.0;
                (canvas, [canvas / 2.0 - half, canvas / 2.0 + half])
            }
            m::GeneratorKind::Text => (canvas, text_box(g, canvas)?),
        },
    };
    // Crop takes fractions off the source edges.
    let crop = |side: &str| num(mc.property_at(&format!("transform.crop_{side}"), local), 0.0).clamp(0.0, 1.0);
    let [mut a, mut b] = content;
    a.x = a.x.max(crop("left") * source.x);
    a.y = a.y.max(crop("top") * source.y);
    b.x = b.x.min(source.x * (1.0 - crop("right")));
    b.y = b.y.min(source.y * (1.0 - crop("bottom")));
    if b.x <= a.x || b.y <= a.y {
        return None;
    }
    let fit = (canvas.x / source.x).min(canvas.y / source.y);
    // A clip that follows a parent is drawn where its parent carries it.
    let xf = if mc.parent.is_some() {
        let placed = model.active().placed_transform(mc, vc.start + local);
        let mut still = mc.clone();
        still.keyframes.retain(|k, _| !k.starts_with("transform."));
        still.transform = placed;
        xf_at(&still, local)
    } else {
        xf_at(mc, local)
    };
    let space = mc.transform.three_d.then(|| {
        let seq = model.active();
        let tr = m::space::oriented_transform(seq, mc, vc.start + local, canvas.x as f64);
        let plane = m::space::Plane::of(&tr, m::Vec2::new(source.x as f64, source.y as f64), fit as f64);
        Space3 { plane, view: m::space::view_at(seq, vc.start + local, canvas.x as f64), anchor: [tr.position.x, tr.position.y, tr.position_z] }
    });
    Some(Geo { id, xf, source, fit, content: [a, b], space })
}

/// A title's drawn box in canvas pixels, laid out as the renderer lays it.
fn text_box(g: &m::Generator, canvas: Vec2) -> Option<[Vec2; 2]> {
    let text = g.plain_text();
    if text.trim().is_empty() {
        return None;
    }
    let s = |k: &str, d: f32| match g.params.get(k) {
        Some(m::Value::Scalar(x)) => *x as f32,
        _ => d,
    };
    let preset = match g.params.get("style") {
        Some(m::Value::Choice(s) | m::Value::Text(s)) => s.as_str(),
        _ => "",
    };
    let plate = !matches!(g.params.get("plate"), Some(m::Value::Bool(false)));
    let mut ts = crate::text_look::style_of(preset, s("size", -1.0), s("letter_spacing", 0.0), [1.0; 4], plate);
    let family = match g.params.get("font") {
        Some(m::Value::Text(f) | m::Value::Choice(f)) => f.as_str(),
        _ => "",
    };
    let italic = matches!(g.params.get("font_italic_set"), Some(m::Value::Bool(true))).then(|| matches!(g.params.get("font_italic"), Some(m::Value::Bool(true))));
    crate::text_look::set_font(&mut ts, family, s("font_weight", -1.0), italic);
    let l = crate::text_look::engine().layout(&studio_text::TextBlock::plain(text), &ts, &studio_text::style::Canvas::new(canvas.x as u32, canvas.y as u32));
    // The layout box is centred on the canvas; the plate (when the style
    // has one) is part of what shows.
    let (w, h) = (l.bounds.w.max(1.0), l.bounds.h.max(1.0));
    let ink = if ts.background.is_some() { l.bounds } else { l.ink };
    let o = canvas / 2.0 - Vec2::new(w, h) / 2.0;
    Some([o + Vec2::new(ink.x, ink.y), o + Vec2::new(ink.x + ink.w.max(1.0), ink.y + ink.h.max(1.0))])
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// A corner (both set) or a side (one zero) of the content box.
    Scale { u: f32, v: f32 },
    Rotate,
    Anchor,
    /// A key of the motion path, by its time from the clip start.
    PathKey(Time),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    id: u32,
    grab: Grab,
    start: Pos2,
    geo: Geo,
    /// Position at the playhead when the drag began (before snapping).
    local: Time,
}

/// Writes `value` to `path` of `mc` at `local`: a key when the property
/// is animated (replacing a key at that moment), the value otherwise.
pub fn put(mc: &mut m::Clip, path: &str, value: m::Value, local: Time, frame: Time) {
    match mc.keyframes.get_mut(path).filter(|t| !t.keys.is_empty()) {
        Some(track) => {
            let near = track.keys.iter().find(|k| (k.time - local).abs() <= frame / 2).map(|k| k.time);
            match near {
                Some(t) => {
                    if let Some(k) = track.keys.iter_mut().find(|k| k.time == t) {
                        k.value = value;
                    }
                }
                None => {
                    // A new key eases like the key before it.
                    let interp = track.keys.iter().rev().find(|k| k.time < local).or(track.keys.first()).map_or(m::Interp::Ease, |k| k.interp);
                    track.set_key(m::Keyframe::new(local, value, interp));
                }
            }
        }
        None => {
            mc.set_property(path, value);
        }
    }
}

fn pt(v: Vec2) -> m::Value {
    m::Value::Point(m::Vec2 { x: v.x as f64, y: v.y as f64 })
}

/// The handles, hit boxes, hover outline, motion path and the gestures on
/// them. `order` is every visible layer, topmost first.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, area: Rect, order: &[u32], scale: f32) {
    let t = theme::tokens(ui.ctx());
    let now = app.editor.hover_time.unwrap_or(app.editor.playhead);
    let geos: Vec<Geo> = order.iter().filter_map(|&id| geo(&app.doc.project, &app.doc.model, id, now)).collect();
    let screen = |c: Vec2| canvas.center() + c * scale;
    let from_screen = |p: Pos2| (p - canvas.center()) / scale;
    let locked = |proj: &Project, id: u32| proj.clip(id).and_then(|c| proj.lane(c.lane)).is_some_and(|l| l.locked);
    let frame = (SEC as f64 / (app.doc.project.fps as f64).max(1.0)) as Time;
    let drag_id = egui::Id::unique("canvas-xform-drag");
    let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
    let sel = app.editor.selection.first().copied();
    let sel_geo = sel.and_then(|id| geos.iter().find(|g| g.id == id)).copied().filter(|g| !locked(&app.doc.project, g.id));
    let p = ui.painter().with_clip_rect(area.expand(8.0));
    let stroke = Stroke::new(1.0, t.select);

    // The canvas first: widgets made later sit on top of it, so the
    // handles get the pointer where they are.
    let body = ui.interact(area, egui::Id::unique("canvas-xform-body"), Sense::click_and_drag());

    // Handles of the selected layer.
    let mut handle: Option<(Grab, egui::Response)> = None;
    if let Some(g) = sel_geo {
        let q = g.quad().map(screen);
        for k in 0..4 {
            p.line_segment([q[k], q[(k + 1) % 4]], stroke);
        }
        let mut hs: Vec<(Grab, Pos2)> = Vec::new();
        // The motion path of an animated position, a handle per key.
        if let (Some(vc), Some(mc)) = (app.doc.project.clip(g.id), app.doc.model.active().clip(m::ClipId(g.id as u64))) {
            if let Some(track) = mc.keyframes.get("transform.position").filter(|tr| tr.keys.len() > 1) {
                let n = 64;
                let at = |t: Time| match &g.space {
                    // A 3D layer's path is in space: seen through the camera.
                    Some(s3) => {
                        let p = point(mc.property_at("transform.position", t));
                        let z = num(mc.property_at("transform.position_z", t), 0.0);
                        s3.view.project([p.x as f64, p.y as f64, z as f64]).map_or(p, |(c, _)| Vec2::new(c.x as f32, c.y as f32))
                    }
                    None => point(mc.property_at("transform.position", t)),
                };
                let path: Vec<Pos2> = (0..=n).map(|i| screen(at(vc.dur * i / n))).collect();
                p.add(egui::Shape::dashed_line(&path, Stroke::new(1.0, theme::alpha(t.select, 0.7)), 4.0, 3.0));
                for k in &track.keys {
                    if matches!(k.value, m::Value::Point(_)) {
                        hs.push((Grab::PathKey(k.time), screen(at(k.time))));
                    }
                }
            }
        }
        let top = (q[0] + q[1].to_vec2()) / 2.0;
        let up = (top - (q[3] + q[2].to_vec2()) / 2.0).normalized();
        let up = if up.is_finite() { up } else { Vec2::new(0.0, -1.0) };
        let knob = top + up * 22.0;
        p.line_segment([top, knob], stroke);
        hs.push((Grab::Rotate, knob));
        for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)] {
            hs.push((Grab::Scale { u, v }, screen(g.to_canvas(g.content_point(u, v)))));
        }
        // The anchor last: egui gives the pointer to the widget made last,
        // so it wins where it sits on another handle (a path key, a corner).
        hs.push((Grab::Anchor, screen(g.anchor_canvas())));
        for (h, at) in hs.iter() {
            let size = match h {
                Grab::Anchor => 16.0,
                Grab::Scale { u, v } if *u == 0.0 || *v == 0.0 => 10.0,
                _ => 12.0,
            };
            let id = egui::Id::unique(("canvas-xform-handle", format!("{h:?}")));
            let resp = ui.interact(Rect::from_center_size(*at, Vec2::splat(size + 6.0)), id, Sense::click_and_drag());
            let hot = resp.hovered() || resp.dragged();
            let grow = hot as u8 as f32;
            match h {
                Grab::Rotate => {
                    p.circle_filled(*at, 5.0 + grow, Color32::WHITE);
                    p.circle_stroke(*at, 5.0 + grow, stroke);
                }
                Grab::Anchor => {
                    // A crosshair in a ring: where scale and rotation turn.
                    let r = 6.0 + grow;
                    p.circle_stroke(*at, r, Stroke::new(1.5, Color32::WHITE));
                    p.circle_stroke(*at, r + 1.5, Stroke::new(1.0, Color32::from_black_alpha(140)));
                    for d in [Vec2::X, Vec2::Y] {
                        p.line_segment([*at - d * (r + 4.0), *at + d * (r + 4.0)], Stroke::new(1.0, t.select));
                    }
                }
                Grab::PathKey(kt) => {
                    let current = app.doc.project.clip(g.id).is_some_and(|vc| (now - vc.start - kt).abs() <= frame / 2);
                    let r = Rect::from_center_size(*at, Vec2::splat(7.0 + grow * 2.0));
                    p.rect(r, 1, if current { t.select } else { Color32::WHITE }, Stroke::new(1.0, t.select), egui::StrokeKind::Inside);
                }
                Grab::Scale { .. } => {
                    let r = Rect::from_center_size(*at, Vec2::splat(if size > 10.0 { 8.0 } else { 6.0 } + grow * 2.0));
                    p.rect(r, 1, Color32::WHITE, stroke, egui::StrokeKind::Inside);
                }
                Grab::Move => {}
            }
            let cursor = match h {
                Grab::Rotate => egui::CursorIcon::Alias,
                Grab::Anchor | Grab::PathKey(_) => egui::CursorIcon::Crosshair,
                Grab::Scale { u, v } => {
                    // The cursor follows the handle's direction on screen.
                    let dir = (*at - screen(g.to_canvas(g.content_point(0.0, 0.0)))).angle().to_degrees().rem_euclid(180.0);
                    let _ = (u, v);
                    match dir {
                        d if !(22.5..157.5).contains(&d) => egui::CursorIcon::ResizeHorizontal,
                        d if (67.5..112.5).contains(&d) => egui::CursorIcon::ResizeVertical,
                        d if d < 90.0 => egui::CursorIcon::ResizeNwSe,
                        _ => egui::CursorIcon::ResizeNeSw,
                    }
                }
                Grab::Move => egui::CursorIcon::Grab,
            };
            let resp = resp.on_hover_cursor(cursor);
            // Double-click the anchor: back to the middle of the layer.
            if *h == Grab::Anchor && resp.double_clicked() {
                if let Some(o) = app.doc.model.active().clip(m::ClipId(g.id as u64)).cloned() {
                    let local = app.doc.project.clip(g.id).map_or(0, |vc| (app.editor.playhead - vc.start).clamp(0, vc.dur));
                    re_anchor(app, g, &o, g.content_point(0.0, 0.0) - g.source / 2.0, local, frame);
                }
            }
            // The topmost handle under the pointer is the one in play.
            if resp.drag_started() || resp.dragged() || resp.drag_stopped() || hot {
                handle = Some((*h, resp));
            }
        }
    }

    // The 3D gizmo last, so it is on top of the other handles.
    let mut gizmo = false;
    if let Some(g) = sel_geo.filter(|g| g.space.is_some()) {
        gizmo = super::three_d::gizmo(app, ui, canvas, scale, g.id, now);
        if gizmo {
            handle = None;
        }
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let press = ui.input(|i| i.pointer.press_origin());
    // Right-click on a layer: the clip menu.
    if body.secondary_clicked()
        && !gizmo
        && let Some(pos) = body.interact_pointer_pos()
    {
        let c = from_screen(pos);
        if let Some(g) = sel_geo.filter(|g| g.contains(c)).or_else(|| geos.iter().find(|g| g.contains(c)).copied()) {
            super::clip_menu::open(app, ui.ctx(), g.id, pos);
        }
    }
    let playhead = app.editor.playhead;
    let local_of = move |proj: &Project, id: u32| proj.clip(id).map_or(0, |c| (playhead - c.start).clamp(0, c.dur));
    if let Some((h, resp)) = &handle {
        if resp.drag_started() {
            if let (Some(g), Some(pos)) = (sel_geo, press.or(resp.interact_pointer_pos())) {
                drag = Some(Drag { id: g.id, grab: *h, start: pos, geo: g, local: local_of(&app.doc.project, g.id) });
            }
        }
    } else if body.drag_started() && !gizmo {
        if let Some(pos) = press.or(body.interact_pointer_pos()) {
            // The selection if it was grabbed, otherwise the topmost layer.
            let c = from_screen(pos);
            let hit = sel_geo.filter(|g| g.contains(c)).or_else(|| geos.iter().find(|g| g.contains(c)).copied());
            if let Some(g) = hit.filter(|g| !locked(&app.doc.project, g.id)) {
                app.editor.selection = vec![g.id];
                drag = Some(Drag { id: g.id, grab: Grab::Move, start: pos, geo: g, local: local_of(&app.doc.project, g.id) });
            }
        }
    }

    // The clip as it was when the drag began: every frame of a drag works
    // from it, so nothing piles up.
    let origin_id = drag_id.with("origin");
    if let Some(d) = drag {
        if ui.data(|m| m.get_temp::<(u32, m::Clip)>(origin_id)).is_none_or(|(id, _)| id != d.id) || handle.as_ref().is_some_and(|(_, r)| r.drag_started()) || body.drag_started() {
            if let Some(mc) = app.doc.model.active().clip(m::ClipId(d.id as u64)).cloned() {
                ui.data_mut(|m| m.insert_temp(origin_id, (d.id, mc)));
            }
        }
    }
    let origin: Option<m::Clip> = ui.data(|m| m.get_temp::<(u32, m::Clip)>(origin_id)).map(|(_, c)| c);
    let mut guides: Vec<(bool, f32)> = Vec::new();
    if let (Some(d), Some(pos)) = (drag, pointer) {
        let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let g = d.geo;
        let x0 = g.xf;
        let mut x = x0;
        let readout;
        match d.grab {
            Grab::Move => {
                let mut delta = (pos - d.start) / scale;
                // Shift keeps the move on one axis.
                if shift {
                    if delta.x.abs() > delta.y.abs() { delta.y = 0.0 } else { delta.x = 0.0 }
                }
                x.pos = x0.pos + delta;
                if let Some(s3) = &g.space {
                    // Slides parallel to the screen at the layer's depth.
                    let depth = s3.view.view(s3.anchor)[2].max(1.0);
                    let k = (depth / s3.view.zoom) as f32;
                    let r = s3.view.ray(m::Vec2::new((delta.x * k) as f64, (delta.y * k) as f64));
                    let world = [r[0] - s3.view.axes[2][0], r[1] - s3.view.axes[2][1], r[2] - s3.view.axes[2][2]];
                    x.pos = x0.pos + Vec2::new((world[0] * s3.view.zoom) as f32, (world[1] * s3.view.zoom) as f32);
                    x.z = x0.z + (world[2] * s3.view.zoom) as f32;
                } else if app.prefs.snapping && !alt {
                    let moved = Geo { xf: x, ..g };
                    let (dx, dy) = snap(&moved, &geos, app.doc.project.width as f32, app.doc.project.height as f32, 6.0 / scale, &mut guides);
                    x.pos += Vec2::new(dx, dy);
                }
                readout = format!("{:.0}, {:.0}", x.pos.x, x.pos.y);
            }
            Grab::Scale { u, v } => {
                // About the anchor: in the layer's own axes, how far the
                // handle now is from the anchor against where it was.
                let rot = egui::emath::Rot2::from_angle(-x0.rot.to_radians());
                let a = screen(g.anchor_canvas());
                let grab = rot * (d.start - a);
                let now_ = rot * (pos - a);
                let ratio = |n: f32, o: f32| if o.abs() < 2.0 { None } else { Some(n / o) };
                let (kx, ky) = if u == 0.0 {
                    (None, ratio(now_.y, grab.y))
                } else if v == 0.0 {
                    (ratio(now_.x, grab.x), None)
                } else if shift {
                    // Shift on a corner: each axis on its own.
                    (ratio(now_.x, grab.x), ratio(now_.y, grab.y))
                } else {
                    let k = if grab.length_sq() < 4.0 { None } else { Some(now_.dot(grab) / grab.length_sq()) };
                    (k, k)
                };
                if let Some(k) = kx {
                    x.sx = (x0.sx * k).clamp(-100.0, 100.0);
                }
                if let Some(k) = ky {
                    x.sy = (x0.sy * k).clamp(-100.0, 100.0);
                }
                readout = if (x.sx - x.sy).abs() < 1e-4 { format!("{:.0}%", x.sx * 100.0) } else { format!("{:.0}% × {:.0}%", x.sx * 100.0, x.sy * 100.0) };
            }
            Grab::Rotate => {
                let a = screen(g.anchor_canvas());
                let a0 = (d.start - a).angle();
                let a1 = (pos - a).angle();
                let mut deg = x0.rot + (a1 - a0).to_degrees();
                // Unwrapped, so a spin past 180° keeps counting.
                let turns = ((deg - x0.rot) / 360.0).round();
                deg -= turns * 360.0;
                if shift {
                    deg = (deg / 15.0).round() * 15.0;
                }
                x.rot = deg;
                readout = format!("{:.1}°", x.rot);
            }
            Grab::Anchor => {
                let Some(mut s) = g.to_source(from_screen(pos)) else { return };
                // Snaps to the corners, edge middles and centre of the layer.
                if !alt {
                    for (u, v) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (0.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
                        let cp = g.content_point(u, v);
                        if screen(g.to_canvas(cp)).distance(pos) < 8.0 {
                            s = cp;
                        }
                    }
                }
                let anchor = s - g.source / 2.0;
                if let Some(o) = &origin {
                    re_anchor(app, g, o, anchor, d.local, frame);
                }
                readout = format!("anchor {:.0}, {:.0}", anchor.x, anchor.y);
            }
            Grab::PathKey(kt) => {
                let c = from_screen(pos);
                if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(d.id as u64)).cloned() {
                    if let Some(k) = mc.keyframes.get_mut("transform.position").and_then(|tr| tr.keys.iter_mut().find(|k| k.time == kt)) {
                        k.value = pt(c);
                    }
                    let seq = app.doc.seq();
                    app.doc.apply_gesture(&format!("path-key-{}-{kt}", d.id), "Move key", m::Edit::SetClip { seq, clip: Box::new(mc) });
                }
                readout = format!("{:.0}, {:.0}", c.x, c.y);
            }
        }
        if x != x0 && !matches!(d.grab, Grab::Anchor | Grab::PathKey(_)) {
            write(app, d.id, d.grab, x0, x, d.local, frame);
        }
        show_readout(ui, pos, &readout, &t);
    }
    for (vertical, v) in guides {
        let gp = ui.painter().with_clip_rect(canvas.expand(1.0));
        if vertical {
            gp.vline(canvas.center().x + v * scale, canvas.y_range(), Stroke::new(1.0, t.accent));
        } else {
            gp.hline(canvas.x_range(), canvas.center().y + v * scale, Stroke::new(1.0, t.accent));
        }
    }
    if ui.input(|i| i.pointer.any_released()) {
        keep(ui, drag_id, None);
        ui.data_mut(|m| m.remove::<(u32, m::Clip)>(origin_id));
    } else {
        keep(ui, drag_id, drag);
    }

    // Click: the topmost layer under the pointer; again on the same spot,
    // the one below it.
    if body.clicked() && handle.is_none() {
        if let Some(pos) = body.interact_pointer_pos() {
            if ui.data(|d| d.get_temp(kit::eyedropper_id()).unwrap_or(false)) {
                ui.data_mut(|d| d.insert_temp(kit::eyedropper_id(), false));
                kit::push(ui.ctx(), "Colour picked from the viewer", None, Some(crate::icons::Icon::Eyedropper));
                return;
            }
            let c = from_screen(pos);
            let hits: Vec<u32> = geos.iter().filter(|g| g.contains(c)).map(|g| g.id).collect();
            let ed = &mut app.editor;
            if hits.is_empty() {
                ed.selection.clear();
                ed.last_pick = None;
            } else {
                let idx = match &ed.last_pick {
                    Some((p0, h0, i)) if p0.distance(pos) < 4.0 && *h0 == hits => (i + 1) % hits.len(),
                    _ => 0,
                };
                ed.selection = vec![hits[idx]];
                ed.last_pick = Some((pos, hits, idx));
            }
        }
    }
    // Hover: outline what a click would pick.
    if body.hovered() && handle.is_none() && drag.is_none() {
        if let Some(pos) = pointer {
            if let Some(g) = geos.iter().find(|g| g.contains(from_screen(pos))).filter(|g| Some(g.id) != sel) {
                let q = g.quad().map(screen);
                for k in 0..4 {
                    p.line_segment([q[k], q[(k + 1) % 4]], Stroke::new(1.0, theme::alpha(t.select, 0.6)));
                }
            }
        }
    }
    // Arrow keys nudge the selection a pixel (ten with shift).
    if let Some(g) = sel_geo.filter(|_| ui.memory(|m| m.focused().is_none())) {
        let (l, r, u, d, shift) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.modifiers.shift));
        let step = if shift { 10.0 } else { 1.0 };
        let delta = Vec2::new((r as u8 as f32 - l as u8 as f32) * step, (d as u8 as f32 - u as u8 as f32) * step);
        if delta != Vec2::ZERO && body.hovered() {
            let x0 = g.xf;
            let x = Xf { pos: x0.pos + delta, ..x0 };
            write(app, g.id, Grab::Move, x0, x, local_of(&app.doc.project, g.id), frame);
        }
    }
}

fn keep(ui: &egui::Ui, id: egui::Id, d: Option<Drag>) {
    ui.data_mut(|m| match d {
        Some(d) => {
            m.insert_temp(id, d);
        }
        None => m.remove::<Drag>(id),
    });
}

fn show_readout(ui: &egui::Ui, pos: Pos2, text: &str, t: &theme::Tokens) {
    let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("canvas-xform-readout")));
    let g = lp.layout_no_wrap(text.to_owned(), theme::body_medium(), t.text);
    let br = Rect::from_min_size(pos + Vec2::new(14.0, 14.0), g.size() + Vec2::new(12.0, 6.0));
    lp.rect(br, 6, t.panel, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    lp.galley(br.min + Vec2::new(6.0, 3.0), g, t.text);
}

/// Writes the changed parts of a placement as one gesture per drag.
fn write(app: &mut StudioApp, id: u32, grab: Grab, x0: Xf, x: Xf, local: Time, frame: Time) {
    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    if x.pos != x0.pos {
        put(&mut mc, "transform.position", pt(x.pos), local, frame);
    }
    if x.sx != x0.sx {
        put(&mut mc, "transform.scale_x", m::Value::Scalar(x.sx as f64), local, frame);
    }
    if x.sy != x0.sy {
        put(&mut mc, "transform.scale_y", m::Value::Scalar(x.sy as f64), local, frame);
    }
    if x.rot != x0.rot {
        put(&mut mc, "transform.rotation", m::Value::Angle(x.rot as f64), local, frame);
    }
    if x.z != x0.z {
        put(&mut mc, "transform.position_z", m::Value::Scalar(x.z as f64), local, frame);
    }
    let label = match grab {
        Grab::Move => "Move",
        Grab::Scale { .. } => "Scale",
        Grab::Rotate => "Rotate",
        Grab::Anchor => "Anchor point",
        Grab::PathKey(_) => "Move key",
    };
    let seq = app.doc.seq();
    app.doc.apply_gesture(&format!("canvas-{id}-{label}"), label, m::Edit::SetClip { seq, clip: Box::new(mc) });
}

/// Moves the anchor of `origin` (the clip as `g` measured it) to `anchor`
/// (source pixels from the source centre) without moving the picture: the
/// position follows, on every position key too when it is animated.
fn re_anchor(app: &mut StudioApp, g: Geo, origin: &m::Clip, anchor: Vec2, local: Time, frame: Time) {
    let mc = re_anchored(&g, origin, anchor, local, frame);
    let seq = app.doc.seq();
    app.doc.apply_gesture(&format!("canvas-{}-anchor", g.id), "Anchor point", m::Edit::SetClip { seq, clip: Box::new(mc) });
}

/// `origin` with its anchor at `anchor` (source pixels from the source
/// centre) and its position moved so the picture stays where `g` saw it;
/// an animated position moves on every key.
pub fn re_anchored(g: &Geo, origin: &m::Clip, anchor: Vec2, local: Time, frame: Time) -> m::Clip {
    let mut mc = origin.clone();
    // Where the new anchor point is on the canvas now: the position that
    // keeps everything where it is (in space, for a 3D layer).
    let (pos, dz) = match &g.space {
        Some(s3) => {
            let w = s3.plane.at((g.source.x / 2.0 + anchor.x) as f64, (g.source.y / 2.0 + anchor.y) as f64);
            (g.xf.pos + Vec2::new((w[0] - s3.anchor[0]) as f32, (w[1] - s3.anchor[1]) as f32), (w[2] - s3.anchor[2]) as f32)
        }
        None => (g.to_canvas(g.source / 2.0 + anchor), 0.0),
    };
    if dz != 0.0 {
        let z = num(mc.property_at("transform.position_z", local), 0.0) + dz;
        put(&mut mc, "transform.position_z", m::Value::Scalar(z as f64), local, frame);
    }
    let shift = pos - g.xf.pos;
    match mc.keyframes.get_mut("transform.position").filter(|tr| !tr.keys.is_empty()) {
        Some(track) => {
            for k in &mut track.keys {
                if let m::Value::Point(p) = &mut k.value {
                    p.x += shift.x as f64;
                    p.y += shift.y as f64;
                }
            }
        }
        None => {
            mc.set_property("transform.position", pt(pos));
        }
    }
    put(&mut mc, "transform.anchor", pt(anchor), local, frame);
    mc
}

/// A named point of a layer's box as (`u`, `v`) from -1 to 1: `centre`,
/// `top_left`, `top`, `top_right`, `left`, `right`, `bottom_left`,
/// `bottom`, `bottom_right`.
pub fn named_point(name: &str) -> Option<(f32, f32)> {
    let n = name.to_lowercase().replace([' ', '-'], "_").replace("center", "centre");
    Some(match n.as_str() {
        "centre" | "middle" => (0.0, 0.0),
        "top_left" | "left_top" => (-1.0, -1.0),
        "top" => (0.0, -1.0),
        "top_right" | "right_top" => (1.0, -1.0),
        "left" => (-1.0, 0.0),
        "right" => (1.0, 0.0),
        "bottom_left" | "left_bottom" => (-1.0, 1.0),
        "bottom" => (0.0, 1.0),
        "bottom_right" | "right_bottom" => (1.0, 1.0),
        _ => return None,
    })
}

/// How far to nudge a moved layer so its box's edges or centre meet the
/// canvas's or another layer's (canvas pixels), with the guide lines.
fn snap(g: &Geo, others: &[Geo], cw: f32, ch: f32, threshold: f32, guides: &mut Vec<(bool, f32)>) -> (f32, f32) {
    let bounds = |q: [Vec2; 4]| {
        let xs = q.map(|p| p.x);
        let ys = q.map(|p| p.y);
        let f = |a: [f32; 4], mx: bool| a.into_iter().fold(if mx { f32::MIN } else { f32::MAX }, |m, v| if mx { m.max(v) } else { m.min(v) });
        [f(xs, false), f(ys, false), f(xs, true), f(ys, true)]
    };
    let b = bounds(g.quad());
    let mut xs = vec![0.0, -cw / 2.0, cw / 2.0];
    let mut ys = vec![0.0, -ch / 2.0, ch / 2.0];
    for o in others.iter().filter(|o| o.id != g.id) {
        let ob = bounds(o.quad());
        xs.extend([ob[0], ob[2], (ob[0] + ob[2]) / 2.0]);
        ys.extend([ob[1], ob[3], (ob[1] + ob[3]) / 2.0]);
    }
    let axis = |mine: [f32; 3], targets: &[f32]| -> Option<(f32, f32)> {
        let mut best: Option<(f32, f32, f32)> = None;
        for &tg in targets {
            for v in mine {
                let d = (tg - v).abs();
                if d < threshold && best.is_none_or(|b| d < b.0) {
                    best = Some((d, tg - v, tg));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    };
    let mut out = (0.0, 0.0);
    if let Some((d, line)) = axis([b[0], b[2], (b[0] + b[2]) / 2.0], &xs) {
        out.0 = d;
        guides.push((true, line));
    }
    if let Some((d, line)) = axis([b[1], b[3], (b[1] + b[3]) / 2.0], &ys) {
        out.1 = d;
        guides.push((false, line));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(xf: Xf) -> Geo {
        Geo { id: 1, xf, source: Vec2::new(200.0, 100.0), fit: 1.0, content: [Vec2::ZERO, Vec2::new(200.0, 100.0)], space: None }
    }

    fn xf() -> Xf {
        Xf { pos: Vec2::ZERO, sx: 1.0, sy: 1.0, rot: 0.0, anchor: Vec2::ZERO, flip_x: false, flip_y: false, z: 0.0 }
    }

    #[test]
    fn anchor_lands_on_position_and_turns_the_layer() {
        // Anchor at the right edge's middle, placed at the canvas centre.
        let a = g(Xf { anchor: Vec2::new(100.0, 0.0), rot: 90.0, ..xf() });
        assert!(a.to_canvas(Vec2::new(200.0, 50.0)).length() < 1e-3);
        // The left edge's middle swings round to below it (y down, clockwise).
        let p = a.to_canvas(Vec2::new(0.0, 50.0));
        assert!((p - Vec2::new(0.0, -200.0)).length() < 1e-3 || (p - Vec2::new(0.0, 200.0)).length() < 1e-3, "{p:?}");
        // And back.
        let s = a.to_source(p).unwrap();
        assert!((s - Vec2::new(0.0, 50.0)).length() < 1e-3);
    }

    #[test]
    fn hit_box_follows_scale_rotation_and_content() {
        let a = g(Xf { sx: 2.0, sy: 0.5, rot: 0.0, ..xf() });
        assert!(a.contains(Vec2::new(190.0, 0.0)));
        assert!(!a.contains(Vec2::new(210.0, 0.0)));
        assert!(!a.contains(Vec2::new(0.0, 40.0)));
        // A shape drawn smaller than its frame is only its own box.
        let small = Geo { content: [Vec2::new(90.0, 40.0), Vec2::new(110.0, 60.0)], ..g(xf()) };
        assert!(small.contains(Vec2::new(5.0, 5.0)));
        assert!(!small.contains(Vec2::new(40.0, 0.0)));
    }

    #[test]
    fn keyed_properties_get_keys_and_still_ones_get_values() {
        let mut c = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 2 * SEC, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        put(&mut c, "transform.rotation", m::Value::Angle(30.0), SEC, SEC / 30);
        assert_eq!(c.get_property("transform.rotation"), Some(m::Value::Angle(30.0)));
        assert!(c.keyframes.get("transform.rotation").is_none_or(|t| t.keys.is_empty()));
        c.keyframes.insert("transform.rotation".into(), m::Track::new(vec![m::Keyframe::new(0, m::Value::Angle(0.0), m::Interp::Linear), m::Keyframe::new(2 * SEC, m::Value::Angle(90.0), m::Interp::Linear)]));
        put(&mut c, "transform.rotation", m::Value::Angle(10.0), SEC, SEC / 30);
        let tr = &c.keyframes["transform.rotation"];
        assert_eq!(tr.keys.len(), 3);
        assert_eq!(tr.keys[1].time, SEC);
        // On an existing key: that key changes, no new one.
        put(&mut c, "transform.rotation", m::Value::Angle(45.0), 2 * SEC - 1000, SEC / 30);
        let tr = &c.keyframes["transform.rotation"];
        assert_eq!(tr.keys.len(), 3);
        assert_eq!(tr.keys[2].value, m::Value::Angle(45.0));
    }
}
