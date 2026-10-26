// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AudioSpecificConfig (ISO/IEC 14496-3 1.6.2.1), GASpecificConfig (4.4.1),
//! program_config_element (4.4.1.1) and the ADTS header (1.A.2.2).

use crate::bits::BitReader;
use crate::error::{invalid, Error, Result};

/// Sampling frequency per `sampling_frequency_index` (Table 1.18).
pub const SAMPLE_RATES: [u32; 13] =
    [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

pub const AOT_AAC_MAIN: u8 = 1;
pub const AOT_AAC_LC: u8 = 2;
pub const AOT_AAC_SSR: u8 = 3;
pub const AOT_AAC_LTP: u8 = 4;
pub const AOT_SBR: u8 = 5;
pub const AOT_PS: u8 = 29;

/// Whether a tool is signalled in the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Explicitly signalled as present.
    Present,
    /// Explicitly signalled as absent.
    Absent,
    /// Not signalled: the tool may appear implicitly in the bitstream.
    Unknown,
}

/// The sampling-frequency index used for band tables of a (possibly
/// non-standard) sample rate (ISO/IEC 14496-3 Table 4.82).
pub fn rate_to_index(rate: u32) -> usize {
    const LIMITS: [u32; 11] = [92017, 75132, 55426, 46009, 37566, 27713, 23004, 18783, 13856, 11502, 9391];
    LIMITS.iter().position(|&l| rate >= l).unwrap_or(11)
}

/// Kind of a syntactic element that carries audio channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    /// single_channel_element
    Sce,
    /// channel_pair_element
    Cpe,
    /// lfe_channel_element
    Lfe,
}

impl ElementKind {
    pub fn channels(self) -> usize {
        if self == ElementKind::Cpe { 2 } else { 1 }
    }
}

/// Speaker position of an output channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    FrontCenter,
    FrontLeft,
    FrontRight,
    SideLeft,
    SideRight,
    BackLeft,
    BackRight,
    BackCenter,
    Lfe,
    /// Position not described by the configuration (PCE "front/side/back"
    /// element beyond the usual layouts).
    Other,
}

/// One channel element slot of the output layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementSlot {
    pub kind: ElementKind,
    /// `element_instance_tag` when known (PCE layouts); `None` for the fixed
    /// `channelConfiguration` layouts, which match elements in order.
    pub tag: Option<u8>,
    pub speakers: Vec<Speaker>,
}

/// program_config_element()
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgramConfig {
    pub element_instance_tag: u8,
    pub object_type: u8,
    pub sampling_frequency_index: u8,
    /// (is_cpe, tag) per front, side and back element.
    pub front: Vec<(bool, u8)>,
    pub side: Vec<(bool, u8)>,
    pub back: Vec<(bool, u8)>,
    pub lfe: Vec<u8>,
    pub assoc_data: Vec<u8>,
    /// (is_ind_sw, tag) per valid coupling channel element.
    pub cc: Vec<(bool, u8)>,
    pub mono_mixdown: Option<u8>,
    pub stereo_mixdown: Option<u8>,
    /// (matrix_mixdown_idx, pseudo_surround_enable)
    pub matrix_mixdown: Option<(u8, bool)>,
    pub comment: Vec<u8>,
}

