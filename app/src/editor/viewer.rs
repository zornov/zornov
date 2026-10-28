// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Viewer (spec §4): canvas centred on `sunken` with a hairline frame,
//! overlays, before/after split, on-canvas handles with snapping, click to
//! pick (and click again to cycle) and the floating transport pill.
//!
//! Until `render` exists, frames are composited here from the generated
//! footage: each visual clip under the playhead is drawn with its
//! transform, grade and look.

use super::library::{self, LibDrag};
use super::{EditorState, toggle_play};
use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, IconButton, MenuItem, ShowNoted};
use crate::view::{self, Clip, ClipKind, LaneKind, MediaKind, Project, SEC, Time};
use crate::theme;
use crate::thumbs::{GradeParams, Thumbs};
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

pub(super) const TOOLBAR_H: f32 = 34.0;
const PILL_H: f32 = 40.0;
const PILL_MARGIN: f32 = 0.0;
const ZOOMS: [Option<f32>; 5] = [None, Some(0.25), Some(0.5), Some(1.0), Some(2.0)];
const ZOOM_NAMES: [&str; 5] = ["Fit", "25%", "50%", "100%", "200%"];
const QUALITY: [&str; 3] = ["Full", "Half", "Quarter"];

/// Where a layer sits on screen.
#[derive(Clone, Copy)]
#[allow(dead_code)] // where the simple preview drew it; picking uses `xform`
pub(super) struct Placed {
    pub(super) id: u32,
    pub(super) center: Pos2,
    pub(super) size: Vec2,
    pub(super) angle: f32,
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    app.editor.viewer_rect = r;
    let p = ui.painter().with_clip_rect(r);
    // Header and bottom bar are separated from the picture by hairlines.
    p.hline(r.x_range(), r.top() + TOOLBAR_H - 0.5, Stroke::new(1.0, t.line));
    p.hline(r.x_range(), r.bottom() - PILL_H + 0.5, Stroke::new(1.0, t.line));

    let area = Rect::from_min_max(
        egui::pos2(r.left() + 12.0, r.top() + TOOLBAR_H + 10.0),
        egui::pos2(r.right() - 12.0, r.bottom() - PILL_H - 10.0),
    );
    // The 3D workspace, when on, is the whole picture area.
    #[cfg(feature = "render")]
    if super::viewport::ui::show(app, ui, r, area) {
        toolbar(app, ui, r);
        transport(app, ui, r);
        return;
    }
    let proj = &app.doc.project;
    let aspect = proj.width as f32 / proj.height.max(1) as f32;
    let fit = (area.width() / aspect).min(area.height()).max(10.0);
    let fit_size = Vec2::new(fit * aspect, fit);
    let ppp = ui.ctx().pixels_per_point();
    let size = match app.editor.zoom {
        None => fit_size,
        Some(z) => Vec2::new(proj.width as f32, proj.height as f32) * z / ppp,
    };
    let canvas = Rect::from_center_size(area.center(), size);
    let canvas = Rect::from_min_size(egui::pos2(kit::snap(ui.ctx(), canvas.left()), kit::snap(ui.ctx(), canvas.top())), canvas.size());
    let cp = p.with_clip_rect(canvas.intersect(area.expand2(Vec2::new(16.0, 0.0))));
    let scale = canvas.width() / proj.width as f32;

