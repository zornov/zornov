// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Output conversion: working space → the output colour space, dithered to
//! 8 bits or kept at 16 bits (for 10-bit export).
//!
//! Dithering adds triangular-PDF noise of ±1 LSB before rounding, from a
//! hash of pixel position, channel and frame number, so it is deterministic
//! (scrubbing and export agree) and the GPU path (`output.wgsl`) produces
//! the same bits.

use studio_colour::Transfer;
use studio_colour::math::Mat3;
use studio_colour::primaries::rgb_to_rgb;
use studio_colour::Primaries;

use crate::frame::{Frame, unpremultiply};

/// Output colour spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum OutputSpace {
    /// The viewer and still exports: BT.709 primaries, sRGB curve. On an
    /// sRGB monitor, this shows the working picture as it is.
    #[default]
    SrgbDisplay,
    /// Video export for BT.709 files: BT.709 primaries, inverse BT.1886
    /// EOTF, so a decoded BT.709 source passes through unchanged.
    Rec709,
    /// Display P3 primaries with the sRGB curve.
    DisplayP3,
    /// BT.2020 primaries, PQ (ST 2084), 1.0 = 203 cd/m².
    Rec2020Pq,
    /// BT.2020 primaries, HLG (BT.2100), 1.0 = 203 cd/m² on a 1000 cd/m²
    /// display.
    Rec2020Hlg,
    /// Scene-referred: ACES AP1 primaries, ACEScct encoded (−0.007 … 222
    /// linear in 0..1). For linear deliverables (OpenEXR), which decode it
    /// back; no view transform is applied to it.
    AcesCct,
}

impl OutputSpace {
    /// Whether the output can carry values above 1.0 (no tone mapping of
    /// HDR sources).
    pub fn is_hdr(self) -> bool {
        matches!(self, OutputSpace::Rec2020Pq | OutputSpace::Rec2020Hlg)
    }

    fn primaries(self) -> Primaries {
        match self {
            OutputSpace::SrgbDisplay | OutputSpace::Rec709 => Primaries::BT709,
            OutputSpace::DisplayP3 => Primaries::P3_D65,
            OutputSpace::Rec2020Pq | OutputSpace::Rec2020Hlg => Primaries::BT2020,
            OutputSpace::AcesCct => Primaries::ACES_AP1,
        }
    }

    /// Whether this output is scene-referred (no view transform).
    pub fn is_scene_referred(self) -> bool {
        self == OutputSpace::AcesCct
    }

    /// Matrix from working (BT.709) to the output primaries.
    pub fn from_working(self) -> Mat3 {
        match self {
            OutputSpace::SrgbDisplay | OutputSpace::Rec709 => Mat3::IDENTITY,
            p => rgb_to_rgb(&Primaries::BT709, &p.primaries()),
        }
    }

    /// Linear → code value in 0..1.
    pub fn encode(self, l: f32) -> f32 {
        match self {
            OutputSpace::SrgbDisplay | OutputSpace::DisplayP3 => Transfer::Srgb.encode(l),
            OutputSpace::Rec709 => Transfer::Bt1886.encode(l),
            OutputSpace::Rec2020Pq => Transfer::Pq.encode(l),
            // Display light (1 = 203 cd/m²) → scene light through the
            // inverse OOTF at 1000 cd/m², per channel (see colour_space).
            OutputSpace::Rec2020Hlg => Transfer::Hlg.encode(signed_pow(l * 203.0 / 1000.0, 1.0 / 1.2)),
            OutputSpace::AcesCct => Transfer::AcesCct.encode(l),
        }
    }

    /// Index used by the WGSL `encode_output` function.
    pub fn shader_id(self) -> u32 {
        match self {
            OutputSpace::SrgbDisplay => 0,
            OutputSpace::Rec709 => 1,
            OutputSpace::DisplayP3 => 2,
            OutputSpace::Rec2020Pq => 3,
            OutputSpace::Rec2020Hlg => 4,
            OutputSpace::AcesCct => 5,
        }
    }
}

fn signed_pow(x: f32, p: f32) -> f32 {
    if x < 0.0 { -(-x).powf(p) } else { x.powf(p) }
}

