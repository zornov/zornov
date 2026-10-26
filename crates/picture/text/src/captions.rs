// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Captions: a track of timed cues with word-level timings, caption
//! styles (plain, karaoke highlight, word pop, box per line, bottom bar,
//! speech bubble), speaker colours, automatic segmentation and line
//! breaking by reading speed, safe margins, an "edit the text, keep the
//! timings" helper, and SRT / WebVTT / ASS export (plus SRT / VTT import).
//!
//! The track is plain data (serde); [`CaptionTrack::frame`] renders the
//! cue active at a time with the [`TextEngine`], and `render` composites
//! the result like any other text layer.
//!
//! Conventions follow published subtitle practice (BBC subtitle
//! guidelines, Netflix timed-text style guide): at most two lines, lines
//! broken at phrase boundaries and balanced, a reading speed ceiling in
//! characters per second, a minimum on-screen time, and a small gap
//! between consecutive cues.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::engine::TextEngine;
use crate::font::FontRequest;
use crate::geom::{Ease, Rect};
use crate::paint::{Hooks, UnitPose};
use crate::raster;
use crate::rgba::Rgba;
use crate::style::*;
use crate::Image;

// ---------------------------------------------------------------------------
// Data

/// A spoken word with its timing in seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f32,
    pub end: f32,
    /// Drawn in the highlight colour whatever the time (keyword emphasis).
    #[serde(default, skip_serializing_if = "is_false")]
    pub keyword: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Word {
    pub fn new(text: impl Into<String>, start: f32, end: f32) -> Word {
        Word { text: text.into(), start, end, keyword: false }
    }

    pub fn keyword(mut self) -> Word {
        self.keyword = true;
        self
    }
}

/// Who is speaking; the index into [`CaptionTrack::speakers`] is stored
/// on each cue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
    pub colour: Rgba,
}

/// One caption: a group of words shown together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start: f32,
    pub end: f32,
    pub words: Vec<Word>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<usize>,
    /// Word indices after which the text breaks onto a new line. Empty
    /// means "break automatically".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line_breaks: Vec<usize>,
}

impl Cue {
    /// A cue whose words are timed evenly across `start..end`.
    pub fn from_text(text: &str, start: f32, end: f32) -> Cue {
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let n = tokens.len().max(1) as f32;
        let span = (end - start).max(0.0);
        let words = tokens
            .iter()
            .enumerate()
            .map(|(i, t)| Word::new(*t, start + span * i as f32 / n, start + span * (i + 1) as f32 / n))
            .collect();
        Cue { start, end, words, speaker: None, line_breaks: Vec::new() }
    }

    pub fn with_speaker(mut self, s: usize) -> Cue {
        self.speaker = Some(s);
        self
    }

    /// The words joined by spaces (no line breaks).
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// Number of characters when shown on one line.
    pub fn char_len(&self) -> usize {
        self.words.iter().map(|w| w.text.chars().count()).sum::<usize>() + self.words.len().saturating_sub(1)
    }

    /// Index of the word being spoken at `t` (the last word whose start
    /// is at or before `t`), or `None` before the first word.
    pub fn word_at(&self, t: f32) -> Option<usize> {
        let mut cur = None;
        for (i, w) in self.words.iter().enumerate() {
            if w.start <= t + 1e-4 {
                cur = Some(i);
            } else {
                break;
            }
        }
        cur
    }

    /// Word ranges per line, using explicit breaks when set and otherwise
    /// balanced automatic breaks within `max_chars` per line and
    /// `max_lines` lines.
    pub fn lines(&self, max_chars: usize, max_lines: usize) -> Vec<std::ops::Range<usize>> {
        if !self.line_breaks.is_empty() {
            let mut out = Vec::new();
            let mut start = 0;
            for &b in &self.line_breaks {
                let end = (b + 1).min(self.words.len());
                if end > start {
                    out.push(start..end);
                }
                start = end;
            }
            if start < self.words.len() {
                out.push(start..self.words.len());
            }
            return out;
        }
        break_lines(&self.words, max_chars, max_lines)
    }

    /// The cue text with `\n` between lines.
    pub fn display_text(&self, max_chars: usize, max_lines: usize) -> String {
        self.lines(max_chars, max_lines)
            .into_iter()
            .map(|r| self.words[r].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Replaces the words with the tokens of `new_text`, keeping the
    /// timing of words that survive and interpolating times for new
    /// ones ("edit text → retime words").
    pub fn retime(&mut self, new_text: &str) {
        self.words = retime_words(&self.words, new_text, self.start, self.end);
        self.line_breaks.clear();
    }
}

/// Where captions sit on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPosition {
    #[default]
    Bottom,
    Centre,
    Top,
}

/// What a style does over time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CaptionLook {
    /// Words appear with the cue and stay as styled.
    Plain,
    /// Spoken words turn `highlight`; unspoken ones stay dim.
    Karaoke { highlight: Rgba },
    /// Spoken words turn `highlight` and the current word scales up.
    WordPop { highlight: Rgba, scale: f32 },
    /// A box behind every line (uses the style's background).
    BoxPerLine,
    /// A bar across the whole canvas width behind the text.
    BottomBar { colour: Rgba, padding: f32 },
    /// A rounded bubble with a tail below it.
    SpeechBubble { tail: f32 },
    /// Words appear one by one as they are spoken.
    Reveal,
}

/// Which part of the look takes the speaker's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerTint {
    #[default]
    None,
    Text,
    Highlight,
    Box,
}

/// A caption style: typography plus the look plus layout rules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyle {
    pub id: String,
    pub name: String,
    pub text: TextStyle,
    pub look: CaptionLook,
    #[serde(default)]
    pub position: CaptionPosition,
    /// Fractions of the canvas width / height kept clear at the edges.
    #[serde(default = "default_margins")]
    pub safe_margins: [f32; 2],
    #[serde(default = "two")]
    pub max_lines: usize,
    /// Characters per line before breaking.
    #[serde(default = "default_chars")]
    pub max_chars_per_line: usize,
    /// Reading speed ceiling, characters per second.
    #[serde(default = "default_cps")]
    pub reading_speed_cps: f32,
    #[serde(default)]
    pub speaker_tint: SpeakerTint,
    /// Fade in / out time in seconds (0 = cut).
    #[serde(default)]
    pub fade: f32,
}

fn default_margins() -> [f32; 2] {
    [0.08, 0.08]
}

fn two() -> usize {
    2
}

fn default_chars() -> usize {
    38
}

fn default_cps() -> f32 {
    20.0
}

impl Default for CaptionStyle {
    fn default() -> Self {
        catalogue()[0].clone()
    }
}

