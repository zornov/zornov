// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rigs: what makes a model move like a character.
//!
//! A [`Rig`] rides on a [`Mesh`] and holds
//! - an armature: [`Bone`]s with parents, each a head (the joint it turns
//!   about) and a tail, in the model's pixels;
//! - skin weights: how much each bone carries each vertex (linear blend
//!   skinning, up to four bones a vertex);
//! - shape keys ([`ShapeKey`]): sculpted offsets of vertices mixed in by a
//!   weight (a smile, a blink, a viseme), applied before the skin;
//! - [`Constraint`]s: IK chains (a hand or foot placed, the limb follows,
//!   the knee aiming at a pole), look-at, rotation limits, copied
//!   rotations, solved in order after the bones' own rotations;
//! - [`Action`]s: recorded motion (a walk, mocap, a glTF animation) as
//!   keyed tracks per bone and shape key.
//!
//! Bone frames: each bone's Y axis runs from head to tail, its Z axis
//! points the way the character faces (-z in the world at rest, or down
//! for a bone that already points that way), X = Y × Z, then turned by
//! the bone's roll. So a bone's rotation in degrees `[x, y, z]` reads as
//! x: swing forward (+) or back (−); y: twist along the bone; z: swing
//! sideways. Poses are relative to the rest pose; rest is the pose the
//! mesh was modelled in.
//!
//! The method is the textbook one (forward kinematics, linear blend
//! skinning, analytic two-bone IK with a pole, FABRIK with a pole for
//! longer chains); no engine's code is copied.

use crate::{Mesh, V3, add, cross, dot, length, normalise, scale, sub};
use std::collections::HashMap;

// ------------------------------------------------------------------ math

/// A rotation as a unit quaternion `[x, y, z, w]`.
pub type Quat = [f32; 4];

pub const Q_ID: Quat = [0.0, 0.0, 0.0, 1.0];

pub fn q_mul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

pub fn q_conj(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

pub fn q_normalise(q: Quat) -> Quat {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l > 1e-12 { [q[0] / l, q[1] / l, q[2] / l, q[3] / l] } else { Q_ID }
}

pub fn q_rotate(q: Quat, v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let t = scale(cross(u, v), 2.0);
    add(add(v, scale(t, q[3])), cross(u, t))
}

/// A turn of `angle` radians about `axis` (right-handed).
pub fn q_axis_angle(axis: V3, angle: f32) -> Quat {
    let a = normalise(axis);
    if a == [0.0; 3] {
        return Q_ID;
    }
    let (s, c) = (angle * 0.5).sin_cos();
    [a[0] * s, a[1] * s, a[2] * s, c]
}

/// The smallest turn taking direction `a` to direction `b`.
pub fn q_from_to(a: V3, b: V3) -> Quat {
    let (a, b) = (normalise(a), normalise(b));
    let d = dot(a, b);
    if d > 0.999_999 {
        return Q_ID;
    }
    if d < -0.999_999 {
        // Half a turn about any axis square to a.
        let mut axis = cross([1.0, 0.0, 0.0], a);
        if length(axis) < 1e-3 {
            axis = cross([0.0, 1.0, 0.0], a);
        }
        return q_axis_angle(axis, std::f32::consts::PI);
    }
    let c = cross(a, b);
    q_normalise([c[0], c[1], c[2], 1.0 + d])
}

pub fn q_slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let b = if d < 0.0 {
        d = -d;
        [-b[0], -b[1], -b[2], -b[3]]
    } else {
        b
    };
    if d > 0.9995 {
        return q_normalise(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
    }
    let th = d.clamp(-1.0, 1.0).acos();
    let s = th.sin();
    let (ka, kb) = (((1.0 - t) * th).sin() / s, (t * th).sin() / s);
    std::array::from_fn(|i| a[i] * ka + b[i] * kb)
}

/// Degrees `[x, y, z]` about a bone's own axes, X applied first.
pub fn q_from_euler(deg: V3) -> Quat {
    let r = deg.map(f32::to_radians);
    let qx = q_axis_angle([1.0, 0.0, 0.0], r[0]);
    let qy = q_axis_angle([0.0, 1.0, 0.0], r[1]);
    let qz = q_axis_angle([0.0, 0.0, 1.0], r[2]);
    q_mul(qz, q_mul(qy, qx))
}

/// The degrees [`q_from_euler`] takes back (X first, then Y, then Z).
pub fn q_to_euler(q: Quat) -> V3 {
    let m = q_to_mat(q);
    // R = Rz·Ry·Rx: m[2][0] = −sin y.
    let sy = (-m[2][0]).clamp(-1.0, 1.0);
    let y = sy.asin();
    let (x, z) = if sy.abs() < 0.9999 { (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0])) } else { ((-m[1][2]).atan2(m[1][1]), 0.0) };
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// Rows of the rotation matrix.
pub fn q_to_mat(q: Quat) -> [V3; 3] {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// The rotation whose columns are `x`, `y`, `z` (orthonormal).
pub fn q_from_axes(x: V3, y: V3, z: V3) -> Quat {
    // Rows of the matrix with those columns.
    let m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
    let tr = m[0][0] + m[1][1] + m[2][2];
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).max(1e-12).sqrt() * 2.0;
        [0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).max(1e-12).sqrt() * 2.0;
        [(m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).max(1e-12).sqrt() * 2.0;
        [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s]
    };
    q_normalise(q)
}

/// A rigid move with a uniform scale: `p ↦ pos + rot·(scale·p)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub rot: Quat,
    pub pos: V3,
    pub scale: f32,
}

impl Default for Xf {
    fn default() -> Xf {
        Xf::IDENTITY
    }
}

impl Xf {
    pub const IDENTITY: Xf = Xf { rot: Q_ID, pos: [0.0; 3], scale: 1.0 };

    pub fn new(rot: Quat, pos: V3) -> Xf {
        Xf { rot, pos, scale: 1.0 }
    }
    pub fn apply(&self, p: V3) -> V3 {
        add(self.pos, q_rotate(self.rot, scale(p, self.scale)))
    }
    pub fn apply_dir(&self, v: V3) -> V3 {
        q_rotate(self.rot, v)
    }
    /// `self` after `o`: `(self ∘ o)(p) = self(o(p))`.
    pub fn then(&self, o: &Xf) -> Xf {
        Xf { rot: q_normalise(q_mul(self.rot, o.rot)), pos: self.apply(o.pos), scale: self.scale * o.scale }
    }
    pub fn inverse(&self) -> Xf {
        let r = q_conj(self.rot);
        let s = if self.scale.abs() > 1e-12 { 1.0 / self.scale } else { 1.0 };
        Xf { rot: r, pos: scale(q_rotate(r, self.pos), -s), scale: s }
    }
    /// Rows of the 3×4 matrix.
    pub fn matrix(&self) -> [[f32; 4]; 3] {
        let m = q_to_mat(self.rot);
        std::array::from_fn(|r| [m[r][0] * self.scale, m[r][1] * self.scale, m[r][2] * self.scale, self.pos[r]])
    }
    pub fn lerp(&self, o: &Xf, t: f32) -> Xf {
        Xf { rot: q_slerp(self.rot, o.rot, t), pos: crate::lerp(self.pos, o.pos, t), scale: self.scale + (o.scale - self.scale) * t }
    }
}

