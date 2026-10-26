// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal H.264 syntax writer used to build synthetic test streams that exercise tools
//! common encoders never emit (I_PCM, long-term references and all MMCOs, FMO, ASO,
//! explicit bi-predictive weights, direct_8x8_inference_flag = 0, ...).

#![allow(dead_code)]

#[derive(Default, Clone)]
pub struct BitWriter {
    pub bits: Vec<bool>,
}

impl BitWriter {
    pub fn u(&mut self, n: u32, v: u64) {
        for i in (0..n).rev() {
            self.bits.push((v >> i) & 1 == 1);
        }
    }
    pub fn flag(&mut self, b: bool) {
        self.bits.push(b);
    }
    pub fn ue(&mut self, v: u32) {
        let v = v as u64 + 1;
        let len = 64 - v.leading_zeros();
        self.u(len - 1, 0);
        self.u(len, v);
    }
    pub fn se(&mut self, v: i32) {
        let k = if v > 0 { 2 * v as i64 - 1 } else { -2 * v as i64 };
        self.ue(k as u32);
    }
    pub fn te(&mut self, max: u32, v: u32) {
        if max == 1 {
            self.flag(v == 0);
        } else {
            self.ue(v);
        }
    }
    pub fn aligned(&self) -> bool {
        self.bits.len() % 8 == 0
    }
    pub fn align_zero(&mut self) {
        while !self.aligned() {
            self.bits.push(false);
        }
    }
    pub fn trailing(&mut self) {
        self.bits.push(true);
        self.align_zero();
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.bits.len().div_ceil(8)];
        for (i, &b) in self.bits.iter().enumerate() {
            if b {
                out[i / 8] |= 0x80 >> (i % 8);
            }
        }
        out
    }
}

/// Wrap an RBSP into an Annex-B NAL unit with emulation prevention.
pub fn nal(nal_ref_idc: u8, nal_type: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, (nal_ref_idc << 5) | nal_type];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        if b == 0 { zeros += 1 } else { zeros = 0 }
    }
    out
}

#[derive(Clone)]
pub struct SpsCfg {
    pub profile_idc: u8,
    pub level_idc: u8,
    pub sps_id: u32,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    pub num_ref_frames: u32,
    pub gaps_allowed: bool,
    pub mb_w: u32,
    pub mb_h: u32,
    pub direct_8x8_inference: bool,
    pub crop: Option<(u32, u32, u32, u32)>,
    /// VUI bitstream_restriction: (max_num_reorder_frames, max_dec_frame_buffering).
    pub reorder: Option<(u32, u32)>,
    /// frame_mbs_only_flag (false: field pictures allowed; `mb_h` counts map units).
    pub frame_mbs_only: bool,
    pub mbaff: bool,
}

impl Default for SpsCfg {
    fn default() -> Self {
        SpsCfg {
            profile_idc: 77,
            level_idc: 30,
            sps_id: 0,
            log2_max_frame_num: 4,
            poc_type: 0,
            log2_max_poc_lsb: 6,
            num_ref_frames: 4,
            gaps_allowed: false,
            mb_w: 4,
            mb_h: 3,
            direct_8x8_inference: true,
            crop: None,
            reorder: None,
            frame_mbs_only: true,
            mbaff: false,
        }
    }
}

