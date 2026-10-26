// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Drivers reach the renderer: a layer whose position and rotation are
//! driven by another layer's is described, and drawn, where its formula
//! says, the same on the CPU and the GPU.

use std::sync::Arc;
use studio_model::{Clip, ClipId, ClipSource, Driver, DriverVar, Generator, GeneratorKind, Interp, Keyframe, Lane, LaneId, LaneKind, Project, ProjectSettings, Track, Value, Vec2};
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::model_scene::ModelScene;
use studio_render::scene::Scene;
use studio_render::source::{DecodeCache, SourceRegistry, Sources};
use studio_render::{CpuCompositor, GpuCompositor, GpuContext};

const SECOND: i64 = 1_000_000;

fn card(id: u64, lane: LaneId, colour: [f64; 3]) -> Clip {
    let mut g = Generator::new(GeneratorKind::Shape);
    g.params.insert("fill".into(), Value::Colour(studio_model::Rgba { r: colour[0], g: colour[1], b: colour[2], a: 1.0 }));
    g.params.insert("size".into(), Value::Point(Vec2::new(0.1, 0.1)));
    Clip::new(ClipId(id), lane, 0, 2 * SECOND, ClipSource::Generator(g))
}

fn project() -> Project {
    let mut project = Project::new(ProjectSettings::default());
    let seq = project.sequences.iter_mut().next().unwrap();
    seq.lanes.insert(0, Lane::new(LaneId(70), LaneKind::Visual, "A"));
    seq.lanes.insert(0, Lane::new(LaneId(71), LaneKind::Visual, "B"));
    let mut leader = card(1, LaneId(70), [1.0, 0.2, 0.1]);
    leader.keyframes.insert("transform.position".into(), Track::new(vec![Keyframe::new(0, Vec2::new(-400.0, 0.0), Interp::Linear), Keyframe::new(SECOND, Vec2::new(400.0, 200.0), Interp::Linear)]));
    let mut follower = card(2, LaneId(71), [0.1, 0.4, 1.0]);
    // Mirrors the leader across the centre, and turns with its x.
    follower.drivers.insert("transform.position".into(), Driver { expr: "-p".into(), vars: vec![DriverVar { name: "p".into(), clip: ClipId(1), property: "transform.position".into(), component: None, delay: 0 }], enabled: true });
    follower.drivers.insert("transform.rotation".into(), Driver { expr: "x / 10".into(), vars: vec![DriverVar { name: "x".into(), clip: ClipId(1), property: "transform.position".into(), component: Some(0), delay: 0 }], enabled: true });
    seq.clips.push(leader);
    seq.clips.push(follower);
    project
}

#[test]
fn a_driven_layer_is_described_where_its_driver_says() {
    let scene = ModelScene::new(Arc::new(project()));
    let at = |t: i64| {
        let list = scene.clips_at(t);
        let f = list.iter().find(|c| c.id == 2).expect("the follower draws").placement.clone();
        (f.position, f.rotation)
    };
    assert_eq!(at(0), ([400.0, 0.0], -40.0));
    assert_eq!(at(SECOND / 2), ([0.0, -100.0], 0.0));
    assert_eq!(at(SECOND), ([-400.0, -200.0], 40.0));
}

#[test]
fn gpu_matches_cpu_on_a_driven_layer() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU: agreement test skipped");
        return;
    };
    let scene = ModelScene::new(Arc::new(project()));
    let reg = SourceRegistry::new();
    let mut cpu = CpuCompositor::new(Sources::new(reg.clone(), DecodeCache::new(64 << 20)));
    let mut gpu = GpuCompositor::new(ctx, Sources::new(reg, DecodeCache::new(64 << 20)));
    for t in [0, SECOND / 3, SECOND] {
        let a = cpu.render(&scene, t, Some((320, 180)));
        let b = gpu.render(&scene, t, Some((320, 180)));
        let (max, bad) = compare_relative(&a, &b, 0.01);
        assert!(bad <= 0.005, "t={t}: {:.2}% of pixels differ (max {max})", bad * 100.0);
    }
}
