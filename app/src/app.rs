// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Top-level application: screens, overlays, theme, background jobs,
//! global shortcuts and the screenshot hook used for visual checks.

use crate::cli::{Cli, Screen};
use crate::doc::Doc;
use crate::kit;
use crate::prefs::{Prefs, ThemeChoice};
use crate::thumbs::Thumbs;
use crate::{editor, gallery, icons, screens, theme, view};
use eframe::egui::{self, ColorImage, ThemePreference};
use std::sync::{Arc, Mutex};

/// Dialog-like screens shown over the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// The media library, project safety and rights.
    Library,
    Export,
    Settings,
    Production,
    /// Searchable help (F1).
    Help,
}

/// Progress of real background work behind a job.
pub trait JobTask: Send + Sync + std::fmt::Debug {
    /// 0..1.
    fn fraction(&self) -> f32;
    fn finished(&self) -> bool;
    fn cancel(&self);
    fn cancelled(&self) -> bool;
    fn take_error(&self) -> Option<String>;
}

/// A background task shown in the jobs popover (spec §7).
#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub detail: String,
    pub progress: f32,
    /// Seconds a full run takes (simulated until real work is wired up).
    pub length: f32,
    pub done: bool,
    pub cancelled: bool,
    /// Real work behind the job (exports); simulated jobs have none.
    pub task: Option<Arc<dyn JobTask>>,
    pub started: f64,
    pub error: Option<String>,
    /// Where the result was written (exports), for "Show in Finder".
    pub path: Option<std::path::PathBuf>,
}

#[derive(Default)]
pub struct Jobs {
    pub list: Vec<Job>,
    next: u64,
}

impl Jobs {
    pub fn start(&mut self, name: impl Into<String>, detail: impl Into<String>, length: f32) -> u64 {
        self.next += 1;
        self.list.push(Job { id: self.next, name: name.into(), detail: detail.into(), progress: 0.0, length, done: false, cancelled: false, task: None, started: -1.0, error: None, path: None });
        self.next
    }
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    pub fn start_task(&mut self, name: impl Into<String>, detail: impl Into<String>, task: Arc<dyn JobTask>, path: Option<std::path::PathBuf>) -> u64 {
        let id = self.start(name, detail, 1.0);
        if let Some(j) = self.list.last_mut() {
            j.task = Some(task);
            j.path = path;
        }
        id
    }
    pub fn running(&self) -> usize {
        self.list.iter().filter(|j| !j.done && !j.cancelled).count()
    }
    /// Advance simulated jobs; returns jobs that finished this frame.
    pub fn tick(&mut self, dt: f32, now: f64) -> Vec<Job> {
        let mut finished = Vec::new();
        for j in &mut self.list {
            if j.started < 0.0 {
                j.started = now;
            }
            if let (Some(task), false, false) = (&j.task, j.done, j.cancelled) {
                j.progress = task.fraction();
                // ETA from the pace so far.
                let elapsed = (now - j.started) as f32;
                j.length = if j.progress > 0.01 { elapsed / j.progress } else { j.length.max(elapsed + 1.0) };
                if task.finished() {
                    match task.take_error() {
                        Some(e) => {
                            j.cancelled = true;
                            j.error = Some(e);
                        }
                        None if task.cancelled() => j.cancelled = true,
                        None => {
                            j.progress = 1.0;
                            j.done = true;
                        }
                    }
                    finished.push(j.clone());
                }
                continue;
            }
            if !j.done && !j.cancelled {
                j.progress = (j.progress + dt / j.length.max(0.1)).min(1.0);
                if j.progress >= 1.0 {
                    j.done = true;
                    finished.push(j.clone());
                }
            }
        }
        finished
    }
    pub fn get(&self, id: u64) -> Option<&Job> {
        self.list.iter().find(|j| j.id == id)
    }
    pub fn cancel(&mut self, id: u64) {
        if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
            match &j.task {
                // The export thread notices and cleans up.
                Some(t) => t.cancel(),
                None => j.cancelled = true,
            }
        }
    }
    pub fn eta(j: &Job) -> String {
        if j.task.is_some() && j.progress < 0.01 {
            return "starting".into();
        }
        let s = ((1.0 - j.progress) * j.length).ceil() as u32;
        if s >= 60 { format!("{} min {} s left", s / 60, s % 60) } else { format!("{s} s left") }
    }
}

