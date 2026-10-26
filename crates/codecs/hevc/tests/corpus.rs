// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression corpus: small x265 streams and randomly synthesised streams
//! (tiles, PCM, dependent slices, long-term pictures, scaling lists, ...).
//! Each expected hash was verified against independent decoders (ffmpeg,
//! Apple VideoToolbox and/or libde265) when the corpus was built; the
//! verifying decoders are listed in tests/data/expected.txt.

use studio_hevc::{Decoder, Frame, PlaneData};
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

struct Fnv(u64);
impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf29ce484222325)
    }
    fn update(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

fn hash_frames(frames: &[Frame]) -> u64 {
    let mut h = Fnv::new();
    for f in frames {
        for p in [&f.y, &f.u, &f.v] {
            match p {
                PlaneData::U8(v) => h.update(v),
                PlaneData::U16(v) => {
                    for &s in v {
                        h.update(&s.to_le_bytes());
                    }
                }
            }
        }
    }
    h.0
}

fn decode_annexb(data: &[u8]) -> (Vec<Frame>, usize) {
    let mut dec = Decoder::new::<&[u8]>(&[], 4).unwrap();
    let mut frames = Vec::new();
    let mut errors = 0;
    match dec.decode_annexb(data) {
        Ok(f) => frames.extend(f),
        Err(_) => errors += 1,
    }
    frames.extend(dec.flush());
    (frames, errors)
}

#[test]
fn corpus_matches_reference_decoders() {
    let manifest = std::fs::read_to_string(data_dir().join("expected.txt")).expect("manifest");
    let mut failures = Vec::new();
    let mut n = 0;
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let (name, frames, hash) = (parts[0], parts[1].parse::<usize>().unwrap(), u64::from_str_radix(parts[2], 16).unwrap());
        let data = std::fs::read(data_dir().join(format!("{name}.hevc"))).expect("stream");
        let (out, errors) = decode_annexb(&data);
        n += 1;
        if out.len() != frames || hash_frames(&out) != hash || errors != 0 {
            failures.push(format!("{name}: {} frames (expected {frames}), errors {errors}", out.len()));
        }
    }
    assert!(n > 50, "corpus too small");
    assert!(failures.is_empty(), "corpus failures:\n{}", failures.join("\n"));
}

/// Feeding the same streams NAL by NAL (with explicit picture ends) must give
/// identical output.
#[test]
fn nal_by_nal_feeding_is_equivalent() {
    for name in ["x265_open_gop", "x265_slices_wpp", "synth_tiles_100", "synth_inter+lt+lt_convert_100"] {
        let path = data_dir().join(format!("{name}.hevc"));
        let Ok(data) = std::fs::read(&path) else { continue };
        let (reference, _) = decode_annexb(&data);
        let mut dec = Decoder::new::<&[u8]>(&[], 4).unwrap();
        let mut frames = Vec::new();
        for nal in studio_hevc::nal::split_annexb(&data) {
            let _ = dec.decode_nal(nal);
        }
        frames.extend(dec.end_of_picture());
        frames.extend(dec.flush());
        assert_eq!(hash_frames(&frames), hash_frames(&reference), "{name}");
    }
}