    cp.rect_filled(canvas, 0, proj.background);
    // Frames come from `studio-render` when it is on and has produced one;
    // the simple compositor below draws until then (and always places
    // the layers for handles and picking).
    #[allow(unused_mut)]
    let mut rendered = false;
    #[cfg(feature = "render")]
    if app.prefs.preview_engine == 0 && !app.cli.flag("simple-preview") {
        let rv = app.render.get_or_insert_with(super::render_view::RenderView::new);
        let time = app.editor.hover_time.unwrap_or(app.editor.playhead);
        rv.sync(&app.doc, time);
        rv.set_view(None);
        let q = [1.0, 0.5, 0.25][app.editor.quality.min(2)];
        let px = ((canvas.width() * ppp * q).round().max(16.0) as u32, (canvas.height() * ppp * q).round().max(16.0) as u32);
        // Ask on the project's frame grid: the picture only changes at
        // frame boundaries, so finer times are the same frame re-rendered.
        let fps = (app.doc.project.fps as f64).max(1.0);
        let time = ((time as f64 * fps / 1e6).floor() * 1e6 / fps) as i64;
        let scrubbing = app.editor.hover_time.is_some() || matches!(app.editor.tl_drag, Some(super::TlDrag::Scrub | super::TlDrag::Move { .. } | super::TlDrag::Trim { .. } | super::TlDrag::Slip { .. }));
        rv.track_drops(ui.input(|i| i.time), app.editor.playing);
        if let Some(tex) = rv.frame(ui.ctx(), time, px, scrubbing, app.editor.playing && app.editor.hover_time.is_none()) {
            cp.image(tex, canvas, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            rendered = true;
        }
    }
    let proj = &app.doc.project;
    let placed = composite(&cp, &mut app.thumbs, proj, &app.editor, canvas, scale, ppp, !rendered);
    if proj.clips.is_empty() && canvas.height() > 80.0 {
        let c = canvas.center();
        cp.circle_filled(c - Vec2::new(0.0, 14.0), 20.0, Color32::from_white_alpha(18));
        icons::paint(&cp, Rect::from_center_size(c - Vec2::new(0.0, 14.0), Vec2::splat(20.0)), Icon::MonitorPlay, 20.0, Color32::from_white_alpha(170));
        cp.text(c + Vec2::new(0.0, 22.0), Align2::CENTER_CENTER, crate::i18n::shown("Your sequence plays here"), theme::body(), Color32::from_white_alpha(170));
    }

    if app.editor.safe_area {
        for (k, label) in [(0.9, "Title safe"), (0.95, "Action safe")] {
            let sr = Rect::from_center_size(canvas.center(), canvas.size() * k);
            let s = Stroke::new(1.0, Color32::from_white_alpha(110));
            for w in [sr.left_top(), sr.right_top(), sr.right_bottom(), sr.left_bottom(), sr.left_top()].windows(2) {
                cp.extend(egui::Shape::dashed_line(&[w[0], w[1]], s, 6.0, 4.0));
            }
            cp.text(sr.left_top() + Vec2::new(4.0, 3.0), Align2::LEFT_TOP, label, theme::caption(), Color32::from_white_alpha(170));
        }
    }
    // Composition guides: thirds, golden ratio, centre and an aspect
    // frame, from the same lines the AI reads the picture against.
    let on = active_guides(&app.editor, ui.ctx());
    crate::agent::guides::set_viewer(&on);
    {
        use crate::agent::guides::Guide;
        let at = |x: f32, y: f32| egui::pos2(egui::lerp(canvas.x_range(), x), egui::lerp(canvas.y_range(), y));
        let drawn: Vec<Guide> = on.iter().copied().filter(|g| *g != Guide::Safe).collect();
        let l = crate::agent::guides::lines(&drawn, proj.width as f32, proj.height as f32);
        if let Some(m) = l.matte {
            let inner = Rect::from_min_max(at(m[0], m[1]), at(m[2], m[3]));
            let dim = Color32::from_black_alpha(150);
            for band in [Rect::from_min_max(canvas.left_top(), egui::pos2(canvas.right(), inner.top())), Rect::from_min_max(egui::pos2(canvas.left(), inner.bottom()), canvas.right_bottom()), Rect::from_min_max(egui::pos2(canvas.left(), inner.top()), egui::pos2(inner.left(), inner.bottom())), Rect::from_min_max(egui::pos2(inner.right(), inner.top()), egui::pos2(canvas.right(), inner.bottom()))] {
                if band.width() > 0.5 && band.height() > 0.5 {
                    cp.rect_filled(band, 0, dim);
                }
            }
            cp.rect_stroke(inner, 0, Stroke::new(1.0, Color32::from_white_alpha(130)), egui::StrokeKind::Inside);
            if let Some(g) = drawn.iter().find(|g| matches!(g, Guide::Aspect(_))) {
                cp.text(inner.left_top() + Vec2::new(4.0, 3.0), Align2::LEFT_TOP, g.name(), theme::caption(), Color32::from_white_alpha(170));
            }
        }
        let thirds = Stroke::new(1.0, Color32::from_white_alpha(90));
        let golden = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 205, 90, 120));
        let frame = l.matte.unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let third_lines = crate::agent::guides::lines(&drawn.iter().copied().filter(|g| *g != Guide::Golden).collect::<Vec<_>>(), proj.width as f32, proj.height as f32);
        for &x in &l.vertical {
            let s = if third_lines.vertical.contains(&x) { thirds } else { golden };
            cp.line_segment([at(x, frame[1]), at(x, frame[3])], s);
        }
        for &y in &l.horizontal {
            let s = if third_lines.horizontal.contains(&y) { thirds } else { golden };
            cp.line_segment([at(frame[0], y), at(frame[2], y)], s);
        }
        if on.contains(&Guide::Centre) {
            let c = canvas.center();
            let arm = canvas.width() / 40.0;
            cp.hline(egui::Rangef::new(c.x - arm, c.x + arm), c.y, thirds);
            cp.vline(c.x, egui::Rangef::new(c.y - arm, c.y + arm), thirds);
        }
    }
    if app.editor.before_after {
        before_after(app, ui, canvas, ppp);
    }

    // Hit boxes and handles from each layer's real placement (anchor,
    // scale on each axis, keyframes), in stacking order.
    let order: Vec<u32> = placed.iter().map(|pl| pl.id).collect();
    // Pins show on the selected layer; while a pin tool is on, it has the
    // pointer instead of the transform handles.
    let pin_tool = super::pins::tool(ui.ctx());
    super::pins::interact(app, ui, canvas, area, scale);
    // The bone tool, when on, has the pointer as a pin tool does.
    let pin_tool = if super::bones::interact(app, ui, canvas, area, scale) { super::pins::PinTool::Puppet } else { pin_tool };
    // The Track tool: a track's path, new tracks and corrections.
    #[cfg(feature = "render")]
    let track_tool = pin_tool == super::pins::PinTool::Off && (super::cutout::interact(app, ui, canvas, area, scale) | super::track::interact(app, ui, canvas, area, scale));
    #[cfg(not(feature = "render"))]
    let track_tool = false;
    // Sculpting the selected model, then a camera tool: each has the
    // pointer over the whole picture.
    #[cfg(feature = "render")]
    let sculpting = pin_tool == super::pins::PinTool::Off && !track_tool && super::sculpt::interact(app, ui, area, scale);
    #[cfg(not(feature = "render"))]
    let sculpting = false;
    let camera_tool = pin_tool == super::pins::PinTool::Off && !track_tool && !sculpting && super::three_d::camera_interact(app, ui, area, scale);
    if pin_tool == super::pins::PinTool::Off && !camera_tool && !track_tool && !sculpting {
        super::xform::interact(app, ui, canvas, area, &order, scale);
    }
    // Marks for the AI editor: on top of the handles while a tool is on.
    super::ai_marks::interact(app, ui, canvas, scale);
    super::review::viewer(app, ui, canvas, scale);
    // Double-click words to edit them where they are.
    super::canvas_text::interact(app, ui, canvas, scale, &placed);
    // Scopes over the picture, above the handles.
    super::scopes::show(app, ui, area);
    toolbar(app, ui, r);
    transport(app, ui, r);

    // Drop from the Library onto the canvas: add at the playhead.
    let drop = ui.interact(area, egui::Id::unique("viewer-drop"), Sense::hover());
    if let Some(payload) = drop.dnd_release_payload::<LibDrag>() {
        let ctx = ui.ctx().clone();
        library::add(app, &ctx, payload.rail, payload.index, None, None);
    } else if drop.dnd_hover_payload::<LibDrag>().is_some() {
        ui.painter().rect_stroke(canvas, 0, Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
    }
}

