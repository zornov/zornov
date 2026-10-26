// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Transfer functions (the curves between light and code values).
//!
//! Sources: IEC 61966-2-1 (sRGB), ITU-R BT.709 (camera OETF), ITU-R BT.1886
//! (reference display EOTF), SMPTE ST 2084 / ITU-R BT.2100 (PQ), ITU-R
//! BT.2100 (HLG), and Academy S-2016-001 / S-2014-003 (ACEScct / ACEScc).
//!
//! Every function works on one channel. The SDR curves (sRGB, BT.709,
//! BT.1886, gamma) are mirrored around zero, so slightly negative values
//! from gamut conversions survive a round trip instead of being clipped.

/// A named transfer curve. `encode` goes from linear light to code value,
/// `decode` goes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Transfer {
    /// No curve.
    #[default]
    Linear,
    /// IEC 61966-2-1 piecewise sRGB curve.
    Srgb,
    /// ITU-R BT.709 camera OETF (and its inverse for decode).
    Bt709,
    /// ITU-R BT.1886 reference display (pure 2.4 power with black at 0,
    /// white at 1). Encoding is the inverse EOTF.
    Bt1886,
    /// SMPTE ST 2084. Linear 1.0 is [`PQ_REFERENCE_WHITE_NITS`].
    Pq,
    /// ITU-R BT.2100 hybrid log-gamma OETF on normalised scene light.
    Hlg,
    /// ACEScct log encoding (meant for AP1 linear values).
    AcesCct,
    /// ACEScc pure-log encoding (meant for AP1 linear values).
    AcesCc,
}

/// Linear 1.0 maps to this luminance when encoding PQ through
/// [`Transfer::Pq`]. 203 cd/m² is the HDR reference (graphics) white of
/// ITU-R BT.2408, so SDR material placed into a PQ timeline keeps its
/// expected brightness.
pub const PQ_REFERENCE_WHITE_NITS: f32 = 203.0;

impl Transfer {
    /// Linear light → code value.
    pub fn encode(self, x: f32) -> f32 {
        match self {
            Transfer::Linear => x,
            Transfer::Srgb => mirror(x, srgb_encode),
            Transfer::Bt709 => mirror(x, bt709_oetf),
            Transfer::Bt1886 => mirror(x, |v| bt1886_inverse_eotf(v, 1.0, 0.0)),
            Transfer::Pq => pq_inverse_eotf(x * PQ_REFERENCE_WHITE_NITS),
            Transfer::Hlg => hlg_oetf(x),
            Transfer::AcesCct => lin_to_acescct(x),
            Transfer::AcesCc => lin_to_acescc(x),
        }
    }

    /// Code value → linear light.
    pub fn decode(self, v: f32) -> f32 {
        match self {
            Transfer::Linear => v,
            Transfer::Srgb => mirror(v, srgb_decode),
            Transfer::Bt709 => mirror(v, bt709_inverse_oetf),
            Transfer::Bt1886 => mirror(v, |e| bt1886_eotf(e, 1.0, 0.0)),
            Transfer::Pq => pq_eotf(v) / PQ_REFERENCE_WHITE_NITS,
            Transfer::Hlg => hlg_inverse_oetf(v),
            Transfer::AcesCct => acescct_to_lin(v),
            Transfer::AcesCc => acescc_to_lin(v),
        }
    }

    pub fn encode_rgb(self, c: [f32; 3]) -> [f32; 3] {
        [self.encode(c[0]), self.encode(c[1]), self.encode(c[2])]
    }

    pub fn decode_rgb(self, c: [f32; 3]) -> [f32; 3] {
        [self.decode(c[0]), self.decode(c[1]), self.decode(c[2])]
    }

    /// Index used by the WGSL `encode_transfer` / `decode_transfer`
    /// functions (see `shaders/transfer.wgsl`).
    pub fn shader_id(self) -> u32 {
        match self {
            Transfer::Linear => 0,
            Transfer::Srgb => 1,
            Transfer::Bt709 => 2,
            Transfer::Bt1886 => 3,
            Transfer::Pq => 4,
            Transfer::Hlg => 5,
            Transfer::AcesCct => 6,
            Transfer::AcesCc => 7,
        }
    }
}

