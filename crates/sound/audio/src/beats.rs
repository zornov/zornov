// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Beat detection for audio clips ("Auto mark beats").
//!
//! 1. Onset strength: spectral flux of log-compressed STFT band magnitudes
//!    (~46 ms frames, 10 ms hop; 36 log-spaced bands from 30 Hz, so a kick's
//!    few low bins weigh as much as a hi-hat's hundreds of high ones and the
//!    tracker locks to the kick, not the off-beat hats), half-wave
//!    rectified, summed over bands, then high-passed by a moving average and
//!    normalised.
//! 2. Tempo: autocorrelation of the onset strength over 60-200 BPM, weighted
//!    by a log-normal prior around 120 BPM, refined parabolically.
//! 3. Beats: Ellis' dynamic-programming tracker — each frame's score is its
//!    onset strength plus the best predecessor's score less a penalty on the
//!    log ratio of the gap to the beat period; backtracking from the best
//!    final frame gives evenly spaced beats that land on onsets.
//!
//! Times are microseconds from the start of the samples.

pub type Micros = i64;

const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 200.0;

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let a = -std::f32::consts::TAU / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wi, wr) = (a * k as f32).sin_cos();
                let (p, q) = (start + k, start + k + len / 2);
                let tr = re[q] * wr - im[q] * wi;
                let ti = re[q] * wi + im[q] * wr;
                re[q] = re[p] - tr;
                im[q] = im[p] - ti;
                re[p] += tr;
                im[p] += ti;
            }
        }
        len <<= 1;
    }
}

/// Onset strength envelope of a mono signal and its frame rate (frames/s).
/// Frame `i` is centred at sample `i * hop`; an attack peaks the envelope
/// about half a window before the attack itself (see [`LEAD`]).
pub fn onset_envelope(mono: &[f32], rate: u32) -> (Vec<f32>, f64) {
    let (env, _, fps) = envelopes(mono, rate);
    (env, fps)
}

/// Bands the flux is summed over: log-spaced from 30 Hz, so a kick drum
/// (a few low bins) counts as much as a hi-hat (hundreds of high bins).
const BANDS: usize = 36;
/// Bands up to here make the low envelope (kick and bass) that decides
/// which of two half-beat phases is the beat.
const LOW_HZ: f64 = 180.0;

/// The onset envelope over every band and over the low bands alone, with
/// the frame rate: spectral flux of log-compressed band magnitudes (~46 ms
/// frames, 10 ms hop), half-wave rectified, summed over bands, high-passed
/// by a moving mean and normalised.
fn envelopes(mono: &[f32], rate: u32) -> (Vec<f32>, Vec<f32>, f64) {
    let rate_f = rate.max(1) as f64;
    let hop = ((rate_f * 0.01).round() as usize).max(1);
    let n = ((rate_f * 0.046) as usize).next_power_of_two().max(64);
    let fps = rate_f / hop as f64;
    if mono.len() < n {
        return (Vec::new(), Vec::new(), fps);
    }
    let window: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let bins = n / 2;
    // Each bin's band (none below 30 Hz) and how many bins each band holds.
    let (lo, hi) = (30.0f64, (rate_f * 0.45).min(16_000.0));
    let bin_hz = rate_f / n as f64;
    let band_of: Vec<Option<usize>> = (0..bins)
        .map(|k| {
            let hz = k as f64 * bin_hz;
            (hz >= lo && hz < hi).then(|| (((hz / lo).ln() / (hi / lo).ln() * BANDS as f64) as usize).min(BANDS - 1))
        })
        .collect();
    let mut width = [0usize; BANDS];
    for b in band_of.iter().flatten() {
        width[*b] += 1;
    }
    let low_bands = (((LOW_HZ / lo).ln() / (hi / lo).ln() * BANDS as f64).ceil() as usize).clamp(1, BANDS);
    let frames = mono.len() / hop;
    let mut prev = [0f32; BANDS];
    let (mut env, mut low) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let (mut re, mut im) = (vec![0f32; n], vec![0f32; n]);
    for f in 0..frames {
        let centre = f * hop;
        for i in 0..n {
            let at = (centre + i).wrapping_sub(n / 2);
            re[i] = mono.get(at).copied().unwrap_or(0.0) * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut power = [0f32; BANDS];
        for k in 1..bins {
            if let Some(b) = band_of[k] {
                power[b] += re[k] * re[k] + im[k] * im[k];
            }
        }
        let (mut flux, mut flux_low) = (0.0, 0.0);
        for b in 0..BANDS {
            if width[b] == 0 {
                continue;
            }
            let m = (1.0 + 100.0 * (power[b] / width[b] as f32).sqrt()).ln();
            if f > 0 {
                let d = (m - prev[b]).max(0.0);
                flux += d;
                if b < low_bands {
                    flux_low += d;
                }
            }
            prev[b] = m;
        }
        env.push(flux);
        low.push(flux_low);
    }
    (detrend(&env, fps), detrend(&low, fps), fps)
}

/// Removes the slow trend (~0.4 s moving mean), keeps the rises, normalises.
fn detrend(env: &[f32], fps: f64) -> Vec<f32> {
    let w = (0.2 * fps) as usize;
    let mut prefix = vec![0f64; env.len() + 1];
    for (i, v) in env.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *v as f64;
    }
    let hp: Vec<f32> = (0..env.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
            let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
            (env[i] as f64 - mean).max(0.0) as f32
        })
        .collect();
    let sd = (hp.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / hp.len().max(1) as f64).sqrt();
    if sd > 1e-9 { hp.iter().map(|v| (*v as f64 / sd) as f32).collect() } else { vec![0.0; hp.len()] }
}

