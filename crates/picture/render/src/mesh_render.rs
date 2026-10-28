// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D models drawn into a picture: triangles seen through the scene's
//! camera, sorted by a depth buffer, lit physically (GGX highlights,
//! metals and rough surfaces) by the scene's lights with shadow maps.
//!
//! Neighbouring model layers are drawn in one pass (see
//! [`crate::resolve`]), so models in front hide models behind wherever
//! they overlap. The result is a full-canvas premultiplied picture that
//! composites like any other layer. Shading happens once per visible
//! pixel: the raster pass keeps the nearest triangle and where on it the
//! pixel landed, and the shading pass reads only those.
//!
//! A layer can ask for toon shading instead ([`Toon`]): cel shading in two
//! or three hard bands with a shadow colour of its own, a rim light, hard
//! highlights and ink outlines round its silhouette and where it passes in
//! front of itself (screen-space, drawn outside the nearer surface).
//!
//! The CPU version is the only one; the GPU compositor uploads its result.

use crate::frame::Frame;
use crate::scene::Canvas;
use crate::space::{CameraDesc, LightDesc, LightKind, NEAR, V3, add, cross, dot, length, look_at, normalise, scale, sub};
use crate::transform::{Placement, Pose};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;

/// Settings a model layer puts over its own materials.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MaterialOverride {
    /// Linear RGBA (straight).
    pub base_colour: Option<[f32; 4]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub emissive: Option<V3>,
    pub opacity: Option<f32>,
    pub subsurface: Option<f32>,
    /// How wet the surface is, 0 dry to 1 soaked: darker and richer in
    /// colour, glossy, reflecting what is round it, with puddles where it
    /// faces up.
    pub wetness: Option<f32>,
    /// Multiplies the glow of the materials (their own, or `emissive`
    /// over them): above 1 a glow is brighter than white, so it blooms
    /// (a neon tube 4-8, a lit window 2-4, a bulb 6-10).
    pub emissive_strength: Option<f32>,
    /// Toon (cel) shading with ink outlines instead of physical shading.
    pub toon: Option<Toon>,
}

/// Cel shading: the light in hard bands, a colour for the shadow, a rim of
/// light round the edge and ink lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Toon {
    /// 2 (light and shadow) or 3 (with a middle tone).
    pub bands: u8,
    /// Linear RGB the shadow side is multiplied by (a cool tint reads as
    /// animation; grey as print).
    pub shadow: V3,
    /// Where the light turns to shadow, 0–1 of the light falling on it.
    pub threshold: f32,
    /// Rim light strength (0 none) and colour.
    pub rim: f32,
    pub rim_colour: V3,
    /// Hard highlights on glossy surfaces (hair, eyes).
    pub highlights: bool,
    /// Ink line width in canvas px (0 none) and its linear RGB.
    pub outline: f32,
    pub outline_colour: V3,
}

impl Default for Toon {
    fn default() -> Toon {
        Toon { bands: 2, shadow: [0.55, 0.5, 0.68], threshold: 0.35, rim: 0.35, rim_colour: [1.0, 0.97, 0.92], highlights: true, outline: 2.0, outline_colour: [0.04, 0.03, 0.05] }
    }
}

impl MaterialOverride {
    /// The glow of a material under this override.
    pub fn glow_of(&self, own: V3) -> V3 {
        let e = self.emissive.unwrap_or(own);
        let k = self.emissive_strength.map_or(1.0, |k| k.clamp(0.0, 100.0));
        [e[0] * k, e[1] * k, e[2] * k]
    }
}

/// One model layer in a pass.
#[derive(Clone, Debug)]
pub struct MeshInstance {
    pub id: u64,
    pub mesh: Arc<Mesh>,
    /// What the mesh is (its file), for the layer cache.
    pub key: String,
    /// Model px → world, as the rows of a 3×4 matrix.
    pub world: [[f32; 4]; 3],
    pub overrides: MaterialOverride,
    /// 0 = faceted, 1 = the model's own smooth normals.
    pub smooth: f32,
    pub accepts_lights: bool,
    pub casts_shadows: bool,
    pub accepts_shadows: bool,
    /// Where the model is when the shutter closes (motion blur in path
    /// traced renders).
    pub motion: Option<Arc<Motion>>,
}

/// A model layer as it is a moment later: its posed mesh and placement
/// (the world matrix is filled in when the frame is resolved).
#[derive(Clone, Debug)]
pub struct Motion {
    pub mesh: Arc<Mesh>,
    pub key: String,
    pub placement: Placement,
    pub space: crate::space::Space,
    pub world: [[f32; 4]; 3],
}

impl PartialEq for MeshInstance {
    fn eq(&self, o: &MeshInstance) -> bool {
        self.id == o.id
            && self.key == o.key
            && self.world == o.world
            && self.overrides == o.overrides
            && self.smooth == o.smooth
            && (self.accepts_lights, self.casts_shadows, self.accepts_shadows) == (o.accepts_lights, o.casts_shadows, o.accepts_shadows)
            && (Arc::ptr_eq(&self.mesh, &o.mesh) || self.mesh.triangles() == o.mesh.triangles())
            && match (&self.motion, &o.motion) {
                (None, None) => true,
                (Some(a), Some(b)) => a.key == b.key && a.world == b.world,
                _ => false,
            }
    }
}

/// Models drawn together, with what they are seen through and lit by
/// (filled in when the frame is resolved).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshDesc {
    pub instances: Vec<MeshInstance>,
    pub camera: Option<CameraDesc>,
    pub lights: Vec<LightDesc>,
}

type MeshCache = Mutex<(HashMap<String, Arc<Mesh>>, Vec<String>)>;

fn cache() -> &'static MeshCache {
    static CACHE: OnceLock<MeshCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

/// Models in memory, by file. Files are named by their content, so one
/// read stays right; the oldest go first past a few dozen.
pub fn load(path: &str) -> Option<Arc<Mesh>> {
    if let Some(m) = cache().lock().unwrap_or_else(|e| e.into_inner()).0.get(path) {
        return Some(m.clone());
    }
    let mesh = Arc::new(studio_mesh::io::load(std::path::Path::new(path)).ok()?);
    remember(path, mesh.clone());
    Some(mesh)
}

/// Keeps a model under a name of its own (a model being sculpted, before
/// it is saved), so a layer can show it by that name.
pub fn remember(name: &str, mesh: Arc<Mesh>) {
    let mut g = cache().lock().unwrap_or_else(|e| e.into_inner());
    if g.0.insert(name.to_string(), mesh).is_none() {
        g.1.push(name.to_string());
    }
    if g.1.len() > 48 {
        let old = g.1.remove(0);
        g.0.remove(&old);
    }
}

/// The model → world matrix of a model layer: its anchor (x, y and z) to
/// the origin, scaled, turned (orientation then X, Y, Z) and moved to its
/// position and depth, with a motion pose folded in as for 3D layers.
pub fn model_matrix(canvas: [f32; 2], p: &Placement, s: &crate::space::Space, pose: &Pose) -> [[f32; 4]; 3] {
    let r = crate::space::rotation(s.orientation, s.rotation_x + pose.turn[0], s.rotation_y + pose.turn[1], p.rotation + pose.rotation);
    let k = [
        p.scale[0] * pose.scale[0] * if p.flip_x { -1.0 } else { 1.0 },
        p.scale[1] * pose.scale[1] * if p.flip_y { -1.0 } else { 1.0 },
        s.scale_z * (pose.scale[0] + pose.scale[1]) * 0.5,
    ];
    let pos = [p.position[0] + pose.offset[0] * canvas[0], p.position[1] + pose.offset[1] * canvas[1], s.z + pose.depth * canvas[0]];
    let anchor = [p.anchor[0], p.anchor[1], s.anchor_z];
    std::array::from_fn(|row| {
        let m = [r[row][0] * k[0], r[row][1] * k[1], r[row][2] * k[2]];
        [m[0], m[1], m[2], pos[row] - (m[0] * anchor[0] + m[1] * anchor[1] + m[2] * anchor[2])]
    })
}

