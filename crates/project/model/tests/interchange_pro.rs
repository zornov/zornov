// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Professional interchange: EDL import (and its round trip with export),
//! FCPXML export → import, source timecode, and conform / relink.

mod common;
use common::*;
use studio_model::interchange::{conform, edl, fcpxml, fcpxml_write};
use studio_model::*;

/// The fixture at 25 fps with a frame-exact 0.4 s dissolve.
fn at_25() -> Fx {
    let mut f = fixture();
    let seq = f.seq;
    let mut settings = f.p.settings.clone();
    settings.frame_rate = FrameRate::FPS_25;
    f.p.apply(Edit::SetSettings { settings: Box::new(settings) }).unwrap();
    let mut t = f.seq().transition(f.dissolve).unwrap().clone();
    t.duration = 400_000;
    f.p.apply(Edit::SetTransition { seq, transition: t }).unwrap();
    f
}

fn spine_media(p: &Project, seq: SequenceId) -> Vec<(Micros, Micros, Micros, Option<MediaId>)> {
    p.sequence(seq).unwrap().spine_clips().iter().filter(|c| !c.is_gap()).map(|c| (c.start, c.duration, c.source_in, c.media_id())).collect()
}

#[test]
fn edl_round_trip_keeps_the_cut() {
    let mut f = at_25();
    let (text, _) = edl::export(&f.p, f.seq, edl::EdlOptions::default()).unwrap();
    let imp = edl::import(&mut f.p, &text, edl::EdlImport::default()).unwrap();
    f.p.apply(imp.edit).unwrap();
    f.p.check_invariants().unwrap();
    assert_eq!(spine_media(&f.p, imp.sequence), spine_media(&f.p, f.seq), "{text}");
    let s = f.p.sequence(imp.sequence).unwrap();
    assert_eq!(s.transitions.len(), 1);
    assert_eq!(s.transitions[0].duration, 400_000);
    // Reels matched existing media: nothing offline.
    assert!(!f.p.media.iter().any(|m| m.path.starts_with("offline:")), "{}", imp.report);
}

#[test]
fn edl_uses_source_timecode_reels_and_cdl() {
    let mut f = at_25();
    let seq = f.seq;
    // The camera file starts at 14:22:10:00 and has a reel name.
    let mut m = f.p.media_item(f.video).unwrap().clone();
    m.timecode = Some("14:22:10:00".into());
    m.reel = Some("A001C003".into());
    f.p.apply(Edit::SetMedia { item: Box::new(m) }).unwrap();
    let mut a = f.clip(f.a).clone();
    let mut g = Grade::default();
    for (k, v) in [("cdl_slope_r", 1.1), ("cdl_offset_b", -0.02), ("cdl_power_g", 0.9), ("cdl_sat", 0.8)] {
        g.params.insert(k.into(), Value::Scalar(v));
    }
    a.grade = Some(g);
    f.p.apply(Edit::SetClip { seq, clip: Box::new(a) }).unwrap();
    f.p.apply(Edit::AddMarker { seq, clip: None, marker: Marker::new(MarkerId(0), 2 * S, "VFX 012") }).unwrap();
    let (text, _) = edl::export(&f.p, seq, edl::EdlOptions::default()).unwrap();
    // Source in of a (1 s into the file) is the file's timecode + 1 s.
    assert!(text.contains("001  A001C003 B     C        14:22:11:00"), "{text}");
    assert!(text.contains("*ASC_SOP (1.100000 1.000000 1.000000)(0.000000 0.000000 -0.020000)(1.000000 0.900000 1.000000)"), "{text}");
    assert!(text.contains("*ASC_SAT 0.800000"), "{text}");
    assert!(text.contains("* LOC: 01:00:02:00"), "{text}");
    // Importing maps the timecode back to the same source frames.
    let imp = edl::import(&mut f.p, &text, edl::EdlImport::default()).unwrap();
    f.p.apply(imp.edit).unwrap();
    assert_eq!(spine_media(&f.p, imp.sequence), spine_media(&f.p, seq));
    let s = f.p.sequence(imp.sequence).unwrap();
    let g = s.spine_clips()[0].grade.clone().unwrap();
    assert_eq!(g.params.get("cdl_slope_r"), Some(&Value::Scalar(1.1)));
    assert_eq!(g.params.get("cdl_sat"), Some(&Value::Scalar(0.8)));
    assert_eq!(s.markers.len(), 1);
    assert_eq!(s.markers[0].name, "VFX 012");
    assert_eq!(s.markers[0].time, 2 * S);
}

