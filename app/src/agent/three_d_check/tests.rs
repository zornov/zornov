// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use studio_mesh::{Material, Mesh, ops, prim, region::Region, rig::{Bone, Rig}, sdf};
use studio_modeling::{bevel, poly::{NormalMode, Poly}, select::Edges};

fn hero(id: u32, name: &str, start: f64, end: f64, mesh: &Mesh, posed: bool, file: &str) -> Hero {
    Hero { id, name: name.into(), start, end, file: file.into(), info: Arc::new(Info::of(mesh)), posed, look_set: false, glow_set: false, reference: None, math: false }
}

/// What the AURA run built: two rounded boxes, one carved out of the other.
fn blob() -> Mesh {
    let mut body = sdf::Part::new(sdf::Shape::Box, [0.0; 3], [300.0, 120.0, 200.0]);
    body.round = 40.0;
    let mut cut = sdf::Part::new(sdf::Shape::Box, [0.0, -30.0, 0.0], [260.0, 90.0, 170.0]);
    cut.round = 30.0;
    cut.join = sdf::Join::Subtract;
    sdf::build(&[body, cut], vec![Material::default()], 4.0)
}

fn bevelled_box(size: [f32; 3], at: [f32; 3], material: u32) -> Mesh {
    let mut b = prim::cube(size);
    ops::transform(&mut b, &Region::All, at, [0.0; 3], [1.0; 3], [0.0; 3]);
    let mut poly = Poly::from_mesh(&b);
    let edges = Edges::Sharper { degrees: 50.0 }.list(&poly);
    bevel::bevel(&mut poly, &edges, 6.0, 3, 0.5);
    let mut m = poly.to_mesh(40.0, NormalMode::Area);
    ops::subdivide(&mut m, 1);
    m.materials = vec![Material::default(); material as usize + 1];
    m
}

/// A case: body, hinged lid and a light strip, each its own bevelled
/// piece with its own named material, bones at the hinge, bound whole.
fn case() -> Mesh {
    let mut body = bevelled_box([300.0, 100.0, 200.0], [0.0, 0.0, 0.0], 0);
    let mut lid = bevelled_box([300.0, 40.0, 200.0], [0.0, -75.0, 0.0], 0);
    let mut strip = bevelled_box([200.0, 8.0, 6.0], [0.0, -10.0, -101.0], 0);
    body.materials = vec![Material { name: "body".into(), base_colour: [0.02, 0.02, 0.025, 1.0], roughness: 0.6, ..Default::default() }];
    lid.materials = vec![Material { name: "lid".into(), base_colour: [0.03, 0.03, 0.03, 1.0], roughness: 0.45, ..Default::default() }];
    strip.materials = vec![Material { name: "light strip".into(), emissive: [1.0, 0.1, 0.07], ..Default::default() }];
    let mut m = body;
    let (n0, n1) = (m.materials.len() as u32, 0);
    let _ = (n0, n1);
    merge_with_materials(&mut m, &lid);
    merge_with_materials(&mut m, &strip);
    let mut rig = Rig::default();
    rig.bones.push(Bone::new("body", None, [0.0, -40.0, 0.0], [0.0, 40.0, 0.0]));
    rig.bones.push(Bone::new("lid", Some(0), [0.0, -95.0, 100.0], [0.0, -95.0, -100.0]));
    studio_mesh::autorig::bind_rigid(&m, &mut rig);
    m.rig = Some(Arc::new(rig));
    m
}

fn merge_with_materials(a: &mut Mesh, b: &Mesh) {
    let base = a.materials.len() as u32;
    let first_new_tri = a.triangles();
    ops::merge(a, b);
    // Triangles that came from `b` use its materials, after a's.
    let mut per: Vec<u32> = a.tri_materials();
    for t in first_new_tri..per.len() {
        per[t] += base;
    }
    a.materials.extend(b.materials.iter().cloned());
    a.regroup(&per);
}

fn lit() -> Env {
    Env { lights: vec![Light { start: 0.0, end: 40.0, ambient: false }, Light { start: 0.0, end: 40.0, ambient: false }, Light { start: 0.0, end: 40.0, ambient: true }], cameras: vec![Camera { start: 0.0, end: 40.0, moves: true, dof: true }], models: Vec::new(), ..Default::default() }
}

fn kinds(p: &[Problem]) -> Vec<&'static str> {
    p.iter().map(|p| p.kind).collect()
}

