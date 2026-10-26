// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 1080p performance and bit-exactness checks (ignored; run in release):
//!
//! ```text
//! cargo test --release -p studio-h264 --test perf1080 -- --ignored --nocapture
//! ```
//!
//! `perf_1080p_hashes` compares an FNV-1a hash of every decoded picture's Y/U/V planes
//! against `tests/data/perf_1080p_hashes.txt` (recorded before any optimisation work).
//! Set `H264_PERF_BLESS=1` to rewrite that file.

mod common;

use std::time::Instant;

use studio_h264::{Decoder, Frame};

const FILES: [&str; 2] = ["perf/h264_1080p30.mp4", "perf/h264_1080p60.mp4"];

fn fnv(h: &mut u64, data: &[u8]) {
    for &b in data {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

fn frame_hash(f: &Frame) -> String {
    let mut out = String::new();
    for p in [&f.y, &f.u, &f.v] {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        fnv(&mut h, p);
        out.push_str(&format!("{h:016x} "));
    }
    out.push_str(&format!("poc={}", f.poc));
    out
}

/// CPU time of the calling thread in seconds (0 where unsupported).
fn thread_cpu_secs() -> f64 {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        #[repr(C)]
        struct Timespec {
            sec: i64,
            nsec: i64,
        }
        unsafe extern "C" {
            fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
        }
        #[cfg(target_os = "macos")]
        const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
        #[cfg(target_os = "linux")]
        const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
        let mut ts = Timespec { sec: 0, nsec: 0 };
        // SAFETY: plain libc call writing into `ts`.
        if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0 {
            return ts.sec as f64 + ts.nsec as f64 * 1e-9;
        }
    }
    0.0
}

/// Decode all samples; returns (frames, decode seconds, picture count).
fn decode_all(name: &str) -> (Vec<Frame>, f64, usize) {
    let (f, wall, _cpu, n) = decode_all_cpu(name);
    (f, wall, n)
}

/// As `decode_all`, plus the CPU seconds of the calling (entropy decoding) thread.
fn decode_all_cpu(name: &str) -> (Vec<Frame>, f64, f64, usize) {
    let path = common::media_dir().join(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let track = common::mp4mini::read_video_track(&data).expect("video track");
    let mut dec = Decoder::from_avcc_bytes(&track.avcc).expect("decoder");
    let mut frames = Vec::new();
    let t = Instant::now();
    let c = thread_cpu_secs();
    for (i, s) in track.samples.iter().enumerate() {
        frames.extend(dec.decode_with_pts(s, Some(i as i64)).unwrap_or_else(|e| panic!("{name}: sample {i}: {e}")));
    }
    frames.extend(dec.flush());
    let wall = t.elapsed().as_secs_f64();
    (frames, wall, thread_cpu_secs() - c, track.samples.len())
}

#[test]
#[ignore]
fn perf_1080p_timing() {
    for name in FILES {
        // Warm-up run, then the best of five (the machine may be shared).
        let _ = decode_all(name);
        let (mut best, mut best_cpu) = (f64::MAX, f64::MAX);
        let mut n = 0;
        for _ in 0..5 {
            let (_f, secs, cpu, count) = decode_all_cpu(name);
            best = best.min(secs);
            best_cpu = best_cpu.min(cpu);
            n = count;
        }
        println!(
            "{name}: {n} pictures, {:.2} ms/picture (calling thread CPU {:.2} ms/picture)",
            best * 1000.0 / n as f64,
            best_cpu * 1000.0 / n as f64
        );
    }
}

#[test]
#[ignore]
fn perf_1080p_hashes() {
    let golden = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/perf_1080p_hashes.txt");
    let mut text = String::new();
    for name in FILES {
        let (frames, _, _) = decode_all(name);
        for (i, f) in frames.iter().enumerate() {
            text.push_str(&format!("{name} {i} {}\n", frame_hash(f)));
        }
    }
    if std::env::var_os("H264_PERF_BLESS").is_some() {
        std::fs::write(&golden, &text).expect("write golden");
        println!("wrote {}", golden.display());
        return;
    }
    let want = std::fs::read_to_string(&golden).expect("golden hashes (run with H264_PERF_BLESS=1 first)");
    for (a, b) in want.lines().zip(text.lines()) {
        assert_eq!(a, b, "picture hash mismatch");
    }
    assert_eq!(want.lines().count(), text.lines().count(), "picture count");
}
