// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animated GIF (GIF89a) writing and reading.
//!
//! Every frame gets its own palette of up to 256 colours chosen by median cut
//! over a 15-bit colour histogram, optional Floyd–Steinberg dithering, and
//! variable-width LZW compression (clear code at 4096 entries). Frames are
//! written as they arrive, so a long GIF never sits in memory.
//!
//! The reader decodes what the writer produces (and ordinary GIFs: global or
//! local palettes, interlacing, transparency, disposal), for tests and import.

use crate::RgbaImage;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GifOptions {
    /// Palette size per frame, 2..=256.
    pub colors: usize,
    /// Floyd–Steinberg error diffusion.
    pub dither: bool,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self { colors: 256, dither: true }
    }
}

// --- palette ------------------------------------------------------------------

const BINS: usize = 1 << 15;

fn key(r: u8, g: u8, b: u8) -> usize {
    ((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3)
}

/// Median-cut palette for an image: at most `colors` entries, each the
/// pixel-weighted mean of its box.
pub fn median_cut(img: &RgbaImage, colors: usize) -> Vec<[u8; 3]> {
    let colors = colors.clamp(2, 256);
    let mut count = vec![0u32; BINS];
    let mut sum = vec![[0u64; 3]; BINS];
    for p in img.pixels.chunks_exact(4) {
        let k = key(p[0], p[1], p[2]);
        count[k] += 1;
        sum[k][0] += u64::from(p[0]);
        sum[k][1] += u64::from(p[1]);
        sum[k][2] += u64::from(p[2]);
    }
    let bins: Vec<u16> = (0..BINS).filter(|&k| count[k] > 0).map(|k| k as u16).collect();
    if bins.is_empty() {
        return vec![[0, 0, 0]];
    }
    let chan = |k: u16, c: usize| ((k as usize >> (10 - 5 * c)) & 31) as u8;

    struct Box_ {
        bins: Vec<u16>,
        weight: u64,
        /// (channel, extent) of the widest channel.
        axis: (usize, u8),
    }
    let make = |bins: Vec<u16>| {
        let weight = bins.iter().map(|&k| u64::from(count[k as usize])).sum();
        let mut axis = (0, 0);
        for c in 0..3 {
            let (lo, hi) = bins.iter().fold((31u8, 0u8), |(lo, hi), &k| (lo.min(chan(k, c)), hi.max(chan(k, c))));
            let ext = hi - lo;
            if ext > axis.1 {
                axis = (c, ext);
            }
        }
        Box_ { bins, weight, axis }
    };
    let mut boxes = vec![make(bins)];
    while boxes.len() < colors {
        // Split the box with the most pixels × extent that can still be split.
        let Some(i) = (0..boxes.len()).filter(|&i| boxes[i].bins.len() > 1 && boxes[i].axis.1 > 0).max_by_key(|&i| boxes[i].weight * (u64::from(boxes[i].axis.1) + 1)) else {
            break;
        };
        let mut b = boxes.swap_remove(i);
        let c = b.axis.0;
        b.bins.sort_unstable_by_key(|&k| chan(k, c));
        // Weighted median, keeping both halves non-empty.
        let half = b.weight / 2;
        let mut acc = 0u64;
        let mut cut = 1;
        for (j, &k) in b.bins.iter().enumerate() {
            acc += u64::from(count[k as usize]);
            if acc >= half {
                cut = j + 1;
                break;
            }
        }
        let cut = cut.clamp(1, b.bins.len() - 1);
        let right = b.bins.split_off(cut);
        boxes.push(make(b.bins));
        boxes.push(make(right));
    }
    boxes
        .iter()
        .map(|b| {
            let mut s = [0u64; 3];
            for &k in &b.bins {
                for c in 0..3 {
                    s[c] += sum[k as usize][c];
                }
            }
            let w = b.weight.max(1);
            [(s[0] / w) as u8, (s[1] / w) as u8, (s[2] / w) as u8]
        })
        .collect()
}

/// Maps colours to palette indices through a lazily filled 15-bit cache.
struct Mapper<'a> {
    palette: &'a [[u8; 3]],
    cache: Vec<u16>,
}

