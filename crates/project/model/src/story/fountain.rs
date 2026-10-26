// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Screenplays in and out: Fountain (<https://fountain.io>) and plain text.
//!
//! Reading Fountain: the title page, scene headings (`INT.` / `EXT.` /
//! forced `.`, with `#1A#` scene numbers), character cues (`(V.O.)` makes
//! narration), parentheticals, dialogue, action, sections (`#` = beats)
//! and synopses (`=`). Studio's own extras ride in what other Fountain
//! apps ignore: each shot is a note (`[[SHOT 1A WS push_in 4.00s L1-2
//! @fox+owl side:a looks:right | what happens | framing]]`), and the
//! places and cast are a boneyard block (`/* studio:bible {…} */`), so a
//! story written out and read back is the same story.
//!
//! Plain text (no scene headings): `# Heading` lines start scenes (each
//! paragraph is a scene when there are none), `NAME: words` is dialogue
//! and everything else is narration, a line a sentence so each can be
//! given to the shot that shows it.

use super::times::{self, Timing, take_timing};
use super::{Beat, CameraMove, Character, Line, LineKind, Place, ScreenSide, Scene, Shot, ShotKind, Story, TimeOfDay};
use crate::time::SECOND;

/// An id from a name: lower case, words joined by `_`.
pub fn slug(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    s.split('_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_")
}

pub(super) fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const BIBLE: &str = "studio:bible";

/// Whether `text` reads as a Fountain screenplay (it has scene headings or
/// a title page) rather than plain prose.
pub fn looks_like_fountain(text: &str) -> bool {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let title_page = first.split_once(':').is_some_and(|(k, _)| matches!(k.trim().to_lowercase().as_str(), "title" | "credit" | "author" | "authors" | "draft date" | "contact"));
    title_page || text.lines().any(|l| heading(take_timing(l.trim()).1).is_some())
}

/// Reads a Fountain screenplay or plain text, whichever `text` is.
pub fn read(text: &str) -> Story {
    if looks_like_fountain(text) { parse(text) } else { parse_plain(text) }
}

/// A scene heading: (interior, place words, time of day words, scene
/// number).
fn heading(l: &str) -> Option<(bool, String, Option<String>, Option<String>)> {
    let (body, forced) = match l.strip_prefix('.') {
        Some(rest) if !rest.starts_with('.') && !rest.is_empty() => (rest, true),
        _ => (l, false),
    };
    let up = body.to_uppercase();
    let prefixes = ["INT./EXT.", "INT/EXT.", "INT/EXT", "I/E.", "I/E", "EXT.", "INT.", "EST.", "EXT ", "INT ", "EST "];
    let (interior, rest) = match prefixes.iter().find(|p| up.starts_with(**p)) {
        Some(p) => (p.starts_with("INT") && !p.contains("EXT") || p.starts_with("I/E"), body[p.len()..].trim()),
        None if forced => (false, body.trim()),
        None => return None,
    };
    let (rest, number) = match rest.trim_end().strip_suffix('#').and_then(|r| r.rsplit_once('#')) {
        Some((a, n)) => (a.trim(), Some(n.trim().to_owned())),
        None => (rest, None),
    };
    let (place, time) = match rest.rsplit_once(" - ").or_else(|| rest.rsplit_once(" -- ")) {
        Some((p, t)) => (p.trim().to_owned(), Some(t.trim().to_owned())),
        None => (rest.trim().to_owned(), None),
    };
    Some((interior, place, time, number))
}

/// A character cue: (name, extension such as `V.O.`).
fn cue(l: &str) -> Option<(String, Option<String>)> {
    let (body, forced) = match l.strip_prefix('@') {
        Some(r) => (r, true),
        None => (l, false),
    };
    let body = body.trim().trim_end_matches('^').trim();
    let (name, ext) = match body.find('(') {
        Some(i) => (body[..i].trim(), Some(body[i..].trim_matches(|c| c == '(' || c == ')' || c == ' ').to_owned())),
        None => (body, None),
    };
    if name.is_empty() || name.ends_with("TO:") {
        return None;
    }
    let letters = name.chars().any(char::is_alphabetic);
    let upper = name.chars().all(|c| !c.is_lowercase());
    (forced || (letters && upper && name.len() <= 40 && !name.ends_with(':'))).then(|| (name.to_owned(), ext))
}

/// Reads a shot note body (`SHOT 1A WS push_in 4s L1-2 @fox | action | framing`).
fn shot_note(note: &str) -> Option<Shot> {
    let rest = note.trim().strip_prefix("SHOT ").or_else(|| note.trim().strip_prefix("shot "))?;
    let mut parts = rest.splitn(3, " | ");
    let head = parts.next().unwrap_or("");
    let mut sh = Shot { action: parts.next().unwrap_or("").trim().to_owned(), framing: parts.next().unwrap_or("").trim().to_owned(), ..Default::default() };
    let mut toks = head.split_whitespace();
    sh.id = toks.next()?.to_owned();
    for t in toks {
        if let Some(s) = t.strip_suffix('s').and_then(|n| n.parse::<f64>().ok()) {
            sh.duration = (s * SECOND as f64).round() as i64;
        } else if let Some(k) = ShotKind::parse(t) {
            sh.kind = k;
        } else if let Some(m) = CameraMove::parse(t) {
            sh.camera = m;
        } else if let Some(l) = t.strip_prefix('L') {
            for r in l.split(',') {
                let (a, b) = r.split_once('-').unwrap_or((r, r));
                if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                    sh.lines.extend((a.max(1)..=b.max(a)).map(|n| n - 1));
                }
            }
        } else if let Some(s) = t.strip_prefix('@') {
            sh.subjects = s.split('+').filter(|w| !w.is_empty()).map(str::to_owned).collect();
        } else if let Some(s) = t.strip_prefix("side:") {
            sh.side = Some(s.to_owned());
        } else if let Some(s) = t.strip_prefix("looks:") {
            sh.looks = ScreenSide::parse(s);
        } else if t == "crosses" {
            sh.crosses = true;
        }
    }
    Some(sh)
}

