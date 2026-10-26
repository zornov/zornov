// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ragdolls from any rig: a person, a dog, a dragon, a tentacle.
//!
//! Every bone long enough to matter gets a body: the hull of the skin it
//! carries (small bones such as fingers, toes, eyes and the jaw ride on
//! their parent's body, their skin counted with it). Each body hangs from
//! its nearest ancestor's by a joint at the bone's head with limits by
//! what the bone is: knees and elbows are hinges that bend one way,
//! shoulders and hips swing wide, the spine and neck a little, anything
//! else (tails, ears, wings) moderately. Joint limits are measured from
//! the rest pose, so the ragdoll starts in whatever pose the character
//! was in.
//!
//! The same bodies serve three ways: kinematic, following the animation
//! (the character pushes things and stands in their way); powered, with
//! drives pulling each joint towards the animated pose with a strength
//! (a stumble, a hit that rocks the body); limp (a fall).

use super::hull::Hull;
use super::joint::{Drive, Joint, JointKind, Spring};
use super::material::Stuff;
use super::math::*;
use super::shape::Shape;
use super::{Body, Collider, Kind, World};
use std::sync::Arc;
use studio_mesh::Mesh;
use studio_mesh::rig::{BonePose, Posed, Rig, Xf};

/// Model pixels to the world: `world = place.apply(model · k)`.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub place: Pose,
    pub k: f32,
}

impl Placement {
    pub fn point(&self, p: V3) -> V3 {
        self.place.apply(scale(p, self.k))
    }

    pub fn frame(&self, x: &Xf) -> Pose {
        Pose { p: self.point(x.pos), q: q_normalise(q_mul(self.place.q, x.rot)) }
    }

    /// A world pose back as a model frame (rotation and position in px).
    pub fn unframe(&self, p: &Pose) -> (Quat, V3) {
        (q_normalise(q_mul(q_conj(self.place.q), p.q)), scale(self.place.unapply(p.p), 1.0 / self.k.max(1e-12)))
    }
}

/// What a bone is, from its name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    Knee,
    Elbow,
    Shoulder,
    Hip,
    Spine,
    Neck,
    Head,
    Hand,
    Foot,
    Other,
}

pub fn role(name: &str) -> Role {
    let n = name.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["shin", "calf", "knee", "lower_leg", "lowerleg", "leg_lower"]) {
        Role::Knee
    } else if has(&["forearm", "lower_arm", "lowerarm", "elbow", "arm_lower"]) {
        Role::Elbow
    } else if has(&["upper_arm", "upperarm", "shoulder", "clavicle", "arm_upper"]) || n == "arm.l" || n == "arm.r" {
        Role::Shoulder
    } else if has(&["thigh", "upper_leg", "upperleg", "leg_upper"]) || n == "leg.l" || n == "leg.r" {
        Role::Hip
    } else if has(&["spine", "chest", "torso", "abdomen", "belly", "pelvis", "hips", "back"]) {
        Role::Spine
    } else if has(&["neck"]) {
        Role::Neck
    } else if has(&["head", "skull"]) {
        Role::Head
    } else if has(&["hand", "wrist", "paw"]) {
        Role::Hand
    } else if has(&["foot", "ankle", "toe", "hoof"]) {
        Role::Foot
    } else {
        Role::Other
    }
}

/// How a ragdoll is made.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub stuff: Stuff,
    /// Bones shorter than this share of the rig's size ride on their
    /// parent's body.
    pub min_bone: f32,
    pub kind: Kind,
}

/// A ragdoll in a world.
#[derive(Clone, Debug)]
pub struct Ragdoll {
    /// For each bone, the body it moves with (its own or an ancestor's).
    pub body_of: Vec<Option<usize>>,
    pub own: Vec<bool>,
    /// Joint index per bone with its own body (None for the root).
    pub joint_of: Vec<Option<usize>>,
    pub place: Placement,
    /// How strongly each joint can hold (N·m at full strength).
    torque: Vec<f32>,
}

fn rig_size(rig: &Rig) -> f32 {
    let mut b = Aabb::EMPTY;
    for bone in &rig.bones {
        b.grow(bone.head);
        b.grow(bone.tail);
    }
    if b.is_empty() { 1.0 } else { length(b.size()).max(1e-6) }
}

