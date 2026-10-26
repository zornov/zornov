// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `story`: the production plan kept in the project. A screenplay or a
//! narration is read in, broken into beats, scenes and shots; each scene
//! has its place and its cast from the character bible; the plan is
//! checked for craft (coverage, variety, the line, eyelines, pacing) and
//! laid on the timeline, and the timeline is read back so they agree.
//!
//! For the agent, times are seconds and shot lines count from 1.

use super::{Session, f, micros, secs};
use crate::view::{self, LaneKind};
use serde_json::{Map, Value, json};
use studio_model as m;
use studio_model::story::{self as st, fountain};

/// Words of a line shown in a shot list.
fn excerpt(s: &str, n: usize) -> String {
    let w: Vec<&str> = s.split_whitespace().collect();
    if w.len() <= n { w.join(" ") } else { format!("{}…", w[..n].join(" ")) }
}

/// A path from the agent, `~` for home.
fn path_of(s: &str) -> std::path::PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default().join(rest),
        None => std::path::PathBuf::from(s),
    }
}

fn us(s: f64) -> m::Micros {
    (s * m::SECOND as f64).round() as m::Micros
}

fn sec(t: m::Micros) -> f64 {
    (t as f64 / m::SECOND as f64 * 100.0).round() / 100.0
}

/// A shot as the agent sees it.
fn shot_json(scene: &st::Scene, sh: &st::Shot, start: m::Micros) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), json!(sh.id));
    o.insert("kind".into(), json!(sh.kind.name()));
    if !sh.subjects.is_empty() {
        o.insert("subjects".into(), json!(sh.subjects));
    }
    if !sh.framing.is_empty() {
        o.insert("framing".into(), json!(sh.framing));
    }
    o.insert("camera".into(), json!(sh.camera.name()));
    o.insert("start".into(), json!(sec(start)));
    o.insert("duration".into(), json!(sec(sh.duration)));
    if !sh.lines.is_empty() {
        o.insert("lines".into(), json!(sh.lines.iter().map(|l| l + 1).collect::<Vec<_>>()));
        let words = st::Story::spoken_secs(scene, sh);
        if words > 0.0 {
            o.insert("words_take".into(), json!((words * 10.0).round() / 10.0));
        }
    }
    if !sh.action.is_empty() {
        o.insert("action".into(), json!(sh.action));
    }
    if let Some(s) = &sh.side {
        o.insert("side".into(), json!(s));
    }
    if sh.crosses {
        o.insert("crosses".into(), json!(true));
    }
    if let Some(l) = sh.looks {
        o.insert("looks".into(), json!(l.name()));
    }
    if !sh.clips.is_empty() {
        o.insert("clips".into(), json!(sh.clips.iter().map(|c| c.0).collect::<Vec<_>>()));
    }
    if !sh.notes.is_empty() {
        o.insert("notes".into(), json!(sh.notes));
    }
    Value::Object(o)
}

/// A shot in a line: `1A EST push_in 4.5s @fox L1-2 · the meadow wakes`.
fn shot_line(sh: &st::Shot, start: m::Micros) -> String {
    let mut s = format!("{} {} {} {:.1}s at {:.1}", sh.id, sh.kind.abbrev(), sh.camera.name(), sec(sh.duration), sec(start));
    if !sh.subjects.is_empty() {
        s.push_str(&format!(" @{}", sh.subjects.join("+")));
    }
    if !sh.lines.is_empty() {
        s.push_str(&format!(" L{}", sh.lines.iter().map(|l| (l + 1).to_string()).collect::<Vec<_>>().join(",")));
    }
    if let Some(side) = &sh.side {
        s.push_str(&format!(" side {side}"));
    }
    if let Some(l) = sh.looks {
        s.push_str(&format!(" looks {}", l.name()));
    }
    if !sh.clips.is_empty() {
        s.push_str(" [built]");
    }
    if !sh.action.is_empty() {
        s.push_str(&format!(" · {}", sh.action));
    }
    s
}

fn line_json(i: usize, l: &st::Line, t: Option<st::LineTime>) -> String {
    let who = match (l.kind, l.who.as_deref()) {
        (st::LineKind::Action, _) => "ACTION".to_owned(),
        (st::LineKind::Narration, Some("narrator") | None) => "NARRATOR".to_owned(),
        (st::LineKind::Narration, Some(w)) => format!("{} (V.O.)", w.to_uppercase()),
        (st::LineKind::Dialogue, w) => w.unwrap_or("?").to_uppercase(),
    };
    // `[0:12-0:18]` as the writer gave it; `~` after it when the time is
    // worked out (from the shot or reading speed) rather than given.
    let time = t.map(|t| format!(" [{}-{}]{}", st::times::format_time(t.start), st::times::format_time(t.end), if l.start.is_some() { "" } else { "~" })).unwrap_or_default();
    format!("{}{time} {who}{}: {}{}", i + 1, l.how.as_ref().map(|h| format!(" ({h})")).unwrap_or_default(), l.text, if l.note.is_empty() { String::new() } else { format!(" [note: {}]", l.note) })
}

/// The script in a line: how many lines, how many carry a time the writer
/// gave, and how long it runs.
fn script_summary(s: &st::Story) -> String {
    let lines: usize = s.scenes.iter().map(|x| x.lines.len()).sum();
    let timed = s.scenes.iter().flat_map(|x| x.lines.iter()).filter(|l| l.start.is_some()).count();
    format!("{} scenes, {lines} lines ({timed} with a given start; the rest follow their shot or reading speed), runs {}", s.scenes.len(), st::times::format_time(s.script_end()))
}

/// What the person's request starts with when the project has a script:
/// the AI is told so it builds from it.
pub fn script_context(s: &st::Story) -> Option<String> {
    let lines: usize = s.scenes.iter().map(|x| x.lines.len()).sum();
    (lines > 0).then(|| format!("the project has a script ({}): build and time the film from it with the story tool (show, line, plan, build) and keep the person's words", script_summary(s)))
}

/// A time the agent gives: seconds, or a timecode (`0:12`, `1:02:03`,
/// `00:00:12,500`, `12s`).
pub(super) fn time_arg(v: &Value, what: &str) -> Result<Option<m::Micros>, String> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(Some(us(n.as_f64().ok_or_else(|| format!("{what} is seconds"))?.max(0.0)))),
        Value::String(s) => {
            let s = s.trim();
            if let Ok(x) = s.parse::<f64>() {
                return Ok(Some(us(x.max(0.0))));
            }
            st::times::parse_time(s).map(Some).ok_or_else(|| format!("{what} \"{s}\" is not a time: seconds or a timecode like 0:12 or 00:01:02,500"))
        }
        _ => Err(format!("{what} is seconds or a timecode")),
    }
}

/// The start and length in a line or scene object from the agent:
/// `start` (or `at`), `end`, `duration`, or `time` (`"0:12-0:18"`, `"[0:12]"`).
/// Takes them out of the object. `Some(None)` means "clear it".
fn timing_in(o: &mut Map<String, Value>) -> Result<(Option<Option<m::Micros>>, Option<Option<m::Micros>>), String> {
    let mut start: Option<Option<m::Micros>> = None;
    let mut dur: Option<Option<m::Micros>> = None;
    if let Some(v) = o.remove("time").or_else(|| o.remove("timing")) {
        if let Some(t) = v.as_str() {
            let inner = t.trim().trim_start_matches(['[', '(']).trim_end_matches([']', ')']);
            let wrapped = format!("[{inner}]");
            let (tm, rest) = st::times::take_timing(&wrapped);
            if tm.is_none() || !rest.is_empty() {
                return Err(format!("time \"{t}\" is not a time or a range like 0:12-0:18"));
            }
            start = tm.start.map(Some);
            dur = tm.duration.map(Some);
        } else {
            start = Some(time_arg(&v, "time")?);
        }
    }
    let at = o.remove("start").or_else(|| o.remove("at"));
    let end = o.remove("end");
    let d = o.remove("duration").or_else(|| o.remove("length"));
    if let Some(v) = at {
        start = Some(time_arg(&v, "start")?);
    }
    if let Some(v) = d {
        dur = Some(match &v {
            Value::String(x) if x.trim().parse::<f64>().is_err() => st::times::parse_duration(x).or_else(|| st::times::parse_time(x)).map(Some).ok_or_else(|| format!("duration \"{x}\" is seconds or like 4s, 1m30s"))?,
            _ => time_arg(&v, "duration")?,
        });
    }
    if let Some(v) = end {
        let e = time_arg(&v, "end")?;
        if let (Some(Some(a)), Some(e)) = (start, e) {
            if e <= a {
                return Err("end is not after start".into());
            }
            dur = Some(Some(e - a));
        } else if e.is_some() {
            return Err("end needs a start (in the same call, or give a duration)".into());
        }
    }
    Ok((start, dur))
}

/// Reads one line from the agent: `"FOX: words"`, `"words"` (narration)
/// or `{"kind", "who", "text", "how"}`.
fn line_from(v: &Value) -> Result<st::Line, String> {
    match v {
        Value::String(s) => {
            // A time typed in front: "[0:12-0:18] FOX: words".
            let (tm, s) = st::times::take_timing(s);
            let mut l = line_from_words(s);
            l.start = tm.start;
            l.duration = tm.duration;
            Ok(l)
        }
        Value::Object(o) => {
            let mut o = o.clone();
            let (start, dur) = timing_in(&mut o)?;
            let mut l: st::Line = serde_json::from_value(Value::Object(o)).map_err(|e| format!("a line is \"WHO: words\" or {{kind, who, text, how, note, start, end, duration}}: {e}"))?;
            if l.kind == st::LineKind::Narration && l.who.is_none() {
                l.who = Some("narrator".into());
            }
            if let Some(s) = start {
                l.start = s;
            }
            if let Some(d) = dur {
                l.duration = d;
            }
            Ok(l)
        }
        _ => Err("a line is \"WHO: words\", plain words (narration) or {kind, who, text, how, note, start, end, duration}".into()),
    }
}

