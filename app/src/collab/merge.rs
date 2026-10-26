// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Three-way merge: two copies of a project that grew apart from one base.
//!
//! Objects (clips, lanes, transitions, markers, media, sequences) match by
//! id. Whatever only one side changed is taken; when both changed the same
//! object, their changes are merged property by property, and only a
//! property both set differently is a conflict (yours is kept, and the
//! conflict is reported). The main lane is an ordered list, not times: its
//! clips are merged in order and packed end to end again, so an insert on
//! one side and a trim on the other both survive. A clip that would land
//! on top of another on a free lane moves to a new lane above it.
//!
//! The result is a project; [`super::delta::edits_between`] turns "yours"
//! into it, so taking in a merge is one undoable step.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use studio_model as m;

/// A property both sides changed differently (yours was kept).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Conflict {
    pub what: String,
    pub id: u64,
    pub name: String,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub merged: m::Project,
    pub conflicts: Vec<Conflict>,
    /// Repairs made so the result is a valid edit (moved to a new lane,
    /// a transition dropped, …).
    pub fixes: Vec<String>,
}

/// Merges `theirs` into `mine`; both grew from `base`.
pub fn three_way(base: &m::Project, mine: &m::Project, theirs: &m::Project) -> Result<Outcome, String> {
    let theirs = &renumber_clashes(base, mine, theirs);
    let mut conflicts = Vec::new();
    let mut fixes = Vec::new();
    let mut out = mine.clone();
    out.next_id = mine.next_id.max(theirs.next_id);
    out.settings = merge_obj(&base.settings, &mine.settings, &theirs.settings, "settings", 0, "project settings", &mut conflicts);
    out.media = merge_list(&base.media, &mine.media, &theirs.media, |x| x.id.0, |x| x.name.clone(), "media", &mut conflicts);
    out.presets = merge_list(&base.presets, &mine.presets, &theirs.presets, |x| x.id.0, |x| x.name.clone(), "preset", &mut conflicts);

    // Sequences: matched by id, each merged object by object.
    let mut seqs = Vec::new();
    for s in &mine.sequences {
        match (base.sequence(s.id), theirs.sequence(s.id)) {
            (Some(b), Some(t)) => seqs.push(merge_sequence(b, s, t, &mut conflicts, &mut fixes)),
            (Some(b), None) if b == s => {} // They removed it; you did not touch it.
            (Some(_), None) => {
                conflicts.push(Conflict { what: "sequence".into(), id: s.id.0, name: s.name.clone(), detail: "they removed it, you changed it: kept yours".into() });
                seqs.push(s.clone());
            }
            (None, _) => seqs.push(s.clone()),
        }
    }
    for t in &theirs.sequences {
        if mine.sequence(t.id).is_none() {
            match base.sequence(t.id) {
                None => seqs.push(t.clone()),
                Some(b) if b != t => conflicts.push(Conflict { what: "sequence".into(), id: t.id.0, name: t.name.clone(), detail: "you removed it, they changed it: left removed".into() }),
                _ => {}
            }
        }
    }
    out.sequences = seqs;
    if out.sequence(out.active_sequence).is_none() {
        out.active_sequence = out.sequences.first().map(|s| s.id).ok_or("the merge left no sequence")?;
    }
    // Media a kept clip still uses stays in the bin.
    let used: HashSet<u64> = out.sequences.iter().flat_map(|s| s.clips.iter()).filter_map(|c| if let m::ClipSource::Media(r) = &c.source { Some(r.media.0) } else { None }).collect();
    for id in used {
        if !out.media.iter().any(|x| x.id.0 == id)
            && let Some(item) = mine.media_item(m::MediaId(id)).or_else(|| theirs.media_item(m::MediaId(id))).or_else(|| base.media_item(m::MediaId(id)))
        {
            out.media.push(item.clone());
        }
    }
    repair(&mut out, mine, &mut fixes)?;
    Ok(Outcome { merged: out, conflicts, fixes })
}

