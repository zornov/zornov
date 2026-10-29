// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-frame readback: the blocking path (the exact frame, waiting for the
//! GPU) against the ring (the frame before, a non-blocking poll). First a
//! bare 1080p RGBA8 texture re-uploaded every frame, then the real-time
//! renderer. Prints a Markdown table of milliseconds per call and asserts
//! the ring waits once (the first frame) and never again.
//!
//! `cargo bench -p studio-render --bench readback` (add `-- --frames 30`).

use std::time::{Duration, Instant};

use studio_render::GpuContext;
use studio_render::realtime::readback::Readback;
use studio_render::realtime::{Options, Renderer, frame::View};
use studio_world::build::{NoLibrary, WorldSpec, build};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn row(path: &str, case: &str, ms: Vec<f64>, waits: u64) {
    let mean = ms.iter().sum::<f64>() / ms.len() as f64;
    println!("| {path} | {case} | {:.2} | {mean:.2} | {waits} |", median(ms));
}

fn bare(ctx: &GpuContext, frames: u64) {
    let (w, h) = (1920u32, 1080u32);
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let data = vec![0x7fu8; (w * h * 4) as usize];
    let produce = |n: u64| {
        let mut d = data.clone();
        d[0] = n as u8;
        ctx.queue.write_texture(
            tex.as_image_copy(),
            &d,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    };
    let ext = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let case = "1080p RGBA8 upload + readback";
    let mut out = Vec::new();

    let mut rb = Readback::new();
    let mut ms = Vec::new();
    for n in 0..frames {
        produce(n);
        let t = Instant::now();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let (buffer, padded) = rb.sync_target(&ctx.device, w, h, 4);
        enc.copy_texture_to_buffer(tex.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } }, ext);
        ctx.queue.submit([enc.finish()]);
        rb.read_sync(&ctx.device, &mut out).unwrap();
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    row("blocking", case, ms, rb.stats().blocking_waits);

    let mut rb = Readback::new();
    let mut ms = Vec::new();
    let mut fresh = 0;
    for n in 0..frames {
        produce(n);
        let t = Instant::now();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let (buffer, padded) = rb.ring_target(&ctx.device, w, h, 4).unwrap();
        enc.copy_texture_to_buffer(tex.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } }, ext);
        ctx.queue.submit([enc.finish()]);
        fresh += rb.ring_latest(&ctx.device, &mut out).unwrap().fresh as u64;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        // The app's own frame work between presents: lets the GPU finish.
        std::thread::sleep(Duration::from_millis(8));
    }
    let waits = rb.stats().blocking_waits;
    row("ring", &format!("{case} ({fresh}/{frames} fresh)"), ms, waits);
    assert!(waits <= 1, "the ring waited {waits} times in {frames} frames");
}

fn renderer(ctx: std::sync::Arc<GpuContext>, frames: u64) {
    let mut r = Renderer::new(ctx).expect("renderer");
    let w = build(&WorldSpec { preset: "forest".into(), size: Some(240.0), quality: "fast".into(), seed: Some(5), ..WorldSpec::default() }, &NoLibrary).unwrap();
    let mut v = View::from_pose(&studio_world::walk::Pose { pos: [w.spawn.position[0], w.spawn.position[1] + 1.62, w.spawn.position[2]], yaw: w.spawn.yaw, pitch: 0.0, fov: 60.0 });
    v.far = 800.0;
    let mut o = Options::new(640, 360);
    o.refine = false;
    let case = "world 640x360 render_srgb*";
    let frames = frames.min(20);
    r.render_srgb(&w, &v, &o).unwrap();

    let before = r.readback_stats();
    let mut ms = Vec::new();
    for i in 0..frames {
        v.yaw += 0.01 * i as f32;
        let t = Instant::now();
        r.render_srgb(&w, &v, &o).unwrap();
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    row("blocking", case, ms, r.readback_stats().blocking_waits - before.blocking_waits);

    let mut out = Vec::new();
    let mut ms = Vec::new();
    let before = r.readback_stats();
    for i in 0..frames {
        v.yaw += 0.01 * i as f32;
        let t = Instant::now();
        r.render_srgb_pipelined(&w, &v, &o, &mut out).unwrap();
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let waits = r.readback_stats().blocking_waits - before.blocking_waits;
    row("ring", case, ms, waits);
    println!("\nring waits over {frames} frames: {waits} (the first frame only is expected; more means the GPU fell more than 3 frames behind, as software Vulkan does)");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: u64 = args.iter().position(|a| a == "--frames").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(40);
    let Some(ctx) = GpuContext::new() else {
        println!("no GPU adapter: skipped");
        return;
    };
    println!("adapter: {}\n", ctx.adapter_name);
    println!("| path | case | median ms/call | mean ms/call | blocking waits |");
    println!("|---|---|---:|---:|---:|");
    bare(&ctx, frames);
    renderer(ctx, frames);
}