pub fn sps(c: &SpsCfg) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.u(8, c.profile_idc as u64);
    w.u(8, 0);
    w.u(8, c.level_idc as u64);
    w.ue(c.sps_id);
    if c.profile_idc == 100 {
        w.ue(1); // chroma_format_idc
        w.ue(0);
        w.ue(0);
        w.flag(false);
        w.flag(false);
    }
    w.ue(c.log2_max_frame_num - 4);
    w.ue(c.poc_type);
    if c.poc_type == 0 {
        w.ue(c.log2_max_poc_lsb - 4);
    } else if c.poc_type == 1 {
        w.flag(false); // delta_pic_order_always_zero
        w.se(-1); // offset_for_non_ref_pic
        w.se(0);
        w.ue(2);
        w.se(3);
        w.se(1);
    }
    w.ue(c.num_ref_frames);
    w.flag(c.gaps_allowed);
    w.ue(c.mb_w - 1);
    w.ue(c.mb_h - 1);
    w.flag(c.frame_mbs_only);
    if !c.frame_mbs_only {
        w.flag(c.mbaff);
    }
    w.flag(c.direct_8x8_inference);
    match c.crop {
        Some((l, r, t, b)) => {
            w.flag(true);
            w.ue(l);
            w.ue(r);
            w.ue(t);
            w.ue(b);
        }
        None => w.flag(false),
    }
    match c.reorder {
        Some((reorder, dec_buf)) => {
            w.flag(true); // vui
            w.flag(false); // aspect
            w.flag(false); // overscan
            w.flag(false); // video signal
            w.flag(false); // chroma loc
            w.flag(false); // timing
            w.flag(false); // nal hrd
            w.flag(false); // vcl hrd
            w.flag(false); // pic_struct
            w.flag(true); // bitstream_restriction
            w.flag(true);
            w.ue(0);
            w.ue(0);
            w.ue(16);
            w.ue(16);
            w.ue(reorder);
            w.ue(dec_buf);
        }
        None => w.flag(false),
    }
    w.trailing();
    nal(3, 7, &w.bytes())
}

#[derive(Clone)]
pub struct PpsCfg {
    pub pps_id: u32,
    pub sps_id: u32,
    pub cabac: bool,
    pub slice_groups: Option<SliceGroups>,
    pub num_ref_idx_default: [u32; 2],
    pub weighted_pred: bool,
    pub weighted_bipred_idc: u32,
    pub init_qp: i32,
    pub chroma_qp_offset: i32,
    pub deblocking_control: bool,
    pub constrained_intra: bool,
    pub redundant_pic_cnt_present: bool,
}

#[derive(Clone)]
pub enum SliceGroups {
    Interleaved(Vec<u32>),
    Dispersed(u32),
    Foreground(u32, Vec<(u32, u32)>),
    /// type 3..5 with change direction and rate
    Evolving(u32, bool, u32),
    Explicit(u32, Vec<u32>),
}

impl Default for PpsCfg {
    fn default() -> Self {
        PpsCfg {
            pps_id: 0,
            sps_id: 0,
            cabac: false,
            slice_groups: None,
            num_ref_idx_default: [1, 1],
            weighted_pred: false,
            weighted_bipred_idc: 0,
            init_qp: 26,
            chroma_qp_offset: 0,
            deblocking_control: true,
            constrained_intra: false,
            redundant_pic_cnt_present: false,
        }
    }
}

pub fn pps(c: &PpsCfg, pic_size_in_mbs: u32) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.ue(c.pps_id);
    w.ue(c.sps_id);
    w.flag(c.cabac);
    w.flag(false); // bottom_field_pic_order_in_frame_present
    match &c.slice_groups {
        None => w.ue(0),
        Some(sg) => match sg {
            SliceGroups::Interleaved(runs) => {
                w.ue(runs.len() as u32 - 1);
                w.ue(0);
                for &r in runs {
                    w.ue(r - 1);
                }
            }
            SliceGroups::Dispersed(n) => {
                w.ue(n - 1);
                w.ue(1);
            }
            SliceGroups::Foreground(n, rects) => {
                w.ue(n - 1);
                w.ue(2);
                for &(tl, br) in rects {
                    w.ue(tl);
                    w.ue(br);
                }
            }
            SliceGroups::Evolving(t, dir, rate) => {
                w.ue(1);
                w.ue(*t);
                w.flag(*dir);
                w.ue(rate - 1);
            }
            SliceGroups::Explicit(n, ids) => {
                w.ue(n - 1);
                w.ue(6);
                w.ue(pic_size_in_mbs - 1);
                let bits = 32 - (n - 1).leading_zeros();
                for &id in ids {
                    w.u(bits, id as u64);
                }
            }
        },
    }
    w.ue(c.num_ref_idx_default[0] - 1);
    w.ue(c.num_ref_idx_default[1] - 1);
    w.flag(c.weighted_pred);
    w.u(2, c.weighted_bipred_idc as u64);
    w.se(c.init_qp - 26);
    w.se(0);
    w.se(c.chroma_qp_offset);
    w.flag(c.deblocking_control);
    w.flag(c.constrained_intra);
    w.flag(c.redundant_pic_cnt_present);
    w.trailing();
    nal(3, 8, &w.bytes())
}

