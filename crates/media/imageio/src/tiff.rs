// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Baseline TIFF.
//!
//! Read: the first image; 8 / 16-bit unsigned or 16 / 32-bit float
//! samples; grey, grey + alpha, RGB, RGBA (chunky or planar); strips or
//! tiles; no compression, LZW, Deflate (8 and 32946) or PackBits; the
//! horizontal (2) and floating-point (3) predictors.
//!
//! Write: 16-bit RGB or RGBA, Deflate with the horizontal predictor, one
//! strip per 16 rows.

use crate::{Encoding, Image, Reader, half};

const T_WIDTH: u16 = 256;
const T_HEIGHT: u16 = 257;
const T_BITS: u16 = 258;
const T_COMPRESSION: u16 = 259;
const T_PHOTOMETRIC: u16 = 262;
const T_STRIP_OFFSETS: u16 = 273;
const T_SAMPLES: u16 = 277;
const T_ROWS_PER_STRIP: u16 = 278;
const T_STRIP_BYTES: u16 = 279;
const T_PLANAR: u16 = 284;
const T_PREDICTOR: u16 = 317;
const T_TILE_W: u16 = 322;
const T_TILE_H: u16 = 323;
const T_TILE_OFFSETS: u16 = 324;
const T_TILE_BYTES: u16 = 325;
const T_EXTRA: u16 = 338;
const T_FORMAT: u16 = 339;

struct Ifd {
    tags: Vec<(u16, Vec<u32>)>,
}

impl Ifd {
    fn get(&self, t: u16) -> Option<&[u32]> {
        self.tags.iter().find(|(k, _)| *k == t).map(|(_, v)| v.as_slice())
    }

    fn one(&self, t: u16, d: u32) -> u32 {
        self.get(t).and_then(|v| v.first().copied()).unwrap_or(d)
    }
}

fn read_ifd(b: &[u8], big: bool, at: usize) -> Result<Ifd, String> {
    let mut r = Reader::at(b, at, big);
    let n = r.u16()? as usize;
    let mut tags = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = r.u16()?;
        let ty = r.u16()?;
        let count = r.u32()? as usize;
        let size = match ty {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => 1,
        };
        let total = size * count;
        let value_pos = if total <= 4 { r.pos } else { Reader::at(b, r.pos, big).u32()? as usize };
        r.pos += 4;
        if count > 1 << 24 {
            return Err("TIFF tag too large".into());
        }
        let mut vr = Reader::at(b, value_pos, big);
        let mut vals = Vec::with_capacity(count.min(1 << 16));
        for _ in 0..count {
            vals.push(match ty {
                3 | 8 => vr.u16()? as u32,
                4 | 9 => vr.u32()?,
                5 | 10 => {
                    let v = vr.u32()?;
                    vr.u32()?;
                    v
                }
                _ => vr.u8()? as u32,
            });
        }
        tags.push((tag, vals));
    }
    Ok(Ifd { tags })
}

