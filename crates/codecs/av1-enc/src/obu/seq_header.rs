// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M1: sequence header OBU writer (spec 5.5) and the `av1C` record.
//!
//! The stream shape is fixed for phase (a): one operating point, no timing or decoder model, no
//! frame ids, no superres, no film grain, screen content tools off, and the inter tools that
//! `SeqParams` cannot ask for disabled. `enable_filter_intra` and `enable_intra_edge_filter` are
//! off as well: `SeqParams` has no field for them.

use crate::bits::BitWriter;
use crate::obu::{ObuType, obu};
use crate::types::SeqParams;

/// `order_hint_bits` the writer uses when `enable_order_hint` is set.
pub const ORDER_HINT_BITS: u32 = 7;

/// `chroma_sample_position` written for 4:2:0 (0 = unknown).
const CHROMA_SAMPLE_POSITION: u32 = 0;

/// Bits needed to code `v - 1` as `frame_width_bits_minus_1 + 1` (at least 1, at most 16).
fn size_bits(v: u32) -> u32 {
    (32 - (v - 1).leading_zeros()).max(1)
}

fn check(seq: &SeqParams) {
    assert!(
        seq.width >= 1 && seq.width <= 1 << 16 && seq.height >= 1 && seq.height <= 1 << 16,
        "frame size out of range"
    );
    assert!(seq.profile == 0, "only seq_profile 0 (4:2:0) is supported");
    assert!(
        seq.bit_depth == 8 || seq.bit_depth == 10,
        "profile 0 is 8 or 10 bit"
    );
    assert!(seq.level_idx < 32, "seq_level_idx is 5 bits");
    assert!(
        (
            seq.color_primaries,
            seq.transfer_characteristics,
            seq.matrix_coefficients
        ) != (1, 13, 0),
        "the sRGB identity triple means 4:4:4 and cannot be used with profile 0"
    );
}

/// The sequence header OBU payload (without OBU framing), ending in trailing bits.
pub fn write_sequence_header(seq: &SeqParams) -> Vec<u8> {
    check(seq);
    let mut w = BitWriter::new();
    w.put(seq.profile as u32, 3);
    w.flag(false); // still_picture
    w.flag(false); // reduced_still_picture_header
    w.flag(false); // timing_info_present_flag
    w.flag(false); // initial_display_delay_present_flag
    w.put(0, 5); // operating_points_cnt_minus_1
    w.put(0, 12); // operating_point_idc[0]
    w.put(seq.level_idx as u32, 5);
    if seq.level_idx > 7 {
        w.flag(false); // seq_tier[0]
    }
    let (wb, hb) = (size_bits(seq.width), size_bits(seq.height));
    w.put(wb - 1, 4);
    w.put(hb - 1, 4);
    w.put(seq.width - 1, wb);
    w.put(seq.height - 1, hb);
    w.flag(false); // frame_id_numbers_present_flag
    w.flag(seq.use_128x128_superblock);
    w.flag(false); // enable_filter_intra
    w.flag(false); // enable_intra_edge_filter
    w.flag(false); // enable_interintra_compound
    w.flag(false); // enable_masked_compound
    w.flag(false); // enable_warped_motion
    w.flag(false); // enable_dual_filter
    w.flag(seq.enable_order_hint);
    if seq.enable_order_hint {
        w.flag(false); // enable_jnt_comp
        w.flag(false); // enable_ref_frame_mvs
    }
    w.flag(false); // seq_choose_screen_content_tools
    w.put(0, 1); // seq_force_screen_content_tools (so seq_force_integer_mv is not coded)
    if seq.enable_order_hint {
        w.put(ORDER_HINT_BITS - 1, 3);
    }
    w.flag(false); // enable_superres
    w.flag(seq.enable_cdef);
    w.flag(seq.enable_restoration);
    // color_config()
    w.flag(seq.bit_depth > 8); // high_bitdepth
    w.flag(false); // mono_chrome
    let unspecified = (
        seq.color_primaries,
        seq.transfer_characteristics,
        seq.matrix_coefficients,
    ) == (2, 2, 2);
    w.flag(!unspecified); // color_description_present_flag
    if !unspecified {
        w.put(seq.color_primaries as u32, 8);
        w.put(seq.transfer_characteristics as u32, 8);
        w.put(seq.matrix_coefficients as u32, 8);
    }
    w.flag(seq.full_range); // color_range (subsampling is implied 4:2:0 for profile 0)
    w.put(CHROMA_SAMPLE_POSITION, 2);
    w.flag(false); // separate_uv_delta_q
    w.flag(false); // film_grain_params_present
    w.trailing_bits();
    w.finish()
}

/// The `av1C` record for the MP4 sample entry: four fixed bytes followed by the sequence header
/// OBU (`configOBUs`).
pub fn av1c(seq: &SeqParams) -> Vec<u8> {
    check(seq);
    let mut out = vec![
        0x80 | 1, // marker, version
        (seq.profile << 5) | seq.level_idx,
        // seq_tier_0 = 0, high_bitdepth, twelve_bit = 0, monochrome = 0, subsampling_x/y = 1, csp
        (u8::from(seq.bit_depth > 8) << 6) | (1 << 3) | (1 << 2) | CHROMA_SAMPLE_POSITION as u8,
        0, // reserved, initial_presentation_delay_present = 0, reserved
    ];
    out.extend(obu(ObuType::SequenceHeader, &write_sequence_header(seq)));
    out
}
