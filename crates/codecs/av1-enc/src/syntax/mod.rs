// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M4: block-level syntax writers (spec 5.11), the inverse of the decoder's tile parsing. Keeps its
//! own above/left context state and mirrors the decoder's context derivation.
//!
//! Scope of this version: intra frames and phase (b) inter frames (single reference, translation
//! only, inter blocks only), one tile, 4:2:0, 64x64 or 128x128 superblocks, every
//! partition type, all 13 intra modes with angle deltas, the intra transform sets, `TX_MODE_LARGEST`
//! or `TX_MODE_SELECT`, and lossless. The frame header the writer pairs with (M1) fixes the rest:
//! no segmentation, delta q, CDEF bits, palettes, filter intra or chroma-from-luma.

pub mod coef;
pub mod ctx;
pub mod inter_info;
pub mod intra_info;
pub mod lr;

use studio_av1::spec_tables::*;

use studio_av1::mvpred::MvFrame;

use crate::entropy::SymbolEncoder;
use crate::entropy::cdf_ctx::CdfContext;
use crate::motion::mvpred::{find_stack, frame_info};
use crate::recon::IntraExtra;
use crate::types::{BlockDecision, BlockMode, CoefBlock, InterDecision, InterSbDecision, SbDecision, SbPos, SeqParams};

use coef::{TxbParams, write_coefficients};
use ctx::*;
use inter_info::write_inter_block_mode_info;
use intra_info::{BlockInfo, FrameSyntax, write_intra_block_in_inter_frame, write_intra_frame_mode_info};

/// A leaf of a partition: `(mi_row, mi_col, w4, h4)`.
type Leaf = (usize, usize, usize, usize);

/// What the residual syntax needs to know about a block.
struct ResidualGeo {
    mi_row: usize,
    mi_col: usize,
    mi_size: usize,
    has_chroma: bool,
    tx_size: usize,
    y_mode: usize,
    uv_mode: usize,
    is_inter: bool,
}

/// Writes one tile's symbols.
pub struct TileWriter {
    enc: SymbolEncoder,
    cdfs: CdfContext,
    ctx: ContextState,
    mi_rows: usize,
    mi_cols: usize,
    base_q_idx: u8,
    sb_size: usize,
    frame: FrameSyntax,
    /// Frame-level inputs of the motion vector processes (inter frames).
    mv_frame: MvFrame,
    /// Loop restoration units, coded in front of each superblock.
    lr: Option<lr::LrWriter>,
}

impl TileWriter {
    /// A writer for the whole frame as one tile, with default CDFs for `base_q_idx`, adaptation on
    /// and `TX_MODE_LARGEST` (what M1's frame header codes).
    pub fn new(seq: &SeqParams, base_q_idx: u8, mi_cols: u32, mi_rows: u32) -> TileWriter {
        TileWriter {
            enc: SymbolEncoder::new(),
            cdfs: CdfContext::new(base_q_idx as u32),
            ctx: ContextState::new(mi_cols as usize, mi_rows as usize),
            mi_rows: mi_rows as usize,
            mi_cols: mi_cols as usize,
            base_q_idx,
            sb_size: if seq.use_128x128_superblock { BLOCK_128X128 } else { BLOCK_64X64 },
            frame: FrameSyntax { tx_mode_select: false, lossless: base_q_idx == 0 },
            mv_frame: frame_info(mi_rows as usize, mi_cols as usize),
            lr: None,
        }
    }

    /// Codes the restoration units of `choice` in front of the superblocks.
    pub fn with_lr(mut self, choice: crate::types::LrChoice) -> TileWriter {
        self.lr = Some(lr::LrWriter::new(choice));
        self
    }

    fn write_lr(&mut self, sb: SbPos) {
        let sb4 = NUM_4X4_BLOCKS_WIDE[self.sb_size] as usize;
        if let Some(lr) = &mut self.lr {
            lr.write_sb(&mut self.enc, &mut self.cdfs.cdfs, sb.row as usize * sb4, sb.col as usize * sb4, sb4);
        }
    }

    /// Codes `cdef_idx` (`bits` literal bits, from `idx`, one per 64x64 block) as the decoder reads it.
    pub fn with_cdef(mut self, bits: u32, idx: Vec<i8>) -> TileWriter {
        self.ctx.cdef = Some(CdefSyntax::new(bits, self.mi_cols.div_ceil(16), idx));
        self
    }

