// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CLV, Studio's movie container: CLC video frames plus 16-bit PCM audio.
//!
//! ```text
//! 0   "CLV1"
//! 4   version u16, flags u16
//! 8   width u32, height u32
//! 16  fps_num u32, fps_den u32
//! 24  frame_count u32, keyframe_interval u32
//! 32  audio_rate u32, audio_channels u16, reserved u16
//! 40  audio_frames u64          (samples per channel)
//! 48  audio_offset u64, index_offset u64
//! 64  frame payloads ... | audio s16le interleaved | index: (offset u64, size u32, key u8, pad 3) × frames
//! ```

use crate::codec::{is_key, Decoder, Encoder};
use crate::rgba::RgbaImage;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HEADER: usize = 64;
const MAGIC: &[u8; 4] = b"CLV1";

#[derive(Clone, Debug, PartialEq)]
pub struct ClvInfo {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame_count: u32,
    pub keyframe_interval: u32,
    pub audio_rate: u32,
    pub audio_channels: u16,
    pub audio_frames: u64,
}

impl ClvInfo {
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den.max(1) as f64
    }
    /// Duration in microseconds, the longer of video and audio.
    pub fn duration(&self) -> i64 {
        let v = if self.fps_num > 0 { self.frame_count as f64 * 1e6 / self.fps() } else { 0.0 };
        let a = if self.audio_rate > 0 { self.audio_frames as f64 * 1e6 / self.audio_rate as f64 } else { 0.0 };
        v.max(a).round() as i64
    }
}

#[derive(Clone, Copy, Debug)]
struct IndexEntry {
    offset: u64,
    size: u32,
    key: bool,
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

// --- writer ----------------------------------------------------------------

pub struct ClvWriter {
    file: BufWriter<File>,
    path: PathBuf,
    audio_path: PathBuf,
    audio: BufWriter<File>,
    info: ClvInfo,
    index: Vec<IndexEntry>,
    offset: u64,
    encoder: Encoder,
}

impl ClvWriter {
    pub fn create(path: &Path, width: u32, height: u32, fps_num: u32, fps_den: u32, audio_rate: u32, quality: u8) -> io::Result<Self> {
        if width == 0 || height == 0 || width > 16_000 || height > 16_000 {
            return Err(bad("frame size out of range"));
        }
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(&[0u8; HEADER])?;
        let audio_path = path.with_extension("clv-audio.part");
        let audio = BufWriter::new(File::create(&audio_path)?);
        Ok(Self {
            file,
            path: path.to_path_buf(),
            audio_path,
            audio,
            info: ClvInfo {
                width,
                height,
                fps_num,
                fps_den: fps_den.max(1),
                frame_count: 0,
                keyframe_interval: 15,
                audio_rate,
                audio_channels: 2,
                audio_frames: 0,
            },
            index: Vec::new(),
            offset: HEADER as u64,
            encoder: Encoder::new(quality),
        })
    }

    pub fn info(&self) -> &ClvInfo {
        &self.info
    }

    /// Bytes of encoded video written so far (rate control reads this).
    pub fn video_bytes(&self) -> u64 {
        self.offset - HEADER as u64
    }

    /// Quality (1..=100) used for the frames pushed from now on. Each frame
    /// records its own quality, so changing it mid-stream is safe.
    pub fn set_quality(&mut self, quality: u8) {
        self.encoder.quality = quality.clamp(1, 100);
    }

    /// Encodes and appends one frame; it must match the declared size.
    pub fn push_frame(&mut self, img: &RgbaImage) -> io::Result<()> {
        if img.width != self.info.width || img.height != self.info.height {
            return Err(bad("frame size differs from the movie size"));
        }
        let force_key = self.index.len() as u32 % self.info.keyframe_interval == 0;
        let f = self.encoder.encode(img, force_key);
        self.file.write_all(&f.data)?;
        self.index.push(IndexEntry { offset: self.offset, size: f.data.len() as u32, key: f.key });
        self.offset += f.data.len() as u64;
        self.info.frame_count += 1;
        Ok(())
    }

