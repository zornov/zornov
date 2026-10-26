// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Basic corrections: exposure, white balance, contrast, tone ranges,
//! fade, saturation and vibrance (per pixel), plus the parameter blocks of
//! the spatial tools that live in [`crate::spatial`].
//!
//! # Tone-range weights
//!
//! Highlights, shadows, whites and blacks each add
//! `TONE_STRENGTH × amount × w(n)` to the normalised grading value `n`
//! (see [`crate::space`]; n = 0 is black, n = 1 diffuse white). The weights
//! are products of smoothsteps, so they are smooth, bounded by 1 and
//! overlap gently:
//!
//! | control    | weight w(n)                                              | peak      |
//! |------------|----------------------------------------------------------|-----------|
//! | blacks     | 1 − smoothstep(0, 0.4, n)                                 | n ≤ 0     |
//! | shadows    | smoothstep(−0.05, 0.25, n) · (1 − smoothstep(0.25, 0.6, n)) | n = 0.25 |
//! | highlights | smoothstep(0.4, 0.75, n) · (1 − smoothstep(0.75, 1.1, n))   | n = 0.75 |
//! | whites     | smoothstep(0.6, 1.0, n)                                   | n ≥ 1     |
//!
//! `TONE_STRENGTH` = 0.12 moves a tone by at most ≈ 1 stop. It is chosen
//! so that the steepest falling weight slope times the strength stays
//! below 1 even when two neighbouring controls push in opposite
//! directions, so the tone curve never inverts (tested).
//!
//! # Fade
//!
//! `n' = n + fade × FADE_LIFT × (1 − clamp(n, 0, 1))²` lifts black towards
//! a grey (n = 0.25 at full fade) while leaving white in place. Its slope
//! is at least 1 − 2·FADE_LIFT = 0.5, so it is always monotone.
//!
//! # Vibrance
//!
//! Chroma `c` in the hue plane is scaled by
//! `1 + vibrance × (1 − s)² × (1 − 0.75 × skin)`, where
//! `s = clamp(c / 0.35, 0, 1)` and `skin` is a soft weight around hue 25°
//! (the range of typical skin tones in the hue plane), so muted colours
//! gain most and faces are protected.

use crate::math::{REC709_LUMA, Rgb, dot3, smoothstep};
use crate::space::{N_GREY, Opponent};

/// Strength of the four tone-range controls, in normalised units.
pub const TONE_STRENGTH: f32 = 0.12;
/// Black lift of fade at 1.0, in normalised units.
pub const FADE_LIFT: f32 = 0.25;

/// Unsharp-mask sharpening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sharpen {
    /// 0..1.
    pub amount: f32,
    /// Blur radius (Gaussian sigma) in pixels, 0.5..3.
    pub radius: f32,
}

impl Default for Sharpen {
    fn default() -> Sharpen {
        Sharpen { amount: 0.0, radius: 1.0 }
    }
}

/// Lens-style vignette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignette {
    /// −1 (dark edges, −2 stops) .. +1 (bright edges, +2 stops).
    pub amount: f32,
    /// Where the falloff starts, 0 (near the centre) .. 1 (near the corners).
    pub midpoint: f32,
    /// 0 follows the frame's aspect ratio (an ellipse), 1 is a circle.
    pub roundness: f32,
    /// Width of the falloff, 0 (hard) .. 1 (very soft).
    pub feather: f32,
    /// 0 is elliptical, 1 approaches a rounded rectangle.
    pub shape: f32,
}

impl Default for Vignette {
    fn default() -> Vignette {
        Vignette { amount: 0.0, midpoint: 0.5, roundness: 0.0, feather: 0.5, shape: 0.0 }
    }
}

/// Film-like grain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grain {
    /// 0..1.
    pub amount: f32,
    /// Grain size in pixels, 0.5..4.
    pub size: f32,
    /// 0 is smooth single-scale grain, 1 adds a fine, rough layer.
    pub roughness: f32,
    /// Base seed; the frame number is mixed in so grain moves every frame.
    pub seed: u32,
}

