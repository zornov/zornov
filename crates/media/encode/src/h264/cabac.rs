// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CABAC: the arithmetic encoding engine (9.3.4.2), context initialisation
//! (9.3.1.1) and the macroblock-layer binarisations / context selection
//! (9.3.2, 9.3.3.1) for I and P slices of progressive frames.

use super::cabac_tables::CABAC_INIT_MN;
use super::mb::{Kind, MbCoded, MbState, Neighbours};
use super::tables::BLK4_XY;
use crate::bits::BitWriter;

/// Table 9-44: rangeTabLPS[pStateIdx][qCodIRangeIdx].
#[rustfmt::skip]
static RANGE_TAB_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240], [128, 167, 197, 227], [128, 158, 187, 216], [123, 150, 178, 205],
    [116, 142, 169, 195], [111, 135, 160, 185], [105, 128, 152, 175], [100, 122, 144, 166],
    [95, 116, 137, 158], [90, 110, 130, 150], [85, 104, 123, 142], [81, 99, 117, 135],
    [77, 94, 111, 128], [73, 89, 105, 122], [69, 85, 100, 116], [66, 80, 95, 110],
    [62, 76, 90, 104], [59, 72, 86, 99], [56, 69, 81, 94], [53, 65, 77, 89],
    [51, 62, 73, 85], [48, 59, 69, 80], [46, 56, 66, 76], [43, 53, 63, 72],
    [41, 50, 59, 69], [39, 48, 56, 65], [37, 45, 54, 62], [35, 43, 51, 59],
    [33, 41, 48, 56], [32, 39, 46, 53], [30, 37, 43, 50], [29, 35, 41, 48],
    [27, 33, 39, 45], [26, 31, 37, 43], [24, 30, 35, 41], [23, 28, 33, 39],
    [22, 27, 32, 37], [21, 26, 30, 35], [20, 24, 29, 33], [19, 23, 27, 31],
    [18, 22, 26, 30], [17, 21, 25, 28], [16, 20, 23, 27], [15, 19, 22, 25],
    [14, 18, 21, 24], [14, 17, 20, 23], [13, 16, 19, 22], [12, 15, 18, 21],
    [12, 14, 17, 20], [11, 14, 16, 19], [11, 13, 15, 18], [10, 12, 15, 17],
    [10, 12, 14, 16], [9, 11, 13, 15], [9, 11, 12, 14], [8, 10, 12, 14],
    [8, 9, 11, 13], [7, 9, 11, 12], [7, 9, 10, 12], [7, 8, 10, 11],
    [6, 8, 9, 11], [6, 7, 9, 10], [6, 7, 8, 9], [2, 2, 2, 2],
];

/// Table 9-45: transIdxLPS.
#[rustfmt::skip]
static TRANS_IDX_LPS: [u8; 64] = [
     0,  0,  1,  2,  2,  4,  4,  5,  6,  7,  8,  9,  9, 11, 11, 12,
    13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21, 21, 22, 22, 23, 24,
    24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33,
    33, 33, 34, 34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];

/// (state << 1 | mps) transitions: [state][0 = MPS, 1 = LPS].
static NEXT_STATE: [[u8; 2]; 128] = {
    let mut t = [[0u8; 2]; 128];
    let mut s = 0;
    while s < 128 {
        let p = (s >> 1) as u8;
        let mps = (s & 1) as u8;
        let up = if p < 62 { p + 1 } else { p };
        t[s][0] = (up << 1) | mps;
        let new_mps = if p == 0 { 1 - mps } else { mps };
        t[s][1] = (TRANS_IDX_LPS[p as usize] << 1) | new_mps;
        s += 1;
    }
    t
};

/// ctxIdxBlockCatOffset for coded_block_flag, significance maps and levels.
const CBF_OFF: [usize; 5] = [0, 4, 8, 12, 16];
const SIG_OFF: [usize; 5] = [0, 15, 29, 44, 47];
const ABS_OFF: [usize; 5] = [0, 10, 20, 30, 39];

pub struct CabacWriter {
    w: BitWriter,
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
    ctx: [u8; 1024],
}

impl CabacWriter {
    /// Initialise contexts for the slice QP (cabac_init_idc 0 for P slices).
    pub fn new(qp: i32, p_slice: bool) -> CabacWriter {
        let qp = qp.clamp(0, 51);
        let table = &CABAC_INIT_MN[if p_slice { 0 } else { 3 }];
        let mut ctx = [0u8; 1024];
        for (i, mn) in table.iter().enumerate() {
            let (m, n) = (mn[0] as i32, mn[1] as i32);
            let pre = (((m * qp) >> 4) + n).clamp(1, 126);
            ctx[i] = if pre <= 63 { ((63 - pre) << 1) as u8 } else { (((pre - 64) << 1) | 1) as u8 };
        }
        CabacWriter { w: BitWriter::with_capacity(4096), low: 0, range: 510, outstanding: 0, first: true, ctx }
    }