impl ProgramConfig {
    pub(crate) fn parse(br: &mut BitReader) -> Result<ProgramConfig> {
        let mut p = ProgramConfig {
            element_instance_tag: br.read(4)? as u8,
            object_type: br.read(2)? as u8,
            sampling_frequency_index: br.read(4)? as u8,
            ..Default::default()
        };
        let num_front = br.read(4)?;
        let num_side = br.read(4)?;
        let num_back = br.read(4)?;
        let num_lfe = br.read(2)?;
        let num_assoc = br.read(3)?;
        let num_cc = br.read(4)?;
        if br.read_bit()? {
            p.mono_mixdown = Some(br.read(4)? as u8);
        }
        if br.read_bit()? {
            p.stereo_mixdown = Some(br.read(4)? as u8);
        }
        if br.read_bit()? {
            let idx = br.read(2)? as u8;
            p.matrix_mixdown = Some((idx, br.read_bit()?));
        }
        for (n, v) in [(num_front, &mut p.front), (num_side, &mut p.side), (num_back, &mut p.back)] {
            for _ in 0..n {
                let cpe = br.read_bit()?;
                v.push((cpe, br.read(4)? as u8));
            }
        }
        for _ in 0..num_lfe {
            p.lfe.push(br.read(4)? as u8);
        }
        for _ in 0..num_assoc {
            p.assoc_data.push(br.read(4)? as u8);
        }
        for _ in 0..num_cc {
            let ind = br.read_bit()?;
            p.cc.push((ind, br.read(4)? as u8));
        }
        br.byte_align()?;
        let n = br.read(8)?;
        for _ in 0..n {
            p.comment.push(br.read(8)? as u8);
        }
        Ok(p)
    }

    /// Output layout in PCE order: front, side, back, LFE elements.
    pub fn layout(&self) -> Vec<ElementSlot> {
        let mut out = Vec::new();
        let mut push = |cpe: bool, tag: u8, mono: Speaker, l: Speaker, r: Speaker| {
            if cpe {
                out.push(ElementSlot { kind: ElementKind::Cpe, tag: Some(tag), speakers: vec![l, r] });
            } else {
                out.push(ElementSlot { kind: ElementKind::Sce, tag: Some(tag), speakers: vec![mono] });
            }
        };
        use Speaker::*;
        for &(cpe, tag) in &self.front {
            push(cpe, tag, FrontCenter, FrontLeft, FrontRight);
        }
        for &(cpe, tag) in &self.side {
            push(cpe, tag, Other, SideLeft, SideRight);
        }
        for &(cpe, tag) in &self.back {
            push(cpe, tag, BackCenter, BackLeft, BackRight);
        }
        for &tag in &self.lfe {
            out.push(ElementSlot { kind: ElementKind::Lfe, tag: Some(tag), speakers: vec![Lfe] });
        }
        out
    }
}

/// Layout of `channelConfiguration` 1..=7 (Table 1.19), in bitstream order.
pub fn channel_config_layout(cfg: u8) -> Option<Vec<ElementSlot>> {
    use ElementKind::{Cpe, Sce};
    use Speaker::*;
    let s = |kind, speakers: &[Speaker]| ElementSlot { kind, tag: None, speakers: speakers.to_vec() };
    let c = s(Sce, &[FrontCenter]);
    let lr = s(Cpe, &[FrontLeft, FrontRight]);
    Some(match cfg {
        1 => vec![c],
        2 => vec![lr],
        3 => vec![c, lr],
        4 => vec![c, lr, s(Sce, &[BackCenter])],
        5 => vec![c, lr, s(Cpe, &[BackLeft, BackRight])],
        6 => vec![c, lr, s(Cpe, &[BackLeft, BackRight]), s(ElementKind::Lfe, &[Speaker::Lfe])],
        7 => vec![c, lr, s(Cpe, &[SideLeft, SideRight]), s(Cpe, &[BackLeft, BackRight]), s(ElementKind::Lfe, &[Speaker::Lfe])],
        _ => return None,
    })
}

/// Parsed AudioSpecificConfig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    /// Object type of the core coder (2 = AAC LC). For SBR/PS streams with
    /// explicit hierarchical signalling this is the underlying core type.
    pub object_type: u8,
    /// Sample rate of the core coder.
    pub sample_rate: u32,
    /// Band-table index derived from the core sample rate.
    pub sampling_index: usize,
    pub channel_config: u8,
    pub sbr: Signal,
    pub ps: Signal,
    /// Output sample rate of the SBR tool when `sbr == Present`.
    pub ext_sample_rate: u32,
    pub frame_length_960: bool,
    pub depends_on_core_coder: bool,
    pub extension_flag: bool,
    pub pce: Option<ProgramConfig>,
}

