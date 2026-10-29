// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Screen reader and keyboard support beyond what the shared widgets give.
//!
//! * [`announce`] puts a sentence in a live region, so a screen reader
//!   (through egui's AccessKit support) speaks it. Toasts, undo and the
//!   keyboard navigation below use it.
//! * Timeline clips get a name, kind, lane, time and selection state
//!   ([`describe_clip`], [`clip_node`]); a clip that receives keyboard focus
//!   is selected and the playhead moves to it.
//! * [`landmark`] marks the main regions (Library, Viewer, Inspector,
//!   Timeline); F6 and Shift+F6 move between them.
//! * [`keys`] adds the keyboard commands for working without a pointer.
//!
//! Keyboard map (also in Help ▸ Keyboard):
//! `[` / `]` previous / next clip · `↑` / `↓` previous / next edit point ·
//! `⌥↑` / `⌥↓` clip on the lane above / below · `⌥←` / `⌥→` nudge the
//! selection one frame (`⇧` one second) · `⌥.` say where you are ·
//! `F6` / `⇧F6` next / previous region · `F1` help · `⌥1`–`⌥6` workspaces.

use crate::app::{Overlay, StudioApp};
use crate::i18n::{tr_args, Value};
use crate::view::{self, Clip, ClipKind, Lane, Project, SEC, Time};
use eframe::egui::{self, accesskit, Context, Id, Rect, Response, Role, Sense, Ui, Vec2, WidgetInfo};
use studio_model as m;

// --- announcements -----------------------------------------------------------

#[derive(Clone, Default)]
struct Say {
    text: String,
    seq: u64,
    on: bool,
}

fn say_id() -> Id {
    Id::new("a11y-say")
}

/// Turn spoken updates on or off (the "Spoken updates" setting).
pub fn set_enabled(ctx: &Context, on: bool) {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(say_id(), Say::default).on = on);
}

/// Have a screen reader say `text` (politely: after what it is saying).
pub fn announce(ctx: &Context, text: impl Into<String>) {
    let text = text.into();
    ctx.data_mut(|d| {
        let s = d.get_temp_mut_or_insert_with(say_id(), || Say { on: true, ..Say::default() });
        if s.on {
            s.text = text;
            s.seq += 1;
        }
    });
    ctx.request_repaint();
}

/// The last thing announced (for tests and the status line).
pub fn last_announcement(ctx: &Context) -> String {
    ctx.data(|d| d.get_temp::<Say>(say_id()).map(|s| s.text).unwrap_or_default())
}

/// Draw the live region. Call once per frame, last.
pub fn live_region(ui: &mut Ui) {
    let say = ui.ctx().data(|d| d.get_temp::<Say>(say_id())).unwrap_or_default();
    // A 1 px widget in the corner: invisible, but in the accessibility tree.
    let r = Rect::from_min_size(ui.max_rect().left_top(), Vec2::splat(1.0));
    let resp = ui.interact(r, Id::new("a11y-live-region"), Sense::hover());
    // Identical messages in a row must still be spoken: alternate a
    // zero-width mark so the label changes.
    let label = if say.seq % 2 == 1 { format!("{}\u{200B}", say.text) } else { say.text.clone() };
    ui.ctx().accesskit_node_builder(resp.id, |b| {
        b.set_role(Role::Label);
        b.set_label(label.as_str());
        b.set_live(accesskit::Live::Polite);
    });
}

// --- describing the edit ---------------------------------------------------------

fn kind_id(k: ClipKind) -> &'static str {
    match k {
        ClipKind::Video => "kind-video",
        ClipKind::Audio => "kind-audio",
        ClipKind::Title => "kind-title",
        ClipKind::Fx => "kind-effect",
    }
}

/// `1.5` seconds, `12` seconds.
pub fn seconds(t: Time) -> String {
    let s = t as f64 / SEC as f64;
    if (s - s.round()).abs() < 0.05 { format!("{}", s.round() as i64) } else { format!("{s:.1}") }
}

