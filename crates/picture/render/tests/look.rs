// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Light, air and look: the pass that makes lamps glow, rain fall, fog hold
//! light, wet ground reflect and the picture finish like film. Each element
//! is rendered headless on the CPU path and on the GPU, and checked to show.

use studio_render::atmosphere::samples::*;

#[test]
fn the_shader_validates() {
    let src = studio_render::atmosphere::gpu::source();
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("atmosphere: {}", e.emit_to_string(src)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("atmosphere: {e:?}"));
}

/// Writes before and after pictures of the sample scenes when
/// `STUDIO_LOOK_DIR` names a folder.
#[test]
fn gallery() {
    let Some(dir) = std::env::var_os("STUDIO_LOOK_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (960, 540);
    type Make = fn(bool, u32, u32) -> Scene;
    let scenes: [(&str, Make); 5] = [("forest_dawn", forest_dawn), ("neon_alley", neon_alley), ("candlelit_room", candlelit_room), ("snowy_village", snowy_village), ("desert_noon", desert_noon)];
    let only = std::env::var("STUDIO_LOOK_ONLY").ok();
    for (name, make) in scenes {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        for (tag, cinematic) in [("before", false), ("after", true)] {
            let t = std::time::Instant::now();
            let s = make(cinematic, w, h);
            let f = render_on(&s, w, h, true);
            f.save_png(&dir.join(format!("{name}_{tag}.png"))).unwrap();
            if cinematic && std::env::var_os("STUDIO_LOOK_CPU").is_some() {
                render_on(&s, w, h, false).save_png(&dir.join(format!("{name}_{tag}_cpu.png"))).unwrap();
            }
            eprintln!("{name} {tag}: {:.1}s", t.elapsed().as_secs_f32());
        }
    }
}


use studio_render::Frame;
use studio_render::atmosphere::{Atmos, Bloom, Flame, Fog, Grade, LightLook, Tonemap, Weather, WeatherKind};
use studio_render::space::LightKind;

const W: u32 = 320;
const H: u32 = 180;

/// The backends to try: the CPU always, the GPU when there is one.
fn backends() -> Vec<(&'static str, bool)> {
    let mut v = vec![("cpu", false)];
    if studio_render::atmosphere::gpu::available() {
        v.push(("gpu", true));
    } else {
        eprintln!("no GPU: the GPU path is not checked");
    }
    v
}

fn mean_lum(f: &Frame) -> f32 {
    f.pixels.iter().map(|p| lum(*p)).sum::<f32>() / f.pixels.len() as f32
}

/// Pixels whose luminance differs between two frames by more than `thr`.
fn differing(a: &Frame, b: &Frame, thr: f32) -> usize {
    a.pixels.iter().zip(&b.pixels).filter(|(p, q)| (lum(**p) - lum(**q)).abs() > thr).count()
}

fn air_only(mut a: Atmos) -> Atmos {
    a.bloom = Bloom::default();
    a.grade = Grade { auto_exposure: false, ..Default::default() };
    a
}

/// A dark wall some way off, a floor, the camera at standing height.
fn dark_stage() -> Scene {
    let mut s = Scene::new(W, H);
    let m = s.m();
    s.camera([0.0, -1.5 * m, -3.0 * m], [0.0, -1.5 * m, 10.0 * m]);
    s.add(studio_mesh::prim::cube([40.0 * m, 12.0 * m, 0.5 * m]), [0.0, -3.0 * m, 22.0 * m], mat([0.03, 0.03, 0.035], 0.9));
    s.add(studio_mesh::prim::plane(40.0 * m, 40.0 * m, 4), [0.0, 0.0, 10.0 * m], mat([0.05, 0.05, 0.055], 0.9));
    s.light(ambient([0.03, 0.035, 0.05]));
    s
}

fn with_air(s: &mut Scene, a: Atmos) {
    s.desc.lights.retain(|l| !l.look.is_some_and(|k| k.atmos.is_some()));
    s.light(air(a));
}

#[test]
fn rain_is_visible_against_a_dark_backdrop() {
    for (name, gpu) in backends() {
        let mut s = dark_stage();
        let m = s.m();
        // A lamp beside the view lights the rain.
        s.light(with_look(base_light(LightKind::Point, [2.0 * m, -3.5 * m, 6.0 * m], [0.0, 1.0, 0.0], [1.0, 0.85, 0.6]), lamp_look(0.0, 0.0, 0.0, 0.1)));
        let dry = render_on(&s, W, H, gpu);
        let mut a = Atmos { metre: m, ..Default::default() };
        a.weather[0] = Some(Weather::new(WeatherKind::Rain, 0.8));
        with_air(&mut s, a);
        let wet = render_on(&s, W, H, gpu);
        let n = differing(&dry, &wet, 0.04);
        let share = n as f32 / (W * H) as f32;
        assert!(share > 0.0012, "{name}: rain streaks cover {:.3}% of a dark frame", share * 100.0);
        // Streaks are up and down: a bright pixel has a bright neighbour above or below.
        let w = W as usize;
        let bright = |x: usize, y: usize| lum(wet.pixels[y * w + x]) - lum(dry.pixels[y * w + x]) > 0.03;
        let (mut tall, mut total) = (0, 0);
        for y in 2..H as usize - 2 {
            for x in 0..w {
                if bright(x, y) {
                    total += 1;
                    if bright(x, y - 1) || bright(x, y + 1) {
                        tall += 1;
                    }
                }
            }
        }
        assert!(total > 50 && tall as f32 / total as f32 > 0.5, "{name}: rain is not streaks ({tall}/{total})");
    }
}

#[test]
fn every_kind_of_weather_shows() {
    for (name, gpu) in backends() {
        for kind in WeatherKind::ALL {
            let mut s = dark_stage();
            let m = s.m();
            s.light(with_look(base_light(LightKind::Point, [1.0 * m, -2.5 * m, 4.0 * m], [0.0, 1.0, 0.0], [1.0, 0.9, 0.8].map(|c| c * 1.6)), lamp_look(0.0, 0.0, 0.0, 0.1)));
            let off = render_on(&s, W, H, gpu);
            let mut a = Atmos { metre: m, ..Default::default() };
            let mut w = Weather::new(kind, 1.0);
            // Smoke and embers stand over something; give them a place in front of the lens.
            if matches!(kind, WeatherKind::Smoke | WeatherKind::Embers | WeatherKind::Sparks) {
                w.zone = Some(([-2.0 * m, -3.0 * m, 3.0 * m], [2.0 * m, 0.0, 8.0 * m]));
            }
            a.weather[0] = Some(w);
            with_air(&mut s, a);
            let on = render_on(&s, W, H, gpu);
            let n = differing(&off, &on, 0.02);
            assert!(n > 40, "{name}: {} leaves {n} pixels changed", kind.name());
        }
    }
}

#[test]
fn wet_ground_reflects_a_light() {
    for (name, gpu) in backends() {
        let build = |wetness: f32| {
            let mut s = Scene::new(W, H);
            let m = s.m();
            s.camera([0.0, -1.3 * m, -3.0 * m], [0.0, -1.0 * m, 10.0 * m]);
            let mut road = mat([0.2, 0.2, 0.22], 0.8);
            if wetness > 0.0 {
                road = wet(road, wetness);
            }
            s.add(studio_mesh::prim::plane(60.0 * m, 60.0 * m, 4), [0.0, 0.0, 20.0 * m], road);
            // A lamp straight ahead and up.
            let lamp = [0.0, -3.2 * m, 30.0 * m];
            s.add(studio_mesh::prim::uv_sphere(0.15 * m, 10, 6), lamp, glowing([1.0, 0.8, 0.5], 8.0));
            s.light(with_look(base_light(LightKind::Point, lamp, [0.0, 1.0, 0.0], [1.0, 0.8, 0.5]), lamp_look(1.0, 0.0, 0.0, 0.15)));
            s.light(ambient([0.03, 0.03, 0.04]));
            s
        };
        let dry = render_on(&build(0.0), W, H, gpu);
        let wet_f = render_on(&build(0.9), W, H, gpu);
        // Below the horizon, in the column under the lamp.
        let (x0, x1) = (W as usize / 2 - 8, W as usize / 2 + 8);
        let rows = (H as usize * 3 / 5)..H as usize;
        let sum = |f: &Frame| -> f32 { rows.clone().map(|y| (x0..x1).map(|x| lum(f.pixels[y * W as usize + x])).sum::<f32>()).sum() };
        let (d, w) = (sum(&dry), sum(&wet_f));
        assert!(w > d * 1.8 + 0.5, "{name}: wet ground below the lamp {w:.2} against dry {d:.2}");
        // Reflected light is stretched down the ground, not a dot: bright rows span many.
        let col_bright = rows.clone().filter(|y| (x0..x1).any(|x| lum(wet_f.pixels[y * W as usize + x]) > 0.25)).count();
        assert!(col_bright > 12, "{name}: the reflection is only {col_bright} rows tall");
    }
}

#[test]
fn a_volumetric_beam_appears_in_fog() {
    for (name, gpu) in backends() {
        let build = |fog: bool, beam: f32| {
            let mut s = dark_stage();
            let m = s.m();
            let top = [0.0, -4.2 * m, 8.0 * m];
            s.light(with_look(spot(top, [0.0, 0.0, 8.0 * m], [1.0, 0.95, 0.85].map(|c| c * 1.6), 28.0, false), LightLook { beam, ..Default::default() }));
            if fog {
                let mut a = air_only(Atmos { metre: m, ..Default::default() });
                a.fog = Some(Fog { density: 0.04, falloff: 0.0, base_y: 0.0, colour: [1.0, 1.0, 1.0], anisotropy: 0.3, range: 40.0, patchiness: 0.0 });
                with_air(&mut s, a);
            }
            s
        };
        let none = render_on(&build(false, 0.0), W, H, gpu);
        let foggy = render_on(&build(true, 2.0), W, H, gpu);
        // The cone: the column under the lamp between the lamp and the floor, against the same rows at the edge.
        let w = W as usize;
        let mid: f32 = (40..110).map(|y| (w / 2 - 6..w / 2 + 6).map(|x| lum(foggy.pixels[y * w + x]) - lum(none.pixels[y * w + x])).sum::<f32>()).sum::<f32>() / (70.0 * 12.0);
        let side: f32 = (40..110).map(|y| (10..22).map(|x| lum(foggy.pixels[y * w + x]) - lum(none.pixels[y * w + x])).sum::<f32>()).sum::<f32>() / (70.0 * 12.0);
        assert!(mid > 0.05 && mid > side * 3.0, "{name}: beam {mid:.3} against its surroundings {side:.3}");
    }
}

#[test]
fn a_blocker_cuts_the_shaft() {
    // Light from the side through gaps: the shaft is brighter where nothing stands in its way.
    for (name, gpu) in backends() {
        let build = |blocker: bool| {
            let mut s = dark_stage();
            let m = s.m();
            let mut sun = base_light(LightKind::Parallel, [0.0; 3], [0.0, 0.7, 0.7], [1.2, 1.1, 0.9]);
            sun.casts_shadows = true;
            s.light(with_look(sun, LightLook { beam: 1.0, ..Default::default() }));
            if blocker {
                // A slatted screen high up, between the sun and the room.
                for i in 0..6 {
                    s.add(studio_mesh::prim::cube([0.5 * m, 0.1 * m, 9.0 * m]), [(-2.5 + i as f32 * 1.0) * m, -5.0 * m, 7.0 * m], mat([0.01, 0.01, 0.01], 0.9));
                }
            }
            let mut a = air_only(Atmos { metre: m, ..Default::default() });
            a.fog = Some(Fog { density: 0.05, falloff: 0.0, base_y: 0.0, colour: [1.0, 1.0, 1.0], anisotropy: 0.3, range: 30.0, patchiness: 0.0 });
            with_air(&mut s, a);
            s
        };
        let open = render_on(&build(false), W, H, gpu);
        let slatted = render_on(&build(true), W, H, gpu);
        // Across the middle rows the slatted shaft alternates bright and dark; the open one does not.
        let w = W as usize;
        let spread = |f: &Frame| -> f32 {
            let row: Vec<f32> = (w / 4..3 * w / 4).map(|x| (60..90).map(|y| lum(f.pixels[y * w + x])).sum::<f32>() / 30.0).collect();
            let mean = row.iter().sum::<f32>() / row.len() as f32;
            (row.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / row.len() as f32).sqrt() / mean.max(1e-3)
        };
        assert!(spread(&slatted) > spread(&open) * 1.8 + 0.02, "{name}: slatted {:.3} open {:.3}", spread(&slatted), spread(&open));
    }
}

#[test]
fn a_lamp_glows_and_a_flat_one_does_not() {
    for (name, gpu) in backends() {
        let build = |glow: f32| {
            let mut s = dark_stage();
            let m = s.m();
            let at = [0.0, -2.0 * m, 8.0 * m];
            s.add(studio_mesh::prim::uv_sphere(0.15 * m, 12, 8), at, glowing([1.0, 0.8, 0.5], 4.0));
            s.light(with_look(base_light(LightKind::Point, at, [0.0, 1.0, 0.0], [1.0, 0.8, 0.5]), lamp_look(glow, 0.0, 0.0, 0.15)));
            s
        };
        let flat = render_on(&build(0.0), W, H, gpu);
        let glowing_f = render_on(&build(1.0), W, H, gpu);
        // Rings at 2 to 5 times the bulb's size: light spills round a glowing lamp.
        let (cx, cy) = (W as usize / 2, H as usize / 2 - 12);
        let ring = |f: &Frame| -> f32 {
            let mut s = 0.0;
            let mut n = 0.0;
            for y in 0..H as usize {
                for x in 0..W as usize {
                    let d = (((x as f32 - cx as f32).powi(2) + (y as f32 - cy as f32).powi(2)) as f32).sqrt();
                    if (14.0..30.0).contains(&d) {
                        s += lum(f.pixels[y * W as usize + x]);
                        n += 1.0;
                    }
                }
            }
            s / n
        };
        assert!(ring(&glowing_f) > ring(&flat) * 2.0 + 0.01, "{name}: halo {:.4} against none {:.4}", ring(&glowing_f), ring(&flat));
    }
}

#[test]
fn a_flame_is_a_flame_not_an_oval() {
    for (name, gpu) in backends() {
        let build = |t: f32| {
            let mut s = Scene::new(W, H);
            let m = s.m();
            s.camera([0.0, -1.0 * m, -1.0 * m], [0.0, -1.0 * m, 4.0 * m]);
            s.add(studio_mesh::prim::cube([6.0 * m, 4.0 * m, 0.2 * m]), [0.0, -1.0 * m, 5.0 * m], mat([0.1, 0.09, 0.08], 0.9));
            let at = [0.0, -1.0 * m, 3.0 * m];
            let k = LightLook { glow: 0.4, size: 0.05, time: t, flame: Some(Flame { height: 0.45, width: 0.2, tongues: 3.0, seed: 0.2 }), ..Default::default() };
            s.light(with_look(base_light(LightKind::Point, at, [0.0, 1.0, 0.0], [1.0, 0.5, 0.15].map(|c| c * 2.0)), k));
            s.light(ambient([0.02, 0.02, 0.025]));
            s
        };
        let a = render_on(&build(0.0), W, H, gpu);
        let b = render_on(&build(0.37), W, H, gpu);
        // Warm pixels (red above green above blue) well above the background.
        let w = W as usize;
        let warm = |f: &Frame| -> Vec<(usize, usize)> {
            let mut v = Vec::new();
            for y in 0..H as usize {
                for x in 0..w {
                    let p = f.pixels[y * w + x];
                    if p[0] > 0.5 && p[0] > p[1] * 1.1 && p[1] > p[2] * 1.3 {
                        v.push((x, y));
                    }
                }
            }
            v
        };
        let wa = warm(&a);
        assert!(wa.len() > 150, "{name}: only {} flame pixels", wa.len());
        let (x0, x1) = (wa.iter().map(|p| p.0).min().unwrap(), wa.iter().map(|p| p.0).max().unwrap());
        let (y0, y1) = (wa.iter().map(|p| p.1).min().unwrap(), wa.iter().map(|p| p.1).max().unwrap());
        assert!((y1 - y0) as f32 > (x1 - x0) as f32 * 0.9, "{name}: the flame is {}x{} px, not tall", x1 - x0, y1 - y0);
        // It has a hot core and a cooler edge (not one flat colour).
        let hottest = wa.iter().map(|p| a.pixels[p.1 * w + p.0][1]).fold(0.0f32, f32::max);
        let coolest = wa.iter().map(|p| a.pixels[p.1 * w + p.0][1]).fold(f32::MAX, f32::min);
        assert!(hottest > coolest * 2.0, "{name}: flat flame {coolest:.2}..{hottest:.2}");
        // And it moves.
        assert!(differing(&a, &b, 0.03) > 20, "{name}: the flame does not flicker");
    }
}

#[test]
fn emissive_surfaces_bloom() {
    for (name, gpu) in backends() {
        let build = |bloom: f32| {
            let mut s = dark_stage();
            let m = s.m();
            s.add(studio_mesh::prim::cube([1.5 * m, 0.4 * m, 0.1 * m]), [0.0, -1.8 * m, 8.0 * m], glowing([0.2, 0.7, 1.0], 6.0));
            let mut a = air_only(Atmos { metre: m, ..Default::default() });
            a.bloom = Bloom { strength: bloom, threshold: 0.9, knee: 0.5, radius: 0.5 };
            with_air(&mut s, a);
            s
        };
        let plain = render_on(&build(0.0), W, H, gpu);
        let bloomed = render_on(&build(0.4), W, H, gpu);
        // Light spills past the sign's edge.
        assert!(differing(&plain, &bloomed, 0.02) > 400, "{name}: only {} pixels changed", differing(&plain, &bloomed, 0.02));
        assert!(mean_lum(&bloomed) > mean_lum(&plain) * 1.15, "{name}: no spill");
    }
}

#[test]
fn fog_thickens_with_distance_and_height_fog_pools() {
    for (name, gpu) in backends() {
        let mut s = Scene::new(W, H);
        let m = s.m();
        s.camera([0.0, -1.6 * m, -3.0 * m], [0.0, -1.6 * m, 10.0 * m]);
        s.add(studio_mesh::prim::plane(60.0 * m, 80.0 * m, 4), [0.0, 0.0, 30.0 * m], mat([0.4, 0.4, 0.4], 0.9));
        s.add(studio_mesh::prim::cube([3.0 * m, 4.0 * m, 1.0 * m]), [-2.0 * m, -2.0 * m, 4.0 * m], mat([0.8, 0.2, 0.1], 0.9));
        s.add(studio_mesh::prim::cube([3.0 * m, 4.0 * m, 1.0 * m]), [2.0 * m, -2.0 * m, 28.0 * m], mat([0.8, 0.2, 0.1], 0.9));
        s.light(base_light(LightKind::Parallel, [0.0; 3], [0.2, 1.0, 0.3], [1.0, 1.0, 1.0]));
        s.light(ambient([0.3, 0.3, 0.35]));
        let clear = render_on(&s, W, H, gpu);
        let mut a = air_only(Atmos { metre: m, ..Default::default() });
        a.fog = Some(Fog { density: 0.05, falloff: 0.0, base_y: 0.0, colour: [0.8, 0.85, 0.9], anisotropy: 0.2, range: 80.0, patchiness: 0.0 });
        with_air(&mut s, a);
        let foggy = render_on(&s, W, H, gpu);
        let w = W as usize;
        // The far block is hidden more than the near one.
        let red = |f: &Frame, x: usize| f.pixels[90 * w + x][0] - f.pixels[90 * w + x][2];
        let (near_clear, near_fog) = (red(&clear, 100), red(&foggy, 100));
        let (far_clear, far_fog) = (red(&clear, 190), red(&foggy, 190));
        assert!(near_clear > 0.15 && far_clear > 0.1, "{name}: test blocks not seen ({near_clear} {far_clear})");
        let near_keep = near_fog / near_clear;
        let far_keep = far_fog / far_clear;
        assert!(near_keep > far_keep * 1.5, "{name}: near keeps {near_keep:.2}, far {far_keep:.2}");
    }
}

#[test]
fn the_look_grades_with_restraint() {
    for (name, gpu) in backends() {
        let build = |g: Grade| {
            let mut s = Scene::new(W, H);
            let m = s.m();
            s.camera([0.0, -1.0 * m, -3.0 * m], [0.0, -1.0 * m, 10.0 * m]);
            s.add(studio_mesh::prim::cube([60.0 * m, 20.0 * m, 0.5 * m]), [0.0, -1.0 * m, 12.0 * m], mat([0.5, 0.5, 0.5], 0.9));
            // A very bright orange light and a very dark colourful patch.
            s.add(studio_mesh::prim::cube([2.0 * m, 1.5 * m, 0.1 * m]), [-2.0 * m, -1.0 * m, 8.0 * m], glowing([1.0, 0.35, 0.05], 5.0));
            s.add(studio_mesh::prim::cube([2.0 * m, 1.5 * m, 0.1 * m]), [2.5 * m, -1.0 * m, 8.0 * m], mat([0.02, 0.01, 0.04], 0.9));
            s.light(ambient([0.7, 0.7, 0.7]));
            let mut a = Atmos { metre: m, grade: g, ..Default::default() };
            a.bloom = Bloom::default();
            s.light(air(a));
            s
        };
        let none = render_on(&build(Grade::default()), W, H, gpu);
        let filmic = render_on(&build(Grade { tonemap: Tonemap::Filmic, ..Default::default() }), W, H, gpu);
        let w = W as usize;
        // The glowing patch keeps its colour under the filmic curve instead of clipping to flat white.
        let p = |f: &Frame| f.pixels[(H as usize / 2) * w + w / 2 - 70];
        let (n, f) = (p(&none), p(&filmic));
        assert!(f[0] <= 1.0 && f[1] < f[0] && f[2] < f[1], "{name}: filmic {:?}", f);
        assert!(n[1] > 1.0 || n[0] > 1.0, "{name}: the light is not over white in the plain picture: {n:?}");
        // Vignette darkens the corners and not the centre; grain varies a flat area; both modestly.
        let v = render_on(&build(Grade { vignette: 0.5, grain: 0.5, ..Default::default() }), W, H, gpu);
        let corner = lum(v.pixels[3 * w + 3]) / lum(none.pixels[3 * w + 3]).max(1e-4);
        let centre = lum(v.pixels[(H as usize / 2) * w + w / 2 + 20]) / lum(none.pixels[(H as usize / 2) * w + w / 2 + 20]).max(1e-4);
        assert!(corner < 0.8 && centre > 0.9, "{name}: vignette corner {corner:.2} centre {centre:.2}");
        let flat: Vec<f32> = (0..30).map(|i| lum(v.pixels[20 * w + 100 + i]) - lum(none.pixels[20 * w + 100 + i])).collect();
        let mean = flat.iter().sum::<f32>() / 30.0;
        let sd = (flat.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / 30.0).sqrt();
        assert!(sd > 0.0005 && sd < 0.06, "{name}: grain sd {sd}");
        // Rich darks: the dark patch gets deeper and keeps its hue.
        let rich = render_on(&build(Grade { tonemap: Tonemap::Filmic, richness: 1.0, ..Default::default() }), W, H, gpu);
        let dark = |f: &Frame| f.pixels[(H as usize / 2) * w + w / 2 + 80];
        assert!(lum(dark(&rich)) <= lum(dark(&filmic)) + 1e-4, "{name}: richer darks are not deeper");
    }
}

#[test]
fn the_gpu_agrees_with_the_cpu() {
    if !studio_render::atmosphere::gpu::available() {
        eprintln!("no GPU: skipped");
        return;
    }
    for (label, make) in [("neon_alley", neon_alley as fn(bool, u32, u32) -> Scene), ("snowy_village", snowy_village), ("forest_dawn", forest_dawn)] {
        let s = make(true, W, H);
        let cpu = render_on(&s, W, H, false);
        let gpu = render_on(&s, W, H, true);
        let mean_abs = cpu.pixels.iter().zip(&gpu.pixels).map(|(a, b)| (lum(*a) - lum(*b)).abs()).sum::<f32>() / cpu.pixels.len() as f32;
        let mean = mean_lum(&cpu).max(1e-3);
        assert!(mean_abs / mean < 0.06, "{label}: the two differ by {:.1}% of the picture's light", mean_abs / mean * 100.0);
    }
}

#[test]
fn the_pass_runs_over_a_path_traced_picture() {
    let mut s = dark_stage();
    let m = s.m();
    s.desc.camera = s.desc.camera.map(|mut c| {
        c.trace = Some(studio_render::trace::TraceSettings { samples: 4, preview_samples: 4, bounces: 2, denoise: false, ..Default::default() });
        c
    });
    s.light(base_light(LightKind::Point, [0.0, -3.0 * m, 8.0 * m], [0.0, 1.0, 0.0], [1.0, 0.8, 0.5]));
    let (w, h) = (96u32, 54u32);
    let plain = studio_render::mesh_render::render(&s.desc, &s.canvas, (w, h));
    let mut a = air_only(Atmos { metre: m, ..Default::default() });
    a.fog = Some(Fog { density: 0.12, falloff: 0.0, base_y: 0.0, colour: [1.0, 1.0, 1.0], anisotropy: 0.4, range: 40.0, patchiness: 0.0 });
    with_air(&mut s, a);
    let foggy = studio_render::mesh_render::render(&s.desc, &s.canvas, (w, h));
    assert!(mean_lum(&foggy) > mean_lum(&plain) * 1.1, "fog did not show over the path tracer: {} against {}", mean_lum(&foggy), mean_lum(&plain));
}

/// A pixel is blown when every channel is at or near white.
fn white_share(f: &Frame) -> f32 {
    f.pixels.iter().filter(|p| p[0].min(p[1]).min(p[2]) >= 0.9).count() as f32 / f.pixels.len() as f32
}

#[test]
fn a_glow_stronger_than_white_is_brighter_and_blooms() {
    for (name, gpu) in backends() {
        let build = |k: f32| {
            let mut s = dark_stage();
            let m = s.m();
            let mut o = mat([0.02, 0.02, 0.02], 0.5);
            o.emissive = Some([1.0, 0.45, 0.15]);
            o.emissive_strength = Some(k);
            s.add(studio_mesh::prim::uv_sphere(0.3 * m, 14, 8), [0.0, -1.5 * m, 8.0 * m], o);
            let mut a = air_only(Atmos { metre: m, ..Default::default() });
            a.bloom = Bloom { strength: 0.3, threshold: 0.9, knee: 0.5, radius: 0.5 };
            with_air(&mut s, a);
            s
        };
        let dim = render_on(&build(1.0), W, H, gpu);
        let hot = render_on(&build(6.0), W, H, gpu);
        let peak = |f: &Frame| f.pixels.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        assert!(peak(&hot) > peak(&dim) * 2.5, "{name}: peak {:.2} against {:.2}", peak(&hot), peak(&dim));
        // The bloom of the stronger glow reaches well outside the sphere.
        let (cx, cy) = (W as usize / 2, H as usize / 2 - 18);
        let ring = |f: &Frame| -> f32 {
            let w = W as usize;
            let mut sum = 0.0;
            let mut n = 0.0;
            for y in 0..H as usize {
                for x in 0..w {
                    let d = (((x as f32 - cx as f32).powi(2) + (y as f32 - cy as f32).powi(2)) as f32).sqrt();
                    if (22.0..40.0).contains(&d) {
                        sum += lum(f.pixels[y * w + x]);
                        n += 1.0;
                    }
                }
            }
            sum / n
        };
        assert!(ring(&hot) > ring(&dim) * 2.0, "{name}: halo {:.4} against {:.4}", ring(&hot), ring(&dim));
    }
}

#[test]
fn a_scene_whose_only_light_carries_the_air_still_lights_its_models() {
    for (name, gpu) in backends() {
        let mut s = Scene::new(W, H);
        let m = s.m();
        s.camera([0.0, -1.5 * m, -3.0 * m], [0.0, -1.0 * m, 10.0 * m]);
        s.add(studio_mesh::prim::uv_sphere(0.8 * m, 14, 8), [0.0, -1.0 * m, 6.0 * m], mat([0.7, 0.3, 0.2], 0.6));
        s.light(air(air_only(Atmos { metre: m, ..Default::default() })));
        let f = render_on(&s, W, H, gpu);
        assert!(f.pixels[(H as usize / 2) * W as usize + W as usize / 2][0] > 0.05, "{name}: the model is black: nothing lights it but the air's carrier");
    }
}

#[test]
fn bright_scenes_do_not_blow_out_by_default() {
    for (name, gpu) in backends() {
        for (label, make) in [("neon_alley", neon_alley as fn(bool, u32, u32) -> Scene), ("desert_noon", desert_noon), ("snowy_village", snowy_village), ("forest_dawn", forest_dawn), ("candlelit_room", candlelit_room)] {
            let f = render_on(&make(true, W, H), W, H, gpu);
            let share = white_share(&f);
            assert!(share < 0.04, "{name}: {label} is white over {:.1}% of the frame", share * 100.0);
        }
    }
}

#[test]
fn the_metering_lowers_a_hot_picture_and_leaves_a_good_one() {
    let hot = vec![[3.0f32, 3.0, 3.0, 1.0]; 400];
    let off = studio_render::atmosphere::meter(&hot, 1.0);
    assert!(off < -1.0 && off >= -3.0, "{off}");
    let fine: Vec<[f32; 4]> = (0..400).map(|i| { let v = 0.05 + 0.8 * (i as f32 / 400.0); [v, v, v, 1.0] }).collect();
    assert_eq!(studio_render::atmosphere::meter(&fine, 1.0), 0.0);
    // A few hot pixels (a lamp, a glint) do not dim the rest.
    let mut lamp = fine.clone();
    for p in lamp.iter_mut().take(6) {
        *p = [40.0, 40.0, 40.0, 1.0];
    }
    assert_eq!(studio_render::atmosphere::meter(&lamp, 1.0), 0.0);
    // Transparent pixels (nothing there) do not count.
    let mut bg = hot.clone();
    bg.extend(vec![[0.0, 0.0, 0.0, 0.0]; 4000]);
    assert!(studio_render::atmosphere::meter(&bg, 1.0) < -1.0);
}

#[test]
fn light_shown_through_air_cannot_whiten_the_picture() {
    // Lamps lined up down a fogged street: what the air adds is bounded.
    for (name, gpu) in backends() {
        let mut s = dark_stage();
        let m = s.m();
        for z in [4.0f32, 7.0, 10.0, 13.0, 16.0] {
            s.light(with_look(base_light(LightKind::Point, [0.5 * m, -3.0 * m, z * m], [0.0, 1.0, 0.0], [1.0, 0.8, 0.5].map(|c| c * 3.0)), lamp_look(0.0, 0.0, 2.5, 0.1)));
        }
        let mut a = air_only(Atmos { metre: m, ..Default::default() });
        a.fog = Some(Fog { density: 0.12, falloff: 0.0, base_y: 0.0, colour: [1.0, 0.9, 0.8], anisotropy: 0.6, range: 60.0, patchiness: 0.0 });
        with_air(&mut s, a);
        let f = render_on(&s, W, H, gpu);
        let top = f.pixels.iter().map(|p| p[0].max(p[1]).max(p[2])).fold(0.0f32, f32::max);
        assert!(top < 4.0, "{name}: the air alone makes light {top:.1} times white");
    }
}
