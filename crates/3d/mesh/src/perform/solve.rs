// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Posing a rig by where things go rather than by angles: aim a bone,
//! turn it about an axis, put a limb's end on a point with the middle
//! joint bending the natural way. A [`Solver`] holds the local poses and
//! works out world frames on demand, so edits compose in any order.

use super::body::Limb;
use crate::rig::{BonePose, Quat, Rig, Xf, q_axis_angle, q_conj, q_from_to, q_mul, q_normalise, q_rotate};
use crate::{V3, add, cross, dot, length, normalise, scale, sub};
use studio_rig::ik::{self, Reach};

/// What an IK reach came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reached {
    /// Where the end really is.
    pub end: V3,
    /// How far the target was from the limb's root.
    pub asked: f32,
    /// The limb's length (both bones).
    pub length: f32,
    /// Whether the end is on the target (a target past full reach is not).
    pub on_target: bool,
}

pub struct Solver<'a> {
    pub rig: &'a Rig,
    pub local: Vec<BonePose>,
    rest_local: Vec<Xf>,
    rest_world: Vec<Xf>,
    world: Vec<Xf>,
    fresh: bool,
}

impl<'a> Solver<'a> {
    pub fn new(rig: &'a Rig) -> Solver<'a> {
        let n = rig.bones.len();
        Solver::with_pose(rig, vec![BonePose::default(); n])
    }

    pub fn with_pose(rig: &'a Rig, local: Vec<BonePose>) -> Solver<'a> {
        Solver { rig, local, rest_local: rig.rest_local(), rest_world: rig.rest_world(), world: Vec::new(), fresh: false }
    }

    fn refresh(&mut self) {
        if self.fresh {
            return;
        }
        let mut world: Vec<Xf> = Vec::with_capacity(self.rig.bones.len());
        for (i, b) in self.rig.bones.iter().enumerate() {
            let here = self.rest_local[i].then(&self.local.get(i).copied().unwrap_or_default().xf());
            world.push(match b.parent {
                Some(p) => world[p].then(&here),
                None => here,
            });
        }
        self.world = world;
        self.fresh = true;
    }

    /// Every bone's frame, posed.
    pub fn world(&mut self) -> &[Xf] {
        self.refresh();
        &self.world
    }

    pub fn frame(&mut self, i: usize) -> Xf {
        self.refresh();
        self.world[i]
    }

    /// The joint a bone turns about (its head), posed.
    pub fn head(&mut self, i: usize) -> V3 {
        self.frame(i).pos
    }

    /// The far end of a bone, posed.
    pub fn tail(&mut self, i: usize) -> V3 {
        let f = self.frame(i);
        f.apply([0.0, self.rig.bones[i].length(), 0.0])
    }

    /// The way a bone points, posed.
    pub fn dir(&mut self, i: usize) -> V3 {
        self.frame(i).apply_dir([0.0, 1.0, 0.0])
    }

    pub fn rest_rot(&self, i: usize) -> Quat {
        self.rest_world[i].rot
    }

    /// Edit a local pose; world frames are worked out again when asked.
    pub fn edit(&mut self, i: usize, f: impl FnOnce(&mut BonePose)) {
        if let Some(l) = self.local.get_mut(i) {
            f(l);
            self.fresh = false;
        }
    }

    /// Gives a bone this frame in the model; its children follow.
    pub fn set_world_rot(&mut self, i: usize, rot: Quat) {
        self.refresh();
        let parent = self.rig.bones[i].parent.map_or(Xf::IDENTITY, |p| self.world[p]);
        let frame = parent.then(&self.rest_local[i]);
        self.local[i].rot = q_normalise(q_mul(q_conj(frame.rot), rot));
        self.fresh = false;
    }

    /// Turns a bone about its head so it points along `dir` (the least
    /// turn from where it points).
    pub fn aim(&mut self, i: usize, dir: V3) {
        if length(dir) < 1e-9 {
            return;
        }
        let f = self.frame(i);
        let turn = q_from_to(f.apply_dir([0.0, 1.0, 0.0]), dir);
        self.set_world_rot(i, q_mul(turn, f.rot));
    }

    /// Turns a bone about a model axis (degrees; the axis through its head).
    pub fn rotate(&mut self, i: usize, axis: V3, deg: f32) {
        let f = self.frame(i);
        self.set_world_rot(i, q_mul(q_axis_angle(normalise(axis), deg.to_radians()), f.rot));
    }

    /// Turns a bone so its tip moves toward `dir` by `deg`.
    pub fn tilt_toward(&mut self, i: usize, dir: V3, deg: f32) {
        let d = self.dir(i);
        let axis = cross(d, dir);
        if length(axis) > 1e-6 {
            self.rotate(i, axis, deg);
        }
    }

    /// Shifts the whole figure (the root bone) by a model-space offset.
    pub fn shift_root(&mut self, root: usize, disp: V3) {
        self.refresh();
        let frame = self.rest_local[root];
        let d = q_rotate(q_conj(frame.rot), disp);
        self.local[root].loc = add(self.local[root].loc, d);
        self.fresh = false;
    }

    /// Puts a limb's end (ankle, wrist) on `target`, the middle joint
    /// bending toward `bend`. Bones keep their length: past full reach the
    /// limb straightens toward the target and `Reached::on_target` says
    /// so.
    pub fn limb_to(&mut self, limb: &Limb, target: V3, bend: V3) -> Reached {
        let (a, b) = (limb.upper, limb.lower);
        let root = self.head(a);
        let la = self.rig.bones[a].length();
        let lb = self.rig.bones[b].length();
        let solved = ik::two_bone(root.map(f64::from), f64::from(la), f64::from(lb), target.map(f64::from), bend.map(f64::from), Reach::default());
        let mid = solved.mid.map(|x| x as f32);
        let end = solved.end.map(|x| x as f32);
        // Upper bone to the middle joint, then the lower to the end.
        let fa = self.frame(a);
        self.set_world_rot(a, q_mul(q_from_to(fa.apply_dir([0.0, 1.0, 0.0]), sub(mid, root)), fa.rot));
        let fb = self.frame(b);
        self.set_world_rot(b, q_mul(q_from_to(fb.apply_dir([0.0, 1.0, 0.0]), sub(end, fb.pos)), fb.rot));
        Reached { end, asked: length(sub(target, root)), length: la + lb, on_target: length(sub(end, target)) < 1e-3 * (la + lb).max(1.0) }
    }

    /// The limb's current middle joint direction away from the line root
    /// to end (which way it bends), unit; zero when straight.
    pub fn bend_of(&mut self, limb: &Limb) -> V3 {
        let root = self.head(limb.upper);
        let mid = self.head(limb.lower);
        let end = self.head(limb.end);
        let line = normalise(sub(end, root));
        let off = sub(mid, root);
        normalise(sub(off, scale(line, dot(off, line))))
    }

    /// The local poses so far.
    pub fn poses(&self) -> Vec<BonePose> {
        self.local.clone()
    }
}

/// The twist about `axis` (degrees) that carries direction `from` to
/// direction `to`, both taken flat across the axis.
pub fn signed_angle(from: V3, to: V3, axis: V3) -> f32 {
    let a = normalise(sub(from, scale(axis, dot(from, axis))));
    let b = normalise(sub(to, scale(axis, dot(to, axis))));
    if a == [0.0; 3] || b == [0.0; 3] {
        return 0.0;
    }
    let s = dot(cross(a, b), axis);
    s.atan2(dot(a, b)).to_degrees()
}

/// Rotates a vector about an axis.
pub fn rotated(v: V3, axis: V3, deg: f32) -> V3 {
    q_rotate(q_axis_angle(normalise(axis), deg.to_radians()), v)
}

#[cfg(test)]
mod tests {
    use super::super::body::Body;
    use super::*;

    fn person() -> (crate::Mesh, std::sync::Arc<Rig>) {
        let m = crate::character::character(400.0, &crate::character::Build { detail: 1, ..Default::default() });
        let r = m.rig.clone().unwrap();
        (m, r)
    }

    #[test]
    fn a_limb_reaches_a_point_with_bones_that_keep_their_length() {
        let (_, rig) = person();
        let body = Body::of(&rig);
        let arm = body.arm(super::super::body::Side::L).unwrap().clone();
        let mut s = Solver::new(&rig);
        let shoulder = s.head(arm.upper);
        let total = Body::limb_length(&rig, &arm);
        let target = add(shoulder, scale(body.fwd, total * 0.7));
        let r = s.limb_to(&arm, target, scale(body.fwd, -1.0));
        assert!(r.on_target, "{r:?}");
        let (e, m) = (s.head(arm.end), s.head(arm.lower));
        assert!(length(sub(e, target)) < 0.5, "{e:?} vs {target:?}");
        // The elbow went behind the line from shoulder to hand.
        assert!(dot(sub(m, shoulder), body.fwd) < dot(sub(e, shoulder), body.fwd));
        assert!((length(sub(m, shoulder)) - rig.bones[arm.upper].length()).abs() < 0.01);
        // Too far: it straightens toward the target and says so.
        let far = add(shoulder, scale(body.fwd, total * 2.0));
        let r = s.limb_to(&arm, far, scale(body.fwd, -1.0));
        assert!(!r.on_target && r.asked > r.length);
        let e = s.head(arm.end);
        assert!((length(sub(e, shoulder)) - total).abs() < 0.5, "straight, not stretched");
    }

    #[test]
    fn aim_and_tilt_turn_a_bone_where_asked() {
        let (_, rig) = person();
        let body = Body::of(&rig);
        let head = body.head.unwrap();
        let mut s = Solver::new(&rig);
        s.tilt_toward(head, body.fwd, 30.0);
        let d = s.dir(head);
        assert!((dot(d, body.fwd) - 30f32.to_radians().sin()).abs() < 0.02, "{d:?}");
        s.aim(head, scale(body.left(), 1.0));
        assert!(dot(s.dir(head), body.left()) > 0.999);
    }
}
