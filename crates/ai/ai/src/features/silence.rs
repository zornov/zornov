// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Silence and filler-word removal. Nothing here edits the timeline: every
//! function returns [`CutProposal`]s that the UI lists for review (each can
//! be accepted or rejected) before any cut is made.
//!
//! * Silences come from a voice-activity estimate on the audio: 10 ms frame
//!   levels against a noise floor learnt from the clip, with hysteresis and
//!   minimum lengths. A little padding is left at each side so cuts do not
//!   clip breaths and word endings.
//! * Fillers ("um", "uh", "äh", "euh" …) and stutters ("I I") come from the
//!   transcript's words. Speech recognisers often leave fillers out of the
//!   text, so voiced gaps between transcribed words are proposed too, as
//!   "possible filler" with lower confidence.

use crate::cache::{Cacheable, Reader, Writer};
use crate::features::transcribe::Transcript;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalKind {
    Silence,
    Filler,
    /// A repeated word or phrase ("the the").
    Repeat,
}

/// A suggested cut. Times in seconds, `start < end`.
#[derive(Clone, Debug, PartialEq)]
pub struct CutProposal {
    pub start: f64,
    pub end: f64,
    pub kind: ProposalKind,
    /// Shown in the review list ("um", "1.4 s silence").
    pub label: String,
    /// 0..1; the UI can pre-select only confident proposals.
    pub confidence: f32,
}

