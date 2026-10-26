// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The public decoder: raw_data_block() parsing (ISO/IEC 14496-3 4.4.2.1),
//! element-to-channel mapping, the per-channel reconstruction chain and the
//! SBR / PS post-processing.

use crate::bits::BitReader;
use crate::cce::{Cce, CouplingPoint};
use crate::config::{AdtsHeader, AudioSpecificConfig, ElementKind, ElementSlot, ProgramConfig, Signal, Speaker};
use crate::error::{invalid, Error, Result};
use crate::filterbank::Filterbank;
use crate::ics::{ChannelFrame, IcsInfo, BAND_STRIDE};
use crate::pns::{self, NoiseGen};
use crate::sbr::SbrElement;
use crate::spectral;
use crate::stereo::{self, MsInfo};
use crate::tns;

const ID_SCE: u32 = 0;
const ID_CPE: u32 = 1;
const ID_CCE: u32 = 2;
const ID_LFE: u32 = 3;
const ID_DSE: u32 = 4;
const ID_PCE: u32 = 5;
const ID_FIL: u32 = 6;
const ID_END: u32 = 7;

const EXT_SBR_DATA: u32 = 13;
const EXT_SBR_DATA_CRC: u32 = 14;

/// 16-bit PCM scale of the spectral domain (the standard reconstructs to
/// +-32768 full scale).
const OUTPUT_SCALE: f32 = 1.0 / 32768.0;

/// Core samples are kept within +-2^21 (64 x full scale): legitimate
/// streams never get there, while corrupt scale factors could otherwise
/// overflow the SBR energy estimates and poison the decoder state.
const CORE_LIMIT: f32 = 2_097_152.0;

#[inline]
fn sanitize(buf: &mut [f32], limit: f32) {
    for v in buf {
        // NaN fails both comparisons and becomes 0.
        *v = if *v >= -limit && *v <= limit { *v } else if *v > limit { limit } else if *v < -limit { -limit } else { 0.0 };
    }
}

struct ChannelState {
    frame: ChannelFrame,
    overlap: Vec<f32>,
    prev_shape: u8,
    /// Core time-domain output of the current frame (1024 samples).
    time: Vec<f32>,
}

impl ChannelState {
    fn new() -> ChannelState {
        ChannelState { frame: ChannelFrame::new(), overlap: vec![0.0; 1024], prev_shape: 0, time: vec![0.0; 1024] }
    }
}

struct Element {
    slot: ElementSlot,
    present: bool,
    common_window: bool,
    ms: MsInfo,
    ch: Vec<ChannelState>,
    /// First output channel of this element.
    out_channel: usize,
    sbr: Option<Box<SbrElement>>,
    /// SBR payload received for the current frame.
    sbr_data: bool,
    /// element_instance_tag seen in the current frame.
    tag: u8,
}

/// Counters of the coding tools seen by a [`Decoder`] (diagnostics).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecoderStats {
    /// Access units decoded successfully.
    pub frames: u64,
    /// Channel frames (one per channel per access unit).
    pub channel_frames: u64,
    pub long_start_windows: u64,
    pub eight_short_windows: u64,
    pub long_stop_windows: u64,
    pub kbd_windows: u64,
    pub tns_channel_frames: u64,
    pub pulse_channel_frames: u64,
    pub ms_bands: u64,
    pub intensity_bands: u64,
    pub noise_bands: u64,
    pub coupling_elements: u64,
    pub sbr_element_frames: u64,
    /// SBR channel pair frames using envelope coupling (balance coding).
    pub sbr_coupled_frames: u64,
    pub ps_frames: u64,
    pub ps_34_band_frames: u64,
    pub ps_ipd_opd_frames: u64,
}

