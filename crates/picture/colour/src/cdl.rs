// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ASC Color Decision List (CDL) v1.2.
//!
//! Per channel `out = (in × slope + offset) ^ power`, followed by a
//! saturation operator using Rec. 709 luma weights:
//! `out = luma + sat × (in − luma)`.

use crate::math::{REC709_LUMA, Rgb, dot3};

/// The ten numbers of an ASC CDL.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cdl {
    pub slope: Rgb,
    pub offset: Rgb,
    pub power: Rgb,
    pub saturation: f32,
}

impl Default for Cdl {
    fn default() -> Cdl {
        Cdl::IDENTITY
    }
}

impl Cdl {
    pub const IDENTITY: Cdl = Cdl { slope: [1.0; 3], offset: [0.0; 3], power: [1.0; 3], saturation: 1.0 };

    pub fn is_identity(&self) -> bool {
        *self == Cdl::IDENTITY
    }

    /// Slope, offset and power only.
    ///
    /// A fractional power of a negative number is undefined, so the value is
    /// clamped at 0 before the power — but only for channels whose power is
    /// not 1, so a power of 1 passes negatives through untouched (as the
    /// "no-clamp" form used in floating-point pipelines does).
    #[inline]
    pub fn apply_sop(&self, c: Rgb) -> Rgb {
        let mut o = [0.0; 3];
        for i in 0..3 {
            let v = c[i] * self.slope[i] + self.offset[i];
            o[i] = if self.power[i] == 1.0 { v } else { v.max(0.0).powf(self.power[i]) };
        }
        o
    }

    /// The saturation step on its own.
    #[inline]
    pub fn apply_sat(&self, c: Rgb) -> Rgb {
        if self.saturation == 1.0 {
            return c;
        }
        let l = dot3(c, REC709_LUMA);
        c.map(|v| l + self.saturation * (v - l))
    }

    /// Full CDL without range clamping (floating-point pipelines).
    #[inline]
    pub fn apply(&self, c: Rgb) -> Rgb {
        self.apply_sat(self.apply_sop(c))
    }

    /// Full CDL with the v1.2 reference clamping of each stage to 0..1, for
    /// display-referred material and interchange checks.
    pub fn apply_clamped(&self, c: Rgb) -> Rgb {
        let mut o = [0.0; 3];
        for i in 0..3 {
            let v = (c[i] * self.slope[i] + self.offset[i]).clamp(0.0, 1.0);
            o[i] = v.powf(self.power[i]);
        }
        self.apply_sat(o).map(|v| v.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close3(a: Rgb, b: Rgb, eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= eps)
    }

    #[test]
    fn identity() {
        let c = [0.25, -0.1, 3.0];
        assert_eq!(Cdl::IDENTITY.apply(c), c);
    }

    // Reference values evaluated from the equations of the ASC CDL v1.2
    // specification in double precision (see README for derivation).
    #[test]
    fn asc_equations() {
        let cdl = Cdl { slope: [1.2, 1.0, 0.8], offset: [0.05, -0.02, 0.1], power: [1.1, 0.9, 1.0], saturation: 0.8 };
        let sop = cdl.apply_sop([0.5, 0.25, 0.75]);
        assert!(close3(sop, [0.622_593_7, 0.266_412_8, 0.7], 2e-6), "{sop:?}");
        let out = cdl.apply([0.5, 0.25, 0.75]);
        assert!(close3(out, [0.572_763_3, 0.287_818_6, 0.634_688_4], 2e-6), "{out:?}");

        // A grey stays grey under any saturation.
        let g = Cdl { saturation: 0.0, ..Cdl::IDENTITY }.apply([0.4; 3]);
        assert!(close3(g, [0.4; 3], 1e-7));

        // Saturation 0 collapses to Rec. 709 luma.
        let l = Cdl { saturation: 0.0, ..Cdl::IDENTITY }.apply([1.0, 0.0, 0.0]);
        assert!(close3(l, [0.2126; 3], 1e-7));

        // Clamped form: offset pushes past 1, power of a clamped 1 stays 1.
        let c = Cdl { slope: [2.0; 3], offset: [0.1; 3], power: [2.0; 3], saturation: 1.0 };
        assert!(close3(c.apply_clamped([0.6, 0.2, -0.3]), [1.0, 0.25, 0.0], 1e-6));
    }
}
