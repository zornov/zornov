// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The end-to-end flow at the logic level: pick
//! Vertical, import three clips, drop a title from Text, add a Cut between
//! two clips, apply a Look, add a Motion to a sticker from Assets, export
//! Social 1080p. Drives only the services and `studio-model`; no UI.

use super::export::{self, Event, ExportJob};
use super::library::{self, Panel};
use super::project::{self, tests::testdata};
use super::transport;
use crate::catalogue;
use crate::view::{ClipKind, LaneKind, SEC};

#[test]
fn finish_line() {
    // 1. Pick Vertical.
    let mut doc = project::new_project(project::canvas("Vertical").unwrap());
    assert_eq!((doc.project.width, doc.project.height), (1080, 1920));

    // 2. Import three clips onto the main lane.
    let files = ["h264_high_cabac.mp4", "h264_baseline_cavlc.mp4", "h264_main_cabac_bframes.mp4"].map(testdata);
    let (report, clips) = project::import_to_timeline(&mut doc, &files).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(clips.len(), 3);
    let spine = doc.project.spine().unwrap();
    let mut main: Vec<_> = doc.project.clips.iter().filter(|c| c.lane == spine).cloned().collect();
    main.sort_by_key(|c| c.start);
    assert_eq!(main.iter().map(|c| c.id).collect::<Vec<_>>(), clips);
    for w in main.windows(2) {
        assert_eq!(w[0].end(), w[1].start, "main lane is packed");
    }

    // 3. A title from Text at the start.
    let style = library::search(&doc, Panel::Text, 0, "")[0] as usize;
    let title = library::add(&mut doc, Panel::Text, style, 0, None).unwrap().select[0];
    let t = doc.project.clip(title).unwrap().clone();
    assert_eq!(t.kind, ClipKind::Title);
    assert!(t.text_style.is_some() && !t.text.is_empty());

    // 4. A Cut between the first two clips.
    let dissolve = catalogue::cuts().iter().position(|c| c.id == "dissolve").unwrap();
    library::apply(&mut doc, Panel::Cuts, dissolve, &[clips[0]]).unwrap();
    // The clips use their whole files, so the cut trims a quarter second
    // off each side to have frames to blend.
    let tr = doc.project.transitions.iter().find(|tr| tr.left == clips[0] && tr.right == clips[1]).unwrap();
    assert_eq!((tr.kind.as_str(), tr.dur), ("dissolve", SEC / 2));

    // 5. A Look on the second clip.
    let look = catalogue::looks()[0];
    library::apply(&mut doc, Panel::Looks, 0, &[clips[1]]).unwrap();
    assert_eq!(doc.project.clip(clips[1]).unwrap().look.as_deref(), Some(look.id));

    // 6. A sticker from Assets, with a Motion.
    let sticker_query = library::search(&doc, Panel::Assets, if Panel::Assets.uses_content() { 1 } else { 0 }, "");
    let sticker = library::add(&mut doc, Panel::Assets, sticker_query[0] as usize, SEC / 2, None).unwrap().select[0];
    let rise = catalogue::motions().iter().position(|m| m.id == "rise").unwrap();
    library::apply(&mut doc, Panel::Motions, rise, &[sticker]).unwrap();
    let s = doc.project.clip(sticker).unwrap();
    assert_eq!(s.motion_in.as_deref(), Some("rise"));
    assert_ne!(s.lane, spine);
    assert_eq!(doc.project.lane(s.lane).unwrap().kind, LaneKind::Visual);

    // 7. Export Social 1080p: the vertical project comes out 1080 × 1920.
    let dur = transport::duration(&doc);
    assert_eq!(dur, doc.project.duration());
    let dir = std::env::temp_dir().join(format!("studio-finish-line-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let preset = export::preset("Social 1080p").unwrap();
    let path = export::free_path(&dir, &doc.project.name, preset.format);
    let req = export::request(&doc, preset, path.clone(), None);
    assert_eq!(req.size, (1080, 1920));
    let frames = export::frame_count(req.range, req.fps);
    let mut job = ExportJob::start(&doc, req);
    assert_eq!(job.wait(), Event::Finished { path: path.clone() });

    let movie = studio_mp4::open(&path).unwrap();
    let movie = movie.movie();
    let video = movie.tracks.iter().find(|t| t.kind == studio_mp4::demux::TrackKind::Video).unwrap();
    assert_eq!((video.width, video.height), (1080, 1920));
    assert_eq!(video.samples.len() as u64, frames, "frame count");
    let expected_secs = frames as f64 / 30.0;
    assert!((video.duration_secs - expected_secs).abs() < 1e-6, "duration {} vs {}", video.duration_secs, expected_secs);
    // The picture has content (not a black frame).
    let f = crate::media_io::frame(&path, 0.5, 54, 96, false).unwrap();
    assert!(f.pixels.iter().any(|p| p.r() > 30 || p.g() > 30 || p.b() > 30));
    let _ = std::fs::remove_dir_all(&dir);

    // Every step was its own undo step: undoing them all empties the
    // sequence and the bin again.
    let mut n = 0;
    while doc.undo().is_some() {
        n += 1;
    }
    assert!(n >= 8, "{n} undo steps"); // import, 3 clips, title, cut, look, sticker, motion
    assert!(doc.project.clips.is_empty() && doc.project.media.is_empty());
}
