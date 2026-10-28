// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A game-quality real-time renderer for walkable worlds.
//!
//! Big scenes stay fast by drawing only what is seen: terrain chunks and
//! scattered copies are frustum-culled, drawn at a level of detail that
//! follows distance, and instanced (one draw per shape and level, however
//! many trees). The sun casts shadows through four cascades; the sky lights
//! everything (diffuse from its spherical harmonics, glossy reflections
//! from its mips) and fogs the distance; contact shadowing comes from
//! screen-space ambient occlusion; water reflects what is above it and
//! shows what is below; bloom and a filmic tone map finish the picture.
//!
//! A still camera refines: each frame is rendered with a different sub-pixel
//! jitter and the frames are averaged, which anti-aliases, softens shadow
//! edges and cleans up the ambient occlusion. Move, and it resets to a
//! single fast frame.

pub mod frame;
pub mod gpu;
mod draw;
mod targets;

use crate::frame::Frame;
use crate::gpu::{GpuContext, f16_to_f32};
use frame::{Cascade, EnvData, FrameU, LightU, View};
use gpu::{HDR, Pipes};
use std::collections::HashMap;
use std::sync::Arc;
use studio_mesh::Mesh;
use studio_world::cull::{CullParams, DrawList, cull};
use studio_world::math::{self, Frustum, mat_mul};
use studio_world::world::World;
use targets::{BLOOM_LEVELS, Targets};
use wgpu::util::DeviceExt;

pub use frame::{View as Camera, build_env};

/// How much to spend on a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Moving about: three cascades of 1024, no reflections, no occlusion.
    Fast,
    /// Four cascades of 2048, occlusion, reflections at half size.
    Balanced,
    /// Everything on, more cascades' reach, finer filtering.
    High,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub width: u32,
    pub height: u32,
    pub quality: Quality,
    /// Seconds, for wind and waves.
    pub time: f32,
    /// Keep averaging jittered frames while nothing changes.
    pub refine: bool,
    /// Frames to average at most.
    pub max_refine: u32,
    /// Hour of the day to light the world for, over the world's own.
    pub sky_time: Option<f32>,
    /// A figure standing where the walker is (third person).
    pub avatar: Option<Avatar>,
    /// Fog from outside the world (a project's scene air): density per
    /// metre, thinning per metre, colour; over the world's own.
    pub fog: Option<(f32, f32, Option<[f32; 3]>)>,
    /// A look from outside the world (a project's scene look): over its own.
    /// Exposure (stops), bloom, occlusion, saturation, vignette.
    pub look: Option<[f32; 5]>,
    /// Toon (cel) shading in this many hard bands (2 or 3); 0 shades
    /// physically.
    pub toon: u8,
}

/// The walker's body, drawn when the camera is behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Avatar {
    /// Feet.
    pub pos: [f32; 3],
    pub yaw: f32,
    /// Metres per second along the ground.
    pub speed: f32,
    /// Seconds, for the stride.
    pub time: f32,
}

