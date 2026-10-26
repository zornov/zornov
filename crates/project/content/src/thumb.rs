// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Thumbnails: decode any library file to a picture, key green screens,
//! downscale with the Lanczos filter and cache the result on disk.
//!
//! Cache entries are PNGs named by a hash of the source path, its size and
//! modification time, the requested size and a format version, so edits to
//! a source file or to the pipeline invalidate them automatically.

use crate::image::resize_to_fit;
use crate::item::Kind;
use crate::{key, png, svg, video};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use studio_media::rgba::RgbaImage;

/// Bump when the pipeline changes so old cache files are ignored.
pub const CACHE_VERSION: u32 = 1;

/// FNV-1a 64-bit, for cache keys (not security).
pub fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for &b in *p {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Generates and caches thumbnails.
#[derive(Clone, Debug)]
pub struct Thumbnailer {
    pub cache_dir: PathBuf,
    pub svg_options: svg::RenderOptions,
}

impl Thumbnailer {
    pub fn new(cache_dir: impl Into<PathBuf>) -> io::Result<Thumbnailer> {
        let cache_dir = cache_dir.into();
        fs::create_dir_all(&cache_dir)?;
        Ok(Thumbnailer { cache_dir, svg_options: svg::RenderOptions::default() })
    }

    /// Cache file for `src` at `size`.
    pub fn cache_path(&self, src: &Path, kind: Kind, size: u32) -> io::Result<PathBuf> {
        let meta = fs::metadata(src)?;
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let key = fnv64(&[
            src.to_string_lossy().as_bytes(),
            &meta.len().to_le_bytes(),
            &mtime.to_le_bytes(),
            &size.to_le_bytes(),
            &CACHE_VERSION.to_le_bytes(),
            kind.folder().as_bytes(),
        ]);
        let name = format!("{key:016x}.png");
        Ok(self.cache_dir.join(&name[..2]).join(name))
    }

    /// The thumbnail of `src`, from the cache when present.
    pub fn thumbnail(&self, src: &Path, kind: Kind, size: u32) -> Result<RgbaImage, String> {
        let path = self.cache_path(src, kind, size).map_err(|e| format!("{}: {e}", src.display()))?;
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(img) = studio_media::png::decode(&bytes) {
                return Ok(img);
            }
        }
        let img = self.render(src, kind, size)?;
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = png::write(&img, &path);
        Ok(img)
    }

    /// Renders without touching the cache.
    pub fn render(&self, src: &Path, kind: Kind, size: u32) -> Result<RgbaImage, String> {
        // A kept 3D model shows the picture saved beside it.
        if kind == Kind::Model {
            let img = load_picture(&src.with_file_name("thumb.png"))?;
            return Ok(resize_to_fit(&img, size));
        }
        let ext = src.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext == "svg" {
            let text = fs::read_to_string(src).map_err(|e| format!("{}: {e}", src.display()))?;
            return svg::render_fit(&text, size, &self.svg_options);
        }
        if kind == Kind::Font || matches!(ext.as_str(), "ttf" | "otf") {
            return font_sample(src, size);
        }
        if kind.is_audio() || matches!(ext.as_str(), "ogg" | "wav" | "mp3" | "flac") {
            let (samples, channels) = load_audio(src)?;
            return Ok(waveform(&samples, channels, size, size / 2));
        }
        let mut img = load_picture(src)?;
        if kind.is_green_screen() {
            if let Some((keyed, _)) = key::auto(&img) {
                img = keyed;
            }
        }
        Ok(resize_to_fit(&img, size))
    }

    /// Removes every cached file.
    pub fn clear(&self) -> io::Result<()> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
        }
        fs::create_dir_all(&self.cache_dir)
    }
}

/// Decodes a still (PNG or JPEG) or the poster frame of an MP4.
pub fn load_picture(src: &Path) -> Result<RgbaImage, String> {
    let ext = src.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => studio_media::png::read(src),
        "jpg" | "jpeg" => studio_media::jpeg::read(src),
        "mp4" | "m4v" | "mov" => video::poster_frame(src, 0.25),
        _ => Err(format!("{}: not a picture format", src.display())),
    }
}

/// Decodes an audio file to interleaved samples in −1..1.
pub fn load_audio(src: &Path) -> Result<(Vec<f32>, u16), String> {
    let ext = src.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "wav" => studio_media::wav::read(src).map(|w| (w.samples, w.channels)).map_err(|e| e.to_string()),
        "ogg" => studio_media::ogg::read(src).map(|w| (w.samples, w.channels)),
        "mp3" => studio_media::mp3::read(src).map(|w| (w.samples, w.channels)),
        "flac" => {
            let bytes = fs::read(src).map_err(|e| e.to_string())?;
            let f = studio_media::flac::decode(&bytes)?;
            let scale = 1.0 / ((1i64 << (f.bits_per_sample.max(1) - 1)) as f32);
            Ok((f.samples.iter().map(|&s| s as f32 * scale).collect(), f.channels))
        }
        _ => Err(format!("{}: not an audio format", src.display())),
    }
}

