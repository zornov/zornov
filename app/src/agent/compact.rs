// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tool answers, trimmed of what the AI cannot use.
//!
//! Everything a tool returns stays in the AI's context for the rest of the
//! job, so a reply carries what was asked for and nothing twice:
//!
//! * numbers are rounded to what they mean (`0.44999998807907104` is
//!   `0.45`), which is several tokens each;
//! * an `edit` reply that touched many clips shows the first few in full
//!   and the rest as a short line each (id, lane, kind, name, time), with
//!   long lists of keyed properties cut to their first names; the full
//!   line of any clip is one `inspect` away;
//! * notes that say the same thing again are said once, with a count.
//!
//! Nothing here changes what happened, only how much of it is repeated.

use super::Content;
use serde_json::{Value, json};

/// Significant digits kept in a number (an `f32` is good for about seven).
const DIGITS: i32 = 7;
/// Clips an `edit` reply shows in full, per list; the rest are short lines.
pub const FULL_LINES: usize = 8;
/// Keyed property names shown on a clip line before "+N more".
const KEYS_SHOWN: usize = 4;

/// A number rounded to [`DIGITS`] significant digits.
fn round_sig(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() || x.fract() == 0.0 {
        return x;
    }
    let scale = 10f64.powi(DIGITS - 1 - x.abs().log10().floor() as i32);
    let r = (x * scale).round() / scale;
    if r.is_finite() { r } else { x }
}

