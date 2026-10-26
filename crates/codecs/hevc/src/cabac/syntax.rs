// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Binarisations and context selection for all CABAC-coded syntax elements
//! (9.3.3, 9.3.4.2), including residual_coding() (7.3.8.11).

use super::tables::*;
use super::Cabac;
use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Scan orders (6.5.3 - 6.5.5)
// ---------------------------------------------------------------------------

const fn gen_scan(blk: usize, scan: usize) -> [(u8, u8); 64] {
    let mut out = [(0u8, 0u8); 64];
    let n = blk * blk;
    match scan {
        0 => {
            let mut i = 0usize;
            let mut x: isize = 0;
            let mut y: isize = 0;
            while i < n {
                while y >= 0 {
                    if (x as usize) < blk && (y as usize) < blk {
                        out[i] = (x as u8, y as u8);
                        i += 1;
                    }
                    y -= 1;
                    x += 1;
                }
                y = x;
                x = 0;
            }
        }
        1 => {
            let mut i = 0;
            while i < n {
                out[i] = ((i % blk) as u8, (i / blk) as u8);
                i += 1;
            }
        }
        _ => {
            let mut i = 0;
            while i < n {
                out[i] = ((i / blk) as u8, (i % blk) as u8);
                i += 1;
            }
        }
    }
    out
}

/// Scan positions inside a 4x4 sub-block: SCAN_4X4[scanIdx][n] = (x, y).
pub static SCAN_4X4: [[(u8, u8); 64]; 3] = [gen_scan(4, 0), gen_scan(4, 1), gen_scan(4, 2)];

/// Sub-block scans: SCAN_SUB[log2(blocks per side)][scanIdx][i] = (xS, yS).
pub static SCAN_SUB: [[[(u8, u8); 64]; 3]; 4] = [
    [gen_scan(1, 0), gen_scan(1, 1), gen_scan(1, 2)],
    [gen_scan(2, 0), gen_scan(2, 1), gen_scan(2, 2)],
    [gen_scan(4, 0), gen_scan(4, 1), gen_scan(4, 2)],
    [gen_scan(8, 0), gen_scan(8, 1), gen_scan(8, 2)],
];

const CTX_IDX_MAP: [u8; 16] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8, 8];

/// sigCtx pattern inside a sub-block for prevCsbf = 0..3, indexed by (yP << 2) + xP.
const SIG_PATTERN: [[u8; 16]; 4] = {
    let mut t = [[0u8; 16]; 4];
    let mut p = 0;
    while p < 16 {
        let xp = p & 3;
        let yp = p >> 2;
        t[0][p] = if xp + yp == 0 { 2 } else if xp + yp < 3 { 1 } else { 0 };
        t[1][p] = if yp == 0 { 2 } else if yp == 1 { 1 } else { 0 };
        t[2][p] = if xp == 0 { 2 } else if xp == 1 { 1 } else { 0 };
        t[3][p] = 2;
        p += 1;
    }
    t
};

/// Result of residual_coding().
#[derive(Clone, Copy, Debug)]
pub struct ResidualInfo {
    pub transform_skip: bool,
    /// Bounding box (inclusive) of the non-zero coefficients.
    pub max_x: usize,
    pub max_y: usize,
}

pub const PRED_L0: u8 = 0;
pub const PRED_L1: u8 = 1;
pub const PRED_BI: u8 = 2;

/// Partition modes (Table 7-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartMode {
    P2Nx2N,
    P2NxN,
    PNx2N,
    PNxN,
    P2NxnU,
    P2NxnD,
    PnLx2N,
    PnRx2N,
}

impl<'a> Cabac<'a> {
    // ---------------- SAO ----------------
    pub fn sao_merge_flag(&mut self) -> bool {
        self.bin(SAO_MERGE_FLAG) != 0
    }
    pub fn sao_type_idx(&mut self) -> u8 {
        if self.bin(SAO_TYPE_IDX) == 0 {
            0
        } else if self.bypass() == 0 {
            1
        } else {
            2
        }
    }
    pub fn sao_offset_abs(&mut self, cmax: u32) -> i32 {
        let mut v = 0;
        while v < cmax && self.bypass() != 0 {
            v += 1;
        }
        v as i32
    }
    pub fn sao_band_position(&mut self) -> u8 {
        self.bypass_bits(5) as u8
    }
    pub fn sao_eo_class(&mut self) -> u8 {
        self.bypass_bits(2) as u8
    }

