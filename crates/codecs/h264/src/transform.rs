// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scaling (dequantisation) and inverse transforms (8.5.6 .. 8.5.14).
//!
//! Coefficients are dequantised while parsing (see `dequant4` / `dequant8`) and
//! clamped to the 16-bit range a conforming bitstream never exceeds, which keeps
//! every intermediate value of the transforms well inside i32.

use crate::tables::{norm_adjust4, norm_adjust8};

/// Clamp a value to the range conforming bitstreams stay inside (8.5.12.1).
#[inline(always)]
pub fn clamp_coef(v: i64) -> i32 {
    v.clamp(-(1 << 15), (1 << 15) - 1) as i32
}

/// LevelScale4x4 for one scaling list: [qp % 6][raster pos].
pub type LevelScale4 = [[i32; 16]; 6];
/// LevelScale8x8 for one scaling list: [qp % 6][raster pos].
pub type LevelScale8 = [[i32; 64]; 6];

pub fn level_scale4(weights: &[u8; 16]) -> LevelScale4 {
    let mut ls = [[0; 16]; 6];
    for m in 0..6 {
        for p in 0..16 {
            ls[m][p] = weights[p] as i32 * norm_adjust4(m, p);
        }
    }
    ls
}

pub fn level_scale8(weights: &[u8; 64]) -> LevelScale8 {
    let mut ls = [[0; 64]; 6];
    for m in 0..6 {
        for p in 0..64 {
            ls[m][p] = weights[p] as i32 * norm_adjust8(m, p);
        }
    }
    ls
}

/// Scaling of a 4x4 AC/regular coefficient (8.5.12.1), qP = `qp`.
#[inline(always)]
pub fn dequant4(c: i32, ls: i32, qp: i32) -> i32 {
    let v = c as i64 * ls as i64;
    let r = if qp >= 24 { v << (qp / 6 - 4) } else { (v + (1 << (3 - qp / 6))) >> (4 - qp / 6) };
    clamp_coef(r)
}

/// Scaling of an 8x8 coefficient (8.5.13.1).
#[inline(always)]
pub fn dequant8(c: i32, ls: i32, qp: i32) -> i32 {
    let v = c as i64 * ls as i64;
    let r = if qp >= 36 { v << (qp / 6 - 6) } else { (v + (1 << (5 - qp / 6))) >> (6 - qp / 6) };
    clamp_coef(r)
}

/// Intra16x16 luma DC: inverse Hadamard and scaling (8.5.10).
/// `c` holds the DC levels in raster order of the 4x4 DC matrix; returns dcY in the same layout.
pub fn luma_dc_dequant(c: &[i32; 16], ls00: i32, qp: i32) -> [i32; 16] {
    let mut t = [0i64; 16];
    // rows
    for i in 0..4 {
        let (a, b, cc, d) = (c[i * 4] as i64, c[i * 4 + 1] as i64, c[i * 4 + 2] as i64, c[i * 4 + 3] as i64);
        t[i * 4] = a + b + cc + d;
        t[i * 4 + 1] = a + b - cc - d;
        t[i * 4 + 2] = a - b - cc + d;
        t[i * 4 + 3] = a - b + cc - d;
    }
    let mut f = [0i64; 16];
    for j in 0..4 {
        let (a, b, cc, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        f[j] = a + b + cc + d;
        f[4 + j] = a + b - cc - d;
        f[8 + j] = a - b - cc + d;
        f[12 + j] = a - b + cc - d;
    }
    let mut out = [0i32; 16];
    for k in 0..16 {
        let v = f[k] * ls00 as i64;
        let r = if qp >= 36 { v << (qp / 6 - 6) } else { (v + (1 << (5 - qp / 6))) >> (6 - qp / 6) };
        out[k] = clamp_coef(r);
    }
    out
}

/// 4:2:0 chroma DC: 2x2 transform and scaling (8.5.11). `c` = [c00, c01, c10, c11].
pub fn chroma_dc_dequant(c: &[i32; 4], ls00: i32, qp: i32) -> [i32; 4] {
    let (a, b, cc, d) = (c[0] as i64, c[1] as i64, c[2] as i64, c[3] as i64);
    let f = [a + b + cc + d, a - b + cc - d, a + b - cc - d, a - b - cc + d];
    let mut out = [0i32; 4];
    for k in 0..4 {
        out[k] = clamp_coef(((f[k] * ls00 as i64) << (qp / 6)) >> 5);
    }
    out
}

#[inline(always)]
fn clip_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// 4x4 inverse transform (8.5.12.2) and addition to the prediction in `dst`.
pub fn idct4_add(d: &[i32; 16], dst: &mut [u8], off: usize, stride: usize) {
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
            let p = off + i * stride + j;
            dst[p] = clip_u8(dst[p] as i32 + ((h[i] + 32) >> 6));
        }
    }
}

