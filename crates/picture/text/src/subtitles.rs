// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Subtitle delivery: the timing and layout rules a subtitle file is held
//! to ([`SubtitleSpec`], with presets after the Netflix timed-text style
//! guide and the BBC subtitle guidelines, and per-language line lengths),
//! conforming a track to them ([`conform`]: frame-accurate times, minimum
//! and maximum durations, the gap between cues, snapping to shot changes),
//! a quality check that lists what still breaks a rule ([`check`]), and
//! translated tracks that keep the original timing ([`translated`]).

use crate::captions::{CaptionTrack, Cue};

/// Rules for a subtitle file.
#[derive(Clone, Debug, PartialEq)]
pub struct SubtitleSpec {
    /// Characters per line.
    pub max_chars_per_line: usize,
    /// Lines per subtitle.
    pub max_lines: usize,
    /// Reading speed ceiling, characters per second (spaces counted).
    pub max_cps: f32,
    /// Shortest a subtitle may stay up, seconds.
    pub min_secs: f32,
    /// Longest, seconds.
    pub max_secs: f32,
    /// Frames kept clear between consecutive subtitles.
    pub gap_frames: u32,
    /// Frame rate the times snap to.
    pub fps: f32,
    /// A subtitle edge within this many frames of a shot change moves to
    /// it (in) or to the gap before it (out).
    pub shot_snap_frames: u32,
}

impl SubtitleSpec {
    /// Netflix timed-text style for a language (ISO 639-1 or -2 code):
    /// 42 characters a line for Latin and most scripts, 16 for Japanese,
    /// Chinese and Korean, 2 lines, 17 characters a second for adult
    /// programmes (20 for some, 13 for children's is a separate choice),
    /// 5/6 s to 7 s on screen, 2 frames between subtitles.
    pub fn netflix(lang: &str, fps: f32) -> SubtitleSpec {
        let (chars, cps) = match base_lang(lang) {
            "ja" => (13, 4.0),
            "zh" => (16, 9.0),
            "ko" => (16, 12.0),
            "ar" | "he" | "fa" | "ur" => (42, 17.0),
            "hi" | "bn" | "ta" | "te" | "mr" | "gu" | "kn" | "ml" | "pa" => (42, 17.0),
            "th" => (35, 17.0),
            _ => (42, 17.0),
        };
        SubtitleSpec { max_chars_per_line: chars, max_lines: 2, max_cps: cps, min_secs: 5.0 / 6.0, max_secs: 7.0, gap_frames: 2, fps: fps.max(1.0), shot_snap_frames: 7 }
    }

    /// BBC subtitle guidelines: 37 characters a line (68% of a 16:9
    /// picture), 2 lines, about 160–180 words a minute (≈ 15 cps), one
    /// second to 7 s on screen, 1 frame between subtitles.
    pub fn bbc(fps: f32) -> SubtitleSpec {
        SubtitleSpec { max_chars_per_line: 37, max_lines: 2, max_cps: 15.0, min_secs: 1.0, max_secs: 7.0, gap_frames: 1, fps: fps.max(1.0), shot_snap_frames: 6 }
    }

    /// Short-form social video: one or two short lines, quick.
    pub fn social(fps: f32) -> SubtitleSpec {
        SubtitleSpec { max_chars_per_line: 32, max_lines: 2, max_cps: 20.0, min_secs: 0.7, max_secs: 5.0, gap_frames: 1, fps: fps.max(1.0), shot_snap_frames: 4 }
    }

    /// A preset by name: `netflix`, `bbc`, `social` (`lang` picks the
    /// Netflix line length).
    pub fn preset(name: &str, lang: &str, fps: f32) -> Option<SubtitleSpec> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "netflix" | "streaming" | "film" | "default" => SubtitleSpec::netflix(lang, fps),
            "bbc" | "broadcast" => SubtitleSpec::bbc(fps),
            "social" | "short" => SubtitleSpec::social(fps),
            _ => return None,
        })
    }

    fn frame(&self) -> f32 {
        1.0 / self.fps
    }

    fn snap(&self, t: f32) -> f32 {
        (t * self.fps).round() / self.fps
    }
}

