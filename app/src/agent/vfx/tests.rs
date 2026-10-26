// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::{Path, PathBuf};

use serde_json::json;
use studio_encode::{h264, mux, yuv};

use crate::agent::Session;
use crate::services;
use crate::services::tracking::test_movie;

/// Writes an H.264 movie whose pixel (x, y) of frame i is `px(i, x, y)`.
fn write_movie(path: &Path, w: u32, h: u32, fps: u32, n: u32, px: impl Fn(u32, u32, u32) -> [u8; 3] + Sync) {
    let mut cfg = h264::Config::new(w, h, fps, 1);
    cfg.max_slices = 1;
    cfg.bitrate_kbps = 12000;
    let mut enc = h264::Encoder::new(cfg).unwrap();
    let mut muxer = mux::Muxer::create(path, mux::Options::mp4()).unwrap();
    let v = muxer.add_video(enc.track_info()).unwrap();
    for i in 0..n {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let c = px(i, x, y);
                let k = ((y * w + x) * 4) as usize;
                rgba[k..k + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let f = enc.encode(&yuv::Yuv420::from_rgba(&rgba, w, h));
        muxer.write_sample(v, &f.data, f.keyframe).unwrap();
    }
    muxer.finish().unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("studio-vfx-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn session_with(path: &Path, w: u32, h: u32, fps: f32) -> (Session, u32) {
    let mut s = Session::new(services::project::new_custom("VFX", w, h, fps));
    s.run("import", &json!({"paths": [path.to_string_lossy()], "to_timeline": true})).unwrap();
    let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
    (s, id)
}

/// The rendered pixel at canvas point `p` (from the centre), RGB.
fn px_at(s: &mut Session, t: f64, w: u32, h: u32, p: [f64; 2]) -> [u8; 3] {
    let img = s.frames.rgba(&s.doc, (t * 1e6) as crate::view::Time, w).unwrap();
    let k = img.width as f64 / w as f64;
    let (x, y) = (((p[0] + w as f64 / 2.0) * k) as u32, ((p[1] + h as f64 / 2.0) * k) as u32);
    let i = ((y * img.width + x) * 4) as usize;
    [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]
}

#[test]
fn comp_builds_a_graph_and_it_renders() {
    let _l = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, id) = session_with(&test_movie::movie(), test_movie::W, test_movie::H, test_movie::FPS as f32);
    let p = [-100.0, -60.0];
    let before = px_at(&mut s, 0.5, test_movie::W, test_movie::H, p);
    let r = s.edit(&[json!({"op": "comp", "clip": id, "add": [{"name": "lift", "kind": "grade", "params": {"multiply": 2.0}}]})], "comp", false).unwrap().to_string();
    assert!(r.contains("lift"), "{r}");
    let after = px_at(&mut s, 0.5, test_movie::W, test_movie::H, p);
    assert!(after[0] as u32 > before[0] as u32 + 20, "{before:?} -> {after:?}");
    // Keyed: brighter later.
    s.edit(&[json!({"op": "comp", "clip": id, "set": [{"node": "lift", "keys": {"multiply": [[0.0, 1.0], [1.0, 3.0]]}}]})], "key", false).unwrap();
    let early = px_at(&mut s, 0.05, test_movie::W, test_movie::H, p);
    let late = px_at(&mut s, 1.5, test_movie::W, test_movie::H, p);
    assert!(late[0] > early[0], "{early:?} {late:?}");
    // Mistakes are refused with what is possible.
    let e = s.edit(&[json!({"op": "comp", "clip": id, "add": [{"kind": "frobnicate"}]})], "bad", false).unwrap_err();
    assert!(e.contains("keyer"), "{e}");
    let e = s.edit(&[json!({"op": "comp", "clip": id, "set": [{"node": "lift", "params": {"nope": 1}}]})], "bad", false).unwrap_err();
    assert!(e.contains("gain"), "{e}");
    let pre = px_at(&mut s, 0.5, test_movie::W, test_movie::H, p);
    // A shot comp over everything.
    let r = s.edit(&[json!({"op": "comp", "shot": true, "at": 0, "duration": 1, "add": [{"kind": "invert", "params": {"channels": "Colour"}}]})], "shot", false).unwrap().to_string();
    assert!(r.contains("comp layer"), "{r}");
    let inv = px_at(&mut s, 0.5, test_movie::W, test_movie::H, p);
    // Inverted in linear light: 1 - linear(pre), back in sRGB.
    let lin = |v: u8| { let c = v as f64 / 255.0; if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } };
    let enc = |l: f64| (if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 } * 255.0) as i32;
    assert!((inv[0] as i32 - enc(1.0 - lin(pre[0]))).abs() < 12, "{pre:?} -> {inv:?}");
    s.edit(&[json!({"op": "comp", "clip": id, "delete": true})], "rm", false).unwrap();
    assert!(services::vfx::graph_of(&s.model_clip(id).unwrap()).is_none());
}

