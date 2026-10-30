// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Things where they physically can be.
//!
//! - [`rest_on_support`]: a model put in a scene comes down onto what is
//!   under it (a table, the floor, a shelf, the terrain) and up out of what
//!   it sinks into, keeping its turn; one put inside or through something
//!   stays where it is.
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
fn gap_below(scene: &Scene, body: usize, down: V3, reach: f32) -> (Option<f32>, f32) {
    (scene.world.sweep(body, down, reach), scene.world.overlap(body, f32::INFINITY))
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
    // Sunk into something as deep as the check would call it sunk: lifted
    // out first (up to its height). Still in something up there, it was
    // put inside or through a thing, not on it, and stays where put.
    let tol = (height * ppm * 0.02).max(1.5) / ppm;
    let mut lift = 0.0;
    if scene.world.overlap(body, tol) > tol {
        lift = height * 1.05;
        scene.world.bodies[body].pose.p = mad(scene.world.bodies[body].pose.p, down, -lift);
        if scene.world.overlap(body, tol) > tol {
            return None;
        }
    }
    // It looks as far below as the check does for a floating thing: up to three heights.
    let gap = scene.world.sweep(body, down, lift + height * 3.0)?;
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

/// Answers of [`check`] by scene: the fingerprint of what is on screen at
/// that moment, and per model what it found (`None`: it stays put).
type Memo = std::sync::Mutex<std::collections::HashMap<u64, std::collections::HashMap<u64, Option<Finding>>>>;

#[cfg(test)]
thread_local! {
    /// Models [`check_one`] simulated on this thread, so a test can tell
    /// work done from work answered out of the memo.
    static SIMULATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many models the rest check has simulated on this thread so far.
#[cfg(test)]
pub(crate) fn simulated() -> usize {
    SIMULATED.with(|c| c.get())
}

fn memo() -> &'static Memo {
    static M: std::sync::OnceLock<Memo> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// What decides the rest check at `t`: every clip on screen then, as it
/// is saved (a model's file is named by its content), the project's
/// settings, and the moment. A change anywhere in it asks again.
fn fingerprint(project: &m::Project, s: &m::Sequence, t: m::Micros) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (s.id.0, t).hash(&mut h);
    serde_json::to_string(&project.settings).unwrap_or_default().hash(&mut h);
    for c in s.clips.iter().filter(|c| c.start <= t && t < c.end()) {
        serde_json::to_string(c).unwrap_or_default().hash(&mut h);
    }
    for l in &s.lanes {
        (l.id.0, l.hidden).hash(&mut h);
    }
    h.finish()
}

/// How long [`check`] may spend on models it has not answered for yet:
/// `STUDIO_REST_CHECK_MS`, or 4 s. What is left is answered by the next
/// call (answers are kept), and [`check_within`] says how many.
pub(crate) fn budget() -> std::time::Duration {
    std::time::Duration::from_millis(std::env::var("STUDIO_REST_CHECK_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000))
}

/// What in the scene at `t` would fall, is sunk into something or would
/// tip over (props only: not grounds, characters, animated or airborne
/// things, or things marked static).
pub fn check(project: &m::Project, seq: m::SequenceId, t: m::Micros) -> Vec<Finding> {
    check_within(project, seq, t, budget()).0
}

/// [`check`] spending at most `budget` on models not answered before, and
/// how many models it had no time left for.
pub fn check_within(project: &m::Project, seq: m::SequenceId, t: m::Micros, budget: std::time::Duration) -> (Vec<Finding>, usize) {
    let Some(s) = project.sequence(seq) else { return (Vec::new(), 0) };
    let fp = fingerprint(project, s, t);
    let known = memo().lock().unwrap_or_else(|e| e.into_inner()).get(&fp).cloned().unwrap_or_default();
    // Answered before for this very scene: nothing to build or simulate.
    if known.contains_key(&u64::MAX) {
        let mut out: Vec<Finding> = known.into_iter().filter_map(|(_, f)| f).collect();
        out.sort_by_key(|f| f.clip);
        return (out, 0);
    }
    let deadline = std::time::Instant::now() + budget;
    let (out, answers, left) = check_fresh(project, s, seq, t, &known, deadline);
    let mut g = memo().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 64 {
        g.clear();
    }
    let e = g.entry(fp).or_default();
    e.extend(answers);
    if left == 0 {
        // Complete: marked so the next call answers at once.
        e.insert(u64::MAX, None);
    }
    (out, left)
}

/// The rest check proper, reusing `known` answers: findings in clip order,
/// the answers it worked out, and how many models were left when
/// `deadline` passed.
fn check_fresh(project: &m::Project, s: &m::Sequence, seq: m::SequenceId, t: m::Micros, known: &std::collections::HashMap<u64, Option<Finding>>, deadline: std::time::Instant) -> (Vec<Finding>, Vec<(u64, Option<Finding>)>, usize) {
    let mut answers = Vec::new();
    let mut left = 0;
    let mut out = check_scene(project, s, seq, t, known, &mut answers, &mut left, deadline);
    out.sort_by_key(|f| f.clip);
    (out, answers, left)
}

#[allow(clippy::too_many_arguments)]
fn check_scene(project: &m::Project, s: &m::Sequence, seq: m::SequenceId, t: m::Micros, known: &std::collections::HashMap<u64, Option<Finding>>, answers: &mut Vec<(u64, Option<Finding>)>, left: &mut usize, deadline: std::time::Instant) -> Vec<Finding> {
    let mut out = Vec::new();
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
    let candidates: Vec<&&Item> = candidates.iter().take(40).collect();
    for (k, it) in candidates.iter().enumerate() {
        if let Some(f) = known.get(&it.clip) {
            out.extend(f.clone());
            continue;
        }
        if std::time::Instant::now() > deadline {
            *left = candidates[k..].iter().filter(|i| !known.contains_key(&i.clip)).count();
            break;
        }
        let found = out.len();
        let answered = check_one(project, s, seq, t, it, ppm, &mut out);
        if answered {
            answers.push((it.clip, out.get(found).cloned()));
        }
    }
    out
}

/// Whether `it` would float, sink or tip, onto `out`; false when it could
/// not be asked (no body for it).
#[allow(clippy::too_many_arguments)]
fn check_one(project: &m::Project, s: &m::Sequence, seq: m::SequenceId, t: m::Micros, it: &Item, ppm: f32, out: &mut Vec<Finding>) -> bool {
    {
        let Some(c) = s.clip(m::ClipId(it.clip)) else { return false };
        #[cfg(test)]
        SIMULATED.with(|n| n.set(n.get() + 1));
        let mut b2 = Build::at(seq, t);
        b2.skip = vec![it.clip];
        b2.characters = false;
        b2.floor = Floor::Off;
        let Ok(mut scene) = Scene::build(project, &b2) else { return false };
        let Some((body, bounds)) = as_body(&mut scene, c, t) else { return true };
        let height = (bounds.hi[1] - bounds.lo[1]).max(1.0);
        let down = normalise_or(scene.world.gravity, [0.0, 1.0, 0.0]);
        let (gap, overlap) = gap_below(&scene, body, down, height * 3.0 / ppm);
        let tol = (height * 0.02).max(1.5);
        if overlap * ppm > tol.max(height * 0.05) {
            out.push(Finding { clip: it.clip, name: it.name.clone(), kind: "sunk", amount: overlap * ppm, under: None });
            return true;
        }
        let Some(gap) = gap else { return true };
        if gap * ppm > tol {
            out.push(Finding { clip: it.clip, name: it.name.clone(), kind: "floating", amount: gap * ppm, under: None });
            return true;
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
    true
}

fn footprint(b: &Aabb) -> f32 {
    (b.hi[0] - b.lo[0]).max(0.0) * (b.hi[2] - b.lo[2]).max(0.0)
}
