// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Phoneme timing: the sounds of each word (dictionary, else rules) laid
//! on the audio. A left-to-right segmental alignment: every phoneme is a
//! state with a typical sound (loudness, hiss, hum, voicing) and a typical
//! length, stops are a closure then a burst, and the best path through the
//! frames of the voice decides where each begins. Word times, when known
//! (from speech recognition or the narration engine), fence each word in.

use crate::dict;
use crate::features::{self, FPS, Frames};
use crate::phoneme::{Class, Ph, Timed};

/// A word to align: its text and, if known, when it is said (seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct WordIn {
    pub text: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
}

impl WordIn {
    pub fn timed(text: &str, start: f64, end: f64) -> WordIn {
        WordIn { text: text.into(), start: Some(start), end: Some(end) }
    }
    pub fn plain(text: &str) -> WordIn {
        WordIn { text: text.into(), start: None, end: None }
    }
}

/// Phonemes with times for a whole line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alignment {
    pub phones: Vec<Timed>,
    /// Each word's span, `(start, end)`, in order of the words given
    /// (words with no sounds, such as punctuation, have `None`).
    pub words: Vec<Option<(f64, f64)>>,
    /// How the timing was found.
    pub method: Method,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Method {
    /// Laid on the audio's own loudness, hiss and voicing.
    #[default]
    Acoustic,
    /// Spread over word times by typical lengths (no audio, or the
    /// acoustic path failed).
    Estimated,
    /// Given by the speech engine.
    Engine,
}

impl Alignment {
    pub fn end(&self) -> f64 {
        self.phones.last().map_or(0.0, |p| p.end)
    }

    /// A plain-text form to keep beside a sound file.
    pub fn to_tsv(&self) -> String {
        let mut s = format!("#method\t{}\n", match self.method { Method::Acoustic => "acoustic", Method::Estimated => "estimated", Method::Engine => "engine" });
        for p in &self.phones {
            s += &format!("{}\t{}\t{:.4}\t{:.4}\t{}\n", p.ph.name(), p.stress, p.start, p.end, p.word.map_or(-1, |w| w as i64));
        }
        s
    }

    pub fn from_tsv(t: &str) -> Option<Alignment> {
        let mut a = Alignment::default();
        for l in t.lines() {
            if let Some(m) = l.strip_prefix("#method\t") {
                a.method = match m.trim() { "estimated" => Method::Estimated, "engine" => Method::Engine, _ => Method::Acoustic };
                continue;
            }
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 5 {
                continue;
            }
            let (ph, _) = Ph::parse(f[0])?;
            let w: i64 = f[4].parse().ok()?;
            a.phones.push(Timed { ph, stress: f[1].parse().ok()?, start: f[2].parse().ok()?, end: f[3].parse().ok()?, word: (w >= 0).then_some(w as usize) });
        }
        let nw = a.phones.iter().filter_map(|p| p.word).max().map_or(0, |m| m + 1);
        a.words = (0..nw)
            .map(|w| {
                let ps: Vec<&Timed> = a.phones.iter().filter(|p| p.word == Some(w)).collect();
                Some((ps.first()?.start, ps.last()?.end))
            })
            .collect();
        (!a.phones.is_empty()).then_some(a)
    }
}

// ------------------------------------------------------------ the sound models

#[derive(Clone, Copy)]
struct Tmpl {
    /// (mean, weight) of loudness, hiss, hum, voicing.
    m: [(f32, f32); 4],
}

const fn t(l: (f32, f32), h: (f32, f32), u: (f32, f32), v: (f32, f32)) -> Tmpl {
    Tmpl { m: [l, h, u, v] }
}

struct Seg {
    ph: Ph,
    stress: u8,
    word: Option<usize>,
    /// Index of the phoneme this belongs to (a stop has two segs).
    owner: usize,
    tm: Tmpl,
    dmin: usize,
    dmean: f64,
    dmax: usize,
    /// Allowed span in frames of the group.
    lo: usize,
    hi: usize,
    free: bool,
}