/// When both sides made new objects with the same ids (copies that were
/// not set up to sync), their new ids move past yours.
fn renumber_clashes(base: &m::Project, mine: &m::Project, theirs: &m::Project) -> m::Project {
    let ids = |p: &m::Project| -> HashSet<u64> {
        let mut s = HashSet::new();
        collect_ids(&serde_json::to_value(p).unwrap_or(Value::Null), &mut s);
        s
    };
    let b = ids(base);
    let mine_new: HashSet<u64> = ids(mine).difference(&b).copied().collect();
    let theirs_new: HashSet<u64> = ids(theirs).difference(&b).copied().collect();
    let clash: Vec<u64> = mine_new.intersection(&theirs_new).copied().collect();
    if clash.is_empty() {
        return theirs.clone();
    }
    let mut next = mine.next_id.max(theirs.next_id);
    let mut map = HashMap::new();
    let mut sorted: Vec<u64> = theirs_new.into_iter().collect();
    sorted.sort_unstable();
    for id in sorted {
        map.insert(id, next);
        next += 1;
    }
    let mut v = serde_json::to_value(theirs).unwrap_or(Value::Null);
    remap_ids(&mut v, &map);
    let mut p: m::Project = serde_json::from_value(v).unwrap_or_else(|_| theirs.clone());
    p.next_id = next;
    p
}

/// Keys whose numbers are object ids, wherever they appear in a project.
const ID_KEYS: &[&str] = &["id", "lane", "clip", "left", "right", "link", "group", "media", "sequence", "active_sequence"];

fn collect_ids(v: &Value, out: &mut HashSet<u64>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                if ID_KEYS.contains(&k.as_str())
                    && let Some(n) = x.as_u64()
                {
                    out.insert(n);
                }
                collect_ids(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_ids(x, out)),
        _ => {}
    }
}

fn remap_ids(v: &mut Value, map: &HashMap<u64, u64>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if ID_KEYS.contains(&k.as_str())
                    && let Some(n) = x.as_u64()
                    && let Some(to) = map.get(&n)
                {
                    *x = Value::from(*to);
                    continue;
                }
                remap_ids(x, map);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| remap_ids(x, map)),
        _ => {}
    }
}

/// Three-way merge of JSON values: objects key by key, anything else whole.
fn merge_value(b: &Value, mi: &Value, t: &Value, path: &str, clashes: &mut Vec<String>) -> Value {
    if mi == t || t == b {
        return mi.clone();
    }
    if mi == b {
        return t.clone();
    }
    if let (Value::Object(bo), Value::Object(mo), Value::Object(to)) = (b, mi, t) {
        let mut out = serde_json::Map::new();
        let keys: Vec<&String> = {
            let mut k: Vec<&String> = mo.keys().collect();
            k.extend(to.keys().filter(|x| !mo.contains_key(*x)));
            k
        };
        let null = Value::Null;
        for k in keys {
            let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
            let v = merge_value(bo.get(k).unwrap_or(&null), mo.get(k).unwrap_or(&null), to.get(k).unwrap_or(&null), &p, clashes);
            if !v.is_null() || mo.contains_key(k) {
                out.insert(k.clone(), v);
            }
        }
        return Value::Object(out);
    }
    clashes.push(path.to_owned());
    mi.clone()
}

fn merge_obj<T: Serialize + DeserializeOwned + Clone + PartialEq>(b: &T, mi: &T, t: &T, what: &str, id: u64, name: &str, conflicts: &mut Vec<Conflict>) -> T {
    if mi == t || t == b {
        return mi.clone();
    }
    if mi == b {
        return t.clone();
    }
    let (bv, mv, tv) = (serde_json::to_value(b).unwrap_or(Value::Null), serde_json::to_value(mi).unwrap_or(Value::Null), serde_json::to_value(t).unwrap_or(Value::Null));
    let mut clashes = Vec::new();
    let v = merge_value(&bv, &mv, &tv, "", &mut clashes);
    if !clashes.is_empty() {
        conflicts.push(Conflict { what: what.into(), id, name: name.into(), detail: format!("both changed {}: kept yours", clashes.join(", ")) });
    }
    serde_json::from_value(v).unwrap_or_else(|_| mi.clone())
}

