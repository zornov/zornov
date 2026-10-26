// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Timeline (spec §5): toolbar, ruler, lane headers, clips with
//! filmstrips or waveforms, transitions as bow-ties, markers, in/out range,
//! the playhead, snapping and the editing tools.

use super::library::{self, LibDrag};
use super::{Action, TlDrag, Tool, clip_fill, run};
use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, IconButton, Segment};
use crate::view::{self, Clip, ClipKind, Lane, LaneKind, MediaKind, Project, SEC, Time};
use crate::theme;
use studio_model as m;
use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Stroke, Vec2};

pub const HEADER_W: f32 = 120.0;
pub(super) const TOOLBAR_H: f32 = 34.0;
const RULER_H: f32 = 24.0;
const CLIP_TOP: f32 = 18.0;
const SNAP_PX: f32 = 8.0;
const LANE_GAP: f32 = 3.0;
const MIN_PPS: f32 = 2.0;
const MAX_PPS: f32 = 600.0;

pub(super) struct View {
    x0: f32,
    pps: f32,
    scroll_x: f32,
}

impl View {
    pub(super) fn x(&self, t: Time) -> f32 {
        self.x0 + (t as f64 / SEC as f64) as f32 * self.pps - self.scroll_x
    }
    pub(super) fn t(&self, x: f32) -> Time {
        (((x - self.x0 + self.scroll_x) / self.pps) as f64 * SEC as f64) as Time
    }
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    toolbar(app, ui, Rect::from_min_size(r.min, Vec2::new(r.width(), TOOLBAR_H)), r);

    let body = Rect::from_min_max(egui::pos2(r.left(), r.top() + TOOLBAR_H), r.max);
    kit::hline(ui, r.left()..=r.right(), body.top());
    let ruler = Rect::from_min_max(egui::pos2(body.left() + HEADER_W, body.top()), egui::pos2(body.right(), body.top() + RULER_H));
    let lanes_r = Rect::from_min_max(egui::pos2(body.left(), ruler.bottom()), body.max - Vec2::new(0.0, 6.0));
    let track_r = Rect::from_min_max(egui::pos2(body.left() + HEADER_W, ruler.bottom()), lanes_r.max);

    let proj_dur = app.doc.project.duration();
    // Until the user zooms, keep the sequence fitted as the panel resizes.
    if app.editor.auto_fit && (app.editor.fit_width - track_r.width()).abs() > 0.5 {
        app.editor.fit_pending = true;
    }
    if app.editor.fit_pending {
        app.editor.fit_width = track_r.width();
        let span = (proj_dur.max(10 * SEC) as f64 / SEC as f64) as f32;
        app.editor.px_per_sec = ((track_r.width() - 48.0) / span).clamp(MIN_PPS, MAX_PPS);
        app.editor.tl_scroll.x = 0.0;
        app.editor.fit_pending = false;
    }
    let view = View { x0: track_r.left() + 12.0, pps: app.editor.px_per_sec, scroll_x: app.editor.tl_scroll.x };

    // Lane layout (scrolls vertically when lanes overflow).
    let total_h: f32 = app.doc.project.lanes.iter().map(|l| l.height + LANE_GAP).sum();
    let max_scroll_y = (total_h - lanes_r.height() + 4.0).max(0.0);
    app.editor.tl_scroll.y = app.editor.tl_scroll.y.clamp(0.0, max_scroll_y);
    let mut lane_rects: Vec<(u32, Rect)> = Vec::new();
    let mut y = lanes_r.top() + 4.0 - app.editor.tl_scroll.y;
    for l in &app.doc.project.lanes {
        lane_rects.push((l.id, Rect::from_min_size(egui::pos2(lanes_r.left(), y), Vec2::new(lanes_r.width(), l.height))));
        y += l.height + LANE_GAP;
    }

    scroll_and_zoom(app, ui, track_r.union(ruler), &view, max_scroll_y, proj_dur);
    // The lanes scroll both ways, the ruler sideways with them.
    kit::note_scroll(&ctx, lanes_r, false);
    kit::note_scroll(&ctx, track_r.union(ruler), true);
    let view = View { x0: view.x0, pps: app.editor.px_per_sec, scroll_x: app.editor.tl_scroll.x };

    // Keep the playhead in view while playing.
    if app.editor.playing {
        let px = view.x(app.editor.playhead);
        if px > track_r.right() - 24.0 || px < track_r.left() {
            app.editor.tl_scroll.x = ((app.editor.playhead as f64 / SEC as f64) as f32 * view.pps - 48.0).max(0.0);
        }
    }

    let clip_p = ui.painter().with_clip_rect(track_r.intersect(ui.clip_rect()));
    let comp = super::comp::is_on(&ctx);
    let graph = super::graph::is_on(&ctx) || comp;
    if !graph {
        lanes_bg(app, &clip_p, &lane_rects, track_r, &t);
    }
    let range = app.doc.project.range;
    if let Some((a, b)) = range {
        let band = Rect::from_min_max(egui::pos2(view.x(a), track_r.top()), egui::pos2(view.x(b), track_r.bottom()));
        clip_p.rect_filled(band, 0, theme::alpha(t.accent, 0.05));
    }
    for m in &app.doc.project.markers {
        clip_p.vline(view.x(m.time), track_r.y_range(), Stroke::new(1.0, theme::alpha(t.warn, 0.28)));
    }

    // The graph editor takes the lanes' place, on the same time axis.
    let interact = if comp {
        // The compositor takes their place too, with the whole width.
        super::comp::show(app, ui, lanes_r);
        false
    } else if graph {
        let list = Rect::from_min_max(lanes_r.min, egui::pos2(lanes_r.left() + HEADER_W, lanes_r.bottom()));
        super::graph::show(app, ui, list, track_r, &view);
        false
    } else {
        let i = clips(app, ui, &clip_p, &view, &lane_rects, track_r);
        headers(app, ui, &lane_rects, lanes_r, &t);
        i
    };
    if max_scroll_y > 2.0 && !graph {
        // A thin indicator so it is clear more lanes are below.
        let track_h = track_r.height() - 8.0;
        let thumb_h = (track_h * track_r.height() / (total_h + 8.0)).max(24.0);
        let k = app.editor.tl_scroll.y / max_scroll_y;
        let y0 = track_r.top() + 4.0 + (track_h - thumb_h) * k;
        let bar = Rect::from_min_size(egui::pos2(track_r.right() - 7.0, y0), Vec2::new(4.0, thumb_h));
        ui.painter().rect_filled(bar, CornerRadius::same(2), theme::alpha(t.text_2, 0.6));
    }
    ruler_ui(app, ui, ruler, &view, &t);
    corner(app, ui, Rect::from_min_max(body.min, egui::pos2(ruler.left(), ruler.bottom())), &t);

    // Snap line with a short pulse when it appears.
    if let Some((st, born)) = app.editor.snap_line {
        if app.editor.tl_drag.is_some() {
            let x = view.x(st);
            let age = (ctx.input(|i| i.time) - born) as f32;
            let pulse = (1.0 - age / 0.14).clamp(0.0, 1.0);
            let p = ui.painter().with_clip_rect(track_r.union(ruler));
            let _ = pulse;
            p.vline(x, ruler.top()..=track_r.bottom(), Stroke::new(1.0, t.accent));
        }
    }

    // Playhead: an ember line with a soft glow and a small rounded tab in
    // the ruler.
    let px = kit::snap(&ctx, view.x(app.editor.playhead)) + 0.5;
    if px >= ruler.left() - 6.0 && px <= ruler.right() + 6.0 {
        let p = ui.painter().with_clip_rect(Rect::from_min_max(egui::pos2(ruler.left() - 6.0, ruler.top()), track_r.max));
        // A plain white line with a small handle in the ruler.
        let top = ruler.top() + 2.0;
        p.vline(px, top + 10.0..=track_r.bottom(), Stroke::new(1.0, Color32::WHITE));
        let h = vec![egui::pos2(px - 5.0, top), egui::pos2(px + 5.0, top), egui::pos2(px + 5.0, top + 7.0), egui::pos2(px, top + 12.0), egui::pos2(px - 5.0, top + 7.0)];
        p.add(egui::Shape::convex_polygon(h, Color32::WHITE, Stroke::NONE));
    }

