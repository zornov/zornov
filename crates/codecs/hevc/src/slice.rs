// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slice segment header parsing (7.3.6).

use crate::bitreader::BitReader;
use crate::error::{Error, Result};
use crate::nal::NalHeader;
use crate::params::{parse_st_rps, Pps, Sps, StRps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    B = 0,
    P = 1,
    I = 2,
}

/// Explicit weighted prediction parameters (7.3.6.3), already derived.
#[derive(Debug, Clone)]
pub struct PredWeightTable {
    pub luma_log2_denom: u8,
    pub chroma_log2_denom: u8,
    /// [list][refIdx][component] weights
    pub weight: [[[i32; 3]; 16]; 2],
    /// [list][refIdx][component] offsets (at 8-bit precision, before bit-depth scaling)
    pub offset: [[[i32; 3]; 16]; 2],
}

impl Default for PredWeightTable {
    fn default() -> Self {
        PredWeightTable {
            luma_log2_denom: 0,
            chroma_log2_denom: 0,
            weight: [[[1; 3]; 16]; 2],
            offset: [[[0; 3]; 16]; 2],
        }
    }
}

#[derive(Debug, Clone)]
pub struct SliceHeader {
    pub first_slice_segment_in_pic: bool,
    pub no_output_of_prior_pics: bool,
    pub pps_id: u8,
    pub dependent_slice_segment: bool,
    pub segment_address: usize,
    /// SliceAddrRs: address of the first CTB of the (independent) slice.
    pub slice_addr_rs: usize,
    pub slice_type: SliceType,
    pub pic_output_flag: bool,
    pub poc_lsb: i32,
    pub st_rps: StRps,
    /// Number of bits used by st_ref_pic_set() in the slice header (0 if from SPS).
    pub st_rps_bits: usize,
    pub num_long_term: usize,
    pub lt_poc_lsb: Vec<i32>,
    pub lt_used_by_curr: Vec<bool>,
    pub lt_delta_poc_msb_present: Vec<bool>,
    pub lt_delta_poc_msb_cycle: Vec<i32>, // cumulative DeltaPocMsbCycleLt
    pub temporal_mvp_enabled: bool,
    pub sao_luma: bool,
    pub sao_chroma: bool,
    pub num_ref_idx_active: [usize; 2],
    pub rpl_modification: [bool; 2],
    pub list_entry: [[u8; 16]; 2],
    pub mvd_l1_zero: bool,
    pub cabac_init_flag: bool,
    pub collocated_from_l0: bool,
    pub collocated_ref_idx: usize,
    pub pwt: Option<PredWeightTable>,
    pub max_num_merge_cand: usize,
    pub slice_qp: i32,
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub deblocking_filter_disabled: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub loop_filter_across_slices_enabled: bool,
    pub entry_point_offsets: Vec<u32>,
    /// Byte offset of slice_segment_data() within the RBSP.
    pub data_offset: usize,
}

impl SliceHeader {
    pub fn is_intra(&self) -> bool {
        self.slice_type == SliceType::I
    }
    pub fn is_b(&self) -> bool {
        self.slice_type == SliceType::B
    }
    /// NumPicTotalCurr (7-55).
    pub fn num_pic_total_curr(&self) -> usize {
        let mut n = 0;
        for i in 0..self.st_rps.num_negative {
            if self.st_rps.used_s0[i] {
                n += 1;
            }
        }
        for i in 0..self.st_rps.num_positive {
            if self.st_rps.used_s1[i] {
                n += 1;
            }
        }
        for i in 0..self.num_long_term {
            if self.lt_used_by_curr[i] {
                n += 1;
            }
        }
        n
    }
    /// initType for CABAC context initialisation (9.3.2.2).
    pub fn init_type(&self) -> usize {
        match self.slice_type {
            SliceType::I => 0,
            SliceType::P => {
                if self.cabac_init_flag {
                    2
                } else {
                    1
                }
            }
            SliceType::B => {
                if self.cabac_init_flag {
                    1
                } else {
                    2
                }
            }
        }
    }
}

fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

/// Reads first_slice_segment_in_pic_flag and slice_pic_parameter_set_id only.
pub fn peek_pps_id(rbsp: &[u8], nal: &NalHeader) -> Result<(bool, u8)> {
    let mut r = BitReader::new(rbsp);
    r.skip(16)?;
    let first = r.flag()?;
    if nal.is_irap() {
        r.skip(1)?;
    }
    let id = r.ue_max(63, "slice_pic_parameter_set_id")? as u8;
    Ok((first, id))
}

