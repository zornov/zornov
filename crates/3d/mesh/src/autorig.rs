// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rigging without hand work: armatures from templates fitted to a model,
//! and skin weights worked out from the bones.
//!
//! - [`humanoid`]: a biped armature (hips, spine, chest, neck, head,
//!   shoulders, arms, hands, legs, feet, toes) fitted to a standing figure
//!   facing -z, in a T-pose, an A-pose or with its arms down; each joint
//!   is pulled to the middle of the body's cross-section around it.
//! - [`chain`]: a row of bones along a model (a tail, a rope, a tentacle,
//!   a snake, a flag pole).
//! - [`quadruped`]: four legs, a spine, a neck, a head and a tail for an
//!   animal standing along x.
//! - [`bind`]: skin weights. Each vertex goes to the bones it can see
//!   (a straight line inside the body reaches them, so an arm does not
//!   pull the side of the chest it rests against), nearer bones weighing
//!   more; then the weights are smoothed over the surface so joints bend
//!   softly, and each vertex keeps its four strongest bones.

use crate::bvh::Bvh;
use crate::rig::{Bone, Constraint, Rig, Target};
use crate::proportions::Proportions;
use crate::{Mesh, V3, add, dot, length, normalise, scale, sub};

/// How a humanoid's arms hang.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arms {
    /// Straight out to the sides.
    T,
    /// Down at about 40°.
    A,
    /// Measured from the model's width.
    Auto,
}

/// Lengths along an arm, from the shoulder joint (px), for a figure `h`
/// tall: to the elbow, to the wrist, and the hand's length.
pub fn arm_lengths(pr: &Proportions, h: f32) -> (f32, f32, f32) {
    (0.175 * h * pr.arm, 0.325 * h * pr.arm, 0.095 * h * pr.hand)
}

/// A biped armature for a figure standing in the box `lo..hi` (y down,
/// facing -z). `arms` says how the arms hang.
pub fn humanoid(mesh: &Mesh, arms: Arms) -> Rig {
    humanoid_fitted(mesh, arms, &Proportions::default())
}

