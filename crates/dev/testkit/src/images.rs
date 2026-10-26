// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fake still images with known pixel values.
//!
//! [`pixel`] is the reference picture: a deterministic function of position
//! and size, so a test can compute what any pixel must decode to. The PNG
//! writer here supports every colour type and bit depth of the standard,
//! with or without Adam7 interlacing, so decoders can be checked on all of
//! them; the JPEG path uses the `content` crate's encoder and can add an
//! EXIF orientation. [`truncated`] cuts any file short.

use std::io;
use std::path::{Path, PathBuf};

use studio_media::RgbaImage;
use studio_media::deflate::zlib_compress;
use studio_media::png::crc32;

use crate::fixtures;

/// PNG colour types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourType {
    Gray,
    Rgb,
    Palette,
    GrayAlpha,
    Rgba,
}

impl ColourType {
    fn code(self) -> u8 {
        match self {
            ColourType::Gray => 0,
            ColourType::Rgb => 2,
            ColourType::Palette => 3,
            ColourType::GrayAlpha => 4,
            ColourType::Rgba => 6,
        }
    }
    fn channels(self) -> usize {
        match self {
            ColourType::Gray | ColourType::Palette => 1,
            ColourType::GrayAlpha => 2,
            ColourType::Rgb => 3,
            ColourType::Rgba => 4,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ColourType::Gray => "gray",
            ColourType::Rgb => "rgb",
            ColourType::Palette => "palette",
            ColourType::GrayAlpha => "gray_alpha",
            ColourType::Rgba => "rgba",
        }
    }
}

/// The reference picture: a diagonal gradient with a hard-edged square
/// and (for alpha images) a transparent corner, as straight-alpha RGBA8.
pub fn pixel(x: u32, y: u32, width: u32, height: u32) -> [u8; 4] {
    let fx = (x as f32 + 0.5) / width.max(1) as f32;
    let fy = (y as f32 + 0.5) / height.max(1) as f32;
    let r = (fx * 255.0) as u8;
    let g = (fy * 255.0) as u8;
    let b = (((fx + fy) * 0.5) * 255.0) as u8;
    let in_square = (0.3..0.6).contains(&fx) && (0.3..0.6).contains(&fy);
    let a = if fx > 0.75 && fy > 0.75 { 0 } else if fx > 0.75 { 128 } else { 255 };
    if in_square { [255, 255, 255, a] } else { [r, g, b, a] }
}

/// The reference picture as an image.
pub fn reference(width: u32, height: u32) -> RgbaImage {
    let mut img = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            img.pixels[i..i + 4].copy_from_slice(&pixel(x, y, width, height));
        }
    }
    img
}

/// A 16-colour palette used for palette PNGs.
pub fn palette() -> Vec<[u8; 4]> {
    (0..16u32).map(|i| [(i * 17) as u8, 255 - (i * 17) as u8, ((i * 37) % 256) as u8, if i == 15 { 0 } else { 255 }]).collect()
}

/// Palette index of the reference picture at a point (for comparing a
/// decoded palette PNG).
pub fn palette_index(x: u32, y: u32, width: u32, height: u32) -> u8 {
    let p = pixel(x, y, width, height);
    ((p[0] as u32 + p[1] as u32) / 32 % 16) as u8
}

/// What a decoder must produce for a PNG written by [`png_bytes`] at
/// `(x, y)`: the reference colour quantised to the written depth and
/// colour type, expanded back to RGBA8 the way PNG defines it.
pub fn expected_png_pixel(x: u32, y: u32, width: u32, height: u32, ty: ColourType, depth: u8) -> [u8; 4] {
    let p = pixel(x, y, width, height);
    let q = |v: u8| -> u8 {
        // Quantise to `depth` bits and scale back as decoders do (replicate bits).
        match depth {
            16 => v,
            8 => v,
            d => {
                let max = (1u32 << d) - 1;
                let s = ((v as u32 * max + 127) / 255) as u32;
                ((s * 255 + max / 2) / max) as u8
            }
        }
    };
    let luma = ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8;
    match ty {
        ColourType::Gray => {
            let l = q(luma);
            [l, l, l, 255]
        }
        ColourType::GrayAlpha => {
            let l = q(luma);
            [l, l, l, q(p[3])]
        }
        ColourType::Rgb => [q(p[0]), q(p[1]), q(p[2]), 255],
        ColourType::Rgba => [q(p[0]), q(p[1]), q(p[2]), q(p[3])],
        ColourType::Palette => {
            let pal = palette();
            let idx = palette_index(x, y, width, height) as usize % (1usize << depth.min(8));
            pal[idx]
        }
    }
}

