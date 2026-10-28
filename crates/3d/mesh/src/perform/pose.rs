// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Body poses and acting beats as additive layers.
//!
//! Everything here changes a [`Solver`] that already holds a base pose
//! (rest, or a frame of a walk). A layer is made by posing at a series of
//! moments and keeping, per bone, what differs from the base: those
//! differences are what the clip plays on top of whatever else it does, so
//! a look can play over a walk.
//!
//! * [`look`]: the eyes lead, the head follows, the chest last; a target
//!   past what they can reach comes back with how much body turn is
//!   missing.
//! * [`reach`]: a hand to a point, the elbow bending the natural way, the
//!   body leaning in a little and never a bone stretched; too far says by
//!   how much.
//! * [`sit`], [`crouch`], [`lean`]: feet stay planted, the hips move.
//! * [`beat`]: nod, shake the head, shrug, sigh, startle, react, glance.

use super::body::{Body, Limb, Side};
use super::hands;
use super::solve::{Reached, Solver, signed_angle};
use crate::rig::{BonePose, Rig, q_conj, q_mul, q_rotate, q_to_euler};
use crate::{V3, add, dot, length, normalise, scale, sub};
use std::f32::consts::PI;

/// A bone's channel over time, ready to become clip keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub bone: String,
    /// `x`, `y`, `z` (degrees about the bone) or `lx`, `ly`, `lz` (px).
    pub axis: &'static str,
    /// (seconds, value), simplified.
    pub keys: Vec<(f32, f32)>,
}

