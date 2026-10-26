// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CAVLC residual block coding (9.2), the inverse of the decoder's parsing.

use super::tables::{vlc_tables, Code};
use crate::bits::BitWriter;

/// Which coeff_token table a block uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nc {
    /// Predicted nC of a regular block.
    Luma(i32),
    /// 4:2:0 chroma DC (nC = -1).
    ChromaDc,
}

#[inline]
fn put(w: &mut BitWriter, c: Code) {
    debug_assert!(c.len > 0, "missing VLC code");
    w.put(c.len as u32, c.bits as u32);
}

/// Write residual_block_cavlc() for `coef` (levels in scan order, length =
/// maxNumCoeff). Returns TotalCoeff.
pub fn write_block(w: &mut BitWriter, nc: Nc, coef: &[i32]) -> u32 {
    let t = vlc_tables();
    let max = coef.len();
    // Non-zero levels, highest frequency first, with their scan positions.
    let mut levels = [0i32; 16];
    let mut pos = [0usize; 16];
    let mut total = 0usize;
    for i in (0..max).rev() {
        if coef[i] != 0 {
            levels[total] = coef[i];
            pos[total] = i;
            total += 1;
        }
    }
    let mut t1 = 0usize;
    while t1 < total.min(3) && levels[t1].abs() == 1 {
        t1 += 1;
    }
    let table = match nc {
        Nc::Luma(n) if n < 2 => 0,
        Nc::Luma(n) if n < 4 => 1,
        Nc::Luma(n) if n < 8 => 2,
        Nc::Luma(_) => 3,
        Nc::ChromaDc => 4,
    };
    if table == 3 {
        // 6-bit fixed length code (Table 9-5, 8 <= nC).
        let c = t.coeff_token[3][total][t1];
        put(w, c);
    } else {
        put(w, t.coeff_token[table][total][t1]);
    }
    if total == 0 {
        return 0;
    }
    for &l in &levels[..t1] {
        w.put_bit(l < 0);
    }
    let mut suffix_length: u32 = if total > 10 && t1 < 3 { 1 } else { 0 };
    for i in t1..total {
        let level = levels[i];
        let mut level_code: i32 = if level > 0 { 2 * level - 2 } else { -2 * level - 1 };
        if i == t1 && t1 < 3 {
            level_code -= 2;
        }
        write_level(w, level_code as u32, suffix_length);
        if suffix_length == 0 {
            suffix_length = 1;
        }
        if level.abs() > (3 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
    }
    // total_zeros: zeros before the last (lowest frequency... ) highest non-zero coefficient.
    let last = pos[0];
    let total_zeros = last + 1 - total;
    if total < max {
        let c = if nc == Nc::ChromaDc { t.total_zeros_2x2[total - 1][total_zeros] } else { t.total_zeros_4x4[total - 1][total_zeros] };
        put(w, c);
    }
    let mut zeros_left = total_zeros;
    for i in 0..total - 1 {
        if zeros_left == 0 {
            break;
        }
        let run = pos[i] - pos[i + 1] - 1;
        put(w, t.run_before[zeros_left.min(7) - 1][run]);
        zeros_left -= run;
    }
    total as u32
}

/// level_prefix / level_suffix for `level_code` with `suffix_length`.
fn write_level(w: &mut BitWriter, level_code: u32, suffix_length: u32) {
    if suffix_length == 0 {
        if level_code < 14 {
            w.put(level_code + 1, 1);
        } else if level_code < 30 {
            w.put(15, 1);
            w.put(4, level_code - 14);
        } else {
            w.put(16, 1);
            w.put(12, (level_code - 30).min(4095));
        }
    } else {
        let prefix = level_code >> suffix_length;
        if prefix < 15 {
            w.put(prefix + 1, 1);
            w.put(suffix_length, level_code & ((1 << suffix_length) - 1));
        } else {
            w.put(16, 1);
            w.put(12, (level_code - (15 << suffix_length)).min(4095));
        }
    }
}
