// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dubbing: every subtitle of a translated track spoken by its character's
//! voice for that language (through the speech service chosen in Settings,
//! so the choice holds here too), fitted to when the original actor speaks
//! the line, and laid on a dub lane that plays only in that language's mix
//! (the original dialogue gives way to it; music and effects stay: the M&E).
//!
//! Timing follows the original performance: the line is fitted to the
//! speech the original has inside the subtitle's time (not to the subtitle,
//! which reading time stretches). With `phrases`, a line with as many
//! pauses as the original is fitted phrase by phrase, so mouths open and
//! close where the picture's do; lip sync of drawn or generated faces reads
//! the words file written beside each line (`<line>.words.json`, the same
//! form narration uses) and the clip's `dub` note (language, character,
//! text).

use super::{Desk, Language, LaneMix, SubTrack};
use crate::doc::Doc;
use serde_json::{Value, json};
use studio_model as m;
use studio_sound::align;
use studio_text::subtitles::{base_lang, language_name};

/// Most a dubbed line is sped up or slowed down to fit (beyond this it
/// sounds wrong; the line needs rewording).
pub const MAX_STRETCH: f32 = 1.25;

/// How a line is fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// One stretch onto the original's speech span.
    Span,
    /// Phrase by phrase when the pause counts match, else `Span`.
    Phrases,
    /// As spoken, starting where the original does.
    Natural,
}

impl Fit {
    pub fn from_id(s: &str) -> Option<Fit> {
        Some(match s {
            "span" | "line" | "timing" => Fit::Span,
            "phrases" | "lips" | "lip_sync" | "mouth" => Fit::Phrases,
            "natural" | "none" | "as_spoken" => Fit::Natural,
            _ => return None,
        })
    }
}

/// What happened to one line.
#[derive(Clone, Debug)]
pub struct LineReport {
    pub cue: usize,
    pub character: Option<String>,
    pub start: f64,
    pub end: f64,
    /// Stretch applied (1 = as spoken).
    pub stretch: f64,
    /// The line did not fit even at the stretch limit.
    pub long_by: Option<f64>,
    pub voice: String,
}

/// Speech spans (seconds) of the original dialogue inside `[a, b)`.
fn original_speech(dx: &[f32], rate: u32, a: f64, b: f64) -> Vec<(f64, f64)> {
    let fa = ((a * rate as f64) as usize).min(dx.len());
    let fb = ((b * rate as f64) as usize).min(dx.len());
    if fb <= fa {
        return Vec::new();
    }
    studio_sound::vad::speech_segments(&dx[fa..fb], rate, studio_sound::vad::VadParams::default())
        .into_iter()
        .map(|(s, e)| (a + s as f64 / 1e6, a + e as f64 / 1e6))
        .collect()
}

/// Phrases (seconds from the start) of a synthesised line: it has no
/// background noise, so plain energy finds them (10 ms frames over 40 dB
/// under the line's peak; pauses under 150 ms and blips under 80 ms do
/// not count).
fn line_phrases(x: &[f32], rate: u32) -> Vec<(f64, f64)> {
    let hop = (rate as usize / 100).max(1);
    let e: Vec<f32> = x.chunks(hop).map(|c| c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).collect();
    let peak = e.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return Vec::new();
    }
    let on: Vec<bool> = e.iter().map(|v| *v > peak * 1e-4).collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, &o) in on.iter().enumerate() {
        if !o {
            continue;
        }
        match spans.last_mut() {
            Some(s) if i - s.1 <= 15 => s.1 = i + 1,
            _ => spans.push((i, i + 1)),
        }
    }
    spans.into_iter().filter(|(a, b)| b - a >= 8).map(|(a, b)| (a as f64 * 0.01, b as f64 * 0.01)).collect()
}

/// Trims the synthesiser's silence, keeping 20 ms either side.
fn trim(x: &[f32], rate: u32) -> Vec<f32> {
    let thr = 0.004;
    let first = x.iter().position(|v| v.abs() > thr).unwrap_or(0);
    let last = x.iter().rposition(|v| v.abs() > thr).unwrap_or(x.len().saturating_sub(1));
    let pad = rate as usize / 50;
    x[first.saturating_sub(pad)..(last + pad).min(x.len())].to_vec()
}

