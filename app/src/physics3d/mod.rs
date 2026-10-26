// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Physics in Studio's 3D scenes: what each model clip is physically
//! (its body type, material, mass, collider and joints, kept in its
//! `physics.*` settings), the scene's scale (pixels to the metre) and
//! gravity, and the bridge to the engine (`studio_sim::dynamics`): a
//! moment of the timeline becomes a world of bodies, a run of it becomes
//! keys on the clips, so the result plays, scrubs and edits like any
//! other animation, and the same scene always gives the same result.
//!
//! - [`scene`]: builds the world from the clips at a moment (scenery as
//!   exact triangles, props as fitted convex shapes, animated things and
//!   characters as kinematic bodies that follow their animation, worlds'
//!   terrain, a floor);
//! - [`run`]: simulates a stretch of time and bakes it (transform keys for
//!   bodies, bone keys for characters that fall or are knocked about,
//!   pieces for things that break);
//! - [`rest`]: placing things physically (resting on what is under them,
//!   out of what they overlap) and checking a scene for things floating,
//!   sunk into each other or balanced where they would fall.

pub mod rest;
pub mod run;
pub mod scene;

use studio_model as m;
use studio_sim::dynamics::decompose::Fit;
use studio_sim::dynamics::material::{self, Stuff};
use studio_sim::dynamics::math::{Pose, Quat, V3};

/// How a clip takes part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyType {
    /// Decided by the scene: moving props when simulated, scenery when
    /// still, animated things and characters following their animation.
    Auto,
    Dynamic,
    Static,
    Kinematic,
    /// Not in the physics at all (a cloud, a light shaft, a hologram).
    None,
}

impl BodyType {
    pub fn parse(s: &str) -> Option<BodyType> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" | "" => BodyType::Auto,
            "dynamic" | "rigid" | "active" | "moving" | "falls" => BodyType::Dynamic,
            "static" | "fixed" | "scenery" | "passive" => BodyType::Static,
            "kinematic" | "animated" | "keyed" => BodyType::Kinematic,
            "none" | "off" | "ghost" => BodyType::None,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BodyType::Auto => "auto",
            BodyType::Dynamic => "dynamic",
            BodyType::Static => "static",
            BodyType::Kinematic => "kinematic",
            BodyType::None => "none",
        }
    }

    pub const ALL: [BodyType; 5] = [BodyType::Auto, BodyType::Dynamic, BodyType::Static, BodyType::Kinematic, BodyType::None];
}

/// A joint kept on a clip (to another clip, or to the world).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JointSpec {
    /// hinge, ball, slider, fixed or spring.
    pub kind: String,
    /// The other clip; none for a point in the world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
    /// Where it is (scene pixels); the middle between the two by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f32; 3]>,
    /// The hinge axis, slide direction or twist axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<[f32; 3]>,
    /// Degrees for a hinge or ball cone, pixels for a slider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<[f32; 2]>,
    /// A motor: degrees (or pixels) a second and the most torque (N·m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motor: Option<[f32; 2]>,
    /// Spring stiffness (Hz) and damping (0..1), for spring joints and
    /// springy hinges and sliders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spring: Option<[f32; 2]>,
    /// Breaks above this force (N).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breaks: Option<f32>,
}

/// A clip's physical properties (its `physics.*` settings).
#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub body: BodyType,
    /// A material name (wood, glass, steel, rubber, flesh ...); else guessed
    /// from the model's look.
    pub material: Option<String>,
    /// kg, overriding volume × density.
    pub mass: Option<f32>,
    pub fit: Fit,
    pub friction: Option<f32>,
    pub bounce: Option<f32>,
    /// Breaks into pieces when hit hard enough (glass, china, plaster:
    /// on by material unless set).
    pub breakable: Option<bool>,
    pub joints: Vec<JointSpec>,
}

impl Default for Props {
    fn default() -> Props {
        Props { body: BodyType::Auto, material: None, mass: None, fit: Fit::Auto, friction: None, bounce: None, breakable: None, joints: Vec::new() }
    }
}

fn param<'a>(c: &'a m::Clip, k: &str) -> Option<&'a m::Value> {
    match &c.source {
        m::ClipSource::Generator(g) => g.params.get(k),
        _ => None,
    }
}

fn text(c: &m::Clip, k: &str) -> Option<String> {
    match param(c, k)? {
        m::Value::Text(s) | m::Value::Choice(s) => Some(s.clone()),
        _ => None,
    }
}

fn number(c: &m::Clip, k: &str) -> Option<f32> {
    match param(c, k)? {
        m::Value::Scalar(v) | m::Value::Angle(v) => Some(*v as f32),
        _ => None,
    }
}