/// A biped armature fitted to a figure with these proportions. Every
/// humanoid has the same bones: `hips`, `spine`, `chest`, `neck`, `head`,
/// `eye.L/R`; per side `shoulder`, `upper_arm`, `forearm`, `hand`, a hand's
/// `thumb`, `index`, `middle`, `ring` and `pinky` each of three bones
/// (`index.L.0`…`.2`), a `grip` control bone at the middle of the fist,
/// `thigh`, `shin`, `foot` and `toe`.
pub fn humanoid_fitted(mesh: &Mesh, arms: Arms, pr: &Proportions) -> Rig {
    let (lo, hi) = mesh.bounds();
    let h = (hi[1] - lo[1]).max(1.0);
    let cx = (lo[0] + hi[0]) * 0.5;
    let cz = (lo[2] + hi[2]) * 0.5;
    let y = |f: f32| lo[1] + pr.y(f) * h;
    let half_span = (hi[0] - lo[0]) * 0.5;
    let shoulder_x = 0.115 * h;
    let (elbow_len, wrist_len, hand_len) = arm_lengths(pr, h);
    let arm_len = wrist_len + hand_len;
    let droop = match arms {
        Arms::T => 0.0,
        Arms::A => 40f32.to_radians(),
        Arms::Auto => ((half_span - shoulder_x) / arm_len).clamp(0.05, 1.0).acos(),
    };
    let mut r = Rig::default();
    let mut add_bone = |name: &str, parent: Option<usize>, head: V3, tail: V3| -> usize {
        r.bones.push(Bone::new(name, parent, head, tail));
        r.bones.len() - 1
    };
    let hips = add_bone("hips", None, [cx, y(0.53), cz], [cx, y(0.46), cz]);
    let spine = add_bone("spine", Some(hips), [cx, y(0.46), cz], [cx, y(0.35), cz]);
    let chest = add_bone("chest", Some(spine), [cx, y(0.35), cz], [cx, y(0.185), cz]);
    let neck = add_bone("neck", Some(chest), [cx, y(0.185), cz], [cx, y(0.13), cz]);
    add_bone("head", Some(neck), [cx, y(0.13), cz], [cx, y(0.0), cz]);
    for (side, sx) in [("L", 1.0f32), ("R", -1.0)] {
        let sh = add_bone(&format!("shoulder.{side}"), Some(chest), [cx + sx * 0.02 * h, y(0.2), cz], [cx + sx * shoulder_x, y(0.195), cz]);
        let dir = [sx * droop.cos(), droop.sin(), 0.0];
        let s0 = [cx + sx * shoulder_x, y(0.195), cz];
        let elbow = add(s0, scale(dir, elbow_len));
        // A slight bend at the elbow tells IK which way it folds.
        let elbow = add(elbow, [0.0, 0.0, 0.004 * h]);
        let wrist = add(s0, scale(dir, wrist_len));
        let tip = add(s0, scale(dir, arm_len));
        let ua = add_bone(&format!("upper_arm.{side}"), Some(sh), s0, elbow);
        let fa = add_bone(&format!("forearm.{side}"), Some(ua), elbow, wrist);
        add_bone(&format!("hand.{side}"), Some(fa), wrist, tip);
        let hip_x = cx + sx * 0.09 * h;
        let th = add_bone(&format!("thigh.{side}"), Some(hips), [hip_x, y(0.53), cz], [hip_x, y(0.74), cz - 0.006 * h]);
        let shin = add_bone(&format!("shin.{side}"), Some(th), [hip_x, y(0.74), cz - 0.006 * h], [hip_x, y(0.955), cz + 0.01 * h]);
        let foot = add_bone(&format!("foot.{side}"), Some(shin), [hip_x, y(0.955), cz + 0.01 * h], [hip_x, y(0.99), cz - 0.07 * h * pr.foot]);
        add_bone(&format!("toe.{side}"), Some(foot), [hip_x, y(0.99), cz - 0.07 * h * pr.foot], [hip_x, y(1.0), cz - 0.115 * h * pr.foot]);
    }
    centre_joints(mesh, &mut r, h);
    add_head_details(&mut r, pr, h, cz);
    add_hands(&mut r, cx);
    humanoid_controls(&mut r);
    r
}

/// Eye bones on the head, pointing forward from where the eyes sit in a
/// head of this size (a head base places them exactly afterwards).
fn add_head_details(r: &mut Rig, pr: &Proportions, h: f32, cz: f32) {
    let Some(head) = r.bone("head") else { return };
    let (top, size) = (r.bones[head].tail, (pr.head * h).max(1.0));
    for (side, sx) in [("L", 1.0f32), ("R", -1.0)] {
        let c = [top[0] + sx * 0.12 * size, top[1] + 0.53 * size, cz - 0.375 * size];
        let mut eye = Bone::new(format!("eye.{side}"), Some(head), c, add(c, [0.0, 0.0, -0.06 * size]));
        // Control bones until there are eyeballs for them to carry.
        eye.deform = false;
        r.bones.push(eye);
    }
    // The jaw and the tongue's three bones, for lip-sync (a head base fits them).
    let s = size;
    let o = [top[0], top[1] + 0.53 * s, cz - 0.07 * s - 0.305 * s];
    let at = |p: V3| [o[0] + p[0] * s, o[1] + p[1] * s, o[2] + (p[2] + 0.305) * s];
    let mut jaw = Bone::new("jaw", Some(head), at([0.0, 0.12, 0.08]), at([0.0, 0.45, -0.2]));
    jaw.deform = false;
    jaw.roll = std::f32::consts::PI;
    r.bones.push(jaw);
    let jaw_i = r.bones.len() - 1;
    // Heights in metres; 0.318 only looks like 1/π.
    #[allow(clippy::approx_constant)]
    let tp = [[0.0, 0.318, -0.19], [0.0, 0.317, -0.245], [0.0, 0.315, -0.3], [0.0, 0.312, -0.345]];
    let mut parent = jaw_i;
    for k in 0..3 {
        let mut t = Bone::new(format!("tongue.{k}"), Some(parent), at(tp[k]), at(tp[k + 1]));
        t.deform = false;
        r.bones.push(t);
        parent = r.bones.len() - 1;
    }
}

