// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Review notes: a thread pinned to a moment of the edit, optionally to a
//! clip (the note then follows the clip when it moves) and to a region of
//! the frame. Threads take replies and are resolved or reopened; nothing
//! is ever lost: a deleted note stays as a tombstone so a merge with an
//! older copy does not bring it back.
//!
//! Kept in `review/comments.json`, outside the undo history: a note is a
//! conversation about the edit, not part of it.

use super::{clock, now_ms, parse_time, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use studio_model as m;

/// Where a project's notes are kept.
pub fn file(root: &Path) -> PathBuf {
    root.join("review").join("comments.json")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Open,
    Resolved,
}

/// An answer in a thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub author: String,
    pub text: String,
    pub created: u64,
}

/// One review note and its thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    /// `author-n`, e.g. `sam-3`: readable, and unique across people.
    pub id: String,
    /// Sequence time the note is about.
    pub at: m::Micros,
    /// End of the stretch the note is about, when it is about a stretch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<m::Micros>,
    /// The sequence (`None`: the one active when the note was made).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// The clip the note is about; the note then follows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<u64>,
    /// `at` minus the clip's start when the note was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_offset: Option<m::Micros>,
    /// A box on the frame, canvas pixels from the centre, y down:
    /// `[x0, y0, x1, y1]` (the units of the agent's tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<[f32; 4]>,
    pub author: String,
    pub text: String,
    pub created: u64,
    /// Last change to the note or its thread (merges keep the newer).
    pub updated: u64,
    #[serde(default)]
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
    /// The version the note was made against, when one was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Deleted: kept so merges do not resurrect it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

impl Comment {
    /// Where the note sits now: its clip's position when it has a clip
    /// that is still there (and whether that clip is gone).
    pub fn time_in(&self, p: &m::Project) -> (m::Micros, bool) {
        let (Some(clip), Some(off)) = (self.clip, self.clip_offset) else { return (self.at, false) };
        let seq = self.sequence.map(m::SequenceId).unwrap_or(p.active_sequence);
        match p.sequence(seq).and_then(|s| s.clip(m::ClipId(clip))) {
            Some(c) => (c.start + off.clamp(0, c.duration.max(1) - 1), false),
            None => (self.at, true),
        }
    }

    pub fn is_open(&self) -> bool {
        !self.deleted && self.status == Status::Open
    }
}

/// What a note is pinned to, when making one.
#[derive(Clone, Debug, Default)]
pub struct Pin {
    pub at: m::Micros,
    pub until: Option<m::Micros>,
    pub clip: Option<u64>,
    pub region: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default = "format_name")]
    pub format: String,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

fn format_name() -> String {
    "studio-comments".into()
}

/// Which notes to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    Open,
    Resolved,
    All,
}

impl Filter {
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_lowercase().as_str() {
            "" | "open" | "todo" | "unresolved" => Filter::Open,
            "resolved" | "done" | "closed" => Filter::Resolved,
            "all" | "any" => Filter::All,
            o => return Err(format!("status \"{o}\": open, resolved or all")),
        })
    }
    fn keeps(self, c: &Comment) -> bool {
        !c.deleted
            && match self {
                Filter::Open => c.status == Status::Open,
                Filter::Resolved => c.status == Status::Resolved,
                Filter::All => true,
            }
    }
}

fn slug(s: &str) -> String {
    // The person, not the "(AI)" working for them.
    let s = s.split('(').next().unwrap_or(s);
    let w: String = s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).take(12).collect();
    if w.is_empty() { "note".into() } else { w }
}

impl Store {
    pub fn load(root: &Path) -> Result<Self, String> {
        Ok(read_json::<Store>(&file(root))?.unwrap_or_default())
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let mut s = self.clone();
        s.format = format_name();
        write_json(&file(root), &s)
    }

    fn next_id(&self, author: &str) -> String {
        let base = slug(author);
        let n = self.comments.iter().flat_map(|c| std::iter::once(&c.id).chain(c.replies.iter().map(|r| &r.id))).filter_map(|id| id.strip_prefix(&format!("{base}-")).and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0);
        format!("{base}-{}", n + 1)
    }

    pub fn get(&self, id: &str) -> Option<&Comment> {
        self.comments.iter().find(|c| c.id == id && !c.deleted)
    }