    // Library drop.
    if !graph {
        drop_target(app, ui, track_r, &view, &lane_rects);
    }

    if app.doc.project.clips.is_empty() && app.editor.tl_drag.is_none() && !graph {
        let msg_r = Rect::from_center_size(track_r.center(), Vec2::new(track_r.width().min(420.0), 44.0));
        let p = ui.painter();
        p.rect(msg_r, CornerRadius::same(theme::radius::CARD), theme::alpha(t.panel, 0.9), Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        icons::paint(p, Rect::from_center_size(egui::pos2(msg_r.left() + 22.0, msg_r.center().y), Vec2::splat(16.0)), Icon::FilmStrip, 16.0, t.text_2);
        p.text(egui::pos2(msg_r.left() + 40.0, msg_r.center().y), Align2::LEFT_CENTER, crate::i18n::shown("Drag media here, or press + on a Library item."), theme::body(), t.text_2);
    }
    let _ = interact;
}

fn toolbar(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, _panel: Rect) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    let inner = r.shrink2(Vec2::new(10.0, 4.0));
    let mut action = None;
    // Right: zoom (laid out first, so the tools know their room).
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 4.0;
    if IconButton::new(Icon::ArrowsOutLineHorizontal, "Fit sequence").shortcut("⇧Z").show(&mut right).clicked() {
        action = Some(Action::Fit);
    }
    if IconButton::new(Icon::MagnifyingGlassPlus, "Zoom in").shortcut("⌘=").show(&mut right).clicked() {
        action = Some(Action::ZoomIn);
    }
    // Log-scale zoom slider.
    let mut z = ((app.editor.px_per_sec / MIN_PPS).ln() / (MAX_PPS / MIN_PPS).ln() * 100.0).clamp(0.0, 100.0);
    let w = if inner.width() > 760.0 { 120.0 } else { 72.0 };
    if kit::Slider::new("Timeline zoom", &mut z, 0.0..=100.0).default(0.0).width(w).show(&mut right).changed() {
        zoom_to(app, MIN_PPS * (MAX_PPS / MIN_PPS).powf(z / 100.0), None, r.left() + HEADER_W + 12.0);
        app.editor.auto_fit = false;
    }
    if IconButton::new(Icon::MagnifyingGlassMinus, "Zoom out").shortcut("⌘-").show(&mut right).clicked() {
        action = Some(Action::ZoomOut);
    }
    if inner.width() > 820.0 {
        divider(&mut right);
        let n = app.editor.selection.len();
        let label = if n == 0 { format!("{} clips", app.doc.project.clips.len()) } else { format!("{n} selected") };
        right.label(egui::RichText::new(label).font(theme::caption()).color(t.text_2));
    }
    // Left: the tools, in the room the zoom leaves; a narrow timeline
    // scrolls them sideways.
    let left = Rect::from_min_max(inner.min, egui::pos2((right.min_rect().left() - 8.0).max(inner.left()), inner.bottom()));
    let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::left_to_right(egui::Align::Center)));
    outer.set_clip_rect(left.intersect(ui.clip_rect()));
    kit::scroll_strip(&mut outer, "timeline-tools", |bar| {
        bar.spacing_mut().item_spacing.x = 4.0;
        for (tool, icon, name, key) in [(Tool::Select, Icon::Cursor, "Select (drag empty space to box-select)", "A"), (Tool::Blade, Icon::Scissors, "Blade", "B"), (Tool::Slip, Icon::ArrowsHorizontal, "Slip", "Y"), (Tool::Range, Icon::BracketsSquare, "Range", "R")] {
            if IconButton::new(icon, name).shortcut(key).active(app.editor.tool == tool).show(bar).clicked() {
                app.editor.tool = tool;
                app.editor.cutting.mode = super::cutting::TrimMode::Off;
            }
        }
        super::cutting::toolbar(app, bar);
        divider(bar);
        bar.add_enabled_ui(app.doc.can_undo(), |ui| {
            if IconButton::new(Icon::ArrowUUpLeft, "Undo").shortcut("⌘Z").show(ui).clicked() {
                action = Some(Action::Undo);
            }
        });
        bar.add_enabled_ui(app.doc.can_redo(), |ui| {
            if IconButton::new(Icon::ArrowUUpRight, "Redo").shortcut("⇧⌘Z").show(ui).clicked() {
                action = Some(Action::Redo);
            }
        });
        divider(bar);
        if IconButton::new(Icon::Scissors, "Split at playhead").shortcut("S").show(bar).clicked() {
            action = Some(Action::Split);
        }
        bar.add_enabled_ui(!app.editor.selection.is_empty(), |ui| {
            if IconButton::new(Icon::Trash, "Delete selection").shortcut("⌫").show(ui).clicked() {
                action = Some(Action::Delete);
            }
        });
        if IconButton::new(Icon::Flag, "Add marker").shortcut("M").show(bar).clicked() {
            action = Some(Action::Marker);
        }
        divider(bar);
        if IconButton::new(Icon::Magnet, if app.prefs.snapping { "Snapping on" } else { "Snapping off" }).shortcut("N").active(app.prefs.snapping).show(bar).clicked() {
            app.prefs.snapping = !app.prefs.snapping;
        }
        if IconButton::new(Icon::MagnetStraight, if app.prefs.magnetic { "Magnetic main lane on" } else { "Magnetic main lane off" }).shortcut("⇧M").active(app.prefs.magnetic).show(bar).clicked() {
            app.prefs.magnetic = !app.prefs.magnetic;
        }
        if !ctx.egui_wants_keyboard_input() && crate::keymap::hit(&ctx, &app.prefs, "magnetic") {
            app.prefs.magnetic = !app.prefs.magnetic;
        }
        divider(bar);
        // Physics: the 3D scene from the playhead, baked into keys.
        if super::body3d::scene_has_bodies(app) && IconButton::new(Icon::ArrowsOutCardinal, "Simulate physics from the playhead: moving bodies, joints, breaks and knocked characters baked into keys").show(bar).clicked() {
            super::body3d::bake_scene(app, &ctx);
        }
        let graph = super::graph::is_on(&ctx);
        if IconButton::new(Icon::Path, if graph { "Back to the clips" } else { "Graph editor: the selected clip's animation as curves" }).shortcut("⇧G").active(graph).show(bar).clicked()
            || (!ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::G))))
        {
            super::graph::toggle(&ctx);
        }
        let comp = super::comp::is_on(&ctx);
        if IconButton::new(Icon::Intersect, if comp { "Back to the clips" } else { "Compositor: the selected clip's node graph (keys, cleanup, set extension, depth of field)" }).shortcut("⇧C").active(comp).show(bar).clicked()
            || (!ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::C))))
        {
            super::comp::toggle(&ctx);
        }
        if IconButton::new(Icon::Faders, "Sound: mixer, loudness and stems, dialogue and ADR, subtitles and dubs").shortcut("⇧X").show(bar).clicked()
            || (!ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::X))))
        {
            super::mixer::toggle(&ctx);
        }
    });
    if let Some(a) = action {
        run(app, &ctx, a);
    }
}

fn divider(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, 20.0), Sense::hover());
    kit::vline(ui, r.center().x, r.top()..=r.bottom());
}

/// Zoom keeping the time under `anchor_x` fixed.
fn zoom_to(app: &mut StudioApp, pps: f32, anchor_x: Option<f32>, x0: f32) {
    let ed = &mut app.editor;
    let pps = pps.clamp(MIN_PPS, MAX_PPS);
    let ax = anchor_x.unwrap_or(x0);
    let t_at = (ax - x0 + ed.tl_scroll.x) / ed.px_per_sec;
    ed.px_per_sec = pps;
    ed.tl_scroll.x = (t_at * pps - (ax - x0)).max(0.0);
}