pub fn smooth(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// 0 → 1 over `[t0, t0 + dur]`.
pub fn ramp(t: f32, t0: f32, dur: f32) -> f32 {
    smooth((t - t0) / dur.max(1e-3))
}

/// The ramp up, a hold, the ramp down: 1 between.
pub fn envelope(t: f32, t0: f32, t1: f32, fade_in: f32, fade_out: f32) -> f32 {
    ramp(t, t0, fade_in) * (1.0 - ramp(t, t1, fade_out))
}

/// Poses at each of `times` with `pose`, from `base(t)`, and keeps what
/// changed per bone. `tol_deg` and `tol_px` thin the keys.
pub fn layer(rig: &Rig, times: &[f32], base: &dyn Fn(f32) -> Vec<BonePose>, pose: &mut dyn FnMut(&mut Solver, f32), tol_deg: f32, tol_px: f32) -> Vec<Channel> {
    let n = rig.bones.len();
    let mut rot: Vec<Vec<V3>> = vec![Vec::with_capacity(times.len()); n];
    let mut loc: Vec<Vec<V3>> = vec![Vec::with_capacity(times.len()); n];
    for &t in times {
        let b = base(t);
        let mut s = Solver::with_pose(rig, b.clone());
        pose(&mut s, t);
        for i in 0..n {
            let (nb, bb) = (s.local[i], b.get(i).copied().unwrap_or_default());
            rot[i].push(q_to_euler(q_mul(q_conj(bb.rot), nb.rot)));
            loc[i].push(crate::sub(nb.loc, bb.loc));
        }
    }
    let mut out = Vec::new();
    for i in 0..n {
        for (a, name) in ["x", "y", "z"].iter().enumerate() {
            let vals: Vec<f32> = rot[i].iter().map(|e| e[a]).collect();
            if vals.iter().fold(0.0f32, |m, v| m.max(v.abs())) > tol_deg {
                out.push(Channel { bone: rig.bones[i].name.clone(), axis: name, keys: simplify(times, &vals, tol_deg) });
            }
        }
        for (a, name) in ["lx", "ly", "lz"].iter().enumerate() {
            let vals: Vec<f32> = loc[i].iter().map(|e| e[a]).collect();
            if vals.iter().fold(0.0f32, |m, v| m.max(v.abs())) > tol_px {
                out.push(Channel { bone: rig.bones[i].name.clone(), axis: name, keys: simplify(times, &vals, tol_px) });
            }
        }
    }
    out
}

/// Keeps the keys a curve cannot stand in for: lines first
/// (Ramer-Douglas-Peucker), then the curve the keys are played on. They
/// become smooth ([`studio_model::Interp::Auto`]) keys, which ease into
/// every key that turns or stops, so a straight ramp kept as two keys
/// played as an S up to a unit away from the samples; each sample the
/// played curve misses by more than `tol` is kept as well, the worst
/// first, until none does.
pub fn simplify(t: &[f32], v: &[f32], tol: f32) -> Vec<(f32, f32)> {
    let cols = [v.to_vec()];
    let mut keep = keep_indices(t, &cols, tol);
    let us = |s: f32| (f64::from(s) * 1e6).round() as studio_model::time::Micros;
    let ordered = t.windows(2).all(|w| us(w[1]) > us(w[0]));
    while ordered && keep.len() < t.len() {
        let track = studio_model::Track::new(keep.iter().map(|&i| studio_model::Keyframe::new(us(t[i]), f64::from(v[i]), studio_model::Interp::Auto)).collect());
        let mut worst = (tol, None);
        for (i, (&ti, &vi)) in t.iter().zip(v).enumerate() {
            let played = track.eval(us(ti)).and_then(|x| x.as_f64()).unwrap_or(f64::from(vi));
            let miss = (played - f64::from(vi)).abs() as f32;
            if miss > worst.0 {
                worst = (miss, Some(i));
            }
        }
        match worst.1 {
            Some(i) => {
                let at = keep.partition_point(|&k| k < i);
                keep.insert(at, i);
            }
            None => break,
        }
    }
    keep.into_iter().map(|i| (t[i], v[i])).collect()
}

/// The indices of the samples that matter when several values (a point's
/// x and y) must stay within `tol` of the line through the kept ones.
pub fn keep_indices(t: &[f32], cols: &[Vec<f32>], tol: f32) -> Vec<usize> {
    let n = t.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let u = (t[i] - t[a]) / (t[b] - t[a]).max(1e-9);
            let d = cols.iter().map(|v| (v[i] - (v[a] + (v[b] - v[a]) * u)).abs()).fold(0.0, f32::max);
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

// ------------------------------------------------------------------ look

/// What a look came to.
#[derive(Clone, Debug, PartialEq)]
pub struct Looked {
    /// Turn and tilt asked of the gaze, degrees (left and up positive).
    pub yaw: f32,
    pub pitch: f32,
    /// What the eyes, head (with neck) and chest took, yaw.
    pub eyes: f32,
    pub head: f32,
    pub chest: f32,
    /// Degrees of body turn still missing: the character must turn.
    pub short_by: f32,
}

/// Where the character looks from.
pub fn eye_point(s: &mut Solver, body: &Body) -> V3 {
    if !body.eyes.is_empty() {
        let sum = body.eyes.iter().map(|&e| s.head(e)).fold([0.0; 3], add);
        return scale(sum, 1.0 / body.eyes.len() as f32);
    }
    match body.head {
        Some(h) => {
            let a = s.head(h);
            let t = s.tail(h);
            add(a, scale(sub(t, a), 0.55))
        }
        None => [0.0; 3],
    }
}

/// The way the head (or an eye) faces now.
pub fn facing(s: &mut Solver, body: &Body, bone: usize) -> V3 {
    let f = s.frame(bone);
    let rest = s.rest_rot(bone);
    q_rotate(q_mul(f.rot, q_conj(rest)), body.fwd)
}

/// Eyes, head and chest turn toward `target` (model px). `w` is how far
/// each has got (0..1): the eyes move first, the head after, the chest last,
/// so ramping the three with a lag gives a look that leads with the eyes.
pub fn look(s: &mut Solver, body: &Body, target: V3, w: [f32; 3], head_share: f32) -> Looked {
    let up = body.up;
    let left = body.left();
    let Some(head) = body.head else { return Looked { yaw: 0.0, pitch: 0.0, eyes: 0.0, head: 0.0, chest: 0.0, short_by: 0.0 } };
    let from = eye_point(s, body);
    let want = normalise(sub(target, from));
    let now = facing(s, body, head);
    let yaw = signed_angle(now, want, up);
    let pitch = want.iter().zip(up).map(|(a, b)| a * b).sum::<f32>().clamp(-1.0, 1.0).asin().to_degrees() - dot(now, up).clamp(-1.0, 1.0).asin().to_degrees();
    let has_eyes = !body.eyes.is_empty();
    let clamp = |v: f32, m: f32| v.clamp(-m, m);
    // Shares of the turn: eyes first, then head and neck, the chest the rest.
    let eyes_yaw = if has_eyes { clamp(yaw * 0.4, 30.0) } else { 0.0 };
    let head_yaw = clamp((yaw - eyes_yaw) * head_share.clamp(0.2, 1.0), 62.0);
    let chest_yaw = clamp(yaw - eyes_yaw - head_yaw, 30.0);
    let short_by = (yaw - eyes_yaw - head_yaw - chest_yaw).abs();
    let eyes_pitch = if has_eyes { clamp(pitch * 0.4, 20.0) } else { 0.0 };
    let head_pitch = clamp((pitch - eyes_pitch) * 0.85, 38.0);
    let chest_pitch = clamp(pitch - eyes_pitch - head_pitch, 10.0);
    let torso: Vec<usize> = body.spine.iter().copied().chain(body.chest).collect();
    // Parents first: chest, neck, head, eyes.
    let (wc, wh, we) = (w[2], w[1], w[0]);
    let chest_bone = body.chest.or_else(|| body.spine.last().copied());
    if let Some(c) = chest_bone {
        let share = if torso.len() > 1 { 1.0 / torso.len() as f32 } else { 1.0 };
        for &b in &torso {
            s.rotate(b, up, chest_yaw * wc * share);
            s.rotate(b, left, -chest_pitch * wc * share);
        }
        let _ = c;
    }
    if let Some(n) = body.neck {
        s.rotate(n, up, head_yaw * wh * 0.35);
        s.rotate(n, left, -head_pitch * wh * 0.35);
    }
    let neck_part = if body.neck.is_some() { 0.65 } else { 1.0 };
    s.rotate(head, up, head_yaw * wh * neck_part);
    s.rotate(head, left, -head_pitch * wh * neck_part);
    for &e in &body.eyes {
        s.rotate(e, up, eyes_yaw * we);
        s.rotate(e, left, -eyes_pitch * we);
    }
    // What is left after the weights is small; close it with the eyes (or head).
    if (w[0] - 1.0).abs() < 1e-3 && (w[1] - 1.0).abs() < 1e-3 {
        let last = body.eyes.first().copied().unwrap_or(head);
        let from = eye_point(s, body);
        let want = normalise(sub(target, from));
        let now = facing(s, body, last);
        let (ry, rp) = (signed_angle(now, want, up), dot(want, up).clamp(-1.0, 1.0).asin().to_degrees() - dot(now, up).clamp(-1.0, 1.0).asin().to_degrees());
        let lim = if has_eyes { 12.0 } else { 6.0 };
        let closers: Vec<usize> = if body.eyes.is_empty() { vec![head] } else { body.eyes.clone() };
        for e in closers {
            s.rotate(e, up, ry.clamp(-lim, lim));
            s.rotate(e, left, -rp.clamp(-lim, lim));
        }
    }
    Looked { yaw, pitch, eyes: eyes_yaw, head: head_yaw, chest: chest_yaw, short_by }
}

// ----------------------------------------------------------------- reach

/// What a reach came to, for the report.
#[derive(Clone, Debug, PartialEq)]
pub struct ReachInfo {
    pub reached: Reached,
    /// Straight-line distance shoulder to target, model px.
    pub distance: f32,
    /// How far past the arm's length the target is (0 when in reach).
    pub short_by: f32,
    /// How much the body leaned in, degrees.
    pub leaned: f32,
}

/// A hand (the wrist) to `target`, elbow natural, no bone stretched. `w`
/// blends from the base pose (0) to the full reach (1) along the way.
pub fn reach(s: &mut Solver, body: &Body, rig: &Rig, side: Side, target: V3, w: f32, lean_in: bool) -> Option<ReachInfo> {
    let arm = body.arm(side)?.clone();
    let up = body.up;
    let left = body.left();
    let len = Body::limb_length(rig, &arm);
    let wrist0 = s.head(arm.end);
    let goal = add(wrist0, scale(sub(target, wrist0), smooth(w)));
    // Lean the body toward a far target until it is within reach.
    let mut leaned = 0.0;
    let torso: Vec<usize> = body.spine.iter().copied().chain(body.chest).collect();
    if lean_in && !torso.is_empty() {
        let shoulder = s.head(arm.upper);
        let dist = length(sub(goal, shoulder));
        if dist > 0.92 * len {
            let dir = normalise(sub(goal, s.head(torso[0])));
            let need = (dist - 0.92 * len) / len;
            let total = (need * 70.0).min(26.0) * smooth(w).max(0.0);
            for &b in &torso {
                s.tilt_toward(b, dir, total / torso.len() as f32);
            }
            leaned = total;
        }
    }
    let shoulder = s.head(arm.upper);
    let sign = side.sign();
    let mut bend = normalise(add(add(scale(up, -1.0), scale(left, 0.45 * sign)), scale(body.fwd, -0.25)));
    let mut r = s.limb_to(&arm, goal, bend);
    // An elbow inside the chest goes out.
    let mid = s.head(arm.lower);
    let spine_top = body.neck.map(|n| s.head(n)).unwrap_or_else(|| s.head(body.hips));
    let spine_bot = s.head(body.hips);
    let radius = 0.06 * length(sub(spine_top, spine_bot)) * 2.0;
    if dist_to_segment(mid, spine_bot, spine_top) < radius {
        bend = normalise(add(bend, scale(left, 0.9 * sign)));
        r = s.limb_to(&arm, goal, bend);
    }
    let distance = length(sub(target, shoulder));
    Some(ReachInfo { reached: r, distance, short_by: (distance - len).max(0.0), leaned })
}

fn dist_to_segment(p: V3, a: V3, b: V3) -> f32 {
    let ab = sub(b, a);
    let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-9)).clamp(0.0, 1.0);
    length(sub(p, add(a, scale(ab, t))))
}