fn sample_bits(ty: ColourType, depth: u8, x: u32, y: u32, width: u32, height: u32) -> Vec<u16> {
    let p = pixel(x, y, width, height);
    let scale = |v: u8| -> u16 {
        match depth {
            16 => (v as u16) << 8 | v as u16,
            8 => v as u16,
            d => ((v as u32 * ((1u32 << d) - 1) + 127) / 255) as u16,
        }
    };
    let luma = ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8;
    match ty {
        ColourType::Gray => vec![scale(luma)],
        ColourType::GrayAlpha => vec![scale(luma), scale(p[3])],
        ColourType::Rgb => vec![scale(p[0]), scale(p[1]), scale(p[2])],
        ColourType::Rgba => vec![scale(p[0]), scale(p[1]), scale(p[2]), scale(p[3])],
        ColourType::Palette => vec![(palette_index(x, y, width, height) as u16) % (1u16 << depth.min(8))],
    }
}

/// Packs one scanline of samples at `depth` bits per sample.
fn pack_row(samples: &[u16], depth: u8) -> Vec<u8> {
    match depth {
        16 => samples.iter().flat_map(|s| s.to_be_bytes()).collect(),
        8 => samples.iter().map(|&s| s as u8).collect(),
        d => {
            let per_byte = 8 / d as usize;
            let mut out = vec![0u8; samples.len().div_ceil(per_byte)];
            for (i, &s) in samples.iter().enumerate() {
                let shift = 8 - d as usize * (i % per_byte + 1);
                out[i / per_byte] |= (s as u8) << shift;
            }
            out
        }
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out.extend_from_slice(&crc32(&[kind, body]).to_be_bytes());
}

/// Writes the reference picture as a PNG of the given colour type, depth
/// (1, 2, 4, 8 or 16 as the type allows) and interlace mode. Rows use
/// filter 0 (none) and the deflate stream is our own.
pub fn png_bytes(width: u32, height: u32, ty: ColourType, depth: u8, interlaced: bool) -> Vec<u8> {
    let valid = match ty {
        ColourType::Gray => [1, 2, 4, 8, 16].contains(&depth),
        ColourType::Palette => [1, 2, 4, 8].contains(&depth),
        _ => [8, 16].contains(&depth),
    };
    assert!(valid, "{ty:?} does not allow bit depth {depth}");
    let channels = ty.channels();
    let row_bytes = |w: u32| -> Vec<u8> { vec![0u8; (w as usize * channels * depth as usize).div_ceil(8)] };
    let mut raw = Vec::new();
    let mut emit_pass = |x0: u32, y0: u32, dx: u32, dy: u32| {
        if x0 >= width || y0 >= height {
            return;
        }
        let pw = (width - x0).div_ceil(dx);
        let ph = (height - y0).div_ceil(dy);
        let _ = row_bytes(pw);
        for py in 0..ph {
            let y = y0 + py * dy;
            let mut samples = Vec::with_capacity(pw as usize * channels);
            for px in 0..pw {
                let x = x0 + px * dx;
                samples.extend(sample_bits(ty, depth, x, y, width, height));
            }
            raw.push(0u8); // filter: none
            raw.extend(pack_row(&samples, depth));
        }
    };
    if interlaced {
        // Adam7 passes: (x start, y start, x step, y step).
        for (x0, y0, dx, dy) in [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)] {
            emit_pass(x0, y0, dx, dy);
        }
    } else {
        emit_pass(0, 0, 1, 1);
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[depth, ty.code(), 0, 0, interlaced as u8]);
    chunk(&mut out, b"IHDR", &ihdr);
    if ty == ColourType::Palette {
        let pal = palette();
        let n = 1usize << depth.min(8);
        let plte: Vec<u8> = pal.iter().take(n).flat_map(|c| [c[0], c[1], c[2]]).collect();
        chunk(&mut out, b"PLTE", &plte);
        let trns: Vec<u8> = pal.iter().take(n).map(|c| c[3]).collect();
        chunk(&mut out, b"tRNS", &trns);
    }
    chunk(&mut out, b"IDAT", &zlib_compress(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The reference picture as an 8-bit RGBA PNG via the `media` encoder.
pub fn png_rgba8(width: u32, height: u32) -> Vec<u8> {
    studio_media::png::encode(&reference(width, height))
}

/// The reference picture as a baseline JPEG (opaque; alpha is dropped) at
/// `quality` 1..=100 via the `content` crate's encoder.
pub fn jpeg_bytes(width: u32, height: u32, quality: u8) -> Vec<u8> {
    let mut img = reference(width, height);
    for p in img.pixels.chunks_exact_mut(4) {
        p[3] = 255;
    }
    studio_content::jpeg::encode(&img, quality)
}

/// Inserts an EXIF APP1 segment with the given Orientation tag (1..=8)
/// right after SOI. Decoders that honour EXIF rotate the picture.
pub fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    assert!(jpeg.len() > 2 && jpeg[0] == 0xFF && jpeg[1] == 0xD8, "not a JPEG");
    // TIFF header (big endian) + IFD0 with one entry.
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\x00\x2A");
    tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD offset
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
    tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes()); // count
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]); // padding of the value field
    tiff.extend_from_slice(&0u32.to_be_bytes()); // next IFD
    let mut app1 = Vec::new();
    app1.extend_from_slice(b"Exif\x00\x00");
    app1.extend_from_slice(&tiff);
    let mut out = Vec::with_capacity(jpeg.len() + app1.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Width and height after applying an EXIF orientation to a `w`×`h` picture.
pub fn oriented_size(w: u32, h: u32, orientation: u16) -> (u32, u32) {
    if (5..=8).contains(&orientation) { (h, w) } else { (w, h) }
}

/// The first `fraction` of a file (`0.0..1.0`).
pub fn truncated(bytes: &[u8], fraction: f32) -> Vec<u8> {
    let n = ((bytes.len() as f32) * fraction.clamp(0.0, 1.0)) as usize;
    bytes[..n].to_vec()
}

/// Compares two RGBA8 buffers with a per-channel tolerance; returns the
/// first mismatch as `(x, y, channel, got, want)`.
pub fn first_mismatch(got: &[u8], want: &[u8], width: u32, tol: u8) -> Option<(u32, u32, usize, u8, u8)> {
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        if (*g as i32 - *w as i32).abs() > tol as i32 {
            let px = (i / 4) as u32;
            return Some((px % width, px / width, i % 4, *g, *w));
        }
    }
    None
}

