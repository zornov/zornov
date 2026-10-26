// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Speed ramps: a clip's playback speed as an editable curve over its
//! output time, with presets, and the time mapping that follows from it.
//!
//! The curve gives a speed multiplier at each point of the clip's output
//! duration (normalised to 0..1). Between keys the speed follows a cubic
//! Bézier timing curve, interpolated in log2 space so that a ramp from 0.25x
//! to 4x feels as even on the way up as on the way down.

use crate::easing::cubic_bezier;
use crate::image::Image;

/// Slowest and fastest speeds a curve may use.
pub const MIN_SPEED: f32 = 0.1;
pub const MAX_SPEED: f32 = 100.0;

/// One key of a speed curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedKey {
    /// Position in the clip's output, 0..1.
    pub t: f32,
    /// Speed multiplier (1 = normal).
    pub speed: f32,
    /// Timing curve of the segment that starts at this key, as cubic
    /// Bézier control points (x1, y1, x2, y2).
    pub ease: [f32; 4],
}

/// The default segment shape: gentle in and out.
pub const SMOOTH: [f32; 4] = [0.42, 0.0, 0.58, 1.0];
pub const LINEAR: [f32; 4] = [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0];

const fn key(t: f32, speed: f32) -> SpeedKey {
    SpeedKey { t, speed, ease: SMOOTH }
}

/// A named starting point for a speed curve.
#[derive(Clone, Copy, Debug)]
pub struct SpeedPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub keys: &'static [SpeedKey],
}

pub static PRESETS: &[SpeedPreset] = &[
    SpeedPreset { id: "constant", name: "Constant", description: "The same speed throughout.", keys: &[key(0.0, 1.0), key(1.0, 1.0)] },
    SpeedPreset { id: "speed_up", name: "Speed up", description: "Starts slow and accelerates.", keys: &[key(0.0, 0.4), key(1.0, 2.5)] },
    SpeedPreset { id: "slow_down", name: "Slow down", description: "Starts fast and slows to a crawl.", keys: &[key(0.0, 2.5), key(1.0, 0.4)] },
    SpeedPreset {
        id: "surge",
        name: "Surge",
        description: "A burst of speed in the middle.",
        keys: &[key(0.0, 1.0), key(0.5, 3.0), key(1.0, 1.0)],
    },
    SpeedPreset {
        id: "dramatic",
        name: "Dramatic",
        description: "Fast, then a long slow-motion moment, then fast again.",
        keys: &[key(0.0, 3.0), key(0.35, 0.2), key(0.65, 0.2), key(1.0, 3.0)],
    },
    SpeedPreset {
        id: "spotlight",
        name: "Spotlight",
        description: "Normal speed with a slow-motion beat in the middle.",
        keys: &[key(0.0, 1.0), key(0.3, 1.0), key(0.45, 0.25), key(0.75, 0.25), key(1.0, 1.0)],
    },
    SpeedPreset {
        id: "rhythm",
        name: "Rhythm",
        description: "Alternates fast and slow for rhythmic edits.",
        keys: &[key(0.0, 2.0), key(0.25, 0.5), key(0.5, 2.0), key(0.75, 0.5), key(1.0, 2.0)],
    },
    SpeedPreset { id: "rush_in", name: "Rush in", description: "Rushes in, then plays normally.", keys: &[key(0.0, 5.0), key(0.3, 1.0), key(1.0, 1.0)] },
    SpeedPreset { id: "rush_out", name: "Rush out", description: "Plays normally, then rushes out.", keys: &[key(0.0, 1.0), key(0.7, 1.0), key(1.0, 5.0)] },
];

pub fn find_preset(id: &str) -> Option<&'static SpeedPreset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// A speed curve with its integral precomputed for fast time mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeedCurve {
    keys: Vec<SpeedKey>,
    /// Cumulative integral of speed over 0..1 at `TABLE + 1` even steps.
    cumulative: Vec<f64>,
}

