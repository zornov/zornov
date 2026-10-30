// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M7 (phase b): motion search and the mode decision of inter frames.
//!
//! [`InterFrameCoder`] decides the blocks of one inter frame at a fixed quantiser: a uniform block
//! size (the fixed partition of phase (a), cut to `block4`), a motion search against the previous
//! reconstruction, and for each block a comparison of `NEARESTMV`, `GLOBALMV`, `NEARMV` and the
//! searched `NEWMV`, and intra prediction (`DC_PRED` first, the mode search when that is
//! competitive), by squared error plus lambda times the estimated bits. Every candidate is coded
//! in full (prediction, transform, quantisation, reconstruction), so the winner's reconstruction
//! is exactly what the decoder will produce.
//!
//! Limits of this version: LAST as the only reference, blocks of 8x8 and up, `TX_MODE_LARGEST`,
//! luma `DCT_DCT` for inter blocks, and `base_q_idx > 0`.

pub mod mvpred;
pub mod search;

use std::sync::Arc;

use studio_av1::mvpred::MvFrame;
use studio_av1::spec_tables::*;

use crate::mode::intra as intra_mode;
use crate::mode::partition::Shape;
use crate::mode::rdcost::{coef_bits, lambda, mode_bits as intra_mode_bits};
use crate::recon::{FrameBuf, RefStore};
use crate::syntax::ctx::ContextState;
use crate::transform::forward_transform;
use crate::transform::quant::quantize_block;
use crate::types::{BlockMode, CoefBlock, Frame, InterDecision, InterSbDecision, QuantParams, SbPos};
use mvpred::{find_stack_in_tile, frame_info, predicted_mv};
use search::{Matcher, mv_bits};

/// Bits charged for a partition symbol when comparing a block with its split.
const PARTITION_BITS: f64 = 2.0;

/// A motion vector in 1/8 pel units: `[row, col]`.
pub type Mv = [i32; 2];

/// Superblock size in 4x4 units.
const SB4: usize = 16;

/// The blocks of the uniform partition into `block4` x `block4` blocks (in 4x4 units) of the
/// node at `(r, c)`, with the edge handling of the fixed partition.
fn uniform_partition(r: usize, c: usize, n4: usize, block4: usize, mi_rows: usize, mi_cols: usize, out: &mut Vec<Shape>) {
    if r >= mi_rows || c >= mi_cols {
        return;
    }
    let half = n4 / 2;
    let has_rows = r + half < mi_rows;
    let has_cols = c + half < mi_cols;
    if n4 <= block4 && has_rows && has_cols {
        out.push((r, c, n4, n4));
    } else if n4 <= block4 && has_cols {
        out.push((r, c, n4, half));
    } else if n4 <= block4 && has_rows {
        out.push((r, c, half, n4));
    } else {
        for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
            uniform_partition(r + dr, c + dc, half, block4, mi_rows, mi_cols, out);
        }
    }
}

fn block_size_of(w4: usize, h4: usize) -> usize {
    (0..BLOCK_SIZES).find(|&b| NUM_4X4_BLOCKS_WIDE[b] as usize == w4 && NUM_4X4_BLOCKS_HIGH[b] as usize == h4).unwrap_or_else(|| panic!("no block size {w4}x{h4}"))
}

fn find_tx_size(w: usize, h: usize) -> usize {
    (0..TX_SIZES_ALL).find(|&t| TX_WIDTH[t] as usize == w && TX_HEIGHT[t] as usize == h).unwrap_or_else(|| panic!("no transform size {w}x{h}"))
}

/// Decides the blocks of one inter frame, superblock by superblock in raster order.
pub struct InterFrameCoder<'a> {
    src: &'a Frame,
    refs: &'a RefStore,
    ref_idx: [u8; 7],
    reference: Arc<FrameBuf>,
    mi_rows: usize,
    mi_cols: usize,
    /// The block size in 4x4 units, or 0 to try 64x64, 32x32 and 16x16 in every superblock.
    block4: usize,
    /// Run the wide integer search (slower) rather than only the predicted starts and a local one.
    full_search: bool,
    /// Smallest block the partition search splits down to, in 4x4 units (2 is 8x8).
    min_n4: usize,
    /// Mirror of the mode info the tile writer keeps, for the candidate stacks.
    ctx: ContextState,
    mv_frame: MvFrame,
}

