// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D: layers as cards in space, seen through a camera and lit by lights.
//!
//! The world shares the canvas's units and axes: x right and y down in
//! canvas pixels from the canvas centre, z away from the viewer. The plane
//! z = 0 is the canvas, and the default camera sees it at exactly its own
//! size, so a 3D layer at rest draws like the 2D layer it was.
//!
//! A 3D layer's picture is a flat [`Plane`]: image pixels map affinely to
//! world points, and the camera maps those to the screen by a
//! [`Homography`] whose third coordinate is the depth in front of the lens.
//! The compositors inverse-map each output pixel through it (behind the
//! lens is empty), light the point (ambient, parallel, point and spot
//! lights with Blinn–Phong highlights and soft cone edges) and test
//! shadows by casting a ray to each light through the layers that throw
//! them. Contiguous runs of 3D layers draw far to near ([`draw_order`]);
//! a 2D layer, an adjustment, a transition or a matte ends a run, so
//! stacking above and below a 3D group still follows the timeline.
//!
//! The CPU functions here are the reference; `composite3d.wgsl` mirrors
//! [`shade`] and [`shadow`] line for line.

use crate::resolve::{Node, RenderList};
use crate::scene::{Canvas, ClipDesc};
use crate::transform::{Affine, Homography, Placement, Pose, cropped_centre};

/// Lens distance (pixels) that shows the canvas plane at its own size: a
/// 50 mm lens on a 36 mm frame.
pub fn default_zoom(width: f32) -> f32 {
    width * 1.388_888_9
}

/// Nothing nearer the lens than this (pixels) is drawn.
pub const NEAR: f32 = 1.0;

/// Most shadow-throwing layers in one 3D run.
pub const MAX_CASTERS: usize = 4;

/// Most lights that count.
pub const MAX_LIGHTS: usize = 8;

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalise(a: V3) -> V3 {
    scale(a, 1.0 / length(a).max(1e-20))
}

type M3 = [[f32; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]))
}
fn apply(m: &M3, v: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}
fn rot_x(deg: f32) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
}
fn rot_y(deg: f32) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
}
fn rot_z(deg: f32) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// A layer's rotation (columns are its right, down and back axes in the
/// world): orientation, then X, Y, Z (`studio_model::space::rotation`).
pub fn rotation(orientation: [f32; 3], rx: f32, ry: f32, rz: f32) -> M3 {
    let o = mul(&mul(&rot_x(orientation[0]), &rot_y(orientation[1])), &rot_z(orientation[2]));
    mul(&o, &mul(&mul(&rot_x(rx), &rot_y(ry)), &rot_z(rz)))
}

/// Camera axes (rows: right, down, forward) looking from `from` at `to`,
/// rolled clockwise by `roll` degrees (`studio_model::space::look_at`).
pub fn look_at(from: V3, to: V3, roll: f32) -> M3 {
    let mut f = sub(to, from);
    if length(f) < 1e-6 {
        f = [0.0, 0.0, 1.0];
    }
    let f = normalise(f);
    let mut r = cross([0.0, 1.0, 0.0], f);
    if length(r) < 1e-6 {
        r = cross([0.0, 0.0, -1.0], f);
    }
    let r = normalise(r);
    let d = cross(f, r);
    let z = rot_z(roll);
    let zt: M3 = std::array::from_fn(|i| std::array::from_fn(|j| z[j][i]));
    mul(&zt, &[r, d, f])
}

/// How a 3D layer takes light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialDesc {
    pub accepts_lights: bool,
    pub casts_shadows: bool,
    pub accepts_shadows: bool,
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    pub shininess: f32,
    /// Thickness in canvas pixels (0 = a flat card).
    pub extrude: f32,
    /// Brightness of the extruded sides relative to the face.
    pub side_shade: f32,
}

impl Default for MaterialDesc {
    fn default() -> MaterialDesc {
        MaterialDesc { accepts_lights: true, casts_shadows: false, accepts_shadows: true, ambient: 1.0, diffuse: 1.0, specular: 0.25, shininess: 20.0, extrude: 0.0, side_shade: 0.55 }
    }
}

