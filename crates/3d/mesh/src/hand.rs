// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hands with five digits, and the bones, grip points and finger curls that
//! go with them.
//!
//! One layout serves the sculpt, the armature and the poses, so they always
//! agree: a palm, four fingers and a thumb, each finger three segments
//! (`index.L.0`, `.1`, `.2`; the thumb's first segment is its metacarpal),
//! relaxed and slightly curled at rest.
//!
//! The canonical hand is the left one, built in its own frame: origin at the
//! wrist, `+y` along the hand to the fingertips, `+x` out of the palm (the
//! way the fingers curl), `+z` towards the thumb. [`Frame`] puts it in the
//! model: a figure with its palms towards its thighs has the palm normal
//! pointing inward and the thumb forward. The right hand is the mirror
//! image across the body's middle.
//!
//! Grips: each hand has a `grip.L` / `grip.R` bone (a control bone riding on
//! the hand) at the middle of the closed fist, its `y` along the fist's
//! axis. [`GripFrame`] says how a held thing sits in the hand.

use crate::rig::{Quat, Rig, q_from_axes};
use crate::sdf::{Part, Shape};
use crate::{Material, Mesh, V3, add, cross, dot, normalise, scale, sub};

/// The digits, thumb first.
pub const DIGITS: [&str; 5] = ["thumb", "index", "middle", "ring", "pinky"];

/// One finger or the thumb: its joints from the knuckle to the tip, and the
/// radius of each of its three segments.
#[derive(Clone, Debug, PartialEq)]
pub struct Digit {
    pub name: &'static str,
    pub joints: [V3; 4],
    pub radius: [f32; 3],
}

/// Where the hand sits in the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub wrist: V3,
    /// Unit vector along the hand, wrist to fingertips.
    pub along: V3,
    /// The palm's normal (the way the fingers curl), unit and square to `along`.
    pub palm: V3,
    /// Hand length, wrist to the tip of the middle finger (px).
    pub length: f32,
    /// A left hand (the canonical one) or a right (mirrored across `mid_x`).
    pub left: bool,
    pub mid_x: f32,
}

impl Frame {
    /// The frame of a hand whose arm leaves the shoulder along `along`
    /// (wrist to fingers) with the palm turned in towards the body.
    pub fn hanging(wrist: V3, along: V3, length: f32, left: bool, mid_x: f32) -> Frame {
        let along = normalise(along);
        let medial = [if left { -1.0 } else { 1.0 }, 0.0, 0.0];
        let palm = normalise(sub(medial, scale(along, dot(medial, along))));
        Frame { wrist, along, palm, length, left, mid_x }
    }

    /// The frame as the canonical left hand sees it: wrist and axes.
    fn canonical(&self) -> (V3, V3, V3, V3) {
        if self.left {
            let f = cross(self.palm, self.along);
            (self.wrist, self.palm, self.along, f)
        } else {
            let wrist = mirror_point(self.wrist, self.mid_x);
            let along = [-self.along[0], self.along[1], self.along[2]];
            let palm = [-self.palm[0], self.palm[1], self.palm[2]];
            (wrist, palm, along, cross(palm, along))
        }
    }

    /// A point of the canonical hand (x out of the palm, y along, z to the
    /// thumb), in the model.
    pub fn to_model(&self, p: V3) -> V3 {
        let (w, x, y, z) = self.canonical();
        let q = add(w, add(add(scale(x, p[0]), scale(y, p[1])), scale(z, p[2])));
        if self.left { q } else { mirror_point(q, self.mid_x) }
    }

    /// A direction of the canonical hand, in the model.
    pub fn dir_to_model(&self, d: V3) -> V3 {
        let (_, x, y, z) = self.canonical();
        let q = add(add(scale(x, d[0]), scale(y, d[1])), scale(z, d[2]));
        if self.left { q } else { [-q[0], q[1], q[2]] }
    }
}

fn mirror_point(p: V3, mid_x: f32) -> V3 {
    [2.0 * mid_x - p[0], p[1], p[2]]
}

