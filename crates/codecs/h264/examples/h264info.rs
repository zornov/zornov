// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decode an MP4/MOV (or Annex-B .264) file and print per-frame statistics.
//! Usage: cargo run --release -p studio-h264 --example h264info -- <file> [out.yuv]

#[path = "../tests/common/mp4mini.rs"]
mod mp4mini;

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("file");
    let data = std::fs::read(path).expect("read");
    let mut out = args.get(2).map(|p| std::fs::File::create(p).expect("create"));
    let t0 = std::time::Instant::now();
    let mut frames = Vec::new();
    let mut errors = 0;
    if path.ends_with(".264") || path.ends_with(".h264") || path.ends_with(".avc") {
        let mut dec = studio_h264::Decoder::new_annex_b();
        match dec.decode(&data) {
            Ok(f) => frames.extend(f),
            Err(e) => {
                errors += 1;
                eprintln!("error: {e}");
            }
        }
        frames.extend(dec.flush());
    } else {
        let track = mp4mini::read_video_track(&data).expect("track");
        let mut dec = studio_h264::Decoder::from_avcc_bytes(&track.avcc).expect("dec");
        for (i, s) in track.samples.iter().enumerate() {
            match dec.decode_with_pts(s, Some(i as i64)) {
                Ok(f) => frames.extend(f),
                Err(e) => {
                    errors += 1;
                    eprintln!("sample {i}: {e}");
                }
            }
        }
        frames.extend(dec.flush());
    }
    let el = t0.elapsed();
    for (i, f) in frames.iter().enumerate() {
        let sum: u64 = f.y.iter().map(|&v| v as u64).sum();
        println!("frame {i}: {}x{} poc {} pts {:?} mean_y {:.2}", f.width, f.height, f.poc, f.pts, sum as f64 / f.y.len() as f64);
        if let Some(o) = out.as_mut() {
            o.write_all(&f.y).unwrap();
            o.write_all(&f.u).unwrap();
            o.write_all(&f.v).unwrap();
        }
    }
    println!("{} frames, {} errors, {:.1} ms ({:.0} fps)", frames.len(), errors, el.as_secs_f64() * 1e3, frames.len() as f64 / el.as_secs_f64());
}
