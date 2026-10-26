// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A complete grade: every tool in one parameter block, applied in a fixed,
//! documented order.
//!
//! # Order
//!
//! 1. Un-premultiply.
//! 2. **Dehaze** (spatial, linear light — haze is a property of the scene).
//! 3. Per-pixel colour pass, [`GradeProcessor::apply_rgb`]:
//!    1. levels (auto levels result), linear
//!    2. match colour (Oklab), linear
//!    3. white balance (Bradford) and exposure, linear
//!    4. → grading space (AP1, ACEScct, normalised; see [`crate::space`])
//!    5. contrast, highlights / shadows / whites / blacks, fade
//!    6. colour wheels (CDL or shadows / midtones / highlights)
//!    7. curves (master, RGB, then hue / lum / sat curves)
//!    8. HSL bands
//!    9. saturation, vibrance
//!    10. → working linear
//!    11. LUT (with its own input space, intensity and skin protection)
//! 4. **Clarity**, then **sharpen** (spatial, luminance).
//! 5. **Vignette**, then **grain** (position-based; grain last so it is not
//!    shaped by the other tools).
//! 6. Re-premultiply.
//!
//! A stage whose controls are at their defaults is skipped entirely, and
//! a grade that is all defaults returns without touching the image, so the
//! identity is bit-exact.

use crate::auto::{ColourMatch, Levels, PreparedMatch};
use crate::basic::{Basic, apply_saturation, apply_tone};
use crate::curves::{Curves, PreparedCurves};
use crate::hsl::Hsl;
use crate::image::{Image, for_each_row, premultiply, unpremultiply};
use crate::lut::{LutSettings, PreparedLut};
use crate::math::{Mat3, Rgb, dot3};
use crate::primaries::{Primaries, white_balance_matrix};
use crate::space::GradingSpace;
use crate::spatial;
use crate::wheels::{PreparedWheels, Wheels};

/// All grading parameters of one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Grade {
    /// Primaries of the linear working space the image is in.
    pub working: Primaries,
    pub levels: Levels,
    pub colour_match: Option<ColourMatch>,
    pub basic: Basic,
    pub wheels: Wheels,
    pub curves: Curves,
    pub hsl: Hsl,
    pub lut: Option<LutSettings>,
}

impl Default for Grade {
    fn default() -> Grade {
        Grade {
            working: Primaries::BT709,
            levels: Levels::default(),
            colour_match: None,
            basic: Basic::default(),
            wheels: Wheels::default(),
            curves: Curves::default(),
            hsl: Hsl::default(),
            lut: None,
        }
    }
}

/// Per-frame inputs that are not grade parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FrameInfo {
    /// Frame number, used to animate grain.
    pub frame: u64,
}

impl Grade {
    pub fn is_identity(&self) -> bool {
        let b = &self.basic;
        self.prepare().colour_identity
            && b.dehaze == 0.0
            && b.clarity == 0.0
            && b.sharpen.amount == 0.0
            && b.vignette.amount == 0.0
            && b.grain.amount == 0.0
    }

    /// Compiles the per-pixel part of the grade.
    pub fn prepare(&self) -> GradeProcessor {
        let b = &self.basic;
        let mut wb = white_balance_matrix(&self.working, b.temperature, b.tint);
        if b.exposure != 0.0 {
            wb = Mat3::diag([b.exposure.exp2() as f64; 3]).mul(&wb);
        }
        let curves = self.curves.prepare();
        let wheels = self.wheels.prepare();
        let colour_match = self.colour_match.as_ref().map(|m| m.prepare(&self.working)).filter(|m| m.strength != 0.0);
        let lut = self.lut.as_ref().map(|l| l.prepare(&self.working)).filter(|l| l.intensity != 0.0);
        let grading = !b.tone_is_identity()
            || !wheels.identity
            || !curves.tone_identity
            || !curves.colour_identity
            || !self.hsl.is_identity()
            || !b.colour_is_identity();
        let colour_identity =
            self.levels.is_identity() && colour_match.is_none() && wb == Mat3::IDENTITY && !grading && lut.is_none();
        GradeProcessor {
            levels: self.levels,
            colour_match,
            linear: if wb == Mat3::IDENTITY { None } else { Some(wb) },
            space: GradingSpace::new(&self.working),
            grading,
            basic: *b,
            wheels,
            curves,
            hsl: self.hsl,
            lut,
            colour_identity,
            luma_weights: self.working.luminance_weights(),
        }
    }