/// How the final buffer is laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputSettings {
    pub space: OutputSpace,
    /// Keep alpha premultiplied (what egui textures want) instead of
    /// un-premultiplying (what image files want).
    pub premultiplied: bool,
    /// Add ±1 LSB triangular dither before rounding.
    pub dither: bool,
    /// Draw at a requested size larger than the canvas (export); the
    /// preview leaves it off and is never upscaled.
    pub allow_upscale: bool,
}

impl Default for OutputSettings {
    fn default() -> Self {
        OutputSettings { space: OutputSpace::SrgbDisplay, premultiplied: false, dither: true, allow_upscale: false }
    }
}

/// 32-bit integer hash (the "lowbias32" mix), the same function as in
/// `output.wgsl`.
#[inline]
pub fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    x
}

/// Triangular dither in −1..1 LSB for a pixel, channel and frame.
#[inline]
pub fn dither_noise(x: u32, y: u32, channel: u32, frame: u32) -> f32 {
    let seed = hash32(x.wrapping_mul(0x9E3779B1) ^ y.wrapping_mul(0x85EBCA77) ^ channel.wrapping_mul(0xC2B2AE3D) ^ frame.wrapping_mul(0x27D4EB2F));
    let a = (seed & 0xffff) as f32 / 65536.0;
    let b = (seed >> 16) as f32 / 65536.0;
    a + b - 1.0
}

/// Converts one working pixel to encoded straight-or-premultiplied RGBA in
/// 0..1 (not yet quantised).
#[inline]
fn encode_pixel(p: [f32; 4], m: Option<&[[f32; 3]; 3]>, s: &OutputSettings) -> [f32; 4] {
    let a = p[3].clamp(0.0, 1.0);
    let c = if s.premultiplied { p } else { unpremultiply(p) };
    let mut rgb = [c[0], c[1], c[2]];
    if let Some(m) = m {
        rgb = [
            m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
            m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
            m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
        ];
    }
    let e = [s.space.encode(rgb[0]), s.space.encode(rgb[1]), s.space.encode(rgb[2])];
    // Premultiplied output must not exceed alpha after encoding; clamping
    // the encoded colour to alpha keeps textures valid.
    let lim = if s.premultiplied { a } else { 1.0 };
    [e[0].clamp(0.0, lim), e[1].clamp(0.0, lim), e[2].clamp(0.0, lim), a]
}

fn matrix(s: &OutputSettings) -> Option<[[f32; 3]; 3]> {
    let m = s.space.from_working();
    if m == Mat3::IDENTITY { None } else { Some(m.to_f32()) }
}

/// Bits per channel of an encoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputDepth {
    /// 8 bits per channel, dithered.
    Eight,
    /// 16 bits per channel (shift right by 6 for 10-bit video).
    Sixteen,
}

/// Encoded pixels at one of the two depths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodedPixels {
    Rgba8(Vec<u8>),
    Rgba16(Vec<u16>),
}

/// A frame after output conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: EncodedPixels,
}

/// Working frame → encoded pixels at `depth` (the CPU path; the GPU does
/// the same in `output.wgsl`).
pub fn encode(frame: &Frame, s: &OutputSettings, depth: OutputDepth, frame_index: u32) -> EncodedFrame {
    let pixels = match depth {
        OutputDepth::Eight => EncodedPixels::Rgba8(convert_rgba8(frame, s, frame_index)),
        OutputDepth::Sixteen => EncodedPixels::Rgba16(convert_rgba16(frame, s, frame_index)),
    };
    EncodedFrame { width: frame.width, height: frame.height, pixels }
}

