// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Facade: the public encoder. Owns the module pipeline.
//!
//! Phase (a): every frame is a key frame coded at a constant quantiser (`RateMode::Cqp`), 8-bit
//! 4:2:0, one tile, 64x64 superblocks, no loop filter, CDEF or restoration. Each packet carries
//! its own sequence header, so every packet is a sync sample.

use crate::bits::BitWriter;
use crate::error::{Error, Result};
use crate::mode::{FixedDecider, ModeDecider, SearchDecider};
use crate::obu::{ObuType, av1c, obu, write_frame_header, write_sequence_header};
use crate::recon::{FrameBuf, QuantInfo};
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
    q: u8,
    decider: Box<dyn ModeDecider + Send>,
    frames: u32,
    recon: Option<Frame>,
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
        let q = match cfg.rate {
            RateMode::Cqp(0) => return Err(Error::Unsupported("lossless (base_q_idx 0)")),
            RateMode::Cqp(q) => q,
            RateMode::Crf(_) | RateMode::Bitrate(_) => return Err(Error::Unsupported("rate control: use RateMode::Cqp")),
        };
        let seq = SeqParams {
            width: cfg.width,
            height: cfg.height,
            bit_depth: cfg.bit_depth,
            fps: cfg.fps,
            profile: 0,
            level_idx: level_for(cfg.width, cfg.height),
            use_128x128_superblock: false,
            enable_cdef: false,
            enable_restoration: false,
            enable_order_hint: true,
            // BT.709, limited range: what `color::from_rgba8` produces
            color_primaries: 1,
            transfer_characteristics: 1,
            matrix_coefficients: 1,
            full_range: false,
        };
        let decider: Box<dyn ModeDecider + Send> = if cfg.speed >= FIXED_FROM_SPEED { Box::new(FixedDecider { base_q_idx: q }) } else { Box::new(SearchDecider { base_q_idx: q }) };
        Ok(Encoder { cfg, seq, q, decider, frames: 0, recon: None })
    }

    /// The `av1C` record for the MP4 track.
    pub fn av1c(&self) -> Vec<u8> {
        av1c(&self.seq)
    }

    /// The reconstruction of the last frame pushed: what a decoder outputs for its packet.
    pub fn reconstruction(&self) -> Option<&Frame> {
        self.recon.as_ref()
    }

    /// Encodes one frame as a key frame; the packet comes back at once.
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
        let (mi_cols, mi_rows) = (2 * ((frame.width + 7) >> 3), 2 * ((frame.height + 7) >> 3));
        let mut buf = FrameBuf::new(frame.width, frame.height, frame.bit_depth);
        buf.quant = QuantInfo { base_q_idx: self.q, ..Default::default() };
        // the sequence header written here turns the intra edge filter off
        buf.enable_intra_edge_filter = false;
        let mut tw = TileWriter::new(&self.seq, self.q, mi_cols, mi_rows);
        for row in 0..mi_rows.div_ceil(16) {
            for col in 0..mi_cols.div_ceil(16) {
                let pos = SbPos { col, row };
                let sb = self.decider.decide_sb(frame, &mut buf, pos);
                tw.write_sb(pos, &sb);
            }
        }
        let tile = tw.finish();

        let fp = FrameParams {
            frame_type: FrameType::Key,
            show_frame: true,
            base_q_idx: self.q,
            disable_cdf_update: false,
            refresh_frame_flags: 0xFF,
            order_hint: self.frames & 0x7F,
            ref_frame_idx: [0; 7],
            loop_filter_level: [0, 0],
            cdef_enabled: false,
            tile_cols_log2: 0,
            tile_rows_log2: 0,
        };
        let mut w = BitWriter::new();
        write_frame_header(&self.seq, &fp, &mut w);
        w.byte_align();
        w.put_bytes(&tile);
        let mut data = obu(ObuType::SequenceHeader, &write_sequence_header(&self.seq));
        data.extend(obu(ObuType::Frame, &w.finish()));
        self.frames += 1;
        self.recon = Some(buf.to_frame());
        Ok(vec![Packet { data, keyframe: true, pts }])
    }

    /// Nothing is held back: every frame leaves in its own packet.
    pub fn flush(&mut self) -> Result<Vec<Packet>> {
        Ok(Vec::new())
    }
}