/// Finger bones, and a grip bone, on each hand of a rig that has `hand.*`.
/// Rigs made by hand-placed bones keep theirs: this does nothing for a hand
/// that already has a finger.
pub fn add_hands(r: &mut Rig, mid_x: f32) {
    for (side, left) in [("L", true), ("R", false)] {
        let Some(hand) = r.bone(&format!("hand.{side}")) else { continue };
        if r.bone(&format!("index.{side}.0")).is_some() {
            continue;
        }
        let (wrist, tip) = (r.bones[hand].head, r.bones[hand].tail);
        let len = length(sub(tip, wrist)).max(1.0);
        let frame = crate::hand::Frame::hanging(wrist, sub(tip, wrist), len, left, mid_x);
        for d in crate::hand::digits(len, 0.0) {
            let mut parent = hand;
            for seg in 0..3 {
                let (a, b) = (frame.to_model(d.joints[seg]), frame.to_model(d.joints[seg + 1]));
                r.bones.push(Bone::new(crate::hand::bone_name(d.name, left, seg), Some(parent), a, b));
                parent = r.bones.len() - 1;
            }
        }
        let g = frame.to_model(crate::hand::grip_point(len));
        let mut grip = Bone::new(format!("grip.{side}"), Some(hand), g, add(g, scale(frame.dir_to_model([0.0, 0.0, 1.0]), 0.25 * len)));
        grip.deform = false;
        r.bones.push(grip);
    }
}

/// IK for the feet and hands (off until a target is set: influence 0) and
/// a look-at for the head, with poles that bend knees forward and elbows
/// back. Their names: `foot.L`, `foot.R`, `hand.L`, `hand.R`, `look`.
pub fn humanoid_controls(r: &mut Rig) {
    let h = {
        let (a, b) = (r.bone("head").map(|i| r.bones[i].tail), r.bone("toe.L").map(|i| r.bones[i].tail));
        match (a, b) {
            (Some(a), Some(b)) => (b[1] - a[1]).abs().max(1.0),
            _ => 100.0,
        }
    };
    for side in ["L", "R"] {
        if let Some(shin) = r.bone(&format!("shin.{side}")) {
            let knee = r.bones[shin].head;
            let ankle = r.bones[shin].tail;
            r.constraints.push(Constraint::Ik { name: format!("foot.{side}"), bone: shin, chain: 2, target: Target::Point(ankle), pole: Some(add(knee, [0.0, 0.0, -h])), influence: 0.0 });
        }
        if let Some(fa) = r.bone(&format!("forearm.{side}")) {
            let elbow = r.bones[fa].head;
            let wrist = r.bones[fa].tail;
            r.constraints.push(Constraint::Ik { name: format!("hand.{side}"), bone: fa, chain: 2, target: Target::Point(wrist), pole: Some(add(elbow, [0.0, 0.0, h])), influence: 0.0 });
        }
    }
    if let Some(head) = r.bone("head") {
        let p = r.bones[head].head;
        r.constraints.push(Constraint::LookAt { name: "look".into(), bone: head, target: Target::Point(add(p, [0.0, -0.13 * h, -h])), influence: 0.0 });
    }
    // The eyes follow a look target too (off until it is set).
    let mid = match (r.bone("eye.L"), r.bone("eye.R")) {
        (Some(a), Some(b)) => Some(crate::lerp(r.bones[a].head, r.bones[b].head, 0.5)),
        _ => None,
    };
    if let Some(mid) = mid {
        for side in ["L", "R"] {
            if let Some(e) = r.bone(&format!("eye.{side}")) {
                r.constraints.push(Constraint::LookAt { name: format!("eyes.{side}"), bone: e, target: Target::Point(add(mid, [0.0, 0.0, -10.0 * h])), influence: 0.0 });
            }
        }
    }
}