fn read_aot(br: &mut BitReader) -> Result<u8> {
    let aot = br.read(5)? as u8;
    Ok(if aot == 31 { 32 + br.read(6)? as u8 } else { aot })
}

fn read_rate(br: &mut BitReader) -> Result<u32> {
    let idx = br.read(4)? as usize;
    match idx {
        0..=12 => Ok(SAMPLE_RATES[idx]),
        15 => {
            let r = br.read(24)?;
            if r == 0 {
                invalid!("zero sampling frequency");
            }
            Ok(r)
        }
        _ => invalid!("reserved sampling frequency index"),
    }
}

impl AudioSpecificConfig {
    /// Parse an AudioSpecificConfig (e.g. the payload of an MP4 `esds`
    /// DecoderSpecificInfo).
    pub fn parse(data: &[u8]) -> Result<AudioSpecificConfig> {
        let mut br = BitReader::new(data);
        let mut aot = read_aot(&mut br)?;
        let sample_rate = read_rate(&mut br)?;
        let channel_config = br.read(4)? as u8;
        let mut sbr = Signal::Unknown;
        let mut ps = Signal::Unknown;
        let mut ext_sample_rate = 0;
        let mut ext_aot = 0u8;
        if aot == AOT_SBR || aot == AOT_PS {
            ext_aot = AOT_SBR;
            sbr = Signal::Present;
            if aot == AOT_PS {
                ps = Signal::Present;
            }
            ext_sample_rate = read_rate(&mut br)?;
            aot = read_aot(&mut br)?;
            if aot == 22 {
                return Err(Error::Unsupported("ER BSAC"));
            }
        }
        match aot {
            AOT_AAC_LC => {}
            AOT_AAC_MAIN => return Err(Error::Unsupported("AAC Main profile (prediction)")),
            AOT_AAC_SSR => return Err(Error::Unsupported("AAC SSR profile (gain control)")),
            AOT_AAC_LTP => return Err(Error::Unsupported("AAC LTP profile")),
            17 | 19..=23 => return Err(Error::Unsupported("error resilient object types")),
            _ => return Err(Error::Unsupported("audio object type")),
        }
        // GASpecificConfig
        let frame_length_960 = br.read_bit()?;
        let depends_on_core_coder = br.read_bit()?;
        if depends_on_core_coder {
            br.skip(14)?; // coreCoderDelay
        }
        let extension_flag = br.read_bit()?;
        let pce = if channel_config == 0 { Some(ProgramConfig::parse(&mut br)?) } else { None };
        if extension_flag {
            br.skip(1)?; // extensionFlag3
        }
        // Backward compatible (implicit-hierarchy) extension signalling.
        if ext_aot != AOT_SBR && br.bits_left() >= 16 {
            let mut probe = br.clone();
            if probe.read(11)? == 0x2b7 {
                let ext = read_aot(&mut probe)?;
                if ext == AOT_SBR {
                    if probe.read_bit()? {
                        sbr = Signal::Present;
                        ext_sample_rate = read_rate(&mut probe)?;
                        if probe.bits_left() >= 12 && probe.read(11)? == 0x548 {
                            ps = if probe.read_bit()? { Signal::Present } else { Signal::Absent };
                        }
                    } else {
                        sbr = Signal::Absent;
                    }
                }
                let _ = probe;
            }
        }
        if frame_length_960 {
            return Err(Error::Unsupported("960-sample frames"));
        }
        if channel_config > 7 {
            return Err(Error::Unsupported("channel configuration > 7"));
        }
        if sample_rate > 96000 {
            invalid!("core sample rate above 96 kHz");
        }
        Ok(AudioSpecificConfig {
            object_type: aot,
            sample_rate,
            sampling_index: rate_to_index(sample_rate),
            channel_config,
            sbr,
            ps,
            ext_sample_rate,
            frame_length_960,
            depends_on_core_coder,
            extension_flag,
            pce,
        })
    }

