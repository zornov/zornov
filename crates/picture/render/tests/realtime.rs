// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The real-time world renderer: its shaders validate, and a generated
//! world drawn headless from two camera positions shows real depth.

use studio_render::GpuContext;
use studio_render::realtime::{Options, Quality, Renderer, frame::View, gpu::sources};
use studio_world::build::{NoLibrary, WorldSpec, build};

fn renderer() -> Option<Renderer> {
    let ctx = GpuContext::new()?;
    Renderer::new(ctx).map_err(|e| panic!("{e}")).ok()
}

fn dump(name: &str, img: &studio_render::realtime::Image) {
    if let Ok(dir) = std::env::var("RT_DUMP") {
        std::fs::create_dir_all(&dir).unwrap();
        let png = studio_media::png::encode(&studio_media::RgbaImage { width: img.width, height: img.height, pixels: img.rgba.clone() });
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png).unwrap();
    }
}

fn spec(preset: &str, size: f32) -> WorldSpec {
    WorldSpec { preset: preset.into(), size: Some(size), quality: "fast".into(), seed: Some(5), ..WorldSpec::default() }
}

#[test]
fn every_shader_module_validates() {
    for (name, src) in sources() {
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

#[test]
fn a_forest_renders() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let w = build(&spec("forest", 240.0), &NoLibrary).unwrap();
    let mut v = View::from_pose(&studio_world::walk::Pose { pos: [w.spawn.position[0], w.spawn.position[1] + 1.62, w.spawn.position[2]], yaw: w.spawn.yaw, pitch: 0.0, fov: 60.0 });
    v.far = 800.0;
    let o = Options::new(640, 360);
    let (img, st) = r.render_srgb(&w, &v, &o).unwrap();
    eprintln!("{st:?}");
    dump("forest_a", &img);
    assert_eq!(img.rgba.len(), 640 * 360 * 4);
    let _ = Quality::Fast;
}

/// A look round every preset, for people to see (RT_GALLERY=1 RT_DUMP=dir).
#[test]
fn gallery() {
    if std::env::var_os("RT_GALLERY").is_none() {
        return;
    }
    let Some(mut r) = renderer() else { return };
    for (preset, size, hour) in [("forest", 240.0, None), ("meadow", 240.0, None), ("desert", 240.0, None), ("mountains", 400.0, None), ("island", 300.0, None), ("canyon", 300.0, None), ("village", 240.0, None), ("city", 300.0, None), ("interior", 60.0, None), ("scifi", 200.0, None), ("city", 300.0, Some(23.0)), ("forest", 240.0, Some(6.4))] {
        let mut s = spec(preset, size);
        if let Some(h) = hour {
            s.sky = Some(studio_world::build::SkyJ { time: Some(studio_world::build::Time::Hours(h)), ..Default::default() });
        }
        let t = std::time::Instant::now();
        let w = build(&s, &NoLibrary).unwrap();
        let built = t.elapsed();
        let mut v = View::from_pose(&studio_world::walk::Pose { pos: [w.spawn.position[0], w.spawn.position[1] + 1.62, w.spawn.position[2]], yaw: w.spawn.yaw, pitch: 0.0, fov: 62.0 });
        v.far = 1200.0;
        let (img, st) = r.render_srgb(&w, &v, &Options::new(800, 450)).unwrap();
        let mean = img.rgba.chunks_exact(4).map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).sum::<f32>() / (img.rgba.len() / 4) as f32;
        eprintln!("{preset} {hour:?}: built in {built:?}, {:.0} ms, mean {mean:.0}/255, exposure {:.2}, {st:?}", st.ms, studio_render::realtime::build_env(&w.sky, w.look.exposure).exposure);
        dump(&format!("{preset}{}", hour.map_or(String::new(), |h| format!("_{h}"))), &img);
    }
}

fn poles_world() -> studio_world::world::World {
    use studio_world::asset::Asset;
    use studio_world::kit::{MeshBuilder, mat};
    use studio_world::terrain::Terrain;
    use studio_world::world::{Instance, ObjectKind, World};
    let mut w = World::new("poles");
    w.terrain = Some(Terrain::flat([400.0, 400.0], 1.0));
    for (name, colour, z) in [("red", [1.0, 0.04, 0.04], 6.0f32), ("green", [0.04, 1.0, 0.04], 60.0)] {
        let mut b = MeshBuilder::default();
        let m = b.material(mat(name, colour, 0.9, 0.0));
        b.cylinder([0.0, 0.0, 0.0], 0.3, 7.0, 12, m);
        let a = w.add_asset(Asset::new(name, b.finish(), &[]));
        w.place(name, a, ObjectKind::Prop, Instance::at([0.0, 0.0, z]), false);
    }
    w.sky.time = 12.5;
    w.finish();
    w
}

