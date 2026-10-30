// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion tracking and stabilizing in the editor: the canvas Track tool and
//! the Inspector's "Tracking" section (`services::tracking` does the work).
//!
//! With the tool on Point, a click on the selected video clip tracks that
//! point (the ring shows how much picture around it is followed); on
//! Plane, a drag draws the surface's box. Tracking runs in the background
//! with progress in the Inspector. The selected track is drawn as its path
//! around the playhead, green where the tracker was sure and red where it
//! was not; with the tool on Correct, dragging its point (or a plane's
//! corner) at the playhead puts it right there and re-tracks from that
//! frame, and every layer that follows it moves along.

use std::sync::Mutex;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_ai::features::track::StabilizeSettings;
use studio_ai::{JobHandle, Jobs};
use studio_model as m;

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit;
use crate::services::tracking::{self as tracking, FollowMode, Kind, Samples, StabilizeJob, TrackJob};
use crate::theme;
use crate::view::{ClipKind, Time, SEC};

/// What the Track tool does with the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackTool {
    #[default]
    Off,
    /// A click tracks a point.
    Point,
    /// A drag draws a surface to track.
    Plane,
    /// Drag the selected track's point or corners to correct it.
    Correct,
}

fn tool_id() -> egui::Id {
    egui::Id::unique("canvas-track-tool")
}

pub fn tool(ctx: &egui::Context) -> TrackTool {
    ctx.data(|d| d.get_temp(tool_id())).unwrap_or_default()
}

pub fn set_tool(ctx: &egui::Context, t: TrackTool) {
    ctx.data_mut(|d| d.insert_temp(tool_id(), t));
}

/// The track shown on the canvas: (clip, track).
fn shown(ctx: &egui::Context) -> Option<(u32, u32)> {
    ctx.data(|d| d.get_temp(tool_id().with("shown")))
}

fn set_shown(ctx: &egui::Context, v: Option<(u32, u32)>) {
    ctx.data_mut(|d| match v {
        Some(v) => {
            d.insert_temp(tool_id().with("shown"), v);
        }
        None => d.remove::<(u32, u32)>(tool_id().with("shown")),
    });
}

/// Region size for a new point track (canvas pixels).
fn radius(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp(tool_id().with("radius"))).unwrap_or(40.0)
}

enum Running {
    Track { job: TrackJob, fixed: bool, handle: JobHandle<Samples> },
    Stabilize { job: StabilizeJob, handle: JobHandle<(tracking::CameraPath, f32)> },
}

impl Running {
    fn clip(&self) -> u32 {
        match self {
            Running::Track { job, .. } => job.clip,
            Running::Stabilize { job, .. } => job.clip,
        }
    }
    fn progress(&self) -> f32 {
        match self {
            Running::Track { handle, .. } => handle.progress(),
            Running::Stabilize { handle, .. } => handle.progress(),
        }
    }
    fn finished(&self) -> bool {
        match self {
            Running::Track { handle, .. } => handle.is_finished(),
            Running::Stabilize { handle, .. } => handle.is_finished(),
        }
    }
    fn cancel(&self) {
        match self {
            Running::Track { handle, .. } => handle.cancel(),
            Running::Stabilize { handle, .. } => handle.cancel(),
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Running::Track { fixed: true, .. } => "Re-tracking",
            Running::Track { .. } => "Tracking",
            Running::Stabilize { .. } => "Stabilizing",
        }
    }
}

fn jobs() -> &'static Jobs {
    static J: std::sync::OnceLock<Jobs> = std::sync::OnceLock::new();
    J.get_or_init(|| Jobs::new(1))
}

fn running() -> &'static Mutex<Option<Running>> {
    static R: Mutex<Option<Running>> = Mutex::new(None);
    &R
}

fn busy() -> bool {
    running().lock().map(|r| r.is_some()).unwrap_or(false)
}

fn start_track(ctx: &egui::Context, job: TrackJob, fixed: bool) {
    let j = job.clone();
    let handle = jobs().spawn(if fixed { "Re-track" } else { "Track" }, move |c| j.run(c).map_err(studio_ai::Error::Io));
    if let Ok(mut r) = running().lock() {
        *r = Some(Running::Track { job, fixed, handle });
    }
    ctx.request_repaint();
}

