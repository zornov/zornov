// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M8: tests for rate control and the GOP planner.
//!
//! The gate: with `RateMode::Bitrate` the average bitrate of 60 frames lands within +-5% of the
//! target on two kinds of moving content and several targets; with `RateMode::Crf` the quality
//! is steady (per-frame PSNR within a stated band) and falls as the CRF value rises; every frame
//! decodes with `studio-av1` to the encoder's reconstruction; key frames come at `keyint` and at
//! scene cuts.

use studio_av1::Decoder;
use studio_av1_enc::rc::gop::GopPlanner;
use studio_av1_enc::rc::{MAX_RETRIES, RateControl};
use studio_av1_enc::types::{Config, Frame, FrameType, RateMode};
use studio_av1_enc::Encoder;

/// The tolerance of the bitrate gate.
const BITRATE_TOLERANCE: f64 = 0.05;
/// The widest the per-frame luma PSNR may stray from the mean under CRF, in dB.
const CRF_PSNR_BAND: f64 = 3.0;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn image(w: usize, h: usize, mut f: impl FnMut(usize, usize, usize) -> u8) -> Frame {
    let mut fr = Frame::new(w as u32, h as u32, 8);
    for p in 0..3 {
        let (pw, ph) = if p == 0 { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
        for y in 0..ph {
            for x in 0..pw {
                fr.planes[p][y * fr.stride[p] + x] = f(p, x, y) as u16;
            }
        }
    }
    fr
}

fn canvas(x: f64, y: f64) -> f64 {
    90.0 + 45.0 * (x * 0.045).sin() * (y * 0.035).cos() + 30.0 * ((x + 2.0 * y) * 0.021).sin() + 12.0 * (x * 0.17).sin() * (y * 0.13).sin()
}

#[derive(Clone, Copy)]
enum Content {
    Pan,
    Object,
}

fn sequence(content: Content, w: usize, h: usize, n: usize) -> Vec<Frame> {
    (0..n)
        .map(|i| {
            image(w, h, |p, x, y| {
                let k = if p == 0 { 1.0 } else { 2.0 };
                let (fx, fy) = (x as f64 * k, y as f64 * k);
                let v = match content {
                    Content::Pan => canvas(fx + 2.5 * i as f64, fy + 1.25 * i as f64),
                    Content::Object => {
                        let (ox, oy) = (10.0 + 4.0 * i as f64, 20.0 + 1.5 * i as f64);
                        if fx >= ox && fx < ox + 50.0 && fy >= oy && fy < oy + 50.0 { 220.0 } else { canvas(fx, fy) }
                    }
                };
                let v = if p == 0 { v } else { 128.0 + (v - 90.0) * 0.3 };
                v.round().clamp(16.0, 235.0) as u8
            })
        })
        .collect()
}

fn noise_frame(w: usize, h: usize, seed: u64) -> Frame {
    let mut rng = Rng(seed);
    image(w, h, |_, _, _| (60 + rng.next() % 120) as u8)
}

fn psnr(a: &[u16], b: &[u16]) -> f64 {
    let mse = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

struct Run {
    bytes: Vec<usize>,
    keys: Vec<bool>,
    psnr: Vec<f64>,
}

/// Encodes the frames, decodes every packet and checks it against the reconstruction.
fn run(cfg: Config, frames: &[Frame]) -> Run {
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let mut r = Run { bytes: vec![], keys: vec![], psnr: vec![] };
    for (i, f) in frames.iter().enumerate() {
        let p = enc.push_frame(f, i as i64).unwrap().remove(0);
        let mut pics = dec.decode(&p.data).unwrap_or_else(|e| panic!("frame {i}: {e:?}"));
        pics.extend(dec.flush_result().unwrap());
        assert_eq!(pics.len(), 1);
        let want = enc.reconstruction().unwrap();
        for plane in 0..3 {
            assert!(pics[0].planes[plane] == want.planes[plane], "frame {i} plane {plane} differs from the reconstruction");
        }
        r.bytes.push(p.data.len());
        r.keys.push(p.keyframe);
        r.psnr.push(psnr(&pics[0].planes[0], &f.planes[0]));
    }
    r
}

fn cfg(w: usize, h: usize, rate: RateMode) -> Config {
    Config { rate, speed: 9, ..Config::new(w as u32, h as u32) }
}

// ---- the controller on a model ----

/// A frame size model: bits fall by half every ~30 quantiser indices; key frames cost 4x.
fn model_bits(q: u8, key: bool, complexity: f64) -> f64 {
    let base = 120_000.0 * complexity * 2f64.powf(-(q as f64) / 30.5);
    if key { 4.0 * base } else { base }
}

#[test]
fn the_controller_converges_on_a_model() {
    for &(kbps, complexity) in &[(300u32, 1.0f64), (1200, 1.0), (300, 2.5), (2000, 0.9)] {
        let mut c = cfg(320, 180, RateMode::Bitrate(kbps));
        c.keyint = 240;
        let mut rc = RateControl::new(&c);
        let mut total = 0.0;
        for i in 0..60 {
            let ft = if i == 0 { FrameType::Key } else { FrameType::Inter };
            let mut q = rc.frame_qidx(ft);
            let mut bits = model_bits(q, ft == FrameType::Key, complexity);
            for _ in 0..MAX_RETRIES {
                match rc.retry_qidx(ft, q, (bits / 8.0) as usize) {
                    Some(nq) => {
                        q = nq;
                        bits = model_bits(q, ft == FrameType::Key, complexity);
                    }
                    None => break,
                }
            }
            rc.update(ft, q, (bits / 8.0) as usize);
            total += bits;
        }
        let rate = total / 2.0 / 1000.0; // 60 frames at 30 fps
        assert!((rate / kbps as f64 - 1.0).abs() <= BITRATE_TOLERANCE, "target {kbps} kbps, complexity {complexity}: got {rate:.0} kbps");
    }
}

#[test]
fn a_far_overshoot_is_coded_again_at_a_coarser_quantiser() {
    let c = cfg(320, 180, RateMode::Bitrate(400));
    let mut rc = RateControl::new(&c);
    let q = rc.frame_qidx(FrameType::Inter);
    let target_bytes = 400_000.0 / 30.0 / 8.0;
    assert_eq!(rc.retry_qidx(FrameType::Inter, q, (target_bytes * 1.5) as usize), None, "a modest overshoot is left to the next frames");
    let nq = rc.retry_qidx(FrameType::Inter, q, (target_bytes * 8.0) as usize).expect("8x the target");
    assert!(nq > q + 60, "three octaves of size is about 90 indices: {q} -> {nq}");
    assert_eq!(rc.retry_qidx(FrameType::Inter, 255, 1_000_000), None, "nothing coarser exists");
    // a frame that used a tenth of its budget is coded again, finer
    let nq = rc.retry_qidx(FrameType::Inter, q, (target_bytes * 0.1) as usize).expect("a tenth of the target");
    assert!(nq < q, "{q} -> {nq}");
    assert_eq!(rc.retry_qidx(FrameType::Inter, q, (target_bytes * 0.6) as usize), None);
    assert_eq!(rc.retry_qidx(FrameType::Inter, 1, 1), None, "nothing finer exists");
    assert_eq!(rc.retry_qidx(FrameType::Key, q, 1), None, "a key frame target is a ceiling");
    // other modes never retry
    let mut crf = RateControl::new(&cfg(320, 180, RateMode::Crf(30)));
    let q = crf.frame_qidx(FrameType::Inter);
    assert_eq!(crf.retry_qidx(FrameType::Inter, q, 10_000_000), None);
}

// ---- the gate: bitrate ----

/// The targets span the useful range of both contents. For scale (release build, 60 frames at a
/// constant quantiser): pan spends 2136 kbps at `Cqp(1)`, 963 at `Cqp(8)`, 162 at `Cqp(48)`;
/// object spends 889 kbps at `Cqp(1)`, 686 at `Cqp(4)`, 148 at `Cqp(48)`. Object at 1000 kbps is
/// therefore above what any constant quantiser spends; the controller still lands it (a key frame
/// coded coarser than the P frames that follow leaves them more to code), but at a lower PSNR than
/// `Cqp(1)`, so that case checks the rate only.
#[test]
fn bitrate_lands_within_five_percent_over_60_frames() {
    for (name, content) in [("pan", Content::Pan), ("object", Content::Object)] {
        let frames = sequence(content, 320, 180, 60);
        for kbps in [150u32, 400, 700, 1000] {
            let r = run(cfg(320, 180, RateMode::Bitrate(kbps)), &frames);
            let total_bits: usize = r.bytes.iter().sum::<usize>() * 8;
            let rate = total_bits as f64 / 2.0 / 1000.0;
            let mean_psnr = r.psnr.iter().sum::<f64>() / r.psnr.len() as f64;
            eprintln!("bytes per frame: {:?}\nkeys at {:?}", r.bytes, r.keys.iter().enumerate().filter(|&(_, k)| *k).map(|(i, _)| i).collect::<Vec<_>>());
            eprintln!("{name:6} target {kbps:5} kbps: {rate:7.1} kbps ({:+.1}%), mean PSNR {mean_psnr:.2} dB, key frame {} B, largest P {} B", 100.0 * (rate / kbps as f64 - 1.0), r.bytes[0], r.bytes[1..].iter().max().unwrap());
            assert!((rate / kbps as f64 - 1.0).abs() <= BITRATE_TOLERANCE, "{name}: target {kbps} kbps, got {rate:.1} kbps");
            assert_eq!(r.keys.iter().filter(|&&k| k).count(), 1);
        }
    }
}

#[test]
fn a_higher_bitrate_buys_more_quality() {
    let frames = sequence(Content::Pan, 320, 180, 30);
    let psnr_at = |kbps| {
        let r = run(cfg(320, 180, RateMode::Bitrate(kbps)), &frames);
        r.psnr.iter().sum::<f64>() / r.psnr.len() as f64
    };
    let (lo, mid, hi) = (psnr_at(150), psnr_at(400), psnr_at(1000));
    assert!(lo < mid && mid < hi, "{lo:.2} {mid:.2} {hi:.2}");
}

#[test]
fn a_noisy_burst_does_not_blow_the_buffer() {
    // 20 smooth frames, 5 frames of noise, 20 smooth frames: the noise frames overshoot, are
    // coded again coarser, and the average over the whole run stays near the target
    let mut frames = sequence(Content::Pan, 320, 180, 45);
    for (k, f) in frames.iter_mut().enumerate().skip(20).take(5) {
        *f = noise_frame(320, 180, 99 + k as u64);
    }
    let target = 400u32;
    let r = run(Config { keyint: 0, ..cfg(320, 180, RateMode::Bitrate(target)) }, &frames);
    let rate = r.bytes.iter().sum::<usize>() as f64 * 8.0 / 1.5 / 1000.0;
    eprintln!("noisy burst: {rate:.1} kbps against {target}; noise frames {:?} B", &r.bytes[20..25]);
    assert!((rate / target as f64 - 1.0).abs() <= 0.15, "got {rate:.1} kbps");
    let avg_frame = target as f64 * 1000.0 / 30.0 / 8.0;
    assert!(r.bytes[20..25].iter().all(|&b| (b as f64) < 8.0 * avg_frame), "no frame is more than 8 times the average: {:?}", &r.bytes[20..25]);
}

// ---- constant quality ----

#[test]
fn crf_keeps_the_quality_steady_and_falls_with_the_value() {
    for (name, content) in [("pan", Content::Pan), ("object", Content::Object)] {
        let frames = sequence(content, 320, 180, 20);
        let mut means = Vec::new();
        for crf in [10u8, 22, 40] {
            let r = run(cfg(320, 180, RateMode::Crf(crf)), &frames);
            let p = &r.psnr[1..];
            let mean = p.iter().sum::<f64>() / p.len() as f64;
            let (lo, hi) = (p.iter().cloned().fold(f64::MAX, f64::min), p.iter().cloned().fold(f64::MIN, f64::max));
            eprintln!("{name:6} CRF {crf:2}: P frames {mean:.2} dB (min {lo:.2}, max {hi:.2}), key frame {:.2} dB, {} B", r.psnr[0], r.bytes.iter().sum::<usize>());
            assert!(lo >= mean - CRF_PSNR_BAND && hi <= mean + CRF_PSNR_BAND, "{name} CRF {crf}: PSNR {lo:.2}..{hi:.2} around {mean:.2} leaves the +-{CRF_PSNR_BAND} dB band");
            // (the hard-edged object loses detail in 64x64 intra blocks until the partition search
            // of the next step, whatever the quantiser)
            if name == "pan" {
                assert!(r.psnr[0] >= mean - 0.5, "the key frame is not coded worse than the frames that follow");
            }
            means.push((mean, r.bytes.iter().sum::<usize>()));
        }
        assert!(means[0].0 > means[1].0 && means[1].0 > means[2].0, "{name}: quality falls as CRF rises: {means:?}");
        assert!(means[0].1 > means[1].1 && means[1].1 > means[2].1, "{name}: bytes fall as CRF rises: {means:?}");
    }
}

// ---- key frames ----

#[test]
fn key_frames_come_at_keyint() {
    let frames = sequence(Content::Pan, 128, 96, 25);
    let r = run(Config { keyint: 10, ..cfg(128, 96, RateMode::Cqp(80)) }, &frames);
    let keys: Vec<usize> = r.keys.iter().enumerate().filter(|&(_, k)| *k).map(|(i, _)| i).collect();
    assert_eq!(keys, [0, 10, 20]);
}

#[test]
fn a_scene_cut_gets_a_key_frame() {
    let mut frames = sequence(Content::Pan, 128, 96, 6);
    // a different picture altogether, then more of it
    let other: Vec<Frame> = (0..6).map(|i| image(128, 96, |p, x, y| if p == 0 { (30 + (x * 2 + y * 3 + i * 2) % 190) as u8 } else { 100 })).collect();
    frames.extend(other);
    let r = run(Config { keyint: 0, ..cfg(128, 96, RateMode::Cqp(80)) }, &frames);
    let keys: Vec<usize> = r.keys.iter().enumerate().filter(|&(_, k)| *k).map(|(i, _)| i).collect();
    assert_eq!(keys, [0, 6], "the first frame and the cut");
}

#[test]
fn smooth_motion_is_not_a_scene_cut() {
    let mut g = GopPlanner::new(0);
    let frames = sequence(Content::Object, 160, 96, 20);
    let types: Vec<FrameType> = frames.iter().map(|f| g.next_type(f)).collect();
    assert_eq!(types[0], FrameType::Key);
    assert!(types[1..].iter().all(|&t| t == FrameType::Inter));
}

