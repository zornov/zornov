// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The air and look pass on the GPU: the picture, its depth and the packed
//! parameters go into storage buffers, `atmosphere.wgsl` runs its stages
//! as compute dispatches, the result is read back. A device of its own,
//! opened once; [`run`] returns `false` without a GPU (or with
//! `STUDIO_RENDER_NO_GPU` set) and the CPU takes over.

use super::cpu::BLOOM_LEVELS;
use super::{Picture, Plan};
use std::sync::{Mutex, OnceLock};
use wgpu::util::DeviceExt;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    sprites: wgpu::ComputePipeline,
    reflect: wgpu::ComputePipeline,
    air: wgpu::ComputePipeline,
    down: wgpu::ComputePipeline,
    blur: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
}

/// Whether the pass can run on a GPU here.
pub fn available() -> bool {
    gpu().is_some()
}

/// The shader's source (for validation in the tests).
pub fn source() -> &'static str {
    include_str!("atmosphere.wgsl")
}

fn gpu() -> Option<&'static Mutex<Gpu>> {
    static G: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
    G.get_or_init(|| {
        if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
        let limits = adapter.limits();
        if limits.max_storage_buffers_per_shader_stage < 8 {
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("studio-atmosphere"), required_features: wgpu::Features::empty(), required_limits: limits, ..Default::default() })).ok()?;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("atmosphere"), source: wgpu::ShaderSource::Wgsl(source().into()) });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let entries = [
            storage(0, true),
            storage(1, true),
            storage(2, true),
            storage(3, true),
            storage(4, false),
            storage(5, false),
            storage(6, false),
            storage(7, false),
            wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("atmosphere"), entries: &entries });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("atmosphere"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipe = |entry: &str| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry), layout: Some(&pl), module: &module, entry_point: Some(entry), compilation_options: Default::default(), cache: None });
        let g = Gpu { sprites: pipe("sprites"), reflect: pipe("reflect_pass"), air: pipe("air"), down: pipe("bloom_down"), blur: pipe("bloom_blur"), finish: pipe("finish"), device, queue, layout };
        if let Some(e) = pollster::block_on(scope.pop()) {
            eprintln!("studio-render: air and look pass unavailable on the GPU: {e}");
            return None;
        }
        Some(Mutex::new(g))
    })
    .as_ref()
}

