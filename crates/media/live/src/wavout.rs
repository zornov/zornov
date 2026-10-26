// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A WAV file written as sound arrives (24-bit PCM, mono), header fixed
//! up at the end, so a recording can be as long as the disk allows.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct WavWriter {
    path: PathBuf,
    out: BufWriter<File>,
    rate: u32,
    frames: u64,
}

impl WavWriter {
    pub fn create(path: &Path, rate: u32) -> io::Result<WavWriter> {
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&header(rate, 0))?;
        Ok(WavWriter { path: path.to_owned(), out, rate, frames: 0 })
    }

    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(samples.len() * 3);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            buf.extend_from_slice(&v.to_le_bytes()[..3]);
        }
        self.frames += samples.len() as u64;
        self.out.write_all(&buf)
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.out.flush()?;
        let mut f = self.out.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(self.rate, self.frames))?;
        f.sync_all()?;
        Ok(self.path)
    }
}

fn header(rate: u32, frames: u64) -> [u8; 44] {
    let data = (frames * 3).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * 3).to_le_bytes());
    h[32..34].copy_from_slice(&3u16.to_le_bytes());
    h[34..36].copy_from_slice(&24u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

/// Reads a mono-or-stereo 16/24/32-bit PCM or float WAV back to mono
/// floats (for tests and for sync): `(rate, samples)`.
pub fn read_mono(path: &Path) -> Result<(u32, Vec<f32>), String> {
    let b = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{} is not a WAV file", path.display()));
    }
    let (mut pos, mut fmt, mut data) = (12, None, None);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            fmt = Some((u16::from_le_bytes([body[0], body[1]]), u16::from_le_bytes([body[2], body[3]]) as usize, u32::from_le_bytes([body[4], body[5], body[6], body[7]]), u16::from_le_bytes([body[14], body[15]]) as usize));
        } else if id == b"data" {
            data = Some(body);
        }
        pos += 8 + len + (len & 1);
    }
    let ((tag, ch, rate, bits), data) = fmt.zip(data).ok_or("WAV without fmt or data")?;
    let bytes = bits / 8;
    let step = bytes * ch;
    let mut out = Vec::with_capacity(data.len() / step.max(1));
    for fr in data.chunks_exact(step.max(1)) {
        let mut sum = 0f32;
        for c in 0..ch {
            let s = &fr[c * bytes..(c + 1) * bytes];
            sum += match (tag, bits) {
                (1, 16) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                (1, 32) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                _ => return Err(format!("WAV format {tag}/{bits} bit is not supported")),
            };
        }
        out.push(sum / ch as f32);
    }
    Ok((rate, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back_24_bit() {
        let p = std::env::temp_dir().join(format!("studio-live-wav-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&p, 48000).unwrap();
        let s: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.7).collect();
        w.write(&s[..1000]).unwrap();
        w.write(&s[1000..]).unwrap();
        w.finish().unwrap();
        let (rate, back) = read_mono(&p).unwrap();
        assert_eq!((rate, back.len()), (48000, 4800));
        for (a, b) in s.iter().zip(&back) {
            assert!((a - b).abs() < 2e-7);
        }
        let _ = std::fs::remove_file(p);
    }
}