pub struct StudioApp {
    /// `BENCH=...`: the benchmark harness driving this run.
    pub bench: Option<crate::bench::Bench>,
    pub prefs: Prefs,
    pub cli: Cli,
    pub screen: Screen,
    pub overlay: Overlay,
    pub doc: Doc,
    pub thumbs: Thumbs,
    pub jobs: Jobs,
    pub editor: editor::EditorState,
    pub start: screens::start::StartState,
    pub export: screens::export::ExportState,
    pub settings: screens::settings::SettingsState,
    pub gallery: gallery::GalleryState,
    /// Media ids being imported, with the time the import started.
    pub imports: Vec<(u32, f64)>,
    pub autosave: studio_model::file::autosave::Autosave,
    /// Viewer frames from `studio-render`, created on first use.
    #[cfg(feature = "render")]
    pub render: Option<editor::render_view::RenderView>,
    pub autosave_rev: u64,
    /// An autosave writing in the background: the history revision it
    /// saved, and how it went when it is done.
    pub autosaving: Option<(u64, std::sync::mpsc::Receiver<crate::files::Autosaved>)>,
    /// The project folder the Start screen last heard of.
    pub remembered: Option<std::path::PathBuf>,
    /// The toast offering "Show in Finder" for a finished export.
    pub reveal: Option<(u64, std::path::PathBuf)>,
    /// The AI editor, made when the AI panel first opens.
    pub agent: Option<editor::agent_panel::AgentUi>,
    frame: u64,
    shot: Arc<Mutex<Option<Arc<ColorImage>>>>,
    shot_requested: bool,
    applied_scale: f32,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cli: Cli) -> Self {
        let ctx = &cc.egui_ctx;
        if cli.shot.is_some() {
            // A screenshot shows popped-out panels inside the window (a
            // window of their own would not be in the picture).
            ctx.set_embed_viewports(true);
        }
        theme::install_fonts(ctx);
        #[cfg(feature = "render")]
        if cli.shot.is_none() {
            // The queue is drawn in the background for as long as the window is open.
            crate::deliver_run::start_background_runner();
        }
        theme::apply_style(ctx);
        egui_extras_free_image_loaders(ctx);
        let mut prefs = if cli.fresh { Prefs::default() } else { Prefs::load(cc.storage) };
        if let Some(dark) = cli.theme {
            prefs.theme = if dark { ThemeChoice::Dark } else { ThemeChoice::Light };
        }
        // Headless tests are not a first run: no welcome tour over the editor
        // taking its keys and clicks (the tour's own test turns it on).
        #[cfg(test)]
        {
            prefs.tour_done = true;
        }
        let project = cli.project.as_deref().map(view::by_name).unwrap_or_else(view::demo);
        let mut editor = editor::EditorState::new(&prefs);
        if project.clips.is_empty() {
            editor.playhead = 0;
        }
        let screen = cli.screen.unwrap_or(Screen::Start);
        let (screen, overlay) = match screen {
            Screen::Export => (Screen::Editor, Overlay::Export),
            Screen::Settings => (Screen::Start, Overlay::Settings),
            s => (s, Overlay::None),
        };
        let mut app = Self {
            bench: crate::bench::Bench::from_env(),
            thumbs: Thumbs::new(ctx),
            doc: Doc::from_view(&project),
            jobs: Jobs::default(),
            editor,
            start: Default::default(),
            export: Default::default(),
            settings: Default::default(),
            gallery: Default::default(),
            imports: Vec::new(),
            autosave: studio_model::file::autosave::Autosave::new(),
            #[cfg(feature = "render")]
            render: None,
            autosave_rev: 0,
            autosaving: None,
            remembered: None,
            reveal: None,
            agent: None,
            prefs,
            screen,
            overlay,
            frame: 0,
            shot: Default::default(),
            shot_requested: false,
            applied_scale: 0.0,
            cli,
        };
        // `--media=a.mp4,b.mov`: import files and lay them on the spine.
        if let Some(list) = app.cli.flags.iter().find_map(|f| f.strip_prefix("media=")).map(str::to_owned) {
            let paths: Vec<std::path::PathBuf> = list.split(',').map(std::path::PathBuf::from).collect();
            let ids = app.import_paths(ctx, &paths);
            let seq = app.doc.seq();
            let spine = app.doc.model.active().spine_id();
            for id in ids {
                let Some(mi) = app.doc.project.media(id).cloned() else { continue };
                let clip_id = app.doc.alloc_id();
                let dur = mi.dur.min(20 * view::SEC);
                let clip = studio_model::Clip::media(studio_model::ClipId(clip_id), spine, 0, studio_model::MediaId(id as u64), 0, dur);
                let mut clip = clip;
                clip.name = mi.name.rsplit_once('.').map_or(mi.name.clone(), |(a, _)| a.to_owned());
                let lane = (mi.kind == view::MediaKind::Audio).then(|| app.doc.model.active().lanes.iter().find(|l| l.kind == studio_model::LaneKind::Audio).map(|l| l.id)).flatten();
                if let Some(l) = lane {
                    clip.lane = l;
                    if let studio_model::ClipSource::Media(r) = &mut clip.source {
                        r.video = false;
                    }
                }
                app.doc.apply("Add clip", studio_model::Edit::Append { seq, lane, clip: Box::new(clip) });
            }
        }
        if let Some(i) = app.cli.select {
            // Index into the clips ordered by id, so it is stable per sample.
            let mut ids: Vec<u32> = app.doc.project.clips.iter().map(|c| c.id).collect();
            ids.sort();
            if let Some(id) = ids.get(i) {
                app.editor.selection = vec![*id];
            }
        }
        app.apply_prefs(ctx);
        if let Some(dir) = app.cli.open.take() {
            app.open_folder(ctx, Some(dir));
        }
        app.recover_after_crash(ctx);
        icons::preload(ctx);
        crate::content::preload(ctx);
        if app.cli.flag("settings-shortcuts") {
            app.settings.page = 3;
        }
        // Visual checks of the splitter limits.
        if app.cli.flag("layout-min") {
            app.editor.set_tree(crate::dock::Tree::legacy((true, 0.0), (true, 0.0), (true, Some(0.0))));
        }
        if app.cli.flag("layout-max") {
            app.editor.set_tree(crate::dock::Tree::legacy((true, 10_000.0), (true, 10_000.0), (true, Some(10_000.0))));
        }
        if app.cli.flag("collapsed") {
            app.editor.dock.close(crate::dock::Panel::Library);
            app.editor.dock.close(crate::dock::Panel::Inspector);
        }
        // Visual checks of docking: `--dock=scopes:viewer:right` (any number,
        // comma-separated; `window` as the target is the window's edge).
        if let Some(spec) = app.cli.flags.iter().find_map(|f| f.strip_prefix("dock=")).map(str::to_owned) {
            for one in spec.split(',') {
                let parts: Vec<&str> = one.split(':').collect();
                if let [p, target, zone] = parts[..]
                    && let (Some(p), Some(zone)) = (crate::dock::Panel::parse(p), crate::dock::Zone::parse(zone))
                {
                    let target = if target.eq_ignore_ascii_case("window") { None } else { crate::dock::Panel::parse(target) };
                    let lay = app.editor.last_layout();
                    app.editor.dock.dock(p, target, zone, &lay);
                }
            }
        }
        // `--popout=ai,scopes`: those panels in windows of their own.
        if let Some(spec) = app.cli.flags.iter().find_map(|f| f.strip_prefix("popout=")).map(str::to_owned) {
            for p in spec.split(',').filter_map(crate::dock::Panel::parse) {
                let lay = app.editor.last_layout();
                if !app.editor.dock.contains(p) {
                    app.editor.dock.open(p, &lay);
                }
                app.editor.dock.pop_out(p, &lay);
            }
        }
        if let Some(r) = app.cli.flags.iter().find_map(|f| f.strip_prefix("rail=")) {
            if let Some(item) = editor::RailItem::ALL.iter().find(|i| i.name().eq_ignore_ascii_case(r)) {
                app.editor.rail = *item;
            }
        }
        if app.cli.flag("list") {
            app.prefs.library_list = true;
        }
        if app.cli.flag("settings-about") {
            app.settings.page = 5;
        }
        app
    }

    pub fn apply_prefs(&mut self, ctx: &egui::Context) {
        self.apply_access(ctx);
        ctx.set_theme(match self.prefs.theme {
            ThemeChoice::Dark => ThemePreference::Dark,
            ThemeChoice::Light => ThemePreference::Light,
            ThemeChoice::System => ThemePreference::System,
        });
        kit::set_reduce_motion(ctx, self.prefs.reduce_motion);
        let scale = self.prefs.ui_scale.clamp(0.8, 2.0);
        if (scale - self.applied_scale).abs() > 0.001 {
            ctx.set_zoom_factor(scale);
            self.applied_scale = scale;
        }
    }

    pub fn open_project(&mut self, project: view::Project) {
        self.doc = Doc::from_view(&project);
        self.editor = editor::EditorState::new(&self.prefs);
        self.editor.playhead = 0;
        self.screen = Screen::Editor;
    }

    /// Open a model project in the editor.
    pub fn open_model(&mut self, project: studio_model::Project) {
        self.doc = Doc::new(project);
        self.editor = editor::EditorState::new(&self.prefs);
        self.editor.playhead = 0;
        self.screen = Screen::Editor;
    }

    fn global_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, Key::F1))) {
            if self.overlay == Overlay::Help {
                self.overlay = Overlay::None;
            } else {
                let topic = crate::a11y::focused_region(ctx).map(crate::help::topic_for_region);
                crate::help::open(self, ctx, topic);
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(cmd_shift, Key::G))) {
            self.screen = if self.screen == Screen::Gallery { Screen::Start } else { Screen::Gallery };
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(cmd_shift, Key::P))) {
            self.screen = if self.screen == Screen::Paint { Screen::Editor } else { Screen::Paint };
        }
        let dialogs = self.cli.shot.is_none();
        if dialogs && ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O))) {
            self.open_folder(ctx, None);
        }
        if dialogs && self.screen == Screen::Editor {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(cmd_shift, Key::S))) {
                self.save(ctx, true);
            }
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S))) {
                self.save(ctx, false);
            }
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::I))) {
                self.import_media_files(ctx);
            }
        }
        if crate::keymap::hit(ctx, &self.prefs, "settings") {
            self.overlay = if self.overlay == Overlay::Settings { Overlay::None } else { Overlay::Settings };
        }
    }

    fn screenshot_hook(&mut self, ctx: &egui::Context) {
        if self.frame <= 3 {
            // Some window managers ignore the initial size; insist.
            if let Some((w, h)) = self.cli.size {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            }
        }
        let Some(path) = self.cli.shot.clone() else { return };
        // Wait for background thumbnails so the picture is complete.
        let settled = self.frame >= self.cli.frames as u64 && self.thumbs.pending() == 0;
        if settled && !self.shot_requested {
            let slot = self.shot.clone();
            ctx.request_screenshot(move |img| *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(img));
            self.shot_requested = true;
        }
        if let Some(img) = self.shot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if let Err(e) = save_png(&path, &img) {
                eprintln!("screenshot failed: {e}");
            } else {
                eprintln!("saved {path} ({}x{})", img.size[0], img.size[1]);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

/// eframe has no image loaders installed by default; we only draw our own
/// textures, so nothing is needed here. Kept as a hook for when real media
/// thumbnails arrive.
fn egui_extras_free_image_loaders(_ctx: &egui::Context) {}

fn save_png(path: &str, img: &ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    w.write_image_data(&bytes)?;
    Ok(())
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let started = std::time::Instant::now();
        self.frame += 1;
        self.thumbs.pump(&ctx);
        self.apply_prefs(&ctx);
        self.autosave_tick();
        self.keep_session();
        self.library_tick();
        self.recents_tick();
        if self.frame == 1 && self.cli.flags.iter().any(|f| f == "link-dialog") {
            editor::agent_panel::open_link_dialog(self, &ctx);
        }
        if self.frame == 1 && self.cli.flags.iter().any(|f| f == "ai-editor") {
            // Visual checks: open on the AI editor.
            self.prefs.ai_cloud_ok |= self.cli.flags.iter().any(|f| f == "ai-on");
            editor::agent_panel::open(self);
            for (flag, writing) in [("ai-marks-demo", false), ("ai-note-demo", true)] {
                if self.cli.flags.iter().any(|f| f == flag) {
                    let ph = self.editor.playhead;
                    self.agent.get_or_insert_with(|| editor::agent_panel::AgentUi::new(&ctx)).demo_marks(ph, writing);
                }
            }
            for (flag, ask) in [("ai-ask-demo", true), ("ai-chat-demo", false)] {
                if self.cli.flags.iter().any(|f| f == flag) {
                    self.agent.get_or_insert_with(|| editor::agent_panel::AgentUi::new(&ctx)).demo_ask(ask);
                }
            }
        }
        editor::agent_panel::AgentUi::tick(self, &ctx);
        self.global_shortcuts(&ctx);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let now = ctx.input(|i| i.time);
        for job in self.jobs.tick(dt, now) {
            match (&job.error, job.cancelled) {
                (Some(e), _) => kit::push(&ctx, format!("{} failed: {e}", job.name), None, Some(icons::Icon::Warning)),
                (None, true) => kit::push(&ctx, format!("{} cancelled", job.name), None, Some(icons::Icon::XCircle)),
                (None, false) => {
                    let id = kit::push(&ctx, format!("{} finished", job.name), job.path.as_ref().map(|_| "Show in Finder"), Some(icons::Icon::CheckCircle));
                    if let Some(p) = &job.path {
                        self.reveal = Some((id, p.clone()));
                    }
                    if job.task.is_some() && self.cli.shot.is_none() {
                        crate::chime::play();
                    }
                    id
                }
            };
        }
        if self.jobs.running() > 0 {
            ctx.request_repaint();
        }

        let t = theme::tokens(&ctx);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0, t.ground);
        match self.screen {
            Screen::Start => screens::start::show(self, ui),
            Screen::Editor | Screen::Export | Screen::Settings => editor::show(self, ui),
            Screen::Gallery => gallery::show(self, ui),
            Screen::Paint => screens::paint::show(self, ui),
        }
        match self.overlay {
            Overlay::Export => screens::export::show(self, &ctx),
            Overlay::Settings => screens::settings::show(self, &ctx),
            Overlay::Library => screens::library_safety::show(self, &ctx),
            Overlay::Production => screens::production::show(self, &ctx),
            Overlay::Help => crate::help::show(self, &ctx),
            Overlay::None => {}
        }
        crate::release::notice(self, &ctx);
        if self.bench.is_some() {
            crate::bench::Bench::frame(self, &ctx, started.elapsed().as_secs_f64() * 1000.0);
        }
        if std::env::var_os("STUDIO_PERF").is_some() {
            perf::frame(started.elapsed().as_secs_f64() * 1000.0, self);
        }
        if std::env::var_os("STUDIO_DEBUG").is_some() {
            let ev: Vec<String> = ctx.input(|i| i.events.iter().filter(|e| !matches!(e, egui::Event::PointerMoved(_))).map(|e| format!("{e:?}")).collect());
            if !ev.is_empty() {
                eprintln!("frame {} overlay {:?} playing {} events {:?}", self.frame, self.overlay, self.editor.playing, ev);
            }
        }
        crate::onboarding::show_tour(self, &ctx);
        crate::onboarding::show_tip(self, &ctx);
        crate::a11y::live_region(ui);
        self.screenshot_hook(&ctx);
    }

    /// `--tabs N` presses Tab N times after the first frames, to check the
    /// keyboard focus order and ring in screenshots.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.cli.shot.is_some() {
            // Screenshot runs see only scripted input, never the person's
            // own typing or pointer.
            raw.events.clear();
        }
        let tabs = self.cli.flags.iter().find_map(|f| f.strip_prefix("tabs=")).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        if let Some((x, y)) = self.cli.flags.iter().find_map(|f| f.strip_prefix("hover=")).and_then(|v| v.split_once(',')).and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))) {
            if self.frame >= 5 {
                raw.events.push(egui::Event::PointerMoved(egui::pos2(x, y)));
            }
        }
        // `--script=path`: lines of `frame move|down|up|click x y` or
        // `frame key NAME [shift|cmd|alt]` replayed as input.
        if let Some(path) = self.cli.flags.iter().find_map(|f| f.strip_prefix("script=")) {
            let script = std::fs::read_to_string(path).unwrap_or_default();
            for line in script.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let Some(f) = parts.first().and_then(|f| f.parse::<u64>().ok()) else { continue };
                if f != self.frame {
                    continue;
                }
                let num = |i: usize| parts.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                let mods = |i: usize| {
                    let mut m = egui::Modifiers::NONE;
                    for p in parts.iter().skip(i) {
                        match *p {
                            "shift" => m.shift = true,
                            "alt" => m.alt = true,
                            "cmd" => {
                                m.command = true;
                                m.mac_cmd = true;
                            }
                            _ => {}
                        }
                    }
                    m
                };
                let pos = egui::pos2(num(2), num(3));
                if matches!(parts.get(1).copied(), Some("down" | "up" | "click" | "move")) {
                    raw.events.push(egui::Event::ModifiersChanged(mods(4)));
                }
                let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: mods(4) };
                match parts.get(1).copied() {
                    Some("move") => raw.events.push(egui::Event::PointerMoved(pos)),
                    Some("down") => {
                        raw.events.push(egui::Event::PointerMoved(pos));
                        raw.events.push(button(true));
                    }
                    Some("up") => {
                        raw.events.push(egui::Event::PointerMoved(pos));
                        raw.events.push(button(false));
                    }
                    Some("click") => {
                        raw.events.push(egui::Event::PointerMoved(pos));
                        raw.events.push(button(true));
                        raw.events.push(button(false));
                    }
                    Some("key") => {
                        if let Some(key) = parts.get(2).and_then(|k| egui::Key::from_name(k)) {
                            raw.events.push(egui::Event::ModifiersChanged(mods(3)));
                            for pressed in [true, false] {
                                raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: mods(3) });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if tabs > 0 && self.frame >= 10 && self.frame < 10 + tabs * 2 && self.frame % 2 == 0 {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.cli.shot.is_none() {
            self.prefs.save(storage);
        }
    }

    fn on_exit(&mut self) {
        // Quitting ends every program and job the AI started.
        crate::agent::procs::kill_all();
        crate::recover::clean_quit();
    }
}

/// `STUDIO_PERF=1`: once a second, print UI frame times (average and
/// worst), frames per second, the thumbnail queue and the viewer's render
/// time and dropped frames.
mod perf {
    use std::sync::Mutex;
    struct Window {
        start: Option<std::time::Instant>,
        n: u32,
        sum: f64,
        max: f64,
        gaps_max: f64,
        last: Option<std::time::Instant>,
    }
    static W: Mutex<Window> = Mutex::new(Window { start: None, n: 0, sum: 0.0, max: 0.0, gaps_max: 0.0, last: None });

    pub fn frame(ms: f64, app: &super::StudioApp) {
        let now = std::time::Instant::now();
        let mut w = W.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = w.last {
            w.gaps_max = w.gaps_max.max((now - l).as_secs_f64() * 1000.0);
        }
        w.last = Some(now);
        let start = *w.start.get_or_insert(now);
        w.n += 1;
        w.sum += ms;
        w.max = w.max.max(ms);
        if (now - start).as_secs_f64() >= 1.0 {
            #[cfg(feature = "render")]
            let (render_ms, dropped) = app.render.as_ref().map_or((0.0, 0), |r| (r.last_ms, r.dropping));
            #[cfg(not(feature = "render"))]
            let (render_ms, dropped) = (0.0f32, 0u64);
            eprintln!(
                "perf fps {:3} ui avg {:5.1} ms max {:5.1} ms gap max {:5.1} ms | thumbs pending {:4} | render {:5.1} ms dropped {}",
                w.n, w.sum / w.n as f64, w.max, w.gaps_max, app.thumbs.pending(), render_ms, dropped
            );
            *w = Window { start: Some(now), n: 0, sum: 0.0, max: 0.0, gaps_max: 0.0, last: Some(now) };
        }
    }
}

impl StudioApp {
    /// Language and accessibility settings: set when they change.
    fn apply_access(&mut self, ctx: &egui::Context) {
        let lang = studio_i18n::Lang::from_tag(&self.prefs.language).unwrap_or_else(studio_i18n::Lang::detect);
        if crate::i18n::lang() != lang {
            crate::i18n::set_lang(lang);
            ctx.request_repaint();
        }
        crate::a11y::set_enabled(ctx, self.prefs.announce);
        if crate::access::set(self.prefs.high_contrast, self.prefs.colour_vision) {
            theme::apply_style(ctx);
            ctx.request_repaint();
        }
    }
}