fn mono_of(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect()
}

/// How far the onset envelope runs ahead of the attacks it marks (half the
/// 46 ms analysis window), added back when frames become times.
pub const LEAD: f64 = 0.023;

fn to_micros(frame: f64, fps: f64) -> Micros {
    ((frame / fps + LEAD) * 1e6).round() as Micros
}

/// Tempo in BPM from an onset envelope, or None when there is no periodicity.
pub fn estimate_tempo(env: &[f32], fps: f64) -> Option<f64> {
    let min_lag = (60.0 * fps / MAX_BPM).floor() as usize;
    let max_lag = (60.0 * fps / MIN_BPM).ceil() as usize;
    if env.len() < max_lag * 2 || env.iter().all(|v| *v == 0.0) {
        return None;
    }
    let ac = |lag: usize| -> f64 { env.iter().zip(&env[lag..]).map(|(a, b)| (*a * *b) as f64).sum::<f64>() / (env.len() - lag) as f64 };
    let zero = ac(0);
    if zero <= 0.0 {
        return None;
    }
    let scores: Vec<f64> = (min_lag..=max_lag)
        .map(|lag| {
            let bpm = 60.0 * fps / lag as f64;
            let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
            ac(lag) * prior
        })
        .collect();
    let (i, best) = scores.iter().enumerate().fold((0, f64::MIN), |b, (i, &s)| if s > b.1 { (i, s) } else { b });
    if best <= 0.05 * zero {
        return None;
    }
    // Parabolic refinement of the peak lag.
    let lag = min_lag as f64 + i as f64;
    let refine = if i > 0 && i + 1 < scores.len() {
        let (a, b, c) = (scores[i - 1], scores[i], scores[i + 1]);
        let d = a - 2.0 * b + c;
        if d.abs() > 1e-12 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 }
    } else {
        0.0
    };
    Some(60.0 * fps / (lag + refine))
}

/// Ellis DP beat tracking over an onset envelope with beat period `period` frames.
fn track(env: &[f32], period: f64) -> Vec<usize> {
    let n = env.len();
    let alpha = 100.0;
    let mut score = vec![0f64; n];
    let mut back = vec![usize::MAX; n];
    let lo = (period / 2.0).round() as usize;
    let hi = (period * 2.0).round() as usize;
    for t in 0..n {
        let mut best = (f64::MIN, usize::MAX);
        if t >= lo {
            for prev in t.saturating_sub(hi)..=t - lo {
                let s = score[prev] - alpha * ((t - prev) as f64 / period).ln().powi(2);
                if s > best.0 {
                    best = (s, prev);
                }
            }
        }
        if best.1 == usize::MAX {
            score[t] = env[t] as f64;
        } else {
            score[t] = env[t] as f64 + best.0;
            back[t] = best.1;
        }
    }
    // End on the best frame within the last period.
    let tail = n.saturating_sub(period.ceil() as usize);
    let mut t = (tail..n).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(0);
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    beats
}

