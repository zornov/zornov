// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project folders, media import and interchange, through the model's
//! `file` and `interchange` modules and native file dialogs.

use crate::app::StudioApp;
use crate::bridge;
use crate::doc::Doc;
use crate::icons::Icon;
use crate::kit;
use crate::media_io;
use crate::theme;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use crate::view;
use studio_model as m;

/// An error in front of the person. One place to move onto `kit::push_error`
/// (ux-feedback's, not on main yet).
pub(crate) fn say_error(ctx: &egui::Context, text: impl Into<String>) -> u64 {
    kit::push_error(ctx, text)
}

/// A project on the Start screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    pub name: String,
    /// Seconds since the Unix epoch.
    pub edited: u64,
    pub length: m::Micros,
    pub width: u32,
    pub height: u32,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// "Today, 09:12", "Yesterday", or a date.
pub fn edited_label(secs: u64) -> String {
    let now = now_secs();
    let ago = now.saturating_sub(secs);
    // Local offset is unknown without a time zone database; days are
    // counted in whole 24-hour steps, which reads right for recent work.
    let day = 86_400;
    if ago < 60 {
        "Just now".into()
    } else if ago < 3600 {
        format!("{} min ago", ago / 60)
    } else if ago < day {
        format!("{} h ago", ago / 3600)
    } else if ago < 2 * day {
        "Yesterday".into()
    } else {
        format!("{} days ago", ago / day)
    }
}

/// Where new projects live until the person moves them (Save As).
pub fn projects_dir() -> PathBuf {
    studio_paths::studio().join("Projects")
}

/// A package for a new project called `name` that does not exist yet:
/// "Name.zornov", then "Name 2.zornov", …
pub fn free_project_dir(name: &str) -> PathBuf {
    let clean: String = name.chars().map(|c| if matches!(c, '/' | ':' | '\\') { '-' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.');
    let base = if clean.is_empty() { "Untitled" } else { clean };
    let dir = projects_dir();
    (1..)
        .map(|n| if n == 1 { dir.join(format!("{base}.{}", m::file::PACKAGE_EXT)) } else { dir.join(format!("{base} {n}.{}", m::file::PACKAGE_EXT)) })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// `path` with `.zornov` on the end unless it has it.
pub fn package_path(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(m::file::PACKAGE_EXT)) {
        return path;
    }
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}", m::file::PACKAGE_EXT));
    path.with_file_name(name)
}

/// Every project folder any Zornov opened or made (this window, `zornov`
/// in a terminal, agents), oldest first: the Start screen lists them all.
fn shared_list() -> PathBuf {
    studio_paths::studio().join("recent-projects.txt")
}

/// Notes a project folder in the shared list.
pub fn note_project(dir: &Path) {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    // Throwaway projects (tests, benchmarks, scratch folders) never reach
    // Home, where they were mistaken for real work and opened.
    if is_temporary(&dir) || std::env::var_os("STUDIO_NO_RECENTS").is_some() {
        return;
    }
    let path = shared_list();
    let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().map(str::to_owned).collect();
    let me = dir.display().to_string();
    lines.retain(|l| *l != me);
    lines.push(me);
    let keep = lines.len().saturating_sub(60);
    let text = lines[keep..].join("\n") + "\n";
    // The shared list only feeds the Start screen; a failure costs a tile,
    // not work, so it is logged rather than put in front of the person.
    if let Err(e) = std::fs::create_dir_all(studio_paths::studio()).and_then(|()| m::file::write_atomic(&path, text.as_bytes())) {
        eprintln!("zornov: could not update the list of projects: {e}");
    }
}

/// A folder under the system's temporary places.
pub fn is_temporary(dir: &Path) -> bool {
    // The temp dir does not move while Studio runs; resolve it once.
    static TMP: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let tmp = TMP.get_or_init(|| std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir()));
    dir.starts_with(tmp) || ["/tmp", "/private/tmp", "/private/var/folders", "/var/folders"].iter().any(|t| dir.starts_with(t))
}

