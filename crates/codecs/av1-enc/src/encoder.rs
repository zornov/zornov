// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Facade: the public encoder. Owns the module pipeline.
//!
//! 8-bit 4:2:0, one tile, 64x64 superblocks, no loop filter, CDEF or restoration. The rate is a
//! constant quantiser (`RateMode::Cqp`), constant quality (`Crf`) or an average bitrate
//! (`Bitrate`), see [`crate::rc`]. A key frame starts the stream and then comes every `keyint`
//! frames and at scene cuts; the frames between are inter frames predicted from the previous
//! frame. A key frame's packet carries a sequence header and is a sync sample; an inter frame's
//! does not.

use std::sync::Arc;

use crate::bits::BitWriter;
use crate::filters::{self, FilterParams};
use crate::error::{Error, Result};
use crate::mode::{FixedDecider, ModeDecider, SearchDecider};
use crate::rc::gop::GopPlanner;
use crate::rc::{MAX_RETRIES, RateControl};
use crate::obu::{ObuType, av1c, obu, write_frame_header, write_sequence_header};
use crate::motion::InterFrameCoder;
use crate::recon::{FrameBuf, QuantInfo, RefStore};
use crate::syntax::TileWriter;
use crate::types::{Config, Frame, FrameParams, FrameType, Packet, RateMode, SbPos, SeqParams};

/// Speeds from this one up skip the mode search (fixed `DC_PRED`).
const FIXED_FROM_SPEED: u8 = 9;

/// `(seq_level_idx, MaxPicSize)` of the AV1 levels, smallest first.
const LEVELS: [(u8, u64); 10] = [(0, 147_456), (1, 278_784), (4, 665_856), (5, 1_065_024), (8, 2_359_296), (9, 2_359_296), (12, 8_912_896), (13, 8_912_896), (14, 35_651_584), (16, 35_651_584)];

/// The lowest level that admits a `width` x `height` picture (`MaxPicSize` and the side limits
/// of `sqrt(8 * MaxPicSize)`); the highest listed one for anything larger.
fn level_for(width: u32, height: u32) -> u8 {
    let area = width as u64 * height as u64;
    for &(idx, max) in &LEVELS {
        let side = ((max * 8) as f64).sqrt() as u64;
        if area <= max && width as u64 <= side && height as u64 <= side {
            return idx;
        }
    }
    LEVELS[LEVELS.len() - 1].0
}

pub struct Encoder {
    cfg: Config,
    seq: SeqParams,
    dseq: studio_av1::SequenceHeader,
    filters: filters::FilterSet,
    gop: GopPlanner,
    rc: RateControl,
    frames: u32,
    refs: RefStore,
    recon: Option<Frame>,
}

/// One coded frame, before it becomes the encoder's state.
struct Coded {
    data: Vec<u8>,
    recon: FrameBuf,
    refresh: u8,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder> {
        if cfg.width == 0 || cfg.height == 0 {
            return Err(Error::Invalid("zero size"));
        }
        if cfg.width > 65536 || cfg.height > 65536 {
            return Err(Error::Invalid("size beyond 65536"));
        }
        if !matches!(cfg.bit_depth, 8 | 10) {
            return Err(Error::Invalid("bit depth must be 8 or 10"));
        }
        if cfg.fps.0 == 0 || cfg.fps.1 == 0 {
            return Err(Error::Invalid("frame rate must be positive"));
        }
        if cfg.bit_depth != 8 {
            return Err(Error::Unsupported("10-bit encoding"));
        }
        if cfg.tiles != (0, 0) {
            return Err(Error::Unsupported("tiles"));
        }
        match cfg.rate {
            RateMode::Cqp(0) => return Err(Error::Unsupported("lossless (base_q_idx 0)")),
            RateMode::Crf(c) if c > 63 => return Err(Error::Invalid("CRF is 0 to 63")),
            RateMode::Bitrate(0) => return Err(Error::Invalid("bitrate must be positive")),
            _ => {}
        }
        let seq = SeqParams {
            width: cfg.width,
            height: cfg.height,
            bit_depth: cfg.bit_depth,
            fps: cfg.fps,
            profile: 0,
            level_idx: level_for(cfg.width, cfg.height),
            use_128x128_superblock: false,
            enable_cdef: true,
            enable_restoration: true,
            enable_order_hint: true,
            // BT.709, limited range: what `color::from_rgba8` produces
            color_primaries: 1,
            transfer_characteristics: 1,
            matrix_coefficients: 1,
            full_range: false,
        };
        let dseq = filters::decoder_sequence(&write_sequence_header(&seq));
        Ok(Encoder { dseq, filters: Default::default(), gop: GopPlanner::new(cfg.keyint), rc: RateControl::new(&cfg), cfg, seq, frames: 0, refs: RefStore::default(), recon: None })
    }

