// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M6: coding one intra block: prediction, residual, transform, quantisation, reconstruction, and
//! the intra mode search.
//!
//! A block uses one transform per plane (the largest the block allows), so its prediction does not
//! depend on its own residual. That lets a candidate mode be predicted by reconstructing the block
//! as a skip block, reading the prediction back from the reconstruction buffer, and finally
//! reconstructing the chosen decision with its coefficients.

use studio_av1::spec_tables::*;

use super::partition::Shape;
use super::rdcost::{coef_bits, lambda, lambda_satd, mode_bits, satd};
use crate::recon::FrameBuf;
use crate::transform::forward_transform;
use crate::transform::quant::quantize_block;
use crate::types::{BlockDecision, CoefBlock, Frame, QuantParams};

fn block_size_of(w4: usize, h4: usize) -> usize {
    (0..BLOCK_SIZES).find(|&b| NUM_4X4_BLOCKS_WIDE[b] as usize == w4 && NUM_4X4_BLOCKS_HIGH[b] as usize == h4).unwrap_or_else(|| panic!("no block size {w4}x{h4}"))
}

fn find_tx_size(w: usize, h: usize) -> usize {
    (0..TX_SIZES_ALL).find(|&t| TX_WIDTH[t] as usize == w && TX_HEIGHT[t] as usize == h).unwrap_or_else(|| panic!("no transform size {w}x{h}"))
}

/// Where a block lives: luma and chroma rectangles in samples.
struct Geometry {
    w4: usize,
    h4: usize,
    has_chroma: bool,
    /// `(x, y, w, h)` per plane.
    rect: [(usize, usize, usize, usize); 3],
    tx: [usize; 3],
}

impl Geometry {
    fn new(r: usize, c: usize, w4: usize, h4: usize) -> Geometry {
        let ms = block_size_of(w4, h4);
        let has_chroma = !((h4 == 1 && r & 1 == 0) || (w4 == 1 && c & 1 == 0));
        let (bw, bh) = (w4 * 4, h4 * 4);
        let (cw, ch) = ((bw / 2).max(4), (bh / 2).max(4));
        let luma = (c * 4, r * 4, bw, bh);
        let chroma = ((c >> 1) * 4, (r >> 1) * 4, cw, ch);
        // the decoder's get_tx_size(): chroma transforms are capped at 32
        let ctx = find_tx_size(cw.min(32), ch.min(32));
        Geometry { w4, h4, has_chroma, rect: [luma, chroma, chroma], tx: [MAX_TX_SIZE_RECT[ms] as usize, ctx, ctx] }
    }

    fn planes(&self) -> usize {
        1 + 2 * self.has_chroma as usize
    }
}

/// The residual `source - prediction` of a plane rectangle. With `pad`, samples outside the
/// picture take the nearest source sample (edge replication, which keeps the block smooth for the
/// transform); without it they count as perfectly predicted (for measuring errors).
fn residual(src: &Frame, recon: &FrameBuf, plane: usize, rect: (usize, usize, usize, usize), pad: bool) -> Vec<i16> {
    let (x0, y0, w, h) = rect;
    let (pw, ph) = if plane == 0 { (src.width as usize, src.height as usize) } else { ((src.width as usize + 1) >> 1, (src.height as usize + 1) >> 1) };
    let mut out = vec![0i16; w * h];
    for y in 0..h {
        let sy = y0 + y;
        if sy >= ph && !pad {
            break;
        }
        let s = &src.planes[plane][sy.min(ph - 1) * src.stride[plane]..];
        let p = recon.planes[plane].row_from(sy, x0);
        for x in 0..w {
            let sx = x0 + x;
            if sx >= pw && !pad {
                break;
            }
            out[y * w + x] = s[sx.min(pw - 1)] as i16 - p[x] as i16;
        }
    }
    out
}