#[test]
fn keying_pulls_a_green_screen_over_a_background() {
    let _l = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (w, h) = (320, 180);
    let movie = scratch("green.mp4");
    write_movie(&movie, w, h, 24, 12, |i, x, y| {
        let (cx, cy) = (120.0 + 4.0 * i as f64, 90.0);
        let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
        let a = (1.0 - (d - 40.0) / 3.0).clamp(0.0, 1.0);
        let screen = [20.0, 170.0 + 20.0 * (x as f64 / w as f64), 40.0];
        let subj = [200.0, 120.0, 90.0];
        std::array::from_fn(|k| (subj[k] * a + screen[k] * (1.0 - a)) as u8)
    });
    let bg = scratch("bg.mp4");
    write_movie(&bg, w, h, 24, 12, |_, _, _| [40, 60, 200]);
    let (mut s, id) = session_with(&movie, w, h, 24.0);
    let r = s.edit(&[json!({"op": "keying", "clip": id, "screen": "green", "background": bg.to_string_lossy()})], "key", false).unwrap().to_string();
    assert!(r.contains("light wrap"), "{r}");
    let t = 4.0 / 24.0;
    let subject = px_at(&mut s, t, w, h, [136.0 - 160.0, 0.0]);
    let corner = px_at(&mut s, t, w, h, [-150.0, -80.0]);
    assert!(subject[0] > 150 && subject[1] < 150, "subject {subject:?}");
    assert!(corner[2] > 150 && corner[1] < 100, "background shows: {corner:?}");
}

#[test]
fn remove_object_fills_a_box_from_nearby_frames() {
    let _l = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, id) = session_with(&test_movie::movie(), test_movie::W, test_movie::H, test_movie::FPS as f32);
    let i = 20u32;
    let t = i as f64 / test_movie::FPS as f64;
    let (ox, oy) = test_movie::object(i);
    let (sx, sy) = test_movie::shake(i);
    let c = [ox + sx - test_movie::W as f64 / 2.0, oy + sy - test_movie::H as f64 / 2.0];
    let blue_gap = |p: [u8; 3]| (p[2] as i32 - p[0] as i32).abs();
    let before = px_at(&mut s, t, test_movie::W, test_movie::H, [c[0] + 8.0, c[1] + 5.0]);
    assert!(blue_gap(before) > 30, "the square is there first: {before:?}");
    // A box that holds the square for a few frames around i.
    let b = [[c[0] - 40.0, c[1] - 36.0], [c[0] + 40.0, c[1] + 36.0]];
    let r = s.edit(&[json!({"op": "remove_object", "clip": id, "box": b, "range": 24, "step": 3})], "rm", false).unwrap().to_string();
    assert!(r.contains("temporal_fill"), "{r}");
    let after = px_at(&mut s, t, test_movie::W, test_movie::H, [c[0] + 8.0, c[1] + 5.0]);
    assert!(blue_gap(after) < 16, "the wall shows through: {after:?}");
}

/// A dolly past a textured floor, back wall and a pillar, rendered by
/// casting rays; the camera slides right and a little forward.
fn dolly_movie(path: &Path, w: u32, h: u32, n: u32) {
    let f = 0.9 * w as f64;
    let tex = |u: f64, v: f64| -> f64 {
        let c = if ((u * 2.0).floor() as i64 + (v * 2.0).floor() as i64) % 2 == 0 { 0.65 } else { 0.35 };
        c + 0.25 * ((u * 7.3).sin() * (v * 5.1).cos()) + 0.1 * ((u * 23.0 + v * 17.0).sin())
    };
    write_movie(path, w, h, 24, n, |i, x, y| {
        let s = i as f64 / n as f64;
        let cam = [-1.5 + 3.0 * s, 0.0, 2.0 * s];
        let yaw: f64 = -0.12 * s;
        let d0 = [(x as f64 + 0.5 - w as f64 / 2.0) / f, (y as f64 + 0.5 - h as f64 / 2.0) / f, 1.0];
        // Camera turns about y: world = Rᵀ · d.
        let (sn, cs) = yaw.sin_cos();
        let d = [cs * d0[0] + sn * d0[2], d0[1], -sn * d0[0] + cs * d0[2]];
        let mut best = (f64::INFINITY, 0.0);
        // Floor y = 1.2 (below), wall z = 16, pillar plane x = 1 for z 7..9.
        if d[1] > 1e-6 {
            let t = (1.2 - cam[1]) / d[1];
            let p = [cam[0] + d[0] * t, cam[2] + d[2] * t];
            best = (t, tex(p[0], p[1]));
        }
        if d[2] > 1e-6 {
            let t = (16.0 - cam[2]) / d[2];
            if t < best.0 {
                best = (t, tex(cam[0] + d[0] * t + 50.0, cam[1] + d[1] * t));
            }
        }
        if d[0].abs() > 1e-6 {
            let t = (1.0 - cam[0]) / d[0];
            let z = cam[2] + d[2] * t;
            let yy = cam[1] + d[1] * t;
            if t > 0.0 && t < best.0 && (7.0..9.0).contains(&z) && yy > -2.0 {
                best = (t, tex(z * 1.5 + 9.0, yy * 1.5) * 0.8 + 0.2);
            }
        }
        let g = (best.1.clamp(0.0, 1.0) * 255.0) as u8;
        [g, g, (g as f64 * 0.9) as u8]
    });
}

