// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The timeline read like an edit decision list (`script`), and a
//! notebook kept with the project (`notes`) so an agent can leave itself
//! the plan, the beat map and what is left to do.

use super::{Session, secs};
use crate::view::{self, ClipKind, LaneKind};
use serde_json::{Value, json};
use studio_model as m;

fn t(v: view::Time) -> String {
    format!("{:6.2}", secs(v))
}

impl Session {
    /// One line per clip, lane by lane top to bottom, with what is on it;
    /// transitions between the main-lane lines; markers last.
    pub(super) fn script(&self) -> String {
        let p = &self.doc.project;
        let mut out = format!("{} · {}×{} · {} fps · {:.2} s\n", p.name, p.width, p.height, p.fps, secs(p.duration()));
        for lane in &p.lanes {
            let mut cs: Vec<&view::Clip> = p.clips.iter().filter(|c| c.lane == lane.id).collect();
            if cs.is_empty() {
                continue;
            }
            cs.sort_by_key(|c| c.start);
            let role = match lane.kind {
                LaneKind::Spine => "main",
                LaneKind::Visual => "overlay",
                LaneKind::Audio => "sound",
            };
            let flags: Vec<&str> = [(lane.locked, "locked"), (lane.hidden, "hidden"), (lane.muted, "muted"), (lane.solo, "solo")].iter().filter(|(b, _)| *b).map(|(_, s)| *s).collect();
            out.push_str(&format!("\n{} ({role}{}{})\n", lane.name.to_uppercase(), if flags.is_empty() { "" } else { ", " }, flags.join(", ")));
            for c in cs {
                let mut what: Vec<String> = Vec::new();
                match c.kind {
                    ClipKind::Title if !c.text.is_empty() => {
                        let k = c.text_colour;
                        what.push(format!("\"{}\" {} {:.0}px #{:02x}{:02x}{:02x}", c.text.replace('\n', " / "), c.text_style.clone().unwrap_or_default(), c.extra.text_size, k.r(), k.g(), k.b()));
                    }
                    _ => {
                        if let Some(mi) = c.media.and_then(|m| p.media(m)) {
                            let src = secs(c.src_in);
                            let used = secs(c.dur) * c.speed as f64 / 100.0;
                            let stem = mi.name.rsplit_once('.').map_or(mi.name.as_str(), |(a, _)| a);
                            let short: String = stem.split(" -").next().unwrap_or(stem).chars().take(28).collect();
                            what.push(format!("{} {:.2}–{:.2}", short.trim(), src, src + used));
                        } else {
                            what.push(c.name.clone());
                        }
                    }
                }
                if (c.speed - 100.0).abs() > 0.01 {
                    what.push(format!("speed {}", c.speed / 100.0));
                }
                if (c.transform.scale - 100.0).abs() > 0.01 {
                    what.push(format!("scale {:.0}%", c.transform.scale));
                }
                if c.transform.x.abs() > 0.5 || c.transform.y.abs() > 0.5 {
                    what.push(format!("at ({:.0}, {:.0})", c.transform.x, c.transform.y));
                }
                if (c.transform.opacity - 100.0).abs() > 0.01 {
                    what.push(format!("opacity {:.0}%", c.transform.opacity));
                }
                if let Some(l) = &c.look {
                    what.push(format!("look {l}"));
                }
                for e in c.effects.iter().filter(|e| super::props::effect_name(&e.kind) != super::props::effect_name(&c.name)) {
                    what.push(format!("{}{}", super::props::effect_name(&e.kind), if e.enabled { "" } else { " (off)" }));
                }
                if let Some(m) = &c.motion_in {
                    what.push(format!("enters {m}"));
                }
                let g = &c.grade;
                if g.exposure.abs() > 0.01 {
                    what.push(format!("exposure {:+.1}", g.exposure / 25.0));
                }
                for (n, v) in [("contrast", g.contrast), ("saturation", g.saturation), ("temperature", g.temperature), ("tint", g.tint)] {
                    if v.abs() > 0.5 {
                        what.push(format!("{n} {v:+.0}"));
                    }
                }
                if c.kind == ClipKind::Audio || (c.kind == ClipKind::Video && lane.kind == LaneKind::Audio) {
                    if !c.audio.enabled {
                        what.push("muted".into());
                    } else if c.audio.volume.abs() > 0.01 {
                        what.push(format!("{:+.1} dB", c.audio.volume));
                    }
                    if c.audio.fade_in > 0.0 {
                        what.push(format!("fade in {:.1}", c.audio.fade_in));
                    }
                    if c.audio.fade_out > 0.0 {
                        what.push(format!("fade out {:.1}", c.audio.fade_out));
                    }
                } else if c.kind == ClipKind::Video && !c.audio.enabled {
                    what.push("sound off".into());
                }
                if !c.keys.is_empty() {
                    let names: Vec<String> = c.keys.iter().map(|(k, ts)| format!("{} ×{}", k.rsplit('.').next().unwrap_or(k), ts.len())).collect();
                    what.push(format!("keys: {}", names.join(", ")));
                }
                out.push_str(&format!("{}–{}  #{:<4} {}\n", t(c.start), t(c.end()), c.id, what.join(" · ")));
                if let Some(tr) = p.transitions.iter().find(|tr| tr.left == c.id) {
                    out.push_str(&format!("                 ↳ {} {:.2}s into #{}\n", tr.kind, secs(tr.dur), tr.right));
                }
            }
        }
        if !p.markers.is_empty() {
            let mut ms = p.markers.clone();
            ms.sort_by_key(|m| m.time);
            out.push_str("\nMARKERS\n");
            for m in ms {
                out.push_str(&format!("{}  {}\n", t(m.time), m.note));
            }
        }
        if let Some(n) = self.notes_text() {
            let first: String = n.lines().take(3).collect::<Vec<_>>().join(" / ");
            out.push_str(&format!("\nNOTES (see notes): {first}\n"));
        }
        out
    }

