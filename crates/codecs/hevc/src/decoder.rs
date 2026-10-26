// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Top-level decoder: NAL dispatch, picture start/finish, DPB management.

use std::sync::Arc;

use crate::bitreader::nal_to_rbsp;
use crate::ctu::{PicState, SliceDecoder};
use crate::deblock::deblock_picture;
use crate::dpb::{compute_poc, Dpb, GenParams, RpsSets};
use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::nal::{self, NalHeader};
use crate::params::{parse_pps, parse_sps, parse_vps, Pps, ScalingFactors, ScalingList, Sps, TileLayout};
use crate::picture::{Picture, RefMark, SliceRefs};
use crate::pool::Pool;
use crate::sao::sao_picture;
use crate::slice::{parse_slice_header, peek_pps_id, SliceHeader};

struct CurrentPicture {
    pic: Picture,
    st: PicState,
    headers: Vec<SliceHeader>,
    sps: Arc<Sps>,
    pps: Arc<Pps>,
    layout: Arc<TileLayout>,
    scaling: Option<Arc<ScalingFactors>>,
    rps: RpsIds,
    output_flag: bool,
    last_independent: Option<usize>,
}

/// RPS sets expressed with stable picture identifiers (decode_order).
#[derive(Default, Clone)]
struct RpsIds {
    before: Vec<u64>,
    after: Vec<u64>,
    lt: Vec<u64>,
}

#[cfg(feature = "synth")]
type SynthResult = Option<Box<crate::cabac::SynthBins>>;
#[cfg(not(feature = "synth"))]
type SynthResult = Option<()>;

struct LayoutCache {
    sps: Arc<Sps>,
    pps: Arc<Pps>,
    layout: Arc<TileLayout>,
    scaling: Option<Arc<ScalingFactors>>,
}

/// H.265 / HEVC decoder (Main and Main 10 profiles, 4:2:0).
pub struct Decoder {
    sps: Vec<Option<Arc<Sps>>>,
    pps: Vec<Option<Arc<Pps>>>,
    nal_length_size: usize,
    dpb: Dpb,
    pending: Vec<Frame>,
    prev_tid0_poc: i32,
    first_picture: bool,
    after_eos: bool,
    skip_rasl: bool,
    skipping_picture: bool,
    cur: Option<CurrentPicture>,
    decode_counter: u64,
    active_sps: Option<Arc<Sps>>,
    cache: Option<LayoutCache>,
    st_pool: Option<PicState>,
    sao_scratch: Vec<u16>,
    /// Worker threads for picture-level parallelism (created on first use).
    pool: Option<Pool>,
    /// Number of threads to use (1 = single-threaded).
    threads: usize,
    /// Pictures with fewer luma samples are decoded single-threaded.
    mt_min_samples: usize,
}

/// Default minimum picture size (luma samples) for multi-threaded decoding.
const MT_MIN_SAMPLES: usize = 640 * 360;

impl Decoder {
    /// Creates a decoder from parameter set NAL units (e.g. the hvcC arrays)
    /// and the NAL unit length field size of the samples (1, 2 or 4).
    pub fn new<T: AsRef<[u8]>>(parameter_sets: &[T], nal_length_size: usize) -> Result<Decoder> {
        if !(1..=4).contains(&nal_length_size) {
            return Err(Error::Invalid("nal_length_size must be 1..=4"));
        }
        let mut d = Decoder {
            sps: vec![None; 16],
            pps: vec![None; 64],
            nal_length_size,
            dpb: Dpb::default(),
            pending: Vec::new(),
            prev_tid0_poc: 0,
            first_picture: true,
            after_eos: false,
            skip_rasl: false,
            skipping_picture: false,
            cur: None,
            decode_counter: 0,
            active_sps: None,
            cache: None,
            st_pool: None,
            sao_scratch: Vec::new(),
            pool: None,
            threads: crate::pool::default_threads(),
            mt_min_samples: MT_MIN_SAMPLES,
        };
        for ps in parameter_sets {
            d.decode_nal(ps.as_ref())?;
        }
        Ok(d)
    }

    /// Creates a decoder from an hvcC (HEVCDecoderConfigurationRecord) box payload.
    pub fn from_hvcc(hvcc: &[u8]) -> Result<Decoder> {
        let (len, sets) = crate::parse_hvcc(hvcc)?;
        Decoder::new(&sets, len)
    }

