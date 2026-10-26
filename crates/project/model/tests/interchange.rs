// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OTIO round trips, FCPXML import and EDL export.

mod common;
use common::*;
use serde_json::Value;
use studio_model::interchange::{edl, fcpxml, otio};
use studio_model::*;

/// A sequence as JSON with ids replaced by structure, so two sequences
/// built with different ids compare equal when they look the same.
fn shape(p: &Project, seq: SequenceId) -> Value {
    let s = p.sequence(seq).unwrap();
    let pos = |l: LaneId| s.lane_position(l).unwrap();
    let mut clips: Vec<Value> = s
        .clips
        .iter()
        .map(|c| {
            let mut c = c.clone();
            let lane = pos(c.lane);
            c.id = ClipId(0);
            c.lane = LaneId(0);
            c.link = c.link.map(|_| LinkId(1));
            c.group = c.group.map(|_| GroupId(1));
            for m in &mut c.markers {
                m.id = MarkerId(0);
            }
            let nested = c.sequence_id().map(|n| shape(p, n));
            if let ClipSource::Sequence(r) = &mut c.source {
                r.sequence = SequenceId(0);
            }
            if let ClipSource::Media(r) = &mut c.source {
                // Media is compared by path.
                let path = p.media_item(r.media).unwrap().path.clone();
                r.media = MediaId(0);
                c.meta.insert("test_media_path".into(), path.into());
            }
            serde_json::json!({ "lane": lane, "clip": c, "nested": nested })
        })
        .collect();
    clips.sort_by_key(|v| (v["lane"].as_i64(), v["clip"]["start"].as_i64()));
    let start_of = |id: ClipId| s.clip(id).map(|c| (pos(c.lane), c.start));
    let transitions: Vec<Value> = s.transitions.iter().map(|t| serde_json::json!({ "at": start_of(t.left), "kind": t.kind, "duration": t.duration, "align": t.align, "params": t.params })).collect();
    let markers: Vec<Value> = s.markers.iter().map(|m| serde_json::json!({ "time": m.time, "name": m.name, "note": m.note, "colour": m.colour, "duration": m.duration })).collect();
    let lanes: Vec<Value> = s.lanes.iter().map(|l| serde_json::json!({ "kind": l.kind, "name": l.name, "muted": l.muted, "locked": l.locked })).collect();
    serde_json::json!({ "name": s.name, "lanes": lanes, "clips": clips, "transitions": transitions, "markers": markers })
}

/// Panics with the first differing JSON path.
fn assert_same(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                assert_same(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), &format!("{path}/{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "length differs at {path}");
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                assert_same(p, q, &format!("{path}/{i}"));
            }
        }
        _ => assert_eq!(a, b, "differs at {path}"),
    }
}

/// The fixture plus speed, reverse, a compound clip, markers, effects and
/// a gap on the spine.
fn busy() -> Fx {
    let mut f = fixture();
    let seq = f.seq;
    f.p.apply(Edit::SetSpeed { seq, clip: f.b, speed: Speed::Constant(2.0) }).unwrap();
    f.p.apply(Edit::Reverse { seq, clips: vec![f.c] }).unwrap();
    f.p.apply(Edit::AddMarker { seq, clip: None, marker: Marker::new(MarkerId(0), 2 * S, "beat") }).unwrap();
    f.p.apply(Edit::AddMarker { seq, clip: Some(f.a), marker: Marker::new(MarkerId(0), S, "smile") }).unwrap();
    f.p.apply(Edit::AddEffect { seq, clip: f.title, stack: Stack::Video, effect: Effect::new(EffectId(0), "glow"), index: None }).unwrap();
    f.p.apply(Edit::SetKeyframe { seq, clip: f.title, property: "transform.opacity".into(), key: Keyframe::new(0, 0.0, Interp::Ease) }).unwrap();
    f.p.apply(Edit::Lift { seq, clips: vec![f.b] }).unwrap();
    let extra = Clip::media(ClipId(0), f.spine, 0, f.video, 0, S);
    f.p.apply(Edit::Append { seq, lane: None, clip: Box::new(extra) }).unwrap();
    let last = f.seq().spine_clips().last().unwrap().id;
    let prev = f.seq().spine_clips()[f.seq().spine_clips().len() - 2].id;
    f.p.apply(Edit::CreateCompound { seq, clips: vec![prev, last], name: "Outro".into(), new_sequence: SequenceId(0), new_clip: ClipId(0) }).unwrap();
    // A fade to black from a into the gap.
    let t = Transition { id: TransitionId(0), left: f.a, right: ClipId(0), kind: "dip".into(), duration: S / 2, align: TransitionAlign::StartAtCut, params: Default::default(), extra: Default::default() };
    f.p.apply(Edit::AddTransition { seq, transition: t }).unwrap();
    let mut lane = f.seq().lane(f.a1).unwrap().clone();
    lane.muted = true;
    f.p.apply(Edit::SetLane { seq, lane }).unwrap();
    f.p.check_invariants().unwrap();
    f
}

