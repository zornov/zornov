// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CABAC: context initialisation (9.3.1.1), arithmetic decoding engine (9.3.1.2, 9.3.3.2)
//! and the binarisations of the syntax elements (9.3.2, 9.3.3.1.2/3).
//! Context index increments that depend on neighbouring macroblocks are computed by the
//! slice decoder and passed in.

use crate::bitreader::BitReader;
use crate::cabac_tables::CABAC_INIT_MN;
use crate::error::{Error, Result};

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

/// Table 9-45: transIdxMPS.
const fn trans_idx_mps(s: u8) -> u8 {
    if s < 62 { s + 1 } else { s }
}

/// Combined (state << 1 | mps) transition: [state][0 = MPS path, 1 = LPS path].
static NEXT_STATE: [[u8; 2]; 128] = {
    let mut t = [[0u8; 2]; 128];
    let mut s = 0;
    while s < 128 {
        let p = (s >> 1) as u8;
        let mps = (s & 1) as u8;
        t[s][0] = (trans_idx_mps(p) << 1) | mps;
        let new_mps = if p == 0 { 1 - mps } else { mps };
        t[s][1] = (TRANS_IDX_LPS[p as usize] << 1) | new_mps;
        s += 1;
    }
    t
};

/// Table 9-43: significant_coeff_flag ctxIdxInc for frame coded 8x8 blocks.
#[rustfmt::skip]
static SIG8_FRAME: [u8; 63] = [
     0,  1,  2,  3,  4,  5,  5,  4,  4,  3,  3,  4,  4,  4,  5,  5,
     4,  4,  4,  4,  3,  3,  6,  7,  7,  7,  8,  9, 10,  9,  8,  7,
     7,  6, 11, 12, 13, 11,  6,  7,  8,  9, 14, 10,  9,  8,  6, 11,
    12, 13, 11,  6,  9, 14, 10,  9, 11, 12, 13, 11, 14, 10, 12,
];

/// Table 9-43: significant_coeff_flag ctxIdxInc for field coded 8x8 blocks.
#[rustfmt::skip]
static SIG8_FIELD: [u8; 63] = [
     0,  1,  1,  2,  2,  3,  3,  4,  5,  6,  7,  7,  7,  8,  4,  5,
     6,  9, 10, 10,  8, 11, 12, 11,  9,  9, 10, 10,  8, 11, 12, 11,
     9,  9, 10, 10,  8, 11, 12, 11,  9,  9, 10, 10,  8, 13, 13,  9,
     9, 10, 10,  8, 13, 13,  9,  9, 10, 10, 14, 14, 14, 14, 14,
];

/// Table 9-43: last_significant_coeff_flag ctxIdxInc for 8x8 blocks.
#[rustfmt::skip]
static LAST8: [u8; 63] = [
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 6, 6, 6, 6, 7, 7, 7, 7, 8, 8, 8,
];

/// Table 9-40: ctxIdxBlockCatOffset for ctxBlockCat 0..=4:
/// coded_block_flag, significant/last_significant_coeff_flag, coeff_abs_level_minus1.
const CBF_OFF: [usize; 5] = [0, 4, 8, 12, 16];
const SIG_OFF: [usize; 5] = [0, 15, 29, 44, 47];
const ABS_OFF: [usize; 5] = [0, 10, 20, 30, 39];

pub const CTX_CODED_BLOCK_FLAG: usize = 85;

pub struct Cabac<'a> {
    br: BitReader<'a>,
    range: u32,
    /// codIOffset followed by the next `bits` not yet consumed bits of the stream
    /// (`codIOffset << bits | lookahead`). Renormalisation then only lowers `bits`, and the
    /// bit reader is touched once per 32 bits instead of once per renormalisation.
    value: u64,
    bits: u32,
    pub ctx: [u8; 1024],
}