fn with_models(mut env: Env, heroes: &[Hero]) -> Env {
    env.models = heroes.iter().map(|h| (h.id, h.name.clone(), h.start, h.end)).collect();
    env
}

const BRIEF: &str = "A real 3D model of the case (rounded, matte black, soft coral light strip). It opens on a beat with a hinged lid.";

fn words() -> Words {
    Words { brief: BRIEF.into(), notes: String::new() }
}

#[test]
fn a_two_box_blob_is_flagged_with_concrete_notes() {
    let h = hero(1, "Case", 6.0, 16.0, &blob(), false, "a");
    let env = with_models(lit(), std::slice::from_ref(&h));
    let p = judge(&[h], &env, &words());
    let k = kinds(&p);
    for want in ["3d_primitive", "3d_default_material", "3d_missing_part"] {
        assert!(k.contains(&want), "{want} in {k:?}");
    }
    let missing = p.iter().find(|p| p.kind == "3d_missing_part").unwrap();
    assert!(missing.problem.contains("strip") && missing.problem.contains("lid"), "{}", missing.problem);
    assert!(p.iter().any(|p| p.kind == "3d_missing_emissive"));
    assert!(p.iter().find(|p| p.kind == "3d_primitive").unwrap().problem.contains("1 separate piece"));
}

#[test]
fn razor_edges_are_named_on_a_plain_box() {
    let h = hero(1, "Box", 0.0, 6.0, &prim::cube([200.0; 3]), false, "a");
    let p = judge(&[h], &lit(), &Words::default());
    let n = p.iter().find(|p| p.kind == "3d_no_bevel").expect("no bevel note");
    assert!(n.problem.contains("no bevels on hero object"), "{}", n.problem);
}

#[test]
fn a_bevelled_multi_part_model_with_a_hinge_pivot_passes() {
    let h = hero(1, "Case", 6.0, 16.0, &case(), true, "a");
    let env = with_models(lit(), std::slice::from_ref(&h));
    let p = judge(&[h], &env, &words());
    assert!(p.is_empty(), "{:?}", p.iter().map(|p| (p.kind, p.problem.clone())).collect::<Vec<_>>());
}

#[test]
fn a_hinge_that_is_never_posed_and_default_light_are_named() {
    let h = hero(1, "Case", 6.0, 16.0, &case(), false, "a");
    let env = with_models(Env { cameras: vec![Camera { start: 0.0, end: 40.0, moves: false, dof: false }], ..Default::default() }, std::slice::from_ref(&h));
    let k = kinds(&judge(&[h], &env, &words()));
    for want in ["3d_part_static", "3d_default_light", "3d_no_camera_move", "3d_no_dof"] {
        assert!(k.contains(&want), "{want} in {k:?}");
    }
}

#[test]
fn a_second_model_standing_in_for_a_state_change_is_flagged() {
    let a = hero(1, "AURA case open", 6.0, 16.0, &case(), true, "file-a");
    let b = hero(2, "AURA case closing", 36.0, 40.0, &case(), true, "file-b");
    let env = with_models(lit(), &[a, b]);
    let (a, b) = (hero(1, "AURA case open", 6.0, 16.0, &case(), true, "file-a"), hero(2, "AURA case closing", 36.0, 40.0, &case(), true, "file-b"));
    let p = judge(&[a, b], &env, &Words::default());
    let n = p.iter().find(|p| p.kind == "3d_state_swap").expect("swap noted");
    assert!(n.fix.contains("pose") && n.problem.contains("same object"), "{}", n.problem);
    // The same file cut in two is not a swap.
    let (a, b) = (hero(1, "AURA case open", 6.0, 16.0, &case(), true, "same"), hero(2, "AURA case closing", 36.0, 40.0, &case(), true, "same"));
    assert!(!kinds(&judge(&[a, b], &env, &Words::default())).contains(&"3d_state_swap"));
}

#[test]
fn an_object_the_brief_calls_real_3d_must_exist_and_the_ai_can_set_style_aside() {
    let h = hero(1, "AURA case", 6.0, 16.0, &case(), true, "a");
    let env = with_models(lit(), std::slice::from_ref(&h));
    let w = Words { brief: "The earbud is real 3D. A 3D model of the earbud case.".into(), notes: String::new() };
    let p = judge(std::slice::from_ref(&h), &env, &w);
    assert!(p.iter().any(|p| p.kind == "3d_missing_part" && p.problem.contains("earbud")), "{:?}", kinds(&p));
    // A low-poly look the brief asks for may be set aside; a missing part may not.
    let h = hero(1, "Rock", 0.0, 6.0, &prim::cube([200.0; 3]), false, "b");
    let w = Words { brief: String::new(), notes: "3d-ok: no_bevel, primitive - the brief asks for a blocky low-poly look".into() };
    let k = kinds(&judge(&[h], &lit(), &w));
    assert!(!k.contains(&"3d_no_bevel") && !k.contains(&"3d_primitive"), "{k:?}");
    assert_eq!(accepted("3d-ok: missing_part, state_swap - because"), Vec::<String>::new());
}

