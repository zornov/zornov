// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M3: quantisation (spec 7.12.2 inverted) with a fixed quantiser.
//!
//! Levels are indexed by position `pos = row * tw + col` with `tw = min(width, 32)`, the
//! specification's `Quant[ pos ]`; the coefficient writer maps scan index to `pos` with
//! [`scan_for`]. Quantiser matrices and delta-Q offsets are not used: the step of the DC
//! coefficient is `Dc_Qlookup[ base_q_idx ]` and every other one `Ac_Qlookup[ base_q_idx ]`.

use studio_av1::spec_tables::*;

use crate::types::{CoefBlock, QuantParams, TxSize, TxType};

/// Rounding offset of the quantiser, in sixteenths of a step (8 = round to nearest). A slightly
/// smaller value widens the dead zone, which saves bits at little cost in PSNR.
pub const ROUNDING_16THS: i64 = 7;

/// Largest level magnitude the coefficient syntax round-trips (the decoder masks with 0xFFFFF).
const MAX_LEVEL: i64 = 0xFFFFF;

/// True when the frame is coded losslessly (`base_q_idx == 0`, no delta-Q): the WHT is used.
pub fn is_lossless(qp: QuantParams) -> bool {
    qp.base_q_idx == 0
}

/// `(dc_step, ac_step)` for the plane.
pub fn steps(qp: QuantParams) -> (i64, i64) {
    let bd_idx = ((qp.bit_depth.max(8) - 8) >> 1) as usize;
    let q = qp.base_q_idx as usize;
    (DC_QLOOKUP[bd_idx][q] as i64, AC_QLOOKUP[bd_idx][q] as i64)
}

/// The divisor the decoder applies to dequantised values of large blocks (7.12.3).
fn dq_denom(tx_size: TxSize) -> i64 {
    match tx_size {
        TX_32X32 | TX_16X32 | TX_32X16 | TX_16X64 | TX_64X16 => 2,
        TX_64X64 | TX_32X64 | TX_64X32 => 4,
        _ => 1,
    }
}

/// Coded dimensions of a block: `(tw, th)`, 64-point sides carrying only 32 coefficients.
pub fn coded_dims(tx_size: TxSize) -> (usize, usize) {
    ((1usize << TX_WIDTH_LOG2[tx_size]).min(32), (1usize << TX_HEIGHT_LOG2[tx_size]).min(32))
}

/// The scan (scan index -> `pos`) the decoder uses for the block, as chosen in the coefficient
/// syntax (5.11.39): 64-point blocks use the 32-point scans, 1D identity-class types the row or
/// column scans, everything else the default scan.
pub fn scan_for(tx_size: TxSize, tx_type: TxType) -> &'static [u16] {
    match tx_size {
        TX_16X64 => return &DEFAULT_SCAN_16X32,
        TX_64X16 => return &DEFAULT_SCAN_32X16,
        _ => {}
    }
    if TX_SIZE_SQR_UP[tx_size] as usize == TX_64X64 {
        return &DEFAULT_SCAN_32X32;
    }
    let row = matches!(tx_type, V_DCT | V_ADST | V_FLIPADST);
    let col = matches!(tx_type, H_DCT | H_ADST | H_FLIPADST);
    let pick = |dflt: &'static [u16], r: &'static [u16], c: &'static [u16]| if row { r } else if col { c } else { dflt };
    match tx_size {
        TX_4X4 => pick(&DEFAULT_SCAN_4X4, &MROW_SCAN_4X4, &MCOL_SCAN_4X4),
        TX_4X8 => pick(&DEFAULT_SCAN_4X8, &MROW_SCAN_4X8, &MCOL_SCAN_4X8),
        TX_8X4 => pick(&DEFAULT_SCAN_8X4, &MROW_SCAN_8X4, &MCOL_SCAN_8X4),
        TX_8X8 => pick(&DEFAULT_SCAN_8X8, &MROW_SCAN_8X8, &MCOL_SCAN_8X8),
        TX_8X16 => pick(&DEFAULT_SCAN_8X16, &MROW_SCAN_8X16, &MCOL_SCAN_8X16),
        TX_16X8 => pick(&DEFAULT_SCAN_16X8, &MROW_SCAN_16X8, &MCOL_SCAN_16X8),
        TX_16X16 => pick(&DEFAULT_SCAN_16X16, &MROW_SCAN_16X16, &MCOL_SCAN_16X16),
        TX_4X16 => pick(&DEFAULT_SCAN_4X16, &MROW_SCAN_4X16, &MCOL_SCAN_4X16),
        TX_16X4 => pick(&DEFAULT_SCAN_16X4, &MROW_SCAN_16X4, &MCOL_SCAN_16X4),
        // 1D types exist only up to 16 points, so the larger sizes always scan by default.
        TX_16X32 => &DEFAULT_SCAN_16X32,
        TX_32X16 => &DEFAULT_SCAN_32X16,
        TX_8X32 => &DEFAULT_SCAN_8X32,
        TX_32X8 => &DEFAULT_SCAN_32X8,
        _ => &DEFAULT_SCAN_32X32,
    }
}

