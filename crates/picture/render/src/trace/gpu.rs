// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The path tracer on the GPU: the scene's arrays packed into a handful of
//! storage buffers, `trace.wgsl` run once per sample (each run adds a
//! sample to every pixel's running sum), the sums read back. A device of
//! its own, opened once; `None` without a GPU (the CPU tracer takes over),
//! or when `STUDIO_RENDER_NO_GPU` is set.

use super::scene::Scene;
use super::{Passes, TraceSettings};
use crate::frame::Frame;
use std::sync::{Mutex, OnceLock};
use wgpu::util::DeviceExt;

struct Tracer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
}

fn tracer() -> Option<&'static Mutex<Tracer>> {
    static T: OnceLock<Option<Mutex<Tracer>>> = OnceLock::new();
    T.get_or_init(|| {
        if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
        let limits = adapter.limits();
        if limits.max_storage_buffers_per_shader_stage < 8 {
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("studio-trace"), required_features: wgpu::Features::empty(), required_limits: limits, ..Default::default() })).ok()?;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("trace"), source: wgpu::ShaderSource::Wgsl(source().into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("trace"), layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        if let Some(e) = pollster::block_on(scope.pop()) {
            eprintln!("studio-render: path tracer unavailable on the GPU: {e}");
            return None;
        }
        Some(Mutex::new(Tracer { device, queue, pipeline }))
    })
    .as_ref()
}

/// The tracer's WGSL (for validation in the tests).
pub fn source() -> &'static str {
    include_str!("../gpu/shaders/trace.wgsl")
}

