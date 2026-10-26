// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D: layers in space, cameras and lights.
//!
//! A clip with its 3D switch on ([`Transform::three_d`]) is a flat card in
//! a 3D world. The world uses the canvas's own units and axes: x right and
//! y down in canvas pixels from the canvas centre, z away from the viewer.
//! The plane z = 0 is the canvas: a 3D layer at rest looks exactly like the
//! 2D layer it was, and the default camera sits in front of it at a
//! distance of [`default_zoom`] (a 50 mm lens).
//!
//! A layer turns by its orientation, then X, then Y, then Z rotation
//! (applied to the picture in the order Z, Y, X, orientation). Positive X
//! tips the top edge away, positive Y turns the right edge away, positive
//! Z (the 2D rotation) turns clockwise.
//!
//! Cameras and lights are clips too: generators of kind
//! [`GeneratorKind::Camera`] and [`GeneratorKind::Light`] placed by their
//! transform (position, `position_z`, rotations) with settings in their
//! generator parameters (see [`CameraSettings`] and [`LightSettings`]), so
//! they live on the timeline, have a time range and keyframe like anything
//! else. The topmost camera on the timeline at a time is the one that
//! looks.
//!
//! [`Transform::three_d`]: crate::Transform::three_d
//! [`GeneratorKind::Camera`]: crate::GeneratorKind::Camera
//! [`GeneratorKind::Light`]: crate::GeneratorKind::Light

use crate::clip::{Clip, ClipSource, GeneratorKind, Transform};
use crate::sequence::{LaneKind, Sequence};
use crate::time::Micros;
use crate::value::{ParamMap, Rgba, Value, Vec2};
use serde::{Deserialize, Serialize};

/// Camera distance (in pixels) at which the canvas plane shows at its own
/// size, for a canvas `width` pixels wide: a 50 mm lens on a 36 mm frame
/// (39.6° across).
pub fn default_zoom(width: f64) -> f64 {
    width * 1.388_888_9
}

/// Zoom (pixels) from a focal length in millimetres on a 36 mm wide frame.
pub fn zoom_from_focal_length(mm: f64, width: f64) -> f64 {
    width * mm.max(1.0) / 36.0
}

/// Focal length in millimetres on a 36 mm wide frame from a zoom.
pub fn focal_length_from_zoom(zoom: f64, width: f64) -> f64 {
    zoom * 36.0 / width.max(1.0)
}

/// Zoom (pixels) giving a horizontal field of view of `deg` degrees.
pub fn zoom_from_fov(deg: f64, width: f64) -> f64 {
    width * 0.5 / (deg.clamp(1.0, 179.0).to_radians() * 0.5).tan()
}

/// Horizontal field of view (degrees) of a zoom.
pub fn fov_from_zoom(zoom: f64, width: f64) -> f64 {
    2.0 * (width * 0.5 / zoom.max(1.0)).atan().to_degrees()
}

/// A 3×3 rotation matrix, rows.
pub type Mat3 = [[f64; 3]; 3];

/// Matrix product `a × b`.
pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}

/// `m × v`.
pub fn apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// The transpose (the inverse of a rotation).
pub fn transpose(m: &Mat3) -> Mat3 {
    std::array::from_fn(|r| std::array::from_fn(|c| m[c][r]))
}

/// Rotation about x by `deg`: positive tips the top (−y) away (+z).
pub fn rot_x(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
}

/// Rotation about y by `deg`: positive turns the right edge (+x) away.
pub fn rot_y(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
}