// ------------------------------------------------------------------ rig

/// One bone of an armature.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    /// Unique in its rig; keys and the AI name bones by it. Humanoid rigs
    /// use `hips`, `spine`, `chest`, `neck`, `head`, `shoulder.L`,
    /// `upper_arm.L`, `forearm.L`, `hand.L`, `thigh.L`, `shin.L`,
    /// `foot.L`, `toe.L` (and `.R`): the same roles as Studio's 2D
    /// skeletons, so motions and mocap carry over by name.
    pub name: String,
    /// Index of the bone it hangs from (always earlier in the list).
    pub parent: Option<usize>,
    /// The joint it turns about, at rest (model px).
    pub head: V3,
    /// Its other end at rest.
    pub tail: V3,
    /// Turn about its own length, radians.
    pub roll: f32,
    /// Whether it carries skin (off for control bones such as IK targets).
    pub deform: bool,
}

impl Bone {
    pub fn new(name: impl Into<String>, parent: Option<usize>, head: V3, tail: V3) -> Bone {
        Bone { name: name.into(), parent, head, tail, roll: 0.0, deform: true }
    }

    pub fn length(&self) -> f32 {
        length(sub(self.tail, self.head))
    }

    /// The bone's frame at rest in the model: origin at the head, axes as
    /// the module explains.
    pub fn rest_frame(&self) -> Xf {
        let y = normalise(sub(self.tail, self.head));
        let y = if y == [0.0; 3] { [0.0, -1.0, 0.0] } else { y };
        let reference = if dot(y, [0.0, 0.0, -1.0]).abs() > 0.95 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, -1.0] };
        let z = normalise(sub(reference, scale(y, dot(reference, y))));
        let x = cross(y, z);
        let base = q_from_axes(x, y, z);
        Xf::new(q_mul(base, q_axis_angle([0.0, 1.0, 0.0], self.roll)), self.head)
    }
}

/// A bone's pose relative to rest, in its own frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BonePose {
    pub rot: Quat,
    /// Shift of the head along the bone's own axes (px).
    pub loc: V3,
    pub scale: f32,
}

impl Default for BonePose {
    fn default() -> BonePose {
        BonePose { rot: Q_ID, loc: [0.0; 3], scale: 1.0 }
    }
}

impl BonePose {
    pub fn xf(&self) -> Xf {
        Xf { rot: self.rot, pos: self.loc, scale: self.scale }
    }
    pub fn mix(&self, o: &BonePose, t: f32) -> BonePose {
        BonePose { rot: q_slerp(self.rot, o.rot, t), loc: crate::lerp(self.loc, o.loc, t), scale: self.scale + (o.scale - self.scale) * t }
    }
    /// `o` added on top of this pose (rotation after, offsets summed).
    pub fn layered(&self, o: &BonePose) -> BonePose {
        BonePose { rot: q_normalise(q_mul(self.rot, o.rot)), loc: add(self.loc, o.loc), scale: self.scale * o.scale }
    }
}

/// Offsets of some vertices, mixed in by a weight.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeKey {
    pub name: String,
    /// Which vertices move (ascending).
    pub indices: Vec<u32>,
    pub deltas: Vec<V3>,
}

impl ShapeKey {
    /// A key from full per-vertex offsets (tiny ones dropped).
    pub fn from_dense(name: impl Into<String>, deltas: &[V3]) -> ShapeKey {
        let mut k = ShapeKey { name: name.into(), indices: Vec::new(), deltas: Vec::new() };
        for (i, d) in deltas.iter().enumerate() {
            if length(*d) > 1e-4 {
                k.indices.push(i as u32);
                k.deltas.push(*d);
            }
        }
        k
    }
}

/// Where a constraint aims.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A point in the model's pixels.
    Point(V3),
    /// A bone's head, posed.
    Bone(usize),
}

/// Rules solved after the bones' own rotations, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Constraint {
    /// Turns a chain so its last bone's tail reaches the target; `pole`
    /// (model px) is where the middle joint points (knee forward, elbow
    /// back).
    Ik { name: String, bone: usize, chain: usize, target: Target, pole: Option<V3>, influence: f32 },
    /// Turns the bone so it points at the target (heads, eyes, cameras on
    /// a rig).
    LookAt { name: String, bone: usize, target: Target, influence: f32 },
    /// Keeps a bone's rotation within degrees `[x, y, z]` from rest.
    Limit { name: String, bone: usize, min: V3, max: V3 },
    /// The bone turns as another bone does (in their own frames).
    CopyRotation { name: String, bone: usize, from: usize, influence: f32 },
}

impl Constraint {
    pub fn name(&self) -> &str {
        match self {
            Constraint::Ik { name, .. } | Constraint::LookAt { name, .. } | Constraint::Limit { name, .. } | Constraint::CopyRotation { name, .. } => name,
        }
    }
    pub fn bone(&self) -> usize {
        match self {
            Constraint::Ik { bone, .. } | Constraint::LookAt { bone, .. } | Constraint::Limit { bone, .. } | Constraint::CopyRotation { bone, .. } => *bone,
        }
    }
    /// Where it runs in a solve: IK, copies, limits, look-at.
    pub fn stage(&self) -> u8 {
        match self {
            Constraint::Ik { .. } => 0,
            Constraint::CopyRotation { .. } => 1,
            Constraint::Limit { .. } => 2,
            Constraint::LookAt { .. } => 3,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Constraint::Ik { .. } => "ik",
            Constraint::LookAt { .. } => "look_at",
            Constraint::Limit { .. } => "limit",
            Constraint::CopyRotation { .. } => "copy_rotation",
        }
    }
}

/// What a track moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Rotation(usize),
    Location(usize),
    Scale(usize),
    Shape(usize),
}

/// Keys of one channel. Rotations are quaternions; locations use the
/// first three values; scales and shape weights the first.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub channel: Channel,
    /// Seconds, ascending.
    pub times: Vec<f32>,
    pub values: Vec<[f32; 4]>,
    /// Holds each key until the next (no in-betweens).
    pub step: bool,
}

impl Track {
    pub fn sample(&self, t: f32) -> [f32; 4] {
        let n = self.times.len();
        if n == 0 {
            return [0.0; 4];
        }
        if n == 1 || t <= self.times[0] {
            return self.values[0];
        }
        if t >= self.times[n - 1] {
            return self.values[n - 1];
        }
        let i = self.times.partition_point(|&k| k <= t).max(1) - 1;
        let (a, b) = (self.values[i], self.values[i + 1]);
        if self.step {
            return a;
        }
        let span = (self.times[i + 1] - self.times[i]).max(1e-9);
        let u = (t - self.times[i]) / span;
        match self.channel {
            Channel::Rotation(_) => q_slerp(a, b, u),
            _ => std::array::from_fn(|k| a[k] + (b[k] - a[k]) * u),
        }
    }
}

/// Recorded motion.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    /// Seconds; it loops over this length.
    pub duration: f32,
    pub tracks: Vec<Track>,
}