#[inline]
fn mirror(x: f32, f: impl Fn(f32) -> f32) -> f32 {
    if x < 0.0 { -f(-x) } else { f(x) }
}

// ---------------------------------------------------------------- sRGB

pub fn srgb_encode(l: f32) -> f32 {
    if l <= 0.003_130_8 { 12.92 * l } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

// ---------------------------------------------------------------- BT.709

// The exact constants that make the two pieces meet with equal slope
// (BT.2020 lists them to more digits than BT.709's rounded 1.099 / 0.018).
const BT709_ALPHA: f32 = 1.099_296_8;
const BT709_BETA: f32 = 0.018_053_97;

/// BT.709 camera OETF: scene light → signal.
pub fn bt709_oetf(l: f32) -> f32 {
    if l < BT709_BETA { 4.5 * l } else { BT709_ALPHA * l.powf(0.45) - (BT709_ALPHA - 1.0) }
}

pub fn bt709_inverse_oetf(v: f32) -> f32 {
    if v < 4.5 * BT709_BETA {
        v / 4.5
    } else {
        ((v + (BT709_ALPHA - 1.0)) / BT709_ALPHA).powf(1.0 / 0.45)
    }
}

// ---------------------------------------------------------------- BT.1886

/// BT.1886 EOTF. `lw` / `lb` are display white and black luminance
/// (any unit; pass 1 and 0 for a normalised display).
pub fn bt1886_eotf(v: f32, lw: f32, lb: f32) -> f32 {
    let (a, b) = bt1886_ab(lw, lb);
    a * (v + b).max(0.0).powf(2.4)
}

pub fn bt1886_inverse_eotf(l: f32, lw: f32, lb: f32) -> f32 {
    let (a, b) = bt1886_ab(lw, lb);
    (l.max(0.0) / a).powf(1.0 / 2.4) - b
}

fn bt1886_ab(lw: f32, lb: f32) -> (f32, f32) {
    let w = lw.powf(1.0 / 2.4);
    let k = lb.powf(1.0 / 2.4);
    ((w - k).powf(2.4), k / (w - k))
}

// ---------------------------------------------------------------- PQ

const PQ_M1: f32 = 2610.0 / 16384.0;
const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f32 = 3424.0 / 4096.0;
const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;

/// ST 2084 EOTF: signal (0..1) → absolute luminance in cd/m² (0..10000).
pub fn pq_eotf(e: f32) -> f32 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    let y = ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1);
    y * 10000.0
}