/// Add only a DC term (all other coefficients zero) — equivalent to `idct4_add`.
pub fn idct4_dc_add(dc: i32, dst: &mut [u8], off: usize, stride: usize) {
    let v = (dc + 32) >> 6;
    for i in 0..4 {
        let row = &mut dst[off + i * stride..off + i * stride + 4];
        for p in row {
            *p = clip_u8(*p as i32 + v);
        }
    }
}

#[inline(always)]
fn idct8_1d(d: [i32; 8]) -> [i32; 8] {
    let a0 = d[0] + d[4];
    let a4 = d[0] - d[4];
    let a2 = (d[2] >> 1) - d[6];
    let a6 = d[2] + (d[6] >> 1);
    let b0 = a0 + a6;
    let b2 = a4 + a2;
    let b4 = a4 - a2;
    let b6 = a0 - a6;
    let a1 = -d[3] + d[5] - d[7] - (d[7] >> 1);
    let a3 = d[1] + d[7] - d[3] - (d[3] >> 1);
    let a5 = -d[1] + d[7] + d[5] + (d[5] >> 1);
    let a7 = d[3] + d[5] + d[1] + (d[1] >> 1);
    let b1 = a1 + (a7 >> 2);
    let b7 = a7 - (a1 >> 2);
    let b3 = a3 + (a5 >> 2);
    let b5 = (a3 >> 2) - a5;
    [b0 + b7, b2 + b5, b4 + b3, b6 + b1, b6 - b1, b4 - b3, b2 - b5, b0 - b7]
}

/// 8x8 inverse transform (8.5.13.2) and addition to the prediction.
pub fn idct8_add(d: &[i32; 64], dst: &mut [u8], off: usize, stride: usize) {
    let mut t = [0i32; 64];
    for i in 0..8 {
        let mut row = [0i32; 8];
        row.copy_from_slice(&d[i * 8..i * 8 + 8]);
        let g = idct8_1d(row);
        t[i * 8..i * 8 + 8].copy_from_slice(&g);
    }
    for j in 0..8 {
        let mut col = [0i32; 8];
        for i in 0..8 {
            col[i] = t[i * 8 + j];
        }
        let m = idct8_1d(col);
        for i in 0..8 {
            let p = off + i * stride + j;
            dst[p] = clip_u8(dst[p] as i32 + ((m[i] + 32) >> 6));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idct4_dc_matches_full() {
        for dc in [-300, -64, -1, 0, 5, 31, 32, 100, 4000] {
            let mut d = [0i32; 16];
            d[0] = dc;
            let mut a = vec![100u8; 16];
            let mut b = vec![100u8; 16];
            idct4_add(&d, &mut a, 0, 4);
            idct4_dc_add(dc, &mut b, 0, 4);
            assert_eq!(a, b, "dc {dc}");
        }
    }

    #[test]
    fn idct4_known_vector() {
        // Single coefficient at (0,1): row transform gives [d, d/2... ] pattern.
        let mut d = [0i32; 16];
        d[1] = 64;
        let mut out = vec![128u8; 16];
        idct4_add(&d, &mut out, 0, 4);
        // e = [0, 0, 32, 64] -> f row = [64, 32, -32, -64]; columns: each column constant.
        // r = (f + 32) >> 6 = [1, 1, 0, -1]
        for i in 0..4 {
            assert_eq!(&out[i * 4..i * 4 + 4], &[129, 129, 128, 127]);
        }
    }

    #[test]
    fn idct8_dc_only() {
        let mut d = [0i32; 64];
        d[0] = 64 * 3;
        let mut out = vec![10u8; 64];
        idct8_add(&d, &mut out, 0, 8);
        assert!(out.iter().all(|&v| v == 13));
    }

    #[test]
    fn hadamard_roundtrip_scale() {
        // DC matrix with a single value: f = c00 everywhere.
        let mut c = [0i32; 16];
        c[0] = 2;
        // qp 36: dcY = (f * ls) << 0
        let out = luma_dc_dequant(&c, 16 * 10, 36);
        assert!(out.iter().all(|&v| v == 320));
        let out = chroma_dc_dequant(&[4, 0, 0, 0], 16 * 10, 6);
        // ((4*160) << 1) >> 5 = 40
        assert_eq!(out, [40; 4]);
    }

    #[test]
    fn dequant_rounding() {
        // qp < 24 rounding branch
        assert_eq!(dequant4(1, 160, 0), (160 + 8) >> 4);
        assert_eq!(dequant4(-1, 160, 12), (-160 + 2) >> 2);
        assert_eq!(dequant4(3, 160, 30), (3 * 160) << 1);
        assert_eq!(dequant8(1, 320, 6), (320 + 16) >> 5);
    }
}