/// Adds the projects in the shared list that `recents` does not have yet
/// (made in a terminal or by an agent), newest first.
pub fn merge_shared(recents: &mut Vec<Recent>) {
    let Ok(text) = std::fs::read_to_string(shared_list()) else { return };
    let mut added = false;
    for line in text.lines().rev() {
        let path = PathBuf::from(line);
        if line.is_empty() || is_temporary(&path) || recents.iter().any(|r| r.path == path || std::fs::canonicalize(&r.path).ok().as_ref() == Some(&path)) {
            continue;
        }
        let Ok(file) = m::file::ProjectFolder::new(&path).load() else { continue };
        let view = bridge::view_of(&file.project, 0);
        let edited = std::fs::metadata(&path).and_then(|md| md.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        recents.push(Recent { path, name: view.name.clone(), edited, length: view.duration(), width: view.width, height: view.height });
        added = true;
    }
    if added {
        recents.sort_by(|a, b| b.edited.cmp(&a.edited));
        recents.truncate(24);
    }
}

/// File ▸ Import media's choices, built from what reads each kind: the
/// MP4 / MOV demuxer and the WebM / MKV converter, the picture readers
/// (PNG, JPEG, and EXR, DPX and TIFF through `studio-imageio`), Photoshop
/// documents, the sound decoders, Lottie, and every model importer. The
/// first, "All media", holds them all.
pub(crate) fn import_filters() -> Vec<(&'static str, Vec<&'static str>)> {
    let join = |lists: &[&[&'static str]]| -> Vec<&'static str> {
        let mut v: Vec<&'static str> = Vec::new();
        for e in lists.iter().flat_map(|l| l.iter()) {
            if !v.contains(e) {
                v.push(e);
            }
        }
        v
    };
    let models = crate::mesh_store::model_extensions();
    let groups = vec![
        ("Video", join(&[media_io::VIDEO_EXT, crate::transcode::CONTAINERS])),
        ("Pictures", join(&[media_io::IMAGE_EXT, studio_imageio::EXTENSIONS, crate::psd_import::EXTENSIONS])),
        ("Sound", join(&[media_io::AUDIO_EXT, media_io::SOUND_DECODED])),
        ("3D models", models),
        ("Lottie animations", media_io::LOTTIE_EXT.to_vec()),
    ];
    let all = join(&groups.iter().map(|g| g.1.as_slice()).collect::<Vec<_>>());
    std::iter::once(("All media", all)).chain(groups).collect()
}

/// Whether Import media takes a file, by its extension (any case).
pub(crate) fn importable(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    import_filters()[0].1.contains(&ext.as_str())
}

impl StudioApp {
    fn remember(&mut self, path: &Path) {
        let p = &self.doc.project;
        let entry = Recent { path: path.to_path_buf(), name: p.name.clone(), edited: now_secs(), length: p.duration(), width: p.width, height: p.height };
        self.prefs.recents.retain(|r| r.path != entry.path);
        self.prefs.recents.insert(0, entry);
        self.prefs.recents.truncate(24);
        self.remembered = Some(path.to_path_buf());
        note_project(path);
    }

    /// A new project gets its own folder at once (in Studio's Projects
    /// folder) and is saved there, so it is on the Start screen straight
    /// away and autosave keeps it. A failure is left in `doc.error`; the
    /// window's own paths use [`StudioApp::give_folder_or_say`].
    pub fn give_folder(&mut self) {
        if let Err(e) = self.try_give_folder() {
            self.doc.error = Some(e);
        }
    }

    /// [`StudioApp::give_folder`] that tells the person when it fails.
    pub fn give_folder_or_say(&mut self, ctx: &egui::Context) {
        if let Err(e) = self.try_give_folder() {
            say_error(ctx, crate::i18n::tf(format!("{e}; the project is open but not saved yet")));
        }
    }

    fn try_give_folder(&mut self) -> Result<(), String> {
        if self.doc.folder.is_some() {
            return Ok(());
        }
        let dir = free_project_dir(&self.doc.project.name);
        self.give_folder_at(&dir)
    }

    /// Saves the open project as a new package at `dir`.
    pub(crate) fn give_folder_at(&mut self, dir: &Path) -> Result<(), String> {
        let root = crate::services::project::save(&mut self.doc, Some(dir))?;
        self.remember(&root);
        autosave_health().ok();
        Ok(())
    }

    /// The open project replaced by `doc`, in a fresh editor.
    fn adopt(&mut self, doc: Doc) {
        self.doc = doc;
        self.editor = crate::editor::EditorState::new(&self.prefs);
        self.editor.playhead = 0;
        self.screen = crate::cli::Screen::Editor;
    }

    /// Save to the project's folder, asking for one the first time.
    pub fn save(&mut self, ctx: &egui::Context, save_as: bool) {
        let folder = match (&self.doc.folder, save_as) {
            (Some(f), false) => f.clone(),
            _ => {
                let Some(dir) = rfd::FileDialog::new().set_title("Save project").set_file_name(format!("{}.{}", self.doc.project.name, m::file::PACKAGE_EXT)).add_filter("Zornov project", &[m::file::PACKAGE_EXT]).save_file() else { return };
                let dir = package_path(dir);
                // The dialog asked about "Name", not "Name.zornov": an existing
                // project of that name is replaced only if the person says so.
                if save_target(&dir, self.doc.folder.as_ref().map(|f| f.root())) == SaveTarget::Replaces && !self.cli.shot.is_some() {
                    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let yes = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Replace project?").set_description(format!("{name} already exists here. Saving over it replaces its edit.")).set_buttons(rfd::MessageButtons::YesNo).show();
                    if yes != rfd::MessageDialogResult::Yes {
                        return;
                    }
                }
                let made = if m::file::ProjectFolder::is_project(&dir) { Ok(m::file::ProjectFolder::new(&dir)) } else { crate::services::project::new_package(&dir) };
                match made {
                    Ok(f) => f,
                    Err(e) => {
                        say_error(ctx, crate::i18n::tf(format!("Could not create the folder: {e}")));
                        return;
                    }
                }
            }
        };
        // Through the project service: a save over work someone else saved
        // meanwhile takes that work in first (see `services::project::save`).
        let target = (self.doc.folder.as_ref().map(|f| f.root()) != Some(folder.root())).then(|| folder.root().to_path_buf());
        match crate::services::project::save(&mut self.doc, target.as_deref()) {
            Ok(root) => {
                self.remember(&root);
                autosave_health().ok();
                kit::push(ctx, crate::i18n::tf(format!("Saved {}", self.doc.project.name)), None, Some(Icon::CheckCircle));
            }
            Err(e) => {
                say_error(ctx, crate::i18n::tf(e));
            }
        }
    }

    /// Open a project folder (ask when `path` is None). Unsaved work in
    /// the open project is settled first (see [`StudioApp::replace`]).
    pub fn open_folder(&mut self, ctx: &egui::Context, path: Option<PathBuf>) {
        let Some(dir) = path.or_else(|| rfd::FileDialog::new().set_title("Open project").pick_folder()) else { return };
        self.replace(ctx, Replace::Open(dir));
    }

    /// Opens `dir` through the project service, so the undo history, the
    /// hold on the folder, the team's rules and the 3D links come with it.
    fn open_dir(&mut self, ctx: &egui::Context, dir: PathBuf) {
        match crate::services::project::open(&dir) {
            Ok(doc) => {
                self.adopt(doc);
                self.remember(&dir);
                // Work an autosave holds that the saved file does not (a
                // save failed, or Zornov stopped): offered, never silent.
                set_offer(ctx, crate::safety::newer_autosave(&dir).map(|s| Offer { path: s.path, at_ms: s.at_ms }));
            }
            Err(e) => {
                self.prefs.recents.retain(|r| r.path != dir);
                say_error(ctx, crate::i18n::tf(e));
            }
        }
    }

    /// What the top bar shows about saving: see [`SaveState`].
    pub fn save_state(&self) -> SaveState {
        if let Some(e) = autosave_failure() {
            return SaveState::AutosaveFailed(e);
        }
        if self.autosaving.is_some() {
            return SaveState::Saving;
        }
        match (self.doc.dirty(), self.doc.folder.is_some()) {
            (false, true) => SaveState::Saved,
            (false, false) => SaveState::Scratch,
            (true, _) => SaveState::Unsaved,
        }
    }

    /// Whether the open project has changes nothing has saved.
    pub fn has_unsaved(&self) -> bool {
        self.doc.dirty()
    }

    /// Gets the open project's changes safe before something replaces or
    /// closes it: a project with a folder is saved on the spot; one without
    /// (a sample, the demo) has to be asked about.
    fn settle(&mut self, ctx: &egui::Context) -> Settled {
        if !self.doc.dirty() {
            return Settled::Clean;
        }
        if self.doc.folder.is_none() {
            return Settled::Ask;
        }
        match crate::services::project::save(&mut self.doc, None) {
            Ok(root) => {
                self.remember(&root);
                autosave_health().ok();
                Settled::Clean
            }
            Err(e) => {
                say_error(ctx, crate::i18n::tf(format!("{e}; {} is still open", self.doc.project.name)));
                Settled::Failed
            }
        }
    }

    /// Does `what` once the open project's changes are safe: saved here,
    /// or after the person chose in the "Save changes?" dialog.
    pub fn replace(&mut self, ctx: &egui::Context, what: Replace) {
        match self.settle(ctx) {
            Settled::Clean => self.perform(ctx, what),
            Settled::Failed => {}
            Settled::Ask => set_pending(ctx, Some(what)),
        }
    }

    /// Back to the Start screen; the project is saved first.
    pub fn go_home(&mut self, ctx: &egui::Context) {
        self.replace(ctx, Replace::Home);
    }

    fn perform(&mut self, ctx: &egui::Context, what: Replace) {
        match what {
            Replace::Quit => {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("files-quit-ok"), true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Replace::Home => self.screen = crate::cli::Screen::Start,
            Replace::Open(dir) => self.open_dir(ctx, dir),
            Replace::ImportTimeline => self.import_timeline_now(ctx),
            Replace::New(p) => {
                self.open_project(p);
                self.give_folder_or_say(ctx);
            }
            Replace::Sample(p) => {
                self.open_project(p);
                kit::push(ctx, "Sample project: changes are not kept until you use Save As", None, Some(Icon::Info));
            }
        }
    }

    /// Every frame (one line in `app.rs`): the close button, Cmd+Q and the
    /// "Save changes?" dialog, and what the save machinery wants to say.
    pub fn files_frame(&mut self, ctx: &egui::Context) {
        // The Start screen calls this too; once per pass is enough.
        let pass = ctx.cumulative_pass_nr();
        let ran = egui::Id::new("files-frame-pass");
        if ctx.data(|d| d.get_temp::<u64>(ran)) == Some(pass) {
            return;
        }
        ctx.data_mut(|d| d.insert_temp(ran, pass));
        if let Some(note) = crate::prefs::take_notice() {
            say_error(ctx, crate::i18n::tf(note));
        }
        if let Some(note) = crate::recover::take_protection_notice() {
            say_error(ctx, crate::i18n::tf(note));
        }
        if let Some(e) = autosave_health().take_warning() {
            say_error(ctx, crate::i18n::tf(format!("Autosave failed: {e}; it retries, and Cmd+S saves now")));
        }
        let quitting = ctx.input(|i| i.viewport().close_requested()) && self.cli.shot.is_none() && !ctx.data(|d| d.get_temp::<bool>(egui::Id::new("files-quit-ok"))).unwrap_or(false);
        if quitting {
            match self.settle(ctx) {
                Settled::Clean => {}
                Settled::Failed => ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose),
                Settled::Ask => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    set_pending(ctx, Some(Replace::Quit));
                }
            }
        }
        self.unsaved_dialog(ctx);
        if pending(ctx).is_none() {
            self.restore_dialog(ctx);
        }
    }

    /// "Found unsaved changes from …": restore them as an edit, or keep
    /// the saved version.
    fn restore_dialog(&mut self, ctx: &egui::Context) {
        let Some(offer) = ctx.data(|d| d.get_temp::<Option<Offer>>(egui::Id::new(OFFER))).flatten() else { return };
        let when = edited_label(offer.at_ms / 1000).to_lowercase();
        let mut answer = None;
        let out = kit::modal(ctx, egui::Id::new("files-restore"), egui::vec2(460.0, 190.0), &crate::i18n::tf(format!("Found unsaved changes from {when}")), |ui| {
            ui.add_space(theme::space::MD);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(crate::i18n::shown("Zornov was closed before they were saved. Restoring adds them as one step you can undo."));
            });
            ui.add_space(theme::space::LG);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                if kit::Button::primary("Restore").show(ui).clicked() {
                    answer = Some(true);
                }
                if kit::Button::secondary("Keep saved version").show(ui).clicked() {
                    answer = Some(false);
                }
            });
        });
        if out.close {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                set_offer(ctx, None);
                self.restore_autosave(ctx, &offer.path);
            }
            Some(false) => set_offer(ctx, None),
            None => {}
        }
    }

    /// Takes the project an autosave holds in as one undoable edit, so the
    /// project reads unsaved. Says so when it cannot.
    pub(crate) fn restore_autosave(&mut self, ctx: &egui::Context, path: &Path) -> bool {
        let read = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| m::file::from_json(&t).map_err(|e| e.to_string()));
        match read.and_then(|file| self.doc.take_in("Recovered changes", &file.project)) {
            Ok(_) => true,
            Err(e) => {
                say_error(ctx, crate::i18n::tf(format!("Could not restore the unsaved changes: {e}")));
                false
            }
        }
    }

    /// Start screen: projects whose window ended without a clean quit and
    /// are still waiting, each to open (its unsaved work is offered then)
    /// or dismiss.
    pub fn recovered_panel(&mut self, ctx: &egui::Context) {
        self.recovered_panel_in(ctx, &studio_paths::studio());
    }

    pub(crate) fn recovered_panel_in(&mut self, ctx: &egui::Context, root: &Path) {
        let rows = recovered_rows(ctx, root);
        if rows.is_empty() {
            return;
        }
        let t = theme::tokens(ctx);
        let mut chosen = None;
        egui::Area::new(egui::Id::new("files-recovered")).anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(24.0, -24.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.line)).corner_radius(egui::CornerRadius::same(theme::radius::PANEL)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.label(egui::RichText::new(crate::i18n::shown("Closed unexpectedly")).font(theme::section_title()).color(t.text));
                for (name, project, pid) in &rows {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if kit::Button::secondary("Open").show(ui).clicked() {
                            chosen = Some((*pid, Some(project.clone())));
                        }
                        if kit::Button::ghost("Dismiss").show(ui).clicked() {
                            chosen = Some((*pid, None));
                        }
                    });
                }
            });
        });
        if let Some((pid, open)) = chosen {
            self.choose_recovered(ctx, root, pid, open);
        }
    }

    pub(crate) fn choose_recovered(&mut self, ctx: &egui::Context, root: &Path, pid: u32, open: Option<PathBuf>) {
        crate::recover::forget(ctx, root, pid);
        ctx.data_mut(|d| d.remove::<RecoveredCache>(egui::Id::new(CRASHED)));
        if let Some(project) = open {
            self.open_folder(ctx, Some(project));
        }
    }

    /// "Save changes to X?" for a project that has no folder yet.
    fn unsaved_dialog(&mut self, ctx: &egui::Context) {
        let Some(what) = pending(ctx) else { return };
        let id = egui::Id::new("files-unsaved");
        let name = self.doc.project.name.clone();
        let mut answer = None;
        let out = kit::modal(ctx, id, egui::vec2(440.0, 190.0), &crate::i18n::tf(format!("Save changes to {name}?")), |ui| {
            ui.add_space(theme::space::MD);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(crate::i18n::shown("It is not saved anywhere yet. Without saving, your changes are lost."));
            });
            ui.add_space(theme::space::LG);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                if kit::Button::primary("Save").show(ui).clicked() {
                    answer = Some(Unsaved::Save);
                }
                if kit::Button::secondary("Don't save").show(ui).clicked() {
                    answer = Some(Unsaved::Discard);
                }
                if kit::Button::ghost("Cancel").show(ui).clicked() {
                    answer = Some(Unsaved::Cancel);
                }
            });
        });
        if out.close {
            answer = Some(Unsaved::Cancel);
        }
        if let Some(a) = answer {
            self.answer_unsaved(ctx, what, a);
        }
    }

    /// The person's answer to the "Save changes?" dialog.
    pub(crate) fn answer_unsaved(&mut self, ctx: &egui::Context, what: Replace, answer: Unsaved) {
        match answer {
            Unsaved::Cancel => set_pending(ctx, None),
            Unsaved::Discard => {
                set_pending(ctx, None);
                self.perform(ctx, what);
            }
            Unsaved::Save => match self.try_give_folder() {
                Ok(()) => {
                    set_pending(ctx, None);
                    let name = self.doc.project.name.clone();
                    kit::push(ctx, crate::i18n::tf(format!("Saved {name} to Projects")), None, Some(Icon::CheckCircle));
                    self.perform(ctx, what);
                }
                // Stay in the dialog: nothing was lost, and Cancel is there.
                Err(e) => {
                    say_error(ctx, crate::i18n::tf(e));
                }
            },
        }
    }

    /// Import OpenTimelineIO or FCPXML as a new project.
    pub fn import_timeline(&mut self, ctx: &egui::Context) {
        self.replace(ctx, Replace::ImportTimeline);
    }

    fn import_timeline_now(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().set_title("Import timeline").add_filter("Timeline", &["otio", "fcpxml", "xml"]).pick_file() else { return };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                say_error(ctx, crate::i18n::tf(format!("Could not read the file: {e}")));
                return;
            }
        };
        let mut project = m::Project::new(m::ProjectSettings::default());
        let is_otio = path.extension().is_some_and(|e| e == "otio");
        let imported = if is_otio { m::interchange::otio::import(&mut project, &text) } else { m::interchange::fcpxml::import(&mut project, &text) };
        match imported {
            Ok(imp) => {
                if let Err(e) = project.apply(imp.edit) {
                    say_error(ctx, crate::i18n::tf(format!("Import failed: {e}")));
                    return;
                }
                if let Some(seq) = project.sequences.iter().position(|s| s.id == imp.sequence) {
                    project.active_sequence = project.sequences[seq].id;
                }
                self.open_model(project);
                self.give_folder_or_say(ctx);
                let n = imp.report.issues.len();
                if n > 0 {
                    let first = &imp.report.issues[0];
                    kit::push(ctx, crate::i18n::tf(format!("Imported with {n} note{}: {} — {}", if n == 1 { "" } else { "s" }, first.item, first.message)), None, Some(Icon::Info));
                } else {
                    kit::push(ctx, "Timeline imported", None, Some(Icon::CheckCircle));
                }
            }
            Err(e) => {
                say_error(ctx, crate::i18n::tf(format!("Could not import: {e}")));
            }
        }
    }

    /// Pick media files and add them to the project bin.
    pub fn import_media_files(&mut self, ctx: &egui::Context) {
        let dialog = import_filters().into_iter().fold(rfd::FileDialog::new().set_title("Import media"), |d, (name, exts)| d.add_filter(name, &exts));
        let Some(paths) = dialog.pick_files() else { return };
        self.import_paths(ctx, &paths);
    }

    /// Files dragged in from the desktop and dropped anywhere on the
    /// editor (the Library among it): what Import media takes is imported
    /// (media into the bin, models as 3D layers); anything else is named
    /// and left. Taken once: a panel that wants a drop of its own (Story,
    /// for scripts) takes it first.
    pub fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files)).iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect();
        if files.is_empty() {
            return;
        }
        let (take, refuse): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|p| importable(p));
        if let Some(first) = refuse.first() {
            let name = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let more = if refuse.len() > 1 { crate::i18n::tf(format!(" (and {} more)", refuse.len() - 1)) } else { String::new() };
            say_error(ctx, crate::i18n::tf(format!("Zornov cannot import {name}{more}: drop video, pictures, sound, Lottie or 3D models")));
        }
        if !take.is_empty() {
            self.import_paths(ctx, &take);
        }
    }

    /// Add files to the bin; returns the new media ids.
    pub fn import_paths(&mut self, ctx: &egui::Context, paths: &[PathBuf]) -> Vec<u32> {
        // 3D model files become model layers, not bin items.
        let (models, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths.iter().cloned().partition(|p| crate::mesh_store::is_model_file(p) && p.extension().is_some_and(|e| e != "smesh"));
        if !models.is_empty() {
            crate::mesh_store::import_models(self, ctx, &models);
            if paths.is_empty() {
                return Vec::new();
            }
        }
        let paths = &paths[..];
        // Files Studio cannot even open (WebM, MKV, AVI…) are converted in
        // the background and imported when ready.
        let (now_paths, later): (Vec<PathBuf>, Vec<PathBuf>) = paths.iter().cloned().partition(|p| crate::media_io::probe(p).is_ok());
        if !later.is_empty() {
            if self.agent.is_none() {
                self.agent = Some(crate::editor::agent_panel::AgentUi::new(ctx));
            }
            if let Some(a) = self.agent.as_mut() {
                a.import_later(ctx, later);
            }
            if now_paths.is_empty() {
                return Vec::new();
            }
        }
        let paths = &now_paths;
        let r = crate::services::project::import_media(&mut self.doc, paths);
        let now = ctx.input(|i| i.time);
        for &id in &r.ids {
            self.imports.push((id, now));
        }
        let n = r.ids.len();
        match (n, r.failed.first()) {
            (0, Some(f)) => say_error(ctx, crate::i18n::tf(format!("Could not import {f}"))),
            (_, Some(f)) => say_error(ctx, crate::i18n::tf(format!("Imported {n}; skipped {}", f))),
            (n, None) => kit::push(ctx, crate::i18n::tf(format!("Imported {n} item{}", if n == 1 { "" } else { "s" })), None, Some(Icon::DownloadSimple)),
        };
        r.ids
    }

    /// Autosave after 30 s of idle, keeping 20 backups (model rules).
    pub fn autosave_tick(&mut self) {
        if !self.prefs.autosave {
            return;
        }
        let rev = self.doc.history.revision();
        let now = std::time::Instant::now();
        if rev != self.autosave_rev {
            self.autosave_rev = rev;
            self.autosave.changed(now);
        }
        // A save writing in the background finished.
        if let Some((saved_rev, rx)) = &self.autosaving {
            match rx.try_recv() {
                Ok(done) => {
                    let saved_rev = *saved_rev;
                    self.autosaving = None;
                    match done {
                        Autosaved::Saved(synced) => {
                            autosave_health().ok();
                            self.doc.synced = Some(synced);
                            self.doc.history.mark_saved_at(saved_rev);
                            if let Some(root) = self.doc.folder.as_ref().map(|f| f.root().to_path_buf()) {
                                self.remember(&root);
                            }
                        }
                        // Someone else (an agent's session) saved the
                        // project meanwhile: take their work in and save
                        // the whole, here, rather than write over it.
                        Autosaved::Changed => match crate::services::project::save(&mut self.doc, None) {
                            Ok(_) => autosave_health().ok(),
                            Err(e) => self.autosave_failed(e),
                        },
                        Autosaved::Failed(e) => self.autosave_failed(e),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => self.autosaving = None,
            }
        }
        if let Some(folder) = &self.doc.folder {
            // Autosave saves the project (and keeps a backup), as the
            // setting says. The writing happens off the window's thread: a
            // long project's two files were a tenth of a second of freeze.
            if self.autosave.due(now) {
                self.autosave.mark_done();
                let (folder, model, rev, keep) = (folder.clone(), self.doc.model.clone(), rev, self.autosave.keep);
                let history = self.doc.history.snapshot();
                let expected = self.doc.synced.as_ref().map(|s| s.stamp);
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    // The backup is a second copy: its failure does not hold
                    // back the save itself, and the save's does show.
                    if let Err(e) = studio_model::file::autosave::write_backup(&folder, &model, std::time::SystemTime::now(), keep) {
                        eprintln!("zornov: autosave backup failed: {e}");
                    }
                    let done = match folder.save_if(&model, &Default::default(), expected) {
                        Ok(stamp) => {
                            // The undo history goes with the saved file, so a reopen after an autosave still undoes.
                            if let Err(e) = crate::services::project::keep_history_of(&folder, &history) {
                                eprintln!("zornov: autosave could not keep the undo history: {e}");
                            }
                            Autosaved::Saved(crate::doc::Synced { stamp, base: std::sync::Arc::new(model) })
                        }
                        Err(m::file::FileError::Changed { .. }) => Autosaved::Changed,
                        Err(e) => Autosaved::Failed(e.to_string()),
                    };
                    let _ = tx.send(done);
                });
                self.autosaving = Some((rev, rx));
            }
        }
    }

    /// An autosave did not write: say so once, and try again after the
    /// next idle time rather than give the work up.
    fn autosave_failed(&mut self, e: String) {
        autosave_health().failed(e);
        self.autosave.changed(std::time::Instant::now());
    }

    /// Every frame: when the window's project moved to another folder
    /// (opened, saved, or an agent opened or made one), the Start screen
    /// learns of it.
    pub fn recents_tick(&mut self) {
        let root = self.doc.folder.as_ref().map(|f| f.root().to_path_buf());
        if root.is_some() && root != self.remembered {
            let root = root.expect("folder");
            self.remember(&root);
        }
    }
}

