// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The library: one SQLite database (`library.db` in Studio's cache) that
//! knows every project on this Mac and what is in it, searchable at once:
//! titles and on-screen text, what narration says and when, markers,
//! media files and where they are used, the AI editors' notes, saved moves
//! and designs. Full-text search (FTS5, with word stems) answers "where did
//! anyone say render", "every project using trailer.mp4", "the move I made
//! for a hook" in one query, across projects, while the window, the
//! command line and agents all use it at once (WAL).
//!
//! It is refreshed incrementally: a project is read again only when its
//! file changed. Analysers (shots, transcripts) can add what they find with
//! [`record_lines`] and [`record_shots`].

use crate::sqlite::{Db, V};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use studio_model as m;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS seen (path TEXT PRIMARY KEY, modified INTEGER);
CREATE TABLE IF NOT EXISTS projects (path TEXT PRIMARY KEY, name TEXT, duration REAL);
CREATE TABLE IF NOT EXISTS media (path TEXT PRIMARY KEY, name TEXT, kind TEXT, duration REAL, width INTEGER, height INTEGER);
CREATE TABLE IF NOT EXISTS uses (project TEXT, media TEXT, PRIMARY KEY (project, media));
CREATE TABLE IF NOT EXISTS shots (media TEXT, t0 REAL, t1 REAL, facts TEXT);
CREATE INDEX IF NOT EXISTS shots_media ON shots (media, t0);
CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(kind UNINDEXED, owner UNINDEXED, place UNINDEXED, body, tokenize = 'porter unicode61');
";

fn path() -> PathBuf {
    std::env::var_os("STUDIO_LIBRARY_DB").map(PathBuf::from).unwrap_or_else(|| studio_paths::cache().join("library.db"))
}

fn db() -> Result<&'static Db, String> {
    static DB: OnceLock<Result<Db, String>> = OnceLock::new();
    DB.get_or_init(|| {
        let d = Db::open(&path())?;
        d.exec(SCHEMA)?;
        Ok(d)
    })
    .as_ref()
    .map_err(|e| format!("the library could not open: {e}"))
}

/// Runs `f` as one write transaction. `BEGIN IMMEDIATE` takes the write
/// lock up front, so another process writing makes this wait (the busy
/// timeout) rather than fail with "database is locked" when a read turns
/// into a write; and one transaction at a time in this process (they share
/// one connection). A failure rolls back, never leaving it open.
fn tx<T>(d: &Db, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    d.exec("BEGIN IMMEDIATE")?;
    let r = f();
    match &r {
        Ok(_) => {
            if let Err(e) = d.exec("COMMIT") {
                let _ = d.exec("ROLLBACK");
                return Err(e);
            }
        }
        Err(_) => {
            let _ = d.exec("ROLLBACK");
        }
    }
    r
}

fn modified(p: &Path) -> i64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

/// Whether `file` changed since it was last read (and notes it as read).
fn changed(d: &Db, file: &Path) -> Result<bool, String> {
    let now = modified(file);
    let key = file.display().to_string();
    let was = d.query("SELECT modified FROM seen WHERE path = ?", &[key.clone().into()])?;
    if was.first().is_some_and(|r| r[0].int() == now) {
        return Ok(false);
    }
    d.run("INSERT OR REPLACE INTO seen VALUES (?, ?)", &[key.into(), now.into()])?;
    Ok(true)
}

fn doc(d: &Db, kind: &str, owner: &str, place: &str, body: &str) -> Result<(), String> {
    if body.trim().is_empty() {
        return Ok(());
    }
    d.run("INSERT INTO docs (kind, owner, place, body) VALUES (?, ?, ?, ?)", &[kind.into(), owner.into(), place.into(), body.into()])
}

/// The project folders Studio knows: the Projects folder, Home's list, and
/// `current` (the one being edited, wherever it is).
fn project_folders(current: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(studio_paths::studio().join("Projects")).map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("project.json").is_file()).collect()).unwrap_or_default();
    if let Ok(list) = std::fs::read_to_string(studio_paths::studio().join("recent-projects.txt")) {
        for l in list.lines().map(PathBuf::from) {
            if l.join("project.json").is_file() && !crate::files::is_temporary(&l) && !out.contains(&l) {
                out.push(l);
            }
        }
    }
    if let Some(c) = current.filter(|c| c.join("project.json").is_file()) {
        if !out.iter().any(|p| p == c) {
            out.push(c.to_path_buf());
        }
    }
    out
}