/// A waveform picture: one bar per column with the peak of that slice,
/// drawn in a neutral tone on transparency.
pub fn waveform(samples: &[f32], channels: u16, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    let mut img = RgbaImage::new(w, h);
    let ch = channels.max(1) as usize;
    let frames = samples.len() / ch;
    if frames == 0 {
        return img;
    }
    let mid = h as f32 / 2.0;
    for x in 0..w {
        let a = (x as usize * frames) / w as usize;
        let b = (((x + 1) as usize * frames) / w as usize).max(a + 1).min(frames);
        let mut peak = 0f32;
        for i in a..b {
            for c in 0..ch {
                peak = peak.max(samples[i * ch + c].abs());
            }
        }
        let half = (peak.min(1.0) * (mid - 1.0)).max(0.5);
        let y0 = (mid - half).floor().max(0.0) as u32;
        let y1 = (mid + half).ceil().min(h as f32) as u32;
        for y in y0..y1 {
            let i = ((y * w + x) * 4) as usize;
            img.pixels[i..i + 4].copy_from_slice(&[120, 130, 150, 255]);
        }
    }
    img
}

/// "Aa" set in the font, fitted into a `size` square.
pub fn font_sample(src: &Path, size: u32) -> Result<RgbaImage, String> {
    let data = fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let font = studio_media::ttf::Font::parse(data, 0).map_err(|e| format!("{}: {}", src.display(), e.0))?;
    let px = size as f32 * 0.6;
    let mut pen = 0f32;
    let mut glyphs = Vec::new();
    for c in ['A', 'a'] {
        let g = font.glyph_index(c);
        let bm = font.rasterize(g, px);
        let adv = font.advance(g) * px / font.units_per_em;
        glyphs.push((bm, pen));
        pen += adv;
    }
    let mut img = RgbaImage::new(size, size);
    let baseline = (size as f32 * 0.72) as i32;
    let x_off = ((size as f32 - pen) / 2.0).max(0.0) as i32;
    for (bm, pen_x) in glyphs {
        for y in 0..bm.height {
            for x in 0..bm.width {
                let cov = bm.coverage[y * bm.width + x];
                if cov == 0 {
                    continue;
                }
                let dx = x_off + pen_x as i32 + bm.left + x as i32;
                let dy = baseline - bm.top + y as i32;
                if dx < 0 || dy < 0 || dx >= size as i32 || dy >= size as i32 {
                    continue;
                }
                let i = ((dy as u32 * size + dx as u32) * 4) as usize;
                img.pixels[i..i + 4].copy_from_slice(&[32, 32, 36, cov]);
            }
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-content-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn caches_and_reuses_thumbnails() {
        let dir = temp_dir("thumb");
        let src = dir.join("big.png");
        let mut img = RgbaImage::new(640, 320);
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            let x = i % 640;
            p.copy_from_slice(&[(x / 3) as u8, 90, 200, if x < 500 { 255 } else { 0 }]);
        }
        fs::write(&src, png::encode(&img)).unwrap();
        let t = Thumbnailer::new(dir.join("cache")).unwrap();
        let a = t.thumbnail(&src, Kind::Sticker, 128).unwrap();
        assert_eq!((a.width, a.height), (128, 64));
        let path = t.cache_path(&src, Kind::Sticker, 128).unwrap();
        assert!(path.exists());
        let b = t.thumbnail(&src, Kind::Sticker, 128).unwrap();
        assert_eq!(a, b);
        // A different size is a different cache entry.
        assert_ne!(path, t.cache_path(&src, Kind::Sticker, 64).unwrap());
        t.clear().unwrap();
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn svg_sources_render_directly() {
        let dir = temp_dir("svg");
        let src = dir.join("dot.svg");
        fs::write(&src, r##"<svg viewBox="0 0 10 20"><rect width="10" height="20" fill="#f00"/></svg>"##).unwrap();
        let t = Thumbnailer::new(dir.join("cache")).unwrap();
        let img = t.thumbnail(&src, Kind::Emoji, 100).unwrap();
        assert_eq!((img.width, img.height), (50, 100));
        assert_eq!(&img.pixels[..4], &[255, 0, 0, 255]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn green_screen_items_arrive_keyed() {
        let dir = temp_dir("key");
        let src = dir.join("gs.png");
        let mut img = RgbaImage::new(64, 64);
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i % 64, i / 64);
            let subject = (20..44).contains(&x) && (20..44).contains(&y);
            p.copy_from_slice(if subject { &[180, 60, 60, 255] } else { &[40, 200, 50, 255] });
        }
        fs::write(&src, png::encode(&img)).unwrap();
        let t = Thumbnailer::new(dir.join("cache")).unwrap();
        let out = t.thumbnail(&src, Kind::GreenScreen, 32).unwrap();
        assert_eq!(out.pixels[3], 0, "corner transparent");
        assert!(out.pixels[((16 * 32 + 16) * 4) + 3] > 250, "subject opaque");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn waveform_shape() {
        let samples: Vec<f32> = (0..2000).map(|i| ((i as f32) * 0.05).sin() * if i < 1000 { 1.0 } else { 0.1 }).collect();
        let img = waveform(&samples, 1, 64, 32);
        // Loud first half fills more rows than the quiet second half.
        let col = |x: u32| (0..32).filter(|&y| img.pixels[((y * 64 + x) * 4 + 3) as usize] > 0).count();
        assert!(col(10) > col(50) + 10);
        assert_eq!(waveform(&[], 2, 8, 4).pixels.iter().filter(|&&v| v > 0).count(), 0);
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(fnv64(&[b"a", b"b"]), fnv64(&[b"a", b"b"]));
        assert_ne!(fnv64(&[b"a", b"b"]), fnv64(&[b"ab"]));
    }
}
