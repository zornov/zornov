// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Biquad filters (Robert Bristow-Johnson's "Audio EQ Cookbook" designs) and
//! the five-band parametric equaliser built from them.
//!
//! Coefficients are computed in f64 and the filter runs in the transposed
//! direct form II, which keeps low-frequency shelves and high-pass filters
//! well behaved in f32.

use crate::buffer::Effect;

/// Filter shape of one EQ band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BandKind {
    /// Peaking bell; `gain_db` is the boost or cut at the centre.
    #[default]
    Bell,
    /// Low shelf; `gain_db` applies below `freq`.
    LowShelf,
    /// High shelf; `gain_db` applies above `freq`.
    HighShelf,
    /// 12 dB/oct high-pass (gain ignored).
    HighPass,
    /// 12 dB/oct low-pass (gain ignored).
    LowPass,
    /// Notch at `freq` (gain ignored).
    Notch,
    /// Constant-0 dB-peak band-pass (gain ignored).
    BandPass,
}

/// Normalised biquad coefficients (`a0` folded in).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    /// Feed-forward.
    pub b0: f32,
    /// Feed-forward.
    pub b1: f32,
    /// Feed-forward.
    pub b2: f32,
    /// Feedback.
    pub a1: f32,
    /// Feedback.
    pub a2: f32,
}

impl Coeffs {
    /// The pass-through filter.
    pub const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// RBJ cookbook coefficients. `q` is the quality factor (bells, notches,
    /// band-passes, HP/LP; 0.707 is Butterworth) and also sets shelf slope.
    pub fn design(kind: BandKind, rate: u32, freq: f32, q: f32, gain_db: f32) -> Coeffs {
        let fs = rate.max(1) as f64;
        let f0 = (freq as f64).clamp(1.0, fs * 0.499);
        let q = (q as f64).max(0.025);
        let w0 = std::f64::consts::TAU * f0 / fs;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            BandKind::Bell => (1.0 + alpha * a, -2.0 * cs, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cs, 1.0 - alpha / a),
            BandKind::LowShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + sa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - sa),
                    (a + 1.0) + (a - 1.0) * cs + sa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - sa,
                )
            }
            BandKind::HighShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + sa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - sa),
                    (a + 1.0) - (a - 1.0) * cs + sa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - sa,
                )
            }
            BandKind::HighPass => ((1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BandKind::LowPass => ((1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BandKind::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BandKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        };
        Coeffs { b0: (b0 / a0) as f32, b1: (b1 / a0) as f32, b2: (b2 / a0) as f32, a1: (a1 / a0) as f32, a2: (a2 / a0) as f32 }
    }

    /// Magnitude response in dB at `freq` for sample rate `rate`.
    pub fn response_db(&self, rate: u32, freq: f32) -> f32 {
        let w = std::f64::consts::TAU * freq as f64 / rate.max(1) as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let num_re = self.b0 as f64 + self.b1 as f64 * c1 + self.b2 as f64 * c2;
        let num_im = -(self.b1 as f64 * s1 + self.b2 as f64 * s2);
        let den_re = 1.0 + self.a1 as f64 * c1 + self.a2 as f64 * c2;
        let den_im = -(self.a1 as f64 * s1 + self.a2 as f64 * s2);
        let mag = ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).sqrt();
        (20.0 * mag.max(1e-12).log10()) as f32
    }
}

/// A stereo biquad (two independent state pairs sharing coefficients).
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    /// Current coefficients.
    pub coeffs: Coeffs,
    z: [[f32; 2]; 2],
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(Coeffs::IDENTITY)
    }
}

impl Biquad {
    /// A biquad with the given coefficients.
    pub fn new(coeffs: Coeffs) -> Self {
        Self { coeffs, z: [[0.0; 2]; 2] }
    }

    /// Designs and wraps a filter in one step.
    pub fn design(kind: BandKind, rate: u32, freq: f32, q: f32, gain_db: f32) -> Self {
        Self::new(Coeffs::design(kind, rate, freq, q, gain_db))
    }

    /// Filters one channel in place.
    #[inline]
    pub fn process_channel(&mut self, ch: usize, x: &mut [f32]) {
        let c = self.coeffs;
        let [mut z1, mut z2] = self.z[ch];
        for v in x.iter_mut() {
            let input = *v;
            let y = c.b0 * input + z1;
            z1 = c.b1 * input - c.a1 * y + z2;
            z2 = c.b2 * input - c.a2 * y;
            *v = y;
        }
        // Flush denormals that would otherwise slow the loop for silent input.
        if z1.abs() < 1e-20 {
            z1 = 0.0;
        }
        if z2.abs() < 1e-20 {
            z2 = 0.0;
        }
        self.z[ch] = [z1, z2];
    }

