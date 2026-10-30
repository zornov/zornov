// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Timeline (spec §5): toolbar, ruler, lane headers, clips with
//! filmstrips or waveforms, transitions as bow-ties, markers, in/out range,
//! the playhead, snapping and the editing tools.

use super::library::{self, LibDrag};
use super::{Action, TlDrag, Tool, clip_fill, run};
use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, IconButton, MenuItem, Segment};
use crate::view::{self, Clip, ClipKind, Lane, LaneKind, MediaKind, Project, SEC, Time};
use crate::theme;
use studio_model as m;
use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Stroke, Vec2};

pub const HEADER_W: f32 = 160.0;
pub(super) const TOOLBAR_H: f32 = 34.0;
const RULER_H: f32 = 24.0;
const CLIP_TOP: f32 = 18.0;
const SNAP_PX: f32 = 8.0;
// TODO(kit): theme token for the playhead and the selection outline.
const PLAYHEAD: Color32 = Color32::WHITE;
const LANE_GAP: f32 = 3.0;
/// A track holding only 3D cameras and lights: one slim row each, not a full track.
const SCENE_H: f32 = 22.0;
const SCENE_GAP: f32 = 1.0;
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
    let scene = scene_lanes(app);
    let lane_h = |l: &Lane| if scene.contains(&l.id) { (SCENE_H, SCENE_GAP) } else { (l.height, LANE_GAP) };
    let total_h: f32 = app.doc.project.lanes.iter().map(|l| lane_h(l).0 + lane_h(l).1).sum();
    let max_scroll_y = (total_h - lanes_r.height() + 4.0).max(0.0);
    app.editor.tl_scroll.y = app.editor.tl_scroll.y.clamp(0.0, max_scroll_y);
    let mut lane_rects: Vec<(u32, Rect)> = Vec::new();
    let mut y = lanes_r.top() + 4.0 - app.editor.tl_scroll.y;
    for l in &app.doc.project.lanes {
        let (h, gap) = lane_h(l);
        lane_rects.push((l.id, Rect::from_min_size(egui::pos2(lanes_r.left(), y), Vec2::new(lanes_r.width(), h))));
        y += h + gap;
    }

    // The compositor keeps the wheel over its graph for zooming it; the
    // ruler still scrolls time.
    let wheel_area = if super::comp::is_on(&ctx) { ruler } else { track_r.union(ruler) };
    scroll_and_zoom(app, ui, wheel_area, &view, max_scroll_y, proj_dur);
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
        // A line with a small handle in the ruler.
        let top = ruler.top() + 2.0;
        p.vline(px, top + 10.0..=track_r.bottom(), Stroke::new(1.0, PLAYHEAD));
        let h = vec![egui::pos2(px - 5.0, top), egui::pos2(px + 5.0, top), egui::pos2(px + 5.0, top + 7.0), egui::pos2(px, top + 12.0), egui::pos2(px - 5.0, top + 7.0)];
        p.add(egui::Shape::convex_polygon(h, PLAYHEAD, Stroke::NONE));
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
    context_menu(app, &ctx);
}

fn toolbar(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, _panel: Rect) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    let inner = r.shrink2(Vec2::new(10.0, 4.0));
    let mut action = None;
    let key = |id: &str| crate::keymap::hint(&app.prefs, id);
    let keys: std::collections::HashMap<&str, String> = ["fit", "snap", "magnetic", "select", "blade", "slip", "range"].into_iter().map(|id| (id, key(id))).collect();
    let k = |id: &str| keys.get(id).map_or("", String::as_str);
    // Right: zoom (laid out first, so the tools know their room). Undo,
    // Redo, Split, Delete, Marker and the zoom buttons are in the menus
    // (Edit, Clip, View) and on the keys; only Fit stays beside the slider.
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 4.0;
    if IconButton::new(Icon::ArrowsOutLineHorizontal, "Fit sequence").shortcut(k("fit")).show(&mut right).clicked() {
        action = Some(Action::Fit);
    }
    // Log-scale zoom slider.
    let mut z = ((app.editor.px_per_sec / MIN_PPS).ln() / (MAX_PPS / MIN_PPS).ln() * 100.0).clamp(0.0, 100.0);
    let w = if inner.width() > 760.0 { 120.0 } else { 72.0 };
    if kit::Slider::new("Timeline zoom", &mut z, 0.0..=100.0).default(0.0).width(w).show(&mut right).changed() {
        zoom_to(app, MIN_PPS * (MAX_PPS / MIN_PPS).powf(z / 100.0), None, r.left() + HEADER_W + 12.0);
        app.editor.auto_fit = false;
    }
    if inner.width() > 820.0 {
        divider(&mut right);
        let n = app.editor.selection.len();
        let label = if n == 0 { format!("{} clips", app.doc.project.clips.len()) } else { format!("{n} selected") };
        right.label(egui::RichText::new(label).font(theme::caption()).color(t.text_2));
    }
    // Left: the controls in four groups, in the room the zoom leaves; a
    // narrow timeline scrolls them sideways.
    let left = Rect::from_min_max(inner.min, egui::pos2((right.min_rect().left() - 8.0).max(inner.left()), inner.bottom()));
    let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::left_to_right(egui::Align::Center)));
    outer.set_clip_rect(left.intersect(ui.clip_rect()));
    kit::scroll_strip(&mut outer, "timeline-tools", |bar| {
        bar.spacing_mut().item_spacing.x = 4.0;
        // Tools.
        for (tool, icon, name, id) in [(Tool::Select, Icon::Cursor, "Select (drag empty space to box-select)", "select"), (Tool::Blade, Icon::Scissors, "Blade", "blade"), (Tool::Slip, Icon::ArrowsHorizontal, "Slip", "slip"), (Tool::Range, Icon::BracketsSquare, "Range", "range")] {
            if IconButton::new(icon, name).shortcut(k(id)).active(app.editor.tool == tool).show(bar).clicked() {
                app.editor.tool = tool;
                app.editor.cutting.mode = super::cutting::TrimMode::Off;
            }
        }
        divider(bar);
        // Trim: one control that also shows the mode.
        super::cutting::trim_menu(app, bar);
        divider(bar);
        // Toggles.
        if IconButton::new(Icon::Magnet, if app.prefs.snapping { "Snapping on" } else { "Snapping off" }).shortcut(k("snap")).active(app.prefs.snapping).show(bar).clicked() {
            app.prefs.snapping = !app.prefs.snapping;
        }
        if IconButton::new(Icon::MagnetStraight, if app.prefs.magnetic { "Magnetic main track on" } else { "Magnetic main track off" }).shortcut(k("magnetic")).active(app.prefs.magnetic).show(bar).clicked() {
            app.prefs.magnetic = !app.prefs.magnetic;
        }
        if !ctx.egui_wants_keyboard_input() && crate::keymap::hit(&ctx, &app.prefs, "magnetic") {
            app.prefs.magnetic = !app.prefs.magnetic;
        }
        super::cutting::linked_toggle(app, bar);
        divider(bar);
        // What the timeline shows: the clips, their curves or their nodes.
        let mut view = if super::comp::is_on(&ctx) { 2 } else if super::graph::is_on(&ctx) { 1 } else { 0 };
        let was = view;
        let opts = [Segment::text("Clips"), Segment::text("Curves"), Segment::text("Nodes")];
        kit::segmented(bar, "timeline-view", &mut view, &opts, None, None).on_hover_text("Clips, the selected clip's animation curves (⇧G), or its compositor nodes (⇧C)");
        let shortcut = |key| !ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, key)));
        if shortcut(egui::Key::G) {
            view = if was == 1 { 0 } else { 1 };
        }
        if shortcut(egui::Key::C) {
            view = if was == 2 { 0 } else { 2 };
        }
        if view != was {
            super::graph::set_on(&ctx, view == 1);
            super::comp::set_on(&ctx, view == 2);
        }
        divider(bar);
        more_menu(app, bar, &ctx);
        if !ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::X))) {
            super::mixer::toggle(&ctx);
        }
    });
    if let Some(a) = action {
        run(app, &ctx, a);
    }
}