    /// Chooses which in-loop filters may be used (all of them by default).
    pub fn with_filters(mut self, filters: filters::FilterSet) -> Encoder {
        self.filters = filters;
        self
    }

    /// The `av1C` record for the MP4 track.
    pub fn av1c(&self) -> Vec<u8> {
        av1c(&self.seq)
    }

    /// The reconstruction of the last frame pushed: what a decoder outputs for its packet.
    pub fn reconstruction(&self) -> Option<&Frame> {
        self.recon.as_ref()
    }

    /// Codes `frame` at quantiser `q` without touching the encoder's state.
    fn code_frame(&self, frame: &Frame, key: bool, q: u8) -> Coded {
        let (mi_cols, mi_rows) = (2 * ((frame.width + 7) >> 3), 2 * ((frame.height + 7) >> 3));
        let mut buf = FrameBuf::new(frame.width, frame.height, frame.bit_depth);
        buf.quant = QuantInfo { base_q_idx: q, ..Default::default() };
        // the sequence header written here turns the intra edge filter off
        buf.enable_intra_edge_filter = false;
        let mut tw = TileWriter::new(&self.seq, q, mi_cols, mi_rows);
        let ref_idx = [0u8; 7];
        let (mut intra_sbs, mut inter_sbs) = (Vec::new(), Vec::new());
        if key {
            let mut decider: Box<dyn ModeDecider> = if self.cfg.speed >= FIXED_FROM_SPEED { Box::new(FixedDecider { base_q_idx: q }) } else { Box::new(SearchDecider { base_q_idx: q }) };
            for row in 0..mi_rows.div_ceil(16) {
                for col in 0..mi_cols.div_ceil(16) {
                    let pos = SbPos { col, row };
                    let sb = decider.decide_sb(frame, &mut buf, pos);
                    tw.write_sb(pos, &sb);
                    intra_sbs.push((pos, sb));
                }
            }
        } else {
            let fast = self.cfg.speed >= FIXED_FROM_SPEED;
            // fast: one block size by quantiser (coarser ones leave less to gain from small blocks,
            // and their mode info costs more); otherwise the size is chosen for every superblock
            let fixed_size = if q >= 160 { 16 } else { 8 };
            let mut coder = InterFrameCoder::new(frame, &self.refs, ref_idx, if fast { fixed_size } else { 0 }, self.cfg.speed <= 6);
            for row in 0..mi_rows.div_ceil(16) {
                for col in 0..mi_cols.div_ceil(16) {
                    let pos = SbPos { col, row };
                    let sb = coder.decide_sb(&mut buf, pos);
                    tw.write_inter_sb(pos, &sb);
                    inter_sbs.push((pos, sb));
                }
            }
        }
        // in-loop filters: the search sees the finished tile, the references the filtered frame
        let fast = self.cfg.speed >= FIXED_FROM_SPEED;
        let lf = if fast || !self.filters.deblock { [0, 0] } else { filters::search_deblock(&self.dseq, &buf, tw.mode_info(), frame) };
        let deblocked = filters::apply(&self.dseq, &buf, tw.mode_info(), &FilterParams { lf_level: lf, cdef: None, lr: None });
        let cdef = if fast || !self.filters.cdef { None } else { filters::search_cdef(&self.dseq, &deblocked, tw.mode_info(), frame, q) };
        let lr = if fast || !self.filters.restoration || self.cfg.speed > 6 { None } else { filters::search_lr(&self.dseq, &buf, tw.mode_info(), frame, lf, cdef.as_ref(), q) };
        let buf = if cdef.is_none() && lr.is_none() {
            deblocked
        } else {
            filters::apply(&self.dseq, &buf, tw.mode_info(), &FilterParams { lf_level: lf, cdef: cdef.clone(), lr: lr.clone() })
        };
        // the strength index and the restoration units are coded inside the tile data, so a frame
        // that uses more than one strength set or any restoration is written again
        let rewrite = cdef.as_ref().is_some_and(|c| c.params.bits > 0) || lr.is_some();
        let tile = if rewrite {
            let mut tw2 = TileWriter::new(&self.seq, q, mi_cols, mi_rows);
            if let Some(c) = cdef.as_ref().filter(|c| c.params.bits > 0) {
                tw2 = tw2.with_cdef(c.params.bits as u32, c.idx.clone());
            }
            if let Some(l) = &lr {
                tw2 = tw2.with_lr(l.clone());
            }
            for (pos, sb) in &intra_sbs {
                tw2.write_sb(*pos, sb);
            }
            for (pos, sb) in &inter_sbs {
                tw2.write_inter_sb(*pos, sb);
            }
            tw2.finish()
        } else {
            tw.finish()
        };

        let fp = FrameParams {
            frame_type: if key { FrameType::Key } else { FrameType::Inter },
            show_frame: true,
            base_q_idx: q,
            disable_cdf_update: false,
            refresh_frame_flags: if key { 0xFF } else { 0x01 },
            order_hint: self.frames & 0x7F,
            ref_frame_idx: ref_idx,
            loop_filter_level: [lf[0] as u8, lf[1] as u8],
            cdef_enabled: false,
            cdef: cdef.map(|c| c.params).unwrap_or_default(),
            lr: lr.as_ref().map(|l| l.params).unwrap_or_default(),
            tile_cols_log2: 0,
            tile_rows_log2: 0,
        };
        let mut w = BitWriter::new();
        write_frame_header(&self.seq, &fp, &mut w);
        w.byte_align();
        w.put_bytes(&tile);
        let mut data = if key { obu(ObuType::SequenceHeader, &write_sequence_header(&self.seq)) } else { Vec::new() };
        data.extend(obu(ObuType::Frame, &w.finish()));
        Coded { data, recon: buf, refresh: fp.refresh_frame_flags }
    }