impl Options {
    pub fn new(width: u32, height: u32) -> Options {
        Options { width, height, quality: Quality::Balanced, time: 0.0, refine: false, max_refine: 48, sky_time: None, avatar: None, fog: None, look: None, toon: 0 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub drawn: usize,
    pub chunks: usize,
    pub triangles: usize,
    pub draw_calls: usize,
    /// Frames averaged into this picture (1 while moving).
    pub refined: u32,
    pub ms: f32,
}

/// An 8-bit sRGB picture.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct GroupGpu {
    first: u32,
    count: u32,
    bind: wgpu::BindGroup,
}

struct MeshGpu {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    groups: Vec<GroupGpu>,
}

struct ChunkGpu {
    rev: u64,
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
}

struct TerrainGpu {
    rev: u64,
    dims: [u32; 2],
    splat: targets::Tex,
    layers: wgpu::Buffer,
    bind: wgpu::BindGroup,
    chunks: HashMap<(usize, usize, usize), ChunkGpu>,
}

struct PosedGpu {
    mesh: MeshGpu,
    used: u64,
}

struct WaterGpu {
    ubuf: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

/// What a frame is to draw, with the instance data placed in one buffer.
struct Prepared {
    lists: Vec<DrawList>,
    starts: Vec<Vec<u32>>,
    /// Posed characters per list: mesh key and instance index.
    dyn_draws: Vec<Vec<((u64, String, i64), u32)>>,
    refl: Option<ReflPlan>,
    cascades: Vec<Cascade>,
    frame_main: FrameU,
    frame_refl: Option<FrameU>,
    frame_cascade: Vec<FrameU>,
    water_visible: Vec<usize>,
    stats: Stats,
    exposure: f32,
    bloom: f32,
    refine_n: u32,
    use_ssao: bool,
    use_shadows: bool,
    saturation: f32,
    vignette: f32,
    fxaa: bool,
}

struct ReflPlan {
    level: f32,
}

pub struct Renderer {
    ctx: Arc<GpuContext>,
    p: Pipes,
    targets: Option<Targets>,
    frame_bufs: Vec<wgpu::Buffer>,
    bg_min: Vec<wgpu::BindGroup>,
    inst: wgpu::Buffer,
    inst_cap: u64,
    env_tex: wgpu::Texture,
    env_view: wgpu::TextureView,
    env_key: u64,
    env: Option<EnvData>,
    env_exposure: f32,
    meshes: HashMap<(u64, usize), MeshGpu>,
    posed: HashMap<(u64, String, i64), PosedGpu>,
    frame_no: u64,
    avatar_asset: Option<studio_world::asset::Asset>,
    terrain: Option<TerrainGpu>,
    white_tex: targets::Tex,
    water_grid: (wgpu::Buffer, wgpu::Buffer, u32),
    water: Vec<WaterGpu>,
    accum_n: u32,
    accum_key: u64,
    accum_cur: usize,
    /// Cache keys of the current world's assets, by index.
    keys: Vec<u64>,
}

fn halton(mut i: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

fn hash_f(h: &mut std::collections::hash_map::DefaultHasher, vals: &[f32]) {
    use std::hash::Hash;
    for v in vals {
        v.to_bits().hash(h);
    }
}

impl Renderer {
    pub fn new(ctx: Arc<GpuContext>) -> Result<Renderer, String> {
        let p = Pipes::new(&ctx)?;
        let d = &ctx.device;
        let frame_bufs: Vec<wgpu::Buffer> = (0..6).map(|_| d.create_buffer(&wgpu::BufferDescriptor { label: Some("rt frame"), size: std::mem::size_of::<FrameU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })).collect();
        let bg_min = frame_bufs.iter().map(|b| d.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt frame min"), layout: &p.layouts.min, entries: &[wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }] })).collect();
        let env_tex = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("rt env"),
            size: wgpu::Extent3d { width: frame::ENV_W, height: frame::ENV_H, depth_or_array_layers: 1 },
            mip_level_count: frame::ENV_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let env_view = env_tex.create_view(&Default::default());
        let white_tex = targets::tex2d(d, "rt white rgba", 1, 1, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        ctx.queue.write_texture(white_tex.tex.as_image_copy(), &[255; 4], wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) }, wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
        // A unit grid of water.
        let n = 64u32;
        let mut verts = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                verts.extend_from_slice(&[x as f32 / n as f32, z as f32 / n as f32]);
            }
        }
        let mut idx = Vec::new();
        for z in 0..n {
            for x in 0..n {
                let a = z * (n + 1) + x;
                idx.extend_from_slice(&[a, a + n + 1, a + n + 2, a, a + n + 2, a + 1]);
            }
        }
        let wvb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water grid"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
        let wib = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water grid"), contents: bytemuck::cast_slice(&idx), usage: wgpu::BufferUsages::INDEX });
        let inst = d.create_buffer(&wgpu::BufferDescriptor { label: Some("rt instances"), size: 32 * 4096, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Ok(Renderer {
            ctx,
            p,
            targets: None,
            frame_bufs,
            bg_min,
            inst,
            inst_cap: 32 * 4096,
            env_tex,
            env_view,
            env_key: 0,
            env: None,
            env_exposure: 0.0,
            meshes: HashMap::new(),
            posed: HashMap::new(),
            frame_no: 0,
            avatar_asset: None,
            terrain: None,
            white_tex,
            water_grid: (wvb, wib, idx.len() as u32),
            water: Vec::new(),
            accum_n: 0,
            accum_key: 0,
            accum_cur: 0,
            keys: Vec::new(),
        })
    }

    pub fn context(&self) -> &Arc<GpuContext> {
        &self.ctx
    }

    /// Forgets the models and terrain kept on the GPU (a different world).
    pub fn reset(&mut self) {
        self.meshes.clear();
        self.terrain = None;
        self.accum_n = 0;
        self.env_key = 0;
    }

    // -------------------------------------------------------------- uploads

    fn ensure_env(&mut self, w: &World, sky_time: Option<f32>, exposure: f32) {
        let mut sky = w.sky.clone();
        if let Some(t) = sky_time {
            sky.time = t.rem_euclid(24.0);
        }
        let key = frame::env_key(&sky) ^ exposure.to_bits() as u64;
        if key == self.env_key && self.env.is_some() {
            return;
        }
        let env = frame::build_env(&sky, exposure);
        for (level, (mw, mh, bits)) in env.mips.iter().enumerate() {
            self.ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.env_tex, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(bits),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(mw * 8), rows_per_image: Some(*mh) },
                wgpu::Extent3d { width: *mw, height: *mh, depth_or_array_layers: 1 },
            );
        }
        self.env_exposure = env.exposure;
        self.env = Some(env);
        self.env_key = key;
    }

    fn ensure_mesh(&mut self, key: u64, lod: usize, mesh: &Mesh, height: f32, sway: f32) {
        if self.meshes.contains_key(&(key, lod)) {
            return;
        }
        let g = self.make_mesh_gpu(mesh, height, sway);
        self.meshes.insert((key, lod), g);
    }

    fn make_mesh_gpu(&self, mesh: &Mesh, height: f32, sway: f32) -> MeshGpu {
        let d = &self.ctx.device;
        let mut verts: Vec<f32> = Vec::with_capacity(mesh.positions.len() * 8);
        for (i, p) in mesh.positions.iter().enumerate() {
            let n = mesh.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let uv = mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            verts.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1]]);
        }
        let vb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("rt mesh"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
        let ib = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("rt mesh"), contents: bytemuck::cast_slice(&mesh.indices), usage: wgpu::BufferUsages::INDEX });
        let mut groups = Vec::new();
        let default_group = [studio_mesh::Group { start: 0, count: mesh.triangles() as u32, material: 0 }];
        let gs: &[studio_mesh::Group] = if mesh.groups.is_empty() { &default_group } else { &mesh.groups };
        for g in gs {
            let m = mesh.materials.get(g.material as usize).cloned().unwrap_or_default();
            let (view, textured) = match &m.texture {
                Some(t) if t.width > 0 && t.height > 0 && t.rgba.len() >= (t.width * t.height * 4) as usize => (self.upload_texture(t), 1.0),
                _ => (None, 0.0),
            };
            let uniform: [f32; 16] = [
                m.base_colour[0], m.base_colour[1], m.base_colour[2], m.base_colour[3],
                m.emissive[0], m.emissive[1], m.emissive[2], m.metallic,
                m.roughness, m.subsurface.max(if sway > 0.0 { 0.35 } else { 0.0 }), textured, sway,
                height, 1.0, 0.0, 0.0,
            ];
            let ub = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("rt material"), contents: bytemuck::cast_slice(&uniform), usage: wgpu::BufferUsages::UNIFORM });
            let tv = view.as_ref().unwrap_or(&self.white_tex.view);
            let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rt material"),
                layout: &self.p.layouts.mat,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tv) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.p.tex_samp) },
                ],
            });
            groups.push(GroupGpu { first: g.start * 3, count: g.count * 3, bind });
        }
        MeshGpu { vb, ib, groups }
    }

    fn upload_texture(&self, t: &studio_mesh::Texture) -> Option<wgpu::TextureView> {
        let d = &self.ctx.device;
        let mips = (32 - t.width.max(t.height).leading_zeros()).clamp(1, 12);
        let tex = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("rt material texture"),
            size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h) = (t.width, t.height);
        let mut data = t.rgba.clone();
        for level in 0..mips {
            self.ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut s = 0u32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                            s += data[((sy * w + sx) * 4 + c) as usize] as u32;
                        }
                        next[((y * nw + x) * 4 + c) as usize] = (s / 4) as u8;
                    }
                }
            }
            data = next;
            w = nw;
            h = nh;
        }
        Some(tex.create_view(&Default::default()))
    }

    fn ensure_terrain(&mut self, w: &World) {
        let Some(t) = &w.terrain else {
            self.terrain = None;
            return;
        };
        let d = &self.ctx.device;
        let dims = [t.cells[0] as u32 + 1, t.cells[1] as u32 + 1];
        let rebuild = self.terrain.as_ref().is_none_or(|g| g.dims != dims);
        if rebuild {
            let splat = targets::tex2d(d, "rt splat", dims[0], dims[1], wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
            let layers = d.create_buffer(&wgpu::BufferDescriptor { label: Some("rt layers"), size: 16 * 4 * 3, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rt terrain"),
                layout: &self.p.layouts.terrain,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&splat.view) }, wgpu::BindGroupEntry { binding: 1, resource: layers.as_entire_binding() }],
            });
            self.terrain = Some(TerrainGpu { rev: 0, dims, splat, layers, bind, chunks: HashMap::new() });
        }
        let g = self.terrain.as_mut().unwrap();
        if g.rev != t.revision {
            let bytes: Vec<u8> = t.splat.iter().flat_map(|s| *s).collect();
            self.ctx.queue.write_texture(g.splat.tex.as_image_copy(), &bytes, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(dims[0] * 4), rows_per_image: Some(dims[1]) }, wgpu::Extent3d { width: dims[0], height: dims[1], depth_or_array_layers: 1 });
            let mut data = [0.0f32; 24];
            for k in 0..4 {
                let l = &t.layers[k];
                data[k * 4..k * 4 + 4].copy_from_slice(&[l.colour[0], l.colour[1], l.colour[2], l.roughness]);
                data[16 + k] = l.tile;
            }
            let size = t.size();
            data[20..24].copy_from_slice(&[t.origin[0], t.origin[1], size[0], size[1]]);
            self.ctx.queue.write_buffer(&g.layers, 0, bytemuck::cast_slice(&data));
            g.rev = t.revision;
        }
    }

    fn ensure_chunk(&mut self, w: &World, i: usize, j: usize, lod: usize) {
        let (Some(t), Some(g)) = (&w.terrain, self.terrain.as_mut()) else { return };
        let rev = t.chunk_revision(i, j);
        if g.chunks.get(&(i, j, lod)).is_some_and(|c| c.rev == rev) {
            return;
        }
        let m = t.chunk_mesh(i, j, lod);
        let mut verts: Vec<f32> = Vec::with_capacity(m.positions.len() * 6);
        for (p, n) in m.positions.iter().zip(&m.normals) {
            verts.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2]]);
        }
        let d = &self.ctx.device;
        let vb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("rt chunk"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
        let ib = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("rt chunk"), contents: bytemuck::cast_slice(&m.indices), usage: wgpu::BufferUsages::INDEX });
        g.chunks.insert((i, j, lod), ChunkGpu { rev, vb, ib, count: m.indices.len() as u32 });
    }

    fn ensure_targets(&mut self, w: u32, h: u32, shadow_res: u32) {
        let ok = self.targets.as_ref().is_some_and(|t| t.w == w && t.h == h && t.shadow_res == shadow_res);
        if !ok {
            self.targets = Some(Targets::new(&self.ctx, &self.p, w, h, shadow_res, &self.frame_bufs, &self.env_view));
            self.water.clear();
            self.accum_n = 0;
        }
    }

    fn ensure_water(&mut self, n: usize) {
        let t = self.targets.as_ref().unwrap();
        while self.water.len() < n.min(8) {
            let d = &self.ctx.device;
            let ubuf = d.create_buffer(&wgpu::BufferDescriptor { label: Some("rt water"), size: 48, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rt water"),
                layout: &self.p.layouts.water,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&t.depth_copy.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.col_copy.view) },
                ],
            });
            self.water.push(WaterGpu { ubuf, bind });
        }
    }
}

