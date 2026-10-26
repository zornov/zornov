// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::image::{Image, linear_to_srgb, srgb_to_linear};

/// A reader over in-memory frame generators.
struct Frames {
    files: HashMap<String, Box<dyn Fn(i64) -> Image + Sync>>,
    fps: f64,
    reads: Mutex<usize>,
}

impl Reader for Frames {
    fn read(&self, path: &str, seconds: f64) -> Option<(Arc<Image>, u64)> {
        let f = self.files.get(path)?;
        let frame = (seconds * self.fps).round() as i64;
        *self.reads.lock().unwrap() += 1;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (path, frame).hash(&mut h);
        Some((Arc::new(f(frame)), h.finish()))
    }
}

fn texture(w: usize, h: usize, dx: f32, dy: f32) -> Image {
    Image::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 - dx, y as f32 - dy);
        let k = 0.25 + 0.15 * (u * 0.31).sin() * (v * 0.23).cos() + 0.25 * crate::math::value_noise(u / 5.0, v / 5.0, 9);
        [k, k * 0.9, k * 0.7, 1.0]
    })
}

fn host<'a>(img: &'a Image, reader: &'a dyn Reader, time: f64) -> Host<'a> {
    Host { input: img, input_key: hash_image(img), reader, time, frame_rate: 24.0, seed: 1 }
}

fn run(g: &Graph, img: &Image) -> Plate {
    (*evaluate(g, &host(img, &NoFiles, 0.0)).unwrap()).clone()
}

#[test]
fn every_node_kind_is_well_formed_and_runs() {
    let img = texture(48, 32, 0.0, 0.0);
    for def in nodes::all() {
        crate::params::validate_defs(def.params).unwrap_or_else(|e| panic!("{}: {e}", def.id));
        if matches!(def.id, "read" | "effect" | "sky_replace") {
            continue;
        }
        let mut g = Graph::passthrough();
        let inputs: Vec<Option<NodeId>> = def.inputs.iter().map(|_| Some(1)).collect();
        g.add(def.id, &inputs);
        let out = evaluate(&g, &host(&img, &NoFiles, 0.5)).unwrap_or_else(|e| panic!("{}: {e}", def.id));
        assert_eq!((out.width(), out.height()), (48, 32), "{}", def.id);
        assert!(out.rgba.is_finite(), "{}", def.id);
    }
}

#[test]
fn graphs_round_trip_through_json_and_refuse_loops() {
    let mut g = Graph::passthrough();
    let b = g.add("blur", &[Some(1)]);
    let m = g.add("merge", &[Some(1), Some(b)]);
    g.node_mut(m).unwrap().params.insert("operation".into(), json!("screen"));
    g.node_mut(b).unwrap().keys.insert("size".into(), vec![Key { t: 0.0, v: json!(1.0), hold: false }, Key { t: 1.0, v: json!(9.0), hold: false }]);
    let back = Graph::from_json(&g.to_json()).unwrap();
    assert_eq!(back, g);
    assert!(g.is_animated());
    assert!((g.node(b).unwrap().params_at(0.5).f("size") - 5.0).abs() < 1e-5);
    let mut bad = g.clone();
    bad.node_mut(b).unwrap().inputs = vec![Some(m)];
    assert!(bad.check().unwrap_err().contains("loop"));
    let mut bad = g.clone();
    bad.nodes.push(Node::new(9, "frobnicate"));
    assert!(bad.check().unwrap_err().contains("frobnicate"));
}

#[test]
fn merge_over_is_porter_duff() {
    let a = [0.25, 0.0, 0.0, 0.5];
    let b = [0.0, 0.0, 1.0, 1.0];
    let o = nodes::merge_px(0, a, b);
    assert_eq!(o, [0.25, 0.0, 0.5, 1.0]);
    let m = nodes::merge_px(nodes::MERGE_OPS.iter().position(|s| *s == "Mask").unwrap(), a, b);
    assert_eq!(m, [0.0, 0.0, 0.5, 0.5]);
}

