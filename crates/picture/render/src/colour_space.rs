// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Source colour descriptions and the conversion into the working space.
//!
//! The working space is linear light with BT.709 primaries, premultiplied
//! alpha, `f32` on the CPU and `rgba16float` on the GPU. 1.0 is diffuse
//! white; HDR values above 1 are kept until output.
//!
//! Sources arrive as Y′CbCr planes (video) or 8-bit sRGB (stills). Their
//! description follows ITU-T H.273 code points as carried in the MP4 /
//! SPS VUI: colour primaries, transfer characteristics, matrix
//! coefficients and the full/limited range flag.
//!
//! HDR sources (PQ, HLG) are mapped to SDR with the knee curve documented
//! at [`tone_map`]: 203 cd/m² (BT.2408 graphics white) is 1.0, values above
//! a knee of 0.8 roll off smoothly towards 2.0 on the largest channel, so
//! hue is kept and nothing clips abruptly.

use std::sync::OnceLock;
use std::path::Path;
use std::sync::{Arc, Mutex};

use studio_colour::camera::{CameraSpace, Gamut, LogCurve};
use studio_colour::math::Mat3;
use studio_colour::ocio::{Config, Processor};
use studio_colour::{Primaries, Transfer, YCbCrMatrix};

/// Which Y′CbCr matrix a video source uses (H.273 `MatrixCoefficients`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Matrix {
    Bt601,
    #[default]
    Bt709,
    Bt2020,
    /// Planes already hold R′G′B′ (identity matrix, code 0).
    Rgb,
}

impl Matrix {
    /// From an H.273 matrix_coefficients code; unspecified means BT.709,
    /// as every player assumes for HD material.
    pub fn from_h273(code: u8) -> Matrix {
        match code {
            0 => Matrix::Rgb,
            5 | 6 => Matrix::Bt601,
            9 | 10 => Matrix::Bt2020,
            _ => Matrix::Bt709,
        }
    }

    pub fn ycbcr(self) -> YCbCrMatrix {
        match self {
            Matrix::Bt601 => YCbCrMatrix::Bt601,
            Matrix::Bt709 | Matrix::Rgb => YCbCrMatrix::Bt709,
            Matrix::Bt2020 => YCbCrMatrix::Bt2020,
        }
    }
}

/// Which primaries a source is in (H.273 `ColourPrimaries`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SourcePrimaries {
    #[default]
    Bt709,
    /// BT.601 625-line (EBU) primaries, code 5.
    Bt601_625,
    /// BT.601 525-line (SMPTE C) primaries, code 6.
    Bt601_525,
    Bt2020,
    P3D65,
    /// A camera or ACES gamut (set by an input colour space).
    Gamut(Gamut),
}

impl SourcePrimaries {
    /// From an H.273 colour_primaries code; unspecified means BT.709.
    pub fn from_h273(code: u8) -> SourcePrimaries {
        match code {
            5 => SourcePrimaries::Bt601_625,
            6 | 7 => SourcePrimaries::Bt601_525,
            9 => SourcePrimaries::Bt2020,
            12 => SourcePrimaries::P3D65,
            _ => SourcePrimaries::Bt709,
        }
    }

    pub fn primaries(self) -> Primaries {
        match self {
            SourcePrimaries::Bt709 => Primaries::BT709,
            SourcePrimaries::Bt601_625 => Primaries::BT601_625,
            SourcePrimaries::Bt601_525 => Primaries::BT601_525,
            SourcePrimaries::Bt2020 => Primaries::BT2020,
            SourcePrimaries::P3D65 => Primaries::P3_D65,
            SourcePrimaries::Gamut(g) => g.primaries(),
        }
    }
}

/// Transfer characteristics of a source (H.273 `TransferCharacteristics`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SourceTransfer {
    /// sRGB (IEC 61966-2-1), code 13; also what stills use.
    Srgb,
    /// BT.709 / BT.601 camera curve, codes 1, 6, 14, 15. Video is mastered
    /// for a BT.1886 display, so decoding uses the BT.1886 EOTF, which is
    /// what the viewer sees; the camera OETF is not inverted.
    #[default]
    Bt709,
    /// Pure 2.2 power (code 4), rare.
    Gamma22,
    /// Pure 2.8 power (code 5), rare.
    Gamma28,
    /// SMPTE ST 2084 PQ, code 16.
    Pq,
    /// ARIB STD-B67 HLG, code 18.
    Hlg,
    /// Linear, code 8.
    Linear,
    /// A camera log curve (set by an input colour space). `legal` says
    /// the planes are limited range, so code values are first mapped back
    /// to full range as the curve's formula expects.
    Camera { curve: LogCurve, legal: bool },
    /// An OpenColorIO input processor registered with [`register_custom`]:
    /// it maps encoded R′G′B′ straight to working linear (primaries
    /// included), so it is applied per pixel rather than per channel.
    Custom(u32),
}

