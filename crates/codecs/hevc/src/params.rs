// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parameter sets: VPS, SPS (with VUI, scaling lists, short-term RPS) and PPS
//! (with tiles), as specified in ITU-T H.265 clause 7.3.2.

use crate::bitreader::BitReader;
use crate::error::{Error, Result};

pub const MAX_SUB_LAYERS: usize = 7;

// ---------------------------------------------------------------------------
// Profile / tier / level
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ProfileTierLevel {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility: u32,
    pub general_level_idc: u8,
}

fn parse_profile_tier_level(
    r: &mut BitReader,
    profile_present: bool,
    max_sub_layers_minus1: usize,
) -> Result<ProfileTierLevel> {
    let mut ptl = ProfileTierLevel::default();
    if profile_present {
        ptl.general_profile_space = r.u(2)? as u8;
        ptl.general_tier_flag = r.flag()?;
        ptl.general_profile_idc = r.u(5)? as u8;
        ptl.general_profile_compatibility = r.u(32)?;
        // progressive, interlaced, non_packed, frame_only + 43 bits + 1 bit
        r.skip(4 + 43 + 1)?;
    }
    ptl.general_level_idc = r.u(8)? as u8;
    let mut sub_profile_present = [false; 8];
    let mut sub_level_present = [false; 8];
    for i in 0..max_sub_layers_minus1 {
        sub_profile_present[i] = r.flag()?;
        sub_level_present[i] = r.flag()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            r.skip(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 {
        if sub_profile_present[i] {
            r.skip(88)?;
        }
        if sub_level_present[i] {
            r.skip(8)?;
        }
    }
    Ok(ptl)
}

// ---------------------------------------------------------------------------
// VPS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Vps {
    pub id: u8,
    pub max_sub_layers_minus1: u8,
}

pub fn parse_vps(rbsp: &[u8]) -> Result<Vps> {
    let mut r = BitReader::new(rbsp);
    r.skip(16)?; // NAL header
    let id = r.u(4)? as u8;
    r.skip(2)?; // base layer internal / available
    r.skip(6)?; // max_layers_minus1
    let max_sub_layers_minus1 = r.u(3)? as u8;
    // The rest is not needed for decoding the base layer.
    Ok(Vps { id, max_sub_layers_minus1 })
}

// ---------------------------------------------------------------------------
// Scaling lists
// ---------------------------------------------------------------------------

const DEFAULT_INTRA_8X8: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18, 17, 18, 18, 17, 18, 21, 19, 20,
    21, 20, 19, 21, 24, 22, 22, 24, 24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35, 35, 31,
    29, 36, 41, 44, 41, 36, 47, 54, 54, 47, 65, 70, 65, 88, 88, 115,
];
const DEFAULT_INTER_8X8: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 18, 18, 18, 18, 18, 18, 20, 20, 20,
    20, 20, 20, 20, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 28, 28, 28, 28, 28,
    28, 33, 33, 33, 33, 33, 41, 41, 41, 41, 54, 54, 54, 71, 71, 91,
];

/// Up-right diagonal scan of a square block of size `blk`: returns (x, y) pairs.
pub fn diag_scan(blk: usize) -> Vec<(u8, u8)> {
    let mut out = Vec::with_capacity(blk * blk);
    let (mut x, mut y): (isize, isize) = (0, 0);
    loop {
        while y >= 0 {
            if (x as usize) < blk && (y as usize) < blk {
                out.push((x as u8, y as u8));
            }
            y -= 1;
            x += 1;
        }
        y = x;
        x = 0;
        if out.len() >= blk * blk {
            break;
        }
    }
    out
}

/// Scaling list data in coded (diagonal scan) order plus DC values.
#[derive(Debug, Clone)]
pub struct ScalingList {
    /// [sizeId][matrixId][i], coefficient i in diagonal scan order (16 or 64 entries used).
    pub list: [[[u8; 64]; 6]; 4],
    /// DC coefficients for sizeId 2 and 3.
    pub dc: [[u8; 6]; 4],
}

impl ScalingList {
    pub fn default_lists() -> ScalingList {
        let mut sl = ScalingList { list: [[[16u8; 64]; 6]; 4], dc: [[16u8; 6]; 4] };
        for size_id in 1..4 {
            for m in 0..6 {
                sl.list[size_id][m] = if m < 3 { DEFAULT_INTRA_8X8 } else { DEFAULT_INTER_8X8 };
            }
        }
        sl
    }

    fn default_for(size_id: usize, matrix_id: usize) -> [u8; 64] {
        if size_id == 0 {
            [16u8; 64]
        } else if matrix_id < 3 {
            DEFAULT_INTRA_8X8
        } else {
            DEFAULT_INTER_8X8
        }
    }