/// Rotation about z by `deg`: clockwise on screen (y down).
pub fn rot_z(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// A layer's rotation: orientation (x, y, z) then X, Y and Z rotation.
pub fn rotation(orientation: [f64; 3], rx: f64, ry: f64, rz: f64) -> Mat3 {
    let o = mul(&mul(&rot_x(orientation[0]), &rot_y(orientation[1])), &rot_z(orientation[2]));
    mul(&o, &mul(&mul(&rot_x(rx), &rot_y(ry)), &rot_z(rz)))
}

/// X, Y, Z angles (degrees) whose [`rotation`] (no orientation) is `m`.
pub fn euler(m: &Mat3) -> [f64; 3] {
    // m = Rx · Ry · Rz; m[0][2] = -sin(ry).
    let sy = (-m[0][2]).clamp(-1.0, 1.0);
    let ry = sy.asin();
    if sy.abs() < 0.999_999 {
        let rx = m[1][2].atan2(m[2][2]);
        let rz = m[0][1].atan2(m[0][0]);
        // Rz's (0,1) entry is -sin(rz) in the product; see `rot_z`.
        [rx.to_degrees(), ry.to_degrees(), (-rz).to_degrees()]
    } else {
        // Gimbal lock: put all of the remaining turn into x.
        let rx = (-m[2][1]).atan2(m[1][1]);
        [rx.to_degrees(), ry.to_degrees(), 0.0]
    }
}

/// Rotation that points the forward axis (+z) from `from` at `to`, with
/// the down axis kept as close to world down (+y) as possible, then rolled
/// clockwise by `roll` degrees. Rows are the right, down and forward axes
/// in world space, so `apply(&m, world)` gives camera coordinates.
pub fn look_at(from: [f64; 3], to: [f64; 3], roll: f64) -> Mat3 {
    let mut f = sub(to, from);
    if norm(f) < 1e-9 {
        f = [0.0, 0.0, 1.0];
    }
    let f = normalise(f);
    // Right = down × forward with world down; fall back when looking
    // straight up or down.
    let mut r = cross([0.0, 1.0, 0.0], f);
    if norm(r) < 1e-6 {
        r = cross([0.0, 0.0, -1.0], f);
    }
    let r = normalise(r);
    let d = cross(f, r);
    let base = [r, d, f];
    mul(&transpose(&rot_z(roll)), &base)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn normalise(a: [f64; 3]) -> [f64; 3] {
    let n = norm(a).max(1e-12);
    [a[0] / n, a[1] / n, a[2] / n]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// How a 3D layer (or camera) turns by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoOrient {
    /// Turns only by its rotations.
    #[default]
    Off,
    /// Faces along its motion path (steers round curves).
    Path,
    /// Always faces the camera (a billboard).
    Camera,
}

impl AutoOrient {
    /// Whether it is off.
    pub fn is_off(&self) -> bool {
        *self == AutoOrient::Off
    }

    /// Parses a name (`off`, `path`, `camera`).
    pub fn parse(s: &str) -> Option<AutoOrient> {
        Some(match s {
            "off" | "none" => AutoOrient::Off,
            "path" | "along_path" => AutoOrient::Path,
            "camera" | "towards_camera" | "billboard" => AutoOrient::Camera,
            _ => return None,
        })
    }
}

/// How a 3D layer takes light. Keyframable as `material.<field>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    /// Lights change its colour (off: shown as if unlit).
    pub accepts_lights: bool,
    /// Throws shadows onto other layers from lights that cast them.
    pub casts_shadows: bool,
    /// Shadows of other layers fall on it.
    pub accepts_shadows: bool,
    /// How much ambient light it takes, 0..1.
    pub ambient: f64,
    /// How much direct light it takes, 0..1.
    pub diffuse: f64,
    /// Strength of highlights, 0..1.
    pub specular: f64,
    /// Highlight tightness: 1 broad to 200 pin-sharp.
    pub shininess: f64,
    /// Thickness in pixels: the picture extruded back into a solid
    /// (text and logos); 0 = flat card.
    pub extrude: f64,
    /// Brightness of the extruded sides, 0..1 of the face.
    pub side_shade: f64,
}

impl Default for Material {
    fn default() -> Self {
        Material { accepts_lights: true, casts_shadows: false, accepts_shadows: true, ambient: 1.0, diffuse: 1.0, specular: 0.25, shininess: 20.0, extrude: 0.0, side_shade: 0.55 }
    }
}

impl Material {
    /// The fields that can be keyframed, by name.
    pub const ANIMATED: [&'static str; 7] = ["ambient", "diffuse", "specular", "shininess", "extrude", "side_shade", "shadow"];

    /// A field by name.
    pub fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "accepts_lights" => Value::Bool(self.accepts_lights),
            "casts_shadows" => Value::Bool(self.casts_shadows),
            "accepts_shadows" => Value::Bool(self.accepts_shadows),
            "ambient" => Value::Scalar(self.ambient),
            "diffuse" => Value::Scalar(self.diffuse),
            "specular" => Value::Scalar(self.specular),
            "shininess" => Value::Scalar(self.shininess),
            "extrude" => Value::Scalar(self.extrude),
            "side_shade" => Value::Scalar(self.side_shade),
            _ => return None,
        })
    }

    /// Sets a field by name; `false` for an unknown name or wrong kind.
    pub fn set(&mut self, key: &str, v: &Value) -> bool {
        match (key, v) {
            ("accepts_lights", Value::Bool(b)) => self.accepts_lights = *b,
            ("casts_shadows", Value::Bool(b)) => self.casts_shadows = *b,
            ("accepts_shadows", Value::Bool(b)) => self.accepts_shadows = *b,
            (k, v) => {
                let Some(x) = v.as_f64() else { return false };
                match k {
                    "ambient" => self.ambient = x.clamp(0.0, 4.0),
                    "diffuse" => self.diffuse = x.clamp(0.0, 4.0),
                    "specular" => self.specular = x.clamp(0.0, 4.0),
                    "shininess" => self.shininess = x.clamp(1.0, 1000.0),
                    "extrude" => self.extrude = x.clamp(0.0, 4000.0),
                    "side_shade" => self.side_shade = x.clamp(0.0, 1.0),
                    _ => return false,
                }
            }
        }
        true
    }
}