fn start_stabilize(ctx: &egui::Context, job: StabilizeJob) {
    let j = job.clone();
    let handle = jobs().spawn("Stabilize", move |c| j.run(c).map_err(studio_ai::Error::Io));
    if let Ok(mut r) = running().lock() {
        *r = Some(Running::Stabilize { job, handle });
    }
    ctx.request_repaint();
}

/// Applies a finished job to the document (one undo step).
pub fn poll(app: &mut StudioApp, ctx: &egui::Context) {
    let done = {
        let Ok(mut r) = running().lock() else { return };
        match r.as_ref() {
            Some(x) if x.finished() => r.take(),
            Some(_) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
                None
            }
            None => None,
        }
    };
    let Some(done) = done else { return };
    let seq = app.doc.seq();
    match done {
        Running::Track { job, fixed, handle } => match handle.take_result() {
            Some(Ok(samples)) => {
                let Some(mut mc) = app.doc.model.active().clip(m::ClipId(job.clip as u64)).cloned() else { return };
                let tid = tracking::store(&mut mc, &job, &samples, fixed);
                let mut model = app.doc.model.clone();
                if let Some(s) = model.sequences.iter_mut().find(|s| s.id == seq)
                    && let Some(c) = s.clips.iter_mut().find(|c| c.id == mc.id)
                {
                    *c = mc.clone();
                }
                let followers = tracking::refreshed_followers(&app.doc.project, &model, job.clip);
                let mut edits = vec![m::Edit::SetClip { seq, clip: Box::new(mc.clone()) }];
                edits.extend(followers.into_iter().map(|c| m::Edit::SetClip { seq, clip: Box::new(c) }));
                app.doc.apply(if fixed { "Correct track" } else { "Track" }, m::Edit::Batch { edits });
                set_shown(ctx, Some((job.clip, tid)));
                let said = tracking::track_of(&mc, tid).map(|t| tracking::describe(&t)).unwrap_or_default();
                kit::push(ctx, said, None, Some(Icon::Selection));
                if !fixed {
                    set_tool(ctx, TrackTool::Correct);
                }
            }
            Some(Err(studio_ai::Error::Cancelled)) => {
                kit::push(ctx, "Tracking stopped", None, Some(Icon::Info));
            }
            Some(Err(e)) => {
                kit::push(ctx, crate::i18n::tf(format!("Tracking failed: {e}")), None, Some(Icon::Warning));
            }
            None => {}
        },
        Running::Stabilize { job, handle } => match handle.take_result() {
            Some(Ok((path, agree))) => {
                if job.track.is_none() && agree < 0.15 {
                    kit::push(ctx, "The camera's motion could not be measured here (too dark, blurred or plain). Track a detailed point and stabilize on it.", None, Some(Icon::Warning));
                    return;
                }
                match tracking::apply_stabilize(&app.doc.project, &app.doc.model, &job, &path) {
                    Ok((c, info)) => {
                        app.doc.apply("Stabilize", m::Edit::SetClip { seq, clip: Box::new(c) });
                        let eased = if info.strength < 0.999 { format!(", eased to {:.0}% to stay in the zoom limit", info.strength * 100.0) } else { String::new() };
                        kit::push(ctx, crate::i18n::tf(format!("Stabilized (zoom {:.0}%{eased})", info.zoom * 100.0)), None, Some(Icon::Sparkle));
                    }
                    Err(e) => {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
                }
            }
            Some(Err(studio_ai::Error::Cancelled)) => {
                kit::push(ctx, "Stabilizing stopped", None, Some(Icon::Info));
            }
            Some(Err(e)) => {
                kit::push(ctx, crate::i18n::tf(format!("Stabilizing failed: {e}")), None, Some(Icon::Warning));
            }
            None => {}
        },
    }
}

/// Green when sure, through yellow, to red.
fn confidence_colour(c: f32) -> Color32 {
    let c = c.clamp(0.0, 1.0);
    if c > 0.5 {
        let k = (c - 0.5) * 2.0;
        Color32::from_rgb((250.0 * (1.0 - k) + 74.0 * k) as u8, (204.0 * (1.0 - k) + 222.0 * k) as u8, (21.0 * (1.0 - k) + 128.0 * k) as u8)
    } else {
        let k = c * 2.0;
        Color32::from_rgb((239.0 * (1.0 - k) + 250.0 * k) as u8, (68.0 * (1.0 - k) + 204.0 * k) as u8, (68.0 * (1.0 - k) + 21.0 * k) as u8)
    }
}

