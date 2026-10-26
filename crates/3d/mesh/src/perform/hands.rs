// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A library of hand poses for any hand with finger bones (found by name,
//! see [`super::body`]): relaxed, open, fist, point, cup, pinch, grip,
//! hold a small thing. A pose is how far each finger joint curls, the
//! thumb's turn across the palm and the spread of the fingers; poses
//! blend into one another and scale from open to full.
//!
//! Curling is done by direction, not by guessed axes: a joint turns toward
//! the palm side of the hand, whatever the bones' own frames are.

use super::body::{Body, Limb, Side};
use super::solve::Solver;
use crate::rig::Rig;
use crate::{V3, add, cross, dot, normalise, scale, sub};

/// Fingers in order: thumb, index, middle, ring, pinky; per joint (from
/// the knuckle out) degrees of curl.
#[derive(Clone, Debug, PartialEq)]
pub struct HandPose {
    pub name: &'static str,
    pub curl: [[f32; 3]; 5],
    /// Degrees the fingers fan apart (+) or close (-).
    pub spread: f32,
    /// How far the thumb comes across the palm, 0..1.
    pub oppose: f32,
    /// The wrist bends toward the palm (+) or the back (-), degrees.
    pub wrist: f32,
}

pub const NAMES: &[&str] = &["relaxed", "open", "fist", "point", "cup", "pinch", "grip", "hold_small"];

pub fn named(name: &str) -> Option<HandPose> {
    let n = name.trim().to_lowercase().replace([' ', '-'], "_");
    let curled = [85.0, 95.0, 70.0];
    Some(match n.as_str() {
        "relaxed" | "rest" | "neutral" | "loose" => HandPose { name: "relaxed", curl: [[10.0, 10.0, 6.0], [18.0, 22.0, 12.0], [24.0, 28.0, 16.0], [30.0, 32.0, 18.0], [34.0, 36.0, 20.0]], spread: 2.0, oppose: 0.15, wrist: 0.0 },
        "open" | "flat" | "spread" => HandPose { name: "open", curl: [[0.0; 3]; 5], spread: 10.0, oppose: 0.0, wrist: 0.0 },
        "fist" | "clench" => HandPose { name: "fist", curl: [[30.0, 45.0, 30.0], curled, curled, curled, curled], spread: -2.0, oppose: 0.8, wrist: 0.0 },
        "point" | "pointing" => HandPose { name: "point", curl: [[25.0, 30.0, 15.0], [0.0, 2.0, 0.0], curled, curled, curled], spread: -2.0, oppose: 0.5, wrist: 0.0 },
        "cup" | "cupped" => HandPose { name: "cup", curl: [[15.0, 15.0, 10.0], [38.0, 32.0, 18.0], [40.0, 34.0, 20.0], [42.0, 36.0, 20.0], [44.0, 38.0, 22.0]], spread: -4.0, oppose: 0.4, wrist: 8.0 },
        "pinch" | "pincer" => HandPose { name: "pinch", curl: [[30.0, 30.0, 22.0], [38.0, 38.0, 26.0], [48.0, 55.0, 38.0], [58.0, 62.0, 42.0], [64.0, 68.0, 46.0]], spread: -2.0, oppose: 0.95, wrist: 0.0 },
        "grip" | "grasp" | "hold" => HandPose { name: "grip", curl: [[30.0, 25.0, 20.0], [60.0, 70.0, 45.0], [64.0, 74.0, 48.0], [66.0, 76.0, 50.0], [68.0, 78.0, 52.0]], spread: 0.0, oppose: 0.6, wrist: 0.0 },
        "hold_small" | "hold_small_object" | "small_object" | "holding" => HandPose { name: "hold_small", curl: [[30.0, 20.0, 15.0], [45.0, 45.0, 30.0], [48.0, 48.0, 32.0], [50.0, 50.0, 34.0], [52.0, 52.0, 36.0]], spread: -1.0, oppose: 0.7, wrist: 0.0 },
        _ => return None,
    })
}

impl HandPose {
    /// This pose partway toward `other`.
    pub fn mix(&self, other: &HandPose, t: f32) -> HandPose {
        let l = |a: f32, b: f32| a + (b - a) * t;
        let mut curl = [[0.0; 3]; 5];
        for f in 0..5 {
            for j in 0..3 {
                curl[f][j] = l(self.curl[f][j], other.curl[f][j]);
            }
        }
        HandPose { name: other.name, curl, spread: l(self.spread, other.spread), oppose: l(self.oppose, other.oppose), wrist: l(self.wrist, other.wrist) }
    }

    /// From the open hand (0) to this pose (1).
    pub fn amount(&self, w: f32) -> HandPose {
        named("open").map_or_else(|| self.clone(), |o| HandPose { name: self.name, ..o.mix(self, w.clamp(0.0, 1.0)) })
    }
}