    // ---------------- coding quadtree / CU ----------------
    pub fn split_cu_flag(&mut self, ctx_inc: usize) -> bool {
        self.bin(SPLIT_CU_FLAG + ctx_inc) != 0
    }
    pub fn cu_transquant_bypass_flag(&mut self) -> bool {
        self.bin(CU_TRANSQUANT_BYPASS_FLAG) != 0
    }
    pub fn cu_skip_flag(&mut self, ctx_inc: usize) -> bool {
        self.bin(CU_SKIP_FLAG + ctx_inc) != 0
    }
    /// Returns true for MODE_INTRA.
    pub fn pred_mode_flag(&mut self) -> bool {
        self.bin(PRED_MODE_FLAG) != 0
    }
    pub fn part_mode(&mut self, intra: bool, log2_cb: u8, log2_min_cb: u8, amp: bool) -> PartMode {
        if self.bin(PART_MODE) != 0 {
            return PartMode::P2Nx2N;
        }
        if intra {
            return PartMode::PNxN;
        }
        if log2_cb == log2_min_cb {
            if self.bin(PART_MODE + 1) != 0 {
                return PartMode::P2NxN;
            }
            if log2_cb == 3 {
                return PartMode::PNx2N;
            }
            if self.bin(PART_MODE + 2) != 0 {
                return PartMode::PNx2N;
            }
            return PartMode::PNxN;
        }
        if !amp {
            if self.bin(PART_MODE + 1) != 0 {
                return PartMode::P2NxN;
            }
            return PartMode::PNx2N;
        }
        if self.bin(PART_MODE + 1) != 0 {
            if self.bin(PART_MODE + 3) != 0 {
                return PartMode::P2NxN;
            }
            if self.bypass() != 0 {
                return PartMode::P2NxnD;
            }
            return PartMode::P2NxnU;
        }
        if self.bin(PART_MODE + 3) != 0 {
            return PartMode::PNx2N;
        }
        if self.bypass() != 0 {
            return PartMode::PnRx2N;
        }
        PartMode::PnLx2N
    }
    pub fn prev_intra_luma_pred_flag(&mut self) -> bool {
        self.bin(PREV_INTRA_LUMA_PRED_FLAG) != 0
    }
    pub fn mpm_idx(&mut self) -> u8 {
        if self.bypass() == 0 {
            0
        } else if self.bypass() == 0 {
            1
        } else {
            2
        }
    }
    pub fn rem_intra_luma_pred_mode(&mut self) -> u8 {
        self.bypass_bits(5) as u8
    }
    pub fn intra_chroma_pred_mode(&mut self) -> u8 {
        if self.bin(INTRA_CHROMA_PRED_MODE) == 0 {
            4
        } else {
            self.bypass_bits(2) as u8
        }
    }
    pub fn rqt_root_cbf(&mut self) -> bool {
        self.bin(RQT_ROOT_CBF) != 0
    }

    // ---------------- prediction unit ----------------
    pub fn merge_flag(&mut self) -> bool {
        self.bin(MERGE_FLAG) != 0
    }
    pub fn merge_idx(&mut self, max_num_merge_cand: usize) -> usize {
        if max_num_merge_cand <= 1 {
            return 0;
        }
        if self.bin(MERGE_IDX) == 0 {
            return 0;
        }
        let mut i = 1;
        while i < max_num_merge_cand - 1 && self.bypass() != 0 {
            i += 1;
        }
        i
    }
    pub fn inter_pred_idc(&mut self, npbw: usize, npbh: usize, ct_depth: usize) -> u8 {
        if npbw + npbh != 12 && self.bin(INTER_PRED_IDC + ct_depth.min(3)) != 0 {
            return PRED_BI;
        }
        if self.bin(INTER_PRED_IDC + 4) != 0 {
            PRED_L1
        } else {
            PRED_L0
        }
    }
    pub fn ref_idx(&mut self, num_ref: usize) -> usize {
        let cmax = num_ref.saturating_sub(1);
        let mut i = 0;
        while i < cmax {
            let b = if i < 2 { self.bin(REF_IDX + i) } else { self.bypass() };
            if b == 0 {
                break;
            }
            i += 1;
        }
        i
    }
    pub fn mvp_flag(&mut self) -> usize {
        self.bin(MVP_FLAG) as usize
    }
    fn exp_golomb_bypass(&mut self, mut k: u32) -> Result<u32> {
        let mut v: u32 = 0;
        while self.bypass() != 0 {
            v = v.wrapping_add(1 << k);
            k += 1;
            if k > 30 {
                return Err(Error::Invalid("exp-golomb bypass code too long"));
            }
        }
        Ok(v.wrapping_add(self.bypass_bits(k)))
    }
    pub fn mvd_coding(&mut self) -> Result<[i32; 2]> {
        let g0x = self.bin(ABS_MVD_GREATER0) != 0;
        let g0y = self.bin(ABS_MVD_GREATER0) != 0;
        let g1x = g0x && self.bin(ABS_MVD_GREATER1) != 0;
        let g1y = g0y && self.bin(ABS_MVD_GREATER1) != 0;
        let mut mvd = [0i32; 2];
        for (c, (g0, g1)) in [(g0x, g1x), (g0y, g1y)].into_iter().enumerate() {
            if g0 {
                let mut abs = 1i32;
                if g1 {
                    let v = self.exp_golomb_bypass(1)?;
                    if v > (1 << 15) {
                        return Err(Error::Invalid("abs_mvd_minus2"));
                    }
                    abs = v as i32 + 2;
                }
                mvd[c] = if self.bypass() != 0 { -abs } else { abs };
            }
        }
        Ok(mvd)
    }

