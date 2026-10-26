// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The teleprompter: a script scrolling at speaking pace, or following the
//! speaker by what a live transcription hears, drawn large (optionally
//! mirrored for a prompter glass).
//!
//! The script is a list of [`PromptLine`]s (the app builds them from the
//! story's scenes, or from plain text). Position is a word number with a
//! fraction; speaker names and stage directions are shown but never
//! waited for. Voice-follow is tolerant: the last few heard words are
//! matched against the script just ahead of (and a little behind) the
//! reader, with spelling slips and dropped words allowed, and when
//! nothing matches the prompter holds instead of running away.

use crate::frame::Frame;
use crate::text::{TextOpts, draw_text, wrap_lines};
use std::ops::Range;
use studio_text::Align;

/// One line of the script as given.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptLine {
    /// Who speaks (shown as a heading before their lines).
    pub who: Option<String>,
    pub text: String,
    /// A direction, not speech: shown dimmed, never waited for.
    pub action: bool,
}

#[derive(Clone, Debug)]
struct PWord {
    text: String,
    norm: String,
    spoken: bool,
}

#[derive(Clone, Debug)]
struct Para {
    words: Range<usize>,
    kind: ParaKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParaKind {
    Heading,
    Speech,
    Action,
}

/// The script, split into words.
#[derive(Clone, Debug)]
pub struct Script {
    words: Vec<PWord>,
    paras: Vec<Para>,
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

impl Script {
    pub fn from_lines(lines: &[PromptLine]) -> Script {
        let mut words = Vec::new();
        let mut paras = Vec::new();
        let mut last_who: Option<&str> = None;
        for l in lines {
            if l.text.trim().is_empty() {
                continue;
            }
            if let (Some(w), false) = (l.who.as_deref().filter(|w| !w.is_empty()), l.action) {
                if last_who != Some(w) {
                    let s = words.len();
                    for t in format!("{}:", w.to_uppercase()).split_whitespace() {
                        words.push(PWord { text: t.into(), norm: norm(t), spoken: false });
                    }
                    paras.push(Para { words: s..words.len(), kind: ParaKind::Heading });
                }
                last_who = Some(w);
            }
            let s = words.len();
            for t in l.text.split_whitespace() {
                words.push(PWord { text: t.into(), norm: norm(t), spoken: !l.action });
            }
            paras.push(Para { words: s..words.len(), kind: if l.action { ParaKind::Action } else { ParaKind::Speech } });
        }
        Script { words, paras }
    }

    /// Plain text: paragraphs split at blank lines; a line like `(pause)`
    /// or `[smile]` is a direction.
    pub fn from_text(text: &str) -> Script {
        let lines: Vec<PromptLine> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| {
                let action = (l.starts_with('(') && l.ends_with(')')) || (l.starts_with('[') && l.ends_with(']'));
                PromptLine { who: None, text: l.to_owned(), action }
            })
            .collect();
        Script::from_lines(&lines)
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// How many words are spoken (not headings or directions).
    pub fn spoken_len(&self) -> usize {
        self.words.iter().filter(|w| w.spoken).count()
    }

    /// Seconds to read the script at `wpm`.
    pub fn duration(&self, wpm: f64) -> f64 {
        self.spoken_len() as f64 / wpm.max(1.0) * 60.0
    }

    fn text(&self) -> String {
        self.paras.iter().map(|p| self.words[p.words.clone()].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
    }
}

/// How the position moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// At a steady pace, words per minute.
    Pace(f64),
    /// With the speaker, by what is heard.
    Follow,
    /// Only where the operator puts it.
    Manual,
}

#[derive(Clone, Debug)]
pub struct View {
    /// Text size in pixels of the output frame.
    pub size: f32,
    pub mirror: bool,
    /// Margin each side as a fraction of the width.
    pub margin: f32,
    /// Where the current line sits, 0 (top) to 1 (bottom).
    pub reading_line: f32,
}

impl Default for View {
    fn default() -> Self {
        View { size: 64.0, mirror: false, margin: 0.08, reading_line: 0.38 }
    }
}

pub struct Prompter {
    script: Script,
    pub mode: Mode,
    pub view: View,
    running: bool,
    /// Words completed (fractional inside a word).
    pos: f64,
    /// The smoothed position the picture shows.
    shown: f64,
    hold: f64,
    heard: Vec<String>,
    since_match: usize,
    lost: bool,
}

/// What the prompter reports.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Status {
    pub word: usize,
    pub of: usize,
    pub running: bool,
    pub percent: f64,
    /// In follow mode: nothing recently heard matches the script.
    pub lost: bool,
    pub next_words: String,
    pub seconds_left: f64,
}

impl Prompter {
    pub fn new(script: Script, mode: Mode) -> Prompter {
        Prompter { script, mode, view: View::default(), running: false, pos: 0.0, shown: 0.0, hold: 0.0, heard: Vec::new(), since_match: 0, lost: false }
    }