    /// Mirrors `disable_cdf_update` of the frame header. Call before writing anything.
    pub fn with_disable_cdf_update(mut self, disable: bool) -> TileWriter {
        self.enc = SymbolEncoder::with_disable_update(disable);
        self
    }

    /// Mirrors `tx_mode_select` of the frame header (`TX_MODE_SELECT` when true).
    pub fn with_tx_mode_select(mut self, select: bool) -> TileWriter {
        self.frame.tx_mode_select = select;
        self
    }

    /// The partition symbol at a block of `bsize` (spec 5.11.4). Writes nothing where the decoder
    /// infers the partition: blocks outside the frame, blocks below 8x8, and blocks that cross
    /// both frame edges (always split).
    ///
    /// # Panics
    /// If `partition` is not one the position allows.
    pub fn write_partition(&mut self, mi_row: u32, mi_col: u32, bsize: usize, partition: usize) {
        let (r, c) = (mi_row as usize, mi_col as usize);
        if r >= self.mi_rows || c >= self.mi_cols {
            return;
        }
        if bsize < BLOCK_8X8 {
            assert_eq!(partition, PARTITION_NONE);
            return;
        }
        let num4x4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let half = num4x4 >> 1;
        let has_rows = r + half < self.mi_rows;
        let has_cols = c + half < self.mi_cols;
        let bsl = MI_WIDTH_LOG2[bsize] as usize;
        let pctx = self.ctx.partition_ctx(r, c, bsl);
        let cdfs = &mut *self.cdfs.cdfs;
        if has_rows && has_cols {
            let cdf: &mut [u16] = match bsl {
                1 => &mut cdfs.partition_w8[pctx],
                2 => &mut cdfs.partition_w16[pctx],
                3 => &mut cdfs.partition_w32[pctx],
                4 => &mut cdfs.partition_w64[pctx],
                _ => &mut cdfs.partition_w128[pctx],
            };
            self.enc.write_symbol(cdf, partition);
        } else if has_cols || has_rows {
            let pcdf: Vec<u16> = match bsl {
                2 => cdfs.partition_w16[pctx].to_vec(),
                3 => cdfs.partition_w32[pctx].to_vec(),
                4 => cdfs.partition_w64[pctx].to_vec(),
                _ => cdfs.partition_w128[pctx].to_vec(),
            };
            // split_or_horz / split_or_vert: a bit whose probability comes from the partition CDF
            let pr = |k: usize| pcdf[k] as i32 - if k == 0 { 0 } else { pcdf[k - 1] as i32 };
            let mut psum = if has_cols {
                assert!(partition == PARTITION_HORZ || partition == PARTITION_SPLIT, "partition {partition} at a bottom edge");
                pr(PARTITION_VERT) + pr(PARTITION_SPLIT) + pr(PARTITION_HORZ_A) + pr(PARTITION_VERT_A) + pr(PARTITION_VERT_B)
            } else {
                assert!(partition == PARTITION_VERT || partition == PARTITION_SPLIT, "partition {partition} at a right edge");
                pr(PARTITION_HORZ) + pr(PARTITION_SPLIT) + pr(PARTITION_HORZ_A) + pr(PARTITION_HORZ_B) + pr(PARTITION_VERT_A)
            };
            if bsize != BLOCK_128X128 {
                psum += if has_cols { pr(PARTITION_VERT_4) } else { pr(PARTITION_HORZ_4) };
            }
            // a one-off CDF: adapting this copy is harmless
            let mut cdf = [((1i32 << 15) - psum) as u16, 1 << 15, 0];
            self.enc.write_symbol(&mut cdf, (partition == PARTITION_SPLIT) as usize);
        } else {
            assert_eq!(partition, PARTITION_SPLIT, "a block past both edges must split");
        }
    }

    /// One block's mode info and coefficients, with no angle deltas.
    pub fn write_block(&mut self, dec: &BlockDecision) {
        self.write_block_ex(dec, IntraExtra::default());
    }

