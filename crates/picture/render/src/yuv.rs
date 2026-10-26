// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoded pictures as they come out of the decoders, and their conversion
//! into the working space on the CPU.
//!
//! The decode cache keeps pictures in this compact form (8- or 16-bit
//! planes, 1.5 bytes per pixel for 8-bit 4:2:0) rather than as `f32` RGBA
//! (16 bytes per pixel). Conversion happens when a picture is drawn: on
//! the GPU in `convert.wgsl`, on the CPU here. Both do the same steps in the
//! same order: dequantise, Y′CbCr → R′G′B′, transfer decode, primaries to
//! BT.709, optional tone mapping, premultiply (alpha 1). Chroma is taken
//! from the co-located subsampled sample (nearest), as the decoders' own
//! RGB converters do.

use std::sync::Arc;

use crate::colour_space::{ColourDesc, DecodeLut, Matrix, SourceTransfer, tone_map};
use crate::frame::Frame;

/// Sample planes at 8 or more bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Planes {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl Planes {
    pub fn len(&self) -> usize {
        match self {
            Planes::U8(v) => v.len(),
            Planes::U16(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn byte_size(&self) -> usize {
        match self {
            Planes::U8(v) => v.len(),
            Planes::U16(v) => v.len() * 2,
        }
    }

    #[inline]
    pub fn get(&self, i: usize) -> f32 {
        match self {
            Planes::U8(v) => v[i] as f32,
            Planes::U16(v) => v[i] as f32,
        }
    }
}

/// Pixel data of a decoded picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PixelData {
    /// Planar Y′CbCr with chroma subsampled by `chroma_div` (2 for 4:2:0,
    /// 1 for 4:4:4) in both directions.
    Yuv { y: Planes, u: Planes, v: Planes, chroma_width: u32, chroma_height: u32 },
    /// Interleaved straight-alpha RGBA, 8 bits per channel.
    Rgba8(Vec<u8>),
}

/// A decoded picture plus the colour description needed to use it.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedPicture {
    pub width: u32,
    pub height: u32,
    pub data: PixelData,
    pub colour: ColourDesc,
}

impl DecodedPicture {
    /// Bytes held by the sample data (what the decode cache budgets).
    pub fn byte_size(&self) -> usize {
        match &self.data {
            PixelData::Yuv { y, u, v, .. } => y.byte_size() + u.byte_size() + v.byte_size(),
            PixelData::Rgba8(p) => p.len(),
        }
    }

    /// Wraps straight-alpha sRGB RGBA8 pixels (a still image).
    pub fn from_rgba8(width: u32, height: u32, rgba: Vec<u8>) -> DecodedPicture {
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
        DecodedPicture { width, height, data: PixelData::Rgba8(rgba), colour: ColourDesc::SRGB8 }
    }

    /// Wraps 8-bit 4:2:0 planes.
    pub fn from_i420(width: u32, height: u32, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>, colour: ColourDesc) -> DecodedPicture {
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        assert_eq!(y.len(), width as usize * height as usize);
        assert_eq!(u.len(), cw as usize * ch as usize);
        assert_eq!(v.len(), cw as usize * ch as usize);
        DecodedPicture {
            width,
            height,
            data: PixelData::Yuv { y: Planes::U8(y), u: Planes::U8(u), v: Planes::U8(v), chroma_width: cw, chroma_height: ch },
            colour: ColourDesc { bit_depth: 8, ..colour },
        }
    }

    /// Converts into the working space. `to_sdr` applies [`tone_map`] to
    /// HDR sources (PQ / HLG); SDR sources are never tone mapped.
    pub fn to_frame(&self, to_sdr: bool) -> Frame {
        let conv = Converter::new(&self.colour, to_sdr);
        let (w, h) = (self.width as usize, self.height as usize);
        let mut pixels = vec![[0.0f32; 4]; w * h];
        match &self.data {
            PixelData::Rgba8(rgba) => {
                let lut = crate::colour_space::srgb8_to_linear_lut();
                let srgb = self.colour.transfer == SourceTransfer::Srgb && self.colour.bit_depth == 8;
                for (out, p) in pixels.iter_mut().zip(rgba.chunks_exact(4)) {
                    let a = p[3] as f32 / 255.0;
                    let rgb = if srgb {
                        [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]]
                    } else {
                        conv.rgb_prime_to_linear([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
                    };
                    let rgb = conv.finish(rgb);
                    *out = [rgb[0] * a, rgb[1] * a, rgb[2] * a, a];
                }
            }
            PixelData::Yuv { y, u, v, chroma_width, .. } => {
                let cw = *chroma_width as usize;
                let xdiv = if cw >= w { 1 } else { 2 };
                let ydiv = if (y.len() / w.max(1)) <= (u.len() / cw.max(1)) { 1 } else { 2 };
                studio_media::par::par_rows(&mut pixels, w, 32, |row, out| {
                    let yrow = row * w;
                    let crow = (row / ydiv) * cw;
                    for (x, px) in out.iter_mut().enumerate() {
                        let yy = y.get(yrow + x);
                        let cb = u.get(crow + x / xdiv);
                        let cr = v.get(crow + x / xdiv);
                        let rgb = conv.ycc_codes_to_linear([yy, cb, cr]);
                        let rgb = conv.finish(rgb);
                        *px = [rgb[0], rgb[1], rgb[2], 1.0];
                    }
                });
            }
        }
        Frame { width: self.width, height: self.height, pixels }
    }
}

/// Precomputed conversion state for one colour description.
pub struct Converter {
    y_off: f32,
    y_scale: f32,
    c_off: f32,
    c_scale: f32,
    matrix: Matrix,
    kr: f32,
    kb: f32,
    lut: DecodeLut,
    to_working: Option<[[f32; 3]; 3]>,
    tone_map: bool,
    /// An OCIO input processor (encoded R′G′B′ → working linear).
    custom: Option<std::sync::Arc<studio_colour::ocio::Processor>>,
}

impl Converter {
    pub fn new(desc: &ColourDesc, to_sdr: bool) -> Converter {
        let (y_off, y_scale, c_off, c_scale) = desc.quantisation();
        let (kr, kb) = desc.matrix.ycbcr().kr_kb();
        let m = desc.to_working();
        let to_working = if m == studio_colour::math::Mat3::IDENTITY { None } else { Some(m.to_f32()) };
        Converter {
            y_off,
            y_scale,
            c_off,
            c_scale,
            matrix: desc.matrix,
            kr,
            kb,
            lut: DecodeLut::new(desc.transfer),
            to_working,
            tone_map: to_sdr && desc.transfer.is_hdr(),
            custom: match desc.transfer {
                SourceTransfer::Custom(id) => crate::colour_space::custom(id),
                _ => None,
            },
        }
    }

