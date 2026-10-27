// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The asset database: every picture, clip, sound and generated file Studio
//! has seen, across all projects, in one SQLite file in Studio's data
//! folder (`Library/assets.db`; it holds the person's ratings, tags and
//! licences, so unlike a cache it is never cleaned away).
//!
//! * identity: a file is known by its fingerprint as well as its path, so
//!   moving or renaming it finds it again (relink) and copies are found
//!   (duplicates);
//! * organising: bins (folders, nested), smart bins (a saved search),
//!   collections, star ratings, favourites, tags, keywords, people, places;
//! * metadata: camera, lens, timecode, place, read from the file;
//! * rights: licence, source, attribution and expiry per asset (see
//!   [`crate::rights`]);
//! * search: one full-text index over names, labels, notes, spoken words,
//!   on-screen text and what the pictures show, with meaning-level word
//!   groups ("night" finds "dark"), and picture / sound similarity.

use crate::asset_meta;
use crate::sqlite::{Db, V};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use studio_model as m;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS assets (id INTEGER PRIMARY KEY, fp TEXT, path TEXT UNIQUE, name TEXT, kind TEXT, size INTEGER, mtime INTEGER,
  dur REAL DEFAULT 0, w INTEGER DEFAULT 0, h INTEGER DEFAULT 0, rating INTEGER DEFAULT 0, fav INTEGER DEFAULT 0, source TEXT DEFAULT 'imported',
  added INTEGER, missing INTEGER DEFAULT 0, note TEXT DEFAULT '', meta TEXT DEFAULT '{}', analysed INTEGER DEFAULT 0);
CREATE INDEX IF NOT EXISTS assets_fp ON assets (fp);
CREATE TABLE IF NOT EXISTS labels (asset INTEGER, type TEXT, value TEXT, PRIMARY KEY (asset, type, value));
CREATE INDEX IF NOT EXISTS labels_value ON labels (type, value);
CREATE TABLE IF NOT EXISTS bins (id INTEGER PRIMARY KEY, name TEXT, parent INTEGER DEFAULT 0, rule TEXT, UNIQUE (parent, name));
CREATE TABLE IF NOT EXISTS bin_items (bin INTEGER, asset INTEGER, PRIMARY KEY (bin, asset));
CREATE TABLE IF NOT EXISTS uses (asset INTEGER, project TEXT, PRIMARY KEY (asset, project));
CREATE TABLE IF NOT EXISTS rights (asset INTEGER PRIMARY KEY, licence TEXT DEFAULT 'unknown', source TEXT DEFAULT '', author TEXT DEFAULT '',
  attribution TEXT DEFAULT '', url TEXT DEFAULT '', expires TEXT DEFAULT '', cost TEXT DEFAULT '', terms TEXT DEFAULT '', updated INTEGER);
CREATE TABLE IF NOT EXISTS sigs (asset INTEGER, t REAL, vec TEXT);
CREATE INDEX IF NOT EXISTS sigs_asset ON sigs (asset);
CREATE TABLE IF NOT EXISTS faces (asset INTEGER, t REAL, cluster INTEGER, vec TEXT);
CREATE INDEX IF NOT EXISTS faces_cluster ON faces (cluster);
CREATE TABLE IF NOT EXISTS people (id INTEGER PRIMARY KEY, name TEXT DEFAULT '');
CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(asset UNINDEXED, field UNINDEXED, t0 UNINDEXED, t1 UNINDEXED, body, tokenize = 'porter unicode61');
";

/// Fields that are about the file as a whole (rebuilt when they change).
const LABEL_FIELDS: &[&str] = &["name", "tag", "keyword", "person", "place", "note", "meta", "licence"];

pub struct Store {
    db: Db,
    one: Mutex<()>,
}

/// The shared database of this Mac (or `STUDIO_ASSETS_DB`, for tests).
pub fn global() -> Result<&'static Store, String> {
    static S: OnceLock<Result<Store, String>> = OnceLock::new();
    S.get_or_init(|| Store::open(&std::env::var_os("STUDIO_ASSETS_DB").map(PathBuf::from).unwrap_or_else(|| studio_paths::studio().join("Library").join("assets.db"))))
        .as_ref()
        .map_err(|e| format!("the asset library could not open: {e}"))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn modified(p: &Path) -> i64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// One asset as the app and the AI see it.
#[derive(Clone, Debug)]
pub struct Asset {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: i64,
    pub dur: f64,
    pub w: i64,
    pub h: i64,
    pub rating: i64,
    pub fav: bool,
    pub source: String,
    pub missing: bool,
    pub note: String,
    pub meta: Value,
    pub fp: String,
}

const COLS: &str = "a.id, a.path, a.name, a.kind, a.size, a.dur, a.w, a.h, a.rating, a.fav, a.source, a.missing, a.note, a.meta, a.fp";

fn asset_of(r: &[V]) -> Asset {
    Asset {
        id: r[0].int(),
        path: r[1].text(),
        name: r[2].text(),
        kind: r[3].text(),
        size: r[4].int(),
        dur: r[5].real(),
        w: r[6].int(),
        h: r[7].int(),
        rating: r[8].int(),
        fav: r[9].int() != 0,
        source: r[10].text(),
        missing: r[11].int() != 0,
        note: r[12].text(),
        meta: serde_json::from_str(&r[13].text()).unwrap_or(Value::Null),
        fp: r[14].text(),
    }
}

impl Asset {
    pub fn json(&self) -> Value {
        let mut v = json!({"id": self.id, "name": self.name, "path": self.path, "kind": self.kind, "bytes": self.size, "seconds": (self.dur * 100.0).round() / 100.0, "rating": self.rating, "favourite": self.fav, "source": self.source});
        if self.w > 0 {
            v["size"] = json!([self.w, self.h]);
        }
        if self.missing {
            v["missing"] = json!(true);
        }
        if !self.note.is_empty() {
            v["note"] = json!(self.note);
        }
        if self.meta.as_object().is_some_and(|o| !o.is_empty()) {
            v["metadata"] = self.meta.clone();
        }
        v
    }
}

/// What a search asks for. Every field is optional; all given must hold.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// Words to find in any text (names, labels, speech, on-screen text,
    /// what the pictures show). Meaning-level groups are tried too.
    pub text: String,
    pub kinds: Vec<String>,
    pub rating_min: i64,
    pub favourite: Option<bool>,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
    pub people: Vec<String>,
    pub places: Vec<String>,
    /// A bin by path or id (a smart bin applies its saved search).
    pub bin: Option<String>,
    /// A project folder: only what it uses.
    pub project: Option<String>,
    pub missing: Option<bool>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub location: Option<String>,
    pub source: Option<String>,
    pub licence: Option<String>,
    pub min_seconds: Option<f64>,
    pub max_seconds: Option<f64>,
    pub limit: usize,
    /// "recent" (default without text), "name", "rating", "length".
    pub sort: String,
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

impl Query {
    pub fn from_json(v: &Value) -> Query {
        let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        Query {
            text: s("query").or_else(|| s("text")).unwrap_or_default(),
            kinds: strings(&v["kind"]).into_iter().chain(strings(&v["kinds"])).collect(),
            rating_min: v.get("min_rating").or_else(|| v.get("rating")).and_then(Value::as_i64).unwrap_or(0),
            favourite: v.get("favourite").and_then(Value::as_bool),
            tags: strings(&v["tags"]).into_iter().chain(strings(&v["tag"])).collect(),
            keywords: strings(&v["keywords"]).into_iter().chain(strings(&v["keyword"])).collect(),
            people: strings(&v["people"]).into_iter().chain(strings(&v["person"])).collect(),
            places: strings(&v["places"]).into_iter().chain(strings(&v["place"])).collect(),
            bin: s("bin"),
            project: s("project"),
            missing: v.get("missing").and_then(Value::as_bool),
            camera: s("camera"),
            lens: s("lens"),
            location: s("location"),
            source: s("source"),
            licence: s("licence"),
            min_seconds: v.get("min_seconds").and_then(Value::as_f64),
            max_seconds: v.get("max_seconds").and_then(Value::as_f64),
            limit: v.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize,
            sort: s("sort").unwrap_or_default(),
        }
    }
}

