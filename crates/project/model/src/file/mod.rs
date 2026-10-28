// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The project file format and project folders.
//!
//! A project lives in a folder:
//!
//! ```text
//! My Video/
//!   project.json   the document (this module)
//!   media/         optional copies of imported media
//!   cache/         thumbnails, waveforms, proxies — safe to delete
//!   backups/       rolling autosaves (see [`autosave`])
//! ```
//!
//! `project.json` is `{"format": "studio-project", "version": N,
//! "project": {...}}`. Fields this version does not understand — at the top
//! level or inside any object that has an `extra` map — are kept and
//! written back unchanged, so opening a project in an older Studio does not
//! destroy what a newer one stored. Older versions are upgraded on load by
//! the functions in [`migrate`]. Writes are atomic: a crash mid-save leaves
//! the previous file intact.

pub mod autosave;
pub mod migrate;

use crate::edit::Violation;
use crate::project::Project;
use crate::value::Extra;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub use migrate::CURRENT_VERSION;

/// The value of the top-level `format` field.
pub const FORMAT: &str = "studio-project";

/// File name of the document inside a project folder.
pub const PROJECT_FILE: &str = "project.json";

/// The top-level JSON document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    /// Always [`FORMAT`].
    pub format: String,
    /// Format version the document follows.
    pub version: u64,
    /// The project.
    pub project: Project,
    /// Unknown top-level fields, preserved.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Why a project could not be read or written.
#[derive(Debug)]
pub enum FileError {
    /// The file system refused.
    Io(std::io::Error),
    /// The file is not valid JSON or does not match the schema.
    Json(serde_json::Error),
    /// The JSON is not a Studio project.
    NotAProject,
    /// The file was written by a newer Studio.
    TooNew {
        /// Version in the file.
        found: u64,
        /// Newest version this build reads.
        supported: u64,
    },
    /// Upgrading from an older version failed.
    Migration(String),
    /// The project breaks an invariant (the file was edited by hand or damaged).
    Invalid(Violation),
    /// Someone else saved the project since it was loaded (another window,
    /// another agent's session): the save was refused, nothing was
    /// written. `on_disk` is the [`stamp_of`] the file has now.
    Changed {
        on_disk: u64,
    },
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::Io(e) => write!(f, "file error: {e}"),
            FileError::Json(e) => write!(f, "project file is malformed: {e}"),
            FileError::NotAProject => write!(f, "not a Zornov project file"),
            FileError::TooNew { found, supported } => write!(f, "project was saved by a newer Zornov (format {found}; this version reads up to {supported})"),
            FileError::Migration(e) => write!(f, "could not upgrade the project: {e}"),
            FileError::Invalid(v) => write!(f, "project is damaged: {v}"),
            FileError::Changed { .. } => write!(f, "the project was saved by someone else since it was opened"),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileError::Io(e) => Some(e),
            FileError::Json(e) => Some(e),
            FileError::Invalid(v) => Some(v),
            _ => None,
        }
    }
}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        FileError::Io(e)
    }
}

impl From<serde_json::Error> for FileError {
    fn from(e: serde_json::Error) -> Self {
        FileError::Json(e)
    }
}

/// Serialises a project document (pretty-printed for readable diffs).
pub fn to_json(file: &ProjectFile) -> String {
    serde_json::to_string_pretty(file).expect("project serialises")
}

/// Parses a project document, upgrading old versions and checking
/// invariants. Media paths are left as stored; see [`ProjectFolder::load`].
pub fn from_json(text: &str) -> Result<ProjectFile, FileError> {
    let mut doc: serde_json::Value = serde_json::from_str(text)?;
    if doc.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
        return Err(FileError::NotAProject);
    }
    let version = doc.get("version").and_then(|v| v.as_u64()).ok_or(FileError::NotAProject)?;
    if version > CURRENT_VERSION {
        return Err(FileError::TooNew { found: version, supported: CURRENT_VERSION });
    }
    migrate::migrate(&mut doc, version)?;
    let file: ProjectFile = serde_json::from_value(doc)?;
    file.project.check_invariants().map_err(FileError::Invalid)?;
    Ok(file)
}