    /// Element layout described by the configuration.
    pub fn layout(&self) -> Result<Vec<ElementSlot>> {
        if let Some(p) = &self.pce {
            let l = p.layout();
            if l.is_empty() {
                invalid!("program config element without channels");
            }
            return Ok(l);
        }
        channel_config_layout(self.channel_config).ok_or(Error::InvalidData("channel configuration 0 without PCE"))
    }

    /// Build a minimal AudioSpecificConfig for AAC LC (used for ADTS).
    pub fn lc(sample_rate_index: u8, channel_config: u8) -> Vec<u8> {
        let v: u16 = ((AOT_AAC_LC as u16) << 11) | ((sample_rate_index as u16) << 7) | ((channel_config as u16) << 3);
        v.to_be_bytes().to_vec()
    }
}

/// Fixed and variable part of an ADTS frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdtsHeader {
    pub mpeg2: bool,
    pub protection_absent: bool,
    /// Object type (profile + 1).
    pub object_type: u8,
    pub sampling_index: u8,
    pub channel_config: u8,
    /// Total frame length in bytes including the header.
    pub frame_length: usize,
    pub buffer_fullness: u16,
    pub num_raw_data_blocks: u8,
    /// Header length in bytes (7, or 9 with CRC).
    pub header_length: usize,
}

impl AdtsHeader {
    pub fn parse(data: &[u8]) -> Result<AdtsHeader> {
        let mut br = BitReader::new(data);
        if br.read(12)? != 0xfff {
            invalid!("ADTS sync word");
        }
        let mpeg2 = br.read_bit()?;
        if br.read(2)? != 0 {
            invalid!("ADTS layer");
        }
        let protection_absent = br.read_bit()?;
        let object_type = br.read(2)? as u8 + 1;
        let sampling_index = br.read(4)? as u8;
        if sampling_index > 12 {
            invalid!("ADTS sampling index");
        }
        br.skip(1)?; // private
        let channel_config = br.read(3)? as u8;
        br.skip(4)?; // original, home, copyright id bit/start
        let frame_length = br.read(13)? as usize;
        let buffer_fullness = br.read(11)? as u16;
        let num_raw_data_blocks = br.read(2)? as u8;
        let header_length = if protection_absent { 7 } else { 9 };
        if frame_length < header_length {
            invalid!("ADTS frame length");
        }
        Ok(AdtsHeader {
            mpeg2,
            protection_absent,
            object_type,
            sampling_index,
            channel_config,
            frame_length,
            buffer_fullness,
            num_raw_data_blocks,
            header_length,
        })
    }