    /// Keeps what summary / script showed at this revision, so a later
    /// call can send only what changed since.
    pub(super) fn snapshot(&mut self) {
        let rev = self.doc.history.revision();
        let lines: serde_json::Map<String, Value> = self.doc.project.clips.iter().map(|c| (c.id.to_string(), self.clip_line(c))).collect();
        let memo = self.memo_pub();
        memo.insert(format!("snap:{rev}"), Value::Object(lines));
        // The last few only.
        let mut snaps: Vec<u64> = memo.keys().filter_map(|k| k.strip_prefix("snap:").and_then(|r| r.parse().ok())).collect();
        snaps.sort_unstable();
        for old in snaps.iter().rev().skip(12) {
            memo.remove(&format!("snap:{old}"));
        }
        self.memo_save_pub();
    }

    pub(super) fn summary_since(&mut self, rev: u64) -> Value {
        let now = self.doc.history.revision();
        let Some(Value::Object(then)) = self.memo_pub().get(&format!("snap:{rev}")).cloned() else {
            self.snapshot();
            let mut v = self.summary();
            v["note"] = json!(format!("no record of revision {rev}; this is the whole project"));
            return v;
        };
        let lines: Vec<(String, Value)> = self.doc.project.clips.iter().map(|c| (c.id.to_string(), self.clip_line(c))).collect();
        let added: Vec<&Value> = lines.iter().filter(|(k, _)| !then.contains_key(k)).map(|(_, v)| v).collect();
        let changed: Vec<&Value> = lines.iter().filter(|(k, v)| then.get(k).is_some_and(|o| o != v)).map(|(_, v)| v).collect();
        let removed: Vec<&String> = then.keys().filter(|k| !lines.iter().any(|(id, _)| id == *k)).collect();
        let out = json!({"revision": now, "since": rev, "added": added, "changed": changed, "removed": removed, "duration": secs(self.doc.project.duration())});
        self.snapshot();
        out
    }

    /// The script, or with `since` only the lines of clips that changed.
    pub(super) fn script_since(&mut self, since: Option<u64>) -> String {
        let full = self.script();
        let Some(rev) = since else {
            self.snapshot();
            return format!("{full}\n(revision {})", self.doc.history.revision());
        };
        let Some(Value::Object(then)) = self.memo_pub().get(&format!("snap:{rev}")).cloned() else {
            self.snapshot();
            return format!("{full}\n(no record of revision {rev}; whole script, revision {})", self.doc.history.revision());
        };
        let now: std::collections::HashMap<String, Value> = self.doc.project.clips.iter().map(|c| (c.id.to_string(), self.clip_line(c))).collect();
        let touched: Vec<String> = now.iter().filter(|(k, v)| then.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        let gone: Vec<&String> = then.keys().filter(|k| !now.contains_key(*k)).collect();
        let mut out: Vec<String> = full.lines().filter(|l| touched.iter().any(|id| l.contains(&format!("#{id} ")))).map(str::to_owned).collect();
        if !gone.is_empty() {
            out.push(format!("removed: {}", gone.iter().map(|g| format!("#{g}")).collect::<Vec<_>>().join(", ")));
        }
        self.snapshot();
        format!("since revision {rev} (now {}):\n{}", self.doc.history.revision(), if out.is_empty() { "nothing changed".to_owned() } else { out.join("\n") })
    }

    fn notes_path(&self) -> Option<std::path::PathBuf> {
        self.home.as_ref().map(|h| m::file::ProjectFolder::new(h).state(m::file::State::AgentNotes)).or_else(|| self.doc.folder.as_ref().map(|f| m::file::ProjectFolder::new(f.root()).state(m::file::State::AgentNotes)))
    }

    pub(super) fn notes_text(&self) -> Option<String> {
        match self.notes_path() {
            Some(p) => std::fs::read_to_string(p).ok(),
            None => self.notes.clone(),
        }
        .map(|n| super::outside::clean_saved(&n))
        .filter(|s| !s.trim().is_empty())
    }

    /// `notes`: read them, or `write` (replace) / `append` text.
    pub(super) fn notes(&mut self, a: &Value) -> Result<Value, String> {
        for t in [a.get("write"), a.get("append")].into_iter().flatten().filter_map(Value::as_str) {
            super::outside::check_saved(t)?;
        }
        let new = match (a.get("write").and_then(Value::as_str), a.get("append").and_then(Value::as_str)) {
            (Some(w), _) => Some(w.to_owned()),
            (None, Some(x)) => Some(format!("{}{}{x}", self.notes_text().unwrap_or_default(), if self.notes_text().is_some_and(|n| !n.ends_with('\n')) { "\n" } else { "" })),
            _ => None,
        };
        if let Some(text) = new {
            match self.notes_path() {
                Some(p) => std::fs::write(&p, &text).map_err(|e| format!("could not write notes: {e}"))?,
                None => self.notes = Some(text),
            }
        }
        Ok(json!({"notes": self.notes_text().unwrap_or_default(), "kept_in": self.notes_path().map(|p| p.display().to_string()).unwrap_or_else(|| "this session only (save the project to keep them)".into())}))
    }
}
