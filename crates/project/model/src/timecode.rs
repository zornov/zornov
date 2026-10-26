// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SMPTE ST 12 timecode at any frame rate.
//!
//! Labels count frames at the *nominal* rate (24 for 23.976, 30 for
//! 29.97, 60 for 59.94). Drop-frame (29.97 and 59.94 only) skips the
//! labels `;00` and `;01` (`;00`–`;03` at 59.94) at the start of every
//! minute except each tenth, so the label keeps pace with the wall clock;
//! the separator before frames is then `;`. Non-drop 29.97 and 23.976
//! labels run slow by 0.1 % against real time, as they do everywhere.

use crate::time::{FrameRate, Micros};
use std::fmt;

/// A timecode format: frame rate and drop-frame counting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timecode {
    rate: FrameRate,
    /// Nominal (integer) frames per second.
    fps: i64,
    drop: bool,
}

impl Timecode {
    /// Timecode at `rate`; drop-frame only applies to 29.97 and 59.94.
    pub fn new(rate: FrameRate, drop_frame: bool) -> Self {
        let fps = (rate.as_f64()).round() as i64;
        let ntsc = rate.den == 1001 && (fps == 30 || fps == 60);
        Self { rate, fps: fps.max(1), drop: drop_frame && ntsc }
    }

    /// The usual counting for a rate: drop-frame at 29.97 and 59.94.
    pub fn for_rate(rate: FrameRate) -> Self {
        Self::new(rate, true)
    }

    /// The real frame rate.
    pub fn rate(&self) -> FrameRate {
        self.rate
    }

    /// Frames per second the labels count.
    pub fn nominal_fps(&self) -> i64 {
        self.fps
    }

    /// Whether labels skip numbers (29.97 / 59.94 drop-frame).
    pub fn is_drop_frame(&self) -> bool {
        self.drop
    }

    fn dropped(&self) -> i64 {
        if self.fps == 60 { 4 } else { 2 }
    }

    /// Frame count → (hours, minutes, seconds, frames) label.
    pub fn label(&self, frame: i64) -> (i64, i64, i64, i64) {
        let mut f = frame.max(0);
        if self.drop {
            let drop = self.dropped();
            let per_min = self.fps * 60 - drop;
            let per_10 = per_min * 10 + drop;
            let tens = f / per_10;
            let rem = f % per_10;
            f += drop * 9 * tens + if rem > drop { drop * ((rem - drop) / per_min) } else { 0 };
        }
        let ff = f % self.fps;
        let s = f / self.fps;
        ((s / 3600) % 24, (s / 60) % 60, s % 60, ff)
    }

    /// Label → frame count (the inverse of [`Timecode::label`]).
    pub fn frames_of(&self, h: i64, m: i64, s: i64, ff: i64) -> i64 {
        let total_minutes = h * 60 + m;
        let mut f = ((h * 3600 + m * 60 + s) * self.fps) + ff;
        if self.drop {
            f -= self.dropped() * (total_minutes - total_minutes / 10);
        }
        f
    }

    /// Formats a frame count as `HH:MM:SS:FF` (`;` before frames for
    /// drop-frame).
    pub fn format_frames(&self, frame: i64) -> String {
        let (h, m, s, f) = self.label(frame);
        let sep = if self.drop { ';' } else { ':' };
        format!("{h:02}:{m:02}:{s:02}{sep}{f:02}")
    }

    /// The frame count of a timecode *label* given as time of day: `t` is
    /// read as HH:MM:SS plus a fraction of a second in label frames, so
    /// 1 hour means the label 01:00:00:00 (frame 86400 at 23.976, 107892
    /// at 29.97 drop-frame), not one hour of real time.
    pub fn label_frames(&self, t: Micros) -> i64 {
        let secs = t.div_euclid(crate::time::SECOND);
        let frac = t.rem_euclid(crate::time::SECOND) as f64 / crate::time::SECOND as f64;
        let ff = ((frac * self.fps as f64).round() as i64).min(self.fps - 1);
        self.frames_of(secs / 3600, (secs / 60) % 60, secs % 60, ff)
    }

    /// Formats time `t` (rounded to the nearest frame).
    pub fn format(&self, t: Micros) -> String {
        self.format_frames(self.rate.micros_to_frame_round(t))
    }

    /// Parses `HH:MM:SS:FF`, `HH:MM:SS;FF`, `HH;MM;SS;FF` or
    /// `HH:MM:SS.FF` into a frame count. Fails on a label that cannot
    /// exist (frames past the rate, or a dropped label).
    pub fn parse(&self, s: &str) -> Option<i64> {
        let parts: Vec<&str> = s.trim().split([':', ';', '.', ',']).collect();
        if parts.len() != 4 {
            return None;
        }
        let n: Vec<i64> = parts.iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
        let (h, m, sec, ff) = (n[0], n[1], n[2], n[3]);
        if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..60).contains(&sec) || !(0..self.fps).contains(&ff) {
            return None;
        }
        if self.drop && sec == 0 && m % 10 != 0 && ff < self.dropped() {
            return None;
        }
        Some(self.frames_of(h, m, sec, ff))
    }

    /// Like [`Timecode::parse`], but a drop-frame label that does not
    /// exist (`00:01:00;00`) is read as the next one that does
    /// (`00:01:00;02`), as editing systems do.
    pub fn parse_lenient(&self, s: &str) -> Option<i64> {
        self.parse(s).or_else(|| {
            let parts: Vec<i64> = s.trim().split([':', ';', '.', ',']).map(|p| p.parse().ok()).collect::<Option<_>>()?;
            (self.drop && parts.len() == 4 && parts[2] == 0 && parts[3] < self.dropped()).then(|| self.frames_of(parts[0], parts[1], 0, self.dropped()))
        })
    }

    /// Parses a label into time from midnight.
    pub fn parse_micros(&self, s: &str) -> Option<Micros> {
        self.parse(s).map(|f| self.rate.frame_to_micros(f))
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} fps {}", self.rate.as_f64(), if self.drop { "drop-frame" } else { "non-drop" })
    }
}