// ---------------------------------------------------------------------------
// Frame setup (CPU)

fn quality_params(q: Quality) -> (u32, usize, bool, bool, f32) {
    // shadow resolution, cascades, ssao, reflections, shadow distance factor
    match q {
        Quality::Fast => (1024, 3, false, false, 0.7),
        Quality::Balanced => (2048, 4, true, true, 1.0),
        Quality::High => (2048, 4, true, true, 1.25),
    }
}

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    fn prepare(&mut self, w: &World, view: &View, o: &Options) -> Result<Prepared, String> {
        let (width, height) = (o.width.max(16), o.height.max(16));
        let aspect = width as f32 / height as f32;
        let (shadow_res, ncasc, want_ssao, want_refl, shadow_scale) = quality_params(o.quality);
        self.keys = w.assets.iter().map(|a| a.key).collect();
        // What the picture is finished with: the world's, or the project's.
        let look = match o.look {
            Some([exposure, bloom, ssao, saturation, vignette]) => studio_world::world::Look { exposure, bloom, ssao, saturation, vignette },
            None => w.look.clone(),
        };
        let atmosphere = match o.fog {
            Some((fog, height_falloff, colour)) => studio_world::world::Atmosphere { fog, height_falloff, colour },
            None => w.atmosphere.clone(),
        };
        self.ensure_env(w, o.sky_time, look.exposure);
        self.ensure_targets(width, height, shadow_res);
        self.ensure_terrain(w);
        let env = self.env.as_ref().unwrap();
        // Progressive refinement: a still camera averages jittered frames.
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            hash_f(&mut h, &[view.position[0], view.position[1], view.position[2], view.yaw, view.pitch, view.roll, view.fov, o.time, view.near, view.far]);
            (width, height, w.revision, o.quality as u8, w as *const World as usize, self.env_key, w.terrain.as_ref().map_or(0, |t| t.revision)).hash(&mut h);
            h.finish()
        };
        if key == self.accum_key && o.refine {
            self.accum_n = (self.accum_n + 1).min(o.max_refine.max(1));
        } else {
            self.accum_n = 1;
            self.accum_cur = 0;
        }
        let refine_n = self.accum_n;
        self.accum_key = key;
        let jitter_px = if o.refine && refine_n > 1 { [halton(refine_n, 2) - 0.5, halton(refine_n, 3) - 0.5] } else { [0.0, 0.0] };
        let jitter_ndc = [jitter_px[0] * 2.0 / width as f32, -jitter_px[1] * 2.0 / height as f32];
        let vm = view.view_matrix();
        let pm = view.proj_matrix(aspect, jitter_ndc);
        let vp = mat_mul(&pm, &vm);
        let vp_clean = mat_mul(&view.proj_matrix(aspect, [0.0, 0.0]), &vm);
        let frustum = Frustum::from_view_proj(&vp_clean);
        let bounds = w.bounds();
        let extent = math::len(math::sub(bounds.max, bounds.min));
        // The sun.
        let light_dir = math::mul(env.light_to, -1.0);
        let shadow_far = (180.0 * shadow_scale).min(view.far).min(extent.max(60.0));
        let reach = (bounds.max[1] - bounds.min[1]).max(20.0) + 40.0;
        let use_shadows = env.light_colour.iter().sum::<f32>() > 1e-4;
        let casc = frame::cascades(view, aspect, light_dir, ncasc, shadow_far, shadow_res, reach);
        // Lists to draw: main, reflection (if any), cascades.
        let mut lists = Vec::new();
        let terrain_lod = [45.0, 110.0, 260.0];
        lists.push(cull(w, &CullParams { eye: view.position, frustum, lod_bias: 1.0, density: 1.0, terrain_lod, time: o.time }));
        // Water in view.
        let mut water_visible = Vec::new();
        for (i, wt) in w.water.iter().enumerate().take(8) {
            let b = math::Aabb::new([wt.min[0], wt.level - 0.5, wt.min[1]], [wt.max[0], wt.level + wt.wave_height + 0.5, wt.max[1]]);
            if frustum.aabb(&b) {
                water_visible.push(i);
            }
        }
        let under = w.water.iter().any(|wt| view.position[0] >= wt.min[0] && view.position[0] <= wt.max[0] && view.position[2] >= wt.min[1] && view.position[2] <= wt.max[1] && view.position[1] < wt.level + 0.05);
        let refl = if want_refl && !under { water_visible.first().map(|i| ReflPlan { level: w.water[*i].level }) } else { None };
        let mut refl_view = None;
        if let Some(r) = &refl {
            let m = reflect_matrix(r.level);
            let rv = mat_mul(&vm, &m);
            let rvp = mat_mul(&mat_mul(&view.proj_matrix(aspect, jitter_ndc), &vm), &m);
            let eye2 = [view.position[0], 2.0 * r.level - view.position[1], view.position[2]];
            let f2 = Frustum::from_view_proj(&mat_mul(&mat_mul(&view.proj_matrix(aspect, [0.0; 2]), &vm), &m));
            lists.push(cull(w, &CullParams { eye: eye2, frustum: f2, lod_bias: 0.55, density: 0.25, terrain_lod: [60.0, 140.0, 300.0], time: o.time }));
            refl_view = Some((rv, rvp, eye2));
        }
        let first_cascade = lists.len();
        if use_shadows {
            for c in &casc {
                let f = Frustum::from_view_proj(&c.vp);
                // Shadows: nothing thinned, coarser terrain.
                let mut l = cull(w, &CullParams { eye: view.position, frustum: f, lod_bias: 0.9, density: 1.0, terrain_lod: [30.0, 80.0, 160.0], time: o.time });
                l.batches.retain(|b| w.assets[b.asset].casts_shadow);
                lists.push(l);
            }
        }
        let stats = Stats { drawn: lists[0].stats.drawn, chunks: lists[0].stats.chunks, triangles: lists[0].stats.triangles, draw_calls: 0, refined: refine_n, ms: 0.0 };
        // Characters that act, posed for this moment (and the walker's own
        // body in third person).
        self.frame_no += 1;
        if o.avatar.is_some() && self.avatar_asset.is_none() {
            self.avatar_asset = Some(studio_world::build::character_asset(None, None, 1));
        }
        let mut perf_lists: Vec<Vec<(studio_world::world::Performer, ((u64, String, i64), f32))>> = Vec::new();
        for l in &lists {
            let mut pfs: Vec<studio_world::world::Performer> = l.performers.clone();
            if let Some(a) = &o.avatar {
                let (clip, speed) = if a.speed > 5.0 { ("run", 3.8) } else if a.speed > 0.3 { ("walk", 1.4) } else { ("idle", 1.0) };
                pfs.push(studio_world::world::Performer { object: usize::MAX, inst: studio_world::world::Instance { pos: a.pos, yaw: a.yaw, scale: 1.0, rank: 0 }, clip: clip.into(), clip_time: a.time * (a.speed.max(0.0) / speed).clamp(0.6, 2.5).max(if clip == "idle" { 1.0 } else { 0.6 }) });
            }
            let mut out = Vec::new();
            for pf in pfs {
                let asset = if pf.object == usize::MAX { self.avatar_asset.clone() } else { Some(w.assets[w.objects[pf.object].asset].clone()) };
                let Some(asset) = asset else { continue };
                let key = (asset.key, pf.clip.clone(), (pf.clip_time * 24.0).round() as i64);
                if let Some(p) = self.posed.get_mut(&key) {
                    p.used = self.frame_no;
                } else {
                    let (mesh, k) = match studio_world::character::posed(&asset, &pf.clip, pf.clip_time) {
                        Some(m) => (m, key.clone()),
                        None => ((*asset.lods[0]).clone(), (asset.key, String::new(), 0)),
                    };
                    let g = self.make_mesh_gpu(&mesh, asset.bounds.max[1], 0.0);
                    self.posed.insert(k.clone(), PosedGpu { mesh: g, used: self.frame_no });
                    out.push((pf, (k, 0.0)));
                    continue;
                }
                out.push((pf, (key, 0.0)));
            }
            perf_lists.push(out);
        }
        if self.posed.len() > 128 {
            let now = self.frame_no;
            self.posed.retain(|_, p| p.used + 90 > now);
        }
        // Meshes and chunks the lists need.
        let mut need: Vec<(u64, usize, Arc<Mesh>, f32, f32)> = Vec::new();
        let mut chunks = Vec::new();
        for l in &lists {
            for b in &l.batches {
                let a = &w.assets[b.asset];
                need.push((a.key, b.lod, a.lods[b.lod].clone(), a.bounds.max[1], a.sway));
            }
            chunks.extend(l.chunks.iter().copied());
        }
        for (key, lod, mesh, h, sway) in need {
            self.ensure_mesh(key, lod, &mesh, h, sway);
        }
        for (i, j, lod) in chunks {
            self.ensure_chunk(w, i, j, lod);
        }
        // Instances into one buffer.
        let mut starts: Vec<Vec<u32>> = Vec::new();
        let mut data: Vec<[f32; 8]> = Vec::new();
        let mut dyn_draws: Vec<Vec<((u64, String, i64), u32)>> = Vec::new();
        for (li, l) in lists.iter().enumerate() {
            let mut s = Vec::with_capacity(l.batches.len());
            for b in &l.batches {
                s.push(data.len() as u32);
                data.extend_from_slice(&b.instances);
            }
            starts.push(s);
            let mut d = Vec::new();
            for (pf, (key, _)) in &perf_lists[li] {
                d.push((key.clone(), data.len() as u32));
                data.push(pf.inst.raw());
            }
            dyn_draws.push(d);
        }
        let bytes = (data.len() as u64 * 32).max(32);
        if bytes > self.inst_cap {
            let cap = bytes.next_power_of_two();
            self.inst = self.ctx.device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt instances"), size: cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            self.inst_cap = cap;
        }
        if !data.is_empty() {
            self.ctx.queue.write_buffer(&self.inst, 0, bytemuck::cast_slice(&data));
        }
        // Uniforms.
        let lights = frame::pick_lights(w, view.position, &frustum);
        let env = self.env.as_ref().unwrap();
        let exposure = env.exposure;
        let mut fu = base_frame(&atmosphere, &look, env, &lights, width, height, o.time);
        fu.view_proj = vp;
        fu.view = vm;
        fu.inv_view_proj = frame::invert(&vp);
        fu.eye = [view.position[0], view.position[1], view.position[2], o.time];
        fu.view_eye = fu.eye;
        fu.jitter = [jitter_ndc[0], jitter_ndc[1], refine_n as f32, if want_ssao { look.ssao } else { 0.0 }];
        for (i, c) in casc.iter().enumerate() {
            fu.cascade_vp[i] = c.vp;
            fu.cascade_texel[i] = c.texel;
        }
        for i in 0..4 {
            fu.cascade_splits[i] = casc.get(i).map_or(1e9, |c| c.split);
        }
        fu.sun_dir[3] = if use_shadows { 1.0 } else { 0.0 };
        // Under water: thick tinted fog.
        if under {
            let wt = w.water.iter().find(|wt| view.position[0] >= wt.min[0] && view.position[0] <= wt.max[0] && view.position[2] >= wt.min[1] && view.position[2] <= wt.max[1] && view.position[1] < wt.level + 0.05).unwrap();
            fu.fog = [3.0 / wt.clarity.max(0.5) * 0.35, 0.0, 1.0, 0.0];
            let irr = (env.sh[0][0] * 0.282 + 0.15).max(0.02);
            fu.fog_colour = [wt.colour[0] * irr * 2.5, wt.colour[1] * irr * 2.5, wt.colour[2] * irr * 2.5, 1.0];
        }
        let mut frame_refl = None;
        if let (Some((rv, rvp, eye2)), Some(r)) = (&refl_view, &refl) {
            let mut f = fu;
            f.view = *rv;
            f.view_proj = *rvp;
            f.eye = [eye2[0], eye2[1], eye2[2], o.time];
            f.view_eye = fu.eye;
            f.clip = [0.0, 1.0, 0.0, -r.level + 0.03];
            f.flags[0] = 1.0;
            f.jitter[3] = 0.0;
            f.screen = [(width / 2) as f32, (height / 2) as f32, 2.0 / width as f32, 2.0 / height as f32];
            frame_refl = Some(f);
        }
        let mut frame_cascade = Vec::new();
        for c in &casc {
            let mut f = fu;
            f.view_proj = c.vp;
            f.eye[3] = o.time;
            frame_cascade.push(f);
        }
        let mut fu = fu;
        fu.flags[1] = look.saturation;
        fu.flags[2] = look.vignette;
        fu.flags[3] = if o.toon >= 2 { o.toon.min(3) as f32 } else { 0.0 };
        let _ = first_cascade;
        Ok(Prepared {
            lists,
            starts,
            dyn_draws,
            refl,
            cascades: casc,
            frame_main: fu,
            frame_refl,
            frame_cascade,
            water_visible,
            stats,
            exposure,
            bloom: look.bloom,
            refine_n,
            use_ssao: want_ssao && look.ssao > 0.0,
            use_shadows,
            saturation: look.saturation,
            vignette: look.vignette,
            fxaa: refine_n < 4,
        })
    }
}

