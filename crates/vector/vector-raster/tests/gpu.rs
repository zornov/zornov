// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU renderer draws what the CPU renderer draws: same coverage, same
//! paints, blends, clips, masks and effects, to within rounding. Skipped
//! (with a note) on machines without a usable GPU.
#![cfg(feature = "gpu")]

mod common;

use std::sync::Arc;

use common::*;
use studio_vector_raster::gpu::GpuRaster;
use studio_vector_raster::kurbo::{Affine, BezPath, Circle, Point, Rect, Shape};
use studio_vector_raster::*;

fn gpu() -> Option<Arc<GpuRaster>> {
    let g = GpuRaster::new();
    if g.is_none() {
        eprintln!("no GPU: skipping");
    }
    g
}

fn compare(name: &str, g: &GpuRaster, scene: &Scene, opts: &RenderOptions, mean_limit: f64, off_limit: f64) {
    let cpu = render(scene, opts);
    let t = std::time::Instant::now();
    let gpu = g.render(scene, opts).expect("gpu render");
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let dir = out_dir();
    write_png(&dir.join(format!("gpu-{name}.png")), &gpu);
    write_png(&dir.join(format!("gpu-{name}.cpu.png")), &cpu);
    let d = diff(&gpu, &cpu);
    eprintln!("{name}: {d:?} ({ms:.1} ms on the GPU)");
    assert!(d.mean <= mean_limit && d.off <= off_limit, "{name}: GPU and CPU differ: {d:?}");
}

#[test]
fn gpu_matches_cpu_on_the_reference_art() {
    let Some(g) = gpu() else { return };
    for (name, svg) in [
        ("shapes", include_str!("art/shapes.svg")),
        ("strokes", include_str!("art/strokes.svg")),
        ("gradients", include_str!("art/gradients.svg")),
        ("groups", include_str!("art/groups.svg")),
        ("clip_mask", include_str!("art/clip_mask.svg")),
        ("pattern_blur", include_str!("art/pattern_blur.svg")),
    ] {
        let (scene, w, h) = svg_scene(svg);
        let mut o = RenderOptions::new(w * 2, h * 2).with_transform(Affine::scale(2.0));
        o.dither = false;
        // Prepass images are 8-bit on the GPU (float on the CPU), so effect
        // and mask layers may differ by a level here and there.
        compare(name, &g, &scene, &o, 0.2, 0.002);
    }
}