impl Ragdoll {
    /// Builds the bodies and joints of `rig` (skinning `mesh`) posed as
    /// `posed`, placed in the world by `place`.
    pub fn build(world: &mut World, rig: &Rig, mesh: &Mesh, posed: &Posed, place: Placement, spec: &Spec) -> Ragdoll {
        let n = rig.bones.len();
        let size = rig_size(rig);
        // Which bones have bodies: deforming, long enough (roots always).
        let mut own = vec![false; n];
        for (i, b) in rig.bones.iter().enumerate() {
            let long = b.length() >= spec.min_bone * size;
            own[i] = b.deform && (long || b.parent.is_none_or(|p| !rig.bones[p].deform));
        }
        let carrier = |i: usize, own: &[bool]| -> Option<usize> {
            let mut k = Some(i);
            while let Some(j) = k {
                if own[j] {
                    return Some(j);
                }
                k = rig.bones[j].parent;
            }
            None
        };
        // Skin per body: the vertices each bone carries most.
        let rest = rig.rest_world();
        let mut skin: Vec<Vec<V3>> = vec![Vec::new(); n];
        if rig.is_skinned() {
            for (v, p) in mesh.positions.iter().enumerate() {
                let (j, w) = (rig.joints[v], rig.weights[v]);
                let best = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
                if w[best] <= 0.0 {
                    continue;
                }
                if let Some(c) = carrier(j[best] as usize, &own) {
                    skin[c].push(*p);
                }
            }
        }
        let mut body_of: Vec<Option<usize>> = vec![None; n];
        let mut mass_of = vec![0.0f32; n];
        for i in 0..n {
            if !own[i] {
                continue;
            }
            let b = &rig.bones[i];
            let rf = rest[i];
            let len = b.length().max(size * 0.01);
            // The skin in the bone's own frame, in metres.
            let mut local: Vec<V3> = skin[i].iter().map(|p| scale(rf.inverse().apply(*p), place.k)).collect();
            // Skinning hands a bone a few far points (an armpit on the upper
            // arm, the seat on a thigh); in a hull they would make the limb
            // several times its weight. Keep the skin near the bone itself.
            if local.len() >= 8 {
                let lm = len * place.k;
                let radial = |p: &V3| (p[0] * p[0] + p[2] * p[2]).sqrt();
                let mut r: Vec<f32> = local.iter().map(radial).collect();
                r.sort_by(f32::total_cmp);
                let cut = r[r.len() / 2] * 1.6;
                let near: Vec<V3> = local.iter().copied().filter(|p| radial(p) <= cut && p[1] >= -0.1 * lm && p[1] <= 1.1 * lm).collect();
                if near.len() >= 8 {
                    local = near;
                }
            }
            let shape = if local.len() >= 8 {
                Hull::from_points(&local, 24, len * place.k * 0.05).map(|h| Shape::Hull(Arc::new(h)))
            } else {
                None
            };
            let (shape, at) = match shape {
                Some(s) => (s, Pose::IDENTITY),
                None => {
                    let r = len * place.k * 0.15;
                    (Shape::Capsule { half: (len * place.k * 0.5 - r).max(0.0), r }, Pose::at([0.0, len * place.k * 0.5, 0.0]))
                }
            };
            let mut col = Collider::new(shape, at, &spec.stuff);
            col.group = 2;
            let body = Body::new(spec.kind, vec![col], place.frame(&posed.world[i]));
            mass_of[i] = body.mass;
            let id = world.add(body);
            body_of[i] = Some(id);
        }
        for i in 0..n {
            if body_of[i].is_none() {
                body_of[i] = carrier(i, &own).and_then(|c| body_of[c]);
            }
        }
        // Joints from each body to its carrier's.
        let mut joint_of = vec![None; n];
        let mut torque = vec![0.0; n];
        let subtree_mass: Vec<f32> = (0..n).map(|i| (0..n).filter(|&j| is_under(rig, j, i) && own[j]).map(|j| mass_of[j]).sum()).collect();
        let g = length(world.gravity).max(1.0);
        for i in 0..n {
            if !own[i] {
                continue;
            }
            let Some(p) = rig.bones[i].parent.and_then(|p| carrier(p, &own)) else { continue };
            let (Some(ba), Some(bb)) = (body_of[p], body_of[i]) else { continue };
            let bone = &rig.bones[i];
            let r = role(&bone.name);
            // The joint's frame at rest: x along the bone (twist) for a
            // ball, x across it (the bend axis) for a hinge.
            let rf = rest[i];
            let along = rf.apply_dir([0.0, 1.0, 0.0]);
            let across = rf.apply_dir([1.0, 0.0, 0.0]);
            let kind = match r {
                Role::Knee | Role::Elbow => {
                    // Which way it bends: knees move the foot back (+z),
                    // elbows the hand forward (−z), for figures facing −z.
                    let lever = sub(bone.tail, bone.head);
                    let moves = cross(across, lever);
                    let want = if r == Role::Knee { 1.0 } else { -1.0 };
                    let s = if moves[2] * want >= 0.0 { 1.0 } else { -1.0 };
                    let (lo, hi) = if s > 0.0 { (-0.05, 2.6) } else { (-2.6, 0.05) };
                    JointKind::Hinge { limits: Some((lo, hi)), motor: None, spring: None }
                }
                _ => {
                    let (cone, twist) = match r {
                        Role::Shoulder => (85f32, 40f32),
                        Role::Hip => (80.0, 35.0),
                        Role::Spine => (22.0, 15.0),
                        Role::Neck => (35.0, 30.0),
                        Role::Head => (35.0, 45.0),
                        Role::Hand | Role::Foot => (50.0, 20.0),
                        _ => (45.0, 25.0),
                    };
                    JointKind::Ball { cone: Some(cone.to_radians()), twist: Some((-twist.to_radians(), twist.to_radians())), drive: None }
                }
            };
            let axis = if matches!(kind, JointKind::Hinge { .. }) { across } else { along };
            let (t1, t2) = tangents(axis);
            let jq = q_from_mat(&[[axis[0], t1[0], t2[0]], [axis[1], t1[1], t2[1]], [axis[2], t1[2], t2[2]]]);
            // Rest frames in the bodies.
            let j_rest = place.frame(&Xf::new(jq, bone.head));
            let model_a = place.frame(&rest[p]);
            let model_b = place.frame(&rest[i]);
            let fa = world.bodies[ba].offset.inverse().then(&model_a.inverse()).then(&j_rest);
            let fb = world.bodies[bb].offset.inverse().then(&model_b.inverse()).then(&j_rest);
            let mut joint = Joint::with_frames(ba, bb, fa, fb, kind);
            joint.collide = false;
            joint_of[i] = Some(world.join(joint));
            // Enough to hold what hangs below it out level, twice over.
            let reach = (0..n).filter(|&j| is_under(rig, j, i)).map(|j| length(sub(rig.bones[j].tail, bone.head))).fold(bone.length(), f32::max) * place.k;
            torque[i] = 2.0 * subtree_mass[i] * g * reach;
        }
        // Parts of the body that overlap at rest never collide.
        let ids: Vec<usize> = (0..n).filter(|&i| own[i]).filter_map(|i| body_of[i]).collect();
        for (x, &a) in ids.iter().enumerate() {
            for &b in &ids[x + 1..] {
                let (ba, bb) = (world.bodies[a].bounds().fattened(0.01), world.bodies[b].bounds().fattened(0.01));
                if ba.overlaps(&bb) {
                    world.ignore.insert((a.min(b), a.max(b)));
                }
            }
        }
        Ragdoll { body_of, own, joint_of, place, torque }
    }

