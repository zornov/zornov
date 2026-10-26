// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scaling (dequantisation) and inverse transforms (8.6.2 - 8.6.4).

/// Magnitudes of the HEVC transform coefficients indexed by the angle index
/// m of cos(pi * m / 64) (m = 0 maps to the DC basis value 64).
const COS_TAB: [i32; 33] = [
    64, 90, 90, 90, 89, 88, 87, 85, 83, 82, 80, 78, 75, 73, 70, 67, 64, 61, 57, 54, 50, 46, 43, 38,
    36, 31, 25, 22, 18, 13, 9, 4, 0,
];

const fn build_mat32() -> [[i32; 32]; 32] {
    let mut m = [[0i32; 32]; 32];
    let mut k = 0;
    while k < 32 {
        let mut n = 0;
        while n < 32 {
            let mut a = ((2 * n + 1) * k) % 128;
            let mut sign = 1;
            if a > 64 {
                a = 128 - a;
            }
            if a > 32 {
                a = 64 - a;
                sign = -1;
            }
            m[k][n] = sign * COS_TAB[a];
            n += 1;
        }
        k += 1;
    }
    m
}

/// The 32x32 DCT matrix transMatrix (8-xxx); rows are basis functions.
pub static MAT32: [[i32; 32]; 32] = build_mat32();

/// DST-VII 4x4 matrix for intra luma 4x4 blocks.
pub const DST4: [[i32; 4]; 4] = [[29, 55, 74, 84], [74, 74, 0, -74], [84, -29, -74, 55], [55, -84, 74, -29]];

const LEVEL_SCALE: [i64; 6] = [40, 45, 51, 57, 64, 72];

/// Scaling process for transform coefficients (8.6.4.2 / 8.6.3).
/// `scale` is the ScalingFactor array (raster, y * n + x) or None for flat 16.
#[cfg(test)]
pub fn dequantize(coeffs: &mut [i32], log2: u32, qp: i32, bit_depth: u32, scale: Option<&[u8]>) {
    let n = 1usize << log2;
    let bd_shift = bit_depth as i64 + log2 as i64 - 5;
    let qp = qp.max(0);
    let ls = LEVEL_SCALE[(qp % 6) as usize] << (qp / 6);
    let add = 1i64 << (bd_shift - 1);
    match scale {
        None => {
            let f = 16 * ls;
            for c in coeffs[..n * n].iter_mut() {
                if *c != 0 {
                    let v = (*c as i64 * f + add) >> bd_shift;
                    *c = v.clamp(-32768, 32767) as i32;
                }
            }
        }
        Some(m) => {
            for (i, c) in coeffs[..n * n].iter_mut().enumerate() {
                if *c != 0 {
                    let v = (*c as i64 * m[i] as i64 * ls + add) >> bd_shift;
                    *c = v.clamp(-32768, 32767) as i32;
                }
            }
        }
    }
}

/// Scaling of the coefficients at the given raster positions only.
pub fn dequantize_positions(coeffs: &mut [i32], pos: &[u16], log2: u32, qp: i32, bit_depth: u32, scale: Option<&[u8]>) {
    let bd_shift = bit_depth as i64 + log2 as i64 - 5;
    let qp = qp.max(0);
    let ls = LEVEL_SCALE[(qp % 6) as usize] << (qp / 6);
    let add = 1i64 << (bd_shift - 1);
    match scale {
        None => {
            let f = 16 * ls;
            for &p in pos {
                let c = &mut coeffs[p as usize];
                *c = ((*c as i64 * f + add) >> bd_shift).clamp(-32768, 32767) as i32;
            }
        }
        Some(m) => {
            for &p in pos {
                let c = &mut coeffs[p as usize];
                *c = ((*c as i64 * m[p as usize] as i64 * ls + add) >> bd_shift).clamp(-32768, 32767) as i32;
            }
        }
    }
}

#[inline(always)]
fn idct4(s: &[i32]) -> [i32; 4] {
    let e0 = 64 * (s[0] + s[2]);
    let e1 = 64 * (s[0] - s[2]);
    let o0 = 83 * s[1] + 36 * s[3];
    let o1 = 36 * s[1] - 83 * s[3];
    [e0 + o0, e1 + o1, e1 - o1, e0 - o0]
}

/// Odd part of an N-point inverse DCT: o[k] = sum over odd j < nz of M_N[j][k] * s[j].
#[inline(always)]
fn odd_part<const H: usize>(s: &[i32], nz: usize, step: usize) -> [i32; H] {
    let mut o = [0i32; H];
    let mut j = 1;
    while j < nz {
        let c = s[j];
        if c != 0 {
            let row = &MAT32[j * step];
            for k in 0..H {
                o[k] += row[k] * c;
            }
        }
        j += 2;
    }
    o
}