    /// Shows the position now, without easing (a picture asked for out of time).
    pub fn settle(&mut self) {
        self.shown = self.pos;
    }

    pub fn start(&mut self) {
        self.running = true;
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    pub fn word(&self) -> usize {
        (self.pos.floor() as usize).min(self.script.len())
    }

    pub fn jump(&mut self, word: usize) {
        self.pos = word.min(self.script.len()) as f64;
        self.hold = 0.0;
        self.heard.clear();
        self.skip_unspoken();
    }

    /// Back to the start of the paragraph the reader is in (or the one before).
    pub fn back_paragraph(&mut self) {
        let w = self.word();
        let mut target = 0;
        for p in &self.script.paras {
            if p.words.start + 1 < w {
                target = p.words.start;
            }
        }
        self.jump(target);
    }

    fn skip_unspoken(&mut self) {
        while (self.pos as usize) < self.script.len() && !self.script.words[self.pos as usize].spoken {
            self.pos = self.pos.floor() + 1.0;
        }
    }

    fn crossing_pause(&self, from: f64, to: f64) -> bool {
        let (a, b) = (from.floor() as usize, to.floor() as usize);
        a != b && self.script.paras.iter().any(|p| p.words.end == b && p.kind == ParaKind::Speech)
    }

    /// Time passes. In pace mode the position moves; the picture eases
    /// towards it either way.
    pub fn tick(&mut self, dt: f64) {
        if self.running {
            if let Mode::Pace(wpm) = self.mode {
                if self.hold > 0.0 {
                    self.hold -= dt;
                } else if (self.pos as usize) < self.script.len() {
                    let before = self.pos;
                    self.pos = (self.pos + wpm / 60.0 * dt).min(self.script.len() as f64);
                    if self.crossing_pause(before, self.pos) {
                        // A breath between paragraphs.
                        self.hold = 0.5;
                    }
                    self.skip_unspoken();
                }
            }
        }
        // Critically damped easing: reaches the target in about 0.5 s.
        let k = 1.0 - (-dt * 8.0).exp();
        self.shown += (self.pos - self.shown) * k;
    }

    /// Words just heard (in order, as a live transcription delivers them).
    pub fn hear(&mut self, words: &[&str]) {
        if self.mode != Mode::Follow {
            return;
        }
        for w in words {
            let n = norm(w);
            if n.is_empty() {
                continue;
            }
            self.heard.push(n);
            if self.heard.len() > 8 {
                self.heard.remove(0);
            }
            self.since_match += 1;
            self.follow_step();
        }
        self.lost = self.since_match >= 6;
    }

    fn follow_step(&mut self) {
        let spoken: Vec<usize> = (0..self.script.len()).filter(|&i| self.script.words[i].spoken).collect();
        if spoken.is_empty() {
            return;
        }
        // Position as an index into `spoken`.
        let here = spoken.partition_point(|&i| (i as f64) < self.pos);
        let lo = here.saturating_sub(6);
        let hi = (here + 40).min(spoken.len());
        let mut best: Option<(f64, usize)> = None;
        for end in lo..hi {
            // How well the last k heard words fit the script ending at `end`.
            let mut score = 0.0;
            let mut k = 0;
            let mut newest = None;
            for (back, h) in self.heard.iter().rev().take(5).enumerate() {
                if back > end {
                    break;
                }
                k += 1;
                let sw = &self.script.words[spoken[end - back]].norm;
                if same_word(h, sw) {
                    score += 1.0 / (1.0 + back as f64 * 0.15);
                    newest.get_or_insert(back);
                }
            }
            if k == 0 {
                continue;
            }
            // A lone match is believed only just ahead of the reader.
            let need = if self.heard.len() >= 3 { 2.0 } else { 1.0 };
            let ahead = end as i64 - here as i64;
            if score < need || (self.heard.len() >= 3 && score < 1.6) {
                continue;
            }
            if self.heard.len() < 3 && !(0..=3).contains(&ahead) {
                continue;
            }
            // Prefer the match closest ahead of the reader.
            let dist = (ahead as f64 - 1.0).abs();
            let s = score * 10.0 - dist * 0.3;
            if best.is_none_or(|(b, _)| s > b) {
                best = Some((s, end - newest.unwrap_or(0)));
            }
        }
        if let Some((_, end)) = best {
            let to = (spoken[end] + 1) as f64;
            if to != self.pos {
                self.since_match = 0;
                self.lost = false;
            }
            self.pos = to;
            self.skip_unspoken();
        }
    }

    pub fn status(&self) -> Status {
        let w = self.word();
        let next: Vec<&str> = self.script.words.iter().skip(w).take(8).map(|x| x.text.as_str()).collect();
        let wpm = if let Mode::Pace(p) = self.mode { p } else { 150.0 };
        let left = self.script.words.iter().skip(w).filter(|x| x.spoken).count() as f64 / wpm * 60.0;
        Status { word: w, of: self.script.len(), running: self.running, percent: if self.script.is_empty() { 100.0 } else { 100.0 * self.pos / self.script.len() as f64 }, lost: self.lost, next_words: next.join(" "), seconds_left: left }
    }

    /// The prompter picture at `w × h`.
    pub fn render(&self, w: u32, h: u32) -> Frame {
        let mut f = Frame::new(w, h, [0, 0, 0, 255]);
        let size = self.view.size.max(8.0);
        let margin = w as f32 * self.view.margin;
        let wrap = w as f32 - 2.0 * margin;
        let opts = |c: [u8; 4], weight: u16| TextOpts { size, colour: c, weight, family: None, align: Align::Left, wrap: Some(wrap), plate: None };
        // Lines of every paragraph, as they wrap, with their word numbers.
        struct L {
            text: String,
            words: Range<usize>,
            kind: ParaKind,
            first: bool,
        }
        let mut lines: Vec<L> = Vec::new();
        for p in &self.script.paras {
            let text = self.script.words[p.words.clone()].iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" ");
            let mut at = p.words.start;
            for (i, t) in wrap_lines(&text, &opts([255; 4], 600), h, w).into_iter().enumerate() {
                let n = t.split_whitespace().count();
                lines.push(L { text: t, words: at..at + n, kind: p.kind, first: i == 0 });
                at += n;
            }
        }
        let line_h = size * 1.35;
        let cur_word = self.shown;
        let cur_idx = lines.iter().position(|l| (cur_word as usize) < l.words.end).unwrap_or(lines.len().saturating_sub(1));
        // Fractional line: scroll smoothly between lines.
        let frac = lines.get(cur_idx).map(|l| ((cur_word - l.words.start as f64) / l.words.len().max(1) as f64).clamp(0.0, 1.0)).unwrap_or(0.0);
        let focus_y = h as f32 * self.view.reading_line;
        for (i, l) in lines.iter().enumerate() {
            let y = focus_y + (i as f32 - cur_idx as f32 - frac as f32) * line_h + if l.first && i > 0 { 0.0 } else { 0.0 };
            if y < -line_h || y > h as f32 + line_h {
                continue;
            }
            let (colour, weight) = match (l.kind, i.cmp(&cur_idx)) {
                (ParaKind::Heading, _) => ([240, 190, 60, 255], 700),
                (ParaKind::Action, _) => ([140, 160, 190, 255], 400),
                (_, std::cmp::Ordering::Less) => ([110, 110, 110, 255], 600),
                (_, std::cmp::Ordering::Equal) => ([255, 255, 255, 255], 700),
                (_, std::cmp::Ordering::Greater) => ([225, 225, 225, 255], 600),
            };
            let o = opts(colour, weight);
            let d = draw_text(&mut f, &l.text, margin + wrap * 0.5, y, &o);
            // The word being said, underlined.
            if i == cur_idx && l.kind == ParaKind::Speech {
                let wi = (cur_word as usize).saturating_sub(l.words.start);
                if let Some(b) = d.words.get(wi) {
                    f.fill_rect(b[0] as i32, (b[1] + b[3]) as i32 + 2, b[2] as i32, (size / 14.0).max(2.0) as i32, [255, 210, 60, 255]);
                }
            }
        }
        // The reading marker: a triangle pointing at the line.
        let ty = focus_y as i32;
        let m = ((size * 0.32) as i32).max(4);
        let x0 = (margin * 0.3) as i32;
        for dy in -m..=m {
            f.fill_rect(x0, ty + dy, (m - dy.abs()).max(1), 1, [255, 210, 60, 255]);
        }
        if self.view.mirror { f.mirrored() } else { f }
    }

