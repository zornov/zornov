// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M3: tests for the `transform` module.

use studio_av1::spec_tables::*;
use studio_av1::transform::inverse_transform_2d;
use studio_av1_enc::transform::quant::*;
use studio_av1_enc::transform::*;
use studio_av1_enc::types::QuantParams;

const QP: fn(u8) -> QuantParams = |q| QuantParams { base_q_idx: q, bit_depth: 8, plane: 0 };

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u32) as i32
    }
}

fn dims(sz: usize) -> (usize, usize) {
    (1 << TX_WIDTH_LOG2[sz], 1 << TX_HEIGHT_LOG2[sz])
}

/// Smooth-ish residual with some noise, like prediction error.
fn residual(rng: &mut Rng, w: usize, h: usize, amp: i32) -> Vec<i16> {
    // 64-point sides keep only 32 coefficients, so only smooth content survives them.
    let noise = if w == 64 || h == 64 { 0 } else { amp / 8 };
    let (gx, gy) = (rng.range(-amp, amp) as f64 / w as f64, rng.range(-amp, amp) as f64 / h as f64);
    (0..w * h).map(|p| ((p % w) as f64 * gx + (p / w) as f64 * gy + rng.range(-noise, noise) as f64) as i16).collect()
}

#[test]
fn forward_inverse_without_quantisation_is_near_exact() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut worst = 0;
    for sz in 0..19 {
        for ty in 0..16 {
            if !is_supported(sz, ty) {
                continue;
            }
            let (w, h) = dims(sz);
            let (ud, lr) = flips(ty);
            for _ in 0..4 {
                let r = residual(&mut rng, w, h, 100);
                let mut coef = vec![0i32; 4096];
                forward_transform(&r, sz, ty, &mut coef);
                let mut res = vec![0i32; w * h];
                inverse_transform_2d(&coef, &mut res, sz, ty, false, 8);
                // The decoder flips its output when adding it to the prediction.
                let mut err = 0;
                for i in 0..h {
                    for j in 0..w {
                        let (yy, xx) = (if ud { h - 1 - i } else { i }, if lr { w - 1 - j } else { j });
                        err = err.max((res[yy * w + xx] - r[i * w + j] as i32).abs());
                    }
                }
                worst = worst.max(err);
                assert!(err <= 3, "size {sz} type {ty}: max error {err}");
            }
        }
    }
    println!("unquantised round trip worst error {worst}");
}

/// forward -> quantise -> dequantise -> decoder inverse, error against the source residual.
fn quantised_round_trip(rng: &mut Rng, sz: usize, ty: usize, q: u8) -> (f64, i32) {
    let (w, h) = dims(sz);
    let (ud, lr) = flips(ty);
    let r = residual(rng, w, h, 100);
    let mut coef = vec![0i32; 4096];
    forward_transform(&r, sz, ty, &mut coef);
    let blk = quantize_block(&coef, sz, ty, 0, QP(q));
    let mut dq = vec![0i32; 4096];
    dequantize_block(&blk.levels, sz, QP(q), &mut dq);
    let mut res = vec![0i32; w * h];
    inverse_transform_2d(&dq, &mut res, sz, ty, false, 8);
    let (mut se, mut worst) = (0f64, 0);
    for i in 0..h {
        for j in 0..w {
            let (yy, xx) = (if ud { h - 1 - i } else { i }, if lr { w - 1 - j } else { j });
            let e = res[yy * w + xx] - r[i * w + j] as i32;
            se += (e * e) as f64;
            worst = worst.max(e.abs());
        }
    }
    ((se / (w * h) as f64).sqrt(), worst)
}

#[test]
fn quantised_round_trip_error_is_bounded_by_the_step() {
    let mut rng = Rng(0xfeed_beef_0bad_cafe);
    for q in [8u8, 40, 100, 160, 220] {
        let (_, ac) = steps(QP(q));
        let (mut sum_rms, mut count, mut worst) = (0f64, 0, 0);
        for sz in 0..19 {
            let (w, h) = dims(sz);
            let denom = match sz {
                TX_32X32 | TX_16X32 | TX_32X16 | TX_16X64 | TX_64X16 => 2.0,
                TX_64X64 | TX_32X64 | TX_64X32 => 4.0,
                _ => 1.0,
            };
            // coefficient = gain * orthonormal coefficient; see the module docs.
            let rect = if (TX_WIDTH_LOG2[sz] as i32 - TX_HEIGHT_LOG2[sz] as i32).abs() == 1 { 2f64.sqrt() } else { 1.0 };
            let gain = (1u32 << (TRANSFORM_ROW_SHIFT[sz] + 4)) as f64 * rect * 2.0 / ((w * h) as f64).sqrt();
            let step = ac as f64 / denom / gain;
            for ty in 0..16 {
                if !is_supported(sz, ty) {
                    continue;
                }
                let (rms, mx) = quantised_round_trip(&mut rng, sz, ty, q);
                // Per-coefficient error is at most 9/16 of a step, so the RMS cannot exceed it
                // (plus rounding of the inverse).
                assert!(rms <= 0.5625 * step + 1.0, "size {sz} type {ty} q {q}: rms {rms:.2} step {step:.2}");
                sum_rms += rms;
                count += 1;
                worst = worst.max(mx);
            }
        }
        println!("q_idx {q:3} (ac step {ac:4}): mean rms error {:.3}, worst sample error {worst}, {count} size/type pairs", sum_rms / count as f64);
    }
}

