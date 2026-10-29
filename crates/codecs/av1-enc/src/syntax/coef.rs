// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M4: coefficient writer, the inverse of the decoder's `coeffs` (spec 5.11.39), for intra and
//! inter blocks.

use studio_av1::cdf::Cdfs;
use studio_av1::spec_tables::*;

use super::ctx::*;
use crate::entropy::SymbolEncoder;
use crate::transform::quant::scan_for;
use crate::types::CoefBlock;

/// Where a transform block sits and what the block it belongs to decided.
#[derive(Debug, Clone, Copy)]
pub struct TxbParams {
    pub plane: usize,
    /// Position of the transform block in 4x4 units of its plane.
    pub x4: usize,
    pub y4: usize,
    /// The block's size (`MiSize`), from which the plane's block size follows.
    pub mi_size: usize,
    pub y_mode: usize,
    pub uv_mode: usize,
    /// `CodedLossless`: 4x4 WHT, no transform type is coded.
    pub lossless: bool,
    /// `base_q_idx > 0`; transform types are coded only then.
    pub qidx_nonzero: bool,
    /// An inter block: transform types come from the inter sets.
    pub is_inter: bool,
    /// The transform type of the block's luma transform (inter chroma follows it).
    pub luma_tx_type: usize,
}

/// The transform set of an intra block (`get_tx_set`, `reduced_tx_set = 0`).
pub fn intra_tx_set(tx_sz: usize) -> usize {
    let sqr = TX_SIZE_SQR[tx_sz] as usize;
    let sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
    if sqr_up >= TX_32X32 {
        TX_SET_DCTONLY
    } else if sqr == TX_16X16 {
        TX_SET_INTRA_2
    } else {
        TX_SET_INTRA_1
    }
}

/// The transform set of an inter block (`get_tx_set`, `reduced_tx_set = 0`).
pub fn inter_tx_set(tx_sz: usize) -> usize {
    let sqr = TX_SIZE_SQR[tx_sz] as usize;
    let sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
    if sqr_up > TX_32X32 {
        TX_SET_DCTONLY
    } else if sqr_up == TX_32X32 {
        TX_SET_INTER_3
    } else if sqr == TX_16X16 {
        TX_SET_INTER_2
    } else {
        TX_SET_INTER_1
    }
}

fn tx_set(is_inter: bool, tx_sz: usize) -> usize {
    if is_inter { inter_tx_set(tx_sz) } else { intra_tx_set(tx_sz) }
}

fn type_in_set(is_inter: bool, set: usize, t: usize) -> bool {
    if is_inter { TX_TYPE_IN_SET_INTER[set][t] == 1 } else { TX_TYPE_IN_SET_INTRA[set][t] == 1 }
}

/// The transform type the decoder uses for the transform block, given what the block codes
/// (`compute_tx_type`). `luma_type` is the type coded for luma (the block's only luma transform):
/// inter chroma takes it over when its set has it, intra chroma derives one from the uv mode.
pub fn plane_tx_type(p: &TxbParams, tx_sz: usize, luma_type: usize) -> usize {
    let sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
    if p.lossless || sqr_up > TX_32X32 {
        return DCT_DCT;
    }
    let set = tx_set(p.is_inter, tx_sz);
    if p.plane == 0 {
        if set > 0 && p.qidx_nonzero { luma_type } else { DCT_DCT }
    } else {
        let t = if p.is_inter { luma_type } else { MODE_TO_TXFM[p.uv_mode] as usize };
        if type_in_set(p.is_inter, set, t) { t } else { DCT_DCT }
    }
}

