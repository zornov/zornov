// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M1: uncompressed frame header writer (spec 5.9) for key, intra-only and inter frames.
//!
//! Choices fixed here (the sequence header the writer pairs with is in `seq_header.rs`): frame size
//! equals the sequence size (no override, no superres, render size equal), no segmentation, no
//! delta q / delta lf, no quantiser matrices, zero delta q for DC and chroma, loop filter deltas off,
//! `TX_MODE_LARGEST`, `reduced_tx_set = 0`, `disable_frame_end_update_cdf = 1`, uniform tile spacing
//! with `context_update_tile_id = 0` and `tile_size_bytes = 4`. CDEF (when the sequence enables it)
//! is coded with zero strengths and loop restoration as `RESTORE_NONE`, because `FrameParams`
//! carries only on/off flags for them.
//!
//! Inter frames (phase (b)) use no primary reference frame (default CDFs), the seven references
//! named by `FrameParams::ref_frame_idx`, quarter-pel motion vectors (no high precision), the fixed
//! regular 8-tap interpolation filter, no switchable motion modes, no reference mvs, single
//! reference prediction (`reference_select = 0`, so no skip mode) and no global motion.

use crate::bits::BitWriter;
use crate::types::{FrameParams, FrameType, SeqParams};

use super::seq_header::ORDER_HINT_BITS;
use super::{ObuType, obu};

/// Bytes used for each tile size field in a tile group (`tile_size_bytes_minus_1 = 3`).
pub const TILE_SIZE_BYTES: usize = 4;

const MAX_TILE_WIDTH: u32 = 4096;
const MAX_TILE_AREA: u32 = 4096 * 2304;
const MAX_TILE_COLS: u32 = 64;
const MAX_TILE_ROWS: u32 = 64;

fn tile_log2(blk: u32, target: u32) -> u32 {
    let mut k = 0;
    while (blk << k) < target {
        k += 1;
    }
    k
}

/// The `(min, max)` `TileColsLog2` and `(.., max)` `TileRowsLog2` bounds of 5.9.15 for this
/// sequence: `(min_cols, max_cols, min_tiles, max_rows)`.
fn tile_limits(seq: &SeqParams) -> (u32, u32, u32, u32) {
    let mi_cols = 2 * ((seq.width + 7) >> 3);
    let mi_rows = 2 * ((seq.height + 7) >> 3);
    let sb_shift = if seq.use_128x128_superblock { 5 } else { 4 };
    let sb_cols = (mi_cols + (1 << sb_shift) - 1) >> sb_shift;
    let sb_rows = (mi_rows + (1 << sb_shift) - 1) >> sb_shift;
    let sb_size = sb_shift + 2;
    let max_tile_width_sb = MAX_TILE_WIDTH >> sb_size;
    let max_tile_area_sb = MAX_TILE_AREA >> (2 * sb_size);
    let min_cols = tile_log2(max_tile_width_sb, sb_cols);
    let max_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
    let max_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS));
    let min_tiles = min_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));
    (min_cols, max_cols, min_tiles, max_rows)
}

/// Clamp a requested tile layout (log2 columns, log2 rows) to what the frame size allows.
pub fn clamp_tile_log2(seq: &SeqParams, cols_log2: u8, rows_log2: u8) -> (u8, u8) {
    let (min_cols, max_cols, min_tiles, max_rows) = tile_limits(seq);
    let c = (cols_log2 as u32).clamp(min_cols, max_cols);
    let min_rows = min_tiles.saturating_sub(c);
    let r = (rows_log2 as u32).clamp(min_rows, max_rows.max(min_rows));
    (c as u8, r as u8)
}

