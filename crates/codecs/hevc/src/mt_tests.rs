// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multi-threaded decoding must be bit-exact with single-threaded decoding.
//! The regression corpus (tests/data) consists of small pictures, so these
//! tests force the threaded paths (WPP row decoding, parallel deblocking and
//! SAO) regardless of picture size.

use crate::decoder::Decoder;
use crate::error::Error;
use crate::frame::{Frame, PlaneData};
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn hash_frames(frames: &[Frame]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut upd = |b: &[u8]| {
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for f in frames {
        for p in [&f.y, &f.u, &f.v] {
            match p {
                PlaneData::U8(v) => upd(v),
                PlaneData::U16(v) => {
                    for &s in v {
                        upd(&s.to_le_bytes());
                    }
                }
            }
        }
    }
    h
}

fn decode(data: &[u8], threads: usize) -> (Vec<Frame>, usize) {
    let mut dec = Decoder::new::<&[u8]>(&[], 4).unwrap();
    dec.set_threading(threads, 0);
    let mut frames = Vec::new();
    let mut errors = 0;
    match dec.decode_annexb(data) {
        Ok(f) => frames.extend(f),
        Err(e) => {
            assert!(!matches!(e, Error::Internal(_)), "internal error {e}");
            errors += 1
        }
    }
    frames.extend(dec.flush());
    (frames, errors)
}

fn corpus() -> Vec<(String, usize, u64)> {
    let manifest = std::fs::read_to_string(data_dir().join("expected.txt")).expect("manifest");
    let mut out = Vec::new();
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        out.push((parts[0].to_string(), parts[1].parse().unwrap(), u64::from_str_radix(parts[2], 16).unwrap()));
    }
    out
}

#[test]
fn threaded_corpus_matches_reference() {
    let mut failures = Vec::new();
    let list = corpus();
    let par0 = crate::wpp::STATS_PARALLEL.load(std::sync::atomic::Ordering::Relaxed);
    assert!(list.len() > 50);
    for threads in [2usize, 3, 8] {
        for (name, frames, hash) in &list {
            let data = std::fs::read(data_dir().join(format!("{name}.hevc"))).expect("stream");
            let (out, errors) = decode(&data, threads);
            if out.len() != *frames || hash_frames(&out) != *hash || errors != 0 {
                failures.push(format!("{name} ({threads} threads): {} frames, errors {errors}", out.len()));
            }
        }
    }
    assert!(failures.is_empty(), "threaded corpus failures:\n{}", failures.join("\n"));
    let par = crate::wpp::STATS_PARALLEL.load(std::sync::atomic::Ordering::Relaxed) - par0;
    eprintln!("parallel WPP segments: {par}, fallbacks so far: {}", crate::wpp::STATS_FALLBACK.load(std::sync::atomic::Ordering::Relaxed));
    assert!(par > 100, "WPP parallel path not exercised ({par})");
}

/// Corrupted streams decoded multi-threaded must not panic or hang.
#[test]
fn threaded_corrupted_streams_never_panic() {
    let mut rng = 0x1234_5678_9abc_def1u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    for (name, _, _) in corpus().iter().filter(|(n, _, _)| n.contains("wpp") || n.starts_with("x265")) {
        let data = std::fs::read(data_dir().join(format!("{name}.hevc"))).expect("stream");
        for _ in 0..6 {
            let mut v = data.clone();
            let flips = 1 + (next() % 16) as usize;
            for _ in 0..flips {
                let i = 64 + (next() as usize) % (v.len() - 64);
                v[i] ^= 1 << (next() % 8);
            }
            let _ = decode(&v, 4);
        }
    }
}
