// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The media library and project safety as tools for the AI: `assets`
//! (the asset database, search, bins, rights) and `project_care` (history,
//! backups, collect, health, cleanup). The same functions serve the window.

use super::{Content, Session};
use crate::assets::{self, Asset, Query, Store};
use crate::{rights, safety, services};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_model as m;

pub fn tools() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    let int = |d: &str| json!({"type": "integer", "description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let any = |d: &str| json!({"description": d});
    let list = |d: &str| json!({"type": "array", "items": {"type": "string"}, "description": d});
    let t = |name: &str, desc: &str, props: Value| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": []}});
    vec![
        t(
            "assets",
            "The media library across every project: files, bins, ratings, tags, rights, and search by what is in them (also Studio's own sounds, music and pictures). Fields: action (overview, add, find, similar, info, rate, favourite, tag, note, bins, bin_create, bin_add, duplicates, missing, relink, index, people, licence, rights_check, expiring, forget), query, asset(s), media, paths, kind, tag, min_rating, limit. More: guide {topic: \"tool:assets\"}.",
            merge(vec![json!({"action": s("what to do"), "query": s("words to find"), "asset": any("an asset: id, name or path"), "assets": any("several assets (ids, names or paths)"), "media": any("the project's media ids"), "paths": any("files or folders to add"), "kind": any("find: image, video, audio; tag/untag: tag, keyword, person, place"), "min_rating": int("at least this many stars"), "favourite": b("only favourites / set favourite"), "on": b("favourite on or off"), "stars": int("0-5")}), json!({"tag": any("a tag or tags"), "keyword": any("a keyword or keywords"), "person": any("a person (find) or a face group number (name_person)"), "place": any("a place or places"), "bin": s("only this bin"), "project": s("only what this project folder uses")}), json!({"camera": s("camera contains"), "lens": s("lens contains"), "location": s("place contains"), "source": s("imported, generated, downloaded, bundled"), "licence": s("a licence id, or the licence to record"), "min_seconds": json!({"type": "number"}), "max_seconds": json!({"type": "number"}), "missing": b("only missing files"), "sort": s("recent, name, rating, length"), "limit": int("how many (default 30)"), "values": list("labels to add or remove"), "text": s("a note"), "name": s("a bin path, or a person's name"), "rule": json!({"type": "object", "description": "a find, saved as a smart bin"}), "near": b("duplicates: include look-alikes"), "search": list("relink: folders to search"), "apply": b("relink: change the project (default true)"), "speech": b("index: also write out what is said (slow)"), "all": b("index: every asset not yet looked into, not only this project's")}), json!({"author": s("rights: creator"), "credit": s("rights: the credit line to show"), "url": s("rights: where it came from"), "expires": s("rights: YYYY-MM-DD"), "cost": s("rights: what it cost"), "terms": s("rights: territory, term, media or other limits"), "commercial": b("rights_check: will the film be used commercially (default true)"), "days": int("expiring: within this many days")})]),
        ),
        t(
            "project_care",
            "Keeps the project safe. Fields: action (health, history, snapshot, restore, backup, backups, restore_backup, collect, archive, verify, relocate, clean, recover), which, label, to, from, into, folder, keep, media, apply. `clean` only says what it would remove unless apply is true.",
            json!({"action": s("what to do"), "which": any("restore: a number, \"latest\", or part of a name"), "label": s("snapshot: a name"), "to": s("backup, backups, collect, archive: the folder"), "from": s("restore_backup: the backup folder"), "project": s("restore_backup: the project folder it was a backup of (default: this one)"), "into": s("restore_backup: a new folder"), "snapshot": s("restore_backup: which snapshot (default newest)"), "folder": s("verify, relocate: the archive folder"), "keep": int("backup: snapshots to keep"), "media": b("backup: include media (default true)"), "only_used": b("collect: leave out media no clip uses"), "with_cache": b("collect: copy caches too"), "apply": b("clean: really remove (default: only say what would go)"), "keep_autosaves": int("clean: newest autosaves to keep (default 10)"), "shared_cache_days": int("clean: shared cache files unused this many days (default 30; 0 skips)")}),
        ),
    ]
}

/// One schema object from several parts (a long `json!` literal overruns the macro recursion limit).
fn merge(parts: Vec<Value>) -> Value {
    let mut out = serde_json::Map::new();
    for p in parts {
        if let Value::Object(o) = p {
            out.extend(o);
        }
    }
    Value::Object(out)
}

fn ids_of(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Null => Vec::new(),
        one => vec![one.clone()],
    }
}

