// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Film editing in the window, on `studio_model::editorial`:
//!
//! - **Trim modes** (T cycles ripple / roll, U slide): edge drags in the
//!   timeline ripple or roll, body drags slide; the edit point stays
//!   selected for `[` / `]` (a frame, ten with Shift) and JKL trimming
//!   (play to where the cut belongs, K applies it). While trimming, the
//!   viewer shows the outgoing and incoming frames side by side.
//! - **Source monitor** (Shift+2, Shift+4 back to the program): the media
//!   selected in the library, with its own playhead and marks (I, O, J, K,
//!   L while it is up). `,` inserts and `.` overwrites into the program at
//!   its marks or the playhead (three and four points); Replace and Fit to
//!   fill on its bar.
//! - **Marks** in the program: `;` lifts, `'` extracts.
//! - Shift+F match frame (the source monitor opens on the frame under the
//!   playhead), E extend edit, Q / W ripple the clip under the playhead
//!   so its start / end comes to the playhead.
//! - **Lane headers**: target (crosshair) and sync lock (chain) toggles.

use crate::app::StudioApp;
use crate::doc::Doc;
use crate::icons::Icon;
use crate::kit::{self, ShowNoted};
use crate::theme;
use crate::view::{SEC, Time};
use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, Vec2};
use studio_model as m;
use studio_model::editorial as ed;

pub(super) mod panel;

/// How edge and body drags in the timeline behave.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrimMode {
    /// Plain: the timeline's own trims and moves.
    #[default]
    Off,
    /// Edges ripple: later clips follow (and sync-locked lanes).
    Ripple,
    /// Edges roll the cut: both sides change.
    Roll,
    /// Body drags slide the clip between its neighbours.
    Slide,
}

/// A selected edit point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimPoint {
    pub clip: u32,
    /// The clip's start (true) or end.
    pub left: bool,
}

/// What is loaded in the source monitor.
pub struct Source {
    pub what: ed::Source,
    pub name: String,
    pub len: Time,
    pub playhead: Time,
    pub mark_in: Option<Time>,
    pub mark_out: Option<Time>,
    pub playing: bool,
    pub rate: f32,
    /// A document holding just the source on its own sequence, for the
    /// renderer.
    doc: Doc,
    #[cfg(feature = "render")]
    view: Option<super::render_view::RenderView>,
}

#[derive(Default)]
pub struct Cutting {
    pub mode: TrimMode,
    pub point: Option<TrimPoint>,
    pub source: Option<Source>,
    /// The viewer shows the source monitor instead of the program.
    pub show_source: bool,
    /// JKL trimming: the playhead when playback started over a selected
    /// edit point.
    jkl_from: Option<Time>,
    /// The Editorial window (index, checks, takes, sequences, script).
    pub panel: panel::Panel,
    /// Linked selection off: moves and nudges leave linked clips behind.
    pub unlinked: bool,
    /// A slide in progress: clip, pointer x at the press, delta sent.
    slide: Option<(u32, f32, Time)>,
    /// The media last picked in the library (the source follows it).
    picked: Option<usize>,
    #[cfg(feature = "render")]
    trim_views: Option<(super::render_view::RenderView, super::render_view::RenderView)>,
}

fn toast(ctx: &egui::Context, text: impl Into<String>) {
    kit::push(ctx, text, None, Some(Icon::Info));
}

fn frame(app: &StudioApp) -> Time {
    app.editor.frame_dur(app.doc.project.fps)
}

/// Applies a planned editorial edit as one undo step.
fn run(app: &mut StudioApp, ctx: &egui::Context, label: &str, r: Result<m::Edit, m::EditError>) -> bool {
    match r {
        Ok(e) => app.doc.apply(label, e),
        Err(e) => {
            toast(ctx, e.to_string());
            false
        }
    }
}

