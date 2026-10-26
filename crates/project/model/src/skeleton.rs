// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skeletons: bones with parents that bend a layer's picture and carry
//! part layers, posed by forward kinematics (each bone's rotation) and
//! inverse kinematics (a hand or foot placed, the limb follows).
//!
//! Bones are drawn on the layer's picture in mask coordinates (0..1 across
//! the layer after crop), so they move with the layer. Rotations are
//! worked out in picture pixels (`aspect` is the picture's width over its
//! height) so a turned bone stays rigid on a wide picture. A bone turns
//! about its `head` (the joint it hangs from) and carries its children.
//!
//! What animates (keyframe paths): `bone.<id>.rotation` (clockwise degrees
//! from rest), `bone.<id>.offset` (a shift of the head, 0..1 layer
//! units; usually only the root's), `ik.<id>.target` (where the chain's
//! end reaches, layer coordinates) and `ik.<id>.blend` (0 = the bones'
//! own rotations, 1 = the IK solution).
//!
//! The renderer bends the picture by the posed bones (linear blend
//! skinning on the puppet mesh, weights measured through the picture) and
//! part layers parented to a bone follow it ([`crate::Parent::bone`]).

use crate::value::{Extra, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The bones of one layer, its IK handles and its saved poses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    /// Off keeps the bones but shows the picture unbent.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The picture's width over its height, in pixels (after crop), when
    /// the bones were drawn: rotations are rigid in pixels.
    #[serde(default = "one")]
    pub aspect: f64,
    /// Bones, parents before children.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bones: Vec<Bone>,
    /// IK handles, solved in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ik: Vec<IkHandle>,
    /// The pose library: named poses to apply again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub poses: Vec<Pose>,
    /// How far each bone's pull reaches into the picture around it, as a
    /// fraction of the picture's shorter side (smaller = stiffer limbs,
    /// sharper joints).
    #[serde(default = "falloff")]
    pub falloff: f64,
    /// Which way the character faces in its picture: right, left, or the
    /// camera (`None`); motions swing limbs toward the front.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces_right: Option<bool>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_origin(v: &Vec2) -> bool {
    *v == Vec2::ZERO
}
fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}
fn falloff() -> f64 {
    0.06
}

impl Default for Skeleton {
    fn default() -> Self {
        Skeleton { enabled: true, aspect: 1.0, bones: Vec::new(), ik: Vec::new(), poses: Vec::new(), falloff: falloff(), faces_right: None, extra: Extra::new() }
    }
}

/// One bone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    /// Unique within its skeleton; keyframe paths use it.
    pub id: u32,
    /// Display name, also its role for motions (`upper_arm.front`,
    /// `shin.back`, `head`, `tail.2`…).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The bone it hangs from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    /// The joint it turns about, at rest (0..1 layer coordinates).
    pub head: Vec2,
    /// Its other end at rest.
    pub tail: Vec2,
    /// Clockwise degrees from rest (animatable).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// Shift of the head from where its parent carries it, 0..1 layer
    /// units (animatable; usually the root's: the whole body moving).
    #[serde(default, skip_serializing_if = "is_origin")]
    pub offset: Vec2,
    /// Lowest rotation allowed (degrees from rest), for IK and posing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Highest rotation allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Where its part of the picture is drawn where parts fold over each
    /// other, -1 (behind) to 1 (in front).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: f64,
    /// Whether this bone bends the picture; off for bones that only carry
    /// part layers or steer others (a control bone).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub deform: bool,
    /// How much longer than at rest the bone is (1 = rigid). IK with
    /// stretch sets it; its children ride along at the stretched tail.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub stretch: f64,
}

impl Bone {
    /// A bone at rest from `head` to `tail`.
    pub fn new(id: u32, name: impl Into<String>, parent: Option<u32>, head: Vec2, tail: Vec2) -> Bone {
        Bone { id, name: name.into(), parent, head, tail, rotation: 0.0, offset: Vec2::ZERO, min: None, max: None, depth: 0.0, deform: true, stretch: 1.0 }
    }

