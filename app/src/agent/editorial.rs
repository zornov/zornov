// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Core editing the way film editors cut, as ops and one look-up tool, on
//! `studio_model::editorial`:
//!
//! - source to record: `insert`, `overwrite`, `fit_to_fill`, `replace`
//!   with marks (`source_edit` takes `mode`), three or four points, picture
//!   and sound patched to lanes;
//! - trimming: `ripple` (sync locks kept), `trim_sides` (asymmetric and
//!   multi-lane trims), `extend`, `j_cut` / `l_cut` / `split_edit`;
//! - ranges: `lift`, `extract`; lanes: `sync_lock`, `target`;
//! - organising: `sequence` (new, duplicate, switch, rename, delete, nest),
//!   `string_out`, `selects`, `take` (scene / take / circled), `speed_ramp`,
//!   `text_cut` (cut a clip by taking words out of its transcript);
//! - the `editorial` tool: match frame, reverse match frame, gaps,
//!   duplicate frames, the timeline index, sequences, lanes, takes, a
//!   clip's transcript, and the takes that say a script line.

use super::craft::{lane_of, media_of};
use super::{Session, f, micros, secs};
use crate::view;
use serde_json::{Value, json};
use studio_model as m;
use studio_model::editorial as ed;

/// The ops this module answers.
pub(super) fn is_op(name: &str) -> bool {
    op_fields(name).is_some() && !matches!(name, "marker" | "replace" | "move")
}

/// The fields of this module's ops (and the ones it widens).
pub(super) fn op_fields(op: &str) -> Option<&'static [&'static str]> {
    const SOURCE: &[&str] = &["mode", "media", "sequence", "src_in", "src_out", "at", "rec_out", "lane", "audio_lane", "audio", "video", "clip"];
    Some(match op {
        "source_edit" | "insert" | "overwrite" | "fit_to_fill" | "replace" => SOURCE,
        "ripple" => &["clip", "edge", "by"],
        "trim_sides" => &["sides", "by"],
        "extend" => &["clip", "edge", "to"],
        "j_cut" | "l_cut" => &["clip", "by"],
        "split_edit" => &["clip", "by", "part"],
        "lift" | "extract" => &["from", "to", "lanes"],
        "sync_lock" | "target" => &["lane", "lanes", "on"],
        "sequence" => &["action", "name", "sequence", "clips", "open"],
        "speed_ramp" => &["clip", "points", "speeds"],
        "string_out" => &["media", "scene", "circled", "name", "gap", "open"],
        "selects" => &["takes", "name", "gap", "open"],
        "take" => &["media", "scene", "take", "camera", "circled", "note", "rating", "auto"],
        "text_cut" => &["clip", "remove", "keep", "lines", "ranges"],
        "marker" => &["at", "note", "remove", "move", "name", "colour", "color", "duration", "clip"],
        "nudge" => &["clips", "by", "linked", "lane"],
        "move" => &["clips", "by", "to", "lane", "linked"],
        _ => return None,
    })
}

fn edge_of(op: &Value, name: &str) -> Result<m::Edge, String> {
    match op.get("edge").and_then(Value::as_str).unwrap_or("") {
        "end" | "out" | "tail" => Ok(m::Edge::End),
        "start" | "in" | "head" => Ok(m::Edge::Start),
        "" => Err(format!("{name} needs \"edge\": \"start\" or \"end\"")),
        e => Err(format!("edge \"{e}\": start or end")),
    }
}

fn err(e: m::EditError) -> String {
    match e {
        m::EditError::Locked(_) => "that lane is locked".into(),
        m::EditError::NoHandles(s) => format!("not enough media: {s}"),
        m::EditError::Invalid(s) => s,
        m::EditError::NotFound(s) => format!("{s} not found"),
        m::EditError::Invariant(v) => format!("not possible: {v}"),
    }
}

/// Lower-case words without punctuation.
fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect()
}

/// A line of speech in a file: seconds into the file, and its words with
/// their own times.
#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    pub from: f64,
    pub to: f64,
    pub text: String,
    pub words: Vec<Word>,
}

/// A spoken word, seconds into the file; `norm` is lower case without
/// punctuation.
#[derive(Clone, Debug, Default)]
pub(crate) struct Word {
    pub norm: String,
    pub from: f64,
    pub to: f64,
}

impl Line {
    /// A line whose words share its time by their length (a transcript
    /// with line times only).
    pub(crate) fn spread(from: f64, to: f64, text: &str) -> Self {
        let raw: Vec<&str> = text.split_whitespace().collect();
        let total: f64 = raw.iter().map(|w| w.chars().count() as f64 + 1.0).sum::<f64>().max(1.0);
        let mut at = from;
        let words = raw
            .iter()
            .map(|w| {
                let len = (to - from) * (w.chars().count() as f64 + 1.0) / total;
                let word = Word { norm: norm(w), from: at, to: at + len };
                at += len;
                word
            })
            .filter(|w| !w.norm.is_empty())
            .collect();
        Self { from, to, text: text.to_owned(), words }
    }
}

fn norm(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

type Store = std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Arc<Vec<Line>>>>;

fn store() -> &'static Store {
    static S: std::sync::OnceLock<Store> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Takes a transcript for a file (tests give theirs this way).
#[cfg(test)]
pub(crate) fn remember(path: &std::path::Path, lines: Vec<Line>) {
    store().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), std::sync::Arc::new(lines));
}

/// What a file says, heard once with word times (Whisper) and kept for
/// the session.
pub(crate) fn transcript(path: &std::path::Path) -> Result<std::sync::Arc<Vec<Line>>, String> {
    if let Some(t) = store().lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return Ok(t.clone());
    }
    let lines = hear(path)?;
    let lines = std::sync::Arc::new(lines);
    store().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), lines.clone());
    Ok(lines)
}

fn hear(path: &std::path::Path) -> Result<Vec<Line>, String> {
    use studio_ai::features::transcribe::{TranscribeOptions, Whisper};
    let d = crate::sound_bridge::decoded(&path.to_string_lossy()).ok_or("no sound we can read in that file")?;
    let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
    let whisper = Whisper::load_best(&studio_ai::store::ModelStore::default()).map_err(|e| format!("speech recognition is not available: {e}"))?;
    let tr = whisper.transcribe(&mono, d.rate, &TranscribeOptions::default(), &studio_ai::jobs::JobCtx::detached()).map_err(|e| e.to_string())?;
    Ok(tr
        .segments
        .iter()
        .map(|sg| {
            let words: Vec<Word> = sg.words.iter().map(|w| Word { norm: norm(&w.text), from: w.start, to: w.end }).filter(|w| !w.norm.is_empty()).collect();
            if words.is_empty() { Line::spread(sg.start, sg.end, sg.text.trim()) } else { Line { from: sg.start, to: sg.end, text: sg.text.trim().to_owned(), words } }
        })
        .collect())
}

/// Longest common subsequence of two word lists (how much of a script
/// line a take says, in order).
fn lcs(a: &[String], b: &[String]) -> usize {
    let mut row = vec![0usize; b.len() + 1];
    for x in a {
        let mut prev = 0;
        for (j, y) in b.iter().enumerate() {
            let keep = row[j + 1];
            row[j + 1] = if x == y { prev + 1 } else { row[j + 1].max(row[j]) };
            prev = keep;
        }
    }
    row[b.len()]
}