// ------------------------------------------------------- sit, crouch, lean

fn keep_feet(s: &mut Solver, body: &Body, rig: &Rig, ankles: &[V3]) {
    for (leg, a) in body.legs.iter().zip(ankles) {
        let bend = bend_of_leg(body, rig, leg);
        s.limb_to(leg, *a, bend);
    }
}

/// The way a leg's middle joint bends: toward the front for knees, back
/// for an animal's front legs.
pub fn bend_of_leg(body: &Body, _rig: &Rig, leg: &Limb) -> V3 {
    if body.kind == super::body::Kind::Quadruped && leg.front { scale(body.fwd, -1.0) } else { body.fwd }
}

/// Sits `w` of the way down onto a seat `seat` px above the soles (default
/// knee height): the hips go back and down with the feet staying put.
pub fn sit(s: &mut Solver, body: &Body, rig: &Rig, w: f32, seat: Option<f32>) {
    if body.legs.len() < 2 {
        return;
    }
    let w = smooth(w);
    let ankles: Vec<V3> = body.legs.iter().map(|l| s.head(l.end)).collect();
    let leg0 = &body.legs[0];
    let hip_y = dot(s.head(leg0.upper), body.up);
    let sole = body.sole_height(rig);
    let hip_h = hip_y - sole;
    let shin = rig.bones[leg0.lower].length();
    let thigh = rig.bones[leg0.upper].length();
    let ankle_h = dot(ankles[0], body.up) - sole;
    let seat_h = seat.unwrap_or(shin + ankle_h);
    let drop = (hip_h - seat_h).max(0.0) * w;
    let back = thigh * 0.92 * w;
    s.shift_root(body.root, add(scale(body.up, -drop), scale(body.fwd, -back)));
    // The torso stays upright over the hips.
    keep_feet(s, body, rig, &ankles);
}

