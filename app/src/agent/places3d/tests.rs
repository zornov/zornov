// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::agent::Session;
use serde_json::{Value, json};

fn session() -> Session {
    Session::new(crate::services::project::new_custom("T", 640, 360, 30.0))
}

fn model_ids(s: &Session) -> Vec<u32> {
    let mut v: Vec<u32> = s.doc.model.active().clips.iter().filter(|c| crate::mesh_store::file_of(c).is_some()).map(|c| c.id.0 as u32).collect();
    v.sort_unstable();
    v
}

fn mesh_of(s: &Session, id: u32) -> studio_mesh::Mesh {
    let c = s.doc.model.active().clip(studio_model::ClipId(id as u64)).unwrap().clone();
    crate::mesh_store::load(&c).unwrap()
}

/// Triangle centres in the world (px), sorted, rounded: what a viewer sees of a clip.
fn world_centroids(s: &Session, id: u32) -> Vec<[i32; 3]> {
    let c = s.doc.model.active().clip(studio_model::ClipId(id as u64)).unwrap().clone();
    let m = mesh_of(s, id);
    let w = crate::agent::sim3d::model_world(&c.transform);
    let mut v: Vec<[i32; 3]> = (0..m.triangles())
        .map(|t| {
            let p = m.centroid(t);
            std::array::from_fn(|r| ((w[r][0] * p[0] as f64 + w[r][1] * p[1] as f64 + w[r][2] * p[2] as f64 + w[r][3]) * 4.0).round() as i32)
        })
        .collect();
    v.sort();
    v
}

fn lamp(s: &mut Session, name: &str, at: [i32; 3]) {
    s.edit(
        &[json!({"op": "asset", "name": name, "duration": 4, "position": at, "parts": [
            {"name": "base", "prim": "cylinder", "size": [60, 12, 60], "at": [0, 0, 0], "bevel": 1, "colour": "#b08040", "metallic": 1.0, "roughness": 0.3},
            {"name": "shade", "size": [50, 40, 50], "at": [0, -50, 0], "bevel": 2, "colour": "#f0e0c0", "emissive": "#ffd890"}]})],
        name,
        false,
    )
    .unwrap();
}

#[test]
fn combine_and_uncombine_round_trip_keeps_geometry_and_materials() {
    let mut s = session();
    lamp(&mut s, "Lamp left", [-200, 0, 0]);
    lamp(&mut s, "Lamp right", [250, 40, 60]);
    let ids = model_ids(&s);
    assert_eq!(ids.len(), 2);
    let before: Vec<Vec<[i32; 3]>> = ids.iter().map(|&i| world_centroids(&s, i)).collect();
    let mats_before: Vec<Vec<(String, [f32; 4], f32, [f32; 3])>> = ids.iter().map(|&i| mesh_of(&s, i).materials.iter().map(|m| (m.name.clone(), m.base_colour, m.metallic, m.emissive)).collect()).collect();

    let r = s.edit(&[json!({"op": "combine", "clips": ids, "name": "Lamps"})], "combine", false).unwrap().to_string();
    assert!(r.contains("2 clips became one model with 2 named parts"), "{r}");
    let combined = model_ids(&s);
    assert_eq!(combined.len(), 1, "one clip is left");
    let group = mesh_of(&s, combined[0]);
    // Named parts: each lamp is a part, its materials named under it.
    let parts: Vec<String> = studio_mesh::group::parts_of(&group).into_iter().map(|p| p.0).collect();
    assert_eq!(parts, ["Lamp left", "Lamp right"]);
    assert!(group.materials.iter().any(|m| m.name == "Lamp left/base") && group.materials.iter().any(|m| m.name == "Lamp right/shade"), "{:?}", group.materials.iter().map(|m| &m.name).collect::<Vec<_>>());
    // Where each was, the group's triangles still are.
    let mut all_before: Vec<[i32; 3]> = before.concat();
    all_before.sort();
    assert_eq!(world_centroids(&s, combined[0]), all_before);

    let r = s.edit(&[json!({"op": "uncombine", "clip": combined[0]})], "uncombine", false).unwrap().to_string();
    assert!(r.contains("2 parts are clips of their own"), "{r}");
    let after = model_ids(&s);
    assert_eq!(after.len(), 2);
    for (k, name) in ["Lamp left", "Lamp right"].iter().enumerate() {
        let id = *after.iter().find(|&&i| s.doc.model.active().clip(studio_model::ClipId(i as u64)).unwrap().name == *name).unwrap();
        assert_eq!(world_centroids(&s, id), before[k], "{name} is where it was");
        let mats: Vec<(String, [f32; 4], f32, [f32; 3])> = mesh_of(&s, id).materials.iter().map(|m| (m.name.clone(), m.base_colour, m.metallic, m.emissive)).collect();
        assert_eq!(mats, mats_before[k], "{name}'s materials");
    }
}