impl<'a> Mapper<'a> {
    fn new(palette: &'a [[u8; 3]]) -> Self {
        Self { palette, cache: vec![u16::MAX; BINS] }
    }
    fn nearest(&mut self, r: u8, g: u8, b: u8) -> u8 {
        let k = key(r, g, b);
        if self.cache[k] != u16::MAX {
            return self.cache[k] as u8;
        }
        // Centre of the bin.
        let (r, g, b) = (i32::from(r & !7) + 4, i32::from(g & !7) + 4, i32::from(b & !7) + 4);
        let mut best = (i32::MAX, 0usize);
        for (i, p) in self.palette.iter().enumerate() {
            let (dr, dg, db) = (r - i32::from(p[0]), g - i32::from(p[1]), b - i32::from(p[2]));
            // Perceptual-ish weights.
            let d = 2 * dr * dr + 4 * dg * dg + 3 * db * db;
            if d < best.0 {
                best = (d, i);
            }
        }
        self.cache[k] = best.1 as u16;
        best.1 as u8
    }
}

/// Palette indices for every pixel, optionally Floyd–Steinberg dithered.
pub fn index_pixels(img: &RgbaImage, palette: &[[u8; 3]], dither: bool) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut map = Mapper::new(palette);
    let mut out = vec![0u8; w * h];
    if !dither {
        for (o, p) in out.iter_mut().zip(img.pixels.chunks_exact(4)) {
            *o = map.nearest(p[0], p[1], p[2]);
        }
        return out;
    }
    // Error rows (current and next) in 1/16 units, 3 channels.
    let mut cur = vec![0i32; (w + 2) * 3];
    let mut next = vec![0i32; (w + 2) * 3];
    for y in 0..h {
        next.iter_mut().for_each(|e| *e = 0);
        for x in 0..w {
            let p = &img.pixels[(y * w + x) * 4..][..3];
            let e = &cur[(x + 1) * 3..][..3];
            let v = [0, 1, 2].map(|c| (i32::from(p[c]) + e[c] / 16).clamp(0, 255));
            let i = map.nearest(v[0] as u8, v[1] as u8, v[2] as u8);
            out[y * w + x] = i;
            let q = palette[i as usize];
            for c in 0..3 {
                let err = v[c] - i32::from(q[c]);
                cur[(x + 2) * 3 + c] += err * 7;
                next[x * 3 + c] += err * 3;
                next[(x + 1) * 3 + c] += err * 5;
                next[(x + 2) * 3 + c] += err;
            }
        }
        std::mem::swap(&mut cur, &mut next);
    }
    out
}

// --- LZW ----------------------------------------------------------------------

