// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The path tracer: its shader validates, the CPU reference lights and
//! shadows a scene sensibly, skies and glowing surfaces light it, smoke
//! dims what is behind it, and the GPU agrees with the CPU.

use std::sync::Arc;
use studio_mesh::Mesh;
use studio_render::mesh_render::{MaterialOverride, MeshDesc, MeshInstance};
use studio_render::scene::Canvas;
use studio_render::space::{CameraDesc, LightDesc, LightKind};
use studio_render::trace::{self, TraceSettings};

fn instance(id: u64, mesh: Mesh, at: [f32; 3]) -> MeshInstance {
    MeshInstance {
        id,
        mesh: Arc::new(mesh),
        key: format!("m{id}"),
        world: [[1.0, 0.0, 0.0, at[0]], [0.0, 1.0, 0.0, at[1]], [0.0, 0.0, 1.0, at[2]]],
        overrides: MaterialOverride::default(),
        smooth: 1.0,
        accepts_lights: true,
        casts_shadows: true,
        accepts_shadows: true,
        motion: None,
        direct: None,
    }
}

fn sun(dir: [f32; 3]) -> LightDesc {
    LightDesc { kind: LightKind::Parallel, position: [0.0; 3], direction: studio_render::space::normalise(dir), colour: [1.0; 3], cone: [0.0, 1.0], radius: 0.0, falloff: 0.0, casts_shadows: true, shadow_darkness: 1.0, sky: None, look: None }
}

fn ambient(c: f32) -> LightDesc {
    LightDesc { kind: LightKind::Ambient, colour: [c; 3], ..sun([0.0, 1.0, 0.0]) }
}

/// A ball resting on a floor, a sun from the upper left.
fn ball_on_floor(canvas: &Canvas) -> MeshDesc {
    let mut floor = studio_mesh::prim::plane(2000.0, 2000.0, 2);
    floor.materials[0].base_colour = [0.7, 0.7, 0.7, 1.0];
    let ball = studio_mesh::prim::ico_sphere(60.0, 3);
    MeshDesc {
        instances: vec![instance(1, floor, [0.0, 60.0, 200.0]), instance(2, ball, [0.0, 0.0, 200.0])],
        camera: Some(CameraDesc::default_for(canvas)),
        lights: vec![sun([0.6, 1.0, 0.3]), ambient(0.15)],
    }
}