#[test]
fn unchanged_nodes_are_not_recomputed() {
    clear_cache();
    let img = texture(64, 40, 0.0, 0.0);
    let mut g = Graph::passthrough();
    let bl = g.add("blur", &[Some(1)]);
    let gr = g.add("grade", &[Some(bl)]);
    let k1 = node_keys(&g, &host(&img, &NoFiles, 0.0)).unwrap();
    run(&g, &img);
    let (h0, _) = cache_stats();
    run(&g, &img);
    let (h1, _) = cache_stats();
    assert!(h1 > h0, "the second run reads the cache");
    g.node_mut(gr).unwrap().params.insert("gain".into(), json!(2.0));
    let k2 = node_keys(&g, &host(&img, &NoFiles, 0.0)).unwrap();
    let same: Vec<bool> = k1.iter().zip(&k2).map(|(a, b)| a.1 == b.1).collect();
    // Input and blur keep their keys; only the grade changes.
    assert_eq!(same, vec![true, true, false]);
    // Time alone does not change a still graph's keys.
    let k3 = node_keys(&g, &host(&img, &NoFiles, 3.0)).unwrap();
    assert_eq!(k2, k3);
}

fn green_screen(w: usize, h: usize) -> (Image, Vec<f32>) {
    // A warm subject disc with a soft edge over an uneven green screen.
    let mut truth = vec![0.0f32; w * h];
    let img = Image::from_fn(w, h, |x, y| {
        let d = ((x as f32 - w as f32 * 0.5).powi(2) + (y as f32 - h as f32 * 0.5).powi(2)).sqrt();
        let a = (1.0 - (d - 14.0) / 4.0).clamp(0.0, 1.0);
        let shade = 0.8 + 0.2 * (x as f32 / w as f32);
        let screen = [0.04 * shade, 0.5 * shade, 0.08 * shade];
        // Some green spill on the subject.
        let subj = [0.6, 0.38, 0.3];
        let c: [f32; 3] = std::array::from_fn(|i| subj[i] * a + screen[i] * (1.0 - a));
        [c[0], c[1], c[2], 1.0]
    });
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - w as f32 * 0.5).powi(2) + (y as f32 - h as f32 * 0.5).powi(2)).sqrt();
            truth[y * w + x] = (1.0 - (d - 14.0) / 4.0).clamp(0.0, 1.0);
        }
    }
    (img, truth)
}

#[test]
fn the_keyer_pulls_core_and_edges_and_removes_spill() {
    let (w, h) = (64, 64);
    let (img, truth) = green_screen(w, h);
    let mut g = Graph::passthrough();
    let k = g.add("keyer", &[Some(1)]);
    g.node_mut(k).unwrap().params.insert("core_shrink".into(), json!(1.0));
    g.node_mut(k).unwrap().params.insert("edge_band".into(), json!(8.0));
    let out = run(&g, &img);
    let err: f32 = (0..w * h).map(|i| (out.rgba.pixels[i][3] - truth[i]).abs()).sum::<f32>() / (w * h) as f32;
    assert!(err < 0.03, "mean matte error {err}");
    // The centre is solid and not green.
    let c = out.rgba.get(32, 32);
    assert!(c[3] > 0.99);
    assert!(c[1] <= c[0], "spill left: {c:?}");
    // A corner of screen is gone.
    assert!(out.rgba.get(2, 2)[3] < 0.02);
    // The core output is narrower than the matte.
    g.node_mut(k).unwrap().params.insert("output".into(), json!("Core"));
    let core = run(&g, &img);
    let n_core = core.rgba.pixels.iter().filter(|p| p[0] > 0.5).count();
    let n_all = out.rgba.pixels.iter().filter(|p| p[3] > 0.5).count();
    assert!(n_core < n_all, "{n_core} {n_all}");
}

