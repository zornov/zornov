// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Conformance against Apple's decodes of the Studio test fixtures.

mod common;

use common::mp4mini::read_hevc_track;
use studio_hevc::{Decoder, DownConvert, Frame};
use std::path::PathBuf;
use std::time::Instant;

fn media_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media")
}

fn decode_file(name: &str) -> (Vec<Frame>, f64) {
    let data = std::fs::read(media_dir().join(name)).expect("fixture");
    let track = read_hevc_track(&data).expect("hevc track");
    let t0 = Instant::now();
    let mut dec = Decoder::from_hvcc(&track.hvcc).expect("decoder");
    let mut frames = Vec::new();
    for (i, s) in track.samples.iter().enumerate() {
        match dec.decode(s) {
            Ok(f) => frames.extend(f),
            Err(e) => panic!("{name}: sample {i}: {e}"),
        }
    }
    frames.extend(dec.flush());
    (frames, t0.elapsed().as_secs_f64())
}

struct Cmp {
    frames: usize,
    samples: usize,
    exact: usize,
    within1: usize,
    max_diff: u8,
    first_bad_frame: Option<usize>,
}

fn compare(frames: &[Frame], reference: &[u8], mode: DownConvert) -> Cmp {
    let mut c = Cmp { frames: frames.len(), samples: 0, exact: 0, within1: 0, max_diff: 0, first_bad_frame: None };
    let mut off = 0usize;
    for (fi, f) in frames.iter().enumerate() {
        let ours = f.to_i420_u8(mode);
        let refr = &reference[off.min(reference.len())..(off + ours.len()).min(reference.len())];
        off += ours.len();
        let mut bad = false;
        for (a, b) in ours.iter().zip(refr.iter()) {
            let d = a.abs_diff(*b);
            c.samples += 1;
            if d == 0 {
                c.exact += 1;
            }
            if d <= 1 {
                c.within1 += 1;
            }
            if d != 0 {
                bad = true;
            }
            c.max_diff = c.max_diff.max(d);
        }
        if bad && c.first_bad_frame.is_none() {
            c.first_bad_frame = Some(fi);
        }
    }
    c
}

#[test]
fn hevc_main_bit_exact() {
    let (frames, secs) = decode_file("hevc_main.mp4");
    let reference = read_yuv(&media_dir().join("hevc_main.yuv"));
    assert_eq!(frames.len(), 30);
    assert!(frames.iter().all(|f| f.width == 320 && f.height == 240 && f.bit_depth == 8));
    let c = compare(&frames, &reference, DownConvert::Truncate);
    eprintln!(
        "hevc_main: {} frames in {:.3}s ({:.0} fps); exact {}/{} samples, max diff {}, first bad frame {:?}",
        c.frames,
        secs,
        c.frames as f64 / secs,
        c.exact,
        c.samples,
        c.max_diff,
        c.first_bad_frame
    );
    assert_eq!(c.samples, reference.len());
    assert_eq!(c.exact, c.samples, "hevc_main must be bit-exact");
}

#[test]
fn hevc_main10_matches() {
    let (frames, secs) = decode_file("hevc_main10.mov");
    let reference = read_yuv(&media_dir().join("hevc_main10.yuv"));
    assert_eq!(frames.len(), 24);
    assert!(frames.iter().all(|f| f.width == 320 && f.height == 240 && f.bit_depth == 10));
    let t = compare(&frames, &reference, DownConvert::Truncate);
    let r = compare(&frames, &reference, DownConvert::Round);
    for (name, c) in [("v >> 2", &t), ("(v + 2) >> 2", &r)] {
        eprintln!(
            "hevc_main10 [{name}]: {} frames in {:.3}s; exact {:.4}%, within +-1 {:.4}%, max diff {}",
            c.frames,
            secs,
            100.0 * c.exact as f64 / c.samples as f64,
            100.0 * c.within1 as f64 / c.samples as f64,
            c.max_diff
        );
    }
    assert_eq!(t.samples, reference.len());
    let best = if t.exact >= r.exact { &t } else { &r };
    assert!(best.within1 as f64 >= 0.999 * best.samples as f64, "main10 mismatch");
}

/// Reads a reference I420 dump kept gzipped in the repository (`name` + `.gz`).
fn read_yuv(path: &std::path::Path) -> Vec<u8> {
    use std::io::Read;
    let gz = std::fs::File::open(path.with_extension("yuv.gz")).expect("reference yuv.gz");
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut out).expect("gunzip reference yuv");
    out
}
