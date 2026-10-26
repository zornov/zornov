// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Baseline JPEG encoder (4:2:0, standard Huffman tables) for opaque
//! backgrounds, photos and overlays, where PNG would cost ten times the
//! bytes. `studio-media` decodes what this writes.

use studio_media::rgba::RgbaImage;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

const LUMA_Q: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92,
    95, 98, 112, 100, 103, 99,
];
const CHROMA_Q: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99,
];

// Standard Huffman tables (ITU-T T.81 Annex K): (bits counts, values).
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_LUMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_CHROMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53,
    0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6,
    0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4,
    0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// Code (value, length) per symbol built from a standard table.
fn build_codes(bits: &[u8; 16], vals: &[u8]) -> [(u16, u8); 256] {
    let mut codes = [(0u16, 0u8); 256];
    let mut code = 0u16;
    let mut k = 0;
    for (len, &n) in bits.iter().enumerate() {
        for _ in 0..n {
            codes[vals[k] as usize] = (code, len as u8 + 1);
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    codes
}

fn scaled_table(base: &[u8; 64], quality: u8) -> [u16; 64] {
    let q = quality.clamp(1, 100) as u32;
    let scale = if q < 50 { 5000 / q } else { 200 - q * 2 };
    let mut out = [0u16; 64];
    for i in 0..64 {
        out[i] = ((base[i] as u32 * scale + 50) / 100).clamp(1, 255) as u16;
    }
    out
}

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u16, len: u8) {
        self.acc = (self.acc << len) | (code as u32 & ((1u32 << len) - 1));
        self.n += len as u32;
        while self.n >= 8 {
            let byte = (self.acc >> (self.n - 8)) as u8;
            self.out.push(byte);
            if byte == 0xff {
                self.out.push(0); // byte stuffing
            }
            self.n -= 8;
        }
        self.acc &= (1u32 << self.n) - 1;
    }

    fn flush(&mut self) {
        if self.n > 0 {
            self.put(0x7f, (8 - self.n) as u8);
        }
    }
}

fn fdct(block: &[f32; 64]) -> [f32; 64] {
    let cos = cos_table();
    let mut out = [0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut sum = 0f32;
            for y in 0..8 {
                for x in 0..8 {
                    sum += block[y * 8 + x] * cos[x][u] * cos[y][v];
                }
            }
            let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            let cv = if v == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            out[v * 8 + u] = 0.25 * cu * cv * sum;
        }
    }
    out
}

/// cos((2x+1)uπ/16), computed once.
fn cos_table() -> &'static [[f32; 8]; 8] {
    static TABLE: std::sync::OnceLock<[[f32; 8]; 8]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [[0f32; 8]; 8];
        for (x, row) in t.iter_mut().enumerate() {
            for (u, v) in row.iter_mut().enumerate() {
                *v = (((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0).cos();
            }
        }
        t
    })
}

fn category(v: i32) -> (u8, u16) {
    let a = v.unsigned_abs();
    let bits = 32 - a.leading_zeros() as u8;
    let code = if v < 0 { (v - 1) as u32 } else { v as u32 };
    (bits, (code & ((1u32 << bits) - 1)) as u16)
}

#[allow(clippy::too_many_arguments)]
fn encode_block(bits: &mut Bits, block: &[f32; 64], q: &[u16; 64], prev_dc: &mut i32, dc: &[(u16, u8); 256], ac: &[(u16, u8); 256]) {
    let coeffs = fdct(block);
    let mut zz = [0i32; 64];
    for (i, &zi) in ZIGZAG.iter().enumerate() {
        zz[i] = (coeffs[zi] / q[i] as f32).round() as i32;
    }
    let diff = zz[0] - *prev_dc;
    *prev_dc = zz[0];
    let (cat, code) = category(diff);
    bits.put(dc[cat as usize].0, dc[cat as usize].1);
    if cat > 0 {
        bits.put(code, cat);
    }
    let mut run = 0u8;
    for &v in &zz[1..] {
        if v == 0 {
            run += 1;
            continue;
        }
        while run >= 16 {
            bits.put(ac[0xf0].0, ac[0xf0].1);
            run -= 16;
        }
        let (cat, code) = category(v);
        let sym = (run << 4) | cat;
        bits.put(ac[sym as usize].0, ac[sym as usize].1);
        bits.put(code, cat);
        run = 0;
    }
    if run > 0 {
        bits.put(ac[0].0, ac[0].1);
    }
}