fn take_label(info: &ed::TakeInfo) -> String {
    let mut s = String::new();
    if let Some(sc) = &info.scene {
        s.push_str(&format!("Sc {sc}"));
    }
    if let Some(tk) = &info.take {
        s.push_str(&format!("{}Tk {tk}", if s.is_empty() { "" } else { " " }));
    }
    if let Some(c) = &info.camera {
        s.push_str(&format!(" Cam {c}"));
    }
    if info.circled {
        s.push_str(" (circled)");
    }
    s.trim().to_owned()
}

/// Natural order for scene and take numbers (2 before 10, 12 before 12A).
fn natural(s: &Option<String>) -> (u64, String) {
    let v = s.clone().unwrap_or_default();
    let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
    (digits.parse().unwrap_or(u64::MAX), v)
}

impl Session {
    fn lane_arg(&self, v: &Value) -> Result<m::LaneId, String> {
        Ok(m::LaneId(lane_of(&self.doc.project, v, None)? as u64))
    }

    fn lanes_arg(&self, op: &Value) -> Result<Option<Vec<m::LaneId>>, String> {
        let list: Vec<Value> = match (op.get("lanes"), op.get("lane")) {
            (Some(Value::Array(a)), _) => a.clone(),
            (Some(Value::String(s)), _) if s == "all" => return Ok(None),
            (Some(v), _) | (None, Some(v)) => vec![v.clone()],
            (None, None) => return Ok(None),
        };
        list.iter().map(|v| self.lane_arg(v)).collect::<Result<Vec<_>, _>>().map(Some)
    }

