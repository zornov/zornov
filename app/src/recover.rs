// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The window's side of crash recovery: while a project is open its place
//! (playhead, selection, timeline view) is written as it changes; a clean
//! quit removes that note; if Studio starts and finds a note whose window
//! is gone, it reopens the project exactly there (see [`crate::safety`]).

use crate::app::StudioApp;
use eframe::egui;
use crate::icons::Icon;
use crate::kit;
use crate::safety::{self, Session};
use serde_json::json;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static LAST: Mutex<Option<(Instant, Session)>> = Mutex::new(None);

/// Whether the last note was written less than half a second ago.
fn too_soon(last: Option<Instant>) -> bool {
    last.is_some_and(|t| t.elapsed() < Duration::from_millis(500))
}

impl StudioApp {
    /// Every frame: notes where the window is, at most twice a second and
    /// only when something moved.
    pub fn keep_session(&mut self) {
        if self.cli.shot.is_some() || self.cli.fresh {
            return;
        }
        // The throttle comes first: the checks below touch the filesystem.
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if too_soon(last.as_ref().map(|(t, _)| *t)) {
            return;
        }
        let Some(root) = self.doc.folder.as_ref().map(|f| f.root().to_path_buf()).filter(|r| !crate::files::is_temporary(r)) else { return };
        let e = &self.editor;
        let mut view = serde_json::Map::new();
        view.insert("px_per_sec".into(), json!(e.px_per_sec));
        view.insert("scroll".into(), json!([e.tl_scroll.x, e.tl_scroll.y]));
        if let Some(z) = e.zoom {
            view.insert("zoom".into(), json!(z));
        }
        view.insert("library_query".into(), json!(e.library_query));
        let now = Session { pid: std::process::id(), project: root, playhead: e.playhead, selection: e.selection.clone(), sequence: self.doc.seq().0, view, saved_ms: 0 };
        if last.as_ref().is_some_and(|(_, s)| *s == now) {
            return;
        }
        safety::save_session(&studio_paths::studio(), &now);
        *last = Some((Instant::now(), now));
    }

    /// At start: if the last window ended without a clean quit, reopen its
    /// project where it was. Returns whether something was recovered.
    pub fn recover_after_crash(&mut self, ctx: &egui::Context) -> bool {
        // A project the command line (or Finder) already opened is what the
        // person asked for: recovery does not replace it.
        if self.cli.shot.is_some() || self.cli.fresh || self.cli.open.is_some() || self.doc.folder.is_some() {
            return false;
        }
        let root = studio_paths::studio();
        let found = safety::crashed(&root);
        // The newest comes back now; the others stay noted and come back the
        // next time Zornov starts, so none is dropped unseen.
        let Some(session) = found.first().cloned() else { return false };
        let more = found.len() - 1;
        safety::dismiss(&root, session.pid);
        // Work saved after the last full save (an autosave newer than the project file) wins.
        let newer = safety::newer_autosave(&session.project);
        self.open_folder(ctx, Some(session.project.clone()));
        if self.doc.folder.is_none() {
            return false;
        }
        if let Some(snap) = &newer {
            // As an edit on top of the saved file: the project is unsaved
            // again (quit asks, autosave writes it) and one undo undoes it.
            match std::fs::read_to_string(&snap.path).map_err(|e| e.to_string()).and_then(|t| studio_model::file::from_json(&t).map_err(|e| e.to_string())) {
                Ok(file) => {
                    if let Err(e) = self.doc.take_in("Recovered changes", &file.project) {
                        kit::push(ctx, crate::i18n::tf(format!("Could not restore your last unsaved changes: {e}")), None, Some(Icon::Warning));
                    }
                }
                Err(e) => {
                    kit::push(ctx, crate::i18n::tf(format!("Could not read your last unsaved changes: {e}")), None, Some(Icon::Warning));
                }
            }
        }
        let e = &mut self.editor;
        e.playhead = session.playhead.clamp(0, self.doc.project.duration().max(0));
        e.selection = session.selection.iter().copied().filter(|id| self.doc.project.clip(*id).is_some()).collect();
        if let Some(v) = session.view.get("px_per_sec").and_then(|v| v.as_f64()) {
            e.px_per_sec = v as f32;
        }
        if let Some(s) = session.view.get("scroll").and_then(|v| v.as_array()) {
            e.tl_scroll = egui::vec2(s.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32, s.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32);
        }
        e.zoom = session.view.get("zoom").and_then(|v| v.as_f64()).map(|z| z as f32);
        if let Some(q) = session.view.get("library_query").and_then(|v| v.as_str()) {
            e.library_query = q.to_owned();
        }
        let name = self.doc.project.name.clone();
        let tail = match more {
            0 => String::new(),
            1 => "; 1 more project to recover next time".into(),
            n => format!("; {n} more projects to recover next time"),
        };
        kit::push(ctx, crate::i18n::tf(format!("Zornov closed unexpectedly. {name} is back where you were{}{tail}", if newer.is_some() { ", with your last unsaved changes" } else { "" })), None, Some(Icon::CheckCircle));
        true
    }
}

/// A clean quit: there is nothing to recover.
pub fn clean_quit() {
    safety::end_session(&studio_paths::studio(), std::process::id());
}

static LIBRARY: Mutex<Option<(Instant, std::path::PathBuf, u64)>> = Mutex::new(None);

impl StudioApp {
    /// Every frame: when the project's media changed (or a project was
    /// opened), the asset library learns of it in the background.
    pub fn library_tick(&mut self) {
        if self.cli.shot.is_some() || self.cli.fresh || self.editor.playing {
            return;
        }
        let Some(root) = self.doc.folder.as_ref().map(|f| f.root().to_path_buf()) else { return };
        let key = self.doc.model.media.iter().fold(self.doc.model.media.len() as u64, |h, m| h.wrapping_mul(31).wrapping_add(m.id.0).wrapping_add(m.path.len() as u64));
        let mut last = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref().is_some_and(|(t, r, k)| (*r == root && *k == key) || t.elapsed() < Duration::from_secs(5)) {
            return;
        }
        if crate::agent::care::sync_and_index(root.clone(), self.doc.model.clone()) {
            *last = Some((Instant::now(), root, key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_not_due_within_half_a_second_of_the_last() {
        assert!(!too_soon(None));
        assert!(too_soon(Some(Instant::now())));
        assert!(!too_soon(Instant::now().checked_sub(Duration::from_secs(1))));
    }

    /// PR-8: work recovered from an autosave comes back as an edit, so the
    /// project reads unsaved (quit asks, autosave writes it) and one undo
    /// undoes it.
    #[test]
    fn recovered_changes_are_an_unsaved_undoable_edit() {
        let mut doc = crate::services::project::new_custom("R", 640, 360, 30.0);
        let mut backup = doc.model.clone();
        backup.name = "R recovered".into();
        assert!(!doc.dirty());
        doc.take_in("Recovered changes", &backup).unwrap();
        assert!(doc.dirty());
        assert_eq!(doc.project.name, "R recovered");
        doc.undo();
        assert_eq!(doc.project.name, "R");
    }
}
