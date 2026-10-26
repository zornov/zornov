// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytic test signals and simple measurements.

use std::f64::consts::PI;

/// A sine at `hz`, peak `amp`, `frames` samples long, mono.
pub fn sine(hz: f64, amp: f32, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (amp as f64 * (2.0 * PI * hz * i as f64 / rate as f64).sin()) as f32).collect()
}

/// The ticking tone used in fixtures: a 440 Hz sine at `tone_db` dBFS with a
/// 2 ms full-scale click at every whole second. Mono.
pub fn ticking_tone(rate: u32, seconds: f64, tone_db: f32) -> Vec<f32> {
    let frames = (rate as f64 * seconds).round() as usize;
    let amp = db_to_gain(tone_db);
    let mut v = sine(440.0, amp, rate, frames);
    let click_len = (rate as f64 * 0.002).round() as usize;
    let mut t = 0usize;
    while t < frames {
        for i in 0..click_len.min(frames - t) {
            // Alternating full-scale samples: unmistakable in any meter.
            v[t + i] = if i % 2 == 0 { 0.95 } else { -0.95 };
        }
        t += rate as usize;
    }
    v
}

/// Interleaves mono into `channels` identical channels.
pub fn interleave(mono: &[f32], channels: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(mono.len() * channels);
    for &s in mono {
        for _ in 0..channels {
            out.push(s);
        }
    }
    out
}

/// Takes one channel out of interleaved samples.
pub fn deinterleave(interleaved: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    interleaved.chunks_exact(channels.max(1)).map(|f| f[channel.min(channels - 1)]).collect()
}

/// Root mean square.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / samples.len() as f64).sqrt() as f32
}

/// RMS of each whole second (the last partial second is included).
pub fn rms_per_second(mono: &[f32], rate: u32) -> Vec<f32> {
    mono.chunks(rate as usize).map(rms).collect()
}

/// Peak absolute sample.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// dB → linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear gain → dB (−200 for zero).
pub fn gain_to_db(g: f32) -> f32 {
    if g <= 0.0 { -200.0 } else { 20.0 * g.log10() }
}

/// Signal-to-noise ratio in dB of `got` against `want` (same length).
pub fn snr_db(want: &[f32], got: &[f32]) -> f32 {
    let n = want.len().min(got.len());
    let sig: f64 = want[..n].iter().map(|s| (*s as f64).powi(2)).sum();
    let err: f64 = want[..n].iter().zip(&got[..n]).map(|(a, b)| ((*a - *b) as f64).powi(2)).sum();
    if err <= 0.0 { 200.0 } else { (10.0 * (sig / err).log10()) as f32 }
}

/// Magnitude in dB of a single frequency in `x` (Goertzel), relative to a
/// full-scale sine.
pub fn tone_level_db(x: &[f32], hz: f64, rate: u32) -> f32 {
    let w = 2.0 * PI * hz / rate as f64;
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    let c = 2.0 * w.cos();
    for &v in x {
        let s0 = v as f64 + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let re = s1 - s2 * w.cos();
    let im = s2 * w.sin();
    let mag = (re * re + im * im).sqrt() * 2.0 / x.len().max(1) as f64;
    gain_to_db(mag as f32)
}

/// Sample positions of clicks (samples above `threshold` after a quiet
/// neighbourhood), useful to find the fixture ticks after decoding.
pub fn find_clicks(mono: &[f32], threshold: f32, min_gap: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut last: Option<usize> = None;
    for (i, &s) in mono.iter().enumerate() {
        if s.abs() >= threshold && last.is_none_or(|l| i - l >= min_gap) {
            out.push(i);
            last = Some(i);
        }
    }
    out
}

/// A WAV file (16-bit PCM) of interleaved samples.
pub fn wav_bytes(interleaved: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    let w = studio_media::wav::Wav { sample_rate: rate, channels, samples: interleaved.to_vec() };
    studio_media::wav::encode_pcm16(&w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_rms_is_amp_over_root_two() {
        let s = sine(1000.0, 1.0, 48_000, 48_000);
        assert!((rms(&s) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!((tone_level_db(&s, 1000.0, 48_000)).abs() < 0.05);
    }

    #[test]
    fn ticking_tone_has_a_click_each_second() {
        let t = ticking_tone(8000, 3.0, -12.0);
        let clicks = find_clicks(&t, 0.9, 4000);
        assert_eq!(clicks, vec![0, 8000, 16000]);
    }
}
