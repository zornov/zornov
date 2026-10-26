// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dialogue editing helpers: phrase-by-phrase leveling (what a dialogue
//! editor does riding clip gain) and voice matching between takes (EQ,
//! level and room so an ADR line or a pickup sits with the production
//! sound).
//!
//! Both work offline on a mono signal and return settings, not audio: the
//! leveler gives gain keys for the clip's volume automation and the
//! matcher gives EQ band gains, a level offset and a reverb amount, so the
//! result stays editable in the timeline.

use crate::Micros;
use crate::fft::{Fft, hann};
use crate::loudness::KWeighting;
use crate::vad::{VadParams, speech_segments};

/// Settings of the dialogue leveler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelerParams {
    /// Loudness each phrase is brought to, LUFS (the K-weighted level over
    /// the phrase). Film dialogue sits around −27..−24; online video −16.
    pub target_lufs: f32,
    /// Largest boost, dB (quiet lines are not lifted into the noise).
    pub max_boost_db: f32,
    /// Largest cut, dB.
    pub max_cut_db: f32,
    /// Pauses shorter than this join two phrases into one, ms.
    pub join_ms: f32,
    /// Changes smaller than this are left alone, dB.
    pub deadband_db: f32,
}

impl Default for LevelerParams {
    fn default() -> Self {
        Self { target_lufs: -24.0, max_boost_db: 10.0, max_cut_db: 12.0, join_ms: 350.0, deadband_db: 0.5 }
    }
}

/// One phrase the leveler found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phrase {
    /// Start, relative to the signal start.
    pub start: Micros,
    /// End.
    pub end: Micros,
    /// Its loudness before leveling, LUFS.
    pub lufs: f32,
    /// The gain that brings it to the target, dB.
    pub gain_db: f32,
}

/// K-weighted loudness of a span (no gating: a phrase is all speech).
pub fn span_lufs(mono: &[f32], rate: u32) -> f32 {
    if mono.is_empty() {
        return f32::NEG_INFINITY;
    }
    let mut k = KWeighting::new(rate);
    let mut sum = 0.0f64;
    for &x in mono {
        let y = k.tick(0, x) as f64;
        sum += y * y;
    }
    // Mono heard from both speakers, as BS.1770 sums two equal channels.
    let p = 2.0 * sum / mono.len() as f64;
    (-0.691 + 10.0 * p.max(1e-30).log10()) as f32
}

/// The speech phrases of `mono` with the gain each needs. Phrases come from
/// the speech detector with short pauses joined.
pub fn phrases(mono: &[f32], rate: u32, p: LevelerParams) -> Vec<Phrase> {
    let segs = speech_segments(mono, rate, VadParams::default());
    let join = (p.join_ms * 1000.0) as Micros;
    let mut merged: Vec<(Micros, Micros)> = Vec::new();
    for (s, e) in segs {
        match merged.last_mut() {
            Some(last) if s - last.1 <= join => last.1 = e,
            _ => merged.push((s, e)),
        }
    }
    merged
        .into_iter()
        .filter(|(s, e)| e - s >= 120_000)
        .map(|(s, e)| {
            let a = crate::micros_to_frames(s, rate).max(0) as usize;
            let b = (crate::micros_to_frames(e, rate) as usize).min(mono.len());
            let lufs = span_lufs(&mono[a.min(b)..b], rate);
            let mut gain = (p.target_lufs - lufs).clamp(-p.max_cut_db, p.max_boost_db);
            if gain.abs() < p.deadband_db || !lufs.is_finite() {
                gain = 0.0;
            }
            Phrase { start: s, end: e, lufs, gain_db: gain }
        })
        .collect()
}

/// Gain keys `(time, dB)` for a clip's volume automation that level each
/// phrase: the gain holds over a phrase and moves across the pause to the
/// next one. `base_db` is the clip's existing volume, added to every key.
pub fn level_keys(phrases: &[Phrase], base_db: f32) -> Vec<(Micros, f32)> {
    let mut keys = Vec::new();
    for ph in phrases {
        keys.push((ph.start, base_db + ph.gain_db));
        keys.push((ph.end, base_db + ph.gain_db));
    }
    keys
}

/// Octave-ish bands matching the clip EQ's five bands (low shelf, low mid,
/// mid, presence, air), in Hz.
pub const MATCH_BANDS: [(f32, f32); 5] = [(60.0, 160.0), (160.0, 500.0), (500.0, 2000.0), (2000.0, 6000.0), (6000.0, 16000.0)];