#[derive(Clone, Copy, Debug)]
pub enum Mmco {
    ForgetShort(u32),
    ForgetLong(u32),
    ShortToLong(u32, u32),
    MaxLong(u32),
    ForgetAll,
    CurrentToLong(u32),
}

#[derive(Clone)]
pub struct SliceCfg {
    pub first_mb: u32,
    /// 0 P, 1 B, 2 I
    pub slice_type: u32,
    pub pps_id: u32,
    pub frame_num: u32,
    pub idr: bool,
    pub idr_pic_id: u32,
    pub nal_ref_idc: u8,
    pub poc_lsb: u32,
    pub delta_poc: [i32; 2],
    pub direct_spatial: bool,
    pub num_ref_idx: Option<[u32; 2]>,
    /// (idc, value) per list
    pub modification: [Vec<(u32, u32)>; 2],
    /// Explicit weights per list: Vec<(luma (w, o) or None, chroma [(w, o); 2] or None)>.
    pub weights: Option<(u32, u32, [Vec<(Option<(i32, i32)>, Option<[(i32, i32); 2]>)>; 2])>,
    pub long_term_reference_flag: bool,
    pub no_output_of_prior_pics: bool,
    pub mmcos: Option<Vec<Mmco>>,
    pub qp_delta: i32,
    pub deblock: Option<(u32, i32, i32)>,
    pub slice_group_change_cycle: Option<(u32, u32)>,
    pub redundant_pic_cnt: Option<u32>,
    pub cabac_init_idc: u32,
    /// Field picture: Some(bottom_field_flag).
    pub field: Option<bool>,
}

impl Default for SliceCfg {
    fn default() -> Self {
        SliceCfg {
            first_mb: 0,
            slice_type: 2,
            pps_id: 0,
            frame_num: 0,
            idr: true,
            idr_pic_id: 0,
            nal_ref_idc: 3,
            poc_lsb: 0,
            delta_poc: [0, 0],
            direct_spatial: true,
            num_ref_idx: None,
            modification: [Vec::new(), Vec::new()],
            weights: None,
            long_term_reference_flag: false,
            no_output_of_prior_pics: false,
            mmcos: None,
            qp_delta: 0,
            deblock: None,
            slice_group_change_cycle: None,
            redundant_pic_cnt: None,
            cabac_init_idc: 0,
            field: None,
        }
    }
}