/// A decision with no coefficients (`skip`): reconstructing it leaves the prediction in `recon`.
fn predict(recon: &mut FrameBuf, r: usize, c: usize, g: &Geometry, y_mode: usize, uv_mode: usize) -> BlockDecision {
    let d = BlockDecision { mi_row: r as u32, mi_col: c as u32, w4: g.w4 as u32, h4: g.h4 as u32, y_mode, uv_mode, tx_size: g.tx[0], skip: true, coefs: vec![] };
    recon.reconstruct_block(&d);
    d
}

/// The transform set of an intra block's transform size (`get_tx_set`, intra, no reduced set).
fn intra_tx_set(tx: usize) -> usize {
    if TX_SIZE_SQR_UP[tx] as usize >= TX_32X32 {
        TX_SET_DCTONLY
    } else if TX_SIZE_SQR[tx] as usize == TX_16X16 {
        TX_SET_INTRA_2
    } else {
        TX_SET_INTRA_1
    }
}

/// The transform type of a plane: luma uses `DCT_DCT`, chroma the one the decoder derives from
/// the uv mode (when the transform's set has it).
fn tx_type(plane: usize, tx: usize, uv_mode: usize) -> usize {
    if plane == 0 {
        return DCT_DCT;
    }
    let t = MODE_TO_TXFM[uv_mode] as usize;
    if TX_TYPE_IN_SET_INTRA[intra_tx_set(tx)][t] == 1 { t } else { DCT_DCT }
}

/// Codes the block at `(r, c)` with the given modes: predicts, transforms and quantises the
/// residual, reconstructs into `recon`, and returns the decision. A block whose coefficients are
/// all zero becomes a skip block.
pub fn code_block(src: &Frame, recon: &mut FrameBuf, shape: Shape, y_mode: usize, uv_mode: usize) -> BlockDecision {
    let (r, c, w4, h4) = shape;
    assert!(recon.quant.base_q_idx > 0, "the intra coder does not handle lossless frames");
    let g = Geometry::new(r, c, w4, h4);
    let mut d = predict(recon, r, c, &g, y_mode, uv_mode);
    let mut coefs: Vec<CoefBlock> = Vec::with_capacity(3);
    for plane in 0..g.planes() {
        let tx = g.tx[plane];
        let ty = tx_type(plane, tx, uv_mode);
        let res = residual(src, recon, plane, g.rect[plane], true);
        let mut freq = vec![0i32; 64 * 64];
        forward_transform(&res, tx, ty, &mut freq);
        let qp = QuantParams { base_q_idx: recon.quant.base_q_idx, bit_depth: recon.bit_depth, plane: plane as u8 };
        coefs.push(quantize_block(&freq, tx, ty, plane as u8, qp));
    }
    if coefs.iter().any(|b| b.eob > 0) {
        d.skip = false;
        d.coefs = coefs;
        recon.reconstruct_block(&d);
    }
    d
}

/// The cost of predicting plane `plane` of the block with the modes now in `recon`.
fn plane_cost(src: &Frame, recon: &FrameBuf, g: &Geometry, plane: usize) -> u64 {
    let rect = g.rect[plane];
    satd(&residual(src, recon, plane, rect, true), rect.2, rect.3)
}

/// Squared error of a plane rectangle of the reconstruction (samples inside the picture only).
fn plane_sse(src: &Frame, recon: &FrameBuf, plane: usize, rect: (usize, usize, usize, usize)) -> f64 {
    // the reconstruction now holds prediction + residual, so `residual` is the coding error
    residual(src, recon, plane, rect, false).iter().map(|&e| (e as f64) * (e as f64)).sum()
}

