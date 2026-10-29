// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M5: intra prediction + residual reconstruction through `studio_av1::intra::predict_intra` and
//! `studio_av1::transform::inverse_transform_2d`.
//!
//! The flow of [`FrameBuf::reconstruct_block`] is the decoder's `residual()` for an intra block
//! with 4:2:0 subsampling: 64x64 chunks, planes in order, transform blocks in raster order, each
//! one predicted from the already reconstructed neighbours and then given its residual.

use super::FrameBuf;
use crate::types::{BlockDecision, CoefBlock, SbPos};
use studio_av1::intra::{IntraParams, predict_intra};
use studio_av1::spec_tables::*;
use studio_av1::transform::inverse_transform_2d;

/// Quantiser parameters (spec 5.9.12): `base_q_idx` and the plane deltas. No segmentation, no
/// delta-q, no quantiser matrices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuantInfo {
    pub base_q_idx: u8,
    pub delta_q_y_dc: i32,
    pub delta_q_u_dc: i32,
    pub delta_q_u_ac: i32,
    pub delta_q_v_dc: i32,
    pub delta_q_v_ac: i32,
}

impl QuantInfo {
    /// `CodedLossless`: `base_q_idx == 0` with all deltas zero. Blocks then use 4x4 WHT.
    pub fn lossless(&self) -> bool {
        self.base_q_idx == 0
            && self.delta_q_y_dc == 0
            && self.delta_q_u_dc == 0
            && self.delta_q_u_ac == 0
            && self.delta_q_v_dc == 0
            && self.delta_q_v_ac == 0
    }
}

/// Per-block intra syntax that `BlockDecision` does not carry. `Default` is "no angle delta".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntraExtra {
    /// -3..=3, only for directional modes on blocks of 8x8 and larger.
    pub angle_delta_y: i32,
    pub angle_delta_uv: i32,
}

const SB4: usize = 16; // 64x64 superblocks
const BD_DIM: usize = 35;

/// Mode info and availability the intra predictor needs from earlier blocks.
#[derive(Default, Clone)]
pub(crate) struct IntraState {
    mi_rows: usize,
    mi_cols: usize,
    /// Tile bounds in mi units; the whole frame until `FrameBuf::set_tile`.
    tile: [usize; 4],
    y_mode: Vec<u8>,
    uv_mode: Vec<u8>,
    /// `BlockDecoded` for the current superblock, per plane, `(y + 1) * 35 + x + 1`.
    block_decoded: Vec<bool>,
}

impl IntraState {
    pub(crate) fn new(width: usize, height: usize) -> IntraState {
        let mi_cols = 2 * ((width + 7) >> 3);
        let mi_rows = 2 * ((height + 7) >> 3);
        IntraState {
            mi_rows,
            mi_cols,
            tile: [0, mi_rows, 0, mi_cols],
            y_mode: vec![DC_PRED as u8; mi_rows * mi_cols],
            uv_mode: vec![DC_PRED as u8; mi_rows * mi_cols],
            block_decoded: vec![false; 3 * BD_DIM * BD_DIM],
        }
    }

    fn inside(&self, r: isize, c: isize) -> bool {
        c >= self.tile[2] as isize
            && c < self.tile[3] as isize
            && r >= self.tile[0] as isize
            && r < self.tile[1] as isize
    }

    fn bd(&self, plane: usize, y: isize, x: isize) -> bool {
        let (yy, xx) = (y + 1, x + 1);
        if yy < 0 || xx < 0 || yy > BD_DIM as isize - 1 || xx > BD_DIM as isize - 1 {
            return false;
        }
        self.block_decoded[(plane * BD_DIM + yy as usize) * BD_DIM + xx as usize]
    }

    fn set_bd(&mut self, plane: usize, y: usize, x: usize) {
        if y + 1 < BD_DIM && x + 1 < BD_DIM {
            self.block_decoded[(plane * BD_DIM + y + 1) * BD_DIM + x + 1] = true;
        }
    }
}

/// Block-level facts shared by all transform blocks of one block.
struct Blk {
    mi_row: usize,
    mi_col: usize,
    avail_u: [bool; 2],
    avail_l: [bool; 2],
    y_mode: usize,
    uv_mode: usize,
    extra: IntraExtra,
}