/// Merges two lists of objects with ids: your order, their new objects
/// after the one they follow in their list.
fn merge_list<T: Serialize + DeserializeOwned + Clone + PartialEq>(base: &[T], mine: &[T], theirs: &[T], key: impl Fn(&T) -> u64, name: impl Fn(&T) -> String, what: &str, conflicts: &mut Vec<Conflict>) -> Vec<T> {
    let find = |l: &[T], k: u64| l.iter().find(|x| key(x) == k).cloned();
    let mut out: Vec<T> = Vec::new();
    for x in mine {
        let k = key(x);
        match (find(base, k), find(theirs, k)) {
            (Some(b), Some(t)) => out.push(merge_obj(&b, x, &t, what, k, &name(x), conflicts)),
            (Some(b), None) if b == *x => {}
            (Some(_), None) => {
                conflicts.push(Conflict { what: what.into(), id: k, name: name(x), detail: "they removed it, you changed it: kept yours".into() });
                out.push(x.clone());
            }
            (None, _) => out.push(x.clone()),
        }
    }
    for (i, t) in theirs.iter().enumerate() {
        let k = key(t);
        if find(mine, k).is_some() {
            continue;
        }
        match find(base, k) {
            None => {
                // After the nearest object before it (in their list) that is kept.
                let at = theirs[..i].iter().rev().find_map(|p| out.iter().position(|o| key(o) == key(p))).map_or(0, |p| p + 1);
                out.insert(at, t.clone());
            }
            Some(b) if b != *t => conflicts.push(Conflict { what: what.into(), id: k, name: name(t), detail: "you removed it, they changed it: left removed".into() }),
            _ => {}
        }
    }
    out
}

fn merge_sequence(b: &m::Sequence, mi: &m::Sequence, t: &m::Sequence, conflicts: &mut Vec<Conflict>, fixes: &mut Vec<String>) -> m::Sequence {
    let mut out = mi.clone();
    if mi.name != t.name && mi.name == b.name {
        out.name = t.name.clone();
    } else if mi.name != t.name && t.name != b.name {
        conflicts.push(Conflict { what: "sequence".into(), id: mi.id.0, name: mi.name.clone(), detail: format!("both renamed it (they: \"{}\"): kept yours", t.name) });
    }
    out.lanes = merge_list(&b.lanes, &mi.lanes, &t.lanes, |x| x.id.0, |x| x.name.clone(), "lane", conflicts);
    out.markers = merge_list(&b.markers, &mi.markers, &t.markers, |x| x.id.0, |x| x.name.clone(), "marker", conflicts);
    out.transitions = merge_list(&b.transitions, &mi.transitions, &t.transitions, |x| x.id.0, |x| x.kind.clone(), "transition", conflicts);

    // Main-lane positions are worked out from the order below, not merged.
    let spine = |s: &m::Sequence| s.lanes.iter().find(|l| l.kind == m::LaneKind::Spine).map(|l| l.id);
    let on_spine = |s: &m::Sequence, c: &m::Clip| spine(s) == Some(c.lane);
    let level = |s: &m::Sequence| -> Vec<m::Clip> {
        s.clips
            .iter()
            .map(|c| {
                let mut c = c.clone();
                if on_spine(s, &c) {
                    c.start = 0;
                }
                c
            })
            .collect()
    };
    let (lb, lm, lt) = (level(b), level(mi), level(t));
    let mut clips = merge_list(&lb, &lm, &lt, |x| x.id.0, |x| x.name.clone(), "clip", conflicts);

    // Lanes a kept clip still sits on stay.
    for c in &clips {
        if !out.lanes.iter().any(|l| l.id == c.lane)
            && let Some(l) = mi.lanes.iter().chain(&t.lanes).chain(&b.lanes).find(|l| l.id == c.lane)
        {
            let at = mi.lanes.iter().position(|x| x.id == l.id).unwrap_or(0).min(out.lanes.len());
            out.lanes.insert(at, l.clone());
            fixes.push(format!("kept lane \"{}\": clips still sit on it", l.name));
        }
    }

    // The spine in order: yours, with their reordering when you kept the
    // base order, and their new clips after the clip they follow.
    if let Some(sp) = spine(&out) {
        let order = |s: &m::Sequence| -> Vec<u64> {
            let mut v: Vec<&m::Clip> = s.clips.iter().filter(|c| Some(c.lane) == spine(s)).collect();
            v.sort_by_key(|c| c.start);
            v.iter().map(|c| c.id.0).collect()
        };
        let (ob, om, ot) = (order(b), order(mi), order(t));
        let kept: HashSet<u64> = clips.iter().filter(|c| c.lane == sp).map(|c| c.id.0).collect();
        let common = |o: &[u64], other: &[u64]| -> Vec<u64> { o.iter().copied().filter(|x| other.contains(x)).collect() };
        let mut seq: Vec<u64> = if common(&om, &ob) == common(&ob, &om) && common(&ot, &ob) != common(&ob, &ot) {
            // You kept the order; they changed it: theirs, with your additions.
            let mut s: Vec<u64> = ot.iter().copied().filter(|x| ob.contains(x)).collect();
            for (i, x) in om.iter().enumerate() {
                if !s.contains(x) {
                    let at = om[..i].iter().rev().find_map(|p| s.iter().position(|q| q == p)).map_or(0, |p| p + 1);
                    s.insert(at, *x);
                }
            }
            s
        } else {
            om.clone()
        };
        for (i, x) in ot.iter().enumerate() {
            if !seq.contains(x) && !ob.contains(x) {
                let at = ot[..i].iter().rev().find_map(|p| seq.iter().position(|q| q == p)).map_or(0, |p| p + 1);
                seq.insert(at, *x);
            }
        }
        // Clips moved onto the spine by the merge join at the end.
        for c in &clips {
            if c.lane == sp && !seq.contains(&c.id.0) {
                seq.push(c.id.0);
            }
        }
        seq.retain(|x| kept.contains(x));
        let mut at = 0;
        for id in seq {
            if let Some(c) = clips.iter_mut().find(|c| c.id.0 == id) {
                c.start = at;
                at += c.duration;
            }
        }
    }
    out.clips = clips;
    normalize(&mut out);
    out
}