#[test]
fn light_wrap_brightens_edges_only() {
    let (w, h) = (48, 48);
    let fg = Image::from_fn(w, h, |x, _| if x < 24 { [0.1, 0.1, 0.1, 1.0] } else { [0.0; 4] });
    let reader = Frames { files: HashMap::from([("bg".to_string(), Box::new(move |_| Image::filled(48, 48, [1.0, 0.8, 0.6, 1.0])) as Box<dyn Fn(i64) -> Image + Sync>)]), fps: 24.0, reads: Mutex::new(0) };
    let mut g = Graph::passthrough();
    let r = g.add("read", &[]);
    g.node_mut(r).unwrap().data = json!({"path": "bg"});
    let lw = g.add("light_wrap", &[Some(1), Some(r)]);
    g.node_mut(lw).unwrap().params.insert("output".into(), json!("Foreground with wrap"));
    let out = evaluate(&g, &host(&fg, &reader, 0.0)).unwrap();
    let edge = out.rgba.get(22, 24)[0];
    let inner = out.rgba.get(4, 24)[0];
    assert!(edge > 0.15 && (inner - 0.1).abs() < 0.02, "edge {edge} inner {inner}");
}

#[test]
fn roto_and_paint_change_over_time() {
    let img = Image::filled(40, 40, [0.5, 0.5, 0.5, 1.0]);
    let mut g = Graph::passthrough();
    let r = g.add("roto", &[Some(1)]);
    g.node_mut(r).unwrap().data = json!({"shapes": [{"keys": [
        {"t": 0.0, "points": [[0,0],[10,0],[10,10],[0,10]]},
        {"t": 1.0, "points": [[30,30],[40,30],[40,40],[30,40]]}
    ]}]});
    g.node_mut(r).unwrap().params.insert("output".into(), json!(0));
    assert!(g.is_animated());
    let a = evaluate(&g, &host(&img, &NoFiles, 0.0)).unwrap();
    let b = evaluate(&g, &host(&img, &NoFiles, 1.0)).unwrap();
    assert!(a.rgba.get(5, 5)[3] > 0.99 && a.rgba.get(35, 35)[3] < 0.01);
    assert!(b.rgba.get(5, 5)[3] < 0.01 && b.rgba.get(35, 35)[3] > 0.99);
    let mut g = Graph::passthrough();
    let p = g.add("paint", &[Some(1)]);
    g.node_mut(p).unwrap().data = json!({"strokes": [{"tool": "colour", "colour": "#ff0000", "points": [[5,20],[35,20]], "radius": 3, "softness": 0.2, "from": 0.5, "to": 1.0}]});
    let before = evaluate(&g, &host(&img, &NoFiles, 0.2)).unwrap();
    let during = evaluate(&g, &host(&img, &NoFiles, 0.7)).unwrap();
    assert_eq!(before.rgba.get(20, 20), [0.5, 0.5, 0.5, 1.0]);
    let c = during.rgba.get(20, 20);
    assert!(c[0] > 0.9 && c[1] < 0.05, "{c:?}");
}

