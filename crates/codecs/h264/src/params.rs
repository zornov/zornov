// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sequence and picture parameter sets (7.3.2.1, 7.3.2.2) incl. VUI (E.1.1)
//! and scaling matrices (7.3.2.1.1.1, Table 7-2 fall-back rules).

use crate::bitreader::BitReader;
use crate::error::{Error, Result, check};
use crate::tables::{DEFAULT_4X4_INTER, DEFAULT_4X4_INTRA, DEFAULT_8X8_INTER, DEFAULT_8X8_INTRA, ZIGZAG4, ZIGZAG8};

/// Scaling matrices in raster order: 6 lists of 4x4 (Intra Y/Cb/Cr, Inter Y/Cb/Cr) and
/// 6 lists of 8x8 (Intra Y, Inter Y, Intra Cb, Inter Cb, Intra Cr, Inter Cr).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingMatrices {
    pub m4: [[u8; 16]; 6],
    pub m8: [[u8; 64]; 6],
}

impl ScalingMatrices {
    pub fn flat() -> Self {
        ScalingMatrices { m4: [[16; 16]; 6], m8: [[16; 64]; 6] }
    }
}

fn raster4(zz: &[u8; 16]) -> [u8; 16] {
    let mut r = [0u8; 16];
    for k in 0..16 {
        r[ZIGZAG4[k] as usize] = zz[k];
    }
    r
}

fn raster8(zz: &[u8; 64]) -> [u8; 64] {
    let mut r = [0u8; 64];
    for k in 0..64 {
        r[ZIGZAG8[k] as usize] = zz[k];
    }
    r
}

/// scaling_list() syntax. Returns `None` when useDefaultScalingMatrixFlag is set,
/// otherwise the list in zig-zag scan order.
fn parse_scaling_list<const N: usize>(r: &mut BitReader) -> Result<Option<[u8; N]>> {
    let mut list = [0u8; N];
    let mut last = 8i32;
    let mut next = 8i32;
    for j in 0..N {
        if next != 0 {
            let delta = r.se_range(-128, 127, "delta_scale out of range")?;
            next = (last + delta + 256) % 256;
            if j == 0 && next == 0 {
                return Ok(None);
            }
        }
        list[j] = if next == 0 { last as u8 } else { next as u8 };
        last = list[j] as i32;
    }
    Ok(Some(list))
}

/// Parse the scaling list block of an SPS or PPS.
/// `fallback` is `None` for rule A, or the SPS matrices for rule B.
fn parse_matrices(r: &mut BitReader, count: usize, fallback: Option<&ScalingMatrices>) -> Result<ScalingMatrices> {
    let mut m = ScalingMatrices::flat();
    for i in 0..count {
        let present = r.flag();
        if i < 6 {
            let def = if i < 3 { DEFAULT_4X4_INTRA } else { DEFAULT_4X4_INTER };
            if present {
                match parse_scaling_list::<16>(r)? {
                    None => m.m4[i] = raster4(&def),
                    Some(l) => m.m4[i] = raster4(&l),
                }
            } else {
                m.m4[i] = match (i, fallback) {
                    (0, None) | (3, None) => raster4(&def),
                    (0, Some(f)) => f.m4[0],
                    (3, Some(f)) => f.m4[3],
                    _ => m.m4[i - 1],
                };
            }
        } else {
            let k = i - 6;
            let def = if k % 2 == 0 { DEFAULT_8X8_INTRA } else { DEFAULT_8X8_INTER };
            if present {
                match parse_scaling_list::<64>(r)? {
                    None => m.m8[k] = raster8(&def),
                    Some(l) => m.m8[k] = raster8(&l),
                }
            } else {
                m.m8[k] = match (k, fallback) {
                    (0, None) | (1, None) => raster8(&def),
                    (0, Some(f)) => f.m8[0],
                    (1, Some(f)) => f.m8[1],
                    _ => m.m8[k - 2],
                };
            }
        }
    }
    // Lists not transmitted (8x8 when transform_8x8_mode is off, or 4:4:4 chroma 8x8) are
    // filled by the same fall-back rules so every entry is defined.
    for k in count.saturating_sub(6).max(0)..6 {
        if k < 2 {
            let def = if k % 2 == 0 { DEFAULT_8X8_INTRA } else { DEFAULT_8X8_INTER };
            m.m8[k] = match fallback {
                None => raster8(&def),
                Some(f) => f.m8[k],
            };
        } else {
            m.m8[k] = m.m8[k - 2];
        }
    }
    Ok(m)
}

