// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Walks of different characters: planted feet, stops, turns, steps, and
//! the checks that tell a good walk from a sliding one.

use super::body::{Body, Kind};
use super::check::{self, Sample};
use super::gait::{self, Gait, Plan, Style, Walk, Waypoint};
use super::query::{ClipXf, points};
use super::sheet;
use crate::character::{Build, character};
use crate::rig::{BonePose, Rig};
use crate::{V3, dot, sub};
use std::sync::Arc;

pub struct Cast {
    pub name: &'static str,
    pub rig: Arc<Rig>,
    pub body: Body,
    pub k: f32,
}

/// A varied cast: a grown person, a child, a heavy old man, a dancer, a
/// quadruped.
pub fn cast() -> Vec<Cast> {
    let mut out = Vec::new();
    let mut add = |name: &'static str, height: f32, b: Build, k: f32| {
        let m = character(height, &Build { detail: 1, ..b });
        let rig = m.rig.clone().unwrap();
        let body = Body::of(&rig);
        out.push(Cast { name, rig, body, k });
    };
    add("adult", 400.0, Build::default(), 0.5);
    add("child", 260.0, Build { head: 1.4, weight: 0.85, ..Build::default() }, 0.5);
    add("old man", 380.0, Build { weight: 1.2, shoulders: 0.9, ..Build::default() }, 0.5);
    add("dancer", 420.0, Build { weight: 0.85, ..Build::default() }, 0.5);
    // A four-legged animal along x, head toward -x.
    let mut mesh = crate::prim::cube([1.0, 1.0, 1.0]);
    for p in &mut mesh.positions {
        *p = [p[0] * 300.0, p[1] * 160.0, p[2] * 80.0];
    }
    let rig = Arc::new(crate::autorig::quadruped(&mesh));
    let body = Body::of(&rig);
    out.push(Cast { name: "quadruped", rig, body, k: 0.5 });
    out
}

pub fn plan(points: &[[f32; 2]]) -> Plan {
    Plan {
        start: [0.0, 0.0],
        heading: 0.0,
        waypoints: points.iter().enumerate().map(|(i, p)| Waypoint { x: p[0], z: p[1], stop: i + 1 == points.len(), pause: 0.0, face: None }).collect(),
        speed: None,
        duration: None,
        hold: 0.5,
        lead: 0.2,
    }
}

pub fn flat(_x: f32, _z: f32) -> f32 {
    0.0
}

pub fn samples(rig: &Rig, w: &Walk, k: f32) -> Vec<Sample> {
    w.frames.iter().map(|f| Sample { t: f.t, xf: ClipXf { pos: [f.x, -f.ground, f.z], yaw_deg: f.yaw_deg, scale: k }, world: rig.forward(&f.poses) }).collect()
}

fn ankle_world(rig: &Rig, body: &Body, w: &Walk, k: f32, leg: usize) -> Vec<V3> {
    samples(rig, w, k).iter().map(|s| s.xf.world(s.world[body.legs[leg].end].pos)).collect()
}