/// One search result: the asset and why it matched.
pub struct Hit {
    pub asset: Asset,
    /// (field, from, to, text) of the best matches inside it.
    pub why: Vec<(String, f64, f64, String)>,
    pub score: f64,
}

pub struct Synced {
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub ids: Vec<i64>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, String> {
        let db = Db::open(path)?;
        db.exec(SCHEMA)?;
        Ok(Store { db, one: Mutex::new(()) })
    }

    fn tx<T>(&self, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let _one = self.one.lock().unwrap_or_else(|e| e.into_inner());
        self.db.exec("BEGIN IMMEDIATE")?;
        let r = f();
        match &r {
            Ok(_) => {
                if let Err(e) = self.db.exec("COMMIT") {
                    let _ = self.db.exec("ROLLBACK");
                    return Err(e);
                }
            }
            Err(_) => {
                let _ = self.db.exec("ROLLBACK");
            }
        }
        r
    }

    fn q(&self, sql: &str, args: &[V]) -> Result<Vec<Vec<V>>, String> {
        self.db.query(sql, args)
    }

    fn r(&self, sql: &str, args: &[V]) -> Result<(), String> {
        self.db.run(sql, args)
    }

    // ------------------------------------------------------------ files

    /// Registers a file (or refreshes it when it changed), returning its id.
    /// `source` says where it came from ("imported", "generated", "recorded",
    /// "bundled", "downloaded").
    pub fn add_file(&self, path: &Path, source: Option<&str>) -> Result<i64, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        let p = path.display().to_string();
        let (size, mtime) = (meta.len() as i64, modified(path));
        if let Some(r) = self.q("SELECT id, size, mtime, fp FROM assets WHERE path = ?", &[p.clone().into()])?.first() {
            let id = r[0].int();
            if r[1].int() == size && r[2].int() == mtime && !r[3].text().is_empty() {
                self.r("UPDATE assets SET missing = 0 WHERE id = ?", &[id.into()])?;
                return Ok(id);
            }
        }
        let fp = asset_meta::fingerprint(path).unwrap_or_default();
        let found = asset_meta::read(path);
        let (kind, dur, w, h) = probe(path);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let source = source.map(str::to_owned).unwrap_or_else(|| guess_source(path));
        let id = self.tx(|| {
            self.r(
                "INSERT INTO assets (path, name, kind, size, mtime, fp, dur, w, h, source, added, meta) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, fp = excluded.fp, dur = excluded.dur, w = excluded.w, h = excluded.h, missing = 0, meta = excluded.meta, analysed = 0",
                &[p.clone().into(), name.into(), kind.into(), size.into(), mtime.into(), fp.into(), dur.into(), (w as i64).into(), (h as i64).into(), source.into(), now().into(), Value::Object(found).to_string().into()],
            )?;
            let id = self.q("SELECT id FROM assets WHERE path = ?", &[p.clone().into()])?[0][0].int();
            self.r("DELETE FROM sigs WHERE asset = ?", &[id.into()])?;
            self.r("DELETE FROM docs WHERE asset = ? AND field IN ('speech', 'text', 'seen')", &[id.into()])?;
            self.reindex_labels(id)?;
            Ok(id)
        })?;
        Ok(id)
    }

    /// Reads every media file of a project folder into the library and
    /// notes which project uses them. Files that are gone are marked missing.
    pub fn sync_project(&self, folder: &Path) -> Result<Synced, String> {
        let file = m::file::ProjectFolder::new(folder).load().map_err(|e| format!("{}: {e}", folder.display()))?;
        self.sync_model(folder, &file.project)
    }

    /// As [`Store::sync_project`] for a project already in memory.
    pub fn sync_model(&self, folder: &Path, project: &m::Project) -> Result<Synced, String> {
        let owner = folder.display().to_string();
        let mut out = Synced { added: 0, updated: 0, missing: 0, ids: Vec::new() };
        let mut keep = HashSet::new();
        for item in &project.media {
            let path = Path::new(&item.path);
            let existed = self.q("SELECT id FROM assets WHERE path = ?", &[item.path.clone().into()])?.first().map(|r| r[0].int());
            let id = if path.is_file() {
                let source = if path.starts_with(folder.join("media")) { None } else { None::<&str> };
                let id = self.add_file(path, source)?;
                if existed.is_none() {
                    out.added += 1;
                } else {
                    out.updated += 1;
                }
                id
            } else {
                // Not on disk: keep what is known, or make a stub so it can be relinked.
                let id = match existed {
                    Some(id) => id,
                    None => {
                        let kind = format!("{:?}", item.info.kind).to_lowercase();
                        self.r("INSERT INTO assets (path, name, kind, size, mtime, fp, added, source) VALUES (?, ?, ?, 0, 0, '', ?, 'imported')", &[item.path.clone().into(), item.name.clone().into(), kind.into(), now().into()])?;
                        self.q("SELECT id FROM assets WHERE path = ?", &[item.path.clone().into()])?[0][0].int()
                    }
                };
                self.r("UPDATE assets SET missing = 1 WHERE id = ?", &[id.into()])?;
                out.missing += 1;
                id
            };
            // The project's own facts fill what the file did not say.
            if item.info.duration.is_some() || item.info.video.is_some() {
                let dur = item.info.duration.unwrap_or(0) as f64 / 1e6;
                let (w, h) = item.info.video.as_ref().map_or((0, 0), |v| (v.width as i64, v.height as i64));
                self.r("UPDATE assets SET dur = CASE WHEN dur = 0 THEN ? ELSE dur END, w = CASE WHEN w = 0 THEN ? ELSE w END, h = CASE WHEN h = 0 THEN ? ELSE h END WHERE id = ?", &[dur.into(), w.into(), h.into(), id.into()])?;
            }
            self.r("INSERT OR IGNORE INTO uses VALUES (?, ?)", &[id.into(), owner.clone().into()])?;
            if let Some(f) = item.folder.as_deref().filter(|f| !f.is_empty()) {
                let bin = self.bin_path(f)?;
                self.r("INSERT OR IGNORE INTO bin_items VALUES (?, ?)", &[bin.into(), id.into()])?;
            }
            keep.insert(id);
            out.ids.push(id);
        }
        // What the project no longer holds.
        for r in self.q("SELECT asset FROM uses WHERE project = ?", &[owner.clone().into()])? {
            if !keep.contains(&r[0].int()) {
                self.r("DELETE FROM uses WHERE asset = ? AND project = ?", &[r[0].clone(), owner.clone().into()])?;
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: i64) -> Result<Asset, String> {
        self.q(&format!("SELECT {COLS} FROM assets a WHERE a.id = ?"), &[id.into()])?.first().map(|r| asset_of(r)).ok_or_else(|| format!("no asset {id}"))
    }

    /// An asset by id, or by path or (part of) name when it is unique.
    pub fn resolve(&self, key: &Value) -> Result<Asset, String> {
        if let Some(id) = key.as_i64() {
            return self.get(id);
        }
        let s = key.as_str().ok_or("name an asset by id, path or name")?;
        if let Ok(id) = s.parse::<i64>() {
            if let Ok(a) = self.get(id) {
                return Ok(a);
            }
        }
        let rows = self.q(&format!("SELECT {COLS} FROM assets a WHERE a.path = ? OR a.name = ? OR a.name LIKE ? ORDER BY (a.path = ?) DESC, a.missing, a.added DESC LIMIT 5"), &[s.into(), s.into(), format!("%{s}%").into(), s.into()])?;
        match rows.len() {
            0 => Err(format!("no asset matches \"{s}\"; use assets action \"find\" to look")),
            1 => Ok(asset_of(&rows[0])),
            _ => {
                let first = asset_of(&rows[0]);
                if first.path == s || first.name == s && asset_of(&rows[1]).name != s {
                    Ok(first)
                } else {
                    Err(format!("\"{s}\" matches several assets ({}); use an id", rows.iter().map(|r| format!("{} {}", r[0].int(), r[2].text())).collect::<Vec<_>>().join(", ")))
                }
            }
        }
    }

    // ------------------------------------------------------- organising

    pub fn set_rating(&self, id: i64, stars: i64) -> Result<(), String> {
        self.r("UPDATE assets SET rating = ? WHERE id = ?", &[stars.clamp(0, 5).into(), id.into()])
    }

    pub fn set_favourite(&self, id: i64, on: bool) -> Result<(), String> {
        self.r("UPDATE assets SET fav = ? WHERE id = ?", &[(on as i64).into(), id.into()])
    }

    pub fn set_note(&self, id: i64, note: &str) -> Result<(), String> {
        self.tx(|| {
            self.r("UPDATE assets SET note = ? WHERE id = ?", &[note.into(), id.into()])?;
            self.reindex_labels(id)
        })
    }

    /// Adds labels of `kind` ("tag", "keyword", "person", "place").
    pub fn label(&self, id: i64, kind: &str, values: &[String]) -> Result<(), String> {
        if !["tag", "keyword", "person", "place"].contains(&kind) {
            return Err(format!("unknown label kind \"{kind}\" (tag, keyword, person, place)"));
        }
        self.tx(|| {
            for v in values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
                self.r("INSERT OR IGNORE INTO labels VALUES (?, ?, ?)", &[id.into(), kind.into(), v.into()])?;
            }
            self.reindex_labels(id)
        })
    }

    pub fn unlabel(&self, id: i64, kind: &str, values: &[String]) -> Result<(), String> {
        self.tx(|| {
            for v in values {
                self.r("DELETE FROM labels WHERE asset = ? AND type = ? AND lower(value) = lower(?)", &[id.into(), kind.into(), v.trim().into()])?;
            }
            self.reindex_labels(id)
        })
    }

    pub fn labels(&self, id: i64) -> Result<Vec<(String, String)>, String> {
        Ok(self.q("SELECT type, value FROM labels WHERE asset = ? ORDER BY type, value", &[id.into()])?.iter().map(|r| (r[0].text(), r[1].text())).collect())
    }

    /// Rebuilds the searchable text about the file itself.
    fn reindex_labels(&self, id: i64) -> Result<(), String> {
        self.r(&format!("DELETE FROM docs WHERE asset = ? AND field IN ({})", LABEL_FIELDS.iter().map(|f| format!("'{f}'")).collect::<Vec<_>>().join(",")), &[id.into()])?;
        let a = self.get(id)?;
        let add = |field: &str, body: &str| -> Result<(), String> {
            if body.trim().is_empty() {
                return Ok(());
            }
            self.r("INSERT INTO docs (asset, field, t0, t1, body) VALUES (?, ?, 0, 0, ?)", &[id.into(), field.into(), body.into()])
        };
        let stem = Path::new(&a.name).file_stem().map(|s| s.to_string_lossy().replace(['_', '-', '.'], " ")).unwrap_or_default();
        add("name", &format!("{} {stem}", a.name))?;
        add("note", &a.note)?;
        for (kind, value) in self.labels(id)? {
            add(&kind, &value)?;
        }
        let mut meta = String::new();
        if let Some(o) = a.meta.as_object() {
            for (k, v) in o {
                if k == "location" {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    meta.push_str(s);
                    meta.push(' ');
                }
            }
        }
        add("meta", &meta)?;
        if let Some(r) = self.q("SELECT licence, source, author FROM rights WHERE asset = ?", &[id.into()])?.first() {
            add("licence", &format!("{} {} {}", r[0].text(), r[1].text(), r[2].text()))?;
        }
        Ok(())
    }

    /// Reindex after rights changed (called by `rights`).
    pub(crate) fn reindex(&self, id: i64) -> Result<(), String> {
        self.tx(|| self.reindex_labels(id))
    }

    pub(crate) fn db(&self) -> &Db {
        &self.db
    }

    /// The bin at a `/`-separated path, creating it (and parents).
    pub fn bin_path(&self, path: &str) -> Result<i64, String> {
        let mut parent = 0i64;
        for part in path.split('/').map(str::trim).filter(|p| !p.is_empty()) {
            let found = self.q("SELECT id FROM bins WHERE parent = ? AND name = ?", &[parent.into(), part.into()])?.first().map(|r| r[0].int());
            parent = match found {
                Some(id) => id,
                None => {
                    self.r("INSERT INTO bins (name, parent) VALUES (?, ?)", &[part.into(), parent.into()])?;
                    self.q("SELECT id FROM bins WHERE parent = ? AND name = ?", &[parent.into(), part.into()])?[0][0].int()
                }
            };
        }
        if parent == 0 {
            return Err("a bin needs a name".into());
        }
        Ok(parent)
    }

    /// Finds a bin by id or path without creating it.
    pub fn bin_find(&self, key: &str) -> Result<i64, String> {
        if let Ok(id) = key.parse::<i64>() {
            if !self.q("SELECT 1 FROM bins WHERE id = ?", &[id.into()])?.is_empty() {
                return Ok(id);
            }
        }
        let mut parent = 0i64;
        for part in key.split('/').map(str::trim).filter(|p| !p.is_empty()) {
            parent = self
                .q("SELECT id FROM bins WHERE parent = ? AND lower(name) = lower(?)", &[parent.into(), part.into()])?
                .first()
                .map(|r| r[0].int())
                .ok_or_else(|| format!("no bin \"{key}\"; use assets action \"bins\" to list them"))?;
        }
        Ok(parent)
    }

    /// Makes a bin; with a `rule` (a search) it is a smart bin that always
    /// holds whatever the search finds.
    pub fn bin_create(&self, path: &str, rule: Option<&Value>) -> Result<i64, String> {
        self.tx(|| {
            let id = self.bin_path(path)?;
            if let Some(rule) = rule {
                self.r("UPDATE bins SET rule = ? WHERE id = ?", &[rule.to_string().into(), id.into()])?;
            }
            Ok(id)
        })
    }

    pub fn bin_delete(&self, key: &str) -> Result<usize, String> {
        let id = self.bin_find(key)?;
        self.tx(|| {
            let mut all = vec![id];
            let mut i = 0;
            while i < all.len() {
                for r in self.q("SELECT id FROM bins WHERE parent = ?", &[all[i].into()])? {
                    all.push(r[0].int());
                }
                i += 1;
            }
            for b in &all {
                self.r("DELETE FROM bin_items WHERE bin = ?", &[(*b).into()])?;
                self.r("DELETE FROM bins WHERE id = ?", &[(*b).into()])?;
            }
            Ok(all.len())
        })
    }

    pub fn bin_add(&self, key: &str, ids: &[i64]) -> Result<usize, String> {
        let bin = self.bin_find(key).or_else(|_| self.bin_create(key, None))?;
        if self.q("SELECT rule FROM bins WHERE id = ?", &[bin.into()])?.first().is_some_and(|r| !r[0].text().is_empty()) {
            return Err("that is a smart bin: it fills itself from its search; change the search instead".into());
        }
        self.tx(|| {
            for id in ids {
                self.r("INSERT OR IGNORE INTO bin_items VALUES (?, ?)", &[bin.into(), (*id).into()])?;
            }
            Ok(ids.len())
        })
    }

    pub fn bin_remove(&self, key: &str, ids: &[i64]) -> Result<(), String> {
        let bin = self.bin_find(key)?;
        for id in ids {
            self.r("DELETE FROM bin_items WHERE bin = ? AND asset = ?", &[bin.into(), (*id).into()])?;
        }
        Ok(())
    }

    /// Every bin as (id, path, smart rule, number of assets).
    pub fn bins(&self) -> Result<Vec<(i64, String, Option<Value>, usize)>, String> {
        let rows = self.q("SELECT id, name, parent, rule FROM bins ORDER BY name", &[])?;
        let by_id: HashMap<i64, (String, i64)> = rows.iter().map(|r| (r[0].int(), (r[1].text(), r[2].int()))).collect();
        let mut out = Vec::new();
        for r in &rows {
            let (mut path, mut at) = (r[1].text(), r[2].int());
            while at != 0 {
                let Some((n, p)) = by_id.get(&at) else { break };
                path = format!("{n}/{path}");
                at = *p;
            }
            let rule = serde_json::from_str::<Value>(&r[3].text()).ok();
            let n = self.bin_assets(r[0].int(), 100_000)?.len();
            out.push((r[0].int(), path, rule, n));
        }
        out.sort_by(|a, b| a.1.cmp(&b.1));
        Ok(out)
    }

    /// The contents of a bin (a smart bin runs its search; a manual bin
    /// includes what is in its sub-bins).
    pub fn bin_assets(&self, bin: i64, limit: usize) -> Result<Vec<Asset>, String> {
        let rule = self.q("SELECT rule FROM bins WHERE id = ?", &[bin.into()])?.first().map(|r| r[0].text()).unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<Value>(&rule) {
            let mut q = Query::from_json(&v);
            q.limit = limit;
            q.bin = None;
            return Ok(self.find(&q)?.into_iter().map(|h| h.asset).collect());
        }
        let mut all = vec![bin];
        let mut i = 0;
        while i < all.len() {
            for r in self.q("SELECT id FROM bins WHERE parent = ?", &[all[i].into()])? {
                all.push(r[0].int());
            }
            i += 1;
        }
        let ids = all.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",");
        Ok(self.q(&format!("SELECT DISTINCT {COLS} FROM assets a JOIN bin_items b ON b.asset = a.id WHERE b.bin IN ({ids}) ORDER BY a.name LIMIT {limit}"), &[])?.iter().map(|r| asset_of(r)).collect())
    }

    // -------------------------------------------------------------- find

    /// Searches the library.
    pub fn find(&self, q: &Query) -> Result<Vec<Hit>, String> {
        let limit = q.limit.clamp(1, 500);
        // Text first: ranked ids with the passages that matched.
        let mut ranked: Vec<(i64, f64)> = Vec::new();
        let mut why: HashMap<i64, Vec<(String, f64, f64, String)>> = HashMap::new();
        let has_text = !q.text.trim().is_empty();
        if has_text {
            for (strict, fts) in fts_queries(&q.text) {
                let rows = self.q(
                    "SELECT asset, field, t0, t1, snippet(docs, 4, '[', ']', '…', 12), bm25(docs) FROM docs WHERE docs MATCH ? ORDER BY bm25(docs) LIMIT 2000",
                    &[fts.into()],
                )?;
                let mut score: HashMap<i64, f64> = HashMap::new();
                for r in &rows {
                    let id = r[0].int();
                    let field = r[1].text();
                    // Labels and names the person wrote count for more than a guess.
                    let weight = match field.as_str() {
                        "tag" | "keyword" | "person" | "place" => 2.0,
                        "name" | "note" => 1.5,
                        "seen" => 0.7,
                        _ => 1.0,
                    };
                    *score.entry(id).or_default() += -r[5].real() * weight + weight;
                    let w = why.entry(id).or_default();
                    if w.len() < 3 {
                        w.push((field, r[2].real(), r[3].real(), r[4].text()));
                    }
                }
                if !score.is_empty() || !strict {
                    ranked = score.into_iter().collect();
                    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
                    break;
                }
            }
            if ranked.is_empty() {
                return Ok(Vec::new());
            }
        }
        let mut conds: Vec<String> = Vec::new();
        let mut args: Vec<V> = Vec::new();
        let mut join = String::new();
        if has_text {
            conds.push(format!("a.id IN ({})", ranked.iter().take(1000).map(|(i, _)| i.to_string()).collect::<Vec<_>>().join(",")));
        }
        if !q.kinds.is_empty() {
            conds.push(format!("a.kind IN ({})", vec!["?"; q.kinds.len()].join(",")));
            args.extend(q.kinds.iter().map(|k| V::from(k.to_lowercase())));
        }
        if q.rating_min > 0 {
            conds.push("a.rating >= ?".into());
            args.push(q.rating_min.into());
        }
        if let Some(f) = q.favourite {
            conds.push("a.fav = ?".into());
            args.push((f as i64).into());
        }
        if let Some(mi) = q.missing {
            conds.push("a.missing = ?".into());
            args.push((mi as i64).into());
        }
        for (kind, list) in [("tag", &q.tags), ("keyword", &q.keywords), ("person", &q.people), ("place", &q.places)] {
            for v in list {
                conds.push("EXISTS (SELECT 1 FROM labels l WHERE l.asset = a.id AND l.type = ? AND lower(l.value) = lower(?))".into());
                args.push(kind.into());
                args.push(v.as_str().into());
            }
        }
        if let Some(bin) = &q.bin {
            let id = self.bin_find(bin)?;
            let items = self.bin_assets(id, 100_000)?;
            conds.push(format!("a.id IN ({})", if items.is_empty() { "0".to_owned() } else { items.iter().map(|a| a.id.to_string()).collect::<Vec<_>>().join(",") }));
        }
        if let Some(p) = &q.project {
            join.push_str(" JOIN uses u ON u.asset = a.id");
            conds.push("u.project = ?".into());
            args.push(p.as_str().into());
        }
        for (key, val) in [("camera", &q.camera), ("lens", &q.lens)] {
            if let Some(v) = val {
                conds.push("lower(json_extract(a.meta, ?)) LIKE lower(?)".into());
                args.push(format!("$.{key}").into());
                args.push(format!("%{v}%").into());
            }
        }
        if let Some(v) = &q.location {
            // A place name from the labels, or a lat,lon pair near the shot's position.
            conds.push("EXISTS (SELECT 1 FROM labels l WHERE l.asset = a.id AND l.type = 'place' AND lower(l.value) LIKE lower(?))".into());
            args.push(format!("%{v}%").into());
        }
        if let Some(v) = &q.source {
            conds.push("a.source = ?".into());
            args.push(v.as_str().into());
        }
        if let Some(v) = &q.licence {
            conds.push("EXISTS (SELECT 1 FROM rights r WHERE r.asset = a.id AND lower(r.licence) = lower(?))".into());
            args.push(v.as_str().into());
        }
        if let Some(v) = q.min_seconds {
            conds.push("a.dur >= ?".into());
            args.push(v.into());
        }
        if let Some(v) = q.max_seconds {
            conds.push("a.dur <= ?".into());
            args.push(v.into());
        }
        let order = match q.sort.as_str() {
            "name" => "a.name COLLATE NOCASE",
            "rating" => "a.rating DESC, a.name",
            "length" => "a.dur DESC",
            _ => "a.added DESC, a.id DESC",
        };
        let cap = if has_text { 1000 } else { limit };
        let sql = format!("SELECT DISTINCT {COLS} FROM assets a{join}{} ORDER BY {order} LIMIT {cap}", if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) });
        let mut found: Vec<Asset> = self.q(&sql, &args)?.iter().map(|r| asset_of(r)).collect();
        let mut score_of: HashMap<i64, f64> = HashMap::new();
        if has_text {
            score_of = ranked.into_iter().collect();
            if q.sort.is_empty() {
                found.sort_by(|a, b| score_of.get(&b.id).unwrap_or(&0.0).total_cmp(score_of.get(&a.id).unwrap_or(&0.0)));
            }
            found.truncate(limit);
        }
        Ok(found
            .into_iter()
            .map(|a| {
                let score = score_of.get(&a.id).copied().unwrap_or(0.0);
                let why = why.remove(&a.id).unwrap_or_default();
                Hit { asset: a, why, score }
            })
            .collect())
    }

    /// Counts for an overview.
    pub fn counts(&self) -> Result<Value, String> {
        let n = |sql: &str| self.q(sql, &[]).ok().and_then(|r| r.first().map(|r| r[0].int())).unwrap_or(0);
        Ok(json!({
            "assets": n("SELECT count(*) FROM assets"),
            "missing": n("SELECT count(*) FROM assets WHERE missing = 1"),
            "favourites": n("SELECT count(*) FROM assets WHERE fav = 1"),
            "rated": n("SELECT count(*) FROM assets WHERE rating > 0"),
            "bins": n("SELECT count(*) FROM bins"),
            "indexed": n("SELECT count(*) FROM assets WHERE analysed > 0"),
            "unlicensed": n("SELECT count(*) FROM assets a WHERE NOT EXISTS (SELECT 1 FROM rights r WHERE r.asset = a.id AND r.licence != 'unknown')"),
        }))
    }

    /// Every label in use, with how many assets carry it.
    pub fn label_counts(&self, kind: &str) -> Result<Vec<(String, i64)>, String> {
        Ok(self.q("SELECT value, count(*) FROM labels WHERE type = ? GROUP BY lower(value) ORDER BY count(*) DESC, value LIMIT 500", &[kind.into()])?.iter().map(|r| (r[0].text(), r[1].int())).collect())
    }

    // -------------------------------------------------------- duplicates

    /// Groups of files with the same content (confirmed byte for byte).
    /// `near`: also pictures that look the same and run the same length.
    pub fn duplicates(&self, near: bool) -> Result<Vec<Vec<Asset>>, String> {
        let rows = self.q(&format!("SELECT {COLS} FROM assets a WHERE a.fp != '' AND a.fp IN (SELECT fp FROM assets WHERE fp != '' GROUP BY fp HAVING count(*) > 1) ORDER BY a.fp, a.id"), &[])?;
        let mut groups: Vec<Vec<Asset>> = Vec::new();
        let mut by_fp: HashMap<String, Vec<Asset>> = HashMap::new();
        for r in &rows {
            let a = asset_of(r);
            by_fp.entry(a.fp.clone()).or_default().push(a);
        }
        for (_, list) in by_fp {
            // Same fingerprint: confirm with a full hash against the first that exists.
            let mut kept: Vec<Vec<Asset>> = Vec::new();
            for a in list {
                if a.missing {
                    // A missing file cannot be compared; the fingerprint is all there is.
                    match kept.first_mut() {
                        Some(g) => g.push(a),
                        None => kept.push(vec![a]),
                    }
                    continue;
                }
                match kept.iter_mut().find(|g| g.iter().find(|x| !x.missing).is_some_and(|x| asset_meta::same_content(Path::new(&x.path), Path::new(&a.path)))) {
                    Some(g) => g.push(a),
                    None => kept.push(vec![a]),
                }
            }
            groups.extend(kept.into_iter().filter(|g| g.len() > 1));
        }
        if near {
            let sigs = self.all_sigs()?;
            let mut seen: HashSet<i64> = groups.iter().flatten().map(|a| a.id).collect();
            let assets: Vec<Asset> = self.q(&format!("SELECT {COLS} FROM assets a WHERE a.id IN (SELECT DISTINCT asset FROM sigs)"), &[])?.iter().map(|r| asset_of(r)).collect();
            for (i, a) in assets.iter().enumerate() {
                if seen.contains(&a.id) {
                    continue;
                }
                let mut g = vec![a.clone()];
                for b in &assets[i + 1..] {
                    if seen.contains(&b.id) || a.kind != b.kind || (a.dur - b.dur).abs() > 0.02 * a.dur.max(0.5) {
                        continue;
                    }
                    if let (Some(x), Some(y)) = (sigs.get(&a.id), sigs.get(&b.id)) {
                        if best_match(x, y) > 0.995 {
                            g.push(b.clone());
                        }
                    }
                }
                if g.len() > 1 {
                    seen.extend(g.iter().map(|a| a.id));
                    groups.push(g);
                }
            }
        }
        groups.sort_by_key(|g| std::cmp::Reverse(g.iter().map(|a| a.size).sum::<i64>()));
        Ok(groups)
    }

    // ------------------------------------------------------------ relink

    /// Missing assets, optionally only those a project uses.
    pub fn missing(&self, project: Option<&Path>) -> Result<Vec<Asset>, String> {
        // Re-check the disk first: a drive may have come back.
        let rows = self.q(&format!("SELECT {COLS} FROM assets a WHERE a.missing = 1 OR a.id IN (SELECT id FROM assets)"), &[])?;
        let mut out = Vec::new();
        let used: Option<HashSet<i64>> = match project {
            Some(p) => Some(self.q("SELECT asset FROM uses WHERE project = ?", &[p.display().to_string().into()])?.iter().map(|r| r[0].int()).collect()),
            None => None,
        };
        for r in rows {
            let mut a = asset_of(&r);
            if used.as_ref().is_some_and(|u| !u.contains(&a.id)) {
                continue;
            }
            let there = Path::new(&a.path).is_file();
            if there == a.missing {
                self.r("UPDATE assets SET missing = ? WHERE id = ?", &[((!there) as i64).into(), a.id.into()])?;
                a.missing = !there;
            }
            if a.missing {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// Looks for `wanted` (missing assets) in `dirs`: same fingerprint
    /// first, then same size and name. Returns (asset, where found, how sure).
    pub fn find_moved(&self, wanted: &[Asset], dirs: &[PathBuf]) -> Vec<(Asset, PathBuf, &'static str)> {
        let mut by_size: HashMap<i64, Vec<&Asset>> = HashMap::new();
        for a in wanted.iter().filter(|a| a.size > 0) {
            by_size.entry(a.size).or_default().push(a);
        }
        let mut found: HashMap<i64, (PathBuf, &'static str)> = HashMap::new();
        let mut visited = 0usize;
        let mut stack: Vec<(PathBuf, usize)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
        let mut seen_dirs = HashSet::new();
        while let Some((dir, depth)) = stack.pop() {
            if visited > 400_000 || depth > 12 || !seen_dirs.insert(dir.clone()) {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.filter_map(|e| e.ok()) {
                visited += 1;
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if !n.starts_with('.') && !n.ends_with(".app") && n != "Library" && n != "node_modules" {
                        stack.push((p, depth + 1));
                    }
                } else if ft.is_file() {
                    let Ok(md) = e.metadata() else { continue };
                    let Some(cands) = by_size.get(&(md.len() as i64)) else { continue };
                    let fp = asset_meta::fingerprint(&p).unwrap_or_default();
                    for a in cands {
                        if found.get(&a.id).is_some_and(|f| f.1 == "exact") {
                            continue;
                        }
                        if !a.fp.is_empty() && a.fp == fp {
                            found.insert(a.id, (p.clone(), "exact"));
                        } else if a.fp.is_empty() && p.file_name().is_some_and(|n| n.to_string_lossy() == a.name) {
                            found.entry(a.id).or_insert((p.clone(), "same name and size"));
                        }
                    }
                }
            }
        }
        wanted.iter().filter_map(|a| found.remove(&a.id).map(|(p, how)| (a.clone(), p, how))).collect()
    }

    /// Points an asset at its new place in the library.
    pub fn moved(&self, id: i64, to: &Path) -> Result<(), String> {
        let p = to.display().to_string();
        // Another row may already own that path (the file was registered on its own).
        if let Some(r) = self.q("SELECT id FROM assets WHERE path = ? AND id != ?", &[p.clone().into(), id.into()])?.first() {
            let other = r[0].int();
            self.tx(|| {
                for (t, c) in [("labels", "asset"), ("bin_items", "asset"), ("uses", "asset")] {
                    self.r(&format!("UPDATE OR IGNORE {t} SET {c} = ? WHERE {c} = ?"), &[id.into(), other.into()])?;
                    self.r(&format!("DELETE FROM {t} WHERE {c} = ?"), &[other.into()])?;
                }
                self.r("DELETE FROM docs WHERE asset = ?", &[other.into()])?;
                self.r("DELETE FROM sigs WHERE asset = ?", &[other.into()])?;
                self.r("DELETE FROM assets WHERE id = ?", &[other.into()])
            })?;
        }
        self.tx(|| {
            self.r("UPDATE assets SET path = ?, name = ?, missing = 0, mtime = ? WHERE id = ?", &[p.into(), to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into(), modified(to).into(), id.into()])?;
            self.reindex_labels(id)
        })
    }

    // ------------------------------------------------- what is in it

    /// Replaces the passages of one field ("speech", "text", "seen") found
    /// in an asset: (from, to, text).
    pub fn record(&self, id: i64, field: &str, passages: &[(f64, f64, String)]) -> Result<(), String> {
        self.tx(|| {
            self.r("DELETE FROM docs WHERE asset = ? AND field = ?", &[id.into(), field.into()])?;
            for (a, b, t) in passages {
                if !t.trim().is_empty() {
                    self.r("INSERT INTO docs (asset, field, t0, t1, body) VALUES (?, ?, ?, ?, ?)", &[id.into(), field.into(), (*a).into(), (*b).into(), t.as_str().into()])?;
                }
            }
            Ok(())
        })
    }

    /// Stores the picture signatures of an asset (one per sampled moment).
    pub fn record_sigs(&self, id: i64, sigs: &[(f64, Vec<f32>)]) -> Result<(), String> {
        self.tx(|| {
            self.r("DELETE FROM sigs WHERE asset = ?", &[id.into()])?;
            for (t, v) in sigs {
                let s = v.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(",");
                self.r("INSERT INTO sigs VALUES (?, ?, ?)", &[id.into(), (*t).into(), s.into()])?;
            }
            Ok(())
        })
    }

    pub fn mark_analysed(&self, id: i64, level: i64) -> Result<(), String> {
        self.r("UPDATE assets SET analysed = ? WHERE id = ?", &[level.into(), id.into()])
    }

    /// Assets the background pass has not looked into yet.
    pub fn unanalysed(&self, limit: usize) -> Result<Vec<Asset>, String> {
        Ok(self.q(&format!("SELECT {COLS} FROM assets a WHERE a.analysed = 0 AND a.missing = 0 ORDER BY a.added DESC LIMIT {limit}"), &[])?.iter().map(|r| asset_of(r)).collect())
    }

    fn all_sigs(&self) -> Result<HashMap<i64, Vec<Vec<f32>>>, String> {
        let mut out: HashMap<i64, Vec<Vec<f32>>> = HashMap::new();
        for r in self.q("SELECT asset, vec FROM sigs", &[])? {
            out.entry(r[0].int()).or_default().push(r[1].text().split(',').filter_map(|x| x.parse().ok()).collect());
        }
        Ok(out)
    }

    /// Assets that look (or sound) like `id`, best first, with how like.
    pub fn similar(&self, id: i64, limit: usize) -> Result<Vec<(Asset, f64)>, String> {
        let sigs = self.all_sigs()?;
        let mine = sigs.get(&id).ok_or("that asset has not been looked at yet; run assets action \"index\" first")?;
        let me = self.get(id)?;
        let mut scored: Vec<(i64, f64)> = sigs.iter().filter(|(k, _)| **k != id).map(|(k, v)| (*k, best_match(mine, v))).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut out = Vec::new();
        for (k, s) in scored.into_iter().take(limit.max(1) * 2) {
            let a = self.get(k)?;
            // A video of a still, or a tune beside a picture, is not "like" it.
            if (a.kind == "audio") != (me.kind == "audio") {
                continue;
            }
            out.push((a, s));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    // ----------------------------------------------------------- people

    /// Face groups: (cluster, name, appearances, assets).
    pub fn people(&self) -> Result<Vec<(i64, String, i64, i64)>, String> {
        Ok(self
            .q("SELECT f.cluster, coalesce(p.name, ''), count(*), count(DISTINCT f.asset) FROM faces f LEFT JOIN people p ON p.id = f.cluster GROUP BY f.cluster ORDER BY count(*) DESC", &[])?
            .iter()
            .map(|r| (r[0].int(), r[1].text(), r[2].int(), r[3].int()))
            .collect())
    }

    /// Names a face group; its assets get the person label too.
    pub fn name_person(&self, cluster: i64, name: &str) -> Result<usize, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("give the person a name".into());
        }
        self.tx(|| {
            self.r("INSERT OR REPLACE INTO people VALUES (?, ?)", &[cluster.into(), name.into()])?;
            let ids: Vec<i64> = self.q("SELECT DISTINCT asset FROM faces WHERE cluster = ?", &[cluster.into()])?.iter().map(|r| r[0].int()).collect();
            for id in &ids {
                self.r("INSERT OR IGNORE INTO labels VALUES (?, 'person', ?)", &[(*id).into(), name.into()])?;
                self.reindex_labels(*id)?;
            }
            Ok(ids.len())
        })
    }

    /// Adds the faces found in an asset, giving each to the nearest known
    /// face group or starting a new one.
    pub fn record_faces(&self, id: i64, faces: &[(f64, Vec<f32>)]) -> Result<(), String> {
        self.tx(|| {
            self.r("DELETE FROM faces WHERE asset = ?", &[id.into()])?;
            let mut centres: Vec<(i64, Vec<f32>)> = Vec::new();
            for r in self.q("SELECT cluster, vec FROM faces", &[])? {
                let v: Vec<f32> = r[1].text().split(',').filter_map(|x| x.parse().ok()).collect();
                if !centres.iter().any(|(c, _)| *c == r[0].int()) {
                    centres.push((r[0].int(), v));
                }
            }
            let mut next = self.q("SELECT coalesce(max(cluster), 0) + 1 FROM faces", &[])?[0][0].int();
            for (t, v) in faces {
                let best = centres.iter().map(|(c, cv)| (*c, cosine(v, cv))).max_by(|a, b| a.1.total_cmp(&b.1));
                let cluster = match best {
                    Some((c, s)) if s >= FACE_SAME => c,
                    _ => {
                        let c = next;
                        next += 1;
                        centres.push((c, v.clone()));
                        c
                    }
                };
                let s = v.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(",");
                self.r("INSERT INTO faces VALUES (?, ?, ?, ?)", &[id.into(), (*t).into(), cluster.into(), s.into()])?;
            }
            // Named people are labels on their assets.
            let named: Vec<(i64, String)> = self.q("SELECT DISTINCT f.asset, p.name FROM faces f JOIN people p ON p.id = f.cluster WHERE f.asset = ? AND p.name != ''", &[id.into()])?.iter().map(|r| (r[0].int(), r[1].text())).collect();
            for (a, n) in named {
                self.r("INSERT OR IGNORE INTO labels VALUES (?, 'person', ?)", &[a.into(), n.into()])?;
            }
            self.reindex_labels(id)
        })
    }

    pub fn forget(&self, id: i64) -> Result<(), String> {
        self.tx(|| {
            for (t, c) in [("labels", "asset"), ("bin_items", "asset"), ("uses", "asset"), ("rights", "asset"), ("sigs", "asset"), ("faces", "asset"), ("docs", "asset"), ("assets", "id")] {
                self.r(&format!("DELETE FROM {t} WHERE {c} = ?"), &[id.into()])?;
            }
            Ok(())
        })
    }
}

/// Faces closer than this are the same person.
pub const FACE_SAME: f64 = 0.9;

/// Cosine similarity of two signatures.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut d, mut x, mut y) = (0.0f64, 0.0f64, 0.0f64);
    for (p, q) in a.iter().zip(b) {
        d += (*p as f64) * (*q as f64);
        x += (*p as f64).powi(2);
        y += (*q as f64).powi(2);
    }
    if x == 0.0 || y == 0.0 { 0.0 } else { d / (x.sqrt() * y.sqrt()) }
}

/// How alike two assets are: the average, over the moments of the first, of
/// the closest moment of the second.
pub fn best_match(a: &[Vec<f32>], b: &[Vec<f32>]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.iter().map(|x| b.iter().map(|y| cosine(x, y)).fold(0.0, f64::max)).sum::<f64>() / a.len() as f64
}

fn probe(path: &Path) -> (String, f64, u32, u32) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if let Ok(p) = crate::media_io::probe(path) {
        let kind = format!("{:?}", p.kind).to_lowercase();
        let (w, h) = p.video.as_ref().map_or((0, 0), |v| (v.width, v.height));
        return (kind, p.duration.unwrap_or(0) as f64 / 1e6, w, h);
    }
    let kind = if ["mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "mpg", "mpeg"].contains(&ext.as_str()) {
        "video"
    } else if ["wav", "mp3", "m4a", "aac", "flac", "ogg", "aif", "aiff", "opus"].contains(&ext.as_str()) {
        "audio"
    } else if ["png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "heic", "psd", "svg"].contains(&ext.as_str()) {
        "image"
    } else {
        "other"
    };
    (kind.into(), 0.0, 0, 0)
}

/// Where a file came from, as far as its place says.
fn guess_source(path: &Path) -> String {
    let s = path.display().to_string().replace('\\', "/");
    let studio = studio_paths::studio().display().to_string().replace('\\', "/");
    if s.starts_with(&format!("{studio}/Made")) || s.contains("/generated/") || s.contains("/Generated/") {
        "generated".into()
    } else if s.starts_with(&format!("{studio}/Content")) || s.contains("/studio-content/") {
        "bundled".into()
    } else if s.starts_with(&format!("{studio}/Downloads")) || s.contains("/Downloads/") {
        "downloaded".into()
    } else {
        "imported".into()
    }
}

// ------------------------------------------------------------- the words

/// Groups of words that mean nearly the same to someone searching footage.
const MEANING: &[&[&str]] = &[
    &["night", "dark", "evening", "dusk", "nighttime", "lowlight"],
    &["day", "daylight", "sunny", "bright", "daytime"],
    &["car", "vehicle", "auto", "automobile", "driving", "road"],
    &["crowd", "group", "people", "audience", "gathering"],
    &["face", "portrait", "closeup", "headshot", "person"],
    &["speech", "talking", "speaking", "interview", "dialogue", "voice"],
    &["music", "song", "track", "tune", "score"],
    &["city", "urban", "street", "downtown", "skyline"],
    &["nature", "forest", "trees", "woods", "landscape"],
    &["sea", "ocean", "beach", "water", "waves", "coast"],
    &["happy", "joy", "smile", "smiling", "laugh", "laughing"],
    &["sad", "crying", "tears", "grief", "sorrow"],
    &["fight", "action", "chase", "battle", "combat"],
    &["love", "romance", "romantic", "kiss", "couple"],
    &["house", "home", "apartment", "interior", "room", "indoor"],
    &["outdoor", "outside", "exterior"],
    &["title", "text", "caption", "lettering", "logo", "words"],
    &["wide", "establishing", "overview", "aerial", "drone"],
    &["explosion", "blast", "fire", "flames"],
    &["rain", "storm", "stormy", "thunder", "weather"],
    &["child", "kid", "children", "kids"],
    &["dog", "puppy", "pet", "animal"],
    &["food", "meal", "kitchen", "cooking", "dinner"],
    &["dance", "dancing", "song", "choreography"],
    &["wedding", "bride", "groom", "marriage"],
    &["red", "crimson", "scarlet"],
    &["blue", "azure", "navy"],
    &["green", "emerald"],
    &["yellow", "gold", "golden"],
];

fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.replace('\'', "").to_lowercase())
        .collect()
}