impl SourceTransfer {
    /// From an H.273 transfer_characteristics code; unspecified means
    /// BT.709.
    pub fn from_h273(code: u8) -> SourceTransfer {
        match code {
            13 => SourceTransfer::Srgb,
            4 => SourceTransfer::Gamma22,
            5 => SourceTransfer::Gamma28,
            8 => SourceTransfer::Linear,
            16 => SourceTransfer::Pq,
            18 => SourceTransfer::Hlg,
            _ => SourceTransfer::Bt709,
        }
    }

    /// Whether the source is high dynamic range (needs tone mapping for
    /// an SDR output).
    pub fn is_hdr(self) -> bool {
        matches!(self, SourceTransfer::Pq | SourceTransfer::Hlg)
    }

    /// Whether this is an RGB → RGB processor rather than a curve.
    pub fn is_custom(self) -> bool {
        matches!(self, SourceTransfer::Custom(_))
    }

    /// Code value (0..1) → linear light (1 = diffuse / graphics white).
    pub fn decode(self, v: f32) -> f32 {
        match self {
            SourceTransfer::Srgb => Transfer::Srgb.decode(v),
            SourceTransfer::Bt709 => Transfer::Bt1886.decode(v),
            SourceTransfer::Gamma22 => mirror(v, |x| x.powf(2.2)),
            SourceTransfer::Gamma28 => mirror(v, |x| x.powf(2.8)),
            SourceTransfer::Pq => Transfer::Pq.decode(v),
            // HLG scene light → display light through the BT.2100 OOTF at a
            // 1000 cd/m² display, then scaled so 203 cd/m² is 1.0. The
            // per-channel form is used for the LUT; the shader does the
            // same, so the two paths agree.
            SourceTransfer::Hlg => {
                let scene = Transfer::Hlg.decode(v);
                hlg_display(scene) / (203.0 / 1000.0)
            }
            SourceTransfer::Linear => v,
            SourceTransfer::Camera { curve, legal } => curve.decode(if legal { studio_colour::camera::legal_to_full(v) } else { v }),
            // Applied per pixel by the converter.
            SourceTransfer::Custom(_) => v,
        }
    }

    /// Index used by the WGSL `decode_source` function: camera curves are
    /// `16 + 2 × curve + legal`.
    pub fn shader_id(self) -> u32 {
        match self {
            SourceTransfer::Srgb => 0,
            SourceTransfer::Bt709 => 1,
            SourceTransfer::Gamma22 => 2,
            SourceTransfer::Gamma28 => 3,
            SourceTransfer::Pq => 4,
            SourceTransfer::Hlg => 5,
            SourceTransfer::Linear => 6,
            SourceTransfer::Camera { curve, legal } => 16 + 2 * curve.shader_id() + legal as u32,
            SourceTransfer::Custom(_) => 6,
        }
    }
}

/// HLG system gamma 1.2 at 1000 cd/m² applied per channel (an approximation
/// of the luminance-based OOTF that keeps the CPU and GPU paths simple and
/// identical).
fn hlg_display(scene: f32) -> f32 {
    mirror(scene, |x| x.powf(1.2))
}

#[inline]
fn mirror(x: f32, f: impl Fn(f32) -> f32) -> f32 {
    if x < 0.0 { -f(-x) } else { f(x) }
}

/// Everything needed to turn a source's samples into the working space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColourDesc {
    pub primaries: SourcePrimaries,
    pub transfer: SourceTransfer,
    pub matrix: Matrix,
    pub full_range: bool,
    /// Bits per sample of the planes (8..16).
    pub bit_depth: u8,
}

impl Default for ColourDesc {
    /// BT.709 limited-range 8-bit video.
    fn default() -> ColourDesc {
        ColourDesc { primaries: SourcePrimaries::Bt709, transfer: SourceTransfer::Bt709, matrix: Matrix::Bt709, full_range: false, bit_depth: 8 }
    }
}

impl ColourDesc {
    /// sRGB 8-bit RGBA stills.
    pub const SRGB8: ColourDesc =
        ColourDesc { primaries: SourcePrimaries::Bt709, transfer: SourceTransfer::Srgb, matrix: Matrix::Rgb, full_range: true, bit_depth: 8 };