    /// The bodies' poses for an animated pose.
    fn goal(&self, i: usize, world: &World, posed: &Posed) -> Option<Pose> {
        let b = self.body_of[i]?;
        Some(self.place.frame(&posed.world[i]).then(&world.bodies[b].offset))
    }

    /// Kinematic bodies follow the animation this step.
    pub fn follow(&self, world: &mut World, posed: &Posed) {
        for i in 0..self.own.len() {
            if !self.own[i] {
                continue;
            }
            if let (Some(b), Some(g)) = (self.body_of[i], self.goal(i, world, posed))
                && world.bodies[b].kind == Kind::Kinematic
            {
                world.bodies[b].target = Some(g);
            }
        }
    }

    /// Every body of the ragdoll moves as `kind` from now (kinematic ones
    /// keep the speed they had).
    pub fn set_kind(&self, world: &mut World, kind: Kind) {
        for i in 0..self.own.len() {
            if self.own[i]
                && let Some(b) = self.body_of[i]
            {
                let (v, w) = (world.bodies[b].v, world.bodies[b].w);
                world.bodies[b].set_kind(kind);
                world.bodies[b].v = v;
                world.bodies[b].w = w;
            }
        }
    }

    /// Joint drives pull towards the animated pose with `strength` (0 is
    /// limp, 1 holds the pose against gravity).
    pub fn drive(&self, world: &mut World, posed: &Posed, strength: f32) {
        let s = strength.clamp(0.0, 4.0);
        for i in 0..self.own.len() {
            let Some(j) = self.joint_of[i] else { continue };
            let (ja, jb) = (world.joints[j].a, world.joints[j].b);
            let pa = self.body_of.iter().position(|&b| b == Some(ja)).and_then(|bone| self.goal(bone, world, posed).map(|g| (bone, g)));
            let gb = self.goal(i, world, posed);
            let (Some((_, ga)), Some(gb)) = (pa, gb) else { continue };
            let joint = &mut world.joints[j];
            let wa = ga.then(&joint.frame_a);
            let wb = gb.then(&joint.frame_b);
            let target = q_normalise(q_mul(q_conj(wa.q), wb.q));
            let max = self.torque[i] * s;
            let hz = 2.0 + 10.0 * s.min(1.0);
            let _ = jb;
            match &mut joint.kind {
                JointKind::Ball { drive, .. } => *drive = (s > 0.0).then_some(Drive { target, hertz: hz, damping: 1.0, max }),
                JointKind::Hinge { spring, .. } => {
                    // The animated bend angle about the hinge.
                    let ya = wa.rotate([0.0, 1.0, 0.0]);
                    let ax = wa.rotate([1.0, 0.0, 0.0]);
                    let yb = wb.rotate([0.0, 1.0, 0.0]);
                    let yb = sub(yb, scale(ax, dot(yb, ax)));
                    let angle = dot(cross(ya, yb), ax).atan2(dot(ya, yb));
                    *spring = (s > 0.0).then_some(Spring { target: angle, hertz: hz, damping: 1.0, max });
                }
                _ => {}
            }
        }
    }