fn num(p: &ParamMap, k: &str, d: f64) -> f64 {
    p.get(k).and_then(Value::as_f64).unwrap_or(d)
}
fn flag(p: &ParamMap, k: &str, d: bool) -> bool {
    p.get(k).and_then(Value::as_bool).unwrap_or(d)
}
fn point(p: &ParamMap, k: &str) -> Option<Vec2> {
    p.get(k).and_then(Value::as_point)
}

/// A camera clip's settings, read from its generator parameters.
///
/// Parameters: `zoom` (pixels from the lens to the plane shown at its own
/// size; `focal_length` in mm or `fov` in degrees are accepted when
/// writing and converted), `two_node` (aims at `poi`, `poi_z`), `dof`,
/// `focus_distance` (pixels), `aperture` (pixels, how wide the lens is
/// open), `blur_level` (0..1+).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct CameraSettings {
    pub zoom: f64,
    /// Aims at the point of interest (else it looks along its rotation).
    pub two_node: bool,
    pub poi: [f64; 3],
    pub dof: bool,
    pub focus_distance: f64,
    pub aperture: f64,
    pub blur_level: f64,
}

impl CameraSettings {
    /// Settings for a canvas `width` pixels wide.
    pub fn read(p: &ParamMap, width: f64) -> CameraSettings {
        let zoom = default_zoom(width);
        let poi = point(p, "poi").unwrap_or(Vec2::ZERO);
        CameraSettings {
            zoom: num(p, "zoom", zoom).max(1.0),
            two_node: flag(p, "two_node", false),
            poi: [poi.x, poi.y, num(p, "poi_z", 0.0)],
            dof: flag(p, "dof", false),
            focus_distance: num(p, "focus_distance", zoom).max(1.0),
            aperture: num(p, "aperture", 25.0).max(0.0),
            blur_level: num(p, "blur_level", 1.0).max(0.0),
        }
    }

    /// Default parameters of a new camera.
    pub fn defaults(width: f64) -> ParamMap {
        let mut p = ParamMap::new();
        p.insert("zoom".into(), Value::Scalar(default_zoom(width)));
        p.insert("two_node".into(), Value::Bool(false));
        p.insert("poi".into(), Value::Point(Vec2::ZERO));
        p.insert("poi_z".into(), Value::Scalar(0.0));
        p.insert("dof".into(), Value::Bool(false));
        p.insert("focus_distance".into(), Value::Scalar(default_zoom(width)));
        p.insert("aperture".into(), Value::Scalar(25.0));
        p.insert("blur_level".into(), Value::Scalar(1.0));
        p
    }
}

/// Kinds of light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    /// From a direction, like the sun: even over everything.
    Parallel,
    /// A cone from a point, aimed at its point of interest.
    Spot,
    /// From a point in every direction, like a bulb.
    #[default]
    Point,
    /// Everywhere at once, no direction or shadows.
    Ambient,
}