/// Video usability information subset that affects output.
#[derive(Debug, Clone, Default)]
pub struct Vui {
    pub aspect_ratio_idc: u8,
    pub sar_width: u16,
    pub sar_height: u16,
    pub video_signal_type_present: bool,
    pub video_format: u8,
    pub video_full_range_flag: bool,
    pub colour_description_present: bool,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub timing_info_present: bool,
    pub num_units_in_tick: u32,
    pub time_scale: u32,
    pub fixed_frame_rate_flag: bool,
    pub bitstream_restriction: bool,
    pub max_num_reorder_frames: u32,
    pub max_dec_frame_buffering: u32,
}

fn skip_hrd(r: &mut BitReader) -> Result<()> {
    let cpb_cnt = r.ue_max(31, "cpb_cnt_minus1")? + 1;
    r.read(4); // bit_rate_scale
    r.read(4); // cpb_size_scale
    for _ in 0..cpb_cnt {
        r.ue(); // bit_rate_value_minus1
        r.ue(); // cpb_size_value_minus1
        r.flag(); // cbr_flag
    }
    r.read(5); // initial_cpb_removal_delay_length_minus1
    r.read(5); // cpb_removal_delay_length_minus1
    r.read(5); // dpb_output_delay_length_minus1
    r.read(5); // time_offset_length
    Ok(())
}

fn parse_vui(r: &mut BitReader) -> Result<Vui> {
    let mut v = Vui::default();
    if r.flag() {
        v.aspect_ratio_idc = r.read(8) as u8;
        if v.aspect_ratio_idc == 255 {
            v.sar_width = r.read(16) as u16;
            v.sar_height = r.read(16) as u16;
        }
    }
    if r.flag() {
        r.flag(); // overscan_appropriate_flag
    }
    v.video_signal_type_present = r.flag();
    if v.video_signal_type_present {
        v.video_format = r.read(3) as u8;
        v.video_full_range_flag = r.flag();
        v.colour_description_present = r.flag();
        if v.colour_description_present {
            v.colour_primaries = r.read(8) as u8;
            v.transfer_characteristics = r.read(8) as u8;
            v.matrix_coefficients = r.read(8) as u8;
        }
    }
    if r.flag() {
        r.ue(); // chroma_sample_loc_type_top_field
        r.ue(); // chroma_sample_loc_type_bottom_field
    }
    v.timing_info_present = r.flag();
    if v.timing_info_present {
        v.num_units_in_tick = r.read(32);
        v.time_scale = r.read(32);
        v.fixed_frame_rate_flag = r.flag();
    }
    let nal_hrd = r.flag();
    if nal_hrd {
        skip_hrd(r)?;
    }
    let vcl_hrd = r.flag();
    if vcl_hrd {
        skip_hrd(r)?;
    }
    if nal_hrd || vcl_hrd {
        r.flag(); // low_delay_hrd_flag
    }
    r.flag(); // pic_struct_present_flag
    v.bitstream_restriction = r.flag();
    if v.bitstream_restriction {
        r.flag(); // motion_vectors_over_pic_boundaries_flag
        r.ue(); // max_bytes_per_pic_denom
        r.ue(); // max_bits_per_mb_denom
        r.ue(); // log2_max_mv_length_horizontal
        r.ue(); // log2_max_mv_length_vertical
        v.max_num_reorder_frames = r.ue();
        v.max_dec_frame_buffering = r.ue();
    }
    Ok(v)
}