/// A subtle radial vignette: the surround darkens toward its edges.
fn vignette(p: &egui::Painter, r: Rect, t: &theme::Tokens) {
    let mut mesh = egui::Mesh::default();
    let c = r.center();
    let inner = theme::alpha(Color32::BLACK, 0.0);
    let outer = theme::alpha(Color32::BLACK, if t.dark { 0.35 } else { 0.07 });
    mesh.colored_vertex(c, inner);
    let n = 48;
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        // Ellipse that reaches the corners.
        let d = Vec2::new(a.cos() * r.width() * 0.72, a.sin() * r.height() * 0.72);
        let pt = c + d;
        mesh.colored_vertex(egui::pos2(pt.x.clamp(r.left(), r.right()), pt.y.clamp(r.top(), r.bottom())), outer);
    }
    for i in 1..=n as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Fraction (0..1) of a media item's length shown at `time` in `clip`.
fn media_phase(clip: &Clip, media: Option<&view::MediaItem>, time: Time) -> f32 {
    let Some(m) = media else { return 0.0 };
    if m.kind == MediaKind::Image {
        return 0.0;
    }
    let local = (time - clip.start) as f64 * clip.speed as f64 / 100.0 + clip.src_in as f64;
    (local / m.dur.max(1) as f64).clamp(0.0, 1.0) as f32
}

fn grade_of(c: &Clip) -> GradeParams {
    let g = &c.grade;
    let look = c.look.as_deref().and_then(|l| crate::catalogue::looks().iter().position(|x| x.id == l)).map(|i| i as u8);
    let effects = c.effects.iter().filter(|e| e.enabled).map(|e| (e.kind.clone(), e.params.clone())).collect();
    if !g.enabled {
        return GradeParams { effects, ..Default::default() };
    }
    GradeParams { exposure: g.exposure, contrast: g.contrast, saturation: g.saturation, temperature: g.temperature, tint: g.tint, look, effects }
}

