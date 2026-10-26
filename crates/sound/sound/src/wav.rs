// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Delivery WAV files: 16/24-bit PCM or 32-bit float, any channel count,
//! `WAVE_FORMAT_EXTENSIBLE` with the speaker mask for surround layouts, and
//! an optional Broadcast Wave `bext` chunk (description, originator, the
//! timeline position of the first sample). Built in memory; the caller
//! writes the bytes.

use crate::surround::Layout;

/// Sample format of a written file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SampleFormat {
    /// 16-bit integer (dithered with TPDF noise).
    Pcm16,
    /// 24-bit integer, the usual delivery format.
    #[default]
    Pcm24,
    /// 32-bit float.
    Float32,
}

impl SampleFormat {
    fn bits(self) -> u16 {
        match self {
            SampleFormat::Pcm16 => 16,
            SampleFormat::Pcm24 => 24,
            SampleFormat::Float32 => 32,
        }
    }
}

/// Broadcast Wave metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bext {
    /// What the file is (up to 256 characters), e.g. `"Reel 1 DX stem"`.
    pub description: String,
    /// Who made it (up to 32).
    pub originator: String,
    /// Samples since midnight of the first sample (the timeline start).
    pub time_reference: u64,
}

/// A WAV file of planar `chans` at `rate`. `layout` sets the speaker mask
/// (and reorders 7.1 from crate to WAV order); `None` writes the channels
/// as they are with no speaker assignment.
pub fn write(chans: &[Vec<f32>], rate: u32, format: SampleFormat, layout: Option<Layout>, bext: Option<&Bext>) -> Vec<u8> {
    write_with(chans, rate, format, layout, bext, &[])
}

