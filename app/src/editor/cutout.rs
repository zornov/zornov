// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background removal, object cut-outs and roto in the editor: the
//! Inspector's "Cut out" section, the canvas Pick tool and the AI panel's
//! "Remove background" (`services::cutout` does the work).
//!
//! With Pick on, a click on the selected video clip marks the object (⌥-
//! click marks what is not it, a drag draws a box round it); SAM shows
//! what it would take at once, tinted, on the frame at the playhead. Then
//! "Cut out" carries it through the clip as a matte, or "Roto" makes it a
//! path mask keyed through the clip that the Pins tool's Mask mode edits
//! point by point.

use std::sync::Mutex;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_ai::frame::Mask;
use studio_ai::{JobHandle, Jobs};
use studio_model as m;

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit;
use crate::services::cutout::{self, CutJob, CutResult, Pick};
use crate::theme;
use crate::view::{ClipKind, Time};

// TODO(kit): canvas marks for the object pick; the theme has no success colour
// or on-canvas outline token.
const MARK_IN: Color32 = Color32::from_rgb(74, 222, 128);
const MARK_EDGE: Color32 = Color32::BLACK;
/// How strongly the preview tints what would be taken (0..255).
const PREVIEW_ALPHA: f32 = 120.0;

/// What the Pick tool's clicks do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CutTool {
    #[default]
    Off,
    Pick,
}

fn tool_id() -> egui::Id {
    egui::Id::unique("canvas-cut-tool")
}

pub fn tool(ctx: &egui::Context) -> CutTool {
    ctx.data(|d| d.get_temp(tool_id())).unwrap_or_default()
}

pub fn set_tool(ctx: &egui::Context, t: CutTool) {
    ctx.data_mut(|d| d.insert_temp(tool_id(), t));
}

/// The pick being made: clip, the moment, and the clicks / box.
#[derive(Clone, Debug, Default)]
struct Picking {
    clip: u32,
    at: Time,
    pick: Pick,
    /// What SAM takes for it on that frame (matte pixels).
    preview: Option<std::sync::Arc<Mask>>,
    /// The pick the preview was made for (to know when it is stale).
    shown_for: usize,
    asked: usize,
}

fn picking() -> &'static Mutex<Option<Picking>> {
    static P: Mutex<Option<Picking>> = Mutex::new(None);
    &P
}

enum Running {
    Matte { job: CutJob, handle: JobHandle<CutResult> },
    Roto { job: CutJob, handle: JobHandle<Vec<(u64, Vec<(f32, f32)>)>> },
    Preview { clip: u32, version: usize, handle: JobHandle<Mask> },
}

fn jobs() -> &'static Jobs {
    static J: std::sync::OnceLock<Jobs> = std::sync::OnceLock::new();
    J.get_or_init(|| Jobs::new(1))
}

fn running() -> &'static Mutex<Vec<Running>> {
    static R: Mutex<Vec<Running>> = Mutex::new(Vec::new());
    &R
}

/// A long job (cut-out or roto) is running for `clip`: its name and
/// progress.
fn busy_with(clip: u32) -> Option<(&'static str, f32)> {
    let r = running().lock().ok()?;
    r.iter().find_map(|x| match x {
        Running::Matte { job, handle } if job.clip == clip => Some(("Cutting out", handle.progress())),
        Running::Roto { job, handle } if job.clip == clip => Some(("Rotoscoping", handle.progress())),
        _ => None,
    })
}

/// A cut-out or roto is running (the AI panel shows its tile busy).
pub fn busy() -> bool {
    long_busy()
}

fn long_busy() -> bool {
    running().lock().map(|r| r.iter().any(|x| !matches!(x, Running::Preview { .. }))).unwrap_or(false)
}

/// Starts removing the background (the people in the shot) of clip `id`.
pub fn start_people(app: &StudioApp, ctx: &egui::Context, id: u32) -> Result<String, String> {
    if long_busy() {
        return Err("A cut-out is already running".into());
    }
    let job = cutout::plan(&app.doc, id, None, 0.6, 1)?;
    cutout::models_ready(&job)?;
    let j = job.clone();
    let handle = jobs().spawn("Remove background", move |c| j.run_matte(c).map_err(studio_ai::Error::Io));
    if let Ok(mut r) = running().lock() {
        r.push(Running::Matte { job, handle });
    }
    ctx.request_repaint();
    Ok("Removing the background…".into())
}

