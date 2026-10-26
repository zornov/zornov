// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Review packages: what goes to a director, a client or a producer who
//! watches without the project.
//!
//! A package is a folder: a small proxy render, the notes so far (as
//! Studio notes, CSV and Markdown), which version it shows, and a notes
//! template. The reviewer watches the proxy and writes notes in any of the
//! forms [`super::comments::Store::import`] reads; the folder (or just
//! the notes file) comes back and is read in with [`read_back`]. Writing a
//! package only writes files; sending it is the person's choice.

use super::comments::{Filter, Store};
use super::{now_ms, write_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use studio_model as m;

/// `review.json`: what the package shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub format: String,
    pub project: String,
    pub sequence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    pub fingerprint: String,
    pub by: String,
    pub created: u64,
    pub duration_seconds: f64,
    pub fps: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    pub notes: usize,
}

/// A fresh folder for a package inside `parent`.
pub fn folder_in(parent: &Path, project: &str) -> PathBuf {
    let base: String = project.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    let base = format!("{} review", base.trim());
    let mut p = parent.join(&base);
    let mut n = 2;
    while p.exists() {
        p = parent.join(format!("{base} {n}"));
        n += 1;
    }
    p
}

/// Writes everything but the proxy into `dir`; the caller renders the
/// proxy to [`proxy_path`] and passes its name.
pub fn write(dir: &Path, p: &m::Project, notes: &Store, version: Option<(&str, &str)>, by: &str, proxy: Option<&str>) -> Result<Info, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let seq = p.active();
    let duration = seq.clips.iter().map(|c| c.start + c.duration).max().unwrap_or(0);
    let mut kept = notes.clone();
    kept.comments.retain(|c| !c.deleted);
    let info = Info {
        format: "studio-review".into(),
        project: seq.name.clone(),
        sequence: seq.name.clone(),
        version: version.map(|v| v.0.to_owned()),
        version_name: version.map(|v| v.1.to_owned()),
        fingerprint: super::fingerprint(p),
        by: by.to_owned(),
        created: now_ms(),
        duration_seconds: duration as f64 / 1e6,
        fps: p.settings.frame_rate.as_f64(),
        proxy: proxy.map(str::to_owned),
        notes: kept.comments.len(),
    };
    write_json(&dir.join("review.json"), &info)?;
    write_json(&dir.join("notes.json"), &kept)?;
    let put = |name: &str, text: &str| std::fs::write(dir.join(name), text).map_err(|e| format!("could not write {name}: {e}"));
    put("notes.csv", &kept.export(p, Filter::All, "csv")?)?;
    put("notes.md", &kept.export(p, Filter::All, "markdown")?)?;
    put("notes-template.csv", "timecode,note,author\n00:00:00:00,,\n")?;
    let version_line = match &info.version_name {
        Some(n) => format!("Version: {n} ({})\n", info.version.clone().unwrap_or_default()),
        None => String::new(),
    };
    put(
        "README.txt",
        &format!(
            "Review: {}\n{version_line}Length: {:.1} s at {} fps\n\n{}Write your notes in notes-template.csv (one per line: timecode as hh:mm:ss:ff, the note,\nyour name), or as plain lines like \"00:01:23:12 the music comes in late\", and send\nthe file back. Notes made in Zornov are in notes.json; replies there are kept.\n",
            info.sequence,
            info.duration_seconds,
            info.fps,
            if proxy.is_some() { "Watch proxy.mp4 (timecode starts at 00:00:00:00).\n" } else { "" }
        ),
    )?;
    Ok(info)
}

/// Where the proxy goes in a package.
pub fn proxy_path(dir: &Path) -> PathBuf {
    dir.join("proxy.mp4")
}

/// The notes text a package (or a single notes file) brings back: the
/// reviewer's filled template or notes file first, then the package's
/// Studio notes.
pub fn read_back(path: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    if path.is_file() {
        return Ok(vec![(path.to_path_buf(), std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?)]);
    }
    if !path.is_dir() {
        return Err(format!("{} does not exist", path.display()));
    }
    let mut out = Vec::new();
    for name in ["notes.json", "notes-template.csv", "notes.txt", "notes back.csv"] {
        let f = path.join(name);
        if let Ok(text) = std::fs::read_to_string(&f) {
            // The empty template is not a note.
            if name == "notes-template.csv" && text.lines().skip(1).all(|l| l.trim_matches([',', ' ']).trim_start_matches("00:00:00:00").trim_matches(',').is_empty()) {
                continue;
            }
            out.push((f, text));
        }
    }
    // Any other CSV or text file the reviewer added.
    if let Ok(rd) = std::fs::read_dir(path) {
        let mut extra: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "csv" || x == "txt" || x == "edl")).filter(|p| !matches!(p.file_name().and_then(|n| n.to_str()), Some("notes.csv" | "notes-template.csv" | "notes.txt" | "README.txt" | "notes back.csv"))).collect();
        extra.sort();
        for f in extra {
            if let Ok(text) = std::fs::read_to_string(&f) {
                out.push((f, text));
            }
        }
    }
    if out.is_empty() {
        return Err(format!("no notes in {}: expected notes.json, a filled notes-template.csv, or a notes file", path.display()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::{comments, testing};

    #[test]
    fn a_package_goes_out_and_notes_come_back() {
        let dir = testing::Scratch::new("package");
        let p = testing::project();
        let mut notes = Store::default();
        notes.add(&p, "Ash", "first cut, be honest", comments::Pin { at: 0, ..Default::default() }, None).unwrap();
        let pkg = folder_in(&dir.0, "My Film");
        let info = write(&pkg, &p, &notes, Some(("v2", "Rough cut")), "Ash", None).unwrap();
        assert_eq!((info.notes, info.duration_seconds), (1, 9.0));
        assert!(pkg.join("README.txt").exists() && pkg.join("notes.csv").exists());
        assert_eq!(folder_in(&dir.0, "My Film").file_name().unwrap().to_str(), Some("My Film review 2"));
        // Nothing written by the reviewer yet: only our own notes come back.
        assert_eq!(read_back(&pkg).unwrap().len(), 1);

        // The reviewer fills the template and replies in notes.json.
        std::fs::write(pkg.join("notes-template.csv"), "timecode,note,author\n00:00:04:00,hold on the green longer,Director\n").unwrap();
        let mut back: Store = serde_json::from_str(&std::fs::read_to_string(pkg.join("notes.json")).unwrap()).unwrap();
        back.reply("ash-1", "Director", "it is good").unwrap();
        std::fs::write(pkg.join("notes.json"), serde_json::to_string(&back).unwrap()).unwrap();

        let mut mine = notes.clone();
        let mut n = 0;
        for (_, text) in read_back(&pkg).unwrap() {
            n += mine.import(&p, &text, "Reviewer").unwrap();
        }
        assert_eq!(n, 2, "the reply and the new note");
        assert_eq!(mine.get("ash-1").unwrap().replies.len(), 1);
        let new = mine.list(&p, Filter::Open).into_iter().find(|c| c.author == "Director").unwrap();
        assert_eq!(new.at, 4_000_000);
    }
}
