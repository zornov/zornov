// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyframe curves evaluated per sample.
//!
//! The project model stores keyframes with microsecond times and an easing
//! per segment; the mixer turns those into a [`Curve`] and asks it to fill a
//! gain ramp for each block, so a volume keyframe lands on the exact sample
//! it was placed on rather than on a block boundary.

use crate::{Micros, SECOND};

/// Shape of the segment leaving a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Ease {
    /// Straight line to the next key.
    #[default]
    Linear,
    /// Stay on this key's value until the next key.
    Hold,
    /// Slow start, fast finish (quadratic).
    In,
    /// Fast start, slow finish (quadratic).
    Out,
    /// Smooth both ends (smoothstep).
    InOut,
}

impl Ease {
    /// Maps progress `t` in 0..=1 to the eased progress.
    #[inline]
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Ease::Linear => t,
            Ease::Hold => 0.0,
            Ease::In => t * t,
            Ease::Out => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::InOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// One keyframe: a value at a time, with the easing used to reach the next key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// Position on the clip or lane timeline.
    pub time: Micros,
    /// The value at that time (dB for volume, −1..1 for pan, …).
    pub value: f32,
    /// Easing of the segment that starts here.
    pub ease: Ease,
}

impl Key {
    /// A linear key.
    pub const fn new(time: Micros, value: f32) -> Self {
        Self { time, value, ease: Ease::Linear }
    }

    /// A key with an explicit easing.
    pub const fn eased(time: Micros, value: f32, ease: Ease) -> Self {
        Self { time, value, ease }
    }
}

/// A piecewise curve over sorted keys. Before the first key it holds the
/// first value, after the last it holds the last. An empty curve is a
/// constant (its `default` value).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    keys: Vec<Key>,
    default: f32,
}

impl Curve {
    /// A constant curve.
    pub fn constant(value: f32) -> Self {
        Self { keys: Vec::new(), default: value }
    }

    /// A curve over `keys` (sorted by time here; equal times keep the later one).
    pub fn new(mut keys: Vec<Key>, default: f32) -> Self {
        keys.sort_by_key(|k| k.time);
        keys.dedup_by(|later, earlier| {
            if later.time == earlier.time {
                *earlier = *later;
                true
            } else {
                false
            }
        });
        Self { keys, default }
    }

    /// The keys, sorted by time.
    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// True when the curve never changes value.
    pub fn is_constant(&self) -> bool {
        self.keys.len() <= 1
    }

    /// The value at `t`.
    pub fn eval(&self, t: Micros) -> f32 {
        match self.keys.len() {
            0 => self.default,
            1 => self.keys[0].value,
            _ => {
                let i = self.keys.partition_point(|k| k.time <= t);
                if i == 0 {
                    self.keys[0].value
                } else if i == self.keys.len() {
                    self.keys[i - 1].value
                } else {
                    Self::between(&self.keys[i - 1], &self.keys[i], t as f64)
                }
            }
        }
    }

    #[inline]
    fn between(a: &Key, b: &Key, t: f64) -> f32 {
        let span = (b.time - a.time) as f64;
        let p = if span <= 0.0 { 1.0 } else { ((t - a.time as f64) / span).clamp(0.0, 1.0) as f32 };
        a.value + (b.value - a.value) * a.ease.apply(p)
    }

    /// Fills `out` with the curve sampled at `rate` from timeline time `start`,
    /// one value per frame. Allocation-free.
    pub fn fill(&self, start: Micros, rate: u32, out: &mut [f32]) {
        if self.keys.len() <= 1 {
            out.fill(self.eval(start));
            return;
        }
        let dt = SECOND as f64 / rate.max(1) as f64;
        let mut t = start as f64;
        // Segment index: keys[i-1] <= t < keys[i].
        let mut i = self.keys.partition_point(|k| (k.time as f64) <= t);
        for v in out.iter_mut() {
            while i < self.keys.len() && (self.keys[i].time as f64) <= t {
                i += 1;
            }
            *v = if i == 0 {
                self.keys[0].value
            } else if i == self.keys.len() {
                self.keys[i - 1].value
            } else {
                Self::between(&self.keys[i - 1], &self.keys[i], t)
            };
            t += dt;
        }
    }

    /// Shifts every key by `delta` (used when a clip moves on the timeline).
    pub fn shift(&mut self, delta: Micros) {
        for k in &mut self.keys {
            k.time += delta;
        }
    }
}

/// Shape of a fade-in or fade-out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FadeShape {
    /// Linear gain ramp.
    #[default]
    Linear,
    /// Sine quarter-wave: constant power across a crossfade of two clips.
    EqualPower,
    /// Smooth S-curve, gentle at both ends.
    SCurve,
    /// Fast rise then slow (logarithmic feel); "fade-in" shape favoured for music.
    Fast,
    /// Slow rise then fast (exponential feel); hides tails of noise.
    Slow,
}