fn block_size_index(w4: usize, h4: usize) -> usize {
    (0..NUM_4X4_BLOCKS_WIDE.len())
        .find(|&b| NUM_4X4_BLOCKS_WIDE[b] as usize == w4 && NUM_4X4_BLOCKS_HIGH[b] as usize == h4)
        .expect("block size must be a valid AV1 block size")
}

fn find_tx_size(w: usize, h: usize) -> usize {
    (0..TX_WIDTH.len())
        .find(|&t| TX_WIDTH[t] as usize == w && TX_HEIGHT[t] as usize == h)
        .expect("valid transform size")
}

fn is_directional(mode: usize) -> bool {
    (V_PRED..=D67_PRED).contains(&mode)
}

impl FrameBuf {
    /// Restricts availability to one tile (mi units, end exclusive). Defaults to the whole frame.
    pub fn set_tile(
        &mut self,
        mi_row_start: usize,
        mi_row_end: usize,
        mi_col_start: usize,
        mi_col_end: usize,
    ) {
        self.intra.tile = [
            mi_row_start,
            mi_row_end.min(self.intra.mi_rows),
            mi_col_start,
            mi_col_end.min(self.intra.mi_cols),
        ];
    }

    /// Call at the start of every superblock, before its blocks (spec 5.11.3
    /// `clear_block_decoded_flags`, 64x64 superblocks).
    pub fn begin_sb(&mut self, sb: SbPos) {
        let (r, c) = (sb.row as usize * SB4, sb.col as usize * SB4);
        let st = &mut self.intra;
        for plane in 0..3 {
            let sub = (plane > 0) as usize;
            let sb_w4 = (st.tile[3] as isize - c as isize) >> sub;
            let sb_h4 = (st.tile[1] as isize - r as isize) >> sub;
            for y in -1..=((SB4 >> sub) as isize) {
                for x in -1..=((SB4 >> sub) as isize) {
                    let v = if y < 0 && x < sb_w4 {
                        true
                    } else {
                        x < 0 && y < sb_h4
                    };
                    st.block_decoded
                        [(plane * BD_DIM + (y + 1) as usize) * BD_DIM + (x + 1) as usize] = v;
                }
            }
            st.block_decoded[(plane * BD_DIM + (SB4 >> sub) + 1) * BD_DIM] = false;
        }
    }

    /// Records an inter block of an inter frame for the intra blocks coded after it: its mode info
    /// counts as a non-smooth `DC_PRED` neighbour, and its transform blocks (one per plane, 8x8
    /// blocks and larger) are marked as decoded for the above-right / below-left availability of
    /// intra prediction. Call it once the block's samples are in the buffer.
    pub(crate) fn mark_inter_block(&mut self, mi_row: usize, mi_col: usize, bw4: usize, bh4: usize) {
        let st = &mut self.intra;
        for r in mi_row..(mi_row + bh4).min(st.mi_rows) {
            for c in mi_col..(mi_col + bw4).min(st.mi_cols) {
                st.y_mode[r * st.mi_cols + c] = DC_PRED as u8;
                st.uv_mode[r * st.mi_cols + c] = DC_PRED as u8;
            }
        }
        for plane in 0..3 {
            let sub = (plane > 0) as usize;
            let (w, h) = ((bw4 >> sub).max(1), (bh4 >> sub).max(1));
            let (sbr, sbc) = ((mi_row & (SB4 - 1)) >> sub, (mi_col & (SB4 - 1)) >> sub);
            for i in 0..h {
                for j in 0..w {
                    st.set_bd(plane, sbr + i, sbc + j);
                }
            }
        }
    }

    /// Reconstructs one intra block with default [`IntraExtra`].
    pub fn reconstruct_block(&mut self, b: &BlockDecision) {
        self.reconstruct_block_ex(b, IntraExtra::default());
    }