fn scroll_and_zoom(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect, view: &View, max_scroll_y: f32, dur: Time) {
    let hovered = ui.rect_contains_pointer(area);
    if !hovered {
        return;
    }
    let (delta, zoom_delta, cmd, shift, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.command, i.modifiers.shift, i.pointer.hover_pos()));
    if cmd && delta.y != 0.0 || zoom_delta != 1.0 {
        let k = if zoom_delta != 1.0 { zoom_delta } else { (delta.y / 200.0).exp() };
        zoom_to(app, app.editor.px_per_sec * k, pointer.map(|p| p.x), view.x0);
        app.editor.auto_fit = false;
        return;
    }
    let ed = &mut app.editor;
    let (mut dx, mut dy) = (delta.x, delta.y);
    if shift && dx == 0.0 {
        dx = dy;
        dy = 0.0;
    }
    if max_scroll_y <= 0.0 && dx == 0.0 {
        // Nothing to scroll vertically: the wheel scrolls time.
        dx = dy;
        dy = 0.0;
    }
    let content_w = (dur as f64 / SEC as f64) as f32 * view.pps + area.width() * 0.5;
    ed.tl_scroll.x = (ed.tl_scroll.x - dx).clamp(0.0, (content_w - area.width()).max(0.0));
    ed.tl_scroll.y = (ed.tl_scroll.y - dy).clamp(0.0, max_scroll_y);
}

fn lanes_bg(app: &StudioApp, p: &Painter, lanes: &[(u32, Rect)], track: Rect, t: &theme::Tokens) {
    for (id, r) in lanes {
        let Some(l) = app.doc.project.lane(*id) else { continue };
        let lr = Rect::from_min_max(egui::pos2(track.left(), r.top()), egui::pos2(track.right(), r.bottom()));
        let _ = l;
        p.rect_filled(lr, 0, t.alt);
    }
}

/// Beat times inside music clips (every half second), used for snapping
/// and drawn as ticks.
fn beats(proj: &Project) -> Vec<Time> {
    let music: Vec<&Lane> = proj.lanes.iter().filter(|l| l.kind == LaneKind::Audio && l.name == "Music").collect();
    let mut out = Vec::new();
    for c in proj.clips.iter().filter(|c| music.iter().any(|l| l.id == c.lane)) {
        let mut tt = c.start + (SEC / 2 - c.src_in % (SEC / 2)) % (SEC / 2);
        while tt < c.end() && out.len() < 4000 {
            out.push(tt);
            tt += SEC / 2;
        }
    }
    out
}

fn snap_targets(app: &StudioApp, exclude: &[u32]) -> Vec<Time> {
    let p = &app.doc.project;
    let mut v = vec![app.editor.playhead, 0];
    for c in &p.clips {
        if !exclude.contains(&c.id) {
            v.push(c.start);
            v.push(c.end());
        }
    }
    v.extend(p.markers.iter().map(|m| m.time));
    if let Some((a, b)) = p.range {
        v.push(a);
        v.push(b);
    }
    v.extend(beats(p));
    v.extend(super::cutting::snap_extras(app, exclude));
    v
}

/// Snap `times` (moving together by the same offset) to the nearest target
/// within 8 px. Returns the correction and the target hit.
fn snap(app: &StudioApp, times: &[Time], exclude: &[u32], pps: f32) -> Option<(Time, Time)> {
    if !app.prefs.snapping {
        return None;
    }
    let targets = snap_targets(app, exclude);
    let thresh = (SNAP_PX / pps * SEC as f32) as Time;
    let mut best: Option<(Time, Time)> = None;
    for &tm in times {
        for &tg in &targets {
            let d = tg - tm;
            if d.abs() <= thresh && best.is_none_or(|b| d.abs() < b.0.abs()) {
                best = Some((d, tg));
            }
        }
    }
    best
}

fn set_snap_line(app: &mut StudioApp, now: f64, hit: Option<Time>) {
    app.editor.snap_line = match (hit, app.editor.snap_line) {
        (Some(h), Some((old, born))) if old == h => Some((h, born)),
        (Some(h), _) => Some((h, now)),
        (None, _) => None,
    };
}

fn lane_at(lanes: &[(u32, Rect)], y: f32) -> Option<u32> {
    lanes.iter().find(|(_, r)| y >= r.top() && y < r.bottom() + LANE_GAP).map(|(id, _)| *id)
}

fn compatible(lane: &Lane, kind: ClipKind) -> bool {
    match lane.kind {
        LaneKind::Audio => kind == ClipKind::Audio,
        LaneKind::Spine => kind == ClipKind::Video,
        LaneKind::Visual => kind != ClipKind::Audio,
    }
}

fn clip_rect(view: &View, lane_r: Rect, c: &Clip) -> Rect {
    let x0 = view.x(c.start);
    let x1 = view.x(c.end());
    Rect::from_min_max(egui::pos2(x0 + 0.5, lane_r.top() + 1.0), egui::pos2((x1 - 0.5).max(x0 + 2.0), lane_r.bottom() - 1.0))
}