/// A recent project's thumbnail seed (stable per path).
/// The picture that stands for a project on the start screen: the first
/// video or still in its media bin that is still on disk, with its length
/// in seconds (0 for a still). None when it has none or will not load.
pub fn cover_of(dir: &Path) -> Option<(PathBuf, f64)> {
    let file = m::file::ProjectFolder::new(dir).load().ok()?;
    file.project.media.iter().find_map(|item| {
        let path = PathBuf::from(&item.path);
        let picture = matches!(item.info.kind, m::MediaKind::Video | m::MediaKind::Image) && (item.info.kind == m::MediaKind::Image || item.info.video.is_some());
        (picture && path.is_file()).then(|| (path, item.info.duration.map_or(0.0, |d| d as f64 / 1e6)))
    })
}

pub fn seed_of(path: &Path) -> u32 {
    path.to_string_lossy().bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) & !crate::thumbs::FILE_BIT
}

/// Export the active sequence as OTIO next to the project (File menu).
pub fn export_otio(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(path) = rfd::FileDialog::new().set_title("Export OpenTimelineIO").set_file_name(format!("{}.otio", app.doc.project.name)).save_file() else { return };
    match m::interchange::otio::export(&app.doc.model, app.doc.seq()) {
        Ok((json, report)) => match m::file::write_atomic(&path, json.as_bytes()) {
            Ok(()) => {
                let note = if report.issues.is_empty() { String::new() } else { format!(" ({} notes)", report.issues.len()) };
                kit::push(ctx, crate::i18n::tf(format!("Exported OTIO{note}")), None, Some(Icon::CheckCircle));
            }
            Err(e) => {
                say_error(ctx, crate::i18n::tf(format!("Could not write: {e}")));
            }
        },
        Err(e) => {
            say_error(ctx, crate::i18n::tf(format!("Could not export: {e}")));
        }
    }
    let _ = bridge::SEED_KEY;
}