impl CutProposal {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SilenceSettings {
    /// Level below which audio counts as silent, in dBFS. `None` learns it
    /// from the clip (noise floor + 12 dB, at most −30 dBFS).
    pub threshold_db: Option<f32>,
    /// Shorter pauses are kept (natural rhythm).
    pub min_silence: f64,
    /// Kept at each side of a removed silence.
    pub padding: f64,
}

impl Default for SilenceSettings {
    fn default() -> Self {
        SilenceSettings { threshold_db: None, min_silence: 0.6, padding: 0.12 }
    }
}

impl SilenceSettings {
    pub fn cache_params(&self) -> String {
        format!("v1 thr={:?} min={:.3} pad={:.3}", self.threshold_db, self.min_silence, self.padding)
    }
}

const FRAME: f64 = 0.01;

/// Per-10 ms RMS level in dBFS of mono audio.
pub fn levels_db(mono: &[f32], rate: u32) -> Vec<f32> {
    let n = ((rate as f64 * FRAME) as usize).max(1);
    mono.chunks(n)
        .map(|c| {
            let ms = c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32;
            10.0 * (ms + 1e-12).log10()
        })
        .collect()
}

/// The threshold used when none is given: noise floor (10th percentile of
/// frame levels) + 12 dB, clamped to [−60, −30] dBFS.
pub fn auto_threshold(levels: &[f32]) -> f32 {
    if levels.is_empty() {
        return -40.0;
    }
    let mut s = levels.to_vec();
    s.sort_by(f32::total_cmp);
    (s[s.len() / 10] + 12.0).clamp(-60.0, -30.0)
}

/// Silent stretches worth cutting.
pub fn find_silences(mono: &[f32], rate: u32, settings: &SilenceSettings) -> Vec<CutProposal> {
    if rate == 0 || mono.is_empty() {
        return Vec::new();
    }
    let lv = levels_db(mono, rate);
    let thr = settings.threshold_db.unwrap_or_else(|| auto_threshold(&lv));
    // hysteresis: speech must rise 3 dB above the threshold to end a silence
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let total = mono.len() as f64 / rate as f64;
    for (i, &l) in lv.iter().enumerate() {
        match start {
            None if l < thr => start = Some(i),
            Some(s) if l > thr + 3.0 => {
                push_silence(&mut out, s, i, total, settings);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        push_silence(&mut out, s, lv.len(), total, settings);
    }
    out
}

fn push_silence(out: &mut Vec<CutProposal>, a: usize, b: usize, total: f64, s: &SilenceSettings) {
    let (t0, t1) = (a as f64 * FRAME, (b as f64 * FRAME).min(total));
    if t1 - t0 < s.min_silence {
        return;
    }
    // no padding against the clip edges: leading/trailing silence goes fully
    let start = if a == 0 { 0.0 } else { t0 + s.padding };
    let end = if t1 >= total - 1e-9 { total } else { t1 - s.padding };
    if end - start > 0.05 {
        let len = t1 - t0;
        out.push(CutProposal { start, end, kind: ProposalKind::Silence, label: format!("{len:.1} s silence"), confidence: (0.5 + len as f32 / 4.0).min(1.0) });
    }
}

/// Filler words per language (lower case, without punctuation).
pub fn default_fillers(language: &str) -> &'static [&'static str] {
    match language {
        "en" => &["um", "umm", "uh", "uhh", "uhm", "erm", "er", "ah", "hmm", "mm", "mhm"],
        "de" => &["äh", "ähm", "öhm", "hm", "hmm", "äm"],
        "fr" => &["euh", "heu", "bah", "hum"],
        "es" => &["eh", "em", "este", "mmm"],
        "it" => &["ehm", "eh", "uhm", "mmm"],
        "nl" => &["eh", "uh", "uhm", "ehm"],
        "pt" => &["é", "hã", "hum", "ahn"],
        _ => &["um", "uh", "hmm", "mm"],
    }
}

fn norm_word(w: &str) -> String {
    w.trim().trim_matches(|c: char| c.is_ascii_punctuation() || "“”‘’«»¿¡…".contains(c)).to_lowercase()
}

/// Filler words and immediate repeats in a transcript.
pub fn find_fillers(t: &Transcript, extra_fillers: &[&str]) -> Vec<CutProposal> {
    let list: Vec<String> = default_fillers(&t.language).iter().chain(extra_fillers).map(|w| norm_word(w)).collect();
    let words: Vec<_> = t.words().collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let n = norm_word(&w.text);
        if n.is_empty() {
            continue;
        }
        if list.contains(&n) {
            out.push(CutProposal { start: w.start, end: w.end, kind: ProposalKind::Filler, label: n, confidence: 0.6 + 0.4 * w.probability });
        } else if i > 0 && norm_word(&words[i - 1].text) == n && w.start - words[i - 1].end < 0.5 {
            // keep the second occurrence (usually the fluent one): cut the first
            let p = words[i - 1];
            out.push(CutProposal { start: p.start, end: w.start, kind: ProposalKind::Repeat, label: format!("{n} {n}"), confidence: 0.5 });
        }
    }
    out
}

/// Voiced stretches between transcribed words (likely fillers the
/// recogniser dropped).
pub fn find_untranscribed_sounds(t: &Transcript, mono: &[f32], rate: u32) -> Vec<CutProposal> {
    if rate == 0 {
        return Vec::new();
    }
    let lv = levels_db(mono, rate);
    let thr = auto_threshold(&lv);
    let words: Vec<_> = t.words().collect();
    let mut out = Vec::new();
    for w in words.windows(2) {
        let (a, b) = (w[0].end + 0.05, w[1].start - 0.05);
        if b - a < 0.25 {
            continue;
        }
        let (fa, fb) = ((a / FRAME) as usize, ((b / FRAME) as usize).min(lv.len()));
        if fa >= fb {
            continue;
        }
        let voiced = lv[fa..fb].iter().filter(|&&l| l > thr + 6.0).count() as f64 * FRAME;
        if voiced > 0.2 {
            out.push(CutProposal { start: a, end: b, kind: ProposalKind::Filler, label: "possible filler".into(), confidence: 0.35 });
        }
    }
    out
}

/// Sorts proposals and merges overlapping ones (the stronger kind and
/// label win; confidence is the maximum).
pub fn merge(mut p: Vec<CutProposal>) -> Vec<CutProposal> {
    p.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<CutProposal> = Vec::new();
    for c in p {
        if let Some(last) = out.last_mut()
            && c.start < last.end
        {
            last.end = last.end.max(c.end);
            if c.confidence > last.confidence {
                last.kind = c.kind;
                last.label = c.label;
                last.confidence = c.confidence;
            }
            continue;
        }
        out.push(c);
    }
    out
}

impl Cacheable for CutProposal {
    const TAG: &'static str = "cut-proposal-v1";
    fn encode(&self, w: &mut Writer) {
        w.f64(self.start);
        w.f64(self.end);
        w.u8(self.kind as u8);
        w.str(&self.label);
        w.f32(self.confidence);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        let (start, end) = (r.f64()?, r.f64()?);
        let kind = match r.u8()? {
            0 => ProposalKind::Silence,
            1 => ProposalKind::Filler,
            2 => ProposalKind::Repeat,
            _ => return None,
        };
        Some(CutProposal { start, end, kind, label: r.str()?, confidence: r.f32()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::transcribe::{Segment, Word};

    fn tone(secs: f64, rate: u32, amp: f32) -> Vec<f32> {
        (0..(secs * rate as f64) as usize).map(|i| (i as f32 * 0.07).sin() * amp).collect()
    }

    #[test]
    fn finds_pauses_with_padding() {
        let r = 16_000;
        let mut a = tone(1.0, r, 0.3);
        a.extend(tone(1.5, r, 0.0005)); // quiet room noise
        a.extend(tone(1.0, r, 0.3));
        a.extend(tone(0.3, r, 0.0005)); // short pause: kept
        a.extend(tone(1.0, r, 0.3));
        let s = find_silences(&a, r, &SilenceSettings::default());
        assert_eq!(s.len(), 1, "{s:?}");
        assert!((s[0].start - 1.12).abs() < 0.03 && (s[0].end - 2.38).abs() < 0.03, "{s:?}");
        assert_eq!(s[0].kind, ProposalKind::Silence);
    }

    #[test]
    fn edge_silence_is_cut_fully() {
        let r = 8_000;
        let mut a = tone(1.0, r, 0.0);
        a.extend(tone(1.0, r, 0.3));
        let s = find_silences(&a, r, &SilenceSettings { threshold_db: Some(-40.0), ..Default::default() });
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].start, 0.0);
    }

    fn word(t: &str, s: f64, e: f64) -> Word {
        Word { text: t.into(), start: s, end: e, probability: 0.9 }
    }

    #[test]
    fn fillers_and_repeats_from_words() {
        let t = Transcript {
            language: "en".into(),
            segments: vec![Segment { start: 0.0, end: 5.0, text: String::new(), words: vec![word(" So", 0.0, 0.3), word(" um,", 0.4, 0.8), word(" the", 1.0, 1.2), word(" the", 1.3, 1.5), word(" plan.", 1.6, 2.0)] }],
        };
        let p = find_fillers(&t, &[]);
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!((p[0].kind, p[0].label.as_str(), p[0].start), (ProposalKind::Filler, "um", 0.4));
        assert_eq!((p[1].kind, p[1].start, p[1].end), (ProposalKind::Repeat, 1.0, 1.3));
        // custom filler words
        assert_eq!(find_fillers(&t, &["so"]).len(), 3);
    }

    #[test]
    fn voiced_gaps_are_possible_fillers() {
        let r = 16_000;
        let mut a = tone(1.0, r, 0.3);
        a.extend(tone(0.5, r, 0.0005));
        a.extend(tone(0.6, r, 0.3)); // "uhh", not in the transcript
        a.extend(tone(0.5, r, 0.0005));
        a.extend(tone(1.0, r, 0.3));
        let t = Transcript { language: "en".into(), segments: vec![Segment { start: 0.0, end: 3.6, text: String::new(), words: vec![word(" Hello", 0.0, 1.0), word(" there", 2.6, 3.6)] }] };
        let p = find_untranscribed_sounds(&t, &a, r);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].start < 1.6 && p[0].end > 2.1);
    }

    #[test]
    fn merge_overlaps() {
        let c = |s, e, k, conf| CutProposal { start: s, end: e, kind: k, label: String::new(), confidence: conf };
        let m = merge(vec![c(2.0, 3.0, ProposalKind::Filler, 0.9), c(0.0, 2.5, ProposalKind::Silence, 0.5), c(4.0, 5.0, ProposalKind::Silence, 0.5)]);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].start, m[0].end, m[0].kind), (0.0, 3.0, ProposalKind::Filler));
    }
}