#[test]
fn matchmove_solves_a_dolly_and_places_a_layer_on_the_ground() {
    let _l = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (w, h) = (480, 270);
    let movie = scratch("dolly.mp4");
    dolly_movie(&movie, w, h, 40);
    let (mut s, id) = session_with(&movie, w, h, 24.0);
    let r = s.edit(&[json!({"op": "matchmove", "clip": id})], "solve", false).unwrap().to_string();
    assert!(r.contains("camera solved"), "{r}");
    assert!(!r.contains("nodal"), "a dolly has parallax: {r}");
    let cam = s.doc.model.sequences[0].clips.iter().find(|c| c.meta.contains_key("matchmove")).unwrap().clone();
    let err = cam.meta["matchmove"]["error"].as_f64().unwrap();
    assert!(err < 1.0, "{r}");
    assert!(cam.keyframes.contains_key("transform.position") && cam.keyframes.contains_key("transform.rotation_y"));
    // The camera travels right (x grows) as the dolly did.
    let xs: Vec<f64> = cam.keyframes["transform.position"].keys.iter().filter_map(|k| if let studio_model::Value::Point(p) = k.value { Some(p.x) } else { None }).collect();
    assert!(xs.last().unwrap() > xs.first().unwrap(), "{xs:?}");
    // A card stands on the ground at a floor point low in frame.
    s.edit(&[json!({"op": "card", "colour": "#ff0000", "at": 0, "duration": 1, "width": 60, "height": 120})], "card", false).unwrap();
    let card = s.doc.project.clips.iter().max_by_key(|c| c.id).unwrap().id;
    let r = s.edit(&[json!({"op": "place_on_ground", "clip": card, "point": [0.0, 100.0], "at": 0.0})], "place", false).unwrap().to_string();
    assert!(r.contains("standing on the ground"), "{r}");
    let mc = s.model_clip(card).unwrap();
    // In front of the camera.
    assert!(mc.transform.three_d && mc.transform.position_z > cam.transform.position_z, "{:?} vs camera {}", mc.transform, cam.transform.position_z);
    // Export and import as Nuke .chan.
    let chan = scratch("cam.chan");
    s.edit(&[json!({"op": "export_camera", "path": chan.to_string_lossy()})], "export", false).unwrap();
    let text = std::fs::read_to_string(&chan).unwrap();
    assert!(text.lines().count() >= 30, "{text}");
    let r = s.edit(&[json!({"op": "import_camera", "path": chan.to_string_lossy(), "at": 0})], "import", false).unwrap().to_string();
    assert!(r.contains("frames"), "{r}");
    let imported = s.doc.model.sequences[0].clips.iter().filter(|c| c.name == "Imported camera").count();
    assert_eq!(imported, 1);
}

#[test]
fn the_ready_made_graphs_build_and_render() {
    let _l = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, id) = session_with(&test_movie::movie(), test_movie::W, test_movie::H, test_movie::FPS as f32);
    let sky = scratch("sky.mp4");
    write_movie(&sky, 64, 36, 30, 2, |_, _, y| [250, 120 + y as u8, 40]);
    for (op, kind) in [
        (json!({"op": "wire", "clip": id, "from": [-200, -150], "to": [200, 150], "width": 3}), "wire_remove"),
        (json!({"op": "paint", "clip": id, "strokes": [{"tool": "clone", "points": [[-50, 0], [-30, 10]], "radius": 8, "offset": [40, 0]}]}), "paint"),
        (json!({"op": "lens", "clip": id, "k1": -0.05, "chroma": 0.002}), "lens_distort"),
        (json!({"op": "grain_match", "clip": id, "reference": id}), "grain_match"),
        (json!({"op": "motion_blur", "clip": id, "shutter": 0.5}), "vector_blur"),
        (json!({"op": "sky", "clip": id, "sky": sky.to_string_lossy()}), "sky_replace"),
    ] {
        let r = s.edit(&[op.clone()], "vfx", false).unwrap_or_else(|e| panic!("{op}: {e}")).to_string();
        assert!(r.contains(kind), "{op}: {r}");
    }
    let g = services::vfx::graph_of(&s.model_clip(id).unwrap()).unwrap().1;
    assert!(g.nodes.len() >= 9, "{}", g.to_json());
    let p = px_at(&mut s, 0.5, test_movie::W, test_movie::H, [0.0, 0.0]);
    let _ = p;
    // The node list for the AI.
    let cat = crate::services::vfx::catalogue();
    for k in ["keyer", "temporal_fill", "zdefocus", "roto", "grain_match", "sky_replace"] {
        assert!(cat.contains(k), "{k}");
    }
}
