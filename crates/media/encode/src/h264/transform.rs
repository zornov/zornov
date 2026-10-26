// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Forward transforms and quantisation, and the decoder-exact scaling and
//! inverse transforms used for reconstruction (8.5).

use super::tables::{level_scale, pos_class, MF4};

/// Forward 4x4 core transform of a residual block (raster order).
pub fn fdct4(r: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let (a, b, c, d) = (r[i * 4], r[i * 4 + 1], r[i * 4 + 2], r[i * 4 + 3]);
        let s03 = a + d;
        let d03 = a - d;
        let s12 = b + c;
        let d12 = b - c;
        t[i * 4] = s03 + s12;
        t[i * 4 + 1] = 2 * d03 + d12;
        t[i * 4 + 2] = s03 - s12;
        t[i * 4 + 3] = d03 - 2 * d12;
    }
    let mut o = [0i32; 16];
    for j in 0..4 {
        let (a, b, c, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        let s03 = a + d;
        let d03 = a - d;
        let s12 = b + c;
        let d12 = b - c;
        o[j] = s03 + s12;
        o[4 + j] = 2 * d03 + d12;
        o[8 + j] = s03 - s12;
        o[12 + j] = d03 - 2 * d12;
    }
    o
}

/// 4x4 Hadamard (unnormalised), raster order.
pub fn hadamard4(c: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let (a, b, cc, d) = (c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]);
        t[i * 4] = a + b + cc + d;
        t[i * 4 + 1] = a + b - cc - d;
        t[i * 4 + 2] = a - b - cc + d;
        t[i * 4 + 3] = a - b + cc - d;
    }
    let mut f = [0i32; 16];
    for j in 0..4 {
        let (a, b, cc, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        f[j] = a + b + cc + d;
        f[4 + j] = a + b - cc - d;
        f[8 + j] = a - b - cc + d;
        f[12 + j] = a - b + cc - d;
    }
    f
}

/// Quantisation rounding: intra blocks use 1/3, inter blocks 1/6.
#[inline]
pub fn deadzone(intra: bool, qbits: u32) -> i64 {
    if intra { (1i64 << qbits) / 3 } else { (1i64 << qbits) / 6 }
}

/// Largest level magnitude the encoder emits (keeps CAVLC level_prefix <= 15
/// and CABAC values in range for 8-bit video).
pub const MAX_LEVEL: i32 = 2063;

#[inline]
fn quant1(w: i32, mf: i32, f: i64, qbits: u32) -> i32 {
    let a = ((w.unsigned_abs() as i64 * mf as i64 + f) >> qbits) as i32;
    let a = a.min(MAX_LEVEL);
    if w < 0 { -a } else { a }
}

/// Quantise a transformed 4x4 block (raster) into levels (raster). When
/// `skip_dc` the DC position is left at zero.
pub fn quant4(w: &[i32; 16], qp: i32, intra: bool, skip_dc: bool) -> [i32; 16] {
    let qbits = 15 + (qp / 6) as u32;
    let f = deadzone(intra, qbits);
    let m = (qp % 6) as usize;
    let mut out = [0i32; 16];
    for (p, o) in out.iter_mut().enumerate() {
        if skip_dc && p == 0 {
            continue;
        }
        *o = quant1(w[p], MF4[m][pos_class(p)], f, qbits);
    }
    out
}

/// Intra 16x16 luma DC: Hadamard of the 16 block DCs (raster of blocks), then
/// quantisation. Returns levels in raster order of the DC matrix.
pub fn quant_luma_dc(dc: &[i32; 16], qp: i32) -> [i32; 16] {
    let h = hadamard4(dc);
    let qbits = 16 + (qp / 6) as u32;
    let f = deadzone(true, qbits);
    let mf = MF4[(qp % 6) as usize][0];
    let mut out = [0i32; 16];
    for k in 0..16 {
        // (H W H) / 2, rounded towards zero, as in the reference encoder.
        out[k] = quant1(h[k] / 2, mf, f, qbits);
    }
    out
}

/// 4:2:0 chroma DC: 2x2 Hadamard then quantisation. `dc` = [c00, c01, c10, c11].
pub fn quant_chroma_dc(dc: &[i32; 4], qp: i32, intra: bool) -> [i32; 4] {
    let (a, b, c, d) = (dc[0], dc[1], dc[2], dc[3]);
    let f2 = [a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d];
    let qbits = 16 + (qp / 6) as u32;
    let f = deadzone(intra, qbits);
    let mf = MF4[(qp % 6) as usize][0];
    let mut out = [0i32; 4];
    for k in 0..4 {
        out[k] = quant1(f2[k], mf, f, qbits);
    }
    out
}

#[inline(always)]
fn clamp_coef(v: i64) -> i32 {
    v.clamp(-(1 << 15), (1 << 15) - 1) as i32
}

/// Decoder scaling of one 4x4 coefficient level (8.5.12.1).
#[inline]
pub fn dequant4(c: i32, pos: usize, qp: i32) -> i32 {
    let ls = level_scale((qp % 6) as usize, pos);
    let v = c as i64 * ls as i64;
    let r = if qp >= 24 { v << (qp / 6 - 4) } else { (v + (1 << (3 - qp / 6))) >> (4 - qp / 6) };
    clamp_coef(r)
}

/// Decoder scaling of a whole block (raster); DC kept as given when `dc` is Some.
pub fn dequant_block(levels: &[i32; 16], qp: i32, dc: Option<i32>) -> [i32; 16] {
    let mut d = [0i32; 16];
    for p in 0..16 {
        if levels[p] != 0 {
            d[p] = dequant4(levels[p], p, qp);
        }
    }
    if let Some(v) = dc {
        d[0] = v;
    }
    d
}