fn parse_pred_weight_table(
    r: &mut BitReader,
    sps: &Sps,
    sh: &SliceHeader,
) -> Result<PredWeightTable> {
    let mut t = PredWeightTable::default();
    t.luma_log2_denom = r.ue_max(7, "luma_log2_weight_denom")? as u8;
    let chroma = sps.chroma_format_idc != 0;
    if chroma {
        let d = r.se()? + t.luma_log2_denom as i32;
        if !(0..=7).contains(&d) {
            return Err(Error::Invalid("delta_chroma_log2_weight_denom"));
        }
        t.chroma_log2_denom = d as u8;
    }
    let lists = if sh.is_b() { 2 } else { 1 };
    for l in 0..lists {
        let n = sh.num_ref_idx_active[l];
        let mut luma_flags = [false; 16];
        let mut chroma_flags = [false; 16];
        for f in luma_flags.iter_mut().take(n) {
            *f = r.flag()?;
        }
        if chroma {
            for f in chroma_flags.iter_mut().take(n) {
                *f = r.flag()?;
            }
        }
        for i in 0..n {
            t.weight[l][i][0] = 1 << t.luma_log2_denom;
            t.offset[l][i][0] = 0;
            if luma_flags[i] {
                let dw = r.se_range(-128, 127, "delta_luma_weight")?;
                t.weight[l][i][0] += dw;
                t.offset[l][i][0] = r.se_range(-128, 127, "luma_offset")?;
            }
            for j in 0..2 {
                t.weight[l][i][1 + j] = 1 << t.chroma_log2_denom;
                t.offset[l][i][1 + j] = 0;
            }
            if chroma_flags[i] {
                for j in 0..2 {
                    let dw = r.se_range(-128, 127, "delta_chroma_weight")?;
                    let w = (1 << t.chroma_log2_denom) + dw;
                    let doff = r.se_range(-512, 511, "delta_chroma_offset")?;
                    let off = (128 + doff - ((128 * w) >> t.chroma_log2_denom)).clamp(-128, 127);
                    t.weight[l][i][1 + j] = w;
                    t.offset[l][i][1 + j] = off;
                }
            }
        }
    }
    Ok(t)
}