/// The digits of the canonical hand, `l` long, curled by `curl` (0 relaxed
/// and open, 1 a closed fist).
pub fn digits(l: f32, curl: f32) -> Vec<Digit> {
    let palm = 0.46 * l;
    // Base z, spread (degrees), length, curl factor, radius, per finger.
    let fingers = [("index", 0.17, 6.0, 0.49, 1.0, 0.054), ("middle", 0.057, 1.0, 0.53, 1.1, 0.055), ("ring", -0.057, -4.0, 0.49, 1.2, 0.052), ("pinky", -0.165, -11.0, 0.39, 1.35, 0.046)];
    let share = [0.45, 0.30, 0.25];
    let mut out = Vec::new();
    let rest = 0.16 + 0.84 * curl.clamp(0.0, 1.0);
    // The thumb first.
    {
        let mut p = [0.02 * l, 0.14 * l, 0.17 * l];
        let mut d = normalise([0.28, 0.78, 0.56]);
        let lens = [0.21 * l, 0.16 * l, 0.13 * l];
        let mut joints = [p; 4];
        for k in 0..3 {
            p = add(p, scale(d, lens[k]));
            joints[k + 1] = p;
            // Bends in towards the palm and across it.
            d = normalise(add(d, scale([0.55, 0.0, -0.45], rest * (0.42 + 0.1 * k as f32))));
        }
        out.push(Digit { name: "thumb", joints, radius: [0.062 * l, 0.056 * l, 0.051 * l] });
    }
    for (name, z, spread, len, factor, r) in fingers {
        let base = [0.0, palm, z * l];
        let s = f32::to_radians(spread);
        let flex = [85.0 * rest * factor, 100.0 * rest * factor, 65.0 * rest * factor].map(f32::to_radians);
        let mut joints = [base; 4];
        let mut acc = 0.0;
        let mut p = base;
        for k in 0..3 {
            acc += flex[k] * if rest > 0.0 { 1.0 } else { 0.0 };
            let d = normalise([acc.sin(), acc.cos() * s.cos(), acc.cos() * s.sin()]);
            p = add(p, scale(d, len * l * share[k]));
            joints[k + 1] = p;
        }
        out.push(Digit { name, joints, radius: [r * l, r * 0.93 * l, r * 0.85 * l] });
    }
    out
}

/// The grip point of the canonical hand: the middle of the fist.
pub fn grip_point(l: f32) -> V3 {
    [0.075 * l, 0.40 * l, 0.02 * l]
}

/// The hand sculpted as separate pieces in the canonical frame: the palm (with
/// the wrist and the pads of the thumb and little finger) and, each its own
/// closed surface running a little way into the palm, the thumb and the four
/// fingers. Each digit is its own geometry on its own three bones, so it
/// bends and spreads alone. One material (index 0). `chunk` 1 is a natural
/// hand and more a stubby cartoon one; `cell` is the field's grid step (px).
pub fn mesh(l: f32, curl: f32, chunk: f32, cell: f32) -> Mesh {
    let ch = chunk.clamp(0.7, 2.0);
    let cap = |a: V3, b: V3, r: f32, blend: f32| Part { a, b, blend, ..Part::new(Shape::Capsule, [0.0; 3], [r * 2.0; 3]) };
    let ell = |at: V3, size: V3, blend: f32| Part { blend, ..Part::new(Shape::Ellipsoid, at, size) };
    let material = || vec![Material { name: "Skin".into(), roughness: 0.55, subsurface: 0.6, ..Material::default() }];
    let palm = [
        ell([0.0, 0.03 * l, 0.0], [0.17 * l * ch, 0.26 * l, 0.29 * l * ch.sqrt()], 0.0),
        ell([0.0, 0.25 * l, 0.0], [0.2 * l * ch, 0.52 * l, 0.47 * l * ch.sqrt()], 0.06 * l),
        ell([0.035 * l, 0.22 * l, 0.15 * l], [0.17 * l, 0.28 * l, 0.17 * l], 0.05 * l),
        ell([0.0, 0.30 * l, -0.14 * l], [0.15 * l, 0.28 * l, 0.14 * l], 0.04 * l),
    ];
    let mut out = crate::sdf::build(&palm, material(), cell);
    for d in digits(l, curl) {
        let mut parts = Vec::new();
        // The knuckle end reaches back into the palm so no gap shows.
        let back = {
            let dir = normalise(sub(d.joints[1], d.joints[0]));
            sub(d.joints[0], scale(dir, 0.06 * l))
        };
        parts.push(cap(back, d.joints[1], d.radius[0] * ch.sqrt(), 0.0));
        for k in 1..3 {
            parts.push(cap(d.joints[k], d.joints[k + 1], d.radius[k] * ch.sqrt(), 0.01 * l));
        }
        let t = d.joints[3];
        parts.push(Part { blend: 0.008 * l, ..Part::new(Shape::Ellipsoid, t, [d.radius[2] * 2.1 * ch.sqrt(); 3]) });
        let finger = crate::sdf::build(&parts, material(), (cell * 0.8).max(0.05));
        let mut mats = out.tri_materials();
        mats.extend(std::iter::repeat_n(0u32, finger.triangles()));
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&finger.positions);
        out.normals.extend_from_slice(&finger.normals);
        out.uvs.extend_from_slice(&finger.uvs);
        out.indices.extend(finger.indices.iter().map(|i| i + base));
        out.regroup(&mats);
    }
    out
}