/// Fits `line` to the original's speech; returns the audio, where it
/// starts (seconds), the stretch and how much it is still too long.
pub fn fit_line(line: &[f32], rate: u32, original: &[(f64, f64)], cue: (f64, f64), how: Fit) -> (Vec<f32>, f64, f64, Option<f64>) {
    let (span_a, span_b) = match (original.first(), original.last()) {
        (Some(f), Some(l)) => (f.0, l.1),
        _ => cue,
    };
    let target = (span_b - span_a).max(0.1);
    let natural = line.len() as f64 / rate as f64;
    if how == Fit::Natural {
        let long = (natural - (cue.1 - span_a)).max(0.0);
        return (line.to_vec(), span_a, 1.0, (long > 0.05).then_some(long));
    }
    if how == Fit::Phrases && original.len() > 1 {
        let mine = line_phrases(line, rate);
        if mine.len() == original.len() {
            // Map each of the line's phrases onto the original's.
            let mut points = vec![(0.0f32, 0.0f32)];
            for (p, o) in mine.iter().zip(original) {
                points.push((p.0 as f32, (o.0 - span_a) as f32));
                points.push((p.1 as f32, (o.1 - span_a) as f32));
            }
            points.push((natural as f32, (natural * target / natural.max(1e-3)) as f32));
            points.dedup_by(|b, a| b.0 <= a.0 || b.1 <= a.1);
            let map = align::WarpMap { points, cost: 0.0 };
            let out = align::conform(line, rate, &map, MAX_STRETCH);
            let stretch = out.len() as f64 / line.len().max(1) as f64;
            return (out, span_a, stretch, None);
        }
    }
    let (out, f) = align::fit(line, rate, (target * rate as f64) as usize, MAX_STRETCH);
    let long = out.len() as f64 / rate as f64 - target;
    (out, span_a, f, (long > 0.05).then_some(long))
}

/// Word timings of a fitted line: its words spread over its speech by
/// length (seconds from the line start).
fn words_of(text: &str, x: &[f32], rate: u32) -> Vec<Value> {
    let spans = line_phrases(x, rate);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let total: f64 = spans.iter().map(|(a, b)| b - a).sum::<f64>().max(1e-3);
    let spans = if spans.is_empty() { vec![(0.0, x.len() as f64 / rate as f64)] } else { spans };
    let chars: f64 = words.iter().map(|w| w.chars().count() as f64 + 1.0).sum();
    let mut out = Vec::new();
    let mut used = 0.0;
    let at = |d: f64| -> f64 {
        // Speech time `d` into the line → line time.
        let mut left = d;
        for (a, b) in &spans {
            if left <= b - a {
                return a + left;
            }
            left -= b - a;
        }
        spans.last().map_or(d, |s| s.1)
    };
    for w in words {
        let share = (w.chars().count() as f64 + 1.0) / chars * total;
        out.push(json!({"text": w, "start": (at(used) * 1000.0).round() / 1000.0, "end": (at(used + share * 0.9) * 1000.0).round() / 1000.0}));
        used += share;
    }
    out
}

/// A dubbed line ready to lay down: subtitle number, the written file,
/// where it starts and how long it is (seconds), its text and character.
pub type Made = (usize, std::path::PathBuf, f64, f64, String, Option<String>);

/// Dubs `lang` from its subtitle track. `characters`: only these (empty =
/// all); `cues`: only these subtitle numbers (1-based; empty = all).
/// Replaces earlier dubbed lines of the same cues. One undo step.
pub fn dub(doc: &mut Doc, home: Option<&std::path::Path>, lang: &str, how: Fit, only_cues: &[usize], characters: &[String]) -> Result<Vec<LineReport>, String> {
    let (made, reports) = prepare(&doc.model, home, lang, how, only_cues, characters)?;
    lay_down(doc, lang, &made)?;
    Ok(reports)
}