    /// Filters one sample of one channel (for per-sample modulation loops).
    #[inline]
    pub fn tick(&mut self, ch: usize, input: f32) -> f32 {
        let c = self.coeffs;
        let y = c.b0 * input + self.z[ch][0];
        self.z[ch][0] = c.b1 * input - c.a1 * y + self.z[ch][1];
        self.z[ch][1] = c.b2 * input - c.a2 * y;
        y
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.z = [[0.0; 2]; 2];
    }
}

impl Effect for Biquad {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process_channel(0, left);
        self.process_channel(1, right);
    }
    fn reset(&mut self) {
        Biquad::reset(self)
    }
}

/// One band of the parametric EQ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// Whether the band is in the signal path.
    pub enabled: bool,
    /// Filter shape.
    pub kind: BandKind,
    /// Centre or corner frequency in Hz (20–20000).
    pub freq: f32,
    /// Quality factor (0.1–18; 0.707 is Butterworth for HP/LP).
    pub q: f32,
    /// Boost or cut in dB (−24..+24) for bells and shelves.
    pub gain_db: f32,
}

impl Band {
    /// A flat (0 dB) bell at `freq`.
    pub const fn flat(freq: f32) -> Self {
        Self { enabled: true, kind: BandKind::Bell, freq, q: 1.0, gain_db: 0.0 }
    }

    /// A bell band.
    pub const fn bell(freq: f32, q: f32, gain_db: f32) -> Self {
        Self { enabled: true, kind: BandKind::Bell, freq, q, gain_db }
    }

    /// Any band kind.
    pub const fn new(kind: BandKind, freq: f32, q: f32, gain_db: f32) -> Self {
        Self { enabled: true, kind, freq, q, gain_db }
    }

    fn coeffs(&self, rate: u32) -> Coeffs {
        let g = self.gain_db.clamp(-24.0, 24.0);
        Coeffs::design(self.kind, rate, self.freq.clamp(10.0, 24_000.0), self.q.clamp(0.1, 18.0), g)
    }
}

/// Number of bands in [`ParametricEq`].
pub const EQ_BANDS: usize = 5;

/// The five-band parametric equaliser with an optional high-pass and
/// low-pass on either side. Default bands: low shelf 100 Hz, bells at 300 Hz,
/// 1 kHz and 3 kHz, high shelf 8 kHz, all at 0 dB.
pub struct ParametricEq {
    rate: u32,
    bands: [Band; EQ_BANDS],
    filters: [Biquad; EQ_BANDS],
    high_pass: Option<(f32, Biquad)>,
    low_pass: Option<(f32, Biquad)>,
    /// Output trim in dB.
    pub output_db: f32,
}

impl ParametricEq {
    /// A flat EQ at `rate`.
    pub fn new(rate: u32) -> Self {
        let bands = [
            Band::new(BandKind::LowShelf, 100.0, 0.707, 0.0),
            Band::flat(300.0),
            Band::flat(1000.0),
            Band::flat(3000.0),
            Band::new(BandKind::HighShelf, 8000.0, 0.707, 0.0),
        ];
        let mut eq = Self { rate, bands, filters: [Biquad::default(); EQ_BANDS], high_pass: None, low_pass: None, output_db: 0.0 };
        for i in 0..EQ_BANDS {
            eq.refresh(i);
        }
        eq
    }

    /// The bands.
    pub fn bands(&self) -> &[Band; EQ_BANDS] {
        &self.bands
    }

    /// Replaces band `i` (0..5).
    pub fn set_band(&mut self, i: usize, band: Band) {
        if i < EQ_BANDS {
            self.bands[i] = band;
            self.refresh(i);
        }
    }

    fn refresh(&mut self, i: usize) {
        self.filters[i].coeffs = self.bands[i].coeffs(self.rate);
    }

    /// Sets (or with `None` removes) the high-pass corner frequency.
    pub fn set_high_pass(&mut self, freq: Option<f32>) {
        self.high_pass = freq.map(|f| {
            let c = Coeffs::design(BandKind::HighPass, self.rate, f, 0.707, 0.0);
            let mut b = self.high_pass.map(|(_, b)| b).unwrap_or_default();
            b.coeffs = c;
            (f, b)
        });
    }

    /// Sets (or removes) the low-pass corner frequency.
    pub fn set_low_pass(&mut self, freq: Option<f32>) {
        self.low_pass = freq.map(|f| {
            let c = Coeffs::design(BandKind::LowPass, self.rate, f, 0.707, 0.0);
            let mut b = self.low_pass.map(|(_, b)| b).unwrap_or_default();
            b.coeffs = c;
            (f, b)
        });
    }