    fn get_mut(&mut self, id: &str) -> Result<&mut Comment, String> {
        let known: Vec<String> = self.comments.iter().filter(|c| !c.deleted).map(|c| c.id.clone()).take(12).collect();
        self.comments.iter_mut().find(|c| c.id == id && !c.deleted).ok_or_else(|| format!("no note \"{id}\"; notes: {}", known.join(", ")))
    }

    /// Adds a note; returns its id.
    pub fn add(&mut self, p: &m::Project, author: &str, text: &str, pin: Pin, version: Option<String>) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a note needs text".into());
        }
        let seq = p.active_sequence;
        let clip_offset = match pin.clip {
            Some(c) => Some(pin.at - p.sequence(seq).and_then(|s| s.clip(m::ClipId(c))).ok_or_else(|| format!("no clip {c} in the active sequence"))?.start),
            None => None,
        };
        let now = now_ms();
        let id = self.next_id(author);
        self.comments.push(Comment {
            id: id.clone(),
            at: pin.at.max(0),
            until: pin.until.filter(|u| *u > pin.at),
            sequence: Some(seq.0),
            clip: pin.clip,
            clip_offset,
            region: pin.region,
            author: author.to_owned(),
            text: text.to_owned(),
            created: now,
            updated: now,
            status: Status::Open,
            resolved_by: None,
            replies: Vec::new(),
            version,
            deleted: false,
        });
        Ok(id)
    }

    pub fn reply(&mut self, id: &str, author: &str, text: &str) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("a reply needs text".into());
        }
        let rid = self.next_id(author);
        let c = self.get_mut(id)?;
        let now = now_ms();
        c.replies.push(Reply { id: rid.clone(), author: author.to_owned(), text: text.trim().to_owned(), created: now });
        c.updated = now;
        Ok(rid)
    }

    /// Resolves a note, with an optional closing reply (what was done).
    pub fn resolve(&mut self, id: &str, who: &str, note: Option<&str>) -> Result<(), String> {
        if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
            self.reply(id, who, n)?;
        }
        let c = self.get_mut(id)?;
        c.status = Status::Resolved;
        c.resolved_by = Some(who.to_owned());
        c.updated = now_ms();
        Ok(())
    }

    pub fn reopen(&mut self, id: &str) -> Result<(), String> {
        let c = self.get_mut(id)?;
        c.status = Status::Open;
        c.resolved_by = None;
        c.updated = now_ms();
        Ok(())
    }

    pub fn edit_text(&mut self, id: &str, text: &str) -> Result<(), String> {
        let c = self.get_mut(id)?;
        c.text = text.trim().to_owned();
        c.updated = now_ms();
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let c = self.get_mut(id)?;
        c.deleted = true;
        c.updated = now_ms();
        Ok(())
    }

    /// The notes `filter` keeps, in time order (where they sit now).
    pub fn list(&self, p: &m::Project, filter: Filter) -> Vec<&Comment> {
        let mut v: Vec<&Comment> = self.comments.iter().filter(|c| filter.keeps(c)).collect();
        v.sort_by_key(|c| (c.time_in(p).0, c.created));
        v
    }

    pub fn open_count(&self) -> usize {
        self.comments.iter().filter(|c| c.is_open()).count()
    }

    /// Takes in notes from another copy: new ones are added, ones both
    /// have keep the newer state and every reply. Returns how many notes
    /// were added or changed.
    pub fn merge(&mut self, other: &Store) -> usize {
        let mut n = 0;
        for o in &other.comments {
            match self.comments.iter_mut().find(|c| c.id == o.id) {
                // Same id, different note (two people with one name).
                Some(c) if c.created != o.created || c.author != o.author => {
                    if !self.comments.iter().any(|c| c.created == o.created && c.author == o.author && c.text == o.text) {
                        let mut o = o.clone();
                        o.id = format!("{}-{}", o.id, o.created % 10_000);
                        if !self.comments.iter().any(|c| c.id == o.id) {
                            self.comments.push(o);
                            n += 1;
                        }
                    }
                }
                Some(c) => {
                    let mut replies = c.replies.clone();
                    for r in &o.replies {
                        if !replies.iter().any(|x| x.id == r.id && x.created == r.created) {
                            replies.push(r.clone());
                        }
                    }
                    replies.sort_by_key(|r| r.created);
                    let before = c.clone();
                    if o.updated > c.updated {
                        *c = o.clone();
                    }
                    c.replies = replies;
                    if *c != before {
                        n += 1;
                    }
                }
                None => {
                    self.comments.push(o.clone());
                    n += 1;
                }
            }
        }
        n
    }

    /// The notes as a list people read or other tools take in.
    pub fn export(&self, p: &m::Project, filter: Filter, format: &str) -> Result<String, String> {
        let fps = p.settings.frame_rate.as_f64();
        let list = self.list(p, filter);
        let tc = |t: m::Micros| timecode(t, fps);
        Ok(match format.trim().to_lowercase().as_str() {
            "csv" => {
                let mut s = String::from("id,timecode,seconds,end,clip,status,author,note,replies\n");
                for c in list {
                    let (t, _) = c.time_in(p);
                    let replies = c.replies.iter().map(|r| format!("{}: {}", r.author, r.text)).collect::<Vec<_>>().join(" | ");
                    s.push_str(&[c.id.clone(), tc(t), format!("{:.3}", t as f64 / 1e6), c.until.map(|u| format!("{:.3}", u as f64 / 1e6)).unwrap_or_default(), c.clip.map(|x| x.to_string()).unwrap_or_default(), status_name(c.status).into(), c.author.clone(), c.text.clone(), replies].iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
                    s.push('\n');
                }
                s
            }
            "md" | "markdown" | "text" | "txt" => {
                let name = p.active().name.clone();
                let mut s = format!("# Notes on {name}\n\n");
                for c in list {
                    let (t, gone) = c.time_in(p);
                    let tick = if c.status == Status::Resolved { "x" } else { " " };
                    let on = match (c.clip, gone) {
                        (Some(id), false) => format!(" (clip #{id})"),
                        (Some(id), true) => format!(" (clip #{id}, since removed)"),
                        _ => String::new(),
                    };
                    s.push_str(&format!("- [{tick}] **{}** {} — {}: {}{on}\n", tc(t), c.id, c.author, c.text));
                    for r in &c.replies {
                        s.push_str(&format!("  - {}: {}\n", r.author, r.text));
                    }
                }
                s
            }
            "edl" => {
                // Markers as an EDL other editors read: one locator
                // per note.
                let mut s = format!("TITLE: {} notes\nFCM: NON-DROP FRAME\n\n", p.active().name);
                for (i, c) in list.iter().enumerate() {
                    let (t, _) = c.time_in(p);
                    let (a, b) = (tc(t), tc(t + (1e6 / fps.max(1.0)) as m::Micros));
                    s.push_str(&format!("{:03}  001      V     C        {a} {b} {a} {b}\n", i + 1));
                    s.push_str(&format!(" |C:ResolveColorBlue |M:{} |D:1\n", c.text.replace('\n', " ")));
                    s.push_str(&format!("* LOC: {a} BLUE {}\n\n", c.text.replace('\n', " ")));
                }
                s
            }
            "json" => serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?,
            o => return Err(format!("format \"{o}\": csv, markdown, edl or json")),
        })
    }

    /// Takes in notes from a file someone sent back: Studio's own notes
    /// (merged thread by thread), a CSV with a time and a note column, an
    /// EDL with `LOC:` markers, or plain lines `00:01:23:12 note`. Notes
    /// already here (same moment, same text) are skipped. Returns how many
    /// were added or changed.
    pub fn import(&mut self, p: &m::Project, text: &str, default_author: &str) -> Result<usize, String> {
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            let other: Store = serde_json::from_str(trimmed).map_err(|e| format!("not a notes file: {e}"))?;
            return Ok(self.merge(&other));
        }
        if trimmed.starts_with('[') {
            let comments: Vec<Comment> = serde_json::from_str(trimmed).map_err(|e| format!("not a notes list: {e}"))?;
            return Ok(self.merge(&Store { format: format_name(), comments }));
        }
        let fps = p.settings.frame_rate.as_f64();
        let mut found: Vec<(m::Micros, String, String)> = Vec::new();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let header = lines.first().map(|l| split_csv(l).iter().map(|h| h.trim().to_lowercase()).collect::<Vec<_>>()).unwrap_or_default();
        let col = |names: &[&str]| header.iter().position(|h| names.iter().any(|n| h == n || h.contains(n)));
        let (tcol, ncol, acol) = (col(&["timecode", "seconds", "time", "tc", "in"]), col(&["note", "comment", "text", "description", "marker name"]), col(&["author", "user", "by", "commenter"]));
        if lines.len() > 1 && tcol.is_some() && ncol.is_some() && lines[0].contains(',') {
            for l in &lines[1..] {
                let f = split_csv(l);
                let (Some(t), Some(n)) = (f.get(tcol.unwrap()).and_then(|t| parse_time(t, fps)), f.get(ncol.unwrap())) else { continue };
                let who = acol.and_then(|a| f.get(a)).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| default_author.to_owned());
                found.push((t, n.trim().to_owned(), who));
            }
        } else {
            let edl = lines.iter().any(|l| l.trim_start().starts_with("* LOC:"));
            for l in &lines {
                let l = l.trim();
                if edl && !l.starts_with("* LOC:") {
                    continue;
                }
                // EDL locator: `* LOC: 01:00:05:00 BLUE note`.
                if let Some(rest) = l.strip_prefix("* LOC:") {
                    let mut w = rest.split_whitespace();
                    if let Some(t) = w.next().and_then(|t| parse_time(t, fps)) {
                        let rest: Vec<&str> = w.collect();
                        let note = if rest.len() > 1 { rest[1..].join(" ") } else { rest.join(" ") };
                        found.push((t, note, default_author.to_owned()));
                    }
                    continue;
                }
                let l = l.trim_start_matches(['-', '*', ' ']).trim_start_matches("[ ]").trim_start_matches("[x]").trim().trim_start_matches("**");
                let Some((t, rest)) = l.split_once(char::is_whitespace) else { continue };
                // A bare number is too often something else (an event number).
                let t = t.trim_end_matches("**");
                if !t.contains([':', '.']) {
                    continue;
                }
                let Some(t) = parse_time(t, fps) else { continue };
                let note = rest.trim().trim_start_matches(['-', '—', ':', ' ']).trim();
                // Studio's own Markdown list: `id — author: note (clip #n)`.
                if let Some((id, said)) = note.split_once(" — ")
                    && !id.contains(' ')
                    && let Some((who, text)) = said.split_once(": ")
                {
                    let text = text.rsplit_once(" (clip #").map_or(text, |(t, _)| t);
                    found.push((t, text.trim().to_owned(), who.trim().to_owned()));
                    continue;
                }
                if !note.is_empty() {
                    found.push((t, note.to_owned(), default_author.to_owned()));
                }
            }
        }
        if found.is_empty() {
            return Err("no notes found: expected Zornov notes, a CSV with time and note columns, an EDL with LOC markers, or lines like \"00:01:23:12 note\"".into());
        }
        let frame = (1e6 / fps.max(1.0)) as m::Micros;
        let mut n = 0;
        for (t, note, who) in found {
            if note.is_empty() || self.comments.iter().any(|c| !c.deleted && (c.at - t).abs() <= frame && c.text == note) {
                continue;
            }
            self.add(p, &who, &note, Pin { at: t, ..Pin::default() }, None)?;
            n += 1;
        }
        Ok(n)
    }
}

