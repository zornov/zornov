// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Numbered image sequences.
//!
//! A sequence is written as a pattern with `#` for the frame number's
//! digits (`/shots/A001.####.exr`); `%04d` and a single `@` per digit are
//! accepted on input too. [`Sequence::from_file`] finds the sequence a
//! frame belongs to (the last run of digits in the file name), scanning
//! its folder for the frame range and gaps.

use std::path::{Path, PathBuf};

/// A frame sequence on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub dir: PathBuf,
    /// File name before the frame number.
    pub prefix: String,
    /// File name after it (extension included).
    pub suffix: String,
    /// Digits of zero padding (the number may be longer).
    pub padding: usize,
    pub first: i64,
    pub last: i64,
    /// Frame numbers inside first..=last with no file.
    pub missing: Vec<i64>,
}

impl Sequence {
    /// The `#` pattern path.
    pub fn pattern(&self) -> PathBuf {
        self.dir.join(format!("{}{}{}", self.prefix, "#".repeat(self.padding), self.suffix))
    }

    /// The file of frame number `n`.
    pub fn frame(&self, n: i64) -> PathBuf {
        let num = if n < 0 { format!("-{:0w$}", -n, w = self.padding) } else { format!("{n:0w$}", w = self.padding) };
        self.dir.join(format!("{}{num}{}", self.prefix, self.suffix))
    }

    /// Number of frames from first to last.
    pub fn len(&self) -> usize {
        (self.last - self.first + 1).max(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `path` is written as a pattern (`#`, `%0Nd` or `@`).
    pub fn is_pattern(path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name.contains('#') || name.contains("%0") || name.contains("%d") || name.contains('@')
    }

    /// Opens a sequence from its pattern path, scanning the folder.
    pub fn from_pattern(path: &Path) -> Result<Sequence, String> {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let name = path.file_name().and_then(|n| n.to_str()).ok_or("bad sequence path")?;
        let (prefix, padding, suffix) = split_pattern(name).ok_or_else(|| format!("\"{name}\" is not a frame pattern (use #### for the frame number)"))?;
        scan(&dir, &prefix, &suffix, padding)
    }

    /// The sequence a numbered frame belongs to.
    pub fn from_file(path: &Path) -> Result<Sequence, String> {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let name = path.file_name().and_then(|n| n.to_str()).ok_or("bad file name")?;
        let (prefix, digits, suffix) = split_number(name).ok_or_else(|| format!("\"{name}\" has no frame number"))?;
        // A zero-padded number fixes the width; otherwise the width comes
        // from what is on disk.
        let padding = if digits.starts_with('0') { digits.len() } else { 0 };
        scan(&dir, &prefix, &suffix, padding)
    }
}

/// `name.####.ext` → (prefix, padding, suffix).
fn split_pattern(name: &str) -> Option<(String, usize, String)> {
    if let Some(start) = name.find('#') {
        let n = name[start..].chars().take_while(|c| *c == '#').count();
        return Some((name[..start].to_string(), n, name[start + n..].to_string()));
    }
    if let Some(start) = name.find('@') {
        let n = name[start..].chars().take_while(|c| *c == '@').count();
        return Some((name[..start].to_string(), n, name[start + n..].to_string()));
    }
    let start = name.find('%')?;
    let rest = &name[start + 1..];
    let end = rest.find('d')?;
    let width: usize = rest[..end].trim_start_matches('0').parse().unwrap_or(1);
    Some((name[..start].to_string(), width.max(1), rest[end + 1..].to_string()))
}

/// The last run of digits in a file name (before the extension).
fn split_number(name: &str) -> Option<(String, String, String)> {
    let bytes = name.as_bytes();
    let stem_end = name.rfind('.').unwrap_or(name.len());
    let mut end = stem_end;
    while end > 0 && !bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    Some((name[..start].to_string(), name[start..end].to_string(), name[end..].to_string()))
}

fn scan(dir: &Path, prefix: &str, suffix: &str, padding: usize) -> Result<Sequence, String> {
    let mut frames = Vec::new();
    let mut widths = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let n = e.file_name();
        let Some(n) = n.to_str() else { continue };
        let Some(mid) = n.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix)) else { continue };
        let digits = mid.strip_prefix('-').unwrap_or(mid);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if padding > 0 && digits.len() < padding {
            continue;
        }
        if let Ok(v) = mid.parse::<i64>() {
            frames.push(v);
            widths.push((digits.len(), digits.starts_with('0')));
        }
    }
    // Width 0: every name has the same number of digits, or the shortest
    // are zero-padded, so that is the padding; else unpadded.
    let padding = if padding > 0 {
        padding
    } else {
        let min = widths.iter().map(|w| w.0).min().unwrap_or(1);
        let same = widths.iter().all(|w| w.0 == min);
        if same || widths.iter().any(|w| w.0 == min && w.1) { min } else { 1 }
    };
    if frames.is_empty() {
        return Err(format!("no frames of {}{}{} in {}", prefix, "#".repeat(padding), suffix, dir.display()));
    }
    frames.sort_unstable();
    frames.dedup();
    let (first, last) = (frames[0], *frames.last().unwrap_or(&frames[0]));
    let mut missing = Vec::new();
    let mut it = frames.iter().peekable();
    for n in first..=last {
        if it.peek() == Some(&&n) {
            it.next();
        } else {
            missing.push(n);
        }
    }
    Ok(Sequence { dir: dir.to_path_buf(), prefix: prefix.to_string(), suffix: suffix.to_string(), padding, first, last, missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ranges_and_gaps() {
        let dir = std::env::temp_dir().join(format!("studio-seq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in [1001, 1002, 1003, 1005] {
            std::fs::write(dir.join(format!("shot_A.{n:04}.exr")), b"x").unwrap();
        }
        std::fs::write(dir.join("shot_A.thumb.exr"), b"x").unwrap();
        std::fs::write(dir.join("other.1001.exr"), b"x").unwrap();
        let s = Sequence::from_file(&dir.join("shot_A.1002.exr")).unwrap();
        assert_eq!((s.first, s.last, s.padding), (1001, 1005, 4));
        assert_eq!(s.missing, vec![1004]);
        assert_eq!(s.len(), 5);
        assert_eq!(s.frame(1003), dir.join("shot_A.1003.exr"));
        assert_eq!(s.pattern(), dir.join("shot_A.####.exr"));
        let p = Sequence::from_pattern(&dir.join("shot_A.%04d.exr")).unwrap();
        assert_eq!(p, s);
        assert!(Sequence::is_pattern(&s.pattern()));
        assert!(!Sequence::is_pattern(&s.frame(1001)));
        assert!(Sequence::from_pattern(&dir.join("none.####.exr")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn number_split() {
        assert_eq!(split_number("A001_C002.0042.dpx"), Some(("A001_C002.".into(), "0042".into(), ".dpx".into())));
        assert_eq!(split_pattern("x_%05d.tif"), Some(("x_".into(), 5, ".tif".into())));
        assert_eq!(split_number("noframes.exr"), None);
    }
}
