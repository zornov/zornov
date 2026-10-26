// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a character animator would catch in a performance, found from the
//! posed bones alone: feet that slide while down, feet that cross, limbs
//! stretched past their length or torn from their joints, joints bent the
//! wrong way or folded through themselves, hands and legs through the
//! body, and a character not looking where it was told to.
//!
//! Every finding says who, when and how much, and carries the exact fix.

use super::body::{Body, Kind, Limb, Side};
use super::query::{ClipXf, Points, annotate, points};
use super::solve::Solver;
use crate::rig::{Rig, Xf, q_conj, q_mul, q_rotate};
use crate::{V3, add, cross, dot, length, normalise, scale, sub};

/// Every kind this module reports.
pub const KINDS: &[&str] = &["char_foot_slide", "char_feet_cross", "char_limb_stretched", "char_joint_detached", "char_joint_broken", "char_through_body", "char_not_looking"];

/// One posed instant.
pub struct Sample {
    pub t: f32,
    pub xf: ClipXf,
    pub world: Vec<Xf>,
}

/// Where the character was told to look, over a stretch.
#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    pub t0: f32,
    pub t1: f32,
    /// A world point.
    pub target: V3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub kind: &'static str,
    pub t0: f32,
    pub t1: f32,
    /// Who: a limb or bone name.
    pub who: String,
    pub detail: String,
    pub fix: String,
}

fn runs(t: &[f32], flag: &[bool], gap: f32, min: f32) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in 0..t.len() {
        if !flag[i] {
            continue;
        }
        match out.last_mut() {
            Some(r) if t[i] - t[r.1] <= gap => r.1 = i,
            _ => out.push((i, i)),
        }
    }
    out.retain(|r| t[r.1] - t[r.0] >= min);
    out
}

fn fmt_t(t: f32) -> String {
    format!("{:.1}", t)
}

/// The legs' reference length: hip height above the soles, in the world.
pub fn leg_unit(rig: &Rig, body: &Body, k: f32) -> f32 {
    let mut s = Solver::new(rig);
    match body.legs.first() {
        Some(l) => (dot(s.head(l.upper), body.up) - body.sole_height(rig)).abs() * k,
        None => body.height(rig) * 0.5 * k,
    }
}

/// All findings over a series of samples (ascending in time, 10 or more a
/// second is plenty).
pub fn analyse(rig: &Rig, body: &Body, samples: &[Sample], intents: &[Intent]) -> Vec<Finding> {
    let mut out = Vec::new();
    if samples.len() < 2 {
        return out;
    }
    let k = samples[0].xf.scale.abs().max(1e-3);
    let ell = leg_unit(rig, body, k);
    let t: Vec<f32> = samples.iter().map(|s| s.t).collect();
    let mut pts: Vec<Points> = samples.iter().map(|s| points(rig, body, &s.world, &s.xf, s.t)).collect();
    annotate(&mut pts, ell, body.up[1].signum());
    slide(body, &t, &pts, ell, &mut out);
    cross_feet(rig, body, samples, &t, &pts, ell, &mut out);
    stretch(rig, body, samples, &t, k, &mut out);
    broken(rig, body, samples, &t, &pts, ell, &mut out);
    looking(&t, &pts, intents, &mut out);
    out
}

/// One stretch of a foot sliding while down.
#[derive(Clone, Debug, PartialEq)]
pub struct Slide {
    pub leg: String,
    pub t0: f32,
    pub t1: f32,
    /// Px slid along the ground.
    pub dist: f32,
    pub peak: f32,
}

/// Stretches where a foot in contact moves along the ground.
pub fn slides(body: &Body, t: &[f32], pts: &[Points], ell: f32) -> Vec<Slide> {
    let mut out = Vec::new();
    for (li, leg) in body.legs.iter().enumerate() {
        let flag: Vec<bool> = pts.iter().map(|p| p.feet[li].contact && p.feet[li].speed > 0.12 * ell).collect();
        for (a, b) in runs(t, &flag, 0.12, 0.2) {
            let dist: f32 = (a..=b).map(|i| pts[i].feet[li].speed * (t[(i + 1).min(t.len() - 1)] - t[i])).sum();
            if dist < 0.06 * ell {
                continue;
            }
            let peak = (a..=b).map(|i| pts[i].feet[li].speed).fold(0.0, f32::max);
            out.push(Slide { leg: leg.name.clone(), t0: t[a], t1: t[b], dist, peak });
        }
    }
    out
}