/// An EDL as another system writes it: 29.97 drop-frame, a reel that is
/// not in the project, a dissolve, a speed change and a wipe.
#[test]
fn external_drop_frame_edl_with_offline_reels_then_conform() {
    let mut p = Project::default();
    let mut settings = p.settings.clone();
    settings.frame_rate = FrameRate::FPS_29_97;
    p.apply(Edit::SetSettings { settings: Box::new(settings) }).unwrap();
    let edl_text = "TITLE: REEL 1 LOCKED CUT\r\nFCM: DROP FRAME\r\n\r\n\
001  B004C012 V     C        13:01:10;00 13:01:14;00 01:00:00;00 01:00:04;00\r\n\
* FROM CLIP NAME:  B004C012_210314_R2PK.MOV\r\n\
002  B004C012 V     C        13:01:20;00 13:01:20;00 01:00:04;00 01:00:04;00\r\n\
002  A002C007 V     D    030 09:15:30;10 09:15:35;10 01:00:04;00 01:00:09;00\r\n\
M2   A002C007       059.9                09:15:30;10\r\n\
* FROM CLIP NAME:  A002C007.MOV\r\n\
003  BL       V     C        00:00:00;00 00:00:01;00 01:00:09;00 01:00:10;00\r\n\
004  B004C012 V     W001 015 13:02:05;00 13:02:07;00 01:00:10;00 01:00:12;00\r\n\
005  A002C007 AA    C        09:15:30;10 09:15:35;10 01:00:04;00 01:00:09;00\r\n";
    let imp = edl::import(&mut p, edl_text, edl::EdlImport::default()).unwrap();
    p.apply(imp.edit).unwrap();
    p.check_invariants().unwrap();
    let report = imp.report.to_string();
    assert!(report.contains("offline"), "{report}");
    assert!(report.contains("W001"), "wipe reported: {report}");
    let offline: Vec<&MediaItem> = p.media.iter().filter(|m| m.path.starts_with("offline:")).collect();
    assert_eq!(offline.len(), 2);
    let s = p.sequence(imp.sequence).unwrap();
    let rate = FrameRate::FPS_29_97;
    let tc = Timecode::new(rate, true);
    // Event 2 is a 30-frame dissolve at 01:00:04;00 of A002C007 at 2x.
    let spine = s.spine_clips();
    let b = spine.iter().find(|c| c.name.contains("A002C007")).unwrap();
    assert!(matches!(b.speed, Speed::Constant(v) if (v - 2.0).abs() < 0.01), "{:?}", b.speed);
    assert_eq!(s.transitions.len(), 2, "dissolve and the wipe (as a dissolve)");
    // The black event is a gap; audio went to an audio lane.
    assert!(spine.iter().any(|c| c.is_gap()));
    assert!(s.lanes.iter().any(|l| l.kind == LaneKind::Audio));

    // Conform: the real camera files have their own timecode.
    let first_used = |p: &Project, reel: &str| -> i64 {
        let m = p.media.iter().find(|m| m.reel.as_deref() == Some(reel)).unwrap();
        let origin = rate.micros_to_frame_round(m.timecode_start(rate));
        let c = p.sequence(imp.sequence).unwrap().clips.iter().filter(|c| c.media_id() == Some(m.id)).min_by_key(|c| c.start).unwrap();
        origin + rate.micros_to_frame_round(c.source_in)
    };
    let before = first_used(&p, "B004C012");
    assert_eq!(tc.format_frames(before), "13:01:10;00");
    let candidates = vec![
        conform::Candidate { path: "/footage/B004C012_210314_R2PK.MOV".into(), reel: Some("B004C012".into()), timecode: Some("13:00:00;00".into()), duration: Some(600 * S), frame_rate: Some(rate) },
        conform::Candidate { path: "/footage/A002C007.MOV".into(), reel: None, timecode: Some("09:15:00;00".into()), duration: Some(120 * S), frame_rate: Some(rate) },
        conform::Candidate { path: "/footage/other.mov".into(), reel: Some("Z999".into()), timecode: Some("00:00:00;00".into()), duration: Some(S), frame_rate: Some(rate) },
    ];
    let ids: Vec<MediaId> = offline.iter().map(|m| m.id).collect();
    let (relinks, report) = conform::plan(&p, &ids, &candidates, true);
    assert_eq!(relinks.len(), 2, "{report}");
    let edit = conform::apply(&p, &relinks);
    p.apply(edit).unwrap();
    p.check_invariants().unwrap();
    assert!(!p.media.iter().any(|m| m.path.starts_with("offline:")));
    // The same source frames are used against the real file's timecode.
    assert_eq!(tc.format_frames(first_used(&p, "B004C012")), "13:01:10;00");
}

