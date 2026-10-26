// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A heavy project where most of the picture does not change from frame
//! to frame: a gradient background, a dozen copies of the same sticker,
//! titles, cards with soft shadows, a blurred still and one live video
//! picture-in-picture. Everything moves (positions drift, stickers turn),
//! but only the video's pixels change. Prints a Markdown table of
//! milliseconds per frame for the viewer (960×540, playing and
//! scrubbing) and export (1080p, rgba8) on the GPU and the CPU, with the
//! layer cache off (every layer processed every frame, as before) and on;
//! the cache counters go to stderr.
//!
//! `cargo bench -p studio-render --bench reuse` (add `-- --frames 120`;
//! `STUDIO_RENDER_NO_GPU=1` skips the GPU rows).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use studio_render::compositor::FrameRenderer;
use studio_render::scene::{
    Canvas, ClipDesc, EffectDesc, GeneratorDesc, GeneratorKind, Micros, Param, Params, ParamsExt, SECOND, Scene, SourceDesc, StaticScene, TextRun, TransitionDesc,
};
use studio_render::source::{DecodeCache, ProceduralSource, SourceRegistry, SourceSpec, Sources};
use studio_render::yuv::{DecodedPicture, PixelData};
use studio_render::{CpuCompositor, Frame, GpuCompositor, GpuContext, MediaKey, OutputDepth, OutputSettings};

/// Text drawn with the `text` crate and the app's bundled Manrope.
struct BenchText(studio_text::TextEngine);

impl studio_render::text::TextRenderer for BenchText {
    fn raster(&self, runs: &[TextRun], style: &Params, width: u32, height: u32) -> Option<Frame> {
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        let c = style.colour("colour", [1.0; 4]);
        let ts = studio_text::TextStyle {
            font: studio_text::FontRequest::new("Manrope", 700, false),
            size: style.f("size", 60.0),
            fill: studio_text::Fill::Solid(studio_text::Rgba { r: c[0], g: c[1], b: c[2], a: c[3] }),
            ..Default::default()
        };
        let mut img = studio_colour::Image::new(width, height);
        self.0.render_onto(&mut img, &studio_text::TextBlock::plain(text), &ts, [width as f32 / 2.0, height as f32 / 2.0], &studio_text::Hooks::NONE);
        Some(Frame::from_pixels(width, height, img.pixels))
    }
}

fn install_text() {
    let mut fonts = studio_text::FontLibrary::new();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/assets/fonts");
    for f in ["Manrope-Bold.ttf", "Manrope-SemiBold.ttf"] {
        let _ = fonts.load_file(&dir.join(f));
    }
    studio_render::text::set_text_renderer(Arc::new(BenchText(studio_text::TextEngine::new(fonts))));
}

/// The project with every placement drifting over time, so nothing can be
/// skipped by comparing whole frames.
struct Drifting(StaticScene);

impl Scene for Drifting {
    fn canvas(&self) -> Canvas {
        self.0.canvas()
    }
    fn duration(&self) -> Micros {
        self.0.duration()
    }
    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        let s = t as f32 / SECOND as f32;
        let mut v = self.0.clips_at(t);
        for c in &mut v {
            if let SourceDesc::Media { time, media } = &mut c.source
                && media.0 == 1
            {
                *time = t - c.start;
            }
            if c.order > 0 {
                let phase = c.id as f32 * 0.7;
                c.placement.position[0] += 40.0 * (s * 1.3 + phase).sin();
                c.placement.position[1] += 25.0 * (s * 0.9 + phase).cos();
            }
            if (10..30).contains(&c.id) {
                c.placement.rotation += 30.0 * s;
            }
        }
        v
    }
    fn transition_at(&self, _t: Micros) -> Option<TransitionDesc> {
        None
    }
}

