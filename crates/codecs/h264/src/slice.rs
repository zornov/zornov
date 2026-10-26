// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slice header (7.3.3) incl. ref_pic_list_modification, pred_weight_table and
//! dec_ref_pic_marking.

use crate::bitreader::BitReader;
use crate::error::{Error, Result, check};
use crate::params::{Pps, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    P,
    B,
    I,
    SP,
    SI,
}

impl SliceType {
    pub fn is_intra(self) -> bool {
        matches!(self, SliceType::I | SliceType::SI)
    }
    pub fn is_b(self) -> bool {
        self == SliceType::B
    }
    /// P or SP.
    pub fn is_p(self) -> bool {
        matches!(self, SliceType::P | SliceType::SP)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModOp {
    pub idc: u32,
    pub value: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mmco {
    pub op: u32,
    pub difference_of_pic_nums_minus1: u32,
    pub long_term_pic_num: u32,
    pub long_term_frame_idx: u32,
    pub max_long_term_frame_idx_plus1: u32,
}

/// Explicit weighted prediction parameters for one reference index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeightEntry {
    pub luma_weight: i32,
    pub luma_offset: i32,
    pub luma_flag: bool,
    pub chroma_weight: [i32; 2],
    pub chroma_offset: [i32; 2],
    pub chroma_flag: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PredWeightTable {
    pub luma_log2_denom: u32,
    pub chroma_log2_denom: u32,
    pub l0: Vec<WeightEntry>,
    pub l1: Vec<WeightEntry>,
}

#[derive(Debug, Clone)]
pub struct SliceHeader {
    pub nal_ref_idc: u8,
    pub idr: bool,
    pub first_mb_in_slice: u32,
    pub slice_type: SliceType,
    pub pps_id: u32,
    pub frame_num: u32,
    pub field_pic: bool,
    pub bottom_field: bool,
    pub idr_pic_id: u32,
    pub pic_order_cnt_lsb: u32,
    pub delta_pic_order_cnt_bottom: i32,
    pub delta_pic_order_cnt: [i32; 2],
    pub redundant_pic_cnt: u32,
    pub direct_spatial_mv_pred: bool,
    pub num_ref_idx_active: [u32; 2],
    pub ref_pic_list_modification: [Vec<ModOp>; 2],
    pub pred_weight: Option<PredWeightTable>,
    pub no_output_of_prior_pics: bool,
    pub long_term_reference_flag: bool,
    pub adaptive_ref_pic_marking: bool,
    pub mmcos: Vec<Mmco>,
    pub cabac_init_idc: u32,
    pub slice_qp_delta: i32,
    pub disable_deblocking_filter_idc: u32,
    pub slice_alpha_c0_offset_div2: i32,
    pub slice_beta_offset_div2: i32,
    pub slice_group_change_cycle: u32,
    /// Bit position of slice_data() in the RBSP.
    pub data_bit_offset: usize,
}

impl SliceHeader {
    pub fn has_mmco5(&self) -> bool {
        self.mmcos.iter().any(|m| m.op == 5)
    }

    pub fn slice_qp(&self, pps: &Pps) -> i32 {
        pps.pic_init_qp + self.slice_qp_delta
    }
}

/// Parse only the first fields (up to pps id) — used to look up the PPS.
pub fn peek_pps_id(rbsp: &[u8]) -> Result<u32> {
    let mut r = BitReader::new(rbsp);
    r.ue();
    r.ue();
    let id = r.ue_max(255, "pic_parameter_set_id")?;
    Ok(id)
}

fn parse_pred_weight_table(r: &mut BitReader, sps: &Sps, st: SliceType, n: [u32; 2]) -> Result<PredWeightTable> {
    let mut t = PredWeightTable { luma_log2_denom: r.ue_max(7, "luma_log2_weight_denom")?, ..Default::default() };
    let chroma = sps.chroma_array_type() != 0;
    if chroma {
        t.chroma_log2_denom = r.ue_max(7, "chroma_log2_weight_denom")?;
    }
    let lists = if st.is_b() { 2 } else { 1 };
    for list in 0..lists {
        let mut v = Vec::with_capacity(n[list] as usize);
        for _ in 0..n[list] {
            let mut e = WeightEntry {
                luma_weight: 1 << t.luma_log2_denom,
                luma_offset: 0,
                luma_flag: false,
                chroma_weight: [1 << t.chroma_log2_denom; 2],
                chroma_offset: [0; 2],
                chroma_flag: false,
            };
            e.luma_flag = r.flag();
            if e.luma_flag {
                e.luma_weight = r.se_range(-128, 127, "luma_weight")?;
                e.luma_offset = r.se_range(-128, 127, "luma_offset")?;
            }
            if chroma {
                e.chroma_flag = r.flag();
                if e.chroma_flag {
                    for c in 0..2 {
                        e.chroma_weight[c] = r.se_range(-128, 127, "chroma_weight")?;
                        e.chroma_offset[c] = r.se_range(-128, 127, "chroma_offset")?;
                    }
                }
            }
            v.push(e);
        }
        if list == 0 {
            t.l0 = v;
        } else {
            t.l1 = v;
        }
    }
    Ok(t)
}

/// Parse a slice header. `get_pps` resolves the PPS by id; the SPS is the PPS's.
pub fn parse_slice_header(rbsp: &[u8], nal_unit_type: u8, nal_ref_idc: u8, sps: &Sps, pps: &Pps) -> Result<SliceHeader> {
    let mut r = BitReader::new(rbsp);
    let idr = nal_unit_type == 5;
    let first_mb_in_slice = r.ue();
    let slice_type_raw = r.ue_max(9, "slice_type")?;
    let slice_type = match slice_type_raw % 5 {
        0 => SliceType::P,
        1 => SliceType::B,
        2 => SliceType::I,
        3 => SliceType::SP,
        _ => SliceType::SI,
    };
    let pps_id = r.ue_max(255, "pic_parameter_set_id")?;
    if sps.separate_colour_plane {
        r.read(2);
    }
    let frame_num = r.read(sps.log2_max_frame_num);
    let mut field_pic = false;
    let mut bottom_field = false;
    if !sps.frame_mbs_only {
        field_pic = r.flag();
        if field_pic {
            bottom_field = r.flag();
        }
    }
    let mbaff = sps.mb_adaptive_frame_field && !field_pic;
    let pic_size_in_mbs = sps.pic_width_in_mbs * sps.frame_height_in_mbs() / (1 + (field_pic || mbaff) as u32);
    check(first_mb_in_slice < pic_size_in_mbs, "first_mb_in_slice out of range")?;
    let idr_pic_id = if idr { r.ue_max(65535, "idr_pic_id")? } else { 0 };
    let mut pic_order_cnt_lsb = 0;
    let mut delta_pic_order_cnt_bottom = 0;
    let mut delta_pic_order_cnt = [0; 2];
    if sps.poc_type == 0 {
        pic_order_cnt_lsb = r.read(sps.log2_max_poc_lsb);
        if pps.bottom_field_pic_order_in_frame_present && !field_pic {
            delta_pic_order_cnt_bottom = r.se();
        }
    }
    if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
        delta_pic_order_cnt[0] = r.se();
        if pps.bottom_field_pic_order_in_frame_present && !field_pic {
            delta_pic_order_cnt[1] = r.se();
        }
    }
    let redundant_pic_cnt = if pps.redundant_pic_cnt_present { r.ue_max(127, "redundant_pic_cnt")? } else { 0 };
    let direct_spatial_mv_pred = if slice_type.is_b() { r.flag() } else { false };
    let mut num_ref_idx_active = [pps.num_ref_idx_l0_default_active, pps.num_ref_idx_l1_default_active];
    if !slice_type.is_intra() {
        if r.flag() {
            let max = if field_pic { 31 } else { 15 };
            num_ref_idx_active[0] = r.ue_max(max, "num_ref_idx_l0_active_minus1")? + 1;
            if slice_type.is_b() {
                num_ref_idx_active[1] = r.ue_max(max, "num_ref_idx_l1_active_minus1")? + 1;
            }
        }
        if !field_pic && (num_ref_idx_active[0] > 16 || num_ref_idx_active[1] > 16) {
            return Err(Error::Bitstream("num_ref_idx_active exceeds 16 for a frame"));
        }
    }
    if !slice_type.is_b() {
        num_ref_idx_active[1] = 0;
    }
    if slice_type.is_intra() {
        num_ref_idx_active = [0, 0];
    }
    let mut ref_pic_list_modification = [Vec::new(), Vec::new()];
    if nal_unit_type == 20 || nal_unit_type == 21 {
        return Err(Error::Unsupported("MVC slice extension".into()));
    }
    if !slice_type.is_intra() {
        let lists = if slice_type.is_b() { 2 } else { 1 };
        for list in 0..lists {
            if r.flag() {
                loop {
                    let idc = r.ue_max(5, "modification_of_pic_nums_idc")?;
                    if idc == 3 {
                        break;
                    }
                    if idc > 2 {
                        return Err(Error::Unsupported("MVC ref pic list modification".into()));
                    }
                    let value = r.ue();
                    ref_pic_list_modification[list].push(ModOp { idc, value });
                    if ref_pic_list_modification[list].len() > 100 || r.overrun() {
                        return Err(Error::Bitstream("ref_pic_list_modification too long"));
                    }
                }
            }
        }
    }
    let pred_weight = if (pps.weighted_pred && slice_type.is_p()) || (pps.weighted_bipred_idc == 1 && slice_type.is_b()) {
        Some(parse_pred_weight_table(&mut r, sps, slice_type, num_ref_idx_active)?)
    } else {
        None
    };
    let mut no_output_of_prior_pics = false;
    let mut long_term_reference_flag = false;
    let mut adaptive_ref_pic_marking = false;
    let mut mmcos = Vec::new();
    if nal_ref_idc != 0 {
        if idr {
            no_output_of_prior_pics = r.flag();
            long_term_reference_flag = r.flag();
        } else {
            adaptive_ref_pic_marking = r.flag();
            if adaptive_ref_pic_marking {
                loop {
                    let op = r.ue_max(6, "memory_management_control_operation")?;
                    if op == 0 {
                        break;
                    }
                    let mut m = Mmco {
                        op,
                        difference_of_pic_nums_minus1: 0,
                        long_term_pic_num: 0,
                        long_term_frame_idx: 0,
                        max_long_term_frame_idx_plus1: 0,
                    };
                    if op == 1 || op == 3 {
                        m.difference_of_pic_nums_minus1 = r.ue();
                    }
                    if op == 2 {
                        m.long_term_pic_num = r.ue();
                    }
                    if op == 3 || op == 6 {
                        m.long_term_frame_idx = r.ue_max(31, "long_term_frame_idx")?;
                    }
                    if op == 4 {
                        m.max_long_term_frame_idx_plus1 = r.ue_max(16, "max_long_term_frame_idx_plus1")?;
                    }
                    mmcos.push(m);
                    if mmcos.len() > 66 || r.overrun() {
                        return Err(Error::Bitstream("too many memory_management_control_operations"));
                    }
                }
            }
        }
    }
    let cabac_init_idc =
        if pps.entropy_coding_mode && !slice_type.is_intra() { r.ue_max(2, "cabac_init_idc")? } else { 0 };
    let slice_qp_delta = r.se();
    let qp = pps.pic_init_qp as i64 + slice_qp_delta as i64;
    check((0..=51).contains(&qp), "slice QP out of range")?;
    if matches!(slice_type, SliceType::SP | SliceType::SI) {
        if slice_type == SliceType::SP {
            r.flag(); // sp_for_switch_flag
        }
        let slice_qs_delta = r.se();
        let qs = pps.pic_init_qs as i64 + slice_qs_delta as i64;
        check((0..=51).contains(&qs), "slice QS out of range")?;
    }
    let mut disable_deblocking_filter_idc = 0;
    let mut slice_alpha_c0_offset_div2 = 0;
    let mut slice_beta_offset_div2 = 0;
    if pps.deblocking_filter_control_present {
        disable_deblocking_filter_idc = r.ue_max(2, "disable_deblocking_filter_idc")?;
        if disable_deblocking_filter_idc != 1 {
            slice_alpha_c0_offset_div2 = r.se_range(-6, 6, "slice_alpha_c0_offset_div2")?;
            slice_beta_offset_div2 = r.se_range(-6, 6, "slice_beta_offset_div2")?;
        }
    }
    let mut slice_group_change_cycle = 0;
    if pps.num_slice_groups > 1 && (3..=5).contains(&pps.slice_group_map_type) {
        let pic_size_in_map_units = sps.pic_width_in_mbs * sps.pic_height_in_map_units;
        let v = pic_size_in_map_units.div_ceil(pps.slice_group_change_rate) + 1;
        // Ceil(Log2(v))
        let bits = 32 - (v - 1).leading_zeros();
        slice_group_change_cycle = r.read(bits);
    }
    if r.overrun() {
        return Err(Error::Bitstream("truncated slice header"));
    }
    Ok(SliceHeader {
        nal_ref_idc,
        idr,
        first_mb_in_slice,
        slice_type,
        pps_id,
        frame_num,
        field_pic,
        bottom_field,
        idr_pic_id,
        pic_order_cnt_lsb,
        delta_pic_order_cnt_bottom,
        delta_pic_order_cnt,
        redundant_pic_cnt,
        direct_spatial_mv_pred,
        num_ref_idx_active,
        ref_pic_list_modification,
        pred_weight,
        no_output_of_prior_pics,
        long_term_reference_flag,
        adaptive_ref_pic_marking,
        mmcos,
        cabac_init_idc,
        slice_qp_delta,
        disable_deblocking_filter_idc,
        slice_alpha_c0_offset_div2,
        slice_beta_offset_div2,
        slice_group_change_cycle,
        data_bit_offset: r.pos(),
    })
}
