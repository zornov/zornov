// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time values and the frame grid.
//!
//! Every time in the model is an integer number of microseconds ([`Micros`]).
//! Integers keep edits exact and repeatable: splitting and re-joining a clip
//! never drifts the way floating-point seconds would. Frame-accurate
//! operations snap to a [`FrameRate`] grid explicitly, through the helpers
//! here, so callers always know when rounding happens.

use serde::{Deserialize, Serialize};

/// A time or duration in microseconds.
pub type Micros = i64;

/// Microseconds in one second.
pub const SECOND: Micros = 1_000_000;

/// A frame rate as an exact fraction of frames per second, e.g. 30000/1001.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRate {
    /// Frames per `den` seconds.
    pub num: u32,
    /// Seconds denominator; never zero.
    pub den: u32,
}

impl FrameRate {
    /// Creates a rate of `num / den` frames per second.
    ///
    /// # Panics
    /// Panics if either part is zero.
    pub const fn new(num: u32, den: u32) -> Self {
        assert!(num > 0 && den > 0, "frame rate parts must be positive");
        Self { num, den }
    }

    /// 24 frames per second.
    pub const FPS_24: Self = Self::new(24, 1);
    /// 25 frames per second.
    pub const FPS_25: Self = Self::new(25, 1);
    /// 30 frames per second.
    pub const FPS_30: Self = Self::new(30, 1);
    /// 29.97 (30000/1001) frames per second.
    pub const FPS_29_97: Self = Self::new(30000, 1001);
    /// 60 frames per second.
    pub const FPS_60: Self = Self::new(60, 1);

    /// The rate as a float, for display and for formats that store floats.
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// The start time of frame `index` (rounded to the nearest microsecond).
    pub fn frame_to_micros(self, index: i64) -> Micros {
        let n = index as i128 * SECOND as i128 * self.den as i128;
        div_round(n, self.num as i128) as Micros
    }

    /// The frame that contains time `t` (floor), so a time inside a frame
    /// maps to that frame rather than the next one.
    pub fn micros_to_frame_floor(self, t: Micros) -> i64 {
        // Add half a microsecond's worth of slack so times produced by
        // `frame_to_micros` (which rounds) map back to the same frame.
        let n = (t as i128 * 2 + 1) * self.num as i128;
        let d = 2 * SECOND as i128 * self.den as i128;
        n.div_euclid(d) as i64
    }

    /// The nearest frame index to time `t`.
    pub fn micros_to_frame_round(self, t: Micros) -> i64 {
        let n = t as i128 * self.num as i128;
        let d = SECOND as i128 * self.den as i128;
        div_round(n, d) as i64
    }

    /// Snaps `t` to the nearest frame boundary.
    pub fn snap(self, t: Micros) -> Micros {
        self.frame_to_micros(self.micros_to_frame_round(t))
    }

    /// Snaps `t` down to the start of the frame containing it.
    pub fn snap_floor(self, t: Micros) -> Micros {
        self.frame_to_micros(self.micros_to_frame_floor(t))
    }

    /// The duration of one frame, rounded to the nearest microsecond.
    pub fn frame_duration(self) -> Micros {
        self.frame_to_micros(1)
    }
}

impl Default for FrameRate {
    fn default() -> Self {
        Self::FPS_30
    }
}

/// Divides rounding half away from zero.
fn div_round(n: i128, d: i128) -> i128 {
    debug_assert!(d > 0);
    if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
}

/// A half-open time range `[start, start + duration)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct TimeRange {
    /// First instant in the range.
    pub start: Micros,
    /// Length of the range; zero means empty.
    pub duration: Micros,
}

impl TimeRange {
    /// Creates `[start, start + duration)`.
    pub const fn new(start: Micros, duration: Micros) -> Self {
        Self { start, duration }
    }

    /// Creates the range between two instants.
    pub const fn from_bounds(start: Micros, end: Micros) -> Self {
        Self { start, duration: end - start }
    }

    /// The first instant after the range.
    pub const fn end(&self) -> Micros {
        self.start + self.duration
    }

    /// Whether `t` lies inside the range.
    pub const fn contains(&self, t: Micros) -> bool {
        t >= self.start && t < self.end()
    }

    /// Whether the two ranges share any instant.
    pub const fn overlaps(&self, other: &TimeRange) -> bool {
        self.start < other.end() && other.start < self.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_grid_round_trips() {
        for rate in [FrameRate::FPS_24, FrameRate::FPS_29_97, FrameRate::FPS_60] {
            for f in [-5i64, 0, 1, 2, 29, 30, 1000, 123_456] {
                let t = rate.frame_to_micros(f);
                assert_eq!(rate.micros_to_frame_floor(t), f, "{rate:?} {f}");
                assert_eq!(rate.micros_to_frame_round(t), f);
                assert_eq!(rate.snap(t), t);
            }
        }
    }

    #[test]
    fn snapping() {
        let r = FrameRate::FPS_25; // 40 ms frames
        assert_eq!(r.snap(19_999), 0);
        assert_eq!(r.snap(20_000), 40_000);
        assert_eq!(r.snap_floor(79_999), 40_000);
        assert_eq!(r.frame_duration(), 40_000);
        assert_eq!(FrameRate::FPS_29_97.frame_duration(), 33_367);
    }

    #[test]
    fn ranges() {
        let a = TimeRange::new(10, 10);
        assert!(a.contains(10) && a.contains(19) && !a.contains(20));
        assert!(a.overlaps(&TimeRange::new(19, 5)));
        assert!(!a.overlaps(&TimeRange::new(20, 5)));
    }
}