impl<'a> InterFrameCoder<'a> {
    /// `refs.slots[ref_idx[0]]` is the reference (LAST) the frame predicts from. `block4` is the
    /// block size in 4x4 units (2, 4, 8 or 16), or 0 to choose among 16, 8 and 4 for every
    /// superblock by rate-distortion cost.
    pub fn new(src: &'a Frame, refs: &'a RefStore, ref_idx: [u8; 7], block4: usize, full_search: bool) -> InterFrameCoder<'a> {
        assert!(matches!(block4, 0 | 2 | 4 | 8 | 16));
        let reference = refs.slots[ref_idx[0] as usize].clone().expect("the LAST reference slot is empty");
        let (mi_cols, mi_rows) = (2 * ((src.width as usize + 7) >> 3), 2 * ((src.height as usize + 7) >> 3));
        InterFrameCoder { src, refs, ref_idx, reference, mi_rows, mi_cols, block4, full_search, min_n4: 2, ctx: ContextState::new(mi_cols, mi_rows), mv_frame: frame_info(mi_rows, mi_cols) }
    }

    /// Chooses and reconstructs the blocks of one superblock.
    pub fn decide_sb(&mut self, recon: &mut FrameBuf, sb: SbPos) -> InterSbDecision {
        assert!(recon.quant.base_q_idx > 0, "the inter coder does not handle lossless frames");
        let (r0, c0) = (sb.row as usize * SB4, sb.col as usize * SB4);
        if self.block4 != 0 {
            recon.begin_sb(sb);
            return self.code_sb(recon, r0, c0, self.block4).0;
        }
        // search the partition: every node is coded whole and split into four, the cheaper kept
        recon.begin_sb(sb);
        let mut blocks = Vec::new();
        self.search_node(recon, r0, c0, SB4, &mut blocks);
        InterSbDecision { blocks }
    }

    /// Searches the partition of the `n4` x `n4` node at `(r, c)`: the block whole or split into
    /// four, recursively down to 8x8, whichever costs less. The winner is left coded in `recon`
    /// and in the mode info; its blocks are appended to `out` in decode order. Returns the cost.
    fn search_node(&mut self, recon: &mut FrameBuf, r: usize, c: usize, n4: usize, out: &mut Vec<BlockMode>) -> f64 {
        if r >= self.mi_rows || c >= self.mi_cols {
            return 0.0;
        }
        let half = n4 / 2;
        let lam = lambda(recon.quant.base_q_idx, recon.bit_depth);
        if !(r + half < self.mi_rows && c + half < self.mi_cols) {
            // past the frame edge the partition is forced; the pieces that are full nodes are searched
            let mut shapes = Vec::new();
            uniform_partition(r, c, n4, n4, self.mi_rows, self.mi_cols, &mut shapes);
            if shapes.iter().all(|s| s.2 != s.3) || shapes.len() == 1 {
                let mut total = 0.0;
                for shape in shapes {
                    let (b, cost) = self.decide_block(recon, shape);
                    out.push(b);
                    total += cost;
                }
                return total;
            }
            let mut total = 0.0;
            for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
                total += self.search_node(recon, r + dr, c + dc, half, out);
            }
            return total;
        }
        let base_ctx = self.ctx.save_region(r, c, n4, n4);
        let base_rec = recon.save_region(r, c, n4);
        let (whole, whole_cost) = self.decide_block(recon, (r, c, n4, n4));
        let whole_cost = whole_cost + lam * PARTITION_BITS;
        if n4 <= self.min_n4 {
            out.push(whole);
            return whole_cost;
        }
        let after_ctx = self.ctx.save_region(r, c, n4, n4);
        let after_rec = recon.save_region(r, c, n4);
        self.ctx.restore_region(&base_ctx);
        recon.restore_region(&base_rec);
        let mut split = Vec::new();
        let mut split_cost = lam * PARTITION_BITS;
        for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
            split_cost += self.search_node(recon, r + dr, c + dc, half, &mut split);
        }
        if whole_cost <= split_cost {
            self.ctx.restore_region(&after_ctx);
            recon.restore_region(&after_rec);
            out.push(whole);
            whole_cost
        } else {
            out.extend(split);
            split_cost
        }
    }

    /// Codes the tile `[mi_row_start, mi_row_end)` x `[mi_col_start, mi_col_end)` only: neighbour
    /// availability and the vector candidate scan stop at its edges (positions stay frame
    /// coordinates; the caller visits the tile's superblocks).
    pub fn with_tile(mut self, mi_row0: usize, mi_row1: usize, mi_col0: usize, mi_col1: usize) -> Self {
        self.ctx.tile = [mi_row0, mi_row1.min(self.mi_rows), mi_col0, mi_col1.min(self.mi_cols)];
        self
    }

    /// Limits the partition search to blocks of at least `min_n4` x `min_n4` (4x4 units).
    pub fn with_min_block(mut self, min_n4: usize) -> Self {
        self.min_n4 = min_n4.max(2);
        self
    }

    /// Codes the superblock with uniform blocks of `block4`; returns the decisions and their total
    /// cost.
    fn code_sb(&mut self, recon: &mut FrameBuf, r0: usize, c0: usize, block4: usize) -> (InterSbDecision, f64) {
        let mut shapes = Vec::new();
        uniform_partition(r0, c0, SB4, block4, self.mi_rows, self.mi_cols, &mut shapes);
        let mut blocks = Vec::with_capacity(shapes.len());
        let mut total = 0.0;
        for shape in shapes {
            let (b, cost) = self.decide_block(recon, shape);
            total += cost;
            blocks.push(b);
        }
        (InterSbDecision { blocks }, total)
    }

    fn decide_block(&mut self, recon: &mut FrameBuf, shape: Shape) -> (BlockMode, f64) {
        let (r, c, w4, h4) = shape;
        let stack = find_stack_in_tile(&self.ctx.mi, &self.mv_frame, shape, 1, self.ctx.tile);
        let lam = lambda(recon.quant.base_q_idx, recon.bit_depth);
        // motion search in SAD units: one bit is worth about lambda / (SSE per SAD unit)
        let lam_sad = lam.sqrt() * 0.5;
        let nearest = predicted_mv(&stack, NEARESTMV, 0);
        let new_pred = predicted_mv(&stack, NEWMV, 0);
        let mut starts = vec![nearest, [0, 0]];
        if stack.num_mv_found >= 2 {
            starts.push(predicted_mv(&stack, NEARMV, 1));
        }
        let matcher = Matcher { src: self.src, reference: &self.reference.planes[0], x: c * 4, y: r * 4, w: w4 * 4, h: h4 * 4 };
        let (searched, _) = search::search(&matcher, &starts, new_pred, lam_sad, self.full_search);

        // candidates, cheapest signalling first; a vector is tried once under its cheapest mode
        let mut cands: Vec<(usize, usize, Mv)> = vec![(NEARESTMV, 0, nearest), (GLOBALMV, 0, [0, 0])];
        if stack.num_mv_found >= 2 {
            cands.push((NEARMV, 1, predicted_mv(&stack, NEARMV, 1)));
        }
        cands.push((NEWMV, 0, searched));
        let mut tried: Vec<Mv> = Vec::new();
        let mut best: Option<(f64, InterDecision)> = None;
        for (mode, idx, mv) in cands {
            if tried.contains(&mv) {
                continue;
            }
            tried.push(mv);
            let d = self.code_block(recon, shape, mode, idx, mv);
            let mode_bits = match mode {
                NEARESTMV => 1.0,
                GLOBALMV => 1.5,
                NEARMV => 2.5,
                _ => 3.0 + mv_bits([mv[0] - new_pred[0], mv[1] - new_pred[1]]),
            };
            let cost = self.sse(recon, shape) + lam * (mode_bits + self.coef_rate(&d));
            if best.as_ref().is_none_or(|b| cost < b.0) {
                best = Some((cost, d));
            }
        }
        let (inter_cost, inter) = best.expect("at least one candidate");

        // intra: DC first; the full mode search only when DC is in the running
        let ms = block_size_of(w4, h4);
        let intra_bits = |d: &crate::types::BlockDecision| 1.0 + intra_mode_bits(d.y_mode) + intra_mode_bits(d.uv_mode) + if d.skip { 0.5 } else { d.coefs.iter().map(|b| coef_bits(&b.levels, b.eob)).sum::<f64>() };
        let dc = intra_mode::code_block(self.src, recon, shape, DC_PRED, DC_PRED);
        let dc_cost = self.sse(recon, shape) + lam * intra_bits(&dc);
        let mut best_intra = (dc_cost, dc);
        if dc_cost < 1.5 * inter_cost {
            let (y, uv) = intra_mode::search_modes(self.src, recon, shape);
            if (y, uv) != (DC_PRED, DC_PRED) {
                let d = intra_mode::code_block(self.src, recon, shape, y, uv);
                let cost = self.sse(recon, shape) + lam * intra_bits(&d);
                if cost < best_intra.0 {
                    best_intra = (cost, d);
                }
            }
        }
        if best_intra.0 < inter_cost {
            // intra wins: put it back into the reconstruction (the last thing coded may differ)
            recon.reconstruct_block(&best_intra.1);
            let d = best_intra.1;
            self.ctx.store_block(r, c, ms, d.y_mode, d.skip, MAX_TX_SIZE_RECT[ms] as usize);
            return (BlockMode::Intra(d), best_intra.0);
        }
        // put the winner back into the reconstruction
        recon.reconstruct_inter_block(self.refs, &self.ref_idx, &inter);
        self.ctx.store_inter_block(r, c, ms, inter.mode, inter.skip, MAX_TX_SIZE_RECT[ms] as usize, inter.ref_frame as i8, inter.mv);
        (BlockMode::Inter(inter), inter_cost)
    }

    fn coef_rate(&self, d: &InterDecision) -> f64 {
        if d.skip { 0.5 } else { d.coefs.iter().map(|b| coef_bits(&b.levels, b.eob)).sum() }
    }

    /// Squared error of the current reconstruction of the block (inside the picture).
    fn sse(&self, recon: &FrameBuf, (r, c, w4, h4): Shape) -> f64 {
        let mut sse = 0.0;
        for plane in 0..3 {
            let sub = (plane > 0) as usize;
            let (pw, ph) = ((self.src.width as usize + sub) >> sub, (self.src.height as usize + sub) >> sub);
            let (x0, y0) = ((c * 4) >> sub, (r * 4) >> sub);
            let (w, h) = ((w4 * 4) >> sub, (h4 * 4) >> sub);
            for y in y0..(y0 + h).min(ph) {
                let s = &self.src.planes[plane][y * self.src.stride[plane]..];
                let p = recon.planes[plane].row(y);
                for x in x0..(x0 + w).min(pw) {
                    let e = s[x] as f64 - p[x] as f64;
                    sse += e * e;
                }
            }
        }
        sse
    }

    /// Codes the block with the given mode and vector: predicts it, quantises the residual and
    /// reconstructs into `recon`. A block with only zero coefficients becomes a skip block.
    fn code_block(&self, recon: &mut FrameBuf, (r, c, w4, h4): Shape, mode: usize, ref_mv_idx: usize, mv: Mv) -> InterDecision {
        let mut d = InterDecision { mi_row: r as u32, mi_col: c as u32, w4: w4 as u32, h4: h4 as u32, mode, ref_frame: 1, ref_mv_idx, mv, skip: true, coefs: vec![] };
        recon.reconstruct_inter_block(self.refs, &self.ref_idx, &d);
        let ms = block_size_of(w4, h4);
        let tx = [MAX_TX_SIZE_RECT[ms] as usize, 0, 0];
        let (cw, ch) = ((w4 * 4) / 2, (h4 * 4) / 2);
        let chroma_tx = find_tx_size(cw.min(32), ch.min(32));
        let mut coefs: Vec<CoefBlock> = Vec::with_capacity(3);
        for plane in 0..3 {
            let sub = (plane > 0) as usize;
            let (w, h) = ((w4 * 4) >> sub, (h4 * 4) >> sub);
            let (x0, y0) = ((c * 4) >> sub, (r * 4) >> sub);
            let (pw, ph) = ((self.src.width as usize + sub) >> sub, (self.src.height as usize + sub) >> sub);
            // source minus prediction, the picture edge replicated into the part of the block outside
            let mut res = vec![0i16; w * h];
            for y in 0..h {
                let s = &self.src.planes[plane][(y0 + y).min(ph - 1) * self.src.stride[plane]..];
                let p = recon.planes[plane].row_from(y0 + y, x0);
                for x in 0..w {
                    res[y * w + x] = s[(x0 + x).min(pw - 1)] as i16 - p[x] as i16;
                }
            }
            let t = if plane == 0 { tx[0] } else { chroma_tx };
            let mut freq = vec![0i32; 64 * 64];
            forward_transform(&res, t, DCT_DCT, &mut freq);
            let qp = QuantParams { base_q_idx: recon.quant.base_q_idx, bit_depth: recon.bit_depth, plane: plane as u8 };
            coefs.push(quantize_block(&freq, t, DCT_DCT, plane as u8, qp));
        }
        if coefs.iter().any(|b| b.eob > 0) {
            d.skip = false;
            d.coefs = coefs;
            recon.reconstruct_inter_block(self.refs, &self.ref_idx, &d);
        }
        d
    }
}