/// ST 2084 inverse EOTF: luminance in cd/m² → signal.
pub fn pq_inverse_eotf(nits: f32) -> f32 {
    let y = (nits / 10000.0).clamp(0.0, 1.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
}

// ---------------------------------------------------------------- HLG

const HLG_A: f32 = 0.178_832_77;
const HLG_B: f32 = 0.284_668_92; // 1 − 4a
const HLG_C: f32 = 0.559_910_73; // 0.5 − a·ln(4a)

/// BT.2100 HLG OETF: normalised scene light (0..1) → signal (0..1).
pub fn hlg_oetf(e: f32) -> f32 {
    let e = e.max(0.0);
    if e <= 1.0 / 12.0 { (3.0 * e).sqrt() } else { HLG_A * (12.0 * e - HLG_B).ln() + HLG_C }
}

pub fn hlg_inverse_oetf(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.5 { v * v / 3.0 } else { (((v - HLG_C) / HLG_A).exp() + HLG_B) / 12.0 }
}

/// BT.2100 HLG OOTF: scene-linear BT.2020 RGB (0..1) → display light in
/// cd/m² for a display of nominal peak `lw`. The system gamma follows the
/// BT.2100 formula 1.2 + 0.42·log10(Lw / 1000).
pub fn hlg_ootf(rgb: [f32; 3], lw: f32) -> [f32; 3] {
    let gamma = 1.2 + 0.42 * (lw / 1000.0).log10();
    let ys = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
    let k = lw * ys.max(0.0).powf(gamma - 1.0);
    [k * rgb[0], k * rgb[1], k * rgb[2]]
}

// ---------------------------------------------------------------- ACES

const CCT_A: f32 = 10.540_237_7;
const CCT_B: f32 = 0.072_905_534;
const CCT_LIN_BREAK: f32 = 0.007_812_5;
const CCT_BREAK: f32 = 0.155_251_14;

/// Linear AP1 → ACEScct (S-2016-001).
pub fn lin_to_acescct(l: f32) -> f32 {
    if l <= CCT_LIN_BREAK { CCT_A * l + CCT_B } else { (l.log2() + 9.72) / 17.52 }
}

/// ACEScct → linear AP1, capped at the half-float maximum as the spec does.
pub fn acescct_to_lin(v: f32) -> f32 {
    if v <= CCT_BREAK {
        (v - CCT_B) / CCT_A
    } else if v < (65504f32.log2() + 9.72) / 17.52 {
        (v * 17.52 - 9.72).exp2()
    } else {
        65504.0
    }
}

/// Linear AP1 → ACEScc (S-2014-003).
pub fn lin_to_acescc(l: f32) -> f32 {
    if l <= 0.0 {
        (-16f32 + 9.72) / 17.52
    } else if l < (-15f32).exp2() {
        (((-16f32).exp2() + l * 0.5).log2() + 9.72) / 17.52
    } else {
        (l.log2() + 9.72) / 17.52
    }
}

/// ACEScc → linear AP1.
pub fn acescc_to_lin(v: f32) -> f32 {
    if v < (9.72 - 15.0) / 17.52 {
        ((v * 17.52 - 9.72).exp2() - (-16f32).exp2()) * 2.0
    } else if v < (65504f32.log2() + 9.72) / 17.52 {
        (v * 17.52 - 9.72).exp2()
    } else {
        65504.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn reference_points() {
        // sRGB mid code value.
        assert!(close(srgb_decode(0.5), 0.214_041, 1e-5));
        // BT.709: the knee of the curve.
        assert!(close(bt709_oetf(0.018), 0.081, 2e-4));
        assert!(close(bt709_oetf(1.0), 1.0, 1e-6));
        // PQ: 100 cd/m² sits at about 0.508, 10000 at 1.0.
        assert!(close(pq_inverse_eotf(100.0), 0.508_08, 1e-4));
        assert!(close(pq_inverse_eotf(10000.0), 1.0, 1e-6));
        // HLG: the two pieces meet at 1/12 → 0.5, and 1 → 1.
        assert!(close(hlg_oetf(1.0 / 12.0), 0.5, 1e-6));
        assert!(close(hlg_oetf(1.0), 1.0, 1e-6));
        // ACEScct: 18 % grey and the linear/log break point.
        assert!(close(lin_to_acescct(0.18), 0.413_588, 1e-5));
        assert!(close(lin_to_acescct(CCT_LIN_BREAK), CCT_BREAK, 1e-6));
        assert!(close(lin_to_acescc(0.18), 0.413_588, 1e-5));
        // BT.1886 with Lb = 0 is a plain 2.4 power.
        assert!(close(bt1886_eotf(0.5, 1.0, 0.0), 0.5f32.powf(2.4), 1e-6));
        // BT.1886 with a raised black maps code 0 to Lb.
        assert!(close(bt1886_eotf(0.0, 100.0, 0.1), 0.1, 1e-4));
    }

    #[test]
    fn round_trips() {
        let all = [
            Transfer::Linear,
            Transfer::Srgb,
            Transfer::Bt709,
            Transfer::Bt1886,
            Transfer::Pq,
            Transfer::Hlg,
            Transfer::AcesCct,
            Transfer::AcesCc,
        ];
        for t in all {
            // PQ's steep exponents lose a few more bits in f32.
            let eps = if t == Transfer::Pq { 3e-4 } else { 2e-5 };
            for i in 0..=200 {
                let x = i as f32 / 200.0;
                let y = t.decode(t.encode(x));
                assert!(close(x, y, eps * x.max(1.0)), "{t:?} at {x}: {y}");
            }
        }
    }

    #[test]
    fn hlg_ootf_peak() {
        let w = hlg_ootf([1.0, 1.0, 1.0], 1000.0);
        assert!(close(w[0], 1000.0, 1e-2));
    }
}