/// Writes `bytes` into `dir` as `<stem>-<content hash>.<ext>` unless that
/// very file is already there: whatever Studio makes is stored once, and
/// anything that differs in the slightest gets its own file.
pub fn write_once(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    // FNV-1a over the content.
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3));
    let path = dir.join(format!("{stem}-{hash:016x}.{ext}"));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// How an autosave written in the background went.
pub enum Autosaved {
    /// Written: the file as it now is.
    Saved(crate::doc::Synced),
    /// Someone else saved the project since; nothing was written.
    Changed,
    /// The save failed, and why.
    Failed(String),
}

/// Where the open project stands with saving, for the top bar.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveState {
    /// Saved in its folder, nothing waiting.
    Saved,
    /// Changes not saved yet (autosave will write them when idle).
    Unsaved,
    /// An autosave is writing now.
    Saving,
    /// The last autosave failed, and why; it retries.
    AutosaveFailed(String),
    /// No folder yet (a sample or the demo) and nothing changed.
    Scratch,
}

/// What is about to take the open project's place (or close it).
#[derive(Clone, Debug, PartialEq)]
pub enum Replace {
    Quit,
    Home,
    Open(PathBuf),
    ImportTimeline,
    /// A new project with its own folder.
    New(view::Project),
    /// A sample or the demo: scratch, never saved unless the person says.
    Sample(view::Project),
}

