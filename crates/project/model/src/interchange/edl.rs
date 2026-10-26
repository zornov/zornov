// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CMX3600 EDLs: export of a sequence's spine, and import.
//!
//! Export: one event per spine clip, in timeline order. Source timecodes
//! are the media's own (its first-frame timecode plus the source time)
//! and reels are the media's reel names when set. Gaps leave a hole in the
//! record timecodes; dissolves become two-line `D` events; speed changes
//! and reverse get `M2` motion lines; an exact ASC CDL on a clip's grade is
//! written as `*ASC_SOP` / `*ASC_SAT`; sequence markers become `* LOC:`
//! lines. Everything else (other lanes, effects, generators, …) is listed
//! in the report, since an EDL cannot hold it.
//!
//! Import ([`import`]): video and audio events, cuts and dissolves (wipes
//! and keys become cuts, reported), `M2` speed changes, `* FROM CLIP
//! NAME`, `* SOURCE FILE`, `*ASC_SOP` / `*ASC_SAT` (onto the clip's grade)
//! and `* LOC` markers. Each reel is matched to project media by reel
//! name, then by file name; the source timecode is turned into a source
//! position with the media's own timecode. Unmatched reels become offline
//! media items (path `offline:<reel>` or the EDL's source file) that a
//! later conform or relink resolves.

use super::{ImportError, Imported, Report, dry_run, fit_transitions};
use crate::clip::{Clip, ClipSource, Grade, MediaRef, Speed};
use crate::edit::{Edit, EditError};
use crate::id::{ClipId, LaneId, MarkerId, MediaId, SequenceId, TransitionId};
use crate::media::{AudioInfo, MediaInfo, MediaItem, MediaKind, VideoInfo};
use crate::project::Project;
use crate::sequence::{Lane, LaneKind, Marker, Sequence, Transition, TransitionAlign};
use crate::time::{FrameRate, Micros, SECOND};
use crate::value::Value;
use std::collections::HashMap;
use std::fmt::Write;

/// Export settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdlOptions {
    /// Record timecode of the sequence start (commonly 01:00:00:00).
    pub record_start: Micros,
    /// Use drop-frame timecode for 29.97 and 59.94 fps sequences.
    pub drop_frame: bool,
}

impl Default for EdlOptions {
    fn default() -> Self {
        Self { record_start: 3600 * SECOND, drop_frame: true }
    }
}