fn parts(ph: Ph) -> Vec<(Tmpl, usize, f64, usize)> {
    use Ph::*;
    let n = (0.0, 0.0);
    let k = |s: Ph| s.typical() * FPS;
    match ph.class() {
        Class::Vowel => vec![(t((0.75, 1.0), (0.08, 1.0), n, (0.85, 1.0)), 3, k(ph), 40)],
        Class::Glide => vec![(t((0.55, 1.0), (0.1, 1.0), n, (0.75, 1.0)), 2, k(ph), 24)],
        Class::Nasal => vec![(t((0.4, 1.0), (0.05, 1.0), (0.8, 0.5), (0.7, 1.0)), 3, k(ph), 24)],
        Class::VoicedFric => {
            let h = if matches!(ph, DH | V) { 0.25 } else { 0.5 };
            vec![(t((0.4, 1.0), (h, 1.0), n, (0.5, 0.8)), 2, k(ph), 22)]
        }
        Class::UnvoicedFric => match ph {
            S | SH => vec![(t((0.45, 1.0), (0.8, 1.2), n, (0.05, 0.8)), 3, k(ph), 34)],
            HH => vec![(t((0.3, 1.0), (0.3, 0.7), n, (0.2, 0.7)), 2, k(ph), 16)],
            _ => vec![(t((0.15, 1.0), (0.45, 1.0), n, (0.05, 0.8)), 3, k(ph), 26)],
        },
        Class::VoicedStop => vec![(t((0.06, 1.2), (0.1, 0.3), n, (0.3, 0.6)), 2, 4.5, 22), (t((0.35, 1.0), (0.5, 1.0), n, (0.35, 0.5)), 1, 2.0, 8)],
        Class::UnvoicedStop => vec![(t((0.06, 1.2), (0.1, 0.3), n, (0.1, 0.8)), 2, 5.0, 22), (t((0.3, 1.0), (0.6, 1.0), n, (0.1, 0.8)), 1, 4.0, 14)],
        Class::Affricate => vec![(t((0.06, 1.2), (0.1, 0.3), n, (0.15, 0.8)), 2, 4.5, 20), (t((0.45, 1.0), (0.75, 1.0), n, (0.15, 0.8)), 2, 7.0, 24)],
        Class::Silence => vec![(t((0.0, 1.6), (0.0, 0.0), n, (0.0, 0.6)), 0, 10.0, 400)],
    }
}

fn emit(tm: &Tmpl, f: &Frames, i: usize) -> f32 {
    let x = [f.loud[i], f.hf[i], f.lf[i], f.voice[i]];
    let mut c = 0.0;
    for (k, &(mu, w)) in tm.m.iter().enumerate() {
        let z = (x[k] - mu) / 0.28;
        c += w * z * z;
    }
    -0.5 * c
}

/// Phonemes (with stress) of each word; `None` where a word has no sounds.
fn pronounce_all(words: &[WordIn]) -> Vec<Vec<(Ph, u8)>> {
    words.iter().map(|w| dict::pronounce(&w.text)).collect()
}

/// Lays the words' phonemes on the audio.
pub fn align(mono: &[f32], rate: u32, words: &[WordIn]) -> Result<Alignment, String> {
    let prons = pronounce_all(words);
    if prons.iter().all(|p| p.is_empty()) {
        return Err("no speakable words to align".into());
    }
    let frames = features::analyse(mono, rate);
    if frames.len() < 5 {
        return Err("the sound is too short to align".into());
    }
    let total = frames.len() as f64 / FPS;
    let timed = words.iter().all(|w| w.start.is_some() && w.end.is_some());
    // Groups of words aligned together.
    let spoken: Vec<usize> = (0..words.len()).filter(|&i| !prons[i].is_empty()).collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    if timed {
        let mut cur: Vec<usize> = Vec::new();
        for &i in &spoken {
            if let Some(&first) = cur.first() {
                let span = words[i].end.unwrap() - words[first].start.unwrap();
                let gap = words[i].start.unwrap() - words[*cur.last().unwrap()].end.unwrap();
                if (span > 20.0 && gap >= 0.1) || span > 30.0 || gap > 0.8 {
                    groups.push(std::mem::take(&mut cur));
                }
            }
            cur.push(i);
        }
        if !cur.is_empty() {
            groups.push(cur);
        }
    } else {
        if total > 90.0 {
            return Err("more than 90 s of speech needs word times to align".into());
        }
        groups.push(spoken.clone());
    }
    let mut phones: Vec<Timed> = Vec::new();
    let mut method = Method::Acoustic;
    for (gi, g) in groups.iter().enumerate() {
        let (a, b) = if timed {
            let a = if gi == 0 { 0.0 } else { (words[g[0]].start.unwrap() - 0.25).max(phones.last().map_or(0.0, |p| p.end)) };
            let b = if gi + 1 == groups.len() { total } else { (words[*g.last().unwrap()].end.unwrap() + 0.25).min(total) };
            (a, b)
        } else {
            (0.0, total)
        };
        let (f0, f1) = ((a * FPS) as usize, ((b * FPS) as usize).min(frames.len()));
        if f1 <= f0 + 3 {
            continue;
        }
        let got = align_group(&frames, f0, f1, g, words, &prons, timed).or_else(|| {
            method = Method::Estimated;
            None
        });
        match got {
            Some(mut p) => phones.append(&mut p),
            None => {
                // Spread by typical lengths over the words' own times.
                phones.append(&mut estimate_group(g, words, &prons, a, b));
            }
        }
    }
    phones.sort_by(|x, y| x.start.total_cmp(&y.start));
    let mut al = Alignment { phones, words: Vec::new(), method };
    al.words = word_spans(&al.phones, words.len());
    Ok(al)
}

