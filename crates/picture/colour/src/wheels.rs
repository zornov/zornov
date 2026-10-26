// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Four-way colour wheels on the ASC CDL model, and a shadows / midtones /
//! highlights mode with soft tonal masks.
//!
//! Each wheel produces a colour push (hue angle in degrees in the hue plane
//! of [`crate::space::Opponent`], amount 0..1) and a luma amount −1..+1.
//! `d(h)` below is [`crate::space::hue_direction`]: a zero-mean RGB offset
//! of unit chroma towards hue `h`.
//!
//! # Lift / gamma / gain / offset → CDL
//!
//! The CDL runs on normalised grading values `n` (0 = black, 1 = diffuse
//! white), so its slope pivots at black and its power pivots at black and
//! diffuse white. Per channel `c`:
//!
//! ```text
//! lift   l_c = 0.25·lift.luma   + 0.20·lift.amount·d_c(lift.hue)
//! gain   g_c = 2^(gain.luma     + 0.75·gain.amount·d_c(gain.hue))
//! gamma  p_c = 2^−(0.75·gamma.luma + 0.50·gamma.amount·d_c(gamma.hue))
//! offset o_c = 0.25·offset.luma + 0.15·offset.amount·d_c(offset.hue)
//!
//! slope_c  = g_c − l_c          (black moves to l, white to g)
//! offset_c = l_c + o_c
//! power_c  = p_c                (positive gamma brightens midtones)
//! ```
//!
//! # Shadows / midtones / highlights
//!
//! Three masks of the pixel's normalised luma `t` (Rec. 709 weights):
//!
//! ```text
//! shadows    m_s = 1 − smoothstep(0.10, 0.55, t)
//! highlights m_h = smoothstep(0.45, 0.90, t)
//! midtones   m_m = 1 − m_s − m_h
//! ```
//!
//! They are smooth, non-negative and sum to one everywhere. Each wheel
//! adds `m_k · (0.20·luma_k + 0.15·amount_k·d_c(hue_k))` to channel `c`;
//! the offset wheel then applies as in four-way mode.
//!
//! Finally `intensity` (0..1) mixes the wheel result with its input.

use crate::cdl::Cdl;
use crate::math::{REC709_LUMA, Rgb, dot3, smoothstep};
use crate::space::hue_direction;

/// One grading wheel.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Wheel {
    /// Push direction in degrees (hue plane: red 0°, green 120°, blue 240°).
    pub hue: f32,
    /// Push strength, 0..1.
    pub amount: f32,
    /// Luma ring / slider, −1..+1.
    pub luma: f32,
}

impl Wheel {
    pub fn is_neutral(&self) -> bool {
        self.amount == 0.0 && self.luma == 0.0
    }

    /// `a·luma + b·amount·d(hue)` per channel.
    fn push(&self, a: f32, b: f32) -> Rgb {
        let d = hue_direction(self.hue);
        d.map(|dc| a * self.luma + b * self.amount * dc)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WheelMode {
    #[default]
    LiftGammaGain,
    ShadowsMidtonesHighlights,
}

/// Wheel settings. Only the wheels of the active mode (plus offset) are
/// used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheels {
    pub mode: WheelMode,
    pub lift: Wheel,
    pub gamma: Wheel,
    pub gain: Wheel,
    pub offset: Wheel,
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    /// 0..1 mix of the wheel result with its input.
    pub intensity: f32,
    /// An exact ASC CDL (slope, offset, power) from interchange (EDL,
    /// `.cdl` / `.cc` / `.ccc`, or `cdl_*` grade parameters). When set it
    /// is this stage's CDL: lift / gamma / gain / offset are not used.
    pub asc: Option<Cdl>,
}

impl Default for Wheels {
    fn default() -> Wheels {
        Wheels {
            mode: WheelMode::LiftGammaGain,
            lift: Wheel::default(),
            gamma: Wheel::default(),
            gain: Wheel::default(),
            offset: Wheel::default(),
            shadows: Wheel::default(),
            midtones: Wheel::default(),
            highlights: Wheel::default(),
            intensity: 1.0,
            asc: None,
        }
    }
}

/// Shadows / midtones / highlights masks at normalised luma `t`.
#[inline]
pub fn tonal_masks(t: f32) -> [f32; 3] {
    let s = 1.0 - smoothstep(0.10, 0.55, t);
    let h = smoothstep(0.45, 0.90, t);
    [s, 1.0 - s - h, h]
}

impl Wheels {
    pub fn is_identity(&self) -> bool {
        if let Some(c) = &self.asc {
            return self.intensity == 0.0 || (c.slope == [1.0; 3] && c.offset == [0.0; 3] && c.power == [1.0; 3] && self.mode == WheelMode::LiftGammaGain);
        }
        if self.intensity == 0.0 || !self.offset.is_neutral() {
            return self.intensity == 0.0;
        }
        match self.mode {
            WheelMode::LiftGammaGain => self.lift.is_neutral() && self.gamma.is_neutral() && self.gain.is_neutral(),
            WheelMode::ShadowsMidtonesHighlights => {
                self.shadows.is_neutral() && self.midtones.is_neutral() && self.highlights.is_neutral()
            }
        }
    }

    /// The CDL (in normalised grading units) for four-way mode. In
    /// shadows/midtones/highlights mode only the offset wheel contributes.
    pub fn to_cdl(&self) -> Cdl {
        if let Some(c) = self.asc {
            return Cdl { saturation: 1.0, ..c };
        }
        let o = self.offset.push(0.25, 0.15);
        if self.mode == WheelMode::ShadowsMidtonesHighlights {
            return Cdl { offset: o, ..Cdl::IDENTITY };
        }
        let l = self.lift.push(0.25, 0.20);
        let g = self.gain.push(1.0, 0.75).map(f32::exp2);
        let p = self.gamma.push(0.75, 0.50).map(|v| (-v).exp2());
        Cdl {
            slope: [g[0] - l[0], g[1] - l[1], g[2] - l[2]],
            offset: [l[0] + o[0], l[1] + o[1], l[2] + o[2]],
            power: p,
            saturation: 1.0,
        }
    }

