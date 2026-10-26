// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenEXR (single-part scanline images).
//!
//! Read: channels `R`, `G`, `B`, `A` (or `Y` for grey; layer prefixes such
//! as `beauty.R` are accepted when there is no bare `R`), pixel types
//! UINT, HALF and FLOAT, compression NONE, RLE, ZIPS and ZIP, any data
//! window (pixels outside it are transparent black), the
//! `chromaticities`, `timeCode`, `framesPerSecond` and
//! `acesImageContainerFlag` attributes. Tiled, deep, multi-part images and
//! the PIZ / PXR24 / B44 / DWA codecs are reported as unsupported.
//!
//! Write: RGB or RGBA, HALF or FLOAT, NONE or ZIP, with optional
//! chromaticities, time code and frame rate — an ACES2065-1 container when
//! asked for (uncompressed, AP0 chromaticities, the container flag).

use crate::half;
use crate::{Encoding, Image, Meta, Reader, TimeCode};

const MAGIC: [u8; 4] = [0x76, 0x2f, 0x31, 0x01];

/// Compression methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Rle,
    Zips,
    Zip,
}

impl Compression {
    fn code(self) -> u8 {
        match self {
            Compression::None => 0,
            Compression::Rle => 1,
            Compression::Zips => 2,
            Compression::Zip => 3,
        }
    }