fn word_spans(ph: &[Timed], n: usize) -> Vec<Option<(f64, f64)>> {
    (0..n)
        .map(|w| {
            let v: Vec<&Timed> = ph.iter().filter(|p| p.word == Some(w)).collect();
            Some((v.first()?.start, v.last()?.end))
        })
        .collect()
}

/// Without audio: phonemes spread over the words' times (or evenly),
/// longer for vowels and stress.
pub fn estimate(words: &[WordIn]) -> Alignment {
    let prons = pronounce_all(words);
    let all: Vec<usize> = (0..words.len()).filter(|&i| !prons[i].is_empty()).collect();
    let mut t = 0.0;
    let mut phones = Vec::new();
    for &i in &all {
        let w = &words[i];
        let need: f64 = prons[i].iter().map(|p| p.0.typical()).sum();
        let (s, e) = match (w.start, w.end) {
            (Some(s), Some(e)) if e > s => (s, e),
            _ => (t, t + need + 0.04),
        };
        phones.extend(spread(i, &prons[i], s, e));
        t = e + 0.04;
    }
    let mut al = Alignment { phones, words: Vec::new(), method: Method::Estimated };
    al.words = word_spans(&al.phones, words.len());
    al
}

fn spread(word: usize, pr: &[(Ph, u8)], s: f64, e: f64) -> Vec<Timed> {
    let weights: Vec<f64> = pr.iter().map(|&(p, st)| p.typical() * if st == 1 { 1.25 } else { 1.0 }).collect();
    let sum: f64 = weights.iter().sum::<f64>().max(1e-6);
    let mut t = s;
    pr.iter()
        .zip(&weights)
        .map(|(&(ph, stress), w)| {
            let d = (e - s) * w / sum;
            let p = Timed { ph, stress, start: t, end: t + d, word: Some(word) };
            t += d;
            p
        })
        .collect()
}

fn estimate_group(g: &[usize], words: &[WordIn], prons: &[Vec<(Ph, u8)>], a: f64, b: f64) -> Vec<Timed> {
    let mut out = Vec::new();
    let mut t = a;
    let total: f64 = g.iter().map(|&i| prons[i].iter().map(|p| p.0.typical()).sum::<f64>() + 0.04).sum::<f64>().max(1e-6);
    for &i in g {
        let need: f64 = prons[i].iter().map(|p| p.0.typical()).sum::<f64>() + 0.04;
        let (s, e) = match (words[i].start, words[i].end) {
            (Some(s), Some(e)) if e > s => (s, e),
            _ => (t, t + (b - a) * need / total),
        };
        out.extend(spread(i, &prons[i], s, e));
        t = e;
    }
    out
}

