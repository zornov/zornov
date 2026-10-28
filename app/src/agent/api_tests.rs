// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tool interface as agents met it in recorded evals: the names they
//! guessed, the units they got wrong and the fields the guides promised.
//! Each test is one failure that cost a run.

use super::{Content, Session};
use crate::services;
use serde_json::{Value, json};
use studio_model as m;

fn session() -> Session {
    Session::new(services::project::new_custom("Api", 640, 360, 30.0))
}

fn card(s: &mut Session, at: f64, duration: f64) -> u32 {
    s.edit(&[json!({"op": "card", "colour": "#336699", "at": at, "duration": duration, "lane": "overlay"})], "card", false).unwrap();
    s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
}

fn said(r: Vec<Content>) -> String {
    r.into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect::<Vec<_>>().join("\n")
}

fn err(r: Result<Vec<Content>, String>) -> String {
    match r {
        Ok(p) => panic!("expected an error, got {}", said(p)),
        Err(e) => e,
    }
}

fn keys(s: &Session, id: u32, path: &str) -> Vec<(i64, m::Value)> {
    let mc = s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap();
    mc.keyframes.get(path).map(|t| t.keys.iter().map(|k| (k.time, k.value.clone())).collect()).unwrap_or_default()
}

fn scalar(v: &m::Value) -> f64 {
    match v {
        m::Value::Scalar(x) | m::Value::Angle(x) => *x,
        other => panic!("not a number: {other:?}"),
    }
}

/// `keyframe {property: "scale"}` said "Not found: property `scale`" while
/// `set` and `animate` took it: keyframe now takes the same names and units.
#[test]
fn keyframe_takes_the_friendly_names_and_units() {
    let mut s = session();
    let id = card(&mut s, 0.0, 4.0);
    s.edit(&[json!({"op": "keyframe", "clip": id, "property": "scale", "time": 1.0, "value": 150})], "k", false).unwrap();
    let k = keys(&s, id, "transform.scale_x");
    assert_eq!(k.len(), 1, "scale keys both axes");
    assert!((scalar(&k[0].1) - 1.5).abs() < 1e-6, "150 % is a factor of 1.5: {k:?}");
    assert_eq!(keys(&s, id, "transform.scale_y").len(), 1);
    s.edit(&[json!({"op": "keyframe", "clip": id, "property": "opacity", "time": 2.0, "value": 50})], "k", false).unwrap();
    let o = keys(&s, id, "transform.opacity");
    assert!((scalar(&o[0].1) - 0.5).abs() < 1e-6, "opacity in percent: {o:?}");
    // Other editors' names mean the same.
    s.edit(&[json!({"op": "keyframe", "clip": id, "property": "transform.scale", "time": 3.0, "value": 120})], "k", false).unwrap();
    assert_eq!(keys(&s, id, "transform.scale_x").len(), 2);
    // A model path the clip has is keyed as before, in model units.
    s.edit(&[json!({"op": "keyframe", "clip": id, "property": "transform.opacity", "time": 3.0, "value": 0.25})], "k", false).unwrap();
    assert!(keys(&s, id, "transform.opacity").iter().any(|(_, v)| (scalar(v) - 0.25).abs() < 1e-6));
}

/// A misspelt property is answered with the closest name and the units.
#[test]
fn an_unknown_property_says_what_was_meant() {
    let mut s = session();
    let id = card(&mut s, 0.0, 4.0);
    let e = s.edit(&[json!({"op": "animate", "clip": id, "property": "scael", "keys": [[0, 100], [1, 120]]})], "a", false).unwrap_err();
    assert!(e.contains("did you mean \"scale\""), "{e}");
    assert!(e.contains("percent"), "the units come with it: {e}");
    let e = s.edit(&[json!({"op": "animate", "clip": id, "property": "transform.position", "keys": [[0, 1], [1, 2]]})], "a", false).unwrap_err();
    assert!(e.contains("x and y"), "{e}");
    // position.x is x.
    s.edit(&[json!({"op": "animate", "clip": id, "property": "position.x", "keys": [[0, -100], [1, 100]]})], "a", false).unwrap();
}

/// Keys given in timeline seconds on a clip that starts later: the error
/// names the fix instead of only the range.
#[test]
fn keys_in_timeline_time_are_explained() {
    let mut s = session();
    let id = card(&mut s, 5.0, 3.0);
    let e = s.edit(&[json!({"op": "animate", "clip": id, "property": "opacity", "keys": [[5.5, 0], [7.5, 100]]})], "a", false).unwrap_err();
    assert!(e.contains("\"timeline\": true"), "{e}");
    s.edit(&[json!({"op": "animate", "clip": id, "property": "opacity", "keys": [[5.5, 0], [7.5, 100]], "timeline": true})], "a", false).unwrap();
}