/// Mean column of pixels the test calls red / green.
fn centroid(img: &studio_render::realtime::Image, red: bool) -> Option<f32> {
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for y in 0..img.height as usize {
        for x in 0..img.width as usize {
            let p = &img.rgba[(y * img.width as usize + x) * 4..][..3];
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let hit = if red { r > 70.0 && r > g * 2.5 && r > b * 2.5 } else { g > 70.0 && g > r * 2.2 && g > b * 2.2 };
            if hit {
                sum += x as f32;
                n += 1.0;
            }
        }
    }
    (n > 20.0).then(|| sum / n)
}

#[test]
fn moving_the_camera_sideways_shifts_near_things_far_more_than_far_things() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let w = poles_world();
    let shoot = |r: &mut Renderer, x: f32| {
        let mut v = View::from_pose(&studio_world::walk::Pose { pos: [x, 1.6, 0.0], yaw: 0.0, pitch: 0.0, fov: 60.0 });
        v.far = 500.0;
        let mut o = Options::new(640, 360);
        o.quality = Quality::Fast;
        r.render_srgb(&w, &v, &o).unwrap().0
    };
    let (a, b) = (shoot(&mut r, -1.0), shoot(&mut r, 1.0));
    dump("poles_left", &a);
    dump("poles_right", &b);
    let (near_a, near_b) = (centroid(&a, true).expect("near pole seen"), centroid(&b, true).expect("near pole seen"));
    let (far_a, far_b) = (centroid(&a, false).expect("far pole seen"), centroid(&b, false).expect("far pole seen"));
    let (near_shift, far_shift) = (near_a - near_b, far_a - far_b);
    // f = 180 / tan(30 deg) = 311.8 px; shift = f * 2 m / distance.
    assert!((near_shift - 311.8 * 2.0 / 6.0).abs() < 8.0, "near pole moved {near_shift} px, expected ~104");
    assert!((far_shift - 311.8 * 2.0 / 60.0).abs() < 3.0, "far pole moved {far_shift} px, expected ~10");
    assert!(near_shift > far_shift * 6.0, "parallax: {near_shift} vs {far_shift}");
}

/// Mean absolute difference of two bands of rows after shifting b by dx.
fn band_shift(a: &studio_render::realtime::Image, b: &studio_render::realtime::Image, y0: usize, y1: usize) -> i32 {
    let w = a.width as usize;
    let luma = |img: &studio_render::realtime::Image, x: usize, y: usize| {
        let p = &img.rgba[(y * w + x) * 4..][..3];
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let mut best = (f32::MAX, 0i32);
    for dx in -60i32..=60 {
        let mut sad = 0.0;
        let mut n = 0.0;
        for y in y0..y1 {
            for x in 70..w - 70 {
                let xb = (x as i32 + dx) as usize;
                sad += (luma(a, x, y) - luma(b, xb, y)).abs();
                n += 1.0;
            }
        }
        if sad / n < best.0 {
            best = (sad / n, dx);
        }
    }
    best.1
}

#[test]
fn a_generated_forest_seen_from_two_places_has_depth_the_ground_slides_faster_than_the_horizon() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let w = build(&spec("forest", 240.0), &NoLibrary).unwrap();
    let base = studio_world::walk::Pose { pos: [w.spawn.position[0], w.spawn.position[1] + 1.62, w.spawn.position[2]], yaw: w.spawn.yaw, pitch: -4.0, fov: 60.0 };
    let side = base.right();
    let shoot = |r: &mut Renderer, k: f32| {
        let mut p = base;
        for i in 0..3 {
            p.pos[i] += side[i] * k;
        }
        let mut v = View::from_pose(&p);
        v.far = 800.0;
        let mut o = Options::new(640, 360);
        o.quality = Quality::Fast;
        r.render_srgb(&w, &v, &o).unwrap().0
    };
    let (a, b) = (shoot(&mut r, -0.5), shoot(&mut r, 0.5));
    dump("forest_left", &a);
    dump("forest_right", &b);
    assert_ne!(a.rgba, b.rgba);
    // Rows near the bottom see the ground a few metres away; rows near the
    // horizon see the far trees.
    let ground = band_shift(&a, &b, 300, 350);
    let far = band_shift(&a, &b, 150, 190);
    assert!(ground.abs() > far.abs() + 3, "ground shifts {ground} px, far trees {far} px");
}