    /// Reconstructs one intra block: prediction plus dequantised, inverse-transformed residual,
    /// for every transform block in decode order. `b.coefs` holds one entry per transform block
    /// that starts inside the coded area (the decoder skips the rest), in decode order (chunk, plane, raster), and is ignored when `b.skip`; a
    /// block with `eob == 0` adds nothing. `CoefBlock::levels` is indexed `row * tw + col` with
    /// `tw = min(width, 32)` (the decoder's `Quant` layout, spec 7.12.3). Calls must follow the
    /// bitstream order, with [`FrameBuf::begin_sb`] at each superblock start.
    pub fn reconstruct_block_ex(&mut self, b: &BlockDecision, extra: IntraExtra) {
        let (mi_row, mi_col, bw4, bh4) = (
            b.mi_row as usize,
            b.mi_col as usize,
            b.w4 as usize,
            b.h4 as usize,
        );
        let has_chroma = !((bh4 == 1 && mi_row & 1 == 0) || (bw4 == 1 && mi_col & 1 == 0));
        let st = &self.intra;
        let avail_u = st.inside(mi_row as isize - 1, mi_col as isize);
        let avail_l = st.inside(mi_row as isize, mi_col as isize - 1);
        let (mut avail_uc, mut avail_lc) = (avail_u, avail_l);
        if has_chroma {
            if bh4 == 1 {
                avail_uc = st.inside(mi_row as isize - 2, mi_col as isize);
            }
            if bw4 == 1 {
                avail_lc = st.inside(mi_row as isize, mi_col as isize - 2);
            }
        } else {
            avail_uc = false;
            avail_lc = false;
        }
        let uv_mode = if has_chroma { b.uv_mode } else { DC_PRED };
        let blk = Blk {
            mi_row,
            mi_col,
            avail_u: [avail_u, avail_uc],
            avail_l: [avail_l, avail_lc],
            y_mode: b.y_mode,
            uv_mode,
            extra,
        };

        // Mode info is stored before the residual: later blocks read it, and so does the edge
        // filter type of this block's own neighbours.
        let st = &mut self.intra;
        for r in mi_row..(mi_row + bh4).min(st.mi_rows) {
            for c in mi_col..(mi_col + bw4).min(st.mi_cols) {
                st.y_mode[r * st.mi_cols + c] = b.y_mode as u8;
                st.uv_mode[r * st.mi_cols + c] = uv_mode as u8;
            }
        }

        let lossless = self.quant.lossless();
        let ms = block_size_index(bw4, bh4);
        let (bw, bh) = (bw4 * 4, bh4 * 4);
        let w_chunks = (bw >> 6).max(1);
        let h_chunks = (bh >> 6).max(1);
        let chunk_ms = if w_chunks > 1 || h_chunks > 1 {
            BLOCK_64X64
        } else {
            ms
        };
        let mut coefs = if b.skip { [].iter() } else { b.coefs.iter() };
        for chunk_y in 0..h_chunks {
            for chunk_x in 0..w_chunks {
                for plane in 0..(1 + 2 * has_chroma as usize) {
                    let sub = (plane > 0) as usize;
                    let tx_sz = if lossless {
                        TX_4X4
                    } else {
                        self.tx_size_for(plane, ms, b.tx_size)
                    };
                    let step_x = TX_WIDTH[tx_sz] as usize >> 2;
                    let step_y = TX_HEIGHT[tx_sz] as usize >> 2;
                    let plane_sz = SUBSAMPLED_SIZE[chunk_ms][sub][sub] as usize;
                    let n4w = NUM_4X4_BLOCKS_WIDE[plane_sz] as usize;
                    let n4h = NUM_4X4_BLOCKS_HIGH[plane_sz] as usize;
                    let base_x = (mi_col >> sub) * 4;
                    let base_y = (mi_row >> sub) * 4;
                    let mut y = 0;
                    while y < n4h {
                        let mut x = 0;
                        while x < n4w {
                            self.transform_block(
                                &blk,
                                plane,
                                base_x,
                                base_y,
                                tx_sz,
                                x + ((chunk_x << 4) >> sub),
                                y + ((chunk_y << 4) >> sub),
                                &mut coefs,
                            );
                            x += step_x;
                        }
                        y += step_y;
                    }
                }
            }
        }
    }

