// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M5b (phase b): inter prediction and reconstruction of one block, through the decoder's own
//! `studio_av1::inter::block_inter_prediction` and `inverse_transform_2d`, so the pixels match
//! bit for bit.
//!
//! Scope: one reference frame, translation only (unscaled references), the regular 8-tap filter,
//! blocks of at least 8x8 and at most 64x64, with one transform per plane (`TX_MODE_LARGEST`), 4:2:0.

use super::{FrameBuf, RefStore};
use crate::transform::flips;
use crate::types::{CoefBlock, InterDecision};
use studio_av1::inter::{PRED_STRIDE, block_inter_prediction, rounding};
use studio_av1::spec_tables::*;
use studio_av1::transform::inverse_transform_2d;

/// The start position of a block in 1/1024 sample units for an unscaled reference (the motion
/// vector scaling process, 7.11.3.3, with a scale of 1): `x` is the sample position in the plane,
/// `mv` the vector component in 1/8 luma samples, `sub` the plane's subsampling shift.
fn start_position(x: usize, mv: i32, sub: u32) -> i32 {
    (((x as i32) << 4) + ((2 * mv) >> sub)) * 64 + 32
}

impl FrameBuf {
    /// Predicts the inter block `d` from the reference `refs[ref_frame_idx[d.ref_frame - 1]]` and
    /// adds its residual (unless `d.skip`).
    ///
    /// # Panics
    /// If the reference slot is empty, the block is smaller than 8x8, or the coefficient blocks do
    /// not match the block's transform blocks.
    pub fn reconstruct_inter_block(&mut self, refs: &RefStore, ref_frame_idx: &[u8; 7], d: &InterDecision) {
        let (mi_row, mi_col, bw4, bh4) = (d.mi_row as usize, d.mi_col as usize, d.w4 as usize, d.h4 as usize);
        assert!(bw4 >= 2 && bh4 >= 2 && bw4 <= 16 && bh4 <= 16, "inter blocks are 8x8 to 64x64");
        assert!((1..=7).contains(&d.ref_frame));
        let slot = ref_frame_idx[d.ref_frame as usize - 1] as usize;
        let reference = refs.slots[slot].as_ref().unwrap_or_else(|| panic!("reference slot {slot} is empty"));
        assert_eq!((reference.width, reference.height), (self.width, self.height), "scaled references are not supported");
        let bd = self.bit_depth as u32;
        let rnd = rounding(false, bd);
        let max = (1i32 << bd) - 1;
        let mut pred = vec![0i32; PRED_STRIDE * 64];
        let mut tmp = vec![0i32; 71 * 64];
        let mut coefs = d.coefs.iter();
        for plane in 0..3 {
            let sub = (plane > 0) as u32;
            let (w, h) = ((bw4 * 4) >> sub, (bh4 * 4) >> sub);
            let (x, y) = ((mi_col * 4) >> sub, (mi_row * 4) >> sub);
            let (pw, ph) = (((self.width + sub) >> sub) as i32, ((self.height + sub) >> sub) as i32);
            block_inter_prediction(
                &reference.planes[plane],
                pw - 1,
                ph - 1,
                start_position(x, d.mv[1], sub),
                start_position(y, d.mv[0], sub),
                1 << SCALE_SUBPEL_BITS,
                1 << SCALE_SUBPEL_BITS,
                w,
                h,
                [EIGHTTAP as u8; 2],
                rnd,
                &mut pred,
                &mut tmp,
            );
            let pl = &mut self.planes[plane];
            for i in 0..h {
                let row = &mut pl.row_from_mut(y + i, x)[..w];
                for (j, o) in row.iter_mut().enumerate() {
                    *o = pred[i * PRED_STRIDE + j].clamp(0, max) as u16;
                }
            }
            if !d.skip {
                let c = coefs.next().unwrap_or_else(|| panic!("missing coefficients for plane {plane}"));
                assert_eq!(c.plane as usize, plane);
                if c.eob > 0 {
                    self.add_inter_residual(plane, x, y, c);
                }
            }
        }
        assert!(coefs.next().is_none(), "more coefficient blocks than planes");
    }

    /// Dequantisation (7.12.3) and inverse transform (7.13.3), added to the prediction with
    /// clipping and the flips of the FLIPADST types.
    fn add_inter_residual(&mut self, plane: usize, x: usize, y: usize, c: &CoefBlock) {
        let tx_sz = c.tx_size;
        let dq_denom: i64 = match tx_sz {
            TX_32X32 | TX_16X32 | TX_32X16 | TX_16X64 | TX_64X16 => 2,
            TX_64X64 | TX_32X64 | TX_64X32 => 4,
            _ => 1,
        };
        let w = TX_WIDTH[tx_sz] as usize;
        let h = TX_HEIGHT[tx_sz] as usize;
        let (tw, th) = (w.min(32), h.min(32));
        let (flip_ud, flip_lr) = flips(c.tx_type);
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
        inverse_transform_2d(&dequant, &mut residual, tx_sz, c.tx_type, self.quant.lossless(), bd);
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