/// Exports the spine of `seq` as CMX3600 text.
pub fn export(project: &Project, seq: SequenceId, opts: EdlOptions) -> Result<(String, Report), EditError> {
    let s = project.sequence(seq).ok_or_else(|| EditError::NotFound(seq.to_string()))?;
    let rate = project.settings.frame_rate;
    let tc = Timecode::new(rate, opts.drop_frame);
    let mut report = Report::default();
    let mut out = String::new();
    let title: String = s.name.chars().filter(|c| !c.is_control()).take(70).collect();
    writeln!(out, "TITLE: {title}").ok();
    writeln!(out, "FCM: {}", if tc.is_drop_frame() { "DROP FRAME" } else { "NON-DROP FRAME" }).ok();
    writeln!(out).ok();

    let others = s.clips.iter().filter(|c| c.lane != s.spine_id()).count();
    if others > 0 {
        report.add(&s.name, format!("{others} clip(s) on lanes other than the spine are not in the EDL"));
    }

    let spine = s.spine_clips();
    let mut event = 0;
    let rec_start = tc.label_frames(opts.record_start);
    let rec = |t: Micros| tc.format_frames(rec_start + rate.micros_to_frame_round(t));
    for (i, c) in spine.iter().enumerate() {
        if c.is_gap() {
            continue;
        }
        let name = if c.name.is_empty() { format!("clip {}", c.id.0) } else { c.name.clone() };
        let Some((reel, path)) = reel_of(project, c, &mut report, &name) else { continue };
        let channels = channels_of(project, c);
        note_lossy(c, &name, &mut report);
        if !c.markers.is_empty() {
            report.add(&name, "clip markers are not exported");
        }

        // A dissolve into this clip starts `pre` before the cut; the
        // previous event was cut short there.
        let incoming = s.transitions.iter().find(|t| t.right == c.id);
        let outgoing_pre = s.transitions.iter().find(|t| t.left == c.id).map_or(0, |t| t.split().0);
        let start_shift = incoming.map_or(0, |t| t.split().0);
        let rec_in = c.start - start_shift;
        let rec_out = c.end() - outgoing_pre;
        let origin = media_origin(project, c);
        let src_in = origin + source_at(c, rec_in);
        let src_out = origin + source_at(c, rec_out);
        event += 1;
        match incoming {
            Some(t) if t.kind == "dissolve" && i > 0 && !spine[i - 1].is_gap() => {
                let prev = spine[i - 1];
                let prev_reel = reel_of(project, prev, &mut Report::default(), "").map_or("BL".to_string(), |r| r.0);
                let at = media_origin(project, prev) + source_at(prev, rec_in);
                let frames = rate.micros_to_frame_round(t.duration);
                writeln!(out, "{event:03}  {prev_reel:<8} {:<5} C        {} {} {} {}", channels_of(project, prev), tc.format(at), tc.format(at), rec(rec_in), rec(rec_in)).ok();
                writeln!(out, "{event:03}  {reel:<8} {channels:<5} D    {frames:03} {} {} {} {}", tc.format(src_in), tc.format(src_out), rec(rec_in), rec(rec_out)).ok();
            }
            other => {
                if let Some(t) = other {
                    report.add(&name, format!("transition `{}` exported as a cut", t.kind));
                }
                writeln!(out, "{event:03}  {reel:<8} {channels:<5} C        {} {} {} {}", tc.format(src_in), tc.format(src_out), rec(rec_in), rec(rec_out)).ok();
            }
        }
        let speed = match &c.speed {
            Speed::Constant(r) => *r,
            Speed::Curve(_) => {
                report.add(&name, "speed curve exported as its average speed");
                c.source_len() as f64 / c.duration as f64
            }
        };
        if speed != 1.0 || c.reversed {
            let fps = rate.as_f64() * speed * if c.reversed { -1.0 } else { 1.0 };
            writeln!(out, "M2   {reel:<8}       {fps:05.1}                {}", tc.format(src_in)).ok();
        }
        if let Some((sop, sat)) = clip_cdl(c) {
            writeln!(out, "*ASC_SOP ({:.6} {:.6} {:.6})({:.6} {:.6} {:.6})({:.6} {:.6} {:.6})", sop[0], sop[1], sop[2], sop[3], sop[4], sop[5], sop[6], sop[7], sop[8]).ok();
            writeln!(out, "*ASC_SAT {sat:.6}").ok();
        }
        writeln!(out, "* FROM CLIP NAME: {name}").ok();
        if let Some(p) = path {
            writeln!(out, "* SOURCE FILE: {p}").ok();
        }
    }
    for m in &s.markers {
        let name: String = m.name.chars().filter(|c| !c.is_control()).collect();
        writeln!(out, "* LOC: {} {:<7} {name}", rec(m.time), marker_colour_name(m)).ok();
    }
    Ok((out, report))
}

/// Source time of a clip's media at its file start: the media's first-frame
/// timecode as time of day (0 when it has none).
fn media_origin(p: &Project, c: &Clip) -> Micros {
    match &c.source {
        ClipSource::Media(r) => p.media_item(r.media).map_or(0, |m| m.timecode_start(p.settings.frame_rate)),
        _ => 0,
    }
}

/// An exact ASC CDL on a clip's grade: (slope, offset, power) and sat.
fn clip_cdl(c: &Clip) -> Option<([f64; 9], f64)> {
    let g = c.grade.as_ref()?;
    let num = |k: &str| match g.params.get(k) {
        Some(Value::Scalar(v)) => Some(*v),
        _ => None,
    };
    let keys = ["cdl_slope_r", "cdl_slope_g", "cdl_slope_b", "cdl_offset_r", "cdl_offset_g", "cdl_offset_b", "cdl_power_r", "cdl_power_g", "cdl_power_b"];
    if keys.iter().all(|k| num(k).is_none()) {
        return None;
    }
    let mut sop = [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    for (i, k) in keys.iter().enumerate() {
        if let Some(v) = num(k) {
            sop[i] = v;
        }
    }
    Some((sop, num("cdl_sat").unwrap_or(1.0)))
}

fn marker_colour_name(m: &Marker) -> &'static str {
    let c = m.colour;
    let (r, g, b) = (c.r, c.g, c.b);
    if r > 0.6 && g < 0.4 && b < 0.4 {
        "RED"
    } else if g > 0.6 && r < 0.4 && b < 0.4 {
        "GREEN"
    } else if b > 0.6 && r < 0.4 && g < 0.6 {
        "BLUE"
    } else if r > 0.6 && g > 0.6 && b < 0.4 {
        "YELLOW"
    } else if g > 0.6 && b > 0.6 && r < 0.4 {
        "CYAN"
    } else if r > 0.6 && b > 0.6 && g < 0.4 {
        "MAGENTA"
    } else {
        "WHITE"
    }
}