/// Draw and interact with clips. Returns true if a clip handled the click.
fn clips(app: &mut StudioApp, ui: &mut egui::Ui, p: &Painter, view: &View, lanes: &[(u32, Rect)], track: Rect) -> bool {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let hover_pos = ctx.input(|i| i.pointer.hover_pos());
    let tool = app.editor.tool;

    // Background interaction (empty space).
    let bg = ui.interact(track, egui::Id::unique("tl-bg"), Sense::click_and_drag());
    let mut handled = false;

    let proj = app.doc.project.clone();
    let (dragging_ids, drag_delta, drag_off): (Vec<u32>, Time, i32) = match &app.editor.tl_drag {
        Some(TlDrag::Move { ids, delta, lane_offset, .. }) => (ids.clone(), *delta, *lane_offset),
        _ => (vec![], 0, 0),
    };
    let lane_order: Vec<u32> = proj.lanes.iter().map(|l| l.id).collect();
    let beat_times = beats(&proj);
    let mut hovered_clip: Option<(u32, f32)> = None;
    // Draw dragged clips last so they sit on top.
    let mut order: Vec<&Clip> = proj.clips.iter().collect();
    order.sort_by_key(|c| dragging_ids.contains(&c.id));
    // Clips too narrow to see or grab at this zoom (a long project zoomed
    // out): drawn as runs per lane, without a widget each; tens of
    // thousands of widgets were most of a frame.
    let mut runs: Vec<(Rect, Color32)> = Vec::new();
    for c in order {
        let lifted = dragging_ids.contains(&c.id);
        // While dragging, the clip is drawn where it would land; a faint
        // outline stays at its old place.
        let shown_lane = if lifted {
            let i = lane_order.iter().position(|x| *x == c.lane).unwrap_or(0) as i32 + drag_off;
            lane_order.get(i.max(0) as usize).copied().unwrap_or(c.lane)
        } else {
            c.lane
        };
        let Some((_, lr)) = lanes.iter().find(|(id, _)| *id == shown_lane) else { continue };
        let Some(lane) = proj.lane(c.lane) else { continue };
        let moved;
        let c = if lifted {
            if let Some((_, olr)) = lanes.iter().find(|(id, _)| *id == c.lane) {
                p.rect_stroke(clip_rect(view, *olr, c), CornerRadius::same(6), Stroke::new(1.0, theme::alpha(t.text_2, 0.5)), egui::StrokeKind::Inside);
            }
            moved = view::Clip { start: c.start + drag_delta, ..c.clone() };
            &moved
        } else {
            c
        };
        let r = clip_rect(view, *lr, c);
        if r.right() < track.left() || r.left() > track.right() || r.bottom() < track.top() || r.top() > track.bottom() {
            continue;
        }
        if r.width() < 3.0 && !lifted && !app.editor.selection.contains(&c.id) {
            let fill = clip_fill(&t, c.kind);
            match runs.last_mut() {
                Some((last, k)) if *k == fill && (last.top() - r.top()).abs() < 0.5 && r.left() <= last.right() + 1.0 && r.right() >= last.left() - 1.0 => *last = last.union(r),
                _ => runs.push((r, fill)),
            }
            continue;
        }
        let id = egui::Id::unique(("clip", c.id));
        let resp = ui.interact(r.intersect(track), id, Sense::click_and_drag());
        let selected = app.editor.selection.contains(&c.id);
        let hov = resp.hovered() && app.editor.tl_drag.is_none();
        let hover_k = kit::hover_t(&ctx, id, hov);
        paint_clip(p, &mut app.thumbs, &proj, c, lane, r, selected, hover_k, lifted, &t, &beat_times);
        super::keys::timeline(app, ui, c, r, track);

        // Edge zones for trimming.
        let edge = |pos: Pos2| -> Option<bool> {
            if tool != Tool::Select || r.width() < 20.0 {
                return None;
            }
            if (pos.x - r.left()).abs() <= 8.0 { Some(true) } else if (pos.x - r.right()).abs() <= 8.0 { Some(false) } else { None }
        };
        if hov {
            if let Some(pos) = hover_pos {
                hovered_clip = Some((c.id, pos.x));
                let cursor = match tool {
                    Tool::Blade => egui::CursorIcon::Crosshair,
                    Tool::Slip => egui::CursorIcon::ResizeHorizontal,
                    Tool::Range => egui::CursorIcon::Text,
                    Tool::Select => if edge(pos).is_some() { egui::CursorIcon::ResizeColumn } else { egui::CursorIcon::Grab },
                };
                ctx.set_cursor_icon(cursor);
                if tool == Tool::Select && !lane.locked {
                    // Trim handles appear on hover: 8 px, brighter fill.
                    for left in [true, false] {
                        let hr = if left { Rect::from_min_size(r.min, Vec2::new(8.0, r.height())) } else { Rect::from_min_max(egui::pos2(r.right() - 8.0, r.top()), r.max) };
                        let near = edge(pos) == Some(left);
                        let cr = if left { CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 } } else { CornerRadius { ne: 6, se: 6, nw: 0, sw: 0 } };
                        p.rect_filled(hr, cr, Color32::from_white_alpha(if near { 120 } else { 55 }));
                        let gx = hr.center().x;
                        p.vline(gx, (hr.center().y - 6.0)..=(hr.center().y + 6.0), Stroke::new(1.5, Color32::from_black_alpha(110)));
                    }
                }
                if tool == Tool::Blade {
                    let tt = view.t(pos.x);
                    let snapped = snap(app, &[tt], &[], view.pps).map_or(tt, |s| s.1);
                    let x = view.x(snapped);
                    p.vline(x, r.y_range(), Stroke::new(1.0, Color32::WHITE));
                }
            }
        }
        if lane.locked {
            if resp.clicked() {
                kit::push(&ctx, format!("{} lane is locked", lane.name), None, Some(Icon::LockSimple));
            }
            continue;
        }

        // Clicks.
        if resp.clicked() {
            handled = true;
            let m = ctx.input(|i| i.modifiers);
            match tool {
                Tool::Blade => {
                    if let Some(pos) = pointer {
                        let tt = view.t(pos.x);
                        let tt = snap(app, &[tt], &[], view.pps).map_or(tt, |s| s.1);
                        let seq = app.doc.seq();
                        app.doc.apply("Blade", m::Edit::Split { seq, at: tt, clips: Some(vec![m::ClipId(c.id as u64)]) });
                    }
                }
                _ => {
                    let ed = &mut app.editor;
                    if m.shift || m.command {
                        if let Some(i) = ed.selection.iter().position(|x| *x == c.id) {
                            ed.selection.remove(i);
                        } else {
                            ed.selection.push(c.id);
                        }
                    } else {
                        ed.selection = vec![c.id];
                    }
                    ed.selected_cut = None;
                }
            }
        }
        if resp.double_clicked() && c.kind == ClipKind::Title {
            app.editor.selection = vec![c.id];
        }
        if resp.drag_started() {
            handled = true;
            // Decide by where the press began, not where the pointer is
            // once the drag threshold is crossed.
            let Some(pos) = ctx.input(|i| i.pointer.press_origin()).or(pointer) else { continue };
            let grab = view.t(pos.x);
            match tool {
                Tool::Select => {
                    if let Some(left) = edge(pos) {
                        app.editor.tl_drag = Some(TlDrag::Trim { id: c.id, left, origin_start: c.start, origin_dur: c.dur, origin_in: c.src_in, applied: 0 });
                    } else if !super::cutting::start_slide(app, c.id, pos.x) {
                        if !app.editor.selection.contains(&c.id) {
                            app.editor.selection = vec![c.id];
                        }
                        let ids: Vec<u32> = app.editor.selection.iter().copied().filter(|id| proj.clip(*id).and_then(|c| proj.lane(c.lane)).is_some_and(|l| !l.locked)).collect();
                        let origin = ids.iter().filter_map(|id| proj.clip(*id)).map(|c| (c.id, c.start, c.lane)).collect();
                        app.editor.tl_drag = Some(TlDrag::Move { ids, grab, origin, delta: 0, lane_offset: 0 });
                    }
                }
                Tool::Slip => app.editor.tl_drag = Some(TlDrag::Slip { id: c.id, origin_in: c.src_in, applied: 0 }),
                Tool::Range => app.editor.tl_drag = Some(TlDrag::Range { from: grab }),
                Tool::Blade => {}
            }
        }
        if resp.secondary_clicked() {
            let at = resp.interact_pointer_pos().unwrap_or(resp.rect.center());
            super::clip_menu::open(app, &ctx, c.id, at);
        }
        // Accessible name, kind, lane, time and selection for screen readers; a clip
        // that gets keyboard focus is selected (a11y.rs).
        crate::a11y::clip_node(app, &resp, c, selected);
    }
    for (r, fill) in runs {
        p.rect_filled(r.intersect(track), CornerRadius::same(2), theme::mix(fill, Color32::BLACK, 0.2));
    }

    // Transitions: bow-tie over the cut.
    for tr in &proj.transitions {
        let (Some(a), Some(b)) = (proj.clip(tr.left), proj.clip(tr.right)) else { continue };
        let Some((_, lr)) = lanes.iter().find(|(id, _)| *id == a.lane) else { continue };
        let cut = view.x(b.start);
        let half = (tr.dur as f64 / SEC as f64) as f32 * view.pps / 2.0;
        let half = half.max(5.0);
        let cy = lr.top() + CLIP_TOP + (lr.height() - CLIP_TOP) / 2.0;
        let h = ((lr.height() - CLIP_TOP) * 0.32).clamp(5.0, 10.0);
        let sel = app.editor.selected_cut == Some(tr.id);
        let fill = if sel { t.select } else { Color32::from_white_alpha(225) };
        let edge = Stroke::new(1.0, Color32::from_black_alpha(90));
        p.add(egui::Shape::convex_polygon(vec![egui::pos2(cut - half, cy - h), egui::pos2(cut, cy), egui::pos2(cut - half, cy + h)], fill, edge));
        p.add(egui::Shape::convex_polygon(vec![egui::pos2(cut + half, cy - h), egui::pos2(cut + half, cy + h), egui::pos2(cut, cy)], fill, edge));
        let tip_r = Rect::from_center_size(egui::pos2(cut, cy), Vec2::new((half * 2.0).max(14.0), (h * 2.0).max(14.0)));
        let resp = ui.interact(tip_r, egui::Id::unique(("transition", tr.id)), Sense::click());
        if resp.clicked() {
            app.editor.selected_cut = Some(tr.id);
            app.editor.selection.clear();
            handled = true;
        }
        if app.editor.tl_drag.is_none() {
            resp.on_hover_text(format!("{} · {}", crate::catalogue::cut_name(&tr.kind), view::format_short(tr.dur)));
        }
    }

    // Background: clear selection, scrub, range.
    if bg.clicked() && !handled {
        app.editor.selection.clear();
        app.editor.selected_cut = None;
    }
    if bg.drag_started() && !handled && app.editor.tl_drag.is_none() {
        if let Some(pos) = pointer {
            // Select tool: drag a box over clips to select them (Shift or
            // Cmd adds to the selection). Range tool: set in/out.
            app.editor.tl_drag = Some(match tool {
                Tool::Range => TlDrag::Range { from: view.t(pos.x) },
                Tool::Select => {
                    let add = ctx.input(|i| i.modifiers.shift || i.modifiers.command);
                    TlDrag::Marquee { origin: ctx.input(|i| i.pointer.press_origin()).unwrap_or(pos), base: if add { app.editor.selection.clone() } else { Vec::new() } }
                }
                _ => TlDrag::Scrub,
            });
        }
    }

    // A scrub never outlives the button: with nothing held it ends.
    if matches!(app.editor.tl_drag, Some(TlDrag::Scrub)) && !ctx.input(|i| i.pointer.any_down()) {
        app.editor.tl_drag = None;
    }

    // Continue drags.
    let released = ctx.input(|i| i.pointer.any_released());
    if let (Some(drag), Some(pos)) = (app.editor.tl_drag.clone(), pointer) {
        let tnow = view.t(pos.x);
        match drag {
            TlDrag::Move { ids, grab, origin, .. } => {
                let mut delta = tnow - grab;
                // Keep everything at or after zero.
                let min_start = origin.iter().map(|o| o.1).min().unwrap_or(0);
                delta = delta.max(-min_start);
                let edges: Vec<Time> = origin.iter().flat_map(|(id, s, _)| {
                    let d = proj.clip(*id).map_or(0, |c| c.dur);
                    [s + delta, s + delta + d]
                }).collect();
                let hit = snap(app, &edges, &ids, view.pps);
                if let Some((corr, _)) = hit {
                    delta += corr;
                }
                set_snap_line(app, now, hit.map(|h| h.1));
                // Vertical: the lane under the pointer, if every moved clip
                // fits a lane of its kind there.
                let target_lane = lane_at(lanes, pos.y);
                let grab_lane = origin.first().map(|o| o.2);
                let lane_ids: Vec<u32> = proj.lanes.iter().map(|l| l.id).collect();
                let li = |id: u32| lane_ids.iter().position(|x| *x == id).unwrap_or(0) as i32;
                let off = match (target_lane, grab_lane) {
                    (Some(tl), Some(gl)) => li(tl) - li(gl),
                    _ => 0,
                };
                let can_shift = origin.iter().all(|(id, _, lane)| {
                    let ni = li(*lane) + off;
                    ni >= 0 && (ni as usize) < lane_ids.len() && proj.clip(*id).is_some_and(|c| proj.lane(lane_ids[ni as usize]).is_some_and(|l| compatible(l, c.kind) && !l.locked))
                });
                let off = if can_shift { off } else { 0 };
                if let Some(TlDrag::Move { delta: d, lane_offset: o, .. }) = &mut app.editor.tl_drag {
                    *d = delta;
                    *o = off;
                }
                if released && (delta != 0 || off != 0) {
                    commit_move(app, &ctx, &proj, &ids, &origin, delta, off, &lane_ids);
                }
            }
            TlDrag::Trim { id, left, origin_start, origin_dur, origin_in, applied } => {
                let c = proj.clip(id).cloned();
                let media_len = c.as_ref().and_then(|c| c.media.and_then(|m| proj.media(m))).filter(|m| m.kind != MediaKind::Image).map(|m| m.dur);
                let min = SEC / 10;
                let mut tt = tnow;
                let hit = snap(app, &[tt], &[id], view.pps);
                if let Some((_, h)) = hit {
                    tt = h;
                }
                set_snap_line(app, now, hit.map(|h| h.1));
                // Total delta of the dragged edge from where it started.
                let want = if left {
                    let mut d = (tt - origin_start).min(origin_dur - min).max(-origin_start);
                    if media_len.is_some() {
                        d = d.max(-origin_in);
                    }
                    d
                } else {
                    let mut end = tt.max(origin_start + min);
                    if let Some(ml) = media_len {
                        end = end.min(origin_start + ml - origin_in);
                    }
                    end - (origin_start + origin_dur)
                };
                let spine = c.as_ref().map(|c| Some(c.lane) == proj.spine()).unwrap_or(false);
                let step = want - applied;
                if step != 0 {
                    let seq = app.doc.seq();
                    let _ = seq;
                    let edit = super::cutting::trim_edit(app, id, left, step, spine);
                    if app.doc.apply_gesture("tl-trim", "Trim", edit) {
                        if let Some(TlDrag::Trim { applied: a, .. }) = &mut app.editor.tl_drag {
                            *a = want;
                        }
                    } else {
                        // Out of media or blocked: stay where we are, quietly.
                        app.doc.error = None;
                    }
                }
            }
            TlDrag::Slip { id, origin_in, applied } => {
                if let Some(c) = proj.clip(id).cloned() {
                    let ml = c.media.and_then(|m| proj.media(m)).map_or(c.dur, |m| m.dur);
                    let grab_dx = ctx.input(|i| i.pointer.press_origin()).map_or(0.0, |o| pos.x - o.x);
                    let shift = (grab_dx / view.pps * SEC as f32) as Time;
                    // Dragging right shows earlier source material.
                    let want = (-shift).clamp(-origin_in, (ml - c.dur - origin_in).max(-origin_in));
                    let step = want - applied;
                    if step != 0 {
                        let seq = app.doc.seq();
                        if app.doc.apply_gesture("tl-slip", "Slip", m::Edit::Slip { seq, clip: m::ClipId(id as u64), delta: step }) {
                            if let Some(TlDrag::Slip { applied: a, .. }) = &mut app.editor.tl_drag {
                                *a = want;
                            }
                        } else {
                            app.doc.error = None;
                        }
                    }
                }
            }
            TlDrag::Range { from } => {
                let mut to = tnow;
                if let Some((_, h)) = snap(app, &[to], &[], view.pps) {
                    to = h;
                }
                let (a, b) = if to < from { (to, from) } else { (from, to) };
                if b - a > SEC / 20 {
                    app.doc.edit_gesture("tl-range", "Set range", |p| p.range = Some((a.max(0), b)));
                }
            }
            TlDrag::Scrub => {
                app.editor.playhead = tnow.max(0);
                app.editor.playing = false;
            }
            TlDrag::Marquee { origin, base } => {
                let rect = Rect::from_two_pos(origin, pos);
                let mut sel = base.clone();
                for c in &proj.clips {
                    let Some((_, lr)) = lanes.iter().find(|(id, _)| *id == c.lane) else { continue };
                    if proj.lane(c.lane).is_some_and(|l| l.locked) {
                        continue;
                    }
                    if clip_rect(view, *lr, c).intersects(rect) && !sel.contains(&c.id) {
                        sel.push(c.id);
                    }
                }
                app.editor.selection = sel;
                app.editor.selected_cut = None;
                let box_p = ui.painter().with_clip_rect(track);
                box_p.rect_filled(rect, 0, theme::alpha(t_accent(ui), 0.10));
                box_p.rect_stroke(rect, 0, Stroke::new(1.0, t_accent(ui)), egui::StrokeKind::Inside);
            }
            TlDrag::LaneHeight { lane, origin } => {
                let dy = ctx.input(|i| i.pointer.press_origin()).map_or(0.0, |o| pos.y - o.y);
                let h = (origin + dy).clamp(32.0, 140.0);
                app.doc.edit_gesture(&format!("lane-h-{lane}"), "Lane height", |p| {
                    if let Some(l) = p.lane_mut(lane) {
                        l.height = h;
                    }
                });
            }
        }
    }
    if released {
        app.editor.tl_drag = None;
        app.editor.snap_line = None;
    }

    // Optional hover-scrub: preview frames without moving the playhead.
    app.editor.hover_time = hover_scrub(app.prefs.hover_scrub, app.editor.tl_drag.is_some(), app.editor.playing, hover_pos.filter(|p| track.contains(*p)).map(|p| view.t(p.x).max(0)));
    let _ = hovered_clip;
    handled
}