fn marker(out: &mut Vec<u8>, m: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xff, m]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Encodes `img` as a baseline JPEG (4:2:0) at `quality` 1..100. Alpha is
/// ignored: callers use this for opaque pictures only.
pub fn encode(img: &RgbaImage, quality: u8) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let lq = scaled_table(&LUMA_Q, quality);
    let cq = scaled_table(&CHROMA_Q, quality);
    let mut out = vec![0xff, 0xd8];
    // JFIF APP0.
    marker(&mut out, 0xe0, &[b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
    // Quantisation tables in zigzag order.
    let mut dqt = Vec::with_capacity(130);
    for (id, t) in [(0u8, &lq), (1u8, &cq)] {
        dqt.push(id);
        for i in 0..64 {
            dqt.push(t[i] as u8);
        }
    }
    marker(&mut out, 0xdb, &dqt);
    // Frame header: 8-bit, 3 components, Y 2x2, Cb/Cr 1x1.
    let mut sof = vec![8];
    sof.extend_from_slice(&(h as u16).to_be_bytes());
    sof.extend_from_slice(&(w as u16).to_be_bytes());
    sof.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    marker(&mut out, 0xc0, &sof);
    // Huffman tables.
    let mut dht = Vec::new();
    for (class_id, bits, vals) in [(0x00u8, &DC_LUMA_BITS, &DC_LUMA_VALS[..]), (0x10, &AC_LUMA_BITS, &AC_LUMA_VALS[..]), (0x01, &DC_CHROMA_BITS, &DC_CHROMA_VALS[..]), (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS[..])] {
        dht.push(class_id);
        dht.extend_from_slice(bits);
        dht.extend_from_slice(vals);
    }
    marker(&mut out, 0xc4, &dht);
    marker(&mut out, 0xda, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    let dc_l = build_codes(&DC_LUMA_BITS, &DC_LUMA_VALS);
    let ac_l = build_codes(&AC_LUMA_BITS, &AC_LUMA_VALS);
    let dc_c = build_codes(&DC_CHROMA_BITS, &DC_CHROMA_VALS);
    let ac_c = build_codes(&AC_CHROMA_BITS, &AC_CHROMA_VALS);
    // Planes (BT.601 full range), edge-replicated to the MCU grid.
    let mcu_w = w.div_ceil(16) * 16;
    let mcu_h = h.div_ceil(16) * 16;
    let mut yp = vec![0f32; mcu_w * mcu_h];
    let mut cb = vec![0f32; mcu_w * mcu_h];
    let mut cr = vec![0f32; mcu_w * mcu_h];
    for y in 0..mcu_h {
        let sy = y.min(h - 1);
        for x in 0..mcu_w {
            let sx = x.min(w - 1);
            let p = &img.pixels[(sy * w + sx) * 4..(sy * w + sx) * 4 + 3];
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            yp[y * mcu_w + x] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
            cb[y * mcu_w + x] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
            cr[y * mcu_w + x] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
        }
    }
    let mut bits = Bits { out: Vec::with_capacity(w * h / 4), acc: 0, n: 0 };
    let (mut pdy, mut pdb, mut pdr) = (0i32, 0i32, 0i32);
    let mut block = [0f32; 64];
    for my in (0..mcu_h).step_by(16) {
        for mx in (0..mcu_w).step_by(16) {
            for (by, bx) in [(0, 0), (0, 8), (8, 0), (8, 8)] {
                for y in 0..8 {
                    for x in 0..8 {
                        block[y * 8 + x] = yp[(my + by + y) * mcu_w + mx + bx + x];
                    }
                }
                encode_block(&mut bits, &block, &lq, &mut pdy, &dc_l, &ac_l);
            }
            for (plane, prev) in [(&cb, &mut pdb), (&cr, &mut pdr)] {
                for y in 0..8 {
                    for x in 0..8 {
                        let i = (my + y * 2) * mcu_w + mx + x * 2;
                        block[y * 8 + x] = 0.25 * (plane[i] + plane[i + 1] + plane[i + mcu_w] + plane[i + mcu_w + 1]);
                    }
                }
                encode_block(&mut bits, &block, &cq, prev, &dc_c, &ac_c);
            }
        }
    }
    bits.flush();
    out.extend_from_slice(&bits.out);
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

/// Writes `img` as a JPEG file (atomically via a `.part` file).
pub fn write(img: &RgbaImage, quality: u8, path: &std::path::Path) -> std::io::Result<()> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, encode(img, quality))?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_follow_the_standard() {
        assert_eq!(category(0), (0, 0));
        assert_eq!(category(1), (1, 1));
        assert_eq!(category(-1), (1, 0));
        assert_eq!(category(-3), (2, 0));
        assert_eq!(category(1023), (10, 1023));
    }

    #[test]
    fn round_trips_through_media_decoder_with_small_error() {
        let (w, h) = (77u32, 45u32);
        let mut img = RgbaImage::new(w, h);
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i % w as usize) as f32, (i / w as usize) as f32);
            p.copy_from_slice(&[(x * 3.0) as u8, (y * 5.0) as u8, ((x + y) * 1.5) as u8, 255]);
        }
        let bytes = encode(&img, 90);
        let back = studio_media::jpeg::decode(&bytes).unwrap();
        assert_eq!((back.width, back.height), (w, h));
        let mut err = 0u64;
        for (a, b) in img.pixels.chunks_exact(4).zip(back.pixels.chunks_exact(4)) {
            for c in 0..3 {
                err += (a[c] as i64 - b[c] as i64).unsigned_abs();
            }
        }
        let mean = err as f64 / (w as f64 * h as f64 * 3.0);
        assert!(mean < 3.0, "mean abs error {mean}");
        assert!(bytes.len() < (w * h) as usize, "{} bytes", bytes.len());
        // Lower quality is smaller.
        assert!(encode(&img, 50).len() < bytes.len());
    }
}