    /// One block's mode info and coefficients (`decode_block()` without prediction).
    /// `dec.coefs` lists the transform blocks in decode order, as `FrameBuf::reconstruct_block`
    /// expects them, and is ignored when `dec.skip`.
    pub fn write_block_ex(&mut self, dec: &BlockDecision, extra: IntraExtra) {
        let (r, c) = (dec.mi_row as usize, dec.mi_col as usize);
        let (bw4, bh4) = (dec.w4 as usize, dec.h4 as usize);
        let ms = block_size_of(bw4, bh4).unwrap_or_else(|| panic!("no block size {bw4}x{bh4}"));
        assert!(r < self.mi_rows && c < self.mi_cols, "block outside the frame");
        let has_chroma = !((bh4 == 1 && r & 1 == 0) || (bw4 == 1 && c & 1 == 0));
        let info = BlockInfo { mi_row: r, mi_col: c, mi_size: ms, has_chroma, y_mode: dec.y_mode, uv_mode: dec.uv_mode, skip: dec.skip, tx_size: dec.tx_size, extra };
        write_intra_frame_mode_info(&mut self.enc, &mut self.cdfs.cdfs, &mut self.ctx, &info, self.frame);
        if dec.skip {
            self.ctx.reset_block_context(r, c, bw4, bh4, has_chroma);
        } else {
            self.write_residual(&info, dec);
        }
        let tx = if self.frame.lossless { TX_4X4 } else { dec.tx_size };
        self.ctx.store_block(r, c, ms, dec.y_mode, dec.skip, tx);
    }

    /// `residual()` (5.11.34) of an intra block.
    fn write_residual(&mut self, b: &BlockInfo, dec: &BlockDecision) {
        let g = ResidualGeo { mi_row: b.mi_row, mi_col: b.mi_col, mi_size: b.mi_size, has_chroma: b.has_chroma, tx_size: b.tx_size, y_mode: b.y_mode, uv_mode: b.uv_mode, is_inter: false };
        self.write_residual_coefs(&g, &dec.coefs);
    }

    /// `residual()` (5.11.34): the transform blocks of a block in decode order. `coefs` has one
    /// entry for each transform block that starts inside the coded area.
    fn write_residual_coefs(&mut self, b: &ResidualGeo, all: &[CoefBlock]) {
        let ms = b.mi_size;
        let width_chunks = (block_width(ms) >> 6).max(1);
        let height_chunks = (block_height(ms) >> 6).max(1);
        let mi_size_chunk = if width_chunks > 1 || height_chunks > 1 { BLOCK_64X64 } else { ms };
        let mut coefs = all.iter();
        // the type of the luma transform, which inter chroma follows
        let mut luma_type = DCT_DCT;
        for chunk_y in 0..height_chunks {
            for chunk_x in 0..width_chunks {
                for plane in 0..(1 + 2 * b.has_chroma as usize) {
                    let tx_sz = if self.frame.lossless { TX_4X4 } else { uv_aware_tx_size(ms, plane, b.tx_size) };
                    let step_x = TX_WIDTH[tx_sz] as usize >> 2;
                    let step_y = TX_HEIGHT[tx_sz] as usize >> 2;
                    let plane_sz = plane_residual_size(mi_size_chunk, plane);
                    let num4x4_w = NUM_4X4_BLOCKS_WIDE[plane_sz] as usize;
                    let num4x4_h = NUM_4X4_BLOCKS_HIGH[plane_sz] as usize;
                    let sub = if plane > 0 { SS } else { 0 };
                    let base_x = (b.mi_col >> sub) * 4;
                    let base_y = (b.mi_row >> sub) * 4;
                    let max_x = (self.mi_cols * 4) >> sub;
                    let max_y = (self.mi_rows * 4) >> sub;
                    for y in (0..num4x4_h).step_by(step_y) {
                        for x in (0..num4x4_w).step_by(step_x) {
                            let start_x = base_x + 4 * (x + ((chunk_x << 4) >> sub));
                            let start_y = base_y + 4 * (y + ((chunk_y << 4) >> sub));
                            if start_x >= max_x || start_y >= max_y {
                                continue;
                            }
                            let cb = coefs.next().unwrap_or_else(|| panic!("missing coefficients for plane {plane} at ({start_x},{start_y})"));
                            assert_eq!(cb.tx_size, tx_sz, "plane {plane} transform block at ({start_x},{start_y}) has the wrong size");
                            let p = TxbParams {
                                plane,
                                x4: start_x >> 2,
                                y4: start_y >> 2,
                                mi_size: ms,
                                y_mode: b.y_mode,
                                uv_mode: b.uv_mode,
                                lossless: self.frame.lossless,
                                qidx_nonzero: self.base_q_idx > 0,
                                is_inter: b.is_inter,
                                luma_tx_type: luma_type,
                            };
                            write_coefficients(&mut self.enc, &mut self.cdfs.cdfs, &mut self.ctx, &p, cb);
                            if plane == 0 {
                                luma_type = if cb.eob > 0 { cb.tx_type } else { DCT_DCT };
                            }
                        }
                    }
                }
            }
        }
        assert!(coefs.next().is_none(), "more coefficient blocks than transform blocks");
    }

