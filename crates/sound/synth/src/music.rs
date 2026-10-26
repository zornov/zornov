// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The composition engine: original music in a genre, made from nothing but
//! a description, a length and cue points.
//!
//! The pipeline: read the description (genre, mood, instruments) and fix a
//! key and tempo; lay a bar grid over the length (tempo bends a little so
//! cues land on beats and the last chord falls at `length - tail`); choose a
//! chord progression and a shape for the energy; write each part of the band
//! bar by bar (melody with phrases and answering motifs, bass lines, comping
//! patterns, drums); render every note with the instrument models, mix with
//! a reverb bus and bring the result to a target loudness.

use crate::dsp::*;
use crate::instrument::*;
use crate::level;
use crate::style::*;
use crate::theory::*;
use std::collections::HashMap;
use std::f32::consts::PI;

/// What a cue asks of the music.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueKind {
    /// An accent: cymbal, low boom and a chord stab on the cue.
    Hit,
    /// The harmony arrives home on the cue (the bar before leans on the dominant).
    Resolve,
    /// A synth riser climbing to the cue, with a hit on it.
    Riser,
    /// Everything stops for the beat before the cue; then a hit.
    Drop,
    /// A string swell building to the cue and cutting there.
    Swell,
}

impl CueKind {
    /// Reads "hit", "stinger", "resolve", "riser", "drop", "swell"…
    pub fn parse(s: &str) -> Option<CueKind> {
        Some(match s.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "hit" | "accent" | "stinger" | "stab" | "impact" | "sting" | "boom" => CueKind::Hit,
            "resolve" | "resolution" | "release" | "land" | "cadence" | "home" => CueKind::Resolve,
            "riser" | "rise" | "build" | "buildup" | "build_up" => CueKind::Riser,
            "drop" | "stop" | "break" | "silence" | "pause" => CueKind::Drop,
            "swell" | "crescendo" => CueKind::Swell,
            _ => return None,
        })
    }
    /// The name.
    pub fn name(self) -> &'static str {
        match self {
            CueKind::Hit => "hit",
            CueKind::Resolve => "resolve",
            CueKind::Riser => "riser",
            CueKind::Drop => "drop",
            CueKind::Swell => "swell",
        }
    }
}

/// A moment in the picture the music must land on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    /// Seconds from the start of the music.
    pub at: f32,
    /// What happens.
    pub kind: CueKind,
}

/// What to compose.
#[derive(Clone, Debug)]
pub struct MusicSpec {
    /// Free words: genre, mood, instruments ("noir jazz with sax and double bass").
    pub description: String,
    /// Exact length of the result in seconds.
    pub seconds: f32,
    /// Tempo; `None` lets the genre and mood choose, and the grid bend to fit.
    pub bpm: Option<f32>,
    /// Key, such as "D minor" or "F#"; `None` chooses one.
    pub key: Option<String>,
    /// Seconds of ring-out after the last chord; `None` chooses.
    pub tail: Option<f32>,
    /// Moments to land on.
    pub cues: Vec<Cue>,
    /// Instruments asked for by name, in addition to those in the description.
    pub instruments: Vec<String>,
    /// Same seed, same piece.
    pub seed: u64,
    /// Loudness to deliver in LUFS.
    pub lufs: f32,
}

impl MusicSpec {
    /// A spec for `description` lasting `seconds`, seeded from the words.
    pub fn new(description: &str, seconds: f32) -> Self {
        let seed = description.bytes().fold(0x9E37_79B9u64, |h, b| h.wrapping_mul(0x100_0000_01b3).wrapping_add(b as u64));
        Self { description: description.to_owned(), seconds, bpm: None, key: None, tail: None, cues: Vec::new(), instruments: Vec::new(), seed, lufs: -14.0 }
    }
}

/// Where a cue landed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    /// Asked for, seconds.
    pub at: f32,
    /// What it is.
    pub kind: CueKind,
    /// True when the cue falls exactly on a beat of the grid.
    pub on_beat: bool,
    /// True when it falls on the first beat of a bar.
    pub on_bar: bool,
}

/// What was written: the facts behind the audio.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Genre used.
    pub genre: &'static str,
    /// Key used.
    pub key: String,
    /// Tempo asked for or chosen (beats a minute).
    pub bpm: f32,
    /// The slowest and fastest tempo of the grid after bending to fit.
    pub bpm_range: (f32, f32),
    /// Beats in a bar.
    pub meter: u32,
    /// Who plays.
    pub parts: Vec<String>,
    /// Time of every beat in seconds, from the start up to the last chord.
    pub beats: Vec<f32>,
    /// Time of each bar's first beat.
    pub bars: Vec<f32>,
    /// Where each cue landed.
    pub landings: Vec<Landing>,
    /// Seconds at which the last chord starts.
    pub final_chord: f32,
    /// Seconds of ring-out after the last chord.
    pub tail: f32,
    /// Length in seconds.
    pub seconds: f32,
    /// Loudness reached (LUFS).
    pub lufs: f32,
    /// Sample peak reached (0..1).
    pub peak: f32,
    /// Notes written.
    pub notes: usize,
}

impl Plan {
    /// A line for the tool reply.
    pub fn describe(&self) -> String {
        let bend = if (self.bpm_range.1 - self.bpm_range.0) > 0.6 { format!(", grid bent {:.0}-{:.0} to land the cues", self.bpm_range.0, self.bpm_range.1) } else { String::new() };
        let cues: Vec<String> = self.landings.iter().map(|l| format!("{} at {:.2}s{}", l.kind.name(), l.at, if l.on_bar { " (on a downbeat)" } else if l.on_beat { " (on a beat)" } else { " (between beats: no tempo fit it)" })).collect();
        format!(
            "{} in {}, {:.0} bpm{}, {}/4, {}; last chord at {:.2}s then a {:.1}s tail; {:.1} LUFS, peak {:.2}{}",
            self.genre,
            self.key,
            self.bpm,
            bend,
            self.meter,
            self.parts.join(", "),
            self.final_chord,
            self.tail,
            self.lufs,
            self.peak,
            if cues.is_empty() { String::new() } else { format!("; {}", cues.join(", ")) }
        )
    }
}

/// The audio and what is in it.
#[derive(Clone, Debug)]
pub struct Piece {
    /// Stereo audio, exactly `seconds` long.
    pub audio: Stereo,
    /// What was written.
    pub plan: Plan,
}

// ----- the bar grid -----------------------------------------------------------

struct TimeMap {
    anchors: Vec<(f64, f64)>,
    tail_bpm: f64,
}

impl TimeMap {
    fn time_of(&self, beat: f64) -> f64 {
        let a = &self.anchors;
        let mut i = 0;
        while i + 1 < a.len() && a[i + 1].0 <= beat {
            i += 1;
        }
        if i + 1 < a.len() {
            let (b0, t0) = a[i];
            let (b1, t1) = a[i + 1];
            t0 + (beat - b0) * (t1 - t0) / (b1 - b0)
        } else {
            a[i].1 + (beat - a[i].0) * 60.0 / self.tail_bpm
        }
    }
    fn bpm_range(&self, until: f64) -> (f32, f32) {
        let mut lo = f64::MAX;
        let mut hi = 0.0f64;
        for w in self.anchors.windows(2) {
            if w[0].0 >= until {
                break;
            }
            let b = (w[1].0 - w[0].0) / (w[1].1 - w[0].1) * 60.0;
            lo = lo.min(b);
            hi = hi.max(b);
        }
        if hi == 0.0 {
            return (self.tail_bpm as f32, self.tail_bpm as f32);
        }
        (lo as f32, hi as f32)
    }
}

/// Chooses the beat a time should fall on: the nearest of floor, ceil and
/// the nearest bar line after `from`, scored by how little the tempo bends.
fn snap_beat(from: (f64, f64), at: f64, bpm: f64, meter: f64, flex: f64) -> Option<f64> {
    let ideal = from.0 + (at - from.1) * bpm / 60.0;
    let bar = (ideal / meter).round() * meter;
    let mut best: Option<(f64, f64)> = None;
    for c in [ideal.floor(), ideal.ceil(), bar] {
        if c < from.0 + 1.0 - 1e-9 || at <= from.1 {
            continue;
        }
        let seg = (c - from.0) / (at - from.1) * 60.0;
        let dev = (seg / bpm).ln().abs();
        let score = dev - if (c / meter).fract().abs() < 1e-9 { 0.03 } else { 0.0 };
        if best.is_none_or(|(_, s)| score < s) {
            best = Some((c, score));
        }
        let _ = dev;
    }
    let (c, _) = best?;
    let seg = (c - from.0) / (at - from.1) * 60.0;
    ((seg / bpm).ln().abs() <= (1.0 + flex).ln()).then_some(c)
}

// ----- notes ------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Ev {
    beat: f64,
    dur: f64,
    midi: f32,
    vel: f32,
    drum: Option<Drum>,
    nudge: f32,
}

fn note(beat: f64, dur: f64, midi: i32, vel: f32) -> Ev {
    Ev { beat, dur, midi: midi as f32, vel: vel.clamp(0.05, 1.0), drum: None, nudge: 0.0 }
}

