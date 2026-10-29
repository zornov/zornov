// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `ProjectFolder` is the only code that knows where things live in a
//! project. This test fails when another file joins one of the layout's
//! names onto a path again, so the knowledge stays in one place.
//!
//! A hit that is not a project path (a folder in the person's own home, an
//! application-wide folder, a test's scratch folder) goes in [`ALLOWED`]
//! with the reason.

use std::path::{Path, PathBuf};

/// Names that only `crates/project/model/src/file/` may join.
const NAMES: &[&str] = &[
    "Models", "Worlds", "Generated", "Takes", "Proxies", "backups", "Stills", "Cut-outs", "Slow motion", "Cameras", "Clean plates", "Parts", "Imported", "Characters", "LUTs", "Recordings",
    "Voice-over", "Simulations", "agent-frames", "agent-usage.jsonl", "agent-policy.json", "agent-notes.md", "accepted.json", "workspace-3d.json", ".last-backup.json", ".project.lock", ".project.open",
    "Sessions", "Dubs", "ADR", "production.json", "Placed",
];

/// (file suffix, name, why). A listed pair may join that name.
const ALLOWED: &[(&str, &str, &str)] = &[
    ("app/src/agent/care.rs", "Pictures", "the person's own Pictures folder"),
    ("crates/project/paths/src/lib.rs", "Models", "the application-wide folders"),
    ("crates/ai/ai/src/store.rs", "Models", "the application-wide model store"),
    ("app/src/my_models.rs", "Models", "the library of kept models"),
    ("app/src/agent/picture.rs", "Placed", "beside the pictures of a session with no project"),
    ("app/src/safety.rs", "agent-notes.md", "a name inside a backup snapshot, which is not a project"),
    ("app/src/rights.rs", "Generated", "Studio's own folder, not a project's"),
    ("app/src/services/grade_stills.rs", "Stills", "the cache folder used when there is no project"),
    ("app/src/services/cutout.rs", "Cut-outs", "Studio's folder used when there is no project"),
    ("app/src/services/slowmo.rs", "Slow motion", "Studio's folder used when there is no project"),
    ("app/src/services/vfx.rs", "Cameras", "Studio's folder used when there is no project"),
    ("app/src/services/character.rs", "Parts", "Studio's folder used when there is no project"),
    ("app/src/services/consolidate.rs", "Proxies", "inside the folder the person chose to consolidate into"),
    ("app/src/services/consolidate.rs", "LUTs", "inside the folder the person chose to consolidate into"),
    ("app/src/services/genmedia.rs", "Takes", "inside the Generated folder the caller passes in"),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !matches!(name.as_str(), "target" | "tests" | "fixtures" | ".git") {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
            out.push(p);
        }
    }
}

#[test]
fn only_project_folder_names_layout_folders() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let repo = std::fs::canonicalize(repo).unwrap();
    let mut files = Vec::new();
    rust_files(&repo.join("app/src"), &mut files);
    rust_files(&repo.join("crates"), &mut files);
    let mut hits = Vec::new();
    for f in files {
        let rel = f.strip_prefix(&repo).unwrap().to_string_lossy().replace('\\', "/");
        if rel.starts_with("crates/project/model/src/file/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        // Unit tests sit at the bottom of a file, after `#[cfg(test)]`.
        let code = text.split("#[cfg(test)]").next().unwrap_or("");
        for (n, line) in code.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            for name in NAMES {
                if line.contains(&format!("join(\"{name}\")")) && !ALLOWED.iter().any(|(file, n, _)| rel.ends_with(file) && n == name) {
                    hits.push(format!("{rel}:{}: join(\"{name}\")", n + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "these join a project folder's name themselves; use ProjectFolder:\n{}", hits.join("\n"));
}
