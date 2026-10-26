// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal PNG writer for RGBA8 images: one IDAT holding a zlib stream of
//! uncompressed ("stored") deflate blocks. Output is larger than a compressing
//! encoder's but valid everywhere, and needs no compression library.

use crate::rgba::RgbaImage;

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

pub fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for part in parts {
        for &b in *part {
            c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xffff_ffff
}

pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// zlib stream with stored deflate blocks (max 65535 bytes each).
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65_535 * 5 + 16);
    out.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = data.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        let len = block.len() as u16;
        out.push(u8::from(last));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Reverses a PNG row filter in place.
fn unfilter_row(kind: u8, row: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), PngError> {
    if kind > 4 {
        return Err(PngError::BadFilter);
    }
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let p = match kind {
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        row[i] = row[i].wrapping_add(p);
    }
    Ok(())
}

fn filter_encode(kind: u8, row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>) {
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let p = match kind {
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        out.push(row[i].wrapping_sub(p));
    }
}

pub fn encode(img: &RgbaImage) -> Vec<u8> {
    let stride = img.width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * img.height as usize);
    let zero = vec![0u8; stride];
    let mut trial = Vec::with_capacity(stride);
    for y in 0..img.height as usize {
        let row = &img.pixels[y * stride..(y + 1) * stride];
        let prev = if y == 0 { &zero[..] } else { &img.pixels[(y - 1) * stride..y * stride] };
        // Pick the filter with the smallest sum of absolute signed residuals.
        let mut best = (u64::MAX, 0u8, Vec::new());
        for kind in 0..5u8 {
            trial.clear();
            filter_encode(kind, row, prev, 4, &mut trial);
            let score: u64 = trial.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
            if score < best.0 {
                best = (score, kind, trial.clone());
            }
        }
        raw.push(best.1);
        raw.extend_from_slice(&best.2);
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, adaptive filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &crate::deflate::zlib_compress(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngError {
    NotPng,
    BadChunk,
    Unsupported(&'static str),
    BadFilter,
    Inflate(crate::deflate::InflateError),
    TooLarge,
}

impl std::fmt::Display for PngError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPng => f.write_str("not a PNG file"),
            Self::BadChunk => f.write_str("PNG chunk is damaged"),
            Self::Unsupported(what) => write!(f, "PNG feature not supported: {what}"),
            Self::BadFilter => f.write_str("PNG row filter is invalid"),
            Self::Inflate(e) => write!(f, "PNG image data is damaged ({e})"),
            Self::TooLarge => f.write_str("PNG is larger than 16384×16384"),
        }
    }
}

impl std::error::Error for PngError {}