#[test]
fn otio_round_trip_is_lossless() {
    let mut f = busy();
    let (text, report) = otio::export(&f.p, f.seq).unwrap();
    // Effects and keyframes are reported as Studio-only.
    assert!(report.issues.iter().any(|i| i.message.contains("effects")), "{report}");
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["OTIO_SCHEMA"], "Timeline.1");
    let tracks = doc["tracks"]["children"].as_array().unwrap();
    assert_eq!(tracks.iter().filter(|t| t["kind"] == "Audio").count(), 1);
    assert_eq!(tracks[0]["children"][1]["OTIO_SCHEMA"], "Transition.1");

    let imported = otio::import(&mut f.p, &text).unwrap();
    let new_seq = imported.sequence;
    f.p.apply(imported.edit).unwrap();
    assert_eq!(f.p.media.len(), 3, "media matched by path, not duplicated");
    assert_same(&shape(&f.p, f.seq), &shape(&f.p, new_seq), "");

    // Into an empty project: media comes from the file.
    let mut fresh = Project::default();
    let imported = otio::import(&mut fresh, &text).unwrap();
    let id = imported.sequence;
    fresh.apply(imported.edit).unwrap();
    let mut paths: Vec<&str> = fresh.media.iter().map(|m| m.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["/media/beach.mp4", "/media/logo.png", "/media/song.wav"]);
    assert_same(&shape(&fresh, id)["clips"], &shape(&f.p, f.seq)["clips"], "");
}