/// Working frame → interleaved RGBA8.
pub fn convert_rgba8(frame: &Frame, s: &OutputSettings, frame_index: u32) -> Vec<u8> {
    let m = matrix(s);
    let w = frame.width as usize;
    let mut out = vec![0u8; frame.pixels.len() * 4];
    studio_media::par::par_rows(&mut out, w * 4, 64, |row, dst| {
        let y = row as u32;
        for (x, (d, p)) in dst.chunks_exact_mut(4).zip(&frame.pixels[row * w..row * w + w]).enumerate() {
            let e = encode_pixel(*p, m.as_ref(), s);
            for c in 0..4 {
                let v = e[c] * 255.0;
                let n = if s.dither { dither_noise(x as u32, y, c as u32, frame_index) } else { 0.0 };
                d[c] = (v + n + 0.5).floor().clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

/// Working frame → interleaved RGBA16 (full 16-bit range; shift right by
/// 6 for 10-bit video). Dither is scaled to the 16-bit LSB.
pub fn convert_rgba16(frame: &Frame, s: &OutputSettings, frame_index: u32) -> Vec<u16> {
    let m = matrix(s);
    let w = frame.width as usize;
    let mut out = vec![0u16; frame.pixels.len() * 4];
    studio_media::par::par_rows(&mut out, w * 4, 64, |row, dst| {
        let y = row as u32;
        for (x, (d, p)) in dst.chunks_exact_mut(4).zip(&frame.pixels[row * w..row * w + w]).enumerate() {
            let e = encode_pixel(*p, m.as_ref(), s);
            for c in 0..4 {
                let v = e[c] * 65535.0;
                let n = if s.dither { dither_noise(x as u32, y, c as u32, frame_index) } else { 0.0 };
                d[c] = (v + n + 0.5).floor().clamp(0.0, 65535.0) as u16;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip_8bit() {
        // Every 8-bit sRGB grey survives decode → encode exactly (no dither).
        let f = Frame::from_fn(256, 1, |x, _| {
            let l = Transfer::Srgb.decode(x as f32 / 255.0);
            [l, l, l, 1.0]
        });
        let out = convert_rgba8(&f, &OutputSettings { dither: false, ..Default::default() }, 0);
        for x in 0..256 {
            assert_eq!(out[x * 4] as usize, x);
            assert_eq!(out[x * 4 + 3], 255);
        }
    }

    #[test]
    fn dither_is_zero_mean_and_bounded() {
        let mut sum = 0.0f64;
        let n = 100_000;
        for i in 0..n {
            let d = dither_noise(i % 1000, i / 1000, i % 4, 7);
            assert!((-1.0..=1.0).contains(&d));
            sum += d as f64;
        }
        assert!((sum / n as f64).abs() < 0.01);
    }

    #[test]
    fn dither_breaks_up_flat_quantisation() {
        // A grey exactly between two codes rounds to both with dither on.
        let l = Transfer::Srgb.decode(100.5 / 255.0);
        let f = Frame::filled(64, 64, [l, l, l, 1.0]);
        let out = convert_rgba8(&f, &OutputSettings::default(), 3);
        let lo = out.chunks_exact(4).filter(|p| p[0] == 100).count();
        let hi = out.chunks_exact(4).filter(|p| p[0] == 101).count();
        assert!(lo > 1000 && hi > 1000, "{lo} {hi}");
    }

    #[test]
    fn straight_alpha_output() {
        let f = Frame::filled(1, 1, [0.25, 0.25, 0.25, 0.5]);
        let out = convert_rgba8(&f, &OutputSettings { dither: false, ..Default::default() }, 0);
        // 0.5 linear → sRGB 188; alpha 128.
        assert_eq!(out, vec![188, 188, 188, 128]);
        let out = convert_rgba8(&f, &OutputSettings { dither: false, premultiplied: true, ..Default::default() }, 0);
        // Premultiplied: encoded 0.25 linear = 137 but clamped to alpha (128).
        assert_eq!(out, vec![128, 128, 128, 128]);
    }

    #[test]
    fn hdr_spaces_encode_white() {
        let s = OutputSettings { space: OutputSpace::Rec2020Pq, dither: false, premultiplied: false, allow_upscale: false };
        let f = Frame::filled(1, 1, [1.0, 1.0, 1.0, 1.0]);
        let out = convert_rgba16(&f, &s, 0);
        // 203 nits in PQ is about 0.58.
        let v = out[0] as f32 / 65535.0;
        assert!((v - 0.58).abs() < 0.01, "{v}");
        let s = OutputSettings { space: OutputSpace::Rec2020Hlg, ..s };
        let out = convert_rgba16(&f, &s, 0);
        let v = out[0] as f32 / 65535.0;
        // HLG 203 nits ≈ 0.75 signal (BT.2408).
        assert!((v - 0.75).abs() < 0.03, "{v}");
    }
}