#[derive(Debug, Clone)]
pub struct Sps {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub sps_id: u32,
    pub chroma_format_idc: u32,
    pub separate_colour_plane: bool,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub qpprime_y_zero_transform_bypass: bool,
    pub scaling_matrix_present: bool,
    pub scaling: ScalingMatrices,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    pub delta_pic_order_always_zero: bool,
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub offset_for_ref_frame: Vec<i32>,
    pub max_num_ref_frames: u32,
    pub gaps_in_frame_num_allowed: bool,
    pub pic_width_in_mbs: u32,
    pub pic_height_in_map_units: u32,
    pub frame_mbs_only: bool,
    pub mb_adaptive_frame_field: bool,
    pub direct_8x8_inference: bool,
    pub frame_cropping: bool,
    pub crop_left: u32,
    pub crop_right: u32,
    pub crop_top: u32,
    pub crop_bottom: u32,
    pub vui_present: bool,
    pub vui: Vui,
}

impl Sps {
    pub fn frame_height_in_mbs(&self) -> u32 {
        (2 - self.frame_mbs_only as u32) * self.pic_height_in_map_units
    }

    pub fn chroma_array_type(&self) -> u32 {
        if self.separate_colour_plane { 0 } else { self.chroma_format_idc }
    }

    pub fn max_frame_num(&self) -> u32 {
        1 << self.log2_max_frame_num
    }

    /// Cropped output size in luma samples.
    pub fn cropped_size(&self) -> (u32, u32) {
        let (cx, cy) = self.crop_units();
        let w = self.pic_width_in_mbs * 16;
        let h = self.frame_height_in_mbs() * 16;
        (w - cx * (self.crop_left + self.crop_right), h - cy * (self.crop_top + self.crop_bottom))
    }

    /// (CropUnitX, CropUnitY), 7.4.2.1.1.
    pub fn crop_units(&self) -> (u32, u32) {
        let fm = 2 - self.frame_mbs_only as u32;
        match self.chroma_array_type() {
            0 => (1, fm),
            1 => (2, 2 * fm),
            2 => (2, fm),
            _ => (1, fm),
        }
    }

    /// MaxDpbFrames from Table A-1 (or VUI), at least max_num_ref_frames, at most 16.
    pub fn max_dpb_frames(&self) -> usize {
        let max_dpb_mbs: u32 = match self.level_idc {
            9 | 10 => 396,
            11 => {
                if self.constraint_flags & 0x10 != 0 && matches!(self.profile_idc, 66 | 77 | 88) {
                    396 // level 1b
                } else {
                    900
                }
            }
            12 | 13 | 20 => 2376,
            21 => 4752,
            22 | 30 => 8100,
            31 => 18000,
            32 => 20480,
            40 | 41 => 32768,
            42 => 34816,
            50 => 110400,
            51 | 52 => 184320,
            _ => 696320,
        };
        let frame_mbs = (self.pic_width_in_mbs * self.frame_height_in_mbs()).max(1);
        let mut n = (max_dpb_mbs / frame_mbs).clamp(1, 16) as usize;
        if self.vui_present && self.vui.bitstream_restriction {
            n = (self.vui.max_dec_frame_buffering as usize).clamp(1, 16);
        }
        n.max(self.max_num_ref_frames as usize).clamp(1, 16)
    }