/// `"FOX: words"`, `"FOX (V.O.): words"`, `"ACTION: …"` or plain narration.
fn line_from_words(s: &str) -> st::Line {
    if let Some((who, text)) = s.split_once(':').filter(|(w, t)| !w.trim().is_empty() && w.split_whitespace().count() <= 3 && !t.trim().is_empty()) {
        let who = who.trim();
        let (who, vo) = match who.strip_suffix("(V.O.)").or_else(|| who.strip_suffix("(VO)")) {
            Some(w) => (w.trim(), true),
            None => (who, false),
        };
        let id = fountain::slug(who);
        return match id.as_str() {
            "action" => st::Line { kind: st::LineKind::Action, who: None, text: text.trim().into(), ..Default::default() },
            "narrator" | "narration" => st::Line { kind: st::LineKind::Narration, who: Some("narrator".into()), text: text.trim().into(), ..Default::default() },
            _ => st::Line { kind: if vo { st::LineKind::Narration } else { st::LineKind::Dialogue }, who: Some(id), text: text.trim().into(), ..Default::default() },
        };
    }
    st::Line { kind: st::LineKind::Narration, who: Some("narrator".into()), text: s.trim().into(), ..Default::default() }
}

/// Reads a shot from the agent over `base`: loose names, seconds, lines
/// from 1.
fn shot_from(v: &Value, mut sh: st::Shot) -> Result<st::Shot, String> {
    let o = v.as_object().ok_or("a shot is an object: {id, kind, subjects, camera, duration, lines, action…}")?;
    for (k, x) in o {
        let s = || x.as_str().map(str::to_owned);
        match k.as_str() {
            "id" => sh.id = s().ok_or("shot id is text")?,
            "kind" | "type" | "size" => sh.kind = st::ShotKind::parse(&s().unwrap_or_default()).ok_or_else(|| format!("shot kind \"{x}\": {}", st::ShotKind::ALL.map(|k| k.name()).join(", ")))?,
            "camera" | "move" => sh.camera = st::CameraMove::parse(&s().unwrap_or_default()).ok_or_else(|| format!("camera \"{x}\": {}", st::CameraMove::ALL.map(|k| k.name()).join(", ")))?,
            "subjects" | "subject" | "who" => {
                sh.subjects = match x {
                    Value::String(t) => t.split(['+', ',']).map(|w| fountain::slug(w)).filter(|w| !w.is_empty()).collect(),
                    Value::Array(a) => a.iter().filter_map(Value::as_str).map(fountain::slug).collect(),
                    _ => Vec::new(),
                }
            }
            "framing" => sh.framing = s().unwrap_or_default(),
            "duration" => sh.duration = us(x.as_f64().ok_or("shot duration is seconds")?.max(0.04)),
            "lines" => {
                sh.lines = match x {
                    Value::Array(a) => a.iter().filter_map(Value::as_u64).filter(|n| *n >= 1).map(|n| n as usize - 1).collect(),
                    Value::Number(n) => n.as_u64().filter(|n| *n >= 1).map(|n| vec![n as usize - 1]).unwrap_or_default(),
                    Value::String(t) => {
                        let mut v = Vec::new();
                        for r in t.split(',') {
                            let (a, b) = r.trim().split_once('-').unwrap_or((r.trim(), r.trim()));
                            if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                                v.extend((a.max(1)..=b.max(a)).map(|n| n - 1));
                            }
                        }
                        v
                    }
                    _ => Vec::new(),
                }
            }
            "action" | "what" => sh.action = s().unwrap_or_default(),
            "side" => sh.side = s().filter(|t| !t.is_empty()),
            "crosses" => sh.crosses = x.as_bool().unwrap_or(false),
            "looks" | "eyeline" => sh.looks = match s() {
                Some(t) if !t.is_empty() => Some(st::ScreenSide::parse(&t).ok_or("looks is left, right or camera")?),
                _ => None,
            },
            "notes" => sh.notes = s().unwrap_or_default(),
            "clips" => sh.clips = x.as_array().map(|a| a.iter().filter_map(Value::as_u64).map(m::ClipId).collect()).unwrap_or_default(),
            "start" | "after" | "remove" | "scene" | "words_take" => {}
            other => return Err(format!("a shot has no \"{other}\": id, kind, subjects, framing, camera, duration, lines, action, side, crosses, looks, notes")),
        }
    }
    Ok(sh)
}

/// Merges `patch` over `base` (both JSON objects): keys in the patch win,
/// null removes.
fn merge<T: serde::Serialize + serde::de::DeserializeOwned>(base: &T, patch: &Value, what: &str) -> Result<T, String> {
    let mut v = serde_json::to_value(base).map_err(|e| e.to_string())?;
    if let (Some(o), Some(p)) = (v.as_object_mut(), patch.as_object()) {
        for (k, x) in p {
            if k == "remove" || k == "after" {
                continue;
            }
            let x = match (k.as_str(), x) {
                ("time_of_day", Value::String(s)) => json!(st::TimeOfDay::parse(s).ok_or_else(|| format!("time_of_day \"{s}\": dawn, morning, day, golden_hour, dusk, night"))?.name()),
                ("start" | "duration", v) => match time_arg(v, k)? {
                    Some(t) => json!(t),
                    None => Value::Null,
                },
                ("facing", Value::String(s)) => json!(st::ScreenSide::parse(s).ok_or("facing is left, right or camera")?.name()),
                ("references" | "moves" | "cast", Value::String(s)) => json!(s.split(',').map(|w| w.trim()).filter(|w| !w.is_empty()).collect::<Vec<_>>()),
                _ => x.clone(),
            };
            if x.is_null() {
                o.remove(k);
            } else {
                o.insert(k.clone(), x);
            }
        }
    }
    serde_json::from_value(v).map_err(|e| format!("{what}: {e}"))
}

/// How big the main subject stands in each shot size, as a share of the
/// frame height, and what the framing should be.
fn framing_guide(k: st::ShotKind) -> (f64, &'static str) {
    use st::ShotKind::*;
    match k {
        Establishing => (0.15, "the whole place; characters small (under a fifth of the height) or not there yet; horizon on a third"),
        Wide => (0.45, "whole bodies with room around them and ground under their feet; where they are in the place reads"),
        TwoShot => (0.6, "both characters, bodies cropped at the knees or waist, one each side of the frame facing in"),
        Medium => (0.85, "from the waist (an animal: the shoulders or chest) up; the eyes on the upper third"),
        OverTheShoulder => (0.8, "past the near character's shoulder (blurred, frame edge) onto the far one's face"),
        CloseUp => (1.6, "the face fills the frame: chin to top of the head, eyes on the upper third, looking room on the side they look to"),
        ExtremeCloseUp => (3.0, "eyes only, or one detail of the face"),
        Reaction => (1.4, "close on the one who hears it: the reaction is the shot, hold it"),
        Insert => (0.0, "the thing itself filling the frame: a paw, a door, a letter, the water"),
        Pov => (0.0, "what the character sees from where they stand: their eye height, no them in frame"),
    }
}

fn camera_guide(c: st::CameraMove) -> &'static str {
    use st::CameraMove::*;
    match c {
        Static => "locked off: no move",
        Pan => "turn across the place: animate x of the backdrop (or a camera move pan) over the shot",
        Tilt => "turn up or down: animate y of the backdrop over the shot",
        PushIn => "move in slowly: camera move push_in, or animate scale of the backdrop about 100→110% over the shot",
        PullOut => "move back: camera move pull_out, or animate scale about 110→100%",
        Track => "travel with the subject: the backdrop slides the other way (parallax: far layers slower), the character walks in place or travels",
        Crane => "rise or fall: animate y and a little scale together",
        Handheld => "alive: a small noise modifier on position (2-4 px)",
        Zoom => "the lens zooms: scale the whole frame, nothing shifts in depth",
    }
}

impl Session {
    /// The project's story, its built shots read back from the timeline.
    pub(super) fn story_now(&self) -> st::Story {
        let mut s = self.doc.model.story.clone();
        if let Some(seq) = self.doc.model.sequence(self.doc.seq()) {
            s.sync(seq);
        }
        s
    }

    fn put_story(&mut self, story: st::Story, label: &str) -> Result<(), String> {
        if story == self.doc.model.story {
            return Ok(());
        }
        let ok = self.doc.apply(label, m::Edit::SetStory { story: Box::new(story) });
        self.check(ok, "").map(|_| ())
    }

    /// The `story` tool.
    pub(super) fn story_tool(&mut self, a: &Value) -> Result<Value, String> {
        let what = a.get("do").or_else(|| a.get("action")).and_then(Value::as_str).unwrap_or("show");
        match what {
            "show" | "outline" | "read" => Ok(self.story_show(a)),
            "cast" | "bible" | "places" => Ok(self.story_bible()),
            "import" => self.story_import(a),
            "export" => self.story_export(a),
            "set" | "write" | "revise" => self.story_set(a),
            "remove" | "delete" => self.story_remove(a),
            "plan" => self.story_plan(a),
            "line" | "lines" | "time" | "times" | "retime" => self.story_line(a),
            "check" => Ok(self.story_check()),
            "build" => self.story_build(a),
            "bind" => self.story_bind(a),
            "brief" => self.story_brief(a),
            other => Err(format!("story \"do\": \"{other}\": show, cast, import, export, set, remove, plan, line, retime, check, build, bind or brief")),
        }
    }