/// The canvas part: draws the shown track, starts tracks and takes
/// corrections. Returns whether the tool has the pointer.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, area: Rect, scale: f32) -> bool {
    let ctx = ui.ctx().clone();
    poll(app, &ctx);
    let mut tool = tool(&ctx);
    if tool != TrackTool::Off && (ui.input(|i| i.key_pressed(egui::Key::Escape)) || app.editor.selection.is_empty()) {
        set_tool(&ctx, TrackTool::Off);
        tool = TrackTool::Off;
    }
    let Some(id) = app.editor.selection.first().copied() else { return false };
    let Some(vc) = app.doc.project.clip(id).cloned() else { return false };
    let now = app.editor.playhead.clamp(vc.start, vc.start + vc.dur - 1);
    let screen = |c: Vec2| canvas.center() + c * scale;
    let from_screen = |p: Pos2| (p - canvas.center()) / scale;
    let p = ui.painter().with_clip_rect(area);
    let frame = (SEC as f64 / (app.doc.project.fps as f64).max(1.0)) as Time;

    // The track on show: its path for 1.5 s either side, then where it is
    // now.
    let shown_track = shown(&ctx).filter(|(c, _)| *c == id).and_then(|(c, t)| app.doc.model.active().clip(m::ClipId(c as u64)).and_then(|mc| tracking::track_of(mc, t)));
    let mut handles: Vec<Pos2> = Vec::new();
    if let Some(tr) = &shown_track {
        let mut prev: Option<Pos2> = None;
        let mut t = (now - SEC * 3 / 2).max(vc.start);
        while t <= (now + SEC * 3 / 2).min(vc.start + vc.dur - 1) {
            if let Some((pts, c, lost)) = tracking::canvas_points(&app.doc.project, &app.doc.model, id, tr, t) {
                let q = screen(pts[0]);
                if let Some(a) = prev {
                    p.line_segment([a, q], Stroke::new(2.0, if lost { Color32::from_rgb(239, 68, 68) } else { confidence_colour(c) }));
                }
                prev = Some(q);
            }
            t += frame;
        }
        if let Some((pts, c, lost)) = tracking::canvas_points(&app.doc.project, &app.doc.model, id, tr, now) {
            let col = if lost { Color32::from_rgb(239, 68, 68) } else { confidence_colour(c) };
            match tr.kind {
                Kind::Point => {
                    let q = screen(pts[0]);
                    let r = (pts[1] - pts[0]).length() * scale;
                    p.circle_stroke(q, r.max(6.0), Stroke::new(1.0, col.gamma_multiply(0.7)));
                    p.line_segment([q - Vec2::new(7.0, 0.0), q + Vec2::new(7.0, 0.0)], Stroke::new(1.5, col));
                    p.line_segment([q - Vec2::new(0.0, 7.0), q + Vec2::new(0.0, 7.0)], Stroke::new(1.5, col));
                    handles.push(q);
                }
                Kind::Plane => {
                    let q: Vec<Pos2> = pts.iter().map(|c| screen(*c)).collect();
                    for k in 0..4 {
                        p.line_segment([q[k], q[(k + 1) % 4]], Stroke::new(1.5, col));
                    }
                    for c in &q {
                        p.circle_filled(*c, 4.0, col);
                    }
                    handles.extend(q);
                }
            }
        }
    }

    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return false };
    let trackable = matches!(vc.kind, ClipKind::Video) && !busy();
    match tool {
        TrackTool::Off => false,
        TrackTool::Point => {
            let resp = ui.interact(area, egui::Id::unique("canvas-track-point"), Sense::click());
            if let Some(h) = resp.hover_pos() {
                p.circle_stroke(h, radius(&ctx) * scale, Stroke::new(1.5, Color32::from_rgb(74, 222, 128)));
                p.circle_filled(h, 2.5, Color32::from_rgb(74, 222, 128));
            }
            if resp.clicked() && trackable
                && let Some(h) = resp.interact_pointer_pos()
            {
                match tracking::plan_new(&app.doc.project, &app.doc.model, id, Kind::Point, now, &[from_screen(h)], radius(&ctx), None, "") {
                    Ok(job) => start_track(&ctx, job, false),
                    Err(e) => {
                        kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
            true
        }
        TrackTool::Plane => {
            let drag_id = egui::Id::unique("canvas-track-plane-drag");
            let resp = ui.interact(area, egui::Id::unique("canvas-track-plane"), Sense::drag());
            if resp.drag_started()
                && let Some(h) = resp.interact_pointer_pos()
            {
                ui.data_mut(|d| d.insert_temp(drag_id, h));
            }
            let start: Option<Pos2> = ui.data(|d| d.get_temp(drag_id));
            if let (Some(a), Some(b)) = (start, resp.interact_pointer_pos().or(resp.hover_pos())) {
                let r = Rect::from_two_pos(a, b);
                p.rect_stroke(r, 0, Stroke::new(1.5, Color32::from_rgb(74, 222, 128)), egui::StrokeKind::Middle);
                if resp.drag_stopped() {
                    ui.data_mut(|d| d.remove::<Pos2>(drag_id));
                    if r.width() > 8.0 && r.height() > 8.0 && trackable {
                        let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(from_screen);
                        match tracking::plan_new(&app.doc.project, &app.doc.model, id, Kind::Plane, now, &corners, 0.0, None, "") {
                            Ok(job) => start_track(&ctx, job, false),
                            Err(e) => {
                                kit::push(&ctx, e, None, Some(Icon::Warning));
                            }
                        }
                    }
                }
            }
            true
        }
        TrackTool::Correct => {
            let Some(tr) = shown_track else { return false };
            let grab_id = egui::Id::unique("canvas-track-grab");
            let mut grab: Option<(usize, Vec<Vec2>)> = ui.data(|d| d.get_temp(grab_id));
            for (k, at) in handles.iter().enumerate() {
                let resp = ui.interact(Rect::from_center_size(*at, Vec2::splat(18.0)), egui::Id::unique(("canvas-track-handle", id, k)), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    p.circle_stroke(*at, 8.0, Stroke::new(1.5, Color32::WHITE));
                }
                if resp.drag_started() {
                    let pts = tracking::canvas_points(&app.doc.project, &app.doc.model, id, &tr, now).map(|x| x.0).unwrap_or_default();
                    grab = Some((k, pts));
                }
                if resp.dragged()
                    && let Some((g, pts)) = grab.as_mut()
                    && *g == k
                    && let Some(h) = resp.interact_pointer_pos()
                {
                    let i = if tr.kind == Kind::Point { 0 } else { k };
                    if let Some(q) = pts.get_mut(i) {
                        *q = from_screen(h);
                    }
                    p.circle_stroke(h, 6.0, Stroke::new(2.0, Color32::from_rgb(74, 222, 128)));
                }
                if resp.drag_stopped()
                    && let Some((_, pts)) = grab.take()
                {
                    let points: Vec<Vec2> = if tr.kind == Kind::Point { pts.into_iter().take(1).collect() } else { pts };
                    if !busy() {
                        match tracking::plan_fix(&app.doc.project, &app.doc.model, id, tr.id, now, Some(&points), false) {
                            Ok(job) => start_track(&ctx, job, true),
                            Err(e) => {
                                kit::push(&ctx, e, None, Some(Icon::Warning));
                            }
                        }
                    }
                }
            }
            ui.data_mut(|d| match &grab {
                Some(g) => {
                    d.insert_temp(grab_id, g.clone());
                }
                None => d.remove::<(usize, Vec<Vec2>)>(grab_id),
            });
            let _ = mc;
            !handles.is_empty()
        }
    }
}

/// The Inspector's "Tracking" section for clip `id`.
pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    use crate::kit::{Button, Section, Segment};
    let Some(vc) = app.doc.project.clip(id).cloned() else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let video = matches!(vc.kind, ClipKind::Video) && matches!(&mc.source, m::ClipSource::Media(_));
    let follow = tracking::follow_of(&mc);
    if !video && follow.is_none() {
        return;
    }
    let ctx = ui.ctx().clone();
    let list = tracking::tracks(&mc);
    let seq = app.doc.seq();
    Section::new("tracking", "Tracking").icon(Icon::Selection).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        let w = ui.available_width();
        if let Some(f) = &follow {
            let name = app.doc.project.clip(f.clip).map(|c| c.name.clone()).unwrap_or_default();
            ui.label(egui::RichText::new(crate::i18n::tf(format!("Follows track {} of “{name}” ({})", f.track, f.mode.name()))).font(theme::body()).color(t.text));
            if Button::secondary("Stop following").icon(Icon::X).show(ui).clicked() {
                let mut c = mc.clone();
                tracking::detach(&mut c, false);
                app.doc.apply("Stop following", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
        }
        if !video {
            return;
        }
        // The job in progress.
        let job = running().lock().ok().and_then(|r| r.as_ref().filter(|x| x.clip() == id).map(|x| (x.label(), x.progress())));
        if let Some((label, pr)) = job {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{label}… {:.0}%", pr * 100.0)).font(theme::body()).color(t.text_2));
                if Button::ghost("Stop").show(ui).clicked()
                    && let Ok(r) = running().lock()
                    && let Some(x) = r.as_ref()
                {
                    x.cancel();
                }
            });
        }
        let tool_now = tool(&ctx);
        let mut cur = match tool_now {
            TrackTool::Off => 0,
            TrackTool::Point => 1,
            TrackTool::Plane => 2,
            TrackTool::Correct => 3,
        };
        let mut opts = vec![Segment::text("Off"), Segment::text("Point"), Segment::text("Plane")];
        if !list.is_empty() {
            opts.push(Segment::text("Correct"));
        }
        if kit::segmented(ui, "track-tool", &mut cur, &opts, Some(w), None).changed() {
            let next = [TrackTool::Off, TrackTool::Point, TrackTool::Plane, TrackTool::Correct][cur.min(3)];
            if next != TrackTool::Off {
                crate::editor::pins::set_tool(&ctx, crate::editor::pins::PinTool::Off);
            }
            set_tool(&ctx, next);
        }
        let hint = match tool_now {
            TrackTool::Point => "Click a point (eye, logo, corner) to track it through the clip",
            TrackTool::Plane => "Drag a box over a flat surface; its four corners are tracked",
            TrackTool::Correct => "Drag the point (or corner) to its spot at the playhead",
            TrackTool::Off => "",
        };
        if !hint.is_empty() {
            ui.label(egui::RichText::new(hint).font(theme::caption()).color(t.text_2));
        }
        if tool_now == TrackTool::Point {
            let mut r = radius(&ctx);
            if kit::Slider::new("Size", &mut r, 8.0..=200.0).default(40.0).unit("px").decimals(0).show(ui).changed() {
                ctx.data_mut(|d| d.insert_temp(tool_id().with("radius"), r));
            }
        }

        // The tracks.
        let shown_now = shown(&ctx).filter(|(c, _)| *c == id).map(|(_, t)| t);
        for tr in &list {
            ui.add_space(4.0);
            let (c, lost) = tr.quality();
            ui.horizontal_wrapped(|ui| {
                let sel = shown_now == Some(tr.id);
                if kit::chip(ui, &tr.name, sel, None).clicked() {
                    set_shown(&ctx, if sel { None } else { Some((id, tr.id)) });
                }
                let col = confidence_colour(c);
                ui.label(egui::RichText::new(format!("{:.0}%{}", c * 100.0, if lost > 0 { format!(" · lost {lost}") } else { String::new() })).font(theme::caption()).color(col));
            });
            if shown_now == Some(tr.id) {
                ui.horizontal_wrapped(|ui| {
                    if Button::secondary("Re-track from here").icon(Icon::Repeat).compact().show(ui).clicked() && !busy() {
                        match tracking::plan_fix(&app.doc.project, &app.doc.model, id, tr.id, app.editor.playhead, None, false) {
                            Ok(job) => start_track(&ctx, job, false),
                            Err(e) => {
                                kit::push(&ctx, e, None, Some(Icon::Warning));
                            }
                        }
                    }
                    if Button::ghost("Remove").icon(Icon::Trash).compact().show(ui).clicked() {
                        let changed = tracking::remove_track(&app.doc.model, id, tr.id);
                        app.doc.apply("Remove track", m::Edit::Batch { edits: changed.into_iter().map(|c| m::Edit::SetClip { seq, clip: Box::new(c) }).collect() });
                        set_shown(&ctx, None);
                    }
                });
                // Attach another layer.
                let others: Vec<(u32, String)> = app
                    .doc
                    .project
                    .clips
                    .iter()
                    .filter(|c| c.id != id && !matches!(c.kind, ClipKind::Audio | ClipKind::Fx) && c.start < vc.start + vc.dur && c.start + c.dur > vc.start)
                    .map(|c| (c.id, if c.name.is_empty() { crate::i18n::tf(format!("Clip {}", c.id)) } else { c.name.clone() }))
                    .collect();
                if !others.is_empty() {
                    let pick_id = tool_id().with(("attach", id, tr.id));
                    let (mut who, mut how): (usize, usize) = ctx.data(|d| d.get_temp(pick_id)).unwrap_or((0, if tr.kind == Kind::Plane { 2 } else { 0 }));
                    who = who.min(others.len() - 1);
                    let names: Vec<&str> = others.iter().map(|o| o.1.as_str()).collect();
                    kit::labeled_row(ui, "Layer", |ui| {
                        if let Some(i) = kit::menu_button(ui, "track-attach-who", who, &names, ui.available_width(), None) {
                            who = i;
                        }
                    });
                    let modes: &[&str] = if tr.kind == Kind::Plane { &["Position", "Transform", "Corners", "Mask"] } else { &["Position", "Transform", "Mask"] };
                    let mut m_i = how.min(modes.len() - 1);
                    kit::labeled_row(ui, "Follow", |ui| {
                        if let Some(i) = kit::menu_button(ui, "track-attach-how", m_i, modes, ui.available_width(), None) {
                            m_i = i;
                        }
                    });
                    how = m_i;
                    ctx.data_mut(|d| d.insert_temp(pick_id, (who, how)));
                    if Button::primary("Attach").icon(Icon::LinkSimple).show(ui).clicked() {
                        let follower = others[who].0;
                        let fc = app.doc.model.active().clip(m::ClipId(follower as u64)).cloned();
                        let mode = match modes[how] {
                            "Position" => Ok(FollowMode::Position),
                            "Transform" => Ok(FollowMode::Transform),
                            "Corners" => Ok(FollowMode::Corners),
                            _ => fc.and_then(|c| c.masks.first().map(|k| FollowMode::Mask(k.id.0))).ok_or_else(|| "give that layer a mask first (Inspector ▸ Mask)".to_owned()),
                        };
                        match mode.and_then(|mode| tracking::attach(&app.doc.project, &app.doc.model, follower, id, tr.id, mode, app.editor.playhead)) {
                            Ok(c) => {
                                app.doc.apply("Follow track", m::Edit::SetClip { seq, clip: Box::new(c) });
                                kit::push(&ctx, format!("{} follows {}", others[who].1, tr.name), None, Some(Icon::LinkSimple));
                            }
                            Err(e) => {
                                kit::push(&ctx, e, None, Some(Icon::Warning));
                            }
                        }
                    }
                }
            }
        }

        // Stabilizing.
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::i18n::shown("Stabilize")).font(theme::control_medium()).color(t.text));
        let sid = tool_id().with(("stab", id));
        let (mut lock, mut smooth, mut zoom, mut rot): (bool, f32, f32, bool) = ctx.data(|d| d.get_temp(sid)).unwrap_or((false, 1.0, 115.0, true));
        let mut mode = usize::from(lock);
        if kit::segmented(ui, "stab-mode", &mut mode, &[Segment::text("Smooth"), Segment::text("Lock")], Some(w), None).changed() {
            lock = mode == 1;
        }
        if !lock {
            kit::Slider::new("Smoothness", &mut smooth, 0.1..=4.0).default(1.0).unit("s").decimals(1).show(ui);
        }
        kit::Slider::new("Max zoom", &mut zoom, 100.0..=150.0).default(115.0).unit("%").decimals(0).show(ui);
        kit::labeled_row(ui, "Rotation", |ui| {
            kit::toggle(ui, &mut rot, "Correct rotation", None);
        });
        ctx.data_mut(|d| d.insert_temp(sid, (lock, smooth, zoom, rot)));
        let done = tracking::stabilized(&mc);
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(if done.is_some() { "Stabilize again" } else { "Stabilize" }).icon(Icon::ArrowsOutCardinal).show(ui).clicked() && !busy() {
                let s = StabilizeSettings { lock, smoothness: smooth as f64, rotation: rot, scale: false, max_zoom: zoom as f64 / 100.0 };
                match tracking::plan_stabilize(&app.doc.project, &app.doc.model, id, s, None) {
                    Ok(job) => start_stabilize(&ctx, job),
                    Err(e) => {
                        kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
            if let Some(d) = &done {
                ui.label(egui::RichText::new(format!("zoom {:.0}%", d.zoom * 100.0)).font(theme::caption()).color(t.text_2));
                if Button::ghost("Remove").icon(Icon::X).compact().show(ui).clicked()
                    && let Some(c) = tracking::unstabilize(&mc)
                {
                    app.doc.apply("Remove stabilize", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tracking::test_movie::{self, FPS, H, W};
    use serde_json::json;

    fn app_with_movie() -> (egui::Context, StudioApp, u32) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Track", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        app.doc = s.doc;
        app.editor.selection = vec![id];
        (ctx, app, id)
    }

    fn wait_for_job(app: &mut StudioApp, ctx: &egui::Context) {
        for _ in 0..15000 {
            if !busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            poll(app, ctx);
        }
        panic!("the tracking job did not finish");
    }

    /// The real canvas tool, headless: a click tracks the square, the
    /// track's point can then be dragged to correct it, and the Inspector
    /// draws with it all.
    #[test]
    fn the_track_tool_tracks_a_click_and_takes_a_correction() {
        let _guard = crate::services::tracking::test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut app, id) = app_with_movie();
        app.editor.playhead = (15.0 / FPS as f64 * SEC as f64) as Time;
        set_tool(&ctx, TrackTool::Point);
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(W as f32, H as f32));
        let mut t = 0.0;
        let mut frame = |app: &mut StudioApp, events: Vec<egui::Event>| {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(canvas), time: Some(t), events, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| {
                interact(app, ui, canvas, canvas, 1.0);
            });
        };
        let press = |p: Pos2, down: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE };
        // The square's centre in frame 15, in screen pixels (canvas = screen).
        let (ox, oy) = test_movie::object(15);
        let (sx, sy) = test_movie::shake(15);
        let at = Pos2::new((ox + sx) as f32, (oy + sy) as f32);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![press(at, true)]);
        frame(&mut app, vec![press(at, false)]);
        wait_for_job(&mut app, &ctx);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        let tr = tracking::tracks(&mc).into_iter().next().expect("a track from the click");
        assert!(tr.quality().0 > 0.5, "{}", tracking::describe(&tr));
        // The tool moved on to Correct, with the track on show.
        assert_eq!(tool(&ctx), TrackTool::Correct);
        assert_eq!(shown(&ctx), Some((id, tr.id)));
        // Drag its point 30 px to the right at frame 15.
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![press(at, true)]);
        for k in 1..=6 {
            frame(&mut app, vec![egui::Event::PointerMoved(at + Vec2::new(5.0 * k as f32, 0.0))]);
        }
        frame(&mut app, vec![press(at + Vec2::new(30.0, 0.0), false)]);
        frame(&mut app, vec![]);
        wait_for_job(&mut app, &ctx);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        let tr = tracking::track_of(&mc, tr.id).unwrap();
        assert_eq!(tr.fixes, vec![15]);
        let (p, _, _) = tracking::canvas_points(&app.doc.project, &app.doc.model, id, &tr, app.editor.playhead).unwrap();
        let want = (ox + sx - W as f64 / 2.0 + 30.0) as f32;
        assert!((p[0].x - want).abs() < 2.0, "{:?} vs {want}", p[0]);
    }

    /// The whole editor, headless, with a tracked clip, a follower and a
    /// stabilize: the Tracking section and the canvas draw without trouble.
    #[test]
    fn the_editor_draws_the_tracking_section() {
        let _guard = crate::services::tracking::test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, app, id) = app_with_movie();
        let mut s = crate::agent::Session::new(app.doc);
        s.edit(&[json!({"op": "text", "text": "HI", "at": 0, "duration": 2}), json!({"op": "track", "clip": id, "at": 0.5, "point": [-90, -15], "radius": 26, "attach": "$1"}), json!({"op": "stabilize", "clip": id})], "t", false).unwrap();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        app.doc = s.doc;
        app.editor.selection = vec![id];
        set_shown(&ctx, Some((id, 1)));
        let mut frame = eframe::Frame::_new_kittest();
        for (k, tl) in [TrackTool::Off, TrackTool::Point, TrackTool::Plane, TrackTool::Correct].into_iter().enumerate() {
            set_tool(&ctx, tl);
            for i in 0..3 {
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some((k * 3 + i) as f64 / 60.0), ..Default::default() };
                crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            }
        }
        // The follower too.
        let title = app.doc.project.clips.iter().find(|c| c.media.is_none()).unwrap().id;
        app.editor.selection = vec![title];
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(1.0), ..Default::default() };
        crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap();
        assert_eq!(tracking::tracks(mc).len(), 1);
        assert!(tracking::stabilized(mc).is_some());
    }
}
