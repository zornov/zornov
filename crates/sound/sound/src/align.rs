// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lining a new recording up with a guide: an ADR take with the production
//! line, a dubbed line with the original performance.
//!
//! - [`offset`] finds the single shift that best lines the take up
//!   (cross-correlation of the two loudness envelopes).
//! - [`warp_map`] follows the take's timing against the guide's word by
//!   word (dynamic time warping over band-energy features), and
//!   [`conform`] stretches the take piece by piece (WSOLA, pitch kept) so
//!   its syllables land where the guide's do, which is what keeps lips in
//!   sync.
//!
//! Features are 10 ms frames of log energy in twelve bands, normalised per
//! band over the whole signal so a different microphone or voice does not
//! decide the match; what is matched is where energy starts, stops and
//! moves.

use crate::fft::{Fft, hann};

/// Feature frame hop in seconds.
pub const HOP_SECS: f32 = 0.01;
const BANDS: usize = 12;

/// Band-energy features, one row per 10 ms.
fn features(x: &[f32], rate: u32) -> Vec<[f32; BANDS]> {
    let hop = ((rate as f32 * HOP_SECS) as usize).max(1);
    let n = 1024usize.min((rate as usize / 20).next_power_of_two()).max(256);
    let fft = Fft::new(n);
    let mut win = vec![0.0f32; n];
    hann(&mut win);
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let frames = x.len().div_ceil(hop);
    let hz = rate as f32 / n as f32;
    // Log-spaced band edges 80 Hz .. 8 kHz.
    let edges: Vec<usize> = (0..=BANDS).map(|b| ((80.0 * 100f32.powf(b as f32 / BANDS as f32)) / hz) as usize).map(|k| k.clamp(1, n / 2 - 1)).collect();
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let c = f * hop;
        for k in 0..n {
            let i = (c + k).wrapping_sub(n / 2);
            re[k] = if i < x.len() { x[i] * win[k] } else { 0.0 };
            im[k] = 0.0;
        }
        fft.forward(&mut re, &mut im);
        let mut row = [0.0f32; BANDS];
        for b in 0..BANDS {
            let (k0, k1) = (edges[b], edges[b + 1].max(edges[b] + 1));
            let mut e = 0.0;
            for k in k0..k1 {
                e += re[k] * re[k] + im[k] * im[k];
            }
            row[b] = (e / (k1 - k0) as f32 + 1e-10).ln();
        }
        out.push(row);
    }
    // Per-band normalisation (mean 0, unit deviation).
    for b in 0..BANDS {
        let m = out.iter().map(|r| r[b]).sum::<f32>() / out.len().max(1) as f32;
        let v = out.iter().map(|r| (r[b] - m) * (r[b] - m)).sum::<f32>() / out.len().max(1) as f32;
        let s = v.sqrt().max(1e-3);
        for r in &mut out {
            r[b] = (r[b] - m) / s;
        }
    }
    out
}

/// Loudness envelope per 10 ms (mean of the normalised features).
fn envelope(f: &[[f32; BANDS]]) -> Vec<f32> {
    f.iter().map(|r| r.iter().sum::<f32>() / BANDS as f32).collect()
}

/// The shift (in frames at `rate`) to add to the take's position so it
/// lines up with the guide, searched within ±`max_shift_secs`, and how
/// clearly it matched (correlation 0..1).
pub fn offset(guide: &[f32], take: &[f32], rate: u32, max_shift_secs: f32) -> (i64, f32) {
    let g = envelope(&features(guide, rate));
    let t = envelope(&features(take, rate));
    let max = (max_shift_secs / HOP_SECS) as i64;
    let mut best = (0i64, f32::MIN);
    for lag in -max..=max {
        // Take frame j sits at guide frame j + lag.
        let mut s = 0.0f32;
        let mut ng = 0.0f32;
        let mut nt = 0.0f32;
        for (j, tv) in t.iter().enumerate() {
            let i = j as i64 + lag;
            if i < 0 || i as usize >= g.len() {
                continue;
            }
            let gv = g[i as usize];
            s += gv * tv;
            ng += gv * gv;
            nt += tv * tv;
        }
        let c = if ng > 0.0 && nt > 0.0 { s / (ng.sqrt() * nt.sqrt()) } else { 0.0 };
        if c > best.1 {
            best = (lag, c);
        }
    }
    let hop = (rate as f32 * HOP_SECS) as i64;
    (best.0 * hop, best.1.max(0.0))
}

