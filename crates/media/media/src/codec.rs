// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CLC, Studio's own video codec.
//!
//! * Colour: full-range BT.601 YCbCr, chroma subsampled 4:2:0.
//! * Macroblocks of 16×16 luma (four 8×8 blocks) plus one 8×8 block per chroma plane.
//! * Each block: 8×8 DCT-II, JPEG-style quantisation scaled by quality, zig-zag
//!   order, DC coded as a signed Exp-Golomb difference from the previous block of
//!   the same plane, AC coded as (count, then run/level pairs) in Exp-Golomb.
//! * Key frames code every macroblock. Delta frames send one bit per macroblock:
//!   skip (copy from the previous decoded frame) or a fresh intra macroblock.
//! * Every macroblock row is an independent, byte-aligned slice, so rows encode
//!   and decode in parallel.
//!
//! Frame layout: `"CLC"`, kind (`K`|`D`), width u16, height u16, quality u8,
//! row count u16, row sizes u32 × rows, then the row payloads. All little-endian.

use crate::bits::{BitReader, BitWriter};
use crate::rgba::RgbaImage;
use std::sync::OnceLock;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42,
    49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const LUMA_Q: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112,
    100, 103, 99,
];

const CHROMA_Q: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    Truncated,
    BadHeader,
    SizeMismatch,
    NeedsReference,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Truncated => "frame data is truncated",
            Self::BadHeader => "not a CLC frame",
            Self::SizeMismatch => "frame size differs from the reference frame",
            Self::NeedsReference => "delta frame decoded without a reference frame",
        })
    }
}

impl std::error::Error for CodecError {}

fn quant_table(base: &[u16; 64], quality: u8) -> [f32; 64] {
    let q = quality.clamp(1, 100) as u32;
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut t = [0f32; 64];
    for i in 0..64 {
        t[i] = ((u32::from(base[i]) * scale + 50) / 100).clamp(1, 255) as f32;
    }
    t
}

fn cos_table() -> &'static [[f32; 8]; 8] {
    static T: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [[0f32; 8]; 8];
        for (u, row) in t.iter_mut().enumerate() {
            let cu = if u == 0 { (0.5f32).sqrt() } else { 1.0 };
            for (x, v) in row.iter_mut().enumerate() {
                *v = 0.5 * cu * (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos();
            }
        }
        t
    })
}

fn fdct(block: &mut [f32; 64]) {
    let c = cos_table();
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for x in 0..8 {
                s += c[u][x] * block[y * 8 + x];
            }
            tmp[y * 8 + u] = s;
        }
    }
    for u in 0..8 {
        for v in 0..8 {
            let mut s = 0.0;
            for y in 0..8 {
                s += c[v][y] * tmp[y * 8 + u];
            }
            block[v * 8 + u] = s;
        }
    }
}

fn idct(block: &mut [f32; 64]) {
    let c = cos_table();
    let mut tmp = [0f32; 64];
    for v in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for u in 0..8 {
                s += c[u][x] * block[v * 8 + u];
            }
            tmp[v * 8 + x] = s;
        }
    }
    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                s += c[v][y] * tmp[v * 8 + x];
            }
            block[y * 8 + x] = s;
        }
    }
}

/// A frame in planar YCbCr with dimensions padded to whole macroblocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    pub width: u32,
    pub height: u32,
    pub mb_w: usize,
    pub mb_h: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

impl Planes {
    fn blank(width: u32, height: u32) -> Self {
        let mb_w = (width as usize).div_ceil(16);
        let mb_h = (height as usize).div_ceil(16);
        Self { width, height, mb_w, mb_h, y: vec![0; mb_w * 16 * mb_h * 16], cb: vec![128; mb_w * 8 * mb_h * 8], cr: vec![128; mb_w * 8 * mb_h * 8] }
    }
    fn luma_stride(&self) -> usize {
        self.mb_w * 16
    }
    fn chroma_stride(&self) -> usize {
        self.mb_w * 8
    }

