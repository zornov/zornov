// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Beat and onset analysis for music clips ("Auto beats", beat markers,
//! beat-snapped cuts).
//!
//! The onset envelope, tempo estimate and dynamic-programming beat tracker
//! live in the `audio` crate (`studio_audio::beats`); this module builds on
//! them with a confidence score, downbeat estimation, a regular [`BeatGrid`]
//! for snapping, and a planar-stereo entry point.

use crate::Micros;
pub use studio_audio::beats::{detect_beats_mono, detect_onsets, estimate_tempo, onset_envelope};

/// Everything the analyser found in a clip.
#[derive(Clone, Debug, PartialEq)]
pub struct BeatAnalysis {
    /// Tempo in beats per minute, `None` when no steady pulse was found.
    pub bpm: Option<f64>,
    /// Beat times from the clip start.
    pub beats: Vec<Micros>,
    /// Note-onset times from the clip start (denser than beats).
    pub onsets: Vec<Micros>,
    /// The beats judged to start a bar (every fourth beat, phased by energy).
    pub downbeats: Vec<Micros>,
    /// 0..1: how clearly the beats line up with onsets (under 0.3 means the
    /// markers are a guess).
    pub confidence: f32,
}

impl BeatAnalysis {
    /// A regular grid fitted to the detected beats, for snapping.
    pub fn grid(&self) -> Option<BeatGrid> {
        BeatGrid::fit(self.bpm?, &self.beats)
    }
}

/// Analyses planar stereo audio at `rate`.
pub fn analyse(left: &[f32], right: &[f32], rate: u32) -> BeatAnalysis {
    let mono: Vec<f32> = left.iter().zip(right).map(|(l, r)| 0.5 * (l + r)).collect();
    analyse_mono(&mono, rate)
}

/// Analyses interleaved stereo audio at `rate`.
pub fn analyse_interleaved(stereo: &[f32], rate: u32) -> BeatAnalysis {
    let mono: Vec<f32> = stereo.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect();
    analyse_mono(&mono, rate)
}

/// Analyses mono audio at `rate`.
pub fn analyse_mono(mono: &[f32], rate: u32) -> BeatAnalysis {
    let (env, fps) = onset_envelope(mono, rate);
    let bpm = estimate_tempo(&env, fps);
    let beats = detect_beats_mono(mono, rate);
    let stereo_like: Vec<f32> = mono.iter().flat_map(|v| [*v, *v]).collect();
    let onsets = detect_onsets(&stereo_like, rate);
    let strength_at = |t: Micros| -> f32 {
        let f = ((t as f64 / 1e6 - studio_audio::beats::LEAD) * fps).round() as isize;
        let lo = (f - 2).max(0) as usize;
        let hi = ((f + 3).max(0) as usize).min(env.len());
        if lo >= hi { 0.0 } else { env[lo..hi].iter().fold(0.0f32, |m, v| m.max(*v)) }
    };
    // Confidence: share of beats that sit on a clear onset.
    let confidence = if beats.is_empty() {
        0.0
    } else {
        beats.iter().filter(|b| strength_at(**b) > 1.0).count() as f32 / beats.len() as f32
    };
    // Downbeats: of the four possible phases, the one whose beats carry the
    // most onset energy (kicks and bass hits land on the one).
    let downbeats = if beats.len() >= 4 {
        let mut best = (f32::MIN, 0usize);
        for phase in 0..4 {
            let s: f32 = beats.iter().skip(phase).step_by(4).map(|b| strength_at(*b)).sum();
            let n = beats.iter().skip(phase).step_by(4).count().max(1) as f32;
            if s / n > best.0 {
                best = (s / n, phase);
            }
        }
        beats.iter().skip(best.1).step_by(4).copied().collect()
    } else {
        Vec::new()
    };
    BeatAnalysis { bpm: if beats.is_empty() { None } else { bpm }, beats, onsets, downbeats, confidence }
}

/// A regular beat grid: `offset + k * period`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatGrid {
    /// Tempo.
    pub bpm: f64,
    /// Time of beat 0 from the clip start (may be negative).
    pub offset: Micros,
}