/// Write `uncompressed_header()` into `w`. The caller adds `trailing_bits()` (frame header OBU) or
/// `byte_alignment()` and the tile group (frame OBU).
///
/// Panics for parameter combinations the spec forbids (e.g. an intra-only frame refreshing all
/// slots).
pub fn write_frame_header(seq: &SeqParams, fh: &FrameParams, w: &mut BitWriter) {
    let is_key = fh.frame_type == FrameType::Key;
    let is_inter = fh.frame_type == FrameType::Inter;
    w.flag(false); // show_existing_frame
    // frame_type: KEY_FRAME = 0, INTER_FRAME = 1, INTRA_ONLY_FRAME = 2
    w.put(
        match fh.frame_type {
            FrameType::Key => 0,
            FrameType::Inter => 1,
            FrameType::IntraOnly => 2,
        },
        2,
    );
    w.flag(fh.show_frame);
    if !fh.show_frame {
        w.flag(!is_key); // showable_frame
    }
    let error_resilient = is_key && fh.show_frame;
    if !error_resilient {
        w.flag(false); // error_resilient_mode
    }
    w.flag(fh.disable_cdf_update);
    // allow_screen_content_tools is not coded (seq_force_screen_content_tools = 0).
    w.flag(false); // frame_size_override_flag
    if seq.enable_order_hint {
        w.put(
            fh.order_hint & ((1 << ORDER_HINT_BITS) - 1),
            ORDER_HINT_BITS,
        );
    }
    if is_inter {
        w.put(7, 3); // primary_ref_frame = PRIMARY_REF_NONE
    }
    // (for intra frames primary_ref_frame is PRIMARY_REF_NONE and not coded)
    if !(is_key && fh.show_frame) {
        assert!(
            fh.refresh_frame_flags != 0xFF || fh.frame_type != FrameType::IntraOnly,
            "an intra-only frame cannot refresh every slot"
        );
        w.put(fh.refresh_frame_flags as u32, 8);
    }
    // ref_order_hint[] is only coded under error_resilient_mode, which is off here.
    if is_inter {
        if seq.enable_order_hint {
            w.flag(false); // frame_refs_short_signaling
        }
        for &idx in &fh.ref_frame_idx {
            assert!(idx < 8, "ref_frame_idx is a slot number");
            w.put(idx as u32, 3);
        }
    }
    // frame_size(): no override, no superres; render_size():
    w.flag(false); // render_and_frame_size_different
    // allow_intrabc is not coded (screen content tools off).
    if is_inter {
        w.flag(false); // allow_high_precision_mv
        w.flag(false); // is_filter_switchable
        w.put(0, 2); // interpolation_filter = EIGHTTAP
        w.flag(false); // is_motion_mode_switchable
        // use_ref_frame_mvs is not coded (enable_ref_frame_mvs is off)
    }
    if !fh.disable_cdf_update {
        w.flag(true); // disable_frame_end_update_cdf (implied 1 when disable_cdf_update is set)
    }
    write_tile_info(seq, fh, w);
    // quantization_params()
    w.put(fh.base_q_idx as u32, 8);
    w.flag(false); // DeltaQYDc coded
    w.flag(false); // DeltaQUDc coded
    w.flag(false); // DeltaQUAc coded
    w.flag(false); // using_qmatrix
    w.flag(false); // segmentation_enabled
    if fh.base_q_idx > 0 {
        w.flag(false); // delta_q_present
    }
    let coded_lossless = fh.base_q_idx == 0;
    if !coded_lossless {
        // loop_filter_params()
        w.put(fh.loop_filter_level[0] as u32, 6);
        w.put(fh.loop_filter_level[1] as u32, 6);
        if fh.loop_filter_level[0] != 0 || fh.loop_filter_level[1] != 0 {
            w.put(fh.loop_filter_level[0] as u32, 6); // loop_filter_level[2] (U): same as luma
            w.put(fh.loop_filter_level[1] as u32, 6); // loop_filter_level[3] (V): same as luma
        }
        w.put(0, 3); // loop_filter_sharpness
        w.flag(false); // loop_filter_delta_enabled
        if seq.enable_cdef {
            let c = &fh.cdef;
            w.put(c.damping as u32 - 3, 2); // cdef_damping_minus_3
            w.put(c.bits as u32, 2); // cdef_bits
            let sec = |v: u8| if v == 4 { 3 } else { v as u32 };
            for i in 0..1usize << c.bits {
                w.put(c.y_pri[i] as u32, 4);
                w.put(sec(c.y_sec[i]), 2);
                w.put(c.uv_pri[i] as u32, 4);
                w.put(sec(c.uv_sec[i]), 2);
            }
        }
        if seq.enable_restoration {
            for _ in 0..3 {
                w.put(0, 2); // lr_type = RESTORE_NONE
            }
        }
        w.flag(false); // tx_mode_select = 0: TX_MODE_LARGEST
    }
    if is_inter {
        w.flag(false); // reference_select (skip mode is then not allowed, so not coded)
    }
    // warped motion is off in the sequence header. reduced_tx_set:
    w.flag(false);
    if is_inter {
        for _ in 0..7 {
            w.flag(false); // is_global for LAST_FRAME..=ALTREF_FRAME
        }
    }
    // film_grain_params() codes nothing here.
}

fn write_tile_info(seq: &SeqParams, fh: &FrameParams, w: &mut BitWriter) {
    let (min_cols, max_cols, min_tiles, max_rows) = tile_limits(seq);
    let (cols_log2, rows_log2) = clamp_tile_log2(seq, fh.tile_cols_log2, fh.tile_rows_log2);
    let (cols_log2, rows_log2) = (cols_log2 as u32, rows_log2 as u32);
    w.flag(true); // uniform_tile_spacing_flag
    let mut c = min_cols;
    while c < max_cols {
        let inc = c < cols_log2;
        w.flag(inc); // increment_tile_cols_log2
        if !inc {
            break;
        }
        c += 1;
    }
    let min_rows = min_tiles.saturating_sub(cols_log2);
    let mut r = min_rows;
    while r < max_rows {
        let inc = r < rows_log2;
        w.flag(inc); // increment_tile_rows_log2
        if !inc {
            break;
        }
        r += 1;
    }
    if cols_log2 > 0 || rows_log2 > 0 {
        w.put(0, rows_log2 + cols_log2); // context_update_tile_id
        w.put(TILE_SIZE_BYTES as u32 - 1, 2); // tile_size_bytes_minus_1
    }
}

/// A frame header OBU (type 3): the header followed by `trailing_bits()`.
pub fn frame_header_obu(seq: &SeqParams, fh: &FrameParams) -> Vec<u8> {
    let mut w = BitWriter::new();
    write_frame_header(seq, fh, &mut w);
    w.trailing_bits();
    obu(ObuType::FrameHeader, &w.finish())
}