    pub fn from_rgba(img: &RgbaImage) -> Self {
        let mut p = Self::blank(img.width, img.height);
        let (w, h) = (img.width as usize, img.height as usize);
        let ls = p.luma_stride();
        let px = |x: usize, y: usize| {
            let i = (y.min(h - 1) * w + x.min(w - 1)) * 4;
            (img.pixels[i] as f32, img.pixels[i + 1] as f32, img.pixels[i + 2] as f32)
        };
        for y in 0..p.mb_h * 16 {
            for x in 0..ls {
                let (r, g, b) = px(x, y);
                p.y[y * ls + x] = (0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8;
            }
        }
        let cs = p.chroma_stride();
        for cy in 0..p.mb_h * 8 {
            for cx in 0..cs {
                let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (pr, pg, pb) = px(cx * 2 + dx, cy * 2 + dy);
                    r += pr;
                    g += pg;
                    b += pb;
                }
                let (r, g, b) = (r / 4.0, g / 4.0, b / 4.0);
                p.cb[cy * cs + cx] = (128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b).round().clamp(0.0, 255.0) as u8;
                p.cr[cy * cs + cx] = (128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b).round().clamp(0.0, 255.0) as u8;
            }
        }
        p
    }

    pub fn to_rgba(&self) -> RgbaImage {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = RgbaImage::new(self.width, self.height);
        let (ls, cs) = (self.luma_stride(), self.chroma_stride());
        let ch = self.mb_h * 8;
        for y in 0..h {
            // Bilinear chroma upsampling with centred sample positions.
            let fy = ((y as f32 + 0.5) / 2.0 - 0.5).max(0.0);
            let y0 = fy as usize;
            let y1 = (y0 + 1).min(ch - 1);
            let wy = fy - y0 as f32;
            for x in 0..w {
                let fx = ((x as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let x0 = fx as usize;
                let x1 = (x0 + 1).min(cs - 1);
                let wx = fx - x0 as f32;
                let sample = |plane: &[u8]| {
                    let a = plane[y0 * cs + x0] as f32 * (1.0 - wx) + plane[y0 * cs + x1] as f32 * wx;
                    let b = plane[y1 * cs + x0] as f32 * (1.0 - wx) + plane[y1 * cs + x1] as f32 * wx;
                    a * (1.0 - wy) + b * wy - 128.0
                };
                let (cb, cr) = (sample(&self.cb), sample(&self.cr));
                let l = self.y[y * ls + x] as f32;
                let i = (y * w + x) * 4;
                out.pixels[i] = (l + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
                out.pixels[i + 1] = (l - 0.344_136 * cb - 0.714_136 * cr).round().clamp(0.0, 255.0) as u8;
                out.pixels[i + 2] = (l + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
                out.pixels[i + 3] = 255;
            }
        }
        out
    }
}

/// Where each of the six blocks of a macroblock lives: (plane 0=Y 1=Cb 2=Cr, x, y).
fn mb_blocks(mbx: usize, mby: usize) -> [(usize, usize, usize); 6] {
    let (lx, ly) = (mbx * 16, mby * 16);
    let (cx, cy) = (mbx * 8, mby * 8);
    [(0, lx, ly), (0, lx + 8, ly), (0, lx, ly + 8), (0, lx + 8, ly + 8), (1, cx, cy), (2, cx, cy)]
}

fn mb_sad(a: &Planes, b: &Planes, mbx: usize, mby: usize) -> f32 {
    let ls = a.luma_stride();
    let mut sad = 0u32;
    for y in mby * 16..mby * 16 + 16 {
        for x in mbx * 16..mbx * 16 + 16 {
            sad += (a.y[y * ls + x] as i32 - b.y[y * ls + x] as i32).unsigned_abs();
        }
    }
    let cs = a.chroma_stride();
    let mut csad = 0u32;
    for y in mby * 8..mby * 8 + 8 {
        for x in mbx * 8..mbx * 8 + 8 {
            csad += (a.cb[y * cs + x] as i32 - b.cb[y * cs + x] as i32).unsigned_abs();
            csad += (a.cr[y * cs + x] as i32 - b.cr[y * cs + x] as i32).unsigned_abs();
        }
    }
    (sad as f32 / 256.0).max(csad as f32 / 128.0)
}

struct RowPlanes {
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
}

struct RowCoder<'a> {
    quant: [&'a [f32; 64]; 3],
    dc_pred: [i32; 3],
}

impl RowCoder<'_> {
    /// Codes one block and returns its reconstruction.
    fn encode_block(&mut self, w: &mut BitWriter, plane: usize, src: &[u8], stride: usize, x: usize, y: usize) -> [u8; 64] {
        let mut b = [0f32; 64];
        for r in 0..8 {
            for c in 0..8 {
                b[r * 8 + c] = src[(y + r) * stride + x + c] as f32 - 128.0;
            }
        }
        fdct(&mut b);
        let q = self.quant[plane];
        let mut levels = [0i32; 64];
        for (i, &z) in ZIGZAG.iter().enumerate() {
            levels[i] = (b[z] / q[z]).round() as i32;
        }
        w.se(levels[0] - self.dc_pred[plane]);
        self.dc_pred[plane] = levels[0];
        let nonzero = levels[1..].iter().filter(|&&l| l != 0).count();
        w.ue(nonzero as u32);
        let mut run = 0;
        for &l in &levels[1..] {
            if l == 0 {
                run += 1;
            } else {
                w.ue(run);
                w.se(l);
                run = 0;
            }
        }
        reconstruct(&levels, q)
    }

    fn decode_block(&mut self, r: &mut BitReader, plane: usize) -> Result<[u8; 64], CodecError> {
        let mut levels = [0i32; 64];
        levels[0] = self.dc_pred[plane] + r.se().map_err(|_| CodecError::Truncated)?;
        self.dc_pred[plane] = levels[0];
        let nonzero = r.ue().map_err(|_| CodecError::Truncated)?;
        let mut pos = 1usize;
        for _ in 0..nonzero {
            pos += r.ue().map_err(|_| CodecError::Truncated)? as usize;
            if pos > 63 {
                return Err(CodecError::Truncated);
            }
            levels[pos] = r.se().map_err(|_| CodecError::Truncated)?;
            pos += 1;
        }
        Ok(reconstruct(&levels, self.quant[plane]))
    }
}

fn reconstruct(levels: &[i32; 64], q: &[f32; 64]) -> [u8; 64] {
    let mut b = [0f32; 64];
    for (i, &z) in ZIGZAG.iter().enumerate() {
        b[z] = levels[i] as f32 * q[z];
    }
    idct(&mut b);
    let mut out = [0u8; 64];
    for i in 0..64 {
        out[i] = (b[i] + 128.0).round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn put_block(dst: &mut [u8], stride: usize, x: usize, y: usize, block: &[u8; 64]) {
    for r in 0..8 {
        dst[(y + r) * stride + x..(y + r) * stride + x + 8].copy_from_slice(&block[r * 8..r * 8 + 8]);
    }
}

/// Splits row-local work over the available cores.
fn par_rows<T: Send>(rows: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(rows.max(1));
    let mut out: Vec<Option<T>> = (0..rows).map(|_| None).collect();
    std::thread::scope(|s| {
        let chunks: Vec<_> = out.chunks_mut(rows.div_ceil(threads).max(1)).enumerate().collect();
        let size = rows.div_ceil(threads).max(1);
        for (ci, chunk) in chunks {
            let f = &f;
            s.spawn(move || {
                for (i, slot) in chunk.iter_mut().enumerate() {
                    *slot = Some(f(ci * size + i));
                }
            });
        }
    });
    out.into_iter().map(Option::unwrap).collect()
}

pub struct Encoder {
    pub quality: u8,
    /// Mean absolute difference per sample under which a macroblock is skipped.
    pub skip_threshold: f32,
    reference: Option<Planes>,
}

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub key: bool,
}

impl Encoder {
    pub fn new(quality: u8) -> Self {
        Self { quality, skip_threshold: 1.5, reference: None }
    }

    pub fn encode(&mut self, img: &RgbaImage, force_key: bool) -> EncodedFrame {
        let src = Planes::from_rgba(img);
        let reference = self.reference.take().filter(|r| !force_key && r.width == src.width && r.height == src.height);
        let key = reference.is_none();
        let (lq, cq) = (quant_table(&LUMA_Q, self.quality), quant_table(&CHROMA_Q, self.quality));
        let threshold = self.skip_threshold;

        // Each row returns (bytes, reconstructed rows of the three planes).
        let rows = par_rows(src.mb_h, |mby| {
            let mut w = BitWriter::new();
            let mut coder = RowCoder { quant: [&lq, &cq, &cq], dc_pred: [0; 3] };
            // Reconstruction of this macroblock row only.
            let mut recon = RowPlanes {
                y: vec![0; src.luma_stride() * 16],
                cb: vec![128; src.chroma_stride() * 8],
                cr: vec![128; src.chroma_stride() * 8],
            };
            for mbx in 0..src.mb_w {
                if let Some(r) = &reference {
                    let skip = mb_sad(&src, r, mbx, mby) < threshold;
                    w.bit(skip);
                    if skip {
                        // copy reference into the row-local reconstruction
                        let (ls, cs) = (src.luma_stride(), src.chroma_stride());
                        for y in 0..16 {
                            let s = (mby * 16 + y) * ls + mbx * 16;
                            recon.y[y * ls + mbx * 16..y * ls + mbx * 16 + 16].copy_from_slice(&r.y[s..s + 16]);
                        }
                        for y in 0..8 {
                            let s = (mby * 8 + y) * cs + mbx * 8;
                            recon.cb[y * cs + mbx * 8..y * cs + mbx * 8 + 8].copy_from_slice(&r.cb[s..s + 8]);
                            recon.cr[y * cs + mbx * 8..y * cs + mbx * 8 + 8].copy_from_slice(&r.cr[s..s + 8]);
                        }
                        continue;
                    }
                }
                for (plane, x, y) in mb_blocks(mbx, mby) {
                    let (data, stride, local_y) = match plane {
                        0 => (&src.y, src.luma_stride(), y - mby * 16),
                        1 => (&src.cb, src.chroma_stride(), y - mby * 8),
                        _ => (&src.cr, src.chroma_stride(), y - mby * 8),
                    };
                    let block = coder.encode_block(&mut w, plane, data, stride, x, y);
                    let dst = match plane {
                        0 => &mut recon.y,
                        1 => &mut recon.cb,
                        _ => &mut recon.cr,
                    };
                    put_block(dst, stride, x, local_y, &block);
                }
            }
            (w.finish(), recon)
        });

        let mut full = Planes::blank(src.width, src.height);
        let (ls, cs) = (full.luma_stride(), full.chroma_stride());
        let mut data = Vec::new();
        data.extend_from_slice(b"CLC");
        data.push(if key { b'K' } else { b'D' });
        data.extend_from_slice(&(src.width as u16).to_le_bytes());
        data.extend_from_slice(&(src.height as u16).to_le_bytes());
        data.push(self.quality);
        data.extend_from_slice(&(src.mb_h as u16).to_le_bytes());
        for (bytes, _) in &rows {
            data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        }
        for (mby, (bytes, recon)) in rows.into_iter().enumerate() {
            data.extend_from_slice(&bytes);
            full.y[mby * 16 * ls..(mby + 1) * 16 * ls].copy_from_slice(&recon.y);
            full.cb[mby * 8 * cs..(mby + 1) * 8 * cs].copy_from_slice(&recon.cb);
            full.cr[mby * 8 * cs..(mby + 1) * 8 * cs].copy_from_slice(&recon.cr);
        }
        self.reference = Some(full);
        EncodedFrame { data, key }
    }
}

#[derive(Default)]
pub struct Decoder {
    reference: Option<Planes>,
}

pub fn is_key(frame: &[u8]) -> bool {
    frame.len() > 3 && frame[3] == b'K'
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.reference = None;
    }

    pub fn decode_planes(&mut self, data: &[u8]) -> Result<&Planes, CodecError> {
        if data.len() < 12 || &data[..3] != b"CLC" {
            return Err(CodecError::BadHeader);
        }
        let key = match data[3] {
            b'K' => true,
            b'D' => false,
            _ => return Err(CodecError::BadHeader),
        };
        let width = u32::from(u16::from_le_bytes([data[4], data[5]]));
        let height = u32::from(u16::from_le_bytes([data[6], data[7]]));
        let quality = data[8];
        let rows = usize::from(u16::from_le_bytes([data[9], data[10]]));
        let mut full = Planes::blank(width, height);
        if rows != full.mb_h || width == 0 || height == 0 {
            return Err(CodecError::BadHeader);
        }
        let reference = if key {
            None
        } else {
            let r = self.reference.as_ref().ok_or(CodecError::NeedsReference)?;
            if r.width != width || r.height != height {
                return Err(CodecError::SizeMismatch);
            }
            Some(r)
        };
        let table_end = 11 + rows * 4;
        if data.len() < table_end {
            return Err(CodecError::Truncated);
        }
        let mut slices = Vec::with_capacity(rows);
        let mut at = table_end;
        for i in 0..rows {
            let n = u32::from_le_bytes(data[11 + i * 4..15 + i * 4].try_into().unwrap()) as usize;
            let slice = data.get(at..at + n).ok_or(CodecError::Truncated)?;
            slices.push(slice);
            at += n;
        }
        let (lq, cq) = (quant_table(&LUMA_Q, quality), quant_table(&CHROMA_Q, quality));
        let (ls, cs, mb_w) = (full.luma_stride(), full.chroma_stride(), full.mb_w);

        let decoded = par_rows(rows, |mby| -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), CodecError> {
            let mut r = BitReader::new(slices[mby]);
            let mut coder = RowCoder { quant: [&lq, &cq, &cq], dc_pred: [0; 3] };
            let mut y_row = vec![0u8; ls * 16];
            let mut cb_row = vec![128u8; cs * 8];
            let mut cr_row = vec![128u8; cs * 8];
            for mbx in 0..mb_w {
                if let Some(rf) = reference {
                    if r.bit().map_err(|_| CodecError::Truncated)? {
                        for y in 0..16 {
                            let s = (mby * 16 + y) * ls + mbx * 16;
                            y_row[y * ls + mbx * 16..y * ls + mbx * 16 + 16].copy_from_slice(&rf.y[s..s + 16]);
                        }
                        for y in 0..8 {
                            let s = (mby * 8 + y) * cs + mbx * 8;
                            cb_row[y * cs + mbx * 8..y * cs + mbx * 8 + 8].copy_from_slice(&rf.cb[s..s + 8]);
                            cr_row[y * cs + mbx * 8..y * cs + mbx * 8 + 8].copy_from_slice(&rf.cr[s..s + 8]);
                        }
                        continue;
                    }
                }
                for (plane, x, y) in mb_blocks(mbx, mby) {
                    let block = coder.decode_block(&mut r, plane)?;
                    match plane {
                        0 => put_block(&mut y_row, ls, x, y - mby * 16, &block),
                        1 => put_block(&mut cb_row, cs, x, y - mby * 8, &block),
                        _ => put_block(&mut cr_row, cs, x, y - mby * 8, &block),
                    }
                }
            }
            Ok((y_row, cb_row, cr_row))
        });
        for (mby, row) in decoded.into_iter().enumerate() {
            let (y_row, cb_row, cr_row) = row?;
            full.y[mby * 16 * ls..(mby + 1) * 16 * ls].copy_from_slice(&y_row);
            full.cb[mby * 8 * cs..(mby + 1) * 8 * cs].copy_from_slice(&cb_row);
            full.cr[mby * 8 * cs..(mby + 1) * 8 * cs].copy_from_slice(&cr_row);
        }
        self.reference = Some(full);
        Ok(self.reference.as_ref().unwrap())
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<RgbaImage, CodecError> {
        Ok(self.decode_planes(data)?.to_rgba())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, phase: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                img.pixels[i] = ((x + phase) * 255 / w.max(1)) as u8;
                img.pixels[i + 1] = (y * 255 / h.max(1)) as u8;
                img.pixels[i + 2] = 128;
                img.pixels[i + 3] = 255;
            }
        }
        img
    }

    fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
        let mut se = 0f64;
        let mut n = 0f64;
        for (pa, pb) in a.pixels.chunks(4).zip(b.pixels.chunks(4)) {
            for c in 0..3 {
                let d = pa[c] as f64 - pb[c] as f64;
                se += d * d;
                n += 1.0;
            }
        }
        10.0 * (255.0f64 * 255.0 / (se / n).max(1e-9)).log10()
    }

