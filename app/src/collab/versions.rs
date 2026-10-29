// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Named versions of a project: "Director's cut v3", "Locked picture".
//!
//! A version is the whole `project.json` as it was, kept in
//! `versions/<id>.json` with a line in `versions/index.json`. Versions are
//! compared with each other or with the edit as it is now
//! ([`super::delta::report`]), restored (one undoable step back to that
//! state) or branched: the version's cut joins the project as another
//! sequence, so the alternate cut and the main one live side by side.

use super::{fingerprint, now_ms, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use studio_model as m;

fn dir(root: &Path) -> PathBuf {
    m::file::ProjectFolder::new(root).review_versions_dir()
}

/// One saved version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    /// `v1`, `v2`, … in the order they were saved.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub author: String,
    pub created: u64,
    /// [`fingerprint`] of the saved project.
    pub fingerprint: String,
    /// The version this one was made after (the last one saved or
    /// restored), so the list reads as a family tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The line of work it belongs to (`main` unless branched).
    #[serde(default = "main")]
    pub branch: String,
    /// The active sequence's name and length when saved.
    #[serde(default)]
    pub sequence: String,
    #[serde(default)]
    pub duration: m::Micros,
    #[serde(default)]
    pub clips: usize,
}

fn main() -> String {
    "main".into()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub versions: Vec<Version>,
    /// The version the edit last matched (saved or restored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// The branch new versions belong to.
    #[serde(default = "main")]
    pub branch: String,
}

impl Index {
    pub fn load(root: &Path) -> Result<Self, String> {
        let mut i = read_json::<Index>(&dir(root).join("index.json"))?.unwrap_or_default();
        if i.branch.is_empty() {
            i.branch = main();
        }
        Ok(i)
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        write_json(&dir(root).join("index.json"), self)
    }

    /// A version by id (`v3`), by name (loosely), or `latest`.
    pub fn find(&self, key: &str) -> Result<&Version, String> {
        let k = key.trim().to_lowercase();
        if k == "latest" || k == "last" {
            return self.versions.last().ok_or_else(|| "no versions saved yet".into());
        }
        self.versions
            .iter()
            .find(|v| v.id.to_lowercase() == k)
            .or_else(|| self.versions.iter().rev().find(|v| v.name.to_lowercase() == k))
            .or_else(|| self.versions.iter().rev().find(|v| v.name.to_lowercase().contains(&k)))
            .ok_or_else(|| format!("no version \"{key}\"; versions: {}", self.versions.iter().map(|v| format!("{} \"{}\"", v.id, v.name)).collect::<Vec<_>>().join(", ")))
    }
}

/// Saves the project as a named version.
pub fn save(root: &Path, p: &m::Project, name: &str, note: &str, author: &str) -> Result<Version, String> {
    let mut index = Index::load(root)?;
    let n = index.versions.iter().filter_map(|v| v.id.strip_prefix('v').and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0) + 1;
    let id = format!("v{n}");
    let name = if name.trim().is_empty() { format!("Version {n}") } else { name.trim().to_owned() };
    let folder = m::file::ProjectFolder::new(root);
    let doc = folder.document(p, &m::Extra::new());
    std::fs::create_dir_all(dir(root)).map_err(|e| format!("could not create the versions folder: {e}"))?;
    m::file::write_atomic(&dir(root).join(format!("{id}.json")), m::file::to_json(&doc).as_bytes()).map_err(|e| format!("could not save the version: {e}"))?;
    let seq = p.active();
    let v = Version {
        id: id.clone(),
        name,
        note: note.trim().to_owned(),
        author: author.to_owned(),
        created: now_ms(),
        fingerprint: fingerprint(p),
        parent: index.current.clone(),
        branch: index.branch.clone(),
        sequence: seq.name.clone(),
        duration: seq.clips.iter().map(|c| c.start + c.duration).max().unwrap_or(0),
        clips: seq.clips.len(),
    };
    index.versions.push(v.clone());
    index.current = Some(id);
    index.save(root)?;
    Ok(v)
}