    /// One inter block of an inter frame (`decode_block()` without prediction): its mode info
    /// with the motion vector, then the coefficients unless it is a skip block.
    pub fn write_inter_block(&mut self, d: &InterDecision) {
        let (r, c) = (d.mi_row as usize, d.mi_col as usize);
        let (bw4, bh4) = (d.w4 as usize, d.h4 as usize);
        let ms = block_size_of(bw4, bh4).unwrap_or_else(|| panic!("no block size {bw4}x{bh4}"));
        assert!(r < self.mi_rows && c < self.mi_cols, "block outside the frame");
        let stack = find_stack(&self.ctx.mi, &self.mv_frame, (r, c, bw4, bh4), d.ref_frame);
        write_inter_block_mode_info(&mut self.enc, &mut self.cdfs.cdfs, &self.ctx, d, &stack);
        // TX_MODE_LARGEST: the transform size is not coded
        let tx = if self.frame.lossless { TX_4X4 } else { MAX_TX_SIZE_RECT[ms] as usize };
        assert!(!self.frame.tx_mode_select && !self.frame.lossless, "inter blocks need TX_MODE_LARGEST and a lossy frame");
        if d.skip {
            self.ctx.reset_block_context(r, c, bw4, bh4, true);
        } else {
            let g = ResidualGeo { mi_row: r, mi_col: c, mi_size: ms, has_chroma: true, tx_size: tx, y_mode: DC_PRED, uv_mode: DC_PRED, is_inter: true };
            self.write_residual_coefs(&g, &d.coefs);
        }
        self.ctx.store_inter_block(r, c, ms, d.mode, d.skip, tx, d.ref_frame as i8, d.mv);
    }

    /// One intra block of an inter frame (8x8 or larger), with no angle deltas.
    pub fn write_intra_block_in_inter(&mut self, dec: &BlockDecision) {
        self.write_intra_block_in_inter_ex(dec, IntraExtra::default());
    }

    /// One intra block of an inter frame: like [`write_block_ex`](Self::write_block_ex), with the
    /// mode info of `inter_frame_mode_info()`.
    pub fn write_intra_block_in_inter_ex(&mut self, dec: &BlockDecision, extra: IntraExtra) {
        let (r, c) = (dec.mi_row as usize, dec.mi_col as usize);
        let (bw4, bh4) = (dec.w4 as usize, dec.h4 as usize);
        assert!(bw4 >= 2 && bh4 >= 2, "blocks of inter frames are at least 8x8");
        let ms = block_size_of(bw4, bh4).unwrap_or_else(|| panic!("no block size {bw4}x{bh4}"));
        assert!(r < self.mi_rows && c < self.mi_cols, "block outside the frame");
        let info = BlockInfo { mi_row: r, mi_col: c, mi_size: ms, has_chroma: true, y_mode: dec.y_mode, uv_mode: dec.uv_mode, skip: dec.skip, tx_size: dec.tx_size, extra };
        write_intra_block_in_inter_frame(&mut self.enc, &mut self.cdfs.cdfs, &mut self.ctx, &info, self.frame);
        if dec.skip {
            self.ctx.reset_block_context(r, c, bw4, bh4, true);
        } else {
            self.write_residual(&info, dec);
        }
        let tx = if self.frame.lossless { TX_4X4 } else { dec.tx_size };
        self.ctx.store_block(r, c, ms, dec.y_mode, dec.skip, tx);
    }

