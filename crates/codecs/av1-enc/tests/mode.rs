// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M6: tests for the `mode` module.
//!
//! The gate is the full path: a decider's decisions are written with M4's `TileWriter`, wrapped in
//! a key frame with M1's headers, and decoded by `studio_av1::Decoder`. The decoded picture must
//! equal the decider's own reconstruction sample for sample. The search is then compared with the
//! fixed `DC_PRED` decider on bounded test images, in bytes and PSNR.

use studio_av1::Decoder;
use studio_av1::spec_tables::*;
use studio_av1_enc::bits::BitWriter;
use studio_av1_enc::mode::partition::fixed_partition;
use studio_av1_enc::mode::rdcost::{lambda, sad, satd};
use studio_av1_enc::mode::{FixedDecider, ModeDecider, SearchDecider};
use studio_av1_enc::obu::{ObuType, obu, temporal_delimiter, write_frame_header, write_sequence_header};
use studio_av1_enc::recon::{FrameBuf, QuantInfo};
use studio_av1_enc::syntax::TileWriter;
use studio_av1_enc::types::{Frame, FrameParams, FrameType, SbPos, SeqParams};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn seq(w: u32, h: u32) -> SeqParams {
    SeqParams {
        width: w,
        height: h,
        bit_depth: 8,
        fps: (30, 1),
        profile: 0,
        level_idx: 8,
        use_128x128_superblock: false,
        enable_cdef: false,
        enable_restoration: false,
        enable_order_hint: true,
        color_primaries: 1,
        transfer_characteristics: 1,
        matrix_coefficients: 1,
        full_range: false,
    }
}

fn key(q: u8) -> FrameParams {
    FrameParams {
        frame_type: FrameType::Key,
        show_frame: true,
        base_q_idx: q,
        disable_cdf_update: false,
        refresh_frame_flags: 0xFF,
        order_hint: 0,
        ref_frame_idx: [0; 7],
        loop_filter_level: [0, 0],
        cdef_enabled: false,
        cdef: Default::default(),
        lr: Default::default(),
        tile_cols_log2: 0,
        tile_rows_log2: 0,
    }
}

// ---- test images ----

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

fn gradient(w: usize, h: usize) -> Frame {
    image(w, h, |p, x, y| match p {
        0 => ((x * 255 / w.max(1) + y * 255 / h.max(1)) / 2) as u8,
        1 => (64 + x * 128 / w.max(1)) as u8,
        _ => (200 - y * 128 / h.max(1)) as u8,
    })
}

fn noise(w: usize, h: usize, seed: u64) -> Frame {
    let mut rng = Rng(seed);
    image(w, h, |_, _, _| (96 + rng.next() % 64) as u8)
}

/// Hard edges: stripes in several directions and a bright disc, like text and graphics.
fn edges(w: usize, h: usize) -> Frame {
    image(w, h, |p, x, y| {
        let (cx, cy) = (w as i32 / 2, h as i32 / 2);
        let (dx, dy) = (x as i32 - cx / (1 + (p > 0) as i32), y as i32 - cy / (1 + (p > 0) as i32));
        let disc = dx * dx + dy * dy < (h as i32 / 3).pow(2) / (1 + 3 * (p > 0) as i32);
        let stripes = if (x / 5 + y / 3) % 2 == 0 { 60 } else { 190 };
        let diag = if ((x + y) / 4) % 2 == 0 { 30 } else { 0 };
        if disc { 235 } else { stripes + diag }
    })
}

fn flat(w: usize, h: usize, v: u8) -> Frame {
    image(w, h, |_, _, _| v)
}

fn test_images() -> Vec<(&'static str, Frame)> {
    vec![
        ("gradient 64x64", gradient(64, 64)),
        ("gradient 130x70", gradient(130, 70)),
        ("noise 66x66", noise(66, 66, 5)),
        ("edges 64x64", edges(64, 64)),
        ("edges 200x120", edges(200, 120)),
        ("edges 66x66", edges(66, 66)),
        ("flat 100x60", flat(100, 60, 77)),
    ]
}

// ---- the path to the decoder ----

struct Coded {
    bytes: usize,
    recon: Frame,
    skip_blocks: usize,
    blocks: usize,
}