/// A monotonic time map from take time to guide time: `(take_secs,
/// guide_secs)` anchor points, first at (0, 0)-ish, last at the two ends.
#[derive(Clone, Debug, PartialEq)]
pub struct WarpMap {
    /// Anchor points.
    pub points: Vec<(f32, f32)>,
    /// Mean feature distance along the path (lower = closer match).
    pub cost: f32,
}

/// Dynamic time warping of the take onto the guide. `band_secs` limits how
/// far the paths may drift apart (a Sakoe-Chiba band; 0.6 s is plenty for
/// ADR). Anchors are thinned to about one per `anchor_secs`.
pub fn warp_map(guide: &[f32], take: &[f32], rate: u32, band_secs: f32, anchor_secs: f32) -> WarpMap {
    let g = features(guide, rate);
    let t = features(take, rate);
    let (n, m) = (t.len(), g.len());
    if n == 0 || m == 0 {
        return WarpMap { points: vec![(0.0, 0.0)], cost: 0.0 };
    }
    let band = ((band_secs / HOP_SECS) as usize).max(n.abs_diff(m) + 2);
    let dist = |i: usize, j: usize| -> f32 { t[i].iter().zip(&g[j]).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt() };
    // Row i covers guide frames around the diagonal.
    let centre = |i: usize| (i as f64 * m as f64 / n as f64) as i64;
    let lo = |i: usize| (centre(i) - band as i64).max(0) as usize;
    let hi = |i: usize| ((centre(i) + band as i64) as usize).min(m - 1);
    let inf = f32::INFINITY;
    let mut cost: Vec<Vec<f32>> = Vec::with_capacity(n);
    let mut from: Vec<Vec<u8>> = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (lo(i), hi(i));
        let mut row = vec![inf; b - a + 1];
        let mut dir = vec![0u8; b - a + 1];
        for j in a..=b {
            let d = dist(i, j);
            let get = |ii: usize, jj: usize, cost: &Vec<Vec<f32>>| -> f32 {
                let (a2, b2) = (lo(ii), hi(ii));
                if jj < a2 || jj > b2 { inf } else { cost[ii][jj - a2] }
            };
            let (c, k) = if i == 0 && j == 0 {
                (0.0, 0)
            } else {
                let diag = if i > 0 && j > 0 { get(i - 1, j - 1, &cost) } else { inf };
                let up = if i > 0 { get(i - 1, j, &cost) } else { inf };
                let left = if j > a { row[j - a - 1] } else { inf };
                // Slight preference for the diagonal keeps the path from
                // wandering through silence.
                let opts = [(diag, 0u8), (up + 0.1, 1u8), (left + 0.1, 2u8)];
                opts.iter().copied().fold((inf, 0u8), |acc, o| if o.0 < acc.0 { o } else { acc })
            };
            row[j - a] = c + d;
            dir[j - a] = k;
        }
        cost.push(row);
        from.push(dir);
    }
    // Back-track from the corner.
    let mut path = Vec::new();
    let (mut i, mut j) = (n - 1, m - 1);
    let total = if j >= lo(i) && j <= hi(i) { cost[i][j - lo(i)] } else { inf };
    loop {
        path.push((i, j));
        if i == 0 && j == 0 {
            break;
        }
        let k = if j >= lo(i) && j <= hi(i) { from[i][j - lo(i)] } else { 0 };
        match k {
            0 if i > 0 && j > 0 => {
                i -= 1;
                j -= 1;
            }
            1 if i > 0 => i -= 1,
            _ if j > 0 => j -= 1,
            _ => i -= 1,
        }
    }
    path.reverse();
    let step = ((anchor_secs / HOP_SECS) as usize).max(1);
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut last_take = usize::MAX;
    for (k, &(ti, gj)) in path.iter().enumerate() {
        if k == 0 || k + 1 == path.len() || (ti % step == 0 && ti != last_take) {
            let p = (ti as f32 * HOP_SECS, gj as f32 * HOP_SECS);
            if points.last().is_none_or(|q| p.0 > q.0 && p.1 > q.1) || k + 1 == path.len() {
                points.push(p);
            }
            last_take = ti;
        }
    }
    // Ends land on the true signal lengths.
    if let Some(last) = points.last_mut() {
        *last = (take.len() as f32 / rate as f32, guide.len() as f32 / rate as f32);
    }
    points.dedup_by(|b, a| b.0 <= a.0 || b.1 <= a.1);
    WarpMap { points, cost: if total.is_finite() { total / path.len() as f32 } else { f32::INFINITY } }
}