/// Crouches `depth` (0..1) of the way: the hips drop and move back a little,
/// the torso leans forward to balance, the feet stay put.
pub fn crouch(s: &mut Solver, body: &Body, rig: &Rig, depth: f32) {
    if body.legs.len() < 2 {
        return;
    }
    let d = depth.clamp(0.0, 1.0);
    let ankles: Vec<V3> = body.legs.iter().map(|l| s.head(l.end)).collect();
    let hip_h = dot(s.head(body.legs[0].upper), body.up) - body.sole_height(rig);
    let drop = d * 0.5 * hip_h;
    s.shift_root(body.root, add(scale(body.up, -drop), scale(body.fwd, -0.3 * drop)));
    let torso: Vec<usize> = body.spine.iter().copied().chain(body.chest).collect();
    for &b in &torso {
        s.tilt_toward(b, body.fwd, d * 32.0 / torso.len().max(1) as f32);
    }
    if let Some(h) = body.head {
        s.tilt_toward(h, body.fwd, -d * 18.0);
    }
    keep_feet(s, body, rig, &ankles);
}

/// Leans the body toward `dir` (a model direction across up) by `deg`,
/// the hips going the other way to keep the balance, feet planted.
pub fn lean(s: &mut Solver, body: &Body, rig: &Rig, dir: V3, deg: f32) {
    let ankles: Vec<V3> = body.legs.iter().map(|l| s.head(l.end)).collect();
    let torso: Vec<usize> = body.spine.iter().copied().chain(body.chest).collect();
    for &b in &torso {
        s.tilt_toward(b, dir, deg / torso.len().max(1) as f32);
    }
    if let Some(h) = body.head {
        s.tilt_toward(h, dir, -deg * 0.5);
    }
    let hip_h = if body.legs.is_empty() { 100.0 } else { dot(s.head(body.legs[0].upper), body.up) - body.sole_height(rig) };
    s.shift_root(body.root, scale(dir, -deg.to_radians().sin() * 0.25 * hip_h));
    if !body.legs.is_empty() {
        keep_feet(s, body, rig, &ankles);
    }
}

// --------------------------------------------------------------- recipes

/// Two-handed actions built from the same primitives: reaches to places in
/// front of the chest and the hand poses of [`super::hands`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recipe {
    /// A small thing held in one hand, struck across a surface held in the
    /// other, cupped to shelter it (a match, a flint, a lighter's wheel).
    Strike,
    /// Both hands brought together, cupped, in front of the chest (to
    /// hold, catch, carry or drink from).
    CuppedHands,
}