fn slide(body: &Body, t: &[f32], pts: &[Points], ell: f32, out: &mut Vec<Finding>) {
    for s in slides(body, t, pts, ell) {
        let (leg, dist, peak) = (s.leg, s.dist, s.peak);
        out.push(Finding {
            kind: "char_foot_slide",
            t0: s.t0,
            t1: s.t1,
            who: leg.clone(),
            detail: format!("the {} foot slides {:.0} px along the ground while it is down ({}-{} s, up to {:.0} px/s)", leg, dist, fmt_t(s.t0), fmt_t(s.t1), peak),
            fix: format!(
                "a foot in contact must not move: do not key the body's travel and the legs apart. Use act {{motion: walk (or run, sneak, limp...), route: [[x, z], ...] or to: [x, z], at: {}}} on the clip: it plants every foot and drives the body from the steps, so speed matches stride. If the legs are keyed by hand, key the clip's position so the planted foot stays put: advance the body by exactly the distance the leg swings back",
                fmt_t((s.t0 - 1.0).max(0.0))
            ),
        });
    }
}

fn cross_feet(rig: &Rig, body: &Body, samples: &[Sample], t: &[f32], pts: &[Points], ell: f32, out: &mut Vec<Finding>) {
    // Pairs of left and right legs (front with front).
    let mut pairs = Vec::new();
    for (i, a) in body.legs.iter().enumerate() {
        if a.side != Side::L {
            continue;
        }
        if let Some(j) = body.legs.iter().position(|b| b.side == Side::R && (b.front == a.front || body.kind == Kind::Biped)) {
            pairs.push((i, j));
        }
    }
    if pairs.is_empty() {
        return;
    }
    let mut rest = Solver::new(rig);
    let left_m = body.left();
    for (i, j) in pairs {
        let d0 = (dot(rest.head(body.legs[i].end), left_m) - dot(rest.head(body.legs[j].end), left_m)).abs() * samples[0].xf.scale;
        let flag: Vec<bool> = samples
            .iter()
            .zip(pts)
            .map(|(s, p)| {
                let l = normalise(s.xf.world_dir(left_m));
                let sep = dot(sub(p.feet[i].ankle, p.feet[j].ankle), l);
                let low_a = (p.feet[i].contact || p.feet[i].speed < 0.2 * ell) && p.feet[j].contact;
                sep < 0.12 * d0 && low_a
            })
            .collect();
        for (a, b) in runs(t, &flag, 0.1, 0.15) {
            let worst = (a..=b).map(|n| dot(sub(pts[n].feet[i].ankle, pts[n].feet[j].ankle), normalise(samples[n].xf.world_dir(left_m)))).fold(f32::INFINITY, f32::min);
            let state = if worst < 0.0 { format!("the {} foot is {:.0} px past the {} foot", body.legs[i].name, -worst, body.legs[j].name) } else { format!("the feet are only {:.0} px apart (standing {:.0})", worst, d0) };
            out.push(Finding {
                kind: "char_feet_cross",
                t0: t[a],
                t1: t[b],
                who: format!("{} / {}", body.legs[i].name, body.legs[j].name),
                detail: format!("feet cross or touch while down ({}-{} s): {state}", fmt_t(t[a]), fmt_t(t[b])),
                fix: "stand with the feet about hip width apart: finish a walk with act (it closes the last step beside the other foot, never across it), or key the feet's sideways places so the left stays left of the right while both are down; for a turn on the spot act {motion: turn, face: heading} steps the feet round".into(),
            });
        }
    }
}