impl BeatGrid {
    /// Fits a grid to detected beats by least squares on the beat index.
    pub fn fit(bpm: f64, beats: &[Micros]) -> Option<BeatGrid> {
        if beats.is_empty() || bpm <= 0.0 {
            return None;
        }
        let period = 60e6 / bpm;
        // Phase: mean of (beat − k·period) with k the nearest index.
        let first = beats[0] as f64;
        let mut sum = 0.0;
        for &b in beats {
            let k = ((b as f64 - first) / period).round();
            sum += b as f64 - k * period;
        }
        Some(BeatGrid { bpm, offset: (sum / beats.len() as f64).round() as Micros })
    }

    /// Beat period.
    pub fn period(&self) -> Micros {
        (60e6 / self.bpm).round() as Micros
    }

    /// The grid beat nearest to `t`.
    pub fn nearest(&self, t: Micros) -> Micros {
        let p = 60e6 / self.bpm;
        let k = ((t - self.offset) as f64 / p).round();
        self.offset + (k * p).round() as Micros
    }

    /// Grid beats within `start..end`.
    pub fn beats_in(&self, start: Micros, end: Micros) -> Vec<Micros> {
        let p = 60e6 / self.bpm;
        let k0 = ((start - self.offset) as f64 / p).ceil() as i64;
        let mut out = Vec::new();
        let mut k = k0;
        loop {
            let t = self.offset + (k as f64 * p).round() as Micros;
            if t >= end {
                break;
            }
            out.push(t);
            k += 1;
        }
        out
    }
}

/// Snaps `t` to the nearest marker in a sorted list when within `tolerance`.
pub fn snap_to(markers: &[Micros], t: Micros, tolerance: Micros) -> Option<Micros> {
    let i = markers.partition_point(|m| *m < t);
    let candidates = [i.checked_sub(1).map(|j| markers[j]), markers.get(i).copied()];
    candidates.iter().flatten().copied().filter(|m| (m - t).abs() <= tolerance).min_by_key(|m| (m - t).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(bpm: f64, secs: f64, rate: u32, accent_every: usize) -> Vec<f32> {
        let n = (secs * rate as f64) as usize;
        let mut x = vec![0f32; n];
        let period = 60.0 / bpm;
        let mut t = 0.25;
        let mut seed = 7u32;
        let mut k = 0;
        while t < secs {
            let s = (t * rate as f64) as usize;
            let amp = if k % accent_every == 0 { 0.9 } else { 0.35 };
            for j in 0..(0.03 * rate as f64) as usize {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                if s + j < n {
                    x[s + j] = amp * noise * (-(j as f32) / (0.005 * rate as f32)).exp();
                }
            }
            t += period;
            k += 1;
        }
        x
    }

    #[test]
    fn finds_tempo_beats_and_downbeats() {
        let rate = 48_000;
        let x = clicks(120.0, 12.0, rate, 4);
        let a = analyse(&x, &x, rate);
        let bpm = a.bpm.expect("tempo");
        assert!((bpm - 120.0).abs() < 2.0, "{bpm}");
        assert!(a.beats.len() >= 20, "{}", a.beats.len());
        assert!(a.confidence > 0.8, "{}", a.confidence);
        assert!(!a.onsets.is_empty());
        // Downbeats fall on the accented clicks (every 2 s from 0.25 s).
        assert!(a.downbeats.len() >= 4);
        for d in &a.downbeats {
            let rel = (d - 250_000).rem_euclid(2_000_000);
            let off = rel.min(2_000_000 - rel);
            assert!(off < 30_000, "downbeat {d} off by {off}");
        }
        let grid = a.grid().unwrap();
        assert!((grid.bpm - 120.0).abs() < 2.0);
        let near = grid.nearest(3_260_000);
        assert!((near - 3_250_000).abs() < 30_000, "{near}");
        let in_range = grid.beats_in(1_000_000, 3_000_000);
        assert_eq!(in_range.len(), 4, "{in_range:?}");
    }

    #[test]
    fn silence_has_no_beats_and_snap_respects_tolerance() {
        let z = vec![0.0f32; 48_000 * 5];
        let a = analyse_mono(&z, 48_000);
        assert!(a.beats.is_empty() && a.bpm.is_none() && a.confidence == 0.0);
        let m = [1_000_000, 1_500_000, 2_000_000];
        assert_eq!(snap_to(&m, 1_520_000, 50_000), Some(1_500_000));
        assert_eq!(snap_to(&m, 1_760_000, 50_000), None);
        assert_eq!(snap_to(&m, 2_200_000, 300_000), Some(2_000_000));
        assert_eq!(snap_to(&[], 5, 10), None);
    }
}