/// Fixture files for the standard image cases, generated on first use.
pub fn png_fixture(width: u32, height: u32, ty: ColourType, depth: u8, interlaced: bool) -> PathBuf {
    let name = format!("image_{width}x{height}_{}_{depth}bit{}.png", ty.name(), if interlaced { "_adam7" } else { "" });
    fixtures::fixture(&name, |p| std::fs::write(p, png_bytes(width, height, ty, depth, interlaced)))
}

/// A JPEG fixture, optionally with an EXIF orientation.
pub fn jpeg_fixture(width: u32, height: u32, quality: u8, orientation: Option<u16>) -> PathBuf {
    let name = match orientation {
        Some(o) => format!("image_{width}x{height}_q{quality}_exif{o}.jpg"),
        None => format!("image_{width}x{height}_q{quality}.jpg"),
    };
    fixtures::fixture(&name, |p| {
        let mut j = jpeg_bytes(width, height, quality);
        if let Some(o) = orientation {
            j = with_exif_orientation(&j, o);
        }
        std::fs::write(p, j)
    })
}

/// Writes `bytes` to a file inside the scratch folder `dir`.
pub fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let p = dir.join(name);
    std::fs::write(&p, bytes)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba8_png_decodes_to_the_reference() {
        let bytes = png_bytes(37, 23, ColourType::Rgba, 8, false);
        let img = studio_media::png::decode(&bytes).unwrap();
        assert_eq!((img.width, img.height), (37, 23));
        assert_eq!(img.pixels, reference(37, 23).pixels);
    }

    #[test]
    fn packing_sub_byte_rows() {
        assert_eq!(pack_row(&[1, 0, 1, 1, 0, 0, 0, 1, 1], 1), vec![0b1011_0001, 0b1000_0000]);
        assert_eq!(pack_row(&[3, 0, 2, 1], 2), vec![0b1100_1001]);
        assert_eq!(pack_row(&[0xA, 0x5, 0xF], 4), vec![0xA5, 0xF0]);
    }

    #[test]
    fn exif_segment_is_well_formed() {
        let j = jpeg_bytes(16, 16, 80);
        let e = with_exif_orientation(&j, 6);
        assert_eq!(&e[..4], &[0xFF, 0xD8, 0xFF, 0xE1]);
        let len = u16::from_be_bytes([e[4], e[5]]) as usize;
        assert_eq!(&e[6..12], b"Exif\0\0");
        assert_eq!(&e[4 + len..4 + len + 2], &j[2..4]);
    }
}