#[test]
fn transform_motion_blur_smears_along_the_move() {
    let img = Image::from_fn(128, 16, |x, _| if (30..34).contains(&x) { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
    let mut g = Graph::passthrough();
    let t = g.add("transform", &[Some(1)]);
    let n = g.node_mut(t).unwrap();
    n.keys.insert("translate_x".into(), vec![Key { t: 0.0, v: json!(0.0), hold: false }, Key { t: 0.1, v: json!(40.0), hold: false }]);
    n.params.insert("motion_blur".into(), json!(1.0));
    let out = evaluate(&g, &host(&img, &NoFiles, 0.05)).unwrap();
    let lit = (0..128).filter(|&x| out.rgba.get(x, 8)[0] > 0.02).count();
    assert!(lit > 8, "only {lit} pixels lit: no blur");
}

#[test]
fn colour_match_moves_the_means() {
    let a = texture(40, 30, 0.0, 0.0);
    let reader = Frames { files: HashMap::from([("ref".to_string(), Box::new(|_| texture(40, 30, 3.0, 1.0).map(|p| [p[0] * 0.3, p[1] * 0.6, p[2] * 1.4, 1.0])) as Box<dyn Fn(i64) -> Image + Sync>)]), fps: 24.0, reads: Mutex::new(0) };
    let mut g = Graph::passthrough();
    let r = g.add("read", &[]);
    g.node_mut(r).unwrap().data = json!({"path": "ref"});
    g.add("colour_match", &[Some(1), Some(r)]);
    let out = evaluate(&g, &host(&a, &reader, 0.0)).unwrap();
    let (mo, _) = nodes::log_stats(&out.rgba);
    let (mr, _) = nodes::log_stats(&reader.read("ref", 0.0).unwrap().0);
    for c in 0..3 {
        assert!((mo[c] - mr[c]).abs() < 0.05, "{mo:?} {mr:?}");
    }
}

#[test]
fn depth_of_field_keeps_focus_sharp_and_blurs_far() {
    let (w, h) = (64, 32);
    let img = Image::from_fn(w, h, |x, y| if (x / 2 + y / 2) % 2 == 0 { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
    // Left half near (focus), right half far: a depth pass, white near.
    let reader = Frames {
        files: HashMap::from([("depth".to_string(), Box::new(move |_| Image::from_fn(64, 32, |x, _| if x < 32 { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] })) as Box<dyn Fn(i64) -> Image + Sync>)]),
        fps: 24.0,
        reads: Mutex::new(0),
    };
    let mut g = Graph::passthrough();
    let r = g.add("read", &[]);
    g.node_mut(r).unwrap().data = json!({"path": "depth"});
    let d = g.add("set_depth", &[Some(1), Some(r)]);
    let n = g.node_mut(d).unwrap();
    n.params.insert("near".into(), json!(10.0));
    n.params.insert("far".into(), json!(100.0));
    let z = g.add("zdefocus", &[Some(d)]);
    g.node_mut(z).unwrap().params.insert("focus".into(), json!(10.0));
    g.node_mut(z).unwrap().params.insert("size".into(), json!(6.0));
    let out = evaluate(&g, &host(&img, &reader, 0.0)).unwrap();
    let contrast = |x0: usize, x1: usize| {
        let v: Vec<f32> = (x0..x1).map(|x| out.rgba.get(x, 16)[0]).collect();
        v.iter().cloned().fold(0.0f32, f32::max) - v.iter().cloned().fold(1.0f32, f32::min)
    };
    assert!(contrast(8, 24) > 0.95, "focus is sharp");
    assert!(contrast(44, 60) < 0.5, "far is soft: {}", contrast(44, 60));
}

#[test]
fn grain_match_adds_the_missing_grain() {
    let (w, h) = (96, 96);
    let noisy = Image::from_fn(w, h, |x, y| {
        let n = crate::math::hash2(x as i32, y as i32, 5) - 0.5;
        let v = srgb_to_linear((0.45 + 0.08 * n).clamp(0.0, 1.0));
        [v, v, v, 1.0]
    });
    let clean = Image::filled(w, h, [srgb_to_linear(0.45), srgb_to_linear(0.45), srgb_to_linear(0.45), 1.0]);
    let want = lens::measure_grain(&noisy);
    let have = lens::measure_grain(&clean);
    assert!(have.spread[1][1] < 0.002);
    let reader = Frames { files: HashMap::from([("plate".to_string(), Box::new(move |_| noisy.clone()) as Box<dyn Fn(i64) -> Image + Sync>)]), fps: 24.0, reads: Mutex::new(0) };
    let mut g = Graph::passthrough();
    let r = g.add("read", &[]);
    g.node_mut(r).unwrap().data = json!({"path": "plate"});
    g.add("grain_match", &[Some(1), Some(r)]);
    let out = evaluate(&g, &host(&clean, &reader, 0.0)).unwrap();
    let got = lens::measure_grain(&out.rgba);
    let (a, b) = (got.spread[1][1], want.spread[1][1]);
    assert!((a / b - 1.0).abs() < 0.35, "added {a}, wanted {b}");
    let _ = linear_to_srgb;
}

#[test]
fn lens_distortion_round_trips() {
    let img = texture(96, 64, 0.0, 0.0);
    let mut g = Graph::passthrough();
    let u = g.add("lens_distort", &[Some(1)]);
    g.node_mut(u).unwrap().params.insert("k1".into(), json!(-0.12));
    let d = g.add("lens_distort", &[Some(u)]);
    g.node_mut(d).unwrap().params.insert("k1".into(), json!(-0.12));
    g.node_mut(d).unwrap().params.insert("mode".into(), json!("Distort"));
    let out = run(&g, &img);
    let mut err = 0.0;
    let mut n = 0.0;
    for y in 16..48 {
        for x in 24..72 {
            err += (out.rgba.get(x, y)[0] - img.get(x, y)[0]).abs();
            n += 1.0;
        }
    }
    assert!(err / n < 0.02, "{}", err / n);
}

#[test]
fn inpaint_fills_a_hole_in_a_texture() {
    let (w, h) = (80, 60);
    let img = Image::from_fn(w, h, |x, y| {
        let k = if (x / 4 + y / 4) % 2 == 0 { 0.8 } else { 0.2 };
        [k, k, k, 1.0]
    });
    let mut holed = img.clone();
    let mut hole = vec![false; w * h];
    for y in 24..36 {
        for x in 30..44 {
            holed.set(x, y, [1.0, 0.0, 1.0, 1.0]);
            hole[y * w + x] = true;
        }
    }
    let filled = clean::inpaint_image(&holed, &hole, 7, 6);
    let err: f32 = (0..w * h).filter(|&i| hole[i]).map(|i| (filled.pixels[i][0] - img.pixels[i][0]).abs() + filled.pixels[i][1].min(1.0) * 0.0 + (filled.pixels[i][1] - img.pixels[i][1]).abs()).sum::<f32>() / hole.iter().filter(|&&b| b).count() as f32;
    assert!(err < 0.2, "mean error in the hole {err}");
    // No magenta left.
    assert!((0..w * h).filter(|&i| hole[i]).all(|i| (filled.pixels[i][0] - filled.pixels[i][1]).abs() < 0.3));
}

#[test]
fn temporal_fill_reveals_the_background_behind_a_moving_object() {
    let (w, h) = (96, 64);
    // The camera pans 2 px a frame; a grey box moves the other way.
    let frame = move |f: i64| {
        let bg = texture(96, 64, -2.0 * f as f32, 0.0);
        let bx = 40.0 + 2.0 * f as f32;
        bg.map_xy(|x, y, p| if (x as f32) >= bx && (x as f32) < bx + 14.0 && (20..40).contains(&y) { [0.5, 0.5, 0.5, 1.0] } else { p })
    };
    let matte = move |f: i64| {
        let bx = 40.0 + 2.0 * f as f32;
        Image::from_fn(96, 64, |x, y| if (x as f32) >= bx && (x as f32) < bx + 14.0 && (20..40).contains(&y) { [1.0; 4] } else { [0.0; 4] })
    };
    let reader = Frames {
        files: HashMap::from([
            ("plate".to_string(), Box::new(frame) as Box<dyn Fn(i64) -> Image + Sync>),
            ("matte".to_string(), Box::new(matte) as Box<dyn Fn(i64) -> Image + Sync>),
        ]),
        fps: 24.0,
        reads: Mutex::new(0),
    };
    let t = 10.0 / 24.0;
    let cur = frame(10);
    let mut g = Graph::passthrough();
    g.input_media = Some(InputMedia { path: "plate".into(), start: 0.0, speed: 1.0 });
    let m = g.add("read", &[]);
    g.node_mut(m).unwrap().data = json!({"path": "matte"});
    let tf = g.add("temporal_fill", &[Some(1), Some(m)]);
    g.node_mut(tf).unwrap().params.insert("range".into(), json!(8));
    let out = evaluate(&g, &host(&cur, &reader, t)).unwrap();
    let truth = texture(w, h, -20.0, 0.0);
    let bx = 40 + 20;
    let mut err = 0.0;
    let mut n = 0.0;
    for y in 22..38 {
        for x in (bx + 2)..(bx + 12) {
            err += (out.rgba.get(x, y)[0] - truth.get(x, y)[0]).abs();
            n += 1.0;
        }
    }
    assert!(err / n < 0.03, "mean error {}", err / n);
}

#[test]
fn clean_plate_drops_a_passer_by_on_a_locked_shot() {
    let frame = move |f: i64| {
        let bg = texture(64, 48, 0.0, 0.0);
        let bx = 4.0 + 5.0 * f as f32;
        bg.map_xy(|x, y, p| if (x as f32) >= bx && (x as f32) < bx + 6.0 && (10..30).contains(&y) { [1.0, 0.0, 0.0, 1.0] } else { p })
    };
    let reader = Frames { files: HashMap::from([("plate".to_string(), Box::new(frame) as Box<dyn Fn(i64) -> Image + Sync>)]), fps: 24.0, reads: Mutex::new(0) };
    let cur = frame(5);
    let mut g = Graph::passthrough();
    g.input_media = Some(InputMedia { path: "plate".into(), start: 0.0, speed: 1.0 });
    let c = g.add("clean_plate", &[Some(1)]);
    g.node_mut(c).unwrap().params.insert("range".into(), json!(5));
    g.node_mut(c).unwrap().params.insert("step".into(), json!(1));
    let out = evaluate(&g, &host(&cur, &reader, 5.0 / 24.0)).unwrap();
    let truth = texture(64, 48, 0.0, 0.0);
    assert!(out.rgba.max_abs_diff(&truth) < 1e-4);
}

#[test]
fn wire_removal_hides_a_thin_line() {
    let (w, h) = (64, 64);
    let base = Image::from_fn(w, h, |x, y| {
        let k = 0.3 + 0.004 * x as f32 + 0.002 * y as f32;
        [k, k, k, 1.0]
    });
    let wired = base.map_xy(|x, y, p| if (x as i32 - y as i32).abs() <= 1 { [0.02, 0.02, 0.02, 1.0] } else { p });
    let mut g = Graph::passthrough();
    let wr = g.add("wire_remove", &[Some(1)]);
    let n = g.node_mut(wr).unwrap();
    n.params.insert("from".into(), json!([0.0, 0.0]));
    n.params.insert("to".into(), json!([1.0, 1.0]));
    n.params.insert("width".into(), json!(4.0));
    n.params.insert("texture".into(), json!(0.0));
    let out = run(&g, &wired);
    let err = (10..54).map(|i| (out.rgba.get(i, i)[0] - base.get(i, i)[0]).abs()).fold(0.0f32, f32::max);
    assert!(err < 0.03, "{err}");
}

#[test]
fn sky_is_found_and_replaced() {
    let (w, h) = (64, 48);
    let img = Image::from_fn(w, h, |x, y| {
        if y < 20 {
            [srgb_to_linear(0.45), srgb_to_linear(0.65), srgb_to_linear(0.95), 1.0]
        } else {
            let k = 0.1 + 0.2 * crate::math::hash2(x as i32, y as i32, 3);
            [k * 0.6, k, k * 0.4, 1.0]
        }
    });
    let m = clean::detect_sky(&img, 0.5);
    let top: f32 = (0..w * 10).map(|i| m[i]).sum::<f32>() / (w * 10) as f32;
    let bottom: f32 = (w * 24..w * h).map(|i| m[i]).sum::<f32>() / (w * 24) as f32;
    assert!(top > 0.95 && bottom < 0.05, "top {top} bottom {bottom}");
    let reader = Frames { files: HashMap::from([("sky".to_string(), Box::new(|_| Image::filled(64, 48, [1.0, 0.4, 0.1, 1.0])) as Box<dyn Fn(i64) -> Image + Sync>)]), fps: 24.0, reads: Mutex::new(0) };
    let mut g = Graph::passthrough();
    let r = g.add("read", &[]);
    g.node_mut(r).unwrap().data = json!({"path": "sky"});
    g.add("sky_replace", &[Some(1), Some(r)]);
    let out = evaluate(&g, &host(&img, &reader, 0.0)).unwrap();
    let s = out.rgba.get(30, 5);
    assert!(s[0] > 0.9 && s[2] < 0.2, "{s:?}");
}

#[test]
fn motion_vectors_feed_vector_blur() {
    let frame = |f: i64| texture(80, 48, 4.0 * f as f32, 0.0);
    let reader = Frames { files: HashMap::from([("plate".to_string(), Box::new(frame) as Box<dyn Fn(i64) -> Image + Sync>)]), fps: 24.0, reads: Mutex::new(0) };
    let cur = frame(3);
    let mut g = Graph::passthrough();
    g.input_media = Some(InputMedia { path: "plate".into(), start: 0.0, speed: 1.0 });
    let mv = g.add("motion_vectors", &[Some(1)]);
    let out = evaluate_node(&g, &host(&cur, &reader, 3.0 / 24.0), mv).unwrap();
    let m = out.motion.as_ref().unwrap();
    let mean_x: f32 = (0..80 * 48).filter(|i| (i % 80) > 16 && (i % 80) < 64).map(|i| m[i][0]).sum::<f32>() / (48.0 * 47.0);
    assert!((mean_x - 4.0).abs() < 0.6, "{mean_x}");
    let vb = g.add("vector_blur", &[Some(mv)]);
    g.node_mut(vb).unwrap().params.insert("shutter".into(), json!(2.0));
    let blurred = evaluate(&g, &host(&cur, &reader, 3.0 / 24.0)).unwrap();
    // Horizontal detail is smoothed, so neighbours along x are closer.
    let grad = |img: &Image| (16..64).map(|x| (img.get(x + 1, 24)[0] - img.get(x, 24)[0]).abs()).sum::<f32>();
    assert!(grad(&blurred.rgba) < grad(&cur) * 0.8);
}

#[test]
fn the_keyer_takes_a_plain_white_or_black_backdrop() {
    // A mid-tone disc with a soft edge on white, then on black.
    for (bg, hex) in [(1.0f32, "#ffffff"), (0.0, "#000000")] {
        let (w, h) = (64, 64);
        let img = Image::from_fn(w, h, |x, y| {
            let d = ((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt();
            let a = (1.0 - (d - 14.0) / 4.0).clamp(0.0, 1.0);
            let subj = [0.3, 0.2, 0.5];
            let c: [f32; 3] = std::array::from_fn(|i| subj[i] * a + bg * (1.0 - a));
            [c[0], c[1], c[2], 1.0]
        });
        let mut g = Graph::passthrough();
        let k = g.add("keyer", &[Some(1)]);
        g.node_mut(k).unwrap().params.insert("screen".into(), json!("Custom"));
        g.node_mut(k).unwrap().params.insert("colour".into(), json!(hex));
        g.node_mut(k).unwrap().params.insert("core_shrink".into(), json!(1.0));
        g.node_mut(k).unwrap().params.insert("edge_band".into(), json!(8.0));
        g.node_mut(k).unwrap().params.insert("edge_colour".into(), json!(1.0));
        let out = run(&g, &img);
        assert!(out.rgba.get(2, 2)[3] < 0.02, "{hex}: backdrop kept {:?}", out.rgba.get(2, 2));
        let c = out.rgba.get(32, 32);
        assert!(c[3] > 0.99, "{hex}: subject lost {c:?}");
        // The soft edge carries the subject's colour, not the backdrop's.
        let e = out.rgba.get(32 + 16, 32);
        if e[3] > 0.1 {
            let fg = [e[0] / e[3], e[1] / e[3], e[2] / e[3]];
            assert!((fg[0] - 0.3).abs() < 0.15 && (fg[2] - 0.5).abs() < 0.15, "{hex}: fringe {fg:?}");
        }
    }
}