    pub fn parse(r: &mut BitReader) -> Result<ScalingList> {
        let mut sl = ScalingList::default_lists();
        for size_id in 0..4usize {
            let step = if size_id == 3 { 3 } else { 1 };
            let mut matrix_id = 0usize;
            while matrix_id < 6 {
                let pred_mode_flag = r.flag()?;
                let coef_num = 64.min(1 << (4 + (size_id << 1)));
                if !pred_mode_flag {
                    let delta = r.ue()? as usize;
                    if delta * step > matrix_id {
                        return Err(Error::Invalid("scaling_list_pred_matrix_id_delta"));
                    }
                    if delta == 0 {
                        sl.list[size_id][matrix_id] = Self::default_for(size_id, matrix_id);
                        sl.dc[size_id][matrix_id] = 16;
                    } else {
                        let ref_id = matrix_id - delta * step;
                        sl.list[size_id][matrix_id] = sl.list[size_id][ref_id];
                        sl.dc[size_id][matrix_id] = sl.dc[size_id][ref_id];
                    }
                } else {
                    let mut next_coef: i32 = 8;
                    if size_id > 1 {
                        let dc = r.se_range(-7, 247, "scaling_list_dc_coef_minus8")?;
                        next_coef = dc + 8;
                        sl.dc[size_id][matrix_id] = next_coef as u8;
                    }
                    for i in 0..coef_num {
                        let delta = r.se_range(-128, 127, "scaling_list_delta_coef")?;
                        next_coef = (next_coef + delta + 256).rem_euclid(256);
                        if next_coef == 0 {
                            return Err(Error::Invalid("scaling list coefficient zero"));
                        }
                        sl.list[size_id][matrix_id][i] = next_coef as u8;
                    }
                }
                matrix_id += step;
            }
        }
        // 32x32 chroma lists (only used for 4:4:4) derive from the 16x16 ones.
        for m in [1usize, 2, 4, 5] {
            sl.list[3][m] = sl.list[2][m];
            sl.dc[3][m] = sl.dc[2][m];
        }
        Ok(sl)
    }

    /// Computes the ScalingFactor arrays (raster order, index y * size + x).
    pub fn factors(&self) -> ScalingFactors {
        let d4 = diag_scan(4);
        let d8 = diag_scan(8);
        let mut f = ScalingFactors {
            f: [
                vec![vec![0u8; 16]; 6],
                vec![vec![0u8; 64]; 6],
                vec![vec![0u8; 256]; 6],
                vec![vec![0u8; 1024]; 6],
            ],
        };
        for m in 0..6 {
            for (i, &(x, y)) in d4.iter().enumerate() {
                f.f[0][m][y as usize * 4 + x as usize] = self.list[0][m][i];
            }
            for (i, &(x, y)) in d8.iter().enumerate() {
                let (x, y) = (x as usize, y as usize);
                f.f[1][m][y * 8 + x] = self.list[1][m][i];
                for k in 0..2 {
                    for j in 0..2 {
                        f.f[2][m][(y * 2 + j) * 16 + x * 2 + k] = self.list[2][m][i];
                    }
                }
                for k in 0..4 {
                    for j in 0..4 {
                        f.f[3][m][(y * 4 + j) * 32 + x * 4 + k] = self.list[3][m][i];
                    }
                }
            }
            f.f[2][m][0] = self.dc[2][m];
            f.f[3][m][0] = self.dc[3][m];
        }
        f
    }
}

/// Expanded scaling factors: f[sizeId][matrixId][y * size + x].
#[derive(Debug, Clone)]
pub struct ScalingFactors {
    pub f: [Vec<Vec<u8>>; 4],
}

// ---------------------------------------------------------------------------
// Short-term reference picture sets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct StRps {
    pub num_negative: usize,
    pub num_positive: usize,
    pub delta_poc_s0: [i32; 16],
    pub used_s0: [bool; 16],
    pub delta_poc_s1: [i32; 16],
    pub used_s1: [bool; 16],
}

impl StRps {
    pub fn num_delta_pocs(&self) -> usize {
        self.num_negative + self.num_positive
    }
}

