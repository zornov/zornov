// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relinking missing media: find candidates by name, size and length in the
//! chosen folders, show them, then point the project at them in one undo step.
//! The assistant's `assets` action "relink" and the Library sheet both use this.

use crate::assets::{Asset, Store};
use crate::doc::Doc;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_model as m;

/// One missing file and where it turned up.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub asset: Asset,
    pub to: PathBuf,
    /// "exact" (same content) or "same name and size".
    pub how: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct RelinkReport {
    /// How many files were missing.
    pub wanted: usize,
    pub found: Vec<Candidate>,
    pub still_missing: Vec<String>,
    pub searched: Vec<PathBuf>,
    /// The project was changed.
    pub applied: bool,
}

impl RelinkReport {
    pub fn json(&self) -> Value {
        let done: Vec<Value> = self.found.iter().map(|c| json!({"asset": c.asset.name, "now": c.to.display().to_string(), "match": c.how})).collect();
        json!({
            "relinked": if self.applied { done.len() } else { 0 },
            "found": done,
            "still_missing": self.still_missing,
            "searched": self.searched.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
            "changed_project": self.applied && !self.found.is_empty(),
            "note": if self.still_missing.is_empty() { Value::Null } else { json!("name other folders or drives in search to look further") },
        })
    }
}

/// Default places to look for moved files: next to the project, the usual
/// media folders and connected drives.
pub fn usual_places(project: Option<&Path>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(p) = project {
        v.push(p.to_path_buf());
        if let Some(parent) = p.parent() {
            v.push(parent.to_path_buf());
        }
    }
    let home = studio_paths::home();
    v.extend([studio_paths::videos(), home.join("Desktop"), home.join("Downloads"), home.join("Documents"), home.join("Pictures"), home.join("Music")]);
    if let Ok(rd) = std::fs::read_dir("/Volumes") {
        v.extend(rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()));
    }
    v.retain(|p| p.is_dir());
    v.sort();
    v.dedup();
    v
}

/// Looks for the project's missing files (`all`: every missing file in the
/// library) in `search` (the usual places when empty). Changes nothing.
/// `check_length` drops a candidate whose length differs from the original's.
pub fn plan(store: &Store, root: Option<&Path>, all: bool, search: &[PathBuf], check_length: bool) -> Result<RelinkReport, String> {
    let wanted = store.missing(if all { None } else { root })?;
    if wanted.is_empty() {
        return Ok(RelinkReport::default());
    }
    let dirs: Vec<PathBuf> = if search.is_empty() { usual_places(root) } else { search.to_vec() };
    let mut found = store.find_moved(&wanted, &dirs);
    if check_length {
        found.retain(|(a, to, _)| same_length(a, to));
    }
    let found: Vec<Candidate> = found.into_iter().map(|(asset, to, how)| Candidate { asset, to, how }).collect();
    let still_missing = wanted.iter().filter(|w| !found.iter().any(|c| c.asset.id == w.id)).map(|w| w.name.clone()).collect();
    Ok(RelinkReport { wanted: wanted.len(), found, still_missing, searched: dirs, applied: false })
}

/// Both lengths known and more than a frame or 1 % apart: not the same recording.
fn same_length(a: &Asset, to: &Path) -> bool {
    if a.dur <= 0.0 {
        return true;
    }
    match crate::media_io::probe(to).ok().and_then(|p| p.duration) {
        Some(d) => ((d as f64 / 1e6) - a.dur).abs() <= (a.dur * 0.01).max(0.1),
        None => true,
    }
}

/// Points the project (one undo step) and the library at what `report` found.
pub fn apply(doc: &mut Doc, store: &Store, report: &mut RelinkReport) -> Result<usize, String> {
    let mut edits = Vec::new();
    for c in &report.found {
        let Some(old) = doc.model.media.iter().find(|m| m.path == c.asset.path).cloned() else { continue };
        let mut item = old.clone();
        item.path = c.to.display().to_string();
        if let Ok(pr) = crate::media_io::probe(&c.to) {
            item.info = m::MediaInfo { kind: pr.kind, container: pr.container, duration: pr.duration.or(old.info.duration), video: pr.video, audio: pr.audio, extra: old.info.extra.clone() };
        }
        edits.push(m::Edit::SetMedia { item: Box::new(item) });
    }
    if !edits.is_empty() && !doc.apply("Relink media", m::Edit::Batch { edits }) {
        let first = report.found.first().map_or("media", |c| c.asset.name.as_str()).to_owned();
        return Err(doc.error.take().unwrap_or_else(|| format!("{first} could not be relinked")));
    }
    for c in &report.found {
        store.moved(c.asset.id, &c.to)?;
    }
    report.applied = true;
    Ok(report.found.len())
}

/// Plan and (when `apply_now`) apply in one go: the assistant's path.
pub fn relink_missing(doc: &mut Doc, store: &Store, root: Option<&Path>, all: bool, search: &[PathBuf], apply_now: bool, check_length: bool) -> Result<RelinkReport, String> {
    let mut report = plan(store, root, all, search, check_length)?;
    if apply_now && !report.found.is_empty() {
        apply(doc, store, &mut report)?;
    }
    Ok(report)
}