/// One sentence about a clip, for a screen reader.
pub fn describe_clip(c: &Clip, lane: Option<&Lane>, fps: f32, selected: bool) -> String {
    let name = if c.name.is_empty() { crate::i18n::tr(kind_id(c.kind)).into_owned() } else { c.name.clone() };
    let args: [(&str, Value); 6] = [
        ("name", name.into()),
        ("kind", crate::i18n::tr(kind_id(c.kind)).into_owned().into()),
        ("lane", lane.map(|l| crate::i18n::t(&l.name).into_owned()).unwrap_or_default().into()),
        ("start", view::timecode(c.start, fps).into()),
        ("dur", seconds(c.dur).into()),
        ("selected", if selected { "yes" } else { "no" }.into()),
    ];
    tr_args("a11y-clip", &args).into_owned()
}

/// Is keyboard focus on a timeline clip?
pub fn clip_has_focus(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<Id>(Id::new("a11y-clip-focus"))).is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
}

/// Give a timeline clip's widget its accessible name, and select it when
/// it gains keyboard focus.
pub fn clip_node(app: &mut StudioApp, resp: &Response, c: &Clip, selected: bool) {
    let lane = app.doc.project.lane(c.lane);
    let text = describe_clip(c, lane, app.doc.project.fps, selected);
    resp.widget_info(|| WidgetInfo::labeled(Role::ListBoxOption, true, text.clone()));
    // Selection as the platform understands it (not a toggle).
    resp.ctx.accesskit_node_builder(resp.id, |b| b.set_selected(selected));
    if resp.has_focus() {
        resp.ctx.data_mut(|d| d.insert_temp(Id::new("a11y-clip-focus"), resp.id));
    }
    if resp.gained_focus() && !resp.clicked() && !selected {
        app.editor.selection = vec![c.id];
        app.editor.playhead = c.start;
        announce(&resp.ctx, text);
    }
}

/// A one-pixel focus stop that names a region; F6 moves between them.
pub fn landmark(ui: &mut Ui, region: Region, rect: Rect) {
    let id = region.id();
    crate::onboarding::note_target(ui.ctx(), region.name(), rect);
    ui.ctx().data_mut(|d| d.get_temp_mut_or_insert_with(visible_id(true), Vec::<Region>::new).push(region));
    let r = Rect::from_min_size(rect.left_top(), Vec2::splat(1.0));
    let resp = ui.interact(r, id, Sense::focusable_noninteractive());
    let label = crate::i18n::t(region.name()).into_owned();
    ui.ctx().accesskit_node_builder(id, |b| {
        b.set_role(Role::Region);
        b.set_label(label.as_str());
    });
    if resp.has_focus() {
        let t = crate::theme::tokens(ui.ctx());
        ui.painter().rect_stroke(rect.shrink(1.0), 6, egui::Stroke::new(if crate::access::high_contrast() { 3.0 } else { 2.0 }, t.accent), egui::StrokeKind::Inside);
    }
    if resp.gained_focus() {
        announce(ui.ctx(), label);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Library,
    Viewer,
    Inspector,
    Timeline,
    // Panels that can be docked beside the four above.
    Sound,
    Scopes,
    Review,
    Record,
    Editorial,
    Ai,
}

impl Region {
    pub const ALL: [Region; 10] = [Region::Library, Region::Viewer, Region::Inspector, Region::Timeline, Region::Sound, Region::Scopes, Region::Review, Region::Record, Region::Editorial, Region::Ai];
    pub fn name(self) -> &'static str {
        match self {
            Region::Library => "Library",
            Region::Viewer => "Viewer",
            Region::Inspector => "Inspector",
            Region::Timeline => "Timeline",
            Region::Sound => "Sound",
            Region::Scopes => "Scopes",
            Region::Review => "Review",
            Region::Record => "Record",
            Region::Editorial => "Editorial",
            Region::Ai => "AI editor",
        }
    }
    fn id(self) -> Id {
        Id::new(("a11y-region", self.name()))
    }
}

