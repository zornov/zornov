// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Secondary motion: the parts that lag, swing and settle after the body
//! moves.
//!
//! - [`Chains`]: bone chains that follow through: a tail, ears, antennae,
//!   a ponytail, coat tails, a tie, a trunk, feathers on bones. Each bone's
//!   tip is a particle pulled towards where the animation puts it
//!   (stiffness), carried by its own momentum (the lag), pulled down by
//!   gravity, slowed by drag, kept at the bone's length and pushed out of
//!   the body's capsules so a tail does not swing through a leg.
//! - [`Jiggle`]: any mesh's vertices on damped springs behind where the
//!   skin or animation puts them, by how loose each one is (a belly, cheeks,
//!   fur tips, the feathers of a bird with no bones for them).
//!
//! Both run in the world's units (metres) at a fixed step and are
//! deterministic.

use super::math::*;
use studio_mesh::rig::{Rig, Xf};

/// Words in bone names that mark parts that swing.
pub const LOOSE_WORDS: &[&str] = &[
    "tail", "ear", "antenna", "hair", "ponytail", "braid", "pigtail", "tentacle", "feather", "plume", "crest", "coat", "cape", "skirt", "tie", "scarf", "trunk", "whisker", "fin", "jiggle", "belly", "breast", "cheek", "wattle", "comb", "mane", "tassel", "chain", "rope", "flap", "strap", "ribbon", "tongue", "fringe", "frond", "leaf", "vine", "spring", "bob", "dangle", "loose",
];

/// Whether a bone's name says it swings.
pub fn is_loose(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // "earring" swings; "hearth" is not an ear: whole words or prefixes.
    n.split(|c: char| !c.is_ascii_alphabetic()).any(|w| LOOSE_WORDS.iter().any(|l| w == *l || (w.starts_with(l) && w.len() <= l.len() + 2)))
}

/// Chains of loose bones in a rig, each listed root to tip: runs of loose
/// bones, each starting at a loose bone whose parent is not.
pub fn find_chains(rig: &Rig, only: &[String]) -> Vec<Vec<usize>> {
    let n = rig.bones.len();
    let picked: Vec<bool> = (0..n)
        .map(|i| {
            let b = &rig.bones[i];
            if only.is_empty() {
                is_loose(&b.name)
            } else {
                only.iter().any(|o| {
                    let o = o.to_ascii_lowercase();
                    let name = b.name.to_ascii_lowercase();
                    name == o || name.starts_with(&format!("{o}.")) || name.starts_with(&format!("{o}_")) || rig.bone(&o) == Some(i)
                })
            }
        })
        .collect();
    let mut chains = Vec::new();
    for i in 0..n {
        if !picked[i] || rig.bones[i].parent.is_some_and(|p| picked[p]) {
            continue;
        }
        // Down the line: the child that is also loose (the longest run).
        let mut chain = vec![i];
        let mut cur = i;
        loop {
            let kids: Vec<usize> = rig.children(cur).into_iter().filter(|&c| picked[c]).collect();
            if kids.is_empty() {
                break;
            }
            cur = kids[0];
            chain.push(cur);
            // Side branches start chains of their own.
            for &k in &kids[1..] {
                let mut side = vec![k];
                let mut c2 = k;
                while let Some(&next) = rig.children(c2).iter().find(|&&c| picked[c]) {
                    side.push(next);
                    c2 = next;
                }
                chains.push(side);
            }
        }
        chains.push(chain);
    }
    chains.sort();
    chains
}

/// How a chain moves.
#[derive(Clone, Copy, Debug)]
pub struct Feel {
    /// 0 floppy .. 1 holds the animated shape.
    pub stiffness: f32,
    /// 0 swings on .. 1 dead.
    pub damping: f32,
    pub gravity: f32,
    /// Air drag (1/s).
    pub drag: f32,
}