/// A 3D layer's place in space, on top of its [`Placement`] (whose x/y
/// position, scale, z rotation, anchor, crop and flips still apply).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    /// Depth of the anchor; positive is further away.
    pub z: f32,
    pub anchor_z: f32,
    pub scale_z: f32,
    pub rotation_x: f32,
    pub rotation_y: f32,
    pub orientation: [f32; 3],
    pub material: MaterialDesc,
}

impl Default for Space {
    fn default() -> Space {
        Space { z: 0.0, anchor_z: 0.0, scale_z: 1.0, rotation_x: 0.0, rotation_y: 0.0, orientation: [0.0; 3], material: MaterialDesc::default() }
    }
}

/// Depth of field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dof {
    /// Distance in focus (pixels from the lens).
    pub focus: f32,
    /// How wide the lens is open (pixels); blur grows with it.
    pub aperture: f32,
    /// Multiplies the blur.
    pub blur_level: f32,
}

/// The camera at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraDesc {
    /// Lens position in the world.
    pub position: V3,
    /// Rows: the camera's right, down and forward axes in the world.
    pub axes: M3,
    /// Distance (pixels) at which the plane facing the lens shows at its
    /// own size.
    pub zoom: f32,
    pub dof: Option<Dof>,
    /// Path-traced final renders of models seen through this camera
    /// (else they are drawn with the raster lighting).
    pub trace: Option<crate::trace::TraceSettings>,
}

impl CameraDesc {
    /// The comp camera: straight on, canvas plane at its own size.
    pub fn default_for(canvas: &Canvas) -> CameraDesc {
        let zoom = default_zoom(canvas.width as f32);
        CameraDesc { position: [0.0, 0.0, -zoom], axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], zoom, dof: None, trace: None }
    }

    /// Camera coordinates (right, down, depth) of a world point.
    pub fn view(&self, p: V3) -> V3 {
        apply(&self.axes, sub(p, self.position))
    }

    /// Screen position (canvas pixels from the top left) and depth.
    pub fn project(&self, canvas: [f32; 2], p: V3) -> ([f32; 2], f32) {
        let v = self.view(p);
        let w = v[2].max(1e-6);
        ([canvas[0] * 0.5 + self.zoom * v[0] / w, canvas[1] * 0.5 + self.zoom * v[1] / w], v[2])
    }

    /// The world point on the plane at depth `depth` (pixels in front of
    /// the lens) under a screen position.
    pub fn unproject(&self, canvas: [f32; 2], screen: [f32; 2], depth: f32) -> V3 {
        let x = (screen[0] - canvas[0] * 0.5) * depth / self.zoom;
        let y = (screen[1] - canvas[1] * 0.5) * depth / self.zoom;
        let a = &self.axes;
        add(self.position, add(add(scale(a[0], x), scale(a[1], y)), scale(a[2], depth)))
    }
}

/// Kinds of light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Parallel = 0,
    Spot = 1,
    Point = 2,
    Ambient = 3,
}

/// A light at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightDesc {
    pub kind: LightKind,
    pub position: V3,
    /// Unit direction the light travels (parallel and spot).
    pub direction: V3,
    /// Linear RGB times intensity.
    pub colour: V3,
    /// Spot cone: cosines of the outer edge and of where full strength
    /// begins.
    pub cone: [f32; 2],
    /// Full strength within `radius`, fading to nothing over `falloff`
    /// pixels beyond it (0 = no falloff).
    pub radius: f32,
    pub falloff: f32,
    pub casts_shadows: bool,
    pub shadow_darkness: f32,
    /// A sky (daylight or a panorama) this light brings: it lights 3D
    /// models from every direction and can show behind them; a parallel
    /// light with a sky is its sun.
    pub sky: Option<crate::sky::SkyDesc>,
    /// How the light shows itself, or the scene's air and look when this
    /// light carries them (see [`crate::atmosphere`]).
    pub look: Option<crate::atmosphere::LightLook>,
}

/// A flat picture in the world: `world(u, v) = origin + u·eu + v·ev` for
/// picture pixels `(u, v)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub origin: V3,
    pub eu: V3,
    pub ev: V3,
    /// World step for one pixel of the layer's own depth (towards its back).
    pub back: V3,
}

