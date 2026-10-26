// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU path must match the CPU reference within tolerance. These tests
//! run only when a GPU is available (and `STUDIO_RENDER_NO_GPU` is unset);
//! without one they print a note and pass.
//!
//! Tolerance: 1 % of the pixel's magnitude (at least 0.01 in linear light,
//! about 2/255 after sRGB encoding in the mid tones), with 0.5 % of pixels
//! allowed beyond it. The outliers are where a quantising effect
//! (posterise) or a non-separable blend flips at a boundary because the
//! GPU holds `f16` between passes.

mod common;

use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::mask::{MaskMode, MaskShape, MaskSpec};
use studio_render::scene::{Canvas, ClipDesc, MotionRef, Param, SECOND, SourceDesc, StaticScene};
use studio_render::{BlendMode, CpuCompositor, GpuCompositor, GpuContext, MediaKey, OutputDepth, OutputSettings};

const TOL: f32 = 0.01;
const OUTLIERS: f64 = 0.005;

fn gpu() -> Option<std::sync::Arc<GpuContext>> {
    let ctx = GpuContext::new();
    if ctx.is_none() {
        eprintln!("no GPU: agreement test skipped");
    }
    ctx
}

fn dump(name: &str, a: &studio_render::Frame, b: &studio_render::Frame) {
    if std::env::var_os("RENDER_DUMP").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/agreement");
        std::fs::create_dir_all(&dir).unwrap();
        a.save_png(&dir.join(format!("{name}_cpu.png"))).unwrap();
        b.save_png(&dir.join(format!("{name}_gpu.png"))).unwrap();
    }
}

#[test]
fn gpu_matches_cpu_on_the_busy_scene() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let scene = common::busy_scene();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    // Before, during and after the transition, at the canvas size and at
    // smaller preview sizes.
    for (t, size) in [(SECOND / 2, None), (2 * SECOND, None), (3 * SECOND, None), (SECOND / 2, Some((96, 54))), (2 * SECOND, Some((120, 90)))] {
        let a = cpu.render(&scene, t, size);
        let b = gpu.render(&scene, t, size);
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert!(a.is_finite() && b.is_finite());
        let (max, bad) = compare_relative(&a, &b, TOL);
        eprintln!("t={t} size={size:?}: max relative diff {max:.4}, {:.3}% beyond tolerance", bad * 100.0);
        dump(&format!("busy_{t}_{}", size.map(|s| s.0).unwrap_or(0)), &a, &b);
        assert!(bad <= OUTLIERS, "t={t} size={size:?}: {:.2}% of pixels differ by more than {TOL} (max {max})", bad * 100.0);
    }
}