impl LightKind {
    /// Every kind's name.
    pub const NAMES: [&'static str; 4] = ["parallel", "spot", "point", "ambient"];

    /// Parses a name.
    pub fn parse(s: &str) -> Option<LightKind> {
        Some(match s {
            "parallel" | "directional" | "sun" => LightKind::Parallel,
            "spot" | "spotlight" => LightKind::Spot,
            "point" | "bulb" | "omni" => LightKind::Point,
            "ambient" => LightKind::Ambient,
            _ => return None,
        })
    }

    /// The name.
    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

/// A light clip's settings, read from its generator parameters.
///
/// Parameters: `light_type` (choice), `colour`, `intensity` (1 = 100 %),
/// `poi`/`poi_z` (where parallel and spot lights aim), `cone_angle`
/// (degrees), `cone_feather` (0..1), `radius` and `falloff` (pixels: full
/// strength within the radius, fading to nothing over the falloff; 0 =
/// no falloff), `casts_shadows`, `shadow_darkness` (0..1).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct LightSettings {
    pub kind: LightKind,
    pub colour: Rgba,
    pub intensity: f64,
    pub poi: [f64; 3],
    pub cone_angle: f64,
    pub cone_feather: f64,
    pub radius: f64,
    pub falloff: f64,
    pub casts_shadows: bool,
    pub shadow_darkness: f64,
}

impl LightSettings {
    /// Reads a light's parameters.
    pub fn read(p: &ParamMap) -> LightSettings {
        let poi = point(p, "poi").unwrap_or(Vec2::ZERO);
        LightSettings {
            kind: p.get("light_type").and_then(Value::as_str).and_then(LightKind::parse).unwrap_or_default(),
            colour: p.get("colour").and_then(Value::as_colour).unwrap_or(Rgba::new(1.0, 1.0, 1.0, 1.0)),
            intensity: num(p, "intensity", 1.0).max(0.0),
            poi: [poi.x, poi.y, num(p, "poi_z", 0.0)],
            cone_angle: num(p, "cone_angle", 90.0).clamp(1.0, 179.0),
            cone_feather: num(p, "cone_feather", 0.5).clamp(0.0, 1.0),
            radius: num(p, "radius", 0.0).max(0.0),
            falloff: num(p, "falloff", 0.0).max(0.0),
            casts_shadows: flag(p, "casts_shadows", false),
            shadow_darkness: num(p, "shadow_darkness", 0.6).clamp(0.0, 1.0),
        }
    }

    /// Default parameters of a new light of `kind`.
    pub fn defaults(kind: LightKind) -> ParamMap {
        let mut p = ParamMap::new();
        p.insert("light_type".into(), Value::Choice(kind.name().into()));
        p.insert("colour".into(), Value::Colour(Rgba::new(1.0, 1.0, 1.0, 1.0)));
        p.insert("intensity".into(), Value::Scalar(if kind == LightKind::Ambient { 0.3 } else { 1.0 }));
        p.insert("poi".into(), Value::Point(Vec2::ZERO));
        p.insert("poi_z".into(), Value::Scalar(0.0));
        if kind == LightKind::Spot {
            p.insert("cone_angle".into(), Value::Angle(90.0));
            p.insert("cone_feather".into(), Value::Scalar(0.5));
        }
        if kind != LightKind::Ambient {
            p.insert("casts_shadows".into(), Value::Bool(false));
            p.insert("shadow_darkness".into(), Value::Scalar(0.6));
        }
        p
    }
}

/// What a camera sees from, at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Lens position in the world.
    pub position: [f64; 3],
    /// Rows: the camera's right, down and forward axes in the world.
    pub axes: Mat3,
    /// Pixels from the lens to the plane that shows at its own size.
    pub zoom: f64,
}

impl View {
    /// The default camera of a canvas `width` pixels wide: straight on,
    /// the canvas plane at its own size.
    pub fn default_for(width: f64) -> View {
        let zoom = default_zoom(width);
        View { position: [0.0, 0.0, -zoom], axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], zoom }
    }

    /// Camera coordinates (right, down, depth) of a world point.
    pub fn view(&self, p: [f64; 3]) -> [f64; 3] {
        apply(&self.axes, sub(p, self.position))
    }

    /// Screen position (canvas pixels from the canvas centre) of a world
    /// point and its depth; `None` behind the lens.
    pub fn project(&self, p: [f64; 3]) -> Option<(Vec2, f64)> {
        let v = self.view(p);
        (v[2] > 1.0).then(|| (Vec2::new(self.zoom * v[0] / v[2], self.zoom * v[1] / v[2]), v[2]))
    }

    /// The world direction of the ray through a screen point (canvas
    /// pixels from the centre).
    pub fn ray(&self, screen: Vec2) -> [f64; 3] {
        apply(&transpose(&self.axes), [screen.x / self.zoom, screen.y / self.zoom, 1.0])
    }
}

/// A flat picture in the world: `world(u, v) = origin + u·eu + v·ev` for
/// source pixels `(u, v)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// Where source pixel (0, 0) is.
    pub origin: [f64; 3],
    /// World step per source pixel to the right.
    pub eu: [f64; 3],
    /// World step per source pixel down.
    pub ev: [f64; 3],
}

