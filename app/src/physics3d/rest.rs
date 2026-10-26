// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Things where they physically can be.
//!
//! - [`rest_on_support`]: a model put in a scene comes down onto what is
//!   under it (a table, the floor, a shelf, the terrain) and up out of what
//!   it sinks into, keeping its turn.
//! - [`settle`]: models dropped for real until they stop (a chair that
//!   tips onto its legs, a pile that slumps), placed where they ended up.
//! - [`check`]: what in a still scene would not stay as it is: floating
//!   above what is under it, sunk into something, or balanced where it
//!   would tip over, each with how far and what fixes it.

use super::scene::{Build, Floor, Item, Role, Scene, frame_of, load_mesh, physics_tris};
use super::{Props, run};
use studio_model as m;
use studio_sim::dynamics::decompose::{self, Fit};
use studio_sim::dynamics::math::*;
use studio_sim::dynamics::{Body, Collider, Kind};

use super::{AIRBORNE, GROUNDS, has_word};

/// A clip as a dynamic body in a scene it is not part of.
fn as_body(scene: &mut Scene, c: &m::Clip, t: m::Micros) -> Option<(usize, Aabb)> {
    let mesh = load_mesh(c)?;
    let tr = c.transform_at((t - c.start).clamp(0, c.duration.max(0)));
    let tris = physics_tris(&mesh, &tr, scene.ppm);
    let props = Props::of(c);
    let fit = if props.fit == Fit::Mesh { Fit::Auto } else { props.fit };
    let shapes = decompose::shapes_for(&tris, fit, 24)?;
    let stuff = props.stuff(Some(&mesh), &c.name);
    let cols: Vec<Collider> = shapes.parts.into_iter().map(|(s, p)| Collider::new(s, p, &stuff)).collect();
    let body = scene.world.add(Body::new(Kind::Dynamic, cols, frame_of(&tr, scene.ppm)));
    scene.item_of.push(None);
    let b = super::scene::scene_bounds(&mesh, &tr);
    Some((body, b))
}

/// How far a body can move along `down` before it touches something, and
/// how deep it is in something now (both metres); None when nothing is
/// under it within `reach`.
fn gap_below(scene: &mut Scene, body: usize, down: V3, reach: f32) -> (Option<f32>, f32) {
    let mut moved = 0.0f32;
    let start = scene.world.bodies[body].pose;
    let mut overlap = 0.0f32;
    for round in 0..12 {
        let contacts = scene.world.contacts_of(body, (reach - moved).max(0.0) + 0.001);
        let mut step = f32::INFINITY;
        let mut deepest = 0.0f32;
        for (_, mf) in &contacts {
            let along = dot(mf.normal, down);
            if along < 0.25 {
                // Touching beside or above: only counts as overlap.
                for p in &mf.points {
                    deepest = deepest.max(-p.sep);
                }
                continue;
            }
            for p in &mf.points {
                deepest = deepest.max(-p.sep);
                step = step.min(p.sep / along);
            }
        }
        if round == 0 {
            overlap = deepest;
        }
        if !step.is_finite() {
            scene.world.bodies[body].pose = start;
            return (if moved > 0.0 { Some(moved) } else { None }, overlap);
        }
        if step.abs() < 1e-4 {
            break;
        }
        moved += step;
        if moved > reach {
            scene.world.bodies[body].pose = start;
            return (None, overlap);
        }
        let b = &mut scene.world.bodies[body];
        b.pose.p = mad(b.pose.p, down, step);
    }
    scene.world.bodies[body].pose = start;
    (Some(moved), overlap)
}

/// Where a clip should be to rest on what is under it at `t`: its new
/// position, or None when it already rests, or nothing is under it within
/// reach (a thing meant to hang in the air).
pub fn rest_on_support(project: &m::Project, seq: m::SequenceId, clip: u64, t: m::Micros) -> Option<m::Vec2> {
    let s = project.sequence(seq)?;
    let c = s.clip(m::ClipId(clip))?.clone();
    if has_word(&c.name, AIRBORNE) || Props::of(&c).body == super::BodyType::None {
        return None;
    }
    let mut b = Build::at(seq, t);
    b.skip = vec![clip];
    b.characters = false;
    b.floor = Floor::Off;
    let mut scene = Scene::build(project, &b).ok()?;
    if scene.items.is_empty() {
        return None;
    }
    let (body, bounds) = as_body(&mut scene, &c, t)?;
    let ppm = scene.ppm;
    let height = (bounds.hi[1] - bounds.lo[1]).max(1.0) / ppm;
    let down = normalise_or(scene.world.gravity, [0.0, 1.0, 0.0]);
    // Lift it out of anything it is sunk into first (up to its height).
    let lift = height * 1.05;
    scene.world.bodies[body].pose.p = mad(scene.world.bodies[body].pose.p, down, -lift);
    let (gap, _) = gap_below(&mut scene, body, down, lift + height * 1.5);
    let gap = gap?;
    let drop = gap - lift;
    // Already resting (within a hair): nothing to do.
    if drop.abs() * ppm < 0.75 {
        return None;
    }
    let tr = c.transform_at((t - c.start).clamp(0, c.duration.max(0)));
    Some(m::Vec2::new(tr.position.x + (down[0] * drop * ppm) as f64, tr.position.y + (down[1] * drop * ppm) as f64))
}

