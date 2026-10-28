// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D models for Studio.
//!
//! A [`Mesh`] is triangles with per-vertex normals and texture coordinates,
//! split into groups that each use one [`Material`] (physically based:
//! base colour, metallic, roughness, emissive, an optional colour
//! texture). Units are canvas pixels in Studio's world: x right, y down,
//! z away from the viewer, the origin at the model's centre.
//!
//! - [`prim`]: cube, spheres, cylinder, cone, torus, capsule, plane.
//! - [`io`]: Studio's own compact `.smesh` file (what projects keep).
//! - [`import`] / [`export`]: glTF 2.0 (`.glb`, `.gltf`), OBJ, STL in;
//!   GLB and OBJ out.
//! - [`ops`]: normals, transforms, Loop subdivision, smoothing, sculpt
//!   brushes, mirror and symmetry, extrusion, displacement, merge, array,
//!   UV projection; [`csg`]: booleans; [`decimate`]: fewer triangles.
//! - [`sdf`]: blended shapes melted into one surface (organic forms),
//!   and an anatomical head to start from.
//! - [`math`]: shapes from formulas: implicit solids and lattices,
//!   fractals, parametric surfaces, tubes, height fields, space warps,
//!   L-systems and phyllotaxis.
//! - [`region`]: picking part of a model in words a person or an AI can
//!   say ("the faces looking up", "inside this sphere").

pub mod autorig;
pub mod bvh;
pub mod cache;
pub mod character;
pub mod csg;
pub mod decimate;
pub mod export;
pub mod fabric;
pub mod foreign;
pub mod face;
pub mod facekit;
pub mod hand;
pub mod flora;
pub mod group;
pub mod import;
pub mod io;
pub mod loft;
pub mod math;
pub mod mocap;
pub mod motion;
pub mod perform;
pub mod ops;
pub mod prim;
pub mod prop;
pub mod proportions;
pub mod region;
pub mod rig;
pub mod rigfit;
pub mod sdf;
pub mod seal;
pub mod species;
pub mod terrain;
pub mod wear;

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
    let l = length(a);
    if l > 1e-12 { scale(a, 1.0 / l) } else { [0.0, 0.0, 0.0] }
}
pub fn lerp(a: V3, b: V3, t: f32) -> V3 {
    add(a, scale(sub(b, a), t))
}

/// A colour picture for a material (straight RGBA, sRGB-encoded).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    /// The colour at texture coordinates `uv` (wrapping, bilinear), as
    /// linear RGBA.
    pub fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        let (w, h) = (self.width.max(1) as f32, self.height.max(1) as f32);
        let x = (uv[0] - uv[0].floor()) * w - 0.5;
        let y = (uv[1] - uv[1].floor()) * h - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| -> [f32; 4] {
            let xi = (xi as i64).rem_euclid(self.width.max(1) as i64) as usize;
            let yi = (yi as i64).rem_euclid(self.height.max(1) as i64) as usize;
            let i = (yi * self.width as usize + xi) * 4;
            let p = self.rgba.get(i..i + 4).unwrap_or(&[255, 255, 255, 255]);
            [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3] as f32 / 255.0]
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

pub fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// How a surface takes light (the glTF metallic-roughness model).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// Linear RGB and opacity.
    pub base_colour: [f32; 4],
    /// 0 = plastic, skin, wood; 1 = metal.
    pub metallic: f32,
    /// 0 = mirror-smooth; 1 = fully matte.
    pub roughness: f32,
    /// Light the surface gives off itself (linear RGB).
    pub emissive: V3,
    /// Light that travels under the surface and comes out warmer and
    /// softer (skin, wax, marble, leaves): 0 none – 1 a lot.
    pub subsurface: f32,
    /// Light passing through (glass, water, ice): 0 none – 1 clear.
    pub transmission: f32,
    /// How much light bends going in (1.0 air, 1.33 water, 1.5 glass,
    /// 2.4 diamond).
    pub ior: f32,
    pub texture: Option<Texture>,
}