    fn sequence_arg(&self, v: &Value) -> Result<m::SequenceId, String> {
        let model = &self.doc.model;
        if let Some(id) = v.as_u64() {
            return model.sequence(m::SequenceId(id)).map(|s| s.id).ok_or_else(|| format!("no sequence {id}"));
        }
        let want = v.as_str().ok_or("a sequence is an id or a name")?.trim().to_lowercase();
        model
            .sequences
            .iter()
            .find(|s| s.name.to_lowercase() == want)
            .or_else(|| {
                let hits: Vec<&m::Sequence> = model.sequences.iter().filter(|s| s.name.to_lowercase().contains(&want)).collect();
                (hits.len() == 1).then(|| hits[0])
            })
            .map(|s| s.id)
            .ok_or_else(|| format!("no sequence \"{want}\"; sequences: {}", model.sequences.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")))
    }

    fn plan(&mut self, label: &str, r: Result<m::Edit, m::EditError>, what: String) -> Result<String, String> {
        let e = r.map_err(err)?;
        let ok = self.doc.apply(label, e);
        self.check(ok, &what)
    }

    fn model_media(&self, op: &Value) -> Result<m::MediaId, String> {
        Ok(m::MediaId(media_of(&self.doc.project, op.get("media").ok_or("needs \"media\"")?)? as u64))
    }

    /// The first audio lane, made when there is none.
    fn audio_lane(&mut self) -> Result<m::LaneId, String> {
        if let Some(l) = self.doc.model.active().lanes.iter().find(|l| l.kind == m::LaneKind::Audio && !l.locked) {
            return Ok(l.id);
        }
        let id = m::LaneId(self.doc.alloc_id());
        let seq = self.doc.seq();
        let ok = self.doc.apply("Lane", m::Edit::AddLane { seq, lane: m::Lane::new(id, m::LaneKind::Audio, "Audio 1"), index: None });
        self.check(ok, "lane")?;
        Ok(id)
    }

    /// Runs one of this module's ops.
    pub(super) fn editorial(&mut self, name: &str, op: &Value) -> Result<String, String> {
        let seq = self.doc.seq();
        let clip = || op.get("clip").and_then(Value::as_u64).map(m::ClipId).ok_or_else(|| format!("{name} needs \"clip\""));
        let by = || f(op, "by").map(micros).ok_or_else(|| format!("{name} needs \"by\" (seconds)"));
        match name {
            "source_edit" | "insert" | "overwrite" | "fit_to_fill" => self.source_op(name, op),
            "ripple" => {
                let (c, edge, d) = (clip()?, edge_of(op, name)?, by()?);
                let r = ed::ripple_trim(&mut self.doc.model, seq, c, edge, d);
                self.plan("Ripple trim", r, format!("ripple-trimmed clip {} {} by {}s", c.0, if edge == m::Edge::End { "end" } else { "start" }, secs(d)))
            }
            "trim_sides" => {
                let d = by()?;
                let list = op.get("sides").and_then(Value::as_array).ok_or("trim_sides needs \"sides\": [{clip, edge, kind}]")?;
                let mut sides = Vec::new();
                for s in list {
                    let c = s.get("clip").and_then(Value::as_u64).ok_or("each side needs \"clip\" (an id)")?;
                    let kind = match s.get("kind").and_then(Value::as_str).unwrap_or("ripple") {
                        "ripple" => ed::TrimKind::Ripple,
                        "roll" => ed::TrimKind::Roll,
                        "plain" | "trim" => ed::TrimKind::Plain,
                        k => return Err(format!("side kind \"{k}\": ripple, roll or plain")),
                    };
                    sides.push(ed::TrimSide { clip: m::ClipId(c), edge: edge_of(s, "each side")?, kind });
                }
                let r = ed::trim_sides(&mut self.doc.model, seq, &sides, d);
                self.plan("Trim", r, format!("trimmed {} sides by {}s", sides.len(), secs(d)))
            }
            "extend" => {
                let (c, edge) = (clip()?, edge_of(op, name)?);
                let to = micros(f(op, "to").ok_or("extend needs \"to\" (timeline seconds)")?);
                let r = ed::extend(&mut self.doc.model, seq, c, edge, to);
                self.plan("Extend edit", r, format!("extended the cut to {}s", secs(to)))
            }
            "j_cut" | "l_cut" | "split_edit" => {
                let c = clip()?;
                let d = by()?.abs();
                let (delta, part) = match name {
                    "j_cut" => (-d, ed::Part::Audio),
                    "l_cut" => (d, ed::Part::Audio),
                    _ => {
                        let part = match op.get("part").and_then(Value::as_str).unwrap_or("audio") {
                            "audio" | "sound" => ed::Part::Audio,
                            "video" | "picture" => ed::Part::Video,
                            p => return Err(format!("part \"{p}\": audio or video")),
                        };
                        (by()?, part)
                    }
                };
                let r = ed::split_edit(&mut self.doc.model, seq, c, delta, part);
                let what = match (name, part) {
                    ("j_cut", _) => format!("J cut: the next shot is heard {}s early", secs(d)),
                    ("l_cut", _) => format!("L cut: clip {}'s sound runs {}s under the next shot", c.0, secs(d)),
                    (_, ed::Part::Audio) => format!("sound cut moved {}s", secs(delta)),
                    _ => format!("picture cut moved {}s", secs(delta)),
                };
                self.plan("Split edit", r, what)
            }
            "lift" | "extract" => {
                let a = micros(f(op, "from").ok_or_else(|| format!("{name} needs \"from\" and \"to\" (timeline seconds)"))?);
                let b = micros(f(op, "to").ok_or_else(|| format!("{name} needs \"to\""))?);
                let lanes = self.lanes_arg(op)?;
                let r = if name == "lift" { ed::lift(&mut self.doc.model, seq, a, b, lanes.as_deref()) } else { ed::extract(&mut self.doc.model, seq, a, b, lanes.as_deref()) };
                self.plan(if name == "lift" { "Lift" } else { "Extract" }, r, format!("{name}ed {}-{}s", secs(a), secs(b)))
            }
            "sync_lock" | "target" => {
                let on = op.get("on").and_then(Value::as_bool).unwrap_or(true);
                let lanes = match self.lanes_arg(op)? {
                    Some(l) => l,
                    None => self.doc.model.active().lanes.iter().map(|l| l.id).collect(),
                };
                let mut edits = Vec::new();
                for l in &lanes {
                    let e = if name == "sync_lock" { ed::set_sync_lock(&self.doc.model, seq, *l, on) } else { ed::set_targeted(&self.doc.model, seq, *l, on) };
                    edits.push(e.map_err(err)?);
                }
                let what = format!("{} {} on {} lane(s)", if name == "sync_lock" { "sync lock" } else { "targeting" }, if on { "on" } else { "off" }, lanes.len());
                self.plan("Lanes", Ok(m::Edit::Batch { edits }), what)
            }
            "sequence" => self.sequence_op(op),
            "speed_ramp" => {
                let c = clip()?;
                let pts: Vec<(f64, f64)> = match (op.get("points"), op.get("speeds")) {
                    (Some(Value::Array(a)), _) => a
                        .iter()
                        .map(|p| match p {
                            Value::Array(xy) => Some((xy.first()?.as_f64()?, xy.get(1)?.as_f64()?)),
                            Value::Object(o) => Some((o.get("at")?.as_f64()?, o.get("speed")?.as_f64()?)),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or("points are [position 0..1 through the clip's footage, speed] pairs")?,
                    (_, Some(Value::Array(s))) => {
                        let v: Vec<f64> = s.iter().filter_map(Value::as_f64).collect();
                        let n = v.len().max(2) - 1;
                        v.iter().enumerate().map(|(i, r)| (i as f64 / n as f64, *r)).collect()
                    }
                    _ => return Err("speed_ramp needs \"points\": [[0, 1], [0.5, 0.25], [1, 1]] or \"speeds\": [1, 0.25, 1]".into()),
                };
                let r = ed::speed_ramp(&self.doc.model, seq, c, &pts);
                let what = format!("speed ramp on clip {} through {}", c.0, pts.iter().map(|(u, r)| format!("{:.0}%@{u:.2}", r * 100.0)).collect::<Vec<_>>().join(" → "));
                self.plan("Speed ramp", r, what)
            }
            "string_out" => self.string_out_op(op),
            "selects" => self.selects_op(op),
            "take" => self.take_op(op),
            "text_cut" => self.text_cut(op),
            "nudge" | "move" => {
                let clips: Vec<m::ClipId> = super::ids(op, "clips").into_iter().map(|c| m::ClipId(c as u64)).collect();
                // `move` takes `to` (where the first clip starts) as well.
                let first = clips.iter().filter_map(|c| self.doc.model.active().clip(*c)).map(|c| c.start).min().ok_or_else(|| format!("{name} needs existing \"clips\""))?;
                let d = match (f(op, "by"), f(op, "to")) {
                    (Some(b), _) => micros(b),
                    (None, Some(t)) => micros(t) - first,
                    _ => return Err(format!("{name} needs \"by\" or \"to\" (seconds)")),
                };
                let to_lane = match op.get("lane") {
                    Some(v) => Some(self.lane_arg(v)?),
                    None => None,
                };
                let what = format!("nudged {} clip(s) {}s{}", clips.len(), d as f64 / 1e6, if op.get("linked") == Some(&json!(false)) { " without their linked clips (now out of sync)" } else { "" });
                if op.get("linked") == Some(&json!(false)) {
                    let r = ed::move_alone(&mut self.doc.model, seq, &clips, d, to_lane);
                    self.plan("Nudge", r, what)
                } else {
                    let ok = self.doc.apply("Nudge", m::Edit::Move { seq, clips, delta: d, to_lane });
                    self.check(ok, &what)
                }
            }
            other => Err(format!("unknown op {other}")),
        }
    }

    /// `insert` / `overwrite` / `fit_to_fill` / `replace` (with marks).
    pub(super) fn source_op(&mut self, name: &str, op: &Value) -> Result<String, String> {
        let seq = self.doc.seq();
        let mode_name = op.get("mode").and_then(Value::as_str).unwrap_or(if name == "source_edit" { "overwrite" } else { name });
        let mode = match mode_name {
            "insert" | "splice" => ed::SourceMode::Insert,
            "overwrite" => ed::SourceMode::Overwrite,
            "replace" => ed::SourceMode::Replace,
            "fit_to_fill" | "fit" => ed::SourceMode::FitToFill,
            m => return Err(format!("mode \"{m}\": insert, overwrite, replace or fit_to_fill")),
        };
        let (source, has_video, has_audio) = match op.get("sequence") {
            Some(v) => (ed::Source::Sequence(self.sequence_arg(v)?), true, true),
            None => {
                let id = self.model_media(op)?;
                let item = self.doc.model.media_item(id).ok_or("that media is gone")?;
                (ed::Source::Media(id), item.has_video(), item.has_audio())
            }
        };
        let no = |k: &str| op.get(k).is_some_and(|v| v.is_null() || v == &json!(false));
        let video_lane = if no("video") || no("lane") || (!has_video && op.get("lane").is_none()) {
            None
        } else {
            Some(match op.get("lane") {
                Some(v) => self.lane_arg(v)?,
                None => self.doc.model.active().spine_id(),
            })
        };
        let audio_lane = match op.get("audio_lane") {
            Some(v) if !v.is_null() && v != &json!(false) => Some(self.lane_arg(v)?),
            _ if video_lane.is_none() && has_audio && !no("audio") => Some(self.audio_lane()?),
            _ => None,
        };
        // Replace aims at a clip: its time is the record mark.
        let mut rec_in = f(op, "at").map(micros);
        if mode == ed::SourceMode::Replace
            && rec_in.is_none()
            && let Some(c) = op.get("clip").and_then(Value::as_u64).and_then(|c| self.doc.model.active().clip(m::ClipId(c)))
        {
            rec_in = Some(c.start);
        }
        let e = ed::SourceEdit {
            source,
            src_in: f(op, "src_in").map(micros),
            src_out: f(op, "src_out").map(micros),
            rec_in,
            rec_out: f(op, "rec_out").map(micros),
            mode,
            video_lane,
            audio_lane,
            audio: !no("audio"),
        };
        // No record mark: the end of the main lane (append).
        let end = self.doc.model.active().spine_end();
        let (edit, done) = ed::source_edit(&mut self.doc.model, seq, &e, end).map_err(err)?;
        let ok = self.doc.apply(mode_name, edit);
        let speed = if (done.speed - 1.0).abs() > 1e-6 { format!(" at {:.0}% speed", done.speed * 100.0) } else { String::new() };
        let what = format!(
            "{mode_name}: source {}-{}s into {}-{}s{speed}, clip(s) {}",
            secs(done.src.start),
            secs(done.src.end()),
            secs(done.rec.start),
            secs(done.rec.end()),
            done.clips.iter().map(|c| c.0.to_string()).collect::<Vec<_>>().join(", ")
        );
        let _ = has_audio;
        self.check(ok, &what)
    }

    fn sequence_op(&mut self, op: &Value) -> Result<String, String> {
        let action = op.get("action").and_then(Value::as_str).unwrap_or("new");
        let name = op.get("name").and_then(Value::as_str).map(str::to_owned);
        let open = op.get("open").and_then(Value::as_bool).unwrap_or(true);
        let target = match op.get("sequence") {
            Some(v) => self.sequence_arg(v)?,
            None => self.doc.seq(),
        };
        let switch = |s: &mut Self, id: m::SequenceId| -> Result<String, String> {
            let ok = s.doc.apply("Switch sequence", m::Edit::SetActiveSequence { id });
            s.check(ok, "switched")
        };
        match action {
            "new" | "duplicate" => {
                let (edit, id) = if action == "new" {
                    let n = name.unwrap_or_else(|| format!("Sequence {}", self.doc.model.sequences.len() + 1));
                    ed::new_sequence(&mut self.doc.model, &n)
                } else {
                    let base = self.doc.model.sequence(target).map(|s| s.name.clone()).unwrap_or_default();
                    ed::duplicate_sequence(&mut self.doc.model, target, &name.unwrap_or(format!("{base} copy"))).map_err(err)?
                };
                let ok = self.doc.apply("Sequence", edit);
                self.check(ok, "sequence")?;
                if open {
                    switch(self, id)?;
                }
                Ok(format!("{} sequence {} \"{}\"{}", if action == "new" { "new" } else { "duplicated as" }, id.0, self.doc.model.sequence(id).map(|s| s.name.as_str()).unwrap_or(""), if open { ", now active" } else { "" }))
            }
            "switch" | "open" => {
                switch(self, target)?;
                Ok(format!("now cutting \"{}\"", self.doc.model.active().name))
            }
            "rename" => {
                let n = name.ok_or("rename needs \"name\"")?;
                let ok = self.doc.apply("Rename sequence", m::Edit::RenameSequence { id: target, name: n.clone() });
                self.check(ok, &format!("renamed to \"{n}\""))
            }
            "delete" => {
                if target == self.doc.seq() {
                    return Err("switch to another sequence before deleting this one".into());
                }
                let ok = self.doc.apply("Delete sequence", m::Edit::RemoveSequence { id: target });
                self.check(ok, "sequence deleted")
            }
            "nest" => {
                let clips: Vec<m::ClipId> = super::ids(op, "clips").into_iter().map(|c| m::ClipId(c as u64)).collect();
                if clips.is_empty() {
                    return Err("nest needs \"clips\"".into());
                }
                let new_sequence = m::SequenceId(self.doc.alloc_id());
                let new_clip = m::ClipId(self.doc.alloc_id());
                let seq = self.doc.seq();
                let ok = self.doc.apply("Nest", m::Edit::CreateCompound { seq, clips, name: name.unwrap_or_else(|| "Nest".into()), new_sequence, new_clip });
                self.check(ok, &format!("nested into clip {} (sequence {})", new_clip.0, new_sequence.0))
            }
            a => Err(format!("sequence action \"{a}\": new, duplicate, switch, rename, delete, nest")),
        }
    }

    fn string_out_op(&mut self, op: &Value) -> Result<String, String> {
        let p = self.doc.project.clone();
        let mut ids: Vec<m::MediaId> = match op.get("media") {
            None | Some(Value::Null) => self.doc.model.media.iter().filter(|mi| mi.has_video()).map(|mi| mi.id).collect(),
            Some(Value::String(s)) if s == "all" => self.doc.model.media.iter().filter(|mi| mi.has_video()).map(|mi| mi.id).collect(),
            Some(Value::Array(a)) => a.iter().map(|v| media_of(&p, v).map(|i| m::MediaId(i as u64))).collect::<Result<_, _>>()?,
            Some(v) => vec![m::MediaId(media_of(&p, v)? as u64)],
        };
        let scene = op.get("scene").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()).to_uppercase());
        let circled = op.get("circled").and_then(Value::as_bool).unwrap_or(false);
        let infos: Vec<(m::MediaId, ed::TakeInfo)> = ids.iter().filter_map(|id| self.doc.model.media_item(*id).map(|mi| (*id, ed::take_info(mi)))).collect();
        ids = infos.iter().filter(|(_, i)| scene.as_ref().is_none_or(|s| i.scene.as_deref().map(str::to_uppercase).as_ref() == Some(s)) && (!circled || i.circled)).map(|(id, _)| *id).collect();
        let mut takes: Vec<(ed::TakeInfo, ed::Take)> = ids
            .iter()
            .map(|id| {
                let info = infos.iter().find(|(m, _)| m == id).map(|(_, i)| i.clone()).unwrap_or_default();
                let label = take_label(&info);
                (info, ed::Take { media: *id, from: None, to: None, label })
            })
            .collect();
        if takes.is_empty() {
            return Err("no media matches (string_out takes video files; check scene / circled)".into());
        }
        takes.sort_by_key(|(i, t)| (natural(&i.scene), natural(&i.take), t.media.0));
        let name = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| match &scene {
            Some(s) => format!("Scene {s} string-out"),
            None => "String-out".into(),
        });
        let gap = micros(f(op, "gap").unwrap_or(0.0));
        let list: Vec<ed::Take> = takes.into_iter().map(|(_, t)| t).collect();
        self.make_reel(&name, &list, gap, op)
    }

    fn selects_op(&mut self, op: &Value) -> Result<String, String> {
        let p = self.doc.project.clone();
        let list = op.get("takes").and_then(Value::as_array).ok_or("selects needs \"takes\": [{media, from, to, label}]")?;
        let mut takes = Vec::new();
        for t in list {
            let media = m::MediaId(media_of(&p, t.get("media").ok_or("each take needs \"media\"")?)? as u64);
            takes.push(ed::Take { media, from: f(t, "from").map(micros), to: f(t, "to").map(micros), label: t.get("label").and_then(Value::as_str).unwrap_or("").to_owned() });
        }
        let name = op.get("name").and_then(Value::as_str).unwrap_or("Selects").to_owned();
        self.make_reel(&name, &takes, micros(f(op, "gap").unwrap_or(0.0)), op)
    }

    fn make_reel(&mut self, name: &str, takes: &[ed::Take], gap: view::Time, op: &Value) -> Result<String, String> {
        let (edit, id) = ed::string_out(&mut self.doc.model, name, takes, gap).map_err(err)?;
        let ok = self.doc.apply(name, edit);
        self.check(ok, "reel")?;
        let open = op.get("open").and_then(Value::as_bool).unwrap_or(true);
        if open {
            let ok = self.doc.apply("Switch sequence", m::Edit::SetActiveSequence { id });
            self.check(ok, "switched")?;
        }
        let dur = self.doc.model.sequence(id).map(|s| s.duration()).unwrap_or(0);
        Ok(format!("sequence {} \"{name}\": {} takes, {:.1}s{}", id.0, takes.len(), secs(dur), if open { ", now active (switch back with sequence {action: switch})" } else { "" }))
    }

    fn take_op(&mut self, op: &Value) -> Result<String, String> {
        if op.get("auto").and_then(Value::as_bool).unwrap_or(false) {
            let mut edits = Vec::new();
            let mut found = Vec::new();
            for mi in &self.doc.model.media {
                if mi.extra.contains_key("scene") {
                    continue;
                }
                if let Some((scene, take, camera)) = ed::parse_take(&mi.name) {
                    let info = ed::TakeInfo { scene: Some(scene), take, camera, ..ed::take_info(mi) };
                    found.push(format!("{} = {}", mi.name, take_label(&info)));
                    edits.push(ed::set_take_info(&self.doc.model, mi.id, &info).map_err(err)?);
                }
            }
            let what = if found.is_empty() { "no file names carry scene and take".into() } else { format!("read from names: {}", found.join("; ")) };
            return self.plan("Takes", Ok(m::Edit::Batch { edits }), what);
        }
        let id = self.model_media(op)?;
        let item = self.doc.model.media_item(id).ok_or("that media is gone")?;
        let mut info = ed::take_info(item);
        let text = |k: &str| op.get(k).map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()));
        if let Some(v) = text("scene") {
            info.scene = Some(v);
        }
        if let Some(v) = text("take") {
            info.take = Some(v);
        }
        if let Some(v) = text("camera") {
            info.camera = Some(v);
        }
        if let Some(v) = text("note") {
            info.note = Some(v);
        }
        if let Some(b) = op.get("circled").and_then(Value::as_bool) {
            info.circled = b;
        }
        if let Some(r) = op.get("rating").and_then(Value::as_u64) {
            info.rating = r.min(5) as u8;
        }
        let what = format!("{}: {}", item.name, take_label(&info));
        let r = ed::set_take_info(&self.doc.model, id, &info);
        self.plan("Take", r, what)
    }

    /// The transcript lines a clip's footage says, in source seconds.
    fn transcript_of(&self, clip: &m::Clip) -> Result<Vec<Line>, String> {
        clip_lines(&self.doc.model, clip)
    }

    fn text_cut(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("text_cut needs \"clip\"")?;
        let c = self.doc.model.active().clip(m::ClipId(id)).cloned().ok_or_else(|| format!("no clip {id} on this sequence"))?;
        let lines = self.transcript_of(&c)?;
        let (lo, hi) = (c.source_in as f64 / 1e6, c.source_out() as f64 / 1e6);
        let all: Vec<&Word> = lines.iter().flat_map(|l| l.words.iter()).filter(|w| w.to > lo && w.from < hi).collect();
        let find = |phrase: &str| -> Result<Vec<(f64, f64)>, String> {
            let want = words(phrase);
            if want.is_empty() {
                return Err("an empty phrase".into());
            }
            let hits: Vec<(f64, f64)> = (0..all.len().saturating_sub(want.len() - 1)).filter(|&i| want.iter().enumerate().all(|(k, w)| &all[i + k].norm == w)).map(|i| (all[i].from, all[i + want.len() - 1].to)).collect();
            if hits.is_empty() {
                let near: Vec<&str> = lines.iter().filter(|l| words(&l.text).iter().any(|w| want.contains(w))).map(|l| l.text.as_str()).take(3).collect();
                return Err(format!("\"{phrase}\" is not said in clip {id}{}", if near.is_empty() { String::new() } else { format!("; close: {}", near.join(" | ")) }));
            }
            Ok(hits)
        };
        let mut cut: Vec<(f64, f64)> = Vec::new();
        let list = |k: &str| -> Vec<Value> {
            match op.get(k) {
                Some(Value::Array(a)) => a.clone(),
                Some(v) => vec![v.clone()],
                None => Vec::new(),
            }
        };
        for v in list("remove") {
            cut.extend(find(v.as_str().ok_or("remove lists phrases (strings)")?)?);
        }
        for v in list("lines") {
            let n = v.as_u64().ok_or("lines are numbers from editorial {ask: transcript}")? as usize;
            let l = lines.get(n.wrapping_sub(1)).ok_or_else(|| format!("clip {id} has {} lines", lines.len()))?;
            cut.push((l.from, l.to));
        }
        for v in list("ranges") {
            let (a, b) = (v.get(0).and_then(Value::as_f64), v.get(1).and_then(Value::as_f64));
            cut.push((a.ok_or("ranges are [from, to] source seconds")?, b.ok_or("ranges are [from, to] source seconds")?));
        }
        let keep = list("keep");
        if !keep.is_empty() {
            let mut kept: Vec<(f64, f64)> = Vec::new();
            for v in keep {
                match v {
                    Value::Number(n) => {
                        let l = lines.get((n.as_u64().unwrap_or(0) as usize).wrapping_sub(1)).ok_or_else(|| format!("clip {id} has {} lines", lines.len()))?;
                        kept.push((l.from, l.to));
                    }
                    Value::String(s) => kept.extend(find(&s)?.into_iter().take(1)),
                    _ => return Err("keep lists phrases or line numbers".into()),
                }
            }
            kept.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut at = lo;
            for (a, b) in kept {
                if a > at {
                    cut.push((at, a));
                }
                at = at.max(b);
            }
            if at < hi {
                cut.push((at, hi));
            }
        }
        if cut.is_empty() {
            return Err("text_cut needs \"remove\" (phrases), \"lines\", \"keep\" or \"ranges\"".into());
        }
        let total: f64 = cut.iter().map(|(a, b)| (b.min(hi) - a.max(lo)).max(0.0)).sum();
        let ranges: Vec<(view::Time, view::Time)> = cut.iter().map(|(a, b)| (micros(*a), micros(*b))).collect();
        let seq = self.doc.seq();
        let r = ed::remove_source_ranges(&mut self.doc.model, seq, m::ClipId(id), &ranges);
        self.plan("Text cut", r, format!("cut {} piece(s), {:.2}s of footage, out of clip {id}", cut.len(), total))
    }

    // ------------------------------------------------------------- the tool

    /// `editorial`: look-ups an editor makes while cutting.
    pub(super) fn editorial_tool(&mut self, a: &Value) -> Result<Value, String> {
        let ask = a.get("ask").and_then(Value::as_str).ok_or("editorial needs \"ask\": match_frame, reverse_match, gaps, duplicates, index, sequences, lanes, takes, transcript, find_takes")?;
        let model = &self.doc.model;
        let seq = model.active();
        let name_of = |src: ed::Source| match src {
            ed::Source::Media(id) => json!({"media": id.0, "name": model.media_item(id).map(|i| i.name.clone())}),
            ed::Source::Sequence(id) => json!({"sequence": id.0, "name": model.sequence(id).map(|s| s.name.clone())}),
        };
        Ok(match ask {
            "match_frame" => {
                let t = micros(f(a, "t").ok_or("match_frame needs \"t\" (timeline seconds)")?);
                let lane = match a.get("lane") {
                    Some(v) => Some(self.lane_arg(v)?),
                    None => None,
                };
                let hit = ed::match_frame(model, seq.id, t, lane).ok_or("no footage on screen there")?;
                json!({"clip": hit.clip.0, "source": name_of(hit.source), "source_time": secs(hit.source_time), "clip_uses": [secs(hit.source_in), secs(hit.source_out)], "hint": "insert/overwrite with this media and src_in to cut in from the same moment"})
            }
            "reverse_match" => {
                let src = match a.get("sequence") {
                    Some(v) => ed::Source::Sequence(self.sequence_arg(v)?),
                    None => ed::Source::Media(m::MediaId(media_of(&self.doc.project, a.get("media").ok_or("reverse_match needs \"media\" and \"t\" (seconds into it)")?)? as u64)),
                };
                let t = micros(f(a, "t").ok_or("reverse_match needs \"t\"")?);
                let uses: Vec<Value> = ed::reverse_match(model, src, t).iter().map(|u| json!({"sequence": model.sequence(u.seq).map(|s| s.name.clone()), "clip": u.clip.0, "at": secs(u.at)})).collect();
                json!({"source": name_of(src), "t": secs(t), "used": uses})
            }
            "gaps" => {
                let min = micros(f(a, "min").unwrap_or(0.0));
                let gaps: Vec<Value> = ed::find_gaps(seq, min).iter().map(|g| json!({"lane": seq.lane(g.lane).map(|l| l.name.clone()), "from": secs(g.start), "to": secs(g.end), "length": secs(g.end - g.start)})).collect();
                json!({"gaps": gaps, "hint": "close a main-lane gap with delete {clips: [the gap]} or extract {from, to}"})
            }
            "duplicates" => {
                let min = micros(f(a, "min").unwrap_or(1.0 / 24.0));
                let d: Vec<Value> = ed::find_duplicates(seq, min).iter().map(|d| json!({"media": model.media_item(d.media).map(|i| i.name.clone()), "clips": [d.a.0, d.b.0], "source": [secs(d.source.start), secs(d.source.end())], "length": secs(d.source.duration)})).collect();
                json!({"duplicates": d})
            }
            "index" => {
                let kind = a.get("kind").and_then(Value::as_str);
                let rows: Vec<Value> = ed::timeline_index(model, seq)
                    .into_iter()
                    .filter(|r| kind.is_none_or(|k| r.kind == k || k == "markers" && r.kind.ends_with("marker")))
                    .map(|r| json!({"kind": r.kind, "from": secs(r.start), "to": secs(r.end), "lane": r.lane.and_then(|l| seq.lane(l)).map(|l| l.name.clone()), "clip": r.clip.map(|c| c.0), "name": r.name, "detail": r.detail}))
                    .collect();
                json!({"sequence": seq.name, "rows": rows})
            }
            "sequences" => {
                let list: Vec<Value> = model.sequences.iter().map(|s| json!({"id": s.id.0, "name": s.name, "active": s.id == seq.id, "clips": s.clips.iter().filter(|c| !c.is_gap()).count(), "duration": secs(s.duration()), "nested_in": model.sequences.iter().filter(|o| o.clips.iter().any(|c| c.sequence_id() == Some(s.id))).map(|o| o.name.clone()).collect::<Vec<_>>()})).collect();
                json!({"sequences": list})
            }
            "lanes" => {
                let list: Vec<Value> = seq.lanes.iter().map(|l| json!({"id": l.id.0, "name": l.name, "kind": format!("{:?}", l.kind).to_lowercase(), "sync_lock": ed::sync_locked(l), "targeted": ed::targeted(l), "locked": l.locked})).collect();
                json!({"lanes": list})
            }
            "takes" => {
                let scene = a.get("scene").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()).to_uppercase());
                let mut list: Vec<(ed::TakeInfo, &m::MediaItem)> = model.media.iter().map(|mi| (ed::take_info(mi), mi)).filter(|(i, _)| scene.as_ref().is_none_or(|s| i.scene.as_deref().map(str::to_uppercase).as_ref() == Some(s))).collect();
                list.sort_by_key(|(i, mi)| (natural(&i.scene), natural(&i.take), mi.id.0));
                let rows: Vec<Value> = list
                    .iter()
                    .map(|(i, mi)| {
                        let used = seq.clips.iter().filter(|c| c.media_id() == Some(mi.id)).count();
                        json!({"media": mi.id.0, "name": mi.name, "scene": i.scene, "take": i.take, "camera": i.camera, "circled": i.circled, "rating": i.rating, "note": i.note, "length": mi.duration().map(secs), "clips_in_cut": used})
                    })
                    .collect();
                json!({"takes": rows})
            }
            "transcript" => {
                let id = a.get("clip").and_then(Value::as_u64).ok_or("transcript needs \"clip\"")?;
                let c = seq.clip(m::ClipId(id)).cloned().ok_or_else(|| format!("no clip {id}"))?;
                let lines = self.transcript_of(&c)?;
                let rows: Vec<Value> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let at = ed::timeline_time(&c, micros(l.from).max(c.source_in)).map(secs);
                        json!({"line": i + 1, "text": l.text, "source": [l.from, l.to], "at": at})
                    })
                    .collect();
                json!({"clip": id, "lines": rows, "hint": "text_cut {clip, remove: [\"words to drop\"]} or {lines: [n]} or {keep: [n, ...]}"})
            }
            "find_takes" => {
                let line = a.get("line").and_then(Value::as_str).ok_or("find_takes needs \"line\" (a line of the script)")?;
                self.find_takes(line, a.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize)?
            }
            other => return Err(format!("ask \"{other}\": match_frame, reverse_match, gaps, duplicates, index, sequences, lanes, takes, transcript, find_takes")),
        })
    }

    /// Every take that says a script line, best first: the media, where in
    /// it (source seconds), how much of the line it says, its scene and
    /// take, and whether the cut already uses it.
    pub(crate) fn find_takes(&self, line: &str, limit: usize) -> Result<Value, String> {
        let hits = take_hits(&self.doc.model, &self.doc.project.media, line, limit)?;
        let takes: Vec<Value> = hits
            .iter()
            .map(|h| json!({"media": h.media, "name": h.name, "from": (h.from * 100.0).round() / 100.0, "to": (h.to * 100.0).round() / 100.0, "says": h.says, "match": (h.score * 100.0).round() / 100.0, "take": h.take, "in_cut": h.in_cut}))
            .collect();
        Ok(json!({"line": line, "takes": takes, "hint": "cut one in with insert/overwrite {media, src_in: from, src_out: to}; string them all with selects"}))
    }
}