    /// Applies the whole grade to `img` in place.
    pub fn apply(&self, img: &mut Image, info: FrameInfo) {
        if self.is_identity() {
            return;
        }
        let p = self.prepare();
        let b = &self.basic;
        let (w, h) = (img.width as usize, img.height as usize);
        let mut rgb: Vec<Rgb> = Vec::new();
        let spatial_needed = b.dehaze != 0.0 || b.clarity != 0.0 || b.sharpen.amount != 0.0;

        for_each_row(&mut img.pixels, w, |_, row| {
            for px in row.iter_mut() {
                *px = unpremultiply(*px);
            }
        });

        if spatial_needed {
            rgb = img.pixels.iter().map(|p| [p[0], p[1], p[2]]).collect();
            spatial::dehaze(&mut rgb, w, h, b.dehaze);
            for (d, s) in img.pixels.iter_mut().zip(&rgb) {
                d[0] = s[0];
                d[1] = s[1];
                d[2] = s[2];
            }
        }

        if !p.colour_identity {
            for_each_row(&mut img.pixels, w, |_, row| {
                for px in row.iter_mut() {
                    let c = p.apply_rgb([px[0], px[1], px[2]]);
                    *px = [c[0], c[1], c[2], px[3]];
                }
            });
        }

        if b.clarity != 0.0 || b.sharpen.amount != 0.0 {
            for (d, s) in rgb.iter_mut().zip(&img.pixels) {
                *d = [s[0], s[1], s[2]];
            }
            spatial::clarity(&mut rgb, w, h, p.luma_weights, b.clarity);
            spatial::sharpen(&mut rgb, w, h, p.luma_weights, &b.sharpen);
            for (d, s) in img.pixels.iter_mut().zip(&rgb) {
                d[0] = s[0];
                d[1] = s[1];
                d[2] = s[2];
            }
        }

        let vig = b.vignette;
        let grain = b.grain;
        let seed = spatial::grain_seed(&grain, info.frame);
        let lw = p.luma_weights;
        let (fw, fh) = (w as f32, h as f32);
        for_each_row(&mut img.pixels, w, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut k = 1.0;
                if vig.amount != 0.0 {
                    k *= spatial::vignette_gain(&vig, cx, cy, fw, fh);
                }
                if grain.amount != 0.0 {
                    let yl = dot3([px[0], px[1], px[2]], lw) * k;
                    k *= spatial::grain_gain(&grain, seed, cx, cy, yl);
                }
                if k != 1.0 {
                    px[0] *= k;
                    px[1] *= k;
                    px[2] *= k;
                }
                *px = premultiply(*px);
            }
        });
    }
}

/// The per-pixel colour part of a [`Grade`], compiled.
#[derive(Clone, Debug)]
pub struct GradeProcessor {
    pub levels: Levels,
    pub colour_match: Option<PreparedMatch>,
    /// White balance × exposure, if not the identity.
    pub linear: Option<Mat3>,
    pub space: GradingSpace,
    /// True if any grading-space stage is active.
    pub grading: bool,
    pub basic: Basic,
    pub wheels: PreparedWheels,
    pub curves: PreparedCurves,
    pub hsl: Hsl,
    pub lut: Option<PreparedLut>,
    pub colour_identity: bool,
    pub luma_weights: Rgb,
}

