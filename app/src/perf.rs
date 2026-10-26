// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Heavy-project benchmark, headless (no window): a long timeline with many
//! lanes, thousands of clips and keyframes, and the time each thing the app
//! does per frame or per edit takes on it.
//!
//! `HOME=/tmp/x PERF_HOURS=20 cargo test --release -p studio-app -- --ignored heavy_project --nocapture`

use crate::bridge;
use crate::doc::Doc;
use crate::view::{self, ClipKind, Lane, LaneKind, MediaItem, MediaKind, Project, SEC};
use std::time::Instant;
use studio_model as m;

/// A project of `hours` of cut footage on the main lane, `overlays` lanes of
/// titles and pictures over it, `sounds` lanes of sound, and keyframes on a
/// quarter of the visual clips.
pub fn heavy(hours: f64, overlays: u32, sounds: u32) -> m::Project {
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut rand = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % 10_000) as f64 / 10_000.0
    };
    let base = view::demo();
    let mut lanes: Vec<Lane> = Vec::new();
    let mut id = 10u32;
    let mut lane_ids = Vec::new();
    for i in 0..overlays {
        let mut l = base.lanes[1].clone();
        l.id = 1000 + i;
        l.name = format!("Overlay {i}");
        lanes.push(l.clone());
        lane_ids.push((l.id, LaneKind::Visual));
    }
    let spine = base.lanes.iter().find(|l| l.kind == LaneKind::Spine).unwrap().clone();
    lanes.push(spine.clone());
    for i in 0..sounds {
        let mut l = base.lanes[3].clone();
        l.id = 2000 + i;
        l.name = format!("Sound {i}");
        lanes.push(l.clone());
        lane_ids.push((l.id, LaneKind::Audio));
    }
    let media: Vec<MediaItem> = view::demo_media();
    let video = |r: f64| media.iter().filter(|m| m.kind == MediaKind::Video).nth((r * 9.0) as usize % 9).unwrap().id;
    let mut clips = Vec::new();
    let len = (hours * 3600.0 * SEC as f64) as i64;
    let template = base.clips.iter().find(|c| c.kind == ClipKind::Video).unwrap().clone();
    let title = base.clips.iter().find(|c| c.kind == ClipKind::Title).unwrap().clone();
    let sound = base.clips.iter().find(|c| c.kind == ClipKind::Audio).unwrap().clone();
    // Main lane: back-to-back cuts of 4-16 s.
    let mut t = 0;
    while t < len {
        let d = ((4.0 + 12.0 * rand()) * SEC as f64) as i64;
        let mut c = template.clone();
        (c.id, c.lane, c.start, c.dur, c.src_in, c.media) = (id, spine.id, t, d.min(len - t), 0, Some(video(rand())));
        let mi = media.iter().find(|m| Some(m.id) == c.media).unwrap();
        c.dur = c.dur.min(mi.dur);
        clips.push(c.clone());
        id += 1;
        t += c.dur;
    }
    // Overlays: a title or picture every 15-40 s; sounds every 20-60 s.
    for &(lane, kind) in &lane_ids {
        let mut t = (rand() * 10.0 * SEC as f64) as i64;
        while t < len {
            let d = ((3.0 + 6.0 * rand()) * SEC as f64) as i64;
            let mut c = if kind == LaneKind::Audio { sound.clone() } else if rand() < 0.6 { title.clone() } else { template.clone() };
            c.id = id;
            c.lane = lane;
            c.start = t;
            c.dur = if kind == LaneKind::Audio { d * 4 } else { d };
            c.src_in = 0;
            if c.kind == ClipKind::Title {
                c.text = format!("Title {id}");
            }
            if let Some(m) = c.media {
                let mi = media.iter().find(|x| x.id == m).unwrap();
                c.dur = c.dur.min(mi.dur);
            }
            clips.push(c.clone());
            id += 1;
            t += c.dur + ((if kind == LaneKind::Audio { 20.0 } else { 15.0 } + 25.0 * rand()) * SEC as f64) as i64;
        }
    }
    let v = Project { lanes, clips, transitions: Vec::new(), markers: Vec::new(), range: None, next_id: id + 10, media, ..base };
    let mut model = bridge::to_model(&v);
    // Keyframes: a quarter of the visual clips fade and move.
    let seq_id = model.active_sequence;
    let seq = model.sequences.iter_mut().find(|s| s.id == seq_id).unwrap();
    for (k, c) in seq.clips.iter_mut().enumerate() {
        if k % 4 != 0 || matches!(c.source, m::ClipSource::Media(ref r) if !r.video) {
            continue;
        }
        let d = c.duration;
        c.keyframes.insert("transform.opacity".into(), m::Track::new(vec![m::Keyframe::new(0, m::Value::Scalar(0.0), m::Interp::Linear), m::Keyframe::new(d / 4, m::Value::Scalar(1.0), m::Interp::Linear), m::Keyframe::new(d - d / 4, m::Value::Scalar(1.0), m::Interp::Linear), m::Keyframe::new(d, m::Value::Scalar(0.0), m::Interp::Linear)]));
        c.keyframes.insert(
            "transform.position".into(),
            m::Track::new(vec![m::Keyframe::new(0, m::Value::Point(m::Vec2 { x: -200.0, y: 0.0 }), m::Interp::Linear), m::Keyframe::new(d, m::Value::Point(m::Vec2 { x: 200.0, y: 0.0 }), m::Interp::Linear)]),
        );
    }
    model
}

