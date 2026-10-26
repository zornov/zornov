// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node compositor in the renderer: a `comp` effect on an adjustment
//! layer (the whole shot) and on a clip, on the CPU and the GPU, which
//! must agree.

use serde_json::json;
use studio_fx::comp::{Graph, Key, Node};
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::scene::{Canvas, ClipDesc, EffectDesc, GeneratorDesc, GeneratorKind, Param, Params, SECOND, SourceDesc, StaticScene};
use studio_render::source::{DecodeCache, SourceRegistry, Sources};
use studio_render::{CpuCompositor, GpuCompositor, GpuContext};

fn params(kv: &[(&str, Param)]) -> Params {
    kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn gradient(id: u64, order: i32) -> ClipDesc {
    ClipDesc::new(
        id,
        order,
        0,
        4 * SECOND,
        SourceDesc::Generator(GeneratorDesc {
            kind: GeneratorKind::Gradient,
            params: params(&[("start", Param::Colour([0.9, 0.3, 0.1, 1.0])), ("end", Param::Colour([0.1, 0.4, 0.9, 1.0])), ("angle", Param::Float(30.0))]),
            text: vec![],
        }),
    )
}

fn shape(id: u64, order: i32) -> ClipDesc {
    ClipDesc::new(
        id,
        order,
        0,
        4 * SECOND,
        SourceDesc::Generator(GeneratorDesc {
            kind: GeneratorKind::Shape,
            params: params(&[("shape", Param::Choice("ellipse".into())), ("fill", Param::Colour([0.2, 0.9, 0.3, 1.0])), ("size", Param::Point([0.4, 0.5]))]),
            text: vec![],
        }),
    )
}

fn comp_effect(g: &Graph) -> EffectDesc {
    EffectDesc { kind: "comp".into(), params: params(&[("graph", Param::Text(g.to_json()))]) }
}

fn set(g: &mut Graph, id: u32, k: &str, v: serde_json::Value) {
    g.node_mut(id).unwrap().params.insert(k.into(), v);
}

/// A graph using most GPU kernels with CPU nodes in between.
fn busy_graph() -> Graph {
    let mut g = Graph::passthrough();
    let gr = g.add("grade", &[Some(1)]);
    set(&mut g, gr, "gain", json!(1.3));
    set(&mut g, gr, "gamma", json!(1.2));
    set(&mut g, gr, "saturation", json!(0.7));
    let tr = g.add("transform", &[Some(gr)]);
    set(&mut g, tr, "translate_x", json!(12.0));
    set(&mut g, tr, "rotate", json!(8.0));
    set(&mut g, tr, "scale", json!(0.9));
    let ramp = g.add("ramp", &[]);
    let m = g.add("merge", &[Some(ramp), Some(tr)]);
    set(&mut g, m, "operation", json!("Over"));
    let lens = g.add("lens_distort", &[Some(m)]);
    set(&mut g, lens, "k1", json!(-0.1));
    set(&mut g, lens, "chroma", json!(0.004));
    // A CPU node in the middle.
    let med = g.add("median", &[Some(lens)]);
    let bl = g.add("blur", &[Some(med)]);
    set(&mut g, bl, "size", json!(2.5));
    // Depth from a ramp (CPU), then depth of field (GPU).
    let dramp = g.add("ramp", &[]);
    set(&mut g, dramp, "from", json!([0.0, 0.5]));
    set(&mut g, dramp, "to", json!([1.0, 0.5]));
    let sd = g.add("set_depth", &[Some(bl), Some(dramp)]);
    set(&mut g, sd, "near", json!(5.0));
    set(&mut g, sd, "far", json!(50.0));
    let z = g.add("zdefocus", &[Some(sd)]);
    set(&mut g, z, "focus", json!(6.0));
    set(&mut g, z, "size", json!(5.0));
    set(&mut g, z, "quality", json!(24));
    let lm = g.add("luma_matte", &[Some(z)]);
    set(&mut g, lm, "output", json!(2));
    let ds = g.add("despill", &[Some(lm)]);
    let sh = g.add("shuffle", &[Some(ds)]);
    set(&mut g, sh, "r", json!("B green"));
    set(&mut g, sh, "g", json!("B red"));
    let cr = g.add("crop", &[Some(sh)]);
    set(&mut g, cr, "top_left", json!([0.05, 0.05]));
    set(&mut g, cr, "softness", json!(4.0));
    g.node_mut(cr).unwrap().mix = 0.8;
    g
}

#[test]
fn a_whole_shot_comp_agrees_on_cpu_and_gpu() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let reg = SourceRegistry::new();
    let sources = || Sources::new(reg.clone(), DecodeCache::new(64 << 20));
    let mut adj = ClipDesc::new(9, 5, 0, 4 * SECOND, SourceDesc::Adjustment);
    adj.effects.push(comp_effect(&busy_graph()));
    let scene = StaticScene::new(Canvas::new(160, 90, 24.0), 4 * SECOND).with_clip(gradient(1, 0)).with_clip(shape(2, 1)).with_clip(adj);
    let mut cpu = CpuCompositor::new(sources());
    let mut gpu = GpuCompositor::new(ctx, sources());
    let a = cpu.render(&scene, SECOND, None);
    let b = gpu.render(&scene, SECOND, None);
    let plain = {
        let s = StaticScene::new(Canvas::new(160, 90, 24.0), 4 * SECOND).with_clip(gradient(1, 0)).with_clip(shape(2, 1));
        cpu.render(&s, SECOND, None)
    };
    // The graph did something.
    let changed = a.pixels.iter().zip(&plain.pixels).filter(|(p, q)| (p[0] - q[0]).abs() > 0.02).count();
    assert!(changed > a.pixels.len() / 2, "the comp changed only {changed} pixels");
    let (max, bad) = compare_relative(&a, &b, 0.02);
    eprintln!("comp: max relative diff {max:.4}, {:.3}% beyond tolerance", bad * 100.0);
    assert!(bad < 0.01, "{:.2}% of pixels differ (max {max})", bad * 100.0);
}

#[test]
fn a_clip_comp_with_keyframes_changes_over_time() {
    let reg = SourceRegistry::new();
    let mut g = Graph::passthrough();
    let t = g.add("transform", &[Some(1)]);
    g.node_mut(t).unwrap().keys.insert("translate_x".into(), vec![Key { t: 0.0, v: json!(0.0), hold: false }, Key { t: 2.0, v: json!(60.0), hold: false }]);
    let mut clip = shape(2, 1);
    clip.effects.push(comp_effect(&g));
    let scene = StaticScene::new(Canvas::new(160, 90, 24.0), 4 * SECOND).with_clip(gradient(1, 0)).with_clip(clip);
    let mut cpu = CpuCompositor::new(Sources::new(reg.clone(), DecodeCache::new(64 << 20)));
    let a = cpu.render(&scene, 0, None);
    let b = cpu.render(&scene, 2 * SECOND, None);
    let diff = a.pixels.iter().zip(&b.pixels).filter(|(p, q)| (p[1] - q[1]).abs() > 0.05).count();
    assert!(diff > 200, "the keyed move did not show ({diff} pixels changed)");
    let _ = Node::new(0, "dot");
}
