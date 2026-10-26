// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Y′CbCr matrices (ITU-R BT.601, BT.709, BT.2020) and code-value ranges.
//!
//! All functions work on gamma-encoded (primed) R′G′B′ in 0..1. Y′ is
//! 0..1 and Cb/Cr are −0.5..+0.5.

/// Which luma coefficients to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum YCbCrMatrix {
    Bt601,
    #[default]
    Bt709,
    /// BT.2020 non-constant-luminance.
    Bt2020,
}

impl YCbCrMatrix {
    /// (Kr, Kb) as published; Kg = 1 − Kr − Kb.
    pub fn kr_kb(self) -> (f32, f32) {
        match self {
            YCbCrMatrix::Bt601 => (0.299, 0.114),
            YCbCrMatrix::Bt709 => (0.2126, 0.0722),
            YCbCrMatrix::Bt2020 => (0.2627, 0.0593),
        }
    }

    pub fn luma_weights(self) -> [f32; 3] {
        let (kr, kb) = self.kr_kb();
        [kr, 1.0 - kr - kb, kb]
    }

    pub fn to_ycbcr(self, rgb: [f32; 3]) -> [f32; 3] {
        let (kr, kb) = self.kr_kb();
        let y = kr * rgb[0] + (1.0 - kr - kb) * rgb[1] + kb * rgb[2];
        [y, (rgb[2] - y) / (2.0 * (1.0 - kb)), (rgb[0] - y) / (2.0 * (1.0 - kr))]
    }

    pub fn from_ycbcr(self, ycc: [f32; 3]) -> [f32; 3] {
        let (kr, kb) = self.kr_kb();
        let kg = 1.0 - kr - kb;
        let r = ycc[0] + 2.0 * (1.0 - kr) * ycc[2];
        let b = ycc[0] + 2.0 * (1.0 - kb) * ycc[1];
        let g = (ycc[0] - kr * r - kb * b) / kg;
        [r, g, b]
    }
}

/// Code-value range of integer Y′CbCr.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Range {
    /// "Narrow" / "video" range: Y′ 16–235, C 16–240 at 8 bits.
    #[default]
    Limited,
    /// Full range: Y′ 0–2ⁿ−1, C centred on 2ⁿ⁻¹.
    Full,
}

impl Range {
    /// Normalised Y′CbCr → integer code values at `bits` (8–16).
    pub fn quantize(self, ycc: [f32; 3], bits: u32) -> [u16; 3] {
        let max = ((1u32 << bits) - 1) as f32;
        let q = |v: f32| v.round().clamp(0.0, max) as u16;
        match self {
            Range::Limited => {
                let s = (1u32 << (bits - 8)) as f32;
                [q((219.0 * ycc[0] + 16.0) * s), q((224.0 * ycc[1] + 128.0) * s), q((224.0 * ycc[2] + 128.0) * s)]
            }
            Range::Full => {
                let mid = (1u32 << (bits - 1)) as f32;
                [q(max * ycc[0]), q(max * ycc[1] + mid), q(max * ycc[2] + mid)]
            }
        }
    }

    /// Integer code values → normalised Y′CbCr.
    pub fn dequantize(self, code: [u16; 3], bits: u32) -> [f32; 3] {
        let c = [code[0] as f32, code[1] as f32, code[2] as f32];
        match self {
            Range::Limited => {
                let s = (1u32 << (bits - 8)) as f32;
                [(c[0] / s - 16.0) / 219.0, (c[1] / s - 128.0) / 224.0, (c[2] / s - 128.0) / 224.0]
            }
            Range::Full => {
                let max = ((1u32 << bits) - 1) as f32;
                let mid = (1u32 << (bits - 1)) as f32;
                [c[0] / max, (c[1] - mid) / max, (c[2] - mid) / max]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_primaries() {
        for m in [YCbCrMatrix::Bt601, YCbCrMatrix::Bt709, YCbCrMatrix::Bt2020] {
            let w = m.to_ycbcr([1.0, 1.0, 1.0]);
            assert!((w[0] - 1.0).abs() < 1e-6 && w[1].abs() < 1e-6 && w[2].abs() < 1e-6);
            // Pure blue and pure red reach the ±0.5 extremes.
            assert!((m.to_ycbcr([0.0, 0.0, 1.0])[1] - 0.5).abs() < 1e-6);
            assert!((m.to_ycbcr([1.0, 0.0, 0.0])[2] - 0.5).abs() < 1e-6);
            let c = [0.3, 0.6, 0.9];
            let r = m.from_ycbcr(m.to_ycbcr(c));
            for i in 0..3 {
                assert!((r[i] - c[i]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn code_values() {
        assert_eq!(Range::Limited.quantize([1.0, 0.0, 0.0], 8), [235, 128, 128]);
        assert_eq!(Range::Limited.quantize([0.0, -0.5, 0.5], 10), [64, 64, 960]);
        assert_eq!(Range::Full.quantize([1.0, 0.0, 0.0], 8), [255, 128, 128]);
        let d = Range::Limited.dequantize([235, 240, 16], 8);
        assert!((d[0] - 1.0).abs() < 1e-6 && (d[1] - 0.5).abs() < 1e-6 && (d[2] + 0.5).abs() < 1e-6);
    }
}