/// The project as it was saved in a version.
pub fn load(root: &Path, key: &str) -> Result<(Version, m::Project), String> {
    let index = Index::load(root)?;
    let v = index.find(key)?.clone();
    let text = std::fs::read_to_string(dir(root).join(format!("{}.json", v.id))).map_err(|e| format!("version {} is missing its file: {e}", v.id))?;
    let file = m::file::from_json(&text).map_err(|e| format!("version {} is damaged: {e}", v.id))?;
    let mut project = file.project;
    m::file::ProjectFolder::new(root).resolve_media(&mut project);
    Ok((v, project))
}

/// Marks `id` as the version the edit now matches (after a restore).
pub fn set_current(root: &Path, id: &str) -> Result<(), String> {
    let mut index = Index::load(root)?;
    index.current = Some(id.to_owned());
    index.save(root)
}

/// Starts a new line of work: versions saved from now on belong to it.
pub fn set_branch(root: &Path, branch: &str) -> Result<(), String> {
    let mut index = Index::load(root)?;
    index.branch = branch.trim().to_owned();
    index.save(root)
}

/// The edits that add `from`'s cut of sequence `seq` (default: its active
/// one) to `into` as a new sequence called `name`, with fresh ids, plus
/// any media and nested sequences it needs that `into` lacks.
pub fn branch_edits(into: &mut m::Project, from: &m::Project, seq: Option<m::SequenceId>, name: &str) -> Result<(Vec<m::Edit>, m::SequenceId), String> {
    let src = from.sequence(seq.unwrap_or(from.active_sequence)).ok_or("that version has no such sequence")?.clone();
    let mut edits = Vec::new();
    for item in &from.media {
        if into.media_item(item.id).is_none() {
            edits.push(m::Edit::InsertMedia { item: Box::new(item.clone()), index: None });
        }
    }
    // Nested sequences the cut uses that this project no longer has come
    // along (with fresh ids too).
    let mut seq_map: HashMap<u64, u64> = HashMap::new();
    let mut nested = Vec::new();
    let mut todo: Vec<m::SequenceId> = nested_refs(&src);
    while let Some(id) = todo.pop() {
        if into.sequence(id).is_some() || seq_map.contains_key(&id.0) {
            continue;
        }
        let Some(s) = from.sequence(id) else { continue };
        seq_map.insert(id.0, into.alloc_id());
        todo.extend(nested_refs(s));
        nested.push(s.clone());
    }
    let new_id = m::SequenceId(into.alloc_id());
    let mut copies = Vec::new();
    for s in nested {
        let id = m::SequenceId(seq_map[&s.id.0]);
        copies.push(renumber(into, s, id, &seq_map));
    }
    let mut main = renumber(into, src, new_id, &seq_map);
    main.name = name.to_owned();
    for s in copies {
        edits.push(m::Edit::InsertSequence { sequence: Box::new(s), index: None });
    }
    edits.push(m::Edit::InsertSequence { sequence: Box::new(main), index: None });
    Ok((edits, new_id))
}

fn nested_refs(s: &m::Sequence) -> Vec<m::SequenceId> {
    s.clips
        .iter()
        .filter_map(|c| match &c.source {
            m::ClipSource::Sequence(r) => Some(r.sequence),
            _ => None,
        })
        .collect()
}