impl Props {
    pub fn of(c: &m::Clip) -> Props {
        Props {
            body: text(c, "physics.body").and_then(|s| BodyType::parse(&s)).unwrap_or(BodyType::Auto),
            material: text(c, "physics.material").filter(|s| !s.is_empty()),
            mass: number(c, "physics.mass").filter(|m| *m > 0.0),
            fit: text(c, "physics.collider").and_then(|s| Fit::parse(&s)).unwrap_or(Fit::Auto),
            friction: number(c, "physics.friction"),
            bounce: number(c, "physics.bounce"),
            breakable: match param(c, "physics.breakable") {
                Some(m::Value::Bool(b)) => Some(*b),
                _ => None,
            },
            joints: text(c, "physics.joints").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        }
    }

    /// Writes these into the clip's settings (only what differs from the
    /// defaults is kept).
    pub fn write(&self, c: &mut m::Clip) {
        let m::ClipSource::Generator(g) = &mut c.source else { return };
        let mut set = |k: &str, v: Option<m::Value>| {
            match v {
                Some(v) => g.params.insert(k.to_string(), v),
                None => g.params.remove(k),
            };
        };
        set("physics.body", (self.body != BodyType::Auto).then(|| m::Value::Text(self.body.name().into())));
        set("physics.material", self.material.clone().map(m::Value::Text));
        set("physics.mass", self.mass.map(|v| m::Value::Scalar(v as f64)));
        set("physics.collider", (self.fit != Fit::Auto).then(|| m::Value::Text(self.fit.name().into())));
        set("physics.friction", self.friction.map(|v| m::Value::Scalar(v as f64)));
        set("physics.bounce", self.bounce.map(|v| m::Value::Scalar(v as f64)));
        set("physics.breakable", self.breakable.map(m::Value::Bool));
        set("physics.joints", (!self.joints.is_empty()).then(|| m::Value::Text(serde_json::to_string(&self.joints).unwrap_or_default())));
    }

    /// The material: as set, else from the model's look.
    pub fn stuff(&self, mesh: Option<&studio_mesh::Mesh>, name: &str) -> Stuff {
        let mut s = self
            .material
            .as_deref()
            .and_then(material::named)
            .or_else(|| material::named(name))
            .unwrap_or_else(|| match mesh.and_then(|m| m.materials.first()) {
                Some(l) => material::guess(&l.name, l.metallic, l.roughness, l.transmission, l.subsurface),
                None => material::default_stuff(),
            });
        if let Some(f) = self.friction {
            s.friction = f.clamp(0.0, 2.0);
        }
        if let Some(b) = self.bounce {
            s.bounce = b.clamp(0.0, 1.0);
        }
        s
    }

    pub fn is_breakable(&self, stuff: &Stuff) -> bool {
        self.breakable.unwrap_or(stuff.breaks_at.is_finite() && stuff.breaks_at < 5.0)
    }
}

/// The scene's physical settings (kept with the project).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct World {
    /// Scene pixels to the metre; none = worked out from what is in it.
    pub px_per_metre: Option<f32>,
    /// m/s² (9.81 on Earth).
    pub gravity: f32,
    /// Things put in the scene rest on what is under them.
    pub rest_on_place: bool,
}

impl Default for World {
    fn default() -> World {
        World { px_per_metre: None, gravity: 9.81, rest_on_place: true }
    }
}

pub const SETTINGS_KEY: &str = "physics";

impl World {
    pub fn of(p: &m::Project) -> World {
        let mut w = World::default();
        if let Some(v) = p.settings.extra.get(SETTINGS_KEY) {
            w.px_per_metre = v.get("px_per_metre").and_then(|x| x.as_f64()).map(|x| x as f32).filter(|x| *x > 0.0);
            w.gravity = v.get("gravity").and_then(|x| x.as_f64()).map_or(9.81, |x| x as f32);
            w.rest_on_place = v.get("rest_on_place").and_then(|x| x.as_bool()).unwrap_or(true);
        }
        w
    }

    pub fn write(&self, s: &mut m::ProjectSettings) {
        let mut o = serde_json::Map::new();
        if let Some(p) = self.px_per_metre {
            o.insert("px_per_metre".into(), serde_json::json!(p));
        }
        if (self.gravity - 9.81).abs() > 1e-4 {
            o.insert("gravity".into(), serde_json::json!(self.gravity));
        }
        if !self.rest_on_place {
            o.insert("rest_on_place".into(), serde_json::json!(false));
        }
        if o.is_empty() {
            s.extra.remove(SETTINGS_KEY);
        } else {
            s.extra.insert(SETTINGS_KEY.into(), serde_json::Value::Object(o));
        }
    }
}

/// Where Studio sizes things when nothing in the scene says: a figure the
/// `model` op makes is about 475 px to the metre.
pub const DEFAULT_PX_PER_METRE: f32 = 475.0;