/// Quantises the 64x64-stride `coef` block; returns the levels indexed by `pos` and the end of
/// block, counted along the default scan. For a type whose scan is not the default (the 1D
/// types) use [`quantize_block`].
pub fn quantize(coef: &[i32], tx_size: TxSize, qp: QuantParams) -> (Vec<i32>, usize) {
    let (levels, eob) = quantize_scan(coef, tx_size, qp, scan_for(tx_size, 0));
    (levels, eob)
}

/// Quantises a block for the given type and packages it for the coefficient writer.
pub fn quantize_block(coef: &[i32], tx_size: TxSize, tx_type: TxType, plane: u8, qp: QuantParams) -> CoefBlock {
    let (levels, eob) = quantize_scan(coef, tx_size, qp, scan_for(tx_size, tx_type));
    CoefBlock { plane, tx_size, tx_type, levels, eob }
}

fn quantize_scan(coef: &[i32], tx_size: TxSize, qp: QuantParams, scan: &[u16]) -> (Vec<i32>, usize) {
    let (tw, th) = coded_dims(tx_size);
    let (dc_q, ac_q) = steps(qp);
    let denom = dq_denom(tx_size);
    let mut levels = vec![0i32; tw * th];
    for i in 0..th {
        for j in 0..tw {
            let c = coef[i * 64 + j] as i64;
            let q = if i == 0 && j == 0 { dc_q } else { ac_q };
            // The decoder reconstructs level * q / denom.
            let mag = ((c.abs() * denom * 16 + q * ROUNDING_16THS) / (q * 16)).min(MAX_LEVEL);
            levels[i * tw + j] = if c < 0 { -mag } else { mag } as i32;
        }
    }
    let eob = scan.iter().rposition(|&p| levels[p as usize] != 0).map_or(0, |s| s + 1);
    (levels, eob)
}

/// The dequantised value of `level` at `pos` for a block without large-block scaling (every size
/// whose sides are at most 16; see [`dequantize_sized`] for the others). `pos == 0` is the DC.
pub fn dequantize(level: i32, pos: usize, qp: QuantParams) -> i32 {
    dequantize_sized(level, pos, TX_4X4, qp)
}

/// The dequantised value of `level` at `pos` (what the decoder reconstructs, 7.12.3).
pub fn dequantize_sized(level: i32, pos: usize, tx_size: TxSize, qp: QuantParams) -> i32 {
    let (dc_q, ac_q) = steps(qp);
    let bd = qp.bit_depth.max(8) as u32;
    let lim = 1i64 << (7 + bd);
    let dq = level as i64 * if pos == 0 { dc_q } else { ac_q };
    let mag = (dq.abs() & 0xFFFFFF) / dq_denom(tx_size);
    (if dq < 0 { -mag } else { mag }).clamp(-lim, lim - 1) as i32
}

/// Dequantises a whole block into the 64x64-stride layout `inverse_transform_2d` reads (only the
/// top-left `tw x th` entries are written).
pub fn dequantize_block(levels: &[i32], tx_size: TxSize, qp: QuantParams, out: &mut [i32]) {
    let (tw, th) = coded_dims(tx_size);
    for i in 0..th {
        for j in 0..tw {
            out[i * 64 + j] = dequantize_sized(levels[i * tw + j], i * tw + j, tx_size, qp);
        }
    }
}
