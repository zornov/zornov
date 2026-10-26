// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Curves: master and per-channel tone curves, plus the hue / luminance /
//! saturation curves.
//!
//! All curves work on normalised grading values (see [`crate::space`]).
//! Point coordinates:
//!
//! | curve      | x                                  | y (default)                                      |
//! |------------|------------------------------------|--------------------------------------------------|
//! | master/RGB | input n (0 black, 1 diffuse white) | output n — the identity line `(0,0)-(1,1)`       |
//! | hue vs hue | hue as a fraction of a turn (0..1) | hue shift in turns, −0.5..0.5 (0)                |
//! | hue vs sat | hue (0..1)                         | saturation change −1..1, ×(1+y) (0)              |
//! | hue vs lum | hue (0..1)                         | lightness change −1..1, ±`HUE_LUM_SCALE` n (0)   |
//! | lum vs sat | luma n (0..1)                      | saturation change −1..1, ×(1+y) (0)              |
//! | sat vs sat | chroma / `SAT_AXIS_MAX` (0..1)     | saturation change −1..1, ×(1+y) (0)              |
//!
//! Hue is the angle in the hue plane ([`crate::space::Opponent`]), so 0 is
//! red, 1/6 yellow, 1/3 green, 1/2 cyan, 2/3 blue, 5/6 magenta. An empty
//! point list means "no change".

use crate::math::{REC709_LUMA, Rgb, dot3, smoothstep};
use crate::space::Opponent;
use crate::spline::Spline;

/// Lightness change of the hue-vs-lum curve at y = ±1, in normalised units.
pub const HUE_LUM_SCALE: f32 = 0.25;
/// Chroma that maps to x = 1 on the sat-vs-sat axis.
pub const SAT_AXIS_MAX: f32 = 0.6;

/// Curve points as edited by the user (at most 16 per curve).
#[derive(Clone, Debug, PartialEq)]
pub struct Curves {
    pub master: Vec<[f32; 2]>,
    pub red: Vec<[f32; 2]>,
    pub green: Vec<[f32; 2]>,
    pub blue: Vec<[f32; 2]>,
    pub hue_vs_hue: Vec<[f32; 2]>,
    pub hue_vs_sat: Vec<[f32; 2]>,
    pub hue_vs_lum: Vec<[f32; 2]>,
    pub lum_vs_sat: Vec<[f32; 2]>,
    pub sat_vs_sat: Vec<[f32; 2]>,
}

const IDENTITY_POINTS: [[f32; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];

impl Default for Curves {
    fn default() -> Curves {
        Curves {
            master: IDENTITY_POINTS.to_vec(),
            red: IDENTITY_POINTS.to_vec(),
            green: IDENTITY_POINTS.to_vec(),
            blue: IDENTITY_POINTS.to_vec(),
            hue_vs_hue: Vec::new(),
            hue_vs_sat: Vec::new(),
            hue_vs_lum: Vec::new(),
            lum_vs_sat: Vec::new(),
            sat_vs_sat: Vec::new(),
        }
    }
}

impl Curves {
    pub fn prepare(&self) -> PreparedCurves {
        let tone = [
            Spline::monotone(&self.master),
            Spline::monotone(&self.red),
            Spline::monotone(&self.green),
            Spline::monotone(&self.blue),
        ];
        let hue = [
            Spline::periodic(&self.hue_vs_hue),
            Spline::periodic(&self.hue_vs_sat),
            Spline::periodic(&self.hue_vs_lum),
        ];
        let lum_sat = Spline::shape_preserving(&self.lum_vs_sat);
        let sat_sat = Spline::shape_preserving(&self.sat_vs_sat);
        let tone_identity = tone.iter().all(Spline::is_identity);
        let colour_identity = hue.iter().all(Spline::is_zero) && lum_sat.is_zero() && sat_sat.is_zero();
        PreparedCurves { tone, hue, lum_sat, sat_sat, tone_identity, colour_identity }
    }

    pub fn is_identity(&self) -> bool {
        let p = self.prepare();
        p.tone_identity && p.colour_identity
    }
}

/// Curves compiled into splines.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedCurves {
    /// Master, red, green, blue.
    pub tone: [Spline; 4],
    /// Hue vs hue, hue vs sat, hue vs lum.
    pub hue: [Spline; 3],
    pub lum_sat: Spline,
    pub sat_sat: Spline,
    pub tone_identity: bool,
    pub colour_identity: bool,
}

impl PreparedCurves {
    #[inline]
    pub fn apply(&self, n: Rgb) -> Rgb {
        let mut c = n;
        if !self.tone_identity {
            for (i, v) in c.iter_mut().enumerate() {
                *v = self.tone[i + 1].eval(self.tone[0].eval(*v));
            }
        }
        if !self.colour_identity {
            let luma = dot3(c, REC709_LUMA);
            let mut o = Opponent::from_rgb(c);
            let chroma = o.chroma();
            // Hue is meaningless on greys, so fade hue-driven changes in
            // over a small chroma range to avoid noise turning colourful.
            let hue_w = smoothstep(0.0, 0.05, chroma);
            let h = o.hue_degrees() / 360.0;
            let shift = self.hue[0].eval(h) * hue_w;
            if shift != 0.0 {
                o = o.rotate(shift * 360.0);
            }
            let sat = (1.0 + self.hue[1].eval(h) * hue_w)
                * (1.0 + self.lum_sat.eval(luma.clamp(0.0, 1.0)))
                * (1.0 + self.sat_sat.eval((chroma / SAT_AXIS_MAX).clamp(0.0, 1.0)));
            o = o.scale_chroma(sat.max(0.0));
            o.m += self.hue[2].eval(h) * HUE_LUM_SCALE * hue_w;
            c = o.to_rgb();
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_identity() {
        let c = Curves::default();
        assert!(c.is_identity());
        let p = c.prepare();
        assert_eq!(p.apply([0.3, 1.7, -0.1]), [0.3, 1.7, -0.1]);
    }

    #[test]
    fn master_then_channel() {
        let c = Curves { master: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]], ..Curves::default() };
        let o = c.prepare().apply([0.5, 0.5, 0.5]);
        assert!((o[0] - 0.6).abs() < 1e-6 && (o[2] - 0.6).abs() < 1e-6);
        let c = Curves { red: vec![[0.0, 0.0], [1.0, 0.5]], ..Curves::default() };
        let o = c.prepare().apply([0.8, 0.8, 0.8]);
        assert!((o[0] - 0.4).abs() < 1e-6 && o[1] == 0.8);
    }

    #[test]
    fn hue_curves_target_their_hue() {
        // Desaturate greens only.
        let c = Curves { hue_vs_sat: vec![[1.0 / 3.0, -1.0], [0.0, 0.0], [2.0 / 3.0, 0.0]], ..Curves::default() };
        let p = c.prepare();
        let green = [0.3, 0.7, 0.3];
        let red = [0.7, 0.3, 0.3];
        assert!(Opponent::from_rgb(p.apply(green)).chroma() < 1e-4);
        let r = p.apply(red);
        assert!((r[0] - 0.7).abs() < 1e-5, "{r:?}");
        // Grey is untouched by hue curves.
        let g = p.apply([0.5; 3]);
        assert!((g[0] - 0.5).abs() < 1e-6 && (g[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hue_vs_hue_rotates() {
        let c = Curves { hue_vs_hue: vec![[0.0, 1.0 / 6.0]], ..Curves::default() };
        let o = c.prepare().apply([0.8, 0.2, 0.2]);
        assert!((Opponent::from_rgb(o).hue_degrees() - 60.0).abs() < 0.1);
    }
}
