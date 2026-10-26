// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Round trips through the workspace's hand-written H.264 decoder.

use studio_encode::h264::{Config, Encoder, Profile, RateMode};
use studio_encode::yuv::Yuv420;

/// A gradient background with a textured box moving across it and some
/// detail that changes over time.
fn synth(w: u32, h: u32, t: u32) -> Yuv420 {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let bx = (t * 5) as i32 % (w as i32 - 24).max(1);
    let by = (t * 3) as i32 % (h as i32 - 24).max(1);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let p = ((y as u32 * w + x as u32) * 4) as usize;
            let mut r = (x * 255 / w as i32) as u8;
            let mut g = (y * 255 / h as i32) as u8;
            let mut b = (((x + y) * 2 + t as i32 * 4) % 256) as u8;
            if x >= bx && x < bx + 24 && y >= by && y < by + 24 {
                let checker = ((x - bx) / 4 + (y - by) / 4) % 2 == 0;
                r = if checker { 230 } else { 30 };
                g = 200;
                b = if checker { 40 } else { 220 };
            }
            rgba[p] = r;
            rgba[p + 1] = g;
            rgba[p + 2] = b;
            rgba[p + 3] = 255;
        }
    }
    Yuv420::from_rgba(&rgba, w, h)
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

struct Run {
    min_psnr: f64,
    bytes: usize,
    frames: usize,
    keyframes: usize,
}

fn round_trip(cfg: Config, n: u32) -> Run {
    let (w, h) = (cfg.width, cfg.height);
    let mut enc = Encoder::new(cfg).unwrap();
    enc.keep_reconstruction(true);
    let mut dec = studio_h264::Decoder::from_avcc_bytes(&enc.avcc()).expect("avcC accepted");
    let mut min_psnr = f64::MAX;
    let mut bytes = 0;
    let mut frames = 0;
    let mut keyframes = 0;
    for t in 0..n {
        let src = synth(w, h, t);
        let f = enc.encode(&src);
        bytes += f.data.len();
        keyframes += f.keyframe as usize;
        let out = dec.decode(&f.data).expect("decodes");
        let rec = enc.reconstruction().unwrap().clone();
        for d in out {
            assert_eq!((d.width, d.height), (w & !1, h & !1));
            // The encoder's reconstruction must match the decoder exactly
            // (prediction, transforms and deblocking all agree).
            assert!(d.y == rec.y, "frame {t}: luma differs from the encoder's reconstruction");
            assert!(d.u == rec.u && d.v == rec.v, "frame {t}: chroma differs");
            let p = psnr(&d.y, &src.y);
            min_psnr = min_psnr.min(p);
            frames += 1;
        }
    }
    frames += dec.flush().len();
    assert_eq!(dec.concealed_macroblocks(), 0, "decoder concealed macroblocks");
    eprintln!("{}x{}: {} frames, {} bytes, min PSNR {:.2} dB", w, h, frames, bytes, min_psnr);
    Run { min_psnr, bytes, frames, keyframes }
}

fn cfg(w: u32, h: u32, profile: Profile, mode: RateMode) -> Config {
    Config { profile, rate_mode: mode, gop: 8, max_slices: 2, ..Config::new(w, h, 30, 1) }
}

#[test]
fn baseline_const_qp_matches_decoder() {
    let r = round_trip(cfg(176, 144, Profile::Baseline, RateMode::ConstQp(26)), 12);
    assert_eq!(r.frames, 12);
    assert_eq!(r.keyframes, 2);
    assert!(r.min_psnr > 30.0, "PSNR {:.1}", r.min_psnr);
}

#[test]
fn main_cabac_const_qp_matches_decoder() {
    let r = round_trip(cfg(176, 144, Profile::Main, RateMode::ConstQp(26)), 12);
    assert_eq!(r.frames, 12);
    assert!(r.min_psnr > 30.0, "PSNR {:.1}", r.min_psnr);
}

#[test]
fn cabac_is_smaller_than_cavlc() {
    let a = round_trip(cfg(176, 144, Profile::Baseline, RateMode::ConstQp(28)), 6);
    let b = round_trip(cfg(176, 144, Profile::Main, RateMode::ConstQp(28)), 6);
    assert!(b.bytes < a.bytes, "CABAC {} vs CAVLC {}", b.bytes, a.bytes);
}

