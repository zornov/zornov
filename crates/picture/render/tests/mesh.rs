// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D models through the whole pipeline: a project's model layer, its
//! transform, the scene's camera and lights, the CPU and GPU compositors.

mod common;

use std::sync::Arc;
use studio_model::{Clip, ClipId, ClipSource, Generator, GeneratorKind, Project, ProjectSettings, Value};
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::model_scene::ModelScene;
use studio_render::{CpuCompositor, GpuCompositor, GpuContext};

fn model_file(name: &str, mesh: &studio_mesh::Mesh) -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/models");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.smesh"));
    studio_mesh::io::save(mesh, &path).unwrap();
    path.to_string_lossy().into_owned()
}

fn project_with(file: &str, f: impl Fn(&mut Clip)) -> Project {
    let mut project = Project::new(ProjectSettings::default());
    project.settings.width = 320;
    project.settings.height = 180;
    let spine = project.active().spine_id();
    let mut g = Generator::new(GeneratorKind::Model);
    g.params.insert("file".into(), Value::Text(file.into()));
    let mut clip = Clip::new(ClipId(5), spine, 0, 2_000_000, ClipSource::Generator(g));
    f(&mut clip);
    project.sequences.iter_mut().next().unwrap().clips.push(clip);
    project
}

fn dump(name: &str, f: &studio_render::Frame) {
    if std::env::var_os("RENDER_DUMP").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/mesh");
        std::fs::create_dir_all(&dir).unwrap();
        f.save_png(&dir.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn a_model_layer_turns_with_its_transform() {
    // A long box: turned 90° about y it shows its end, which is narrow.
    let file = model_file("bar", &studio_mesh::prim::cube([200.0, 40.0, 40.0]));
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let width_of = |f: &studio_render::Frame| (0..f.width as usize).filter(|&x| f.pixels[90 * f.width as usize + x][1] > 0.05).count();
    let flat = cpu.render(&ModelScene::new(Arc::new(project_with(&file, |_| {}))), 0, None);
    dump("bar_flat", &flat);
    let turned = cpu.render(&ModelScene::new(Arc::new(project_with(&file, |c| c.transform.rotation_y = 90.0))), 0, None);
    dump("bar_turned", &turned);
    let (a, b) = (width_of(&flat), width_of(&turned));
    assert!((195..=215).contains(&a), "flat width {a}");
    assert!(b < 60, "end on: {b}");
    // Moved right by its x position.
    let moved = cpu.render(&ModelScene::new(Arc::new(project_with(&file, |c| c.transform.position.x = 60.0))), 0, None);
    assert!(moved.pixels[90 * 320 + 160 + 60 + 90][1] > 0.05 && moved.pixels[90 * 320 + 160 - 60][1] < 0.05);
}

#[test]
fn gpu_matches_cpu_on_a_model() {
    let Some(ctx) = GpuContext::new() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut ball = studio_mesh::prim::ico_sphere(70.0, 4);
    ball.materials[0].base_colour = [0.8, 0.3, 0.2, 1.0];
    let file = model_file("ball", &ball);
    let scene = ModelScene::new(Arc::new(project_with(&file, |c| c.transform.rotation_x = 20.0)));
    let reg = common::registry();
    let a = CpuCompositor::new(common::sources(&reg)).render(&scene, 0, None);
    let b = GpuCompositor::new(ctx, common::sources(&reg)).render(&scene, 0, None);
    dump("ball_cpu", &a);
    dump("ball_gpu", &b);
    let (max, bad) = compare_relative(&a, &b, 0.01);
    assert!(bad <= 0.005, "{:.2}% differ (max {max})", bad * 100.0);
}

#[test]
fn the_head_base_reads_as_a_head_from_three_sides() {
    let file = model_file("head", &studio_mesh::sdf::head(390.0, 5));
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    for turn in [0.0, 35.0, 90.0] {
        let mut project = project_with(&file, |c| {
            c.transform.rotation_y = turn;
            c.transform.position.y = -60.0;
        });
        (project.settings.width, project.settings.height) = (960, 540);
        let f = cpu.render(&ModelScene::new(Arc::new(project)), 0, None);
        dump(&format!("head_{turn}"), &f);
        assert!(f.pixels[210 * 960 + 480][0] > 0.1, "the face shows");
    }
}