    /// Integer code values → linear RGB in the source primaries.
    #[inline]
    pub fn ycc_codes_to_linear(&self, c: [f32; 3]) -> [f32; 3] {
        let y = (c[0] - self.y_off) * self.y_scale;
        let cb = (c[1] - self.c_off) * self.c_scale;
        let cr = (c[2] - self.c_off) * self.c_scale;
        let rgb = if self.matrix == Matrix::Rgb {
            // Planes hold G, B, R in the Y, Cb, Cr positions (H.273 identity
            // matrix layout); recover with full-range scale.
            [(c[2] - self.y_off) * self.y_scale, y, (c[1] - self.y_off) * self.y_scale]
        } else {
            let kg = 1.0 - self.kr - self.kb;
            let r = y + 2.0 * (1.0 - self.kr) * cr;
            let b = y + 2.0 * (1.0 - self.kb) * cb;
            let g = (y - self.kr * r - self.kb * b) / kg;
            [r, g, b]
        };
        self.rgb_prime_to_linear(rgb)
    }

    /// Encoded R′G′B′ (0..1) → linear RGB in the source primaries.
    #[inline]
    pub fn rgb_prime_to_linear(&self, c: [f32; 3]) -> [f32; 3] {
        if let Some(p) = &self.custom {
            return p.apply(c);
        }
        [self.lut.decode(c[0]), self.lut.decode(c[1]), self.lut.decode(c[2])]
    }

    /// Primaries conversion and tone mapping.
    #[inline]
    pub fn finish(&self, rgb: [f32; 3]) -> [f32; 3] {
        let rgb = match &self.to_working {
            None => rgb,
            Some(m) => [
                m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
                m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
                m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
            ],
        };
        if self.tone_map { tone_map(rgb) } else { rgb }
    }
}

/// Shared handle to a cached picture.
pub type PictureRef = Arc<DecodedPicture>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_ramp_round_trip() {
        // Limited-range BT.709 grey: Y′ = 16 is black, 235 is white.
        let w = 8;
        let y: Vec<u8> = (0..w).map(|x| 16 + (x * 219 / (w - 1)) as u8).collect();
        let pic = DecodedPicture::from_i420(w as u32, 1, y, vec![128; 4], vec![128; 4], ColourDesc::default());
        let f = pic.to_frame(true);
        assert!(f.get(0, 0)[0].abs() < 1e-3);
        assert!((f.get(7, 0)[0] - 1.0).abs() < 2e-3);
        // Neutral: R = G = B.
        for x in 0..8 {
            let p = f.get(x, 0);
            assert!((p[0] - p[1]).abs() < 1e-4 && (p[1] - p[2]).abs() < 1e-4);
            assert_eq!(p[3], 1.0);
        }
        // Monotonic.
        for x in 1..8 {
            assert!(f.get(x, 0)[0] > f.get(x - 1, 0)[0]);
        }
    }

    #[test]
    fn rgba8_still_is_srgb_premultiplied() {
        let pic = DecodedPicture::from_rgba8(1, 1, vec![255, 128, 0, 128]);
        let f = pic.to_frame(true);
        let p = f.get(0, 0);
        let a = 128.0 / 255.0;
        assert!((p[3] - a).abs() < 1e-6);
        assert!((p[0] - a).abs() < 1e-5);
        assert!((p[1] - 0.2158605 * a).abs() < 1e-3);
        assert_eq!(p[2], 0.0);
    }

    #[test]
    fn hdr_white_lands_near_one() {
        // PQ code for 203 nits.
        let v = studio_colour::Transfer::Pq.encode(1.0);
        let code = (v * 1023.0).round() as u16;
        let desc = ColourDesc::from_h273(9, 16, 9, true, 10);
        let pic = DecodedPicture {
            width: 1,
            height: 1,
            data: PixelData::Yuv { y: Planes::U16(vec![code]), u: Planes::U16(vec![512]), v: Planes::U16(vec![512]), chroma_width: 1, chroma_height: 1 },
            colour: desc,
        };
        let p = pic.to_frame(true).get(0, 0);
        for c in &p[..3] {
            assert!((c - 1.0).abs() < 0.03, "{p:?}");
        }
    }
}