fn stem_eq(a: &str, b: &str) -> bool {
    a == b || (a.len() > 3 && b.len() > 3 && (a.starts_with(b) || b.starts_with(a)))
}

/// The word and the words that mean the same.
pub fn alternatives(word: &str) -> Vec<String> {
    let mut out = vec![word.to_owned()];
    for g in MEANING {
        if g.iter().any(|x| stem_eq(x, word)) {
            out.extend(g.iter().filter(|x| !stem_eq(x, word)).map(|x| (*x).to_owned()));
        }
    }
    out.dedup();
    out
}

const FILLER: &[&str] = &["a", "an", "the", "of", "in", "on", "at", "with", "and", "or", "to", "for", "shot", "clip", "footage", "video", "picture", "photo", "image", "sound", "where", "someone", "something", "is", "are", "who", "that", "show", "shows", "find", "me", "all"];

/// FTS queries to try, strictest first: every word (or a word of the same
/// meaning) must be there; then any word.
fn fts_queries(text: &str) -> Vec<(bool, String)> {
    let words: Vec<String> = words_of(text);
    let keep: Vec<&String> = {
        let k: Vec<&String> = words.iter().filter(|w| !FILLER.contains(&w.as_str())).collect();
        if k.is_empty() { words.iter().collect() } else { k }
    };
    if keep.is_empty() {
        return Vec::new();
    }
    let quote = |w: &str| format!("\"{}\"*", w.replace('"', ""));
    let exact: Vec<String> = keep.iter().map(|w| quote(w)).collect();
    let meaning: Vec<String> = keep.iter().map(|w| format!("({})", alternatives(w).iter().map(|a| quote(a)).collect::<Vec<_>>().join(" OR "))).collect();
    let loose: Vec<String> = keep.iter().flat_map(|w| alternatives(w)).map(|a| quote(&a)).collect();
    let mut out = vec![(true, exact.join(" AND "))];
    if meaning != exact {
        out.push((true, meaning.join(" AND ")));
    }
    if keep.len() > 1 {
        out.push((false, loose.join(" OR ")));
    }
    out
}