/// The answers to "Save changes?".
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unsaved {
    Save,
    Discard,
    Cancel,
}

enum Settled {
    Clean,
    Failed,
    Ask,
}

const PENDING: &str = "files-pending";
const OFFER: &str = "files-restore-offer";
const CRASHED: &str = "files-crashed";

/// Unsaved work an autosave holds for the project just opened.
#[derive(Clone, Debug)]
struct Offer {
    path: PathBuf,
    at_ms: u64,
}

fn set_offer(ctx: &egui::Context, offer: Option<Offer>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(OFFER), offer));
}

/// The recovery path restores by itself; it clears the offer a plain open made.
pub(crate) fn clear_offer(ctx: &egui::Context) {
    set_offer(ctx, None);
}

#[derive(Clone)]
struct RecoveredCache {
    at: f64,
    sessions: Vec<crate::safety::Session>,
}

/// (name, project folder, pid) of each waiting crashed session, looked up
/// every few seconds.
fn recovered_rows(ctx: &egui::Context, root: &Path) -> Vec<(String, PathBuf, u32)> {
    let now = ctx.input(|i| i.time);
    let id = egui::Id::new(CRASHED);
    let cache = ctx.data(|d| d.get_temp::<RecoveredCache>(id)).filter(|c| now - c.at < 3.0);
    let cache = cache.unwrap_or_else(|| {
        let c = RecoveredCache { at: now, sessions: crate::safety::crashed(root) };
        ctx.data_mut(|d| d.insert_temp(id, c.clone()));
        c
    });
    cache.sessions.iter().map(|s| (s.project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), s.project.clone(), s.pid)).collect()
}

