// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the person likes: preferences, corrections and rejections the AI
//! editor remembers, per project and across projects, and a style guide
//! per project. Every request starts with them (see [`brief`]), so the AI
//! does not make the same mistake twice.
//!
//! * `<project>/AI/taste.json`: this project's notes.
//! * `<project>/AI/Style guide.md`: the project's style guide, written by
//!   the person or the AI (`taste` with `style_guide`).
//! * `Studio/AI/taste.json` (the Studio folder): notes for every project,
//!   and corrections from all projects, newest first.
//!
//! Notes come from the AI (`taste` with `remember`), from corrections the
//! person sends while a request runs, and from steps they skip or send
//! back.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_model as m;

/// What a note is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Like,
    Dislike,
    /// A rule to follow ("captions always bottom centre").
    Rule,
    /// Something the person corrected mid-run.
    Correction,
    /// A step or change they skipped or sent back.
    Rejected,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Like => "like",
            Kind::Dislike => "dislike",
            Kind::Rule => "rule",
            Kind::Correction => "correction",
            Kind::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Kind {
        match s.trim().to_lowercase().as_str() {
            "like" | "likes" | "love" | "prefer" | "style" => Kind::Like,
            "dislike" | "hate" | "avoid" | "never" => Kind::Dislike,
            "correction" | "corrected" => Kind::Correction,
            "rejected" | "reject" | "skipped" => Kind::Rejected,
            _ => Kind::Rule,
        }
    }
}

/// Notes kept at most per file (oldest corrections go first).
const KEEP: usize = 200;

fn project_file(project: &Path) -> PathBuf {
    m::file::ProjectFolder::new(project).state(m::file::State::Taste)
}

pub fn style_guide_path(project: &Path) -> PathBuf {
    m::file::ProjectFolder::new(project).state(m::file::State::StyleGuide)
}

thread_local! {
    /// Tests keep their cross-project notes in a folder of their own.
    static GLOBAL: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The AI editor's own folder in the Studio folder (tests: a scratch one,
/// never the person's).
pub fn ai_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("studio-ai-test-{}", std::process::id()));
    }
    studio_paths::studio().join("AI")
}

fn global_file() -> PathBuf {
    GLOBAL.with(|g| g.borrow().clone()).unwrap_or_else(ai_dir).join("taste.json")
}

fn read(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["notes"].as_array().cloned()).unwrap_or_default()
}

fn write(path: &Path, notes: &[Value]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&json!({"notes": notes})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn project_name(project: Option<&Path>) -> String {
    project.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Remembers `text`. `everywhere`: for every project (else this one; with
/// no project open, everywhere). Corrections and rejections also go to the
/// cross-project list, tagged with the project.
pub fn remember(project: Option<&Path>, kind: Kind, text: &str, everywhere: bool) -> Result<u64, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("nothing to remember".into());
    }
    let id = next_id();
    let note = json!({"id": id, "kind": kind.name(), "text": text, "when": now(), "project": project_name(project)});
    let mut targets = Vec::new();
    match project {
        Some(p) if !everywhere => targets.push(project_file(p)),
        _ => targets.push(global_file()),
    }
    if matches!(kind, Kind::Correction | Kind::Rejected) && project.is_some() && !everywhere {
        targets.push(global_file());
    }
    for path in targets {
        let mut notes = read(&path);
        // The same words again: just newer.
        notes.retain(|n| n["text"].as_str() != Some(text) || n["kind"].as_str() != Some(kind.name()));
        notes.push(note.clone());
        if notes.len() > KEEP {
            let drop = notes.len() - KEEP;
            notes.drain(..drop);
        }
        write(&path, &notes)?;
    }
    Ok(id)
}

/// A new note id: the time in microseconds, never the same twice.
fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
    let mut last = LAST.load(Ordering::SeqCst);
    loop {
        let id = t.max(last + 1);
        match LAST.compare_exchange(last, id, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return id,
            Err(now) => last = now,
        }
    }
}

/// A correction the person sent mid-run.
pub fn corrected(project: Option<&Path>, text: &str) {
    let _ = remember(project, Kind::Correction, text, false);
}

/// Something the person skipped or sent back.
pub fn rejected(project: Option<&Path>, what: &str) {
    let _ = remember(project, Kind::Rejected, what, false);
}

/// Forgets a note by id, wherever it is.
pub fn forget(project: Option<&Path>, id: u64) -> bool {
    let mut gone = false;
    for path in project.map(project_file).into_iter().chain([global_file()]) {
        let mut notes = read(&path);
        let n = notes.len();
        notes.retain(|v| v["id"].as_u64() != Some(id));
        if notes.len() != n {
            gone = true;
            let _ = write(&path, &notes);
        }
    }
    gone
}

pub fn style_guide(project: Option<&Path>) -> Option<String> {
    project.and_then(|p| std::fs::read_to_string(style_guide_path(p)).ok()).map(|g| super::outside::clean_saved(&g)).filter(|s| !s.trim().is_empty())
}

fn line(n: &Value) -> String {
    let from = match n["project"].as_str() {
        Some(p) if !p.is_empty() => format!(" (in {p})"),
        _ => String::new(),
    };
    format!("[{}] {}: {}{from}", n["id"], n["kind"].as_str().unwrap_or("rule"), n["text"].as_str().unwrap_or(""))
}