/// Speaks and fits the lines (the slow part: the speech service and the
/// renders), writing each to a file. Pure on the project, so it can run on
/// a worker thread; [`lay_down`] then puts them on the timeline.
pub fn prepare(project: &m::Project, home: Option<&std::path::Path>, lang: &str, how: Fit, only_cues: &[usize], characters: &[String]) -> Result<(Vec<Made>, Vec<LineReport>), String> {
    let desk = Desk::of(project);
    let track: SubTrack = desk.subtitles_of(lang).cloned().ok_or_else(|| format!("no {lang} subtitles to dub from: make or translate them first (subtitles translate)"))?;
    if base_lang(lang) == base_lang(&desk.original_language) {
        return Err(format!("{lang} is the original language"));
    }
    let rate = crate::sound_bridge::RATE;
    // The original dialogue, to time lines against.
    let end = super::render::end_of(project);
    let stereo = studio_sound::surround::Layout::Stereo;
    let req = super::mix::MixRequest { layout: Some(stereo), stem: Some(vec![studio_sound::BusKind::Dialogue]), normalise: false, limiter: false, ..Default::default() };
    let dx = super::render::render(project, &req, 0, end);
    let dx: Vec<f32> = dx[0].iter().zip(&dx[1]).map(|(a, b)| 0.5 * (a + b)).collect();
    let service = crate::agent::voice_service::VoiceSettings::load();
    let dir = home.map(|h| m::file::ProjectFolder::new(h).generated_dir(m::file::MadeKind::Dubs).join(lang)).unwrap_or_else(|| studio_paths::cache().join("dubs").join(lang));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut made: Vec<Made> = Vec::new();
    let mut reports = Vec::new();
    for (i, c) in track.cues.iter().enumerate() {
        if !only_cues.is_empty() && !only_cues.contains(&(i + 1)) {
            continue;
        }
        if !characters.is_empty() && !c.speaker.as_ref().is_some_and(|s| characters.iter().any(|x| x.eq_ignore_ascii_case(s))) {
            continue;
        }
        let text = c.text.replace('\n', " ");
        if text.trim().is_empty() {
            continue;
        }
        let ch = c.speaker.as_ref().and_then(|s| desk.characters.iter().find(|(k, _)| k.eq_ignore_ascii_case(s))).map(|(_, v)| v);
        let voice = ch.and_then(|v| v.voices.get(lang).or_else(|| v.voices.get(base_lang(lang))));
        let line = crate::agent::voice_service::speak_with(&service, &text, voice.map(String::as_str), ch.and_then(|v| v.pace)).map_err(|e| format!("subtitle {}: {e}", i + 1))?;
        let mut samples = trim(&line.samples, line.rate);
        crate::agent::voice_service::level_speech(&mut samples, line.rate);
        let samples = if line.rate == rate { samples } else { studio_sound::resample::resample(&samples, &samples, line.rate, rate, studio_sound::Quality::Best).0 };
        let orig = original_speech(&dx, rate, c.start as f64, c.end as f64);
        let (fitted, at, stretch, long_by) = fit_line(&samples, rate, &orig, (c.start as f64, c.end as f64), how);
        let wav = studio_media::wav::Wav { sample_rate: rate, channels: 2, samples: fitted.iter().flat_map(|s| [*s, *s]).collect() };
        let path = crate::files::write_once(&dir, &format!("line{:04}", i + 1), "wav", &studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("could not write the line: {e}"))?;
        let _ = std::fs::write(path.with_extension("words.json"), serde_json::to_vec(&words_of(&text, &fitted, rate)).unwrap_or_default());
        let dur = fitted.len() as f64 / rate as f64;
        reports.push(LineReport { cue: i + 1, character: c.speaker.clone(), start: at, end: at + dur, stretch, long_by, voice: line.by.clone() });
        made.push((i + 1, path, at, dur, text, c.speaker.clone()));
    }
    if made.is_empty() {
        return Err("no subtitles matched".into());
    }
    Ok((made, reports))
}

/// Lays prepared lines on the language's dub lane (one undo step).
pub fn lay_down(doc: &mut Doc, lang: &str, made: &[Made]) -> Result<(), String> {
    doc.begin_batch(&format!("Dub {}", language_name(lang)));
    match place(doc, lang, made) {
        Ok(()) => {
            doc.commit_batch();
            Ok(())
        }
        Err(e) => {
            doc.rollback_batch();
            Err(e)
        }
    }
}

/// The clip name of a dubbed subtitle: language and subtitle number.
pub fn clip_name(lang: &str, cue: usize) -> String {
    format!("{lang} {cue}")
}