pub const RECIPES: &[&str] = &["strike", "cupped_hands"];

impl Recipe {
    pub fn parse(s: &str) -> Option<Recipe> {
        Some(match s.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "strike" | "strike_match" | "strike_light" | "strike_a_light" | "strike_flint" => Recipe::Strike,
            "cupped_hands" | "cup_hands" | "two_hands_cupped" | "hands_cupped" | "cupped_together" => Recipe::CuppedHands,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Recipe::Strike => "strike",
            Recipe::CuppedHands => "cupped_hands",
        }
    }

    pub fn seconds(self) -> f32 {
        match self {
            Recipe::Strike => 2.6,
            Recipe::CuppedHands => 1.4,
        }
    }
}

/// Where the hands meet: in front of the chest, a little below the chin.
pub fn recipe_gaze(s: &mut Solver, body: &Body, rig: &Rig) -> V3 {
    let h = body.height(rig);
    let top = body.neck.map(|n| s.head(n)).unwrap_or_else(|| s.head(body.hips));
    let bot = s.head(body.hips);
    let chest = add(bot, scale(sub(top, bot), 0.85));
    add(add(chest, scale(body.fwd, 0.17 * h)), scale(body.up, -0.02 * h))
}

/// Poses a recipe at progress `u` (0..1); returns the two wrist targets.
pub fn recipe(s: &mut Solver, body: &Body, rig: &Rig, kind: Recipe, dominant: Side, u: f32) {
    let h = body.height(rig);
    let focus = recipe_gaze(s, body, rig);
    let dom = dominant.sign();
    let left = body.left();
    let other = if dominant == Side::L { Side::R } else { Side::L };
    let approach = ramp(u, 0.0, 0.3) * (1.0 - ramp(u, 0.88, 0.12));
    match kind {
        Recipe::Strike => {
            // The shelter hand rests at the middle; the striking hand comes from the side and sweeps across it.
            let shelter = add(focus, scale(left, -dom * 0.02 * h));
            let start = add(add(focus, scale(left, dom * 0.11 * h)), scale(body.fwd, 0.015 * h));
            let end = add(add(focus, scale(left, -dom * 0.02 * h)), scale(body.up, -0.015 * h));
            let stroke = ramp(u, 0.38, 0.17);
            let strike_at = add(start, scale(sub(end, start), stroke));
            reach(s, body, rig, other, shelter, approach, false);
            reach(s, body, rig, dominant, strike_at, approach, false);
            let p_cup = hands::named("cup").unwrap_or_else(|| hands::named("relaxed").expect("relaxed"));
            let p_hold = hands::named("pinch").unwrap_or_else(|| hands::named("relaxed").expect("relaxed"));
            hands::apply(s, body, rig, other, &p_cup, approach);
            hands::apply(s, body, rig, dominant, &p_hold, approach);
        }
        Recipe::CuppedHands => {
            let (l, r) = (add(focus, scale(left, 0.022 * h)), add(focus, scale(left, -0.022 * h)));
            reach(s, body, rig, Side::L, l, approach, false);
            reach(s, body, rig, Side::R, r, approach, false);
            let p = hands::named("cup").unwrap_or_else(|| hands::named("relaxed").expect("relaxed"));
            hands::apply(s, body, rig, Side::L, &p, approach);
            hands::apply(s, body, rig, Side::R, &p, approach);
        }
    }
}

// ----------------------------------------------------------------- beats

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    Nod,
    Shake,
    Shrug,
    Sigh,
    Startle,
    React,
    Glance,
}

pub const BEATS: &[&str] = &["nod", "shake_head", "shrug", "sigh", "startle", "react", "glance"];

impl Beat {
    pub fn parse(s: &str) -> Option<Beat> {
        Some(match s.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "nod" | "yes" => Beat::Nod,
            "shake_head" | "shake" | "no" | "head_shake" => Beat::Shake,
            "shrug" => Beat::Shrug,
            "sigh" => Beat::Sigh,
            "startle" | "flinch" | "jump" => Beat::Startle,
            "react" | "reaction" => Beat::React,
            "glance" => Beat::Glance,
            _ => return None,
        })
    }

    /// How long it takes, seconds.
    pub fn seconds(self) -> f32 {
        match self {
            Beat::Nod => 1.0,
            Beat::Shake => 1.2,
            Beat::Shrug => 1.1,
            Beat::Sigh => 2.2,
            Beat::Startle => 0.9,
            Beat::React => 1.4,
            Beat::Glance => 1.2,
        }
    }
}

