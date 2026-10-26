// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plain RGBA8 image buffers and a tiny cache file format
//! (`CLRG`, width u32 LE, height u32 LE, then pixels).

use std::fs;
use std::io;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

const MAGIC: &[u8; 4] = b"CLRG";

impl RgbaImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, pixels: vec![0; width as usize * height as usize * 4] }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut out = Vec::with_capacity(12 + self.pixels.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.pixels);
        let tmp = path.with_extension("part");
        fs::write(&tmp, out)?;
        fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a Zornov RGBA file");
        if bytes.len() < 12 || &bytes[..4] != MAGIC {
            return Err(bad());
        }
        let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if bytes.len() != 12 + width as usize * height as usize * 4 {
            return Err(bad());
        }
        Ok(Self { width, height, pixels: bytes[12..].to_vec() })
    }

    /// Source-over blend of a premultiplied-free RGBA pixel.
    pub fn blend(&mut self, x: i64, y: i64, rgb: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 || alpha <= 0.0 {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let a = alpha.min(1.0);
        let dst_a = self.pixels[i + 3] as f32 / 255.0;
        let out_a = a + dst_a * (1.0 - a);
        for c in 0..3 {
            let src = rgb[c] as f32;
            let dst = self.pixels[i + c] as f32;
            let v = if out_a > 0.0 { (src * a + dst * dst_a * (1.0 - a)) / out_a } else { 0.0 };
            self.pixels[i + c] = v.round().clamp(0.0, 255.0) as u8;
        }
        self.pixels[i + 3] = (out_a * 255.0).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip() {
        let mut img = RgbaImage::new(3, 2);
        img.blend(1, 1, [255, 0, 0], 1.0);
        let p = std::env::temp_dir().join(format!("studio-rgba-{}.rgba", std::process::id()));
        img.save(&p).unwrap();
        assert_eq!(RgbaImage::load(&p).unwrap(), img);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn blend_over_transparent_keeps_colour() {
        let mut img = RgbaImage::new(1, 1);
        img.blend(0, 0, [10, 20, 30], 0.5);
        assert_eq!(&img.pixels, &[10, 20, 30, 128]);
    }
}
