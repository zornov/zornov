// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reused layers must look exactly like freshly rendered ones: a
//! compositor that keeps processed layers between frames is compared,
//! frame by frame, with one that keeps nothing, while the layers move,
//! copies repeat, small copies use kept minified pictures, effects
//! animate, video plays and the budget is tiny.

mod common;

use std::sync::Arc;

use common::*;
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::mask::MaskShape;
use studio_render::scene::{Canvas, ClipDesc, GeneratorDesc, GeneratorKind, Micros, Param, Params, ParamsExt, SECOND, Scene, SourceDesc, StaticScene, TextRun, TransitionDesc};
use studio_render::source::{ProceduralSource, SourceRegistry, SourceSpec};
use studio_render::text::{TextRenderer, set_text_renderer};
use studio_render::{CpuCompositor, Frame, GpuCompositor, GpuContext, MediaKey};

const FRAMES: i64 = 6;

/// Every layer above the spine drifts and turns with time; media plays
/// from the clip start.
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
            if let SourceDesc::Media { time, .. } = &mut c.source {
                *time = t - c.start;
            }
            if c.order > 0 {
                c.placement.position[0] += 30.0 * s + c.id as f32;
                c.placement.rotation += 40.0 * s;
            }
        }
        v
    }
    fn transition_at(&self, _t: Micros) -> Option<TransitionDesc> {
        None
    }
}

/// Key 1 a moving test pattern, key 3 a still picture.
fn reg() -> Arc<SourceRegistry> {
    let reg = registry();
    reg.register(MediaKey(3), SourceSpec::Procedural(ProceduralSource::new(64, 48, 30.0, 1, |_| (ProceduralSource::test_pattern(64, 48, 30.0, 1).generate)(0))));
    reg
}

fn still(id: u64, order: i32) -> ClipDesc {
    let mut c = media(id, order, 0, 4 * SECOND, MediaKey(3));
    c.placement.scale = [0.3, 0.3];
    c
}

fn scene() -> Drifting {
    let dur = 4 * SECOND;
    let mut bg = gradient(1, 0, 0, dur, [0.1, 0.0, 0.3, 1.0], [0.9, 0.5, 0.1, 1.0], 20.0);
    bg.masks.push(mask(MaskShape::Ellipse, 0.2));
    let mut card = shape(2, 1, 0, dur, "rectangle", [0.9, 0.9, 1.0, 0.8]);
    card.placement.scale = [0.4, 0.4];
    card.effects.push(effect("drop_shadow", &[]));
    let mut waving = shape(3, 2, 0, dur, "star", [1.0, 0.3, 0.2, 1.0]);
    waving.placement.scale = [0.5, 0.5];
    waving.placement.position = [-60.0, -30.0];
    waving.effects.push(effect("wave", &[("amplitude", Param::Float(2.0)), ("speed", Param::Float(2.0))]));
    let mut video = media(4, 3, 0, dur, MediaKey(1));
    video.placement.scale = [0.4, 0.4];
    video.placement.position = [55.0, -25.0];
    video.placement.crop = [0.1, 0.0, 0.0, 0.1];
    video.grade = Some(grade(&[("exposure", Param::Float(0.3)), ("grain", Param::Float(0.5))]));
    let mut s = StaticScene::new(Canvas::new(192, 108, 30.0), dur).with_clip(bg).with_clip(card).with_clip(waving).with_clip(video);
    // Copies of one still, small enough to be drawn from halved pictures;
    // one glows, one is bent by pins, one is blurred by its motion.
    for i in 0..5u64 {
        let mut c = still(10 + i, 4 + i as i32);
        c.placement.position = [-70.0 + i as f32 * 30.0, 20.0];
        match i {
            1 => c.effects.push(effect("glow", &[])),
            2 => c.warp = Some(puppet_warp()),
            3 => c.placement.scale = [0.12, 0.12],
            _ => {}
        }
        s = s.with_clip(c);
    }
    let mut blurred = still(20, 9);
    blurred.motion.entry = Some(studio_render::scene::MotionRef { kind: "blur_in".into(), duration: SECOND, params: Params::new() });
    s = s.with_clip(blurred);
    Drifting(s)
}

fn times() -> impl Iterator<Item = Micros> {
    (0..FRAMES).map(|i| i * SECOND / 30)
}

fn assert_same(what: &str, t: Micros, a: &Frame, b: &Frame) {
    let (max, _) = compare_relative(a, b, 0.0);
    assert!(max == 0.0, "{what} at {t}: kept and fresh pictures differ by {max}");
}

#[test]
fn kept_layers_match_fresh_ones_on_the_cpu() {
    let reg = reg();
    let s = scene();
    let mut kept = CpuCompositor::new(sources(&reg));
    let mut fresh = CpuCompositor::with_budget(sources(&reg), 0);
    for size in [None, Some((96, 54))] {
        for t in times() {
            assert_same("cpu", t, &kept.render(&s, t, size), &fresh.render(&s, t, size));
        }
    }
    let st = kept.reuse_stats();
    assert!(st.hits > st.misses, "{st:?}");
    assert_eq!(fresh.reuse_stats().entries, 0);
}

