// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M4: above/left context arrays (partition, skip, modes, coefficients) and the context
//! derivations of spec 8.3.2. Everything here mirrors `studio_av1`'s tile decoder, so a symbol is
//! written with exactly the CDF the decoder will pick for it.
//!
//! One tile, 4:2:0, intra frames only: neighbours are available whenever they lie inside the
//! frame, and the mode info of earlier blocks is kept in per-4x4 grids.

use studio_av1::frame::{MiInfo, Mv};
use studio_av1::spec_tables::*;

/// Chroma subsampling of the supported streams (profile 0 is always 4:2:0).
pub const SS: usize = 1;

/// Zero padding (columns and rows) of the coefficient level buffer.
pub const TX_PAD: usize = 4;

/// Slack the per-plane above/left arrays carry past the frame edge (a block can reach past it).
const ARRAY_SLACK: usize = 64;

pub fn block_width(bs: usize) -> usize {
    4 * NUM_4X4_BLOCKS_WIDE[bs] as usize
}

pub fn block_height(bs: usize) -> usize {
    4 * NUM_4X4_BLOCKS_HIGH[bs] as usize
}

/// The size of the chroma block of a luma block of size `bsize` (`get_plane_residual_size`).
pub fn plane_residual_size(bsize: usize, plane: usize) -> usize {
    let s = if plane > 0 { SS } else { 0 };
    SUBSAMPLED_SIZE[bsize][s][s] as usize
}

/// The block size with `w4` x `h4` units, `None` if AV1 has no such block.
pub fn block_size_of(w4: usize, h4: usize) -> Option<usize> {
    (0..BLOCK_SIZES).find(|&b| NUM_4X4_BLOCKS_WIDE[b] as usize == w4 && NUM_4X4_BLOCKS_HIGH[b] as usize == h4)
}

/// The transform size with the given sample dimensions.
pub fn find_tx_size(w: usize, h: usize) -> usize {
    (0..TX_SIZES_ALL).find(|&t| TX_WIDTH[t] as usize == w && TX_HEIGHT[t] as usize == h).unwrap_or(TX_4X4)
}

pub fn get_tx_class(t: usize) -> usize {
    if t == V_DCT || t == V_ADST || t == V_FLIPADST {
        TX_CLASS_VERT
    } else if t == H_DCT || t == H_ADST || t == H_FLIPADST {
        TX_CLASS_HORIZ
    } else {
        TX_CLASS_2D
    }
}

/// Coefficient base context from the neighbourhood magnitude `mag` (`get_coeff_base_ctx`).
pub fn coeff_base_ctx(tx_sz: usize, tx_class: usize, row: usize, col: usize, mag: u32) -> usize {
    let ctx = ((mag + 1) >> 1).min(4) as usize;
    if tx_class == TX_CLASS_2D {
        if row == 0 && col == 0 {
            return 0;
        }
        return ctx + COEFF_BASE_CTX_OFFSET[tx_sz][row.min(4)][col.min(4)] as usize;
    }
    let idx = if tx_class == TX_CLASS_VERT { row } else { col };
    ctx + COEFF_BASE_POS_CTX_OFFSET[idx.min(2)] as usize
}

/// `coeff_br` context from the neighbourhood magnitude `mag`.
pub fn coeff_br_ctx(tx_class: usize, row: usize, col: usize, pos: usize, mag: u32) -> usize {
    let mag = ((mag + 1) >> 1).min(6) as usize;
    if pos == 0 {
        mag
    } else if tx_class == TX_CLASS_2D {
        if row < 2 && col < 2 { mag + 7 } else { mag + 14 }
    } else if tx_class == TX_CLASS_HORIZ {
        if col == 0 { mag + 7 } else { mag + 14 }
    } else if row == 0 {
        mag + 7
    } else {
        mag + 14
    }
}

/// `coeff_base_eob` context of scan position `c`.
pub fn coeff_base_eob_ctx(c: usize, bwl: usize, height: usize) -> usize {
    if c == 0 {
        return 0;
    }
    if c <= (height << bwl) / 8 {
        return 1;
    }
    if c <= (height << bwl) / 4 {
        return 2;
    }
    3
}