    fn story_show(&self, a: &Value) -> Value {
        let s = self.story_now();
        if s.is_empty() {
            return json!({"story": "none yet", "next": "import a screenplay or narration (story {do: import, text | path}), or write one with story {do: set, scenes: [...]}"});
        }
        let times = s.timings();
        let start_of = |si: usize, i: usize| times.iter().find(|t| t.scene == si && t.shot == i).map(|t| t.start).unwrap_or(0);
        if let Some(id) = a.get("scene").and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string()))) {
            let Some(si) = s.scenes.iter().position(|x| x.id.eq_ignore_ascii_case(&id)) else {
                return json!({"error": format!("no scene {id}: {}", s.scenes.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", "))});
            };
            let sc = &s.scenes[si];
            let lt = s.line_times();
            return json!({
                "scene": sc.id, "heading": fountain::slug_line(&s, sc), "beat": sc.beat, "place": sc.place, "time_of_day": sc.time_of_day.map(|t| t.name()),
                "cast": sc.cast, "summary": sc.summary,
                "start": sc.start.map(|t| st::times::format_time(t)), "notes": sc.notes,
                "lines": sc.lines.iter().enumerate().map(|(i, l)| line_json(i, l, lt.iter().find(|t| t.scene == si && t.line == i).copied())).collect::<Vec<_>>(),
                "shots": sc.shots.iter().enumerate().map(|(i, sh)| shot_json(sc, sh, start_of(si, i))).collect::<Vec<_>>(),
            });
        }
        let issues = s.check();
        json!({
            "title": s.title, "logline": s.logline,
            "beats": s.beats.iter().map(|b| format!("{}: {}{}", b.id, b.summary, if b.purpose.is_empty() { String::new() } else { format!(" ({})", b.purpose) })).collect::<Vec<_>>(),
            "places": s.places.iter().map(|p| format!("{}{}{}{}{}", p.id, p.backdrop.as_ref().map(|b| format!(" · backdrop {b}")).unwrap_or_default(), p.ambience.as_ref().map(|b| format!(" · sounds {b}")).unwrap_or_default(), p.surface.as_ref().map(|b| format!(" · ground {b}")).unwrap_or_default(), if p.light.is_empty() { String::new() } else { format!(" · light {}", p.light) })).collect::<Vec<_>>(),
            "cast": s.cast.iter().map(|c| format!("{}{}{}{}", c.id, if (c.size - 1.0).abs() > 1e-6 { format!(" · size {}", c.size) } else { String::new() }, c.voice.as_ref().map(|v| format!(" · voice {v}")).unwrap_or_default(), if c.look.is_empty() { " · NO LOOK".to_owned() } else { format!(" · {}", excerpt(&c.look, 8)) })).collect::<Vec<_>>(),
            "scenes": s.scenes.iter().enumerate().map(|(si, sc)| json!({
                "id": sc.id, "heading": fountain::slug_line(&s, sc), "beat": sc.beat, "cast": sc.cast,
                "lines": sc.lines.len(), "summary": if sc.summary.is_empty() { sc.lines.first().map(|l| excerpt(&l.text, 10)).unwrap_or_default() } else { sc.summary.clone() },
                "shots": sc.shots.iter().enumerate().map(|(i, sh)| shot_line(sh, start_of(si, i))).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "script": script_summary(&s),
            "shots": times.len(), "planned": sec(s.duration()), "timeline": secs(self.doc.project.duration()),
            "problems": issues.iter().filter(|i| i.severity >= st::Severity::Warning).count(),
            "next": if issues.is_empty() { "story check is clean: build it, or stage each shot (story brief) and bind its clips" } else { "story check lists what to fix" },
        })
    }

    fn story_bible(&self) -> Value {
        let s = self.story_now();
        json!({"cast": s.cast, "places": s.places, "narrator_voice": s.extra.get("narrator_voice")})
    }

    fn story_import(&mut self, a: &Value) -> Result<Value, String> {
        let (text, from, ext) = match (a.get("text").and_then(Value::as_str), a.get("path").and_then(Value::as_str)) {
            (Some(t), _) => (t.to_owned(), "text".to_owned(), None),
            (None, Some(p)) => {
                let p = path_of(p);
                let ext = p.extension().and_then(|e| e.to_str()).map(str::to_lowercase);
                (crate::script_import::read_file(&p)?, p.display().to_string(), ext)
            }
            _ => return Err("import needs \"text\" (the screenplay or narration) or \"path\" (.fountain, .txt, .md, .srt, .vtt, .docx)".into()),
        };
        let mut new = match a.get("format").and_then(Value::as_str) {
            Some("fountain") => fountain::parse(&text),
            Some("text" | "plain") => fountain::parse_plain(&text),
            Some("markdown" | "md") => st::import::parse_markdown(&text),
            Some("srt" | "vtt" | "subtitles") => st::import::parse_subtitles(&text),
            Some(other) => return Err(format!("import format \"{other}\": fountain, text, markdown, srt or vtt (default: whichever it reads as)")),
            None => st::import::read_script(&text, if ext.as_deref() == Some("docx") { Some("md") } else { ext.as_deref() }),
        };
        if new.scenes.is_empty() {
            return Err("no scenes in that text".into());
        }
        let old = self.story_now();
        let replace = a.get("replace").and_then(Value::as_bool).unwrap_or(false);
        let merge = a.get("merge").and_then(Value::as_bool).unwrap_or(false);
        if !old.scenes.is_empty() && !replace && !merge {
            return Err(format!("the project already has a story ({} scenes): pass \"replace\": true to read this one in its place (the cast and places you described are kept), or \"merge\": true to put this text over it scene by scene (scenes with the same id keep their shots)", old.scenes.len()));
        }
        if merge {
            new = st::import::merge_script(&old, new);
        } else {
            // What the bible already says is kept.
            for c in &old.cast {
                match new.cast.iter_mut().find(|n| n.id == c.id) {
                    Some(n) => *n = c.clone(),
                    None => new.cast.push(c.clone()),
                }
            }
            for p in &old.places {
                match new.places.iter_mut().find(|n| n.id == p.id) {
                    Some(n) => *n = p.clone(),
                    None => new.places.push(p.clone()),
                }
            }
        }
        if new.title.is_empty() {
            new.title = if old.title.is_empty() { self.doc.project.name.clone() } else { old.title.clone() };
        }
        let planned = if a.get("plan").and_then(Value::as_bool).unwrap_or(false) { new.plan(false) } else { Vec::new() };
        if planned.is_empty() {
            new.retime_shots();
        }
        let lines: usize = new.scenes.iter().map(|s| s.lines.len()).sum();
        self.put_story(new, "Import script")?;
        let mut out = self.story_show(&json!({}));
        out["imported"] = json!(format!("{} scenes, {lines} lines, {} places, {} characters from {from}{}", self.doc.model.story.scenes.len(), self.doc.model.story.places.len(), self.doc.model.story.cast.len(), if planned.is_empty() { String::new() } else { format!("; shots planned for {} scenes", planned.len()) }));
        Ok(out)
    }

    fn story_export(&mut self, a: &Value) -> Result<Value, String> {
        let s = self.story_now();
        if s.scenes.is_empty() {
            return Err("there is no story to export".into());
        }
        let path = a.get("path").and_then(Value::as_str).map(path_of);
        let fmt = a.get("format").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| match path.as_ref().and_then(|p| p.extension()).and_then(|e| e.to_str()) {
            Some("txt" | "md") => "text".into(),
            Some("srt") => "srt".into(),
            _ => "fountain".into(),
        });
        let text = match fmt.as_str() {
            "fountain" => fountain::write(&s),
            "text" | "plain" => fountain::write_plain(&s),
            "shots" | "shot_list" => s.shot_list(),
            "srt" | "subtitles" => st::import::write_srt(&s),
            "script" => fountain::write_script(&s),
            other => return Err(format!("export format \"{other}\": fountain, script, text, srt or shots")),
        };
        match path {
            Some(p) => {
                std::fs::write(&p, &text).map_err(|e| format!("could not write {}: {e}", p.display()))?;
                Ok(json!({"written": p.display().to_string(), "format": fmt, "bytes": text.len()}))
            }
            None => Ok(json!({"format": fmt, "text": text})),
        }
    }

    fn story_set(&mut self, a: &Value) -> Result<Value, String> {
        let mut s = self.story_now();
        let mut said: Vec<String> = Vec::new();
        for k in ["title", "logline"] {
            if let Some(v) = a.get(k).and_then(Value::as_str) {
                if k == "title" { s.title = v.into() } else { s.logline = v.into() }
                said.push(k.into());
            }
        }
        if let Some(v) = a.get("narrator_voice") {
            if v.is_null() {
                s.extra.remove("narrator_voice");
            } else {
                s.extra.insert("narrator_voice".into(), v.clone());
            }
            said.push("narrator voice".into());
        }
        for b in a.get("beats").and_then(Value::as_array).into_iter().flatten() {
            let id = b.get("id").and_then(Value::as_str).ok_or("each beat needs an id")?.to_owned();
            if b.get("remove").and_then(Value::as_bool) == Some(true) {
                s.beats.retain(|x| x.id != id);
                continue;
            }
            match s.beats.iter().position(|x| x.id == id) {
                Some(i) => s.beats[i] = merge(&s.beats[i], b, &format!("beat {id}"))?,
                None => s.beats.push(merge(&st::Beat { id: id.clone(), ..Default::default() }, b, &format!("beat {id}"))?),
            }
            said.push(format!("beat {id}"));
        }
        for p in a.get("places").and_then(Value::as_array).into_iter().flatten() {
            let id = fountain::slug(p.get("id").or_else(|| p.get("name")).and_then(Value::as_str).ok_or("each place needs an id (or a name)")?);
            if p.get("remove").and_then(Value::as_bool) == Some(true) {
                s.places.retain(|x| x.id != id);
                continue;
            }
            let mut p = p.clone();
            p["id"] = json!(id);
            match s.places.iter().position(|x| x.id == id) {
                Some(i) => s.places[i] = merge(&s.places[i], &p, &format!("place {id}"))?,
                None => s.places.push(merge(&st::Place { id: id.clone(), ..Default::default() }, &p, &format!("place {id}"))?),
            }
            said.push(format!("place {id}"));
        }
        for c in a.get("cast").and_then(Value::as_array).into_iter().flatten() {
            let id = fountain::slug(c.get("id").or_else(|| c.get("name")).and_then(Value::as_str).ok_or("each character needs an id (or a name)")?);
            if c.get("remove").and_then(Value::as_bool) == Some(true) {
                s.cast.retain(|x| x.id != id);
                continue;
            }
            let mut c = c.clone();
            c["id"] = json!(id);
            if let Some(m) = c.get("moves").and_then(Value::as_array) {
                if let Some(bad) = m.iter().filter_map(Value::as_str).find(|a| !super::pins::is_act(a)) {
                    return Err(format!("{id} moves \"{bad}\" is not a puppet act: {}", super::pins::ACTS));
                }
            }
            match s.cast.iter().position(|x| x.id == id) {
                Some(i) => s.cast[i] = merge(&s.cast[i], &c, &format!("character {id}"))?,
                None => s.cast.push(merge(&st::Character { id: id.clone(), size: 1.0, ..Default::default() }, &c, &format!("character {id}"))?),
            }
            said.push(format!("character {id}"));
        }
        for sc in a.get("scenes").and_then(Value::as_array).into_iter().flatten() {
            let id = sc.get("id").map(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string()))).unwrap_or(None).unwrap_or_else(|| (s.scenes.len() + 1).to_string());
            if sc.get("remove").and_then(Value::as_bool) == Some(true) {
                s.scenes.retain(|x| x.id != id);
                said.push(format!("scene {id} removed"));
                continue;
            }
            let si = match s.scenes.iter().position(|x| x.id.eq_ignore_ascii_case(&id)) {
                Some(i) => i,
                None => {
                    let at = match sc.get("after").and_then(Value::as_str) {
                        Some(after) => s.scenes.iter().position(|x| x.id == after).map(|i| i + 1).ok_or_else(|| format!("no scene {after} to put scene {id} after"))?,
                        None => s.scenes.len(),
                    };
                    s.scenes.insert(at, st::Scene { id: id.clone(), ..Default::default() });
                    at
                }
            };
            let mut patch = sc.clone();
            let o = patch.as_object_mut().ok_or("a scene is an object")?;
            let lines = o.remove("lines");
            let shots = o.remove("shots");
            let replace_shots = o.remove("replace_shots").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(Value::String(p)) = o.get("place").cloned() {
                o.insert("place".into(), json!(fountain::slug(&p)));
            }
            if let Some(Value::Array(c)) = o.get("cast").cloned() {
                o.insert("cast".into(), json!(c.iter().filter_map(Value::as_str).map(fountain::slug).collect::<Vec<_>>()));
            }
            let keep_shots = std::mem::take(&mut s.scenes[si].shots);
            let keep_lines = std::mem::take(&mut s.scenes[si].lines);
            let mut scene = merge(&s.scenes[si], &patch, &format!("scene {id}"))?;
            scene.shots = keep_shots;
            scene.lines = match lines {
                Some(Value::Array(ls)) => ls.iter().map(line_from).collect::<Result<_, _>>()?,
                Some(Value::String(t)) => fountain::parse_plain(&t).scenes.into_iter().flat_map(|x| x.lines).collect(),
                _ => keep_lines,
            };
            s.scenes[si] = scene;
            if let Some(Value::Array(list)) = shots {
                if replace_shots {
                    s.scenes[si].shots.clear();
                }
                for v in &list {
                    let sid = v.get("id").and_then(Value::as_str).map(str::to_owned);
                    if v.get("remove").and_then(Value::as_bool) == Some(true) {
                        if let Some(sid) = &sid {
                            s.scenes[si].shots.retain(|x| &x.id != sid);
                        }
                        continue;
                    }
                    match sid.as_ref().and_then(|sid| s.scenes[si].shots.iter().position(|x| &x.id == sid)) {
                        Some(i) => s.scenes[si].shots[i] = shot_from(v, s.scenes[si].shots[i].clone())?,
                        None => {
                            if let Some(sid) = &sid {
                                if s.find_shot(sid).is_some() {
                                    return Err(format!("shot {sid} is in another scene: shot ids are unique in the film"));
                                }
                            }
                            let mut sh = shot_from(v, st::Shot { duration: us(3.0), ..Default::default() })?;
                            if sh.id.is_empty() {
                                sh.id = s.next_shot_id(si);
                            }
                            let at = match v.get("after").and_then(Value::as_str) {
                                Some(after) => s.scenes[si].shots.iter().position(|x| x.id == after).map(|i| i + 1).ok_or_else(|| format!("no shot {after} in scene {id}"))?,
                                None => s.scenes[si].shots.len(),
                            };
                            s.scenes[si].shots.insert(at, sh);
                        }
                    }
                }
            }
            said.push(format!("scene {id}"));
        }
        // Shots by id wherever they are.
        let mut resized: Vec<String> = Vec::new();
        for v in a.get("shots").and_then(Value::as_array).into_iter().flatten() {
            let sid = v.get("id").and_then(Value::as_str).ok_or("each shot in \"shots\" needs its id (new shots go in a scene's shots)")?;
            let (si, i) = s.find_shot(sid).ok_or_else(|| format!("no shot {sid}"))?;
            if v.get("remove").and_then(Value::as_bool) == Some(true) {
                s.scenes[si].shots.remove(i);
                said.push(format!("shot {sid} removed"));
                continue;
            }
            let before = s.scenes[si].shots[i].duration;
            let sh = shot_from(v, s.scenes[si].shots[i].clone())?;
            if sh.duration != before && !sh.clips.is_empty() {
                resized.push(sid.to_owned());
            }
            s.scenes[si].shots[i] = sh;
            said.push(format!("shot {sid}"));
        }
        if said.is_empty() {
            return Err("set changes nothing: give title, logline, narrator_voice, beats, places, cast, scenes (with lines and shots) or shots".into());
        }
        // A built shot that changes length: its clips follow.
        let mut notes = Vec::new();
        if !resized.is_empty() {
            self.doc.begin_batch("Story");
            for sid in &resized {
                let (si, i) = s.find_shot(sid).unwrap();
                let sh = s.scenes[si].shots[i].clone();
                for c in &sh.clips {
                    if let Some(vc) = self.doc.project.clip(c.0 as u32).cloned() {
                        let want = secs(micros(sec(sh.duration))) - (secs(vc.start) - sec(sh.start.unwrap_or(0)));
                        if want > 0.05 {
                            if let Err(e) = self.op(&json!({"op": "set", "clip": vc.id, "duration": want})) {
                                notes.push(format!("clip {} of shot {sid} kept its length: {e}", vc.id));
                            }
                        }
                    }
                }
            }
            // The plan as the trimmed clips now stand, in the same step.
            let mut synced = s;
            if let Some(seq) = self.doc.model.sequence(self.doc.seq()) {
                synced.sync(seq);
            }
            if let Err(e) = self.put_story(synced, "Story") {
                self.doc.rollback_batch();
                return Err(e);
            }
            self.doc.commit_batch();
            notes.push(format!("built shots {} were trimmed to their new length on the timeline", resized.join(", ")));
        } else {
            self.put_story(s, "Story")?;
        }
        let problems: Vec<Value> = self.story_now().check().into_iter().filter(|i| i.severity >= st::Severity::Warning).take(8).map(|i| json!(format!("{}{}: {}", i.scene.map(|s| format!("scene {s} ")).unwrap_or_default(), i.kind, i.problem))).collect();
        Ok(json!({"ok": true, "changed": said, "notes": notes, "revision": self.doc.history.revision(), "problems": problems}))
    }

    /// `story {do: line}`: read and write the lines of the script and their
    /// times. Patch line `line` of `scene` (text, who, kind, how, note, start,
    /// end, duration, or `time: "0:12-0:18"`; `clear_time: true` lets it follow
    /// its shot again), add one (no `line`; `after` puts it after line n),
    /// `remove: true`, shift times (`shift` seconds, from line `from`), or
    /// several at once in `edits`. `do: retime` lays planned shots on the
    /// times. With nothing to change it lists the lines with their times.
    fn story_line(&mut self, a: &Value) -> Result<Value, String> {
        let what = a.get("do").and_then(Value::as_str).unwrap_or("line");
        let mut s = self.story_now();
        let scene_of = |v: &Value| v.get("scene").and_then(|x| x.as_str().map(str::to_owned).or_else(|| x.as_u64().map(|n| n.to_string())));
        if what == "retime" {
            let n = s.retime_shots();
            if n == 0 {
                return Err("nothing to retime: no planned shot has lines with a given start (built shots follow the timeline)".into());
            }
            self.put_story(s, "Retime shots")?;
            let mut out = self.story_show(&json!({}));
            out["retimed"] = json!(format!("{n} shots now cut where the script's times say"));
            return Ok(out);
        }
        let edits: Vec<Value> = match a.get("edits").and_then(Value::as_array) {
            Some(l) => l.iter().map(|e| {
                let mut e = e.clone();
                if e.get("scene").is_none() {
                    if let (Some(o), Some(sc)) = (e.as_object_mut(), a.get("scene")) {
                        o.insert("scene".into(), sc.clone());
                    }
                }
                e
            }).collect(),
            None => {
                let changes = ["text", "who", "speaker", "kind", "how", "note", "start", "at", "end", "duration", "length", "time", "timing", "clear_time", "remove", "shift"];
                if changes.iter().any(|k| a.get(*k).is_some()) { vec![a.clone()] } else { Vec::new() }
            }
        };
        if edits.is_empty() {
            // Read: the lines with their times.
            let lt = s.line_times();
            let pick = scene_of(a);
            let scenes: Vec<Value> = s.scenes.iter().enumerate().filter(|(_, sc)| pick.as_deref().is_none_or(|p| sc.id.eq_ignore_ascii_case(p))).map(|(si, sc)| json!({
                "scene": sc.id, "start": sc.start.map(st::times::format_time),
                "lines": sc.lines.iter().enumerate().map(|(i, l)| line_json(i, l, lt.iter().find(|t| t.scene == si && t.line == i).copied())).collect::<Vec<_>>(),
            })).collect();
            if scenes.is_empty() {
                return Err(match pick {
                    Some(p) => format!("no scene {p}: {}", s.scenes.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")),
                    None => "there is no script yet: story {do: import, text | path} or story {do: set, scenes: [{lines}]}".into(),
                });
            }
            return Ok(json!({"scenes": scenes, "runs": st::times::format_time(s.script_end()), "note": "[start-end] is the line's time; a ~ after it means worked out (from its shot or reading speed), not given. Set one with start/end/duration or time: \"0:12-0:18\"; clear_time: true lets it follow again"}));
        }
        let mut changed: Vec<String> = Vec::new();
        let mut touched: Vec<usize> = Vec::new();
        for e in &edits {
            let id = scene_of(e).or_else(|| (s.scenes.len() == 1).then(|| s.scenes[0].id.clone())).ok_or("which scene? give scene (an id)")?;
            let si = s.scenes.iter().position(|x| x.id.eq_ignore_ascii_case(&id)).ok_or_else(|| format!("no scene {id}: {}", s.scenes.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")))?;
            touched.push(si);
            let n = s.scenes[si].lines.len();
            let at_line = |v: Option<&Value>, what: &str| -> Result<Option<usize>, String> {
                match v.and_then(Value::as_u64) {
                    Some(k) if k >= 1 && k as usize <= n => Ok(Some(k as usize - 1)),
                    Some(k) => Err(format!("{what} {k}: scene {id} has {n} lines (they count from 1)")),
                    None => Ok(None),
                }
            };
            // Shift times.
            if let Some(d) = e.get("shift").and_then(Value::as_f64) {
                let from = at_line(e.get("from"), "from")?.unwrap_or(0);
                let to = at_line(e.get("to"), "to")?.unwrap_or(n.saturating_sub(1));
                let by = us(d.abs()) * if d < 0.0 { -1 } else { 1 };
                for l in s.scenes[si].lines.iter_mut().skip(from).take(to + 1 - from.min(to + 1)) {
                    if let Some(t) = l.start.as_mut() {
                        *t = (*t + by).max(0);
                    }
                }
                changed.push(format!("scene {id} times moved {d:+}s"));
                continue;
            }
            let mut o = e.as_object().cloned().unwrap_or_default();
            for k in ["do", "action", "scene", "edits"] {
                o.remove(k);
            }
            let line_no = at_line(o.remove("line").as_ref(), "line")?;
            let after = o.remove("after");
            let remove = o.remove("remove").and_then(|v| v.as_bool()).unwrap_or(false);
            let clear = o.remove("clear_time").and_then(|v| v.as_bool()).unwrap_or(false);
            if remove {
                let i = line_no.ok_or("remove needs line (its number)")?;
                s.remove_line(si, i);
                changed.push(format!("scene {id} line {} removed", i + 1));
                continue;
            }
            let (start, dur) = timing_in(&mut o)?;
            let text = o.remove("text").and_then(|v| v.as_str().map(str::to_owned));
            let who = o.remove("who").or_else(|| o.remove("speaker")).and_then(|v| v.as_str().map(|w| fountain::slug(w)));
            let kind = match o.remove("kind").and_then(|v| v.as_str().map(str::to_lowercase)).as_deref() {
                None => None,
                Some("action") => Some(st::LineKind::Action),
                Some("dialogue" | "dialog" | "speech") => Some(st::LineKind::Dialogue),
                Some("narration" | "narrator" | "voice_over" | "vo") => Some(st::LineKind::Narration),
                Some(k) => return Err(format!("line kind \"{k}\": action, dialogue or narration")),
            };
            let how = o.remove("how").and_then(|v| v.as_str().map(str::to_owned));
            let note = o.remove("note").and_then(|v| v.as_str().map(str::to_owned));
            if let Some(k) = o.keys().next() {
                return Err(format!("a line has no \"{k}\": text, who, kind, how, note, start, end, duration, time"));
            }
            let fresh = line_no.is_none();
            let mut l = match line_no {
                Some(i) => s.scenes[si].lines[i].clone(),
                None => match &text {
                    Some(t) => line_from(&Value::String(t.clone()))?,
                    None => return Err("a new line needs text (or give line, the number of the one to change)".into()),
                },
            };
            if let (Some(t), false) = (text, fresh) {
                l.text = t;
            }
            if let Some(k) = kind {
                l.kind = k;
            }
            if let Some(w) = who {
                l.who = Some(w);
                if kind.is_none() && l.kind == st::LineKind::Action {
                    l.kind = st::LineKind::Dialogue;
                }
            }
            if l.kind == st::LineKind::Action {
                l.who = None;
            } else if l.who.is_none() {
                l.who = Some("narrator".into());
            }
            if l.kind == st::LineKind::Dialogue && l.who.as_deref() == Some("narrator") {
                l.kind = st::LineKind::Narration;
            }
            if let Some(h) = how {
                l.how = (!h.is_empty()).then_some(h);
            }
            if let Some(nt) = note {
                l.note = nt;
            }
            if clear {
                l.start = None;
                l.duration = None;
            }
            if let Some(x) = start {
                l.start = x;
            }
            if let Some(x) = dur {
                l.duration = x;
            }
            if let Some(w) = l.who.clone() {
                s.know_speaker(si, &w, l.kind);
            }
            match line_no {
                Some(i) => {
                    s.scenes[si].lines[i] = l;
                    changed.push(format!("scene {id} line {}", i + 1));
                }
                None => {
                    let at = match after {
                        Some(v) => match v.as_u64() {
                            Some(k) if k as usize <= n => k as usize,
                            _ => return Err(format!("after {v}: 0 (the start) to {n} (the end) of scene {id}'s lines")),
                        },
                        None => n,
                    };
                    s.insert_line(si, at, l);
                    changed.push(format!("scene {id} line {} added", at + 1));
                }
            }
        }
        // Said times that run backwards are worth a word.
        let mut notes: Vec<String> = Vec::new();
        touched.sort_unstable();
        touched.dedup();
        for si in &touched {
            let mut prev: Option<(usize, m::Micros)> = None;
            for (i, l) in s.scenes[*si].lines.iter().enumerate() {
                if let Some(t) = l.start {
                    if let Some((pi, pt)) = prev.filter(|(_, pt)| t < *pt) {
                        notes.push(format!("scene {} line {} starts at {} but line {} starts later ({})", s.scenes[*si].id, i + 1, st::times::format_time(t), pi + 1, st::times::format_time(pt)));
                    }
                    prev = Some((i, t));
                }
            }
        }
        self.put_story(s, "Script line")?;
        let s = self.story_now();
        let lt = s.line_times();
        let shown: Vec<Value> = touched.iter().map(|si| json!({"scene": s.scenes[*si].id, "lines": s.scenes[*si].lines.iter().enumerate().map(|(i, l)| line_json(i, l, lt.iter().find(|t| t.scene == *si && t.line == i).copied())).collect::<Vec<_>>()})).collect();
        Ok(json!({"ok": true, "changed": changed, "notes": notes, "scenes": shown, "next": "story retime lays planned shots so their cuts fall on these times (plan and build do it too)"}))
    }

    fn story_remove(&mut self, a: &Value) -> Result<Value, String> {
        let mut s = self.story_now();
        let name = |k: &str| a.get(k).and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string())));
        let said = if let Some(id) = name("shot") {
            let (si, i) = s.find_shot(&id).ok_or_else(|| format!("no shot {id}"))?;
            s.scenes[si].shots.remove(i);
            format!("shot {id}")
        } else if let Some(id) = name("scene") {
            let n = s.scenes.len();
            s.scenes.retain(|x| !x.id.eq_ignore_ascii_case(&id));
            if n == s.scenes.len() {
                return Err(format!("no scene {id}"));
            }
            format!("scene {id}")
        } else if let Some(id) = name("place") {
            s.places.retain(|x| x.id != id);
            format!("place {id}")
        } else if let Some(id) = name("character") {
            s.cast.retain(|x| x.id != id);
            format!("character {id}")
        } else if let Some(id) = name("beat") {
            s.beats.retain(|x| x.id != id);
            format!("beat {id}")
        } else if a.get("all").and_then(Value::as_bool) == Some(true) {
            s = st::Story::default();
            "the whole story".into()
        } else {
            return Err("remove a shot, scene, place, character or beat (by id), or all: true".into());
        };
        self.put_story(s, "Story")?;
        Ok(json!({"removed": said, "note": "clips already built stay on the timeline"}))
    }

    fn story_plan(&mut self, a: &Value) -> Result<Value, String> {
        let mut s = self.story_now();
        let replace = a.get("replace").and_then(Value::as_bool).unwrap_or(false);
        let only: Vec<String> = a.get("scenes").and_then(Value::as_array).map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string()))).collect()).unwrap_or_default();
        let mut planned = Vec::new();
        for i in 0..s.scenes.len() {
            let pick = if only.is_empty() { replace || s.scenes[i].shots.is_empty() } else { only.iter().any(|o| o.eq_ignore_ascii_case(&s.scenes[i].id)) };
            if !pick {
                continue;
            }
            if s.scenes[i].shots.iter().any(|sh| !sh.clips.is_empty()) {
                return Err(format!("scene {} has shots on the timeline: plan it only after removing them (story build rebuild) or revise its shots with set", s.scenes[i].id));
            }
            s.plan_scene(i);
            planned.push(s.scenes[i].id.clone());
        }
        if planned.is_empty() {
            return Err("every scene already has shots (replace: true plans them again; scenes: [ids] picks some)".into());
        }
        self.put_story(s, "Plan shots")?;
        let mut out = self.story_show(&json!({}));
        out["planned"] = json!(planned);
        out["note"] = json!("a first plan: revise it. Write each shot's action and framing, move lines between shots, add inserts and POVs the story calls for, and set side and looks for every conversation");
        Ok(out)
    }

    fn story_check(&self) -> Value {
        let s = self.story_now();
        let mut issues: Vec<Value> = s.check().into_iter().map(|i| serde_json::to_value(i).unwrap_or(Value::Null)).collect();
        // The timeline against the plan.
        let built: Vec<(&st::Scene, &st::Shot)> = s.scenes.iter().flat_map(|sc| sc.shots.iter().map(move |sh| (sc, sh))).filter(|(_, sh)| sh.start.is_some()).collect();
        for w in built.windows(2) {
            let ((sa, a), (_, b)) = (w[0], w[1]);
            if b.start < a.start {
                issues.push(json!({"severity": "warning", "kind": "out_of_story_order", "scene": sa.id, "shot": b.id, "problem": format!("shot {} plays before shot {} on the timeline, but comes after it in the story", b.id, a.id), "fix": "move its clips back into order, or reorder the shots in the story if the cut is better so"}));
            }
        }
        if !built.is_empty() {
            let unbuilt: Vec<&str> = s.scenes.iter().flat_map(|sc| sc.shots.iter()).filter(|sh| sh.clips.is_empty()).map(|sh| sh.id.as_str()).collect();
            if !unbuilt.is_empty() {
                issues.push(json!({"severity": "note", "kind": "shots_not_built", "problem": format!("{} shots are not on the timeline yet: {}", unbuilt.len(), unbuilt.iter().take(12).copied().collect::<Vec<_>>().join(", ")), "fix": "story build lays them out; or stage them and story bind"}));
            }
        }
        json!({"problems": issues, "shots": s.timings().len(), "planned": sec(s.duration())})
    }

    /// Where shots start when built: after what the main lane already
    /// holds, unless `at` says.
    fn story_build(&mut self, a: &Value) -> Result<Value, String> {
        let mut s = self.story_now();
        if s.scenes.iter().all(|sc| sc.shots.is_empty()) {
            return Err("no shots to build: story plan first (or write them with set)".into());
        }
        let only: Vec<String> = a.get("scenes").and_then(Value::as_array).map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string()))).collect()).unwrap_or_default();
        let picked = |sc: &st::Scene| only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(&sc.id));
        let rebuild = a.get("rebuild").and_then(Value::as_bool).unwrap_or(false);
        let ambience = a.get("ambience").and_then(Value::as_bool).unwrap_or(true);
        let markers = a.get("markers").and_then(Value::as_bool).unwrap_or(true);
        let voices = a.get("voices").and_then(Value::as_bool).unwrap_or(false);
        // Planned shots cut where the script's times say.
        if a.get("retime").and_then(Value::as_bool) != Some(false) {
            s.retime_shots();
        }
        self.doc.begin_batch("Build from story");
        let r = self.build_shots(&mut s, &picked, rebuild, ambience, markers, voices, f(a, "at"));
        match r {
            Ok(said) => {
                if let Err(e) = self.put_story(s, "Build from story") {
                    self.doc.rollback_batch();
                    return Err(e);
                }
                self.doc.commit_batch();
                let mut out = self.story_show(&json!({}));
                out["built"] = json!(said);
                out["note"] = json!("each shot has a stand-in: its place's backdrop framed for the shot size and camera move, or a slate card. Stage the characters on it (story brief gives what each shot needs), then story bind so the shot owns its clips");
                Ok(out)
            }
            Err(e) => {
                self.doc.rollback_batch();
                Err(format!("{e}. Nothing was changed."))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_shots(&mut self, s: &mut st::Story, picked: &dyn Fn(&st::Scene) -> bool, rebuild: bool, ambience: bool, markers: bool, voices: bool, at: Option<f64>) -> Result<Vec<String>, String> {
        let spine = self.doc.project.spine().ok_or("no main lane")?;
        // Shots already there stay; a rebuild takes picked scenes' clips off.
        if rebuild {
            let mut gone: Vec<u32> = Vec::new();
            for sc in s.scenes.iter_mut().filter(|sc| picked(sc)) {
                for sh in &mut sc.shots {
                    gone.extend(sh.clips.drain(..).map(|c| c.0 as u32));
                    sh.start = None;
                }
                if let Some(Value::Array(a)) = sc.extra.remove("sound_clips") {
                    gone.extend(a.iter().filter_map(Value::as_u64).map(|c| c as u32));
                }
            }
            gone.retain(|c| self.doc.project.clip(*c).is_some());
            if !gone.is_empty() {
                self.op(&json!({"op": "delete", "clips": gone, "ripple": false}))?;
            }
        }
        let main_end = self.doc.project.clips.iter().filter(|c| c.lane == spine).map(|c| c.end()).max().unwrap_or(0);
        let mut t = match at {
            Some(x) => us(x.max(0.0)),
            None => s.scenes.iter().flat_map(|sc| sc.shots.iter()).filter_map(|sh| sh.start.map(|a| a + sh.duration)).max().unwrap_or(main_end as m::Micros).max(main_end as m::Micros),
        };
        let mut said = Vec::new();
        // The voices make every line of the scenes to build ahead, several
        // at once; the shots below then find them ready.
        if voices {
            let narrate: Vec<Value> = s
                .scenes
                .iter()
                .filter(|sc| picked(sc))
                .flat_map(|sc| sc.shots.iter().filter(|sh| sh.clips.is_empty()).flat_map(move |sh| sh.lines.iter().filter_map(move |l| sc.lines.get(*l))))
                .filter(|l| l.is_spoken())
                .map(|l| {
                    let voice = match l.who.as_deref() {
                        Some("narrator") | None => s.extra.get("narrator_voice").and_then(Value::as_str).map(str::to_owned),
                        Some(w) => s.character(w).and_then(|c| c.voice.clone()),
                    };
                    let mut op = json!({"op": "narrate", "say": l.text});
                    if let Some(v) = voice {
                        op["voice"] = json!(v);
                    }
                    op
                })
                .collect();
            self.prepare_narration(&narrate);
        }
        // Where each line plays as planned, so a spoken line lands on its
        // own time inside its shot.
        let planned = s.timings();
        let line_times = s.line_times();
        for si in 0..s.scenes.len() {
            if !picked(&s.scenes[si]) {
                continue;
            }
            let scene_start = t;
            let place = s.place_of(&s.scenes[si]).cloned();
            let backdrop = place.as_ref().and_then(|p| p.backdrop.clone()).and_then(|b| self.backdrop_media(&b));
            let mut built = 0;
            for i in 0..s.scenes[si].shots.len() {
                let sh = s.scenes[si].shots[i].clone();
                if !sh.clips.is_empty() {
                    t = sh.start.unwrap_or(t) + sh.duration;
                    continue;
                }
                let ids_before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
                let (at, dur) = (sec(t), sec(sh.duration).max(0.1));
                match backdrop {
                    Some(media) => {
                        self.op(&json!({"op": "place", "media": media, "at": at, "duration": dur, "lane": spine, "fit": "fill"}))?;
                        let id = self.fresh(&ids_before).into_iter().next().ok_or("the backdrop was not placed")?;
                        self.frame_stand_in(id, &sh, dur)?;
                    }
                    None => {
                        let shade = match sh.kind.tightness() {
                            0 | 1 => "#1d2a33",
                            2 | 3 => "#2a2433",
                            _ => "#33241f",
                        };
                        self.op(&json!({"op": "card", "colour": shade, "at": at, "duration": dur, "lane": spine}))?;
                        let words: Vec<String> = sh.lines.iter().filter_map(|l| s.scenes[si].lines.get(*l)).map(|l| excerpt(&l.text, 8)).collect();
                        let slate = format!("{} · {}{}{}", sh.id, sh.kind.name().replace('_', " ").to_uppercase(), if sh.subjects.is_empty() { String::new() } else { format!(" · {}", sh.subjects.join(" + ")) }, if sh.action.is_empty() { words.first().map(|w| format!("\n{w}")).unwrap_or_default() } else { format!("\n{}", sh.action) });
                        self.op(&json!({"op": "text", "text": slate, "at": at, "duration": dur, "size": 46, "lane": "overlay"}))?;
                    }
                }
                if voices {
                    let mut off = 0.15;
                    let shot_at = planned.iter().find(|p| p.scene == si && p.shot == i).map(|p| p.start).unwrap_or(0);
                    for (li, l) in sh.lines.iter().filter_map(|l| s.scenes[si].lines.get(*l).map(|x| (*l, x))).filter(|(_, l)| l.is_spoken()).map(|(k, l)| (k, l.clone())).collect::<Vec<_>>() {
                        // A line the writer timed speaks at its time; the rest follow each other.
                        if let Some(lt) = line_times.iter().find(|x| x.scene == si && x.line == li).filter(|x| x.source == st::TimeSource::Said) {
                            off = sec(lt.start - shot_at).max(0.0);
                        }
                        let voice = match l.who.as_deref() {
                            Some("narrator") | None => s.extra.get("narrator_voice").and_then(Value::as_str).map(str::to_owned),
                            Some(w) => s.character(w).and_then(|c| c.voice.clone()),
                        };
                        let mut op = json!({"op": "narrate", "say": l.text, "at": at + off});
                        if let Some(v) = voice {
                            op["voice"] = json!(v);
                        }
                        self.op(&op)?;
                        off += l.spoken_secs();
                    }
                }
                let fresh = self.fresh(&ids_before);
                let shot = &mut s.scenes[si].shots[i];
                shot.clips = fresh.iter().map(|c| m::ClipId(*c as u64)).collect();
                shot.start = Some(t);
                t += sh.duration;
                built += 1;
            }
            let scene_end = t;
            if built == 0 {
                continue;
            }
            let mut sound_clips: Vec<u32> = Vec::new();
            if markers {
                let heading = fountain::slug_line(s, &s.scenes[si]);
                self.op(&json!({"op": "marker", "at": sec(scene_start), "note": format!("Scene {} · {}", s.scenes[si].id, heading.trim_start_matches('.'))}))?;
            }
            if ambience {
                if let Some(bed) = place.as_ref().and_then(|p| p.ambience.clone()).filter(|b| super::synth::is_ambience(b)) {
                    let mut from = scene_start;
                    while scene_end - from > m::SECOND / 2 {
                        let len = (scene_end - from).min(120 * m::SECOND);
                        let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
                        self.op(&json!({"op": "sfx", "kind": bed, "at": sec(from), "duration": sec(len), "volume": -14, "variant": si}))?;
                        sound_clips.extend(self.fresh(&before));
                        from += len;
                    }
                }
            }
            if !sound_clips.is_empty() {
                s.scenes[si].extra.insert("sound_clips".into(), json!(sound_clips));
            }
            said.push(format!("scene {}: {built} shots {:.1}–{:.1}s", s.scenes[si].id, sec(scene_start), sec(scene_end)));
        }
        if said.is_empty() {
            return Err("every picked shot is already on the timeline (rebuild: true lays them again)".into());
        }
        Ok(said)
    }

    fn fresh(&self, before: &std::collections::BTreeSet<u32>) -> Vec<u32> {
        let mut v: Vec<u32> = self.doc.project.clips.iter().map(|c| c.id).filter(|c| !before.contains(c)).collect();
        v.sort_unstable();
        v
    }

    /// A media item in the bin for a place's backdrop (its name or file),
    /// importing the file when it exists and is not in yet.
    fn backdrop_media(&mut self, b: &str) -> Option<u32> {
        if let Ok(id) = super::craft::media_of(&self.doc.project, &json!(b)) {
            return Some(id);
        }
        let p = path_of(b);
        if p.is_file() {
            let r = crate::services::project::import_media(&mut self.doc, &[p]);
            return r.ids.first().copied();
        }
        None
    }

    /// Frames a backdrop for the shot size (tighter shots crop in) and
    /// moves it as the camera moves.
    fn frame_stand_in(&mut self, id: u32, sh: &st::Shot, dur: f64) -> Result<(), String> {
        use st::CameraMove::*;
        let base = match sh.kind.tightness() {
            0 | 1 => 100.0,
            2 => 115.0,
            3 => 130.0,
            4 => 160.0,
            _ => 200.0,
        };
        // Keys are in the clip's own time.
        let k = |a: f64, b: f64| json!([[0.0, a], [dur, b]]);
        let ops: Vec<Value> = match sh.camera {
            PushIn | Zoom => vec![json!({"op": "animate", "clip": id, "property": "scale", "keys": k(base, base * 1.1), "ease": "smooth"})],
            PullOut => vec![json!({"op": "animate", "clip": id, "property": "scale", "keys": k(base * 1.12, base), "ease": "smooth"})],
            Pan | Track => vec![json!({"op": "set", "clip": id, "scale": base.max(115.0)}), json!({"op": "animate", "clip": id, "property": "x", "keys": k(60.0, -60.0), "ease": "linear"})],
            Tilt => vec![json!({"op": "set", "clip": id, "scale": base.max(115.0)}), json!({"op": "animate", "clip": id, "property": "y", "keys": k(-40.0, 40.0), "ease": "smooth"})],
            Crane => vec![json!({"op": "animate", "clip": id, "property": "scale", "keys": k(base * 1.08, base), "ease": "smooth"}), json!({"op": "animate", "clip": id, "property": "y", "keys": k(40.0, -20.0), "ease": "smooth"})],
            Static | Handheld if (base - 100.0).abs() > 0.1 => vec![json!({"op": "set", "clip": id, "scale": base})],
            _ => Vec::new(),
        };
        for op in ops {
            self.op(&op)?;
        }
        Ok(())
    }

    fn story_bind(&mut self, a: &Value) -> Result<Value, String> {
        let mut s = self.story_now();
        let sid = a.get("shot").and_then(Value::as_str).ok_or("bind needs \"shot\" (its id) and \"clips\", or from / to (every picture clip that starts in that range)")?;
        let (si, i) = s.find_shot(sid).ok_or_else(|| format!("no shot {sid}"))?;
        let ids: Vec<u32> = match a.get("clips") {
            Some(Value::Array(l)) => l.iter().filter_map(Value::as_u64).map(|c| c as u32).collect(),
            Some(v) => v.as_u64().map(|c| vec![c as u32]).unwrap_or_default(),
            None => {
                let t = s.timings().into_iter().find(|t| t.scene == si && t.shot == i).ok_or("shot gone")?;
                let from = f(a, "from").map(micros).unwrap_or(t.start as view::Time);
                let to = f(a, "to").map(micros).unwrap_or((t.start + t.duration) as view::Time);
                let p = &self.doc.project;
                p.clips.iter().filter(|c| c.start >= from - 1000 && c.start < to && p.lane(c.lane).is_some_and(|l| l.kind != LaneKind::Audio)).map(|c| c.id).collect()
            }
        };
        if let Some(bad) = ids.iter().find(|c| self.doc.project.clip(**c).is_none()) {
            return Err(format!("no clip {bad}"));
        }
        if ids.is_empty() {
            return Err(format!("no picture clips start inside shot {sid}'s time: pass clips"));
        }
        // A clip belongs to one shot.
        for sc in &mut s.scenes {
            for sh in &mut sc.shots {
                sh.clips.retain(|c| !ids.contains(&(c.0 as u32)));
            }
        }
        let add = a.get("add").and_then(Value::as_bool).unwrap_or(false);
        let shot = &mut s.scenes[si].shots[i];
        if !add {
            shot.clips.clear();
        }
        shot.clips.extend(ids.iter().map(|c| m::ClipId(*c as u64)));
        if let Some(seq) = self.doc.model.sequence(self.doc.seq()) {
            s.sync(seq);
        }
        let (si, i) = s.find_shot(sid).unwrap();
        let line = shot_line(&s.scenes[si].shots[i], s.scenes[si].shots[i].start.unwrap_or(0));
        self.put_story(s, "Bind shot")?;
        Ok(json!({"shot": line}))
    }

    /// What staging shot `shot` needs: the place, who is in frame with
    /// their look, size and moves, the lines with voices, and what the
    /// shots either side did (side of the line, eyelines).
    fn story_brief(&self, a: &Value) -> Result<Value, String> {
        let s = self.story_now();
        let sid = a.get("shot").and_then(Value::as_str).ok_or("brief needs \"shot\" (an id from story show)")?;
        let (si, i) = s.find_shot(sid).ok_or_else(|| format!("no shot {sid}"))?;
        let sc = &s.scenes[si];
        let sh = &sc.shots[i];
        let t = s.timings().into_iter().find(|t| t.scene == si && t.shot == i).ok_or("shot gone")?;
        let (share, framing) = framing_guide(sh.kind);
        let h = self.doc.project.height as f64;
        // Sizes against the main subject.
        let main = sh.subjects.first().and_then(|w| s.character(w));
        let main_size = main.map(|c| c.size).unwrap_or(1.0).max(0.05);
        let cast: Vec<Value> = sh
            .subjects
            .iter()
            .filter_map(|w| s.character(w))
            .map(|c| {
                let tall = if share > 0.0 { (h * share * c.size / main_size).round() } else { 0.0 };
                json!({
                    "id": c.id, "name": c.name, "look": c.look, "references": c.references, "cast_item": c.cast_item, "voice": c.voice,
                    "size": c.size, "stands_px": if tall > 0.0 { json!(tall) } else { Value::Null },
                    "personality": c.personality, "moves": c.moves, "facing": c.facing.map(|f| f.name()), "notes": c.notes,
                })
            })
            .collect();
        let others: Vec<&str> = sh.subjects.iter().filter(|w| s.character(w).is_none()).map(String::as_str).collect();
        let place = s.place_of(sc).map(|p| {
            json!({"id": p.id, "name": p.name, "interior": p.interior, "backdrop": p.backdrop, "in_bin": p.backdrop.as_ref().and_then(|b| super::craft::media_of(&self.doc.project, &json!(b)).ok()), "light": p.light, "time_of_day": sc.time_of_day.or(p.time_of_day).map(|t| t.name()), "ambience": p.ambience, "surface": p.surface, "notes": p.notes})
        });
        let lines: Vec<Value> = sh
            .lines
            .iter()
            .filter_map(|l| sc.lines.get(*l).map(|x| (l, x)))
            .map(|(n, l)| {
                let voice = match l.who.as_deref() {
                    Some("narrator") | None if l.is_spoken() => s.extra.get("narrator_voice").and_then(Value::as_str).map(str::to_owned),
                    Some(w) => s.character(w).and_then(|c| c.voice.clone()),
                    _ => None,
                };
                json!({"n": n + 1, "line": line_json(*n, l, None), "voice": voice, "takes": (l.spoken_secs() * 10.0).round() / 10.0})
            })
            .collect();
        let near = |d: isize| -> Value {
            let j = i as isize + d;
            if j < 0 || j as usize >= sc.shots.len() {
                return Value::Null;
            }
            let o = &sc.shots[j as usize];
            json!(shot_line(o, s.timings().into_iter().find(|t| t.scene == si && t.shot == j as usize).map(|t| t.start).unwrap_or(0)))
        };
        let mut continuity: Vec<String> = Vec::new();
        if let Some(side) = &sh.side {
            continuity.push(format!("camera on side {side} of the line: keep each character on the side of the frame they held in the shots before"));
        }
        if let Some(l) = sh.looks {
            continuity.push(format!("{} looks frame {} (leave room on that side)", sh.subjects.first().map(String::as_str).unwrap_or("the subject"), l.name()));
        }
        for c in sh.subjects.iter().filter_map(|w| s.character(w)) {
            if !c.moves.is_empty() {
                continue;
            }
            continuity.push(format!("{} has no moves in the bible: decide how they move (their acts) and write it down so every shot agrees", c.id));
        }
        if let Some(c) = main {
            if !c.references.is_empty() || c.cast_item.is_some() {
                continuity.push(format!("draw {} from their reference / cast item every time: never a new picture of them", c.id));
            }
        }
        Ok(json!({
            "shot": shot_json(sc, sh, t.start), "scene": {"id": sc.id, "heading": fountain::slug_line(&s, sc), "summary": sc.summary, "beat": sc.beat},
            "timeline": {"from": sec(t.start), "to": sec(t.start + t.duration)},
            "place": place, "in_frame": cast, "things": others, "lines": lines,
            "framing": framing, "camera": camera_guide(sh.camera),
            "before": near(-1), "after": near(1), "continuity": continuity,
            "then": format!("stage it on the timeline from {:.2}s to {:.2}s, look at it (frame), then story bind {{shot: \"{}\"}}", sec(t.start), sec(t.start + t.duration), sh.id),
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::Session;
    use crate::services;
    use serde_json::{Value, json};

    fn session() -> Session {
        Session::new(services::project::new_custom("Story", 640, 360, 30.0))
    }

    fn call(s: &mut Session, a: Value) -> Value {
        match s.call("story", &a).unwrap_or_else(|e| panic!("story {a}: {e}")).into_iter().next() {
            Some(super::super::Content::Text(t)) => serde_json::from_str(&t).unwrap(),
            _ => panic!("no text"),
        }
    }

    const SCRIPT: &str = "Title: River\n\nEXT. RIVER BANK - DAY\n\nNARRATOR (V.O.)\nFern the fox walked down to the water.\n\nFERN\nIs anyone there?\n\nHERON\nOnly me!\n\nINT. DEN - NIGHT\n\nNARRATOR (V.O.)\nThat night she dreamed of the other side.\n";

    #[test]
    fn import_plan_revise_build_and_read_back() {
        let mut s = session();
        let r = call(&mut s, json!({"do": "import", "text": SCRIPT, "plan": true}));
        assert_eq!(r["scenes"].as_array().unwrap().len(), 2, "{r}");
        assert!(s.doc.model.story.scenes.iter().all(|sc| !sc.shots.is_empty()));
        // The bible.
        let dir = std::env::temp_dir().join(format!("studio-story-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("river.png");
        let (w, h) = (64u32, 36u32);
        let pixels: Vec<u8> = (0..w * h).flat_map(|i| [40, 90 + (i % w) as u8, 160, 255]).collect();
        std::fs::write(&png, studio_media::png::encode(&studio_media::rgba::RgbaImage { width: w, height: h, pixels })).unwrap();
        let r = call(&mut s, json!({"do": "set", "places": [{"id": "river_bank", "backdrop": png.display().to_string(), "ambience": "stream", "surface": "grass", "light": "high sun", "time_of_day": "noon"}], "cast": [{"id": "fern", "look": "small red fox", "voice": "Samantha", "moves": ["walk", "hop"], "facing": "right"}, {"id": "heron", "look": "tall grey heron", "size": 1.6}]}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(s.doc.model.story.place("river_bank").unwrap().time_of_day, Some(studio_model::TimeOfDay::Day));
        assert!(s.call("story", &json!({"do": "set", "cast": [{"id": "fern", "moves": ["moonwalk"]}]})).is_err());
        // Revise one shot; add one.
        let first = s.doc.model.story.scenes[0].shots[0].id.clone();
        call(&mut s, json!({"do": "set", "shots": [{"id": first, "action": "the river in sun; Fern enters frame left", "duration": 4.0}]}));
        call(&mut s, json!({"do": "set", "scenes": [{"id": "1", "shots": [{"kind": "insert", "subjects": "water", "duration": 1.5, "after": first, "action": "ripples"}]}]}));
        assert_eq!(s.doc.model.story.scenes[0].shots[1].kind, studio_model::ShotKind::Insert);
        assert_eq!(s.doc.model.story.scenes[0].shots[0].duration, 4_000_000);
        // Build: one undo step, stand-ins, markers and the river's bed.
        let before = s.doc.history.undo_len();
        let r = call(&mut s, json!({"do": "build"}));
        assert!(r["built"].as_array().is_some_and(|b| b.len() == 2), "{r}");
        let st = s.story_now();
        assert!(st.scenes.iter().flat_map(|sc| sc.shots.iter()).all(|sh| !sh.clips.is_empty() && sh.start.is_some()));
        assert_eq!(s.doc.project.markers.len(), 2);
        assert!(st.scenes[0].extra.get("sound_clips").is_some(), "the stream bed was laid");
        assert_eq!(st.timings()[1].start, 4_000_000);
        // The river's shots stand on its picture, framed and moved per shot.
        let first_clip = s.doc.project.clip(st.scenes[0].shots[0].clips[0].0 as u32).unwrap().clone();
        assert!(first_clip.media.is_some(), "the backdrop, not a slate");
        let moving = st.scenes[0].shots.iter().filter(|sh| sh.camera != studio_model::CameraMove::Static).count();
        let keyed = st.scenes[0].shots.iter().filter(|sh| s.doc.project.clip(sh.clips[0].0 as u32).is_some_and(|c| !c.keys.is_empty())).count();
        assert_eq!(keyed, moving, "each moving shot's backdrop is keyed");
        let _ = std::fs::remove_dir_all(&dir);
        // Trim a built shot on the timeline: the story follows.
        let c = st.scenes[0].shots[0].clips[0].0 as u32;
        s.edit(&[json!({"op": "set", "clip": c, "duration": 3.0})], "x", false).unwrap();
        assert_eq!(s.story_now().scenes[0].shots[0].duration, 3_000_000);
        // And the story drives the timeline: a new length trims the clips.
        let r = call(&mut s, json!({"do": "set", "shots": [{"id": first, "duration": 2.5}]}));
        assert!(r["notes"].to_string().contains("trimmed"), "{r}");
        assert!((s.doc.project.clip(c).unwrap().dur as f64 / 1e6 - 2.5).abs() < 0.05);
        // Undo goes back through the steps; the build is one.
        // The build is one step; then the trim and the story's own.
        assert_eq!(s.doc.history.undo_len() - before, 3);
        while s.doc.history.undo_len() > before {
            s.doc.undo();
        }
        assert!(s.story_now().scenes.iter().flat_map(|sc| sc.shots.iter()).all(|sh| sh.clips.is_empty()));
        assert!(s.doc.project.markers.is_empty());
    }

    #[test]
    fn brief_check_bind_and_export() {
        let mut s = session();
        call(&mut s, json!({"do": "import", "text": SCRIPT}));
        call(&mut s, json!({"do": "set", "cast": [{"id": "fern", "look": "small red fox", "voice": "Samantha", "size": 1.0}, {"id": "heron", "look": "tall grey heron", "size": 1.6, "voice": "Daniel"}],
            "scenes": [{"id": "1", "shots": [
                {"id": "1A", "kind": "establishing", "lines": [1], "duration": 4},
                {"id": "1B", "kind": "close-up", "subjects": ["fern"], "lines": [2], "duration": 2, "side": "a", "looks": "right"},
                {"id": "1C", "kind": "cu", "subjects": "heron", "lines": "3", "duration": 1.5, "side": "b", "looks": "right"}]},
                {"id": "2", "shots": [{"id": "2A", "kind": "wide", "lines": [1], "duration": 4}]}]}));
        let r = call(&mut s, json!({"do": "check"}));
        let kinds: Vec<&str> = r["problems"].as_array().unwrap().iter().filter_map(|p| p["kind"].as_str()).collect();
        assert!(kinds.contains(&"crossed_line") && kinds.contains(&"eyelines_miss"), "{kinds:?}");
        let b = call(&mut s, json!({"do": "brief", "shot": "1C"}));
        assert_eq!(b["in_frame"][0]["voice"], "Daniel", "{b}");
        assert_eq!(b["lines"][0]["voice"], "Daniel");
        assert!(b["before"].as_str().unwrap().starts_with("1B"));
        assert!(b["framing"].as_str().unwrap().contains("face"));
        // A shot of both: the heron stands 1.6 times the fox.
        call(&mut s, json!({"do": "set", "shots": [{"id": "1A", "subjects": ["fern", "heron"], "kind": "wide"}]}));
        let b = call(&mut s, json!({"do": "brief", "shot": "1A"}));
        let (f, h) = (b["in_frame"][0]["stands_px"].as_f64().unwrap(), b["in_frame"][1]["stands_px"].as_f64().unwrap());
        assert!((h / f - 1.6).abs() < 0.01, "{b}");
        // Bind clips staged by hand.
        let r = s.edit(&[json!({"op": "card", "colour": "#203040", "at": 0, "duration": 4})], "x", false).unwrap();
        let id = r["changed"]["added"][0]["id"].as_u64().unwrap();
        let r = call(&mut s, json!({"do": "bind", "shot": "1A"}));
        assert!(r["shot"].as_str().unwrap().contains("[built]"), "{r}");
        assert_eq!(s.story_now().scenes[0].shots[0].clips[0].0, id);
        // Export and read back.
        let r = call(&mut s, json!({"do": "export", "format": "fountain"}));
        let text = r["text"].as_str().unwrap();
        assert!(text.contains("[[SHOT 1C CU static 1.50s L3 @heron side:b looks:right]]"), "{text}");
        let back = studio_model::story::fountain::read(text);
        assert_eq!(back.scenes[0].shots.len(), 3);
        assert_eq!(back.character("heron").unwrap().size, 1.6);
        let shots = call(&mut s, json!({"do": "export", "format": "shots"}));
        assert!(shots["text"].as_str().unwrap().contains("1B"));
        assert!(s.call("story", &json!({"do": "import", "text": SCRIPT})).is_err(), "a second import needs replace");
    }

    #[test]
    fn the_ai_reads_and_writes_the_scripts_times() {
        let mut s = session();
        let text = "[0:00-0:04] The fox woke early.\n\n[0:06] Fern: Is anyone there?\n\nThe heron answered slowly from the reeds.\n";
        call(&mut s, json!({"do": "import", "text": text, "plan": true}));
        let r = call(&mut s, json!({"do": "line"}));
        let l = |r: &Value, sc: usize| r["scenes"][sc]["lines"][0].as_str().unwrap().to_owned();
        assert!(l(&r, 0).starts_with("1 [0:00-0:04] NARRATOR"), "{r}");
        assert!(l(&r, 1).contains("[0:06") && l(&r, 1).contains("FERN"), "{r}");
        assert!(l(&r, 2).contains("]~"), "a worked-out time is marked: {r}");
        // The shots were cut where the lines say.
        let st = s.story_now();
        assert!(st.timings().iter().any(|t| t.start == 6_000_000), "{}", st.shot_list());
        // Set a time three ways; a bad one is refused.
        let r = call(&mut s, json!({"do": "line", "scene": "3", "line": 1, "start": "0:10", "end": 13}));
        assert!(r["scenes"][0]["lines"][0].as_str().unwrap().contains("[0:10-0:13] NARRATOR"), "{r}");
        let r = call(&mut s, json!({"do": "line", "scene": "3", "line": 1, "time": "0:11-0:14", "note": "slow"}));
        assert!(r["scenes"][0]["lines"][0].as_str().unwrap().contains("[0:11-0:14]") && r["scenes"][0]["lines"][0].as_str().unwrap().contains("[note: slow]"), "{r}");
        assert!(s.call("story", &json!({"do": "line", "scene": "3", "line": 1, "start": "soon"})).is_err());
        assert!(s.call("story", &json!({"do": "line", "scene": "3", "line": 9, "text": "x"})).is_err());
        // Add a line from a new speaker after line 1; the speaker joins the cast.
        let r = call(&mut s, json!({"do": "line", "scene": "3", "text": "HERON: Only me.", "after": 1, "duration": 2}));
        assert!(r["scenes"][0]["lines"][1].as_str().unwrap().contains("HERON: Only me."), "{r}");
        assert!(s.story_now().character("heron").is_some());
        // Several edits in one call; a shift moves only the given times.
        call(&mut s, json!({"do": "line", "edits": [{"scene": "1", "line": 1, "start": 1}, {"scene": "2", "line": 1, "text": "Is anyone out there?"}]}));
        call(&mut s, json!({"do": "line", "scene": "1", "shift": 2.5}));
        assert_eq!(s.story_now().scenes[0].lines[0].start, Some(3_500_000));
        assert_eq!(s.story_now().scenes[1].lines[0].start, Some(6_000_000));
        call(&mut s, json!({"do": "line", "scene": "3", "line": 2, "remove": true}));
        assert_eq!(s.story_now().scenes[2].lines.len(), 1);
        // A line the writer did not time follows the timeline once built.
        call(&mut s, json!({"do": "line", "scene": "2", "line": 1, "clear_time": true}));
        let r = call(&mut s, json!({"do": "build"}));
        assert!(r["built"].as_array().is_some(), "{r}");
        let before = s.story_now().line_time(1, 0).unwrap();
        assert_eq!(before.source, studio_model::story::TimeSource::Shot);
        let clips: Vec<u64> = s.story_now().scenes[1].shots.iter().flat_map(|sh| sh.clips.iter().map(|c| c.0)).collect();
        s.edit(&[json!({"op": "move", "clips": clips, "by": 5.0})], "x", false).unwrap();
        let after = s.story_now().line_time(1, 0).unwrap();
        assert_eq!(after.start, before.start + 5_000_000, "the line went with its clips");
        // Export as subtitles and as a script.
        let r = call(&mut s, json!({"do": "export", "format": "srt"}));
        assert!(r["text"].as_str().unwrap().contains("-->"), "{r}");
        let r = call(&mut s, json!({"do": "export", "format": "script"}));
        assert!(!r["text"].as_str().unwrap().contains("[[SHOT"), "{r}");
        // An imported subtitle file keeps its times; merge keeps the shots.
        let r = call(&mut s, json!({"do": "import", "text": "1\n00:00:01,000 --> 00:00:02,000\nHello.\n", "format": "srt", "replace": true}));
        assert!(r["script"].as_str().unwrap().contains("1 lines (1 with a given start"), "{r}");
    }
}