/// Intra 16x16 luma DC: inverse Hadamard and scaling (8.5.10), raster in, raster out.
pub fn dequant_luma_dc(c: &[i32; 16], qp: i32) -> [i32; 16] {
    let f = hadamard4(c);
    let ls00 = level_scale((qp % 6) as usize, 0) as i64;
    let mut out = [0i32; 16];
    for k in 0..16 {
        let v = f[k] as i64 * ls00;
        let r = if qp >= 36 { v << (qp / 6 - 6) } else { (v + (1 << (5 - qp / 6))) >> (6 - qp / 6) };
        out[k] = clamp_coef(r);
    }
    out
}

/// 4:2:0 chroma DC: inverse 2x2 transform and scaling (8.5.11).
pub fn dequant_chroma_dc(c: &[i32; 4], qp: i32) -> [i32; 4] {
    let (a, b, cc, d) = (c[0] as i64, c[1] as i64, c[2] as i64, c[3] as i64);
    let f = [a + b + cc + d, a - b + cc - d, a + b - cc - d, a - b - cc + d];
    let ls00 = level_scale((qp % 6) as usize, 0) as i64;
    let mut out = [0i32; 4];
    for k in 0..4 {
        out[k] = clamp_coef(((f[k] * ls00) << (qp / 6)) >> 5);
    }
    out
}

/// 4x4 inverse transform (8.5.12.2) added to the prediction `pred` (stride 16
/// at `po`), written to `dst` (at `off`, `stride`).
pub fn idct4_add(d: &[i32; 16], pred: &[u8], po: usize, dst: &mut [u8], off: usize, stride: usize) {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let r = &d[i * 4..i * 4 + 4];
        let e0 = r[0] + r[2];
        let e1 = r[0] - r[2];
        let e2 = (r[1] >> 1) - r[3];
        let e3 = r[1] + (r[3] >> 1);
        t[i * 4] = e0 + e3;
        t[i * 4 + 1] = e1 + e2;
        t[i * 4 + 2] = e1 - e2;
        t[i * 4 + 3] = e0 - e3;
    }
    for j in 0..4 {
        let (f0, f1, f2, f3) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        let g0 = f0 + f2;
        let g1 = f0 - f2;
        let g2 = (f1 >> 1) - f3;
        let g3 = f1 + (f3 >> 1);
        let h = [g0 + g3, g1 + g2, g1 - g2, g0 - g3];
        for i in 0..4 {
            let p = pred[po + i * 16 + j] as i32;
            dst[off + i * stride + j] = (p + ((h[i] + 32) >> 6)).clamp(0, 255) as u8;
        }
    }
}

/// Copy a prediction block (no residual) into `dst`.
pub fn copy4(pred: &[u8], po: usize, dst: &mut [u8], off: usize, stride: usize) {
    for i in 0..4 {
        dst[off + i * stride..off + i * stride + 4].copy_from_slice(&pred[po + i * 16..po + i * 16 + 4]);
    }
}

/// Sum of absolute transformed differences of a 4x4 block (Hadamard / 2).
#[inline]
pub fn satd4(diff: &[i32; 16]) -> u32 {
    let h = hadamard4(diff);
    (h.iter().map(|v| v.unsigned_abs()).sum::<u32>() + 1) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quant_dequant_round_trip_is_close() {
        let mut r = [0i32; 16];
        for (k, v) in r.iter_mut().enumerate() {
            *v = (k as i32 * 7) % 23 - 11;
        }
        for qp in [0, 10, 22, 28, 36, 51] {
            let w = fdct4(&r);
            let l = quant4(&w, qp, true, false);
            let d = dequant_block(&l, qp, None);
            let pred = [128u8; 64];
            let mut out = [0u8; 16];
            idct4_add(&d, &pred, 0, &mut out, 0, 4);
            let err: i32 = (0..16).map(|k| (out[k] as i32 - 128 - r[k]).abs()).max().unwrap();
            let bound = if qp <= 10 { 1 } else if qp <= 28 { 14 } else { 40 };
            assert!(err <= bound, "qp {qp}: err {err}");
        }
    }

    #[test]
    fn luma_dc_path_is_consistent() {
        // A flat residual of +20 in every block reconstructs close to +20.
        let r = [20i32; 16];
        let w = fdct4(&r);
        let dc = [w[0]; 16];
        for qp in [12, 26, 40] {
            let l = quant_luma_dc(&dc, qp);
            let d = dequant_luma_dc(&l, qp);
            let mut blk = [0i32; 16];
            blk[0] = d[5];
            let pred = [100u8; 64];
            let mut out = [0u8; 16];
            idct4_add(&blk, &pred, 0, &mut out, 0, 4);
            assert!((out[0] as i32 - 120).abs() <= 3, "qp {qp}: {}", out[0]);
        }
    }

    #[test]
    fn chroma_dc_path_is_consistent() {
        let r = [-12i32; 16];
        let w = fdct4(&r);
        for qp in [12, 26, 39] {
            let l = quant_chroma_dc(&[w[0]; 4], qp, true);
            let d = dequant_chroma_dc(&l, qp);
            let mut blk = [0i32; 16];
            blk[0] = d[3];
            let pred = [100u8; 64];
            let mut out = [0u8; 16];
            idct4_add(&blk, &pred, 0, &mut out, 0, 4);
            assert!((out[0] as i32 - 88).abs() <= 3, "qp {qp}: {}", out[0]);
        }
    }
}