impl Plane {
    pub fn at(&self, p: [f32; 2]) -> V3 {
        add(self.origin, add(scale(self.eu, p[0]), scale(self.ev, p[1])))
    }

    /// The same plane for pictures related by `a` (picture px → this
    /// plane's px).
    pub fn then_after(&self, a: &Affine) -> Plane {
        let m = &a.0;
        Plane {
            origin: self.at([m[0][2], m[1][2]]),
            eu: add(scale(self.eu, m[0][0]), scale(self.ev, m[1][0])),
            ev: add(scale(self.eu, m[0][1]), scale(self.ev, m[1][1])),
            back: self.back,
        }
    }

    /// Moved `d` pixels of its own depth back.
    pub fn pushed(&self, d: f32) -> Plane {
        Plane { origin: add(self.origin, scale(self.back, d)), ..*self }
    }

    /// Unit normal (towards the layer's front, before facing).
    pub fn normal(&self) -> V3 {
        normalise(cross(self.ev, self.eu))
    }

    /// The screen map through `camera`: plane px → canvas px, with the
    /// depth in front of the lens as the third coordinate.
    pub fn homography(&self, camera: &CameraDesc, canvas: [f32; 2]) -> Homography {
        let a = &camera.axes;
        let o = sub(self.origin, camera.position);
        let row = |axis: V3| [dot(axis, self.eu), dot(axis, self.ev), dot(axis, o)];
        let (x, y, w) = (row(a[0]), row(a[1]), row(a[2]));
        let z = camera.zoom;
        Homography([
            std::array::from_fn(|i| z * x[i] + canvas[0] * 0.5 * w[i]),
            std::array::from_fn(|i| z * y[i] + canvas[1] * 0.5 * w[i]),
            w,
        ])
    }
}

/// The plane of a 3D layer's full source picture (source px → world),
/// with the pose folded in: its offset moves, its scale and turns add to
/// the layer's own.
pub fn layer_plane(canvas: [f32; 2], source: [f32; 2], p: &Placement, s: &Space, pose: &Pose) -> Plane {
    let fit = crate::transform::fit_scale(canvas, source);
    let sx = fit * p.scale[0] * pose.scale[0] * if p.flip_x { -1.0 } else { 1.0 };
    let sy = fit * p.scale[1] * pose.scale[1] * if p.flip_y { -1.0 } else { 1.0 };
    let r = rotation(s.orientation, s.rotation_x + pose.turn[0], s.rotation_y + pose.turn[1], p.rotation + pose.rotation);
    let col = |c: usize| [r[0][c], r[1][c], r[2][c]];
    let pos = [p.position[0] + pose.offset[0] * canvas[0], p.position[1] + pose.offset[1] * canvas[1], s.z + pose.depth * canvas[0]];
    let (eu, ev, ez) = (scale(col(0), sx), scale(col(1), sy), scale(col(2), s.scale_z));
    // Source (0, 0) sits at minus the anchor (centre + anchor), and the
    // anchor's own depth is behind the picture.
    let a = [-(source[0] * 0.5 + p.anchor[0]), -(source[1] * 0.5 + p.anchor[1])];
    let origin = add(pos, add(add(scale(eu, a[0]), scale(ev, a[1])), scale(ez, -s.anchor_z)));
    Plane { origin, eu, ev, back: ez }
}

/// The plane of a 2D layer turned by a pose's 3D part: placed flat on the
/// canvas by `m` (source px → canvas px), then turned about the picture's
/// centre `centre` (source px) and pushed back by the pose's depth.
pub fn posed_plane(canvas: [f32; 2], m: &Affine, centre: [f32; 2], pose: &Pose) -> Plane {
    let a = &m.0;
    let flat = Plane { origin: [a[0][2] - canvas[0] * 0.5, a[1][2] - canvas[1] * 0.5, 0.0], eu: [a[0][0], a[1][0], 0.0], ev: [a[0][1], a[1][1], 0.0], back: [0.0, 0.0, 1.0] };
    let c = flat.at(centre);
    let r = rotation([0.0; 3], pose.turn[0], pose.turn[1], 0.0);
    let turn = |v: V3| apply(&r, v);
    Plane { origin: add(add(c, turn(sub(flat.origin, c))), [0.0, 0.0, pose.depth * canvas[0]]), eu: turn(flat.eu), ev: turn(flat.ev), back: turn(flat.back) }
}

