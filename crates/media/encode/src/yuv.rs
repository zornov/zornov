// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RGBA to Y'CbCr 4:2:0 conversion, ITU-R BT.709 matrix, limited ("video") range.

/// An 8-bit 4:2:0 picture. Luma is `width` x `height`; each chroma plane is
/// `ceil(width / 2)` x `ceil(height / 2)`; all planes are tightly packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yuv420 {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

// 16.16 fixed-point BT.709 coefficients scaled to limited range
// (Y: 16..235 = 219 steps, Cb/Cr: 16..240 = 224 steps).
const KRY: i32 = 11966;
const KGY: i32 = 40254;
const KBY: i32 = 4064;
const KRU: i32 = -6596;
const KGU: i32 = -22188;
const KBU: i32 = 28784;
const KRV: i32 = 28784;
const KGV: i32 = -26145;
const KBV: i32 = -2639;

impl Yuv420 {
    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }

    pub fn chroma_height(&self) -> u32 {
        self.height.div_ceil(2)
    }

    /// A flat picture.
    pub fn filled(width: u32, height: u32, y: u8, u: u8, v: u8) -> Self {
        let (cw, ch) = (width.div_ceil(2) as usize, height.div_ceil(2) as usize);
        Yuv420 {
            width,
            height,
            y: vec![y; width as usize * height as usize],
            u: vec![u; cw * ch],
            v: vec![v; cw * ch],
        }
    }

    /// Convert straight RGBA (alpha ignored; the caller composites onto an
    /// opaque background) to BT.709 limited-range 4:2:0. Chroma is the
    /// average of each 2x2 block (centre sited).
    pub fn from_rgba(rgba: &[u8], width: u32, height: u32) -> Self {
        let (w, h) = (width as usize, height as usize);
        assert!(rgba.len() >= w * h * 4, "RGBA buffer too small");
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        for (row, out) in y.chunks_mut(w.max(1)).enumerate().take(h) {
            let src = &rgba[row * w * 4..(row + 1) * w * 4];
            for (x, o) in out.iter_mut().enumerate() {
                let (r, g, b) = (src[x * 4] as i32, src[x * 4 + 1] as i32, src[x * 4 + 2] as i32);
                *o = (((KRY * r + KGY * g + KBY * b + 32768) >> 16) + 16) as u8;
            }
        }
        for cy in 0..ch {
            for cx in 0..cw {
                let (mut r, mut g, mut b, mut n) = (0i32, 0i32, 0i32, 0i32);
                for dy in 0..2 {
                    let yy = cy * 2 + dy;
                    if yy >= h {
                        continue;
                    }
                    for dx in 0..2 {
                        let xx = cx * 2 + dx;
                        if xx >= w {
                            continue;
                        }
                        let p = (yy * w + xx) * 4;
                        r += rgba[p] as i32;
                        g += rgba[p + 1] as i32;
                        b += rgba[p + 2] as i32;
                        n += 1;
                    }
                }
                // Average first (n is 1, 2 or 4), keeping two extra bits of precision.
                let (r, g, b) = ((r << 2) / n, (g << 2) / n, (b << 2) / n);
                let cu = ((KRU * r + KGU * g + KBU * b + (1 << 17)) >> 18) + 128;
                let cv = ((KRV * r + KGV * g + KBV * b + (1 << 17)) >> 18) + 128;
                u[cy * cw + cx] = cu.clamp(16, 240) as u8;
                v[cy * cw + cx] = cv.clamp(16, 240) as u8;
            }
        }
        Yuv420 { width, height, y, u, v }
    }

    /// Inverse of [`Yuv420::from_rgba`] (nearest-neighbour chroma), opaque output.
    pub fn to_rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let cw = self.chroma_width() as usize;
        let mut out = vec![255u8; w * h * 4];
        for yy in 0..h {
            for xx in 0..w {
                let c = (yy / 2) * cw + xx / 2;
                let (r, g, b) = ycbcr_to_rgb(self.y[yy * w + xx], self.u[c], self.v[c]);
                let p = (yy * w + xx) * 4;
                out[p] = r;
                out[p + 1] = g;
                out[p + 2] = b;
            }
        }
        out
    }
}

/// BT.709 limited range Y'CbCr to full range R'G'B'.
pub fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    let y = (y as f32 - 16.0) * (255.0 / 219.0);
    let cb = (cb as f32 - 128.0) * (255.0 / 224.0);
    let cr = (cr as f32 - 128.0) * (255.0 / 224.0);
    let r = y + 1.5748 * cr;
    let g = y - 0.187_324 * cb - 0.468_124 * cr;
    let b = y + 1.8556 * cb;
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    (c(r), c(g), c(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_map_to_bt709_limited_range() {
        let px = |r: u8, g: u8, b: u8| {
            let p = Yuv420::from_rgba(&[r, g, b, 255], 1, 1);
            (p.y[0], p.u[0], p.v[0])
        };
        assert_eq!(px(0, 0, 0), (16, 128, 128));
        assert_eq!(px(255, 255, 255), (235, 128, 128));
        // BT.709 red: Y 63, Cb 102, Cr 240.
        let (y, u, v) = px(255, 0, 0);
        assert!((y as i32 - 63).abs() <= 1 && (u as i32 - 102).abs() <= 1 && v == 240, "{y} {u} {v}");
        let (y, u, v) = px(0, 0, 255);
        assert!((y as i32 - 32).abs() <= 1 && u == 240 && (v as i32 - 118).abs() <= 1, "{y} {u} {v}");
    }

    #[test]
    fn round_trip_is_close() {
        let mut rgba = Vec::new();
        for i in 0..64u32 {
            rgba.extend_from_slice(&[(i * 4) as u8, 128, (255 - i * 4) as u8, 255]);
        }
        let p = Yuv420::from_rgba(&rgba, 64, 1);
        let back = p.to_rgba();
        for (a, b) in rgba.iter().zip(&back) {
            assert!((*a as i32 - *b as i32).abs() <= 6, "{a} vs {b}");
        }
    }
}
