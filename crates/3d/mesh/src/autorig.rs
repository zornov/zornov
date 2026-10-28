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
//!   animal standing on all fours along any line on the ground (its long
//!   axis found from the shape, the head at the end that has one).
//! - [`bind`]: skin weights by distance through the surface (geodesic,
//!   so an arm does not pull the side of the chest it rests against),
//!   nearer bones weighing more, blended wider where bones lie deep;
//!   then smoothed over the surface so joints bend softly, and each
//!   vertex keeps its four strongest bones. [`audit_weights`] counts the
//!   faults a bad skin shows (far, cross-side and unjoined influences).

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
    centre_joints(mesh, &mut r, h, [1.0, 0.0, 0.0]);
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
/// the vertices in a thin slice across the bone at that joint. Bones on
/// the middle line (no side in their name) keep their place along
/// `lateral`, the axis across the body.
fn centre_joints(mesh: &Mesh, r: &mut Rig, h: f32, lateral: V3) {
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
            // Kept where the template put it across the body (`lateral`):
            // x for a figure facing -z, the animal's left for one on all
            // fours.
            head = add(head, scale(lateral, dot(sub(r.bones[i].head, head), lateral)));
            tail = add(tail, scale(lateral, dot(sub(r.bones[i].tail, tail), lateral)));
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
    centre_joints(mesh, &mut r, h * 0.5, [1.0, 0.0, 0.0]);
    r
}

/// How a four-legged body lies on the ground: its long axis (the line
/// through it with the most spread, found from the surface's second
/// moments in the ground plane), which end its head is at, and its
/// extent along, across and up. Y is down in Studio, so up is -y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyFrame {
    /// Unit, in the ground plane, from the tail end to the head end.
    pub fwd: V3,
    /// Unit, in the ground plane: the animal's left (up × forward).
    pub left: V3,
    /// Along `fwd`: the head end's coordinate, and the length.
    pub head_at: f32,
    pub length: f32,
    /// Along `left`: the middle and the width.
    pub mid: f32,
    pub width: f32,
    /// Top and bottom (y, down).
    pub top: f32,
    pub bottom: f32,
    /// The head end was told, or found from the shape (the end with more
    /// surface high up: a head is bulky and held up, a tail thin); false
    /// when the ends look alike and the head went to the default end
    /// (the low end of the long axis: -x for a body along x, -z for one
    /// along z).
    pub head_found: bool,
}

impl BodyFrame {
    /// A point `f` of the length back from the head end (0 head, 1 tail),
    /// `s` px to the animal's left of its middle, at height `y`.
    pub fn at(&self, f: f32, s: f32, y: f32) -> V3 {
        let a = self.head_at - f * self.length;
        let l = self.mid + s;
        [self.fwd[0] * a + self.left[0] * l, y, self.fwd[2] * a + self.left[2] * l]
    }
}