pub fn slice_header(w: &mut BitWriter, s: &SliceCfg, sps: &SpsCfg, pps: &PpsCfg) {
    w.ue(s.first_mb);
    w.ue(s.slice_type + 5);
    w.ue(s.pps_id);
    w.u(sps.log2_max_frame_num, s.frame_num as u64);
    if !sps.frame_mbs_only {
        w.flag(s.field.is_some());
        if let Some(b) = s.field {
            w.flag(b);
        }
    }
    if s.idr {
        w.ue(s.idr_pic_id);
    }
    if sps.poc_type == 0 {
        w.u(sps.log2_max_poc_lsb, s.poc_lsb as u64);
    } else if sps.poc_type == 1 {
        w.se(s.delta_poc[0]);
    }
    if pps.redundant_pic_cnt_present {
        w.ue(s.redundant_pic_cnt.unwrap_or(0));
    }
    if s.slice_type == 1 {
        w.flag(s.direct_spatial);
    }
    if s.slice_type != 2 {
        match s.num_ref_idx {
            Some(n) => {
                w.flag(true);
                w.ue(n[0] - 1);
                if s.slice_type == 1 {
                    w.ue(n[1] - 1);
                }
            }
            None => w.flag(false),
        }
        let lists = if s.slice_type == 1 { 2 } else { 1 };
        for l in 0..lists {
            if s.modification[l].is_empty() {
                w.flag(false);
            } else {
                w.flag(true);
                for &(idc, v) in &s.modification[l] {
                    w.ue(idc);
                    w.ue(v);
                }
                w.ue(3);
            }
        }
    }
    let wp = (pps.weighted_pred && s.slice_type == 0) || (pps.weighted_bipred_idc == 1 && s.slice_type == 1);
    if wp {
        let (ld, cd, lists) = s.weights.as_ref().expect("weights");
        w.ue(*ld);
        w.ue(*cd);
        let nl = if s.slice_type == 1 { 2 } else { 1 };
        for l in 0..nl {
            for e in &lists[l] {
                match e.0 {
                    Some((wt, o)) => {
                        w.flag(true);
                        w.se(wt);
                        w.se(o);
                    }
                    None => w.flag(false),
                }
                match e.1 {
                    Some(c) => {
                        w.flag(true);
                        for (wt, o) in c {
                            w.se(wt);
                            w.se(o);
                        }
                    }
                    None => w.flag(false),
                }
            }
        }
    }
    if s.nal_ref_idc != 0 {
        if s.idr {
            w.flag(s.no_output_of_prior_pics);
            w.flag(s.long_term_reference_flag);
        } else {
            match &s.mmcos {
                None => w.flag(false),
                Some(ops) => {
                    w.flag(true);
                    for op in ops {
                        match *op {
                            Mmco::ForgetShort(d) => {
                                w.ue(1);
                                w.ue(d);
                            }
                            Mmco::ForgetLong(n) => {
                                w.ue(2);
                                w.ue(n);
                            }
                            Mmco::ShortToLong(d, i) => {
                                w.ue(3);
                                w.ue(d);
                                w.ue(i);
                            }
                            Mmco::MaxLong(p) => {
                                w.ue(4);
                                w.ue(p);
                            }
                            Mmco::ForgetAll => w.ue(5),
                            Mmco::CurrentToLong(i) => {
                                w.ue(6);
                                w.ue(i);
                            }
                        }
                    }
                    w.ue(0);
                }
            }
        }
    }
    if pps.cabac && s.slice_type != 2 {
        w.ue(s.cabac_init_idc);
    }
    w.se(s.qp_delta);
    if pps.deblocking_control {
        let (idc, a, b) = s.deblock.unwrap_or((0, 0, 0));
        w.ue(idc);
        if idc != 1 {
            w.se(a);
            w.se(b);
        }
    }
    if let Some((bits, v)) = s.slice_group_change_cycle {
        w.u(bits, v as u64);
    }
}

/// CAVLC macroblock descriptions with no residual (cbp = 0) or I_PCM.
#[derive(Clone, Debug)]
pub enum Mb {
    Pcm(Vec<u8>),
    /// P_L0_16x16: ref_idx, mvd.
    P16x16(u32, [i32; 2]),
    /// P_8x8 with sub types (0..3) and per sub: ref, mvds (one per sub-partition).
    P8x8([u32; 4], [u32; 4], Vec<[i32; 2]>),
    /// B 16x16: pred (1 L0, 2 L1, 3 Bi), refs, mvds.
    B16x16(u32, [u32; 2], [[i32; 2]; 2]),
    /// B_Direct_16x16 (no residual).
    BDirect,
    /// B_8x8 with sub types (0..12), refs per sub per list, mvds in syntax order.
    B8x8([u32; 4], [[u32; 4]; 2], Vec<[i32; 2]>),
    Skip,
}

