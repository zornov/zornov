// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenTimelineIO JSON import and export.
//!
//! Mapping:
//!
//! | Studio                  | OTIO                                          |
//! |-------------------------|-----------------------------------------------|
//! | sequence                | `Timeline.1` whose `tracks` is a `Stack.1`    |
//! | spine, visual lanes     | `Track.1` kind `Video` (spine first, upwards) |
//! | audio lanes             | `Track.1` kind `Audio` (nearest spine first)  |
//! | media clip              | `Clip.2` + `ExternalReference.1`              |
//! | generator clip          | `Clip.2` + `GeneratorReference.1`             |
//! | gap / empty lane space  | `Gap.1`                                       |
//! | compound clip           | nested `Stack.1` with a `source_range`        |
//! | transition              | `Transition.1` (`in_offset` before the cut)   |
//! | speed / reverse         | `LinearTimeWarp.1` / `FreezeFrame.1`          |
//! | markers                 | `Marker.2` (clip markers in clip source time) |
//!
//! Each OTIO object also carries the full Studio object under
//! `metadata.studio`, so a Studio → OTIO → Studio round trip keeps
//! effects, grades, masks, keyframes and the rest. Other tools ignore that
//! metadata; the export report lists what they will not see.
//!
//! Clip `source_range.duration` is the clip's length on the track; the
//! time warp says how fast media is consumed, as OTIO defines it.

use super::{ImportError, Imported, Report, dry_run, path_to_url, url_to_path};
use crate::clip::{Clip, ClipSource, Generator, GeneratorKind, MediaRef, SequenceRef, Speed};
use crate::edit::{Edit, EditError};
use crate::id::*;
use crate::media::{MediaInfo, MediaItem, MediaKind};
use crate::project::Project;
use crate::sequence::{Lane, LaneKind, Marker, Sequence, Transition, TransitionAlign};
use crate::time::Micros;
use crate::value::Rgba;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------- export

/// Exports sequence `seq` as an OTIO JSON string.
pub fn export(project: &Project, seq: SequenceId) -> Result<(String, Report), EditError> {
    let s = project.sequence(seq).ok_or_else(|| EditError::NotFound(seq.to_string()))?;
    let rate = project.settings.frame_rate.as_f64();
    let mut report = Report::default();
    let stack = export_stack(project, s, rate, &mut report, None);
    let doc = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": s.name,
        "global_start_time": null,
        "metadata": { "studio": { "settings": project.settings, "sequence_extra": s.extra } },
        "tracks": stack,
    });
    Ok((serde_json::to_string_pretty(&doc).expect("json"), report))
}

fn rt(t: Micros, rate: f64) -> Value {
    json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": t as f64 * rate / 1e6 })
}

fn range(start: Micros, dur: Micros, rate: f64) -> Value {
    json!({ "OTIO_SCHEMA": "TimeRange.1", "start_time": rt(start, rate), "duration": rt(dur, rate) })
}

/// A sequence as a `Stack.1`; `clip` is the compound clip when nested.
fn export_stack(p: &Project, s: &Sequence, rate: f64, report: &mut Report, clip: Option<&Clip>) -> Value {
    let spine_idx = s.lanes.iter().position(|l| l.kind == LaneKind::Spine).expect("spine");
    // Bottom to top for video (spine first), then audio nearest-first.
    let mut tracks = Vec::new();
    for idx in (0..=spine_idx).rev() {
        tracks.push(export_track(p, s, &s.lanes[idx], rate, report));
    }
    for lane in &s.lanes[spine_idx + 1..] {
        tracks.push(export_track(p, s, lane, rate, report));
    }
    let mut stack = json!({
        "OTIO_SCHEMA": "Stack.1",
        "name": clip.map_or(s.name.clone(), |c| c.name.clone()),
        "children": tracks,
        "effects": [],
        "markers": s.markers.iter().map(|m| export_marker(m, m.time, rate)).collect::<Vec<_>>(),
        "metadata": {},
        "source_range": null,
    });
    if let Some(c) = clip {
        stack["source_range"] = range(c.source_in, c.duration, rate);
        stack["metadata"] = json!({ "studio": { "clip": c, "sequence_name": s.name, "sequence_extra": s.extra } });
        stack["effects"] = Value::Array(time_warp(c));
        stack["markers"] = Value::Array(c.markers.iter().map(|m| export_marker(m, c.source_in + m.time, rate)).collect());
        // A nested stack needs the nested sequence's own markers somewhere;
        // keep them in metadata (OTIO has one marker list per item).
        stack["metadata"]["studio"]["sequence_markers"] = json!(s.markers);
    }
    stack
}