impl Feel {
    pub fn for_bone(name: &str) -> Feel {
        let n = name.to_ascii_lowercase();
        let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
        if has(&["antenna", "whisker"]) {
            Feel { stiffness: 0.35, damping: 0.08, gravity: 0.1, drag: 1.0 }
        } else if has(&["ear"]) {
            Feel { stiffness: 0.45, damping: 0.15, gravity: 0.3, drag: 1.5 }
        } else if has(&["tail", "trunk", "tentacle"]) {
            Feel { stiffness: 0.25, damping: 0.1, gravity: 0.4, drag: 1.0 }
        } else if has(&["belly", "breast", "cheek", "jiggle", "wattle"]) {
            Feel { stiffness: 0.55, damping: 0.25, gravity: 0.2, drag: 0.5 }
        } else if has(&["feather", "plume", "crest", "comb"]) {
            Feel { stiffness: 0.5, damping: 0.2, gravity: 0.15, drag: 3.0 }
        } else {
            // Hair, coats, ties, ropes, ribbons.
            Feel { stiffness: 0.12, damping: 0.08, gravity: 1.0, drag: 2.0 }
        }
    }
}

/// A sphere or capsule the chains stay out of (a limb, the head).
#[derive(Clone, Copy, Debug)]
pub struct Blocker {
    pub a: V3,
    pub b: V3,
    pub r: f32,
}

struct Chain {
    bones: Vec<usize>,
    /// Tip of each bone: now, before.
    pos: Vec<V3>,
    prev: Vec<V3>,
    feel: Feel,
    radius: f32,
}

/// Bone chains on a rig, simulated in the world.
pub struct Chains {
    chains: Vec<Chain>,
    started: bool,
}

impl Chains {
    pub fn new(rig: &Rig, chains: Vec<Vec<usize>>, feel: Option<Feel>, k: f32) -> Chains {
        let chains = chains
            .into_iter()
            .filter(|c| !c.is_empty())
            .map(|bones| {
                let f = feel.unwrap_or_else(|| Feel::for_bone(&rig.bones[bones[0]].name));
                let len = bones.iter().map(|&b| rig.bones[b].length()).sum::<f32>() / bones.len() as f32;
                Chain { pos: vec![[0.0; 3]; bones.len()], prev: vec![[0.0; 3]; bones.len()], bones, feel: f, radius: len * k * 0.12 }
            })
            .collect();
        Chains { chains, started: false }
    }

    pub fn len(&self) -> usize {
        self.chains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chains.is_empty()
    }

    pub fn bones(&self) -> Vec<usize> {
        self.chains.iter().flat_map(|c| c.bones.iter().copied()).collect()
    }

    /// One step: `frames` are the animated bone frames in the world (as
    /// [`Xf`] with metres), `gravity` the world's.
    pub fn step(&mut self, frames: &[Xf], tails: &[V3], gravity: V3, dt: f32, blockers: &[Blocker]) {
        for c in &mut self.chains {
            if !self.started {
                for (k, &b) in c.bones.iter().enumerate() {
                    c.pos[k] = tails[b];
                    c.prev[k] = tails[b];
                }
                continue;
            }
            let f = c.feel;
            // Stiffness per step at any frame rate.
            let pull = 1.0 - (1.0 - f.stiffness.clamp(0.0, 1.0)).powf(dt * 60.0);
            let keep = (1.0 - f.damping.clamp(0.0, 1.0)).powf(dt * 60.0) / (1.0 + f.drag * dt);
            for k in 0..c.bones.len() {
                let b = c.bones[k];
                let head = if k == 0 { frames[b].pos } else { c.pos[k - 1] };
                // Where the animation wants this tip: the bone's animated
                // direction from where its head is now.
                let anim_dir = sub(tails[b], frames[b].pos);
                let goal = add(head, anim_dir);
                let v = scale(sub(c.pos[k], c.prev[k]), keep);
                let mut p = add(c.pos[k], v);
                p = mad(p, gravity, f.gravity * dt * dt);
                p = add(p, scale(sub(goal, p), pull));
                // The bone's length.
                let len = length(anim_dir);
                p = add(head, scale(normalise_or(sub(p, head), normalise_or(anim_dir, [0.0, 1.0, 0.0])), len));
                for bl in blockers {
                    let q = closest_on_segment(bl.a, bl.b, p);
                    let d = sub(p, q);
                    let l = length(d);
                    let reach = bl.r + c.radius;
                    if l < reach {
                        p = add(q, scale(normalise_or(d, normalise_or(anim_dir, [0.0, 1.0, 0.0])), reach));
                        p = add(head, scale(normalise_or(sub(p, head), anim_dir), len));
                    }
                }
                c.prev[k] = c.pos[k];
                c.pos[k] = p;
            }
        }
        self.started = true;
    }