/// What to read before working, short: this project's notes, the notes
/// for every project, recent corrections from other projects, the style
/// guide. Empty when there is nothing.
pub fn brief(project: Option<&Path>) -> String {
    let mut out = Vec::new();
    let mut budget = 3_500usize;
    let mut push = |out: &mut Vec<String>, s: String| {
        if s.len() <= budget {
            budget -= s.len();
            out.push(s);
        }
    };
    let here = project.map(|p| read(&project_file(p))).unwrap_or_default();
    let name = project_name(project);
    let global = read(&global_file());
    let (mine, others): (Vec<&Value>, Vec<&Value>) = global.iter().partition(|n| !matches!(n["kind"].as_str(), Some("correction" | "rejected")));
    for n in here.iter().rev().take(25) {
        push(&mut out, format!("- {}", line(n)));
    }
    for n in mine.iter().rev().take(20) {
        push(&mut out, format!("- {} (every project)", line(n)));
    }
    for n in others.iter().rev().filter(|n| n["project"].as_str() != Some(&name)).take(6) {
        push(&mut out, format!("- {}", line(n)));
    }
    let guide = style_guide(project).map(|g| super::brain::one_line(&g, 2_500));
    if out.is_empty() && guide.is_none() {
        return String::new();
    }
    let mut s = String::from("[The person's taste, remembered from earlier work: follow it; never repeat what they rejected or corrected");
    if !out.is_empty() {
        s.push_str(":\n");
        s.push_str(&super::outside::clean_saved(&out.join("\n")));
    }
    if let Some(g) = guide {
        s.push_str(&format!("\nThis project's style guide: {g}"));
    }
    s.push(']');
    s
}

/// The `taste` tool.
pub fn tool(project: Option<&Path>, a: &Value) -> Result<String, String> {
    if let Some(text) = a.get("remember").and_then(Value::as_str) {
        super::outside::check_saved(text)?;
        let kind = Kind::parse(a.get("kind").and_then(Value::as_str).unwrap_or("rule"));
        let everywhere = a.get("everywhere").and_then(Value::as_bool).unwrap_or(false);
        let id = remember(project, kind, text, everywhere)?;
        let scope = if everywhere || project.is_none() { "for every project" } else { "for this project" };
        return Ok(format!("Remembered {scope} (note {id})."));
    }
    if let Some(id) = a.get("forget").and_then(Value::as_u64) {
        return Ok(if forget(project, id) { format!("Forgot note {id}.") } else { format!("There is no note {id}.") });
    }
    if let Some(g) = a.get("style_guide").and_then(Value::as_str) {
        super::outside::check_saved(g)?;
        let p = project.ok_or("Save the project first: the style guide is kept in its folder.")?;
        let path = style_guide_path(p);
        let text = if a.get("append").and_then(Value::as_bool).unwrap_or(false) {
            format!("{}\n{}", style_guide(Some(p)).unwrap_or_default().trim_end(), g.trim())
        } else {
            g.trim().to_owned()
        };
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, format!("{}\n", text.trim())).map_err(|e| e.to_string())?;
        return Ok(format!("The style guide is saved ({} words).", text.split_whitespace().count()));
    }
    let here = project.map(|p| read(&project_file(p))).unwrap_or_default();
    let global = read(&global_file());
    let mut out = Vec::new();
    out.push(format!("This project ({}): {}", project_name(project), if here.is_empty() { "nothing yet".to_owned() } else { String::new() }));
    out.extend(here.iter().rev().map(|n| format!("  {}", line(n))));
    out.push(format!("Every project: {}", if global.is_empty() { "nothing yet" } else { "" }));
    // This project's corrections are listed above already.
    let name = project_name(project);
    out.extend(global.iter().rev().filter(|n| !(matches!(n["kind"].as_str(), Some("correction" | "rejected")) && n["project"].as_str() == Some(&name))).take(60).map(|n| format!("  {}", line(n))));
    out.push(match style_guide(project) {
        Some(g) => format!("Style guide:\n{g}"),
        None => "No style guide yet: write one with `taste` `style_guide` once the look and voice are settled.".into(),
    });
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taste_is_remembered_per_project_and_everywhere() {
        let home = std::env::temp_dir().join(format!("studio-taste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        GLOBAL.with(|g| *g.borrow_mut() = Some(home.join("Studio/AI")));
        let a = home.join("Projects/Beach");
        let b = home.join("Projects/City");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_eq!(brief(Some(&a)), "", "nothing yet");
        tool(Some(&a), &json!({"remember": "warm, filmic grade", "kind": "like"})).unwrap();
        tool(Some(&a), &json!({"remember": "no whoosh transitions, ever", "kind": "dislike", "everywhere": true})).unwrap();
        corrected(Some(&a), "slower cuts in the intro");
        let ids: Vec<String> = tool(Some(&a), &json!({})).unwrap().lines().filter_map(|l| l.trim().strip_prefix('[').and_then(|l| l.split(']').next()).map(str::to_owned)).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "every note has its own id: {ids:?}");
        let ba = brief(Some(&a));
        assert!(ba.contains("warm, filmic grade") && ba.contains("no whoosh") && ba.contains("slower cuts"), "{ba}");
        let bb = brief(Some(&b));
        assert!(!bb.contains("warm, filmic"), "a project's own likes stay with it: {bb}");
        assert!(bb.contains("no whoosh"), "every-project notes travel");
        assert!(bb.contains("slower cuts") && bb.contains("in Beach"), "corrections from other projects are shown, tagged: {bb}");
        tool(Some(&a), &json!({"style_guide": "Titles: Manrope, white, bottom left."})).unwrap();
        assert!(brief(Some(&a)).contains("Manrope"));
        let all = tool(Some(&a), &json!({})).unwrap();
        let id: u64 = all.lines().find(|l| l.contains("warm, filmic")).and_then(|l| l.trim().strip_prefix('[')).and_then(|l| l.split(']').next()).and_then(|n| n.parse().ok()).unwrap();
        assert!(tool(Some(&a), &json!({"forget": id})).unwrap().starts_with("Forgot"));
        assert!(!brief(Some(&a)).contains("warm, filmic"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