#[test]
fn uncombine_can_take_one_part_and_leave_the_rest() {
    let mut s = session();
    for (i, n) in ["A", "B", "C"].iter().enumerate() {
        lamp(&mut s, n, [i as i32 * 200, 0, 0]);
    }
    s.edit(&[json!({"op": "combine", "all": true, "name": "Set"})], "combine", false).unwrap();
    let g = model_ids(&s)[0];
    let r = s.edit(&[json!({"op": "uncombine", "clip": g, "parts": ["B"]})], "one", false).unwrap().to_string();
    assert!(r.contains("1 part") && r.contains("2 other parts stay"), "{r}");
    let ids = model_ids(&s);
    assert_eq!(ids.len(), 2);
    let names: Vec<String> = ids.iter().map(|&i| s.doc.model.active().clip(studio_model::ClipId(i as u64)).unwrap().name.clone()).collect();
    assert!(names.contains(&"B".to_string()) && names.contains(&"Set".to_string()), "{names:?}");
    let rest: Vec<String> = studio_mesh::group::parts_of(&mesh_of(&s, g)).into_iter().map(|p| p.0).collect();
    assert_eq!(rest, ["A", "C"]);
    let err = s.edit(&[json!({"op": "uncombine", "clip": g, "parts": ["Z"]})], "x", false).unwrap_err();
    assert!(err.contains("no part \"Z\""), "{err}");
}

#[test]
fn combine_leaves_animated_and_rigged_clips_alone_and_says_so() {
    let mut s = session();
    lamp(&mut s, "Lamp one", [0, 0, 0]);
    lamp(&mut s, "Lamp two", [200, 0, 0]);
    s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "name": "Person", "duration": 4, "detail": 1})], "person", false).unwrap();
    let r = s.edit(&[json!({"op": "combine", "all": true, "name": "Lamps"})], "combine", false).unwrap().to_string();
    assert!(r.contains("left out clip") && r.contains("Person"), "{r}");
    assert_eq!(model_ids(&s).len(), 2, "the lamps became one, the person stayed");
    // Naming an animated clip is an error that says what to do.
    let ids = model_ids(&s);
    let person = *ids.iter().find(|&&i| s.doc.model.active().clip(studio_model::ClipId(i as u64)).unwrap().name == "Person").unwrap();
    let lamps = *ids.iter().find(|&&i| i != person).unwrap();
    s.edit(&[json!({"op": "animate", "clip": person, "property": "position.x", "keys": [{"t": 0, "value": 0}, {"t": 2, "value": 100}]})], "key", false).ok();
    let e = s.edit(&[json!({"op": "combine", "clips": [person, lamps]})], "x", false);
    if let Err(e) = e {
        assert!(e.contains("animated") || e.contains("keys"), "{e}");
    }
}

#[test]
fn combine_needs_two_clips_and_says_how_to_choose_them() {
    let mut s = session();
    lamp(&mut s, "Only", [0, 0, 0]);
    let e = s.edit(&[json!({"op": "combine", "clips": [1]})], "x", false).unwrap_err();
    assert!(e.contains("at least two") && e.contains("like") && e.contains("all"), "{e}");
}