/// Decodes a TIFF file.
pub fn decode(b: &[u8]) -> Result<Image, String> {
    let big = match b.get(..4) {
        Some(b"II*\0") => false,
        Some(b"MM\0*") => true,
        _ => return Err("not a TIFF file".into()),
    };
    let first = Reader::at(b, 4, big).u32()? as usize;
    let ifd = read_ifd(b, big, first)?;
    let w = ifd.one(T_WIDTH, 0) as usize;
    let h = ifd.one(T_HEIGHT, 0) as usize;
    if w == 0 || h == 0 || w > 65536 || h > 65536 {
        return Err("bad TIFF size".into());
    }
    let spp = ifd.one(T_SAMPLES, 1) as usize;
    let bits = ifd.one(T_BITS, 1) as usize;
    let format = ifd.one(T_FORMAT, 1);
    let comp = ifd.one(T_COMPRESSION, 1);
    let photometric = ifd.one(T_PHOTOMETRIC, 2);
    let planar = ifd.one(T_PLANAR, 1) == 2;
    let predictor = ifd.one(T_PREDICTOR, 1);
    if !matches!(bits, 8 | 16 | 32) || (bits == 32 && format != 3) {
        return Err(format!("TIFF {bits}-bit samples (format {format}) are not supported"));
    }
    if !matches!(photometric, 0 | 1 | 2) {
        return Err(format!("TIFF photometric {photometric} is not supported (use RGB or grey)"));
    }
    let bps = bits / 8;
    let float = format == 3;
    // Blocks: strips or tiles.
    let (bw, bh, offsets, counts) = if let Some(off) = ifd.get(T_TILE_OFFSETS) {
        (ifd.one(T_TILE_W, 0) as usize, ifd.one(T_TILE_H, 0) as usize, off.to_vec(), ifd.get(T_TILE_BYTES).ok_or("TIFF tiles without byte counts")?.to_vec())
    } else {
        let rps = (ifd.one(T_ROWS_PER_STRIP, h as u32) as usize).min(h).max(1);
        (w, rps, ifd.get(T_STRIP_OFFSETS).ok_or("TIFF without image data")?.to_vec(), ifd.get(T_STRIP_BYTES).ok_or("TIFF strips without byte counts")?.to_vec())
    };
    if bw == 0 || bh == 0 {
        return Err("bad TIFF tile size".into());
    }
    let across = w.div_ceil(bw);
    let down = h.div_ceil(bh);
    let planes = if planar { spp } else { 1 };
    let per_px = if planar { 1 } else { spp };
    let mut img = Image::new(w as u32, h as u32, if float { Encoding::Linear } else { Encoding::Code { bits: bits as u8 } });
    img.has_alpha = spp == 2 || spp >= 4;
    let mut buf = vec![0f32; w * h * spp];
    for p in 0..planes {
        for ty in 0..down {
            for tx in 0..across {
                let idx = p * across * down + ty * across + tx;
                let (off, cnt) = (*offsets.get(idx).ok_or("TIFF block table is short")? as usize, *counts.get(idx).ok_or("TIFF block table is short")? as usize);
                let data = b.get(off..off + cnt).ok_or("TIFF data is truncated")?;
                let row_bytes = bw * per_px * bps;
                let want = row_bytes * bh;
                let mut raw = match comp {
                    1 => data.to_vec(),
                    5 => lzw_decode(data, want)?,
                    8 | 32946 => studio_media::deflate::zlib_decompress(data, want * 2 + 1024).map_err(|e| format!("bad TIFF Deflate data: {e:?}"))?,
                    32773 => packbits(data, want),
                    c => return Err(format!("TIFF compression {c} is not supported")),
                };
                raw.resize(want, 0);
                for row in raw.chunks_exact_mut(row_bytes) {
                    if predictor == 2 {
                        undo_horizontal(row, per_px, bps, big);
                    } else if predictor == 3 {
                        undo_float(row, per_px, bps, big);
                    }
                }
                for yy in 0..bh {
                    let y = ty * bh + yy;
                    if y >= h {
                        break;
                    }
                    let row = &raw[yy * row_bytes..(yy + 1) * row_bytes];
                    let mut rr = Reader::at(row, 0, big);
                    for xx in 0..bw {
                        let x = tx * bw + xx;
                        for s in 0..per_px {
                            let v = match (bps, float) {
                                (1, _) => rr.u8()? as f32 / 255.0,
                                (2, false) => rr.u16()? as f32 / 65535.0,
                                (2, true) => half::to_f32(rr.u16()?),
                                _ => rr.f32()?,
                            };
                            if x < w {
                                let ch = if planar { p } else { s };
                                buf[(y * w + x) * spp + ch] = v;
                            }
                        }
                    }
                }
            }
        }
    }
    let assoc_alpha = ifd.get(T_EXTRA).and_then(|v| v.first().copied()) == Some(1);
    for (i, px) in img.pixels.iter_mut().enumerate() {
        let s = &buf[i * spp..(i + 1) * spp];
        let invert = |v: f32| if photometric == 0 { 1.0 - v } else { v };
        *px = match spp {
            1 => {
                let g = invert(s[0]);
                [g, g, g, 1.0]
            }
            2 => {
                let g = invert(s[0]);
                [g, g, g, s[1]]
            }
            3 => [s[0], s[1], s[2], 1.0],
            _ => [s[0], s[1], s[2], s[3]],
        };
        if assoc_alpha && px[3] > 0.0 && px[3] < 1.0 {
            for c in 0..3 {
                px[c] /= px[3];
            }
        }
    }
    Ok(img)
}