    /// The settings that can carry keyframes.
    pub const ANIMATED: [&'static str; 2] = ["rotation", "offset"];

    /// `rotation` kept inside the bone's limits.
    pub fn limit(&self, deg: f64) -> f64 {
        let lo = self.min.unwrap_or(f64::NEG_INFINITY);
        let hi = self.max.unwrap_or(f64::INFINITY);
        if lo <= hi { deg.clamp(lo, hi) } else { deg }
    }

    /// Reads a setting by name.
    pub fn get(&self, key: &str) -> Option<crate::Value> {
        use crate::Value;
        Some(match key {
            "rotation" => Value::Angle(self.rotation),
            "offset" => Value::Point(self.offset),
            "head" => Value::Point(self.head),
            "tail" => Value::Point(self.tail),
            "depth" => Value::Scalar(self.depth),
            _ => return None,
        })
    }

    /// Writes a setting by name; `false` for an unknown one or a value of
    /// the wrong kind.
    pub fn set(&mut self, key: &str, v: &crate::Value) -> bool {
        use crate::Value;
        match (key, v) {
            ("rotation", Value::Angle(x) | Value::Scalar(x)) => self.rotation = *x,
            ("offset", Value::Point(p)) => self.offset = *p,
            ("head", Value::Point(p)) => self.head = *p,
            ("tail", Value::Point(p)) => self.tail = *p,
            ("depth", Value::Scalar(x)) => self.depth = *x,
            _ => return false,
        }
        true
    }
}

/// An inverse-kinematics handle: the end of a chain of bones reaching for
/// a target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IkHandle {
    /// Unique within its skeleton; keyframe paths use it.
    pub id: u32,
    /// Display name ("front foot").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The last bone of the chain; its tail reaches for the target.
    pub bone: u32,
    /// How many bones up from `bone` turn (2 for an arm or a leg).
    #[serde(default = "two")]
    pub chain: u32,
    /// Where the chain's end reaches, 0..1 layer coordinates (animatable).
    pub target: Vec2,
    /// 0 = the bones' own rotations, 1 = fully solved (animatable).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub blend: f64,
    /// Bend the middle joint the other way than at rest.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip: bool,
    /// Off keeps the handle but leaves the bones to their rotations.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Where the middle joints bend toward (0..1 layer coordinates); none
    /// keeps the bend the bones have at rest (see `flip`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pole: Option<Vec2>,
    /// The longest the chain may grow to reach a far target, as a factor
    /// of its length (1 = rigid bones; 1.5 = half again as long).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub stretch: f64,
    /// Soft IK, 0..0.5: the last fraction of the reach over which the limb
    /// eases into straight instead of snapping.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub soft: f64,
}

impl Default for IkHandle {
    fn default() -> Self {
        IkHandle { id: 0, name: String::new(), bone: 0, chain: 2, target: Vec2::ZERO, blend: 1.0, flip: false, enabled: true, pole: None, stretch: 1.0, soft: 0.0 }
    }
}

fn two() -> u32 {
    2
}

impl IkHandle {
    /// The settings that can carry keyframes.
    pub const ANIMATED: [&'static str; 2] = ["target", "blend"];

    /// Reads a setting by name.
    pub fn get(&self, key: &str) -> Option<crate::Value> {
        use crate::Value;
        Some(match key {
            "target" => Value::Point(self.target),
            "blend" => Value::Scalar(self.blend),
            _ => return None,
        })
    }

    /// Writes a setting by name.
    pub fn set(&mut self, key: &str, v: &crate::Value) -> bool {
        use crate::Value;
        match (key, v) {
            ("target", Value::Point(p)) => self.target = *p,
            ("blend", Value::Scalar(x)) => self.blend = x.clamp(0.0, 1.0),
            _ => return false,
        }
        true
    }
}

/// A named pose: each bone's rotation (by bone name, so a pose moves to
/// another character with the same bone names) and the root's offset.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Pose {
    /// What the pose is called ("contact", "arms up").
    pub name: String,
    /// Rotation by bone name.
    #[serde(default)]
    pub rotations: BTreeMap<String, f64>,
    /// Offset by bone name (only bones that have one).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub offsets: BTreeMap<String, Vec2>,
}

/// A 2D affine map: `x' = m[0] x + m[1] y + m[2]`, `y' = m[3] x + m[4] y + m[5]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    /// Changes nothing.
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    /// A clockwise turn by `deg` about `c` (y down).
    pub fn turn_about(deg: f64, c: Vec2) -> Affine {
        let (s, co) = deg.to_radians().sin_cos();
        Affine([co, -s, c.x - co * c.x + s * c.y, s, co, c.y - s * c.x - co * c.y])
    }

    /// A shift.
    pub fn shift(d: Vec2) -> Affine {
        Affine([1.0, 0.0, d.x, 0.0, 1.0, d.y])
    }

    /// Scales x and y.
    pub fn scale(x: f64, y: f64) -> Affine {
        Affine([x, 0.0, 0.0, 0.0, y, 0.0])
    }

    /// `self` after `o` (`o` applied first).
    pub fn then_after(&self, o: &Affine) -> Affine {
        let (a, b) = (self.0, o.0);
        Affine([
            a[0] * b[0] + a[1] * b[3],
            a[0] * b[1] + a[1] * b[4],
            a[0] * b[2] + a[1] * b[5] + a[2],
            a[3] * b[0] + a[4] * b[3],
            a[3] * b[1] + a[4] * b[4],
            a[3] * b[2] + a[4] * b[5] + a[5],
        ])
    }

    /// Where `p` goes.
    pub fn apply(&self, p: Vec2) -> Vec2 {
        let m = self.0;
        Vec2::new(m[0] * p.x + m[1] * p.y + m[2], m[3] * p.x + m[4] * p.y + m[5])
    }

    /// The inverse, if it has one.
    pub fn inverse(&self) -> Option<Affine> {
        let m = self.0;
        let det = m[0] * m[4] - m[1] * m[3];
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
        Some(Affine([a, b, -(a * m[2] + b * m[5]), c, d, -(c * m[2] + d * m[5])]))
    }

    /// How far it turns things, clockwise degrees.
    pub fn angle(&self) -> f64 {
        self.0[3].atan2(self.0[0]).to_degrees()
    }
}