/// The region that has keyboard focus, if one does.
pub fn focused_region(ctx: &Context) -> Option<Region> {
    Region::ALL.into_iter().find(|r| ctx.memory(|m| m.has_focus(r.id())))
}

/// Move keyboard focus to the next (or previous) region.
pub fn cycle_region(ctx: &Context, reverse: bool) {
    // Only the regions that were on screen last frame (a closed panel has none).
    let regions: Vec<Region> = ctx.data(|d| d.get_temp::<Vec<Region>>(visible_id(false))).unwrap_or_default();
    let regions = if regions.is_empty() { Region::ALL.to_vec() } else { regions };
    let cur = regions.iter().position(|r| ctx.memory(|m| m.has_focus(r.id())));
    let n = regions.len();
    let next = match (cur, reverse) {
        (None, false) => 0,
        (None, true) => n - 1,
        (Some(i), false) => (i + 1) % n,
        (Some(i), true) => (i + n - 1) % n,
    };
    ctx.memory_mut(|m| m.request_focus(regions[next].id()));
}

fn visible_id(current: bool) -> Id {
    Id::new(if current { "a11y-regions-now" } else { "a11y-regions-prev" })
}

/// Start a frame of region bookkeeping: what was registered becomes what F6 cycles.
pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|d| {
        let now: Vec<Region> = d.get_temp(visible_id(true)).unwrap_or_default();
        d.insert_temp(visible_id(false), now);
        d.insert_temp(visible_id(true), Vec::<Region>::new());
    });
}

// --- navigating the edit (pure, tested) -------------------------------------------

/// Clips ordered by start (then lane order).
fn ordered(p: &Project) -> Vec<&Clip> {
    let lane_ix = |id: u32| p.lanes.iter().position(|l| l.id == id).unwrap_or(usize::MAX);
    let mut v: Vec<&Clip> = p.clips.iter().filter(|c| p.lane(c.lane).is_none_or(|l| !l.hidden)).collect();
    v.sort_by_key(|c| (c.start, lane_ix(c.lane), c.id));
    v
}

/// The clip after (or before) `from` on its lane; with no `from`, the first
/// clip at or after the playhead on the main lane.
pub fn step_clip(p: &Project, from: Option<u32>, playhead: Time, forward: bool) -> Option<u32> {
    let all = ordered(p);
    let lane = match from.and_then(|id| p.clip(id)) {
        Some(c) => c.lane,
        None => p.spine().or_else(|| all.first().map(|c| c.lane))?,
    };
    let on_lane: Vec<&&Clip> = all.iter().filter(|c| c.lane == lane).collect();
    let pos = match from.and_then(|id| on_lane.iter().position(|c| c.id == id)) {
        Some(i) => {
            if forward { i.checked_add(1).filter(|&j| j < on_lane.len()) } else { i.checked_sub(1) }
        }
        None => {
            if forward {
                on_lane.iter().position(|c| c.start >= playhead).or(on_lane.len().checked_sub(1))
            } else {
                on_lane.iter().rposition(|c| c.start < playhead).or(Some(0)).filter(|_| !on_lane.is_empty())
            }
        }
    };
    pos.map(|i| on_lane[i].id)
}

/// The next (previous) cut or clip edge after (before) `t`.
pub fn step_edit(p: &Project, t: Time, forward: bool) -> Option<Time> {
    let mut pts: Vec<Time> = p.clips.iter().flat_map(|c| [c.start, c.end()]).collect();
    pts.sort_unstable();
    pts.dedup();
    if forward { pts.into_iter().find(|&x| x > t) } else { pts.into_iter().rev().find(|&x| x < t) }
}