fn start_object(app: &StudioApp, ctx: &egui::Context, roto: bool) -> Result<(), String> {
    if long_busy() {
        return Err("A cut-out is already running".into());
    }
    let p = picking().lock().ok().and_then(|p| p.clone()).ok_or("Pick the object first")?;
    let fps = (app.doc.project.fps as f64).max(1.0);
    let frames = app.doc.project.clip(p.clip).map_or(0.0, |c| c.dur as f64 / 1e6 * fps);
    let step = if !roto || frames <= 90.0 { 1 } else if frames <= 240.0 { 2 } else { 3 };
    let job = cutout::plan(&app.doc, p.clip, Some((&p.pick, p.at)), 0.6, step)?;
    cutout::models_ready(&job)?;
    let j = job.clone();
    let r = if roto {
        Running::Roto { job, handle: jobs().spawn("Roto", move |c| j.run_roto(32, c).map_err(studio_ai::Error::Io)) }
    } else {
        Running::Matte { job, handle: jobs().spawn("Cut out", move |c| j.run_matte(c).map_err(studio_ai::Error::Io)) }
    };
    if let Ok(mut q) = running().lock() {
        q.push(r);
    }
    ctx.request_repaint();
    Ok(())
}

/// Asks SAM for the current pick's mask on its frame.
fn ask_preview(app: &StudioApp) {
    let Some(p) = picking().lock().ok().and_then(|p| p.clone()) else { return };
    if p.pick.is_empty() || p.asked == p.shown_for && p.preview.is_some() {
        return;
    }
    let Ok(job) = cutout::plan(&app.doc, p.clip, Some((&p.pick, p.at)), 0.0, 1) else { return };
    if cutout::models_ready(&job).is_err() {
        return;
    }
    let version = p.asked;
    let handle = jobs().spawn("Pick", move |_| {
        let cutout::Subject::Object { at, prompts } = &job.subject else { return Err(studio_ai::Error::Invalid("no pick".into())) };
        let frame = job.frame(*at).map_err(studio_ai::Error::Io)?;
        let sam = studio_ai::features::segment::ObjectCutout::load(&studio_ai::ModelStore::default())?;
        Ok(sam.cut_out(&frame, prompts)?.mask)
    });
    if let Ok(mut r) = running().lock() {
        r.retain(|x| !matches!(x, Running::Preview { .. }));
        r.push(Running::Preview { clip: p.clip, version, handle });
    }
}

