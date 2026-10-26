// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Voice activity detection for ducking and the recorder.
//!
//! The detector is deliberately simple and robust rather than clever: it
//! band-limits the signal to the speech band, tracks the energy of 10 ms
//! frames in dB, estimates the noise floor with an asymmetric tracker (fast
//! down, slow up) and calls speech when the energy clears the floor by a
//! margin, with hysteresis and a hangover so pauses between words do not
//! chop the decision. Speech may only *start* once the level shows
//! syllable-rate modulation (a few dB of spread over the last frames), which
//! rejects steady tones and drones; a sustained vowel keeps an already-open
//! decision open, it just cannot open one on its own.

use crate::Micros;
use crate::eq::{BandKind, Biquad};

/// Detector settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VadParams {
    /// dB above the noise floor that counts as speech (6..20).
    pub threshold_db: f32,
    /// dB the energy may fall below the threshold before speech ends (0..10).
    pub hysteresis_db: f32,
    /// Time after the last speech frame before the decision turns off, in ms.
    pub hangover_ms: f32,
    /// Minimum run of frames above threshold before speech starts, in ms
    /// (at least 60 ms, which the modulation check needs).
    pub onset_ms: f32,
    /// Lowest level ever treated as speech, in dBFS; quieter rooms are noise.
    pub floor_dbfs: f32,
}

impl Default for VadParams {
    fn default() -> Self {
        Self { threshold_db: 9.0, hysteresis_db: 3.0, hangover_ms: 250.0, onset_ms: 60.0, floor_dbfs: -55.0 }
    }
}

const FRAME_MS: f32 = 10.0;
/// Frames of level history used for the modulation check (320 ms).
const MOD_WINDOW: usize = 32;
/// Active frames needed before the modulation check is trusted.
const MOD_MIN_FRAMES: usize = 6;
/// Spread in dB over those frames that counts as modulated.
const MOD_SPREAD_DB: f32 = 4.0;

/// Streaming detector over mono input.
pub struct Vad {
    rate: u32,
    params: VadParams,
    hp: Biquad,
    lp: Biquad,
    frame_len: usize,
    frame_pos: usize,
    frame_energy: f64,
    levels: [f32; MOD_WINDOW],
    levels_pos: usize,
    noise_db: f32,
    speaking: bool,
    hang_left: usize,
    onset_count: usize,
    hang_frames: usize,
    onset_frames: usize,
    last_level_db: f32,
    probability: f32,
}

impl Vad {
    /// A detector at `rate`.
    pub fn new(rate: u32, params: VadParams) -> Self {
        let frame_len = ((FRAME_MS * 0.001 * rate as f32) as usize).max(16);
        let mut v = Self {
            rate,
            params,
            hp: Biquad::design(BandKind::HighPass, rate, 120.0, 0.707, 0.0),
            lp: Biquad::design(BandKind::LowPass, rate, 4000.0, 0.707, 0.0),
            frame_len,
            frame_pos: 0,
            frame_energy: 0.0,
            levels: [-100.0; MOD_WINDOW],
            levels_pos: 0,
            noise_db: -60.0,
            speaking: false,
            hang_left: 0,
            onset_count: 0,
            hang_frames: 0,
            onset_frames: 0,
            last_level_db: -100.0,
            probability: 0.0,
        };
        v.set_params(params);
        v
    }

    /// Applies new settings.
    pub fn set_params(&mut self, p: VadParams) {
        self.params = p;
        self.hang_frames = (p.hangover_ms / FRAME_MS).round() as usize;
        self.onset_frames = ((p.onset_ms / FRAME_MS).round() as usize).max(MOD_MIN_FRAMES);
    }

    /// Sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Feeds mono samples; the decision updates every 10 ms.
    pub fn process(&mut self, mono: &[f32]) {
        for &x in mono {
            let y = self.lp.tick(0, self.hp.tick(0, x));
            self.frame_energy += (y * y) as f64;
            self.frame_pos += 1;
            if self.frame_pos == self.frame_len {
                self.finish_frame();
            }
        }
    }

