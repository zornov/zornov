// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion made from rules rather than recorded: walk and run cycles, an
//! idle that breathes, a wave, a jump, a nod, a head shake, talking
//! gestures, a dance bounce. Each is an [`Action`] for a humanoid rig
//! (bones named as [`crate::autorig::humanoid`] names them), so it plays,
//! loops and blends like any other; bones a rig lacks are skipped.
//!
//! Angles follow the bone frames of [`crate::rig`]: x swings forward,
//! y twists, z swings sideways (mirrored on `.R` bones, so the same
//! number raises either arm away from the body when its sign is flipped).

use crate::rig::{Action, BonePose, Rig, q_from_euler};
use crate::V3;
use std::f32::consts::{PI, TAU};

/// The procedural motions [`make`] knows.
pub const NAMES: &[&str] = &["walk", "run", "idle", "wave", "jump", "nod", "shake_head", "talk", "dance", "sneak"];

/// Settings for a motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Seconds per cycle (0 = the motion's own).
    pub cycle: f32,
    /// How big the movements are, 0.2–2.
    pub energy: f32,
    /// Arms hang this many degrees below horizontal at rest in the rig
    /// (40 for Studio's characters, 0 for a T-pose, 90 arms down).
    pub rest_arm_droop: f32,
}

impl Default for Style {
    fn default() -> Style {
        Style { cycle: 0.0, energy: 1.0, rest_arm_droop: 40.0 }
    }
}

/// A bump: 1 at `centre` (radians on the cycle), falling off over `width`.
fn bump(phase: f32, centre: f32, width: f32) -> f32 {
    let d = (phase - centre + PI).rem_euclid(TAU) - PI;
    (-(d / width) * (d / width)).exp()
}