/// Stretches `take` piece by piece so it follows `map` onto the guide's
/// timeline (pitch kept). Each piece's factor is held within
/// `[1/max_ratio, max_ratio]`. Pieces are joined with 8 ms crossfades.
pub fn conform(take: &[f32], rate: u32, map: &WarpMap, max_ratio: f32) -> Vec<f32> {
    let total = map.points.last().map_or(0.0, |p| p.1);
    let want = (total * rate as f32).round() as usize;
    let mut out = vec![0.0f32; want + rate as usize / 10];
    let fade = (rate as usize * 8 / 1000).max(1);
    for w in map.points.windows(2) {
        let (t0, g0) = w[0];
        let (t1, g1) = w[1];
        let (a, b) = ((t0 * rate as f32) as usize, ((t1 * rate as f32) as usize).min(take.len()));
        if b <= a {
            continue;
        }
        let dur_g = (g1 - g0).max(0.0);
        let factor = (dur_g / (t1 - t0).max(1e-4)).clamp(1.0 / max_ratio, max_ratio) as f64;
        // A little extra on each side for the crossfade.
        let a2 = a.saturating_sub(fade);
        let b2 = (b + fade).min(take.len());
        let piece = &take[a2..b2];
        let (st, _) = crate::pitch::stretch(piece, piece, rate, factor);
        let lead = ((a - a2) as f64 * factor) as usize;
        let tail = ((b2 - b) as f64 * factor) as usize;
        let start = (g0 * rate as f32) as usize;
        for (k, v) in st.iter().enumerate() {
            let pos = (start + k) as i64 - lead as i64;
            if pos < 0 || pos as usize >= out.len() {
                continue;
            }
            // Linear crossfades over the overlap on each side: the extra
            // input before `a` and after `b` (none at the signal's ends).
            let fin = if lead == 0 { 1.0 } else { (k as f32 / (2 * lead) as f32).min(1.0) };
            let fout = if tail == 0 { 1.0 } else { ((st.len() - k) as f32 / (2 * tail) as f32).min(1.0) };
            out[pos as usize] += v * fin.min(fout);
        }
    }
    out.truncate(want);
    out
}

