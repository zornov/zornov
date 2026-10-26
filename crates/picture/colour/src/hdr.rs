// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HDR measurement and display mapping.
//!
//! * [`HdrStats`]: the numbers an HDR deliverable is checked against —
//!   MaxCLL and MaxFALL (CTA-861.3, the brightest pixel and the brightest
//!   frame average, both from max(R, G, B) in Rec.2020), the luminance
//!   spread, and how much of the picture lies outside Rec.709 and P3.
//! * [`eetf`]: the ITU-R BT.2390 EETF, the standard roll-off that fits
//!   PQ material mastered for one peak onto a display with a lower one.
//! * [`nits_to_pq`] / [`pq_to_nits`] for HDR waveform scales.
//!
//! Studio's working linear has 1.0 = 203 cd/m² (BT.2408 graphics white)
//! whenever an HDR output is involved; [`NITS_PER_UNIT`] is that scale.

use crate::image::Image;
use crate::math::Mat3;
use crate::primaries::{Primaries, rgb_to_rgb};
use crate::transfer::{pq_eotf, pq_inverse_eotf};

/// cd/m² of working linear 1.0 in HDR.
pub const NITS_PER_UNIT: f32 = 203.0;

/// Luminance and gamut figures of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct HdrStats {
    /// Brightest max(R, G, B), cd/m² (the frame's contribution to MaxCLL).
    pub max_cll: f32,
    /// Mean of max(R, G, B) over the frame, cd/m² (its MaxFALL).
    pub frame_average: f32,
    /// Luminance percentiles, cd/m².
    pub median_nits: f32,
    pub p99_nits: f32,
    /// Share of pixels brighter than 203 cd/m² (above graphics white).
    pub above_203: f32,
    /// Share brighter than 1000 cd/m².
    pub above_1000: f32,
    /// Share of pixels outside the Rec.709 gamut (by more than 1 %).
    pub outside_709: f32,
    /// Share outside P3-D65.
    pub outside_p3: f32,
}

/// Measures a working-linear image (BT.709 primaries, 1.0 = 203 cd/m²).
/// Every `step`-th pixel in each direction is read (1 reads them all).
pub fn measure(img: &Image, step: u32) -> HdrStats {
    let step = step.max(1);
    let to2020 = rgb_to_rgb(&Primaries::BT709, &Primaries::BT2020).to_f32();
    let top3 = rgb_to_rgb(&Primaries::BT709, &Primaries::P3_D65).to_f32();
    let w = Primaries::BT2020.luminance_weights();
    let apply = |m: &[[f32; 3]; 3], c: [f32; 3]| [0, 1, 2].map(|r| m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]);
    let mut lum = Vec::new();
    let (mut n, mut sum_max, mut max_cll) = (0usize, 0f64, 0f32);
    let (mut a203, mut a1000, mut o709, mut op3) = (0usize, 0usize, 0usize, 0usize);
    for y in (0..img.height).step_by(step as usize) {
        for x in (0..img.width).step_by(step as usize) {
            let p = img.get(x, y);
            let a = p[3];
            let c = if a > 0.0 && a < 1.0 { [p[0] / a, p[1] / a, p[2] / a] } else { [p[0], p[1], p[2]] };
            let c2020 = apply(&to2020, c);
            let mx = c2020[0].max(c2020[1]).max(c2020[2]).max(0.0) * NITS_PER_UNIT;
            let l = (w[0] * c2020[0] + w[1] * c2020[1] + w[2] * c2020[2]).max(0.0) * NITS_PER_UNIT;
            max_cll = max_cll.max(mx);
            sum_max += mx as f64;
            lum.push(l);
            a203 += usize::from(l > 203.5);
            a1000 += usize::from(l > 1000.0);
            let tol = 0.01 * c.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-4);
            o709 += usize::from(c.iter().any(|v| *v < -tol));
            op3 += usize::from(apply(&top3, c).iter().any(|v| *v < -tol));
            n += 1;
        }
    }
    if n == 0 {
        return HdrStats::default();
    }
    lum.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |q: f32| lum[((lum.len() - 1) as f32 * q).round() as usize];
    let share = |k: usize| k as f32 / n as f32;
    HdrStats {
        max_cll,
        frame_average: (sum_max / n as f64) as f32,
        median_nits: pct(0.5),
        p99_nits: pct(0.99),
        above_203: share(a203),
        above_1000: share(a1000),
        outside_709: share(o709),
        outside_p3: share(op3),
    }
}

/// Running MaxCLL / MaxFALL over many frames.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LightLevels {
    pub max_cll: f32,
    pub max_fall: f32,
}

impl LightLevels {
    pub fn add(&mut self, s: &HdrStats) {
        self.max_cll = self.max_cll.max(s.max_cll);
        self.max_fall = self.max_fall.max(s.frame_average);
    }
}