impl Action {
    /// The pose `t` seconds in (looped when `looped`), as offsets per bone
    /// and weights per shape key.
    pub fn sample(&self, bones: usize, shapes: usize, t: f32, looped: bool) -> (Vec<BonePose>, Vec<Option<f32>>) {
        let t = if looped && self.duration > 1e-6 { t.rem_euclid(self.duration) } else { t.clamp(0.0, self.duration.max(0.0)) };
        let mut pose = vec![BonePose::default(); bones];
        let mut w = vec![None; shapes];
        for tr in &self.tracks {
            let v = tr.sample(t);
            match tr.channel {
                Channel::Rotation(b) if b < bones => pose[b].rot = q_normalise(v),
                Channel::Location(b) if b < bones => pose[b].loc = [v[0], v[1], v[2]],
                Channel::Scale(b) if b < bones => pose[b].scale = v[0],
                Channel::Shape(s) if s < shapes => w[s] = Some(v[0]),
                _ => {}
            }
        }
        (pose, w)
    }

    /// An action from poses sampled `fps` times a second (rotations and
    /// root shifts), dropping channels that never move.
    pub fn from_frames(name: impl Into<String>, fps: f32, frames: &[Vec<BonePose>]) -> Action {
        let n = frames.first().map_or(0, Vec::len);
        let times: Vec<f32> = (0..frames.len()).map(|i| i as f32 / fps.max(1e-3)).collect();
        let mut tracks = Vec::new();
        for b in 0..n {
            let rots: Vec<[f32; 4]> = frames.iter().map(|f| f[b].rot).collect();
            if rots.iter().any(|q| (q[3].abs() - 1.0).abs() > 1e-5) {
                tracks.push(Track { channel: Channel::Rotation(b), times: times.clone(), values: rots, step: false });
            }
            let locs: Vec<[f32; 4]> = frames.iter().map(|f| [f[b].loc[0], f[b].loc[1], f[b].loc[2], 0.0]).collect();
            if locs.iter().any(|l| l[0].abs() + l[1].abs() + l[2].abs() > 1e-4) {
                tracks.push(Track { channel: Channel::Location(b), times: times.clone(), values: locs, step: false });
            }
        }
        let duration = if frames.len() > 1 { (frames.len() - 1) as f32 / fps.max(1e-3) } else { 0.0 };
        Action { name: name.into(), duration, tracks }
    }
}

/// An armature with its skin, shape keys, constraints and motion.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Rig {
    /// Parents before children.
    pub bones: Vec<Bone>,
    /// Per vertex: up to four bones and how much each carries (summing
    /// to 1). Empty until the skin is bound.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub shapes: Vec<ShapeKey>,
    pub constraints: Vec<Constraint>,
    pub actions: Vec<Action>,
    /// Skin with dual quaternions (`studio_rig::dq`) instead of blended
    /// matrices: no collapsing "candy wrapper" at twisted joints.
    pub dual_quat: bool,
}

/// What to pose a rig with at one instant: anything left out stays at
/// rest (or as the action has it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseInput {
    /// An action and where in it, looped or held at the ends.
    pub action: Option<(String, f32, bool)>,
    /// A second action mixed over the first by a weight (walk into run).
    pub blend: Option<(String, f32, bool, f32)>,
    /// Bone rotations in degrees `[x, y, z]`, added on top of the action.
    pub rotations: HashMap<String, V3>,
    /// Shifts of bone heads (px, the bone's own axes), added on top.
    pub offsets: HashMap<String, V3>,
    /// Shape key weights (overriding an action's).
    pub shapes: HashMap<String, f32>,
    /// Constraint targets by constraint name (model px).
    pub targets: HashMap<String, V3>,
    /// Constraint influences by name, 0..1.
    pub influences: HashMap<String, f32>,
}

impl PoseInput {
    pub fn is_rest(&self) -> bool {
        self.action.is_none() && self.blend.is_none() && self.rotations.is_empty() && self.offsets.is_empty() && self.shapes.is_empty() && self.targets.is_empty() && self.influences.is_empty()
    }
}

/// A rig worked out at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Posed {
    /// Each bone's frame in the model, posed.
    pub world: Vec<Xf>,
    /// Rest → posed per bone (what moves the skin).
    pub skin: Vec<Xf>,
    /// The bones' local poses after constraints.
    pub local: Vec<BonePose>,
    pub shapes: Vec<f32>,
}

