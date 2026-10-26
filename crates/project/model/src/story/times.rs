// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time in the script: typed timecodes (`00:12`, `[0:12–0:18]`, `[+4s]`),
//! how they are written back, and when every line plays.
//!
//! A line can carry a start and a duration ([`Line::start`],
//! [`Line::duration`]). Where the writer gave none, a line follows its
//! shot (so it moves when the clips move) and, with no shot, is estimated
//! from reading speed. [`Story::line_times`] gives all three kinds on one
//! scale and [`Story::retime_shots`] lays planned shots so their cuts land
//! on the times the writer gave.

use super::{Line, LineKind, Story};
use crate::time::{Micros, SECOND};

/// Reading speed for spoken lines, words a second.
pub const SPOKEN_WPS: f64 = 2.6;
/// Reading speed for action lines (read, not said), words a second.
pub const ACTION_WPS: f64 = 3.0;

/// A start and/or a duration read from text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    /// Start from the beginning of the film.
    pub start: Option<Micros>,
    /// How long it runs.
    pub duration: Option<Micros>,
}

impl Timing {
    /// Neither a start nor a duration.
    pub fn is_none(&self) -> bool {
        self.start.is_none() && self.duration.is_none()
    }
}

fn us(s: f64) -> Micros {
    (s * SECOND as f64).round() as Micros
}

/// A duration: `4s`, `4.5s`, `1m30s`, `1m 30s`, `2m`, `1h`, `250ms`.
pub fn parse_duration(s: &str) -> Option<Micros> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut rest = s.as_str();
    let mut any = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        let n_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        if n_end == 0 {
            return None;
        }
        let n: f64 = rest[..n_end].parse().ok()?;
        rest = rest[n_end..].trim_start();
        let u_end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let secs = match &rest[..u_end] {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "ms" => 0.001,
            _ => return None,
        };
        total += n * secs;
        any = true;
        rest = &rest[u_end..];
    }
    any.then(|| us(total))
}

/// A point in time: `0:12`, `00:12`, `1:02:03`, `00:00:12,500`,
/// `0:12.5`, or a duration-like `12s` / `1m30s`.
pub fn parse_time(s: &str) -> Option<Micros> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if !s.contains(':') {
        return parse_duration(s);
    }
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let (last, rest) = parts.split_last()?;
    if last.is_empty() || last.matches('.').count() > 1 || !last.chars().all(|c| c.is_ascii_digit() || c == '.') || last.starts_with('.') {
        return None;
    }
    let sec: f64 = last.parse().ok()?;
    let mut nums = Vec::new();
    for p in rest {
        if p.is_empty() || p.len() > 5 || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        nums.push(p.parse::<i64>().ok()?);
    }
    let (h, m) = if nums.len() == 2 { (nums[0], nums[1]) } else { (0, nums[0]) };
    if sec >= 60.0 || (nums.len() == 2 && m >= 60) {
        return None;
    }
    Some(us((h * 3600 + m * 60) as f64 + sec))
}

/// Writes a point in time: `0:12`, `0:12.5`, `1:02:03`.
pub fn format_time(t: Micros) -> String {
    let ms = (t.max(0) as f64 / 1000.0).round() as i64;
    let (h, m, s, f) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    let mut out = if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{}:{s:02}", m) };
    if f != 0 {
        out.push_str(format!(".{f:03}").trim_end_matches('0'));
    }
    out
}

/// Writes a length: `4s`, `4.5s`, `1m30s`.
pub fn format_duration(d: Micros) -> String {
    let ms = (d.max(0) as f64 / 1000.0).round() as i64;
    let (m, s, f) = (ms / 60_000, ms / 1000 % 60, ms % 1000);
    let secs = if f == 0 { format!("{s}") } else { format!("{s}.{f:03}").trim_end_matches('0').to_owned() };
    if m > 0 {
        if s == 0 && f == 0 { format!("{m}m") } else { format!("{m}m{secs}s") }
    } else {
        format!("{secs}s")
    }
}

/// Writes a timing the way it is typed: `[0:12–0:18]`, `[0:12]`, `[+4s]`,
/// or nothing for none.
pub fn format_timing(t: Timing) -> String {
    match (t.start, t.duration) {
        (Some(a), Some(d)) if d > 0 => format!("[{}-{}]", format_time(a), format_time(a + d)),
        (Some(a), _) => format!("[{}]", format_time(a)),
        (None, Some(d)) => format!("[+{}]", format_duration(d)),
        (None, None) => String::new(),
    }
}

const RANGE_SEPS: [&str; 5] = ["-->", "->", "–", "—", " to "];