/// Scene pixels to the metre: as set; else from the people in it (a
/// standing figure is about 1.7 m), a world (its own scale) or Studio's
/// usual size.
pub fn px_per_metre(p: &m::Project) -> f32 {
    if let Some(v) = World::of(p).px_per_metre {
        return v;
    }
    let mut votes: Vec<f32> = Vec::new();
    for seq in &p.sequences {
        for c in &seq.clips {
            if let m::ClipSource::Generator(g) = &c.source
                && let Some(m::Value::Scalar(mpp)) = g.params.get("metres_per_px")
                && *mpp > 0.0
            {
                votes.push(1.0 / *mpp as f32);
            }
        }
    }
    if votes.is_empty() {
        for seq in &p.sequences {
            for c in &seq.clips {
                if let Some(h) = person_height_px(c) {
                    votes.push(h / 1.7);
                }
            }
        }
    }
    if votes.is_empty() {
        return DEFAULT_PX_PER_METRE;
    }
    votes.sort_by(f32::total_cmp);
    votes[votes.len() / 2].clamp(5.0, 20_000.0)
}

/// A rigged humanoid's standing height in scene pixels.
fn person_height_px(c: &m::Clip) -> Option<f32> {
    crate::mesh_store::file_of(c)?;
    let mesh = scene::load_mesh(c)?;
    let rig = mesh.rig.as_ref()?;
    if rig.bone("head").is_none() || rig.bone("foot.L").is_none() {
        return None;
    }
    let (lo, hi) = mesh.bounds();
    let k = (c.transform.scale_y.abs() as f32).max(1e-6);
    Some((hi[1] - lo[1]) * k)
}

/// Words for things that are meant to be off the ground.
pub const AIRBORNE: &[&str] = &["bird", "plane", "aircraft", "jet", "drone", "cloud", "balloon", "kite", "sun", "moon", "star", "planet", "ufo", "spaceship", "satellite", "helicopter", "butterfly", "bee", "fly", "bat", "ghost", "spirit", "lamp", "chandelier", "light", "sign", "banner", "flag", "particle", "spark", "ember", "smoke", "fog", "mist", "logo", "title", "text", "hologram", "orb", "float", "hover", "flying", "hanging", "pendant", "ceiling", "wall"];

/// Words for the things everything else stands on.
pub const GROUNDS: &[&str] = &["floor", "ground", "terrain", "landscape", "street", "road", "room", "building", "house", "stage", "platform", "plane", "grass", "lawn", "field", "sand", "beach", "court", "pitch", "rink", "pavement", "deck", "world", "place", "interior", "facade", "table", "desk", "counter", "shelf", "bench", "bridge", "stairs", "fence"];

/// Whether a name holds one of the words (whole, or with a plural s).
pub fn has_word(name: &str, words: &[&str]) -> bool {
    let n = name.to_ascii_lowercase();
    n.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| words.iter().any(|x| w == *x || (w.len() == x.len() + 1 && w.starts_with(x) && w.ends_with('s'))))
}

/// A clip's turn and place in the scene (orientation, then X, Y, Z, as
/// the renderer places model layers), with the scale left out: the scale
/// goes into the shapes.
pub fn clip_pose(tr: &m::Transform) -> (Quat, V3) {
    use studio_sim::rigid::{quat_from_studio, studio_rotation_matrix};
    let r1 = studio_rotation_matrix(tr.orientation[0] as f32, tr.orientation[1] as f32, tr.orientation[2] as f32);
    let r2 = studio_rotation_matrix(tr.rotation_x as f32, tr.rotation_y as f32, tr.rotation as f32);
    let r: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| r1[i][k] * r2[k][j]).sum()));
    let q = if tr.orientation == [0.0; 3] { quat_from_studio(tr.rotation_x as f32, tr.rotation_y as f32, tr.rotation as f32) } else { studio_sim::dynamics::math::q_from_mat(&r) };
    (q, [tr.position.x as f32, tr.position.y as f32, tr.position_z as f32])
}

/// The model's points as the physics sees them (scaled, about the anchor).
pub fn scaled_point(tr: &m::Transform, p: V3) -> V3 {
    let k = scale_of(tr);
    [(p[0] - tr.anchor.x as f32) * k[0], (p[1] - tr.anchor.y as f32) * k[1], (p[2] - tr.anchor_z as f32) * k[2]]
}

pub fn scale_of(tr: &m::Transform) -> V3 {
    [(tr.scale_x * if tr.flip_x { -1.0 } else { 1.0 }) as f32, (tr.scale_y * if tr.flip_y { -1.0 } else { 1.0 }) as f32, tr.scale_z as f32]
}

/// A body's model pose (metres) as the clip's position and X, Y, Z turn.
pub fn pose_to_transform(pose: &Pose, ppm: f32) -> ([f64; 3], [f64; 3]) {
    let ang = studio_sim::rigid::studio_angles(pose.q);
    ([(pose.p[0] * ppm) as f64, (pose.p[1] * ppm) as f64, (pose.p[2] * ppm) as f64], [ang[0] as f64, ang[1] as f64, ang[2] as f64])
}

#[cfg(test)]
mod tests;