#[test]
fn a_still_camera_refines_and_a_moving_one_resets() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let w = poles_world();
    let mut v = View::from_pose(&studio_world::walk::Pose { pos: [-1.0, 1.6, 0.0], yaw: 0.0, pitch: 0.0, fov: 60.0 });
    v.far = 500.0;
    let mut o = Options::new(320, 180);
    o.refine = true;
    let (first, s1) = r.render_srgb(&w, &v, &o).unwrap();
    assert_eq!(s1.refined, 1);
    let mut last = first.clone();
    let mut n = 0;
    for _ in 0..7 {
        let (img, st) = r.render_srgb(&w, &v, &o).unwrap();
        n = st.refined;
        last = img;
    }
    assert_eq!(n, 8, "frames averaged");
    assert_ne!(first.rgba, last.rgba, "the averaged picture differs from the first frame (smoother edges)");
    dump("refined_8", &last);
    // Move: back to one frame.
    v.position[0] += 0.5;
    let (_, st) = r.render_srgb(&w, &v, &o).unwrap();
    assert_eq!(st.refined, 1);
    // Capped.
    o.max_refine = 4;
    v.position[0] += 0.0;
    let mut last = 0;
    for _ in 0..8 {
        last = r.render_srgb(&w, &v, &o).unwrap().1.refined;
    }
    assert_eq!(last, 4);
}

#[test]
fn frame_times_of_a_busy_forest_at_720p_and_1080p() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    eprintln!("adapter: {}", r.context().adapter_name);
    let mut median_fast = 0.0;
    for (worldq, label0) in [("fast", "low-poly"), ("high", "modeled kits")] {
    let mut s = spec("forest", 400.0);
    s.quality = worldq.into();
    let t0 = std::time::Instant::now();
    let w = build(&s, &NoLibrary).unwrap();
    eprintln!("forest {label0}: built in {:?}, {} triangles in all", t0.elapsed(), w.triangle_count());
    let base = studio_world::walk::Pose { pos: [w.spawn.position[0], w.spawn.position[1] + 1.62, w.spawn.position[2]], yaw: w.spawn.yaw, pitch: 0.0, fov: 65.0 };
    for (label, width, height, quality) in [("720p Fast (moving)", 1280u32, 720u32, Quality::Fast), ("1080p Balanced", 1920, 1080, Quality::Balanced), ("1080p High", 1920, 1080, Quality::High)] {
        let mut times = Vec::new();
        let mut tris = 0;
        for i in 0..40 {
            let mut p = base;
            p.yaw += i as f32 * 1.5;
            let mut v = View::from_pose(&p);
            v.far = 900.0;
            let mut o = Options::new(width, height);
            o.quality = quality;
            let (_, st) = r.render_srgb(&w, &v, &o).unwrap();
            times.push(st.ms);
            tris = st.triangles;
        }
        times.sort_by(|a, b| a.total_cmp(b));
        eprintln!("forest {label0} {label}: median {:.1} ms, p90 {:.1} ms, {tris} of {} triangles drawn", times[times.len() / 2], times[times.len() * 9 / 10], w.triangle_count());
        // The bar is for a moving camera in the low-poly forest; the modeled
        // kits' figures are printed for comparison, not held to it.
        if quality == Quality::Fast && worldq == "fast" {
            median_fast = times[times.len() / 2];
        }
    }
    }
    if r.context().software {
        // A frame-time bar on llvmpipe or WARP measures the CPU, not a GPU.
        eprintln!("software adapter: the 250 ms bar holds on GPUs only (median {median_fast:.1} ms here)");
        return;
    }
    assert!(median_fast < 250.0, "720p Fast in the low-poly forest: median {median_fast:.1} ms");
}

