// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Write the synthetic test streams (and generator-predicted output where available).
//! Usage: h264synth <out-dir>    |    h264synth --hash <file>
#[path = "../tests/common/writer.rs"]
mod writer;
#[path = "../tests/common/synth.rs"]
mod synth;

fn fnv(data: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1] == "--hash" {
        let d = std::fs::read(&args[2]).unwrap();
        println!("{:016x} {}", fnv(&d), d.len());
        return;
    }
    let dir = std::path::Path::new(&args[1]);
    std::fs::create_dir_all(dir).unwrap();
    for s in synth::all() {
        std::fs::write(dir.join(format!("{}.264", s.name)), &s.data).unwrap();
        if let Some(e) = &s.expected {
            std::fs::write(dir.join(format!("{}.expected.yuv", s.name)), e).unwrap();
        }
        println!("{} {} bytes", s.name, s.data.len());
    }
}
