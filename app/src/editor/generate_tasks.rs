// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Generate card's "Change" side: the generative media tasks on the
//! selected clip (animate a still, two frames joined, restyle, relight,
//! time of day, season, extend, fill a region, upscale, enhance), by
//! Codex or this Mac, through the same engine as the AI's `generate` and
//! `enhance` tools (`services::genmedia`). The work runs in the
//! background; the result goes back into the clip (or, for a new clip, at
//! the playhead), with a note when Codex would have done better.

use std::sync::{Arc, Mutex};

use eframe::egui;
use serde_json::{Value, json};

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, Segment};
use super::super::physics::cloud_feedback as feedback;
use crate::services::genmedia::{self as g, Task};
use crate::theme;
use studio_model as m;

/// The tasks in the menu: the task, its name, what to select.
const TASKS: [(Task, &str, &str); 10] = [
    (Task::Animate, "Animate a still", "Select a picture, or a video at its frame"),
    (Task::Between, "Between two frames", "Select two clips: the first and last frame"),
    (Task::Restyle, "Restyle", "Select a clip. A second one sets the look"),
    (Task::Relight, "Relight", "Select a clip"),
    (Task::TimeOfDay, "Time of day", "Select a clip"),
    (Task::Season, "Season", "Select a clip"),
    (Task::Outpaint, "Extend the picture", "Select a picture clip, such as a backdrop"),
    (Task::Inpaint, "Fill a region", "Select a picture clip"),
    (Task::Upscale, "Upscale", "Select a clip"),
    (Task::Enhance, "Enhance and restore", "Select a clip"),
];
const PROVIDERS: [(&str, Option<g::Provider>); 3] = [("Best available", None), ("Codex", Some(g::Provider::Codex)), ("This Mac", Some(g::Provider::Local))];
// What the ops take, and (in the `_NAMES` beside them) what the menus show.
const DIRECTIONS: [&str; 8] = ["left", "right", "top", "top-left", "top-right", "bottom", "front", "back"];
const DIRECTION_NAMES: [&str; 8] = ["Left", "Right", "Top", "Top left", "Top right", "Bottom", "Front", "Back"];
const COLOURS: [&str; 8] = ["warm", "golden", "white", "cool", "blue", "moonlight", "red", "neon"];
const COLOUR_NAMES: [&str; 8] = ["Warm", "Golden", "White", "Cool", "Blue", "Moonlight", "Red", "Neon"];
const TIMES: [&str; 7] = ["dawn", "morning", "noon", "golden hour", "sunset", "blue hour", "night"];
const TIME_NAMES: [&str; 7] = ["Dawn", "Morning", "Noon", "Golden hour", "Sunset", "Blue hour", "Night"];
const SEASONS: [&str; 4] = ["spring", "summer", "autumn", "winter"];
const SEASON_NAMES: [&str; 4] = ["Spring", "Summer", "Autumn", "Winter"];
const MOTIONS: [&str; 9] = ["push_in", "pull_out", "pan_left", "pan_right", "rise", "fall", "orbit_left", "orbit_right", "dolly_zoom"];
const MOTION_NAMES: [&str; 9] = ["Push in", "Pull out", "Pan left", "Pan right", "Rise", "Fall", "Orbit left", "Orbit right", "Dolly zoom"];
const SIDES: [&str; 6] = ["Left and right", "Right", "Left", "Top and bottom", "Top", "Bottom"];
const FPS: [(&str, Option<f64>); 4] = [("Same as the clip", None), ("48 fps", Some(48.0)), ("50 fps", Some(50.0)), ("60 fps", Some(60.0))];

/// A task's name as the menu shows it, from its op name.
fn task_name(op: &str) -> String {
    TASKS.iter().find(|t| t.0.name() == op).map_or_else(|| feedback::shown_name(op), |t| t.1.to_owned())
}

/// A row for a 0..1 amount shown as a percentage.
fn percent_row(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32) {
    let mut pct = *v * 100.0;
    let (lo, hi) = (*range.start() * 100.0, *range.end() * 100.0);
    if FieldRow::new(label, &mut pct, lo..=hi).default(default * 100.0).unit("%").decimals(0).show(ui).changed {
        *v = pct / 100.0;
    }
}

