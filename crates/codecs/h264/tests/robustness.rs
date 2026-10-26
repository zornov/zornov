// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Truncated and corrupted input must produce errors or concealed frames, never panics.

mod common;

use common::mp4mini;
use studio_h264::Decoder;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn load(name: &str) -> mp4mini::Track {
    let data = std::fs::read(common::media_dir().join(name)).unwrap();
    mp4mini::read_video_track(&data).unwrap()
}

/// Corrupt one sample in one of several ways.
fn corrupt(rng: &mut Rng, s: &[u8], nal_len: usize) -> Vec<u8> {
    let mut v = s.to_vec();
    if v.len() <= nal_len + 1 {
        return v;
    }
    match rng.below(7) {
        0 => {
            // Flip a few bits in the payload (keep the length prefix intact).
            for _ in 0..1 + rng.below(8) {
                let i = nal_len + 1 + rng.below(v.len() - nal_len - 1);
                v[i] ^= 1 << rng.below(8);
            }
        }
        1 => {
            // Truncate the payload but keep a consistent length prefix.
            let keep = nal_len + 1 + rng.below(v.len() - nal_len - 1);
            v.truncate(keep);
            let len = keep - nal_len;
            for k in 0..nal_len {
                v[k] = (len >> (8 * (nal_len - 1 - k))) as u8;
            }
        }
        2 => {
            // Truncate without fixing the prefix.
            let keep = rng.below(v.len());
            v.truncate(keep);
        }
        3 => {
            // Overwrite a run with random bytes.
            let start = nal_len + 1 + rng.below(v.len() - nal_len - 1);
            let n = (1 + rng.below(64)).min(v.len() - start);
            for b in &mut v[start..start + n] {
                *b = rng.next() as u8;
            }
        }
        4 => {
            // Zero a run.
            let start = nal_len + 1 + rng.below(v.len() - nal_len - 1);
            let n = (1 + rng.below(256)).min(v.len() - start);
            v[start..start + n].fill(0);
        }
        5 => {
            // Change the NAL header (type / ref idc).
            v[nal_len] = (v[nal_len] & 0x80) | (rng.next() as u8 & 0x7f);
        }
        _ => {
            // Completely random sample.
            let n = rng.below(2000);
            v = (0..n).map(|_| rng.next() as u8).collect();
        }
    }
    v
}

fn fuzz_fixture(name: &str, seed: u64, rounds: usize) {
    let track = load(name);
    let nal_len = (track.avcc[4] & 3) as usize + 1;
    let mut rng = Rng(seed);
    for _ in 0..rounds {
        let mut dec = Decoder::from_avcc_bytes(&track.avcc).unwrap();
        let p = 1 + rng.below(4);
        for s in &track.samples {
            let input = if rng.below(10) < p { corrupt(&mut rng, s, nal_len) } else { s.clone() };
            let _ = dec.decode(&input);
        }
        let _ = dec.flush();
    }
}

#[test]
fn fuzz_baseline_cavlc() {
    fuzz_fixture("h264_baseline_cavlc.mp4", 1, 40);
}

#[test]
fn fuzz_main_cabac() {
    fuzz_fixture("h264_main_cabac_bframes.mp4", 2, 40);
}

#[test]
fn fuzz_main_cavlc_b() {
    fuzz_fixture("h264_main_cavlc_bframes.mp4", 3, 40);
}

#[test]
fn fuzz_high_cabac() {
    fuzz_fixture("h264_high_cabac.mp4", 4, 25);
}

#[test]
fn fuzz_high_odd() {
    fuzz_fixture("h264_high_odd_size.mov", 5, 40);
}

#[test]
fn truncated_every_length() {
    // Every prefix of the first few samples (IDR incl.), each fed to a fresh decoder state.
    let track = load("h264_main_cabac_bframes.mp4");
    let nal_len = (track.avcc[4] & 3) as usize + 1;
    for s in track.samples.iter().take(3) {
        for keep in (nal_len + 1..s.len()).step_by(7) {
            let mut v = s[..keep].to_vec();
            let len = keep - nal_len;
            for k in 0..nal_len {
                v[k] = (len >> (8 * (nal_len - 1 - k))) as u8;
            }
            let mut dec = Decoder::from_avcc_bytes(&track.avcc).unwrap();
            let _ = dec.decode(&track.samples[0]);
            let _ = dec.decode(&v);
            let _ = dec.flush();
        }
    }
}

#[test]
fn garbage_parameter_sets_and_config() {
    let mut rng = Rng(99);
    for _ in 0..2000 {
        let n = rng.below(64);
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        let _ = studio_h264::AvcConfig::parse(&bytes);
        let _ = Decoder::from_avcc_bytes(&bytes);
        // Random SPS / PPS / slice NALs through the Annex-B path.
        let mut d = Decoder::new_annex_b();
        let mut s = vec![0, 0, 0, 1, [0x67u8, 0x68, 0x65, 0x41, 0x01][rng.below(5)]];
        s.extend_from_slice(&bytes);
        let _ = d.decode(&s);
        let _ = d.flush();
    }
}

#[test]
fn corrupted_stream_reports_error_or_conceals() {
    // A corrupted P frame must not prevent decoding of later frames.
    let track = load("h264_baseline_cavlc.mp4");
    let mut dec = Decoder::from_avcc_bytes(&track.avcc).unwrap();
    let mut frames = 0;
    let mut errors = 0;
    for (i, s) in track.samples.iter().enumerate() {
        let mut v = s.clone();
        if i == 5 {
            let n = v.len();
            v[n / 2..].fill(0xff);
        }
        match dec.decode(&v) {
            Ok(f) => frames += f.len(),
            Err(_) => errors += 1,
        }
    }
    frames += dec.flush().len();
    assert!(errors >= 1);
    assert_eq!(frames, track.samples.len());
}