    // ---------------- transform tree ----------------
    pub fn split_transform_flag(&mut self, log2_size: u8) -> bool {
        self.bin(SPLIT_TRANSFORM_FLAG + (5 - log2_size as usize).min(2)) != 0
    }
    pub fn cbf_luma(&mut self, trafo_depth: usize) -> bool {
        self.bin(CBF_LUMA + if trafo_depth == 0 { 1 } else { 0 }) != 0
    }
    pub fn cbf_chroma(&mut self, trafo_depth: usize) -> bool {
        self.bin(CBF_CHROMA + trafo_depth.min(4)) != 0
    }
    pub fn cu_qp_delta(&mut self) -> Result<i32> {
        let mut prefix = 0u32;
        if self.bin(CU_QP_DELTA_ABS) != 0 {
            prefix = 1;
            while prefix < 5 && self.bin(CU_QP_DELTA_ABS + 1) != 0 {
                prefix += 1;
            }
        }
        let mut abs = prefix;
        if prefix >= 5 {
            let s = self.exp_golomb_bypass(0)?;
            if s > 1000 {
                return Err(Error::Invalid("cu_qp_delta_abs"));
            }
            abs += s;
        }
        if abs == 0 {
            return Ok(0);
        }
        let sign = self.bypass();
        Ok(if sign != 0 { -(abs as i32) } else { abs as i32 })
    }
    fn coeff_abs_level_remaining(&mut self, rice: u32) -> Result<u32> {
        let mut prefix = 0u32;
        while self.bypass() != 0 {
            prefix += 1;
            if prefix > 28 {
                return Err(Error::Invalid("coeff_abs_level_remaining prefix too long"));
            }
        }
        if prefix <= 3 {
            Ok((prefix << rice) + self.bypass_bits(rice))
        } else {
            let nb = prefix - 3 + rice;
            let base = ((1u32 << (prefix - 3)) + 2) << rice;
            Ok(base + self.bypass_bits(nb))
        }
    }