fn xform(m: &[[f32; 4]; 3], p: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

/// Normals go through the inverse transpose (non-uniform scales).
fn normal_matrix(m: &[[f32; 4]; 3]) -> [V3; 3] {
    let a: [V3; 3] = std::array::from_fn(|r| [m[r][0], m[r][1], m[r][2]]);
    let c0 = [a[0][0], a[1][0], a[2][0]];
    let c1 = [a[0][1], a[1][1], a[2][1]];
    let c2 = [a[0][2], a[1][2], a[2][2]];
    // Rows of the inverse transpose are the cofactor columns.
    let n0 = cross(c1, c2);
    let n1 = cross(c2, c0);
    let n2 = cross(c0, c1);
    [[n0[0], n1[0], n2[0]], [n0[1], n1[1], n2[1]], [n0[2], n1[2], n2[2]]]
}

fn mul3(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// The bounding box of a pass in the world.
pub fn world_bounds(desc: &MeshDesc) -> Option<(V3, V3)> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for inst in &desc.instances {
        let (a, b) = inst.mesh.bounds();
        for k in 0..8 {
            let c = [if k & 1 == 0 { a[0] } else { b[0] }, if k & 2 == 0 { a[1] } else { b[1] }, if k & 4 == 0 { a[2] } else { b[2] }];
            let w = xform(&inst.world, c);
            for i in 0..3 {
                lo[i] = lo[i].min(w[i]);
                hi[i] = hi[i].max(w[i]);
            }
        }
    }
    lo[0].is_finite().then_some((lo, hi))
}

/// Where each model covers the canvas (canvas px: left, top, right,
/// bottom), from its box seen through the camera; for picking.
pub fn screen_rects(desc: &MeshDesc, canvas: &Canvas) -> Vec<(u64, [f32; 4])> {
    let camera = desc.camera.unwrap_or_else(|| CameraDesc::default_for(canvas));
    desc.instances
        .iter()
        .filter_map(|inst| {
            let (a, b) = inst.mesh.bounds();
            let mut r = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
            for k in 0..8 {
                let c = [if k & 1 == 0 { a[0] } else { b[0] }, if k & 2 == 0 { a[1] } else { b[1] }, if k & 4 == 0 { a[2] } else { b[2] }];
                let (s, d) = camera.project(canvas.size(), xform(&inst.world, c));
                if d < NEAR {
                    // Partly behind the lens: it may cover anything.
                    return Some((inst.id, [0.0, 0.0, canvas.width as f32, canvas.height as f32]));
                }
                r = [r[0].min(s[0]), r[1].min(s[1]), r[2].max(s[0]), r[3].max(s[1])];
            }
            Some((inst.id, r))
        })
        .collect()
}

/// A material with a layer's overrides applied.
#[derive(Clone)]
struct Surface {
    base: [f32; 4],
    metallic: f32,
    roughness: f32,
    emissive: V3,
    subsurface: f32,
    wet: f32,
    texture: Option<Arc<studio_mesh::Texture>>,
}

/// The parts of a [`Surface`] the shading reads, after wetness.
#[derive(Clone, Copy)]
struct Mat {
    metallic: f32,
    roughness: f32,
    emissive: V3,
    subsurface: f32,
}

/// One instance moved into the world.
struct Placed {
    world_pos: Vec<V3>,
    world_n: Vec<V3>,
    surfaces: Vec<Surface>,
    tri_surface: Vec<u32>,
    smooth: f32,
    lit: bool,
    accepts_shadows: bool,
    toon: Option<Toon>,
}

/// A triangle in a target's pixels, with what it was cut from.
#[derive(Clone, Copy)]
struct RasterTri {
    inst: u32,
    tri: u32,
    xy: [[f32; 2]; 3],
    /// Depth of each corner (distance in front of the lens, or along the
    /// light for an orthographic map).
    z: [f32; 3],
    /// Each corner's barycentric place on the original triangle (corners
    /// move when the near plane cuts it).
    bary: [V3; 3],
}

/// A view to rasterise through.
#[derive(Clone, Copy)]
struct View {
    position: V3,
    axes: [V3; 3],
    /// Pixels per unit of x/z (perspective) or per world pixel
    /// (orthographic).
    zoom: f32,
    centre: [f32; 2],
    ortho: bool,
}

impl View {
    fn local(&self, p: V3) -> V3 {
        let d = sub(p, self.position);
        [dot(self.axes[0], d), dot(self.axes[1], d), dot(self.axes[2], d)]
    }
    fn screen(&self, v: V3) -> [f32; 2] {
        if self.ortho {
            [self.centre[0] + self.zoom * v[0], self.centre[1] + self.zoom * v[1]]
        } else {
            let w = v[2].max(1e-6);
            [self.centre[0] + self.zoom * v[0] / w, self.centre[1] + self.zoom * v[1] / w]
        }
    }
}

/// Triangles of every instance seen through `view`, cut at the near plane.
fn raster_tris(placed: &[Placed], meshes: &[&Mesh], view: &View, only: impl Fn(usize) -> bool) -> Vec<RasterTri> {
    let mut out = Vec::new();
    let unit: [V3; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for (i, (pl, mesh)) in placed.iter().zip(meshes).enumerate() {
        if !only(i) {
            continue;
        }
        let local: Vec<V3> = pl.world_pos.iter().map(|p| view.local(*p)).collect();
        for t in 0..mesh.triangles() {
            let c = mesh.tri(t);
            let v = c.map(|k| local[k]);
            let near = if view.ortho { f32::NEG_INFINITY } else { NEAR };
            let inside = v.map(|p| p[2] >= near);
            let n_in = inside.iter().filter(|b| **b).count();
            if n_in == 0 {
                continue;
            }
            if n_in == 3 {
                out.push(RasterTri { inst: i as u32, tri: t as u32, xy: v.map(|p| view.screen(p)), z: v.map(|p| p[2]), bary: unit });
                continue;
            }
            // Sutherland–Hodgman against z = near.
            let mut poly: Vec<(V3, V3)> = Vec::with_capacity(4);
            for k in 0..3 {
                let j = (k + 1) % 3;
                let (a, b) = ((v[k], unit[k]), (v[j], unit[j]));
                if inside[k] {
                    poly.push(a);
                }
                if inside[k] != inside[j] {
                    let s = (near - a.0[2]) / (b.0[2] - a.0[2]);
                    poly.push((crate::space::add(a.0, scale(sub(b.0, a.0), s)), add(a.1, scale(sub(b.1, a.1), s))));
                }
            }
            for k in 1..poly.len().saturating_sub(1) {
                let q = [poly[0], poly[k], poly[k + 1]];
                out.push(RasterTri { inst: i as u32, tri: t as u32, xy: q.map(|p| view.screen(p.0)), z: q.map(|p| p.0[2]), bary: q.map(|p| p.1) });
            }
        }
    }
    out
}

/// What the raster pass leaves at each pixel.
#[derive(Clone, Copy)]
struct Hit {
    depth: f32,
    tri: u32,
    /// Barycentrics on the raster triangle (perspective-correct; the third
    /// is 1 − the other two).
    b: [f32; 2],
}

const EMPTY: Hit = Hit { depth: f32::INFINITY, tri: u32::MAX, b: [0.0; 2] };

/// Nearest triangle per pixel, in parallel bands of rows.
fn rasterise(tris: &[RasterTri], width: usize, height: usize, perspective: bool) -> Vec<Hit> {
    let mut hits = vec![EMPTY; width * height];
    if width == 0 || height == 0 {
        return hits;
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let band = height.div_ceil(threads * 4).max(8);
    let bands = height.div_ceil(band);
    // Which triangles touch which band.
    let mut lists: Vec<Vec<u32>> = vec![Vec::new(); bands];
    for (i, t) in tris.iter().enumerate() {
        let y0 = t.xy.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let y1 = t.xy.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
        if !(y1 >= 0.0 && y0 < height as f32) {
            continue;
        }
        let a = (y0.max(0.0) as usize / band).min(bands - 1);
        let b = (y1.min(height as f32 - 1.0).max(0.0) as usize / band).min(bands - 1);
        for l in &mut lists[a..=b] {
            l.push(i as u32);
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let chunks: Vec<Mutex<&mut [Hit]>> = hits.chunks_mut(band * width).map(Mutex::new).collect();
    std::thread::scope(|s| {
        for _ in 0..threads.min(bands) {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if k >= bands {
                        break;
                    }
                    let mut rows = chunks[k].lock().unwrap_or_else(|e| e.into_inner());
                    let y_start = k * band;
                    let y_end = (y_start + band).min(height);
                    for &ti in &lists[k] {
                        raster_one(&tris[ti as usize], ti, &mut rows, width, y_start, y_end, perspective);
                    }
                }
            });
        }
    });
    drop(chunks);
    hits
}

fn raster_one(t: &RasterTri, id: u32, rows: &mut [Hit], width: usize, y_start: usize, y_end: usize, perspective: bool) {
    let [p0, p1, p2] = t.xy;
    let area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p1[1] - p0[1]) * (p2[0] - p0[0]);
    if area.abs() < 1e-12 || !area.is_finite() {
        return;
    }
    let inv = 1.0 / area;
    let x0 = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as usize;
    let x1 = (p0[0].max(p1[0]).max(p2[0]).ceil().max(0.0) as usize).min(width);
    let ya = (p0[1].min(p1[1]).min(p2[1]).floor().max(y_start as f32) as usize).max(y_start);
    let yb = (p0[1].max(p1[1]).max(p2[1]).ceil().max(0.0) as usize).min(y_end);
    if x0 >= x1 || ya >= yb {
        return;
    }
    let iz = if perspective { t.z.map(|z| 1.0 / z.max(1e-6)) } else { t.z };
    for y in ya..yb {
        let py = y as f32 + 0.5;
        let row = &mut rows[(y - y_start) * width..(y - y_start + 1) * width];
        for (x, hit) in row.iter_mut().enumerate().take(x1).skip(x0) {
            let px = x as f32 + 0.5;
            // Barycentrics from the edge functions; both windings fill.
            let w0 = ((p1[0] - px) * (p2[1] - py) - (p1[1] - py) * (p2[0] - px)) * inv;
            let w1 = ((p2[0] - px) * (p0[1] - py) - (p2[1] - py) * (p0[0] - px)) * inv;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let (depth, b) = if perspective {
                let s = w0 * iz[0] + w1 * iz[1] + w2 * iz[2];
                (1.0 / s, [w0 * iz[0] / s, w1 * iz[1] / s])
            } else {
                (w0 * iz[0] + w1 * iz[1] + w2 * iz[2], [w0, w1])
            };
            if depth < hit.depth {
                *hit = Hit { depth, tri: id, b };
            }
        }
    }
}

/// A light's depth map.
struct ShadowMap {
    view: View,
    size: usize,
    depth: Vec<f32>,
    /// World size of one texel at unit distance (perspective) or anywhere
    /// (orthographic), for the bias.
    texel: f32,
}

impl ShadowMap {
    /// Fraction of light reaching `p` (1 = lit), softened over 3×3 texels.
    fn lit(&self, p: V3, n: V3, to_light: V3) -> f32 {
        let v = self.view.local(p);
        if !self.view.ortho && v[2] <= NEAR {
            return 1.0;
        }
        let s = self.view.screen(v);
        let slope = (1.0 - dot(n, to_light).clamp(0.0, 1.0)).sqrt() * 2.0 + 1.0;
        let texel = if self.view.ortho { self.texel } else { self.texel * v[2] };
        let bias = texel * 1.5 * slope + 0.5;
        let mut lit = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let x = (s[0] - 0.5).round() as i64 + dx;
                let y = (s[1] - 0.5).round() as i64 + dy;
                if x < 0 || y < 0 || x >= self.size as i64 || y >= self.size as i64 {
                    lit += 1.0;
                    continue;
                }
                let d = self.depth[y as usize * self.size + x as usize];
                if v[2] - bias <= d {
                    lit += 1.0;
                }
            }
        }
        lit / 9.0
    }
}