/// MPEG-4 AAC LC / HE-AAC / HE-AAC v2 decoder.
///
/// Feed it one access unit (raw_data_block) at a time; the result is
/// interleaved `f32` PCM in `[-1, 1]` at [`Decoder::sample_rate`] with
/// [`Decoder::channels`] channels, in the order of the AAC channel
/// configuration (see [`Decoder::channel_layout`]).
pub struct Decoder {
    asc: AudioSpecificConfig,
    elements: Vec<Element>,
    cces: Vec<Cce>,
    fb: Filterbank,
    rng: NoiseGen,
    out: Vec<f32>,
    /// Per-channel planar buffers of the current frame.
    planar: Vec<Vec<f32>>,
    sbr_active: bool,
    ps_active: bool,
    sample_rate: u32,
    channels: usize,
    frame_len: usize,
    stats: DecoderStats,
}

impl Decoder {
    /// Create a decoder from AudioSpecificConfig bytes (MP4 `esds`
    /// DecoderSpecificInfo).
    pub fn new(asc: &[u8]) -> Result<Decoder> {
        Decoder::from_config(AudioSpecificConfig::parse(asc)?)
    }

    /// Create a decoder for an ADTS stream from its first frame header.
    pub fn new_adts(frame: &[u8]) -> Result<Decoder> {
        let h = AdtsHeader::parse(frame)?;
        if h.channel_config == 0 {
            return Err(Error::Unsupported("ADTS with in-band PCE channel configuration"));
        }
        Decoder::new(&h.audio_specific_config())
    }

    pub fn from_config(asc: AudioSpecificConfig) -> Result<Decoder> {
        let layout = asc.layout()?;
        let mut elements = Vec::new();
        let mut ch = 0;
        for slot in layout {
            let n = slot.kind.channels();
            elements.push(Element {
                slot,
                present: false,
                common_window: false,
                ms: MsInfo::new(),
                ch: (0..n).map(|_| ChannelState::new()).collect(),
                out_channel: ch,
                sbr: None,
                sbr_data: false,
                tag: 0,
            });
            ch += n;
        }
        let mut d = Decoder {
            asc,
            elements,
            cces: (0..16).map(|_| Cce::new()).collect(),
            fb: Filterbank::new(),
            rng: NoiseGen::new(),
            out: Vec::new(),
            planar: Vec::new(),
            sbr_active: false,
            ps_active: false,
            sample_rate: 0,
            channels: ch,
            frame_len: 1024,
            stats: DecoderStats::default(),
        };
        if d.asc.sbr == Signal::Present {
            d.enable_sbr()?;
        }
        d.update_format();
        Ok(d)
    }

    fn sbr_rate(&self) -> u32 {
        if self.asc.sbr == Signal::Present && self.asc.ext_sample_rate != 0 {
            self.asc.ext_sample_rate
        } else {
            (self.asc.sample_rate * 2).min(96000)
        }
    }

    fn enable_sbr(&mut self) -> Result<()> {
        let rate = self.sbr_rate();
        let downsampled = rate <= self.asc.sample_rate;
        // SBR operates on fs_out (twice the core rate) unless downsampled.
        let sbr_fs = if downsampled { self.asc.sample_rate * 2 } else { rate };
        for e in &mut self.elements {
            if e.sbr.is_none() {
                e.sbr = Some(Box::new(SbrElement::new(sbr_fs, e.slot.kind.channels(), downsampled)?));
            }
        }
        self.sbr_active = true;
        if self.asc.ps == Signal::Present && self.is_mono() {
            self.ps_active = true;
        }
        Ok(())
    }

    fn is_mono(&self) -> bool {
        self.elements.len() == 1 && self.elements[0].slot.kind != ElementKind::Cpe
    }

    fn update_format(&mut self) {
        let core: usize = self.elements.iter().map(|e| e.ch.len()).sum();
        self.channels = if self.ps_active { 2 } else { core };
        if self.sbr_active {
            let downsampled = self.sbr_rate() <= self.asc.sample_rate;
            self.frame_len = if downsampled { 1024 } else { 2048 };
            self.sample_rate = if downsampled { self.asc.sample_rate } else { self.sbr_rate() };
        } else {
            self.frame_len = 1024;
            self.sample_rate = self.asc.sample_rate;
        }
        self.planar = (0..self.channels).map(|_| vec![0.0; self.frame_len]).collect();
    }