fn hit(beat: f64, d: Drum, vel: f32) -> Ev {
    Ev { beat, dur: 0.25, midi: 0.0, vel: vel.clamp(0.03, 1.0), drum: Some(d), nudge: 0.0 }
}

struct Ctx<'a> {
    style: &'a Style,
    key: Key,
    minor: bool,
    meter: f64,
    bars: usize,
    chords: Vec<Chord>,
    inten: Vec<f32>,
    final_beat: f64,
    sparse: bool,
    fills: Vec<bool>,
    cue_beats: Vec<(f64, CueKind)>,
}

impl Ctx<'_> {
    fn chord_at(&self, beat: f64) -> Chord {
        self.chords[((beat / self.meter).max(0.0) as usize).min(self.chords.len() - 1)]
    }
    fn inten_at(&self, beat: f64) -> f32 {
        self.inten[((beat / self.meter).max(0.0) as usize).min(self.inten.len() - 1)]
    }
    fn pool(&self) -> Vec<i32> {
        self.key.scale()
    }
}

// ----- melody -----------------------------------------------------------------

/// A rhythm for `bars` bars: `(offset, length)` in beats.
fn rhythm(rng: &mut Rng, bars: usize, meter: f64, sub: f64, rest_p: f32, long_p: f32) -> Vec<(f64, f64)> {
    let total = (bars as f64 * meter / sub).round() as usize;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < total {
        if pos > 0 && rng.chance(rest_p * 0.6) {
            pos += 1 + rng.below(2);
            continue;
        }
        let weights: [(usize, f32); 6] = [(1, 3.0), (2, 3.5), (3, 1.5), (4, 2.0 + long_p * 2.0), (6, long_p * 3.0), (8, long_p * 2.5)];
        let sum: f32 = weights.iter().map(|w| w.1).sum();
        let mut pick = rng.unit() * sum;
        let mut units = 2;
        for (u, w) in weights {
            if pick < w {
                units = u;
                break;
            }
            pick -= w;
        }
        // Notes shorter than a beat should start on the grid's strong side.
        if pos == 0 {
            units = units.max(2);
        }
        units = units.min(total - pos);
        let dur = units as f64 * sub * if units > 1 && rng.chance(0.5) { 0.92 } else { 1.0 };
        out.push((pos as f64 * sub, dur));
        pos += units;
        if rng.chance(rest_p * 0.5) {
            pos += 1;
        }
    }
    out
}

#[derive(Clone)]
struct Motif {
    rhy: Vec<(f64, f64)>,
    pitch: Vec<i32>,
    strong: Vec<bool>,
}

fn ladder(cx: &Ctx, lo: i32, hi: i32, t0: f64, len: f64) -> Vec<i32> {
    let mut pool = cx.pool();
    let mut b = t0;
    while b < t0 + len {
        pool.extend(cx.chord_at(b).pcs());
        b += cx.meter;
    }
    (lo..=hi).filter(|p| pool.contains(&p.rem_euclid(12))).collect()
}

fn nearest_idx(l: &[i32], p: i32) -> usize {
    l.iter().enumerate().min_by_key(|(_, v)| (**v - p).abs()).map_or(0, |(i, _)| i)
}

fn is_strong(cx: &Ctx, beat: f64, dur: f64, last: bool) -> bool {
    let bib = beat % cx.meter;
    bib < 0.01 || (cx.meter as i32 == 4 && (bib - 2.0).abs() < 0.01) || dur >= 1.5 || last
}