    /// Appends interleaved stereo samples in [-1, 1].
    pub fn push_audio(&mut self, stereo: &[f32]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(stereo.len() * 2);
        for &s in stereo {
            buf.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
        self.audio.write_all(&buf)?;
        self.info.audio_frames += (stereo.len() / 2) as u64;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<ClvInfo> {
        self.audio.flush()?;
        drop(self.audio);
        let audio_offset = self.offset;
        let mut audio = File::open(&self.audio_path)?;
        let copied = io::copy(&mut audio, &mut self.file)?;
        let _ = std::fs::remove_file(&self.audio_path);
        let index_offset = audio_offset + copied;
        for e in &self.index {
            self.file.write_all(&e.offset.to_le_bytes())?;
            self.file.write_all(&e.size.to_le_bytes())?;
            self.file.write_all(&[u8::from(e.key), 0, 0, 0])?;
        }
        let i = &self.info;
        let mut h = Vec::with_capacity(HEADER);
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        for v in [i.width, i.height, i.fps_num, i.fps_den, i.frame_count, i.keyframe_interval, i.audio_rate] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&i.audio_channels.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&i.audio_frames.to_le_bytes());
        h.extend_from_slice(&audio_offset.to_le_bytes());
        h.extend_from_slice(&index_offset.to_le_bytes());
        h.resize(HEADER, 0);
        self.file.flush()?;
        let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&h)?;
        f.sync_all()?;
        let _ = &self.path;
        Ok(self.info)
    }

    /// Removes the partial output after a cancelled export.
    pub fn abandon(self) {
        let path = self.path.clone();
        let audio = self.audio_path.clone();
        drop(self);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(audio);
    }
}

// --- reader ----------------------------------------------------------------

pub struct ClvReader {
    file: File,
    pub info: ClvInfo,
    index: Vec<IndexEntry>,
    audio_offset: u64,
    decoder: Decoder,
    last: Option<(u32, RgbaImage)>,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl ClvReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let mut h = [0u8; HEADER];
        file.read_exact(&mut h).map_err(|_| bad("file is too short to be a CLV movie"))?;
        if &h[..4] != MAGIC {
            return Err(bad("not a CLV movie"));
        }
        let info = ClvInfo {
            width: u32_at(&h, 8),
            height: u32_at(&h, 12),
            fps_num: u32_at(&h, 16),
            fps_den: u32_at(&h, 20).max(1),
            frame_count: u32_at(&h, 24),
            keyframe_interval: u32_at(&h, 28),
            audio_rate: u32_at(&h, 32),
            audio_channels: u16_at(&h, 36),
            audio_frames: u64_at(&h, 40),
        };
        let audio_offset = u64_at(&h, 48);
        let index_offset = u64_at(&h, 56);
        let index_len = u64::from(info.frame_count) * 16;
        if index_offset.checked_add(index_len).is_none_or(|end| end > len) {
            return Err(bad("CLV index is missing; the file may be incomplete"));
        }
        file.seek(SeekFrom::Start(index_offset))?;
        let mut raw = vec![0u8; index_len as usize];
        file.read_exact(&mut raw)?;
        let index: Vec<IndexEntry> = raw
            .chunks_exact(16)
            .map(|c| IndexEntry { offset: u64_at(c, 0), size: u32_at(c, 8), key: c[12] == 1 })
            .collect();
        if index.iter().any(|e| e.offset + u64::from(e.size) > len) {
            return Err(bad("CLV frame table points past the end of the file"));
        }
        Ok(Self { file, info, index, audio_offset, decoder: Decoder::new(), last: None })
    }