    /// Equivalent AudioSpecificConfig bytes.
    pub fn audio_specific_config(&self) -> Vec<u8> {
        let v: u16 = ((self.object_type as u16) << 11) | ((self.sampling_index as u16) << 7) | ((self.channel_config as u16) << 3);
        v.to_be_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lc() {
        let c = AudioSpecificConfig::parse(&[0x13, 0x88]).unwrap();
        assert_eq!((c.object_type, c.sample_rate, c.channel_config), (2, 22050, 1));
        assert_eq!(c.sbr, Signal::Unknown);
        let c = AudioSpecificConfig::parse(&[0x11, 0x90]).unwrap();
        assert_eq!((c.sample_rate, c.channel_config), (48000, 2));
    }

    #[test]
    fn backward_compatible_sbr_ps() {
        let c = AudioSpecificConfig::parse(&[0x13, 0x90, 0x56, 0xe5, 0xa0]).unwrap();
        assert_eq!((c.sample_rate, c.ext_sample_rate, c.channel_config), (22050, 44100, 2));
        assert_eq!((c.sbr, c.ps), (Signal::Present, Signal::Unknown));
        let c = AudioSpecificConfig::parse(&[0x13, 0x88, 0x56, 0xe5, 0xa5, 0x48, 0x80]).unwrap();
        assert_eq!((c.sample_rate, c.ext_sample_rate, c.channel_config), (22050, 44100, 1));
        assert_eq!((c.sbr, c.ps), (Signal::Present, Signal::Present));
    }

    #[test]
    fn explicit_hierarchical() {
        // AOT 5, 24 kHz core (idx 6), 2 ch, ext idx 3 (48 kHz), AOT 2, GASpecific 000
        let mut bits: u64 = 0;
        let mut n: usize = 0;
        for (v, w) in [(5u64, 5), (6, 4), (2, 4), (3, 4), (2, 5), (0, 3)] {
            bits = (bits << w) | v;
            n += w;
        }
        bits <<= 32 - n;
        let b = (bits as u32).to_be_bytes();
        let c = AudioSpecificConfig::parse(&b[..n.div_ceil(8)]).unwrap();
        assert_eq!((c.object_type, c.sample_rate, c.ext_sample_rate), (2, 24000, 48000));
        assert_eq!(c.sbr, Signal::Present);
        // AOT 29 also flags PS.
        let mut bits: u64 = 0;
        for (v, w) in [(29u64, 5), (6, 4), (1, 4), (3, 4), (2, 5), (0, 3)] {
            bits = (bits << w) | v;
        }
        bits <<= 32 - n;
        let b = (bits as u32).to_be_bytes();
        let c = AudioSpecificConfig::parse(&b[..n.div_ceil(8)]).unwrap();
        assert_eq!((c.sbr, c.ps, c.channel_config), (Signal::Present, Signal::Present, 1));
    }

    #[test]
    fn escape_rate_and_errors() {
        // AOT 2, index 15 + 24-bit 44100, cfg 1
        let mut bits: u64 = 0;
        let mut n: usize = 0;
        for (v, w) in [(2u64, 5), (15, 4), (44100, 24), (1, 4), (0, 3)] {
            bits = (bits << w) | v;
            n += w;
        }
        bits <<= 64 - n;
        let b = bits.to_be_bytes();
        let c = AudioSpecificConfig::parse(&b[..n.div_ceil(8)]).unwrap();
        assert_eq!((c.sample_rate, c.sampling_index), (44100, 4));
        assert!(AudioSpecificConfig::parse(&[0x13]).is_err());
        assert!(AudioSpecificConfig::parse(&[]).is_err());
        assert!(matches!(AudioSpecificConfig::parse(&[0x0b, 0x88]), Err(Error::Unsupported(_)))); // Main
        assert_eq!(rate_to_index(8000), 11);
        assert_eq!(rate_to_index(50000), 3);
    }

    #[test]
    fn layouts() {
        assert_eq!(channel_config_layout(7).unwrap().iter().map(|s| s.kind.channels()).sum::<usize>(), 8);
        assert_eq!(channel_config_layout(6).unwrap().len(), 4);
        assert!(channel_config_layout(0).is_none());
    }

    #[test]
    fn adts_header() {
        // 44.1 kHz LC stereo, 371-byte frame, no CRC.
        let len = 371u64;
        let mut bits: u64 = 0;
        for (v, w) in [(0xfffu64, 12), (0, 1), (0, 2), (1, 1), (1, 2), (4, 4), (0, 1), (2, 3), (0, 4), (len, 13), (0x7ff, 11), (0, 2)] {
            bits = (bits << w) | v;
        }
        let b = &bits.to_be_bytes()[1..];
        let h = AdtsHeader::parse(b).unwrap();
        assert_eq!((h.object_type, h.sampling_index, h.channel_config, h.frame_length), (2, 4, 2, 371));
        assert_eq!(h.audio_specific_config(), vec![0x12, 0x10]);
        assert!(AdtsHeader::parse(&b[..5]).is_err());
    }
}
