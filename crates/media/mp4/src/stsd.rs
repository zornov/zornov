// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sample descriptions (`stsd`): visual and sound sample entries, including
//! QuickTime version-1/2 sound descriptions and their child boxes
//! (`avcC`, `hvcC`, `av1C`, `esds`, `wave`, `chan`, `pasp`, `colr`, `btrt`).

use crate::aac::{parse_esds, AudioSpecificConfig};
use crate::r#box::{find, Boxes, FourCC, Reader};
use crate::codec::{Av1Config, AvcConfig, HevcConfig};
use crate::demux::{Bitrate, ChannelLayout, Codec, ColorInfo, TrackKind};
use crate::error::{Error, Result};

/// Everything the demuxer keeps from the first sample entry.
#[derive(Debug, Clone)]
pub(crate) struct SampleEntry {
    pub fourcc: FourCC,
    pub codec: Codec,
    pub coded_width: u32,
    pub coded_height: u32,
    pub pixel_aspect: Option<(u32, u32)>,
    pub color: Option<ColorInfo>,
    pub bitrate: Option<Bitrate>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub channel_layout: Option<ChannelLayout>,
}

impl SampleEntry {
    fn new(fourcc: FourCC) -> Self {
        SampleEntry {
            fourcc,
            codec: Codec::Other(fourcc),
            coded_width: 0,
            coded_height: 0,
            pixel_aspect: None,
            color: None,
            bitrate: None,
            sample_rate: 0,
            channels: 0,
            bits_per_sample: 0,
            channel_layout: None,
        }
    }
}

/// Parses `stsd` and returns its first entry. Further entries (rare
/// mid-stream format changes) are ignored.
pub(crate) fn parse_stsd(data: &[u8], kind: TrackKind) -> Result<SampleEntry> {
    let mut r = Reader::new(data, "stsd");
    r.full_header()?;
    let count = r.u32()?;
    if count == 0 {
        return Err(Error::Missing("sample entry"));
    }
    let first = Boxes::new(r.rest()).next().ok_or(Error::Missing("sample entry"))??;
    match kind {
        TrackKind::Video => parse_visual(first.kind, first.data),
        TrackKind::Audio => parse_sound(first.kind, first.data),
        TrackKind::Other => Ok(SampleEntry::new(first.kind)),
    }
}

fn parse_visual(fourcc: FourCC, data: &[u8]) -> Result<SampleEntry> {
    let mut e = SampleEntry::new(fourcc);
    let mut r = Reader::new(data, "visual sample entry");
    r.skip(8 + 16)?; // reserved, data_reference_index, pre_defined/reserved
    e.coded_width = r.u16()? as u32;
    e.coded_height = r.u16()? as u32;
    r.skip(4 + 4 + 4 + 2 + 32)?; // resolutions, reserved, frame_count, compressorname
    e.bits_per_sample = r.u16()?; // depth
    r.u16()?;
    let children = r.rest();
    for b in Boxes::new(children) {
        let b = b?;
        match &b.kind {
            b"avcC" if matches!(&fourcc, b"avc1" | b"avc3") => {
                e.codec = Codec::Avc { config: AvcConfig::parse(b.data)? };
            }
            b"hvcC" if matches!(&fourcc, b"hvc1" | b"hev1") => {
                e.codec = Codec::Hevc { config: HevcConfig::parse(b.data)? };
            }
            b"av1C" if &fourcc == b"av01" => {
                e.codec = Codec::Av1 { config: Av1Config::parse(b.data)? };
            }
            b"pasp" => {
                let mut p = Reader::new(b.data, "pasp");
                let (h, v) = (p.u32()?, p.u32()?);
                if h != 0 && v != 0 {
                    e.pixel_aspect = Some((h, v));
                }
            }
            b"colr" => e.color = parse_colr(b.data)?,
            b"btrt" => e.bitrate = Some(parse_btrt(b.data)?),
            _ => {}
        }
    }
    if matches!(&fourcc, b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"av01")
        && matches!(e.codec, Codec::Other(_))
    {
        return Err(Error::Missing("avcC/hvcC/av1C decoder configuration"));
    }
    Ok(e)
}

