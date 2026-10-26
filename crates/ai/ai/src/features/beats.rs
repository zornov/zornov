// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Beat sync helpers: beat and onset times (via `studio-audio`'s detector)
//! and the arithmetic to put cuts on them. Times are seconds.

/// Beat times of interleaved audio with `channels` channels.
pub fn beat_times(interleaved: &[f32], rate: u32, channels: usize) -> Vec<f64> {
    let mono = mono(interleaved, channels);
    studio_audio::beats::detect_beats_mono(&mono, rate).into_iter().map(|us| us as f64 / 1e6).collect()
}

/// Onset (note attack) times, for cutting on hits rather than the pulse.
pub fn onset_times(interleaved: &[f32], rate: u32, channels: usize) -> Vec<f64> {
    // `detect_onsets` takes interleaved stereo
    let m = mono(interleaved, channels);
    let stereo: Vec<f32> = m.iter().flat_map(|&s| [s, s]).collect();
    studio_audio::beats::detect_onsets(&stereo, rate).into_iter().map(|us| us as f64 / 1e6).collect()
}

fn mono(x: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 { x.to_vec() } else { studio_nn::audio::downmix(x, channels) }
}

/// Tempo in beats per minute from beat times (median interval).
pub fn tempo(beats: &[f64]) -> Option<f64> {
    let mut d: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).filter(|&d| d > 0.0).collect();
    if d.is_empty() {
        return None;
    }
    d.sort_by(f64::total_cmp);
    Some(60.0 / d[d.len() / 2])
}

/// The beat nearest to `t` within `tolerance` seconds.
pub fn nearest_beat(beats: &[f64], t: f64, tolerance: f64) -> Option<f64> {
    let i = beats.partition_point(|&b| b < t);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|k| beats.get(k).copied()).filter(|b| (b - t).abs() <= tolerance).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
}

/// Moves each time to the nearest beat within `tolerance`; others stay.
pub fn snap_to_beats(times: &[f64], beats: &[f64], tolerance: f64) -> Vec<f64> {
    times.iter().map(|&t| nearest_beat(beats, t, tolerance).unwrap_or(t)).collect()
}

/// Every `every`-th beat starting at beat `offset` (e.g. every 4th beat =
/// once per bar in 4/4), as cut points.
pub fn cuts_every(beats: &[f64], every: usize, offset: usize) -> Vec<f64> {
    beats.iter().skip(offset).step_by(every.max(1)).copied().collect()
}

/// Lays clips end to end so every cut lands on a beat: clip `i` gets the
/// span from one cut point to the next, using `every` beats per clip (more
/// when the clip is long enough to cover them, never more than its
/// duration allows). Returns `(start, end)` on the music's timeline per
/// clip; clips that no longer fit before the last beat are left out.
pub fn fit_clips_to_beats(durations: &[f64], beats: &[f64], every: usize) -> Vec<(f64, f64)> {
    let every = every.max(1);
    let mut out = Vec::new();
    let mut b = 0usize;
    for &d in durations {
        if b >= beats.len() {
            break;
        }
        let start = beats[b];
        // the furthest beat (in steps of `every`) the clip can still fill
        let mut e = b + every;
        while e + every < beats.len() && beats[e + every] - start <= d {
            e += every;
        }
        if e >= beats.len() || beats[e] - start > d {
            break;
        }
        out.push((start, beats[e]));
        b = e;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize, period: f64) -> Vec<f64> {
        (0..n).map(|i| i as f64 * period).collect()
    }

    #[test]
    fn snapping_and_tempo() {
        let b = grid(8, 0.5);
        assert_eq!(tempo(&b), Some(120.0));
        assert_eq!(snap_to_beats(&[0.1, 0.74, 2.1, 3.3], &b, 0.15), vec![0.0, 0.74, 2.0, 3.3]);
        assert_eq!(nearest_beat(&b, 1.26, 0.3), Some(1.5));
        assert_eq!(nearest_beat(&[], 1.0, 1.0), None);
        assert_eq!(cuts_every(&b, 4, 1), vec![0.5, 2.5]);
    }

    #[test]
    fn clips_fill_whole_beats() {
        let b = grid(17, 0.5); // 8 s at 120 BPM
        let spans = fit_clips_to_beats(&[1.2, 3.0, 0.4, 2.0], &b, 2);
        // 1.2 s → 1 s (2 beats); 3.0 s → 3 s; 0.4 s cannot fill 2 beats → stop
        assert_eq!(spans, vec![(0.0, 1.0), (1.0, 4.0)]);
        for (s, e) in &spans {
            assert!(b.contains(s) && b.contains(e));
        }
    }

    #[test]
    fn detects_a_click_track() {
        let rate = 22_050;
        let secs = 8.0;
        let n = (rate as f64 * secs) as usize;
        let period = 0.5;
        let mut x = vec![0.0f32; n];
        for k in 0..(secs / period) as usize {
            let s = (k as f64 * period * rate as f64) as usize;
            for i in 0..400.min(n - s) {
                x[s + i] = (i as f32 * 0.3).sin() * (1.0 - i as f32 / 400.0);
            }
        }
        let b = beat_times(&x, rate, 1);
        assert!(b.len() >= 10, "{b:?}");
        let t = tempo(&b).unwrap();
        assert!((t - 120.0).abs() < 3.0, "{t}");
        assert!(!onset_times(&x, rate, 1).is_empty());
    }
}