impl Plane {
    /// The plane of a 3D layer whose source is `source` pixels, fitted
    /// into the canvas at `fit` canvas pixels per source pixel.
    pub fn of(tr: &Transform, source: Vec2, fit: f64) -> Plane {
        let r = rotation(tr.orientation, tr.rotation_x, tr.rotation_y, tr.rotation);
        let col = |c: usize| [r[0][c], r[1][c], r[2][c]];
        let sx = fit * tr.scale_x * if tr.flip_x { -1.0 } else { 1.0 };
        let sy = fit * tr.scale_y * if tr.flip_y { -1.0 } else { 1.0 };
        let (eu, ev, ez) = (scale(col(0), sx), scale(col(1), sy), scale(col(2), tr.scale_z));
        let a = [-(source.x * 0.5 + tr.anchor.x), -(source.y * 0.5 + tr.anchor.y)];
        let pos = [tr.position.x, tr.position.y, tr.position_z];
        Plane { origin: add(add(pos, add(scale(eu, a[0]), scale(ev, a[1]))), scale(ez, -tr.anchor_z)), eu, ev }
    }

    /// A source point in the world.
    pub fn at(&self, u: f64, v: f64) -> [f64; 3] {
        add(self.origin, add(scale(self.eu, u), scale(self.ev, v)))
    }