pub fn write_mb_cavlc(w: &mut BitWriter, mb: &Mb, slice_type: u32, nref: [u32; 2], chroma: bool) {
    match mb {
        Mb::Pcm(data) => {
            let t = match slice_type {
                2 => 25,
                0 => 5 + 25,
                _ => 23 + 25,
            };
            w.ue(t);
            w.align_zero();
            let n = if chroma { 384 } else { 256 };
            for &b in &data[..n] {
                w.u(8, b as u64);
            }
        }
        Mb::P16x16(r, mvd) => {
            w.ue(0);
            if nref[0] > 1 {
                w.te(nref[0] - 1, *r);
            }
            w.se(mvd[0]);
            w.se(mvd[1]);
            w.ue(0); // cbp 0 (inter)
        }
        Mb::P8x8(subs, refs, mvds) => {
            w.ue(3);
            for &s in subs {
                w.ue(s);
            }
            if nref[0] > 1 {
                for &r in refs {
                    w.te(nref[0] - 1, r);
                }
            }
            for m in mvds {
                w.se(m[0]);
                w.se(m[1]);
            }
            w.ue(0);
        }
        Mb::B16x16(pred, refs, mvds) => {
            w.ue(*pred);
            for l in 0..2 {
                if pred & (1 << l) != 0 && nref[l] > 1 {
                    w.te(nref[l] - 1, refs[l]);
                }
            }
            for l in 0..2 {
                if pred & (1 << l) != 0 {
                    w.se(mvds[l][0]);
                    w.se(mvds[l][1]);
                }
            }
            w.ue(0);
        }
        Mb::BDirect => {
            w.ue(0);
            w.ue(0);
        }
        Mb::B8x8(subs, refs, mvds) => {
            w.ue(22);
            for &s in subs {
                w.ue(s);
            }
            let uses = |s: u32, l: usize| -> bool {
                let p = match s {
                    0 => 0,
                    1 | 4 | 5 | 10 => 1,
                    2 | 6 | 7 | 11 => 2,
                    _ => 3,
                };
                p & (1 << l) != 0
            };
            for l in 0..2 {
                for i in 0..4 {
                    if uses(subs[i], l) && nref[l] > 1 {
                        w.te(nref[l] - 1, refs[l][i]);
                    }
                }
            }
            for m in mvds {
                w.se(m[0]);
                w.se(m[1]);
            }
            w.ue(0);
        }
        Mb::Skip => unreachable!("skips are coded as runs"),
    }
}

/// Write a CAVLC slice: header + macroblocks (with skip runs) + trailing bits.
pub fn cavlc_slice(s: &SliceCfg, sps_c: &SpsCfg, pps_c: &PpsCfg, mbs: &[Mb]) -> Vec<u8> {
    let mut w = BitWriter::default();
    slice_header(&mut w, s, sps_c, pps_c);
    let nref = s.num_ref_idx.unwrap_or(pps_c.num_ref_idx_default);
    let mut run = 0;
    for mb in mbs {
        if s.slice_type != 2 {
            if let Mb::Skip = mb {
                run += 1;
                continue;
            }
            w.ue(run);
            run = 0;
        }
        write_mb_cavlc(&mut w, mb, s.slice_type, nref, true);
    }
    if run > 0 {
        w.ue(run);
    }
    w.trailing();
    nal(s.nal_ref_idc, if s.idr { 5 } else { 1 }, &w.bytes())
}

// ---- CABAC writer (I_PCM / skip macroblocks only) ----------------------------------------

#[path = "../../src/cabac_tables.rs"]
mod cabac_tables;