/// Parses st_ref_pic_set(stRpsIdx). `sets` holds the already-parsed SPS sets.
pub fn parse_st_rps(
    r: &mut BitReader,
    idx: usize,
    num_sets_in_sps: usize,
    sets: &[StRps],
    max_dec_pic_buffering: usize,
) -> Result<StRps> {
    let mut rps = StRps::default();
    let inter = if idx != 0 { r.flag()? } else { false };
    if inter {
        let mut delta_idx_minus1 = 0usize;
        if idx == num_sets_in_sps {
            delta_idx_minus1 = r.ue()? as usize;
        }
        if delta_idx_minus1 + 1 > idx {
            return Err(Error::Invalid("delta_idx_minus1"));
        }
        let ref_idx = idx - (delta_idx_minus1 + 1);
        let refr = sets.get(ref_idx).ok_or(Error::Invalid("st rps reference"))?;
        let sign = r.flag()?;
        let abs = r.ue_max(32767, "abs_delta_rps_minus1")? as i32 + 1;
        let delta_rps = if sign { -abs } else { abs };
        let n = refr.num_delta_pocs();
        let mut used = [false; 33];
        let mut use_delta = [true; 33];
        for j in 0..=n {
            used[j] = r.flag()?;
            if !used[j] {
                use_delta[j] = r.flag()?;
            }
        }
        // (7-61)
        let mut i = 0usize;
        for j in (0..refr.num_positive).rev() {
            let d = refr.delta_poc_s1[j] + delta_rps;
            if d < 0 && use_delta[refr.num_negative + j] {
                if i >= 16 {
                    return Err(Error::Invalid("st rps too large"));
                }
                rps.delta_poc_s0[i] = d;
                rps.used_s0[i] = used[refr.num_negative + j];
                i += 1;
            }
        }
        if delta_rps < 0 && use_delta[n] {
            if i >= 16 {
                return Err(Error::Invalid("st rps too large"));
            }
            rps.delta_poc_s0[i] = delta_rps;
            rps.used_s0[i] = used[n];
            i += 1;
        }
        for j in 0..refr.num_negative {
            let d = refr.delta_poc_s0[j] + delta_rps;
            if d < 0 && use_delta[j] {
                if i >= 16 {
                    return Err(Error::Invalid("st rps too large"));
                }
                rps.delta_poc_s0[i] = d;
                rps.used_s0[i] = used[j];
                i += 1;
            }
        }
        rps.num_negative = i;
        // (7-62)
        i = 0;
        for j in (0..refr.num_negative).rev() {
            let d = refr.delta_poc_s0[j] + delta_rps;
            if d > 0 && use_delta[j] {
                if i >= 16 {
                    return Err(Error::Invalid("st rps too large"));
                }
                rps.delta_poc_s1[i] = d;
                rps.used_s1[i] = used[j];
                i += 1;
            }
        }
        if delta_rps > 0 && use_delta[n] {
            if i >= 16 {
                return Err(Error::Invalid("st rps too large"));
            }
            rps.delta_poc_s1[i] = delta_rps;
            rps.used_s1[i] = used[n];
            i += 1;
        }
        for j in 0..refr.num_positive {
            let d = refr.delta_poc_s1[j] + delta_rps;
            if d > 0 && use_delta[refr.num_negative + j] {
                if i >= 16 {
                    return Err(Error::Invalid("st rps too large"));
                }
                rps.delta_poc_s1[i] = d;
                rps.used_s1[i] = used[refr.num_negative + j];
                i += 1;
            }
        }
        rps.num_positive = i;
    } else {
        let max = max_dec_pic_buffering.clamp(1, 16) as u32;
        rps.num_negative = r.ue_max(max, "num_negative_pics")? as usize;
        rps.num_positive = r.ue_max(max, "num_positive_pics")? as usize;
        if rps.num_negative + rps.num_positive > 16 {
            return Err(Error::Invalid("st rps too large"));
        }
        let mut prev = 0i32;
        for i in 0..rps.num_negative {
            let d = r.ue_max(32767, "delta_poc_s0_minus1")? as i32 + 1;
            prev -= d;
            rps.delta_poc_s0[i] = prev;
            rps.used_s0[i] = r.flag()?;
        }
        prev = 0;
        for i in 0..rps.num_positive {
            let d = r.ue_max(32767, "delta_poc_s1_minus1")? as i32 + 1;
            prev += d;
            rps.delta_poc_s1[i] = prev;
            rps.used_s1[i] = r.flag()?;
        }
    }
    Ok(rps)
}

// ---------------------------------------------------------------------------
// VUI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Vui {
    pub video_full_range_flag: bool,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coeffs: u8,
    pub sar: (u16, u16),
    pub timing: Option<(u32, u32)>,
}

impl Default for Vui {
    fn default() -> Self {
        Vui {
            video_full_range_flag: false,
            colour_primaries: 2,
            transfer_characteristics: 2,
            matrix_coeffs: 2,
            sar: (1, 1),
            timing: None,
        }
    }
}

const SAR_TABLE: [(u16, u16); 17] = [
    (0, 0),
    (1, 1),
    (12, 11),
    (10, 11),
    (16, 11),
    (40, 33),
    (24, 11),
    (20, 11),
    (32, 11),
    (80, 33),
    (18, 11),
    (15, 11),
    (64, 33),
    (160, 99),
    (4, 3),
    (3, 2),
    (2, 1),
];

fn parse_hrd(r: &mut BitReader, common: bool, max_sub_layers_minus1: usize) -> Result<()> {
    let mut nal = false;
    let mut vcl = false;
    let mut sub_pic = false;
    if common {
        nal = r.flag()?;
        vcl = r.flag()?;
        if nal || vcl {
            sub_pic = r.flag()?;
            if sub_pic {
                r.skip(8 + 5 + 1 + 5)?;
            }
            r.skip(4 + 4)?;
            if sub_pic {
                r.skip(4)?;
            }
            r.skip(5 + 5 + 5)?;
        }
    }
    for _ in 0..=max_sub_layers_minus1 {
        let fixed_general = r.flag()?;
        let mut fixed_within = true;
        if !fixed_general {
            fixed_within = r.flag()?;
        }
        let mut low_delay = false;
        if fixed_within {
            r.ue()?;
        } else {
            low_delay = r.flag()?;
        }
        let mut cpb_cnt = 1usize;
        if !low_delay {
            cpb_cnt = r.ue_max(31, "cpb_cnt_minus1")? as usize + 1;
        }
        for present in [nal, vcl] {
            if present {
                for _ in 0..cpb_cnt {
                    r.ue()?;
                    r.ue()?;
                    if sub_pic {
                        r.ue()?;
                        r.ue()?;
                    }
                    r.skip(1)?;
                }
            }
        }
    }
    Ok(())
}