fn shadow_map(light: &LightDesc, placed: &[Placed], meshes: &[&Mesh], casters: &[bool], bounds: (V3, V3)) -> Option<ShadowMap> {
    let centre = scale(add(bounds.0, bounds.1), 0.5);
    let radius = (length(sub(bounds.1, bounds.0)) * 0.5).max(1.0);
    let size = 2048usize;
    let view = match light.kind {
        LightKind::Ambient => return None,
        LightKind::Parallel => {
            let dir = normalise(light.direction);
            let from = sub(centre, scale(dir, radius * 2.0));
            let axes = look_at(from, centre, 0.0);
            let zoom = size as f32 * 0.5 / radius;
            View { position: from, axes, zoom, centre: [size as f32 * 0.5; 2], ortho: true }
        }
        LightKind::Spot | LightKind::Point => {
            let dist = length(sub(centre, light.position));
            if dist <= radius * 0.5 {
                // A light inside the scene would need a cube of maps.
                return None;
            }
            let aim = if light.kind == LightKind::Spot { add(light.position, light.direction) } else { centre };
            let axes = look_at(light.position, aim, 0.0);
            let mut half = (radius / dist).clamp(-1.0, 1.0).asin() * 1.15;
            if light.kind == LightKind::Spot {
                half = half.max(light.cone[0].clamp(-1.0, 1.0).acos());
            }
            let half = half.clamp(0.05, 80f32.to_radians());
            let zoom = size as f32 * 0.5 / half.tan();
            View { position: light.position, axes, zoom, centre: [size as f32 * 0.5; 2], ortho: false }
        }
    };
    let tris = raster_tris(placed, meshes, &view, |i| casters[i]);
    if tris.is_empty() {
        return None;
    }
    let hits = rasterise(&tris, size, size, !view.ortho);
    let texel = 1.0 / view.zoom;
    Some(ShadowMap { view, size, depth: hits.into_iter().map(|h| h.depth).collect(), texel })
}

/// The lights a scene is shaded with: its own, or, when none of them
/// gives any light (no lights at all, or only the one that carries the
/// scene's air and look, which lights nothing), the stock rig with them.
pub fn scene_lights(own: &[LightDesc]) -> Vec<LightDesc> {
    let lights_something = |l: &LightDesc| l.sky.is_some() || l.colour.iter().any(|c| *c > 1e-4);
    if own.iter().any(lights_something) {
        return own.to_vec();
    }
    let mut v = default_lights();
    v.extend(own.iter().cloned());
    v
}

