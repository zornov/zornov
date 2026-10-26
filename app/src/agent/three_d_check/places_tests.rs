// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The critic on places and figures: a street made of forty cubes is
//! flagged once, with the way out; a generated one passes; a figure made
//! of cones and cubes is sent to the character base; a walk keyed pose by
//! pose is sent to `act`.

use super::*;
use serde_json::{Value, json};

fn session() -> crate::agent::Session {
    crate::agent::Session::new(crate::services::project::new_custom("T", 640, 360, 30.0))
}

fn lit_env(mut env: Env) -> Env {
    env.lights = vec![Light { start: 0.0, end: 40.0, ambient: false }, Light { start: 0.0, end: 40.0, ambient: false }, Light { start: 0.0, end: 40.0, ambient: true }];
    env.cameras = vec![Camera { start: 0.0, end: 40.0, moves: true, dof: true }];
    env
}

/// Every note about the models of the film, lit and with a moving camera.
fn notes(s: &crate::agent::Session) -> Vec<Problem> {
    let (heroes, env) = scene(&s.doc.model, &s.doc.project, &[]);
    judge(&heroes, &lit_env(env), &Words::default())
}

fn kinds(p: &[Problem]) -> Vec<&'static str> {
    p.iter().map(|p| p.kind).collect()
}

/// The street the AI built in the run that started this: a cube per
/// pilaster, a cube per cornice, a cube per window frame.
fn forty_cube_street() -> crate::agent::Session {
    let mut s = session();
    let mut ops = Vec::new();
    for i in 0..40 {
        let (x, y) = (-900.0 + (i % 10) as f64 * 190.0, -300.0 + (i / 10) as f64 * 160.0);
        let what = ["Pilaster", "Cornice", "Window frame", "Door frame"][i % 4];
        ops.push(json!({"op": "model", "shape": "cube", "size": [30, 140, 30], "name": format!("{what} {i}"), "duration": 6, "position": [x, y, 0], "colour": "#c8b89a"}));
    }
    s.edit(&ops, "street", false).unwrap();
    s
}

#[test]
fn a_forty_cube_street_is_flagged_once_with_the_way_out() {
    let s = forty_cube_street();
    assert_eq!(s.doc.project.clips.len(), 40);
    let p = notes(&s);
    let many: Vec<&Problem> = p.iter().filter(|p| p.kind == "3d_many_primitives").collect();
    assert_eq!(many.len(), 1, "{:?}", kinds(&p));
    assert!(many[0].problem.contains("40 separate primitive model clips"), "{}", many[0].problem);
    for word in ["build_place", "combine", "build_world", "named parts"] {
        assert!(many[0].fix.contains(word), "{word} in {}", many[0].fix);
    }
    // The forty are one note, not forty notes about each cube.
    assert!(!kinds(&p).contains(&"3d_primitive") && !kinds(&p).contains(&"3d_no_bevel"), "{:?}", kinds(&p));
}

#[test]
fn eight_is_where_a_place_begins_and_seven_props_are_not_one() {
    let mut s = session();
    let ops: Vec<Value> = (0..7).map(|i| json!({"op": "model", "shape": "cube", "size": 60, "name": format!("Crate {i}"), "duration": 6, "position": [i * 90, 0, 0]})).collect();
    s.edit(&ops, "seven", false).unwrap();
    assert!(!kinds(&notes(&s)).contains(&"3d_many_primitives"));
    s.edit(&[json!({"op": "model", "shape": "cube", "size": 60, "name": "Crate 8", "duration": 6, "position": [900, 0, 0]})], "eight", false).unwrap();
    assert!(kinds(&notes(&s)).contains(&"3d_many_primitives"));
}

#[test]
fn a_set_aside_note_with_a_reason_is_honoured() {
    let s = forty_cube_street();
    let (heroes, env) = scene(&s.doc.model, &s.doc.project, &[]);
    let words = Words { brief: String::new(), notes: "3d-ok: many_primitives - the brief is a voxel-art look made of blocks".into() };
    let p = judge(&heroes, &lit_env(env), &words);
    assert!(!kinds(&p).contains(&"3d_many_primitives"), "{:?}", kinds(&p));
}

#[test]
fn a_generated_street_is_one_clip_and_passes() {
    let mut s = session();
    s.edit(&[json!({"op": "build_place", "kind": "street", "style": "classical", "size": 2600, "duration": 6, "detail": 1})], "street", false).unwrap();
    assert_eq!(s.doc.project.clips.len(), 1, "the whole street is one model clip");
    let p = notes(&s);
    assert!(p.is_empty(), "{:?}", p.iter().map(|p| (p.kind, p.problem.clone())).collect::<Vec<_>>());
}