fn export_track(p: &Project, s: &Sequence, lane: &Lane, rate: f64, report: &mut Report) -> Value {
    let kind = if lane.kind == LaneKind::Audio { "Audio" } else { "Video" };
    let mut children = Vec::new();
    let mut cursor = 0;
    for c in s.clips_on(lane.id) {
        if c.start > cursor {
            children.push(gap(c.start - cursor, rate));
        }
        cursor = c.end();
        children.push(export_item(p, c, rate, report));
        if let Some(t) = s.transitions.iter().find(|t| t.left == c.id) {
            let (pre, post) = t.split();
            let ty = if t.kind == "dissolve" { "SMPTE_Dissolve" } else { "Custom_Transition" };
            if ty != "SMPTE_Dissolve" {
                report.add(format!("transition {}", t.kind), "exported as a custom transition; other tools may show a dissolve");
            }
            children.push(json!({
                "OTIO_SCHEMA": "Transition.1",
                "name": t.kind,
                "transition_type": ty,
                "in_offset": rt(pre, rate),
                "out_offset": rt(post, rate),
                "metadata": { "studio": { "transition": t } },
            }));
        }
    }
    json!({
        "OTIO_SCHEMA": "Track.1",
        "name": lane.name,
        "kind": kind,
        "children": children,
        "effects": [],
        "markers": [],
        "metadata": { "studio": { "lane": lane } },
        "source_range": null,
    })
}

fn gap(dur: Micros, rate: f64) -> Value {
    json!({ "OTIO_SCHEMA": "Gap.1", "name": "", "source_range": range(0, dur, rate), "effects": [], "markers": [], "metadata": {} })
}

fn time_warp(c: &Clip) -> Vec<Value> {
    let scalar = match &c.speed {
        Speed::Constant(r) => *r,
        Speed::Curve(_) => c.source_len() as f64 / c.duration as f64,
    };
    if scalar == 0.0 {
        return vec![json!({ "OTIO_SCHEMA": "FreezeFrame.1", "name": "", "effect_name": "FreezeFrame", "time_scalar": 0.0, "metadata": {} })];
    }
    let scalar = if c.reversed { -scalar } else { scalar };
    if scalar == 1.0 {
        return Vec::new();
    }
    vec![json!({ "OTIO_SCHEMA": "LinearTimeWarp.1", "name": "", "effect_name": "LinearTimeWarp", "time_scalar": scalar, "metadata": {} })]
}

