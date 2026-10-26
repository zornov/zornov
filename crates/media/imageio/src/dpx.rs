// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SMPTE 268M DPX (v1 and v2), the film-scan and VFX interchange format.
//!
//! Read: the first image element; RGB (descriptor 50), RGBA (51), ABGR
//! (52) or luma (6); 8, 10 (packing 1, "method A": three samples per 32-bit
//! word, two padding bits at the bottom, or packing 0 for the rare packed
//! layout), 12 (packing 1, one sample per 16 bits) and 16-bit samples;
//! either byte order; the time code and frame rate of the TV header (or
//! the film header's frame rate); the transfer characteristic.
//!
//! Write: 10-bit (method A) or 16-bit RGB, big-endian, with transfer,
//! time code and frame rate.

use crate::{Encoding, Image, Meta, Reader, TimeCode, Transfer};

fn transfer_of(code: u8) -> Transfer {
    match code {
        1 => Transfer::PrintingDensity,
        2 => Transfer::Linear,
        3 => Transfer::Logarithmic,
        5 | 6 | 7 | 8 | 9 => Transfer::Video,
        c => Transfer::Other(c),
    }
}

fn transfer_code(t: Transfer) -> u8 {
    match t {
        Transfer::PrintingDensity => 1,
        Transfer::Linear => 2,
        Transfer::Logarithmic => 3,
        Transfer::Video => 6,
        Transfer::Other(c) => c,
    }
}

/// Decodes a DPX file.
pub fn decode(b: &[u8]) -> Result<Image, String> {
    let big = match b.get(..4) {
        Some(b"SDPX") => true,
        Some(b"XPDS") => false,
        _ => return Err("not a DPX file".into()),
    };
    let mut r = Reader::at(b, 4, big);
    let data_offset = r.u32()? as usize;
    let mut r = Reader::at(b, 768, big);
    let _orientation = r.u16()?;
    let elements = r.u16()?;
    if elements == 0 {
        return Err("DPX has no image element".into());
    }
    let width = r.u32()? as usize;
    let height = r.u32()? as usize;
    if width == 0 || height == 0 || width > 65536 || height > 65536 {
        return Err("bad DPX size".into());
    }
    // Element 1.
    let mut e = Reader::at(b, 780, big);
    let _sign = e.u32()?;
    e.bytes(16)?; // reference low / high data and quantities
    let descriptor = e.u8()?;
    let transfer = e.u8()?;
    let _colorimetric = e.u8()?;
    let bits = e.u8()?;
    let packing = e.u16()?;
    let encoding = e.u16()?;
    let elem_offset = e.u32()? as usize;
    if encoding != 0 {
        return Err("run-length encoded DPX is not supported".into());
    }
    let channels = match descriptor {
        50 => 3,
        51 | 52 => 4,
        6 | 1 | 2 | 3 | 4 => 1,
        d => return Err(format!("DPX descriptor {d} is not supported")),
    };
    let start = if elem_offset > 0 { elem_offset } else { data_offset };
    let n = width * channels;
    let max = ((1u32 << bits.min(16)) - 1) as f32;
    let mut img = Image::new(width as u32, height as u32, Encoding::Code { bits });
    img.has_alpha = channels == 4;
    let word = |p: usize| -> Result<u32, String> { Reader::at(b, p, big).u32() };
    let mut samples = vec![0u32; n];
    let mut pos = start;
    for y in 0..height {
        match (bits, packing) {
            (8, _) => {
                let row = b.get(pos..pos + n).ok_or("DPX image data is truncated")?;
                for (s, v) in samples.iter_mut().zip(row) {
                    *s = *v as u32;
                }
                pos += n.div_ceil(4) * 4;
            }
            (10, 1) => {
                let words = n.div_ceil(3);
                for i in 0..words {
                    let w = word(pos + i * 4)?;
                    for k in 0..3 {
                        let idx = i * 3 + k;
                        if idx < n {
                            samples[idx] = (w >> (22 - 10 * k as u32)) & 0x3FF;
                        }
                    }
                }
                pos += words * 4;
            }
            (10, 2) => {
                // Method B: padding at the top.
                let words = n.div_ceil(3);
                for i in 0..words {
                    let w = word(pos + i * 4)?;
                    for k in 0..3 {
                        let idx = i * 3 + k;
                        if idx < n {
                            samples[idx] = (w >> (20 - 10 * k as u32)) & 0x3FF;
                        }
                    }
                }
                pos += words * 4;
            }
            (10, 0) | (12, 0) => {
                // Tightly packed bits, lines padded to 32 bits.
                let bpl = (n * bits as usize).div_ceil(32) * 4;
                let row = b.get(pos..pos + bpl).ok_or("DPX image data is truncated")?;
                let words: Vec<u32> = row.chunks_exact(4).map(|c| if big { u32::from_be_bytes([c[0], c[1], c[2], c[3]]) } else { u32::from_le_bytes([c[0], c[1], c[2], c[3]]) }).collect();
                for (i, s) in samples.iter_mut().enumerate() {
                    let bit = i * bits as usize;
                    let (wi, off) = (bit / 32, bit % 32);
                    let lo = words[wi] as u64;
                    let hi = *words.get(wi + 1).unwrap_or(&0) as u64;
                    let v = ((hi << 32) | lo) >> off;
                    *s = (v & ((1 << bits) - 1)) as u32;
                }
                pos += bpl;
            }
            (12, _) | (16, _) => {
                let mut rr = Reader::at(b, pos, big);
                for s in samples.iter_mut() {
                    let v = rr.u16()? as u32;
                    *s = if bits == 12 { v >> 4 } else { v };
                }
                pos += (n * 2).div_ceil(4) * 4;
            }
            _ => return Err(format!("DPX {bits}-bit packing {packing} is not supported")),
        }
        for x in 0..width {
            let s = |c: usize| samples[x * channels + c] as f32 / max;
            img.pixels[y * width + x] = match (channels, descriptor) {
                (1, _) => [s(0), s(0), s(0), 1.0],
                (4, 52) => [s(3), s(2), s(1), s(0)],
                (4, _) => [s(0), s(1), s(2), s(3)],
                _ => [s(0), s(1), s(2), 1.0],
            };
        }
    }
    let mut meta = Meta { transfer: Some(transfer_of(transfer)), ..Meta::default() };
    if b.len() >= 1956 {
        let mut tv = Reader::at(b, 1920, big);
        let tc = tv.u32()?;
        if tc != 0xFFFF_FFFF && tc != 0 {
            meta.timecode = Some(TimeCode::from_bcd(tc));
        }
        let fr = Reader::at(b, 1940, big).f32()?;
        if fr.is_finite() && fr > 0.0 && fr < 1000.0 {
            meta.frame_rate = Some(fr as f64);
        }
    }
    if meta.frame_rate.is_none() && b.len() >= 1728 {
        let fr = Reader::at(b, 1724, big).f32()?;
        if fr.is_finite() && fr > 0.0 && fr < 1000.0 {
            meta.frame_rate = Some(fr as f64);
        }
    }
    if b.len() >= 1588 {
        // Input device name (orientation header) often carries the camera
        // roll / reel.
        let dev = &b[1556..1588];
        let s: String = String::from_utf8_lossy(dev).trim_end_matches('\0').trim().to_string();
        if !s.is_empty() && s.chars().all(|c| !c.is_control()) {
            meta.reel = Some(s);
        }
    }
    img.meta = meta;
    Ok(img)
}

