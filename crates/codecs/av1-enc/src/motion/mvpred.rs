// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M7 (phase b): motion vector candidate stack (spec 7.10).
//!
//! The processes themselves are the decoder's (`studio_av1::mvpred`), fed with plain data, so the
//! encoder and the decoder cannot disagree about candidates or contexts.

use studio_av1::frame::MiInfo;
use studio_av1::mvpred::{MvBlock, MvFrame, MvPredictor, MvStack};
use studio_av1::spec_tables::*;

/// The frame-level inputs of the motion vector processes for the phase (b) inter frames: no high
/// precision, no integer-only vectors, no temporal vectors, every reference in the past, no global
/// motion.
pub fn frame_info(mi_rows: usize, mi_cols: usize) -> MvFrame {
    let mut gm_params = [[0i32; 6]; 8];
    for p in gm_params.iter_mut() {
        p[2] = 1 << WARPEDMODEL_PREC_BITS;
        p[5] = 1 << WARPEDMODEL_PREC_BITS;
    }
    MvFrame {
        mi_rows: mi_rows as u32,
        mi_cols: mi_cols as u32,
        allow_high_precision_mv: false,
        force_integer_mv: false,
        use_ref_frame_mvs: false,
        ref_frame_sign_bias: [false; 8],
        gm_type: [IDENTITY as u8; 8],
        gm_params,
    }
}

/// The motion vector stack of the block `shape = (mi_row, mi_col, w4, h4)` predicting from
/// `ref_frame`, given the mode info of the blocks coded before it (one tile covering the frame).
pub fn find_stack(mi: &MiInfo, frame: &MvFrame, shape: (usize, usize, usize, usize), ref_frame: u8) -> MvStack {
    let (r, c, w4, h4) = shape;
    let mi_size = (0..BLOCK_SIZES).find(|&b| NUM_4X4_BLOCKS_WIDE[b] as usize == w4 && NUM_4X4_BLOCKS_HIGH[b] as usize == h4).unwrap_or_else(|| panic!("no block size {w4}x{h4}"));
    let blk = MvBlock { mi_row: r, mi_col: c, mi_size, bw4: w4, bh4: h4, ref_frame: [ref_frame as i8, -1], avail_u: r > 0, avail_l: c > 0, mv_i: [[0; 2]; 2] };
    let motion_field: [Vec<[i32; 2]>; 8] = Default::default();
    let mut stack = MvStack::default();
    let mut p = MvPredictor {
        b: &blk,
        fh: frame,
        mi,
        motion_field: &motion_field,
        mi_row_start: 0,
        mi_row_end: frame.mi_rows as usize,
        mi_col_start: 0,
        mi_col_end: frame.mi_cols as usize,
        mvs: &mut stack,
    };
    p.find_mv_stack(false);
    stack
}

/// The vector `mode` starts from (`assign_mv` for one reference): the global vector (zero here),
/// the first stack entry, the `ref_mv_idx` entry, or for `NEWMV` the entry the difference is coded
/// against.
pub fn predicted_mv(stack: &MvStack, mode: usize, ref_mv_idx: usize) -> [i32; 2] {
    match mode {
        GLOBALMV => stack.global_mvs[0],
        NEARESTMV => stack.ref_stack_mv[0][0],
        NEARMV => stack.ref_stack_mv[ref_mv_idx][0],
        NEWMV => stack.ref_stack_mv[if stack.num_mv_found <= 1 { 0 } else { ref_mv_idx }][0],
        m => panic!("inter mode {m} is not supported"),
    }
}