/// Writes the `all_zero` flag, the luma transform type, the end of block, the levels, the signs
/// and the Golomb tails of one transform block, then updates the coefficient contexts.
///
/// `block.levels` is indexed `row * tw + col` (`tw = min(width, 32)`, the decoder's `Quant`
/// layout); `block.eob` is the scan index after the last nonzero level, and `block.tx_type` the
/// type the block uses (for chroma it must be the one the decoder derives from `uv_mode`).
///
/// # Panics
/// If `block` is inconsistent: a wrong `eob`, a type outside the set, a chroma type that is not
/// the derived one.
pub fn write_coefficients(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, ctx: &mut ContextState, p: &TxbParams, block: &CoefBlock) {
    let tx_sz = block.tx_size;
    let plane = p.plane;
    assert_eq!(block.plane as usize, plane, "coefficient block of the wrong plane");
    let (x4, y4) = (p.x4, p.y4);
    let tx_sz_ctx = (TX_SIZE_SQR[tx_sz] as usize + TX_SIZE_SQR_UP[tx_sz] as usize + 1) >> 1;
    let ptype = (plane > 0) as usize;
    let eob = block.eob;
    let azctx = ctx.all_zero_ctx(plane, tx_sz, x4, y4, p.mi_size);
    enc.write_symbol(&mut cdfs.txb_skip[tx_sz_ctx][azctx], (eob == 0) as usize);
    let mut cul_level = 0u32;
    let mut dc_category = 0u8;
    if eob > 0 {
        let set = tx_set(p.is_inter, tx_sz);
        if plane == 0 && set > 0 && p.qidx_nonzero && !p.lossless {
            let sqr = TX_SIZE_SQR[tx_sz] as usize;
            assert!(type_in_set(p.is_inter, set, block.tx_type), "transform type {} not in set {set}", block.tx_type);
            let position = |inv: &[u8]| inv.iter().position(|&t| t as usize == block.tx_type).unwrap();
            if p.is_inter {
                match set {
                    TX_SET_INTER_1 => enc.write_symbol(&mut cdfs.inter_tx_type_set1[sqr], position(&TX_TYPE_INTER_INV_SET1)),
                    TX_SET_INTER_2 => enc.write_symbol(&mut cdfs.inter_tx_type_set2, position(&TX_TYPE_INTER_INV_SET2)),
                    _ => enc.write_symbol(&mut cdfs.inter_tx_type_set3[sqr], position(&TX_TYPE_INTER_INV_SET3)),
                }
            } else if set == TX_SET_INTRA_1 {
                enc.write_symbol(&mut cdfs.intra_tx_type_set1[sqr][p.y_mode], position(&TX_TYPE_INTRA_INV_SET1));
            } else {
                enc.write_symbol(&mut cdfs.intra_tx_type_set2[sqr][p.y_mode], position(&TX_TYPE_INTRA_INV_SET2));
            }
        }
        let ptt = plane_tx_type(p, tx_sz, if plane == 0 { block.tx_type } else { p.luma_tx_type });
        assert_eq!(block.tx_type, ptt, "plane {plane} block must use transform type {ptt}, not {}", block.tx_type);
        let scan = scan_for(tx_sz, ptt);
        let tx_class = get_tx_class(ptt);

        // The level buffer has the decoder's layout: coded dimensions plus TX_PAD zero columns
        // and rows, so neighbour sums need no bounds checks.
        let adj = ADJUSTED_TX_SIZE[tx_sz] as usize;
        let bwl = TX_WIDTH_LOG2[adj] as usize;
        let height = TX_HEIGHT[adj] as usize;
        let width = 1usize << bwl;
        assert!(block.levels.len() >= width * height, "levels shorter than the coded block");
        assert!(eob <= width * height);
        assert!(block.levels[scan[eob - 1] as usize] != 0, "eob must end on a nonzero level");
        assert!((eob..width * height).all(|c| block.levels[scan[c] as usize] == 0), "nonzero level past eob");

        // End of block: eob_pt, then the extra bit and the literal bits below it.
        let eob_pt = if eob <= 2 { eob } else { (usize::BITS - (eob - 1).leading_zeros()) as usize + 1 };
        let eob_multisize = (TX_WIDTH_LOG2[tx_sz] as usize).min(5) + (TX_HEIGHT_LOG2[tx_sz] as usize).min(5) - 4;
        let ectx = (tx_class != TX_CLASS_2D) as usize;
        let sym = eob_pt - 1;
        match eob_multisize {
            0 => enc.write_symbol(&mut cdfs.eob_pt_16[ptype][ectx], sym),
            1 => enc.write_symbol(&mut cdfs.eob_pt_32[ptype][ectx], sym),
            2 => enc.write_symbol(&mut cdfs.eob_pt_64[ptype][ectx], sym),
            3 => enc.write_symbol(&mut cdfs.eob_pt_128[ptype][ectx], sym),
            4 => enc.write_symbol(&mut cdfs.eob_pt_256[ptype][ectx], sym),
            5 => enc.write_symbol(&mut cdfs.eob_pt_512[ptype], sym),
            _ => enc.write_symbol(&mut cdfs.eob_pt_1024[ptype], sym),
        }
        if eob_pt >= 3 {
            let rem = eob - ((1usize << (eob_pt - 2)) + 1);
            let shift = eob_pt - 3;
            enc.write_symbol(&mut cdfs.eob_extra[tx_sz_ctx][ptype][eob_pt - 3], (rem >> shift) & 1);
            for sh in (0..shift).rev() {
                enc.write_literal(((rem >> sh) & 1) as u32, 1);
            }
        }

        // Levels, last scan position first.
        let stride = width + TX_PAD;
        let mut lev = vec![0u8; (height + TX_PAD) * stride];
        let sig = &SIG_REF_DIFF_OFFSET[tx_class];
        let sig_off: [usize; SIG_REF_DIFF_OFFSET_NUM] = std::array::from_fn(|i| sig[i][0] as usize * stride + sig[i][1] as usize);
        let magr = &MAG_REF_OFFSET_WITH_TX_CLASS[tx_class];
        let mag_off: [usize; 3] = std::array::from_fn(|i| magr[i][0] as usize * stride + magr[i][1] as usize);
        for c in (0..eob).rev() {
            let pos = scan[c] as usize;
            let row = pos >> bwl;
            let col = pos & (width - 1);
            let li = row * stride + col;
            let level = block.levels[pos].unsigned_abs() as usize;
            if c == eob - 1 {
                let bctx = coeff_base_eob_ctx(c, bwl, height);
                enc.write_symbol(&mut cdfs.coeff_base_eob[tx_sz_ctx][ptype][bctx], level.min(3) - 1);
            } else {
                let mut mag = 0u32;
                for o in sig_off {
                    mag += (lev[li + o] as u32).min(3);
                }
                let bctx = coeff_base_ctx(tx_sz, tx_class, row, col, mag);
                enc.write_symbol(&mut cdfs.coeff_base[tx_sz_ctx][ptype][bctx], level.min(3));
            }
            let mut coded = level.min(NUM_BASE_LEVELS + 1);
            if level > NUM_BASE_LEVELS {
                let mut mag = 0u32;
                for o in mag_off {
                    mag += (lev[li + o] as u32).min((COEFF_BASE_RANGE + NUM_BASE_LEVELS + 1) as u32);
                }
                let rctx = coeff_br_ctx(tx_class, row, col, pos, mag);
                let mut left = (level - coded).min(COEFF_BASE_RANGE);
                for _ in 0..(COEFF_BASE_RANGE / (BR_CDF_SIZE - 1)) {
                    let br = left.min(BR_CDF_SIZE - 1);
                    enc.write_symbol(&mut cdfs.coeff_br[tx_sz_ctx.min(TX_32X32)][ptype][rctx], br);
                    coded += br;
                    left -= br;
                    if br < BR_CDF_SIZE - 1 {
                        break;
                    }
                }
            }
            lev[li] = coded as u8;
        }

        // Signs and Golomb tails, first scan position first.
        for c in 0..eob {
            let pos = scan[c] as usize;
            let v = block.levels[pos];
            let mag = v.unsigned_abs();
            assert!(mag <= 0xFFFFF, "level {v} is beyond what the syntax round-trips");
            if v != 0 {
                let neg = v < 0;
                if c == 0 {
                    let sctx = ctx.dc_sign_ctx(plane, tx_sz, x4, y4);
                    enc.write_symbol(&mut cdfs.dc_sign[ptype][sctx], neg as usize);
                } else {
                    enc.write_literal(neg as u32, 1);
                }
                if mag as usize > NUM_BASE_LEVELS + COEFF_BASE_RANGE {
                    // golomb: x = mag - 14 >= 1, as (len - 1) zeros, then x in len bits
                    let x = mag - (NUM_BASE_LEVELS + COEFF_BASE_RANGE) as u32;
                    let len = 32 - x.leading_zeros();
                    for _ in 1..len {
                        enc.write_literal(0, 1);
                    }
                    enc.write_literal(x, len);
                }
                if pos == 0 {
                    dc_category = if neg { 1 } else { 2 };
                }
            }
            cul_level += mag;
        }
        cul_level = cul_level.min(63);
    }
    ctx.set_coef_ctx(plane, tx_sz, x4, y4, cul_level as u8, dc_category);
}