    pub fn parse(rbsp: &[u8]) -> Result<Sps> {
        let mut r = BitReader::new(rbsp);
        let profile_idc = r.read(8) as u8;
        let constraint_flags = r.read(8) as u8;
        let level_idc = r.read(8) as u8;
        let sps_id = r.ue_max(31, "seq_parameter_set_id")?;
        let mut chroma_format_idc = 1;
        let mut separate_colour_plane = false;
        let mut bit_depth_luma = 8;
        let mut bit_depth_chroma = 8;
        let mut bypass = false;
        let mut scaling_matrix_present = false;
        let mut scaling = ScalingMatrices::flat();
        if matches!(profile_idc, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
            chroma_format_idc = r.ue_max(3, "chroma_format_idc")?;
            if chroma_format_idc == 3 {
                separate_colour_plane = r.flag();
            }
            bit_depth_luma = r.ue_max(6, "bit_depth_luma_minus8")? + 8;
            bit_depth_chroma = r.ue_max(6, "bit_depth_chroma_minus8")? + 8;
            bypass = r.flag();
            scaling_matrix_present = r.flag();
            if scaling_matrix_present {
                let count = if chroma_format_idc != 3 { 8 } else { 12 };
                scaling = parse_matrices(&mut r, count, None)?;
            }
        }
        let log2_max_frame_num = r.ue_max(12, "log2_max_frame_num_minus4")? + 4;
        let poc_type = r.ue_max(2, "pic_order_cnt_type")?;
        let mut log2_max_poc_lsb = 0;
        let mut delta_pic_order_always_zero = false;
        let mut offset_for_non_ref_pic = 0;
        let mut offset_for_top_to_bottom_field = 0;
        let mut offset_for_ref_frame = Vec::new();
        if poc_type == 0 {
            log2_max_poc_lsb = r.ue_max(12, "log2_max_pic_order_cnt_lsb_minus4")? + 4;
        } else if poc_type == 1 {
            delta_pic_order_always_zero = r.flag();
            offset_for_non_ref_pic = r.se();
            offset_for_top_to_bottom_field = r.se();
            let n = r.ue_max(255, "num_ref_frames_in_pic_order_cnt_cycle")?;
            for _ in 0..n {
                offset_for_ref_frame.push(r.se());
            }
        }
        let max_num_ref_frames = r.ue_max(16, "max_num_ref_frames")?;
        let gaps_in_frame_num_allowed = r.flag();
        let pic_width_in_mbs = r.ue_max(1023, "pic_width_in_mbs_minus1")? + 1;
        let pic_height_in_map_units = r.ue_max(1023, "pic_height_in_map_units_minus1")? + 1;
        let frame_mbs_only = r.flag();
        let mb_adaptive_frame_field = if !frame_mbs_only { r.flag() } else { false };
        let direct_8x8_inference = r.flag();
        let frame_cropping = r.flag();
        let (mut crop_left, mut crop_right, mut crop_top, mut crop_bottom) = (0, 0, 0, 0);
        if frame_cropping {
            crop_left = r.ue();
            crop_right = r.ue();
            crop_top = r.ue();
            crop_bottom = r.ue();
        }
        let vui_present = r.flag();
        let vui = if vui_present {
            // A truncated/odd VUI must not make the stream undecodable.
            let mut rr = r.clone();
            match parse_vui(&mut rr) {
                Ok(v) if !rr.overrun() => v,
                _ => Vui::default(),
            }
        } else {
            Vui::default()
        };
        let sps = Sps {
            profile_idc,
            constraint_flags,
            level_idc,
            sps_id,
            chroma_format_idc,
            separate_colour_plane,
            bit_depth_luma,
            bit_depth_chroma,
            qpprime_y_zero_transform_bypass: bypass,
            scaling_matrix_present,
            scaling,
            log2_max_frame_num,
            poc_type,
            log2_max_poc_lsb,
            delta_pic_order_always_zero,
            offset_for_non_ref_pic,
            offset_for_top_to_bottom_field,
            offset_for_ref_frame,
            max_num_ref_frames,
            gaps_in_frame_num_allowed,
            pic_width_in_mbs,
            pic_height_in_map_units,
            frame_mbs_only,
            mb_adaptive_frame_field,
            direct_8x8_inference,
            frame_cropping,
            crop_left,
            crop_right,
            crop_top,
            crop_bottom,
            vui_present,
            vui,
        };
        if r.overrun() {
            return Err(Error::Bitstream("truncated SPS"));
        }
        let (cx, cy) = sps.crop_units();
        let w = sps.pic_width_in_mbs * 16;
        let h = sps.frame_height_in_mbs() * 16;
        check(
            (sps.crop_left as u64 + sps.crop_right as u64) * (cx as u64) < w as u64
                && (sps.crop_top as u64 + sps.crop_bottom as u64) * (cy as u64) < h as u64,
            "frame cropping exceeds picture size",
        )?;
        Ok(sps)
    }

