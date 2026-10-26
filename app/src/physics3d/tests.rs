// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn physical_settings_round_trip_on_a_clip() {
    let mut c = crate::mesh_store::new_clip(1, 1, 0, 1_000_000, "x.smesh".into(), "Crate");
    assert_eq!(Props::of(&c), Props::default());
    let p = Props {
        body: BodyType::Dynamic,
        material: Some("oak".into()),
        mass: Some(4.5),
        fit: Fit::Hull,
        friction: Some(0.8),
        bounce: None,
        breakable: Some(false),
        joints: vec![JointSpec { kind: "hinge".into(), to: Some(7), at: Some([1.0, 2.0, 3.0]), axis: Some([0.0, 1.0, 0.0]), limits: Some([0.0, 90.0]), motor: None, spring: None, breaks: None }],
    };
    p.write(&mut c);
    assert_eq!(Props::of(&c), p);
    // The defaults leave nothing behind.
    Props::default().write(&mut c);
    assert!(matches!(&c.source, m::ClipSource::Generator(g) if !g.params.keys().any(|k| k.starts_with("physics."))));
    assert_eq!(p.stuff(None, "Crate").name, "hardwood");
}

#[test]
fn the_scene_scale_comes_from_its_settings_or_studio_s_usual_size() {
    let mut p = m::Project::new(m::ProjectSettings::default());
    assert_eq!(px_per_metre(&p), DEFAULT_PX_PER_METRE);
    let w = World { px_per_metre: Some(100.0), ..Default::default() };
    w.write(&mut p.settings);
    assert_eq!(px_per_metre(&p), 100.0);
    assert_eq!(World::of(&p), w);
}

#[test]
fn a_clip_pose_round_trips_through_the_transform() {
    let mut tr = m::Transform::default();
    (tr.rotation_x, tr.rotation_y, tr.rotation) = (20.0, -35.0, 70.0);
    tr.position = m::Vec2::new(100.0, -50.0);
    tr.position_z = 30.0;
    let ppm = 200.0;
    let pose = scene::frame_of(&tr, ppm);
    let (p, r) = pose_to_transform(&pose, ppm);
    assert!((p[0] - 100.0).abs() < 1e-3 && (p[1] + 50.0).abs() < 1e-3 && (p[2] - 30.0).abs() < 1e-3);
    let back = scene::frame_of(&m::Transform { rotation_x: r[0], rotation_y: r[1], rotation: r[2], ..tr.clone() }, ppm);
    assert!(studio_sim::dynamics::math::length(studio_sim::dynamics::math::q_log(studio_sim::dynamics::math::q_mul(back.q, studio_sim::dynamics::math::q_conj(pose.q)))) < 1e-3);
}
