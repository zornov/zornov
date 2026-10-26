// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Matte sequences on disk: one file holds a clip's per-frame alpha mattes
//! (8 bits a pixel, each frame zlib-compressed after a left-neighbour
//! delta), indexed by source frame so any frame is read on its own.
//!
//! Layout: the frames one after another, then a trailer: the byte offset
//! of each frame (u64, plus the end), the first frame index (u64), the
//! frame count, width and height (u32 each) and the magic `SMATTE01`.
//! Everything little-endian.

use crate::error::{Error, Result};
use crate::frame::Mask;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SMATTE01";
/// Largest frame accepted (pixels), so a corrupt trailer cannot ask for
/// gigabytes.
const MAX_PIXELS: usize = 8192 * 8192;

fn io(e: impl std::fmt::Display) -> Error {
    Error::Io(e.to_string())
}

/// Writes mattes in frame order.
pub struct Writer {
    file: std::io::BufWriter<std::fs::File>,
    width: u32,
    height: u32,
    first: u64,
    offsets: Vec<u64>,
    at: u64,
}

impl Writer {
    /// A new file for `width`×`height` mattes starting at source frame
    /// `first`.
    pub fn create(path: &Path, width: u32, height: u32, first: u64) -> Result<Writer> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
        let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(io)?);
        Ok(Writer { file, width, height, first, offsets: Vec::new(), at: 0 })
    }

    /// Appends the next frame's matte (resized to the file's size).
    pub fn push(&mut self, m: &Mask) -> Result<()> {
        let m = m.resized(self.width as usize, self.height as usize);
        let w = self.width as usize;
        let mut plane = Vec::with_capacity(m.data.len());
        for row in m.data.chunks(w) {
            let mut prev = 0u8;
            for &v in row {
                let b = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                plane.push(b.wrapping_sub(prev));
                prev = b;
            }
        }
        let z = studio_media::deflate::zlib_compress(&plane);
        self.offsets.push(self.at);
        self.file.write_all(&z).map_err(io)?;
        self.at += z.len() as u64;
        Ok(())
    }

    pub fn frames(&self) -> usize {
        self.offsets.len()
    }

    /// Writes the trailer and closes the file.
    pub fn finish(mut self) -> Result<()> {
        let mut t = Vec::new();
        for o in &self.offsets {
            t.extend_from_slice(&o.to_le_bytes());
        }
        t.extend_from_slice(&self.at.to_le_bytes());
        t.extend_from_slice(&self.first.to_le_bytes());
        t.extend_from_slice(&(self.offsets.len() as u32).to_le_bytes());
        t.extend_from_slice(&self.width.to_le_bytes());
        t.extend_from_slice(&self.height.to_le_bytes());
        t.extend_from_slice(MAGIC);
        self.file.write_all(&t).map_err(io)?;
        self.file.flush().map_err(io)
    }
}

/// Reads frames of a matte file.
pub struct Reader {
    file: std::fs::File,
    pub width: u32,
    pub height: u32,
    /// Source frame of the first matte.
    pub first: u64,
    offsets: Vec<u64>,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader> {
        let mut file = std::fs::File::open(path).map_err(io)?;
        let len = file.seek(SeekFrom::End(0)).map_err(io)?;
        if len < 36 {
            return Err(Error::Io(format!("{}: not a matte file", path.display())));
        }
        let mut tail = [0u8; 28];
        file.seek(SeekFrom::End(-28)).map_err(io)?;
        file.read_exact(&mut tail).map_err(io)?;
        if &tail[20..28] != MAGIC {
            return Err(Error::Io(format!("{}: not a matte file", path.display())));
        }
        let first = u64::from_le_bytes(tail[0..8].try_into().expect("8"));
        let count = u32::from_le_bytes(tail[8..12].try_into().expect("4")) as u64;
        let width = u32::from_le_bytes(tail[12..16].try_into().expect("4"));
        let height = u32::from_le_bytes(tail[16..20].try_into().expect("4"));
        let table = (count + 1) * 8;
        if width == 0 || height == 0 || (width as usize) * (height as usize) > MAX_PIXELS || table + 28 > len {
            return Err(Error::Io(format!("{}: damaged matte file", path.display())));
        }
        let mut raw = vec![0u8; table as usize];
        file.seek(SeekFrom::End(-(28 + table as i64))).map_err(io)?;
        file.read_exact(&mut raw).map_err(io)?;
        let offsets: Vec<u64> = raw.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().expect("8"))).collect();
        if offsets.windows(2).any(|w| w[1] < w[0]) || offsets.last().is_some_and(|&e| e > len) {
            return Err(Error::Io(format!("{}: damaged matte file", path.display())));
        }
        Ok(Reader { file, width, height, first, offsets })
    }

    pub fn frames(&self) -> u64 {
        self.offsets.len().saturating_sub(1) as u64
    }

    /// The 8-bit matte of source frame `index` (the nearest stored frame
    /// outside the stored range).
    pub fn plane(&mut self, index: u64) -> Result<Vec<u8>> {
        let n = self.frames();
        if n == 0 {
            return Err(Error::Io("the matte file is empty".into()));
        }
        let i = index.saturating_sub(self.first).min(n - 1) as usize;
        let (a, b) = (self.offsets[i], self.offsets[i + 1]);
        let mut z = vec![0u8; (b - a) as usize];
        self.file.seek(SeekFrom::Start(a)).map_err(io)?;
        self.file.read_exact(&mut z).map_err(io)?;
        let (w, h) = (self.width as usize, self.height as usize);
        let mut plane = studio_media::deflate::zlib_decompress(&z, w * h).map_err(|e| Error::Io(format!("matte frame {index}: {e:?}")))?;
        if plane.len() != w * h {
            return Err(Error::Io(format!("matte frame {index} has the wrong size")));
        }
        for row in plane.chunks_mut(w) {
            let mut prev = 0u8;
            for v in row.iter_mut() {
                *v = v.wrapping_add(prev);
                prev = *v;
            }
        }
        Ok(plane)
    }

    /// The matte of `index` as a 0..1 mask.
    pub fn mask(&mut self, index: u64) -> Result<Mask> {
        let p = self.plane(index)?;
        Ok(Mask::new(self.width as usize, self.height as usize, p.iter().map(|&b| b as f32 / 255.0).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back_any_frame() {
        let dir = std::env::temp_dir().join(format!("matte-file-{}", std::process::id()));
        let path = dir.join("a.smatte");
        let mut w = Writer::create(&path, 64, 32, 10).unwrap();
        for k in 0..5 {
            let data = (0..64 * 32).map(|i| if (i % 64) < 10 + k * 8 { 1.0 } else { (i % 7) as f32 / 7.0 }).collect();
            w.push(&Mask::new(64, 32, data)).unwrap();
        }
        assert_eq!(w.frames(), 5);
        w.finish().unwrap();
        let mut r = Reader::open(&path).unwrap();
        assert_eq!((r.width, r.height, r.first, r.frames()), (64, 32, 10, 5));
        // Out of order, and outside the range (nearest end).
        for (ask, k) in [(13u64, 3usize), (10, 0), (99, 4), (0, 0)] {
            let m = r.mask(ask).unwrap();
            for x in 0..64 {
                let want = if x < 10 + k * 8 { 1.0 } else { (x % 7) as f32 / 7.0 };
                assert!((m.data[x] - want).abs() < 0.003, "frame {ask} x {x}");
            }
        }
        std::fs::write(dir.join("bad.smatte"), b"not a matte file at all, really not").unwrap();
        assert!(Reader::open(&dir.join("bad.smatte")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