    /// Precomputes everything needed per pixel.
    pub fn prepare(&self) -> PreparedWheels {
        PreparedWheels {
            identity: self.is_identity(),
            mode: self.mode,
            cdl: self.to_cdl(),
            smh: [self.shadows.push(0.20, 0.15), self.midtones.push(0.20, 0.15), self.highlights.push(0.20, 0.15)],
            intensity: self.intensity,
        }
    }
}

/// Wheels ready to apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedWheels {
    pub identity: bool,
    pub mode: WheelMode,
    pub cdl: Cdl,
    /// Shadows / midtones / highlights pushes.
    pub smh: [Rgb; 3],
    pub intensity: f32,
}

impl PreparedWheels {
    /// Applies the wheels to normalised grading values.
    #[inline]
    pub fn apply(&self, n: Rgb) -> Rgb {
        if self.identity {
            return n;
        }
        let mut out = n;
        if self.mode == WheelMode::ShadowsMidtonesHighlights {
            let m = tonal_masks(dot3(n, REC709_LUMA));
            for (c, v) in out.iter_mut().enumerate() {
                *v += m[0] * self.smh[0][c] + m[1] * self.smh[1][c] + m[2] * self.smh[2][c];
            }
        }
        out = self.cdl.apply_sop(out);
        if self.intensity != 1.0 {
            for (c, v) in out.iter_mut().enumerate() {
                *v = n[c] + self.intensity * (*v - n[c]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let w = Wheels::default();
        assert!(w.is_identity());
        assert!(w.to_cdl().is_identity());
        let p = w.prepare();
        assert_eq!(p.apply([0.3, -0.2, 1.4]), [0.3, -0.2, 1.4]);
    }

    #[test]
    fn exact_asc_cdl() {
        let asc = Cdl { slope: [1.2, 1.0, 0.9], offset: [0.01, 0.0, -0.02], power: [0.95, 1.0, 1.1], saturation: 0.9 };
        let w = Wheels { asc: Some(asc), lift: Wheel { luma: 0.5, ..Wheel::default() }, ..Wheels::default() };
        let p = w.prepare();
        let c = [0.4, 0.5, 0.6];
        let want = asc.apply_sop(c);
        assert_eq!(p.apply(c), want);
        assert!(Wheels { asc: Some(Cdl::IDENTITY), ..Wheels::default() }.is_identity());
    }

    #[test]
    fn lift_and_gain_pin_their_ends() {
        let w = Wheels {
            lift: Wheel { luma: 0.4, ..Wheel::default() },
            gain: Wheel { luma: 0.5, ..Wheel::default() },
            ..Wheels::default()
        };
        let p = w.prepare();
        let black = p.apply([0.0; 3]);
        let white = p.apply([1.0; 3]);
        assert!((black[0] - 0.1).abs() < 1e-6, "lift 0.4 → black at 0.1: {black:?}");
        assert!((white[0] - 0.5f32.exp2()).abs() < 1e-6, "gain 0.5 → white ×√2: {white:?}");
    }

    #[test]
    fn gamma_moves_mids_only() {
        let w = Wheels { gamma: Wheel { luma: 0.5, ..Wheel::default() }, ..Wheels::default() };
        let p = w.prepare();
        assert_eq!(p.apply([0.0; 3])[0], 0.0);
        assert!((p.apply([1.0; 3])[0] - 1.0).abs() < 1e-6);
        assert!(p.apply([0.5; 3])[0] > 0.5);
    }

    #[test]
    fn colour_push_direction() {
        let w = Wheels { gain: Wheel { hue: 0.0, amount: 1.0, luma: 0.0 }, ..Wheels::default() };
        let c = w.prepare().apply([0.8; 3]);
        assert!(c[0] > 0.8 && c[1] < 0.8 && c[2] < 0.8, "{c:?}");
        let w = Wheels { gain: Wheel { hue: 240.0, amount: 1.0, luma: 0.0 }, ..Wheels::default() };
        let c = w.prepare().apply([0.8; 3]);
        assert!(c[2] > c[0] && c[2] > c[1], "{c:?}");
    }

    #[test]
    fn masks_sum_to_one() {
        for i in -50..=150 {
            let t = i as f32 / 100.0;
            let m = tonal_masks(t);
            assert!(m.iter().all(|&v| (-1e-6..=1.0 + 1e-6).contains(&v)), "{t}: {m:?}");
            assert!((m[0] + m[1] + m[2] - 1.0).abs() < 1e-6);
        }
        assert!(tonal_masks(0.0)[0] == 1.0 && tonal_masks(1.0)[2] == 1.0);
        assert!(tonal_masks(0.5)[1] > 0.9);
    }

    #[test]
    fn smh_and_intensity() {
        let mut w = Wheels {
            mode: WheelMode::ShadowsMidtonesHighlights,
            shadows: Wheel { luma: 1.0, ..Wheel::default() },
            ..Wheels::default()
        };
        let p = w.prepare();
        let dark = p.apply([0.05; 3])[0] - 0.05;
        let bright = p.apply([0.95; 3])[0] - 0.95;
        assert!(dark > 0.19 && bright.abs() < 1e-3, "{dark} {bright}");
        w.intensity = 0.5;
        let half = w.prepare().apply([0.05; 3])[0] - 0.05;
        assert!((half - dark * 0.5).abs() < 1e-6);
    }
}