/// Median milliseconds of `n` runs of `f`.
pub fn time(n: usize, mut f: impl FnMut()) -> f64 {
    let mut v: Vec<f64> = (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hours() -> f64 {
        std::env::var("PERF_HOURS").ok().and_then(|v| v.parse().ok()).unwrap_or(20.0)
    }

    /// Model-level costs: what every edit and every playback frame pays.
    #[test]
    #[ignore]
    fn heavy_project() {
        let t0 = Instant::now();
        let model = heavy(hours(), 12, 6);
        let seq = model.sequence(model.active_sequence).unwrap();
        let keyed = seq.clips.iter().filter(|c| !c.keyframes.is_empty()).count();
        eprintln!("project: {} clips ({} keyed) on {} lanes, {:.1} h, built in {:.0} ms", seq.clips.len(), keyed, seq.lanes.len(), seq.duration() as f64 / 3.6e9, t0.elapsed().as_secs_f64() * 1000.0);
        let mut doc = Doc::new(model);
        let mid = doc.project.clips.iter().map(|c| c.end()).max().unwrap() / 2;
        let row = |what: &str, ms: f64| eprintln!("  {what:44} {ms:9.2} ms");
        row("view of the project (bridge::view_of)", time(5, || drop(bridge::view_of(&doc.model, mid))));
        let mut tt = mid;
        row("playhead moves (Doc::set_time, keyed clips)", time(5, || {
            tt += SEC / 30;
            doc.set_time(tt);
        }));
        let target = doc.project.clips.iter().find(|c| c.start > mid).unwrap().id;
        let mut dx = 0;
        row("drag step (edit_gesture: move one clip)", time(5, || {
            dx += SEC / 30;
            doc.edit_gesture("drag", "Move", |p| {
                if let Some(c) = p.clips.iter_mut().find(|c| c.id == target) {
                    c.transform.x += 1.0;
                }
            });
        }));
        doc.end_gesture();
        row("one edit (Doc::edit)", time(5, || {
            doc.edit("Opacity", |p| {
                if let Some(c) = p.clips.iter_mut().find(|c| c.id == target) {
                    c.transform.opacity = if c.transform.opacity > 50.0 { 40.0 } else { 90.0 };
                }
            });
        }));
        row("undo + redo", time(5, || {
            doc.undo();
            doc.redo();
        }));
        let arc = std::sync::Arc::new(doc.model.clone());
        row("clone the model (Arc for the renderer)", time(5, || drop(std::sync::Arc::new(doc.model.clone()))));
        let scene = studio_render::model_scene::ModelScene::new(arc);
        use studio_render::scene::Scene;
        let mut st = mid;
        row("scene: clips at a time (per rendered frame)", time(20, || {
            st += SEC / 30;
            drop(scene.clips_at(st));
        }));
        row("sound plan (crate::sound_bridge::plan)", time(3, || drop(crate::sound_bridge::plan(&doc.model))));
        {
            let after = doc.project.clone();
            row("  part: clone the view", time(5, || drop(doc.project.clone())));
            row("  part: diff the view", time(5, || drop(bridge::diff(&doc.model, &doc.project, &after, mid))));
            let mut mm = doc.model.clone();
            let mut c = mm.active().clips.iter().find(|c| c.id.0 as u32 == target).unwrap().clone();
            row("  part: model apply SetClip", time(5, || {
                c.transform.opacity = if c.transform.opacity > 0.5 { 0.4 } else { 0.9 };
                let seq = mm.active_sequence;
                mm.apply(m::Edit::SetClip { seq, clip: Box::new(c.clone()) }).unwrap();
            }));
            row("  part: invariants check", time(5, || drop(mm.check_invariants())));
        }
        row("save as JSON (serialize)", time(3, || drop(serde_json::to_vec(&doc.model).unwrap())));
    }

    /// The whole window's frames, headless: the real UI code drawing the
    /// heavy project (no window opens). Idle, playing, zoomed out to all of
    /// it, and dragging a clip.
    #[test]
    #[ignore]
    fn heavy_ui() {
        use eframe::egui;
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = crate::app::StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        // PERF_PROJECT=DIR: a real project instead of the made-up one.
        app.doc = match std::env::var("PERF_PROJECT") {
            Ok(dir) => crate::agent::Session::at(std::path::Path::new(&dir)).expect("PERF_PROJECT opens").doc,
            Err(_) => Doc::new(heavy(hours(), 12, 6)),
        };
        let span = app.doc.project.clips.iter().map(|c| c.end()).max().unwrap_or(SEC) as f32 / SEC as f32;
        let n = app.doc.project.clips.len();
        let mut frame = eframe::Frame::_new_kittest();
        let mut t = 0.0f64;
        let mut step = |app: &mut crate::app::StudioApp| -> f64 {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))), time: Some(t), ..Default::default() };
            let s = Instant::now();
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(app, ui, &mut frame));
            s.elapsed().as_secs_f64() * 1000.0
        };
        let report = |what: &str, v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            eprintln!("  {what:28} median {:7.2} ms   p95 {:7.2} ms   worst {:7.2} ms", v[v.len() / 2], v[v.len() * 95 / 100], v[v.len() - 1]);
        };
        eprintln!("ui frames on {n} clips, {:.2} h:", span / 3600.0);
        for _ in 0..30 {
            step(&mut app);
        }
        let mid = app.doc.project.clips.iter().map(|c| c.end()).max().unwrap() / 2;
        app.editor.playhead = mid;
        // PERF_LOOP=idle|drag: run that for 10 s (to sample the process).
        if let Ok(which) = std::env::var("PERF_LOOP") {
            if which == "zoom" {
                app.editor.px_per_sec = 1920.0 / span;
                app.editor.zoom = Some(app.editor.px_per_sec);
            }
            let until = Instant::now() + std::time::Duration::from_secs(10);
            let target = app.doc.project.clips.iter().find(|c| c.start > mid && c.kind != ClipKind::Audio).or(app.doc.project.clips.first()).unwrap().id;
            let mut k = 0;
            while Instant::now() < until {
                if which == "drag" {
                    k += 1;
                    app.doc.edit_gesture("drag", "Move", |p| {
                        if let Some(c) = p.clips.iter_mut().find(|c| c.id == target) {
                            c.transform.x = k as f32;
                        }
                    });
                }
                step(&mut app);
            }
            return;
        }
        let mut v: Vec<f64> = (0..60).map(|_| step(&mut app)).collect();
        report("idle", &mut v);
        let mut v: Vec<f64> = (0..120)
            .map(|_| {
                app.editor.playhead += SEC / 60;
                step(&mut app)
            })
            .collect();
        report("playing (playhead moving)", &mut v);
        let px = app.editor.px_per_sec;
        app.editor.px_per_sec = 1920.0 / span;
        app.editor.zoom = Some(app.editor.px_per_sec);
        let mut v: Vec<f64> = (0..60).map(|_| step(&mut app)).collect();
        report("zoomed out to everything", &mut v);
        app.editor.px_per_sec = px;
        app.editor.zoom = Some(px);
        let target = app.doc.project.clips.iter().find(|c| c.start > mid && c.kind != ClipKind::Audio).or(app.doc.project.clips.first()).unwrap().id;
        let mut v: Vec<f64> = (0..60)
            .map(|k| {
                app.doc.edit_gesture("drag", "Move", |p| {
                    if let Some(c) = p.clips.iter_mut().find(|c| c.id == target) {
                        c.transform.x = k as f32 * 3.0;
                    }
                });
                step(&mut app)
            })
            .collect();
        app.doc.end_gesture();
        report("dragging (edit every frame)", &mut v);
    }
}