fn horizontal(a: V3, b: V3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn walk_of(c: &Cast, name: &str, style: &Style, route: &[[f32; 2]]) -> (Gait, Walk) {
    let g = gait::gait(name, &c.body, style).unwrap();
    let w = gait::walk(&c.rig, &c.body, &g, c.k, &plan(route), 30.0, &flat).unwrap();
    (g, w)
}

#[test]
fn a_walk_along_a_path_has_zero_foot_slide_while_in_contact() {
    for c in cast() {
        let style = if c.name == "old man" { Style { weary: 0.7, heavy: 0.5, ..Style::default() } } else { Style::default() };
        let (_, w) = walk_of(&c, "walk", &style, &[[0.0, -700.0], [500.0, -1100.0]]);
        assert!(w.steps >= 6, "{}: {} steps", c.name, w.steps);
        // Planted feet stay where they are, frame after frame.
        for leg in 0..c.body.legs.len() {
            let a = ankle_world(&c.rig, &c.body, &w, c.k, leg);
            let mut worst = 0.0f32;
            let mut planted_pairs = 0;
            for i in 1..a.len() {
                if w.frames[i].planted[leg] && w.frames[i - 1].planted[leg] {
                    planted_pairs += 1;
                    worst = worst.max(horizontal(a[i], a[i - 1]));
                }
            }
            assert!(planted_pairs > 10, "{} leg {leg}: planted for {planted_pairs} frames", c.name);
            assert!(worst < 0.35, "{} leg {leg}: a planted foot moves {worst:.2} px in a frame", c.name);
        }
        // The independent check agrees: nothing slides, crosses, stretches or breaks.
        let found = check::analyse(&c.rig, &c.body, &samples(&c.rig, &w, c.k), &[]);
        assert!(found.is_empty(), "{}: {:#?}", c.name, found.iter().map(|f| (f.kind, f.t0, f.t1, f.detail.clone())).collect::<Vec<_>>());
    }
}

#[test]
fn speed_matches_stride_and_the_body_arrives_and_stops() {
    let c = &cast()[0];
    let (g, w) = walk_of(c, "walk", &Style::default(), &[[0.0, -1500.0]]);
    // The body ends where it was sent, still.
    let last = w.frames.last().unwrap();
    assert!(horizontal([last.x, 0.0, last.z], [0.0, 0.0, -1500.0]) < 3.0, "ends at ({}, {})", last.x, last.z);
    let n = w.frames.len();
    assert!(horizontal([w.frames[n - 1].x, 0.0, w.frames[n - 1].z], [w.frames[n - 6].x, 0.0, w.frames[n - 6].z]) < 0.5, "still at the end");
    // Cruise speed: the middle of the walk is at the gait's pace.
    let mid = n / 2;
    let v = horizontal([w.frames[mid + 5].x, 0.0, w.frames[mid + 5].z], [w.frames[mid - 5].x, 0.0, w.frames[mid - 5].z]) / (10.0 / 30.0);
    // Stride: the distance between one foot's plants is the speed times the cycle.
    let a = ankle_world(&c.rig, &c.body, &w, c.k, 0);
    let mut plants: Vec<(usize, V3)> = Vec::new();
    for i in 1..a.len() {
        if w.frames[i].planted[0] && !w.frames[i - 1].planted[0] {
            plants.push((i, a[i + 2.min(a.len() - 1 - i)]));
        }
    }
    assert!(plants.len() >= 4, "{} plants", plants.len());
    let mid_plants: Vec<_> = plants.iter().skip(1).take(plants.len().saturating_sub(2)).collect();
    let (p0, p1) = (mid_plants[0], mid_plants[1]);
    let stride = horizontal(p0.1, p1.1);
    let cycle_secs = (p1.0 - p0.0) as f32 / 30.0;
    assert!((stride / cycle_secs - v).abs() < 0.2 * v, "stride {stride:.0} px per {cycle_secs:.2} s = {:.0} px/s but the body goes {v:.0} px/s", stride / cycle_secs);
    assert!(g.speed > 0.0);
}

#[test]
fn the_stop_has_the_feet_apart_not_crossed() {
    for c in cast().iter().filter(|c| c.body.kind == Kind::Biped) {
        for route in [vec![[0.0, -600.0]], vec![[0.0, -300.0], [400.0, -300.0]], vec![[-300.0, -500.0]]] {
            let (_, w) = walk_of(c, "walk", &Style::default(), &route);
            let s = samples(&c.rig, &w, c.k);
            let end = s.last().unwrap();
            let p = points(&c.rig, &c.body, &end.world, &end.xf, end.t);
            let left = crate::normalise(end.xf.world_dir(c.body.left()));
            let sep = dot(sub(p.feet[0].ankle, p.feet[1].ankle), left);
            // At rest they stand this far apart.
            let rest = Rig::forward(&c.rig, &vec![BonePose::default(); c.rig.bones.len()]);
            let rp = points(&c.rig, &c.body, &rest, &ClipXf { pos: [0.0; 3], yaw_deg: 0.0, scale: c.k }, 0.0);
            let rest_sep = dot(sub(rp.feet[0].ankle, rp.feet[1].ankle), c.body.left()).abs();
            assert!(sep > 0.6 * rest_sep, "{} {route:?}: feet {sep:.0} px apart (standing {rest_sep:.0})", c.name);
            assert!(sep < 1.6 * rest_sep, "{} {route:?}: feet {sep:.0} px apart (standing {rest_sep:.0})", c.name);
            // Side by side: neither well ahead of the other.
            let fwd = crate::normalise(end.xf.world_dir(c.body.fwd));
            let stagger = dot(sub(p.feet[0].ankle, p.feet[1].ankle), fwd).abs();
            assert!(stagger < 0.12 * 2.0 * rest_sep.max(30.0) + 10.0, "{} {route:?}: one foot {stagger:.0} px ahead", c.name);
            assert!(p.feet.iter().all(|f| f.speed < 1.0), "still");
            let found = check::analyse(&c.rig, &c.body, &s, &[]);
            assert!(found.iter().all(|f| f.kind != "char_feet_cross"), "{} {route:?}: {:?}", c.name, found.iter().map(|f| f.detail.clone()).collect::<Vec<_>>());
        }
    }
}

#[test]
fn styles_change_the_walk_and_other_gaits_plant_their_feet_too() {
    let c = &cast()[0];
    let plain = walk_of(c, "walk", &Style::default(), &[[0.0, -900.0]]);
    let weary = walk_of(c, "walk", &Style { weary: 1.0, ..Style::default() }, &[[0.0, -900.0]]);
    let brisk = walk_of(c, "walk", &Style { brisk: 1.0, ..Style::default() }, &[[0.0, -900.0]]);
    assert!(weary.1.duration > plain.1.duration * 1.1, "weary {} vs {}", weary.1.duration, plain.1.duration);
    assert!(brisk.1.duration < plain.1.duration * 0.92, "brisk {} vs {}", brisk.1.duration, plain.1.duration);
    for name in ["run", "jog", "sneak", "limp", "march", "trudge", "stroll", "tiptoe"] {
        let (_, w) = walk_of(c, name, &Style::default(), &[[0.0, -900.0], [300.0, -1200.0]]);
        let found = check::analyse(&c.rig, &c.body, &samples(&c.rig, &w, c.k), &[]);
        assert!(found.is_empty(), "{name}: {:?}", found.iter().map(|f| (f.kind, f.t0, f.detail.clone())).collect::<Vec<_>>());
    }
    // A run is faster than a walk, a sneak slower.
    let run = walk_of(c, "run", &Style::default(), &[[0.0, -900.0]]).1.duration;
    let sneak = walk_of(c, "sneak", &Style::default(), &[[0.0, -900.0]]).1.duration;
    assert!(run < plain.1.duration * 0.85 && sneak > plain.1.duration * 1.3, "run {run} walk {} sneak {sneak}", plain.1.duration);
    // A given duration is honoured.
    let g = gait::gait("walk", &c.body, &Style::default()).unwrap();
    let mut p = plan(&[[0.0, -900.0]]);
    p.duration = Some(9.0);
    let w = gait::walk(&c.rig, &c.body, &g, c.k, &p, 30.0, &flat).unwrap();
    assert!((w.duration - 9.0).abs() < 1.2, "{}", w.duration);
}

#[test]
fn a_quadruped_trots_on_four_planted_feet() {
    let c = cast().into_iter().find(|c| c.name == "quadruped").unwrap();
    assert_eq!(c.body.legs.len(), 4);
    for name in ["walk", "trot", "gallop"] {
        let (_, w) = walk_of(&c, name, &Style::default(), &[[-800.0, 0.0]]);
        assert!(w.steps >= 6, "{name}: {} steps", w.steps);
        let found = check::analyse(&c.rig, &c.body, &samples(&c.rig, &w, c.k), &[]);
        assert!(found.iter().all(|f| f.kind != "char_foot_slide"), "{name}: {:?}", found.iter().map(|f| (f.kind, f.t0, f.detail.clone())).collect::<Vec<_>>());
        for leg in 0..4 {
            let a = ankle_world(&c.rig, &c.body, &w, c.k, leg);
            let worst = (1..a.len()).filter(|&i| w.frames[i].planted[leg] && w.frames[i - 1].planted[leg]).map(|i| horizontal(a[i], a[i - 1])).fold(0.0, f32::max);
            assert!(worst < 0.4, "{name} leg {leg}: planted foot moves {worst}");
        }
    }
    // It faces the way it goes: the head ends up toward -z (we sent it -x, so the clip turns).
    let (_, w) = walk_of(&c, "trot", &Style::default(), &[[-800.0, 0.0]]);
    let s = samples(&c.rig, &w, c.k);
    let end = s.last().unwrap();
    let f = crate::normalise(end.xf.world_dir(c.body.fwd));
    assert!(f[0] < -0.95, "{f:?}");
}

#[test]
fn it_turns_on_the_spot_and_climbs_steps() {
    let c = &cast()[0];
    let g = gait::gait("walk", &c.body, &Style::default()).unwrap();
    // A turn: stand, face 100 degrees to the left, feet stepping round.
    let mut p = plan(&[[0.0, 0.0]]);
    p.waypoints[0].face = Some(100f32.to_radians());
    let w = gait::walk(&c.rig, &c.body, &g, c.k, &p, 30.0, &flat).unwrap();
    let s = samples(&c.rig, &w, c.k);
    let end = s.last().unwrap();
    assert!((end.xf.yaw_deg - 100.0).abs() < 2.0, "{}", end.xf.yaw_deg);
    assert!(w.steps >= 2, "{} steps", w.steps);
    let found = check::analyse(&c.rig, &c.body, &s, &[]);
    assert!(found.is_empty(), "{:?}", found.iter().map(|f| (f.kind, f.t0, f.detail.clone())).collect::<Vec<_>>());
    // Steps up, then down: each foot stands on the height of its place.
    let stairs = |x: f32, z: f32| -> f32 {
        let _ = x;
        let d = (-z - 300.0).max(0.0);
        (d / 60.0).floor().min(4.0) * 14.0
    };
    let w = gait::walk(&c.rig, &c.body, &g, c.k, &plan(&[[0.0, -1000.0]]), 30.0, &stairs).unwrap();
    let rise = w.frames.iter().map(|f| f.ground).fold(0.0, f32::max);
    assert!(rise > 40.0, "the body rises {rise}");
    let s = samples(&c.rig, &w, c.k);
    // Feet on a step stand at its height: the ankle is its rest height above the step.
    let ky = c.k * super::solve::Solver::new(&c.rig).head(c.body.legs[0].end)[1];
    let mut checked = 0;
    for (i, f) in w.frames.iter().enumerate() {
        for leg in 0..2 {
            if f.planted[leg] {
                let a = s[i].xf.world(s[i].world[c.body.legs[leg].end].pos);
                let want = stairs(a[0], a[2]);
                assert!((ky - a[1] - want).abs() < 1.5, "frame {i} leg {leg}: the foot is {:.1} up but the step is {want}", ky - a[1]);
                checked += 1;
            }
        }
    }
    assert!(checked > 40);
    let found = check::analyse(&c.rig, &c.body, &s, &[]);
    assert!(found.iter().all(|f| f.kind != "char_limb_stretched" && f.kind != "char_joint_detached"), "{:?}", found.iter().map(|f| (f.kind, f.t0, f.detail.clone())).collect::<Vec<_>>());
}

#[test]
fn the_critic_flags_a_sliding_walk_and_passes_a_clean_one() {
    let c = &cast()[0];
    let (_, clean) = walk_of(c, "walk", &Style::default(), &[[0.0, -900.0]]);
    assert!(check::analyse(&c.rig, &c.body, &samples(&c.rig, &clean, c.k), &[]).is_empty());
    // The same legs with the body gliding at a different pace: the feet slide.
    let mut slid = samples(&c.rig, &clean, c.k);
    let n = slid.len();
    for (i, s) in slid.iter_mut().enumerate() {
        let u = i as f32 / (n - 1) as f32;
        // Travel 1.5 times as far as the steps carry the body, evenly.
        s.xf.pos[2] = -900.0 * 1.5 * u;
        s.xf.pos[0] = 0.0;
    }
    let found = check::analyse(&c.rig, &c.body, &slid, &[]);
    let sl: Vec<_> = found.iter().filter(|f| f.kind == "char_foot_slide").collect();
    assert!(!sl.is_empty(), "no slide found: {:?}", found.iter().map(|f| f.kind).collect::<Vec<_>>());
    assert!(sl[0].fix.contains("act") && sl[0].detail.contains("px"), "{}", sl[0].detail);
    // A hand-keyed sine gait with the body keyed apart: feet slide and cross.
    let mut hand = clean.clone();
    for f in &mut hand.frames {
        let t = f.t;
        let mut poses = vec![BonePose::default(); c.rig.bones.len()];
        for (name, sign) in [("thigh.L", 1.0f32), ("thigh.R", -1.0)] {
            let i = c.rig.bone(name).unwrap();
            poses[i].rot = crate::rig::q_from_euler([sign * 30.0 * (t * 6.0).sin(), 0.0, 0.0]);
        }
        f.poses = poses;
        f.z = -150.0 * t;
        f.x = 0.0;
        f.yaw_deg = 0.0;
        f.ground = 0.0;
    }
    let found = check::analyse(&c.rig, &c.body, &samples(&c.rig, &hand, c.k), &[]);
    assert!(found.iter().any(|f| f.kind == "char_foot_slide"), "{:?}", found.iter().map(|f| f.kind).collect::<Vec<_>>());
    // Every finding names the fix.
    assert!(found.iter().all(|f| !f.fix.is_empty() && !f.detail.is_empty()));
}

#[test]
fn the_critic_finds_stretched_torn_and_backward_limbs_and_a_wandering_gaze() {
    let c = &cast()[0];
    let rest = vec![BonePose::default(); c.rig.bones.len()];
    let mk = |poses: Vec<BonePose>| -> Vec<Sample> {
        (0..12).map(|i| Sample { t: i as f32 * 0.1, xf: ClipXf { pos: [0.0; 3], yaw_deg: 0.0, scale: c.k }, world: c.rig.forward(&poses) }).collect()
    };
    assert!(check::analyse(&c.rig, &c.body, &mk(rest.clone()), &[]).is_empty(), "the rest pose is clean");
    // Forearm pushed out along its own length.
    let mut p = rest.clone();
    let fa = c.rig.bone("forearm.L").unwrap();
    p[fa].loc = [0.0, 40.0, 0.0];
    let f = check::analyse(&c.rig, &c.body, &mk(p), &[]);
    assert!(f.iter().any(|f| f.kind == "char_joint_detached"), "{:?}", f.iter().map(|f| f.kind).collect::<Vec<_>>());
    // A knee bent backward.
    let mut p = rest.clone();
    p[c.rig.bone("shin.L").unwrap()].rot = crate::rig::q_from_euler([60.0, 0.0, 0.0]);
    let f1 = check::analyse(&c.rig, &c.body, &mk(p), &[]);
    let mut p = rest.clone();
    p[c.rig.bone("shin.L").unwrap()].rot = crate::rig::q_from_euler([-60.0, 0.0, 0.0]);
    let f2 = check::analyse(&c.rig, &c.body, &mk(p), &[]);
    assert!(f1.iter().any(|f| f.kind == "char_joint_broken") != f2.iter().any(|f| f.kind == "char_joint_broken"), "exactly one direction is wrong: {:?} / {:?}", f1.iter().map(|f| &f.detail).collect::<Vec<_>>(), f2.iter().map(|f| &f.detail).collect::<Vec<_>>());
    // Not looking: told to look far to the side, the head stays forward.
    let samples = mk(rest);
    let target = {
        let s = &samples[0];
        let p = points(&c.rig, &c.body, &s.world, &s.xf, 0.0);
        let to = crate::add(p.eyes, crate::scale(s.xf.world_dir(c.body.left()), 400.0));
        to
    };
    let f = check::analyse(&c.rig, &c.body, &samples, &[check::Intent { t0: 0.0, t1: 1.1, target }]);
    let g = f.iter().find(|f| f.kind == "char_not_looking").expect("a gaze that is off");
    assert!(g.fix.contains("look_at"), "{}", g.fix);
}



/// Writes contact sheets when CHARMOVE_SHEETS names a folder.
#[test]
fn contact_sheets_of_walks_and_stops() {
    let Ok(dir) = std::env::var("CHARMOVE_SHEETS") else { return };
    let dir = std::path::PathBuf::from(dir);
    let c = &cast()[0];
    let m = character(400.0, &Build { detail: 2, ..Build::default() });
    let rig = m.rig.clone().unwrap();
    let tiles_of = |w: &Walk, c: &Cast, every: usize| -> Vec<sheet::Tile> {
        let s = samples(&c.rig, w, c.k);
        (0..w.frames.len())
            .step_by(every)
            .map(|i| {
                let planted: Vec<V3> = (0..c.body.legs.len()).filter(|&l| w.frames[i].planted[l]).map(|l| s[i].xf.world(s[i].world[c.body.legs[l].end].pos)).collect();
                sheet::Tile { xf: s[i].xf, poses: w.frames[i].poses.clone(), planted, label: format!("{:.2}s", w.frames[i].t) }
            })
            .collect()
    };
    let (_, w) = walk_of(c, "walk", &Style::default(), &[[0.0, -520.0]]);
    let n = w.frames.len();
    let tiles = tiles_of(&w, c, (n / 18).max(1));
    sheet::sheet(&dir.join("walk_and_stop.png"), Some(&m), &rig, &c.body, &tiles, 6, (170, 260), 1.0);
    let q = cast().into_iter().find(|c| c.name == "quadruped").unwrap();
    let (_, w) = walk_of(&q, "trot", &Style::default(), &[[-600.0, 0.0]]);
    let n = w.frames.len();
    let tiles = tiles_of(&w, &q, (n / 12).max(1));
    sheet::sheet(&dir.join("quadruped_trot.png"), None, &q.rig, &q.body, &tiles, 6, (250, 200), 1.0);
    // A strike from the general two-handed recipe: hands meet, one sweeps across the other, the eyes go to them.
    let body = &c.body;
    let times: Vec<f32> = (0..=11).map(|i| i as f32 / 11.0).collect();
    let tiles: Vec<sheet::Tile> = times
        .iter()
        .map(|&u| {
            let mut s = super::solve::Solver::new(&rig);
            super::pose::recipe(&mut s, body, &rig_body_arc(&rig), super::pose::Recipe::Strike, super::body::Side::R, u);
            let at = super::pose::recipe_gaze(&mut super::solve::Solver::new(&rig), body, &rig);
            let e = super::pose::ramp(u, 0.0, 0.25);
            super::pose::look(&mut s, body, at, [e, e * 0.9, e * 0.6], 0.8);
            sheet::Tile { xf: ClipXf { pos: [0.0, 0.0, 0.0], yaw_deg: 0.0, scale: 1.0 }, poses: s.poses(), planted: Vec::new(), label: format!("{u:.2}") }
        })
        .collect();
    sheet::sheet(&dir.join("match_strike.png"), Some(&m), &rig, body, &tiles, 6, (190, 260), 0.6);
}

fn rig_body_arc(r: &Arc<Rig>) -> Rig {
    (**r).clone()
}