/// The standing height of a humanoid rig (crown to toes), px.
pub fn rig_height(rig: &Rig) -> f32 {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for b in &rig.bones {
        for p in [b.head, b.tail] {
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
    }
    if lo.is_finite() { (hi - lo).max(1.0) } else { 100.0 }
}

/// The arms' droop below horizontal in a humanoid rig, degrees.
pub fn arm_droop(rig: &Rig) -> f32 {
    rig.bone("upper_arm.L").map_or(40.0, |i| {
        let b = &rig.bones[i];
        let d = crate::sub(b.tail, b.head);
        d[1].atan2(d[0].abs().max(1e-6)).to_degrees()
    })
}

/// How far a walk or run carries the character in one cycle (px), for
/// moving the layer so the feet do not slide.
pub fn travel_per_cycle(rig: &Rig, name: &str, style: &Style) -> f32 {
    let h = rig_height(rig);
    let e = style.energy.clamp(0.2, 2.0);
    match name {
        "walk" => 0.72 * h * e.sqrt(),
        "run" => 1.35 * h * e.sqrt(),
        "sneak" => 0.45 * h * e.sqrt(),
        _ => 0.0,
    }
}

/// The motion's own cycle length, seconds.
pub fn default_cycle(name: &str) -> f32 {
    match name {
        "walk" => 1.1,
        "run" => 0.7,
        "idle" => 4.0,
        "wave" => 1.0,
        "jump" => 1.4,
        "nod" => 0.8,
        "shake_head" => 0.9,
        "talk" => 3.0,
        "dance" => 0.9,
        "sneak" => 1.6,
        _ => 1.0,
    }
}

/// Builds a procedural motion for `rig`.
pub fn make(rig: &Rig, name: &str, style: &Style) -> Result<Action, String> {
    if !NAMES.contains(&name) {
        return Err(format!("motion \"{name}\": {}", NAMES.join(", ")));
    }
    let cycle = if style.cycle > 0.0 { style.cycle } else { default_cycle(name) };
    let e = style.energy.clamp(0.2, 2.0);
    let h = rig_height(rig);
    // Arms come down from the rig's rest pose to hang at the sides.
    let droop = arm_droop(rig);
    let hang = 78.0 - droop;
    let fps = 30.0;
    let frames = (cycle * fps).round().max(2.0) as usize;
    let n = rig.bones.len();
    let mut out: Vec<Vec<BonePose>> = Vec::with_capacity(frames + 1);
    for f in 0..=frames {
        let ph = TAU * f as f32 / frames as f32;
        let mut rot: Vec<(&str, V3)> = Vec::new();
        let mut loc: Vec<(&str, V3)> = Vec::new();
        // Arms down at the sides (z sideways: − lowers the left, + the right).
        let arms_down = |rot: &mut Vec<(&str, V3)>, swing_l: f32, swing_r: f32, bend: f32| {
            rot.push(("upper_arm.L", [swing_l, 0.0, -hang]));
            rot.push(("upper_arm.R", [swing_r, 0.0, hang]));
            rot.push(("forearm.L", [bend + swing_l.max(0.0) * 0.6, 0.0, 0.0]));
            rot.push(("forearm.R", [bend + swing_r.max(0.0) * 0.6, 0.0, 0.0]));
        };
        match name {
            "walk" | "run" | "sneak" => {
                let (thigh, knee, arm, bend, bob, lean) = match name {
                    "walk" => (24.0, 52.0, 16.0, 12.0, 0.012, 2.0),
                    "run" => (38.0, 95.0, 30.0, 75.0, 0.03, 12.0),
                    _ => (20.0, 70.0, 10.0, 35.0, 0.008, 18.0),
                };
                let (thigh, knee, arm) = (thigh * e, knee * e.sqrt(), arm * e);
                for (side, off) in [("L", 0.0f32), ("R", PI)] {
                    let p = ph + off;
                    let swing = thigh * p.sin();
                    // Knee folds in early swing, a little at contact.
                    let k = 5.0 + knee * bump(p, -0.3 * PI, 0.55) + 8.0 * bump(p, 0.75 * PI, 0.4);
                    let foot = 14.0 * bump(p, 0.45 * PI, 0.5) - 18.0 * bump(p, -0.55 * PI, 0.4);
                    rot.push((if side == "L" { "thigh.L" } else { "thigh.R" }, [swing - if name == "sneak" { 15.0 } else { 0.0 }, 0.0, 0.0]));
                    rot.push((if side == "L" { "shin.L" } else { "shin.R" }, [-k, 0.0, 0.0]));
                    rot.push((if side == "L" { "foot.L" } else { "foot.R" }, [foot, 0.0, 0.0]));
                }
                let a = arm * ph.sin();
                arms_down(&mut rot, -a, a, bend);
                rot.push(("hips", [0.0, 5.0 * e * ph.sin(), 2.5 * e * (2.0 * ph).cos()]));
                rot.push(("spine", [lean * 0.5, -3.0 * e * ph.sin(), 0.0]));
                rot.push(("chest", [lean * 0.5, -4.0 * e * ph.sin(), 0.0]));
                rot.push(("neck", [-lean * 0.4, 1.5 * ph.sin(), 0.0]));
                rot.push(("head", [-lean * 0.4, 1.5 * ph.sin(), 0.0]));
                // The body is lowest as each foot lands (twice a cycle).
                let drop = if name == "sneak" { 0.06 } else { 0.0 };
                loc.push(("hips", [0.0, h * (bob * (2.0 * ph).cos() - bob - drop), 0.0]));
            }
            "idle" => {
                let breath = (ph).sin();
                let sway = (ph * 0.5 + 0.3).sin();
                arms_down(&mut rot, 2.0 * breath * e, 2.0 * breath * e, 8.0);
                rot.push(("chest", [1.2 * breath * e, 0.0, 0.0]));
                rot.push(("spine", [0.5 * breath * e, 0.0, 0.8 * sway * e]));
                rot.push(("hips", [0.0, 0.0, -1.0 * sway * e]));
                rot.push(("head", [1.5 * (ph * 2.0).sin() * e * 0.5, 3.0 * sway * e, 0.0]));
                rot.push(("shoulder.L", [0.0, 0.0, 1.0 * breath * e]));
                rot.push(("shoulder.R", [0.0, 0.0, -1.0 * breath * e]));
            }
            "wave" => {
                // The right hand up beside the head, waving from the elbow.
                rot.push(("upper_arm.L", [0.0, 0.0, -hang]));
                rot.push(("forearm.L", [8.0, 0.0, 0.0]));
                rot.push(("upper_arm.R", [10.0, 0.0, -(droop + 35.0)]));
                rot.push(("forearm.R", [25.0, 0.0, -(55.0 + 25.0 * e * ph.sin())]));
                rot.push(("hand.R", [0.0, 0.0, -10.0 * ph.sin()]));
                rot.push(("head", [0.0, -6.0, 3.0]));
                rot.push(("chest", [0.0, -4.0, 2.0]));
            }
            "jump" => {
                // Crouch, spring, hang, land, settle over one cycle.
                let t = f as f32 / frames as f32;
                let crouch = bump(t * TAU, 0.18 * TAU, 0.5) + bump(t * TAU, 0.8 * TAU, 0.45) * 0.8;
                let air = if (0.3..0.7).contains(&t) { ((t - 0.3) / 0.4 * PI).sin() } else { 0.0 };
                let tuck = air * 0.6;
                rot.push(("thigh.L", [55.0 * crouch * e + 35.0 * tuck, 0.0, 0.0]));
                rot.push(("thigh.R", [55.0 * crouch * e + 35.0 * tuck, 0.0, 0.0]));
                rot.push(("shin.L", [-95.0 * crouch * e - 60.0 * tuck, 0.0, 0.0]));
                rot.push(("shin.R", [-95.0 * crouch * e - 60.0 * tuck, 0.0, 0.0]));
                rot.push(("foot.L", [40.0 * crouch * e - 20.0 * air, 0.0, 0.0]));
                rot.push(("foot.R", [40.0 * crouch * e - 20.0 * air, 0.0, 0.0]));
                let reach = 120.0 * air;
                rot.push(("upper_arm.L", [-35.0 * crouch + reach * 0.5, 0.0, -hang + reach * 0.7]));
                rot.push(("upper_arm.R", [-35.0 * crouch + reach * 0.5, 0.0, hang - reach * 0.7]));
                rot.push(("spine", [20.0 * crouch, 0.0, 0.0]));
                rot.push(("chest", [10.0 * crouch - 5.0 * air, 0.0, 0.0]));
                rot.push(("head", [-15.0 * crouch, 0.0, 0.0]));
                loc.push(("hips", [0.0, h * (air * 0.28 * e - 0.12 * crouch * e), 0.0]));
            }
            "nod" => {
                arms_down(&mut rot, 0.0, 0.0, 8.0);
                rot.push(("head", [-14.0 * e * ph.sin().max(-0.3), 0.0, 0.0]));
                rot.push(("neck", [-5.0 * e * ph.sin(), 0.0, 0.0]));
            }
            "shake_head" => {
                arms_down(&mut rot, 0.0, 0.0, 8.0);
                rot.push(("head", [0.0, 25.0 * e * ph.sin(), 0.0]));
                rot.push(("neck", [0.0, 8.0 * e * ph.sin(), 0.0]));
            }
            "talk" => {
                // Easy hand gestures and head moves to go with speech.
                let g1 = (ph * 2.0).sin() * 0.5 + (ph * 3.0 + 1.0).sin() * 0.5;
                let g2 = (ph * 2.0 + 2.0).sin() * 0.5 + (ph * 5.0).sin() * 0.3;
                rot.push(("upper_arm.L", [20.0 + 10.0 * g1 * e, 0.0, -hang + 10.0]));
                rot.push(("upper_arm.R", [20.0 + 10.0 * g2 * e, 0.0, hang - 10.0]));
                rot.push(("forearm.L", [60.0 + 15.0 * g1 * e, 15.0 * g2, 0.0]));
                rot.push(("forearm.R", [60.0 + 15.0 * g2 * e, -15.0 * g1, 0.0]));
                rot.push(("head", [4.0 * (ph * 3.0).sin() * e, 8.0 * (ph * 1.5).sin() * e, 3.0 * (ph * 2.0).sin()]));
                rot.push(("chest", [1.5 * (ph * 2.0).sin(), 3.0 * g1 * e, 0.0]));
            }
            "dance" => {
                let b = (ph * 2.0).cos();
                rot.push(("thigh.L", [10.0 + 8.0 * b * e, 0.0, 0.0]));
                rot.push(("thigh.R", [10.0 + 8.0 * b * e, 0.0, 0.0]));
                rot.push(("shin.L", [-20.0 - 16.0 * b * e, 0.0, 0.0]));
                rot.push(("shin.R", [-20.0 - 16.0 * b * e, 0.0, 0.0]));
                rot.push(("foot.L", [10.0 + 8.0 * b * e, 0.0, 0.0]));
                rot.push(("foot.R", [10.0 + 8.0 * b * e, 0.0, 0.0]));
                rot.push(("hips", [0.0, 12.0 * ph.sin() * e, 6.0 * ph.sin() * e]));
                rot.push(("chest", [0.0, -10.0 * ph.sin() * e, -6.0 * ph.sin() * e]));
                rot.push(("upper_arm.L", [40.0 + 30.0 * ph.sin() * e, 0.0, -hang + 30.0]));
                rot.push(("upper_arm.R", [40.0 - 30.0 * ph.sin() * e, 0.0, hang - 30.0]));
                rot.push(("forearm.L", [80.0, 0.0, 0.0]));
                rot.push(("forearm.R", [80.0, 0.0, 0.0]));
                rot.push(("head", [6.0 * (ph * 2.0).sin() * e, 0.0, 4.0 * ph.sin()]));
                loc.push(("hips", [0.0, h * (-0.02 - 0.015 * b * e), 0.0]));
            }
            _ => {}
        }
        let mut pose = vec![BonePose::default(); n];
        for (bone, deg) in rot {
            if let Some(i) = rig.bone(bone) {
                pose[i].rot = q_from_euler(deg);
            }
        }
        for (bone, d) in loc {
            if let Some(i) = rig.bone(bone) {
                // World y (down) into the bone's rest frame.
                let r = rig.bones[i].rest_frame();
                pose[i].loc = crate::rig::q_rotate(crate::rig::q_conj(r.rot), d);
            }
        }
        out.push(pose);
    }
    let mut a = Action::from_frames(name, fps, &out);
    a.duration = cycle;
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::PoseInput;

    #[test]
    fn a_walk_swings_legs_in_turn_and_loops() {
        let m = crate::character::character(400.0, &crate::character::Build { detail: 1, face: false, ..Default::default() });
        let rig = m.rig.clone().unwrap();
        let walk = make(&rig, "walk", &Style::default()).unwrap();
        assert!((walk.duration - 1.1).abs() < 1e-3);
        let toe = |t: f32, side: &str| {
            let mut r = (*rig).clone();
            r.actions = vec![walk.clone()];
            let p = r.evaluate(&PoseInput { action: Some(("walk".into(), t, true)), ..Default::default() });
            r.bone_lines(&p)[r.bone(&format!("toe.{side}")).unwrap()].1
        };
        // A quarter in, the left foot is forward (-z) and the right back.
        let (l, r) = (toe(0.275, "L"), toe(0.275, "R"));
        assert!(l[2] < r[2] - 40.0, "{l:?} {r:?}");
        // Half a cycle later they swap; a full cycle later it is back.
        let (l2, r2) = (toe(0.825, "L"), toe(0.825, "R"));
        assert!(r2[2] < l2[2] - 40.0);
        let again = toe(0.275 + 1.1, "L");
        assert!(crate::length(crate::sub(again, l)) < 1.0);
        assert!(travel_per_cycle(&rig, "walk", &Style::default()) > 200.0);
        for n in NAMES {
            assert!(make(&rig, n, &Style::default()).is_ok(), "{n}");
        }
    }
}