    /// From H.273 code points as carried in a VUI or `colr` box.
    pub fn from_h273(primaries: u8, transfer: u8, matrix: u8, full_range: bool, bit_depth: u8) -> ColourDesc {
        ColourDesc {
            primaries: SourcePrimaries::from_h273(primaries),
            transfer: SourceTransfer::from_h273(transfer),
            matrix: Matrix::from_h273(matrix),
            full_range,
            bit_depth: bit_depth.clamp(8, 16),
        }
    }

    /// Matrix from the source primaries to the working (BT.709) primaries,
    /// with Bradford adaptation of the white point. Identity for BT.709
    /// and both BT.601 variants, whose primaries differ from BT.709 by
    /// less than the quantisation of 8-bit video (and which every player
    /// treats as BT.709).
    pub fn to_working(&self) -> Mat3 {
        match self.primaries {
            SourcePrimaries::Bt709 | SourcePrimaries::Bt601_625 | SourcePrimaries::Bt601_525 => Mat3::IDENTITY,
            p => studio_colour::primaries::rgb_to_rgb(&p.primaries(), &Primaries::BT709),
        }
    }

    /// Luma/chroma scale and offset for integer code values at this bit
    /// depth: `(y_offset, y_scale, c_offset, c_scale)`, so that
    /// `Y' = (code - y_offset) * y_scale` and `C = (code - c_offset) * c_scale`.
    pub fn quantisation(&self) -> (f32, f32, f32, f32) {
        let bd = self.bit_depth as i32;
        let c_off = (1i32 << (bd - 1)) as f32;
        if self.full_range {
            let s = 1.0 / ((1i32 << bd) - 1) as f32;
            (0.0, s, c_off, s)
        } else {
            let sh = bd - 8;
            ((16 << sh) as f32, 1.0 / (219 << sh) as f32, c_off, 1.0 / (224 << sh) as f32)
        }
    }
}

/// A colour space chosen for a source, overriding what its file says (the
/// input transform of a colour-managed pipeline). Only the parts it sets
/// are replaced; matrix and range still come from the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct InputColour {
    pub primaries: Option<SourcePrimaries>,
    pub transfer: Option<InputTransfer>,
}

/// The transfer part of an [`InputColour`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputTransfer {
    Fixed(SourceTransfer),
    /// A camera curve; range is taken from the stream.
    Camera(LogCurve),
}

impl InputColour {
    /// Applies the override to a stream's own description.
    pub fn apply(&self, mut d: ColourDesc) -> ColourDesc {
        if let Some(p) = self.primaries {
            d.primaries = p;
        }
        match self.transfer {
            Some(InputTransfer::Fixed(t)) => d.transfer = t,
            Some(InputTransfer::Camera(curve)) => d.transfer = SourceTransfer::Camera { curve, legal: !d.full_range && d.matrix != Matrix::Rgb },
            None => {}
        }
        d
    }

