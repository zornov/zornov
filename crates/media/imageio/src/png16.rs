// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PNG at full precision: 8 or 16 bits per sample, grey, grey + alpha,
//! RGB, RGBA (palette images go through `studio_media::png`). Interlaced
//! files are not supported. Writes 16-bit RGB or RGBA.

use crate::{Encoding, Image};

/// Decodes a PNG keeping 16-bit precision.
pub fn decode(b: &[u8]) -> Result<Image, String> {
    if !b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("not a PNG file".into());
    }
    let mut pos = 8;
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0usize, 0usize, 0u8, 0u8, 0u8);
    let mut idat = Vec::new();
    while pos + 8 <= b.len() {
        let len = u32::from_be_bytes([b[pos], b[pos + 1], b[pos + 2], b[pos + 3]]) as usize;
        let kind = &b[pos + 4..pos + 8];
        let body = b.get(pos + 8..pos + 8 + len).ok_or("PNG is truncated")?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
                h = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;
                depth = body[8];
                ctype = body[9];
                interlace = body[12];
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if ctype == 3 || depth < 8 {
        // Palette and low bit depths: the 8-bit decoder handles them.
        let img = studio_media::png::decode(b).map_err(|e| format!("{e:?}"))?;
        let mut out = Image::new(img.width, img.height, Encoding::Code { bits: 8 });
        out.has_alpha = true;
        for (o, p) in out.pixels.iter_mut().zip(img.pixels.chunks_exact(4)) {
            *o = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0];
        }
        return Ok(out);
    }
    if interlace != 0 {
        return Err("interlaced PNG is not supported".into());
    }
    if w == 0 || h == 0 || w > 65536 || h > 65536 {
        return Err("bad PNG size".into());
    }
    let channels = match ctype {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        c => return Err(format!("PNG colour type {c} is not supported")),
    };
    let bpp = channels * depth as usize / 8;
    let stride = w * bpp;
    let raw = studio_media::deflate::zlib_decompress(&idat, (stride + 1) * h).map_err(|e| format!("bad PNG data: {e:?}"))?;
    if raw.len() < (stride + 1) * h {
        return Err("PNG image data is short".into());
    }
    let mut cur = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let max = if depth == 16 { 65535.0 } else { 255.0 };
    let mut img = Image::new(w as u32, h as u32, Encoding::Code { bits: depth });
    img.has_alpha = channels == 2 || channels == 4;
    for y in 0..h {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let filter = line[0];
        cur.copy_from_slice(&line[1..]);
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
            let up = prev[i] as i32;
            let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
            let add = match filter {
                0 => 0,
                1 => a,
                2 => up,
                3 => (a + up) / 2,
                4 => {
                    let p = a + up - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - up).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc { a } else if pb <= pc { up } else { c }
                }
                f => return Err(format!("bad PNG filter {f}")),
            };
            cur[i] = (cur[i] as i32 + add) as u8;
        }
        for x in 0..w {
            let s = |k: usize| -> f32 {
                if depth == 16 {
                    u16::from_be_bytes([cur[(x * channels + k) * 2], cur[(x * channels + k) * 2 + 1]]) as f32 / max
                } else {
                    cur[x * channels + k] as f32 / max
                }
            };
            img.pixels[y * w + x] = match channels {
                1 => [s(0), s(0), s(0), 1.0],
                2 => [s(0), s(0), s(0), s(1)],
                3 => [s(0), s(1), s(2), 1.0],
                _ => [s(0), s(1), s(2), s(3)],
            };
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Ok(img)
}

/// Encodes code values (0..1) as a 16-bit RGB or RGBA PNG.
pub fn encode(width: u32, height: u32, pixels: &[[f32; 4]], alpha: bool) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let ch = if alpha { 4 } else { 3 };
    let mut raw = Vec::with_capacity((w * ch * 2 + 1) * h);
    for y in 0..h {
        // Filter "up" suits smooth 16-bit images well.
        raw.push(2);
        for x in 0..w {
            for c in 0..ch {
                let v = (pixels[y * w + x][c].clamp(0.0, 1.0) * 65535.0).round() as u16;
                let p = if y > 0 { (pixels[(y - 1) * w + x][c].clamp(0.0, 1.0) * 65535.0).round() as u16 } else { 0 };
                let (vb, pb) = (v.to_be_bytes(), p.to_be_bytes());
                raw.push(vb[0].wrapping_sub(pb[0]));
                raw.push(vb[1].wrapping_sub(pb[1]));
            }
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |out: &mut Vec<u8>, kind: &[u8], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&studio_media::png::crc32(&[kind, body]).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[16, if alpha { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &studio_media::deflate::zlib_compress(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bit_round_trip() {
        let (w, h) = (17, 9);
        let px: Vec<[f32; 4]> = (0..w * h).map(|i| [i as f32 / (w * h) as f32, 0.123_456, 1.0 - i as f32 / (w * h) as f32, 0.75]).collect();
        for alpha in [false, true] {
            let img = decode(&encode(w as u32, h as u32, &px, alpha)).unwrap();
            assert_eq!(img.encoding, Encoding::Code { bits: 16 });
            for (a, b) in img.pixels.iter().zip(&px) {
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() <= 0.5 / 65535.0 + 1e-7);
                }
            }
        }
        // The 8-bit decoder reads it too.
        let eight = studio_media::png::decode(&encode(w as u32, h as u32, &px, true)).unwrap();
        assert_eq!(eight.width, w as u32);
    }
}
