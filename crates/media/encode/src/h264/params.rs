// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sequence / picture parameter sets with VUI, slice headers, levels and the
//! `avcC` decoder configuration record.

use crate::bits::BitWriter;

/// Colour description written to the VUI (and the container's `colr` box).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorDesc {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

impl ColorDesc {
    /// ITU-R BT.709, limited range ("Rec. 709 SDR").
    pub const BT709: ColorDesc = ColorDesc { primaries: 1, transfer: 1, matrix: 1, full_range: false };
}

pub struct SeqParams {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub mb_w: u32,
    pub mb_h: u32,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub log2_max_frame_num: u32,
    pub color: ColorDesc,
}

/// (level_idc, MaxMBPS, MaxFS, MaxBR in kbit/s for Baseline/Main), Table A-1.
const LEVELS: [(u8, u64, u64, u64); 18] = [
    (10, 1485, 99, 64),
    (11, 3000, 396, 192),
    (12, 6000, 396, 384),
    (13, 11880, 396, 768),
    (20, 11880, 396, 2000),
    (21, 19800, 792, 4000),
    (22, 20250, 1620, 4000),
    (30, 40500, 1620, 10000),
    (31, 108000, 3600, 14000),
    (32, 216000, 5120, 20000),
    (40, 245760, 8192, 20000),
    (41, 245760, 8192, 50000),
    (42, 522240, 8704, 50000),
    (50, 589824, 22080, 135000),
    (51, 983040, 36864, 240000),
    (52, 2073600, 36864, 240000),
    (60, 4177920, 139264, 240000),
    (61, 8355840, 139264, 480000),
];

/// Smallest level that fits the frame size, macroblock rate and bit rate.
pub fn pick_level(mb_w: u32, mb_h: u32, fps: f64, kbps: u32) -> u8 {
    let fs = mb_w as u64 * mb_h as u64;
    let mbps = (fs as f64 * fps).ceil() as u64;
    for &(idc, max_mbps, max_fs, max_br) in &LEVELS {
        let side = ((max_fs * 8) as f64).sqrt() as u64;
        if fs <= max_fs && mbps <= max_mbps && (kbps as u64) <= max_br && (mb_w as u64) <= side && (mb_h as u64) <= side {
            return idc;
        }
    }
    62
}

impl SeqParams {
    /// seq_parameter_set_rbsp().
    pub fn sps_rbsp(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(8, self.profile_idc as u32);
        w.put(8, self.constraint_flags as u32);
        w.put(8, self.level_idc as u32);
        w.ue(0); // seq_parameter_set_id
        w.ue(self.log2_max_frame_num - 4);
        w.ue(2); // pic_order_cnt_type: output order = decoding order
        w.ue(1); // max_num_ref_frames
        w.put_bit(false); // gaps_in_frame_num_value_allowed_flag
        w.ue(self.mb_w - 1);
        w.ue(self.mb_h - 1);
        w.put_bit(true); // frame_mbs_only_flag
        w.put_bit(true); // direct_8x8_inference_flag
        let crop_r = (self.mb_w * 16 - self.width) / 2;
        let crop_b = (self.mb_h * 16 - self.height) / 2;
        let crop = crop_r > 0 || crop_b > 0;
        w.put_bit(crop);
        if crop {
            w.ue(0);
            w.ue(crop_r);
            w.ue(0);
            w.ue(crop_b);
        }
        w.put_bit(true); // vui_parameters_present_flag
        self.vui(&mut w);
        w.trailing();
        w.finish()
    }