/// The clip on the lane above/below `from`'s lane that covers `t` (or is
/// nearest to it).
pub fn step_lane(p: &Project, from: Option<u32>, t: Time, down: bool) -> Option<u32> {
    let cur_lane = from.and_then(|id| p.clip(id)).map(|c| c.lane).or_else(|| p.spine())?;
    let ix = p.lanes.iter().position(|l| l.id == cur_lane)?;
    let mut order: Vec<usize> = if down { (ix + 1..p.lanes.len()).collect() } else { (0..ix).rev().collect() };
    order.retain(|&i| !p.lanes[i].hidden);
    for i in order {
        let lane = p.lanes[i].id;
        let mut cs: Vec<&Clip> = p.clips.iter().filter(|c| c.lane == lane).collect();
        if cs.is_empty() {
            continue;
        }
        cs.sort_by_key(|c| (c.start <= t && t < c.end()).then_some(0).unwrap_or(1) as i64 * i64::MAX / 4 + (c.start - t).abs().min(i64::MAX / 8));
        return Some(cs[0].id);
    }
    None
}

// --- keyboard ------------------------------------------------------------------------

/// Extra keyboard commands. Called after the editor's own shortcuts, with
/// the same gating.
pub fn keys(app: &mut StudioApp, ctx: &Context) {
    use egui::{Key, KeyboardShortcut as S, Modifiers as M};
    if app.overlay != Overlay::None || crate::onboarding::tour_running(app) {
        return;
    }
    // F6 works even from a text field: it is the way out of one.
    // Shift first: a plain shortcut also matches the shifted key.
    let sf6 = ctx.input_mut(|i| i.consume_shortcut(&S::new(M::SHIFT, Key::F6)));
    let f6 = !sf6 && ctx.input_mut(|i| i.consume_shortcut(&S::new(M::NONE, Key::F6)));
    if f6 || sf6 {
        cycle_region(ctx, sf6);
    }
    // Only a text field keeps these keys; a focused clip or region must not.
    if ctx.text_edit_focused() {
        return;
    }
    canvas_keys(app, ctx);
    let pressed = |m: M, k: Key| ctx.input_mut(|i| i.consume_shortcut(&S::new(m, k)));
    let fps = app.doc.project.fps;
    let frame = app.editor.frame_dur(fps).max(1);

    let sel_one = if app.editor.selection.len() == 1 { Some(app.editor.selection[0]) } else { app.editor.selection.first().copied() };
    // With a trim point set, `[` and `]` are the trim keys (cutting.rs); they step clips only otherwise.
    let trimming = app.editor.cutting.point.is_some();
    for (key, forward) in [(Key::CloseBracket, true), (Key::OpenBracket, false)] {
        if !trimming && pressed(M::NONE, key) {
            if let Some(id) = step_clip(&app.doc.project, sel_one, app.editor.playhead, forward) {
                select_and_say(app, ctx, id);
            } else {
                announce(ctx, crate::i18n::tr(if forward { "a11y-no-next-clip" } else { "a11y-no-previous-clip" }));
            }
        }
    }
    for (key, down) in [(Key::ArrowDown, true), (Key::ArrowUp, false)] {
        if pressed(M::ALT, key) {
            match step_lane(&app.doc.project, sel_one, app.editor.playhead, down) {
                Some(id) => select_and_say(app, ctx, id),
                None => announce(ctx, crate::i18n::tr("a11y-no-more-lanes")),
            }
        }
    }
    for (key, dir) in [(Key::ArrowRight, 1i64), (Key::ArrowLeft, -1i64)] {
        for (mods, step, unit) in [(M::ALT | M::SHIFT, SEC, "second"), (M::ALT, frame, "frame")] {
            if pressed(mods, key) {
                nudge(app, ctx, dir * step, unit);
            }
        }
    }
    // Plain Up and Down, after the Alt combinations (a plain shortcut also matches them).
    for (key, forward) in [(Key::ArrowDown, true), (Key::ArrowUp, false)] {
        if pressed(M::NONE, key) {
            match step_edit(&app.doc.project, app.editor.playhead, forward) {
                Some(t) => {
                    app.editor.playhead = t;
                    announce(ctx, tr_args("a11y-edit-point", &[("time", view::timecode(t, fps).into())]).into_owned());
                }
                None => announce(ctx, crate::i18n::tr(if forward { "a11y-no-next-edit" } else { "a11y-no-previous-edit" })),
            }
        }
    }
    if pressed(M::ALT, Key::Period) {
        let text = status_sentence(app);
        announce(ctx, text);
    }
}

