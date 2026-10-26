// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slice data decoding (7.3.4, 7.3.5): the macroblock loop (including MBAFF macroblock
//! pairs), macroblock layer parsing for CAVLC and CABAC, neighbour derivation (6.4.12) and
//! dispatch to prediction and reconstruction.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::bitreader::BitReader;
use crate::cabac::Cabac;
use crate::deblock::DbMb;
use crate::error::{Error, Result, check};
use crate::mb::*;
use crate::params::{Pps, Sps};
use crate::picture::{MbMotion, PAR_FRAME, Picture};
use crate::recon::{MbRec, ReconEnv, ReconSink};
use crate::slice::{SliceHeader, SliceType, WeightEntry};
use crate::tables::chroma_qp;
use crate::transform::{LevelScale4, LevelScale8, level_scale4, level_scale8};

/// A reference picture (frame or field) as seen by a slice.
#[derive(Clone)]
pub struct RefPic {
    pub pic: Arc<Picture>,
    /// PAR_TOP, PAR_BOTTOM or PAR_FRAME.
    pub parity: u8,
    pub long_term: bool,
    pub poc: i32,
}

impl RefPic {
    /// Identity of the referenced frame or field.
    #[inline]
    pub fn code(&self) -> i32 {
        ref_code(self.pic.id, self.parity)
    }

    /// The field of this frame reference with the given parity.
    pub fn field(&self, parity: u8) -> RefPic {
        RefPic { pic: self.pic.clone(), parity, long_term: self.long_term, poc: self.pic.poc_of(parity) }
    }
}

#[inline]
pub fn ref_code(id: u32, parity: u8) -> i32 {
    ((id & 0x1fff_ffff) as i32) * 4 + parity as i32
}

/// Weighted prediction mode of a slice.
#[derive(Clone, Default)]
pub struct PredWeights {
    /// 0 = default, 1 = explicit, 2 = implicit.
    pub mode: u32,
    pub luma_log2: u32,
    pub chroma_log2: u32,
    pub explicit: [Vec<WeightEntry>; 2],
    /// Implicit weights (w0, w1) indexed by refIdxL0 * n1 + refIdxL1 (slice lists).
    pub implicit: Vec<(i32, i32)>,
    pub n1: usize,
    /// MBAFF field macroblocks: implicit weights per parity over the field lists.
    pub implicit_field: [Vec<(i32, i32)>; 2],
}

/// Dequantisation tables for the 6 4x4 lists and the 2 (luma) 8x8 lists.
pub struct Dequant {
    pub ls4: [LevelScale4; 6],
    pub ls8: [LevelScale8; 2],
}

impl Dequant {
    pub fn new(pps: &Pps) -> Box<Dequant> {
        let s = &pps.scaling;
        Box::new(Dequant {
            ls4: [
                level_scale4(&s.m4[0]),
                level_scale4(&s.m4[1]),
                level_scale4(&s.m4[2]),
                level_scale4(&s.m4[3]),
                level_scale4(&s.m4[4]),
                level_scale4(&s.m4[5]),
            ],
            ls8: [level_scale8(&s.m8[0]), level_scale8(&s.m8[1])],
        })
    }
}

/// Everything a slice needs from the decoder.
pub struct SliceEnv<'a> {
    pub sps: &'a Sps,
    pub pps: &'a Pps,
    pub hdr: &'a SliceHeader,
    pub rbsp: &'a [u8],
    pub slice_idx: u16,
    /// RefPicList0/1 of the slice (fields for field pictures).
    pub refs: &'a [Vec<RefPic>; 2],
    /// MBAFF: field reference lists for field macroblocks, [parity][list].
    pub field_refs: Option<&'a [[Vec<RefPic>; 2]; 2]>,
    pub weights: &'a PredWeights,
    pub dequant: &'a Dequant,
    /// FMO: slice group of every macroblock (None with a single slice group).
    pub mb_slice_group: Option<&'a [u8]>,
    /// POC of the current picture (frame POC, or the field's POC for field pictures).
    pub cur_poc: i32,
    /// POCs of the current frame's top and bottom field (MBAFF field macroblocks).
    pub cur_field_poc: [i32; 2],
    /// Data partitioning: RBSP and slice_data() start bit of partitions B and C
    /// (`Some` for a partition A slice; inner `None` when that partition is absent).
    pub partitions: Option<[Option<(&'a [u8], usize)>; 2]>,
    /// Record the bit ranges of category 3/4 data (testing aid for data partitioning).
    pub trace: bool,
}

/// Mutable per-picture state.
pub struct PicState<'a> {
    pub info: &'a mut [MbInfo],
    pub db: &'a mut [DbMb],
    pub motion: &'a mut [MbMotion],
}

/// Pictures with at least this many macroblocks (and rows) are reconstructed on a
/// separate thread while the entropy decoder runs ahead.
const RECON_THREAD_MIN_MBS: usize = 396;
const RECON_THREAD_MIN_ROWS: usize = 8;

/// Reference index marking an unavailable neighbour partition.
pub(crate) const REF_NA: i8 = -2;