fn undo_horizontal(row: &mut [u8], spp: usize, bps: usize, big: bool) {
    if bps == 1 {
        for i in spp..row.len() {
            row[i] = row[i].wrapping_add(row[i - spp]);
        }
    } else if bps == 2 {
        let get = |r: &[u8], i: usize| if big { u16::from_be_bytes([r[i * 2], r[i * 2 + 1]]) } else { u16::from_le_bytes([r[i * 2], r[i * 2 + 1]]) };
        let n = row.len() / 2;
        for i in spp..n {
            let v = get(row, i).wrapping_add(get(row, i - spp));
            let b = if big { v.to_be_bytes() } else { v.to_le_bytes() };
            row[i * 2..i * 2 + 2].copy_from_slice(&b);
        }
    }
}

fn undo_float(row: &mut [u8], spp: usize, bps: usize, big: bool) {
    // Byte-wise differencing, then bytes regrouped from planes.
    for i in spp..row.len() {
        row[i] = row[i].wrapping_add(row[i - spp]);
    }
    let n = row.len() / bps;
    let src = row.to_vec();
    for i in 0..n {
        for k in 0..bps {
            // Planes hold the most significant byte first.
            let byte = src[k * n + i];
            let at = if big { i * bps + k } else { i * bps + (bps - 1 - k) };
            row[at] = byte;
        }
    }
}

fn packbits(data: &[u8], want: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0;
    while i < data.len() && out.len() < want {
        let n = data[i] as i8;
        i += 1;
        if n >= 0 {
            let k = n as usize + 1;
            out.extend_from_slice(&data[i..(i + k).min(data.len())]);
            i += k;
        } else if n != -128 {
            if let Some(&v) = data.get(i) {
                out.extend(std::iter::repeat_n(v, (1 - n as i32) as usize));
            }
            i += 1;
        }
    }
    out
}

/// TIFF LZW (MSB-first codes, early change).
fn lzw_decode(data: &[u8], want: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(want);
    let mut table: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8]).collect();
    table.push(Vec::new());
    table.push(Vec::new());
    let (mut bitpos, mut width) = (0usize, 9usize);
    let mut prev: Option<usize> = None;
    let read = |bitpos: usize, width: usize| -> Option<usize> {
        let mut v = 0usize;
        for k in 0..width {
            let bit = bitpos + k;
            let byte = *data.get(bit / 8)?;
            v = (v << 1) | ((byte >> (7 - bit % 8)) & 1) as usize;
        }
        Some(v)
    };
    while let Some(code) = read(bitpos, width) {
        bitpos += width;
        if code == 256 {
            table.truncate(258);
            width = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            break;
        }
        let entry = if code < table.len() {
            table[code].clone()
        } else if let Some(p) = prev {
            let mut e = table[p].clone();
            e.push(table[p][0]);
            e
        } else {
            return Err("bad TIFF LZW data".into());
        };
        out.extend_from_slice(&entry);
        if let Some(p) = prev {
            let mut n = table[p].clone();
            n.push(entry[0]);
            table.push(n);
        }
        prev = Some(code);
        // Early change: widen one code before the table fills.
        if table.len() + 1 >= (1 << width) && width < 12 {
            width += 1;
        }
        if out.len() >= want {
            break;
        }
    }
    Ok(out)
}

