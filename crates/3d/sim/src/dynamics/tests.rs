// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::joint::{JointKind, Motor};
use super::material::named;
use super::*;
use std::sync::Arc;

/// y is down (as in Studio), the ground solid below y = 0.
fn world() -> World {
    let mut w = World::new([0.0, 9.81, 0.0]);
    let stone = named("concrete").unwrap();
    w.add(Body::new(Kind::Static, vec![Collider::new(Shape::Plane { n: [0.0, -1.0, 0.0], d: 0.0 }, Pose::IDENTITY, &stone)], Pose::IDENTITY));
    w
}

fn boxy(w: &mut World, half: V3, at: V3, stuff: &str) -> usize {
    w.add(Body::new(Kind::Dynamic, vec![Collider::new(Shape::cuboid(half), Pose::IDENTITY, &named(stuff).unwrap())], Pose::at(at)))
}

fn ball(w: &mut World, r: f32, at: V3, stuff: &str) -> usize {
    w.add(Body::new(Kind::Dynamic, vec![Collider::new(Shape::Sphere { r }, Pose::IDENTITY, &named(stuff).unwrap())], Pose::at(at)))
}

fn tilt(b: &Body) -> f32 {
    // How far its up axis has turned from up (degrees).
    let up = b.pose.rotate([0.0, -1.0, 0.0]);
    dot(up, [0.0, -1.0, 0.0]).clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn a_box_falls_and_rests_on_the_ground_then_sleeps() {
    let mut w = world();
    let b = boxy(&mut w, [0.25; 3], [0.0, -2.0, 0.0], "wood");
    let mut lowest = f32::NEG_INFINITY;
    for _ in 0..240 {
        w.step();
        lowest = lowest.max(w.bodies[b].pose.p[1]);
    }
    let y = w.bodies[b].pose.p[1];
    assert!((y + 0.25).abs() < 0.01, "rests on its face: {y}");
    assert!(lowest < 0.0, "never through the ground");
    assert!(!w.bodies[b].awake, "sleeps once still");
    // Wood: 650 kg/m³ × 0.125 m³.
    assert!((w.bodies[b].mass - 81.25).abs() < 0.5, "{}", w.bodies[b].mass);
}

#[test]
fn a_tall_stack_of_boxes_stands() {
    let mut w = world();
    let ids: Vec<usize> = (0..10).map(|k| boxy(&mut w, [0.2; 3], [0.0, -0.2 - 0.4 * k as f32 - 0.001 * k as f32, 0.0], "wood")).collect();
    w.advance(5.0);
    let top = &w.bodies[*ids.last().unwrap()];
    assert!(top.pose.p[0].abs() < 0.02 && top.pose.p[2].abs() < 0.02, "the top stays over the bottom: {:?}", top.pose.p);
    assert!((top.pose.p[1] + 3.8).abs() < 0.05, "no sinking: {:?}", top.pose.p);
    assert!(ids.iter().all(|&i| !w.bodies[i].awake), "the stack sleeps");
}

#[test]
fn dominoes_knock_each_other_down() {
    let mut w = world();
    // 2 cm × 10 cm × 5 cm tiles standing 8 cm apart.
    let tiles: Vec<usize> = (0..12).map(|k| boxy(&mut w, [0.0075, 0.05, 0.025], [k as f32 * 0.07, -0.05, 0.0], "wood")).collect();
    w.bodies[tiles[0]].apply_impulse([0.02, 0.0, 0.0], [0.0, -0.09, 0.0]);
    w.advance(4.0);
    let fallen = tiles.iter().filter(|&&t| tilt(&w.bodies[t]) > 50.0).count();
    assert!(fallen >= 11, "the row falls: {fallen} of 12, tilts {:?}", tiles.iter().map(|&t| tilt(&w.bodies[t]) as i32).collect::<Vec<_>>());
}

#[test]
fn a_fast_ball_does_not_pass_through_a_thin_wall() {
    let mut w = world();
    let glass = named("glass").unwrap();
    w.add(Body::new(Kind::Static, vec![Collider::new(Shape::cuboid([0.005, 1.0, 1.0]), Pose::IDENTITY, &glass)], Pose::at([2.0, -1.0, 0.0])));
    let b = ball(&mut w, 0.02, [0.0, -1.0, 0.0], "steel");
    w.bodies[b].gravity_scale = 0.0;
    w.bodies[b].v = [150.0, 0.0, 0.0];
    w.advance(0.2);
    assert!(w.bodies[b].pose.p[0] < 2.0, "stopped at the wall: {:?}", w.bodies[b].pose.p);
}

#[test]
fn a_ball_bounces_to_a_fraction_of_its_height() {
    let mut w = world();
    let b = ball(&mut w, 0.1, [0.0, -1.1, 0.0], "rubber");
    let mut peak_after = f32::INFINITY;
    let mut landed = false;
    for _ in 0..240 {
        w.step();
        let y = w.bodies[b].pose.p[1];
        if !landed && w.bodies[b].v[1] < 0.0 {
            landed = true;
        }
        if landed {
            peak_after = peak_after.min(y);
        }
    }
    // Rubber on concrete: bounce 0.8, so it rises to ~64% of 1 m.
    let rise = -(peak_after + 0.1);
    assert!(rise > 0.45 && rise < 0.8, "rises {rise} m");
}

#[test]
fn friction_holds_a_box_on_a_gentle_slope_but_not_a_steep_one() {
    for (deg, slides) in [(15.0f32, false), (45.0, true)] {
        let mut w = World::new([0.0, 9.81, 0.0]);
        let q = studio_mesh::rig::q_axis_angle([0.0, 0.0, 1.0], deg.to_radians());
        let wood = named("wood").unwrap();
        w.add(Body::new(Kind::Static, vec![Collider::new(Shape::cuboid([5.0, 0.1, 2.0]), Pose::IDENTITY, &wood)], Pose::new([0.0; 3], q)));
        let up = q_rotate(q, [0.0, -1.0, 0.0]);
        let b = w.add(Body::new(Kind::Dynamic, vec![Collider::new(Shape::cuboid([0.2; 3]), Pose::IDENTITY, &wood)], Pose::new(scale(up, 0.301), q)));
        let start = w.bodies[b].pose.p;
        w.advance(1.5);
        let moved = length(sub(w.bodies[b].pose.p, start));
        assert_eq!(moved > 0.3, slides, "{deg}°: moved {moved}");
    }
}

#[test]
fn a_pendulum_on_a_hinge_swings_with_the_right_period() {
    let mut w = world();
    let pivot = w.add(Body::new(Kind::Static, vec![Collider::new(Shape::Sphere { r: 0.01 }, Pose::IDENTITY, &named("steel").unwrap())], Pose::at([0.0, -3.0, 0.0])));
    // A small heavy bob 1 m out to the side, swinging about z.
    let bob = ball(&mut w, 0.03, [1.0, -3.0, 0.0], "steel");
    let j = Joint::between(&w.bodies, pivot, bob, [0.0, -3.0, 0.0], [1.0, -3.0, 0.0], JointKind::Spring { rest: 1.0, hertz: 0.0, damping: 0.0, min: 0.0, max: f32::INFINITY });
    w.join(j);
    let mut crossings = Vec::new();
    let mut last = w.bodies[bob].pose.p[0];
    for i in 0..600 {
        w.step();
        let x = w.bodies[bob].pose.p[0];
        if last > 0.0 && x <= 0.0 {
            crossings.push(i as f32 * w.settings.dt);
        }
        last = x;
    }
    assert!(crossings.len() >= 2, "{crossings:?}");
    let period = crossings[1] - crossings[0];
    // Released at 90°: T ≈ 1.18 × 2π √(L/g).
    let expect = 1.18 * std::f32::consts::TAU * (1.0f32 / 9.81).sqrt();
    assert!((period - expect).abs() / expect < 0.06, "period {period} vs {expect}");
    let r = length(sub(w.bodies[bob].pose.p, [0.0, -3.0, 0.0]));
    assert!((r - 1.0).abs() < 0.01, "the rod keeps its length: {r}");
}

#[test]
fn a_hinged_door_turns_only_about_its_hinge_within_limits() {
    let mut w = World::new([0.0, 9.81, 0.0]);
    let wall = w.add(Body::new(Kind::Static, vec![Collider::new(Shape::cuboid([0.05; 3]), Pose::IDENTITY, &named("wood").unwrap())], Pose::at([0.0, -1.0, 0.0])));
    let door = boxy(&mut w, [0.4, 1.0, 0.02], [0.45, -1.0, 0.0], "wood");
    // Hinge up the door's edge (y axis), opening 0..100°.
    w.join(Joint::at(&w.bodies, wall, door, [0.05, -1.0, 0.0], [0.0, -1.0, 0.0], JointKind::Hinge { limits: Some((0.0, 100f32.to_radians())), motor: Some(Motor { speed: 3.0, max: 200.0 }), spring: None }));
    w.advance(3.0);
    let ang = w.joints[0].position(&w.bodies).to_degrees();
    assert!(ang > 90.0 && ang < 104.0, "opened to its stop: {ang}");
    let edge = w.bodies[door].pose.apply([-0.4, 0.0, 0.0]);
    assert!(length(sub(edge, [0.05, -1.0, 0.0])) < 0.02, "hangs on its hinge: {edge:?}");
}

#[test]
fn a_chain_of_ball_joints_hangs_straight_down() {
    let mut w = world();
    let top = w.add(Body::new(Kind::Static, vec![Collider::new(Shape::Sphere { r: 0.02 }, Pose::IDENTITY, &named("steel").unwrap())], Pose::at([0.0, -3.0, 0.0])));
    let mut prev = top;
    let mut links = Vec::new();
    for k in 0..8 {
        // Links sticking out sideways to begin with.
        let l = w.add(Body::new(Kind::Dynamic, vec![Collider::new(Shape::Capsule { half: 0.08, r: 0.02 }, Pose::new([0.0; 3], studio_mesh::rig::q_axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2)), &named("steel").unwrap())], Pose::at([0.1 + 0.2 * k as f32, -3.0, 0.0])));
        w.bodies[l].angular_damping = 2.0;
        w.bodies[l].linear_damping = 0.5;
        let at = [0.2 * k as f32, -3.0, 0.0];
        w.join(Joint::at(&w.bodies, prev, l, at, [1.0, 0.0, 0.0], JointKind::Ball { cone: None, twist: None, drive: None }));
        links.push(l);
        prev = l;
    }
    w.advance(8.0);
    let end = w.bodies[*links.last().unwrap()].pose.p;
    assert!(end[0].abs() < 0.3 && end[1] > -3.0 + 1.4, "hangs below the pin: {end:?}");
}