    fn lines(self) -> usize {
        match self {
            Compression::Zip => 16,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug)]
struct Channel {
    name: String,
    kind: i32,
}

impl Channel {
    fn size(&self) -> usize {
        if self.kind == 1 { 2 } else { 4 }
    }
}

/// Decodes an EXR file.
pub fn decode(b: &[u8]) -> Result<Image, String> {
    if !b.starts_with(&MAGIC) {
        return Err("not an OpenEXR file".into());
    }
    let mut r = Reader::at(b, 4, false);
    let version = r.u32()?;
    if version & 0x200 != 0 {
        return Err("tiled OpenEXR images are not supported (save as scanline)".into());
    }
    if version & 0x800 != 0 || version & 0x1000 != 0 {
        return Err("deep or multi-part OpenEXR files are not supported".into());
    }
    let mut channels: Vec<Channel> = Vec::new();
    let mut compression = 0u8;
    let mut dw = [0i32; 4];
    let mut disp = [0i32; 4];
    let mut meta = Meta::default();
    loop {
        let name = cstr(&mut r)?;
        if name.is_empty() {
            break;
        }
        let ty = cstr(&mut r)?;
        let size = r.u32()? as usize;
        let start = r.pos;
        let mut v = Reader::at(b, start, false);
        match (name.as_str(), ty.as_str()) {
            ("channels", "chlist") => loop {
                let n = cstr(&mut v)?;
                if n.is_empty() {
                    break;
                }
                let kind = v.i32()?;
                v.bytes(4)?; // pLinear + reserved
                let (xs, ys) = (v.i32()?, v.i32()?);
                if xs != 1 || ys != 1 {
                    return Err(format!("channel {n} is subsampled, which is not supported"));
                }
                channels.push(Channel { name: n, kind });
            },
            ("compression", _) => compression = v.u8()?,
            ("dataWindow", "box2i") => dw = [v.i32()?, v.i32()?, v.i32()?, v.i32()?],
            ("displayWindow", "box2i") => disp = [v.i32()?, v.i32()?, v.i32()?, v.i32()?],
            ("chromaticities", "chromaticities") => {
                let mut c = [0f32; 8];
                for x in &mut c {
                    *x = v.f32()?;
                }
                meta.chromaticities = Some(c);
            }
            ("timeCode", "timecode") => meta.timecode = Some(TimeCode::from_bcd(v.u32()?)),
            ("framesPerSecond", "rational") => {
                let (n, d) = (v.i32()?, v.u32()?);
                if d > 0 {
                    meta.frame_rate = Some(n as f64 / d as f64);
                }
            }
            ("acesImageContainerFlag", "int") => meta.aces_container = v.i32()? == 1,
            ("reelName", "string") | ("reel", "string") => meta.reel = Some(String::from_utf8_lossy(v.bytes(size)?).into_owned()),
            _ => {}
        }
        r.pos = start + size;
    }
    let comp = match compression {
        0 => Compression::None,
        1 => Compression::Rle,
        2 => Compression::Zips,
        3 => Compression::Zip,
        4 => return Err("PIZ-compressed OpenEXR is not supported yet (re-save as ZIP)".into()),
        5 => return Err("PXR24-compressed OpenEXR is not supported (re-save as ZIP)".into()),
        6 | 7 => return Err("B44-compressed OpenEXR is not supported (re-save as ZIP)".into()),
        8 | 9 => return Err("DWA-compressed OpenEXR is not supported (re-save as ZIP)".into()),
        c => return Err(format!("unknown OpenEXR compression {c}")),
    };
    let (dw_w, dw_h) = ((dw[2] - dw[0] + 1) as usize, (dw[3] - dw[1] + 1) as usize);
    let (w, h) = ((disp[2] - disp[0] + 1).max(1) as usize, (disp[3] - disp[1] + 1).max(1) as usize);
    if dw_w == 0 || dw_h == 0 || dw_w > 65536 || dw_h > 65536 || w > 65536 || h > 65536 {
        return Err("bad OpenEXR data window".into());
    }
    // Channels are stored sorted by name; find R G B A (or a layer's).
    let pick = |c: &str| -> Option<usize> {
        channels.iter().position(|ch| ch.name == c).or_else(|| channels.iter().position(|ch| ch.name.rsplit('.').next() == Some(c)))
    };
    let (ri, gi, bi, ai, yi) = (pick("R"), pick("G"), pick("B"), pick("A"), pick("Y"));
    if ri.is_none() && yi.is_none() {
        return Err(format!("no R/G/B or Y channels (found {})", channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let line_bytes: usize = channels.iter().map(|c| c.size() * dw_w).sum();
    let block = comp.lines();
    let chunks = dw_h.div_ceil(block);
    let mut img = Image::new(w as u32, h as u32, Encoding::Linear);
    img.has_alpha = ai.is_some();
    // Outside the data window is transparent black.
    for p in &mut img.pixels {
        *p = [0.0; 4];
    }
    let table = r.pos;
    for c in 0..chunks {
        let off = Reader::at(b, table + c * 8, false).u64()? as usize;
        let mut cr = Reader::at(b, off, false);
        let y0 = cr.i32()?;
        let size = cr.u32()? as usize;
        let data = cr.bytes(size)?;
        let first = (y0 - dw[1]) as usize;
        let lines = block.min(dw_h.saturating_sub(first));
        let want = line_bytes * lines;
        let raw = match comp {
            Compression::None => data.to_vec(),
            _ if size == want => data.to_vec(),
            Compression::Rle => unpredict(rle_decode(data, want)?),
            Compression::Zips | Compression::Zip => unpredict(studio_media::deflate::zlib_decompress(data, want).map_err(|e| format!("bad ZIP data: {e:?}"))?),
        };
        if raw.len() < want {
            return Err("OpenEXR block is short".into());
        }
        for l in 0..lines {
            let row = &raw[l * line_bytes..(l + 1) * line_bytes];
            let fy = dw[1] + (first + l) as i32;
            let oy = fy - disp[1];
            if oy < 0 || oy as usize >= h {
                continue;
            }
            let mut offs = Vec::with_capacity(channels.len());
            let mut o = 0;
            for ch in &channels {
                offs.push(o);
                o += ch.size() * dw_w;
            }
            let sample = |ci: usize, x: usize| -> f32 {
                let ch = &channels[ci];
                let at = offs[ci] + x * ch.size();
                match ch.kind {
                    0 => u32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]]) as f32,
                    1 => half::to_f32(u16::from_le_bytes([row[at], row[at + 1]])),
                    _ => f32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]]),
                }
            };
            for x in 0..dw_w {
                let ox = dw[0] + x as i32 - disp[0];
                if ox < 0 || ox as usize >= w {
                    continue;
                }
                let px = &mut img.pixels[oy as usize * w + ox as usize];
                if let (Some(r), Some(g), Some(bb)) = (ri, gi, bi) {
                    *px = [sample(r, x), sample(g, x), sample(bb, x), 1.0];
                } else if let Some(y) = yi.or(ri) {
                    let v = sample(y, x);
                    *px = [v, v, v, 1.0];
                }
                if let Some(a) = ai {
                    px[3] = sample(a, x);
                }
            }
        }
    }
    // EXR stores premultiplied colour; hand out straight alpha.
    if img.has_alpha {
        for p in &mut img.pixels {
            if p[3] > 0.0 && p[3] < 1.0 {
                p[0] /= p[3];
                p[1] /= p[3];
                p[2] /= p[3];
            }
        }
    }
    img.meta = meta;
    Ok(img)
}

