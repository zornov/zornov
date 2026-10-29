// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Probes for the superpowers map (docs/superpowers-map.md). Each test
//! measures one behaviour found during the film asset analysis, so the
//! map's claims rest on runs, not only on reading.

use studio_mesh::{autorig, prim, Mesh};

/// A bear-like body: a long torso ellipsoid plus a head lump at one end,
/// laid along `axis` (0 = x, 2 = z), head at the low end, y down.
fn body_along(axis: usize) -> Mesh {
    let mut m = prim::uv_sphere(1.0, 32, 16);
    for p in &mut m.positions {
        let (a, b, c) = (p[0], p[1], p[2]);
        // torso: 150 long, 55 tall, 55 wide
        let mut q = [0.0f32; 3];
        q[axis] = a * 150.0;
        q[1] = b * 55.0;
        q[if axis == 0 { 2 } else { 0 }] = c * 55.0;
        *p = q;
    }
    m
}

fn dir(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    [d[0] / l, d[1] / l, d[2] / l]
}

fn spine_axis(m: &Mesh) -> ([f32; 3], f32, f32) {
    let rig = autorig::quadruped(m);
    let hips = rig.bones.iter().find(|b| b.name == "hips").unwrap();
    let head = rig.bones.iter().find(|b| b.name == "head").unwrap();
    let d = dir(hips.head, head.tail);
    // how far apart the left and right front legs are, on x and on z
    let fl = rig.bones.iter().find(|b| b.name == "leg_front.L.0").unwrap();
    let fr = rig.bones.iter().find(|b| b.name == "leg_front.R.0").unwrap();
    (d, (fl.head[0] - fr.head[0]).abs(), (fl.head[2] - fr.head[2]).abs())
}

/// The body of [`body_along`] with a head: a ball on top of the end at
/// `+axis` (`sign` 1) or `-axis` (`sign` -1), raised above the back.
fn bear_along(axis: usize, sign: f32) -> Mesh {
    let mut m = body_along(axis);
    let mut head = prim::uv_sphere(38.0, 24, 12);
    for p in &mut head.positions {
        p[axis] += sign * 150.0;
        p[1] -= 40.0;
    }
    studio_mesh::ops::merge(&mut m, &head);
    m
}

#[test]
fn quadruped_template_follows_the_body_s_long_axis() {
    let (dx, leg_x_dx, leg_x_dz) = spine_axis(&body_along(0));
    let (dz, leg_dx, leg_dz) = spine_axis(&body_along(2));
    println!("body along x: hips->head {dx:?}; front legs apart by x {leg_x_dx:.1}, z {leg_x_dz:.1}");
    println!("body along z: hips->head {dz:?}; front legs apart by x {leg_dx:.1}, z {leg_dz:.1}");
    // Along x the spine runs down the body and the legs split across it.
    assert!(dx[0].abs() > 0.9, "control: spine follows x for an x-long body: {dx:?}");
    assert!(leg_x_dz > leg_x_dx, "control: legs split across an x-long body");
    // Along z (how a formula-built animal with its length on formula y
    // arrives after the (x, -z, y) remap) the spine follows z and the legs
    // split across the body (along x), not along its length.
    assert!(dz[2].abs() > 0.9 && dz[0].abs() < 0.2, "spine follows the long axis: {dz:?}");
    assert!(leg_dx > leg_dz && leg_dx > 30.0, "left/right legs split across the body: x {leg_dx}, z {leg_dz}");
    // A diagonal body (turned 30 degrees about up) gets a diagonal spine.
    let mut diag = body_along(0);
    let (s, c) = 30f32.to_radians().sin_cos();
    for p in &mut diag.positions {
        *p = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
    }
    let (dd, ..) = spine_axis(&diag);
    let along = (dd[0] * c + dd[2] * s).abs();
    assert!(along > 0.9, "spine follows a turned body: {dd:?}");
}