    /// The rig's local poses from where the bodies are: bones with bodies
    /// take their body's turn, the rest keep `anim`'s; the topmost body's
    /// bone also takes its position.
    pub fn locals(&self, world: &World, rig: &Rig, anim: &Posed) -> Vec<BonePose> {
        let n = rig.bones.len();
        let rl = rig.rest_local();
        let mut world_rot = vec![Q_ID; n];
        let mut frames: Vec<Xf> = Vec::with_capacity(n);
        let mut out = Vec::with_capacity(n);
        for (i, b) in rig.bones.iter().enumerate() {
            let parent = b.parent.map_or(Xf::IDENTITY, |p| frames[p]);
            let frame = parent.then(&rl[i]);
            let mut pose = anim.local.get(i).copied().unwrap_or_default();
            if self.own[i]
                && let Some(body) = self.body_of[i]
            {
                let (rot, pos) = self.place.unframe(&world.bodies[body].model_pose());
                world_rot[i] = rot;
                pose.rot = q_normalise(q_mul(q_conj(frame.rot), rot));
                // The first bone with a body carries the whole figure's place.
                let top = b.parent.is_none_or(|p| self.body_of[p].is_none());
                if top {
                    pose.loc = q_rotate(q_conj(frame.rot), sub(pos, frame.pos));
                    pose.loc = scale(pose.loc, 1.0 / frame.scale.max(1e-9));
                }
            }
            let here = frame.then(&pose.xf());
            world_rot[i] = here.rot;
            frames.push(here);
            out.push(pose);
        }
        out
    }

    /// All the ragdoll's bodies.
    pub fn bodies(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.own.len()).filter(|&i| self.own[i]).filter_map(|i| self.body_of[i]).collect();
        v.dedup();
        v
    }
}

