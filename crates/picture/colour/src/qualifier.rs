// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HSL qualifiers: a soft key that selects pixels by hue, saturation and
//! lightness, for secondary corrections (grade only the skin, the sky…).
//!
//! The key is computed on BT.709 display-encoded values (BT.1886), the
//! way colourists read a picture: hue in degrees on the HSL wheel (red 0,
//! yellow 60, green 120, cyan 180, blue 240, magenta 300), saturation and
//! lightness 0..1 as in HSL. Each axis is a window with soft edges:
//!
//! ```text
//! w(v) = smoothstep(low − soft, low, v) · (1 − smoothstep(high, high + soft, v))
//! ```
//!
//! (hue wraps around), and the key is the product of the three windows,
//! optionally inverted. Greys have no hue, so the hue window fades out as
//! saturation drops below 0.04.

use crate::math::{Rgb, smoothstep};
use crate::transfer::Transfer;

/// One HSL qualifier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Qualifier {
    /// Hue centre, degrees.
    pub hue: f32,
    /// Half width of the hue window, degrees (180 = every hue).
    pub hue_width: f32,
    /// Hue edge softness, degrees.
    pub hue_soft: f32,
    pub sat_low: f32,
    pub sat_high: f32,
    pub sat_soft: f32,
    pub lum_low: f32,
    pub lum_high: f32,
    pub lum_soft: f32,
    /// Select everything except the qualified range.
    pub invert: bool,
}

impl Default for Qualifier {
    /// Everything: a key of 1 everywhere.
    fn default() -> Qualifier {
        Qualifier { hue: 0.0, hue_width: 180.0, hue_soft: 0.0, sat_low: 0.0, sat_high: 1.0, sat_soft: 0.0, lum_low: 0.0, lum_high: 1.0, lum_soft: 0.0, invert: false }
    }
}

impl Qualifier {
    /// Named presets: `skin`, `sky`, `foliage` (greens), `highlights`,
    /// `shadows`, `midtones`, and the six hue names (`red`, `yellow`,
    /// `green`, `cyan`, `blue`, `magenta`, plus `orange`).
    pub fn preset(name: &str) -> Option<Qualifier> {
        let hue = |h: f32, w: f32| Qualifier { hue: h, hue_width: w, hue_soft: 15.0, sat_low: 0.12, sat_high: 1.0, sat_soft: 0.08, lum_low: 0.05, lum_high: 0.98, lum_soft: 0.05, invert: false };
        let lum = |lo: f32, hi: f32| Qualifier { lum_low: lo, lum_high: hi, lum_soft: 0.12, ..Qualifier::default() };
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "skin" | "skin tones" | "skintones" | "faces" => Qualifier { hue: 22.0, hue_width: 22.0, hue_soft: 14.0, sat_low: 0.12, sat_high: 0.75, sat_soft: 0.1, lum_low: 0.15, lum_high: 0.92, lum_soft: 0.08, invert: false },
            "sky" | "skies" => Qualifier { hue: 212.0, hue_width: 28.0, hue_soft: 16.0, sat_low: 0.12, sat_high: 1.0, sat_soft: 0.1, lum_low: 0.3, lum_high: 1.0, lum_soft: 0.15, invert: false },
            "foliage" | "greens" | "grass" | "trees" => Qualifier { hue: 95.0, hue_width: 40.0, hue_soft: 18.0, sat_low: 0.1, sat_high: 1.0, sat_soft: 0.08, lum_low: 0.05, lum_high: 0.95, lum_soft: 0.05, invert: false },
            "red" | "reds" => hue(0.0, 18.0),
            "orange" | "oranges" => hue(30.0, 14.0),
            "yellow" | "yellows" => hue(58.0, 16.0),
            "green" => hue(120.0, 30.0),
            "cyan" | "aqua" => hue(180.0, 22.0),
            "blue" | "blues" => hue(230.0, 28.0),
            "magenta" | "purple" | "magentas" => hue(300.0, 30.0),
            "highlights" | "brights" => lum(0.7, 1.0),
            "shadows" | "darks" => Qualifier { lum_low: 0.0, lum_high: 0.25, lum_soft: 0.12, ..Qualifier::default() },
            "midtones" | "mids" => lum(0.3, 0.7),
            _ => return None,
        })
    }

    /// A qualifier centred on a picked (working linear) colour, with the
    /// given tolerance (0..1, about 0.15 is a sensible start).
    pub fn from_sample(lin: Rgb, tolerance: f32) -> Qualifier {
        let (h, s, l) = hsl(Transfer::Bt1886.encode_rgb(lin));
        let t = tolerance.clamp(0.01, 1.0);
        Qualifier {
            hue: h,
            hue_width: if s < 0.05 { 180.0 } else { 10.0 + 60.0 * t },
            hue_soft: 8.0 + 30.0 * t,
            sat_low: (s - t).max(0.0),
            sat_high: (s + t).min(1.0),
            sat_soft: 0.5 * t,
            lum_low: (l - 1.5 * t).max(0.0),
            lum_high: (l + 1.5 * t).min(1.0),
            lum_soft: 0.5 * t,
            invert: false,
        }
    }

    /// Key 0..1 of one working-linear (BT.709) pixel.
    pub fn key(&self, lin: Rgb) -> f32 {
        let (h, s, l) = hsl(Transfer::Bt1886.encode_rgb(lin.map(|v| v.max(0.0))));
        let sat = window(s, self.sat_low, self.sat_high, self.sat_soft);
        let lum = window(l, self.lum_low, self.lum_high, self.lum_soft);
        let hue = if self.hue_width >= 180.0 {
            1.0
        } else {
            let mut d = (h - self.hue).rem_euclid(360.0);
            if d > 180.0 {
                d = 360.0 - d;
            }
            let w = 1.0 - smoothstep(self.hue_width, self.hue_width + self.hue_soft.max(1e-3), d);
            // Greys have no hue: fade the hue test out towards them.
            let g = smoothstep(0.0, 0.04, s);
            w * g
        };
        let k = (hue * sat * lum).clamp(0.0, 1.0);
        if self.invert { 1.0 - k } else { k }
    }
}