/// Centre of a layer's visible picture in front of the lens, for sorting;
/// larger is further.
pub fn plane_depth(camera: &CameraDesc, plane: &Plane, visible: [f32; 4]) -> f32 {
    camera.view(plane.at([(visible[0] + visible[2]) * 0.5, (visible[1] + visible[3]) * 0.5]))[2]
}

/// Draw order of a resolved list: node indices bottom to top, with each
/// contiguous run of 3D layers sorted far to near by `depth` (ties keep
/// timeline order). `depth` gives `None` for a layer that cannot be
/// placed (it keeps its place).
pub fn draw_order(list: &RenderList, mut depth: impl FnMut(&ClipDesc, &Pose) -> Option<f32>) -> Vec<usize> {
    let in_space = |n: &Node| matches!(n, Node::Layer { clip, .. } if clip.space.is_some() && !clip.blend.is_matte());
    let mut order = Vec::with_capacity(list.nodes.len());
    let mut i = 0;
    while i < list.nodes.len() {
        if !in_space(&list.nodes[i]) {
            order.push(i);
            i += 1;
            continue;
        }
        let start = i;
        while i < list.nodes.len() && in_space(&list.nodes[i]) {
            i += 1;
        }
        let mut run: Vec<(usize, f32)> = (start..i)
            .map(|k| {
                let d = match &list.nodes[k] {
                    Node::Layer { clip, pose } => depth(clip, pose),
                    _ => None,
                };
                (k, d.unwrap_or(f32::NEG_INFINITY))
            })
            .collect();
        run.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        order.extend(run.into_iter().map(|(k, _)| k));
    }
    order
}

/// A layer that throws shadows: its plane in picture pixels with the dual
/// basis that maps a world point back to them.
#[derive(Clone, Copy, Debug)]
pub struct Caster {
    pub origin: V3,
    /// `u = (x − origin)·du`, `v = (x − origin)·dv` for x on the plane.
    pub du: V3,
    pub dv: V3,
    pub normal: V3,
    pub visible: [f32; 4],
    /// The clip it belongs to (a layer never shadows itself).
    pub id: u64,
}

impl Caster {
    pub fn new(plane: &Plane, visible: [f32; 4], id: u64) -> Option<Caster> {
        let n = cross(plane.eu, plane.ev);
        let a = cross(plane.ev, n);
        let b = cross(n, plane.eu);
        let (ka, kb) = (dot(plane.eu, a), dot(plane.ev, b));
        if ka.abs() < 1e-20 || kb.abs() < 1e-20 {
            return None;
        }
        Some(Caster { origin: plane.origin, du: scale(a, 1.0 / ka), dv: scale(b, 1.0 / kb), normal: normalise(n), visible, id })
    }
}

