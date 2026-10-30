// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M4: intra frame mode info (spec 5.11.7): skip, y/uv modes, angle deltas and the transform size.
//!
//! Not coded, because the frame header turns them off: segment ids, CDEF indices (zero bits),
//! delta q / delta lf, intra block copy, palettes, filter intra and chroma-from-luma.

use studio_av1::cdf::Cdfs;
use studio_av1::spec_tables::*;

use super::ctx::*;
use crate::entropy::SymbolEncoder;
use crate::recon::IntraExtra;

/// Block-level facts the mode info writers need.
#[derive(Debug, Clone, Copy)]
pub struct BlockInfo {
    pub mi_row: usize,
    pub mi_col: usize,
    pub mi_size: usize,
    pub has_chroma: bool,
    pub y_mode: usize,
    pub uv_mode: usize,
    pub skip: bool,
    pub tx_size: usize,
    pub extra: IntraExtra,
}

/// Frame-level switches that shape the block syntax.
#[derive(Debug, Clone, Copy)]
pub struct FrameSyntax {
    /// `TX_MODE_SELECT` (the transform depth is coded) rather than `TX_MODE_LARGEST`.
    pub tx_mode_select: bool,
    /// `CodedLossless`: every transform is a 4x4 WHT.
    pub lossless: bool,
}

fn is_directional(mode: usize) -> bool {
    (V_PRED..=D67_PRED).contains(&mode)
}

fn write_angle_delta(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, mi_size: usize, mode: usize, delta: i32) {
    if mi_size >= BLOCK_8X8 && is_directional(mode) {
        assert!((-(MAX_ANGLE_DELTA as i32)..=MAX_ANGLE_DELTA as i32).contains(&delta), "angle delta {delta} out of range");
        enc.write_symbol(&mut cdfs.angle_delta[mode - V_PRED], (delta + MAX_ANGLE_DELTA as i32) as usize);
    } else {
        assert_eq!(delta, 0, "an angle delta is coded only for directional modes of blocks from 8x8");
    }
}

/// `intra_frame_mode_info()` followed by the block's transform size (`read_block_tx_size()`).
pub fn write_intra_frame_mode_info(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, ctx: &mut ContextState, b: &BlockInfo, f: FrameSyntax) {
    let (r, c, ms) = (b.mi_row, b.mi_col, b.mi_size);
    let sctx = ctx.skip_ctx(r, c);
    enc.write_symbol(&mut cdfs.skip[sctx], b.skip as usize);
    ctx.write_cdef(enc, r, c, b.skip);

    assert!(b.y_mode <= PAETH_PRED, "y mode {} is not an intra mode", b.y_mode);
    let (actx, lctx) = ctx.y_mode_ctx(r, c);
    enc.write_symbol(&mut ctx.intra_y_mode_cdf[actx][lctx], b.y_mode);
    write_angle_delta(enc, cdfs, ms, b.y_mode, b.extra.angle_delta_y);
    if b.has_chroma {
        assert!(b.uv_mode <= PAETH_PRED, "uv mode {} is not supported (chroma-from-luma needs its alphas)", b.uv_mode);
        let cfl_allowed = if f.lossless && plane_residual_size(ms, 1) == BLOCK_4X4 {
            true
        } else {
            !f.lossless && block_width(ms).max(block_height(ms)) <= 32
        };
        if cfl_allowed {
            enc.write_symbol(&mut cdfs.uv_mode_cfl_allowed[b.y_mode], b.uv_mode);
        } else {
            enc.write_symbol(&mut cdfs.uv_mode_cfl_not_allowed[b.y_mode], b.uv_mode);
        }
        write_angle_delta(enc, cdfs, ms, b.uv_mode, b.extra.angle_delta_uv);
    } else {
        assert_eq!(b.extra.angle_delta_uv, 0);
    }
    write_tx_size(enc, cdfs, ctx, b, f);
}

/// `inter_frame_mode_info()` of an intra block in an inter frame: skip, `is_inter = 0`, the y mode
/// (coded by block size group, not by the neighbours), the uv mode, the angle deltas, and the
/// transform size.
pub fn write_intra_block_in_inter_frame(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, ctx: &mut ContextState, b: &BlockInfo, f: FrameSyntax) {
    let (r, c, ms) = (b.mi_row, b.mi_col, b.mi_size);
    let sctx = ctx.skip_ctx(r, c);
    enc.write_symbol(&mut cdfs.skip[sctx], b.skip as usize);
    ctx.write_cdef(enc, r, c, b.skip);
    let ictx = ctx.is_inter_ctx(r, c);
    enc.write_symbol(&mut cdfs.is_inter[ictx], 0);

    assert!(b.y_mode <= PAETH_PRED, "y mode {} is not an intra mode", b.y_mode);
    enc.write_symbol(&mut cdfs.y_mode[SIZE_GROUP[ms] as usize], b.y_mode);
    write_angle_delta(enc, cdfs, ms, b.y_mode, b.extra.angle_delta_y);
    if b.has_chroma {
        assert!(b.uv_mode <= PAETH_PRED, "uv mode {} is not supported (chroma-from-luma needs its alphas)", b.uv_mode);
        let cfl_allowed = if f.lossless && plane_residual_size(ms, 1) == BLOCK_4X4 { true } else { !f.lossless && block_width(ms).max(block_height(ms)) <= 32 };
        if cfl_allowed {
            enc.write_symbol(&mut cdfs.uv_mode_cfl_allowed[b.y_mode], b.uv_mode);
        } else {
            enc.write_symbol(&mut cdfs.uv_mode_cfl_not_allowed[b.y_mode], b.uv_mode);
        }
        write_angle_delta(enc, cdfs, ms, b.uv_mode, b.extra.angle_delta_uv);
    } else {
        assert_eq!(b.extra.angle_delta_uv, 0);
    }
    write_tx_size(enc, cdfs, ctx, b, f);
}

/// `read_tx_size()` for an intra block.
fn write_tx_size(enc: &mut SymbolEncoder, cdfs: &mut Cdfs, ctx: &ContextState, b: &BlockInfo, f: FrameSyntax) {
    if f.lossless {
        assert_eq!(b.tx_size, TX_4X4, "lossless blocks use 4x4 transforms");
        return;
    }
    let ms = b.mi_size;
    let max_rect = MAX_TX_SIZE_RECT[ms] as usize;
    if ms > BLOCK_4X4 && f.tx_mode_select {
        let mut depth = 0;
        let mut t = max_rect;
        while t != b.tx_size {
            assert!(t != TX_4X4 && depth < 2, "transform size {} is not reachable from {max_rect}", b.tx_size);
            t = SPLIT_TX_SIZE[t] as usize;
            depth += 1;
        }
        let tctx = ctx.tx_depth_ctx(b.mi_row, b.mi_col, max_rect);
        match MAX_TX_DEPTH_TABLE[ms] {
            4 => enc.write_symbol(&mut cdfs.tx_64x64[tctx], depth),
            3 => enc.write_symbol(&mut cdfs.tx_32x32[tctx], depth),
            2 => enc.write_symbol(&mut cdfs.tx_16x16[tctx], depth),
            _ => enc.write_symbol(&mut cdfs.tx_8x8[tctx], depth),
        }
    } else {
        assert_eq!(b.tx_size, max_rect, "TX_MODE_LARGEST needs the largest transform ({max_rect})");
    }
}