fn select_and_say(app: &mut StudioApp, ctx: &Context, id: u32) {
    let Some(c) = app.doc.project.clip(id).cloned() else { return };
    app.editor.selection = vec![id];
    app.editor.playhead = c.start;
    let text = describe_clip(&c, app.doc.project.lane(c.lane), app.doc.project.fps, true);
    announce(ctx, text);
}

/// Move the selected clips by `delta` (clamped so none starts before 0).
fn nudge(app: &mut StudioApp, ctx: &Context, delta: Time, unit: &str) {
    if app.editor.selection.is_empty() {
        announce(ctx, crate::i18n::tr("a11y-select-first"));
        return;
    }
    let earliest = app.editor.selection.iter().filter_map(|id| app.doc.project.clip(*id)).map(|c| c.start).min().unwrap_or(0);
    let delta = delta.max(-earliest);
    if delta == 0 {
        announce(ctx, crate::i18n::tr("a11y-at-start"));
        return;
    }
    let seq = app.doc.seq();
    let clips = app.editor.selection.iter().map(|id| m::ClipId(*id as u64)).collect();
    if app.doc.apply("Nudge", m::Edit::Move { seq, clips, delta, to_lane: None }) {
        let id = format!("a11y-nudged-{}-{}", if delta > 0 { "later" } else { "earlier" }, if unit == "second" { "seconds" } else { "frames" });
        let n = if unit == "second" { delta.abs() / SEC } else { delta.abs() / app.editor.frame_dur(app.doc.project.fps).max(1) };
        announce(ctx, tr_args(&id, &[("n", n.into())]).into_owned());
    }
}

/// Where the playhead is and what is selected, as a sentence.
pub fn status_sentence(app: &StudioApp) -> String {
    let p = &app.doc.project;
    let tc = view::timecode(app.editor.playhead, p.fps);
    let sel = app.editor.selection.len();
    tr_args(
        "a11y-status",
        &[("time", tc.into()), ("selected", sel.into()), ("clips", p.clips.len().into()), ("playing", if app.editor.playing { "yes" } else { "no" }.into())],
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Clip, Project};

    fn project() -> Project {
        let mut p = view::by_name("empty");
        p.lanes = vec![
            Lane { id: 1, name: "Main".into(), kind: view::LaneKind::Spine, locked: false, hidden: false, muted: false, solo: false, height: 40.0, colour: egui::Color32::GRAY },
            Lane { id: 2, name: "Music".into(), kind: view::LaneKind::Audio, locked: false, hidden: false, muted: false, solo: false, height: 40.0, colour: egui::Color32::GRAY },
        ];
        let mk = |id, lane, start: i64, dur: i64, kind| Clip { id, lane, start: start * SEC, dur: dur * SEC, kind, name: format!("clip{id}"), ..view::demo().clips[0].clone() };
        p.clips = vec![mk(1, 1, 0, 4, ClipKind::Video), mk(2, 1, 4, 3, ClipKind::Video), mk(3, 1, 7, 5, ClipKind::Video), mk(4, 2, 2, 8, ClipKind::Audio)];
        p
    }

    #[test]
    fn clips_step_along_their_lane() {
        let p = project();
        assert_eq!(step_clip(&p, Some(1), 0, true), Some(2));
        assert_eq!(step_clip(&p, Some(2), 0, true), Some(3));
        assert_eq!(step_clip(&p, Some(3), 0, true), None, "no clip after the last");
        assert_eq!(step_clip(&p, Some(2), 0, false), Some(1));
        assert_eq!(step_clip(&p, Some(1), 0, false), None);
    }

    #[test]
    fn with_nothing_selected_it_starts_from_the_playhead() {
        let p = project();
        assert_eq!(step_clip(&p, None, 5 * SEC, true), Some(3), "first clip starting at or after 5 s");
        assert_eq!(step_clip(&p, None, 5 * SEC, false), Some(2), "last clip starting before 5 s");
    }

    #[test]
    fn edit_points_are_clip_edges() {
        let p = project();
        assert_eq!(step_edit(&p, 0, true), Some(2 * SEC));
        assert_eq!(step_edit(&p, 2 * SEC, true), Some(4 * SEC));
        assert_eq!(step_edit(&p, 12 * SEC, true), None);
        assert_eq!(step_edit(&p, 4 * SEC, false), Some(2 * SEC));
        assert_eq!(step_edit(&p, 0, false), None);
    }

    #[test]
    fn lanes_step_to_the_clip_under_the_playhead() {
        let p = project();
        assert_eq!(step_lane(&p, Some(1), 3 * SEC, true), Some(4));
        assert_eq!(step_lane(&p, Some(4), 3 * SEC, false), Some(1), "the main-lane clip covering 3 s");
        assert_eq!(step_lane(&p, Some(4), 0, true), None, "no lane below");
    }

    #[test]
    fn seconds_read_naturally() {
        assert_eq!(seconds(3 * SEC), "3");
        assert_eq!(seconds(SEC + SEC / 2), "1.5");
    }
}