/// The mode info of one 4x4 unit, as saved and restored by [`ContextState::save_region`].
#[derive(Clone)]
struct MiEntry {
    mi_size: u8,
    skip: bool,
    y_mode: u8,
    tx_size: u8,
    inter_tx_size: u8,
    is_inter: bool,
    ref_frame: [i8; 2],
    mv: [Mv; 2],
    written: bool,
}

/// A saved rectangle of mode info.
pub struct RegionSave {
    r: usize,
    c: usize,
    rows: usize,
    cols: usize,
    data: Vec<MiEntry>,
}

/// The context state of one tile.
/// `cdef_idx` syntax of a frame: the index of every 64x64 block and which ones are coded already.
pub struct CdefSyntax {
    pub bits: u32,
    /// Row-major, one per 64x64 block.
    pub idx: Vec<i8>,
    pub cols: usize,
    written: Vec<std::cell::Cell<bool>>,
}

impl CdefSyntax {
    pub fn new(bits: u32, cols: usize, idx: Vec<i8>) -> CdefSyntax {
        let written = vec![std::cell::Cell::new(false); idx.len()];
        CdefSyntax { bits, idx, cols, written }
    }
}

pub struct ContextState {
    pub mi_rows: usize,
    pub mi_cols: usize,
    /// Mode info of the blocks written so far, in the decoder's own layout: the motion vector
    /// processes read it as they do in the decoder.
    pub mi: MiInfo,
    above_level: [Vec<u8>; 3],
    above_dc: [Vec<u8>; 3],
    left_level: [Vec<u8>; 3],
    left_dc: [Vec<u8>; 3],
    /// `TileIntraFrameYModeCdf`: reset at every tile start, outside the saved CDF set.
    pub intra_y_mode_cdf: [[[u16; 14]; 5]; 5],
    /// `read_cdef()` state; none when the sequence has CDEF off.
    pub cdef: Option<CdefSyntax>,
    /// The tile being written, `[mi_row_start, mi_row_end, mi_col_start, mi_col_end]`: neighbours
    /// outside it are not available. The whole frame by default.
    pub tile: [usize; 4],
}

impl ContextState {
    pub fn new(mi_cols: usize, mi_rows: usize) -> ContextState {
        ContextState {
            mi_rows,
            mi_cols,
            mi: MiInfo::new(mi_cols, mi_rows, false),
            above_level: std::array::from_fn(|_| vec![0; mi_cols + ARRAY_SLACK]),
            above_dc: std::array::from_fn(|_| vec![0; mi_cols + ARRAY_SLACK]),
            left_level: std::array::from_fn(|_| vec![0; mi_rows + ARRAY_SLACK]),
            left_dc: std::array::from_fn(|_| vec![0; mi_rows + ARRAY_SLACK]),
            intra_y_mode_cdf: DEFAULT_INTRA_FRAME_Y_MODE_CDF,
            cdef: None,
            tile: [0, mi_rows, 0, mi_cols],
        }
    }

    /// The block above row `r` belongs to the tile.
    #[inline]
    pub fn avail_u(&self, r: usize) -> bool {
        r > self.tile[0]
    }

    /// The block left of column `c` belongs to the tile.
    #[inline]
    pub fn avail_l(&self, c: usize) -> bool {
        c > self.tile[2]
    }

    /// `read_cdef()`: the index of the 64x64 block, coded with the first block of it that is not
    /// skipped.
    pub fn write_cdef(&self, enc: &mut crate::entropy::SymbolEncoder, r: usize, c: usize, skip: bool) {
        let Some(cd) = &self.cdef else { return };
        if skip || cd.bits == 0 {
            return;
        }
        let slot = (r >> 4) * cd.cols + (c >> 4);
        if !cd.written[slot].get() {
            cd.written[slot].set(true);
            enc.write_literal(cd.idx[slot].max(0) as u32, cd.bits);
        }
    }

    #[inline]
    fn idx(&self, r: usize, c: usize) -> usize {
        self.mi.idx(r, c)
    }

    /// `clear_left_context()`: at the start of every superblock row.
    pub fn clear_left(&mut self) {
        for p in 0..3 {
            self.left_level[p].fill(0);
            self.left_dc[p].fill(0);
        }
    }