    /// Writes one superblock of an inter frame, blocks in decode order; see
    /// [`write_sb`](Self::write_sb).
    pub fn write_inter_sb(&mut self, sb: SbPos, dec: &InterSbDecision) {
        if sb.col == 0 {
            self.ctx.clear_left();
        }
        self.write_lr(sb);
        let shapes: Vec<Leaf> = dec.blocks.iter().map(BlockMode::shape).collect();
        let sb4 = NUM_4X4_BLOCKS_WIDE[self.sb_size] as usize;
        let mut cur = 0;
        let mut emit = |w: &mut TileWriter, i: usize| match &dec.blocks[i] {
            BlockMode::Inter(d) => w.write_inter_block(d),
            BlockMode::Intra(b) => w.write_intra_block_in_inter(b),
        };
        self.write_tree(&shapes, &mut cur, sb.row as usize * sb4, sb.col as usize * sb4, self.sb_size, &mut emit);
        assert_eq!(cur, shapes.len(), "blocks left over after the partition tree");
    }

    /// Writes one superblock: its partition tree and blocks. `dec.blocks` lists the blocks in
    /// decode order; the partition at every node is inferred from their shapes. Superblocks must
    /// come in raster order.
    ///
    /// # Panics
    /// If the blocks do not form a valid partition of the superblock.
    pub fn write_sb(&mut self, sb: SbPos, dec: &SbDecision) {
        self.write_sb_ex(sb, dec, &[]);
    }

    /// Like [`write_sb`](Self::write_sb), with the angle deltas of each block (blocks past the
    /// end of `extras` have none).
    pub fn write_sb_ex(&mut self, sb: SbPos, dec: &SbDecision, extras: &[IntraExtra]) {
        if sb.col == 0 {
            self.ctx.clear_left();
        }
        self.write_lr(sb);
        let shapes: Vec<Leaf> = dec.blocks.iter().map(|b| (b.mi_row as usize, b.mi_col as usize, b.w4 as usize, b.h4 as usize)).collect();
        let sb4 = NUM_4X4_BLOCKS_WIDE[self.sb_size] as usize;
        let mut cur = 0;
        let mut emit = |w: &mut TileWriter, i: usize| w.write_block_ex(&dec.blocks[i], extras.get(i).copied().unwrap_or_default());
        self.write_tree(&shapes, &mut cur, sb.row as usize * sb4, sb.col as usize * sb4, self.sb_size, &mut emit);
        assert_eq!(cur, dec.blocks.len(), "blocks left over after the partition tree");
    }