#[test]
fn a_tiny_budget_still_draws_the_same() {
    let reg = reg();
    let s = scene();
    // Room for about two canvas-sized layers at a time.
    let budget = 700 << 10;
    let mut kept = CpuCompositor::with_budget(sources(&reg), budget);
    let mut fresh = CpuCompositor::with_budget(sources(&reg), 0);
    for t in times() {
        assert_same("tiny budget", t, &kept.render(&s, t, None), &fresh.render(&s, t, None));
        assert!(kept.reuse_stats().bytes <= budget, "{:?}", kept.reuse_stats());
    }
    assert!(kept.reuse_stats().hits > 0, "{:?}", kept.reuse_stats());
}

#[test]
fn copies_are_processed_once() {
    let reg = reg();
    let mut s = StaticScene::new(Canvas::new(192, 108, 30.0), SECOND);
    for i in 0..8 {
        s = s.with_clip(still(i, i as i32));
    }
    let mut c = CpuCompositor::new(sources(&reg));
    c.render(&s, 0, None);
    let st = c.reuse_stats();
    // One picture made and shared by all eight.
    assert_eq!((st.misses, st.hits, st.entries), (1, 7, 1), "{st:?}");
    c.render(&s, SECOND / 30, None);
    assert_eq!(c.reuse_stats().misses, 1);
}

/// Text drawn as a block whose brightness is set at creation.
struct Blocks(f32);

impl TextRenderer for Blocks {
    fn raster(&self, runs: &[TextRun], style: &Params, width: u32, height: u32) -> Option<Frame> {
        let size = style.f("size", 0.3);
        let v = self.0;
        (!runs.is_empty()).then(|| Frame::from_fn(width, height, |x, y| if (x as f32) < width as f32 * size && y > height / 3 { [v, v, v, 1.0] } else { [0.0; 4] }))
    }
}

#[test]
fn a_new_text_renderer_redraws_titles() {
    let text = |size: f64| {
        let g = GeneratorDesc { kind: GeneratorKind::Text, params: params(&[("size", Param::Float(size))]), text: vec![TextRun { text: "Hi".into(), style: Params::new() }] };
        ClipDesc::new(1, 0, 0, SECOND, SourceDesc::Generator(g))
    };
    let s = StaticScene::new(Canvas::new(64, 36, 30.0), SECOND).with_clip(text(0.5));
    let mut c = CpuCompositor::new(sources(&reg()));
    set_text_renderer(Arc::new(Blocks(0.25)));
    let dim = c.render(&s, 0, None);
    assert_eq!(dim.get(4, 30), [0.25, 0.25, 0.25, 1.0]);
    set_text_renderer(Arc::new(Blocks(1.0)));
    assert_eq!(c.render(&s, 0, None).get(4, 30), [1.0; 4]);
    // A changed parameter is a new picture too.
    let s = StaticScene::new(Canvas::new(64, 36, 30.0), SECOND).with_clip(text(0.1));
    assert_eq!(c.render(&s, 0, None).get(20, 30), [0.0, 0.0, 0.0, 1.0]);
}

fn gpu() -> Option<Arc<GpuContext>> {
    if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
        return None;
    }
    let ctx = GpuContext::new();
    if ctx.is_none() {
        eprintln!("no GPU: test skipped");
    }
    ctx
}

#[test]
fn kept_layers_match_fresh_ones_on_the_gpu_and_agree_with_the_cpu() {
    let Some(ctx) = gpu() else { return };
    let reg = reg();
    let s = scene();
    let mut kept = GpuCompositor::new(ctx.clone(), sources(&reg));
    let mut fresh = GpuCompositor::new(ctx, sources(&reg));
    fresh.set_cache_budget(0);
    let mut cpu = CpuCompositor::new(sources(&reg));
    for size in [None, Some((96, 54))] {
        for t in times() {
            let k = kept.render(&s, t, size);
            assert_same("gpu", t, &k, &fresh.render(&s, t, size));
            let (max, bad) = compare_relative(&cpu.render(&s, t, size), &k, 0.01);
            assert!(bad < 0.005, "gpu and cpu differ at {t}: {:.2}% of pixels, max {max}", bad * 100.0);
        }
    }
    let st = kept.reuse_stats();
    assert!(st.hits > st.misses, "{st:?}");
}

#[test]
fn kept_layers_match_fresh_ones_in_3d_runs() {
    // 3D runs build every layer before drawing (shadows need all
    // casters), so kept textures are held across the whole run.
    let reg = reg();
    let ctx = gpu();
    for (name, s, _) in space_scenes(Canvas::new(160, 90, 30.0)) {
        let s = Drifting(s);
        let mut kept = CpuCompositor::new(sources(&reg));
        let mut fresh = CpuCompositor::with_budget(sources(&reg), 0);
        let mut gpu = ctx.as_ref().map(|c| (GpuCompositor::new(c.clone(), sources(&reg)), GpuCompositor::new(c.clone(), sources(&reg))));
        if let Some((_, fresh)) = &mut gpu {
            fresh.set_cache_budget(0);
        }
        for t in times() {
            assert_same(name, t, &kept.render(&s, t, None), &fresh.render(&s, t, None));
            if let Some((kept, fresh)) = &mut gpu {
                assert_same(name, t, &kept.render(&s, t, None), &fresh.render(&s, t, None));
            }
        }
        assert!(kept.reuse_stats().hits > 0, "{name}: {:?}", kept.reuse_stats());
    }
}