/// Applies finished jobs (called every frame by the canvas).
pub fn poll(app: &mut StudioApp, ctx: &egui::Context) {
    let done: Vec<Running> = {
        let Ok(mut r) = running().lock() else { return };
        if r.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
        let (fin, keep): (Vec<Running>, Vec<Running>) = std::mem::take(&mut *r).into_iter().partition(|x| match x {
            Running::Matte { handle, .. } => handle.is_finished(),
            Running::Roto { handle, .. } => handle.is_finished(),
            Running::Preview { handle, .. } => handle.is_finished(),
        });
        *r = keep;
        fin
    };
    let seq = app.doc.seq();
    for d in done {
        match d {
            Running::Matte { job, handle } => match handle.take_result() {
                Some(Ok(res)) => {
                    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(job.clip as u64)).cloned() else { continue };
                    cutout::store_matte(&mut mc, &job, &res, false);
                    let said = cutout::describe(&cutout::matte_of(&mc).unwrap_or_default());
                    app.doc.apply("Cut out", m::Edit::SetClip { seq, clip: Box::new(mc) });
                    clear_pick();
                    set_tool(ctx, CutTool::Off);
                    kit::push(ctx, capital(&said), None, Some(Icon::MagicWand));
                }
                Some(Err(studio_ai::Error::Cancelled)) => {
                    kit::push(ctx, "Cut-out stopped", None, Some(Icon::Info));
                }
                Some(Err(e)) => {
                    kit::push(ctx, crate::i18n::tf(format!("Cut-out failed: {e}")), None, Some(Icon::Warning));
                }
                None => {}
            },
            Running::Roto { job, handle } => match handle.take_result() {
                Some(Ok(outlines)) => {
                    let short = job.footage.width.min(job.footage.height).max(1) as f64;
                    match cutout::store_roto(&app.doc.project, &app.doc.model, job.clip, &job, &outlines, 4.0 / short, false) {
                        Ok((mc, _)) => {
                            let n = mc.masks.len();
                            app.doc.apply("Roto", m::Edit::SetClip { seq, clip: Box::new(mc) });
                            clear_pick();
                            set_tool(ctx, CutTool::Off);
                            kit::push(ctx, crate::i18n::tf(format!("Roto mask {n} added on {} frames, edit its points with Pins ▸ Mask", outlines.len())), None, Some(Icon::Path));
                        }
                        Err(e) => {
                            kit::push(ctx, e, None, Some(Icon::Warning));
                        }
                    }
                }
                Some(Err(studio_ai::Error::Cancelled)) => {
                    kit::push(ctx, "Roto stopped", None, Some(Icon::Info));
                }
                Some(Err(e)) => {
                    kit::push(ctx, crate::i18n::tf(format!("Roto failed: {e}")), None, Some(Icon::Warning));
                }
                None => {}
            },
            Running::Preview { clip, version, handle } => {
                if let Some(Ok(mask)) = handle.take_result()
                    && let Ok(mut p) = picking().lock()
                    && let Some(p) = p.as_mut()
                    && p.clip == clip
                {
                    p.preview = Some(std::sync::Arc::new(mask));
                    p.shown_for = version;
                }
            }
        }
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn clear_pick() {
    if let Ok(mut p) = picking().lock() {
        *p = None;
    }
}

/// The canvas part: the pick's marks and preview, and its clicks. Returns
/// whether the tool has the pointer.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, area: Rect, scale: f32) -> bool {
    let ctx = ui.ctx().clone();
    poll(app, &ctx);
    crate::editor::slowmo::poll(app, &ctx);
    let mut tool = tool(&ctx);
    if tool != CutTool::Off && (ui.input(|i| i.key_pressed(egui::Key::Escape)) || app.editor.selection.is_empty()) {
        set_tool(&ctx, CutTool::Off);
        clear_pick();
        tool = CutTool::Off;
    }
    if tool == CutTool::Off {
        return false;
    }
    let Some(id) = app.editor.selection.first().copied() else { return false };
    let Some(vc) = app.doc.project.clip(id).cloned() else { return false };
    if !matches!(vc.kind, ClipKind::Video) {
        return false;
    }
    let now = app.editor.playhead.clamp(vc.start, vc.start + vc.dur - 1);
    let screen = |c: Vec2| canvas.center() + c * scale;
    let from_screen = |p: Pos2| (p - canvas.center()) / scale;
    // A pick belongs to one clip and one moment: moving on starts afresh.
    {
        let mut g = picking().lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_none_or(|p| p.clip != id || p.at != now) {
            *g = Some(Picking { clip: id, at: now, ..Default::default() });
        }
    }
    let tk = theme::tokens(&ctx);
    let p = ui.painter().with_clip_rect(area);
    let cur = picking().lock().ok().and_then(|g| g.clone()).unwrap_or_default();
    // The preview: SAM's mask over the layer, tinted.
    if let (Some(mask), Some(geo)) = (&cur.preview, crate::editor::xform::geo(&app.doc.project, &app.doc.model, id, now)) {
        let (w, h) = (mask.width, mask.height);
        let [tr, tg, tb, _] = tk.select.to_srgba_unmultiplied();
        let px: Vec<Color32> = mask.data.iter().map(|&a| Color32::from_rgba_unmultiplied(tr, tg, tb, (a.clamp(0.0, 1.0) * PREVIEW_ALPHA) as u8)).collect();
        let img = egui::ColorImage::new([w, h], px);
        let tex = ctx.load_texture("cut-preview", img, egui::TextureOptions::LINEAR);
        let s = geo.source();
        let corners = [Vec2::ZERO, Vec2::new(s.x, 0.0), s, Vec2::new(0.0, s.y)].map(|c| screen(geo.to_canvas(c)));
        let uv = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
        let mut mesh = egui::Mesh::with_texture(tex.id());
        for k in 0..4 {
            mesh.vertices.push(egui::epaint::Vertex { pos: corners[k], uv: uv[k], color: Color32::WHITE });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
        p.add(egui::Shape::mesh(mesh));
    }
    for c in &cur.pick.inside {
        p.circle_filled(screen(*c), 5.0, MARK_IN);
        p.circle_stroke(screen(*c), 5.0, Stroke::new(1.5, MARK_EDGE));
    }
    for c in &cur.pick.outside {
        p.circle_filled(screen(*c), 5.0, tk.danger);
        p.circle_stroke(screen(*c), 5.0, Stroke::new(1.5, MARK_EDGE));
    }
    if let Some([a, b]) = cur.pick.corners {
        p.rect_stroke(Rect::from_two_pos(screen(a), screen(b)), 0, Stroke::new(1.5, MARK_IN), egui::StrokeKind::Middle);
    }
    let resp = ui.interact(area, egui::Id::unique("canvas-cut-pick"), Sense::click_and_drag());
    let drag_id = egui::Id::unique("canvas-cut-drag");
    if resp.drag_started()
        && let Some(h) = resp.interact_pointer_pos()
    {
        ui.data_mut(|d| d.insert_temp(drag_id, h));
    }
    let mut changed = false;
    if let Some(a) = ui.data(|d| d.get_temp::<Pos2>(drag_id)) {
        if let Some(b) = resp.interact_pointer_pos().or(resp.hover_pos()) {
            p.rect_stroke(Rect::from_two_pos(a, b), 0, Stroke::new(1.0, MARK_IN), egui::StrokeKind::Middle);
            if resp.drag_stopped() {
                ui.data_mut(|d| d.remove::<Pos2>(drag_id));
                if (b - a).length() > 8.0
                    && let Ok(mut g) = picking().lock()
                    && let Some(pk) = g.as_mut()
                {
                    pk.pick.corners = Some([from_screen(a), from_screen(b)]);
                    changed = true;
                }
            }
        }
    } else if resp.clicked()
        && let Some(h) = resp.interact_pointer_pos()
        && let Ok(mut g) = picking().lock()
        && let Some(pk) = g.as_mut()
    {
        if ui.input(|i| i.modifiers.alt) {
            pk.pick.outside.push(from_screen(h));
        } else {
            pk.pick.inside.push(from_screen(h));
        }
        changed = true;
    }
    if changed {
        if let Ok(mut g) = picking().lock()
            && let Some(pk) = g.as_mut()
        {
            pk.asked += 1;
        }
        ask_preview(app);
    }
    true
}

/// The Inspector's "Cut out" section for clip `id`.
pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    use crate::kit::{Button, Section};
    let Some(vc) = app.doc.project.clip(id).cloned() else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    if !(matches!(vc.kind, ClipKind::Video) && matches!(&mc.source, m::ClipSource::Media(_))) {
        return;
    }
    let ctx = ui.ctx().clone();
    let seq = app.doc.seq();
    let existing = cutout::matte_of(&mc);
    Section::new("cutout", "Cut out").icon(Icon::MagicWand).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        let store = studio_ai::ModelStore::default();
        let modnet = store.require(studio_ai::ModelId::Modnet).is_ok();
        let sam = store.require(studio_ai::ModelId::Sam21Tiny).is_ok();
        if let Some(o) = &existing {
            ui.label(egui::RichText::new(capital(&cutout::describe(o))).font(theme::caption()).color(t.text_2));
            let mut on = o.get("enabled").and_then(|v| v.as_bool()) != Some(false);
            let mut inv = o.get("invert").and_then(|v| v.as_bool()) == Some(true);
            kit::labeled_row(ui, "On", |ui| {
                if kit::toggle(ui, &mut on, "Cut-out on", None).changed() {
                    let mut c = mc.clone();
                    cutout::set_flag(&mut c, "enabled", on);
                    app.doc.apply("Cut-out on", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
            });
            kit::labeled_row(ui, "Invert", |ui| {
                if kit::toggle(ui, &mut inv, "Keep the background instead", None).changed() {
                    let mut c = mc.clone();
                    cutout::set_flag(&mut c, "invert", inv);
                    app.doc.apply("Invert cut-out", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
            });
            if Button::ghost("Remove cut-out").icon(Icon::X).compact().show(ui).clicked() {
                let mut c = mc.clone();
                c.meta.remove("matte");
                app.doc.apply("Remove cut-out", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
            ui.add_space(theme::space::GAP * 2.0);
        }
        if let Some((label, pr)) = busy_with(id) {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{label}… {:.0}%", pr * 100.0)).font(theme::body()).color(t.text_2));
                if Button::ghost("Stop").show(ui).clicked()
                    && let Ok(r) = running().lock()
                {
                    for x in r.iter() {
                        match x {
                            Running::Matte { handle, .. } => handle.cancel(),
                            Running::Roto { handle, .. } => handle.cancel(),
                            Running::Preview { .. } => {}
                        }
                    }
                }
            });
            return;
        }
        // People.
        let b = ui.add_enabled_ui(modnet, |ui| Button::secondary("Remove background").icon(Icon::Sparkle).show(ui)).inner;
        let b = kit::tip(b, if modnet { "Keep the people, drop everything behind them" } else { "Install the Remove background model in Settings ▸ Models" }, None);
        if b.clicked() {
            match start_people(app, &ctx, id) {
                Ok(msg) => {
                    kit::push(&ctx, msg, None, Some(Icon::Sparkle));
                }
                Err(e) => {
                    kit::push(&ctx, e, None, Some(Icon::Warning));
                }
            }
        }
        // An object.
        let mut picking_now = tool(&ctx) == CutTool::Pick;
        ui.add_enabled_ui(sam, |ui| {
            kit::labeled_row(ui, "Pick object", |ui| {
                if kit::toggle(ui, &mut picking_now, "Pick an object on the canvas", None).changed() {
                    if picking_now {
                        crate::editor::pins::set_tool(&ctx, crate::editor::pins::PinTool::Off);
                        crate::editor::track::set_tool(&ctx, crate::editor::track::TrackTool::Off);
                    } else {
                        clear_pick();
                    }
                    set_tool(&ctx, if picking_now { CutTool::Pick } else { CutTool::Off });
                }
            });
        });
        if !sam {
            ui.label(egui::RichText::new(crate::i18n::shown("Picking objects needs the Object cut-out model (Settings ▸ Models)")).font(theme::caption()).color(t.text_2));
        } else if picking_now {
            let have = picking().lock().ok().and_then(|p| p.as_ref().map(|p| !p.pick.is_empty())).unwrap_or(false);
            ui.label(egui::RichText::new(crate::i18n::shown("Click the object (⌥-click what is not it, or drag a box round it). The tinted area is what will be taken")).font(theme::caption()).color(t.text_2));
            ui.add_enabled_ui(have, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if Button::primary("Cut out").icon(Icon::MagicWand).compact().show(ui).clicked()
                        && let Err(e) = start_object(app, &ctx, false)
                    {
                        kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                    if Button::secondary("Roto mask").icon(Icon::Path).compact().show(ui).clicked()
                        && let Err(e) = start_object(app, &ctx, true)
                    {
                        kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                    if Button::ghost("Clear").compact().show(ui).clicked() {
                        clear_pick();
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tracking::test_movie::{self, FPS, H, W};
    use serde_json::json;

    /// The real Pick tool, headless: two clicks on the square ask SAM for a
    /// preview; Cut out runs and lands a matte on the clip; the whole editor
    /// draws the section before and after.
    #[test]
    fn the_pick_tool_previews_and_cuts_out() {
        let _guard = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if studio_ai::ModelStore::default().require(studio_ai::ModelId::Sam21Tiny).is_err() {
            eprintln!("SAM is not installed: skipped");
            return;
        }
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Cut", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": id, "end": 0.3})], "trim", false).unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![id];
        app.editor.playhead = (3.0 / FPS as f64 * 1e6) as Time;
        set_tool(&ctx, CutTool::Pick);
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(W as f32, H as f32));
        let mut t = 0.0;
        let mut frame = |app: &mut StudioApp, events: Vec<egui::Event>| {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(canvas), time: Some(t), events, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| {
                interact(app, ui, canvas, canvas, 1.0);
            });
        };
        let (ox, oy) = test_movie::object(3);
        let (sx, sy) = test_movie::shake(3);
        let at = Pos2::new((ox + sx) as f32, (oy + sy) as f32);
        let press = |p: Pos2, down: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE };
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![press(at, true)]);
        frame(&mut app, vec![press(at, false)]);
        frame(&mut app, vec![]);
        // The preview arrives. The waits only catch a job that never ends:
        // SAM is about a second a frame on its own, many times slower while
        // other tests (or other builds) share the machine.
        let wait = std::time::Duration::from_secs(1200);
        let deadline = std::time::Instant::now() + wait;
        while std::time::Instant::now() < deadline {
            if picking().lock().unwrap().as_ref().is_some_and(|p| p.preview.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame(&mut app, vec![]);
        }
        let prev = picking().lock().unwrap().as_ref().and_then(|p| p.preview.clone()).expect("a preview");
        let cov = prev.coverage();
        assert!(cov > 0.008 && cov < 0.03, "preview covers {cov}");
        start_object(&app, &ctx, false).unwrap();
        let deadline = std::time::Instant::now() + wait;
        while std::time::Instant::now() < deadline {
            if !long_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            poll(&mut app, &ctx);
        }
        poll(&mut app, &ctx);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap();
        assert!(cutout::matte_of(mc).is_some(), "the cut-out landed");
        // The whole editor with the section.
        let mut ef = eframe::Frame::_new_kittest();
        for i in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(10.0 + i as f64 / 60.0), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut ef));
        }
    }
}