/// The time hover-scrub previews: only with it on, while paused and not
/// dragging. During playback the viewer follows the playhead wherever the
/// pointer rests (it used to flip between the two).
fn hover_scrub(enabled: bool, dragging: bool, playing: bool, under_pointer: Option<Time>) -> Option<Time> {
    (enabled && !dragging && !playing).then_some(under_pointer).flatten()
}

/// Send the move the user just finished dragging.
#[allow(clippy::too_many_arguments)]
fn commit_move(app: &mut StudioApp, ctx: &egui::Context, proj: &Project, ids: &[u32], origin: &[(u32, Time, u32)], delta: Time, off: i32, lane_ids: &[u32]) {
    let seq = app.doc.seq();
    let spine = proj.spine();
    let li = |id: u32| lane_ids.iter().position(|x| *x == id).unwrap_or(0) as i32;
    let cids: Vec<m::ClipId> = ids.iter().map(|id| m::ClipId(*id as u64)).collect();
    let all_spine = off == 0 && origin.iter().all(|o| Some(o.2) == spine);
    let ok = if all_spine && app.prefs.magnetic {
        // On the magnetic spine a move is a reorder: drop before the first
        // remaining clip whose middle is right of the dragged clip's middle.
        let mut moved: Vec<&(u32, Time, u32)> = origin.iter().collect();
        moved.sort_by_key(|o| o.1);
        let first = moved[0];
        let dur = proj.clip(first.0).map_or(0, |c| c.dur);
        let centre = first.1 + delta + dur / 2;
        let before = proj
            .clips
            .iter()
            .filter(|c| Some(c.lane) == spine && !ids.contains(&c.id))
            .filter(|c| c.start + c.dur / 2 > centre)
            .min_by_key(|c| c.start)
            .map(|c| m::ClipId(c.id as u64));
        let edits = moved.iter().map(|o| m::Edit::Reorder { seq, clip: m::ClipId(o.0 as u64), before }).collect();
        let e = super::cutting::alone_edit(app, ids, m::Edit::Batch { edits });
        app.doc.apply("Move clip", e)
    } else {
        let lanes: std::collections::HashSet<u32> = origin.iter().map(|o| o.2).collect();
        let to_lane = (off != 0 && lanes.len() == 1).then(|| m::LaneId(lane_ids[(li(origin[0].2) + off) as usize] as u64));
        let mut ok = (off == 0 || to_lane.is_some()) && {
            let e = super::cutting::move_edit(app, cids, delta, to_lane);
            app.doc.apply(if ids.len() == 1 { "Move clip" } else { "Move clips" }, e)
        };
        if !ok {
            // The model refuses overlaps on free lanes; place the clips
            // ourselves, finding room or adding a lane.
            app.doc.error = None;
            let origin = origin.to_vec();
            let lane_ids = lane_ids.to_vec();
            ok = app.doc.edit("Move clip", |p| {
                for (id, s, lane) in &origin {
                    if let Some(c) = p.clip_mut(*id) {
                        c.start = s + delta;
                        c.lane = lane_ids[(li(*lane) + off) as usize];
                    }
                }
                for (id, _, _) in &origin {
                    p.resolve_overlap(*id);
                }
            });
        }
        ok
    };
    if ok {
        kit::push(ctx, crate::i18n::tf(format!("Moved {}", if ids.len() == 1 { "clip".to_owned() } else { format!("{} clips", ids.len()) })), Some("Undo"), Some(Icon::ArrowsOutCardinal));
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_clip(p: &Painter, thumbs: &mut crate::thumbs::Thumbs, proj: &Project, c: &Clip, lane: &Lane, r: Rect, selected: bool, hover: f32, lifted: bool, t: &theme::Tokens, beats: &[Time]) {
    let cr = CornerRadius::same(theme::radius::CLIP);
    let mut fill = clip_fill(t, c.kind);
    let dim = lane.hidden || lane.muted || (c.kind == ClipKind::Audio && !c.audio.enabled);
    if dim {
        fill = theme::mix(fill, t.sunken, 0.55);
    }
    fill = theme::mix(fill, Color32::WHITE, 0.06 * hover);
    if lifted {
        p.add(theme::popover_shadow().as_shape(r, cr));
    }
    p.rect_filled(r, cr, theme::mix(fill, Color32::BLACK, 0.35));
    p.rect_filled(Rect::from_min_max(r.min, egui::pos2(r.right(), r.top() + CLIP_TOP)), CornerRadius { nw: theme::radius::CLIP, ne: theme::radius::CLIP, sw: 0, se: 0 }, fill);
    // Hovered clips lift their pictures 1 px under a brighter top edge.
    let lift = hover.round();
    let body = Rect::from_min_max(egui::pos2(r.left(), r.top() + CLIP_TOP - lift), r.max - Vec2::new(0.0, lift));
    let inner = p.with_clip_rect(r.shrink(1.0).intersect(p.clip_rect()));
    if body.height() > 6.0 {
        match c.kind {
            ClipKind::Video => {
                // Filmstrip: frames at their position in the source.
                let media = c.media.and_then(|m| proj.media(m));
                let th = body.height() - 2.0;
                let tw = (th * 16.0 / 9.0).max(8.0);
                let clip_r = inner.clip_rect();
                let mut x = body.left();
                let start_x = clip_r.left().max(body.left());
                if start_x > x {
                    x += ((start_x - x) / tw).floor() * tw;
                }
                let span = c.dur as f32 / SEC as f32;
                let len = media.map_or(10.0, |m| m.dur as f32 / SEC as f32).max(0.1);
                let mut n = 0;
                while x < body.right().min(clip_r.right()) && n < 64 {
                    let u = ((x - body.left()) / r.width()).clamp(0.0, 1.0);
                    let phase = ((c.src_in as f32 / SEC as f32) + u * span) / len;
                    // Quantise so neighbouring zoom levels share frames.
                    let phase = (phase * 40.0).floor() / 40.0;
                    let rect = Rect::from_min_size(egui::pos2(x, body.top()), Vec2::new(tw, th));
                    let seed = media.map_or(c.id, |m| thumbs.seed_of(m));
                    if let Some(tex) = thumbs.scene(seed, phase, 96, 54) {
                        inner.image(tex, rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), if dim { Color32::from_gray(110) } else { Color32::WHITE });
                    }
                    x += tw;
                    n += 1;
                }
            }
            ClipKind::Audio => {
                let seed = c.media.and_then(|m| proj.media(m)).map_or(c.id, |m| m.seed);
                let mid = body.center().y;
                let amp = body.height() * 0.42 * (10f32.powf(c.audio.volume / 20.0)).sqrt().clamp(0.15, 1.3);
                let clip_r = inner.clip_rect();
                let x0 = clip_r.left().max(body.left()) as i32;
                let x1 = clip_r.right().min(body.right()) as i32;
                let len = c.media.and_then(|m| proj.media(m)).map_or(c.dur, |m| m.dur).max(1) as f32;
                let col = Color32::from_white_alpha(if dim { 90 } else { 175 });
                let mut x = x0 - x0 % 2;
                while x < x1 {
                    let u = (x as f32 - r.left()) / r.width();
                    let src = (c.src_in as f32 + u * c.dur as f32) / len;
                    let mut a = library::waveform_sample(seed, src * 6.0) * amp;
                    // Fades shape the waveform.
                    let tsec = u * c.dur as f32 / SEC as f32;
                    if c.audio.fade_in > 0.0 {
                        a *= (tsec / c.audio.fade_in).min(1.0);
                    }
                    if c.audio.fade_out > 0.0 {
                        a *= ((c.dur as f32 / SEC as f32 - tsec) / c.audio.fade_out).min(1.0);
                    }
                    inner.line_segment([egui::pos2(x as f32, mid - a), egui::pos2(x as f32, mid + a)], Stroke::new(1.0, col));
                    x += 2;
                }
                // Beat ticks on music clips.
                if lane.name == "Music" {
                    for &b in beats.iter().filter(|b| **b >= c.start && **b < c.end()) {
                        let bx = r.left() + ((b - c.start) as f32 / c.dur as f32) * r.width();
                        inner.vline(bx, (r.bottom() - 4.0)..=(r.bottom() - 1.0), Stroke::new(1.0, Color32::from_white_alpha(120)));
                    }
                }
            }
            ClipKind::Title if r.width() > 36.0 => {
                let txt = if c.text.is_empty() { &c.name } else { &c.text };
                kit::paint_text_clipped(&inner, egui::pos2(r.left() + 8.0, body.center().y), Align2::LEFT_CENTER, &format!("“{txt}”"), theme::caption(), Color32::from_white_alpha(220), r.width() - 12.0);
            }
            ClipKind::Title => {}
            ClipKind::Fx => {
                // Diagonal hatching marks an adjustment clip.
                let mut x = body.left() - body.height();
                while x < body.right() {
                    inner.line_segment([egui::pos2(x, body.bottom()), egui::pos2(x + body.height(), body.top())], Stroke::new(1.0, Color32::from_white_alpha(40)));
                    x += 8.0;
                }
                let label = c.effects.first().map(|e| crate::catalogue::effect_name(&e.kind).to_owned()).or_else(|| c.look.as_deref().map(|l| crate::catalogue::look_name(l).to_owned())).unwrap_or_default();
                if r.width() > 36.0 {
                    kit::paint_text_clipped(&inner, egui::pos2(r.left() + 8.0, body.center().y), Align2::LEFT_CENTER, &label, theme::caption(), Color32::from_white_alpha(220), r.width() - 12.0);
                }
            }
        }
    }
    if hover > 0.0 {
        p.hline((r.left() + 6.0)..=(r.right() - 6.0), r.top() + 1.0, Stroke::new(1.0, Color32::from_white_alpha((70.0 * hover) as u8)));
    }
    // Name and duration on one line in the top 18 px.
    let top = Rect::from_min_max(r.min, egui::pos2(r.right(), r.top() + CLIP_TOP));
    if r.width() > 36.0 {
        let dur = view::format_short(c.dur);
        let dur_w = 6.0 * dur.len() as f32 + 6.0;
        let show_dur = r.width() > dur_w + 60.0;
        let mut name_w = r.width() - 12.0 - if show_dur { dur_w } else { 0.0 };
        let mut x = r.left() + 6.0;
        if c.effects.iter().any(|_| true) || c.look.is_some() || c.motion_in.is_some() {
            if c.kind != ClipKind::Fx && r.width() > 60.0 {
                icons::paint(&inner, Rect::from_center_size(egui::pos2(x + 5.0, top.center().y), Vec2::splat(10.0)), Icon::MagicWand, 10.0, t.on_clip);
                x += 14.0;
                name_w -= 14.0;
            }
        }
        kit::paint_text_clipped(&inner, egui::pos2(x, top.center().y + 1.0), Align2::LEFT_CENTER, &c.name, theme::caption(), t.on_clip, name_w);
        if show_dur {
            inner.text(egui::pos2(r.right() - 6.0, top.center().y + 1.0), Align2::RIGHT_CENTER, dur, theme::caption(), t.on_clip);
        }
    }
    // Keyframe marks are drawn (and handled) by `keys`.
    if selected {
        p.rect_stroke(r, cr, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
    } else {
        let _ = t;
    }
}

fn headers(app: &mut StudioApp, ui: &mut egui::Ui, lanes: &[(u32, Rect)], area: Rect, t: &theme::Tokens) {
    let ctx = ui.ctx().clone();
    let clip = Rect::from_min_max(area.min, egui::pos2(area.left() + HEADER_W, area.bottom()));
    let mut hui = ui.new_child(egui::UiBuilder::new().max_rect(clip));
    hui.set_clip_rect(clip.intersect(ui.clip_rect()));
    for (id, r) in lanes {
        let Some(lane) = app.doc.project.lane(*id).cloned() else { continue };
        let hr = Rect::from_min_max(egui::pos2(clip.left(), r.top()), egui::pos2(clip.right(), r.bottom()));
        if hr.bottom() < clip.top() || hr.top() > clip.bottom() {
            continue;
        }
        let p = hui.painter().clone();
        // Four small icons, centred: lane type, lock, visibility, sound.
        let kind_icon = match lane.kind {
            LaneKind::Audio => Icon::MusicNotes,
            LaneKind::Spine => Icon::FilmStrip,
            LaneKind::Visual => Icon::Stack,
        };
        let cy = hr.center().y;
        let x0 = hr.center().x - 1.5 * 22.0;
        let icon_r = |k: f32| Rect::from_center_size(egui::pos2(x0 + k * 22.0, cy), Vec2::splat(20.0));
        icons::paint(&p, icon_r(0.0).shrink(3.0), kind_icon, 13.0, t.text_2);
        let lane_name = lane.name.clone();
        hui.interact(icon_r(0.0), egui::Id::unique(("lane-kind", lane.id)), Sense::hover()).on_hover_text(lane_name);
        let sound_on = lane.kind == LaneKind::Audio || lane.kind == LaneKind::Spine;
        let toggles: [(Icon, &str, bool, u8, bool); 3] = [
            (if lane.locked { Icon::LockSimple } else { Icon::LockSimpleOpen }, if lane.locked { "Unlock lane" } else { "Lock lane" }, lane.locked, 0, true),
            (if lane.hidden { Icon::EyeSlash } else { Icon::Eye }, if lane.hidden { "Show lane" } else { "Hide lane" }, lane.hidden, 1, lane.kind != LaneKind::Audio),
            (if lane.muted { Icon::SpeakerSlash } else { Icon::SpeakerHigh }, if lane.muted { "Unmute lane" } else { "Mute lane" }, lane.muted, 2, sound_on),
        ];
        for (k, (icon, name, on, which, enabled)) in toggles.into_iter().enumerate() {
            let br = icon_r(k as f32 + 1.0);
            if !enabled {
                continue;
            }
            let mut b = IconButton::new(icon, name).size(20.0, 13.0);
            if on {
                b = b.tint(t.warn);
            }
            if b.show_at(&mut hui, br).clicked() {
                let lid = lane.id;
                let label = name.to_owned();
                app.doc.edit(&label, |p| {
                    if let Some(l) = p.lane_mut(lid) {
                        match which {
                            0 => l.locked = !l.locked,
                            1 => l.hidden = !l.hidden,
                            _ => l.muted = !l.muted,
                        }
                    }
                });
            }
        }

        super::cutting::lane_toggles(app, &mut hui, hr, lane.id);

        // Height grip along the bottom edge.
        let grip = Rect::from_min_max(egui::pos2(hr.left(), hr.bottom() - 3.0), egui::pos2(hr.right(), hr.bottom() + LANE_GAP + 1.0));
        let gresp = hui.interact(grip, egui::Id::unique(("lane-grip", lane.id)), Sense::drag()).on_hover_and_drag_cursor(egui::CursorIcon::ResizeRow);
        let k = kit::hover_t(&ctx, gresp.id, gresp.hovered() || gresp.dragged());
        if k > 0.0 {
            p.hline((hr.left() + 24.0)..=(hr.right() - 24.0), hr.bottom() + 1.0, Stroke::new(2.0, theme::alpha(t.accent, k)));
        }
        if gresp.drag_started() {
            app.editor.tl_drag = Some(TlDrag::LaneHeight { lane: lane.id, origin: lane.height });
        }
    }
}

fn corner(app: &StudioApp, ui: &egui::Ui, r: Rect, t: &theme::Tokens) {
    let _ = (app, ui, r, t);
}

fn ruler_ui(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, view: &View, t: &theme::Tokens) {
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    // Tick spacing: the smallest "nice" step at least 72 px apart.
    let steps = [0.1f32, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];
    let step = steps.iter().copied().find(|s| s * view.pps >= 72.0).unwrap_or(600.0);
    let minor = step / 5.0;
    let t0 = (view.t(r.left()) as f64 / SEC as f64).max(0.0) as f32;
    let t1 = view.t(r.right()) as f64 / SEC as f64;
    let mut k = (t0 / minor).floor() as i64;
    loop {
        let ts = k as f32 * minor;
        if ts as f64 > t1 {
            break;
        }
        let x = kit::snap(ui.ctx(), view.x((ts as f64 * SEC as f64) as Time)) + 0.5;
        let major = (k % 5) == 0;
        let h = if major { 8.0 } else { 4.0 };
        p.vline(x, (r.bottom() - h)..=r.bottom(), Stroke::new(1.0, if major { t.text_2 } else { t.line }));
        if major {
            let s = ts.round() as i64;
            let label = if step < 1.0 { format!("{:02}:{:02}.{}", s / 60, s % 60, ((ts.fract() * 10.0).round() as i64) % 10) } else { format!("{:02}:{:02}", s / 60, s % 60) };
            p.vline(x, r.top() + 4.0..=r.bottom(), Stroke::new(1.0, t.text_2));
            p.text(egui::pos2(x + 4.0, r.top() + 11.0), Align2::LEFT_CENTER, label, theme::caption(), t.text_2);
        }
        k += 1;
    }
    kit::hline(ui, r.left()..=r.right(), r.bottom());

    // In/out range in the ruler.
    if let Some((a, b)) = app.doc.project.range {
        let band = Rect::from_min_max(egui::pos2(view.x(a), r.bottom() - 6.0), egui::pos2(view.x(b), r.bottom()));
        p.rect_filled(band, CornerRadius::same(2), theme::alpha(t.accent, 0.35));
        for x in [band.left(), band.right()] {
            p.vline(x, (r.top() + 14.0)..=r.bottom(), Stroke::new(1.5, t.accent));
        }
    }
    // Markers.
    for (i, m) in app.doc.project.markers.clone().iter().enumerate() {
        let x = view.x(m.time);
        let pts = vec![egui::pos2(x - 4.0, r.top() + 14.0), egui::pos2(x + 4.0, r.top() + 14.0), egui::pos2(x + 4.0, r.top() + 20.0), egui::pos2(x, r.top() + 24.0), egui::pos2(x - 4.0, r.top() + 20.0)];
        p.add(egui::Shape::convex_polygon(pts, t.warn, Stroke::NONE));
        let hit = Rect::from_center_size(egui::pos2(x, r.top() + 19.0), Vec2::new(12.0, 14.0));
        let resp = ui.interact(hit, egui::Id::unique(("marker", i)), Sense::click());
        if resp.clicked() {
            app.editor.playhead = m.time;
        }
        resp.on_hover_text(format!("{} · {}", m.note, view::timecode(m.time, app.doc.project.fps)));
    }
    // Review notes.
    super::review::ruler(app, ui, &p, r, |t| view.x(t));

    // Scrub.
    let resp = ui.interact(r, egui::Id::unique("ruler"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    // A click moves the playhead there and lets go; only a drag scrubs.
    // (A click counts on release, after the timeline has already ended
    // drags for this frame: starting a scrub on it left the playhead stuck
    // to the pointer until the next click.)
    if resp.drag_started() {
        app.editor.tl_drag = Some(TlDrag::Scrub);
    }
    if resp.clicked() || (resp.dragged() && matches!(app.editor.tl_drag, Some(TlDrag::Scrub))) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let mut tt = view.t(pos.x).max(0);
            if app.prefs.snapping {
                let targets: Vec<Time> = app.doc.project.clips.iter().flat_map(|c| [c.start, c.end()]).chain(app.doc.project.markers.iter().map(|m| m.time)).collect();
                let thresh = (6.0 / view.pps * SEC as f32) as Time;
                if let Some(s) = targets.iter().filter(|x| (**x - tt).abs() <= thresh).min_by_key(|x| (**x - tt).abs()) {
                    tt = *s;
                }
            }
            app.editor.playhead = tt;
            app.editor.playing = false;
        }
    }
}

fn drop_target(app: &mut StudioApp, ui: &mut egui::Ui, track: Rect, view: &View, lanes: &[(u32, Rect)]) {
    let t = theme::tokens(ui.ctx());
    let resp = ui.interact(track, egui::Id::unique("tl-drop"), Sense::hover());
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else { return };
    let hover = resp.dnd_hover_payload::<LibDrag>();
    let release = resp.dnd_release_payload::<LibDrag>();
    let Some(d) = hover.as_deref().copied().or(release.as_deref().copied()) else { return };
    let proj = &app.doc.project;
    let kind = library::drag_kind(d.rail, &proj.media, d.index);
    let applies_to_clip = matches!(d.rail, super::RailItem::Effects | super::RailItem::Looks | super::RailItem::Motions | super::RailItem::Cuts);
    let lane = lane_at(lanes, pos.y);
    let target_clip = lane.and_then(|l| proj.clips.iter().find(|c| c.lane == l && view.t(pos.x) >= c.start && view.t(pos.x) < c.end())).map(|c| c.id);
    if applies_to_clip {
        if let Some(cid) = target_clip {
            let c = proj.clip(cid).unwrap();
            let (_, lr) = lanes.iter().find(|(id, _)| *id == c.lane).unwrap();
            ui.painter().rect_stroke(clip_rect(view, *lr, c), CornerRadius::same(theme::radius::CLIP), Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
            if release.is_some() {
                let ctx = ui.ctx().clone();
                library::apply_to(app, &ctx, d, cid);
            }
            return;
        }
        // On empty space, effects and looks become an adjustment clip over
        // what they cover: the main-lane clip at that time.
        if !matches!(d.rail, super::RailItem::Effects | super::RailItem::Looks) {
            return;
        }
        let at = view.t(pos.x).max(0);
        let (start, dur) = proj.clips.iter().find(|c| Some(c.lane) == proj.spine() && at >= c.start && at < c.end()).map_or((at, 4 * SEC), |c| (c.start, c.dur));
        let lane = lane.filter(|l| proj.lane(*l).is_some_and(|l| l.kind == LaneKind::Visual && !l.locked));
        let ghost_lane = lane.or_else(|| proj.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.id));
        if let Some((_, lr)) = ghost_lane.and_then(|gl| lanes.iter().find(|(id, _)| *id == gl)) {
            let ghost = Rect::from_min_max(egui::pos2(view.x(start), lr.top() + 1.0), egui::pos2(view.x(start + dur), lr.bottom() - 1.0));
            ui.painter().rect(ghost, CornerRadius::same(theme::radius::CLIP), theme::alpha(t.fx, 0.6), Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        }
        if release.is_some() {
            let ctx = ui.ctx().clone();
            library::add_adjustment(app, &ctx, d, start, dur, lane);
        }
        return;
    }
    // Ghost clip on the lane under the pointer.
    let lane = lane.filter(|l| proj.lane(*l).is_some_and(|l| compatible(l, kind) && !l.locked));
    let dur = library::drag_dur(d.rail, &proj.media, d.index);
    let mut at = view.t(pos.x).max(0);
    if let Some((corr, _)) = snap(app, &[at, at + dur], &[], view.pps) {
        at += corr;
    }
    if let Some(l) = lane {
        let (_, lr) = lanes.iter().find(|(id, _)| *id == l).unwrap();
        let ghost = Rect::from_min_max(egui::pos2(view.x(at), lr.top() + 1.0), egui::pos2(view.x(at + dur), lr.bottom() - 1.0));
        let fill = library::drag_fill(&t, d.rail, &app.doc.project.media, d.index);
        ui.painter().rect(ghost, CornerRadius::same(6), theme::alpha(fill, 0.6), Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        ui.painter().vline(view.x(at), track.y_range(), Stroke::new(1.0, theme::alpha(t.accent, 0.8)));
    }
    if release.is_some() {
        let ctx = ui.ctx().clone();
        library::add(app, &ctx, d.rail, d.index, Some(at), lane);
    }
}

fn t_accent(ui: &egui::Ui) -> Color32 {
    theme::tokens(ui.ctx()).accent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_scrub_waits_while_playing() {
        assert_eq!(hover_scrub(true, false, false, Some(5)), Some(5));
        assert_eq!(hover_scrub(true, false, true, Some(5)), None, "playing: the viewer follows the playhead");
        assert_eq!(hover_scrub(true, true, false, Some(5)), None);
        assert_eq!(hover_scrub(false, false, false, Some(5)), None);
    }
}