/// A close look at a house and a tree with and without ambient occlusion
/// and shadows (RT_GALLERY=1 RT_DUMP=dir), for people to compare.
#[test]
fn occlusion_gallery() {
    if std::env::var_os("RT_GALLERY").is_none() {
        return;
    }
    let Some(mut r) = renderer() else { return };
    let mut s = spec("village", 200.0);
    s.quality = "high".into();
    s.scatter = vec![studio_world::build::ScatterJ { kind: "pine".into(), count: 6, size: 12.0, near: Some([0.0, 14.0]), radius: Some(16.0), ..Default::default() }, studio_world::build::ScatterJ { kind: "rock".into(), count: 8, size: 1.5, near: Some([4.0, 8.0]), radius: Some(6.0), max_slope: 90.0, ..Default::default() }];
    s.buildings = vec![studio_world::build::BuildingJ { kind: "house".into(), at: Some([0.0, 22.0]), floors: 2, width: 9.0, depth: 7.0, ..Default::default() }];
    s.props = vec![];
    s.characters = vec![];
    let mut w = build(&s, &NoLibrary).unwrap();
    let g = w.ground_y(0.0, 6.0);
    let pose = studio_world::walk::Pose { pos: [-3.0, g + 1.7, 6.0], yaw: 20.0, pitch: 4.0, fov: 55.0 };
    for (name, ao) in [("ao_off", 0.0f32), ("ao_on", 1.0)] {
        w.look.ssao = ao;
        w.finish();
        let mut o = Options::new(800, 450);
        o.quality = Quality::High;
        let (img, st) = r.render_srgb(&w, &View::from_pose(&pose), &o).unwrap();
        eprintln!("{name}: {:.0} ms {st:?}", st.ms);
        dump(name, &img);
    }
}

#[test]
fn ambient_occlusion_darkens_contacts_and_never_brightens() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut w = poles_world();
    let pose = studio_world::walk::Pose { pos: [-2.5, 2.0, 0.0], yaw: 0.0, pitch: -8.0, fov: 60.0 };
    let shoot = |r: &mut Renderer, w: &studio_world::world::World| {
        let mut o = Options::new(480, 270);
        o.quality = Quality::Balanced;
        r.render_srgb(w, &View::from_pose(&pose), &o).unwrap().0
    };
    w.look.ssao = 0.0;
    w.finish();
    let off = shoot(&mut r, &w);
    w.look.ssao = 1.0;
    w.finish();
    let on = shoot(&mut r, &w);
    let sum = |i: &studio_render::realtime::Image| i.rgba.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum::<u64>();
    let changed = off.rgba.chunks_exact(4).zip(on.rgba.chunks_exact(4)).filter(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).sum::<i32>() > 4).count();
    assert!(changed > 100, "occlusion shades the contact of the pole with the ground: {changed} pixels");
    assert!(sum(&on) < sum(&off), "darker overall: {} vs {}", sum(&on), sum(&off));
}

#[test]
fn a_hundred_times_the_trees_cost_a_few_more_draws_not_a_hundred_times() {
    let Some(mut r) = renderer() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let pose = studio_world::walk::Pose { pos: [0.0, 3.0, -100.0], yaw: 0.0, pitch: 0.0, fov: 70.0 };
    let draws = |r: &mut Renderer, count: usize| {
        let mut s = spec("meadow", 240.0);
        s.water = None;
        s.scatter = vec![studio_world::build::ScatterJ { kind: "pine".into(), count, size: 10.0, max_slope: 90.0, clumping: 0.0, ..Default::default() }];
        let w = build(&s, &NoLibrary).unwrap();
        let placed: usize = w.scatters.iter().map(|s| s.instances.len()).sum();
        let mut v = View::from_pose(&pose);
        v.far = 600.0;
        let mut o = Options::new(320, 180);
        o.quality = Quality::Fast;
        let (_, st) = r.render_srgb(&w, &v, &o).unwrap();
        (placed, st.draw_calls, st.drawn)
    };
    let (n_small, calls_small, drawn_small) = draws(&mut r, 20);
    let (n_big, calls_big, drawn_big) = draws(&mut r, 2000);
    assert!(n_big > n_small * 50, "{n_small} vs {n_big} trees placed");
    assert!(drawn_big > drawn_small * 20, "{drawn_small} vs {drawn_big} copies drawn");
    assert!(calls_big < calls_small * 3 + 40, "instancing: {calls_small} draw calls for {drawn_small} trees, {calls_big} for {drawn_big}");
}