    /// Parses a Studio input colour space name:
    ///
    /// * camera spaces: `slog3_sgamut3cine`, `logc3_awg3`, `vlog_vgamut`,
    ///   `applelog`, … (a bare curve takes the camera's native gamut);
    /// * `rec709` (BT.1886 video), `srgb`, `linear` / `lin_rec709`,
    ///   `acescg`, `aces` / `aces2065-1`, `acescct`, `rec2020`,
    ///   `rec2020_pq`, `rec2020_hlg`, `p3d65`, `displayp3`;
    /// * `ocio:<colour space>`: from the project's OCIO config (`config`).
    pub fn parse(spec: &str, config: Option<&Path>) -> Result<InputColour, String> {
        let s = spec.trim();
        if let Some(name) = s.strip_prefix("ocio:") {
            let path = config.ok_or("an ocio: input colour space needs an OCIO config on the project (colour config)")?;
            let id = register_ocio_input(path, name)?;
            return Ok(InputColour { primaries: Some(SourcePrimaries::Bt709), transfer: Some(InputTransfer::Fixed(SourceTransfer::Custom(id))) });
        }
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        let fixed = |p: SourcePrimaries, t: SourceTransfer| Ok(InputColour { primaries: Some(p), transfer: Some(InputTransfer::Fixed(t)) });
        match k.as_str() {
            "" | "auto" | "file" | "default" => return Ok(InputColour::default()),
            "rec709" | "bt709" | "bt1886" | "video" => return fixed(SourcePrimaries::Bt709, SourceTransfer::Bt709),
            "srgb" => return fixed(SourcePrimaries::Bt709, SourceTransfer::Srgb),
            "linear" | "linrec709" | "linsrgb" | "linearrec709" | "linearsrgb" | "scenelinear" => return fixed(SourcePrimaries::Bt709, SourceTransfer::Linear),
            "acescg" | "linap1" => return fixed(SourcePrimaries::Gamut(Gamut::AcesAp1), SourceTransfer::Linear),
            "aces" | "aces20651" | "linap0" => return fixed(SourcePrimaries::Gamut(Gamut::AcesAp0), SourceTransfer::Linear),
            "rec2020" | "bt2020" => return fixed(SourcePrimaries::Bt2020, SourceTransfer::Bt709),
            "linrec2020" | "linearrec2020" => return fixed(SourcePrimaries::Bt2020, SourceTransfer::Linear),
            "rec2020pq" | "rec2100pq" | "pq" | "hdr10" => return fixed(SourcePrimaries::Bt2020, SourceTransfer::Pq),
            "rec2020hlg" | "rec2100hlg" | "hlg" => return fixed(SourcePrimaries::Bt2020, SourceTransfer::Hlg),
            "p3d65" | "p3" => return fixed(SourcePrimaries::P3D65, SourceTransfer::Bt709),
            "displayp3" => return fixed(SourcePrimaries::P3D65, SourceTransfer::Srgb),
            _ => {}
        }
        let cam = CameraSpace::parse(s).ok_or_else(|| format!("unknown input colour space \"{s}\"; try {}", input_names().join(", ")))?;
        Ok(InputColour { primaries: Some(gamut_primaries(cam.gamut)), transfer: Some(InputTransfer::Camera(cam.curve)) })
    }
}

fn gamut_primaries(g: Gamut) -> SourcePrimaries {
    match g {
        Gamut::Rec709 => SourcePrimaries::Bt709,
        Gamut::Rec2020 => SourcePrimaries::Bt2020,
        Gamut::P3D65 => SourcePrimaries::P3D65,
        g => SourcePrimaries::Gamut(g),
    }
}

/// The input colour space names Studio knows (besides `ocio:`).
pub fn input_names() -> Vec<String> {
    let mut v: Vec<String> = ["rec709", "srgb", "linear", "acescg", "aces2065-1", "acescct", "rec2020", "rec2020_pq", "rec2020_hlg", "p3d65", "displayp3"].iter().map(|s| s.to_string()).collect();
    v.extend(CameraSpace::presets().into_iter().map(|c| c.slug()));
    v
}

/// OCIO input processors, by id (see [`SourceTransfer::Custom`]).
fn custom_registry() -> &'static Mutex<Vec<(String, Arc<Processor>)>> {
    static R: OnceLock<Mutex<Vec<(String, Arc<Processor>)>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// Registers an encoded-R′G′B′ → working-linear processor under `key`
/// (the same key returns the same id).
pub fn register_custom(key: &str, p: Processor) -> u32 {
    let mut r = custom_registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = r.iter().position(|(k, _)| k == key) {
        return i as u32;
    }
    r.push((key.to_string(), Arc::new(p)));
    (r.len() - 1) as u32
}

/// The processor behind a custom transfer id.
pub fn custom(id: u32) -> Option<Arc<Processor>> {
    custom_registry().lock().unwrap_or_else(|e| e.into_inner()).get(id as usize).map(|(_, p)| p.clone())
}

/// OCIO configs by path, loaded once.
pub fn ocio_config(path: &Path) -> Result<Arc<Config>, String> {
    static C: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, Arc<Config>>>> = OnceLock::new();
    let mut g = C.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.get(path) {
        return Ok(c.clone());
    }
    let c = Arc::new(Config::load(path)?);
    g.insert(path.to_path_buf(), c.clone());
    Ok(c)
}

fn register_ocio_input(config: &Path, name: &str) -> Result<u32, String> {
    let cfg = ocio_config(config)?;
    let p = cfg.to_working(name)?;
    Ok(register_custom(&format!("{}\u{0}{name}", config.display()), p))
}

/// Tone mapping from HDR working values to SDR, applied to the largest
/// channel so hue is preserved: identity up to the knee `K = 0.8`; above it
/// `K + (2 - K) * (1 - exp(-(x - K) / (2 - K)))`, which is continuous in
/// value and slope at the knee and never exceeds 2.0 (so a little headroom
/// above display white survives for the output encoder to clip).
pub fn tone_map(rgb: [f32; 3]) -> [f32; 3] {
    const K: f32 = 0.8;
    const TOP: f32 = 2.0;
    let m = rgb[0].max(rgb[1]).max(rgb[2]);
    if m <= K || !m.is_finite() {
        return rgb;
    }
    let mapped = K + (TOP - K) * (1.0 - (-(m - K) / (TOP - K)).exp());
    let k = mapped / m;
    [rgb[0] * k, rgb[1] * k, rgb[2] * k]
}

