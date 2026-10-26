// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The product requirement: 1080p30, three stacked clips with two effects
//! each, real time on an M1-class machine. Renders the scene with the GPU
//! and the CPU backends at the canvas size and at a half-size preview and
//! prints a Markdown table of milliseconds per frame.
//!
//! `cargo bench -p studio-render` (add `-- --frames 120` for a longer run;
//! `STUDIO_RENDER_NO_GPU=1` skips the GPU rows).

use std::sync::Arc;
use std::time::Instant;

use studio_render::compositor::FrameRenderer;
use studio_render::scene::{Canvas, ClipDesc, EffectDesc, Micros, Param, SECOND, Scene, SourceDesc, StaticScene, TransitionDesc};
use studio_render::source::{DecodeCache, ProceduralSource, SourceRegistry, SourceSpec, Sources};
use studio_render::{BlendMode, CpuCompositor, GpuCompositor, GpuContext, MediaKey, OutputDepth, OutputSettings};

struct Moving(StaticScene);

impl Scene for Moving {
    fn canvas(&self) -> Canvas {
        self.0.canvas()
    }
    fn duration(&self) -> Micros {
        self.0.duration()
    }
    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        let mut v = self.0.clips_at(t);
        for c in &mut v {
            if let SourceDesc::Media { time, .. } = &mut c.source {
                *time = t - c.start;
            }
        }
        v
    }
    fn transition_at(&self, _t: Micros) -> Option<TransitionDesc> {
        None
    }
}

fn effect(kind: &str, kv: &[(&str, f64)]) -> EffectDesc {
    EffectDesc { kind: kind.into(), params: kv.iter().map(|(k, v)| (k.to_string(), Param::Float(*v))).collect() }
}

/// Three 1080p clips: a full-frame base, a picture-in-picture at half size
/// with rotation and a blend mode, and a small third layer; two
/// shader-backed effects on each.
fn scene() -> (Arc<dyn Scene>, Arc<SourceRegistry>) {
    let reg = SourceRegistry::new();
    for k in 1..=3u64 {
        reg.register(MediaKey(k), SourceSpec::Procedural(ProceduralSource::test_pattern(1920, 1080, 30.0, 3000)));
    }
    let dur = 100 * SECOND;
    let mut a = ClipDesc::new(1, 0, 0, dur, SourceDesc::Media { media: MediaKey(1), time: 0 });
    a.effects = vec![effect("posterise", &[("levels", 8.0)]), effect("chromatic_aberration", &[("amount", 0.8)])];
    let mut b = ClipDesc::new(2, 1, 0, dur, SourceDesc::Media { media: MediaKey(2), time: 0 });
    b.placement.scale = [0.5, 0.5];
    b.placement.rotation = 8.0;
    b.placement.position = [300.0, -120.0];
    b.blend = BlendMode::Screen;
    b.effects = vec![effect("wave", &[("amplitude", 1.0)]), effect("rgb_split", &[("distance", 1.5)])];
    let mut c = ClipDesc::new(3, 2, 0, dur, SourceDesc::Media { media: MediaKey(3), time: 0 });
    c.placement.scale = [0.3, 0.3];
    c.placement.position = [-500.0, 250.0];
    c.placement.opacity = 0.85;
    c.effects = vec![effect("pixelate", &[("size", 1.5)]), effect("edge_detect", &[("strength", 1.0)])];
    let s = StaticScene::new(Canvas::new(1920, 1080, 30.0), dur).with_clip(a).with_clip(b).with_clip(c);
    (Arc::new(Moving(s)), reg)
}

fn time_frames(r: &mut dyn FrameRenderer, scene: &dyn Scene, frames: u64, size: Option<(u32, u32)>, encode: bool) -> f64 {
    let settings = OutputSettings::default();
    // Warm up: pipelines, pool, picture cache.
    for i in 0..2 {
        if encode {
            r.render_encoded(scene, i * SECOND / 30, size, &settings, OutputDepth::Eight, i as u32);
        } else {
            r.render(scene, i * SECOND / 30, size);
        }
    }
    let start = Instant::now();
    for i in 0..frames {
        let t = (i + 2) as Micros * SECOND / 30;
        if encode {
            r.render_encoded(scene, t, size, &settings, OutputDepth::Eight, i as u32);
        } else {
            r.render(scene, t, size);
        }
    }
    start.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: u64 = args.iter().position(|a| a == "--frames").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(60);
    let (scene, reg) = scene();
    let cache = DecodeCache::new(512 << 20);
    let sources = || Sources::new(reg.clone(), cache.clone());

    // Cost of the stand-in "decode" alone (generating three 1080p test
    // pictures), so the compositing numbers can be read net of it.
    let mut s = sources();
    let start = Instant::now();
    for i in 0..frames {
        for k in 1..=3u64 {
            s.picture(MediaKey(k), (i + 1000) as Micros * SECOND / 30);
        }
    }
    let decode_ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;

    println!("| backend | case | ms/frame | fps |");
    println!("|---|---|---:|---:|");
    println!("| (source) | three 1080p test pictures | {decode_ms:.1} | {:.0} |", 1000.0 / decode_ms);
    if let Some(ctx) = GpuContext::new() {
        let mut gpu = GpuCompositor::new(ctx.clone(), sources());
        for (case, size, encode) in [("1080p, rgba8 out", None, true), ("1080p, f32 readback", None, false), ("960x540 preview, rgba8 out", Some((960, 540)), true)] {
            let ms = time_frames(&mut gpu, scene.as_ref(), frames, size, encode);
            println!("| gpu ({}) | {case} | {ms:.1} | {:.0} |", ctx.adapter_name, 1000.0 / ms);
        }
    } else {
        println!("| gpu | unavailable | | |");
    }
    let cpu_frames = (frames / 6).max(3);
    let mut cpu = CpuCompositor::new(sources());
    for (case, size) in [("1080p, rgba8 out", None), ("960x540 preview, rgba8 out", Some((960, 540)))] {
        let ms = time_frames(&mut cpu, scene.as_ref(), cpu_frames, size, true);
        println!("| cpu ({} threads) | {case} | {ms:.1} | {:.0} |", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), 1000.0 / ms);
    }
}