/// The tonal and level fingerprint of a voice recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceProfile {
    /// Band levels in dB relative to the mean of the five.
    pub bands_db: [f32; 5],
    /// Loudness over speech, LUFS.
    pub lufs: f32,
    /// How much sound lingers after phrases end relative to the phrase
    /// level, dB (more negative = drier room).
    pub tail_db: f32,
}

/// The fingerprint of `mono`, measured over its speech only.
pub fn profile(mono: &[f32], rate: u32) -> VoiceProfile {
    let segs = speech_segments(mono, rate, VadParams::default());
    let spans: Vec<(usize, usize)> = if segs.is_empty() {
        vec![(0, mono.len())]
    } else {
        segs.iter().map(|&(s, e)| (crate::micros_to_frames(s, rate).max(0) as usize, (crate::micros_to_frames(e, rate) as usize).min(mono.len()))).collect()
    };
    // Long-term average spectrum over the speech.
    let n = 2048;
    let fft = Fft::new(n);
    let mut win = vec![0.0f32; n];
    hann(&mut win);
    let mut power = vec![0.0f64; n / 2];
    let mut frames = 0usize;
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    for &(a, b) in &spans {
        let mut i = a;
        while i + n <= b {
            for k in 0..n {
                re[k] = mono[i + k] * win[k];
                im[k] = 0.0;
            }
            fft.forward(&mut re, &mut im);
            for k in 0..n / 2 {
                power[k] += (re[k] * re[k] + im[k] * im[k]) as f64;
            }
            frames += 1;
            i += n / 2;
        }
    }
    let mut bands_db = [0.0f32; 5];
    if frames > 0 {
        let hz = rate as f32 / n as f32;
        for (bi, (lo, hi)) in MATCH_BANDS.iter().enumerate() {
            let (k0, k1) = (((lo / hz) as usize).max(1), ((hi / hz) as usize).min(n / 2 - 1));
            let e: f64 = power[k0..=k1.max(k0)].iter().sum::<f64>() / (k1.max(k0) - k0 + 1) as f64;
            bands_db[bi] = (10.0 * e.max(1e-20).log10()) as f32;
        }
        let mean = bands_db.iter().sum::<f32>() / 5.0;
        for b in &mut bands_db {
            *b -= mean;
        }
    }
    let speech: Vec<f32> = spans.iter().flat_map(|&(a, b)| mono[a.min(b)..b].iter().copied()).collect();
    let lufs = span_lufs(&speech, rate);
    // Room: energy 60–260 ms after each phrase end against the phrase's
    // last 200 ms.
    let ms = |m: usize| m * rate as usize / 1000;
    let mut tails = Vec::new();
    for &(a, b) in &spans {
        if b < a + ms(200) || b + ms(260) > mono.len() {
            continue;
        }
        let e = |x: &[f32]| x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len().max(1) as f64;
        let body = e(&mono[b - ms(200)..b]);
        let tail = e(&mono[b + ms(60)..b + ms(260)]);
        if body > 1e-12 {
            tails.push((10.0 * (tail.max(1e-20) / body).log10()) as f32);
        }
    }
    let tail_db = if tails.is_empty() { -60.0 } else { tails.iter().sum::<f32>() / tails.len() as f32 };
    VoiceProfile { bands_db, lufs, tail_db }
}

/// What it takes to make a take sound like the reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceMatch {
    /// EQ gains for the five clip EQ bands (bass, low_mid, mid, presence,
    /// air), dB, ±12.
    pub eq_db: [f32; 5],
    /// Level change, dB.
    pub gain_db: f32,
    /// Reverb mix to add (0..0.5) when the reference room is audibly
    /// livelier than the take's; 0 when the take is as live or livelier.
    pub reverb_mix: f32,
}