/// One feature at a time over a gradient backdrop, so a regression names
/// its stage.
#[test]
fn gpu_matches_cpu_stage_by_stage() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let canvas = Canvas::new(192, 108, 30.0);
    let cases: Vec<(&str, Box<dyn Fn(&mut ClipDesc)>)> = vec![
        ("plain", Box::new(|_| {})),
        ("crop_scale", Box::new(|c| {
            c.placement.crop = [0.1, 0.0, 0.0, 0.15];
            c.placement.scale = [1.1, 1.1];
        })),
        ("flip", Box::new(|c| c.placement.flip_x = true)),
        ("rotate_small", Box::new(|c| {
            c.placement.scale = [0.6, 0.6];
            c.placement.rotation = 25.0;
        })),
        ("minify", Box::new(|c| c.placement.scale = [0.2, 0.2])),
        ("effect_shader", Box::new(|c| c.effects.push(common::effect("posterise", &[("levels", Param::Float(5.0))])))),
        ("effect_cpu_only", Box::new(|c| c.effects.push(common::effect("gaussian_blur", &[("radius", Param::Float(1.5))])))),
        ("effect_mixed_stack", Box::new(|c| {
            c.effects.push(common::effect("chromatic_aberration", &[]));
            c.effects.push(common::effect("box_blur", &[("radius", Param::Float(1.0))]));
            c.effects.push(common::effect("rgb_split", &[]));
        })),
        ("grade_exposure", Box::new(|c| c.grade = Some(common::grade(&[("exposure", Param::Float(0.4))])))),
        ("grade_sat_gain", Box::new(|c| c.grade = Some(common::grade(&[("saturation", Param::Float(1.3)), ("gain_luma", Param::Float(0.1))])))),
        ("grade_contrast_temp", Box::new(|c| c.grade = Some(common::grade(&[("contrast", Param::Float(0.3)), ("temperature", Param::Float(5600.0))])))),
        ("grade_cpu_only", Box::new(|c| c.grade = Some(common::grade(&[("sharpen", Param::Float(0.5)), ("exposure", Param::Float(0.2))])))),
        ("mask_ellipse", Box::new(|c| c.masks.push(MaskSpec { shape: MaskShape::Ellipse, feather: 0.15, size: [0.9, 0.8], ..Default::default() }))),
        ("mask_subtract", Box::new(|c| {
            c.masks.push(MaskSpec { shape: MaskShape::Ellipse, feather: 0.15, size: [0.9, 0.8], ..Default::default() });
            c.masks.push(MaskSpec { shape: MaskShape::Rectangle, mode: MaskMode::Subtract, size: [0.3, 0.3], centre: [0.7, 0.3], ..Default::default() });
        })),
        ("mask_line", Box::new(|c| c.masks.push(MaskSpec { shape: MaskShape::Line, rotation: -20.0, feather: 0.2, ..Default::default() }))),
        ("mask_star", Box::new(|c| c.masks.push(MaskSpec { shape: MaskShape::Star, feather: 0.05, ..Default::default() }))),
        ("mask_heart", Box::new(|c| c.masks.push(MaskSpec { shape: MaskShape::Heart, feather: 0.05, ..Default::default() }))),
        ("mask_path", Box::new(|c| c.masks.push(MaskSpec { shape: common::path_mask(), feather: 0.05, ..Default::default() }))),
        ("blend_screen", Box::new(|c| {
            c.blend = BlendMode::Screen;
            c.placement.opacity = 0.7;
        })),
        ("blend_overlay", Box::new(|c| c.blend = BlendMode::Overlay)),
        ("blend_hue", Box::new(|c| c.blend = BlendMode::Hue)),
        // Track mattes: a smaller, turned layer, so the canvas outside it
        // (cleared or kept) is compared too.
        ("matte_stencil_alpha", Box::new(|c| {
            c.blend = BlendMode::StencilAlpha;
            c.placement.scale = [0.5, 0.5];
            c.placement.rotation = 15.0;
        })),
        ("matte_silhouette_alpha", Box::new(|c| {
            c.blend = BlendMode::SilhouetteAlpha;
            c.placement.scale = [0.5, 0.5];
        })),
        ("matte_stencil_luma", Box::new(|c| {
            c.blend = BlendMode::StencilLuma;
            c.placement.scale = [0.6, 0.6];
            c.placement.opacity = 0.8;
        })),
        ("matte_silhouette_luma", Box::new(|c| {
            c.blend = BlendMode::SilhouetteLuma;
            c.placement.scale = [0.6, 0.6];
        })),
        ("puppet", Box::new(|c| c.warp = Some(common::puppet_warp()))),
        ("puppet_masked", Box::new(|c| {
            c.masks.push(MaskSpec { shape: MaskShape::Ellipse, feather: 0.05, size: [0.8, 0.7], ..Default::default() });
            c.warp = Some(common::puppet_warp());
            c.placement.scale = [0.8, 0.8];
        })),
        ("corner_pin", Box::new(|c| c.warp = Some(common::corner_warp()))),
        ("puppet_and_corner_pin", Box::new(|c| {
            let mut w = common::puppet_warp();
            w.corner_pin = common::corner_warp().corner_pin;
            c.warp = Some(w);
        })),
        ("mask_pins", Box::new(|c| c.masks.push(common::pinned_mask()))),
        ("motion_loop", Box::new(|c| c.motion.looping = Some(MotionRef { kind: "float".into(), duration: 2 * SECOND, params: Default::default() }))),
    ];
    for (name, f) in cases {
        let under = common::gradient(1, 0, 0, SECOND, [0.3, 0.1, 0.6, 1.0], [0.9, 0.7, 0.2, 1.0], 0.0);
        let mut clip = common::media(2, 1, 0, SECOND, MediaKey(1));
        f(&mut clip);
        let scene = StaticScene::new(canvas, SECOND).with_clip(under).with_clip(clip);
        let a = cpu.render(&scene, 0, None);
        let b = gpu.render(&scene, 0, None);
        let (max, bad) = compare_relative(&a, &b, TOL);
        eprintln!("{name:22} max {max:.4}  beyond {:.3}%", bad * 100.0);
        dump(name, &a, &b);
        assert!(bad <= OUTLIERS, "{name}: {:.2}% of pixels beyond tolerance (max {max})", bad * 100.0);
    }
    // A puppet on a shape: the mesh only covers what the shape draws.
    for (name, kind) in [("puppet_shape_star", "star"), ("puppet_shape_ellipse", "ellipse")] {
        let under = common::gradient(1, 0, 0, SECOND, [0.3, 0.1, 0.6, 1.0], [0.9, 0.7, 0.2, 1.0], 0.0);
        let mut clip = common::shape(2, 1, 0, SECOND, kind, [0.9, 0.9, 0.95, 1.0]);
        clip.warp = Some(common::puppet_warp());
        let scene = StaticScene::new(canvas, SECOND).with_clip(under).with_clip(clip);
        let (a, b) = (cpu.render(&scene, 0, None), gpu.render(&scene, 0, None));
        let (max, bad) = compare_relative(&a, &b, TOL);
        eprintln!("{name:22} max {max:.4}  beyond {:.3}%", bad * 100.0);
        dump(name, &a, &b);
        assert!(bad <= OUTLIERS, "{name}: {:.2}% of pixels beyond tolerance (max {max})", bad * 100.0);
    }
    // Ground shadows under a cut-out: contact and cast, falling several
    // ways, on a bent, flipped and smaller layer.
    for (name, angle, warp) in [("shadow_cast_right", 30.0, false), ("shadow_away_pinned", 250.0, true), ("shadow_towards_flipped", 110.0, false)] {
        let under = common::gradient(1, 0, 0, SECOND, [0.3, 0.1, 0.6, 1.0], [0.9, 0.7, 0.2, 1.0], 0.0);
        let mut clip = common::shape(2, 1, 0, SECOND, "star", [0.9, 0.9, 0.95, 1.0]);
        clip.placement.scale = [0.45, 0.45];
        clip.placement.flip_x = name.ends_with("flipped");
        if warp {
            clip.warp = Some(common::puppet_warp());
        }
        clip.shadow = Some(studio_render::shadow::ShadowDesc { angle, length: 0.8, softness: 0.06, ..Default::default() });
        let scene = StaticScene::new(canvas, SECOND).with_clip(under).with_clip(clip);
        let (a, b) = (cpu.render(&scene, 0, None), gpu.render(&scene, 0, None));
        let (max, bad) = compare_relative(&a, &b, TOL);
        eprintln!("{name:22} max {max:.4}  beyond {:.3}%", bad * 100.0);
        dump(name, &a, &b);
        assert!(bad <= OUTLIERS, "{name}: {:.2}% of pixels beyond tolerance (max {max})", bad * 100.0);
    }
    // Adjustment layer and transitions (shader-backed, CPU-only, unknown).
    let under = common::media(1, 0, 0, SECOND, MediaKey(1));
    let mut adj = ClipDesc::new(5, 3, 0, SECOND, SourceDesc::Adjustment);
    adj.grade = Some(common::grade(&[("exposure", Param::Float(-0.5))]));
    adj.effects.push(common::effect("posterise", &[("levels", Param::Float(8.0))]));
    adj.masks.push(MaskSpec { shape: MaskShape::Line, rotation: -20.0, feather: 0.2, ..Default::default() });
    let scene = StaticScene::new(canvas, SECOND).with_clip(under.clone()).with_clip(adj);
    let (a, b) = (cpu.render(&scene, 0, None), gpu.render(&scene, 0, None));
    let (max, bad) = compare_relative(&a, &b, TOL);
    eprintln!("{:22} max {max:.4}  beyond {:.3}%", "adjustment", bad * 100.0);
    assert!(bad <= OUTLIERS, "adjustment: {:.2}% beyond (max {max})", bad * 100.0);
    let right = common::media(2, 0, SECOND, SECOND, MediaKey(2));
    for kind in ["wipe", "dissolve", "slide", "iris", "no_such_transition"] {
        let scene = StaticScene::new(canvas, 2 * SECOND).with_clip(under.clone()).with_clip(right.clone()).with_transition(1, 2, kind, SECOND / 2, SECOND, Default::default());
        for progress in [0.0, 0.5, 1.0] {
            let t = SECOND / 2 + (progress * SECOND as f32) as i64 - if progress >= 1.0 { 1 } else { 0 };
            let (a, b) = (cpu.render(&scene, t, None), gpu.render(&scene, t, None));
            let (max, bad) = compare_relative(&a, &b, TOL);
            eprintln!("{:22} max {max:.4}  beyond {:.3}%", format!("transition {kind} {progress}"), bad * 100.0);
            assert!(bad <= OUTLIERS, "transition {kind} at {progress}: {:.2}% beyond (max {max})", bad * 100.0);
        }
    }
}