impl Default for Material {
    fn default() -> Material {
        Material { name: "Default".into(), base_colour: [0.8, 0.8, 0.8, 1.0], metallic: 0.0, roughness: 0.5, emissive: [0.0; 3], subsurface: 0.0, transmission: 0.0, ior: 1.45, texture: None }
    }
}

/// A run of triangles using one material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    /// First triangle (not index) of the run.
    pub start: u32,
    pub count: u32,
    pub material: u32,
}

/// Triangles with normals and texture coordinates, in groups by material.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Mesh {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub uvs: Vec<[f32; 2]>,
    /// Three per triangle, wound so that `cross(b - a, c - a)` points out
    /// of the surface.
    pub indices: Vec<u32>,
    pub groups: Vec<Group>,
    pub materials: Vec<Material>,
    /// Bones, skin, shape keys and motion, when the model is rigged.
    pub rig: Option<std::sync::Arc<rig::Rig>>,
    /// Smoke, fire or mist filling a box (a simulation frame), drawn by
    /// marching rays through it.
    pub volume: Option<std::sync::Arc<Volume>>,
}

/// How a volume looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeLook {
    /// Linear RGB the smoke scatters (white steam, grey smoke, black soot).
    pub colour: V3,
    /// How much light a full cell stops per pixel travelled.
    pub density: f32,
    /// How bright the hot parts glow (0 = no fire).
    pub fire: f32,
}

impl Default for VolumeLook {
    fn default() -> VolumeLook {
        VolumeLook { colour: [0.55, 0.55, 0.58], density: 0.05, fire: 0.0 }
    }
}

/// A grid of density and heat over the box `min..max` (model px), cells
/// x fastest, then y, then z.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub dims: [usize; 3],
    pub min: V3,
    pub max: V3,
    /// Density and heat per cell, 0–255.
    pub cells: Vec<[u8; 2]>,
    pub look: VolumeLook,
}

impl Volume {
    /// Density and heat (0..1) at `p`, blended between cells; zero outside.
    pub fn sample(&self, p: V3) -> (f32, f32) {
        let [nx, ny, nz] = self.dims;
        if nx == 0 || ny == 0 || nz == 0 || self.cells.len() < nx * ny * nz {
            return (0.0, 0.0);
        }
        let g: [f32; 3] = std::array::from_fn(|i| (p[i] - self.min[i]) / (self.max[i] - self.min[i]).max(1e-6) * self.dims[i] as f32 - 0.5);
        if g.iter().zip(&self.dims).any(|(v, d)| *v < -0.5 || *v > *d as f32 - 0.5) {
            return (0.0, 0.0);
        }
        let i0: [usize; 3] = std::array::from_fn(|i| (g[i].floor().max(0.0) as usize).min(self.dims[i] - 1));
        let i1: [usize; 3] = std::array::from_fn(|i| (i0[i] + 1).min(self.dims[i] - 1));
        let f: [f32; 3] = std::array::from_fn(|i| (g[i] - i0[i] as f32).clamp(0.0, 1.0));
        let at = |x: usize, y: usize, z: usize| self.cells[(z * ny + y) * nx + x];
        let mut out = [0.0f32; 2];
        for (dz, wz) in [(i0[2], 1.0 - f[2]), (i1[2], f[2])] {
            for (dy, wy) in [(i0[1], 1.0 - f[1]), (i1[1], f[1])] {
                for (dx, wx) in [(i0[0], 1.0 - f[0]), (i1[0], f[0])] {
                    let c = at(dx, dy, dz);
                    let w = wx * wy * wz;
                    out[0] += c[0] as f32 * w;
                    out[1] += c[1] as f32 * w;
                }
            }
        }
        (out[0] / 255.0, out[1] / 255.0)
    }
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// The corners of triangle `t`.
    pub fn tri(&self, t: usize) -> [usize; 3] {
        [self.indices[3 * t] as usize, self.indices[3 * t + 1] as usize, self.indices[3 * t + 2] as usize]
    }