    /// Combined magnitude response in dB at `freq` (for the inspector curve).
    pub fn response_db(&self, freq: f32) -> f32 {
        let mut db = self.output_db;
        for (b, f) in self.bands.iter().zip(&self.filters) {
            if b.enabled {
                db += f.coeffs.response_db(self.rate, freq);
            }
        }
        if let Some((_, f)) = &self.high_pass {
            db += f.coeffs.response_db(self.rate, freq);
        }
        if let Some((_, f)) = &self.low_pass {
            db += f.coeffs.response_db(self.rate, freq);
        }
        db
    }
}

impl Effect for ParametricEq {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if let Some((_, f)) = &mut self.high_pass {
            f.process(left, right);
        }
        for (b, f) in self.bands.iter().zip(self.filters.iter_mut()) {
            if b.enabled && (b.gain_db != 0.0 || !matches!(b.kind, BandKind::Bell | BandKind::LowShelf | BandKind::HighShelf)) {
                f.process(left, right);
            }
        }
        if let Some((_, f)) = &mut self.low_pass {
            f.process(left, right);
        }
        if self.output_db != 0.0 {
            let g = crate::db_to_gain(self.output_db);
            for v in left.iter_mut().chain(right.iter_mut()) {
                *v *= g;
            }
        }
    }

    fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
        if let Some((_, f)) = &mut self.high_pass {
            f.reset();
        }
        if let Some((_, f)) = &mut self.low_pass {
            f.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_gain_db(f: &mut dyn Effect, rate: u32, freq: f32) -> f32 {
        let n = rate as usize;
        let mut l: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect();
        let mut r = l.clone();
        f.process(&mut l, &mut r);
        let rms = (l[n / 2..].iter().map(|v| v * v).sum::<f32>() / (n / 2) as f32).sqrt();
        crate::gain_to_db(rms / std::f32::consts::FRAC_1_SQRT_2)
    }

    #[test]
    fn bell_boosts_centre_and_leaves_far_bands() {
        let c = Coeffs::design(BandKind::Bell, 48_000, 1000.0, 1.0, 6.0);
        assert!((c.response_db(48_000, 1000.0) - 6.0).abs() < 0.01);
        assert!(c.response_db(48_000, 50.0).abs() < 0.1);
        assert!(c.response_db(48_000, 18_000.0).abs() < 0.1);
        let mut b = Biquad::new(c);
        let g = tone_gain_db(&mut b, 48_000, 1000.0);
        assert!((g - 6.0).abs() < 0.05, "{g}");
    }

    #[test]
    fn high_pass_is_minus_3db_at_corner_and_steep_below() {
        let c = Coeffs::design(BandKind::HighPass, 48_000, 200.0, 0.707, 0.0);
        assert!((c.response_db(48_000, 200.0) + 3.01).abs() < 0.05);
        assert!(c.response_db(48_000, 50.0) < -23.0);
        assert!(c.response_db(48_000, 5000.0).abs() < 0.01);
    }

    #[test]
    fn shelves_apply_gain_on_their_side() {
        let lo = Coeffs::design(BandKind::LowShelf, 48_000, 200.0, 0.707, -9.0);
        assert!((lo.response_db(48_000, 10.0) + 9.0).abs() < 0.05);
        assert!(lo.response_db(48_000, 10_000.0).abs() < 0.05);
        let hi = Coeffs::design(BandKind::HighShelf, 48_000, 4000.0, 0.707, 4.0);
        assert!((hi.response_db(48_000, 20_000.0) - 4.0).abs() < 0.1);
        assert!(hi.response_db(48_000, 50.0).abs() < 0.05);
    }

    #[test]
    fn flat_eq_is_transparent_and_bands_sum() {
        let mut eq = ParametricEq::new(48_000);
        let mut l: Vec<f32> = (0..4096).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let r0 = l.clone();
        let mut r = l.clone();
        eq.process(&mut l, &mut r);
        assert_eq!(l, r0);
        eq.set_band(2, Band::bell(1000.0, 2.0, -6.0));
        eq.set_band(4, Band::new(BandKind::HighShelf, 8000.0, 0.707, 3.0));
        eq.set_high_pass(Some(80.0));
        assert!((eq.response_db(1000.0) + 6.0).abs() < 0.1);
        assert!((eq.response_db(20_000.0) - 3.0).abs() < 0.15);
        assert!((eq.response_db(80.0) + 3.0).abs() < 0.15);
        let g = tone_gain_db(&mut eq, 48_000, 1000.0);
        assert!((g + 6.0).abs() < 0.1, "{g}");
    }
}
