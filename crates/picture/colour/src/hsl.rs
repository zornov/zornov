// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Eight-band HSL: per hue band, a hue shift, a saturation change and a
//! lightness change.
//!
//! Band centres (degrees in the hue plane of [`crate::space::Opponent`]):
//! red 0, orange 30, yellow 60, green 120, aqua 180, blue 240, purple 270,
//! magenta 300.
//!
//! # Band weights
//!
//! Between two neighbouring centres `c_i < c_j` a hue `h` gives band `j`
//! the weight `s = smoothstep(c_i, c_j, h)` and band `i` the weight `1 − s`;
//! all other bands get 0. So the weights overlap with their neighbours,
//! are C¹-smooth (zero slope at every centre) and always sum to one.
//!
//! Per pixel the blended controls are applied as:
//! hue rotation `Σ wᵢ·hueᵢ · 30°`, chroma × `(1 + Σ wᵢ·satᵢ)`, and mean
//! level `+ Σ wᵢ·lightᵢ · 0.2 · smoothstep(0, 0.08, chroma)`, the last
//! factor keeping greys (which have no meaningful hue) untouched.

use crate::math::{Rgb, smoothstep};
use crate::space::Opponent;

pub const BAND_NAMES: [&str; 8] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];
pub const BAND_CENTRES: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];
/// Hue shift at ±1, degrees.
pub const HUE_RANGE_DEG: f32 = 30.0;
/// Lightness change at ±1, normalised grading units.
pub const LIGHTNESS_RANGE: f32 = 0.2;

/// Controls of one band, each −1..+1.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct HslBand {
    pub hue: f32,
    pub saturation: f32,
    pub lightness: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Hsl {
    pub bands: [HslBand; 8],
}

/// Weights of the eight bands at `hue_deg`.
pub fn band_weights(hue_deg: f32) -> [f32; 8] {
    let h = hue_deg.rem_euclid(360.0);
    let mut w = [0.0; 8];
    for i in 0..8 {
        let j = (i + 1) % 8;
        let a = BAND_CENTRES[i];
        let b = if j == 0 { 360.0 } else { BAND_CENTRES[j] };
        if h >= a && h < b {
            let s = smoothstep(a, b, h);
            w[i] = 1.0 - s;
            w[j] = s;
            break;
        }
    }
    w
}

impl Hsl {
    pub fn is_identity(&self) -> bool {
        self.bands.iter().all(|b| *b == HslBand::default())
    }

    /// Applies the bands to normalised grading values.
    pub fn apply(&self, n: Rgb) -> Rgb {
        let o = Opponent::from_rgb(n);
        let chroma = o.chroma();
        if chroma == 0.0 {
            return n;
        }
        let w = band_weights(o.hue_degrees());
        let (mut dh, mut ds, mut dl) = (0.0, 0.0, 0.0);
        for (wi, b) in w.iter().zip(&self.bands) {
            dh += wi * b.hue;
            ds += wi * b.saturation;
            dl += wi * b.lightness;
        }
        let mut o = o.rotate(dh * HUE_RANGE_DEG).scale_chroma((1.0 + ds).max(0.0));
        o.m += dl * LIGHTNESS_RANGE * smoothstep(0.0, 0.08, chroma);
        o.to_rgb()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_partition_unity() {
        for i in 0..3600 {
            let h = i as f32 / 10.0;
            let w = band_weights(h);
            let sum: f32 = w.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "{h}: {w:?}");
            assert!(w.iter().all(|&v| v >= 0.0));
        }
        for (i, &c) in BAND_CENTRES.iter().enumerate() {
            assert!((band_weights(c)[i] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn band_targets_its_hue() {
        let mut hsl = Hsl::default();
        hsl.bands[5].saturation = -1.0; // blue
        let blue = hsl.apply([0.2, 0.2, 0.8]);
        assert!(Opponent::from_rgb(blue).chroma() < 1e-5);
        let red = [0.8, 0.2, 0.2];
        assert_eq!(hsl.apply(red), Opponent::from_rgb(red).to_rgb());
        assert_eq!(hsl.apply([0.4; 3]), [0.4; 3]);
    }
}