/// Motion-in preset progress multiplier for the first half second.
fn motion_in(c: &Clip, time: Time) -> (f32, Vec2, f32) {
    let Some(name) = c.motion_in.as_deref() else { return (1.0, Vec2::ZERO, 1.0) };
    let k = ((time - c.start) as f32 / (SEC as f32 * c.extra.motion_dur.max(0.1))).clamp(0.0, 1.0);
    let e = kit::ease_out(k);
    match name {
        "Fade In" => (e, Vec2::ZERO, 1.0),
        "Rise" => (e, Vec2::new(0.0, (1.0 - e) * 60.0), 1.0),
        "Drop" => (e, Vec2::new(0.0, -(1.0 - e) * 60.0), 1.0),
        "Zoom In" | "Pop" | "Spin In" => (e, Vec2::ZERO, 0.6 + 0.4 * e),
        "Zoom Out" => (e, Vec2::ZERO, 1.3 - 0.3 * e),
        "Slide Left" => (1.0, Vec2::new((1.0 - e) * 300.0, 0.0), 1.0),
        "Slide Right" => (1.0, Vec2::new(-(1.0 - e) * 300.0, 0.0), 1.0),
        _ => (1.0, Vec2::ZERO, 1.0),
    }
}

/// Draw the frame at the playhead. Returns where each selectable layer
/// ended up, topmost first.
#[allow(clippy::too_many_arguments)]
fn composite(p: &egui::Painter, thumbs: &mut Thumbs, proj: &Project, ed: &EditorState, canvas: Rect, scale: f32, ppp: f32, draw: bool) -> Vec<Placed> {
    let time = ed.playhead;
    let stack = proj.visual_stack_at(time);
    let q = [1.0, 0.5, 0.25][ed.quality.min(2)];
    let mut placed = Vec::new();
    // Bottom layer first.
    for c in stack.iter().rev() {
        let media = c.media.and_then(|m| proj.media(m));
        let tr = c.transform;
        let (fade, offset, mscale) = motion_in(c, time);
        let mut opacity = tr.opacity / 100.0 * fade;
        // Spine transitions: fade the incoming clip over the outgoing one.
        if proj.lane(c.lane).map(|l| l.kind) == Some(LaneKind::Spine) {
            if let Some(tr) = proj.transitions.iter().find(|t| t.right == c.id) {
                let k = ((time - c.start + tr.dur / 2) as f32 / tr.dur.max(1) as f32).clamp(0.0, 1.0);
                if k < 1.0 {
                    if let Some(prev) = proj.clip(tr.left).filter(|_| draw) {
                        draw_media(p, thumbs, proj, prev, canvas, scale, ppp, q, prev.end() - 1, 1.0);
                    }
                    if tr.kind == "dip" && draw {
                        p.rect_filled(canvas, 0, theme::alpha(Color32::BLACK, 1.0 - (k - 0.5).abs() * 2.0));
                        opacity *= ((k - 0.5) * 2.0).max(0.0);
                    } else {
                        opacity *= k;
                    }
                }
            }
        }
        match c.kind {
            ClipKind::Video => {
                let aspect = media.map_or(16.0 / 9.0, |m| m.width.max(1) as f32 / m.height.max(1) as f32);
                let cw = proj.width as f32;
                let ch = proj.height as f32;
                // Contain the media in the canvas, then apply the transform.
                let base = if aspect > cw / ch { Vec2::new(cw, cw / aspect) } else { Vec2::new(ch * aspect, ch) };
                let size = base * (tr.scale / 100.0) * mscale * scale;
                let center = canvas.center() + (Vec2::new(tr.x, tr.y) + offset) * scale;
                let angle = tr.rotation.to_radians();
                let rect = Rect::from_center_size(center, size);
                let px_w = ((size.x * ppp * q).clamp(32.0, 1920.0) / 32.0).ceil() as u16 * 32;
                let px_h = (px_w as f32 / aspect).round() as u16;
                let seed = media.map_or(c.id, |m| thumbs.seed_of(m));
                let phase = media_phase(c, media, time);
                // Quantise to the project's frame rate so frames are reused.
                let frames = (media.map_or(10.0, |m| m.dur as f32 / SEC as f32) * proj.fps).max(1.0);
                let tq = (phase * frames).floor() / frames;
                if !draw {
                } else if let Some(tex) = thumbs.graded(seed, tq, &grade_of(c), px_w, px_h.max(8)) {
                    library::image_rotated(p, tex, rect, angle, Color32::from_white_alpha((opacity * 255.0) as u8));
                } else {
                    p.rect_filled(rect, 0, theme::alpha(Color32::from_gray(40), opacity));
                }
                placed.push(Placed { id: c.id, center, size, angle });
            }
            ClipKind::Title if c.text.is_empty() => {
                // A shape: half the canvas at 100 %, as `render` draws it.
                let center = canvas.center() + (Vec2::new(tr.x, tr.y) + offset) * scale;
                let side = proj.width.min(proj.height) as f32 * 0.5 * (tr.scale / 100.0) * mscale * scale;
                if draw {
                    p.rect_stroke(Rect::from_center_size(center, Vec2::splat(side)), CornerRadius::same(8), Stroke::new(1.0, theme::alpha(Color32::WHITE, 0.5 * opacity)), egui::StrokeKind::Middle);
                }
                placed.push(Placed { id: c.id, center, size: Vec2::splat(side), angle: tr.rotation.to_radians() });
            }
            ClipKind::Title => {
                let txt = if c.text.is_empty() { c.name.as_str() } else { c.text.as_str() };
                let px = c.extra.text_size * (tr.scale / 100.0) * mscale * scale * proj.height as f32 / 1080.0;
                let font = egui::FontId::new(px.max(4.0), egui::FontFamily::Name(theme::SEMIBOLD.into()));
                let galley = p.layout_no_wrap(txt.to_owned(), font, Color32::WHITE);
                let center = canvas.center() + (Vec2::new(tr.x, tr.y) + offset) * scale;
                let size = galley.size();
                let col = theme::alpha(c.text_colour, opacity);
                // Soft shadow keeps titles legible over bright footage.
                if draw {
                p.galley(center - size / 2.0 + Vec2::new(0.0, px * 0.04), p.layout_no_wrap(txt.to_owned(), egui::FontId::new(px.max(4.0), egui::FontFamily::Name(theme::SEMIBOLD.into())), theme::alpha(Color32::BLACK, 0.35 * opacity)), Color32::BLACK);
                p.galley(center - size / 2.0, galley, col);
                }
                placed.push(Placed { id: c.id, center, size: size + Vec2::splat(8.0 * scale.max(0.2)), angle: tr.rotation.to_radians() });
            }
            ClipKind::Fx if !draw => {}
            ClipKind::Fx => {
                // Adjustment clip: a light approximation of its effect.
                if c.effects.iter().any(|e| e.kind == "glow" || e.kind == "bloom") {
                    p.rect_filled(canvas, 0, Color32::from_rgba_unmultiplied(255, 236, 210, (28.0 * opacity) as u8));
                } else if c.look.is_some() {
                    let n = 24;
                    for k in 0..n {
                        let f = k as f32 / n as f32;
                        p.rect_stroke(canvas.shrink(f * canvas.height() * 0.25), 0, Stroke::new(canvas.height() * 0.25 / n as f32 + 1.0, Color32::from_black_alpha(((1.0 - f) * 18.0 * opacity) as u8)), egui::StrokeKind::Inside);
                    }
                } else {
                    p.rect_filled(canvas, 0, Color32::from_rgba_unmultiplied(120, 90, 255, (18.0 * opacity) as u8));
                }
            }
            ClipKind::Audio => {}
        }
    }
    placed.reverse();
    placed
}