fn params(kv: &[(&str, Param)]) -> Params {
    kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn effect(kind: &str, kv: &[(&str, f64)]) -> EffectDesc {
    EffectDesc { kind: kind.into(), params: kv.iter().map(|(k, v)| (k.to_string(), Param::Float(*v))).collect() }
}

fn generator(kind: GeneratorKind, p: Params, text: &str) -> SourceDesc {
    let text = if text.is_empty() { vec![] } else { vec![TextRun { text: text.into(), style: Params::new() }] };
    SourceDesc::Generator(GeneratorDesc { kind, params: p, text })
}

/// A 512×512 sticker: a disc with a ring, straight RGBA.
fn sticker() -> DecodedPicture {
    let n = 512usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - 255.5, y as f32 - 255.5);
            let r = (dx * dx + dy * dy).sqrt();
            let a = (250.0 - r).clamp(0.0, 1.0);
            let ring = r > 200.0;
            let i = (y * n + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[if ring { 255 } else { 250 }, if ring { 210 } else { 90 }, (x / 3) as u8, (a * 255.0) as u8]);
        }
    }
    DecodedPicture { width: n as u32, height: n as u32, colour: Default::default(), data: PixelData::Rgba8(rgba) }
}

fn scene() -> (Arc<dyn Scene>, Arc<SourceRegistry>) {
    let reg = SourceRegistry::new();
    let fps = 30.0;
    reg.register(MediaKey(1), SourceSpec::Procedural(ProceduralSource::test_pattern(1920, 1080, fps, 3000)));
    reg.register(MediaKey(2), SourceSpec::Procedural(ProceduralSource::new(512, 512, fps, 1, |_| sticker())));
    // A photo-sized still behind a blur.
    reg.register(MediaKey(3), SourceSpec::Procedural(ProceduralSource::new(1280, 720, fps, 1, |_| {
        let p = ProceduralSource::test_pattern(1280, 720, 30.0, 1);
        (p.generate)(0)
    })));
    let dur = 100 * SECOND;
    let mut s = StaticScene::new(Canvas::new(1920, 1080, fps), dur);
    let mut order = 0;
    let mut add = |s: StaticScene, id: u64, src: SourceDesc, f: &dyn Fn(&mut ClipDesc)| {
        let mut c = ClipDesc::new(id, order, 0, dur, src);
        order += 1;
        f(&mut c);
        s.with_clip(c)
    };
    // Background.
    let grad = params(&[
        ("radial", Param::Bool(true)),
        ("start", Param::Colour([0.05, 0.02, 0.2, 1.0])),
        ("end", Param::Colour([0.6, 0.1, 0.3, 1.0])),
    ]);
    s = add(s, 1, generator(GeneratorKind::Gradient, grad, ""), &|_| {});
    // A blurred still panel.
    s = add(s, 2, SourceDesc::Media { media: MediaKey(3), time: 0 }, &|c| {
        c.placement.scale = [0.7, 0.7];
        c.placement.position = [-300.0, 0.0];
        c.effects = vec![effect("gaussian_blur", &[("radius", 2.0)])];
    });
    // Live video, picture-in-picture.
    s = add(s, 3, SourceDesc::Media { media: MediaKey(1), time: 0 }, &|c| {
        c.placement.scale = [0.4, 0.4];
        c.placement.position = [500.0, -250.0];
    });
    // Cards: rounded rectangles with a soft shadow.
    for i in 0..6u64 {
        let p = params(&[
            ("shape", Param::Choice("rectangle".into())),
            ("size", Param::Point([0.22, 0.12])),
            ("roundness", Param::Float(0.4)),
            ("fill", Param::Colour([0.95, 0.95, 0.97, 0.9])),
        ]);
        s = add(s, 40 + i, generator(GeneratorKind::Shape, p, ""), &move |c| {
            c.placement.position = [-700.0 + i as f32 * 280.0, 380.0];
            c.effects = vec![effect("drop_shadow", &[])];
        });
    }
    // A dozen copies of one sticker.
    for i in 0..12u64 {
        s = add(s, 10 + i, SourceDesc::Media { media: MediaKey(2), time: 0 }, &move |c| {
            c.placement.scale = [0.25, 0.25];
            c.placement.position = [-800.0 + (i % 6) as f32 * 320.0, if i < 6 { -100.0 } else { 120.0 }];
            if i % 3 == 0 {
                c.effects = vec![effect("glow", &[])];
            }
        });
    }
    // Titles.
    for (i, t) in ["Studio", "Summer trip", "Day one", "The lake", "Thanks for watching", "@studio", "Chapter 2", "Fin"].iter().enumerate() {
        let p = params(&[("size", Param::Float(if i == 0 { 120.0 } else { 56.0 })), ("colour", Param::Colour([1.0, 1.0, 1.0, 1.0]))]);
        s = add(s, 30 + i as u64, generator(GeneratorKind::Text, p, t), &move |c| {
            c.placement.position = [-600.0 + (i % 4) as f32 * 400.0, -420.0 + (i / 4) as f32 * 120.0];
        });
    }
    (Arc::new(Drifting(s)), reg)
}