/// Lights for a scene that has none: a key from the upper left, a soft
/// fill from the right, a rim from behind and some ambient.
pub fn default_lights() -> Vec<LightDesc> {
    let l = |kind, direction: V3, c: f32, shadows| LightDesc {
        kind,
        position: [0.0; 3],
        direction: normalise(direction),
        colour: [c; 3],
        cone: [0.0, 1.0],
        radius: 0.0,
        falloff: 0.0,
        casts_shadows: shadows,
        shadow_darkness: 0.85,
        sky: None,
        look: None,
    };
    vec![
        l(LightKind::Parallel, [0.45, 0.6, 0.65], 1.0, true),
        l(LightKind::Parallel, [-0.7, 0.15, 0.55], 0.32, false),
        l(LightKind::Parallel, [0.1, 0.45, -0.9], 0.45, false),
        l(LightKind::Ambient, [0.0, 0.0, 1.0], 0.22, false),
    ]
}

/// Physically based shading of one point (linear, straight RGB out).
#[allow(clippy::too_many_arguments)]
fn shade(s: &Mat, base: [f32; 3], p: V3, n: V3, eye: V3, lights: &[LightDesc], env: Option<(&crate::sky::SkyDesc, &crate::sky::Irradiance)>, shadow: &mut dyn FnMut(usize, V3) -> f32) -> V3 {
    let v = normalise(sub(eye, p));
    let n = if dot(n, v) < 0.0 { scale(n, -1.0) } else { n };
    let ndv = dot(n, v).max(1e-4);
    let metallic = s.metallic.clamp(0.0, 1.0);
    let rough = s.roughness.clamp(0.045, 1.0);
    let a2 = (rough * rough) * (rough * rough);
    let f0: V3 = std::array::from_fn(|c| 0.04 + (base[c] - 0.04) * metallic);
    let diffuse: V3 = std::array::from_fn(|c| base[c] * (1.0 - metallic));
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    let mut out = [0.0f32; 3];
    let mut ambient = [0.0f32; 3];
    for (i, l) in lights.iter().enumerate() {
        if l.kind == LightKind::Ambient {
            ambient = add(ambient, l.colour);
            continue;
        }
        let (to, mut atten) = if l.kind == LightKind::Parallel {
            (scale(normalise(l.direction), -1.0), 1.0)
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
        // Light under the surface wraps past where the surface turns away
        // and comes out warmer (Penner-style wrap with a red shift).
        let wrap = 0.6 * s.subsurface;
        let wrapped = ((ndl + wrap) / (1.0 + wrap)).max(0.0);
        if wrapped <= 0.0 || atten <= 0.0 {
            continue;
        }
        let mut shade_k = 1.0;
        if l.casts_shadows {
            let lit = shadow(i, to);
            // Shadows on such a surface are softer and warmer at the edge.
            shade_k = lit + (1.0 - lit) * 0.2 * s.subsurface;
            if shade_k <= 0.0 {
                continue;
            }
        }
        if ndl <= 0.0 || s.subsurface > 0.0 {
            let scatter = (wrapped - ndl.max(0.0)).max(0.0);
            let tint = [1.0, 0.45, 0.3];
            for c in 0..3 {
                out[c] += diffuse[c] * l.colour[c] * atten * shade_k * (ndl.max(0.0) * (1.0 - s.subsurface * 0.15) + scatter * tint[c]);
            }
            if ndl <= 0.0 {
                continue;
            }
        }
        atten *= shade_k;
        let h = normalise(add(to, v));
        let ndh = dot(n, h).max(0.0);
        let vdh = dot(v, h).max(0.0);
        let dd = ndh * ndh * (a2 - 1.0) + 1.0;
        let d = a2 / (std::f32::consts::PI * dd * dd);
        let g = (ndv / (ndv * (1.0 - k) + k)) * (ndl / (ndl * (1.0 - k) + k));
        let fw = (1.0 - vdh).powi(5);
        for c in 0..3 {
            let f = f0[c] + (1.0 - f0[c]) * fw;
            // Lambert at full strength head-on, as the 3D layers are lit;
            // the highlight scaled to match.
            let spec = std::f32::consts::PI * d * g * f / (4.0 * ndv * ndl + 1e-4);
            let d = if s.subsurface > 0.0 { 0.0 } else { (1.0 - f) * diffuse[c] };
            out[c] += (d + spec) * l.colour[c] * atten * ndl;
        }
    }
    // Ambient: brighter from above, with a rough reflection for metals.
    let sky = 0.75 + 0.25 * -n[1];
    let fr = (1.0 - ndv).powi(5);
    // A sky lights from everywhere: its irradiance on the diffuse part,
    // its colour in the reflection direction on the glossy part.
    let (env_diffuse, env_spec) = match env {
        Some((sk, irr)) => {
            let r = sub(scale(n, 2.0 * dot(n, v)), v);
            (irr.at(n), crate::sky::radiance(sk, r))
        }
        None => ([0.0; 3], [0.0; 3]),
    };
    for c in 0..3 {
        let env_f = f0[c] + (((1.0 - rough).max(f0[c])) - f0[c]) * fr;
        out[c] += ambient[c] * sky * (diffuse[c] + env_f * (0.35 + 0.65 * (1.0 - rough)));
        out[c] += env_diffuse[c] * diffuse[c] + env_spec[c] * env_f * (1.0 - rough * 0.7);
        out[c] += s.emissive[c];
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Toon shading: how much light falls on the point (by its lights and
/// shadows) cut into hard bands, the shadow side in the toon's shadow
/// colour, a hard highlight and a rim of light.
fn shade_toon(t: &Toon, s: &Mat, base: [f32; 3], p: V3, n: V3, eye: V3, lights: &[LightDesc], shadow: &mut dyn FnMut(usize, V3) -> f32) -> V3 {
    let v = normalise(sub(eye, p));
    let n = if dot(n, v) < 0.0 { scale(n, -1.0) } else { n };
    let mut light = 0.0f32;
    let mut colour = [0.0f32; 3];
    let mut weight = 0.0f32;
    let mut ambient = 0.0f32;
    let mut glint = 0.0f32;
    for (i, l) in lights.iter().enumerate() {
        let lum = (0.2126 * l.colour[0] + 0.7152 * l.colour[1] + 0.0722 * l.colour[2]).max(0.0);
        if l.kind == LightKind::Ambient {
            ambient += lum;
            continue;
        }
        let (to, mut atten) = if l.kind == LightKind::Parallel {
            (scale(normalise(l.direction), -1.0), 1.0)
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
        let lit = if l.casts_shadows { shadow(i, to) } else { 1.0 };
        let k = ndl * atten * lit * lum;
        light += k;
        if lum > 0.0 {
            colour = add(colour, scale(l.colour, atten / lum));
            weight += atten;
        }
        let h = normalise(add(to, v));
        let shine = (2.0 / (s.roughness.clamp(0.05, 1.0).powi(4)) - 2.0).clamp(4.0, 400.0);
        glint = glint.max(dot(n, h).max(0.0).powf(shine) * lit * atten);
    }
    let tint: V3 = if weight > 0.0 { scale(colour, 1.0 / weight) } else { [1.0; 3] };
    // The light against what lights it fully, in hard steps (a narrow
    // smoothstep so the edge is clean, not stair-stepped).
    let total = (light / (light + ambient * 0.5).max(1e-4) * (light + ambient).min(1.5)).clamp(0.0, 1.5);
    let e = 0.02;
    let k = match t.bands {
        0..=2 => smoothstep(t.threshold - e, t.threshold + e, total),
        _ => 0.5 * smoothstep(t.threshold * 0.5 - e, t.threshold * 0.5 + e, total) + 0.5 * smoothstep(t.threshold * 1.6 - e, t.threshold * 1.6 + e, total),
    };
    let bright = (light + ambient).clamp(0.6, 1.2);
    let mut out: V3 = std::array::from_fn(|c| {
        let lit = base[c] * tint[c] * bright;
        let dark = base[c] * t.shadow[c] * bright.min(1.0);
        dark + (lit - dark) * k
    });
    if t.highlights && s.roughness < 0.65 {
        let g = smoothstep(0.55, 0.6, glint) * (1.0 - s.roughness) * 0.6;
        out = add(out, [g; 3]);
    }
    if t.rim > 0.0 {
        let fres = 1.0 - dot(n, v).clamp(0.0, 1.0);
        let r = smoothstep(0.62, 0.66, fres) * t.rim * (0.4 + 0.6 * k);
        out = add(out, scale(t.rim_colour, r));
    }
    add(out, s.emissive)
}

/// Ink lines: wherever a toon surface with an outline is in front of
/// something farther (or of nothing), the farther pixels within the line's
/// width take the ink, so the line sits just outside the nearer surface.
fn draw_outlines(hits: &[Hit], tris: &[RasterTri], placed: &[Placed], out: &mut [[f32; 4]], w: usize, h: usize, px_scale: f32) {
    if !placed.iter().any(|p| p.toon.is_some_and(|t| t.outline > 0.0)) {
        return;
    }
    let toon_at = |i: usize| -> Option<Toon> {
        let hit = hits[i];
        if hit.tri == u32::MAX {
            return None;
        }
        placed[tris[hit.tri as usize].inst as usize].toon.filter(|t| t.outline > 0.0)
    };
    let farther = |a: Hit, b: Hit| b.tri == u32::MAX || b.depth > a.depth * 1.004 + 1e-3;
    // The near side of every edge: a toon pixel next to something farther.
    let mut stamps: Vec<(usize, Toon)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let Some(t) = toon_at(i) else { continue };
            let a = hits[i];
            let edge = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && farther(a, hits[ny as usize * w + nx as usize])
            });
            if edge {
                stamps.push((i, t));
            }
        }
    }
    let mut inked = vec![false; w * h];
    for (i, t) in stamps {
        let r = (t.outline * px_scale * 0.5).max(0.5);
        let ri = r.ceil() as i32;
        let (x0, y0) = ((i % w) as i32, (i / w) as i32);
        let a = hits[i];
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                if (dx * dx + dy * dy) as f32 > r * r {
                    continue;
                }
                let (x, y) = (x0 + dx, y0 + dy);
                if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                    continue;
                }
                let j = y as usize * w + x as usize;
                if inked[j] || !(j == i || farther(a, hits[j])) {
                    continue;
                }
                inked[j] = true;
                let c = t.outline_colour;
                out[j] = [c[0], c[1], c[2], 1.0];
            }
        }
    }
}

