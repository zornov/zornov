// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M7: inter block mode info (spec 5.11.7 and 5.11.18 to 5.11.33) for the phase (b) subset: one
//! reference frame, translation only (`NEARESTMV`, `NEARMV`, `GLOBALMV`, `NEWMV`), quarter-pel
//! vectors, a fixed interpolation filter and no segmentation, skip mode or compound tools.
//!
//! The frame header M1 writes turns those tools off, so nothing for them is coded here.

use studio_av1::cdf::Cdfs;
use studio_av1::mvpred::MvStack;
use studio_av1::spec_tables::*;

use super::ctx::*;
use crate::entropy::SymbolEncoder;
use crate::motion::mvpred::predicted_mv;
use crate::types::InterDecision;

fn cmp3(a: usize, b: usize) -> usize {
    match a.cmp(&b) {
        std::cmp::Ordering::Less => 0,
        std::cmp::Ordering::Equal => 1,
        std::cmp::Ordering::Greater => 2,
    }
}

/// The reference frame symbols of a single-reference block (`read_ref_frames`).
fn write_single_ref(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, ctx: &ContextState, r: usize, c: usize, rf: usize) {
    assert!((LAST_FRAME..=ALTREF_FRAME).contains(&rf), "reference frame {rf}");
    let n = |ft: usize| ctx.count_refs(r, c, ft);
    let single_p1 = cmp3(n(LAST_FRAME) + n(LAST2_FRAME) + n(LAST3_FRAME) + n(GOLDEN_FRAME), n(BWDREF_FRAME) + n(ALTREF2_FRAME) + n(ALTREF_FRAME));
    let comp_ref = cmp3(n(LAST_FRAME) + n(LAST2_FRAME), n(LAST3_FRAME) + n(GOLDEN_FRAME));
    let comp_ref_p1 = cmp3(n(LAST_FRAME), n(LAST2_FRAME));
    let comp_ref_p2 = cmp3(n(LAST3_FRAME), n(GOLDEN_FRAME));
    let comp_bwdref = cmp3(n(BWDREF_FRAME) + n(ALTREF2_FRAME), n(ALTREF_FRAME));
    let comp_bwdref_p1 = cmp3(n(BWDREF_FRAME), n(ALTREF2_FRAME));
    let backward = rf >= BWDREF_FRAME;
    enc.write_symbol(&mut cdfs.single_ref[single_p1][0], backward as usize);
    if backward {
        enc.write_symbol(&mut cdfs.single_ref[comp_bwdref][1], (rf == ALTREF_FRAME) as usize);
        if rf != ALTREF_FRAME {
            enc.write_symbol(&mut cdfs.single_ref[comp_bwdref_p1][5], (rf == ALTREF2_FRAME) as usize);
        }
    } else {
        let upper = rf == LAST3_FRAME || rf == GOLDEN_FRAME;
        enc.write_symbol(&mut cdfs.single_ref[comp_ref][2], upper as usize);
        if upper {
            enc.write_symbol(&mut cdfs.single_ref[comp_ref_p2][4], (rf == GOLDEN_FRAME) as usize);
        } else {
            enc.write_symbol(&mut cdfs.single_ref[comp_ref_p1][3], (rf == LAST2_FRAME) as usize);
        }
    }
}

/// The `drl_mode` bits (`(index into the stack contexts, bit)`) that make the decoder end up with
/// `ref_mv_idx == target` for `mode`.
fn drl_bits(mode: usize, target: usize, num_mv_found: usize) -> Vec<(usize, usize)> {
    let mut bits = Vec::new();
    let (start, range) = if mode == NEWMV { (0, 0..2) } else { (1, 1..3) };
    let mut idx_now = start;
    for idx in range {
        if num_mv_found > idx + 1 {
            let bit = (target > idx) as usize;
            bits.push((idx, bit));
            if bit == 0 {
                idx_now = idx;
                break;
            }
            idx_now = idx + 1;
        }
    }
    assert_eq!(idx_now, target, "ref_mv_idx {target} is not reachable for mode {mode} with {num_mv_found} stack entries");
    bits
}