/// The rest of the toolbar's commands, named: Editorial, the source
/// monitor, physics and the sound desk.
fn more_menu(app: &mut StudioApp, bar: &mut egui::Ui, ctx: &egui::Context) {
    let id = bar.scope_id().with("timeline-more");
    let resp = IconButton::new(Icon::DotsThree, "More").active(kit::is_open(ctx, id)).show(bar);
    if resp.clicked() {
        kit::toggle_open(ctx, id);
    }
    enum Pick {
        Editorial,
        Source,
        Physics,
        Sound,
    }
    let mut pick = None;
    let editorial_key = crate::keymap::hint(&app.prefs, "editorial-panel");
    let (editorial, source, bodies) = (app.editor.cutting.panel.open, app.editor.cutting.show_source, super::body3d::scene_has_bodies(app));
    kit::popover(ctx, id, resp.rect, kit::Side::Below, 248.0, |ui| {
        if MenuItem::new("Editorial").icon(Icon::List).shortcut(&editorial_key).checked(editorial).show(ui).clicked() {
            pick = Some(Pick::Editorial);
        }
        if MenuItem::new(if source { "Back to the program" } else { "Source monitor" }).icon(Icon::FilmSlate).shortcut(if source { "⇧4" } else { "⇧2" }).show(ui).clicked() {
            pick = Some(Pick::Source);
        }
        if bodies && MenuItem::new("Simulate physics from the playhead").icon(Icon::ArrowsOutCardinal).show(ui).clicked() {
            pick = Some(Pick::Physics);
        }
        if MenuItem::new("Sound").icon(Icon::Faders).shortcut("⇧X").show(ui).clicked() {
            pick = Some(Pick::Sound);
        }
    });
    if let Some(p) = pick {
        kit::set_open(ctx, id, false);
        match p {
            Pick::Editorial => super::cutting::toggle_editorial(app),
            Pick::Source => super::cutting::toggle_source(app),
            Pick::Physics => super::body3d::bake_scene(app, ctx),
            Pick::Sound => super::mixer::toggle(ctx),
        }
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

/// Tracks that hold only 3D cameras and lights: scene objects, not timed
/// media, so they are drawn as slim rows grouped together (D13).
fn scene_lanes(app: &StudioApp) -> std::collections::HashSet<u32> {
    let model = app.doc.model.active();
    let proj = &app.doc.project;
    proj.lanes
        .iter()
        .filter(|l| l.kind == LaneKind::Visual)
        .filter(|l| {
            let mut mine = proj.clips.iter().filter(|c| c.lane == l.id).peekable();
            mine.peek().is_some() && mine.all(|c| model.clip(m::ClipId(c.id as u64)).is_some_and(|mc| mc.is_camera_or_light()))
        })
        .map(|l| l.id)
        .collect()
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
                    p.vline(x, r.y_range(), Stroke::new(1.0, PLAYHEAD));
                }
            }
        }
        if lane.locked {
            if resp.clicked() {
                kit::push(&ctx, format!("{} track is locked", lane.name), None, Some(Icon::LockSimple));
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
                            // Shift or Cmd keeps what was selected.
                            if ctx.input(|i| i.modifiers.shift || i.modifiers.command) {
                                app.editor.selection.push(c.id);
                            } else {
                                app.editor.selection = vec![c.id];
                            }
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

    // Right-click on empty space: the timeline menu.
    if bg.secondary_clicked() && !handled {
        if let Some(pos) = pointer {
            app.editor.selection.clear();
            app.editor.selected_cut = None;
            open_menu(&ctx, TlMenu::Empty(view.t(pos.x).max(0), lane_at(lanes, pos.y)), pos);
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
                    if app.doc.apply_gesture(&format!("tl-trim-{id}-{origin_start}-{origin_dur}-{left}"), "Trim", edit) {
                        if let Some(TlDrag::Trim { applied: a, .. }) = &mut app.editor.tl_drag {
                            *a = want;
                        }
                    } else {
                        // Out of media or blocked: stay where we are; say why once, on release.
                        note_refused(&ctx, app.doc.error.take());
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
                        if app.doc.apply_gesture(&format!("tl-slip-{id}-{origin_in}"), "Slip", m::Edit::Slip { seq, clip: m::ClipId(id as u64), delta: step }) {
                            if let Some(TlDrag::Slip { applied: a, .. }) = &mut app.editor.tl_drag {
                                *a = want;
                            }
                        } else {
                            note_refused(&ctx, app.doc.error.take());
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
        // The drag's edits are one undo step from here on, whatever has
        // keyboard focus.
        app.doc.end_gesture();
        app.editor.tl_drag = None;
        app.editor.snap_line = None;
        say_refused(&ctx);
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
fn refused_id() -> egui::Id {
    egui::Id::new("tl-refused")
}

/// A trim or slip the model refused mid-drag: the clip stays put, and the
/// reason is shown once when the pointer is released.
fn note_refused(ctx: &egui::Context, why: Option<String>) {
    if let Some(w) = why.filter(|w| !w.is_empty()) {
        ctx.data_mut(|d| d.insert_temp(refused_id(), w));
    }
}

/// On release: say why a trim or slip during the drag was refused, once.
fn say_refused(ctx: &egui::Context) {
    if let Some(why) = ctx.data_mut(|d| d.remove_temp::<String>(refused_id())) {
        kit::push(ctx, why, None, Some(Icon::Warning));
    }
}

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
        super::toast_undo(&mut app.editor, ctx, crate::i18n::tf(format!("Moved {}", if ids.len() == 1 { "clip".to_owned() } else { format!("{} clips", ids.len()) })), Icon::ArrowsOutCardinal);
    }
}

/// How a clip reads its sound's real waveform: which stretch of the file it
/// plays, and the gain and fades that shape it. Pure, so tests can check the
/// numbers without painting.
struct AudioShape {
    /// Where the clip's stretch starts / ends in the file, 0..1.
    from: f32,
    to: f32,
    /// Linear gain from the clip's volume.
    gain: f32,
    fade_in: f32,
    fade_out: f32,
    /// The clip's length in seconds.
    secs: f32,
    /// Fraction of the clip one drawn column covers.
    step: f32,
}

impl AudioShape {
    fn of(c: &Clip, media_dur: view::Time, width: f32) -> Self {
        let len = media_dur.max(1) as f32;
        let speed = if c.speed > 0.0 { c.speed / 100.0 } else { 1.0 };
        let from = (c.src_in as f32 / len).clamp(0.0, 1.0);
        let to = ((c.src_in as f32 + c.dur as f32 * speed) / len).clamp(from, 1.0);
        AudioShape { from, to, gain: 10f32.powf(c.audio.volume / 20.0), fade_in: c.audio.fade_in, fade_out: c.audio.fade_out, secs: c.dur as f32 / SEC as f32, step: 2.0 / width.max(1.0) }
    }

    /// The (peak, body) half-heights 0..1 at `u` (0..1 along the clip): the
    /// real bars under that column, times gain and fades, on the decibel
    /// scale. Fades are in the level, so a fade-in ramps the wave up.
    fn column(&self, w: &crate::services::library::Wave, u: f32) -> (f32, f32) {
        use crate::services::library::wave_height;
        let u = u.clamp(0.0, 1.0);
        let at = |x: f32| self.from + (self.to - self.from) * x;
        let (peak, rms) = w.range(at(u), at((u + self.step).min(1.0)));
        let t = u * self.secs;
        let mut fade = 1.0f32;
        if self.fade_in > 0.0 {
            fade *= (t / self.fade_in).min(1.0);
        }
        if self.fade_out > 0.0 {
            fade *= ((self.secs - t) / self.fade_out).clamp(0.0, 1.0);
        }
        let k = self.gain * fade;
        let (pk, bd) = (wave_height(peak * k), wave_height(rms * k));
        (pk, bd.min(pk))
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
                let media = c.media.and_then(|m| proj.media(m));
                let wave = media.and_then(|m| m.path.as_deref()).and_then(crate::services::library::wave);
                let mid = body.center().y;
                let room = body.height() * 0.46;
                let clip_r = inner.clip_rect();
                let x0 = clip_r.left().max(body.left()) as i32;
                let x1 = clip_r.right().min(body.right()) as i32;
                let ink = Color32::from_white_alpha(if dim { 90 } else { 200 });
                match wave.as_deref() {
                    // Still being read, or no readable sound: a quiet dotted line, never an invented wave.
                    None => {
                        let mut x = (x0 + 4) as f32;
                        while x < (x1 - 4) as f32 {
                            inner.circle_filled(egui::pos2(x, mid), 0.7, Color32::from_white_alpha(70));
                            x += 6.0;
                        }
                    }
                    Some(w) => {
                        let shape = AudioShape::of(c, media.map_or(c.dur, |m| m.dur), r.width());
                        let mut x = x0 - x0 % 2;
                        while x < x1 {
                            let (peak, body_h) = shape.column(w, (x as f32 - r.left()) / r.width());
                            let edge = Rect::from_center_size(egui::pos2(x as f32 + 0.75, mid), Vec2::new(1.5, (peak * room * 2.0).max(1.0)));
                            let core = Rect::from_center_size(edge.center(), Vec2::new(1.5, (body_h * room * 2.0).max(1.0).min(edge.height())));
                            inner.rect_filled(edge, 0, ink.gamma_multiply(0.4));
                            inner.rect_filled(core, 0, ink);
                            x += 2;
                        }
                    }
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
        p.rect_stroke(r, cr, Stroke::new(1.5, PLAYHEAD), egui::StrokeKind::Inside);
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
        let cy = hr.center().y;
        // Target and sync lock sit at the edges (`cutting::lane_toggles`);
        // between them the name, then lock, visibility and sound.
        let name_r = Rect::from_min_max(egui::pos2(hr.left() + 20.0, hr.top()), egui::pos2(hr.left() + 82.0, hr.bottom()));
        let icon_r = |k: f32| Rect::from_center_size(egui::pos2(hr.left() + 92.0 + k * 20.0, cy), Vec2::splat(20.0));
        // Right-click anywhere on the header: the track menu. Added first,
        // so the buttons above it take their own clicks.
        let hresp = hui.interact(hr, egui::Id::unique(("lane-header", lane.id)), Sense::click());
        if hresp.secondary_clicked() {
            let at = hresp.interact_pointer_pos().unwrap_or(hr.center());
            open_menu(&ctx, TlMenu::Track(lane.id), at);
        }
        let renaming = ctx.data(|d| d.get_temp::<(u32, String)>(rename_id())).filter(|r| r.0 == lane.id);
        if let Some((rid, mut text)) = renaming {
            let edit = egui::TextEdit::singleline(&mut text).font(theme::caption()).desired_width(name_r.width());
            let er = hui.put(Rect::from_center_size(name_r.center(), Vec2::new(name_r.width(), 20.0)), edit);
            if !er.has_focus() && !er.lost_focus() {
                er.request_focus();
            }
            let cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if er.lost_focus() || cancel {
                ctx.data_mut(|d| d.remove::<(u32, String)>(rename_id()));
                let name = text.trim().to_owned();
                if !cancel && !name.is_empty() && name != lane.name {
                    app.doc.edit("Rename track", |p| {
                        if let Some(l) = p.lane_mut(rid) {
                            l.name = name.clone();
                        }
                    });
                }
            } else {
                ctx.data_mut(|d| d.insert_temp(rename_id(), (rid, text)));
            }
        } else {
            let col = if lane.hidden || lane.muted { t.text_2 } else { t.text };
            kit::paint_text_clipped(&p, egui::pos2(name_r.left(), cy), Align2::LEFT_CENTER, &lane.name, theme::caption(), col, name_r.width());
            let nresp = hui.interact(name_r, egui::Id::unique(("lane-name", lane.id)), Sense::click()).on_hover_text(format!("{} (double-click to rename)", lane.name));
            if nresp.double_clicked() {
                ctx.data_mut(|d| d.insert_temp(rename_id(), (lane.id, lane.name.clone())));
            }
            // The name sits over the header and takes its clicks, so a right-click
            // on the name opens the track menu from here.
            if nresp.secondary_clicked() {
                open_menu(&ctx, TlMenu::Track(lane.id), nresp.interact_pointer_pos().unwrap_or(name_r.center()));
            }
        }
        let sound_on = lane.kind == LaneKind::Audio || lane.kind == LaneKind::Spine;
        let toggles: [(Icon, &str, bool, LaneToggle, bool); 3] = [
            (if lane.locked { Icon::LockSimple } else { Icon::LockSimpleOpen }, if lane.locked { "Unlock track" } else { "Lock track" }, lane.locked, LaneToggle::Lock, true),
            (if lane.hidden { Icon::EyeSlash } else { Icon::Eye }, if lane.hidden { "Show track" } else { "Hide track" }, lane.hidden, LaneToggle::Hide, lane.kind != LaneKind::Audio),
            (if lane.muted { Icon::SpeakerSlash } else { Icon::SpeakerHigh }, if lane.muted { "Unmute track" } else { "Mute track" }, lane.muted, LaneToggle::Mute, sound_on),
        ];
        for (k, (icon, name, on, which, enabled)) in toggles.into_iter().enumerate() {
            let br = icon_r(k as f32);
            if !enabled {
                continue;
            }
            let mut b = IconButton::new(icon, name).size(20.0, 13.0);
            if on {
                b = b.tint(t.warn);
            }
            if b.show_at(&mut hui, br).clicked() {
                toggle_lane(app, lane.id, which);
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

/// One ruler tick: its frame number, whether it is labelled, and the label.
#[derive(Debug, PartialEq)]
struct RulerTick {
    frame: i64,
    major: bool,
    label: Option<String>,
}

/// Timecode for a whole frame count, `MM:SS:FF` (hours join in once
/// there are any). Counts frames, so labels never run backwards.
fn ruler_label(frame: i64, fps: f32) -> String {
    let fps_i = fps.round().max(1.0) as i64;
    let s = frame / fps_i;
    let f = frame % fps_i;
    if s >= 3600 { format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f) } else { format!("{:02}:{:02}:{:02}", s / 60, s % 60, f) }
}

/// Ticks between `t0` and `t1` seconds: a major step of whole frames (1, 2,
/// 5, 10) or whole seconds, at least 72 px apart, with minor ticks between.
fn ruler_ticks(pps: f32, fps: f32, t0: f64, t1: f64) -> Vec<RulerTick> {
    let fps = fps.max(1.0);
    let fpsi = fps.round().max(1.0) as i64;
    let mut steps: Vec<i64> = vec![1, 2, 5, 10];
    steps.extend([1i64, 2, 5, 10, 15, 30, 60, 120, 300, 600].iter().map(|s| s * fpsi));
    let px_per_frame = pps / fps;
    let step = steps.iter().copied().find(|s| *s as f32 * px_per_frame >= 72.0).unwrap_or(600 * fpsi);
    let subs = if step >= 5 { 5 } else { step };
    let f0 = (t0 * fps as f64).floor().max(0.0) as i64;
    let f1 = (t1 * fps as f64).ceil() as i64;
    let mut out = Vec::new();
    let mut k = f0 * subs / step;
    while out.len() < 2000 {
        let frame = (k * step + subs / 2) / subs;
        if frame > f1 {
            break;
        }
        let major = k % subs == 0;
        out.push(RulerTick { frame, major, label: major.then(|| ruler_label(frame, fps)) });
        k += 1;
    }
    out
}

fn ruler_ui(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, view: &View, t: &theme::Tokens) {
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    let fps = app.doc.project.fps;
    let t0 = (view.t(r.left()) as f64 / SEC as f64).max(0.0);
    let t1 = view.t(r.right()) as f64 / SEC as f64;
    for tick in ruler_ticks(view.pps, fps, t0, t1) {
        let x = kit::snap(ui.ctx(), view.x((tick.frame as f64 / fps.max(1.0) as f64 * SEC as f64).round() as Time)) + 0.5;
        let h = if tick.major { 8.0 } else { 4.0 };
        p.vline(x, (r.bottom() - h)..=r.bottom(), Stroke::new(1.0, if tick.major { t.text_2 } else { t.line }));
        if let Some(label) = tick.label {
            p.vline(x, r.top() + 4.0..=r.bottom(), Stroke::new(1.0, t.text_2));
            p.text(egui::pos2(x + 4.0, r.top() + 11.0), Align2::LEFT_CENTER, label, theme::caption(), t.text_2);
        }
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
    if resp.secondary_clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let on_marker = app.doc.project.markers.iter().find(|m| (view.x(m.time) - pos.x).abs() <= 6.0 && pos.y > r.top() + 10.0).map(|m| m.id);
            let what = on_marker.map_or(TlMenu::Ruler(view.t(pos.x).max(0)), TlMenu::Marker);
            open_menu(ui.ctx(), what, pos);
        }
    }
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

// ---- Context menus: tracks, track headers, the empty timeline, the ruler.

fn rename_id() -> egui::Id {
    egui::Id::unique("tl-rename-track")
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum LaneToggle {
    Lock,
    Hide,
    Mute,
    Solo,
}

fn toggle_lane(app: &mut StudioApp, lid: u32, which: LaneToggle) {
    let label = match which {
        LaneToggle::Lock => "Lock track",
        LaneToggle::Hide => "Hide track",
        LaneToggle::Mute => "Mute track",
        LaneToggle::Solo => "Solo track",
    };
    app.doc.edit(label, |p| {
        if let Some(l) = p.lane_mut(lid) {
            match which {
                LaneToggle::Lock => l.locked = !l.locked,
                LaneToggle::Hide => l.hidden = !l.hidden,
                LaneToggle::Mute => l.muted = !l.muted,
                LaneToggle::Solo => l.solo = !l.solo,
            }
        }
    });
}

/// What a timeline context menu was opened on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum TlMenu {
    /// A track header (or a track's own background).
    Track(u32),
    /// Empty timeline at a time, over a track or below them all.
    Empty(Time, Option<u32>),
    /// The ruler at a time.
    Ruler(Time),
    /// A marker, by id.
    Marker(u64),
}

fn menu_id() -> egui::Id {
    egui::Id::unique("tl-context-menu")
}

fn open_menu(ctx: &egui::Context, what: TlMenu, at: Pos2) {
    ctx.data_mut(|d| d.insert_temp(menu_id().with("what"), (what, at)));
    kit::set_open(ctx, menu_id(), true);
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Do {
    Rename(u32),
    AddTrack(u32, bool),
    DeleteTrack(u32),
    Height(u32, f32),
    Toggle(u32, LaneToggle),
    Target(u32, bool),
    Paste(Time),
    Marker(Time),
    DeleteMarker(u64),
    In(Time),
    Out(Time),
    ClearRange,
    SelectAfter(Time),
}

struct Row {
    label: &'static str,
    icon: Icon,
    enabled: bool,
    checked: Option<bool>,
    danger: bool,
    act: Do,
    /// A thin line above the row.
    gap: bool,
}

fn row(label: &'static str, icon: Icon, act: Do) -> Row {
    Row { label, icon, enabled: true, checked: None, danger: false, act, gap: false }
}

const HEIGHTS: [(&str, f32); 3] = [("Small track", 40.0), ("Medium track", 64.0), ("Large track", 104.0)];

fn track_rows(v: &mut Vec<Row>, id: u32, l: &Lane, targeted: bool) {
    v.push(row("Rename", Icon::TextT, Do::Rename(id)));
    v.push(Row { gap: true, ..row("Add track above", Icon::Stack, Do::AddTrack(id, true)) });
    v.push(row("Add track below", Icon::Stack, Do::AddTrack(id, false)));
    v.push(Row { enabled: l.kind != LaneKind::Spine, danger: true, ..row("Delete track", Icon::Trash, Do::DeleteTrack(id)) });
    for (i, (label, h)) in HEIGHTS.iter().enumerate() {
        v.push(Row { gap: i == 0, checked: Some((l.height - h).abs() < 6.0), ..row(label, Icon::ArrowsOutCardinal, Do::Height(id, *h)) });
    }
    v.push(Row { gap: true, checked: Some(l.locked), ..row("Lock track", Icon::LockSimple, Do::Toggle(id, LaneToggle::Lock)) });
    if l.kind != LaneKind::Audio {
        v.push(Row { checked: Some(l.hidden), ..row("Hide track", Icon::EyeSlash, Do::Toggle(id, LaneToggle::Hide)) });
    }
    if l.kind != LaneKind::Visual {
        v.push(Row { checked: Some(l.muted), ..row("Mute track", Icon::SpeakerSlash, Do::Toggle(id, LaneToggle::Mute)) });
    }
    if l.kind == LaneKind::Audio {
        v.push(Row { checked: Some(l.solo), ..row("Solo track", Icon::SpeakerHigh, Do::Toggle(id, LaneToggle::Solo)) });
    }
    v.push(Row { checked: Some(targeted), ..row("Target track", Icon::Circle, Do::Target(id, !targeted)) });
}

/// The rows of a menu, with the same verbs as the Edit, Clip and View menus
/// and the clip menu ("Add marker", "Set in point", "Paste at playhead"…).
fn menu_rows(what: TlMenu, lane: Option<&Lane>, can_paste: bool, has_range: bool, targeted: bool) -> Vec<Row> {
    let mut v = Vec::new();
    match what {
        TlMenu::Track(id) => {
            let Some(l) = lane else { return v };
            track_rows(&mut v, id, l, targeted);
        }
        TlMenu::Empty(t, over) => {
            v.push(Row { enabled: can_paste, ..row("Paste at playhead", Icon::DownloadSimple, Do::Paste(t)) });
            v.push(row("Add marker", Icon::Flag, Do::Marker(t)));
            v.push(Row { gap: true, ..row("Select clips after here", Icon::Cursor, Do::SelectAfter(t)) });
            if let (Some(id), Some(l)) = (over, lane) {
                let first = v.len();
                track_rows(&mut v, id, l, targeted);
                v[first].gap = true;
            }
        }
        TlMenu::Ruler(t) => {
            v.push(row("Add marker", Icon::Flag, Do::Marker(t)));
            v.push(Row { gap: true, ..row("Set in point", Icon::BracketsSquare, Do::In(t)) });
            v.push(row("Set out point", Icon::BracketsSquare, Do::Out(t)));
            v.push(Row { enabled: has_range, ..row("Clear range", Icon::X, Do::ClearRange) });
        }
        TlMenu::Marker(id) => {
            v.push(Row { danger: true, ..row("Delete marker", Icon::Trash, Do::DeleteMarker(id)) });
        }
    }
    v
}

/// Draws the open timeline menu. Call once a frame.
fn context_menu(app: &mut StudioApp, ctx: &egui::Context) {
    if !kit::is_open(ctx, menu_id()) {
        return;
    }
    let Some((what, at)) = ctx.data(|d| d.get_temp::<(TlMenu, Pos2)>(menu_id().with("what"))) else { return };
    let over = match what {
        TlMenu::Track(id) | TlMenu::Empty(_, Some(id)) => Some(id),
        _ => None,
    };
    let lane = over.and_then(|id| app.doc.project.lane(id).cloned());
    let targeted = over.is_some_and(|id| super::cutting::is_targeted(app, id));
    let rows = menu_rows(what, lane.as_ref(), super::clip_menu::can_paste(ctx), app.doc.project.range.is_some(), targeted);
    if rows.is_empty() {
        kit::set_open(ctx, menu_id(), false);
        return;
    }
    let mut pick = None;
    kit::popover(ctx, menu_id(), Rect::from_min_size(at, Vec2::splat(1.0)), kit::Side::Below, 200.0, |ui| {
        for r in &rows {
            if r.gap {
                kit::menu_separator(ui);
            }
            let mut item = MenuItem::new(r.label).icon(r.icon).enabled(r.enabled);
            if let Some(c) = r.checked {
                item = item.checked(c);
            }
            if r.danger {
                item = item.danger();
            }
            if item.show(ui).clicked() {
                pick = Some(r.act);
            }
        }
    });
    let Some(act) = pick else { return };
    kit::set_open(ctx, menu_id(), false);
    run_menu(app, ctx, act);
}

fn run_menu(app: &mut StudioApp, ctx: &egui::Context, act: Do) {
    let seq = app.doc.seq();
    match act {
        Do::Rename(id) => {
            let name = app.doc.project.lane(id).map(|l| l.name.clone()).unwrap_or_default();
            ctx.data_mut(|d| d.insert_temp(rename_id(), (id, name)));
        }
        Do::AddTrack(id, above) => {
            let lanes = &app.doc.project.lanes;
            let Some(i) = lanes.iter().position(|l| l.id == id) else { return };
            let kind = if lanes[i].kind == LaneKind::Audio { m::LaneKind::Audio } else { m::LaneKind::Visual };
            let name = if kind == m::LaneKind::Audio { "Audio" } else { "Video" };
            let new = m::LaneId(app.doc.alloc_id());
            app.doc.apply("Add track", m::Edit::AddLane { seq, lane: m::Lane::new(new, kind, name), index: Some(if above { i } else { i + 1 }) });
        }
        Do::DeleteTrack(id) => {
            app.doc.apply("Delete track", m::Edit::RemoveLane { seq, lane: m::LaneId(id as u64) });
        }
        Do::Height(id, h) => {
            app.doc.edit("Track height", |p| {
                if let Some(l) = p.lane_mut(id) {
                    l.height = h;
                }
            });
        }
        Do::Toggle(id, which) => toggle_lane(app, id, which),
        Do::Target(id, on) => super::cutting::set_target(app, id, on),
        Do::Paste(t) => super::clip_menu::paste_at(app, ctx, t),
        Do::Marker(t) => {
            app.doc.edit("Add marker", |p| {
                let id = p.alloc_id() as u64;
                p.markers.push(view::Marker { id, time: t, note: "Marker".into() });
            });
        }
        Do::DeleteMarker(id) => {
            app.doc.edit("Delete marker", |p| p.markers.retain(|m| m.id != id));
        }
        Do::In(t) => {
            app.doc.edit("Set in point", |p| {
                let out = p.range.map_or(p.duration(), |r| r.1).max(t + SEC / 10);
                p.range = Some((t, out));
            });
        }
        Do::Out(t) => {
            app.doc.edit("Set out point", |p| {
                let inn = p.range.map_or(0, |r| r.0).min(t - SEC / 10).max(0);
                p.range = Some((inn, t));
            });
        }
        Do::ClearRange => {
            app.doc.edit("Clear range", |p| p.range = None);
        }
        Do::SelectAfter(t) => {
            app.editor.selection = app.doc.project.clips.iter().filter(|c| c.end() > t && app.doc.project.lane(c.lane).is_some_and(|l| !l.locked)).map(|c| c.id).collect();
            app.editor.selected_cut = None;
        }
    }
}

fn t_accent(ui: &egui::Ui) -> Color32 {
    theme::tokens(ui.ctx()).accent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Centre of the first accessibility node with this label.
    fn node_centre(h: &crate::ui_tests::Harness, label: &str) -> Option<Pos2> {
        let update = h.last.as_ref()?.platform_output.accesskit_update.as_ref()?;
        update.nodes.iter().find(|(_, n)| n.label() == Some(label)).and_then(|(_, n)| n.bounds()).map(|b| egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
    }

    fn right_click(h: &mut crate::ui_tests::Harness, at: Pos2) {
        use egui::{Event, Modifiers, PointerButton};
        h.with_events(vec![Event::PointerMoved(at)]);
        h.with_events(vec![Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE }]);
        h.with_events(vec![Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE }]);
        h.frames(2);
    }

    #[test]
    fn right_click_on_a_track_header_opens_the_track_menu() {
        let mut h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        h.frames(4);
        let lock = node_centre(&h, "Lock track").or_else(|| node_centre(&h, "Unlock track")).expect("a track header is on screen");
        // The name sits left of the lock button, on the header.
        let on_header = egui::pos2(lock.x - 50.0, lock.y);
        right_click(&mut h, on_header);
        assert!(kit::is_open(&h.ctx, menu_id()), "the menu opened on the header");
        let what = h.ctx.data(|d| d.get_temp::<(TlMenu, Pos2)>(menu_id().with("what"))).expect("a menu target");
        assert!(matches!(what.0, TlMenu::Track(_)), "it is the track menu, not the empty-timeline one: {:?}", what.0);
        let labels = h.labels();
        for want in ["Rename", "Add track above", "Delete track"] {
            assert!(labels.iter().any(|l| l == want), "{want} is in the menu: {labels:?}");
        }
    }

    fn burst_wave() -> crate::services::library::Wave {
        // 10 s at 2048 samples/s: quiet tone, a loud burst at 6.0–6.5 s.
        let mut x: Vec<f32> = (0..20480).map(|i| (i as f32 * 0.3).sin() * 0.05).collect();
        x[12288..13312].iter_mut().for_each(|v| *v = 0.8);
        crate::services::library::wave_of(&x, &[], 2048)
    }

    fn audio_clip(src_in_s: i64, dur_s: i64) -> Clip {
        let mut c = view::demo().clips[0].clone();
        c.kind = ClipKind::Audio;
        c.src_in = src_in_s * SEC;
        c.dur = dur_s * SEC;
        c.speed = 100.0;
        c.audio = view::AudioParams { enabled: true, volume: 0.0, pan: 0.0, fade_in: 0.0, fade_out: 0.0 };
        c
    }

    #[test]
    fn timeline_audio_reads_the_stretch_the_clip_plays() {
        let w = burst_wave();
        // A clip that plays 5–8 s of a 10 s sound: the burst is a third in.
        let shape = AudioShape::of(&audio_clip(5, 3), 10 * SEC, 300.0);
        let quiet = shape.column(&w, 0.05).0;
        let loud = shape.column(&w, 0.4).0;
        assert!(loud > quiet + 0.2, "the burst shows where it is: {quiet} vs {loud}");
        // A clip that plays 0–3 s never sees the burst.
        let early = AudioShape::of(&audio_clip(0, 3), 10 * SEC, 300.0);
        assert!((0..150).all(|k| early.column(&w, k as f32 / 150.0).0 < loud - 0.2));
        // Faster playback covers more of the file in the same clip length.
        let mut fast = audio_clip(0, 3);
        fast.speed = 200.0;
        assert!((AudioShape::of(&fast, 10 * SEC, 300.0).to - 0.6).abs() < 1e-4);
    }

    #[test]
    fn timeline_audio_gain_and_fades_shape_the_real_peaks() {
        let w = burst_wave();
        let at = |c: &Clip, u: f32| AudioShape::of(c, 10 * SEC, 300.0).column(&w, u).0;
        let flat = audio_clip(5, 3);
        let mut down = flat.clone();
        down.audio.volume = -12.0;
        assert!(at(&down, 0.4) < at(&flat, 0.4) - 0.1, "-12 dB draws lower");
        let mut up = flat.clone();
        up.audio.volume = 6.0;
        assert!(at(&up, 0.4) >= at(&flat, 0.4));
        let mut faded = flat.clone();
        faded.audio.fade_in = 1.5;
        faded.audio.fade_out = 1.5;
        assert_eq!(at(&faded, 0.0), 0.0, "a fade-in starts from nothing");
        assert_eq!(at(&faded, 1.0), 0.0, "a fade-out ends in nothing");
        let mid = AudioShape::of(&faded, 10 * SEC, 300.0).column(&w, 0.5);
        let plain = AudioShape::of(&flat, 10 * SEC, 300.0).column(&w, 0.5);
        assert_eq!(mid, plain, "between the fades the wave is untouched");
        let (pk, body) = AudioShape::of(&flat, 10 * SEC, 300.0).column(&w, 0.4);
        assert!(body <= pk && (0.0..=1.0).contains(&pk));
    }

    #[test]
    fn timeline_audio_without_a_read_sound_has_no_wave_to_draw() {
        // The wave comes only from the cache of real reads: a missing file never gets one.
        assert!(crate::services::library::wave(std::path::Path::new("/no/such/clip.wav")).is_none());
    }

    /// A trim or slip the model refuses leaves the clip alone, keeps its
    /// reason through the drag, and says it once when the pointer is released.
    #[test]
    fn a_refused_trim_or_slip_is_told_once_on_release() {
        let mut doc = crate::doc::Doc::from_view(&crate::view::demo());
        let before = doc.model.clone();
        let seq = doc.seq();
        let gone = m::ClipId(999_999);
        for (key, label, edit) in [
            ("tl-trim-999999", "Trim", m::Edit::Trim { seq, clip: gone, edge: m::Edge::End, delta: crate::view::SEC, ripple: false }),
            ("tl-slip-999999", "Slip", m::Edit::Slip { seq, clip: gone, delta: crate::view::SEC }),
        ] {
            let ctx = egui::Context::default();
            assert!(!doc.apply_gesture(key, label, edit), "{label} was accepted");
            let why = doc.error.clone().unwrap_or_default();
            assert!(!why.is_empty(), "{label}: the model gave no reason");
            // The same hand-off the drag does mid-gesture.
            note_refused(&ctx, doc.error.take());
            assert!(kit::recent(&ctx).is_empty(), "nothing is said while the pointer is down");
            say_refused(&ctx);
            let said = kit::recent(&ctx);
            assert_eq!(said.len(), 1, "{label}: {said:?}");
            assert_eq!(said[0].text, why);
            assert_eq!(said[0].icon, Some(Icon::Warning));
            // Once: a second release says nothing more.
            say_refused(&ctx);
            assert_eq!(kit::recent(&ctx).len(), 1);
        }
        doc.end_gesture();
        assert!(doc.model.content_eq(&before), "a refusal changes nothing");
        assert!(!doc.can_undo(), "a refusal leaves no undo step");
    }

    #[test]
    fn hover_scrub_waits_while_playing() {
        assert_eq!(hover_scrub(true, false, false, Some(5)), Some(5));
        assert_eq!(hover_scrub(true, false, true, Some(5)), None, "playing: the viewer follows the playhead");
        assert_eq!(hover_scrub(true, true, false, Some(5)), None);
        assert_eq!(hover_scrub(false, false, false, Some(5)), None);
    }

    /// Shift-click and Cmd-click add to the selection; a plain click replaces it.
    #[test]
    fn modifier_clicks_add_to_the_selection() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Select", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 3, "width": 200, "height": 100}), serde_json::json!({"op": "card", "colour": "#00ccff", "at": 4, "duration": 3, "width": 200, "height": 100})], "cards", false).unwrap();
        let ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        assert!(ids.len() >= 2);
        app.doc = s.doc;
        let mut frame = eframe::Frame::_new_kittest();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let mut clock = 0.0;
        let mut run = |app: &mut StudioApp, events: Vec<egui::Event>| {
            clock += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(clock), events, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(app, ui, &mut frame));
        };
        for _ in 0..3 {
            run(&mut app, vec![]);
        }
        let centre = |id: u32| ctx.read_response(egui::Id::unique(("clip", id))).expect("the clip was drawn").rect.center();
        let click = |app: &mut StudioApp, run: &mut dyn FnMut(&mut StudioApp, Vec<egui::Event>), at: egui::Pos2, mods: egui::Modifiers| {
            let b = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: mods };
            run(app, vec![egui::Event::ModifiersChanged(mods), egui::Event::PointerMoved(at)]);
            run(app, vec![b(true)]);
            run(app, vec![b(false)]);
            run(app, vec![]);
            run(app, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        };
        click(&mut app, &mut run, centre(ids[0]), egui::Modifiers::NONE);
        assert_eq!(app.editor.selection, vec![ids[0]]);
        click(&mut app, &mut run, centre(ids[1]), egui::Modifiers::SHIFT);
        assert_eq!(app.editor.selection, vec![ids[0], ids[1]], "Shift-click adds");
        click(&mut app, &mut run, centre(ids[1]), egui::Modifiers::COMMAND);
        assert_eq!(app.editor.selection, vec![ids[0]], "Cmd-click on a selected clip takes it out");
        click(&mut app, &mut run, centre(ids[1]), egui::Modifiers::COMMAND);
        assert_eq!(app.editor.selection, vec![ids[0], ids[1]], "Cmd-click adds");
        click(&mut app, &mut run, centre(ids[1]), egui::Modifiers::NONE);
        assert_eq!(app.editor.selection, vec![ids[1]], "a plain click replaces");
    }

    fn lane(kind: LaneKind) -> Lane {
        Lane { id: 7, name: "Titles".into(), kind, locked: false, hidden: false, muted: false, solo: false, height: 64.0, colour: Color32::GRAY }
    }

    #[test]
    fn track_menu_offers_what_the_track_can_do() {
        let labels = |kind| menu_rows(TlMenu::Track(7), Some(&lane(kind)), false, false, false).iter().map(|r| r.label).collect::<Vec<_>>();
        let video = labels(LaneKind::Visual);
        assert!(video.contains(&"Rename") && video.contains(&"Add track above") && video.contains(&"Add track below") && video.contains(&"Delete track"));
        assert!(video.contains(&"Lock track") && video.contains(&"Hide track") && !video.contains(&"Mute track") && !video.contains(&"Solo track"));
        let audio = labels(LaneKind::Audio);
        assert!(audio.contains(&"Mute track") && audio.contains(&"Solo track") && !audio.contains(&"Hide track"));
        assert!(video.contains(&"Small track") && video.contains(&"Medium track") && video.contains(&"Large track"));
        let spine = menu_rows(TlMenu::Track(7), Some(&lane(LaneKind::Spine)), false, false, false);
        assert!(!spine.iter().find(|r| r.label == "Delete track").unwrap().enabled, "the main track stays");
    }

    #[test]
    fn empty_timeline_and_ruler_menus_use_the_menu_verbs() {
        let empty = menu_rows(TlMenu::Empty(5, None), None, true, false, false);
        let names: Vec<&str> = empty.iter().map(|r| r.label).collect();
        assert_eq!(names, ["Paste at playhead", "Add marker", "Select clips after here"]);
        assert!(empty[0].enabled, "paste is on once something was copied");
        assert!(!menu_rows(TlMenu::Empty(5, None), None, false, false, false)[0].enabled);
        let ruler = menu_rows(TlMenu::Ruler(5), None, false, true, false);
        let names: Vec<&str> = ruler.iter().map(|r| r.label).collect();
        assert_eq!(names, ["Add marker", "Set in point", "Set out point", "Clear range"]);
        assert!(!menu_rows(TlMenu::Ruler(5), None, false, false, false)[3].enabled, "nothing to clear");
        // Over a track the track rows follow.
        let over = menu_rows(TlMenu::Empty(5, Some(7)), Some(&lane(LaneKind::Visual)), false, false, false);
        assert!(over.iter().any(|r| r.label == "Rename") && over.iter().any(|r| r.label == "Add marker"));
    }

    #[test]
    fn ruler_labels_never_run_backwards() {
        for fps in [24.0f32, 25.0, 30.0, 60.0] {
            for pps in [2.0f32, 10.0, 60.0, 200.0, 600.0] {
                let ticks = ruler_ticks(pps, fps, 0.0, 6.0);
                assert!(ticks.len() < 2000, "bounded output");
                assert!(ticks.windows(2).all(|w| w[0].frame < w[1].frame), "frames increase at {fps} fps, {pps} px/s");
                let labelled: Vec<&RulerTick> = ticks.iter().filter(|t| t.label.is_some()).collect();
                assert!(labelled.windows(2).all(|w| w[0].label < w[1].label), "labels increase at {fps} fps, {pps} px/s");
            }
        }
    }

    #[test]
    fn ruler_is_frame_accurate_when_zoomed_in() {
        // 24 fps at 600 px/s: 25 px a frame. Every frame ticks, every fifth
        // is labelled, and the labels count frames.
        let ticks = ruler_ticks(600.0, 24.0, 1.0, 2.0);
        assert!(ticks.len() >= 24, "a tick per frame: {}", ticks.len());
        let labels: Vec<&str> = ticks.iter().filter_map(|t| t.label.as_deref()).collect();
        assert_eq!(labels[..3], ["00:01:01", "00:01:06", "00:01:11"]);
        // The old rounding printed 00:01.5 for half a second.
        assert_eq!(ruler_label(12, 24.0), "00:00:12");
        assert_eq!(ruler_label(36, 24.0), "00:01:12");
        assert_eq!(ruler_label(3600 * 24 + 5, 24.0), "01:00:00:05");
    }

    /// The Move toast's Undo button is the toast the move pushed, and it
    /// takes the move back.
    #[test]
    fn the_move_toast_undoes_the_move() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Move", 320, 180, 24.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 2.0, "duration": 2.0, "lane": "overlay", "width": 100, "height": 100})], "card", false).unwrap();
        app.doc = s.doc;
        let proj = app.doc.project.clone();
        let c = proj.clips.iter().find(|c| c.name.contains("card")).unwrap();
        let (id, start, lane) = (c.id, c.start, c.lane);
        let lane_ids: Vec<u32> = proj.lanes.iter().map(|l| l.id).collect();
        commit_move(&mut app, &ctx, &proj, &[id], &[(id, start, lane)], SEC, 0, &lane_ids);
        assert_eq!(app.doc.project.clip(id).unwrap().start, start + SEC);
        let toast = kit::recent(&ctx).into_iter().next().unwrap();
        assert_eq!((toast.text.as_str(), toast.action.as_deref()), ("Moved clip", Some("Undo")));
        assert_eq!(app.editor.undo_toast, Some(toast.id));
        assert!(app.doc.undo().is_some());
        assert_eq!(app.doc.project.clip(id).unwrap().start, start);
    }
}