/// The frame of a body standing on all fours: see [`BodyFrame`]. With
/// `forward` given (any direction; only its ground-plane part counts) the
/// head goes that way and the long axis is not looked for.
pub fn body_frame(mesh: &Mesh, forward: Option<V3>) -> BodyFrame {
    let (lo, hi) = mesh.bounds();
    // Area-weighted second moments of the surface in x and z: vertex
    // counts would follow where a mesh happens to be dense (a UV sphere's
    // poles), area follows the shape.
    let mut acc = [0f64; 6]; // area, cx, cz, xx, xz, zz
    for t in 0..mesh.triangles() {
        let [a, b, c] = mesh.tri(t).map(|v| mesh.positions[v]);
        let area = 0.5 * length(crate::cross(sub(b, a), sub(c, a))) as f64;
        let m = scale(add(add(a, b), c), 1.0 / 3.0);
        let (x, z) = (m[0] as f64, m[2] as f64);
        acc[0] += area;
        acc[1] += area * x;
        acc[2] += area * z;
        acc[3] += area * x * x;
        acc[4] += area * x * z;
        acc[5] += area * z * z;
    }
    let long_axis = || -> V3 {
        let ext = [hi[0] - lo[0], hi[2] - lo[2]];
        let by_box: V3 = if ext[0] >= ext[1] { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
        if acc[0] <= 1e-9 {
            return by_box;
        }
        let w = acc[0];
        let (mx, mz) = (acc[1] / w, acc[2] / w);
        let (sxx, sxz, szz) = (acc[3] / w - mx * mx, acc[4] / w - mx * mz, acc[5] / w - mz * mz);
        // Eigenvalues of [[sxx, sxz], [sxz, szz]].
        let tr = sxx + szz;
        let det = sxx * szz - sxz * sxz;
        let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
        let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
        if l1 < 1.15 * l2.max(1e-12) {
            // Round in plan: no long axis worth trusting.
            return by_box;
        }
        let th = 0.5 * (2.0 * sxz).atan2(sxx - szz);
        let mut d = [th.cos() as f32, 0.0, th.sin() as f32];
        // A fixed sign: the long axis points to +x (or +z when it runs
        // along z), so the default head end is the low end.
        if d[0].abs() >= d[2].abs() {
            if d[0] < 0.0 {
                d = scale(d, -1.0)
            }
        } else if d[2] < 0.0 {
            d = scale(d, -1.0);
        }
        d
    };
    let given = forward.map(|f| [f[0], 0.0, f[2]]).filter(|f| length(*f) > 1e-6).map(normalise);
    let axis = given.unwrap_or_else(long_axis);
    let up: V3 = [0.0, -1.0, 0.0];
    let proj = |p: V3, d: V3| p[0] * d[0] + p[2] * d[2];
    let (mut a_lo, mut a_hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in &mesh.positions {
        let a = proj(*p, axis);
        a_lo = a_lo.min(a);
        a_hi = a_hi.max(a);
    }
    let len = (a_hi - a_lo).max(1.0);
    // Which end the head is at (when not told): the end with more surface
    // above the body's middle height within a fifth of the length.
    let (fwd, head_found) = match given {
        Some(g) => (g, true),
        None => {
            let mid_y = (lo[1] + hi[1]) * 0.5;
            let slice = 0.2 * len;
            let (mut at_lo, mut at_hi) = (0f32, 0f32);
            for t in 0..mesh.triangles() {
                let [a, b, c] = mesh.tri(t).map(|v| mesh.positions[v]);
                let m = scale(add(add(a, b), c), 1.0 / 3.0);
                if m[1] > mid_y {
                    continue;
                }
                let area = 0.5 * length(crate::cross(sub(b, a), sub(c, a)));
                let along = proj(m, axis);
                if along < a_lo + slice {
                    at_lo += area;
                } else if along > a_hi - slice {
                    at_hi += area;
                }
            }
            if at_hi > 1.15 * at_lo && at_hi > 0.0 {
                (axis, true)
            } else if at_lo > 1.15 * at_hi && at_lo > 0.0 {
                (scale(axis, -1.0), true)
            } else {
                (scale(axis, -1.0), false)
            }
        }
    };
    let left = normalise(crate::cross(up, fwd));
    let (mut a_lo, mut a_hi, mut s_lo, mut s_hi) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for p in &mesh.positions {
        let (a, s) = (proj(*p, fwd), proj(*p, left));
        a_lo = a_lo.min(a);
        a_hi = a_hi.max(a);
        s_lo = s_lo.min(s);
        s_hi = s_hi.max(s);
    }
    if !a_lo.is_finite() {
        return BodyFrame { fwd: [-1.0, 0.0, 0.0], left: [0.0, 0.0, -1.0], head_at: 0.0, length: 1.0, mid: 0.0, width: 1.0, top: 0.0, bottom: 1.0, head_found: false };
    }
    BodyFrame { fwd, left, head_at: a_hi, length: (a_hi - a_lo).max(1.0), mid: (s_lo + s_hi) * 0.5, width: (s_hi - s_lo).max(1.0), top: lo[1], bottom: hi[1], head_found }
}

/// An animal standing on all fours, along its own long axis (found from
/// the shape, see [`body_frame`]), head towards the end that has one
/// (or the low end of the axis when the two ends look alike: -x for a
/// body along x): spine, neck, head, jaw, tail and four legs
/// (`leg_front.L`… with `.0` upper and `.1` lower, `paw_front.L`…).
pub fn quadruped(mesh: &Mesh) -> Rig {
    quadruped_facing(mesh, None)
}

/// [`quadruped`] with the head towards `forward` (its ground-plane part)
/// when given.
pub fn quadruped_facing(mesh: &Mesh, forward: Option<V3>) -> Rig {
    let fr = body_frame(mesh, forward);
    let ht = (fr.bottom - fr.top).max(1.0);
    let y = |f: f32| fr.top + f * ht;
    let half_w = fr.width * 0.3;
    let at = |f: f32, s: f32, h: f32| fr.at(f, s, y(h));
    let mut r = Rig::default();
    let mut b = |name: &str, parent: Option<usize>, head: V3, tail: V3| -> usize {
        r.bones.push(Bone::new(name, parent, head, tail));
        r.bones.len() - 1
    };
    let hips = b("hips", None, at(0.72, 0.0, 0.35), at(0.55, 0.0, 0.33));
    let spine = b("spine", Some(hips), at(0.55, 0.0, 0.33), at(0.3, 0.0, 0.32));
    let neck = b("neck", Some(spine), at(0.3, 0.0, 0.32), at(0.16, 0.0, 0.15));
    let head = b("head", Some(neck), at(0.16, 0.0, 0.15), at(0.0, 0.0, 0.2));
    b("jaw", Some(head), at(0.12, 0.0, 0.22), at(0.02, 0.0, 0.27));
    let mut prev = hips;
    for i in 0..3 {
        let f0 = 0.72 + i as f32 * 0.093;
        prev = b(&format!("tail.{i}"), Some(prev), at(f0, 0.0, 0.33 + i as f32 * 0.02), at(f0 + 0.093, 0.0, 0.35 + i as f32 * 0.03));
    }
    for (end, fx, parent) in [("front", 0.3f32, spine), ("back", 0.7, hips)] {
        for (side, sz) in [("L", 1.0f32), ("R", -1.0)] {
            let s = sz * half_w;
            let u = b(&format!("leg_{end}.{side}.0"), Some(parent), at(fx, s, 0.4), at(fx + 0.01, s, 0.68));
            let l = b(&format!("leg_{end}.{side}.1"), Some(u), at(fx + 0.01, s, 0.68), at(fx, s, 0.95));
            b(&format!("paw_{end}.{side}"), Some(l), at(fx, s, 0.95), at(fx - 0.04, s, 1.0));
        }
    }
    centre_joints(mesh, &mut r, ht, fr.left);
    r
}

/// Binds the skin: works out every vertex's weights from the rig's
/// deforming bones by distance *through the solid model* (voxel geodesic,
/// [`crate::solid`]: the way from a bone to a vertex runs inside the
/// body), so an arm resting on the chest, a leg against the belly or the
/// two sides of a crotch do not pull each other, while a bone deep in a
/// torso still reaches all round it. A bone weighs
/// `exp(-((d - nearest) / σ)²)` with σ a fifth of a percent of the model's
/// size plus 12 % of the nearest bone's distance (so blends scale with
/// depth: wider in a torso, narrow between fingers, and two parallel limbs
/// do not share the skin between them). A vertex keeps only bones within
/// three joints of each other in the armature (strongest first), so the
/// skin between two parts that move apart is not dragged by both; the
/// weights are then smoothed over the surface and each vertex keeps its
/// four strongest bones. Pieces of the model not joined to any bone's surface
/// go to the nearest bone in a straight line. Leaves a rig with no bones
/// alone. [`bind_by_sight`] is the older way (straight lines that the body
/// does not block), kept to compare against.
pub fn bind(mesh: &Mesh, rig: &mut Rig) {
    let deform: Vec<usize> = (0..rig.bones.len()).filter(|&i| rig.bones[i].deform).collect();
    if deform.is_empty() || mesh.positions.is_empty() {
        rig.joints.clear();
        rig.weights.clear();
        return;
    }
    use studio_rig::weights as sw;
    let (ids, unique) = mesh.position_ids();
    let pos: Vec<[f64; 3]> = unique.iter().map(|p| p.map(f64::from)).collect();
    let tris: Vec<[u32; 3]> = (0..mesh.triangles()).map(|t| mesh.tri(t).map(|v| ids[v])).collect();
    let graph = sw::Graph::from_triangles(&pos, &tris);
    let (lo, hi) = mesh.bounds();
    let size = length(sub(hi, lo)).max(1.0);
    // Distances through the solid model, bone by bone in parallel.
    let solid = crate::solid::Solid::of(mesh, 80);
    let reach = size * 0.6;
    let fields: Vec<Vec<f64>> = {
        let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16).min(deform.len());
        let next = std::sync::atomic::AtomicUsize::new(0);
        let mut out: Vec<Vec<f64>> = vec![Vec::new(); deform.len()];
        let slots: Vec<std::sync::Mutex<&mut Vec<f64>>> = out.iter_mut().map(std::sync::Mutex::new).collect();
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if k >= deform.len() {
                            break;
                        }
                        let b = &rig.bones[deform[k]];
                        let d = solid.distances_from_segment(b.head, b.tail, reach);
                        let f: Vec<f64> = unique
                            .iter()
                            .map(|&p| {
                                let x = solid.read(&d, p);
                                // Cut off from this bone: far, by the straight line.
                                if x.is_finite() { f64::from(x) } else { f64::from(length(sub(p, closest_on_segment(b.head, b.tail, p)))) * 4.0 + 1e6 }
                            })
                            .collect();
                        **slots[k].lock().unwrap_or_else(|e| e.into_inner()) = f;
                    }
                });
            }
        });
        out
    };
    let mut w = sw::from_distances(&fields, f64::from(size) * 0.002, 0.12);
    // A vertex rides on bones near each other in the armature: walking
    // from the strongest down, a bone more than three joints from one
    // already kept is dropped (it would drag the skin between two parts
    // that do not move together: a flank between a front and a back leg).
    {
        let hops = joint_hops(rig);
        for i in 0..w.nodes() {
            let r = w.row_mut(i);
            let mut order: Vec<usize> = (0..r.len()).filter(|&b| r[b] > 0.0).collect();
            order.sort_by(|&a, &b| r[b].total_cmp(&r[a]));
            let mut kept: Vec<usize> = Vec::new();
            for b in order {
                if kept.iter().all(|&k| hops(deform[k], deform[b]) <= 3) {
                    kept.push(b);
                } else {
                    r[b] = 0.0;
                }
            }
        }
    }
    w.normalise();
    sw::smooth(&graph, &mut w, 6, 0.5, &[]);
    w.limit(4, 0.01);
    rig.joints = Vec::with_capacity(mesh.positions.len());
    rig.weights = Vec::with_capacity(mesh.positions.len());
    for &id in &ids {
        let (j, x) = w.top4(id as usize);
        rig.joints.push(j.map(|k| deform[(k as usize).min(deform.len() - 1)] as u16));
        rig.weights.push(x);
    }
}