    fn read_frame(&mut self, i: u32) -> io::Result<Vec<u8>> {
        let e = self.index[i as usize];
        self.file.seek(SeekFrom::Start(e.offset))?;
        let mut buf = vec![0u8; e.size as usize];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Frame shown at source time `t` (microseconds), clamped to the last frame.
    pub fn frame_index_at(&self, t: i64) -> Option<u32> {
        if self.info.frame_count == 0 {
            return None;
        }
        let i = (t.max(0) as f64 * self.info.fps() / 1e6).floor() as u64;
        Some(i.min(u64::from(self.info.frame_count) - 1) as u32)
    }

    /// Decodes frame `i`, walking forward from the nearest key frame when needed.
    pub fn frame(&mut self, i: u32) -> io::Result<RgbaImage> {
        if i >= self.info.frame_count {
            return Err(bad("frame index out of range"));
        }
        if let Some((last, img)) = &self.last {
            if *last == i {
                return Ok(img.clone());
            }
        }
        let start = match &self.last {
            Some((last, _)) if *last < i && !self.index[*last as usize + 1..=i as usize].iter().any(|e| e.key) => *last + 1,
            _ => (0..=i).rev().find(|&k| self.index[k as usize].key).ok_or_else(|| bad("no key frame before this frame"))?,
        };
        if self.index[start as usize].key {
            self.decoder.reset();
        }
        let mut img = None;
        for k in start..=i {
            let data = self.read_frame(k)?;
            if k == i {
                img = Some(self.decoder.decode(&data).map_err(|e| bad(&e.to_string()))?);
            } else {
                self.decoder.decode_planes(&data).map_err(|e| bad(&e.to_string()))?;
            }
        }
        let img = img.unwrap();
        self.last = Some((i, img.clone()));
        Ok(img)
    }

    pub fn frame_at(&mut self, t: i64) -> io::Result<Option<RgbaImage>> {
        match self.frame_index_at(t) {
            Some(i) => self.frame(i).map(Some),
            None => Ok(None),
        }
    }

    /// Reads `frames` stereo frames from sample position `start` as f32.
    pub fn audio(&mut self, start: u64, frames: usize) -> io::Result<Vec<f32>> {
        let ch = u64::from(self.info.audio_channels.max(1));
        let avail = self.info.audio_frames.saturating_sub(start).min(frames as u64);
        let mut out = vec![0f32; frames * 2];
        if avail == 0 {
            return Ok(out);
        }
        self.file.seek(SeekFrom::Start(self.audio_offset + start * ch * 2))?;
        let mut raw = vec![0u8; (avail * ch * 2) as usize];
        self.file.read_exact(&mut raw)?;
        for (f, chunk) in raw.chunks_exact((ch * 2) as usize).enumerate() {
            let l = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0;
            let r = if ch > 1 { i16::from_le_bytes([chunk[2], chunk[3]]) as f32 / 32768.0 } else { l };
            out[f * 2] = l;
            out[f * 2 + 1] = r;
        }
        Ok(out)
    }

    pub fn is_key(&mut self, i: u32) -> io::Result<bool> {
        Ok(is_key(&self.read_frame(i)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: u32) -> RgbaImage {
        let mut img = RgbaImage::new(48, 32);
        for (i, px) in img.pixels.chunks_mut(4).enumerate() {
            let x = i as u32 % 48;
            px.copy_from_slice(&[((x + n * 4) * 5) as u8, 100, (n * 20) as u8, 255]);
        }
        img
    }

    #[test]
    fn write_read_seek() {
        let path = std::env::temp_dir().join(format!("studio-{}.clv", format!("{}-{:?}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        let mut w = ClvWriter::create(&path, 48, 32, 30, 1, 48_000, 90).unwrap();
        for n in 0..40 {
            w.push_frame(&frame(n)).unwrap();
        }
        let tone: Vec<f32> = (0..48_000).flat_map(|i| {
            let s = (i as f32 * 0.05).sin() * 0.5;
            [s, -s]
        }).collect();
        w.push_audio(&tone).unwrap();
        let info = w.finish().unwrap();
        assert_eq!(info.frame_count, 40);

        let mut r = ClvReader::open(&path).unwrap();
        assert_eq!(r.info, info);
        assert!((r.info.duration() - 1_333_333).abs() < 2);
        assert!(r.is_key(0).unwrap() && r.is_key(15).unwrap() && !r.is_key(16).unwrap());
        // Random access then backwards: both must equal sequential decoding.
        let a = r.frame(37).unwrap();
        let b = r.frame(3).unwrap();
        let mut seq = ClvReader::open(&path).unwrap();
        for i in 0..=37 {
            let f = seq.frame(i).unwrap();
            if i == 3 {
                assert_eq!(f, b);
            }
            if i == 37 {
                assert_eq!(f, a);
            }
        }
        let audio = r.audio(100, 4).unwrap();
        assert!((audio[0] - tone[200]).abs() < 1e-3 && (audio[1] - tone[201]).abs() < 1e-3);
        assert_eq!(r.audio(47_999, 4).unwrap()[2..], [0.0; 6]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_garbage() {
        let path = std::env::temp_dir().join(format!("studio-bad-{}.clv", format!("{}-{:?}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::write(&path, b"CLV1 but truncated").unwrap();
        assert!(ClvReader::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
