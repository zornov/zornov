// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RIFF/WAVE reading (PCM 8/16/24/32-bit, IEEE float, WAVE_FORMAT_EXTENSIBLE)
//! and 16-bit PCM writing.

use std::io;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples in [-1, 1].
    pub samples: Vec<f32>,
}

impl Wav {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
    pub fn duration_us(&self) -> i64 {
        (self.frames() as f64 * 1e6 / self.sample_rate.max(1) as f64).round() as i64
    }
    /// Sample (left, right) at frame `i`, mono duplicated, silence out of range.
    pub fn stereo_at(&self, i: usize) -> (f32, f32) {
        let ch = self.channels.max(1) as usize;
        if i >= self.frames() {
            return (0.0, 0.0);
        }
        let l = self.samples[i * ch];
        let r = if ch > 1 { self.samples[i * ch + 1] } else { l };
        (l, r)
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub fn parse(bytes: &[u8]) -> io::Result<Wav> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if tag == 0xfffe && body.len() >= 26 {
                    tag = u16::from_le_bytes([body[24], body[25]]);
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or_else(|| bad("WAV has no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("WAV has no data chunk"))?;
    if channels == 0 || rate == 0 {
        return Err(bad("WAV declares no channels or no sample rate"));
    }
    let samples: Vec<f32> = match (tag, bits) {
        (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 16) => data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 32) => data.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap()) as f32 / 2_147_483_648.0).collect(),
        (3, 32) => data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        (3, 64) => data.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap()) as f32).collect(),
        _ => return Err(bad("unsupported WAV encoding (use PCM or float)")),
    };
    Ok(Wav { sample_rate: rate, channels, samples })
}

pub fn read(path: &Path) -> io::Result<Wav> {
    parse(&std::fs::read(path)?)
}

pub fn encode_pcm16(w: &Wav) -> Vec<u8> {
    let data_len = w.samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&w.channels.to_le_bytes());
    out.extend_from_slice(&w.sample_rate.to_le_bytes());
    out.extend_from_slice(&(w.sample_rate * u32::from(w.channels) * 2).to_le_bytes());
    out.extend_from_slice(&(w.channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in &w.samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_round_trip() {
        let w = Wav { sample_rate: 44_100, channels: 2, samples: vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25] };
        let back = parse(&encode_pcm16(&w)).unwrap();
        assert_eq!(back.sample_rate, 44_100);
        assert_eq!(back.frames(), 3);
        for (a, b) in w.samples.iter().zip(&back.samples) {
            assert!((a - b).abs() < 1e-4);
        }
        assert_eq!(back.stereo_at(1), (back.samples[2], back.samples[3]));
    }

    #[test]
    fn float_and_24bit() {
        let mut bytes = encode_pcm16(&Wav { sample_rate: 8000, channels: 1, samples: vec![] });
        // rewrite as 24-bit mono with one sample of -0.5
        bytes[34] = 24;
        bytes[32] = 3;
        bytes.truncate(44);
        bytes.extend_from_slice(&[0x00, 0x00, 0xc0]);
        let n = (bytes.len() - 44) as u32;
        bytes[40..44].copy_from_slice(&n.to_le_bytes());
        let w = parse(&bytes).unwrap();
        assert!((w.samples[0] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn rejects_non_wav() {
        assert!(parse(b"RIFF\0\0\0\0AVI ").is_err());
    }
}