/// A skeleton posed at one moment: each bone's map from its rest place
/// (0..1 layer coordinates) to where it is now, and its ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Posed {
    /// Per bone, in the skeleton's order: id, map, head, tail.
    pub bones: Vec<PosedBone>,
}

/// One posed bone.
#[derive(Clone, Debug, PartialEq)]
pub struct PosedBone {
    /// The bone's id.
    pub id: u32,
    /// Rest picture → posed picture, 0..1 layer coordinates.
    pub map: Affine,
    /// Where its head now is.
    pub head: Vec2,
    /// Where its tail now is.
    pub tail: Vec2,
    /// How far it is turned from rest on the picture, clockwise degrees.
    pub turn: f64,
}

impl Posed {
    /// The posed bone `id`.
    pub fn bone(&self, id: u32) -> Option<&PosedBone> {
        self.bones.iter().find(|b| b.id == id)
    }
}

impl Skeleton {
    /// The bone with id `id`.
    pub fn bone(&self, id: u32) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }

    /// The bone with id `id`, to change.
    pub fn bone_mut(&mut self, id: u32) -> Option<&mut Bone> {
        self.bones.iter_mut().find(|b| b.id == id)
    }

    /// The bone called `name` (any case).
    pub fn named(&self, name: &str) -> Option<&Bone> {
        self.bones.iter().find(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// The IK handle with id `id`.
    pub fn handle(&self, id: u32) -> Option<&IkHandle> {
        self.ik.iter().find(|h| h.id == id)
    }

    /// The IK handle with id `id`, to change.
    pub fn handle_mut(&mut self, id: u32) -> Option<&mut IkHandle> {
        self.ik.iter_mut().find(|h| h.id == id)
    }

    /// An id no bone has yet.
    pub fn next_bone_id(&self) -> u32 {
        self.bones.iter().map(|b| b.id).max().map_or(1, |m| m + 1)
    }

    /// An id no IK handle has yet.
    pub fn next_ik_id(&self) -> u32 {
        self.ik.iter().map(|h| h.id).max().map_or(1, |m| m + 1)
    }

    /// Adds a bone and returns its id.
    pub fn add(&mut self, name: impl Into<String>, parent: Option<u32>, head: Vec2, tail: Vec2) -> u32 {
        let id = self.next_bone_id();
        self.bones.push(Bone::new(id, name, parent, head, tail));
        self.sort();
        id
    }

    /// Puts parents before their children (keeping the order otherwise),
    /// and cuts loops and parents that are not there.
    pub fn sort(&mut self) {
        let ids: Vec<u32> = self.bones.iter().map(|b| b.id).collect();
        for b in &mut self.bones {
            if b.parent.is_some_and(|p| !ids.contains(&p) || p == b.id) {
                b.parent = None;
            }
        }
        let mut out: Vec<Bone> = Vec::with_capacity(self.bones.len());
        let mut left = std::mem::take(&mut self.bones);
        while !left.is_empty() {
            let before = left.len();
            let mut i = 0;
            while i < left.len() {
                if left[i].parent.is_none_or(|p| out.iter().any(|o| o.id == p)) {
                    out.push(left.remove(i));
                } else {
                    i += 1;
                }
            }
            if left.len() == before {
                // A loop: break it at the first bone left.
                left[0].parent = None;
            }
        }
        self.bones = out;
    }

    /// The bones from `id` up to the root.
    pub fn chain_up(&self, id: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut at = Some(id);
        while let Some(b) = at.and_then(|i| self.bone(i)) {
            if out.contains(&b.id) {
                break;
            }
            out.push(b.id);
            at = b.parent;
        }
        out
    }

    /// The children of `id`.
    pub fn children(&self, id: u32) -> Vec<u32> {
        self.bones.iter().filter(|b| b.parent == Some(id)).map(|b| b.id).collect()
    }

    /// Whether any bone is turned or shifted from rest.
    pub fn posed_away(&self) -> bool {
        self.enabled && self.bones.iter().any(|b| b.rotation != 0.0 || b.offset != Vec2::ZERO)
    }

    fn px(&self) -> (Affine, Affine) {
        let a = self.aspect.max(1e-6);
        (Affine::scale(a, 1.0), Affine::scale(1.0 / a, 1.0))
    }

    /// Forward kinematics: every bone's map from the rotations and
    /// offsets as they stand (no IK).
    pub fn forward(&self) -> Posed {
        let (to, from) = self.px();
        let a = self.aspect.max(1e-6);
        let mut world: Vec<(u32, Affine)> = Vec::with_capacity(self.bones.len());
        let mut out = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let parent = b.parent.and_then(|p| world.iter().find(|w| w.0 == p)).map_or(Affine::IDENTITY, |w| w.1);
            let head = Vec2::new(b.head.x * a, b.head.y);
            let off = Vec2::new(b.offset.x * a, b.offset.y);
            // Turn about the rest head, then shift, then go where the
            // parent carries it.
            let local = Affine::shift(off).then_after(&Affine::turn_about(b.rotation, head));
            let w = parent.then_after(&local);
            // A stretched bone grows along itself; its children follow the
            // far end without being stretched themselves.
            let k = b.stretch;
            let (own, carry) = if (k - 1.0).abs() > 1e-9 {
                let tail = Vec2::new(b.tail.x * a, b.tail.y);
                let d = Vec2::new(tail.x - head.x, tail.y - head.y);
                let len = (d.x * d.x + d.y * d.y).sqrt().max(1e-12);
                let (u, v) = (Vec2::new(d.x / len, d.y / len), Vec2::new(-d.y / len, d.x / len));
                // Scale by k along u about the head.
                let m = [u.x * u.x * k + v.x * v.x, u.x * u.y * k + v.x * v.y, u.x * u.y * k + v.x * v.y, u.y * u.y * k + v.y * v.y];
                let sc = Affine([m[0], m[1], head.x - m[0] * head.x - m[1] * head.y, m[2], m[3], head.y - m[2] * head.x - m[3] * head.y]);
                (w.then_after(&sc), w.then_after(&Affine::shift(Vec2::new(d.x * (k - 1.0), d.y * (k - 1.0)))))
            } else {
                (w, w)
            };
            world.push((b.id, carry));
            let map = from.then_after(&own).then_after(&to);
            out.push(PosedBone { id: b.id, map, head: map.apply(b.head), tail: map.apply(b.tail), turn: w.angle() });
        }
        Posed { bones: out }
    }

    /// The pose with the IK handles solved: the rotations they set are
    /// written into the bones (within each bone's limits), blended with
    /// the bones' own by each handle's `blend`.
    pub fn solve(&mut self) {
        if !self.enabled {
            return;
        }
        let handles: Vec<IkHandle> = self.ik.iter().filter(|h| h.enabled && h.blend > 0.0).cloned().collect();
        for h in handles {
            let mut chain = self.chain_up(h.bone);
            chain.truncate(h.chain.max(1) as usize);
            chain.reverse(); // root of the chain first
            if chain.is_empty() {
                continue;
            }
            let own: Vec<f64> = chain.iter().map(|&id| self.bone(id).map_or(0.0, |b| b.rotation)).collect();
            let rig = h.pole.is_some() || h.stretch > 1.0 || h.soft > 0.0;
            let mut h = h;
            if chain.len() == 2 && h.pole.is_none() {
                h.flip = self.flip_for_limits(&chain, h.target, h.flip);
            }
            if rig {
                self.solve_rig(&chain, &h);
            } else if chain.len() == 2 {
                self.two_bone(&chain, h.target, h.flip);
            } else {
                self.ccd(&chain, h.target);
            }
            for (i, &id) in chain.iter().enumerate() {
                if let Some(b) = self.bone_mut(id) {
                    if rig {
                        b.stretch = 1.0 + (b.stretch - 1.0) * h.blend.clamp(0.0, 1.0);
                    }
                    let solved = b.rotation;
                    b.rotation = b.limit(own[i] + (wrap(solved - own[i])) * h.blend.clamp(0.0, 1.0));
                }
            }
        }
    }

    /// The posed skeleton (IK solved) without changing `self`.
    pub fn posed(&self) -> Posed {
        let mut s = self.clone();
        s.solve();
        s.forward()
    }

    /// World angle (pixels, clockwise degrees) of the segment `a`→`b`.
    fn angle_px(&self, a: Vec2, b: Vec2) -> f64 {
        ((b.y - a.y)).atan2((b.x - a.x) * self.aspect).to_degrees()
    }

    fn dist_px(&self, a: Vec2, b: Vec2) -> f64 {
        (((b.x - a.x) * self.aspect).powi(2) + (b.y - a.y).powi(2)).sqrt()
    }

    /// The world turn of bone `id` as posed (degrees).
    fn world_turn(posed: &Posed, id: u32) -> f64 {
        posed.bone(id).map_or(0.0, |b| b.turn)
    }

    /// Two bones (upper and lower) reaching for `target` by the law of
    /// cosines, bending the way they bend at rest (or the other way).
    fn two_bone(&mut self, chain: &[u32], target: Vec2, flip: bool) {
        let (Some(up), Some(low)) = (self.bone(chain[0]).cloned(), self.bone(chain[1]).cloned()) else { return };
        let posed = self.forward();
        let (Some(pu), Some(pl)) = (posed.bone(up.id), posed.bone(low.id)) else { return };
        let a = pu.head; // shoulder / hip, where it now is
        let l1 = self.dist_px(up.head, low.head);
        let l2 = self.dist_px(low.head, low.tail);
        if l1 < 1e-9 || l2 < 1e-9 {
            return self.ccd(chain, target);
        }
        let d = self.dist_px(a, target).clamp((l1 - l2).abs() + 1e-6, l1 + l2 - 1e-6);
        // Which way the joint bends at rest: the side of the elbow from the
        // shoulder-to-hand line.
        let cross = (low.head.x - up.head.x) * self.aspect * (low.tail.y - up.head.y) - (low.head.y - up.head.y) * (low.tail.x - up.head.x) * self.aspect;
        let side = if cross >= 0.0 { -1.0 } else { 1.0 } * if flip { -1.0 } else { 1.0 };
        let to_target = self.angle_px(a, target);
        let inner = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0).acos().to_degrees();
        let joint = ((l1 * l1 + l2 * l2 - d * d) / (2.0 * l1 * l2)).clamp(-1.0, 1.0).acos().to_degrees();
        let up_world = to_target + side * inner;
        let low_world = up_world - side * (180.0 - joint);
        // Local rotations: world angle of the segment = parent's world turn
        // + own rotation + the segment's rest angle.
        let parent_turn = up.parent.map_or(0.0, |p| Self::world_turn(&posed, p));
        let rest_up = self.angle_px(up.head, low.head);
        let r_up = up.limit(wrap(up_world - parent_turn - rest_up));
        if let Some(b) = self.bone_mut(up.id) {
            b.rotation = r_up;
        }
        let posed = self.forward();
        let turn_up = Self::world_turn(&posed, up.id);
        let _ = pl;
        let rest_low = self.angle_px(low.head, low.tail);
        // The lower bone's parent is usually the upper one; if not, its
        // own parent's turn.
        let parent_low = low.parent.map_or(0.0, |p| if p == up.id { turn_up } else { Self::world_turn(&posed, p) });
        let r_low = low.limit(wrap(low_world - parent_low - rest_low));
        if let Some(b) = self.bone_mut(low.id) {
            b.rotation = r_low;
        }
    }

    /// Which way a two-bone chain bends: `flip` as asked, unless that way
    /// needs a joint past its limits and the other does not.
    fn flip_for_limits(&self, chain: &[u32], target: Vec2, flip: bool) -> bool {
        if chain.iter().all(|&id| self.bone(id).is_none_or(|b| b.min.is_none() && b.max.is_none())) {
            return flip;
        }
        let past = |f: bool| {
            let mut free = self.clone();
            for &id in chain {
                if let Some(b) = free.bone_mut(id) {
                    b.min = None;
                    b.max = None;
                }
            }
            free.two_bone(chain, target, f);
            chain.iter().map(|&id| {
                let (r, b) = (free.bone(id).map_or(0.0, |b| b.rotation), self.bone(id));
                b.map_or(0.0, |b| (b.limit(r) - r).abs())
            }).sum::<f64>()
        };
        let (a, b) = (past(flip), past(!flip));
        if b + 1e-6 < a { !flip } else { flip }
    }

    /// The chain solved by the shared rig solvers (`studio_rig::ik`): the
    /// exact two-bone solution or FABRIK, with a pole, soft reach and
    /// stretch. Bone rotations are set within each bone's limits; stretch
    /// is written to the chain's bones.
    fn solve_rig(&mut self, chain: &[u32], h: &IkHandle) {
        use studio_rig::ik::{self, Reach};
        let a = self.aspect.max(1e-6);
        let px = |p: Vec2| [p.x * a, p.y];
        for &id in chain {
            if let Some(b) = self.bone_mut(id) {
                b.stretch = 1.0;
            }
        }
        let bones: Vec<Bone> = chain.iter().filter_map(|&id| self.bone(id).cloned()).collect();
        if bones.len() != chain.len() {
            return;
        }
        let posed = self.forward();
        let Some(root) = posed.bone(bones[0].id).map(|p| px(p.head)) else { return };
        // Rest lengths of each bone toward the next joint (the last to its tail).
        let rest_to = |i: usize| if i + 1 < bones.len() { bones[i + 1].head } else { bones[i].tail };
        let lens: Vec<f64> = (0..bones.len()).map(|i| self.dist_px(bones[i].head, rest_to(i))).collect();
        if lens.iter().any(|&l| l < 1e-9) {
            return self.ccd(chain, h.target);
        }
        let goal = px(h.target);
        let reach = Reach { stretch: h.stretch.max(1.0), soft: h.soft };
        let mut joints: Vec<[f64; 2]> = Vec::with_capacity(bones.len() + 1);
        let k;
        if bones.len() == 2 {
            // Bend toward the pole, or the way the bones bend at rest.
            let bend = match h.pole {
                Some(p) => studio_rig::v::sub(px(p), root),
                None => {
                    let cross = (bones[1].head.x - bones[0].head.x) * a * (bones[1].tail.y - bones[0].head.y) - (bones[1].head.y - bones[0].head.y) * (bones[1].tail.x - bones[0].head.x) * a;
                    let side = if cross >= 0.0 { -1.0 } else { 1.0 } * if h.flip { -1.0 } else { 1.0 };
                    let t = (goal[1] - root[1]).atan2(goal[0] - root[0]) + side * std::f64::consts::FRAC_PI_2;
                    [t.cos(), t.sin()]
                }
            };
            let s = ik::two_bone(root, lens[0], lens[1], goal, bend, reach);
            joints.extend([root, s.mid, s.end]);
            k = s.stretch;
        } else {
            joints.push(root);
            // Start from the pose as it stands.
            for i in 0..bones.len() {
                let p = posed.bone(bones[i].id).unwrap();
                joints.push(px(if i + 1 < bones.len() { posed.bone(bones[i + 1].id).unwrap().head } else { p.tail }));
            }
            // Bones of an FK pose share joints only if they hang end to end;
            // otherwise lay the chain out along its current directions.
            let mut at = root;
            for i in 0..bones.len() {
                let d = studio_rig::v::sub(joints[i + 1], joints[i]);
                let l = studio_rig::v::len(d);
                let dir = if l > 1e-9 { studio_rig::v::scale(d, 1.0 / l) } else { [1.0, 0.0] };
                at = studio_rig::v::add(at, studio_rig::v::scale(dir, lens[i]));
                joints[i + 1] = at;
            }
            let pole = h.pole.map(px);
            k = ik::fabrik(&mut joints, goal, pole, reach, 1e-5, 32).stretch;
        }
        for i in 0..bones.len() {
            let seg = studio_rig::v::sub(joints[i + 1], joints[i]);
            let world = seg[1].atan2(seg[0]).to_degrees();
            let posed = self.forward();
            let parent_turn = bones[i].parent.map_or(0.0, |p| Self::world_turn(&posed, p));
            let rest = self.angle_px(bones[i].head, rest_to(i));
            if let Some(b) = self.bone_mut(bones[i].id) {
                b.rotation = b.limit(wrap(world - parent_turn - rest));
                b.stretch = k;
            }
        }
    }

    /// Cyclic coordinate descent for chains of any length: each bone in
    /// turn, from the end up, turned to point the end at the target.
    fn ccd(&mut self, chain: &[u32], target: Vec2) {
        let Some(&end) = chain.last() else { return };
        let Some(tail) = self.bone(end).map(|b| b.tail) else { return };
        for _ in 0..24 {
            for &id in chain.iter().rev() {
                let posed = self.forward();
                let (Some(pb), Some(pe)) = (posed.bone(id), posed.bone(end)) else { return };
                let tip = pe.map.apply(tail);
                if self.dist_px(tip, target) < 1e-4 {
                    return;
                }
                let delta = wrap(self.angle_px(pb.head, target) - self.angle_px(pb.head, tip));
                if let Some(b) = self.bone_mut(id) {
                    b.rotation = b.limit(b.rotation + delta);
                }
            }
        }
    }

    /// The current rotations and offsets as a pose called `name`.
    pub fn pose_now(&self, name: impl Into<String>) -> Pose {
        let mut p = Pose { name: name.into(), ..Pose::default() };
        for b in &self.bones {
            let key = if b.name.is_empty() { b.id.to_string() } else { b.name.clone() };
            p.rotations.insert(key.clone(), b.rotation);
            if b.offset != Vec2::ZERO {
                p.offsets.insert(key, b.offset);
            }
        }
        p
    }

    /// The rotation (and offset) a pose gives each bone it names, matched
    /// by name (any case) or id: `(bone id, rotation, offset)`.
    pub fn pose_targets(&self, pose: &Pose) -> Vec<(u32, f64, Option<Vec2>)> {
        let mut out = Vec::new();
        for b in &self.bones {
            let key = |m: &BTreeMap<String, f64>| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(&b.name) || **k == b.id.to_string()).map(|(_, v)| *v);
            let off = pose.offsets.iter().find(|(k, _)| k.eq_ignore_ascii_case(&b.name) || **k == b.id.to_string()).map(|(_, v)| *v);
            if let Some(r) = key(&pose.rotations) {
                out.push((b.id, b.limit(r), off));
            }
        }
        out
    }
}

