// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Undo history kept as an append-only journal (`history.jsonl`).
//!
//! The history used to be written whole on every save and autosave (a few
//! megabytes for a long session). Here each undo step is written once, as
//! one line, the first time it is seen; every save then only appends the
//! new steps and a small line naming which steps make up the undo and redo
//! stacks now:
//!
//! ```text
//! {"s":0,"step":{…}}
//! {"s":1,"step":{…}}
//! {"state":{"project_hash":…,"revision":…,"undo":[0,1],"redo":[]}}
//! ```
//!
//! A crash mid-write loses at most the last line; the state before it
//! still names only steps written before it. When dead steps (undone and
//! replaced) outweigh live ones the journal is rewritten compactly.

use studio_model as m;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(serde::Serialize, serde::Deserialize)]
struct State {
    project_hash: u64,
    revision: u64,
    undo: Vec<u64>,
    redo: Vec<u64>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Line {
    Step { s: u64, step: m::Step },
    State { state: State },
}

/// What is already in a journal: each step's text hash to its number.
#[derive(Default)]
struct Written {
    numbers: HashMap<u64, u64>,
    next: u64,
    bytes: u64,
}

fn written() -> &'static Mutex<HashMap<PathBuf, Written>> {
    static W: OnceLock<Mutex<HashMap<PathBuf, Written>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Reads what a journal holds: its steps by number and its states, oldest
/// first.
fn read(path: &Path) -> Option<(HashMap<u64, m::Step>, Vec<State>, Written)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut steps = HashMap::new();
    let mut states = Vec::new();
    let mut w = Written { bytes: text.len() as u64, ..Default::default() };
    for line in text.lines() {
        // A torn last line (a crash mid-write) is simply not there.
        let Ok(l) = serde_json::from_str::<Line>(line) else { continue };
        match l {
            Line::Step { s, step } => {
                if let Ok(t) = serde_json::to_string(&step) {
                    w.numbers.insert(fnv(&t), s);
                }
                w.next = w.next.max(s + 1);
                steps.insert(s, step);
            }
            Line::State { state: st } => states.push(st),
        }
    }
    Some((steps, states, w))
}

/// The history saved for the project file whose hash is `project_hash`,
/// if the journal belongs to it and is whole.
pub fn load(path: &Path, project_hash: u64) -> Option<m::SavedHistory> {
    let (mut steps, states, w) = read(path)?;
    // The newest state for this project file: a stray line after it (from
    // another copy of the project, or a damaged write) does not lose it.
    let st = states.into_iter().rev().find(|s| s.project_hash == project_hash)?;
    let mut take = |ns: &[u64]| -> Option<Vec<m::Step>> { ns.iter().map(|n| steps.get(n).cloned()).collect() };
    let saved = m::SavedHistory { undo: take(&st.undo)?, redo: take(&st.redo)?, revision: st.revision };
    steps.clear();
    written().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), w);
    Some(saved)
}

