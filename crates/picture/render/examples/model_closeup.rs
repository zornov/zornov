// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Draws a model (a built-in character, or a model file) close up to a PNG
//! with the renderer's own lights, for looking at a body, a face or a hand
//! without opening the app.
//!
//! ```text
//! cargo run -p studio-render --example model_closeup -- --build child --focus head --out head.png
//! cargo run -p studio-render --example model_closeup -- --file robot.glb --focus full --out robot.png
//! ```
//!
//! `--wear "coat:trench,colour=#b49a6a;hat:fedora"` dresses a built character
//! (`kind[:style]` then `key=value` pairs: colour, colour2, material, pattern,
//! fit, length, sleeves); `--bare` builds it undressed first.
//!
//! `--pose "head=0,60,0;eye.L=20,0,0"` turns bones (degrees x,y,z) and
//! `--shapes "jawOpen=1;eyeBlinkLeft=1"` sets shape keys.
//!
//! `--build` takes a preset of `studio_mesh::character::Build::preset`;
//! `--focus` is `full`, `upper`, `head`, `hand` or `feet`; `--size` the
//! picture's side in pixels; `--yaw` degrees to turn the model.

use std::sync::Arc;
use studio_mesh::Mesh;
use studio_render::mesh_render::{MaterialOverride, MeshDesc, MeshInstance, render};
use studio_render::scene::Canvas;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn main() {
    let size: u32 = arg("--size").and_then(|s| s.parse().ok()).unwrap_or(720);
    let focus = arg("--focus").unwrap_or_else(|| "full".into());
    let yaw: f32 = arg("--yaw").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let out = arg("--out").unwrap_or_else(|| "closeup.png".into());
    let mesh: Mesh = match arg("--file") {
        Some(f) => studio_mesh::import::load(std::path::Path::new(&f), 400.0).expect("model file").mesh,
        None => {
            let b = studio_mesh::character::Build::preset(&arg("--build").unwrap_or_else(|| "adult".into())).expect("a build preset");
            let bare = std::env::args().any(|a| a == "--bare") || arg("--wear").is_some();
            studio_mesh::character::character(400.0, &studio_mesh::character::Build { detail: arg("--detail").and_then(|s| s.parse().ok()).unwrap_or(3), dressed: !bare, ..b })
        }
    };
    let mesh = match arg("--wear") {
        Some(list) => dress(mesh, &list),
        None => mesh,
    };
    let mesh = match (arg("--pose"), arg("--shapes")) {
        (None, None) => mesh,
        (pose, shapes) => {
            let mut input = studio_mesh::rig::PoseInput::default();
            for kv in pose.unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
                let (bone, v) = kv.split_once('=').unwrap_or((kv, "0,0,0"));
                let d: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                input.rotations.insert(bone.into(), [d.first().copied().unwrap_or(0.0), d.get(1).copied().unwrap_or(0.0), d.get(2).copied().unwrap_or(0.0)]);
            }
            for kv in shapes.unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
                let (k, v) = kv.split_once('=').unwrap_or((kv, "1"));
                input.shapes.insert(k.into(), v.parse().unwrap_or(1.0));
            }
            match mesh.rig.clone() {
                Some(rig) => rig.posed_mesh(&mesh, &input),
                None => mesh,
            }
        }
    };
    let (lo, hi) = mesh.bounds();
    let height = hi[1] - lo[1];
    // The part to fill the picture: its centre and its extent.
    let (centre, extent) = match focus.as_str() {
        "head" => {
            let top = lo[1];
            let sel: Vec<_> = mesh.positions.iter().filter(|p| p[1] < top + height * 0.15).collect();
            bounds_of(&sel)
        }
        "upper" => {
            let sel: Vec<_> = mesh.positions.iter().filter(|p| p[1] < lo[1] + height * 0.55).collect();
            bounds_of(&sel)
        }
        "hand" => {
            let rig = mesh.rig.as_ref().expect("a rigged model");
            let hand = rig.bones[rig.bone("hand.L").expect("hand.L")].head;
            let sel: Vec<_> = mesh.positions.iter().filter(|p| studio_mesh::length(studio_mesh::sub(**p, hand)) < height * 0.1).collect();
            bounds_of(&sel)
        }
        "feet" => {
            let sel: Vec<_> = mesh.positions.iter().filter(|p| p[1] > hi[1] - height * 0.1).collect();
            bounds_of(&sel)
        }
        _ => bounds_of(&mesh.positions.iter().collect::<Vec<_>>()),
    };
    let k = size as f32 * 0.8 / extent.max(1.0);
    let (s, c) = yaw.to_radians().sin_cos();
    // Turned about y, scaled, centred on the picture.
    let m = |v: [f32; 3]| [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c];
    let cc = m(centre);
    let world = [[k * c, 0.0, k * s, -cc[0] * k], [0.0, k, 0.0, -cc[1] * k], [-k * s, 0.0, k * c, -cc[2] * k]];
    let canvas = Canvas::new(size, size, 30.0);
    let inst = MeshInstance {
        id: 1,
        mesh: Arc::new(mesh),
        key: "closeup".into(),
        world,
        overrides: MaterialOverride::default(),
        smooth: 1.0,
        accepts_lights: true,
        casts_shadows: true,
        accepts_shadows: true,
        motion: None,
    };
    let frame = render(&MeshDesc { instances: vec![inst], camera: None, lights: vec![] }, &canvas, (size, size));
    // Over a plain dark backdrop.
    let mut f = frame;
    for p in &mut f.pixels {
        let a = p[3];
        for ch in 0..3 {
            p[ch] += (1.0 - a) * [0.012, 0.014, 0.02][ch];
        }
        p[3] = 1.0;
    }
    f.save_png(std::path::Path::new(&out)).expect("write the picture");
    println!("{out}");
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    (s.len() == 6).then(|| [u8::from_str_radix(&s[0..2], 16).unwrap_or(0), u8::from_str_radix(&s[2..4], 16).unwrap_or(0), u8::from_str_radix(&s[4..6], 16).unwrap_or(0)])
}

