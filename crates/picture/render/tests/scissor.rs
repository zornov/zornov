// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cuts through the whole pipeline: a project's layer with puppet pins and
//! a cut, rendered headless. Without the cut the pins drag the whole
//! picture; with it each side moves alone and the gap shows the fill.

mod common;

use std::sync::Arc;
use studio_model as m;
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::model_scene::ModelScene;
use studio_render::scene::Scene;
use studio_render::{CpuCompositor, GpuCompositor, GpuContext};

const W: u32 = 160;
const H: u32 = 90;

/// A red layer with one pin on its left half pulled left by a tenth of its
/// width, and, when `cut` is set, a cut down the middle that fills its gap
/// with blue.
fn project(cut: Option<m::CutFill>) -> m::Project {
    let mut p = m::Project::new(m::ProjectSettings::default());
    p.settings.width = W;
    p.settings.height = H;
    let spine = p.active().spine_id();
    let mut g = m::Generator::new(m::GeneratorKind::Solid);
    g.params.insert("colour".into(), m::Value::Colour(m::Rgba::new(1.0, 0.0, 0.0, 1.0)));
    let mut clip = m::Clip::new(m::ClipId(5), spine, 0, 2_000_000, m::ClipSource::Generator(g));
    let mut puppet = m::Puppet::default();
    let pin = puppet.add(m::PinKind::Position, m::Vec2::new(0.25, 0.5));
    puppet.pin_mut(pin).unwrap().position = m::Vec2::new(0.15, 0.5);
    clip.puppet = Some(puppet);
    if let Some(fill) = cut {
        let mut c = m::Cut::new(1, vec![m::Vec2::new(0.5, -0.05), m::Vec2::new(0.5, 1.05)]);
        c.fill = fill;
        clip.cuts.push(c);
    }
    p.sequences.iter_mut().next().unwrap().clips.push(clip);
    p
}

fn render(project: m::Project) -> studio_render::Frame {
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    cpu.render(&ModelScene::new(Arc::new(project)), 0, None)
}

fn dump(name: &str, f: &studio_render::Frame) {
    if std::env::var_os("RENDER_DUMP").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/scissor");
        std::fs::create_dir_all(&dir).unwrap();
        f.save_png(&dir.join(format!("{name}.png"))).unwrap();
    }
}

fn red(f: &studio_render::Frame, x: u32, y: u32) -> bool {
    let p = f.pixels[(y * f.width + x) as usize];
    p[0] > 0.5 && p[1] < 0.2 && p[2] < 0.2
}

fn blue(f: &studio_render::Frame, x: u32, y: u32) -> bool {
    let p = f.pixels[(y * f.width + x) as usize];
    p[2] > 0.3 && p[0] < 0.2
}

#[test]
fn a_cut_lets_one_side_move_alone() {
    let (y, right) = (H / 2, W - 4);
    let whole = render(project(None));
    dump("whole", &whole);
    let cut = render(project(Some(m::CutFill::None)));
    dump("cut", &cut);
    // The pin drags everything left: the right edge of the picture is
    // pulled in.
    assert!(!red(&whole, right, y), "without a cut the right half follows the pin");
    assert!(red(&whole, 2, y), "and its left side is carried off to the left");
    // With the cut the right half stays put; the left half still moves.
    assert!(red(&cut, right, y), "the right half stays");
    assert!(red(&cut, W / 2 + 4, y));
    // The gap that opened down the middle is empty without a fill.
    let gap = (0..W).filter(|&x| !red(&cut, x, y) && x > W / 4).count();
    assert!(gap >= 8, "a gap opened at the cut: {gap} px");
}

#[test]
fn the_gap_shows_the_fill_colour() {
    let y = H / 2;
    let f = render(project(Some(m::CutFill::Colour(m::Rgba::new(0.0, 0.0, 1.0, 1.0)))));
    dump("filled", &f);
    let blues = (0..W).filter(|&x| blue(&f, x, y)).count();
    assert!(blues >= 8, "{blues} px of fill in the gap");
    // Every pixel of the row is the picture or the fill: no hole.
    for x in (W / 4)..W {
        assert!(red(&f, x, y) || blue(&f, x, y), "hole at {x}");
    }
}

#[test]
fn a_clip_without_cuts_is_unchanged() {
    // The scene description carries no cuts, so nothing changes for clips
    // that never had any.
    let p = project(None);
    let scene = ModelScene::new(Arc::new(p));
    assert!(scene.clips_at(0).iter().all(|c| c.warp.as_ref().is_none_or(|w| w.cuts.is_empty())));
    let p = project(Some(m::CutFill::Auto));
    let scene = ModelScene::new(Arc::new(p));
    let clips = scene.clips_at(0);
    let w = clips.iter().find_map(|c| c.warp.clone()).unwrap();
    assert_eq!(w.cuts.len(), 1);
    assert_eq!(w.cuts[0].fill, studio_render::puppet::FillDesc::Auto);
}

#[test]
fn the_gpu_draws_cuts_like_the_cpu() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU: agreement test skipped");
        return;
    };
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let mut gpu = GpuCompositor::new(ctx, common::sources(&reg));
    for (name, fill) in [("none", m::CutFill::None), ("colour", m::CutFill::Colour(m::Rgba::new(0.1, 0.3, 0.9, 1.0))), ("auto", m::CutFill::Auto)] {
        let scene = ModelScene::new(Arc::new(project(Some(fill))));
        let (a, b) = (cpu.render(&scene, 0, None), gpu.render(&scene, 0, None));
        let (max, bad) = compare_relative(&a, &b, 0.01);
        assert!(bad <= 0.005, "{name}: {:.2}% beyond tolerance (max {max})", bad * 100.0);
    }
}