    /// The partition context (8.3.2) of the block of log2 width `bsl` (in 4x4 units) at `(r, c)`.
    pub fn partition_ctx(&self, r: usize, c: usize, bsl: usize) -> usize {
        let above = self.avail_u(r) && (MI_WIDTH_LOG2[self.mi.mi_size[self.idx(r - 1, c)] as usize] as usize) < bsl;
        let left = self.avail_l(c) && (MI_HEIGHT_LOG2[self.mi.mi_size[self.idx(r, c - 1)] as usize] as usize) < bsl;
        left as usize * 2 + above as usize
    }

    pub fn skip_ctx(&self, r: usize, c: usize) -> usize {
        let mut ctx = 0;
        if self.avail_u(r) {
            ctx += self.mi.skip[self.idx(r - 1, c)] as usize;
        }
        if self.avail_l(c) {
            ctx += self.mi.skip[self.idx(r, c - 1)] as usize;
        }
        ctx
    }

    /// `(above, left)` contexts of the intra frame y mode.
    pub fn y_mode_ctx(&self, r: usize, c: usize) -> (usize, usize) {
        let above = if self.avail_u(r) { self.mi.y_mode[self.idx(r - 1, c)] as usize } else { DC_PRED };
        let left = if self.avail_l(c) { self.mi.y_mode[self.idx(r, c - 1)] as usize } else { DC_PRED };
        (INTRA_MODE_CONTEXT[above] as usize, INTRA_MODE_CONTEXT[left] as usize)
    }

    /// The `tx_depth` context of a block whose largest transform is `max_rect`.
    pub fn tx_depth_ctx(&self, r: usize, c: usize, max_rect: usize) -> usize {
        let max_w = TX_WIDTH[max_rect] as usize;
        let max_h = TX_HEIGHT[max_rect] as usize;
        let mi = &self.mi;
        // an inter neighbour counts with its block size, an intra one with its transform size
        let above_w = if !self.avail_u(r) {
            0
        } else if mi.is_inter[self.idx(r - 1, c)] {
            block_width(mi.mi_size[self.idx(r - 1, c)] as usize)
        } else {
            TX_WIDTH[mi.inter_tx_size[self.idx(r - 1, c)] as usize] as usize
        };
        let left_h = if !self.avail_l(c) {
            0
        } else if mi.is_inter[self.idx(r, c - 1)] {
            block_height(mi.mi_size[self.idx(r, c - 1)] as usize)
        } else {
            TX_HEIGHT[mi.inter_tx_size[self.idx(r, c - 1)] as usize] as usize
        };
        (above_w >= max_w) as usize + (left_h >= max_h) as usize
    }

    /// Records a coded intra block in the mode info (clipped to the frame).
    pub fn store_block(&mut self, r: usize, c: usize, mi_size: usize, y_mode: usize, skip: bool, tx_size: usize) {
        self.store(r, c, mi_size, y_mode, skip, tx_size, [INTRA_FRAME as i8, -1], [Mv::ZERO; 2], false);
    }

    /// Records a coded inter block in the mode info (clipped to the frame).
    #[allow(clippy::too_many_arguments)]
    pub fn store_inter_block(&mut self, r: usize, c: usize, mi_size: usize, mode: usize, skip: bool, tx_size: usize, ref_frame: i8, mv: [i32; 2]) {
        self.store(r, c, mi_size, mode, skip, tx_size, [ref_frame, -1], [Mv::new(mv[0], mv[1]), Mv::ZERO], true);
    }

    #[allow(clippy::too_many_arguments)]
    fn store(&mut self, r: usize, c: usize, mi_size: usize, mode: usize, skip: bool, tx_size: usize, ref_frame: [i8; 2], mv: [Mv; 2], is_inter: bool) {
        let (bh4, bw4) = (NUM_4X4_BLOCKS_HIGH[mi_size] as usize, NUM_4X4_BLOCKS_WIDE[mi_size] as usize);
        for y in 0..bh4.min(self.mi_rows.saturating_sub(r)) {
            for x in 0..bw4.min(self.mi_cols.saturating_sub(c)) {
                let i = self.idx(r + y, c + x);
                let mi = &mut self.mi;
                mi.mi_size[i] = mi_size as u8;
                mi.skip[i] = skip;
                mi.y_mode[i] = mode as u8;
                mi.tx_size[i] = tx_size as u8;
                mi.inter_tx_size[i] = tx_size as u8;
                mi.is_inter[i] = is_inter;
                mi.ref_frame[i] = ref_frame;
                mi.mv[i] = mv;
                mi.written[i] = true;
            }
        }
    }