#[test]
fn the_same_scene_plays_the_same_every_time() {
    let run = || {
        let mut w = world();
        for k in 0..6 {
            boxy(&mut w, [0.2; 3], [0.05 * k as f32, -0.5 - 0.45 * k as f32, 0.03 * k as f32], "wood");
        }
        let b = ball(&mut w, 0.15, [-2.0, -0.3, 0.0], "steel");
        w.bodies[b].v = [8.0, 0.0, 0.0];
        w.advance(2.0);
        w.bodies.iter().map(|b| (b.pose.p, b.pose.q)).collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn a_moving_kinematic_body_pushes_but_is_not_pushed() {
    let mut w = world();
    let crate_ = boxy(&mut w, [0.2; 3], [1.0, -0.2, 0.0], "cardboard");
    let pusher = w.add(Body::new(Kind::Kinematic, vec![Collider::new(Shape::Capsule { half: 0.5, r: 0.2 }, Pose::IDENTITY, &named("flesh").unwrap())], Pose::at([0.0, -0.7, 0.0])));
    for i in 0..240 {
        let x = 1.5 * (i + 1) as f32 / 240.0;
        w.bodies[pusher].target = Some(Pose::at([x, -0.7, 0.0]));
        w.step();
    }
    assert!((w.bodies[pusher].pose.p[0] - 1.5).abs() < 1e-4, "on its path");
    assert!(w.bodies[crate_].pose.p[0] > 1.6, "the crate was shoved: {:?}", w.bodies[crate_].pose.p);
}

#[test]
fn shapes_made_from_a_mesh_rest_on_a_triangle_floor() {
    let mut w = World::new([0.0, 9.81, 0.0]);
    let floor = vec![[[-5.0, 0.0, -5.0], [5.0, 0.0, 5.0], [5.0, 0.0, -5.0]], [[-5.0, 0.0, -5.0], [-5.0, 0.0, 5.0], [5.0, 0.0, 5.0]]];
    w.add(Body::new(Kind::Static, vec![Collider::new(Shape::Mesh(Arc::new(shape::TriMesh::new(floor))), Pose::IDENTITY, &named("wood").unwrap())], Pose::IDENTITY));
    // An L-shaped model: falls, rests, does not sink.
    let mut tris: Vec<[V3; 3]> = Vec::new();
    let m = studio_mesh::prim::cube([0.6, 0.2, 0.2]);
    tris.extend((0..m.triangles()).map(|t| m.tri(t).map(|v| m.positions[v])));
    let m2 = studio_mesh::prim::cube([0.2, 0.6, 0.2]);
    tris.extend((0..m2.triangles()).map(|t| m2.tri(t).map(|v| add(m2.positions[v], [-0.2, -0.4, 0.0]))));
    let shapes = decompose::shapes_for(&tris, decompose::Fit::Auto, 32).unwrap();
    let cols = shapes.parts.into_iter().map(|(s, p)| Collider::new(s, p, &named("wood").unwrap())).collect();
    let b = w.add(Body::new(Kind::Dynamic, cols, Pose::at([0.0, -1.5, 0.0])));
    w.advance(4.0);
    let lowest = w.bodies[b].colliders.iter().flat_map(|c| match &c.shape {
        Shape::Hull(h) => h.verts.iter().map(|v| w.bodies[b].pose.then(&c.local).apply(*v)[1]).collect::<Vec<_>>(),
        _ => vec![],
    }).fold(f32::NEG_INFINITY, f32::max);
    assert!(lowest > -0.03 && lowest < 0.02, "rests on the floor: lowest point {lowest}");
}