/// Pulls each joint to the middle of the body around it: the centroid of
/// the vertices in a thin slice across the bone at that joint.
fn centre_joints(mesh: &Mesh, r: &mut Rig, h: f32) {
    let slab = 0.018 * h;
    let reach = 0.11 * h;
    let mut moved: Vec<(V3, V3)> = Vec::with_capacity(r.bones.len());
    for b in &r.bones {
        let axis = normalise(sub(b.tail, b.head));
        let fix = |p: V3| -> V3 {
            let mut acc = [0.0f32; 3];
            let mut n = 0.0;
            for v in &mesh.positions {
                let d = sub(*v, p);
                let along = dot(d, axis);
                if along.abs() > slab {
                    continue;
                }
                let across = sub(d, scale(axis, along));
                if length(across) > reach {
                    continue;
                }
                acc = add(acc, across);
                n += 1.0;
            }
            if n < 8.0 { p } else { add(p, scale(acc, 1.0 / n)) }
        };
        moved.push((fix(b.head), fix(b.tail)));
    }
    // Joints shared by parent tail and child head move together, and the
    // spine stays on the middle line.
    for i in 0..r.bones.len() {
        let (mut head, mut tail) = moved[i];
        if let Some(p) = r.bones[i].parent
            && length(sub(r.bones[p].tail, r.bones[i].head)) < 1e-3
        {
            head = moved[p].1;
        }
        let centre_line = !r.bones[i].name.contains('.');
        if centre_line {
            head[0] = r.bones[i].head[0];
            tail[0] = r.bones[i].tail[0];
        }
        // Ends of limbs (hand and toe tips, the top of the head) keep their
        // length along the bone.
        if r.children(i).is_empty() {
            tail = add(head, sub(r.bones[i].tail, r.bones[i].head));
        }
        moved[i] = (head, tail);
    }
    for (b, (h2, t2)) in r.bones.iter_mut().zip(moved) {
        b.head = h2;
        b.tail = t2;
    }
}

/// `count` bones along the model's longest side (from the end nearest
/// `start`, a side name: top, bottom, left, right, front, back).
pub fn chain(mesh: &Mesh, count: usize, start: &str, name: &str) -> Rig {
    let (lo, hi) = mesh.bounds();
    let ext: V3 = std::array::from_fn(|i| hi[i] - lo[i]);
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
    let axis = match start {
        "top" | "bottom" => 1,
        "left" | "right" => 0,
        "front" | "back" => 2,
        _ => axis,
    };
    // From the low end of the axis (top, left, front) unless told otherwise.
    let from_low = !matches!(start, "bottom" | "right" | "back");
    let c = scale(add(lo, hi), 0.5);
    let mut a = c;
    let mut b = c;
    a[axis] = if from_low { lo[axis] } else { hi[axis] };
    b[axis] = if from_low { hi[axis] } else { lo[axis] };
    let n = count.clamp(1, 64);
    let mut r = Rig::default();
    for i in 0..n {
        let h0 = crate::lerp(a, b, i as f32 / n as f32);
        let h1 = crate::lerp(a, b, (i + 1) as f32 / n as f32);
        r.bones.push(Bone::new(format!("{name}.{i}"), if i == 0 { None } else { Some(i - 1) }, h0, h1));
    }
    let h = length(sub(b, a));
    centre_joints(mesh, &mut r, h * 0.5);
    r
}