fn strs(v: &Value) -> Vec<String> {
    ids_of(v).iter().filter_map(|x| x.as_str().map(str::to_owned).or_else(|| x.as_i64().map(|n| n.to_string()))).collect()
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Default places to look for moved files: next to the project, the usual
/// media folders and connected drives.
fn usual_places(project: Option<&Path>) -> Vec<PathBuf> {
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

/// The rights check an export makes: `Ok(Null)` when all is well and there
/// is nothing to say, `Ok(report)` with warnings or credits, and an error
/// that names what cannot be used when something is blocked.
pub fn rights_gate(doc: &crate::doc::Doc, home: Option<&Path>, a: &Value) -> Result<Value, String> {
    let Some(root) = home.map(Path::to_path_buf).or_else(|| doc.folder.as_ref().map(|f| f.root().to_path_buf())) else { return Ok(Value::Null) };
    let store = match assets::global() {
        Ok(s) => s,
        Err(_) => return Ok(Value::Null),
    };
    let opts = rights::Options { commercial: a.get("commercial").and_then(Value::as_bool).unwrap_or(true), ..Default::default() };
    let rep = rights::check(store, &root, &doc.model, &opts)?;
    if rep.blocked() > 0 && !a.get("ignore_rights").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!(
            "the export was not made: {} asset{} cannot be used.\n{}\nFix them (assets action \"licence\", \"relink\", or replace the asset), say commercial: false if this is not for commercial use, or pass ignore_rights: true to export anyway.",
            rep.blocked(),
            if rep.blocked() == 1 { "" } else { "s" },
            rep.findings.iter().filter(|f| f.level == "blocked").map(|f| format!("- {}: {} ({})", f.asset, f.message, f.fix)).collect::<Vec<_>>().join("\n")
        ));
    }
    if rep.findings.is_empty() && rep.credits.is_empty() {
        return Ok(Value::Null);
    }
    Ok(rep.json())
}

