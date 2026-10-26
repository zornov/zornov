// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Colour-managed decoding and viewing: camera log input transforms decode
//! to the right scene-linear values on the CPU, the GPU agrees, and view
//! transforms are applied once to the finished frame.

mod common;

use std::sync::Arc;

use studio_colour::LogCurve;
use studio_colour::camera::Gamut;
use studio_render::colour_space::{ColourDesc, InputColour, Matrix, SourcePrimaries, SourceTransfer};
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::scene::{Canvas, ClipDesc, SECOND, SourceDesc, StaticScene};
use studio_render::source::{ProceduralSource, SourceRegistry, SourceSpec};
use studio_render::view::ViewTransform;
use studio_render::yuv::{DecodedPicture, PixelData, Planes};
use studio_render::{CpuCompositor, GpuCompositor, GpuContext, MediaKey, OutputDepth, OutputSettings, OutputSpace};

/// A 16-bit full-range RGB picture (planes hold G, B, R) whose code
/// values are `codes` (0..1), one per column.
fn rgb16(codes: &[[f32; 3]], rows: u32, colour: ColourDesc) -> DecodedPicture {
    let w = codes.len() as u32;
    let q = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    let mut y = Vec::new();
    let mut u = Vec::new();
    let mut v = Vec::new();
    for _ in 0..rows {
        for c in codes {
            y.push(q(c[1]));
            u.push(q(c[2]));
            v.push(q(c[0]));
        }
    }
    DecodedPicture { width: w, height: rows, data: PixelData::Yuv { y: Planes::U16(y), u: Planes::U16(u), v: Planes::U16(v), chroma_width: w, chroma_height: rows }, colour }
}

fn rgb_desc() -> ColourDesc {
    ColourDesc { primaries: SourcePrimaries::Bt709, transfer: SourceTransfer::Linear, matrix: Matrix::Rgb, full_range: true, bit_depth: 16 }
}

#[test]
fn camera_grey_decodes_to_eighteen_percent() {
    for curve in LogCurve::ALL {
        let input = InputColour::parse(curve.slug(), None).unwrap();
        let desc = input.apply(rgb_desc());
        let code = curve.encode(0.18);
        let pic = rgb16(&[[code; 3]], 1, desc);
        let f = pic.to_frame(true);
        let p = f.pixels[0];
        for c in 0..3 {
            assert!((p[c] - 0.18).abs() < 0.003, "{curve:?}: {p:?}");
        }
    }
}

#[test]
fn limited_range_log_is_mapped_back_to_full() {
    // 10-bit limited-range Y′CbCr S-Log3 grey: Y code 420, neutral chroma.
    let input = InputColour::parse("slog3_sgamut3cine", None).unwrap();
    let desc = input.apply(ColourDesc { bit_depth: 10, ..ColourDesc::default() });
    assert_eq!(desc.transfer, SourceTransfer::Camera { curve: LogCurve::SLog3, legal: true });
    assert_eq!(desc.primaries, SourcePrimaries::Gamut(Gamut::SGamut3Cine));
    let pic = DecodedPicture { width: 2, height: 2, data: PixelData::Yuv { y: Planes::U16(vec![420; 4]), u: Planes::U16(vec![512; 1]), v: Planes::U16(vec![512; 1]), chroma_width: 1, chroma_height: 1 }, colour: desc };
    let p = pic.to_frame(true).pixels[0];
    for c in 0..3 {
        assert!((p[c] - 0.18).abs() < 0.003, "{p:?}");
    }
}

#[test]
fn input_names_parse() {
    for n in studio_render::colour_space::input_names() {
        InputColour::parse(&n, None).unwrap_or_else(|e| panic!("{n}: {e}"));
    }
    assert!(InputColour::parse("ocio:ACEScg", None).is_err());
    assert!(InputColour::parse("nonsense", None).is_err());
    assert_eq!(InputColour::parse("auto", None).unwrap(), InputColour::default());
}

fn log_scene(reg: &Arc<SourceRegistry>, view: ViewTransform) -> StaticScene {
    // An S-Log3 ramp of code values across, slightly warm.
    let src = ProceduralSource::new(64, 15, 30.0, 1, |_| {
        let codes: Vec<[f32; 3]> = (0..64).map(|i| {
            let v = 0.05 + 0.9 * i as f32 / 63.0;
            [v, v * 0.92, v * 0.85]
        }).collect();
        rgb16(&codes, 15, InputColour::parse("slog3", None).unwrap().apply(rgb_desc()))
    });
    reg.register(MediaKey(41), SourceSpec::Procedural(src));
    let mut canvas = Canvas::new(64, 15, 30.0);
    canvas.view = view;
    StaticScene::new(canvas, SECOND).with_clip(ClipDesc::new(1, 0, 0, SECOND, SourceDesc::Media { media: MediaKey(41), time: 0 }))
}

#[test]
fn gpu_decodes_every_camera_curve_like_the_cpu() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU: skipped");
        return;
    };
    for curve in LogCurve::ALL {
        let reg = SourceRegistry::new();
        let src = ProceduralSource::new(64, 4, 30.0, 1, move |_| {
            let codes: Vec<[f32; 3]> = (0..64).map(|i| {
                let v = 0.03 + 0.94 * i as f32 / 63.0;
                [v, v * 0.95, v * 0.88]
            }).collect();
            rgb16(&codes, 4, InputColour::parse(curve.slug(), None).unwrap().apply(rgb_desc()))
        });
        reg.register(MediaKey(7), SourceSpec::Procedural(src));
        let scene = StaticScene::new(Canvas::new(64, 4, 30.0), SECOND).with_clip(ClipDesc::new(1, 0, 0, SECOND, SourceDesc::Media { media: MediaKey(7), time: 0 }));
        let mut cpu = CpuCompositor::new(common::sources(&reg));
        let mut gpu = GpuCompositor::new(ctx.clone(), common::sources(&reg));
        let a = cpu.render(&scene, 0, None);
        let b = gpu.render(&scene, 0, None);
        let (max, bad) = compare_relative(&a, &b, 0.01);
        eprintln!("{curve:?}: max {max:.4}, beyond {:.2}%", bad * 100.0);
        assert!(bad <= 0.01, "{curve:?}: max {max}, {:.2}% beyond", bad * 100.0);
    }
}

#[test]
fn aces_view_is_applied_once_and_agrees() {
    let reg = SourceRegistry::new();
    let plain = log_scene(&reg, ViewTransform::None);
    let viewed = log_scene(&reg, ViewTransform::Aces { hdr_peak: 1000 });
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let a = cpu.render(&plain, 0, None);
    let b = cpu.render(&viewed, 0, None);
    // The view compresses highlights: the brightest scene value is above 1
    // without it and within display range with it.
    let max = |f: &studio_render::Frame| f.pixels.iter().map(|p| p[0]).fold(0f32, f32::max);
    assert!(max(&a) > 2.0, "scene-linear highlights {}", max(&a));
    assert!(max(&b) <= 1.001, "viewed {}", max(&b));
    // Encoding through Rec.709 output also applies it.
    let enc = cpu.render_encoded(&viewed, 0, None, &OutputSettings { space: OutputSpace::Rec709, ..Default::default() }, OutputDepth::Sixteen, 0);
    assert_eq!(enc.width, 64);
    if let Some(ctx) = GpuContext::new() {
        let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
        let g = gpu.render(&viewed, 0, None);
        let (m, bad) = compare_relative(&b, &g, 0.01);
        assert!(bad <= 0.01, "view on GPU: max {m}, {:.2}% beyond", bad * 100.0);
    }
}
