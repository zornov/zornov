// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Picture-level decoding: parameter set activation, picture boundaries (7.4.1.2.4),
//! picture order count (8.2.1), reference picture lists for frames and fields (8.2.4),
//! reference marking (8.2.5), frame_num gaps (8.2.5.2), complementary field pairs and the
//! DPB output/bumping process (C.4).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::bitreader::BitReader;
use crate::dbpipe::DeblockPipe;
use crate::deblock::{DbMb, DbSlice, DeblockCtx, deblock_picture};
use crate::error::{Error, Result};
use crate::fmo::slice_group_map;
use crate::mb::MbInfo;
use crate::nal::{self, AvcConfig, Nal};
use crate::params::{Pps, Sps};
use crate::picture::{MbMotion, PAR_BOTTOM, PAR_FRAME, PAR_TOP, Picture, Planes, Structure};
use crate::slice::{SliceHeader, SliceType, parse_slice_header, peek_pps_id};
use crate::recon::{MbRec, ReconSink};
use crate::slicedec::{Dequant, PicState, PredWeights, RefPic, SliceDecoder, SliceEnv};

/// Colour description from the VUI, used for RGB conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorInfo {
    /// matrix_coefficients (2 = unspecified).
    pub matrix_coefficients: u8,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub full_range: bool,
}

impl Default for ColorInfo {
    fn default() -> Self {
        ColorInfo { matrix_coefficients: 2, colour_primaries: 2, transfer_characteristics: 2, full_range: false }
    }
}