const FOREIGN_OTIO: &str = r#"{
  "OTIO_SCHEMA": "Timeline.1",
  "name": "From elsewhere",
  "tracks": {
    "OTIO_SCHEMA": "Stack.1",
    "children": [
      {
        "OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "V1",
        "children": [
          {
            "OTIO_SCHEMA": "Clip.1", "name": "shot1",
            "source_range": {"OTIO_SCHEMA": "TimeRange.1",
              "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24},
              "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 48}},
            "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///footage/shot%201.mov",
              "available_range": {"OTIO_SCHEMA": "TimeRange.1",
                "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
                "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 240}}},
            "markers": [{"OTIO_SCHEMA": "Marker.2", "name": "look", "color": "RED", "comment": "",
              "marked_range": {"OTIO_SCHEMA": "TimeRange.1",
                "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 36},
                "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0}}}],
            "effects": [{"OTIO_SCHEMA": "Effect.1", "effect_name": "Blur", "metadata": {}}]
          },
          {"OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve",
            "in_offset": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 6},
            "out_offset": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 6}},
          {
            "OTIO_SCHEMA": "Clip.2", "name": "shot2",
            "source_range": {"OTIO_SCHEMA": "TimeRange.1",
              "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 48},
              "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}},
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///footage/shot2.mov", "available_range": null}},
            "active_media_reference_key": "DEFAULT_MEDIA",
            "effects": [{"OTIO_SCHEMA": "LinearTimeWarp.1", "effect_name": "LinearTimeWarp", "time_scalar": 0.5}]
          },
          {"OTIO_SCHEMA": "Gap.1", "source_range": {"OTIO_SCHEMA": "TimeRange.1",
              "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
              "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 12}}},
          {"OTIO_SCHEMA": "Clip.2", "name": "card",
            "source_range": {"OTIO_SCHEMA": "TimeRange.1",
              "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
              "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}},
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "GeneratorReference.1", "generator_kind": "SolidColor", "parameters": {}}}}
        ]
      },
      {
        "OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "A1",
        "children": [
          {"OTIO_SCHEMA": "Gap.1", "source_range": {"OTIO_SCHEMA": "TimeRange.1",
              "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
              "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}}},
          {"OTIO_SCHEMA": "Clip.2", "name": "vo",
            "source_range": {"OTIO_SCHEMA": "TimeRange.1",
              "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
              "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 48}},
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///audio/vo.wav"}}}
        ]
      }
    ]
  }
}"#;

#[test]
fn otio_from_another_tool() {
    let mut p = Project::default();
    let imported = otio::import(&mut p, FOREIGN_OTIO).unwrap();
    let report = imported.report.clone();
    let seq = imported.sequence;
    p.apply(imported.edit).unwrap();
    let s = p.sequence(seq).unwrap();
    assert_eq!(s.name, "From elsewhere");
    let spine: Vec<(String, Micros, Micros, Micros)> = s.spine_clips().iter().map(|c| (c.name.clone(), c.start, c.duration, c.source_in)).collect();
    assert_eq!(spine, vec![("shot1".into(), 0, 2 * S, S), ("shot2".into(), 2 * S, S, 2 * S), ("".into(), 3 * S, S / 2, 0), ("card".into(), 3 * S + S / 2, S, 0)]);
    assert_eq!(s.spine_clips()[1].speed, Speed::Constant(0.5));
    assert_eq!(s.transitions.len(), 1);
    assert_eq!(s.transitions[0].duration, S / 2);
    assert_eq!(s.spine_clips()[0].markers[0].time, S / 2);
    assert_eq!(s.spine_clips()[0].markers[0].colour, Rgba::new(1.0, 0.0, 0.0, 1.0));
    assert!(matches!(&s.spine_clips()[3].source, ClipSource::Generator(g) if g.kind == GeneratorKind::Solid));
    let audio = s.lanes.iter().find(|l| l.kind == LaneKind::Audio).unwrap();
    assert_eq!(s.clips_on(audio.id)[0].start, S);
    assert!(p.media.iter().any(|m| m.path == "/footage/shot 1.mov" && m.info.duration == Some(10 * S)));
    assert!(p.media.iter().any(|m| m.path == "/audio/vo.wav" && m.info.kind == MediaKind::Audio));
    // Nothing silently dropped: the blur and missing probe data are reported.
    assert!(report.issues.iter().any(|i| i.message.contains("Blur")), "{report}");
    assert!(report.issues.iter().any(|i| i.message.contains("not probed")), "{report}");
}

#[test]
fn otio_rejects_other_documents() {
    let mut p = Project::default();
    assert!(otio::import(&mut p, "not json").is_err());
    assert!(otio::import(&mut p, r#"{"OTIO_SCHEMA": "Clip.2"}"#).is_err());
}

const FCPXML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.10">
  <resources>
    <format id="r1" frameDuration="1/25s" width="1920" height="1080"/>
    <asset id="r2" name="Interview" start="0s" duration="60s" hasVideo="1" hasAudio="1" format="r1" audioRate="48k" audioChannels="2">
      <media-rep kind="original-media" src="file:///footage/Interview%20A.mov"/>
    </asset>
    <asset id="r3" name="Broll" start="3600s" duration="20s" hasVideo="1" format="r1">
      <media-rep kind="original-media" src="file:///footage/broll.mov"/>
    </asset>
    <asset id="r4" name="Music" start="0s" duration="120s" hasAudio="1" audioRate="44.1k">
      <media-rep kind="original-media" src="file:///audio/music.wav"/>
    </asset>
    <effect id="r5" name="Basic Title" uid=".../Titles.localized/Basic Text.localized/Basic Title.localized/Basic Title.moti"/>
    <media id="r6" name="Nested">
      <sequence format="r1" duration="2s" tcStart="0s">
        <spine>
          <asset-clip ref="r3" offset="0s" start="3601s" duration="2s" name="Broll inside"/>
        </spine>
      </sequence>
    </media>
  </resources>
  <library>
    <event name="Day 1">
      <project name="Story">
        <sequence format="r1" duration="12s" tcStart="0s">
          <spine>
            <asset-clip ref="r2" offset="0s" start="10s" duration="4s" name="Intro">
              <title ref="r5" lane="1" offset="11s" duration="2s" name="Name card">
                <text><text-style ref="ts1">Jane Doe</text-style></text>
              </title>
              <asset-clip ref="r4" lane="-1" offset="10s" start="5s" duration="4s" name="Bed"/>
              <marker start="12s" duration="1/25s" value="Good line" note="keep"/>
              <filter-video ref="r9" name="Colour Board"/>
            </asset-clip>
            <transition name="Cross Dissolve" offset="7/2s" duration="1s"/>
            <asset-clip ref="r3" offset="4s" start="3602s" duration="3s" name="Cutaway"/>
            <gap name="Gap" offset="7s" start="0s" duration="1s"/>
            <ref-clip ref="r6" offset="8s" start="0s" duration="2s" name="Nested clip"/>
            <title ref="r5" offset="10s" start="0s" duration="2s" name="End">
              <text><text-style ref="ts2">Thanks</text-style><text-style ref="ts3"> for watching</text-style></text>
            </title>
          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>"#;

#[test]
fn fcpxml_import() {
    let mut p = Project::default();
    let imported = fcpxml::import(&mut p, FCPXML).unwrap();
    let report = imported.report.clone();
    let seq = imported.sequence;
    p.apply(imported.edit).unwrap();
    let s = p.sequence(seq).unwrap();
    assert_eq!(s.name, "Story");
    let spine: Vec<(String, Micros, Micros, Micros)> = s.spine_clips().iter().map(|c| (c.name.clone(), c.start, c.duration, c.source_in)).collect();
    assert_eq!(
        spine,
        vec![("Intro".into(), 0, 4 * S, 10 * S), ("Cutaway".into(), 4 * S, 3 * S, 2 * S), ("".into(), 7 * S, S, 0), ("Nested clip".into(), 8 * S, 2 * S, 0), ("End".into(), 10 * S, 2 * S, 0),]
    );
    let t = &s.transitions[0];
    assert_eq!((t.kind.as_str(), t.duration, t.align), ("dissolve", S, TransitionAlign::Centre));
    // Connected clips.
    let v1 = s.lane_at_position(1).unwrap().id;
    let title = &s.clips_on(v1)[0];
    assert_eq!((title.start, title.duration), (S, 2 * S));
    assert!(matches!(&title.source, ClipSource::Generator(g) if g.plain_text() == "Jane Doe"));
    let a1 = s.lane_at_position(-1).unwrap().id;
    let bed = &s.clips_on(a1)[0];
    assert_eq!((bed.start, bed.source_in), (0, 5 * S));
    let ClipSource::Media(r) = &bed.source else { panic!() };
    assert!(!r.video);
    // Marker in clip-local time.
    assert_eq!(s.spine_clips()[0].markers[0].time, 2 * S);
    assert_eq!(s.spine_clips()[0].markers[0].note, "keep");
    // Compound clip and its nested sequence.
    let nested = s.spine_clips()[3].sequence_id().unwrap();
    assert_eq!(p.sequence(nested).unwrap().clips[0].source_in, S);
    assert!(matches!(&s.spine_clips()[4].source, ClipSource::Generator(g) if g.plain_text() == "Thanks for watching"));
    // Media facts from the resources.
    let interview = p.media.iter().find(|m| m.path == "/footage/Interview A.mov").unwrap();
    assert_eq!(interview.info.duration, Some(60 * S));
    assert_eq!(interview.info.video.as_ref().unwrap().width, 1920);
    assert_eq!(p.media.iter().find(|m| m.name == "Music").unwrap().info.audio.as_ref().unwrap().sample_rate, 44_100);
    // The colour board filter and frame rate difference are reported.
    assert!(report.issues.iter().any(|i| i.message.contains("filter-video")), "{report}");
    assert!(report.issues.iter().any(|i| i.message.contains("frame rate")), "{report}");
}

#[test]
fn fcpxml_rejects_other_documents() {
    let mut p = Project::default();
    assert!(fcpxml::import(&mut p, "<xmeml version=\"4\"/>").is_err());
    assert!(fcpxml::import(&mut p, "<<<").is_err());
}

#[test]
fn edl_export() {
    let mut f = fixture();
    let seq = f.seq;
    // 25 fps and a 0.4 s dissolve keep every edit on a whole frame.
    let mut settings = f.p.settings.clone();
    settings.frame_rate = FrameRate::FPS_25;
    f.p.apply(Edit::SetSettings { settings: Box::new(settings) }).unwrap();
    let mut t = f.seq().transition(f.dissolve).unwrap().clone();
    t.duration = 400_000;
    f.p.apply(Edit::SetTransition { seq, transition: t }).unwrap();
    f.p.apply(Edit::SetSpeed { seq, clip: f.c, speed: Speed::Constant(2.0) }).unwrap();
    let (text, report) = edl::export(&f.p, seq, edl::EdlOptions::default()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "TITLE: Sequence 1");
    assert_eq!(lines[1], "FCM: NON-DROP FRAME", "drop frame only applies to 29.97/59.94");
    // a: cut short by half the dissolve (0.2 s).
    assert_eq!(lines[3], "001  BEACH    B     C        00:00:01:00 00:00:03:20 01:00:00:00 01:00:02:20");
    // The dissolve into b: zero-length outgoing line, then the D event.
    assert_eq!(lines[6], "002  BEACH    B     C        00:00:03:20 00:00:03:20 01:00:02:20 01:00:02:20");
    assert_eq!(lines[7], "002  BEACH    B     D    010 00:00:04:20 00:00:07:00 01:00:02:20 01:00:05:00");
    // c: a still at double speed gets an M2 line.
    assert!(lines.iter().any(|l| l.starts_with("003  LOGO     V     C")), "{text}");
    assert!(lines.iter().any(|l| l.starts_with("M2   LOGO") && l.contains("050.0")), "{text}");
    assert!(text.contains("* FROM CLIP NAME: b"));
    assert!(text.contains("* SOURCE FILE: /media/beach.mp4"));
    assert!(report.issues.iter().any(|i| i.message.contains("not in the EDL")), "{report}");
}