impl Default for Grain {
    fn default() -> Grain {
        Grain { amount: 0.0, size: 1.0, roughness: 0.5, seed: 0 }
    }
}

/// The basic correction controls. Defaults are the identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basic {
    /// Stops, −4..+4.
    pub exposure: f32,
    /// Kelvin of the light the footage was shot under, 2000..15000.
    /// [`crate::primaries::NEUTRAL_KELVIN`] is neutral.
    pub temperature: f32,
    /// −1 (greener) .. +1 (more magenta).
    pub tint: f32,
    /// −1..+1; scales distance from the 18 % grey pivot by 2^contrast.
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    /// 0..2, 1 is unchanged.
    pub saturation: f32,
    /// −1..+1.
    pub vibrance: f32,
    /// Local contrast, −1..+1.
    pub clarity: f32,
    /// −1 (add haze) .. +1 (remove haze).
    pub dehaze: f32,
    /// 0..1.
    pub fade: f32,
    pub sharpen: Sharpen,
    pub vignette: Vignette,
    pub grain: Grain,
}

impl Default for Basic {
    fn default() -> Basic {
        Basic {
            exposure: 0.0,
            temperature: crate::primaries::NEUTRAL_KELVIN,
            tint: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            saturation: 1.0,
            vibrance: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            fade: 0.0,
            sharpen: Sharpen::default(),
            vignette: Vignette::default(),
            grain: Grain::default(),
        }
    }
}

impl Basic {
    pub fn tone_is_identity(&self) -> bool {
        self.contrast == 0.0
            && self.highlights == 0.0
            && self.shadows == 0.0
            && self.whites == 0.0
            && self.blacks == 0.0
            && self.fade == 0.0
    }

    pub fn colour_is_identity(&self) -> bool {
        self.saturation == 1.0 && self.vibrance == 0.0
    }
}

/// The four tone-range weights at normalised value `n`:
/// `[blacks, shadows, highlights, whites]`.
#[inline]
pub fn tone_weights(n: f32) -> [f32; 4] {
    [
        1.0 - smoothstep(0.0, 0.4, n),
        smoothstep(-0.05, 0.25, n) * (1.0 - smoothstep(0.25, 0.6, n)),
        smoothstep(0.4, 0.75, n) * (1.0 - smoothstep(0.75, 1.1, n)),
        smoothstep(0.6, 1.0, n),
    ]
}

/// Contrast, tone ranges and fade on one normalised channel value.
#[inline]
pub fn apply_tone(b: &Basic, n: f32) -> f32 {
    let mut v = n;
    if b.contrast != 0.0 {
        v = N_GREY + (v - N_GREY) * b.contrast.exp2();
    }
    if b.highlights != 0.0 || b.shadows != 0.0 || b.whites != 0.0 || b.blacks != 0.0 {
        let w = tone_weights(v);
        v += TONE_STRENGTH * (b.blacks * w[0] + b.shadows * w[1] + b.highlights * w[2] + b.whites * w[3]);
    }
    if b.fade != 0.0 {
        let d = 1.0 - v.clamp(0.0, 1.0);
        v += b.fade * FADE_LIFT * d * d;
    }
    v
}

/// Soft weight of skin-like hues in the hue plane (centred on 25°, where
/// typical skin tones fall in the grading space).
#[inline]
pub fn skin_hue_weight(hue_deg: f32) -> f32 {
    let mut d = (hue_deg - 25.0).abs();
    if d > 180.0 {
        d = 360.0 - d;
    }
    1.0 - smoothstep(10.0, 40.0, d)
}