struct BitPacker {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitPacker {
    fn put(&mut self, code: u16, size: u32) {
        self.acc |= u32::from(code) << self.n;
        self.n += size;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

const MAX_CODES: u16 = 4096;
const TABLE: usize = 8192;

/// GIF-flavoured LZW of palette indices (LSB-first codes).
pub fn lzw_encode(data: &[u8], min_code_size: u8) -> Vec<u8> {
    let mcs = u32::from(min_code_size.clamp(2, 8));
    let clear = 1u16 << mcs;
    let eoi = clear + 1;
    let mut bits = BitPacker { out: Vec::with_capacity(data.len() / 2), acc: 0, n: 0 };
    // Open-addressed (prefix, byte) → code table.
    let mut keys = vec![0u32; TABLE];
    let mut vals = vec![0u16; TABLE];
    let mut size = mcs + 1;
    let mut max_code = eoi; // last assigned code
    bits.put(clear, size);
    let Some((&first, rest)) = data.split_first() else {
        bits.put(eoi, size);
        return bits.finish();
    };
    let mut cur = u16::from(first) & (clear - 1);
    for &b in rest {
        let b = u16::from(b) & (clear - 1);
        let k = (u32::from(cur) << 8 | u32::from(b)) + 1;
        let mut h = ((k.wrapping_mul(2_654_435_761)) >> 19) as usize & (TABLE - 1);
        loop {
            if keys[h] == k {
                cur = vals[h];
                break;
            }
            if keys[h] == 0 {
                bits.put(cur, size);
                max_code += 1;
                keys[h] = k;
                vals[h] = max_code;
                if u32::from(max_code) >= 1 << size && size < 12 {
                    size += 1;
                }
                if max_code == MAX_CODES - 1 {
                    bits.put(clear, size);
                    keys.iter_mut().for_each(|k| *k = 0);
                    size = mcs + 1;
                    max_code = eoi;
                }
                cur = b;
                break;
            }
            h = (h + 1) & (TABLE - 1);
        }
    }
    bits.put(cur, size);
    // The decoder adds one more entry after reading the last code.
    if max_code + 1 < MAX_CODES && u32::from(max_code + 1) >= 1 << size && size < 12 {
        size += 1;
    }
    bits.put(eoi, size);
    bits.finish()
}

/// Decodes GIF LZW data into at most `limit` indices.
pub fn lzw_decode(data: &[u8], min_code_size: u8, limit: usize) -> Result<Vec<u8>, String> {
    let mcs = u32::from(min_code_size);
    if !(1..=11).contains(&mcs) {
        return Err(format!("bad LZW code size {mcs}"));
    }
    let clear = 1u32 << mcs;
    let eoi = clear + 1;
    let mut prefix = vec![0u16; 4096];
    let mut suffix = vec![0u8; 4096];
    let mut length = vec![0u16; 4096];
    for i in 0..clear as usize {
        suffix[i] = i as u8;
        length[i] = 1;
    }
    let mut out = Vec::with_capacity(limit);
    let (mut acc, mut n, mut pos) = (0u32, 0u32, 0usize);
    let mut size = mcs + 1;
    let mut next = eoi + 1;
    let mut prev: Option<u32> = None;
    let mut stack = Vec::with_capacity(4096);
    loop {
        while n < size {
            let Some(&b) = data.get(pos) else { return Ok(out) };
            acc |= u32::from(b) << n;
            n += 8;
            pos += 1;
        }
        let code = acc & ((1 << size) - 1);
        acc >>= size;
        n -= size;
        if code == clear {
            size = mcs + 1;
            next = eoi + 1;
            prev = None;
            continue;
        }
        if code == eoi {
            return Ok(out);
        }
        let Some(p) = prev else {
            if code >= clear {
                return Err("LZW stream starts with an unknown code".into());
            }
            out.push(code as u8);
            prev = Some(code);
            continue;
        };
        let known = code < next;
        if !known && code != next {
            return Err("corrupt LZW code".into());
        }
        // Emit the string for `code` (or prev + first(prev) for the KwKwK case).
        let mut c = if known { code } else { p };
        stack.clear();
        while c >= clear {
            stack.push(suffix[c as usize]);
            c = u32::from(prefix[c as usize]);
        }
        stack.push(c as u8);
        let first = c as u8;
        out.extend(stack.iter().rev());
        if !known {
            out.push(first);
        }
        if next < 4096 {
            prefix[next as usize] = p as u16;
            suffix[next as usize] = first;
            length[next as usize] = length[p as usize] + 1;
            next += 1;
            if next == 1 << size && size < 12 {
                size += 1;
            }
        }
        prev = Some(code);
        if out.len() >= limit {
            out.truncate(limit);
            return Ok(out);
        }
    }
}

// --- writer -------------------------------------------------------------------

/// Streams frames into a looping GIF89a.
pub struct GifWriter<W: Write> {
    w: W,
    width: u16,
    height: u16,
    frames: u32,
}

impl<W: Write> GifWriter<W> {
    /// `loops`: 0 repeats forever.
    pub fn new(mut w: W, width: u32, height: u32, loops: u16) -> io::Result<Self> {
        if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "GIF size must be 1..65535"));
        }
        let (width, height) = (width as u16, height as u16);
        w.write_all(b"GIF89a")?;
        w.write_all(&width.to_le_bytes())?;
        w.write_all(&height.to_le_bytes())?;
        w.write_all(&[0x00, 0, 0])?; // no global palette
        w.write_all(&[0x21, 0xFF, 0x0B])?;
        w.write_all(b"NETSCAPE2.0")?;
        w.write_all(&[0x03, 0x01])?;
        w.write_all(&loops.to_le_bytes())?;
        w.write_all(&[0x00])?;
        Ok(Self { w, width, height, frames: 0 })
    }