    /// Feeds a stereo block (summed to mono).
    pub fn process_stereo(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            let x = 0.5 * (l + r);
            let y = self.lp.tick(0, self.hp.tick(0, x));
            self.frame_energy += (y * y) as f64;
            self.frame_pos += 1;
            if self.frame_pos == self.frame_len {
                self.finish_frame();
            }
        }
    }

    /// Whether the recent active frames show syllable-like level movement.
    /// `None` when too few frames were active to tell.
    fn modulated(&self, active_floor_db: f32) -> Option<bool> {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut n = 0;
        for &v in &self.levels {
            if v > active_floor_db {
                lo = lo.min(v);
                hi = hi.max(v);
                n += 1;
            }
        }
        if n < MOD_MIN_FRAMES { None } else { Some(hi - lo >= MOD_SPREAD_DB) }
    }

    fn finish_frame(&mut self) {
        let p = self.params;
        let mean = self.frame_energy / self.frame_len as f64;
        let level_db = (10.0 * mean.max(1e-12).log10()) as f32;
        self.frame_energy = 0.0;
        self.frame_pos = 0;
        self.last_level_db = level_db;
        self.levels[self.levels_pos] = level_db;
        self.levels_pos = (self.levels_pos + 1) % MOD_WINDOW;
        // Noise floor: drops quickly, rises slowly (about 3 dB/s), and never
        // chases speech itself.
        if level_db < self.noise_db {
            self.noise_db += (level_db - self.noise_db) * 0.3;
        } else if !self.speaking {
            self.noise_db += 0.03;
        } else {
            self.noise_db += 0.005;
        }
        self.noise_db = self.noise_db.max(-100.0);
        let margin = level_db - self.noise_db;
        let active_floor = (self.noise_db + p.threshold_db - p.hysteresis_db).max(p.floor_dbfs - 3.0);
        let above_on = margin > p.threshold_db && level_db > p.floor_dbfs;
        let above_off = margin > p.threshold_db - p.hysteresis_db && level_db > p.floor_dbfs - 3.0;
        if self.speaking {
            if above_off {
                self.hang_left = self.hang_frames;
            } else if self.hang_left > 0 {
                self.hang_left -= 1;
            } else {
                self.speaking = false;
                self.onset_count = 0;
            }
        } else if above_on {
            self.onset_count += 1;
            if self.onset_count >= self.onset_frames && self.modulated(active_floor) == Some(true) {
                self.speaking = true;
                self.hang_left = self.hang_frames;
            }
        } else {
            self.onset_count = 0;
        }
        // Soft score: how far past the threshold, squashed to 0..1; it only
        // crosses 0.5 while the decision is on.
        let soft = ((margin - p.threshold_db) / 6.0 + 0.5).clamp(0.0, 1.0);
        let target = if self.speaking { soft.max(0.5) } else { soft * 0.4 };
        self.probability += (target - self.probability) * 0.5;
    }

    /// The current decision.
    pub fn is_speech(&self) -> bool {
        self.speaking
    }

    /// A smoothed 0..1 confidence (for meters and soft ducking).
    pub fn probability(&self) -> f32 {
        self.probability
    }

    /// Estimated noise floor in dBFS (speech-band energy).
    pub fn noise_floor_db(&self) -> f32 {
        self.noise_db
    }

    /// Speech-band level of the last frame in dBFS.
    pub fn level_db(&self) -> f32 {
        self.last_level_db
    }

    /// Forgets the state (on seek).
    pub fn reset(&mut self) {
        self.hp.reset();
        self.lp.reset();
        self.frame_pos = 0;
        self.frame_energy = 0.0;
        self.levels = [-100.0; MOD_WINDOW];
        self.levels_pos = 0;
        self.noise_db = -60.0;
        self.speaking = false;
        self.hang_left = 0;
        self.onset_count = 0;
        self.probability = 0.0;
    }
}

/// Offline detection: speech segments of a mono signal as timeline ranges
/// relative to its start.
pub fn speech_segments(mono: &[f32], rate: u32, params: VadParams) -> Vec<(Micros, Micros)> {
    let mut vad = Vad::new(rate, params);
    let frame = vad.frame_len;
    let mut out: Vec<(Micros, Micros)> = Vec::new();
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < mono.len() {
        let end = (i + frame).min(mono.len());
        vad.process(&mono[i..end]);
        let t = i;
        match (vad.is_speech(), open) {
            (true, None) => open = Some(t.saturating_sub(frame * (vad.onset_frames + 1))),
            (false, Some(s)) => {
                // Trim the hangover off the end.
                let e = t.saturating_sub(frame * vad.hang_frames).max(s + frame);
                out.push((crate::frames_to_micros(s as i64, rate), crate::frames_to_micros(e as i64, rate)));
                open = None;
            }
            _ => {}
        }
        i = end;
    }
    if let Some(s) = open {
        out.push((crate::frames_to_micros(s as i64, rate), crate::frames_to_micros(mono.len() as i64, rate)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Speech-like test signal: a 150 Hz buzz with formant-ish harmonics,
    /// amplitude-modulated at syllable rate, gated on and off.
    fn pseudo_speech(rate: u32, secs: f32, on: &[(f32, f32)], noise_amp: f32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        let mut seed = 12345u32;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) * noise_amp;
                let speaking = on.iter().any(|(a, b)| t >= *a && t < *b);
                if !speaking {
                    return noise;
                }
                let syl = 0.6 + 0.4 * (std::f32::consts::TAU * 4.0 * t).sin();
                let mut v = 0.0;
                for h in 1..12 {
                    let f = 150.0 * h as f32 * (1.0 + 0.02 * (t * 3.0).sin());
                    let formant = (-(f - 700.0).powi(2) / 2.0e5).exp() + 0.6 * (-(f - 1500.0).powi(2) / 4.0e5).exp();
                    v += (std::f32::consts::TAU * f * t).sin() * formant / h as f32;
                }
                v * syl * 0.3 + noise
            })
            .collect()
    }

    #[test]
    fn detects_speech_segments_in_noise() {
        let rate = 48_000;
        let x = pseudo_speech(rate, 6.0, &[(1.0, 2.5), (4.0, 5.0)], 0.003);
        let segs = speech_segments(&x, rate, VadParams::default());
        assert_eq!(segs.len(), 2, "{segs:?}");
        let s = |t: f32| (t * 1e6) as Micros;
        assert!((segs[0].0 - s(1.0)).abs() < 120_000, "{:?}", segs[0]);
        assert!((segs[0].1 - s(2.5)).abs() < 150_000, "{:?}", segs[0]);
        assert!((segs[1].0 - s(4.0)).abs() < 120_000, "{:?}", segs[1]);
        assert!((segs[1].1 - s(5.0)).abs() < 150_000, "{:?}", segs[1]);
    }

    #[test]
    fn ignores_silence_and_steady_tones() {
        let rate = 48_000;
        let quiet = vec![0.0f32; rate as usize * 3];
        assert!(speech_segments(&quiet, rate, VadParams::default()).is_empty());
        let tone: Vec<f32> = (0..rate as usize * 3).map(|i| 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin()).collect();
        assert!(speech_segments(&tone, rate, VadParams::default()).is_empty());
        let mut v = Vad::new(rate, VadParams::default());
        v.process(&tone);
        assert!(!v.is_speech());
        assert!(v.probability() < 0.5);
    }
}
