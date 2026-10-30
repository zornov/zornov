// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `read_lr()` and `read_lr_unit()` (spec 5.11.57, 5.11.58), inverted: the restoration units
//! covered by a superblock are coded in front of its partition tree.

use studio_av1::cdf::Cdfs;
use studio_av1::spec_tables::*;

use crate::entropy::SymbolEncoder;
use crate::types::LrChoice;

const WIENER_MIN: [i32; 3] = [-5, -23, -17];
const WIENER_MAX: [i32; 3] = [10, 8, 46];
const WIENER_K: [u32; 3] = [1, 2, 3];
const SGR_MIN: [i32; 2] = [-96, -32];
const SGR_MAX: [i32; 2] = [31, 95];

/// The restoration units of a frame plus the reference values the unit coefficients are coded
/// against (reset at the start of the tile).
pub struct LrWriter {
    pub choice: LrChoice,
    ref_wiener: [[[i32; 3]; 2]; 3],
    ref_xqd: [[i32; 2]; 3],
}

impl LrWriter {
    pub fn new(choice: LrChoice) -> LrWriter {
        LrWriter { choice, ref_wiener: [[[3, -7, 15]; 2]; 3], ref_xqd: [[-32, 31]; 3] }
    }

    /// The units of the superblock whose top left mode info position is (`r`, `c`), of
    /// `sb4` x `sb4` mode info units.
    pub fn write_sb(&mut self, enc: &mut SymbolEncoder, cdfs: &mut Cdfs, r: usize, c: usize, sb4: usize) {
        for plane in 0..3 {
            if !self.choice.params.switchable[plane] {
                continue;
            }
            let sub = if plane == 0 { 0 } else { 1 };
            let unit_size = self.choice.params.unit_size(plane);
            let (rows, cols) = (self.choice.unit_rows[plane], self.choice.unit_cols[plane]);
            let row_start = (r * (4 >> sub)).div_ceil(unit_size);
            let row_end = rows.min(((r + sb4) * (4 >> sub)).div_ceil(unit_size));
            let col_start = (c * (4 >> sub)).div_ceil(unit_size);
            let col_end = cols.min(((c + sb4) * (4 >> sub)).div_ceil(unit_size));
            for ur in row_start..row_end {
                for uc in col_start..col_end {
                    self.write_unit(enc, cdfs, plane, ur * cols + uc);
                }
            }
        }
    }

    fn write_unit(&mut self, enc: &mut SymbolEncoder, cdfs: &mut Cdfs, plane: usize, idx: usize) {
        let u = self.choice.units[plane][idx];
        // restoration_type: 0 none, 1 Wiener, 2 self-guided
        enc.write_symbol(&mut cdfs.restoration_type, u.kind as usize);
        if u.kind as usize == RESTORE_WIENER {
            for pass in 0..2 {
                let first = if plane > 0 { 1 } else { 0 };
                for j in first..3 {
                    let v = u.wiener[pass][j] as i32;
                    write_signed_subexp(enc, WIENER_MIN[j], WIENER_MAX[j] + 1, WIENER_K[j], self.ref_wiener[plane][pass][j], v);
                    self.ref_wiener[plane][pass][j] = v;
                }
            }
        } else if u.kind as usize == RESTORE_SGRPROJ {
            enc.write_literal(u.sgr_set as u32, SGRPROJ_PARAMS_BITS as u32);
            for i in 0..2 {
                let radius = SGR_PARAMS[u.sgr_set as usize][i * 2];
                let v = if radius != 0 {
                    let v = u.sgr_xqd[i] as i32;
                    write_signed_subexp(enc, SGR_MIN[i], SGR_MAX[i] + 1, SGRPROJ_PRJ_SUBEXP_K as u32, self.ref_xqd[plane][i], v);
                    v
                } else if i == 1 {
                    ((1 << SGRPROJ_PRJ_BITS) - self.ref_xqd[plane][0]).clamp(SGR_MIN[i], SGR_MAX[i])
                } else {
                    0
                };
                assert_eq!(v, u.sgr_xqd[i] as i32, "an unused self-guided weight must be the implied one");
                self.ref_xqd[plane][i] = v;
            }
        }
    }
}

fn recenter(r: i32, v: i32) -> i32 {
    if v > 2 * r {
        v
    } else if v >= r {
        (v - r) << 1
    } else {
        ((r - v) << 1) - 1
    }
}

fn write_signed_subexp(enc: &mut SymbolEncoder, low: i32, high: i32, k: u32, r: i32, v: i32) {
    assert!((low..high).contains(&v) && (low..high).contains(&r), "subexp value {v} / reference {r} outside {low}..{high}");
    let (mx, r, v) = (high - low, r - low, v - low);
    let x = if (r << 1) <= mx { recenter(r, v) } else { recenter(mx - 1 - r, mx - 1 - v) };
    write_subexp(enc, mx, k, x);
}

fn write_subexp(enc: &mut SymbolEncoder, num_syms: i32, k: u32, x: i32) {
    let (mut i, mut mk) = (0u32, 0i32);
    loop {
        let b2 = if i > 0 { k + i - 1 } else { k };
        let a = 1i32 << b2;
        if num_syms <= mk + 3 * a {
            enc.write_ns((x - mk) as u32, (num_syms - mk) as u32);
            return;
        } else if x >= mk + a {
            enc.write_literal(1, 1);
            i += 1;
            mk += a;
        } else {
            enc.write_literal(0, 1);
            enc.write_literal((x - mk) as u32, b2);
            return;
        }
    }
}