    /// Adds a full frame shown for `delay_cs` hundredths of a second. The
    /// image is cropped or padded (black) to the GIF size.
    pub fn push(&mut self, img: &RgbaImage, delay_cs: u16, opts: &GifOptions) -> io::Result<()> {
        let fitted;
        let img = if img.width == u32::from(self.width) && img.height == u32::from(self.height) {
            img
        } else {
            let mut f = RgbaImage::new(u32::from(self.width), u32::from(self.height));
            for y in 0..f.height.min(img.height) as usize {
                let n = f.width.min(img.width) as usize * 4;
                let src = y * img.width as usize * 4;
                let dst = y * f.width as usize * 4;
                f.pixels[dst..dst + n].copy_from_slice(&img.pixels[src..src + n]);
            }
            fitted = f;
            &fitted
        };
        let palette = median_cut(img, opts.colors);
        let indices = index_pixels(img, &palette, opts.dither);
        let bits = (palette.len().max(2) as f64).log2().ceil() as u32;
        let bits = bits.clamp(1, 8);
        let w = &mut self.w;
        // Graphic control: disposal "leave in place", no transparency.
        w.write_all(&[0x21, 0xF9, 0x04, 0x04])?;
        w.write_all(&delay_cs.to_le_bytes())?;
        w.write_all(&[0x00, 0x00])?;
        // Image descriptor with a local palette.
        w.write_all(&[0x2C, 0, 0, 0, 0])?;
        w.write_all(&self.width.to_le_bytes())?;
        w.write_all(&self.height.to_le_bytes())?;
        w.write_all(&[0x80 | (bits - 1) as u8])?;
        let mut table = vec![0u8; 3 << bits];
        for (i, c) in palette.iter().enumerate() {
            table[i * 3..i * 3 + 3].copy_from_slice(c);
        }
        w.write_all(&table)?;
        let mcs = bits.max(2) as u8;
        w.write_all(&[mcs])?;
        let data = lzw_encode(&indices, mcs);
        for chunk in data.chunks(255) {
            w.write_all(&[chunk.len() as u8])?;
            w.write_all(chunk)?;
        }
        w.write_all(&[0x00])?;
        self.frames += 1;
        Ok(())
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Writes the trailer and returns the sink.
    pub fn finish(mut self) -> io::Result<W> {
        self.w.write_all(&[0x3B])?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// Per-frame delays (centiseconds) for `fps` that add up without drift.
pub fn frame_delay(i: u64, fps: f64) -> u16 {
    let at = |n: u64| (n as f64 * 100.0 / fps.max(0.1)).round() as u64;
    (at(i + 1) - at(i)).clamp(1, 65_535) as u16
}

// --- reader -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GifFrame {
    /// The full canvas after this frame was drawn.
    pub image: RgbaImage,
    pub delay_cs: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gif {
    pub width: u32,
    pub height: u32,
    /// Netscape loop count (0 = forever), if present.
    pub loops: Option<u16>,
    pub frames: Vec<GifFrame>,
}

fn sub_blocks(b: &[u8], pos: &mut usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let n = *b.get(*pos).ok_or("GIF ends inside a data block")? as usize;
        *pos += 1;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(b.get(*pos..*pos + n).ok_or("GIF ends inside a data block")?);
        *pos += n;
    }
}

pub fn decode(b: &[u8]) -> Result<Gif, String> {
    if b.len() < 13 || !(b.starts_with(b"GIF89a") || b.starts_with(b"GIF87a")) {
        return Err("not a GIF file".into());
    }
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let (width, height) = (u32::from(u16_at(6)), u32::from(u16_at(8)));
    let flags = b[10];
    let mut pos = 13;
    let mut global: Vec<[u8; 3]> = Vec::new();
    if flags & 0x80 != 0 {
        let n = 2usize << (flags & 7);
        let t = b.get(pos..pos + n * 3).ok_or("truncated palette")?;
        global = t.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        pos += n * 3;
    }
    let mut canvas = RgbaImage::new(width, height);
    let mut gif = Gif { width, height, loops: None, frames: Vec::new() };
    let (mut delay, mut transparent, mut disposal) = (0u16, None::<u8>, 0u8);
    loop {
        match b.get(pos).copied().ok_or("GIF has no trailer")? {
            0x3B => return Ok(gif),
            0x21 => {
                let label = *b.get(pos + 1).ok_or("truncated extension")?;
                pos += 2;
                let data = sub_blocks(b, &mut pos)?;
                match label {
                    0xF9 if data.len() >= 4 => {
                        disposal = (data[0] >> 2) & 7;
                        delay = u16::from_le_bytes([data[1], data[2]]);
                        transparent = (data[0] & 1 != 0).then_some(data[3]);
                    }
                    0xFF if data.starts_with(b"NETSCAPE2.0") && data.len() >= 14 => {
                        gif.loops = Some(u16::from_le_bytes([data[12], data[13]]));
                    }
                    _ => {}
                }
            }
            0x2C => {
                let d = b.get(pos + 1..pos + 10).ok_or("truncated image descriptor")?;
                let (x, y) = (u32::from(u16::from_le_bytes([d[0], d[1]])), u32::from(u16::from_le_bytes([d[2], d[3]])));
                let (w, h) = (u32::from(u16::from_le_bytes([d[4], d[5]])), u32::from(u16::from_le_bytes([d[6], d[7]])));
                let f = d[8];
                pos += 10;
                let local: Vec<[u8; 3]>;
                let palette = if f & 0x80 != 0 {
                    let n = 2usize << (f & 7);
                    let t = b.get(pos..pos + n * 3).ok_or("truncated palette")?;
                    pos += n * 3;
                    local = t.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                    &local
                } else {
                    &global
                };
                let mcs = *b.get(pos).ok_or("truncated image")?;
                pos += 1;
                let data = sub_blocks(b, &mut pos)?;
                let idx = lzw_decode(&data, mcs, (w * h) as usize)?;
                let before = (disposal == 3).then(|| canvas.clone());
                let rows: Vec<u32> = if f & 0x40 != 0 {
                    [(0, 8), (4, 8), (2, 4), (1, 2)].iter().flat_map(|&(s, st)| (s..h).step_by(st)).collect()
                } else {
                    (0..h).collect()
                };
                for (r, &row) in rows.iter().enumerate() {
                    for col in 0..w {
                        let Some(&i) = idx.get(r * w as usize + col as usize) else { break };
                        if Some(i) == transparent {
                            continue;
                        }
                        let (cx, cy) = (x + col, y + row);
                        if cx >= width || cy >= height {
                            continue;
                        }
                        let c = palette.get(i as usize).copied().unwrap_or([0, 0, 0]);
                        let o = ((cy * width + cx) * 4) as usize;
                        canvas.pixels[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                }
                gif.frames.push(GifFrame { image: canvas.clone(), delay_cs: delay });
                match disposal {
                    2 => {
                        for cy in y..(y + h).min(height) {
                            for cx in x..(x + w).min(width) {
                                let o = ((cy * width + cx) * 4) as usize;
                                canvas.pixels[o..o + 4].fill(0);
                            }
                        }
                    }
                    3 => canvas = before.unwrap_or(canvas),
                    _ => {}
                }
                (delay, transparent, disposal) = (0, None, 0);
            }
            other => return Err(format!("unexpected GIF block 0x{other:02x}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, phase: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                img.pixels[o..o + 4].copy_from_slice(&[((x + phase) * 255 / w) as u8, (y * 255 / h) as u8, ((x * y + phase * 7) % 256) as u8, 255]);
            }
        }
        img
    }

    #[test]
    fn lzw_round_trips() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let cases: Vec<(Vec<u8>, u8)> = vec![
            (vec![], 2),
            (vec![0], 2),
            (vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1], 2),
            ((0..100_000).map(|_| rnd() & 3).collect(), 2),
            ((0..200_000).map(|_| rnd()).collect(), 8),
            ((0..300_000).map(|i| (i / 977 % 256) as u8).collect(), 8),
            ((0..50_000).map(|_| rnd() & 15).collect(), 4),
        ];
        for (data, mcs) in cases {
            let enc = lzw_encode(&data, mcs);
            let dec = lzw_decode(&enc, mcs, data.len()).unwrap();
            assert_eq!(dec.len(), data.len());
            assert!(dec == data, "mcs {mcs} len {}", data.len());
        }
        // Long runs compress well.
        assert!(lzw_encode(&vec![7u8; 100_000], 8).len() < 2_000);
    }

    #[test]
    fn median_cut_finds_the_colours() {
        let mut img = RgbaImage::new(30, 10);
        let cols = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]];
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            let c = cols[(i % 30) / 10];
            p.copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
        let pal = median_cut(&img, 256);
        assert_eq!(pal.len(), 3);
        for c in cols {
            assert!(pal.contains(&c), "{pal:?}");
        }
        let idx = index_pixels(&img, &pal, true);
        for (i, &k) in idx.iter().enumerate() {
            assert_eq!(pal[k as usize], cols[(i % 30) / 10]);
        }
        // Palette size is capped.
        assert!(median_cut(&gradient(64, 64, 0), 16).len() <= 16);
    }

    #[test]
    fn gif_round_trips_with_small_error() {
        let (w, h) = (96, 54);
        let mut g = GifWriter::new(Vec::new(), w, h, 0).unwrap();
        let frames: Vec<RgbaImage> = (0..4).map(|i| gradient(w, h, i * 10)).collect();
        for (i, f) in frames.iter().enumerate() {
            g.push(f, frame_delay(i as u64, 15.0), &GifOptions { colors: 256, dither: i % 2 == 0 }).unwrap();
        }
        assert_eq!(g.frames(), 4);
        let bytes = g.finish().unwrap();
        let gif = decode(&bytes).unwrap();
        assert_eq!((gif.width, gif.height, gif.loops, gif.frames.len()), (w, h, Some(0), 4));
        let delays: u32 = gif.frames.iter().map(|f| u32::from(f.delay_cs)).sum();
        assert_eq!(delays, 27); // 4 frames at 15 fps = 26.67 cs
        for (src, got) in frames.iter().zip(&gif.frames) {
            let err: f64 = src.pixels.iter().zip(&got.image.pixels).map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs()).sum::<f64>() / src.pixels.len() as f64;
            assert!(err < 10.0, "mean error {err}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!(decode(b"PNG").is_err());
        assert!(GifWriter::new(Vec::new(), 0, 10, 0).is_err());
        assert!(lzw_decode(&[0xFF, 0xFF], 2, 10).is_err());
    }
}