/// An animal standing along x, head towards -x: spine, neck, head, jaw,
/// tail and four legs (`leg_front.L`… with `.0` upper and `.1` lower,
/// `paw_front.L`…).
pub fn quadruped(mesh: &Mesh) -> Rig {
    let (lo, hi) = mesh.bounds();
    let len = (hi[0] - lo[0]).max(1.0);
    let ht = (hi[1] - lo[1]).max(1.0);
    let cz = (lo[2] + hi[2]) * 0.5;
    let x = |f: f32| lo[0] + f * len;
    let y = |f: f32| lo[1] + f * ht;
    let half_w = (hi[2] - lo[2]) * 0.3;
    let mut r = Rig::default();
    let mut b = |name: &str, parent: Option<usize>, head: V3, tail: V3| -> usize {
        r.bones.push(Bone::new(name, parent, head, tail));
        r.bones.len() - 1
    };
    let hips = b("hips", None, [x(0.72), y(0.35), cz], [x(0.55), y(0.33), cz]);
    let spine = b("spine", Some(hips), [x(0.55), y(0.33), cz], [x(0.3), y(0.32), cz]);
    let neck = b("neck", Some(spine), [x(0.3), y(0.32), cz], [x(0.16), y(0.15), cz]);
    let head = b("head", Some(neck), [x(0.16), y(0.15), cz], [x(0.0), y(0.2), cz]);
    b("jaw", Some(head), [x(0.12), y(0.22), cz], [x(0.02), y(0.27), cz]);
    let mut prev = hips;
    for i in 0..3 {
        let f0 = 0.72 + i as f32 * 0.093;
        prev = b(&format!("tail.{i}"), Some(prev), [x(f0), y(0.33 + i as f32 * 0.02), cz], [x(f0 + 0.093), y(0.35 + i as f32 * 0.03), cz]);
    }
    for (end, fx, parent) in [("front", 0.3f32, spine), ("back", 0.7, hips)] {
        for (side, sz) in [("L", -1.0f32), ("R", 1.0)] {
            let z = cz + sz * half_w;
            let u = b(&format!("leg_{end}.{side}.0"), Some(parent), [x(fx), y(0.4), z], [x(fx + 0.01), y(0.68), z]);
            let l = b(&format!("leg_{end}.{side}.1"), Some(u), [x(fx + 0.01), y(0.68), z], [x(fx), y(0.95), z]);
            b(&format!("paw_{end}.{side}"), Some(l), [x(fx), y(0.95), z], [x(fx - 0.04), y(1.0), z]);
        }
    }
    centre_joints(mesh, &mut r, ht);
    r
}

