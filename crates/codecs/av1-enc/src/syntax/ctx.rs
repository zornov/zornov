// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M4: above/left context arrays (partition, skip, modes, coefficients) and the context
//! derivations of spec 8.3.2. Everything here mirrors `studio_av1`'s tile decoder, so a symbol is
//! written with exactly the CDF the decoder will pick for it.
//!
//! One tile, 4:2:0, intra frames only: neighbours are available whenever they lie inside the
//! frame, and the mode info of earlier blocks is kept in per-4x4 grids.

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

/// The context state of one tile.
pub struct ContextState {
    pub mi_rows: usize,
    pub mi_cols: usize,
    mi_size: Vec<u8>,
    skip: Vec<bool>,
    y_mode: Vec<u8>,
    /// Transform size of each 4x4 unit (`InterTxSizes`, which equals `TxSize` in intra frames).
    tx_size: Vec<u8>,
    above_level: [Vec<u8>; 3],
    above_dc: [Vec<u8>; 3],
    left_level: [Vec<u8>; 3],
    left_dc: [Vec<u8>; 3],
    /// `TileIntraFrameYModeCdf`: reset at every tile start, outside the saved CDF set.
    pub intra_y_mode_cdf: [[[u16; 14]; 5]; 5],
}

impl ContextState {
    pub fn new(mi_cols: usize, mi_rows: usize) -> ContextState {
        let n = mi_rows * mi_cols;
        ContextState {
            mi_rows,
            mi_cols,
            mi_size: vec![0; n],
            skip: vec![false; n],
            y_mode: vec![DC_PRED as u8; n],
            tx_size: vec![0; n],
            above_level: std::array::from_fn(|_| vec![0; mi_cols + ARRAY_SLACK]),
            above_dc: std::array::from_fn(|_| vec![0; mi_cols + ARRAY_SLACK]),
            left_level: std::array::from_fn(|_| vec![0; mi_rows + ARRAY_SLACK]),
            left_dc: std::array::from_fn(|_| vec![0; mi_rows + ARRAY_SLACK]),
            intra_y_mode_cdf: DEFAULT_INTRA_FRAME_Y_MODE_CDF,
        }
    }

    #[inline]
    fn idx(&self, r: usize, c: usize) -> usize {
        r * self.mi_cols + c
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
        let above = r > 0 && (MI_WIDTH_LOG2[self.mi_size[self.idx(r - 1, c)] as usize] as usize) < bsl;
        let left = c > 0 && (MI_HEIGHT_LOG2[self.mi_size[self.idx(r, c - 1)] as usize] as usize) < bsl;
        left as usize * 2 + above as usize
    }

    pub fn skip_ctx(&self, r: usize, c: usize) -> usize {
        let mut ctx = 0;
        if r > 0 {
            ctx += self.skip[self.idx(r - 1, c)] as usize;
        }
        if c > 0 {
            ctx += self.skip[self.idx(r, c - 1)] as usize;
        }
        ctx
    }

    /// `(above, left)` contexts of the intra frame y mode.
    pub fn y_mode_ctx(&self, r: usize, c: usize) -> (usize, usize) {
        let above = if r > 0 { self.y_mode[self.idx(r - 1, c)] as usize } else { DC_PRED };
        let left = if c > 0 { self.y_mode[self.idx(r, c - 1)] as usize } else { DC_PRED };
        (INTRA_MODE_CONTEXT[above] as usize, INTRA_MODE_CONTEXT[left] as usize)
    }

    /// The `tx_depth` context of a block whose largest transform is `max_rect`.
    pub fn tx_depth_ctx(&self, r: usize, c: usize, max_rect: usize) -> usize {
        let max_w = TX_WIDTH[max_rect] as usize;
        let max_h = TX_HEIGHT[max_rect] as usize;
        let above_w = if r > 0 { TX_WIDTH[self.tx_size[self.idx(r - 1, c)] as usize] as usize } else { 0 };
        let left_h = if c > 0 { TX_HEIGHT[self.tx_size[self.idx(r, c - 1)] as usize] as usize } else { 0 };
        (above_w >= max_w) as usize + (left_h >= max_h) as usize
    }

    /// Records a coded block in the mode info grids (clipped to the frame).
    pub fn store_block(&mut self, r: usize, c: usize, mi_size: usize, y_mode: usize, skip: bool, tx_size: usize) {
        let (bh4, bw4) = (NUM_4X4_BLOCKS_HIGH[mi_size] as usize, NUM_4X4_BLOCKS_WIDE[mi_size] as usize);
        for y in 0..bh4.min(self.mi_rows.saturating_sub(r)) {
            for x in 0..bw4.min(self.mi_cols.saturating_sub(c)) {
                let i = self.idx(r + y, c + x);
                self.mi_size[i] = mi_size as u8;
                self.skip[i] = skip;
                self.y_mode[i] = y_mode as u8;
                self.tx_size[i] = tx_size as u8;
            }
        }
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
