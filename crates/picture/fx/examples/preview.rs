// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Renders every effect at its defaults on the test card and writes PNGs:
//! `cargo run -p studio-fx --example preview -- <out_dir> [width height]`.

use studio_fx::effects::{self, EffectContext};
use studio_fx::testcard::test_card;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("fx-preview"));
    let w: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(320);
    let h: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(180);
    std::fs::create_dir_all(&out).unwrap();
    let card = test_card(w, h, 0.0);
    let history: Vec<_> = (1..=40).map(|k| test_card(w, h, -0.01 * k as f32)).collect();
    let plate = test_card(w, h, 0.5);
    let ctx = EffectContext { time: 1.3, frame_rate: 30.0, seed: 1, history: &history, aux: Some(&plate) };
    save(&out.join("_card.png"), &card);
    let mut tiles = vec![card.clone()];
    for e in effects::all() {
        let t = std::time::Instant::now();
        let img = e.apply(&ctx, &card, &e.defaults());
        println!("{:24} {:>6.1} ms", e.id, t.elapsed().as_secs_f64() * 1000.0);
        save(&out.join(format!("{}.png", e.id)), &img);
        tiles.push(img);
    }
    // Contact sheets, 4 x 3 tiles each, in catalogue order (card first).
    for (n, chunk) in tiles.chunks(12).enumerate() {
        let mut sheet = studio_fx::Image::filled(w * 4, h * 3, [0.2, 0.0, 0.2, 1.0]);
        for (i, t) in chunk.iter().enumerate() {
            let (ox, oy) = ((i % 4) * w, (i / 4) * h);
            for y in 0..h {
                for x in 0..w {
                    // Dark checker behind transparency.
                    let bg = if (x / 8 + y / 8) % 2 == 0 { 0.05 } else { 0.1 };
                    let p = t.get(x, y);
                    let k = 1.0 - p[3];
                    sheet.set(ox + x, oy + y, [p[0] + bg * k, p[1] + bg * k, p[2] + bg * k, 1.0]);
                }
            }
        }
        save(&out.join(format!("_sheet{n}.png")), &sheet);
    }
}

fn save(path: &std::path::Path, img: &studio_fx::Image) {
    let rgba = studio_media::RgbaImage { width: img.width as u32, height: img.height as u32, pixels: img.to_srgb8() };
    std::fs::write(path, studio_media::png::encode(&rgba)).unwrap();
}