pub struct SliceDecoder<'a> {
    pub(crate) env: SliceEnv<'a>,
    pub(crate) st: PicState<'a>,
    /// Reconstruction target (None while it is lent to the reconstruction thread).
    pub(crate) sink: Option<ReconSink<'a>>,
    /// Reconstruct on a separate thread (progressive pictures of sufficient size).
    pub(crate) threaded: bool,
    /// Record of the macroblock being parsed.
    pub(crate) rec: Box<MbRec>,
    /// Spare records (kept by the decoder across slices).
    pub(crate) rec_pool: &'a mut Vec<Box<MbRec>>,
    /// Threaded reconstruction: records waiting to be sent, the channel to the
    /// reconstruction thread and the channel returning processed records.
    pub(crate) batch: Vec<Box<MbRec>>,
    pub(crate) recon_tx: Option<Sender<Vec<Box<MbRec>>>>,
    pub(crate) recon_ret: Option<Receiver<Vec<Box<MbRec>>>>,
    pub(crate) br: BitReader<'a>,
    pub(crate) cabac: Option<Cabac<'a>>,
    /// Data partitioning: category 3 (intra residual, PCM) and category 4 (inter residual).
    pub(crate) dp: bool,
    pub(crate) br_b: Option<BitReader<'a>>,
    pub(crate) br_c: Option<BitReader<'a>>,
    /// Bit ranges of residual / PCM sample data: (start, end, kind) with kind 0 = intra
    /// residual, 1 = inter residual, 2 = PCM samples.
    pub(crate) trace: Option<Vec<(usize, usize, u8)>>,
    /// Picture size in macroblocks (field macroblocks for field pictures).
    pub(crate) mb_w: usize,
    pub(crate) mb_h: usize,
    pub(crate) chroma: bool,
    pub(crate) slice_type: SliceType,
    pub(crate) mbaff: bool,
    pub(crate) field_pic: bool,
    /// Parity of a field picture (0 top, 1 bottom).
    pub(crate) bottom_field: u8,
    pub(crate) qp: i32,
    pub(crate) last_qp_delta: i32,
    // Current macroblock.
    pub(crate) addr: usize,
    /// Macroblock column (MBAFF: pair column).
    pub(crate) mbx: usize,
    /// Macroblock row (MBAFF: pair row).
    pub(crate) mby: usize,
    /// MBAFF: bottom macroblock of its pair.
    pub(crate) is_bottom: bool,
    /// Current macroblock is a field macroblock (field picture or MBAFF field pair).
    pub(crate) mb_field: bool,
    pub(crate) mb_a: Option<usize>,
    pub(crate) mb_b: Option<usize>,
    pub(crate) mb_c: Option<usize>,
    pub(crate) mb_d: Option<usize>,
    /// MBAFF: top macroblock addresses of the available neighbouring pairs.
    pub(crate) pair_a: Option<usize>,
    pub(crate) pair_b: Option<usize>,
    pub(crate) pair_c: Option<usize>,
    pub(crate) pair_d: Option<usize>,
    /// Sample views of the current macroblock in the target planes: origin and row step.
    pub(crate) lx0: usize,
    pub(crate) ly0: usize,
    pub(crate) cx0: usize,
    pub(crate) cy0: usize,
    pub(crate) vstep: usize,
    pub(crate) cur: MbInfo,
    pub(crate) kind: MbKind,
    pub(crate) transform_8x8: bool,
    /// Bit per luma4x4BlkIdx (or per 8x8 block for 8x8 transforms) with non-zero coefficients.
    pub(crate) luma_nz: u16,
    /// Bit c * 4 + blk: chroma block has non-zero coefficients (AC or DC).
    pub(crate) chroma_nz: u8,
    /// Intra prediction modes (4x4: per luma4x4BlkIdx; 8x8: per 8x8 block in [0..4]).
    pub(crate) pred_modes: [u8; 16],
    // Motion of the current macroblock.
    /// Per 4x4 raster block, per list.
    pub(crate) mb_mv: [[[i16; 2]; 16]; 2],
    /// Per 8x8 block, per list (-1 unused).
    pub(crate) mb_ref: [[i8; 4]; 2],
    /// Per list: 4x4 blocks (raster bits) whose motion is already derived (6.4.11.7).
    pub(crate) decoded: [u16; 2],
    /// Parsed reference indices (for CABAC ref_idx contexts) per 8x8 block.
    pub(crate) parsed_ref: [[i8; 4]; 2],
    pub(crate) sub_types: [SubType; 4],
    /// MBAFF + CABAC: skip flag of the bottom macroblock decoded ahead (top skipped).
    pub(crate) next_skip: Option<bool>,
    /// MBAFF + CABAC: mb_field_decoding_flag of the bottom macroblock decoded ahead.
    pub(crate) next_field: Option<bool>,
}