fn fresh_motif(cx: &Ctx, rng: &mut Rng, rhy: Vec<(f64, f64)>, t0: f64, len: f64, variant: usize, prev: &mut i32) -> Motif {
    let (lo, hi) = (60, 79);
    let lad = ladder(cx, lo, hi, t0, len);
    let n = rhy.len();
    let center = (lo + hi) as f32 / 2.0;
    let amp = (hi - lo) as f32 * 0.28 + if variant == 2 { 3.0 } else { 0.0 };
    let shift = if variant == 2 { 2.5 } else { 0.0 };
    let strong: Vec<bool> = rhy.iter().enumerate().map(|(i, (o, d))| is_strong(cx, t0 + o, *d, i + 1 == n)).collect();
    let mut idx = vec![0usize; n];
    let mut last_strong = *prev;
    for i in 0..n {
        if !strong[i] {
            continue;
        }
        let b = t0 + rhy[i].0;
        let frac = (rhy[i].0 / len) as f32;
        let target = center + shift + amp * (PI * frac).sin();
        let tones = cx.chord_at(b).pcs();
        let mut best = (f32::MAX, 0usize);
        for (k, p) in lad.iter().enumerate() {
            if !tones.contains(&p.rem_euclid(12)) {
                continue;
            }
            let cost = (*p as f32 - target).abs() + 0.45 * (*p - last_strong).abs() as f32 + rng.range(0.0, 2.5);
            if cost < best.0 {
                best = (cost, k);
            }
        }
        idx[i] = best.1;
        last_strong = lad[best.1];
    }
    // Weak notes walk between the strong ones.
    let mut i = 0;
    let mut anchor = nearest_idx(&lad, *prev);
    while i < n {
        if strong[i] {
            anchor = idx[i];
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && !strong[j] {
            j += 1;
        }
        let target = if j < n { idx[j] } else { (anchor as i32 + rng.below(5) as i32 - 2).clamp(0, lad.len() as i32 - 1) as usize };
        let m = j - i;
        for k in 0..m {
            let f = (k + 1) as f32 / (m + 1) as f32;
            let mut v = (anchor as f32 + (target as f32 - anchor as f32) * f).round() as i32;
            v += [-1, 0, 0, 1][rng.below(4)];
            idx[i + k] = v.clamp(0, lad.len() as i32 - 1) as usize;
        }
        i = j;
    }
    let mut pitch: Vec<i32> = idx.iter().map(|k| lad[*k]).collect();
    // Bebop approach: a weak note a semitone under or over the next strong one.
    if cx.style.name == "jazz" {
        for i in 0..n.saturating_sub(1) {
            if !strong[i] && strong[i + 1] && rng.chance(0.4) {
                pitch[i] = pitch[i + 1] + if rng.chance(0.5) { 1 } else { -1 };
            }
        }
    }
    // Never three of the same note running.
    for i in 2..n {
        if pitch[i] == pitch[i - 1] && pitch[i] == pitch[i - 2] {
            let k = nearest_idx(&lad, pitch[i]);
            pitch[i] = lad[(k + 1).min(lad.len() - 1)];
        }
    }
    if let Some(l) = pitch.last() {
        *prev = *l;
    }
    Motif { rhy, pitch, strong }
}

/// The motif again over new harmony: strong notes move to the nearest tone
/// of the new chord, weak notes keep their step from the strong note before.
fn answer(cx: &Ctx, m: &Motif, t0: f64, len: f64, end_home: bool) -> Motif {
    let lad = ladder(cx, 58, 82, t0, len);
    let n = m.rhy.len();
    let mut pitch = m.pitch.clone();
    let mut old_anchor = m.pitch[0];
    let mut new_anchor = m.pitch[0];
    for i in 0..n {
        if m.strong[i] {
            let tones = cx.chord_at(t0 + m.rhy[i].0).pcs();
            let p = lad.iter().copied().filter(|p| tones.contains(&p.rem_euclid(12))).min_by_key(|p| (*p - m.pitch[i]).abs()).unwrap_or(m.pitch[i]);
            old_anchor = m.pitch[i];
            new_anchor = p;
            pitch[i] = p;
        } else {
            let steps = nearest_idx(&lad, m.pitch[i]) as i32 - nearest_idx(&lad, old_anchor) as i32;
            let k = (nearest_idx(&lad, new_anchor) as i32 + steps).clamp(0, lad.len() as i32 - 1);
            pitch[i] = lad[k as usize];
        }
    }
    if end_home {
        if let Some(last) = pitch.last_mut() {
            let root = cx.key.root;
            *last = lad.iter().copied().filter(|p| p.rem_euclid(12) == root).min_by_key(|p| (*p - *last).abs()).unwrap_or(*last);
        }
    }
    Motif { rhy: m.rhy.clone(), pitch, strong: m.strong.clone() }
}

fn melody(cx: &Ctx, rng: &mut Rng) -> Vec<Ev> {
    let m = cx.meter;
    let (sub, rest_p, long_p) = cx.style.lead;
    let rest_p = rest_p + if cx.sparse { 0.2 } else { 0.0 };
    let pb = if sub >= 1.0 { 4 } else { 2 };
    let len = pb as f64 * m;
    let mut out = Vec::new();
    let mut prev = 70;
    let mut theme: Option<Motif> = None;
    let mut phrase = 0;
    let mut t0 = 0.0;
    while t0 < cx.final_beat - 0.5 {
        let variant = phrase % 4;
        let last_phrase = t0 + len >= cx.final_beat - 0.01;
        let motif = match (variant, &theme) {
            (0, _) | (_, None) => {
                let rhy = rhythm(rng, pb, m, sub as f64, rest_p, long_p);
                let mo = fresh_motif(cx, rng, rhy, t0, len, 0, &mut prev);
                theme = Some(mo.clone());
                mo
            }
            (1, Some(th)) => answer(cx, th, t0, len, false),
            (2, _) => {
                let rhy = rhythm(rng, pb, m, sub as f64, rest_p, long_p);
                fresh_motif(cx, rng, rhy, t0, len, 2, &mut prev)
            }
            (_, Some(th)) => answer(cx, th, t0, len, true),
        };
        let n = motif.rhy.len();
        for i in 0..n {
            let b = t0 + motif.rhy[i].0;
            if b >= cx.final_beat - 0.05 {
                break;
            }
            let mut dur = motif.rhy[i].1;
            if i + 1 == n {
                dur = dur.max(1.5).min(len - motif.rhy[i].0);
            }
            let e = cx.inten_at(b);
            let vel = 0.5 + 0.35 * e + if motif.strong[i] { 0.08 } else { -0.04 } + rng.range(-0.04, 0.04);
            out.push(note(b, dur.min(cx.final_beat - b), motif.pitch[i], vel));
        }
        let _ = last_phrase;
        t0 += len;
        phrase += 1;
        if variant == 3 {
            theme = None;
        }
    }
    out
}

// ----- parts ------------------------------------------------------------------

fn active_bars(cx: &Ctx, enter: f32) -> Vec<bool> {
    let mut on = false;
    cx.inten
        .iter()
        .map(|e| {
            if !on && *e >= enter {
                on = true;
            } else if on && *e < enter - 0.08 {
                on = false;
            }
            on
        })
        .collect()
}

fn rootless(c: Chord, low: i32) -> Vec<i32> {
    let iv: &[i32] = match c.q {
        Q::Maj7 | Q::Dom7 => &[4, if c.q == Q::Maj7 { 11 } else { 10 }, 14, 7],
        Q::Min7 | Q::Min6 => &[3, if c.q == Q::Min6 { 9 } else { 10 }, 14, 7],
        Q::Hdim7 => &[3, 6, 10, 13],
        Q::Maj => &[4, 7, 11, 14],
        Q::Min => &[3, 7, 10, 14],
        _ => &[5, 7, 10, 14],
    };
    let mut v: Vec<i32> = iv.iter().map(|i| { let pc = (c.root + i).rem_euclid(12); low + (pc - low).rem_euclid(12) }).collect();
    v.sort();
    // Keep the voicing compact and near the middle.
    let avg = v.iter().sum::<i32>() / v.len() as i32;
    if avg > 68 {
        v.iter_mut().for_each(|p| *p -= 12);
    } else if avg < 56 {
        v.iter_mut().for_each(|p| *p += 12);
    }
    v
}

fn third_of(q: Q) -> i32 {
    match q {
        Q::Min | Q::Min7 | Q::Min6 | Q::Dim | Q::Hdim7 => 3,
        Q::Sus4 => 5,
        Q::Sus2 => 2,
        Q::Power => 7,
        _ => 4,
    }
}

fn gen_comp(cx: &Ctx, kind: Comp, enter: f32, rng: &mut Rng) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let mut out = Vec::new();
    let m = cx.meter;
    for bar in 0..cx.bars {
        if !on[bar] {
            continue;
        }
        let b0 = bar as f64 * m;
        let c = cx.chords[bar];
        let e = cx.inten[bar];
        let vb = 0.45 + 0.35 * e;
        match kind {
            Comp::Jazz => {
                let pats: &[&[(f64, f64)]] = if e < 0.4 { &[&[(0.0, 3.6)], &[(1.5, 0.6), (3.5, 0.4)], &[(0.0, 1.8), (2.5, 1.2)]] } else { &[&[(0.0, 0.9), (1.5, 0.45)], &[(1.5, 0.45), (3.5, 0.4)], &[(0.0, 1.4), (2.5, 1.0)], &[(0.5, 0.4), (2.0, 0.4), (3.5, 0.4)]] };
                let p = pats[rng.below(pats.len())];
                let v = rootless(c, 52);
                for (o, d) in p {
                    for (k, n) in v.iter().enumerate() {
                        let mut ev = note(b0 + o, *d, *n, vb + rng.range(-0.05, 0.05));
                        ev.nudge = k as f32 * 0.006;
                        out.push(ev);
                    }
                }
            }
            Comp::Strum => {
                let r = c.root_at(40);
                let t = third_of(c.q);
                let five = if c.q == Q::Dim || c.q == Q::Hdim7 { 6 } else { 7 };
                let shape = [r, r + five, r + 12, r + 12 + t, r + 12 + five, r + 24];
                if e < 0.4 {
                    // Fingerpicked until the song fills out.
                    for (k, s) in [0usize, 4, 3, 4, 1, 4, 3, 4].iter().enumerate() {
                        out.push(note(b0 + k as f64 * 0.5, 1.2, shape[*s], vb - 0.08 + if k % 4 == 0 { 0.1 } else { 0.0 }));
                    }
                } else {
                    for (o, down) in [(0.0, true), (1.0, true), (1.5, false), (2.5, false), (3.0, true), (3.5, false)] {
                        let idx: Vec<usize> = if down { vec![0, 1, 2, 3, 4, 5] } else { vec![5, 4, 3, 2] };
                        for (j, k) in idx.iter().enumerate() {
                            let mut ev = note(b0 + o, if down { 1.0 } else { 0.5 }, shape[*k], vb * if down { 0.95 } else { 0.7 } + if o == 0.0 { 0.08 } else { 0.0 });
                            ev.nudge = j as f32 * if down { 0.011 } else { 0.008 };
                            out.push(ev);
                        }
                    }
                }
            }
            Comp::Picking => {
                let v = c.stack(c.root_at(45), 4);
                let sub = if e < 0.5 { 1.0 } else { 0.5 };
                let seq = [0usize, 1, 2, 3, 2, 1];
                let mut o = 0.0;
                let mut k = 0;
                while o < m {
                    out.push(note(b0 + o, sub * 2.5, v[seq[k % seq.len()]] + if seq[k % seq.len()] == 0 { 0 } else { 12 }, vb - 0.1 + if k % 4 == 0 { 0.08 } else { 0.0 }));
                    o += sub;
                    k += 1;
                }
            }
            Comp::Block => {
                let low = c.root_at(36);
                out.push(note(b0, 2.0, low, vb + 0.05));
                out.push(note(b0 + 2.0, 2.0, low + 7, vb - 0.1));
                let v = c.stack(c.root_at(57), 3);
                if e > 0.5 {
                    for (k, i) in [0usize, 1, 2, 1, 0, 1, 2, 1].iter().enumerate() {
                        out.push(note(b0 + k as f64 * 0.5, 1.0, v[*i], vb - 0.1 + if k % 4 == 0 { 0.08 } else { 0.0 }));
                    }
                } else {
                    for n in &v {
                        out.push(note(b0, 2.0, *n, vb - 0.05));
                    }
                    for n in &v {
                        out.push(note(b0 + 2.0, 2.0, *n, vb - 0.15));
                    }
                }
            }
            Comp::Stabs => {
                let v = c.stack(c.root_at(55), 4);
                for o in [0.5, 1.5, 2.5, 3.5] {
                    for n in &v {
                        out.push(note(b0 + o, 0.25, *n, vb - 0.05));
                    }
                }
            }
            Comp::OomPah => {
                let low = c.root_at(38);
                let v = c.stack(c.root_at(55), 3);
                for (o, bass) in [(0.0, low), (1.0, 0), (2.0, low + 7), (3.0, 0)] {
                    if bass > 0 {
                        out.push(note(b0 + o, 0.8, bass, vb + 0.05));
                    } else {
                        for n in &v {
                            out.push(note(b0 + o, 0.55, *n, vb - 0.05));
                        }
                    }
                }
            }
        }
    }
    out
}