/// The mesh of a hand placed in the model by `frame`.
pub fn place(m: &Mesh, frame: &Frame) -> Mesh {
    let mut out = m.clone();
    for p in &mut out.positions {
        *p = frame.to_model(*p);
    }
    for n in &mut out.normals {
        *n = normalise(frame.dir_to_model(*n));
    }
    if !frame.left {
        for t in out.indices.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
    out
}

/// Bone names for one hand's digits: `thumb.L.0`…`pinky.L.2`.
pub fn bone_name(digit: &str, left: bool, segment: usize) -> String {
    format!("{digit}.{}.{segment}", if left { "L" } else { "R" })
}

/// How a held thing sits in the hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// The fist closes round a handle or body that runs across it: a cup,
    /// a bottle, a lantern, a tool, a bag handle, a phone, the grip of a
    /// pistol. The thing's long (y) axis runs across the fist and its front
    /// (-z) points the way the hand does.
    Handle,
    /// Thumb and fingers pinch a slim thing held like a pencil: a pen, a
    /// cigarette, a match, a wand. Its tip (-y) points out from the hand.
    Pinch,
    /// The thing rests on the open palm: a ball, a tray, a bowl, a coin.
    Palm,
}

impl Grip {
    pub fn parse(s: &str) -> Option<Grip> {
        Some(match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "handle" | "fist" | "grasp" | "grab" | "hold" | "wrap" | "power" | "" => Grip::Handle,
            "pinch" | "pen" | "pencil" | "precision" | "fingers" | "tip" | "cigarette" => Grip::Pinch,
            "palm" | "open" | "rest" | "cradle" | "tray" | "flat" => Grip::Palm,
            _ => return None,
        })
    }

    pub const NAMES: &'static str = "handle, pinch or palm";

    /// Finger curls (thumb, index, middle, ring, pinky; 0 open, 1 fist) for a
    /// thing `thickness` px across held in a hand `length` px long.
    pub fn curls(self, thickness: f32, length: f32) -> [f32; 5] {
        let t = (thickness / length.max(1.0)).clamp(0.0, 1.0);
        match self {
            Grip::Handle => {
                let c = (0.98 - 1.7 * t).clamp(0.22, 0.95);
                [(c * 0.8).max(0.2), c, c, (c + 0.04).min(1.0), (c + 0.08).min(1.0)]
            }
            Grip::Pinch => [0.45, 0.5, 0.85, 0.9, 0.9],
            Grip::Palm => [0.18, 0.22, 0.2, 0.2, 0.22],
        }
    }

    /// The turn taking a thing modelled upright (y down, front -z) to its
    /// place in the hand, as a rotation in the model's frame, given the
    /// hand's axes: `along` (wrist to fingers), `palm` (the fingers' curl)
    /// and `thumb` (across the fist towards the thumb).
    pub fn orientation(self, along: V3, palm: V3, thumb: V3) -> Quat {
        let (y, z) = match self {
            // Long axis across the fist (top towards the thumb), front along the hand.
            Grip::Handle => (scale(thumb, -1.0), scale(along, -1.0)),
            // Tip (-y) out along the hand, tilted to the thumb side and away from the palm.
            Grip::Pinch => (scale(normalise(add(add(scale(along, 1.0), scale(thumb, 0.35)), scale(palm, -0.3))), -1.0), scale(palm, -1.0)),
            // Top (-y) out of the palm, front along the hand.
            Grip::Palm => (scale(palm, -1.0), scale(along, -1.0)),
        };
        let y = normalise(y);
        let z = normalise(sub(z, scale(y, dot(z, y))));
        let x = cross(y, z);
        q_from_axes(x, y, z)
    }
}

/// Bone rotations (degrees `[x, y, z]`, by bone name) that curl one hand's
/// digits by `curls` (thumb, index, middle, ring, pinky; 0 open, 1 fist).
/// The axis and sign are found from the rig itself, so any hand built from
/// these bones curls the right way whatever its side or orientation.
pub fn curl_pose(rig: &Rig, left: bool, curls: [f32; 5]) -> Vec<(String, V3)> {
    let side = if left { "L" } else { "R" };
    let max = [[35.0f32, 50.0, 45.0], [85.0, 100.0, 65.0], [88.0, 102.0, 66.0], [90.0, 100.0, 62.0], [92.0, 98.0, 60.0]];
    let mut out = Vec::new();
    for (d, name) in DIGITS.iter().enumerate() {
        for seg in 0..3 {
            let Some(b) = rig.bone(&format!("{name}.{side}.{seg}")) else { continue };
            let bone = &rig.bones[b];
            // The rotation about the bone's own z that moves its tip towards
            // the palm side (the direction the next segment's curl goes).
            let sign = curl_sign(rig, b);
            out.push((bone.name.clone(), [0.0, 0.0, sign * max[d][seg] * curls[d].clamp(0.0, 1.0)]));
        }
    }
    out
}