    /// The material index of each triangle.
    pub fn tri_materials(&self) -> Vec<u32> {
        let mut m = vec![0u32; self.triangles()];
        for g in &self.groups {
            for t in g.start..(g.start + g.count).min(m.len() as u32) {
                m[t as usize] = g.material;
            }
        }
        m
    }

    /// Groups rebuilt from a material per triangle (triangles reordered so
    /// each material's are together).
    pub fn regroup(&mut self, mats: &[u32]) {
        let n = self.triangles();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&t| mats.get(t).copied().unwrap_or(0));
        let mut idx = Vec::with_capacity(self.indices.len());
        let mut groups: Vec<Group> = Vec::new();
        for (k, &t) in order.iter().enumerate() {
            idx.extend_from_slice(&self.indices[3 * t..3 * t + 3]);
            let m = mats.get(t).copied().unwrap_or(0);
            match groups.last_mut() {
                Some(g) if g.material == m => g.count += 1,
                _ => groups.push(Group { start: k as u32, count: 1, material: m }),
            }
        }
        self.indices = idx;
        self.groups = groups;
        if self.materials.is_empty() {
            self.materials.push(Material::default());
        }
    }

    /// One group over every triangle with material 0 (made if missing).
    pub fn single_group(&mut self) {
        if self.materials.is_empty() {
            self.materials.push(Material::default());
        }
        self.groups = vec![Group { start: 0, count: self.triangles() as u32, material: 0 }];
    }

    /// Smallest and largest corner.
    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (lo, hi)
    }

    /// Face normal of triangle `t` (unit; zero for a degenerate one).
    pub fn face_normal(&self, t: usize) -> V3 {
        let [a, b, c] = self.tri(t);
        normalise(cross(sub(self.positions[b], self.positions[a]), sub(self.positions[c], self.positions[a])))
    }

    pub fn centroid(&self, t: usize) -> V3 {
        let [a, b, c] = self.tri(t);
        scale(add(add(self.positions[a], self.positions[b]), self.positions[c]), 1.0 / 3.0)
    }

    /// Missing normals and texture coordinates filled in, groups covering
    /// every triangle: what every importer and op leaves behind.
    pub fn tidy(&mut self) {
        let n = self.positions.len();
        if self.uvs.len() != n {
            self.uvs.resize(n, [0.0, 0.0]);
        }
        if self.normals.len() != n {
            self.normals = vec![[0.0; 3]; n];
            ops::smooth_normals_in_place(self);
        }
        let covered: u32 = self.groups.iter().map(|g| g.count).sum();
        if self.groups.is_empty() || covered as usize != self.triangles() {
            let mats = self.tri_materials();
            self.regroup(&mats);
        }
        if self.materials.is_empty() {
            self.materials.push(Material::default());
        }
    }

    /// Welds vertices that share a position: for each vertex, the id of
    /// its position (equal positions share an id). Seams in normals or
    /// texture coordinates stay as separate vertices with one position id.
    pub fn position_ids(&self) -> (Vec<u32>, Vec<V3>) {
        let mut map: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
        let mut ids = Vec::with_capacity(self.positions.len());
        let mut unique = Vec::new();
        // Quantised to a thousandth of a pixel: equal for practical purposes.
        for p in &self.positions {
            let k = [(p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64, (p[2] * 1000.0).round() as i64];
            let id = *map.entry(k).or_insert_with(|| {
                unique.push(*p);
                (unique.len() - 1) as u32
            });
            ids.push(id);
        }
        (ids, unique)
    }

    /// Statistics in words, for `inspect` and the Inspector.
    pub fn stats(&self) -> String {
        let (lo, hi) = self.bounds();
        format!("{} triangles, {} vertices, {} material{}, {:.0}×{:.0}×{:.0} px", self.triangles(), self.positions.len(), self.materials.len(), if self.materials.len() == 1 { "" } else { "s" }, hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2])
    }
}