    /// Saves the mode info of the `rows` x `cols` units at `(r, c)` (clipped to the frame), so a
    /// search can try several codings of the region and go back.
    pub fn save_region(&self, r: usize, c: usize, rows: usize, cols: usize) -> RegionSave {
        let (rows, cols) = (rows.min(self.mi_rows.saturating_sub(r)), cols.min(self.mi_cols.saturating_sub(c)));
        let mut data = Vec::with_capacity(rows * cols);
        for y in 0..rows {
            for x in 0..cols {
                let (i, mi) = (self.idx(r + y, c + x), &self.mi);
                data.push(MiEntry { mi_size: mi.mi_size[i], skip: mi.skip[i], y_mode: mi.y_mode[i], tx_size: mi.tx_size[i], inter_tx_size: mi.inter_tx_size[i], is_inter: mi.is_inter[i], ref_frame: mi.ref_frame[i], mv: mi.mv[i], written: mi.written[i] });
            }
        }
        RegionSave { r, c, rows, cols, data }
    }

    /// Puts a saved rectangle back.
    pub fn restore_region(&mut self, save: &RegionSave) {
        for y in 0..save.rows {
            for x in 0..save.cols {
                let e = &save.data[y * save.cols + x];
                let i = self.idx(save.r + y, save.c + x);
                let mi = &mut self.mi;
                mi.mi_size[i] = e.mi_size;
                mi.skip[i] = e.skip;
                mi.y_mode[i] = e.y_mode;
                mi.tx_size[i] = e.tx_size;
                mi.inter_tx_size[i] = e.inter_tx_size;
                mi.is_inter[i] = e.is_inter;
                mi.ref_frame[i] = e.ref_frame;
                mi.mv[i] = e.mv;
                mi.written[i] = e.written;
            }
        }
    }

    /// The `is_inter` context.
    pub fn is_inter_ctx(&self, r: usize, c: usize) -> usize {
        let (avail_u, avail_l) = (self.avail_u(r), self.avail_l(c));
        let intra_of = |rr: usize, cc: usize| self.mi.ref_frame[self.idx(rr, cc)][0] <= INTRA_FRAME as i8;
        if avail_u && avail_l {
            let (a, l) = (intra_of(r - 1, c), intra_of(r, c - 1));
            if a && l { 3 } else { (a || l) as usize }
        } else if avail_u || avail_l {
            2 * (if avail_u { intra_of(r - 1, c) } else { intra_of(r, c - 1) }) as usize
        } else {
            0
        }
    }

    /// How often the above and left neighbours use reference frame `frame_type` (`count_refs`).
    pub fn count_refs(&self, r: usize, c: usize, frame_type: usize) -> usize {
        let mut n = 0;
        if self.avail_u(r) {
            let rf = self.mi.ref_frame[self.idx(r - 1, c)];
            n += (rf[0] == frame_type as i8) as usize + (rf[1] == frame_type as i8) as usize;
        }
        if self.avail_l(c) {
            let rf = self.mi.ref_frame[self.idx(r, c - 1)];
            n += (rf[0] == frame_type as i8) as usize + (rf[1] == frame_type as i8) as usize;
        }
        n
    }

    /// `reset_block_context()` for a skipped block.
    pub fn reset_block_context(&mut self, r: usize, c: usize, bw4: usize, bh4: usize, has_chroma: bool) {
        for plane in 0..(1 + 2 * has_chroma as usize) {
            let s = if plane > 0 { SS } else { 0 };
            for i in (c >> s)..((c + bw4) >> s) {
                self.above_level[plane][i] = 0;
                self.above_dc[plane][i] = 0;
            }
            for i in (r >> s)..((r + bh4) >> s) {
                self.left_level[plane][i] = 0;
                self.left_dc[plane][i] = 0;
            }
        }
    }

