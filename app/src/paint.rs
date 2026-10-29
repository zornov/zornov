// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pictures open in Studio: one set shared by the Paint workspace and the
//! AI editor, so what the AI paints shows in the window and what a person
//! paints the AI can see.
//!
//! Pictures live in the project's `Pictures` folder: `<name>.pcraft` (the
//! lossless picture: layers, masks, adjustments, smart objects, history
//! snapshots are session-only), `<name>.png` (the flattened picture the
//! timeline shows), and for animations `<name>.flipbook.json` and a
//! `<name>.frames/` folder of rendered drawings. A person's own files are
//! only read: opening one makes a copy here.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use studio_canvas::Canvas;
use studio_canvas::flipbook::Flipbook;
use studio_model as m;

static CANVAS: OnceLock<Mutex<Canvas>> = OnceLock::new();

/// The open pictures.
pub fn canvas() -> MutexGuard<'static, Canvas> {
    let m = CANVAS.get_or_init(|| Mutex::new(Canvas::new(None)));
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Where a project's pictures are kept.
pub fn pictures_dir(project_root: Option<&Path>) -> PathBuf {
    match project_root {
        Some(r) => m::file::ProjectFolder::new(r).media_of(m::file::MediaKind::Pictures),
        None => studio_paths::studio().join("Pictures"),
    }
}

/// Point the canvas at a project's pictures (new pictures and saves go there).
pub fn use_dir(c: &mut Canvas, dir: PathBuf) {
    if c.dir.as_deref() != Some(dir.as_path()) {
        c.dir = Some(dir);
    }
}

/// The saved pictures in `dir`, most recently changed first.
pub fn saved(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == studio_canvas::files::NATIVE))
                .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// Open a picture saved in the project (and its animation timing), keeping
/// it at home there so saves go back to it.
pub fn open_saved(c: &mut Canvas, file: &Path) -> Result<(), String> {
    let name = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if c.activate(&name).is_ok() {
        return Ok(());
    }
    let o = studio_canvas::files::open(file)?;
    let mut doc = o.document;
    doc.name = name;
    c.engine.add_document(doc, None);
    if let Some(m) = c.meta_mut() {
        m.file = Some(file.to_path_buf());
        m.flipbook = Flipbook::load(&file.with_extension("flipbook.json"));
    }
    if c.meta().and_then(|m| m.flipbook.as_ref()).is_some() {
        let cur = c.meta().and_then(|m| m.flipbook.as_ref()).map(|f| f.current).unwrap_or(0);
        Flipbook::show(c, cur)?;
    }
    Ok(())
}

/// Make `name` the active picture, opening it from `dir` if it was saved
/// there; with no name and nothing open, the most recent saved picture.
pub fn bring(c: &mut Canvas, dir: &Path, name: Option<&str>) -> Result<(), String> {
    match name {
        Some(n) => {
            if c.activate(n).is_ok() {
                return Ok(());
            }
            let want = n.trim().to_lowercase();
            let hit = saved(dir).into_iter().find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == want));
            match hit {
                Some(p) => open_saved(c, &p),
                None => Err(format!("no picture \"{n}\"; open: {}; saved: {}", c.names().join(", "), saved(dir).iter().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", "))),
            }
        }
        None => {
            if c.doc().is_none() {
                if let Some(p) = saved(dir).into_iter().next() {
                    open_saved(c, &p)?;
                }
            }
            Ok(())
        }
    }
}

/// The open pictures, unless something is working on them right now (the
/// window never waits on a long AI or filter run).
pub fn try_canvas() -> Option<MutexGuard<'static, Canvas>> {
    let m = CANVAS.get_or_init(|| Mutex::new(Canvas::new(None)));
    match m.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

static MODELS: OnceLock<Mutex<studio_canvas::vision::LocalModels>> = OnceLock::new();

/// Run `f` with the local models for AI selections (loaded once, shared by
/// the window and the AI editor). Take the canvas first when both are needed.
pub fn with_models<R>(f: impl FnOnce(&mut dyn studio_canvas::vision::Vision) -> R) -> R {
    let m = MODELS.get_or_init(|| Mutex::new(studio_canvas::vision::LocalModels::installed()));
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    f(&mut *g)
}

/// Run picture ops on the open pictures (as the AI editor's `picture` tool
/// does), all or nothing.
pub fn apply(ops: &[serde_json::Value]) -> Result<serde_json::Value, String> {
    let mut c = canvas();
    with_models(|v| studio_canvas::ops::apply(&mut c, ops, v))
}

/// Tests that use the shared pictures take turns.
#[cfg(test)]
pub fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}