/// Beat times of interleaved stereo `samples` at `rate`. Empty when the clip
/// is too short (under ~2.5 s) or has no steady pulse.
pub fn detect_beats(samples: &[f32], rate: u32) -> Vec<Micros> {
    detect_beats_mono(&mono_of(samples), rate)
}

/// [`detect_beats`] for a mono signal.
pub fn detect_beats_mono(mono: &[f32], rate: u32) -> Vec<Micros> {
    let (env, fps) = onset_envelope(mono, rate);
    let Some(bpm) = estimate_tempo(&env, fps) else { return Vec::new() };
    let period = 60.0 * fps / bpm;
    let beats = track(&env, period);
    // Drop leading/trailing beats that sit in silence.
    let strong = |i: usize| env[i.saturating_sub(2)..(i + 3).min(env.len())].iter().any(|v| *v > 0.3);
    let first = beats.iter().position(|&b| strong(b)).unwrap_or(beats.len());
    let last = beats.iter().rposition(|&b| strong(b)).map(|i| i + 1).unwrap_or(0);
    if first >= last {
        return Vec::new();
    }
    // Each beat snaps to the strongest onset within ±40 ms.
    let r = (0.04 * fps).round() as usize;
    let mut out: Vec<Micros> = beats[first..last]
        .iter()
        .map(|&b| {
            let (a, z) = (b.saturating_sub(r), (b + r + 1).min(env.len()));
            let peak = (a..z).max_by(|&x, &y| env[x].total_cmp(&env[y]).then(y.abs_diff(b).cmp(&x.abs_diff(b)))).unwrap_or(b);
            to_micros(peak as f64, fps)
        })
        .collect();
    out.dedup();
    out
}