#[allow(clippy::too_many_arguments)]
fn draw_media(p: &egui::Painter, thumbs: &mut Thumbs, proj: &Project, c: &Clip, canvas: Rect, scale: f32, ppp: f32, q: f32, time: Time, opacity: f32) {
    let media = c.media.and_then(|m| proj.media(m));
    let size = Vec2::new(proj.width as f32, proj.height as f32) * (c.transform.scale / 100.0) * scale;
    let rect = Rect::from_center_size(canvas.center() + Vec2::new(c.transform.x, c.transform.y) * scale, size);
    let px_w = ((size.x * ppp * q).clamp(32.0, 1920.0) / 32.0).ceil() as u16 * 32;
    let px_h = (px_w as f32 * proj.height as f32 / proj.width as f32) as u16;
    let phase = media_phase(c, media, time);
    let seed = media.map_or(c.id, |m| thumbs.seed_of(m));
    if let Some(tex) = thumbs.graded(seed, phase, &grade_of(c), px_w, px_h.max(8)) {
        p.image(tex, rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::from_white_alpha((opacity * 255.0) as u8));
    }
}

/// Before/after: the left side shows the frame without grade and look.
fn before_after(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, ppp: f32) {
    let t = theme::tokens(ui.ctx());
    let proj = &app.doc.project;
    let x = egui::lerp(canvas.x_range(), app.editor.split_pos);
    let left = Rect::from_min_max(canvas.min, egui::pos2(x, canvas.bottom()));
    let lp = ui.painter().with_clip_rect(left);
    lp.rect_filled(left, 0, proj.background);
    if let Some(c) = proj.visual_stack_at(app.editor.playhead).into_iter().rev().find(|c| c.kind == ClipKind::Video && proj.lane(c.lane).map(|l| l.kind) == Some(LaneKind::Spine)) {
        let media = c.media.and_then(|m| proj.media(m));
        let phase = media_phase(c, media, app.editor.playhead);
        let w = ((canvas.width() * ppp).min(1920.0) / 32.0).ceil() as u16 * 32;
        let h = (w as f32 * proj.height as f32 / proj.width as f32) as u16;
        let seed = media.map_or(c.id, |m| app.thumbs.seed_of(m));
        if let Some(tex) = app.thumbs.graded(seed, phase, &GradeParams::default(), w, h) {
            lp.image(tex, canvas, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    let handle = Rect::from_center_size(egui::pos2(x, canvas.center().y), Vec2::new(12.0, canvas.height()));
    let resp = ui.interact(handle, egui::Id::unique("before-after"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if resp.dragged() {
        app.editor.split_pos = (app.editor.split_pos + resp.drag_delta().x / canvas.width()).clamp(0.02, 0.98);
    }
    let p = ui.painter();
    p.vline(x, canvas.y_range(), Stroke::new(2.0, Color32::WHITE));
    p.circle_filled(egui::pos2(x, canvas.center().y), 12.0, Color32::WHITE);
    icons::paint(p, Rect::from_center_size(egui::pos2(x, canvas.center().y), Vec2::splat(14.0)), Icon::ArrowsHorizontal, 14.0, t.accent_ink.gamma_multiply(1.0));
    for (label, pos, al) in [("Before", canvas.left_top() + Vec2::new(8.0, 8.0), Align2::LEFT_TOP), ("After", canvas.right_top() + Vec2::new(-8.0, 8.0), Align2::RIGHT_TOP)] {
        let g = p.layout_no_wrap(label.into(), theme::caption(), Color32::WHITE);
        let br = al.anchor_size(pos, g.size() + Vec2::new(12.0, 6.0));
        p.rect_filled(br, CornerRadius::same(4), Color32::from_black_alpha(150));
        p.galley(br.center() - g.size() / 2.0, g, Color32::WHITE);
    }
}

/// Overlays beyond the safe areas and thirds (kept for the session).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MoreGuides {
    golden: bool,
    centre: bool,
    /// Index into the aspect guides.
    aspect: Option<usize>,
}

const MORE_GUIDES: &str = "viewer-more-guides";

fn more_guides(ctx: &egui::Context) -> MoreGuides {
    ctx.data(|d| d.get_temp(egui::Id::unique(MORE_GUIDES))).unwrap_or_default()
}

/// The composition guides on in the viewer now.
pub fn active_guides(ed: &EditorState, ctx: &egui::Context) -> Vec<crate::agent::guides::Guide> {
    use crate::agent::guides::{ASPECTS, Guide};
    let more = more_guides(ctx);
    let mut g = Vec::new();
    if ed.thirds {
        g.push(Guide::Thirds);
    }
    if more.golden {
        g.push(Guide::Golden);
    }
    if more.centre {
        g.push(Guide::Centre);
    }
    if ed.safe_area {
        g.push(Guide::Safe);
    }
    if let Some((_, a)) = more.aspect.and_then(|i| ASPECTS.get(i)) {
        g.push(Guide::Aspect(*a));
    }
    g
}

/// The viewer's Overlays menu (its keys open it from anywhere in the editor).
pub(crate) fn overlays_id() -> egui::Id {
    egui::Id::unique("viewer-overlays")
}

fn toolbar(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let bar = Rect::from_min_size(r.min + Vec2::new(12.0, 4.0), Vec2::new(r.width() - 20.0, TOOLBAR_H - 8.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bui.spacing_mut().item_spacing.x = 4.0;
    bui.label(egui::RichText::new(crate::i18n::shown("Player")).font(theme::section_title()).color(t.text));
    bui.add_space(10.0);
    // The buttons scroll sideways when the viewer is narrower than they
    // are, leaving the picture's size and rate their room at the end.
    let (overlays_keys, before_after_keys) = (crate::keymap::hint(&app.prefs, "overlays"), crate::keymap::hint(&app.prefs, "before-after"));
    let proj = &app.doc.project;
    let info = format!("{} × {} · {} fps", proj.width, proj.height, proj.fps);
    let info_w = kit::text_width(&bui, &info, theme::caption()) + 8.0;
    let strip_w = (bui.available_width() - info_w - 8.0).max(120.0);
    egui::ScrollArea::horizontal().id_salt("viewer-tools").max_width(strip_w).auto_shrink([true, true]).show_h(&mut bui, |ui| {
        ui.horizontal(|bui| {
            let ed = &mut app.editor;
            let zi = ZOOMS.iter().position(|z| *z == ed.zoom).unwrap_or(0);
            if let Some(i) = kit::menu_button(bui, "viewer-zoom", zi, &ZOOM_NAMES, 84.0, None) {
                ed.zoom = ZOOMS[i];
            }
            let overlay_id = overlays_id();
            let any = ed.safe_area || ed.thirds || more_guides(bui.ctx()) != MoreGuides::default();
            let resp = IconButton::new(Icon::FrameCorners, "Overlays").shortcut(&overlays_keys).active(any || kit::is_open(bui.ctx(), overlay_id)).show(bui);
            if resp.clicked() {
                kit::toggle_open(bui.ctx(), overlay_id);
            }
            kit::popover(bui.ctx(), overlay_id, resp.rect, kit::Side::Below, 200.0, |ui| {
                kit::menu_heading(ui, "Overlays");
                if MenuItem::new("Safe areas").checked(ed.safe_area).show(ui).clicked() {
                    ed.safe_area = !ed.safe_area;
                }
                if MenuItem::new("Rule of thirds").checked(ed.thirds).show(ui).clicked() {
                    ed.thirds = !ed.thirds;
                }
                let mut more = more_guides(ui.ctx());
                let was = more;
                if MenuItem::new("Golden ratio").checked(more.golden).show(ui).clicked() {
                    more.golden = !more.golden;
                }
                if MenuItem::new("Centre cross").checked(more.centre).show(ui).clicked() {
                    more.centre = !more.centre;
                }
                kit::menu_separator(ui);
                kit::menu_heading(ui, "Aspect guide");
                for (i, (name, _)) in crate::agent::guides::ASPECTS.iter().enumerate() {
                    if MenuItem::new(name).checked(more.aspect == Some(i)).show(ui).clicked() {
                        more.aspect = if more.aspect == Some(i) { None } else { Some(i) };
                    }
                }
                if more != was {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(MORE_GUIDES), more));
                }
            });
            if IconButton::new(Icon::SquareSplitHorizontal, "Before / after").shortcut(&before_after_keys).active(ed.before_after).show(bui).clicked() {
                ed.before_after = !ed.before_after;
            }
            // Camera tools, cameras and lights.
            super::three_d::toolbar_buttons(app, bui);
            // Vector illustration: logos, icons, illustrations, infographics.
            super::vector::toolbar_button(app, bui);
            // Motion graphics templates, shape layers, charts, 3D text.
            super::mograph::toolbar_buttons(app, bui);
            // The 3D workspace: move through the scene.
            #[cfg(feature = "render")]
            super::viewport::ui::toolbar_button(app, bui);
            // Point the AI editor at part of the picture.
            super::ai_marks::toolbar_buttons(app, bui);
            super::scopes::toolbar_button(bui);
            super::capture::toolbar_button(bui);
            let key_hit = !bui.ctx().egui_wants_keyboard_input() && crate::keymap::hit(bui.ctx(), &app.prefs, "before-after");
            let ed = &mut app.editor;
            if key_hit {
                ed.before_after = !ed.before_after;
            }
            // Dropped frames: information only; a click offers a lower quality,
            // the app never changes it by itself.
            #[cfg(feature = "render")]
            if app.render.as_ref().is_some_and(|r| r.dropping > 2) && app.editor.quality < 2 {
                let id = egui::Id::unique("dropped-frames");
                let resp = kit::Button::ghost("Dropping frames").icon(Icon::Warning).compact().show(bui);
                if resp.clicked() {
                    kit::toggle_open(bui.ctx(), id);
                }
                let ed = &mut app.editor;
                kit::popover(bui.ctx(), id, resp.rect, kit::Side::Below, 260.0, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(egui::RichText::new(crate::i18n::shown("Playback can't keep up at this quality. Lower it while you edit?")).font(theme::body()).color(t.text)).wrap());
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        let next = ["Half", "Quarter"][ed.quality.min(1)];
                        if kit::Button::primary(&crate::i18n::tf(format!("Switch to {next}"))).compact().show(ui).clicked() {
                            ed.quality += 1;
                            kit::set_open(ui.ctx(), id, false);
                        }
                    });
                    ui.add_space(4.0);
                });
            }
        });
    });
    bui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        kit::label_trunc(ui, egui::RichText::new(info).font(theme::caption()).color(t.text_2));
    });
}