#[test]
fn fcpxml_export_round_trips_through_import() {
    let mut f = at_25();
    let seq = f.seq;
    let mut m = f.p.media_item(f.video).unwrap().clone();
    m.timecode = Some("10:00:00:00".into());
    f.p.apply(Edit::SetMedia { item: Box::new(m) }).unwrap();
    f.p.apply(Edit::AddMarker { seq, clip: Some(f.a), marker: Marker::new(MarkerId(0), S, "smile") }).unwrap();
    let (xml, report) = fcpxml_write::export(&f.p, seq, fcpxml_write::FcpxmlOptions::default()).unwrap();
    assert!(xml.starts_with("<?xml"), "{xml}");
    assert!(xml.contains("frameDuration=\"1/25s\""), "{xml}");
    assert!(xml.contains("start=\"36000s\""), "asset start at its timecode: {xml}");
    assert!(xml.contains("<title ref="), "{xml}");
    assert!(xml.contains("Cross Dissolve"), "{xml}");
    let _ = report;
    // Well-formed and read back by the importer into the same layout.
    let imp = fcpxml::import(&mut f.p, &xml).unwrap_or_else(|e| panic!("{e}\n{xml}"));
    f.p.apply(imp.edit).unwrap();
    f.p.check_invariants().unwrap();
    assert_eq!(spine_media(&f.p, imp.sequence), spine_media(&f.p, seq), "{xml}");
    let s = f.p.sequence(imp.sequence).unwrap();
    let orig = f.seq();
    assert_eq!(s.transitions.len(), orig.transitions.len());
    assert_eq!(s.transitions[0].duration, orig.transitions[0].duration);
    // Title and music come back on their lanes.
    let title = s.clips.iter().find(|c| matches!(&c.source, ClipSource::Generator(g) if g.plain_text() == "Hello")).expect("title");
    assert_eq!((title.start, title.duration), (S, 2 * S));
    assert!(s.lane_position(title.lane).unwrap() > 0);
    let song = s.clips.iter().find(|c| c.media_id() == Some(f.music)).expect("music");
    assert!(s.lane_position(song.lane).unwrap() < 0);
    assert_eq!((song.start, song.duration), (0, 5 * S));
    // The clip marker is back on clip a, at the same place in the clip.
    let a = s.spine_clips()[0];
    assert_eq!(a.markers.len(), 1);
    assert_eq!(a.markers[0].time, S);
}

#[test]
fn drop_frame_fcpxml_times_are_exact() {
    let mut p = Project::default();
    let mut settings = p.settings.clone();
    settings.frame_rate = FrameRate::new(24000, 1001);
    p.apply(Edit::SetSettings { settings: Box::new(settings) }).unwrap();
    let (xml, _) = fcpxml_write::export(&p, p.active_sequence, fcpxml_write::FcpxmlOptions::default()).unwrap();
    // 01:00:00:00 at 23.976 is frame 86400: 86400 × 1001 / 24000 s.
    assert!(xml.contains("tcStart=\"18018/5s\""), "{xml}");
    assert!(xml.contains("frameDuration=\"1001/24000s\""), "{xml}");
}