fn bell(u: f32) -> f32 {
    (PI * u.clamp(0.0, 1.0)).sin().powi(2)
}

/// Poses an acting beat at progress `u` (0..1), `size` around 1. Glance and
/// react aim at `toward` (model px) when given.
pub fn beat(s: &mut Solver, body: &Body, kind: Beat, u: f32, size: f32, toward: Option<V3>) {
    let up = body.up;
    let f = body.fwd;
    let left = body.left();
    let head = body.head;
    let torso: Vec<usize> = body.spine.iter().copied().chain(body.chest).collect();
    let tilt_torso = |s: &mut Solver, dir: V3, deg: f32| {
        for &b in &torso {
            s.tilt_toward(b, dir, deg / torso.len().max(1) as f32);
        }
    };
    match kind {
        Beat::Nod => {
            let a = 16.0 * size * (PI * 2.0 * u).sin().abs() * (1.0 - u * 0.25) * ramp(u, 0.0, 0.08);
            if let Some(n) = body.neck {
                s.tilt_toward(n, f, a * 0.4);
            }
            if let Some(h) = head {
                s.tilt_toward(h, f, a * 0.8);
            }
        }
        Beat::Shake => {
            let a = 24.0 * size * (PI * 5.0 * u).sin() * bell(u).sqrt();
            if let Some(n) = body.neck {
                s.rotate(n, up, a * 0.3);
            }
            if let Some(h) = head {
                s.rotate(h, up, a * 0.7);
            }
        }
        Beat::Shrug => {
            let e = (ramp(u, 0.0, 0.25)) * (1.0 - ramp(u, 0.65, 0.3));
            for (side, sh) in &body.shoulders {
                s.tilt_toward(*sh, up, 14.0 * size * e);
                let _ = side;
            }
            for arm in &body.arms {
                s.tilt_toward(arm.upper, scale(left, arm.side.sign()), 12.0 * size * e);
                s.tilt_toward(arm.lower, up, 30.0 * size * e);
            }
            if let Some(h) = head {
                s.tilt_toward(h, left, 6.0 * size * e);
                s.tilt_toward(h, scale(up, -1.0), 0.0);
            }
        }
        Beat::Sigh => {
            let inhale = ramp(u, 0.0, 0.35) * (1.0 - ramp(u, 0.35, 0.05));
            let exhale = ramp(u, 0.38, 0.4) * (1.0 - ramp(u, 0.8, 0.2));
            tilt_torso(s, scale(f, -1.0), 5.0 * size * inhale);
            tilt_torso(s, f, 9.0 * size * exhale);
            for (_, sh) in &body.shoulders {
                s.tilt_toward(*sh, up, 6.0 * size * inhale - 5.0 * size * exhale);
            }
            if let Some(h) = head {
                s.tilt_toward(h, f, 9.0 * size * exhale - 3.0 * size * inhale);
            }
        }
        Beat::Startle | Beat::React => {
            let k = if kind == Beat::React { 0.45 } else { 1.0 };
            let attack = ramp(u, 0.0, 0.12);
            let decay = (-(u - 0.12).max(0.0) * 4.5).exp();
            let e = attack * decay * k * size;
            tilt_torso(s, scale(f, -1.0), 10.0 * e);
            if let Some(h) = head {
                s.tilt_toward(h, scale(f, -1.0), 12.0 * e);
            }
            for arm in &body.arms {
                s.tilt_toward(arm.upper, scale(left, arm.side.sign()), 24.0 * e);
                s.tilt_toward(arm.lower, up, 28.0 * e);
            }
            if kind == Beat::React
                && let Some(t) = toward
            {
                let ramp_w = ramp(u, 0.05, 0.25) * (1.0 - ramp(u, 0.7, 0.3));
                look(s, body, t, [ramp_w, ramp_w * 0.9, ramp_w * 0.6], 0.7);
            }
        }
        Beat::Glance => {
            if let Some(t) = toward {
                let w = ramp(u, 0.0, 0.2) * (1.0 - ramp(u, 0.65, 0.3));
                let lag = |d: f32| ramp(u, d, 0.2) * (1.0 - ramp(u, 0.65 + d, 0.3));
                let _ = w;
                look(s, body, t, [lag(0.0), lag(0.06), lag(0.12)], 0.45);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person() -> (crate::Mesh, std::sync::Arc<Rig>, Body) {
        let m = crate::character::character(400.0, &crate::character::Build { detail: 1, ..Default::default() });
        let r = m.rig.clone().unwrap();
        let b = Body::of(&r);
        (m, r, b)
    }

    fn gaze(s: &mut Solver, body: &Body) -> V3 {
        let e = body.eyes.first().copied().or(body.head).unwrap();
        facing(s, body, e)
    }

    #[test]
    fn a_look_turns_the_eyes_then_the_head_then_the_chest() {
        let (_, rig, body) = person();
        // A point off to the left and a little up, at head height.
        let mut s0 = Solver::new(&rig);
        let from = eye_point(&mut s0, &body);
        let target = add(add(from, scale(body.fwd, 60.0)), add(scale(body.left(), 70.0), scale(body.up, 25.0)));
        let ramp_at = |t: f32| -> [f32; 3] { [ramp(t, 0.0, 0.25), ramp(t, 0.08, 0.35), ramp(t, 0.2, 0.5)] };
        // How far each has turned (about up) over time.
        let yaw_of = |s: &mut Solver, bone: usize| -> f32 { signed_angle(body.fwd, facing(s, &body, bone), body.up) };
        let (mut t_eyes, mut t_head, mut t_chest) = (f32::NAN, f32::NAN, f32::NAN);
        let mut final_yaw = [0.0; 3];
        for i in 0..=100 {
            let t = i as f32 * 0.01 * 1.0;
            let mut s = Solver::new(&rig);
            look(&mut s, &body, target, ramp_at(t), 0.7);
            let ys = [yaw_of(&mut s, body.eyes[0]), yaw_of(&mut s, body.head.unwrap()), yaw_of(&mut s, body.chest.unwrap())];
            final_yaw = ys;
            let half = |v: f32, full: f32| v.abs() >= 0.5 * full.abs();
            let _ = half;
            if t_eyes.is_nan() && ys[0].abs() > 8.0 {
                t_eyes = t;
            }
            if t_head.is_nan() && ys[1].abs() > 8.0 {
                t_head = t;
            }
            if t_chest.is_nan() && ys[2].abs() > 3.0 {
                t_chest = t;
            }
        }
        assert!(t_eyes < t_head && t_head < t_chest, "eyes {t_eyes} head {t_head} chest {t_chest}");
        assert!(final_yaw[1] > 10.0, "{final_yaw:?}");
        // At full weight the gaze is on the target.
        let mut s = Solver::new(&rig);
        let r = look(&mut s, &body, target, [1.0, 1.0, 1.0], 0.7);
        let from = eye_point(&mut s, &body);
        let want = normalise(sub(target, from));
        let got = gaze(&mut s, &body);
        let err = dot(want, got).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(err < 4.0, "gaze is {err}° off: {r:?}");
        assert!(r.short_by < 1.0);
        // Behind the character is more than the neck can do: it says so.
        let behind = add(from, scale(body.fwd, -100.0));
        let mut s = Solver::new(&rig);
        assert!(look(&mut s, &body, behind, [1.0; 3], 0.7).short_by > 20.0);
    }

    #[test]
    fn a_reach_never_stretches_a_bone_and_says_when_it_is_too_far() {
        let (_, rig, body) = person();
        let arm = body.arm(Side::L).unwrap().clone();
        let len = Body::limb_length(&rig, &arm);
        let mut s0 = Solver::new(&rig);
        let shoulder = s0.head(arm.upper);
        let rest_lengths = [rig.bones[arm.upper].length(), rig.bones[arm.lower].length()];
        for (dist, expect_short) in [(0.5, false), (0.8, false), (1.0, false), (1.4, true), (2.5, true)] {
            let target = add(add(shoulder, scale(body.fwd, len * dist * 0.9)), scale(body.up, len * dist * 0.3));
            let mut s = Solver::new(&rig);
            let info = reach(&mut s, &body, &rig, Side::L, target, 1.0, true).unwrap();
            let (u, m, e) = (s.head(arm.upper), s.head(arm.lower), s.head(arm.end));
            // The bones keep their length, whatever was asked.
            assert!((length(sub(m, u)) - rest_lengths[0]).abs() < 0.02 * rest_lengths[0], "upper arm stretched at {dist}");
            assert!((length(sub(e, m)) - rest_lengths[1]).abs() < 0.02 * rest_lengths[1], "forearm stretched at {dist}");
            assert_eq!(info.short_by > 0.0, expect_short, "{dist}: {info:?}");
            if !expect_short {
                assert!(length(sub(e, target)) < 0.03 * len + 1.0, "{dist}: the hand is {} px off", length(sub(e, target)));
            }
            // The elbow is below the line from shoulder to hand (natural), not above.
            let line_mid = scale(add(u, e), 0.5);
            assert!(dot(sub(m, line_mid), body.up) < 1.0, "elbow up at {dist}");
        }
        // A far target is reached by leaning in, up to a point.
        let far = add(shoulder, scale(body.fwd, len * 1.05));
        let mut s = Solver::new(&rig);
        let info = reach(&mut s, &body, &rig, Side::L, far, 1.0, true).unwrap();
        assert!(info.leaned > 0.0, "{info:?}");
    }

    #[test]
    fn sitting_crouching_and_leaning_keep_the_feet_where_they_are() {
        let (_, rig, body) = person();
        let mut s0 = Solver::new(&rig);
        let rest: Vec<V3> = body.legs.iter().map(|l| s0.head(l.end)).collect();
        for which in 0..3 {
            let mut s = Solver::new(&rig);
            match which {
                0 => sit(&mut s, &body, &rig, 1.0, None),
                1 => crouch(&mut s, &body, &rig, 0.8),
                _ => lean(&mut s, &body, &rig, body.fwd, 18.0),
            }
            for (l, r) in body.legs.iter().zip(&rest) {
                assert!(length(sub(s.head(l.end), *r)) < 1.5, "pose {which}: a foot moved");
            }
            let hips_now = dot(s.head(body.hips), body.up);
            if which < 2 {
                assert!(hips_now < dot(s0.head(body.hips), body.up) - 20.0, "pose {which}: the hips come down");
            }
        }
    }

    /// Thinned keys are played as smooth (Auto) keys, so every sample must
    /// sit within the tolerance of that curve, not of straight lines.
    #[test]
    fn thinned_keys_fit_the_curve_they_are_played_on() {
        let times: Vec<f32> = (0..=60).map(|i| i as f32 / 30.0).collect();
        // A straight ramp up, a hold, a sharp dip and a wobble.
        let vals: Vec<f32> = times.iter().map(|&t| if t < 0.5 { 20.0 * t } else if t < 1.0 { 10.0 } else if t < 1.2 { 10.0 - 40.0 * (t - 1.0) } else { 2.0 + (t * 9.0).sin() }).collect();
        let tol = 0.3;
        let keys = simplify(&times, &vals, tol);
        let us = |t: f32| (f64::from(t) * 1e6).round() as i64;
        let track = studio_model::Track::new(keys.iter().map(|&(t, v)| studio_model::Keyframe::new(us(t), f64::from(v), studio_model::Interp::Auto)).collect());
        for (&t, &v) in times.iter().zip(&vals) {
            let got = track.eval(us(t)).unwrap().as_f64().unwrap();
            assert!((got - f64::from(v)).abs() <= f64::from(tol) + 1e-4, "at {t}: played {got}, sampled {v}");
        }
        assert!(keys.len() < times.len() / 2, "{} keys", keys.len());
    }

    #[test]
    fn layers_keep_only_what_changes_and_simplify() {
        let (_, rig, body) = person();
        let times: Vec<f32> = (0..=30).map(|i| i as f32 / 30.0).collect();
        let base = |_t: f32| vec![BonePose::default(); rig.bones.len()];
        let channels = layer(&rig, &times, &base, &mut |s, t| beat(s, &body, Beat::Nod, t, 1.0, None), 0.3, 0.2);
        assert!(!channels.is_empty() && channels.len() < 12, "{} channels", channels.len());
        assert!(channels.iter().all(|c| c.bone == "head" || c.bone == "neck"), "{:?}", channels.iter().map(|c| &c.bone).collect::<Vec<_>>());
        assert!(channels.iter().all(|c| c.keys.len() < 30));
    }

    #[test]
    fn a_two_handed_recipe_brings_the_hands_together_without_stretching() {
        let (_, rig, body) = person();
        let mut s0 = Solver::new(&rig);
        let rest_gap = length(sub(s0.head(body.arm(Side::L).unwrap().end), s0.head(body.arm(Side::R).unwrap().end)));
        for kind in [Recipe::Strike, Recipe::CuppedHands] {
            let mut s = Solver::new(&rig);
            recipe(&mut s, &body, &rig, kind, Side::R, 0.7);
            let (l, r) = (body.arm(Side::L).unwrap().clone(), body.arm(Side::R).unwrap().clone());
            let gap = length(sub(s.head(l.end), s.head(r.end)));
            assert!(gap < rest_gap * 0.35, "{kind:?}: the hands are {gap:.0} apart (were {rest_gap:.0})");
            // In front of the chest.
            let front = dot(sub(s.head(l.end), s.head(body.hips)), body.fwd);
            assert!(front > 30.0, "{kind:?}: {front}");
            for arm in [l, r] {
                for (a, b) in [(arm.upper, arm.lower), (arm.lower, arm.end)] {
                    let len = rig.bones[a].length();
                    assert!((length(sub(s.head(b), s.head(a))) - len).abs() < 0.03 * len, "{kind:?}: a bone stretched");
                }
            }
        }
    }
}