/// cd/m² → PQ signal (0..1).
pub fn nits_to_pq(nits: f32) -> f32 {
    pq_inverse_eotf(nits)
}

/// PQ signal → cd/m².
pub fn pq_to_nits(pq: f32) -> f32 {
    pq_eotf(pq)
}

/// ITU-R BT.2390-10 EETF: maps a luminance (cd/m²) from a master with
/// `src_black..src_peak` onto a display with `dst_black..dst_peak`,
/// working on PQ values with a Hermite-spline knee. Identity when the
/// display covers the master.
pub fn eetf(nits: f32, src_black: f32, src_peak: f32, dst_black: f32, dst_peak: f32) -> f32 {
    if dst_peak >= src_peak && dst_black <= src_black {
        return nits;
    }
    let (sb, sw) = (nits_to_pq(src_black), nits_to_pq(src_peak));
    let e = (nits_to_pq(nits) - sb) / (sw - sb);
    let min_l = (nits_to_pq(dst_black) - sb) / (sw - sb);
    let max_l = (nits_to_pq(dst_peak) - sb) / (sw - sb);
    let ks = 1.5 * max_l - 0.5;
    let e2 = if e < ks || ks <= 0.0 && e < 0.0 {
        e
    } else {
        let t = (e - ks) / (1.0 - ks);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks) + (-2.0 * t3 + 3.0 * t2) * max_l
    };
    let e3 = if e2 >= 0.0 { e2 + min_l * (1.0 - e2).max(0.0).powi(4) } else { e2 };
    pq_to_nits(e3 * (sw - sb) + sb)
}

/// Applies [`eetf`] to the largest channel of working-linear pixels, so
/// hue is kept (the BT.2390 "maxRGB" variant).
pub fn eetf_image(img: &mut Image, src_peak: f32, dst_peak: f32) {
    for p in &mut img.pixels {
        let m = p[0].max(p[1]).max(p[2]);
        if m <= 0.0 {
            continue;
        }
        let nits = m * NITS_PER_UNIT;
        let k = eetf(nits, 0.0, src_peak, 0.0, dst_peak) / nits;
        p[0] *= k;
        p[1] *= k;
        p[2] *= k;
    }
}

/// Rec.709 → Rec.2020 matrix (for callers measuring their own pixels).
pub fn bt709_to_bt2020() -> Mat3 {
    rgb_to_rgb(&Primaries::BT709, &Primaries::BT2020)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eetf_keeps_low_and_caps_high() {
        // A 4000-nit master on a 1000-nit display.
        assert!((eetf(10.0, 0.0, 4000.0, 0.0, 1000.0) - 10.0).abs() < 0.2);
        assert!((eetf(100.0, 0.0, 4000.0, 0.0, 1000.0) - 100.0).abs() < 2.0);
        let top = eetf(4000.0, 0.0, 4000.0, 0.0, 1000.0);
        assert!((top - 1000.0).abs() < 5.0, "{top}");
        let mut last = 0.0;
        for i in 1..400 {
            let v = eetf(i as f32 * 10.0, 0.0, 4000.0, 0.0, 1000.0);
            assert!(v >= last && v <= 1000.5);
            last = v;
        }
        // Nothing to do when the display covers the master.
        assert_eq!(eetf(500.0, 0.0, 1000.0, 0.0, 1000.0), 500.0);
    }

    #[test]
    fn light_levels() {
        // Left half graphics white, right half 1000-nit highlights.
        let v = 1000.0 / 203.0;
        let img = Image::from_fn(10, 4, |x, _| if x < 5 { [1.0, 1.0, 1.0, 1.0] } else { [v, v, v, 1.0] });
        let s = measure(&img, 1);
        assert!((s.max_cll - 1000.0).abs() < 1.0, "{s:?}");
        assert!((s.frame_average - 601.5).abs() < 2.0, "{s:?}");
        assert!((s.above_203 - 0.5).abs() < 1e-6);
        assert_eq!(s.outside_709, 0.0);
        let mut ll = LightLevels::default();
        ll.add(&s);
        assert!(ll.max_fall > 600.0);
    }

    #[test]
    fn gamut_excursions() {
        // A pure Rec.2020 green, expressed in working (709) primaries.
        let g = rgb_to_rgb(&Primaries::BT2020, &Primaries::BT709).apply([0.0, 0.5, 0.0]);
        let img = Image::from_fn(2, 1, |x, _| if x == 0 { [g[0], g[1], g[2], 1.0] } else { [0.3, 0.3, 0.3, 1.0] });
        let s = measure(&img, 1);
        assert_eq!(s.outside_709, 0.5);
        assert_eq!(s.outside_p3, 0.5);
    }
}