#[derive(Clone, Debug)]
struct Form {
    task: usize,
    prompt: String,
    provider: usize,
    direction: usize,
    colour: usize,
    time: usize,
    season: usize,
    motion: usize,
    side: usize,
    grow: f32,
    region: [f32; 4],
    scale: f32,
    fps: usize,
    old_film: bool,
    faces: bool,
    seconds: f32,
    strength: f32,
    /// Takes to choose the best from (Codex pictures).
    takes: f32,
}

impl Default for Form {
    fn default() -> Self {
        Form { task: 0, prompt: String::new(), provider: 0, direction: 0, colour: 0, time: 4, season: 2, motion: 0, side: 0, grow: 0.5, region: [0.4, 0.4, 0.2, 0.2], scale: 2.0, fps: 0, old_film: false, faces: false, seconds: 5.0, strength: 1.0, takes: 1.0 }
    }
}

struct Run {
    status: String,
    done: Option<Result<(g::Request, g::Outcome), String>>,
    args: Value,
    /// The app that started it ([`crate::editor::owner_of`]).
    owner: usize,
}

fn form_id() -> egui::Id {
    egui::Id::unique("ai-generate-change")
}

fn mode_id() -> egui::Id {
    egui::Id::unique("ai-generate-mode")
}

fn run() -> &'static Mutex<Option<Arc<Mutex<Run>>>> {
    static R: Mutex<Option<Arc<Mutex<Run>>>> = Mutex::new(None);
    &R
}

/// The Make / Change switch; true for Change.
pub fn mode(ui: &mut egui::Ui, w: f32) -> bool {
    let ctx = ui.ctx().clone();
    let mut m: usize = ctx.data(|d| d.get_temp(mode_id())).unwrap_or(0);
    let segs = [Segment::text("Make"), Segment::text("Change")];
    kit::segmented(ui, "gen-mode", &mut m, &segs, Some(w), None);
    ctx.data_mut(|d| d.insert_temp(mode_id(), m));
    m == 1
}

/// The picture a clip shows: a still's file, or a video clip's frame under
/// the playhead (or at its start) saved as a PNG.
fn picture_of(app: &StudioApp, id: u32) -> Result<std::path::PathBuf, String> {
    let c = app.doc.project.clips.iter().find(|c| c.id == id).ok_or("the clip is gone")?;
    let mid = c.media.ok_or("that clip is not a picture or footage")?;
    let mi = app.doc.project.media(mid).ok_or("its media is gone")?;
    let path = mi.path.clone().ok_or("its media has no file")?;
    if mi.kind == crate::view::MediaKind::Image {
        return Ok(path);
    }
    let mc = app.doc.model.active().clip(m::ClipId(id as u64)).ok_or("the clip is gone")?;
    let t = app.editor.playhead.clamp(mc.start, mc.start + mc.duration - 1);
    let secs = mc.source_time_at(t) as f64 / 1e6;
    let img = crate::media_io::frame(&path, secs, mi.width as usize, mi.height as usize, false)?;
    let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    let f = studio_ai::Frame::new(img.size[0], img.size[1], rgba).map_err(|e| e.to_string())?;
    let out = std::env::temp_dir().join(format!("studio-panel-frame-{}-{id}.png", std::process::id()));
    g::save_png(&f, &out)?;
    Ok(out)
}