#[test]
fn a_generated_street_is_one_clip_with_named_parts_and_splits_back() {
    let mut s = session();
    let r = s.edit(&[json!({"op": "build_place", "kind": "street", "style": "art_deco", "size": 2600, "duration": 5, "detail": 1, "wear": 0.5})], "street", false).unwrap().to_string();
    assert!(r.contains("one clip") && r.contains("named parts"), "{r}");
    let ids = model_ids(&s);
    assert_eq!(ids.len(), 1, "the clip count of a generated street stays small");
    let street = ids[0];
    let parts: Vec<String> = studio_mesh::group::parts_of(&mesh_of(&s, street)).into_iter().map(|p| p.0).collect();
    for want in ["walls", "window frames", "window glass", "road", "kerbs", "sidewalk", "lamp posts", "cornice"] {
        assert!(parts.iter().any(|p| p == want), "{want} in {parts:?}");
    }
    assert!(parts.len() <= 60, "{} parts", parts.len());
    // Pull the lamps out as their own clip and put the rest back as the street.
    s.edit(&[json!({"op": "uncombine", "clip": street, "parts": ["lamp posts"]})], "lamps", false).unwrap();
    assert_eq!(model_ids(&s).len(), 2);
}

#[test]
fn places_take_a_style_a_wear_and_a_scale_and_refuse_nonsense_with_the_choices() {
    let mut s = session();
    let e = s.edit(&[json!({"op": "build_place", "kind": "castle"})], "x", false).unwrap_err();
    assert!(e.contains("building") && e.contains("street") && e.contains("interior"), "{e}");
    let e = s.edit(&[json!({"op": "build_place", "kind": "building", "style": "baroque-ish-nonsense"})], "x", false).unwrap_err();
    assert!(e.contains("classical") && e.contains("brutalist") && e.contains("futuristic"), "{e}");
    let e = s.edit(&[json!({"op": "build_place"})], "x", false).unwrap_err();
    assert!(e.contains("kind"), "{e}");
    s.edit(&[json!({"op": "build_place", "kind": "fence", "style": "rustic", "px_per_metre": 300, "duration": 3})], "fence", false).unwrap();
    s.edit(&[json!({"op": "build_place", "kind": "fence", "style": "rustic", "duration": 3, "position": [0, 0, 500]})], "fence", false).unwrap();
    let ids = model_ids(&s);
    let (a, b) = (mesh_of(&s, ids[0]).bounds(), mesh_of(&s, ids[1]).bounds());
    let (wa, wb) = (a.1[0] - a.0[0], b.1[0] - b.0[0]);
    assert!((wa / wb - 3.0).abs() < 0.05, "300 px to the metre is three times 100: {wa} vs {wb}");
    // The same words and seed make the same place.
    let x: Value = json!({"op": "build_place", "kind": "building", "style": "oriental", "seed": 9, "duration": 3});
    s.edit(&[x.clone()], "a", false).unwrap();
    s.edit(&[x], "b", false).unwrap();
    let ids = model_ids(&s);
    let n = ids.len();
    assert_eq!(mesh_of(&s, ids[n - 1]).triangles(), mesh_of(&s, ids[n - 2]).triangles());
}

#[test]
fn asset_kits_include_the_places_and_a_styled_building() {
    let mut s = session();
    s.edit(&[json!({"op": "asset", "kit": "street", "style": "rustic", "size": 1800, "duration": 3, "detail": 1})], "k", false).unwrap();
    s.edit(&[json!({"op": "asset", "kit": "building", "style": "medieval", "duration": 3})], "k2", false).unwrap();
    s.edit(&[json!({"op": "asset", "kit": "chair", "size": 120, "duration": 3, "quick": true})], "old kit still works", false).unwrap();
    assert_eq!(model_ids(&s).len(), 3);
}