fn transport(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let key = |id: &str| crate::keymap::hint(&app.prefs, id);
    let (back_keys, play_keys, fwd_keys, full_keys) = (key("frame-back"), key("play"), key("frame-fwd"), key("fullscreen"));
    let proj = &app.doc.project;
    let fps = proj.fps;
    let dur = crate::services::transport::duration(&app.doc);
    let ed = &mut app.editor;
    let tc = view::timecode(ed.playhead, fps);
    let total = view::timecode(dur, fps);
    let bar = Rect::from_min_max(egui::pos2(r.left() + 12.0, r.bottom() - PILL_H), egui::pos2(r.right() - 10.0, r.bottom()));

    // Centre: previous frame, play, next frame (plain icons).
    let frame = ed.frame_dur(fps);
    let mid = Rect::from_center_size(bar.center(), Vec2::new(96.0, 28.0));

    // Left: the playhead time in the accent, the length muted. In a narrow
    // viewer the length goes first, then the time ends in an ellipsis; the
    // hover shows both.
    let room = mid.left() - 8.0 - bar.left();
    let mut job = egui::text::LayoutJob::default();
    job.append(&tc, 0.0, egui::TextFormat { font_id: theme::mono_small(), color: t.accent, ..Default::default() });
    job.append(&format!(" / {total}"), 0.0, egui::TextFormat { font_id: theme::mono_small(), color: t.text_2, ..Default::default() });
    let g = ui.painter().layout_job(job);
    let full = g.size().x <= room;
    if full {
        ui.painter().galley(egui::pos2(bar.left(), bar.center().y - g.size().y / 2.0), g, t.text);
    } else {
        kit::paint_text_clipped(ui.painter(), egui::pos2(bar.left(), bar.center().y), Align2::LEFT_CENTER, &tc, theme::mono_small(), t.accent, room);
        let hover = Rect::from_min_max(egui::pos2(bar.left(), bar.top()), egui::pos2(bar.left() + room.max(0.0), bar.bottom()));
        ui.interact(hover, egui::Id::unique("viewer-timecode"), Sense::hover()).on_hover_text(format!("{tc} / {total}"));
    }
    let mut mui = ui.new_child(egui::UiBuilder::new().max_rect(mid).layout(egui::Layout::left_to_right(egui::Align::Center)));
    mui.spacing_mut().item_spacing.x = 4.0;
    if IconButton::new(Icon::CaretLeft, "Previous frame").shortcut(&back_keys).show(&mut mui).clicked() {
        ed.playhead = (ed.playhead - frame).max(0);
        ed.playing = false;
    }
    let (pr, presp) = mui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let st = kit::State::from_response(&mui, &presp, None);
    icons::paint_fill(mui.painter(), pr, if ed.playing { Icon::PauseFill } else { Icon::PlayFill }, 18.0, theme::mix(t.text, Color32::WHITE, st.hover));
    if st.focused {
        kit::focus_ring(mui.painter(), pr, CornerRadius::same(theme::radius::CONTROL), &t);
    }
    let playing = ed.playing;
    presp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if playing { "Pause" } else { "Play" }));
    if kit::tip(presp, if ed.playing { "Pause" } else { "Play" }, Some(&play_keys)).clicked() {
        toggle_play(ed);
    }
    if IconButton::new(Icon::CaretRight, "Next frame").shortcut(&fwd_keys).show(&mut mui).clicked() {
        ed.playhead += frame;
        ed.playing = false;
    }

    // Right: loop, quality, full screen.
    let right = Rect::from_min_max(egui::pos2(mid.right() + 8.0, bar.top()), bar.max);
    let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
    rui.spacing_mut().item_spacing.x = 2.0;
    if IconButton::new(if ed.fullscreen { Icon::CornersIn } else { Icon::CornersOut }, if ed.fullscreen { "Exit full screen" } else { "Full screen" }).shortcut(&full_keys).show(&mut rui).clicked() {
        ed.fullscreen = !ed.fullscreen;
    }
    if r.width() >= 520.0 {
        if let Some(i) = kit::menu_button(&mut rui, "viewer-quality", ed.quality, &QUALITY, 64.0, None) {
            ed.quality = i;
        }
    }
    if IconButton::new(Icon::Repeat, if proj.range.is_some() { "Loop in–out range" } else { "Loop sequence" }).shortcut("⌘L").active(ed.loop_on).show(&mut rui).clicked() {
        ed.loop_on = !ed.loop_on;
    }
    if rui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::L))) {
        ed.loop_on = !ed.loop_on;
    }
}