impl GradeProcessor {
    /// Grades one straight (un-premultiplied) working linear colour.
    pub fn apply_rgb(&self, c: Rgb) -> Rgb {
        if self.colour_identity {
            return c;
        }
        let mut c = c;
        if !self.levels.is_identity() {
            c = self.levels.apply(c);
        }
        if let Some(m) = &self.colour_match {
            c = m.apply(c);
        }
        if let Some(m) = &self.linear {
            c = m.apply(c);
        }
        if self.grading {
            let b = &self.basic;
            let mut n = self.space.to_grading(c);
            if !b.tone_is_identity() {
                n = n.map(|v| apply_tone(b, v));
            }
            n = self.wheels.apply(n);
            n = self.curves.apply(n);
            if !self.hsl.is_identity() {
                n = self.hsl.apply(n);
            }
            if !b.colour_is_identity() {
                n = apply_saturation(b, n);
            }
            c = self.space.from_grading(n);
        }
        if let Some(l) = &self.lut {
            c = l.apply(c);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Lcg;
    use crate::wheels::Wheel;

    fn noise_image(seed: u64) -> Image {
        let mut r = Lcg(seed);
        Image::from_fn(97, 61, |_, _| {
            let a = if r.next() < 0.1 { r.next() } else { 1.0 };
            [r.next() * 2.0 * a, r.next() * a, (r.next() - 0.05) * a, a]
        })
    }

    #[test]
    fn default_grade_is_bit_exact_identity() {
        let img = noise_image(1);
        let mut out = img.clone();
        Grade::default().apply(&mut out, FrameInfo { frame: 12 });
        assert!(img.pixels.iter().zip(&out.pixels).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
        // A zero-intensity LUT and zero-strength match are identities too.
        let g = Grade { wheels: Wheels { intensity: 0.0, lift: Wheel { luma: 1.0, ..Wheel::default() }, ..Wheels::default() }, ..Grade::default() };
        assert!(g.is_identity());
    }

    #[test]
    fn exposure_doubles_light() {
        let g = Grade { basic: Basic { exposure: 1.0, ..Basic::default() }, ..Grade::default() };
        let mut img = Image::from_pixels(1, 1, vec![[0.1, 0.2, 0.3, 0.5]]);
        g.apply(&mut img, FrameInfo::default());
        let p = img.pixels[0];
        for (i, e) in [0.2, 0.4, 0.6].iter().enumerate() {
            assert!((p[i] - e).abs() < 1e-6, "{p:?}");
        }
        assert_eq!(p[3], 0.5);
    }

    #[test]
    fn grading_space_round_trip_is_tight() {
        // A grade with a no-op-in-effect but active grading stage (HSL band
        // with zero chroma effect on greys) leaves greys unchanged.
        let mut g = Grade::default();
        g.hsl.bands[0].saturation = 0.5;
        let p = g.prepare();
        for v in [0.0, 0.01, 0.18, 1.0, 5.0] {
            let o = p.apply_rgb([v; 3]);
            for c in o {
                assert!((c - v).abs() <= 1e-5 * v.max(1.0), "{v} → {o:?}");
            }
        }
    }

    #[test]
    fn full_grade_runs_and_stays_finite() {
        let mut img = noise_image(3);
        let mut g = Grade::default();
        g.basic = Basic {
            exposure: 0.5,
            temperature: 4500.0,
            tint: 0.2,
            contrast: 0.3,
            highlights: -0.5,
            shadows: 0.4,
            whites: 0.2,
            blacks: -0.2,
            saturation: 1.2,
            vibrance: 0.3,
            clarity: 0.4,
            dehaze: 0.3,
            fade: 0.2,
            sharpen: crate::basic::Sharpen { amount: 0.5, radius: 1.0 },
            vignette: crate::basic::Vignette { amount: -0.5, ..Default::default() },
            grain: crate::basic::Grain { amount: 0.3, ..Default::default() },
        };
        g.wheels.gain = Wheel { hue: 30.0, amount: 0.3, luma: 0.1 };
        g.curves.master = vec![[0.0, 0.05], [0.5, 0.45], [1.0, 1.0]];
        g.curves.hue_vs_sat = vec![[0.33, -0.3]];
        g.hsl.bands[3].hue = 0.4;
        g.apply(&mut img, FrameInfo { frame: 5 });
        assert!(img.pixels.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }
}