fn cstr(r: &mut Reader) -> Result<String, String> {
    let start = r.pos;
    let end = r.b[start..].iter().position(|c| *c == 0).ok_or("unterminated name")? + start;
    r.pos = end + 1;
    Ok(String::from_utf8_lossy(&r.b[start..end]).into_owned())
}

fn rle_decode(data: &[u8], want: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0;
    while i < data.len() && out.len() < want {
        let n = data[i] as i8;
        i += 1;
        if n < 0 {
            let k = (-(n as i32)) as usize;
            out.extend_from_slice(data.get(i..i + k).ok_or("bad RLE data")?);
            i += k;
        } else {
            let v = *data.get(i).ok_or("bad RLE data")?;
            out.extend(std::iter::repeat_n(v, n as usize + 1));
            i += 1;
        }
    }
    Ok(out)
}

fn rle_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let mut run = 1;
        while i + run < data.len() && data[i + run] == data[i] && run < 128 {
            run += 1;
        }
        if run >= 3 {
            out.push((run - 1) as u8);
            out.push(data[i]);
            i += run;
        } else {
            let start = i;
            while i < data.len() && i - start < 127 && !(i + 2 < data.len() && data[i] == data[i + 1] && data[i] == data[i + 2]) {
                i += 1;
            }
            out.push((-((i - start) as i32)) as u8);
            out.extend_from_slice(&data[start..i]);
        }
    }
    out
}

/// Undoes the ZIP / RLE predictor and byte interleaving.
fn unpredict(mut t: Vec<u8>) -> Vec<u8> {
    for i in 1..t.len() {
        t[i] = t[i - 1].wrapping_add(t[i]).wrapping_sub(128);
    }
    let n = t.len();
    let half = n.div_ceil(2);
    let mut out = vec![0u8; n];
    for k in 0..n {
        out[k] = if k % 2 == 0 { t[k / 2] } else { t[half + k / 2] };
    }
    out
}

fn predict(raw: &[u8]) -> Vec<u8> {
    let n = raw.len();
    let half = n.div_ceil(2);
    let mut t = vec![0u8; n];
    for k in 0..n {
        if k % 2 == 0 {
            t[k / 2] = raw[k];
        } else {
            t[half + k / 2] = raw[k];
        }
    }
    for i in (1..n).rev() {
        t[i] = t[i].wrapping_sub(t[i - 1]).wrapping_add(128);
    }
    t
}