/// Reads again every project, note and saved move that changed (and
/// `current`, the project being edited).
pub fn refresh(current: Option<&Path>) -> Result<usize, String> {
    let d = db()?;
    let mut n = 0;
    tx(d, || {
        for folder in project_folders(current) {
            if changed(d, &folder.join("project.json"))? {
                index_project(d, &folder)?;
                n += 1;
            }
            let notes = folder.join("agent-notes.md");
            if notes.is_file() && changed(d, &notes)? {
                let owner = folder.display().to_string();
                d.run("DELETE FROM docs WHERE kind = 'note' AND owner = ?", &[owner.clone().into()])?;
                for para in std::fs::read_to_string(&notes).unwrap_or_default().split("\n\n") {
                    doc(d, "note", &owner, "agent-notes.md", para)?;
                }
                n += 1;
            }
        }
        for kind in ["moves", "designs"] {
            let dir = studio_paths::studio().join("Made").join(kind);
            for f in std::fs::read_dir(&dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default() {
                if changed(d, &f)? {
                    let v: Value = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
                    let owner = f.display().to_string();
                    d.run("DELETE FROM docs WHERE owner = ?", &[owner.clone().into()])?;
                    let body = format!("{} {}", v["name"].as_str().unwrap_or(""), v["description"].as_str().unwrap_or(""));
                    doc(d, &kind[..kind.len() - 1], &owner, v["name"].as_str().unwrap_or(""), &body)?;
                    n += 1;
                }
            }
        }
        Ok(())
    })?;
    Ok(n)
}

/// One project's names, text, markers, narration and media.
fn index_project(d: &Db, folder: &Path) -> Result<(), String> {
    let owner = folder.display().to_string();
    for t in ["DELETE FROM docs WHERE owner = ? AND kind != 'note'", "DELETE FROM uses WHERE project = ?", "DELETE FROM projects WHERE path = ?"] {
        d.run(t, &[owner.clone().into()])?;
    }
    let Ok(file) = m::file::ProjectFolder::new(folder).load() else { return Ok(()) };
    let view = crate::bridge::view_of(&file.project, 0);
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    d.run("INSERT INTO projects VALUES (?, ?, ?)", &[owner.clone().into(), name.clone().into(), (view.duration() as f64 / 1e6).into()])?;
    doc(d, "project", &owner, "", &format!("{name} {}", view.name))?;
    let at = |t: i64| format!("{:.2}s", t as f64 / 1e6);
    for c in &view.clips {
        if !c.text.trim().is_empty() {
            doc(d, "title", &owner, &format!("clip {} at {}", c.id, at(c.start)), &c.text)?;
        }
    }
    for mk in &view.markers {
        doc(d, "marker", &owner, &at(mk.time), &mk.note)?;
    }
    for mi in &view.media {
        let Some(p) = &mi.path else { continue };
        let ps = p.display().to_string();
        d.run("INSERT OR REPLACE INTO media VALUES (?, ?, ?, ?, ?, ?)", &[ps.clone().into(), mi.name.clone().into(), format!("{:?}", mi.kind).to_lowercase().into(), (mi.dur as f64 / 1e6).into(), (mi.width as i64).into(), (mi.height as i64).into()])?;
        d.run("INSERT OR IGNORE INTO uses VALUES (?, ?)", &[owner.clone().into(), ps.clone().into()])?;
        doc(d, "media", &owner, &ps, &mi.name)?;
        // Narration Studio made keeps its words beside it.
        if let Ok(b) = std::fs::read(p.with_extension("words.json")) {
            if let Ok(words) = serde_json::from_slice::<Vec<Value>>(&b) {
                let text: Vec<&str> = words.iter().filter_map(|w| w["text"].as_str()).collect();
                let t0 = words.first().and_then(|w| w["start"].as_f64()).unwrap_or(0.0);
                // Where on this timeline the line plays.
                let when = view.clips.iter().find(|c| c.media == Some(mi.id)).map(|c| at(c.start + (t0 * 1e6) as i64 - c.src_in)).unwrap_or_default();
                doc(d, "spoken", &owner, &format!("{} {when}", mi.name), &text.join(" "))?;
            }
        }
    }
    Ok(())
}

/// Adds what an analyser found a media file says: (from, to, text).
pub fn record_lines(media: &Path, lines: &[(f64, f64, String)]) -> Result<(), String> {
    let d = db()?;
    let owner = media.display().to_string();
    tx(d, || {
        d.run("DELETE FROM docs WHERE kind = 'line' AND owner = ?", &[owner.clone().into()])?;
        for (a, b, t) in lines {
            doc(d, "line", &owner, &format!("{a:.2}-{b:.2}s"), t)?;
        }
        Ok(())
    })
}

/// Adds the shots an analyser found in a media file, with their facts.
pub fn record_shots(media: &Path, shots: &[(f64, f64, Value)]) -> Result<(), String> {
    let d = db()?;
    let owner = media.display().to_string();
    tx(d, || {
        d.run("DELETE FROM shots WHERE media = ?", &[owner.clone().into()])?;
        for (a, b, f) in shots {
            d.run("INSERT INTO shots VALUES (?, ?, ?, ?)", &[owner.clone().into(), (*a).into(), (*b).into(), f.to_string().into()])?;
        }
        Ok(())
    })
}

/// Plain words to an FTS5 query: every word, each as a prefix.
fn fts_query(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `search`: everything the library knows that mentions every word of
/// `query`, best first; `kinds` limits it (title, spoken, line, marker,
/// media, note, move, design, project).
pub fn search(query: &str, kinds: &[String], limit: usize, current: Option<&Path>) -> Result<Value, String> {
    // A refresh that cannot finish (another process holding the library
    // past the busy timeout) is no reason to fail: search what is indexed.
    let _ = refresh(current);
    let d = db()?;
    let q = fts_query(query);
    // No words: everything of the kinds asked, newest first.
    let listing = q.is_empty();
    let mut sql = if listing {
        "SELECT kind, owner, place, substr(body, 1, 160) FROM docs WHERE 1".to_owned()
    } else {
        "SELECT kind, owner, place, snippet(docs, 3, '[', ']', '…', 14) FROM docs WHERE docs MATCH ?".to_owned()
    };
    let mut args: Vec<V> = if listing { Vec::new() } else { vec![q.into()] };
    if !kinds.is_empty() {
        sql.push_str(&format!(" AND kind IN ({})", vec!["?"; kinds.len()].join(", ")));
        args.extend(kinds.iter().map(|k| V::from(k.as_str())));
    }
    let total = if listing {
        let count_sql = sql.replacen("SELECT kind, owner, place, substr(body, 1, 160)", "SELECT count(*)", 1);
        d.query(&count_sql, &args).ok().and_then(|r| r.first().map(|r| r[0].int()))
    } else {
        None
    };
    sql.push_str(if listing { " ORDER BY rowid DESC LIMIT ?" } else { " ORDER BY bm25(docs) LIMIT ?" });
    args.push((limit.clamp(1, if listing { 500 } else { 100 }) as i64).into());
    let rows = d.query(&sql, &args)?;
    let name = |owner: &str| Path::new(owner).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| owner.to_owned());
    let hits: Vec<Value> = rows.iter().map(|r| json!({"kind": r[0].text(), "in": name(&r[1].text()), "where": r[2].text(), "text": r[3].text()})).collect();
    let count = |t: &str| d.query(&crate::i18n::tf(format!("SELECT count(*) FROM {t}")), &[]).ok().and_then(|r| r.first().map(|r| r[0].int())).unwrap_or(0);
    let mut out = json!({"hits": hits, "library": {"projects": count("projects"), "media": count("media"), "entries": count("docs")}});
    if let Some(n) = total {
        out["of"] = json!(n);
    }
    Ok(out)
}

/// The projects that use a media file (by name or path, part of it).
pub fn used_in(media: &str) -> Result<Vec<String>, String> {
    let _ = refresh(None);
    let d = db()?;
    let rows = d.query("SELECT DISTINCT u.project FROM uses u JOIN media m ON m.path = u.media WHERE m.name LIKE ? OR m.path LIKE ?", &[format!("%{media}%").into(), format!("%{media}%").into()])?;
    Ok(rows.iter().map(|r| r[0].text()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_words_become_prefixes() {
        assert_eq!(fts_query("render race!"), "\"render\"* \"race\"*");
        assert_eq!(fts_query("  "), "");
    }
}