fn pending(ctx: &egui::Context) -> Option<Replace> {
    ctx.data(|d| d.get_temp::<Option<Replace>>(egui::Id::new(PENDING))).flatten()
}

fn set_pending(ctx: &egui::Context, what: Option<Replace>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PENDING), what));
}

/// Where Save As would write, against the project that is open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SaveTarget {
    /// Nothing there yet.
    Fresh,
    /// The open project's own folder.
    Current,
    /// Another project is there and would be written over.
    Replaces,
}

pub fn save_target(dir: &Path, current: Option<&Path>) -> SaveTarget {
    if !m::file::ProjectFolder::is_project(dir) {
        return SaveTarget::Fresh;
    }
    let same = |a: &Path, b: &Path| a == b || std::fs::canonicalize(a).ok().is_some_and(|a| std::fs::canonicalize(b).ok() == Some(a));
    if current.is_some_and(|c| same(c, dir)) { SaveTarget::Current } else { SaveTarget::Replaces }
}

/// Whether autosave is working: the last failure, and whether the person
/// has been told about it yet.
#[derive(Default)]
pub struct SaveHealth {
    failure: Option<String>,
    warned: bool,
}

impl SaveHealth {
    pub fn failed(&mut self, why: String) {
        self.failure = Some(why);
    }
    pub fn ok(&mut self) {
        *self = Self::default();
    }
    /// The failure, once per run of failures.
    pub fn take_warning(&mut self) -> Option<String> {
        let e = self.failure.clone().filter(|_| !self.warned)?;
        self.warned = true;
        Some(e)
    }
}

static HEALTH: std::sync::Mutex<SaveHealth> = std::sync::Mutex::new(SaveHealth { failure: None, warned: false });