fn reflect_matrix(level: f32) -> math::Mat4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 2.0 * level, 0.0, 1.0]]
}

fn base_frame(atmosphere: &studio_world::world::Atmosphere, look: &studio_world::world::Look, env: &EnvData, lights: &[LightU], width: u32, height: u32, time: f32) -> FrameU {
    let mut f: FrameU = bytemuck::Zeroable::zeroed();
    f.sun_dir = [env.light_to[0], env.light_to[1], env.light_to[2], 1.0];
    f.sun_colour = [env.light_colour[0], env.light_colour[1], env.light_colour[2], env.sun_sin.max(if env.sun_sin <= -0.04 { 0.01 } else { 0.0 })];
    f.fog = [atmosphere.fog, atmosphere.height_falloff, 0.0, 0.0];
    if let Some(c) = atmosphere.colour {
        f.fog_colour = [c[0], c[1], c[2], 1.0];
    }
    f.sky_misc = [env.clouds, frame::ENV_MIPS as f32, env.exposure, look.bloom];
    for k in 0..9 {
        f.sh[k] = [env.sh[k][0], env.sh[k][1], env.sh[k][2], 0.0];
    }
    f.screen = [width as f32, height as f32, 1.0 / width as f32, 1.0 / height as f32];
    f.clip = [0.0, 0.0, 0.0, 1.0];
    f.flags = [0.0, look.saturation, look.vignette, 0.0];
    f.counts[0] = lights.len() as u32;
    for (i, l) in lights.iter().enumerate().take(16) {
        f.lights[i] = *l;
    }
    f.eye[3] = time;
    f
}