    /// Check that this decoder implements every tool the SPS enables.
    pub fn check_supported(&self) -> Result<()> {
        if !matches!(self.profile_idc, 66 | 77 | 88 | 100 | 110 | 122 | 244) {
            return Err(Error::Unsupported(format!(
                "profile_idc {} (supported: Baseline 66, Main 77, Extended 88, High 100)",
                self.profile_idc
            )));
        }
        if self.bit_depth_luma != 8 || self.bit_depth_chroma != 8 {
            return Err(Error::Unsupported(format!(
                "bit depth luma {} / chroma {} (only 8-bit is supported)",
                self.bit_depth_luma, self.bit_depth_chroma
            )));
        }
        if self.chroma_format_idc > 1 {
            return Err(Error::Unsupported(format!(
                "chroma_format_idc {} (only 4:2:0 and monochrome are supported)",
                self.chroma_format_idc
            )));
        }
        if self.qpprime_y_zero_transform_bypass {
            return Err(Error::Unsupported("qpprime_y_zero_transform_bypass (lossless)".into()));
        }
        if self.separate_colour_plane {
            return Err(Error::Unsupported("separate_colour_plane_flag (4:4:4)".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Pps {
    pub pps_id: u32,
    pub sps_id: u32,
    pub entropy_coding_mode: bool,
    pub bottom_field_pic_order_in_frame_present: bool,
    pub num_slice_groups: u32,
    pub slice_group_map_type: u32,
    pub run_length_minus1: Vec<u32>,
    pub top_left: Vec<u32>,
    pub bottom_right: Vec<u32>,
    pub slice_group_change_direction: bool,
    pub slice_group_change_rate: u32,
    pub slice_group_id: Vec<u32>,
    pub num_ref_idx_l0_default_active: u32,
    pub num_ref_idx_l1_default_active: u32,
    pub weighted_pred: bool,
    pub weighted_bipred_idc: u32,
    pub pic_init_qp: i32,
    pub pic_init_qs: i32,
    pub chroma_qp_index_offset: i32,
    pub second_chroma_qp_index_offset: i32,
    pub deblocking_filter_control_present: bool,
    pub constrained_intra_pred: bool,
    pub redundant_pic_cnt_present: bool,
    pub transform_8x8_mode: bool,
    pub pic_scaling_matrix_present: bool,
    /// Effective scaling matrices for slices using this PPS (SPS/PPS rules applied).
    pub scaling: ScalingMatrices,
}

impl Pps {
    pub fn parse(rbsp: &[u8], sps_list: &[Option<std::sync::Arc<Sps>>]) -> Result<Pps> {
        let mut r = BitReader::new(rbsp);
        let pps_id = r.ue_max(255, "pic_parameter_set_id")?;
        let sps_id = r.ue_max(31, "seq_parameter_set_id")?;
        let sps = sps_list.get(sps_id as usize).and_then(|s| s.clone()).ok_or(Error::MissingParameterSet("SPS for PPS"))?;
        let entropy_coding_mode = r.flag();
        let bottom_field_pic_order_in_frame_present = r.flag();
        let num_slice_groups = r.ue_max(7, "num_slice_groups_minus1")? + 1;
        let mut slice_group_map_type = 0;
        let mut run_length_minus1 = Vec::new();
        let mut top_left = Vec::new();
        let mut bottom_right = Vec::new();
        let mut slice_group_change_direction = false;
        let mut slice_group_change_rate = 1;
        let mut slice_group_id = Vec::new();
        if num_slice_groups > 1 {
            slice_group_map_type = r.ue_max(6, "slice_group_map_type")?;
            match slice_group_map_type {
                0 => {
                    for _ in 0..num_slice_groups {
                        run_length_minus1.push(r.ue());
                    }
                }
                2 => {
                    for _ in 0..num_slice_groups - 1 {
                        top_left.push(r.ue());
                        bottom_right.push(r.ue());
                    }
                }
                3..=5 => {
                    slice_group_change_direction = r.flag();
                    let pic_size = sps.pic_width_in_mbs * sps.pic_height_in_map_units;
                    slice_group_change_rate = r.ue_max(pic_size - 1, "slice_group_change_rate_minus1")? + 1;
                }
                6 => {
                    let n = r.ue_max(1 << 20, "pic_size_in_map_units_minus1")? + 1;
                    let bits = 32 - (num_slice_groups - 1).leading_zeros();
                    for _ in 0..n {
                        slice_group_id.push(r.read(bits));
                    }
                }
                _ => {}
            }
        }
        let num_ref_idx_l0_default_active = r.ue_max(31, "num_ref_idx_l0_default_active_minus1")? + 1;
        let num_ref_idx_l1_default_active = r.ue_max(31, "num_ref_idx_l1_default_active_minus1")? + 1;
        let weighted_pred = r.flag();
        let weighted_bipred_idc = r.read(2);
        check(weighted_bipred_idc < 3, "weighted_bipred_idc")?;
        let pic_init_qp = 26 + r.se_range(-26, 25, "pic_init_qp_minus26")?;
        let pic_init_qs = 26 + r.se_range(-26, 25, "pic_init_qs_minus26")?;
        let chroma_qp_index_offset = r.se_range(-12, 12, "chroma_qp_index_offset")?;
        let deblocking_filter_control_present = r.flag();
        let constrained_intra_pred = r.flag();
        let redundant_pic_cnt_present = r.flag();
        let mut transform_8x8_mode = false;
        let mut pic_scaling_matrix_present = false;
        let mut second_chroma_qp_index_offset = chroma_qp_index_offset;
        let mut scaling = sps.scaling.clone();
        if r.more_rbsp_data() {
            transform_8x8_mode = r.flag();
            pic_scaling_matrix_present = r.flag();
            if pic_scaling_matrix_present {
                let count = 6 + if sps.chroma_format_idc != 3 { 2 } else { 6 } * transform_8x8_mode as usize;
                let fallback = if sps.scaling_matrix_present { Some(&sps.scaling) } else { None };
                scaling = parse_matrices(&mut r, count, fallback)?;
            }
            second_chroma_qp_index_offset = r.se_range(-12, 12, "second_chroma_qp_index_offset")?;
        }
        if r.overrun() {
            return Err(Error::Bitstream("truncated PPS"));
        }
        Ok(Pps {
            pps_id,
            sps_id,
            entropy_coding_mode,
            bottom_field_pic_order_in_frame_present,
            num_slice_groups,
            slice_group_map_type,
            run_length_minus1,
            top_left,
            bottom_right,
            slice_group_change_direction,
            slice_group_change_rate,
            slice_group_id,
            num_ref_idx_l0_default_active,
            num_ref_idx_l1_default_active,
            weighted_pred,
            weighted_bipred_idc,
            pic_init_qp,
            pic_init_qs,
            chroma_qp_index_offset,
            second_chroma_qp_index_offset,
            deblocking_filter_control_present,
            constrained_intra_pred,
            redundant_pic_cnt_present,
            transform_8x8_mode,
            pic_scaling_matrix_present,
            scaling,
        })
    }
}
