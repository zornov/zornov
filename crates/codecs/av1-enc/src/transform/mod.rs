// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M3: forward transforms (spec 7.13 inverted), producing the 64x64-stride coefficient layout
//! `studio_av1::transform::inverse_transform_2d` reads.
//!
//! The specification defines only the inverse. Its 1D inverses are, up to 12-bit rounding, the
//! matrices `M` below (output `n` from coefficient `k`):
//!
//! * DCT: `c_k cos((2n+1) k pi / 2N)`, `c_0 = 1/sqrt2`, else 1;
//! * ADST4: `(2 sqrt2 / 3) sin(pi (n+1)(2k+1) / 9)` (the `SINPI_k_9` constants of 7.13.2.6);
//! * ADST8 / ADST16: `sin(pi (2n+1)(2k+1) / 4N)`;
//! * identity: `sqrt(N/2)`.
//!
//! Each satisfies `M^T M = (N/2) I`, so the forward 1D transform is `F = (2/N) M^T`. The 2D
//! forward transform applies `F` to the rows, then the columns, and multiplies by the gain that
//! cancels the inverse's shifts (`Transform_Row_Shift`, the final 4, and the 1/sqrt2 of
//! rectangular blocks). Rows and columns beyond 32 are dropped for 64-point transforms, as the
//! decoder treats those coefficients as zero. The lossless path is the exact integer inverse of
//! the Walsh-Hadamard steps of 7.13.2.10.

pub mod quant;

use std::sync::OnceLock;

use studio_av1::spec_tables::*;

use crate::types::{TxSize, TxType};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind1d {
    Dct = 0,
    Adst = 1,
    Identity = 2,
}

/// The transform applied to the rows (horizontal direction) for `tx_type`.
pub(crate) fn row_kind(tx_type: TxType) -> Kind1d {
    match tx_type {
        DCT_DCT | ADST_DCT | FLIPADST_DCT | H_DCT => Kind1d::Dct,
        DCT_ADST | ADST_ADST | DCT_FLIPADST | FLIPADST_FLIPADST | ADST_FLIPADST | FLIPADST_ADST | H_ADST | H_FLIPADST => Kind1d::Adst,
        _ => Kind1d::Identity,
    }
}

/// The transform applied to the columns (vertical direction) for `tx_type`.
pub(crate) fn col_kind(tx_type: TxType) -> Kind1d {
    match tx_type {
        DCT_DCT | DCT_ADST | DCT_FLIPADST | V_DCT => Kind1d::Dct,
        ADST_DCT | ADST_ADST | FLIPADST_DCT | FLIPADST_FLIPADST | ADST_FLIPADST | FLIPADST_ADST | V_ADST | V_FLIPADST => Kind1d::Adst,
        _ => Kind1d::Identity,
    }
}

/// The flips the reconstruction applies to the residual (up-down, left-right), 7.12.3.
pub fn flips(tx_type: TxType) -> (bool, bool) {
    let ud = matches!(tx_type, FLIPADST_DCT | FLIPADST_ADST | V_FLIPADST | FLIPADST_FLIPADST);
    let lr = matches!(tx_type, DCT_FLIPADST | ADST_FLIPADST | H_FLIPADST | FLIPADST_FLIPADST);
    (ud, lr)
}

/// Whether the transform size/type pair can be coded: DCT up to 64, ADST up to 16, identity up
/// to 32 (the other pairs have no inverse in the specification).
pub fn is_supported(tx_size: TxSize, tx_type: TxType) -> bool {
    let (lw, lh) = (TX_WIDTH_LOG2[tx_size] as u32, TX_HEIGHT_LOG2[tx_size] as u32);
    let ok = |k: Kind1d, l: u32| match k {
        Kind1d::Dct => true,
        Kind1d::Adst => l <= 4,
        Kind1d::Identity => l <= 5,
    };
    tx_type < 16 && ok(row_kind(tx_type), lw) && ok(col_kind(tx_type), lh)
}

/// `F[j * m + k]`: the forward 1D matrix of length `1 << log2n`, transposed, keeping the first
/// `m = min(N, 32)` coefficients (sample `j` contributes `F[j][k]` to coefficient `k`).
fn basis(kind: Kind1d, log2n: u32) -> &'static [f32] {
    static CACHE: [[OnceLock<Vec<f32>>; 5]; 3] = [const { [const { OnceLock::new() }; 5] }; 3];
    assert!((2..=6).contains(&log2n), "transform length 2^{log2n}");
    CACHE[kind as usize][log2n as usize - 2].get_or_init(|| {
        let n = 1usize << log2n;
        let m = n.min(32);
        let nf = n as f64;
        let mut f = vec![0f32; n * m];
        for j in 0..n {
            for k in 0..m {
                let (jf, kf) = (j as f64, k as f64);
                let inv = match kind {
                    Kind1d::Dct => {
                        let c = if k == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                        c * ((2.0 * jf + 1.0) * kf * std::f64::consts::PI / (2.0 * nf)).cos()
                    }
                    Kind1d::Adst if n == 4 => (2.0 * 2f64.sqrt() / 3.0) * ((jf + 1.0) * (2.0 * kf + 1.0) * std::f64::consts::PI / 9.0).sin(),
                    Kind1d::Adst => ((2.0 * jf + 1.0) * (2.0 * kf + 1.0) * std::f64::consts::PI / (4.0 * nf)).sin(),
                    Kind1d::Identity => {
                        if j == k {
                            (nf / 2.0).sqrt()
                        } else {
                            0.0
                        }
                    }
                };
                f[j * m + k] = (2.0 / nf * inv) as f32;
            }
        }
        f
    })
}