fn export_item(p: &Project, c: &Clip, rate: f64, report: &mut Report) -> Value {
    let name = if c.name.is_empty() { format!("clip {}", c.id.0) } else { c.name.clone() };
    note_lossy(c, &name, report);
    let reference = match &c.source {
        ClipSource::Gap => {
            let mut g = gap(c.duration, rate);
            g["metadata"] = json!({ "studio": { "clip": c } });
            return g;
        }
        ClipSource::Sequence(r) => match p.sequence(r.sequence) {
            Some(nested) => return export_stack(p, nested, rate, report, Some(c)),
            None => json!({ "OTIO_SCHEMA": "MissingReference.1", "name": "", "metadata": {}, "available_range": null }),
        },
        ClipSource::Media(r) => match p.media_item(r.media) {
            Some(m) => json!({
                "OTIO_SCHEMA": "ExternalReference.1",
                "name": m.name,
                "target_url": path_to_url(&m.path),
                "available_range": m.duration().map_or(Value::Null, |d| range(0, d, rate)),
                "metadata": { "studio": { "media": m } },
            }),
            None => json!({ "OTIO_SCHEMA": "MissingReference.1", "name": "", "metadata": {}, "available_range": null }),
        },
        ClipSource::Generator(g) => json!({
            "OTIO_SCHEMA": "GeneratorReference.1",
            "name": "",
            "generator_kind": match g.kind {
                GeneratorKind::Solid => "SolidColor",
                GeneratorKind::Gradient => "Gradient",
                GeneratorKind::Text => "Text",
                GeneratorKind::Shape => "Shape",
                GeneratorKind::Adjustment => "Adjustment",
                GeneratorKind::Camera => "Camera",
                GeneratorKind::Light => "Light",
                GeneratorKind::Model => "Model",
            },
            "parameters": serde_json::to_value(&g.params).expect("json"),
            "available_range": null,
            "metadata": {},
        }),
    };
    json!({
        "OTIO_SCHEMA": "Clip.2",
        "name": c.name,
        "enabled": c.enabled,
        "source_range": range(c.source_in, c.duration, rate),
        "media_references": { "DEFAULT_MEDIA": reference },
        "active_media_reference_key": "DEFAULT_MEDIA",
        "effects": time_warp(c),
        "markers": c.markers.iter().map(|m| export_marker(m, c.source_in + m.time, rate)).collect::<Vec<_>>(),
        "metadata": { "studio": { "clip": c } },
    })
}

/// Reports clip features other OTIO readers will not understand.
fn note_lossy(c: &Clip, name: &str, report: &mut Report) {
    let mut lost = Vec::new();
    if !c.effects.is_empty() || !c.audio.effects.is_empty() {
        lost.push("effects");
    }
    if c.grade.is_some() {
        lost.push("colour grade");
    }
    if !c.masks.is_empty() {
        lost.push("masks");
    }
    if !c.keyframes.is_empty() {
        lost.push("keyframes");
    }
    if c.transform != Default::default() {
        lost.push("transform");
    }
    if c.blend != Default::default() {
        lost.push("blend mode");
    }
    if c.motion != Default::default() {
        lost.push("motion presets");
    }
    if c.audio.volume_db != 0.0 || c.audio.pan != 0.0 || c.audio.fade_in.duration != 0 || c.audio.fade_out.duration != 0 {
        lost.push("audio levels and fades");
    }
    if matches!(c.speed, Speed::Curve(_)) {
        lost.push("speed curve (exported as its average speed)");
    }
    if !lost.is_empty() {
        report.add(name, format!("{} kept only in Zornov metadata", lost.join(", ")));
    }
}

const COLOURS: [(&str, [f64; 3]); 11] = [
    ("RED", [1.0, 0.0, 0.0]),
    ("PINK", [1.0, 0.6, 0.8]),
    ("ORANGE", [1.0, 0.55, 0.0]),
    ("YELLOW", [1.0, 1.0, 0.0]),
    ("GREEN", [0.0, 0.8, 0.0]),
    ("CYAN", [0.0, 1.0, 1.0]),
    ("BLUE", [0.0, 0.3, 1.0]),
    ("PURPLE", [0.5, 0.0, 0.8]),
    ("MAGENTA", [1.0, 0.0, 1.0]),
    ("BLACK", [0.0, 0.0, 0.0]),
    ("WHITE", [1.0, 1.0, 1.0]),
];

fn colour_name(c: Rgba) -> &'static str {
    COLOURS
        .iter()
        .min_by(|a, b| {
            let d = |x: &[f64; 3]| (x[0] - c.r).powi(2) + (x[1] - c.g).powi(2) + (x[2] - c.b).powi(2);
            d(&a.1).total_cmp(&d(&b.1))
        })
        .map_or("BLUE", |x| x.0)
}