#[test]
fn a_brief_for_3d_with_no_model_is_named() {
    let w = Words { brief: "Show a real 3D model of the product.".into(), notes: String::new() };
    assert!(kinds(&judge(&[], &Env::default(), &w)).contains(&"3d_no_model"));
}

fn session_with_blob() -> crate::agent::Session {
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("T", 640, 360, 30.0));
    s.edit(&[json!({"op": "model", "name": "Case", "duration": 6, "parts": [
        {"shape": "box", "size": [300, 120, 200], "round": 40},
        {"shape": "box", "at": [0, -30, 0], "size": [260, 90, 170], "round": 30, "subtract": true}], "colour": "#111111"})], "m", false).unwrap();
    s
}

use serde_json::json;

#[test]
fn open_3d_notes_block_done_in_review_and_check() {
    let mut s = session_with_blob();
    let r = s.run("review", &json!({"brief": BRIEF})).unwrap();
    let v: serde_json::Value = serde_json::from_str(&first_text_of(&r)).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["verdict"].as_str().unwrap().starts_with("NOT DONE"), "{v}");
    assert!(v["three_d"]["open"].as_u64().unwrap() >= 3, "{v}");
    assert!(v["notes"].as_array().unwrap().iter().any(|n| n["kind"] == "3d_primitive"));
    let only = s.run("review", &json!({"only": "3d"})).unwrap();
    assert!(first_text_of(&only).contains("NOT DONE"));
    let c = s.run("check", &json!({"frames": false})).unwrap();
    assert!(first_text_of(&c).contains("3d_primitive"));
}

#[test]
fn bevel_step_and_rigid_rig_make_a_designed_model() {
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("T", 640, 360, 30.0));
    s.edit(&[json!({"op": "model", "shape": "cube", "size": 200, "name": "Body"})], "m", false).unwrap();
    let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    let (_, before) = s.model_of(id).unwrap();
    assert!(hero::measure(&before).sharp_share > 0.5);
    s.edit(&[json!({"op": "mesh", "clip": id, "steps": [
        {"do": "add", "shape": "cube", "size": 60, "at": [0, -150, 0], "name": "lid", "colour": "#222222", "symmetry": false},
        {"do": "bevel", "width": 6, "segments": 3}]})], "b", false).unwrap();
    let (_, after) = s.model_of(id).unwrap();
    assert!(hero::measure(&after).sharp_share < 0.2, "{:?}", hero::measure(&after));
    s.edit(&[json!({"op": "rig", "clip": id, "rigid": true, "bones": [
        {"name": "body", "head": [0, -50, 0], "tail": [0, 50, 0]},
        {"name": "lid", "parent": "body", "head": [0, -150, 30], "tail": [0, -150, -30]}]})], "r", false).unwrap();
    let (_, rigged) = s.model_of(id).unwrap();
    let r = rigged.rig.as_ref().unwrap();
    assert!(r.weights.iter().all(|w| w[0] == 1.0));
    assert!(hero::measure(&rigged).bones == 2);
}

fn first_text_of(parts: &[crate::agent::Content]) -> String {
    parts.iter().find_map(|p| if let crate::agent::Content::Text(t) = p { Some(t.clone()) } else { None }).unwrap_or_default()
}