fn bytes_of<T: Copy>(v: &[T]) -> &[u8] {
    // Plain arrays of f32 / u32 groups.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// Floats in the bloom pyramid (as vec4 slots).
fn pyramid_len(w: usize, h: usize) -> usize {
    let (mut lw, mut lh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let mut total = 0;
    for _ in 0..BLOOM_LEVELS {
        total += lw * lh;
        lw = lw.div_ceil(2).max(1);
        lh = lh.div_ceil(2).max(1);
    }
    total
}

/// Runs the plan on the GPU. `false` when there is none (nothing was changed).
pub fn run(plan: &Plan, pic: &Picture, colour: &mut [[f32; 4]]) -> bool {
    let Some(lock) = gpu() else { return false };
    let g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (w, h) = (pic.width as u32, pic.height as u32);
    let n = pic.width * pic.height;
    if n == 0 || colour.len() < n {
        return true;
    }
    let dev = &g.device;
    let init = |label: &str, bytes: &[u8], usage: wgpu::BufferUsages| dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage });
    let st = wgpu::BufferUsages::STORAGE;
    // Metering on what is there already, when no stage comes before it.
    let early = plan.stages.sprites || plan.stages.reflect || plan.stages.air;
    let pre;
    let plan = if plan.metered() && !early {
        pre = plan.exposed(super::meter(&colour[..n], plan.params[super::P_FOG + 5][3]));
        &pre
    } else {
        plan
    };
    let params_b = init("params", bytes_of(&plan.params), st | wgpu::BufferUsages::COPY_DST);
    let shadows: Vec<f32> = if plan.shadows.is_empty() { vec![0.0; 4] } else { plan.shadows.clone() };
    let shadow_b = init("shadows", bytes_of(&shadows), st);
    let depth: Vec<f32> = pic.depth.iter().map(|d| if d.is_finite() { *d } else { 1.0e30 }).collect();
    let depth_b = init("depth", bytes_of(&depth), st);
    let surf: Vec<[f32; 4]> = if pic.surface.is_empty() { vec![[0.0; 4]] } else { pic.surface.to_vec() };
    let surf_b = init("surface", bytes_of(&surf), st);
    let a_b = init("colour a", bytes_of(&colour[..n]), st | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST);
    let b_b = init("colour b", &vec![0u8; n * 16], st | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST);
    let pyr = pyramid_len(pic.width, pic.height).max(1);
    let bl_b = init("bloom", &vec![0u8; pyr * 16], st);
    let bt_b = init("bloom tmp", &vec![0u8; pyr * 16], st);
    let uni = |x: u32, y: u32| init("stage", bytes_of(&[x, y, 0u32, 0u32]), wgpu::BufferUsages::UNIFORM);
    let group = |u: &wgpu::Buffer| {
        let entries = [&params_b, &shadow_b, &depth_b, &surf_b, &a_b, &b_b, &bl_b, &bt_b, u];
        let entries: Vec<wgpu::BindGroupEntry> = entries.iter().enumerate().map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }).collect();
        dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("atmosphere"), layout: &g.layout, entries: &entries })
    };
    let scope = dev.push_error_scope(wgpu::ErrorFilter::Validation);
    let zero = uni(0, 0);
    let base = group(&zero);
    let stages = plan.stages;
    let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("atmosphere") });
    let (gx, gy) = (w.div_ceil(8), h.div_ceil(8));
    if stages.sprites {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sprites"), timestamp_writes: None });
        pass.set_bind_group(0, &base, &[]);
        pass.set_pipeline(&g.sprites);
        pass.dispatch_workgroups(gx, gy, 1);
    }
    if stages.reflect {
        // Wet surfaces see the picture as it is after the sources are drawn.
        enc.copy_buffer_to_buffer(&a_b, 0, &b_b, 0, (n * 16) as u64);
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("reflect"), timestamp_writes: None });
        pass.set_bind_group(0, &base, &[]);
        pass.set_pipeline(&g.reflect);
        pass.dispatch_workgroups(gx, gy, 1);
    }
    if stages.air {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("air"), timestamp_writes: None });
        pass.set_bind_group(0, &base, &[]);
        pass.set_pipeline(&g.air);
        pass.dispatch_workgroups(gx, gy, 1);
    }
    // Metering after the sources, the air and the weather are in: the
    // picture so far comes back, the exposure is worked out and written,
    // and the rest runs in a second submission.
    if plan.metered() && early {
        let size = (n * 16) as u64;
        let mid = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("atmosphere meter"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        enc.copy_buffer_to_buffer(&a_b, 0, &mid, 0, size);
        g.queue.submit([enc.finish()]);
        let slice = mid.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = dev.poll(wgpu::PollType::wait_indefinitely());
        if !matches!(rx.recv(), Ok(Ok(()))) {
            return false;
        }
        let Ok(range) = slice.get_mapped_range() else { return false };
        let seen: Vec<[f32; 4]> = range.chunks_exact(16).map(|b| std::array::from_fn(|k| f32::from_le_bytes([b[k * 4], b[k * 4 + 1], b[k * 4 + 2], b[k * 4 + 3]]))).collect();
        drop(range);
        mid.unmap();
        let metered = plan.exposed(super::meter(&seen, plan.params[super::P_FOG + 5][3]));
        g.queue.write_buffer(&params_b, (super::P_FOG + 5) as u64 * 16, bytes_of(&metered.params[super::P_FOG + 5..super::P_FOG + 6]));
        enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("atmosphere 2") });
    }
    // Bloom: down the pyramid, then each level blurred both ways.
    let mut keep: Vec<(wgpu::Buffer, wgpu::BindGroup)> = Vec::new();
    let mut level_dims = Vec::new();
    if stages.bloom {
        let mut dims = (pic.width.div_ceil(2).max(1) as u32, pic.height.div_ceil(2).max(1) as u32);
        for k in 0..BLOOM_LEVELS as u32 {
            level_dims.push(dims);
            for dir in 0..3u32 {
                let u = uni(k, dir.saturating_sub(1));
                let gr = group(&u);
                keep.push((u, gr));
            }
            dims = (dims.0.div_ceil(2).max(1), dims.1.div_ceil(2).max(1));
        }
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("bloom"), timestamp_writes: None });
        for (k, d) in level_dims.iter().enumerate() {
            pass.set_pipeline(&g.down);
            pass.set_bind_group(0, &keep[k * 3].1, &[]);
            pass.dispatch_workgroups(d.0.div_ceil(8), d.1.div_ceil(8), 1);
        }
        for (k, d) in level_dims.iter().enumerate() {
            pass.set_pipeline(&g.blur);
            pass.set_bind_group(0, &keep[k * 3 + 1].1, &[]);
            pass.dispatch_workgroups(d.0.div_ceil(8), d.1.div_ceil(8), 1);
            pass.set_bind_group(0, &keep[k * 3 + 2].1, &[]);
            pass.dispatch_workgroups(d.0.div_ceil(8), d.1.div_ceil(8), 1);
        }
    }
    if stages.finish {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("finish"), timestamp_writes: None });
        pass.set_bind_group(0, &base, &[]);
        pass.set_pipeline(&g.finish);
        pass.dispatch_workgroups(gx, gy, 1);
    }
    let result = if stages.finish { &b_b } else { &a_b };
    let size = (n * 16) as u64;
    let staging = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("atmosphere readback"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    enc.copy_buffer_to_buffer(result, 0, &staging, 0, size);
    g.queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = dev.poll(wgpu::PollType::wait_indefinitely());
    if let Some(e) = pollster::block_on(scope.pop()) {
        eprintln!("studio-render: air and look pass failed on the GPU: {e}");
        return false;
    }
    if !matches!(rx.recv(), Ok(Ok(()))) {
        return false;
    }
    let Ok(range) = slice.get_mapped_range() else { return false };
    let bytes = range.to_vec();
    drop(range);
    staging.unmap();
    for (i, px) in colour[..n].iter_mut().enumerate() {
        let f = |k: usize| f32::from_le_bytes([bytes[i * 16 + k * 4], bytes[i * 16 + k * 4 + 1], bytes[i * 16 + k * 4 + 2], bytes[i * 16 + k * 4 + 3]]);
        *px = [f(0), f(1), f(2), f(3)];
    }
    true
}