fn gen_bass(cx: &Ctx, kind: Bass, inst: Inst, enter: f32, rng: &mut Rng) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let low = inst.range().0 + 3;
    let mut out = Vec::new();
    let m = cx.meter;
    let mut prev = low + 5;
    for bar in 0..cx.bars {
        let b0 = bar as f64 * m;
        let c = cx.chords[bar];
        let next = cx.chords[(bar + 1).min(cx.bars - 1)];
        let e = cx.inten[bar];
        let vb = 0.55 + 0.3 * e;
        let root = c.root_at(low);
        if !on[bar] {
            prev = root;
            continue;
        }
        match kind {
            Bass::Walking => {
                if e < 0.4 {
                    out.push(note(b0, 2.0, root, vb));
                    out.push(note(b0 + 2.0, 2.0, nearest((c.root + 7) % 12, root), vb - 0.1));
                } else {
                    let tones = c.pcs();
                    let third = nearest(tones[1 % tones.len()], root + 4);
                    let fifth = nearest((c.root + 7) % 12, root + 7);
                    let (s1, s2) = if rng.chance(0.5) { (third, fifth) } else { (fifth, third) };
                    let nr = next.root_at(low);
                    let approach = if rng.chance(0.6) { nr - 1 } else { nr + 1 };
                    for (k, p) in [root, s1, s2, approach].iter().enumerate() {
                        out.push(note(b0 + k as f64, 0.95, (*p).clamp(low - 2, low + 22), vb + if k == 0 { 0.08 } else { 0.0 } + rng.range(-0.04, 0.04)));
                    }
                }
            }
            Bass::RootFifth => {
                out.push(note(b0, 1.6, root, vb + 0.05));
                out.push(note(b0 + 2.0, 1.6, nearest((c.root + 7) % 12, root + 5), vb - 0.05));
                if e > 0.6 {
                    out.push(note(b0 + 3.5, 0.4, nearest((c.root + 7) % 12, root + 5), vb - 0.15));
                }
            }
            Bass::Pedal => {
                let same = bar > 0 && cx.chords[bar - 1] == c && bar % 2 == 1;
                if !same {
                    out.push(note(b0, m * if cx.chords.get(bar + 1) == Some(&c) { 2.0 } else { 1.0 } - 0.1, root, vb));
                }
            }
            Bass::Pulse => {
                for k in 0..8 {
                    let p = if k % 4 == 3 && e > 0.5 { root + 12 } else { root };
                    out.push(note(b0 + k as f64 * 0.5, 0.45, p, vb - 0.05 + if k % 2 == 0 { 0.06 } else { 0.0 }));
                }
            }
            Bass::Offbeat => {
                if e < 0.55 {
                    out.push(note(b0, 0.9, root, vb));
                }
                for o in [0.5, 1.5, 2.5, 3.5] {
                    let p = if rng.chance(0.15) { root + 12 } else { root };
                    out.push(note(b0 + o, 0.4, p, vb));
                }
                if bar % 4 == 3 {
                    out.push(note(b0 + 3.25, 0.2, next.root_at(low), vb));
                }
            }
            Bass::Lazy => {
                out.push(note(b0, 1.3, root, vb + 0.05));
                out.push(note(b0 + 1.75, 0.5, nearest((c.root + 7) % 12, root + 5), vb - 0.05));
                out.push(note(b0 + 2.5, 0.9, root, vb));
                if e > 0.5 {
                    out.push(note(b0 + 3.5, 0.4, next.root_at(low), vb - 0.1));
                }
            }
            Bass::Rock => {
                for k in 0..8 {
                    let p = if k == 5 && e > 0.6 { nearest((c.root + 7) % 12, root + 5) } else { root };
                    out.push(note(b0 + k as f64 * 0.5, 0.48, p, vb + if k % 4 == 0 { 0.08 } else { -0.03 }));
                }
            }
        }
        prev = root;
    }
    let _ = prev;
    out
}

fn gen_pad(cx: &Ctx, enter: f32) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let mut out = Vec::new();
    let m = cx.meter;
    let mut bar = 0;
    while bar < cx.bars {
        if !on[bar] {
            bar += 1;
            continue;
        }
        // One held chord across bars that share it.
        let c = cx.chords[bar];
        let mut end = bar + 1;
        while end < cx.bars && cx.chords[end] == c && on[end] && end - bar < 4 {
            end += 1;
        }
        let b0 = bar as f64 * m;
        let len = (end - bar) as f64 * m + 0.5;
        let e = cx.inten[bar];
        let mut v = c.stack(c.root_at(55), 3);
        v.push(c.root_at(36));
        for n in v {
            let dur = len.min((cx.final_beat - b0).max(0.5));
            out.push(note(b0, dur, n, 0.45 + 0.3 * e));
        }
        bar = end;
    }
    out
}

fn gen_drone(cx: &Ctx, enter: f32) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let mut out = Vec::new();
    let m = cx.meter;
    let mut bar = 0;
    while bar < cx.bars {
        if !on[bar] {
            bar += 1;
            continue;
        }
        let span = 4.min(cx.bars - bar);
        let b0 = bar as f64 * m;
        let len = (span as f64 * m + 1.5).min((cx.final_beat - b0).max(1.0));
        let r = cx.key.root;
        out.push(note(b0, len, 24 + (r - 24).rem_euclid(12), 0.55));
        out.push(note(b0, len, 36 + (r + 7 - 36).rem_euclid(12), 0.35));
        bar += span;
    }
    out
}

fn gen_arp(cx: &Ctx, enter: f32) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let mut out = Vec::new();
    let m = cx.meter;
    for bar in 0..cx.bars {
        if !on[bar] {
            continue;
        }
        let c = cx.chords[bar];
        let e = cx.inten[bar];
        let v = c.stack(c.root_at(60), 4);
        let seq: &[usize] = if (bar / 2) % 2 == 0 { &[0, 1, 2, 3, 2, 1, 0, 2] } else { &[0, 2, 1, 3, 2, 3, 1, 2] };
        let sub = if e > 0.8 { 0.25 } else { 0.5 };
        let n = (m / sub) as usize;
        for k in 0..n {
            let p = v[seq[k % seq.len()]];
            let acc = if k % (n / 4).max(1) == 0 { 0.1 } else { 0.0 };
            out.push(note(bar as f64 * m + k as f64 * sub, sub * 0.8, p, 0.4 + 0.25 * e + acc));
        }
    }
    out
}

fn gen_ostinato(cx: &Ctx, enter: f32) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let mut out = Vec::new();
    let m = cx.meter;
    for bar in 0..cx.bars {
        if !on[bar] {
            continue;
        }
        let c = cx.chords[bar];
        let e = cx.inten[bar];
        let r = c.root_at(60);
        let f = nearest((c.root + 7) % 12, r + 5);
        for k in 0..(m * 2.0) as usize {
            let p = if k % 2 == 0 { r } else { f };
            out.push(note(bar as f64 * m + k as f64 * 0.5, 0.45, p, 0.28 + 0.3 * e + if k % 4 == 0 { 0.08 } else { 0.0 }));
        }
    }
    out
}

fn gen_sparse(cx: &Ctx, inst: Inst, enter: f32, rng: &mut Rng) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let (ilo, ihi) = inst.range();
    let lo = (ilo + 12).max(60).min(ihi - 19);
    let mut out = Vec::new();
    let m = cx.meter;
    let mut bar = 0;
    while bar < cx.bars {
        if on[bar] && rng.chance(if cx.sparse { 0.5 } else { 0.7 }) {
            let c = cx.chords[bar];
            let mut pool = c.pcs();
            if rng.chance(0.4) {
                pool.extend(cx.pool());
            }
            let pc = pool[rng.below(pool.len())];
            let p = lo + (pc - lo).rem_euclid(12) + 12 * rng.below(2) as i32;
            let off = (rng.below((m * 2.0) as usize)) as f64 * 0.5;
            let b = bar as f64 * m + off;
            if b < cx.final_beat - 1.0 {
                out.push(note(b, 3.0, p.min(ihi), 0.35 + 0.25 * cx.inten[bar] + rng.range(-0.05, 0.05)));
            }
        }
        bar += 2;
    }
    out
}