/// Drops clips for real at `t` until they stop (up to `seconds`): where
/// each ends up (position, rotation X Y Z), for those that moved.
pub fn settle(project: &m::Project, seq: m::SequenceId, clips: &[u64], t: m::Micros, seconds: f32) -> Result<Vec<(u64, [f64; 3], [f64; 3])>, String> {
    let mut b = Build::at(seq, t);
    b.dynamic = clips.to_vec();
    b.characters = true;
    let scene = Scene::build(project, &b)?;
    let mut r = run::Run::new(t, seconds, 30.0);
    r.stop_at_rest = true;
    r.react = false;
    let baked = run::simulate(project, scene, &r);
    Ok(baked.tracks.iter().filter_map(|tr| tr.samples.last().map(|s| (tr.clip, s.1, s.2))).collect())
}

/// Something in a still scene that would not stay put.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub clip: u64,
    pub name: String,
    /// floating, sunk or tips.
    pub kind: &'static str,
    /// px (floating, sunk) or degrees (tips).
    pub amount: f32,
    pub under: Option<String>,
}

/// What in the scene at `t` would fall, is sunk into something or would
/// tip over (props only: not grounds, characters, animated or airborne
/// things, or things marked static).
pub fn check(project: &m::Project, seq: m::SequenceId, t: m::Micros) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(s) = project.sequence(seq) else { return out };
    let mut b = Build::at(seq, t);
    b.characters = false;
    b.floor = Floor::Off;
    let Ok(base) = Scene::build(project, &b) else { return out };
    if base.items.len() < 2 {
        return out;
    }
    let lowest = base.items.iter().map(|i| i.bounds.hi[1]).fold(f32::NEG_INFINITY, f32::max);
    let candidates: Vec<&Item> = base
        .items
        .iter()
        .filter(|i| matches!(i.role, Role::Static))
        .filter(|i| !has_word(&i.name, GROUNDS) && !has_word(&i.name, AIRBORNE))
        .filter(|i| (lowest - i.bounds.hi[1]).abs() > 1.0 || base.items.iter().any(|o| o.clip != i.clip && o.bounds.hi[1] >= i.bounds.hi[1] - 1.0 && footprint(&o.bounds) > footprint(&i.bounds)))
        .filter(|i| s.clip(m::ClipId(i.clip)).is_some_and(|c| Props::of(c).body == super::BodyType::Auto || Props::of(c).body == super::BodyType::Dynamic))
        .collect();
    let ppm = base.ppm;
    for it in candidates.iter().take(40) {
        let Some(c) = s.clip(m::ClipId(it.clip)) else { continue };
        let mut b2 = Build::at(seq, t);
        b2.skip = vec![it.clip];
        b2.characters = false;
        b2.floor = Floor::Off;
        let Ok(mut scene) = Scene::build(project, &b2) else { continue };
        let Some((body, bounds)) = as_body(&mut scene, c, t) else { continue };
        let height = (bounds.hi[1] - bounds.lo[1]).max(1.0);
        let down = normalise_or(scene.world.gravity, [0.0, 1.0, 0.0]);
        let (gap, overlap) = gap_below(&mut scene, body, down, height * 3.0 / ppm);
        let tol = (height * 0.02).max(1.5);
        if overlap * ppm > tol.max(height * 0.05) {
            out.push(Finding { clip: it.clip, name: it.name.clone(), kind: "sunk", amount: overlap * ppm, under: None });
            continue;
        }
        let Some(gap) = gap else { continue };
        if gap * ppm > tol {
            out.push(Finding { clip: it.clip, name: it.name.clone(), kind: "floating", amount: gap * ppm, under: None });
            continue;
        }
        // Resting: does it stay, or tip?
        let start = scene.world.bodies[body].pose;
        scene.world.advance(1.5);
        let end = scene.world.bodies[body].pose;
        let turned = length(q_log(q_mul(end.q, q_conj(start.q)))).to_degrees();
        let fell = length(sub(end.p, start.p)) * ppm;
        if turned > 12.0 || fell > height * 0.25 {
            out.push(Finding { clip: it.clip, name: it.name.clone(), kind: "tips", amount: turned, under: None });
        }
    }
    out
}

fn footprint(b: &Aabb) -> f32 {
    (b.hi[0] - b.lo[0]).max(0.0) * (b.hi[2] - b.lo[2]).max(0.0)
}