impl<'a> Cabac<'a> {
    /// Initialise contexts (9.3.1.1) and the decoding engine (9.3.1.2) at the byte-aligned
    /// bit position `pos` of `data`. `init` is cabac_init_idc (0..=2) or 3 for I/SI slices.
    pub fn new(data: &'a [u8], pos: usize, slice_qp: i32, init: usize) -> Result<Cabac<'a>> {
        let mut br = BitReader::new(data);
        br.set_pos(pos);
        let mut c = Cabac { br, range: 510, value: 0, bits: 0, ctx: [0; 1024] };
        let qp = slice_qp.clamp(0, 51);
        let table = &CABAC_INIT_MN[init];
        for (i, mn) in table.iter().enumerate() {
            let (m, n) = (mn[0] as i32, mn[1] as i32);
            let pre = (((m * qp) >> 4) + n).clamp(1, 126);
            c.ctx[i] = if pre <= 63 { ((63 - pre) << 1) as u8 } else { (((pre - 64) << 1) | 1) as u8 };
        }
        c.init_engine()?;
        Ok(c)
    }

    fn init_engine(&mut self) -> Result<()> {
        self.range = 510;
        let offset = self.br.read(9);
        self.value = offset as u64;
        self.bits = 0;
        if offset >= 510 {
            return Err(Error::Bitstream("invalid CABAC codIOffset"));
        }
        Ok(())
    }

    /// Bit position of the arithmetic decoder (bits consumed into codIOffset).
    #[inline]
    fn pos(&self) -> usize {
        self.br.pos() - self.bits as usize
    }

    /// Append 32 lookahead bits (zero past the end of the data, like the bit reader).
    #[inline(always)]
    fn refill(&mut self) {
        self.value = (self.value << 32) | self.br.read(32) as u64;
        self.bits += 32;
    }

    pub fn overrun(&self) -> bool {
        // The engine pre-reads 9 bits; allow reading slightly past the end (trailing
        // cabac_zero_words or the stop bit) but not arbitrarily far.
        self.pos() > self.br.bits_total() + 16
    }

    pub fn check(&self) -> Result<()> {
        if self.overrun() { Err(Error::Bitstream("CABAC data exhausted")) } else { Ok(()) }
    }

    /// Read I_PCM samples: align, read bytes, then re-initialise the engine (9.3.1.2).
    pub fn read_pcm(&mut self, out: &mut [u8]) -> Result<()> {
        let pos = self.pos();
        self.br.set_pos(pos);
        self.bits = 0;
        self.br.align();
        for v in out.iter_mut() {
            *v = self.br.read(8) as u8;
        }
        if self.br.overrun() {
            return Err(Error::Bitstream("truncated I_PCM samples"));
        }
        self.init_engine()
    }

    /// DecodeDecision (9.3.3.2.1).
    #[inline(always)]
    pub fn decision(&mut self, ctx_idx: usize) -> u32 {
        // Context states are always < 128 (6-bit pStateIdx << 1 | valMPS); the masks only
        // let the compiler drop bounds checks. The LPS/MPS choice is computed without
        // branches (conditional selects).
        let s = self.ctx[ctx_idx] as usize & 127;
        let q = ((self.range >> 6) & 3) as usize;
        let lps = RANGE_TAB_LPS[s >> 1][q] as u32;
        let rmps = self.range - lps;
        let scaled = (rmps as u64) << self.bits;
        let is_lps = self.value >= scaled;
        self.value -= if is_lps { scaled } else { 0 };
        let range = if is_lps { lps } else { rmps };
        self.ctx[ctx_idx] = NEXT_STATE[s][is_lps as usize];
        // codIRange >= 2 always, so this shift is 0..=7 (0 when no renormalisation needed).
        let shift = range.leading_zeros() - 23;
        self.range = range << shift;
        if self.bits < shift {
            self.refill();
        }
        self.bits -= shift;
        (s as u32 & 1) ^ is_lps as u32
    }

    /// DecodeBypass (9.3.3.2.3).
    #[inline(always)]
    pub fn bypass(&mut self) -> u32 {
        if self.bits == 0 {
            self.refill();
        }
        self.bits -= 1;
        let scaled = (self.range as u64) << self.bits;
        if self.value >= scaled {
            self.value -= scaled;
            1
        } else {
            0
        }
    }

    /// DecodeTerminate (9.3.3.2.2.3).
    #[inline]
    pub fn terminate(&mut self) -> u32 {
        self.range -= 2;
        if self.value >= (self.range as u64) << self.bits {
            1
        } else {
            if self.range < 256 {
                let shift = self.range.leading_zeros() - 23;
                self.range <<= shift;
                if self.bits < shift {
                    self.refill();
                }
                self.bits -= shift;
            }
            0
        }
    }

    // ---- binarisations -------------------------------------------------------------

    /// I-slice mb_type with the exact context assignment of Table 9-39.
    pub fn mb_type_i_slice(&mut self, ctx_inc0: usize) -> u32 {
        if self.decision(3 + ctx_inc0) == 0 {
            return 0;
        }
        if self.terminate() == 1 {
            return 25;
        }
        let mut t = 1 + 12 * self.decision(3 + 3);
        let chroma = self.decision(3 + 4);
        if chroma == 1 {
            t += 4 + 4 * self.decision(3 + 5);
            t += 2 * self.decision(3 + 6);
            t += self.decision(3 + 7);
        } else {
            t += 2 * self.decision(3 + 6);
            t += self.decision(3 + 7);
        }
        t
    }

    /// Intra mb_type suffix in P/SP (`base` = 17) or B (`base` = 32) slices.
    pub fn mb_type_intra_suffix(&mut self, base: usize) -> u32 {
        if self.decision(base) == 0 {
            return 0;
        }
        if self.terminate() == 1 {
            return 25;
        }
        let mut t = 1 + 12 * self.decision(base + 1);
        if self.decision(base + 2) == 1 {
            t += 4 + 4 * self.decision(base + 2);
        }
        t += 2 * self.decision(base + 3);
        t += self.decision(base + 3);
        t
    }

    /// mb_type for P/SP slices. Returns 0..=4 for P types (4 never: P_8x8ref0 is not
    /// CABAC coded) or 5 + I type.
    pub fn mb_type_p(&mut self) -> u32 {
        if self.decision(14) == 0 {
            if self.decision(15) == 0 {
                3 * self.decision(16)
            } else if self.decision(17) == 0 {
                2
            } else {
                1
            }
        } else {
            5 + self.mb_type_intra_suffix(17)
        }
    }

    /// mb_type for B slices: 0..=22 B types, or 23 + I type.
    pub fn mb_type_b(&mut self, ctx_inc0: usize) -> u32 {
        if self.decision(27 + ctx_inc0) == 0 {
            return 0;
        }
        if self.decision(27 + 3) == 0 {
            return 1 + self.decision(27 + 5);
        }
        let mut bits = self.decision(27 + 4) << 3;
        bits |= self.decision(27 + 5) << 2;
        bits |= self.decision(27 + 5) << 1;
        bits |= self.decision(27 + 5);
        if bits < 8 {
            return bits + 3;
        }
        match bits {
            13 => 23 + self.mb_type_intra_suffix(32),
            14 => 11,
            15 => 22,
            _ => {
                let b = (bits << 1) | self.decision(27 + 5);
                b - 4
            }
        }
    }

    /// sub_mb_type in P/SP slices (0..=3).
    pub fn sub_mb_type_p(&mut self) -> u32 {
        if self.decision(21) == 1 {
            return 0;
        }
        if self.decision(22) == 0 {
            return 1;
        }
        if self.decision(23) == 1 { 2 } else { 3 }
    }

    /// sub_mb_type in B slices (0..=12).
    pub fn sub_mb_type_b(&mut self) -> u32 {
        if self.decision(36) == 0 {
            return 0;
        }
        if self.decision(37) == 0 {
            return 1 + self.decision(39);
        }
        let mut t = 3;
        if self.decision(38) == 1 {
            if self.decision(39) == 1 {
                return 11 + self.decision(39);
            }
            t += 4;
        }
        t += 2 * self.decision(39);
        t += self.decision(39);
        t
    }

    /// mb_skip_flag (P: 11, B: 24).
    pub fn mb_skip(&mut self, b_slice: bool, ctx_inc: usize) -> bool {
        self.decision(if b_slice { 24 } else { 11 } + ctx_inc) == 1
    }

    /// ref_idx_lX (unary, ctxIdxOffset 54).
    pub fn ref_idx(&mut self, ctx_inc0: usize) -> u32 {
        if self.decision(54 + ctx_inc0) == 0 {
            return 0;
        }
        let mut v = 1;
        if self.decision(54 + 4) == 0 {
            return v;
        }
        v += 1;
        while self.decision(54 + 5) == 1 {
            v += 1;
            if v > 32 {
                break;
            }
        }
        v
    }

    /// mvd_lX component: `base` = 40 (horizontal) or 47 (vertical); `amvd` = sum of the
    /// absolute neighbouring mvd components.
    pub fn mvd(&mut self, base: usize, amvd: u32) -> i32 {
        let inc = if amvd < 3 {
            0
        } else if amvd > 32 {
            2
        } else {
            1
        };
        if self.decision(base + inc) == 0 {
            return 0;
        }
        let mut v: i32 = 1;
        let mut ctx = base + 3;
        while v < 9 && self.decision(ctx) == 1 {
            if v < 4 {
                ctx += 1;
            }
            v += 1;
        }
        if v >= 9 {
            // UEG3 suffix.
            let mut k = 3;
            while self.bypass() == 1 {
                v = v.saturating_add(1 << k);
                k += 1;
                if k > 24 {
                    break;
                }
            }
            while k > 0 {
                k -= 1;
                v = v.saturating_add((self.bypass() << k) as i32);
            }
        }
        if self.bypass() == 1 { -v } else { v }
    }

    /// mb_qp_delta (ctxIdxOffset 60). Returns the signed value.
    pub fn mb_qp_delta(&mut self, ctx_inc0: usize) -> i32 {
        if self.decision(60 + ctx_inc0) == 0 {
            return 0;
        }
        let mut k: i32 = 1;
        let mut ctx = 60 + 2;
        while self.decision(ctx) == 1 {
            ctx = 60 + 3;
            k += 1;
            if k > 200 {
                break;
            }
        }
        if k & 1 == 1 { (k + 1) / 2 } else { -(k / 2) }
    }

    /// intra_chroma_pred_mode (ctxIdxOffset 64), TU cMax = 3.
    pub fn intra_chroma_pred_mode(&mut self, ctx_inc0: usize) -> u32 {
        if self.decision(64 + ctx_inc0) == 0 {
            return 0;
        }
        if self.decision(64 + 3) == 0 {
            return 1;
        }
        if self.decision(64 + 3) == 0 { 2 } else { 3 }
    }

    /// prev_intra4x4_pred_mode_flag / rem_intra4x4_pred_mode: returns None when the flag
    /// is set (use predicted mode) or Some(rem).
    pub fn intra_pred_mode(&mut self) -> Option<u32> {
        if self.decision(68) == 1 {
            return None;
        }
        let mut v = self.decision(69);
        v |= self.decision(69) << 1;
        v |= self.decision(69) << 2;
        Some(v)
    }

    /// transform_size_8x8_flag (399).
    pub fn transform_8x8(&mut self, ctx_inc: usize) -> bool {
        self.decision(399 + ctx_inc) == 1
    }

    /// coded_block_flag for ctxBlockCat 0..=4.
    pub fn coded_block_flag(&mut self, cat: usize, ctx_inc: usize) -> bool {
        self.decision(CTX_CODED_BLOCK_FLAG + CBF_OFF[cat] + ctx_inc) == 1
    }

    /// Significance map and levels of one block (7.3.5.3.3, 9.3.3.1.3) whose
    /// coded_block_flag is 1. `out[k]` receives the level at scan position k.
    /// `cat` 0..=5, `max_coeff` = number of coefficients (4, 15, 16 or 64); `field` selects
    /// the contexts of field coded macroblocks. Returns the number of non-zero coefficients.
    #[cfg(test)]
    pub fn residual_block(&mut self, cat: usize, max_coeff: usize, field: bool, out: &mut [i32]) -> usize {
        let mut pos = [0u8; 64];
        let mut lvl = [0i32; 64];
        let n = self.residual_block_sparse(cat, max_coeff, field, &mut pos, &mut lvl);
        for v in out.iter_mut().take(max_coeff) {
            *v = 0;
        }
        for k in 0..n {
            out[pos[k] as usize] = lvl[k];
        }
        n
    }

    /// As `residual_block`, but returns the `n` non-zero coefficients as (scan position
    /// `pos[k]`, level `lvl[k]`) pairs (in decoding order, i.e. descending position).
    pub fn residual_block_sparse(
        &mut self,
        cat: usize,
        max_coeff: usize,
        field: bool,
        pos: &mut [u8; 64],
        lvl: &mut [i32; 64],
    ) -> usize {
        let t = cat * 2 + field as usize;
        let end = max_coeff.clamp(1, 64) - 1;
        let sig = &SIG_CTX[t][..end];
        let last = &LAST_CTX[t][..end];
        let abs_base = ABS_BASE[cat];
        let mut n = 0usize;
        let mut ended = false;
        for (i, (&sc, &lc)) in sig.iter().zip(last).enumerate() {
            if self.decision(sc as usize) == 1 {
                pos[n] = i as u8;
                n += 1;
                if self.decision(lc as usize) == 1 {
                    ended = true;
                    break;
                }
            }
        }
        if !ended {
            pos[n] = end as u8;
            n += 1;
        }
        let gt1_max = if cat == 3 { 3 } else { 4 };
        let mut num_gt1 = 0usize;
        let mut num_eq1 = 0usize;
        for k in (0..n).rev() {
            let inc0 = if num_gt1 != 0 { 0 } else { (1 + num_eq1).min(4) };
            let mut abs: i32;
            if self.decision(abs_base + inc0) == 0 {
                abs = 1;
            } else {
                let ctx = abs_base + 5 + num_gt1.min(gt1_max);
                let mut prefix = 1;
                while prefix < 14 && self.decision(ctx) == 1 {
                    prefix += 1;
                }
                abs = prefix + 1;
                if prefix >= 14 {
                    // Exp-Golomb (k = 0) suffix.
                    let mut kk = 0u32;
                    let mut suf: i32 = 0;
                    while self.bypass() == 1 {
                        suf = suf.saturating_add(1 << kk);
                        kk += 1;
                        if kk > 24 {
                            break;
                        }
                    }
                    while kk > 0 {
                        kk -= 1;
                        suf = suf.saturating_add((self.bypass() << kk) as i32);
                    }
                    abs = abs.saturating_add(suf).min(1 << 20);
                }
            }
            if abs == 1 {
                num_eq1 += 1;
            } else {
                num_gt1 += 1;
            }
            lvl[k] = if self.bypass() == 1 { -abs } else { abs };
        }
        n
    }
}

/// significant_coeff_flag / last_significant_coeff_flag ctxIdx per scan position, for
/// [ctxBlockCat * 2 + field].
static SIG_CTX: [[u16; 64]; 12] = coeff_ctx_tables(true);
static LAST_CTX: [[u16; 64]; 12] = coeff_ctx_tables(false);
/// coeff_abs_level_minus1 ctxIdx base per ctxBlockCat.
const ABS_BASE: [usize; 6] = [227, 227 + ABS_OFF[1], 227 + ABS_OFF[2], 227 + ABS_OFF[3], 227 + ABS_OFF[4], 426];

const fn coeff_ctx_tables(sig: bool) -> [[u16; 64]; 12] {
    let mut t = [[0u16; 64]; 12];
    let mut cat = 0;
    while cat < 6 {
        let mut field = 0;
        while field < 2 {
            let base = match (cat, field, sig) {
                (5, 0, true) => 402,
                (5, 1, true) => 436,
                (5, 0, false) => 417,
                (5, _, false) => 451,
                (_, 0, true) => 105 + SIG_OFF[cat],
                (_, _, true) => 277 + SIG_OFF[cat],
                (_, 0, false) => 166 + SIG_OFF[cat],
                (_, _, false) => 338 + SIG_OFF[cat],
            };
            let mut i = 0;
            while i < 63 {
                let inc = match cat {
                    5 => {
                        if !sig {
                            LAST8[i] as usize
                        } else if field == 1 {
                            SIG8_FIELD[i] as usize
                        } else {
                            SIG8_FRAME[i] as usize
                        }
                    }
                    3 => {
                        if i < 2 {
                            i
                        } else {
                            2
                        }
                    }
                    _ => i,
                };
                t[cat * 2 + field][i] = (base + inc) as u16;
                i += 1;
            }
            field += 1;
        }
        cat += 1;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal CABAC encoder (9.3.4.2) used to produce test streams.
    struct Enc {
        low: u32,
        range: u32,
        outstanding: u32,
        first: bool,
        bits: Vec<bool>,
        ctx: [u8; 1024],
    }

    impl Enc {
        fn new(ctx: [u8; 1024]) -> Enc {
            Enc { low: 0, range: 510, outstanding: 0, first: true, bits: Vec::new(), ctx }
        }
        fn put_bit(&mut self, b: bool) {
            if self.first {
                self.first = false;
            } else {
                self.bits.push(b);
            }
            while self.outstanding > 0 {
                self.bits.push(!b);
                self.outstanding -= 1;
            }
        }
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
        fn bypass(&mut self, bin: u32) {
            self.low <<= 1;
            if bin == 1 {
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
            if bin == 1 {
                self.low += self.range;
                // flush
                self.range = 2;
                self.renorm();
                self.put_bit((self.low >> 9) & 1 == 1);
                let v = ((self.low >> 7) & 3) | 1;
                self.bits.push(v & 2 != 0);
                self.bits.push(v & 1 != 0);
            } else {
                self.renorm();
            }
        }
        fn bytes(&self) -> Vec<u8> {
            let mut out = vec![0u8; self.bits.len().div_ceil(8) + 2];
            for (i, &b) in self.bits.iter().enumerate() {
                if b {
                    out[i / 8] |= 0x80 >> (i % 8);
                }
            }
            out
        }
    }

    fn init_ctx(qp: i32, init: usize) -> [u8; 1024] {
        let dummy = [0u8; 4];
        Cabac::new(&dummy, 0, qp, init).unwrap().ctx
    }

    #[test]
    fn engine_roundtrip_random() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            seed >> 16
        };
        let ctx0 = init_ctx(26, 0);
        let mut enc = Enc::new(ctx0);
        let mut ops = Vec::new();
        for _ in 0..5000 {
            let kind = rnd() % 10;
            let ctx = (rnd() % 400) as usize;
            // Skewed bins exercise MPS/LPS paths.
            let bin = if rnd() % 4 == 0 { 1 } else { 0 };
            if kind < 7 {
                enc.decision(ctx, bin);
                ops.push((0, ctx, bin));
            } else {
                enc.bypass(bin);
                ops.push((1, 0, bin));
            }
        }
        enc.terminate(1);
        ops.push((2, 0, 1));
        let data = enc.bytes();
        let mut dec = Cabac::new(&data, 0, 26, 0).unwrap();
        for (i, &(k, ctx, bin)) in ops.iter().enumerate() {
            let got = match k {
                0 => dec.decision(ctx),
                1 => dec.bypass(),
                _ => dec.terminate(),
            };
            assert_eq!(got, bin, "op {i}");
        }
    }

    #[test]
    fn residual_block_roundtrip() {
        // Encode a 4x4 block (cat 2) with the standard binarisation and decode it back.
        let coeffs: [i32; 16] = [7, -3, 0, 1, 0, 0, -1, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        let cat = 2usize;
        let ctx0 = init_ctx(30, 1);
        let mut enc = Enc::new(ctx0);
        let sig_base = 105 + SIG_OFF[cat];
        let last_base = 166 + SIG_OFF[cat];
        let abs_base = 227 + ABS_OFF[cat];
        let last = coeffs.iter().rposition(|&c| c != 0).unwrap();
        for i in 0..15 {
            let s = (coeffs[i] != 0) as u32;
            enc.decision(sig_base + i, s);
            if s == 1 {
                enc.decision(last_base + i, (i == last) as u32);
                if i == last {
                    break;
                }
            }
        }
        let (mut gt1, mut eq1) = (0usize, 0usize);
        for i in (0..=last).rev() {
            let c = coeffs[i];
            if c == 0 {
                continue;
            }
            let a = c.unsigned_abs() - 1;
            let inc0 = if gt1 != 0 { 0 } else { (1 + eq1).min(4) };
            enc.decision(abs_base + inc0, (a > 0) as u32);
            if a > 0 {
                let ctx = abs_base + 5 + gt1.min(4);
                for _ in 1..a.min(14) {
                    enc.decision(ctx, 1);
                }
                if a < 14 {
                    enc.decision(ctx, 0);
                }
                gt1 += 1;
            } else {
                eq1 += 1;
            }
            enc.bypass((c < 0) as u32);
        }
        enc.terminate(1);
        let data = enc.bytes();
        let mut dec = Cabac::new(&data, 0, 30, 1).unwrap();
        let mut out = [0i32; 16];
        let n = dec.residual_block(cat, 16, false, &mut out);
        assert_eq!(n, 5);
        assert_eq!(out, coeffs);
        assert_eq!(dec.terminate(), 1);
    }
}