/// Write options.
#[derive(Clone, Debug, PartialEq)]
pub struct WriteOptions {
    /// 10 (packed method A) or 16 bits.
    pub bits: u8,
    pub transfer: Transfer,
    pub timecode: Option<TimeCode>,
    pub frame_rate: Option<f64>,
    pub reel: Option<String>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { bits: 10, transfer: Transfer::Video, timecode: None, frame_rate: None, reel: None }
    }
}

/// Encodes RGB code values (0..1) as a big-endian DPX file.
pub fn encode(width: u32, height: u32, pixels: &[[f32; 4]], opts: &WriteOptions) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let bits = if opts.bits == 16 { 16u8 } else { 10u8 };
    let row_bytes = if bits == 10 { w * 4 } else { (w * 3 * 2).div_ceil(4) * 4 };
    let size = 2048 + row_bytes * h;
    let mut out = vec![0u8; size];
    let put32 = |out: &mut Vec<u8>, at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_be_bytes());
    let put16 = |out: &mut Vec<u8>, at: usize, v: u16| out[at..at + 2].copy_from_slice(&v.to_be_bytes());
    let putstr = |out: &mut Vec<u8>, at: usize, len: usize, s: &str| {
        let b = s.as_bytes();
        let n = b.len().min(len - 1);
        out[at..at + n].copy_from_slice(&b[..n]);
    };
    out[0..4].copy_from_slice(b"SDPX");
    put32(&mut out, 4, 2048);
    putstr(&mut out, 8, 8, "V2.0");
    put32(&mut out, 16, size as u32);
    put32(&mut out, 20, 1); // ditto key: new image
    put32(&mut out, 24, 1664); // generic header size
    put32(&mut out, 28, 384); // industry header size
    put32(&mut out, 32, 0);
    putstr(&mut out, 160, 100, "Zornov");
    put32(&mut out, 660, 0xFFFF_FFFF);
    // Image information.
    put16(&mut out, 768, 0);
    put16(&mut out, 770, 1);
    put32(&mut out, 772, width);
    put32(&mut out, 776, height);
    put32(&mut out, 780, 0); // unsigned
    put32(&mut out, 784, 0);
    out[788..792].copy_from_slice(&0f32.to_be_bytes());
    put32(&mut out, 792, if bits == 10 { 1023 } else { 65535 });
    out[796..800].copy_from_slice(&2.047f32.to_be_bytes());
    out[800] = 50;
    out[801] = transfer_code(opts.transfer);
    out[802] = transfer_code(opts.transfer);
    out[803] = bits;
    put16(&mut out, 804, if bits == 10 { 1 } else { 0 });
    put16(&mut out, 806, 0);
    put32(&mut out, 808, 2048);
    // Orientation header: offsets and input device (reel).
    if let Some(reel) = &opts.reel {
        putstr(&mut out, 1556, 32, reel);
    }
    // Film and TV headers: undefined fields are all ones.
    for i in (1664..2048).step_by(4) {
        put32(&mut out, i, 0xFFFF_FFFF);
    }
    for (at, len) in [(1664usize, 2usize), (1666, 2), (1668, 2), (1670, 6), (1676, 4), (1680, 32), (1732, 32), (1764, 100)] {
        out[at..at + len].fill(0);
    }
    put32(&mut out, 1920, opts.timecode.map_or(0xFFFF_FFFF, |t| t.to_bcd()));
    put32(&mut out, 1924, 0);
    out[1928..1932].fill(0);
    if let Some(fr) = opts.frame_rate {
        out[1940..1944].copy_from_slice(&(fr as f32).to_be_bytes());
        out[1724..1728].copy_from_slice(&(fr as f32).to_be_bytes());
    }
    let q = |v: f32, m: f32| (v.clamp(0.0, 1.0) * m).round() as u32;
    let mut pos = 2048;
    for y in 0..h {
        for x in 0..w {
            let p = pixels[y * w + x];
            if bits == 10 {
                let wv = (q(p[0], 1023.0) << 22) | (q(p[1], 1023.0) << 12) | (q(p[2], 1023.0) << 2);
                out[pos..pos + 4].copy_from_slice(&wv.to_be_bytes());
                pos += 4;
            } else {
                for c in 0..3 {
                    out[pos..pos + 2].copy_from_slice(&(q(p[c], 65535.0) as u16).to_be_bytes());
                    pos += 2;
                }
            }
        }
        if bits == 16 {
            pos = 2048 + (y + 1) * row_bytes;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_10_and_16_bit() {
        let (w, h) = (19, 7);
        let px: Vec<[f32; 4]> = (0..w * h).map(|i| [(i % 19) as f32 / 18.0, (i / 19) as f32 / 6.0, 0.5, 1.0]).collect();
        for bits in [10u8, 16] {
            let opts = WriteOptions { bits, transfer: Transfer::PrintingDensity, timecode: Some(TimeCode { hours: 1, minutes: 2, seconds: 3, frames: 4, drop_frame: false }), frame_rate: Some(24.0), reel: Some("A001C003".into()) };
            let img = decode(&encode(w as u32, h as u32, &px, &opts)).unwrap();
            assert_eq!((img.width, img.height), (w as u32, h as u32));
            assert_eq!(img.encoding, Encoding::Code { bits });
            let tol = if bits == 10 { 0.5 / 1023.0 + 1e-6 } else { 0.5 / 65535.0 + 1e-6 };
            for (a, b) in img.pixels.iter().zip(&px) {
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() <= tol, "{bits}: {a:?} vs {b:?}");
                }
            }
            assert_eq!(img.meta.transfer, Some(Transfer::PrintingDensity));
            assert_eq!(img.meta.timecode.unwrap().to_string(), "01:02:03:04");
            assert_eq!(img.meta.frame_rate, Some(24.0));
            assert_eq!(img.meta.reel.as_deref(), Some("A001C003"));
        }
    }

    #[test]
    fn little_endian_method_a() {
        // A hand-made 2×1 LE 10-bit file.
        let mut b = encode(2, 1, &[[1.0, 0.0, 0.5, 1.0], [0.0, 1.0, 0.0, 1.0]], &WriteOptions::default());
        b[0..4].copy_from_slice(b"XPDS");
        for chunk in [4usize, 768, 772, 776, 780, 792, 804, 808, 1920] {
            let len = if chunk == 768 || chunk == 804 { 2 } else { 4 };
            b[chunk..chunk + len].reverse();
            if chunk == 768 {
                b[770..772].reverse();
            }
        }
        for i in (2048..b.len()).step_by(4) {
            b[i..i + 4].reverse();
        }
        let img = decode(&b).unwrap();
        assert_eq!(img.pixels[0][0], 1.0);
        assert!((img.pixels[0][2] - 512.0 / 1023.0).abs() < 1e-6);
        assert_eq!(img.pixels[1][1], 1.0);
    }
}