    /// Decodes one access unit of length-prefixed NAL units (one MP4 sample)
    /// and returns the frames that became ready for output, in display order.
    pub fn decode(&mut self, access_unit: &[u8]) -> Result<Vec<Frame>> {
        let r = self.guard(|d| {
            let nals = nal::split_length_prefixed(access_unit, d.nal_length_size)?;
            let mut first_err: Option<Error> = None;
            for n in nals {
                if let Err(e) = d.decode_nal(n) {
                    first_err.get_or_insert(e);
                }
            }
            d.finish_picture();
            match first_err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        });
        match r {
            Ok(()) => Ok(std::mem::take(&mut self.pending)),
            Err(e) => Err(e),
        }
    }

    /// Decodes an Annex B byte stream chunk containing whole NAL units. The
    /// last picture is completed by the next picture, `end_of_picture` or `flush`.
    pub fn decode_annexb(&mut self, data: &[u8]) -> Result<Vec<Frame>> {
        let r = self.guard(|d| {
            let mut first_err: Option<Error> = None;
            for n in nal::split_annexb(data) {
                if let Err(e) = d.decode_nal(n) {
                    first_err.get_or_insert(e);
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        });
        r.map(|_| std::mem::take(&mut self.pending))
    }

    /// Completes the picture currently being decoded (for NAL-level feeding).
    pub fn end_of_picture(&mut self) -> Vec<Frame> {
        let _ = self.guard(|d| {
            d.finish_picture();
            Ok(())
        });
        std::mem::take(&mut self.pending)
    }

    /// Outputs all remaining frames (end of stream).
    pub fn flush(&mut self) -> Vec<Frame> {
        let _ = self.guard(|d| {
            d.finish_picture();
            let mut out = Vec::new();
            d.dpb.flush(&mut out);
            d.pending.extend(out);
            Ok(())
        });
        self.first_picture = true;
        std::mem::take(&mut self.pending)
    }

    /// Runs `f`, converting any internal panic into an error and resetting state.
    fn guard(&mut self, f: impl FnOnce(&mut Decoder) -> Result<()>) -> Result<()> {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        match r {
            Ok(r) => r,
            Err(p) => {
                let msg = if let Some(s) = p.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = p.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "panic".to_string()
                };
                self.cur = None;
                self.dpb.clear();
                self.first_picture = true;
                Err(Error::Internal(msg))
            }
        }
    }

    /// Decodes a single NAL unit (without start code / length prefix).
    pub fn decode_nal(&mut self, data: &[u8]) -> Result<()> {
        let hdr = NalHeader::parse(data)?;
        if hdr.layer_id != 0 {
            return Ok(());
        }
        match hdr.nal_type {
            nal::VPS_NUT => {
                let rbsp = nal_to_rbsp(data);
                parse_vps(&rbsp)?;
            }
            nal::SPS_NUT => {
                let rbsp = nal_to_rbsp(data);
                let s = parse_sps(&rbsp)?;
                let id = s.id as usize;
                self.sps[id] = Some(Arc::new(s));
            }
            nal::PPS_NUT => {
                let rbsp = nal_to_rbsp(data);
                let p = parse_pps(&rbsp)?;
                let id = p.id as usize;
                self.pps[id] = Some(Arc::new(p));
            }
            nal::EOS_NUT => {
                self.finish_picture();
                self.after_eos = true;
            }
            nal::AUD_NUT => self.finish_picture(),
            0..=9 | 16..=21 => self.decode_slice(&hdr, data)?,
            _ => {}
        }
        Ok(())
    }

    fn layout_for(&mut self, sps: &Arc<Sps>, pps: &Arc<Pps>) -> Result<(Arc<TileLayout>, Option<Arc<ScalingFactors>>)> {
        if let Some(c) = &self.cache {
            if Arc::ptr_eq(&c.sps, sps) && Arc::ptr_eq(&c.pps, pps) {
                return Ok((c.layout.clone(), c.scaling.clone()));
            }
        }
        let layout = Arc::new(TileLayout::new(sps, pps)?);
        let scaling = if sps.scaling_list_enabled {
            let list = if let Some(l) = &pps.scaling_list {
                l.clone()
            } else if let Some(l) = &sps.scaling_list {
                l.clone()
            } else {
                ScalingList::default_lists()
            };
            Some(Arc::new(list.factors()))
        } else {
            None
        };
        self.cache = Some(LayoutCache { sps: sps.clone(), pps: pps.clone(), layout: layout.clone(), scaling: scaling.clone() });
        Ok((layout, scaling))
    }

    fn decode_slice(&mut self, hdr: &NalHeader, data: &[u8]) -> Result<()> {
        if hdr.nal_type > nal::CRA_NUT || (10..=15).contains(&hdr.nal_type) {
            return Ok(()); // reserved
        }
        let rbsp = nal_to_rbsp(data);
        self.slice_common(hdr, &rbsp, None, Some(data)).map(|_| ())
    }

    /// Bitstream synthesis: parses the slice segment header in `header_nal`
    /// (a NAL unit containing only the header) and generates random slice
    /// segment data for `ctus` CTUs. Returns the slice data RBSP bytes and the
    /// byte offsets (within it) at which substreams 1.. start.
    #[cfg(feature = "synth")]
    pub fn synth_slice(&mut self, header_nal: &[u8], seed: u64, ctus: usize) -> Result<(Vec<u8>, Vec<usize>)> {
        let hdr = NalHeader::parse(header_nal)?;
        let rbsp = nal_to_rbsp(header_nal);
        let s = self.slice_common(&hdr, &rbsp, Some((seed, ctus)), None)?;
        let s = s.ok_or(Error::Invalid("synth: picture skipped"))?;
        let bytes = s.enc.bytes();
        let starts = s.substream_starts.iter().map(|b| b / 8).collect();
        Ok((bytes, starts))
    }

    #[cfg(feature = "synth")]
    pub fn synth_dpb_pocs(&self) -> Vec<(i32, bool, bool)> {
        self.dpb.pics.iter().map(|p| (p.poc, p.mark != RefMark::Unused, p.mark == RefMark::LongTerm)).collect()
    }

    #[cfg(not(feature = "synth"))]
    fn slice_common(&mut self, hdr: &NalHeader, rbsp: &[u8], _synth: Option<(u64, usize)>, nal: Option<&[u8]>) -> Result<Option<()>> {
        self.slice_common_impl(hdr, rbsp, _synth, nal).map(|_| None)
    }

    #[cfg(feature = "synth")]
    fn slice_common(
        &mut self,
        hdr: &NalHeader,
        rbsp: &[u8],
        synth: Option<(u64, usize)>,
        nal: Option<&[u8]>,
    ) -> Result<Option<Box<crate::cabac::SynthBins>>> {
        self.slice_common_impl(hdr, rbsp, synth, nal)
    }

    #[allow(clippy::type_complexity)]
    fn slice_common_impl(
        &mut self,
        hdr: &NalHeader,
        rbsp: &[u8],
        _synth: Option<(u64, usize)>,
        nal: Option<&[u8]>,
    ) -> Result<SynthResult> {
        let (first, pps_id) = peek_pps_id(rbsp, hdr)?;
        if first {
            self.finish_picture();
            self.skipping_picture = false;
        } else if self.skipping_picture {
            return Ok(None);
        }
        let pps = self.pps[pps_id as usize].clone().ok_or(Error::MissingParameterSet("PPS"))?;
        let sps = self.sps[pps.sps_id as usize].clone().ok_or(Error::MissingParameterSet("SPS"))?;
        if sps.chroma_format_idc != 1 {
            return Err(Error::Unsupported("chroma formats other than 4:2:0"));
        }
        if sps.bit_depth_luma > 12 || sps.bit_depth_chroma > 12 {
            return Err(Error::Unsupported("bit depth > 12"));
        }
        if !first && self.cur.is_none() {
            return Err(Error::Invalid("slice segment without first slice of picture"));
        }
        let prev = self.cur.as_ref().and_then(|c| c.last_independent.map(|i| &c.headers[i]));
        let sh = parse_slice_header(rbsp, hdr, &sps, &pps, prev)?;
        if first {
            self.start_picture(hdr, &sh, &sps, &pps)?;
            if self.skipping_picture {
                return Ok(None);
            }
        }
        let (layout, _) = self.layout_for(&sps, &pps)?;
        // Wavefront rows are decoded in parallel when the stream allows it.
        let mt = nal.is_some() && _synth.is_none() && pps.entropy_coding_sync_enabled && self.use_threads(&sps);
        let pool = self.pool.as_ref();
        let dpb = &self.dpb;
        let cur = match self.cur.as_mut() {
            Some(c) => c,
            None => return Ok(None),
        };
        if !Arc::ptr_eq(&cur.sps, &sps) {
            return Err(Error::Invalid("SPS changed within a picture"));
        }
        if cur.headers.len() >= u16::MAX as usize - 1 {
            return Err(Error::Invalid("too many slice segments"));
        }
        // Reference picture lists
        let sets = RpsSets {
            st_curr_before: ids_to_idx(dpb, &cur.rps.before),
            st_curr_after: ids_to_idx(dpb, &cur.rps.after),
            lt_curr: ids_to_idx(dpb, &cur.rps.lt),
        };
        let lists = dpb.ref_lists(&sets, &sh)?;
        let mut refs = SliceRefs::default();
        for l in 0..2 {
            for i in 0..sh.num_ref_idx_active[l] {
                if let Some(p) = dpb.pics.get(lists[l][i]) {
                    refs.poc[l][i] = p.poc;
                    refs.is_lt[l][i] = p.mark == RefMark::LongTerm;
                }
            }
        }
        let hdr_idx = cur.headers.len();
        if !sh.dependent_slice_segment {
            cur.last_independent = Some(hdr_idx);
        }
        cur.headers.push(sh);
        cur.pic.slice_refs.push(refs);
        let CurrentPicture { pic, st, headers, pps: cpps, scaling, .. } = cur;
        let sh = &headers[hdr_idx];
        if let (true, Some(nal), Some(pool)) = (mt, nal, pool) {
            let job = crate::wpp::SegmentJob {
                sps: &sps,
                pps: cpps,
                sh,
                layout: &layout,
                scaling: scaling.as_deref(),
                rbsp,
                nal,
                dpb: &dpb.pics,
                ref_dpb: lists,
                hdr_idx: hdr_idx as u16,
            };
            if let Some(r) = crate::wpp::decode_segment(&job, pool, pic, st) {
                if r.is_err() {
                    pic.corrupt = true;
                }
                r?;
                return Ok(None);
            }
        }
        let mut sd = SliceDecoder::new(
            &sps,
            cpps,
            sh,
            &layout,
            scaling.as_deref(),
            rbsp,
            pic,
            st,
            &dpb.pics,
            lists,
            hdr_idx as u16,
        );
        #[cfg(feature = "synth")]
        if let Some((seed, ctus)) = _synth {
            sd.cabac.synth = Some(Box::new(crate::cabac::SynthBins::new(seed, ctus)));
        }
        let r = sd.decode();
        #[cfg(feature = "synth")]
        let out = sd.cabac.synth.take();
        let sc = sd.sc;
        st.set_scalars(&sc);
        if r.is_err() {
            pic.corrupt = true;
        }
        r?;
        #[cfg(feature = "synth")]
        return Ok(out);
        #[cfg(not(feature = "synth"))]
        Ok(Some(()))
    }

    fn start_picture(&mut self, hdr: &NalHeader, sh: &SliceHeader, sps: &Arc<Sps>, pps: &Arc<Pps>) -> Result<()> {
        let irap = hdr.is_irap();
        let no_rasl = irap && (hdr.is_idr() || hdr.is_bla() || self.first_picture || self.after_eos);
        if irap {
            self.skip_rasl = no_rasl;
        }
        if hdr.is_rasl() && self.skip_rasl {
            self.skipping_picture = true;
            return Ok(());
        }
        if self.first_picture && !irap {
            // Decoding starts at a non-IRAP picture: references will be generated.
        }
        // A new SPS takes effect at IRAP pictures: output everything decoded so far.
        if let Some(a) = &self.active_sps {
            if !Arc::ptr_eq(a, sps) && (a.pic_width != sps.pic_width || a.pic_height != sps.pic_height || a.bit_depth_luma != sps.bit_depth_luma || a.bit_depth_chroma != sps.bit_depth_chroma) {
                let mut out = Vec::new();
                self.dpb.flush(&mut out);
                self.pending.extend(out);
            }
        }
        self.active_sps = Some(sps.clone());
        let poc = if hdr.is_idr() {
            0
        } else {
            compute_poc(sh.poc_lsb, sps.log2_max_poc_lsb, self.prev_tid0_poc, no_rasl)
        };
        if hdr.temporal_id == 0 && !hdr.is_rasl() && !hdr.is_radl() && !hdr.is_sub_layer_non_ref() {
            self.prev_tid0_poc = poc;
        }
        let genp = GenParams {
            width: sps.pic_width,
            height: sps.pic_height,
            bit_depth_luma: sps.bit_depth_luma,
            bit_depth_chroma: sps.bit_depth_chroma,
            log2_ctb: sps.log2_ctb,
            crop: sps.conf_win,
        };
        // Generated pictures get fresh ids.
        let before_len = self.dpb.pics.len();
        let sets = self.dpb.apply_rps(sh, poc, sps.log2_max_poc_lsb, no_rasl, hdr.is_idr(), &genp);
        for p in self.dpb.pics[before_len..].iter_mut() {
            self.decode_counter += 1;
            p.decode_order = self.decode_counter;
        }
        let ids = RpsIds {
            before: sets.st_curr_before.iter().map(|&i| self.dpb.pics[i].decode_order).collect(),
            after: sets.st_curr_after.iter().map(|&i| self.dpb.pics[i].decode_order).collect(),
            lt: sets.lt_curr.iter().map(|&i| self.dpb.pics[i].decode_order).collect(),
        };
        // Output and removal of pictures from the DPB (C.5.2.2)
        let htid = sps.max_sub_layers_minus1;
        if no_rasl && !self.first_picture {
            // C.5.2.2: a CRA with NoRaslOutputFlag (i.e. after an end of
            // sequence) always discards prior pictures that were not output.
            let discard = hdr.is_cra() || sh.no_output_of_prior_pics;
            if discard {
                self.dpb.clear();
            } else {
                let mut out = Vec::new();
                self.dpb.flush(&mut out);
                self.pending.extend(out);
            }
        } else {
            self.dpb.remove_unused();
            loop {
                let needed = self.dpb.num_needed_for_output();
                let reorder = needed > sps.max_num_reorder[htid] as usize;
                let latency = self.latency_exceeded(sps);
                let full = self.dpb.pics.len() >= sps.max_dec_pic_buffering[htid] as usize;
                if !(reorder || latency || full) {
                    break;
                }
                match self.dpb.bump() {
                    Some(f) => self.pending.push(f),
                    None => break,
                }
            }
        }
        let mut pic = self.dpb.take_picture(&genp);
        pic.poc = poc;
        self.decode_counter += 1;
        pic.decode_order = self.decode_counter;
        pic.slice_refs.clear();
        pic.full_range = sps.vui.video_full_range_flag;
        pic.matrix_coeffs = sps.vui.matrix_coeffs;
        let (layout, scaling) = self.layout_for(sps, pps)?;
        self.cur = Some(CurrentPicture {
            pic,
            st: match self.st_pool.take() {
                Some(mut st) if st.matches(sps) => {
                    st.reset();
                    st
                }
                _ => PicState::new(sps),
            },
            headers: Vec::new(),
            sps: sps.clone(),
            pps: pps.clone(),
            layout,
            scaling,
            rps: ids,
            output_flag: sh.pic_output_flag,
            last_independent: None,
        });
        self.first_picture = false;
        self.after_eos = false;
        Ok(())
    }

    /// Whether pictures of this SPS are decoded multi-threaded; creates the
    /// thread pool on first use.
    fn use_threads(&mut self, sps: &Sps) -> bool {
        if self.threads <= 1 || sps.pic_width * sps.pic_height < self.mt_min_samples {
            return false;
        }
        if self.pool.is_none() {
            self.pool = Some(Pool::new(self.threads));
        }
        true
    }

    /// Test hook: sets the thread count and the minimum picture size for
    /// multi-threaded decoding.
    #[cfg(test)]
    pub(crate) fn set_threading(&mut self, threads: usize, min_samples: usize) {
        self.threads = threads.max(1);
        self.mt_min_samples = min_samples;
        self.pool = None;
    }

    fn latency_exceeded(&self, sps: &Sps) -> bool {
        let htid = sps.max_sub_layers_minus1;
        let plus1 = sps.max_latency_increase_plus1[htid];
        if plus1 == 0 {
            return false;
        }
        let max_latency = sps.max_num_reorder[htid] + plus1 - 1;
        self.dpb.pics.iter().any(|p| p.needed_for_output && p.latency >= max_latency)
    }

    /// Conceals CTBs that were not decoded by copying from the closest reference picture.
    fn conceal(&self, cur: &mut CurrentPicture) {
        let sps = &cur.sps;
        let nctb = sps.pic_size_in_ctbs();
        if cur.st.ctbs_decoded >= nctb {
            return;
        }
        cur.pic.corrupt = true;
        let src = self
            .dpb
            .pics
            .iter()
            .filter(|p| p.width() == cur.pic.width() && p.height() == cur.pic.height())
            .min_by_key(|p| (p.poc - cur.pic.poc).abs());
        let ctb = sps.ctb_size;
        for rs in 0..nctb {
            if cur.st.ctb_hdr[rs] != u16::MAX {
                continue;
            }
            let x0 = (rs % sps.pic_width_in_ctbs) * ctb;
            let y0 = (rs / sps.pic_width_in_ctbs) * ctb;
            let w4 = cur.pic.w4;
            for y4 in (y0 >> 2)..((y0 + ctb).min(sps.pic_height).div_ceil(4)) {
                for x4 in (x0 >> 2)..((x0 + ctb).min(sps.pic_width).div_ceil(4)) {
                    cur.pic.mvf[y4 * w4 + x4] = crate::picture::MvField::default();
                }
            }
            for c in 0..3 {
                let s = if c == 0 { 0 } else { 1 };
                let plane = &mut cur.pic.planes[c];
                let (xs, ys) = (x0 >> s, y0 >> s);
                let xe = ((x0 + ctb) >> s).min(plane.width);
                let ye = ((y0 + ctb) >> s).min(plane.height);
                for y in ys..ye {
                    for x in xs..xe {
                        let v = match src {
                            Some(p) => p.planes[c].get(x, y),
                            None => 1 << ((if c == 0 { sps.bit_depth_luma } else { sps.bit_depth_chroma }) - 1),
                        };
                        plane.set(x, y, v);
                    }
                }
            }
        }
    }

    fn finish_picture(&mut self) {
        let mut cur = match self.cur.take() {
            Some(c) => c,
            None => return,
        };
        self.conceal(&mut cur);
        let CurrentPicture { mut pic, st, headers, sps, pps, layout, output_flag, .. } = cur;
        debug_dump(&pic, "pre-deblock");
        let dbg = crate::debug::opts();
        let pool = if self.use_threads(&sps) { self.pool.as_ref() } else { None };
        if !dbg.skip_deblock {
            deblock_picture(&mut pic, &st, &sps, &pps, &headers, &layout, pool);
        }
        debug_dump(&pic, "pre-sao");
        if sps.sao_enabled && !dbg.skip_sao {
            sao_picture(&mut pic, &st, &sps, &pps, &headers, &layout, &mut self.sao_scratch, pool);
        }
        self.st_pool = Some(st);
        // C.5.2.3
        let htid = sps.max_sub_layers_minus1;
        if output_flag {
            for p in self.dpb.pics.iter_mut() {
                if p.needed_for_output && p.poc > pic.poc {
                    p.latency += 1;
                }
            }
        }
        pic.needed_for_output = output_flag;
        pic.latency = 0;
        pic.mark = RefMark::ShortTerm;
        self.dpb.pics.push(pic);
        loop {
            let needed = self.dpb.num_needed_for_output();
            if !(needed > sps.max_num_reorder[htid] as usize || self.latency_exceeded(&sps)) {
                break;
            }
            match self.dpb.bump() {
                Some(f) => self.pending.push(f),
                None => break,
            }
        }
    }
}

fn ids_to_idx(dpb: &Dpb, ids: &[u64]) -> Vec<usize> {
    ids.iter().filter_map(|id| dpb.pics.iter().position(|p| p.decode_order == *id)).collect()
}

/// STUDIO_HEVC_DUMP_REGION="poc,c,x0,y0,x1,y1" prints sample values of a region.
fn debug_dump(pic: &Picture, stage: &str) {
    if let Some(f) = crate::debug::opts().dump_region {
        if f[0] == pic.poc as i64 {
            let p = &pic.planes[f[1] as usize];
            eprintln!("{stage} poc {} c {}", pic.poc, f[1]);
            for y in f[3]..f[5] {
                let row: Vec<String> = (f[2]..f[4]).map(|x| format!("{:4}", p.get(x as usize, y as usize))).collect();
                eprintln!("{y:4}: {}", row.join(""));
            }
        }
    }
}