/// `"pt-BR"` → `"pt"`, `"hin"` → `"hi"` for the three-letter codes subtitle
/// files use for the languages here.
pub fn base_lang(lang: &str) -> &str {
    let l = lang.split(['-', '_']).next().unwrap_or(lang);
    match l {
        "eng" => "en",
        "spa" => "es",
        "fra" | "fre" => "fr",
        "deu" | "ger" => "de",
        "ita" => "it",
        "por" => "pt",
        "hin" => "hi",
        "ben" => "bn",
        "tam" => "ta",
        "tel" => "te",
        "jpn" => "ja",
        "zho" | "chi" => "zh",
        "kor" => "ko",
        "ara" => "ar",
        "heb" => "he",
        "rus" => "ru",
        "tur" => "tr",
        "tha" => "th",
        "urd" => "ur",
        "fas" | "per" => "fa",
        _ => l,
    }
}

/// The ISO 639-2/T code MP4 tracks carry, from a 639-1 or 639-2 code
/// (`"en"` → `"eng"`); `"und"` when unknown.
pub fn iso639_2(lang: &str) -> &'static str {
    match base_lang(&lang.to_ascii_lowercase()) {
        "en" => "eng",
        "es" => "spa",
        "fr" => "fra",
        "de" => "deu",
        "it" => "ita",
        "pt" => "por",
        "hi" => "hin",
        "bn" => "ben",
        "ta" => "tam",
        "te" => "tel",
        "mr" => "mar",
        "gu" => "guj",
        "kn" => "kan",
        "ml" => "mal",
        "pa" => "pan",
        "ja" => "jpn",
        "zh" => "zho",
        "ko" => "kor",
        "ar" => "ara",
        "he" => "heb",
        "ru" => "rus",
        "tr" => "tur",
        "th" => "tha",
        "ur" => "urd",
        "fa" => "fas",
        "nl" => "nld",
        "pl" => "pol",
        "sv" => "swe",
        "id" => "ind",
        "vi" => "vie",
        "uk" => "ukr",
        "el" => "ell",
        "sw" => "swa",
        _ => "und",
    }
}

/// The language's own name for the track menus ("English", "हिन्दी").
pub fn language_name(lang: &str) -> String {
    match base_lang(&lang.to_ascii_lowercase()) {
        "en" => "English",
        "es" => "Español",
        "fr" => "Français",
        "de" => "Deutsch",
        "it" => "Italiano",
        "pt" => "Português",
        "hi" => "हिन्दी",
        "bn" => "বাংলা",
        "ta" => "தமிழ்",
        "te" => "తెలుగు",
        "mr" => "मराठी",
        "ja" => "日本語",
        "zh" => "中文",
        "ko" => "한국어",
        "ar" => "العربية",
        "he" => "עברית",
        "ru" => "Русский",
        "tr" => "Türkçe",
        "ur" => "اردو",
        other => return other.to_owned(),
    }
    .to_owned()
}

/// Characters of a cue as the reading-speed rule counts them (spaces in,
/// line breaks out).
fn chars(c: &Cue) -> usize {
    c.char_len()
}

