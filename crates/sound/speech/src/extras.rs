// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The rest of a speaking face: blinks that fall at the edges of phrases,
//! brows that lift on stressed, loud words, and small eye movements.
//! Deterministic for a given seed.

use crate::phoneme::{Ph, Timed};

/// How long a blink lasts, closing to open.
pub const BLINK: f64 = 0.22;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xd1b5_4a32_d192_ed03)
    }
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// How shut the eyes are `t` seconds after a blink starts (0 open, 1 shut).
pub fn lid(t: f64) -> f64 {
    if !(0.0..BLINK).contains(&t) {
        0.0
    } else if t < 0.07 {
        let k = t / 0.07;
        k * k
    } else if t < 0.1 {
        1.0
    } else {
        let k = (t - 0.1) / 0.12;
        1.0 - k * (2.0 - k)
    }
}

/// Starts of blinks over `dur` seconds, about every `every` seconds,
/// pulled to the nearest pause or word edge in `phones` within half a
/// second (people blink between phrases).
pub fn blinks(seed: u64, dur: f64, every: f64, phones: &[Timed]) -> Vec<f64> {
    let mut r = Rng::new(seed);
    // Candidate moments: the starts of silences and of words.
    let mut edges: Vec<f64> = Vec::new();
    for (i, p) in phones.iter().enumerate() {
        if p.ph == Ph::Sil && p.dur() > 0.12 {
            edges.push(p.start + 0.03);
        } else if p.word.is_some() && (i == 0 || phones[i - 1].word != p.word) {
            edges.push(p.start - 0.05);
        }
    }
    let mut out: Vec<f64> = Vec::new();
    let mut t = 0.5 + 1.5 * r.next();
    while t < dur - BLINK {
        let snapped = edges.iter().copied().filter(|e| (e - t).abs() < 0.5 && *e >= 0.0 && *e < dur - BLINK).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).unwrap_or(t);
        out.push(snapped);
        if r.next() < 0.15 && snapped + 0.35 < dur - BLINK {
            out.push(snapped + 0.3);
        }
        t += every * (0.55 + 0.9 * r.next());
    }
    out.sort_by(f64::total_cmp);
    out.dedup_by(|b, a| *b - *a < 0.28);
    out
}

/// Brow lifts `(time, amount 0..1)` on the stressed vowel of loud words,
/// and a small one where a phrase starts. `loud` is 100 frames a second.
pub fn brows(phones: &[Timed], loud: &[f32]) -> Vec<(f64, f64)> {
    let level = |t: f64| loud.get((t * 100.0) as usize).copied().unwrap_or(0.0) as f64;
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut after_pause = true;
    for p in phones {
        if p.ph == Ph::Sil {
            after_pause = p.dur() > 0.2;
            continue;
        }
        let c = (p.start + p.end) / 2.0;
        if p.ph.is_vowel() && p.stress == 1 {
            let l = level(c);
            let a = (if after_pause { 0.45 } else { 0.0f64 }).max(((l - 0.8) * 3.0).clamp(0.0, 0.8));
            if a > 0.2 && out.last().is_none_or(|q| c - q.0 > 0.45) {
                out.push((c - 0.05, a));
            }
        }
        if p.ph.is_vowel() {
            after_pause = false;
        }
    }
    out
}

/// Small eye movements: `(time, x, y)` targets in -1..1 (a fraction of
/// the eye's travel), a few times a second at most, more at pauses.
pub fn gaze(seed: u64, dur: f64, phones: &[Timed]) -> Vec<(f64, f64, f64)> {
    let mut r = Rng::new(seed ^ 0x51ed_270b);
    let mut out = vec![(0.0, 0.0, 0.0)];
    let mut t = 0.5 + r.next();
    while t < dur {
        let in_pause = phones.iter().any(|p| p.ph == Ph::Sil && p.start <= t && t < p.end);
        let amp = if in_pause { 0.28 } else { 0.14 };
        // Mostly near the middle, now and then glancing off.
        let (x, y) = ((r.next() * 2.0 - 1.0) * amp, (r.next() * 2.0 - 1.0) * amp * 0.6);
        out.push((t, x, y));
        t += 0.45 + 1.4 * r.next();
    }
    out
}