fn parse_vui(r: &mut BitReader, max_sub_layers_minus1: usize) -> Result<Vui> {
    let mut v = Vui::default();
    if r.flag()? {
        let idc = r.u(8)? as usize;
        if idc == 255 {
            let w = r.u(16)? as u16;
            let h = r.u(16)? as u16;
            v.sar = (w, h);
        } else if idc < SAR_TABLE.len() {
            v.sar = SAR_TABLE[idc];
        }
    }
    if r.flag()? {
        r.skip(1)?; // overscan_appropriate_flag
    }
    if r.flag()? {
        r.skip(3)?; // video_format
        v.video_full_range_flag = r.flag()?;
        if r.flag()? {
            v.colour_primaries = r.u(8)? as u8;
            v.transfer_characteristics = r.u(8)? as u8;
            v.matrix_coeffs = r.u(8)? as u8;
        }
    }
    if r.flag()? {
        r.ue()?;
        r.ue()?;
    }
    r.skip(3)?; // neutral_chroma, field_seq, frame_field_info_present
    if r.flag()? {
        // default display window
        r.ue()?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
    }
    if r.flag()? {
        let num_units = r.u(32)?;
        let time_scale = r.u(32)?;
        v.timing = Some((num_units, time_scale));
        if r.flag()? {
            r.ue()?;
        }
        if r.flag()? {
            parse_hrd(r, true, max_sub_layers_minus1)?;
        }
    }
    if r.flag()? {
        r.skip(3)?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
    }
    Ok(v)
}

// ---------------------------------------------------------------------------
// SPS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Sps {
    pub id: u8,
    pub vps_id: u8,
    pub max_sub_layers_minus1: usize,
    pub ptl: ProfileTierLevel,
    pub chroma_format_idc: u8,
    pub pic_width: usize,
    pub pic_height: usize,
    pub conf_win: [usize; 4], // left, right, top, bottom in luma samples
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub log2_max_poc_lsb: u8,
    pub max_dec_pic_buffering: [u32; MAX_SUB_LAYERS],
    pub max_num_reorder: [u32; MAX_SUB_LAYERS],
    pub max_latency_increase_plus1: [u32; MAX_SUB_LAYERS],
    pub log2_min_cb: u8,
    pub log2_ctb: u8,
    pub log2_min_tb: u8,
    pub log2_max_tb: u8,
    pub max_transform_hierarchy_depth_inter: u8,
    pub max_transform_hierarchy_depth_intra: u8,
    pub scaling_list_enabled: bool,
    pub scaling_list: Option<ScalingList>, // None => default lists
    pub amp_enabled: bool,
    pub sao_enabled: bool,
    pub pcm_enabled: bool,
    pub pcm_bit_depth_luma: u8,
    pub pcm_bit_depth_chroma: u8,
    pub log2_min_pcm_cb: u8,
    pub log2_max_pcm_cb: u8,
    pub pcm_loop_filter_disabled: bool,
    pub st_rps: Vec<StRps>,
    pub long_term_refs_present: bool,
    pub lt_ref_pic_poc_lsb_sps: Vec<u32>,
    pub used_by_curr_pic_lt_sps: Vec<bool>,
    pub temporal_mvp_enabled: bool,
    pub strong_intra_smoothing_enabled: bool,
    pub vui: Vui,
    // derived
    pub ctb_size: usize,
    pub pic_width_in_ctbs: usize,
    pub pic_height_in_ctbs: usize,
    pub min_cb_size: usize,
}

impl Sps {
    pub fn pic_size_in_ctbs(&self) -> usize {
        self.pic_width_in_ctbs * self.pic_height_in_ctbs
    }
    pub fn qp_bd_offset_y(&self) -> i32 {
        6 * (self.bit_depth_luma as i32 - 8)
    }
    pub fn qp_bd_offset_c(&self) -> i32 {
        6 * (self.bit_depth_chroma as i32 - 8)
    }
    pub fn max_poc_lsb(&self) -> i32 {
        1 << self.log2_max_poc_lsb
    }
}