    #[inline]
    fn put_bit(&mut self, b: bool) {
        if self.first {
            self.first = false;
        } else {
            self.w.put_bit(b);
        }
        while self.outstanding > 0 {
            self.w.put_bit(!b);
            self.outstanding -= 1;
        }
    }

    #[inline]
    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(false);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(true);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    #[inline]
    fn decision(&mut self, ctx: usize, bin: u32) {
        let s = self.ctx[ctx];
        let p = (s >> 1) as usize;
        let mps = (s & 1) as u32;
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            self.ctx[ctx] = NEXT_STATE[s as usize][1];
        } else {
            self.ctx[ctx] = NEXT_STATE[s as usize][0];
        }
        self.renorm();
    }

    #[inline]
    fn bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put_bit(true);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put_bit(false);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }

    fn terminate(&mut self, bin: u32) {
        self.range -= 2;
        if bin != 0 {
            self.low += self.range;
            // EncodeFlush: the final written bit doubles as rbsp_stop_one_bit.
            self.range = 2;
            self.renorm();
            self.put_bit((self.low >> 9) & 1 == 1);
            let v = ((self.low >> 7) & 3) | 1;
            self.w.put(2, v);
        } else {
            self.renorm();
        }
    }

    /// The coded slice data (byte aligned, ends with the stop bit).
    pub fn finish(self) -> Vec<u8> {
        self.w.finish()
    }

    /// Exp-Golomb (order k) suffix in bypass bins.
    fn exp_golomb(&mut self, mut v: u32, mut k: u32) {
        loop {
            if v >= (1 << k) {
                self.bypass(1);
                v -= 1 << k;
                k += 1;
            } else {
                self.bypass(0);
                while k > 0 {
                    k -= 1;
                    self.bypass((v >> k) & 1);
                }
                break;
            }
        }
    }

    /// One macroblock, then end_of_slice_flag.
    pub fn write_mb(&mut self, c: &MbCoded, cur: &MbState, nb: &Neighbours, states: &[MbState], p_slice: bool, last: bool) {
        let a = nb.a.map(|i| &states[i]);
        let b = nb.b.map(|i| &states[i]);
        if p_slice {
            let cond = |n: Option<&MbState>| n.is_some_and(|s| s.kind != Kind::PSkip) as usize;
            self.decision(11 + cond(a) + cond(b), (c.kind == Kind::PSkip) as u32);
            if c.kind == Kind::PSkip {
                self.terminate(last as u32);
                return;
            }
        }
        self.mb_type(c, a, b, p_slice);
        match c.kind {
            Kind::I4 => {
                for &rem in &c.i4_rem {
                    if rem == 8 {
                        self.decision(68, 1);
                    } else {
                        self.decision(68, 0);
                        for k in 0..3 {
                            self.decision(69, (rem as u32 >> k) & 1);
                        }
                    }
                }
                self.chroma_pred_mode(c.chroma_mode, a, b);
            }
            Kind::I16 => self.chroma_pred_mode(c.chroma_mode, a, b),
            _ => {
                for comp in 0..2 {
                    let amvd = a.map_or(0, |s| s.mvd[comp].unsigned_abs() as u32) + b.map_or(0, |s| s.mvd[comp].unsigned_abs() as u32);
                    self.mvd(if comp == 0 { 40 } else { 47 }, amvd, c.mvd[comp]);
                }
            }
        }
        if c.kind != Kind::I16 {
            self.cbp(c.cbp, a, b);
        }
        if c.cbp == 0 && c.kind != Kind::I16 {
            self.terminate(last as u32);
            return;
        }
        // mb_qp_delta = 0; the previous macroblock never has a non-zero delta.
        self.decision(60, 0);
        let intra = cur.intra();
        let cbp_l = c.cbp & 15;
        let cbp_c = c.cbp >> 4;
        if c.kind == Kind::I16 {
            let ca = cond_dc(a, intra, 0);
            let cb = cond_dc(b, intra, 0);
            self.block(0, ca + 2 * cb, &c.luma_dc);
        }
        for blk in 0..16 {
            if cbp_l & (1 << (blk / 4)) == 0 {
                continue;
            }
            let (bx, by) = BLK4_XY[blk];
            let ca = if bx > 0 { luma_flag(cur, bx - 1, by) } else { a.map_or(intra as usize, |s| luma_flag(s, 3, by)) };
            let cb = if by > 0 { luma_flag(cur, bx, by - 1) } else { b.map_or(intra as usize, |s| luma_flag(s, bx, 3)) };
            if c.kind == Kind::I16 {
                self.block(1, ca + 2 * cb, &c.luma[blk][1..]);
            } else {
                self.block(2, ca + 2 * cb, &c.luma[blk]);
            }
        }
        if cbp_c != 0 {
            for comp in 0..2 {
                let ca = cond_dc(a, intra, 1 + comp);
                let cb = cond_dc(b, intra, 1 + comp);
                self.block(3, ca + 2 * cb, &c.chroma_dc[comp]);
            }
        }
        if cbp_c == 2 {
            for comp in 0..2 {
                for k in 0..4 {
                    let (bx, by) = (k % 2, k / 2);
                    let ca = if bx > 0 { chroma_flag(cur, comp, 0, by) } else { a.map_or(intra as usize, |s| chroma_flag(s, comp, 1, by)) };
                    let cb = if by > 0 { chroma_flag(cur, comp, bx, 0) } else { b.map_or(intra as usize, |s| chroma_flag(s, comp, bx, 1)) };
                    self.block(4, ca + 2 * cb, &c.chroma_ac[comp][k][1..]);
                }
            }
        }
        self.terminate(last as u32);
    }

    fn mb_type(&mut self, c: &MbCoded, a: Option<&MbState>, b: Option<&MbState>, p_slice: bool) {
        let cbp_l = c.cbp & 15;
        let cbp_c = (c.cbp >> 4) as u32;
        if p_slice {
            if c.kind == Kind::P16 {
                self.decision(14, 0);
                self.decision(15, 0);
                self.decision(16, 0);
                return;
            }
            self.decision(14, 1);
            // Intra suffix (ctxIdxOffset 17).
            if c.kind == Kind::I4 {
                self.decision(17, 0);
                return;
            }
            self.decision(17, 1);
            self.terminate(0);
            self.decision(18, (cbp_l != 0) as u32);
            self.decision(19, (cbp_c != 0) as u32);
            if cbp_c != 0 {
                self.decision(19, (cbp_c == 2) as u32);
            }
            self.decision(20, (c.i16_mode as u32 >> 1) & 1);
            self.decision(20, c.i16_mode as u32 & 1);
            return;
        }
        let cond = |n: Option<&MbState>| n.is_some_and(|s| s.kind != Kind::I4) as usize;
        let inc = cond(a) + cond(b);
        if c.kind == Kind::I4 {
            self.decision(3 + inc, 0);
            return;
        }
        self.decision(3 + inc, 1);
        self.terminate(0);
        self.decision(3 + 3, (cbp_l != 0) as u32);
        self.decision(3 + 4, (cbp_c != 0) as u32);
        if cbp_c != 0 {
            self.decision(3 + 5, (cbp_c == 2) as u32);
        }
        self.decision(3 + 6, (c.i16_mode as u32 >> 1) & 1);
        self.decision(3 + 7, c.i16_mode as u32 & 1);
    }

    fn chroma_pred_mode(&mut self, mode: u8, a: Option<&MbState>, b: Option<&MbState>) {
        let cond = |n: Option<&MbState>| n.is_some_and(|s| s.intra() && s.chroma_mode != 0) as usize;
        let inc = cond(a) + cond(b);
        let m = mode as u32;
        self.decision(64 + inc, (m > 0) as u32);
        if m > 0 {
            self.decision(67, (m > 1) as u32);
            if m > 1 {
                self.decision(67, (m > 2) as u32);
            }
        }
    }

    fn mvd(&mut self, base: usize, amvd: u32, v: i32) {
        let inc = if amvd < 3 { 0 } else if amvd > 32 { 2 } else { 1 };
        let abs = v.unsigned_abs();
        self.decision(base + inc, (abs > 0) as u32);
        if abs == 0 {
            return;
        }
        // TU prefix (cMax 9).
        let mut ctx = base + 3;
        let mut k = 1;
        while k < 9 {
            let more = (abs > k) as u32;
            self.decision(ctx, more);
            if more == 0 {
                break;
            }
            if k < 4 {
                ctx += 1;
            }
            k += 1;
        }
        if abs >= 9 {
            self.exp_golomb(abs - 9, 3);
        }
        self.bypass((v < 0) as u32);
    }

    fn cbp(&mut self, cbp: u8, a: Option<&MbState>, b: Option<&MbState>) {
        let cbp_l = cbp & 15;
        // Luma prefix: one bin per 8x8 block, contexts from the neighbouring 8x8 blocks.
        let bit = |s: &MbState, b8: usize| -> usize {
            // condTermFlagN = 0 when that 8x8 has coefficients; skipped MBs count as "no".
            if s.kind != Kind::PSkip && (s.cbp >> b8) & 1 != 0 { 0 } else { 1 }
        };
        for b8 in 0..4usize {
            let ca = match b8 {
                0 => a.map_or(0, |s| bit(s, 1)),
                2 => a.map_or(0, |s| bit(s, 3)),
                1 => ((cbp_l >> 0) & 1 == 0) as usize,
                _ => ((cbp_l >> 2) & 1 == 0) as usize,
            };
            let cb = match b8 {
                0 => b.map_or(0, |s| bit(s, 2)),
                1 => b.map_or(0, |s| bit(s, 3)),
                2 => ((cbp_l >> 0) & 1 == 0) as usize,
                _ => ((cbp_l >> 1) & 1 == 0) as usize,
            };
            self.decision(73 + ca + 2 * cb, ((cbp_l >> b8) & 1) as u32);
        }
        let cbp_c = cbp >> 4;
        let cond = |n: Option<&MbState>, bin: usize| -> usize {
            match n {
                None => 0,
                Some(s) if s.kind == Kind::PSkip => 0,
                Some(s) => {
                    let c = s.cbp >> 4;
                    (if bin == 0 { c != 0 } else { c == 2 }) as usize
                }
            }
        };
        self.decision(77 + cond(a, 0) + 2 * cond(b, 0), (cbp_c != 0) as u32);
        if cbp_c != 0 {
            self.decision(77 + 4 + cond(a, 1) + 2 * cond(b, 1), (cbp_c == 2) as u32);
        }
    }

    /// residual_block_cabac(): coded_block_flag, significance map, levels.
    fn block(&mut self, cat: usize, cbf_inc: usize, coef: &[i32]) {
        let coded = coef.iter().any(|&v| v != 0);
        self.decision(85 + CBF_OFF[cat] + cbf_inc, coded as u32);
        if !coded {
            return;
        }
        let max = coef.len();
        let last = coef.iter().rposition(|&v| v != 0).unwrap_or(0);
        let (sig, lst, abs_base) = (105 + SIG_OFF[cat], 166 + SIG_OFF[cat], 227 + ABS_OFF[cat]);
        for (i, &v) in coef.iter().enumerate().take(max - 1) {
            let inc = if cat == 3 { i.min(2) } else { i };
            self.decision(sig + inc, (v != 0) as u32);
            if v != 0 {
                self.decision(lst + inc, (i == last) as u32);
                if i == last {
                    break;
                }
            }
        }
        let gt1_max = if cat == 3 { 3 } else { 4 };
        let (mut gt1, mut eq1) = (0usize, 0usize);
        for i in (0..=last).rev() {
            let v = coef[i];
            if v == 0 {
                continue;
            }
            let abs = v.unsigned_abs();
            let m1 = abs - 1;
            let inc0 = if gt1 != 0 { 0 } else { (1 + eq1).min(4) };
            self.decision(abs_base + inc0, (m1 > 0) as u32);
            if m1 > 0 {
                let ctx = abs_base + 5 + gt1.min(gt1_max);
                let mut k = 1;
                while k < 14 {
                    let more = (m1 > k) as u32;
                    self.decision(ctx, more);
                    if more == 0 {
                        break;
                    }
                    k += 1;
                }
                if m1 >= 14 {
                    self.exp_golomb(m1 - 14, 0);
                }
            }
            if abs == 1 {
                eq1 += 1;
            } else {
                gt1 += 1;
            }
            self.bypass((v < 0) as u32);
        }
    }
}

