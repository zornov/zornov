// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MPEG-4 audio configuration: the `esds` descriptor chain (ISO/IEC 14496-1)
//! and `AudioSpecificConfig` (ISO/IEC 14496-3 §1.6.2.1), including explicit
//! hierarchical and backward-compatible SBR / PS signalling.

use crate::r#box::Reader;
use crate::error::{Error, Result};

/// The interesting parts of an `ES_Descriptor`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EsDescriptor {
    /// `objectTypeIndication` (0x40 = MPEG-4 audio, 0x66..0x68 = MPEG-2 AAC).
    pub object_type_indication: u8,
    pub stream_type: u8,
    pub buffer_size: u32,
    pub max_bitrate: u32,
    pub avg_bitrate: u32,
    /// `DecoderSpecificInfo` payload (the AudioSpecificConfig for AAC).
    pub decoder_specific_info: Vec<u8>,
}

impl EsDescriptor {
    /// True when the stream is AAC (MPEG-4 audio or MPEG-2 AAC profiles).
    pub fn is_aac(&self) -> bool {
        matches!(self.object_type_indication, 0x40 | 0x66 | 0x67 | 0x68)
    }
}

/// Parses an `esds` box payload (full-box header included).
pub fn parse_esds(data: &[u8]) -> Result<EsDescriptor> {
    let mut r = Reader::new(data, "esds");
    r.full_header()?;
    let (tag, body) = descriptor(&mut r)?;
    if tag != 0x03 {
        return Err(Error::malformed("esds does not start with ES_Descriptor"));
    }
    let mut es = Reader::new(body, "ES_Descriptor");
    es.u16()?; // ES_ID
    let flags = es.u8()?;
    if flags & 0x80 != 0 {
        es.u16()?; // dependsOn_ES_ID
    }
    if flags & 0x40 != 0 {
        let n = es.u8()? as usize;
        es.skip(n)?; // URL
    }
    if flags & 0x20 != 0 {
        es.u16()?; // OCR_ES_Id
    }
    while es.remaining() > 0 {
        let (tag, body) = descriptor(&mut es)?;
        if tag != 0x04 {
            continue;
        }
        let mut dc = Reader::new(body, "DecoderConfigDescriptor");
        let mut out = EsDescriptor {
            object_type_indication: dc.u8()?,
            stream_type: dc.u8()? >> 2,
            buffer_size: dc.u24()?,
            max_bitrate: dc.u32()?,
            avg_bitrate: dc.u32()?,
            decoder_specific_info: Vec::new(),
        };
        while dc.remaining() > 0 {
            let (tag, body) = descriptor(&mut dc)?;
            if tag == 0x05 {
                out.decoder_specific_info = body.to_vec();
                break;
            }
        }
        return Ok(out);
    }
    Err(Error::Missing("DecoderConfigDescriptor"))
}

/// Reads a descriptor tag and its 1-4 byte expandable length.
fn descriptor<'a>(r: &mut Reader<'a>) -> Result<(u8, &'a [u8])> {
    let tag = r.u8()?;
    let mut len = 0usize;
    for i in 0..4 {
        let b = r.u8()?;
        len = (len << 7) | (b & 0x7f) as usize;
        if b & 0x80 == 0 {
            break;
        }
        if i == 3 {
            return Err(Error::malformed("descriptor length longer than 4 bytes"));
        }
    }
    Ok((tag, r.bytes(len)?))
}

/// Parsed `AudioSpecificConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    /// Effective object type: 29 when PS is signalled, 5 for SBR, else the core
    /// type (2 = AAC LC).
    pub object_type: u8,
    /// Core (underlying) audio object type.
    pub core_object_type: u8,
    /// Core sampling rate.
    pub core_sample_rate: u32,
    /// Output sampling rate (the SBR rate when SBR is signalled).
    pub sample_rate: u32,
    pub channel_configuration: u8,
    /// Output channel count (PS upmixes mono to stereo).
    pub channels: u16,
    pub sbr: bool,
    pub ps: bool,
    /// 960-sample frames instead of 1024.
    pub frame_length_960: bool,
}