/// An angle brought into -180..180.
pub fn wrap(deg: f64) -> f64 {
    let d = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if d == -180.0 { 180.0 } else { d }
}

/// How a part layer follows one bone of its parent's skeleton
/// ([`crate::Parent::bone`]): the bone's motion from rest, carried into
/// the parent's resting frame on the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoneLink {
    /// The parent's bone followed.
    pub bone: u32,
    /// Where the parent's layer point (0, 0) sat when they were linked, in
    /// the parent's resting frame: canvas pixels from its position, before
    /// its rotation.
    pub origin: Vec2,
    /// How far (1, 1) is from (0, 0) in that frame (negative when flipped).
    pub span: Vec2,
}

impl BoneLink {
    /// The bone's motion (rest → posed, layer coordinates) as a map in the
    /// parent's resting frame.
    pub fn carry(&self, map: &Affine) -> Affine {
        let sx = if self.span.x.abs() < 1e-9 { 1e-9 } else { self.span.x };
        let sy = if self.span.y.abs() < 1e-9 { 1e-9 } else { self.span.y };
        let to = Affine([sx, 0.0, self.origin.x, 0.0, sy, self.origin.y]);
        let from = Affine([1.0 / sx, 0.0, -self.origin.x / sx, 0.0, 1.0 / sy, -self.origin.y / sy]);
        to.then_after(map).then_after(&from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm() -> Skeleton {
        // A shoulder at the left, elbow in the middle bent slightly down,
        // hand at the right, on a square picture.
        let mut s = Skeleton::default();
        let up = s.add("upper_arm", None, Vec2::new(0.1, 0.5), Vec2::new(0.5, 0.52));
        s.add("forearm", Some(up), Vec2::new(0.5, 0.52), Vec2::new(0.9, 0.5));
        s
    }

    #[test]
    fn forward_kinematics_carries_children() {
        let mut s = arm();
        s.bones[0].rotation = 90.0; // clockwise: the arm swings down
        let p = s.forward();
        let fore = p.bone(2).unwrap();
        assert!((fore.head.x - 0.08).abs() < 1e-6 && (fore.head.y - 0.9).abs() < 1e-6, "{:?}", fore.head);
        assert!(fore.tail.y > 0.85 && (fore.tail.x - 0.1).abs() < 0.05, "{:?}", fore.tail);
        // On a wide picture the turn is rigid in pixels.
        let mut w = arm();
        w.aspect = 2.0;
        w.bones[0].rotation = 90.0;
        let p = w.forward();
        let h = p.bone(2).unwrap().head;
        // 0.4 across at aspect 2 is 0.8 heights: straight down by 0.8.
        assert!((h.y - (0.5 + 0.8)).abs() < 0.02 && (h.x - 0.1).abs() < 0.02, "{h:?}");
    }

    #[test]
    fn ik_reaches_and_respects_limits_and_bend() {
        let mut s = arm();
        s.ik.push(IkHandle { id: 1, name: "hand".into(), bone: 2, chain: 2, target: Vec2::new(0.5, 0.2), blend: 1.0, flip: false, enabled: true, ..IkHandle::default() });
        let p = s.posed();
        let tip = p.bone(2).unwrap().tail;
        assert!((tip.x - 0.5).abs() < 1e-3 && (tip.y - 0.2).abs() < 1e-3, "reaches {tip:?}");
        // The elbow bends the way it does at rest (down, below the line).
        let elbow = p.bone(2).unwrap().head;
        let flipped = {
            let mut f = s.clone();
            f.ik[0].flip = true;
            f.posed().bone(2).unwrap().head
        };
        assert!((elbow.x - flipped.x).abs() > 0.05, "{elbow:?} vs {flipped:?}");
        // Out of reach: the arm points straight at it.
        s.ik[0].target = Vec2::new(2.0, 0.5);
        let p = s.posed();
        assert!(p.bone(2).unwrap().tail.x > 0.85);
        // A limit stops the forearm.
        let mut l = arm();
        l.bones[1].min = Some(-10.0);
        l.bones[1].max = Some(10.0);
        l.ik.push(IkHandle { id: 1, name: String::new(), bone: 2, chain: 2, target: Vec2::new(0.2, 0.6), blend: 1.0, flip: false, enabled: true, ..IkHandle::default() });
        let mut solved = l.clone();
        solved.solve();
        assert!(solved.bones[1].rotation.abs() <= 10.0 + 1e-9);
        // Half blend lands between.
        let mut h = arm();
        h.ik.push(IkHandle { id: 1, name: String::new(), bone: 2, chain: 2, target: Vec2::new(0.5, 0.2), blend: 0.5, flip: false, enabled: true, ..IkHandle::default() });
        h.solve();
        let mut full = arm();
        full.ik.push(IkHandle { id: 1, name: String::new(), bone: 2, chain: 2, target: Vec2::new(0.5, 0.2), blend: 1.0, flip: false, enabled: true, ..IkHandle::default() });
        full.solve();
        assert!((h.bones[0].rotation - full.bones[0].rotation / 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_pole_picks_the_bend_and_stretch_covers_far_targets() {
        let mut s = arm();
        let mut h = IkHandle { id: 1, bone: 2, target: Vec2::new(0.5, 0.5), ..IkHandle::default() };
        h.pole = Some(Vec2::new(0.5, 0.0));
        s.ik.push(h);
        let p = s.posed();
        let tip = p.bone(2).unwrap().tail;
        assert!((tip.x - 0.5).abs() < 1e-3 && (tip.y - 0.5).abs() < 1e-3, "{tip:?}");
        assert!(p.bone(2).unwrap().head.y < 0.45, "elbow bends toward the pole above");
        s.ik[0].pole = Some(Vec2::new(0.5, 1.0));
        assert!(s.posed().bone(2).unwrap().head.y > 0.55);
        // Far target: rigid points straight at it; stretch reaches it.
        s.ik[0].target = Vec2::new(1.2, 0.5);
        let rigid = s.posed().bone(2).unwrap().tail;
        s.ik[0].stretch = 1.6;
        let long = s.posed().bone(2).unwrap().tail;
        assert!(long.x > rigid.x + 0.1 && (long.x - 1.2).abs() < 1e-3, "{rigid:?} {long:?}");
        // Joint limits still hold; old projects load without the new fields.
        s.bones[1].min = Some(-5.0);
        s.bones[1].max = Some(5.0);
        s.solve();
        assert!(s.bones[1].rotation.abs() <= 5.0 + 1e-9);
        let old: IkHandle = serde_json::from_str(r#"{"id":1,"bone":2,"target":{"x":0.1,"y":0.2}}"#).unwrap();
        assert!(old.pole.is_none() && old.stretch == 1.0);
        // Long chains use FABRIK with a stretch.
        let mut t = Skeleton::default();
        let mut parent = None;
        for i in 0..4 {
            let x = 0.1 + 0.1 * i as f64;
            parent = Some(t.add(format!("tail.{i}"), parent, Vec2::new(x, 0.5), Vec2::new(x + 0.1, 0.5)));
        }
        t.ik.push(IkHandle { bone: parent.unwrap(), chain: 4, target: Vec2::new(0.6, 0.3), stretch: 1.5, ..IkHandle { id: 1, ..IkHandle::default() } });
        let tip = t.posed().bone(parent.unwrap()).unwrap().tail;
        assert!((tip.x - 0.6).abs() < 1e-3 && (tip.y - 0.3).abs() < 1e-3, "{tip:?}");
    }

    #[test]
    fn long_chains_use_ccd_and_poses_round_trip() {
        let mut s = Skeleton::default();
        let mut parent = None;
        for i in 0..4 {
            let x = 0.1 + 0.2 * i as f64;
            parent = Some(s.add(format!("tail.{i}"), parent, Vec2::new(x, 0.5), Vec2::new(x + 0.2, 0.5)));
        }
        s.ik.push(IkHandle { id: 1, name: String::new(), bone: parent.unwrap(), chain: 4, target: Vec2::new(0.6, 0.1), blend: 1.0, flip: false, enabled: true, ..IkHandle::default() });
        let tip = s.posed().bone(parent.unwrap()).unwrap().tail;
        assert!((tip.x - 0.6).abs() < 0.01 && (tip.y - 0.1).abs() < 0.01, "{tip:?}");
        let mut a = arm();
        a.bones[0].rotation = 30.0;
        a.bones[0].offset = Vec2::new(0.1, 0.0);
        let pose = a.pose_now("reach");
        let b = arm();
        let t = b.pose_targets(&pose);
        assert_eq!(t[0], (1, 30.0, Some(Vec2::new(0.1, 0.0))));
        assert_eq!(t[1].1, 0.0);
        let back: Skeleton = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn sorting_puts_parents_first_and_breaks_loops() {
        let mut s = Skeleton::default();
        s.bones.push(Bone::new(2, "b", Some(1), Vec2::ZERO, Vec2::ONE));
        s.bones.push(Bone::new(1, "a", None, Vec2::ZERO, Vec2::ONE));
        s.bones.push(Bone::new(3, "c", Some(4), Vec2::ZERO, Vec2::ONE));
        s.bones.push(Bone::new(4, "d", Some(3), Vec2::ZERO, Vec2::ONE));
        s.sort();
        assert_eq!(s.bones.iter().map(|b| b.id).collect::<Vec<_>>()[..2], [1, 2]);
        assert_eq!(s.bones.len(), 4);
        assert!(s.bones.iter().any(|b| b.parent.is_none() && (b.id == 3 || b.id == 4)));
    }

    #[test]
    fn a_part_layer_follows_its_bone_on_the_timeline() {
        use crate::{Clip, ClipId, ClipSource, Generator, GeneratorKind, LaneId, Parent, Sequence, SequenceId};
        let mut seq = Sequence::new(SequenceId(1), "s", LaneId(2));
        let solid = || ClipSource::Generator(Generator::new(GeneratorKind::Solid));
        let mut rig = Clip::new(ClipId(1), LaneId(2), 0, 1_000_000, solid());
        let mut sk = Skeleton::default();
        let arm = sk.add("upper_arm", None, Vec2::new(0.5, 0.5), Vec2::new(0.5, 0.9));
        rig.skeleton = Some(sk);
        // The rig's picture spans 200 × 200 canvas pixels around its
        // position (0, 0); the part sits at the arm's tip.
        let mut part = Clip::new(ClipId(2), LaneId(2), 0, 1_000_000, solid());
        part.transform.position = Vec2::new(0.0, 80.0);
        part.parent = Some(Parent { clip: ClipId(1), rest_position: Vec2::ZERO, rest_scale: Vec2::ONE, rest_rotation: 0.0, rest_z: 0.0, rest_rotation_x: 0.0, rest_rotation_y: 0.0, bone: Some(BoneLink { bone: arm, origin: Vec2::new(-100.0, -100.0), span: Vec2::new(200.0, 200.0) }) });
        seq.clips = vec![rig, part];
        let at_rest = seq.placed_transform(&seq.clips[1], 0);
        assert!((at_rest.position.y - 80.0).abs() < 1e-9 && at_rest.rotation.abs() < 1e-9);
        // The arm swings a quarter turn clockwise: its tip, and the part,
        // go from below the joint to its left.
        seq.clips[0].skeleton.as_mut().unwrap().bones[0].rotation = 90.0;
        let swung = seq.placed_transform(&seq.clips[1], 0);
        assert!((swung.position.x + 80.0).abs() < 1e-6 && swung.position.y.abs() < 1e-6, "{:?}", swung.position);
        assert!((swung.rotation - 90.0).abs() < 1e-9);
        // Bones off: the part stays where it was put.
        seq.clips[0].skeleton.as_mut().unwrap().enabled = false;
        assert!((seq.placed_transform(&seq.clips[1], 0).position.y - 80.0).abs() < 1e-9);
    }

    #[test]
    fn a_link_carries_the_bone_into_the_canvas_frame() {
        let link = BoneLink { bone: 1, origin: Vec2::new(-100.0, -100.0), span: Vec2::new(200.0, 200.0) };
        // A quarter turn about the layer's middle.
        let m = Affine::turn_about(90.0, Vec2::new(0.5, 0.5));
        let c = link.carry(&m);
        let p = c.apply(Vec2::new(100.0, 0.0));
        assert!((p.x - 0.0).abs() < 1e-9 && (p.y - 100.0).abs() < 1e-9, "{p:?}");
        assert!((c.angle() - 90.0).abs() < 1e-9);
    }
}