/// Every 3D case: perspective, depth sorting, cameras, lights, shadows,
/// extrusion, depth of field, 3D poses, mattes and nesting.
#[test]
fn gpu_matches_cpu_in_space() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let mut failures = Vec::new();
    for (name, scene, t) in common::space_scenes(Canvas::new(192, 108, 30.0)) {
        for size in [None, Some((96, 54))] {
            let a = cpu.render(&scene, t, size);
            let b = gpu.render(&scene, t, size);
            let (max, bad) = compare_relative(&a, &b, TOL);
            eprintln!("{name} {size:?}: max relative diff {max:.4}, {:.3}% beyond tolerance", bad * 100.0);
            dump(&format!("{name}_{}", size.map(|s| s.0).unwrap_or(0)), &a, &b);
            if bad > OUTLIERS {
                failures.push(format!("{name} {size:?}: {:.2}% of pixels differ by more than {TOL} (max {max})", bad * 100.0));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn gpu_output_encoding_matches_cpu() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let scene = common::busy_scene();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    for settings in [OutputSettings::default(), OutputSettings { premultiplied: true, dither: false, ..Default::default() }] {
        let a = cpu.render_encoded(&scene, SECOND, None, &settings, OutputDepth::Eight, 7);
        let b = gpu.render_encoded(&scene, SECOND, None, &settings, OutputDepth::Eight, 7);
        let (studio_render::EncodedPixels::Rgba8(pa), studio_render::EncodedPixels::Rgba8(pb)) = (&a.pixels, &b.pixels) else { panic!() };
        assert_eq!(pa.len(), pb.len());
        let bad = pa.iter().zip(pb).filter(|(x, y)| (**x as i32 - **y as i32).abs() > 2).count();
        eprintln!("8-bit {settings:?}: {bad} of {} bytes differ by more than 2", pa.len());
        assert!(bad as f64 / (pa.len() as f64) < 0.01, "{bad} bytes differ by more than 2 codes");
        // Dither is a deterministic hash: with it on, identical inputs give
        // identical codes, so most bytes still match exactly.
        let exact = pa.iter().zip(pb).filter(|(x, y)| x == y).count();
        assert!(exact as f64 / (pa.len() as f64) > 0.85, "only {exact} of {} bytes identical", pa.len());
    }
    let a = cpu.render_encoded(&scene, SECOND, None, &OutputSettings::default(), OutputDepth::Sixteen, 7);
    let b = gpu.render_encoded(&scene, SECOND, None, &OutputSettings::default(), OutputDepth::Sixteen, 7);
    let (studio_render::EncodedPixels::Rgba16(pa), studio_render::EncodedPixels::Rgba16(pb)) = (&a.pixels, &b.pixels) else { panic!() };
    let bad = pa.iter().zip(pb).filter(|(x, y)| (**x as i32 - **y as i32).abs() > 1024).count();
    assert!(bad as f64 / (pa.len() as f64) < 0.01, "{bad} 16-bit samples differ by more than 1024 (4 8-bit codes)");
}

#[test]
fn gpu_handles_hdr_source_and_nested_scene() {
    let Some(ctx) = gpu() else { return };
    use std::sync::Arc;
    use studio_render::colour_space::ColourDesc;
    use studio_render::source::{ProceduralSource, SourceRegistry, SourceSpec};
    use studio_render::yuv::{DecodedPicture, PixelData, Planes};
    // A 10-bit PQ BT.2020 ramp, so the convert shader's PQ decode, the
    // primaries matrix and the tone map all run.
    let reg = SourceRegistry::new();
    let src = ProceduralSource::new(64, 8, 30.0, 10, |_| {
        let desc = ColourDesc::from_h273(9, 16, 9, false, 10);
        let y: Vec<u16> = (0..64 * 8).map(|i| 64 + ((i % 64) as u16) * 14).collect();
        let u: Vec<u16> = (0..32 * 4).map(|i| 400 + (i % 32) as u16 * 8).collect();
        let v: Vec<u16> = (0..32 * 4).map(|i| 512 + (i / 32) as u16 * 20).collect();
        DecodedPicture { width: 64, height: 8, data: PixelData::Yuv { y: Planes::U16(y), u: Planes::U16(u), v: Planes::U16(v), chroma_width: 32, chroma_height: 4 }, colour: desc }
    });
    reg.register(MediaKey(9), SourceSpec::Procedural(src));
    let inner = StaticScene::new(Canvas::new(64, 8, 30.0), SECOND).with_clip(ClipDesc::new(1, 0, 0, SECOND, SourceDesc::Media { media: MediaKey(9), time: 0 }));
    let mut nested = ClipDesc::new(2, 0, 0, SECOND, SourceDesc::Nested { scene: Arc::new(inner), time: 0 });
    nested.placement.rotation = 10.0;
    let outer = StaticScene::new(Canvas::new(96, 48, 30.0), SECOND).with_clip(nested);
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let a = cpu.render(&outer, 0, None);
    let b = gpu.render(&outer, 0, None);
    let (max, bad) = compare_relative(&a, &b, TOL);
    eprintln!("hdr nested: max {max:.4}, {:.3}% beyond tolerance", bad * 100.0);
    assert!(bad <= OUTLIERS, "max {max}, {:.2}% beyond", bad * 100.0);
    // After tone mapping nothing exceeds the curve's ceiling.
    assert!(a.pixels.iter().all(|p| p[0] <= 2.0 + 1e-3));
}

/// Effects moved from the CPU to shaders must look the same, at several
/// moments (they change over time).
#[test]
fn ported_effects_match_their_cpu_originals() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let canvas = Canvas::new(192, 108, 30.0);
    let cases: Vec<(&str, Vec<(&str, Param)>)> = vec![
        ("film", vec![("dust", Param::Float(0.8)), ("scratches", Param::Float(0.9)), ("grain", Param::Float(0.6)), ("seed", Param::Float(7.0))]),
        ("camera_shake", vec![("amount", Param::Float(4.0)), ("rotation", Param::Float(3.0)), ("seed", Param::Float(3.0))]),
        ("camera_shake", vec![("amount", Param::Float(6.0)), ("auto_zoom", Param::Bool(false))]),
        ("water", vec![("ripple", Param::Float(1.5)), ("shimmer", Param::Float(1.2)), ("direction", Param::Float(30.0)), ("seed", Param::Float(4.0))]),
    ];
    for (kind, params) in cases {
        for t in [0, SECOND / 3, SECOND / 2 + 17_000] {
            let under = common::gradient(1, 0, 0, SECOND, [0.3, 0.1, 0.6, 1.0], [0.9, 0.7, 0.2, 1.0], 0.0);
            let mut clip = common::media(2, 1, 0, SECOND, MediaKey(1));
            clip.effects.push(common::effect(kind, &params));
            let scene = StaticScene::new(canvas, SECOND).with_clip(under).with_clip(clip);
            let (a, b) = (cpu.render(&scene, t, None), gpu.render(&scene, t, None));
            let (max, bad) = compare_relative(&a, &b, TOL);
            eprintln!("{:22} t={t:7} max {max:.4}  beyond {:.3}%", kind, bad * 100.0);
            dump(&format!("ported_{kind}_{t}"), &a, &b);
            assert!(bad <= OUTLIERS, "{kind} at {t}: {:.2}% beyond tolerance (max {max})", bad * 100.0);
        }
    }
}

/// Particles are simulated and binned on the CPU and drawn by a shader on
/// the GPU: both draws agree for every kind of sprite, on a picture and on
/// an empty layer.
#[test]
fn particles_match_on_cpu_and_gpu() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let canvas = Canvas::new(256, 144, 30.0);
    for preset in ["Rain", "Snow", "Falling leaves", "Fireflies", "Sparks", "Mist", "Splashes", "Bubbles", "Confetti", "Magic dust", "Stars"] {
        for (t, over_picture) in [(SECOND / 3, true), (2 * SECOND + 40_000, false)] {
            let mut clip = common::solid(2, 1, 0, 4 * SECOND, [0.0, 0.0, 0.0, 0.0]);
            clip.effects.push(common::effect("particles", &[("preset", Param::Choice(preset.into())), ("size", Param::Float(2.5)), ("seed", Param::Float(3.0))]));
            let mut scene = StaticScene::new(canvas, 4 * SECOND);
            if over_picture {
                scene = scene.with_clip(common::gradient(1, 0, 0, 4 * SECOND, [0.1, 0.12, 0.2, 1.0], [0.4, 0.35, 0.3, 1.0], 0.0));
            }
            let scene = scene.with_clip(clip);
            let (a, b) = (cpu.render(&scene, t, None), gpu.render(&scene, t, None));
            let (max, bad) = compare_relative(&a, &b, TOL);
            eprintln!("{preset:16} t={t:8} max {max:.4}  beyond {:.3}%", bad * 100.0);
            dump(&format!("particles_{preset}_{t}"), &a, &b);
            assert!(bad <= OUTLIERS, "{preset} at {t}: {:.2}% beyond tolerance (max {max})", bad * 100.0);
        }
    }
}