/// The order the model keeps a sequence's lists in (as its own edits do).
fn normalize(s: &mut m::Sequence) {
    s.clips.sort_by_key(|c| (c.lane, c.start, c.id));
    s.transitions.sort_by_key(|t| t.id);
    s.markers.sort_by_key(|mk| (mk.time, mk.id));
    for c in &mut s.clips {
        c.markers.sort_by_key(|mk| (mk.time, mk.id));
    }
}

/// Makes the merged project valid: clips that landed on top of each other
/// on a free lane, transitions that no longer fit.
fn repair(p: &mut m::Project, mine: &m::Project, fixes: &mut Vec<String>) -> Result<(), String> {
    for _ in 0..256 {
        match p.check_invariants() {
            Ok(()) => return Ok(()),
            Err(m::Violation::Overlap(a, b)) => {
                // Move the one that is not as you had it.
                let seq_i = p.sequences.iter().position(|s| s.clips.iter().any(|c| c.id.0 == a)).ok_or("overlap in no sequence")?;
                let sid = p.sequences[seq_i].id;
                let unchanged = |id: u64| mine.sequence(sid).and_then(|s| s.clip(m::ClipId(id))) == p.sequences[seq_i].clip(m::ClipId(id));
                let mover = if unchanged(a) { b } else { a };
                let lane_id = m::LaneId(p.alloc_id());
                let s = &mut p.sequences[seq_i];
                let c = s.clips.iter().find(|c| c.id.0 == mover).ok_or("overlapping clip vanished")?;
                let from = s.lanes.iter().position(|l| l.id == c.lane).ok_or("overlapping clip on no lane")?;
                let mut lane = s.lanes[from].clone();
                lane.id = lane_id;
                lane.name = format!("{} (merged)", lane.name);
                // Visual lanes stack upward (lower index), sound lanes down.
                let at = if lane.kind == m::LaneKind::Audio { from + 1 } else { from };
                s.lanes.insert(at, lane);
                let name = {
                    let c = s.clips.iter_mut().find(|c| c.id.0 == mover).expect("found above");
                    c.lane = lane_id;
                    c.name.clone()
                };
                normalize(s);
                fixes.push(format!("clip #{mover} \"{name}\" would cover another clip: moved to a new lane"));
            }
            Err(m::Violation::BadTransition(id)) => {
                for s in &mut p.sequences {
                    s.transitions.retain(|t| t.id.0 != id);
                }
                fixes.push(format!("transition #{id} no longer fits its clips: removed"));
            }
            Err(v) => return Err(format!("the copies cannot be merged automatically: {v}")),
        }
    }
    Err("the merge could not be made valid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::{delta, testing};

    fn seq(p: &m::Project) -> m::SequenceId {
        p.active_sequence
    }

    #[test]
    fn separate_changes_both_survive() {
        let base = testing::project();
        let ids: Vec<m::ClipId> = base.active().clips.iter().map(|c| c.id).collect();
        // You: trim the first clip by a second (the spine ripples).
        let mut mine = base.clone();
        mine.apply(m::Edit::Trim { seq: seq(&mine), clip: ids[0], edge: m::Edge::End, delta: -1_000_000, ripple: true }).unwrap();
        // They: fade the last clip and remove the middle one.
        let mut theirs = base.clone();
        let mut c = theirs.active().clip(ids[2]).unwrap().clone();
        c.transform.opacity = 0.5;
        theirs.apply(m::Edit::SetClip { seq: seq(&theirs), clip: Box::new(c) }).unwrap();
        theirs.apply(m::Edit::RippleDelete { seq: seq(&theirs), clips: vec![ids[1]] }).unwrap();

        let o = three_way(&base, &mine, &theirs).unwrap();
        assert!(o.conflicts.is_empty(), "{:?}", o.conflicts);
        let s = o.merged.active();
        assert_eq!(s.clips.len(), 2);
        assert_eq!(s.clip(ids[0]).unwrap().duration, 2_000_000, "your trim");
        let last = s.clip(ids[2]).unwrap();
        assert_eq!(last.transform.opacity, 0.5, "their fade");
        assert_eq!(last.start, 2_000_000, "packed after your shorter first clip");
        // Taking it in is a plain batch of edits.
        let mut x = mine.clone();
        x.apply(m::Edit::Batch { edits: delta::edits_between(&mine, &o.merged) }).unwrap();
        assert!(x.content_eq(&o.merged));
    }

    #[test]
    fn same_property_both_ways_is_a_conflict_and_yours_wins() {
        let base = testing::project();
        let id = base.active().clips[0].id;
        let set = |p: &mut m::Project, op: f64, rot: f64| {
            let mut c = p.active().clip(id).unwrap().clone();
            c.transform.opacity = op;
            c.transform.rotation += rot;
            let s = p.active_sequence;
            p.apply(m::Edit::SetClip { seq: s, clip: Box::new(c) }).unwrap();
        };
        let (mut mine, mut theirs) = (base.clone(), base.clone());
        set(&mut mine, 0.2, 0.0);
        set(&mut theirs, 0.9, 15.0);
        let o = three_way(&base, &mine, &theirs).unwrap();
        assert_eq!(o.conflicts.len(), 1);
        assert!(o.conflicts[0].detail.contains("transform.opacity"), "{:?}", o.conflicts);
        let c = o.merged.active().clip(id).unwrap();
        assert_eq!(c.transform.opacity, 0.2, "yours");
        assert_eq!(c.transform.rotation, base.active().clip(id).unwrap().transform.rotation + 15.0, "their rotation, which you did not touch");
    }

    #[test]
    fn new_clips_from_both_sides_with_clashing_ids_and_places() {
        let base = testing::project();
        let mut mine = crate::agent::Session::new(crate::doc::Doc::new(base.clone()));
        mine.run("edit", &serde_json::json!({"ops": [{"op": "card", "colour": "#ffffff", "at": 9, "duration": 2}, {"op": "text", "text": "Hello", "at": 1, "duration": 2}]})).unwrap();
        let mut theirs = crate::agent::Session::new(crate::doc::Doc::new(base.clone()));
        theirs.run("edit", &serde_json::json!({"ops": [{"op": "text", "text": "World", "at": 1.5, "duration": 2}]})).unwrap();
        let (mine, theirs) = (mine.doc.model.clone(), theirs.doc.model.clone());
        let o = three_way(&base, &mine, &theirs).unwrap();
        let s = o.merged.active();
        let names: Vec<String> = s.clips.iter().map(|c| c.name.clone()).collect();
        assert_eq!(s.clips.len(), mine.active().clips.len() + 1, "{names:?}");
        assert!(o.fixes.iter().any(|f| f.contains("new lane")), "the two titles overlap: {:?}", o.fixes);
        o.merged.check_invariants().unwrap();
    }
}
