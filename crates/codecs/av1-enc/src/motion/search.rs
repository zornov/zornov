// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M7 (phase b): motion estimation for one luma block: an integer-pel search (a coarse grid, then
//! a local refinement, starting from a few predicted vectors), then half- and quarter-pel
//! refinement with the decoder's own interpolation. Vectors are `[row, col]` in 1/8 sample units
//! and always even (quarter-pel precision).

use studio_av1::frame::Plane;
use studio_av1::inter::{PRED_STRIDE, block_inter_prediction, rounding};
use studio_av1::spec_tables::*;

use super::Mv;
use crate::types::Frame;

/// The farthest the search reaches from a start point, in whole samples.
pub const RANGE: i32 = 32;

/// A block of the source luma plane and the reference luma plane it is matched against.
pub struct Matcher<'a> {
    pub src: &'a Frame,
    pub reference: &'a Plane,
    /// Position and size of the block in luma samples.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Matcher<'_> {
    fn dims(&self) -> (i32, i32) {
        (self.src.width as i32, self.src.height as i32)
    }

    /// Sum of absolute differences with the reference block at whole-sample offset `(dy, dx)`;
    /// samples of the block outside the picture are not counted. Stops early above `limit`.
    fn sad_int(&self, dy: i32, dx: i32, limit: u64) -> u64 {
        let (pw, ph) = self.dims();
        let mut sad = 0u64;
        for j in 0..self.h {
            let sy = self.y + j;
            if sy as i32 >= ph {
                break;
            }
            let ry = (sy as i32 + dy).clamp(0, ph - 1) as usize;
            let src_row = &self.src.planes[0][sy * self.src.stride[0] + self.x..];
            let ref_row = self.reference.row(ry);
            for i in 0..self.w {
                let sx = self.x + i;
                if sx as i32 >= pw {
                    break;
                }
                let rx = (sx as i32 + dx).clamp(0, pw - 1) as usize;
                sad += (src_row[i] as i32 - ref_row[rx] as i32).unsigned_abs() as u64;
            }
            if sad >= limit {
                return sad;
            }
        }
        sad
    }

    /// SAD of the block predicted with the (possibly fractional) vector `mv`.
    fn sad_mv(&self, mv: Mv, pred: &mut [i32], tmp: &mut [i32]) -> u64 {
        let (pw, ph) = self.dims();
        let start = |p: usize, v: i32| (((p as i32) << 4) + 2 * v) * 64 + 32;
        block_inter_prediction(
            self.reference,
            pw - 1,
            ph - 1,
            start(self.x, mv[1]),
            start(self.y, mv[0]),
            1 << SCALE_SUBPEL_BITS,
            1 << SCALE_SUBPEL_BITS,
            self.w,
            self.h,
            [EIGHTTAP as u8; 2],
            rounding(false, 8),
            pred,
            tmp,
        );
        let mut sad = 0u64;
        for j in 0..self.h {
            let sy = self.y + j;
            if sy as i32 >= ph {
                break;
            }
            let src_row = &self.src.planes[0][sy * self.src.stride[0] + self.x..];
            for i in 0..self.w {
                if (self.x + i) as i32 >= pw {
                    break;
                }
                sad += (src_row[i] as i32 - pred[j * PRED_STRIDE + i].clamp(0, 255)).unsigned_abs() as u64;
            }
        }
        sad
    }
}

/// Approximate bits of a vector difference (both components, 1/8 units).
pub fn mv_bits(diff: Mv) -> f64 {
    let comp = |d: i32| if d == 0 { 0.0 } else { 3.0 + 2.0 * (((d.abs() / 2) + 1) as f64).log2() };
    1.0 + comp(diff[0]) + comp(diff[1])
}

/// Searches the best vector for the block. `starts` are predicted vectors (stack entries, zero)
/// the search also tries; `pred` is the vector `NEWMV` would be coded against; `lambda_sad` is
/// the price of a bit in SAD units. Returns the vector and its SAD.
pub fn search(m: &Matcher<'_>, starts: &[Mv], pred: Mv, lambda_sad: f64, full: bool) -> (Mv, u64) {
    let cost = |mv: Mv, sad: u64| sad as f64 + lambda_sad * mv_bits([mv[0] - pred[0], mv[1] - pred[1]]);
    let to_full = |v: i32| (v + if v >= 0 { 4 } else { -4 }) / 8; // nearest whole sample
    let mut best_int: (i32, i32) = (0, 0);
    let mut best_cost = f64::MAX;
    let mut consider = |dy: i32, dx: i32, best_int: &mut (i32, i32), best_cost: &mut f64| {
        if dy.abs() > RANGE * 3 || dx.abs() > RANGE * 3 {
            return;
        }
        let sad = m.sad_int(dy, dx, if *best_cost == f64::MAX { u64::MAX } else { *best_cost as u64 + 1 });
        let c = cost([dy * 8, dx * 8], sad);
        if c < *best_cost {
            *best_cost = c;
            *best_int = (dy, dx);
        }
    };
    consider(0, 0, &mut best_int, &mut best_cost);
    for s in starts {
        consider(to_full(s[0]), to_full(s[1]), &mut best_int, &mut best_cost);
    }
    if full {
        // coarse grid around zero and around the best start, then local refinement
        let centre = best_int;
        for &(cy, cx) in &[(0, 0), centre] {
            let mut dy = -RANGE;
            while dy <= RANGE {
                let mut dx = -RANGE;
                while dx <= RANGE {
                    consider(cy + dy, cx + dx, &mut best_int, &mut best_cost);
                    dx += 4;
                }
                dy += 4;
            }
        }
    }
    for step in [4, 2, 1] {
        let c = best_int;
        for dy in [-step, 0, step] {
            for dx in [-step, 0, step] {
                consider(c.0 + dy, c.1 + dx, &mut best_int, &mut best_cost);
            }
        }
    }
    let c = best_int;
    for dy in -1..=1 {
        for dx in -1..=1 {
            consider(c.0 + dy, c.1 + dx, &mut best_int, &mut best_cost);
        }
    }

    // fractional refinement: half then quarter sample
    let mut pred_buf = vec![0i32; PRED_STRIDE * 64];
    let mut tmp = vec![0i32; 71 * 64];
    let mut best = [best_int.0 * 8, best_int.1 * 8];
    let mut best_sad = m.sad_mv(best, &mut pred_buf, &mut tmp);
    let mut best_c = cost(best, best_sad);
    for step in [4, 2] {
        let c = best;
        for dy in [-step, 0, step] {
            for dx in [-step, 0, step] {
                if dy == 0 && dx == 0 {
                    continue;
                }
                let mv = [c[0] + dy, c[1] + dx];
                let sad = m.sad_mv(mv, &mut pred_buf, &mut tmp);
                let cc = cost(mv, sad);
                if cc < best_c {
                    best_c = cc;
                    best = mv;
                    best_sad = sad;
                }
            }
        }
    }
    (best, best_sad)
}