/// Onset (note attack) times of interleaved stereo: local maxima of the onset
/// strength above an adaptive threshold, at least 50 ms apart.
pub fn detect_onsets(samples: &[f32], rate: u32) -> Vec<Micros> {
    let (env, fps) = onset_envelope(&mono_of(samples), rate);
    let w = (0.05 * fps).ceil() as usize;
    let gap = (0.05 * fps).ceil() as usize;
    let mut out: Vec<usize> = Vec::new();
    for i in 0..env.len() {
        let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
        let local = &env[a..b];
        let mean = local.iter().sum::<f32>() / local.len() as f32;
        let is_max = local.iter().all(|v| *v <= env[i]);
        if is_max && env[i] > mean + 0.5 && env[i] > 1.0 && out.last().is_none_or(|&l| i - l >= gap) {
            out.push(i);
        }
    }
    out.into_iter().map(|i| to_micros(i as f64, fps)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(bpm: f64, secs: f64, rate: u32, offset: f64) -> Vec<f32> {
        let n = (secs * rate as f64) as usize;
        let mut x = vec![0f32; n * 2];
        let period = 60.0 / bpm;
        let mut t = offset;
        let mut seed = 1u32;
        while t < secs {
            let s = (t * rate as f64) as usize;
            for k in 0..(0.03 * rate as f64) as usize {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                let env = (-(k as f32) / (0.005 * rate as f32)).exp();
                if s + k < n {
                    x[(s + k) * 2] = 0.5 * noise * env;
                    x[(s + k) * 2 + 1] = 0.5 * noise * env;
                }
            }
            t += period;
        }
        x
    }

    #[test]
    fn finds_beats_of_a_click_track() {
        for (bpm, rate) in [(120.0, 48_000), (90.0, 44_100), (150.0, 48_000)] {
            let x = clicks(bpm, 12.0, rate, 0.25);
            let beats = detect_beats(&x, rate);
            let period = (60.0 / bpm * 1e6) as i64;
            assert!(beats.len() as f64 >= 12.0 / (60.0 / bpm) - 3.0, "{bpm}: {} beats", beats.len());
            for b in &beats {
                // Each beat lands on a click (within 25 ms).
                let rel = (b - 250_000).rem_euclid(period);
                let off = rel.min(period - rel);
                assert!(off < 25_000, "{bpm}: beat {b} off by {off}");
            }
            let tempo = estimate_tempo(&onset_envelope(&mono_of(&x), rate).0, 100.0).unwrap();
            assert!((tempo - bpm).abs() < 3.0, "{bpm} vs {tempo}");
        }
    }

    /// A small deterministic noise source for the synthetic songs.
    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        }
    }

    /// A busy, mastered pop arrangement at `bpm` (mono): kick on every
    /// beat, snare on 2 and 4, sixteenth hi-hats, an eighth-note saw bass,
    /// a sustained chord pad and swung off-beat stabs, all driven into a
    /// soft clipper the way a loud master is. The pulse is plain to a
    /// listener, but the onsets are dense and the transients squashed.
    fn song(bpm: f64, secs: f64, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f64) as usize;
        let sr = rate as f32;
        let beat = 60.0 / bpm;
        let mut x = vec![0f32; n];
        let mut rng = Lcg(7);
        let mut add = |at: f64, len: f64, f: &mut dyn FnMut(f32) -> f32, x: &mut [f32]| {
            let s = (at * rate as f64) as usize;
            for k in 0..(len * rate as f64) as usize {
                if s + k < n {
                    x[s + k] += f(k as f32 / sr);
                }
            }
        };
        let beats = (secs / beat) as usize + 1;
        for b in 0..beats {
            let t = b as f64 * beat;
            // Kick: a pitch-dropping sine.
            let mut ph = 0f32;
            add(t, 0.3, &mut |u| {
                ph += std::f32::consts::TAU * (45.0 + 70.0 * (-u / 0.03).exp()) / sr;
                0.9 * ph.sin() * (-u / 0.12).exp()
            }, &mut x);
            if b % 2 == 1 {
                add(t, 0.2, &mut |u| 0.5 * rng.next() * (-u / 0.05).exp(), &mut x);
            }
            // Sixteenth hats.
            for k in 0..4 {
                let acc = if k == 2 { 0.25 } else { 0.12 };
                add(t + k as f64 * beat / 4.0, 0.05, &mut |u| acc * rng.next() * (-u / 0.012).exp(), &mut x);
            }
            // Swung stabs on the off-beat, as loud as the snare.
            add(t + beat * 0.62, 0.15, &mut |u| 0.35 * (std::f32::consts::TAU * 660.0 * u).sin().signum() * (-u / 0.06).exp(), &mut x);
            // Eighth-note bass.
            for k in 0..2 {
                let hz = [55.0f32, 65.4, 73.4, 49.0][(b / 4) % 4];
                add(t + k as f64 * beat / 2.0, beat / 2.0, &mut |u| 0.35 * (2.0 * (u * hz).fract() - 1.0) * (-u / 0.4).exp(), &mut x);
            }
        }
        // A pad that never stops.
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f32 / sr;
            *v += 0.18 * ((std::f32::consts::TAU * 220.0 * t).sin() + (std::f32::consts::TAU * 277.2 * t).sin() + (std::f32::consts::TAU * 329.6 * t).sin());
            *v = (2.5 * *v).tanh() * 0.8;
        }
        x
    }

    /// An upbeat, loudly mastered song has a beat: the tracker finds it
    /// all the way through, on the kick, at the right tempo.
    #[test]
    fn a_busy_mastered_song_at_120_bpm_has_its_beats() {
        for rate in [44_100, 48_000] {
            let x = song(120.0, 20.0, rate);
            let beats = detect_beats_mono(&x, rate);
            assert!(beats.len() >= 36, "{rate}: {} beats in 20 s at 120 bpm", beats.len());
            let gaps: Vec<i64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
            let mut g = gaps.clone();
            g.sort();
            let median = g[g.len() / 2];
            assert!((median - 500_000).abs() < 15_000, "{rate}: median beat gap {median} µs");
            let on_kick = beats.iter().filter(|&&b| {
                let rel = b.rem_euclid(500_000);
                // ±70 ms, the usual tolerance for beat-tracking evaluation.
                rel.min(500_000 - rel) < 70_000
            }).count();
            assert!(on_kick + 1 >= beats.len(), "{rate}: {on_kick} of {} beats on the kick", beats.len());
        }
    }

    #[test]
    fn silence_and_short_clips_have_no_beats() {
        assert!(detect_beats(&vec![0.0; 48_000 * 10 * 2], 48_000).is_empty());
        assert!(detect_beats(&clicks(120.0, 1.0, 48_000, 0.1), 48_000).is_empty());
        assert!(detect_beats(&[], 48_000).is_empty());
    }

    #[test]
    fn onsets_follow_clicks() {
        let x = clicks(100.0, 6.0, 48_000, 0.5);
        let on = detect_onsets(&x, 48_000);
        assert_eq!(on.len(), 10, "{on:?}");
        assert!((on[0] - 500_000).abs() < 20_000, "{}", on[0]);
    }
}