impl Source {
    fn new(app: &StudioApp, what: ed::Source) -> Option<Self> {
        let model = &app.doc.model;
        let (name, len) = match what {
            ed::Source::Media(id) => {
                let mi = model.media_item(id)?;
                (mi.name.clone(), mi.duration().unwrap_or(5 * SEC))
            }
            ed::Source::Sequence(id) => {
                let s = model.sequence(id)?;
                (s.name.clone(), s.duration().max(SEC))
            }
        };
        // The source alone on a sequence of its own, in a copy of the
        // project (its media and nested sequences come along).
        let mut p = model.clone();
        let seq = m::SequenceId(p.alloc_id());
        let spine = m::LaneId(p.alloc_id());
        let mut s = m::Sequence::new(seq, "Source", spine);
        let source = match what {
            ed::Source::Media(id) => m::ClipSource::Media(m::MediaRef::new(id)),
            ed::Source::Sequence(id) => m::ClipSource::Sequence(m::SequenceRef { sequence: id, extra: Default::default() }),
        };
        s.clips.push(m::Clip::new(m::ClipId(p.alloc_id()), spine, 0, len, source));
        p.sequences.push(s);
        p.active_sequence = seq;
        Some(Self { what, name, len, playhead: 0, mark_in: None, mark_out: None, playing: false, rate: 1.0, doc: Doc::new(p), #[cfg(feature = "render")] view: None })
    }
}

/// Loads a source into the monitor and shows it.
pub fn open_source(app: &mut StudioApp, what: ed::Source, at: Time, mark: bool) {
    if let Some(mut s) = Source::new(app, what) {
        s.playhead = at.clamp(0, s.len);
        if mark {
            s.mark_in = Some(s.playhead);
        }
        app.editor.cutting.source = Some(s);
        app.editor.cutting.show_source = true;
    }
}

// ------------------------------------------------------------ the edits

/// Insert, overwrite, replace or fit to fill from the source monitor into
/// the program at its marks (or the playhead).
pub fn source_to_record(app: &mut StudioApp, ctx: &egui::Context, mode: ed::SourceMode) -> bool {
    let Some(src) = &app.editor.cutting.source else {
        toast(ctx, "Pick a clip in the library and open the source monitor (Shift+2) first");
        return false;
    };
    let model = &app.doc.model;
    let seq = model.active();
    let (has_video, has_audio) = match src.what {
        ed::Source::Media(id) => model.media_item(id).map_or((true, true), |mi| (mi.has_video(), mi.has_audio())),
        ed::Source::Sequence(_) => (true, true),
    };
    let targeted = |k: m::LaneKind| seq.lanes.iter().find(|l| l.kind == k && ed::targeted(l) && !l.locked).map(|l| l.id);
    let video_lane = if has_video { Some(seq.spine_id()).filter(|_| seq.lane(seq.spine_id()).is_some_and(|l| ed::targeted(l))).or(targeted(m::LaneKind::Visual)) } else { None };
    let audio_lane = if !has_video && has_audio { targeted(m::LaneKind::Audio) } else { None };
    let (rec_in, rec_out) = match app.doc.project.range {
        Some((a, b)) => (Some(a), Some(b)),
        None => (Some(app.editor.playhead), None),
    };
    let e = ed::SourceEdit { source: src.what, src_in: src.mark_in, src_out: src.mark_out, rec_in, rec_out, mode, video_lane, audio_lane, audio: true };
    let s = seq.id;
    let label = match mode {
        ed::SourceMode::Insert => "Insert",
        ed::SourceMode::Overwrite => "Overwrite",
        ed::SourceMode::Replace => "Replace",
        ed::SourceMode::FitToFill => "Fit to fill",
    };
    let playhead = app.editor.playhead;
    match ed::source_edit(&mut app.doc.model, s, &e, playhead) {
        Ok((edit, done)) => {
            // Like every editor: the playhead lands after the edit, the
            // marks are used up (one undo step with the edit).
            let mut ok = app.doc.apply_gesture("cut-source", label, edit);
            if ok && app.doc.project.range.is_some() {
                ok = app.doc.edit_gesture("cut-source", label, |p| p.range = None);
            }
            app.doc.end_gesture();
            if ok {
                app.editor.playhead = done.rec.end();
                app.editor.cutting.show_source = false;
            }
            ok
        }
        Err(err) => {
            toast(ctx, err.to_string());
            false
        }
    }
}

/// `;` lift / `'` extract between the program marks.
pub fn lift_or_extract(app: &mut StudioApp, ctx: &egui::Context, extract: bool) -> bool {
    let Some((a, b)) = app.doc.project.range else {
        toast(ctx, "Mark in and out first (I, O)");
        return false;
    };
    let seq = app.doc.seq();
    let r = if extract { ed::extract(&mut app.doc.model, seq, a, b, None) } else { ed::lift(&mut app.doc.model, seq, a, b, None) };
    let label = if extract { "Extract" } else { "Lift" };
    let ok = match r {
        // The edit and the used-up marks are one undo step.
        Ok(e) => app.doc.apply_gesture("cut-range", label, e) && app.doc.edit_gesture("cut-range", label, |p| p.range = None),
        Err(e) => {
            toast(ctx, e.to_string());
            false
        }
    };
    app.doc.end_gesture();
    if ok {
        app.editor.playhead = a;
    }
    ok
}

/// Shift+F: the source monitor opens on the frame under the playhead.
pub fn match_frame(app: &mut StudioApp, ctx: &egui::Context) -> bool {
    let seq = app.doc.seq();
    match ed::match_frame(&app.doc.model, seq, app.editor.playhead, None) {
        Some(hit) => {
            open_source(app, hit.source, hit.source_time, true);
            true
        }
        None => {
            toast(ctx, "No footage under the playhead");
            false
        }
    }
}

/// The spine clip under the playhead (or the selected clip).
fn clip_here(app: &StudioApp) -> Option<m::Clip> {
    let seq = app.doc.model.active();
    if let Some(id) = app.editor.selection.first() {
        return seq.clip(m::ClipId(*id as u64)).cloned();
    }
    seq.clip_at(seq.spine_id(), app.editor.playhead).filter(|c| !c.is_gap()).cloned()
}

/// E: the selected edit point (or the nearest cut on the main lane) rolls
/// to the playhead.
pub fn extend(app: &mut StudioApp, ctx: &egui::Context) -> bool {
    let seq = app.doc.seq();
    let t = app.editor.playhead;
    let (clip, edge) = match app.editor.cutting.point {
        Some(p) => (m::ClipId(p.clip as u64), if p.left { m::Edge::Start } else { m::Edge::End }),
        None => {
            let s = app.doc.model.active();
            let spine = s.spine_clips();
            let Some(c) = spine.windows(2).map(|w| w[0]).min_by_key(|c| (c.end() - t).abs()) else {
                toast(ctx, "No cut to extend");
                return false;
            };
            (c.id, m::Edge::End)
        }
    };
    let r = ed::extend(&mut app.doc.model, seq, clip, edge, t);
    run(app, ctx, "Extend edit", r)
}

/// Q / W: ripple the clip under the playhead so its start (Q) or end (W)
/// comes to the playhead.
pub fn ripple_to_playhead(app: &mut StudioApp, ctx: &egui::Context, start: bool) -> bool {
    let Some(c) = clip_here(app) else {
        toast(ctx, "No clip under the playhead");
        return false;
    };
    let t = app.editor.playhead;
    let seq = app.doc.seq();
    let (edge, delta) = if start { (m::Edge::Start, t - c.start) } else { (m::Edge::End, t - c.end()) };
    if delta == 0 || t <= c.start || t >= c.end() {
        return false;
    }
    let r = ed::ripple_trim(&mut app.doc.model, seq, c.id, edge, delta);
    let ok = run(app, ctx, "Ripple trim", r);
    if ok && start {
        app.editor.playhead = c.start;
    }
    ok
}

/// The edit a timeline edge drag sends, by trim mode.
pub fn trim_edit(app: &mut StudioApp, id: u32, left: bool, step: Time, spine: bool) -> m::Edit {
    let seq = app.doc.seq();
    let clip = m::ClipId(id as u64);
    let edge = if left { m::Edge::Start } else { m::Edge::End };
    app.editor.cutting.point = Some(TrimPoint { clip: id, left });
    match app.editor.cutting.mode {
        TrimMode::Ripple => {
            let any_sync = app.doc.model.active().lanes.iter().any(ed::sync_locked);
            if any_sync && let Ok(e) = ed::ripple_trim(&mut app.doc.model, seq, clip, edge, step) {
                return e;
            }
            m::Edit::Trim { seq, clip, edge, delta: step, ripple: true }
        }
        TrimMode::Roll => {
            let s = app.doc.model.active();
            let left_clip = if left {
                s.clip(clip).and_then(|c| s.clips.iter().find(|o| o.lane == c.lane && o.end() == c.start && o.id != c.id)).map(|o| o.id)
            } else {
                Some(clip)
            };
            match left_clip {
                Some(l) => m::Edit::Roll { seq, left: l, delta: step },
                None => m::Edit::Trim { seq, clip, edge, delta: step, ripple: false },
            }
        }
        _ => m::Edit::Trim { seq, clip, edge, delta: step, ripple: spine && app.prefs.magnetic },
    }
}

/// `[` / `]`: the selected edit point moves by `frames`.
pub fn nudge(app: &mut StudioApp, ctx: &egui::Context, frames: i64) -> bool {
    let Some(p) = app.editor.cutting.point else { return false };
    let delta = frames * frame(app);
    let spine = app.doc.model.active().clip(m::ClipId(p.clip as u64)).is_some_and(|c| c.lane == app.doc.model.active().spine_id());
    let e = trim_edit(app, p.clip, p.left, delta, spine);
    let ok = app.doc.apply("Trim", e);
    if !ok && let Some(e) = app.doc.error.take() {
        toast(ctx, e);
    }
    ok
}

/// The edit a timeline move sends: with linked selection off, the clips
/// move without their linked partners.
pub fn move_edit(app: &mut StudioApp, clips: Vec<m::ClipId>, delta: Time, to_lane: Option<m::LaneId>) -> m::Edit {
    let seq = app.doc.seq();
    if app.editor.cutting.unlinked
        && let Ok(e) = ed::move_alone(&mut app.doc.model, seq, &clips, delta, to_lane)
    {
        return e;
    }
    m::Edit::Move { seq, clips, delta, to_lane }
}

/// `edit` as it should run on the selected `ids`: with linked selection
/// off, their linked partners stay put.
pub fn alone_edit(app: &mut StudioApp, ids: &[u32], edit: m::Edit) -> m::Edit {
    if !app.editor.cutting.unlinked {
        return edit;
    }
    let seq = app.doc.seq();
    let clips: Vec<m::ClipId> = ids.iter().map(|id| m::ClipId(*id as u64)).collect();
    ed::without_links(&mut app.doc.model, seq, &clips, edit.clone()).unwrap_or(edit)
}

/// Snap targets beyond clip edges: clip markers, keyframes, transition
/// edges and the sequence end.
pub fn snap_extras(app: &StudioApp, exclude: &[u32]) -> Vec<Time> {
    let s = app.doc.model.active();
    let mut v = vec![s.duration()];
    for c in s.clips.iter().filter(|c| !exclude.contains(&(c.id.0 as u32))) {
        v.extend(c.markers.iter().map(|mk| c.start + mk.time));
        for tr in c.keyframes.values() {
            v.extend(tr.keys.iter().map(|k| c.start + k.time).filter(|t| *t > c.start && *t < c.end()));
        }
        if v.len() > 20_000 {
            break;
        }
    }
    for t in &s.transitions {
        if let Some(r) = s.clip(t.right) {
            let (pre, post) = t.split();
            v.push(r.start - pre);
            v.push(r.start + post);
        }
    }
    v
}

/// Whether a body drag in the timeline should slide (and starts it).
pub fn start_slide(app: &mut StudioApp, id: u32, x: f32) -> bool {
    if app.editor.cutting.mode != TrimMode::Slide {
        return false;
    }
    app.editor.cutting.slide = Some((id, x, 0));
    true
}

// ------------------------------------------------------------- per frame

/// Per frame: slides follow the pointer, JKL trims land when playback
/// stops, the source monitor plays.
pub fn tick(app: &mut StudioApp, ctx: &egui::Context) {
    let cut = &mut app.editor.cutting;
    // A slide follows the pointer while it is held.
    if let Some((id, x0, applied)) = cut.slide {
        let down = ctx.input(|i| i.pointer.any_down());
        if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()).filter(|_| down) {
            let want = ((pos.x - x0) / app.editor.px_per_sec.max(0.01) * SEC as f32) as Time;
            let step = want - applied;
            if step != 0 {
                let seq = app.doc.seq();
                if app.doc.apply_gesture("tl-slide", "Slide", m::Edit::Slide { seq, clip: m::ClipId(id as u64), delta: step }) {
                    app.editor.cutting.slide = Some((id, x0, want));
                } else {
                    app.doc.error = None;
                }
            }
        } else {
            app.editor.cutting.slide = None;
            app.doc.end_gesture();
        }
    }
    // An edge drag selects its edit point.
    if let Some(super::TlDrag::Trim { id, left, .. }) = app.editor.tl_drag {
        app.editor.cutting.point = Some(TrimPoint { clip: id, left });
    }
    if let Some(p) = app.editor.cutting.point
        && app.doc.project.clip(p.clip).is_none()
    {
        app.editor.cutting.point = None;
    }
    // JKL trimming: play over a selected edit point in a trim mode; when
    // playback stops the point moves by how far the playhead went.
    let c = &app.editor.cutting;
    let (point, trimming, show_source, from) = (c.point, c.point.is_some() && matches!(c.mode, TrimMode::Ripple | TrimMode::Roll), c.show_source, c.jkl_from);
    match (trimming && app.editor.playing && !show_source, from) {
        (true, None) => app.editor.cutting.jkl_from = Some(app.editor.playhead),
        (false, Some(from)) => {
            app.editor.cutting.jkl_from = None;
            let delta = app.editor.playhead - from;
            if trimming && delta.abs() >= frame(app) / 2 {
                let p = point.expect("trimming");
                let spine = app.doc.model.active().clip(m::ClipId(p.clip as u64)).is_some_and(|c| c.lane == app.doc.model.active().spine_id());
                let e = trim_edit(app, p.clip, p.left, delta, spine);
                if !app.doc.apply("JKL trim", e)
                    && let Some(e) = app.doc.error.take()
                {
                    toast(ctx, e);
                }
            }
        }
        _ => {}
    }
    // The source monitor plays on its own.
    let dt = ctx.input(|i| i.stable_dt).min(0.1);
    if let Some(s) = &mut app.editor.cutting.source
        && s.playing
    {
        s.playhead = (s.playhead as f64 + dt as f64 * s.rate as f64 * SEC as f64) as Time;
        if s.playhead <= 0 || s.playhead >= s.len {
            s.playhead = s.playhead.clamp(0, s.len);
            s.playing = false;
        }
        ctx.request_repaint();
    }
    // The source follows the media picked in the library.
    if let Some((super::RailItem::Media, i)) = app.editor.library_selected
        && app.editor.cutting.picked != Some(i)
    {
        app.editor.cutting.picked = Some(i);
        if let Some(mi) = app.doc.project.media.get(i) {
            let what = ed::Source::Media(m::MediaId(mi.id as u64));
            if app.editor.cutting.source.as_ref().is_none_or(|s| s.what != what) {
                let show = app.editor.cutting.show_source;
                open_source(app, what, 0, false);
                app.editor.cutting.show_source = show;
            }
        }
    }
}

/// Keys; runs before the editor's and the accessibility keys. A key is
/// only taken when it means something to editing now (their keys are in
/// Settings ▸ Shortcuts) (`,` / `.` with a
/// source loaded, `[` / `]` with an edit point), so the same keys still
/// step layers and clips otherwise. While the source monitor is up it
/// takes the transport and mark keys (I, O, J, K, L, Space, arrows).
pub fn shortcuts(app: &mut StudioApp, ctx: &egui::Context) {
    use crate::keymap::{hit, pressed};
    if ctx.text_edit_focused() || app.overlay != crate::app::Overlay::None || crate::onboarding::tour_running(app) {
        return;
    }
    let prefs = app.prefs.clone();
    if hit(ctx, &prefs, "source-monitor") {
        if app.editor.cutting.source.is_none() {
            let seq = app.doc.seq();
            if let Some(m) = ed::match_frame(&app.doc.model, seq, app.editor.playhead, None) {
                open_source(app, m.source, m.source_time, false);
            } else {
                toast(ctx, "Pick a clip in the library to load the source monitor");
            }
        }
        app.editor.cutting.show_source = app.editor.cutting.source.is_some();
    }
    if hit(ctx, &prefs, "program-monitor") {
        app.editor.cutting.show_source = false;
    }
    if app.editor.cutting.show_source {
        let fr = frame(app);
        let keys = pressed(ctx, &prefs, &["in", "out", "clear-range", "stop", "play", "shuttle-fwd", "shuttle-back", "frame-back", "frame-fwd"]);
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let Some(s) = &mut app.editor.cutting.source else { return };
        for k in keys {
            match k {
                "in" => s.mark_in = Some(s.playhead),
                "out" => s.mark_out = Some(s.playhead.max(s.mark_in.map_or(0, |a| a + fr))),
                "clear-range" => (s.mark_in, s.mark_out) = (None, None),
                "stop" => (s.playing, s.rate) = (false, 1.0),
                "play" => (s.playing, s.rate) = (!s.playing, 1.0),
                "shuttle-fwd" => {
                    s.rate = if s.playing && s.rate > 0.0 { (s.rate * 2.0).min(8.0) } else { 1.0 };
                    s.playing = true;
                }
                "shuttle-back" => {
                    s.rate = if s.playing && s.rate < 0.0 { (s.rate * 2.0).max(-8.0) } else { -1.0 };
                    s.playing = true;
                }
                "frame-back" => s.playhead = (s.playhead - fr).max(0),
                "frame-fwd" => s.playhead = (s.playhead + fr).min(s.len),
                _ => {}
            }
        }
        if escape {
            app.editor.cutting.show_source = false;
        }
    }
    if app.editor.cutting.source.is_some() {
        for k in pressed(ctx, &prefs, &["insert", "overwrite"]) {
            source_to_record(app, ctx, if k == "insert" { ed::SourceMode::Insert } else { ed::SourceMode::Overwrite });
        }
    }
    for k in pressed(ctx, &prefs, &["lift", "extract", "match-frame", "extend", "ripple-start", "ripple-end", "trim-mode", "slide-mode"]) {
        match k {
            "lift" => {
                lift_or_extract(app, ctx, false);
            }
            "extract" => {
                lift_or_extract(app, ctx, true);
            }
            "match-frame" => {
                match_frame(app, ctx);
            }
            "extend" => {
                extend(app, ctx);
            }
            "ripple-start" => {
                ripple_to_playhead(app, ctx, true);
            }
            "ripple-end" => {
                ripple_to_playhead(app, ctx, false);
            }
            "trim-mode" => {
                let c = &mut app.editor.cutting;
                c.mode = match c.mode {
                    TrimMode::Ripple => TrimMode::Roll,
                    TrimMode::Roll => TrimMode::Off,
                    _ => TrimMode::Ripple,
                };
                app.editor.tool = super::Tool::Select;
                toast(ctx, mode_name(app.editor.cutting.mode));
            }
            _ => {
                let c = &mut app.editor.cutting;
                c.mode = if c.mode == TrimMode::Slide { TrimMode::Off } else { TrimMode::Slide };
                app.editor.tool = super::Tool::Select;
                toast(ctx, mode_name(app.editor.cutting.mode));
            }
        }
    }
    // After the mode keys, so T then [ in one go trims in the new mode.
    if app.editor.cutting.point.is_some() {
        for k in pressed(ctx, &prefs, &["trim-back", "trim-fwd", "trim-back-10", "trim-fwd-10"]) {
            let n = match k {
                "trim-back" => -1,
                "trim-fwd" => 1,
                "trim-back-10" => -10,
                _ => 10,
            };
            nudge(app, ctx, n);
        }
    }
    // Another tool picked: trim modes belong to the select tool.
    if app.editor.tool != super::Tool::Select {
        app.editor.cutting.mode = TrimMode::Off;
    }
}

fn mode_name(m: TrimMode) -> &'static str {
    match m {
        TrimMode::Off => "Trim mode off",
        TrimMode::Ripple => "Ripple trim: edges move later clips (T for roll)",
        TrimMode::Roll => "Roll trim: edges move the cut (T to turn off)",
        TrimMode::Slide => "Slide: drag a clip between its neighbours (U to turn off)",
    }
}