/// Records `saved` for the project file with hash `project_hash`:
/// appends the steps not written yet and the new state.
pub fn keep(path: &Path, project_hash: u64, saved: &m::SavedHistory) -> std::io::Result<()> {
    let mut all = written().lock().unwrap_or_else(|e| e.into_inner());
    if !all.contains_key(path) {
        let w = read(path).map(|(_, _, w)| w).unwrap_or_default();
        all.insert(path.to_path_buf(), w);
    }
    let w = all.get_mut(path).expect("just put");
    let mut out = String::new();
    let mut live_bytes = 0u64;
    let mut number = |step: &m::Step, out: &mut String| -> std::io::Result<u64> {
        let t = serde_json::to_string(step).map_err(std::io::Error::other)?;
        live_bytes += t.len() as u64 + 16;
        let h = fnv(&t);
        if let Some(n) = w.numbers.get(&h) {
            return Ok(*n);
        }
        let n = w.next;
        w.next += 1;
        w.numbers.insert(h, n);
        out.push_str(&format!("{{\"s\":{n},\"step\":{t}}}\n"));
        Ok(n)
    };
    let undo = saved.undo.iter().map(|s| number(s, &mut out)).collect::<std::io::Result<Vec<_>>>()?;
    let redo = saved.redo.iter().map(|s| number(s, &mut out)).collect::<std::io::Result<Vec<_>>>()?;
    let state = serde_json::to_string(&serde_json::json!({"state": State { project_hash, revision: saved.revision, undo, redo }})).map_err(std::io::Error::other)?;
    out.push_str(&state);
    out.push('\n');
    // Mostly dead steps: write only what is live, from scratch.
    if w.bytes + out.len() as u64 > 2 * live_bytes + (1 << 20) {
        all.remove(path);
        drop(all);
        return rewrite(path, project_hash, saved);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(out.as_bytes())?;
    w.bytes += out.len() as u64;
    Ok(())
}

/// The journal written afresh with only the live steps.
fn rewrite(path: &Path, project_hash: u64, saved: &m::SavedHistory) -> std::io::Result<()> {
    let mut out = String::new();
    let mut w = Written::default();
    let mut put = |step: &m::Step| -> std::io::Result<u64> {
        let t = serde_json::to_string(step).map_err(std::io::Error::other)?;
        let h = fnv(&t);
        if let Some(n) = w.numbers.get(&h) {
            return Ok(*n);
        }
        let n = w.next;
        w.next += 1;
        w.numbers.insert(h, n);
        out.push_str(&format!("{{\"s\":{n},\"step\":{t}}}\n"));
        Ok(n)
    };
    let undo = saved.undo.iter().map(&mut put).collect::<std::io::Result<Vec<_>>>()?;
    let redo = saved.redo.iter().map(&mut put).collect::<std::io::Result<Vec<_>>>()?;
    out.push_str(&serde_json::to_string(&serde_json::json!({"state": State { project_hash, revision: saved.revision, undo, redo }})).map_err(std::io::Error::other)?);
    out.push('\n');
    w.bytes = out.len() as u64;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    m::file::write_atomic(path, out.as_bytes())?;
    written().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), w);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(label: &str, n: i64) -> m::Step {
        m::Step { label: label.into(), inverse: m::Edit::RemoveMarker { seq: m::SequenceId(1), id: m::MarkerId(n as u64) }, author: m::Author::Person }
    }

    #[test]
    fn saves_append_and_read_back() {
        let dir = std::env::temp_dir().join(format!("studio-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.jsonl");
        let mut h = m::SavedHistory { undo: vec![step("a", 1), step("b", 2)], redo: vec![], revision: 2 };
        keep(&path, 7, &h).unwrap();
        let first = std::fs::metadata(&path).unwrap().len();
        // One more step: the file grows by that step and a state line only.
        h.undo.push(step("c", 3));
        h.revision = 3;
        keep(&path, 8, &h).unwrap();
        let second = std::fs::metadata(&path).unwrap().len();
        assert!(second - first < first, "appended, not rewritten ({first} then {second})");
        // Undo one: the step moves to redo, nothing new is written but state.
        let c = h.undo.pop().unwrap();
        h.redo.push(c);
        keep(&path, 9, &h).unwrap();
        written().lock().unwrap().clear();
        assert_eq!(load(&path, 9), Some(h.clone()));
        assert_eq!(load(&path, 12345), None, "a history for another project file is not used");
        // The project file as it was at an earlier save (a restored backup)
        // gets the history it had then.
        assert_eq!(load(&path, 8).map(|s| s.undo.len()), Some(3));
        // A torn last line: the state before it still holds.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"s\":99,\"step\":{\"lab").unwrap();
        written().lock().unwrap().clear();
        assert_eq!(load(&path, 9), Some(h.clone()));
        // A state for another project file after it: this one's still holds.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"\n{\"state\":{\"project_hash\":1,\"revision\":5,\"undo\":[99999],\"redo\":[]}}\n").unwrap();
        written().lock().unwrap().clear();
        assert_eq!(load(&path, 9), Some(h));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