/// Write options.
#[derive(Clone, Debug, PartialEq)]
pub struct WriteOptions {
    /// HALF (16-bit float) instead of FLOAT.
    pub half: bool,
    pub compression: Compression,
    pub alpha: bool,
    pub chromaticities: Option<[f32; 8]>,
    pub timecode: Option<TimeCode>,
    /// Frame rate as a fraction.
    pub frame_rate: Option<(i32, u32)>,
    /// Write an ACES2065-1 container (forces NONE compression, HALF, AP0
    /// chromaticities and the container flag; pixels must be AP0 linear).
    pub aces: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { half: true, compression: Compression::Zip, alpha: false, chromaticities: None, timecode: None, frame_rate: None, aces: false }
    }
}

/// ACES AP0 chromaticities (red, green, blue, white xy).
pub const AP0: [f32; 8] = [0.7347, 0.2653, 0.0, 1.0, 0.0001, -0.077, 0.32168, 0.33767];
/// Rec.709 / sRGB chromaticities.
pub const REC709: [f32; 8] = [0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.3290];

/// Encodes linear RGBA (straight alpha) as an EXR file.
pub fn encode(width: u32, height: u32, pixels: &[[f32; 4]], opts: &WriteOptions) -> Vec<u8> {
    let mut o = opts.clone();
    if o.aces {
        o.compression = Compression::None;
        o.half = true;
        o.chromaticities = Some(AP0);
    }
    let (w, h) = (width as usize, height as usize);
    let names: Vec<&str> = if o.alpha { vec!["A", "B", "G", "R"] } else { vec!["B", "G", "R"] };
    let kind: i32 = if o.half { 1 } else { 2 };
    let size = if o.half { 2 } else { 4 };
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    let attr = |out: &mut Vec<u8>, name: &str, ty: &str, val: &[u8]| {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.extend_from_slice(ty.as_bytes());
        out.push(0);
        out.extend_from_slice(&(val.len() as u32).to_le_bytes());
        out.extend_from_slice(val);
    };
    let mut ch = Vec::new();
    for n in &names {
        ch.extend_from_slice(n.as_bytes());
        ch.push(0);
        ch.extend_from_slice(&kind.to_le_bytes());
        ch.extend_from_slice(&[0, 0, 0, 0]);
        ch.extend_from_slice(&1i32.to_le_bytes());
        ch.extend_from_slice(&1i32.to_le_bytes());
    }
    ch.push(0);
    if o.aces {
        attr(&mut out, "acesImageContainerFlag", "int", &1i32.to_le_bytes());
    }
    attr(&mut out, "channels", "chlist", &ch);
    if let Some(c) = o.chromaticities {
        let v: Vec<u8> = c.iter().flat_map(|f| f.to_le_bytes()).collect();
        attr(&mut out, "chromaticities", "chromaticities", &v);
    }
    attr(&mut out, "compression", "compression", &[o.compression.code()]);
    let bx: Vec<u8> = [0i32, 0, width as i32 - 1, height as i32 - 1].iter().flat_map(|v| v.to_le_bytes()).collect();
    attr(&mut out, "dataWindow", "box2i", &bx);
    attr(&mut out, "displayWindow", "box2i", &bx);
    if let Some((n, d)) = o.frame_rate {
        let v: Vec<u8> = n.to_le_bytes().into_iter().chain(d.to_le_bytes()).collect();
        attr(&mut out, "framesPerSecond", "rational", &v);
    }
    attr(&mut out, "lineOrder", "lineOrder", &[0]);
    attr(&mut out, "pixelAspectRatio", "float", &1f32.to_le_bytes());
    attr(&mut out, "screenWindowCenter", "v2f", &[0u8; 8]);
    attr(&mut out, "screenWindowWidth", "float", &1f32.to_le_bytes());
    if let Some(tc) = o.timecode {
        let v: Vec<u8> = tc.to_bcd().to_le_bytes().into_iter().chain(0u32.to_le_bytes()).collect();
        attr(&mut out, "timeCode", "timecode", &v);
    }
    out.push(0);
    let block = o.compression.lines();
    let chunks = h.div_ceil(block);
    let table = out.len();
    out.resize(table + chunks * 8, 0);
    let chan = |p: &[f32; 4], n: &str| -> f32 {
        let a = if o.alpha { p[3] } else { 1.0 };
        // EXR stores premultiplied colour.
        match n {
            "R" => p[0] * a,
            "G" => p[1] * a,
            "B" => p[2] * a,
            _ => p[3],
        }
    };
    for c in 0..chunks {
        let y0 = c * block;
        let lines = block.min(h - y0);
        let mut raw = Vec::with_capacity(lines * w * names.len() * size);
        for y in y0..y0 + lines {
            let row = &pixels[y * w..(y + 1) * w];
            for n in &names {
                for p in row {
                    let v = chan(p, n);
                    if o.half {
                        raw.extend_from_slice(&half::from_f32(v).to_le_bytes());
                    } else {
                        raw.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        let data = match o.compression {
            Compression::None => raw,
            Compression::Rle => {
                let e = rle_encode(&predict(&raw));
                if e.len() < raw.len() { e } else { raw }
            }
            Compression::Zips | Compression::Zip => {
                let e = studio_media::deflate::zlib_compress(&predict(&raw));
                if e.len() < raw.len() { e } else { raw }
            }
        };
        let off = out.len() as u64;
        out[table + c * 8..table + c * 8 + 8].copy_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&(y0 as i32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(w: u32, h: u32) -> Vec<[f32; 4]> {
        (0..w * h).map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            [x * 0.37, y * 0.11 + 0.18, (x - y) * 0.05, 0.25 + 0.75 * ((i % 7) as f32 / 6.0)]
        }).collect()
    }

    #[test]
    fn round_trips_every_codec() {
        let (w, h) = (37, 23);
        let px = ramp(w, h);
        for comp in [Compression::None, Compression::Rle, Compression::Zips, Compression::Zip] {
            for half_floats in [true, false] {
                let opts = WriteOptions { half: half_floats, compression: comp, alpha: true, timecode: Some(TimeCode { hours: 1, minutes: 0, seconds: 10, frames: 5, drop_frame: false }), frame_rate: Some((24000, 1001)), ..Default::default() };
                let bytes = encode(w, h, &px, &opts);
                let img = decode(&bytes).unwrap_or_else(|e| panic!("{comp:?}: {e}"));
                assert_eq!((img.width, img.height), (w, h));
                assert!(img.has_alpha);
                for (a, b) in img.pixels.iter().zip(&px) {
                    for c in 0..4 {
                        let tol = if half_floats { 2e-3 * b[c].abs().max(1.0) } else { 1e-6 * b[c].abs().max(1.0) };
                        assert!((a[c] - b[c]).abs() <= tol * 4.0, "{comp:?} half={half_floats}: {a:?} vs {b:?}");
                    }
                }
                assert_eq!(img.meta.timecode.unwrap().to_string(), "01:00:10:05");
                assert!((img.meta.frame_rate.unwrap() - 23.976).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn aces_container() {
        let px = vec![[0.18f32, 0.18, 0.18, 1.0]; 16];
        let bytes = encode(4, 4, &px, &WriteOptions { aces: true, ..Default::default() });
        let img = decode(&bytes).unwrap();
        assert!(img.meta.aces_container);
        assert_eq!(img.meta.chromaticities, Some(AP0));
        assert!(!img.has_alpha);
        assert!((img.pixels[5][1] - 0.18).abs() < 1e-3);
    }

    #[test]
    fn rle_round_trip() {
        let data: Vec<u8> = (0..1000).map(|i| if i % 50 < 30 { 7 } else { (i * 13) as u8 }).collect();
        assert_eq!(rle_decode(&rle_encode(&data), data.len()).unwrap(), data);
        assert_eq!(unpredict(predict(&data)), data);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope").is_err());
        let mut bytes = encode(4, 4, &[[0.0; 4]; 16], &WriteOptions::default());
        bytes.truncate(bytes.len() - 5);
        assert!(decode(&bytes).is_err());
    }
}
