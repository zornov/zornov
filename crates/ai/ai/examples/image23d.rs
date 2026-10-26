// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pictures to models, with turntable renders for looking at them:
//! `cargo run --release -p studio-ai --example image23d -- out_dir picture.png [more.png ...]`
//! A picture without a see-through background is cut out with the object
//! cut-out model. Per picture it writes `<out_dir>/<name>_cut.png` (the
//! object on grey), `_turn.png` (four views) and `_texture.png`, and
//! prints the timings and the quality report.

use studio_ai::features::image23d::{ImageTo3d, Params};
use studio_ai::{JobCtx, ModelStore};
use studio_quality::raster::{frame, render, sheet, write_png, View};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(&a[0]);
    std::fs::create_dir_all(&out).unwrap();
    let store = ModelStore::default();
    let t = std::time::Instant::now();
    let net = ImageTo3d::load(&store).expect("model");
    eprintln!("model loaded in {:.1}s", t.elapsed().as_secs_f32());
    let res = std::env::var("RES").ok().and_then(|s| s.parse().ok()).unwrap_or(256);
    let thr = std::env::var("THRESH").ok().and_then(|s| s.parse().ok()).unwrap_or(Params::default().threshold);
    for file in &a[1..] {
        let name = std::path::Path::new(file).file_stem().unwrap().to_string_lossy().to_string();
        let img = match studio_media::png::read(std::path::Path::new(file)) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("{name}: {e}");
                continue;
            }
        };
        let (w, h) = (img.width as usize, img.height as usize);
        let mut pixels = img.pixels.clone();
        let t0 = std::time::Instant::now();
        match studio_ai::features::image23d::isolate(&store, w, h, &pixels) {
            Some((p, how)) => {
                pixels = p;
                eprintln!("{name}: object found by: {how}");
            }
            None => {
                eprintln!("{name}: NO SUBJECT FOUND");
                continue;
            }
        }
        // The object on grey, to see what the model was given.
        let mut o = img.clone();
        for (d, s) in o.pixels.chunks_exact_mut(4).zip(pixels.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0;
            for k in 0..3 {
                d[k] = (s[k] as f32 * a + 110.0 * (1.0 - a)) as u8;
            }
            d[3] = 255;
        }
        std::fs::write(out.join(format!("{name}_cut.png")), studio_media::png::encode(&o)).unwrap();
        let t1 = std::time::Instant::now();
        let raw = match net.generate(w, h, &pixels, &Params { resolution: res, threshold: thr, ..Default::default() }, &JobCtx::detached()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{name}: FAILED {e}");
                continue;
            }
        };
        let t2 = std::time::Instant::now();
        let (mesh, notes) = studio_quality::scan::to_model(&raw.positions, &raw.colours, &raw.triangles, &studio_quality::scan::ModelOptions::default());
        let t3 = std::time::Instant::now();
        let topo = studio_quality::topo::analyse(&mesh);
        let uv = studio_quality::uv::analyse(&mesh);
        eprintln!(
            "{name}: cut {:.1}s, model {:.1}s ({} tris), finish {:.1}s -> {} tris; open edges {}, shells {}, specks {}, UV stretch {:.2}, spread {:.2}; {}",
            (t1 - t0).as_secs_f32(),
            (t2 - t1).as_secs_f32(),
            raw.triangles.len(),
            (t3 - t2).as_secs_f32(),
            mesh.triangles(),
            topo.boundary_edges,
            topo.components,
            topo.specks,
            uv.stretch,
            uv.density_cov,
            notes.last().cloned().unwrap_or_default()
        );
        let (c, r) = frame(&mesh);
        let views: Vec<_> = [20.0, 110.0, 200.0, 290.0].iter().map(|&y| render(&mesh, View { yaw: y, pitch: 12.0 }, 420, c, r)).collect();
        let (sw, sh, px) = sheet(&views.iter().collect::<Vec<_>>(), 4);
        write_png(&out.join(format!("{name}_turn.png")), sw, sh, &px).unwrap();
        if let Some(tex) = mesh.materials.iter().find_map(|m| m.texture.as_ref()) {
            write_png(&out.join(format!("{name}_texture.png")), tex.width as usize, tex.height as usize, &tex.rgba).unwrap();
        }
    }
}