/// The frame times of a case: playing forward, or scrubbing (jumping
/// back and forth across a few seconds).
fn times(scrub: bool, frames: u64) -> Vec<Micros> {
    (0..frames + 2).map(|i| if scrub { ((i * 37) % 150) as Micros * SECOND / 30 } else { i as Micros * SECOND / 30 }).collect()
}

/// Milliseconds per frame (after two warm-up frames that fill pipelines
/// and caches).
fn time_frames(r: &mut dyn FrameRenderer, scene: &dyn Scene, times: &[Micros], size: Option<(u32, u32)>) -> f64 {
    let settings = OutputSettings::default();
    for (i, t) in times[..2].iter().enumerate() {
        r.render_encoded(scene, *t, size, &settings, OutputDepth::Eight, i as u32);
    }
    let start = Instant::now();
    for (i, t) in times[2..].iter().enumerate() {
        r.render_encoded(scene, *t, size, &settings, OutputDepth::Eight, i as u32);
    }
    start.elapsed().as_secs_f64() * 1000.0 / (times.len() - 2) as f64
}

fn row(backend: &str, case: &str, off: f64, on: f64) {
    println!("| {backend} | {case} | {off:.1} | {on:.1} | {:.0} | {:.1}x |", 1000.0 / on, off / on);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: u64 = args.iter().position(|a| a == "--frames").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(60);
    install_text();
    let (scene, reg) = scene();
    let cache = DecodeCache::new(512 << 20);
    let sources = || Sources::new(reg.clone(), cache.clone());
    let cases = [("viewer 960x540, playing", Some((960, 540)), false), ("viewer 960x540, scrubbing", Some((960, 540)), true), ("export 1080p", None, false)];

    println!("| backend | case | ms/frame, no reuse | ms/frame, reuse | fps | speedup |");
    println!("|---|---|---:|---:|---:|---:|");
    if std::env::var_os("STUDIO_RENDER_NO_GPU").is_none()
        && let Some(ctx) = GpuContext::new()
    {
        for (case, size, scrub) in cases {
            let t = times(scrub, frames);
            let mut off = GpuCompositor::new(ctx.clone(), sources());
            off.set_cache_budget(0);
            let mut on = GpuCompositor::new(ctx.clone(), sources());
            let (a, b) = (time_frames(&mut off, scene.as_ref(), &t, size), time_frames(&mut on, scene.as_ref(), &t, size));
            row(&format!("gpu ({})", ctx.adapter_name), case, a, b);
            eprintln!("gpu {case}: {:?}", on.reuse_stats());
        }
    } else {
        println!("| gpu | unavailable | | | | |");
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    for (case, size, scrub) in cases {
        // The CPU without reuse is slow: fewer frames.
        let t = times(scrub, (frames / 4).max(3));
        let mut off = CpuCompositor::with_budget(sources(), 0);
        let mut on = CpuCompositor::new(sources());
        let (a, b) = (time_frames(&mut off, scene.as_ref(), &t, size), time_frames(&mut on, scene.as_ref(), &t, size));
        row(&format!("cpu ({threads} threads)"), case, a, b);
        eprintln!("cpu {case}: {:?}", on.reuse_stats());
    }
}
