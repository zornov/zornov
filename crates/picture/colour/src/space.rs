// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The grading space and the hue plane used inside it.
//!
//! Grading tools (contrast, tone ranges, wheels, curves, HSL) work on
//! ACEScct values: working linear RGB is converted to AP1 primaries and
//! log-encoded, which spaces the tonal range roughly evenly in stops.
//!
//! Most tools then use a *normalised* grading value
//! `n = (cct − BLACK) / (WHITE − BLACK)`, where `BLACK` is ACEScct of
//! linear 0 and `WHITE` is ACEScct of linear 1.0 (diffuse white). So
//! n = 0 is black, n = 1 is diffuse white, 18 % grey is n ≈ 0.707, and
//! highlights above diffuse white continue past 1. Using n means curve
//! and wheel parameters read the same way on every clip.

use crate::math::{Mat3, Rgb};
use crate::primaries::{Primaries, rgb_to_rgb};
use crate::transfer::{acescct_to_lin, lin_to_acescct};

/// ACEScct of linear 0.
pub const CCT_BLACK: f32 = 0.072_905_534;
/// ACEScct of linear 1.0 (= 9.72 / 17.52).
pub const CCT_WHITE: f32 = 0.554_794_5;
/// ACEScct of linear 0.18, the contrast pivot.
pub const CCT_GREY: f32 = 0.413_588;
/// Normalised grading value of 18 % grey.
pub const N_GREY: f32 = (CCT_GREY - CCT_BLACK) / (CCT_WHITE - CCT_BLACK);

/// ACEScct → normalised grading value.
#[inline]
pub fn cct_to_n(v: f32) -> f32 {
    (v - CCT_BLACK) * (1.0 / (CCT_WHITE - CCT_BLACK))
}

/// Normalised grading value → ACEScct.
#[inline]
pub fn n_to_cct(n: f32) -> f32 {
    n * (CCT_WHITE - CCT_BLACK) + CCT_BLACK
}

/// Precomputed conversions between working linear RGB and the grading
/// space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradingSpace {
    pub to_ap1: Mat3,
    pub from_ap1: Mat3,
}

impl GradingSpace {
    pub fn new(working: &Primaries) -> GradingSpace {
        GradingSpace {
            to_ap1: rgb_to_rgb(working, &Primaries::ACES_AP1),
            from_ap1: rgb_to_rgb(&Primaries::ACES_AP1, working),
        }
    }

    /// Working linear RGB → normalised grading values.
    pub fn to_grading(&self, lin: Rgb) -> Rgb {
        let ap1 = self.to_ap1.apply(lin);
        ap1.map(|c| cct_to_n(lin_to_acescct(c)))
    }

    /// Normalised grading values → working linear RGB.
    pub fn from_grading(&self, n: Rgb) -> Rgb {
        let ap1 = n.map(|c| acescct_to_lin(n_to_cct(c)));
        self.from_ap1.apply(ap1)
    }
}

// --------------------------------------------------------------- hue plane

/// A colour split into its mean level `m` and a 2-D chroma vector
/// `(x, y)` in the plane perpendicular to the grey axis.
///
/// The axes are chosen so that the hue angle `atan2(y, x)` puts the
/// primaries and secondaries at the familiar hexagon positions: red 0°,
/// yellow 60°, green 120°, cyan 180°, blue 240°, magenta 300°. Rotating
/// `(x, y)` changes hue, scaling it changes saturation and moving `m`
/// changes lightness, all without leaving the grading space and for any
/// input value (including negatives), which an HSV conversion cannot do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opponent {
    pub m: f32,
    pub x: f32,
    pub y: f32,
}

const SQRT3_2: f32 = 0.866_025_4;
const INV_SQRT3: f32 = 0.577_350_3;

impl Opponent {
    pub fn from_rgb(c: Rgb) -> Opponent {
        Opponent {
            m: (c[0] + c[1] + c[2]) * (1.0 / 3.0),
            x: c[0] - 0.5 * (c[1] + c[2]),
            y: SQRT3_2 * (c[1] - c[2]),
        }
    }

    pub fn to_rgb(self) -> Rgb {
        let Opponent { m, x, y } = self;
        [m + x * (2.0 / 3.0), m - x * (1.0 / 3.0) + y * INV_SQRT3, m - x * (1.0 / 3.0) - y * INV_SQRT3]
    }

    pub fn chroma(self) -> f32 {
        self.x.hypot(self.y)
    }

    /// Hue in degrees, 0..360.
    pub fn hue_degrees(self) -> f32 {
        let h = self.y.atan2(self.x).to_degrees();
        if h < 0.0 { h + 360.0 } else { h }
    }

    /// Rotates the hue by `deg` degrees.
    pub fn rotate(self, deg: f32) -> Opponent {
        let (s, c) = deg.to_radians().sin_cos();
        Opponent { m: self.m, x: self.x * c - self.y * s, y: self.x * s + self.y * c }
    }

    pub fn scale_chroma(self, k: f32) -> Opponent {
        Opponent { m: self.m, x: self.x * k, y: self.y * k }
    }
}

/// RGB offset of unit chroma in direction `hue_deg` with zero mean, used
/// for the colour push of a grading wheel.
pub fn hue_direction(hue_deg: f32) -> Rgb {
    let (s, c) = hue_deg.to_radians().sin_cos();
    Opponent { m: 0.0, x: c, y: s }.to_rgb()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_agree_with_transfer() {
        assert!((lin_to_acescct(0.0) - CCT_BLACK).abs() < 1e-7);
        assert!((lin_to_acescct(1.0) - CCT_WHITE).abs() < 1e-7);
        assert!((lin_to_acescct(0.18) - CCT_GREY).abs() < 1e-6);
    }

    #[test]
    fn grading_round_trip() {
        let g = GradingSpace::new(&Primaries::BT709);
        for c in [[0.0, 0.0, 0.0], [0.18, 0.18, 0.18], [1.0, 0.5, 0.1], [4.0, 0.02, 0.6]] {
            let n = g.to_grading(c);
            let back = g.from_grading(n);
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-5 * c[i].max(1.0), "{c:?} → {back:?}");
            }
        }
        let grey = g.to_grading([0.18; 3]);
        assert!((grey[0] - N_GREY).abs() < 1e-4 && (grey[1] - N_GREY).abs() < 1e-4);
    }

    #[test]
    fn hue_angles() {
        let h = |c: Rgb| Opponent::from_rgb(c).hue_degrees();
        assert!(h([1.0, 0.0, 0.0]).abs() < 1e-4);
        assert!((h([1.0, 1.0, 0.0]) - 60.0).abs() < 1e-4);
        assert!((h([0.0, 1.0, 0.0]) - 120.0).abs() < 1e-4);
        assert!((h([0.0, 0.0, 1.0]) - 240.0).abs() < 1e-4);
        assert!((h([1.0, 0.0, 1.0]) - 300.0).abs() < 1e-4);
        let c = [0.3, 0.7, 0.2];
        let r = Opponent::from_rgb(c).to_rgb();
        for i in 0..3 {
            assert!((r[i] - c[i]).abs() < 1e-6);
        }
        let d = hue_direction(0.0);
        assert!((d[0] + d[1] + d[2]).abs() < 1e-6 && d[0] > 0.0);
    }
}