/// Joints between two bones of `rig` along the armature (`usize::MAX`
/// for bones in separate trees).
fn joint_hops(rig: &Rig) -> impl Fn(usize, usize) -> usize + '_ {
    let depth: Vec<usize> = (0..rig.bones.len()).map(|i| std::iter::successors(rig.bones[i].parent, |&p| rig.bones[p].parent).count()).collect();
    move |a: usize, b: usize| {
        let (mut x, mut y, mut steps) = (a, b, 0);
        while x != y {
            let up = if depth[x] >= depth[y] { &mut x } else { &mut y };
            match rig.bones[*up].parent {
                Some(p) => *up = p,
                None => return usize::MAX,
            }
            steps += 1;
        }
        steps
    }
}

/// What is wrong with a skin's weights, counted over the model's welded
/// vertices (a vertex counts once per kind of fault).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WeightAudit {
    pub vertices: usize,
    /// Pulled (weight over 5 %, in a blend of two bones or more) by a bone
    /// much further than its nearest: more than a tenth of the rig's size
    /// beyond it. A vertex wholly on one bone (a part bound rigid) is not
    /// counted.
    pub far: usize,
    /// Pulled (over 5 %) by a left bone while on the right of the middle,
    /// or the other way round (the middle found from the rig's paired
    /// bones; vertices within a quarter of the gap between twin bones of
    /// it are let off).
    pub cross_side: usize,
    /// Carried (over 10 % each) by two bones more than three joints apart
    /// in the armature (a flank vertex on a front and a back leg; the base
    /// of the neck on the head and a shoulder is allowed, and so is a
    /// left and a right bone sharing the middle line, a crotch's seam).
    pub non_adjacent: usize,
    /// Edges of the surface across which the weights jump (the two ends'
    /// weights differ by more than 0.7 summed over the bones): where the
    /// skin tears open when the joint bends.
    pub tears: usize,
    /// The bone pairs behind the most faults, worst first (at most five):
    /// the bone that pulls and the bone that should have the vertex (or
    /// the two unjoined bones), and how many vertices.
    pub worst: Vec<(String, String, usize)>,
}