/// A copy of `s` under id `id` with every lane, clip, transition, marker,
/// link and group renumbered from `p`'s counter (references follow).
fn renumber(p: &mut m::Project, mut s: m::Sequence, id: m::SequenceId, seqs: &HashMap<u64, u64>) -> m::Sequence {
    s.id = id;
    let mut lanes = HashMap::new();
    for l in &mut s.lanes {
        let n = p.alloc_id();
        lanes.insert(l.id.0, n);
        l.id = m::LaneId(n);
    }
    let clips: HashMap<u64, u64> = s.clips.iter().map(|c| c.id.0).collect::<Vec<_>>().into_iter().map(|c| (c, p.alloc_id())).collect();
    let (mut links, mut groups) = (HashMap::new(), HashMap::new());
    for c in &mut s.clips {
        c.id = m::ClipId(clips[&c.id.0]);
        c.lane = m::LaneId(*lanes.get(&c.lane.0).unwrap_or(&c.lane.0));
        if let Some(l) = c.link {
            c.link = Some(m::LinkId(*links.entry(l.0).or_insert_with(|| p.alloc_id())));
        }
        if let Some(g) = c.group {
            c.group = Some(m::GroupId(*groups.entry(g.0).or_insert_with(|| p.alloc_id())));
        }
        if let Some(par) = &mut c.parent {
            par.clip = m::ClipId(*clips.get(&par.clip.0).unwrap_or(&par.clip.0));
        }
        for d in c.drivers.values_mut() {
            for v in &mut d.vars {
                v.clip = m::ClipId(*clips.get(&v.clip.0).unwrap_or(&v.clip.0));
            }
        }
        if let m::ClipSource::Sequence(r) = &mut c.source
            && let Some(n) = seqs.get(&r.sequence.0)
        {
            r.sequence = m::SequenceId(*n);
        }
    }
    for t in &mut s.transitions {
        t.id = m::TransitionId(p.alloc_id());
        t.left = m::ClipId(*clips.get(&t.left.0).unwrap_or(&t.left.0));
        t.right = m::ClipId(*clips.get(&t.right.0).unwrap_or(&t.right.0));
    }
    for mk in &mut s.markers {
        mk.id = m::MarkerId(p.alloc_id());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::{delta, testing};

    #[test]
    fn save_list_compare_restore() {
        let dir = testing::Scratch::new("versions");
        let a = testing::project();
        let v1 = save(&dir.0, &a, "Assembly", "first pass", "Ash").unwrap();
        assert_eq!(v1.id, "v1");
        let mut b = a.clone();
        let seq = b.active_sequence;
        let first = b.active().clips[0].id;
        b.apply(m::Edit::RippleDelete { seq, clips: vec![first] }).unwrap();
        let v2 = save(&dir.0, &b, "Tighter", "", "Ash").unwrap();
        assert_eq!(v2.parent.as_deref(), Some("v1"));
        let index = Index::load(&dir.0).unwrap();
        assert_eq!(index.find("assembly").unwrap().id, "v1");
        assert_eq!(index.find("latest").unwrap().id, "v2");
        assert!(index.find("nope").unwrap_err().contains("v2 \"Tighter\""));

        let (_, old) = load(&dir.0, "v1").unwrap();
        assert!(old.content_eq(&a));
        let r = delta::report(&old, &b);
        assert_eq!(r.counts.get("removed"), Some(&1));
        // Restore: one batch of edits back to v1.
        let mut now = b.clone();
        now.apply(m::Edit::Batch { edits: delta::edits_between(&b, &old) }).unwrap();
        assert!(now.content_eq(&a));
    }

    #[test]
    fn a_branch_is_a_second_cut_beside_the_first() {
        let a = testing::project();
        let mut now = a.clone();
        let seq = now.active_sequence;
        let first = now.active().clips[0].id;
        now.apply(m::Edit::RippleDelete { seq, clips: vec![first] }).unwrap();
        let mut counter = now.clone();
        let (edits, id) = branch_edits(&mut counter, &a, None, "Alt cut").unwrap();
        now.next_id = counter.next_id;
        now.apply(m::Edit::Batch { edits }).unwrap();
        assert_eq!(now.sequences.len(), 2);
        let alt = now.sequence(id).unwrap();
        assert_eq!(alt.name, "Alt cut");
        assert_eq!(alt.clips.len(), 3, "the version's cut, all three clips");
        assert_eq!(now.active().clips.len(), 2, "the main cut is untouched");
    }
}