    /// `get_tx_size` (5.11.37): chroma uses the largest transform of its block, capped at 32.
    fn tx_size_for(&self, plane: usize, ms: usize, luma_tx: usize) -> usize {
        if plane == 0 {
            return luma_tx;
        }
        let psz = SUBSAMPLED_SIZE[ms][1][1] as usize;
        let w = (NUM_4X4_BLOCKS_WIDE[psz] as usize * 4).min(32);
        let h = (NUM_4X4_BLOCKS_HIGH[psz] as usize * 4).min(32);
        find_tx_size(w, h)
    }

    #[allow(clippy::too_many_arguments)]
    fn transform_block(
        &mut self,
        blk: &Blk,
        plane: usize,
        base_x: usize,
        base_y: usize,
        tx_sz: usize,
        x: usize,
        y: usize,
        coefs: &mut std::slice::Iter<'_, CoefBlock>,
    ) {
        let start_x = base_x + 4 * x;
        let start_y = base_y + 4 * y;
        let sub = (plane > 0) as usize;
        let row = (start_y << sub) >> 2;
        let col = (start_x << sub) >> 2;
        let sub_block_mi_row = row & (SB4 - 1);
        let sub_block_mi_col = col & (SB4 - 1);
        let step_x = TX_WIDTH[tx_sz] as usize >> 2;
        let step_y = TX_HEIGHT[tx_sz] as usize >> 2;
        let max_x = (self.intra.mi_cols * 4) >> sub;
        let max_y = (self.intra.mi_rows * 4) >> sub;
        if start_x >= max_x || start_y >= max_y {
            return;
        }
        let is_chroma = plane > 0;
        let mode = if is_chroma { blk.uv_mode } else { blk.y_mode };
        let st = &self.intra;
        let sbr = (sub_block_mi_row >> sub) as isize;
        let sbc = (sub_block_mi_col >> sub) as isize;
        let params = IntraParams {
            plane,
            x: start_x,
            y: start_y,
            have_left: blk.avail_l[is_chroma as usize] || x > 0,
            have_above: blk.avail_u[is_chroma as usize] || y > 0,
            have_above_right: st.bd(plane, sbr - 1, sbc + step_x as isize),
            have_below_left: st.bd(plane, sbr + step_y as isize, sbc - 1),
            mode,
            log2w: TX_WIDTH_LOG2[tx_sz] as u32,
            log2h: TX_HEIGHT_LOG2[tx_sz] as u32,
            max_x: max_x as i32 - 1,
            max_y: max_y as i32 - 1,
            bit_depth: self.bit_depth as u32,
            angle_delta: if is_chroma {
                blk.extra.angle_delta_uv
            } else {
                blk.extra.angle_delta_y
            },
            use_filter_intra: false,
            filter_intra_mode: 0,
            enable_intra_edge_filter: self.enable_intra_edge_filter,
            filter_type: is_directional(mode)
                && self.enable_intra_edge_filter
                && self.filter_type(blk, plane),
        };
        predict_intra(&mut self.planes[plane], &params);
        // blocks that start outside the frame are not coded, so they take no coefficients
        if let Some(c) = coefs.next().filter(|c| c.eob > 0) {
            self.add_residual(plane, start_x, start_y, tx_sz, c);
        }
        for i in 0..step_y {
            for j in 0..step_x {
                let by = (sub_block_mi_row >> sub) + i;
                let bx = (sub_block_mi_col >> sub) + j;
                self.intra.set_bd(plane, by, bx);
            }
        }
    }

