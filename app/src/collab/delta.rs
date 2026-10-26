// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The difference between two states of a project, two ways:
//!
//! * [`edits_between`]: the primitive model edits that turn one into the
//!   other. Restoring a version or taking in a merge applies them through
//!   the history, so either is one undo step like any edit.
//! * [`report`]: what changed, for people: clips added, removed, moved,
//!   trimmed, and which of their properties changed (old → new).

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use studio_model as m;

/// The edits that turn `from` into `to` (empty when they already match).
/// The id counter is not an edit; callers keep the larger one.
pub fn edits_between(from: &m::Project, to: &m::Project) -> Vec<m::Edit> {
    use m::Edit::*;
    let mut first = Vec::new();
    let mut last = Vec::new();

    // Media and sequences that are new come first (clips refer to them);
    // the ones that went go last (nothing may still use them).
    for (i, item) in to.media.iter().enumerate() {
        match from.media_item(item.id) {
            None => first.push(InsertMedia { item: Box::new(item.clone()), index: Some(i) }),
            Some(old) if old != item => first.push(SetMedia { item: Box::new(item.clone()) }),
            _ => {}
        }
    }
    for (i, s) in to.sequences.iter().enumerate() {
        if from.sequence(s.id).is_none() {
            first.push(InsertSequence { sequence: Box::new(s.clone()), index: Some(i) });
        }
    }
    if from.settings != to.settings {
        first.push(SetSettings { settings: Box::new(to.settings.clone()) });
    }
    for p in &to.presets {
        match from.preset(p.id) {
            None => first.push(InsertPreset { preset: p.clone() }),
            Some(old) if old != p => first.push(SetPreset { preset: p.clone() }),
            _ => {}
        }
    }
    for p in &from.presets {
        if to.preset(p.id).is_none() {
            last.push(RemovePreset { id: p.id });
        }
    }

    for s in &to.sequences {
        if let Some(old) = from.sequence(s.id) {
            sequence_edits(old, s, &mut first);
        }
    }
    if from.active_sequence != to.active_sequence {
        last.push(SetActiveSequence { id: to.active_sequence });
    }
    for s in &from.sequences {
        if to.sequence(s.id).is_none() {
            last.push(RemoveSequence { id: s.id });
        }
    }
    for item in &from.media {
        if to.media_item(item.id).is_none() {
            last.push(RemoveMedia { id: item.id });
        }
    }
    first.extend(last);
    first
}

fn sequence_edits(from: &m::Sequence, to: &m::Sequence, out: &mut Vec<m::Edit>) {
    use m::Edit::*;
    let seq = to.id;
    let to_clips: HashMap<m::ClipId, &m::Clip> = to.clips.iter().map(|c| (c.id, c)).collect();
    let from_clips: HashMap<m::ClipId, &m::Clip> = from.clips.iter().map(|c| (c.id, c)).collect();

    for t in &from.transitions {
        if !to.transitions.iter().any(|x| x.id == t.id) {
            out.push(RemoveTransition { seq, id: t.id });
        }
    }
    for c in &from.clips {
        if !to_clips.contains_key(&c.id) {
            out.push(RemoveClip { seq, clip: c.id });
        }
    }
    for mk in &from.markers {
        if !to.markers.iter().any(|x| x.id == mk.id) {
            out.push(RemoveMarker { seq, id: mk.id });
        }
    }
    for l in &to.lanes {
        match from.lanes.iter().find(|x| x.id == l.id) {
            None => out.push(InsertLane { seq, index: 0, lane: l.clone() }),
            Some(old) if old != l => out.push(SetLane { seq, lane: l.clone() }),
            _ => {}
        }
    }
    for c in &to.clips {
        match from_clips.get(&c.id) {
            None => out.push(InsertClip { seq, clip: Box::new(c.clone()) }),
            Some(old) if *old != c => out.push(SetClip { seq, clip: Box::new(c.clone()) }),
            _ => {}
        }
    }
    for l in &from.lanes {
        if !to.lanes.iter().any(|x| x.id == l.id) {
            out.push(RemoveLane { seq, lane: l.id });
        }
    }
    // Lane order: after the inserts and removals, put each where `to` has it.
    let mut order: Vec<m::LaneId> = from.lanes.iter().map(|l| l.id).filter(|id| to.lanes.iter().any(|x| x.id == *id)).collect();
    for l in &to.lanes {
        if !order.contains(&l.id) {
            order.insert(0, l.id);
        }
    }
    for (i, l) in to.lanes.iter().enumerate() {
        if order.get(i) != Some(&l.id) {
            out.push(MoveLane { seq, lane: l.id, to: i });
            order.retain(|x| *x != l.id);
            order.insert(i, l.id);
        }
    }
    for t in &to.transitions {
        match from.transitions.iter().find(|x| x.id == t.id) {
            None => out.push(InsertTransition { seq, transition: t.clone() }),
            Some(old) if old != t => out.push(SetTransition { seq, transition: t.clone() }),
            _ => {}
        }
    }
    for mk in &to.markers {
        match from.markers.iter().find(|x| x.id == mk.id) {
            None => out.push(InsertMarker { seq, marker: mk.clone() }),
            Some(old) if old != mk => out.push(SetMarker { seq, marker: mk.clone() }),
            _ => {}
        }
    }
    if from.name != to.name {
        out.push(RenameSequence { id: seq, name: to.name.clone() });
    }
}