/// The colour task scored 0: `curves` and `curve_r` were refused with no
/// list. They are the curve parameters now, and a wrong name gets the list.
#[test]
fn grade_parameters_take_common_names_and_list_the_rest() {
    let mut s = session();
    let id = card(&mut s, 0.0, 2.0);
    s.run("colour", &json!({"action": "grade", "clip": id, "params": {"curve_r": [[0, 0], [0.5, 0.45], [1, 1]], "curves": [[0, 0.02], [1, 1]]}})).unwrap();
    let g = s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap().grade.clone().unwrap();
    assert!(g.params.contains_key("curve_red") && g.params.contains_key("curve_master"), "{:?}", g.params.keys().collect::<Vec<_>>());
    let e = err(s.run("colour", &json!({"action": "grade", "clip": id, "params": {"exposre": 0.5}})));
    assert!(e.contains("did you mean \"exposure\"") && e.contains("curve_red"), "{e}");
}

/// The guide says `skill {do: "learn", name, lesson, kind}`; the tool's
/// schema refused `kind` three times in one run.
#[test]
fn the_skill_tool_takes_what_the_guide_says() {
    let tools = super::mcp::tools();
    let skill = tools.iter().find(|t| t["name"] == "skill").unwrap();
    for k in ["name", "lesson", "kind", "skill", "id"] {
        assert!(skill["inputSchema"]["properties"].get(k).is_some(), "skill lacks {k}");
    }
}

/// Pan is -100..100 in `set` and `animate`; edit_schema said -1..1.
#[test]
fn field_docs_give_pan_in_the_units_set_takes() {
    let doc = super::field_docs::field("set", "pan");
    assert!(doc["range"].as_str().unwrap().contains("-100"), "{doc}");
}

fn lane_names(s: &Session) -> Vec<String> {
    s.doc.model.active().lanes.iter().map(|l| l.name.clone()).collect()
}

/// `lane add` always landed on top and `lane move` took only an index: a
/// 3D lane above a title card hid it. Lanes go next to a named lane now.
#[test]
fn lanes_go_above_or_below_a_named_lane() {
    let mut s = session();
    s.edit(&[json!({"op": "lane", "action": "add", "name": "Titles"})], "l", false).unwrap();
    s.edit(&[json!({"op": "lane", "action": "add", "name": "3D", "kind": "video", "below": "Titles"})], "l", false).unwrap();
    let names = lane_names(&s);
    let (t, d) = (names.iter().position(|n| n == "Titles").unwrap(), names.iter().position(|n| n == "3D").unwrap());
    assert_eq!(d, t + 1, "3D sits just under Titles: {names:?}");
    s.edit(&[json!({"op": "lane", "action": "move", "lane": "Titles", "below": "3D"})], "l", false).unwrap();
    let names = lane_names(&s);
    let (t, d) = (names.iter().position(|n| n == "Titles").unwrap(), names.iter().position(|n| n == "3D").unwrap());
    assert_eq!(t, d + 1, "{names:?}");
    s.edit(&[json!({"op": "lane", "action": "move", "lane": "Titles", "above": "3D"})], "l", false).unwrap();
    let names = lane_names(&s);
    assert_eq!(names.iter().position(|n| n == "Titles").unwrap() + 1, names.iter().position(|n| n == "3D").unwrap(), "{names:?}");
    let e = s.edit(&[json!({"op": "lane", "action": "move", "lane": "Titles"})], "l", false).unwrap_err();
    assert!(e.contains("above"), "{e}");
}

#[allow(dead_code)]
fn text_of(s: &mut Session, tool: &str, a: Value) -> String {
    said(s.run(tool, &a).unwrap())
}

/// A delete that ripples a lane (or an overwrite) changed clips no op
/// named, and the reply never said so.
#[test]
fn the_reply_names_clips_an_edit_changed_on_the_side() {
    let mut s = session();
    let a = card(&mut s, 0.0, 1.0);
    let b = card(&mut s, 3.0, 1.0);
    assert_eq!(s.doc.project.clip(a).unwrap().lane, s.doc.project.clip(b).unwrap().lane, "both on one lane");
    let r = s.edit(&[json!({"op": "delete", "clips": [a]})], "del", false).unwrap();
    let moved = s.doc.project.clip(b).is_some_and(|x| (super::secs(x.start) - 3.0).abs() > 1e-3);
    assert!(!moved, "off the main lane a delete lifts: {r}");
    assert!(r["read_back"].get("side_effects").is_none(), "{r}");
    // Asked to ripple, the later clip slides and the reply names it.
    let a2 = card(&mut s, 0.0, 1.0);
    let r = s.edit(&[json!({"op": "delete", "clips": [a2], "ripple": true})], "del", false).unwrap();
    let moved = s.doc.project.clip(b).is_some_and(|x| (super::secs(x.start) - 3.0).abs() > 1e-3);
    let side = r["read_back"]["side_effects"].to_string();
    assert_eq!(moved, side.contains(&format!("clip {b}")), "a clip the ripple moved is named, and only then: {r}");
    // Placed over a clip on its lane: the clip under is cut, and said so.
    let c = card(&mut s, 10.0, 4.0);
    let lane = s.doc.project.clip(c).unwrap().lane;
    let r = s.edit(&[json!({"op": "move", "clips": [b], "to": 11.0, "lane": lane})], "over", false).unwrap();
    let hit = s.doc.project.clip(c).is_none_or(|x| (super::secs(x.dur) - 4.0).abs() > 1e-3 || (super::secs(x.start) - 10.0).abs() > 1e-3);
    assert_eq!(hit, r["read_back"]["side_effects"].to_string().contains(&format!("clip {c}")), "{r}");
    // An edit that touches only what it names says nothing more.
    let r = s.edit(&[json!({"op": "set", "clip": b, "opacity": 50})], "set", false).unwrap();
    assert!(r["read_back"].get("side_effects").is_none(), "{r}");
}

