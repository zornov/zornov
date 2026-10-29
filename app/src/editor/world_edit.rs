// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "World settings" section: what the AI's `build_world` and
//! `edit_world` put into a world, as controls. Sky and time of day, fog,
//! water and terrain; and every item of the lists (scatter, buildings,
//! roads, props, characters, lights, sculpt and paint strokes) with its
//! own settings, a remove button and an add button.
//!
//! The world is a spec (`studio_world::build::WorldSpec`, a `.sworld`
//! file). Edits are made on a copy; when the pointer is up the spec is
//! saved and the world rebuilt on a worker thread, and when it is ready the
//! layer is pointed at it (one undo step). The old world stays on show until
//! then, and edits that come faster than builds are folded into the latest.

use super::params::{json_value, set_json_choices};
use crate::agent::Session;
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, IconButton, Section};
use crate::theme;
use eframe::egui;
use serde_json::Value;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use studio_model as m;
use studio_world::build::WorldSpec;

/// The lists of a world, the field each is in, its name and a new item.
pub fn lists() -> Vec<(&'static str, &'static str, Value)> {
    use studio_world::build::*;
    fn j<T: serde::Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap_or_default()
    }
    vec![
        ("scatter", "Scatter", j(&ScatterJ::default())),
        ("buildings", "Buildings", j(&BuildingJ::default())),
        ("roads", "Roads", j(&RoadJ::default())),
        ("props", "Props", j(&PropJ::default())),
        ("characters", "Characters", j(&CharJ::default())),
        ("lights", "Lights", j(&LightJ::default())),
        ("sculpt", "Sculpt strokes", j(&SculptJ::default())),
        ("paint", "Paint strokes", j(&PaintJ::default())),
    ]
}

/// The single settings of a world (not lists), by field.
pub const SINGLES: &[(&str, &str)] = &[("sky", "Sky"), ("fog", "Fog"), ("water", "Water"), ("terrain", "Terrain"), ("look", "Look"), ("quality", "Quality"), ("spawn", "Start point")];

/// Choices of string settings, by their path in the spec.
pub fn choices(path: &str) -> Option<&'static [&'static str]> {
    let key = path.rsplit('/').next()?;
    let list = path.split('/').find(|p| matches!(*p, "scatter" | "buildings" | "props" | "lights" | "characters" | "paint" | "sculpt" | "water" | "terrain"))?;
    Some(match (list, key) {
        ("scatter", "kind") => &["pine", "tree", "palm", "bush", "rock", "boulder", "grass", "flower", "cactus", "dead_tree"],
        ("scatter", "on") | ("paint", "layer") => &["grass", "rock", "dirt", "snow", "sand"],
        ("buildings", "kind") => &["house", "apartment", "tower", "shop", "warehouse", "room", "hall"],
        ("buildings", "style") => &["classical", "art_deco", "brutalist", "modern_glass", "medieval", "oriental", "futuristic", "rustic"],
        ("props", "kind") => &["crate", "barrel", "table", "chair", "column", "stairs", "lamp", "fence", "car", "sofa", "bed", "shelf", "desk", "cactus"],
        ("lights", "kind") => &["point", "spot"],
        ("characters", "clip") => &["idle", "walk", "run", "wave", "talk", "dance"],
        ("sculpt", "brush") => &["raise", "lower", "smooth", "flatten"],
        ("water", "kind") => &["none", "sea", "lake", "river"],
        ("terrain", "style") => &["flat", "plains", "hills", "mountains", "dunes", "canyon", "island"],
        _ => return None,
    })
}

/// Edits in progress for one world file.
#[derive(Clone)]
struct Pending {
    file: String,
    spec: Value,
    dirty: bool,
}

/// An edited world spec, saved and waiting to be built.
#[derive(Clone)]
pub(crate) struct WorldJob {
    /// The `.sworld` file the spec is saved in (named by its content).
    pub path: String,
    pub spec: WorldSpec,
}

/// A world build that has finished.
struct Built {
    job: WorldJob,
    /// What is in the world, or why it could not be built.
    result: Result<String, String>,
}