fn bytes_of<T: Copy>(v: &[T]) -> &[u8] {
    // Plain arrays of f32 / u32 groups.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// Renders `scene` with `samples` rays per pixel, or `None` without a GPU.
pub fn render(s: &Scene, samples: u32, settings: &TraceSettings) -> Option<Passes> {
    let lock = tracer()?;
    let t = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (w, h) = (s.width, s.height);
    let pixels = (w * h) as usize;
    // Geometry: positions, then end positions, normals, texture coords.
    let n = s.pos0.len() as u32;
    let mut geo: Vec<[f32; 4]> = Vec::with_capacity(s.pos0.len() * 4 + 1);
    geo.extend_from_slice(&s.pos0);
    geo.extend_from_slice(&s.pos1);
    geo.extend_from_slice(&s.nrm);
    geo.extend(s.uv.iter().map(|u| [u[0], u[1], 0.0, 0.0]));
    if geo.is_empty() {
        geo.push([0.0; 4]);
    }
    let mut tris: Vec<[u32; 4]> = s.tris.iter().zip(&s.tri_obj).map(|(t, o)| [t[0], t[1], t[2], (t[3] & 0xffff) | (o << 16)]).collect();
    if tris.is_empty() {
        tris.push([0; 4]);
    }
    let mut nodes: Vec<[f32; 4]> = s.nodes.iter().flat_map(|n| [n[0], n[1]]).collect();
    if nodes.is_empty() {
        nodes.push([0.0; 4]);
        nodes.push([0.0; 4]);
    }
    // Materials, lights, emitters, volumes, environment, one after another.
    let mut data: Vec<[f32; 4]> = Vec::new();
    let mats_off = data.len() as u32;
    for m in &s.mats {
        data.extend_from_slice(m);
    }
    let lights_off = data.len() as u32;
    for l in &s.lights {
        data.extend_from_slice(l);
    }
    let emit_off = data.len() as u32;
    data.extend_from_slice(&s.emit);
    let vol_off = data.len() as u32;
    let mut words: Vec<u32> = s.tex.clone();
    let cells_off = words.len() as u32;
    words.extend_from_slice(&s.vol_cells);
    for v in &s.volumes {
        data.extend_from_slice(&v.to_box);
        data.push([f32::from_bits(v.dims[0]), f32::from_bits(v.dims[1]), f32::from_bits(v.dims[2]), f32::from_bits(v.offset)]);
        data.push([v.colour[0], v.colour[1], v.colour[2], v.density]);
        data.push([v.fire, v.lo[0], v.lo[1], v.lo[2]]);
        data.push([v.hi[0], v.hi[1], v.hi[2], 0.0]);
    }
    let env_off = data.len() as u32;
    data.extend_from_slice(&s.env);
    if data.is_empty() {
        data.push([0.0; 4]);
    }
    if words.is_empty() {
        words.push(0);
    }
    let mut cdf: Vec<f32> = s.env_rows.clone();
    let cols_off = cdf.len() as u32;
    cdf.extend_from_slice(&s.env_cols);
    if cdf.is_empty() {
        cdf.push(0.0);
    }
    let flags = (s.moving as u32) | ((s.background as u32) << 1) | ((!s.env.is_empty() as u32) << 2);
    let c = &s.cam;
    let mut uniform: [[u32; 4]; 12] = [[0; 4]; 12];
    let fl = |v: [f32; 4]| v.map(f32::to_bits);
    uniform[0] = fl([c.pos[0], c.pos[1], c.pos[2], c.zoom]);
    uniform[1] = fl([c.right[0], c.right[1], c.right[2], c.centre[0]]);
    uniform[2] = fl([c.down[0], c.down[1], c.down[2], c.centre[1]]);
    uniform[3] = fl([c.fwd[0], c.fwd[1], c.fwd[2], c.lens]);
    uniform[4] = fl([c.focus, s.extent, s.haze, s.env_turn.to_radians()]);
    uniform[5] = fl([s.haze_colour[0], s.haze_colour[1], s.haze_colour[2], s.emit_total]);
    uniform[6] = [w, h, s.lights.len() as u32, s.emit.len() as u32];
    uniform[7] = [s.volumes.len() as u32, s.env_w, s.env_h, flags];
    uniform[8] = [n, 2 * n, 3 * n, mats_off];
    uniform[9] = [lights_off, emit_off, vol_off, env_off];
    uniform[10] = [cells_off, cols_off, s.nodes.len() as u32, 0];
    uniform[11] = fl([settings.bounces as f32, settings.clamp, if settings.shutter > 0.0 { 1.0 } else { 0.0 }, 0.0]);
    let dev = &t.device;
    let storage = |label: &str, bytes: &[u8]| dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage: wgpu::BufferUsages::STORAGE });
    let geo_b = storage("geo", bytes_of(&geo));
    let tris_b = storage("tris", bytes_of(&tris));
    let nodes_b = storage("nodes", bytes_of(&nodes));
    let data_b = storage("data", bytes_of(&data));
    let words_b = storage("words", bytes_of(&words));
    let cdf_b = storage("cdf", bytes_of(&cdf));
    let accum_b = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("accum"), contents: &vec![0u8; pixels * 16], usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC });
    let aux_init: Vec<[f32; 4]> = (0..pixels * 2).map(|i| if i % 2 == 0 { [0.0, 0.0, 0.0, 1e30] } else { [0.0; 4] }).collect();
    let aux_b = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("aux"), contents: bytes_of(&aux_init), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC });
    let uni_b = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("trace uniform"), contents: bytes_of(&uniform), usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST });
    let layout = t.pipeline.get_bind_group_layout(0);
    let entries = [&uni_b, &geo_b, &tris_b, &nodes_b, &data_b, &words_b, &cdf_b, &accum_b, &aux_b];
    let entries: Vec<wgpu::BindGroupEntry> = entries.iter().enumerate().map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }).collect();
    let scope = dev.push_error_scope(wgpu::ErrorFilter::Validation);
    let group = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("trace"), layout: &layout, entries: &entries });
    for sample in 0..samples {
        uniform[10][3] = sample;
        t.queue.write_buffer(&uni_b, 0, bytes_of(&uniform));
        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("trace") });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("trace"), timestamp_writes: None });
            pass.set_pipeline(&t.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        t.queue.submit([enc.finish()]);
        // Keep the queue short so a long render never trips the driver's
        // watchdog, and other GPU work gets turns.
        if sample % 4 == 3 {
            let _ = dev.poll(wgpu::PollType::wait_indefinitely());
        }
    }
    if let Some(e) = pollster::block_on(scope.pop()) {
        eprintln!("studio-render: GPU path trace failed: {e}");
        return None;
    }
    let read = |buf: &wgpu::Buffer, size: u64| -> Option<Vec<u8>> {
        let staging = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("trace readback"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("trace readback") });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        t.queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = dev.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let bytes = slice.get_mapped_range().ok()?.to_vec();
        staging.unmap();
        Some(bytes)
    };
    let acc = read(&accum_b, (pixels * 16) as u64)?;
    let aux = read(&aux_b, (pixels * 32) as u64)?;
    let f = |b: &[u8], i: usize| f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    let k = 1.0 / samples as f32;
    let beauty = Frame { width: w, height: h, pixels: (0..pixels).map(|i| [f(&acc, i * 4) * k, f(&acc, i * 4 + 1) * k, f(&acc, i * 4 + 2) * k, f(&acc, i * 4 + 3) * k]).collect() };
    let mut depth = Vec::with_capacity(pixels);
    let mut normal = Vec::with_capacity(pixels);
    let mut albedo = Vec::with_capacity(pixels);
    let mut id = Vec::with_capacity(pixels);
    for i in 0..pixels {
        let b = i * 8;
        let d = f(&aux, b + 3);
        depth.push(if d >= 1e29 { f32::INFINITY } else { d });
        normal.push([f(&aux, b), f(&aux, b + 1), f(&aux, b + 2)]);
        albedo.push([f(&aux, b + 4), f(&aux, b + 5), f(&aux, b + 6)]);
        let obj = f(&aux, b + 7).to_bits();
        id.push(if obj == 0 { 0 } else { s.ids.get(obj as usize - 1).copied().unwrap_or(0) });
    }
    Some(Passes { beauty, depth, normal, albedo, id })
}