/// 256-entry sRGB → linear table.
pub fn srgb8_to_linear_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| Transfer::Srgb.decode(i as f32 / 255.0)))
}

/// Number of entries in the transfer decode tables.
pub const DECODE_LUT_SIZE: usize = 4096;

/// A table of `transfer.decode(v)` for `v` in 0..1 with 4096 entries,
/// linearly interpolated between entries, plus one overshoot entry for
/// values slightly above 1 (super-white in limited-range video).
#[derive(Clone)]
pub struct DecodeLut {
    table: Vec<f32>,
}

impl DecodeLut {
    pub fn new(transfer: SourceTransfer) -> DecodeLut {
        let n = DECODE_LUT_SIZE;
        // Covers -0.1 .. 1.2 so limited-range super-blacks and super-whites
        // decode without clamping.
        let table = (0..=n).map(|i| transfer.decode(LUT_MIN + (LUT_MAX - LUT_MIN) * i as f32 / n as f32)).collect();
        DecodeLut { table }
    }

    #[inline]
    pub fn decode(&self, v: f32) -> f32 {
        let n = DECODE_LUT_SIZE as f32;
        let f = ((v - LUT_MIN) / (LUT_MAX - LUT_MIN) * n).clamp(0.0, n - 1e-3);
        let i = f as usize;
        let t = f - i as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * t
    }
}

/// Range covered by [`DecodeLut`].
pub const LUT_MIN: f32 = -0.1;
pub const LUT_MAX: f32 = 1.2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h273_codes() {
        let d = ColourDesc::from_h273(9, 16, 9, false, 10);
        assert_eq!(d.primaries, SourcePrimaries::Bt2020);
        assert_eq!(d.transfer, SourceTransfer::Pq);
        assert_eq!(d.matrix, Matrix::Bt2020);
        assert!(d.transfer.is_hdr());
        let d = ColourDesc::from_h273(2, 2, 2, false, 8);
        assert_eq!(d, ColourDesc::default());
    }

    #[test]
    fn quantisation_limited_8bit() {
        let d = ColourDesc::default();
        let (yo, ys, co, cs) = d.quantisation();
        assert_eq!(yo, 16.0);
        assert!(((235.0 - yo) * ys - 1.0).abs() < 1e-6);
        assert_eq!(co, 128.0);
        assert!(((240.0 - co) * cs - 0.5).abs() < 1e-6);
    }

    #[test]
    fn tone_map_is_continuous_and_bounded() {
        let a = tone_map([0.8, 0.4, 0.2]);
        assert_eq!(a, [0.8, 0.4, 0.2]);
        let b = tone_map([0.8001, 0.4, 0.2]);
        assert!((b[0] - 0.8001).abs() < 1e-3);
        let big = tone_map([50.0, 25.0, 0.0]);
        assert!(big[0] <= 2.0 && big[0] > 1.9);
        // Hue (channel ratios) is preserved.
        assert!((big[1] / big[0] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn decode_lut_matches_function() {
        for t in [SourceTransfer::Srgb, SourceTransfer::Bt709, SourceTransfer::Pq, SourceTransfer::Hlg] {
            let lut = DecodeLut::new(t);
            for i in 0..=100 {
                let v = i as f32 / 100.0;
                let err = (lut.decode(v) - t.decode(v)).abs();
                let scale = t.decode(v).abs().max(1.0);
                assert!(err / scale < 2e-3, "{t:?} at {v}: {err}");
            }
        }
    }

    #[test]
    fn working_matrix_identity_for_709() {
        assert_eq!(ColourDesc::default().to_working(), Mat3::IDENTITY);
        let m = ColourDesc { primaries: SourcePrimaries::Bt2020, ..ColourDesc::default() }.to_working();
        // BT.2020 red is outside BT.709: red maps to a value above 1 in R
        // and negative G/B.
        let r = m.apply([1.0, 0.0, 0.0]);
        assert!(r[0] > 1.0 && r[1] < 0.0 && r[2] < 0.0);
        // White stays white.
        let w = m.apply([1.0, 1.0, 1.0]);
        for c in w {
            assert!((c - 1.0).abs() < 1e-4);
        }
    }
}