const RATES: [u32; 13] =
    [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    fn get(&mut self, n: usize) -> Result<u32> {
        if n > self.left() {
            return Err(Error::Truncated("AudioSpecificConfig"));
        }
        let mut v = 0u32;
        for _ in 0..n {
            let bit = (self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Ok(v)
    }

    fn object_type(&mut self) -> Result<u8> {
        let t = self.get(5)?;
        Ok(if t == 31 { 32 + self.get(6)? } else { t } as u8)
    }

    fn rate(&mut self) -> Result<u32> {
        match self.get(4)? {
            15 => self.get(24),
            i if (i as usize) < RATES.len() => Ok(RATES[i as usize]),
            _ => Err(Error::malformed("reserved AAC sampling frequency index")),
        }
    }
}

fn channels_for_config(cfg: u8) -> u16 {
    match cfg {
        1..=6 => cfg as u16,
        7 | 12 | 14 => 8,
        11 => 7,
        13 => 24,
        _ => 0,
    }
}

impl AudioSpecificConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut b = Bits { data, pos: 0 };
        let mut aot = b.object_type()?;
        let core_sample_rate = b.rate()?;
        let channel_configuration = b.get(4)? as u8;
        let mut ext_rate = None;
        let (mut sbr, mut ps) = (false, false);
        if aot == 5 || aot == 29 {
            sbr = true;
            ps = aot == 29;
            ext_rate = Some(b.rate()?);
            aot = b.object_type()?;
            if aot == 22 {
                b.get(4)?; // extensionChannelConfiguration
            }
        }
        if aot == 0 {
            return Err(Error::malformed("AAC object type 0"));
        }
        let mut channels = channels_for_config(channel_configuration);
        let mut frame_length_960 = false;
        let ga = matches!(aot, 1..=4 | 6 | 7 | 17 | 19..=23);
        if ga {
            frame_length_960 = b.get(1)? == 1;
            if b.get(1)? == 1 {
                b.get(14)?; // coreCoderDelay
            }
            let extension = b.get(1)? == 1;
            if channel_configuration == 0 {
                channels = program_config_channels(&mut b)?;
            }
            if aot == 6 || aot == 20 {
                b.get(3)?; // layerNr
            }
            if extension {
                if aot == 22 {
                    b.get(16)?;
                }
                if matches!(aot, 17 | 19 | 20 | 23) {
                    b.get(3)?;
                }
                b.get(1)?; // extensionFlag3
            }
            if matches!(aot, 17 | 19..=23) {
                b.get(2)?; // epConfig
            }
            // Backward-compatible SBR / PS signalling.
            if !sbr && b.left() >= 16 && b.get(11)? == 0x2b7 {
                let ext_aot = b.object_type()?;
                if ext_aot == 5 && b.get(1)? == 1 {
                    sbr = true;
                    ext_rate = Some(b.rate()?);
                    if b.left() >= 12 && b.get(11)? == 0x548 {
                        ps = b.get(1)? == 1;
                    }
                }
            }
        }
        if sbr && ext_rate.is_none() {
            ext_rate = Some(core_sample_rate * 2);
        }
        if ps && channels == 1 {
            channels = 2;
        }
        Ok(AudioSpecificConfig {
            object_type: if ps { 29 } else if sbr { 5 } else { aot },
            core_object_type: aot,
            core_sample_rate,
            sample_rate: if sbr { ext_rate.unwrap_or(core_sample_rate) } else { core_sample_rate },
            channel_configuration,
            channels,
            sbr,
            ps,
            frame_length_960,
        })
    }
}

/// Parses a `program_config_element` and returns its channel count.
fn program_config_channels(b: &mut Bits<'_>) -> Result<u16> {
    b.get(4 + 2 + 4)?; // element_instance_tag, object_type, sampling_frequency_index
    let front = b.get(4)?;
    let side = b.get(4)?;
    let back = b.get(4)?;
    let lfe = b.get(2)?;
    let assoc = b.get(3)?;
    let cc = b.get(4)?;
    if b.get(1)? == 1 {
        b.get(4)?; // mono_mixdown_element_number
    }
    if b.get(1)? == 1 {
        b.get(4)?; // stereo_mixdown_element_number
    }
    if b.get(1)? == 1 {
        b.get(3)?; // matrix_mixdown_idx, pseudo_surround_enable
    }
    let mut channels = 0u16;
    for _ in 0..front + side + back {
        let is_cpe = b.get(1)?;
        b.get(4)?;
        channels += if is_cpe == 1 { 2 } else { 1 };
    }
    for _ in 0..lfe {
        b.get(4)?;
        channels += 1;
    }
    b.get((assoc * 4) as usize)?;
    b.get((cc * 5) as usize)?;
    Ok(channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lc_and_explicit_sbr() {
        let lc = AudioSpecificConfig::parse(&[0x12, 0x10]).unwrap();
        assert_eq!((lc.object_type, lc.sample_rate, lc.channels), (2, 44100, 2));
        // Hierarchical signalling: AOT 5, 24 kHz core -> 48 kHz output.
        let he = AudioSpecificConfig::parse(&[0x2b, 0x11, 0x88, 0x00]).unwrap();
        assert!(he.sbr && !he.ps);
        assert_eq!((he.object_type, he.core_sample_rate, he.sample_rate), (5, 24000, 48000));
        assert!(AudioSpecificConfig::parse(&[0x12]).is_err());
    }
}
