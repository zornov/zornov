// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scripts in from files and from the editor: plain text, Markdown,
//! Fountain, SRT and WebVTT subtitles (their times kept), and putting an
//! edited script back over the story it came from without losing the
//! shots, places and cast built on it.

use super::fountain::{self, ensure_cast, who_of};
use super::times;
use super::{Line, LineKind, Scene, Story};
use crate::time::{Micros, SECOND};

/// The kinds of script file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A Fountain screenplay.
    Fountain,
    /// Plain prose or `NAME: words`.
    Plain,
    /// Markdown (headings are scenes).
    Markdown,
    /// SubRip subtitles.
    Srt,
    /// WebVTT subtitles.
    Vtt,
}

impl Format {
    /// The format a file extension names.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_lowercase().trim_start_matches('.') {
            "fountain" | "spmd" => Format::Fountain,
            "txt" | "text" => Format::Plain,
            "md" | "markdown" | "mdown" => Format::Markdown,
            "srt" => Format::Srt,
            "vtt" => Format::Vtt,
            _ => return None,
        })
    }

    /// The extensions a script file can have, for file pickers.
    pub const EXTENSIONS: &'static [&'static str] = &["fountain", "txt", "text", "md", "markdown", "srt", "vtt", "docx"];
}

/// Which format `text` is, by its extension when it has one and then by
/// its look.
pub fn detect(extension: Option<&str>, text: &str) -> Format {
    let by_look = || {
        let t = text.trim_start_matches('\u{feff}').trim_start();
        if t.starts_with("WEBVTT") {
            Format::Vtt
        } else if looks_like_srt(t) {
            Format::Srt
        } else if fountain::looks_like_fountain(t) {
            Format::Fountain
        } else if looks_like_markdown(t) {
            Format::Markdown
        } else {
            Format::Plain
        }
    };
    match extension.and_then(Format::from_extension) {
        // A .txt that is really SRT is read as SRT.
        Some(Format::Plain) | None => by_look(),
        Some(f) => f,
    }
}

fn looks_like_srt(t: &str) -> bool {
    t.lines().take(12).any(|l| l.contains("-->") && l.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

fn looks_like_markdown(t: &str) -> bool {
    t.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("# ") || l.starts_with("## ") || l.starts_with("> ") || l.starts_with("**") || l.starts_with("- ") || l.starts_with("* ") || l.starts_with("```")
    })
}

/// Reads a script of any supported format. `extension` (without the dot)
/// settles the format when the text alone does not.
pub fn read_script(text: &str, extension: Option<&str>) -> Story {
    let text = text.trim_start_matches('\u{feff}');
    match detect(extension, text) {
        Format::Fountain => fountain::parse(text),
        Format::Plain => fountain::read(text),
        Format::Markdown => parse_markdown(text),
        Format::Srt => parse_subtitles(text),
        Format::Vtt => parse_subtitles(text),
    }
}

// ---------------------------------------------------------------------
// Markdown