/// One thing that differs between two versions.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Change {
    /// `added`, `removed`, `moved`, `trimmed`, `changed`, `reordered`.
    pub kind: &'static str,
    /// `clip`, `lane`, `marker`, `transition`, `media`, `sequence`,
    /// `settings`, `preset`.
    pub what: &'static str,
    /// The object's id (0 for settings).
    pub id: u64,
    /// Its name, for people.
    pub name: String,
    /// The sequence it is in, when it is in one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sequence: String,
    /// Details: times, lanes, properties with old → new.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// What changed from `a` to `b`, clip by clip.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Report {
    pub changes: Vec<Change>,
    /// How many changes of each kind (`added`: 2, …).
    pub counts: BTreeMap<String, usize>,
}

impl Report {
    /// True when the two versions show the same edit.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// One line per change.
    pub fn text(&self) -> String {
        if self.changes.is_empty() {
            return "no differences".into();
        }
        self.changes
            .iter()
            .map(|c| {
                let place = if c.sequence.is_empty() { String::new() } else { format!(" in {}", c.sequence) };
                let detail = if c.detail.is_empty() { String::new() } else { format!(": {}", c.detail) };
                format!("{} {} #{} \"{}\"{place}{detail}", c.what, c.kind, c.id, c.name)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn push(&mut self, c: Change) {
        *self.counts.entry(c.kind.to_owned()).or_default() += 1;
        self.changes.push(c);
    }
}

fn secs(t: m::Micros) -> String {
    format!("{:.2}s", t as f64 / 1e6)
}

fn lane_name(s: &m::Sequence, id: m::LaneId) -> String {
    s.lanes.iter().find(|l| l.id == id).map(|l| if l.name.is_empty() { format!("lane {}", id.0) } else { l.name.clone() }).unwrap_or_else(|| format!("lane {}", id.0))
}

/// What changed from `a` (older) to `b` (newer).
pub fn report(a: &m::Project, b: &m::Project) -> Report {
    let mut r = Report::default();
    for s in &b.sequences {
        match a.sequence(s.id) {
            None => r.push(Change { kind: "added", what: "sequence", id: s.id.0, name: s.name.clone(), sequence: String::new(), detail: format!("{} clips", s.clips.len()) }),
            Some(old) => sequence_report(old, s, &mut r),
        }
    }
    for s in &a.sequences {
        if b.sequence(s.id).is_none() {
            r.push(Change { kind: "removed", what: "sequence", id: s.id.0, name: s.name.clone(), sequence: String::new(), detail: String::new() });
        }
    }
    for item in &b.media {
        match a.media_item(item.id) {
            None => r.push(Change { kind: "added", what: "media", id: item.id.0, name: item.name.clone(), sequence: String::new(), detail: String::new() }),
            Some(old) if old != item => r.push(Change { kind: "changed", what: "media", id: item.id.0, name: item.name.clone(), sequence: String::new(), detail: field_diff(old, item, &[]) }),
            _ => {}
        }
    }
    for item in &a.media {
        if b.media_item(item.id).is_none() {
            r.push(Change { kind: "removed", what: "media", id: item.id.0, name: item.name.clone(), sequence: String::new(), detail: String::new() });
        }
    }
    if a.settings != b.settings {
        r.push(Change { kind: "changed", what: "settings", id: 0, name: "project settings".into(), sequence: String::new(), detail: field_diff(&a.settings, &b.settings, &[]) });
    }
    for p in &b.presets {
        match a.preset(p.id) {
            None => r.push(Change { kind: "added", what: "preset", id: p.id.0, name: p.name.clone(), sequence: String::new(), detail: p.kind.clone() }),
            Some(old) if old != p => r.push(Change { kind: "changed", what: "preset", id: p.id.0, name: p.name.clone(), sequence: String::new(), detail: String::new() }),
            _ => {}
        }
    }
    for p in &a.presets {
        if b.preset(p.id).is_none() {
            r.push(Change { kind: "removed", what: "preset", id: p.id.0, name: p.name.clone(), sequence: String::new(), detail: String::new() });
        }
    }
    r
}

fn sequence_report(a: &m::Sequence, b: &m::Sequence, r: &mut Report) {
    let seq = b.name.clone();
    let ch = |kind, what, id: u64, name: &str, detail: String| Change { kind, what, id, name: name.to_owned(), sequence: seq.clone(), detail };
    for c in &b.clips {
        let Some(old) = a.clips.iter().find(|x| x.id == c.id) else {
            r.push(ch("added", "clip", c.id.0, &c.name, format!("at {} on {}, {} long", secs(c.start), lane_name(b, c.lane), secs(c.duration))));
            continue;
        };
        if old.start != c.start || old.lane != c.lane {
            let mut d = Vec::new();
            if old.start != c.start {
                d.push(format!("{} → {}", secs(old.start), secs(c.start)));
            }
            if old.lane != c.lane {
                d.push(format!("{} → {}", lane_name(a, old.lane), lane_name(b, c.lane)));
            }
            r.push(ch("moved", "clip", c.id.0, &c.name, d.join(", ")));
        }
        if old.duration != c.duration || old.source_in != c.source_in {
            let mut d = Vec::new();
            if old.duration != c.duration {
                d.push(format!("length {} → {}", secs(old.duration), secs(c.duration)));
            }
            if old.source_in != c.source_in {
                d.push(format!("source in {} → {}", secs(old.source_in), secs(c.source_in)));
            }
            r.push(ch("trimmed", "clip", c.id.0, &c.name, d.join(", ")));
        }
        let detail = field_diff(old, c, &["id", "lane", "start", "duration", "source_in"]);
        if !detail.is_empty() {
            r.push(ch("changed", "clip", c.id.0, &c.name, detail));
        }
    }
    for c in &a.clips {
        if !b.clips.iter().any(|x| x.id == c.id) {
            r.push(ch("removed", "clip", c.id.0, &c.name, format!("was at {} on {}", secs(c.start), lane_name(a, c.lane))));
        }
    }
    for l in &b.lanes {
        match a.lanes.iter().find(|x| x.id == l.id) {
            None => r.push(ch("added", "lane", l.id.0, &l.name, String::new())),
            Some(old) if old != l => r.push(ch("changed", "lane", l.id.0, &l.name, field_diff(old, l, &["id"]))),
            _ => {}
        }
    }
    for l in &a.lanes {
        if !b.lanes.iter().any(|x| x.id == l.id) {
            r.push(ch("removed", "lane", l.id.0, &l.name, String::new()));
        }
    }
    let common = |x: &m::Sequence, y: &m::Sequence| -> Vec<m::LaneId> { x.lanes.iter().map(|l| l.id).filter(|id| y.lanes.iter().any(|k| k.id == *id)).collect() };
    if common(a, b) != common(b, a) {
        r.push(ch("reordered", "lane", 0, "lanes", String::new()));
    }
    for t in &b.transitions {
        match a.transitions.iter().find(|x| x.id == t.id) {
            None => r.push(ch("added", "transition", t.id.0, &t.kind, format!("{} between #{} and #{}", secs(t.duration), t.left.0, t.right.0))),
            Some(old) if old != t => r.push(ch("changed", "transition", t.id.0, &t.kind, field_diff(old, t, &["id"]))),
            _ => {}
        }
    }
    for t in &a.transitions {
        if !b.transitions.iter().any(|x| x.id == t.id) {
            r.push(ch("removed", "transition", t.id.0, &t.kind, String::new()));
        }
    }
    for mk in &b.markers {
        match a.markers.iter().find(|x| x.id == mk.id) {
            None => r.push(ch("added", "marker", mk.id.0, &mk.name, format!("at {}", secs(mk.time)))),
            Some(old) if old != mk => r.push(ch("changed", "marker", mk.id.0, &mk.name, field_diff(old, mk, &["id"]))),
            _ => {}
        }
    }
    for mk in &a.markers {
        if !b.markers.iter().any(|x| x.id == mk.id) {
            r.push(ch("removed", "marker", mk.id.0, &mk.name, String::new()));
        }
    }
    if a.name != b.name {
        r.push(ch("changed", "sequence", b.id.0, &b.name, format!("renamed from \"{}\"", a.name)));
    }
}

/// `path old → new` for the leaves that differ (at most a dozen), skipping
/// top-level `skip` fields.
pub fn field_diff<T: Serialize>(a: &T, b: &T, skip: &[&str]) -> String {
    let (a, b) = (serde_json::to_value(a).unwrap_or(Value::Null), serde_json::to_value(b).unwrap_or(Value::Null));
    let mut out = Vec::new();
    let mut more = 0usize;
    leaves("", &a, &b, skip, &mut out, &mut more);
    let mut s = out.join("; ");
    if more > 0 {
        s.push_str(&format!("; +{more} more"));
    }
    s
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::Null => "none".to_owned(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() != 0.0 => format!("{:.3}", f).trim_end_matches('0').to_owned(),
            Some(f) if f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        Value::String(s) => format!("\"{s}\""),
        Value::Array(a) => format!("[{} items]", a.len()),
        Value::Object(_) => "{…}".to_owned(),
        Value::Bool(b) => b.to_string(),
    };
    if s.chars().count() > 40 { format!("{}…", s.chars().take(39).collect::<String>()) } else { s }
}

fn leaves(path: &str, a: &Value, b: &Value, skip: &[&str], out: &mut Vec<String>, more: &mut usize) {
    if a == b {
        return;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                if path.is_empty() && skip.contains(&k.as_str()) {
                    continue;
                }
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                leaves(&p, x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), skip, out, more);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() && x.len() <= 8 => {
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                leaves(&format!("{path}[{i}]"), u, v, skip, out, more);
            }
        }
        _ => {
            if out.len() < 12 {
                out.push(format!("{path} {} → {}", short(a), short(b)));
            } else {
                *more += 1;
            }
        }
    }
}

/// The clips, lanes and markers an edit list touches, by sequence: what
/// a lock check needs.
pub fn touched(edits: &[m::Edit], before: &m::Project, out: &mut HashSet<(u64, u64)>, lanes: &mut HashSet<(u64, u64)>) {
    use m::Edit::*;
    for e in edits {
        match e {
            Batch { edits } => touched(edits, before, out, lanes),
            InsertClip { seq, clip } | SetClip { seq, clip } => {
                out.insert((seq.0, clip.id.0));
                lanes.insert((seq.0, clip.lane.0));
                if let Some(old) = before.sequence(*seq).and_then(|s| s.clip(clip.id)) {
                    lanes.insert((seq.0, old.lane.0));
                }
            }
            RemoveClip { seq, clip } => {
                out.insert((seq.0, clip.0));
                if let Some(old) = before.sequence(*seq).and_then(|s| s.clip(*clip)) {
                    lanes.insert((seq.0, old.lane.0));
                }
            }
            InsertLane { seq, lane, .. } | SetLane { seq, lane } => {
                lanes.insert((seq.0, lane.id.0));
            }
            RemoveLane { seq, lane } | MoveLane { seq, lane, .. } => {
                lanes.insert((seq.0, lane.0));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::testing;

    #[test]
    fn edits_between_two_states_rebuild_the_second() {
        let a = testing::project();
        let mut b = a.clone();
        let seq = b.active_sequence;
        let first = b.active().clips[0].id;
        let last = b.active().clips.last().unwrap().id;
        b.apply(m::Edit::RippleDelete { seq, clips: vec![first] }).unwrap();
        let mut c = b.active().clip(last).unwrap().clone();
        c.transform.opacity = 0.5;
        b.apply(m::Edit::SetClip { seq, clip: Box::new(c) }).unwrap();
        b.apply(m::Edit::RenameSequence { id: seq, name: "Cut 2".into() }).unwrap();

        let mut x = a.clone();
        let edits = edits_between(&a, &b);
        assert!(!edits.is_empty());
        x.apply(m::Edit::Batch { edits }).unwrap();
        assert!(x.content_eq(&b), "{}", report(&x, &b).text());
        assert!(edits_between(&b, &b).is_empty());

        // And back.
        let mut y = b.clone();
        y.apply(m::Edit::Batch { edits: edits_between(&b, &a) }).unwrap();
        assert!(y.content_eq(&a));
    }

    #[test]
    fn the_report_names_what_changed() {
        let a = testing::project();
        let mut b = a.clone();
        let seq = b.active_sequence;
        let first = b.active().clips[0].id;
        let second = b.active().clips[1].id;
        b.apply(m::Edit::RippleDelete { seq, clips: vec![first] }).unwrap();
        let mut c = b.active().clip(second).unwrap().clone();
        c.transform.opacity = 0.4;
        b.apply(m::Edit::SetClip { seq, clip: Box::new(c) }).unwrap();
        let r = report(&a, &b);
        let text = r.text();
        assert_eq!(r.counts.get("removed"), Some(&1), "{text}");
        assert!(r.changes.iter().any(|c| c.kind == "moved" && c.id == second.0), "{text}");
        assert!(text.contains("transform.opacity 1 → 0.4"), "{text}");
        assert!(report(&a, &a).is_empty());
    }
}