impl Session {
    fn root(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()))
    }

    fn need_root(&self) -> Result<PathBuf, String> {
        self.root().ok_or_else(|| "this project has no folder yet; save it first (save)".to_owned())
    }

    /// Registers this project's media in the library (cheap when nothing changed).
    fn sync_library(&self, store: &Store) -> Option<assets::Synced> {
        let root = self.root()?;
        store.sync_model(&root, &self.doc.model).ok()
    }

    /// Assets named in the arguments: `asset`, `assets`, or the project's `media` ids.
    fn assets_named(&self, store: &Store, a: &Value) -> Result<Vec<Asset>, String> {
        let mut out: Vec<Asset> = Vec::new();
        for key in ids_of(&a["asset"]).into_iter().chain(ids_of(&a["assets"])) {
            out.push(store.resolve(&key)?);
        }
        for id in ids_of(&a["media"]) {
            let id = id.as_u64().ok_or("media takes the project's media ids (numbers)")?;
            let item = self.doc.model.media.iter().find(|m| m.id.0 == id).ok_or_else(|| format!("no media {id} in this project"))?;
            out.push(store.resolve(&json!(item.path))?);
        }
        if out.is_empty() {
            return Err("name the asset(s): asset, assets (ids, names or paths) or media (the project's media ids)".into());
        }
        out.dedup_by_key(|x| x.id);
        Ok(out)
    }

    fn hit_json(&self, h: &assets::Hit, with_why: bool) -> Value {
        let mut v = h.asset.json();
        if with_why && !h.why.is_empty() {
            v["matched"] = Value::Array(h.why.iter().map(|(field, a, b, text)| if *b > *a { json!({"in": field, "from": round(*a), "to": round(*b), "text": text}) } else { json!({"in": field, "text": text}) }).collect());
        }
        v
    }

    pub(super) fn assets_tool(&mut self, a: &Value) -> Result<Value, String> {
        let store = assets::global()?;
        let action = a.get("action").and_then(Value::as_str).unwrap_or("overview");
        // Anything the project holds is known to the library before it answers.
        let synced = if action != "forget" { self.sync_library(store) } else { None };
        let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
        match action {
            "overview" => {
                let mut v = store.counts()?;
                v["content_library"] = json!("Studio's own sounds, music, pictures and motions are not files you added: catalogue (no arguments) says what they are; find with words searches them too");
                v["bins"] = json!(store.bins()?.into_iter().map(|(_, p, rule, n)| json!({"bin": p, "smart": rule.is_some(), "assets": n})).collect::<Vec<_>>());
                v["tags"] = json!(store.label_counts("tag")?.into_iter().take(20).map(|(t, n)| json!({"tag": t, "assets": n})).collect::<Vec<_>>());
                v["people"] = json!(store.people()?.into_iter().take(10).map(|(c, n, times, assets)| json!({"group": c, "name": n, "appearances": times, "assets": assets})).collect::<Vec<_>>());
                v["places"] = json!(store.label_counts("place")?.into_iter().take(10).map(|(t, n)| json!({"place": t, "assets": n})).collect::<Vec<_>>());
                if let Some(s) = synced {
                    v["this_project"] = json!({"files": s.ids.len(), "missing": s.missing});
                }
                Ok(v)
            }
            "add" => {
                let paths = strs(&a["paths"]);
                if paths.is_empty() {
                    return Err("add needs \"paths\": files or folders".into());
                }
                let mut ids = Vec::new();
                let mut skipped = 0;
                for p in &paths {
                    let p = PathBuf::from(p);
                    let mut files = Vec::new();
                    if p.is_dir() {
                        let mut stack = vec![p.clone()];
                        while let Some(d) = stack.pop() {
                            for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.filter_map(|e| e.ok()) {
                                let ep = e.path();
                                if e.file_name().to_string_lossy().starts_with('.') {
                                    continue;
                                }
                                if ep.is_dir() {
                                    stack.push(ep);
                                } else {
                                    files.push(ep);
                                }
                            }
                        }
                    } else {
                        files.push(p);
                    }
                    for f in files {
                        match store.add_file(&f, None) {
                            Ok(id) => ids.push(id),
                            Err(_) => skipped += 1,
                        }
                    }
                }
                Ok(json!({"added": ids.len(), "skipped": skipped, "ids": ids.iter().take(200).collect::<Vec<_>>(), "next": "assets action \"index\" lets find see inside them"}))
            }
            "find" => {
                let q = Query::from_json(a);
                let hits = store.find(&q)?;
                let mut v = json!({"found": hits.len(), "assets": hits.iter().map(|h| self.hit_json(h, true)).collect::<Vec<_>>()});
                // Studio's own library (sounds, music, pictures, motions) answers the same words.
                if !q.text.is_empty() && a.get("source").and_then(Value::as_str).is_none_or(|s| s == "library" || s == "content") {
                    let own = self.library_hits(&q.text, 8);
                    if !own.is_empty() {
                        v["content_library"] = json!(own);
                    }
                }
                if hits.is_empty() && !q.text.is_empty() {
                    let c = store.counts()?;
                    v["note"] = json!(format!("nothing matches. {} assets are in the library, {} looked into so far; assets action \"index\" lets find see inside pictures, clips and sounds (speech: true for what is said)", c["assets"], c["indexed"]));
                }
                Ok(v)
            }
            "similar" => {
                let one = self.assets_named(store, a)?.remove(0);
                let like = store.similar(one.id, limit.min(50).max(1))?;
                Ok(json!({"like": one.name, "assets": like.iter().map(|(x, s)| { let mut v = x.json(); v["likeness"] = json!(round(*s)); v }).collect::<Vec<_>>()}))
            }
            "info" => {
                let one = self.assets_named(store, a)?.remove(0);
                let mut v = one.json();
                let labels = store.labels(one.id)?;
                for k in ["tag", "keyword", "person", "place"] {
                    let l: Vec<&str> = labels.iter().filter(|(t, _)| t == k).map(|(_, v)| v.as_str()).collect();
                    if !l.is_empty() {
                        v[format!("{k}s")] = json!(l);
                    }
                }
                v["rights"] = rights::get(store, one.id)?.map(|r| r.json()).unwrap_or(json!({"licence": "unknown"}));
                let bins: Vec<String> = store.bins()?.into_iter().filter(|(id, _, _, _)| store.bin_assets(*id, 100_000).is_ok_and(|x| x.iter().any(|y| y.id == one.id))).map(|(_, p, _, _)| p).collect();
                v["bins"] = json!(bins);
                v["used_in"] = json!(store.db().query("SELECT project FROM uses WHERE asset = ?", &[one.id.into()])?.iter().map(|r| r[0].text()).collect::<Vec<_>>());
                Ok(v)
            }
            "rate" => {
                let stars = a.get("stars").and_then(Value::as_i64).ok_or("rate needs \"stars\" 0-5")?;
                let list = self.assets_named(store, a)?;
                for x in &list {
                    store.set_rating(x.id, stars)?;
                }
                Ok(json!({"rated": list.len(), "stars": stars.clamp(0, 5)}))
            }
            "favourite" => {
                let on = a.get("on").and_then(Value::as_bool).or_else(|| a.get("favourite").and_then(Value::as_bool)).unwrap_or(true);
                let list = self.assets_named(store, a)?;
                for x in &list {
                    store.set_favourite(x.id, on)?;
                }
                Ok(json!({"favourite": on, "assets": list.len()}))
            }
            "tag" | "untag" => {
                let kind = a.get("kind").and_then(Value::as_str).unwrap_or("tag");
                let values: Vec<String> = [&a["values"], &a["tag"], &a["keyword"], &a["person"], &a["place"]].iter().flat_map(|v| strs(v)).collect();
                if values.is_empty() {
                    return Err(format!("{action} needs \"values\": the labels"));
                }
                let list = self.assets_named(store, a)?;
                for x in &list {
                    if action == "tag" {
                        store.label(x.id, kind, &values)?;
                    } else {
                        store.unlabel(x.id, kind, &values)?;
                    }
                }
                Ok(json!({action: values, "kind": kind, "assets": list.len()}))
            }
            "note" => {
                let text = a.get("text").and_then(Value::as_str).ok_or("note needs \"text\"")?;
                let list = self.assets_named(store, a)?;
                for x in &list {
                    store.set_note(x.id, text)?;
                }
                Ok(json!({"noted": list.len()}))
            }
            "bins" => Ok(json!({"bins": store.bins()?.into_iter().map(|(id, p, rule, n)| { let mut v = json!({"id": id, "bin": p, "assets": n}); if let Some(r) = rule { v["smart_rule"] = r; } v }).collect::<Vec<_>>()})),
            "bin" => {
                let name = a.get("name").and_then(Value::as_str).or_else(|| a.get("bin").and_then(Value::as_str)).ok_or("bin needs \"name\"")?;
                let id = store.bin_find(name)?;
                let list = store.bin_assets(id, limit)?;
                Ok(json!({"bin": name, "assets": list.iter().map(Asset::json).collect::<Vec<_>>()}))
            }
            "bin_create" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("bin_create needs \"name\" (like \"Footage/Day 1\")")?;
                let rule = a.get("rule").filter(|r| r.is_object());
                let id = store.bin_create(name, rule)?;
                let mut added = 0;
                if rule.is_none() {
                    if let Ok(list) = self.assets_named(store, a) {
                        added = store.bin_add(name, &list.iter().map(|x| x.id).collect::<Vec<_>>())?;
                    }
                }
                Ok(json!({"bin": name, "id": id, "smart": rule.is_some(), "added": added}))
            }
            "bin_add" | "bin_remove" => {
                let name = a.get("name").and_then(Value::as_str).or_else(|| a.get("bin").and_then(Value::as_str)).ok_or("needs \"name\": the bin")?;
                let ids: Vec<i64> = self.assets_named(store, a)?.iter().map(|x| x.id).collect();
                if action == "bin_add" {
                    store.bin_add(name, &ids)?;
                } else {
                    store.bin_remove(name, &ids)?;
                }
                Ok(json!({"bin": name, "assets": ids.len()}))
            }
            "bin_delete" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("bin_delete needs \"name\"")?;
                Ok(json!({"removed_bins": store.bin_delete(name)?, "note": "only the bin; the assets stay"}))
            }
            "duplicates" => {
                let groups = store.duplicates(a.get("near").and_then(Value::as_bool).unwrap_or(false))?;
                let wasted: i64 = groups.iter().map(|g| g.iter().skip(1).map(|x| x.size).sum::<i64>()).sum();
                Ok(json!({"groups": groups.len(), "wasted": safety::human(wasted.max(0) as u64), "duplicates": groups.iter().take(limit).map(|g| g.iter().map(|x| json!({"id": x.id, "path": x.path, "bytes": x.size, "missing": x.missing})).collect::<Vec<_>>()).collect::<Vec<_>>()}))
            }
            "missing" => {
                let root = self.root();
                let only = if a.get("all").and_then(Value::as_bool).unwrap_or(false) { None } else { root.as_deref() };
                let list = store.missing(only)?;
                Ok(json!({"missing": list.len(), "assets": list.iter().map(|x| json!({"id": x.id, "name": x.name, "was": x.path})).collect::<Vec<_>>(), "next": if list.is_empty() { Value::Null } else { json!("assets action \"relink\" searches for them") }}))
            }
            "relink" => {
                let root = self.root();
                let only = if a.get("all").and_then(Value::as_bool).unwrap_or(false) { None } else { root.as_deref() };
                let wanted = store.missing(only)?;
                if wanted.is_empty() {
                    return Ok(json!({"relinked": 0, "note": "nothing is missing"}));
                }
                let dirs: Vec<PathBuf> = match strs(&a["search"]) {
                    s if !s.is_empty() => s.into_iter().map(PathBuf::from).collect(),
                    _ => usual_places(root.as_deref()),
                };
                let found = store.find_moved(&wanted, &dirs);
                let apply = a.get("apply").and_then(Value::as_bool).unwrap_or(true);
                let mut done = Vec::new();
                for (asset, to, how) in &found {
                    if apply {
                        if let Some(old) = self.doc.model.media.iter().find(|m| m.path == asset.path).cloned() {
                            let pr = crate::media_io::probe(to).ok();
                            let mut item = old.clone();
                            item.path = to.display().to_string();
                            if let Some(pr) = pr {
                                item.info = m::MediaInfo { kind: pr.kind, container: pr.container, duration: pr.duration.or(old.info.duration), video: pr.video, audio: pr.audio, extra: old.info.extra.clone() };
                            }
                            if !self.doc.apply("Relink media", m::Edit::SetMedia { item: Box::new(item) }) {
                                return Err(self.doc.error.take().unwrap_or_else(|| format!("{} could not be relinked", asset.name)));
                            }
                        }
                        store.moved(asset.id, to)?;
                    }
                    done.push(json!({"asset": asset.name, "now": to.display().to_string(), "match": how}));
                }
                let left: Vec<&str> = wanted.iter().filter(|w| !found.iter().any(|(f, _, _)| f.id == w.id)).map(|w| w.name.as_str()).collect();
                Ok(json!({"relinked": if apply { done.len() } else { 0 }, "found": done, "still_missing": left, "searched": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(), "changed_project": apply && !found.is_empty(), "note": if left.is_empty() { Value::Null } else { json!("name other folders or drives in search to look further") }}))
            }
            "index" => {
                let pool: Vec<Asset> = if a.get("all").and_then(Value::as_bool).unwrap_or(false) {
                    store.unanalysed(limit.max(1))?
                } else if a.get("asset").is_some() || a.get("assets").is_some() || a.get("media").is_some() {
                    self.assets_named(store, a)?
                } else {
                    let ids: std::collections::HashSet<i64> = synced.as_ref().map(|s| s.ids.iter().copied().collect()).unwrap_or_default();
                    store.unanalysed(100_000)?.into_iter().filter(|x| ids.contains(&x.id)).take(limit.max(1)).collect()
                };
                let speech = a.get("speech").and_then(Value::as_bool).unwrap_or(false);
                let mut report = Vec::new();
                let mut notes = std::collections::BTreeSet::new();
                for x in &pool {
                    match super::library_index::index_asset(store, x, speech) {
                        Ok(f) => {
                            notes.extend(f.notes.clone());
                            report.push(json!({"asset": x.name, "frames": f.frames, "faces": f.faces, "words": f.words}));
                        }
                        Err(e) => report.push(json!({"asset": x.name, "error": e})),
                    }
                }
                Ok(json!({"looked_into": report.len(), "assets": report, "notes": notes, "left": store.unanalysed(100_000)?.len()}))
            }
            "people" => Ok(json!({"people": store.people()?.into_iter().map(|(g, n, t, c)| json!({"group": g, "name": n, "appearances": t, "assets": c})).collect::<Vec<_>>(), "note": "groups are made from how faces look; name a group once with name_person and every asset it appears in gets that person label"})),
            "name_person" => {
                let g = a.get("person").and_then(Value::as_i64).ok_or("name_person needs \"person\": the group number from people")?;
                let n = a.get("name").and_then(Value::as_str).ok_or("name_person needs \"name\"")?;
                Ok(json!({"named": n, "assets": store.name_person(g, n)?}))
            }
            "licence" | "license" => {
                let list = self.assets_named(store, a)?;
                let mut last = None;
                for x in &list {
                    last = Some(rights::set(store, x.id, a)?);
                }
                Ok(json!({"assets": list.len(), "rights": last.map(|r| r.json())}))
            }
            "licences" => Ok(json!({"licences": rights::kinds_json()})),
            "expiring" => {
                let days = a.get("days").and_then(Value::as_i64).unwrap_or(60);
                Ok(json!({"expiring": rights::expiring(store, days)?.into_iter().map(|(x, d, left)| json!({"asset": x.name, "expires": d, "days_left": left})).collect::<Vec<_>>()}))
            }
            "rights_check" => {
                let root = self.need_root()?;
                let opts = rights::Options { commercial: a.get("commercial").and_then(Value::as_bool).unwrap_or(true), ..Default::default() };
                Ok(rights::check(store, &root, &self.doc.model, &opts)?.json())
            }
            "forget" => {
                let list = self.assets_named(store, a)?;
                for x in &list {
                    store.forget(x.id)?;
                }
                Ok(json!({"forgotten": list.len(), "note": "the files are untouched"}))
            }
            other => Err(format!("assets has no action \"{other}\"; try overview, add, find, similar, info, rate, favourite, tag, untag, note, bins, bin, bin_create, bin_add, bin_remove, bin_delete, duplicates, missing, relink, index, people, name_person, licence, licences, expiring, rights_check, forget")),
        }
    }

    /// Reopens the project from disk after a restore (one session, same folder).
    fn reload_from_disk(&mut self, root: &Path) -> Result<(), String> {
        let doc = services::project::open(root)?;
        *self = Session::new(doc);
        self.home = Some(root.to_path_buf());
        Ok(())
    }

    pub(super) fn care_tool(&mut self, a: &Value) -> Result<Value, String> {
        let action = a.get("action").and_then(Value::as_str).unwrap_or("health");
        let to = |key: &str| a.get(key).and_then(Value::as_str).map(PathBuf::from);
        match action {
            "health" => {
                let root = self.need_root()?;
                // What is saved is judged; unsaved edits are not on disk to be lost yet.
                let _ = services::project::save(&mut self.doc, None);
                let mut v = safety::health(&root, None);
                if let Ok(store) = assets::global() {
                    if let Ok(rep) = rights::check(store, &root, &self.doc.model, &rights::Options::default()) {
                        v["rights"] = json!({"blocked": rep.blocked(), "warnings": rep.findings.iter().filter(|f| f.level == "warning").count(), "see": "assets action \"rights_check\""});
                    }
                }
                Ok(v)
            }
            "history" => {
                let root = self.need_root()?;
                let h = safety::history(&root);
                Ok(json!({"snapshots": h.iter().enumerate().take(a.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize).map(|(i, s)| s.json(i)).collect::<Vec<_>>(), "total": h.len(), "note": "restore with which: the number, \"latest\", or part of the file name"}))
            }
            "snapshot" => {
                let root = self.need_root()?;
                let label = a.get("label").and_then(Value::as_str).unwrap_or("");
                let p = safety::keep_snapshot(&root, &self.doc.model, label)?;
                Ok(json!({"kept": p.file_name().map(|n| n.to_string_lossy().into_owned()), "note": "autosave never removes a kept snapshot"}))
            }
            "restore" => {
                let root = self.need_root()?;
                let snap = safety::pick(&root, a.get("which").unwrap_or(&Value::Null))?;
                // What is open now is saved first so nothing is lost.
                let _ = services::project::save(&mut self.doc, None);
                let point = safety::restore(&root, &snap)?;
                self.reload_from_disk(&root)?;
                Ok(json!({"restored": snap.json(0), "previous_state_kept_as": point.file_name().map(|n| n.to_string_lossy().into_owned()), "note": "to go back, restore \"before-restore\""}))
            }
            "backup" => {
                let root = self.need_root()?;
                let dest = to("to").ok_or("backup needs \"to\": a folder or drive")?;
                let _ = services::project::save(&mut self.doc, None);
                let r = safety::backup(&root, &dest, a.get("keep").and_then(Value::as_u64).unwrap_or(5) as usize, a.get("media").and_then(Value::as_bool).unwrap_or(true))?;
                Ok(r.json())
            }
            "backups" => {
                let root = self.need_root()?;
                let dest = to("to").ok_or("backups needs \"to\": the backup folder")?;
                Ok(json!({"snapshots": safety::backups_in(&dest, &root)}))
            }
            "restore_backup" => {
                let from = to("from").or_else(|| to("to")).ok_or("restore_backup needs \"from\": the backup folder")?;
                let into = to("into").ok_or("restore_backup needs \"into\": a new folder for the restored project")?;
                let project = to("project").or_else(|| self.root()).ok_or("restore_backup needs the project it was a backup of")?;
                safety::restore_backup(&from, &project, a.get("snapshot").and_then(Value::as_str).unwrap_or("latest"), &into)
            }
            "collect" | "archive" => {
                let root = self.need_root()?;
                let dest = to("to").ok_or("needs \"to\": a new folder")?;
                let _ = services::project::save(&mut self.doc, None);
                let opts = safety::CollectOptions { only_used: a.get("only_used").and_then(Value::as_bool).unwrap_or(false), with_cache: a.get("with_cache").and_then(Value::as_bool).unwrap_or(false), checksums: action == "archive" };
                let c = safety::collect(&root, &dest, &opts)?;
                let mut v = c.json();
                v["note"] = json!(if action == "archive" { "checked with verify; every file has a SHA-256 in ARCHIVE.json" } else { "the project there points at its own copies; the original is untouched" });
                Ok(v)
            }
            "verify" => safety::verify_archive(&to("folder").or_else(|| to("to")).ok_or("verify needs \"folder\": the archive")?),
            "relocate" => {
                let dir = to("folder").ok_or("relocate needs \"folder\": the moved archive")?;
                Ok(json!({"paths_updated": safety::relocate(&dir)?}))
            }
            "clean" => {
                let root = self.need_root()?;
                let _ = services::project::save(&mut self.doc, None);
                let days = a.get("shared_cache_days").and_then(Value::as_u64).unwrap_or(30);
                let mut o = safety::CleanOptions { keep_autosaves: Some(a.get("keep_autosaves").and_then(Value::as_u64).unwrap_or(10) as usize), shared_cache_days: if days == 0 { None } else { Some(days) }, ..Default::default() };
                // Files other projects name are theirs too.
                o.other_projects = std::fs::read_dir(studio_paths::studio().join("Projects")).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("project.json").is_file() && *p != root).collect()).unwrap_or_default();
                let plan = safety::clean_plan(&root, &o);
                if a.get("apply").and_then(Value::as_bool).unwrap_or(false) {
                    let freed = safety::clean_apply(&root, &o, &plan)?;
                    let mut v = plan.json(10);
                    v["freed"] = json!(safety::human(freed));
                    v["removed"] = json!(true);
                    Ok(v)
                } else {
                    let mut v = plan.json(20);
                    v["removed"] = json!(false);
                    v["note"] = json!("nothing was removed; run again with apply: true to remove exactly this");
                    Ok(v)
                }
            }
            "recover" => {
                let root = studio_paths::studio();
                let crashed = safety::crashed(&root);
                let mut v = json!({"crashed_sessions": crashed.iter().map(|s| json!({"project": s.project.display().to_string(), "playhead": s.playhead as f64 / 1e6, "selection": s.selection, "at": safety::when(s.saved_ms)})).collect::<Vec<_>>()});
                if let Some(p) = self.root() {
                    v["newer_autosave"] = safety::newer_autosave(&p).map(|s| s.json(0)).unwrap_or(Value::Null);
                }
                Ok(v)
            }
            other => Err(format!("project_care has no action \"{other}\"; try health, history, snapshot, restore, backup, backups, restore_backup, collect, archive, verify, relocate, clean, recover")),
        }
    }

    /// Entry point from `Session::call`.
    pub(super) fn care_call(&mut self, tool: &str, a: &Value) -> Result<Vec<Content>, String> {
        let v = if tool == "assets" { self.assets_tool(a)? } else { self.care_tool(a)? };
        Ok(vec![Content::Text(v.to_string())])
    }
}