fn surfaces(mesh: &Mesh, o: &MaterialOverride) -> Vec<Surface> {
    let mut list: Vec<Surface> = mesh
        .materials
        .iter()
        .map(|m| Surface {
            base: o.base_colour.unwrap_or(m.base_colour),
            metallic: o.metallic.unwrap_or(m.metallic),
            roughness: o.roughness.unwrap_or(m.roughness),
            emissive: o.glow_of(m.emissive),
            subsurface: o.subsurface.unwrap_or(m.subsurface).clamp(0.0, 1.0),
            wet: o.wetness.unwrap_or(0.0).clamp(0.0, 1.0),
            texture: m.texture.clone().map(Arc::new),
        })
        .collect();
    if list.is_empty() {
        let d = studio_mesh::Material::default();
        list.push(Surface { base: o.base_colour.unwrap_or(d.base_colour), metallic: o.metallic.unwrap_or(0.0), roughness: o.roughness.unwrap_or(0.5), emissive: o.glow_of([0.0; 3]), subsurface: o.subsurface.unwrap_or(0.0), wet: o.wetness.unwrap_or(0.0).clamp(0.0, 1.0), texture: None });
    }
    if let Some(a) = o.opacity {
        for s in &mut list {
            s.base[3] *= a.clamp(0.0, 1.0);
        }
    }
    list
}

/// Draws the models at `target` pixels (the canvas at that scale).
pub fn render(desc: &MeshDesc, canvas: &Canvas, target: (u32, u32)) -> Frame {
    render_with(desc, canvas, target, true)
}