    /// Output sample rate (twice the core rate when SBR is active).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Number of interleaved output channels (2 for mono + PS).
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Samples per channel produced by each access unit (1024 or 2048).
    pub fn samples_per_frame(&self) -> usize {
        self.frame_len
    }

    /// Whether SBR (HE-AAC) processing is active.
    /// Extra output delay of this decoder relative to the AAC timeline used
    /// by MP4 edit lists / iTunSMPB priming (which already count the 1024 +
    /// encoder-delay samples of the core): 0 for AAC LC, 962 output samples
    /// of SBR QMF delay for HE-AAC (481 for downsampled SBR). Skip
    /// `priming * output_rate / core_rate + output_delay()` samples to align.
    pub fn output_delay(&self) -> usize {
        if !self.sbr_active {
            0
        } else if self.frame_len == 1024 {
            481
        } else {
            962
        }
    }

    /// Tool usage counters accumulated since construction.
    pub fn stats(&self) -> &DecoderStats {
        &self.stats
    }

    pub fn sbr_active(&self) -> bool {
        self.sbr_active
    }

    /// Whether parametric stereo is active.
    pub fn ps_active(&self) -> bool {
        self.ps_active
    }

    pub fn config(&self) -> &AudioSpecificConfig {
        &self.asc
    }

    /// Speaker position of each output channel.
    pub fn channel_layout(&self) -> Vec<Speaker> {
        if self.ps_active {
            return vec![Speaker::FrontLeft, Speaker::FrontRight];
        }
        self.elements.iter().flat_map(|e| e.slot.speakers.clone()).collect()
    }

    /// Clear all inter-frame state (overlap buffers, SBR and PS history), as
    /// after a seek. The configuration is kept.
    pub fn reset(&mut self) {
        for e in &mut self.elements {
            for c in &mut e.ch {
                c.overlap.fill(0.0);
                c.prev_shape = 0;
            }
            if let Some(s) = &mut e.sbr {
                s.reset();
            }
        }
        for c in &mut self.cces {
            c.overlap.fill(0.0);
            c.prev_shape = 0;
        }
        self.rng = NoiseGen::new();
    }

    /// Emit the signal still held in the overlap buffers (the second half
    /// of the last frame) by decoding one silent frame, then reset.
    pub fn flush(&mut self) -> &[f32] {
        for e in &mut self.elements {
            e.present = true;
            e.common_window = false;
            e.ms.present = 0;
            e.sbr_data = false;
            for c in &mut e.ch {
                c.frame.ics = IcsInfo { window_shape: c.prev_shape, ..IcsInfo::default() };
                c.frame.tns.present = false;
                c.frame.pulses.count = 0;
                c.frame.quant.fill(0);
            }
        }
        for c in &mut self.cces {
            c.present = false;
        }
        if self.reconstruct().is_err() {
            self.out.clear();
        }
        self.reset();
        &self.out
    }

    /// Decode one ADTS frame (header included). Multiple raw data blocks in
    /// one frame are decoded in order and concatenated.
    pub fn decode_adts_frame(&mut self, frame: &[u8]) -> Result<&[f32]> {
        let h = AdtsHeader::parse(frame)?;
        if frame.len() < h.frame_length {
            return Err(Error::Truncated);
        }
        if h.object_type != 2 {
            return Err(Error::Unsupported("ADTS profile other than LC"));
        }
        let blocks = h.num_raw_data_blocks as usize + 1;
        let payload = &frame[h.header_length..h.frame_length];
        if blocks == 1 {
            return self.decode_frame(payload);
        }
        let mut all = Vec::new();
        let mut br = BitReader::new(payload);
        if !h.protection_absent {
            // raw_data_block_position[1..] (+ header CRC already skipped)
            br.skip(16 * (blocks - 1))?;
        }
        for _ in 0..blocks {
            let start = br.position() / 8;
            let mut sub = BitReader::new(&payload[start..]);
            self.parse_raw_data_block(&mut sub)?;
            sub.byte_align()?;
            if !h.protection_absent {
                sub.skip(16)?; // crc_check
            }
            br.skip(sub.position())?;
            self.reconstruct()?;
            all.extend_from_slice(&self.out);
        }
        self.out = all;
        Ok(&self.out)
    }