/// Source time at record time `t` (which may lie in a transition handle
/// just outside the clip).
fn source_at(c: &Clip, t: Micros) -> Micros {
    let rate = c.speed.rate_at(0);
    let off = (t - c.start) as f64 * rate;
    if c.reversed { (c.source_out() as f64 - off).round() as Micros } else { (c.source_in as f64 + off).round() as Micros }
}

/// Reel name (8 characters, upper-case letters and digits) and source path.
fn reel_of(p: &Project, c: &Clip, report: &mut Report, name: &str) -> Option<(String, Option<String>)> {
    let clean = |s: &str| -> String {
        let r: String = s.chars().filter(|ch| ch.is_ascii_alphanumeric()).map(|ch| ch.to_ascii_uppercase()).take(8).collect();
        if r.is_empty() { "AX".into() } else { r }
    };
    match &c.source {
        ClipSource::Media(r) => {
            let m = p.media_item(r.media)?;
            let stem = m.name.rsplit_once('.').map_or(m.name.as_str(), |x| x.0);
            let reel = m.reel.as_deref().map(reel_name).unwrap_or_else(|| clean(stem));
            Some((reel, Some(m.path.clone())))
        }
        ClipSource::Sequence(r) => {
            report.add(name, "compound clip exported as one source named after its sequence");
            Some((clean(&p.sequence(r.sequence).map_or("COMPOUND".into(), |s| s.name.clone())), None))
        }
        ClipSource::Generator(_) => {
            report.add(name, "generated clip exported as black (BL)");
            Some(("BL".into(), None))
        }
        ClipSource::Gap => None,
    }
}

/// A reel name as an EDL holds it: letters, digits and `_`, at most 32
/// characters (CMX3600 has 8; longer names are read by every current
/// editor), upper-case.
pub fn reel_name(s: &str) -> String {
    let r: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').map(|c| c.to_ascii_uppercase()).take(32).collect();
    if r.is_empty() { "AX".into() } else { r }
}

fn channels_of(p: &Project, c: &Clip) -> &'static str {
    match &c.source {
        ClipSource::Media(r) => {
            let m = p.media_item(r.media);
            let v = r.video && m.is_none_or(|m| m.has_video());
            let a = r.audio && m.is_some_and(|m| m.has_audio());
            match (v, a) {
                (true, true) => "B",
                (false, true) => "A",
                _ => "V",
            }
        }
        _ => "V",
    }
}

fn note_lossy(c: &Clip, name: &str, report: &mut Report) {
    let grade_beyond_cdl = c.grade.as_ref().is_some_and(|g| g.lut.is_some() || g.params.keys().any(|k| !k.starts_with("cdl_")));
    if !c.effects.is_empty() || grade_beyond_cdl || !c.masks.is_empty() || !c.keyframes.is_empty() || c.transform != Default::default() {
        report.add(name, "effects, grade, masks, keyframes and transform are not exported");
    }
}

pub use crate::timecode::Timecode;

/// Import settings.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct EdlImport {
    /// Frame rate of the EDL's timecodes (default: the project's).
    pub rate: Option<FrameRate>,
}

#[derive(Clone, Debug, Default)]
struct Event {
    num: u32,
    reel: String,
    video: bool,
    audio: bool,
    /// Dissolve (or wipe / key, made a dissolve or cut) length in frames.
    dissolve: Option<i64>,
    src_in: i64,
    src_out: i64,
    rec_in: i64,
    rec_out: i64,
    name: Option<String>,
    file: Option<String>,
    speed: Option<f64>,
    sop: Option<[f64; 9]>,
    sat: Option<f64>,
}