/// [`render`], choosing whether the air and look pass may use the GPU
/// (it falls back to the CPU without one).
pub fn render_with(desc: &MeshDesc, canvas: &Canvas, target: (u32, u32), gpu: bool) -> Frame {
    let (tw, th) = (target.0.max(1), target.1.max(1));
    if desc.instances.is_empty() {
        return Frame::new(tw, th);
    }
    // Final renders with bouncing light, when the camera asks for them (a
    // toon look is drawn here: bands and ink, not bounced light).
    if let Some(settings) = desc.camera.and_then(|c| c.trace).filter(|_| !desc.instances.iter().any(|i| i.overrides.toon.is_some())) {
        let passes = crate::trace::render(desc, canvas, (tw, th), &settings);
        let mut frame = passes.beauty;
        if desc.lights.iter().any(|l| l.look.is_some()) {
            let camera = desc.camera.unwrap_or_else(|| CameraDesc::default_for(canvas));
            let lens = crate::atmosphere::Lens { position: camera.position, axes: camera.axes, zoom: camera.zoom * tw as f32 / canvas.width.max(1) as f32, centre: [tw as f32 * 0.5, th as f32 * 0.5] };
            // The tracer's depth is the distance from the lens; the pass reads depth along the axis.
            let depth: Vec<f32> = passes
                .depth
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let u = ((i % tw as usize) as f32 + 0.5 - lens.centre[0]) / lens.zoom;
                    let v = ((i / tw as usize) as f32 + 0.5 - lens.centre[1]) / lens.zoom;
                    d / (1.0 + u * u + v * v).sqrt()
                })
                .collect();
            let pic = crate::atmosphere::Picture { width: tw as usize, height: th as usize, depth: &depth, surface: &[] };
            if let Some(plan) = crate::atmosphere::plan(&desc.lights, &lens, &pic, &[], canvas.height as f32) {
                crate::atmosphere::run(&plan, &pic, &mut frame.pixels, gpu);
            }
        }
        return frame;
    }
    // Two samples a side, unless the target is already huge.
    let ss = if (tw as u64 * th as u64) <= 4_200_000 { 2 } else { 1 };
    let (w, h) = (tw as usize * ss, th as usize * ss);
    let k = tw as f32 / canvas.width.max(1) as f32 * ss as f32;
    let camera = desc.camera.unwrap_or_else(|| CameraDesc::default_for(canvas));
    let lights = scene_lights(&desc.lights);
    let sky = lights.iter().find_map(|l| l.sky);
    let irradiance = sky.map(|sk| crate::sky::Irradiance::of(&sk));
    let env = sky.as_ref().zip(irradiance.as_ref());
    let meshes: Vec<&Mesh> = desc.instances.iter().map(|i| i.mesh.as_ref()).collect();
    let placed: Vec<Placed> = desc
        .instances
        .iter()
        .map(|inst| {
            let nm = normal_matrix(&inst.world);
            Placed {
                world_pos: inst.mesh.positions.iter().map(|p| xform(&inst.world, *p)).collect(),
                world_n: inst.mesh.normals.iter().map(|n| normalise(mul3(&nm, *n))).collect(),
                surfaces: surfaces(&inst.mesh, &inst.overrides),
                tri_surface: inst.mesh.tri_materials(),
                smooth: inst.smooth.clamp(0.0, 1.0),
                lit: inst.accepts_lights,
                accepts_shadows: inst.accepts_shadows,
                toon: inst.overrides.toon,
            }
        })
        .collect();
    let view = View { position: camera.position, axes: camera.axes, zoom: camera.zoom * k, centre: [w as f32 * 0.5, h as f32 * 0.5], ortho: false };
    let tris = raster_tris(&placed, &meshes, &view, |_| true);
    let hits = rasterise(&tris, w, h, true);
    let casters: Vec<bool> = desc.instances.iter().map(|i| i.casts_shadows).collect();
    let bounds = world_bounds(desc).unwrap_or(([0.0; 3], [0.0; 3]));
    let maps: Vec<Option<ShadowMap>> = lights
        .iter()
        .map(|l| if l.casts_shadows && casters.iter().any(|c| *c) { shadow_map(l, &placed, &meshes, &casters, bounds) } else { None })
        .collect();
    // Shade the visible pixels, band by band.
    let mut big = vec![[0.0f32; 4]; w * h];
    // Normal and wet reflectivity of each pixel's surface, for the air and look pass.
    let mut sbig = vec![[0.0f32; 4]; w * h];
    let metre = lights.iter().find_map(|l| l.look.and_then(|k| k.atmos)).map(|a| a.metre).filter(|m| *m > 0.0).unwrap_or(crate::atmosphere::default_metre(canvas.height as f32));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let rows_per = h.div_ceil(threads * 4).max(4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let chunks: Vec<Mutex<(&mut [[f32; 4]], &mut [[f32; 4]])>> = big.chunks_mut(rows_per * w).zip(sbig.chunks_mut(rows_per * w)).map(Mutex::new).collect();
    let n_chunks = chunks.len();
    std::thread::scope(|s| {
        for _ in 0..threads.min(n_chunks) {
            s.spawn(|| {
                loop {
                    let c = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if c >= n_chunks {
                        break;
                    }
                    let mut guard = chunks[c].lock().unwrap_or_else(|e| e.into_inner());
                    let (out, sout) = &mut *guard;
                    let start = c * rows_per * w;
                    for (i, px) in out.iter_mut().enumerate() {
                        let hit = hits[start + i];
                        if hit.tri == u32::MAX {
                            // The sky behind, when it shows.
                            if let Some(sk) = sky.filter(|sk| sk.background) {
                                let gi = start + i;
                                let d = pixel_dir(&view, (gi % w) as f32 + 0.5, (gi / w) as f32 + 0.5);
                                let mut c = crate::sky::radiance(&sk, d);
                                if dot(d, scale(sk.sun, -1.0)) > crate::sky::SUN_COS {
                                    c = add(c, scale(crate::sky::sun_colour(&sk), 40.0));
                                }
                                *px = [c[0], c[1], c[2], 1.0];
                            }
                            continue;
                        }
                        let rt = &tris[hit.tri as usize];
                        let pl = &placed[rt.inst as usize];
                        let mesh = meshes[rt.inst as usize];
                        let b3 = [hit.b[0], hit.b[1], 1.0 - hit.b[0] - hit.b[1]];
                        let bary: V3 = std::array::from_fn(|j| b3[0] * rt.bary[0][j] + b3[1] * rt.bary[1][j] + b3[2] * rt.bary[2][j]);
                        let corners = mesh.tri(rt.tri as usize);
                        let mix = |f: &dyn Fn(usize) -> V3| add(add(scale(f(corners[0]), bary[0]), scale(f(corners[1]), bary[1])), scale(f(corners[2]), bary[2]));
                        let p = mix(&|v| pl.world_pos[v]);
                        let face = normalise(cross(sub(pl.world_pos[corners[1]], pl.world_pos[corners[0]]), sub(pl.world_pos[corners[2]], pl.world_pos[corners[0]])));
                        let smooth_n = normalise(mix(&|v| pl.world_n.get(v).copied().unwrap_or(face)));
                        let n = if smooth_n == [0.0; 3] { face } else { normalise(add(scale(face, 1.0 - pl.smooth), scale(smooth_n, pl.smooth))) };
                        let surf = &pl.surfaces[(pl.tri_surface.get(rt.tri as usize).copied().unwrap_or(0) as usize).min(pl.surfaces.len() - 1)];
                        let mut base = surf.base;
                        if let Some(t) = &surf.texture {
                            let uv0 = mesh.uvs.get(corners[0]).copied().unwrap_or([0.0; 2]);
                            let uv1 = mesh.uvs.get(corners[1]).copied().unwrap_or([0.0; 2]);
                            let uv2 = mesh.uvs.get(corners[2]).copied().unwrap_or([0.0; 2]);
                            let uv = [uv0[0] * bary[0] + uv1[0] * bary[1] + uv2[0] * bary[2], uv0[1] * bary[0] + uv1[1] * bary[1] + uv2[1] * bary[2]];
                            let c = t.sample(uv);
                            base = [base[0] * c[0], base[1] * c[1], base[2] * c[2], base[3] * c[3]];
                        }
                        let mut mat = Mat { metallic: surf.metallic, roughness: surf.roughness, emissive: surf.emissive, subsurface: surf.subsurface };
                        let mut base = base;
                        if surf.wet > 0.0 {
                            let nv = if dot(n, normalise(sub(camera.position, p))) < 0.0 { scale(n, -1.0) } else { n };
                            let (darken, puddle, refl) = wetness(surf.wet, p, -nv[1], metre);
                            let rich = 1.0 + 0.3 * surf.wet;
                            let dk = darken.powf(1.0 - surf.metallic.clamp(0.0, 1.0));
                            for c in 0..3 {
                                base[c] = base[c].max(0.0).powf(rich) * dk;
                            }
                            mat.roughness = mix_f(mat.roughness, 0.06, 0.8 * surf.wet);
                            mat.roughness = mix_f(mat.roughness, 0.02, puddle);
                            sout[i] = [nv[0], nv[1], nv[2], refl];
                        }
                        let rgb = if pl.lit {
                            let mut sh = |li: usize, to: V3| match (&maps[li], pl.accepts_shadows) {
                                (Some(m), true) => 1.0 - lights[li].shadow_darkness * (1.0 - m.lit(p, n, to)),
                                _ => 1.0,
                            };
                            match &pl.toon {
                                Some(t) => shade_toon(t, &mat, [base[0], base[1], base[2]], p, n, camera.position, &lights, &mut sh),
                                None => shade(&mat, [base[0], base[1], base[2]], p, n, camera.position, &lights, env, &mut sh),
                            }
                        } else {
                            [base[0] + mat.emissive[0], base[1] + mat.emissive[1], base[2] + mat.emissive[2]]
                        };
                        // Distance fades into the haze.
                        let rgb = match sky.filter(|sk| sk.haze > 0.0) {
                            Some(sk) => {
                                let to = sub(p, camera.position);
                                let (hc, k) = crate::sky::haze(&sk, to, length(to));
                                std::array::from_fn(|c| rgb[c] * (1.0 - k) + hc[c] * k)
                            }
                            None => rgb,
                        };
                        let a = base[3].clamp(0.0, 1.0);
                        *px = [rgb[0] * a, rgb[1] * a, rgb[2] * a, a];
                    }
                }
            });
        }
    });
    drop(chunks);
    // Ink lines round toon models.
    draw_outlines(&hits, &tris, &placed, &mut big, w, h, k);
    // Smoke and fire in front of what was drawn.
    if desc.instances.iter().any(|i| i.mesh.volume.is_some()) {
        let depth: Vec<f32> = hits.iter().map(|h| h.depth).collect();
        draw_volumes(desc, &view, &lights, sky.as_ref(), irradiance.as_ref(), &depth, &mut big, w, h);
    }
    let depth_big: Vec<f32> = hits.iter().map(|h| h.depth).collect();
    let (mut out, depth, surf) = if ss == 1 {
        (Frame { width: tw, height: th, pixels: big }, depth_big, sbig)
    } else {
        let mut out = Frame::new(tw, th);
        let mut depth = vec![f32::INFINITY; (tw * th) as usize];
        let mut surf = vec![[0.0f32; 4]; (tw * th) as usize];
        for y in 0..th as usize {
            for x in 0..tw as usize {
                let mut acc = [0.0f32; 4];
                for dy in 0..ss {
                    for dx in 0..ss {
                        let p = big[(y * ss + dy) * w + x * ss + dx];
                        for c in 0..4 {
                            acc[c] += p[c];
                        }
                    }
                }
                let n = (ss * ss) as f32;
                out.pixels[y * tw as usize + x] = acc.map(|v| v / n);
                let centre = (y * ss + ss / 2) * w + x * ss + ss / 2;
                depth[y * tw as usize + x] = depth_big[centre];
                surf[y * tw as usize + x] = sbig[centre];
            }
        }
        (out, depth, surf)
    };
    if lights.iter().any(|l| l.look.is_some()) {
        let lens = crate::atmosphere::Lens { position: camera.position, axes: camera.axes, zoom: camera.zoom * tw as f32 / canvas.width.max(1) as f32, centre: [tw as f32 * 0.5, th as f32 * 0.5] };
        let shadows = air_shadows(&lights, &maps);
        let pic = crate::atmosphere::Picture { width: tw as usize, height: th as usize, depth: &depth, surface: &surf };
        if let Some(plan) = crate::atmosphere::plan(&lights, &lens, &pic, &shadows, canvas.height as f32) {
            crate::atmosphere::run(&plan, &pic, &mut out.pixels, gpu);
        }
    }
    out
}