fn parse_sound(fourcc: FourCC, data: &[u8]) -> Result<SampleEntry> {
    let mut e = SampleEntry::new(fourcc);
    let mut r = Reader::new(data, "sound sample entry");
    r.skip(8)?; // reserved, data_reference_index
    let version = r.u16()?;
    r.skip(2 + 4)?; // revision, vendor
    e.channels = r.u16()?;
    e.bits_per_sample = r.u16()?;
    r.skip(2 + 2)?; // compression_id, packet_size
    e.sample_rate = r.u32()? >> 16;
    match version {
        0 => {}
        1 => r.skip(16)?, // samples/bytes per packet, bytes per frame/sample
        2 => {
            r.u32()?; // sizeOfStructOnly
            let rate = f64::from_bits(r.u64()?);
            e.channels = u16::try_from(r.u32()?).map_err(|_| Error::malformed("channel count"))?;
            r.u32()?; // always 0x7F000000
            e.bits_per_sample = r.u32()? as u16;
            r.skip(12)?; // format flags, bytes per packet, frames per packet
            if rate.is_finite() && rate > 0.0 && rate < 1e7 {
                e.sample_rate = rate.round() as u32;
            }
        }
        v => return Err(Error::malformed(format!("sound sample description version {v}"))),
    }
    let children = r.rest();
    let mut esds = None;
    for b in Boxes::new(children) {
        let b = b?;
        match &b.kind {
            b"esds" => esds = Some(b.data),
            // QuickTime nests the codec configuration in 'wave'.
            b"wave" if esds.is_none() => esds = find(b.data, b"esds")?,
            b"chan" => e.channel_layout = Some(parse_chan(b.data)?),
            b"btrt" => e.bitrate = Some(parse_btrt(b.data)?),
            _ => {}
        }
    }
    if &fourcc == b"mp4a" {
        if let Some(esds) = esds {
            let es = parse_esds(esds)?;
            if e.bitrate.is_none() {
                e.bitrate = Some(Bitrate {
                    buffer_size: es.buffer_size,
                    max: es.max_bitrate,
                    avg: es.avg_bitrate,
                });
            }
            if es.is_aac() && !es.decoder_specific_info.is_empty() {
                let asc = AudioSpecificConfig::parse(&es.decoder_specific_info)?;
                let channels = if asc.channels > 0 { asc.channels } else { e.channels };
                e.codec = Codec::Aac {
                    asc: es.decoder_specific_info,
                    object_type: asc.object_type,
                    sample_rate: asc.sample_rate,
                    channels,
                };
            }
        }
    }
    Ok(e)
}

fn parse_colr(data: &[u8]) -> Result<Option<ColorInfo>> {
    let mut r = Reader::new(data, "colr");
    let kind = r.fourcc()?;
    Ok(match &kind {
        b"nclx" | b"nclc" => Some(ColorInfo::Nclx {
            primaries: r.u16()?,
            transfer: r.u16()?,
            matrix: r.u16()?,
            full_range: if &kind == b"nclx" { r.u8()? & 0x80 != 0 } else { false },
        }),
        b"rICC" | b"prof" => Some(ColorInfo::Icc(r.rest().to_vec())),
        _ => None,
    })
}

fn parse_btrt(data: &[u8]) -> Result<Bitrate> {
    let mut r = Reader::new(data, "btrt");
    Ok(Bitrate { buffer_size: r.u32()?, max: r.u32()?, avg: r.u32()? })
}

fn parse_chan(data: &[u8]) -> Result<ChannelLayout> {
    let mut r = Reader::new(data, "chan");
    r.full_header()?;
    let tag = r.u32()?;
    let bitmap = r.u32()?;
    let n = r.count(20)?;
    let mut labels = Vec::with_capacity(n);
    for _ in 0..n {
        labels.push(r.u32()?);
        r.skip(16)?; // flags, coordinates
    }
    Ok(ChannelLayout { tag, bitmap, labels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn quicktime_v1_sound_description_with_wave_and_chan() {
        let mut entry = vec![0u8; 6];
        entry.extend([0, 1, 0, 1, 0, 0]); // dref index, version 1, revision
        entry.extend(b"appl");
        entry.extend([0, 2, 0, 16, 0xff, 0xfe, 0, 0]); // channels, bits, compression id, packet
        entry.extend(48000u32.wrapping_shl(16).to_be_bytes());
        entry.extend([0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]); // v1 fields
        let mut esds = vec![0, 0, 0, 0, 0x03, 0x16, 0, 0, 0, 0x04, 0x11, 0x40, 0x15];
        esds.extend([0, 0, 0, 0, 0, 0xfa, 0, 0, 0, 0xfa, 0, 0x05, 0x02, 0x11, 0x90]);
        let wave = [bx(b"frma", b"mp4a"), bx(b"esds", &esds), vec![0, 0, 0, 8, 0, 0, 0, 0]].concat();
        entry.extend(bx(b"wave", &wave));
        let chan = [0u32, 101 << 16 | 2, 0, 0].iter().flat_map(|w| w.to_be_bytes()).collect::<Vec<_>>();
        entry.extend(bx(b"chan", &chan));
        let stsd = [vec![0, 0, 0, 0, 0, 0, 0, 1], bx(b"mp4a", &entry)].concat();

        let e = parse_stsd(&stsd, TrackKind::Audio).unwrap();
        assert_eq!((e.sample_rate, e.channels, e.bits_per_sample), (48000, 2, 16));
        assert_eq!(
            e.codec,
            Codec::Aac { asc: vec![0x11, 0x90], object_type: 2, sample_rate: 48000, channels: 2 }
        );
        assert_eq!(e.channel_layout.map(|c| c.tag), Some(101 << 16 | 2));
        assert_eq!(e.bitrate.map(|b| b.avg), Some(0xfa00));
    }
}