/// Binds the skin: works out every vertex's weights from the rig's
/// deforming bones (see the module notes). Leaves a rig with no bones
/// alone.
pub fn bind(mesh: &Mesh, rig: &mut Rig) {
    let deform: Vec<usize> = (0..rig.bones.len()).filter(|&i| rig.bones[i].deform).collect();
    if deform.is_empty() || mesh.positions.is_empty() {
        rig.joints.clear();
        rig.weights.clear();
        return;
    }
    let (ids, unique) = mesh.position_ids();
    // Smooth normals per welded position (for starting rays just inside).
    let mut normals = vec![[0.0f32; 3]; unique.len()];
    for t in 0..mesh.triangles() {
        let [a, b, c] = mesh.tri(t);
        let n = crate::cross(sub(mesh.positions[b], mesh.positions[a]), sub(mesh.positions[c], mesh.positions[a]));
        for v in [a, b, c] {
            let k = ids[v] as usize;
            normals[k] = add(normals[k], n);
        }
    }
    let bvh = Bvh::of_mesh(mesh);
    let (lo, hi) = mesh.bounds();
    let size = length(sub(hi, lo)).max(1.0);
    let segs: Vec<(V3, V3)> = deform.iter().map(|&i| (rig.bones[i].head, rig.bones[i].tail)).collect();
    // Initial weights per welded vertex, in parallel.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let chunk = unique.len().div_ceil(threads).max(1);
    let mut w: Vec<Vec<(u16, f32)>> = vec![Vec::new(); unique.len()];
    std::thread::scope(|s| {
        for (ci, out) in w.chunks_mut(chunk).enumerate() {
            let (unique, normals, segs, bvh) = (&unique, &normals, &segs, &bvh);
            s.spawn(move || {
                for (k, slot) in out.iter_mut().enumerate() {
                    let i = ci * chunk + k;
                    let p = unique[i];
                    let inward = scale(normalise(normals[i]), -size * 1e-4);
                    let start = add(p, inward);
                    let mut d: Vec<(usize, f32, V3)> = segs.iter().enumerate().map(|(b, (h, t))| {
                        let q = closest_on_segment(*h, *t, p);
                        (b, length(sub(q, p)), q)
                    }).collect();
                    d.sort_by(|a, b| a.1.total_cmp(&b.1));
                    let nearest = d[0].1.max(size * 1e-4);
                    let mut picked: Vec<(u16, f32)> = Vec::new();
                    for &(b, dist, q) in d.iter().take(8) {
                        if dist > nearest * 2.2 + size * 0.004 {
                            break;
                        }
                        // Seen from inside the body?
                        let to = sub(q, start);
                        let len = length(to);
                        let visible = len < 1e-3 || !bvh.occluded(start, scale(to, 1.0 / len), size * 2e-4, len * 0.98);
                        if !visible && !picked.is_empty() {
                            continue;
                        }
                        let k = (nearest / dist.max(nearest)).powi(4) * if visible { 1.0 } else { 0.05 };
                        picked.push((b as u16, k));
                    }
                    *slot = picked;
                }
            });
        }
    });
    // Smoothed over the surface: each vertex towards its neighbours.
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); unique.len()];
    for t in 0..mesh.triangles() {
        let c = mesh.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            if a != b {
                if !nb[a as usize].contains(&b) {
                    nb[a as usize].push(b);
                }
                if !nb[b as usize].contains(&a) {
                    nb[b as usize].push(a);
                }
            }
        }
    }
    for slot in &mut w {
        normalise_weights(slot);
    }
    for _ in 0..6 {
        let prev = w.clone();
        for (i, slot) in w.iter_mut().enumerate() {
            if nb[i].is_empty() {
                continue;
            }
            let mut acc: Vec<(u16, f32)> = prev[i].iter().map(|&(b, x)| (b, x * 0.5)).collect();
            let k = 0.5 / nb[i].len() as f32;
            for &j in &nb[i] {
                for &(b, x) in &prev[j as usize] {
                    match acc.iter_mut().find(|e| e.0 == b) {
                        Some(e) => e.1 += x * k,
                        None => acc.push((b, x * k)),
                    }
                }
            }
            *slot = acc;
            normalise_weights(slot);
        }
    }
    rig.joints = Vec::with_capacity(mesh.positions.len());
    rig.weights = Vec::with_capacity(mesh.positions.len());
    for &id in &ids {
        let slot = &w[id as usize];
        let mut j = [0u16; 4];
        let mut x = [0f32; 4];
        for (k, &(b, v)) in slot.iter().take(4).enumerate() {
            j[k] = deform[b as usize] as u16;
            x[k] = v;
        }
        let total: f32 = x.iter().sum();
        if total > 0.0 {
            for v in &mut x {
                *v /= total;
            }
        } else {
            x[0] = 1.0;
        }
        rig.joints.push(j);
        rig.weights.push(x);
    }
}