/// What sits inside brackets: a point, a range, a duration, or a point
/// and a duration.
fn parse_span(inner: &str) -> Option<Timing> {
    let inner = inner.trim().trim_start_matches('@').trim();
    if inner.is_empty() {
        return None;
    }
    let range = |a: &str, b: &str| -> Option<Timing> {
        let (a, b) = (parse_time(a)?, parse_time(b)?);
        Some(Timing { start: Some(a), duration: (b > a).then_some(b - a) })
    };
    for sep in RANGE_SEPS.iter().copied().chain([" - ", "-"]) {
        if let Some((a, b)) = inner.split_once(sep) {
            if let Some(t) = range(a, b) {
                return Some(t);
            }
        }
    }
    if let Some(d) = inner.strip_prefix('+') {
        return parse_duration(d).map(|d| Timing { start: None, duration: Some(d) });
    }
    if inner.contains(':') {
        let mut it = inner.split_whitespace();
        let start = parse_time(it.next()?)?;
        let duration = match it.next() {
            Some(x) => Some(parse_duration(x.trim_start_matches('+').trim_matches(|c| c == '(' || c == ')'))?),
            None => None,
        };
        return Some(Timing { start: Some(start), duration });
    }
    parse_duration(inner).map(|d| Timing { start: None, duration: Some(d) })
}

fn skip_seps(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | '—' | ':' | '|'))
}

/// A bare timecode token at the start of `s` (digits, colons, dots).
fn bare_token(s: &str) -> Option<(Micros, &str)> {
    let end = s.find(|c: char| !(c.is_ascii_digit() || matches!(c, ':' | '.' | ','))).unwrap_or(s.len());
    let tok = s[..end].trim_end_matches(['.', ',', ':']);
    if !tok.contains(':') {
        return None;
    }
    let t = parse_time(tok)?;
    let rest = &s[tok.len()..];
    // The token must stand alone: space, dash, colon, bar, or the end.
    match rest.chars().next() {
        None => Some((t, rest)),
        Some(c) if c.is_whitespace() || matches!(c, '-' | '–' | '—' | ':' | '|' | '.' | ',') => Some((t, rest)),
        _ => None,
    }
}

/// Reads a timing typed at the start of `text` and returns it with what
/// follows: `[0:12–0:18] Words`, `(0:12) Words`, `{+4s} Words`,
/// `0:12 Words`, `00:12 - Words`, `0:12-0:18: Words`.
pub fn take_timing(text: &str) -> (Timing, &str) {
    let t = text.trim_start();
    let none = (Timing::default(), text);
    let Some(open) = t.chars().next() else { return none };
    let close = match open {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        _ => '\0',
    };
    if close != '\0' {
        // `[[note]]` is a Fountain note, not a time.
        if t.starts_with("[[") {
            return none;
        }
        if let Some(j) = t.find(close) {
            let inner = &t[1..j];
            // Round brackets are also how scripts write asides: "(10 seconds)"
            // is words, "(0:12)" and "(4s)" are times.
            let compact = !inner.contains(char::is_whitespace);
            if open == '(' && !(inner.contains(':') || inner.starts_with('+') || compact) {
                return none;
            }
            if let Some(tm) = parse_span(inner) {
                return (tm, skip_seps(&t[j + 1..]));
            }
        }
        return none;
    }
    if !t.starts_with(|c: char| c.is_ascii_digit() || c == '@') {
        return none;
    }
    let t = t.trim_start_matches('@');
    let Some((a, rest)) = bare_token(t) else { return none };
    // A range: `0:12-0:18`, `0:12 – 0:18`, `0:12 --> 0:18`.
    let after = rest.trim_start();
    for sep in ["-->", "->", "–", "—", "-"] {
        if let Some(r) = after.strip_prefix(sep) {
            if let Some((b, rest2)) = bare_token(r.trim_start()) {
                return (Timing { start: Some(a), duration: (b > a).then_some(b - a) }, skip_seps(rest2));
            }
        }
    }
    (Timing { start: Some(a), duration: None }, skip_seps(rest))
}

impl Line {
    /// Seconds the line needs by reading speed, or its own length when it
    /// was given one: spoken lines at an easy pace, action lines a little
    /// faster (read, not said).
    pub fn secs(&self) -> f64 {
        if let Some(d) = self.duration {
            return d as f64 / SECOND as f64;
        }
        let words = self.text.split_whitespace().count() as f64;
        match self.kind {
            LineKind::Action => (words / ACTION_WPS + 0.5).max(1.0),
            _ => words / SPOKEN_WPS + 0.3,
        }
    }