/// Fraction of `light` reaching world point `p` past the casters (1 = no
/// shadow). `alpha(i, u, v)` samples caster `i`'s picture coverage.
pub fn shadow(p: V3, light: &LightDesc, casters: &[Caster], skip: u64, mut alpha: impl FnMut(usize, f32, f32) -> f32) -> f32 {
    let (ray, reach) = match light.kind {
        LightKind::Parallel => (scale(light.direction, -1.0), f32::INFINITY),
        LightKind::Ambient => return 1.0,
        _ => {
            let d = sub(light.position, p);
            let l = length(d);
            (scale(d, 1.0 / l.max(1e-6)), l)
        }
    };
    let mut lit = 1.0;
    for (i, c) in casters.iter().enumerate() {
        if c.id == skip {
            continue;
        }
        let denom = dot(c.normal, ray);
        if denom.abs() < 1e-9 {
            continue;
        }
        // Distance along the ray to the caster's plane.
        let t = dot(c.normal, sub(c.origin, p)) / denom;
        if t <= 0.01 || t >= reach {
            continue;
        }
        let d = sub(add(p, scale(ray, t)), c.origin);
        let (u, v) = (dot(d, c.du), dot(d, c.dv));
        if u < c.visible[0] || u > c.visible[2] || v < c.visible[1] || v > c.visible[3] {
            continue;
        }
        lit *= 1.0 - light.shadow_darkness * alpha(i, u, v).clamp(0.0, 1.0);
    }
    lit
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Lights a premultiplied pixel at world point `p` on a surface with
/// normal `n`, seen from `eye`. `shadow_of(light index)` gives how much of
/// that light gets through. `side` darkens extruded sides.
pub fn shade(px: [f32; 4], p: V3, n: V3, eye: V3, m: &MaterialDesc, lights: &[LightDesc], side: f32, mut shadow_of: impl FnMut(usize) -> f32) -> [f32; 4] {
    let view = normalise(sub(eye, p));
    let n = if dot(n, view) < 0.0 { scale(n, -1.0) } else { n };
    let mut amb = [0.0f32; 3];
    let mut dif = [0.0f32; 3];
    let mut spec = [0.0f32; 3];
    for (i, l) in lights.iter().enumerate() {
        if l.kind == LightKind::Ambient {
            amb = add(amb, l.colour);
            continue;
        }
        let (to, mut atten) = if l.kind == LightKind::Parallel {
            (scale(l.direction, -1.0), 1.0)
        } else {
            let d = sub(l.position, p);
            let dist = length(d);
            let f = if l.falloff > 0.0 { 1.0 - smoothstep(l.radius, l.radius + l.falloff, dist) } else { 1.0 };
            (scale(d, 1.0 / dist.max(1e-6)), f)
        };
        if l.kind == LightKind::Spot {
            atten *= smoothstep(l.cone[0], l.cone[1], dot(scale(to, -1.0), l.direction));
        }
        let ndl = dot(n, to);
        if ndl <= 0.0 || atten <= 0.0 {
            continue;
        }
        let sh = if l.casts_shadows && m.accepts_shadows { shadow_of(i) } else { 1.0 };
        let k = atten * sh;
        dif = add(dif, scale(l.colour, ndl * k));
        let h = normalise(add(to, view));
        spec = add(spec, scale(l.colour, dot(n, h).max(0.0).powf(m.shininess) * k));
    }
    let a = px[3];
    let lit: [f32; 3] = std::array::from_fn(|c| (amb[c] * m.ambient + dif[c] * m.diffuse) * side);
    [px[0] * lit[0] + spec[0] * m.specular * a, px[1] * lit[1] + spec[1] * m.specular * a, px[2] * lit[2] + spec[2] * m.specular * a, a]
}

/// The projective form of a placed layer, when it is in 3D (its own switch
/// or a pose that turns it).
#[derive(Clone, Copy, Debug)]
pub struct Proj {
    /// Picture px → target px; the third coordinate is the depth.
    pub h: Homography,
    /// Picture px → world.
    pub plane: Plane,
    pub material: MaterialDesc,
    /// Lights and shadows apply (a 3D layer in a lit scene that accepts
    /// them).
    pub lit: bool,
    /// Depth of its centre, for sorting.
    pub depth: f32,
}

/// The source plane and camera of a clip, when it draws in perspective:
/// its own 3D switch (seen through `camera`) or a 3D pose on a 2D layer
/// (seen through the default camera).
pub fn source_plane(canvas: &Canvas, source: [f32; 2], clip: &ClipDesc, pose: &Pose, camera: &CameraDesc) -> Option<(Plane, CameraDesc)> {
    if let Some(s) = &clip.space {
        return Some((layer_plane(canvas.size(), source, &clip.placement, s, pose), *camera));
    }
    if !pose.is_3d() {
        return None;
    }
    let flat = Pose { turn: [0.0; 2], depth: 0.0, ..*pose };
    let m = crate::transform::layer_matrix(canvas.size(), source, &clip.placement, &flat);
    let centre = cropped_centre(source, clip.placement.crop);
    Some((posed_plane(canvas.size(), &m, centre, pose), CameraDesc::default_for(canvas)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2
    }

    #[test]
    fn rest_layer_matches_2d() {
        let canvas = Canvas::new(1920, 1080, 30.0);
        let p = Placement { position: [100.0, -40.0], scale: [0.5, 0.7], rotation: 20.0, anchor: [10.0, 5.0], ..Default::default() };
        let plane = layer_plane(canvas.size(), [1280.0, 720.0], &p, &Space::default(), &Pose::REST);
        let h = plane.homography(&CameraDesc::default_for(&canvas), canvas.size());
        let m = crate::transform::placement_matrix(canvas.size(), [1280.0, 720.0], &p);
        for pt in [[0.0, 0.0], [640.0, 360.0], [1280.0, 720.0], [33.0, 600.0]] {
            assert!(close(h.apply(pt), m.apply(pt)), "{pt:?}: {:?} vs {:?}", h.apply(pt), m.apply(pt));
        }
    }

    #[test]
    fn depth_shrinks_and_turn_foreshortens() {
        let canvas = Canvas::new(200, 100, 30.0);
        let cam = CameraDesc::default_for(&canvas);
        let far = Space { z: cam.zoom, ..Space::default() };
        let h = layer_plane(canvas.size(), [200.0, 100.0], &Placement::default(), &far, &Pose::REST).homography(&cam, canvas.size());
        // Twice as far: half the size about the centre.
        assert!(close(h.apply([0.0, 0.0]), [50.0, 25.0]));
        let turned = Space { rotation_y: 60.0, ..Space::default() };
        let h = layer_plane(canvas.size(), [200.0, 100.0], &Placement::default(), &turned, &Pose::REST).homography(&cam, canvas.size());
        let l = h.apply([0.0, 50.0]);
        let r = h.apply([200.0, 50.0]);
        // The right edge went away: it is nearer the centre than the left
        // edge, which came closer.
        assert!(r[0] - 100.0 < 100.0 - l[0], "{l:?} {r:?}");
        assert!(r[0] - 100.0 > 0.0);
    }

    #[test]
    fn unproject_inverts_project() {
        let canvas = Canvas::new(200, 100, 30.0);
        let mut cam = CameraDesc::default_for(&canvas);
        cam.position = [30.0, -20.0, -300.0];
        let p = [12.0, 34.0, 56.0];
        let (s, d) = cam.project(canvas.size(), p);
        let q = cam.unproject(canvas.size(), s, d);
        assert!((0..3).all(|i| (p[i] - q[i]).abs() < 1e-3), "{q:?}");
    }

    #[test]
    fn lights_and_shadows() {
        let m = MaterialDesc { specular: 0.0, ..MaterialDesc::default() };
        let white = [1.0, 1.0, 1.0, 1.0];
        let sun = LightDesc { kind: LightKind::Parallel, position: [0.0; 3], direction: [0.0, 0.0, 1.0], colour: [1.0; 3], cone: [0.0, 1.0], radius: 0.0, falloff: 0.0, casts_shadows: true, shadow_darkness: 1.0, sky: None, look: None };
        // Facing the light head on: full brightness; at 60°: half.
        let lit = shade(white, [0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, -100.0], &m, &[sun], 1.0, |_| 1.0);
        assert!((lit[0] - 1.0).abs() < 1e-5);
        let tilted = normalise([0.0, (60f32).to_radians().sin(), -(60f32).to_radians().cos()]);
        let half = shade(white, [0.0; 3], tilted, [0.0, 0.0, -100.0], &m, &[sun], 1.0, |_| 1.0);
        assert!((half[0] - 0.5).abs() < 1e-3, "{half:?}");
        // A card between the light and the point blocks it.
        let card = Plane { origin: [-50.0, -50.0, -10.0], eu: [1.0, 0.0, 0.0], ev: [0.0, 1.0, 0.0], back: [0.0, 0.0, 1.0] };
        let c = Caster::new(&card, [0.0, 0.0, 100.0, 100.0], 7).unwrap();
        assert_eq!(shadow([0.0; 3], &sun, &[c], 0, |_, _, _| 1.0), 0.0);
        assert_eq!(shadow([0.0; 3], &sun, &[c], 7, |_, _, _| 1.0), 1.0);
        assert_eq!(shadow([0.0, 0.0, -20.0], &sun, &[c], 0, |_, _, _| 1.0), 1.0);
    }
}
