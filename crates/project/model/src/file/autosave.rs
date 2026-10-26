// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Autosave into rolling backups.
//!
//! The app tells [`Autosave`] when the project changes and polls it from
//! its frame loop; once the project has been idle for [`IDLE`] after a
//! change, `poll` writes a backup to `backups/` and keeps only the newest
//! [`KEEP`]. The clock is passed in, so the timing is testable and the
//! caller may run the actual write on a worker thread.

use super::{FileError, ProjectFolder, to_json, write_atomic};
use crate::project::Project;
use crate::value::Extra;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Idle time after a change before a backup is written.
pub const IDLE: Duration = Duration::from_secs(30);

/// Number of backups kept.
pub const KEEP: usize = 20;

const PREFIX: &str = "autosave-";
const SUFFIX: &str = ".json";

/// Tracks changes and decides when to write a backup.
#[derive(Clone, Debug)]
pub struct Autosave {
    /// Idle time required before saving.
    pub idle: Duration,
    /// Backups kept.
    pub keep: usize,
    last_change: Option<Instant>,
}

impl Default for Autosave {
    fn default() -> Self {
        Self { idle: IDLE, keep: KEEP, last_change: None }
    }
}

impl Autosave {
    /// Autosave with the default 30 s idle time and 20 backups.
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes that the project changed at `now`.
    pub fn changed(&mut self, now: Instant) {
        self.last_change = Some(now);
    }

    /// Whether unsaved changes are waiting for a backup.
    pub fn pending(&self) -> bool {
        self.last_change.is_some()
    }

    /// Whether a backup is due at `now`.
    pub fn due(&self, now: Instant) -> bool {
        self.last_change.is_some_and(|t| now.saturating_duration_since(t) >= self.idle)
    }

    /// The due backup is being written elsewhere (on another thread).
    pub fn mark_done(&mut self) {
        self.last_change = None;
    }

    /// Writes a backup if one is due; returns its path when written.
    pub fn poll(&mut self, now: Instant, folder: &ProjectFolder, project: &Project) -> Result<Option<PathBuf>, FileError> {
        if !self.due(now) {
            return Ok(None);
        }
        self.backup_now(folder, project).map(Some)
    }

    /// Writes a backup immediately and prunes old ones.
    pub fn backup_now(&mut self, folder: &ProjectFolder, project: &Project) -> Result<PathBuf, FileError> {
        let path = write_backup(folder, project, SystemTime::now(), self.keep)?;
        self.last_change = None;
        Ok(path)
    }
}

/// Writes `backups/autosave-<milliseconds since epoch>.json` and deletes
/// all but the newest `keep` backups. Returns the new backup's path.
pub fn write_backup(folder: &ProjectFolder, project: &Project, at: SystemTime, keep: usize) -> Result<PathBuf, FileError> {
    let dir = folder.backups_dir();
    std::fs::create_dir_all(&dir)?;
    let mut ms = at.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    // Two backups in the same millisecond get consecutive names.
    let path = loop {
        let p = dir.join(backup_name(ms));
        if !p.exists() {
            break p;
        }
        ms += 1;
    };
    let doc = folder.document(project, &Extra::new());
    write_atomic(&path, to_json(&doc).as_bytes())?;
    for old in list_backups(folder)?.into_iter().skip(keep) {
        std::fs::remove_file(old)?;
    }
    Ok(path)
}

fn backup_name(ms: u64) -> String {
    // Zero-padded so names sort in time order.
    format!("{PREFIX}{ms:016}{SUFFIX}")
}

fn backup_time(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?.parse().ok()
}

/// Backups in the folder, newest first.
pub fn list_backups(folder: &ProjectFolder) -> Result<Vec<PathBuf>, FileError> {
    let dir = folder.backups_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).map(|e| e.path()).filter_map(|p| backup_time(&p).map(|t| (t, p))).collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(found.into_iter().map(|(_, p)| p).collect())
}

/// Loads the newest backup that reads cleanly, skipping damaged ones.
pub fn load_latest_backup(folder: &ProjectFolder) -> Result<Option<Project>, FileError> {
    for path in list_backups(folder)? {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if let Ok(mut file) = super::from_json(&text) {
            folder.resolve_media(&mut file.project);
            return Ok(Some(file.project));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing() {
        let mut a = Autosave::new();
        let t0 = Instant::now();
        assert!(!a.due(t0));
        a.changed(t0);
        assert!(!a.due(t0 + Duration::from_secs(29)));
        assert!(a.due(t0 + Duration::from_secs(30)));
        // A later change restarts the idle wait.
        a.changed(t0 + Duration::from_secs(20));
        assert!(!a.due(t0 + Duration::from_secs(30)));
        assert!(a.due(t0 + Duration::from_secs(50)));
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(backup_time(Path::new(&backup_name(1234))), Some(1234));
        assert_eq!(backup_time(Path::new("other.json")), None);
    }
}