struct HealthGuard(std::sync::MutexGuard<'static, SaveHealth>);

impl std::ops::Deref for HealthGuard {
    type Target = SaveHealth;
    fn deref(&self) -> &SaveHealth {
        &self.0
    }
}
impl std::ops::DerefMut for HealthGuard {
    fn deref_mut(&mut self) -> &mut SaveHealth {
        &mut self.0
    }
}

fn autosave_health() -> HealthGuard {
    HealthGuard(HEALTH.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Why the last autosave failed, while it is still failing (for the top bar).
pub fn autosave_failure() -> Option<String> {
    autosave_health().failure.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_folders_are_told_from_real_ones() {
        let tmp = std::env::temp_dir();
        assert!(is_temporary(&tmp.join("project")));
        assert!(is_temporary(&std::fs::canonicalize(&tmp).unwrap().join("project")));
        assert!(is_temporary(Path::new("/private/var/folders/x")));
        assert!(!is_temporary(Path::new("/Users/someone/Videos/Studio/Film")));
    }

    #[derive(Debug)]
    struct Dropped(PathBuf);
    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    /// Files dragged in from the desktop are imported by type: a picture
    /// into the bin, a model as a 3D layer; anything else is named and left.
    #[test]
    fn files_dropped_on_the_window_are_imported_by_type() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        app.doc = crate::agent::Session::new(crate::services::project::new_custom("Drop", 640, 360, 30.0)).doc;
        let dir = std::env::temp_dir().join(format!("studio-window-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("red.png");
        std::fs::write(&pic, studio_media::png::encode(&studio_media::RgbaImage { width: 2, height: 2, pixels: vec![255, 0, 0, 255].repeat(4) })).unwrap();
        let model = dir.join("tri.obj");
        std::fs::write(&model, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let other = dir.join("notes.docx");
        std::fs::write(&other, b"PK").unwrap();
        let files = [pic, model, other];
        let dropped_files = files.iter().map(|p| std::sync::Arc::new(Dropped(p.clone())) as std::sync::Arc<dyn egui::DroppedFile + Send + Sync>).collect();
        let (media, clips) = (app.doc.project.media.len(), app.doc.model.active().clips.len());
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), time: Some(1.0), dropped_files, ..Default::default() };
        crate::editor::test_frame(&ctx, input, |ui| app.take_dropped_files(ui.ctx()));
        assert_eq!(app.doc.project.media.len(), media + 1, "the picture is in the bin");
        let models = app.doc.model.active().clips.iter().filter(|c| crate::mesh_store::file_of(c).is_some()).count();
        assert_eq!((app.doc.model.active().clips.len(), models), (clips + 1, 1), "the model is a 3D layer");
        // Taken once: the next frame imports nothing again.
        crate::editor::test_frame(&ctx, egui::RawInput { time: Some(2.0), ..Default::default() }, |ui| app.take_dropped_files(ui.ctx()));
        assert_eq!(app.doc.project.media.len(), media + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// File ▸ Import media offers what the decoders and importers read:
    /// WebM and MKV (converted), TIFF, EXR and DPX stills, every model file.
    #[test]
    fn import_media_offers_every_format_zornov_reads() {
        let filters = import_filters();
        let (name, all) = &filters[0];
        assert_eq!(*name, "All media");
        for e in ["mp4", "mov", "m4v", "webm", "mkv", "png", "jpg", "jpeg", "tif", "tiff", "exr", "dpx", "psd", "wav", "mp3", "flac", "ogg", "aif", "m4a", "json", "glb", "gltf", "obj", "stl", "fbx", "ply", "smodel", "usda", "usdz"] {
            assert!(all.contains(&e), "Import media does not offer .{e}");
            assert!(importable(Path::new(&format!("clip.{}", e.to_uppercase()))), ".{e} is importable, whatever its case");
        }
        for (group, exts) in &filters[1..] {
            for e in exts {
                assert!(all.contains(e), "{group}: .{e} is missing from All media");
            }
        }
        assert!(!importable(Path::new("notes.docx")));
        // What it offers really imports: the pro stills.
        let dir = std::env::temp_dir().join(format!("studio-import-filters-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let px = vec![[0.5, 0.25, 0.1, 1.0]; 8];
        let files = [("still.tif", studio_imageio::tiff::encode(4, 2, &px, false)), ("still.exr", studio_imageio::exr::encode(4, 2, &px, &Default::default())), ("still.dpx", studio_imageio::dpx::encode(4, 2, &px, &Default::default()))];
        let paths: Vec<PathBuf> = files.iter().map(|(n, b)| {
            let p = dir.join(n);
            std::fs::write(&p, b).unwrap();
            p
        }).collect();
        let mut doc = crate::agent::Session::new(crate::services::project::new_custom("Import", 640, 360, 30.0)).doc;
        let r = crate::services::project::import_media(&mut doc, &paths);
        assert_eq!(r.ids.len(), 3, "failed: {:?}", r.failed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn app_in(name: &str) -> (StudioApp, egui::Context, PathBuf) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let dir = std::env::temp_dir().join(format!("zornov-flow-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (app, ctx, dir)
    }

    fn saved_project(dir: &Path, name: &str) -> PathBuf {
        let mut doc = crate::services::project::new_custom(name, 640, 360, 30.0);
        let root = dir.join(format!("{name}.zornov"));
        crate::services::project::save(&mut doc, Some(&root)).unwrap();
        root
    }

    /// PR-2: opening another project saves the open one first, so its edits
    /// are on disk (and still undoable) when it is opened again.
    #[test]
    fn opening_another_project_saves_the_open_one_first() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx, dir) = app_in("open-saves");
        let a = saved_project(&dir, "A");
        let b = saved_project(&dir, "B");
        app.open_folder(&ctx, Some(a.clone()));
        assert_eq!(app.doc.project.name, "A");
        app.doc.edit("Rename", |p| p.name = "A edited".into());
        assert!(app.has_unsaved());
        app.open_folder(&ctx, Some(b));
        assert_eq!(app.doc.project.name, "B", "the other project opened");
        assert!(pending(&ctx).is_none(), "a project with a folder is saved, not asked about");
        app.open_folder(&ctx, Some(a));
        assert_eq!(app.doc.project.name, "A edited", "the edit was saved before B replaced it");
        assert!(!app.has_unsaved());
        assert_eq!(app.save_state(), SaveState::Saved);
        app.doc.edit("Rename", |p| p.name = "A again".into());
        assert_eq!(app.save_state(), SaveState::Unsaved);
        app.doc.undo();
        assert!(app.doc.history.can_undo(), "the undo history came back with the project (opened through the service)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-1 / PR-2: unsaved work without a folder is asked about; Cancel
    /// keeps it, Don't save replaces it, and nothing is replaced meanwhile.
    #[test]
    fn unsaved_work_without_a_folder_is_asked_about() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx, dir) = app_in("ask");
        let a = saved_project(&dir, "A");
        app.doc = crate::services::project::new_custom("Scratch", 640, 360, 30.0);
        app.doc.edit("Rename", |p| p.name = "Scratch 2".into());
        app.open_folder(&ctx, Some(a.clone()));
        assert_eq!(pending(&ctx), Some(Replace::Open(a.clone())), "asked, not replaced");
        assert_eq!(app.doc.project.name, "Scratch 2");
        app.answer_unsaved(&ctx, Replace::Open(a.clone()), Unsaved::Cancel);
        assert!(pending(&ctx).is_none());
        assert_eq!(app.doc.project.name, "Scratch 2", "Cancel keeps the work");
        app.open_folder(&ctx, Some(a.clone()));
        app.answer_unsaved(&ctx, Replace::Open(a), Unsaved::Discard);
        assert_eq!(app.doc.project.name, "A", "Don't save lets it go");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-1: a save that fails does not close or replace anything.
    #[cfg(unix)]
    #[test]
    fn a_failed_save_keeps_the_project_open() {
        use std::os::unix::fs::PermissionsExt;
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx, dir) = app_in("fail");
        let a = saved_project(&dir, "A");
        let b = saved_project(&dir, "B");
        app.open_folder(&ctx, Some(a.clone()));
        app.doc.edit("Rename", |p| p.name = "A edited".into());
        let mode = std::fs::metadata(&a).unwrap().permissions();
        std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::File::create(a.join("probe")).is_ok() {
            // Running with rights that ignore the mode: nothing to prove here.
            std::fs::set_permissions(&a, mode).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        app.open_folder(&ctx, Some(b));
        std::fs::set_permissions(&a, mode).unwrap();
        assert_eq!(app.doc.project.name, "A edited", "still open");
        assert!(app.has_unsaved(), "and still unsaved");
        assert!(pending(&ctx).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-3: a project without a folder gets one; a failure is returned.
    #[test]
    fn a_new_project_is_given_a_folder_or_the_failure() {
        let _serial = crate::ui_tests::serial();
        let (mut app, _ctx, dir) = app_in("folder");
        app.doc = crate::services::project::new_custom("Custom", 800, 600, 30.0);
        app.give_folder_at(&dir.join("Custom.zornov")).unwrap();
        assert!(app.doc.folder.is_some() && !app.has_unsaved());
        assert!(dir.join("Custom.zornov").join(m::file::PROJECT_FILE).is_file());
        // A file in the way: the error comes back, the project stays unsaved.
        app.doc = crate::services::project::new_custom("Blocked", 800, 600, 30.0);
        std::fs::write(dir.join("Blocked.zornov"), b"x").unwrap();
        assert!(app.give_folder_at(&dir.join("Blocked.zornov")).is_err());
        assert!(app.doc.folder.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-4: a failed autosave is reported once per run of failures, and
    /// stays visible until one works.
    #[test]
    fn autosave_failures_are_told_once_and_clear_on_success() {
        let mut h = SaveHealth::default();
        assert!(h.take_warning().is_none());
        h.failed("disk full".into());
        assert_eq!(h.take_warning().as_deref(), Some("disk full"));
        h.failed("disk full".into());
        assert!(h.take_warning().is_none(), "once");
        h.ok();
        h.failed("read-only".into());
        assert_eq!(h.take_warning().as_deref(), Some("read-only"), "a new run of failures is told again");
    }

    /// PR-5: an autosave keeps the undo history, so a reopen still undoes.
    #[test]
    fn the_undo_history_survives_an_autosave() {
        let dir = std::env::temp_dir().join(format!("zornov-flow-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("H.zornov");
        let mut doc = crate::services::project::new_custom("H", 640, 360, 30.0);
        crate::services::project::save(&mut doc, Some(&root)).unwrap();
        doc.edit("Rename", |p| p.name = "H2".into());
        // What the autosave thread does.
        let folder = doc.folder.clone().unwrap();
        folder.save_if(&doc.model, &Default::default(), doc.synced.as_ref().map(|s| s.stamp)).unwrap();
        crate::services::project::keep_history_of(&folder, &doc.history.snapshot()).unwrap();
        let mut again = crate::services::project::open(&root).unwrap();
        assert_eq!(again.project.name, "H2");
        assert!(again.history.can_undo(), "the rename is still undoable after reopening");
        assert!(again.undo().is_some());
        assert_eq!(again.project.name, "H");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-9: Save As tells a fresh place from the open project from another
    /// project it would write over.
    #[test]
    fn save_as_knows_when_it_would_replace_a_project() {
        let dir = std::env::temp_dir().join(format!("zornov-flow-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = saved_project(&dir, "A");
        let b = saved_project(&dir, "B");
        assert_eq!(save_target(&dir.join("New.zornov"), Some(&a)), SaveTarget::Fresh);
        assert_eq!(save_target(&a, Some(&a)), SaveTarget::Current);
        assert_eq!(save_target(&b, Some(&a)), SaveTarget::Replaces);
        assert_eq!(save_target(&b, None), SaveTarget::Replaces);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-8: opening a project whose autosave holds unsaved work offers it;
    /// restoring makes the project unsaved and undoable.
    #[test]
    fn unsaved_autosave_work_is_offered_and_restored_as_an_edit() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx, dir) = app_in("offer");
        let a = saved_project(&dir, "A");
        let folder = m::file::ProjectFolder::new(&a);
        let mut d = crate::services::project::open(&a).unwrap();
        d.edit("Rename", |p| p.name = "A restored".into());
        let newer = d.model.clone();
        let when = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
        let backup = m::file::autosave::write_backup(&folder, &newer, when, 5).unwrap();
        app.open_folder(&ctx, Some(a));
        let offer = ctx.data(|d| d.get_temp::<Option<Offer>>(egui::Id::new(OFFER))).flatten();
        assert_eq!(offer.as_ref().map(|o| o.path.clone()), Some(backup.clone()), "the newer autosave is offered");
        assert!(!app.has_unsaved(), "nothing is applied without asking");
        assert!(app.restore_autosave(&ctx, &backup));
        assert!(app.has_unsaved(), "restored work reads as unsaved");
        assert_eq!(app.doc.project.name, "A restored");
        app.doc.undo();
        assert_eq!(app.doc.project.name, "A", "and one undo takes it back");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR-11: sessions that ended without a clean quit are listed on Start,
    /// and opening or dismissing one takes it off the list.
    #[test]
    fn crashed_sessions_are_listed_and_cleared_one_by_one() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx, dir) = app_in("crashed");
        let a = saved_project(&dir, "A");
        let b = saved_project(&dir, "B");
        let home = dir.join("home");
        let dead = || {
            let mut c = std::process::Command::new("true").spawn().unwrap();
            c.wait().unwrap();
            c.id()
        };
        let (p1, p2) = (dead(), dead());
        assert_ne!(p1, p2);
        crate::safety::save_session(&home, &crate::safety::Session { pid: p1, project: a.clone(), ..Default::default() }).unwrap();
        crate::safety::save_session(&home, &crate::safety::Session { pid: p2, project: b.clone(), ..Default::default() }).unwrap();
        let mut names: Vec<String> = recovered_rows(&ctx, &home).into_iter().map(|r| r.0).collect();
        names.sort();
        assert_eq!(names, ["A", "B"]);
        app.choose_recovered(&ctx, &home, p1, None);
        assert_eq!(crate::safety::crashed(&home).len(), 1, "dismissed");
        app.choose_recovered(&ctx, &home, p2, Some(b));
        assert_eq!(app.doc.project.name, "B", "opened");
        assert!(crate::safety::crashed(&home).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_projects_are_named_as_packages() {
        assert_eq!(package_path(PathBuf::from("/a/My film")), PathBuf::from("/a/My film.zornov"));
        assert_eq!(package_path(PathBuf::from("/a/My film.zornov")), PathBuf::from("/a/My film.zornov"));
        assert_eq!(package_path(PathBuf::from("/a/My film.ZORNOV")), PathBuf::from("/a/My film.ZORNOV"));
        assert_eq!(package_path(PathBuf::from("/a/v1.2")), PathBuf::from("/a/v1.2.zornov"), "an unrelated dot is not the extension");
        let name = free_project_dir("My: film/2");
        assert_eq!(name.file_name().unwrap().to_string_lossy(), "My- film-2.zornov");
    }
}