/// `v` with every non-whole number rounded.
pub fn round_numbers(v: &mut Value) {
    match v {
        Value::Number(n) if n.is_f64() => {
            if let Some(r) = n.as_f64().map(round_sig).and_then(serde_json::Number::from_f64) {
                *v = Value::Number(r);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(round_numbers),
        Value::Object(o) => o.values_mut().for_each(round_numbers),
        _ => {}
    }
}

/// A clip line cut to who and when.
fn short_line(line: &Value) -> Value {
    let mut o = serde_json::Map::new();
    for k in ["id", "lane", "kind", "name", "start", "dur"] {
        if let Some(v) = line.get(k) {
            o.insert(k.into(), v.clone());
        }
    }
    Value::Object(o)
}

/// A clip line with a long `animated` list cut to its first names.
fn trim_keys(line: &mut Value) {
    if let Some(a) = line.get_mut("animated").and_then(Value::as_array_mut) {
        if a.len() > KEYS_SHOWN + 1 {
            let more = a.len() - KEYS_SHOWN;
            a.truncate(KEYS_SHOWN);
            a.push(json!(format!("+{more} more (inspect lists them)")));
        }
    }
}

/// The clip lines of an `edit` reply's `changed`: the first [`FULL_LINES`]
/// of each list in full, the rest short.
fn tier_lines(list: &mut Value) {
    let Some(a) = list.as_array_mut() else { return };
    for (i, line) in a.iter_mut().enumerate() {
        if i < FULL_LINES {
            trim_keys(line);
        } else {
            *line = short_line(line);
        }
    }
}

/// Notes with repeats said once: `"moved"` three times becomes
/// `"moved (x3)"`, in the place of the first.
fn dedupe_notes(notes: &mut Vec<Value>) {
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut order: Vec<usize> = Vec::new();
    for n in notes.iter() {
        let s = n.as_str().map_or_else(|| n.to_string(), str::to_owned);
        match seen.iter().position(|(t, _)| *t == s) {
            Some(i) => seen[i].1 += 1,
            None => {
                order.push(seen.len());
                seen.push((s, 1));
            }
        }
    }
    if seen.len() == notes.len() {
        return;
    }
    *notes = order.into_iter().map(|i| json!(if seen[i].1 > 1 { format!("{} (x{})", seen[i].0, seen[i].1) } else { seen[i].0.clone() })).collect();
}

/// A list this long or longer is said in full once per reply.
const LONG_LIST: usize = 3;

/// `v` with empty fields dropped, and every list already said in this
/// reply under the same name (`seen`: the last one said) cut to a pointer
/// back to it, or to the pointer and what is new since.
fn shorten_said(v: &mut Value, seen: &mut Vec<(String, Vec<Value>)>) {
    match v {
        Value::Object(o) => {
            o.retain(|_, x| !x.is_null());
            for (k, x) in o.iter_mut() {
                match x {
                    Value::Array(a) if a.len() >= LONG_LIST => {
                        let now = a.clone();
                        match seen.iter_mut().find(|(n, _)| n == k) {
                            Some((_, last)) => {
                                if *last == now {
                                    *x = json!(format!("same {} {k} as above", now.len()));
                                } else if now.len() > last.len() && last.iter().all(|i| now.contains(i)) {
                                    let mut short = vec![json!(format!("the {} {k} above, and", last.len()))];
                                    short.extend(now.iter().filter(|i| !last.contains(i)).cloned());
                                    *x = Value::Array(short);
                                }
                                *last = now;
                            }
                            None => seen.push((k.clone(), now)),
                        }
                    }
                    _ => shorten_said(x, seen),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| shorten_said(x, seen)),
        _ => {}
    }
}

/// Notes with the JSON a note carries (a model's or a rig's summary) cut
/// to what was not said before in the same reply: a rig's 27 bones and 34
/// shape keys are listed once, not again after every constraint added.
fn shorten_notes(notes: &mut [Value]) {
    let mut seen = Vec::new();
    for n in notes.iter_mut() {
        let Some(s) = n.as_str() else { continue };
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            let mut it = serde_json::Deserializer::from_str(&rest[i..]).into_iter::<Value>();
            match it.next() {
                Some(Ok(mut v)) if v.is_object() => {
                    shorten_said(&mut v, &mut seen);
                    out.push_str(&v.to_string());
                    rest = &rest[i + it.byte_offset()..];
                }
                _ => {
                    out.push('{');
                    rest = &rest[i + 1..];
                }
            }
        }
        out.push_str(rest);
        if out.len() < s.len() {
            *n = json!(out);
        }
    }
}

/// `read_back` lines cut to what the `changed` lines of the same reply did
/// not already say: a clip's name, start and length are given once.
fn trim_read_back(v: &mut Value, key: &str) {
    let mut said: Vec<Value> = Vec::new();
    for k in ["added", "changed"] {
        said.extend(v[key][k].as_array().into_iter().flatten().cloned());
    }
    let Some(lines) = v.get_mut("read_back").and_then(|r| r.get_mut("clips")).and_then(Value::as_array_mut) else { return };
    for line in lines.iter_mut() {
        let Some(c) = said.iter().find(|c| c.get("id").is_some() && c.get("id") == line.get("clip")) else { continue };
        if let Some(o) = line.as_object_mut() {
            for f in ["name", "start", "dur"] {
                if o.get(f).is_some() && o.get(f) == c.get(f) {
                    o.remove(f);
                }
            }
        }
    }
}

/// The `edit` reply, trimmed.
pub fn edit_reply(v: &mut Value) {
    let key = if v.get("changed").is_some() { "changed" } else { "would_change" };
    trim_read_back(v, key);
    if let Some(c) = v.get_mut(key) {
        for k in ["added", "changed"] {
            if let Some(l) = c.get_mut(k) {
                tier_lines(l);
            }
        }
    }
    if let Some(n) = v.get_mut("notes").and_then(Value::as_array_mut) {
        dedupe_notes(n);
        shorten_notes(n);
    }
}

/// One tool's text answer, trimmed. Text that is not a JSON object or list
/// is left as it is.
pub fn text(tool: &str, t: &str) -> Option<String> {
    let first = t.trim_start().as_bytes().first()?;
    if *first != b'{' && *first != b'[' {
        return None;
    }
    let mut v: Value = serde_json::from_str(t).ok()?;
    if tool == "edit" {
        edit_reply(&mut v);
    }
    round_numbers(&mut v);
    let out = v.to_string();
    (out.len() < t.len()).then_some(out)
}

/// The parts of one tool answer, trimmed.
pub fn parts(tool: &str, parts: &mut [Content]) {
    for p in parts {
        if let Content::Text(t) = p {
            if let Some(short) = text(tool, t) {
                *t = short;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lose_float_noise_but_keep_their_meaning() {
        assert_eq!(round_sig(0.44999998807907104), 0.45);
        assert_eq!(round_sig(1.4000000953674316), 1.4);
        assert_eq!(round_sig(320.12345678), 320.1235);
        assert_eq!(round_sig(0.000123456789), 0.0001234568);
        assert_eq!(round_sig(12.0), 12.0);
        let mut v = json!({"a": [0.1 + 0.2, 7], "b": {"c": 2.5}});
        round_numbers(&mut v);
        assert_eq!(v.to_string(), r#"{"a":[0.3,7],"b":{"c":2.5}}"#);
    }

    #[test]
    fn a_big_edit_reply_shows_a_few_clips_in_full_and_the_rest_short() {
        let line = |i: u64| json!({"id": i, "lane": 3, "kind": "audio", "name": "Bark", "start": i as f64 * 0.5, "dur": 0.4, "media": 8, "source_in": 0.065, "speed": 0.44999998807907104, "volume": -5.0, "animated": ["a", "b", "c", "d", "e", "f", "g"]});
        let added: Vec<Value> = (1..=40).map(line).collect();
        let reply = json!({"ok": true, "revision": 5, "changed": {"added": added, "removed": [], "changed": []}, "notes": ["moved", "moved", "placed", "moved"], "duration": 20.0});
        let before = reply.to_string();
        let after = text("edit", &before).expect("shorter");
        assert!(after.len() * 10 < before.len() * 6, "{} -> {}", before.len(), after.len());
        let v: Value = serde_json::from_str(&after).unwrap();
        let a = v["changed"]["added"].as_array().unwrap();
        assert_eq!(a.len(), 40, "every clip is still named");
        assert!(a[0].get("media").is_some() && a[0]["animated"].as_array().unwrap().len() == KEYS_SHOWN + 1);
        assert!(a[FULL_LINES].get("media").is_none() && a[FULL_LINES]["id"] == 9);
        assert_eq!(v["notes"], json!(["moved (x3)", "placed"]));
        assert_eq!(a[0]["speed"], 0.45);
    }

    #[test]
    fn a_rig_summary_repeated_after_each_rig_op_is_listed_once() {
        let bones: Vec<String> = (0..27).map(|i| format!("bone{i}")).collect();
        let keys: Vec<String> = (0..34).map(|i| format!("key{i}")).collect();
        let rig = |c: usize| json!({"bones": bones, "skinned": true, "shape_keys": keys, "constraints": (0..c).map(|i| json!({"name": format!("ik{i}")})).collect::<Vec<_>>(), "rig": null});
        let notes: Vec<Value> = (0..5).map(|c| json!(format!("clip 7: ik constraint \"ik{c}\"; {}", rig(c)))).collect();
        let reply = json!({"ok": true, "changed": {"added": [], "changed": []}, "notes": notes});
        let before = reply.to_string();
        let after = text("edit", &before).expect("shorter");
        assert!(after.len() * 2 < before.len(), "{} -> {}", before.len(), after.len());
        let v: Value = serde_json::from_str(&after).unwrap();
        let n: Vec<&str> = v["notes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(n[0].contains("bone26") && n[0].contains("key33"), "the first note lists them: {}", n[0]);
        assert!(n[4].contains("same 27 bones as above") && n[4].contains("same 34 shape_keys as above"), "{}", n[4]);
        // The constraints grow by one a note: each says only the new one.
        assert!(n[4].contains("the 3 constraints above, and") && n[4].contains("ik3") && !n[4].contains("ik2"), "{}", n[4]);
        assert!(!after.contains("null"), "empty fields are dropped");
        // A note that is not JSON, or has a stray brace, stays as it was.
        let mut odd = vec![json!("set {x} to 4"), json!("made {\"a\": 1} and {broken")];
        shorten_notes(&mut odd);
        assert_eq!(odd, vec![json!("set {x} to 4"), json!("made {\"a\":1} and {broken")]);
    }

    #[test]
    fn read_back_does_not_repeat_the_changed_line() {
        let reply = json!({"ok": true, "changed": {"added": [{"id": 9, "lane": 4, "kind": "model", "name": "Lamp post", "start": 0.0, "dur": 45.0}], "changed": []},
            "read_back": {"clips": [{"clip": 9, "name": "Lamp post", "lane": "Street", "start": 0.0, "dur": 45.0, "keys": ["z: 2 keys"]}, {"clip": 3, "name": "Other", "lane": "A", "start": 1.0, "dur": 2.0}]}});
        let v: Value = serde_json::from_str(&text("edit", &reply.to_string()).expect("shorter")).unwrap();
        assert_eq!(v["read_back"]["clips"][0], json!({"clip": 9, "lane": "Street", "keys": ["z: 2 keys"]}));
        assert_eq!(v["read_back"]["clips"][1]["name"], "Other", "a clip the changed lines do not show keeps its line");
    }

    #[test]
    fn plain_text_and_small_replies_are_left_alone() {
        assert!(text("script", "OVERLAY\n  1.00  #3 card").is_none());
        assert!(text("edit", r#"{"ok":true,"notes":["a"]}"#).is_none());
        assert!(text("guide", "{broken").is_none());
    }
}