/// Saturation (Rec. 709 luma weights) then vibrance, on normalised values.
#[inline]
pub fn apply_saturation(b: &Basic, c: Rgb) -> Rgb {
    let mut c = c;
    if b.saturation != 1.0 {
        let l = dot3(c, REC709_LUMA);
        c = c.map(|v| l + b.saturation * (v - l));
    }
    if b.vibrance != 0.0 {
        let o = Opponent::from_rgb(c);
        let s = (o.chroma() / 0.35).clamp(0.0, 1.0);
        let skin = skin_hue_weight(o.hue_degrees()) * smoothstep(0.02, 0.08, o.chroma());
        let k = 1.0 + b.vibrance * (1.0 - s) * (1.0 - s) * (1.0 - 0.75 * skin);
        c = o.scale_chroma(k.max(0.0)).to_rgb();
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_identity() {
        let b = Basic::default();
        assert!(b.tone_is_identity() && b.colour_is_identity());
        for i in 0..100 {
            let n = i as f32 / 50.0 - 0.5;
            assert_eq!(apply_tone(&b, n), n);
        }
    }

    #[test]
    fn contrast_pivots_at_grey() {
        let b = Basic { contrast: 0.7, ..Basic::default() };
        assert!((apply_tone(&b, N_GREY) - N_GREY).abs() < 1e-6);
        assert!(apply_tone(&b, 0.9) > 0.9 && apply_tone(&b, 0.3) < 0.3);
    }

    #[test]
    fn tone_controls_never_invert() {
        // Every combination of ±1 for the four controls, plus fade and
        // contrast extremes, must give a non-decreasing tone curve.
        for mask in 0..81u32 {
            let mut m = mask;
            let mut a = [0.0f32; 4];
            for v in a.iter_mut() {
                *v = (m % 3) as f32 - 1.0;
                m /= 3;
            }
            for &fade in &[0.0, 1.0] {
                for &contrast in &[-1.0, 0.0, 1.0] {
                    let b = Basic {
                        blacks: a[0],
                        shadows: a[1],
                        highlights: a[2],
                        whites: a[3],
                        fade,
                        contrast,
                        ..Basic::default()
                    };
                    let mut prev = f32::MIN;
                    for i in 0..=2000 {
                        let n = -0.3 + i as f32 * 0.001;
                        let v = apply_tone(&b, n);
                        assert!(v >= prev, "inverted at n={n} for {a:?} fade {fade} contrast {contrast}");
                        prev = v;
                    }
                }
            }
        }
    }

    #[test]
    fn tone_ranges_hit_their_tones() {
        let up = |f: fn(&mut Basic), n: f32| {
            let mut b = Basic::default();
            f(&mut b);
            apply_tone(&b, n) - n
        };
        assert!(up(|b| b.shadows = 1.0, 0.25) > up(|b| b.shadows = 1.0, 0.9));
        assert!(up(|b| b.highlights = 1.0, 0.75) > up(|b| b.highlights = 1.0, 0.2));
        assert!(up(|b| b.blacks = 1.0, 0.0) > 0.11);
        assert!(up(|b| b.whites = 1.0, 1.2) > 0.11);
        assert!((up(|b| b.fade = 1.0, 0.0) - FADE_LIFT).abs() < 1e-6);
        assert_eq!(up(|b| b.fade = 1.0, 1.0), 0.0);
    }

    #[test]
    fn vibrance_prefers_muted_colours() {
        let b = Basic { vibrance: 1.0, ..Basic::default() };
        let gain = |c: Rgb| Opponent::from_rgb(apply_saturation(&b, c)).chroma() / Opponent::from_rgb(c).chroma();
        let muted = gain([0.55, 0.5, 0.6]);
        let vivid = gain([0.9, 0.2, 0.6]);
        assert!(muted > vivid && vivid >= 1.0);
        // Skin-like hue is protected compared with the same chroma elsewhere.
        let skin = gain([0.62, 0.55, 0.5]);
        let teal = gain([0.5, 0.57, 0.62]);
        assert!(skin < teal, "{skin} {teal}");
    }
}