impl Rig {
    /// A bone by name: the exact name first, then the same name in any
    /// case or spelling (`upper_arm.l`, `UpperArm.L`, `left_upperarm`,
    /// `mixamorig:LeftArm`) and by role (`arm` is the upper arm,
    /// `calf` the shin), see [`Rig::resolve_bone`].
    pub fn bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name).or_else(|| self.resolve_bone(name).ok())
    }

    /// A bone by name, forgiving of case, separators, left/right spelling
    /// and common aliases; on failure the error lists the rig's real bone
    /// names and the nearest ones.
    pub fn resolve_bone(&self, name: &str) -> Result<usize, String> {
        if let Some(i) = self.bones.iter().position(|b| b.name == name) {
            return Ok(i);
        }
        let lower = name.trim().to_ascii_lowercase();
        let same = |f: &dyn Fn(&str) -> bool| -> Option<usize> {
            let hits: Vec<usize> = (0..self.bones.len()).filter(|&i| f(&self.bones[i].name)).collect();
            (hits.len() == 1).then(|| hits[0])
        };
        if let Some(i) = same(&|b| b.to_ascii_lowercase() == lower) {
            return Ok(i);
        }
        let key = crate::rigfit::name_key(name);
        if let Some(i) = same(&|b| crate::rigfit::name_key(b) == key) {
            return Ok(i);
        }
        if let Some(role) = studio_rig::names::role(name)
            && let Some(i) = same(&|b| studio_rig::names::role(b).as_deref() == Some(role.as_str()))
        {
            return Ok(i);
        }
        Err(self.unknown_bone(name))
    }

    /// The error for a bone name the rig does not have: the real names
    /// and the closest ones.
    pub fn unknown_bone(&self, name: &str) -> String {
        let names: Vec<&str> = self.bones.iter().map(|b| b.name.as_str()).collect();
        let key = crate::rigfit::name_key(name);
        let mut near: Vec<(&str, usize)> = names.iter().map(|n| (*n, crate::rigfit::distance(&crate::rigfit::name_key(n), &key))).collect();
        near.sort_by_key(|x| x.1);
        let near: Vec<&str> = near.iter().take(3).map(|x| x.0).collect();
        format!("no bone \"{name}\"; closest: {}; all bones: {} (names match in any case, and left/right in any spelling, so upper_arm.L = UpperArm.l = left_upperarm)", near.join(", "), names.join(", "))
    }

    /// A shape key by name (exact, else in any case).
    pub fn shape(&self, name: &str) -> Option<usize> {
        self.shapes.iter().position(|s| s.name == name).or_else(|| self.shapes.iter().position(|s| s.name.eq_ignore_ascii_case(name)))
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    pub fn is_skinned(&self) -> bool {
        !self.joints.is_empty()
    }

    /// Children of bone `i`.
    pub fn children(&self, i: usize) -> Vec<usize> {
        (0..self.bones.len()).filter(|&k| self.bones[k].parent == Some(i)).collect()
    }

    /// Bones at rest, in the model.
    pub fn rest_world(&self) -> Vec<Xf> {
        self.bones.iter().map(Bone::rest_frame).collect()
    }

    /// Each bone's rest frame in its parent's.
    pub fn rest_local(&self) -> Vec<Xf> {
        let w = self.rest_world();
        self.bones.iter().enumerate().map(|(i, b)| b.parent.map_or(w[i], |p| w[p].inverse().then(&w[i]))).collect()
    }

    /// Puts bones in parent-first order (indices everywhere follow).
    pub fn sort(&mut self) {
        let n = self.bones.len();
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        while order.len() < n {
            let before = order.len();
            for i in 0..n {
                if !placed[i] && self.bones[i].parent.is_none_or(|p| p >= n || placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                // A loop of parents: break it.
                if let Some(i) = (0..n).find(|&i| !placed[i]) {
                    self.bones[i].parent = None;
                }
            }
        }
        let mut new_of = vec![0usize; n];
        for (k, &i) in order.iter().enumerate() {
            new_of[i] = k;
        }
        let old = std::mem::take(&mut self.bones);
        self.bones = order.iter().map(|&i| Bone { parent: old[i].parent.filter(|&p| p < n).map(|p| new_of[p]), ..old[i].clone() }).collect();
        for j in &mut self.joints {
            for k in j.iter_mut() {
                *k = new_of.get(*k as usize).copied().unwrap_or(0) as u16;
            }
        }
        let remap = |b: &mut usize| *b = new_of.get(*b).copied().unwrap_or(0);
        for c in &mut self.constraints {
            match c {
                Constraint::Ik { bone, target, .. } | Constraint::LookAt { bone, target, .. } => {
                    remap(bone);
                    if let Target::Bone(t) = target {
                        remap(t);
                    }
                }
                Constraint::Limit { bone, .. } => remap(bone),
                Constraint::CopyRotation { bone, from, .. } => {
                    remap(bone);
                    remap(from);
                }
            }
        }
        for a in &mut self.actions {
            for t in &mut a.tracks {
                match &mut t.channel {
                    Channel::Rotation(b) | Channel::Location(b) | Channel::Scale(b) => remap(b),
                    Channel::Shape(_) => {}
                }
            }
        }
    }

    /// Forward kinematics: posed frames from local poses.
    pub fn forward(&self, local: &[BonePose]) -> Vec<Xf> {
        let rl = self.rest_local();
        let mut world: Vec<Xf> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let here = rl[i].then(&local.get(i).copied().unwrap_or_default().xf());
            world.push(match b.parent {
                Some(p) => world[p].then(&here),
                None => here,
            });
        }
        world
    }

    /// The local rotations that give bones these frames in the model
    /// (rotations only; heads stay where the parents carry them, except
    /// roots, whose shift is kept).
    pub fn local_from_world(&self, world_rot: &[Quat], root_pos: Option<V3>) -> Vec<BonePose> {
        let rl = self.rest_local();
        let mut posed: Vec<Xf> = Vec::with_capacity(self.bones.len());
        let mut out = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let parent = b.parent.map_or(Xf::IDENTITY, |p| posed[p]);
            let frame = parent.then(&rl[i]);
            let rot = q_normalise(q_mul(q_conj(frame.rot), world_rot.get(i).copied().unwrap_or(frame.rot)));
            let mut pose = BonePose { rot, ..BonePose::default() };
            if b.parent.is_none()
                && let Some(p) = root_pos
            {
                // The shift in the bone's rest frame that puts its head at p.
                pose.loc = q_rotate(q_conj(frame.rot), sub(p, frame.pos));
            }
            let here = frame.then(&pose.xf());
            posed.push(here);
            out.push(pose);
        }
        out
    }

    /// Poses the rig: action(s), then the input's rotations on top, then
    /// constraints in order.
    pub fn evaluate(&self, input: &PoseInput) -> Posed {
        let n = self.bones.len();
        let mut local = vec![BonePose::default(); n];
        let mut shapes = vec![0.0f32; self.shapes.len()];
        if let Some((name, t, looped)) = &input.action
            && let Some(a) = self.action(name)
        {
            let (p, w) = a.sample(n, self.shapes.len(), *t, *looped);
            local = p;
            for (s, v) in shapes.iter_mut().zip(w) {
                if let Some(v) = v {
                    *s = v;
                }
            }
        }
        if let Some((name, t, looped, k)) = &input.blend
            && let Some(a) = self.action(name)
        {
            let k = k.clamp(0.0, 1.0);
            let (p, w) = a.sample(n, self.shapes.len(), *t, *looped);
            for (l, q) in local.iter_mut().zip(&p) {
                *l = l.mix(q, k);
            }
            for (s, v) in shapes.iter_mut().zip(w) {
                if let Some(v) = v {
                    *s += (v - *s) * k;
                }
            }
        }
        for (name, deg) in &input.rotations {
            if let Some(i) = self.bone(name) {
                local[i] = local[i].layered(&BonePose { rot: q_from_euler(*deg), ..BonePose::default() });
            }
        }
        for (name, d) in &input.offsets {
            if let Some(i) = self.bone(name) {
                local[i].loc = add(local[i].loc, *d);
            }
        }
        for (name, w) in &input.shapes {
            if let Some(i) = self.shape(name) {
                shapes[i] = *w;
            }
        }
        let mut world = self.forward(&local);
        // IK, then copies, then limits, then look-at, whatever order they
        // were added in: a limit listed before an IK would otherwise be
        // undone by it, and a look-at is the last word on where a bone
        // points. Within a stage the list order stands.
        let mut order: Vec<&Constraint> = self.constraints.iter().collect();
        order.sort_by_key(|c| c.stage());
        for c in order {
            let influence = input.influences.get(c.name()).copied().unwrap_or(match c {
                Constraint::Ik { influence, .. } | Constraint::LookAt { influence, .. } | Constraint::CopyRotation { influence, .. } => *influence,
                Constraint::Limit { .. } => 1.0,
            });
            if influence <= 0.0 {
                continue;
            }
            let target = |t: &Target| -> V3 {
                match input.targets.get(c.name()) {
                    Some(p) => *p,
                    None => match t {
                        Target::Point(p) => *p,
                        Target::Bone(b) => world.get(*b).map_or([0.0; 3], |x| x.pos),
                    },
                }
            };
            match c {
                Constraint::Ik { bone, chain, target: t, pole, .. } => {
                    let goal = target(t);
                    let before = local.clone();
                    self.solve_ik(&mut local, *bone, (*chain).max(1), goal, *pole);
                    if influence < 1.0 {
                        for (l, b) in local.iter_mut().zip(&before) {
                            *l = b.mix(l, influence);
                        }
                    }
                }
                Constraint::LookAt { bone, target: t, .. } => {
                    let goal = target(t);
                    if let Some(w) = world.get(*bone) {
                        let y = w.apply_dir([0.0, 1.0, 0.0]);
                        let turn = q_from_to(y, sub(goal, w.pos));
                        let new_world = q_mul(turn, w.rot);
                        let rl = self.rest_local();
                        let parent = self.bones[*bone].parent.map_or(Xf::IDENTITY, |p| world[p]);
                        let frame = parent.then(&rl[*bone]);
                        let rot = q_normalise(q_mul(q_conj(frame.rot), new_world));
                        local[*bone].rot = q_slerp(local[*bone].rot, rot, influence.min(1.0));
                    }
                }
                Constraint::Limit { bone, min, max, .. } => {
                    if let Some(l) = local.get_mut(*bone) {
                        let e = q_to_euler(l.rot);
                        let c: V3 = std::array::from_fn(|k| e[k].clamp(min[k].min(max[k]), max[k].max(min[k])));
                        if c != e {
                            l.rot = q_from_euler(c);
                        }
                    }
                }
                Constraint::CopyRotation { bone, from, .. } => {
                    if let (Some(src), true) = (local.get(*from).copied(), *bone < n) {
                        local[*bone].rot = q_slerp(local[*bone].rot, src.rot, influence.min(1.0));
                    }
                }
            }
            world = self.forward(&local);
        }
        let rest = self.rest_world();
        let skin = world.iter().zip(&rest).map(|(w, r)| w.then(&r.inverse())).collect();
        Posed { world, skin, local, shapes }
    }

    /// Solves an IK chain ending at bone `tip` (`chain` bones long) so the
    /// tip's tail reaches `goal`.
    fn solve_ik(&self, local: &mut [BonePose], tip: usize, chain: usize, goal: V3, pole: Option<V3>) {
        let mut ids = vec![tip];
        while ids.len() < chain {
            match self.bones[*ids.last().unwrap_or(&tip)].parent {
                Some(p) => ids.push(p),
                None => break,
            }
        }
        ids.reverse();
        let tip_len = self.bones[tip].length();
        let tail_of = |world: &[Xf]| world[tip].apply([0.0, tip_len, 0.0]);
        let set_world_rot = |local: &mut [BonePose], world: &[Xf], i: usize, rot: Quat| {
            let rl = self.rest_local();
            let parent = self.bones[i].parent.map_or(Xf::IDENTITY, |p| world[p]);
            let frame = parent.then(&rl[i]);
            local[i].rot = q_normalise(q_mul(q_conj(frame.rot), rot));
        };
        if ids.len() == 2 {
            // Two bones: the law of cosines, bent towards the pole.
            let (a, b) = (ids[0], ids[1]);
            let world = self.forward(local);
            let root = world[a].pos;
            let la = length(sub(world[b].pos, root));
            let lb = tip_len;
            let dir = normalise(sub(goal, root));
            if dir == [0.0; 3] {
                return;
            }
            // The bend plane: towards the pole, or the way the limb bends now.
            let mid_now = world[b].pos;
            let hint = match pole {
                Some(p) => sub(p, root),
                None => sub(mid_now, root),
            };
            let up = |v: V3| -> [f64; 3] { v.map(f64::from) };
            let down = |v: [f64; 3]| -> V3 { v.map(|x| x as f32) };
            let solved = studio_rig::ik::two_bone(up(root), f64::from(la), f64::from(lb), up(goal), up(hint), studio_rig::ik::Reach::default());
            let (mid, end) = (down(solved.mid), down(solved.end));
            // Upper bone: its Y axis to the middle joint, keeping its twist
            // as close as it can.
            let ya = world[a].apply_dir([0.0, 1.0, 0.0]);
            let rot_a = q_mul(q_from_to(ya, sub(mid, root)), world[a].rot);
            set_world_rot(local, &world, a, rot_a);
            let world = self.forward(local);
            let yb = world[b].apply_dir([0.0, 1.0, 0.0]);
            let rot_b = q_mul(q_from_to(yb, sub(end, world[b].pos)), world[b].rot);
            set_world_rot(local, &world, b, rot_b);
            return;
        }
        // Longer chains: FABRIK on the joints (`studio_rig::ik::fabrik`),
        // the middle joints leaning toward the pole when there is one; then
        // each bone, root first, turned so its next joint lands where the
        // solve put it.
        let world = self.forward(local);
        let mut joints: Vec<[f64; 3]> = ids.iter().map(|&i| world[i].pos.map(f64::from)).collect();
        joints.push(tail_of(&world).map(f64::from));
        // A chain lying straight has no bend for FABRIK to grow from: it
        // folds into a zig-zag. Start it on an arc bowed toward the pole.
        if let Some(p) = pole {
            arc_start(&mut joints, goal.map(f64::from), p.map(f64::from));
        }
        studio_rig::ik::fabrik(&mut joints, goal.map(f64::from), pole.map(|p| p.map(f64::from)), studio_rig::ik::Reach::default(), 0.01, 64);
        let target: Vec<V3> = joints.iter().map(|j| j.map(|x| x as f32)).collect();
        for k in 0..ids.len() {
            let world = self.forward(local);
            let i = ids[k];
            let at = world[i].pos;
            let next_now = if k + 1 < ids.len() { world[ids[k + 1]].pos } else { tail_of(&world) };
            let turn = q_from_to(sub(next_now, at), sub(target[k + 1], at));
            set_world_rot(local, &world, i, q_mul(turn, world[i].rot));
        }
    }

    /// Where the vertices of `mesh` go in a pose, and their normals.
    pub fn deform(&self, mesh: &Mesh, posed: &Posed) -> (Vec<V3>, Vec<V3>) {
        let mut pos = mesh.positions.clone();
        let mut shaped = false;
        for (k, w) in self.shapes.iter().zip(&posed.shapes) {
            if w.abs() < 1e-5 {
                continue;
            }
            shaped = true;
            for (i, d) in k.indices.iter().zip(&k.deltas) {
                if let Some(p) = pos.get_mut(*i as usize) {
                    *p = add(*p, scale(*d, *w));
                }
            }
        }
        let normals = if shaped {
            let mut m = Mesh { positions: pos.clone(), indices: mesh.indices.clone(), normals: vec![[0.0; 3]; pos.len()], ..Default::default() };
            crate::ops::smooth_normals_in_place(&mut m);
            m.normals
        } else {
            mesh.normals.clone()
        };
        if !self.is_skinned() || posed.skin.is_empty() {
            return (pos, normals);
        }
        if self.dual_quat {
            return self.deform_dq(&pos, &normals, posed);
        }
        let mats: Vec<[[f32; 4]; 3]> = posed.skin.iter().map(Xf::matrix).collect();
        let mut out_p = Vec::with_capacity(pos.len());
        let mut out_n = Vec::with_capacity(pos.len());
        for (i, p) in pos.iter().enumerate() {
            let (j, w) = (self.joints.get(i).copied().unwrap_or([0; 4]), self.weights.get(i).copied().unwrap_or([0.0; 4]));
            let mut m = [[0.0f32; 4]; 3];
            let mut total = 0.0;
            for k in 0..4 {
                if w[k] <= 0.0 {
                    continue;
                }
                let Some(b) = mats.get(j[k] as usize) else { continue };
                total += w[k];
                for r in 0..3 {
                    for c in 0..4 {
                        m[r][c] += b[r][c] * w[k];
                    }
                }
            }
            if total <= 1e-6 {
                out_p.push(*p);
                out_n.push(normals.get(i).copied().unwrap_or([0.0; 3]));
                continue;
            }
            let ap = |v: V3, w4: f32| -> V3 { std::array::from_fn(|r| (m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3] * w4) / total) };
            out_p.push(ap(*p, 1.0));
            out_n.push(normalise(ap(normals.get(i).copied().unwrap_or([0.0; 3]), 0.0)));
        }
        (out_p, out_n)
    }

    /// Skinning with dual quaternions: each bone's motion blended as a
    /// dual quaternion (`studio_rig::dq`), normals carried by the same
    /// blend.
    fn deform_dq(&self, pos: &[V3], normals: &[V3], posed: &Posed) -> (Vec<V3>, Vec<V3>) {
        let bones: Vec<studio_rig::dq::Bone> = posed
            .skin
            .iter()
            .map(|x| {
                let m = x.matrix();
                studio_rig::dq::Bone::from_affine(std::array::from_fn(|r| std::array::from_fn(|c| f64::from(m[r][c]))), std::array::from_fn(|r| f64::from(m[r][3])))
            })
            .collect();
        let mut out_p = Vec::with_capacity(pos.len());
        let mut out_n = Vec::with_capacity(pos.len());
        for (i, p) in pos.iter().enumerate() {
            let (j, w) = (self.joints.get(i).copied().unwrap_or([0; 4]), self.weights.get(i).copied().unwrap_or([0.0; 4]));
            let parts: Vec<(usize, f64)> = (0..4).filter(|&k| w[k] > 0.0).map(|k| (j[k] as usize, f64::from(w[k]))).collect();
            let at = |v: V3| -> V3 { studio_rig::dq::skin(v.map(f64::from), &bones, &parts).map(|x| x as f32) };
            let moved = at(*p);
            out_p.push(moved);
            // A small step along the normal, skinned the same way.
            let n = normals.get(i).copied().unwrap_or([0.0; 3]);
            let eps = 1e-2;
            out_n.push(if n == [0.0; 3] { n } else { normalise(sub(at(add(*p, scale(n, eps))), moved)) });
        }
        (out_p, out_n)
    }

    /// The mesh in a pose (a copy; the rig stays with it).
    pub fn posed_mesh(&self, mesh: &Mesh, input: &PoseInput) -> Mesh {
        let posed = self.evaluate(input);
        let (p, n) = self.deform(mesh, &posed);
        let mut out = mesh.clone();
        out.positions = p;
        out.normals = n;
        out
    }

    /// Bone tips in a pose, for drawing the armature: (head, tail) per bone.
    pub fn bone_lines(&self, posed: &Posed) -> Vec<(V3, V3)> {
        self.bones.iter().enumerate().map(|(i, b)| (posed.world[i].pos, posed.world[i].apply([0.0, b.length(), 0.0]))).collect()
    }

    /// The rig moved with its mesh: `point` maps positions (bone ends,
    /// targets), `vector` maps offsets (shape keys), `k` is the overall
    /// scale (bone shifts in actions).
    pub fn map_points(&mut self, point: impl Fn(V3) -> V3, vector: impl Fn(V3) -> V3, k: f32) {
        for b in &mut self.bones {
            b.head = point(b.head);
            b.tail = point(b.tail);
        }
        for s in &mut self.shapes {
            for d in &mut s.deltas {
                *d = vector(*d);
            }
        }
        for c in &mut self.constraints {
            match c {
                Constraint::Ik { target, pole, .. } => {
                    if let Target::Point(p) = target {
                        *p = point(*p);
                    }
                    if let Some(p) = pole {
                        *p = point(*p);
                    }
                }
                Constraint::LookAt { target: Target::Point(p), .. } => *p = point(*p),
                _ => {}
            }
        }
        for a in &mut self.actions {
            for t in &mut a.tracks {
                if let Channel::Location(_) = t.channel {
                    for v in &mut t.values {
                        for x in v.iter_mut().take(3) {
                            *x *= k;
                        }
                    }
                }
            }
        }
    }

    /// The rig carried onto a changed mesh (new vertices after a
    /// subdivision, a boolean, a decimation): bones, constraints and
    /// actions kept; skin weights and shape keys moved to the new vertices
    /// from the nearest old ones.
    pub fn transferred(&self, old: &Mesh, new: &Mesh) -> Rig {
        let mut r = Rig { bones: self.bones.clone(), constraints: self.constraints.clone(), actions: self.actions.clone(), dual_quat: self.dual_quat, ..Default::default() };
        if old.positions.is_empty() {
            return r;
        }
        let grid = PointGrid::new(&old.positions);
        let nearest: Vec<usize> = new.positions.iter().map(|p| grid.nearest(&old.positions, *p)).collect();
        if self.is_skinned() {
            r.joints = nearest.iter().map(|&i| self.joints.get(i).copied().unwrap_or([0; 4])).collect();
            r.weights = nearest.iter().map(|&i| self.weights.get(i).copied().unwrap_or([0.0; 4])).collect();
        }
        for k in &self.shapes {
            let mut dense = vec![[0.0f32; 3]; old.positions.len()];
            for (i, d) in k.indices.iter().zip(&k.deltas) {
                if let Some(v) = dense.get_mut(*i as usize) {
                    *v = *d;
                }
            }
            let moved: Vec<V3> = nearest.iter().map(|&i| dense[i]).collect();
            r.shapes.push(ShapeKey::from_dense(k.name.clone(), &moved));
        }
        r
    }
}