    /// The line's start and length as typed, for writing back.
    pub fn timing(&self) -> Timing {
        Timing { start: self.start, duration: self.duration }
    }
}

/// Where a line's time comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeSource {
    /// The writer gave the start (typed, imported or set).
    Said,
    /// Follows its shot: moves when the shot (its clips) moves.
    Shot,
    /// Estimated from reading speed after the line before.
    Estimated,
}

/// When a line plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineTime {
    /// Index into [`Story::scenes`].
    pub scene: usize,
    /// Index into the scene's lines.
    pub line: usize,
    /// When it starts.
    pub start: Micros,
    /// When it ends.
    pub end: Micros,
    /// Where the start comes from.
    pub source: TimeSource,
}

impl Story {
    /// When every line plays: its own time when it has one, else its
    /// place in its shot (built shots read back from the timeline, planned
    /// ones as laid out), else after the line before at reading speed.
    pub fn line_times(&self) -> Vec<LineTime> {
        let shots = self.timings();
        let mut out = Vec::new();
        let mut cursor: Micros = 0;
        for (si, sc) in self.scenes.iter().enumerate() {
            let own: Vec<&super::ShotTime> = shots.iter().filter(|t| t.scene == si).collect();
            if let Some(a) = sc.start {
                cursor = a;
            } else if let Some(first) = own.first() {
                cursor = first.start;
            }
            for (li, l) in sc.lines.iter().enumerate() {
                let in_shot = sc.shots.iter().position(|s| s.lines.contains(&li));
                let (start, dur, source) = if let Some(a) = l.start {
                    (a, us(l.secs()), TimeSource::Said)
                } else if let Some(k) = in_shot {
                    let tm = own.iter().find(|t| t.shot == k);
                    match tm {
                        Some(tm) => {
                            let sh = &sc.shots[k];
                            let mates: Vec<&Line> = sh.lines.iter().filter_map(|i| sc.lines.get(*i)).collect();
                            let total: f64 = mates.iter().map(|m| m.secs()).sum();
                            let room = tm.duration as f64 / SECOND as f64;
                            let scale = if total > room && total > 0.0 { room / total } else { 1.0 };
                            let before: f64 = sh.lines.iter().take_while(|i| **i != li).filter_map(|i| sc.lines.get(*i)).map(|m| m.secs()).sum();
                            let start = tm.start + us(before * scale);
                            let dur = us(l.secs() * scale).min(tm.start + tm.duration - start).max(0);
                            (start, if l.duration.is_some() { us(l.secs()) } else { dur }, TimeSource::Shot)
                        }
                        None => (cursor, us(l.secs()), TimeSource::Estimated),
                    }
                } else {
                    (cursor, us(l.secs()), TimeSource::Estimated)
                };
                out.push(LineTime { scene: si, line: li, start, end: start + dur, source });
                cursor = if source == TimeSource::Shot { cursor.max(start + dur) } else { start + dur };
            }
            if let Some(last) = own.last() {
                cursor = cursor.max(last.start + last.duration);
            }
        }
        out
    }

    /// When line `line` of scene `scene` plays.
    pub fn line_time(&self, scene: usize, line: usize) -> Option<LineTime> {
        self.line_times().into_iter().find(|t| t.scene == scene && t.line == line)
    }

    /// Where the film runs to by its script: the last line's end or the
    /// last shot's end, whichever is later.
    pub fn script_end(&self) -> Micros {
        self.line_times().iter().map(|t| t.end).max().unwrap_or(0).max(self.duration())
    }

    /// Sets the start and length of one line (`None` clears to follow the
    /// shot or reading speed).
    pub fn set_line_timing(&mut self, scene: usize, line: usize, start: Option<Micros>, duration: Option<Micros>) -> bool {
        match self.scenes.get_mut(scene).and_then(|s| s.lines.get_mut(line)) {
            Some(l) => {
                l.start = start.map(|a| a.max(0));
                l.duration = duration.filter(|d| *d > 0);
                true
            }
            None => false,
        }
    }

