// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Coarticulation: phonemes in time to smooth mouth channels. Every
//! sound pulls each channel toward its own shape with a strength that is
//! flat through the sound and falls away either side; the channel is the
//! strength-weighted blend of every sound near it. So neighbours blend,
//! the mouth anticipates what is coming, lip closures hold, and sounds
//! the lips do not care about (T, D, N, K, G) take their neighbours' shape.

use crate::extras;
use crate::phoneme::{Ph, Timed};
use crate::viseme::{CHANNELS, Channel, VISEMES, Viseme, lip_targets, of_phoneme, tongue_target};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Emotion {
    Neutral,
    Happy,
    Sad,
    Angry,
    Surprised,
    Scared,
}

impl Emotion {
    pub fn named(n: &str) -> Option<Emotion> {
        Some(match n.to_lowercase().trim() {
            "neutral" | "calm" | "" => Emotion::Neutral,
            "happy" | "joy" | "glad" | "excited" => Emotion::Happy,
            "sad" | "sorrow" | "gloomy" => Emotion::Sad,
            "angry" | "mad" | "cross" => Emotion::Angry,
            "surprised" | "surprise" | "shocked" => Emotion::Surprised,
            "scared" | "afraid" | "fear" => Emotion::Scared,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Style {
    /// Scales jaw and lip movement (1 natural; 0.5 mumbling; 1.4 theatrical).
    pub intensity: f32,
    pub emotion: Emotion,
    /// How far the mouth leads the sound, seconds (0.04 to 0.08).
    pub lead: f64,
    /// Shifts all the mouth later (+) or earlier (-), seconds.
    pub offset: f64,
    /// The frame rate lip closures are guaranteed in.
    pub fps: f64,
    /// Fewest frames a M/B/P closure stays shut.
    pub min_closure_frames: f64,
    /// Blinks, brows and eye movement as well as the mouth.
    pub extras: bool,
    pub seed: u64,
}

impl Default for Style {
    fn default() -> Style {
        Style { intensity: 1.0, emotion: Emotion::Neutral, lead: 0.06, offset: 0.0, fps: 30.0, min_closure_frames: 2.0, extras: true, seed: 1 }
    }
}

/// Smooth channels sampled 100 times a second.
#[derive(Clone, Debug)]
pub struct Curves {
    pub data: Vec<Vec<f32>>,
    /// Weight of each viseme (sums to 1), for rigs that have one shape per viseme.
    pub weights: Vec<Vec<f32>>,
    pub len: usize,
}

pub const RATE: f64 = 100.0;

impl Curves {
    pub fn channel(&self, c: Channel) -> &[f32] {
        &self.data[c.index()]
    }

    pub fn weight(&self, v: Viseme) -> &[f32] {
        &self.weights[v.index()]
    }

    pub fn at(&self, c: Channel, t: f64) -> f32 {
        sample(self.channel(c), t)
    }

    pub fn duration(&self) -> f64 {
        self.len as f64 / RATE
    }

    /// How open the mouth looks: the jaw, less what the lips close.
    pub fn open(&self, t: f64) -> f32 {
        self.at(Channel::Jaw, t) * (1.0 - self.at(Channel::Close, t))
    }
}

pub fn sample(v: &[f32], t: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let x = (t * RATE).clamp(0.0, (v.len() - 1) as f64);
    let i = x.floor() as usize;
    let k = (x - i as f64) as f32;
    let b = v[(i + 1).min(v.len() - 1)];
    v[i] + (b - v[i]) * k
}

struct Sub {
    v: Viseme,
    ph: Ph,
    s: f64,
    e: f64,
    /// Where the strength is full: `core.0..core.1`.
    core: (f64, f64),
    /// How fast the strength falls before and after the core.
    att: f64,
    rel: f64,
    lip: [(f32, f32); 6],
    tongue: ([f32; 4], f32),
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn dom(s: &Sub, t: f64) -> f32 {
    let d = if t < s.core.0 { (s.core.0 - t) / s.att } else if t > s.core.1 { (t - s.core.1) / s.rel } else { 0.0 };
    (-2.0 * d * d).exp() as f32
}

/// Expands phonemes into the pieces the mouth is made of (diphthongs are
/// two), with each one's amplitude from its length, stress and loudness.
fn pieces(phones: &[Timed], st: &Style, loud: &[f32]) -> Vec<Sub> {
    let hold = st.min_closure_frames / st.fps.max(1.0);
    let level = |t: f64| loud.get((t.max(0.0) * 100.0) as usize).copied().unwrap_or(1.0);
    let (jaw_m, lip_m, teeth_m) = match st.emotion {
        Emotion::Sad => (0.85, 0.9, 0.8),
        Emotion::Happy => (1.05, 1.1, 1.2),
        Emotion::Angry => (1.0, 0.9, 1.4),
        Emotion::Surprised => (1.15, 1.0, 1.0),
        Emotion::Scared => (1.0, 1.1, 1.1),
        Emotion::Neutral => (1.0, 1.0, 1.0),
    };
    let mut out = Vec::new();
    for p in phones {
        let (v1, v2) = of_phoneme(p.ph);
        let split: Vec<(Viseme, f64, f64)> = match v2 {
            Some(b) => {
                let m = p.start + 0.55 * p.dur();
                vec![(v1, p.start, m), (b, m, p.end)]
            }
            None => vec![(v1, p.start, p.end)],
        };
        for (v, s, e) in split {
            let d = (e - s).max(0.005);
            // Fast speech does not reach the full shape.
            let quick = ((d / 0.07) as f32).clamp(0.45, 1.0);
            let stress = if p.ph.is_vowel() { if p.stress == 1 { 1.12 } else if p.stress == 0 { 0.92 } else { 1.0 } } else { 1.0 };
            let loudk = if p.ph.is_vowel() { 0.65 + 0.45 * level((s + e) / 2.0).clamp(0.0, 1.0) } else { 1.0 };
            let mut lip = lip_targets(v);
            let amp = quick * stress * loudk * st.intensity;
            if v != Viseme::MBP && v != Viseme::Rest {
                lip[0].0 *= amp * jaw_m;
                lip[2].0 *= amp * lip_m;
                lip[3].0 *= amp * lip_m;
                lip[5].0 *= amp.min(1.2) * teeth_m;
            }
            let (mut s2, mut e2) = (s - st.lead + st.offset, e - st.lead + st.offset);
            let mut core = (s2 + 0.2 * d, e2 - 0.2 * d);
            let (mut att, mut rel) = (0.04 + 0.5 * d, 0.05 + 0.6 * d);
            if v == Viseme::MBP {
                // Held shut for at least `hold`, and pressed sharply.
                e2 = e2.max(s2 + hold);
                core = (s2, e2);
                att = 0.012;
                rel = 0.02;
            } else if v == Viseme::Rest {
                core = (s2, e2);
            }
            if s2 > e2 {
                std::mem::swap(&mut s2, &mut e2);
            }
            let (tg, tw) = if v2.is_some() && v != v1 { ([0.0, 0.3, 0.0, 0.0], 0.3) } else { tongue_target(p.ph) };
            out.push(Sub { v, ph: p.ph, s: s2, e: e2, core, att, rel, lip, tongue: (tg, tw) });
        }
    }
    out
}

/// Builds the smooth channels for `phones` over `total` seconds. `loud`
/// is the voice's loudness (100 frames a second, 0..1), or empty.
pub fn curves(phones: &[Timed], total: f64, st: &Style, loud: &[f32]) -> Curves {
    let n = ((total + 0.5) * RATE) as usize + 1;
    let subs = pieces(phones, st, loud);
    let mut data = vec![vec![0.0f32; n]; CHANNELS.len()];
    let mut weights = vec![vec![0.0f32; n]; VISEMES.len()];
    // Sounds that can reach a moment: within a second.
    let mut lo = 0usize;
    for i in 0..n {
        let t = i as f64 / RATE;
        while lo < subs.len() && subs[lo].e < t - 0.8 {
            lo += 1;
        }
        let mut num = [0.0f32; 6];
        let mut den = [0.0f32; 6];
        let mut tnum = [0.0f32; 4];
        let mut tden = 0.0f32;
        let mut vw = [0.0f32; 18];
        let mut vtot = 0.0f32;
        for s in &subs[lo..] {
            if s.s > t + 0.8 {
                break;
            }
            let d = dom(s, t);
            if d < 1e-3 {
                continue;
            }
            let mut mean = 0.0;
            for c in 0..6 {
                let w = d * s.lip[c].1;
                num[c] += w * s.lip[c].0;
                den[c] += w;
                mean += s.lip[c].1;
            }
            let wt = d * s.tongue.1;
            for c in 0..4 {
                tnum[c] += wt * s.tongue.0[c];
            }
            tden += wt;
            let vwt = d * mean / 6.0;
            vw[s.v.index()] += vwt;
            vtot += vwt;
            let _ = s.ph;
        }
        for c in 0..6 {
            let mut v = if den[c] > 1e-6 { num[c] / den[c] } else { 0.0 };
            // Closures and lip rolls snap shut rather than half-close.
            if c == 1 || c == 4 {
                v = smoothstep(0.12, 0.88, v);
            }
            data[c][i] = v;
        }
        for c in 0..4 {
            data[6 + c][i] = if tden > 1e-6 { tnum[c] / tden } else { 0.0 };
        }
        if vtot > 1e-6 {
            for (k, w) in vw.iter().enumerate() {
                weights[k][i] = w / vtot;
            }
        } else {
            weights[Viseme::Rest.index()][i] = 1.0;
        }
    }
    // Emotion: a standing expression under the speech.
    let (smile, frown, brow_up, brow_down, jaw_base) = match st.emotion {
        Emotion::Happy => (0.35, 0.0, 0.1, 0.0, 0.0),
        Emotion::Sad => (0.0, 0.4, 0.2, 0.0, 0.0),
        Emotion::Angry => (0.0, 0.1, 0.0, 0.55, 0.0),
        Emotion::Surprised => (0.0, 0.0, 0.6, 0.0, 0.12),
        Emotion::Scared => (0.0, 0.1, 0.45, 0.0, 0.08),
        Emotion::Neutral => (0.0, 0.0, 0.0, 0.0, 0.0),
    };
    for i in 0..n {
        data[Channel::Smile.index()][i] = smile;
        data[Channel::Frown.index()][i] = frown;
        data[Channel::BrowUp.index()][i] = brow_up;
        data[Channel::BrowDown.index()][i] = brow_down;
        let j = &mut data[Channel::Jaw.index()][i];
        *j = (*j + jaw_base * (1.0 - *j)).min(1.0);
    }
    if st.extras {
        let blink = &mut data[Channel::Blink.index()];
        for b in extras::blinks(st.seed, total, 4.0, phones) {
            for k in (b * RATE) as usize..((b + extras::BLINK) * RATE) as usize + 1 {
                if k < n {
                    blink[k] = blink[k].max(extras::lid(k as f64 / RATE - b) as f32);
                }
            }
        }
        for (t, a) in extras::brows(phones, loud) {
            let c = &mut data[Channel::BrowUp.index()];
            for k in (t * RATE).max(0.0) as usize..((t + 0.7) * RATE) as usize {
                if k < n {
                    let u = k as f64 / RATE - t;
                    let env = if u < 0.12 { u / 0.12 } else { (1.0 - (u - 0.12) / 0.58).max(0.0) };
                    c[k] = c[k].max(brow_up).max((a * env) as f32);
                }
            }
        }
        let g = extras::gaze(st.seed, total, phones);
        for w in g.windows(2) {
            let (a, b) = (w[0], w[1]);
            for k in (a.0 * RATE) as usize..((b.0 * RATE) as usize).min(n) {
                // A quick glance (60 ms), then hold.
                let u = ((k as f64 / RATE - a.0) / 0.06).clamp(0.0, 1.0);
                data[Channel::EyeX.index()][k] = (a.1 + (b.1 - a.1) * u) as f32;
                data[Channel::EyeY.index()][k] = (a.2 + (b.2 - a.2) * u) as f32;
            }
        }
    }
    Curves { data, weights, len: n }
}

/// Sparse keys `(time, value)` that follow a channel within `tol`
/// (Ramer-Douglas-Peucker); always the first and last sample.
pub fn keys(v: &[f32], tol: f32) -> Vec<(f64, f32)> {
    if v.is_empty() {
        return Vec::new();
    }
    let mut keep = vec![false; v.len()];
    keep[0] = true;
    keep[v.len() - 1] = true;
    let mut stack = vec![(0usize, v.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let k = (i - a) as f32 / (b - a) as f32;
            let line = v[a] + (v[b] - v[a]) * k;
            let e = (v[i] - line).abs();
            if e > worst.0 {
                worst = (e, i);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| (i as f64 / RATE, v[i])).collect()
}