/// Words describing what a picture looks like, for the "seen" index.
pub fn describe_look(mean_rgb: [f32; 3], brightness: f32, contrast: f32, faces: usize, biggest_face: f32, lettering: bool, aspect: f32) -> String {
    let mut w: Vec<&str> = Vec::new();
    if brightness < 0.18 {
        w.extend(["dark", "night", "low key"]);
    } else if brightness > 0.7 {
        w.extend(["bright", "day", "high key"]);
    }
    if contrast > 0.35 {
        w.push("contrasty");
    } else if contrast < 0.1 {
        w.push("flat");
    }
    let [r, g, b] = mean_rgb;
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    if mx - mn < 0.06 {
        w.push(if brightness < 0.15 { "black" } else if brightness > 0.8 { "white" } else { "grey" });
        w.push("neutral");
    } else {
        // Hue name from the strongest channel mix.
        let name = if r >= g && r >= b {
            if g > b * 1.6 { if g > r * 0.8 { "yellow" } else if g > r * 0.45 { "orange" } else { "red" } } else if b > g * 1.3 { "pink magenta" } else { "red" }
        } else if g >= r && g >= b {
            if b > r * 1.3 { "teal cyan" } else { "green" }
        } else if r > g * 1.3 {
            "purple violet"
        } else {
            "blue"
        };
        w.push(name);
        w.push(if r > b * 1.25 { "warm" } else if b > r * 1.25 { "cool" } else { "balanced" });
    }
    match faces {
        0 => w.extend(["no people", "empty"]),
        1 => w.extend(["person", "face", "single"]),
        2 => w.extend(["two people", "face", "pair", "couple"]),
        _ => w.extend(["group", "crowd", "people", "faces"]),
    }
    if biggest_face > 0.35 {
        w.extend(["close-up", "closeup", "portrait"]);
    } else if biggest_face > 0.0 && biggest_face < 0.12 {
        w.extend(["wide", "far"]);
    }
    if lettering {
        w.extend(["text", "title", "caption", "lettering", "on-screen text"]);
    }
    w.push(if aspect > 1.2 { "landscape" } else if aspect < 0.85 { "vertical portrait" } else { "square" });
    w.join(" ")
}