/// [`write`] with extra chunks (`id`, body) placed before the audio data
/// (the ADM writer adds `chna` and `axml` this way).
pub fn write_with(chans: &[Vec<f32>], rate: u32, format: SampleFormat, layout: Option<Layout>, bext: Option<&Bext>, extra: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let ch = chans.len().max(1);
    let frames = chans.iter().map(Vec::len).max().unwrap_or(0);
    let order: Vec<usize> = match layout {
        Some(l) if l.channels() == ch => l.wav_order().to_vec(),
        _ => (0..ch).collect(),
    };
    let bits = format.bits();
    let block = ch * bits as usize / 8;
    let data_len = frames * block;
    let mut out = Vec::with_capacity(data_len + 1024);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // fmt: extensible for more than two channels, more than 16 bits, or a
    // speaker layout.
    let extensible = ch > 2 || bits > 16 || layout.is_some();
    let mut fmt = Vec::new();
    let tag: u16 = if extensible { 0xFFFE } else if format == SampleFormat::Float32 { 3 } else { 1 };
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&(ch as u16).to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * block as u32).to_le_bytes());
    fmt.extend_from_slice(&(block as u16).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mask = match layout {
            Some(l) if l.channels() == ch => l.wav_channel_mask(),
            _ => 0,
        };
        fmt.extend_from_slice(&mask.to_le_bytes());
        // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT GUIDs.
        let first: u32 = if format == SampleFormat::Float32 { 3 } else { 1 };
        fmt.extend_from_slice(&first.to_le_bytes());
        fmt.extend_from_slice(&[0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
    }
    chunk(&mut out, b"fmt ", &fmt);
    if let Some(b) = bext {
        chunk(&mut out, b"bext", &bext_body(b));
    }
    for (id, body) in extra {
        chunk(&mut out, id, body);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    let mut seed = 0x1234_5678u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for i in 0..frames {
        for &c in &order {
            let v = chans.get(c).and_then(|x| x.get(i)).copied().unwrap_or(0.0);
            match format {
                SampleFormat::Pcm16 => {
                    let d = (rnd() - rnd()) / 32768.0;
                    let s = ((v + d).clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    out.extend_from_slice(&s.to_le_bytes());
                }
                SampleFormat::Pcm24 => {
                    let s = (v.clamp(-1.0, 1.0) as f64 * 8_388_607.0).round() as i32;
                    out.extend_from_slice(&s.to_le_bytes()[..3]);
                }
                SampleFormat::Float32 => out.extend_from_slice(&v.to_le_bytes()),
            }
        }
    }
    if data_len % 2 == 1 {
        out.push(0);
    }
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

fn fixed(s: &str, n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().filter(|b| b.is_ascii()).take(n).collect();
    v.resize(n, 0);
    v
}

/// EBU Tech 3285 `bext` version 1 body.
fn bext_body(b: &Bext) -> Vec<u8> {
    let mut v = Vec::with_capacity(602);
    v.extend(fixed(&b.description, 256));
    v.extend(fixed(&b.originator, 32));
    v.extend(fixed("", 32)); // originator reference
    v.extend(fixed("", 10)); // origination date yyyy-mm-dd
    v.extend(fixed("", 8)); // origination time
    v.extend_from_slice(&(b.time_reference as u32).to_le_bytes());
    v.extend_from_slice(&((b.time_reference >> 32) as u32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // version
    v.extend(std::iter::repeat_n(0u8, 64)); // UMID
    v.extend(std::iter::repeat_n(0u8, 10)); // loudness fields (unset)
    v.extend(std::iter::repeat_n(0u8, 180)); // reserved
    v
}

/// A parsed WAV, for tests and for reading stems back: planar float
/// channels in file order, rate, bits and the chunk ids seen.
pub struct Parsed {
    /// Channels in file order.
    pub chans: Vec<Vec<f32>>,
    /// Sample rate.
    pub rate: u32,
    /// Bits per sample.
    pub bits: u16,
    /// Speaker mask (0 when not extensible).
    pub mask: u32,
    /// Chunks in the file: id and body.
    pub chunks: Vec<([u8; 4], Vec<u8>)>,
}

/// Reads a file [`write`] produced (PCM 16/24 or float 32). `None` when it is
/// not a WAV this module can read.
pub fn parse(bytes: &[u8]) -> Option<Parsed> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let (mut ch, mut rate, mut bits, mut mask, mut float) = (0usize, 0u32, 0u16, 0u32, false);
    let mut chunks = Vec::new();
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id: [u8; 4] = bytes[at..at + 4].try_into().ok()?;
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..at + 8 + len)?;
        if &id == b"fmt " {
            let tag = u16::from_le_bytes([body[0], body[1]]);
            ch = u16::from_le_bytes([body[2], body[3]]) as usize;
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
            float = tag == 3;
            if tag == 0xFFFE && body.len() >= 40 {
                mask = u32::from_le_bytes(body[20..24].try_into().ok()?);
                float = u32::from_le_bytes(body[24..28].try_into().ok()?) == 3;
            }
        } else if &id == b"data" {
            data = Some(body);
        }
        chunks.push((id, body.to_vec()));
        at += 8 + len + (len & 1);
    }
    let data = data?;
    let bps = bits as usize / 8;
    if ch == 0 || bps == 0 {
        return None;
    }
    let frames = data.len() / (bps * ch);
    let mut chans = vec![Vec::with_capacity(frames); ch];
    for f in 0..frames {
        for (c, out) in chans.iter_mut().enumerate() {
            let o = (f * ch + c) * bps;
            let s = &data[o..o + bps];
            out.push(match (bits, float) {
                (16, _) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                (24, _) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                (32, true) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                (32, false) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                _ => return None,
            });
        }
    }
    Some(Parsed { chans, rate, bits, mask, chunks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surround_round_trips_in_wav_order() {
        let chans: Vec<Vec<f32>> = (0..8).map(|c| vec![c as f32 * 0.1; 10]).collect();
        let bytes = write(&chans, 48_000, SampleFormat::Pcm24, Some(Layout::Surround71), Some(&Bext { description: "DX".into(), originator: "Studio".into(), time_reference: 48_000 * 3600 }));
        let p = parse(&bytes).unwrap();
        assert_eq!((p.rate, p.bits, p.mask, p.chans.len()), (48_000, 24, 0x63F, 8));
        // WAV slot 4 (back left) carries crate channel 6 (Lrs).
        assert!((p.chans[4][0] - 0.6).abs() < 1e-6 && (p.chans[6][0] - 0.4).abs() < 1e-6);
        let bext = p.chunks.iter().find(|c| &c.0 == b"bext").unwrap();
        assert_eq!(&bext.1[..2], b"DX");
        assert_eq!(u32::from_le_bytes(bext.1[338..342].try_into().unwrap()), 48_000 * 3600);
    }

    #[test]
    fn stereo_16_bit_is_plain_pcm() {
        let bytes = write(&[vec![0.5, -0.5], vec![0.25, 0.0]], 44_100, SampleFormat::Pcm16, None, None);
        let p = parse(&bytes).unwrap();
        assert_eq!(u16::from_le_bytes([p.chunks[0].1[0], p.chunks[0].1[1]]), 1);
        assert!((p.chans[0][0] - 0.5).abs() < 1e-3 && (p.chans[1][0] - 0.25).abs() < 1e-3);
        let f = parse(&write(&[vec![0.123]], 48_000, SampleFormat::Float32, None, None)).unwrap();
        assert_eq!(f.chans[0][0], 0.123);
    }
}