pub fn parse_sps(rbsp: &[u8]) -> Result<Sps> {
    let mut r = BitReader::new(rbsp);
    r.skip(16)?; // NAL header
    let vps_id = r.u(4)? as u8;
    let max_sub_layers_minus1 = r.u(3)? as usize;
    if max_sub_layers_minus1 >= MAX_SUB_LAYERS {
        return Err(Error::Invalid("sps_max_sub_layers_minus1"));
    }
    r.skip(1)?; // temporal_id_nesting
    let ptl = parse_profile_tier_level(&mut r, true, max_sub_layers_minus1)?;
    let id = r.ue_max(15, "sps_seq_parameter_set_id")? as u8;
    let chroma_format_idc = r.ue_max(3, "chroma_format_idc")? as u8;
    if chroma_format_idc == 3 && r.flag()? {
        return Err(Error::Unsupported("separate_colour_plane_flag"));
    }
    let pic_width = r.ue()? as usize;
    let pic_height = r.ue()? as usize;
    if pic_width == 0 || pic_height == 0 || pic_width > 16888 || pic_height > 16888 {
        return Err(Error::Invalid("picture size"));
    }
    let mut conf_win = [0usize; 4];
    if r.flag()? {
        let (sub_w, sub_h) = match chroma_format_idc {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        conf_win[0] = r.ue()? as usize * sub_w;
        conf_win[1] = r.ue()? as usize * sub_w;
        conf_win[2] = r.ue()? as usize * sub_h;
        conf_win[3] = r.ue()? as usize * sub_h;
        if conf_win[0] + conf_win[1] >= pic_width || conf_win[2] + conf_win[3] >= pic_height {
            return Err(Error::Invalid("conformance window"));
        }
    }
    let bit_depth_luma = r.ue_max(8, "bit_depth_luma_minus8")? as u8 + 8;
    let bit_depth_chroma = r.ue_max(8, "bit_depth_chroma_minus8")? as u8 + 8;
    let log2_max_poc_lsb = r.ue_max(12, "log2_max_pic_order_cnt_lsb_minus4")? as u8 + 4;
    let sub_layer_ordering_info_present = r.flag()?;
    let mut max_dec_pic_buffering = [0u32; MAX_SUB_LAYERS];
    let mut max_num_reorder = [0u32; MAX_SUB_LAYERS];
    let mut max_latency_increase_plus1 = [0u32; MAX_SUB_LAYERS];
    let start = if sub_layer_ordering_info_present { 0 } else { max_sub_layers_minus1 };
    for i in start..=max_sub_layers_minus1 {
        max_dec_pic_buffering[i] = r.ue_max(15, "sps_max_dec_pic_buffering_minus1")? + 1;
        max_num_reorder[i] = r.ue_max(15, "sps_max_num_reorder_pics")?;
        max_latency_increase_plus1[i] = r.ue()?;
    }
    if !sub_layer_ordering_info_present {
        for i in 0..max_sub_layers_minus1 {
            max_dec_pic_buffering[i] = max_dec_pic_buffering[max_sub_layers_minus1];
            max_num_reorder[i] = max_num_reorder[max_sub_layers_minus1];
            max_latency_increase_plus1[i] = max_latency_increase_plus1[max_sub_layers_minus1];
        }
    }
    let log2_min_cb = r.ue_max(3, "log2_min_luma_coding_block_size_minus3")? as u8 + 3;
    let log2_ctb = log2_min_cb + r.ue_max(3, "log2_diff_max_min_luma_coding_block_size")? as u8;
    let log2_min_tb = r.ue_max(3, "log2_min_luma_transform_block_size_minus2")? as u8 + 2;
    let log2_max_tb = log2_min_tb + r.ue_max(3, "log2_diff_max_min_luma_transform_block_size")? as u8;
    if !(4..=6).contains(&log2_ctb) {
        return Err(Error::Invalid("CTB size"));
    }
    if log2_min_tb >= log2_min_cb || log2_max_tb > 5 || log2_max_tb > log2_ctb {
        return Err(Error::Invalid("transform block sizes"));
    }
    let max_transform_hierarchy_depth_inter = r.ue_max(4, "max_transform_hierarchy_depth_inter")? as u8;
    let max_transform_hierarchy_depth_intra = r.ue_max(4, "max_transform_hierarchy_depth_intra")? as u8;
    let scaling_list_enabled = r.flag()?;
    let mut scaling_list = None;
    if scaling_list_enabled && r.flag()? {
        scaling_list = Some(ScalingList::parse(&mut r)?);
    }
    let amp_enabled = r.flag()?;
    let sao_enabled = r.flag()?;
    let pcm_enabled = r.flag()?;
    let (mut pcm_bit_depth_luma, mut pcm_bit_depth_chroma) = (8, 8);
    let (mut log2_min_pcm_cb, mut log2_max_pcm_cb) = (0, 0);
    let mut pcm_loop_filter_disabled = false;
    if pcm_enabled {
        pcm_bit_depth_luma = r.u(4)? as u8 + 1;
        pcm_bit_depth_chroma = r.u(4)? as u8 + 1;
        log2_min_pcm_cb = r.ue_max(2, "log2_min_pcm_luma_coding_block_size_minus3")? as u8 + 3;
        log2_max_pcm_cb = log2_min_pcm_cb + r.ue_max(2, "log2_diff_max_min_pcm")? as u8;
        pcm_loop_filter_disabled = r.flag()?;
        if pcm_bit_depth_luma > bit_depth_luma || pcm_bit_depth_chroma > bit_depth_chroma {
            return Err(Error::Invalid("pcm bit depth"));
        }
        if log2_max_pcm_cb > 5.min(log2_ctb) {
            return Err(Error::Invalid("pcm sizes"));
        }
    }
    let num_st_rps = r.ue_max(64, "num_short_term_ref_pic_sets")? as usize;
    let mut st_rps: Vec<StRps> = Vec::with_capacity(num_st_rps);
    let max_dpb = max_dec_pic_buffering[max_sub_layers_minus1] as usize;
    for i in 0..num_st_rps {
        let s = parse_st_rps(&mut r, i, num_st_rps, &st_rps, max_dpb)?;
        st_rps.push(s);
    }
    let long_term_refs_present = r.flag()?;
    let mut lt_ref_pic_poc_lsb_sps = Vec::new();
    let mut used_by_curr_pic_lt_sps = Vec::new();
    if long_term_refs_present {
        let n = r.ue_max(32, "num_long_term_ref_pics_sps")?;
        for _ in 0..n {
            lt_ref_pic_poc_lsb_sps.push(r.u(log2_max_poc_lsb as u32)?);
            used_by_curr_pic_lt_sps.push(r.flag()?);
        }
    }
    let temporal_mvp_enabled = r.flag()?;
    let strong_intra_smoothing_enabled = r.flag()?;
    let mut vui = Vui::default();
    let mut vui_ok = true;
    if r.flag()? {
        // A malformed VUI is not fatal: keep defaults (and skip the extension flags).
        let mut rr = r.clone();
        match parse_vui(&mut rr, max_sub_layers_minus1) {
            Ok(v) => {
                vui = v;
                r = rr;
            }
            Err(_) => vui_ok = false,
        }
    }
    if vui_ok && r.flag().unwrap_or(false) {
        // sps_extension_present_flag
        let range_ext = r.flag()?;
        r.skip(7)?; // multilayer, 3d, scc, 4bits
        if range_ext {
            // transform_skip_rotation, transform_skip_context, implicit_rdpcm,
            // explicit_rdpcm, extended_precision, intra_smoothing_disabled,
            // high_precision_offsets, persistent_rice_adaptation, cabac_bypass_alignment
            let flags = r.u(9)?;
            if flags != 0 {
                return Err(Error::Unsupported("SPS range extension tools"));
            }
        }
    }
    let ctb_size = 1usize << log2_ctb;
    let min_cb_size = 1usize << log2_min_cb;
    if pic_width % min_cb_size != 0 || pic_height % min_cb_size != 0 {
        return Err(Error::Invalid("picture size not a multiple of MinCbSizeY"));
    }
    Ok(Sps {
        id,
        vps_id,
        max_sub_layers_minus1,
        ptl,
        chroma_format_idc,
        pic_width,
        pic_height,
        conf_win,
        bit_depth_luma,
        bit_depth_chroma,
        log2_max_poc_lsb,
        max_dec_pic_buffering,
        max_num_reorder,
        max_latency_increase_plus1,
        log2_min_cb,
        log2_ctb,
        log2_min_tb,
        log2_max_tb,
        max_transform_hierarchy_depth_inter,
        max_transform_hierarchy_depth_intra,
        scaling_list_enabled,
        scaling_list,
        amp_enabled,
        sao_enabled,
        pcm_enabled,
        pcm_bit_depth_luma,
        pcm_bit_depth_chroma,
        log2_min_pcm_cb,
        log2_max_pcm_cb,
        pcm_loop_filter_disabled,
        st_rps,
        long_term_refs_present,
        lt_ref_pic_poc_lsb_sps,
        used_by_curr_pic_lt_sps,
        temporal_mvp_enabled,
        strong_intra_smoothing_enabled,
        vui,
        ctb_size,
        pic_width_in_ctbs: pic_width.div_ceil(ctb_size),
        pic_height_in_ctbs: pic_height.div_ceil(ctb_size),
        min_cb_size,
    })
}

// ---------------------------------------------------------------------------
// PPS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Pps {
    pub id: u8,
    pub sps_id: u8,
    pub dependent_slice_segments_enabled: bool,
    pub output_flag_present: bool,
    pub num_extra_slice_header_bits: u8,
    pub sign_data_hiding_enabled: bool,
    pub cabac_init_present: bool,
    pub num_ref_idx_l0_default_active: u8,
    pub num_ref_idx_l1_default_active: u8,
    pub init_qp: i32,
    pub constrained_intra_pred: bool,
    pub transform_skip_enabled: bool,
    pub cu_qp_delta_enabled: bool,
    pub diff_cu_qp_delta_depth: u8,
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub slice_chroma_qp_offsets_present: bool,
    pub weighted_pred: bool,
    pub weighted_bipred: bool,
    pub transquant_bypass_enabled: bool,
    pub tiles_enabled: bool,
    pub entropy_coding_sync_enabled: bool,
    pub num_tile_columns: usize,
    pub num_tile_rows: usize,
    pub uniform_spacing: bool,
    pub column_widths_minus1: Vec<u32>,
    pub row_heights_minus1: Vec<u32>,
    pub loop_filter_across_tiles_enabled: bool,
    pub loop_filter_across_slices_enabled: bool,
    pub deblocking_filter_control_present: bool,
    pub deblocking_filter_override_enabled: bool,
    pub pps_deblocking_filter_disabled: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub scaling_list: Option<ScalingList>,
    pub lists_modification_present: bool,
    pub log2_parallel_merge_level: u8,
    pub slice_segment_header_extension_present: bool,
}