#[test]
fn quadruped_head_goes_to_the_end_with_the_head() {
    for (axis, sign) in [(0usize, 1.0f32), (0, -1.0), (2, 1.0), (2, -1.0)] {
        let rig = autorig::quadruped(&bear_along(axis, sign));
        let hips = rig.bones.iter().find(|b| b.name == "hips").unwrap();
        let head = rig.bones.iter().find(|b| b.name == "head").unwrap();
        let d = dir(hips.head, head.tail);
        println!("head at {sign} along axis {axis}: hips->head {d:?}");
        assert!(d[axis] * sign > 0.9, "head toward the head lump (axis {axis}, sign {sign}): {d:?}");
        // The head bone ends near the head lump, not at the bare end.
        assert!(head.tail[axis] * sign > 120.0, "head bone reaches the head: {:?}", head.tail);
    }
}

#[test]
fn quadruped_refuses_a_body_it_cannot_orient() {
    // Round in plan: no long axis.
    let mut ball = prim::uv_sphere(1.0, 24, 12);
    for p in &mut ball.positions {
        *p = [p[0] * 60.0, p[1] * 40.0, p[2] * 60.0];
    }
    let e = autorig::try_quadruped(&ball, None).unwrap_err();
    assert!(e.contains("forward"), "{e}");
    // A long body with alike ends (a capsule) takes the default head end.
    assert!(autorig::try_quadruped(&body_along(2), None).is_ok());
    assert!(autorig::try_quadruped(&body_along(2), Some([0.0, 0.0, 1.0])).is_ok());
    // A clear body is fine.
    assert!(autorig::try_quadruped(&bear_along(2, 1.0), None).is_ok());
}

#[test]
fn smodel_is_named_not_mistaken_for_a_junk_file() {
    assert!(studio_mesh::foreign::is_model_ext("smodel"));
    let dir = std::env::temp_dir().join(format!("zornov-smodel-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.smodel");
    std::fs::write(&f, b"not really").unwrap();
    let e = studio_mesh::import::load(&f, 0.0).err().unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert!(e.contains("editable model source"), "{e}");
}

#[test]
fn l_system_forks_thin_once_and_keep_the_pipe_rule() {
    use studio_mesh::math::growth::{Turtle, pipe_radii, turtle};
    // `![` thins once, not twice, per level.
    let g = turtle("F![F![F]]", &Turtle { taper: 0.7, width: 1.0, ..Turtle::default() });
    let w: Vec<f64> = g.branches.iter().map(|b| b.radii[0]).collect();
    println!("base radius per branch {w:?}");
    assert!((w[1] / w[0] - 0.7).abs() < 1e-9 && (w[2] / w[1] - 0.7).abs() < 1e-9, "{w:?}");
    // Pipe rule at a fork: r_parent^2 is about the sum of the children's.
    let g = turtle("FFFF[+FFF][-FFF]", &Turtle::default());
    let r = pipe_radii(&g, 1.0, 2.0);
    let at_fork = r[0][4];
    let kids: f64 = (1..g.branches.len()).map(|b| r[b][0].powi(2)).sum();
    assert!((at_fork.powi(2) - kids).abs() / at_fork.powi(2) < 0.35, "fork {at_fork}, kids {kids}");
}

#[test]
fn a_tree_rigs_by_branch_and_a_branch_swings_alone() {
    use studio_mesh::math::growth::{Turtle, pipe_radii, turtle};
    use studio_mesh::{rig::PoseInput, treerig};
    let g = turtle("FFFF[+FFF[+FF]-FF][-FFF]FF", &Turtle { angle: 35.0, step: 10.0, width: 2.0, ..Turtle::default() });
    let place = |p: [f64; 3]| [p[0] as f32, -(p[2] as f32), p[1] as f32];
    let (mut m, ids) = treerig::bark(&g, &pipe_radii(&g, 2.0, 2.0), &|_| 6, &place);
    treerig::rig_mesh(&mut m, &ids, &g, &place, 2).unwrap();
    let rig = m.rig.clone().unwrap();
    assert!(rig.bones.len() >= g.branches.len(), "{} bones for {} branches", rig.bones.len(), g.branches.len());
    let posed = rig.posed_mesh(&m, &PoseInput { rotations: [("branch.1.0".to_string(), [50.0, 0.0, 0.0])].into(), ..Default::default() });
    let moved = |id: u32| (0..m.positions.len()).filter(|&v| ids[v] == id).map(|v| {
        let (a, b) = (posed.positions[v], m.positions[v]);
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }).fold(0.0f32, f32::max);
    assert!(moved(1) > 3.0 && moved(0) < 1e-3, "branch 1 moved {}, trunk {}", moved(1), moved(0));
}