/// A cut-out: a half-size soft matte moving with the frame index, over a
/// cropped, turned media layer with a mask and an effect after it, kept and
/// inverted.
#[test]
fn gpu_matches_cpu_with_a_cutout_matte() {
    use studio_render::scene::{MatteDesc, matte_key};
    use studio_render::source::{ProceduralSource, SourceSpec};
    use studio_render::yuv::DecodedPicture;
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let key = matte_key("/tmp/agreement.smatte");
    reg.register(
        key,
        SourceSpec::Procedural(ProceduralSource::new(64, 36, 30.0, 300, |i| {
            let (w, h) = (64usize, 36usize);
            let cx = 20.0 + (i % 30) as f32;
            let px: Vec<u8> = (0..w * h)
                .flat_map(|k| {
                    let (x, y) = ((k % w) as f32 + 0.5, (k / w) as f32 + 0.5);
                    let d = ((x - cx).powi(2) + (y - 18.0).powi(2)).sqrt();
                    let a = ((14.0 - d) / 3.0).clamp(0.0, 1.0);
                    let b = (a * 255.0).round() as u8;
                    [b, b, b, 255]
                })
                .collect();
            DecodedPicture::from_rgba8(w as u32, h as u32, px)
        })),
    );
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let canvas = Canvas::new(192, 108, 30.0);
    for invert in [false, true] {
        for t in [0, SECOND / 3] {
            let under = common::gradient(1, 0, 0, SECOND, [0.3, 0.1, 0.6, 1.0], [0.9, 0.7, 0.2, 1.0], 0.0);
            let mut clip = common::media(2, 1, 0, SECOND, MediaKey(1));
            clip.source = SourceDesc::Media { media: MediaKey(1), time: t };
            clip.matte = Some(MatteDesc { media: key, invert });
            clip.placement.crop = [0.05, 0.0, 0.1, 0.05];
            clip.placement.rotation = 12.0;
            clip.placement.scale = [1.2, 1.2];
            clip.masks.push(common::mask(MaskShape::Ellipse, 0.1));
            clip.effects.push(common::effect("gaussian_blur", &[("radius", Param::Float(2.0))]));
            let scene = StaticScene::new(canvas, SECOND).with_clip(under).with_clip(clip);
            let (a, b) = (cpu.render(&scene, t, None), gpu.render(&scene, t, None));
            let (max, bad) = compare_relative(&a, &b, TOL);
            eprintln!("matte invert={invert} t={t}: max {max:.4}, beyond {:.3}%", bad * 100.0);
            dump(&format!("matte_{invert}_{t}"), &a, &b);
            assert!(bad <= OUTLIERS, "matte invert={invert} t={t}: {:.2}% beyond tolerance (max {max})", bad * 100.0);
        }
    }
    // The matte really cuts: alone on the (black) canvas, the layer shows
    // in the disc and not away from it.
    let mut clip = common::media(2, 1, 0, SECOND, MediaKey(1));
    clip.matte = Some(MatteDesc { media: key, invert: false });
    let scene = StaticScene::new(canvas, SECOND).with_clip(clip);
    let a = cpu.render(&scene, 0, None);
    let at = |x: u32, y: u32| {
        let p = a.pixels[(y * a.width + x) as usize];
        p[0] + p[1] + p[2]
    };
    assert!(at(60, 54) > 0.05, "inside the disc: {}", at(60, 54));
    assert!(at(180, 10) < 1e-4, "outside: {}", at(180, 10));
}