/// A uniform grid of points for nearest-point queries.
pub struct PointGrid {
    cell: f32,
    lo: V3,
    map: HashMap<[i32; 3], Vec<u32>>,
}

impl PointGrid {
    pub fn new(points: &[V3]) -> PointGrid {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in points {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if points.is_empty() {
            lo = [0.0; 3];
            hi = [1.0; 3];
        }
        let vol = (0..3).map(|i| (hi[i] - lo[i]).max(1e-3)).product::<f32>();
        let cell = (vol / points.len().max(1) as f32).cbrt().max(1e-3) * 2.0;
        let mut map: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        let g = PointGrid { cell, lo, map: HashMap::new() };
        for (i, p) in points.iter().enumerate() {
            map.entry(g.key(*p)).or_default().push(i as u32);
        }
        PointGrid { map, ..g }
    }

    fn key(&self, p: V3) -> [i32; 3] {
        std::array::from_fn(|i| ((p[i] - self.lo[i]) / self.cell).floor() as i32)
    }

    /// Index of the nearest point (0 when there are none).
    pub fn nearest(&self, points: &[V3], p: V3) -> usize {
        let k = self.key(p);
        let mut best = (f32::INFINITY, 0usize);
        for r in 0..64i32 {
            for dz in -r..=r {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        if let Some(list) = self.map.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                            for &i in list {
                                let d = length(sub(points[i as usize], p));
                                if d < best.0 {
                                    best = (d, i as usize);
                                }
                            }
                        }
                    }
                }
            }
            // Anything in a further ring is at least r cells away.
            if best.0.is_finite() && best.0 <= r as f32 * self.cell {
                break;
            }
        }
        best.1
    }

    /// Indices of points within `radius` of `p`.
    pub fn within(&self, points: &[V3], p: V3, radius: f32) -> Vec<usize> {
        let r = (radius / self.cell).ceil() as i32;
        let k = self.key(p);
        let mut out = Vec::new();
        for dz in -r..=r {
            for dy in -r..=r {
                for dx in -r..=r {
                    if let Some(list) = self.map.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        for &i in list {
                            if length(sub(points[i as usize], p)) <= radius {
                                out.push(i as usize);
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// Lays a nearly straight chain of `joints` (root first) on a circle from
/// its root toward `goal`, bowed toward `pole`, each link a chord of its
/// own length; a chain already bent (more than 2 % of its length off the
/// line from root to tip) is left as it is.
fn arc_start(joints: &mut [[f64; 3]], goal: [f64; 3], pole: [f64; 3]) {
    use studio_rig::v;
    let n = joints.len();
    if n < 3 {
        return;
    }
    let lens: Vec<f64> = joints.windows(2).map(|w| v::dist(w[0], w[1])).collect();
    let total: f64 = lens.iter().sum();
    let root = joints[0];
    let line = v::norm(v::sub(joints[n - 1], root));
    if v::len(line) < 0.5 || joints.iter().any(|j| v::len(v::across(v::sub(*j, root), line)) > 0.02 * total) {
        return;
    }
    let c = v::dist(root, goal).min(total * 0.999);
    let u = v::norm(v::sub(goal, root));
    let w = v::norm(v::across(v::sub(pole, root), u));
    if c < 1e-9 || v::len(u) < 0.5 || v::len(w) < 0.5 {
        return;
    }
    // A circle through the joints, each link a chord of its own length:
    // the radius whose chords add up to an arc from the root to the goal.
    let turn = |r: f64| lens.iter().map(|l| 2.0 * (l / (2.0 * r)).min(1.0).asin()).sum::<f64>();
    let chord = |r: f64| {
        let t = turn(r);
        if t >= std::f64::consts::TAU { 0.0 } else { 2.0 * r * (t / 2.0).sin() }
    };
    let longest = lens.iter().copied().fold(0.0, f64::max);
    let (mut lo, mut hi) = (longest / 2.0 * 1.0001, total * 1e3);
    // Folded as far as the links allow, the chord may still be too long:
    // then that tightest circle.
    let r = if chord(lo) >= c {
        lo
    } else {
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if chord(mid) < c { lo = mid } else { hi = mid }
        }
        0.5 * (lo + hi)
    };
    let th = turn(r);
    let centre = v::sub(v::add(root, v::scale(u, r * (th / 2.0).sin())), v::scale(w, r * (th / 2.0).cos()));
    let mut phi = 0.0;
    for i in 1..n {
        phi += 2.0 * (lens[i - 1] / (2.0 * r)).min(1.0).asin();
        let a = th / 2.0 - phi;
        joints[i] = v::add(centre, v::add(v::scale(u, -r * a.sin()), v::scale(w, r * a.cos())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3, tol: f32) -> bool {
        length(sub(a, b)) <= tol
    }

    /// A leg: thigh then shin, pointing down.
    fn leg() -> Rig {
        let mut r = Rig::default();
        r.bones.push(Bone::new("thigh", None, [0.0, 0.0, 0.0], [0.0, 100.0, 0.0]));
        r.bones.push(Bone::new("shin", Some(0), [0.0, 100.0, 0.0], [0.0, 200.0, 0.0]));
        r
    }

    #[test]
    fn dual_quaternion_skinning_keeps_a_twisted_joint_round() {
        let mut r = leg();
        let mesh = Mesh { positions: vec![[10.0, 100.0, 0.0]], normals: vec![[1.0, 0.0, 0.0]], ..Default::default() };
        r.joints = vec![[0, 1, 0, 0]];
        r.weights = vec![[0.5, 0.5, 0.0, 0.0]];
        let input = PoseInput { rotations: HashMap::from([("shin".to_string(), [0.0, 180.0, 0.0])]), ..Default::default() };
        let radius = |m: &Mesh| (m.positions[0][0].powi(2) + m.positions[0][2].powi(2)).sqrt();
        // Blended matrices collapse the vertex onto the axis; dual quaternions keep it round.
        assert!(radius(&r.posed_mesh(&mesh, &input)) < 1.0);
        r.dual_quat = true;
        let dq = r.posed_mesh(&mesh, &input);
        assert!((radius(&dq) - 10.0).abs() < 0.1, "{:?}", dq.positions);
        assert!((length(dq.normals[0]) - 1.0).abs() < 1e-3);
        // Rest stays at rest.
        let rest = r.posed_mesh(&mesh, &PoseInput::default());
        assert!(close(rest.positions[0], mesh.positions[0], 1e-3));
    }

    #[test]
    fn quaternions_and_euler_round_trip() {
        for e in [[10.0, 20.0, 30.0], [-45.0, 5.0, 80.0], [0.0, 0.0, -170.0]] {
            let back = q_to_euler(q_from_euler(e));
            for k in 0..3 {
                assert!((back[k] - e[k]).abs() < 0.05, "{e:?} -> {back:?}");
            }
        }
        let q = q_from_to([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(close(q_rotate(q, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0], 1e-5));
        let x = Xf { rot: q_from_euler([0.0, 0.0, 90.0]), pos: [5.0, 0.0, 0.0], scale: 2.0 };
        let p = [1.0, 2.0, 3.0];
        assert!(close(x.inverse().apply(x.apply(p)), p, 1e-4));
    }

    #[test]
    fn bone_frames_point_along_the_bone_and_face_forward() {
        let b = Bone::new("shin", None, [0.0, 0.0, 0.0], [0.0, 100.0, 0.0]);
        let f = b.rest_frame();
        assert!(close(f.apply_dir([0.0, 1.0, 0.0]), [0.0, 1.0, 0.0], 1e-5));
        assert!(close(f.apply_dir([0.0, 0.0, 1.0]), [0.0, 0.0, -1.0], 1e-5));
        // A forward swing (+x) moves the tail towards -z (towards the camera).
        let mut r = leg();
        let posed = r.evaluate(&PoseInput { rotations: HashMap::from([("thigh".to_string(), [30.0, 0.0, 0.0])]), ..Default::default() });
        let tail = r.bone_lines(&posed)[0].1;
        assert!(tail[2] < -40.0, "{tail:?}");
        r.sort();
        assert_eq!(r.bones[1].parent, Some(0));
    }

    #[test]
    fn skin_follows_the_bones() {
        let mut r = leg();
        // Four vertices: two on the thigh, two on the shin.
        let mut m = Mesh { positions: vec![[5.0, 20.0, 0.0], [5.0, 80.0, 0.0], [5.0, 120.0, 0.0], [5.0, 190.0, 0.0]], ..Default::default() };
        m.normals = vec![[1.0, 0.0, 0.0]; 4];
        r.joints = vec![[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]];
        r.weights = vec![[1.0, 0.0, 0.0, 0.0]; 4];
        // Bend the knee back 90°: the shin's vertices swing behind (+z).
        let posed = r.evaluate(&PoseInput { rotations: HashMap::from([("shin".to_string(), [-90.0, 0.0, 0.0])]), ..Default::default() });
        let (p, _) = r.deform(&m, &posed);
        assert!(close(p[0], m.positions[0], 1e-3));
        assert!(close(p[3], [5.0, 100.0, 90.0], 0.5), "{:?}", p[3]);
    }

    #[test]
    fn two_bone_ik_reaches_and_bends_towards_the_pole() {
        let mut r = leg();
        r.constraints.push(Constraint::Ik { name: "foot".into(), bone: 1, chain: 2, target: Target::Point([0.0, 150.0, -30.0]), pole: Some([0.0, 100.0, -200.0]), influence: 1.0 });
        let posed = r.evaluate(&PoseInput::default());
        let lines = r.bone_lines(&posed);
        assert!(close(lines[1].1, [0.0, 150.0, -30.0], 0.5), "{:?}", lines[1].1);
        // The knee goes forward (towards the pole at -z).
        assert!(lines[1].0[2] < -20.0, "{:?}", lines[1].0);
        // Out of reach: the leg straightens towards it.
        let posed = r.evaluate(&PoseInput { targets: HashMap::from([("foot".to_string(), [0.0, 500.0, 0.0])]), ..Default::default() });
        let tail = r.bone_lines(&posed)[1].1;
        assert!(close(tail, [0.0, 200.0, 0.0], 1.0), "{tail:?}");
    }

    #[test]
    fn constraints_solve_ik_then_limits_then_look_at_whatever_the_list_order() {
        let limit = Constraint::Limit { name: "stiff".into(), bone: 0, min: [-5.0, -5.0, -5.0], max: [5.0, 5.0, 5.0] };
        let ik = Constraint::Ik { name: "foot".into(), bone: 1, chain: 2, target: Target::Point([0.0, 150.0, -30.0]), pole: Some([0.0, 100.0, -200.0]), influence: 1.0 };
        let thigh_x = |r: &Rig| q_to_euler(r.evaluate(&PoseInput::default()).local[0].rot)[0];
        // The limit listed first, then the IK: the limit still holds.
        let mut early = leg();
        early.constraints = vec![limit.clone(), ik.clone()];
        let mut late = leg();
        late.constraints = vec![ik.clone(), limit.clone()];
        let free = {
            let mut r = leg();
            r.constraints = vec![ik];
            thigh_x(&r)
        };
        assert!(free.abs() > 10.0, "the IK alone bends the thigh: {free}");
        assert!(thigh_x(&early).abs() <= 5.01, "limit listed before the IK: {}", thigh_x(&early));
        assert!((thigh_x(&early) - thigh_x(&late)).abs() < 1e-3, "the same either way");
        // A look-at listed before a limit on the same bone has the last word.
        let mut h = Rig::default();
        h.bones.push(Bone::new("head", None, [0.0; 3], [0.0, -50.0, 0.0]));
        h.constraints.push(Constraint::LookAt { name: "look".into(), bone: 0, target: Target::Point([300.0, -300.0, 0.0]), influence: 1.0 });
        h.constraints.push(Constraint::Limit { name: "stiff".into(), bone: 0, min: [0.0; 3], max: [0.0; 3] });
        let posed = h.evaluate(&PoseInput::default());
        let d = normalise(sub(h.bone_lines(&posed)[0].1, [0.0; 3]));
        assert!(close(d, normalise([1.0, -1.0, 0.0]), 1e-3), "{d:?}");
    }

    #[test]
    fn long_chains_and_look_at() {
        let mut r = Rig::default();
        for i in 0..5 {
            r.bones.push(Bone::new(format!("tail.{i}"), if i == 0 { None } else { Some(i - 1) }, [i as f32 * 40.0, 0.0, 0.0], [(i + 1) as f32 * 40.0, 0.0, 0.0]));
        }
        r.constraints.push(Constraint::Ik { name: "reach".into(), bone: 4, chain: 5, target: Target::Point([100.0, 100.0, 0.0]), pole: None, influence: 1.0 });
        let posed = r.evaluate(&PoseInput::default());
        assert!(close(r.bone_lines(&posed)[4].1, [100.0, 100.0, 0.0], 1.0));
        let mut h = Rig::default();
        h.bones.push(Bone::new("head", None, [0.0, 0.0, 0.0], [0.0, -50.0, 0.0]));
        h.constraints.push(Constraint::LookAt { name: "look".into(), bone: 0, target: Target::Point([300.0, -300.0, 0.0]), influence: 1.0 });
        let posed = h.evaluate(&PoseInput::default());
        let d = normalise(sub(h.bone_lines(&posed)[0].1, [0.0; 3]));
        assert!(close(d, normalise([1.0, -1.0, 0.0]), 1e-3), "{d:?}");
    }

    #[test]
    fn a_long_chain_bends_toward_its_pole() {
        // Four bones straight down (y down), the goal pulled up close to
        // the root so the chain must fold; the pole says which way.
        for pole_z in [200.0f32, -200.0] {
            let mut r = Rig::default();
            for i in 0..4 {
                r.bones.push(Bone::new(format!("tail.{i}"), if i == 0 { None } else { Some(i - 1) }, [0.0, i as f32 * 40.0, 0.0], [0.0, (i + 1) as f32 * 40.0, 0.0]));
            }
            r.constraints.push(Constraint::Ik { name: "reach".into(), bone: 3, chain: 4, target: Target::Point([0.0, 90.0, 0.0]), pole: Some([0.0, 80.0, pole_z]), influence: 1.0 });
            let posed = r.evaluate(&PoseInput::default());
            let lines = r.bone_lines(&posed);
            assert!(close(lines[3].1, [0.0, 90.0, 0.0], 1.0), "tip reaches: {:?}", lines[3].1);
            // The middle joints lean the pole's way, and the bones keep their length.
            for k in 1..4 {
                assert!(lines[k].0[2] * pole_z.signum() > 5.0, "joint {k} toward the pole at z {pole_z}: {:?}", lines[k].0);
                assert!((length(sub(lines[k].1, lines[k].0)) - 40.0).abs() < 0.5);
            }
        }
    }

    #[test]
    fn actions_loop_and_shape_keys_mix() {
        let mut r = leg();
        r.actions.push(Action {
            name: "swing".into(),
            duration: 1.0,
            tracks: vec![Track { channel: Channel::Rotation(0), times: vec![0.0, 0.5, 1.0], values: vec![Q_ID, q_from_euler([40.0, 0.0, 0.0]), Q_ID], step: false }],
        });
        r.shapes.push(ShapeKey::from_dense("bulge", &[[0.0, 0.0, -10.0]]));
        let m = Mesh { positions: vec![[0.0, 50.0, 0.0]], normals: vec![[0.0, 0.0, -1.0]], ..Default::default() };
        let input = PoseInput { action: Some(("swing".into(), 1.5, true)), shapes: HashMap::from([("bulge".to_string(), 0.5)]), ..Default::default() };
        let posed = r.evaluate(&input);
        assert!((q_to_euler(posed.local[0].rot)[0] - 40.0).abs() < 0.1);
        let (p, _) = r.deform(&m, &posed);
        assert!(close(p[0], [0.0, 50.0, -5.0], 1e-4));
    }

    #[test]
    fn world_rotations_come_back_as_local_poses() {
        let r = leg();
        let input = PoseInput { rotations: HashMap::from([("thigh".to_string(), [20.0, 0.0, 10.0]), ("shin".to_string(), [-35.0, 0.0, 0.0])]), ..Default::default() };
        let posed = r.evaluate(&input);
        let rots: Vec<Quat> = posed.world.iter().map(|w| w.rot).collect();
        let back = r.local_from_world(&rots, None);
        let again = r.forward(&back);
        for (a, b) in again.iter().zip(&posed.world) {
            assert!(close(a.apply([0.0, 100.0, 0.0]), b.apply([0.0, 100.0, 0.0]), 1e-2));
        }
    }
}