    /// Encodes one frame; the packet comes back at once. The first frame, every `keyint`-th one
    /// after it and every scene cut is a key frame, the others are inter frames.
    pub fn push_frame(&mut self, frame: &Frame, pts: i64) -> Result<Vec<Packet>> {
        if (frame.width, frame.height) != (self.cfg.width, self.cfg.height) {
            return Err(Error::Invalid("frame size differs from the configuration"));
        }
        if frame.bit_depth != self.cfg.bit_depth {
            return Err(Error::Invalid("frame bit depth differs from the configuration"));
        }
        let (cw, ch) = ((frame.width as usize).div_ceil(2), (frame.height as usize).div_ceil(2));
        let want = [frame.width as usize * frame.height as usize, cw * ch, cw * ch];
        if frame.stride != [frame.width as usize, cw, cw] || (0..3).any(|p| frame.planes[p].len() != want[p]) {
            return Err(Error::Invalid("frame planes must be tightly packed 4:2:0"));
        }
        let ftype = self.gop.next_type(frame);
        let key = ftype == FrameType::Key;
        let mut q = self.rc.frame_qidx(ftype);
        let mut coded = self.code_frame(frame, key, q);
        // a frame far off its target is coded again; the attempt nearest the target is kept
        let (mut best_q, mut best_miss) = (q, self.rc.miss(coded.data.len()));
        for _ in 0..MAX_RETRIES {
            match self.rc.retry_qidx(ftype, q, coded.data.len()) {
                Some(nq) => {
                    q = nq;
                    let attempt = self.code_frame(frame, key, q);
                    let miss = self.rc.miss(attempt.data.len());
                    if miss < best_miss {
                        (best_q, best_miss) = (q, miss);
                        coded = attempt;
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
        let q = best_q;
        self.rc.update(ftype, q, coded.data.len());
        self.frames += 1;
        self.recon = Some(coded.recon.to_frame());
        self.refs.refresh(coded.refresh, Arc::new(coded.recon));
        Ok(vec![Packet { data: coded.data, keyframe: key, pts }])
    }

    /// Nothing is held back: every frame leaves in its own packet.
    pub fn flush(&mut self) -> Result<Vec<Packet>> {
        Ok(Vec::new())
    }
}