    /// Walks the partition tree of the `bsize` node at `(r, c)`, inferring each partition from the
    /// shapes of the blocks that follow, and calls `emit` with the index of each block in decode
    /// order.
    fn write_tree(&mut self, shapes: &[Leaf], cur: &mut usize, r: usize, c: usize, bsize: usize, emit: &mut dyn FnMut(&mut TileWriter, usize)) {
        if r >= self.mi_rows || c >= self.mi_cols {
            return;
        }
        let num4x4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let half = num4x4 >> 1;
        let has_rows = r + half < self.mi_rows;
        let has_cols = c + half < self.mi_cols;
        let mut partition = PARTITION_SPLIT;
        let mut leaves: Vec<Leaf> = Vec::new();
        if bsize < BLOCK_8X8 {
            partition = PARTITION_NONE;
            leaves = vec![(r, c, num4x4, NUM_4X4_BLOCKS_HIGH[bsize] as usize)];
        } else {
            let allowed: &[usize] = if has_rows && has_cols {
                match bsize {
                    BLOCK_8X8 => &[PARTITION_NONE, PARTITION_HORZ, PARTITION_VERT],
                    BLOCK_128X128 => &[PARTITION_NONE, PARTITION_HORZ, PARTITION_VERT, PARTITION_HORZ_A, PARTITION_HORZ_B, PARTITION_VERT_A, PARTITION_VERT_B],
                    _ => &[PARTITION_NONE, PARTITION_HORZ, PARTITION_VERT, PARTITION_HORZ_A, PARTITION_HORZ_B, PARTITION_VERT_A, PARTITION_VERT_B, PARTITION_HORZ_4, PARTITION_VERT_4],
                }
            } else if has_cols {
                &[PARTITION_HORZ]
            } else if has_rows {
                &[PARTITION_VERT]
            } else {
                &[]
            };
            for &p in allowed {
                let l = self.leaves(p, r, c, bsize);
                if !l.is_empty() && shapes[*cur..].len() >= l.len() && l.iter().zip(&shapes[*cur..]).all(|(a, b)| a == b) {
                    partition = p;
                    leaves = l;
                    break;
                }
            }
        }
        self.write_partition(r as u32, c as u32, bsize, partition);
        if partition == PARTITION_SPLIT {
            let sub = PARTITION_SUBSIZE[PARTITION_SPLIT][bsize] as usize;
            for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
                self.write_tree(shapes, cur, r + dr, c + dc, sub, emit);
            }
        } else {
            for _ in &leaves {
                assert!(*cur < shapes.len(), "partition at ({r},{c}) ends past the last block");
                emit(self, *cur);
                *cur += 1;
            }
        }
    }

    /// The blocks a non-split `partition` of the `bsize` block at `(r, c)` decodes, in order,
    /// without those outside the frame (`decode_partition()`).
    fn leaves(&self, partition: usize, r: usize, c: usize, bsize: usize) -> Vec<Leaf> {
        let half = (NUM_4X4_BLOCKS_WIDE[bsize] as usize) >> 1;
        let quarter = half >> 1;
        let has_rows = r + half < self.mi_rows;
        let has_cols = c + half < self.mi_cols;
        let sub = PARTITION_SUBSIZE[partition][bsize] as usize;
        let split = PARTITION_SUBSIZE[PARTITION_SPLIT][bsize] as usize;
        if sub >= BLOCK_SIZES || split >= BLOCK_SIZES {
            return Vec::new();
        }
        let leaf = |rr: usize, cc: usize, s: usize| (rr, cc, NUM_4X4_BLOCKS_WIDE[s] as usize, NUM_4X4_BLOCKS_HIGH[s] as usize);
        match partition {
            PARTITION_NONE => vec![leaf(r, c, sub)],
            PARTITION_HORZ => {
                let mut v = vec![leaf(r, c, sub)];
                if has_rows {
                    v.push(leaf(r + half, c, sub));
                }
                v
            }
            PARTITION_VERT => {
                let mut v = vec![leaf(r, c, sub)];
                if has_cols {
                    v.push(leaf(r, c + half, sub));
                }
                v
            }
            PARTITION_HORZ_A => vec![leaf(r, c, split), leaf(r, c + half, split), leaf(r + half, c, sub)],
            PARTITION_HORZ_B => vec![leaf(r, c, sub), leaf(r + half, c, split), leaf(r + half, c + half, split)],
            PARTITION_VERT_A => vec![leaf(r, c, split), leaf(r + half, c, split), leaf(r, c + half, sub)],
            PARTITION_VERT_B => vec![leaf(r, c, sub), leaf(r, c + half, split), leaf(r + half, c + half, split)],
            PARTITION_HORZ_4 => (0..4).filter(|&k| k < 3 || r + quarter * k < self.mi_rows).map(|k| leaf(r + quarter * k, c, sub)).collect(),
            PARTITION_VERT_4 => (0..4).filter(|&k| k < 3 || c + quarter * k < self.mi_cols).map(|k| leaf(r, c + quarter * k, sub)).collect(),
            _ => Vec::new(),
        }
    }

    /// The finished tile data.
    /// The mode info of the frame coded so far.
    pub fn mode_info(&self) -> &studio_av1::frame::MiInfo {
        &self.ctx.mi
    }

    pub fn finish(self) -> Vec<u8> {
        self.enc.finish()
    }
}

/// `get_tx_size()`: the transform size of `plane` given the block's luma transform size.
pub fn uv_aware_tx_size(mi_size: usize, plane: usize, luma_tx: usize) -> usize {
    if plane == 0 {
        return luma_tx;
    }
    let uv_tx = MAX_TX_SIZE_RECT[plane_residual_size(mi_size, plane)] as usize;
    if TX_WIDTH[uv_tx] == 64 || TX_HEIGHT[uv_tx] == 64 {
        if TX_WIDTH[uv_tx] == 16 {
            return TX_16X32;
        }
        if TX_HEIGHT[uv_tx] == 16 {
            return TX_32X16;
        }
        return TX_32X32;
    }
    uv_tx
}