/// Decodes any non-interlaced PNG (gray, RGB, palette, gray+alpha, RGBA; 1–16 bit) to RGBA8.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, PngError> {
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(PngError::NotPng);
    }
    let mut pos = 8;
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat = Vec::new();
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let body = bytes.get(pos + 8..pos + 8 + len).ok_or(PngError::BadChunk)?;
        let crc = bytes.get(pos + 8 + len..pos + 12 + len).ok_or(PngError::BadChunk)?;
        if crc32(&[kind, body]) != u32::from_be_bytes(crc.try_into().unwrap()) {
            return Err(PngError::BadChunk);
        }
        match kind {
            b"IHDR" if body.len() == 13 => {
                w = u32::from_be_bytes(body[0..4].try_into().unwrap());
                h = u32::from_be_bytes(body[4..8].try_into().unwrap());
                depth = body[8];
                ctype = body[9];
                interlace = body[12];
            }
            b"PLTE" => palette = body.chunks_exact(3).map(|c| [c[0], c[1], c[2], 255]).collect(),
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if w == 0 || h == 0 {
        return Err(PngError::BadChunk);
    }
    if w > 16_384 || h > 16_384 {
        return Err(PngError::TooLarge);
    }
    if interlace != 0 {
        return Err(PngError::Unsupported("interlaced images"));
    }
    let channels = match ctype {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err(PngError::Unsupported("colour type")),
    };
    if !matches!(depth, 1 | 2 | 4 | 8 | 16) || (depth < 8 && !matches!(ctype, 0 | 3)) || (depth == 16 && ctype == 3) {
        return Err(PngError::Unsupported("bit depth"));
    }
    for (i, &a) in trns.iter().enumerate().filter(|_| ctype == 3) {
        if let Some(p) = palette.get_mut(i) {
            p[3] = a;
        }
    }
    let bits_pp = channels * depth as usize;
    let stride = (w as usize * bits_pp).div_ceil(8);
    let bpp = bits_pp.div_ceil(8).max(1);
    let raw = crate::deflate::zlib_decompress(&idat, (stride + 1) * h as usize).map_err(PngError::Inflate)?;
    if raw.len() < (stride + 1) * h as usize {
        return Err(PngError::Inflate(crate::deflate::InflateError::Truncated));
    }
    let mut img = RgbaImage::new(w, h);
    let mut prev = vec![0u8; stride];
    let mut row = vec![0u8; stride];
    let key16 = |i: usize| trns.get(i * 2..i * 2 + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    for y in 0..h as usize {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        row.copy_from_slice(&line[1..]);
        unfilter_row(line[0], &mut row, &prev, bpp)?;
        for x in 0..w as usize {
            let sample = |c: usize| -> u16 {
                match depth {
                    16 => u16::from_be_bytes([row[(x * channels + c) * 2], row[(x * channels + c) * 2 + 1]]),
                    8 => row[x * channels + c] as u16,
                    d => {
                        let bit = x * d as usize;
                        ((row[bit / 8] >> (8 - d as usize - bit % 8)) & ((1 << d) - 1)) as u16
                    }
                }
            };
            let to8 = |v: u16| -> u8 {
                match depth {
                    16 => (v >> 8) as u8,
                    8 => v as u8,
                    d => (v as u32 * 255 / ((1u32 << d) - 1)) as u8,
                }
            };
            let px = match ctype {
                0 => {
                    let v = sample(0);
                    let a = if key16(0) == Some(v) { 0 } else { 255 };
                    let g = to8(v);
                    [g, g, g, a]
                }
                2 => {
                    let (r, g, b) = (sample(0), sample(1), sample(2));
                    let a = if trns.len() >= 6 && key16(0) == Some(r) && key16(1) == Some(g) && key16(2) == Some(b) { 0 } else { 255 };
                    [to8(r), to8(g), to8(b), a]
                }
                3 => *palette.get(sample(0) as usize).ok_or(PngError::BadChunk)?,
                4 => {
                    let g = to8(sample(0));
                    [g, g, g, to8(sample(1))]
                }
                _ => [to8(sample(0)), to8(sample(1)), to8(sample(2)), to8(sample(3))],
            };
            let i = (y * w as usize + x) * 4;
            img.pixels[i..i + 4].copy_from_slice(&px);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    Ok(img)
}

pub fn read(path: &std::path::Path) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    decode(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_checksums() {
        assert_eq!(crc32(&[b"123456789"]), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
    }

    #[test]
    fn stored_blocks_split_large_input() {
        let data = vec![7u8; 70_000];
        let z = zlib_stored(&data);
        // header + 2 block headers + payload + adler
        assert_eq!(z.len(), 2 + 5 * 2 + 70_000 + 4);
        assert_eq!(z[2], 0); // first block not final
        assert_eq!(z[2 + 5 + 65_535], 1); // second block final
    }

    #[test]
    fn png_layout() {
        let png = encode(&RgbaImage::new(2, 2));
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn encode_decode_round_trip() {
        let mut img = RgbaImage::new(61, 37);
        for (i, px) in img.pixels.chunks_mut(4).enumerate() {
            px.copy_from_slice(&[(i * 7) as u8, (i / 61 * 9) as u8, (i % 61 * 3) as u8, (255 - i % 200) as u8]);
        }
        let png = encode(&img);
        assert_eq!(decode(&png).unwrap(), img);
        let flat = encode(&RgbaImage::new(256, 256));
        assert!(flat.len() < 4_000, "{}", flat.len());
    }

    #[test]
    fn decodes_reference_files() {
        // Written by an independent encoder: 3x2 RGB, 2x2 palette with tRNS, 4x1 gray 2-bit, 2x1 RGBA 16-bit.
        let rgb = decode(include_bytes!("../testdata/rgb.png")).unwrap();
        assert_eq!((rgb.width, rgb.height), (3, 2));
        assert_eq!(&rgb.pixels[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);
        let pal = decode(include_bytes!("../testdata/palette.png")).unwrap();
        assert_eq!(&pal.pixels[..8], &[10, 20, 30, 0, 200, 100, 50, 255]);
        let gray = decode(include_bytes!("../testdata/gray2.png")).unwrap();
        assert_eq!(gray.pixels.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), vec![0, 85, 170, 255]);
        let deep = decode(include_bytes!("../testdata/rgba16.png")).unwrap();
        assert_eq!(&deep.pixels, &[0x12, 0x56, 0x9a, 0xde, 255, 0, 0, 128]);
    }

    #[test]
    fn rejects_damage() {
        let mut png = encode(&RgbaImage::new(4, 4));
        png[20] ^= 0xff;
        assert_eq!(decode(&png).unwrap_err(), PngError::BadChunk);
        assert_eq!(decode(b"GIF89a").unwrap_err(), PngError::NotPng);
    }
}