/// Fits a line to `target_frames` by one uniform stretch, held within
/// `[1/max_ratio, max_ratio]`; returns the audio and the factor used. A
/// factor at the limit means the line will not fit and needs rewording.
pub fn fit(line: &[f32], rate: u32, target_frames: usize, max_ratio: f32) -> (Vec<f32>, f64) {
    if line.is_empty() || target_frames == 0 {
        return (line.to_vec(), 1.0);
    }
    let want = target_frames as f64 / line.len() as f64;
    let f = want.clamp(1.0 / max_ratio as f64, max_ratio as f64);
    if (f - 1.0).abs() < 0.01 {
        return (line.to_vec(), 1.0);
    }
    let (out, _) = crate::pitch::stretch(line, line, rate, f);
    (out, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// Syllable-like tone bursts at given times (s) and pitches.
    fn syllables(at: &[(f32, f32, f32)], len_secs: f32) -> Vec<f32> {
        let mut x = vec![0.0f32; (len_secs * RATE as f32) as usize];
        for &(t, dur, f) in at {
            let a = (t * RATE as f32) as usize;
            let n = (dur * RATE as f32) as usize;
            for i in 0..n {
                if a + i < x.len() {
                    let env = (std::f32::consts::PI * i as f32 / n as f32).sin();
                    let tt = i as f32 / RATE as f32;
                    x[a + i] += 0.4 * env * ((std::f32::consts::TAU * f * tt).sin() + 0.5 * (std::f32::consts::TAU * 2.0 * f * tt).sin());
                }
            }
        }
        x
    }

    fn onsets(x: &[f32]) -> Vec<f32> {
        let hop = RATE as usize / 100;
        let mut out = Vec::new();
        let mut on = false;
        for (k, c) in x.chunks(hop).enumerate() {
            let e = c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32;
            if !on && e > 0.002 {
                out.push(k as f32 * 0.01);
                on = true;
            } else if on && e < 0.0005 {
                on = false;
            }
        }
        out
    }

    #[test]
    fn offset_finds_a_late_take() {
        let words = [(0.3, 0.2, 300.0), (0.7, 0.15, 420.0), (1.1, 0.3, 250.0), (1.7, 0.2, 380.0)];
        let guide = syllables(&words, 2.5);
        let late: Vec<(f32, f32, f32)> = words.iter().map(|w| (w.0 + 0.23, w.1, w.2 * 1.1)).collect();
        let take = syllables(&late, 2.5);
        let (shift, conf) = offset(&guide, &take, RATE, 1.0);
        let secs = shift as f32 / RATE as f32;
        assert!((secs + 0.23).abs() <= 0.015, "shift {secs}");
        assert!(conf > 0.5, "confidence {conf}");
    }

    #[test]
    fn warp_and_conform_put_syllables_where_the_guide_has_them() {
        let words = [(0.2, 0.2, 300.0), (0.6, 0.15, 420.0), (1.0, 0.3, 250.0), (1.6, 0.2, 380.0), (2.1, 0.2, 330.0)];
        let guide = syllables(&words, 2.6);
        // The actor rushed the middle and dragged the end.
        let take_words = [(0.25, 0.2, 310.0), (0.55, 0.15, 430.0), (0.85, 0.3, 260.0), (1.55, 0.2, 390.0), (2.25, 0.2, 340.0)];
        let take = syllables(&take_words, 2.8);
        let map = warp_map(&guide, &take, RATE, 0.6, 0.1);
        assert!(map.points.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 > w[0].1), "monotonic {:?}", map.points);
        let fixed = conform(&take, RATE, &map, 2.0);
        assert_eq!(fixed.len(), guide.len());
        let want = onsets(&guide);
        let got = onsets(&fixed);
        assert_eq!(want.len(), got.len(), "{want:?} vs {got:?}");
        for (a, b) in want.iter().zip(&got) {
            assert!((a - b).abs() <= 0.04, "syllable at {a} landed at {b}: {got:?}");
        }
        let before = onsets(&take);
        let err_before: f32 = want.iter().zip(&before).map(|(a, b)| (a - b).abs()).sum();
        let err_after: f32 = want.iter().zip(&got).map(|(a, b)| (a - b).abs()).sum();
        assert!(err_after < err_before / 3.0, "{err_before} -> {err_after}");
    }

    #[test]
    fn fit_stretches_within_limits() {
        let x = syllables(&[(0.1, 0.5, 300.0)], 1.0);
        let (y, f) = fit(&x, RATE, (1.2 * RATE as f32) as usize, 1.25);
        assert!((f - 1.2).abs() < 1e-6);
        assert_eq!(y.len(), (1.2 * RATE as f32) as usize);
        let (_, f) = fit(&x, RATE, (2.0 * RATE as f32) as usize, 1.25);
        assert!((f - 1.25).abs() < 1e-6, "held at the limit");
    }
}