fn lum(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

#[test]
fn the_tracer_shader_validates() {
    let src = trace::gpu::source();
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("trace: {}", e.emit_to_string(src)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("trace: {e:?}"));
}

#[test]
fn a_ball_is_lit_casts_a_soft_shadow_and_the_sky_is_clear() {
    let canvas = Canvas::new(160, 90, 24.0);
    let desc = ball_on_floor(&canvas);
    let scene = trace::scene::Scene::build(&desc, &canvas, (160, 90));
    let settings = TraceSettings { samples: 24, denoise: false, ..Default::default() };
    let p = trace::cpu::render(&scene, 24, &settings);
    let at = |x: u32, y: u32| p.beauty.pixels[(y * 160 + x) as usize];
    // The ball's lit side (upper left) beats its shadowed side.
    let (lit, dark) = (lum(at(74, 40)), lum(at(86, 50)));
    assert!(lit > dark * 1.3, "lit {lit} dark {dark}");
    // Its shadow falls on the floor to the lower right of the ball.
    let open = lum(at(40, 80));
    let shade = (60..90).map(|x| lum(at(x, 62))).fold(f32::INFINITY, f32::min);
    assert!(shade < open * 0.8, "shadow {shade} vs open floor {open}");
    // Above the horizon nothing was hit: transparent, no depth.
    assert_eq!(at(80, 2)[3], 0.0);
    assert!(p.depth[2 * 160 + 80].is_infinite());
    assert!(p.depth[45 * 160 + 80].is_finite() && p.id[45 * 160 + 80] == 2);
    // Passes come out as pictures.
    assert_eq!(p.depth_frame(160, 90).pixels.len(), 160 * 90);
    assert_eq!(p.id_frame(160, 90).pixels[45 * 160 + 80][3], 1.0);
}

#[test]
fn a_sky_shows_behind_and_a_glowing_panel_lights_a_room() {
    let canvas = Canvas::new(96, 54, 24.0);
    let mut desc = ball_on_floor(&canvas);
    let sky = studio_render::sky::SkyDesc { sun: studio_render::sky::sun_for_time(15.0, 172.0, 40.0, 0.0), ..Default::default() };
    desc.lights = vec![LightDesc { sky: Some(sky), colour: studio_render::sky::sun_colour(&sky), direction: sky.sun, ..sun([0.0, 1.0, 0.0]) }];
    let s = trace::scene::Scene::build(&desc, &canvas, (96, 54));
    assert!(s.background && !s.env.is_empty());
    let p = trace::cpu::render(&s, 8, &TraceSettings { samples: 8, denoise: false, ..Default::default() });
    let top = p.beauty.pixels[2 * 96 + 48];
    assert!(top[3] == 1.0 && top[2] > top[0], "blue sky behind: {top:?}");
    // A closed box with a glowing ceiling panel and no lights at all.
    let mut room = studio_mesh::prim::cube([400.0, 300.0, 400.0]);
    studio_mesh::ops::flip(&mut room);
    let mut panel = studio_mesh::prim::cube([150.0, 4.0, 150.0]);
    panel.materials[0].emissive = [6.0; 3];
    let cam = CameraDesc { position: [0.0, 0.0, -150.0], ..CameraDesc::default_for(&canvas) };
    let desc = MeshDesc { instances: vec![instance(1, room, [0.0; 3]), instance(3, panel, [0.0, -145.0, 0.0])], camera: Some(cam), lights: vec![ambient(0.0)] };
    let s = trace::scene::Scene::build(&desc, &canvas, (96, 54));
    assert!(!s.emit.is_empty());
    let p = trace::cpu::render(&s, 16, &TraceSettings { samples: 16, denoise: true, ..Default::default() });
    let floor = lum(p.beauty.pixels[50 * 96 + 48]);
    assert!(floor > 0.05, "the panel lights the floor: {floor}");
}

#[test]
fn smoke_dims_what_is_behind_it() {
    let canvas = Canvas::new(64, 36, 24.0);
    let mut wall = studio_mesh::prim::cube([800.0, 800.0, 10.0]);
    wall.materials[0].base_colour = [0.9, 0.9, 0.9, 1.0];
    let mut puff = Mesh::default();
    puff.materials.push(studio_mesh::Material::default());
    puff.volume = Some(Arc::new(studio_mesh::Volume { dims: [4, 4, 4], min: [-60.0; 3], max: [60.0; 3], cells: vec![[255, 0]; 64], look: studio_mesh::VolumeLook { colour: [0.1, 0.1, 0.1], density: 0.08, fire: 0.0 } }));
    let base = MeshDesc { instances: vec![instance(1, wall.clone(), [0.0, 0.0, 300.0])], camera: Some(CameraDesc::default_for(&canvas)), lights: vec![ambient(1.0)] };
    let mut smoky = base.clone();
    smoky.instances.push(instance(2, puff, [0.0, 0.0, 0.0]));
    let settings = TraceSettings { samples: 8, denoise: false, ..Default::default() };
    let a = trace::cpu::render(&trace::scene::Scene::build(&base, &canvas, (64, 36)), 8, &settings);
    let b = trace::cpu::render(&trace::scene::Scene::build(&smoky, &canvas, (64, 36)), 8, &settings);
    let c = 18 * 64 + 32;
    assert!(lum(b.beauty.pixels[c]) < lum(a.beauty.pixels[c]) * 0.6, "{:?} {:?}", b.beauty.pixels[c], a.beauty.pixels[c]);
    // The raster renderer draws it too.
    let r = studio_render::mesh_render::render(&smoky, &canvas, (64, 36));
    let r0 = studio_render::mesh_render::render(&base, &canvas, (64, 36));
    assert!(lum(r.pixels[c]) < lum(r0.pixels[c]) * 0.8);
}

#[test]
fn the_gpu_agrees_with_the_cpu() {
    let canvas = Canvas::new(96, 54, 24.0);
    let desc = ball_on_floor(&canvas);
    let s = trace::scene::Scene::build(&desc, &canvas, (96, 54));
    let settings = TraceSettings { samples: 64, denoise: false, ..Default::default() };
    let Some(g) = trace::gpu::render(&s, 64, &settings) else {
        eprintln!("no GPU here: agreement not checked");
        return;
    };
    let c = trace::cpu::render(&s, 64, &settings);
    let mean = |f: &studio_render::frame::Frame| f.pixels.iter().map(|p| lum(*p)).sum::<f32>() / f.pixels.len() as f32;
    let (mg, mc) = (mean(&g.beauty), mean(&c.beauty));
    assert!((mg - mc).abs() < mc * 0.08 + 0.005, "gpu {mg} cpu {mc}");
    // Region by region too (4×4 blocks of the picture).
    for by in 0..4 {
        for bx in 0..4 {
            let block = |f: &studio_render::frame::Frame| {
                let mut s = 0.0;
                for y in by * 13..(by + 1) * 13 {
                    for x in bx * 24..(bx + 1) * 24 {
                        s += lum(f.pixels[y * 96 + x]);
                    }
                }
                s / (13.0 * 24.0)
            };
            let (a, b) = (block(&g.beauty), block(&c.beauty));
            assert!((a - b).abs() < b * 0.15 + 0.02, "block {bx},{by}: gpu {a} cpu {b}");
        }
    }
    assert_eq!(g.id[27 * 96 + 48], c.id[27 * 96 + 48]);
}

#[test]
fn a_moving_ball_blurs_along_its_path() {
    let canvas = Canvas::new(96, 54, 24.0);
    let mut desc = ball_on_floor(&canvas);
    desc.instances.remove(0);
    desc.lights = vec![ambient(1.0)];
    let ball = desc.instances[0].clone();
    let mut world = ball.world;
    world[0][3] += 120.0;
    desc.instances[0].motion = Some(Arc::new(studio_render::mesh_render::Motion { mesh: ball.mesh.clone(), key: "next".into(), placement: Default::default(), space: Default::default(), world }));
    let s = trace::scene::Scene::build(&desc, &canvas, (96, 54));
    assert!(s.moving);
    let p = trace::cpu::render(&s, 32, &TraceSettings { samples: 32, shutter: 0.5, denoise: false, ..Default::default() });
    // Covered partly along the path, solidly nowhere at its far end.
    let row: Vec<f32> = (0..96).map(|x| p.beauty.pixels[27 * 96 + x][3]).collect();
    let partial = row.iter().filter(|a| **a > 0.1 && **a < 0.9).count();
    assert!(partial > 10, "{row:?}");
}