    /// Each chain bone's world turn: the animated turn swung so the bone
    /// points at its simulated tip.
    pub fn turns(&self, frames: &[Xf], tails: &[V3]) -> Vec<(usize, Quat)> {
        let mut out = Vec::new();
        for c in &self.chains {
            for (k, &b) in c.bones.iter().enumerate() {
                let head = if k == 0 { frames[b].pos } else { c.pos[k - 1] };
                let anim = sub(tails[b], frames[b].pos);
                let sim = sub(c.pos[k], head);
                let swing = studio_mesh::rig::q_from_to(normalise_or(anim, [0.0, 1.0, 0.0]), normalise_or(sim, [0.0, 1.0, 0.0]));
                out.push((b, q_normalise(q_mul(swing, frames[b].rot))));
            }
        }
        out
    }
}

/// Vertices on springs behind their animated places.
pub struct Jiggle {
    pos: Vec<V3>,
    vel: Vec<V3>,
    /// 0 rigid .. 1 fully loose, per vertex.
    pub loose: Vec<f32>,
    pub stiffness: f32,
    pub damping: f32,
    /// The furthest a vertex may lag (world units).
    pub max_lag: f32,
    started: bool,
}

impl Jiggle {
    /// `stiffness` in Hz (how fast it springs back), `damping` 0..1.
    pub fn new(loose: Vec<f32>, stiffness: f32, damping: f32, max_lag: f32) -> Jiggle {
        Jiggle { pos: Vec::new(), vel: Vec::new(), loose, stiffness, damping, max_lag, started: false }
    }

    /// Follows `target` (where the animation puts each vertex) one step;
    /// returns where the vertices are.
    pub fn step(&mut self, target: &[V3], gravity: V3, dt: f32) -> Vec<V3> {
        if !self.started || self.pos.len() != target.len() {
            self.pos = target.to_vec();
            self.vel = vec![[0.0; 3]; target.len()];
            self.started = true;
            return target.to_vec();
        }
        let omega = std::f32::consts::TAU * self.stiffness.max(0.01);
        let zeta = self.damping.clamp(0.0, 1.0) * 2.0;
        let mut out = Vec::with_capacity(target.len());
        for i in 0..target.len() {
            let w = self.loose.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            if w <= 0.0 {
                self.pos[i] = target[i];
                self.vel[i] = [0.0; 3];
                out.push(target[i]);
                continue;
            }
            // A damped spring towards the target, loose ones softer.
            let om = omega * (1.0 - 0.7 * w);
            let d = sub(self.pos[i], target[i]);
            let acc = add(sub(scale(d, -om * om), scale(self.vel[i], 2.0 * zeta * om)), scale(gravity, w * 0.02));
            self.vel[i] = mad(self.vel[i], acc, dt);
            let mut p = mad(self.pos[i], self.vel[i], dt);
            let lag = sub(p, target[i]);
            let l = length(lag);
            let cap = self.max_lag * w;
            if l > cap {
                p = mad(target[i], lag, cap / l);
            }
            self.pos[i] = p;
            out.push(add(target[i], scale(sub(p, target[i]), w)));
        }
        out
    }
}