    /// The script as one string (what is on the screen).
    pub fn script_text(&self) -> String {
        self.script.text()
    }
}

fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la.min(lb) < 4 {
        return false;
    }
    // A shared stem of four or more letters (plural, tense), or a small edit.
    let stem = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    if stem >= 4 && stem as f64 >= la.min(lb) as f64 * 0.7 {
        return true;
    }
    let limit = if la.max(lb) >= 8 { 2 } else { 1 };
    edit_distance(a, b) <= limit
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEECH: &str = "Good evening everyone and welcome to the show.\nTonight we have a special guest who flew in from Mumbai this morning.\n(pause for applause)\nPlease give a warm welcome to Priya Sharma.";

    #[test]
    fn script_splits_speech_and_directions() {
        let s = Script::from_text(SPEECH);
        assert_eq!(s.spoken_len(), 8 + 13 + 8);
        assert_eq!(s.len(), 8 + 13 + 3 + 8);
        assert!((s.duration(150.0) - 29.0 / 150.0 * 60.0).abs() < 1e-9);
    }

    #[test]
    fn pace_moves_at_the_set_speed_skips_directions_and_breathes_between_paragraphs() {
        let mut p = Prompter::new(Script::from_text(SPEECH), Mode::Pace(120.0));
        p.start();
        // 2 words a second: after 4 s, 8 words → the first paragraph is done
        // and the half-second breath has begun.
        for _ in 0..405 {
            p.tick(0.01);
        }
        assert_eq!(p.word(), 8);
        for _ in 0..35 {
            p.tick(0.01);
        }
        assert!(p.word() <= 9, "held for the breath: {}", p.word());
        // Run on to the direction: it is skipped, never waited for.
        for _ in 0..(7 * 100) {
            p.tick(0.01);
        }
        let st = p.status();
        assert!(st.next_words.starts_with("Please") || st.word >= 8 + 13 + 3 - 1, "{st:?}");
        p.pause();
        let w = p.word();
        p.tick(5.0);
        assert_eq!(p.word(), w);
    }