const RANGE_TAB_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240], [128, 167, 197, 227], [128, 158, 187, 216], [123, 150, 178, 205],
    [116, 142, 169, 195], [111, 135, 160, 185], [105, 128, 152, 175], [100, 122, 144, 166],
    [95, 116, 137, 158], [90, 110, 130, 150], [85, 104, 123, 142], [81, 99, 117, 135],
    [77, 94, 111, 128], [73, 89, 105, 122], [69, 85, 100, 116], [66, 80, 95, 110],
    [62, 76, 90, 104], [59, 72, 86, 99], [56, 69, 81, 94], [53, 65, 77, 89],
    [51, 62, 73, 85], [48, 59, 69, 80], [46, 56, 66, 76], [43, 53, 63, 72],
    [41, 50, 59, 69], [39, 48, 56, 65], [37, 45, 54, 62], [35, 43, 51, 59],
    [33, 41, 48, 56], [32, 39, 46, 53], [30, 37, 43, 50], [29, 35, 41, 48],
    [27, 33, 39, 45], [26, 31, 37, 43], [24, 30, 35, 41], [23, 28, 33, 39],
    [22, 27, 32, 37], [21, 26, 30, 35], [20, 24, 29, 33], [19, 23, 27, 31],
    [18, 22, 26, 30], [17, 21, 25, 28], [16, 20, 23, 27], [15, 19, 22, 25],
    [14, 18, 21, 24], [14, 17, 20, 23], [13, 16, 19, 22], [12, 15, 18, 21],
    [12, 14, 17, 20], [11, 14, 16, 19], [11, 13, 15, 18], [10, 12, 15, 17],
    [10, 12, 14, 16], [9, 11, 13, 15], [9, 11, 12, 14], [8, 10, 12, 14],
    [8, 9, 11, 13], [7, 9, 11, 12], [7, 9, 10, 12], [7, 8, 10, 11],
    [6, 8, 9, 11], [6, 7, 9, 10], [6, 7, 8, 9], [2, 2, 2, 2],
];
const TRANS_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21, 21, 22, 22, 23, 24,
    24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34, 34, 35, 35, 35, 36, 36, 36, 37, 37, 37,
    38, 38, 63,
];

pub struct CabacEnc {
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
    /// (pStateIdx, valMPS)
    ctx: Vec<(u8, u8)>,
}

impl CabacEnc {
    pub fn new(qp: i32, init: usize) -> CabacEnc {
        let mut ctx = Vec::with_capacity(1024);
        for mn in cabac_tables::CABAC_INIT_MN[init].iter() {
            let pre = (((mn[0] as i32 * qp.clamp(0, 51)) >> 4) + mn[1] as i32).clamp(1, 126);
            ctx.push(if pre <= 63 { ((63 - pre) as u8, 0) } else { ((pre - 64) as u8, 1) });
        }
        CabacEnc { low: 0, range: 510, outstanding: 0, first: true, ctx }
    }
    fn reset_engine(&mut self) {
        self.low = 0;
        self.range = 510;
        self.outstanding = 0;
        self.first = true;
    }
    fn put_bit(&mut self, w: &mut BitWriter, b: bool) {
        if self.first {
            self.first = false;
        } else {
            w.flag(b);
        }
        while self.outstanding > 0 {
            w.flag(!b);
            self.outstanding -= 1;
        }
    }
    fn renorm(&mut self, w: &mut BitWriter) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(w, false);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(w, true);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }
    pub fn decision(&mut self, w: &mut BitWriter, ctx: usize, bin: u32) {
        let (p, mps) = self.ctx[ctx];
        let lps = RANGE_TAB_LPS[p as usize][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps as u32 {
            self.low += self.range;
            self.range = lps;
            if p == 0 {
                self.ctx[ctx].1 = 1 - mps;
            }
            self.ctx[ctx].0 = TRANS_LPS[p as usize];
        } else if p < 62 {
            self.ctx[ctx].0 = p + 1;
        }
        self.renorm(w);
    }
    /// EncodeTerminate (9.3.4.5); with bin 1 the engine is flushed.
    pub fn terminate(&mut self, w: &mut BitWriter, bin: u32) {
        self.range -= 2;
        if bin != 0 {
            self.low += self.range;
            self.range = 2;
            self.renorm(w);
            self.put_bit(w, (self.low >> 9) & 1 == 1);
            let v = ((self.low >> 7) & 3) | 1;
            w.flag(v & 2 != 0);
            w.flag(v & 1 != 0);
        } else {
            self.renorm(w);
        }
    }
}

