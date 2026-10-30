// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 10-bit gate: 10-bit streams (key, P, intra-in-P, partition search at speeds 6 and 8, filters
//! on) decode with `studio-av1` to the encoder's reconstruction bit for bit; the hardware decoder is
//! asked about them; 10-bit coding needs no more bytes than 8-bit coding of the same source at equal
//! PSNR; and 8-bit streams are unchanged byte for byte.

use studio_av1::Decoder;
use studio_av1_enc::{Config, Encoder, Frame, RateMode};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn image10(w: usize, h: usize, mut f: impl FnMut(usize, usize, usize) -> i32) -> Frame {
    let mut fr = Frame::new(w as u32, h as u32, 10);
    for p in 0..3 {
        let (pw, ph) = if p == 0 { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
        for y in 0..ph {
            for x in 0..pw {
                fr.planes[p][y * fr.stride[p] + x] = f(p, x, y).clamp(0, 1023) as u16;
            }
        }
    }
    fr
}

/// A smooth textured canvas in 10-bit limited-range luma units, sampled so that pans can be
/// fractional.
fn canvas(x: f64, y: f64) -> f64 {
    360.0 + 180.0 * (x * 0.045).sin() * (y * 0.035).cos() + 120.0 * ((x + 2.0 * y) * 0.021).sin() + 12.0 * (x * 0.17).sin() * (y * 0.13).sin()
}

/// A 10-bit pan, with a bright square crossing it (frame `n / 2` jumps to unrelated content, to
/// force an intra frame in the middle of the sequence when `cut` is set).
fn sequence10(w: usize, h: usize, n: usize, cut: bool, noise: u32, seed: u64) -> Vec<Frame> {
    let mut rng = Rng(seed);
    (0..n)
        .map(|i| {
            let (dx, dy) = if cut && i >= n / 2 { (400.0 + 3.0 * i as f64, 90.0) } else { (3.0 * i as f64, 2.0 * i as f64) };
            image10(w, h, |p, x, y| {
                let k = if p == 0 { 1.0 } else { 2.0 };
                let (fx, fy) = (x as f64 * k, y as f64 * k);
                let (ox, oy) = (20.0 + 5.0 * i as f64, 30.0 + 2.0 * i as f64);
                let v = if fx >= ox && fx < ox + 24.0 && fy >= oy && fy < oy + 24.0 { 880.0 } else { canvas(fx + dx, fy + dy) };
                let v = if p == 0 { v } else { 512.0 + (v - 360.0) * 0.3 };
                let n = if noise > 0 { (rng.next() % (2 * noise + 1)) as i32 - noise as i32 } else { 0 };
                v.round() as i32 + n
            })
        })
        .collect()
}

/// The 8-bit rendition of a 10-bit frame (round to nearest).
fn to8(f: &Frame) -> Frame {
    let mut o = f.clone();
    o.bit_depth = 8;
    for p in 0..3 {
        for s in o.planes[p].iter_mut() {
            *s = ((*s as u32 + 2) >> 2).min(255) as u16;
        }
    }
    o
}

/// PSNR of luma `a` against `b` for a 10-bit peak; `shift` scales `a` up first (8-bit to 10-bit).
fn psnr10(a: &[u16], b: &[u16], shift: u32) -> f64 {
    let mse = a.iter().zip(b).map(|(&x, &y)| ((x as f64) * (1u32 << shift) as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (1023.0f64 * 1023.0 / mse).log10() }
}

struct Coded {
    bytes: usize,
    psnr: f64,
    keys: usize,
}

/// Codes `frames`, checks every decoded picture against the reconstruction, returns bytes and
/// the mean luma PSNR against the 10-bit `reference` frames (the codec's own frames scaled up by
/// `shift` when they are 8-bit).
fn code(frames: &[Frame], reference: &[Frame], q: u8, speed: u8, keyint: u32, shift: u32) -> Coded {
    let (w, h) = (frames[0].width, frames[0].height);
    let bd = frames[0].bit_depth;
    let mut enc = Encoder::new(Config { rate: RateMode::Cqp(q), speed, keyint, bit_depth: bd, ..Config::new(w, h) }).unwrap();
    let mut dec = Decoder::new();
    let (mut bytes, mut sum, mut keys) = (0, 0.0, 0);
    for (i, f) in frames.iter().enumerate() {
        let p = enc.push_frame(f, i as i64).unwrap().remove(0);
        bytes += p.data.len();
        keys += p.keyframe as usize;
        let mut pics = dec.decode(&p.data).unwrap_or_else(|e| panic!("{bd}-bit frame {i}: {e:?}"));
        pics.extend(dec.flush_result().unwrap());
        assert_eq!(pics.len(), 1);
        let want = enc.reconstruction().unwrap();
        assert_eq!(want.bit_depth, bd);
        for plane in 0..3 {
            assert!(pics[0].planes[plane] == want.planes[plane], "{bd}-bit q{q} speed {speed} frame {i} (key {}) plane {plane} differs from the reconstruction", p.keyframe);
        }
        let max = (1u16 << bd) - 1;
        assert!(want.planes.iter().all(|pl| pl.iter().all(|&s| s <= max)), "sample above the {bd}-bit maximum");
        sum += psnr10(&want.planes[0], &reference[i].planes[0], shift);
    }
    Coded { bytes, psnr: sum / frames.len() as f64, keys }
}

#[test]
fn ten_bit_streams_decode_bit_exactly_at_every_frame_type_and_speed() {
    let mut cases = 0;
    for &(w, h) in &[(64usize, 64usize), (130, 70), (160, 96)] {
        for cut in [false, true] {
            let frames = sequence10(w, h, 6, cut, 3, 7);
            for speed in [6u8, 8] {
                for q in [40u8, 120, 220] {
                    let c = code(&frames, &frames, q, speed, 240, 0);
                    eprintln!("10-bit {w}x{h} cut={cut} speed {speed} q{q:3}: {:6} B {:5.2} dB keys {}", c.bytes, c.psnr, c.keys);
                    assert!(c.keys >= 1 + cut as usize || cut, "key frames: {}", c.keys);
                    assert!(c.psnr > 30.0 || q > 200, "10-bit PSNR {:.2} at q{q}", c.psnr);
                    cases += 1;
                }
            }
        }
    }
    eprintln!("{cases} 10-bit encodes, all bit-exact");
}

/// The cut forces an intra frame in a P sequence; with keyint 1 every frame is a key frame.
#[test]
fn ten_bit_intra_in_p_and_all_intra_decode_bit_exactly() {
    let frames = sequence10(96, 64, 6, true, 2, 11);
    let all_intra = code(&frames, &frames, 100, 6, 1, 0);
    assert_eq!(all_intra.keys, frames.len());
    let mixed = code(&frames, &frames, 100, 6, 240, 0);
    assert!(mixed.keys >= 2, "the scene cut should force a second key frame, got {}", mixed.keys);
    assert!(mixed.bytes < all_intra.bytes, "P frames should save bytes: {} vs {}", mixed.bytes, all_intra.bytes);
}

/// The workspace's hardware decoder wrapper only returns 8-bit pictures. Report what it says about
/// a 10-bit stream; do not pretend to a decode.
#[test]
fn hardware_decoder_verdict_on_ten_bit_is_recorded() {
    if !studio_hw::Av1Decoder::supported() {
        eprintln!("SKIPPED: this machine's video hardware does not decode AV1");
        return;
    }
    let frames = sequence10(64, 64, 1, false, 0, 1);
    let enc = Encoder::new(Config { bit_depth: 10, ..Config::new(64, 64) }).unwrap();
    let _ = &frames;
    match studio_hw::Av1Decoder::new(&enc.av1c(), 64, 64) {
        Ok(_) => eprintln!("VideoToolbox: a 10-bit decoder was created"),
        Err(e) => eprintln!("VideoToolbox/studio_hw refuses the 10-bit stream: {e}"),
    }
}

/// On a smooth source, 10-bit coding needs no more bytes than 8-bit coding at equal PSNR against
/// the 10-bit source (the 8-bit reconstruction scaled by 4).
#[test]
fn ten_bit_needs_no_more_bytes_than_eight_bit_at_equal_psnr() {
    let f10 = sequence10(160, 96, 4, false, 1, 21);
    let f8: Vec<Frame> = f10.iter().map(to8).collect();
    let qs = [30u8, 60, 90, 120, 150, 180];
    let mut c10: Vec<(f64, f64)> = qs.iter().map(|&q| { let c = code(&f10, &f10, q, 6, 240, 0); (c.psnr, c.bytes as f64) }).collect();
    let mut c8: Vec<(f64, f64)> = qs.iter().map(|&q| { let c = code(&f8, &f10, q, 6, 240, 2); (c.psnr, c.bytes as f64) }).collect();
    c10.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    c8.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    eprintln!("10-bit curve (dB, B): {c10:?}\n8-bit curve (dB, B): {c8:?}");
    let at = |c: &[(f64, f64)], p: f64| c.windows(2).find_map(|w| (w[0].0..=w[1].0).contains(&p).then(|| (w[0].1.ln() + (w[1].1.ln() - w[0].1.ln()) * (p - w[0].0) / (w[1].0 - w[0].0)).exp()));
    let mut ratios = Vec::new();
    for &(p, b8) in &c8 {
        if let Some(b10) = at(&c10, p) {
            eprintln!("{p:5.2} dB: 8-bit {b8:7.0} B, 10-bit {b10:7.0} B, ratio {:.3}", b10 / b8);
            ratios.push(b10 / b8);
        }
    }
    assert!(ratios.len() >= 3, "too few comparable operating points ({})", ratios.len());
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    eprintln!("10-bit needs {:.1}% of the 8-bit bytes at equal PSNR over {} points", 100.0 * mean, ratios.len());
    assert!(mean <= 1.0, "10-bit costs {:.1}% of the 8-bit bytes", 100.0 * mean);
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// Hashes of 8-bit streams recorded before the 10-bit work; they must not move.
#[test]
fn eight_bit_streams_are_byte_identical_to_before() {
    let mut out = Vec::new();
    for (i, &(w, h, q, speed)) in [(64usize, 64usize, 60u8, 6u8), (130, 70, 120, 8), (160, 96, 200, 6)].iter().enumerate() {
        let frames: Vec<Frame> = sequence10(w, h, 4, i == 2, 2, 5 + i as u64).iter().map(to8).collect();
        let mut enc = Encoder::new(Config { rate: RateMode::Cqp(q), speed, ..Config::new(w as u32, h as u32) }).unwrap();
        let mut hash = 0xcbf29ce484222325u64;
        for (n, f) in frames.iter().enumerate() {
            for p in enc.push_frame(f, n as i64).unwrap() {
                hash = fnv(&p.data, hash);
            }
        }
        out.push(hash);
    }
    eprintln!("8-bit stream hashes: {out:#018x?}");
    assert_eq!(out, BASELINE_8BIT, "8-bit output changed");
}

const BASELINE_8BIT: [u64; 3] = [0x9142acfe3a118cb2, 0x029a9627b73857dd, 0x172d1c0c06e55976];