impl CaptionStyle {
    /// Builder for custom styles.
    pub fn new(id: &str, name: &str, text: TextStyle, look: CaptionLook) -> CaptionStyle {
        CaptionStyle {
            id: id.into(),
            name: name.into(),
            text,
            look,
            position: CaptionPosition::Bottom,
            safe_margins: default_margins(),
            max_lines: 2,
            max_chars_per_line: default_chars(),
            reading_speed_cps: default_cps(),
            speaker_tint: SpeakerTint::None,
            fade: 0.0,
        }
    }

    pub fn position(mut self, p: CaptionPosition) -> CaptionStyle {
        self.position = p;
        self
    }

    pub fn chars_per_line(mut self, n: usize) -> CaptionStyle {
        self.max_chars_per_line = n.max(8);
        self
    }

    pub fn tint(mut self, t: SpeakerTint) -> CaptionStyle {
        self.speaker_tint = t;
        self
    }

    pub fn fade(mut self, s: f32) -> CaptionStyle {
        self.fade = s.max(0.0);
        self
    }

    /// Minimum on-screen time for a cue of `chars` characters.
    pub fn min_duration(&self, chars: usize) -> f32 {
        (chars as f32 / self.reading_speed_cps.max(1.0)).max(MIN_CUE_SECONDS)
    }
}

/// A caption track: cues, speakers and the style they share.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionTrack {
    pub cues: Vec<Cue>,
    #[serde(default)]
    pub speakers: Vec<Speaker>,
    pub style: CaptionStyle,
    #[serde(default = "default_lang")]
    pub language: String,
}

fn default_lang() -> String {
    "en".into()
}

/// Shortest a cue may stay on screen.
pub const MIN_CUE_SECONDS: f32 = 0.8;
/// Longest a cue may stay on screen.
pub const MAX_CUE_SECONDS: f32 = 6.0;
/// Gap between consecutive cues (two frames at 25 fps).
pub const CUE_GAP_SECONDS: f32 = 0.08;
/// A pause in speech longer than this starts a new cue.
pub const PAUSE_BREAK_SECONDS: f32 = 0.6;

impl Default for CaptionTrack {
    fn default() -> Self {
        CaptionTrack { cues: Vec::new(), speakers: Vec::new(), style: CaptionStyle::default(), language: "en".into() }
    }
}

impl CaptionTrack {
    pub fn new(style: CaptionStyle) -> CaptionTrack {
        CaptionTrack { style, ..Default::default() }
    }

    /// Builds a track from a flat word list (what speech recognition
    /// gives), segmenting into cues by pauses, sentence ends, reading
    /// speed and the style's line limits. `speaker_of(i)` labels words.
    pub fn from_words(words: Vec<Word>, style: CaptionStyle, speaker_of: impl Fn(usize) -> Option<usize>) -> CaptionTrack {
        let mut track = CaptionTrack::new(style);
        track.cues = segment(&words, &track.style, &speaker_of);
        track
    }

    pub fn add_speaker(&mut self, name: &str, colour: Rgba) -> usize {
        self.speakers.push(Speaker { name: name.into(), colour });
        self.speakers.len() - 1
    }

    /// Re-runs segmentation over all words (after a style change, say).
    pub fn resegment(&mut self) {
        let speakers: Vec<Option<usize>> = self.cues.iter().flat_map(|c| c.words.iter().map(move |_| c.speaker)).collect();
        let words: Vec<Word> = self.cues.iter().flat_map(|c| c.words.iter().cloned()).collect();
        self.cues = segment(&words, &self.style, &|i| speakers.get(i).copied().flatten());
    }

    /// Index of the cue showing at `t`.
    pub fn cue_at(&self, t: f32) -> Option<usize> {
        self.cues.iter().position(|c| t >= c.start && t < c.end)
    }

    /// Total length (end of the last cue).
    pub fn duration(&self) -> f32 {
        self.cues.iter().map(|c| c.end).fold(0.0, f32::max)
    }

    /// Sorts cues, trims overlaps and enforces the minimum gap.
    pub fn normalise(&mut self) {
        self.cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        for i in 1..self.cues.len() {
            let prev_end = self.cues[i - 1].end;
            let c = &mut self.cues[i];
            if c.start < prev_end + CUE_GAP_SECONDS {
                let shift = prev_end + CUE_GAP_SECONDS - c.start;
                let min_end = c.start + shift + 0.1;
                c.start += shift;
                if c.end < min_end {
                    c.end = min_end;
                }
            }
        }
    }

    /// Finds and replaces text across every word (case-sensitive).
    pub fn find_replace(&mut self, find: &str, replace: &str) -> usize {
        if find.is_empty() {
            return 0;
        }
        let mut n = 0;
        for c in &mut self.cues {
            for w in &mut c.words {
                if w.text.contains(find) {
                    w.text = w.text.replace(find, replace);
                    n += 1;
                }
            }
            c.words.retain(|w| !w.text.trim().is_empty());
        }
        n
    }

    /// Removes filler words ("um", "uh", "erm", "hmm", "like" is left alone).
    pub fn remove_fillers(&mut self) -> usize {
        const FILLERS: [&str; 8] = ["um", "uh", "erm", "hmm", "mm", "uhm", "ah", "er"];
        let mut n = 0;
        for c in &mut self.cues {
            let before = c.words.len();
            c.words.retain(|w| {
                let t: String = w.text.trim_matches(|ch: char| !ch.is_alphanumeric()).to_lowercase();
                !FILLERS.contains(&t.as_str())
            });
            n += before - c.words.len();
            c.line_breaks.clear();
        }
        self.cues.retain(|c| !c.words.is_empty());
        n
    }

    // -------------------------------------------------------------- Render