    fn vui(&self, w: &mut BitWriter) {
        w.put_bit(true); // aspect_ratio_info_present_flag
        w.put(8, 1); // 1:1
        w.put_bit(false); // overscan_info_present_flag
        w.put_bit(true); // video_signal_type_present_flag
        w.put(3, 5); // video_format: unspecified
        w.put_bit(self.color.full_range);
        w.put_bit(true); // colour_description_present_flag
        w.put(8, self.color.primaries as u32);
        w.put(8, self.color.transfer as u32);
        w.put(8, self.color.matrix as u32);
        w.put_bit(false); // chroma_loc_info_present_flag
        w.put_bit(true); // timing_info_present_flag
        w.put(32, self.fps_den);
        w.put(32, self.fps_num * 2);
        w.put_bit(true); // fixed_frame_rate_flag
        w.put_bit(false); // nal_hrd_parameters_present_flag
        w.put_bit(false); // vcl_hrd_parameters_present_flag
        w.put_bit(false); // pic_struct_present_flag
        w.put_bit(true); // bitstream_restriction_flag
        w.put_bit(true); // motion_vectors_over_pic_boundaries_flag
        w.ue(2); // max_bytes_per_pic_denom
        w.ue(1); // max_bits_per_mb_denom
        w.ue(11); // log2_max_mv_length_horizontal (+-512 samples)
        w.ue(9); // log2_max_mv_length_vertical (+-128 samples)
        w.ue(0); // max_num_reorder_frames
        w.ue(1); // max_dec_frame_buffering
    }
}

/// pic_parameter_set_rbsp().
pub fn pps_rbsp(cabac: bool) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(0); // pic_parameter_set_id
    w.ue(0); // seq_parameter_set_id
    w.put_bit(cabac); // entropy_coding_mode_flag
    w.put_bit(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.put_bit(false); // weighted_pred_flag
    w.put(2, 0); // weighted_bipred_idc
    w.se(0); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.put_bit(true); // deblocking_filter_control_present_flag
    w.put_bit(false); // constrained_intra_pred_flag
    w.put_bit(false); // redundant_pic_cnt_present_flag
    w.trailing();
    w.finish()
}

/// Fields of one slice header.
pub struct SliceHeader {
    pub first_mb: u32,
    pub idr: bool,
    pub p: bool,
    pub frame_num: u32,
    pub log2_max_frame_num: u32,
    pub idr_pic_id: u32,
    pub qp: i32,
    pub cabac: bool,
}

impl SliceHeader {
    pub fn write(&self, w: &mut BitWriter) {
        w.ue(self.first_mb);
        w.ue(if self.p { 5 } else { 7 }); // all slices of the picture share the type
        w.ue(0); // pic_parameter_set_id
        w.put(self.log2_max_frame_num, self.frame_num);
        if self.idr {
            w.ue(self.idr_pic_id);
        }
        if self.p {
            w.put_bit(false); // num_ref_idx_active_override_flag
            w.put_bit(false); // ref_pic_list_modification_flag_l0
        }
        // dec_ref_pic_marking() (nal_ref_idc != 0)
        if self.idr {
            w.put_bit(false); // no_output_of_prior_pics_flag
            w.put_bit(false); // long_term_reference_flag
        } else {
            w.put_bit(false); // adaptive_ref_pic_marking_mode_flag
        }
        if self.cabac && self.p {
            w.ue(0); // cabac_init_idc
        }
        w.se(self.qp - 26); // slice_qp_delta
        w.ue(0); // disable_deblocking_filter_idc
        w.se(0); // slice_alpha_c0_offset_div2
        w.se(0); // slice_beta_offset_div2
    }
}

/// AVCDecoderConfigurationRecord for the `avcC` box.
pub fn avcc(sps_nal: &[u8], pps_nal: &[u8]) -> Vec<u8> {
    let mut v = vec![1, sps_nal[1], sps_nal[2], sps_nal[3], 0xFF, 0xE1];
    v.extend_from_slice(&(sps_nal.len() as u16).to_be_bytes());
    v.extend_from_slice(sps_nal);
    v.push(1);
    v.extend_from_slice(&(pps_nal.len() as u16).to_be_bytes());
    v.extend_from_slice(pps_nal);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(pick_level(120, 68, 30.0, 10_000), 40);
        assert_eq!(pick_level(120, 68, 60.0, 20_000), 42);
        assert_eq!(pick_level(80, 45, 30.0, 5_000), 31);
        assert_eq!(pick_level(240, 135, 30.0, 40_000), 51);
        assert_eq!(pick_level(480, 270, 30.0, 100_000), 60);
    }
}