/// A decoded, cropped frame in 8-bit I420 layout.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    /// Luma plane, `width * height` bytes.
    pub y: Vec<u8>,
    /// Cb plane, `chroma_width * chroma_height` bytes.
    pub u: Vec<u8>,
    /// Cr plane.
    pub v: Vec<u8>,
    /// Picture order count (display order within a coded video sequence).
    pub poc: i32,
    /// Presentation timestamp passed to `decode_with_pts`, if any.
    pub pts: Option<i64>,
    /// The frame is an IDR picture.
    pub idr: bool,
    /// The frame was coded as two fields or as an interlaced (MBAFF) frame.
    pub interlaced: bool,
    pub color: ColorInfo,
    /// Sample aspect ratio (numerator, denominator); (0, 0) when unknown.
    pub sar: (u16, u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RefMark {
    Unused,
    Short,
    Long(u32),
}

struct DpbEntry {
    pic: Arc<Picture>,
    frame_num: u32,
    /// Marking of the top and bottom field (both equal for frames).
    marks: [RefMark; 2],
    /// Fields present.
    has: [bool; 2],
    /// False while the second field of a pair is awaited.
    complete: bool,
    needed_for_output: bool,
    poc: i32,
    pts: Option<i64>,
    idr: bool,
    interlaced: bool,
}

impl DpbEntry {
    fn is_ref(&self) -> bool {
        self.marks.iter().any(|m| *m != RefMark::Unused)
    }
    fn any_short(&self) -> bool {
        self.marks.contains(&RefMark::Short)
    }
    fn any_long(&self) -> bool {
        self.marks.iter().any(|m| matches!(m, RefMark::Long(_)))
    }
    /// Frame usable as short-term reference for frame decoding (both fields short).
    fn frame_short(&self) -> bool {
        self.has[0] && self.has[1] && self.marks == [RefMark::Short; 2]
    }
    fn frame_long(&self) -> Option<u32> {
        match self.marks {
            [RefMark::Long(a), RefMark::Long(b)] if a == b && self.has[0] && self.has[1] => Some(a),
            _ => None,
        }
    }
    fn long_idx(&self) -> Option<u32> {
        self.marks.iter().find_map(|m| if let RefMark::Long(i) = m { Some(*i) } else { None })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CurStructure {
    Frame,
    Mbaff,
    Field(u8),
}

/// Picture (frame or field) being decoded.
struct CurPic {
    planes: Planes,
    info: Vec<MbInfo>,
    db: Vec<DbMb>,
    db_slices: Vec<DbSlice>,
    motion: Vec<MbMotion>,
    sps: Arc<Sps>,
    /// Header of the first slice (marking, POC).
    hdr: SliceHeader,
    /// Header of the last slice (for picture boundary detection).
    last_hdr: SliceHeader,
    structure: CurStructure,
    /// Second field of the frame with this id (stored in the DPB).
    second_of: Option<u32>,
    pic_id: u32,
    top_poc: i32,
    bottom_poc: i32,
    poc_msb: i32,
    frame_num_offset: i32,
    mbs_decoded: usize,
    pts: Option<i64>,
    /// Deblocking overlapped with decoding (progressive 4:2:0 frames).
    pipe: Option<DeblockPipe>,
}

impl CurPic {
    fn poc(&self) -> i32 {
        match self.structure {
            CurStructure::Field(PAR_TOP) => self.top_poc,
            CurStructure::Field(_) => self.bottom_poc,
            _ => self.top_poc.min(self.bottom_poc),
        }
    }
    fn parity(&self) -> u8 {
        match self.structure {
            CurStructure::Field(p) => p,
            _ => PAR_FRAME,
        }
    }
}

/// A first field waiting for its second field.
#[derive(Clone, Copy)]
struct PendingField {
    pic_id: u32,
    parity: u8,
    frame_num: u32,
}

/// How NAL units are framed in the input buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    /// Each NAL unit preceded by a big-endian length of 1, 2 or 4 bytes (MP4 / avcC).
    LengthPrefixed(usize),
    /// Annex-B byte stream with start codes.
    AnnexB,
}

/// H.264 decoder.
pub struct Decoder {
    framing: Framing,
    sps: Vec<Option<Arc<Sps>>>,
    pps: Vec<Option<Arc<Pps>>>,
    active_sps: Option<Arc<Sps>>,
    dpb: Vec<DpbEntry>,
    cur: Option<CurPic>,
    pending_field: Option<PendingField>,
    next_pic_id: u32,
    // POC / frame_num state of the previous (reference) picture.
    prev_ref_poc_msb: i32,
    prev_ref_poc_lsb: i32,
    prev_frame_num_offset: i32,
    prev_frame_num: u32,
    prev_ref_frame_num: u32,
    prev_pic_had_mmco5: bool,
    max_long_term_frame_idx: Option<u32>,
    output: VecDeque<Frame>,
    errors: Vec<Error>,
    /// Spare macroblock reconstruction records, reused across slices.
    rec_pool: Vec<Box<MbRec>>,
    pending_pts: Option<i64>,
    /// Number of macroblocks concealed so far (corrupted or missing slices).
    concealed_mbs: u64,
    /// Partition A slice waiting for partitions B and C.
    pending_dp: Option<PendingDp>,
    /// Testing aid: per decoded slice, slice_data() offset and residual bit ranges.
    trace: Option<Vec<(usize, Vec<(usize, usize, u8)>)>>,
}

struct PendingDp {
    a: Nal,
    slice_id: u32,
    pps: Arc<Pps>,
    sps: Arc<Sps>,
    /// (RBSP, bit offset of slice_data) for partitions B and C.
    parts: [Option<(Vec<u8>, usize)>; 2],
}

fn crop_frame(e: &DpbEntry, sps: &Sps) -> Frame {
    let pic = &e.pic;
    let (cx, cy) = sps.crop_units();
    let (w, h) = sps.cropped_size();
    let x0 = (sps.crop_left * cx) as usize;
    let y0 = (sps.crop_top * cy) as usize;
    let y = pic.y.crop_to_vec(x0, y0, w as usize, h as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (u, v) = if sps.chroma_array_type() != 0 {
        (
            pic.cb.crop_to_vec(x0 / 2, y0 / 2, cw as usize, ch as usize),
            pic.cr.crop_to_vec(x0 / 2, y0 / 2, cw as usize, ch as usize),
        )
    } else {
        (vec![128; (cw * ch) as usize], vec![128; (cw * ch) as usize])
    };
    let vui = &sps.vui;
    let color = if vui.video_signal_type_present {
        ColorInfo {
            matrix_coefficients: if vui.colour_description_present { vui.matrix_coefficients } else { 2 },
            colour_primaries: if vui.colour_description_present { vui.colour_primaries } else { 2 },
            transfer_characteristics: if vui.colour_description_present { vui.transfer_characteristics } else { 2 },
            full_range: vui.video_full_range_flag,
        }
    } else {
        ColorInfo::default()
    };
    let sar = match vui.aspect_ratio_idc {
        0 => (0, 0),
        255 => (vui.sar_width, vui.sar_height),
        i => {
            const T: [(u16, u16); 17] = [
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
            T.get(i as usize).copied().unwrap_or((0, 0))
        }
    };
    Frame {
        width: w,
        height: h,
        chroma_width: cw,
        chroma_height: ch,
        y,
        u,
        v,
        poc: e.poc,
        pts: e.pts,
        idr: e.idr,
        interlaced: e.interlaced,
        color,
        sar,
    }
}

fn frame_num_wrap(frame_num: u32, cur: u32, max: u32) -> i64 {
    if frame_num > cur { frame_num as i64 - max as i64 } else { frame_num as i64 }
}

/// 8.2.4.2.5: alternate fields of the ordered entries, starting with `parity`.
/// `marked(entry, field)` selects fields usable in this list.
fn alternate_fields<'a>(entries: &[&'a DpbEntry], parity: u8, marked: impl Fn(&DpbEntry, usize) -> bool) -> Vec<(&'a DpbEntry, u8)> {
    let mut out = Vec::new();
    let same = parity as usize;
    let opp = 1 - same;
    let (mut i, mut j) = (0, 0);
    let n = entries.len();
    loop {
        while i < n && !marked(entries[i], same) {
            i += 1;
        }
        while j < n && !marked(entries[j], opp) {
            j += 1;
        }
        if i >= n && j >= n {
            break;
        }
        if i < n {
            out.push((entries[i], same as u8));
            i += 1;
        }
        if j < n {
            out.push((entries[j], opp as u8));
            j += 1;
        }
    }
    out
}

impl Decoder {
    /// Create a decoder for MP4-style input from an `avcC` configuration.
    pub fn from_avcc(cfg: &AvcConfig) -> Result<Decoder> {
        let mut d = Decoder::with_framing(Framing::LengthPrefixed(cfg.nal_length_size));
        for s in &cfg.sps {
            d.handle_nal_bytes(s)?;
        }
        for p in &cfg.pps {
            d.handle_nal_bytes(p)?;
        }
        Ok(d)
    }

    /// Create a decoder from raw `avcC` box payload bytes.
    pub fn from_avcc_bytes(avcc: &[u8]) -> Result<Decoder> {
        Decoder::from_avcc(&AvcConfig::parse(avcc)?)
    }

    /// Create a decoder from SPS/PPS NAL units (header byte included, emulation prevention
    /// bytes allowed) and a NAL length size (0 for Annex-B input).
    pub fn new(sps: &[&[u8]], pps: &[&[u8]], nal_length_size: usize) -> Result<Decoder> {
        let framing = match nal_length_size {
            0 => Framing::AnnexB,
            1 | 2 | 4 => Framing::LengthPrefixed(nal_length_size),
            _ => return Err(Error::InvalidConfig("NAL length size must be 0, 1, 2 or 4")),
        };
        let mut d = Decoder::with_framing(framing);
        for s in sps {
            d.handle_nal_bytes(s)?;
        }
        for p in pps {
            d.handle_nal_bytes(p)?;
        }
        Ok(d)
    }

    /// Decoder for Annex-B byte streams (parameter sets are taken from the stream).
    pub fn new_annex_b() -> Decoder {
        Decoder::with_framing(Framing::AnnexB)
    }

    pub fn with_framing(framing: Framing) -> Decoder {
        crate::cavlc::init_tables();
        Decoder {
            framing,
            sps: vec![None; 32],
            pps: vec![None; 256],
            active_sps: None,
            dpb: Vec::new(),
            cur: None,
            pending_field: None,
            next_pic_id: 1,
            prev_ref_poc_msb: 0,
            prev_ref_poc_lsb: 0,
            prev_frame_num_offset: 0,
            prev_frame_num: 0,
            prev_ref_frame_num: 0,
            prev_pic_had_mmco5: false,
            max_long_term_frame_idx: None,
            output: VecDeque::new(),
            errors: Vec::new(),
            rec_pool: Vec::new(),
            pending_pts: None,
            concealed_mbs: 0,
            pending_dp: None,
            trace: None,
        }
    }

    /// Number of macroblocks that had to be concealed because of bitstream errors.
    pub fn concealed_macroblocks(&self) -> u64 {
        self.concealed_mbs
    }

    /// The active sequence parameter set, once a picture has been decoded.
    pub fn active_sps(&self) -> Option<&Sps> {
        self.active_sps.as_deref()
    }

    /// Decode one access unit (or any whole number of access units). Returns the frames
    /// that became ready for output, in display order.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Frame>> {
        self.decode_with_pts(data, None)
    }

    /// Like `decode`, attaching `pts` to the picture decoded from `data`.
    pub fn decode_with_pts(&mut self, data: &[u8], pts: Option<i64>) -> Result<Vec<Frame>> {
        self.pending_pts = pts;
        let nals: Vec<&[u8]> = match self.framing {
            Framing::AnnexB => nal::split_annex_b(data),
            Framing::LengthPrefixed(n) => match nal::split_length_prefixed(data, n) {
                Ok(v) => v,
                Err(e) => {
                    // Fall back to Annex-B if the sample carries start codes.
                    if nal::looks_like_annex_b(data) {
                        nal::split_annex_b(data)
                    } else {
                        return Err(e);
                    }
                }
            },
        };
        let mut first_err: Option<Error> = None;
        for n in nals {
            if let Err(e) = self.handle_nal_bytes(n) {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        // An access unit is complete at the end of the buffer.
        self.finish_picture();
        let out: Vec<Frame> = self.output.drain(..).collect();
        if let Some(e) = first_err.or_else(|| self.errors.drain(..).next()) {
            self.errors.clear();
            // Keep the frames for the next call so nothing is lost.
            for f in out.into_iter().rev() {
                self.output.push_front(f);
            }
            return Err(e);
        }
        Ok(out)
    }

    /// Streaming input: decode one NAL unit (header byte first, no start code or length
    /// prefix). Picture boundaries are found from the slice headers (7.4.1.2.4), so the
    /// last picture is only completed by the next picture, an access unit delimiter /
    /// end-of-sequence NAL, or [`Decoder::flush`]. Returns frames ready for output.
    pub fn push_nal(&mut self, nal: &[u8]) -> Result<Vec<Frame>> {
        let mut r = self.handle_nal_bytes(nal);
        if r.is_ok() && !self.errors.is_empty() {
            r = Err(self.errors.remove(0));
            self.errors.clear();
        }
        let out: Vec<Frame> = self.output.drain(..).collect();
        match r {
            Ok(()) => Ok(out),
            Err(e) => {
                for f in out.into_iter().rev() {
                    self.output.push_front(f);
                }
                Err(e)
            }
        }
    }

    /// Output all remaining frames (end of stream).
    pub fn flush(&mut self) -> Vec<Frame> {
        self.finish_picture();
        self.close_pending_field();
        self.bump_all();
        self.output.drain(..).collect()
    }

    fn handle_nal_bytes(&mut self, data: &[u8]) -> Result<()> {
        let nal = Nal::parse(data)?;
        self.handle_nal(&nal)
    }

    fn handle_nal(&mut self, nal: &Nal) -> Result<()> {
        // A partition A slice waits for its B / C partitions, which follow it directly.
        if !matches!(nal.nal_unit_type, nal::NAL_DPB | nal::NAL_DPC) && self.pending_dp.is_some() {
            // An error in the buffered slice must not swallow the current NAL unit.
            if let Err(e) = self.decode_pending_partitions() {
                self.errors.push(e);
            }
        }
        match nal.nal_unit_type {
            nal::NAL_SPS => {
                let sps = Sps::parse(&nal.rbsp)?;
                let id = sps.sps_id as usize;
                self.sps[id] = Some(Arc::new(sps));
                Ok(())
            }
            nal::NAL_PPS => {
                let pps = Pps::parse(&nal.rbsp, &self.sps)?;
                let id = pps.pps_id as usize;
                self.pps[id] = Some(Arc::new(pps));
                Ok(())
            }
            nal::NAL_SLICE | nal::NAL_IDR => self.handle_slice(nal, None),
            nal::NAL_DPA => {
                let (pps, sps) = self.slice_param_sets(&nal.rbsp)?;
                let hdr = parse_slice_header(&nal.rbsp, nal.nal_unit_type, nal.nal_ref_idc, &sps, &pps)?;
                let mut r = BitReader::new(&nal.rbsp);
                r.set_pos(hdr.data_bit_offset);
                let slice_id = r.ue();
                r.check_overrun()?;
                self.pending_dp = Some(PendingDp { a: nal.clone(), slice_id, pps, sps, parts: [None, None] });
                Ok(())
            }
            nal::NAL_DPB | nal::NAL_DPC => {
                let Some(p) = self.pending_dp.as_mut() else {
                    return Ok(()); // orphan partition
                };
                let mut r = BitReader::new(&nal.rbsp);
                let slice_id = r.ue();
                if slice_id != p.slice_id {
                    return Ok(());
                }
                if p.sps.separate_colour_plane {
                    r.read(2);
                }
                if p.pps.redundant_pic_cnt_present {
                    r.ue();
                }
                r.check_overrun()?;
                let k = if nal.nal_unit_type == nal::NAL_DPB { 0 } else { 1 };
                p.parts[k] = Some((nal.rbsp.clone(), r.pos()));
                Ok(())
            }
            nal::NAL_END_SEQ | nal::NAL_END_STREAM | nal::NAL_AUD => {
                self.finish_picture();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn new_picture_needed(&self, h: &SliceHeader) -> bool {
        let Some(cur) = &self.cur else {
            return true;
        };
        let p = &cur.last_hdr;
        let sps = &cur.sps;
        if h.frame_num != p.frame_num
            || h.pps_id != p.pps_id
            || h.field_pic != p.field_pic
            || h.bottom_field != p.bottom_field
            || ((h.nal_ref_idc == 0) != (p.nal_ref_idc == 0))
            || h.idr != p.idr
            || (h.idr && p.idr && h.idr_pic_id != p.idr_pic_id)
        {
            return true;
        }
        if sps.poc_type == 0
            && (h.pic_order_cnt_lsb != p.pic_order_cnt_lsb || h.delta_pic_order_cnt_bottom != p.delta_pic_order_cnt_bottom)
        {
            return true;
        }
        if sps.poc_type == 1 && h.delta_pic_order_cnt != p.delta_pic_order_cnt {
            return true;
        }
        // A slice starting at an already decoded macroblock begins a new picture.
        let first = h.first_mb_in_slice as usize * if sps.mb_adaptive_frame_field && !h.field_pic { 2 } else { 1 };
        cur.info.get(first).map(|i| i.slice != u16::MAX).unwrap_or(true)
    }

    fn slice_param_sets(&self, rbsp: &[u8]) -> Result<(Arc<Pps>, Arc<Sps>)> {
        let pps_id = peek_pps_id(rbsp)?;
        let pps = self.pps[pps_id as usize].clone().ok_or(Error::MissingParameterSet("PPS"))?;
        let sps = self.sps[pps.sps_id as usize].clone().ok_or(Error::MissingParameterSet("SPS"))?;
        sps.check_supported()?;
        Ok((pps, sps))
    }

    /// Decode a buffered partition A slice with whatever B / C partitions arrived.
    fn decode_pending_partitions(&mut self) -> Result<()> {
        match self.pending_dp.take() {
            Some(p) => {
                let parts = [p.parts[0].as_ref().map(|(d, o)| (&d[..], *o)), p.parts[1].as_ref().map(|(d, o)| (&d[..], *o))];
                self.handle_slice(&p.a, Some(parts))
            }
            None => Ok(()),
        }
    }

    fn handle_slice(&mut self, nal: &Nal, partitions: Option<[Option<(&[u8], usize)>; 2]>) -> Result<()> {
        let (pps, sps) = self.slice_param_sets(&nal.rbsp)?;
        let mut hdr = parse_slice_header(&nal.rbsp, nal.nal_unit_type, nal.nal_ref_idc, &sps, &pps)?;
        if partitions.is_some() {
            if pps.entropy_coding_mode {
                return Err(Error::Bitstream("data partitioning with CABAC"));
            }
            // Skip slice_id.
            let mut r = BitReader::new(&nal.rbsp);
            r.set_pos(hdr.data_bit_offset);
            r.ue();
            hdr.data_bit_offset = r.pos();
        }
        if hdr.redundant_pic_cnt > 0 {
            // Primary coded pictures are always decoded; redundant ones are ignored.
            return Ok(());
        }
        if matches!(hdr.slice_type, SliceType::SP | SliceType::SI) {
            return Err(Error::Unsupported("SP/SI slices (Extended profile switching slices)".into()));
        }
        if self.new_picture_needed(&hdr) {
            self.finish_picture();
            self.start_picture(&hdr, &sps)?;
        }
        let cur = self.cur.as_mut().expect("current picture");
        if !Arc::ptr_eq(&cur.sps, &sps) && cur.sps.sps_id != sps.sps_id {
            return Err(Error::Bitstream("slices of one picture use different SPSs"));
        }
        cur.last_hdr = hdr.clone();
        let slice_idx = cur.db_slices.len();
        if slice_idx >= u16::MAX as usize - 1 {
            return Err(Error::Bitstream("too many slices"));
        }
        cur.db_slices.push(DbSlice {
            disable_idc: hdr.disable_deblocking_filter_idc,
            offset_a: hdr.slice_alpha_c0_offset_div2 * 2,
            offset_b: hdr.slice_beta_offset_div2 * 2,
            switching: matches!(hdr.slice_type, SliceType::SP | SliceType::SI),
        });
        let cur_poc = cur.poc();
        let cur_field_poc = [cur.top_poc, cur.bottom_poc];
        let mbaff = matches!(cur.structure, CurStructure::Mbaff);
        // Reference lists.
        let refs = if hdr.slice_type.is_intra() { [Vec::new(), Vec::new()] } else { self.build_ref_lists(&hdr, &sps)? };
        let field_refs = if mbaff && !hdr.slice_type.is_intra() {
            let mk = |p: u8| -> [Vec<RefPic>; 2] {
                let f = |l: &Vec<RefPic>| l.iter().flat_map(|r| [r.field(p), r.field(1 - p)]).collect::<Vec<_>>();
                [f(&refs[0]), f(&refs[1])]
            };
            Some([mk(PAR_TOP), mk(PAR_BOTTOM)])
        } else {
            None
        };
        let weights = Self::pred_weights(&hdr, &pps, &refs, field_refs.as_ref(), cur_poc, cur_field_poc);
        let dequant = Dequant::new(&pps);
        let map = if pps.num_slice_groups > 1 { Some(slice_group_map(&sps, &pps, &hdr)) } else { None };
        let trace = self.trace.is_some();
        let cur = self.cur.as_mut().unwrap();
        let env = SliceEnv {
            sps: &sps,
            pps: &pps,
            hdr: &hdr,
            rbsp: &nal.rbsp,
            slice_idx: slice_idx as u16,
            refs: &refs,
            field_refs: field_refs.as_ref(),
            weights: &weights,
            dequant: &dequant,
            mb_slice_group: map.as_deref(),
            cur_poc,
            cur_field_poc,
            partitions,
            trace,
        };
        if let Some(p) = cur.pipe.as_mut() {
            if partitions.is_some() || map.is_some() {
                p.abandon();
            }
            p.set_slices(&cur.db_slices);
        }
        let st = PicState { info: &mut cur.info, db: &mut cur.db, motion: &mut cur.motion };
        let sink = ReconSink { planes: &mut cur.planes, pipe: cur.pipe.as_mut() };
        let mut dec = match SliceDecoder::new(env, st, sink, &mut self.rec_pool) {
            Ok(d) => d,
            Err(e) => {
                if let Some(p) = self.cur.as_mut().and_then(|c| c.pipe.as_mut()) {
                    p.abandon();
                }
                return Err(e);
            }
        };
        let res = dec.decode();
        if let (Some(t), Some(rec)) = (self.trace.as_mut(), dec.trace.take()) {
            t.push((hdr.data_bit_offset, rec));
        }
        match res {
            Ok(n) => {
                cur.mbs_decoded += n;
                Ok(())
            }
            Err(e) => {
                if let Some(p) = cur.pipe.as_mut() {
                    p.abandon();
                }
                Err(e)
            }
        }
    }

    // ---- picture start ---------------------------------------------------------------

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_pic_id;
        self.next_pic_id = self.next_pic_id.wrapping_add(1).max(1);
        id
    }

    fn start_picture(&mut self, hdr: &SliceHeader, sps: &Arc<Sps>) -> Result<()> {
        // Activate the SPS. A change of picture format flushes the DPB.
        let changed = match &self.active_sps {
            None => true,
            Some(a) => {
                a.pic_width_in_mbs != sps.pic_width_in_mbs
                    || a.frame_height_in_mbs() != sps.frame_height_in_mbs()
                    || a.max_dpb_frames() != sps.max_dpb_frames()
                    || a.chroma_format_idc != sps.chroma_format_idc
                    || a.frame_mbs_only != sps.frame_mbs_only
            }
        };
        if changed {
            if self.active_sps.is_some() {
                self.close_pending_field();
                self.bump_all();
                self.dpb.clear();
            }
        }
        self.active_sps = Some(sps.clone());
        // Second field of a complementary field pair?
        let mut second_of = None;
        if let Some(pf) = self.pending_field {
            if hdr.field_pic && hdr.bottom_field as u8 != pf.parity && hdr.frame_num == pf.frame_num && !hdr.idr {
                if self.dpb.iter().any(|e| e.pic.id == pf.pic_id && !e.complete) {
                    second_of = Some(pf.pic_id);
                }
            }
            if second_of.is_none() {
                self.close_pending_field();
            }
        }
        // frame_num gaps (8.2.5.2).
        if !hdr.idr && second_of.is_none() {
            let max = sps.max_frame_num();
            if hdr.frame_num != self.prev_ref_frame_num && hdr.frame_num != (self.prev_ref_frame_num + 1) % max {
                self.fill_frame_num_gap(hdr, sps);
            }
        }
        let mb_w = sps.pic_width_in_mbs as usize;
        let frame_mb_h = sps.frame_height_in_mbs() as usize;
        let structure = if hdr.field_pic {
            CurStructure::Field(hdr.bottom_field as u8)
        } else if sps.mb_adaptive_frame_field {
            CurStructure::Mbaff
        } else {
            CurStructure::Frame
        };
        let mb_h = if hdr.field_pic { frame_mb_h / 2 } else { frame_mb_h };
        let pic_id = match second_of {
            Some(id) => id,
            None => self.alloc_id(),
        };
        let (top, bottom, msb, fno) = self.compute_poc(hdr, sps);
        let n = mb_w * mb_h;
        self.cur = Some(CurPic {
            planes: Planes::new(mb_w * 16, mb_h * 16, sps.chroma_array_type() != 0),
            info: vec![MbInfo::default(); n],
            db: vec![DbMb { slice: u16::MAX, ..DbMb::default() }; n],
            db_slices: Vec::new(),
            motion: vec![MbMotion::default(); n],
            sps: sps.clone(),
            hdr: hdr.clone(),
            last_hdr: hdr.clone(),
            structure,
            second_of,
            pic_id,
            top_poc: top,
            bottom_poc: bottom,
            poc_msb: msb,
            frame_num_offset: fno,
            mbs_decoded: 0,
            pts: self.pending_pts.take(),
            pipe: if structure == CurStructure::Frame && sps.chroma_array_type() == 1 && pipe_worthwhile(mb_h) {
                DeblockPipe::start(mb_w, mb_h)
            } else {
                None
            },
        });
        Ok(())
    }

    /// Returns (TopFieldOrderCnt, BottomFieldOrderCnt, PicOrderCntMsb, FrameNumOffset);
    /// for field pictures only the field's own count is meaningful (the other equals it).
    fn compute_poc(&self, h: &SliceHeader, sps: &Sps) -> (i32, i32, i32, i32) {
        let max_frame_num = sps.max_frame_num() as i32;
        match sps.poc_type {
            0 => {
                let (prev_msb, prev_lsb) = if h.idr { (0, 0) } else { (self.prev_ref_poc_msb, self.prev_ref_poc_lsb) };
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = h.pic_order_cnt_lsb as i32;
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb + max_lsb
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb - max_lsb
                } else {
                    prev_msb
                };
                let v = msb + lsb;
                if h.field_pic {
                    (v, v, msb, 0)
                } else {
                    (v, v + h.delta_pic_order_cnt_bottom, msb, 0)
                }
            }
            1 => {
                let prev_fno = if self.prev_pic_had_mmco5 { 0 } else { self.prev_frame_num_offset };
                let fno = if h.idr {
                    0
                } else if self.prev_frame_num > h.frame_num {
                    prev_fno + max_frame_num
                } else {
                    prev_fno
                };
                let n = sps.offset_for_ref_frame.len() as i32;
                let mut abs = if n != 0 { fno + h.frame_num as i32 } else { 0 };
                if h.nal_ref_idc == 0 && abs > 0 {
                    abs -= 1;
                }
                let mut expected = 0i64;
                if abs > 0 {
                    let delta_cycle: i64 = sps.offset_for_ref_frame.iter().map(|&v| v as i64).sum();
                    let cycle = (abs - 1) / n;
                    let in_cycle = (abs - 1) % n;
                    expected = cycle as i64 * delta_cycle;
                    for i in 0..=in_cycle as usize {
                        expected += sps.offset_for_ref_frame[i] as i64;
                    }
                }
                if h.nal_ref_idc == 0 {
                    expected += sps.offset_for_non_ref_pic as i64;
                }
                let t2b = sps.offset_for_top_to_bottom_field as i64;
                if !h.field_pic {
                    let top = expected + h.delta_pic_order_cnt[0] as i64;
                    let bottom = top + t2b + h.delta_pic_order_cnt[1] as i64;
                    (top as i32, bottom as i32, 0, fno)
                } else if !h.bottom_field {
                    let top = (expected + h.delta_pic_order_cnt[0] as i64) as i32;
                    (top, top, 0, fno)
                } else {
                    let bottom = (expected + t2b + h.delta_pic_order_cnt[0] as i64) as i32;
                    (bottom, bottom, 0, fno)
                }
            }
            _ => {
                let prev_fno = if self.prev_pic_had_mmco5 { 0 } else { self.prev_frame_num_offset };
                let fno = if h.idr {
                    0
                } else if self.prev_frame_num > h.frame_num {
                    prev_fno + max_frame_num
                } else {
                    prev_fno
                };
                let t = if h.idr {
                    0
                } else if h.nal_ref_idc == 0 {
                    2 * (fno + h.frame_num as i32) - 1
                } else {
                    2 * (fno + h.frame_num as i32)
                };
                (t, t, 0, fno)
            }
        }
    }

    fn fill_frame_num_gap(&mut self, h: &SliceHeader, sps: &Arc<Sps>) {
        let max = sps.max_frame_num();
        let target = h.frame_num;
        let mut fn_ = (self.prev_ref_frame_num + 1) % max;
        let gap = (target + max - fn_) % max;
        let keep = sps.max_num_ref_frames.max(1);
        let mut skipped = 0u32;
        let mb_w = sps.pic_width_in_mbs as usize;
        let mb_h = sps.frame_height_in_mbs() as usize;
        let interlaced = !sps.frame_mbs_only;
        let chroma = sps.chroma_array_type() != 0;
        while fn_ != target {
            // Only the most recent max_num_ref_frames frames can survive the sliding window.
            let create = gap - skipped <= keep;
            let fake = SliceHeader {
                frame_num: fn_,
                nal_ref_idc: 1,
                idr: false,
                field_pic: false,
                delta_pic_order_cnt: [0, 0],
                delta_pic_order_cnt_bottom: 0,
                ..h.clone()
            };
            let (top, bottom, _msb, fno) = self.compute_poc(&fake, sps);
            if create {
                let id = self.alloc_id();
                let mut pic = Picture::new(id, mb_w, mb_h, chroma);
                // Content of non-existing frames is unspecified; use the latest reference.
                if let Some(src) = self.dpb.iter().filter(|e| e.is_ref() && e.complete).max_by_key(|e| e.pic.id) {
                    pic.y.data.copy_from_slice(&src.pic.y.data);
                    pic.cb.data.copy_from_slice(&src.pic.cb.data);
                    pic.cr.data.copy_from_slice(&src.pic.cr.data);
                } else {
                    pic.y.data.fill(128);
                }
                pic.non_existing = true;
                pic.top_poc = top;
                pic.bottom_poc = bottom;
                pic.poc = top.min(bottom);
                pic.motion = vec![MbMotion::default(); mb_w * mb_h];
                if interlaced {
                    pic.make_fields(chroma);
                }
                self.sliding_window(sps, None);
                self.make_room_for_storage(sps);
                self.dpb.push(DpbEntry {
                    pic: Arc::new(pic),
                    frame_num: fn_,
                    marks: [RefMark::Short; 2],
                    has: [true; 2],
                    complete: true,
                    needed_for_output: false,
                    poc: top.min(bottom),
                    pts: None,
                    idr: false,
                    interlaced,
                });
            }
            skipped += 1;
            self.prev_frame_num_offset = fno;
            self.prev_frame_num = fn_;
            self.prev_ref_frame_num = fn_;
            self.prev_pic_had_mmco5 = false;
            fn_ = (fn_ + 1) % max;
            if skipped > max {
                break;
            }
        }
    }

    // ---- reference lists ---------------------------------------------------------------

    fn build_ref_lists(&self, h: &SliceHeader, sps: &Sps) -> Result<[Vec<RefPic>; 2]> {
        let cur = self.cur.as_ref().unwrap();
        let max = sps.max_frame_num();
        let cur_fn = h.frame_num;
        let field = h.field_pic;
        let parity = h.bottom_field as u8;
        let cur_poc = cur.poc();
        let wrap = |e: &DpbEntry| frame_num_wrap(e.frame_num, cur_fn, max);
        // Initial lists as (entry, parity) pairs.
        let mut init: [Vec<(&DpbEntry, u8)>; 2] = [Vec::new(), Vec::new()];
        if !field {
            let mut short: Vec<&DpbEntry> = self.dpb.iter().filter(|e| e.frame_short() && e.complete).collect();
            let mut long: Vec<&DpbEntry> = self.dpb.iter().filter(|e| e.frame_long().is_some() && e.complete).collect();
            long.sort_by_key(|e| e.frame_long().unwrap());
            if h.slice_type.is_p() {
                short.sort_by_key(|e| -wrap(e));
                init[0] = short.iter().chain(long.iter()).map(|e| (*e, PAR_FRAME)).collect();
            } else {
                let usable: Vec<&DpbEntry> = short.iter().copied().filter(|e| !(e.pic.non_existing && sps.poc_type == 0)).collect();
                let mut before: Vec<&DpbEntry> = usable.iter().copied().filter(|e| e.poc <= cur_poc).collect();
                let mut after: Vec<&DpbEntry> = usable.iter().copied().filter(|e| e.poc > cur_poc).collect();
                before.sort_by_key(|e| -e.poc);
                after.sort_by_key(|e| e.poc);
                init[0] = before.iter().chain(after.iter()).chain(long.iter()).map(|e| (*e, PAR_FRAME)).collect();
                init[1] = after.iter().chain(before.iter()).chain(long.iter()).map(|e| (*e, PAR_FRAME)).collect();
            }
        } else {
            let short_marked = |e: &DpbEntry, f: usize| e.has[f] && e.marks[f] == RefMark::Short;
            let long_marked = |e: &DpbEntry, f: usize| e.has[f] && matches!(e.marks[f], RefMark::Long(_));
            let mut short: Vec<&DpbEntry> = self.dpb.iter().filter(|e| e.any_short()).collect();
            let mut long: Vec<&DpbEntry> = self.dpb.iter().filter(|e| e.any_long()).collect();
            long.sort_by_key(|e| e.long_idx().unwrap_or(0));
            let long_fields = alternate_fields(&long, parity, long_marked);
            if h.slice_type.is_p() {
                short.sort_by_key(|e| -wrap(e));
                init[0] = alternate_fields(&short, parity, short_marked);
                init[0].extend(long_fields);
            } else {
                // POC of an entry: of the field marked short-term when only one is (8.2.4.2.4).
                let entry_poc = |e: &DpbEntry| -> i32 {
                    let t = short_marked(e, 0);
                    let b = short_marked(e, 1);
                    match (t, b) {
                        (true, false) => e.pic.top_poc,
                        (false, true) => e.pic.bottom_poc,
                        _ => e.pic.top_poc.min(e.pic.bottom_poc),
                    }
                };
                let usable: Vec<&DpbEntry> = short.iter().copied().filter(|e| !(e.pic.non_existing && sps.poc_type == 0)).collect();
                let mut before: Vec<&DpbEntry> = usable.iter().copied().filter(|e| entry_poc(e) <= cur_poc).collect();
                let mut after: Vec<&DpbEntry> = usable.iter().copied().filter(|e| entry_poc(e) > cur_poc).collect();
                before.sort_by_key(|e| -entry_poc(e));
                after.sort_by_key(|e| entry_poc(e));
                let l0: Vec<&DpbEntry> = before.iter().chain(after.iter()).copied().collect();
                let l1: Vec<&DpbEntry> = after.iter().chain(before.iter()).copied().collect();
                init[0] = alternate_fields(&l0, parity, short_marked);
                init[0].extend(long_fields.iter().copied());
                init[1] = alternate_fields(&l1, parity, short_marked);
                init[1].extend(long_fields);
            }
        }
        if h.slice_type.is_b()
            && init[1].len() > 1
            && init[0].len() == init[1].len()
            && init[0].iter().zip(init[1].iter()).all(|(a, b)| std::ptr::eq(a.0, b.0) && a.1 == b.1)
        {
            init[1].swap(0, 1);
        }
        let to_ref = |e: &DpbEntry, p: u8| RefPic {
            pic: e.pic.clone(),
            parity: p,
            long_term: if p == PAR_FRAME { e.frame_long().is_some() } else { matches!(e.marks[p as usize], RefMark::Long(_)) },
            poc: if p == PAR_FRAME { e.poc } else { e.pic.poc_of(p) },
        };
        // Picture numbers (8.2.4.1).
        let (cur_pic_num, max_pic_num) = if field { (2 * cur_fn as i64 + 1, 2 * max as i64) } else { (cur_fn as i64, max as i64) };
        let pic_num = |e: &DpbEntry, p: u8| -> i64 { if field { 2 * wrap(e) + (p == parity) as i64 } else { wrap(e) } };
        let long_pic_num = |idx: u32, p: u8| -> i64 { if field { 2 * idx as i64 + (p == parity) as i64 } else { idx as i64 } };
        // Candidates for modification.
        let mut short_cands: Vec<(&DpbEntry, u8)> = Vec::new();
        let mut long_cands: Vec<(&DpbEntry, u8, u32)> = Vec::new();
        for e in &self.dpb {
            if field {
                for f in 0..2u8 {
                    if !e.has[f as usize] {
                        continue;
                    }
                    match e.marks[f as usize] {
                        RefMark::Short => short_cands.push((e, f)),
                        RefMark::Long(i) => long_cands.push((e, f, i)),
                        RefMark::Unused => {}
                    }
                }
            } else if e.frame_short() && e.complete {
                short_cands.push((e, PAR_FRAME));
            } else if let Some(i) = e.frame_long() {
                long_cands.push((e, PAR_FRAME, i));
            }
        }
        let lists = if h.slice_type.is_b() { 2 } else { 1 };
        let mut out: [Vec<RefPic>; 2] = [Vec::new(), Vec::new()];
        for l in 0..lists {
            let n = h.num_ref_idx_active[l] as usize;
            let mut list: Vec<Option<(&DpbEntry, u8)>> = init[l].iter().copied().map(Some).collect();
            list.truncate(n);
            list.resize(n, None);
            // Modification (8.2.4.3).
            let mut pred = cur_pic_num;
            let mut ref_idx = 0usize;
            for op in &h.ref_pic_list_modification[l] {
                if ref_idx >= n {
                    return Err(Error::Bitstream("too many ref_pic_list_modification operations"));
                }
                let pick: (&DpbEntry, u8) = match op.idc {
                    0 | 1 => {
                        let abs = op.value as i64 + 1;
                        if abs > max_pic_num {
                            return Err(Error::Bitstream("abs_diff_pic_num_minus1 out of range"));
                        }
                        let mut nw = if op.idc == 0 { pred - abs } else { pred + abs };
                        if op.idc == 0 && nw < 0 {
                            nw += max_pic_num;
                        }
                        if op.idc == 1 && nw >= max_pic_num {
                            nw -= max_pic_num;
                        }
                        pred = nw;
                        let pn = if nw > cur_pic_num { nw - max_pic_num } else { nw };
                        match short_cands.iter().find(|(e, p)| pic_num(e, *p) == pn) {
                            Some(&c) => c,
                            None => return Err(Error::Bitstream("ref list modification: picture not found")),
                        }
                    }
                    _ => match long_cands.iter().find(|(_, p, i)| long_pic_num(*i, *p) == op.value as i64) {
                        Some(&(e, p, _)) => (e, p),
                        None => return Err(Error::Bitstream("ref list modification: long-term picture not found")),
                    },
                };
                list.insert(ref_idx, Some(pick));
                ref_idx += 1;
                // Remove a later duplicate of the same picture.
                let mut k = ref_idx;
                while k < list.len() {
                    if let Some((e, p)) = list[k] {
                        if std::ptr::eq(e, pick.0) && p == pick.1 {
                            list.remove(k);
                            continue;
                        }
                    }
                    k += 1;
                }
                list.truncate(n + 1);
            }
            list.truncate(n);
            // Missing entries: conceal with the first available reference.
            let fallback = list
                .iter()
                .flatten()
                .next()
                .copied()
                .or_else(|| init[l].first().copied())
                .or_else(|| self.dpb.iter().filter(|e| e.is_ref()).last().map(|e| (e, if field { e.has.iter().position(|&x| x).unwrap_or(0) as u8 } else { PAR_FRAME })));
            for e in list {
                match e.or(fallback) {
                    Some((e, p)) => out[l].push(to_ref(e, p)),
                    None => return Err(Error::Bitstream("no reference pictures available")),
                }
            }
        }
        Ok(out)
    }

    fn implicit_table(r0s: &[RefPic], r1s: &[RefPic], cur_poc: i32) -> Vec<(i32, i32)> {
        let mut t = Vec::with_capacity(r0s.len() * r1s.len());
        for r0 in r0s {
            for r1 in r1s {
                let diff = r1.poc - r0.poc;
                let ws = if r0.long_term || r1.long_term || diff == 0 {
                    (32, 32)
                } else {
                    let td = diff.clamp(-128, 127);
                    let tb = (cur_poc - r0.poc).clamp(-128, 127);
                    let tx = (16384 + (td / 2).abs()) / td;
                    let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                    if (dsf >> 2) < -64 || (dsf >> 2) > 128 { (32, 32) } else { (64 - (dsf >> 2), dsf >> 2) }
                };
                t.push(ws);
            }
        }
        t
    }

    fn pred_weights(
        h: &SliceHeader,
        pps: &Pps,
        refs: &[Vec<RefPic>; 2],
        field_refs: Option<&[[Vec<RefPic>; 2]; 2]>,
        cur_poc: i32,
        cur_field_poc: [i32; 2],
    ) -> PredWeights {
        let mut w = PredWeights::default();
        if h.slice_type.is_p() && pps.weighted_pred {
            w.mode = 1;
        } else if h.slice_type.is_b() && pps.weighted_bipred_idc == 1 {
            w.mode = 1;
        } else if h.slice_type.is_b() && pps.weighted_bipred_idc == 2 {
            w.mode = 2;
        }
        if w.mode == 1 {
            if let Some(t) = &h.pred_weight {
                w.luma_log2 = t.luma_log2_denom;
                w.chroma_log2 = t.chroma_log2_denom;
                w.explicit = [t.l0.clone(), t.l1.clone()];
            }
        } else if w.mode == 2 {
            w.n1 = refs[1].len();
            w.implicit = Self::implicit_table(&refs[0], &refs[1], cur_poc);
            if let Some(fr) = field_refs {
                for p in 0..2 {
                    w.implicit_field[p] = Self::implicit_table(&fr[p][0], &fr[p][1], cur_field_poc[p]);
                }
            }
        }
        w
    }

    // ---- picture finish ----------------------------------------------------------------

    fn finish_picture(&mut self) {
        if self.pending_dp.is_some() {
            if let Err(e) = self.decode_pending_partitions() {
                self.errors.push(e);
            }
        }
        let Some(mut cur) = self.cur.take() else {
            return;
        };
        let sps = cur.sps.clone();
        let chroma = sps.chroma_array_type() != 0;
        let total = cur.info.len();
        let mut pipe = cur.pipe.take();
        if cur.mbs_decoded < total {
            if let Some(p) = pipe.as_mut() {
                p.abandon();
            }
            self.conceal(&mut cur);
        }
        let mb_w = sps.pic_width_in_mbs as usize;
        let mb_h = total / mb_w.max(1);
        if let Some(planes) = pipe.and_then(|p| p.finish()) {
            // Deblocked on the helper thread while the slices were decoded.
            cur.planes = planes;
        } else {
            let ctx = DeblockCtx {
                mbs: &cur.db,
                motion: &cur.motion,
                slices: &cur.db_slices,
                mb_w,
                mb_h,
                mbaff: cur.structure == CurStructure::Mbaff,
                field_pic: matches!(cur.structure, CurStructure::Field(_)),
                chroma,
            };
            deblock_picture(&mut cur.planes, &ctx);
        }
        cur.planes.extend_borders();
        let hdr = cur.hdr.clone();
        let mmco5 = hdr.has_mmco5();
        let is_ref = hdr.nal_ref_idc != 0;
        let parity = cur.parity();
        let mut frame_num = hdr.frame_num;
        // MMCO 5: POC relative to this picture (8.2.1).
        if mmco5 {
            match cur.structure {
                CurStructure::Field(PAR_TOP) => cur.top_poc = 0,
                CurStructure::Field(_) => cur.bottom_poc = 0,
                _ => {
                    let t = cur.top_poc.min(cur.bottom_poc);
                    cur.top_poc -= t;
                    cur.bottom_poc -= t;
                }
            }
        }
        // C.4.4: IDR / mmco5 handling of earlier pictures.
        if hdr.idr || mmco5 {
            for e in self.dpb.iter_mut() {
                if cur.second_of != Some(e.pic.id) {
                    e.marks = [RefMark::Unused; 2];
                }
            }
            if hdr.idr && hdr.no_output_of_prior_pics {
                let keep = cur.second_of;
                self.dpb.retain(|e| Some(e.pic.id) == keep);
            } else {
                self.bump_all_except(cur.second_of);
            }
            self.dpb.retain(|e| e.needed_for_output || e.is_ref() || Some(e.pic.id) == cur.second_of);
        }
        // Reference marking (8.2.5).
        let mut mark = RefMark::Unused;
        if is_ref {
            let first_mark =
                cur.second_of.and_then(|id| self.dpb.iter().find(|e| e.pic.id == id)).map(|e| e.marks[1 - parity.min(1) as usize]);
            if hdr.idr {
                if hdr.long_term_reference_flag {
                    mark = RefMark::Long(0);
                    self.max_long_term_frame_idx = Some(0);
                } else {
                    mark = RefMark::Short;
                    self.max_long_term_frame_idx = None;
                }
            } else {
                let mut long_idx = None;
                if hdr.adaptive_ref_pic_marking {
                    long_idx = self.apply_mmcos(&hdr, &sps, &cur);
                } else if !(cur.second_of.is_some() && first_mark == Some(RefMark::Short)) {
                    self.sliding_window(&sps, cur.second_of);
                }
                mark = match long_idx {
                    Some(i) => RefMark::Long(i),
                    None => RefMark::Short,
                };
            }
            // Keep within max_num_ref_frames (non-conforming streams).
            let max_refs = sps.max_num_ref_frames.max(1) as usize;
            while self.dpb.iter().filter(|e| e.is_ref() && Some(e.pic.id) != cur.second_of).count() >= max_refs {
                let pick = |short_only: bool| {
                    self.dpb
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| Some(e.pic.id) != cur.second_of)
                        .filter(|(_, e)| if short_only { e.any_short() } else { e.is_ref() })
                        .min_by_key(|(_, e)| e.pic.id)
                        .map(|(i, _)| i)
                };
                match pick(true).or_else(|| pick(false)) {
                    Some(i) => self.dpb[i].marks = [RefMark::Unused; 2],
                    None => break,
                }
            }
        }
        if mmco5 {
            frame_num = 0;
        }
        // POC / frame_num state for following pictures.
        if is_ref {
            if mmco5 {
                self.prev_ref_poc_msb = 0;
                self.prev_ref_poc_lsb = if parity == PAR_BOTTOM { 0 } else { cur.top_poc };
            } else {
                self.prev_ref_poc_msb = cur.poc_msb;
                self.prev_ref_poc_lsb = hdr.pic_order_cnt_lsb as i32;
            }
            self.prev_ref_frame_num = frame_num;
        }
        self.prev_frame_num_offset = cur.frame_num_offset;
        self.prev_frame_num = frame_num;
        self.prev_pic_had_mmco5 = mmco5;
        // Store and output (C.4.5).
        self.dpb.retain(|e| e.needed_for_output || e.is_ref() || Some(e.pic.id) == cur.second_of);
        match cur.structure {
            CurStructure::Field(par) => self.store_field(cur, par, mark, frame_num, &sps),
            _ => self.store_frame(cur, mark, frame_num, &sps),
        }
    }

    fn store_frame(&mut self, cur: CurPic, mark: RefMark, frame_num: u32, sps: &Arc<Sps>) {
        let chroma = sps.chroma_array_type() != 0;
        let mb_w = sps.pic_width_in_mbs as usize;
        let mb_h = sps.frame_height_in_mbs() as usize;
        let mut pic = Picture::new(cur.pic_id, mb_w, mb_h, false);
        pic.y = cur.planes.y;
        pic.cb = cur.planes.cb;
        pic.cr = cur.planes.cr;
        pic.top_poc = cur.top_poc;
        pic.bottom_poc = cur.bottom_poc;
        pic.poc = cur.top_poc.min(cur.bottom_poc);
        pic.structure = if cur.structure == CurStructure::Mbaff { Structure::Mbaff } else { Structure::Frame };
        pic.motion = cur.motion;
        let interlaced = !sps.frame_mbs_only;
        if interlaced {
            pic.make_fields(chroma);
        }
        let poc = pic.poc;
        let entry = DpbEntry {
            pic: Arc::new(pic),
            frame_num,
            marks: [mark; 2],
            has: [true; 2],
            complete: true,
            needed_for_output: true,
            poc,
            pts: cur.pts,
            idr: cur.hdr.idr,
            interlaced: cur.structure == CurStructure::Mbaff,
        };
        self.store_entry(entry, mark != RefMark::Unused, sps);
    }

    fn store_field(&mut self, cur: CurPic, par: u8, mark: RefMark, frame_num: u32, sps: &Arc<Sps>) {
        let chroma = sps.chroma_array_type() != 0;
        let p = par as usize;
        let field_poc = if par == PAR_TOP { cur.top_poc } else { cur.bottom_poc };
        if let Some(id) = cur.second_of {
            if let Some(i) = self.dpb.iter().position(|e| e.pic.id == id) {
                let mut e = self.dpb.remove(i);
                let mut pic = match Arc::try_unwrap(e.pic) {
                    Ok(p) => p,
                    Err(arc) => clone_picture(&arc),
                };
                pic.y.put_field(&cur.planes.y, par);
                if chroma {
                    pic.cb.put_field(&cur.planes.cb, par);
                    pic.cr.put_field(&cur.planes.cr, par);
                }
                pic.extend_borders();
                if par == PAR_TOP {
                    pic.top_poc = field_poc;
                } else {
                    pic.bottom_poc = field_poc;
                }
                pic.poc = pic.top_poc.min(pic.bottom_poc);
                pic.field_motion[p] = cur.motion;
                pic.fields[p] = Some(Arc::new(cur.planes));
                e.poc = pic.poc;
                e.pic = Arc::new(pic);
                e.marks[p] = mark;
                e.has[p] = true;
                e.complete = true;
                self.pending_field = None;
                // The frame buffer already holds the first field.
                self.dpb.push(e);
                self.reorder_limit_output(sps);
                return;
            }
        }
        // First field: allocate the frame buffer.
        let mb_w = sps.pic_width_in_mbs as usize;
        let mb_h = sps.frame_height_in_mbs() as usize;
        let mut pic = Picture::new(cur.pic_id, mb_w, mb_h, chroma);
        pic.y.put_field(&cur.planes.y, par);
        pic.y.put_field(&cur.planes.y, 1 - par);
        if chroma {
            pic.cb.put_field(&cur.planes.cb, par);
            pic.cb.put_field(&cur.planes.cb, 1 - par);
            pic.cr.put_field(&cur.planes.cr, par);
            pic.cr.put_field(&cur.planes.cr, 1 - par);
        }
        pic.extend_borders();
        pic.top_poc = field_poc;
        pic.bottom_poc = field_poc;
        pic.poc = field_poc;
        pic.structure = Structure::Fields;
        pic.field_motion[p] = cur.motion.clone();
        pic.field_motion[1 - p] = cur.motion;
        pic.fields[p] = Some(Arc::new(cur.planes));
        let mut has = [false; 2];
        has[p] = true;
        let mut marks = [RefMark::Unused; 2];
        marks[p] = mark;
        let entry = DpbEntry {
            pic: Arc::new(pic),
            frame_num,
            marks,
            has,
            complete: false,
            needed_for_output: true,
            poc: field_poc,
            pts: cur.pts,
            idr: cur.hdr.idr,
            interlaced: true,
        };
        self.pending_field = Some(PendingField { pic_id: cur.pic_id, parity: par, frame_num });
        self.store_entry(entry, mark != RefMark::Unused, sps);
    }

    /// A first field without second field: complete the frame by repeating the field.
    fn close_pending_field(&mut self) {
        let Some(pf) = self.pending_field.take() else {
            return;
        };
        let Some(e) = self.dpb.iter_mut().find(|e| e.pic.id == pf.pic_id && !e.complete) else {
            return;
        };
        if let Some(pic) = Arc::get_mut(&mut e.pic) {
            let f = pic.fields[pf.parity as usize].clone();
            pic.fields[1 - pf.parity as usize] = f;
        }
        e.complete = true;
        if let Some(sps) = self.active_sps.clone() {
            self.reorder_limit_output(&sps);
        }
    }

    fn store_entry(&mut self, entry: DpbEntry, is_ref: bool, sps: &Sps) {
        let dpb_size = sps.max_dpb_frames();
        let poc = entry.poc;
        if !is_ref && entry.complete {
            // C.4.5.2: a non-reference picture is output directly when it precedes everything
            // waiting and the DPB is full.
            while self.dpb.len() >= dpb_size {
                let min_waiting = self.dpb.iter().filter(|e| e.needed_for_output && e.complete).map(|e| e.poc).min();
                match min_waiting {
                    Some(m) if m < poc => self.bump_one(sps),
                    _ => break,
                }
            }
            if self.dpb.len() >= dpb_size {
                let f = crop_frame(&entry, sps);
                self.output.push_back(f);
                self.reorder_limit_output(sps);
                return;
            }
        } else {
            self.make_room_for_storage(sps);
        }
        self.dpb.push(entry);
        self.reorder_limit_output(sps);
    }

    /// Output frames while more than max_num_reorder_frames are waiting (VUI), which yields
    /// the same order as the bumping process but with lower latency.
    fn reorder_limit_output(&mut self, sps: &Sps) {
        let limit = if sps.vui_present && sps.vui.bitstream_restriction {
            sps.vui.max_num_reorder_frames as usize
        } else {
            sps.max_dpb_frames()
        };
        while self.dpb.iter().filter(|e| e.needed_for_output && e.complete).count() > limit {
            self.bump_one(sps);
        }
    }

    /// Bump until there is an empty frame buffer.
    fn make_room_for_storage(&mut self, sps: &Sps) {
        let size = sps.max_dpb_frames();
        loop {
            self.dpb.retain(|e| e.needed_for_output || e.is_ref() || !e.complete);
            if self.dpb.len() < size {
                break;
            }
            if !self.dpb.iter().any(|e| e.needed_for_output && e.complete) {
                // DPB full of references (non-conforming): drop the oldest short-term one.
                if let Some(i) = self
                    .dpb
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.any_short() && e.complete)
                    .min_by_key(|(_, e)| e.pic.id)
                    .map(|(i, _)| i)
                {
                    self.dpb.remove(i);
                    continue;
                }
                break;
            }
            self.bump_one(sps);
        }
    }

    /// Output the frame with the smallest POC (C.4.5.3).
    fn bump_one(&mut self, sps: &Sps) {
        let Some((i, _)) =
            self.dpb.iter().enumerate().filter(|(_, e)| e.needed_for_output && e.complete).min_by_key(|(_, e)| e.poc)
        else {
            return;
        };
        let f = crop_frame(&self.dpb[i], sps);
        self.output.push_back(f);
        self.dpb[i].needed_for_output = false;
        if !self.dpb[i].is_ref() {
            self.dpb.remove(i);
        }
    }

    fn bump_all(&mut self) {
        self.bump_all_except(None);
    }

    fn bump_all_except(&mut self, keep: Option<u32>) {
        let Some(sps) = self.active_sps.clone() else {
            return;
        };
        loop {
            let Some((i, _)) = self
                .dpb
                .iter()
                .enumerate()
                .filter(|(_, e)| e.needed_for_output && Some(e.pic.id) != keep && (e.complete || keep.is_none()))
                .min_by_key(|(_, e)| e.poc)
            else {
                break;
            };
            let f = crop_frame(&self.dpb[i], &sps);
            self.output.push_back(f);
            self.dpb[i].needed_for_output = false;
            if !self.dpb[i].is_ref() {
                self.dpb.remove(i);
            }
        }
    }

    /// Sliding window marking (8.2.5.3). `exclude` is the frame of the current second field.
    fn sliding_window(&mut self, sps: &Sps, exclude: Option<u32>) {
        let max = sps.max_num_ref_frames.max(1) as usize;
        let num_short = self.dpb.iter().filter(|e| e.any_short() && Some(e.pic.id) != exclude).count();
        let num_long = self.dpb.iter().filter(|e| e.any_long() && !e.any_short() && Some(e.pic.id) != exclude).count();
        if num_short + num_long >= max && num_short > 0 {
            // The short-term entry with the smallest FrameNumWrap is the earliest decoded one.
            let victim = self
                .dpb
                .iter()
                .enumerate()
                .filter(|(_, e)| e.any_short() && Some(e.pic.id) != exclude)
                .min_by_key(|(_, e)| e.pic.id)
                .map(|(i, _)| i);
            if let Some(i) = victim {
                for m in self.dpb[i].marks.iter_mut() {
                    if *m == RefMark::Short {
                        *m = RefMark::Unused;
                    }
                }
            }
        }
    }

    /// Adaptive memory control (8.2.5.4) for frames and fields. Returns the long-term
    /// index for the current picture when MMCO 6 assigns one.
    fn apply_mmcos(&mut self, h: &SliceHeader, sps: &Sps, cur: &CurPic) -> Option<u32> {
        let max = sps.max_frame_num();
        let cur_fn = h.frame_num;
        let field = h.field_pic;
        let parity = h.bottom_field as usize;
        let cur_pic_num = if field { 2 * cur_fn as i64 + 1 } else { cur_fn as i64 };
        let this_frame = cur.second_of;
        // (entry index, field) of a short-term picture with PicNum == x.
        let find_short = |dpb: &[DpbEntry], x: i64| -> Option<(usize, Option<usize>)> {
            for (i, e) in dpb.iter().enumerate() {
                let w = frame_num_wrap(e.frame_num, cur_fn, max);
                if field {
                    for f in 0..2 {
                        if e.has[f] && e.marks[f] == RefMark::Short && 2 * w + (f == parity) as i64 == x {
                            return Some((i, Some(f)));
                        }
                    }
                } else if e.frame_short() && w == x {
                    return Some((i, None));
                }
            }
            None
        };
        let mut cur_long = None;
        for m in &h.mmcos {
            match m.op {
                1 => {
                    let x = cur_pic_num - (m.difference_of_pic_nums_minus1 as i64 + 1);
                    if let Some((i, f)) = find_short(&self.dpb, x) {
                        match f {
                            Some(f) => self.dpb[i].marks[f] = RefMark::Unused,
                            None => self.dpb[i].marks = [RefMark::Unused; 2],
                        }
                    }
                }
                2 => {
                    let n = m.long_term_pic_num as i64;
                    for e in self.dpb.iter_mut() {
                        for f in 0..2 {
                            if let RefMark::Long(i) = e.marks[f] {
                                let lpn = if field { 2 * i as i64 + (f == parity) as i64 } else { i as i64 };
                                if lpn == n && (field || e.frame_long().is_some()) {
                                    e.marks[f] = RefMark::Unused;
                                }
                            }
                        }
                    }
                }
                3 => {
                    let x = cur_pic_num - (m.difference_of_pic_nums_minus1 as i64 + 1);
                    let idx = m.long_term_frame_idx;
                    if let Some((t, f)) = find_short(&self.dpb, x) {
                        for (i, e) in self.dpb.iter_mut().enumerate() {
                            if i != t && e.marks.contains(&RefMark::Long(idx)) {
                                e.marks = [RefMark::Unused; 2];
                            }
                        }
                        // Within the same frame, a field holding another index loses it.
                        match f {
                            Some(f) => {
                                let other = 1 - f;
                                if let RefMark::Long(o) = self.dpb[t].marks[other] {
                                    if o != idx {
                                        self.dpb[t].marks[other] = RefMark::Unused;
                                    }
                                }
                                self.dpb[t].marks[f] = RefMark::Long(idx);
                            }
                            None => self.dpb[t].marks = [RefMark::Long(idx); 2],
                        }
                    }
                }
                4 => {
                    let plus1 = m.max_long_term_frame_idx_plus1;
                    self.max_long_term_frame_idx = if plus1 == 0 { None } else { Some(plus1 - 1) };
                    for e in self.dpb.iter_mut() {
                        for f in 0..2 {
                            if let RefMark::Long(i) = e.marks[f] {
                                if plus1 == 0 || i > plus1 - 1 {
                                    e.marks[f] = RefMark::Unused;
                                }
                            }
                        }
                    }
                }
                5 => {
                    for e in self.dpb.iter_mut() {
                        if Some(e.pic.id) != this_frame {
                            e.marks = [RefMark::Unused; 2];
                        } else {
                            e.marks = [RefMark::Unused; 2];
                        }
                    }
                    self.max_long_term_frame_idx = None;
                }
                6 => {
                    let idx = m.long_term_frame_idx;
                    for e in self.dpb.iter_mut() {
                        if Some(e.pic.id) == this_frame {
                            // The first field of this frame may carry the same index.
                            for f in 0..2 {
                                if let RefMark::Long(o) = e.marks[f] {
                                    if o != idx {
                                        e.marks[f] = RefMark::Unused;
                                    }
                                }
                            }
                            continue;
                        }
                        if e.marks.contains(&RefMark::Long(idx)) {
                            e.marks = [RefMark::Unused; 2];
                        }
                    }
                    cur_long = Some(idx);
                }
                _ => {}
            }
        }
        cur_long
    }

    /// Fill macroblocks no slice decoded with the co-located samples of the most recent
    /// reference picture (or grey).
    fn conceal(&mut self, cur: &mut CurPic) {
        let src = self
            .dpb
            .iter()
            .filter(|e| e.is_ref() && !e.pic.non_existing && e.complete)
            .max_by_key(|e| e.pic.id)
            .map(|e| e.pic.clone());
        let parity = cur.parity();
        let mb_w = cur.sps.pic_width_in_mbs as usize;
        let mbaff = cur.structure == CurStructure::Mbaff;
        for addr in 0..cur.info.len() {
            if cur.info[addr].slice != u16::MAX {
                continue;
            }
            self.concealed_mbs += 1;
            let (mx, my) = if mbaff {
                let p = addr / 2;
                (p % mb_w, (p / mb_w) * 2 + addr % 2)
            } else {
                (addr % mb_w, addr / mb_w)
            };
            let srcp = src.as_ref().map(|p| p.planes(parity));
            for (k, size) in [(0usize, 16usize), (1, 8), (2, 8)] {
                let dst = match k {
                    0 => &mut cur.planes.y,
                    1 => &mut cur.planes.cb,
                    _ => &mut cur.planes.cr,
                };
                if dst.width < (mx + 1) * size || dst.height < (my + 1) * size {
                    // Monochrome pictures carry dummy chroma planes.
                    continue;
                }
                let s = srcp.map(|(y, cb, cr)| match k {
                    0 => y,
                    1 => cb,
                    _ => cr,
                });
                for y in 0..size {
                    let o = dst.idx((mx * size) as isize, (my * size + y) as isize);
                    match s {
                        Some(sp) if sp.width == dst.width && sp.height == dst.height => {
                            dst.data[o..o + size].copy_from_slice(&sp.data[o..o + size]);
                        }
                        _ => dst.data[o..o + size].fill(128),
                    }
                }
            }
        }
    }
}

fn clone_picture(p: &Picture) -> Picture {
    Picture {
        id: p.id,
        mb_width: p.mb_width,
        mb_height: p.mb_height,
        y: p.y.clone(),
        cb: p.cb.clone(),
        cr: p.cr.clone(),
        fields: p.fields.clone(),
        poc: p.poc,
        top_poc: p.top_poc,
        bottom_poc: p.bottom_poc,
        non_existing: p.non_existing,
        structure: p.structure,
        motion: p.motion.clone(),
        field_motion: p.field_motion.clone(),
    }
}

/// Testing aids (not part of the stable API).
#[doc(hidden)]
pub mod testing {
    use super::*;

    struct Bits(Vec<bool>);

    impl Bits {
        fn push_range(&mut self, data: &[u8], start: usize, end: usize) {
            for p in start..end {
                self.0.push(data[p / 8] >> (7 - p % 8) & 1 == 1);
            }
        }
        fn ue(&mut self, v: u32) {
            let v = v as u64 + 1;
            let len = 64 - v.leading_zeros();
            for _ in 0..len - 1 {
                self.0.push(false);
            }
            for i in (0..len).rev() {
                self.0.push((v >> i) & 1 == 1);
            }
        }
        fn align_zero(&mut self) {
            while self.0.len() % 8 != 0 {
                self.0.push(false);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            self.0.push(true);
            self.align_zero();
            let mut out = vec![0u8; self.0.len() / 8];
            for (i, &b) in self.0.iter().enumerate() {
                if b {
                    out[i / 8] |= 0x80 >> (i % 8);
                }
            }
            out
        }
    }

    fn escape(header: u8, rbsp: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, header];
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

    /// Re-package every non-IDR CAVLC slice of an Annex-B stream into data partitions A, B
    /// and C (7.3.2.9, 7.3.2.10). The result decodes to the same pictures. Streams using
    /// constrained intra prediction are not supported (their nC derivation differs).
    pub fn partition_stream(data: &[u8]) -> Result<Vec<u8>> {
        let mut dec = Decoder::new_annex_b();
        dec.trace = Some(Vec::new());
        let mut out = Vec::new();
        let mut slice_id = 0u32;
        for n in nal::split_annex_b(data) {
            let parsed = Nal::parse(n)?;
            let before = dec.trace.as_ref().unwrap().len();
            dec.push_nal(n)?;
            let is_slice = matches!(parsed.nal_unit_type, nal::NAL_SLICE);
            let trace = dec.trace.as_ref().unwrap();
            if !is_slice || trace.len() == before {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(n);
                continue;
            }
            let (data_off, ranges) = trace[before].clone();
            let rbsp = &parsed.rbsp;
            // End of slice data: the rbsp_stop_one_bit.
            let mut last = rbsp.len();
            while last > 0 && rbsp[last - 1] == 0 {
                last -= 1;
            }
            let stop = (last - 1) * 8 + 7 - rbsp[last - 1].trailing_zeros() as usize;
            let (pps, _) = dec.slice_param_sets(rbsp)?;
            let mut a = Bits(Vec::new());
            a.push_range(rbsp, 0, data_off);
            a.ue(slice_id);
            let mut b = Bits(Vec::new());
            let mut c = Bits(Vec::new());
            for part in [&mut b, &mut c] {
                part.ue(slice_id);
                if pps.redundant_pic_cnt_present {
                    part.ue(0);
                }
            }
            let mut pos = data_off;
            for &(s, e, kind) in &ranges {
                // Category 2 data up to the residual / PCM range.
                a.push_range(rbsp, pos, s);
                if kind == 2 {
                    // pcm_alignment_zero_bits are category 3: re-align inside partition B.
                    b.align_zero();
                    b.push_range(rbsp, (s + 7) & !7, e);
                } else {
                    let dst = if kind == 1 { &mut c } else { &mut b };
                    dst.push_range(rbsp, s, e);
                }
                pos = e;
            }
            a.push_range(rbsp, pos, stop);
            let hdr = n[0];
            out.extend(escape((hdr & 0x60) | 2, &a.finish()));
            if ranges.iter().any(|r| r.2 != 1) {
                out.extend(escape((hdr & 0x60) | 3, &b.finish()));
            }
            if ranges.iter().any(|r| r.2 == 1) {
                out.extend(escape((hdr & 0x60) | 4, &c.finish()));
            }
            slice_id += 1;
        }
        Ok(out)
    }
}

/// Overlapping deblocking with decoding pays off for pictures of at least this many
/// macroblock rows on machines with more than one core.
fn pipe_worthwhile(mb_h: usize) -> bool {
    mb_h >= 8 && std::thread::available_parallelism().map(|n| n.get() > 1).unwrap_or(false)
}