type BuildFn = Arc<dyn Fn(&WorldJob) -> Result<String, String> + Send + Sync>;

/// Builds worlds one at a time off the UI thread. An edit that comes while
/// one is building waits in a single place, and a newer one takes it over:
/// the world that ends up shown is always the latest edit's.
#[derive(Clone)]
struct Builder(Arc<Mutex<Builds>>);

struct Builds {
    run: BuildFn,
    running: Option<Receiver<Built>>,
    queued: Option<WorldJob>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::with(Arc::new(Session::build_staged))
    }
}

impl Builder {
    fn with(run: BuildFn) -> Self {
        Self(Arc::new(Mutex::new(Builds { run, running: None, queued: None })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Builds> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Builds this world, now or when the one building is done.
    fn submit(&self, ctx: &egui::Context, job: WorldJob) {
        let mut b = self.lock();
        if b.running.is_some() {
            b.queued = Some(job);
        } else {
            b.start(ctx, job);
        }
    }

    /// The build that finished, if it is still wanted: with a newer edit
    /// waiting, that one is started instead and the finished one dropped.
    fn poll(&self, ctx: &egui::Context) -> Option<Built> {
        let mut b = self.lock();
        let done = match b.running.as_ref()?.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return None,
            // The worker died without an answer (a panic in the build).
            Err(TryRecvError::Disconnected) => {
                b.running = None;
                if let Some(next) = b.queued.take() {
                    b.start(ctx, next);
                }
                return None;
            }
        };
        b.running = None;
        match b.queued.take() {
            Some(next) => {
                b.start(ctx, next);
                None
            }
            None => Some(done),
        }
    }

    fn busy(&self) -> bool {
        self.lock().running.is_some()
    }
}

impl Builds {
    fn start(&mut self, ctx: &egui::Context, job: WorldJob) {
        let (tx, rx) = channel();
        let (run, ctx) = (self.run.clone(), ctx.clone());
        self.running = Some(rx);
        std::thread::spawn(move || {
            let result = run(&job);
            let _ = tx.send(Built { job, result });
            ctx.request_repaint();
        });
    }
}

fn item_title(list: &str, item: &Value, i: usize) -> String {
    let name = ["kind", "name", "brush", "layer"].iter().find_map(|k| item.get(k).and_then(Value::as_str)).unwrap_or("");
    let n = item.get("count").and_then(Value::as_u64).map(|c| format!(" x {c}")).unwrap_or_default();
    format!("{} {}{}", i + 1, if name.is_empty() { list } else { name }, n)
}

pub fn section(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let Some(file) = super::viewport::world::world_file_of(&mc) else { return };
    let ctx = ui.ctx().clone();
    let key = egui::Id::new(("world-edit", id));
    let mut pending: Option<Pending> = ctx.data(|d| d.get_temp(key)).filter(|p: &Pending| p.file == file);
    if pending.is_none() {
        let spec = super::viewport::world::read_spec(&file).and_then(|s| studio_world::build::materialize(&s)).and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()));
        match spec {
            Ok(spec) => pending = Some(Pending { file: file.clone(), spec, dirty: false }),
            Err(_) => return,
        }
    }
    let Some(mut p) = pending else { return };
    let builder: Builder = ctx.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new(("world-build", id)), Builder::default).clone());
    if let Some(done) = builder.poll(&ctx) {
        match done.result.and_then(|_| Session::apply_world(&mut app.doc, id, &done.job.path)) {
            // The clip shows the new world now: the edits go on from there.
            Ok(true) => p.file = done.job.path,
            Ok(false) => {}
            Err(e) => {
                kit::push(&ctx, e, None, Some(Icon::Warning));
            }
        }
    }
    let mut changed = false;
    Section::new("world-spec", "World settings").icon(Icon::Sun).start_closed().show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        set_json_choices(ui, Some(choices));
        ui.label(egui::RichText::new("Everything in this world. Changes rebuild it when you let go.").font(theme::caption()).color(tk.text_2));
        if builder.busy() {
            ui.label(egui::RichText::new("Rebuilding world...").font(theme::caption()).color(tk.text_2));
        }
        // Time of day is the layer's, so it can be animated.
        super::three_d::row(app, ui, &[id], "generator.time_of_day", "Time of day", 0.0..=24.0, 12.0, "h", 1, super::three_d::same, super::three_d::same);
        for (field, title) in SINGLES {
            let Some(v) = p.spec.get_mut(*field) else { continue };
            ui.label(egui::RichText::new(*title).font(theme::section_title()).color(tk.text));
            if v.is_null() {
                if Button::ghost("Set").compact().show(ui).clicked() {
                    *v = match *field {
                        "sky" => serde_json::to_value(studio_world::build::SkyJ::default()).unwrap_or_default(),
                        "water" => serde_json::to_value(studio_world::build::WaterJ { kind: "sea".into(), ..Default::default() }).unwrap_or_default(),
                        "terrain" => serde_json::to_value(studio_world::build::TerrainJ { style: "hills".into(), ..Default::default() }).unwrap_or_default(),
                        "look" => serde_json::to_value(studio_world::build::LookJ::default()).unwrap_or_default(),
                        "spawn" => serde_json::json!([0.0, 2.0, 0.0]),
                        _ => serde_json::json!(0.002),
                    };
                    changed = true;
                }
                continue;
            }
            changed |= json_value(ui, &format!("world/{field}"), v, 0);
        }
        for (field, title, fresh) in lists() {
            let items = p.spec.get_mut(field).and_then(Value::as_array_mut);
            let Some(items) = items else { continue };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{title} ({})", items.len())).font(theme::section_title()).color(tk.text));
                if IconButton::new(Icon::Plus, "Add one").compact().show(ui).clicked() {
                    items.push(fresh.clone());
                    changed = true;
                }
            });
            let mut remove = None;
            for (i, item) in items.iter_mut().enumerate() {
                let head = item_title(field, item, i);
                egui::CollapsingHeader::new(egui::RichText::new(head).font(theme::body()).color(tk.text_2)).id_salt(("world-item", id, field, i)).default_open(false).show(ui, |ui| {
                    changed |= json_value(ui, &format!("world/{field}/{i}"), item, 0);
                    if Button::ghost("Remove").icon(Icon::Trash).compact().show(ui).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                items.remove(i);
                changed = true;
            }
        }
        set_json_choices(ui, None);
    });
    if changed {
        p.dirty = true;
    }
    // Rebuild once the pointer is up.
    if p.dirty && !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
        match serde_json::from_value::<WorldSpec>(p.spec.clone()) {
            Ok(spec) => match Session::stage_world(&app.doc, id, &spec) {
                Ok(job) => builder.submit(&ctx, job),
                Err(e) => {
                    kit::push(&ctx, e, None, Some(Icon::Warning));
                }
            },
            Err(e) => {
                kit::push(&ctx, format!("World: {e}"), None, Some(Icon::Warning));
            }
        }
        p.dirty = false;
    }
    ctx.data_mut(|d| d.insert_temp(key, p));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every list a world spec has is editable here, and every single
    /// setting; a new field of the spec fails this until it has a place.
    #[test]
    fn every_world_setting_has_a_control() {
        let spec = serde_json::to_value(WorldSpec::default()).unwrap();
        let obj = spec.as_object().unwrap();
        let listed: Vec<&str> = lists().iter().map(|l| l.0).collect();
        let single: Vec<&str> = SINGLES.iter().map(|s| s.0).collect();
        // Fields that are the world's identity, not its content.
        let identity = ["name", "description", "preset", "seed", "size", "without"];
        for (k, v) in obj {
            if identity.contains(&k.as_str()) {
                continue;
            }
            assert!(listed.contains(&k.as_str()) || single.contains(&k.as_str()), "world setting \"{k}\" has no control in editor/world_edit.rs");
            if v.is_array() {
                assert!(listed.contains(&k.as_str()), "list \"{k}\" is not editable");
            }
        }
        for l in &listed {
            assert!(obj.get(*l).is_some_and(Value::is_array), "{l} is not a list of the spec");
        }
        // A new item of every list reads back into the spec.
        for (field, _, fresh) in lists() {
            let mut s = spec.clone();
            s[field] = serde_json::json!([fresh]);
            assert!(serde_json::from_value::<WorldSpec>(s).is_ok(), "a new {field} item is not valid");
        }
        assert!(super::super::inspector::all_tab_kinds().contains(&"world-spec"));
    }

    /// The choices offered are values the builder accepts.
    #[test]
    fn choices_are_known_to_the_builder() {
        assert!(choices("world/scatter/0/kind").unwrap().contains(&"pine"));
        assert!(choices("world/lights/0/kind").unwrap().contains(&"spot"));
        assert!(choices("world/sky/clouds").is_none());
    }

    fn job(n: usize) -> WorldJob {
        WorldJob { path: format!("world-{n}.sworld"), spec: WorldSpec::default() }
    }

    /// A builder whose builds wait for a go-ahead, and which notes what it built.
    fn gated() -> (Builder, std::sync::mpsc::Sender<()>, Arc<Mutex<Vec<String>>>) {
        let (go, gate) = channel::<()>();
        let (gate, built) = (Mutex::new(gate), Arc::new(Mutex::new(Vec::new())));
        let log = built.clone();
        let run: BuildFn = Arc::new(move |j: &WorldJob| {
            let _ = gate.lock().unwrap().recv();
            log.lock().unwrap().push(j.path.clone());
            Ok(j.path.clone())
        });
        (Builder::with(run), go, built)
    }

    fn finished(b: &Builder, ctx: &egui::Context) -> Built {
        for _ in 0..2000 {
            if let Some(done) = b.poll(ctx) {
                return done;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("no build finished");
    }

    /// Edits that come faster than builds end in the latest world, and the
    /// ones in between are never built or shown.
    #[test]
    fn a_rapid_series_of_edits_ends_in_the_latest_world() {
        let ctx = egui::Context::default();
        let (b, go, built) = gated();
        b.submit(&ctx, job(1));
        assert!(b.busy());
        for n in 2..=9 {
            b.submit(&ctx, job(n));
        }
        // Nothing is ready while the first is building.
        assert!(b.poll(&ctx).is_none());
        go.send(()).unwrap();
        go.send(()).unwrap();
        let done = finished(&b, &ctx);
        assert_eq!(done.job.path, "world-9.sworld");
        assert_eq!(done.result.as_deref(), Ok("world-9.sworld"));
        assert_eq!(*built.lock().unwrap(), ["world-1.sworld", "world-9.sworld"]);
        assert!(!b.busy());
        // Nothing is left to show or build.
        assert!(b.poll(&ctx).is_none());
    }

    /// A build that fails says why and does not stop the next one.
    #[test]
    fn a_failed_build_is_reported_and_the_next_one_runs() {
        let ctx = egui::Context::default();
        let run: BuildFn = Arc::new(|j: &WorldJob| if j.path.contains('1') { Err("no terrain".into()) } else { Ok(j.path.clone()) });
        let b = Builder::with(run);
        b.submit(&ctx, job(1));
        assert_eq!(finished(&b, &ctx).result, Err("no terrain".to_string()));
        b.submit(&ctx, job(2));
        assert_eq!(finished(&b, &ctx).result.as_deref(), Ok("world-2.sworld"));
    }

    /// A build that panics frees the builder, and the edit waiting runs.
    #[test]
    fn a_build_that_panics_does_not_wedge_the_builder() {
        let ctx = egui::Context::default();
        let run: BuildFn = Arc::new(|j: &WorldJob| if j.path.contains('1') { panic!("bad world") } else { Ok(j.path.clone()) });
        let b = Builder::with(run);
        b.submit(&ctx, job(1));
        b.submit(&ctx, job(2));
        assert_eq!(finished(&b, &ctx).job.path, "world-2.sworld");
    }
}