// --------------------------------------------------------------- viewer

/// The two frames either side of the edit being trimmed (timeline times)
/// and their labels.
pub fn trim_frames(app: &StudioApp) -> Option<(Time, Time, &'static str, &'static str)> {
    let s = app.doc.model.active();
    let fr = frame(app);
    let clip = |id: u32| s.clip(m::ClipId(id as u64));
    if let Some(super::TlDrag::Slip { id, .. }) = app.editor.tl_drag {
        let c = clip(id)?;
        return Some((c.start, c.end() - fr, "In", "Out"));
    }
    if let Some((id, _, _)) = app.editor.cutting.slide {
        let c = clip(id)?;
        return Some((c.start - fr, c.end(), "Before", "After"));
    }
    let (id, left) = match app.editor.tl_drag {
        Some(super::TlDrag::Trim { id, left, .. }) => (id, left),
        _ if app.editor.cutting.mode != TrimMode::Off && app.editor.cutting.mode != TrimMode::Slide && !app.editor.playing => {
            let p = app.editor.cutting.point?;
            (p.clip, p.left)
        }
        _ => return None,
    };
    let c = clip(id)?;
    let at = if left { c.start } else { c.end() };
    Some(((at - fr).max(0), at, "Outgoing", "Incoming"))
}

/// Draws over the viewer: the dual-up trim view, or the source monitor.
pub fn viewer(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let area = Rect::from_min_max(egui::pos2(r.left() + 1.0, r.top() + 35.0), egui::pos2(r.right() - 1.0, r.bottom() - 41.0));
    if area.width() < 80.0 || area.height() < 60.0 {
        return;
    }
    if let Some(frames) = trim_frames(app) {
        dual_up(app, ui, area, frames);
    } else if app.editor.cutting.show_source && app.editor.cutting.source.is_some() {
        source_monitor(app, ui, area);
    } else if app.editor.cutting.source.is_some() {
        // A way back to the loaded source.
        let b = Rect::from_min_size(egui::pos2(area.left() + 8.0, area.top() + 6.0), Vec2::new(86.0, 22.0));
        let resp = ui.interact(b, egui::Id::unique("cut-source-tab"), Sense::click());
        let t = theme::tokens(ui.ctx());
        ui.painter().rect_filled(b, 6, Color32::from_black_alpha(if resp.hovered() { 170 } else { 110 }));
        ui.painter().text(b.center(), Align2::CENTER_CENTER, crate::i18n::shown("Source ⇧2"), theme::caption(), t.text);
        if resp.clicked() {
            app.editor.cutting.show_source = true;
        }
    }
}