/// The window's background pass: registers the project's media in the
/// library and looks into what is new, gently (one file at a time, a short
/// rest between), at most one pass at once. Returns false when one is
/// already running.
pub fn sync_and_index(root: PathBuf, model: m::Project) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::SeqCst) {
        return false;
    }
    std::thread::spawn(move || {
        let _ = (|| -> Result<(), String> {
            let store = assets::global()?;
            let synced = store.sync_model(&root, &model)?;
            let mine: std::collections::HashSet<i64> = synced.ids.iter().copied().collect();
            for a in store.unanalysed(100_000)?.into_iter().filter(|a| mine.contains(&a.id)).take(40) {
                let _ = super::library_index::index_asset(store, &a, false);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Ok(())
        })();
        BUSY.store(false, Ordering::SeqCst);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(path: &Path, w: u32, h: u32, rgb: [u8; 3], seed: u8) {
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).unwrap()), w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        let data: Vec<u8> = (0..w * h).flat_map(|i| [rgb[0].wrapping_add((i % 7) as u8 * seed), rgb[1], rgb[2]]).collect();
        wr.write_image_data(&data).unwrap();
    }

    fn call(s: &mut Session, tool: &str, args: Value) -> Value {
        match s.run(tool, &args) {
            Ok(parts) => match parts.into_iter().next() {
                Some(Content::Text(t)) => serde_json::from_str(&t).unwrap_or(Value::String(t)),
                _ => Value::Null,
            },
            Err(e) => json!({"error": e}),
        }
    }

    /// The whole library and safety story on scratch copies, through the
    /// same entry point the AI uses.
    #[test]
    fn the_ai_can_organise_find_relink_license_and_protect() {
        let dir = std::env::temp_dir().join(format!("studio-care-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shoot")).unwrap();
        // SAFETY: set before the library is first opened; no other test in this binary uses `global()`.
        unsafe { std::env::set_var("STUDIO_ASSETS_DB", dir.join("assets.db")) };
        let (red, red2, blue) = (dir.join("shoot/red.png"), dir.join("shoot/red copy.png"), dir.join("shoot/blue.png"));
        png(&red, 48, 32, [200, 20, 20], 3);
        std::fs::copy(&red, &red2).unwrap();
        png(&blue, 48, 32, [20, 30, 200], 5);
        let mut s = Session::at(&dir.join("proj")).unwrap();
        let paths: Vec<String> = [&red, &red2, &blue].iter().map(|p| p.display().to_string()).collect();
        let r = call(&mut s, "import", json!({ "paths": paths }));
        assert_eq!(r["media"].as_array().map(Vec::len), Some(3), "{r}");
        // The project's media are in the library already; copies are found by content.
        let o = call(&mut s, "assets", json!({}));
        assert_eq!(o["assets"], 3, "{o}");
        assert_eq!(call(&mut s, "assets", json!({"action": "duplicates"}))["groups"], 1);
        // Looked into, the pictures can be searched by how they look and for likeness.
        let idx = call(&mut s, "assets", json!({"action": "index"}));
        assert_eq!(idx["looked_into"], 3, "{idx}");
        let red_hit = call(&mut s, "assets", json!({"action": "find", "query": "red"}));
        assert!(red_hit["assets"].as_array().unwrap().iter().any(|a| a["name"] == "red.png"), "{red_hit}");
        assert!(!red_hit["assets"].as_array().unwrap().iter().any(|a| a["name"] == "blue.png"), "{red_hit}");
        let like = call(&mut s, "assets", json!({"action": "similar", "asset": "red.png"}));
        assert_eq!(like["assets"][0]["name"], "red copy.png");
        assert_eq!(like["assets"][1]["name"], "blue.png");
        // Organising.
        call(&mut s, "assets", json!({"action": "rate", "asset": "blue.png", "stars": 5}));
        call(&mut s, "assets", json!({"action": "bin_create", "name": "Picks", "rule": {"min_rating": 5}}));
        assert_eq!(call(&mut s, "assets", json!({"action": "bin", "name": "Picks"}))["assets"][0]["name"], "blue.png");
        call(&mut s, "assets", json!({"action": "tag", "asset": "red.png", "kind": "place", "values": ["Jaipur"]}));
        assert_eq!(call(&mut s, "assets", json!({"action": "find", "place": "jaipur"}))["found"], 1);
        // Missing media is found again by content and the project follows.
        std::fs::create_dir_all(dir.join("elsewhere")).unwrap();
        std::fs::rename(&blue, dir.join("elsewhere/azure.png")).unwrap();
        assert_eq!(call(&mut s, "assets", json!({"action": "missing"}))["missing"], 1);
        let fix = call(&mut s, "assets", json!({"action": "relink", "search": [dir.join("elsewhere").display().to_string()]}));
        assert_eq!(fix["relinked"], 1, "{fix}");
        assert!(s.doc.model.media.iter().any(|m| m.path.ends_with("elsewhere/azure.png")));
        assert_eq!(call(&mut s, "assets", json!({"action": "missing"}))["missing"], 0);
        assert_eq!(call(&mut s, "assets", json!({"action": "bin", "name": "Picks"}))["assets"][0]["name"], "azure.png", "ratings follow the file");
        // Rights: an expired licence on something the timeline uses stops an export.
        let media_id = s.doc.model.media[0].id.0;
        let placed = call(&mut s, "edit", json!({"ops": [{"op": "place", "media": media_id, "at": 0, "duration": 1}]}));
        assert_eq!(placed["ok"], true, "{placed}");
        call(&mut s, "assets", json!({"action": "licence", "media": media_id, "licence": "licensed", "expires": "2020-01-01"}));
        let blocked = rights_gate(&s.doc, s.home.as_deref(), &json!({}));
        assert!(blocked.as_ref().is_err_and(|e| e.contains("ran out on 2020-01-01") && e.contains("ignore_rights")), "{blocked:?}");
        assert!(rights_gate(&s.doc, s.home.as_deref(), &json!({"ignore_rights": true})).is_ok());
        call(&mut s, "assets", json!({"action": "licence", "media": media_id, "licence": "cc-by", "author": "Asha", "expires": "2099-01-01"}));
        let ok = rights_gate(&s.doc, s.home.as_deref(), &json!({})).unwrap();
        assert!(ok["credits"][0].as_str().unwrap().contains("Asha"), "{ok}");
        // Safety: snapshot, change, restore, and the change itself can be undone.
        call(&mut s, "project_care", json!({"action": "snapshot", "label": "one clip"}));
        call(&mut s, "edit", json!({"ops": [{"op": "place", "media": media_id, "at": 1, "duration": 1}]}));
        assert_eq!(s.doc.project.clips.len(), 2);
        let r = call(&mut s, "project_care", json!({"action": "restore", "which": "one clip"}));
        assert!(r["restored"].is_object(), "{r}");
        assert_eq!(s.doc.project.clips.len(), 1);
        let back = call(&mut s, "project_care", json!({"action": "restore", "which": "before-restore"}));
        assert!(back["restored"].is_object(), "{back}");
        assert_eq!(s.doc.project.clips.len(), 2);
        // Backup to a "drive", then collect; the originals are not changed.
        let drive = dir.join("drive");
        let b = call(&mut s, "project_care", json!({"action": "backup", "to": drive.display().to_string()}));
        assert_eq!(b["media_copied"], 3, "{b}");
        let c = call(&mut s, "project_care", json!({"action": "archive", "to": dir.join("arch").display().to_string()}));
        assert_eq!(c["files"], 3, "{c}");
        assert_eq!(call(&mut s, "project_care", json!({"action": "verify", "folder": dir.join("arch").display().to_string()}))["intact"], true);
        let h = call(&mut s, "project_care", json!({"action": "health"}));
        assert_eq!(h["media"]["missing"], 0, "{h}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