fn place(doc: &mut Doc, lang: &str, made: &[Made]) -> Result<(), String> {
    use crate::view::ClipKind;
    // The dub lane, made once per language.
    let mut desk = Desk::of(&doc.model);
    let existing = desk.languages.iter().find(|l| base_lang(&l.lang) == base_lang(lang)).and_then(|l| l.lane).filter(|id| super::model_lane(doc, *id).is_some());
    let lane = match existing {
        Some(id) => id,
        None => {
            let id = doc.alloc_id();
            let seq = doc.seq();
            let index = doc.model.sequence(seq).map(|s| s.lanes.len());
            let mut l = m::Lane::new(m::LaneId(id), m::LaneKind::Audio, format!("Dub {}", language_name(lang)));
            LaneMix { dub: Some(lang.to_owned()), role: Some("dialogue".into()), ..Default::default() }.store(&mut l);
            if !doc.apply("Add dub lane", m::Edit::AddLane { seq, lane: l, index }) {
                return Err(doc.error.take().unwrap_or_else(|| "the dub lane was refused".into()));
            }
            id
        }
    };
    match desk.languages.iter_mut().find(|l| base_lang(&l.lang) == base_lang(lang)) {
        Some(l) => l.lane = Some(lane),
        None => desk.languages.push(Language { lang: lang.to_owned(), name: language_name(lang), lane: Some(lane) }),
    }
    let settings = desk.stored(&doc.model.settings);
    if settings != doc.model.settings && !doc.apply("Dub language", m::Edit::SetSettings { settings: Box::new(settings) }) {
        return Err(doc.error.take().unwrap_or_else(|| "the language was refused".into()));
    }
    let paths: Vec<std::path::PathBuf> = made.iter().map(|x| x.1.clone()).collect();
    let report = crate::services::project::import_media(doc, &paths);
    if report.ids.len() != paths.len() {
        return Err(format!("the dubbed lines could not be imported: {}", report.failed.join("; ")));
    }
    let lane32 = lane as u32;
    let names: Vec<String> = made.iter().map(|x| clip_name(lang, x.0)).collect();
    let mut clips = Vec::new();
    let mut notes = Vec::new();
    for ((cue, _, at, dur, text, who), media) in made.iter().zip(&report.ids) {
        let mut c = crate::services::library::new_clip((at * 1e6) as i64, (dur * 1e6) as i64, ClipKind::Audio, &clip_name(lang, *cue), Some(*media));
        c.lane = lane32;
        c.id = doc.alloc_id() as u32;
        notes.push((c.id, json!({"lang": lang, "cue": cue, "text": text, "character": who})));
        clips.push(c);
    }
    let ok = doc.edit("Dubbed lines", |p| {
        // Earlier takes of the same subtitles go.
        p.clips.retain(|c| !(c.lane == lane32 && names.contains(&c.name)));
        p.clips.extend(clips);
    });
    if !ok {
        return Err(doc.error.take().unwrap_or_else(|| "the dubbed lines were refused".into()));
    }
    // What each line is, for lip sync and the next pass.
    for (id, note) in notes {
        if let Some((seq, mut c)) = super::dialogue::find_clip(doc, id as u64) {
            c.meta.insert("dub".into(), note);
            doc.apply("Dub note", m::Edit::SetClip { seq, clip: Box::new(c) });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn bursts(at: &[(f64, f64)], len: f64) -> Vec<f32> {
        let mut x = vec![0.0f32; (len * RATE as f64) as usize];
        for &(a, b) in at {
            for i in (a * RATE as f64) as usize..((b * RATE as f64) as usize).min(x.len()) {
                let t = i as f32 / RATE as f32;
                x[i] = 0.3 * (std::f32::consts::TAU * 180.0 * t).sin() * (1.0 + 0.5 * (std::f32::consts::TAU * 360.0 * t).sin());
            }
        }
        x
    }

    #[test]
    fn a_line_is_fitted_to_the_original_speech_not_the_subtitle() {
        let line = bursts(&[(0.0, 1.5)], 1.5);
        let (out, at, f, long) = fit_line(&line, RATE, &[(2.2, 3.4)], (2.0, 4.5), Fit::Span);
        assert!((at - 2.2).abs() < 1e-9);
        assert!((f - 0.8).abs() < 1e-6, "squeezed to the 0.8 limit: {f}");
        assert!(long.is_some_and(|l| (l - 0.0).abs() < 0.2) || long.is_none());
        assert!((out.len() as f64 / RATE as f64 - 1.2).abs() < 0.02);
        // Far too long: held at the limit and reported.
        let long_line = bursts(&[(0.0, 3.0)], 3.0);
        let (_, _, f, long) = fit_line(&long_line, RATE, &[(2.2, 3.4)], (2.0, 4.5), Fit::Span);
        assert!((f - 0.8).abs() < 1e-6);
        assert!(long.unwrap() > 1.0);
    }

    #[test]
    fn phrases_follow_the_original_pauses() {
        // The dub has two phrases, the original pauses later.
        let line = bursts(&[(0.05, 0.75), (1.0, 1.6)], 1.65);
        let orig = [(5.0, 5.9), (6.4, 7.0)];
        let (out, at, _, _) = fit_line(&line, RATE, &orig, (4.8, 7.5), Fit::Phrases);
        assert_eq!(at, 5.0);
        let p = line_phrases(&out, RATE);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!((p[1].0 - 1.4).abs() < 0.08, "second phrase starts with the original's: {p:?}");
        let w = words_of("hola que tal amigo", &out, RATE);
        assert_eq!(w.len(), 4);
        assert!(w[3]["start"].as_f64().unwrap() >= 1.3, "{w:?}");
    }
}