/// `generation: off` is the server's rule, not a request to the model: a
/// generate call is refused whatever the prompt, reads still work, and the
/// agent cannot turn it back on.
#[test]
fn generation_off_is_enforced_by_the_server() {
    let mut s = session();
    assert!(text_of(&mut s, "policy", json!({})).contains("\"generation\":\"on\""));
    text_of(&mut s, "policy", json!({"generation": "off"}));
    let e = err(s.run("generate", &json!({"prompt": "a red fox in the snow"})));
    assert!(e.contains("generation is off"), "{e}");
    let e = err(s.run("edit", &json!({"ops": [{"op": "character", "create": true, "name": "Fox", "generate": true}]})));
    assert!(e.contains("generation is off"), "{e}");
    assert!(super::policy::generative("generate", &json!({"list": true})).is_none(), "reading what can be made is not making");
    let e = err(s.run("policy", &json!({"generation": "on"})));
    assert!(e.contains("only the person"), "{e}");
    // Everything else goes on.
    card(&mut s, 0.0, 1.0);
}

/// `contact_sheet` could not show one clip alone; `solo` draws only the
/// clips named, for the look, and changes nothing.
#[test]
fn solo_draws_one_clip_and_changes_nothing() {
    let mut s = session();
    let under = card(&mut s, 0.0, 2.0);
    s.edit(&[json!({"op": "lane", "action": "add", "name": "Cover", "position": "top"}), json!({"op": "card", "colour": "#ff0000", "at": 0.0, "duration": 2.0, "lane": "Cover"})], "over", false).unwrap();
    let rev = s.doc.history.revision();
    let all = s.run("frame", &json!({"t": 1.0, "force": true})).unwrap();
    let alone = s.run("frame", &json!({"t": 1.0, "solo": [under]})).unwrap();
    let jpeg = |p: &[Content]| p.iter().find_map(|c| if let Content::Jpeg(b) = c { Some(b.clone()) } else { None }).unwrap();
    assert_ne!(jpeg(&all), jpeg(&alone), "the covering card is not drawn");
    assert!(said(alone).contains("\"solo\""));
    assert_eq!(s.doc.history.revision(), rev, "nothing was changed");
    assert!(s.doc.model.sequences.iter().flat_map(|q| q.lanes.iter()).count() > 0);
    let sheet = s.run("contact_sheet", &json!({"count": 2, "solo": under})).unwrap();
    assert!(said(sheet).contains("\"solo\""));
    let e = err(s.run("frame", &json!({"t": 1.0, "solo": [999_999]})));
    assert!(e.contains("no clip"), "{e}");
}

/// `summary` lists at most 80 clips unless narrowed, and says how to narrow.
#[test]
fn summary_is_scoped() {
    let mut s = session();
    let ops: Vec<Value> = (0..100).map(|i| json!({"op": "card", "colour": "#336699", "at": i as f64 * 0.5, "duration": 0.4, "lane": "overlay"})).collect();
    s.edit(&ops, "cards", false).unwrap();
    let v: Value = serde_json::from_str(&text_of(&mut s, "summary", json!({}))).unwrap();
    assert_eq!(v["clips"].as_array().unwrap().len(), 80);
    assert!(v["more"].as_str().unwrap().contains("20 more"), "{}", v["more"]);
    assert!(v["lanes"].as_array().unwrap().iter().any(|l| l["clips"] == 100), "lanes carry their counts");
    let v: Value = serde_json::from_str(&text_of(&mut s, "summary", json!({"from": 10, "to": 12}))).unwrap();
    let n = v["clips"].as_array().unwrap().len();
    assert!((4..=6).contains(&n), "{n} clips between 10 and 12 s");
    assert!(v.get("more").is_none());
}

/// Marker colours had to be one of eight names; a #rrggbb takes the nearest.
#[test]
fn a_marker_takes_a_hex_colour() {
    let mut s = session();
    let r = s.edit(&[json!({"op": "marker", "at": 1.0, "name": "Hit", "colour": "#e03030"})], "m", false).unwrap();
    assert!(r["notes"].to_string().contains("red"), "{r}");
}

/// A contact sheet `every` 0.05 s over a move, as the skills teach.
#[test]
fn a_contact_sheet_takes_every() {
    let mut s = session();
    card(&mut s, 0.0, 2.0);
    let r = s.run("contact_sheet", &json!({"every": 0.25, "start": 0.5, "end": 1.5})).unwrap();
    let t = said(r);
    assert!(t.contains("[0.5,0.75,1.0,1.25,1.5]"), "{t}");
}