/// Brings a track in line with `spec`: line limits on the style, times
/// snapped to frames, edges snapped to nearby shot changes (`shots`, in
/// seconds), each cue lengthened to its reading time and the minimum (as
/// far as the next cue allows) and cut to the maximum, and the gap kept.
/// Returns how many cues changed.
pub fn conform(track: &mut CaptionTrack, spec: &SubtitleSpec, shots: &[f32]) -> usize {
    track.style.max_chars_per_line = spec.max_chars_per_line;
    track.style.max_lines = spec.max_lines;
    track.style.reading_speed_cps = spec.max_cps;
    track.cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    let gap = spec.gap_frames as f32 * spec.frame();
    let near = spec.shot_snap_frames as f32 * spec.frame();
    let mut changed = 0;
    let n = track.cues.len();
    for i in 0..n {
        let before = (track.cues[i].start, track.cues[i].end);
        let next_start = track.cues.get(i + 1).map(|c| spec.snap(c.start));
        let c = &mut track.cues[i];
        let mut s = spec.snap(c.start);
        let mut e = spec.snap(c.end);
        // Shot changes: an in point just before a cut moves onto the cut;
        // an out point near a cut ends `gap` before it.
        if let Some(&cut) = shots.iter().find(|&&cut| (cut - s).abs() <= near) {
            s = spec.snap(cut);
        }
        if let Some(&cut) = shots.iter().find(|&&cut| (cut - e).abs() <= near) {
            e = spec.snap(cut - gap).max(s + spec.frame());
        }
        // Reading time and the minimum.
        let want = (chars(c) as f32 / spec.max_cps.max(1.0)).max(spec.min_secs);
        if e - s < want {
            e = ((s + want) * spec.fps - 1e-3).ceil() / spec.fps;
        }
        if e - s > spec.max_secs {
            e = spec.snap(s + spec.max_secs);
        }
        if let Some(ns) = next_start
            && e > ns - gap
        {
            e = (ns - gap).max(s + spec.frame());
        }
        c.start = s;
        c.end = e;
        if (before.0 - s).abs() > 1e-4 || (before.1 - e).abs() > 1e-4 {
            changed += 1;
        }
        c.line_breaks.clear();
    }
    changed
}

/// One rule a subtitle breaks.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// Cue index (0-based).
    pub cue: usize,
    /// What is wrong, in words.
    pub problem: String,
}

/// Lists every rule each cue still breaks.
pub fn check(track: &CaptionTrack, spec: &SubtitleSpec) -> Vec<Issue> {
    let mut out = Vec::new();
    let gap = spec.gap_frames as f32 * spec.frame() - 1e-3;
    for (i, c) in track.cues.iter().enumerate() {
        let d = c.end - c.start;
        let mut say = |p: String| out.push(Issue { cue: i, problem: p });
        if d < spec.min_secs - 1e-3 {
            say(format!("on screen {d:.2} s, under the {:.2} s minimum", spec.min_secs));
        }
        if d > spec.max_secs + 1e-3 {
            say(format!("on screen {d:.2} s, over the {:.1} s maximum", spec.max_secs));
        }
        let cps = chars(c) as f32 / d.max(0.01);
        if cps > spec.max_cps + 0.05 {
            say(format!("reads at {cps:.1} characters a second (limit {:.0}); shorten the text or give it more time", spec.max_cps));
        }
        let lines = c.lines(spec.max_chars_per_line, spec.max_lines);
        if lines.len() > spec.max_lines {
            say(format!("{} lines (limit {})", lines.len(), spec.max_lines));
        }
        for r in &lines {
            let w = &c.words[r.clone()];
            let len = w.iter().map(|x| x.text.chars().count()).sum::<usize>() + w.len().saturating_sub(1);
            if len > spec.max_chars_per_line {
                say(format!("a line of {len} characters (limit {})", spec.max_chars_per_line));
            }
        }
        if let Some(next) = track.cues.get(i + 1) {
            if next.start < c.end {
                say(format!("overlaps the next subtitle by {:.2} s", c.end - next.start));
            } else if next.start - c.end < gap {
                say(format!("only {:.0} frames before the next subtitle (need {})", ((next.start - c.end) * spec.fps).round(), spec.gap_frames));
            }
        }
    }
    out
}