/// Encodes RGB(A) code values (0..1) as a 16-bit Deflate TIFF.
pub fn encode(width: u32, height: u32, pixels: &[[f32; 4]], alpha: bool) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let spp = if alpha { 4 } else { 3 };
    let rps = 16usize;
    let strips = h.div_ceil(rps);
    let mut blocks = Vec::with_capacity(strips);
    for s in 0..strips {
        let mut raw = Vec::with_capacity(w * spp * 2 * rps);
        for y in s * rps..((s + 1) * rps).min(h) {
            let mut prev = [0u16; 4];
            for x in 0..w {
                let p = pixels[y * w + x];
                for c in 0..spp {
                    let v = (p[c].clamp(0.0, 1.0) * 65535.0).round() as u16;
                    // Horizontal predictor.
                    raw.extend_from_slice(&v.wrapping_sub(prev[c]).to_le_bytes());
                    prev[c] = v;
                }
            }
        }
        blocks.push(studio_media::deflate::zlib_compress(&raw));
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&8u32.to_le_bytes());
    let entries: u16 = if alpha { 13 } else { 12 };
    let ifd_size = 2 + entries as usize * 12 + 4;
    let extra_at = 8 + ifd_size;
    // Out-of-line values: bits per sample, strip offsets, strip byte counts.
    let bits_at = extra_at;
    let offs_at = bits_at + spp * 2;
    let counts_at = offs_at + strips * 4;
    let data_at = counts_at + strips * 4;
    let mut ifd = Vec::new();
    ifd.extend_from_slice(&entries.to_le_bytes());
    let mut tag = |t: u16, ty: u16, count: u32, value: u32| {
        ifd.extend_from_slice(&t.to_le_bytes());
        ifd.extend_from_slice(&ty.to_le_bytes());
        ifd.extend_from_slice(&count.to_le_bytes());
        ifd.extend_from_slice(&value.to_le_bytes());
    };
    tag(T_WIDTH, 4, 1, width);
    tag(T_HEIGHT, 4, 1, height);
    tag(T_BITS, 3, spp as u32, bits_at as u32);
    tag(T_COMPRESSION, 3, 1, 8);
    tag(T_PHOTOMETRIC, 3, 1, 2);
    tag(T_STRIP_OFFSETS, 4, strips as u32, if strips == 1 { data_at as u32 } else { offs_at as u32 });
    tag(T_SAMPLES, 3, 1, spp as u32);
    tag(T_ROWS_PER_STRIP, 4, 1, rps as u32);
    tag(T_STRIP_BYTES, 4, strips as u32, if strips == 1 { blocks[0].len() as u32 } else { counts_at as u32 });
    tag(T_PLANAR, 3, 1, 1);
    tag(T_PREDICTOR, 3, 1, 2);
    if alpha {
        tag(T_EXTRA, 3, 1, 2);
    }
    tag(T_FORMAT, 3, 1, 1);
    ifd.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&ifd);
    for _ in 0..spp {
        out.extend_from_slice(&16u16.to_le_bytes());
    }
    let mut pos = data_at;
    for b in &blocks {
        out.extend_from_slice(&(pos as u32).to_le_bytes());
        pos += b.len();
    }
    for b in &blocks {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    }
    for b in &blocks {
        out.extend_from_slice(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_16_bit() {
        let (w, h) = (33, 21);
        let px: Vec<[f32; 4]> = (0..w * h).map(|i| [(i % 33) as f32 / 32.0, (i / 33) as f32 / 20.0, 0.25, 0.5]).collect();
        for alpha in [false, true] {
            let img = decode(&encode(w as u32, h as u32, &px, alpha)).unwrap();
            assert_eq!(img.has_alpha, alpha);
            for (a, b) in img.pixels.iter().zip(&px) {
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() < 1e-4, "{a:?} {b:?}");
                }
                if alpha {
                    assert!((a[3] - 0.5).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn lzw_and_packbits() {
        // "TOBEORNOTTOBEORTOBEORNOT" in TIFF LZW (9-bit codes, MSB first),
        // built with a tiny encoder here.
        let text = b"TOBEORNOTTOBEORTOBEORNOT#";
        let codes = lzw_encode(text);
        assert_eq!(lzw_decode(&codes, text.len()).unwrap(), text);
        assert_eq!(packbits(&[2, 1, 2, 3, 0xFD, 9], 7), vec![1, 2, 3, 9, 9, 9, 9]);
    }

    fn lzw_encode(data: &[u8]) -> Vec<u8> {
        let mut dict: std::collections::HashMap<Vec<u8>, usize> = (0..256).map(|i| (vec![i as u8], i)).collect();
        let mut next = 258;
        let mut bits: Vec<bool> = Vec::new();
        let mut width = 9;
        let emit = |c: usize, width: usize, bits: &mut Vec<bool>| {
            for k in (0..width).rev() {
                bits.push((c >> k) & 1 == 1);
            }
        };
        emit(256, width, &mut bits);
        let mut w: Vec<u8> = Vec::new();
        for &c in data {
            let mut wc = w.clone();
            wc.push(c);
            if dict.contains_key(&wc) {
                w = wc;
            } else {
                emit(dict[&w], width, &mut bits);
                dict.insert(wc, next);
                next += 1;
                if next + 1 >= (1 << width) {
                    width += 1;
                }
                w = vec![c];
            }
        }
        emit(dict[&w], width, &mut bits);
        emit(257, width, &mut bits);
        bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, b)| a | ((*b as u8) << (7 - i)))).collect()
    }
}