fn fit(area: Rect, w: f32, h: f32) -> Rect {
    let aspect = w / h.max(1.0);
    let s = (area.width() / aspect).min(area.height());
    Rect::from_center_size(area.center(), Vec2::new(s * aspect, s))
}

fn dual_up(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect, (a, b, la, lb): (Time, Time, &str, &str)) {
    // The canvas underneath takes no clicks or drags meanwhile.
    let _ = ui.interact(area, egui::Id::unique("cut-dual-up"), Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(area);
    p.rect_filled(area, 0, Color32::from_rgb(12, 12, 14));
    let half = |k: f32| Rect::from_min_size(egui::pos2(area.left() + k * area.width() / 2.0, area.top()), Vec2::new(area.width() / 2.0, area.height()));
    let (w, h) = (app.doc.project.width as f32, app.doc.project.height as f32);
    let fps = app.doc.project.fps;
    for (k, (t, label)) in [(a, la), (b, lb)].into_iter().enumerate() {
        let slot = fit(half(k as f32).shrink(6.0), w, h);
        p.rect_filled(slot, 0, app.doc.project.background);
        #[cfg(feature = "render")]
        {
            let views = app.editor.cutting.trim_views.get_or_insert_with(|| (super::render_view::RenderView::new(), super::render_view::RenderView::new()));
            let rv = if k == 0 { &mut views.0 } else { &mut views.1 };
            rv.sync(&app.doc, t);
            let ppp = ui.ctx().pixels_per_point();
            let px = ((slot.width() * ppp * 0.5).max(16.0) as u32, (slot.height() * ppp * 0.5).max(16.0) as u32);
            if let Some(tex) = rv.frame(ui.ctx(), t, px, false, false) {
                p.image(tex, slot, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        let tag = format!("{label} · {}", crate::view::timecode(t, fps));
        let tr = Rect::from_min_size(slot.left_bottom() + Vec2::new(6.0, -26.0), Vec2::new(150.0, 20.0));
        p.rect_filled(tr, 5, Color32::from_black_alpha(160));
        p.text(tr.left_center() + Vec2::new(8.0, 0.0), Align2::LEFT_CENTER, tag, theme::caption(), Color32::WHITE);
    }
    p.vline(area.center().x, area.y_range(), Stroke::new(1.0, Color32::from_white_alpha(60)));
    let mode = match app.editor.cutting.mode {
        TrimMode::Ripple => "Ripple",
        TrimMode::Roll => "Roll",
        TrimMode::Slide => "Slide",
        TrimMode::Off => "Trim",
    };
    kit::paint_text_clipped(&p, egui::pos2(area.center().x, area.top() + 12.0), Align2::CENTER_CENTER, &format!("{mode} · [ ] a frame · J K L to trim by playing"), theme::caption(), Color32::from_white_alpha(200), area.width() - 24.0);
}

fn source_monitor(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    let fps = app.doc.project.fps;
    let (w, h) = (app.doc.project.width as f32, app.doc.project.height as f32);
    let bar_h = 58.0;
    let pic_area = Rect::from_min_max(area.min, egui::pos2(area.right(), area.bottom() - bar_h));
    let _ = ui.interact(area, egui::Id::unique("cut-source-area"), Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(area);
    p.rect_filled(area, 0, Color32::from_rgb(10, 10, 12));
    let slot = fit(pic_area.shrink(8.0), w, h);
    p.rect_filled(slot, 0, Color32::BLACK);
    let Some(s) = &mut app.editor.cutting.source else { return };
    #[cfg(feature = "render")]
    {
        let rv = s.view.get_or_insert_with(super::render_view::RenderView::new);
        rv.sync(&s.doc, s.playhead);
        let ppp = ctx.pixels_per_point();
        let px = ((slot.width() * ppp * 0.75).max(16.0) as u32, (slot.height() * ppp * 0.75).max(16.0) as u32);
        let step = (SEC as f64 / fps.max(1.0) as f64) as i64;
        let tq = s.playhead / step.max(1) * step.max(1);
        if let Some(tex) = rv.frame(&ctx, tq, px, false, s.playing) {
            p.image(tex, slot, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    let _ = &s.doc;
    // Title.
    let title = crate::i18n::tf(format!("SOURCE · {} · {}", s.name, crate::view::timecode(s.playhead, fps)));
    let marks = crate::i18n::tf(format!("In {} · Out {}", s.mark_in.map_or("—".into(), |a| crate::view::timecode(a, fps)), s.mark_out.map_or("—".into(), |b| crate::view::timecode(b, fps))));
    // The marks keep their place at the right; the title ends in an
    // ellipsis before them.
    let marks_w = kit::text_width(ui, &marks, theme::caption());
    kit::paint_text_clipped(&p, egui::pos2(area.left() + 12.0, area.top() + 12.0), Align2::LEFT_CENTER, &title, theme::caption(), Color32::from_white_alpha(220), area.width() - 36.0 - marks_w);
    // Scrub bar with marks.
    let bar = Rect::from_min_max(egui::pos2(area.left() + 12.0, area.bottom() - bar_h + 6.0), egui::pos2(area.right() - 12.0, area.bottom() - bar_h + 20.0));
    let x_of = |tt: Time| egui::lerp(bar.x_range(), (tt as f64 / s.len.max(1) as f64) as f32);
    p.rect_filled(bar, 4, Color32::from_white_alpha(30));
    if let (Some(a), b) = (s.mark_in, s.mark_out) {
        let xb = x_of(b.unwrap_or(s.len));
        p.rect_filled(Rect::from_min_max(egui::pos2(x_of(a), bar.top()), egui::pos2(xb, bar.bottom())), 4, theme::alpha(t.accent, 0.45));
    } else if let Some(b) = s.mark_out {
        p.rect_filled(Rect::from_min_max(bar.left_top(), egui::pos2(x_of(b), bar.bottom())), 4, theme::alpha(t.accent, 0.45));
    }
    p.vline(x_of(s.playhead), (bar.top() - 3.0)..=(bar.bottom() + 3.0), Stroke::new(2.0, Color32::WHITE));
    let resp = ui.interact(bar.expand(4.0), egui::Id::unique("cut-source-scrub"), Sense::click_and_drag());
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let u = ((pos.x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0);
        s.playhead = (u as f64 * s.len as f64) as Time;
        s.playing = false;
    }
    // Buttons: a viewer narrower than they are scrolls them sideways.
    let mut clicked = None;
    let buttons: Vec<String> = [("Mark in", "I"), ("Mark out", "O"), ("Insert", ","), ("Overwrite", "."), ("Replace", ""), ("Fit to fill", ""), ("Program", "⇧4")].into_iter().map(|(label, key)| if key.is_empty() { label.to_owned() } else { format!("{label} {key}") }).collect();
    let widths: Vec<f32> = buttons.iter().map(|text| 14.0 + 6.6 * text.chars().count() as f32).collect();
    let total = widths.iter().sum::<f32>() + 6.0 * (widths.len() - 1) as f32;
    let band = Rect::from_min_max(egui::pos2(area.left() + 12.0, bar.bottom() + 8.0), egui::pos2(area.right() - 4.0, bar.bottom() + 32.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(band).id_salt("cut-source-buttons"));
    bui.set_clip_rect(band.intersect(area));
    egui::ScrollArea::horizontal().id_salt("cut-source-buttons-scroll").auto_shrink([false, false]).show_h(&mut bui, |ui| {
        let (strip, _) = ui.allocate_exact_size(Vec2::new(total.max(band.width()), 24.0), Sense::hover());
        let bp = ui.painter().clone();
        let mut x = strip.left();
        for (k, (text, wdt)) in buttons.iter().zip(&widths).enumerate() {
            let row = Rect::from_min_size(egui::pos2(x, strip.top()), Vec2::new(*wdt, 24.0));
            let r = ui.interact(row, egui::Id::unique(("cut-source-btn", k)), Sense::click());
            bp.rect_filled(row, 6, if r.hovered() { Color32::from_white_alpha(46) } else { Color32::from_white_alpha(24) });
            bp.text(row.center(), Align2::CENTER_CENTER, text, theme::caption(), t.text);
            if r.clicked() {
                clicked = Some(k);
            }
            x = row.right() + 6.0;
        }
    });
    p.text(egui::pos2(area.right() - 12.0, area.top() + 12.0), Align2::RIGHT_CENTER, marks, theme::caption(), Color32::from_white_alpha(200));
    match clicked {
        Some(0) => s.mark_in = Some(s.playhead),
        Some(1) => s.mark_out = Some(s.playhead),
        Some(2) => {
            source_to_record(app, &ctx, ed::SourceMode::Insert);
        }
        Some(3) => {
            source_to_record(app, &ctx, ed::SourceMode::Overwrite);
        }
        Some(4) => {
            source_to_record(app, &ctx, ed::SourceMode::Replace);
        }
        Some(5) => {
            source_to_record(app, &ctx, ed::SourceMode::FitToFill);
        }
        Some(6) => app.editor.cutting.show_source = false,
        _ => {}
    }
}

// ------------------------------------------------------------ lanes

/// Target and sync-lock toggles at the edges of a lane header.
pub fn lane_toggles(app: &mut StudioApp, ui: &mut egui::Ui, hr: Rect, lane: u32) {
    let seq = app.doc.seq();
    let Some(l) = app.doc.model.active().lane(m::LaneId(lane as u64)).cloned() else { return };
    let t = theme::tokens(ui.ctx());
    let cy = hr.center().y;
    for (k, (icon, on, tip)) in [(Icon::Circle, ed::targeted(&l), if ed::targeted(&l) { "Targeted: marks, lift and extract use this lane (click to untarget)" } else { "Not targeted (click to target)" }), (Icon::LinkSimple, ed::sync_locked(&l), if ed::sync_locked(&l) { "Sync lock on: follows ripples on other lanes" } else { "Sync lock off (click to keep this lane in sync with ripples)" })].into_iter().enumerate() {
        let x = if k == 0 { hr.left() + 9.0 } else { hr.right() - 9.0 };
        let r = Rect::from_center_size(egui::pos2(x, cy), Vec2::splat(14.0));
        let resp = ui.interact(r, egui::Id::unique(("lane-cut-toggle", lane, k)), Sense::click()).on_hover_text(tip);
        let tint = if on { t.accent } else { theme::alpha(t.text_2, if resp.hovered() { 0.8 } else { 0.35 }) };
        crate::icons::paint(ui.painter(), r, icon, 11.0, tint);
        if resp.clicked() {
            let e = if k == 0 { ed::set_targeted(&app.doc.model, seq, l.id, !on) } else { ed::set_sync_lock(&app.doc.model, seq, l.id, !on) };
            if let Ok(e) = e {
                app.doc.apply(if k == 0 { "Target lane" } else { "Sync lock" }, e);
            }
        }
    }
}

/// Trim mode buttons for the timeline toolbar.
pub fn toolbar(app: &mut StudioApp, bar: &mut egui::Ui) {
    for (mode, icon, name, key) in [(TrimMode::Ripple, Icon::ArrowsOutLineHorizontal, "Ripple trim: edges move later clips", "T"), (TrimMode::Roll, Icon::SquareSplitHorizontal, "Roll trim: edges move the cut", "T T"), (TrimMode::Slide, Icon::ArrowsHorizontal, "Slide: drag a clip between its neighbours", "U")] {
        let on = app.editor.cutting.mode == mode;
        if kit::IconButton::new(icon, name).shortcut(key).active(on).show(bar).clicked() {
            app.editor.cutting.mode = if on { TrimMode::Off } else { mode };
            app.editor.tool = super::Tool::Select;
        }
    }
    let open = app.editor.cutting.panel.open;
    if kit::IconButton::new(Icon::List, "Editorial: timeline index, gaps and duplicates, takes, sequences, find takes for a script line, edit by words (⌥E)").active(open).show(bar).clicked() {
        app.editor.cutting.panel.open = !open;
    }
    let linked = !app.editor.cutting.unlinked;
    if kit::IconButton::new(Icon::LinkSimple, if linked { "Linked selection on: picture and its sound move together (click to move them apart)" } else { "Linked selection off: clips move without their linked sound" }).active(linked).show(bar).clicked() {
        app.editor.cutting.unlinked = linked;
    }
    let src = app.editor.cutting.show_source;
    if kit::IconButton::new(Icon::FilmSlate, if src { "Back to the program (⇧4)" } else { "Source monitor (⇧2): mark in/out, then , insert or . overwrite" }).active(src).show(bar).clicked() {
        if src {
            app.editor.cutting.show_source = false;
        } else if app.editor.cutting.source.is_some() {
            app.editor.cutting.show_source = true;
        } else {
            let seq = app.doc.seq();
            if let Some(hit) = ed::match_frame(&app.doc.model, seq, app.editor.playhead, None) {
                open_source(app, hit.source, hit.source_time, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tracking::test_movie;
    use serde_json::json;

    fn app_with_cut() -> (StudioApp, egui::Context, eframe::Frame) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Cut", 320, 180, 24.0));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()]})).unwrap();
        let media = s.doc.model.media[0].id.0;
        s.edit(&[json!({"op": "overwrite", "media": media, "src_in": 0, "src_out": 0.5, "at": 0}), json!({"op": "overwrite", "media": media, "src_in": 0.6, "src_out": 1.2, "at": 0.5})], "cut", false).unwrap();
        app.doc = s.doc;
        (app, ctx, eframe::Frame::_new_kittest())
    }

    fn draw(app: &mut StudioApp, ctx: &egui::Context, frame: &mut eframe::Frame, t: f64, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), time: Some(t), events, ..Default::default() };
        // One headless frame, finished as a window's backend would.
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
        let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.textures_delta.clear();
    }

    fn key(k: egui::Key, shift: bool) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE } }
    }

    fn spine(app: &StudioApp) -> Vec<(Time, Time)> {
        app.doc.model.active().spine_clips().iter().map(|c| (c.start, c.duration)).collect()
    }

    #[test]
    fn source_monitor_marks_and_cuts_in_with_keys() {
        let _guard = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut app, ctx, mut fr) = app_with_cut();
        draw(&mut app, &ctx, &mut fr, 0.0, vec![]);
        // Match frame at 0.7 s opens the source on 0.8 s of the file.
        app.editor.playhead = 700_000;
        draw(&mut app, &ctx, &mut fr, 0.1, vec![key(egui::Key::F, true)]);
        let s = app.editor.cutting.source.as_ref().expect("source loaded");
        assert!(app.editor.cutting.show_source);
        assert_eq!((s.playhead, s.mark_in), (800_000, Some(800_000)));
        // Out mark 0.3 s later (arrows step the source), then insert at
        // the cut with `,`.
        app.editor.cutting.source.as_mut().unwrap().playhead = 1_100_000;
        draw(&mut app, &ctx, &mut fr, 0.2, vec![key(egui::Key::O, false)]);
        app.editor.playhead = 500_000;
        draw(&mut app, &ctx, &mut fr, 0.3, vec![key(egui::Key::Comma, false)]);
        assert_eq!(spine(&app), vec![(0, 500_000), (500_000, 300_000), (800_000, 600_000)]);
        assert_eq!(app.editor.playhead, 800_000);
        assert!(!app.editor.cutting.show_source);
        // Mark the inserted piece and extract it with `'`.
        app.doc.edit("range", |p| p.range = Some((500_000, 800_000)));
        draw(&mut app, &ctx, &mut fr, 0.4, vec![key(egui::Key::Quote, false)]);
        assert_eq!(spine(&app), vec![(0, 500_000), (500_000, 600_000)]);
        // Undo brings it back.
        app.doc.undo();
        assert_eq!(spine(&app).len(), 3);
    }

    #[test]
    fn trim_modes_extend_and_dual_up() {
        let _guard = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut app, ctx, mut fr) = app_with_cut();
        draw(&mut app, &ctx, &mut fr, 0.0, vec![]);
        // T: ripple mode; an edit point on the first clip's end; `]` trims
        // a frame and the rest follows.
        draw(&mut app, &ctx, &mut fr, 0.1, vec![key(egui::Key::T, false)]);
        assert_eq!(app.editor.cutting.mode, TrimMode::Ripple);
        let first = app.doc.model.active().spine_clips()[0].id.0 as u32;
        app.editor.cutting.point = Some(TrimPoint { clip: first, left: false });
        assert!(trim_frames(&app).is_some(), "the viewer shows the two sides of the cut");
        draw(&mut app, &ctx, &mut fr, 0.2, vec![key(egui::Key::CloseBracket, false)]);
        let f = app.editor.frame_dur(24.0);
        assert_eq!(spine(&app)[0].1, 500_000 + f);
        assert_eq!(app.doc.model.active().spine_end(), 1_100_000 + f);
        // T again: roll; `[` moves the cut back without changing the length.
        draw(&mut app, &ctx, &mut fr, 0.3, vec![key(egui::Key::T, false), key(egui::Key::OpenBracket, false)]);
        assert_eq!(app.editor.cutting.mode, TrimMode::Roll);
        assert_eq!(spine(&app)[0].1, 500_000);
        assert_eq!(app.doc.model.active().spine_end(), 1_100_000 + f);
        // JKL: play over the point and stop: the cut moves by the distance.
        app.editor.playhead = 500_000;
        app.editor.playing = true;
        draw(&mut app, &ctx, &mut fr, 0.4, vec![]);
        let from = app.editor.cutting.jkl_from.expect("playing over the edit point starts a JKL trim");
        app.editor.playhead = 600_000;
        app.editor.playing = false;
        draw(&mut app, &ctx, &mut fr, 0.5, vec![]);
        assert_eq!(spine(&app)[0].1, 500_000 + 600_000 - from);
        // E: extend the selected edit to the playhead.
        app.editor.playhead = 400_000;
        draw(&mut app, &ctx, &mut fr, 0.6, vec![key(egui::Key::E, false)]);
        assert_eq!(spine(&app)[0].1, 400_000);
        // Lane toggles: sync lock the music lane through the model op.
        let music = app.doc.model.active().lanes.iter().find(|l| l.kind == m::LaneKind::Audio).unwrap().id;
        let seq = app.doc.seq();
        let e = ed::set_sync_lock(&app.doc.model, seq, music, true).unwrap();
        app.doc.apply("Sync lock", e);
        assert!(ed::sync_locked(app.doc.model.active().lane(music).unwrap()));
        draw(&mut app, &ctx, &mut fr, 0.7, vec![]);
    }
}