/// +1 or -1: which way a turn about the bone's z swings its tip towards the
/// palm side of the hand (the hand bone's local x-ish normal at rest).
fn curl_sign(rig: &Rig, b: usize) -> f32 {
    let bone = &rig.bones[b];
    let frame = bone.rest_frame();
    let tip = sub(bone.tail, bone.head);
    // Rotating by +z in the bone's frame moves the tip along the frame's
    // -x axis (see the module notes of `rig`): the palm side is towards the
    // hand's palm normal, found from the hand bone's side of the body.
    let x_axis = frame.apply_dir([1.0, 0.0, 0.0]);
    let toward_palm = palm_direction(rig, b);
    let _ = tip;
    if dot(x_axis, toward_palm) > 0.0 { -1.0 } else { 1.0 }
}

/// The direction the fingers curl for the digit bone `b`: from the back of
/// the hand to the palm, taken as pointing towards the body's middle (palms
/// hang inward at rest).
fn palm_direction(rig: &Rig, b: usize) -> V3 {
    let bone = &rig.bones[b];
    let along = normalise(sub(bone.tail, bone.head));
    let mid_x = rig.bone("hips").map_or(0.0, |h| rig.bones[h].head[0]);
    let medial = [if bone.head[0] >= mid_x { -1.0 } else { 1.0 }, 0.0, 0.0];
    normalise(sub(medial, scale(along, dot(medial, along))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_digits_with_three_segments_curled_a_little() {
        let d = digits(40.0, 0.0);
        assert_eq!(d.len(), 5);
        let names: Vec<_> = d.iter().map(|x| x.name).collect();
        assert_eq!(names, DIGITS);
        // The middle finger's tip is at the hand length.
        let mid = &d[2];
        assert!((mid.joints[3][1] - 40.0).abs() < 4.0 || mid.joints[3][1] > 33.0, "{:?}", mid.joints);
        // A fist tucks the tips back towards the palm.
        let fist = digits(40.0, 1.0);
        assert!(fist[2].joints[3][1] < d[2].joints[3][1] - 15.0);
        assert!(fist[2].joints[1][0] > d[2].joints[1][0] + 5.0, "curls to +x, the palm side");
    }

    #[test]
    fn the_sculpted_hand_has_separate_fingers() {
        let m = mesh(40.0, 0.0, 1.0, 0.6);
        // Six closed pieces: the palm and five digits.
        let (ids, unique) = m.position_ids();
        let mut parent: Vec<u32> = (0..unique.len() as u32).collect();
        fn root(p: &mut [u32], mut i: u32) -> u32 {
            while p[i as usize] != i {
                p[i as usize] = p[p[i as usize] as usize];
                i = p[i as usize];
            }
            i
        }
        for t in 0..m.triangles() {
            let c = m.tri(t).map(|v| ids[v]);
            for (a, b) in [(c[0], c[1]), (c[1], c[2])] {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if ra != rb {
                    parent[ra as usize] = rb;
                }
            }
        }
        let pieces: std::collections::HashSet<u32> = (0..unique.len() as u32).map(|i| root(&mut parent, i)).collect();
        assert_eq!(pieces.len(), 6, "palm and five separate digits");
        assert!(m.triangles() > 2000);
        // Slice across the fingers near their tips: several separate runs of material.
        let y = 40.0 * 0.78;
        let mut zs: Vec<f32> = m.positions.iter().filter(|p| (p[1] - y).abs() < 0.8 && p[0].abs() < 8.0).map(|p| p[2]).collect();
        zs.sort_by(|a, b| a.total_cmp(b));
        let mut runs = 1;
        for w in zs.windows(2) {
            if w[1] - w[0] > 0.9 {
                runs += 1;
            }
        }
        assert!(runs >= 4, "fingers read as separate near the tips: {runs} runs");
    }

    #[test]
    fn the_right_hand_is_the_left_mirrored() {
        let l = Frame::hanging([100.0, 0.0, 0.0], [0.77, 0.64, 0.0], 40.0, true, 0.0);
        let r = Frame::hanging([-100.0, 0.0, 0.0], [-0.77, 0.64, 0.0], 40.0, false, 0.0);
        let a = l.to_model([3.0, 20.0, 5.0]);
        let b = r.to_model([3.0, 20.0, 5.0]);
        assert!((a[0] + b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3 && (a[2] - b[2]).abs() < 1e-3);
        // The thumb is forward (-z) on both.
        assert!(l.dir_to_model([0.0, 0.0, 1.0])[2] < -0.9 && r.dir_to_model([0.0, 0.0, 1.0])[2] < -0.9);
        // The palm faces the body.
        assert!(l.palm[0] < 0.0 && r.palm[0] > 0.0);
    }
}