#[inline(always)]
fn idct8(s: &[i32], nz: usize) -> [i32; 8] {
    let e = idct4(&[s[0], s[2], s[4], s[6]]);
    let o = odd_part::<4>(s, nz.min(8), 4);
    let mut out = [0i32; 8];
    for k in 0..4 {
        out[k] = e[k] + o[k];
        out[7 - k] = e[k] - o[k];
    }
    out
}

#[inline(always)]
fn idct16(s: &[i32], nz: usize) -> [i32; 16] {
    let mut ev = [0i32; 8];
    for k in 0..8 {
        ev[k] = s[2 * k];
    }
    let e = idct8(&ev, nz.div_ceil(2));
    let o = odd_part::<8>(s, nz.min(16), 2);
    let mut out = [0i32; 16];
    for k in 0..8 {
        out[k] = e[k] + o[k];
        out[15 - k] = e[k] - o[k];
    }
    out
}

#[inline(always)]
fn idct32(s: &[i32], nz: usize) -> [i32; 32] {
    let mut ev = [0i32; 16];
    for k in 0..16 {
        ev[k] = s[2 * k];
    }
    let e = idct16(&ev, nz.div_ceil(2));
    let o = odd_part::<16>(s, nz.min(32), 1);
    let mut out = [0i32; 32];
    for k in 0..16 {
        out[k] = e[k] + o[k];
        out[31 - k] = e[k] - o[k];
    }
    out
}

#[inline(always)]
fn idst4(s: &[i32]) -> [i32; 4] {
    let mut out = [0i32; 4];
    for (i, o) in out.iter_mut().enumerate() {
        *o = DST4[0][i] * s[0] + DST4[1][i] * s[1] + DST4[2][i] * s[2] + DST4[3][i] * s[3];
    }
    out
}

/// One 1-D inverse transform of `n` inputs (only the first `nz` may be non-zero).
#[inline(always)]
fn inv_1d(n: usize, dst: bool, s: &[i32], nz: usize, out: &mut [i32]) {
    match n {
        4 => out[..4].copy_from_slice(&if dst { idst4(s) } else { idct4(s) }),
        8 => out[..8].copy_from_slice(&idct8(s, nz)),
        16 => out[..16].copy_from_slice(&idct16(s, nz)),
        _ => out[..32].copy_from_slice(&idct32(s, nz)),
    }
}