/// One motion vector component of a difference, `v != 0` in 1/8 units (`read_mv_component` with
/// `force_integer_mv = 0` and `allow_high_precision_mv = 0`, so the difference is even).
fn write_mv_component(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, comp: usize, v: i32) {
    assert!(v != 0 && v % 2 == 0, "motion vector difference component {v} must be even and nonzero");
    let mag = v.unsigned_abs() as usize;
    let z = mag - 1;
    let sign = (v < 0) as usize;
    enc.write_symbol(&mut cdfs.mv_sign[0][comp], sign);
    // class 0 covers z < 16; class c >= 1 covers 8 << c ..= (16 << c) - 1
    let class = if z < 16 { 0 } else { (usize::BITS - 1 - z.leading_zeros()) as usize - 3 };
    assert!(class <= 10, "motion vector difference {v} is out of range");
    enc.write_symbol(&mut cdfs.mv_class[0][comp], class);
    if class == 0 {
        let b0 = z >> 3;
        enc.write_symbol(&mut cdfs.mv_class0_bit[0][comp], b0);
        enc.write_symbol(&mut cdfs.mv_class0_fr[0][comp][b0], (z >> 1) & 3);
        // the high precision bit is not coded and reads as 1: z is odd
        assert!(z & 1 == 1);
    } else {
        let rest = z - (CLASS0_SIZE << (class + 2));
        let d = rest >> 3;
        for i in 0..class {
            enc.write_symbol(&mut cdfs.mv_bit[0][comp][i], (d >> i) & 1);
        }
        enc.write_symbol(&mut cdfs.mv_fr[0][comp], (rest >> 1) & 3);
        assert!(rest & 1 == 1);
    }
}

/// `read_mv()`: the joint and the nonzero components of `mv - pred`.
fn write_mv(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, mv: [i32; 2], pred: [i32; 2]) {
    let diff = [mv[0] - pred[0], mv[1] - pred[1]];
    let joint = match (diff[0] != 0, diff[1] != 0) {
        (false, false) => MV_JOINT_ZERO,
        (true, false) => MV_JOINT_HZVNZ,
        (false, true) => MV_JOINT_HNZVZ,
        (true, true) => MV_JOINT_HNZVNZ,
    };
    enc.write_symbol(&mut cdfs.mv_joint[0], joint);
    if diff[0] != 0 {
        write_mv_component(enc, cdfs, 0, diff[0]);
    }
    if diff[1] != 0 {
        write_mv_component(enc, cdfs, 1, diff[1]);
    }
}

/// `inter_frame_mode_info()` of an inter block: skip, `is_inter`, the reference frame, the mode
/// with its stack contexts, the DRL index and the motion vector. `stack` is the motion vector
/// stack of the block for `d.ref_frame` (`motion::mvpred::find_stack`).
///
/// # Panics
/// If the decision is not codable: a block under 8x8, a motion vector that is not what the mode
/// derives from the stack (for modes other than `NEWMV`), an odd `NEWMV` difference, or a
/// `ref_mv_idx` the stack cannot reach.
pub fn write_inter_block_mode_info(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, ctx: &ContextState, d: &InterDecision, stack: &MvStack) {
    let (r, c) = (d.mi_row as usize, d.mi_col as usize);
    assert!(d.w4 >= 2 && d.h4 >= 2, "inter blocks must be at least 8x8");
    let sctx = ctx.skip_ctx(r, c);
    enc.write_symbol(&mut cdfs.skip[sctx], d.skip as usize);
    let ictx = ctx.is_inter_ctx(r, c);
    enc.write_symbol(&mut cdfs.is_inter[ictx], 1);
    write_single_ref(enc, cdfs, ctx, r, c, d.ref_frame as usize);

    match d.mode {
        NEWMV => enc.write_symbol(&mut cdfs.new_mv[stack.new_mv_context], 0),
        GLOBALMV | NEARESTMV | NEARMV => {
            enc.write_symbol(&mut cdfs.new_mv[stack.new_mv_context], 1);
            enc.write_symbol(&mut cdfs.zero_mv[stack.zero_mv_context], (d.mode != GLOBALMV) as usize);
            if d.mode != GLOBALMV {
                enc.write_symbol(&mut cdfs.ref_mv[stack.ref_mv_context], (d.mode == NEARMV) as usize);
            }
        }
        m => panic!("inter mode {m} is not supported (single reference, no compound)"),
    }
    match d.mode {
        NEWMV => assert!(d.ref_mv_idx <= 2),
        NEARMV => assert!((1..=3).contains(&d.ref_mv_idx)),
        _ => assert_eq!(d.ref_mv_idx, if d.mode == NEARESTMV { 0 } else { d.ref_mv_idx }),
    }
    if d.mode == NEWMV || d.mode == NEARMV {
        for (idx, bit) in drl_bits(d.mode, d.ref_mv_idx, stack.num_mv_found) {
            enc.write_symbol(&mut cdfs.drl_mode[stack.drl_ctx_stack[idx]], bit);
        }
    }
    let pred = predicted_mv(stack, d.mode, d.ref_mv_idx);
    if d.mode == NEWMV {
        write_mv(enc, cdfs, d.mv, pred);
    } else {
        assert_eq!(d.mv, pred, "mode {} derives {:?} from the stack, not {:?}", d.mode, pred, d.mv);
    }
}