/// Removes `**bold**`, `_italic_`, `` `code` ``, links and images from one
/// line of Markdown, keeping the words.
fn strip_inline(l: &str) -> String {
    let cs: Vec<char> = l.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        // Images vanish; links keep their text.
        if c == '!' && cs.get(i + 1) == Some(&'[') {
            if let Some(end) = link_end(&cs, i + 1) {
                i = end;
                continue;
            }
        }
        if c == '[' {
            if let Some(end) = link_end(&cs, i) {
                let close = cs[i..].iter().position(|x| *x == ']').map(|p| i + p).unwrap_or(i);
                out.extend(&cs[i + 1..close]);
                i = end;
                continue;
            }
        }
        match c {
            '*' | '`' => {}
            '~' if cs.get(i + 1) == Some(&'~') => {
                i += 1;
            }
            // Underscores inside words stay (snake_case); emphasis ones go.
            '_' => {
                let before = i.checked_sub(1).and_then(|k| cs.get(k)).is_some_and(|p| p.is_alphanumeric());
                let after = cs.get(i + 1).is_some_and(|n| n.is_alphanumeric());
                if before && after {
                    out.push(c);
                }
            }
            '\\' if cs.get(i + 1).is_some_and(|n| n.is_ascii_punctuation()) => {
                i += 1;
                out.push(cs[i]);
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// The index after `[text](url)` that starts at `i`, if it is one.
fn link_end(cs: &[char], i: usize) -> Option<usize> {
    let close = cs[i..].iter().position(|x| *x == ']')? + i;
    if cs.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = cs[close + 2..].iter().position(|x| *x == ')')? + close + 2;
    Some(end + 1)
}

/// Markdown as the plain text the reader understands: headings stay `#`,
/// list and quote marks go, emphasis goes, bullets become sentences.
pub fn markdown_to_plain(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    let mut in_comment = false;
    for raw in md.lines() {
        let l = raw.trim_end();
        let t = l.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_comment {
            in_comment = !t.contains("-->");
            continue;
        }
        if t.starts_with("<!--") {
            in_comment = !t.contains("-->");
            continue;
        }
        if in_code {
            out.push_str(t);
            out.push('\n');
            continue;
        }
        // Rules and table borders are paragraph breaks.
        if t.len() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_' | '=' | ' ' | '|' | ':')) && !t.starts_with("|") || t.starts_with("|--") || t.starts_with("| --") {
            out.push('\n');
            continue;
        }
        if let Some(h) = t.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim().trim_end_matches('#').trim();
            out.push_str(&format!("\n# {}\n", strip_inline(h)));
            continue;
        }
        let mut body = t;
        let mut bullet = false;
        while let Some(r) = body.strip_prefix('>') {
            body = r.trim_start();
        }
        for m in ["- ", "* ", "+ "] {
            if let Some(r) = body.strip_prefix(m) {
                body = r;
                bullet = true;
                break;
            }
        }
        if !bullet {
            let digits = body.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 && digits <= 3 && (body[digits..].starts_with(". ") || body[digits..].starts_with(") ")) {
                body = &body[digits + 2..];
                bullet = true;
            }
        }
        // `- [ ]` and `- [x]` task marks.
        let body = body.strip_prefix("[ ] ").or_else(|| body.strip_prefix("[x] ")).or_else(|| body.strip_prefix("[X] ")).unwrap_or(body);
        let mut line = strip_inline(body.trim());
        if line.starts_with('|') {
            line = line.trim_matches('|').split('|').map(str::trim).collect::<Vec<_>>().join(" ");
        }
        if bullet && !line.is_empty() && !line.ends_with(['.', '!', '?', ':', '"', '”']) {
            line.push('.');
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Reads Markdown: `#` headings are scenes (a lone first `#` over other
/// headings is the title), paragraphs are narration a sentence a line,
/// `**Name:** words` is dialogue, times typed in front keep their place.
pub fn parse_markdown(md: &str) -> Story {
    let plain = markdown_to_plain(md);
    let heads: Vec<&str> = plain.lines().filter(|l| l.starts_with("# ")).collect();
    let first_is_title = heads.len() >= 2 && plain.trim_start().starts_with("# ") && md.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| l.starts_with("# ") && !l.starts_with("##")) && md.lines().filter(|l| l.starts_with("# ")).count() == 1;
    let (title, body) = if first_is_title {
        let t = plain.trim_start();
        let (first, rest) = t.split_once('\n').unwrap_or((t, ""));
        (Some(first.trim_start_matches('#').trim().to_owned()), rest.to_owned())
    } else {
        (None, plain.clone())
    };
    let mut st = if fountain::looks_like_fountain(&body) { fountain::parse(&body) } else { fountain::parse_plain(&body) };
    if let Some(t) = title {
        st.title = t;
    }
    st
}

// ---------------------------------------------------------------------
// Subtitles

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '<' | '{' => depth += 1,
            '>' | '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line of script from typed text: a time in front (`[0:12-0:18]`) is
/// kept, `NAME: words` is that person speaking, anything else is a line of
/// `default` kind (narration by the narrator, or action).
pub fn line_from_text(text: &str, default: LineKind) -> Line {
    let (tm, rest) = times::take_timing(text.trim());
    let (kind, who, words) = match speaker_of(rest) {
        Some((n, w)) if default != LineKind::Action => {
            let id = fountain::slug(n);
            if id == "narrator" { (LineKind::Narration, Some(id), w.to_owned()) } else { (LineKind::Dialogue, Some(id), w.trim_matches('"').to_owned()) }
        }
        _ => match default {
            LineKind::Action => (LineKind::Action, None, rest.trim().to_owned()),
            _ => (LineKind::Narration, Some("narrator".to_owned()), rest.trim().to_owned()),
        },
    };
    Line { kind, who, text: words, start: tm.start, duration: tm.duration, ..Default::default() }
}

/// `NAME: words` with a short capitalised name.
fn speaker_of(text: &str) -> Option<(&str, &str)> {
    let (n, w) = text.split_once(':')?;
    let n = n.trim();
    let ok = !n.is_empty() && n.split_whitespace().count() <= 3 && n.chars().next().is_some_and(char::is_uppercase) && !n.chars().any(|c| c.is_ascii_digit()) && !w.trim().is_empty() && n.len() <= 30;
    ok.then(|| (n, w.trim()))
}

/// Reads SubRip or WebVTT subtitles: each cue is a line with its start and
/// length kept; a speaker (`NAME: words`, WebVTT `<v Name>`) makes it
/// dialogue, anything else is the narrator. A gap of six seconds or more
/// starts a new scene.
pub fn parse_subtitles(text: &str) -> Story {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut st = Story::default();
    let mut scene = Scene { id: "1".into(), ..Default::default() };
    let mut last_end: Option<Micros> = None;
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let (a, rest) = lines[ti].split_once("-->").unwrap();
        let b = rest.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (times::parse_time(a.trim()), times::parse_time(b)) else { continue };
        let raw: Vec<&str> = lines[ti + 1..].to_vec();
        if raw.is_empty() {
            continue;
        }
        // WebVTT voice tags name the speaker.
        let mut voice: Option<String> = None;
        let joined = raw.join(" ");
        if let Some(i) = joined.find("<v") {
            if let Some(j) = joined[i..].find('>') {
                let name = joined[i + 2..i + j].trim().trim_start_matches('.');
                let name = name.split_once(' ').map(|x| x.1).unwrap_or(name).trim();
                if !name.is_empty() {
                    voice = Some(name.to_owned());
                }
            }
        }
        let mut words = strip_tags(&joined);
        // A dash starts each speaker's turn in a two-person cue; keep it as one line.
        words = words.trim_start_matches(['-', '–', '—']).trim().to_owned();
        let words = words.replace(" - ", " ");
        let (who, words) = match (voice, speaker_of(&words)) {
            (Some(v), _) => (Some(v), words),
            (None, Some((n, w))) => (Some(n.to_owned()), w.to_owned()),
            (None, None) => (None, words),
        };
        if words.is_empty() {
            continue;
        }
        if let Some(prev) = last_end {
            if start >= prev + 6 * SECOND && !scene.lines.is_empty() {
                let n = st.scenes.len() + 2;
                st.scenes.push(std::mem::replace(&mut scene, Scene { id: n.to_string(), ..Default::default() }));
            }
        }
        last_end = Some(end);
        let line = match who {
            Some(w) => {
                let id = who_of(&st, &w);
                if id == "narrator" {
                    Line { kind: LineKind::Narration, who: Some(id), text: words, ..Default::default() }
                } else {
                    ensure_cast(&mut st, Some(&mut scene), &id, &w);
                    Line { kind: LineKind::Dialogue, who: Some(id), text: words, ..Default::default() }
                }
            }
            None => Line { kind: LineKind::Narration, who: Some("narrator".into()), text: words, ..Default::default() },
        };
        scene.lines.push(Line { start: Some(start), duration: (end > start).then_some(end - start), ..line });
    }
    if !scene.lines.is_empty() {
        st.scenes.push(scene);
    }
    st
}

/// The story's lines as SubRip subtitles, each at the time it plays
/// ([`Story::line_times`]); action lines are left out (they are not said).
pub fn write_srt(st: &Story) -> String {
    let clock = |t: Micros| {
        let ms = (t.max(0) as f64 / 1000.0).round() as i64;
        format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    };
    let mut out = String::new();
    let mut n = 0;
    for t in st.line_times() {
        let l = &st.scenes[t.scene].lines[t.line];
        if !l.is_spoken() {
            continue;
        }
        n += 1;
        let speaker = match (l.kind, l.who.as_deref()) {
            (LineKind::Dialogue, Some(w)) => format!("{}: ", title_name(st, w)),
            _ => String::new(),
        };
        out.push_str(&format!("{n}\n{} --> {}\n{speaker}{}\n\n", clock(t.start), clock(t.end), l.text));
    }
    out
}

fn title_name(st: &Story, who: &str) -> String {
    st.character(who).map(|c| if c.name.is_empty() { c.id.replace('_', " ") } else { c.name.clone() }).unwrap_or_else(|| who.replace('_', " "))
}

// ---------------------------------------------------------------------
// Putting an edited script back

/// Puts `new` (a script read from the editor's text or a file) over `old`:
/// scenes with the same id keep their shots (lines the shots refer to that
/// still exist stay, the rest are dropped), places and the character bible
/// are kept and grow with any new speakers or settings. Scenes that are not
/// in `new` are gone.
pub fn merge_script(old: &Story, new: Story) -> Story {
    let mut out = Story { title: if new.title.is_empty() { old.title.clone() } else { new.title.clone() }, logline: if new.logline.is_empty() { old.logline.clone() } else { new.logline.clone() }, ..new.clone() };
    // Places and cast: the old description wins, new names are added.
    out.places = old.places.clone();
    for p in &new.places {
        if !out.places.iter().any(|x| x.id == p.id) {
            out.places.push(p.clone());
        }
    }
    out.cast = old.cast.clone();
    for c in &new.cast {
        if !out.cast.iter().any(|x| x.id == c.id) {
            out.cast.push(c.clone());
        }
    }
    out.beats = old.beats.clone();
    for b in &new.beats {
        match out.beats.iter_mut().find(|x| x.id == b.id) {
            Some(x) => {
                if !b.summary.is_empty() {
                    x.summary = b.summary.clone();
                }
                if !b.purpose.is_empty() {
                    x.purpose = b.purpose.clone();
                }
                x.start = b.start;
                x.duration = b.duration;
            }
            None => out.beats.push(b.clone()),
        }
    }
    out.extra = old.extra.clone();
    for sc in &mut out.scenes {
        let Some(o) = old.scenes.iter().find(|x| x.id.eq_ignore_ascii_case(&sc.id)) else { continue };
        sc.shots = o.shots.clone();
        for sh in &mut sc.shots {
            sh.lines.retain(|i| *i < sc.lines.len());
        }
        // What the text does not say, the scene keeps.
        if sc.place.is_none() {
            sc.place = o.place.clone();
        }
        if sc.beat.is_none() {
            sc.beat = o.beat.clone();
        }
        if sc.time_of_day.is_none() {
            sc.time_of_day = o.time_of_day;
        }
        if sc.summary.is_empty() {
            sc.summary = o.summary.clone();
        }
        for c in &o.cast {
            if !sc.cast.contains(c) {
                sc.cast.push(c.clone());
            }
        }
        sc.extra = o.extra.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::ShotKind;

    const S: Micros = SECOND;

    #[test]
    fn formats_are_told_apart() {
        assert_eq!(detect(Some("srt"), "x"), Format::Srt);
        assert_eq!(detect(Some("txt"), "1\n00:00:01,000 --> 00:00:02,000\nHi\n"), Format::Srt);
        assert_eq!(detect(None, "WEBVTT\n\n00:01.000 --> 00:02.000\nHi\n"), Format::Vtt);
        assert_eq!(detect(None, "EXT. HILL - DAY\n\nWind.\n"), Format::Fountain);
        assert_eq!(detect(None, "# One\n\nWords.\n"), Format::Markdown);
        assert_eq!(detect(Some("txt"), "Just some words.\n"), Format::Plain);
        assert_eq!(detect(Some("fountain"), "Just some words.\n"), Format::Fountain);
        assert_eq!(Format::from_extension(".MD"), Some(Format::Markdown));
        assert_eq!(Format::from_extension("docx"), None);
    }

    #[test]
    fn srt_keeps_its_times_and_speakers() {
        let srt = "\u{feff}1\n00:00:01,000 --> 00:00:03,500\n<i>The fox woke early.</i>\n\n2\n00:00:04,000 --> 00:00:06,000\nFERN: Hello?\n\n3\n00:00:07,000 --> 00:00:08,000\n- Anyone there?\n- Only me.\n\n4\n00:00:30,000 --> 00:00:32,000\nMuch later.\n";
        let st = read_script(srt, Some("srt"));
        assert_eq!(st.scenes.len(), 2, "a long gap starts a scene");
        let l = &st.scenes[0].lines;
        assert_eq!(l.len(), 3);
        assert_eq!((l[0].start, l[0].duration, l[0].kind), (Some(S), Some(2_500_000), LineKind::Narration));
        assert_eq!(l[0].text, "The fox woke early.");
        assert_eq!((l[1].who.as_deref(), l[1].kind, l[1].text.as_str()), (Some("fern"), LineKind::Dialogue, "Hello?"));
        assert_eq!(l[2].text, "Anyone there? Only me.");
        assert_eq!(st.scenes[1].lines[0].start, Some(30 * S));
        assert_eq!(st.character("fern").unwrap().name, "Fern");
        // Written back out, the times are the ones that were read.
        let back = parse_subtitles(&write_srt(&st));
        let times: Vec<_> = back.scenes.iter().flat_map(|s| s.lines.iter().map(|l| (l.start, l.duration))).collect();
        let want: Vec<_> = st.scenes.iter().flat_map(|s| s.lines.iter().map(|l| (l.start, l.duration))).collect();
        assert_eq!(times, want);
    }

    #[test]
    fn vtt_skips_its_header_notes_and_reads_voices() {
        let vtt = "WEBVTT - the film\n\nNOTE this is ignored\n\nSTYLE\n::cue { color: red }\n\nintro\n00:01.000 --> 00:03.000 align:start\n<v Owl>Who goes there?</v>\n\n01:00:00.500 --> 01:00:02.000\nAnd that was that.\n";
        let st = read_script(vtt, Some("vtt"));
        let lines: Vec<&Line> = st.scenes.iter().flat_map(|s| s.lines.iter()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].who.as_deref(), lines[0].start, lines[0].duration), (Some("owl"), Some(S), Some(2 * S)));
        assert_eq!(lines[0].text, "Who goes there?");
        assert_eq!(lines[1].start, Some(3_600_500_000));
    }

    #[test]
    fn markdown_becomes_scenes_lines_and_speakers() {
        let md = "# The Fox and the River\n\n## The morning\n\n**[0:00-0:06]** The *fox* woke early, and the [river](http://x.y) was loud.\n\n**Fern:** Hello? Anyone there?\n\n- She looked left\n- She looked right\n\n```\ncode fence\n```\n\n---\n\n## The crossing\n\n> The water was cold.\n\n<!-- a comment\nover lines -->\nIt rose.\n";
        let st = read_script(md, Some("md"));
        assert_eq!(st.title, "The Fox and the River");
        assert_eq!(st.scenes.len(), 2, "{:#?}", st.scenes.iter().map(|s| (&s.id, &s.summary, s.lines.len())).collect::<Vec<_>>());
        assert_eq!(st.scenes[0].summary, "The morning");
        let l = &st.scenes[0].lines;
        assert_eq!((l[0].start, l[0].duration), (Some(0), Some(6 * S)));
        assert_eq!(l[0].text, "The fox woke early, and the river was loud.");
        assert_eq!((l[1].kind, l[1].who.as_deref(), l[1].text.as_str()), (LineKind::Dialogue, Some("fern"), "Hello? Anyone there?"));
        assert!(l.iter().any(|x| x.text == "She looked left."));
        assert!(!l.iter().any(|x| x.text.contains("comment")));
        assert_eq!(st.scenes[1].lines.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["The water was cold.", "It rose."]);
    }

    #[test]
    fn a_script_in_plain_text_with_times_and_speakers() {
        let text = "[0:00-0:04] Once there was a fox.\n[0:04] Fern: Is anyone there?\n[+3s] The heron answered.\n00:12 - Only me.\n\n0:20\nThe end.\n";
        let st = read_script(text, Some("txt"));
        let l: Vec<&Line> = st.scenes.iter().flat_map(|s| s.lines.iter()).collect();
        assert_eq!(l.len(), 5, "{l:#?}");
        assert_eq!((l[0].start, l[0].duration), (Some(0), Some(4 * S)));
        assert_eq!((l[1].who.as_deref(), l[1].start, l[1].text.as_str()), (Some("fern"), Some(4 * S), "Is anyone there?"));
        assert_eq!((l[2].start, l[2].duration, l[2].text.as_str()), (None, Some(3 * S), "The heron answered."));
        assert_eq!((l[3].start, l[3].text.as_str()), (Some(12 * S), "Only me."));
        assert_eq!((l[4].start, l[4].text.as_str()), (Some(20 * S), "The end."));
    }

    #[test]
    fn fountain_with_times_notes_and_scene_starts() {
        let text = "Title: T\n\n[0:00] EXT. HILL - DAY #1#\n\n[[note: shoot at dawn]]\n\n[0:02-0:05] The wind blows.\n\n[0:06] FERN\n(whispering)\nHello.\n\n[[note: softly]]\n\nNARRATOR (V.O.)\n[0:09] It was quiet.\n\n[0:12]\nAfter a while, nothing.\n";
        let st = fountain::read(text);
        let s = &st.scenes[0];
        assert_eq!((s.start, s.notes.as_str()), (Some(0), "shoot at dawn"));
        assert_eq!(s.lines.len(), 4, "{:#?}", s.lines);
        assert_eq!((s.lines[0].start, s.lines[0].duration, s.lines[0].kind), (Some(2 * S), Some(3 * S), LineKind::Action));
        assert_eq!((s.lines[1].start, s.lines[1].who.as_deref(), s.lines[1].how.as_deref(), s.lines[1].note.as_str()), (Some(6 * S), Some("fern"), Some("whispering"), "softly"));
        assert_eq!((s.lines[2].start, s.lines[2].text.as_str()), (Some(9 * S), "It was quiet."));
        assert_eq!((s.lines[3].start, s.lines[3].text.as_str()), (Some(12 * S), "After a while, nothing."));
    }

    #[test]
    fn times_and_notes_survive_writing_and_reading() {
        let mut st = fountain::read("EXT. HILL - DAY #1#\n\nThe wind blows.\n\nFERN\nHello there.\n\nNARRATOR (V.O.)\nIt was quiet.\n");
        st.scenes[0].start = Some(S);
        st.scenes[0].notes = "dawn".into();
        st.scenes[0].lines[0].start = Some(2 * S);
        st.scenes[0].lines[0].duration = Some(3 * S);
        st.scenes[0].lines[1].start = Some(7_250_000);
        st.scenes[0].lines[1].note = "soft".into();
        st.scenes[0].lines[2].duration = Some(4 * S);
        // A line that would read as a cue or a time if left bare.
        st.scenes[0].lines.push(Line { text: "FERN SPEAKS".into(), start: Some(20 * S), ..Default::default() });
        st.scenes[0].lines.push(Line { text: "0:30 was the hour".into(), ..Default::default() });
        for text in [fountain::write(&st), fountain::write_script(&st)] {
            let back = fountain::read(&text);
            assert_eq!(back.scenes[0].lines, st.scenes[0].lines, "{text}");
            assert_eq!((back.scenes[0].start, &back.scenes[0].notes), (st.scenes[0].start, &st.scenes[0].notes));
        }
        // Plain text keeps times too.
        let plain = fountain::write_plain(&st);
        let back = fountain::parse_plain(&plain);
        let starts: Vec<_> = back.scenes.iter().flat_map(|s| s.lines.iter().map(|l| (l.start, l.duration))).collect();
        assert!(starts.contains(&(Some(2 * S), Some(3 * S))) && starts.contains(&(Some(7_250_000), None)), "{plain}\n{starts:?}");
    }

    #[test]
    fn an_edited_script_goes_back_over_the_story_keeping_its_shots() {
        let mut st = fountain::read("EXT. HILL - DAY #1#\n\nNARRATOR (V.O.)\nOne.\n\nNARRATOR (V.O.)\nTwo.\n\nEXT. RIVER - DAY #2#\n\nNARRATOR (V.O.)\nThree.\n");
        st.plan(false);
        st.cast.push(crate::story::Character { id: "fern".into(), look: "red fox".into(), size: 1.0, ..Default::default() });
        st.places[0].backdrop = Some("hill.png".into());
        let shots1 = st.scenes[0].shots.len();
        // The person edits the text: a line changes, a speaker is added, scene 2 goes.
        let mut text = fountain::write_script(&st);
        text = text.replace("Two.", "Two, changed.");
        text.push_str("\nEXT. CAVE - NIGHT #3#\n\nFERN\nBrr.\n");
        let cut = text.find("EXT. RIVER").unwrap();
        let end = text.find("EXT. CAVE").unwrap();
        text.replace_range(cut..end, "");
        let new = merge_script(&st, fountain::read(&text));
        assert_eq!(new.scenes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["1", "3"]);
        assert_eq!(new.scenes[0].lines[1].text, "Two, changed.");
        assert_eq!(new.scenes[0].shots.len(), shots1, "shots kept");
        assert_eq!(new.cast.iter().find(|c| c.id == "fern").unwrap().look, "red fox", "the bible is kept");
        assert_eq!(new.places.iter().find(|p| p.id == "hill").unwrap().backdrop.as_deref(), Some("hill.png"));
        assert!(new.place("cave").is_some());
        assert!(new.scenes[0].shots.iter().all(|sh| sh.lines.iter().all(|i| *i < 2)));
        assert!(new.scenes[0].shots.iter().any(|sh| sh.kind == ShotKind::Establishing));
    }
}