pub fn parse_pps(rbsp: &[u8]) -> Result<Pps> {
    let mut r = BitReader::new(rbsp);
    r.skip(16)?;
    let id = r.ue_max(63, "pps_pic_parameter_set_id")? as u8;
    let sps_id = r.ue_max(15, "pps_seq_parameter_set_id")? as u8;
    let dependent_slice_segments_enabled = r.flag()?;
    let output_flag_present = r.flag()?;
    let num_extra_slice_header_bits = r.u(3)? as u8;
    let sign_data_hiding_enabled = r.flag()?;
    let cabac_init_present = r.flag()?;
    let num_ref_idx_l0_default_active = r.ue_max(14, "num_ref_idx_l0_default_active_minus1")? as u8 + 1;
    let num_ref_idx_l1_default_active = r.ue_max(14, "num_ref_idx_l1_default_active_minus1")? as u8 + 1;
    let init_qp = 26 + r.se_range(-(26 + 6 * 8), 25, "init_qp_minus26")?;
    let constrained_intra_pred = r.flag()?;
    let transform_skip_enabled = r.flag()?;
    let cu_qp_delta_enabled = r.flag()?;
    let mut diff_cu_qp_delta_depth = 0;
    if cu_qp_delta_enabled {
        diff_cu_qp_delta_depth = r.ue_max(3, "diff_cu_qp_delta_depth")? as u8;
    }
    let cb_qp_offset = r.se_range(-12, 12, "pps_cb_qp_offset")?;
    let cr_qp_offset = r.se_range(-12, 12, "pps_cr_qp_offset")?;
    let slice_chroma_qp_offsets_present = r.flag()?;
    let weighted_pred = r.flag()?;
    let weighted_bipred = r.flag()?;
    let transquant_bypass_enabled = r.flag()?;
    let tiles_enabled = r.flag()?;
    let entropy_coding_sync_enabled = r.flag()?;
    let mut num_tile_columns = 1;
    let mut num_tile_rows = 1;
    let mut uniform_spacing = true;
    let mut column_widths_minus1 = Vec::new();
    let mut row_heights_minus1 = Vec::new();
    let mut loop_filter_across_tiles_enabled = true;
    if tiles_enabled {
        num_tile_columns = r.ue_max(19, "num_tile_columns_minus1")? as usize + 1;
        num_tile_rows = r.ue_max(21, "num_tile_rows_minus1")? as usize + 1;
        uniform_spacing = r.flag()?;
        if !uniform_spacing {
            for _ in 0..num_tile_columns - 1 {
                column_widths_minus1.push(r.ue_max(1 << 16, "column_width_minus1")?);
            }
            for _ in 0..num_tile_rows - 1 {
                row_heights_minus1.push(r.ue_max(1 << 16, "row_height_minus1")?);
            }
        }
        loop_filter_across_tiles_enabled = r.flag()?;
    }
    let loop_filter_across_slices_enabled = r.flag()?;
    let deblocking_filter_control_present = r.flag()?;
    let mut deblocking_filter_override_enabled = false;
    let mut pps_deblocking_filter_disabled = false;
    let mut beta_offset_div2 = 0;
    let mut tc_offset_div2 = 0;
    if deblocking_filter_control_present {
        deblocking_filter_override_enabled = r.flag()?;
        pps_deblocking_filter_disabled = r.flag()?;
        if !pps_deblocking_filter_disabled {
            beta_offset_div2 = r.se_range(-6, 6, "pps_beta_offset_div2")?;
            tc_offset_div2 = r.se_range(-6, 6, "pps_tc_offset_div2")?;
        }
    }
    let mut scaling_list = None;
    if r.flag()? {
        scaling_list = Some(ScalingList::parse(&mut r)?);
    }
    let lists_modification_present = r.flag()?;
    let log2_parallel_merge_level = r.ue_max(4, "log2_parallel_merge_level_minus2")? as u8 + 2;
    let slice_segment_header_extension_present = r.flag()?;
    if r.flag().unwrap_or(false) {
        let range_ext = r.flag()?;
        r.skip(7)?;
        if range_ext {
            if transform_skip_enabled {
                let log2_max_ts = r.ue()?;
                if log2_max_ts != 0 {
                    return Err(Error::Unsupported("log2_max_transform_skip_block_size"));
                }
            }
            if r.flag()? {
                return Err(Error::Unsupported("cross_component_prediction"));
            }
            if r.flag()? {
                return Err(Error::Unsupported("chroma_qp_offset_list"));
            }
            if r.ue()? != 0 || r.ue()? != 0 {
                return Err(Error::Unsupported("log2_sao_offset_scale"));
            }
        }
    }
    Ok(Pps {
        id,
        sps_id,
        dependent_slice_segments_enabled,
        output_flag_present,
        num_extra_slice_header_bits,
        sign_data_hiding_enabled,
        cabac_init_present,
        num_ref_idx_l0_default_active,
        num_ref_idx_l1_default_active,
        init_qp,
        constrained_intra_pred,
        transform_skip_enabled,
        cu_qp_delta_enabled,
        diff_cu_qp_delta_depth,
        cb_qp_offset,
        cr_qp_offset,
        slice_chroma_qp_offsets_present,
        weighted_pred,
        weighted_bipred,
        transquant_bypass_enabled,
        tiles_enabled,
        entropy_coding_sync_enabled,
        num_tile_columns,
        num_tile_rows,
        uniform_spacing,
        column_widths_minus1,
        row_heights_minus1,
        loop_filter_across_tiles_enabled,
        loop_filter_across_slices_enabled,
        deblocking_filter_control_present,
        deblocking_filter_override_enabled,
        pps_deblocking_filter_disabled,
        beta_offset_div2,
        tc_offset_div2,
        scaling_list,
        lists_modification_present,
        log2_parallel_merge_level,
        slice_segment_header_extension_present,
    })
}