/// Whether bone `j` is `i` or hangs under it.
fn is_under(rig: &Rig, j: usize, i: usize) -> bool {
    let mut k = Some(j);
    while let Some(x) = k {
        if x == i {
            return true;
        }
        k = rig.bones[x].parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::material::named;
    use super::*;
    use studio_mesh::rig::PoseInput;

    // Bare: the ragdoll weighs the body, and loose clothes are not flesh.
    fn figure() -> Mesh {
        studio_mesh::character::character(180.0, &studio_mesh::character::Build { dressed: false, ..Default::default() })
    }

    #[test]
    fn a_character_falls_limp_and_lies_on_the_ground() {
        let mesh = figure();
        let rig = mesh.rig.clone().expect("the character base is rigged");
        let posed = rig.evaluate(&PoseInput::default());
        let mut w = World::new([0.0, 9.81, 0.0]);
        w.add(Body::new(Kind::Static, vec![Collider::new(Shape::Plane { n: [0.0, -1.0, 0.0], d: 0.0 }, Pose::IDENTITY, &named("concrete").unwrap())], Pose::IDENTITY));
        // The figure's feet on the ground (its lowest point at y = 0).
        let (_, hi) = mesh.bounds();
        let place = Placement { place: Pose::at([0.0, -hi[1] * 0.01, 0.0]), k: 0.01 };
        let rd = Ragdoll::build(&mut w, &rig, &mesh, &posed, place, &Spec { stuff: named("flesh").unwrap(), min_bone: 0.05, kind: Kind::Dynamic });
        let bodies = rd.bodies();
        assert!(bodies.len() >= 10 && bodies.len() <= 24, "{} bodies", bodies.len());
        let mass: f32 = bodies.iter().map(|&b| w.bodies[b].mass).sum();
        assert!(mass > 25.0 && mass < 160.0, "a person's weight: {mass} kg");
        // A shove at the chest.
        let chest = rd.body_of[rig.bone("chest").unwrap()].unwrap();
        let at = w.bodies[chest].pose.p;
        w.bodies[chest].apply_impulse([0.0, 0.0, 60.0], at);
        w.advance(4.0);
        let head = rd.body_of[rig.bone("head").unwrap()].unwrap();
        let hy = w.bodies[head].pose.p[1];
        assert!(hy > -0.5, "the head ends near the ground: {hy}");
        for &b in &bodies {
            assert!(w.bodies[b].pose.p[1] < 0.05, "nothing sinks through the ground");
        }
        // The rig back from the bodies keeps its bones joined.
        let locals = rd.locals(&w, &rig, &posed);
        let frames = rig.forward(&locals);
        let hips = rig.bone("hips").unwrap();
        let (_, hips_now) = rd.place.unframe(&w.bodies[rd.body_of[hips].unwrap()].model_pose());
        assert!(length(sub(frames[hips].pos, hips_now)) < 1.0, "{:?} vs {:?}", frames[hips].pos, hips_now);
    }

    #[test]
    fn a_powered_ragdoll_holds_its_pose_better_than_a_limp_one() {
        let mesh = figure();
        let rig = mesh.rig.clone().unwrap();
        let posed = rig.evaluate(&PoseInput::default());
        let sag = |strength: f32| {
            let mut w = World::new([0.0, 9.81, 0.0]);
            let place = Placement { place: Pose::at([0.0, -3.0, 0.0]), k: 0.01 };
            let rd = Ragdoll::build(&mut w, &rig, &mesh, &posed, place, &Spec { stuff: named("flesh").unwrap(), min_bone: 0.05, kind: Kind::Dynamic });
            // The hips held up, the rest hanging on its joints.
            let hips = rd.body_of[rig.bone("hips").unwrap()].unwrap();
            w.bodies[hips].set_kind(Kind::Static);
            rd.drive(&mut w, &posed, strength);
            w.advance(1.0);
            let head = rd.body_of[rig.bone("head").unwrap()].unwrap();
            let (_, now) = rd.place.unframe(&w.bodies[head].model_pose());
            length(sub(now, posed.world[rig.bone("head").unwrap()].pos))
        };
        let limp = sag(0.0);
        let held = sag(1.0);
        assert!(held < limp * 0.5, "held {held} vs limp {limp}");
        assert!(held < 15.0, "within 15 cm: {held} px");
    }
}
