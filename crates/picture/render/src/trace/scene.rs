// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A 3D pass flattened for tracing: every model's triangles in the world
//! (where they are when the shutter opens and when it closes), a BVH over
//! them, materials and textures, lights, glowing triangles to sample, the
//! environment as a panorama with its sampling tables, smoke volumes, and
//! the camera. Laid out as plain arrays of 4-float groups, as the GPU reads
//! them; the CPU tracer reads the same arrays.

use crate::mesh_render::{MeshDesc, scene_lights};
use crate::scene::Canvas;
use crate::sky::{self, SkyDesc};
use crate::space::{CameraDesc, LightKind, V3, add, cross, dot, length, normalise, scale, sub};

/// A material: base RGBA; metallic, roughness, transmission, ior;
/// emissive RGB, subsurface; texture offset, texture size (w | h << 16),
/// smooth (0 faceted – 1 smooth), flags (1 hair, 2 unlit).
pub type Mat = [[f32; 4]; 4];
/// A light: kind (0 sun, 1 spot, 2 point), cos of the sun's angular
/// radius, spot cone cosines; position, falloff radius; direction,
/// falloff width; colour, shadow darkness.
pub type Light = [[f32; 4]; 4];

/// Flags in a material's last number.
pub const HAIR: u32 = 1;
pub const UNLIT: u32 = 2;
pub const NO_SHADOW: u32 = 4;

/// The lens.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Camera {
    pub pos: V3,
    pub right: V3,
    pub down: V3,
    pub fwd: V3,
    /// Pixels (of the render) per unit of x/z.
    pub zoom: f32,
    pub centre: [f32; 2],
    /// Thin-lens radius (px) and focus distance (px); radius 0 = pinhole.
    pub lens: f32,
    pub focus: f32,
}

/// A smoke or fire volume in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeInfo {
    /// World → volume box (0..1 per axis) as rows of a 3×4 matrix.
    pub to_box: [[f32; 4]; 3],
    pub dims: [u32; 3],
    /// First cell in [`Scene::vol_cells`] (density and heat per byte pair,
    /// two cells per u32).
    pub offset: u32,
    pub colour: V3,
    /// Extinction per px at full density.
    pub density: f32,
    pub fire: f32,
    /// World box (for quick misses).
    pub lo: V3,
    pub hi: V3,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub pos0: Vec<[f32; 4]>,
    pub pos1: Vec<[f32; 4]>,
    pub nrm: Vec<[f32; 4]>,
    pub uv: Vec<[f32; 2]>,
    /// Corners and material, in BVH leaf order.
    pub tris: Vec<[u32; 4]>,
    /// The instance (index into `ids`) of each triangle.
    pub tri_obj: Vec<u32>,
    /// BVH nodes: (lo, left-or-first as bits), (hi, count as bits).
    pub nodes: Vec<[[f32; 4]; 2]>,
    pub moving: bool,
    pub mats: Vec<Mat>,
    /// Textures packed as RGBA8 (sRGB).
    pub tex: Vec<u32>,
    pub lights: Vec<Light>,
    /// Glowing triangles: (triangle as bits, running total of power,
    /// area, 0), and the total.
    pub emit: Vec<[f32; 4]>,
    pub emit_total: f32,
    /// The environment as a panorama (linear RGB), `env_w` × `env_h`;
    /// empty for none. Rows' running totals then each row's columns.
    pub env: Vec<[f32; 4]>,
    pub env_w: u32,
    pub env_h: u32,
    pub env_rows: Vec<f32>,
    pub env_cols: Vec<f32>,
    pub env_turn: f32,
    /// Whether the environment shows behind the models.
    pub background: bool,
    /// Aerial haze per px and its colour.
    pub haze: f32,
    pub haze_colour: V3,
    pub cam: Camera,
    pub ids: Vec<u64>,
    pub volumes: Vec<VolumeInfo>,
    pub vol_cells: Vec<u32>,
    /// Size of the scene (for ray offsets).
    pub extent: f32,
}