/// A take that says a script line.
#[derive(Clone, Debug)]
pub(crate) struct TakeHit {
    pub media: u32,
    pub name: String,
    /// Source seconds.
    pub from: f64,
    pub to: f64,
    pub says: String,
    /// How much of the line it says, in order (0..1).
    pub score: f64,
    /// "Sc 12 Tk 3 (circled)".
    pub take: String,
    /// Clips of the active sequence already using it.
    pub in_cut: Vec<u64>,
}

/// Every take among `media` that says `line`, best (and circled) first.
/// Files not heard yet are transcribed, side by side.
pub(crate) fn take_hits(model: &m::Project, media: &[view::MediaItem], line: &str, limit: usize) -> Result<Vec<TakeHit>, String> {
    let want = words(line);
    if want.is_empty() {
        return Err("an empty line".into());
    }
    let items: Vec<&view::MediaItem> = media.iter().filter(|mi| mi.path.is_some() && mi.kind != view::MediaKind::Image).collect();
    let indexes: Vec<Option<std::sync::Arc<Vec<Line>>>> = std::thread::scope(|sc| {
        let hs: Vec<_> = items.iter().map(|mi| sc.spawn(move || transcript(mi.path.as_ref()?).ok())).collect();
        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    let mut hits: Vec<(f64, TakeHit)> = Vec::new();
    for (mi, ix) in items.iter().zip(indexes) {
        let Some(ix) = ix else { continue };
        let id = m::MediaId(mi.id as u64);
        let info = model.media_item(id).map(ed::take_info).unwrap_or_default();
        let mut best: Option<(f64, f64, f64, String)> = None;
        let mut found: Vec<(f64, f64, f64, String)> = Vec::new();
        for i in 0..ix.len() {
            for n in 1..=3usize.min(ix.len() - i) {
                let span = &ix[i..i + n];
                let text = span.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
                let got = words(&text);
                let common = lcs(&want, &got) as f64;
                let score = 2.0 * common / (want.len() + got.len()) as f64;
                if score >= 0.5 && best.as_ref().is_none_or(|b| score > b.0) {
                    best = Some((score, span[0].from, span[n - 1].to, text));
                }
            }
            // One hit per place: the best window starting near here.
            if let Some(b) = best.take() {
                if found.last().is_some_and(|l| b.1 < l.2) {
                    if b.0 > found.last().expect("checked").0 {
                        *found.last_mut().expect("checked") = b;
                    }
                } else {
                    found.push(b);
                }
            }
        }
        for (score, from, to, text) in found {
            let in_cut: Vec<u64> = ed::reverse_match(model, ed::Source::Media(id), micros((from + to) / 2.0)).iter().filter(|u| u.seq == model.active_sequence).map(|u| u.clip.0).collect();
            let rank = score + if info.circled { 0.05 } else { 0.0 };
            hits.push((rank, TakeHit { media: mi.id, name: mi.name.clone(), from, to, says: text, score, take: take_label(&info), in_cut }));
        }
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(hits.into_iter().take(limit.max(1)).map(|(_, h)| h).collect())
}

/// The transcript lines a clip's footage says (source seconds).
pub(crate) fn clip_lines(model: &m::Project, clip: &m::Clip) -> Result<Vec<Line>, String> {
    let media = clip.media_id().ok_or("only media clips have a transcript")?;
    let item = model.media_item(media).ok_or("that media is gone")?;
    let lines = transcript(std::path::Path::new(&item.path)).map_err(|e| format!("\"{}\": {e}", item.name))?;
    let (a, b) = (clip.source_in as f64 / 1e6, clip.source_out() as f64 / 1e6);
    Ok(lines.iter().filter(|l| l.to > a && l.from < b).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use crate::services;

    const S: i64 = 1_000_000;

    fn movie(s: &mut Session, name: &str, secs: i64) -> u64 {
        let id = s.doc.alloc_id();
        let info = m::MediaInfo { kind: m::MediaKind::Video, duration: Some(secs * S), video: Some(Default::default()), audio: Some(Default::default()), ..Default::default() };
        // A folder per session: transcripts are kept by path, and tests
        // run side by side.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut mi = m::MediaItem::new(m::MediaId(id), format!("/nonexistent/editorial-test/{n}/{name}"), info);
        mi.name = name.into();
        assert!(s.doc.apply("media", m::Edit::InsertMedia { item: Box::new(mi), index: None }));
        id
    }

    fn session() -> (Session, u64, u64) {
        let doc: Doc = services::project::new_custom("Cut", 640, 360, 24.0);
        let mut s = Session::new(doc);
        let a = movie(&mut s, "S12_T1.mov", 60);
        let b = movie(&mut s, "S12_T2.mov", 60);
        (s, a, b)
    }

    fn edit(s: &mut Session, ops: Value) -> Value {
        let out = s.call("edit", &json!({ "ops": ops })).unwrap_or_else(|e| panic!("edit failed: {e}"));
        let super::super::Content::Text(t) = &out[0] else { panic!("text") };
        serde_json::from_str(t).unwrap()
    }

    fn ask(s: &mut Session, a: Value) -> Value {
        let out = s.call("editorial", &a).unwrap_or_else(|e| panic!("editorial failed: {e}"));
        let super::super::Content::Text(t) = &out[0] else { panic!("text") };
        serde_json::from_str(t).unwrap()
    }

    fn spine(s: &Session) -> Vec<(f64, f64, f64)> {
        s.doc.model.active().spine_clips().iter().map(|c| (secs(c.start), secs(c.duration), secs(c.source_in))).collect()
    }

    #[test]
    fn the_agent_cuts_with_source_edits_trims_and_ranges() {
        let (mut s, a, b) = session();
        // Lay a 10 s, then splice 4 s of b into the middle, backtime an
        // overwrite and fit 6 s of b into 3 s.
        edit(&mut s, json!([{"op": "overwrite", "media": a, "src_in": 0, "src_out": 10, "at": 0}]));
        edit(&mut s, json!([{"op": "insert", "media": b, "src_in": 20, "src_out": 24, "at": 5}]));
        assert_eq!(spine(&s), vec![(0.0, 5.0, 0.0), (5.0, 4.0, 20.0), (9.0, 5.0, 5.0)]);
        edit(&mut s, json!([{"op": "overwrite", "media": b, "src_in": 40, "src_out": 41, "rec_out": 14}]));
        assert_eq!(spine(&s).last().copied(), Some((13.0, 1.0, 40.0)));
        let r = edit(&mut s, json!([{"op": "fit_to_fill", "media": b, "src_in": 0, "src_out": 6, "at": 0, "rec_out": 3}]));
        assert!(r["notes"][0].as_str().unwrap().contains("200% speed"), "{r}");
        // Ripple, extend, lift, extract.
        let first = s.doc.model.active().spine_clips()[1].id.0;
        edit(&mut s, json!([{"op": "ripple", "clip": first, "edge": "end", "by": -1}]));
        assert_eq!(s.doc.model.active().spine_end(), 13 * S);
        edit(&mut s, json!([{"op": "extract", "from": 0, "to": 3}]));
        assert_eq!(s.doc.model.active().spine_end(), 10 * S);
        edit(&mut s, json!([{"op": "lift", "from": 1, "to": 2}]));
        assert!(s.doc.model.active().spine_clips().iter().any(|c| c.is_gap()));
        let gaps = ask(&mut s, json!({"ask": "gaps"}));
        assert_eq!(gaps["gaps"][0]["from"], json!(1.0));
        // One undo takes back the lift.
        s.call("undo", &json!({})).unwrap();
        assert!(!s.doc.model.active().spine_clips().iter().any(|c| c.is_gap()));
    }

    #[test]
    fn j_and_l_cuts_match_frame_and_sequences() {
        let (mut s, a, b) = session();
        edit(&mut s, json!([{"op": "overwrite", "media": a, "src_in": 0, "src_out": 8, "at": 0}, {"op": "overwrite", "media": b, "src_in": 2, "src_out": 10, "at": 8}]));
        let first = s.doc.model.active().spine_clips()[0].id.0;
        let r = edit(&mut s, json!([{"op": "j_cut", "clip": first, "by": 1.5}]));
        assert!(r["notes"][0].as_str().unwrap().contains("J cut"), "{r}");
        let audio: Vec<(f64, f64)> = {
            let q = s.doc.model.active();
            q.clips.iter().filter(|c| q.lane(c.lane).is_some_and(|l| l.kind == m::LaneKind::Audio)).map(|c| (secs(c.start), secs(c.end()))).collect()
        };
        assert_eq!(audio, vec![(0.0, 6.5), (6.5, 16.0)]);
        let mf = ask(&mut s, json!({"ask": "match_frame", "t": 9}));
        assert_eq!((mf["source"]["media"].as_u64(), mf["source_time"].as_f64()), (Some(b), Some(3.0)));
        let rm = ask(&mut s, json!({"ask": "reverse_match", "media": a, "t": 4}));
        assert_eq!(rm["used"][0]["at"], json!(4.0));
        // A second cut, then back.
        let orig = s.doc.model.active_sequence.0;
        let r = edit(&mut s, json!([{"op": "sequence", "action": "duplicate", "name": "Director's cut"}]));
        assert!(r["notes"][0].as_str().unwrap().contains("now active"));
        let list = ask(&mut s, json!({"ask": "sequences"}));
        assert_eq!(list["sequences"].as_array().unwrap().len(), 2);
        edit(&mut s, json!([{"op": "sequence", "action": "switch", "sequence": orig}]));
        assert_eq!(s.doc.model.active_sequence.0, orig);
    }

    #[test]
    fn takes_string_outs_and_script_lines() {
        let (mut s, a, b) = session();
        let r = edit(&mut s, json!([{"op": "take", "auto": true}]));
        assert!(r["notes"][0].as_str().unwrap().contains("Sc 12 Tk 1"), "{r}");
        edit(&mut s, json!([{"op": "take", "media": b, "circled": true, "note": "best read"}]));
        let takes = ask(&mut s, json!({"ask": "takes", "scene": "12"}));
        assert_eq!(takes["takes"][1]["circled"], json!(true));
        // Both takes say the line; the circled one ranks first.
        for (id, at) in [(a, 3.0), (b, 5.0)] {
            let path = s.doc.model.media_item(m::MediaId(id)).unwrap().path.clone();
            let lines = vec![Line::spread(0.5, 2.0, "Where were you last night?"), Line::spread(at, at + 2.5, "I told you, I was at the docks.")];
            remember(std::path::Path::new(&path), lines);
        }
        let found = ask(&mut s, json!({"ask": "find_takes", "line": "I told you I was down at the docks"}));
        let t = &found["takes"];
        assert_eq!(t.as_array().unwrap().len(), 2, "{found}");
        assert_eq!(t[0]["media"].as_u64(), Some(b));
        assert_eq!(t[0]["from"], json!(5.0));
        // A string-out of scene 12, then a selects reel of the line.
        let r = edit(&mut s, json!([{"op": "string_out", "scene": 12, "open": false}]));
        assert!(r["notes"][0].as_str().unwrap().contains("2 takes"), "{r}");
        let r = edit(&mut s, json!([{"op": "selects", "takes": [{"media": b, "from": 5, "to": 7.5, "label": "docks"}, {"media": a, "from": 3, "to": 5.5}], "name": "Docks line"}]));
        assert!(r["notes"][0].as_str().unwrap().contains("5.0s"), "{r}");
        assert_eq!(s.doc.model.active().name, "Docks line");
    }

    #[test]
    fn text_cuts_and_speed_ramps() {
        let (mut s, a, _) = session();
        let path = s.doc.model.media_item(m::MediaId(a)).unwrap().path.clone();
        remember(std::path::Path::new(&path), vec![Line::spread(1.0, 3.0, "So um basically we won"), Line::spread(4.0, 6.0, "and that was that")]);
        edit(&mut s, json!([{"op": "overwrite", "media": a, "src_in": 0, "src_out": 8, "at": 0}]));
        let clip = s.doc.model.active().spine_clips()[0].id.0;
        let tr = ask(&mut s, json!({"ask": "transcript", "clip": clip}));
        assert_eq!(tr["lines"][1]["text"], json!("and that was that"));
        let r = edit(&mut s, json!([{"op": "text_cut", "clip": clip, "remove": ["um basically"]}]));
        assert!(r["notes"][0].as_str().unwrap().contains("1 piece"), "{r}");
        let len = secs(s.doc.model.active().spine_end());
        assert!(len < 7.4 && len > 6.6, "{len}");
        // Not said: refused with what is close.
        let Err(e) = s.call("edit", &json!({"ops": [{"op": "text_cut", "clip": clip, "remove": ["we lost"]}]})) else { panic!("cut words never said") };
        assert!(e.contains("not said"), "{e}");
        // A ramp on the last piece.
        let last = s.doc.model.active().spine_clips().last().unwrap().id.0;
        edit(&mut s, json!([{"op": "speed_ramp", "clip": last, "speeds": [1, 0.25, 1]}]));
        assert!(matches!(s.doc.model.active().clip(m::ClipId(last)).unwrap().speed, m::Speed::Curve(_)));
        let ix = ask(&mut s, json!({"ask": "index", "kind": "clip"}));
        assert!(ix["rows"].as_array().unwrap().iter().any(|r| r["detail"].as_str().unwrap().contains("speed ramp")));
    }

    #[test]
    fn sync_locks_and_markers() {
        let (mut s, a, b) = session();
        edit(&mut s, json!([{"op": "overwrite", "media": a, "src_in": 0, "src_out": 10, "at": 0}, {"op": "overwrite", "media": b, "audio_lane": "audio", "video": false, "src_in": 0, "src_out": 10, "at": 0}]));
        edit(&mut s, json!([{"op": "sync_lock", "lane": "audio"}]));
        let lanes = ask(&mut s, json!({"ask": "lanes"}));
        assert!(lanes["lanes"].as_array().unwrap().iter().any(|l| l["sync_lock"] == json!(true)));
        edit(&mut s, json!([{"op": "insert", "media": b, "src_in": 30, "src_out": 32, "at": 4}]));
        let q = s.doc.model.active();
        let sound: Vec<(f64, f64)> = q.clips.iter().filter(|c| q.lane(c.lane).is_some_and(|l| l.kind == m::LaneKind::Audio)).map(|c| (secs(c.start), secs(c.duration))).collect();
        assert_eq!(sound, vec![(0.0, 4.0), (6.0, 6.0)]);
        // Subframe: the sound slips a quarter frame on its own.
        let sound = s.doc.model.active().clips.iter().find(|c| c.start == 6 * S).unwrap().id.0;
        edit(&mut s, json!([{"op": "nudge", "clips": [sound], "by": 0.0104, "linked": false}]));
        assert_eq!(s.doc.model.active().clip(m::ClipId(sound)).unwrap().start, 6 * S + 10_400);
        // `move` with linked: false leaves the picture's sound behind.
        edit(&mut s, json!([{"op": "move", "clips": [sound], "to": 6.5, "linked": false}]));
        assert_eq!(s.doc.model.active().clip(m::ClipId(sound)).unwrap().start, 6 * S + S / 2);
        // A coloured marker with a note, read back from the index.
        edit(&mut s, json!([{"op": "marker", "at": 2, "name": "Pickup", "colour": "red", "note": "ADR the line"}]));
        let ix = ask(&mut s, json!({"ask": "index", "kind": "markers"}));
        assert_eq!(ix["rows"][0]["detail"], json!("red: ADR the line"));
    }
}