    /// The `all_zero` context of the transform block at `(x4, y4)` (plane 4x4 units).
    pub fn all_zero_ctx(&self, plane: usize, tx_sz: usize, x4: usize, y4: usize, mi_size: usize) -> usize {
        let (w4, h4) = (TX_WIDTH[tx_sz] as usize >> 2, TX_HEIGHT[tx_sz] as usize >> 2);
        let (mut max_x4, mut max_y4) = (self.mi_cols, self.mi_rows);
        if plane > 0 {
            max_x4 >>= SS;
            max_y4 >>= SS;
        }
        let (w, h) = (TX_WIDTH[tx_sz] as usize, TX_HEIGHT[tx_sz] as usize);
        let bsize = plane_residual_size(mi_size, plane);
        let (bw, bh) = (block_width(bsize), block_height(bsize));
        let al = &self.above_level[plane];
        let ll = &self.left_level[plane];
        if plane == 0 {
            let mut top = 0u32;
            let mut left = 0u32;
            for k in 0..w4 {
                if x4 + k < max_x4 {
                    top = top.max(al[x4 + k] as u32);
                }
            }
            for k in 0..h4 {
                if y4 + k < max_y4 {
                    left = left.max(ll[y4 + k] as u32);
                }
            }
            top = top.min(255);
            left = left.min(255);
            if bw == w && bh == h {
                0
            } else if top == 0 && left == 0 {
                1
            } else if top == 0 || left == 0 {
                2 + (top.max(left) > 3) as usize
            } else if top.max(left) <= 3 {
                4
            } else if top.min(left) <= 3 {
                5
            } else {
                6
            }
        } else {
            let ad = &self.above_dc[plane];
            let ld = &self.left_dc[plane];
            let mut above = 0u8;
            let mut left = 0u8;
            for i in 0..w4 {
                if x4 + i < max_x4 {
                    above |= al[x4 + i];
                    above |= ad[x4 + i];
                }
            }
            for i in 0..h4 {
                if y4 + i < max_y4 {
                    left |= ll[y4 + i];
                    left |= ld[y4 + i];
                }
            }
            let mut ctx = (above != 0) as usize + (left != 0) as usize;
            ctx += 7;
            if bw * bh > w * h {
                ctx += 3;
            }
            ctx
        }
    }

    /// The `dc_sign` context.
    pub fn dc_sign_ctx(&self, plane: usize, tx_sz: usize, x4: usize, y4: usize) -> usize {
        let (w4, h4) = (TX_WIDTH[tx_sz] as usize >> 2, TX_HEIGHT[tx_sz] as usize >> 2);
        let (mut max_x4, mut max_y4) = (self.mi_cols, self.mi_rows);
        if plane > 0 {
            max_x4 >>= SS;
            max_y4 >>= SS;
        }
        let mut dc_sign = 0i32;
        for k in 0..w4 {
            if x4 + k < max_x4 {
                match self.above_dc[plane][x4 + k] {
                    1 => dc_sign -= 1,
                    2 => dc_sign += 1,
                    _ => {}
                }
            }
        }
        for k in 0..h4 {
            if y4 + k < max_y4 {
                match self.left_dc[plane][y4 + k] {
                    1 => dc_sign -= 1,
                    2 => dc_sign += 1,
                    _ => {}
                }
            }
        }
        match dc_sign.signum() {
            -1 => 1,
            1 => 2,
            _ => 0,
        }
    }

    /// Stores the coefficient contexts of a coded transform block over its extent.
    pub fn set_coef_ctx(&mut self, plane: usize, tx_sz: usize, x4: usize, y4: usize, cul_level: u8, dc_category: u8) {
        let (w4, h4) = (TX_WIDTH[tx_sz] as usize >> 2, TX_HEIGHT[tx_sz] as usize >> 2);
        for i in 0..w4 {
            if x4 + i < self.above_level[plane].len() {
                self.above_level[plane][x4 + i] = cul_level;
                self.above_dc[plane][x4 + i] = dc_category;
            }
        }
        for i in 0..h4 {
            if y4 + i < self.left_level[plane].len() {
                self.left_level[plane][y4 + i] = cul_level;
                self.left_dc[plane][y4 + i] = dc_category;
            }
        }
    }
}