/// Imports a CMX3600 EDL as a new sequence (see the module notes).
pub fn import(project: &mut Project, text: &str, opts: EdlImport) -> Result<Imported, ImportError> {
    let rate = opts.rate.unwrap_or(project.settings.frame_rate);
    let mut drop = false;
    let mut title = String::from("EDL");
    let mut events: Vec<Event> = Vec::new();
    let mut locs: Vec<(i64, String, String)> = Vec::new();
    let mut report = Report::default();
    let tc_parse = |t: &str, drop: bool| -> Option<i64> {
        let df = drop || t.contains(';');
        Timecode::new(rate, df).parse_lenient(t).or_else(|| Timecode::new(rate, !df).parse(t))
    };
    for (no, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("TITLE:") {
            title = rest.trim().to_string();
            continue;
        }
        if let Some(rest) = t.strip_prefix("FCM:") {
            drop = rest.trim().to_ascii_uppercase().starts_with("DROP");
            continue;
        }
        if let Some(rest) = t.strip_prefix('*') {
            let rest = rest.trim();
            let last = events.last_mut();
            let upper = rest.to_ascii_uppercase();
            if let Some(v) = upper.strip_prefix("FROM CLIP NAME:").map(|_| rest["FROM CLIP NAME:".len()..].trim().to_string()) {
                if let Some(e) = last {
                    e.name = Some(v);
                }
            } else if upper.starts_with("SOURCE FILE:") {
                if let Some(e) = last {
                    e.file = Some(rest["SOURCE FILE:".len()..].trim().to_string());
                }
            } else if let Some(v) = upper.strip_prefix("ASC_SOP") {
                let nums: Vec<f64> = v.replace(['(', ')'], " ").split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if nums.len() == 9 && let Some(e) = last {
                    e.sop = Some([nums[0], nums[1], nums[2], nums[3], nums[4], nums[5], nums[6], nums[7], nums[8]]);
                }
            } else if let Some(v) = upper.strip_prefix("ASC_SAT") {
                if let (Some(e), Ok(x)) = (last, v.trim().parse::<f64>()) {
                    e.sat = Some(x);
                }
            } else if upper.starts_with("LOC:") {
                let parts: Vec<&str> = rest["LOC:".len()..].split_whitespace().collect();
                if let Some(at) = parts.first().and_then(|p| tc_parse(p, drop)) {
                    let colour = parts.get(1).map(|c| c.to_string()).unwrap_or_default();
                    let name = parts.get(2..).map(|p| p.join(" ")).unwrap_or_default();
                    locs.push((at, colour, name));
                }
            }
            continue;
        }
        let tok: Vec<&str> = t.split_whitespace().collect();
        if tok.first() == Some(&"M2") {
            // M2 <reel> <speed fps> <source tc>
            if let (Some(e), Some(fps)) = (events.last_mut(), tok.get(2).and_then(|v| v.parse::<f64>().ok())) {
                e.speed = Some(fps / rate.as_f64());
            }
            continue;
        }
        let Some(num) = tok.first().and_then(|n| n.parse::<u32>().ok()) else {
            if !t.starts_with("SPLIT") && !t.starts_with("AUD") && !t.starts_with(">>>") {
                report.add(format!("line {}", no + 1), format!("not understood: {t}"));
            }
            continue;
        };
        // Number, reel, channels, transition [, length], four timecodes.
        let tcs: Vec<i64> = tok.iter().rev().take(4).filter_map(|v| tc_parse(v, drop)).collect();
        if tcs.len() != 4 || tok.len() < 7 {
            report.add(format!("event {num}"), "needs a reel, channels, a transition and four timecodes; skipped");
            continue;
        }
        let (rec_out, rec_in, src_out, src_in) = (tcs[0], tcs[1], tcs[2], tcs[3]);
        let channels = tok[2].to_ascii_uppercase();
        let kind = tok[3].to_ascii_uppercase();
        let video = channels.contains('V') || channels == "B";
        let audio = channels.starts_with('A') || channels.contains("/A") || channels == "B";
        let dissolve = match kind.as_str() {
            "C" => None,
            "D" => Some(tok.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0)),
            k if k.starts_with('W') || k.starts_with('K') => {
                report.add(format!("event {num}"), format!("transition {k} imported as a dissolve"));
                Some(tok.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0))
            }
            k => {
                report.add(format!("event {num}"), format!("transition {k} imported as a cut"));
                None
            }
        };
        events.push(Event { num, reel: tok[1].to_string(), video, audio, dissolve, src_in, src_out, rec_in, rec_out, ..Default::default() });
    }
    if events.is_empty() {
        return Err(ImportError::Unsupported("no events in the EDL".into()));
    }
    let seq_start = events.iter().map(|e| e.rec_in).min().unwrap_or(0);
    let us = |f: i64| rate.frame_to_micros(f);

    // Media for every reel: existing, or offline.
    let mut new_media: Vec<MediaItem> = Vec::new();
    let mut by_reel: HashMap<String, MediaId> = HashMap::new();
    for e in &events {
        if e.reel.eq_ignore_ascii_case("BL") || e.reel.eq_ignore_ascii_case("BLACK") || by_reel.contains_key(&e.reel) {
            continue;
        }
        let wanted = reel_name(&e.reel);
        let file_name = e.file.as_deref().map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f).to_string());
        let found = project
            .media
            .iter()
            .find(|m| m.reel.as_deref().map(reel_name).as_deref() == Some(wanted.as_str()))
            .or_else(|| project.media.iter().find(|m| e.file.as_deref() == Some(m.path.as_str())))
            .or_else(|| project.media.iter().find(|m| file_name.as_deref() == Some(m.name.as_str())))
            .or_else(|| project.media.iter().find(|m| reel_name(m.name.rsplit_once('.').map_or(m.name.as_str(), |x| x.0)) == wanted));
        let id = match found {
            Some(m) => m.id,
            None => {
                let id = MediaId(project.alloc_id());
                let reel_events: Vec<&Event> = events.iter().filter(|x| x.reel == e.reel).collect();
                // The frames the events use (speed changes use more or
                // less source than record time), plus handles for
                // transitions and trims.
                let handles = (rate.as_f64() * 5.0).round() as i64;
                let first = reel_events.iter().map(|x| x.src_in.min(x.src_out)).min().unwrap_or(0);
                let last = reel_events.iter().map(|x| x.src_in.min(x.src_out) + ((x.rec_out - x.rec_in) as f64 * x.speed.unwrap_or(1.0).abs()).ceil() as i64).max().unwrap_or(first + 1);
                let first = (first - handles).max(0);
                let last = last + handles;
                let any_video = reel_events.iter().any(|x| x.video);
                let any_audio = reel_events.iter().any(|x| x.audio);
                let info = MediaInfo {
                    kind: if any_video { MediaKind::Video } else { MediaKind::Audio },
                    container: String::new(),
                    duration: Some(us(last - first + 1)),
                    video: any_video.then(|| VideoInfo { codec: String::new(), width: project.settings.width, height: project.settings.height, frame_rate: Some(rate), ..Default::default() }),
                    audio: any_audio.then(|| AudioInfo { codec: String::new(), sample_rate: 48_000, channels: 2, ..Default::default() }),
                    extra: Default::default(),
                };
                let path = e.file.clone().unwrap_or_else(|| format!("offline:{}", e.reel));
                let mut item = MediaItem::new(id, path, info);
                item.name = e.name.clone().unwrap_or_else(|| e.reel.clone());
                item.reel = Some(e.reel.clone());
                // The offline item's timecode starts at the earliest source
                // frame the EDL uses, so source positions stay exact.
                item.timecode = Some(Timecode::new(rate, drop).format_frames(first));
                report.add(&e.reel, "reel not found in the project: added as offline media to relink");
                new_media.push(item);
                id
            }
        };
        by_reel.insert(e.reel.clone(), id);
    }
    let origins: HashMap<MediaId, i64> = by_reel
        .values()
        .map(|&id| {
            let item = project.media_item(id).or_else(|| new_media.iter().find(|m| m.id == id));
            (id, item.map_or(0, |m| rate.micros_to_frame_round(m.timecode_start(rate))))
        })
        .collect();
    let origin_of = |id: MediaId| -> i64 { origins.get(&id).copied().unwrap_or(0) };

    let seq_id = SequenceId(project.alloc_id());
    let spine = LaneId(project.alloc_id());
    let mut seq = Sequence::new(seq_id, title.clone(), spine);
    let audio_lane = LaneId(project.alloc_id());
    let mut used_audio = false;
    let mut spine_end = 0i64;
    let mut transitions: Vec<(ClipId, i64)> = Vec::new();
    let mut last_video: Option<ClipId> = None;
    for e in &events {
        let len = e.rec_out - e.rec_in;
        if len <= 0 {
            continue;
        }
        let start = e.rec_in - seq_start;
        let on_spine = e.video;
        let lane = if on_spine { spine } else { audio_lane };
        if !on_spine {
            used_audio = true;
        }
        let id = ClipId(project.alloc_id());
        let mut clip = match by_reel.get(&e.reel) {
            None => Clip::gap(id, lane, us(start), us(len)),
            Some(&m) => {
                let mut r = MediaRef::new(m);
                r.video = e.video;
                r.audio = e.audio;
                let mut c = Clip::new(id, lane, us(start), us(len), ClipSource::Media(r));
                let speed = e.speed.unwrap_or(1.0);
                c.reversed = speed < 0.0;
                if (speed.abs() - 1.0).abs() > 1e-6 {
                    c.speed = Speed::Constant(speed.abs());
                }
                c.source_in = us(e.src_in.min(e.src_out) - origin_of(m)).max(0);
                c.name = e.name.clone().unwrap_or_default();
                if e.sop.is_some() || e.sat.is_some() {
                    let mut g = Grade::default();
                    let sop = e.sop.unwrap_or([1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
                    let keys = ["cdl_slope_r", "cdl_slope_g", "cdl_slope_b", "cdl_offset_r", "cdl_offset_g", "cdl_offset_b", "cdl_power_r", "cdl_power_g", "cdl_power_b"];
                    for (k, v) in keys.iter().zip(sop) {
                        g.params.insert(k.to_string(), Value::Scalar(v));
                    }
                    g.params.insert("cdl_sat".into(), Value::Scalar(e.sat.unwrap_or(1.0)));
                    g.enabled = true;
                    c.grade = Some(g);
                }
                c
            }
        };
        clip.meta.insert("edl_event".into(), serde_json::json!({ "num": e.num, "reel": e.reel, "src_in": Timecode::new(rate, drop).format_frames(e.src_in) }));
        if on_spine {
            if start > spine_end {
                seq.clips.push(Clip::gap(ClipId(project.alloc_id()), spine, us(spine_end), us(start - spine_end)));
            }
            spine_end = spine_end.max(start + len);
            if let (Some(d), Some(_)) = (e.dissolve, last_video) {
                if d > 0 {
                    transitions.push((clip.id, d));
                }
            }
            last_video = Some(clip.id);
        }
        seq.clips.push(clip);
    }
    if used_audio {
        seq.lanes.push(Lane::new(audio_lane, LaneKind::Audio, "Audio 1"));
    }
    // Dissolves: the incoming event starts where the dissolve starts;
    // Studio centres the transition on the cut, so the cut moves to the
    // dissolve's middle (the pictures shown stay the same).
    let mut ts = Vec::new();
    for (right, frames) in transitions {
        let Some(ri) = seq.clips.iter().position(|c| c.id == right) else { continue };
        let Some(li) = seq.clips.iter().position(|c| c.lane == spine && c.end() == seq.clips[ri].start) else { continue };
        let left = seq.clips[li].id;
        let d = us(frames);
        let half = d / 2;
        seq.clips[li].duration += half;
        let rc = &mut seq.clips[ri];
        rc.start += half;
        rc.duration -= half;
        rc.source_in += (half as f64 * rc.speed.rate_at(0)).round() as Micros;
        ts.push(Transition { id: TransitionId(project.alloc_id()), left, right, kind: "dissolve".into(), duration: d, align: TransitionAlign::Centre, params: Default::default(), extra: Default::default() });
    }
    for (at, colour, name) in locs {
        let mut m = Marker::new(MarkerId(project.alloc_id()), us(at - seq_start), name);
        m.note = colour;
        seq.markers.push(m);
    }
    seq.normalize();
    fit_transitions(project, &new_media, &[], &mut seq, ts, &mut report);
    let mut edits: Vec<Edit> = new_media.into_iter().map(|m| Edit::InsertMedia { item: Box::new(m), index: None }).collect();
    edits.push(Edit::InsertSequence { sequence: Box::new(seq), index: None });
    let edit = Edit::Batch { edits };
    dry_run(project, &edit)?;
    Ok(Imported { edit, sequence: seq_id, report })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::FrameRate;

    #[test]
    fn non_drop_timecode() {
        let tc = Timecode::new(FrameRate::FPS_25, true);
        assert_eq!(tc.format(0), "00:00:00:00");
        assert_eq!(tc.format(3600 * SECOND + 40_000 * 3), "01:00:00:03");
    }

    #[test]
    fn drop_frame_timecode() {
        let r = FrameRate::FPS_29_97;
        let tc = Timecode::new(r, true);
        // Frame 1800 is the first frame of minute 1, labelled ;02.
        assert_eq!(tc.format(r.frame_to_micros(1799)), "00:00:59;29");
        assert_eq!(tc.format(r.frame_to_micros(1800)), "00:01:00;02");
        // Ten minutes is 17982 frames and is not dropped.
        assert_eq!(tc.format(r.frame_to_micros(17982)), "00:10:00;00");
        assert_eq!(Timecode::new(r, false).format(r.frame_to_micros(1800)), "00:01:00:00");
    }
}