/// Two-stage inverse transform (8.6.4.2) followed by the bdShift of 8.6.2.
/// `coeffs` holds d[x][y] at index y * n + x; the residual is written to `res`.
#[cfg(test)]
pub fn inverse_transform(coeffs: &[i32], res: &mut [i32], log2: u32, bit_depth: u32, dst: bool) {
    let n = 1usize << log2;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut any = false;
    for y in 0..n {
        for x in 0..n {
            if coeffs[y * n + x] != 0 {
                any = true;
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        for r in res[..n * n].iter_mut() {
            *r = 0;
        }
        return;
    }
    let mut tmp = [0i32; 32 * 32];
    inverse_transform_bbox(coeffs, res, &mut tmp, log2, bit_depth, dst, max_x, max_y);
}

/// As [`inverse_transform`], with the bounding box of the non-zero
/// coefficients (inclusive maxima) supplied by the caller and a scratch
/// buffer of at least n * n entries (contents irrelevant).
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn inverse_transform_bbox(
    coeffs: &[i32],
    res: &mut [i32],
    tmp: &mut [i32],
    log2: u32,
    bit_depth: u32,
    dst: bool,
    max_x: usize,
    max_y: usize,
) {
    let n = 1usize << log2;
    let bd_shift = 20 - bit_depth as i32;
    let rnd2 = 1i32 << (bd_shift - 1);
    if !dst && max_x == 0 && max_y == 0 {
        // DC only: both stages are constant multiplications by 64.
        let g = ((coeffs[0] * 64 + 64) >> 7).clamp(-32768, 32767);
        let r = (g * 64 + rnd2) >> bd_shift;
        for v in res[..n * n].iter_mut() {
            *v = r;
        }
        return;
    }
    let mut col = [0i32; 32];
    let mut out = [0i32; 32];
    // Stage 1: vertical transform of each column that can be non-zero.
    for x in 0..=max_x {
        for y in 0..n {
            col[y] = if y <= max_y { coeffs[y * n + x] } else { 0 };
        }
        inv_1d(n, dst, &col, max_y + 1, &mut out);
        for y in 0..n {
            tmp[y * n + x] = ((out[y] + 64) >> 7).clamp(-32768, 32767);
        }
    }
    // Stage 2: horizontal transform of each row.
    for y in 0..n {
        let row = &mut tmp[y * n..y * n + n];
        for v in row[max_x + 1..].iter_mut() {
            *v = 0;
        }
        inv_1d(n, dst, row, max_x + 1, &mut out);
        let r = &mut res[y * n..y * n + n];
        for x in 0..n {
            r[x] = (out[x] + rnd2) >> bd_shift;
        }
    }
}

/// Residual for transform_skip_flag == 1 (8.6.4.2, tsShift = 5 + log2).
pub fn transform_skip(coeffs: &[i32], res: &mut [i32], log2: u32, bit_depth: u32) {
    let n = 1usize << log2;
    let ts_shift = 5 + log2 as i32;
    let bd_shift = 20 - bit_depth as i32;
    let rnd = 1i32 << (bd_shift - 1);
    for i in 0..n * n {
        res[i] = ((coeffs[i] << ts_shift) + rnd) >> bd_shift;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(coeffs: &[i32], log2: u32, bit_depth: u32, dst: bool) -> Vec<i32> {
        let n = 1usize << log2;
        let mat = |k: usize, i: usize| -> i64 {
            if dst {
                DST4[k][i] as i64
            } else {
                MAT32[k * (32 / n)][i] as i64
            }
        };
        let mut tmp = vec![0i64; n * n];
        for x in 0..n {
            for y in 0..n {
                let mut s = 0i64;
                for k in 0..n {
                    s += mat(k, y) * coeffs[k * n + x] as i64;
                }
                tmp[y * n + x] = ((s + 64) >> 7).clamp(-32768, 32767);
            }
        }
        let bd_shift = 20 - bit_depth as i64;
        let mut out = vec![0i32; n * n];
        for y in 0..n {
            for x in 0..n {
                let mut s = 0i64;
                for k in 0..n {
                    s += mat(k, x) * tmp[y * n + k];
                }
                out[y * n + x] = ((s + (1 << (bd_shift - 1))) >> bd_shift) as i32;
            }
        }
        out
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    #[test]
    fn matrix_rows() {
        assert_eq!(&MAT32[1][..8], &[90, 90, 88, 85, 82, 78, 73, 67]);
        assert_eq!(&MAT32[2][..8], &[90, 87, 80, 70, 57, 43, 25, 9]);
        assert_eq!(&MAT32[4][..4], &[89, 75, 50, 18]);
        assert_eq!(&MAT32[8][..4], &[83, 36, -36, -83]);
        assert_eq!(&MAT32[16][..4], &[64, -64, -64, 64]);
        assert_eq!(&MAT32[3][..8], &[90, 82, 67, 46, 22, -4, -31, -54]);
        assert_eq!(MAT32[31][0], 4);
        assert_eq!(MAT32[31][31], -4);
        assert_eq!(&MAT32[30][..4], &[9, -25, 43, -57]);
        // orthogonality-ish: rows have norm close to 64^2*32
        for k in 0..32 {
            let s: i64 = MAT32[k].iter().map(|&v| (v as i64) * (v as i64)).sum();
            assert!((s - 131072).abs() < 600, "row {k} norm {s}");
        }
    }

    #[test]
    fn fast_matches_reference() {
        let mut rng = Lcg(12345);
        for log2 in 2..=5u32 {
            let n = 1usize << log2;
            for trial in 0..200 {
                let mut c = vec![0i32; n * n];
                let density = trial % 4;
                for v in c.iter_mut() {
                    let r = rng.next();
                    if r % 4 <= density as u32 {
                        let mag = if trial % 3 == 0 { 32767 } else { 600 };
                        *v = (rng.next() % (2 * mag + 1)) as i32 - mag as i32;
                    }
                }
                if trial % 5 == 0 {
                    // sparse low-frequency block
                    for v in c.iter_mut() {
                        *v = 0;
                    }
                    c[0] = (rng.next() % 2000) as i32 - 1000;
                    if trial % 10 == 0 {
                        c[1] = 77;
                    }
                }
                for bd in [8u32, 10] {
                    let mut out = vec![0i32; n * n];
                    inverse_transform(&c, &mut out, log2, bd, false);
                    assert_eq!(out, reference(&c, log2, bd, false), "log2 {log2} trial {trial}");
                    if log2 == 2 {
                        inverse_transform(&c, &mut out, log2, bd, true);
                        assert_eq!(out, reference(&c, log2, bd, true), "dst trial {trial}");
                    }
                }
            }
        }
    }

    #[test]
    fn dequant_flat() {
        let mut c = vec![0i32; 16];
        c[0] = 10;
        c[5] = -3;
        dequantize(&mut c, 2, 22, 8, None);
        // qp 22: levelScale[4]=64 << 3 = 512; m=16; bdShift = 8+2-5 = 5
        assert_eq!(c[0], (10 * 16 * 512 + 16) >> 5);
        assert_eq!(c[5], ((-3i64 * 16 * 512 + 16) >> 5) as i32);
    }
}