    /// Decode one access unit (raw_data_block). Returns interleaved samples.
    pub fn decode_frame(&mut self, au: &[u8]) -> Result<&[f32]> {
        let mut br = BitReader::new(au);
        self.parse_raw_data_block(&mut br)?;
        self.reconstruct()?;
        self.count_tools();
        Ok(&self.out)
    }

    /// Samples produced by the most recent decode call.
    pub fn decode_frame_output(&self) -> &[f32] {
        &self.out
    }

    fn count_tools(&mut self) {
        use crate::ics::{
            EIGHT_SHORT_SEQUENCE, INTENSITY_HCB, INTENSITY_HCB2, LONG_START_SEQUENCE, LONG_STOP_SEQUENCE, NOISE_HCB,
        };
        let s = &mut self.stats;
        s.frames += 1;
        s.coupling_elements += self.cces.iter().filter(|c| c.present).count() as u64;
        for e in self.elements.iter().filter(|e| e.present) {
            for c in &e.ch {
                let f = &c.frame;
                s.channel_frames += 1;
                match f.ics.window_sequence {
                    LONG_START_SEQUENCE => s.long_start_windows += 1,
                    EIGHT_SHORT_SEQUENCE => s.eight_short_windows += 1,
                    LONG_STOP_SEQUENCE => s.long_stop_windows += 1,
                    _ => {}
                }
                s.kbd_windows += f.ics.window_shape as u64;
                s.tns_channel_frames += f.tns.present as u64;
                s.pulse_channel_frames += (f.pulses.count > 0) as u64;
                for g in 0..f.ics.num_window_groups {
                    for sfb in 0..f.ics.max_sfb {
                        let cb = f.band_type[g * BAND_STRIDE + sfb];
                        s.noise_bands += (cb == NOISE_HCB) as u64;
                        s.intensity_bands += (cb == INTENSITY_HCB || cb == INTENSITY_HCB2) as u64;
                    }
                }
            }
            if e.common_window {
                let ics = &e.ch[0].frame.ics;
                for g in 0..ics.num_window_groups {
                    for sfb in 0..ics.max_sfb {
                        s.ms_bands += e.ms.used(g * BAND_STRIDE + sfb) as u64;
                    }
                }
            }
            if let Some(sbr) = &e.sbr {
                s.sbr_element_frames += (e.sbr_data && sbr.has_header()) as u64;
                s.sbr_coupled_frames += (e.sbr_data && e.ch.len() == 2 && sbr.coupled()) as u64;
                if self.ps_active {
                    if let Some((is34, ipd)) = sbr.ps_mode() {
                        s.ps_frames += 1;
                        s.ps_34_band_frames += is34 as u64;
                        s.ps_ipd_opd_frames += ipd as u64;
                    }
                }
            }
        }
    }

    fn find_element(&self, kind: ElementKind, tag: u8) -> Result<usize> {
        let free = |e: &&Element| !e.present && e.slot.kind == kind;
        if let Some(i) = self.elements.iter().position(|e| free(&e) && e.slot.tag == Some(tag)) {
            return Ok(i);
        }
        if let Some(i) = self.elements.iter().position(|e| free(&e)) {
            return Ok(i);
        }
        invalid!("channel element does not match the channel configuration")
    }