fn xf(m: &[[f32; 4]; 3], p: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

fn normal_matrix(m: &[[f32; 4]; 3]) -> [V3; 3] {
    let c0 = [m[0][0], m[1][0], m[2][0]];
    let c1 = [m[0][1], m[1][1], m[2][1]];
    let c2 = [m[0][2], m[1][2], m[2][2]];
    let n0 = cross(c1, c2);
    let n1 = cross(c2, c0);
    let n2 = cross(c0, c1);
    [[n0[0], n1[0], n2[0]], [n0[1], n1[1], n2[1]], [n0[2], n1[2], n2[2]]]
}

/// The inverse of an affine 3×4 matrix.
pub fn invert_affine(m: &[[f32; 4]; 3]) -> [[f32; 4]; 3] {
    let a: [V3; 3] = std::array::from_fn(|r| [m[r][0], m[r][1], m[r][2]]);
    let c0 = cross(a[1], a[2]);
    let det = dot(a[0], c0);
    if det.abs() < 1e-20 {
        return [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    }
    let c1 = cross(a[2], a[0]);
    let c2 = cross(a[0], a[1]);
    let k = 1.0 / det;
    let inv: [V3; 3] = [[c0[0] * k, c1[0] * k, c2[0] * k], [c0[1] * k, c1[1] * k, c2[1] * k], [c0[2] * k, c1[2] * k, c2[2] * k]];
    let t = [m[0][3], m[1][3], m[2][3]];
    std::array::from_fn(|r| [inv[r][0], inv[r][1], inv[r][2], -dot(inv[r], t)])
}

impl Scene {
    pub fn build(desc: &MeshDesc, canvas: &Canvas, target: (u32, u32)) -> Scene {
        let (tw, th) = target;
        let k = tw as f32 / canvas.width.max(1) as f32;
        let camera = desc.camera.unwrap_or_else(|| CameraDesc::default_for(canvas));
        let mut s = Scene { width: tw, height: th, ..Default::default() };
        s.cam = Camera {
            pos: camera.position,
            right: camera.axes[0],
            down: camera.axes[1],
            fwd: camera.axes[2],
            zoom: camera.zoom * k,
            centre: [tw as f32 * 0.5, th as f32 * 0.5],
            lens: camera.dof.map_or(0.0, |d| d.aperture * 0.5 * d.blur_level),
            focus: camera.dof.map_or(1.0, |d| d.focus.max(1.0)),
        };
        // Geometry, materials and textures.
        let mut tris0: Vec<[V3; 3]> = Vec::new();
        let mut tris1: Vec<[V3; 3]> = Vec::new();
        let mut tri_info: Vec<([u32; 4], u32)> = Vec::new();
        for (oi, inst) in desc.instances.iter().enumerate() {
            s.ids.push(inst.id);
            let mesh = &inst.mesh;
            let base_v = s.pos0.len() as u32;
            let base_m = s.mats.len() as u32;
            let nm = normal_matrix(&inst.world);
            let moved = inst.motion.as_ref().filter(|m| m.mesh.positions.len() == mesh.positions.len());
            for (i, p) in mesh.positions.iter().enumerate() {
                let w0 = xf(&inst.world, *p);
                let w1 = match moved {
                    Some(m) => xf(&m.world, m.mesh.positions[i]),
                    None => w0,
                };
                if w1 != w0 {
                    s.moving = true;
                }
                s.pos0.push([w0[0], w0[1], w0[2], 0.0]);
                s.pos1.push([w1[0], w1[1], w1[2], 0.0]);
                let n = mesh.normals.get(i).copied().unwrap_or([0.0; 3]);
                let wn = normalise([dot(nm[0], n), dot(nm[1], n), dot(nm[2], n)]);
                s.nrm.push([wn[0], wn[1], wn[2], 0.0]);
                s.uv.push(mesh.uvs.get(i).copied().unwrap_or([0.0; 2]));
            }
            let o = &inst.overrides;
            let mats: Vec<studio_mesh::Material> = if mesh.materials.is_empty() { vec![studio_mesh::Material::default()] } else { mesh.materials.clone() };
            for m in &mats {
                let mut base = o.base_colour.unwrap_or(m.base_colour);
                if let Some(a) = o.opacity {
                    base[3] *= a.clamp(0.0, 1.0);
                }
                // Wet: darker and richer, and glossy, so the light that
                // bounces shows the world in it.
                let wet = o.wetness.unwrap_or(0.0).clamp(0.0, 1.0);
                let metallic = o.metallic.unwrap_or(m.metallic).clamp(0.0, 1.0);
                if wet > 0.0 {
                    let dk = (1.0 - 0.40 * wet).powf(1.0 - metallic);
                    for c in 0..3 {
                        base[c] = base[c].max(0.0).powf(1.0 + 0.3 * wet) * dk;
                    }
                }
                let rough = o.roughness.unwrap_or(m.roughness);
                let rough = if wet > 0.0 { rough + (0.06 - rough) * 0.8 * wet } else { rough };
                let e = o.glow_of(m.emissive);
                let (tex_off, tex_size) = match &m.texture {
                    Some(t) if t.width > 0 && t.height > 0 && t.width < 65536 && t.height < 65536 && s.tex.len() < 64_000_000 => {
                        let off = s.tex.len() as u32;
                        s.tex.extend(t.rgba.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])));
                        (off, t.width | (t.height << 16))
                    }
                    _ => (0, 0),
                };
                let mut flags = 0u32;
                if m.name == "Hair" {
                    flags |= HAIR;
                }
                if !inst.accepts_lights {
                    flags |= UNLIT;
                }
                if !inst.casts_shadows {
                    flags |= NO_SHADOW;
                }
                s.mats.push([
                    base,
                    [metallic, rough.clamp(0.02, 1.0), m.transmission.clamp(0.0, 1.0), m.ior.clamp(1.0, 4.0)],
                    [e[0], e[1], e[2], o.subsurface.unwrap_or(m.subsurface).clamp(0.0, 1.0)],
                    [f32::from_bits(tex_off), f32::from_bits(tex_size), inst.smooth.clamp(0.0, 1.0), f32::from_bits(flags)],
                ]);
            }
            let tm = mesh.tri_materials();
            for t in 0..mesh.triangles() {
                let c = mesh.tri(t);
                let mat = base_m + tm.get(t).copied().unwrap_or(0).min(mats.len() as u32 - 1);
                let ids = [base_v + c[0] as u32, base_v + c[1] as u32, base_v + c[2] as u32];
                tris0.push(ids.map(|v| [s.pos0[v as usize][0], s.pos0[v as usize][1], s.pos0[v as usize][2]]));
                tris1.push(ids.map(|v| [s.pos1[v as usize][0], s.pos1[v as usize][1], s.pos1[v as usize][2]]));
                tri_info.push(([ids[0], ids[1], ids[2], mat], oi as u32));
            }
            // Smoke and fire.
            if let Some(v) = &mesh.volume
                && v.dims.iter().all(|d| *d > 0)
                && v.cells.len() >= v.dims[0] * v.dims[1] * v.dims[2]
            {
                let ext: V3 = std::array::from_fn(|i| (v.max[i] - v.min[i]).max(1e-6));
                // Model → box coordinates, after the world → model inverse.
                let to_model = invert_affine(&inst.world);
                let to_box: [[f32; 4]; 3] = std::array::from_fn(|r| {
                    let row = to_model[r];
                    [row[0] / ext[r], row[1] / ext[r], row[2] / ext[r], (row[3] - v.min[r]) / ext[r]]
                });
                let mut lo = [f32::INFINITY; 3];
                let mut hi = [f32::NEG_INFINITY; 3];
                for c in 0..8 {
                    let p = [if c & 1 == 0 { v.min[0] } else { v.max[0] }, if c & 2 == 0 { v.min[1] } else { v.max[1] }, if c & 4 == 0 { v.min[2] } else { v.max[2] }];
                    let w = xf(&inst.world, p);
                    for i in 0..3 {
                        lo[i] = lo[i].min(w[i]);
                        hi[i] = hi[i].max(w[i]);
                    }
                }
                let offset = s.vol_cells.len() as u32 * 2;
                let cells = &v.cells[..v.dims[0] * v.dims[1] * v.dims[2]];
                for pair in cells.chunks(2) {
                    let a = pair[0];
                    let b = pair.get(1).copied().unwrap_or([0, 0]);
                    s.vol_cells.push(u32::from_le_bytes([a[0], a[1], b[0], b[1]]));
                }
                // Scaled from model px to world px by the matrix's size.
                let k = (length([inst.world[0][0], inst.world[1][0], inst.world[2][0]]) * length([inst.world[0][1], inst.world[1][1], inst.world[2][1]]) * length([inst.world[0][2], inst.world[1][2], inst.world[2][2]])).cbrt().max(1e-6);
                s.volumes.push(VolumeInfo { to_box, dims: v.dims.map(|d| d as u32), offset, colour: v.look.colour, density: v.look.density / k, fire: v.look.fire, lo, hi });
            }
        }
        let bvh = if s.moving { studio_mesh::bvh::Bvh::swept(tris0.clone(), &tris1) } else { studio_mesh::bvh::Bvh::new(tris0.clone()) };
        s.tris = bvh.order.iter().map(|&t| tri_info[t as usize].0).collect();
        s.tri_obj = bvh.order.iter().map(|&t| tri_info[t as usize].1).collect();
        s.nodes = bvh.nodes.iter().map(|n| [[n.lo[0], n.lo[1], n.lo[2], f32::from_bits(n.index)], [n.hi[0], n.hi[1], n.hi[2], f32::from_bits(n.count)]]).collect();
        if s.tris.is_empty() {
            s.nodes.clear();
        }
        let (lo, hi) = bvh.nodes.first().map_or(([0.0; 3], [1.0; 3]), |n| (n.lo, n.hi));
        s.extent = length(sub(hi, lo)).max(1.0);
        // Glowing triangles, by how much light each gives.
        let mut total = 0.0;
        for (i, t) in s.tris.iter().enumerate() {
            let m = &s.mats[t[3] as usize];
            let lum = sky::luminance([m[2][0], m[2][1], m[2][2]]);
            if lum <= 0.0 {
                continue;
            }
            let p = |v: u32| [s.pos0[v as usize][0], s.pos0[v as usize][1], s.pos0[v as usize][2]];
            let area = length(cross(sub(p(t[1]), p(t[0])), sub(p(t[2]), p(t[0])))) * 0.5;
            if area <= 0.0 {
                continue;
            }
            total += lum * area;
            s.emit.push([f32::from_bits(i as u32), total, area, 0.0]);
        }
        s.emit_total = total;
        // Lights and the environment.
        let lights = scene_lights(&desc.lights);
        let mut ambient = [0.0f32; 3];
        let mut sky_desc: Option<SkyDesc> = None;
        for l in &lights {
            if let Some(sk) = l.sky {
                sky_desc = Some(sk);
            }
            match l.kind {
                LightKind::Ambient => {
                    if l.sky.is_none() {
                        ambient = add(ambient, l.colour);
                    }
                }
                LightKind::Parallel => {
                    // A sun: a small disk (bigger for plain lights: softer).
                    let cos = if l.sky.is_some() { 0.999_85 } else { 0.999_6 };
                    s.lights.push([[0.0, cos, 0.0, 0.0], [0.0; 4], [l.direction[0], l.direction[1], l.direction[2], 0.0], [l.colour[0], l.colour[1], l.colour[2], if l.casts_shadows { l.shadow_darkness } else { 0.0 }]]);
                }
                LightKind::Spot | LightKind::Point => {
                    let kind = if l.kind == LightKind::Spot { 1.0 } else { 2.0 };
                    s.lights.push([[kind, 0.0, l.cone[0], l.cone[1]], [l.position[0], l.position[1], l.position[2], l.radius], [l.direction[0], l.direction[1], l.direction[2], l.falloff], [l.colour[0], l.colour[1], l.colour[2], if l.casts_shadows { l.shadow_darkness } else { 0.0 }]]);
                }
            }
        }
        if sky_desc.is_some() || ambient != [0.0; 3] {
            let (w, h) = match sky_desc.and_then(|sk| if sk.hdri != 0 { sky::hdri(sk.hdri) } else { None }) {
                Some(img) => {
                    // At most 1024 wide.
                    let step = (img.width / 1024).max(1);
                    ((img.width / step) as u32, (img.height / step) as u32)
                }
                None => (256, 128),
            };
            s.env_w = w;
            s.env_h = h;
            s.env_turn = sky_desc.map_or(0.0, |sk| sk.hdri_turn);
            s.env = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let d = sky::Hdri::dir((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, s.env_turn);
                    let c = sky_desc.map_or([0.0; 3], |sk| sky::radiance(&SkyDesc { hdri_turn: s.env_turn, ..sk }, d));
                    s.env.push([c[0] + ambient[0], c[1] + ambient[1], c[2] + ambient[2], 0.0]);
                }
            }
            // Sampling tables: brightness × the row's share of the sphere.
            s.env_rows = Vec::with_capacity(h as usize + 1);
            s.env_cols = Vec::with_capacity((h * (w + 1)) as usize);
            let mut rows = 0.0;
            s.env_rows.push(0.0);
            for y in 0..h {
                let sin = ((y as f32 + 0.5) / h as f32 * std::f32::consts::PI).sin();
                let mut acc = 0.0;
                s.env_cols.push(0.0);
                for x in 0..w {
                    let c = s.env[(y * w + x) as usize];
                    acc += (sky::luminance([c[0], c[1], c[2]]) * sin).max(1e-8);
                    s.env_cols.push(acc);
                }
                rows += acc;
                s.env_rows.push(rows);
            }
            s.background = sky_desc.is_some_and(|sk| sk.background);
            if let Some(sk) = sky_desc
                && sk.haze > 0.0
            {
                s.haze = sk.haze / 10_000.0;
                s.haze_colour = sky::radiance(&sk, [0.0, -0.05, 1.0]);
            }
        }
        s
    }

    /// The environment's colour looking along `d` (black without one).
    pub fn env_at(&self, d: V3) -> V3 {
        if self.env.is_empty() {
            return [0.0; 3];
        }
        let (u, v) = sky::Hdri::uv(d, self.env_turn);
        let x = ((u * self.env_w as f32) as u32).min(self.env_w - 1);
        let y = ((v * self.env_h as f32) as u32).min(self.env_h - 1);
        let c = self.env[(y * self.env_w + x) as usize];
        [c[0], c[1], c[2]]
    }

    /// Picks a direction from the environment by its brightness: the
    /// direction and its probability per unit solid angle.
    pub fn sample_env(&self, u1: f32, u2: f32) -> (V3, f32) {
        let (w, h) = (self.env_w as usize, self.env_h as usize);
        let total = *self.env_rows.last().unwrap_or(&0.0);
        if total <= 0.0 || w == 0 {
            return ([0.0, -1.0, 0.0], 0.0);
        }
        let target = u1 * total;
        let y = self.env_rows.partition_point(|&r| r <= target).clamp(1, h) - 1;
        let row = &self.env_cols[y * (w + 1)..(y + 1) * (w + 1)];
        let rt = u2 * row[w];
        let x = row.partition_point(|&c| c <= rt).clamp(1, w) - 1;
        let d = sky::Hdri::dir((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, self.env_turn);
        (d, self.env_pdf_xy(x, y))
    }

    fn env_pdf_xy(&self, x: usize, y: usize) -> f32 {
        let (w, h) = (self.env_w as usize, self.env_h as usize);
        let total = *self.env_rows.last().unwrap_or(&0.0);
        let row = &self.env_cols[y * (w + 1)..(y + 1) * (w + 1)];
        let p = (row[x + 1] - row[x]) / total.max(1e-12);
        let sin = ((y as f32 + 0.5) / h as f32 * std::f32::consts::PI).sin().max(1e-6);
        p * (w * h) as f32 / (2.0 * std::f32::consts::PI * std::f32::consts::PI * sin)
    }

    /// The probability per solid angle [`sample_env`] gives direction `d`.
    pub fn env_pdf(&self, d: V3) -> f32 {
        if self.env.is_empty() {
            return 0.0;
        }
        let (u, v) = sky::Hdri::uv(d, self.env_turn);
        let x = ((u * self.env_w as f32) as usize).min(self.env_w as usize - 1);
        let y = ((v * self.env_h as f32) as usize).min(self.env_h as usize - 1);
        self.env_pdf_xy(x, y)
    }

    /// Texture colour (linear RGBA) for material `m` at `uv`.
    pub fn texture(&self, m: &Mat, uv: [f32; 2]) -> [f32; 4] {
        let size = m[3][1].to_bits();
        if size == 0 {
            return [1.0; 4];
        }
        let (w, h) = ((size & 0xffff) as usize, (size >> 16) as usize);
        let off = m[3][0].to_bits() as usize;
        let x = (((uv[0] - uv[0].floor()) * w as f32) as usize).min(w - 1);
        let y = (((uv[1] - uv[1].floor()) * h as f32) as usize).min(h - 1);
        let p = self.tex.get(off + y * w + x).copied().unwrap_or(u32::MAX).to_le_bytes();
        let l = studio_mesh::srgb_to_linear;
        [l(p[0]), l(p[1]), l(p[2]), p[3] as f32 / 255.0]
    }

    /// Density and heat of volume `v` at world point `p` (0..1).
    pub fn volume_at(&self, v: &VolumeInfo, p: V3) -> (f32, f32) {
        let b = xf(&v.to_box, p);
        if b.iter().any(|x| !(0.0..1.0).contains(x)) {
            return (0.0, 0.0);
        }
        let g: [f32; 3] = std::array::from_fn(|i| b[i] * v.dims[i] as f32 - 0.5);
        let i0: [u32; 3] = std::array::from_fn(|i| (g[i].max(0.0) as u32).min(v.dims[i] - 1));
        let i1: [u32; 3] = std::array::from_fn(|i| (i0[i] + 1).min(v.dims[i] - 1));
        let f: [f32; 3] = std::array::from_fn(|i| (g[i] - i0[i] as f32).clamp(0.0, 1.0));
        let cell = |x: u32, y: u32, z: u32| -> (f32, f32) {
            let idx = v.offset + (z * v.dims[1] + y) * v.dims[0] + x;
            let word = self.vol_cells.get((idx / 2) as usize).copied().unwrap_or(0).to_le_bytes();
            let (d, h) = if idx % 2 == 0 { (word[0], word[1]) } else { (word[2], word[3]) };
            (d as f32 / 255.0, h as f32 / 255.0)
        };
        let mut out = (0.0, 0.0);
        for (z, wz) in [(i0[2], 1.0 - f[2]), (i1[2], f[2])] {
            for (y, wy) in [(i0[1], 1.0 - f[1]), (i1[1], f[1])] {
                for (x, wx) in [(i0[0], 1.0 - f[0]), (i1[0], f[0])] {
                    let (d, h) = cell(x, y, z);
                    let w = wx * wy * wz;
                    out.0 += d * w;
                    out.1 += h * w;
                }
            }
        }
        out
    }

    /// The camera ray through pixel position `px` (with lens point `lens`
    /// in −1..1²): origin and direction.
    pub fn camera_ray(&self, px: [f32; 2], lens: [f32; 2]) -> (V3, V3) {
        let c = &self.cam;
        let x = (px[0] - c.centre[0]) / c.zoom;
        let y = (px[1] - c.centre[1]) / c.zoom;
        let d = normalise(add(add(scale(c.right, x), scale(c.down, y)), c.fwd));
        if c.lens <= 0.0 {
            return (c.pos, d);
        }
        // Thin lens: aim at the focal plane through a point on the lens.
        let t = c.focus / dot(d, c.fwd).max(1e-4);
        let focal = add(c.pos, scale(d, t));
        // Concentric disk.
        let (a, b) = (lens[0], lens[1]);
        let (r, phi) = if a == 0.0 && b == 0.0 { (0.0, 0.0) } else if a.abs() > b.abs() { (a, std::f32::consts::FRAC_PI_4 * (b / a)) } else { (b, std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_4 * (a / b)) };
        let o = add(c.pos, add(scale(c.right, r * phi.cos() * c.lens), scale(c.down, r * phi.sin() * c.lens)));
        (o, normalise(sub(focal, o)))
    }
}