/// Which of these files are really there (paths).
pub fn existing(paths: &[String]) -> BTreeSet<String> {
    paths.iter().filter(|p| Path::new(p).is_file()).cloned().collect()
}

pub fn meta_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-assets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn payload(seed: u8, n: usize) -> Vec<u8> {
        (0..n).map(|i| (i as u32).wrapping_mul(seed as u32 + 3).wrapping_add(i as u32 / 7) as u8).collect()
    }

    #[test]
    fn meaning_groups() {
        assert!(alternatives("night").contains(&"dark".to_string()));
        assert!(alternatives("crowds").iter().any(|a| a == "audience"));
        assert_eq!(alternatives("zebra"), vec!["zebra".to_string()]);
        let q = fts_queries("find a shot of a dark car");
        assert!(q[0].1.contains("\"dark\"*") && !q[0].1.contains("\"shot\""));
    }

    #[test]
    fn organise_and_search() {
        let d = scratch("org");
        let s = Store::open(&d.join("a.db")).unwrap();
        let a = s.add_file(&file(&d, "beach_sunset.mp4", &payload(1, 5000)), None).unwrap();
        let b = s.add_file(&file(&d, "city_walk.mp4", &payload(2, 5000)), None).unwrap();
        s.label(a, "tag", &["b-roll".into()]).unwrap();
        s.label(a, "place", &["Goa".into()]).unwrap();
        s.label(b, "person", &["Asha".into()]).unwrap();
        s.set_rating(a, 4).unwrap();
        s.set_favourite(b, true).unwrap();
        s.record(a, "speech", &[(2.0, 4.0, "the waves are beautiful tonight".into())]).unwrap();
        let hit = |q: Value| s.find(&Query::from_json(&q)).unwrap();
        assert_eq!(hit(json!({"query": "beach"}))[0].asset.id, a);
        assert_eq!(hit(json!({"query": "goa"}))[0].asset.id, a);
        assert_eq!(hit(json!({"person": "asha"}))[0].asset.id, b);
        assert_eq!(hit(json!({"min_rating": 4})).len(), 1);
        assert_eq!(hit(json!({"favourite": true}))[0].asset.id, b);
        // What is said: found by the words, with when.
        let w = hit(json!({"query": "waves beautiful"}));
        assert_eq!(w[0].asset.id, a);
        assert_eq!(w[0].why[0].0, "speech");
        assert_eq!(w[0].why[0].1, 2.0);
        // By meaning: "sea" reaches "waves"? Not by group; "ocean" reaches "sea" words only.
        assert!(hit(json!({"query": "zebra"})).is_empty());
        // Meaning groups: a night shot is found by "dark".
        s.record(b, "seen", &[(0.0, 5.0, "night street".into())]).unwrap();
        assert_eq!(hit(json!({"query": "dark"}))[0].asset.id, b);
        // Bins and smart bins.
        s.bin_add("Footage/Day 1", &[a]).unwrap();
        assert_eq!(hit(json!({"bin": "Footage"}))[0].asset.id, a);
        s.bin_create("Best", Some(&json!({"min_rating": 4}))).unwrap();
        assert_eq!(hit(json!({"bin": "Best"}))[0].asset.id, a);
        s.set_rating(b, 5).unwrap();
        assert_eq!(hit(json!({"bin": "Best"})).len(), 2, "a smart bin follows its search");
        assert!(s.bin_add("Best", &[a]).is_err());
        let bins = s.bins().unwrap();
        assert!(bins.iter().any(|b| b.1 == "Footage/Day 1" && b.3 == 1));
        s.bin_delete("Footage").unwrap();
        assert!(s.bin_find("Footage/Day 1").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn duplicates_missing_and_relink() {
        let d = scratch("dup");
        let s = Store::open(&d.join("a.db")).unwrap();
        let bytes = payload(9, 300_000);
        let a = s.add_file(&file(&d, "shoot/a.mov", &bytes), None).unwrap();
        let b = s.add_file(&file(&d, "backup/copy of a.mov", &bytes), None).unwrap();
        let mut other = bytes.clone();
        other[150_000] ^= 0xFF;
        let c = s.add_file(&file(&d, "shoot/c.mov", &other), None).unwrap();
        let groups = s.duplicates(false).unwrap();
        assert_eq!(groups.len(), 1);
        let ids: Vec<i64> = groups[0].iter().map(|x| x.id).collect();
        assert!(ids.contains(&a) && ids.contains(&b) && !ids.contains(&c));
        // c is moved to somewhere else; the library finds it by content.
        std::fs::create_dir_all(d.join("new place/deep")).unwrap();
        std::fs::rename(d.join("shoot/c.mov"), d.join("new place/deep/renamed.mov")).unwrap();
        let gone = s.missing(None).unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].id, c);
        let found = s.find_moved(&gone, &[d.join("new place")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].2, "exact");
        s.moved(c, &found[0].1).unwrap();
        assert!(s.missing(None).unwrap().is_empty());
        assert_eq!(s.get(c).unwrap().name, "renamed.mov");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn similar_pictures_and_people() {
        let d = scratch("sim");
        let s = Store::open(&d.join("a.db")).unwrap();
        let ids: Vec<i64> = (0..3).map(|k| s.add_file(&file(&d, &format!("p{k}.png"), &payload(k as u8 + 20, 900)), None).unwrap()).collect();
        s.record_sigs(ids[0], &[(0.0, vec![1.0, 0.0, 0.2])]).unwrap();
        s.record_sigs(ids[1], &[(0.0, vec![0.9, 0.1, 0.2])]).unwrap();
        s.record_sigs(ids[2], &[(0.0, vec![0.0, 1.0, 0.0])]).unwrap();
        let like = s.similar(ids[0], 5).unwrap();
        assert_eq!(like[0].0.id, ids[1]);
        assert!(like[0].1 > like[1].1);
        // Faces: the same descriptor lands in one group; naming it labels every asset.
        s.record_faces(ids[0], &[(0.0, vec![1.0, 0.1, 0.0])]).unwrap();
        s.record_faces(ids[1], &[(1.0, vec![0.99, 0.12, 0.0]), (2.0, vec![0.0, 0.2, 1.0])]).unwrap();
        let people = s.people().unwrap();
        assert_eq!(people.len(), 2);
        assert_eq!(people[0].3, 2, "the first person is in two assets");
        s.name_person(people[0].0, "Meera").unwrap();
        let found = s.find(&Query::from_json(&json!({"person": "meera"}))).unwrap();
        assert_eq!(found.len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn describing_looks() {
        let w = describe_look([0.8, 0.2, 0.1], 0.5, 0.2, 1, 0.4, false, 1.78);
        assert!(w.contains("red") && w.contains("close-up") && w.contains("warm"));
        let n = describe_look([0.02, 0.02, 0.04], 0.05, 0.1, 0, 0.0, true, 1.78);
        assert!(n.contains("dark") && n.contains("title"));
    }
}