/// `residual` is `w * h` samples in picture orientation, row stride `w`; the flip types
/// (FLIPADST) are handled here, since the decoder flips its inverse output when adding it to the
/// prediction (7.12.3). `out` is a 64x64 buffer (row stride 64) of
/// coefficients. Only the top-left `min(w, 32) x min(h, 32)` entries are written (the only ones
/// the decoder reads); the caller need not clear the rest.
///
/// # Panics
/// If the size/type pair is not coded ([`is_supported`]) or the buffers are too short.
pub fn forward_transform(residual: &[i16], tx_size: TxSize, tx_type: TxType, out: &mut [i32]) {
    assert!(is_supported(tx_size, tx_type), "unsupported transform {tx_size}/{tx_type}");
    let (lw, lh) = (TX_WIDTH_LOG2[tx_size] as u32, TX_HEIGHT_LOG2[tx_size] as u32);
    let (w, h) = (1usize << lw, 1usize << lh);
    let (tw, th) = (w.min(32), h.min(32));
    assert!(residual.len() >= w * h && out.len() >= 64 * (th - 1) + tw);
    let fr = basis(row_kind(tx_type), lw);
    let fc = basis(col_kind(tx_type), lh);

    // Row pass: tmp[i][k] = sum_j fr[j][k] * x[i][j], for all h input rows, tw coefficients.
    let mut tmp = vec![0f32; h * tw];
    let (flip_ud, flip_lr) = flips(tx_type);
    for i in 0..h {
        let t = &mut tmp[i * tw..(i + 1) * tw];
        let src = &residual[(if flip_ud { h - 1 - i } else { i }) * w..][..w];
        for j in 0..w {
            let x = src[if flip_lr { w - 1 - j } else { j }];
            if x != 0 {
                let x = x as f32;
                for (tk, &f) in t.iter_mut().zip(&fr[j * tw..(j + 1) * tw]) {
                    *tk += f * x;
                }
            }
        }
    }

    // Column pass, with the gain that cancels the inverse's row shift, column shift (4) and the
    // 1/sqrt2 of 2:1 blocks.
    let rect2 = lw.abs_diff(lh) == 1;
    let gain = (1u32 << (TRANSFORM_ROW_SHIFT[tx_size] as u32 + 4)) as f32 * if rect2 { std::f32::consts::SQRT_2 } else { 1.0 };
    let mut acc = [0f32; 32];
    for l in 0..th {
        acc[..tw].fill(0.0);
        for i in 0..h {
            let f = fc[i * th + l];
            for (a, &t) in acc[..tw].iter_mut().zip(&tmp[i * tw..(i + 1) * tw]) {
                *a += f * t;
            }
        }
        for (o, &a) in out[l * 64..l * 64 + tw].iter_mut().zip(&acc[..tw]) {
            *o = (a * gain).round() as i32;
        }
    }
}

/// The 4x4 Walsh-Hadamard transform of lossless mode: the exact integer inverse of the decoder's
/// 7.13.2.10 steps (columns first, then rows, then the `<< 2` the decoder's row `>> 2` undoes).
/// With `base_q_idx == 0` the quantiser step is 4, so the levels are `out / 4` exactly.
/// `residual` is 16 samples (stride 4); `out` uses the 64-stride layout.
pub fn forward_wht(residual: &[i16], out: &mut [i32]) {
    assert!(residual.len() >= 16 && out.len() >= 64 * 3 + 4);
    let mut t = [0i32; 16];
    for (d, &s) in t.iter_mut().zip(residual) {
        *d = s as i32;
    }
    for j in 0..4 {
        let mut v = [t[j], t[4 + j], t[8 + j], t[12 + j]];
        fwht_1d(&mut v);
        for i in 0..4 {
            t[i * 4 + j] = v[i];
        }
    }
    for i in 0..4 {
        let mut v = [t[i * 4], t[i * 4 + 1], t[i * 4 + 2], t[i * 4 + 3]];
        fwht_1d(&mut v);
        for j in 0..4 {
            out[i * 64 + j] = v[j] << 2;
        }
    }
}

/// Inverts `inverse_wht`: given its outputs (a, b, c, d) = t[0..4], recovers its inputs.
fn fwht_1d(t: &mut [i32; 4]) {
    let (a3, b2, c2, d3) = (t[0], t[1], t[2], t[3]);
    let a1 = a3 + b2;
    let d1 = d3 - c2;
    let e = (a1 - d1) >> 1;
    let b0 = e - b2;
    let c0 = e - c2;
    let a0 = a1 - c0;
    let d0 = d1 + b0;
    *t = [a0, c0, d0, b0];
}
