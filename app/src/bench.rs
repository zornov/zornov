// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Benchmark harness: `BENCH=idle|play|scrub|jump BENCH_MEDIA=file.mp4
//! [BENCH_FRAMES=1000] [BENCH_OUT=out.csv]` (run with `--screen editor
//! --project empty --fresh`). Imports the file, puts it on the timeline,
//! warms up, drives the scenario for BENCH_FRAMES frames and writes one CSV
//! row per frame: `frame,ui_ms,interval_ms,new_picture,picture_latency_ms,
//! playhead_ms`. `picture_latency_ms` runs from asking the renderer for a
//! frame to that frame reaching the screen (empty when no new picture
//! arrived that frame).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

static REQUESTS: Mutex<Option<HashMap<u64, Instant>>> = Mutex::new(None);
static SHOWN: Mutex<Vec<f64>> = Mutex::new(Vec::new());

pub fn enabled() -> bool {
    std::env::var_os("BENCH").is_some()
}

/// The viewer asked for picture `seq`.
pub fn requested(seq: u64) {
    if enabled() {
        REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(seq, Instant::now());
    }
}

/// Picture `seq` reached the screen; older requests were superseded.
pub fn shown(seq: u64) {
    if !enabled() {
        return;
    }
    let mut g = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(t) = map.remove(&seq) {
        SHOWN.lock().unwrap_or_else(|e| e.into_inner()).push(t.elapsed().as_secs_f64() * 1000.0);
    }
    map.retain(|s, _| *s > seq);
}

pub struct Bench {
    scenario: String,
    media: String,
    frames: u64,
    out: String,
    n: i64,
    last: Option<Instant>,
    rows: Vec<String>,
    rng: u64,
    scrub_t: i64,
}

const WARMUP: i64 = 90;

impl Bench {
    pub fn from_env() -> Option<Bench> {
        let scenario = std::env::var("BENCH").ok()?;
        Some(Bench {
            scenario,
            media: std::env::var("BENCH_MEDIA").unwrap_or_default(),
            frames: std::env::var("BENCH_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(1000),
            out: std::env::var("BENCH_OUT").unwrap_or_else(|_| "bench.csv".into()),
            n: -1,
            last: None,
            rows: vec!["frame,ui_ms,interval_ms,new_picture,picture_latency_ms,playhead_ms".into()],
            rng: 0x9E37_79B9_7F4A_7C15,
            scrub_t: 0,
        })
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// Once per frame, after the UI ran (`ui_ms`).
    pub fn frame(app: &mut crate::app::StudioApp, ctx: &eframe::egui::Context, ui_ms: f64) {
        let Some(mut b) = app.bench.take() else { return };
        ctx.request_repaint();
        b.n += 1;
        let now = Instant::now();
        let interval = b.last.map_or(0.0, |l| (now - l).as_secs_f64() * 1000.0);
        b.last = Some(now);
        if b.n == 0 {
            let ids = app.import_paths(ctx, &[std::path::PathBuf::from(&b.media)]);
            if let Some(i) = ids.first().and_then(|id| app.doc.project.media.iter().position(|m| m.id == *id)) {
                crate::editor::library::add(app, ctx, crate::editor::RailItem::Media, i, Some(0), None);
            }
            // BENCH_KEYS=1: the clip is animated (as edits by the AI editor
            // often are), so the view follows keyframes while playing.
            if std::env::var_os("BENCH_KEYS").is_some() {
                let doc = std::mem::replace(&mut app.doc, crate::doc::Doc::new(studio_model::Project::default()));
                let mut s = crate::agent::Session::new(doc);
                let id = s.doc.project.clips.first().map_or(0, |c| c.id);
                let _ = s.run("edit", &serde_json::json!({"ops": [{"op": "animate", "clip": id, "property": "opacity", "keys": [[0, 60], [5, 100], [10, 60]]}]}));
                app.doc = s.doc;
            }
            app.editor.selection.clear();
            app.bench = Some(b);
            return;
        }
        let dur = app.doc.project.duration().max(1);
        if b.n == WARMUP {
            SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clear();
            if b.scenario == "play" {
                app.editor.playhead = 0;
                if !app.editor.playing {
                    crate::editor::toggle_play(&mut app.editor);
                }
            }
        }
        if b.n >= WARMUP {
            match b.scenario.as_str() {
                "scrub" => {
                    b.scrub_t = (b.scrub_t + 333_333) % dur;
                    app.editor.playhead = b.scrub_t;
                }
                "jump" => {
                    app.editor.playhead = (b.next_rand() % dur as u64) as i64;
                }
                _ => {}
            }
            let lat: Vec<f64> = std::mem::take(&mut *SHOWN.lock().unwrap_or_else(|e| e.into_inner()));
            let k = b.n - WARMUP;
            let latency = lat.last().map(|v| format!("{v:.3}")).unwrap_or_default();
            b.rows.push(format!("{k},{ui_ms:.3},{interval:.3},{},{latency},{:.1}", !lat.is_empty() as u8, app.editor.playhead as f64 / 1000.0));
            if k as u64 >= b.frames {
                let _ = std::fs::write(&b.out, b.rows.join("\n") + "\n");
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
            }
        }
        app.bench = Some(b);
    }
}