fn stretch(rig: &Rig, body: &Body, samples: &[Sample], t: &[f32], k: f32, out: &mut Vec<Finding>) {
    // Joints that are one point at rest must still be one point.
    let n = rig.bones.len();
    let connected: Vec<(usize, usize)> = (0..n).filter_map(|i| rig.bones[i].parent.filter(|&p| length(sub(rig.bones[p].tail, rig.bones[i].head)) < 0.05 * rig.bones[p].length().max(1.0)).map(|p| (p, i))).collect();
    let limbs: Vec<&Limb> = body.legs.iter().chain(body.arms.iter()).collect();
    let mut gap_flag: Vec<Vec<bool>> = vec![vec![false; samples.len()]; connected.len()];
    let mut gap_px: Vec<f32> = vec![0.0; connected.len()];
    let mut stretch_flag: Vec<Vec<bool>> = vec![vec![false; samples.len()]; limbs.len()];
    let mut stretch_by: Vec<f32> = vec![0.0; limbs.len()];
    for (si, s) in samples.iter().enumerate() {
        for (ci, &(p, c)) in connected.iter().enumerate() {
            let tail = s.world[p].apply([0.0, rig.bones[p].length(), 0.0]);
            let gap = length(sub(s.world[c].pos, tail));
            let allowed = 0.06 * rig.bones[p].length() + 0.8;
            if gap > allowed {
                gap_flag[ci][si] = true;
                gap_px[ci] = gap_px[ci].max(gap * k);
            }
        }
        for (li, l) in limbs.iter().enumerate() {
            let len = Body::limb_length(rig, l);
            let d = length(sub(s.world[l.end].pos, s.world[l.upper].pos));
            let scaled = [l.upper, l.lower].iter().any(|&b| (s.world[b].scale - 1.0).abs() > 0.03);
            if d > 1.025 * len || scaled {
                stretch_flag[li][si] = true;
                stretch_by[li] = stretch_by[li].max((d / len - 1.0) * 100.0);
            }
        }
    }
    for (li, l) in limbs.iter().enumerate() {
        for (a, b) in runs(t, &stretch_flag[li], 0.1, 0.1) {
            out.push(Finding {
                kind: "char_limb_stretched",
                t0: t[a],
                t1: t[b],
                who: l.name.clone(),
                detail: format!("{} is stretched {:.0}% past its length ({}-{} s)", l.name, stretch_by[li], fmt_t(t[a]), fmt_t(t[b])),
                fix: format!("bones keep their length: reach with pose {{reach: {{\"{}\": [x, y, z]}}}} or pose {{hand: ..., reach: ...}}, which limits the reach to the arm's length and leans or steps for a far target (add step: true); remove any bone offsets (bone.<name>.lx/ly/lz) or scales keyed on the arm or leg", l.name),
            });
        }
    }
    for (ci, &(p, c)) in connected.iter().enumerate() {
        for (a, b) in runs(t, &gap_flag[ci], 0.1, 0.1) {
            out.push(Finding {
                kind: "char_joint_detached",
                t0: t[a],
                t1: t[b],
                who: format!("{} / {}", rig.bones[p].name, rig.bones[c].name),
                detail: format!("{} comes away from {} by {:.0} px ({}-{} s): the limb is torn at the joint", rig.bones[c].name, rig.bones[p].name, gap_px[ci], fmt_t(t[a]), fmt_t(t[b])),
                fix: format!("only rotate bones (bone.{}.x/y/z); remove the location keys bone.{}.lx/ly/lz that move it off its parent", rig.bones[c].name, rig.bones[c].name),
            });
        }
    }
}

fn dist_seg(p: V3, a: V3, b: V3) -> f32 {
    let ab = sub(b, a);
    let u = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-9)).clamp(0.0, 1.0);
    length(sub(p, add(a, scale(ab, u))))
}

fn angle_between(a: V3, b: V3) -> f32 {
    dot(normalise(a), normalise(b)).clamp(-1.0, 1.0).acos().to_degrees()
}