    /// Intra filter type process (7.11.2.8): whether a neighbour uses a smooth mode.
    fn filter_type(&self, blk: &Blk, plane: usize) -> bool {
        let st = &self.intra;
        let is_smooth = |r: usize, c: usize| {
            let m = if plane == 0 {
                st.y_mode[r * st.mi_cols + c]
            } else {
                st.uv_mode[r * st.mi_cols + c]
            } as usize;
            m == SMOOTH_PRED || m == SMOOTH_V_PRED || m == SMOOTH_H_PRED
        };
        let ci = (plane > 0) as usize;
        let (mut above, mut left) = (false, false);
        if blk.avail_u[ci] {
            let mut r = blk.mi_row as isize - 1;
            let mut c = blk.mi_col;
            if plane > 0 {
                if blk.mi_col & 1 == 0 {
                    c += 1;
                }
                if blk.mi_row & 1 == 1 {
                    r -= 1;
                }
            }
            above = is_smooth(r as usize, c.min(st.mi_cols - 1));
        }
        if blk.avail_l[ci] {
            let mut r = blk.mi_row;
            let mut c = blk.mi_col as isize - 1;
            if plane > 0 {
                if blk.mi_col & 1 == 1 {
                    c -= 1;
                }
                if blk.mi_row & 1 == 0 {
                    r += 1;
                }
            }
            left = is_smooth(r.min(st.mi_rows - 1), c as usize);
        }
        above || left
    }

    /// Dequantisation (7.12.3) and inverse transform (7.13.3), added to the prediction with
    /// clipping and the flips of the FLIPADST types.
    fn add_residual(&mut self, plane: usize, x: usize, y: usize, tx_sz: usize, c: &CoefBlock) {
        let dq_denom: i64 = match tx_sz {
            TX_32X32 | TX_16X32 | TX_32X16 | TX_16X64 | TX_64X16 => 2,
            TX_64X64 | TX_32X64 | TX_64X32 => 4,
            _ => 1,
        };
        let w = TX_WIDTH[tx_sz] as usize;
        let h = TX_HEIGHT[tx_sz] as usize;
        let (tw, th) = (w.min(32), h.min(32));
        let t = c.tx_type;
        let flip_ud = matches!(
            t,
            FLIPADST_DCT | FLIPADST_ADST | V_FLIPADST | FLIPADST_FLIPADST
        );
        let flip_lr = matches!(
            t,
            DCT_FLIPADST | ADST_FLIPADST | H_FLIPADST | FLIPADST_FLIPADST
        );
        let bd = self.bit_depth as u32;
        let q = &self.quant;
        let qindex = q.base_q_idx as i32;
        let (dc_delta, ac_delta) = match plane {
            0 => (q.delta_q_y_dc, 0),
            1 => (q.delta_q_u_dc, q.delta_q_u_ac),
            _ => (q.delta_q_v_dc, q.delta_q_v_ac),
        };
        let bd_idx = ((bd - 8) >> 1) as usize;
        let dc_q = DC_QLOOKUP[bd_idx][(qindex + dc_delta).clamp(0, 255) as usize] as i64;
        let ac_q = AC_QLOOKUP[bd_idx][(qindex + ac_delta).clamp(0, 255) as usize] as i64;
        let lim = 1i64 << (7 + bd);
        let mut dequant = vec![0i32; 64 * 64];
        for i in 0..th {
            for j in 0..tw {
                let qv = c.levels.get(i * tw + j).copied().unwrap_or(0);
                if qv == 0 {
                    continue;
                }
                let qs = if i == 0 && j == 0 { dc_q } else { ac_q };
                let dq = qv as i64 * qs;
                let mag = (dq.abs() & 0xFFFFFF) / dq_denom;
                let dq2 = if dq < 0 { -mag } else { mag };
                dequant[i * 64 + j] = dq2.clamp(-lim, lim - 1) as i32;
            }
        }
        let mut residual = vec![0i32; w * h];
        inverse_transform_2d(&dequant, &mut residual, tx_sz, t, self.quant.lossless(), bd);
        let pl = &mut self.planes[plane];
        let max = (1i32 << bd) - 1;
        for i in 0..h {
            let yy = if flip_ud { h - i - 1 } else { i };
            let row = &mut pl.row_from_mut(y + yy, x)[..w];
            let res = &residual[i * w..i * w + w];
            if flip_lr {
                for (o, &r) in row.iter_mut().rev().zip(res) {
                    *o = (*o as i32 + r).clamp(0, max) as u16;
                }
            } else {
                for (o, &r) in row.iter_mut().zip(res) {
                    *o = (*o as i32 + r).clamp(0, max) as u16;
                }
            }
        }
    }
}