#[test]
fn places_in_every_style_pass_the_critic() {
    for style in ["classical", "art_deco", "brutalist", "modern_glass", "medieval", "oriental", "futuristic", "rustic"] {
        let mut s = session();
        s.edit(&[json!({"op": "build_place", "kind": "building", "style": style, "duration": 6, "wear": 0.4, "lit": 0.3})], "b", false).unwrap();
        let p = notes(&s);
        assert!(p.is_empty(), "{style} building: {:?}", p.iter().map(|p| (p.kind, p.problem.clone())).collect::<Vec<_>>());
    }
    for (kind, style) in [("interior", "classical"), ("interior", "oriental"), ("bridge", "art_deco"), ("fence", "medieval"), ("stairs", "modern_glass"), ("facade", "brutalist")] {
        let mut s = session();
        s.edit(&[json!({"op": "build_place", "kind": kind, "style": style, "duration": 6})], "p", false).unwrap();
        let p = notes(&s);
        assert!(p.is_empty(), "{style} {kind}: {:?}", p.iter().map(|p| (p.kind, p.problem.clone())).collect::<Vec<_>>());
    }
}

#[test]
fn combining_the_forty_clears_the_note_and_leaves_one_clip() {
    let mut s = forty_cube_street();
    let r = s.edit(&[json!({"op": "combine", "all": true, "name": "Street set"})], "combine", false).unwrap();
    assert!(r.to_string().contains("40 clips became one model"), "{r}");
    assert_eq!(s.doc.project.clips.len(), 1);
    assert!(!kinds(&notes(&s)).contains(&"3d_many_primitives"));
}

fn figure_from_cones_and_cubes() -> crate::agent::Session {
    let mut s = session();
    let parts = [("Head", "sphere", [40, 40, 40], [0, -230, 0]), ("Torso", "cube", [60, 90, 30], [0, -150, 0]), ("Left arm", "cone", [20, 80, 20], [-50, -150, 0]), ("Right arm", "cone", [20, 80, 20], [50, -150, 0]), ("Left leg", "cube", [24, 90, 24], [-20, -50, 0]), ("Right leg", "cube", [24, 90, 24], [20, -50, 0])];
    let ops: Vec<Value> = parts.iter().map(|(n, shape, size, at)| json!({"op": "model", "shape": shape, "size": size, "name": n, "duration": 6, "position": at})).collect();
    s.edit(&ops, "figure", false).unwrap();
    s
}

#[test]
fn a_figure_of_cones_and_cubes_is_sent_to_the_character_base() {
    let s = figure_from_cones_and_cubes();
    let p = notes(&s);
    let n: Vec<&Problem> = p.iter().filter(|p| p.kind == "3d_character_from_primitives").collect();
    assert_eq!(n.len(), 1, "{:?}", kinds(&p));
    assert!(n[0].problem.contains("6 separate primitive clips"), "{}", n[0].problem);
    for word in ["shape", "character", "wardrobe", "hold", "act"] {
        assert!(n[0].fix.contains(word), "{word} in {}", n[0].fix);
    }
    // Not also six notes about six primitives.
    assert!(!kinds(&p).contains(&"3d_primitive"), "{:?}", kinds(&p));
}

#[test]
fn the_character_base_is_not_a_figure_of_primitives() {
    let mut s = session();
    s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "name": "Hero", "duration": 6, "detail": 1})], "c", false).unwrap();
    assert!(!kinds(&notes(&s)).contains(&"3d_character_from_primitives"));
}

fn keyed(keys: usize, bones: usize, poses: usize, acts: bool) -> Problem_ {
    let env = Env { keyed: vec![KeyedClip { id: 5, name: "Walker".into(), start: 0.0, end: 8.0, keys, bones, poses, acts }], ..Default::default() };
    judge(&[], &lit_env(env), &Words::default())
}

type Problem_ = Vec<Problem>;

#[test]
fn a_walk_keyed_pose_by_pose_is_sent_to_act_and_a_lid_is_not() {
    let p = keyed(72, 6, 12, false);
    let n = p.iter().find(|p| p.kind == "3d_hand_keyed_motion").expect("flagged");
    assert!(n.problem.contains("72 hand-written bone keys on 6 bones across 12 poses"), "{}", n.problem);
    assert!(n.fix.contains("`act`") && n.fix.contains("locomotion"), "{}", n.fix);
    assert!(keyed(6, 1, 3, false).is_empty(), "a hinge opening is hand work");
    assert!(keyed(72, 6, 12, true).is_empty(), "with an action running it is not hand-keyed");
}

#[test]
fn bone_keys_on_a_session_clip_are_counted_from_the_timeline() {
    let mut s = session();
    s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "name": "Walker", "duration": 8, "detail": 1})], "c", false).unwrap();
    let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    let mut keys = Vec::new();
    for k in 0..10 {
        let sw = if k % 2 == 0 { 30.0 } else { -30.0 };
        keys.push(json!({"t": 0.5 * k as f64, "bones": {"thigh.L": [sw, 0, 0], "thigh.R": [-sw, 0, 0], "upper_arm.L": [-sw, 0, 0], "upper_arm.R": [sw, 0, 0]}}));
    }
    s.edit(&[json!({"op": "pose", "clip": id, "keys": keys})], "walk", false).unwrap();
    let p = notes(&s);
    assert!(kinds(&p).contains(&"3d_hand_keyed_motion"), "{:?}", kinds(&p));
}