/// Binds every separate piece of the model whole to one bone: the piece
/// takes the deforming bone nearest its middle, with full weight, so a lid turns about its hinge bone as one rigid thing and the
/// body under it stays put (the smooth bind of [`bind`] would blend the two
/// where they meet). Returns how many pieces each bone ended up carrying.
pub fn bind_rigid(mesh: &Mesh, rig: &mut Rig) -> Vec<usize> {
    let deform: Vec<usize> = (0..rig.bones.len()).filter(|&i| rig.bones[i].deform).collect();
    let mut carried = vec![0usize; rig.bones.len()];
    if deform.is_empty() || mesh.positions.is_empty() {
        rig.joints.clear();
        rig.weights.clear();
        return carried;
    }
    let (ids, unique) = mesh.position_ids();
    let mut parent: Vec<u32> = (0..unique.len() as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    for t in 0..mesh.triangles() {
        let c = mesh.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2])] {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            if ra != rb {
                parent[ra as usize] = rb;
            }
        }
    }
    // A piece belongs to the bone nearest its middle (its box centre).
    let mut boxes: std::collections::HashMap<u32, (V3, V3)> = std::collections::HashMap::new();
    for (i, p) in unique.iter().enumerate() {
        let r = root(&mut parent, i as u32);
        let b = boxes.entry(r).or_insert((*p, *p));
        for k in 0..3 {
            b.0[k] = b.0[k].min(p[k]);
            b.1[k] = b.1[k].max(p[k]);
        }
    }
    let winner: std::collections::HashMap<u32, usize> = boxes
        .iter()
        .map(|(r, (lo, hi))| {
            let mid = scale(add(*lo, *hi), 0.5);
            let best = deform.iter().copied().min_by(|&a, &b| {
                let d = |i: usize| length(sub(closest_on_segment(rig.bones[i].head, rig.bones[i].tail, mid), mid));
                d(a).total_cmp(&d(b))
            });
            (*r, best.unwrap_or(deform[0]))
        })
        .collect();
    for &b in winner.values() {
        carried[b] += 1;
    }
    rig.joints = Vec::with_capacity(mesh.positions.len());
    rig.weights = Vec::with_capacity(mesh.positions.len());
    for &id in &ids {
        let b = winner[&root(&mut parent, id)];
        rig.joints.push([b as u16, 0, 0, 0]);
        rig.weights.push([1.0, 0.0, 0.0, 0.0]);
    }
    carried
}

fn normalise_weights(w: &mut Vec<(u16, f32)>) {
    w.retain(|e| e.1 > 1e-4);
    w.sort_by(|a, b| b.1.total_cmp(&a.1));
    w.truncate(6);
    let total: f32 = w.iter().map(|e| e.1).sum();
    if total > 0.0 {
        for e in w.iter_mut() {
            e.1 /= total;
        }
    }
}

/// The point of segment `a`–`b` nearest `p`.
pub fn closest_on_segment(a: V3, b: V3, p: V3) -> V3 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    if l2 < 1e-12 {
        return a;
    }
    let t = (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0);
    add(a, scale(ab, t))
}

