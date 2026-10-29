// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The stills gallery: a grade kept with a picture of the shot it was made
//! on, to compare against and to apply to other shots. Stills live in the
//! project folder's `Stills` folder (or Studio's cache for an unsaved
//! project) as `<name>.json` (the grade: parameters, LUT, nodes) and
//! `<name>.jpg` (the graded frame).

use crate::doc::Doc;
use serde_json::{Value, json};
use std::path::PathBuf;
use studio_model as m;

/// Where the stills of this project are.
pub fn folder(doc: &Doc) -> PathBuf {
    match &doc.folder {
        Some(f) => f.generated_dir(m::file::MadeKind::Stills),
        None => studio_paths::cache().join("Stills"),
    }
}

fn clean(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Still".into() } else { s }
}

/// One still.
#[derive(Clone, Debug, PartialEq)]
pub struct Still {
    pub name: String,
    pub grade: m::Grade,
    pub picture: Option<PathBuf>,
    pub from_clip: Option<u64>,
    pub at: Option<f64>,
}

/// Keeps `grade` (and the JPEG `picture`, when given) as a still.
pub fn save(doc: &Doc, name: &str, grade: &m::Grade, picture: Option<&[u8]>, from_clip: Option<u64>, at: Option<f64>) -> Result<Still, String> {
    let dir = folder(doc);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = clean(name);
    let json_path = dir.join(format!("{name}.json"));
    let doc_json = json!({ "name": name, "grade": grade, "clip": from_clip, "at": at });
    std::fs::write(&json_path, serde_json::to_string_pretty(&doc_json).unwrap_or_default()).map_err(|e| e.to_string())?;
    let pic = match picture {
        Some(bytes) => {
            let p = dir.join(format!("{name}.jpg"));
            std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
            Some(p)
        }
        None => None,
    };
    Ok(Still { name, grade: grade.clone(), picture: pic, from_clip, at })
}

/// Every still, by name.
pub fn list(doc: &Doc) -> Vec<Still> {
    let Ok(rd) = std::fs::read_dir(folder(doc)) else { return Vec::new() };
    let mut out: Vec<Still> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            let grade: m::Grade = serde_json::from_value(v.get("grade")?.clone()).ok()?;
            let name = v.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| e.path().file_stem().unwrap_or_default().to_string_lossy().into_owned());
            let jpg = e.path().with_extension("jpg");
            Some(Still { name, grade, picture: jpg.exists().then_some(jpg), from_clip: v.get("clip").and_then(Value::as_u64), at: v.get("at").and_then(Value::as_f64) })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A still by name (case-insensitive).
pub fn get(doc: &Doc, name: &str) -> Option<Still> {
    list(doc).into_iter().find(|s| s.name.eq_ignore_ascii_case(name.trim()) || s.name.eq_ignore_ascii_case(&clean(name)))
}

/// Removes a still.
pub fn remove(doc: &Doc, name: &str) -> Result<(), String> {
    let s = get(doc, name).ok_or_else(|| format!("no still \"{name}\""))?;
    let dir = folder(doc);
    let _ = std::fs::remove_file(dir.join(format!("{}.jpg", s.name)));
    std::fs::remove_file(dir.join(format!("{}.json", s.name))).map_err(|e| e.to_string())
}

/// The grade a still puts on a clip: the still's own grade, but a shot
/// match made for the still's shot (`match_*`) is not carried over.
pub fn grade_for(still: &Still) -> m::Grade {
    let mut g = still.grade.clone();
    g.params.retain(|k, _| !k.starts_with("match_"));
    g.enabled = true;
    g
}
