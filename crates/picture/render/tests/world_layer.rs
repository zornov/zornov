// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A walkable world as a layer of a project: drawn through the project's
//! camera by the real-time renderer, and path traced for final quality.

mod common;

use std::sync::Arc;
use studio_model::{Clip, ClipId, ClipSource, Generator, GeneratorKind, Project, ProjectSettings, Value, Vec2};
use studio_render::compositor::FrameRenderer;
use studio_render::model_scene::ModelScene;
use studio_render::{CpuCompositor, Frame};
use studio_world::build::WorldSpec;

fn spec() -> WorldSpec {
    WorldSpec { preset: "meadow".into(), size: Some(120.0), quality: "fast".into(), seed: Some(3), ..WorldSpec::default() }
}

fn world_file(name: &str) -> String {
    let spec = spec();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/worlds");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.sworld"));
    std::fs::write(&path, serde_json::to_string(&spec).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

/// A world clip and a camera 1.6 m up and `back` metres back, at x metres
/// to the side (the world is 100 px to the metre; y is down in pixels).
fn project(file: &str, x_m: f64, render: Option<&str>) -> Project {
    let mut project = Project::new(ProjectSettings::default());
    project.settings.width = 192;
    project.settings.height = 108;
    let spine = project.active().spine_id();
    let mut g = Generator::new(GeneratorKind::Model);
    g.params.insert("world".into(), Value::Text(file.into()));
    g.params.insert("metres_per_px".into(), Value::Scalar(0.01));
    g.params.insert("quality".into(), Value::Choice("fast".into()));
    let world = Clip::new(ClipId(5), spine, 0, 2_000_000, ClipSource::Generator(g));
    let mut cam = Generator::new(GeneratorKind::Camera);
    for (k, v) in studio_model::space::CameraSettings::defaults(192.0) {
        cam.params.insert(k, v);
    }
    if let Some(mode) = render {
        cam.params.insert("render".into(), Value::Text(mode.into()));
        cam.params.insert("samples".into(), Value::Scalar(2.0));
        cam.params.insert("preview_samples".into(), Value::Scalar(2.0));
        cam.params.insert("bounces".into(), Value::Scalar(2.0));
        cam.params.insert("denoise".into(), Value::Bool(false));
    }
    let mut camera = Clip::new(ClipId(6), spine, 0, 2_000_000, ClipSource::Generator(cam));
    // Eye height above the ground there.
    let ground = studio_world::build::build(&spec(), &studio_world::build::NoLibrary).unwrap().ground_y(x_m as f32, -9.0) as f64;
    camera.transform.position = Vec2::new(x_m * 100.0, -(ground + 1.7) * 100.0);
    camera.transform.position_z = -900.0;
    let seq = project.sequences.iter_mut().next().unwrap();
    seq.clips.push(world);
    seq.clips.push(camera);
    project
}

fn mean(f: &Frame, rows: std::ops::Range<usize>) -> [f32; 3] {
    let w = f.width as usize;
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for y in rows {
        for x in 0..w {
            let p = f.pixels[y * w + x];
            for k in 0..3 {
                sum[k] += p[k];
            }
            n += 1.0;
        }
    }
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

#[test]
fn a_world_layer_is_seen_through_the_project_camera() {
    let file = world_file("meadow");
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let a = cpu.render(&ModelScene::new(Arc::new(project(&file, 0.0, None))), 0, None);
    // Sky above, ground below: the top is bluer than the bottom, the
    // bottom greener than the top.
    let (top, bottom) = (mean(&a, 4..30), mean(&a, 80..104));
    assert!(top[2] > top[0] * 1.05, "blue sky: {top:?}");
    assert!(bottom[1] > bottom[2] * 1.2, "green ground: {bottom:?}");
    // Moving the camera along the x axis changes what it sees.
    let b = cpu.render(&ModelScene::new(Arc::new(project(&file, 3.0, None))), 0, None);
    assert_ne!(a.pixels, b.pixels);
    if std::env::var_os("RENDER_DUMP").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/world");
        std::fs::create_dir_all(&dir).unwrap();
        a.save_png(&dir.join("through_camera.png")).unwrap();
    }
}

#[test]
fn the_path_tracer_renders_the_same_world_for_final_quality() {
    let file = world_file("meadow_pt");
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let t = std::time::Instant::now();
    let f = cpu.render(&ModelScene::new(Arc::new(project(&file, 0.0, Some("path")))), 0, None);
    eprintln!("path traced world, 192x108, 2 spp: {:?}", t.elapsed());
    let (top, bottom) = (mean(&f, 4..30), mean(&f, 80..104));
    assert!(top.iter().sum::<f32>() > 0.05 && bottom.iter().sum::<f32>() > 0.02, "not black: {top:?} {bottom:?}");
    assert!(top[2] >= top[0], "sky from the same sun and sky model: {top:?}");
    if std::env::var_os("RENDER_DUMP").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target-render/world");
        std::fs::create_dir_all(&dir).unwrap();
        f.save_png(&dir.join("path_traced.png")).unwrap();
    }
}

/// The project's air and look reach a world layer: a light clip that
/// carries them gives the world its fog, exposure and look (the world keeps
/// its own sun and sky).
#[test]
fn the_scenes_air_and_look_reach_a_world_layer() {
    let file = world_file("meadow_air");
    let reg = common::registry();
    let mut cpu = CpuCompositor::new(common::sources(&reg));
    let plain = project(&file, 0.0, None);
    let mut foggy = plain.clone();
    let mut g = Generator::new(GeneratorKind::Light);
    for (k, v) in studio_model::space::LightSettings::defaults(studio_model::space::LightKind::Ambient) {
        g.params.insert(k, v);
    }
    g.params.insert("intensity".into(), Value::Scalar(0.0));
    g.params.insert("atmosphere".into(), Value::Bool(true));
    g.params.insert("fog_density".into(), Value::Scalar(0.06));
    g.params.insert("fog_colour".into(), Value::Colour(studio_model::Rgba::new(0.9, 0.75, 0.6, 1.0)));
    g.params.insert("exposure".into(), Value::Scalar(-1.0));
    let spine = foggy.active().spine_id();
    foggy.sequences.iter_mut().next().unwrap().clips.push(Clip::new(ClipId(7), spine, 0, 2_000_000, ClipSource::Generator(g)));
    let a = cpu.render(&ModelScene::new(Arc::new(plain)), 0, None);
    let b = cpu.render(&ModelScene::new(Arc::new(foggy)), 0, None);
    let (ma, mb) = (mean(&a, 30..80), mean(&b, 30..80));
    assert_ne!(a.pixels, b.pixels, "the world ignored the scene's air");
    // Warm fog over the middle of the picture, and one stop down: warmer and darker.
    assert!(mb[0] / mb[2].max(1e-3) > ma[0] / ma[2].max(1e-3) * 1.05, "fog colour did not reach it: {ma:?} then {mb:?}");
    assert!(mb[1] < ma[1] * 0.9, "the exposure did not reach it: {ma:?} then {mb:?}");
}