fn encode(src: &Frame, q: u8, decider: &mut dyn ModeDecider) -> (Coded, Vec<u8>) {
    let s = seq(src.width, src.height);
    let (mi_cols, mi_rows) = (2 * ((src.width + 7) >> 3), 2 * ((src.height + 7) >> 3));
    let mut recon = FrameBuf::new(src.width, src.height, 8);
    recon.quant = QuantInfo { base_q_idx: q, ..Default::default() };
    recon.enable_intra_edge_filter = false; // the sequence header M1 writes turns it off
    let mut tw = TileWriter::new(&s, q, mi_cols, mi_rows);
    let (mut skip_blocks, mut blocks) = (0, 0);
    for row in 0..mi_rows.div_ceil(16) {
        for col in 0..mi_cols.div_ceil(16) {
            let pos = SbPos { col, row };
            let sb = decider.decide_sb(src, &mut recon, pos);
            blocks += sb.blocks.len();
            skip_blocks += sb.blocks.iter().filter(|b| b.skip).count();
            tw.write_sb(pos, &sb);
        }
    }
    let tile = tw.finish();
    let mut w = BitWriter::new();
    write_frame_header(&s, &key(q), &mut w);
    let mut payload = w.finish();
    payload.extend_from_slice(&tile);
    let mut tu = temporal_delimiter();
    tu.extend(obu(ObuType::SequenceHeader, &write_sequence_header(&s)));
    tu.extend(obu(ObuType::Frame, &payload));
    (Coded { bytes: tile.len(), recon: recon.to_frame(), skip_blocks, blocks }, tu)
}

fn decode(tu: &[u8]) -> studio_av1::Picture {
    let mut d = Decoder::new();
    let mut pics = d.decode(tu).expect("decode");
    pics.extend(d.flush_result().expect("flush"));
    pics.pop().expect("a picture")
}

