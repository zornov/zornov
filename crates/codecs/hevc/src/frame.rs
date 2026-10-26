// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Output frames (cropped I420 planes) and colour conversion.

use crate::picture::Picture;

/// Sample storage of one plane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaneData {
    /// 8-bit samples.
    U8(Vec<u8>),
    /// Samples with bit depth > 8, stored LSB-aligned in 16-bit containers.
    U16(Vec<u16>),
}

impl PlaneData {
    pub fn len(&self) -> usize {
        match self {
            PlaneData::U8(v) => v.len(),
            PlaneData::U16(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub fn get(&self, i: usize) -> u16 {
        match self {
            PlaneData::U8(v) => v[i] as u16,
            PlaneData::U16(v) => v[i],
        }
    }
    pub fn as_u8(&self) -> Option<&[u8]> {
        match self {
            PlaneData::U8(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_u16(&self) -> Option<&[u16]> {
        match self {
            PlaneData::U16(v) => Some(v),
            _ => None,
        }
    }
}

/// YCbCr to RGB matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl Matrix {
    /// Maps VUI matrix_coeffs (H.273) to a matrix; unspecified values map to BT.709.
    pub fn from_vui(matrix_coeffs: u8) -> Matrix {
        match matrix_coeffs {
            5 | 6 => Matrix::Bt601,
            9 | 10 => Matrix::Bt2020,
            _ => Matrix::Bt709,
        }
    }
    fn kr_kb(self) -> (f32, f32) {
        match self {
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt2020 => (0.2627, 0.0593),
        }
    }
}

/// Colour description from the SPS VUI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorInfo {
    pub full_range: bool,
    pub matrix_coeffs: u8,
}

/// How to reduce >8-bit samples to 8 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownConvert {
    /// `v >> (bd - 8)`
    Truncate,
    /// `min(255, (v + (1 << (bd - 9))) >> (bd - 8))`
    Round,
}

/// A decoded picture in output (display) order with cropping applied.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub chroma_width: usize,
    pub chroma_height: usize,
    pub bit_depth: u8,
    pub bit_depth_chroma: u8,
    pub y: PlaneData,
    pub u: PlaneData,
    pub v: PlaneData,
    /// Picture order count (relative within a coded video sequence).
    pub poc: i32,
    pub color: ColorInfo,
    /// True if decoding errors were concealed in this picture (or it was generated).
    pub corrupt: bool,
}

impl Frame {
    pub(crate) fn from_picture(p: &Picture) -> Frame {
        let [l, r, t, b] = p.crop;
        let w = p.width().saturating_sub(l + r).max(1);
        let h = p.height().saturating_sub(t + b).max(1);
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let crop = |plane: &crate::picture::Plane, x0: usize, y0: usize, w: usize, h: usize, eight: bool| -> PlaneData {
            let w = w.min(plane.width - x0.min(plane.width));
            let h = h.min(plane.height - y0.min(plane.height));
            if eight {
                let mut out = Vec::with_capacity(w * h);
                for y in 0..h {
                    let row = (y0 + y) * plane.stride + x0;
                    out.extend(plane.data[row..row + w].iter().map(|&v| v as u8));
                }
                PlaneData::U8(out)
            } else {
                let mut out = Vec::with_capacity(w * h);
                for y in 0..h {
                    let row = (y0 + y) * plane.stride + x0;
                    out.extend_from_slice(&plane.data[row..row + w]);
                }
                PlaneData::U16(out)
            }
        };
        let eight = p.bit_depth_luma == 8 && p.bit_depth_chroma == 8;
        Frame {
            width: w,
            height: h,
            chroma_width: cw,
            chroma_height: ch,
            bit_depth: p.bit_depth_luma,
            bit_depth_chroma: p.bit_depth_chroma,
            y: crop(&p.planes[0], l, t, w, h, eight),
            u: crop(&p.planes[1], l / 2, t / 2, cw, ch, eight),
            v: crop(&p.planes[2], l / 2, t / 2, cw, ch, eight),
            poc: p.poc,
            color: ColorInfo { full_range: p.full_range, matrix_coeffs: p.matrix_coeffs },
            corrupt: p.corrupt,
        }
    }

    /// Converts the planes to 8-bit I420 (Y, U, V concatenated).
    pub fn to_i420_u8(&self, mode: DownConvert) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.y.len() + self.u.len() + self.v.len());
        for (plane, bd) in [(&self.y, self.bit_depth), (&self.u, self.bit_depth_chroma), (&self.v, self.bit_depth_chroma)] {
            match plane {
                PlaneData::U8(v) => out.extend_from_slice(v),
                PlaneData::U16(v) => {
                    let s = bd.saturating_sub(8) as u32;
                    for &x in v {
                        let y = match mode {
                            DownConvert::Truncate => x >> s,
                            DownConvert::Round => {
                                if s == 0 {
                                    x
                                } else {
                                    ((x as u32 + (1 << (s - 1))) >> s).min(255) as u16
                                }
                            }
                        };
                        out.push(y as u8);
                    }
                }
            }
        }
        out
    }