#[test]
fn odd_sizes_are_cropped() {
    // 100x58: neither dimension is a multiple of 16.
    let r = round_trip(cfg(100, 58, Profile::Main, RateMode::ConstQp(24)), 4);
    assert_eq!(r.frames, 4);
    assert!(r.min_psnr > 30.0, "PSNR {:.1}", r.min_psnr);
}

#[test]
fn extreme_quantisers_round_trip() {
    for qp in [0u8, 10, 40, 51] {
        for profile in [Profile::Baseline, Profile::Main] {
            let r = round_trip(cfg(64, 48, profile, RateMode::ConstQp(qp)), 3);
            assert_eq!(r.frames, 3, "qp {qp}");
        }
    }
}

#[test]
fn rate_control_tracks_target() {
    for (mode, profile) in [(RateMode::Vbr, Profile::Main), (RateMode::Cbr, Profile::Baseline)] {
        let kbps = 600;
        let c = Config { bitrate_kbps: kbps, gop: 15, ..cfg(176, 144, profile, mode) };
        let n = 60;
        let r = round_trip(c, n);
        let actual = r.bytes as f64 * 8.0 / (n as f64 / 30.0) / 1000.0;
        assert!(actual > kbps as f64 * 0.6 && actual < kbps as f64 * 1.4, "{mode:?}: {actual:.0} kbit/s for {kbps}");
        assert!(r.min_psnr > 30.0, "{mode:?}: PSNR {:.1}", r.min_psnr);
    }
}

#[test]
fn sps_carries_bt709_vui() {
    let enc = Encoder::new(Config::new(1920, 1080, 30000, 1001)).unwrap();
    let mut dec = studio_h264::Decoder::from_avcc_bytes(&enc.avcc()).unwrap();
    let _ = dec.decode(&[]);
    let sps = enc.sps();
    assert_eq!(sps[1], 77); // Main
    assert_eq!(enc.level_idc(), 40);
    let mut e2 = Encoder::new(Config { max_slices: 4, ..Config::new(320, 240, 30000, 1001) }).unwrap();
    let f = e2.encode(&Yuv420::filled(320, 240, 120, 128, 128));
    let mut d2 = studio_h264::Decoder::from_avcc_bytes(&e2.avcc()).unwrap();
    let frames = d2.decode(&f.data).unwrap();
    let c = frames[0].color;
    assert_eq!((c.colour_primaries, c.transfer_characteristics, c.matrix_coefficients, c.full_range), (1, 1, 1, false));
    let vui = d2.active_sps().unwrap();
    let _ = vui;
}

/// Noisy, fast-moving content: the rate controller must still land near the target.
#[test]
fn rate_control_on_hard_content() {
    let (w, h) = (176u32, 144u32);
    for mode in [RateMode::Vbr, RateMode::Cbr] {
        let kbps = 1500;
        let mut enc = Encoder::new(Config { bitrate_kbps: kbps, gop: 30, ..cfg(w, h, Profile::Main, mode) }).unwrap();
        let mut seed = 7u32;
        let mut bytes = 0usize;
        let n = 90;
        let mut dec = studio_h264::Decoder::from_avcc_bytes(&enc.avcc()).unwrap();
        let mut decoded = 0;
        for t in 0..n {
            let mut pic = synth(w, h, t * 3);
            for p in pic.y.iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *p = (*p as i32 + ((seed >> 24) as i32 - 128) / 3).clamp(16, 235) as u8;
            }
            let f = enc.encode(&pic);
            bytes += f.data.len();
            if std::env::var("RC_DEBUG").is_ok() {
                eprintln!("{t}: qp {} bytes {} key {}", f.qp, f.data.len(), f.keyframe);
            }
            decoded += dec.decode(&f.data).unwrap().len();
        }
        assert_eq!(decoded, n as usize);
        let actual = bytes as f64 * 8.0 / (n as f64 / 30.0) / 1000.0;
        eprintln!("{mode:?} hard content: {actual:.0} kbit/s for {kbps}");
        assert!(actual > kbps as f64 * 0.8 && actual < kbps as f64 * 1.2, "{mode:?}: {actual:.0} kbit/s for {kbps}");
    }
}