fn psnr(a: &[u16], b: &[u16]) -> f64 {
    let mse = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

struct Result {
    bytes: usize,
    psnr: f64,
    skip_blocks: usize,
    blocks: usize,
}

/// Encodes, decodes with the real decoder, checks bit-exactness against the reconstruction.
fn run(name: &str, src: &Frame, q: u8, search: bool) -> Result {
    let mut fixed = FixedDecider { base_q_idx: q };
    let mut srch = SearchDecider { base_q_idx: q };
    let decider: &mut dyn ModeDecider = if search { &mut srch } else { &mut fixed };
    let (coded, tu) = encode(src, q, decider);
    // bounded: a frame never costs more than ~3 bytes a sample at these quantisers
    assert!(coded.bytes < 3 * (src.width * src.height) as usize + 1024, "{name} q{q}: {} bytes", coded.bytes);
    let pic = decode(&tu);
    for p in 0..3 {
        assert!(pic.planes[p] == coded.recon.planes[p], "{name} q{q} search {search}: decoded plane {p} differs from the reconstruction");
    }
    Result { bytes: coded.bytes, psnr: psnr(&pic.planes[0], &src.planes[0]), skip_blocks: coded.skip_blocks, blocks: coded.blocks }
}

#[test]
fn fixed_decider_decodes_bit_exactly() {
    for (name, img) in test_images() {
        for q in [20u8, 100, 200] {
            run(name, &img, q, false);
        }
    }
}

#[test]
fn search_decider_decodes_bit_exactly() {
    for (name, img) in test_images() {
        for q in [20u8, 100, 200] {
            run(name, &img, q, true);
        }
    }
}

/// `(bytes, luma PSNR)` of the fixed decider, from the encoder's own reconstruction (the decoder
/// is checked to agree in the bit-exactness tests).
fn dc_point(img: &Frame, q: u8) -> (f64, f64) {
    let (coded, _) = encode(img, q, &mut FixedDecider { base_q_idx: q });
    (coded.bytes as f64, psnr(&coded.recon.planes[0], &img.planes[0]))
}

/// The best PSNR the DC-only decider reaches at `bytes`, by log-linear interpolation between its
/// operating points over a range of quantisers; `None` when `bytes` is outside that range.
fn dc_psnr_at(curve: &[(f64, f64)], bytes: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for a in curve {
        for b in curve {
            if a.0 < bytes && bytes <= b.0 {
                let t = (bytes.ln() - a.0.ln()) / (b.0.ln() - a.0.ln());
                let v = a.1 + t * (b.1 - a.1);
                best = Some(best.map_or(v, |x: f64| x.max(v)));
            }
        }
    }
    best
}

#[test]
fn search_never_does_worse_than_dc_only_for_the_bits_spent() {
    // The search trades bytes for quality on its own, so the comparison is on the rate-distortion
    // curve: at the bytes the search spends, the DC-only decider (at whatever quantiser gives
    // that many bytes) must not reach a higher PSNR.
    let (mut gain_sum, mut n, mut tot_f, mut tot_s) = (0.0, 0, 0usize, 0usize);
    for (name, img) in test_images() {
        let curve: Vec<(f64, f64)> = (4..=252).step_by(12).map(|q| dc_point(&img, q as u8)).collect();
        for q in [20u8, 100, 200] {
            let f = run(name, &img, q, false);
            let s = run(name, &img, q, true);
            tot_f += f.bytes;
            tot_s += s.bytes;
            let reference = dc_psnr_at(&curve, s.bytes as f64);
            eprintln!("{name:16} q{q:3}: dc-only {:6} B {:5.2} dB | search {:6} B {:5.2} dB | dc curve at {} B: {}", f.bytes, f.psnr, s.bytes, s.psnr, s.bytes, reference.map_or("-".into(), |v| format!("{v:.2} dB")));
            // at the same quantiser the search must not blow up the size
            assert!(s.bytes as f64 <= f.bytes as f64 * 1.05 + 32.0, "{name} q{q}: search spends {} B against {} B", s.bytes, f.bytes);
            if s.bytes >= 100 {
                if let Some(r) = reference {
                    // 0.4 dB: on noise the curve is flat and its interpolation moves by a few tenths of a dB
                    assert!(s.psnr >= r - 0.4, "{name} q{q}: search {:.2} dB at {} B is below the DC-only curve ({r:.2} dB)", s.psnr, s.bytes);
                    gain_sum += s.psnr - r;
                    n += 1;
                }
            }
        }
    }
    let gain = gain_sum / n.max(1) as f64;
    eprintln!("total: dc-only {tot_f} B, search {tot_s} B; mean PSNR gain over the DC-only curve {gain:.2} dB in {n} cases");
    assert!(n >= 8, "too few comparable cases ({n})");
    assert!(tot_s < tot_f, "the search should save bytes over all images ({tot_s} against {tot_f})");
    assert!(gain > 0.0, "the search should beat the DC-only curve on average (mean gain {gain:.2} dB)");
}

#[test]
fn flat_image_codes_as_skip_blocks() {
    // 128 is what DC_PRED predicts at the frame corner and what every later block copies
    let r = run("flat 128", &flat(100, 60, 128), 100, false);
    assert_eq!(r.skip_blocks, r.blocks);
    assert!(r.bytes < 64, "{} bytes for a flat picture", r.bytes);
}

#[test]
fn fixed_partition_covers_the_frame_with_valid_blocks() {
    for &(w, h) in &[(64usize, 64usize), (66, 66), (130, 70), (200, 120), (8, 8), (1, 1), (9, 33)] {
        let (mi_cols, mi_rows) = (2 * ((w + 7) >> 3), 2 * ((h + 7) >> 3));
        let mut covered = vec![0u8; mi_rows * mi_cols];
        for sr in 0..mi_rows.div_ceil(16) {
            for sc in 0..mi_cols.div_ceil(16) {
                let mut v = Vec::new();
                fixed_partition(sr * 16, sc * 16, 16, mi_rows, mi_cols, &mut v);
                for (r, c, w4, h4) in v {
                    assert!((0..BLOCK_SIZES).any(|b| NUM_4X4_BLOCKS_WIDE[b] as usize == w4 && NUM_4X4_BLOCKS_HIGH[b] as usize == h4), "{w4}x{h4} is not a block size");
                    for y in r..(r + h4).min(mi_rows) {
                        for x in c..(c + w4).min(mi_cols) {
                            covered[y * mi_cols + x] += 1;
                        }
                    }
                }
            }
        }
        assert!(covered.iter().all(|&n| n == 1), "{w}x{h}: every 4x4 unit must be covered exactly once");
    }
}

#[test]
fn cost_helpers() {
    assert_eq!(sad(&[1, -2, 3, 0]), 6);
    assert_eq!(satd(&[0; 64], 8, 8), 0);
    // a constant residual is one DC coefficient per 4x4 tile: 16 * v * (tiles) / 4 in total
    assert_eq!(satd(&[5; 64], 8, 8), 4 * 16 * 5 / 4);
    assert!(lambda(200, 8) > lambda(20, 8));
    // a pattern that the transform compacts costs less than noise of the same energy
    let smooth: Vec<i16> = (0..64).map(|i| (i % 8) as i16).collect();
    let rough: Vec<i16> = (0..64).map(|i| if i % 2 == 0 { 3 } else { -4 }).collect();
    assert!(satd(&smooth, 8, 8) < satd(&rough, 8, 8) + 200);
}

#[test]
#[should_panic(expected = "lossless")]
fn lossless_is_rejected() {
    let src = flat(16, 16, 10);
    let mut recon = FrameBuf::new(16, 16, 8);
    recon.quant = QuantInfo { base_q_idx: 0, ..Default::default() };
    FixedDecider { base_q_idx: 0 }.decide_sb(&src, &mut recon, SbPos { col: 0, row: 0 });
}