#[test]
fn an_asset_of_named_parts_with_a_hinge_bone_passes_the_critic() {
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("T", 640, 360, 30.0));
    let r = s.edit(&[json!({"op": "asset", "name": "AURA case", "duration": 10, "parts": [
        {"name": "body", "prim": "box", "size": [300, 100, 200], "bevel": 6, "subdivide": 1, "material": {"colour": "#101012", "roughness": 0.6}},
        {"name": "lid", "prim": "box", "size": [300, 40, 200], "at": [0, -75, 0], "bevel": 6, "subdivide": 1, "material": "plastic_matte", "bone": {"head": [0, -95, 100], "tail": [0, -95, -100]}},
        {"name": "light strip", "prim": "box", "size": [200, 8, 6], "at": [0, -10, -101], "bevel": 1, "material": {"colour": "#ff5a4e", "emissive": "#ff5a4e"}}]})], "asset", false).unwrap();
    assert!(r.to_string().contains("3 parts"), "{r}");
    let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    let (_, mesh) = s.model_of(id).unwrap();
    let c = hero::measure(&mesh);
    assert_eq!((c.shells, c.bones), (3, 2), "{c:?}");
    assert!(c.emissive_materials >= 1 && mesh.materials.iter().any(|m| m.name == "light strip" || m.name == "lid"), "{:?}", mesh.materials.iter().map(|m| &m.name).collect::<Vec<_>>());
    // Opening it is a pose on the same model.
    s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 1, "bones": {"lid": [0, 0, 0]}}, {"t": 2, "bones": {"lid": [-100, 0, 0]}}]})], "pose", false).unwrap();
    let h = hero_from_session(&s);
    let env = with_models(lit(), std::slice::from_ref(&h));
    let p = judge(&[h], &env, &words());
    assert!(p.is_empty(), "{:?}", p.iter().map(|p| (p.kind, p.problem.clone())).collect::<Vec<_>>());
}

fn hero_from_session(s: &crate::agent::Session) -> Hero {
    let (heroes, _) = scene(&s.doc.model, &s.doc.project, &[]);
    heroes.into_iter().next().expect("a hero model")
}

#[test]
fn kits_from_many_subjects_build() {
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("T", 640, 360, 30.0));
    for kit in ["chair", "barrel", "building"] {
        s.edit(&[json!({"op": "asset", "kit": kit, "name": kit, "size": 300, "duration": 3, "quick": true})], kit, false).unwrap_or_else(|e| panic!("{kit}: {e}"));
    }
    assert_eq!(s.doc.project.clips.len(), 3);
    assert!(s.edit(&[json!({"op": "asset", "kit": "spaceship"})], "x", false).unwrap_err().contains("kits:"));
}

fn judged(s: &crate::agent::Session, brief: &str) -> Vec<(&'static str, String)> {
    let h = hero_from_session(s);
    let env = with_models(lit(), std::slice::from_ref(&h));
    judge(&[h], &env, &Words { brief: brief.into(), notes: String::new() }).into_iter().map(|p| (p.kind, p.problem)).collect()
}

fn session() -> crate::agent::Session {
    crate::agent::Session::new(crate::services::project::new_custom("T", 640, 360, 30.0))
}

const CAR: &str = "A real 3D model of the sports car with four wheels, a windscreen and glowing headlights. The doors open and the wheels spin.";

#[test]
fn a_vehicle_built_as_a_blob_is_missing_its_wheels_glass_and_lights() {
    let mut s = session();
    s.edit(&[json!({"op": "asset", "name": "Sports car", "duration": 8, "parts": [{"name": "body", "size": [400, 120, 180], "bevel": 8, "subdivide": 1}]})], "car", false).unwrap();
    let p = judged(&s, CAR);
    let missing = p.iter().find(|p| p.0 == "3d_missing_part").expect("missing parts").1.clone();
    for w in ["wheel", "windscreen", "headlight", "door"] {
        assert!(missing.contains(w), "{w} in {missing}");
    }
    assert!(p.iter().any(|p| p.0 == "3d_missing_emissive"), "{p:?}");
}

#[test]
fn a_vehicle_of_named_parts_with_posed_doors_passes() {
    let mut s = session();
    let wheel = |n: &str, x: i32, z: i32| json!({"name": n, "prim": "cylinder", "size": [60, 30, 60], "at": [x, 55, z], "turn": [90, 0, 0], "bevel": 2, "material": "rubber"});
    s.edit(&[json!({"op": "asset", "name": "Sports car", "duration": 8, "parts": [
        {"name": "body", "size": [400, 100, 180], "bevel": 8, "subdivide": 1, "material": "car_paint"},
        wheel("wheel front left", 130, -90), wheel("wheel front right", 130, 90), wheel("wheel rear left", -130, -90), wheel("wheel rear right", -130, 90),
        {"name": "windscreen", "size": [90, 50, 150], "at": [60, -70, 0], "bevel": 2, "material": {"colour": "#a8c8d8", "transmission": 1}},
        {"name": "headlight left", "size": [14, 14, 30], "at": [200, -5, -55], "bevel": 2, "material": {"colour": "#ffffff", "emissive": "#fff4d0"}},
        {"name": "headlight right", "size": [14, 14, 30], "at": [200, -5, 55], "bevel": 2, "material": {"colour": "#ffffff", "emissive": "#fff4d0"}},
        {"name": "door left", "size": [100, 70, 6], "at": [0, -5, -92], "bevel": 2, "material": "car_paint", "bone": {"head": [50, 0, -92], "tail": [50, -60, -92]}}]})], "car", false).unwrap();
    let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 1, "bones": {"door left": [0, 0, 0]}}, {"t": 2, "bones": {"door left": [0, 0, 70]}}]})], "pose", false).unwrap();
    let p = judged(&s, CAR);
    assert!(p.is_empty(), "{p:?}");
}

