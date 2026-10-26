// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The asset lock: every committed file that is not source text (pictures,
//! video, sound, fonts, plugins, models, fixtures, test data) is listed in
//! `assets.lock` with its size and SHA-256, so nothing lands, changes or
//! disappears by accident.
//!
//! * `cargo xtask assets-lock` writes the lock from the files git tracks.
//! * `cargo xtask assets-check` fails when a tracked asset is missing from the
//!   lock, differs from it, or a locked one is gone, and when any tracked file
//!   is over [`MAX_BYTES`] without being locked.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Name of the lock file at the repository root.
pub const LOCK: &str = "assets.lock";

/// A tracked file larger than this must be in the lock.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Extensions of source text: these are reviewed as code, not locked.
const SOURCE: &[&str] = &["rs", "wgsl", "toml", "lock", "md", "yml", "yaml", "ftl", "inc", "gitignore", "gitattributes", "desktop", "entitlements", "conf", "sh", "py"];

/// One locked file: size in bytes and SHA-256 (hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub size: u64,
    pub sha256: String,
}

/// Whether `path` (repository-relative) is an asset the lock covers.
pub fn is_asset(path: &str) -> bool {
    if path == LOCK {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        // No extension (LICENSE, NOTICE, LICENSE-MIT): text that is reviewed as such.
        None => false,
        Some(("", _)) => false,
        Some((_, ext)) => !SOURCE.contains(&ext.to_ascii_lowercase().as_str()),
    }
}

fn tracked(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git").args(["ls-files", "-z"]).current_dir(root).output().map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files failed".into());
    }
    Ok(out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

fn entry(file: &Path) -> Result<Entry, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    Ok(Entry { size: bytes.len() as u64, sha256: digest.as_ref().iter().map(|b| format!("{b:02x}")).collect() })
}

/// The assets git tracks now, by path.
fn current(root: &Path) -> Result<BTreeMap<String, Entry>, String> {
    let mut map = BTreeMap::new();
    for path in tracked(root)?.into_iter().filter(|p| is_asset(p)) {
        let file = root.join(&path);
        // A file deleted on disk but still tracked shows up as missing in the check.
        if file.is_file() {
            map.insert(path, entry(&file)?);
        }
    }
    Ok(map)
}

/// The lock file as text.
pub fn render(entries: &BTreeMap<String, Entry>) -> String {
    let mut s = String::from("# Every committed asset (pictures, video, sound, fonts, plugins, models,\n# fixtures, test data) with its size and SHA-256. `cargo xtask assets-check`\n# fails on any asset not listed here; after adding or changing one on\n# purpose, run `cargo xtask assets-lock` and commit this file with it.\n");
    for (path, e) in entries {
        s.push_str(&format!("{} {} {}\n", e.sha256, e.size, path));
    }
    s
}

/// Reads lock text back.
pub fn parse(text: &str) -> Result<BTreeMap<String, Entry>, String> {
    let mut map = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let (Some(sha), Some(size), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!("{LOCK} line {}: expected `sha256 size path`", n + 1));
        };
        let size = size.parse().map_err(|_| format!("{LOCK} line {}: bad size", n + 1))?;
        map.insert(path.to_owned(), Entry { size, sha256: sha.to_owned() });
    }
    Ok(map)
}

/// What differs between the lock and the tracked files.
pub fn compare(locked: &BTreeMap<String, Entry>, now: &BTreeMap<String, Entry>) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, e) in now {
        match locked.get(path) {
            None => problems.push(format!("new, not in {LOCK}: {path} ({} bytes)", e.size)),
            Some(l) if l != e => problems.push(format!("changed since locked: {path} ({} -> {} bytes)", l.size, e.size)),
            Some(_) => {}
        }
    }
    for path in locked.keys().filter(|p| !now.contains_key(*p)) {
        problems.push(format!("locked but gone: {path}"));
    }
    problems
}

/// `cargo xtask assets-lock`.
pub fn lock_cmd(root: &Path) -> Result<String, String> {
    let now = current(root)?;
    std::fs::write(root.join(LOCK), render(&now)).map_err(|e| format!("{LOCK}: {e}"))?;
    Ok(format!("{LOCK}: {} assets, {} bytes in all", now.len(), now.values().map(|e| e.size).sum::<u64>()))
}

/// `cargo xtask assets-check`.
pub fn check_cmd(root: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(root.join(LOCK)).map_err(|_| format!("no {LOCK}: run `cargo xtask assets-lock` once and commit it"))?;
    let locked = parse(&text)?;
    let now = current(root)?;
    let mut problems = compare(&locked, &now);
    for path in tracked(root)?.into_iter().filter(|p| !is_asset(p) && p != LOCK) {
        let size = std::fs::metadata(root.join(&path)).map(|m| m.len()).unwrap_or(0);
        if size > MAX_BYTES {
            problems.push(format!("too large for a source file: {path} ({size} bytes)"));
        }
    }
    if problems.is_empty() {
        Ok(format!("{LOCK}: all {} assets match", now.len()))
    } else {
        Err(format!("{} asset problem(s):\n  {}\nIf these are on purpose, run `cargo xtask assets-lock` and commit {LOCK} with them.", problems.len(), problems.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_not_an_asset_and_media_is() {
        assert!(!is_asset("app/src/main.rs"));
        assert!(!is_asset("crates/picture/render/src/x.wgsl"));
        assert!(!is_asset("README.md"));
        assert!(!is_asset("LICENSE"));
        assert!(!is_asset(".gitignore"));
        assert!(!is_asset(LOCK));
        assert!(is_asset("testdata/media/a.mp4"));
        assert!(is_asset("app/assets/icons/x.svg"));
        assert!(is_asset("plugins/dist/duotone.wasm"));
        assert!(is_asset("testdata/images/jpeg.txt"));
        assert!(is_asset("app/tests/fixtures/search_media/bing_images.html"));
    }

    #[test]
    fn the_lock_reads_back_and_finds_every_kind_of_change() {
        let e = |s: u64, h: &str| Entry { size: s, sha256: h.into() };
        let mut locked = BTreeMap::new();
        locked.insert("a.png".to_owned(), e(3, "aa"));
        locked.insert("b.mp4".to_owned(), e(5, "bb"));
        locked.insert("c.ttf".to_owned(), e(7, "cc"));
        assert_eq!(parse(&render(&locked)).unwrap(), locked);
        let mut now = locked.clone();
        now.insert("d.jpg".to_owned(), e(1, "dd"));
        now.insert("b.mp4".to_owned(), e(6, "bx"));
        now.remove("c.ttf");
        let p = compare(&locked, &now);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p.iter().any(|x| x.starts_with("new") && x.contains("d.jpg")));
        assert!(p.iter().any(|x| x.starts_with("changed") && x.contains("b.mp4")));
        assert!(p.iter().any(|x| x.starts_with("locked but gone") && x.contains("c.ttf")));
        assert!(compare(&locked, &locked).is_empty());
    }
}
