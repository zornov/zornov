// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI panel's "Generate" card: a picture from words, made by Codex's
//! image generation on the person's own ChatGPT plan (the same engine as
//! the AI's `generate` tool). Video, music and sound from words are not
//! available yet. What it makes is imported and laid at the playhead.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::agent::generate::Kind;
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, Segment};
use crate::theme;
use crate::view::Time;

#[cfg(feature = "render")]
#[path = "generate_tasks.rs"]
mod change;

const KINDS: [Kind; 4] = [Kind::Picture, Kind::Video, Kind::Music, Kind::Sound];

/// The card's settings, kept between frames.
#[derive(Clone, Debug, Default)]
struct Form {
    kind: usize,
    prompt: String,
}

/// A run in flight: its state, and what it made.
#[derive(Default)]
struct Run {
    status: String,
    done: Option<Result<Vec<PathBuf>, String>>,
    kind: Option<Kind>,
    at: Time,
    /// The app that started it ([`crate::editor::owner_of`]).
    owner: usize,
}

fn form_id() -> egui::Id {
    egui::Id::unique("ai-generate-form")
}

fn run() -> &'static Mutex<Option<Arc<Mutex<Run>>>> {
    static R: Mutex<Option<Arc<Mutex<Run>>>> = Mutex::new(None);
    &R
}

/// Imports a finished run and lays it at the playhead.
fn land(app: &mut StudioApp, ctx: &egui::Context, files: Vec<PathBuf>, kind: Kind, at: Time) {
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let paths: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let r = s.call("import", &serde_json::json!({"paths": paths}));
    let media: Vec<u64> = r
        .ok()
        .and_then(|parts| parts.iter().find_map(|p| if let crate::agent::Content::Text(t) = p { serde_json::from_str::<serde_json::Value>(t).ok() } else { None }))
        .and_then(|v| v["media"].as_array().map(|a| a.iter().filter_map(|m| m.as_u64()).collect()))
        .unwrap_or_default();
    let secs = at as f64 / 1e6;
    let ops: Vec<serde_json::Value> = media.iter().map(|m| serde_json::json!({"op": "place", "media": m, "at": secs})).collect();
    let placed = if ops.is_empty() { Err("nothing came back to import".to_owned()) } else { s.edit(&ops, "Place generated media", false).map(|_| ()) };
    app.doc = s.doc;
    match placed {
        Ok(()) => {
            kit::push(ctx, crate::i18n::tf(format!("Generated {} placed at {:.1}s", kind.name(), secs)), None, Some(Icon::Sparkle));
        }
        Err(e) => {
            kit::push(ctx, crate::i18n::tf(format!("Generated, but not placed: {e}")), None, Some(Icon::Warning));
        }
    }
}

/// Lands a finished run (called every frame while the AI panel shows).
pub fn poll(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(r) = run().lock().ok().and_then(|g| g.clone()).filter(|r| r.lock().is_ok_and(|g| g.owner == crate::editor::owner_of(app))) else { return };
    let finished = r.lock().ok().and_then(|mut g| g.done.take().map(|d| (d, g.kind, g.at)));
    match finished {
        Some((Ok(files), Some(kind), at)) => {
            if let Ok(mut g) = run().lock() {
                *g = None;
            }
            land(app, ctx, files, kind, at);
        }
        Some((Err(e), _, _)) => {
            if let Ok(mut g) = run().lock() {
                *g = None;
            }
            kit::push(ctx, crate::i18n::tf(format!("Generating failed: {e}")), None, Some(Icon::Warning));
        }
        _ => ctx.request_repaint_after(std::time::Duration::from_millis(300)),
    }
}

