// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Frame readback: the ring hands back frame N-1 without waiting, rows are
//! unpadded exactly (also for widths that are not 256-byte aligned), the
//! exact path returns the frame asked for, and buffers are pooled.

use std::time::Duration;

use studio_render::GpuContext;
use studio_render::realtime::readback::{RING, Readback, padded_row};
use studio_render::realtime::{Options, Renderer, frame::View};
use studio_world::build::{NoLibrary, WorldSpec, build};

const BPP: u32 = 4;

/// Pixel (x, y) of frame `n`: a gradient in x and y, a colour step per frame.
fn pixel(n: u32, x: u32, y: u32) -> [u8; 4] {
    [(n * 40 % 256) as u8, (x * 3 % 256) as u8, (y * 50 % 256) as u8, 255]
}

fn frame_bytes(n: u32, w: u32, h: u32) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).flat_map(move |x| pixel(n, x, y))).collect()
}

fn upload(ctx: &GpuContext, tex: &wgpu::Texture, n: u32, w: u32, h: u32) {
    ctx.queue.write_texture(
        tex.as_image_copy(),
        &frame_bytes(n, w, h),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * BPP), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

fn texture(ctx: &GpuContext, w: u32, h: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("readback test"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn copy_ring(ctx: &GpuContext, rb: &mut Readback, tex: &wgpu::Texture, w: u32, h: u32) {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let (buffer, padded) = rb.ring_target(&ctx.device, w, h, BPP).unwrap();
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
}

fn copy_sync(ctx: &GpuContext, rb: &mut Readback, tex: &wgpu::Texture, w: u32, h: u32) {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let (buffer, padded) = rb.sync_target(&ctx.device, w, h, BPP);
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
}

#[test]
fn rows_are_padded_to_the_copy_alignment() {
    assert_eq!(padded_row(67, 4), 512);
    assert_eq!(padded_row(64, 4), 256);
    assert_eq!(padded_row(1, 8), 256);
}

#[test]
fn the_ring_returns_the_frame_before_with_exact_pixels() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    // 67 px is not 256-byte aligned (268 B a row); 64 px is.
    for (w, h) in [(67u32, 5u32), (64, 3)] {
        let tex = texture(&ctx, w, h);
        let mut rb = Readback::new();
        let mut out = Vec::new();
        for n in 0..8u32 {
            upload(&ctx, &tex, n, w, h);
            copy_ring(&ctx, &mut rb, &tex, w, h);
            let got = rb.ring_latest(&ctx.device, &mut out).unwrap();
            assert!(got.fresh && (got.width, got.height) == (w, h));
            let expect = if n == 0 { 0 } else { n - 1 };
            assert_eq!(got.lag, if n == 0 { 0 } else { 1 }, "frame {n} at {w} px");
            assert_eq!(out.len(), (w * h * BPP) as usize);
            assert_eq!(out, frame_bytes(expect, w, h), "frame {n} at {w} px shows frame {expect}");
            // Spot checks with literal values: the last pixel of the last row.
            let last = &out[out.len() - 4..];
            assert_eq!(last, &[(expect * 40 % 256) as u8, ((w - 1) * 3) as u8, ((h - 1) * 50) as u8, 255]);
            // A frame's worth of pacing, so the previous frame has finished.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn the_exact_path_returns_the_frame_asked_for() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let (w, h) = (67u32, 5u32);
    let tex = texture(&ctx, w, h);
    let mut rb = Readback::new();
    let mut out = Vec::new();
    for n in 0..6u32 {
        upload(&ctx, &tex, n, w, h);
        copy_sync(&ctx, &mut rb, &tex, w, h);
        rb.read_sync(&ctx.device, &mut out).unwrap();
        assert_eq!(out, frame_bytes(n, w, h), "exact frame {n}");
    }
    assert_eq!(rb.stats().buffers_created, 1, "one staging buffer for every frame of a size");
}

#[test]
fn the_ring_never_waits_once_warm_and_pools_its_buffers() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let (w, h) = (67u32, 5u32);
    let tex = texture(&ctx, w, h);
    let mut rb = Readback::new();
    let mut out = Vec::new();
    upload(&ctx, &tex, 0, w, h);
    copy_ring(&ctx, &mut rb, &tex, w, h);
    rb.ring_latest(&ctx.device, &mut out).unwrap();
    assert_eq!(rb.stats().blocking_waits, 1, "the first frame is waited for, once");
    let warm = rb.stats();
    for n in 1..30u32 {
        upload(&ctx, &tex, n, w, h);
        copy_ring(&ctx, &mut rb, &tex, w, h);
        rb.ring_latest(&ctx.device, &mut out).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let s = rb.stats();
    assert_eq!(s.blocking_waits, warm.blocking_waits, "no blocking wait in the steady state");
    assert!(s.polls > warm.polls);
    assert_eq!(s.buffers_created, RING as u64, "buffers are reused, not created per frame");
    // A new size rebuilds the ring and waits for its first frame again.
    let tex2 = texture(&ctx, 33, 4);
    upload(&ctx, &tex2, 3, 33, 4);
    copy_ring(&ctx, &mut rb, &tex2, 33, 4);
    let got = rb.ring_latest(&ctx.device, &mut out).unwrap();
    assert!(got.fresh && got.lag == 0);
    assert_eq!(out, frame_bytes(3, 33, 4));
    assert_eq!(rb.stats().buffers_created, 2 * RING as u64);
    assert_eq!(rb.stats().blocking_waits, warm.blocking_waits + 1);
}

#[test]
fn the_pipelined_renderer_matches_the_exact_one_one_frame_late() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut r = Renderer::new(ctx).unwrap();
    let w = build(&WorldSpec { preset: "meadow".into(), size: Some(60.0), quality: "fast".into(), seed: Some(5), ..WorldSpec::default() }, &NoLibrary).unwrap();
    let v = View::from_pose(&studio_world::walk::Pose { pos: [w.spawn.position[0], w.spawn.position[1] + 1.62, w.spawn.position[2]], yaw: w.spawn.yaw, pitch: 0.0, fov: 60.0 });
    let mut o = Options::new(67, 40);
    o.refine = false;
    let exact = r.render_srgb(&w, &v, &o).unwrap().0;
    let waits = r.readback_stats().blocking_waits;
    let mut out = Vec::new();
    for n in 0..6 {
        let (got, _) = r.render_srgb_pipelined(&w, &v, &o, &mut out).unwrap();
        assert_eq!((got.width, got.height), (exact.width, exact.height));
        assert!(got.fresh);
        assert_eq!(got.lag, if n == 0 { 0 } else { 1 }, "frame {n}");
        assert_eq!(out, exact.rgba, "frame {n}: a still camera draws the same picture");
        // A GPU that keeps up with the frame rate (software Vulkan is slower).
        let _ = r.context().device.poll(wgpu::PollType::wait_indefinitely());
    }
    assert_eq!(r.readback_stats().blocking_waits, waits + 1, "only the first pipelined frame waited");
}