    /// Where a ray from `from` along `dir` meets the plane, as source
    /// pixels and distance along the ray; `None` when it runs alongside.
    pub fn hit(&self, from: [f64; 3], dir: [f64; 3]) -> Option<(f64, f64, f64)> {
        // origin + u·eu + v·ev − t·dir = from, by Cramer's rule.
        let b = sub(from, self.origin);
        let nd = scale(dir, -1.0);
        let det = |a: [f64; 3], b_: [f64; 3], c: [f64; 3]| dot(a, cross(b_, c));
        let d = det(self.eu, self.ev, nd);
        if d.abs() < 1e-12 {
            return None;
        }
        Some((det(b, self.ev, nd) / d, det(self.eu, b, nd) / d, det(self.eu, self.ev, b) / d))
    }
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Direction of travel of a clip at sequence time `t` (unit, world), when
/// it moves.
pub fn travel(seq: &Sequence, clip: &Clip, t: Micros) -> Option<[f64; 3]> {
    let dt = 10_000;
    let at = |t: Micros| {
        let tr = seq.placed_transform(clip, t.clamp(clip.start, clip.end() - 1));
        [tr.position.x, tr.position.y, tr.position_z]
    };
    let d = sub(at(t + dt), at(t - dt));
    let n = norm(d);
    (n > 1e-6).then(|| scale(d, 1.0 / n))
}

/// A camera clip's view at sequence time `t` on a canvas `width` wide.
pub fn camera_view(seq: &Sequence, clip: &Clip, t: Micros, width: f64) -> View {
    let tr = seq.placed_transform(clip, t);
    let set = CameraSettings::read(&clip.generator_params_at((t - clip.start).clamp(0, clip.duration)), width);
    let pos = [tr.position.x, tr.position.y, tr.position_z];
    let axes = match travel(seq, clip, t).filter(|_| tr.auto_orient == AutoOrient::Path) {
        Some(d) => look_at(pos, add(pos, d), tr.rotation),
        None if set.two_node => look_at(pos, set.poi, tr.rotation),
        None => transpose(&rotation(tr.orientation, tr.rotation_x, tr.rotation_y, tr.rotation)),
    };
    View { position: pos, axes, zoom: set.zoom }
}

/// The camera that looks at sequence time `t`: the enabled camera clip on
/// the highest visible lane there (the latest to start breaks a tie).
pub fn active_camera(seq: &Sequence, t: Micros) -> Option<&Clip> {
    seq.clips
        .iter()
        .filter(|c| c.enabled && t >= c.start && t < c.end() && matches!(&c.source, ClipSource::Generator(g) if g.kind == GeneratorKind::Camera))
        .filter_map(|c| {
            let lane = seq.lane(c.lane).filter(|l| !l.hidden && l.kind != LaneKind::Audio)?;
            Some(((seq.lane_position(lane.id)?, c.start, c.id.0), c))
        })
        .max_by_key(|(k, _)| *k)
        .map(|(_, c)| c)
}

/// The view 3D layers are seen through at `t`: the active camera's, or the
/// default camera's.
pub fn view_at(seq: &Sequence, t: Micros, width: f64) -> View {
    match active_camera(seq, t) {
        Some(c) => camera_view(seq, c, t, width),
        None => View::default_for(width),
    }
}

/// A clip's transform at `t` as it is drawn: its own, carried by its
/// parents and, for a 3D layer, turned by its auto-orient (along its path,
/// or to face the camera).
pub fn oriented_transform(seq: &Sequence, clip: &Clip, t: Micros, width: f64) -> Transform {
    let mut tr = seq.placed_transform(clip, t);
    if !tr.three_d || tr.auto_orient == AutoOrient::Off {
        return tr;
    }
    let face = match tr.auto_orient {
        AutoOrient::Off => return tr,
        AutoOrient::Path => {
            // Steers: its right edge leads along the path, its face kept
            // towards the viewer as far as the path allows.
            let Some(d) = travel(seq, clip, t) else { return tr };
            let off = |axis: [f64; 3]| {
                let v = sub(axis, scale(d, dot(axis, d)));
                let n = norm(v);
                (n > 1e-6).then(|| scale(v, 1.0 / n))
            };
            let Some(back) = off([0.0, 0.0, 1.0]).or_else(|| off([0.0, -1.0, 0.0])) else { return tr };
            let down = cross(back, d);
            std::array::from_fn(|r| [d[r], down[r], back[r]])
        }
        AutoOrient::Camera => {
            let eye = view_at(seq, t, width).position;
            transpose(&look_at(eye, [tr.position.x, tr.position.y, tr.position_z], 0.0))
        }
    };
    let own = rotation([0.0; 3], tr.rotation_x, tr.rotation_y, tr.rotation);
    let [rx, ry, rz] = euler(&mul(&face, &own));
    (tr.rotation_x, tr.rotation_y, tr.rotation, tr.orientation) = (rx, ry, rz, [0.0; 3]);
    tr
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn rotation_directions() {
        // Y: the right edge goes away.
        assert!(close(apply(&rot_y(90.0), [1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]));
        // X: the top edge goes away.
        assert!(close(apply(&rot_x(90.0), [0.0, -1.0, 0.0]), [0.0, 0.0, 1.0]));
        // Z: clockwise on screen, as in 2D.
        assert!(close(apply(&rot_z(90.0), [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn euler_round_trips() {
        for (x, y, z) in [(10.0, 20.0, 30.0), (-45.0, 60.0, -120.0), (0.0, 0.0, 90.0)] {
            let e = euler(&rotation([0.0; 3], x, y, z));
            assert!(close(e, [x, y, z]), "{e:?} vs {:?}", [x, y, z]);
        }
    }

    #[test]
    fn look_at_points_forward() {
        let m = look_at([0.0, 0.0, -100.0], [0.0, 0.0, 0.0], 0.0);
        assert!(close(m[2], [0.0, 0.0, 1.0]) && close(m[0], [1.0, 0.0, 0.0]) && close(m[1], [0.0, 1.0, 0.0]));
        let m = look_at([0.0, 0.0, 0.0], [100.0, 0.0, 0.0], 0.0);
        assert!(close(m[2], [1.0, 0.0, 0.0]));
        // Straight down does not break.
        let m = look_at([0.0, -100.0, 0.0], [0.0, 0.0, 0.0], 0.0);
        assert!(m.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn rest_plane_projects_like_2d() {
        let w = 1920.0;
        let tr = Transform { three_d: true, position: Vec2::new(100.0, -50.0), rotation: 30.0, ..Transform::default() };
        let plane = Plane::of(&tr, Vec2::new(1920.0, 1080.0), 1.0);
        let view = View::default_for(w);
        // The anchor (the source centre) lands on the position.
        let (c, d) = view.project(plane.at(960.0, 540.0)).unwrap();
        assert!((c.x - 100.0).abs() < 1e-6 && (c.y + 50.0).abs() < 1e-6 && (d - default_zoom(w)).abs() < 1e-6);
        // A ray back through a projected point hits the same source pixel.
        let (s, _) = view.project(plane.at(300.0, 200.0)).unwrap();
        let (u, v, _) = plane.hit(view.position, view.ray(s)).unwrap();
        assert!((u - 300.0).abs() < 1e-6 && (v - 200.0).abs() < 1e-6);
    }

    #[test]
    fn lens_conversions() {
        let w = 1920.0;
        assert!((focal_length_from_zoom(default_zoom(w), w) - 50.0).abs() < 1e-3);
        assert!((fov_from_zoom(zoom_from_fov(60.0, w), w) - 60.0).abs() < 1e-9);
        assert!((zoom_from_focal_length(50.0, w) - default_zoom(w)).abs() < 1e-3);
    }
}
