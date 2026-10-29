// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M6: costs for the mode search: lambda from the quantiser, SAD and SATD of a residual, and a
//! rough bit count for the intra mode symbols. Costs are in SATD units, so `lambda` converts one
//! bit into the same units.

use studio_av1::spec_tables::*;

fn ac_step(base_q_idx: u8, bit_depth: u8) -> f64 {
    let bd_idx = ((bit_depth.max(8) - 8) >> 1) as usize;
    AC_QLOOKUP[bd_idx][base_q_idx as usize] as f64
}

/// Lagrange multiplier for a quantiser index: squared error per bit. The usual relation
/// `lambda ~ 0.1 * step^2` for the AC quantiser step.
pub fn lambda(base_q_idx: u8, bit_depth: u8) -> f64 {
    let q = ac_step(base_q_idx, bit_depth);
    0.1 * q * q
}

/// The multiplier for SATD costs: SATD units per bit.
pub fn lambda_satd(base_q_idx: u8, bit_depth: u8) -> f64 {
    ac_step(base_q_idx, bit_depth) * 0.25
}

/// Sum of absolute values of a residual.
pub fn sad(residual: &[i16]) -> u64 {
    residual.iter().map(|&r| (r as i32).unsigned_abs() as u64).sum()
}

/// Sum of absolute 4x4 Hadamard coefficients of a `w` x `h` residual (row stride `w`, both
/// multiples of 4): a cheap stand-in for the cost of coding the block after a transform.
pub fn satd(residual: &[i16], w: usize, h: usize) -> u64 {
    assert!(w % 4 == 0 && h % 4 == 0 && residual.len() >= w * h);
    let mut total = 0u64;
    for by in (0..h).step_by(4) {
        for bx in (0..w).step_by(4) {
            let mut m = [[0i32; 4]; 4];
            for (i, row) in m.iter_mut().enumerate() {
                for (j, v) in row.iter_mut().enumerate() {
                    *v = residual[(by + i) * w + bx + j] as i32;
                }
            }
            // rows then columns: 4-point Hadamard
            for row in m.iter_mut() {
                let (a, b, c, d) = (row[0] + row[3], row[1] + row[2], row[0] - row[3], row[1] - row[2]);
                *row = [a + b, a - b, c - d, c + d];
            }
            for j in 0..4 {
                let (a, b, c, d) = (m[0][j] + m[3][j], m[1][j] + m[2][j], m[0][j] - m[3][j], m[1][j] - m[2][j]);
                m[0][j] = a + b;
                m[1][j] = a - b;
                m[2][j] = c - d;
                m[3][j] = c + d;
            }
            total += m.iter().flatten().map(|&v| v.unsigned_abs() as u64).sum::<u64>();
        }
    }
    // the 4x4 Hadamard has gain 4 (sqrt(16)); bring it to the scale of an orthonormal transform
    total / 4
}

/// Approximate bits of coding intra mode `mode` (the y or uv mode symbol): the common modes are
/// cheaper, and a directional mode carries an angle delta.
pub fn mode_bits(mode: usize) -> f64 {
    match mode {
        DC_PRED => 2.0,
        V_PRED | H_PRED => 3.0,
        SMOOTH_PRED | PAETH_PRED => 3.5,
        _ => 4.5,
    }
}

/// Rough bits of a quantised block: a flag when empty, otherwise a fixed part, a cost per scan
/// position up to the end of block, and per nonzero level its sign and magnitude.
pub fn coef_bits(levels: &[i32], eob: usize) -> f64 {
    if eob == 0 {
        return 0.5;
    }
    let nonzero = levels.iter().filter(|&&l| l != 0).map(|&l| 2.5 + 2.0 * ((l.unsigned_abs() + 1) as f64).log2()).sum::<f64>();
    4.0 + 0.6 * eob as f64 + nonzero
}