/// A track in another language with the source's timing: cue `i` gets
/// `texts[i]` (words spread evenly over the cue), the same speaker, and
/// the language code. Missing texts keep the source text.
pub fn translated(source: &CaptionTrack, texts: &[String], lang: &str) -> CaptionTrack {
    let mut t = source.clone();
    t.language = lang.to_owned();
    for (i, c) in t.cues.iter_mut().enumerate() {
        if let Some(text) = texts.get(i).filter(|s| !s.trim().is_empty()) {
            let speaker = c.speaker;
            *c = Cue::from_text(text, c.start, c.end);
            c.speaker = speaker;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captions::{CaptionStyle, Word};

    fn track(cues: &[(&str, f32, f32)]) -> CaptionTrack {
        let mut t = CaptionTrack::new(CaptionStyle::default());
        t.cues = cues.iter().map(|(s, a, b)| Cue::from_text(s, *a, *b)).collect();
        t
    }

    #[test]
    fn conform_snaps_to_frames_extends_short_cues_and_keeps_the_gap() {
        let mut t = track(&[("Hi.", 1.013, 1.3), ("Where are you going at this hour of the night?", 1.35, 2.0), ("Home.", 9.0, 30.0)]);
        let spec = SubtitleSpec::netflix("en", 24.0);
        conform(&mut t, &spec, &[]);
        for c in &t.cues {
            assert!(((c.start * 24.0) - (c.start * 24.0).round()).abs() < 1e-3, "{} on a frame", c.start);
        }
        // First cue would be 0.83 s but the second starts 0.34 s later:
        // it ends two frames before it.
        assert!((t.cues[0].end - (t.cues[1].start - 2.0 / 24.0)).abs() < 1e-3, "{:?}", t.cues[0]);
        // The long line gets its reading time: 46 characters at 17 a second.
        assert!(t.cues[1].end - t.cues[1].start >= 46.0 / 17.0 - 1e-3, "{:?}", t.cues[1]);
        assert!((t.cues[2].end - t.cues[2].start - 7.0).abs() < 1e-3, "capped at 7 s");
        let issues = check(&t, &spec);
        assert!(issues.iter().any(|i| i.cue == 0 && i.problem.contains("minimum")), "{issues:?}");
        assert!(!issues.iter().any(|i| i.cue == 2), "{issues:?}");
    }

    #[test]
    fn shot_changes_pull_edges_onto_the_cut() {
        let mut t = track(&[("One line here.", 2.05, 4.0)]);
        let spec = SubtitleSpec::netflix("en", 25.0);
        conform(&mut t, &spec, &[2.0, 4.12]);
        assert!((t.cues[0].start - 2.0).abs() < 1e-3, "{:?}", t.cues[0]);
        assert!((t.cues[0].end - (4.12 - 0.08)).abs() < 1e-3, "{:?}", t.cues[0]);
    }

    #[test]
    fn check_finds_fast_and_long_lines_and_translation_keeps_timing() {
        let t = track(&[("This sentence is far too long to read in the very short time it is given on screen.", 0.0, 1.0)]);
        let spec = SubtitleSpec::netflix("en", 25.0);
        let issues = check(&t, &spec);
        assert!(issues.iter().any(|i| i.problem.contains("characters a second")), "{issues:?}");
        let mut src = track(&[("Good morning.", 1.0, 2.5), ("See you.", 3.0, 4.0)]);
        src.cues[0].speaker = Some(0);
        let es = translated(&src, &["Buenos días.".into(), "Hasta luego.".into()], "es");
        assert_eq!(es.language, "es");
        assert_eq!((es.cues[0].start, es.cues[0].end), (1.0, 2.5));
        assert_eq!(es.cues[0].speaker, Some(0));
        assert_eq!(es.cues[1].text(), "Hasta luego.");
        assert_eq!(es.to_srt().lines().nth(2), Some("Buenos días."));
        let _ = Word::new("x", 0.0, 1.0);
    }

    #[test]
    fn language_codes() {
        assert_eq!(iso639_2("en"), "eng");
        assert_eq!(iso639_2("hi-IN"), "hin");
        assert_eq!(iso639_2("jpn"), "jpn");
        assert_eq!(iso639_2("xx"), "und");
        assert_eq!(SubtitleSpec::netflix("ja", 24.0).max_chars_per_line, 13);
        assert_eq!(language_name("es"), "Español");
    }
}