const WARDROBE: &str = "The hero is a 3D wardrobe (oak, two doors, brass handles, standing on four legs). The doors swing open to show a rail.";

#[test]
fn furniture_without_doors_handles_and_legs_is_flagged_and_with_them_passes() {
    let mut s = session();
    s.edit(&[json!({"op": "asset", "name": "Oak wardrobe", "duration": 8, "parts": [{"name": "carcass", "size": [200, 300, 90], "bevel": 3, "subdivide": 1, "material": "wood"}]})], "w", false).unwrap();
    let p = judged(&s, WARDROBE);
    let missing = p.iter().find(|p| p.0 == "3d_missing_part").expect("missing").1.clone();
    for w in ["door", "handle", "leg", "rail"] {
        assert!(missing.contains(w), "{w} in {missing}");
    }
    let mut s = session();
    let leg = |n: &str, x: i32, z: i32| json!({"name": n, "size": [12, 40, 12], "at": [x, 170, z], "bevel": 1, "material": "wood"});
    s.edit(&[json!({"op": "asset", "name": "Oak wardrobe", "duration": 8, "parts": [
        {"name": "carcass", "size": [200, 300, 90], "bevel": 3, "subdivide": 1, "material": "wood"},
        leg("leg a", -85, -35), leg("leg b", 85, -35), leg("leg c", -85, 35), leg("leg d", 85, 35),
        {"name": "door left", "size": [98, 280, 4], "at": [-50, 0, -47], "bevel": 1, "material": "wood_varnished", "bone": {"head": [-100, 0, -47], "tail": [-100, -100, -47]}},
        {"name": "door right", "size": [98, 280, 4], "at": [50, 0, -47], "bevel": 1, "material": "wood_varnished", "bone": {"head": [100, 0, -47], "tail": [100, -100, -47]}},
        {"name": "handle left", "size": [4, 30, 4], "at": [-10, 0, -52], "bevel": 1, "material": "gold"},
        {"name": "handle right", "size": [4, 30, 4], "at": [10, 0, -52], "bevel": 1, "material": "gold"},
        {"name": "rail", "prim": "cylinder", "size": [6, 180, 6], "at": [0, -110, 0], "turn": [0, 0, 90], "material": "steel"}]})], "w", false).unwrap();
    let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 1, "bones": {"door left": [0, 0, 0], "door right": [0, 0, 0]}}, {"t": 2, "bones": {"door left": [0, -100, 0], "door right": [0, 100, 0]}}]})], "pose", false).unwrap();
    let p = judged(&s, WARDROBE);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_rigged_character_has_the_parts_a_brief_lists() {
    let mut s = session();
    s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "name": "Hero character", "duration": 6, "detail": 1})], "c", false).unwrap();
    let p = judged(&s, "The hero is a 3D character with a head, two arms and two legs. Parts: body, hands, feet.");
    assert!(!p.iter().any(|p| p.0 == "3d_missing_part"), "{p:?}");
    // The same brief for a car-like blob does not borrow a person's parts: no arm needed on a car.
    let mut s = session();
    s.edit(&[json!({"op": "asset", "name": "Delivery van", "duration": 6, "parts": [{"name": "body", "size": [300, 140, 140], "bevel": 5}]})], "v", false).unwrap();
    let p = judged(&s, "The van has an arm of the sea behind it. Back to black.");
    assert!(!p.iter().any(|p| p.0 == "3d_missing_part"), "{p:?}");
}

