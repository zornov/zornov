// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Facade: RGBA to planar 4:2:0 `u16` (BT.709), 8-bit and 16-bit input.
//!
//! The input is gamma-encoded R'G'B' (what the editor's frames hold); alpha is ignored. Luma is
//! `Y' = 0.2126 R' + 0.7152 G' + 0.0722 B'`, chroma the usual `(B' - Y') / 1.8556` and
//! `(R' - Y') / 1.5748`, scaled to the limited range (`16..=235` luma, `16..=240` chroma, shifted
//! left for 10 bits) or to the full range. Chroma is the box average of each 2x2 block, with the
//! last row and column repeated for odd sizes. All arithmetic is integer, with 24 fractional bits.

use crate::types::Frame;

const KR: f64 = 0.2126;
const KB: f64 = 0.0722;
const KG: f64 = 1.0 - KR - KB;

/// Fractional bits of the coefficients.
const S: u32 = 24;

/// Per-conversion constants: `plane value = offset + (cr * r + cg * g + cb * b)` in `S`-bit fixed
/// point, `r`, `g` and `b` being the input codes.
struct Matrix {
    /// `[y, cb, cr]` x `[r, g, b]`.
    coef: [[i64; 3]; 3],
    /// The offsets, with the rounding of the final shift not included.
    offset: [i64; 3],
    max: i64,
}

impl Matrix {
    fn new(bit_depth: u8, full_range: bool, in_max: f64) -> Matrix {
        assert!(matches!(bit_depth, 8 | 10), "bit depth must be 8 or 10");
        let n = bit_depth as u32 - 8;
        let max = (1i64 << bit_depth) - 1;
        let (y_scale, c_scale, y_off) = if full_range { (max as f64, max as f64, 0.0) } else { (219.0 * (1 << n) as f64, 224.0 * (1 << n) as f64, 16.0 * (1 << n) as f64) };
        let c_off = 128.0 * (1 << n) as f64;
        let fix = |v: f64| (v * (1u64 << S) as f64).round() as i64;
        let (cb_r, cb_g, cb_b) = (-KR / (2.0 * (1.0 - KB)), -KG / (2.0 * (1.0 - KB)), 0.5);
        let (cr_r, cr_g, cr_b) = (0.5, -KG / (2.0 * (1.0 - KR)), -KB / (2.0 * (1.0 - KR)));
        let k = |scale: f64| scale / in_max;
        Matrix {
            coef: [
                [fix(KR * k(y_scale)), fix(KG * k(y_scale)), fix(KB * k(y_scale))],
                [fix(cb_r * k(c_scale)), fix(cb_g * k(c_scale)), fix(cb_b * k(c_scale))],
                [fix(cr_r * k(c_scale)), fix(cr_g * k(c_scale)), fix(cr_b * k(c_scale))],
            ],
            offset: [fix(y_off), fix(c_off), fix(c_off)],
            max,
        }
    }

    /// The unrounded fixed-point value of plane `p` for one pixel.
    #[inline]
    fn plane(&self, p: usize, r: i64, g: i64, b: i64) -> i64 {
        let c = &self.coef[p];
        self.offset[p] + c[0] * r + c[1] * g + c[2] * b
    }
}

fn convert(width: u32, height: u32, bit_depth: u8, full_range: bool, in_max: f64, px: impl Fn(usize, usize) -> [i64; 3]) -> Frame {
    assert!(width > 0 && height > 0, "empty frame");
    let m = Matrix::new(bit_depth, full_range, in_max);
    let mut f = Frame::new(width, height, bit_depth);
    let (w, h) = (width as usize, height as usize);
    let round = 1i64 << (S - 1);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = px(x, y);
            f.planes[0][y * f.stride[0] + x] = ((m.plane(0, r, g, b) + round) >> S).clamp(0, m.max) as u16;
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    for cy in 0..ch {
        for cx in 0..cw {
            let mut sum = [0i64; 2];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let [r, g, b] = px((2 * cx + dx).min(w - 1), (2 * cy + dy).min(h - 1));
                sum[0] += m.plane(1, r, g, b);
                sum[1] += m.plane(2, r, g, b);
            }
            for p in 0..2 {
                f.planes[1 + p][cy * f.stride[1 + p] + cx] = ((sum[p] + (round << 2)) >> (S + 2)).clamp(0, m.max) as u16;
            }
        }
    }
    f
}

/// `rgba` is `width * height * 4` bytes. `bit_depth` of the result is 8 or 10.
pub fn from_rgba8(rgba: &[u8], width: u32, height: u32, bit_depth: u8, full_range: bool) -> Frame {
    let w = width as usize;
    assert_eq!(rgba.len(), w * height as usize * 4, "rgba must be width * height * 4 bytes");
    convert(width, height, bit_depth, full_range, 255.0, |x, y| {
        let i = (y * w + x) * 4;
        [rgba[i] as i64, rgba[i + 1] as i64, rgba[i + 2] as i64]
    })
}

/// `rgba` is `width * height * 4` 16-bit samples.
pub fn from_rgba16(rgba: &[u16], width: u32, height: u32, bit_depth: u8, full_range: bool) -> Frame {
    let w = width as usize;
    assert_eq!(rgba.len(), w * height as usize * 4, "rgba must be width * height * 4 samples");
    convert(width, height, bit_depth, full_range, 65535.0, |x, y| {
        let i = (y * w + x) * 4;
        [rgba[i] as i64, rgba[i + 1] as i64, rgba[i + 2] as i64]
    })
}
