// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Developer probe: border colour statistics and re-encode sizes for a PNG.
use std::path::Path;
fn main() {
    let path = std::env::args().nth(1).expect("png path");
    let img = studio_media::png::read(Path::new(&path)).unwrap();
    let (w, h) = (img.width as usize, img.height as usize);
    let band = 40;
    let mut n = 0; let mut green = 0; let mut sum = [0u64; 4];
    let mut samples = vec![];
    for y in 0..h { for x in 0..w {
        if x >= band && x < w - band && y >= band && y < h - band { continue; }
        let p = &img.pixels[(y * w + x) * 4..(y * w + x) * 4 + 4];
        n += 1; for c in 0..4 { sum[c] += p[c] as u64; }
        if p[1] as i32 > p[0] as i32 + 40 && p[1] as i32 > p[2] as i32 + 40 { green += 1; }
        if (x + y) % 997 == 0 { samples.push([p[0], p[1], p[2], p[3]]); }
    }}
    println!("{w}x{h} border mean rgba {:?} dominant-green {:.2}", sum.map(|s| s / n as u64), green as f64 / n as f64);
    println!("samples {:?}", &samples[..samples.len().min(12)]);
    println!("detect_key -> {:?}", studio_content::key::detect_key(&img));
    let orig = std::fs::metadata(&path).unwrap().len();
    for side in [512u32, 448, 384, 320] {
        let r = studio_content::image::resize_to_fit(&img, side);
        let ours = studio_content::png::encode(&r).len();
        let fixed = studio_media::png::encode(&r).len();
        println!("side {side}: {}x{} ours {} KB, media-fixed {} KB (orig {} KB)", r.width, r.height, ours / 1024, fixed / 1024, orig / 1024);
    }
}