#[test]
fn gpu_matches_cpu_on_every_paint_and_effect() {
    let Some(g) = gpu() else { return };
    let mut s = Scene::new();
    let r = |x: f64, y: f64| Rect::new(x, y, x + 90.0, y + 90.0).to_path(0.0);
    s.fill(r(5.0, 5.0), Affine::IDENTITY, FillRule::NonZero, Paint::gradient(Gradient::conic((50.0, 50.0), 0.3, vec![Stop::new(0.0, Color::rgb(1.0, 0.0, 0.0)), Stop::new(0.5, Color::rgb(0.0, 1.0, 0.0)), Stop::new(1.0, Color::rgb(1.0, 0.0, 0.0))])));
    let ff = FreeformGradient {
        points: vec![
            FreeformPoint { at: Point::new(110.0, 10.0), color: Color::rgb(1.0, 0.8, 0.0), spread: 1.0 },
            FreeformPoint { at: Point::new(190.0, 30.0), color: Color::rgb(0.1, 0.3, 0.9), spread: 2.0 },
            FreeformPoint { at: Point::new(140.0, 90.0), color: Color::rgb(0.9, 0.1, 0.4), spread: 1.0 },
        ],
        transform: Affine::IDENTITY,
    };
    s.fill(r(105.0, 5.0), Affine::IDENTITY, FillRule::NonZero, Paint::Freeform(Arc::new(ff)));
    let colors = [Color::rgb(1.0, 0.0, 0.0), Color::rgb(0.0, 1.0, 0.0), Color::rgb(0.0, 0.0, 1.0), Color::WHITE];
    let mut patch = CoonsPatch::rect(Rect::new(205.0, 5.0, 295.0, 95.0), colors);
    patch.points[1] = Point::new(230.0, -10.0);
    s.fill(Circle::new((250.0, 50.0), 44.0).to_path(0.001), Affine::IDENTITY, FillRule::NonZero, Paint::Mesh(Arc::new(MeshGradient { patches: vec![patch], transform: Affine::IDENTITY })));
    // A raster image, shrunk (mipmaps) and enlarged (bicubic).
    let mut px = Vec::new();
    for y in 0..64u32 {
        for x in 0..64u32 {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            px.extend_from_slice(&if on { [230, 60, 40, 255] } else { [30, 40, 60, 255] });
        }
    }
    let img = Image::from_straight(64, 64, px).unwrap();
    for (x, scale, q) in [(5.0, 0.6, ImageQuality::Bilinear), (105.0, 3.0, ImageQuality::Bicubic)] {
        let ip = ImagePaint { image: img.clone(), transform: Affine::translate((x, 105.0)) * Affine::scale(scale), extend: Extend::Repeat, quality: q, opacity: 0.9 };
        s.fill(r(x, 105.0), Affine::IDENTITY, FillRule::NonZero, Paint::Image(Arc::new(ip)));
    }
    // Effects.
    let mut art = Scene::new();
    let mut star = BezPath::new();
    for i in 0..10 {
        let a = i as f64 * std::f64::consts::PI / 5.0 - std::f64::consts::FRAC_PI_2;
        let rr = if i % 2 == 0 { 40.0 } else { 18.0 };
        let p = Point::new(250.0 + rr * a.cos(), 150.0 + rr * a.sin());
        if i == 0 { star.move_to(p) } else { star.line_to(p) }
    }
    star.close_path();
    art.fill(star, Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgb(0.95, 0.7, 0.1)));
    let layer = Layer::new()
        .with_filter(Filter::DropShadow { dx: 4.0, dy: 6.0, sigma: 3.0, color: Color::rgba(0.0, 0.0, 0.0, 0.6), spread: 1.0 })
        .with_filter(Filter::InnerGlow { sigma: 4.0, color: Color::rgba(1.0, 1.0, 1.0, 0.8) })
        .with_filter(Filter::OuterGlow { sigma: 5.0, color: Color::rgba(1.0, 0.3, 0.0, 0.7), spread: 0.0 });
    s.layer(layer, art);
    // Blend modes inside an isolated, clipped, masked group.
    let mut inner = Scene::new();
    for (i, m) in BlendMode::ALL.iter().enumerate() {
        let c = Circle::new((10.0 + (i % 6) as f64 * 16.0, 210.0 + (i / 6) as f64 * 16.0), 11.0);
        inner.fill(c.to_path(0.001), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgba(0.2, 0.6 + i as f32 * 0.02, 0.9, 0.8))).blended(*m, 0.9);
    }
    let mut mask = Scene::new();
    mask.fill(r(0.0, 195.0), Affine::IDENTITY, FillRule::NonZero, Paint::gradient(Gradient::linear((0.0, 0.0), (100.0, 0.0), vec![Stop::new(0.0, Color::WHITE), Stop::new(1.0, Color::BLACK)])));
    let mut bg = Scene::new();
    bg.fill(Rect::new(0.0, 195.0, 100.0, 260.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgb(0.8, 0.3, 0.3)));
    bg.layer(Layer::new().with_clip(Circle::new((50.0, 225.0), 40.0).to_path(0.001), Affine::IDENTITY, FillRule::EvenOdd).with_mask(Mask { scene: mask, kind: MaskKind::Luminance, invert: false }).with_opacity(0.9), inner);
    s.layer(Layer::new().with_blend(BlendMode::Multiply), bg);
    let mut o = RenderOptions::new(300, 260).with_background(Color::rgb(0.97, 0.97, 0.95));
    o.dither = false;
    compare("paints-effects", &g, &s, &o, 0.25, 0.003);
    // And at 8K-class scale (4× here keeps the test quick): still agrees.
    let mut o4 = RenderOptions::new(1200, 1040).with_transform(Affine::scale(4.0)).with_background(Color::WHITE);
    o4.dither = false;
    compare("paints-effects-4x", &g, &s, &o4, 0.25, 0.003);
}