pub fn status_name(s: Status) -> &'static str {
    match s {
        Status::Open => "open",
        Status::Resolved => "resolved",
    }
}

/// `hh:mm:ss:ff` at `fps`.
pub fn timecode(t: m::Micros, fps: f64) -> String {
    let fps_i = fps.round().max(1.0) as i64;
    let frames = ((t.max(0) as f64 / 1e6) * fps).floor() as i64;
    let (f, s) = (frames % fps_i, frames / fps_i);
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60, f)
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                out.last_mut().unwrap().push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// One line about a note, for lists the AI reads.
pub fn line(c: &Comment, p: &m::Project) -> serde_json::Value {
    let (t, gone) = c.time_in(p);
    let mut o = serde_json::json!({"id": c.id, "t": (t as f64 / 1e3).round() / 1e3, "at": clock(t), "by": c.author, "note": c.text, "status": status_name(c.status)});
    if let Some(u) = c.until {
        o["until"] = serde_json::json!((u as f64 / 1e3).round() / 1e3);
    }
    if let Some(id) = c.clip {
        o["clip"] = serde_json::json!(id);
        if gone {
            o["clip_gone"] = serde_json::json!(true);
        }
    }
    if let Some(r) = c.region {
        o["region"] = serde_json::json!(r.iter().map(|v| v.round()).collect::<Vec<_>>());
    }
    if !c.replies.is_empty() {
        o["replies"] = serde_json::json!(c.replies.iter().map(|r| format!("{}: {}", r.author, r.text)).collect::<Vec<_>>());
    }
    if let Some(v) = &c.version {
        o["version"] = serde_json::json!(v);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::testing;

    #[test]
    fn threads_follow_their_clip_and_resolve() {
        let mut p = testing::project();
        let mut s = Store::default();
        let second = p.active().clips[1].clone();
        let id = s.add(&p, "Sam", "too dark", Pin { at: second.start + 500_000, clip: Some(second.id.0), region: Some([-100.0, -50.0, 100.0, 50.0]), ..Pin::default() }, None).unwrap();
        assert_eq!(id, "sam-1");
        let other = s.add(&p, "Sam", "music cue late", Pin { at: 8_000_000, ..Pin::default() }, None).unwrap();
        assert_eq!(other, "sam-2");
        assert_eq!(s.reply(&id, "Ash", "brightening").unwrap(), "ash-1");
        // The first clip goes: the note moves with the second.
        let seq = p.active_sequence;
        let first = p.active().clips[0].id;
        p.apply(m::Edit::RippleDelete { seq, clips: vec![first] }).unwrap();
        assert_eq!(s.get(&id).unwrap().time_in(&p).0, 500_000);
        s.resolve(&id, "Ash", Some("lifted exposure")).unwrap();
        assert_eq!(s.list(&p, Filter::Open).len(), 1);
        assert_eq!(s.list(&p, Filter::Resolved)[0].replies.len(), 2);
        s.reopen(&id).unwrap();
        assert_eq!(s.open_count(), 2);
        s.delete(&other).unwrap();
        assert_eq!(s.list(&p, Filter::All).len(), 1);
        assert!(s.resolve("nope", "x", None).unwrap_err().contains("sam-1"));
    }

    #[test]
    fn notes_merge_without_losing_replies_or_resurrecting() {
        let p = testing::project();
        let mut a = Store::default();
        let id = a.add(&p, "Sam", "cut sooner", Pin { at: 1_000_000, ..Pin::default() }, None).unwrap();
        let gone = a.add(&p, "Sam", "typo", Pin { at: 2_000_000, ..Pin::default() }, None).unwrap();
        let mut b = a.clone();
        a.reply(&id, "Ash", "done in v2").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        b.reply(&id, "Kim", "agree").unwrap();
        b.delete(&gone).unwrap();
        b.add(&p, "Kim", "new note", Pin { at: 3_000_000, ..Pin::default() }, None).unwrap();
        let n = a.merge(&b);
        assert_eq!(n, 3, "the thread, the deletion and the new note");
        assert_eq!(a.get(&id).unwrap().replies.len(), 2);
        assert!(a.get(&gone).is_none());
        // Merging the older copy again changes nothing.
        let old = b.clone();
        assert_eq!(a.merge(&old), 0);
    }

    #[test]
    fn export_then_import_round_trips_as_a_list() {
        let p = testing::project();
        let mut s = Store::default();
        s.add(&p, "Sam", "hold, then cut", Pin { at: 1_500_000, ..Pin::default() }, None).unwrap();
        s.add(&p, "Sam", "louder, \"much\"", Pin { at: 4_000_000, ..Pin::default() }, None).unwrap();
        let csv = s.export(&p, Filter::All, "csv").unwrap();
        assert!(csv.contains("00:00:01:15"), "{csv}");
        let mut t = Store::default();
        assert_eq!(t.import(&p, &csv, "x").unwrap(), 2);
        assert_eq!(t.list(&p, Filter::All)[1].text, "louder, \"much\"");
        assert_eq!(t.list(&p, Filter::All)[0].author, "Sam");
        // Twice: nothing new.
        assert_eq!(t.import(&p, &csv, "x").unwrap(), 0);

        let md = s.export(&p, Filter::All, "markdown").unwrap();
        let mut u = Store::default();
        assert_eq!(u.import(&p, &md, "Reviewer").unwrap(), 2, "{md}");
        let edl = s.export(&p, Filter::All, "edl").unwrap();
        let mut v = Store::default();
        assert_eq!(v.import(&p, &edl, "Reviewer").unwrap(), 2, "{edl}");
        assert_eq!(v.list(&p, Filter::All)[0].at, 1_500_000);

        let mut w = Store::default();
        assert_eq!(w.import(&p, "00:00:02:00 start the music here\n0:05.5 - fade out\n", "Kim").unwrap(), 2);
        assert_eq!(w.list(&p, Filter::All)[1].at, 5_500_000);
        assert!(w.import(&p, "hello", "Kim").is_err());
    }
}