    /// Lays planned (not yet built) shots so each cut falls where the
    /// writer's times say: a shot whose lines carry a start begins there,
    /// the shot before it runs up to it, the first shot reaches back to
    /// the start of the film and the last runs to the end of its lines.
    /// Returns how many shots changed length.
    pub fn retime_shots(&mut self) -> usize {
        let order: Vec<(usize, usize)> = self.scenes.iter().enumerate().flat_map(|(si, s)| (0..s.shots.len()).map(move |k| (si, k))).collect();
        if order.is_empty() || !self.scenes.iter().any(|s| s.lines.iter().any(|l| l.start.is_some())) {
            return 0;
        }
        // The start each shot wants, from its first line that has a time.
        let anchors: Vec<Option<Micros>> = order
            .iter()
            .map(|&(si, k)| {
                let sc = &self.scenes[si];
                let sh = &sc.shots[k];
                if !sh.clips.is_empty() {
                    return None;
                }
                let mut before = 0.0;
                for i in &sh.lines {
                    let l = sc.lines.get(*i)?;
                    if let Some(a) = l.start {
                        return Some((a - us(before)).max(0));
                    }
                    before += l.secs();
                }
                None
            })
            .collect();
        let min_len = us(0.3);
        let mut changed = 0;
        let mut t: Micros = 0;
        let mut prev: Option<(usize, usize, Micros)> = None; // shot index in order, its start
        for (n, &(si, k)) in order.iter().enumerate() {
            let built = !self.scenes[si].shots[k].clips.is_empty();
            if built {
                let sh = &self.scenes[si].shots[k];
                t = sh.start.unwrap_or(t) + sh.duration;
                prev = None;
                continue;
            }
            let mut start = t;
            if let Some(a) = anchors[n] {
                match prev {
                    Some((ps, pk, pstart)) if a - pstart >= min_len => {
                        let want = a - pstart;
                        if self.scenes[ps].shots[pk].duration != want {
                            self.scenes[ps].shots[pk].duration = want;
                            changed += 1;
                        }
                        start = a;
                    }
                    None if n == 0 && a > min_len => {
                        // The film opens before the first line: the first shot holds.
                        self.scenes[si].shots[k].duration += a;
                        changed += 1;
                    }
                    _ => {}
                }
            }
            // The last shot of the film runs to the end of its lines.
            if n + 1 == order.len() {
                let sc = &self.scenes[si];
                let sh = &sc.shots[k];
                let end = sh.lines.iter().filter_map(|i| sc.lines.get(*i)).filter_map(|l| l.start.map(|a| a + us(l.secs()))).max();
                if let Some(end) = end {
                    if end > start && end - start != sh.duration && end - start >= min_len {
                        self.scenes[si].shots[k].duration = end - start;
                        changed += 1;
                    }
                }
            }
            let sh = &self.scenes[si].shots[k];
            t = start + sh.duration;
            prev = Some((si, k, start));
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{fountain, Scene, Shot, ShotKind};

    const S: Micros = SECOND;

    #[test]
    fn timecodes_are_read_in_every_usual_form() {
        assert_eq!(parse_time("00:12"), Some(12 * S));
        assert_eq!(parse_time("0:12"), Some(12 * S));
        assert_eq!(parse_time("1:02:03"), Some(3723 * S));
        assert_eq!(parse_time("00:00:12,500"), Some(12_500_000));
        assert_eq!(parse_time("0:12.25"), Some(12_250_000));
        assert_eq!(parse_time("75:00"), Some(4500 * S));
        assert_eq!(parse_time("12s"), Some(12 * S));
        assert_eq!(parse_time("1m30s"), Some(90 * S));
        assert_eq!(parse_time("0:75"), None);
        assert_eq!(parse_time("12"), None);
        assert_eq!(parse_time("a:12"), None);
        assert_eq!(parse_duration("250ms"), Some(250_000));
        assert_eq!(parse_duration("1m 30s"), Some(90 * S));
        assert_eq!(parse_duration("fast"), None);
    }

    #[test]
    fn times_are_written_the_way_they_are_typed() {
        for t in [0, 12 * S, 12_500_000, 3723 * S, 59_999_000, 600 * S] {
            assert_eq!(parse_time(&format_time(t)), Some(t), "{}", format_time(t));
        }
        assert_eq!(format_time(12 * S), "0:12");
        assert_eq!(format_time(72_500_000), "1:12.5");
        assert_eq!(format_time(3723 * S), "1:02:03");
        assert_eq!(format_duration(90 * S), "1m30s");
        assert_eq!(format_duration(4_500_000), "4.5s");
        assert_eq!(format_duration(120 * S), "2m");
        let t = Timing { start: Some(12 * S), duration: Some(6 * S) };
        assert_eq!(format_timing(t), "[0:12-0:18]");
        assert_eq!(take_timing(&format!("{} words", format_timing(t))), (t, "words"));
    }

    #[test]
    fn a_time_typed_before_the_words_is_understood() {
        let both = Timing { start: Some(12 * S), duration: Some(6 * S) };
        for s in ["[0:12–0:18] Hello there", "[0:12-0:18] Hello there", "[00:12 - 00:18]: Hello there", "(0:12 --> 0:18) Hello there", "0:12-0:18 Hello there", "00:12 – 00:18: Hello there", "[0:12 +6s] Hello there", "@0:12 6s Hello there"] {
            let (t, rest) = take_timing(s);
            if s.starts_with('@') {
                // A bare `@` form is a point plus words.
                assert_eq!(t.start, Some(12 * S), "{s}");
                continue;
            }
            assert_eq!((t, rest), (both, "Hello there"), "{s}");
        }
        assert_eq!(take_timing("00:12 Hello"), (Timing { start: Some(12 * S), duration: None }, "Hello"));
        assert_eq!(take_timing("[0:12]"), (Timing { start: Some(12 * S), duration: None }, ""));
        assert_eq!(take_timing("[+4s] Hello"), (Timing { start: None, duration: Some(4 * S) }, "Hello"));
        assert_eq!(take_timing("{4.5s} Hello").0.duration, Some(4_500_000));
        assert_eq!(take_timing("(4s) Hello").0.duration, Some(4 * S));
        // Not times.
        for s in ["(10 seconds) Hello", "(whispering) Hello", "[[SHOT 1A]]", "Hello 0:12", "12 monkeys", "[note] x", "10:30pm was late"] {
            assert_eq!(take_timing(s), (Timing::default(), s), "{s}");
        }
    }

    fn scene(lines: Vec<Line>) -> Scene {
        Scene { id: "1".into(), lines, ..Default::default() }
    }

    fn narr(text: &str) -> Line {
        Line { kind: LineKind::Narration, who: Some("narrator".into()), text: text.into(), ..Default::default() }
    }

    #[test]
    fn lines_without_times_are_estimated_after_the_line_before() {
        let mut st = Story::default();
        let mut a = narr("One two three four five six seven eight.");
        a.start = Some(10 * S);
        st.scenes.push(scene(vec![a, narr("Then it ended."), narr("And again.")]));
        let t = st.line_times();
        assert_eq!(t[0].source, TimeSource::Said);
        assert_eq!(t[0].start, 10 * S);
        let dur0 = (8.0 / SPOKEN_WPS + 0.3) * S as f64;
        assert!((t[0].end as f64 - 10.0 * S as f64 - dur0).abs() < 2.0);
        assert_eq!((t[1].source, t[1].start), (TimeSource::Estimated, t[0].end));
        assert_eq!(t[2].start, t[1].end);
        // A said length is used.
        st.scenes[0].lines[1].duration = Some(5 * S);
        assert_eq!(st.line_times()[1].end - st.line_times()[1].start, 5 * S);
    }

    #[test]
    fn lines_follow_their_shot_when_the_clips_move() {
        let mut st = Story::default();
        let mut sc = scene(vec![narr("One two three four."), narr("Five six seven eight.")]);
        sc.shots.push(Shot { id: "1A".into(), kind: ShotKind::Wide, duration: 8 * S, lines: vec![0, 1], ..Default::default() });
        st.scenes.push(sc);
        let t = st.line_times();
        assert_eq!((t[0].source, t[0].start), (TimeSource::Shot, 0));
        assert!(t[1].start > 0 && t[1].end <= 8 * S);
        // The shot moves on the timeline: the lines go with it.
        st.scenes[0].shots[0].start = Some(20 * S);
        let moved = st.line_times();
        assert_eq!(moved[0].start, 20 * S);
        assert_eq!(moved[1].start, t[1].start + 20 * S);
        // A line the writer timed stays where it was put.
        st.scenes[0].lines[1].start = Some(3 * S);
        assert_eq!(st.line_times()[1].start, 3 * S);
        assert!(st.set_line_timing(0, 1, None, None));
        assert_eq!(st.line_times()[1].source, TimeSource::Shot);
    }

    #[test]
    fn planned_shots_cut_where_the_script_says() {
        let text = "[0:00] The fox woke early.\n\n[0:05] She walked to the river and looked at the water for a long time.\n\n[0:14-0:20] Then she jumped.\n";
        let mut st = fountain::read(text);
        assert_eq!(st.scenes.len(), 3);
        let times: Vec<Option<Micros>> = st.scenes.iter().flat_map(|s| s.lines.iter().map(|l| l.start)).collect();
        assert_eq!(times, vec![Some(0), Some(5 * S), Some(14 * S)]);
        st.plan(true);
        let starts: Vec<Micros> = st.timings().iter().map(|t| t.start).collect();
        assert!(starts.contains(&(5 * S)) && starts.contains(&(14 * S)), "{starts:?}\n{}", st.shot_list());
        assert_eq!(st.script_end(), 20 * S);
        assert_eq!(st.duration(), 20 * S, "{}", st.shot_list());
    }
}