/// CABAC slice made of I_PCM and skipped macroblocks (`Mb::Pcm` / `Mb::Skip`), raster order,
/// single slice covering the picture. `mb_w` is the picture width in MBs.
pub fn cabac_pcm_slice(s: &SliceCfg, sps_c: &SpsCfg, pps_c: &PpsCfg, mbs: &[Mb]) -> Vec<u8> {
    let mut w = BitWriter::default();
    slice_header(&mut w, s, sps_c, pps_c);
    while !w.aligned() {
        w.flag(true); // cabac_alignment_one_bit
    }
    let qp = pps_c.init_qp + s.qp_delta;
    let init = if s.slice_type == 2 { 3 } else { s.cabac_init_idc as usize };
    let mut e = CabacEnc::new(qp, init);
    let mb_w = sps_c.mb_w as usize;
    let first = s.first_mb as usize;
    let avail = |i: usize, n: Option<usize>| n.map(|a| a >= first && a < i).unwrap_or(false);
    for (k, mb) in mbs.iter().enumerate() {
        let i = first + k;
        let a = if i % mb_w > 0 { Some(i - 1) } else { None };
        let b = if i >= mb_w { Some(i - mb_w) } else { None };
        let is_skip = |n: usize| matches!(mbs[n - first], Mb::Skip);
        if s.slice_type != 2 {
            let inc = [a, b].iter().filter(|n| avail(i, **n) && !is_skip(n.unwrap())).count();
            let base = if s.slice_type == 1 { 24 } else { 11 };
            let skip = matches!(mb, Mb::Skip);
            e.decision(&mut w, base + inc, skip as u32);
            if skip {
                e.terminate(&mut w, (k + 1 == mbs.len()) as u32);
                continue;
            }
        }
        let Mb::Pcm(data) = mb else { panic!("only PCM and skip") };
        match s.slice_type {
            2 => {
                // bin 0 ctx 3 + (A, B available and not I_NxN): all our MBs are PCM.
                let inc = [a, b].iter().filter(|n| avail(i, **n)).count();
                e.decision(&mut w, 3 + inc, 1);
            }
            0 => {
                e.decision(&mut w, 14, 1); // intra prefix
                e.decision(&mut w, 17, 1); // not I_NxN
            }
            _ => {
                // B: neighbours that are not B_Skip/B_Direct count.
                let inc = [a, b].iter().filter(|n| avail(i, **n) && !is_skip(n.unwrap())).count();
                e.decision(&mut w, 27 + inc, 1);
                e.decision(&mut w, 27 + 3, 1);
                e.decision(&mut w, 27 + 4, 1);
                e.decision(&mut w, 27 + 5, 1);
                e.decision(&mut w, 27 + 5, 0);
                e.decision(&mut w, 27 + 5, 1);
                e.decision(&mut w, 32, 1);
            }
        }
        e.terminate(&mut w, 1); // I_PCM
        w.align_zero();
        for &v in &data[..384] {
            w.u(8, v as u64);
        }
        e.reset_engine();
        e.terminate(&mut w, (k + 1 == mbs.len()) as u32);
    }
    w.align_zero();
    nal(s.nal_ref_idc, if s.idr { 5 } else { 1 }, &w.bytes())
}