    fn parse_raw_data_block(&mut self, br: &mut BitReader) -> Result<()> {
        for e in &mut self.elements {
            e.present = false;
            e.sbr_data = false;
        }
        for c in &mut self.cces {
            c.present = false;
        }
        let sfi = self.asc.sampling_index;
        let mut last: Option<usize> = None;
        loop {
            let id = br.read(3)?;
            match id {
                ID_SCE | ID_LFE => {
                    let tag = br.read(4)? as u8;
                    let kind = if id == ID_SCE { ElementKind::Sce } else { ElementKind::Lfe };
                    let i = self.find_element(kind, tag)?;
                    let e = &mut self.elements[i];
                    e.tag = tag;
                    e.common_window = false;
                    e.ms.present = 0;
                    e.ch[0].frame.parse(br, sfi, false)?;
                    e.present = true;
                    last = Some(i);
                }
                ID_CPE => {
                    let tag = br.read(4)? as u8;
                    let i = self.find_element(ElementKind::Cpe, tag)?;
                    let e = &mut self.elements[i];
                    e.tag = tag;
                    e.common_window = br.read_bit()?;
                    e.ms.present = 0;
                    if e.common_window {
                        let ics = IcsInfo::parse(br, sfi)?;
                        e.ms.present = br.read(2)? as u8;
                        if e.ms.present == 3 {
                            invalid!("reserved ms_mask_present");
                        }
                        if e.ms.present == 1 {
                            for g in 0..ics.num_window_groups {
                                for sfb in 0..ics.max_sfb {
                                    e.ms.used[g * BAND_STRIDE + sfb] = br.read_bit()?;
                                }
                            }
                        }
                        e.ch[0].frame.ics = ics.clone();
                        e.ch[1].frame.ics = ics;
                    }
                    let cw = e.common_window;
                    e.ch[0].frame.parse(br, sfi, cw)?;
                    e.ch[1].frame.parse(br, sfi, cw)?;
                    e.present = true;
                    last = Some(i);
                }
                ID_CCE => {
                    let tag = br.read(4)? as usize;
                    self.cces[tag].parse(br, sfi)?;
                }
                ID_DSE => {
                    br.skip(4)?; // element_instance_tag
                    let align = br.read_bit()?;
                    let mut count = br.read_usize(8)?;
                    if count == 255 {
                        count += br.read_usize(8)?;
                    }
                    if align {
                        br.byte_align()?;
                    }
                    br.skip(8 * count)?;
                }
                ID_PCE => {
                    // In-band PCE: validated; the layout comes from the ASC.
                    ProgramConfig::parse(br)?;
                }
                ID_FIL => {
                    let mut count = br.read_usize(4)?;
                    if count == 15 {
                        count += br.read_usize(8)?;
                        count -= 1;
                    }
                    if count > 0 {
                        let mut payload = br.take(8 * count)?;
                        self.parse_extension(&mut payload, last)?;
                    }
                }
                ID_END => break,
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    /// extension_payload() of a fill element. SBR payloads attach to the
    /// preceding SCE/CPE.
    fn parse_extension(&mut self, br: &mut BitReader, last: Option<usize>) -> Result<()> {
        let ext = br.read(4)?;
        if ext != EXT_SBR_DATA && ext != EXT_SBR_DATA_CRC {
            // Fill, fill data, data element, dynamic range info (not
            // applied), SAC data: nothing to reconstruct.
            return Ok(());
        }
        let Some(i) = last else {
            invalid!("SBR payload without a preceding channel element");
        };
        if self.asc.sbr == Signal::Absent {
            return Ok(());
        }
        if !self.sbr_active {
            // Implicit signalling: SBR found in the bitstream.
            self.enable_sbr()?;
            self.update_format();
        }
        let crc = ext == EXT_SBR_DATA_CRC;
        if crc {
            br.skip(10)?; // bs_sbr_crc_bits
        }
        let kind = self.elements[i].slot.kind;
        let allow_ps = self.asc.ps != Signal::Absent && self.is_mono();
        let e = &mut self.elements[i];
        let sbr = e.sbr.as_mut().expect("SBR state allocated by enable_sbr");
        sbr.parse(br, kind, allow_ps)?;
        e.sbr_data = true;
        if sbr.ps_seen() && !self.ps_active {
            // Implicit PS: switch the mono stream to stereo output.
            self.ps_active = true;
            self.update_format();
        }
        Ok(())
    }

    /// Dequantisation, stereo tools, coupling, TNS, filterbank, SBR/PS and
    /// interleaving of everything parsed for the current frame.
    fn reconstruct(&mut self) -> Result<()> {
        // Coupling channels first: their spectra / time signals feed targets.
        for cce in &mut self.cces {
            if !cce.present {
                continue;
            }
            spectral::dequantize(&mut cce.ch);
            pns::fill(&mut cce.ch, &mut self.rng);
            let ics = cce.ch.ics.clone();
            tns::apply(&cce.ch.tns, &ics, &mut cce.ch.spec);
            if cce.point == CouplingPoint::AfterImdct {
                self.fb.synthesize(&ics, &cce.ch.spec, &mut cce.overlap, &mut cce.prev_shape, &mut cce.time);
            }
        }
        for ei in 0..self.elements.len() {
            let e = &mut self.elements[ei];
            if !e.present {
                continue;
            }
            for c in &mut e.ch {
                spectral::dequantize(&mut c.frame);
                pns::fill(&mut c.frame, &mut self.rng);
            }
            if e.ch.len() == 2 && e.common_window {
                let (a, b) = e.ch.split_at_mut(1);
                stereo::apply(&mut a[0].frame, &mut b[0].frame, &e.ms);
            }
            let is_cpe = e.slot.kind == ElementKind::Cpe;
            let tag = e.tag;
            for (ci, c) in e.ch.iter_mut().enumerate() {
                Self::couple(&self.cces, is_cpe, tag, ci, CouplingPoint::BeforeTns, &mut c.frame.spec);
                let ics = c.frame.ics.clone();
                tns::apply(&c.frame.tns, &ics, &mut c.frame.spec);
                Self::couple(&self.cces, is_cpe, tag, ci, CouplingPoint::BetweenTnsAndImdct, &mut c.frame.spec);
                self.fb.synthesize(&ics, &c.frame.spec, &mut c.overlap, &mut c.prev_shape, &mut c.time);
                Self::couple(&self.cces, is_cpe, tag, ci, CouplingPoint::AfterImdct, &mut c.time);
                sanitize(&mut c.time, CORE_LIMIT);
                sanitize(&mut c.overlap, CORE_LIMIT);
            }
        }
        // Post-processing into the planar output buffers.
        let frame_len = self.frame_len;
        for e in &mut self.elements {
            let oc = e.out_channel;
            if !e.present {
                for k in 0..e.ch.len() {
                    if let Some(p) = self.planar.get_mut(oc + k) {
                        p.fill(0.0);
                    }
                }
                continue;
            }
            if self.sbr_active {
                let sbr = e.sbr.as_mut().expect("SBR state allocated");
                let (l, r) = if e.ch.len() == 2 {
                    let (a, b) = e.ch.split_at(1);
                    (&a[0].time, Some(&b[0].time))
                } else {
                    (&e.ch[0].time, None)
                };
                let (pl, pr) = if self.planar.len() > oc + 1 {
                    let (x, y) = self.planar.split_at_mut(oc + 1);
                    (&mut x[oc], Some(&mut y[0]))
                } else {
                    (&mut self.planar[oc], None)
                };
                let want_ps = self.ps_active && e.ch.len() == 1;
                sbr.process(l, r, e.sbr_data, pl, if e.ch.len() == 2 || want_ps { pr } else { None }, want_ps)?;
            } else {
                for (k, c) in e.ch.iter().enumerate() {
                    self.planar[oc + k][..frame_len].copy_from_slice(&c.time[..frame_len]);
                }
            }
        }
        let nch = self.channels;
        self.out.resize(nch * frame_len, 0.0);
        for (c, p) in self.planar.iter_mut().enumerate() {
            sanitize(p, CORE_LIMIT);
            for (i, v) in p.iter().enumerate() {
                self.out[i * nch + c] = v * OUTPUT_SCALE;
            }
        }
        Ok(())
    }

    fn couple(cces: &[Cce], is_cpe: bool, tag: u8, ch: usize, point: CouplingPoint, target: &mut [f32]) {
        for cce in cces.iter().filter(|c| c.present && c.point == point) {
            for list in cce.lists_for(is_cpe, tag, ch) {
                if point == CouplingPoint::AfterImdct {
                    cce.apply_time(list, target);
                } else {
                    cce.apply_spectral(list, target);
                }
            }
        }
    }
}
