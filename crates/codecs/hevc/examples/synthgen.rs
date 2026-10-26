// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generates random valid HEVC streams (feature `synth`):
//! `synthgen <first_seed> <count> <out_dir>` writes `<seed>.hevc` and the
//! generator's own decode `<seed>.gen.yuv` for each successfully generated seed.

use studio_hevc::{Frame, PlaneData};
use std::io::Write;

fn write_frames(path: &std::path::Path, frames: &[Frame]) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for fr in frames {
        for p in [&fr.y, &fr.u, &fr.v] {
            match p {
                PlaneData::U8(v) => f.write_all(v).unwrap(),
                PlaneData::U16(v) => {
                    for &s in v {
                        f.write_all(&s.to_le_bytes()).unwrap();
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let first: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let count: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let dir = std::path::PathBuf::from(args.get(3).cloned().unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ok = 0;
    for seed in first..first + count {
        match studio_hevc::synth::generate(seed) {
            Ok((stream, frames)) => {
                std::fs::write(dir.join(format!("{seed}.hevc")), &stream).unwrap();
                write_frames(&dir.join(format!("{seed}.gen.yuv")), &frames);
                let f0 = frames.first();
                println!(
                    "{seed}: {} bytes, {} frames {}x{} bd{}",
                    stream.len(),
                    frames.len(),
                    f0.map(|f| f.width).unwrap_or(0),
                    f0.map(|f| f.height).unwrap_or(0),
                    f0.map(|f| f.bit_depth).unwrap_or(0)
                );
                ok += 1;
            }
            Err(e) => println!("{seed}: generation failed: {e}"),
        }
    }
    println!("{ok}/{count} generated");
}