    /// Converts to interleaved 8-bit RGBA using the VUI colour description
    /// (BT.709 limited range when unspecified).
    pub fn to_rgba(&self) -> Vec<u8> {
        self.to_rgba_with(Matrix::from_vui(self.color.matrix_coeffs), self.color.full_range)
    }

    /// Converts to interleaved 8-bit RGBA with an explicit matrix and range.
    pub fn to_rgba_with(&self, matrix: Matrix, full_range: bool) -> Vec<u8> {
        let (kr, kb) = matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        let bd = self.bit_depth as i32;
        let bdc = self.bit_depth_chroma as i32;
        let (y_off, y_scale) = if full_range {
            (0.0f32, 1.0 / ((1i32 << bd) - 1) as f32)
        } else {
            ((16i32 << (bd - 8)) as f32, 1.0 / (219i32 << (bd - 8)) as f32)
        };
        let c_off = (1i32 << (bdc - 1)) as f32;
        let c_scale = if full_range {
            1.0 / ((1i32 << bdc) - 1) as f32
        } else {
            1.0 / (224i32 << (bdc - 8)) as f32
        };
        let cr_r = 2.0 * (1.0 - kr);
        let cb_b = 2.0 * (1.0 - kb);
        let cb_g = -2.0 * kb * (1.0 - kb) / kg;
        let cr_g = -2.0 * kr * (1.0 - kr) / kg;
        let mut out = vec![255u8; self.width * self.height * 4];
        for y in 0..self.height {
            let cy = (y / 2).min(self.chroma_height.saturating_sub(1));
            for x in 0..self.width {
                let cx = (x / 2).min(self.chroma_width.saturating_sub(1));
                let yy = (self.y.get(y * self.width + x) as f32 - y_off) * y_scale;
                let ci = cy * self.chroma_width + cx;
                let cb = (self.u.get(ci) as f32 - c_off) * c_scale;
                let cr = (self.v.get(ci) as f32 - c_off) * c_scale;
                let r = yy + cr_r * cr;
                let g = yy + cb_g * cb + cr_g * cr;
                let b = yy + cb_b * cb;
                let o = (y * self.width + x) * 4;
                out[o] = (r * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                out[o + 1] = (g * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                out[o + 2] = (b * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_gray_and_red() {
        let f = Frame {
            width: 2,
            height: 2,
            chroma_width: 1,
            chroma_height: 1,
            bit_depth: 8,
            bit_depth_chroma: 8,
            y: PlaneData::U8(vec![16, 235, 126, 126]),
            u: PlaneData::U8(vec![128]),
            v: PlaneData::U8(vec![128]),
            poc: 0,
            color: ColorInfo { full_range: false, matrix_coeffs: 1 },
            corrupt: false,
        };
        let rgba = f.to_rgba();
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]);
        // BT.709 red: Y=63, Cb=102, Cr=240
        let f2 = Frame { y: PlaneData::U8(vec![63; 4]), u: PlaneData::U8(vec![102]), v: PlaneData::U8(vec![240]), ..f };
        let rgba = f2.to_rgba();
        assert!(rgba[0] > 250 && rgba[1] < 5 && rgba[2] < 5, "{:?}", &rgba[0..4]);
    }
}