fn gen_drums(cx: &Ctx, beat: Beat, enter: f32, rng: &mut Rng) -> Vec<Ev> {
    let on = active_bars(cx, enter);
    let mut out = Vec::new();
    let m = cx.meter;
    for bar in 0..cx.bars {
        if !on[bar] {
            continue;
        }
        let b0 = bar as f64 * m;
        let e = cx.inten[bar];
        let mut jit = rng.fork(bar as u64);
        let mut h = |off: f64, d: Drum, v: f32| out.push(hit(b0 + off, d, v * (0.75 + 0.35 * e) + jit.range(-0.04, 0.04)));
        match beat {
            Beat::None => {}
            Beat::Jazz => {
                for (o, v) in [(0.0, 0.6), (1.0, 0.55), (1.5, 0.42), (2.0, 0.6), (3.0, 0.55), (3.5, 0.42)] {
                    h(o, Drum::Ride, v);
                }
                h(1.0, Drum::HatPedal, 0.5);
                h(3.0, Drum::HatPedal, 0.5);
                if e > 0.45 {
                    for o in [0.0, 1.0, 2.0, 3.0] {
                        h(o, Drum::Kick, 0.2);
                    }
                }
                if rng.chance(0.4 * e) {
                    h([1.5, 2.5, 3.5][rng.below(3)], Drum::Rim, 0.3);
                }
            }
            Beat::Brushes => {
                h(0.0, Drum::Brush, 0.5);
                h(2.0, Drum::Brush, 0.5);
                h(1.0, Drum::Brush, 0.25);
                h(3.0, Drum::Brush, 0.25);
                h(1.0, Drum::HatPedal, 0.4);
                h(3.0, Drum::HatPedal, 0.4);
                if e > 0.5 {
                    h(0.0, Drum::Kick, 0.2);
                }
                if rng.chance(0.3) {
                    h(2.5, Drum::Ride, 0.3);
                }
            }
            Beat::Folk => {
                h(0.0, Drum::Kick, 0.6);
                if e > 0.5 {
                    h(2.0, Drum::Kick, 0.45);
                }
                if e > 0.55 {
                    h(1.0, Drum::Rim, 0.35);
                    h(3.0, Drum::Rim, 0.35);
                }
                if e > 0.3 {
                    for k in 0..8 {
                        h(k as f64 * 0.5, Drum::Shaker, if k % 2 == 0 { 0.3 } else { 0.2 });
                    }
                }
                if e > 0.78 {
                    h(1.0, Drum::Tambourine, 0.4);
                    h(3.0, Drum::Tambourine, 0.4);
                }
            }
            Beat::Cinematic => {
                let pitch = 36.0 + (cx.key.root - 36).rem_euclid(12) as f32;
                if bar % 2 == 0 && e > 0.3 {
                    h(0.0, Drum::Timpani, 0.7);
                    let mut ev = hit(b0 + 0.0, Drum::Timpani, 0.0);
                    ev.vel = 0.03;
                    let _ = ev;
                }
                if e > 0.65 {
                    h(0.0, Drum::Taiko, 0.8);
                    h(2.0, Drum::Taiko, 0.55);
                    h(3.5, Drum::Taiko, 0.4);
                }
                if bar % 4 == 0 && e > 0.7 {
                    h(0.0, Drum::Crash, 0.5);
                }
                if e > 0.88 {
                    for k in 0..8 {
                        h(k as f64 * 0.5, Drum::Snare, 0.25 + k as f32 * 0.02);
                    }
                }
                // Timpani is pitched on the tonic.
                for ev in out.iter_mut().filter(|e| e.drum == Some(Drum::Timpani)) {
                    ev.midi = pitch;
                }
            }
            Beat::Dark => {
                if bar % 2 == 0 {
                    h(0.0, Drum::Taiko, 0.5);
                    h(0.85, Drum::Taiko, 0.32);
                }
                if e > 0.7 {
                    h(2.5, Drum::TomLow, 0.4);
                }
                if bar % 4 == 3 && e > 0.55 {
                    h(2.0, Drum::Crash, 0.25);
                }
            }
            Beat::Minimal => {
                for k in 0..4 {
                    h(k as f64, Drum::Kick808, 0.8);
                }
                if e > 0.15 {
                    for o in [0.5, 1.5, 2.5, 3.5] {
                        h(o, Drum::HatClosed, if o == 1.5 || o == 3.5 { 0.45 } else { 0.32 });
                    }
                }
                if e > 0.4 {
                    h(1.0, Drum::Rim, 0.5);
                    h(3.0, Drum::Rim, 0.5);
                }
                if e > 0.7 {
                    for k in 0..16 {
                        if k % 2 == 1 {
                            h(k as f64 * 0.25, Drum::HatClosed, 0.15);
                        }
                    }
                }
                if bar % 4 == 3 && e > 0.3 {
                    h(3.5, Drum::HatOpen, 0.4);
                }
            }
            Beat::Lofi => {
                h(0.0, Drum::Kick, 0.8);
                h(1.75, Drum::Kick, 0.5);
                h(2.5, Drum::Kick, 0.65);
                h(1.0, Drum::Snare, 0.55);
                h(3.0, Drum::Snare, 0.55);
                for k in 0..8 {
                    h(k as f64 * 0.5, Drum::HatClosed, if k % 2 == 0 { 0.35 } else { 0.22 });
                }
            }
            Beat::Rock => {
                h(0.0, Drum::Kick, 0.9);
                h(2.0, Drum::Kick, 0.8);
                if e > 0.6 {
                    h(2.5, Drum::Kick, 0.7);
                }
                h(1.0, Drum::Snare, 0.85);
                h(3.0, Drum::Snare, 0.85);
                for k in 0..8 {
                    h(k as f64 * 0.5, Drum::HatClosed, if k % 2 == 0 { 0.5 } else { 0.35 });
                }
                if bar % 4 == 0 && e > 0.6 {
                    h(0.0, Drum::Crash, 0.7);
                }
            }
            Beat::Pop => {
                h(0.0, Drum::Kick, 0.85);
                h(2.0, Drum::Kick, 0.75);
                if bar % 2 == 1 {
                    h(1.75, Drum::Kick, 0.5);
                }
                h(1.0, Drum::Snare, 0.7);
                h(1.0, Drum::Clap, 0.45);
                h(3.0, Drum::Snare, 0.7);
                h(3.0, Drum::Clap, 0.45);
                for k in 0..8 {
                    h(k as f64 * 0.5, Drum::HatClosed, if k % 2 == 0 { 0.4 } else { 0.3 });
                }
                if e > 0.7 {
                    h(3.5, Drum::HatOpen, 0.4);
                }
            }
            Beat::Playful => {
                h(0.0, Drum::Kick, 0.7);
                h(2.0, Drum::Kick, 0.6);
                h(1.0, Drum::Brush, 0.5);
                h(3.0, Drum::Brush, 0.5);
                if e > 0.5 {
                    h(3.5, Drum::Rim, 0.3);
                }
            }
        }
        // A fill into what comes next.
        if cx.fills[bar] && beat != Beat::None {
            let fill = if matches!(beat, Beat::Jazz | Beat::Brushes | Beat::Playful) { Drum::Brush } else if matches!(beat, Beat::Minimal) { Drum::HatClosed } else { Drum::Snare };
            let n = if beat == Beat::Minimal { 8 } else { 4 };
            let start = m - n as f64 * 0.25;
            for k in 0..n {
                let v = 0.3 + 0.55 * (k + 1) as f32 / n as f32;
                out.push(hit(b0 + start + k as f64 * 0.25, fill, v));
            }
        }
    }
    out
}

// ----- the whole piece --------------------------------------------------------

fn class_of(inst: Inst) -> u8 {
    match inst {
        Inst::Sax | Inst::Trumpet | Inst::Trombone | Inst::Horn | Inst::Flute | Inst::Clarinet | Inst::Violin | Inst::SynthLead => 0,
        Inst::UprightBass | Inst::ElectricBass | Inst::SynthBass | Inst::Cello => 1,
        Inst::Piano | Inst::ElectricPiano | Inst::Organ | Inst::Guitar | Inst::NylonGuitar | Inst::Harp => 2,
        Inst::Strings | Inst::Pad | Inst::Choir => 3,
        Inst::SynthPluck => 4,
        Inst::Bell | Inst::Marimba | Inst::Vibraphone => 5,
    }
}

fn layer_class(r: Role) -> u8 {
    match r {
        Role::Lead => 0,
        Role::Bass(_) => 1,
        Role::Comp(_) => 2,
        Role::Pad | Role::Drone => 3,
        Role::Arp => 4,
        Role::Sparse | Role::Harmony | Role::Ostinato => 5,
    }
}