fn align_group(f: &Frames, f0: usize, f1: usize, g: &[usize], words: &[WordIn], prons: &[Vec<(Ph, u8)>], timed: bool) -> Option<Vec<Timed>> {
    let tn = f1 - f0;
    // Speech rate for the untimed case: frames with some voice.
    let speech_frames = (f0..f1).filter(|&i| f.loud[i] > 0.15).count() as f64;
    let mut all_mean = 0.0;
    for &i in g {
        all_mean += prons[i].iter().map(|p| p.0.typical() * FPS).sum::<f64>();
    }
    let global_scale = (speech_frames / all_mean.max(1.0)).clamp(0.5, 2.5);
    let build = |strict: bool| -> Vec<Seg> {
        let mut segs: Vec<Seg> = Vec::new();
        let mut owner = 0usize;
        let sil = |segs: &mut Vec<Seg>, owner: &mut usize, dmin: usize, dmax: usize| {
            let (tm, _, mean, _) = parts(Ph::Sil)[0];
            segs.push(Seg { ph: Ph::Sil, stress: 0, word: None, owner: *owner, tm, dmin, dmean: mean, dmax, lo: 0, hi: tn, free: true });
            *owner += 1;
        };
        sil(&mut segs, &mut owner, 0, tn);
        for (gi, &wi) in g.iter().enumerate() {
            let w = &words[wi];
            let pr = &prons[wi];
            let (lo, hi) = if timed && strict {
                let pad = 8.0;
                (((w.start.unwrap() * FPS - pad) - f0 as f64).max(0.0) as usize, (((w.end.unwrap() * FPS + pad) - f0 as f64).max(1.0) as usize).min(tn))
            } else {
                (0, tn)
            };
            let scale = if timed {
                let span = (w.end.unwrap() - w.start.unwrap()) * FPS;
                let need: f64 = pr.iter().map(|p| p.0.typical() * FPS).sum();
                (span / need.max(1.0)).clamp(0.6, 2.5)
            } else {
                global_scale
            };
            for &(ph, stress) in pr {
                for (k, (tm, dmin, mean, dmax)) in parts(ph).into_iter().enumerate() {
                    let _ = k;
                    let mean = mean * scale * if stress == 1 && ph.is_vowel() { 1.2 } else { 1.0 };
                    let dmin = if strict { dmin } else { dmin.min(1) };
                    segs.push(Seg { ph, stress, word: Some(wi), owner, tm, dmin, dmean: mean, dmax: dmax.max((mean * 2.5) as usize), lo, hi, free: false });
                }
                owner += 1;
            }
            if gi + 1 < g.len() {
                sil(&mut segs, &mut owner, 0, 120);
            }
        }
        sil(&mut segs, &mut owner, 0, tn);
        segs
    };
    for strict in [true, false] {
        let segs = build(strict);
        if let Some(bounds) = dp(f, f0, tn, &segs) {
            return Some(collect(&segs, &bounds, f0));
        }
    }
    None
}

/// Best path; returns each segment's `(start, end)` frame in the group.
fn dp(f: &Frames, f0: usize, tn: usize, segs: &[Seg]) -> Option<Vec<(usize, usize)>> {
    let j_n = segs.len();
    let neg = f32::NEG_INFINITY;
    let mut prev = vec![neg; tn + 1];
    prev[0] = 0.0;
    let mut back: Vec<Vec<u16>> = Vec::with_capacity(j_n);
    for s in segs {
        // Prefix sums of this state's emissions.
        let mut cum = vec![0.0f32; tn + 1];
        for i in 0..tn {
            cum[i + 1] = cum[i] + emit(&s.tm, f, f0 + i);
        }
        let sd = (0.5 * s.dmean + 1.5) as f32;
        let mut cur = vec![neg; tn + 1];
        let mut bk = vec![0u16; tn + 1];
        for end in 0..=tn {
            if !s.free && end > s.hi {
                break;
            }
            let dmax = s.dmax.min(end).min(u16::MAX as usize);
            for d in s.dmin..=dmax {
                let start = end - d;
                if !s.free && d > 0 && start < s.lo {
                    break;
                }
                let p = prev[start];
                if p == neg {
                    continue;
                }
                let pen = if d == 0 {
                    1.5
                } else if s.free {
                    0.002 * d as f32
                } else {
                    let z = (d as f32 - s.dmean as f32) / sd;
                    0.5 * z * z
                };
                let v = p + cum[end] - cum[start] - pen;
                if v > cur[end] {
                    cur[end] = v;
                    bk[end] = d as u16;
                }
            }
        }
        back.push(bk);
        prev = cur;
    }
    if prev[tn] == neg {
        return None;
    }
    let mut out = vec![(0usize, 0usize); j_n];
    let mut t = tn;
    for j in (0..j_n).rev() {
        let d = back[j][t] as usize;
        out[j] = (t - d, t);
        t -= d;
    }
    Some(out)
}

fn collect(segs: &[Seg], b: &[(usize, usize)], f0: usize) -> Vec<Timed> {
    let mut out: Vec<Timed> = Vec::new();
    let mut last_owner = usize::MAX;
    for (s, &(a, e)) in segs.iter().zip(b) {
        let (ts, te) = ((f0 + a) as f64 / FPS, (f0 + e) as f64 / FPS);
        if s.owner == last_owner {
            if let Some(l) = out.last_mut() {
                l.end = te;
            }
            continue;
        }
        last_owner = s.owner;
        if e == a && s.ph == Ph::Sil {
            continue;
        }
        out.push(Timed { ph: s.ph, stress: s.stress, start: ts, end: te, word: s.word });
    }
    // Merge neighbouring silences.
    let mut merged: Vec<Timed> = Vec::new();
    for p in out {
        match merged.last_mut() {
            Some(l) if l.ph == Ph::Sil && p.ph == Ph::Sil => l.end = p.end,
            _ => merged.push(p),
        }
    }
    merged
}