/// The directions of a hand, now.
pub struct Palm {
    /// The way the fingers point.
    pub along: V3,
    /// Out of the palm: fingers curl toward it.
    pub out: V3,
    /// Across the palm toward the pinky.
    pub pinky: V3,
}

pub fn palm(s: &mut Solver, body: &Body, arm: &Limb) -> Palm {
    let along = s.dir(arm.end);
    let side = arm.side.sign();
    let medial = scale(body.left(), -side);
    let mut out = sub(medial, scale(along, dot(medial, along)));
    if dot(out, out) < 0.09 {
        let down = scale(body.up, -1.0);
        out = sub(down, scale(along, dot(down, along)));
    }
    let out = normalise(out);
    // Thumb side: a thumb bone says; otherwise forward of a hanging hand.
    let mut toward_thumb = sub(body.fwd, scale(along, dot(body.fwd, along)));
    if let Some(t) = body.finger("thumb", arm.side).and_then(|t| t.joints.first().copied()) {
        let hand_head = s.head(arm.end);
        let d = sub(s.head(t), hand_head);
        let d = sub(d, scale(along, dot(d, along)));
        if dot(d, d) > 1e-6 {
            toward_thumb = d;
        }
    }
    let across = normalise(cross(along, out));
    let thumb_side = if dot(across, toward_thumb) >= 0.0 { across } else { scale(across, -1.0) };
    Palm { along, out, pinky: scale(thumb_side, -1.0) }
}

/// Curls a hand into `pose` (0..1 of the way from open). Returns the number
/// of finger joints turned (0 when the hand has no finger bones).
pub fn apply(s: &mut Solver, body: &Body, _rig: &Rig, side: Side, pose: &HandPose, w: f32) -> usize {
    let Some(arm) = body.arm(side) else { return 0 };
    let arm = arm.clone();
    let p = pose.amount(w);
    let pl = palm(s, body, &arm);
    let mut turned = 0;
    for (fi, name) in ["thumb", "index", "middle", "ring", "pinky"].iter().enumerate() {
        let Some(f) = body.finger(name, side) else { continue };
        let joints: Vec<usize> = if f.joints.len() > 3 { f.joints[f.joints.len() - 3..].to_vec() } else { f.joints.clone() };
        // Fingers fan about the palm's normal.
        if fi > 0 && let Some(&j0) = joints.first() {
            let off = (fi as f32 - 2.0) * 0.5;
            s.rotate(j0, pl.out, p.spread * off * if side == Side::L { -1.0 } else { 1.0 });
        }
        for (ji, &j) in joints.iter().enumerate() {
            let deg = p.curl[fi][ji.min(2)];
            let toward = if fi == 0 { normalise(add(scale(pl.out, 1.0 - 0.5 * p.oppose), scale(pl.pinky, 0.9 * p.oppose))) } else { pl.out };
            s.tilt_toward(j, toward, deg);
            turned += 1;
        }
    }
    // The wrist follows a little.
    if p.wrist.abs() > 0.01 {
        s.tilt_toward(arm.end, pl.out, p.wrist);
    }
    turned
}

#[cfg(test)]
mod tests {
    use super::super::body::Body;
    use super::*;
    use crate::rig::{Bone, Rig};

