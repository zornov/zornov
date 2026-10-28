// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clips the way an editor talks about them. Wherever an op takes `clip`
//! or `clips`, these all work:
//!
//! - `12` — the clip with that id
//! - `"$1"`, `"$2"`… — the first, second… clip made earlier in the same
//!   `edit` call; `"$last"` (or `"last"`) the latest one
//! - `"@4.2"` — the main-lane clip playing at 4.2 s; `"music@4.2"`,
//!   `"overlay@4.2"`, `"titles@4.2"` — on that lane
//! - `"Night market"` — the clip with that name (or a unique part of it)
//! - in a `clips` list: `"all"`, `"lane:main"` (every clip on a lane), or
//!   `{"lane": "main", "from": 2, "to": 8, "kind": "video"}` (any of those
//!   fields; clips overlapping the range)

use super::craft::lane_of;
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};

fn secs(t: view::Time) -> f64 {
    t as f64 / SEC as f64
}

fn one(p: &view::Project, made: &[u32], v: &Value) -> Result<Vec<u32>, String> {
    if let Some(id) = v.as_u64() {
        return Ok(vec![id as u32]);
    }
    if let Value::Object(o) = v {
        let lane = match o.get("lane") {
            Some(l) => Some(lane_of(p, l, None)?),
            None => None,
        };
        let from = o.get("from").and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY);
        let to = o.get("to").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
        let kind = o.get("kind").and_then(Value::as_str).map(str::to_lowercase);
        let mut cs: Vec<&view::Clip> = p
            .clips
            .iter()
            .filter(|c| lane.is_none_or(|l| c.lane == l))
            .filter(|c| secs(c.start) < to && secs(c.end()) > from)
            .filter(|c| {
                kind.as_deref().is_none_or(|k| match k {
                    "video" => c.kind == ClipKind::Video,
                    "audio" | "sound" => c.kind == ClipKind::Audio,
                    "title" | "text" => c.kind == ClipKind::Title,
                    "fx" | "adjustment" => c.kind == ClipKind::Fx,
                    _ => false,
                })
            })
            .collect();
        cs.sort_by_key(|c| (c.start, c.lane));
        if cs.is_empty() {
            return Err(format!("no clips match {}", Value::Object(o.clone())));
        }
        return Ok(cs.iter().map(|c| c.id).collect());
    }
    let s = v.as_str().ok_or_else(|| format!("{v} is not a clip (an id, \"$1\", \"@4.2\", \"lane@4.2\" or a name)"))?.trim();
    let low = s.to_lowercase();
    if low == "all" {
        return Ok(p.clips.iter().map(|c| c.id).collect());
    }
    if let Some(l) = low.strip_prefix("lane:") {
        let lane = lane_of(p, &json!(l), None)?;
        let mut cs: Vec<&view::Clip> = p.clips.iter().filter(|c| c.lane == lane).collect();
        cs.sort_by_key(|c| c.start);
        return Ok(cs.iter().map(|c| c.id).collect());
    }
    if low == "last" || low == "$last" {
        return made.last().map(|&i| vec![i]).ok_or_else(|| "\"$last\" means the latest clip made in this edit, and none was made yet".into());
    }
    if let Some(n) = low.strip_prefix('$') {
        let n: usize = n.parse().map_err(|_| format!("\"{s}\": use $1, $2… or $last"))?;
        // $n counts clips made earlier in this edit, not ops: an op that
        // makes three clips (a title with its backing, a scene) takes three
        // numbers, and an op that makes none takes none.
        return made.get(n.wrapping_sub(1)).map(|&i| vec![i]).ok_or_else(|| format!("\"{s}\": $n counts the clips made earlier in this edit (the nth new clip), not the ops, and only {} were made so far{}; use $last for the newest, or the id from an earlier answer", made.len(), if made.is_empty() { String::new() } else { format!(" ({})", made.iter().enumerate().map(|(i, c)| format!("${} = clip {c}", i + 1)).collect::<Vec<_>>().join(", ")) }));
    }
    if let Some((lane, t)) = low.rsplit_once('@') {
        let t: f64 = t.trim().parse().map_err(|_| format!("\"{s}\": the part after @ is a time in seconds"))?;
        let lane = lane_of(p, &json!(if lane.is_empty() { "main" } else { lane }), None)?;
        let at = (t * SEC as f64) as view::Time;
        return p
            .clips
            .iter()
            .filter(|c| c.lane == lane && c.start <= at && at < c.end())
            .map(|c| vec![c.id])
            .next()
            .ok_or_else(|| format!("nothing on that lane at {t}s"));
    }
    if let Ok(id) = low.trim_start_matches('#').parse::<u32>() {
        return Ok(vec![id]);
    }
    let exact: Vec<u32> = p.clips.iter().filter(|c| c.name.to_lowercase() == low || c.text.to_lowercase() == low).map(|c| c.id).collect();
    let hits = if exact.is_empty() { p.clips.iter().filter(|c| c.name.to_lowercase().contains(&low) || c.text.to_lowercase().contains(&low)).map(|c| c.id).collect() } else { exact };
    match hits.len() {
        0 => Err(format!("no clip called \"{s}\" (ids, \"@time\", \"lane@time\", \"$1\" also work)")),
        1 => Ok(hits),
        _ => Err(format!("\"{s}\" names {} clips ({}); use an id or \"lane@time\"", hits.len(), hits.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "))),
    }
}

/// Replaces every clip reference in an op with ids.
pub fn resolve_refs(p: &view::Project, made: &[u32], op: &mut Value) -> Result<(), String> {
    let Some(o) = op.as_object_mut() else { return Ok(()) };
    let name = o.get("op").and_then(Value::as_str).unwrap_or("").to_owned();
    // Fields naming one clip ("from" only where it is a clip).
    let singles: &[&str] = match name.as_str() {
        "copy_look" => &["clip", "from"],
        "reorder" => &["clip", "before"],
        "parent" => &["clip", "to"],
        "matte" => &["clip", "with"],
        "restack" => &["clip", "in_front_of", "behind"],
        "card" => &["clip", "behind"],
        "track" => &["clip", "attach"],
        "match_colour" => &["clip", "to"],
        "foreground" => &["clip", "backdrop"],
        _ => &["clip"],
    };
    // Points that may name a clip instead ([x, y, z] stays a point).
    if matches!(name.as_str(), "camera" | "light" | "three_d") {
        for k in ["look_at", "focus"] {
            if let Some(v @ Value::String(_)) = o.get(k).cloned() {
                let ids = one(p, made, &v)?;
                o.insert(k.into(), json!({"clip": ids.first().copied().ok_or_else(|| format!("{v} names no clip"))?}));
            }
        }
        if let Some(mv) = o.get_mut("move")
            && let Some(v @ Value::String(_)) = mv.get("around").cloned()
        {
            let ids = one(p, made, &v)?;
            mv["around"] = json!({"clip": ids.first().copied().ok_or_else(|| format!("{v} names no clip"))?});
        }
    }
    // A driver's sources name clips inside `from` and `vars`.
    if name == "driver" {
        let fix = |spec: &mut Value| -> Result<(), String> {
            if let Some(v) = spec.get("clip").cloned().filter(|v| !v.is_u64() && !v.is_null()) {
                let ids = one(p, made, &v)?;
                spec["clip"] = json!(ids.first().copied().ok_or_else(|| format!("{v} names no clip"))?);
            }
            Ok(())
        };
        if let Some(f) = o.get_mut("from") {
            fix(f)?;
        }
        if let Some(Value::Object(vs)) = o.get_mut("vars") {
            for v in vs.values_mut() {
                fix(v)?;
            }
        }
    }
    for (op_name, key) in [("physics", "static"), ("foreground", "hides")] {
        if name == op_name
            && let Some(Value::Array(list)) = o.get(key).cloned()
        {
            let mut out: Vec<u32> = Vec::new();
            for x in &list {
                out.extend(one(p, made, x)?);
            }
            o.insert(key.into(), json!(out));
        }
    }
    for &k in singles {
        if let Some(v) = o.get(k).cloned() {
            // null lets go (parent "to": null); numbers are ids already.
            if !v.is_u64() && !v.is_null() {
                let ids = one(p, made, &v)?;
                if ids.len() != 1 {
                    return Err(format!("\"{k}\" picks one clip but {v} matches {}; use \"clips\"", ids.len()));
                }
                o.insert(k.into(), json!(ids[0]));
            }
        }
    }
    if let Some(v) = o.get("clips").cloned() {
        let list = match v {
            Value::Array(a) => a,
            other => vec![other],
        };
        let mut out: Vec<u32> = Vec::new();
        for x in &list {
            for id in one(p, made, x)? {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        o.insert("clips".into(), json!(out));
    }
    Ok(())
}