fn broken(rig: &Rig, body: &Body, samples: &[Sample], t: &[f32], pts: &[Points], ell: f32, out: &mut Vec<Finding>) {
    let rest = rig.rest_world();
    // The hinge of each limb, fixed in its upper bone: knees fold the shin
    // back (the knee points forward), elbows fold the forearm forward (the
    // elbow points back); an animal's front legs fold the other way. The
    // axis is square to the pole and to the limb at rest.
    let mut s0 = Solver::new(rig);
    let limbs: Vec<&Limb> = body.legs.iter().chain(body.arms.iter()).collect();
    let hinges: Vec<V3> = limbs
        .iter()
        .map(|l| {
            let is_leg = body.legs.iter().any(|x| std::ptr::eq(x, *l));
            let pole = if is_leg {
                super::pose::bend_of_leg(body, rig, l)
            } else {
                scale(body.fwd, -1.0)
            };
            let u = normalise(sub(s0.head(l.lower), s0.head(l.upper)));
            normalise(cross(pole, u))
        })
        .collect();
    for (li, l) in limbs.iter().enumerate() {
        let mut back = vec![false; samples.len()];
        let mut folded = vec![false; samples.len()];
        let mut worst = (0.0f32, 0.0f32);
        for (si, s) in samples.iter().enumerate() {
            let u = sub(s.world[l.lower].pos, s.world[l.upper].pos);
            let f = sub(s.world[l.end].pos, s.world[l.lower].pos);
            let flex = angle_between(u, f);
            let r = q_mul(s.world[l.upper].rot, q_conj(rest[l.upper].rot));
            let h = q_rotate(r, hinges[li]);
            let along = dot(cross(normalise(u), normalise(f)), h);
            if flex > 18.0 && along < -0.1 {
                back[si] = true;
                worst.0 = worst.0.max(flex);
            }
            if flex > 158.0 {
                folded[si] = true;
                worst.1 = worst.1.max(flex);
            }
        }
        let joint = if body.legs.iter().any(|x| std::ptr::eq(x, *l)) { "knee" } else { "elbow" };
        for (a, b) in runs(t, &back, 0.1, 0.1) {
            out.push(Finding {
                kind: "char_joint_broken",
                t0: t[a],
                t1: t[b],
                who: l.name.clone(),
                detail: format!("the {} of {} bends backwards, {:.0} degrees ({}-{} s)", joint, l.name, worst.0, fmt_t(t[a]), fmt_t(t[b])),
                fix: format!("a {joint} only folds one way: use pose {{reach: {{\"{}\": [x, y, z]}}}} (IK bends it the natural way) or pose {{body: ...}} instead of keying the bone angles by hand; if keying, give {} a positive bend about x", l.name, rig.bones[l.lower].name),
            });
        }
        for (a, b) in runs(t, &folded, 0.1, 0.1) {
            out.push(Finding {
                kind: "char_joint_broken",
                t0: t[a],
                t1: t[b],
                who: l.name.clone(),
                detail: format!("the {} of {} is folded flat on itself, {:.0} degrees ({}-{} s)", joint, l.name, worst.1, fmt_t(t[a]), fmt_t(t[b])),
                fix: format!("keep the {joint} under about 150 degrees: reduce the bend keyed on {}", rig.bones[l.lower].name),
            });
        }
    }
    // Head and torso bent past what a spine does.
    if let (Some(h), Some(_)) = (body.head, body.neck.or(body.chest)) {
        let up_rest = rest[h].apply_dir([0.0, 1.0, 0.0]);
        let flag: Vec<bool> = samples.iter().map(|s| angle_between(s.world[h].apply_dir([0.0, 1.0, 0.0]), up_rest) > 88.0).collect();
        for (a, b) in runs(t, &flag, 0.1, 0.15) {
            out.push(Finding {
                kind: "char_joint_broken",
                t0: t[a],
                t1: t[b],
                who: rig.bones[h].name.clone(),
                detail: format!("the head is tipped more than 88 degrees from upright ({}-{} s): the neck is broken", fmt_t(t[a]), fmt_t(t[b])),
                fix: "keep the head within about 60 degrees of the body's line: pose look_at turns the eyes, head and chest in turn instead of one bone far; or turn the whole character (act turn)".into(),
            });
        }
    }
    // Through the body.
    if body.kind == Kind::Biped && body.neck.is_some() {
        let neck = body.neck.unwrap_or(body.hips);
        let mut flag: Vec<Vec<bool>> = vec![vec![false; samples.len()]; body.arms.len()];
        for (si, s) in samples.iter().enumerate() {
            let a = s.world[body.hips].pos;
            let b = s.world[neck].pos;
            let r = 0.2 * length(sub(b, a));
            for (ai, arm) in body.arms.iter().enumerate() {
                let w = s.world[arm.end].pos;
                if dist_seg(w, a, b) < 0.7 * r && dot(sub(w, a), body.up).abs() < length(sub(b, a)) * 1.1 {
                    flag[ai][si] = true;
                }
            }
        }
        for (ai, arm) in body.arms.iter().enumerate() {
            for (a, b) in runs(t, &flag[ai], 0.1, 0.15) {
                out.push(Finding {
                    kind: "char_through_body",
                    t0: t[a],
                    t1: t[b],
                    who: arm.name.clone(),
                    detail: format!("the hand of {} is inside the torso ({}-{} s)", arm.name, fmt_t(t[a]), fmt_t(t[b])),
                    fix: format!("bring the hand to the body's surface or round it: pose {{reach: {{\"{}\": [x, y, z]}}}} with a target in front of the chest keeps the elbow out; for crossed arms aim the wrists at the opposite elbow, not the chest's middle", arm.name),
                });
            }
        }
    }
    if body.legs.len() >= 2 {
        let (a, b) = (0, 1);
        let flag: Vec<bool> = samples
            .iter()
            .map(|s| {
                let d1 = dist_seg(s.world[body.legs[a].end].pos, s.world[body.legs[b].lower].pos, s.world[body.legs[b].end].pos);
                let d2 = dist_seg(s.world[body.legs[b].end].pos, s.world[body.legs[a].lower].pos, s.world[body.legs[a].end].pos);
                d1.min(d2) * s.xf.scale < 0.02 * ell
            })
            .collect();
        for (x, y) in runs(t, &flag, 0.1, 0.15) {
            out.push(Finding {
                kind: "char_through_body",
                t0: t[x],
                t1: t[y],
                who: format!("{} / {}", body.legs[a].name, body.legs[b].name),
                detail: format!("one leg passes through the other ({}-{} s)", fmt_t(t[x]), fmt_t(t[y])),
                fix: "keep the feet on their own sides while walking: act walk does; for a crossed-leg sit, cross the ankles (one in front of the other) rather than through".into(),
            });
        }
    }
    let _ = pts;
}