    /// residual_coding(): decodes TransCoeffLevel into `coeffs` (raster order,
    /// `coeffs[y * n + x]`). The n*n area of `coeffs` must be zero on entry;
    /// the raster positions of all non-zero coefficients are appended to
    /// `nz` (capacity >= n*n) so that the caller can clear them afterwards.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    pub fn residual_coding(
        &mut self,
        coeffs: &mut [i32],
        nz: &mut Vec<u16>,
        log2_size: u32,
        c_idx: usize,
        scan_idx: usize,
        transform_skip_allowed: bool,
        sign_hiding: bool,
    ) -> Result<ResidualInfo> {
        let n = 1usize << log2_size;
        let mut info = ResidualInfo { transform_skip: false, max_x: 0, max_y: 0 };
        if transform_skip_allowed {
            info.transform_skip = self.bin(TRANSFORM_SKIP_FLAG + if c_idx > 0 { 1 } else { 0 }) != 0;
        }
        // last_sig_coeff_x/y_prefix
        let (ctx_off, ctx_shift) = if c_idx == 0 {
            (3 * (log2_size - 2) + ((log2_size - 1) >> 2), (log2_size + 1) >> 2)
        } else {
            (15, log2_size - 2)
        };
        let cmax = (log2_size << 1) - 1;
        let mut px = 0u32;
        while px < cmax && self.bin(LAST_X_PREFIX + (ctx_off + (px >> ctx_shift)) as usize) != 0 {
            px += 1;
        }
        let mut py = 0u32;
        while py < cmax && self.bin(LAST_Y_PREFIX + (ctx_off + (py >> ctx_shift)) as usize) != 0 {
            py += 1;
        }
        let mut last_x = px;
        if px > 3 {
            let nb = (px >> 1) - 1;
            last_x = (1 << nb) * (2 + (px & 1)) + self.bypass_bits(nb);
        }
        let mut last_y = py;
        if py > 3 {
            let nb = (py >> 1) - 1;
            last_y = (1 << nb) * (2 + (py & 1)) + self.bypass_bits(nb);
        }
        if scan_idx == 2 {
            std::mem::swap(&mut last_x, &mut last_y);
        }
        let (last_x, last_y) = (last_x as usize, last_y as usize);
        if last_x >= n || last_y >= n {
            return Err(Error::Invalid("last significant coefficient position"));
        }

        let log2_sb = (log2_size - 2) as usize;
        let sb_w = 1usize << log2_sb;
        let sb_scan = &SCAN_SUB[log2_sb][scan_idx];
        let pos_scan = &SCAN_4X4[scan_idx];
        let (xs_last, ys_last) = ((last_x >> 2) as u8, (last_y >> 2) as u8);
        let mut last_sub_block = 0usize;
        for i in 0..sb_w * sb_w {
            if sb_scan[i] == (xs_last, ys_last) {
                last_sub_block = i;
                break;
            }
        }
        let (xp_last, yp_last) = ((last_x & 3) as u8, (last_y & 3) as u8);
        let mut last_scan_pos = 0usize;
        for i in 0..16 {
            if pos_scan[i] == (xp_last, yp_last) {
                last_scan_pos = i;
                break;
            }
        }

        let mut csbf = [0u8; 64];
        let mut c1_prev: u32 = 1;
        let mut first_sb_processed = true;
        let gt1_base = GREATER1_FLAG + if c_idx > 0 { 16 } else { 0 };
        let gt2_base = GREATER2_FLAG + if c_idx > 0 { 4 } else { 0 };
        let sig_base = SIG_COEFF_FLAG + if c_idx > 0 { 27 } else { 0 };

        for i in (0..=last_sub_block).rev() {
            let (xs, ys) = (sb_scan[i].0 as usize, sb_scan[i].1 as usize);
            let right = if xs + 1 < sb_w { csbf[ys * 8 + xs + 1] } else { 0 };
            let below = if ys + 1 < sb_w { csbf[(ys + 1) * 8 + xs] } else { 0 };
            let mut infer_sb_dc = false;
            let coded = if i < last_sub_block && i > 0 {
                let ctx_inc = (right | below).min(1) as usize + if c_idx > 0 { 2 } else { 0 };
                infer_sb_dc = true;
                self.bin(CODED_SUB_BLOCK_FLAG + ctx_inc) != 0
            } else {
                true
            };
            csbf[ys * 8 + xs] = coded as u8;

            // significant coefficients, in decreasing scan position order
            let mut sig_pos = [0u8; 16];
            let mut num_sig = 0usize;
            let start: isize;
            if i == last_sub_block {
                sig_pos[0] = last_scan_pos as u8;
                num_sig = 1;
                start = last_scan_pos as isize - 1;
            } else {
                start = 15;
            }
            if coded {
                let prev_csbf = (right as usize) | ((below as usize) << 1);
                // context offset for this sub-block (9.3.4.2.5)
                let off = if log2_size == 2 {
                    0
                } else if c_idx == 0 {
                    (if xs + ys > 0 { 3 } else { 0 }) + if log2_size == 3 { if scan_idx == 0 { 9 } else { 15 } } else { 21 }
                } else if log2_size == 3 {
                    9
                } else {
                    12
                };
                let pattern = &SIG_PATTERN[prev_csbf];
                let first_sb = xs + ys == 0;
                let mut nn = start;
                while nn >= 0 {
                    let np = nn as usize;
                    let (xp, yp) = (pos_scan[np].0 as usize, pos_scan[np].1 as usize);
                    if np > 0 || !infer_sb_dc {
                        let pos = (yp << 2) + xp;
                        let sig_ctx = if log2_size == 2 {
                            CTX_IDX_MAP[pos] as usize
                        } else if first_sb && pos == 0 {
                            0
                        } else {
                            pattern[pos] as usize + off
                        };
                        if self.bin(sig_base + sig_ctx) != 0 {
                            sig_pos[num_sig] = np as u8;
                            num_sig += 1;
                            infer_sb_dc = false;
                        }
                    } else {
                        // DC of a coded sub-block with no other significant coefficient
                        sig_pos[num_sig] = 0;
                        num_sig += 1;
                    }
                    nn -= 1;
                }
            }
            if num_sig == 0 {
                continue;
            }

            // greater1 / greater2 flags
            let mut ctx_set = if i == 0 || c_idx > 0 { 0 } else { 2 };
            if !first_sb_processed && c1_prev == 0 {
                ctx_set += 1;
            }
            first_sb_processed = false;
            let mut c1: u32 = 1;
            let mut gt1 = [0u8; 16];
            let mut first_g1: Option<usize> = None;
            for k in 0..num_sig.min(8) {
                let ctx = ctx_set * 4 + c1.min(3) as usize;
                let f = self.bin(gt1_base + ctx);
                gt1[k] = f as u8;
                if f != 0 {
                    c1 = 0;
                    if first_g1.is_none() {
                        first_g1 = Some(k);
                    }
                } else if c1 > 0 && c1 < 3 {
                    c1 += 1;
                }
            }
            c1_prev = c1;
            let mut gt2_at_first = 0u8;
            if first_g1.is_some() {
                gt2_at_first = self.bin(gt2_base + ctx_set) as u8;
            }
            let last_sig_n = sig_pos[0] as isize;
            let first_sig_n = sig_pos[num_sig - 1] as isize;
            let sign_hidden = sign_hiding && (last_sig_n - first_sig_n > 3);
            let num_signs = if sign_hidden { num_sig - 1 } else { num_sig };
            let signs = self.bypass_bits(num_signs as u32) << (32 - num_signs as u32).min(31);
            let signs = if num_signs == 0 { 0 } else { signs };
            let mut sum_abs: u32 = 0;
            let mut rice: u32 = 0;
            for k in 0..num_sig {
                let mut base = 1 + gt1[k] as u32;
                if Some(k) == first_g1 {
                    base += gt2_at_first as u32;
                }
                let threshold = if k < 8 {
                    if Some(k) == first_g1 {
                        3
                    } else {
                        2
                    }
                } else {
                    1
                };
                let mut abs = base;
                if base == threshold {
                    let rem = self.coeff_abs_level_remaining(rice)?;
                    abs = base.saturating_add(rem);
                    if abs > 3 * (1 << rice) {
                        rice = (rice + 1).min(4);
                    }
                }
                let abs = abs.min(1 << 22) as i32;
                let np = sig_pos[k] as usize;
                let (xp, yp) = (pos_scan[np].0 as usize, pos_scan[np].1 as usize);
                let xc = (xs << 2) + xp;
                let yc = (ys << 2) + yp;
                let mut v = abs;
                if k < num_signs {
                    if (signs >> (31 - k)) & 1 != 0 {
                        v = -abs;
                    }
                }
                if sign_hidden {
                    sum_abs = sum_abs.wrapping_add(abs as u32);
                    if k == num_sig - 1 && sum_abs % 2 == 1 {
                        v = -v;
                    }
                }
                coeffs[yc * n + xc] = v;
                nz.push((yc * n + xc) as u16);
                info.max_x = info.max_x.max(xc);
                info.max_y = info.max_y.max(yc);
            }
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans() {
        assert_eq!(&SCAN_4X4[0][..4], &[(0, 0), (0, 1), (1, 0), (0, 2)]);
        assert_eq!(&SCAN_4X4[1][..5], &[(0, 0), (1, 0), (2, 0), (3, 0), (0, 1)]);
        assert_eq!(&SCAN_4X4[2][..5], &[(0, 0), (0, 1), (0, 2), (0, 3), (1, 0)]);
        assert_eq!(&SCAN_SUB[1][1][..4], &[(0, 0), (1, 0), (0, 1), (1, 1)]);
        assert_eq!(SCAN_SUB[3][0][63], (7, 7));
    }
}