// --- the canvas without a pointer ---------------------------------------------------------

/// Visible layers at the playhead, top first.
pub fn layers_at(p: &Project, t: Time) -> Vec<u32> {
    p.visual_stack_at(t).into_iter().map(|c| c.id).collect()
}

/// The layer above (or below) `from` in the stack; with none selected, the top one.
pub fn step_layer(p: &Project, from: Option<u32>, t: Time, forward: bool) -> Option<u32> {
    let stack = layers_at(p, t);
    if stack.is_empty() {
        return None;
    }
    match from.and_then(|id| stack.iter().position(|x| *x == id)) {
        Some(i) => {
            let j = if forward { i + 1 } else { i.checked_sub(1)? };
            stack.get(j).copied()
        }
        None => Some(stack[0]),
    }
}

/// Where a layer is, as the person would say it: percent of the canvas
/// across and down from the centre.
pub fn layer_position(p: &Project, c: &Clip) -> (i32, i32) {
    let pct = |v: f32, whole: u32| (v / whole.max(1) as f32 * 100.0).round() as i32;
    (pct(c.transform.x, p.width), pct(c.transform.y, p.height))
}

fn say_layer(app: &StudioApp, ctx: &Context, id: u32) {
    let p = &app.doc.project;
    let Some(c) = p.clip(id) else { return };
    let stack = layers_at(p, app.editor.playhead);
    let n = stack.iter().position(|x| *x == id).map_or(1, |i| i + 1);
    let (x, y) = layer_position(p, c);
    let name = if c.name.is_empty() { crate::i18n::tr(kind_id(c.kind)).into_owned() } else { c.name.clone() };
    announce(
        ctx,
        tr_args(
            "a11y-layer",
            &[("n", n.into()), ("total", stack.len().into()), ("name", name.into()), ("kind", crate::i18n::tr(kind_id(c.kind)).into_owned().into()), ("x", x.into()), ("y", y.into()), ("scale", (c.transform.scale.round() as i64).into())],
        )
        .into_owned(),
    );
}

