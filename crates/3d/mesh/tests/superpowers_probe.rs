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

#[test]
fn quadruped_template_follows_x_only() {
    let (dx, ..) = spine_axis(&body_along(0));
    let (dz, leg_dx, leg_dz) = spine_axis(&body_along(2));
    println!("body along x: hips->head direction {dx:?}");
    println!("body along z: hips->head direction {dz:?}; front legs apart by x {leg_dx:.1}, z {leg_dz:.1}");
    // Along x the spine runs down the body.
    assert!(dx[0].abs() > 0.9, "control: spine should follow x for an x-long body");
    // Along z (how a formula-built animal with its length on formula y
    // arrives after the (x, -z, y) remap) the template still lays the spine
    // on x, across the 110 px-wide torso, and spreads the legs along the
    // body's length instead of across it.
    assert!(dz[0].abs() > 0.9 && dz[2].abs() < 0.2, "spine ignores the long axis: {dz:?}");
    assert!(leg_dz > leg_dx, "left/right legs are split along the body's length");
}