fn write_shot(sh: &Shot) -> String {
    let mut head = vec![format!("SHOT {}", sh.id), sh.kind.abbrev().to_owned(), sh.camera.name().to_owned(), format!("{:.2}s", sh.duration as f64 / SECOND as f64)];
    if !sh.lines.is_empty() {
        head.push(format!("L{}", sh.lines.iter().map(|l| (l + 1).to_string()).collect::<Vec<_>>().join(",")));
    }
    if !sh.subjects.is_empty() {
        head.push(format!("@{}", sh.subjects.iter().map(|s| s.replace(' ', "_")).collect::<Vec<_>>().join("+")));
    }
    if let Some(s) = &sh.side {
        head.push(format!("side:{s}"));
    }
    if let Some(l) = sh.looks {
        head.push(format!("looks:{}", l.name()));
    }
    if sh.crosses {
        head.push("crosses".into());
    }
    let mut s = head.join(" ");
    if !sh.action.is_empty() || !sh.framing.is_empty() {
        s.push_str(&format!(" | {}", sh.action.replace("]]", "] ]").replace(" | ", ", ")));
    }
    if !sh.framing.is_empty() {
        s.push_str(&format!(" | {}", sh.framing.replace("]]", "] ]")));
    }
    format!("[[{s}]]")
}

/// The id a cue's name stands for: a bible character by name or id, else
/// the name's slug.
pub(super) fn who_of(st: &Story, name: &str) -> String {
    if name.eq_ignore_ascii_case("narrator") {
        return "narrator".into();
    }
    st.character(name).or_else(|| st.character(&slug(name))).map(|c| c.id.clone()).unwrap_or_else(|| slug(name))
}

/// Sets a scene's slug line from what the person typed; when it reads as
/// one (`EXT. MEADOW - DAWN`) the scene's place and time of day follow.
pub fn set_heading(st: &mut Story, scene: usize, text: &str) {
    let text = text.trim();
    if st.scenes.get(scene).is_none() {
        return;
    }
    let parsed = heading(text);
    let (place, tod) = match &parsed {
        Some((interior, place, time, _)) if !place.is_empty() => {
            let tod = time.as_deref().and_then(TimeOfDay::parse);
            (Some(ensure_place(st, place, *interior, tod)), tod)
        }
        _ => (None, None),
    };
    let sc = &mut st.scenes[scene];
    sc.heading = text.trim_start_matches('.').to_owned();
    if place.is_some() {
        sc.place = place;
        sc.time_of_day = tod;
    }
}

fn ensure_place(st: &mut Story, words: &str, interior: bool, time: Option<TimeOfDay>) -> String {
    let id = slug(words);
    if st.place(&id).is_none() && st.place(words).is_none() {
        st.places.push(Place { id: id.clone(), name: title_case(words), interior, time_of_day: time, ..Default::default() });
    }
    st.place(&id).or_else(|| st.place(words)).map(|p| p.id.clone()).unwrap_or(id)
}