/// The `generate` arguments the form makes for the selection.
fn args_of(app: &StudioApp, f: &Form) -> Result<Value, String> {
    let (task, _, needs) = TASKS[f.task.min(TASKS.len() - 1)];
    let sel = &app.editor.selection;
    let first = *sel.first().ok_or(needs)?;
    let is_still = |id: u32| app.doc.project.clips.iter().find(|c| c.id == id).and_then(|c| c.media).and_then(|m| app.doc.project.media(m)).is_some_and(|mi| mi.kind == crate::view::MediaKind::Image);
    let mut a = json!({"task": task.name(), "prompt": f.prompt.trim()});
    if let Some(p) = PROVIDERS[f.provider.min(2)].1 {
        a["provider"] = json!(p.name());
    }
    if f.takes > 1.5 && !task.makes_video() && !matches!(task, Task::Upscale | Task::Enhance) {
        a["best_of"] = json!(f.takes.round());
    }
    match task {
        Task::Animate => {
            a["image"] = json!(picture_of(app, first)?.display().to_string());
            a["motion"] = json!(MOTIONS[f.motion.min(MOTIONS.len() - 1)]);
            a["seconds"] = json!(f.seconds.round());
            a["fps"] = json!(app.doc.project.fps);
            a["at"] = json!(app.editor.playhead as f64 / 1e6);
        }
        Task::Between => {
            let second = *sel.get(1).ok_or(needs)?;
            a["image"] = json!(picture_of(app, first)?.display().to_string());
            a["end_image"] = json!(picture_of(app, second)?.display().to_string());
            a["seconds"] = json!(f.seconds.round().max(1.0));
            a["fps"] = json!(app.doc.project.fps);
            a["at"] = json!(app.editor.playhead as f64 / 1e6);
        }
        _ => {
            a["clip"] = json!(first);
            if task == Task::Restyle {
                if let Some(&second) = sel.get(1) {
                    a["style_ref"] = json!(picture_of(app, second)?.display().to_string());
                }
                a["strength"] = json!(f.strength.min(1.0) * 0.85);
            }
        }
    }
    match task {
        Task::Relight => {
            a["direction"] = json!(DIRECTIONS[f.direction.min(7)]);
            a["colour"] = json!(COLOURS[f.colour.min(7)]);
            a["strength"] = json!(f.strength);
        }
        Task::TimeOfDay => {
            a["time"] = json!(TIMES[f.time.min(6)]);
            a["strength"] = json!(f.strength.min(1.0));
        }
        Task::Season => {
            a["season"] = json!(SEASONS[f.season.min(3)]);
            a["strength"] = json!(f.strength.min(1.0));
        }
        Task::Outpaint => {
            if !is_still(first) {
                return Err("Extend works on a picture clip (a backdrop, a photo)".into());
            }
            let g = f.grow;
            match f.side {
                0 => a["wider"] = json!(1.0 + g),
                1 => a["right"] = json!(g),
                2 => a["left"] = json!(g),
                3 => a["taller"] = json!(1.0 + g),
                4 => a["top"] = json!(g),
                _ => a["bottom"] = json!(g),
            }
        }
        Task::Inpaint => {
            if !is_still(first) {
                return Err("Fill a region works on a picture clip".into());
            }
            a["region"] = json!(f.region);
        }
        Task::Upscale => {
            a["scale"] = json!(f.scale);
        }
        Task::Enhance => {
            if f.old_film {
                a["old_film"] = json!(true);
            }
            if f.faces {
                a["faces"] = json!(true);
            }
            if f.scale > 1.01 {
                a["scale"] = json!(f.scale);
            }
            if let Some(fps) = FPS[f.fps.min(3)].1 {
                a["fps"] = json!(fps);
            }
        }
        _ => {}
    }
    Ok(a)
}

/// Builds the request on this thread and runs it on another.
fn start(app: &mut StudioApp, args: Value) -> Result<(), String> {
    let task = Task::parse(args["task"].as_str().unwrap_or("")).ok_or("no task")?;
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let req = s.request_for(task, &args);
    app.doc = s.doc;
    let req = req?;
    let state = Arc::new(Mutex::new(Run { status: crate::i18n::shown("Starting").into_owned(), done: None, args, owner: crate::editor::owner_of(app) }));
    if let Ok(mut gl) = run().lock() {
        *gl = Some(state.clone());
    }
    std::thread::spawn(move || {
        let st = state.clone();
        let r = g::run(&req, &studio_ai::JobCtx::detached(), &mut |msg| {
            if let Ok(mut g) = st.lock() {
                g.status = msg;
            }
        });
        if let Ok(mut g) = state.lock() {
            g.done = Some(r.map(|o| (req, o)));
        }
    });
    Ok(())
}