const TABLE: usize = 4096;

impl SpeedCurve {
    /// Builds a curve. Keys are sorted, clamped into range, and the first
    /// and last are pinned to t = 0 and t = 1 (adding them if missing), so
    /// any list of keys gives a usable curve.
    pub fn new(keys: &[SpeedKey]) -> Self {
        let mut k: Vec<SpeedKey> = keys
            .iter()
            .filter(|k| k.t.is_finite() && k.speed.is_finite())
            .map(|k| SpeedKey {
                t: k.t.clamp(0.0, 1.0),
                speed: k.speed.clamp(MIN_SPEED, MAX_SPEED),
                ease: k.ease.map(|v| if v.is_finite() { v } else { 0.5 }),
            })
            .collect();
        k.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        if k.is_empty() {
            k.push(key(0.0, 1.0));
        }
        if k[0].t > 0.0 {
            k.insert(0, SpeedKey { t: 0.0, ..k[0] });
        }
        let last = *k.last().unwrap();
        if last.t < 1.0 {
            k.push(SpeedKey { t: 1.0, ..last });
        }
        let mut curve = SpeedCurve { keys: k, cumulative: Vec::new() };
        curve.cumulative = curve.integrate();
        curve
    }

    pub fn from_preset(id: &str) -> Option<Self> {
        find_preset(id).map(|p| Self::new(p.keys))
    }

    /// Constant speed.
    pub fn constant(speed: f32) -> Self {
        Self::new(&[key(0.0, speed), key(1.0, speed)])
    }

    pub fn keys(&self) -> &[SpeedKey] {
        &self.keys
    }

    /// Speed multiplier at normalised output position `u`.
    pub fn speed_at(&self, u: f32) -> f32 {
        let u = if u.is_finite() { u.clamp(0.0, 1.0) } else { 0.0 };
        let i = self.keys.partition_point(|k| k.t <= u).clamp(1, self.keys.len() - 1);
        let (a, b) = (self.keys[i - 1], self.keys[i]);
        let span = b.t - a.t;
        if span <= 1e-9 || a.speed == b.speed {
            // Flat segments stay exact rather than going through log/exp.
            return b.speed;
        }
        let f = cubic_bezier(a.ease, (u - a.t) / span);
        let (la, lb) = (a.speed.log2(), b.speed.log2());
        (la + (lb - la) * f).exp2().clamp(MIN_SPEED, MAX_SPEED)
    }

    fn integrate(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(TABLE + 1);
        out.push(0.0);
        let h = 1.0 / TABLE as f64;
        let mut acc = 0.0f64;
        let mut prev = self.speed_at(0.0) as f64;
        for i in 1..=TABLE {
            let mid = self.speed_at(((i as f64 - 0.5) * h) as f32) as f64;
            let cur = self.speed_at((i as f64 * h) as f32) as f64;
            // Simpson's rule on each step.
            acc += h * (prev + 4.0 * mid + cur) / 6.0;
            out.push(acc);
            prev = cur;
        }
        out
    }

    /// Average speed over the clip.
    pub fn mean_speed(&self) -> f64 {
        self.cumulative[TABLE]
    }

    /// Length of the clip on the timeline when it plays `source_duration`
    /// seconds of media.
    pub fn output_duration(&self, source_duration: f64) -> f64 {
        source_duration / self.mean_speed()
    }

    /// Fraction of the source consumed by normalised output position `u`.
    fn source_fraction(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        let f = u * TABLE as f64;
        let i = (f.floor() as usize).min(TABLE - 1);
        let k = f - i as f64;
        let v = self.cumulative[i] + (self.cumulative[i + 1] - self.cumulative[i]) * k;
        v / self.mean_speed()
    }