/// Matches `take` to `reference` (both fingerprints from [`profile`]).
pub fn match_voice(reference: &VoiceProfile, take: &VoiceProfile) -> VoiceMatch {
    let mut eq = [0.0f32; 5];
    for i in 0..5 {
        // Half-way corrections on the extremes: the bands are wide and a
        // full correction of a narrow resonance would overshoot.
        let d = reference.bands_db[i] - take.bands_db[i];
        eq[i] = (d * 0.8).clamp(-12.0, 12.0);
        if eq[i].abs() < 0.5 {
            eq[i] = 0.0;
        }
    }
    let gain = if reference.lufs.is_finite() && take.lufs.is_finite() { (reference.lufs - take.lufs).clamp(-24.0, 24.0) } else { 0.0 };
    let live = reference.tail_db - take.tail_db;
    let reverb_mix = if live > 3.0 { (live / 40.0).clamp(0.05, 0.5) } else { 0.0 };
    VoiceMatch { eq_db: eq, gain_db: gain, reverb_mix }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// Speech-like bursts: a buzz with formant-ish harmonics, `amp` loud,
    /// in phrases of `on` seconds separated by `off` seconds of near silence.
    fn talk(amps: &[f32], on: f32, off: f32, tilt: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut seed = 1u32;
        for &a in amps {
            out.extend(std::iter::repeat_n(0.0005, (off * RATE as f32) as usize));
            let n = (on * RATE as f32) as usize;
            for i in 0..n {
                let t = i as f32 / RATE as f32;
                let mut s = 0.0;
                for h in 1..30 {
                    let f = 140.0 * h as f32;
                    s += (std::f32::consts::TAU * f * t).sin() / (h as f32).powf(1.0 - tilt);
                }
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let env = (std::f32::consts::PI * i as f32 / n as f32).sin().powf(0.3) * (1.0 + 0.3 * (t * 9.0).sin());
                out.push(a * env * (0.12 * s + 0.05 * noise * tilt.max(0.0)));
            }
        }
        out.extend(std::iter::repeat_n(0.0005, RATE as usize / 2));
        out
    }

    #[test]
    fn the_leveler_brings_quiet_and_loud_phrases_to_the_target() {
        let x = talk(&[0.05, 0.6, 0.15], 1.2, 0.8, 0.0);
        let p = LevelerParams { target_lufs: -24.0, max_boost_db: 30.0, max_cut_db: 30.0, ..Default::default() };
        let ph = phrases(&x, RATE, p);
        assert_eq!(ph.len(), 3, "{ph:?}");
        assert!(ph[0].gain_db > ph[2].gain_db && ph[2].gain_db > ph[1].gain_db, "{ph:?}");
        for f in &ph {
            let a = crate::micros_to_frames(f.start, RATE) as usize;
            let b = crate::micros_to_frames(f.end, RATE) as usize;
            let g = crate::db_to_gain(f.gain_db);
            let lev: Vec<f32> = x[a..b].iter().map(|v| v * g).collect();
            let l = span_lufs(&lev, RATE);
            assert!((l + 24.0).abs() < 0.3, "phrase at {} reads {l}", f.start);
        }
        let keys = level_keys(&ph, -2.0);
        assert_eq!(keys.len(), 6);
        assert!((keys[0].1 - (ph[0].gain_db - 2.0)).abs() < 1e-6);
    }

    #[test]
    fn limits_hold() {
        let x = talk(&[0.003], 1.0, 0.5, 0.0);
        let ph = phrases(&x, RATE, LevelerParams { max_boost_db: 6.0, ..Default::default() });
        assert!(ph.iter().all(|p| p.gain_db <= 6.0), "{ph:?}");
    }

    #[test]
    fn matching_a_dull_take_to_a_bright_reference_boosts_the_highs() {
        let bright = talk(&[0.3, 0.3], 1.5, 0.6, 0.6);
        let mut dull = talk(&[0.1, 0.1], 1.5, 0.6, 0.6);
        // Dull it with a gentle one-pole low-pass.
        let mut y = 0.0f32;
        for v in &mut dull {
            y += 0.15 * (*v - y);
            *v = y;
        }
        let m = match_voice(&profile(&bright, RATE), &profile(&dull, RATE));
        assert!(m.eq_db[4] > 3.0 && m.eq_db[3] > 1.0, "{m:?}");
        assert!(m.eq_db[4] > m.eq_db[0], "{m:?}");
        assert!(m.gain_db > 3.0, "the quieter take is turned up: {m:?}");
        // A take matched to itself needs nothing.
        let p = profile(&bright, RATE);
        let same = match_voice(&p, &p);
        assert_eq!(same.eq_db, [0.0; 5]);
        assert_eq!(same.gain_db, 0.0);
        assert_eq!(same.reverb_mix, 0.0);
    }
}