/// The instruments a description names.
pub fn named_instruments(text: &str, extra: &[String]) -> Vec<Inst> {
    let mut found: Vec<(usize, Inst)> = Vec::new();
    let words: Vec<String> = text.to_lowercase().replace([',', '.', ';', '(', ')', '+', '/'], " ").split_whitespace().map(str::to_owned).collect();
    let mut i = 0;
    while i < words.len() {
        let mut matched = false;
        for len in [3, 2, 1] {
            if i + len <= words.len() {
                let phrase = words[i..i + len].join(" ");
                if let Some(inst) = Inst::parse(&phrase) {
                    if !found.iter().any(|(_, f)| *f == inst) {
                        found.push((i, inst));
                    }
                    i += len;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            i += 1;
        }
    }
    let mut out: Vec<Inst> = found.into_iter().map(|(_, i)| i).collect();
    for e in extra {
        if let Some(inst) = Inst::parse(e) {
            if !out.contains(&inst) {
                out.push(inst);
            }
        }
    }
    out
}

fn arrange(style: &Style, named: &[Inst], text: &str) -> Vec<Layer> {
    let mut layers: Vec<Layer> = style.layers.to_vec();
    let t = format!(" {} ", text.to_lowercase());
    let bass_word = t.contains(" bass ") && !named.iter().any(|i| class_of(*i) == 1);
    let mut want: Vec<Inst> = named.to_vec();
    if bass_word {
        let b = layers.iter().find(|l| layer_class(l.role) == 1).map_or(Inst::ElectricBass, |l| l.inst);
        want.push(b);
    }
    for inst in want {
        let cls = class_of(inst);
        if let Some(l) = layers.iter_mut().find(|l| l.inst == inst) {
            l.enter = l.enter.min(0.25);
            continue;
        }
        // Cellos are bass in a score and a tune in a quartet; keep to the style's wants.
        if let Some(l) = layers.iter_mut().find(|l| layer_class(l.role) == cls && !named.contains(&l.inst)) {
            l.inst = inst;
            l.enter = l.enter.min(0.3);
            if let Role::Comp(Comp::Strum) = l.role {
                if !matches!(inst, Inst::Guitar | Inst::NylonGuitar) {
                    l.role = Role::Comp(Comp::Block);
                }
            }
            if let Role::Comp(Comp::Jazz | Comp::Block) = l.role {
                if matches!(inst, Inst::Guitar | Inst::NylonGuitar) {
                    l.role = Role::Comp(Comp::Strum);
                }
            }
            continue;
        }
        let role = match cls {
            0 => Role::Lead,
            1 => Role::Bass(Bass::RootFifth),
            2 => Role::Comp(if matches!(inst, Inst::Guitar | Inst::NylonGuitar) { Comp::Picking } else { Comp::Block }),
            3 => Role::Pad,
            4 => Role::Arp,
            _ => Role::Sparse,
        };
        layers.push(Layer { inst, role, enter: 0.2, db: -4.0, pan: 0.0, send: 0.3 });
    }
    layers
}

fn tonic_chord(style: &Style, key: Key) -> Chord {
    let seventh = matches!(style.name, "jazz" | "lofi" | "ambient");
    let q = match (key.mode.is_minor(), seventh) {
        (true, true) => Q::Min7,
        (true, false) => Q::Min,
        (false, true) => Q::Maj7,
        (false, false) => Q::Maj,
    };
    Chord { root: key.root, q }
}

fn base_energy(arc: Arc, x: f32) -> f32 {
    match arc {
        Arc::Rise => 0.22 + 0.78 * x.max(0.0).powf(1.1),
        Arc::Steady => 0.62,
        Arc::Fall => 0.9 - 0.65 * x,
        Arc::Wave => 0.3 + 0.62 * (PI * x).sin().max(0.0).powf(0.8),
    }
}

/// Writes and renders a piece.
pub fn compose(spec: &MusicSpec) -> Result<Piece, String> {
    let total = spec.seconds;
    if !(0.5..=900.0).contains(&total) {
        return Err(format!("music length {total:.1}s: between 0.5 and 900 seconds"));
    }
    let mut rng = Rng::new(spec.seed);
    let text = format!("{} {}", spec.description, spec.instruments.join(" "));
    let style = find_style(&text).unwrap_or_else(|| style_named("cinematic").unwrap());
    let mood = mood_of(&text);
    let minor = mood.minor.unwrap_or_else(|| matches!(style.name, "dark") || (style.name == "jazz" && text.to_lowercase().contains("noir")));
    let key = match spec.key.as_deref().and_then(|k| Key::parse(k, if minor { style.minor_mode } else { style.major_mode })) {
        Some(k) => k,
        None => {
            let roots = if minor { [9, 2, 4, 7, 0, 5] } else { [0, 7, 2, 5, 9, 4] };
            Key { root: roots[rng.below(roots.len())], mode: if minor { style.minor_mode } else { style.major_mode } }
        }
    };
    let minor = key.mode.is_minor();
    // Tempo.
    let pinned = spec.bpm.is_some();
    let mut bpm = spec.bpm.unwrap_or(style.bpm.0 * mood.tempo) as f64;
    if !pinned {
        bpm = bpm.clamp(style.bpm.1 as f64 * 0.8, style.bpm.2 as f64);
    }
    bpm = bpm.clamp(30.0, 240.0);
    let meter = style.meter as f64;

    // Length, tail and the last chord.
    let mut tail = spec.tail.unwrap_or(if total < 6.0 { total * 0.3 } else { (total * 0.08).clamp(1.5, 4.0) }) as f64;
    tail = tail.clamp(0.15, total as f64 * 0.7);
    let t_end = total as f64 - tail;

    // The grid, bent to land cues and the end. With the tempo left open the
    // first stretch may bend a lot (a cue two seconds in will not wait for a
    // slow tempo); later stretches bend a little from there.
    let mut cues: Vec<Cue> = spec.cues.iter().copied().filter(|c| c.at.is_finite() && c.at > 0.05 && c.at < total - 0.05).collect();
    cues.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut anchors: Vec<(f64, f64)> = vec![(0.0, 0.0)];
    let mut landings = Vec::new();
    let mut cue_beats: Vec<(f64, CueKind)> = Vec::new();
    let mut cur = bpm;
    let flex_for = |first: bool| if pinned { 0.06 } else if first { 0.42 } else { 0.12 };
    for c in &cues {
        let at = c.at as f64;
        let from = *anchors.last().unwrap();
        if at > 0.3 && at < t_end - 0.3 {
            if let Some(b) = snap_beat(from, at, cur, meter, flex_for(anchors.len() == 1)) {
                cur = (b - from.0) / (at - from.1) * 60.0;
                anchors.push((b, at));
                cue_beats.push((b, c.kind));
                landings.push(Landing { at: c.at, kind: c.kind, on_beat: true, on_bar: (b / meter).fract().abs() < 1e-9 });
                continue;
            }
        }
        let b = from.0 + (at - from.1) * cur / 60.0;
        cue_beats.push((b, c.kind));
        landings.push(Landing { at: c.at, kind: c.kind, on_beat: false, on_bar: false });
    }
    let from = *anchors.last().unwrap();
    let ideal = from.0 + (t_end - from.1) * cur / 60.0;
    let final_beat = if pinned && anchors.len() == 1 {
        ideal.floor().max(0.0)
    } else if ideal < 1.0 || t_end <= from.1 {
        ideal.floor().max(from.0)
    } else if let Some(b) = snap_beat(from, t_end, cur, meter, flex_for(anchors.len() == 1)) {
        cur = (b - from.0) / (t_end - from.1) * 60.0;
        anchors.push((b, t_end));
        b
    } else {
        ideal.floor().max(from.0)
    };
    let bpm = if pinned { bpm } else { cur };
    let map = TimeMap { anchors, tail_bpm: cur };
    let final_time = map.time_of(final_beat);
    let bars = ((final_beat / meter).ceil() as usize).max(1);

    // Harmony.
    let bank = if minor { style.minor } else { style.major };
    let prog = bank[rng.below(bank.len())];
    let mut chords: Vec<Chord> = (0..bars).map(|k| { let (s, q) = prog.chords[(k / prog.per as usize) % prog.chords.len()]; Chord { root: (key.root + s).rem_euclid(12), q } }).collect();
    let tonic = tonic_chord(style, key);
    let dominant = Chord { root: (key.root + 7) % 12, q: Q::Dom7 };
    if style.cadence && bars >= 2 && final_beat > 0.0 {
        chords[bars - 1] = dominant;
    }
    let bar_of = |b: f64| ((b / meter + 1e-6).floor() as usize).min(bars - 1);
    for (b, k) in &cue_beats {
        if *k == CueKind::Resolve && *b < final_beat {
            let c = bar_of(*b);
            chords[c] = tonic;
            if c >= 1 {
                chords[c - 1] = dominant;
            }
        }
    }

    // Energy.
    let cap = mood.energy;
    let arc = mood.arc.unwrap_or(style.arc);
    let intro = if bars >= 8 { 1 } else { 0 };
    let mut inten: Vec<f32> = (0..bars)
        .map(|k| {
            let beat = (k as f64 + 0.5) * meter;
            let x = (beat / final_beat.max(1.0)) as f32;
            let mut e = base_energy(arc, x.clamp(0.0, 1.0)) * cap;
            if k < intro {
                e = e.min(0.25);
            }
            if matches!(arc, Arc::Wave | Arc::Fall) && k + 1 == bars && bars > 3 {
                e = e.min(0.5);
            }
            e
        })
        .collect();
    let mut fills = vec![false; bars];
    for (b, kind) in &cue_beats {
        let cb = *b / meter;
        for (k, e) in inten.iter_mut().enumerate() {
            let mid = k as f64 + 0.5;
            match kind {
                CueKind::Hit | CueKind::Drop | CueKind::Riser | CueKind::Swell => {
                    if mid < cb && mid > cb - 2.0 {
                        let f = (1.0 - (cb - mid) / 2.0) as f32;
                        *e = *e + (1.0 - *e) * f * f;
                    } else if mid >= cb && mid < cb + 2.0 && !matches!(kind, CueKind::Swell) {
                        let f = ((mid - cb) / 2.0) as f32;
                        *e = (*e).max(1.0 - f * (1.0 - *e));
                    }
                }
                CueKind::Resolve => {
                    if mid < cb && mid > cb - 1.0 {
                        *e *= 0.85;
                    } else if mid >= cb && mid < cb + 2.0 {
                        *e = (*e).max(0.78);
                    }
                }
            }
        }
        if *kind != CueKind::Resolve && cb >= 1.0 {
            let k = (cb.ceil() as usize).saturating_sub(1);
            if k < bars && (cb.fract() < 1e-6) {
                fills[k] = true;
            }
        }
    }
    for (k, f) in fills.iter_mut().enumerate() {
        if k % 8 == 7 && k + 1 < bars && inten[k] > 0.5 {
            *f = true;
        }
    }
    let cx = Ctx { style, key, minor, meter, bars, chords, inten, final_beat, sparse: mood.sparse, fills, cue_beats: cue_beats.clone() };

    // The band.
    let named = named_instruments(&text, &spec.instruments);
    let layers = arrange(style, &named, &text);
    let no_drums = text.to_lowercase().contains("no drums") || text.to_lowercase().contains("without drums");
    let beat_kind = if no_drums { Beat::None } else if mood.brushes && style.beat == Beat::Jazz { Beat::Brushes } else { style.beat };
    let mut mel = melody(&cx, &mut rng.fork(1));
    let mut tracks: Vec<(Layer, Vec<Ev>)> = Vec::new();
    for (li, l) in layers.iter().enumerate() {
        let mut r = rng.fork(100 + li as u64);
        let evs = match l.role {
            Role::Lead | Role::Harmony => {
                let (lo, hi) = l.inst.range();
                let med = {
                    let mut p: Vec<i32> = mel.iter().map(|e| e.midi as i32).collect();
                    p.sort();
                    p.get(p.len() / 2).copied().unwrap_or(70)
                };
                let center = (lo + hi) / 2;
                let shift = (((center - med) as f32) / 12.0).round() as i32 * 12;
                let on = active_bars(&cx, l.enter);
                let lad = ladder(&cx, 40, 100, 0.0, bars as f64 * meter);
                mel.iter()
                    .filter(|e| on[((e.beat / meter) as usize).min(bars - 1)])
                    .map(|e| {
                        let mut ev = *e;
                        ev.midi = (e.midi as i32 + shift) as f32;
                        if l.role == Role::Harmony {
                            let k = nearest_idx(&lad, ev.midi as i32);
                            ev.midi = lad[k.saturating_sub(2)] as f32;
                            ev.vel *= 0.8;
                        }
                        ev
                    })
                    .collect()
            }
            Role::Comp(k) => gen_comp(&cx, k, l.enter, &mut r),
            Role::Bass(k) => gen_bass(&cx, k, l.inst, l.enter, &mut r),
            Role::Pad => gen_pad(&cx, l.enter),
            Role::Drone => gen_drone(&cx, l.enter),
            Role::Arp => gen_arp(&cx, l.enter),
            Role::Ostinato => gen_ostinato(&cx, l.enter),
            Role::Sparse => gen_sparse(&cx, l.inst, l.enter, &mut r),
        };
        tracks.push((*l, evs));
    }
    mel.clear();
    let drum_layer = Layer { inst: Inst::Piano, role: Role::Pad, enter: style.beat_enter, db: style.beat_db, pan: 0.0, send: 0.12 };
    let mut drum_evs = if beat_kind == Beat::None { Vec::new() } else { gen_drums(&cx, beat_kind, style.beat_enter, &mut rng.fork(7)) };
    // Hold the line at the end: nothing but the last chord past the final beat.
    for (_, evs) in tracks.iter_mut() {
        evs.retain(|e| e.beat < final_beat - 1e-6);
        for e in evs.iter_mut() {
            e.dur = e.dur.min(final_beat - e.beat);
        }
    }
    drum_evs.retain(|e| e.beat < final_beat - 1e-6);
    // A drop: nothing sounds for the beat before the cue.
    for (b, k) in &cue_beats {
        if *k == CueKind::Drop {
            for (_, evs) in tracks.iter_mut() {
                evs.retain(|e| !(e.beat >= b - 1.0 && e.beat < *b));
                for e in evs.iter_mut() {
                    if e.beat < b - 1.0 && e.beat + e.dur > b - 1.0 {
                        e.dur = b - 1.0 - e.beat;
                    }
                }
            }
            drum_evs.retain(|e| !(e.beat >= b - 1.0 && e.beat < *b));
        }
    }
    // The last chord, held through the tail by everyone who can hold it.
    let tail_beats = tail * bpm / 60.0 + 0.5;
    for (l, evs) in tracks.iter_mut() {
        match l.role {
            Role::Pad | Role::Comp(_) | Role::Drone | Role::Lead | Role::Bass(_) | Role::Harmony | Role::Sparse => {
                let (lo, hi) = l.inst.range();
                let notes: Vec<i32> = match l.role {
                    Role::Bass(_) | Role::Drone => vec![tonic.root_at(lo + 3)],
                    Role::Lead | Role::Harmony | Role::Sparse => vec![nearest(tonic.root, ((lo + hi) / 2).max(60))],
                    _ => {
                        let mut v = tonic.stack(tonic.root_at(55), 3);
                        v.push(tonic.root_at(36));
                        v
                    }
                };
                for n in notes {
                    evs.push(note(final_beat, tail_beats, n, 0.75));
                }
            }
            _ => {}
        }
    }
    if beat_kind != Beat::None && final_beat > 0.0 {
        let d = match style.name {
            "jazz" | "lofi" => Some(Drum::Ride),
            "rock" | "pop" | "electronic" | "cinematic" => Some(Drum::Crash),
            "dark" => Some(Drum::Taiko),
            _ => None,
        };
        if let Some(d) = d {
            drum_evs.push(hit(final_beat, d, 0.6));
        }
    }

    // Render.
    let n = frames(total);
    let mut dry = Stereo::silent(n);
    let mut send = Stereo::silent(n);
    let mut notes_written = 0usize;
    let human = style.human_ms / 1000.0;
    let sw = style.swing as f64;
    let swing = |b: f64| -> f64 {
        let w = b.floor();
        let f = b - w;
        if sw > 0.0 && (f - 0.5).abs() < 0.02 { w + 0.5 + sw * 0.5 } else { b }
    };
    let drum_pitch = |e: &Ev| if e.drum == Some(Drum::Timpani) { e.midi } else { 0.0 };
    let mut cache: HashMap<(Drum, u8, u8, i32), Vec<f32>> = HashMap::new();
    for (l, evs) in tracks.iter().map(|(l, e)| (l, e)).chain(std::iter::once((&drum_layer, &drum_evs))) {
        let mut buf = vec![0.0f32; n];
        let mut r = Rng::new(spec.seed ^ (l.inst as u64 * 977 + l.role_tag()));
        for e in evs {
            let t0 = map.time_of(swing(e.beat)) as f32 + e.nudge + if human > 0.0 && e.drum.is_none() { r.range(-human, human) } else { 0.0 };
            if t0 < 0.0 || t0 >= total - 0.01 {
                continue;
            }
            let at = frames(t0);
            let samples: Vec<f32> = match e.drum {
                Some(d) => {
                    let key = (d, (e.vel * 12.0) as u8, r.below(3) as u8, drum_pitch(e) as i32);
                    cache.entry(key).or_insert_with(|| render_drum(d, e.vel, drum_pitch(e), &mut Rng::new(key.2 as u64 * 31 + d as u64))).clone()
                }
                None => {
                    let end = map.time_of(swing(e.beat + e.dur)) as f32;
                    let hold = (end - (t0 - e.nudge.min(0.0)).max(0.0)).max(0.06).min(total - t0 + 1.0);
                    render_note(l.inst, e.midi, hold, e.vel, &mut r)
                }
            };
            notes_written += 1;
            mix_into(&mut buf, &samples, at, 1.0);
        }
        let g = db(l.db);
        dry.add_mono(&buf, 0, g, l.pan);
        send.add_mono(&buf, 0, g * l.send, l.pan);
    }

    // Cues: the sounds that make the moment.
    let stab = layers.iter().find(|l| matches!(l.role, Role::Pad | Role::Comp(_))).map_or(Inst::Strings, |l| l.inst);
    let bar_s = meter * 60.0 / bpm;
    for c in &cues {
        let at = c.at;
        let ch = cx.chord_at(map_beat(&map, at as f64));
        let mut rr = rng.fork(900 + (at * 100.0) as u64);
        match c.kind {
            CueKind::Hit | CueKind::Drop | CueKind::Riser => {
                if c.kind == CueKind::Riser {
                    let rise = (bar_s as f32 * 2.0).clamp(0.8, 6.0).min(at);
                    let x = riser(rise, &mut rr);
                    send.add_mono(&x, frames(at - rise), 0.35, 0.0);
                    dry.add_mono(&x, frames(at - rise), 0.35, 0.0);
                }
                let crash = render_drum(Drum::Crash, 0.85, 0.0, &mut rr);
                dry.add_mono(&crash, frames(at), 0.8, 0.1);
                send.add_mono(&crash, frames(at), 0.5, 0.1);
                let kick = render_drum(if matches!(style.name, "electronic" | "pop") { Drum::Kick808 } else { Drum::Taiko }, 0.95, 0.0, &mut rr);
                dry.add_mono(&kick, frames(at), 0.8, 0.0);
                let bm = boom(72.0, 30.0, 0.9, &mut rr);
                dry.add_mono(&bm, frames(at), 0.7, 0.0);
                for (i, p) in ch.stack(ch.root_at(48), 4).iter().enumerate() {
                    let s = render_note(stab, *p as f32, 0.45, 0.9, &mut rr);
                    dry.add_mono(&s, frames(at), 0.45, -0.3 + i as f32 * 0.2);
                    send.add_mono(&s, frames(at), 0.3, 0.0);
                }
            }
            CueKind::Resolve => {
                let crash = render_drum(Drum::Crash, 0.4, 0.0, &mut rr);
                dry.add_mono(&crash, frames(at), 0.45, 0.0);
                send.add_mono(&crash, frames(at), 0.4, 0.0);
                for p in tonic.stack(tonic.root_at(48), 4) {
                    let s = render_note(Inst::Strings, p as f32, (bar_s as f32 * 2.0).min(total - at), 0.8, &mut rr);
                    dry.add_mono(&s, frames(at), 0.4, 0.0);
                    send.add_mono(&s, frames(at), 0.35, 0.0);
                }
            }
            CueKind::Swell => {
                let rise = (bar_s as f32 * 2.0).clamp(0.8, 6.0).min(at);
                for p in ch.stack(ch.root_at(48), 4) {
                    let mut s = render_note(Inst::Strings, p as f32, rise, 0.9, &mut rr);
                    s.truncate(frames(rise));
                    let len = s.len().max(1);
                    for (i, v) in s.iter_mut().enumerate() {
                        let f = i as f32 / len as f32;
                        *v *= f * f * if i + 480 > len { (len - i) as f32 / 480.0 } else { 1.0 };
                    }
                    dry.add_mono(&s, frames(at - rise), 0.45, 0.0);
                    send.add_mono(&s, frames(at - rise), 0.4, 0.0);
                }
            }
        }
    }

    // Mix: dry plus the room, then loudness and a clean ending.
    let wet = level::reverb(&send, style.room);
    let wet_gain = style.wet * 0.9 * if style.name == "dark" { 1.2 } else { 1.0 };
    let mut mix = dry;
    mix.add_at(&wet, 0, wet_gain);
    level::high_pass(&mut mix, 28.0);
    let (lufs, _) = level::master(&mut mix, spec.lufs as f64);
    mix.edges(0.2_f32.min(tail as f32 * 0.5).max(0.05));
    mix.fit(n);
    let peak = mix.peak();

    // Facts for the reply.
    let beats: Vec<f32> = (0..=(final_beat as usize)).map(|b| map.time_of(b as f64) as f32).collect();
    let bar_times: Vec<f32> = (0..=bars).map(|b| map.time_of(b as f64 * meter) as f32).filter(|t| (*t as f64) <= final_time + 1e-3).collect();
    let mut parts: Vec<String> = layers.iter().filter(|l| tracks.iter().any(|(tl, e)| tl.inst == l.inst && layer_class(tl.role) == layer_class(l.role) && e.len() > 1)).map(|l| l.inst.name().to_owned()).collect();
    if beat_kind != Beat::None && !drum_evs.is_empty() {
        parts.push(if matches!(beat_kind, Beat::Brushes) { "brushes".to_owned() } else { "drums".to_owned() });
    }
    parts.dedup();
    let range = map.bpm_range(final_beat);
    let plan = Plan {
        genre: style.name,
        key: key.name(),
        bpm: bpm as f32,
        bpm_range: range,
        meter: style.meter,
        parts,
        beats,
        bars: bar_times,
        landings,
        final_chord: final_time as f32,
        tail: (total as f64 - final_time) as f32,
        seconds: total,
        lufs: lufs as f32,
        peak,
        notes: notes_written,
    };
    let _ = (cx.minor, &cx.cue_beats);
    Ok(Piece { audio: mix, plan })
}

fn map_beat(map: &TimeMap, t: f64) -> f64 {
    // Inverse of the time map for chord lookup (approximate between anchors).
    let a = &map.anchors;
    let mut i = 0;
    while i + 1 < a.len() && a[i + 1].1 <= t {
        i += 1;
    }
    if i + 1 < a.len() {
        let (b0, t0) = a[i];
        let (b1, t1) = a[i + 1];
        b0 + (t - t0) * (b1 - b0) / (t1 - t0)
    } else {
        a[i].0 + (t - a[i].1) * map.tail_bpm / 60.0
    }
}

trait RoleTag {
    fn role_tag(&self) -> u64;
}

impl RoleTag for Layer {
    fn role_tag(&self) -> u64 {
        match self.role {
            Role::Lead => 1,
            Role::Harmony => 2,
            Role::Comp(_) => 3,
            Role::Bass(_) => 4,
            Role::Pad => 5,
            Role::Arp => 6,
            Role::Ostinato => 7,
            Role::Drone => 8,
            Role::Sparse => 9,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_sound::beats;
    use studio_sound::loudness::measure;

    fn quick(desc: &str, secs: f32) -> MusicSpec {
        MusicSpec::new(desc, secs)
    }

    #[test]
    fn lengths_are_exact_for_every_genre() {
        for s in STYLES {
            let spec = quick(s.name, 9.37);
            let p = compose(&spec).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            assert_eq!(p.audio.len(), frames(9.37), "{}", s.name);
            assert!(p.audio.l.iter().chain(&p.audio.r).all(|v| v.is_finite()), "{}", s.name);
        }
    }

    #[test]
    fn no_clipping_and_loudness_is_on_target() {
        for desc in ["noir jazz with sax and double bass", "hopeful acoustic folk", "dark slow score", "minimal electronic on a beat grid", "epic rock"] {
            let p = compose(&quick(desc, 16.0)).unwrap();
            assert!(p.audio.peak() <= 0.9, "{desc}: peak {}", p.audio.peak());
            let m = measure(&p.audio.l, &p.audio.r, RATE);
            assert!((m.integrated_lufs - (-14.0)).abs() < 1.0, "{desc}: {} LUFS", m.integrated_lufs);
            assert!(m.true_peak_dbtp < -0.5, "{desc}: {} dBTP", m.true_peak_dbtp);
        }
    }

    #[test]
    fn the_tempo_asked_for_is_the_tempo_made() {
        for bpm in [72.0f32, 100.0, 128.0] {
            let mut spec = quick("minimal electronic", 20.0);
            spec.bpm = Some(bpm);
            let p = compose(&spec).unwrap();
            // The plan: every beat is exactly one beat apart.
            let d = p.plan.beats[8] - p.plan.beats[7];
            assert!((60.0 / d - bpm).abs() < 0.05, "{bpm}: {}", 60.0 / d);
            // The audio: an independent beat tracker agrees.
            let a = beats::analyse(&p.audio.l, &p.audio.r, RATE);
            let est = a.bpm.unwrap() as f32;
            let ok = [1.0f32, 2.0, 0.5].iter().any(|k| (est / k - bpm).abs() < bpm * 0.03);
            assert!(ok, "{bpm}: tracker heard {est}");
        }
    }

    #[test]
    fn cues_land_on_beats_at_their_time() {
        let mut spec = quick("dark slow score", 14.0);
        spec.cues = vec![Cue { at: 2.0, kind: CueKind::Hit }, Cue { at: 8.5, kind: CueKind::Resolve }];
        let p = compose(&spec).unwrap();
        assert_eq!(p.plan.landings.len(), 2);
        for l in &p.plan.landings {
            assert!(l.on_beat, "{l:?} {:?}", p.plan.bpm_range);
            assert!(p.plan.beats.iter().any(|b| (b - l.at).abs() < 1e-3), "{l:?} is not a beat time");
        }
        // The hit is audible: much more energy just after 2.0 s than just before.
        let at = frames(2.0);
        let e = |a: usize, b: usize| rms(&p.audio.l[a..b]);
        assert!(e(at, at + frames(0.25)) > e(at - frames(0.25), at) * 1.5);
    }

    #[test]
    fn the_last_chord_falls_at_length_minus_tail() {
        let p = compose(&quick("hopeful folk", 20.0)).unwrap();
        assert!((p.plan.final_chord + p.plan.tail - 20.0).abs() < 0.01);
        assert!(p.plan.tail >= 1.4, "{}", p.plan.tail);
        // Unpinned tempo bends to put the last chord on the time asked.
        let want = 20.0 - p.plan.tail;
        assert!((p.plan.final_chord - want).abs() < 0.05, "{} vs {want}", p.plan.final_chord);
        // Quiet at the very end.
        let n = p.audio.len();
        assert!(rms(&p.audio.l[n - 200..]) < 0.01);
    }

    #[test]
    fn same_seed_same_music_different_seed_different() {
        let a = compose(&quick("jazz", 6.0)).unwrap();
        let b = compose(&quick("jazz", 6.0)).unwrap();
        assert_eq!(a.audio, b.audio);
        let mut s = quick("jazz", 6.0);
        s.seed ^= 99;
        let c = compose(&s).unwrap();
        assert_ne!(a.audio, c.audio);
    }

    #[test]
    fn named_instruments_are_in_the_band() {
        let p = compose(&quick("noir jazz with sax and double bass", 12.0)).unwrap();
        assert_eq!(p.plan.genre, "jazz");
        assert!(p.plan.parts.iter().any(|x| x == "saxophone"), "{:?}", p.plan.parts);
        assert!(p.plan.parts.iter().any(|x| x == "double bass"), "{:?}", p.plan.parts);
        let q = compose(&quick("hopeful acoustic folk", 12.0)).unwrap();
        assert!(q.plan.parts.iter().any(|x| x.contains("guitar")), "{:?}", q.plan.parts);
        assert!(q.plan.key.contains("major") || q.plan.key.contains("mixolydian") || q.plan.key.contains("lydian"), "{}", q.plan.key);
    }

    #[test]
    fn the_key_asked_for_is_used_and_notes_stay_in_it() {
        let mut spec = quick("piano ballad", 12.0);
        spec.key = Some("D minor".into());
        let p = compose(&spec).unwrap();
        assert!(p.plan.key.starts_with("D "), "{}", p.plan.key);
        assert!(p.plan.key.contains("minor") || p.plan.key.contains("harmonic"), "{}", p.plan.key);
    }

    #[test]
    fn very_short_and_very_long_pieces_work() {
        for secs in [0.8f32, 2.0, 3.3] {
            let p = compose(&quick("cinematic", secs)).unwrap();
            assert_eq!(p.audio.len(), frames(secs));
            assert!(p.audio.peak() > 0.05);
        }
        assert!(compose(&quick("x", 0.1)).is_err());
    }
}
