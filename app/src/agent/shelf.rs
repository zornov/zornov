// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The session's side of library awareness: what the project already plays
//! from the content library, the library overview, and sound and music
//! search over the library's index (see [`super::inventory`]).

use super::inventory::{self, Ask, Available, Facts, Packs};
use super::{related, sounds, Session};
use crate::content;
use crate::services::{self, library::Panel};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

/// The packs on this Mac and those the credits list but it lacks. The
/// content folder comes from Studio's content settings (`content::find_dir`).
pub fn packs() -> Packs {
    let mut out = Packs::default();
    let dir = content::find_dir();
    if let Some(dir) = &dir {
        let mut folders: Vec<(String, PathBuf)> = Vec::new();
        if dir.join("core/index.json").exists() {
            folders.push(("core".to_owned(), dir.join("core")));
        }
        if let Ok(rd) = std::fs::read_dir(dir.join("packs")) {
            let mut v: Vec<(String, PathBuf)> = rd.filter_map(|e| e.ok()).filter(|e| e.path().join("index.json").exists()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
            v.sort();
            folders.extend(v);
        }
        for (id, path) in folders {
            // A pack with files has folders beside its index.
            let has_files = std::fs::read_dir(&path).is_ok_and(|rd| rd.filter_map(|e| e.ok()).any(|e| e.path().is_dir()));
            if has_files {
                out.installed.push(id);
            } else {
                out.not_downloaded.push(id);
            }
        }
    }
    let known: Vec<&String> = out.installed.iter().chain(out.not_downloaded.iter()).collect();
    out.available = services::licences::all().packs.iter().filter(|p| !p.id.is_empty() && !known.contains(&&p.id)).map(|p| Available { id: p.id.clone(), name: p.name.clone(), summary: p.summary.clone() }).collect();
    out
}

/// The library, waiting for it if it is still loading (a headless run has no
/// window to preload it).
pub fn content() -> Option<std::sync::Arc<content::Content>> {
    content::get_blocking()
}

impl Session {
    /// How many clips play each library file now.
    pub(super) fn library_uses(&self) -> HashMap<PathBuf, usize> {
        let p = &self.doc.project;
        let mut used: HashMap<PathBuf, usize> = HashMap::new();
        for c in &p.clips {
            if let Some(path) = c.media.and_then(|m| p.media(m)).and_then(|m| m.path.clone()) {
                *used.entry(path).or_insert(0) += 1;
            }
        }
        used
    }

    /// What is measured and used about the library's entries.
    pub(super) fn library_facts(&self) -> Facts {
        Facts { heard: sounds::heard().clone(), used: self.library_uses() }
    }

    /// `catalogue` with nothing asked: the whole library in one answer.
    pub(super) fn library_overview(&self) -> Value {
        let packs = packs();
        let mut v = match content() {
            Some(c) => inventory::overview(&c.entries, &self.library_facts(), &packs, content::find_dir().is_some()),
            None => inventory::overview(&[], &Facts::default(), &packs, content::find_dir().is_some()),
        };
        let mut panels = serde_json::Map::new();
        for (name, p) in [("effects", Panel::Effects), ("looks", Panel::Looks), ("motions", Panel::Motions), ("text_styles", Panel::Text), ("transitions", Panel::Cuts)] {
            panels.insert(name.to_owned(), json!(services::library::count(&self.doc, p)));
        }
        v["panels"] = Value::Object(panels);
        // The person's own 3D models, kept from earlier work: look here
        // before modelling the same thing again.
        let kept = crate::my_models::list_in(&crate::my_models::root());
        if !kept.is_empty() {
            let names: Vec<String> = kept.iter().rev().take(30).map(|m| format!("{} ({})", m.name, m.category.to_lowercase())).collect();
            v["kept_models"] = json!({"count": kept.len(), "newest": names, "find": "catalogue {panel: assets, query: words} then add {item}; check before modelling the same thing again"});
        }
        v
    }

    /// Library items of any kind that match `query`, for `search` and
    /// `assets` (pictures, sounds, music, motions, fonts), varied, with
    /// what the project already plays marked.
    pub(super) fn library_hits(&self, query: &str, limit: usize) -> Vec<Value> {
        let Some(c) = content() else { return Vec::new() };
        if query.trim().is_empty() {
            return Vec::new();
        }
        let facts = self.library_facts();
        let (_, hits) = inventory::search_where(&c.entries, &facts, &Ask::from_words(query), &|w| related(w), &|w| sounds::wanted(w), limit, &|_| true);
        hits.iter().map(|h| inventory::describe(&c.entries[h.index], &facts)).collect()
    }

    /// Music and sound effects for `ask`: ranked by name, tags, mood and
    /// what each sounds like, varied, with what the project already plays marked.
    pub(super) fn sound_search(&self, ask: &Ask, limit: usize) -> Value {
        let Some(c) = content() else { return json!({"panel": "audio", "total": 0, "items": []}) };
        let facts = self.library_facts();
        let (total, hits) = inventory::search_audio(&c.entries, &facts, ask, &|w| related(w), &|w| sounds::wanted(w), limit.max(1));
        let want_swell = ask.words.iter().any(|w| sounds::wanted(w) == Some("swell"));
        let items: Vec<Value> = hits
            .iter()
            .map(|h| {
                let e = &c.entries[h.index];
                let mut o = inventory::describe(e, &facts);
                if e.kind == studio_content::item::Kind::Sound {
                    o["sounds_like"] = o["kind"].clone();
                }
                o
            })
            .collect();
        let mut v = json!({"panel": "audio", "total": total, "items": items});
        let used = inventory::used_names(&c.entries, &facts);
        if !used.is_empty() {
            v["already_in_project"] = json!(used);
        }
        if total == 0 {
            v["nothing_for"] = json!("no sound or track matches those words and filters; catalogue {panel: audio} with no query lists the moods, kinds and categories that exist");
        }
        v["note"] = json!(if want_swell {
            "the library has almost no true risers: the sfx op (kind riser, ends_at your drop) makes one that peaks exactly there"
        } else {
            "kind is measured for sounds: hit (short, hard), boom (hard, long tail), swell (builds), swoosh (soft start). Take different ones for different moments; catalogue {kind, mood, max_seconds, fresh: true} narrows. The sfx op makes a riser, whoosh, boom or hit to any length"
        });
        v
    }
}
