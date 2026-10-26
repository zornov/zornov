// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AIFF (Audio Interchange File Format) 16-bit PCM writing and reading.
//! Big-endian samples; the sample rate is an 80-bit IEEE extended float.

use std::io::{self, Seek, SeekFrom, Write};

/// 80-bit extended representation of a positive integer sample rate.
pub fn extended(rate: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if rate == 0 {
        return out;
    }
    let e = 31 - rate.leading_zeros(); // floor(log2)
    let exp = (16383 + e) as u16;
    let mant = u64::from(rate) << (63 - e);
    out[..2].copy_from_slice(&exp.to_be_bytes());
    out[2..].copy_from_slice(&mant.to_be_bytes());
    out
}

fn from_extended(b: &[u8]) -> f64 {
    let exp = i32::from(u16::from_be_bytes([b[0], b[1]]) & 0x7FFF);
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    mant as f64 * 2f64.powi(exp - 16383 - 63)
}

fn header(channels: u16, rate: u32, frames: u32) -> Vec<u8> {
    let data = frames as u64 * u64::from(channels) * 2;
    let ssnd_len = 8 + data;
    let form_len = 4 + (8 + 18) + (8 + ssnd_len);
    let mut h = Vec::with_capacity(54);
    h.extend_from_slice(b"FORM");
    h.extend_from_slice(&(form_len.min(u64::from(u32::MAX)) as u32).to_be_bytes());
    h.extend_from_slice(b"AIFFCOMM");
    h.extend_from_slice(&18u32.to_be_bytes());
    h.extend_from_slice(&channels.to_be_bytes());
    h.extend_from_slice(&frames.to_be_bytes());
    h.extend_from_slice(&16u16.to_be_bytes());
    h.extend_from_slice(&extended(rate));
    h.extend_from_slice(b"SSND");
    h.extend_from_slice(&(ssnd_len.min(u64::from(u32::MAX)) as u32).to_be_bytes());
    h.extend_from_slice(&[0; 8]); // offset, block size
    h
}

/// Streams interleaved samples into an AIFF; sizes are patched on finish.
pub struct AiffWriter<W: Write + Seek> {
    w: W,
    channels: u16,
    rate: u32,
    samples: u64,
}

impl<W: Write + Seek> AiffWriter<W> {
    pub fn new(mut w: W, rate: u32, channels: u16) -> io::Result<Self> {
        if channels == 0 || rate == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsupported AIFF format"));
        }
        w.write_all(&header(channels, rate, 0))?;
        Ok(Self { w, channels, rate, samples: 0 })
    }

    /// Interleaved samples in [-1, 1], quantised like the WAV writer.
    pub fn push_f32(&mut self, samples: &[f32]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            buf.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_be_bytes());
        }
        self.samples += samples.len() as u64;
        self.w.write_all(&buf)
    }

    pub fn finish(mut self) -> io::Result<W> {
        let frames = (self.samples / u64::from(self.channels)).min(u64::from(u32::MAX)) as u32;
        if self.samples % 2 == 1 {
            // Chunks are padded to an even length (only odd for mono 8-bit, kept for safety).
            self.w.write_all(&[0])?;
        }
        self.w.seek(SeekFrom::Start(0))?;
        self.w.write_all(&header(self.channels, self.rate, frames))?;
        self.w.seek(SeekFrom::End(0))?;
        self.w.flush()?;
        Ok(self.w)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Aiff {
    pub sample_rate: f64,
    pub channels: u16,
    pub bits: u16,
    /// Interleaved samples in [-1, 1].
    pub samples: Vec<f32>,
}

/// Reads 8/16/24/32-bit PCM AIFF (and uncompressed AIFF-C `NONE`/`twos`).
pub fn parse(b: &[u8]) -> Result<Aiff, String> {
    if b.len() < 12 || &b[..4] != b"FORM" || !(&b[8..12] == b"AIFF" || &b[8..12] == b"AIFC") {
        return Err("not an AIFF file".into());
    }
    let mut pos = 12;
    let (mut ch, mut frames, mut bits, mut rate) = (0u16, 0u32, 0u16, 0f64);
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_be_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        match id {
            b"COMM" if body.len() >= 18 => {
                ch = u16::from_be_bytes([body[0], body[1]]);
                frames = u32::from_be_bytes([body[2], body[3], body[4], body[5]]);
                bits = u16::from_be_bytes([body[6], body[7]]);
                rate = from_extended(&body[8..18]);
                if &b[8..12] == b"AIFC" && body.len() >= 22 && !matches!(&body[18..22], b"NONE" | b"twos") {
                    return Err("compressed AIFF-C is not supported".into());
                }
            }
            b"SSND" if body.len() >= 8 => {
                let off = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
                data = body.get(8 + off..);
            }
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let data = data.ok_or("AIFF has no sound data")?;
    if ch == 0 || !(8..=32).contains(&bits) {
        return Err("AIFF has an unsupported format".into());
    }
    let bytes = usize::from(bits).div_ceil(8);
    let n = (frames as usize * usize::from(ch)).min(data.len() / bytes);
    let scale = 2f32.powi(i32::from(bits) - 1);
    let samples = data
        .chunks_exact(bytes)
        .take(n)
        .map(|s| {
            let mut v = i32::from(s[0] as i8);
            for &x in &s[1..] {
                v = (v << 8) | i32::from(x);
            }
            v as f32 / (scale * 2f32.powi(8 * bytes as i32 - i32::from(bits)))
        })
        .collect();
    Ok(Aiff { sample_rate: rate, channels: ch, bits, samples })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn extended_rates() {
        assert_eq!(extended(44_100), [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(extended(48_000), [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
        for r in [8_000, 22_050, 48_000, 96_000, 192_000] {
            assert_eq!(from_extended(&extended(r)), f64::from(r));
        }
    }

    #[test]
    fn round_trip() {
        let s: Vec<f32> = (0..2000).map(|i| ((i as f32) * 0.01).sin() * 0.8).collect();
        let mut w = AiffWriter::new(Cursor::new(Vec::new()), 48_000, 2).unwrap();
        w.push_f32(&s[..777]).unwrap();
        w.push_f32(&s[777..]).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        assert_eq!(bytes.len(), 54 + 4000);
        let a = parse(&bytes).unwrap();
        assert_eq!((a.sample_rate, a.channels, a.bits, a.samples.len()), (48_000.0, 2, 16, 2000));
        for (x, y) in s.iter().zip(&a.samples) {
            assert!((x - y).abs() < 1.0 / 16_000.0);
        }
        assert!(parse(b"RIFF....WAVE").is_err());
    }
}