fn colour_from_name(name: &str) -> Rgba {
    COLOURS.iter().find(|x| x.0.eq_ignore_ascii_case(name)).map_or(Marker::DEFAULT_COLOUR, |x| Rgba::new(x.1[0], x.1[1], x.1[2], 1.0))
}

fn export_marker(m: &Marker, at: Micros, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "Marker.2",
        "name": m.name,
        "marked_range": range(at, m.duration, rate),
        "color": colour_name(m.colour),
        "comment": m.note,
        "metadata": { "studio": { "marker": m } },
    })
}

// ---------------------------------------------------------------- import

/// Imports an OTIO JSON timeline. Media is matched to existing bin items by
/// path; new items are created for the rest. `project` is only used for
/// lookups and id allocation; apply the returned edit to add the result.
pub fn import(project: &mut Project, text: &str) -> Result<Imported, ImportError> {
    let doc: Value = serde_json::from_str(text).map_err(|e| ImportError::Parse(e.to_string()))?;
    let mut report = Report::default();
    let timeline = match schema(&doc) {
        "Timeline" => doc.clone(),
        "SerializableCollection" => {
            let children = doc["children"].as_array().cloned().unwrap_or_default();
            let tl = children.iter().find(|c| schema(c) == "Timeline").cloned().ok_or_else(|| ImportError::Unsupported("collection holds no timeline".into()))?;
            if children.len() > 1 {
                report.add("collection", "only the first timeline was imported");
            }
            tl
        }
        other => return Err(ImportError::Unsupported(format!("expected a Timeline, found `{other}`"))),
    };
    let mut imp = Importer { p: project, report, media: Vec::new(), nested: Vec::new(), links: HashMap::new(), groups: HashMap::new(), pending_transitions: Vec::new(), audio_lanes: HashSet::new() };
    let name = timeline["name"].as_str().unwrap_or("Imported").to_string();
    let mut seq = imp.stack(&timeline["tracks"], &name)?;
    if let Some(extra) = timeline.pointer("/metadata/studio/sequence_extra").and_then(|v| serde_json::from_value(v.clone()).ok()) {
        seq.extra = extra;
    }
    imp.fit_transitions(&mut seq);
    let main = seq.id;
    let mut edits: Vec<Edit> = imp.media.into_iter().map(|m| Edit::InsertMedia { item: Box::new(m), index: None }).collect();
    edits.extend(imp.nested.into_iter().map(|s| Edit::InsertSequence { sequence: Box::new(s), index: None }));
    edits.push(Edit::InsertSequence { sequence: Box::new(seq), index: None });
    let edit = Edit::Batch { edits };
    dry_run(imp.p, &edit)?;
    Ok(Imported { edit, sequence: main, report: imp.report })
}

/// Schema name without its version (`"Clip.2"` → `"Clip"`).
fn schema(v: &Value) -> &str {
    v["OTIO_SCHEMA"].as_str().map_or("", |s| s.split('.').next().unwrap_or(s))
}

fn time(v: &Value) -> Option<Micros> {
    let rate = v["rate"].as_f64()?;
    let value = v["value"].as_f64()?;
    (rate > 0.0).then(|| (value / rate * 1e6).round() as Micros)
}

fn time_range(v: &Value) -> Option<(Micros, Micros)> {
    Some((time(&v["start_time"])?, time(&v["duration"])?))
}

fn studio<T: serde::de::DeserializeOwned>(v: &Value, key: &str) -> Option<T> {
    serde_json::from_value(v.pointer(&format!("/metadata/studio/{key}"))?.clone()).ok()
}

struct Importer<'a> {
    p: &'a mut Project,
    report: Report,
    media: Vec<MediaItem>,
    nested: Vec<Sequence>,
    links: HashMap<LinkId, LinkId>,
    groups: HashMap<GroupId, GroupId>,
    /// Transitions found while reading, fitted once their sequence is complete.
    pending_transitions: Vec<(SequenceId, Transition)>,
    /// Lanes created for audio tracks (media found there is sound only).
    audio_lanes: HashSet<LaneId>,
}

