// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! After an edit, what is really on the timeline: every clip the ops
//! named or made, read back from the project (not from what the ops said
//! they did). A clip an op named that came out exactly as it went in is
//! called out, so nobody reports an effect, a key or a move that never
//! landed. Reading back is not looking: it says what is set, not how it
//! looks.

use super::{Session, secs};
use crate::view::{ClipKind, SEC};
use serde_json::{Value, json};
use std::collections::HashMap;
use studio_model as m;

/// The clips an op names (after its references are resolved).
pub(super) fn targets(op: &Value) -> Vec<u32> {
    let mut out = Vec::new();
    for key in ["clip", "clips", "parent", "left", "right"] {
        match op.get(key) {
            Some(Value::Number(n)) => out.extend(n.as_u64().map(|v| v as u32)),
            Some(Value::Array(a)) => out.extend(a.iter().filter_map(Value::as_u64).map(|v| v as u32)),
            _ => {}
        }
    }
    // when they are read back.
    out.sort();
    out.dedup();
    out
}

impl Session {
    /// A fingerprint of everything about a clip (`None`: it is not there).
    pub(super) fn clip_print(&self, id: u32) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mc = self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64)))?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(mc).unwrap_or_default().hash(&mut h);
        Some(h.finish())
    }

    /// One clip as it now is on the timeline.
    fn now_line(&self, id: u32) -> Value {
        let p = &self.doc.project;
        let (Some(c), Some(mc)) = (p.clip(id), self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64)))) else {
            return json!({"clip": id, "on_timeline": false});
        };
        let lane = p.lane(c.lane);
        let mut o = json!({"clip": id, "name": c.name, "lane": lane.map(|l| l.name.clone()).unwrap_or_default(), "start": secs(c.start), "dur": secs(c.dur)});
        let mut off = Vec::new();
        if !mc.enabled {
            off.push("the clip is switched off");
        }
        if lane.is_some_and(|l| l.hidden) && c.kind != ClipKind::Audio {
            off.push("its lane is hidden");
        }
        if lane.is_some_and(|l| l.muted) && c.kind == ClipKind::Audio {
            off.push("its lane is muted");
        }
        if c.kind != ClipKind::Audio && mc.transform.opacity <= 0.001 && !mc.keyframes.contains_key("transform.opacity") {
            off.push("its opacity is 0");
        }
        if !off.is_empty() {
            o["not_seen_because"] = json!(off);
        }
        if !mc.effects.is_empty() {
            o["effects"] = json!(mc.effects.iter().map(|e| if e.enabled { e.kind.clone() } else { format!("{} (off)", e.kind) }).collect::<Vec<_>>());
        }
        if !mc.keyframes.is_empty() {
            let mut groups: Vec<(String, usize, usize)> = Vec::new();
            for (path, tr) in &mc.keyframes {
                // puppet.12.position → puppet: pins are counted, not listed.
                let head = match path.split('.').collect::<Vec<_>>().as_slice() {
                    ["puppet" | "bone" | "ik" | "effect", ..] => path.split('.').next().unwrap_or(path).to_owned(),
                    _ => path.clone(),
                };
                match groups.iter_mut().find(|g| g.0 == head) {
                    Some(g) => {
                        g.1 += 1;
                        g.2 += tr.keys.len();
                    }
                    None => groups.push((head, 1, tr.keys.len())),
                }
            }
            o["keys"] = json!(groups.iter().map(|(h, n, k)| if *n > 1 { format!("{h}: {n} tracks, {k} keys") } else { format!("{h}: {k} keys") }).collect::<Vec<_>>());
            // Motion keyed past the clip's end is never seen (a walk asked
            // for 5 s on a 1 s clip plays its first second only).
            let last = mc.keyframes.values().flat_map(|t| t.keys.iter().map(|k| k.time)).max().unwrap_or(0);
            if last > c.dur + SEC / 20 {
                o["keys_past_end"] = json!(format!("keys run to {}s into the clip but it is {}s long, so the rest is never seen: lengthen it (set duration) or key within it", secs(last), secs(c.dur)));
            }
        }
        // A model standing in or over the surface under it.
        if let Some(g) = self.footing_said(id) {
            o["ground"] = json!(g);
        }
        if let Some(pp) = mc.puppet.as_ref().filter(|q| !q.pins.is_empty()) {
            o["puppet_pins"] = json!(if pp.enabled { pp.pins.len().to_string() } else { format!("{} (puppet off)", pp.pins.len()) });
        }
        if !mc.masks.is_empty() {
            o["masks"] = json!(mc.masks.len());
        }
        let mo: Vec<String> = [("entry", &mc.motion.entry), ("exit", &mc.motion.exit), ("loop", &mc.motion.looping)].iter().filter_map(|(k, v)| v.as_ref().map(|p| format!("{k}: {}", p.kind))).collect();
        if !mo.is_empty() {
            o["motion"] = json!(mo);
        }
        if !mc.drivers.is_empty() {
            o["expressions"] = json!(mc.drivers.keys().collect::<Vec<_>>());
        }
        o
    }

    /// What the edit did to clips no op named: removed, cut short, moved
    /// or put on another lane by a ripple or an overwrite (a `move` and a
    /// `duplicate` overwrote clips and nobody heard). Clips on the main
    /// lane that only slid along by a ripple are counted, not listed.
    /// `None` when nothing else was touched.
    pub(super) fn side_effects(&self, before: &std::collections::BTreeMap<u32, Value>, named: &[(usize, String, Vec<u32>)], made: &[u32]) -> Option<Value> {
        let mine: std::collections::HashSet<u32> = named.iter().flat_map(|(_, _, v)| v.iter().copied()).chain(made.iter().copied()).collect();
        let spine = self.doc.project.spine();
        let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let mut lines: Vec<String> = Vec::new();
        let mut slid = 0usize;
        for (&id, was) in before {
            if mine.contains(&id) {
                continue;
            }
            let label = format!("clip {id}{}", was.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(|n| format!(" ({n})")).unwrap_or_default());
            let Some(c) = self.doc.project.clip(id) else {
                lines.push(format!("{label} is gone: it was {:.2}..{:.2}s; an overwrite or ripple took it (undo brings it back)", num(was, "start"), num(was, "start") + num(was, "dur")));
                continue;
            };
            let (s0, d0, s1, d1) = (num(was, "start"), num(was, "dur"), secs(c.start), secs(c.dur));
            let lane_moved = was.get("lane").and_then(Value::as_u64).is_some_and(|l| l != c.lane as u64);
            if (d1 - d0).abs() > 1e-3 {
                lines.push(format!("{label} was cut from {:.2}s to {:.2}s long ({:.2}..{:.2}s now): another clip was laid over it", d0, d1, s1, s1 + d1));
            } else if lane_moved {
                lines.push(format!("{label} went to another lane"));
            } else if (s1 - s0).abs() > 1e-3 {
                if Some(c.lane) == spine {
                    slid += 1;
                } else {
                    lines.push(format!("{label} moved from {:.2}s to {:.2}s (a ripple)", s0, s1));
                }
            }
        }
        if lines.is_empty() && slid == 0 {
            return None;
        }
        let more = lines.len().saturating_sub(12);
        lines.truncate(12);
        if more > 0 {
            lines.push(format!("{more} more clips no op named changed too (summary lists them)"));
        }
        if slid > 0 {
            lines.push(format!("{slid} main-lane clip(s) slid along with the ripple, as the magnetic main lane does"));
        }
        Some(json!(lines))
    }

    /// What the edit left on the timeline: each named or made clip read
    /// back, and the ops whose clip came out unchanged.
    pub(super) fn read_back(&self, named: &[(usize, String, Vec<u32>)], before: &HashMap<u32, Option<u64>>, made: &[u32]) -> Value {
        let mut ids: Vec<u32> = named.iter().flat_map(|(_, _, v)| v.iter().copied()).filter(|id| before.get(id).is_some_and(Option::is_some)).collect();
        ids.extend(made.iter().copied());
        ids.sort();
        ids.dedup();
        let lines: Vec<Value> = ids.iter().take(24).map(|&id| self.now_line(id)).collect();
        let mut unchanged = Vec::new();
        for (i, name, cs) in named {
            for id in cs {
                if let Some(Some(was)) = before.get(id) {
                    if self.clip_print(*id) == Some(*was) && !matches!(name.as_str(), "select" | "inspect" | "copy" | "duplicate" | "save_move" | "save_design") {
                        unchanged.push(format!("op {} ({name}) named clip {id}, but clip {id} is exactly as it was: nothing of that op is on it", i + 1));
                    }
                }
            }
        }
        let mut o = json!({
            "clips": lines,
            "note": "read back from the timeline after the edit: this is what is set, not how it looks; frame / contact_sheet show it"
        });
        if ids.len() > 24 {
            o["more"] = json!(format!("{} more clips changed (summary lists them)", ids.len() - 24));
        }
        if !unchanged.is_empty() {
            o["unchanged"] = json!(unchanged);
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_the_clips_an_op_names() {
        assert_eq!(targets(&json!({"op": "effect", "clip": 12, "kind": "glow"})), vec![12]);
        assert_eq!(targets(&json!({"op": "set", "clips": [4, 2, 4], "opacity": 50})), vec![2, 4]);
        assert!(targets(&json!({"op": "place", "media": 3, "at": 1.0})).is_empty());
    }

    #[test]
    fn a_new_model_gets_a_shot_and_motion_past_its_end_is_called_out() {
        let mut s = Session::default();
        // A new project is a second long; a character added to it gets a shot's length.
        let r = s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "detail": 1, "name": "Hero"}), json!({"op": "act", "clip": "Hero", "motion": "walk", "duration": 4})], "t", false).unwrap();
        let line = &r["read_back"]["clips"][0];
        assert_eq!(line["dur"].as_f64(), Some(5.0), "{r}");
        assert!(line.get("keys_past_end").is_none(), "{line}");
        // Told to be one second long, the 4 s walk is said to run past it.
        let r = s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "detail": 1, "name": "Short", "duration": 1}), json!({"op": "act", "clip": "Short", "motion": "walk", "duration": 4})], "t", false).unwrap();
        let past = r["read_back"]["clips"].as_array().unwrap().iter().find_map(|c| c["keys_past_end"].as_str()).unwrap_or_default().to_owned();
        assert!(past.contains("is 1s long"), "{r}");
    }

    #[test]
    fn an_op_that_changed_nothing_is_called_out() {
        let mut s = Session::default();
        let r = s.edit(&[json!({"op": "card", "colour": "#203040", "at": 0, "duration": 2})], "t", false).unwrap();
        let id = r["read_back"]["clips"][0]["clip"].as_u64().expect("the card is read back") as u32;
        assert_eq!(r["read_back"]["clips"][0]["dur"].as_f64(), Some(2.0));
        // Setting what is already set leaves the clip as it was.
        let r = s.edit(&[json!({"op": "set", "clip": id, "opacity": 100})], "t", false).unwrap();
        assert!(r["read_back"]["unchanged"][0].as_str().is_some_and(|u| u.contains(&format!("clip {id}"))), "{r}");
        let r = s.edit(&[json!({"op": "set", "clip": id, "opacity": 40})], "t", false).unwrap();
        assert!(r["read_back"].get("unchanged").is_none(), "{r}");
    }
}