pub(super) fn ensure_cast(st: &mut Story, scene: Option<&mut Scene>, who: &str, name: &str) {
    if who == "narrator" {
        return;
    }
    if st.character(who).is_none() {
        st.cast.push(Character { id: who.to_owned(), name: title_case(name), size: 1.0, ..Default::default() });
    }
    if let Some(s) = scene {
        if !s.cast.iter().any(|c| c == who) {
            s.cast.push(who.to_owned());
        }
    }
}

/// Reads a Fountain screenplay.
pub fn parse(text: &str) -> Story {
    let mut st = Story::default();
    // The bible first, so cues resolve to its characters.
    let mut body = text.to_owned();
    if let Some(i) = body.find(&format!("/* {BIBLE}")) {
        if let Some(j) = body[i..].find("*/") {
            let json = &body[i + BIBLE.len() + 3..i + j];
            if let Ok(b) = serde_json::from_str::<Story>(json.trim()) {
                st.places = b.places;
                st.cast = b.cast;
                st.beats = b.beats;
                st.logline = b.logline;
                st.extra = b.extra;
            }
            body.replace_range(i..i + j + 2, "");
        }
    }
    // Other boneyard is ignored.
    while let Some(i) = body.find("/*") {
        let end = body[i..].find("*/").map(|j| i + j + 2).unwrap_or(body.len());
        body.replace_range(i..end, "");
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    // Title page: key: value lines up to the first blank line.
    if lines.first().is_some_and(|l| l.split_once(':').is_some_and(|(k, _)| !k.trim().is_empty() && !k.contains(' ') || k.trim().eq_ignore_ascii_case("draft date"))) && heading(lines[0].trim()).is_none() && take_timing(lines[0].trim()).0.is_none() {
        while i < lines.len() && !lines[i].trim().is_empty() {
            if let Some((k, v)) = lines[i].split_once(':') {
                match k.trim().to_lowercase().as_str() {
                    "title" => st.title = v.trim().trim_matches('_').trim_matches('*').to_owned(),
                    "logline" => st.logline = v.trim().to_owned(),
                    _ => {}
                }
            }
            i += 1;
        }
    }
    let mut beat: Option<String> = None;
    let mut scene: Option<Scene> = None;
    let mut para: Vec<String> = Vec::new();
    // The time typed on the first line of the paragraph being read, and a
    // time typed on a line of its own, waiting for what it belongs to.
    let mut para_tm = Timing::default();
    let mut pending = Timing::default();
    let flush_action = |scene: &mut Option<Scene>, para: &mut Vec<String>, tm: &mut Timing| {
        let at = std::mem::take(tm);
        if para.is_empty() {
            return;
        }
        let text = para.join(" ").trim().to_owned();
        para.clear();
        if text.is_empty() {
            return;
        }
        if let Some(s) = scene {
            s.lines.push(Line { kind: LineKind::Action, who: None, text, start: at.start, duration: at.duration, ..Default::default() });
        }
    };
    while i < lines.len() {
        let raw = lines[i];
        let l = raw.trim();
        let blank_before = i == 0 || lines[i - 1].trim().is_empty();
        i += 1;
        if l.is_empty() {
            flush_action(&mut scene, &mut para, &mut para_tm);
            continue;
        }
        // Notes: Studio's shots, others ignored.
        if l.starts_with("[[") && l.ends_with("]]") {
            flush_action(&mut scene, &mut para, &mut para_tm);
            let body = &l[2..l.len() - 2];
            if let (Some(sh), Some(s)) = (shot_note(body), scene.as_mut()) {
                s.shots.push(sh);
            } else if let (Some(n), Some(s)) = (body.trim().strip_prefix("note:").or_else(|| body.trim().strip_prefix("NOTE:")), scene.as_mut()) {
                // A note after a line is the line's; before any, the scene's.
                match s.lines.last_mut() {
                    Some(last) => last.note = n.trim().to_owned(),
                    None => s.notes = n.trim().to_owned(),
                }
            }
            continue;
        }
        if let Some(sec) = l.strip_prefix('#') {
            flush_action(&mut scene, &mut para, &mut para_tm);
            let name = sec.trim_start_matches('#').trim();
            let (btm, name) = take_timing(name);
            let (id, summary) = match name.split_once(':') {
                Some((a, b)) if !a.contains(' ') => (a.trim().to_owned(), b.trim().to_owned()),
                _ => (slug(name), name.to_owned()),
            };
            if !st.beats.iter().any(|b| b.id == id) {
                st.beats.push(Beat { id: id.clone(), summary, start: btm.start, duration: btm.duration, ..Default::default() });
            }
            beat = Some(id);
            continue;
        }
        if let Some(syn) = l.strip_prefix('=').filter(|r| !r.starts_with("==")) {
            flush_action(&mut scene, &mut para, &mut para_tm);
            match scene.as_mut() {
                Some(s) if s.summary.is_empty() && s.lines.is_empty() => s.summary = syn.trim().to_owned(),
                Some(_) => {}
                None => {
                    if let Some(b) = beat.as_ref().and_then(|b| st.beats.iter_mut().find(|x| &x.id == b)) {
                        b.purpose = syn.trim().to_owned();
                    }
                }
            }
            continue;
        }
        // A time typed before the words, or on a line of its own.
        let (ltm, l_untimed) = take_timing(l);
        if !ltm.is_none() && l_untimed.is_empty() {
            flush_action(&mut scene, &mut para, &mut para_tm);
            pending = ltm;
            continue;
        }
        let timed = !ltm.is_none();
        let l = if timed { l_untimed } else { l };
        let ltm = if timed { ltm } else { std::mem::take(&mut pending) };
        if blank_before {
            if let Some((interior, place, time, number)) = heading(l) {
                flush_action(&mut scene, &mut para, &mut para_tm);
                if let Some(s) = scene.take() {
                    st.scenes.push(s);
                }
                let tod = time.as_deref().and_then(TimeOfDay::parse);
                let pid = if place.is_empty() { None } else { Some(ensure_place(&mut st, &place, interior, tod)) };
                let id = number.unwrap_or_else(|| (st.scenes.len() + 1).to_string());
                let text = l.trim_start_matches('.').trim_end();
                let text = text.strip_suffix('#').and_then(|r| r.rsplit_once('#')).map_or(text, |(a, _)| a.trim_end());
                scene = Some(Scene { id, heading: text.to_owned(), beat: beat.clone(), place: pid, time_of_day: tod, start: ltm.start, ..Default::default() });
                continue;
            }
            // Transitions.
            if (l.ends_with("TO:") && l == l.to_uppercase()) || (l.starts_with('>') && !l.ends_with('<')) {
                flush_action(&mut scene, &mut para, &mut para_tm);
                continue;
            }
            let next = lines.get(i).map(|n| n.trim()).unwrap_or("");
            if !next.is_empty() {
                if let Some((name, ext)) = cue(l) {
                    flush_action(&mut scene, &mut para, &mut para_tm);
                    if scene.is_none() {
                        scene = Some(Scene { id: (st.scenes.len() + 1).to_string(), beat: beat.clone(), ..Default::default() });
                    }
                    let who = who_of(&st, &name);
                    let vo = ext.as_deref().is_some_and(|e| matches!(e.to_uppercase().replace(' ', "").as_str(), "V.O." | "VO" | "V.O" | "NARRATING" | "NARRATION"));
                    let kind = if vo || who == "narrator" { LineKind::Narration } else { LineKind::Dialogue };
                    ensure_cast(&mut st, if kind == LineKind::Dialogue { scene.as_mut() } else { None }, &who, &name);
                    let mut how = None;
                    let mut words: Vec<&str> = Vec::new();
                    while i < lines.len() && !lines[i].trim().is_empty() {
                        let d = lines[i].trim();
                        if d.starts_with('(') && d.ends_with(')') {
                            if words.is_empty() {
                                how = Some(d.trim_matches(|c| c == '(' || c == ')').to_owned());
                            }
                        } else {
                            words.push(d);
                        }
                        i += 1;
                    }
                    if let Some(s) = scene.as_mut() {
                        // `FERN` / `[0:12] Hello` puts the time on the words.
                        let mut text = words.join(" ");
                        let mut tm = ltm;
                        if tm.is_none() {
                            let (t2, rest) = take_timing(&text);
                            if !t2.is_none() {
                                tm = t2;
                                text = rest.to_owned();
                            }
                        }
                        s.lines.push(Line { kind, who: Some(who), text, how, start: tm.start, duration: tm.duration, ..Default::default() });
                    }
                    continue;
                }
            }
        }
        if scene.is_none() {
            scene = Some(Scene { id: (st.scenes.len() + 1).to_string(), beat: beat.clone(), ..Default::default() });
        }
        // A new time starts a new line of action.
        if !ltm.is_none() {
            flush_action(&mut scene, &mut para, &mut para_tm);
            para_tm = ltm;
        }
        para.push(l.trim_start_matches('!').to_owned());
    }
    flush_action(&mut scene, &mut para, &mut para_tm);
    if let Some(s) = scene {
        st.scenes.push(s);
    }
    st
}

/// Sentences of a paragraph (split after `.`, `!`, `?` followed by a
/// space and a capital or a quote).
fn sentences(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let cs: Vec<char> = p.chars().collect();
    for (k, c) in cs.iter().enumerate() {
        cur.push(*c);
        if matches!(c, '.' | '!' | '?') {
            let after: String = cs[k + 1..].iter().take(3).collect();
            let mut a = after.chars();
            let closing = matches!(a.clone().next(), Some('"' | '”' | '\'' | '’'));
            if closing {
                continue;
            }
            if a.next() == Some(' ') && a.next().is_some_and(|n| n.is_uppercase() || matches!(n, '"' | '“' | '\'')) {
                out.push(cur.trim().to_owned());
                cur.clear();
            }
        } else if matches!(c, '"' | '”' | '\'' | '’') && cs.get(k.wrapping_sub(1)).is_some_and(|p| matches!(p, '.' | '!' | '?')) {
            let mut a = cs[k + 1..].iter();
            if a.next() == Some(&' ') && a.next().is_some_and(|n| n.is_uppercase()) {
                out.push(cur.trim().to_owned());
                cur.clear();
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// Reads plain prose: a narration or a simple script.
pub fn parse_plain(text: &str) -> Story {
    let mut st = Story::default();
    let paras: Vec<String> = text.split("\n\n").flat_map(|p| p.split("\r\n\r\n")).map(|p| p.lines().map(str::trim).collect::<Vec<_>>().join("\n").trim().to_owned()).filter(|p| !p.trim().is_empty()).collect();
    let headed = paras.iter().any(|p| p.starts_with('#'));
    let mut scene: Option<Scene> = None;
    let new_scene = |st: &Story, n: usize, summary: String| Scene { id: (st.scenes.len() + n).to_string(), summary, ..Default::default() };
    for p in paras {
        let mut rest = p.as_str();
        if let Some(h) = p.strip_prefix('#') {
            let (first, after) = h.split_once('\n').unwrap_or((h, ""));
            if let Some(s) = scene.take() {
                st.scenes.push(s);
            }
            let (htm, title) = take_timing(first.trim_start_matches('#').trim());
            scene = Some(Scene { start: htm.start, ..new_scene(&st, 1, title.to_owned()) });
            rest = after;
        } else if !headed {
            if let Some(s) = scene.take() {
                st.scenes.push(s);
            }
            scene = Some(new_scene(&st, 1, String::new()));
        } else if scene.is_none() {
            scene = Some(new_scene(&st, 1, String::new()));
        }
        // Dialogue lines (`NAME: words`) and narration between them. A line
        // that starts with a time (`[0:12–0:18] words`) is one line of its
        // own, not cut into sentences; a time on a line of its own waits
        // for the next line.
        let mut prose: Vec<&str> = Vec::new();
        let mut pending_lines: Vec<Line> = Vec::new();
        let flush = |prose: &mut Vec<&str>, pending_lines: &mut Vec<Line>| {
            if !prose.is_empty() {
                for s in sentences(&prose.join(" ")) {
                    pending_lines.push(Line { kind: LineKind::Narration, who: Some("narrator".into()), text: s, ..Default::default() });
                }
                prose.clear();
            }
        };
        let mut waiting = Timing::default();
        for l in rest.lines().filter(|l| !l.trim().is_empty()) {
            let (mut tm, l) = take_timing(l.trim());
            if l.is_empty() {
                flush(&mut prose, &mut pending_lines);
                if !tm.is_none() {
                    waiting = tm;
                }
                continue;
            }
            if tm.is_none() {
                tm = std::mem::take(&mut waiting);
            }
            let spoken = l.split_once(':').filter(|(n, w)| {
                let n = n.trim();
                !n.is_empty() && n.split_whitespace().count() <= 3 && n.chars().next().is_some_and(char::is_uppercase) && !w.trim().is_empty() && !n.chars().any(|c| c.is_ascii_digit())
            });
            if !tm.is_none() {
                flush(&mut prose, &mut pending_lines);
                let (kind, who, text) = match spoken {
                    Some((name, words)) => {
                        let who = who_of(&st, name);
                        (if who == "narrator" { LineKind::Narration } else { LineKind::Dialogue }, Some(who), words.trim().trim_matches('"').to_owned())
                    }
                    None => (LineKind::Narration, Some("narrator".to_owned()), l.trim().to_owned()),
                };
                pending_lines.push(Line { kind, who, text, start: tm.start, duration: tm.duration, ..Default::default() });
                continue;
            }
            match spoken {
                Some((name, words)) => {
                    flush(&mut prose, &mut pending_lines);
                    let who = who_of(&st, name);
                    let kind = if who == "narrator" { LineKind::Narration } else { LineKind::Dialogue };
                    pending_lines.push(Line { kind, who: Some(who), text: words.trim().trim_matches('"').to_owned(), ..Default::default() });
                }
                None => prose.push(l.trim()),
            }
        }
        flush(&mut prose, &mut pending_lines);
        let pending = pending_lines;
        let s = scene.get_or_insert_with(|| new_scene(&st, 1, String::new()));
        let mut speakers = Vec::new();
        for l in pending {
            if l.kind == LineKind::Dialogue {
                speakers.push(l.who.clone().unwrap_or_default());
            }
            s.lines.push(l);
        }
        for who in speakers {
            let name = who.replace('_', " ");
            ensure_cast(&mut st, scene.as_mut(), &who, &name);
        }
    }
    if let Some(s) = scene {
        st.scenes.push(s);
    }
    st
}

fn cue_name(st: &Story, who: &str) -> String {
    st.character(who).map(|c| if c.name.is_empty() { c.id.replace('_', " ") } else { c.name.clone() }).unwrap_or_else(|| who.replace('_', " ")).to_uppercase()
}

/// The scene's slug line: its own heading, or one made from its place.
pub fn slug_line(st: &Story, s: &Scene) -> String {
    if !s.heading.is_empty() {
        return s.heading.clone();
    }
    match st.place_of(s) {
        Some(p) => {
            let time = s.time_of_day.or(p.time_of_day).map(|t| t.name().replace('_', " ").to_uppercase());
            format!("{}. {}{}", if p.interior { "INT" } else { "EXT" }, if p.name.is_empty() { p.id.replace('_', " ") } else { p.name.clone() }.to_uppercase(), time.map(|t| format!(" - {t}")).unwrap_or_default())
        }
        None => format!(".SCENE {}", s.id),
    }
}

/// Writes the story as a Fountain screenplay, its shots as notes and the
/// bible (places, cast, beats) in a boneyard block at the end. Times ride
/// in front of the words (`[0:12-0:18] words`), notes as `[[note: …]]`.
pub fn write(st: &Story) -> String {
    write_with(st, true)
}

/// Only the script: scenes, lines, speakers, times and notes, no shots and
/// no bible. What the Story panel's text editor shows; [`super::import::merge_script`]
/// reads it back over the story it came from.
pub fn write_script(st: &Story) -> String {
    write_with(st, false)
}

fn write_with(st: &Story, full: bool) -> String {
    let mut out = String::new();
    if !st.title.is_empty() {
        out.push_str(&format!("Title: {}\n", st.title));
    }
    if !st.logline.is_empty() {
        out.push_str(&format!("Logline: {}\n", st.logline));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    let mut beat: Option<&str> = None;
    for s in &st.scenes {
        if s.beat.as_deref() != beat {
            if let Some(b) = s.beat.as_deref() {
                let x = st.beats.iter().find(|x| x.id == b);
                let summary = x.map(|x| x.summary.as_str()).unwrap_or("");
                let tm = x.map(|x| times::format_timing(Timing { start: x.start, duration: x.duration })).unwrap_or_default();
                out.push_str(&format!("# {}{b}{}\n\n", if tm.is_empty() { String::new() } else { format!("{tm} ") }, if summary.is_empty() { String::new() } else { format!(": {summary}") }));
            }
            beat = s.beat.as_deref();
        }
        let h = slug_line(st, s);
        let h = if heading(&h).is_some() { h } else { format!(".{h}") };
        let stm = times::format_timing(Timing { start: s.start, duration: None });
        out.push_str(&format!("{}{h} #{}#\n\n", if stm.is_empty() { String::new() } else { format!("{stm} ") }, s.id));
        if !s.summary.is_empty() {
            out.push_str(&format!("= {}\n\n", s.summary));
        }
        if !s.notes.is_empty() {
            out.push_str(&format!("[[note: {}]]\n\n", s.notes.replace("]]", "] ]").replace('\n', " ")));
        }
        for l in &s.lines {
            let tm = times::format_timing(l.timing());
            let tm_pre = if tm.is_empty() { String::new() } else { format!("{tm} ") };
            match l.kind {
                LineKind::Action => {
                    // The cue and heading tests see the words, not the time.
                    let escape = cue(&l.text).is_some() || heading(&l.text).is_some() || !take_timing(&l.text).0.is_none();
                    out.push_str(&format!("{tm_pre}{}{}\n\n", if escape { "!" } else { "" }, l.text));
                }
                LineKind::Dialogue | LineKind::Narration => {
                    let who = l.who.as_deref().unwrap_or("narrator");
                    let name = cue_name(st, who);
                    let vo = l.kind == LineKind::Narration;
                    out.push_str(&format!("{tm_pre}{}{}\n", name, if vo { " (V.O.)" } else { "" }));
                    if let Some(h) = &l.how {
                        out.push_str(&format!("({h})\n"));
                    }
                    out.push_str(&format!("{}\n\n", l.text));
                }
            }
            if !l.note.is_empty() {
                out.push_str(&format!("[[note: {}]]\n\n", l.note.replace("]]", "] ]").replace('\n', " ")));
            }
        }
        if full {
            for sh in &s.shots {
                out.push_str(&write_shot(sh));
                out.push('\n');
            }
            if !s.shots.is_empty() {
                out.push('\n');
            }
        }
    }
    let bible = Story { places: st.places.clone(), cast: st.cast.clone(), beats: st.beats.clone(), logline: st.logline.clone(), extra: st.extra.clone(), ..Default::default() };
    if full && !bible.is_empty() {
        out.push_str(&format!("/* {BIBLE}\n{}\n*/\n", serde_json::to_string_pretty(&bible).unwrap_or_default().replace("*/", "* /")));
    }
    out
}

/// Writes the story as plain text: headings, narration as prose,
/// dialogue as `NAME: words`.
pub fn write_plain(st: &Story) -> String {
    let mut out = String::new();
    if !st.title.is_empty() {
        out.push_str(&format!("{}\n\n", st.title));
    }
    for s in &st.scenes {
        let h = slug_line(st, s);
        let stm = times::format_timing(Timing { start: s.start, duration: None });
        out.push_str(&format!("# {}{}{}\n", if stm.is_empty() { String::new() } else { format!("{stm} ") }, h.trim_start_matches('.'), if s.summary.is_empty() { String::new() } else { format!(": {}", s.summary) }));
        let mut prose: Vec<&str> = Vec::new();
        let flush = |out: &mut String, prose: &mut Vec<&str>| {
            if !prose.is_empty() {
                out.push_str(&format!("{}\n", prose.join(" ")));
                prose.clear();
            }
        };
        for l in &s.lines {
            let tm = times::format_timing(l.timing());
            let pre = if tm.is_empty() { String::new() } else { format!("{tm} ") };
            match (l.kind, l.who.as_deref()) {
                (LineKind::Dialogue, Some(w)) => {
                    flush(&mut out, &mut prose);
                    out.push_str(&format!("{pre}{}: {}\n", title_case(&cue_name(st, w)), l.text));
                }
                (LineKind::Narration, Some(w)) if w != "narrator" => {
                    flush(&mut out, &mut prose);
                    out.push_str(&format!("{pre}{} (voice-over): {}\n", title_case(&cue_name(st, w)), l.text));
                }
                _ if !pre.is_empty() => {
                    flush(&mut out, &mut prose);
                    out.push_str(&format!("{pre}{}\n", l.text));
                }
                _ => prose.push(&l.text),
            }
        }
        flush(&mut out, &mut prose);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "Title: The Fox and the Owl
Credit: written by

# setup: Fern finds the meadow
= quiet, curious

EXT. MEADOW - DAWN #1#

= Fern wakes the meadow.

Mist lies on the grass. FERN, a young fox, pads out of the reeds.

NARRATOR (V.O.)
Every morning, Fern was the first one up.

FERN
(whispering)
Hello? Is anyone there?

OWL
Only me.

[[SHOT 1A EST push_in 4.5s L1-2 | the meadow in mist, Fern enters frame left | high and wide]]
[[SHOT 1B CU static 2s L3 @fern side:a looks:right]]

CUT TO:

INT. OWL'S HOLLOW - NIGHT #2#

The owl blinks.
";

    #[test]
    fn a_screenplay_becomes_scenes_lines_places_and_cast() {
        assert!(looks_like_fountain(SCRIPT));
        let st = read(SCRIPT);
        assert_eq!(st.title, "The Fox and the Owl");
        assert_eq!(st.beats.len(), 1);
        assert_eq!(st.beats[0].id, "setup");
        assert_eq!(st.beats[0].purpose, "quiet, curious");
        assert_eq!(st.scenes.len(), 2);
        let s = &st.scenes[0];
        assert_eq!((s.id.as_str(), s.place.as_deref(), s.time_of_day, s.beat.as_deref()), ("1", Some("meadow"), Some(TimeOfDay::Dawn), Some("setup")));
        assert_eq!(s.summary, "Fern wakes the meadow.");
        assert_eq!(s.lines.len(), 4);
        assert_eq!(s.lines[0].kind, LineKind::Action);
        assert_eq!((s.lines[1].kind, s.lines[1].who.as_deref()), (LineKind::Narration, Some("narrator")));
        assert_eq!((s.lines[2].kind, s.lines[2].who.as_deref(), s.lines[2].how.as_deref()), (LineKind::Dialogue, Some("fern"), Some("whispering")));
        assert_eq!(s.cast, vec!["fern".to_owned(), "owl".to_owned()]);
        assert_eq!(s.shots.len(), 2);
        assert_eq!((s.shots[0].kind, s.shots[0].camera, s.shots[0].duration, s.shots[0].lines.clone()), (ShotKind::Establishing, CameraMove::PushIn, 4_500_000, vec![0, 1]));
        assert_eq!(s.shots[0].framing, "high and wide");
        assert_eq!((s.shots[1].subjects.clone(), s.shots[1].side.as_deref(), s.shots[1].looks), (vec!["fern".to_owned()], Some("a"), Some(ScreenSide::Right)));
        let hollow = st.place("owl_s_hollow").expect("hollow");
        assert!(hollow.interior);
        assert_eq!(st.scenes[1].time_of_day, Some(TimeOfDay::Night));
        assert_eq!(st.cast.len(), 2);
        assert_eq!(st.character("fern").unwrap().name, "Fern");
    }

    #[test]
    fn written_out_and_read_back_it_is_the_same_story() {
        let mut st = read(SCRIPT);
        st.cast[0].look = "small red fox, white socks".into();
        st.cast[0].voice = Some("en-GB-1".into());
        st.places[0].ambience = Some("birds".into());
        st.places[0].surface = Some("grass".into());
        st.scenes[0].shots[1].crosses = true;
        let text = write(&st);
        let back = read(&text);
        assert_eq!(back.title, st.title);
        assert_eq!(back.cast, st.cast);
        assert_eq!(back.places, st.places);
        assert_eq!(back.beats, st.beats);
        assert_eq!(back.scenes.len(), st.scenes.len());
        for (a, b) in back.scenes.iter().zip(&st.scenes) {
            assert_eq!(a.lines, b.lines, "{text}");
            assert_eq!(a.shots, b.shots, "{text}");
            assert_eq!((&a.place, &a.beat, &a.id, &a.cast), (&b.place, &b.beat, &b.id, &b.cast));
        }
    }

    #[test]
    fn a_plain_narration_becomes_a_scene_a_paragraph_and_a_line_a_sentence() {
        let text = "Once upon a time, a fox lived by the river. She was quick and clever.\n\nOne day the rain came. \"Run!\" cried the heron. The fox ran.\n\nHeron: Follow me!\n";
        assert!(!looks_like_fountain(text));
        let st = read(text);
        assert_eq!(st.scenes.len(), 3);
        assert_eq!(st.scenes[0].lines.len(), 2);
        assert!(st.scenes[0].lines.iter().all(|l| l.kind == LineKind::Narration));
        assert_eq!(st.scenes[1].lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["One day the rain came.", "\"Run!\" cried the heron.", "The fox ran."]);
        assert_eq!((st.scenes[2].lines[0].kind, st.scenes[2].lines[0].who.as_deref()), (LineKind::Dialogue, Some("heron")));
        assert_eq!(st.scenes[2].cast, vec!["heron".to_owned()]);
        let plain = write_plain(&st);
        assert!(plain.contains("Heron: Follow me!"), "{plain}");
        assert!(plain.contains("One day the rain came. \"Run!\" cried the heron. The fox ran."), "{plain}");
    }

    #[test]
    fn headings_group_paragraphs_into_scenes() {
        let st = read("# The river\nThe water was high.\n\nIt kept rising.\n\n# The hill\nThey climbed.\n");
        assert_eq!(st.scenes.len(), 2);
        assert_eq!(st.scenes[0].summary, "The river");
        assert_eq!(st.scenes[0].lines.len(), 2);
        assert_eq!(st.scenes[1].lines[0].text, "They climbed.");
    }
}