impl WeightAudit {
    pub fn total(&self) -> usize {
        self.far + self.cross_side + self.non_adjacent + self.tears
    }

    /// In words, for a check note; `None` when clean.
    pub fn describe(&self) -> Option<String> {
        if self.total() == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if self.far > 0 {
            parts.push(format!("{} pulled by a far bone", self.far));
        }
        if self.cross_side > 0 {
            parts.push(format!("{} pulled by a bone of the other side", self.cross_side));
        }
        if self.non_adjacent > 0 {
            parts.push(format!("{} shared by bones not joined to each other", self.non_adjacent));
        }
        if self.tears > 0 {
            parts.push(format!("{} edges where the weights jump (the skin tears when it bends)", self.tears));
        }
        let worst: Vec<String> = self.worst.iter().take(3).map(|(a, b, n)| format!("{a} with {b} ({n})")).collect();
        Some(format!("{} faults over {} vertices: {}{}", self.total(), self.vertices, parts.join(", "), if worst.is_empty() { String::new() } else { format!("; worst: {}", worst.join(", ")) }))
    }
}

/// Audits `mesh`'s skin under `rig`: see [`WeightAudit`]. A rig without
/// weights audits clean.
pub fn audit_weights(mesh: &Mesh, rig: &Rig) -> WeightAudit {
    let nb = rig.bones.len();
    if nb == 0 || rig.weights.len() != mesh.positions.len() || rig.joints.len() != mesh.positions.len() {
        return WeightAudit::default();
    }
    let (ids, unique) = mesh.position_ids();
    // The rig's size, and its middle and across from paired bones.
    let (mut blo, mut bhi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for b in &rig.bones {
        for p in [b.head, b.tail] {
            for i in 0..3 {
                blo[i] = blo[i].min(p[i]);
                bhi[i] = bhi[i].max(p[i]);
            }
        }
    }
    let size = length(sub(bhi, blo)).max(1.0);
    use studio_rig::names::{Side, side};
    let sides: Vec<Option<Side>> = rig.bones.iter().map(|b| side(&b.name).filter(|s| matches!(s, Side::Left | Side::Right))).collect();
    let (mut across, mut mid, mut gap, mut pairs) = ([0f32; 3], [0f32; 3], 0f32, 0f32);
    for (i, b) in rig.bones.iter().enumerate() {
        if sides[i] != Some(Side::Left) {
            continue;
        }
        if let Some(twin) = studio_rig::names::mirror(&b.name).and_then(|n| rig.bone(&n)) {
            let d = sub(b.head, rig.bones[twin].head);
            across = add(across, d);
            mid = add(mid, scale(add(b.head, rig.bones[twin].head), 0.5));
            gap += length(d);
            pairs += 1.0;
        }
    }
    let lateral = (pairs > 0.0 && length(across) > 1e-6).then(|| (normalise(across), scale(mid, 1.0 / pairs), gap / pairs));
    let apart = joint_hops(rig);
    let deform: Vec<usize> = (0..nb).filter(|&i| rig.bones[i].deform).collect();
    let mut first: Vec<Option<usize>> = vec![None; unique.len()];
    for (v, &id) in ids.iter().enumerate() {
        first[id as usize].get_or_insert(v);
    }
    let mut out = WeightAudit { vertices: unique.len(), ..WeightAudit::default() };
    let mut pairs: std::collections::HashMap<(usize, usize), usize> = std::collections::HashMap::new();
    for (k, v) in first.iter().enumerate() {
        let Some(v) = *v else { continue };
        let p = unique[k];
        let infl: Vec<(usize, f32)> = (0..4).filter(|&j| rig.weights[v][j] > 0.0 && (rig.joints[v][j] as usize) < nb).map(|j| (rig.joints[v][j] as usize, rig.weights[v][j])).collect();
        let dist = |b: usize| length(sub(p, closest_on_segment(rig.bones[b].head, rig.bones[b].tail, p)));
        let (nearest, near) = deform.iter().map(|&b| (b, dist(b))).fold((0, f32::INFINITY), |a, x| if x.1 < a.1 { x } else { a });
        // A vertex wholly on one bone was put there on purpose (a part bound
        // rigid: a lid on its hinge); far counts in blends only.
        let blended = infl.iter().filter(|e| e.1 > 0.05).count() >= 2;
        if let Some(&(b, _)) = infl.iter().find(|&&(b, w)| blended && w > 0.05 && dist(b) > near + 0.1 * size) {
            out.far += 1;
            *pairs.entry((b, nearest)).or_default() += 1;
        }
        if let Some((ax, m, gap)) = lateral {
            let s = dot(sub(p, m), ax);
            let margin = 0.25 * gap;
            if let Some(&(b, _)) = infl.iter().find(|&&(b, w)| {
                w > 0.05
                    && match sides[b] {
                        Some(Side::Left) => s < -margin,
                        Some(Side::Right) => s > margin,
                        _ => false,
                    }
            }) {
                out.cross_side += 1;
                *pairs.entry((b, nearest)).or_default() += 1;
            }
        }
        let strong: Vec<usize> = infl.iter().filter(|e| e.1 > 0.1).map(|e| e.0).collect();
        // On the middle line (as the cross-side check lets it off) a left
        // and a right bone may share a vertex: the seam of a crotch.
        let on_middle = lateral.is_some_and(|(ax, m, gap)| dot(sub(p, m), ax).abs() <= 0.25 * gap);
        let twins = |a: usize, b: usize| matches!((sides[a], sides[b]), (Some(Side::Left), Some(Side::Right)) | (Some(Side::Right), Some(Side::Left)));
        if let Some((a, b)) = strong.iter().enumerate().find_map(|(i, &a)| strong[i + 1..].iter().find(|&&b| apart(a, b) > 3 && !(on_middle && twins(a, b))).map(|&b| (a.min(b), a.max(b)))) {
            out.non_adjacent += 1;
            *pairs.entry((a, b)).or_default() += 1;
        }
    }
    // Jumps across edges.
    let row = |v: usize| -> Vec<(usize, f32)> { (0..4).filter(|&j| rig.weights[v][j] > 0.0).map(|j| (rig.joints[v][j] as usize, rig.weights[v][j])).collect() };
    let mut seen = std::collections::HashSet::new();
    for t in 0..mesh.triangles() {
        let c = mesh.tri(t);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            let (ia, ib) = (ids[a], ids[b]);
            if ia == ib || !seen.insert((ia.min(ib), ia.max(ib))) {
                continue;
            }
            let (ra, rb) = (row(a), row(b));
            let mut diff = 0.0;
            for &(bone, w) in &ra {
                diff += (w - rb.iter().find(|e| e.0 == bone).map_or(0.0, |e| e.1)).abs();
            }
            for &(bone, w) in &rb {
                if !ra.iter().any(|e| e.0 == bone) {
                    diff += w;
                }
            }
            if diff > 0.7 {
                out.tears += 1;
            }
        }
    }
    let mut worst: Vec<((usize, usize), usize)> = pairs.into_iter().collect();
    worst.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.worst = worst.into_iter().take(5).map(|((a, b), n)| (rig.bones[a].name.clone(), rig.bones[b].name.clone(), n)).collect();
    out
}