/// The card.
pub fn card(app: &mut StudioApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    poll(app, &ctx);
    #[cfg(feature = "render")]
    change::poll(app, &ctx);
    let t = theme::tokens(&ctx);
    let mut f: Form = ctx.data(|d| d.get_temp(form_id())).unwrap_or_default();
    let w = ui.available_width();
    egui::Frame::new().fill(t.raised).corner_radius(theme::radius::CARD).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(w - 24.0);
        ui.label(egui::RichText::new(crate::i18n::shown("Generate")).font(theme::control_medium()).color(t.text));
        // Make from words, or change what is there (animate, restyle,
        // relight, extend, fill, upscale, enhance).
        #[cfg(feature = "render")]
        if change::mode(ui, w - 24.0) {
            change::section(app, ui, w - 24.0);
            return;
        }
        let segs: Vec<Segment> = ["Picture", "Video", "Music", "Sound"].into_iter().map(Segment::text).collect();
        kit::segmented(ui, "gen-kind", &mut f.kind, &segs, Some(w - 24.0), None);
        let kind = KINDS[f.kind.min(3)];
        let soon = match kind {
            Kind::Picture => None,
            Kind::Video => Some("Video from words is not available yet. Animate a still under Change instead."),
            Kind::Music => Some("Music from words is not available yet. The AI editor scores from Zornov's library."),
            Kind::Sound => Some("Sound from words is not available yet. The AI editor makes risers, whooshes, booms and hits to measure."),
        };
        if let Some(m) = soon {
            ui.label(egui::RichText::new(crate::i18n::shown(m)).font(theme::caption()).color(t.text_2));
            return;
        }
        let codex = crate::agent::brain::find_codex().is_some();
        if !codex && !cfg!(test) {
            ui.label(egui::RichText::new(crate::i18n::shown("Pictures from words are made by the Codex app's image generation, on your ChatGPT plan. Codex is not on this Mac yet.")).font(theme::caption()).color(t.text_2));
            return;
        }
        ui.add(egui::TextEdit::multiline(&mut f.prompt).hint_text("What to see: subject, setting, light, style").desired_rows(2).desired_width(f32::INFINITY));
        let busy = run().lock().ok().and_then(|g| g.as_ref().and_then(|r| r.lock().ok().map(|r| r.status.clone())));
        if let Some(status) = busy {
            ui.label(egui::RichText::new(format!("{status}…")).font(theme::body()).color(t.text_2));
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        } else if !f.prompt.trim().is_empty() && Button::primary("Generate").icon(Icon::Sparkle).show(ui).clicked() {
            start(app, &f.prompt);
        }
    });
    ctx.data_mut(|d| d.insert_temp(form_id(), f));
}

fn start(app: &StudioApp, prompt: &str) {
    let state = Arc::new(Mutex::new(Run { status: crate::i18n::tf("Codex is painting it".to_owned()), kind: Some(Kind::Picture), at: app.editor.playhead, owner: crate::editor::owner_of(app), ..Default::default() }));
    if let Ok(mut g) = run().lock() {
        *g = Some(state.clone());
    }
    let dir = crate::agent::generate::generated_dir(app.doc.folder.as_ref().map(|f| f.root()));
    let prompt = prompt.to_owned();
    std::thread::spawn(move || {
        let r = crate::agent::generate::make_pictures(&prompt, 1, &dir);
        if let Ok(mut g) = state.lock() {
            g.done = Some(r);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_wav(path: &std::path::Path) {
        let n = 44_100u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + n * 2).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        for v in [16u32.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 44_100u32.to_le_bytes().to_vec(), 88_200u32.to_le_bytes().to_vec(), 2u16.to_le_bytes().to_vec(), 16u16.to_le_bytes().to_vec()] {
            out.extend_from_slice(&v);
        }
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            out.extend_from_slice(&(((i as f32 * 0.03).sin() * 6000.0) as i16).to_le_bytes());
        }
        std::fs::write(path, out).unwrap();
    }

    /// The card draws for every kind, and a finished run lands on a sound
    /// lane at the playhead.
    #[test]
    fn the_generate_card_draws_and_lands_a_run() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        for k in 0..4 {
            ctx.data_mut(|d| d.insert_temp(form_id(), Form { kind: k, prompt: "a fox in the snow".into() }));
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 600.0))), time: Some(k as f64), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| card(&mut app, ui));
        }
        let dir = std::env::temp_dir().join(format!("studio-gen-card-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("score.wav");
        tone_wav(&wav);
        land(&mut app, &ctx, vec![wav], Kind::Music, 2_000_000);
        let mid = app.doc.project.media.iter().find(|m| m.name.starts_with("score")).expect("imported").id;
        let c = app.doc.project.clips.iter().find(|c| c.media == Some(mid)).expect("placed");
        assert_eq!(c.start, 2_000_000);
        assert!(matches!(c.kind, crate::view::ClipKind::Audio));
        let _ = std::fs::remove_dir_all(dir);
    }
}