    #[test]
    fn dct_round_trip_is_identity() {
        let mut b = [0f32; 64];
        for (i, v) in b.iter_mut().enumerate() {
            *v = (i as f32 * 7.0) % 50.0 - 25.0;
        }
        let orig = b;
        fdct(&mut b);
        idct(&mut b);
        for i in 0..64 {
            assert!((b[i] - orig[i]).abs() < 1e-3);
        }
    }

    #[test]
    fn key_frame_quality_and_odd_sizes() {
        let img = gradient(37, 23, 0);
        let mut enc = Encoder::new(90);
        let f = enc.encode(&img, true);
        assert!(f.key);
        let out = Decoder::new().decode(&f.data).unwrap();
        assert_eq!((out.width, out.height), (37, 23));
        assert!(psnr(&img, &out) > 32.0, "psnr {}", psnr(&img, &out));
    }

    #[test]
    fn delta_frames_skip_static_blocks_and_match_encoder_reference() {
        let a = gradient(64, 48, 0);
        let mut b = a.clone();
        for y in 0..16 {
            for x in 0..16 {
                let i = ((y * 64 + x) * 4) as usize;
                b.pixels[i..i + 3].copy_from_slice(&[255, 0, 0]);
            }
        }
        let mut enc = Encoder::new(85);
        let fa = enc.encode(&a, true);
        let fb = enc.encode(&b, false);
        assert!(!fb.key);
        assert!(fb.data.len() < fa.data.len() / 2, "delta {} key {}", fb.data.len(), fa.data.len());
        let mut dec = Decoder::new();
        dec.decode(&fa.data).unwrap();
        let out = dec.decode(&fb.data).unwrap();
        assert!(psnr(&b, &out) > 30.0);
        assert!(out.pixels[0] > 200 && out.pixels[1] < 60);
    }

    #[test]
    fn errors_are_reported() {
        let mut enc = Encoder::new(80);
        let img = gradient(32, 32, 0);
        enc.encode(&img, true);
        let delta = enc.encode(&img, false);
        assert_eq!(Decoder::new().decode(&delta.data).unwrap_err(), CodecError::NeedsReference);
        assert_eq!(Decoder::new().decode(b"nope").unwrap_err(), CodecError::BadHeader);
        let key = Encoder::new(80).encode(&img, true);
        assert!(Decoder::new().decode(&key.data[..key.data.len() - 5]).is_err());
    }
}
