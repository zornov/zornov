// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Working as a team: review notes, versions, sharing and roles.
//!
//! Everything here lives in plain files beside `project.json`, so a project
//! folder carries its review state wherever it is copied:
//!
//! ```text
//! review/comments.json   notes pinned to a time (and a clip or a region)
//! versions/index.json    named snapshots; versions/<id>.json each project
//! team.json              who works on the project, their roles, locks
//! sync/state.json        where this copy syncs to and how far it got
//! ```
//!
//! * [`comments`]: threads with replies, resolve / reopen, export as a
//!   list (CSV, Markdown, EDL markers), import notes back.
//! * [`versions`]: save, list, compare, restore and branch.
//! * [`delta`]: the edits that turn one project into another (restore and
//!   merge are ordinary undoable steps) and a readable change report.
//! * [`merge`]: three-way merge of two copies that share a base.
//! * [`sync`]: two or more people on one project through a shared folder
//!   (a network drive, a synced folder, a USB stick). [`sync::Remote`] is
//!   the seam a hosted service plugs into later; nothing here sends data
//!   anywhere unless the person asks for a sync.
//! * [`team`]: editor / reviewer roles and per-person locks on lanes and
//!   clips, enforced on every edit of the document.
//! * [`package`]: a review package (proxy render, notes, version) to send
//!   to someone, and reading their notes back.

pub mod comments;
pub mod delta;
pub mod merge;
pub mod package;
pub mod sync;
pub mod team;
pub mod versions;

use std::path::{Path, PathBuf};
use studio_model as m;

/// Milliseconds since 1970.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Who is working here: `STUDIO_USER`, else the name saved with
/// [`set_identity`], else the computer account's name.
pub fn identity() -> String {
    if let Ok(u) = std::env::var("STUDIO_USER")
        && !u.trim().is_empty()
    {
        return u.trim().to_owned();
    }
    if let Ok(u) = std::fs::read_to_string(identity_file())
        && !u.trim().is_empty()
    {
        return u.trim().to_owned();
    }
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "me".into())
}

fn identity_file() -> PathBuf {
    studio_paths::studio().join("identity.txt")
}

/// Saves the name notes and versions are signed with on this computer.
pub fn set_identity(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a name is needed".into());
    }
    let f = identity_file();
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(f, name).map_err(|e| format!("could not save the name: {e}"))
}

/// FNV-1a over bytes.
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A short fingerprint of what the project contains (the id counter
/// aside): two copies with the same fingerprint show the same edit.
pub fn fingerprint(p: &m::Project) -> String {
    let mut c = p.clone();
    c.next_id = 0;
    format!("{:016x}", fnv(serde_json::to_string(&c).unwrap_or_default().as_bytes()))
}

/// Reads a JSON file, `None` when it is missing.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("{} is damaged: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// Writes JSON atomically, creating the folder.
pub(crate) fn write_json<T: serde::Serialize>(path: &Path, v: &T) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("could not create {}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    m::file::write_atomic(path, text.as_bytes()).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// `h:mm:ss.f` for lists people read.
pub fn clock(t: m::Micros) -> String {
    let ms = (t.max(0) / 1000) as u64;
    let (h, mi, s, f) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000 / 100);
    if h > 0 { format!("{h}:{mi:02}:{s:02}.{f}") } else { format!("{mi}:{s:02}.{f}") }
}

/// Reads a time a person or another tool wrote: seconds (`83.5`),
/// `m:ss(.f)`, `h:mm:ss(.f)` or a timecode `hh:mm:ss:ff` / `hh:mm:ss;ff`
/// at `fps`.
pub fn parse_time(s: &str, fps: f64) -> Option<m::Micros> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<f64>() {
        return (v.is_finite() && v >= 0.0).then(|| (v * 1e6).round() as m::Micros);
    }
    let parts: Vec<&str> = s.split([':', ';']).collect();
    let num = |p: &str| p.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    let secs = match parts.len() {
        2 => num(parts[0])? * 60.0 + num(parts[1])?,
        3 => num(parts[0])? * 3600.0 + num(parts[1])? * 60.0 + num(parts[2])?,
        4 => num(parts[0])? * 3600.0 + num(parts[1])? * 60.0 + num(parts[2])? + num(parts[3])? / fps.max(1.0),
        _ => return None,
    };
    Some((secs * 1e6).round() as m::Micros)
}

#[cfg(test)]
pub(crate) mod testing {
    use studio_model as m;

    /// A small edit: a few spine clips, an overlay lane with a title.
    pub fn project() -> m::Project {
        let mut s = crate::agent::Session::default();
        s.run(
            "edit",
            &serde_json::json!({"ops": [
                {"op": "card", "colour": "#ff0000", "at": 0, "duration": 3, "lane": "main"},
                {"op": "card", "colour": "#00ff00", "at": 3, "duration": 3, "lane": "main"},
                {"op": "card", "colour": "#0000ff", "at": 6, "duration": 3, "lane": "main"},
            ]}),
        )
        .unwrap();
        s.doc.model.clone()
    }

    /// A scratch folder under the system temp dir, removed on drop.
    pub struct Scratch(pub std::path::PathBuf);
    impl Scratch {
        pub fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("studio-collab-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_people_write() {
        assert_eq!(parse_time("83.5", 24.0), Some(83_500_000));
        assert_eq!(parse_time("1:23.5", 24.0), Some(83_500_000));
        assert_eq!(parse_time("0:01:23", 24.0), Some(83_000_000));
        assert_eq!(parse_time("00:00:01:12", 24.0), Some(1_500_000));
        assert_eq!(parse_time("01:00:00;15", 30.0), Some(3_600_500_000));
        assert_eq!(parse_time("soon", 24.0), None);
        assert_eq!(clock(83_500_000), "1:23.5");
    }

    #[test]
    fn fingerprint_ignores_the_id_counter() {
        let mut a = m::Project::default();
        let b = a.clone();
        a.alloc_id();
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }
}