/// Lands a finished task (called every frame while the AI panel shows).
pub fn poll(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(r) = run().lock().ok().and_then(|g| g.clone()).filter(|r| r.lock().is_ok_and(|g| g.owner == crate::editor::owner_of(app))) else { return };
    let finished = r.lock().ok().and_then(|mut g| g.done.take().map(|d| (d, g.args.clone())));
    let Some((done, args)) = finished else {
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
        return;
    };
    if let Ok(mut g) = run().lock() {
        *g = None;
    }
    match done {
        Ok((req, out)) => {
            let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
            let mut s = crate::agent::Session::new(doc);
            let landed = s.land_task(&req, &out, &args);
            app.doc = s.doc;
            match landed {
                Ok(_) => {
                    let what = crate::i18n::tf(format!("{} done by {}", task_name(req.task.name()), out.by));
                    kit::push(ctx, out.note.map_or(what.clone(), |n| format!("{what}. {}", feedback::plain(&app.doc, &n))), None, Some(Icon::Sparkle));
                }
                Err(e) => {
                    kit::push(ctx, crate::i18n::tf(format!("Made, but not placed: {}", feedback::plain(&app.doc, &e))), None, Some(Icon::Warning));
                }
            }
        }
        Err(e) => {
            kit::push(ctx, crate::i18n::tf(format!("{} failed: {}", task_name(args["task"].as_str().unwrap_or("")), feedback::plain(&app.doc, &e))), None, Some(Icon::Warning));
        }
    }
}

fn menu(ui: &mut egui::Ui, label: &str, id: &str, cur: &mut usize, names: &[&str]) -> bool {
    let mut changed = false;
    kit::labeled_row(ui, label, |ui| {
        if let Some(i) = kit::menu_button(ui, id, *cur, names, ui.available_width(), None) {
            changed = i != *cur;
            *cur = i;
        }
    });
    changed
}

/// What the line under the task menu says: what to select, until enough
/// is.
fn selected_line(task: Task, needs: &str, sel: usize) -> String {
    let need = if task == Task::Between { 2 } else { 1 };
    match sel {
        n if n < need => needs.to_owned(),
        1 => "1 clip selected".to_owned(),
        n => crate::i18n::tf(format!("{n} clips selected")),
    }
}