/// What a saved project file is, to tell whether it changed: FNV-1a over
/// its bytes (stable across runs and builds).
pub fn stamp_of(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Writes `bytes` to `path` atomically: the data goes to a temporary file
/// in the same folder, is flushed to disk, and then renamed over `path`.
/// Readers see either the old file or the new one, never a torn write.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name"))?;
    // A name of its own per write: a name per process let two threads of
    // one process (the window's autosave and a save from the menu or an
    // agent) write into one temporary file and rename it from under each
    // other.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{}.tmp-{}-{n}", name.to_string_lossy(), std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        // Make the rename itself durable. Opening a directory for sync is
        // not supported everywhere, so failures here are not fatal.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A project folder on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFolder {
    root: PathBuf,
}

impl ProjectFolder {
    /// Refers to an existing or future folder at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Creates the folder and its sub-folders if missing.
    pub fn create(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let f = Self::new(root);
        for d in [f.root.clone(), f.media_dir(), f.cache_dir(), f.backups_dir()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(f)
    }

    /// The folder itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `project.json`.
    pub fn project_file(&self) -> PathBuf {
        self.root.join(PROJECT_FILE)
    }

    /// Copied media.
    pub fn media_dir(&self) -> PathBuf {
        self.root.join("media")
    }

    /// Disposable caches (thumbnails, waveforms, proxies).
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Rolling autosave backups.
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// Saves `project` atomically, keeping `extra` top-level fields (pass
    /// those from the loaded [`ProjectFile`] to preserve them).
    pub fn save_with(&self, project: &Project, extra: &Extra) -> Result<(), FileError> {
        std::fs::create_dir_all(&self.root)?;
        let file = self.document(project, extra);
        write_atomic(&self.project_file(), to_json(&file).as_bytes())?;
        Ok(())
    }

    /// Saves `project` atomically.
    pub fn save(&self, project: &Project) -> Result<(), FileError> {
        self.save_with(project, &Extra::new())
    }

    /// Saves `project` only if `project.json` is still the file `expected`
    /// names (its [`stamp_of`] when it was loaded or last saved by the
    /// caller); `None` saves whatever is there. Returns the new file's
    /// stamp. Savers in other processes and threads take turns through a
    /// lock beside the file, so a check and its write are one step: two
    /// sessions on one project cannot both save over the same version, and
    /// the second learns of the first ([`FileError::Changed`]).
    pub fn save_if(&self, project: &Project, extra: &Extra, expected: Option<u64>) -> Result<u64, FileError> {
        std::fs::create_dir_all(&self.root)?;
        let _turn = self.lock()?;
        if let Some(want) = expected
            && let Some(now) = self.stamp()
            && now != want
        {
            return Err(FileError::Changed { on_disk: now });
        }
        let bytes = to_json(&self.document(project, extra)).into_bytes();
        write_atomic(&self.project_file(), &bytes)?;
        Ok(stamp_of(&bytes))
    }

    /// The [`stamp_of`] `project.json` as it is now; `None` when there is
    /// none (not saved yet) or it cannot be read.
    pub fn stamp(&self) -> Option<u64> {
        std::fs::read(self.project_file()).ok().map(|b| stamp_of(&b))
    }

    /// An exclusive hold on this folder's saves (released when dropped):
    /// `.project.lock`, locked by the operating system, so a process that
    /// dies lets go of it.
    pub fn lock(&self) -> std::io::Result<std::fs::File> {
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.root.join(".project.lock"))?;
        f.lock()?;
        Ok(f)
    }