impl FadeShape {
    /// Gain for a fade **in** at progress `p` (0 at the start, 1 when done).
    /// A fade out is `gain(1 - p)`.
    #[inline]
    pub fn gain(self, p: f32) -> f32 {
        let p = p.clamp(0.0, 1.0);
        match self {
            FadeShape::Linear => p,
            FadeShape::EqualPower => (p * std::f32::consts::FRAC_PI_2).sin(),
            FadeShape::SCurve => p * p * (3.0 - 2.0 * p),
            FadeShape::Fast => (1.0 - (1.0 - p) * (1.0 - p)).sqrt(),
            FadeShape::Slow => p * p * p,
        }
    }
}

/// A clip's fade-in and fade-out, in frames of the clip's own output.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Fades {
    /// Fade-in length in frames.
    pub fade_in: u64,
    /// Fade-out length in frames.
    pub fade_out: u64,
    /// Shape used for both.
    pub shape: FadeShape,
}

impl Fades {
    /// Multiplies `gains` (already holding the volume ramp) by the fade
    /// envelope for clip frames `first..first + gains.len()` of a clip
    /// `length` frames long.
    pub fn apply(&self, first: u64, length: u64, gains: &mut [f32]) {
        for (i, g) in gains.iter_mut().enumerate() {
            let f = first + i as u64;
            if self.fade_in > 0 && f < self.fade_in {
                *g *= self.shape.gain(f as f32 / self.fade_in as f32);
            }
            if self.fade_out > 0 && f + self.fade_out >= length {
                let remaining = length.saturating_sub(f);
                *g *= self.shape.gain(remaining as f32 / self.fade_out as f32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_and_fill_agree_and_hit_keys_exactly() {
        let c = Curve::new(vec![Key::new(SECOND, 0.0), Key::new(2 * SECOND, -12.0), Key::eased(3 * SECOND, 6.0, Ease::Hold), Key::new(4 * SECOND, 0.0)], 1.0);
        assert_eq!(c.eval(0), 0.0);
        assert_eq!(c.eval(SECOND), 0.0);
        assert!((c.eval(SECOND + SECOND / 2) + 6.0).abs() < 1e-5);
        assert_eq!(c.eval(2 * SECOND), -12.0);
        assert_eq!(c.eval(3 * SECOND + 1), 6.0, "hold keeps the value");
        assert_eq!(c.eval(10 * SECOND), 0.0);
        let mut out = vec![0.0; 48_000];
        c.fill(SECOND + SECOND / 2, 48_000, &mut out);
        for (i, v) in out.iter().enumerate() {
            let t = SECOND + SECOND / 2 + (i as f64 / 48_000.0 * SECOND as f64).round() as Micros;
            assert!((c.eval(t) - v).abs() < 2e-3, "frame {i}: {} vs {v}", c.eval(t));
        }
        // The sample at exactly the 2 s key is exactly -12 dB.
        assert!((out[24_000] + 12.0).abs() < 1e-4);
    }

    #[test]
    fn constant_and_single_key() {
        assert_eq!(Curve::constant(3.0).eval(5), 3.0);
        let c = Curve::new(vec![Key::new(7, 2.0)], 0.0);
        assert_eq!(c.eval(-100), 2.0);
        assert_eq!(c.eval(100), 2.0);
        let mut out = [9.0; 4];
        c.fill(0, 48_000, &mut out);
        assert_eq!(out, [2.0; 4]);
    }

    #[test]
    fn eases_are_monotone_and_hit_ends() {
        for e in [Ease::Linear, Ease::In, Ease::Out, Ease::InOut] {
            assert_eq!(e.apply(0.0), 0.0);
            assert!((e.apply(1.0) - 1.0).abs() < 1e-6);
            let mut last = 0.0;
            for i in 1..=100 {
                let v = e.apply(i as f32 / 100.0);
                assert!(v >= last);
                last = v;
            }
        }
    }

    #[test]
    fn fades_and_equal_power_crossfade() {
        let f = Fades { fade_in: 100, fade_out: 100, shape: FadeShape::EqualPower };
        let mut g = vec![1.0; 1000];
        f.apply(0, 1000, &mut g);
        assert_eq!(g[0], 0.0);
        assert!((g[100] - 1.0).abs() < 1e-6);
        assert!((g[500] - 1.0).abs() < 1e-6);
        assert!(g[999] < 0.02);
        for p in [0.0, 0.25, 0.5, 1.0] {
            let a = FadeShape::EqualPower.gain(p);
            let b = FadeShape::EqualPower.gain(1.0 - p);
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
    }
}