/// The older bind: each vertex goes to the bones it can see (a straight
/// line inside the body reaches them), nearer bones weighing more, then
/// smoothed over the surface. Kept to compare [`bind`] against; it lets a
/// limb pull a torso it can see across a gap, and cuts off sharply where
/// the line of sight is lost.
pub fn bind_by_sight(mesh: &Mesh, rig: &mut Rig) {
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

    /// A clay animal along z, head toward -z, y down: a torso, four legs
    /// melted into it, a neck and head and a tail, from blended shapes as
    /// the formula animals are.
    fn clay_animal() -> Mesh {
        use crate::sdf::{Part, Shape, build};
        let cap = |a: V3, b: V3, r: f32, blend: f32| Part { a, b, blend, ..Part::new(Shape::Capsule, [0.0; 3], [r * 2.0; 3]) };
        let mut parts = vec![Part::new(Shape::Ellipsoid, [0.0, 0.0, 0.0], [110.0, 90.0, 260.0])];
        for (x, z) in [(-32.0f32, -80.0f32), (32.0, -80.0), (-32.0, 80.0), (32.0, 80.0)] {
            parts.push(cap([x, 10.0, z], [x, 120.0, z], 16.0, 12.0));
        }
        parts.push(cap([0.0, -10.0, -110.0], [0.0, -60.0, -150.0], 22.0, 14.0));
        parts.push(Part { blend: 10.0, ..Part::new(Shape::Ellipsoid, [0.0, -70.0, -170.0], [60.0, 55.0, 75.0]) });
        parts.push(cap([0.0, -10.0, 125.0], [0.0, -40.0, 190.0], 8.0, 8.0));
        build(&parts, vec![], 4.0)
    }

    #[test]
    fn geodesic_weights_audit_cleaner_than_sight_on_a_clay_animal() {
        let m = clay_animal();
        let rig0 = quadruped(&m);
        let mut by_sight = rig0.clone();
        bind_by_sight(&m, &mut by_sight);
        let mut geo = rig0.clone();
        bind(&m, &mut geo);
        let (a, b) = (audit_weights(&m, &by_sight), audit_weights(&m, &geo));
        println!("worst by sight {:?}\nworst geodesic {:?}", a.worst, b.worst);
        println!("clay animal, {} vertices: by sight {a:?}; geodesic {b:?}", a.vertices);
        // The spine runs along z (the body's length) toward the head.
        let hips = &geo.bones[geo.bone("hips").unwrap()];
        let head = &geo.bones[geo.bone("head").unwrap()];
        assert!(head.tail[2] < hips.head[2] - 150.0, "head toward -z: {:?} {:?}", hips.head, head.tail);
        // Measured: sight far 160, cross 76, unjoined 80, tears 6 (322);
        // through the solid far 4, cross 1, unjoined 181, tears 60 (246).
        assert!(b.total() < a.total(), "geodesic {b:?} vs sight {a:?}");
        assert!(b.far * 10 < a.far && b.cross_side * 10 < a.cross_side, "geodesic {b:?} vs sight {a:?}");
        // Rows sum to one with at most four bones.
        assert!(geo.weights.iter().all(|w| (w.iter().sum::<f32>() - 1.0).abs() < 1e-3));
    }

    #[test]
    fn geodesic_weights_audit_cleaner_on_a_figure() {
        let (m, _) = crate::character::mannequin(400.0, &crate::character::Build::default());
        let rig0 = humanoid(&m, Arms::A);
        let mut by_sight = rig0.clone();
        bind_by_sight(&m, &mut by_sight);
        let mut geo = rig0;
        bind(&m, &mut geo);
        let (a, b) = (audit_weights(&m, &by_sight), audit_weights(&m, &geo));
        println!("worst by sight {:?}\nworst geodesic {:?}", a.worst, b.worst);
        println!("figure, {} vertices: by sight {a:?}; geodesic {b:?}", a.vertices);
        // Measured: sight tears 44 (arm and torso cut where the line of
        // sight is lost), through the solid none; neither has far,
        // cross-side or unjoined influences.
        assert!(b.total() < a.total() && b.tears == 0, "geodesic {b:?} vs sight {a:?}");
    }

    #[test]
    fn the_audit_catches_a_skin_pulled_across_the_body() {
        let m = clay_animal();
        let mut r = quadruped(&m);
        bind(&m, &mut r);
        let clean = audit_weights(&m, &r);
        // Every vertex half on the left front leg: the right side is pulled
        // across, the far end of the body by a far bone.
        let leg = r.bone("leg_front.L.0").unwrap() as u16;
        for (j, w) in r.joints.iter_mut().zip(r.weights.iter_mut()) {
            j[1] = leg;
            w[0] = 0.5;
            w[1] = 0.5;
            w[2] = 0.0;
            w[3] = 0.0;
        }
        let bad = audit_weights(&m, &r);
        println!("clean {clean:?}, pulled {bad:?}");
        assert!(bad.cross_side > clean.cross_side + bad.vertices / 5 && bad.far > clean.far + bad.vertices / 5, "{bad:?}");
        assert!(bad.describe().is_some_and(|d| d.contains("other side")));
    }
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
