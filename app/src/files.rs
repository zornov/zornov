// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project folders, media import and interchange, through the model's
//! `file` and `interchange` modules and native file dialogs.

use crate::app::StudioApp;
use crate::bridge;
use crate::icons::Icon;
use crate::kit;
use crate::media_io;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use studio_model as m;

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

/// A folder for a new project called `name` that does not exist yet:
/// "Name", then "Name 2", …
pub fn free_project_dir(name: &str) -> PathBuf {
    let clean: String = name.chars().map(|c| if matches!(c, '/' | ':' | '\\') { '-' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.');
    let base = if clean.is_empty() { "Untitled" } else { clean };
    let dir = projects_dir();
    (1..)
        .map(|n| if n == 1 { dir.join(base) } else { dir.join(format!("{base} {n}")) })
        .find(|p| !p.exists())
        .expect("a free name")
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
    let _ = std::fs::create_dir_all(studio_paths::studio());
    let _ = m::file::write_atomic(&path, text.as_bytes());
}

/// A folder under the system's temporary places.
pub fn is_temporary(dir: &Path) -> bool {
    let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    dir.starts_with(&tmp) || ["/tmp", "/private/tmp", "/private/var/folders", "/var/folders"].iter().any(|t| dir.starts_with(t))
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
    /// away and autosave keeps it.
    pub fn give_folder(&mut self) {
        if self.doc.folder.is_some() {
            return;
        }
        let dir = free_project_dir(&self.doc.project.name);
        let Ok(folder) = m::file::ProjectFolder::create(&dir) else { return };
        if folder.save(&self.doc.model).is_ok() {
            self.doc.history.mark_saved();
            self.doc.folder = Some(folder);
            self.remember(&dir);
        }
    }

    /// Save to the project's folder, asking for one the first time.
    pub fn save(&mut self, ctx: &egui::Context, save_as: bool) {
        let folder = match (&self.doc.folder, save_as) {
            (Some(f), false) => f.clone(),
            _ => {
                let Some(dir) = rfd::FileDialog::new().set_title("Save project").set_file_name(&self.doc.project.name).save_file() else { return };
                match m::file::ProjectFolder::create(&dir) {
                    Ok(f) => f,
                    Err(e) => {
                        kit::push(ctx, crate::i18n::tf(format!("Could not create the folder: {e}")), None, Some(Icon::Warning));
                        return;
                    }
                }
            }
        };
        match folder.save(&self.doc.model) {
            Ok(()) => {
                self.doc.history.mark_saved();
                let root = folder.root().to_path_buf();
                self.doc.folder = Some(folder);
                self.remember(&root);
                kit::push(ctx, crate::i18n::tf(format!("Saved {}", self.doc.project.name)), None, Some(Icon::CheckCircle));
            }
            Err(e) => {
                kit::push(ctx, crate::i18n::tf(format!("Could not save: {e:?}")), None, Some(Icon::Warning));
            }
        }
    }

    /// Open a project folder (ask when `path` is None).
    pub fn open_folder(&mut self, ctx: &egui::Context, path: Option<PathBuf>) {
        let Some(dir) = path.or_else(|| rfd::FileDialog::new().set_title("Open project").pick_folder()) else { return };
        let folder = m::file::ProjectFolder::new(&dir);
        match folder.load() {
            Ok(file) => {
                let mut project = file.project;
                folder.resolve_media(&mut project);
                self.open_model(project);
                self.doc.folder = Some(folder);
                self.remember(&dir);
            }
            Err(e) => {
                self.prefs.recents.retain(|r| r.path != dir);
                kit::push(ctx, crate::i18n::tf(format!("Could not open {}: {e:?}", dir.display())), None, Some(Icon::Warning));
            }
        }
    }

    /// Import OpenTimelineIO or FCPXML as a new project.
    pub fn import_timeline(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().set_title("Import timeline").add_filter("Timeline", &["otio", "fcpxml", "xml"]).pick_file() else { return };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                kit::push(ctx, crate::i18n::tf(format!("Could not read the file: {e}")), None, Some(Icon::Warning));
                return;
            }
        };
        let mut project = m::Project::new(m::ProjectSettings::default());
        let is_otio = path.extension().is_some_and(|e| e == "otio");
        let imported = if is_otio { m::interchange::otio::import(&mut project, &text) } else { m::interchange::fcpxml::import(&mut project, &text) };
        match imported {
            Ok(imp) => {
                if let Err(e) = project.apply(imp.edit) {
                    kit::push(ctx, crate::i18n::tf(format!("Import failed: {e:?}")), None, Some(Icon::Warning));
                    return;
                }
                if let Some(seq) = project.sequences.iter().position(|s| s.id == imp.sequence) {
                    project.active_sequence = project.sequences[seq].id;
                }
                self.open_model(project);
                self.give_folder();
                let n = imp.report.issues.len();
                if n > 0 {
                    let first = &imp.report.issues[0];
                    kit::push(ctx, crate::i18n::tf(format!("Imported with {n} note{}: {} — {}", if n == 1 { "" } else { "s" }, first.item, first.message)), None, Some(Icon::Info));
                } else {
                    kit::push(ctx, "Timeline imported", None, Some(Icon::CheckCircle));
                }
            }
            Err(e) => {
                kit::push(ctx, crate::i18n::tf(format!("Could not import: {e:?}")), None, Some(Icon::Warning));
            }
        }
    }

    /// Pick media files and add them to the project bin.
    pub fn import_media_files(&mut self, ctx: &egui::Context) {
        let mut exts: Vec<&str> = media_io::VIDEO_EXT.to_vec();
        exts.extend(["glb", "gltf", "obj", "stl"]);
        exts.extend(media_io::AUDIO_EXT);
        exts.extend(media_io::IMAGE_EXT);
        exts.extend(["psd", "psb"]);
        exts.extend(media_io::LOTTIE_EXT);
        let Some(paths) = rfd::FileDialog::new().set_title("Import media").add_filter("Media", &exts).pick_files() else { return };
        self.import_paths(ctx, &paths);
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
            (0, Some(f)) => kit::push(ctx, crate::i18n::tf(format!("Could not import {f}")), None, Some(Icon::Warning)),
            (_, Some(f)) => kit::push(ctx, crate::i18n::tf(format!("Imported {n}; skipped {}", f)), None, Some(Icon::Warning)),
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
                Ok(ok) => {
                    let saved_rev = *saved_rev;
                    self.autosaving = None;
                    if ok {
                        self.doc.history.mark_saved_at(saved_rev);
                        if let Some(root) = self.doc.folder.as_ref().map(|f| f.root().to_path_buf()) {
                            self.remember(&root);
                        }
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
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let backed_up = studio_model::file::autosave::write_backup(&folder, &model, std::time::SystemTime::now(), keep).is_ok();
                    let _ = tx.send(backed_up && folder.save(&model).is_ok());
                });
                self.autosaving = Some((rev, rx));
            }
        }
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
                kit::push(ctx, crate::i18n::tf(format!("Could not write: {e}")), None, Some(Icon::Warning));
            }
        },
        Err(e) => {
            kit::push(ctx, crate::i18n::tf(format!("Could not export: {e:?}")), None, Some(Icon::Warning));
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