/// Keyboard control of what is on the canvas: `,` and `.` pick the layer
/// above / below at the playhead, `⌘⌥` with the arrows moves the selected
/// layer by 1 % of the canvas (`⇧` 5 %), `⌥,` lists the layers.
pub fn canvas_keys(app: &mut StudioApp, ctx: &Context) {
    use egui::{Key, KeyboardShortcut as S, Modifiers as M};
    let pressed = |m: M, k: Key| ctx.input_mut(|i| i.consume_shortcut(&S::new(m, k)));
    let sel = app.editor.selection.first().copied();
    // Alt combinations first: the plain keys also match them.
    if pressed(M::ALT, Key::Period) {
        announce(ctx, status_sentence(app));
        return;
    }
    if pressed(M::ALT, Key::Comma) {
        let stack = layers_at(&app.doc.project, app.editor.playhead);
        let names: Vec<String> = stack.iter().filter_map(|id| app.doc.project.clip(*id)).map(|c| if c.name.is_empty() { crate::i18n::tr(kind_id(c.kind)).into_owned() } else { c.name.clone() }).collect();
        let text = if names.is_empty() { crate::i18n::tr("a11y-no-layers").into_owned() } else { tr_args("a11y-layers", &[("total", names.len().into()), ("names", names.join(", ").into())]).into_owned() };
        announce(ctx, text);
        return;
    }
    for (key, forward) in [(Key::Period, true), (Key::Comma, false)] {
        if pressed(M::NONE, key) {
            match step_layer(&app.doc.project, sel, app.editor.playhead, forward) {
                Some(id) => {
                    app.editor.selection = vec![id];
                    say_layer(app, ctx, id);
                }
                None => announce(ctx, crate::i18n::tr(if app.doc.project.visual_stack_at(app.editor.playhead).is_empty() { "a11y-no-layers" } else { "a11y-no-more-layers" })),
            }
        }
    }
    let step = |big: bool| if big { 5.0 } else { 1.0 };
    for (key, dx, dy) in [(Key::ArrowLeft, -1.0, 0.0), (Key::ArrowRight, 1.0, 0.0), (Key::ArrowUp, 0.0, -1.0), (Key::ArrowDown, 0.0, 1.0)] {
        for big in [true, false] {
            let mods = if big { M::COMMAND | M::ALT | M::SHIFT } else { M::COMMAND | M::ALT };
            if pressed(mods, key) {
                let Some(id) = sel else {
                    announce(ctx, crate::i18n::tr("a11y-select-first"));
                    continue;
                };
                let (w, h) = (app.doc.project.width as f32, app.doc.project.height as f32);
                let k = step(big) / 100.0;
                if app.doc.edit("Move layer", |p| {
                    if let Some(c) = p.clip_mut(id) {
                        c.transform.x += dx * k * w;
                        c.transform.y += dy * k * h;
                    }
                }) {
                    if let Some(c) = app.doc.project.clip(id) {
                        let (x, y) = layer_position(&app.doc.project, c);
                        announce(ctx, tr_args("a11y-layer-moved", &[("x", x.into()), ("y", y.into())]).into_owned());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod canvas_tests {
    use super::*;

    #[test]
    fn layers_step_through_the_stack() {
        let p = crate::view::demo();
        let t = 2 * SEC;
        let stack = layers_at(&p, t);
        assert!(stack.len() >= 2);
        assert_eq!(step_layer(&p, None, t, true), Some(stack[0]));
        assert_eq!(step_layer(&p, Some(stack[0]), t, true), Some(stack[1]));
        assert_eq!(step_layer(&p, Some(stack[0]), t, false), None, "nothing above the top layer");
        assert_eq!(step_layer(&p, Some(*stack.last().unwrap()), t, true), None);
    }

    #[test]
    fn positions_are_percent_of_the_canvas() {
        let mut p = crate::view::demo();
        let id = p.clips[0].id;
        p.clip_mut(id).unwrap().transform.x = 192.0;
        p.clip_mut(id).unwrap().transform.y = -108.0;
        assert_eq!(layer_position(&p, p.clip(id).unwrap()), (10, -10));
    }
}