/// Shot matching in the grade (colour statistics moved to a reference's)
/// looks the same on both paths, alone and with a basic grade after it.
#[test]
fn gpu_matches_cpu_with_a_shot_match() {
    let Some(ctx) = gpu() else { return };
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    let canvas = Canvas::new(192, 108, 30.0);
    let mut kv: Vec<(String, Param)> = vec![("match_strength".into(), Param::Float(0.8))];
    for (k, v) in [("match_src_mean", [0.55, 0.01, 0.02]), ("match_src_std", [0.2, 0.03, 0.04]), ("match_ref_mean", [0.62, -0.01, 0.05]), ("match_ref_std", [0.25, 0.05, 0.03])] {
        for (i, x) in v.iter().enumerate() {
            kv.push((format!("{k}_{i}"), Param::Float(*x)));
        }
    }
    for extra in [vec![], vec![("contrast".to_owned(), Param::Float(0.3)), ("saturation".to_owned(), Param::Float(1.2))]] {
        let mut clip = common::media(2, 1, 0, SECOND, MediaKey(1));
        let all: Vec<(&str, Param)> = kv.iter().chain(&extra).map(|(k, v)| (k.as_str(), v.clone())).collect();
        clip.grade = Some(common::grade(&all));
        let scene = StaticScene::new(canvas, SECOND).with_clip(clip);
        let (a, b) = (cpu.render(&scene, SECOND / 4, None), gpu.render(&scene, SECOND / 4, None));
        let (max, bad) = compare_relative(&a, &b, TOL);
        eprintln!("shot match (+{} more): max {max:.4}, beyond {:.3}%", extra.len(), bad * 100.0);
        assert!(bad <= OUTLIERS, "{:.2}% beyond tolerance (max {max})", bad * 100.0);
    }
}