    /// Source media time shown at `output_time` seconds into the clip.
    pub fn source_time(&self, output_time: f64, source_duration: f64) -> f64 {
        let out_dur = self.output_duration(source_duration);
        if out_dur <= 0.0 {
            return 0.0;
        }
        self.source_fraction(output_time / out_dur) * source_duration
    }

    /// Inverse of [`SpeedCurve::source_time`]: when (in output time) a
    /// given source time is shown. Used to place markers and audio.
    pub fn output_time(&self, source_time: f64, source_duration: f64) -> f64 {
        if source_duration <= 0.0 {
            return 0.0;
        }
        let target = (source_time / source_duration).clamp(0.0, 1.0) * self.mean_speed();
        let i = self.cumulative.partition_point(|&c| c < target).clamp(1, TABLE);
        let (c0, c1) = (self.cumulative[i - 1], self.cumulative[i]);
        let k = if c1 > c0 { (target - c0) / (c1 - c0) } else { 0.0 };
        ((i - 1) as f64 + k) / TABLE as f64 * self.output_duration(source_duration)
    }
}

/// How in-between source frames are produced in slow motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Repeat the nearest frame.
    Nearest,
    /// Cross-fade the two neighbouring frames.
    Blend,
    /// Motion-compensated in-betweens; the `ai` crate supplies these and
    /// callers fall back to `Blend` when it is not available.
    OpticalFlow,
}

/// The two source frames around `source_time` and the weight of the
/// second, for frames at `frame_rate`.
pub fn frames_around(source_time: f64, frame_rate: f64) -> (i64, i64, f32) {
    let f = (source_time * frame_rate).max(0.0);
    let a = f.floor();
    (a as i64, a as i64 + 1, (f - a) as f32)
}

/// Frame blending: the in-between at weight `t` of two frames.
pub fn blend_frames(a: &Image, b: &Image, t: f32) -> Image {
    crate::ops::mix(a, b, t.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_speed_is_linear() {
        let c = SpeedCurve::constant(2.0);
        assert!((c.output_duration(10.0) - 5.0).abs() < 1e-6);
        assert!((c.source_time(2.5, 10.0) - 5.0).abs() < 1e-6);
        let n = SpeedCurve::constant(1.0);
        for i in 0..=20 {
            let t = i as f64 * 0.5;
            assert!((n.source_time(t, 10.0) - t).abs() < 1e-6);
        }
    }

    #[test]
    fn presets_map_ends_and_invert() {
        for p in PRESETS {
            let c = SpeedCurve::new(p.keys);
            let src = 8.0;
            let out = c.output_duration(src);
            assert!(out.is_finite() && out > 0.0, "{}", p.id);
            assert!(c.source_time(0.0, src).abs() < 1e-9);
            assert!((c.source_time(out, src) - src).abs() < 1e-6, "{}", p.id);
            let mut last = -1.0;
            for i in 0..=100 {
                let t = out * i as f64 / 100.0;
                let s = c.source_time(t, src);
                assert!(s >= last, "{} not monotonic", p.id);
                last = s;
                assert!((c.output_time(s, src) - t).abs() < 1e-3, "{} inverse at {t}", p.id);
            }
        }
    }

    #[test]
    fn keys_are_cleaned() {
        let c = SpeedCurve::new(&[key(0.5, 1000.0), key(f32::NAN, 1.0)]);
        assert_eq!(c.keys().len(), 3);
        assert_eq!(c.keys()[0].t, 0.0);
        assert_eq!(c.keys()[2].t, 1.0);
        assert_eq!(c.speed_at(0.5), MAX_SPEED);
        assert!(SpeedCurve::new(&[]).speed_at(0.3) == 1.0);
    }

    #[test]
    fn frames_around_splits() {
        assert_eq!(frames_around(1.25, 4.0), (5, 6, 0.0));
        let (a, b, t) = frames_around(0.55, 10.0);
        assert_eq!((a, b), (5, 6));
        assert!((t - 0.5).abs() < 1e-4);
    }
}
