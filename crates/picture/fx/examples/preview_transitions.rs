// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Renders every transition at a few progress values and writes contact
//! sheets: `cargo run -p studio-fx --example preview_transitions -- <out_dir>`.

use studio_fx::effects;
use studio_fx::testcard::test_card;
use studio_fx::transitions::{self, TransitionContext};
use studio_fx::{EffectContext, Image};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fx-preview".into()));
    std::fs::create_dir_all(&out).unwrap();
    let (w, h) = (320, 180);
    let a = test_card(w, h, 0.0);
    let grad = effects::find("gradient").unwrap();
    let b = grad.apply(&EffectContext::still(), &Image::new(w, h), &grad.defaults());
    let steps = [0.2, 0.4, 0.6, 0.8];
    let list = transitions::all();
    for (n, chunk) in list.chunks(6).enumerate() {
        let mut sheet = Image::filled(w * steps.len(), h * chunk.len(), [0.0, 0.0, 0.0, 1.0]);
        for (row, t) in chunk.iter().enumerate() {
            for (col, &p) in steps.iter().enumerate() {
                let img = t.apply(&TransitionContext { seed: 3 }, &a, &b, p, &t.defaults());
                for y in 0..h {
                    for x in 0..w {
                        let px = img.get(x, y);
                        sheet.set(col * w + x, row * h + y, [px[0], px[1], px[2], 1.0]);
                    }
                }
            }
            println!("sheet {n} row {row}: {}", t.id);
        }
        let rgba = studio_media::RgbaImage { width: sheet.width as u32, height: sheet.height as u32, pixels: sheet.to_srgb8() };
        std::fs::write(out.join(format!("_trans{n}.png")), studio_media::png::encode(&rgba)).unwrap();
    }
}