/// How loose each vertex of a skinned mesh is: weight on loose bones
/// counts fully; elsewhere, how far it sits from its bone's line compared
/// with the bone's length (flesh on a big belly, fur tips, a coat's hem).
pub fn looseness(rig: &Rig, positions: &[V3], loose_bones: &[usize], flesh: f32) -> Vec<f32> {
    let mut out = vec![0.0; positions.len()];
    if !rig.is_skinned() {
        return out;
    }
    for (v, p) in positions.iter().enumerate() {
        let (j, w) = (rig.joints[v], rig.weights[v]);
        let mut l = 0.0;
        for k in 0..4 {
            let b = j[k] as usize;
            if w[k] <= 0.0 || b >= rig.bones.len() {
                continue;
            }
            if loose_bones.contains(&b) {
                l += w[k];
            } else if flesh > 0.0 {
                let bone = &rig.bones[b];
                let q = closest_on_segment(bone.head, bone.tail, *p);
                let away = length(sub(*p, q)) / bone.length().max(1e-6);
                l += w[k] * flesh * ((away - 0.25) * 1.5).clamp(0.0, 1.0);
            }
        }
        out[v] = l.clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_mesh::rig::Bone;

    fn tail_rig() -> Rig {
        // A body bone and a five-bone tail sticking out along +x.
        let mut bones = vec![Bone::new("hips", None, [0.0; 3], [0.0, -0.2, 0.0])];
        for i in 0..5 {
            let x0 = 0.1 * i as f32;
            bones.push(Bone::new(format!("tail.{i}"), Some(i), [x0, 0.0, 0.0], [x0 + 0.1, 0.0, 0.0]));
        }
        Rig { bones, ..Default::default() }
    }

    #[test]
    fn loose_words_find_tails_ears_and_coats_but_not_arms() {
        assert!(is_loose("tail.3") && is_loose("ear.L") && is_loose("coat_tail.R") && is_loose("Antenna01") && is_loose("ponytail"));
        assert!(!is_loose("upper_arm.L") && !is_loose("hearth") && !is_loose("shin.R"));
        let rig = tail_rig();
        let chains = find_chains(&rig, &[]);
        assert_eq!(chains, vec![vec![1, 2, 3, 4, 5]]);
    }

    #[test]
    fn a_tail_lags_behind_a_turn_and_settles_back() {
        let rig = tail_rig();
        let mut ch = Chains::new(&rig, find_chains(&rig, &[]), Some(Feel { stiffness: 0.2, damping: 0.1, gravity: 0.0, drag: 1.0 }), 1.0);
        let dt = 1.0 / 60.0;
        // The animation: the whole body slides along z at 2 m/s for 0.5 s.
        let mut tip_lag_max = 0.0f32;
        let mut tip_lag_end = 0.0f32;
        for f in 0..180 {
            let t = f as f32 * dt;
            let z = 2.0 * t.min(0.5);
            let frames: Vec<Xf> = rig.rest_world().iter().map(|x| Xf { pos: add(x.pos, [0.0, 0.0, z]), ..*x }).collect();
            let tails: Vec<V3> = rig.bones.iter().map(|b| add(b.tail, [0.0, 0.0, z])).collect();
            ch.step(&frames, &tails, [0.0, 9.81, 0.0], dt, &[]);
            let tip = ch.chains[0].pos[4];
            let lag = (tails[5][2] - tip[2]).abs();
            tip_lag_max = tip_lag_max.max(lag);
            tip_lag_end = lag;
        }
        assert!(tip_lag_max > 0.05, "the tip trails while moving: {tip_lag_max}");
        assert!(tip_lag_end < 0.01, "and comes back after: {tip_lag_end}");
        // Lengths kept.
        let c = &ch.chains[0];
        for k in 1..5 {
            assert!((length(sub(c.pos[k], c.pos[k - 1])) - 0.1).abs() < 1e-3);
        }
    }

    #[test]
    fn a_loose_vertex_overshoots_and_settles() {
        let mut j = Jiggle::new(vec![1.0, 0.0], 3.0, 0.15, 1.0);
        let dt = 1.0 / 120.0;
        let mut over = 0.0f32;
        for f in 0..360 {
            let x = if f < 30 { f as f32 / 30.0 } else { 1.0 };
            let out = j.step(&[[x, 0.0, 0.0], [x, 0.0, 0.0]], [0.0; 3], dt);
            assert_eq!(out[1][0], x, "a rigid vertex stays put");
            over = over.max(out[0][0] - 1.0);
            if f == 359 {
                assert!((out[0][0] - 1.0).abs() < 0.01, "settles: {}", out[0][0]);
            }
        }
        assert!(over > 0.005, "it overshoots the stop: {over}");
    }
}
