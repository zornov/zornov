// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `edit_schema`: the raw edits, and the fields of every simple op.
//!
//! * no arguments: the raw model edits (for `raw` ops);
//! * `op` (a name) or `ops` (names, or whole ops): what each takes; an op
//!   written out is checked (aliases mapped, unknown fields refused)
//!   without being run;
//! * `long`: every op with all its fields, then the raw edits.

use super::{Content, EDIT_SCHEMA, args, field_docs};
use serde_json::{Value, json};

fn fields_of(name: &str) -> Result<Vec<&'static str>, String> {
    args::op_fields(name).map(|f| f.to_vec()).ok_or_else(|| {
        let all = field_docs::all_ops();
        let hint = args::closest(name, &all).map(|c| format!(" (did you mean \"{c}\"?)")).unwrap_or_default();
        format!("no op \"{name}\"{hint}; ops: {}", all.join(", "))
    })
}

pub(super) fn edit_schema(a: &Value) -> Result<Vec<Content>, String> {
    let flag = |keys: &[&str]| keys.iter().any(|k| a.get(*k).is_some_and(|v| v.as_bool() == Some(true) || v.as_str().is_some_and(|s| ["true", "long", "full"].iter().any(|w| s.eq_ignore_ascii_case(w)))));
    let mut asked: Vec<Value> = match a.get("ops") {
        Some(Value::Array(l)) => l.clone(),
        Some(Value::String(s)) => s.split([',', ' ']).filter(|w| !w.is_empty()).map(|w| json!(w)).collect(),
        Some(v @ Value::Object(_)) => vec![v.clone()],
        _ => Vec::new(),
    };
    if let Some(op) = a.get("op").and_then(Value::as_str) {
        asked.push(json!(op));
    }
    if !asked.is_empty() {
        let mut out = Vec::new();
        for item in &asked {
            let item = args::unwrap_op(item);
            match &item {
                // Each field with its type, units, range and meaning, and
                // an example call (bare names left agents guessing units).
                Value::String(name) => out.push((name.clone(), field_docs::op_doc(name, &fields_of(name)?))),
                Value::Object(o) => {
                    let name = o.get("op").and_then(Value::as_str).ok_or("each entry of ops is an op name or {\"op\": name, ...fields}")?.to_owned();
                    let allowed = fields_of(&name)?;
                    let mut check = item.clone();
                    let verdict = match args::normalise(&name, &format!("op {name}"), &mut check, &allowed) {
                        Ok(()) => json!({"ok": true, "fields": allowed}),
                        Err(e) => json!({"ok": false, "problem": e}),
                    };
                    out.push((name, verdict));
                }
                _ => return Err("each entry of ops is an op name or {\"op\": name, ...fields}".into()),
            }
        }
        // One entry per op written out, in order (a repeated name gets a number).
        let mut map = serde_json::Map::new();
        for (name, v) in out {
            let mut key = name.clone();
            let mut n = 1;
            while map.contains_key(&key) {
                n += 1;
                key = format!("{name}#{n}");
            }
            map.insert(key, v);
        }
        return Ok(vec![Content::Text(Value::Object(map).to_string())]);
    }
    if flag(&["long", "full", "detail", "details", "verbose", "all"]) {
        let mut s = String::from("Edit ops, used inside edit {\"ops\": [{\"op\": name, ...}]} (name: the fields it takes):\n");
        for op in field_docs::all_ops() {
            if let Some(f) = args::op_fields(op) {
                s.push_str(&format!("{op}: {}\n", f.join(", ")));
            }
        }
        s.push('\n');
        s.push_str(EDIT_SCHEMA);
        return Ok(vec![Content::Text(s)]);
    }
    Ok(vec![Content::Text(format!("{EDIT_SCHEMA}\n\nOps (edit_schema {{\"op\": name}} gives each field's type, units and range, and an example): {}", field_docs::all_ops().join(", ")))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::services;

    fn said(r: Vec<Content>) -> String {
        r.into_iter().map(|c| if let Content::Text(t) = c { t } else { String::new() }).collect()
    }

    fn session_with_clip() -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Schema", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 100, "detail": 1, "duration": 4})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s, id)
    }

    #[test]
    fn edit_schema_takes_ops_as_a_list_a_name_or_written_out_and_has_a_long_form() {
        let (mut s, _) = session_with_clip();
        let short = said(s.run("edit_schema", &json!({})).unwrap());
        assert!(short.starts_with("Raw model edits"), "{short}");
        // `ops` as names: the fields each takes.
        let v: Value = serde_json::from_str(&said(s.run("edit_schema", &json!({"ops": ["keyframe", "keys"]})).unwrap())).unwrap();
        assert!(v["keyframe"]["fields"].get("value").is_some() && v["keys"]["fields"].get("remove").is_some(), "{v}");
        // `ops` as one string, and `op` for one.
        let v: Value = serde_json::from_str(&said(s.run("edit_schema", &json!({"ops": "pose, act"})).unwrap())).unwrap();
        assert!(v.get("pose").is_some() && v.get("act").is_some(), "{v}");
        let v: Value = serde_json::from_str(&said(s.run("edit_schema", &json!({"op": "animate"})).unwrap())).unwrap();
        assert!(v["animate"]["fields"].get("keys").is_some());
        // Each field says what it is, in which units; an example comes with it.
        let v: Value = serde_json::from_str(&said(s.run("edit_schema", &json!({"op": "act"})).unwrap())).unwrap();
        assert!(v["act"]["fields"]["speed"]["range"].as_str().unwrap().contains("0.1 to 6"), "{v}");
        assert!(v["act"]["fields"]["at"]["units"].as_str().unwrap().contains("clip"), "{v}");
        // Ops the dispatcher runs are all listed, motion graphics too.
        let all = super::field_docs::all_ops();
        for op in ["chart", "shape", "template", "body", "throw", "build_place", "act"] {
            assert!(all.contains(&op), "{op} missing from {all:?}");
        }
        // An op written out is checked, not run.
        let before = s.doc.history.revision();
        let v: Value = serde_json::from_str(&said(s.run("edit_schema", &json!({"ops": [{"op": "keyframe", "clip": 1, "property": "x", "at": 1, "value": "5"}, {"op": "keyframe", "clip": 1, "bogus": 1}]})).unwrap())).unwrap();
        assert_eq!(v["keyframe"]["ok"], json!(true), "{v}");
        assert_eq!(v["keyframe#2"]["ok"], json!(false), "{v}");
        assert!(v["keyframe#2"]["problem"].as_str().unwrap().contains("bogus"));
        assert_eq!(s.doc.history.revision(), before);
        // An unknown op says what exists.
        assert!(s.run("edit_schema", &json!({"ops": ["nonsense"]})).err().unwrap().contains("ops:"));
        // The long form lists every op's fields, then the raw edits.
        let long = said(s.run("edit_schema", &json!({"long": true})).unwrap());
        assert!(long.contains("place: media, at, lane") && long.contains("Raw model edits"), "{}", &long[..200.min(long.len())]);
    }

    #[test]
    fn a_keyframe_value_may_be_a_string_number_and_keys_remove_true_means_all() {
        let (mut s, id) = session_with_clip();
        s.edit(&[json!({"op": "keyframe", "clip": id, "property": "transform.scale_x", "time": 0, "value": "1.5"}), json!({"op": "keyframe", "clip": id, "property": "transform.scale_x", "time": 1, "value": 2})], "k", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.keyframes["transform.scale_x"].keys.len(), 2);
        assert_eq!(mc.keyframes["transform.scale_x"].keys[0].value.as_f64(), Some(1.5));
        assert!(s.edit(&[json!({"op": "keyframe", "clip": id, "property": "transform.scale_x", "time": 2, "value": "tall"})], "k", false).is_err());
        s.edit(&[json!({"op": "keys", "clip": id, "property": "scale_x", "remove": true})], "rm", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.get("transform.scale_x").is_none_or(|t| t.keys.is_empty()));
    }
}