impl<'a> SliceDecoder<'a> {
    pub fn new(
        env: SliceEnv<'a>,
        st: PicState<'a>,
        sink: ReconSink<'a>,
        rec_pool: &'a mut Vec<Box<MbRec>>,
    ) -> Result<SliceDecoder<'a>> {
        let hdr = env.hdr;
        let sps = env.sps;
        let pps = env.pps;
        let mb_w = sps.pic_width_in_mbs as usize;
        let field_pic = hdr.field_pic;
        let mb_h = sps.frame_height_in_mbs() as usize / if field_pic { 2 } else { 1 };
        let mbaff = sps.mb_adaptive_frame_field && !field_pic;
        let qp = hdr.slice_qp(pps);
        let mut br = BitReader::new(env.rbsp);
        br.set_pos(hdr.data_bit_offset);
        let cabac = if pps.entropy_coding_mode {
            br.align();
            let init = if hdr.slice_type.is_intra() { 3 } else { hdr.cabac_init_idc as usize };
            Some(Cabac::new(env.rbsp, br.pos(), qp, init)?)
        } else {
            None
        };
        let chroma = sps.chroma_array_type() != 0;
        let mk = |p: Option<(&'a [u8], usize)>| {
            p.map(|(d, pos)| {
                let mut r = BitReader::new(d);
                r.set_pos(pos);
                r
            })
        };
        let (dp, br_b, br_c) = match env.partitions {
            Some([b, c]) => (true, mk(b), mk(c)),
            None => (false, None, None),
        };
        let trace = if env.trace { Some(Vec::new()) } else { None };
        let threaded = !mbaff
            && mb_w * mb_h >= RECON_THREAD_MIN_MBS
            && mb_h >= RECON_THREAD_MIN_ROWS
            && std::thread::available_parallelism().map(|n| n.get() > 1).unwrap_or(false);
        let rec = rec_pool.pop().unwrap_or_else(MbRec::boxed);
        Ok(SliceDecoder {
            sink: Some(sink),
            threaded,
            rec,
            rec_pool,
            batch: Vec::new(),
            recon_tx: None,
            recon_ret: None,
            trace,
            dp,
            br_b,
            br_c,
            slice_type: hdr.slice_type,
            mbaff,
            field_pic,
            bottom_field: hdr.bottom_field as u8,
            env,
            st,
            br,
            cabac,
            mb_w,
            mb_h,
            chroma,
            qp,
            last_qp_delta: 0,
            addr: 0,
            mbx: 0,
            mby: 0,
            is_bottom: false,
            mb_field: field_pic,
            mb_a: None,
            mb_b: None,
            mb_c: None,
            mb_d: None,
            pair_a: None,
            pair_b: None,
            pair_c: None,
            pair_d: None,
            lx0: 0,
            ly0: 0,
            cx0: 0,
            cy0: 0,
            vstep: 1,
            cur: MbInfo::default(),
            kind: MbKind::PSkip,
            transform_8x8: false,
            luma_nz: 0,
            chroma_nz: 0,
            pred_modes: [0; 16],
            mb_mv: [[[0; 2]; 16]; 2],
            mb_ref: [[-1; 4]; 2],
            decoded: [0; 2],
            parsed_ref: [[-1; 4]; 2],
            sub_types: [SubType { num: 1, w: 2, h: 2, pred: 0 }; 4],
            next_skip: None,
            next_field: None,
        })
    }

    fn next_mb_addr(&self, n: usize) -> usize {
        let total = self.mb_w * self.mb_h;
        if n + 1 >= total {
            return total;
        }
        match self.env.mb_slice_group {
            None => n + 1,
            Some(map) => {
                if self.mbaff && n % 2 == 0 {
                    return n + 1;
                }
                let g = map[n];
                let mut i = n + 1;
                while i < total && map[i] != g {
                    i += if self.mbaff { 2 } else { 1 };
                }
                i.min(total)
            }
        }
    }

    /// Decode all macroblocks of the slice. Returns the number of macroblocks decoded.
    pub fn decode(&mut self) -> Result<usize> {
        let res = if self.threaded { self.decode_threaded() } else { self.decode_macroblocks() };
        // Keep the records for the next slice.
        self.rec_pool.append(&mut self.batch);
        res
    }

    fn decode_macroblocks(&mut self) -> Result<usize> {
        if self.cabac.is_some() { self.decode_cabac_slice() } else { self.decode_cavlc_slice() }
    }

    /// Entropy decoding on this thread, reconstruction (and the deblocking pipe) on a
    /// scoped helper thread fed with macroblock records in decoding order.
    fn decode_threaded(&mut self) -> Result<usize> {
        let mut sink = self.sink.take().expect("reconstruction sink");
        let env = self.recon_env();
        let (tx, rx) = channel::<Vec<Box<MbRec>>>();
        let (ret_tx, ret_rx) = channel::<Vec<Box<MbRec>>>();
        std::thread::scope(|s| {
            let worker = std::thread::Builder::new()
                .name("h264-recon".into())
                .spawn_scoped(s, move || {
                    for batch in rx {
                        for rec in &batch {
                            sink.run(rec, &env);
                        }
                        let _ = ret_tx.send(batch);
                    }
                    sink
                })
                .expect("failed to spawn the reconstruction thread");
            self.recon_tx = Some(tx);
            self.recon_ret = Some(ret_rx);
            let res = self.decode_macroblocks();
            self.flush_batch();
            self.recon_tx = None;
            let sink = match worker.join() {
                Ok(sink) => sink,
                Err(p) => std::panic::resume_unwind(p),
            };
            self.sink = Some(sink);
            if let Some(ret) = self.recon_ret.take() {
                for mut b in ret.try_iter() {
                    self.rec_pool.append(&mut b);
                }
            }
            res
        })
    }

    pub(crate) fn recon_env(&self) -> ReconEnv<'a> {
        ReconEnv { refs: self.env.refs, field_refs: self.env.field_refs, weights: self.env.weights, chroma: self.chroma }
    }

    /// Reconstruct `self.rec` (filled by `submit_mb`).
    pub(crate) fn dispatch_mb(&mut self) {
        if let Some(sink) = self.sink.as_mut() {
            let env = ReconEnv { refs: self.env.refs, field_refs: self.env.field_refs, weights: self.env.weights, chroma: self.chroma };
            sink.run(&self.rec, &env);
            return;
        }
        let next = match self.rec_pool.pop() {
            Some(r) => r,
            None => {
                if let Some(ret) = self.recon_ret.as_ref() {
                    for mut b in ret.try_iter() {
                        self.rec_pool.append(&mut b);
                    }
                }
                self.rec_pool.pop().unwrap_or_else(MbRec::boxed)
            }
        };
        let full = std::mem::replace(&mut self.rec, next);
        self.batch.push(full);
        // One macroblock row per hand-over keeps the synchronisation cost negligible.
        if self.batch.len() >= self.mb_w {
            self.flush_batch();
        }
    }

    fn flush_batch(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        let b = std::mem::take(&mut self.batch);
        if let Some(tx) = self.recon_tx.as_ref() {
            // A failed send means the worker panicked; join() reports it.
            let _ = tx.send(b);
        }
    }

    fn check_addr(&self, addr: usize) -> Result<()> {
        if addr >= self.mb_w * self.mb_h {
            return Err(Error::Bitstream("macroblock address beyond picture"));
        }
        if self.st.info[addr].slice != u16::MAX {
            return Err(Error::Bitstream("macroblock decoded twice"));
        }
        Ok(())
    }

    fn decode_cavlc_slice(&mut self) -> Result<usize> {
        let total = self.mb_w * self.mb_h;
        let mut addr = self.env.hdr.first_mb_in_slice as usize * if self.mbaff { 2 } else { 1 };
        let mut count = 0usize;
        let intra_slice = self.slice_type.is_intra();
        loop {
            let mut prev_skipped = false;
            if !intra_slice {
                let run = self.br.ue();
                check(run as usize <= total.saturating_sub(addr), "mb_skip_run exceeds picture")?;
                for i in 0..run {
                    self.check_addr(addr)?;
                    let field = if self.mbaff { Some(self.cavlc_skip_field(addr, i + 1 == run)) } else { None };
                    self.decode_skip(addr, field)?;
                    count += 1;
                    addr = self.next_mb_addr(addr);
                }
                prev_skipped = run > 0;
                if run > 0 && !self.br.more_rbsp_data() {
                    break;
                }
            }
            self.check_addr(addr)?;
            let read_field = self.mbaff && (addr % 2 == 0 || prev_skipped);
            self.decode_mb(addr, read_field)?;
            count += 1;
            self.br.check_overrun()?;
            for r in [&self.br_b, &self.br_c].into_iter().flatten() {
                r.check_overrun()?;
            }
            if !self.br.more_rbsp_data() {
                break;
            }
            addr = self.next_mb_addr(addr);
        }
        Ok(count)
    }

    /// CAVLC MBAFF: mb_field_decoding_flag for a skipped macroblock.
    fn cavlc_skip_field(&mut self, addr: usize, last_of_run: bool) -> bool {
        if addr % 2 == 1 {
            return self.st.info[addr - 1].flags & F_FIELD != 0;
        }
        if last_of_run && self.br.more_rbsp_data() {
            // The bottom macroblock is coded and starts with its mb_field_decoding_flag.
            return self.br.peek(1) == 1;
        }
        self.inferred_field_flag(addr)
    }

    fn decode_cabac_slice(&mut self) -> Result<usize> {
        let mut addr = self.env.hdr.first_mb_in_slice as usize * if self.mbaff { 2 } else { 1 };
        let mut count = 0usize;
        let intra_slice = self.slice_type.is_intra();
        let mut prev_skipped = false;
        loop {
            self.check_addr(addr)?;
            let terminate_done = false;
            let skip = if intra_slice {
                false
            } else {
                match self.next_skip.take() {
                    Some(s) => s,
                    None => {
                        self.set_mb(addr, None);
                        let inc = self.skip_ctx();
                        let b = self.slice_type.is_b();
                        self.cabac.as_mut().unwrap().mb_skip(b, inc)
                    }
                }
            };
            if skip {
                let field = if !self.mbaff {
                    None
                } else if addr % 2 == 1 {
                    Some(self.st.info[addr - 1].flags & F_FIELD != 0)
                } else {
                    Some(self.cabac_lookahead_pair(addr)?)
                };
                self.decode_skip(addr, field)?;
                prev_skipped = true;
            } else {
                let read_field = self.mbaff && (addr % 2 == 0 || prev_skipped);
                self.decode_mb(addr, read_field)?;
                prev_skipped = false;
            }
            count += 1;
            // In MBAFF frames end_of_slice_flag follows only the bottom macroblock of a pair.
            if !terminate_done && !(self.mbaff && addr % 2 == 0) {
                let c = self.cabac.as_mut().unwrap();
                c.check()?;
                if c.terminate() == 1 {
                    break;
                }
            }
            addr = self.next_mb_addr(addr);
        }
        Ok(count)
    }

    /// CABAC MBAFF: the top MB of a pair is skipped. Its reconstruction needs the pair's
    /// field decoding flag, which may only be coded with the bottom MB, so decode ahead the
    /// bottom MB's mb_skip_flag and, if it is coded, its mb_field_decoding_flag (no
    /// end_of_slice_flag is coded between the two macroblocks of a pair).
    fn cabac_lookahead_pair(&mut self, addr: usize) -> Result<bool> {
        let inferred = self.inferred_field_flag(addr);
        // Temporarily record the top MB as skipped so the bottom's contexts see it.
        let saved = self.st.info[addr];
        self.st.info[addr] = MbInfo {
            slice: self.env.slice_idx,
            flags: F_SKIP | if inferred { F_FIELD } else { 0 } | if self.slice_type.is_b() { F_DIRECT16 } else { 0 },
            direct8: if self.slice_type.is_b() { 0xf } else { 0 },
            ..MbInfo::default()
        };
        self.cabac.as_mut().unwrap().check()?;
        self.set_mb(addr + 1, Some(inferred));
        let inc = self.skip_ctx();
        let b = self.slice_type.is_b();
        let bottom_skip = self.cabac.as_mut().unwrap().mb_skip(b, inc);
        self.next_skip = Some(bottom_skip);
        let flag = if bottom_skip {
            inferred
        } else {
            let inc = self.field_flag_ctx(addr + 1);
            let f = self.cabac.as_mut().unwrap().decision(70 + inc) == 1;
            self.next_field = Some(f);
            f
        };
        self.st.info[addr] = saved;
        Ok(flag)
    }

    /// 7.4.4: inference of mb_field_decoding_flag for a pair without coded flag.
    pub(crate) fn inferred_field_flag(&self, addr: usize) -> bool {
        let p = addr / 2;
        let (px, py) = (p % self.mb_w, p / self.mb_w);
        let s = self.env.slice_idx;
        if px > 0 {
            let a = 2 * (p - 1);
            if self.st.info[a].slice == s {
                return self.st.info[a].flags & F_FIELD != 0;
            }
        }
        if py > 0 {
            let b = 2 * (p - self.mb_w);
            if self.st.info[b].slice == s {
                return self.st.info[b].flags & F_FIELD != 0;
            }
        }
        false
    }

    fn field_flag_ctx(&self, addr: usize) -> usize {
        let p = addr / 2;
        let (px, py) = (p % self.mb_w, p / self.mb_w);
        let s = self.env.slice_idx;
        let f = |a: usize| (self.st.info[a].slice == s && self.st.info[a].flags & F_FIELD != 0) as usize;
        let mut inc = 0;
        if px > 0 {
            inc += f(2 * (p - 1));
        }
        if py > 0 {
            inc += f(2 * (p - self.mb_w));
        }
        inc
    }

    // ---- neighbours ----------------------------------------------------------------

    /// Set up addressing, neighbour availability and sample views for macroblock `addr`.
    /// `field` gives the MBAFF pair's field decoding flag (None: inferred / known).
    pub(crate) fn set_mb(&mut self, addr: usize, field: Option<bool>) {
        self.addr = addr;
        let s = self.env.slice_idx;
        let w = self.mb_w;
        let info = &*self.st.info;
        let avail = |a: usize| -> Option<usize> { if info[a].slice == s { Some(a) } else { None } };
        if !self.mbaff {
            self.mbx = addr % w;
            self.mby = addr / w;
            self.is_bottom = false;
            self.mb_field = self.field_pic;
            self.mb_a = if self.mbx > 0 { avail(addr - 1) } else { None };
            self.mb_b = if self.mby > 0 { avail(addr - w) } else { None };
            self.mb_c = if self.mby > 0 && self.mbx + 1 < w { avail(addr - w + 1) } else { None };
            self.mb_d = if self.mby > 0 && self.mbx > 0 { avail(addr - w - 1) } else { None };
            self.lx0 = self.mbx * 16;
            self.ly0 = self.mby * 16;
            self.cx0 = self.mbx * 8;
            self.cy0 = self.mby * 8;
            self.vstep = 1;
            return;
        }
        let p = addr / 2;
        self.mbx = p % w;
        self.mby = p / w;
        self.is_bottom = addr % 2 == 1;
        self.pair_a = if self.mbx > 0 { avail(2 * (p - 1)) } else { None };
        self.pair_b = if self.mby > 0 { avail(2 * (p - w)) } else { None };
        self.pair_c = if self.mby > 0 && self.mbx + 1 < w { avail(2 * (p - w + 1)) } else { None };
        self.pair_d = if self.mby > 0 && self.mbx > 0 { avail(2 * (p - w - 1)) } else { None };
        let f = match field {
            Some(f) => f,
            None => {
                if self.is_bottom {
                    info[addr - 1].flags & F_FIELD != 0
                } else {
                    self.inferred_field_flag(addr)
                }
            }
        };
        self.set_field(f);
    }

    /// MBAFF: (re)derive geometry for the current pair's field decoding flag.
    pub(crate) fn set_field(&mut self, f: bool) {
        self.mb_field = f;
        let b = self.is_bottom as usize;
        self.lx0 = self.mbx * 16;
        self.cx0 = self.mbx * 8;
        if f {
            self.ly0 = self.mby * 32 + b;
            self.cy0 = self.mby * 16 + b;
            self.vstep = 2;
        } else {
            self.ly0 = self.mby * 32 + 16 * b;
            self.cy0 = self.mby * 16 + 8 * b;
            self.vstep = 1;
        }
        self.mb_a = self.locate(-1, 0, 16, 16).map(|l| l.0);
        self.mb_b = self.locate(0, -1, 16, 16).map(|l| l.0);
        self.mb_c = self.locate(16, -1, 16, 16).map(|l| l.0);
        self.mb_d = self.locate(-1, -1, 16, 16).map(|l| l.0);
    }

    #[inline]
    pub(crate) fn info(&self, a: usize) -> &MbInfo {
        &self.st.info[a]
    }

    /// Neighbouring location derivation (6.4.12): the macroblock covering location
    /// (xN, yN) relative to the current macroblock (luma: mw = mh = 16, chroma 8) and the
    /// location (xW, yW) inside it. Returns the current address for locations inside the
    /// current macroblock.
    #[inline]
    pub(crate) fn locate(&self, xn: i32, yn: i32, mw: i32, mh: i32) -> Option<(usize, i32, i32)> {
        if yn >= mh || (xn >= mw && yn >= 0) {
            return None;
        }
        if !self.mbaff {
            if xn >= 0 && xn < mw && yn >= 0 {
                return Some((self.addr, xn, yn));
            }
            let n = if yn < 0 {
                if xn < 0 {
                    self.mb_d
                } else if xn < mw {
                    self.mb_b
                } else {
                    self.mb_c
                }
            } else {
                self.mb_a
            };
            return n.map(|a| (a, xn.rem_euclid(mw), yn.rem_euclid(mh)));
        }
        let bottom = self.is_bottom as i32;
        let row = if self.mb_field { 2 * yn + bottom } else { yn + mh * bottom };
        let dx = if xn < 0 {
            -1
        } else if xn >= mw {
            1
        } else {
            0
        };
        let dy = if row < 0 { -1 } else { 0 };
        let r = row.rem_euclid(2 * mh);
        let xw = xn.rem_euclid(mw);
        if dx == 0 && dy == 0 {
            if self.mb_field {
                return Some((self.addr, xw, yn));
            }
            let top = self.addr & !1;
            return Some((top + (r / mh) as usize, xw, r % mh));
        }
        let pair = match (dx, dy) {
            (-1, 0) => self.pair_a,
            (-1, _) => self.pair_d,
            (0, _) => self.pair_b,
            (1, -1) => self.pair_c,
            _ => None,
        }?;
        if self.st.info[pair].flags & F_FIELD != 0 {
            Some((pair + (r % 2) as usize, xw, r / 2))
        } else {
            Some((pair + (r / mh) as usize, xw, r % mh))
        }
    }

    /// Locate the 4x4 luma block at (x, y) (in 4x4 units, may be -1 / 4) relative to the
    /// current MB: (None, bx, by) for the current MB, (Some(addr), bx, by) for another one.
    #[inline]
    pub(crate) fn locate4(&self, x: isize, y: isize) -> Option<(Option<usize>, usize, usize)> {
        let (a, xw, yw) = self.locate(x as i32 * 4, y as i32 * 4, 16, 16)?;
        let o = if a == self.addr { None } else { Some(a) };
        Some((o, (xw / 4) as usize, (yw / 4) as usize))
    }

    fn skip_ctx(&self) -> usize {
        let c = |n: Option<usize>| n.map(|a| !self.info(a).skip()).unwrap_or(false) as usize;
        c(self.mb_a) + c(self.mb_b)
    }

    /// Reference list for the current macroblock (MBAFF field MBs use field lists).
    #[inline]
    pub(crate) fn ref_list(&self, list: usize) -> &'a [RefPic] {
        if self.mbaff && self.mb_field {
            if let Some(fr) = self.env.field_refs {
                return &fr[self.is_bottom as usize][list];
            }
        }
        &self.env.refs[list]
    }

    /// Parity of the current macroblock (field MBs) for chroma offsets and field lists.
    #[inline]
    pub(crate) fn cur_parity(&self) -> u8 {
        if self.field_pic {
            self.bottom_field
        } else if self.mbaff && self.mb_field {
            self.is_bottom as u8
        } else {
            PAR_FRAME
        }
    }

    /// POC of the current picture / field (for the current macroblock).
    #[inline]
    pub(crate) fn cur_poc(&self) -> i32 {
        if self.mbaff && self.mb_field { self.env.cur_field_poc[self.is_bottom as usize] } else { self.env.cur_poc }
    }

    // ---- skipped macroblocks -------------------------------------------------------

    fn decode_skip(&mut self, addr: usize, field: Option<bool>) -> Result<()> {
        self.set_mb(addr, field);
        self.cur = MbInfo { slice: self.env.slice_idx, flags: F_SKIP | self.field_flag_bits(), ..MbInfo::default() };
        self.transform_8x8 = false;
        self.last_qp_delta = 0;
        self.luma_nz = 0;
        self.chroma_nz = 0;
        self.reset_motion();
        if self.slice_type.is_b() {
            self.kind = MbKind::BSkip;
            self.cur.flags |= F_DIRECT16;
            self.cur.direct8 = 0xf;
            self.derive_direct_all()?;
        } else {
            self.kind = MbKind::PSkip;
            self.derive_p_skip();
        }
        self.check_refs()?;
        self.finish_mb();
        self.submit_mb();
        Ok(())
    }

    fn field_flag_bits(&self) -> u16 {
        if self.mbaff && self.mb_field { F_FIELD } else { 0 }
    }

    // ---- macroblock layer ------------------------------------------------------------

    fn read_mb_type(&mut self) -> Result<MbKind> {
        let st = self.slice_type;
        if self.cabac.is_some() {
            let f = |n: Option<usize>, s: &Self, flag: u16| n.map(|a| s.info(a).flags & flag == 0).unwrap_or(false) as usize;
            let kind = match st {
                SliceType::I => {
                    let inc = f(self.mb_a, self, F_NXN) + f(self.mb_b, self, F_NXN);
                    i_mb_type(self.cabac.as_mut().unwrap().mb_type_i_slice(inc))
                }
                SliceType::SI => {
                    let inc = f(self.mb_a, self, F_SI) + f(self.mb_b, self, F_SI);
                    if self.cabac.as_mut().unwrap().decision(inc) == 0 {
                        Some(MbKind::Si)
                    } else {
                        let inc = f(self.mb_a, self, F_NXN) + f(self.mb_b, self, F_NXN);
                        i_mb_type(self.cabac.as_mut().unwrap().mb_type_i_slice(inc))
                    }
                }
                SliceType::P | SliceType::SP => {
                    let t = self.cabac.as_mut().unwrap().mb_type_p();
                    if t < 5 { p_mb_type(t) } else { i_mb_type(t - 5) }
                }
                SliceType::B => {
                    let inc = f(self.mb_a, self, F_DIRECT16) + f(self.mb_b, self, F_DIRECT16);
                    let t = self.cabac.as_mut().unwrap().mb_type_b(inc);
                    if t < 23 { b_mb_type(t) } else { i_mb_type(t - 23) }
                }
            };
            return kind.ok_or(Error::Bitstream("invalid mb_type"));
        }
        let t = self.br.ue();
        let k = match st {
            SliceType::I => i_mb_type(t),
            SliceType::SI => {
                if t == 0 {
                    Some(MbKind::Si)
                } else {
                    i_mb_type(t.wrapping_sub(1))
                }
            }
            SliceType::P | SliceType::SP => {
                if t < 5 {
                    p_mb_type(t)
                } else {
                    i_mb_type(t - 5)
                }
            }
            SliceType::B => {
                if t < 23 {
                    b_mb_type(t)
                } else {
                    i_mb_type(t - 23)
                }
            }
        };
        k.ok_or(Error::Bitstream("invalid mb_type"))
    }

    fn reset_motion(&mut self) {
        self.mb_mv = [[[0; 2]; 16]; 2];
        self.mb_ref = [[-1; 4]; 2];
        self.parsed_ref = [[-1; 4]; 2];
        self.decoded = [0; 2];
    }

    fn decode_mb(&mut self, addr: usize, read_field: bool) -> Result<()> {
        // MBAFF: mb_field_decoding_flag.
        let field = if read_field {
            if let Some(f) = self.next_field.take() {
                Some(f)
            } else if let Some(c) = self.cabac.as_mut() {
                let _ = c;
                let inc = self.field_flag_ctx(addr);
                Some(self.cabac.as_mut().unwrap().decision(70 + inc) == 1)
            } else {
                Some(self.br.read_bit())
            }
        } else {
            None
        };
        self.next_field = None;
        self.set_mb(addr, field);
        self.cur = MbInfo { slice: self.env.slice_idx, flags: self.field_flag_bits(), ..MbInfo::default() };
        self.transform_8x8 = false;
        self.luma_nz = 0;
        self.chroma_nz = 0;
        self.reset_motion();
        let kind = self.read_mb_type()?;
        self.kind = kind;
        match kind {
            MbKind::IPcm => return self.decode_pcm(),
            MbKind::Si => {
                return Err(Error::Unsupported("SI macroblocks (Extended profile SI slices)".into()));
            }
            _ => {}
        }
        if kind.is_intra() {
            self.cur.flags |= F_INTRA;
        }
        let pps = self.env.pps;
        let mut cbp_luma: u8 = 0;
        let mut cbp_chroma: u8 = 0;
        let mut no_sub_lt8x8 = true;
        match kind {
            MbKind::INxN => {
                self.cur.flags |= F_NXN;
                if pps.transform_8x8_mode {
                    self.transform_8x8 = self.read_transform_8x8()?;
                }
                self.read_intra_nxn_modes()?;
                self.read_chroma_pred_mode()?;
            }
            MbKind::I16x16 { cbp_chroma: cc, cbp_luma: cl, .. } => {
                self.cur.flags |= F_I16;
                cbp_luma = cl;
                cbp_chroma = cc;
                self.read_chroma_pred_mode()?;
            }
            MbKind::BDirect16x16 => {
                self.cur.flags |= F_DIRECT16;
                self.cur.direct8 = 0xf;
                self.derive_direct_all()?;
                if !self.env.sps.direct_8x8_inference {
                    no_sub_lt8x8 = false;
                }
            }
            MbKind::Inter { part, pred, ref0 } => {
                if part == Part::P8x8 {
                    no_sub_lt8x8 = self.read_sub_mb_pred(ref0)?;
                } else {
                    self.read_mb_pred(part, pred)?;
                }
            }
            _ => unreachable!(),
        }
        if !matches!(kind, MbKind::I16x16 { .. }) {
            let cbp = self.read_cbp(kind.is_intra())?;
            cbp_luma = cbp & 15;
            cbp_chroma = cbp >> 4;
            if cbp_luma != 0
                && pps.transform_8x8_mode
                && !kind.is_intra()
                && no_sub_lt8x8
                && (kind != MbKind::BDirect16x16 || self.env.sps.direct_8x8_inference)
            {
                self.transform_8x8 = self.read_transform_8x8()?;
            }
        }
        if self.transform_8x8 {
            self.cur.flags |= F_T8X8;
        }
        self.cur.cbp = cbp_luma | (cbp_chroma << 4);
        if cbp_luma != 0 || cbp_chroma != 0 || matches!(kind, MbKind::I16x16 { .. }) {
            let dqp = self.read_mb_qp_delta()?;
            self.last_qp_delta = dqp;
            self.qp = (self.qp + dqp + 52) % 52;
            let start = self.br.pos();
            self.read_residual(cbp_luma, cbp_chroma)?;
            if self.trace.is_some() && self.cabac.is_none() && !self.dp {
                let end = self.br.pos();
                let k = if kind.is_intra() { 0 } else { 1 };
                self.trace.as_mut().unwrap().push((start, end, k));
            }
        } else {
            self.last_qp_delta = 0;
        }
        if !kind.is_intra() {
            self.check_refs()?;
        }
        self.finish_mb();
        self.submit_mb();
        Ok(())
    }

    fn decode_pcm(&mut self) -> Result<()> {
        let mut buf = [0u8; 384];
        let n = if self.chroma { 384 } else { 256 };
        if let Some(c) = self.cabac.as_mut() {
            c.read_pcm(&mut buf[..n])?;
        } else {
            // PCM samples are category 3 (partition B with data partitioning).
            let r = if self.dp { self.br_b.as_mut().ok_or(Error::Bitstream("I_PCM without partition B"))? } else { &mut self.br };
            let start = r.pos();
            r.align();
            for v in buf[..n].iter_mut() {
                *v = r.read(8) as u8;
            }
            r.check_overrun()?;
            let end = r.pos();
            if let Some(t) = self.trace.as_mut() {
                t.push((start, end, 2));
            }
        }
        // The samples are written by the reconstruction (in macroblock order).
        self.rec.pcm = buf;
        self.cur.flags |= F_INTRA | F_PCM;
        self.cur.cbp = 0x2f;
        self.cur.nnz = [16; 24];
        self.cur.cbf = 0x07ff_ffff;
        self.last_qp_delta = 0;
        self.transform_8x8 = false;
        self.finish_mb();
        self.submit_mb();
        Ok(())
    }

    // ---- syntax elements ---------------------------------------------------------

    pub(crate) fn read_transform_8x8(&mut self) -> Result<bool> {
        if self.cabac.is_some() {
            let f = |n: Option<usize>, info: &[MbInfo]| n.map(|a| info[a].flags & F_T8X8 != 0).unwrap_or(false) as usize;
            let inc = f(self.mb_a, self.st.info) + f(self.mb_b, self.st.info);
            Ok(self.cabac.as_mut().unwrap().transform_8x8(inc))
        } else {
            Ok(self.br.read_bit())
        }
    }

    fn read_chroma_pred_mode(&mut self) -> Result<()> {
        if !self.chroma {
            return Ok(());
        }
        let m = if self.cabac.is_some() {
            let f = |n: Option<usize>, info: &[MbInfo]| {
                n.map(|a| {
                    let i = &info[a];
                    i.intra() && !i.pcm() && i.chroma_pred != 0
                })
                .unwrap_or(false) as usize
            };
            let inc = f(self.mb_a, self.st.info) + f(self.mb_b, self.st.info);
            self.cabac.as_mut().unwrap().intra_chroma_pred_mode(inc)
        } else {
            self.br.ue_max(3, "intra_chroma_pred_mode")?
        };
        self.cur.chroma_pred = m as u8;
        Ok(())
    }

    /// Intra mode of the 4x4 block at (x, y) relative to the current MB for prediction of
    /// the mode of a neighbouring block; None means dcPredModePredictedFlag.
    fn neighbour_ipred(&self, x: isize, y: isize) -> Option<u8> {
        let constrained = self.env.pps.constrained_intra_pred;
        match self.locate4(x, y) {
            None => None,
            Some((None, bx, by)) => Some(self.cur.ipred[by * 4 + bx]),
            Some((Some(a), bx, by)) => {
                let i = self.info(a);
                if !i.intra() && constrained {
                    None
                } else if i.flags & F_NXN != 0 {
                    Some(i.ipred[by * 4 + bx])
                } else {
                    Some(2)
                }
            }
        }
    }

    fn read_intra_nxn_modes(&mut self) -> Result<()> {
        let n = if self.transform_8x8 { 4 } else { 16 };
        for k in 0..n {
            let (x, y, w) = if self.transform_8x8 {
                ((k % 2) * 2, (k / 2) * 2, 2)
            } else {
                let (x, y) = crate::tables::BLK4_XY[k];
                (x, y, 1)
            };
            let rem = if let Some(c) = self.cabac.as_mut() {
                c.intra_pred_mode()
            } else if self.br.read_bit() {
                None
            } else {
                Some(self.br.read(3))
            };
            let a = self.neighbour_ipred(x as isize - 1, y as isize);
            let b = self.neighbour_ipred(x as isize, y as isize - 1);
            let pred = match (a, b) {
                (Some(a), Some(b)) => a.min(b),
                _ => 2,
            };
            let mode = match rem {
                None => pred,
                Some(r) => {
                    if (r as u8) < pred {
                        r as u8
                    } else {
                        r as u8 + 1
                    }
                }
            };
            self.pred_modes[k] = mode;
            for yy in 0..w {
                for xx in 0..w {
                    self.cur.ipred[(y + yy) * 4 + x + xx] = mode;
                }
            }
        }
        Ok(())
    }

    fn read_cbp(&mut self, intra: bool) -> Result<u8> {
        if self.cabac.is_some() {
            return Ok(self.read_cbp_cabac());
        }
        let v = self.br.ue();
        if self.chroma {
            check(v < 48, "coded_block_pattern")?;
            Ok(if intra { CBP_INTRA[v as usize] } else { CBP_INTER[v as usize] })
        } else {
            check(v < 16, "coded_block_pattern")?;
            Ok(if intra { CBP_INTRA_MONO[v as usize] } else { CBP_INTER_MONO[v as usize] })
        }
    }

    fn read_cbp_cabac(&mut self) -> u8 {
        let mut cbp: u8 = 0;
        for b8 in 0..4usize {
            let (x8, y8) = ((b8 & 1) as i32, (b8 >> 1) as i32);
            // condTermFlagN = 1 when the neighbouring 8x8 block has its cbp bit cleared.
            let cond = |s: &Self, cbp: u8, xn: i32, yn: i32| -> usize {
                match s.locate(xn, yn, 16, 16) {
                    None => 0,
                    Some((a, xw, yw)) => {
                        let b = ((yw / 8) * 2 + xw / 8) as u8;
                        if a == s.addr {
                            return ((cbp >> b) & 1 == 0) as usize;
                        }
                        let i = s.info(a);
                        if i.pcm() {
                            0
                        } else if i.skip() {
                            1
                        } else {
                            ((i.cbp >> b) & 1 == 0) as usize
                        }
                    }
                }
            };
            let a = cond(self, cbp, x8 * 8 - 1, y8 * 8);
            let b = cond(self, cbp, x8 * 8, y8 * 8 - 1);
            let c = self.cabac.as_mut().unwrap();
            if c.decision(73 + a + 2 * b) == 1 {
                cbp |= 1 << b8;
            }
        }
        if self.chroma {
            let f = |n: Option<usize>, s: &Self, two: bool| -> usize {
                match n {
                    None => 0,
                    Some(a) => {
                        let i = s.info(a);
                        if i.pcm() {
                            1
                        } else if i.skip() {
                            0
                        } else {
                            let cc = i.cbp >> 4;
                            (if two { cc == 2 } else { cc != 0 }) as usize
                        }
                    }
                }
            };
            let inc = f(self.mb_a, self, false) + 2 * f(self.mb_b, self, false);
            let c = self.cabac.as_mut().unwrap();
            if c.decision(77 + inc) == 1 {
                let inc = f(self.mb_a, self, true) + 2 * f(self.mb_b, self, true) + 4;
                let c = self.cabac.as_mut().unwrap();
                let v = if c.decision(77 + inc) == 1 { 2 } else { 1 };
                cbp |= v << 4;
            }
        }
        cbp
    }

    fn read_mb_qp_delta(&mut self) -> Result<i32> {
        let v = if let Some(c) = self.cabac.as_mut() {
            c.mb_qp_delta((self.last_qp_delta != 0) as usize)
        } else {
            self.br.se()
        };
        check((-26..=25).contains(&v), "mb_qp_delta out of range")?;
        Ok(v)
    }

    /// ref_idx_lX for the partition whose top-left 4x4 block is (x, y).
    pub(crate) fn read_ref_idx(&mut self, list: usize, x: usize, y: usize) -> Result<i8> {
        // Field macroblocks of MBAFF frames index field lists (twice the entries).
        let n = self.env.hdr.num_ref_idx_active[list] * if self.mbaff && self.mb_field { 2 } else { 1 };
        if n <= 1 {
            return Ok(0);
        }
        let v = if self.cabac.is_some() {
            let cond = |s: &Self, nx: isize, ny: isize| -> usize {
                match s.locate4(nx, ny) {
                    None => 0,
                    Some((None, bx, by)) => {
                        let b8 = (by >> 1) * 2 + (bx >> 1);
                        if s.cur.direct8 >> b8 & 1 != 0 {
                            0
                        } else {
                            (s.parsed_ref[list][b8] > 0) as usize
                        }
                    }
                    Some((Some(a), bx, by)) => {
                        let i = s.info(a);
                        if i.intra() || i.skip() {
                            return 0;
                        }
                        let b8 = (by >> 1) * 2 + (bx >> 1);
                        if s.slice_type.is_b() && i.direct8 >> b8 & 1 != 0 {
                            return 0;
                        }
                        let r = s.st.motion[a].ref_idx[list][b8];
                        // refIdxZeroFlagN with field/frame adaptation (9.3.3.1.1.6).
                        let thr = if s.mbaff && !s.mb_field && i.flags & F_FIELD != 0 { 1 } else { 0 };
                        (r > thr) as usize
                    }
                }
            };
            let inc = cond(self, x as isize - 1, y as isize) + 2 * cond(self, x as isize, y as isize - 1);
            self.cabac.as_mut().unwrap().ref_idx(inc)
        } else {
            self.br.te(n - 1)
        };
        check(v < n, "ref_idx out of range")?;
        Ok(v as i8)
    }

    /// mvd_lX for the partition with top-left 4x4 block (x, y); returns (mvd_x, mvd_y).
    pub(crate) fn read_mvd(&mut self, list: usize, x: usize, y: usize) -> Result<[i32; 2]> {
        if self.cabac.is_some() {
            let get = |s: &Self, nx: isize, ny: isize, comp: usize| -> u32 {
                match s.locate4(nx, ny) {
                    None => 0,
                    Some((None, bx, by)) => s.cur.mvd[list][by * 4 + bx][comp] as u32,
                    Some((Some(a), bx, by)) => {
                        let i = s.info(a);
                        let v = i.mvd[list][by * 4 + bx][comp] as u32;
                        if comp == 1 && s.mbaff {
                            let nf = i.flags & F_FIELD != 0;
                            if s.mb_field && !nf {
                                return v >> 1;
                            }
                            if !s.mb_field && nf {
                                return v << 1;
                            }
                        }
                        v
                    }
                }
            };
            let mut out = [0i32; 2];
            for comp in 0..2 {
                let amvd = get(self, x as isize - 1, y as isize, comp) + get(self, x as isize, y as isize - 1, comp);
                out[comp] = self.cabac.as_mut().unwrap().mvd(if comp == 0 { 40 } else { 47 }, amvd);
            }
            Ok(out)
        } else {
            let mx = self.br.se();
            let my = self.br.se();
            check(mx != i32::MIN && my != i32::MIN, "invalid mvd")?;
            Ok([mx, my])
        }
    }

    pub(crate) fn store_mvd(&mut self, list: usize, x: usize, y: usize, w: usize, h: usize, mvd: [i32; 2]) {
        let a = [mvd[0].unsigned_abs().min(127) as u8, mvd[1].unsigned_abs().min(127) as u8];
        for yy in y..y + h {
            for xx in x..x + w {
                self.cur.mvd[list][yy * 4 + xx] = a;
            }
        }
    }

    // ---- finishing -----------------------------------------------------------------

    /// Store the current macroblock's state for later macroblocks, the deblocking filter
    /// and future pictures.
    pub(crate) fn finish_mb(&mut self) {
        let addr = self.addr;
        let intra = self.cur.intra();
        let pps = self.env.pps;
        let qp_db = if self.cur.pcm() { 0 } else { self.qp };
        self.cur.flags |= if self.transform_8x8 { F_T8X8 } else { 0 };
        // Deblocking info.
        let mut nz: u16 = 0;
        if !intra {
            if self.transform_8x8 {
                for b8 in 0..4 {
                    if self.luma_nz >> b8 & 1 != 0 {
                        let (x8, y8) = (b8 & 1, b8 >> 1);
                        for k in 0..4 {
                            let (x, y) = (x8 * 2 + (k & 1), y8 * 2 + (k >> 1));
                            nz |= 1 << (y * 4 + x);
                        }
                    }
                }
            } else {
                for blk in 0..16 {
                    if self.luma_nz >> blk & 1 != 0 {
                        let (x, y) = crate::tables::BLK4_XY[blk];
                        nz |= 1 << (y * 4 + x);
                    }
                }
            }
        }
        self.st.db[addr] = DbMb {
            slice: self.env.slice_idx,
            intra,
            transform_8x8: self.transform_8x8,
            field: self.mb_field,
            qp: qp_db as u8,
            qpc: [
                chroma_qp(qp_db, pps.chroma_qp_index_offset) as u8,
                chroma_qp(qp_db, pps.second_chroma_qp_index_offset) as u8,
            ],
            nz,
        };
        // Motion data.
        let mut m = MbMotion { intra, field: self.mb_field, ..MbMotion::default() };
        if !intra {
            m.mv = self.mb_mv;
            m.ref_idx = self.mb_ref;
            for list in 0..2 {
                let refs = self.ref_list(list);
                for b8 in 0..4 {
                    let r = self.mb_ref[list][b8];
                    if r >= 0 {
                        m.ref_pic[list][b8] = refs.get(r as usize).map(|rp| rp.code()).unwrap_or(-1);
                    } else {
                        m.mv[list][(b8 >> 1) * 8 + (b8 & 1) * 2] = [0, 0];
                    }
                }
                for blk in 0..16 {
                    if self.mb_ref[list][(blk >> 3) * 2 + ((blk & 3) >> 1)] < 0 {
                        m.mv[list][blk] = [0, 0];
                    }
                }
            }
        }
        self.st.motion[addr] = m;
        if intra {
            self.cur.mvd = [[[0; 2]; 16]; 2];
        }
        self.st.info[addr] = self.cur;
    }
}