    /// Says this process has the project open, until the file returned is
    /// dropped (a shared hold on `.project.open`; the operating system lets
    /// go of it when the process ends, however it ends).
    pub fn hold_open(&self) -> std::io::Result<std::fs::File> {
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.root.join(".project.open"))?;
        f.lock_shared()?;
        Ok(f)
    }

    /// Whether another process has the project open too (see
    /// [`ProjectFolder::hold_open`]); `mine` is this process's hold, let go
    /// of for the moment of asking and taken again. Clean-ups that judge
    /// what is unused from this process's view alone (superseded model
    /// files) must not run while it is shared.
    pub fn open_elsewhere(&self, mine: &mut Option<std::fs::File>) -> bool {
        *mine = None;
        let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.root.join(".project.open")) else { return true };
        let alone = f.try_lock().is_ok();
        let _ = f.unlock();
        if f.lock_shared().is_ok() {
            *mine = Some(f);
        }
        !alone
    }

    /// Loads `project.json`, upgrading and checking it, and re-finds media
    /// that moved together with the folder.
    pub fn load(&self) -> Result<ProjectFile, FileError> {
        Ok(self.load_stamped()?.0)
    }

    /// [`ProjectFolder::load`], with the [`stamp_of`] the bytes it read (to
    /// hand to [`ProjectFolder::save_if`]).
    pub fn load_stamped(&self) -> Result<(ProjectFile, u64), FileError> {
        let bytes = std::fs::read(self.project_file())?;
        let stamp = stamp_of(&bytes);
        let text = String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = from_json(&text)?;
        self.resolve_media(&mut file.project);
        Ok((file, stamp))
    }

    /// The document that [`ProjectFolder::save_with`] writes: media items
    /// get their project-relative paths refreshed.
    pub fn document(&self, project: &Project, extra: &Extra) -> ProjectFile {
        let mut project = project.clone();
        for m in &mut project.media {
            m.relative_path = relative_path(&self.root, Path::new(&m.path));
        }
        ProjectFile { format: FORMAT.into(), version: CURRENT_VERSION, project, extra: extra.clone() }
    }

    /// Points media whose absolute path no longer exists at the
    /// project-relative copy, when that one exists (the folder was moved).
    pub fn resolve_media(&self, project: &mut Project) {
        for m in &mut project.media {
            if Path::new(&m.path).exists() {
                continue;
            }
            if let Some(rel) = &m.relative_path {
                let candidate = normalize(&self.root.join(rel));
                if candidate.exists() {
                    m.path = candidate.to_string_lossy().into_owned();
                }
            }
        }
    }
}

/// Lexical relative path from directory `from` to `to`, using `/`.
/// `None` when either path is relative or they share no root.
pub fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let from = normalize(&absolute(from)?);
    let to = normalize(to);
    if !to.is_absolute() {
        return None;
    }
    let a: Vec<Component> = from.components().collect();
    let b: Vec<Component> = to.components().collect();
    if a.first() != b.first() {
        return None;
    }
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), a.len() - common).collect();
    parts.extend(b[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    Some(if parts.is_empty() { ".".into() } else { parts.join("/") })
}

fn absolute(p: &Path) -> Option<PathBuf> {
    if p.is_absolute() { Some(p.to_path_buf()) } else { std::env::current_dir().ok().map(|d| d.join(p)) }
}

/// Removes `.` and folds `..` without touching the file system.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path(Path::new("/a/b/proj"), Path::new("/a/b/proj/media/x.mp4")).as_deref(), Some("media/x.mp4"));
        assert_eq!(relative_path(Path::new("/a/b/proj"), Path::new("/a/c/x.mp4")).as_deref(), Some("../../c/x.mp4"));
        assert_eq!(relative_path(Path::new("/a"), Path::new("rel.mp4")), None);
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn rejects_foreign_and_future_files() {
        assert!(matches!(from_json(r#"{"format":"other","version":1}"#), Err(FileError::NotAProject)));
        let future = format!(r#"{{"format":"{FORMAT}","version":{}, "project":{{}}}}"#, CURRENT_VERSION + 1);
        assert!(matches!(from_json(&future), Err(FileError::TooNew { .. })));
    }
}