// ---------------------------------------------------------------------------
// Tile / scan derivation (6.5.1)
// ---------------------------------------------------------------------------

/// CTB raster/tile scan conversion tables for an (SPS, PPS) pair.
#[derive(Debug, Clone)]
pub struct TileLayout {
    pub col_bd: Vec<usize>, // in CTBs, len = cols + 1
    pub row_bd: Vec<usize>,
    pub rs_to_ts: Vec<usize>,
    pub ts_to_rs: Vec<usize>,
    pub tile_id: Vec<usize>, // indexed by TS address
}

impl TileLayout {
    pub fn new(sps: &Sps, pps: &Pps) -> Result<TileLayout> {
        let w = sps.pic_width_in_ctbs;
        let h = sps.pic_height_in_ctbs;
        let cols = pps.num_tile_columns;
        let rows = pps.num_tile_rows;
        if cols > w || rows > h {
            return Err(Error::Invalid("more tiles than CTBs"));
        }
        let mut col_width = vec![0usize; cols];
        let mut row_height = vec![0usize; rows];
        if pps.uniform_spacing {
            for i in 0..cols {
                col_width[i] = ((i + 1) * w) / cols - (i * w) / cols;
            }
            for j in 0..rows {
                row_height[j] = ((j + 1) * h) / rows - (j * h) / rows;
            }
        } else {
            let mut sum = 0usize;
            for i in 0..cols - 1 {
                col_width[i] = pps.column_widths_minus1[i] as usize + 1;
                sum += col_width[i];
            }
            if sum >= w {
                return Err(Error::Invalid("tile column widths"));
            }
            col_width[cols - 1] = w - sum;
            sum = 0;
            for j in 0..rows - 1 {
                row_height[j] = pps.row_heights_minus1[j] as usize + 1;
                sum += row_height[j];
            }
            if sum >= h {
                return Err(Error::Invalid("tile row heights"));
            }
            row_height[rows - 1] = h - sum;
        }
        let mut col_bd = vec![0usize; cols + 1];
        for i in 0..cols {
            col_bd[i + 1] = col_bd[i] + col_width[i];
        }
        let mut row_bd = vec![0usize; rows + 1];
        for j in 0..rows {
            row_bd[j + 1] = row_bd[j] + row_height[j];
        }
        let n = w * h;
        let mut rs_to_ts = vec![0usize; n];
        for ctb_rs in 0..n {
            let tbx = ctb_rs % w;
            let tby = ctb_rs / w;
            let mut tile_x = 0;
            for i in 0..cols {
                if tbx >= col_bd[i] {
                    tile_x = i;
                }
            }
            let mut tile_y = 0;
            for j in 0..rows {
                if tby >= row_bd[j] {
                    tile_y = j;
                }
            }
            let mut v = 0usize;
            for i in 0..tile_x {
                v += row_height[tile_y] * col_width[i];
            }
            for j in 0..tile_y {
                v += w * row_height[j];
            }
            v += (tby - row_bd[tile_y]) * col_width[tile_x] + tbx - col_bd[tile_x];
            rs_to_ts[ctb_rs] = v;
        }
        let mut ts_to_rs = vec![0usize; n];
        for rs in 0..n {
            ts_to_rs[rs_to_ts[rs]] = rs;
        }
        let mut tile_id = vec![0usize; n];
        let mut tid = 0;
        for j in 0..rows {
            for i in 0..cols {
                for y in row_bd[j]..row_bd[j + 1] {
                    for x in col_bd[i]..col_bd[i + 1] {
                        tile_id[rs_to_ts[y * w + x]] = tid;
                    }
                }
                tid += 1;
            }
        }
        Ok(TileLayout { col_bd, row_bd, rs_to_ts, ts_to_rs, tile_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diag_scan_4x4() {
        let d = diag_scan(4);
        let expect: [(u8, u8); 16] = [
            (0, 0),
            (0, 1),
            (1, 0),
            (0, 2),
            (1, 1),
            (2, 0),
            (0, 3),
            (1, 2),
            (2, 1),
            (3, 0),
            (1, 3),
            (2, 2),
            (3, 1),
            (2, 3),
            (3, 2),
            (3, 3),
        ];
        assert_eq!(d, expect.to_vec());
    }
}