/// The Change side of the card.
pub fn section(app: &mut StudioApp, ui: &mut egui::Ui, w: f32) {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let mut f: Form = ctx.data(|d| d.get_temp(form_id())).unwrap_or_default();
    let names: Vec<&str> = TASKS.iter().map(|t| t.1).collect();
    if menu(ui, "Task", "gen-task", &mut f.task, &names) {
        f.strength = 1.0;
    }
    let (task, _, needs) = TASKS[f.task.min(TASKS.len() - 1)];
    let here = g::Available::now();
    let sel = app.editor.selection.len();
    ui.label(egui::RichText::new(selected_line(task, needs, sel)).font(theme::caption()).color(t.text_2));
    let hint = match task {
        Task::Animate => Some("What happens as the camera moves"),
        Task::Restyle => Some("The style, such as oil painting"),
        Task::Relight => Some("More about the light (optional)"),
        Task::Outpaint => Some("What lies past the edges (optional)"),
        Task::Inpaint => Some("What to paint there. Empty sky removes"),
        Task::Between => Some("How one becomes the other (a morph for now)"),
        _ => None,
    };
    if let Some(h) = hint {
        ui.add(egui::TextEdit::multiline(&mut f.prompt).hint_text(h).desired_rows(2).desired_width(w));
    }
    match task {
        Task::Animate => {
            menu(ui, "Camera", "gen-motion", &mut f.motion, &MOTION_NAMES);
            FieldRow::new("Length", &mut f.seconds, 1.0..=15.0).default(5.0).unit("s").decimals(0).step(1.0).show(ui);
        }
        Task::Between => {
            FieldRow::new("Length", &mut f.seconds, 1.0..=10.0).default(2.0).unit("s").decimals(0).step(1.0).show(ui);
        }
        Task::Restyle | Task::TimeOfDay | Task::Season => {
            if task == Task::TimeOfDay {
                menu(ui, "Time", "gen-time", &mut f.time, &TIME_NAMES);
            }
            if task == Task::Season {
                menu(ui, "Season", "gen-season", &mut f.season, &SEASON_NAMES);
            }
            f.strength = f.strength.clamp(0.1, 1.0);
            percent_row(ui, "Amount", &mut f.strength, 0.1..=1.0, 1.0);
        }
        Task::Relight => {
            menu(ui, "From", "gen-dir", &mut f.direction, &DIRECTION_NAMES);
            menu(ui, "Colour", "gen-colour", &mut f.colour, &COLOUR_NAMES);
            percent_row(ui, "Amount", &mut f.strength, 0.1..=2.0, 1.0);
        }
        Task::Outpaint => {
            menu(ui, "Sides", "gen-side", &mut f.side, &SIDES);
            percent_row(ui, "Grow by", &mut f.grow, 0.1..=2.0, 0.5);
        }
        Task::Inpaint => {
            let labels = ["Left", "Top", "Width", "Height"];
            for (i, l) in labels.iter().enumerate() {
                percent_row(ui, l, &mut f.region[i], 0.0..=1.0, [0.4, 0.4, 0.2, 0.2][i]);
            }
        }
        Task::Upscale => {
            FieldRow::new("Scale", &mut f.scale, 1.5..=4.0).default(2.0).unit("×").decimals(1).show(ui);
        }
        Task::Enhance => {
            kit::labeled_row(ui, "Old film", |ui| kit::toggle(ui, &mut f.old_film, "Restore old film", None));
            kit::labeled_row(ui, "Faces", |ui| kit::toggle(ui, &mut f.faces, "Restore faces", None));
            f.scale = f.scale.clamp(1.0, 4.0);
            FieldRow::new("Upscale", &mut f.scale, 1.0..=4.0).default(1.0).unit("×").decimals(1).show(ui);
            let names: Vec<&str> = FPS.iter().map(|x| x.0).collect();
            menu(ui, "Frame rate", "gen-fps", &mut f.fps, &names);
        }
    }
    let pnames: Vec<&str> = PROVIDERS.iter().map(|p| p.0).collect();
    menu(ui, "Engine", "gen-provider", &mut f.provider, &pnames);
    if !task.makes_video() && !matches!(task, Task::Upscale | Task::Enhance) && PROVIDERS[f.provider.min(2)].1 != Some(g::Provider::Local) {
        FieldRow::new("Tries", &mut f.takes, 1.0..=4.0).default(1.0).decimals(0).step(1.0).show(ui);
    }
    let can = g::providers(task, false);
    let picked = PROVIDERS[f.provider.min(2)].1;
    let missing = match picked {
        Some(g::Provider::Codex) if !here.codex => Some("Needs the Codex app on this Mac"),
        Some(p) if !can.contains(&p) => Some("That engine can't do this. Best available picks one that can"),
        _ => None,
    };
    if let Some(m) = missing {
        ui.label(egui::RichText::new(m).font(theme::caption()).color(t.text_2));
    }
    // A clip a task made: again, or a variation with a new seed.
    let recipe = app.editor.selection.first().and_then(|id| app.doc.project.clips.iter().find(|c| c.id == *id)).and_then(|c| c.media).and_then(|m| app.doc.project.media(m)).and_then(|mi| mi.path.clone()).and_then(|p| g::recipe(&p)).filter(|r| r["task"].as_str().is_some_and(|t| t != "make"));
    let mut again: Option<bool> = None;
    if let Some(r) = &recipe {
        let what = task_name(r["task"].as_str().unwrap_or(""));
        let said = match r["prompt"].as_str().map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => crate::i18n::tf(format!("Made with {what}: “{p}”")),
            None => crate::i18n::tf(format!("Made with {what}")),
        };
        ui.label(egui::RichText::new(said).font(theme::caption()).color(t.text_2));
        ui.horizontal_wrapped(|ui| {
            if Button::secondary("Make again").show(ui).clicked() {
                again = Some(false);
            }
            if Button::secondary("Variation").show(ui).clicked() {
                again = Some(true);
            }
        });
    }
    let busy = run().lock().ok().and_then(|g| g.as_ref().and_then(|r| r.lock().ok().map(|r| r.status.clone())));
    if let Some(status) = busy {
        ui.label(egui::RichText::new(format!("{status}…")).font(theme::body()).color(t.text_2));
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    } else {
        if Button::primary("Run").icon(Icon::Sparkle).show(ui).clicked() {
            match args_of(app, &f).and_then(|a| start(app, a)) {
                Ok(()) => feedback::done(&ctx, crate::i18n::tf(format!("{} started", TASKS[f.task.min(TASKS.len() - 1)].1))),
                Err(e) => feedback::failed(&ctx, &app.doc, &e),
            };
        }
    }
    if let (Some(vary), Some(r)) = (again, recipe) {
        let mut a = crate::agent::generative::recipe_args(&r);
        if vary {
            let s = a["seed"].as_u64().unwrap_or(0);
            a["seed"] = json!((s.wrapping_add(7919) ^ (ctx.input(|i| i.time) * 1000.0) as u64) % 4_294_967_291);
        }
        if let Some(o) = a.as_object_mut() {
            o.remove("clip");
        }
        a["at"] = json!(app.editor.playhead as f64 / 1e6);
        match start(app, a) {
            Ok(()) => feedback::done(&ctx, if vary { "Making a variation" } else { "Making it again" }),
            Err(e) => feedback::failed(&ctx, &app.doc, &e),
        };
    }
    ctx.data_mut(|d| d.insert_temp(form_id(), f));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Change side draws for every task, makes arguments for a picture
    /// clip, and a local extension lands back in the clip.
    #[test]
    fn the_change_card_builds_tasks_and_lands_them() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let dir = std::env::temp_dir().join(format!("studio-gen-change-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("backdrop.png");
        g::save_png(&studio_ai::Frame::filled(64, 36, [100, 150, 200, 255]), &pic).unwrap();
        let mut s = crate::agent::Session::new(std::mem::replace(&mut app.doc, crate::agent::Session::default().doc));
        s.run("import", &json!({"paths": [pic.display().to_string()], "to_timeline": true})).unwrap();
        app.doc = s.doc;
        // The imported picture (the fresh editor may hold sample clips too).
        let id = app.doc.project.clips.iter().find(|c| c.media.and_then(|m| app.doc.project.media(m)).is_some_and(|mi| mi.path.as_deref() == Some(pic.as_path()))).unwrap().id;
        app.editor.selection = vec![id];
        ctx.data_mut(|d| d.insert_temp(mode_id(), 1usize));
        for k in 0..TASKS.len() {
            ctx.data_mut(|d| d.insert_temp(form_id(), Form { task: k, ..Default::default() }));
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 900.0))), time: Some(k as f64), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| super::super::card(&mut app, ui));
            out.textures_delta.clear();
            let f: Form = ctx.data(|d| d.get_temp(form_id())).unwrap();
            if TASKS[k].0 != Task::Between {
                let a = args_of(&app, &f).unwrap();
                assert_eq!(a["task"], TASKS[k].0.name());
            }
        }
        let f = Form { task: 6, provider: 2, side: 1, grow: 0.5, ..Default::default() };
        let a = args_of(&app, &f).unwrap();
        start(&mut app, a).unwrap();
        let t0 = std::time::Instant::now();
        while run().lock().unwrap().is_some() && t0.elapsed().as_secs() < 60 {
            poll(&mut app, &ctx);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let c = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap();
        assert!(c.meta.contains_key("generated"), "{:?}", c.meta);
        // Every task at the Inspector's width: labelled rows (no bare
        // tracks), menu words for people, notes without trailing periods.
        for k in 0..TASKS.len() {
            ctx.data_mut(|d| d.insert_temp(form_id(), Form { task: k, ..Default::default() }));
            let drawn = crate::editor::physics::cloud_ui_check::draw(&ctx, 3, |ui| super::super::card(&mut app, ui));
            drawn.assert_tidy(&format!("generate-change-{k}"), &["Made with"]);
            assert!(drawn.shows("Engine") && drawn.shows("Run"), "{:?}", drawn.texts);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_menus_and_notes_read_plainly() {
        for list in [&DIRECTION_NAMES[..], &COLOUR_NAMES, &TIME_NAMES, &SEASON_NAMES, &MOTION_NAMES, &SIDES] {
            for n in list {
                assert_eq!(crate::editor::physics::cloud_ui_check::problem(n), None, "{n}");
            }
        }
        assert_eq!(DIRECTIONS.len(), DIRECTION_NAMES.len());
        assert_eq!(COLOURS.len(), COLOUR_NAMES.len());
        assert_eq!(TIMES.len(), TIME_NAMES.len());
        assert_eq!(SEASONS.len(), SEASON_NAMES.len());
        assert_eq!(MOTIONS.len(), MOTION_NAMES.len());
        for t in TASKS {
            assert_eq!(crate::editor::physics::cloud_ui_check::problem(t.2), None, "{}", t.2);
        }
        assert_eq!(task_name("time_of_day"), "Time of day");
        assert_eq!(selected_line(Task::Between, TASKS[1].2, 1), TASKS[1].2);
        assert_eq!(selected_line(Task::Relight, TASKS[3].2, 2), "2 clips selected");
    }
}