    /// A hand on a forearm with fingers named `<finger>.<n>.<side>`.
    pub fn handy_rig(style: usize) -> Rig {
        let mut r = Rig::default();
        let mut add_bone = |name: &str, parent: Option<usize>, head: V3, tail: V3| -> usize {
            r.bones.push(Bone::new(name, parent, head, tail));
            r.bones.len() - 1
        };
        let hips = add_bone("hips", None, [0.0, 0.0, 0.0], [0.0, -20.0, 0.0]);
        let chest = add_bone("chest", Some(hips), [0.0, -20.0, 0.0], [0.0, -45.0, 0.0]);
        let head = add_bone("head", Some(chest), [0.0, -45.0, 0.0], [0.0, -60.0, 0.0]);
        let _ = head;
        for (side, sx) in [("L", 1.0f32), ("R", -1.0)] {
            let ua = add_bone(&format!("upper_arm.{side}"), Some(chest), [sx * 10.0, -42.0, 0.0], [sx * 10.0, -22.0, 0.0]);
            let fa = add_bone(&format!("forearm.{side}"), Some(ua), [sx * 10.0, -22.0, 0.0], [sx * 10.0, -4.0, -0.5]);
            let hand = add_bone(&format!("hand.{side}"), Some(fa), [sx * 10.0, -4.0, -0.5], [sx * 10.0, 4.0, -0.5]);
            let th = add_bone(&format!("thigh.{side}"), Some(hips), [sx * 5.0, 0.0, 0.0], [sx * 5.0, 25.0, 0.0]);
            let sh = add_bone(&format!("shin.{side}"), Some(th), [sx * 5.0, 25.0, 0.0], [sx * 5.0, 50.0, 0.5]);
            let ft = add_bone(&format!("foot.{side}"), Some(sh), [sx * 5.0, 50.0, 0.5], [sx * 5.0, 52.0, -6.0]);
            add_bone(&format!("toe.{side}"), Some(ft), [sx * 5.0, 52.0, -6.0], [sx * 5.0, 53.0, -9.0]);
            for (fi, f) in ["thumb", "index", "middle", "ring", "pinky"].iter().enumerate() {
                let x = sx * (10.0 + (fi as f32 - 2.0) * 1.2);
                let z = if fi == 0 { -2.5 } else { 0.0 };
                let mut parent = hand;
                for j in 0..3 {
                    let y0 = 4.0 + j as f32 * 2.5;
                    let name = match style {
                        0 => format!("{f}.{}.{side}", j + 1),
                        1 => format!("{}_{}_{}", f.to_uppercase(), ["Proximal", "Intermediate", "Distal"][j], side),
                        _ => format!("{}{}{}", ["Left", "Right"][usize::from(side == "R")], f[..1].to_uppercase() + &f[1..], j + 1),
                    };
                    parent = add_bone(&name, Some(parent), [x, y0, z - 0.2 * j as f32], [x, y0 + 2.5, z - 0.2 * j as f32]);
                }
            }
        }
        r
    }

    #[test]
    fn fingers_are_found_by_name_in_any_spelling() {
        for style in 0..3 {
            let rig = handy_rig(style);
            let b = Body::of(&rig);
            assert_eq!(b.fingers.len(), 10, "style {style}: {:?}", b.fingers.iter().map(|f| f.name).collect::<Vec<_>>());
            assert!(b.fingers.iter().all(|f| f.joints.len() == 3));
        }
    }

    #[test]
    fn a_fist_closes_every_finger_and_an_open_hand_does_not() {
        let rig = handy_rig(0);
        let body = Body::of(&rig);
        let tip = |s: &mut Solver, side: Side, f: &str| {
            let j = *body.finger(f, side).unwrap().joints.last().unwrap();
            s.tail(j)
        };
        let wrist = |s: &mut Solver| s.head(body.arm(Side::L).unwrap().end);
        let mut open = Solver::new(&rig);
        let n = apply(&mut open, &body, &rig, Side::L, &named("open").unwrap(), 1.0);
        assert_eq!(n, 15);
        let mut fist = Solver::new(&rig);
        apply(&mut fist, &body, &rig, Side::L, &named("fist").unwrap(), 1.0);
        for f in ["index", "middle", "ring", "pinky"] {
            let (wo, wf) = (wrist(&mut open), wrist(&mut fist));
            let (a, b) = (crate::length(sub(tip(&mut open, Side::L, f), wo)), crate::length(sub(tip(&mut fist, Side::L, f), wf)));
            assert!(b < a * 0.55, "{f}: open {a:.1} → fist {b:.1}: the tip comes in toward the palm");
        }
        // Half way is half closed; the point keeps the index out.
        let mut half = Solver::new(&rig);
        apply(&mut half, &body, &rig, Side::L, &named("fist").unwrap(), 0.5);
        let mut point = Solver::new(&rig);
        apply(&mut point, &body, &rig, Side::L, &named("point").unwrap(), 1.0);
        let reach = |s: &mut Solver, f: &str| {
            let w = wrist(s);
            crate::length(sub(tip(s, Side::L, f), w))
        };
        let (ro, rh, rf) = (reach(&mut open, "middle"), reach(&mut half, "middle"), reach(&mut fist, "middle"));
        assert!(rf < rh && rh < ro, "{ro} {rh} {rf}");
        assert!(reach(&mut point, "index") > 0.9 * reach(&mut open, "index") && reach(&mut point, "middle") < 0.6 * ro);
        // A hand without finger bones turns none and says so.
        let bare = crate::character::character(300.0, &crate::character::Build { detail: 1, ..Default::default() });
        // The same rig with its digit bones renamed past recognition.
        let mut stripped = (*bare.rig.clone().unwrap()).clone();
        for (i, b) in stripped.bones.iter_mut().enumerate() {
            if ["thumb", "index", "middle", "ring", "pinky"].iter().any(|d| b.name.starts_with(d)) {
                b.name = format!("unnamed_{i}");
            }
        }
        let brig = std::sync::Arc::new(stripped);
        let bb = Body::of(&brig);
        let mut s = Solver::new(&brig);
        assert_eq!(apply(&mut s, &bb, &brig, Side::L, &named("fist").unwrap(), 1.0), 0);
    }
}