fn window(v: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    let a = if lo <= 0.0 { 1.0 } else if soft <= 0.0 { f32::from(v >= lo) } else { smoothstep(lo - soft, lo, v) };
    let b = if hi >= 1.0 { 1.0 } else if soft <= 0.0 { f32::from(v <= hi) } else { 1.0 - smoothstep(hi, hi + soft, v) };
    a * b
}

/// HSL of display-encoded RGB: hue degrees, saturation and lightness 0..1.
pub fn hsl(c: Rgb) -> (f32, f32, f32) {
    let c = c.map(|v| v.clamp(0.0, 1.0));
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d <= 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if mx == c[0] {
        60.0 * ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if mx == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    (h, s.min(1.0), l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin(c: Rgb) -> Rgb {
        Transfer::Bt1886.decode_rgb(c)
    }

    #[test]
    fn hsl_reference() {
        let (h, s, l) = hsl([1.0, 0.0, 0.0]);
        assert_eq!((h, s, l), (0.0, 1.0, 0.5));
        assert_eq!(hsl([0.0, 0.0, 1.0]).0, 240.0);
        assert_eq!(hsl([0.5, 0.5, 0.5]), (0.0, 0.0, 0.5));
    }

    #[test]
    fn presets_select_what_they_say() {
        let skin = Qualifier::preset("skin").unwrap();
        let sky = Qualifier::preset("sky").unwrap();
        let face = lin([0.85, 0.62, 0.5]);
        let blue_sky = lin([0.45, 0.65, 0.92]);
        let grass = lin([0.3, 0.55, 0.2]);
        let grey = lin([0.5, 0.5, 0.5]);
        assert!(skin.key(face) > 0.9, "{}", skin.key(face));
        assert!(skin.key(blue_sky) < 0.01);
        assert!(skin.key(grey) < 0.01);
        assert!(sky.key(blue_sky) > 0.9);
        assert!(sky.key(grass) < 0.01);
        assert!(Qualifier::preset("foliage").unwrap().key(grass) > 0.9);
        let hi = Qualifier::preset("highlights").unwrap();
        assert!(hi.key(lin([0.95; 3])) > 0.9 && hi.key(lin([0.2; 3])) < 0.01);
        let mut inv = skin;
        inv.invert = true;
        assert!(inv.key(face) < 0.1);
    }

    #[test]
    fn sample_picks_its_own_colour() {
        let c = lin([0.2, 0.5, 0.7]);
        let q = Qualifier::from_sample(c, 0.15);
        assert!(q.key(c) > 0.99);
        assert!(q.key(lin([0.7, 0.3, 0.2])) < 0.01);
        assert_eq!(Qualifier::default().key(c), 1.0);
    }

    #[test]
    fn hue_wraps() {
        let q = Qualifier::preset("red").unwrap();
        assert!(q.key(lin([0.8, 0.1, 0.15])) > 0.9); // hue ≈ 356
        assert!(q.key(lin([0.8, 0.15, 0.1])) > 0.9); // hue ≈ 4
    }
}
