// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Corrupted input must produce errors or concealed pictures, never panics.
//! (Internal panics would surface as `Error::Internal` through the decoder's
//! safety net; this test requires that never happens.)

mod common;

use common::mp4mini::read_hevc_track;
use studio_hevc::{Decoder, Error};
use std::path::PathBuf;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Iteration multiplier (set STUDIO_FUZZ_SCALE for longer runs).
fn scale() -> usize {
    std::env::var("STUDIO_FUZZ_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(1)
}

fn media(name: &str) -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media").join(name)).unwrap()
}

fn check(r: &Result<Vec<studio_hevc::Frame>, Error>, what: &str) {
    if let Err(Error::Internal(m)) = r {
        panic!("{what}: internal error (panic) {m}");
    }
}

fn corrupt(rng: &mut Rng, s: &[u8]) -> Vec<u8> {
    let mut v = s.to_vec();
    match rng.below(6) {
        0 => {
            // bit flips
            for _ in 0..1 + rng.below(8) {
                if v.is_empty() {
                    break;
                }
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
        }
        1 => {
            // truncate
            let n = rng.below(v.len() + 1);
            v.truncate(n);
        }
        2 => {
            // overwrite a run with random bytes
            if !v.is_empty() {
                let a = rng.below(v.len());
                let b = (a + 1 + rng.below(64)).min(v.len());
                for x in &mut v[a..b] {
                    *x = rng.next() as u8;
                }
            }
        }
        3 => {
            // zero a run (keeps NAL lengths, destroys payload)
            if v.len() > 8 {
                let a = 4 + rng.below(v.len() - 4);
                let b = (a + rng.below(200)).min(v.len());
                for x in &mut v[a..b] {
                    *x = 0;
                }
            }
        }
        4 => {
            // duplicate a chunk
            if !v.is_empty() {
                let a = rng.below(v.len());
                let b = (a + rng.below(100)).min(v.len());
                let chunk = v[a..b].to_vec();
                v.splice(a..a, chunk);
            }
        }
        _ => {
            // flip bytes in the slice header region of the first NAL
            for i in 4..v.len().min(24) {
                if rng.below(3) == 0 {
                    v[i] = rng.next() as u8;
                }
            }
        }
    }
    v
}

#[test]
fn corrupted_samples_never_panic() {
    for (file, seed) in [("hevc_main.mp4", 1u64), ("hevc_main10.mov", 7u64)] {
        let data = media(file);
        let track = read_hevc_track(&data).unwrap();
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1));
        for round in 0..40 * scale() {
            let mut dec = Decoder::from_hvcc(&track.hvcc).unwrap();
            for (i, s) in track.samples.iter().enumerate() {
                let input = if rng.below(4) == 0 { corrupt(&mut rng, s) } else { s.clone() };
                let r = dec.decode(&input);
                check(&r, &format!("{file} round {round} sample {i}"));
            }
            let _ = dec.flush();
            // decoder is still usable afterwards
            let r = dec.decode(&track.samples[0]);
            check(&r, "after flush");
        }
    }
}

#[test]
fn corrupted_parameter_sets_never_panic() {
    let data = media("hevc_main.mp4");
    let track = read_hevc_track(&data).unwrap();
    let (len, sets) = studio_hevc::parse_hvcc(&track.hvcc).unwrap();
    let mut rng = Rng(12345);
    for round in 0..300 * scale() {
        let mut sets2 = sets.clone();
        let k = rng.below(sets2.len());
        sets2[k] = corrupt(&mut rng, &sets2[k]);
        match Decoder::new(&sets2, len) {
            Ok(mut dec) => {
                for s in track.samples.iter().take(6) {
                    let r = dec.decode(s);
                    check(&r, &format!("ps round {round}"));
                }
                let _ = dec.flush();
            }
            Err(Error::Internal(m)) => panic!("internal error {m}"),
            Err(_) => {}
        }
    }
}

#[test]
fn random_garbage_never_panics() {
    let mut rng = Rng(777);
    for round in 0..300 * scale() {
        let n = rng.below(4000);
        let mut v: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        // sprinkle start codes and plausible NAL headers
        for _ in 0..rng.below(20) {
            if v.len() > 6 {
                let i = rng.below(v.len() - 6);
                let t = [32u8, 33, 34, 1, 19, 21, 0][rng.below(7)];
                v[i..i + 5].copy_from_slice(&[0, 0, 1, t << 1, 1]);
            }
        }
        let mut dec = Decoder::new::<&[u8]>(&[], 4).unwrap();
        let r = dec.decode_annexb(&v);
        check(&r, &format!("garbage round {round}"));
        let r = dec.decode(&v);
        check(&r, &format!("garbage lp round {round}"));
        let _ = dec.flush();
    }
}

/// Valid parameter sets followed by mutated slice data from the corpus.
#[test]
fn corrupted_corpus_streams_never_panic() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut rng = Rng(4242);
    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries.iter().filter(|e| e.path().extension().is_some_and(|x| x == "hevc")).step_by(3) {
        let data = std::fs::read(e.path()).unwrap();
        for _ in 0..6 * scale() {
            let mut v = data.clone();
            for _ in 0..1 + rng.below(6) {
                // keep the first 100 bytes (parameter sets) intact more often
                let lo = if rng.below(2) == 0 { 100.min(v.len()) } else { 0 };
                if v.len() > lo {
                    let i = lo + rng.below(v.len() - lo);
                    v[i] ^= 1 << rng.below(8);
                }
            }
            let mut dec = Decoder::new::<&[u8]>(&[], 4).unwrap();
            let r = dec.decode_annexb(&v);
            check(&r, &format!("{:?}", e.file_name()));
            let _ = dec.flush();
        }
    }
}