    #[test]
    fn following_the_speaker_survives_slips_and_stalls_when_lost() {
        let mut p = Prompter::new(Script::from_text(SPEECH), Mode::Follow);
        // The transcription drops "everyone", mishears "welcome" and stumbles.
        p.hear(&["good", "evening", "and", "welcom", "to", "the", "show"]);
        assert_eq!(p.word(), 8, "{:?}", p.status());
        // The speaker skips ahead a sentence: a few matching words pull it along.
        p.hear(&["tonight", "we", "have", "a", "special", "guest"]);
        assert_eq!(p.word(), 8 + 6);
        // Off-script chatter: the prompter holds and says it is lost.
        p.hear(&["uh", "so", "anyway", "banana", "pyjamas", "purple", "monkey"]);
        assert_eq!(p.word(), 8 + 6);
        assert!(p.status().lost);
        // Back on the script: found again.
        p.hear(&["who", "flew", "in", "from", "mumbai"]);
        assert!(!p.status().lost);
        assert_eq!(p.word(), 8 + 11);
    }

    #[test]
    fn speakers_are_headings_and_not_waited_for() {
        let s = Script::from_lines(&[
            PromptLine { who: Some("anna".into()), text: "Hello there.".into(), action: false },
            PromptLine { who: Some("anna".into()), text: "How are you?".into(), action: false },
            PromptLine { who: Some("ravi".into()), text: "Fine, thanks.".into(), action: false },
        ]);
        assert_eq!(s.spoken_len(), 7);
        assert_eq!(s.len(), 7 + 2);
        let mut p = Prompter::new(s, Mode::Follow);
        p.hear(&["hello", "there", "how", "are", "you", "fine", "thanks"]);
        assert_eq!(p.word(), 9);
    }

    #[test]
    fn renders_and_mirrors() {
        let mut p = Prompter::new(Script::from_text(SPEECH), Mode::Pace(150.0));
        p.view.size = 40.0;
        let f = p.render(640, 360);
        let lit = f.rgba.chunks(4).filter(|px| px[0] > 100).count();
        assert!(lit > 500, "{lit}");
        p.view.mirror = true;
        let m = p.render(640, 360);
        assert_eq!(m, f.mirrored());
        // The reading line is the bright one: left half of the first frame has text.
        let left: usize = (0..360).map(|y| (0..320).filter(|&x| f.px(x, y)[0] > 100).count()).sum();
        let right: usize = (0..360).map(|y| (320..640).filter(|&x| f.px(x, y)[0] > 100).count()).sum();
        assert!(left > 100 && right > 100);
    }
}