fn dress(mut mesh: Mesh, list: &str) -> Mesh {
    use studio_mesh::wear::{Spec, attach, fit};
    for item in list.split(';').filter(|s| !s.trim().is_empty()) {
        let mut parts = item.split(',');
        let head = parts.next().unwrap_or("");
        let (kind, style) = head.split_once(':').unwrap_or((head, ""));
        let mut spec = Spec { kind: kind.trim().into(), style: style.trim().into(), ..Spec::default() };
        for kv in parts {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match k.trim() {
                "colour" | "color" => spec.colour = hex(v),
                "colour2" | "color2" => spec.colour2 = hex(v),
                "material" => spec.material = Some(v.into()),
                "pattern" => spec.pattern = studio_mesh::fabric::Pattern::parse(v).unwrap_or(studio_mesh::fabric::Pattern::None),
                "fit" => spec.fit = v.parse().unwrap_or(1.0),
                "length" => spec.length = Some(v.into()),
                "sleeves" => spec.sleeves = Some(v.into()),
                "open" => spec.open = Some(v == "true"),
                "scale" => spec.scale = v.parse().unwrap_or(1.0),
                _ => {}
            }
        }
        if studio_modeling::hair::is_hair(&spec.kind) {
            let mut h = studio_modeling::hair::HairSpec { kind: spec.kind.clone(), style: spec.style.clone(), colour: spec.colour, ..Default::default() };
            h.density = 0.6;
            let g = studio_modeling::hair::fit(&mesh, &h).unwrap_or_else(|e| panic!("{item}: {e}"));
            attach(&mut mesh, &g).unwrap_or_else(|e| panic!("{item}: {e}"));
            continue;
        }
        for g in fit(&mesh, &spec).unwrap_or_else(|e| panic!("{item}: {e}")) {
            attach(&mut mesh, &g).unwrap_or_else(|e| panic!("{item}: {e}"));
        }
    }
    mesh
}

fn bounds_of(points: &[&[f32; 3]]) -> ([f32; 3], f32) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    (c, (hi[0] - lo[0]).max(hi[1] - lo[1]))
}