/// Rigs a model in one go: the template's bones, the skin bound.
pub fn rig_mesh(mesh: &mut Mesh, template: &str) -> Result<(), String> {
    let mut rig = match template {
        "humanoid" | "biped" | "human" | "person" => humanoid(mesh, Arms::Auto),
        "humanoid_t" | "t_pose" => humanoid(mesh, Arms::T),
        "humanoid_a" | "a_pose" => humanoid(mesh, Arms::A),
        "quadruped" | "animal" | "dog" | "cat" | "horse" => quadruped(mesh),
        "chain" | "tail" | "rope" | "tentacle" | "snake" => chain(mesh, 8, "", "bone"),
        other => return Err(format!("template \"{other}\": humanoid, quadruped or chain")),
    };
    // Keep what the model already had (shape keys, motion).
    if let Some(old) = &mesh.rig {
        rig.shapes = old.shapes.clone();
    }
    bind(mesh, &mut rig);
    mesh.rig = Some(std::sync::Arc::new(rig));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::PoseInput;
    use std::collections::HashMap;

    /// A capsule-ish leg standing 200 px tall.
    fn tube() -> Mesh {
        let mut m = crate::prim::cylinder(30.0, 30.0, 200.0, 24);
        crate::ops::subdivide(&mut m, 2);
        m
    }

    #[test]
    fn a_chain_binds_and_bends_softly() {
        let mut m = tube();
        let mut rig = chain(&m, 2, "top", "seg");
        assert_eq!(rig.bones.len(), 2);
        bind(&m, &mut rig);
        assert_eq!(rig.joints.len(), m.positions.len());
        // The top carries the top, the bottom the bottom, both in between.
        let (lo, hi) = m.bounds();
        let top = (0..m.positions.len()).min_by(|&a, &b| m.positions[a][1].total_cmp(&m.positions[b][1])).unwrap();
        assert_eq!(rig.joints[top][0], 0);
        assert!(rig.weights[top][0] > 0.9, "{:?}", rig.weights[top]);
        let mid = (0..m.positions.len()).min_by(|&a, &b| (m.positions[a][1] - (lo[1] + hi[1]) * 0.5).abs().total_cmp(&(m.positions[b][1] - (lo[1] + hi[1]) * 0.5).abs())).unwrap();
        assert!(rig.weights[mid][1] > 0.15, "the joint is shared: {:?}", rig.weights[mid]);
        m.rig = Some(std::sync::Arc::new(rig.clone()));
        let bent = rig.posed_mesh(&m, &PoseInput { rotations: HashMap::from([("seg.1".to_string(), [60.0, 0.0, 0.0])]), ..Default::default() });
        // The bottom swings forward (towards -z).
        let (blo, _) = bent.bounds();
        assert!(blo[2] < lo[2] - 40.0, "{blo:?}");
    }

    #[test]
    fn separate_pieces_each_follow_one_bone_whole() {
        // A body and a lid sitting on it, two pieces; a hinge bone along the lid's back edge.
        let mut body = crate::prim::cube([200.0, 100.0, 200.0]);
        let mut lid = crate::prim::cube([200.0, 30.0, 200.0]);
        crate::ops::transform(&mut lid, &crate::region::Region::All, [0.0, -70.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        crate::ops::merge(&mut body, &lid);
        let mut rig = Rig::default();
        rig.bones.push(Bone::new("body", None, [0.0, 0.0, 0.0], [0.0, 50.0, 0.0]));
        rig.bones.push(Bone::new("lid", Some(0), [0.0, -75.0, 100.0], [0.0, -75.0, -100.0]));
        let carried = bind_rigid(&body, &mut rig);
        assert_eq!(carried, vec![1, 1], "one piece per bone");
        assert!(rig.weights.iter().all(|w| w[0] == 1.0), "whole, not blended");
        body.rig = Some(std::sync::Arc::new(rig.clone()));
        let open = rig.posed_mesh(&body, &PoseInput { rotations: HashMap::from([("lid".to_string(), [60.0, 0.0, 0.0])]), ..Default::default() });
        let (lo, hi) = body.bounds();
        let (olo, _) = open.bounds();
        assert!(olo[1] < lo[1] - 1.0 || open.bounds().1[1] != hi[1], "the lid moved");
        let still = (0..body.positions.len()).filter(|&i| body.positions[i][1] > -52.0).all(|i| (open.positions[i][1] - body.positions[i][1]).abs() < 1e-3);
        assert!(still, "the body stayed where it was");
    }

    #[test]
    fn a_humanoid_fits_a_figure() {
        let mut body = crate::character::mannequin(400.0, &crate::character::Build::default()).0;
        body.rig = None;
        let rig = humanoid(&body, Arms::Auto);
        let hand = rig.bones[rig.bone("hand.L").unwrap()].head;
        let (lo, hi) = body.bounds();
        assert!(hand[0] > (lo[0] + hi[0]) * 0.5 + 60.0, "left hand on the +x side: {hand:?}");
        let foot = rig.bones[rig.bone("foot.R").unwrap()].head;
        assert!(foot[0] < 0.0 && foot[1] > hi[1] - 60.0, "{foot:?}");
        assert!(rig.constraints.iter().any(|c| c.name() == "foot.L"));
    }
}