/// A textured surface of the kind an image-to-3D model makes: here with a
/// torn-open side, a loose speck and a texture smeared over one part.
fn reconstruction(holes: bool, speck: bool, smear: bool) -> Mesh {
    let mut m = prim::by_name("sphere", 100.0, 3).unwrap();
    m.materials = vec![Material { name: "Surface".into(), texture: Some(studio_mesh::Texture { width: 8, height: 8, rgba: vec![200; 8 * 8 * 4] }), ..Default::default() }];
    if holes {
        // Take out every seventh face: many open edges.
        let keep: Vec<u32> = m.indices.chunks_exact(3).enumerate().filter(|(i, _)| i % 7 != 0).flat_map(|(_, t)| t.to_vec()).collect();
        m.indices = keep;
    }
    if smear {
        for (i, uv) in m.uvs.iter_mut().enumerate() {
            if i % 3 == 0 {
                *uv = [0.5 + uv[0] * 0.001, 0.5 + uv[1] * 0.001];
            }
        }
    }
    if speck {
        let mut bit = prim::cube([2.0; 3]);
        ops::transform(&mut bit, &Region::All, [300.0, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        // `merge` appends the speck's material itself: a reconstruction has one or two.
        ops::merge(&mut m, &bit);
    }
    m.tidy();
    m
}

#[test]
fn a_reconstruction_with_holes_floating_pieces_and_smeared_texture_is_named_with_fixes() {
    let bad = hero(1, "Rooster", 0.0, 6.0, &reconstruction(true, true, true), false, "a");
    let p = judge(&[bad], &lit(), &Words::default());
    for want in ["3d_holes", "3d_floating_pieces", "3d_smeared_texture"] {
        let n = p.iter().find(|p| p.kind == want).unwrap_or_else(|| panic!("{want} in {:?}", kinds(&p)));
        assert!(n.fix.contains("model3d") && n.problem.contains("Rooster"), "{}", n.problem);
    }
    // A clean one is left alone, and a surface with no texture is not judged this way.
    let good = hero(2, "Ball", 0.0, 6.0, &reconstruction(false, false, false), false, "b");
    let k = kinds(&judge(&[good], &lit(), &Words::default()));
    assert!(!k.iter().any(|k| ["3d_holes", "3d_floating_pieces", "3d_smeared_texture"].contains(k)), "{k:?}");
}

/// A four-legged clay body along z (torso, legs, head), rigged and bound.
fn clay_animal() -> Mesh {
    let cap = |a: [f32; 3], b: [f32; 3], r: f32| sdf::Part { a, b, blend: 12.0, ..sdf::Part::new(sdf::Shape::Capsule, [0.0; 3], [r * 2.0; 3]) };
    let mut parts = vec![sdf::Part::new(sdf::Shape::Ellipsoid, [0.0; 3], [110.0, 90.0, 260.0])];
    for (x, z) in [(-32.0f32, -80.0f32), (32.0, -80.0), (-32.0, 80.0), (32.0, 80.0)] {
        parts.push(cap([x, 10.0, z], [x, 120.0, z], 16.0));
    }
    parts.push(sdf::Part { blend: 10.0, ..sdf::Part::new(sdf::Shape::Ellipsoid, [0.0, -60.0, -160.0], [60.0, 55.0, 75.0]) });
    let mut m = sdf::build(&parts, vec![Material::default()], 5.0);
    studio_mesh::autorig::rig_mesh(&mut m, "quadruped").unwrap();
    m
}

#[test]
fn skin_weights_pulled_across_the_body_are_a_note() {
    let good = clay_animal();
    let audit = Info::of(&good).skin.expect("a skinned model is audited");
    println!("bound: {audit:?}");
    let p = judge(&[hero(7, "Bear", 0.0, 4.0, &good, true, "bear_good.smodel")], &lit(), &Words::default());
    assert!(!p.iter().any(|x| x.kind == "3d_skin_weights"), "a fresh bind is clean: {audit:?}");
    // Every vertex half on the left front leg: the right side is pulled
    // across, the back of the body by a far bone.
    let mut bad = good.clone();
    let mut r = (**bad.rig.as_ref().unwrap()).clone();
    let leg = r.bone("leg_front.L.0").unwrap() as u16;
    for (j, w) in r.joints.iter_mut().zip(r.weights.iter_mut()) {
        j[1] = leg;
        *w = [0.5, 0.5, 0.0, 0.0];
    }
    bad.rig = Some(Arc::new(r));
    let p = judge(&[hero(7, "Bear", 0.0, 4.0, &bad, true, "bear_bad.smodel")], &lit(), &Words::default());
    let note = p.iter().find(|x| x.kind == "3d_skin_weights").expect("pulled weights are found");
    assert!(note.problem.contains("other side") && note.problem.contains("leg_front.L.0") && note.fix.contains("skinning"), "{}", note.problem);
}