#[test]
fn lossless_wht_is_exact() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let qp = QP(0);
    assert!(is_lossless(qp));
    assert_eq!(steps(qp), (4, 4));
    for _ in 0..20000 {
        let amp = [1, 15, 255][rng.range(0, 2) as usize];
        let r: Vec<i16> = (0..16).map(|_| rng.range(-amp, amp) as i16).collect();
        let mut coef = vec![0i32; 4096];
        forward_wht(&r, &mut coef);
        let blk = quantize_block(&coef, TX_4X4, DCT_DCT, 0, qp);
        let mut dq = vec![0i32; 4096];
        dequantize_block(&blk.levels, TX_4X4, qp, &mut dq);
        let mut res = vec![0i32; 16];
        inverse_transform_2d(&dq, &mut res, TX_4X4, DCT_DCT, true, 8);
        let want: Vec<i32> = r.iter().map(|&v| v as i32).collect();
        assert_eq!(res, want, "residual {r:?}");
    }
}

#[test]
fn eob_follows_the_scan_and_matches_levels() {
    let mut rng = Rng(77);
    for sz in 0..19 {
        for ty in 0..16 {
            if !is_supported(sz, ty) {
                continue;
            }
            let (w, h) = dims(sz);
            let r = residual(&mut rng, w, h, 60);
            let mut coef = vec![0i32; 4096];
            forward_transform(&r, sz, ty, &mut coef);
            let blk = quantize_block(&coef, sz, ty, 1, QP(60));
            let (tw, th) = coded_dims(sz);
            let scan = scan_for(sz, ty);
            assert_eq!(scan.len(), tw * th, "size {sz} type {ty}");
            assert_eq!(blk.levels.len(), tw * th);
            assert!(blk.eob >= 1 && blk.eob <= scan.len());
            assert_ne!(blk.levels[scan[blk.eob - 1] as usize], 0);
            assert!(scan[blk.eob..].iter().all(|&p| blk.levels[p as usize] == 0));
            assert_eq!((blk.plane, blk.tx_size, blk.tx_type), (1, sz, ty));
        }
    }
}

#[test]
fn zero_residual_gives_empty_block_and_flat_gives_dc_only() {
    for sz in 0..19 {
        let (w, h) = dims(sz);
        let mut coef = vec![7i32; 4096];
        forward_transform(&vec![0i16; w * h], sz, DCT_DCT, &mut coef);
        assert_eq!(quantize_block(&coef, sz, DCT_DCT, 0, QP(50)).eob, 0);
        forward_transform(&vec![20i16; w * h], sz, DCT_DCT, &mut coef);
        let blk = quantize_block(&coef, sz, DCT_DCT, 0, QP(50));
        assert_eq!(blk.eob, 1, "size {sz}");
        assert!(blk.levels[0] > 0 && blk.levels[1..].iter().all(|&l| l == 0));
    }
}

#[test]
fn dequantize_matches_the_decoder_formula() {
    let qp = QP(100);
    let (dc, ac) = steps(qp);
    assert_eq!(dc, DC_QLOOKUP[0][100] as i64);
    assert_eq!(ac, AC_QLOOKUP[0][100] as i64);
    assert_eq!(dequantize(3, 0, qp), 3 * dc as i32);
    assert_eq!(dequantize(-3, 5, qp), -3 * ac as i32);
    assert_eq!(dequantize_sized(5, 1, TX_32X32, qp), (5 * ac / 2) as i32);
    assert_eq!(dequantize_sized(-5, 1, TX_64X64, qp), -((5 * ac / 4) as i32));
    // 10-bit uses the second table and a wider clamp.
    let q10 = QuantParams { base_q_idx: 100, bit_depth: 10, plane: 0 };
    assert_eq!(steps(q10).1, AC_QLOOKUP[1][100] as i64);
    assert_eq!(dequantize(1 << 19, 1, QP(255)), (1 << 15) - 1);
}

#[test]
fn ten_bit_round_trip() {
    let mut rng = Rng(5);
    let qp = QuantParams { base_q_idx: 60, bit_depth: 10, plane: 0 };
    for (sz, ty) in [(TX_8X8, DCT_DCT), (TX_16X16, ADST_ADST), (TX_32X32, DCT_DCT), (TX_4X4, IDTX)] {
        let (w, h) = dims(sz);
        let r: Vec<i16> = residual(&mut rng, w, h, 400);
        let mut coef = vec![0i32; 4096];
        forward_transform(&r, sz, ty, &mut coef);
        let blk = quantize_block(&coef, sz, ty, 0, qp);
        let mut dq = vec![0i32; 4096];
        dequantize_block(&blk.levels, sz, qp, &mut dq);
        let mut res = vec![0i32; w * h];
        inverse_transform_2d(&dq, &mut res, sz, ty, false, 10);
        let worst = res.iter().zip(&r).map(|(&a, &b)| (a - b as i32).abs()).max().unwrap();
        assert!(worst <= 3 * steps(qp).1 as i32 / 8 + 4, "size {sz} type {ty}: {worst}");
    }
}
