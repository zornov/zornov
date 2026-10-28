// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::services;

fn session() -> Session {
    Session::new(services::project::new_custom("Physics", 1280, 720, 30.0))
}

fn last(s: &Session) -> u32 {
    s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
}

fn add(s: &mut Session, op: Value) -> u32 {
    s.edit(&[op], "add", false).unwrap();
    last(s)
}

fn at(s: &Session, id: u32, t: f64) -> m::Transform {
    s.model_clip(id).unwrap().transform_at(micros(t) - s.model_clip(id).unwrap().start)
}

/// A wide floor slab whose top is at y = 300.
fn floor(s: &mut Session) -> u32 {
    add(s, json!({"op": "model", "shape": "cube", "size": [2400, 40, 2400], "position": [0, 320, 0], "name": "Floor", "rest": false}))
}

#[test]
fn a_model_put_above_a_table_comes_down_onto_it() {
    let mut s = session();
    floor(&mut s);
    let table = add(&mut s, json!({"op": "model", "shape": "cube", "size": [400, 200, 300], "position": [0, 150, 0], "name": "Table"}));
    // The table already stands on the floor (its bottom at 250 + the slab).
    let t = at(&s, table, 0.0);
    assert!((t.position.y - 200.0).abs() < 2.0, "the table rests on the floor top (300 − 100): {}", t.position.y);
    let r = s.edit(&[json!({"op": "model", "shape": "cylinder", "size": [80, 120, 80], "position": [50, -200, 0], "name": "Mug"})], "mug", false).unwrap();
    let mug = last(&s);
    assert!(r.to_string().contains("rests on what is under it"), "{r}");
    let y = at(&s, mug, 0.0).position.y;
    // Table top at 200 − 100 = 100: the mug's middle 60 above.
    assert!((y - 40.0).abs() < 3.0, "on the table top: {y}");
    // And nothing in the scene is left floating, sunk or about to tip.
    let issues = crate::physics3d::rest::check(&s.doc.model, s.doc.seq(), 0);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn check_finds_floating_sunk_and_tipping_things() {
    let mut s = session();
    floor(&mut s);
    let float = add(&mut s, json!({"op": "model", "shape": "cube", "size": 100, "position": [-300, 150, 0], "name": "Crate", "rest": false}));
    let sunk = add(&mut s, json!({"op": "model", "shape": "cube", "size": 100, "position": [0, 290, 0], "name": "Block", "rest": false}));
    // A tall thin board standing on an edge, leaning 25°.
    let tip = add(&mut s, json!({"op": "model", "shape": "cube", "size": [20, 300, 120], "position": [300, 160, 0], "rotation": [0, 0, 25], "name": "Board", "rest": false}));
    let issues = crate::physics3d::rest::check(&s.doc.model, s.doc.seq(), 0);
    let kind = |id: u32| issues.iter().find(|f| f.clip == id as u64).map(|f| f.kind);
    assert_eq!(kind(float), Some("floating"), "{issues:?}");
    assert_eq!(kind(sunk), Some("sunk"), "{issues:?}");
    assert!(matches!(kind(tip), Some("tips") | Some("floating")), "{issues:?}");
    // settle with no clips fixes all of them.
    s.edit(&[json!({"op": "settle"})], "settle", false).unwrap();
    let after = crate::physics3d::rest::check(&s.doc.model, s.doc.seq(), 0);
    assert!(after.is_empty(), "{after:?}");
    let y = at(&s, float, 0.0).position.y;
    assert!((y - 250.0).abs() < 4.0, "the crate on the floor: {y}");
}

#[test]
fn a_stack_of_boxes_dropped_stands() {
    let mut s = session();
    floor(&mut s);
    let ids: Vec<u32> = (0..5).map(|k| add(&mut s, json!({"op": "model", "shape": "cube", "size": 80, "position": [0, 240 - 85 * k, 0], "name": format!("Box {k}"), "rest": false}))).collect();
    s.edit(&[json!({"op": "drop", "clips": ids, "duration": 3})], "drop", false).unwrap();
    for (k, &id) in ids.iter().enumerate() {
        let t = at(&s, id, 3.0);
        assert!((t.position.y - (260.0 - 80.0 * k as f64)).abs() < 4.0, "box {k} on the one under: {}", t.position.y);
        assert!(t.position.x.abs() < 3.0);
    }
}

#[test]
fn dominoes_fall_in_a_row() {
    let mut s = session();
    floor(&mut s);
    // 8 tiles, 16 × 120 × 60 px, 70 px apart.
    let ids: Vec<u32> = (0..8).map(|k| add(&mut s, json!({"op": "model", "shape": "cube", "size": [16, 120, 60], "position": [-280 + 70 * k, 240, 0], "name": format!("Domino {k}")}))).collect();
    s.edit(&[json!({"op": "push", "clip": ids[0], "direction": [1, 0, 0], "strength": 500, "point": [-280, 190, 0], "duration": 4})], "push", false).unwrap();
    let fallen = ids.iter().filter(|&&id| at(&s, id, 4.0).rotation.abs() > 45.0).count();
    assert!(fallen >= 7, "{fallen} of 8 fell: {:?}", ids.iter().map(|&id| at(&s, id, 4.0).rotation as i32).collect::<Vec<_>>());
}

#[test]
fn a_chair_set_down_tilted_settles_on_its_legs() {
    let mut s = session();
    floor(&mut s);
    s.edit(&[json!({"op": "asset", "kit": "chair", "size": 300, "position": [0, 100, 0], "rotation": [0, 0, 12], "name": "Chair"})], "chair", false).unwrap();
    let chair = last(&s);
    let r = s.edit(&[json!({"op": "settle", "clip": chair})], "settle", false).unwrap();
    assert!(r.to_string().contains("settled"), "{r}");
    let t = at(&s, chair, 0.0);
    assert!(t.rotation.abs() < 4.0 && t.rotation_x.abs() < 4.0, "upright on its legs: {} {}", t.rotation, t.rotation_x);
    let issues = crate::physics3d::rest::check(&s.doc.model, s.doc.seq(), 0);
    assert!(issues.iter().all(|f| f.clip != chair as u64), "{issues:?}");
}

#[test]
fn a_ball_thrown_through_pins_knocks_them_over() {
    let mut s = session();
    floor(&mut s);
    let pins: Vec<u32> = (0..6)
        .map(|k| {
            let (row, col) = ([0, 1, 1, 2, 2, 2][k], [0.0, -0.5, 0.5, -1.0, 0.0, 1.0][k]);
            add(&mut s, json!({"op": "model", "shape": "cylinder", "size": [30, 150, 30], "position": [300 + 60 * row, 200, (col * 60.0) as i32], "name": format!("Pin {k}")}))
        })
        .collect();
    for &p in &pins {
        s.edit(&[json!({"op": "body", "clip": p, "type": "dynamic", "material": "wood"})], "pin", false).unwrap();
    }
    let ball = add(&mut s, json!({"op": "model", "shape": "sphere", "size": 70, "position": [-400, 200, 0], "name": "Ball"}));
    s.edit(&[json!({"op": "body", "clip": ball, "type": "dynamic", "material": "rubber", "mass": 6})], "ball", false).unwrap();
    s.edit(&[json!({"op": "throw", "clip": ball, "velocity": [2600, 0, 0], "duration": 3, "clips": [ball]})], "throw", false).unwrap();
    // The pins are dynamic by their settings: bake them all together.
    let down = pins.iter().filter(|&&p| {
        let t = at(&s, p, 3.0);
        t.rotation.abs() > 40.0 || t.rotation_x.abs() > 40.0
    }).count();
    assert!(down >= 3, "{down} pins down");
    let bx = at(&s, ball, 3.0).position.x;
    assert!(bx > 300.0, "the ball rolled through: {bx}");
}

#[test]
fn a_glass_dropped_on_the_floor_shatters() {
    let mut s = session();
    floor(&mut s);
    let glass = add(&mut s, json!({"op": "model", "shape": "cylinder", "size": [70, 120, 70], "position": [0, -150, 0], "name": "Glass", "rest": false, "transmission": 0.9}));
    s.edit(&[json!({"op": "body", "clip": glass, "material": "glass"})], "glass", false).unwrap();
    let r = s.edit(&[json!({"op": "drop", "clip": glass, "duration": 2})], "drop", false).unwrap();
    assert!(r.to_string().contains("broke into") || r.to_string().contains("pieces"), "{r}");
    let c = s.model_clip(glass).unwrap();
    assert!(matches!(&c.source, m::ClipSource::Generator(g) if g.params.contains_key("cache")), "plays its pieces");
}

#[test]
fn a_character_knocked_loose_falls_as_a_ragdoll() {
    let mut s = session();
    floor(&mut s);
    let who = add(&mut s, json!({"op": "model", "shape": "character", "size": 500, "position": [0, 50, 0], "name": "Walker"}));
    let r = s.edit(&[json!({"op": "ragdoll", "clip": who, "duration": 2.5, "push": [0, 0, 600], "strength": 0.2})], "fall", false).unwrap();
    assert!(r.to_string().contains("goes loose"), "{r}");
    let c = s.model_clip(who).unwrap();
    let keyed = c.keyframes.keys().filter(|k| k.starts_with("generator.bone.")).count();
    assert!(keyed > 20, "bone keys: {keyed}");
}

#[test]
fn a_creature_tail_follows_through() {
    let mut s = session();
    let dog = add(&mut s, json!({"op": "model", "shape": "capsule", "size": [400, 150, 150], "position": [0, 0, 0], "name": "Dog", "rest": false}));
    s.edit(&[json!({"op": "rig", "clip": dog, "template": "quadruped"})], "rig", false).unwrap();
    // Runs to the right and stops.
    s.edit(&[json!({"op": "animate", "clip": dog, "property": "x", "keys": [[0, -300], [0.6, 300]]})], "move", false).unwrap();
    let r = s.edit(&[json!({"op": "jiggle", "clip": dog, "duration": 2})], "tail", false).unwrap();
    assert!(r.to_string().contains("tail"), "{r}");
    let c = s.model_clip(dog).unwrap();
    assert!(c.keyframes.keys().any(|k| k.starts_with("generator.bone.tail")), "{:?}", c.keyframes.keys().collect::<Vec<_>>());
}

#[test]
fn a_door_on_a_hinge_swings_shut_on_its_spring() {
    let mut s = session();
    let door = add(&mut s, json!({"op": "model", "shape": "cube", "size": [200, 400, 20], "position": [100, 0, 0], "name": "Door", "rest": false}));
    s.edit(&[json!({"op": "joint", "clip": door, "kind": "hinge", "at_point": [0, 0, 0], "axis": [0, 1, 0], "limits": [-100, 100], "spring": [1.5, 0.2]})], "hinge", false).unwrap();
    s.edit(&[json!({"op": "push", "clip": door, "direction": [0, 0, 1], "strength": 900, "point": [190, 0, 0], "duration": 3, "floor": false})], "push", false).unwrap();
    let widest = (1..30).map(|k| at(&s, door, k as f64 * 0.02).rotation_y.abs()).fold(0.0, f64::max);
    assert!(widest > 15.0, "swung open: {widest}");
    let end = at(&s, door, 3.0);
    assert!(end.rotation_y.abs() < widest * 0.5, "and back towards shut: {widest} then {}", end.rotation_y);
    // It turned about its hinge: the hinge edge stayed put.
    assert!((end.position.x - 100.0).abs() < 110.0);
}

#[test]
fn models_placed_around_a_million_triangles_rest_quickly_and_right() {
    let mut s = session();
    // About a million triangles packed into a block 600 × 250 × 600 px
    // (like a gyroid lattice): ten bumpy sheets 25 px apart, the top one
    // at y 42..58 once placed, the bottom one near 300.
    let mut stack = studio_mesh::Mesh::default();
    for layer in 0..10 {
        let sheet = studio_mesh::prim::plane(600.0, 600.0, 223);
        let base = stack.positions.len() as u32;
        stack.positions.extend(sheet.positions.iter().map(|p| [p[0], -125.0 + 25.0 * layer as f32 + 8.0 * (p[0] / 9.0).sin() * (p[2] / 9.0).sin(), p[2]]));
        stack.normals.extend(sheet.normals);
        stack.uvs.extend(sheet.uvs);
        stack.indices.extend(sheet.indices.iter().map(|i| i + base));
    }
    stack.single_group();
    assert!(stack.triangles() > 990_000);
    let big = crate::mesh_store::add_model_at(&mut s.doc, &stack, "Lattice", 0).unwrap();
    let mut c = s.model_clip(big).unwrap().clone();
    c.transform.position = m::Vec2::new(0.0, 175.0);
    assert!(s.doc.apply("Place", m::Edit::SetClip { seq: s.doc.seq(), clip: Box::new(c) }));
    let mut place = |s: &mut Session, op: Value, first: bool| {
        let t0 = std::time::Instant::now();
        s.edit(&[op], "add", false).unwrap();
        // The first pays for the tree over the big mesh, once.
        let most = if first { 10.0 } else { 1.0 };
        assert!(t0.elapsed().as_secs_f64() < most, "placing took {:?}", t0.elapsed());
        let id = last(s);
        at(s, id, 0.0).position.y
    };
    // Under the block, nothing under it in reach: it stays, and is not
    // lifted up into the block.
    let y = place(&mut s, json!({"op": "model", "shape": "sphere", "size": 150, "detail": 6, "position": [0, 400, 0], "name": "Under"}), true);
    assert_eq!(y, 400.0);
    // Above: down onto the top sheet.
    let y = place(&mut s, json!({"op": "model", "shape": "torus", "size": 200, "position": [100, -100, 0], "name": "Ring"}), false);
    assert!((42.0..=59.0).contains(&(y + 24.0)), "the ring's underside on the top sheet: {}", y + 24.0);
    // Put inside the block: it stays where put.
    let y = place(&mut s, json!({"op": "model", "shape": "sphere", "size": 120, "position": [-100, 150, 0], "name": "Inside"}), false);
    assert_eq!(y, 150.0);
    // Sunk a little into the top: up out of it, onto it.
    let y = place(&mut s, json!({"op": "model", "shape": "cube", "size": 100, "position": [150, 12, -200], "name": "Crate"}), false);
    assert!((42.0..=59.0).contains(&(y + 50.0)), "the crate's underside on the top sheet: {}", y + 50.0);
}