/// Rate-distortion cost of coding the block with the given modes, over the planes in `planes`
/// (squared error plus lambda times an estimate of the bits). Leaves the candidate in `recon`.
fn rd_cost(src: &Frame, recon: &mut FrameBuf, shape: Shape, y_mode: usize, uv_mode: usize, planes: std::ops::Range<usize>) -> f64 {
    let g = Geometry::new(shape.0, shape.1, shape.2, shape.3);
    let d = code_block(src, recon, shape, y_mode, uv_mode);
    let mut cost = 0.0;
    let lam = lambda(recon.quant.base_q_idx, recon.bit_depth);
    for plane in planes {
        if plane >= g.planes() {
            continue;
        }
        cost += plane_sse(src, recon, plane, g.rect[plane]);
        let bits = d.coefs.iter().find(|b| b.plane as usize == plane).map_or(0.5, |b| coef_bits(&b.levels, b.eob));
        cost += lam * bits;
    }
    cost
}

/// Codes the block with the best modes the search finds and returns the decision with its
/// rate-distortion cost over all planes: squared error plus lambda times the estimated bits of the
/// coefficients and the modes. The block is left reconstructed in `recon`.
pub fn code_block_rd(src: &Frame, recon: &mut FrameBuf, shape: Shape, search: bool) -> (BlockDecision, f64) {
    let (y_mode, uv_mode) = if search { search_modes(src, recon, shape) } else { (DC_PRED, DC_PRED) };
    let g = Geometry::new(shape.0, shape.1, shape.2, shape.3);
    let d = code_block(src, recon, shape, y_mode, uv_mode);
    let lam = lambda(recon.quant.base_q_idx, recon.bit_depth);
    let mut cost = lam * (mode_bits(y_mode) + if g.has_chroma { mode_bits(uv_mode) } else { 0.0 });
    for plane in 0..g.planes() {
        cost += plane_sse(src, recon, plane, g.rect[plane]);
        cost += lam * d.coefs.iter().find(|b| b.plane as usize == plane).map_or(0.5, |b| coef_bits(&b.levels, b.eob));
    }
    (d, cost)
}

/// How many of the best SATD candidates are coded in full to find the winner.
const FINALISTS: usize = 3;

/// The candidates sorted by `score` (smaller first), cut to `FINALISTS`.
fn finalists(mut scored: Vec<(f64, usize)>) -> Vec<usize> {
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(FINALISTS).map(|(_, m)| m).collect()
}

/// Picks the luma and chroma modes of the block at `(r, c)` among the 13 intra modes (no angle
/// deltas). A SATD screen of the prediction residual keeps the few best modes, which are then
/// coded in full and compared by squared error plus lambda times the estimated bits.
pub fn search_modes(src: &Frame, recon: &mut FrameBuf, shape: Shape) -> (usize, usize) {
    let (r, c, w4, h4) = shape;
    let g = Geometry::new(r, c, w4, h4);
    let lam = lambda_satd(recon.quant.base_q_idx, recon.bit_depth);
    let mut scored = Vec::new();
    for mode in DC_PRED..=PAETH_PRED {
        predict(recon, r, c, &g, mode, DC_PRED);
        scored.push((plane_cost(src, recon, &g, 0) as f64 + lam * mode_bits(mode), mode));
    }
    let mut cands = finalists(scored);
    if !cands.contains(&DC_PRED) {
        cands.push(DC_PRED);
    }
    let y_mode = cands.into_iter().map(|m| (rd_cost(src, recon, shape, m, DC_PRED, 0..1) + lambda(recon.quant.base_q_idx, recon.bit_depth) * mode_bits(m), m)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap().1;
    if !g.has_chroma {
        return (y_mode, DC_PRED);
    }
    let mut scored = Vec::new();
    for mode in DC_PRED..=PAETH_PRED {
        predict(recon, r, c, &g, y_mode, mode);
        scored.push(((plane_cost(src, recon, &g, 1) + plane_cost(src, recon, &g, 2)) as f64 + lam * mode_bits(mode), mode));
    }
    let mut cands = finalists(scored);
    if !cands.contains(&DC_PRED) {
        cands.push(DC_PRED);
    }
    let uv_mode = cands.into_iter().map(|m| (rd_cost(src, recon, shape, y_mode, m, 1..3) + lambda(recon.quant.base_q_idx, recon.bit_depth) * mode_bits(m), m)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap().1;
    (y_mode, uv_mode)
}