fn looking(t: &[f32], pts: &[Points], intents: &[Intent], out: &mut Vec<Finding>) {
    for it in intents {
        let flag: Vec<(bool, f32)> = (0..t.len())
            .map(|i| {
                if t[i] < it.t0 + 0.6 || t[i] > it.t1 {
                    return (false, 0.0);
                }
                let want = normalise(sub(it.target, pts[i].eyes));
                let err = angle_between(pts[i].gaze, want);
                (err > 25.0, err)
            })
            .collect();
        let f: Vec<bool> = flag.iter().map(|x| x.0).collect();
        for (a, b) in runs(t, &f, 0.15, 0.3) {
            let worst = (a..=b).map(|i| flag[i].1).fold(0.0, f32::max);
            out.push(Finding {
                kind: "char_not_looking",
                t0: t[a],
                t1: t[b],
                who: "gaze".into(),
                detail: format!("told to look at ({:.0}, {:.0}, {:.0}) but the gaze is up to {:.0} degrees off it ({}-{} s)", it.target[0], it.target[1], it.target[2], worst, fmt_t(t[a]), fmt_t(t[b])),
                fix: format!("redo it with pose {{look_at: [x, y, z] or {{clip: id}}, at: {}}}: the eyes lead, then the head and chest turn; if it says the target is behind, turn the character first (act turn)", fmt_t(it.t0)),
            });
        }
    }
}