/// Parses a slice segment header. `prev` is the previous independent slice segment
/// header of the same picture (required for dependent slice segments).
pub fn parse_slice_header(
    rbsp: &[u8],
    nal: &NalHeader,
    sps: &Sps,
    pps: &Pps,
    prev: Option<&SliceHeader>,
) -> Result<SliceHeader> {
    let mut r = BitReader::new(rbsp);
    r.skip(16)?;
    let first_slice_segment_in_pic = r.flag()?;
    let mut no_output_of_prior_pics = false;
    if nal.is_irap() {
        no_output_of_prior_pics = r.flag()?;
    }
    let pps_id = r.ue_max(63, "slice_pic_parameter_set_id")? as u8;
    let mut dependent = false;
    let mut segment_address = 0usize;
    let pic_size = sps.pic_size_in_ctbs();
    if !first_slice_segment_in_pic {
        if pps.dependent_slice_segments_enabled {
            dependent = r.flag()?;
        }
        segment_address = r.u(ceil_log2(pic_size))? as usize;
        if segment_address >= pic_size {
            return Err(Error::Invalid("slice_segment_address"));
        }
    }
    let mut sh = if dependent {
        let p = prev.ok_or(Error::Invalid("dependent slice without preceding slice"))?;
        let mut s = p.clone();
        s.first_slice_segment_in_pic = false;
        s.dependent_slice_segment = true;
        s.segment_address = segment_address;
        s.entry_point_offsets.clear();
        s
    } else {
        SliceHeader {
            first_slice_segment_in_pic,
            no_output_of_prior_pics,
            pps_id,
            dependent_slice_segment: false,
            segment_address,
            slice_addr_rs: segment_address,
            slice_type: SliceType::I,
            pic_output_flag: true,
            poc_lsb: 0,
            st_rps: StRps::default(),
            st_rps_bits: 0,
            num_long_term: 0,
            lt_poc_lsb: Vec::new(),
            lt_used_by_curr: Vec::new(),
            lt_delta_poc_msb_present: Vec::new(),
            lt_delta_poc_msb_cycle: Vec::new(),
            temporal_mvp_enabled: false,
            sao_luma: false,
            sao_chroma: false,
            num_ref_idx_active: [0, 0],
            rpl_modification: [false, false],
            list_entry: [[0; 16]; 2],
            mvd_l1_zero: false,
            cabac_init_flag: false,
            collocated_from_l0: true,
            collocated_ref_idx: 0,
            pwt: None,
            max_num_merge_cand: 5,
            slice_qp: 26,
            cb_qp_offset: 0,
            cr_qp_offset: 0,
            deblocking_filter_disabled: pps.pps_deblocking_filter_disabled,
            beta_offset_div2: pps.beta_offset_div2,
            tc_offset_div2: pps.tc_offset_div2,
            loop_filter_across_slices_enabled: pps.loop_filter_across_slices_enabled,
            entry_point_offsets: Vec::new(),
            data_offset: 0,
        }
    };

    if !dependent {
        r.skip(pps.num_extra_slice_header_bits as usize)?;
        let st = r.ue_max(2, "slice_type")?;
        sh.slice_type = match st {
            0 => SliceType::B,
            1 => SliceType::P,
            _ => SliceType::I,
        };
        if nal.is_irap() && sh.slice_type != SliceType::I {
            return Err(Error::Invalid("IRAP picture with non-I slice"));
        }
        if pps.output_flag_present {
            sh.pic_output_flag = r.flag()?;
        }
        if !nal.is_idr() {
            sh.poc_lsb = r.u(sps.log2_max_poc_lsb as u32)? as i32;
            let st_sps_flag = r.flag()?;
            if !st_sps_flag {
                let before = r.bit_pos();
                sh.st_rps = parse_st_rps(
                    &mut r,
                    sps.st_rps.len(),
                    sps.st_rps.len(),
                    &sps.st_rps,
                    sps.max_dec_pic_buffering[sps.max_sub_layers_minus1] as usize,
                )?;
                sh.st_rps_bits = r.bit_pos() - before;
            } else {
                if sps.st_rps.is_empty() {
                    return Err(Error::Invalid("no SPS short-term RPS"));
                }
                let idx = r.u(ceil_log2(sps.st_rps.len()))? as usize;
                sh.st_rps = sps.st_rps.get(idx).cloned().ok_or(Error::Invalid("short_term_ref_pic_set_idx"))?;
            }
            if sps.long_term_refs_present {
                let mut num_lt_sps = 0usize;
                let n_sps = sps.lt_ref_pic_poc_lsb_sps.len();
                if n_sps > 0 {
                    num_lt_sps = r.ue_max(n_sps as u32, "num_long_term_sps")? as usize;
                }
                let num_lt_pics = r.ue_max(32, "num_long_term_pics")? as usize;
                let total = num_lt_sps + num_lt_pics;
                if total + sh.st_rps.num_delta_pocs() > 32 {
                    return Err(Error::Invalid("too many long-term pictures"));
                }
                sh.num_long_term = total;
                for i in 0..total {
                    if i < num_lt_sps {
                        let mut idx = 0usize;
                        if n_sps > 1 {
                            idx = r.u(ceil_log2(n_sps))? as usize;
                        }
                        if idx >= n_sps {
                            return Err(Error::Invalid("lt_idx_sps"));
                        }
                        sh.lt_poc_lsb.push(sps.lt_ref_pic_poc_lsb_sps[idx] as i32);
                        sh.lt_used_by_curr.push(sps.used_by_curr_pic_lt_sps[idx]);
                    } else {
                        sh.lt_poc_lsb.push(r.u(sps.log2_max_poc_lsb as u32)? as i32);
                        sh.lt_used_by_curr.push(r.flag()?);
                    }
                    let present = r.flag()?;
                    sh.lt_delta_poc_msb_present.push(present);
                    let mut cycle = 0i32;
                    if present {
                        cycle = r.ue_max(1 << 24, "delta_poc_msb_cycle_lt")? as i32;
                    }
                    // (7-52)
                    if i != 0 && i != num_lt_sps {
                        cycle += sh.lt_delta_poc_msb_cycle[i - 1];
                    }
                    sh.lt_delta_poc_msb_cycle.push(cycle);
                }
            }
            if sps.temporal_mvp_enabled {
                sh.temporal_mvp_enabled = r.flag()?;
            }
        }
        if sps.sao_enabled {
            sh.sao_luma = r.flag()?;
            if sps.chroma_format_idc != 0 {
                sh.sao_chroma = r.flag()?;
            }
        }
        if sh.slice_type != SliceType::I {
            sh.num_ref_idx_active = [
                pps.num_ref_idx_l0_default_active as usize,
                if sh.is_b() { pps.num_ref_idx_l1_default_active as usize } else { 0 },
            ];
            if r.flag()? {
                sh.num_ref_idx_active[0] = r.ue_max(14, "num_ref_idx_l0_active_minus1")? as usize + 1;
                if sh.is_b() {
                    sh.num_ref_idx_active[1] = r.ue_max(14, "num_ref_idx_l1_active_minus1")? as usize + 1;
                }
            }
            let total_curr = sh.num_pic_total_curr();
            if total_curr == 0 {
                return Err(Error::Invalid("inter slice without reference pictures"));
            }
            if pps.lists_modification_present && total_curr > 1 {
                let bits = ceil_log2(total_curr);
                let lists = if sh.is_b() { 2 } else { 1 };
                for l in 0..lists {
                    sh.rpl_modification[l] = r.flag()?;
                    if sh.rpl_modification[l] {
                        for i in 0..sh.num_ref_idx_active[l] {
                            let e = r.u(bits)? as usize;
                            if e >= total_curr {
                                return Err(Error::Invalid("list_entry"));
                            }
                            sh.list_entry[l][i] = e as u8;
                        }
                    }
                }
            }
            if sh.is_b() {
                sh.mvd_l1_zero = r.flag()?;
            }
            if pps.cabac_init_present {
                sh.cabac_init_flag = r.flag()?;
            }
            if sh.temporal_mvp_enabled {
                if sh.is_b() {
                    sh.collocated_from_l0 = r.flag()?;
                }
                let l = if sh.collocated_from_l0 { 0 } else { 1 };
                if sh.num_ref_idx_active[l] > 1 {
                    sh.collocated_ref_idx = r.ue()? as usize;
                    if sh.collocated_ref_idx >= sh.num_ref_idx_active[l] {
                        return Err(Error::Invalid("collocated_ref_idx"));
                    }
                }
            }
            if (pps.weighted_pred && sh.slice_type == SliceType::P) || (pps.weighted_bipred && sh.is_b()) {
                sh.pwt = Some(parse_pred_weight_table(&mut r, sps, &sh)?);
            }
            let five_minus = r.ue_max(4, "five_minus_max_num_merge_cand")? as usize;
            sh.max_num_merge_cand = 5 - five_minus;
        }
        let qp_delta = r.se()?;
        sh.slice_qp = pps.init_qp + qp_delta;
        if sh.slice_qp < -sps.qp_bd_offset_y() || sh.slice_qp > 51 {
            return Err(Error::Invalid("slice_qp_delta"));
        }
        if pps.slice_chroma_qp_offsets_present {
            sh.cb_qp_offset = r.se_range(-12, 12, "slice_cb_qp_offset")?;
            sh.cr_qp_offset = r.se_range(-12, 12, "slice_cr_qp_offset")?;
        }
        let mut override_flag = false;
        if pps.deblocking_filter_override_enabled {
            override_flag = r.flag()?;
        }
        if override_flag {
            sh.deblocking_filter_disabled = r.flag()?;
            if !sh.deblocking_filter_disabled {
                sh.beta_offset_div2 = r.se_range(-6, 6, "slice_beta_offset_div2")?;
                sh.tc_offset_div2 = r.se_range(-6, 6, "slice_tc_offset_div2")?;
            }
        }
        if pps.loop_filter_across_slices_enabled
            && (sh.sao_luma || sh.sao_chroma || !sh.deblocking_filter_disabled)
        {
            sh.loop_filter_across_slices_enabled = r.flag()?;
        }
    }
    if pps.tiles_enabled || pps.entropy_coding_sync_enabled {
        let n = r.ue_max(pic_size as u32, "num_entry_point_offsets")? as usize;
        if n > 0 {
            let len = r.ue_max(31, "offset_len_minus1")? + 1;
            for _ in 0..n {
                sh.entry_point_offsets.push(r.u(len)? + 1);
            }
        }
    }
    if pps.slice_segment_header_extension_present {
        let len = r.ue_max(256, "slice_segment_header_extension_length")? as usize;
        r.skip(len * 8)?;
    }
    // byte_alignment(): one bit equal to 1 then zeros
    if !r.u1()? {
        return Err(Error::Invalid("slice header alignment bit"));
    }
    r.byte_align();
    sh.data_offset = r.bit_pos() / 8;
    if sh.data_offset > rbsp.len() {
        return Err(Error::UnexpectedEof);
    }
    Ok(sh)
}