/// A transition waiting for the clip after it.
struct PendingTransition {
    left: ClipId,
    pre: Micros,
    post: Micros,
    item: Value,
}

impl Importer<'_> {
    fn stack(&mut self, stack: &Value, name: &str) -> Result<Sequence, ImportError> {
        if schema(stack) != "Stack" {
            return Err(ImportError::Unsupported("timeline tracks are not a Stack".into()));
        }
        let seq_id = SequenceId(self.p.alloc_id());
        let spine_id = LaneId(self.p.alloc_id());
        let mut seq = Sequence::new(seq_id, name, spine_id);
        let tracks: Vec<&Value> = stack["children"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        for t in &tracks {
            if schema(t) != "Track" {
                self.report.add(format!("{} in stack", schema(t)), "only tracks are supported directly inside a stack; skipped");
            }
        }
        let video: Vec<&Value> = tracks.iter().copied().filter(|t| schema(t) == "Track" && t["kind"].as_str() != Some("Audio")).collect();
        let audio: Vec<&Value> = tracks.iter().copied().filter(|t| schema(t) == "Track" && t["kind"].as_str() == Some("Audio")).collect();
        let spine_track = video.iter().position(|t| studio::<Lane>(t, "lane").is_some_and(|l| l.kind == LaneKind::Spine)).unwrap_or(0);
        if let Some(t) = video.get(spine_track)
            && let Some(l) = studio::<Lane>(t, "lane")
        {
            seq.lanes[0] = Lane { id: spine_id, kind: LaneKind::Spine, ..l };
        }
        let mut lane_of_track: Vec<(LaneId, &Value)> = Vec::new();
        if let Some(t) = video.get(spine_track) {
            lane_of_track.push((spine_id, t));
        }
        for (i, t) in video.iter().enumerate().filter(|(i, _)| *i != spine_track) {
            let lane = self.lane(t, LaneKind::Visual, format!("Video {i}"));
            lane_of_track.push((lane.id, t));
            seq.lanes.insert(0, lane);
        }
        for (i, t) in audio.iter().enumerate() {
            let lane = self.lane(t, LaneKind::Audio, format!("Audio {}", i + 1));
            self.audio_lanes.insert(lane.id);
            lane_of_track.push((lane.id, t));
            seq.lanes.push(lane);
        }
        for (lane, track) in lane_of_track {
            self.track(&mut seq, lane, track)?;
        }
        for m in stack["markers"].as_array().into_iter().flatten() {
            let at = time_range(&m["marked_range"]).map_or(0, |r| r.0);
            seq.markers.push(self.marker(m, at));
        }
        seq.normalize();
        Ok(seq)
    }

    fn lane(&mut self, track: &Value, kind: LaneKind, default_name: String) -> Lane {
        let id = LaneId(self.p.alloc_id());
        match studio::<Lane>(track, "lane") {
            Some(l) => Lane { id, kind, ..l },
            None => Lane::new(id, kind, track["name"].as_str().filter(|n| !n.is_empty()).map_or(default_name, str::to_string)),
        }
    }

    fn track(&mut self, seq: &mut Sequence, lane: LaneId, track: &Value) -> Result<(), ImportError> {
        let spine = lane == seq.spine_id();
        let mut cursor: Micros = 0;
        let mut pending: Option<PendingTransition> = None;
        let mut last_clip: Option<ClipId> = None;
        let track_name = track["name"].as_str().unwrap_or("track").to_string();
        for item in track["children"].as_array().into_iter().flatten() {
            let kind = schema(item);
            if kind == "Transition" {
                let pre = time(&item["in_offset"]).unwrap_or(0);
                let post = time(&item["out_offset"]).unwrap_or(0);
                match (spine, last_clip) {
                    (true, Some(left)) => pending = Some(PendingTransition { left, pre, post, item: item.clone() }),
                    _ => self.report.add(format!("transition on {track_name}"), "transitions are only supported between spine clips; dropped"),
                }
                continue;
            }
            let Some((src_start, dur)) = time_range(&item["source_range"]).or_else(|| self.available_range(item)) else {
                self.report.add(format!("{kind} on {track_name}"), "item has no duration; skipped");
                continue;
            };
            if dur <= 0 {
                continue;
            }
            let clip = match kind {
                "Gap" => {
                    if spine {
                        let mut g = studio::<Clip>(item, "clip").filter(Clip::is_gap).unwrap_or_else(|| Clip::gap(ClipId(0), lane, 0, dur));
                        g.id = ClipId(self.p.alloc_id());
                        g.lane = lane;
                        g.start = cursor;
                        g.duration = dur;
                        g.markers.clear();
                        Some(g)
                    } else {
                        None
                    }
                }
                "Clip" => Some(self.clip(item, lane, cursor, src_start, dur)?),
                "Stack" => {
                    let nested_name = studio::<String>(item, "sequence_name").unwrap_or_else(|| item["name"].as_str().unwrap_or("Compound").to_string());
                    let mut nested = self.stack(item, &nested_name)?;
                    if let Some(markers) = studio::<Vec<Marker>>(item, "sequence_markers") {
                        nested.markers = markers.into_iter().map(|mut m| {
                            m.id = MarkerId(self.p.alloc_id());
                            m
                        }).collect();
                    }
                    if let Some(extra) = studio(item, "sequence_extra") {
                        nested.extra = extra;
                    }
                    self.fit_transitions(&mut nested);
                    let nested_id = nested.id;
                    self.nested.push(nested);
                    let mut c = self.base_clip(item, lane, cursor, src_start, dur);
                    c.source = ClipSource::Sequence(SequenceRef { sequence: nested_id, extra: Default::default() });
                    self.apply_effects(item, &mut c);
                    self.clip_markers(item, &mut c, src_start);
                    Some(c)
                }
                other => {
                    self.report.add(format!("{other} on {track_name}"), "unsupported item; replaced by empty space");
                    None
                }
            };
            if let Some(c) = clip {
                if let Some(t) = pending.take() {
                    self.transition(seq.id, t, c.id);
                }
                last_clip = Some(c.id);
                seq.clips.push(c);
            } else {
                if pending.take().is_some() {
                    self.report.add(format!("transition on {track_name}"), "transition next to empty space dropped");
                }
                last_clip = None;
                if spine {
                    // Unsupported item on the spine: keep the timing with a gap.
                    seq.clips.push(Clip::gap(ClipId(self.p.alloc_id()), lane, cursor, dur));
                }
            }
            cursor += dur;
        }
        if pending.is_some() {
            self.report.add(format!("transition on {track_name}"), "transition at the end of a track dropped");
        }
        Ok(())
    }

    fn available_range(&self, item: &Value) -> Option<(Micros, Micros)> {
        time_range(&self.reference(item)?["available_range"])
    }

    fn reference<'v>(&self, item: &'v Value) -> Option<&'v Value> {
        let key = item["active_media_reference_key"].as_str().unwrap_or("DEFAULT_MEDIA");
        item["media_references"].get(key).or_else(|| item.get("media_reference")).filter(|r| !r.is_null())
    }

    /// The Studio clip stored in metadata (or a fresh one) with OTIO's timing.
    fn base_clip(&mut self, item: &Value, lane: LaneId, start: Micros, src_start: Micros, dur: Micros) -> Clip {
        let mut c = studio::<Clip>(item, "clip").unwrap_or_else(|| Clip::new(ClipId(0), lane, start, dur, ClipSource::Gap));
        c.id = ClipId(self.p.alloc_id());
        c.lane = lane;
        c.start = start;
        c.duration = dur;
        c.source_in = src_start.max(0);
        if let Some(n) = item["name"].as_str() {
            c.name = n.to_string();
        }
        if let Some(e) = item["enabled"].as_bool() {
            c.enabled = e;
        }
        if let Some(l) = c.link {
            c.link = Some(*self.links.entry(l).or_insert_with(|| LinkId(self.p.alloc_id())));
        }
        if let Some(g) = c.group {
            c.group = Some(*self.groups.entry(g).or_insert_with(|| GroupId(self.p.alloc_id())));
        }
        c
    }

    fn clip(&mut self, item: &Value, lane: LaneId, start: Micros, src_start: Micros, dur: Micros) -> Result<Clip, ImportError> {
        let mut c = self.base_clip(item, lane, start, src_start, dur);
        let name = if c.name.is_empty() { "clip".to_string() } else { c.name.clone() };
        let reference = self.reference(item).cloned();
        let ref_kind = reference.as_ref().map_or("", schema);
        match ref_kind {
            "ExternalReference" => {
                let r = reference.expect("checked");
                let url = r["target_url"].as_str().unwrap_or_default();
                let audio_lane = self.audio_lanes.contains(&lane);
                let media = self.media_for(&url_to_path(url), &r, audio_lane);
                c.source = match c.source {
                    ClipSource::Media(m) => ClipSource::Media(MediaRef { media, ..m }),
                    _ => ClipSource::Media(MediaRef::new(media)),
                };
            }
            "GeneratorReference" => {
                if !matches!(c.source, ClipSource::Generator(_)) {
                    let r = reference.expect("checked");
                    let gk = r["generator_kind"].as_str().unwrap_or_default();
                    let kind = match gk {
                        "SolidColor" | "Solid" => GeneratorKind::Solid,
                        "Gradient" => GeneratorKind::Gradient,
                        "Text" => GeneratorKind::Text,
                        "Shape" => GeneratorKind::Shape,
                        "Adjustment" => GeneratorKind::Adjustment,
                        "Camera" => GeneratorKind::Camera,
                        "Light" => GeneratorKind::Light,
                        "Model" => GeneratorKind::Model,
                        _ => {
                            self.report.add(&name, format!("generator `{gk}` is unknown; imported as a solid"));
                            GeneratorKind::Solid
                        }
                    };
                    let mut g = Generator::new(kind);
                    if let Ok(params) = serde_json::from_value(r["parameters"].clone()) {
                        g.params = params;
                    } else if r["parameters"].as_object().is_some_and(|o| !o.is_empty()) {
                        self.report.add(&name, "generator parameters not understood; defaults used");
                    }
                    c.source = ClipSource::Generator(g);
                }
            }
            other => {
                if !matches!(c.source, ClipSource::Generator(_)) {
                    self.report.add(&name, format!("media reference `{}` is not supported; imported as a solid", if other.is_empty() { "none" } else { other }));
                    c.source = ClipSource::Generator(Generator::new(GeneratorKind::Solid));
                }
            }
        }
        self.apply_effects(item, &mut c);
        self.clip_markers(item, &mut c, src_start);
        // Media shorter than what the clip uses: trust the timeline.
        if let ClipSource::Media(r) = &c.source
            && let Some(m) = self.media.iter_mut().find(|m| m.id == r.media)
            && m.info.kind != MediaKind::Image
            && m.info.duration.is_some_and(|d| d < c.source_out())
        {
            m.info.duration = Some(c.source_out());
            self.report.add(&name, "clip uses more media than the reference's available range; range extended");
        }
        Ok(c)
    }

    fn apply_effects(&mut self, item: &Value, c: &mut Clip) {
        let had_curve = matches!(c.speed, Speed::Curve(_));
        for e in item["effects"].as_array().into_iter().flatten() {
            match schema(e) {
                "LinearTimeWarp" | "FreezeFrame" => {
                    let s = e["time_scalar"].as_f64().unwrap_or(1.0);
                    c.reversed = s < 0.0;
                    if !had_curve {
                        c.speed = Speed::Constant(s.abs());
                    }
                }
                other => self.report.add(if c.name.is_empty() { "clip".into() } else { c.name.clone() }, format!("effect `{}` not supported; dropped", e["effect_name"].as_str().unwrap_or(other))),
            }
        }
        if item["effects"].as_array().is_none_or(|a| a.is_empty()) && !had_curve {
            c.speed = Speed::Constant(1.0);
            c.reversed = false;
        }
    }

    fn clip_markers(&mut self, item: &Value, c: &mut Clip, src_start: Micros) {
        c.markers.clear();
        for m in item["markers"].as_array().into_iter().flatten() {
            let at = time_range(&m["marked_range"]).map_or(src_start, |r| r.0);
            let marker = self.marker(m, at - src_start);
            c.markers.push(marker);
        }
    }

    fn marker(&mut self, m: &Value, at: Micros) -> Marker {
        let dur = time_range(&m["marked_range"]).map_or(0, |r| r.1);
        let mut out = studio::<Marker>(m, "marker").unwrap_or_else(|| {
            let mut mk = Marker::new(MarkerId(0), at, m["name"].as_str().unwrap_or_default());
            mk.note = m["comment"].as_str().unwrap_or_default().to_string();
            mk.colour = colour_from_name(m["color"].as_str().unwrap_or("BLUE"));
            mk
        });
        out.id = MarkerId(self.p.alloc_id());
        out.time = at;
        out.duration = dur;
        out
    }

    fn media_for(&mut self, path: &str, r: &Value, audio_lane: bool) -> MediaId {
        if let Some(m) = self.p.media.iter().chain(&self.media).find(|m| m.path == path) {
            return m.id;
        }
        let id = MediaId(self.p.alloc_id());
        let mut item = match studio::<MediaItem>(r, "media") {
            Some(m) => MediaItem { id, path: path.to_string(), ..m },
            None => {
                let mut info = MediaInfo { kind: if audio_lane { MediaKind::Audio } else { MediaKind::Video }, ..Default::default() };
                info.duration = time_range(&r["available_range"]).map(|(s, d)| s.max(0) + d);
                self.report.add(path, "media was not probed; its stream details are unknown until it is re-imported");
                let mut m = MediaItem::new(id, path, info);
                if let Some(n) = r["name"].as_str().filter(|n| !n.is_empty()) {
                    m.name = n.to_string();
                }
                m
            }
        };
        item.relative_path = None;
        self.media.push(item);
        id
    }

    fn transition(&mut self, seq: SequenceId, t: PendingTransition, right: ClipId) {
        let base: Option<Transition> = studio(&t.item, "transition");
        let (duration, align) = match (t.pre, t.post) {
            (pre, 0) => (pre, TransitionAlign::EndAtCut),
            (0, post) => (post, TransitionAlign::StartAtCut),
            (pre, post) => {
                if (pre - post).abs() > 1 {
                    self.report.add("transition", "off-centre transition imported centred on the cut");
                }
                (pre + post, TransitionAlign::Centre)
            }
        };
        if duration <= 0 {
            return;
        }
        let kind = match t.item["transition_type"].as_str() {
            Some("SMPTE_Dissolve") => "dissolve".to_string(),
            _ => base.as_ref().map(|b| b.kind.clone()).unwrap_or_else(|| {
                self.report.add("transition", "unknown transition type imported as a dissolve");
                "dissolve".into()
            }),
        };
        let id = TransitionId(self.p.alloc_id());
        let mut tr = base.unwrap_or(Transition { id, left: t.left, right, kind: kind.clone(), duration, align, params: Default::default(), extra: Default::default() });
        tr.id = id;
        tr.left = t.left;
        tr.right = right;
        tr.kind = kind;
        tr.duration = duration;
        tr.align = align;
        self.pending_transitions.push((seq, tr));
    }

    /// Shortens or drops transitions that do not fit their clips' handles.
    fn fit_transitions(&mut self, seq: &mut Sequence) {
        let mine: Vec<Transition> = self.pending_transitions.iter().filter(|(s, _)| *s == seq.id).map(|(_, t)| t.clone()).collect();
        self.pending_transitions.retain(|(s, _)| *s != seq.id);
        super::fit_transitions(self.p, &self.media, &self.nested, seq, mine, &mut self.report);
    }

}
