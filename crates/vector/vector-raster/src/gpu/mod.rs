// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU renderer: the shared preparation, coarse tile binning on the
//! CPU ([`encode`]), then WGSL compute for every pixel (`fine.wgsl`) and
//! for layer effects (`filter.wgsl`). Prepasses (effect layers, masks,
//! patterns) render on the GPU into an image buffer the later passes
//! sample, so a frame is one submission.

mod encode;

use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::paint::Image;
use crate::prepare::{self, PFilter, Prepass};
use crate::{Pixmap, RenderOptions, Scene};

/// Why the GPU could not draw a scene (the caller falls back to [`crate::render`]).
#[derive(Debug, Clone, PartialEq)]
pub enum GpuError {
    /// More nested isolated groups in one place than the shader's stack holds.
    TooDeep,
    /// A buffer would exceed what this device can bind.
    TooLarge { bytes: u64, limit: u64 },
    /// The device reported an error.
    Device(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            GpuError::TooDeep => write!(f, "groups nest deeper than the GPU renderer's stack"),
            GpuError::TooLarge { bytes, limit } => write!(f, "a {bytes}-byte buffer is over this GPU's {limit}-byte limit"),
            GpuError::Device(e) => write!(f, "GPU error: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Config {
    origin: [i32; 2],
    size: [u32; 2],
    tiles_x: u32,
    dither: u32,
    paint_off: u32,
    has_bg: u32,
    bg: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FilterParams {
    size: [u32; 2],
    a: [i32; 2],
    f: [f32; 2],
    mode: u32,
    extra: u32,
    color: [f32; 4],
}

struct Pipelines {
    fine_rgba8: wgpu::ComputePipeline,
    fine_f32: wgpu::ComputePipeline,
    blur: wgpu::ComputePipeline,
    shift_alpha: wgpu::ComputePipeline,
    dilate: wgpu::ComputePipeline,
    combine: wgpu::ComputePipeline,
    pack: wgpu::ComputePipeline,
}

/// A device and the vector pipelines. Cheap to share (`Arc`).
pub struct GpuRaster {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pipes: Pipelines,
}

fn compute(device: &wgpu::Device, module: &wgpu::ShaderModule, entry: &str) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

impl GpuRaster {
    /// Opens the best adapter. `None` without a usable GPU, or when
    /// `STUDIO_RENDER_NO_GPU` is set.
    pub fn new() -> Option<Arc<GpuRaster>> {
        if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
        let name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("studio-vector-raster"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()?;
        GpuRaster::from_device(device, queue, name)
    }

    /// Uses an existing device (the application shares its own). `None`
    /// if the shaders do not build on it.
    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue, adapter_name: String) -> Option<Arc<GpuRaster>> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let fine = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("vector fine"), source: wgpu::ShaderSource::Wgsl(include_str!("fine.wgsl").into()) });
        let filt = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("vector filter"), source: wgpu::ShaderSource::Wgsl(include_str!("filter.wgsl").into()) });
        let pipes = Pipelines {
            fine_rgba8: compute(&device, &fine, "fine_rgba8"),
            fine_f32: compute(&device, &fine, "fine_f32"),
            blur: compute(&device, &filt, "blur"),
            shift_alpha: compute(&device, &filt, "shift_alpha"),
            dilate: compute(&device, &filt, "dilate"),
            combine: compute(&device, &filt, "combine"),
            pack: compute(&device, &filt, "pack"),
        };
        if let Some(e) = pollster::block_on(scope.pop()) {
            eprintln!("studio-vector-raster: GPU pipelines unavailable: {e}");
            return None;
        }
        Some(Arc::new(GpuRaster { device, queue, adapter_name, pipes }))
    }

    fn storage(&self, label: &str, bytes: &[u8]) -> wgpu::Buffer {
        // Bindings may not be empty.
        let pad;
        let contents = if bytes.len() < 16 {
            pad = [0u8; 16];
            &pad[..]
        } else {
            bytes
        };
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage: wgpu::BufferUsages::STORAGE })
    }

    fn scratch(&self, label: &str, bytes: u64, extra: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: bytes.max(16).div_ceil(4) * 4, usage: wgpu::BufferUsages::STORAGE | extra, mapped_at_creation: false })
    }

    fn check(&self, bytes: u64) -> Result<(), GpuError> {
        let limit = self.device.limits().max_storage_buffer_binding_size as u64;
        if bytes > limit {
            return Err(GpuError::TooLarge { bytes, limit });
        }
        Ok(())
    }

    fn dispatch(&self, enc: &mut wgpu::CommandEncoder, pipe: &wgpu::ComputePipeline, bindings: &[(u32, &wgpu::Buffer)], x: u32, y: u32) {
        let layout = pipe.get_bind_group_layout(0);
        let entries: Vec<wgpu::BindGroupEntry> = bindings.iter().map(|(b, buf)| wgpu::BindGroupEntry { binding: *b, resource: buf.as_entire_binding() }).collect();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries });
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(x, y, 1);
    }

    /// Draws `scene` on the GPU.
    pub fn render(&self, scene: &Scene, opts: &RenderOptions) -> Result<Pixmap, GpuError> {
        let prep = prepare::prepare(scene, opts);
        let (w, h) = (opts.width, opts.height);
        if w == 0 || h == 0 {
            return Ok(Pixmap { width: w, height: h, data: Vec::new() });
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);

        // Image buffer layout: prepasses first, then user images with mipmaps.
        let mut descs: Vec<[u32; 8]> = Vec::new();
        let mut words: u64 = 0;
        let mut prepass_desc = Vec::with_capacity(prep.prepasses.len());
        for p in &prep.prepasses {
            let r = p.region();
            prepass_desc.push(descs.len() as u32);
            descs.push([words as u32, r.width() as u32, r.height() as u32, 1, r.x0 as u32, r.y0 as u32, 0, 0]);
            words += r.width() as u64 * r.height() as u64;
        }
        let mut user: Vec<(Image, u32)> = Vec::new();
        let mut seen: HashMap<usize, u32> = HashMap::new();
        let mut user_fn = |img: &Image| -> u32 {
            let key = Arc::as_ptr(&img.data) as usize;
            if let Some(&d) = seen.get(&key) {
                return d;
            }
            let d = descs.len() as u32;
            let mips = img.mips();
            descs.push([words as u32, img.width, img.height, 1 + mips.len() as u32, 0, 0, 0, 0]);
            words += img.width as u64 * img.height as u64;
            for m in mips {
                descs.push([words as u32, m.width, m.height, 0, 0, 0, 0, 0]);
                words += m.width as u64 * m.height as u64;
            }
            seen.insert(key, d);
            user.push((img.clone(), d));
            d
        };
        let mut table = encode::ImageTable { prepass_desc: &prepass_desc, user: &mut user_fn };
        let mut encoded_pre = Vec::with_capacity(prep.prepasses.len());
        for p in &prep.prepasses {
            encoded_pre.push(match p {
                Prepass::Scene { region, root, .. } => Some(encode::encode(root, *region, &mut table)),
                Prepass::Ready { .. } => None,
            });
        }
        let root = encode::encode(&prep.root, prep.region, &mut table);
        drop(table);
        if root.depth_exceeded || encoded_pre.iter().flatten().any(|e| e.depth_exceeded) {
            return Err(GpuError::TooDeep);
        }
        self.check(words * 4)?;
        let images = self.scratch("vector images", words * 4, wgpu::BufferUsages::COPY_DST);
        for p in &prep.prepasses {
            if let Prepass::Ready { pixels, .. } = p {
                let i = prep.prepasses.iter().position(|q| std::ptr::eq(q, p)).unwrap_or(0);
                let off = descs[prepass_desc[i] as usize][0] as u64 * 4;
                let bytes: Vec<u8> = pixels
                    .iter()
                    .flat_map(|c| {
                        let a = crate::to_u8(c[3], 0.0);
                        [crate::to_u8(c[0], 0.0).min(a), crate::to_u8(c[1], 0.0).min(a), crate::to_u8(c[2], 0.0).min(a), a]
                    })
                    .collect();
                self.queue.write_buffer(&images, off, &bytes);
            }
        }
        for (img, d) in &user {
            let off = descs[*d as usize][0] as u64 * 4;
            self.queue.write_buffer(&images, off, &img.data);
            for (k, m) in img.mips().iter().enumerate() {
                let off = descs[*d as usize + 1 + k][0] as u64 * 4;
                self.queue.write_buffer(&images, off, &m.data);
            }
        }
        let desc_bytes: Vec<u8> = descs.iter().flat_map(|d| d.iter().flat_map(|v| v.to_le_bytes())).collect();
        let descs_buf = self.storage("vector image descriptors", &desc_bytes);

        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("vector frame") });
        for (i, p) in prep.prepasses.iter().enumerate() {
            let (Prepass::Scene { region, filters, .. }, Some(e)) = (p, &encoded_pre[i]) else { continue };
            let (pw, ph) = (region.width() as u32, region.height() as u32);
            let px_bytes = pw as u64 * ph as u64 * 16;
            self.check(px_bytes)?;
            let a = self.scratch("layer a", px_bytes, wgpu::BufferUsages::empty());
            self.fine(&mut enc, e, *region, false, None, &images, &descs_buf, &a, false)?;
            let a = self.filters(&mut enc, a, pw, ph, filters, px_bytes)?;
            let prm = self.params(FilterParams { size: [pw, ph], a: [0; 2], f: [0.0; 2], mode: descs[prepass_desc[i] as usize][0], extra: 0, color: [0.0; 4] });
            self.dispatch(&mut enc, &self.pipes.pack, &[(0, &prm), (1, &a), (5, &images)], pw.div_ceil(16), ph.div_ceil(16));
        }
        let out_bytes = w as u64 * h as u64 * 4;
        self.check(out_bytes)?;
        let out = self.scratch("vector out", out_bytes, wgpu::BufferUsages::COPY_SRC);
        self.fine(&mut enc, &root, prep.region, opts.dither, prep.background, &images, &descs_buf, &out, true)?;
        let read = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("vector readback"), size: out_bytes, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        enc.copy_buffer_to_buffer(&out, 0, &read, 0, out_bytes);
        self.queue.submit([enc.finish()]);
        let slice = read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        if let Some(e) = pollster::block_on(oom.pop()) {
            return Err(GpuError::Device(e.to_string()));
        }
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Device(e.to_string()));
        }
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(GpuError::Device(e.to_string())),
            Err(e) => return Err(GpuError::Device(e.to_string())),
        }
        let data = slice.get_mapped_range().map_err(|e| GpuError::Device(format!("{e:?}")))?.to_vec();
        read.unmap();
        Ok(Pixmap { width: w, height: h, data })
    }

    fn params(&self, p: FilterParams) -> wgpu::Buffer {
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("filter params"), contents: bytemuck::bytes_of(&p), usage: wgpu::BufferUsages::UNIFORM })
    }

    #[allow(clippy::too_many_arguments)]
    fn fine(
        &self,
        enc: &mut wgpu::CommandEncoder,
        e: &encode::Encoded,
        region: prepare::IRect,
        dither: bool,
        bg: Option<[f32; 4]>,
        images: &wgpu::Buffer,
        descs: &wgpu::Buffer,
        out: &wgpu::Buffer,
        rgba8: bool,
    ) -> Result<(), GpuError> {
        let ubytes: &[u8] = bytemuck::cast_slice(&e.udata);
        let fbytes: &[u8] = bytemuck::cast_slice(&e.fdata);
        self.check(ubytes.len() as u64)?;
        self.check(fbytes.len() as u64)?;
        let cfg = Config {
            origin: [region.x0, region.y0],
            size: [region.width() as u32, region.height() as u32],
            tiles_x: e.tiles_x,
            dither: dither as u32,
            paint_off: e.paint_off,
            has_bg: bg.is_some() as u32,
            bg: bg.unwrap_or([0.0; 4]),
        };
        let cfg = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("vector config"), contents: bytemuck::bytes_of(&cfg), usage: wgpu::BufferUsages::UNIFORM });
        let ud = self.storage("vector udata", ubytes);
        let fd = self.storage("vector fdata", fbytes);
        let (pipe, slot) = if rgba8 { (&self.pipes.fine_rgba8, 5) } else { (&self.pipes.fine_f32, 6) };
        self.dispatch(enc, pipe, &[(0, &cfg), (1, &ud), (2, &fd), (3, images), (4, descs), (slot, out)], e.tiles_x, e.tiles_y);
        Ok(())
    }

    /// Runs a layer's effects; returns the buffer holding the result.
    fn filters(&self, enc: &mut wgpu::CommandEncoder, mut a: wgpu::Buffer, w: u32, h: u32, filters: &[PFilter], bytes: u64) -> Result<wgpu::Buffer, GpuError> {
        if filters.is_empty() {
            return Ok(a);
        }
        let (gx, gy) = (w.div_ceil(16), h.div_ceil(16));
        let mut b = self.scratch("layer b", bytes, wgpu::BufferUsages::empty());
        let mut c = self.scratch("layer c", bytes, wgpu::BufferUsages::empty());
        let mut d = self.scratch("layer d", bytes, wgpu::BufferUsages::empty());
        let base = FilterParams { size: [w, h], a: [0; 2], f: [0.0; 2], mode: 0, extra: 0, color: [0.0; 4] };
        let blur = |enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, tmp: &wgpu::Buffer, sx: f32, sy: f32| {
            // x into tmp, y back into src.
            for (dir, s, from, to) in [(0, sx, src, tmp), (1, sy, tmp, src)] {
                let k = gauss(s);
                let kb = self.storage("blur kernel", bytemuck::cast_slice(&k));
                let prm = self.params(FilterParams { a: [dir, k.len() as i32 - 1], ..base });
                self.dispatch(enc, &self.pipes.blur, &[(0, &prm), (1, from), (2, to), (4, &kb)], gx, gy);
            }
        };
        let dilate = |enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, tmp: &wgpu::Buffer, r: f32| {
            if r <= 0.0 {
                return;
            }
            let ri = r.ceil().min(256.0) as i32;
            let frac = r - (ri as f32 - 1.0);
            for (dir, from, to) in [(0, src, tmp), (1, tmp, src)] {
                let prm = self.params(FilterParams { a: [dir, ri], f: [frac, 0.0], ..base });
                self.dispatch(enc, &self.pipes.dilate, &[(0, &prm), (1, from), (2, to)], gx, gy);
            }
        };
        for f in filters {
            match *f {
                PFilter::Blur { sx, sy } => blur(enc, &a, &b, sx, sy),
                PFilter::Opacity { amount } => {
                    let prm = self.params(FilterParams { mode: 2, color: [0.0, 0.0, 0.0, amount], ..base });
                    self.dispatch(enc, &self.pipes.combine, &[(0, &prm), (1, &a), (2, &b), (3, &c)], gx, gy);
                    std::mem::swap(&mut a, &mut b);
                }
                PFilter::DropShadow { .. } | PFilter::OuterGlow { .. } => {
                    let (dx, dy, sigma, color, spread) = match *f {
                        PFilter::DropShadow { dx, dy, sigma, color, spread } => (dx, dy, sigma, color, spread),
                        PFilter::OuterGlow { sigma, color, spread } => (0.0, 0.0, sigma, color, spread),
                        _ => unreachable!(),
                    };
                    let (ix, iy) = (dx.floor(), dy.floor());
                    let prm = self.params(FilterParams { a: [ix as i32, iy as i32], f: [dx - ix, dy - iy], extra: 0, ..base });
                    self.dispatch(enc, &self.pipes.shift_alpha, &[(0, &prm), (1, &a), (2, &c)], gx, gy);
                    dilate(enc, &c, &d, spread);
                    blur(enc, &c, &d, sigma, sigma);
                    let prm = self.params(FilterParams { mode: 0, color, ..base });
                    self.dispatch(enc, &self.pipes.combine, &[(0, &prm), (1, &a), (2, &b), (3, &c)], gx, gy);
                    std::mem::swap(&mut a, &mut b);
                }
                PFilter::InnerShadow { .. } | PFilter::InnerGlow { .. } => {
                    let (dx, dy, sigma, color) = match *f {
                        PFilter::InnerShadow { dx, dy, sigma, color } => (dx, dy, sigma, color),
                        PFilter::InnerGlow { sigma, color } => (0.0, 0.0, sigma, color),
                        _ => unreachable!(),
                    };
                    let (ix, iy) = (dx.floor(), dy.floor());
                    let prm = self.params(FilterParams { a: [ix as i32, iy as i32], f: [dx - ix, dy - iy], extra: 1, ..base });
                    self.dispatch(enc, &self.pipes.shift_alpha, &[(0, &prm), (1, &a), (2, &c)], gx, gy);
                    blur(enc, &c, &d, sigma, sigma);
                    let prm = self.params(FilterParams { mode: 1, color, ..base });
                    self.dispatch(enc, &self.pipes.combine, &[(0, &prm), (1, &a), (2, &b), (3, &c)], gx, gy);
                    std::mem::swap(&mut a, &mut b);
                }
            }
        }
        // Keep the spare buffers alive until the commands are recorded.
        let _ = (&mut c, &mut d);
        Ok(a)
    }
}

/// Normalised Gaussian weights for deviation `s` (centre first), radius 3s.
fn gauss(s: f32) -> Vec<f32> {
    if s <= 0.05 {
        return vec![1.0];
    }
    let r = (3.0 * s).ceil().max(1.0) as usize;
    let mut k: Vec<f32> = (0..=r).map(|i| (-(i as f32 * i as f32) / (2.0 * s * s)).exp()).collect();
    let sum: f32 = k[0] + 2.0 * k[1..].iter().sum::<f32>();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}