/// Wetness at a point: how much it darkens, the share that is puddle and
/// how well it reflects. `up` is how much the surface faces up.
fn wetness(wet: f32, p: V3, up: f32, metre: f32) -> (f32, f32, f32) {
    let flat = smoothstep(0.35, 0.9, up);
    let cover = ((wet - 0.3) / 0.7).clamp(0.0, 1.0) * 0.65;
    let puddle = if cover > 0.0 {
        let n = crate::atmosphere::cpu::fbm3([p[0] / (1.6 * metre), 0.0, p[2] / (1.6 * metre)]);
        smoothstep(1.0 - cover - 0.07, 1.0 - cover + 0.07, n) * smoothstep(0.85, 0.97, up)
    } else {
        0.0
    };
    let darken = 1.0 - (0.40 * wet + 0.28 * puddle);
    let refl = (wet * (0.20 + 0.12 * flat) + puddle * 0.75).clamp(0.0, 1.0);
    (darken, puddle, refl)
}

fn mix_f(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The shadow maps of the lights the air looks up, reduced to the size it
/// reads.
fn air_shadows(lights: &[LightDesc], maps: &[Option<ShadowMap>]) -> Vec<crate::atmosphere::ShadowVolume> {
    let target = crate::atmosphere::SHADOW_SIZE;
    let mut out = Vec::new();
    for (i, m) in maps.iter().enumerate() {
        let Some(m) = m else { continue };
        if out.len() >= crate::atmosphere::MAX_SHADOWS {
            break;
        }
        // Only lights that fill the air with something to see.
        let seen = lights[i].look.is_some_and(|k| k.beam > 0.0) || lights.iter().any(|l| l.look.is_some_and(|k| k.atmos.is_some_and(|a| a.fog.is_some())));
        if !seen || m.size < target || m.size % target != 0 {
            continue;
        }
        let f = m.size / target;
        let mut depth = vec![f32::INFINITY; target * target];
        for y in 0..target {
            for x in 0..target {
                let mut d = f32::INFINITY;
                for dy in 0..f {
                    for dx in 0..f {
                        d = d.min(m.depth[(y * f + dy) * m.size + x * f + dx]);
                    }
                }
                depth[y * target + x] = if d.is_finite() { d } else { 1.0e30 };
            }
        }
        out.push(crate::atmosphere::ShadowVolume { light: i, position: m.view.position, axes: m.view.axes, zoom: m.view.zoom / f as f32, centre: [m.view.centre[0] / f as f32, m.view.centre[1] / f as f32], ortho: m.view.ortho, texel: m.texel * f as f32, depth });
    }
    out
}

/// The camera ray through a target pixel.
fn pixel_dir(view: &View, x: f32, y: f32) -> V3 {
    let a = view.axes;
    let u = (x - view.centre[0]) / view.zoom;
    let v = (y - view.centre[1]) / view.zoom;
    normalise(add(add(scale(a[0], u), scale(a[1], v)), a[2]))
}

/// Ray-marches each model's volume (smoke, fire) through the picture: the
/// smoke dims what is behind it and scatters the light reaching it (the
/// strongest sun through the smoke's own shadow, plus the ambient and sky
/// light); fire glows by its heat. `depth` is each pixel's distance to the
/// nearest surface along the view axis.
#[allow(clippy::too_many_arguments)]
fn draw_volumes(desc: &MeshDesc, view: &View, lights: &[LightDesc], sky: Option<&crate::sky::SkyDesc>, irr: Option<&crate::sky::Irradiance>, depth: &[f32], out: &mut [[f32; 4]], w: usize, h: usize) {
    let sun = lights.iter().filter(|l| l.kind == LightKind::Parallel).max_by(|a, b| crate::sky::luminance(a.colour).total_cmp(&crate::sky::luminance(b.colour))).copied();
    let ambient: V3 = lights.iter().filter(|l| l.kind == LightKind::Ambient).fold([0.0; 3], |a, l| add(a, l.colour));
    let sky_light = irr.map_or([0.0; 3], |i| i.at([0.0, -1.0, 0.0]));
    let fill = add(ambient, scale(sky_light, 0.6));
    for inst in &desc.instances {
        let Some(vol) = inst.mesh.volume.as_ref() else { continue };
        let [nx, ny, nz] = vol.dims;
        if nx == 0 || ny == 0 || nz == 0 || vol.cells.len() < nx * ny * nz {
            continue;
        }
        let to_model = crate::trace::scene::invert_affine(&inst.world);
        let world_scale = (length([inst.world[0][0], inst.world[1][0], inst.world[2][0]])).max(1e-6);
        let sigma = vol.look.density / world_scale;
        // Light let through from the sun to each cell, worked out once.
        let shade_grid: Vec<f32> = match sun {
            Some(l) => {
                let to_light = normalise(scale(l.direction, -1.0));
                let local = normalise([dot([to_model[0][0], to_model[0][1], to_model[0][2]], to_light), dot([to_model[1][0], to_model[1][1], to_model[1][2]], to_light), dot([to_model[2][0], to_model[2][1], to_model[2][2]], to_light)]);
                let ext: V3 = std::array::from_fn(|i| vol.max[i] - vol.min[i]);
                let step = ext[0].max(ext[1]).max(ext[2]) / 24.0;
                let mut g = vec![1.0f32; nx * ny * nz];
                for z in 0..nz {
                    for y in 0..ny {
                        for x in 0..nx {
                            let mut p = [vol.min[0] + (x as f32 + 0.5) / nx as f32 * ext[0], vol.min[1] + (y as f32 + 0.5) / ny as f32 * ext[1], vol.min[2] + (z as f32 + 0.5) / nz as f32 * ext[2]];
                            let mut optical = 0.0;
                            for _ in 0..24 {
                                p = add(p, scale(local, step));
                                optical += vol.sample(p).0 * vol.look.density * step;
                            }
                            g[(z * ny + y) * nx + x] = (-optical).exp();
                        }
                    }
                }
                g
            }
            None => Vec::new(),
        };
        let lit_at = |p: V3| -> f32 {
            if shade_grid.is_empty() {
                return 1.0;
            }
            let b: [f32; 3] = std::array::from_fn(|i| ((p[i] - vol.min[i]) / (vol.max[i] - vol.min[i]).max(1e-6)).clamp(0.0, 0.999));
            let (x, y, z) = ((b[0] * nx as f32) as usize, (b[1] * ny as f32) as usize, (b[2] * nz as f32) as usize);
            shade_grid[(z * ny + y) * nx + x]
        };
        // The volume's box in the world, to clip each ray.
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for c in 0..8 {
            let p = [if c & 1 == 0 { vol.min[0] } else { vol.max[0] }, if c & 2 == 0 { vol.min[1] } else { vol.max[1] }, if c & 4 == 0 { vol.min[2] } else { vol.max[2] }];
            let q = xform(&inst.world, p);
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        let sun_col = sun.map_or([0.0; 3], |l| l.colour);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
        let rows = h.div_ceil(threads).max(1);
        std::thread::scope(|sc| {
            for (band, chunk) in out.chunks_mut(rows * w).enumerate() {
                let lit_at = &lit_at;
                sc.spawn(move || {
                    for (i, px) in chunk.iter_mut().enumerate() {
                        let gi = band * rows * w + i;
                        let (x, y) = ((gi % w) as f32 + 0.5, (gi / w) as f32 + 0.5);
                        let d = pixel_dir(view, x, y);
                        let o = view.position;
                        // Where the ray is inside the box, and before the surface.
                        let mut t0: f32 = 0.0;
                        let mut t1 = depth[gi] / dot(d, view.axes[2]).max(1e-4);
                        for k in 0..3 {
                            let inv = if d[k].abs() > 1e-12 { 1.0 / d[k] } else { 1e12f32.copysign(d[k]) };
                            let a = (lo[k] - o[k]) * inv;
                            let b = (hi[k] - o[k]) * inv;
                            t0 = t0.max(a.min(b));
                            t1 = t1.min(a.max(b));
                        }
                        if t0 >= t1 {
                            continue;
                        }
                        let steps = 48;
                        let dt = (t1 - t0) / steps as f32;
                        let mut trans = 1.0f32;
                        let mut light = [0.0f32; 3];
                        for k in 0..steps {
                            let wp = add(o, scale(d, t0 + (k as f32 + 0.5) * dt));
                            let mp = xform(&to_model, wp);
                            let (den, heat) = vol.sample(mp);
                            if den <= 0.0 && heat <= 0.0 {
                                continue;
                            }
                            let ext = den * sigma * dt;
                            let a = 1.0 - (-ext).exp();
                            let sun_here = lit_at(mp);
                            let scatter: V3 = std::array::from_fn(|c| vol.look.colour[c] * (sun_col[c] * sun_here + fill[c]));
                            let glow = if vol.look.fire > 0.0 { let f = heat.clamp(0.0, 1.0); [f * 1.6, f * f * 1.1, f * f * f * f * 0.6].map(|v| v * vol.look.fire * f * 4.0) } else { [0.0; 3] };
                            for c in 0..3 {
                                light[c] += trans * (a * scatter[c] + glow[c] * dt * sigma.max(1e-3) * (1.0 - a * 0.5));
                            }
                            trans *= 1.0 - a;
                            if trans < 0.003 {
                                trans = 0.0;
                                break;
                            }
                        }
                        // Over what is behind (premultiplied).
                        for c in 0..3 {
                            px[c] = px[c] * trans + light[c];
                        }
                        px[3] = px[3] * trans + (1.0 - trans);
                    }
                });
            }
        });
        let _ = sky;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(mesh: Mesh, at: V3) -> MeshInstance {
        MeshInstance {
            id: 1,
            mesh: Arc::new(mesh),
            key: "test".into(),
            world: [[1.0, 0.0, 0.0, at[0]], [0.0, 1.0, 0.0, at[1]], [0.0, 0.0, 1.0, at[2]]],
            overrides: MaterialOverride::default(),
            smooth: 1.0,
            accepts_lights: true,
            casts_shadows: true,
            accepts_shadows: true,
            motion: None,
        }
    }

    #[test]
    fn a_ball_is_round_lit_and_in_the_middle() {
        let canvas = Canvas::new(320, 180, 30.0);
        let desc = MeshDesc { instances: vec![instance(studio_mesh::prim::ico_sphere(60.0, 4), [0.0; 3])], camera: None, lights: vec![] };
        let f = render(&desc, &canvas, (320, 180));
        let at = |x: usize, y: usize| f.pixels[y * 320 + x];
        // Covered at the centre, empty at the corner.
        assert!(at(160, 90)[3] > 0.99);
        assert!(at(5, 5)[3] == 0.0);
        // The radius: 60 px at rest depth, so 60 px across from the centre.
        assert!(at(160 + 55, 90)[3] > 0.5 && at(160 + 66, 90)[3] < 0.5);
        // Lit from the upper left: brighter there than lower right.
        let ul = at(160 - 30, 90 - 30)[0];
        let lr = at(160 + 30, 90 + 30)[0];
        assert!(ul > lr * 1.5, "{ul} {lr}");
    }

    #[test]
    fn nearer_models_hide_further_ones_and_cast_shadows() {
        let canvas = Canvas::new(320, 180, 30.0);
        let mut far = instance(studio_mesh::prim::cube([200.0, 200.0, 10.0]), [0.0, 0.0, 100.0]);
        far.id = 2;
        far.overrides.base_colour = Some([0.0, 1.0, 0.0, 1.0]);
        let mut near = instance(studio_mesh::prim::ico_sphere(30.0, 3), [0.0, 0.0, 0.0]);
        near.overrides.base_colour = Some([1.0, 0.0, 0.0, 1.0]);
        // Far first in the list: the depth buffer decides, not the order.
        let desc = MeshDesc { instances: vec![far.clone(), near], camera: None, lights: vec![] };
        let f = render(&desc, &canvas, (320, 180));
        let c = f.pixels[90 * 320 + 160];
        assert!(c[0] > c[1], "the ball is in front: {c:?}");
        // The key light comes from the upper left, so the ball's shadow
        // falls on the board to its lower right.
        let shadowed = f.pixels[(90 + 45) * 320 + 160 + 40][1];
        let open = f.pixels[(90 - 70) * 320 + 160 - 80][1];
        assert!(shadowed < open * 0.7, "shadow {shadowed} vs open {open}");
    }

    #[test]
    fn behind_the_lens_is_cut_not_smeared() {
        let canvas = Canvas::new(160, 90, 30.0);
        let cam = CameraDesc::default_for(&canvas);
        // A floor running from behind the camera to far away.
        let mut floor = studio_mesh::prim::plane(4000.0, 4000.0, 4);
        studio_mesh::ops::transform(&mut floor, &studio_mesh::region::Region::All, [0.0, 60.0, cam.position[2] + 1500.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        let desc = MeshDesc { instances: vec![instance(floor, [0.0; 3])], camera: Some(cam), lights: vec![] };
        let f = render(&desc, &canvas, (160, 90));
        // Below the horizon is floor, above it sky.
        assert!(f.pixels[85 * 160 + 80][3] > 0.99);
        assert!(f.pixels[5 * 160 + 80][3] == 0.0);
    }
}