    /// Renders the caption shown at `t` into its own image, placed at
    /// (`left`, `top`) on a canvas of `canvas`'s size. `None` between cues.
    pub fn frame(&self, engine: &TextEngine, t: f32, canvas: &Canvas) -> Option<CaptionFrame> {
        let ci = self.cue_at(t)?;
        let cue = &self.cues[ci];
        let style = &self.style;
        let speaker = cue.speaker.and_then(|s| self.speakers.get(s));
        let ppu = canvas.px_per_unit();
        let (mx, my) = (style.safe_margins[0].clamp(0.0, 0.45), style.safe_margins[1].clamp(0.0, 0.45));
        let wrap_units = canvas.units(canvas.width as f32 * (1.0 - 2.0 * mx));

        // Text with line breaks, and a map from layout word index → cue word.
        // The reveal look lays out only the words spoken so far, so its
        // box grows as the text is "typed".
        let lines = cue.lines(style.max_chars_per_line, style.max_lines.max(1));
        let current = cue.word_at(t);
        let limit = if matches!(style.look, CaptionLook::Reveal) { current.map(|c| c + 1).unwrap_or(0) } else { usize::MAX };
        let mut text = String::new();
        let mut word_map: Vec<usize> = Vec::new();
        for (li, r) in lines.iter().enumerate() {
            if r.start >= limit {
                break;
            }
            if li > 0 {
                text.push('\n');
            }
            for (k, wi) in r.clone().enumerate() {
                if wi >= limit {
                    break;
                }
                if k > 0 {
                    text.push(' ');
                }
                let w = &cue.words[wi];
                text.push_str(&w.text);
                for _ in w.text.split_whitespace() {
                    word_map.push(wi);
                }
            }
        }
        if text.trim().is_empty() {
            return None;
        }

        let mut ts = style.text.clone();
        let mut highlight = match &style.look {
            CaptionLook::Karaoke { highlight } | CaptionLook::WordPop { highlight, .. } => Some(*highlight),
            _ => None,
        };
        if let Some(sp) = speaker {
            match style.speaker_tint {
                SpeakerTint::None => {}
                SpeakerTint::Text => ts.fill = Fill::Solid(sp.colour),
                SpeakerTint::Highlight => highlight = Some(sp.colour),
                SpeakerTint::Box => {
                    if let Some(b) = &mut ts.background {
                        b.colour = sp.colour.alpha(b.colour.a);
                    }
                }
            }
        }
        if matches!(style.look, CaptionLook::BoxPerLine) {
            if let Some(b) = &mut ts.background {
                b.per_line = true;
            }
        }
        let keyword_colour = highlight.unwrap_or_else(|| style.text.fill.representative());

        let layout_params = LayoutParams::default().align(Align::Centre).wrap(wrap_units);
        let mut layout_params = layout_params;
        layout_params.max_lines = Some(style.max_lines.max(1));
        let block = TextBlock::plain(text).with_layout(layout_params);
        let layout = engine.layout(&block, &ts, canvas);

        // Per-word hooks.
        let look = style.look.clone();
        let words = cue.words.clone();
        let map = word_map.clone();
        let fade_k = if style.fade > 0.0 {
            let fi = ((t - cue.start) / style.fade).clamp(0.0, 1.0);
            let fo = ((cue.end - t) / style.fade).clamp(0.0, 1.0);
            Ease::Smooth.apply(fi.min(fo))
        } else {
            1.0
        };
        let word_hook = move |i: usize, _n: usize| -> UnitPose {
            let wi = map.get(i).copied().unwrap_or(usize::MAX);
            let spoken = match current {
                Some(c) => wi <= c,
                None => false,
            };
            let is_current = current == Some(wi);
            let keyword = words.get(wi).map(|w| w.keyword).unwrap_or(false);
            let mut pose = UnitPose::REST;
            match &look {
                CaptionLook::Karaoke { highlight } => {
                    if spoken || keyword {
                        pose = pose.colour(*highlight);
                    }
                }
                CaptionLook::WordPop { highlight, scale } => {
                    if spoken || keyword {
                        pose = pose.colour(*highlight);
                    }
                    if is_current {
                        // Pop in over the first 120 ms of the word, then hold.
                        let w = &words[wi];
                        let k = ((t - w.start) / 0.12).clamp(0.0, 1.0);
                        let s = 1.0 + (scale - 1.0) * Ease::Overshoot.apply(k);
                        pose = pose.scale(s);
                    }
                }
                CaptionLook::Reveal => {
                    if !spoken {
                        pose = pose.opacity(0.0);
                    } else if is_current {
                        let w = &words[wi];
                        let k = ((t - w.start) / 0.1).clamp(0.0, 1.0);
                        pose = pose.opacity(k);
                    }
                    if keyword {
                        pose = pose.colour(keyword_colour);
                    }
                }
                _ => {
                    if keyword {
                        pose = pose.colour(keyword_colour);
                    }
                }
            }
            pose
        };
        let hooks = Hooks { words: Some(&word_hook), ..Default::default() };
        let r = engine.paint(&layout, &ts, &hooks);
        if r.image.width == 0 {
            return None;
        }

        // Placement of the layout box on the canvas.
        let (cw, ch) = (canvas.width as f32, canvas.height as f32);
        let box_left = (cw - layout.bounds.w) * 0.5;
        let box_top = match style.position {
            CaptionPosition::Bottom => ch * (1.0 - my) - layout.bounds.h - (r.image.height as f32 + r.top as f32 - layout.bounds.h).max(0.0),
            CaptionPosition::Centre => (ch - layout.bounds.h) * 0.5,
            CaptionPosition::Top => ch * my - (r.top as f32).min(0.0),
        };

        // Extra decoration behind the text.
        let (mut image, mut left, mut top) = (r.image, box_left + r.left as f32, box_top + r.top as f32);
        match &style.look {
            CaptionLook::BottomBar { colour, padding } => {
                let pad = padding * ppu;
                let bar_h = image.height as f32 + 2.0 * pad;
                let bar_top = top - pad;
                let mut bar = Image::from_fn(canvas.width, bar_h.ceil() as u32, |_, _| colour.linear_premul());
                raster::blit_over(&mut bar, &image, left.round() as i32, pad.round() as i32);
                image = bar;
                left = 0.0;
                top = bar_top;
            }
            CaptionLook::SpeechBubble { tail } => {
                let tail_px = tail * ppu;
                let colour = ts.background.as_ref().map(|b| b.colour).unwrap_or(Rgba::WHITE);
                let mut with_tail = Image::new(image.width, image.height + tail_px.ceil() as u32);
                let cx = image.width as f32 * 0.5;
                // The layer carries padding for shadows and strokes, so find
                // the bubble's real bottom edge under the centre column.
                let col = (cx as u32).min(image.width - 1);
                let base = (0..image.height)
                    .rev()
                    .find(|&y| image.pixels[(y * image.width + col) as usize][3] > 0.5)
                    .map(|y| y as f32 + 0.5)
                    .unwrap_or(image.height as f32 - 2.0);
                let path = crate::raster::GlyphPath {
                    commands: vec![
                        swash::zeno::Command::MoveTo(swash::zeno::Vector::new(cx - tail_px * 0.8, base)),
                        swash::zeno::Command::LineTo(swash::zeno::Vector::new(cx + tail_px * 0.4, base)),
                        swash::zeno::Command::LineTo(swash::zeno::Vector::new(cx - tail_px * 0.3, base + tail_px)),
                        swash::zeno::Command::Close,
                    ],
                    bounds: Rect::new(cx - tail_px, base, tail_px * 2.0, tail_px),
                };
                let (ti, tl, tt) = raster::path_image(&path, colour.linear_premul());
                raster::blit_over(&mut with_tail, &ti, tl, tt);
                raster::blit_over(&mut with_tail, &image, 0, 0);
                image = with_tail;
                if matches!(style.position, CaptionPosition::Bottom) {
                    top -= tail_px;
                }
            }
            _ => {}
        }
        if fade_k < 1.0 {
            raster::scale_alpha(&mut image, fade_k);
        }
        Some(CaptionFrame { cue: ci, image, left: left.round() as i32, top: top.round() as i32, text_box: Rect::new(box_left, box_top, layout.bounds.w, layout.bounds.h) })
    }