/// condTermFlag for DC blocks: `which` 0 = luma DC (I16), 1 = Cb DC, 2 = Cr DC.
fn cond_dc(n: Option<&MbState>, cur_intra: bool, which: usize) -> usize {
    match n {
        None => cur_intra as usize,
        Some(s) => {
            let avail = match which {
                0 => s.kind == Kind::I16,
                _ => s.kind != Kind::PSkip && (s.cbp >> 4) != 0,
            };
            (avail && s.dc_coded[which]) as usize
        }
    }
}

/// coded_block_flag of the luma 4x4 block at raster (bx, by) of `s` (0 when not transmitted).
fn luma_flag(s: &MbState, bx: usize, by: usize) -> usize {
    let b8 = (by / 2) * 2 + bx / 2;
    (s.kind != Kind::PSkip && (s.cbp >> b8) & 1 != 0 && s.tc_luma[by * 4 + bx] > 0) as usize
}

fn chroma_flag(s: &MbState, comp: usize, bx: usize, by: usize) -> usize {
    (s.kind != Kind::PSkip && (s.cbp >> 4) == 2 && s.tc_chroma[comp][by * 2 + bx] > 0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_state_is_consistent() {
        // MPS path saturates at state 62; LPS from state 0 flips the MPS.
        assert_eq!(NEXT_STATE[(62 << 1) | 1][0], (62 << 1) | 1);
        assert_eq!(NEXT_STATE[0][1], 1);
    }
}