/// Common production frame rates by name: `23.976`, `24`, `25`, `29.97`,
/// `30`, `47.952`, `48`, `50`, `59.94`, `60`, `119.88`, `120` (also
/// `23.98`, `2997`, `30000/1001`).
pub fn rate_from_name(s: &str) -> Option<FrameRate> {
    let t = s.trim().trim_end_matches("fps").trim();
    if let Some((n, d)) = t.split_once('/') {
        let (n, d): (u32, u32) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
        return (n > 0 && d > 0).then(|| FrameRate::new(n, d));
    }
    let v: f64 = t.parse().ok().or_else(|| match t {
        "2398" | "23976" => Some(23.976),
        "2997" => Some(29.97),
        "5994" => Some(59.94),
        _ => None,
    })?;
    if v <= 0.0 || v > 1000.0 {
        return None;
    }
    let ntsc = |base: u32| (v - base as f64 * 1000.0 / 1001.0).abs() < 0.02;
    for base in [24u32, 30, 48, 60, 120] {
        if ntsc(base) && (v - base as f64).abs() > 0.005 {
            return Some(FrameRate::new(base * 1000, 1001));
        }
    }
    if (v - v.round()).abs() < 1e-6 {
        return Some(FrameRate::new(v.round() as u32, 1));
    }
    Some(FrameRate::new((v * 1000.0).round() as u32, 1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::SECOND;

    #[test]
    fn non_drop() {
        let tc = Timecode::new(FrameRate::FPS_25, true);
        assert!(!tc.is_drop_frame());
        assert_eq!(tc.format(0), "00:00:00:00");
        assert_eq!(tc.format(3600 * SECOND + 40_000 * 3), "01:00:00:03");
        assert_eq!(tc.parse("01:00:00:03"), Some(90_003));
        assert_eq!(tc.parse("00:00:00:25"), None);
    }

    #[test]
    fn drop_frame_29_97() {
        let r = FrameRate::FPS_29_97;
        let tc = Timecode::new(r, true);
        assert_eq!(tc.format_frames(1799), "00:00:59;29");
        assert_eq!(tc.format_frames(1800), "00:01:00;02");
        assert_eq!(tc.format_frames(17982), "00:10:00;00");
        assert_eq!(tc.parse("00:01:00;02"), Some(1800));
        assert_eq!(tc.parse("00:01:00;00"), None, "dropped label");
        assert_eq!(tc.parse_lenient("00:01:00;00"), Some(1800));
        assert_eq!(tc.parse("00:10:00;00"), Some(17982));
        // One hour of drop-frame is 107892 frames: real time to 3.6 ms.
        assert_eq!(tc.parse("01:00:00;00"), Some(107_892));
        // Every frame label round-trips over an hour.
        for f in (0..120_000).step_by(7) {
            assert_eq!(tc.parse(&tc.format_frames(f)), Some(f), "frame {f}");
        }
        assert_eq!(Timecode::new(r, false).format_frames(1800), "00:01:00:00");
    }

    #[test]
    fn drop_frame_59_94_and_23_976() {
        let tc = Timecode::new(FrameRate::new(60000, 1001), true);
        assert_eq!(tc.format_frames(3600), "00:01:00;04");
        for f in (0..250_000).step_by(13) {
            assert_eq!(tc.parse(&tc.format_frames(f)), Some(f));
        }
        let film = Timecode::new(FrameRate::new(24000, 1001), true);
        assert!(!film.is_drop_frame(), "23.976 never drops");
        assert_eq!(film.nominal_fps(), 24);
        assert_eq!(film.format_frames(24 * 3600), "01:00:00:00");
    }

    #[test]
    fn label_time() {
        let film = Timecode::new(FrameRate::new(24000, 1001), true);
        assert_eq!(film.label_frames(3600 * SECOND), 86_400);
        let df = Timecode::new(FrameRate::FPS_29_97, true);
        assert_eq!(df.label_frames(3600 * SECOND), 107_892);
        assert_eq!(df.format_frames(df.label_frames(3600 * SECOND)), "01:00:00;00");
    }

    #[test]
    fn rate_names() {
        assert_eq!(rate_from_name("23.976"), Some(FrameRate::new(24000, 1001)));
        assert_eq!(rate_from_name("23.98"), Some(FrameRate::new(24000, 1001)));
        assert_eq!(rate_from_name("29.97"), Some(FrameRate::FPS_29_97));
        assert_eq!(rate_from_name("59.94 fps"), Some(FrameRate::new(60000, 1001)));
        assert_eq!(rate_from_name("25"), Some(FrameRate::FPS_25));
        assert_eq!(rate_from_name("30000/1001"), Some(FrameRate::FPS_29_97));
        assert_eq!(rate_from_name("x"), None);
    }
}