    /// Composites the caption at `t` over `target`.
    pub fn render_onto(&self, engine: &TextEngine, target: &mut Image, t: f32) {
        let canvas = Canvas::new(target.width, target.height);
        if let Some(f) = self.frame(engine, t, &canvas) {
            raster::blit_over(target, &f.image, f.left, f.top);
        }
    }

    // -------------------------------------------------------------- Export

    /// SubRip.
    pub fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, c) in self.cues.iter().enumerate() {
            let _ = writeln!(out, "{}", i + 1);
            let _ = writeln!(out, "{} --> {}", srt_time(c.start), srt_time(c.end));
            let _ = writeln!(out, "{}", self.cue_text_with_speaker(c, "\n"));
            out.push('\n');
        }
        out
    }

    /// WebVTT; with `word_timings`, every word carries a `<hh:mm:ss.mmm>`
    /// timestamp (karaoke in players that support it).
    pub fn to_vtt(&self, word_timings: bool) -> String {
        let mut out = String::from("WEBVTT\n\n");
        for (i, c) in self.cues.iter().enumerate() {
            let _ = writeln!(out, "{}", i + 1);
            let _ = writeln!(out, "{} --> {} align:middle line:{}", vtt_time(c.start), vtt_time(c.end), match self.style.position {
                CaptionPosition::Bottom => "-2",
                CaptionPosition::Centre => "50%",
                CaptionPosition::Top => "1",
            });
            if word_timings {
                let lines = c.lines(self.style.max_chars_per_line, self.style.max_lines);
                let mut s = String::new();
                if let Some(sp) = c.speaker.and_then(|s| self.speakers.get(s)) {
                    let _ = write!(s, "<v {}>", sp.name);
                }
                for (li, r) in lines.iter().enumerate() {
                    if li > 0 {
                        s.push('\n');
                    }
                    for (k, wi) in r.clone().enumerate() {
                        let w = &c.words[wi];
                        if k > 0 || li > 0 {
                            let _ = write!(s, "<{}>", vtt_time(w.start));
                        }
                        if k > 0 {
                            s.push(' ');
                        }
                        s.push_str(&escape_vtt(&w.text));
                    }
                }
                let _ = writeln!(out, "{s}");
            } else {
                let _ = writeln!(out, "{}", escape_vtt(&self.cue_text_with_speaker(c, "\n")));
            }
            out.push('\n');
        }
        out
    }

    /// Advanced SubStation Alpha with a style built from the caption
    /// style and `\k` karaoke tags when the look highlights words.
    pub fn to_ass(&self, canvas: &Canvas) -> String {
        let st = &self.style;
        let mut out = String::new();
        let _ = writeln!(out, "[Script Info]\nTitle: Studio captions\nScriptType: v4.00+\nWrapStyle: 0\nScaledBorderAndShadow: yes\nPlayResX: {}\nPlayResY: {}\n", canvas.width, canvas.height);
        out.push_str("[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
        let primary = st.text.fill.representative();
        let highlight = match &st.look {
            CaptionLook::Karaoke { highlight } | CaptionLook::WordPop { highlight, .. } => Some(*highlight),
            _ => None,
        };
        // In ASS karaoke the PrimaryColour is the sung colour and
        // SecondaryColour the colour before the syllable is reached.
        let (prim, sec) = match highlight {
            Some(h) => (h, primary),
            None => (primary, primary),
        };
        let outline = st.text.strokes.first();
        let (border_style, back) = match (&st.look, &st.text.background) {
            (CaptionLook::BottomBar { colour, .. }, _) => (3, *colour),
            (_, Some(b)) => (3, b.colour),
            _ => (1, st.text.shadow.as_ref().map(|s| s.colour.fade(s.opacity)).unwrap_or(Rgba::BLACK.alpha(0.0))),
        };
        let size = canvas.px(st.text.size);
        let align = match st.position {
            CaptionPosition::Bottom => 2,
            CaptionPosition::Centre => 5,
            CaptionPosition::Top => 8,
        };
        let margin_x = (canvas.width as f32 * st.safe_margins[0]).round() as i32;
        let margin_y = (canvas.height as f32 * st.safe_margins[1]).round() as i32;
        let _ = writeln!(
            out,
            "Style: Studio,{},{:.0},{},{},{},{},{},{},{},0,100,100,{:.1},0,{},{:.1},{:.1},{},{},{},{},1",
            st.text.font.family,
            size,
            ass_colour(prim),
            ass_colour(sec),
            ass_colour(outline.map(|s| s.colour.fade(s.opacity)).unwrap_or(Rgba::BLACK)),
            ass_colour(back),
            if st.text.font.weight >= 600 { -1 } else { 0 },
            if st.text.font.italic { -1 } else { 0 },
            if st.text.underline { -1 } else { 0 },
            st.text.letter_spacing * 0.01 * size,
            border_style,
            outline.map(|s| canvas.px(s.width)).unwrap_or(0.0),
            st.text.shadow.as_ref().map(|s| canvas.px(s.distance)).unwrap_or(0.0),
            align,
            margin_x,
            margin_x,
            margin_y
        );
        out.push_str("\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
        for c in &self.cues {
            let name = c.speaker.and_then(|s| self.speakers.get(s)).map(|s| s.name.clone()).unwrap_or_default();
            let lines = c.lines(st.max_chars_per_line, st.max_lines);
            let mut text = String::new();
            if let Some(sp) = c.speaker.and_then(|s| self.speakers.get(s)) {
                if st.speaker_tint == SpeakerTint::Text {
                    let _ = write!(text, "{{\\c{}}}", ass_colour(sp.colour));
                }
            }
            let mut cursor = c.start;
            for (li, r) in lines.iter().enumerate() {
                if li > 0 {
                    text.push_str("\\N");
                }
                for (k, wi) in r.clone().enumerate() {
                    let w = &c.words[wi];
                    if k > 0 {
                        text.push(' ');
                    }
                    if highlight.is_some() {
                        // \k takes the syllable length in centiseconds; the
                        // gap before a word is folded into it so the sweep
                        // reaches the word at its start.
                        let len = (w.end.max(cursor) - cursor).max(0.0);
                        let _ = write!(text, "{{\\k{}}}", (len * 100.0).round() as i32);
                        cursor = w.end.max(cursor);
                    }
                    text.push_str(&w.text.replace('{', "(").replace('}', ")"));
                }
            }
            let _ = writeln!(out, "Dialogue: 0,{},{},Studio,{},0,0,0,,{}", ass_time(c.start), ass_time(c.end), name, text);
        }
        out
    }

    fn cue_text_with_speaker(&self, c: &Cue, newline: &str) -> String {
        let body = c.display_text(self.style.max_chars_per_line, self.style.max_lines).replace('\n', newline);
        match c.speaker.and_then(|s| self.speakers.get(s)) {
            Some(sp) if self.speakers.len() > 1 => format!("{}: {}", sp.name, body),
            _ => body,
        }
    }

    // -------------------------------------------------------------- Import

    /// Parses SubRip or WebVTT (detected from the content). Words get even
    /// timings within their cue; a `<v Name>` voice tag or a leading
    /// `Name:` becomes a speaker.
    pub fn parse(text: &str, style: CaptionStyle) -> Result<CaptionTrack, String> {
        let text = text.trim_start_matches('\u{FEFF}');
        let is_vtt = text.trim_start().starts_with("WEBVTT");
        let mut track = CaptionTrack::new(style);
        let mut speakers: HashMap<String, usize> = HashMap::new();
        let blocks = text.replace("\r\n", "\n").split("\n\n").map(|b| b.trim().to_string()).filter(|b| !b.is_empty()).collect::<Vec<_>>();
        for b in blocks {
            let lines: Vec<&str> = b.lines().collect();
            let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
            let timing = lines[ti];
            let mut parts = timing.split("-->");
            let (a, bpart) = (parts.next().unwrap_or("").trim(), parts.next().unwrap_or("").trim());
            let b_time = bpart.split_whitespace().next().unwrap_or("");
            let start = parse_time(a).ok_or_else(|| format!("bad start time: {a}"))?;
            let end = parse_time(b_time).ok_or_else(|| format!("bad end time: {b_time}"))?;
            let body: Vec<String> = lines[ti + 1..].iter().map(|l| strip_tags(l)).collect();
            let mut body_text = body.join("\n");
            let mut speaker: Option<usize> = None;
            // Voice tag or "Name:" prefix.
            let voice = lines[ti + 1..].iter().find_map(|l| {
                let s = l.find("<v ")?;
                let e = l[s..].find('>')?;
                Some(l[s + 3..s + e].trim().to_string())
            });
            let prefix: Option<String> = body_text.lines().next().and_then(|first| {
                let i = first.find(':')?;
                let n = &first[..i];
                let ok = n.len() <= 24 && !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-');
                ok.then(|| n.to_string())
            });
            let name = voice.or_else(|| {
                let n = prefix?;
                body_text = body_text.replacen(&format!("{n}:"), "", 1).trim().to_string();
                Some(n)
            });
            if let Some(n) = name {
                let idx = *speakers.entry(n.clone()).or_insert_with(|| {
                    let i = track.speakers.len();
                    track.speakers.push(Speaker { name: n.clone(), colour: SPEAKER_PALETTE[i % SPEAKER_PALETTE.len()] });
                    i
                });
                speaker = Some(idx);
            }
            let mut cue = Cue::from_text(&body_text, start, end);
            cue.speaker = speaker;
            // Keep the file's own line breaks.
            let mut count = 0;
            let line_lens: Vec<usize> = body_text.lines().map(|l| l.split_whitespace().count()).collect();
            if line_lens.len() > 1 {
                for len in &line_lens[..line_lens.len() - 1] {
                    count += len;
                    if count > 0 {
                        cue.line_breaks.push(count - 1);
                    }
                }
            }
            if !cue.words.is_empty() {
                track.cues.push(cue);
            }
        }
        let _ = is_vtt;
        track.normalise();
        Ok(track)
    }
}

/// Speaker colours assigned in order on import and by `add_speaker`
/// callers that do not choose: distinct, readable on dark video.
pub const SPEAKER_PALETTE: [Rgba; 6] = [
    Rgba::rgb8(255, 209, 102),
    Rgba::rgb8(126, 220, 255),
    Rgba::rgb8(255, 140, 170),
    Rgba::rgb8(146, 255, 170),
    Rgba::rgb8(214, 170, 255),
    Rgba::rgb8(255, 180, 120),
];

/// A rendered caption and where it goes.
pub struct CaptionFrame {
    pub cue: usize,
    pub image: Image,
    pub left: i32,
    pub top: i32,
    /// The text box on the canvas (for hit-testing and on-canvas editing).
    pub text_box: Rect,
}

// ---------------------------------------------------------------------------
// Segmentation and line breaking

fn ends_sentence(w: &str) -> bool {
    w.ends_with(['.', '!', '?', '。', '！', '？']) || w.ends_with("...") || w.ends_with('…')
}

fn ends_clause(w: &str) -> bool {
    ends_sentence(w) || w.ends_with([',', ';', ':', '،', '，', '、'])
}

/// Groups words into cues.
pub fn segment(words: &[Word], style: &CaptionStyle, speaker_of: &dyn Fn(usize) -> Option<usize>) -> Vec<Cue> {
    let max_chars = style.max_chars_per_line * style.max_lines.max(1);
    let mut cues: Vec<Cue> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    let mut cur_chars = 0usize;
    let mut cur_speaker: Option<usize> = None;
    let mut cur_start = 0.0f32;
    let flush = |cur: &mut Vec<Word>, cues: &mut Vec<Cue>, speaker: Option<usize>| {
        if cur.is_empty() {
            return;
        }
        let start = cur[0].start;
        let end = cur.last().unwrap().end;
        cues.push(Cue { start, end, words: std::mem::take(cur), speaker, line_breaks: Vec::new() });
    };
    for (i, w) in words.iter().enumerate() {
        let sp = speaker_of(i);
        let wlen = w.text.chars().count();
        let would_be = cur_chars + wlen + usize::from(!cur.is_empty());
        let gap = cur.last().map(|l| w.start - l.end).unwrap_or(0.0);
        let too_long = !cur.is_empty() && (w.end - cur_start) > MAX_CUE_SECONDS;
        let prev_sentence = cur.last().map(|l| ends_sentence(&l.text)).unwrap_or(false);
        let break_here = !cur.is_empty()
            && (would_be > max_chars
                || gap > PAUSE_BREAK_SECONDS
                || too_long
                || sp != cur_speaker
                || (prev_sentence && cur_chars >= max_chars / 3));
        if break_here {
            flush(&mut cur, &mut cues, cur_speaker);
            cur_chars = 0;
        }
        if cur.is_empty() {
            cur_start = w.start;
            cur_speaker = sp;
        }
        cur_chars += wlen + usize::from(!cur.is_empty());
        cur.push(w.clone());
    }
    flush(&mut cur, &mut cues, cur_speaker);

    // Reading-speed floor and gap between cues.
    for i in 0..cues.len() {
        let min = style.min_duration(cues[i].char_len());
        let next_start = cues.get(i + 1).map(|c| c.start);
        let c = &mut cues[i];
        if c.end - c.start < min {
            c.end = c.start + min;
        }
        if let Some(ns) = next_start {
            if c.end > ns - CUE_GAP_SECONDS {
                c.end = (ns - CUE_GAP_SECONDS).max(c.start + 0.2);
            }
        }
    }
    cues
}

/// Breaks words into at most `max_lines` lines of at most `max_chars`
/// characters, preferring breaks after clause punctuation and balancing
/// line lengths (a top-heavy or bottom-heavy pair reads worse).
pub fn break_lines(words: &[Word], max_chars: usize, max_lines: usize) -> Vec<std::ops::Range<usize>> {
    let n = words.len();
    if n == 0 {
        return vec![];
    }
    let lens: Vec<usize> = words.iter().map(|w| w.text.chars().count()).collect();
    let total: usize = lens.iter().sum::<usize>() + n - 1;
    if total <= max_chars || max_lines <= 1 || n == 1 {
        return vec![0..n];
    }
    let width = |a: usize, b: usize| -> usize { lens[a..b].iter().sum::<usize>() + (b - a).saturating_sub(1) };
    if max_lines == 2 {
        // Try every split; score by balance, punctuation and overflow.
        let mut best: Option<(f32, usize)> = None;
        for k in 1..n {
            let (w1, w2) = (width(0, k), width(k, n));
            let over = (w1.saturating_sub(max_chars) + w2.saturating_sub(max_chars)) as f32 * 10.0;
            let balance = (w1 as f32 - w2 as f32).abs() / total as f32;
            let punct = if ends_clause(&words[k - 1].text) { -0.35 } else { 0.0 };
            // Avoid leaving a dangling article or preposition at a line end.
            let dangling = if is_connective(&words[k - 1].text) { 0.3 } else { 0.0 };
            // Slight preference for a longer first line (pyramid-up reads
            // better over picture than pyramid-down).
            let shape = if w2 > w1 { 0.05 } else { 0.0 };
            let score = over + balance + punct + dangling + shape;
            if best.map(|(s, _)| score < s).unwrap_or(true) {
                best = Some((score, k));
            }
        }
        let k = best.map(|(_, k)| k).unwrap_or(n / 2);
        return vec![0..k, k..n];
    }
    // Three or more lines: greedy fill, then the two-line balancer on the
    // last pair when it helps.
    let mut out = Vec::new();
    let mut start = 0;
    let mut w = 0;
    for i in 0..n {
        let add = lens[i] + usize::from(i > start);
        if w + add > max_chars && i > start && out.len() + 1 < max_lines {
            out.push(start..i);
            start = i;
            w = lens[i];
        } else {
            w += add;
        }
    }
    out.push(start..n);
    out
}

fn is_connective(w: &str) -> bool {
    let t = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    matches!(t.as_str(), "a" | "an" | "the" | "and" | "or" | "but" | "of" | "to" | "in" | "on" | "at" | "for" | "with" | "by" | "from" | "as" | "is" | "are" | "was" | "were")
}

/// Aligns the tokens of `new_text` with `old` (longest common
/// subsequence on lower-cased, punctuation-stripped words); matched words
/// keep their timing, new ones take evenly spaced times between their
/// timed neighbours.
pub fn retime_words(old: &[Word], new_text: &str, cue_start: f32, cue_end: f32) -> Vec<Word> {
    let tokens: Vec<&str> = new_text.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let norm = |s: &str| s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let a: Vec<String> = old.iter().map(|w| norm(&w.text)).collect();
    let b: Vec<String> = tokens.iter().map(|t| norm(t)).collect();
    // LCS table.
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] && !a[i].is_empty() { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut matched: Vec<Option<usize>> = vec![None; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] && !a[i].is_empty() {
            matched[j] = Some(i);
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    // Fill times: runs of unmatched tokens share the span between the
    // previous matched end and the next matched start.
    let mut out: Vec<Word> = tokens.iter().map(|t| Word::new(*t, 0.0, 0.0)).collect();
    for (j, m_) in matched.iter().enumerate() {
        if let Some(i) = m_ {
            out[j].start = old[*i].start;
            out[j].end = old[*i].end;
            out[j].keyword = old[*i].keyword;
        }
    }
    let mut j = 0;
    while j < m {
        if matched[j].is_some() {
            j += 1;
            continue;
        }
        let run_start = j;
        while j < m && matched[j].is_none() {
            j += 1;
        }
        let run_end = j; // exclusive
        let count_f = (run_end - run_start) as f32;
        // Inserted words need some time each; when the gap between the
        // neighbours is too small, borrow up to half of each neighbour.
        let needed = 0.12 * count_f;
        let mut lo = if run_start == 0 { cue_start } else { out[run_start - 1].end };
        let mut hi = if run_end >= m { cue_end.max(lo + 0.3 * count_f) } else { out[run_end].start };
        if hi - lo < needed {
            let deficit = needed - (hi - lo);
            if run_start > 0 {
                let prev = &mut out[run_start - 1];
                let take = (deficit * 0.5).min((prev.end - prev.start) * 0.5).max(0.0);
                prev.end -= take;
                lo = prev.end;
            }
            if run_end < m {
                let next = &mut out[run_end];
                let take = (needed - (hi - lo)).min((next.end - next.start) * 0.5).max(0.0);
                next.start += take;
                hi = next.start;
            } else {
                hi = lo + needed;
            }
        }
        let span = (hi - lo).max(0.05);
        let count = (run_end - run_start) as f32;
        for (k, w) in out[run_start..run_end].iter_mut().enumerate() {
            w.start = lo + span * k as f32 / count;
            w.end = lo + span * (k + 1) as f32 / count;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Time formats

fn split_time(t: f32) -> (u32, u32, u32, u32) {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    ((ms / 3_600_000) as u32, ((ms / 60_000) % 60) as u32, ((ms / 1000) % 60) as u32, (ms % 1000) as u32)
}

pub fn srt_time(t: f32) -> String {
    let (h, m, s, ms) = split_time(t);
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

pub fn vtt_time(t: f32) -> String {
    let (h, m, s, ms) = split_time(t);
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

pub fn ass_time(t: f32) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, (cs / 6000) % 60, (cs / 100) % 60, cs % 100)
}

/// `&HAABBGGRR` with ASS's inverted alpha.
pub fn ass_colour(c: Rgba) -> String {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("&H{:02X}{:02X}{:02X}{:02X}", 255 - q(c.a), q(c.b), q(c.g), q(c.r))
}

/// Parses `HH:MM:SS,mmm`, `HH:MM:SS.mmm`, `MM:SS.mmm` or `H:MM:SS.cc`.
pub fn parse_time(s: &str) -> Option<f32> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.len() {
        3 => (parts[0].parse::<f32>().ok()?, parts[1].parse::<f32>().ok()?, parts[2].parse::<f32>().ok()?),
        2 => (0.0, parts[0].parse::<f32>().ok()?, parts[1].parse::<f32>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn escape_vtt(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Removes `<...>` tags and `{\...}` overrides, decodes the common entities.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0;
    let mut brace = 0;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            '{' => brace += 1,
            '}' if brace > 0 => brace -= 1,
            _ if depth == 0 && brace == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&").trim().to_string()
}

// ---------------------------------------------------------------------------
// Catalogue

fn c(hex: &str) -> Rgba {
    Rgba::hex(hex)
}

fn ts(family: &str, weight: u16, size: f32, fill: Rgba) -> TextStyle {
    TextStyle::new(FontRequest::new(family, weight, false), size, Fill::Solid(fill))
}

fn build() -> Vec<CaptionStyle> {
    use CaptionLook::*;
    vec![
        CaptionStyle::new("caption.clean", "Clean", ts("Inter", 600, 40.0, Rgba::WHITE).with_shadow(Shadow::new(Rgba::BLACK, 4.0, 1.5, 90.0).opacity(0.8)).with_stroke(Stroke::new(c("#000000D9"), 1.4)), Plain)
            .tint(SpeakerTint::Text),
        CaptionStyle::new("caption.sunlit", "Sunlit Karaoke", ts("Montserrat", 800, 46.0, Rgba::WHITE).with_stroke(Stroke::new(Rgba::BLACK, 3.0)).with_shadow(Shadow::new(Rgba::BLACK, 2.0, 4.0, 90.0).opacity(0.7)).case(Case::Upper), Karaoke { highlight: c("#FFD23F") })
            .chars_per_line(22).tint(SpeakerTint::Highlight),
        CaptionStyle::new("caption.pop", "Word Pop", ts("Poppins", 800, 48.0, Rgba::WHITE).with_stroke(Stroke::new(c("#141414"), 2.4)).with_shadow(Shadow::new(Rgba::BLACK, 0.0, 5.0, 90.0).opacity(0.5)).letter_spacing(2.0), WordPop { highlight: c("#5CFF8A"), scale: 1.08 })
            .chars_per_line(20).position(CaptionPosition::Centre),
        CaptionStyle::new("caption.boxed", "Boxed Lines", ts("Inter", 700, 38.0, Rgba::WHITE).with_background(Background::new(c("#000000CC"), 6.0, 14.0, 6.0)).line_spacing(16.0), BoxPerLine)
            .tint(SpeakerTint::Text),
        CaptionStyle::new("caption.bar", "Bottom Bar", ts("Inter", 600, 36.0, Rgba::WHITE), BottomBar { colour: c("#000000B8"), padding: 14.0 })
            .tint(SpeakerTint::Text),
        CaptionStyle::new("caption.bubble", "Speech Bubble", ts("Nunito", 800, 40.0, c("#1C1C1E")).with_background(Background::new(Rgba::WHITE, 24.0, 28.0, 14.0)).line_spacing(8.0), SpeechBubble { tail: 18.0 })
            .chars_per_line(28).tint(SpeakerTint::Box),
        CaptionStyle::new("caption.outline", "Outline Bold", ts("Anton", 400, 56.0, Rgba::WHITE).with_stroke(Stroke::new(Rgba::BLACK, 4.0)).letter_spacing(2.0).case(Case::Upper), Karaoke { highlight: c("#FF4D6D") })
            .chars_per_line(18).position(CaptionPosition::Centre),
        CaptionStyle::new("caption.ticker", "Ticker", ts("JetBrains Mono", 500, 30.0, c("#E8F5E9")).letter_spacing(2.0), BottomBar { colour: c("#0B1A12E6"), padding: 10.0 })
            .chars_per_line(48),
        CaptionStyle::new("caption.serif", "Serif Reader", ts("Lora", 500, 38.0, c("#FFF8EE")).with_shadow(Shadow::new(Rgba::BLACK, 8.0, 0.0, 0.0).opacity(0.9)).line_spacing(10.0), Plain)
            .fade(0.15),
        CaptionStyle::new("caption.neon", "Neon Highlight", ts("Space Grotesk", 700, 42.0, c("#C9F6FF")).with_glow(Glow::new(c("#00E5FF"), 0.6, 8.0)), Karaoke { highlight: c("#FF2E9A") })
            .chars_per_line(26),
        CaptionStyle::new("caption.news", "Newsroom", ts("Oswald", 500, 40.0, Rgba::WHITE).letter_spacing(1.0), BottomBar { colour: c("#0F172AE6"), padding: 16.0 })
            .tint(SpeakerTint::Text),
        CaptionStyle::new("caption.comic", "Comic", ts("Bangers", 400, 52.0, c("#FFE600")).with_stroke(Stroke::new(Rgba::BLACK, 3.0)).letter_spacing(2.0), WordPop { highlight: Rgba::WHITE, scale: 1.1 })
            .chars_per_line(18).position(CaptionPosition::Centre),
        CaptionStyle::new("caption.typed", "Typed", ts("Courier Prime", 700, 40.0, Rgba::WHITE).with_background(Background::new(c("#000000D9"), 2.0, 10.0, 4.0).per_line()), Reveal)
            .chars_per_line(32),
        CaptionStyle::new("caption.soft", "Soft Pill", ts("DM Sans", 700, 38.0, c("#1A1A1A")).with_background(Background::new(c("#FFFFFFE6"), 999.0, 24.0, 10.0).per_line()).line_spacing(26.0), BoxPerLine)
            .chars_per_line(30).tint(SpeakerTint::Box),
    ]
}

/// Every caption style.
pub fn catalogue() -> &'static [CaptionStyle] {
    static ALL: OnceLock<Vec<CaptionStyle>> = OnceLock::new();
    ALL.get_or_init(build)
}

pub fn find(id: &str) -> Option<&'static CaptionStyle> {
    catalogue().iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str, start: f32, per_word: f32) -> Vec<Word> {
        text.split_whitespace().enumerate().map(|(i, t)| Word::new(t, start + i as f32 * per_word, start + (i + 1) as f32 * per_word)).collect()
    }

    #[test]
    fn times_format() {
        assert_eq!(srt_time(3661.5), "01:01:01,500");
        assert_eq!(vtt_time(0.0), "00:00:00.000");
        assert_eq!(ass_time(61.25), "0:01:01.25");
        assert_eq!(parse_time("00:01:01,500"), Some(61.5));
        assert_eq!(parse_time("01:01.5"), Some(61.5));
        assert_eq!(ass_colour(Rgba::rgb8(255, 0, 0)), "&H000000FF");
        assert_eq!(ass_colour(Rgba::rgb8(0, 0, 255).alpha(0.0)), "&HFFFF0000");
    }

    #[test]
    fn segmentation_breaks_on_pause_sentence_and_length() {
        let mut w = words("one two three. four five six seven eight nine ten eleven twelve thirteen fourteen fifteen", 0.0, 0.3);
        // A long pause before "eleven".
        for x in &mut w[10..] {
            x.start += 2.0;
            x.end += 2.0;
        }
        let style = CaptionStyle::default().chars_per_line(20);
        let cues = segment(&w, &style, &|_| None);
        assert!(cues.len() >= 3, "{}", cues.len());
        assert_eq!(cues[0].text(), "one two three.");
        assert!(cues.iter().all(|c| c.char_len() <= 40));
        for pair in cues.windows(2) {
            assert!(pair[1].start - pair[0].end >= CUE_GAP_SECONDS - 1e-4);
        }
        // Reading-speed floor: a two-word cue stays up at least MIN_CUE_SECONDS.
        let short = segment(&words("hi there", 0.0, 0.1), &style, &|_| None);
        assert!(short[0].end - short[0].start >= MIN_CUE_SECONDS - 1e-4);
    }

    #[test]
    fn speaker_change_starts_a_cue() {
        let w = words("hello there general kenobi", 0.0, 0.3);
        let cues = segment(&w, &CaptionStyle::default(), &|i| Some(usize::from(i >= 2)));
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].speaker, Some(1));
    }

    #[test]
    fn line_breaking_balances_and_respects_punctuation() {
        let w = words("When the lights went out, everyone in the room stopped talking", 0.0, 0.2);
        let lines = break_lines(&w, 40, 2);
        assert_eq!(lines.len(), 2);
        let first: Vec<&str> = w[lines[0].clone()].iter().map(|x| x.text.as_str()).collect();
        assert_eq!(first.last().copied(), Some("out,"), "{first:?}");
        let short = break_lines(&words("short line", 0.0, 0.2), 40, 2);
        assert_eq!(short, vec![0..2]);
        // Never ends the first line on an article.
        let w2 = words("I went to the market to buy a few apples", 0.0, 0.2);
        let l2 = break_lines(&w2, 22, 2);
        assert!(!is_connective(&w2[l2[0].end - 1].text), "{:?}", &w2[l2[0].clone()]);
    }

    #[test]
    fn retime_keeps_matched_words() {
        let old = words("the quick brown fox", 1.0, 0.5);
        let new = retime_words(&old, "the very quick fox jumps", 1.0, 3.0);
        assert_eq!(new.len(), 5);
        assert_eq!(new[0].start, 1.0); // the
        assert_eq!(new[2].end, 2.0); // quick
        // "very" borrows time from both neighbours and sits between them.
        assert!(new[0].end <= new[1].start + 1e-4 && new[1].end <= new[2].start + 1e-4, "{new:?}");
        assert!(new[1].end - new[1].start >= 0.1);
        assert_eq!((new[3].start, new[3].end), (2.5, 3.0)); // fox
        assert!(new[4].start >= 3.0 - 1e-4); // jumps after
        for pair in new.windows(2) {
            assert!(pair[0].end <= pair[1].start + 1e-4);
        }
        let mut cue = Cue { start: 1.0, end: 3.0, words: old, speaker: None, line_breaks: vec![1] };
        cue.retime("the quick brown fox again");
        assert_eq!(cue.words.len(), 5);
        assert!(cue.line_breaks.is_empty());
    }

    #[test]
    fn word_at_and_cue_at() {
        let cue = Cue::from_text("a b c d", 0.0, 2.0);
        assert_eq!(cue.word_at(-0.1), None);
        assert_eq!(cue.word_at(0.0), Some(0));
        assert_eq!(cue.word_at(1.0), Some(2));
        assert_eq!(cue.word_at(5.0), Some(3));
        let mut t = CaptionTrack::default();
        t.cues.push(cue);
        t.cues.push(Cue::from_text("e", 3.0, 4.0));
        assert_eq!(t.cue_at(0.5), Some(0));
        assert_eq!(t.cue_at(2.5), None);
        assert_eq!(t.cue_at(3.5), Some(1));
    }

    #[test]
    fn exports_have_expected_shape() {
        let mut t = CaptionTrack::new(find("caption.sunlit").unwrap().clone());
        let a = t.add_speaker("Maya", Rgba::rgb8(255, 200, 0));
        t.cues.push(Cue::from_text("Hello world, this is a test", 0.5, 2.5).with_speaker(a));
        t.cues.push(Cue::from_text("Second cue", 3.0, 4.0));
        let srt = t.to_srt();
        assert!(srt.starts_with("1\n00:00:00,500 --> 00:00:02,500\n"));
        assert!(srt.contains("2\n00:00:03,000 --> 00:00:04,000\nSecond cue\n"));
        let vtt = t.to_vtt(true);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("<v Maya>HELLO") || vtt.contains("<v Maya>Hello"), "{vtt}");
        assert!(vtt.contains("<00:00:0"));
        let ass = t.to_ass(&Canvas::HD);
        assert!(ass.contains("PlayResX: 1920"));
        assert!(ass.contains("Style: Studio,Montserrat,46,&H003FD2FF,&H00FFFFFF"));
        assert!(ass.contains("Dialogue: 0,0:00:00.50,0:00:02.50,Studio,Maya,0,0,0,,"));
        assert!(ass.contains("{\\k"));
    }

    #[test]
    fn parses_srt_and_vtt() {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\nMaya: Hello <i>there</i>\nfriend\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n";
        let t = CaptionTrack::parse(srt, CaptionStyle::default()).unwrap();
        assert_eq!(t.cues.len(), 2);
        assert_eq!(t.speakers[0].name, "Maya");
        assert_eq!(t.cues[0].text(), "Hello there friend");
        assert_eq!(t.cues[0].line_breaks, vec![1]);
        assert_eq!(t.cues[0].speaker, Some(0));
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000 align:middle\n<v Sam>Hi &amp; bye\n";
        let t = CaptionTrack::parse(vtt, CaptionStyle::default()).unwrap();
        assert_eq!(t.cues[0].text(), "Hi & bye");
        assert_eq!(t.speakers[0].name, "Sam");
        // Round trip.
        let back = CaptionTrack::parse(&t.to_srt(), CaptionStyle::default()).unwrap();
        assert_eq!(back.cues[0].text(), "Hi & bye");
    }

    #[test]
    fn fillers_and_replace() {
        let mut t = CaptionTrack::default();
        t.cues.push(Cue::from_text("um so, uh, this is it", 0.0, 2.0));
        assert_eq!(t.remove_fillers(), 2);
        assert_eq!(t.cues[0].text(), "so, this is it");
        assert_eq!(t.find_replace("it", "that"), 1);
        assert_eq!(t.cues[0].text(), "so, this is that");
    }

    #[test]
    fn catalogue_is_valid() {
        let ids: std::collections::HashSet<&str> = catalogue().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), catalogue().len());
        assert!(catalogue().len() >= 12);
        for s in catalogue() {
            let j = serde_json::to_string(s).unwrap();
            let back: CaptionStyle = serde_json::from_str(&j).unwrap();
            assert_eq!(&back, s);
        }
    }
}
