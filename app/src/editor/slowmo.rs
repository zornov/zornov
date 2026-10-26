// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Smooth slow motion in the editor: the AI panel's "Smooth slow motion"
//! and the Inspector's "Slow motion" row run `services::slowmo` in the
//! background on a slowed (or ramped) clip.

use std::sync::Mutex;

use eframe::egui;
use studio_ai::features::interpolate::InterpEngine;
use studio_ai::{JobHandle, Jobs};
use studio_model as m;

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, Section};
use crate::services::slowmo::{self, Method, SlowJob};
use crate::theme;
use crate::view::ClipKind;

fn jobs() -> &'static Jobs {
    static J: std::sync::OnceLock<Jobs> = std::sync::OnceLock::new();
    J.get_or_init(|| Jobs::new(1))
}

fn running() -> &'static Mutex<Option<(SlowJob, JobHandle<InterpEngine>, usize)>> {
    static R: Mutex<Option<(SlowJob, JobHandle<InterpEngine>, usize)>> = Mutex::new(None);
    &R
}

pub fn busy() -> bool {
    running().lock().map(|r| r.is_some()).unwrap_or(false)
}

/// Starts smoothing clip `id`; returns what to tell the person.
pub fn start(app: &StudioApp, ctx: &egui::Context, id: u32, method: Method) -> Result<String, String> {
    if busy() {
        return Err("smooth slow motion is already running".into());
    }
    let job = slowmo::plan(&app.doc, id, method, false)?;
    let est = job.estimate_secs();
    let j = job.clone();
    let handle = jobs().spawn("Smooth slow motion", move |c| slowmo::run(&j, c).map_err(studio_ai::Error::Io));
    if let Ok(mut r) = running().lock() {
        *r = Some((job.clone(), handle, crate::editor::owner_of(app)));
    }
    ctx.request_repaint();
    Ok(crate::i18n::tf(format!("Making {} in-between frames (about {})…", job.to_make(), if est < 90.0 { format!("{est:.0} s") } else { format!("{:.0} min", est / 60.0) })))
}

/// Applies a finished bake (called every frame).
pub fn poll(app: &mut StudioApp, ctx: &egui::Context) {
    let done = {
        let Ok(mut r) = running().lock() else { return };
        match r.as_ref() {
            Some((_, h, owner)) if *owner == crate::editor::owner_of(app) && h.is_finished() => r.take(),
            Some((_, _, owner)) if *owner == crate::editor::owner_of(app) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                None
            }
            _ => None,
        }
    };
    let Some((job, h, _)) = done else { return };
    match h.take_result() {
        Some(Ok(engine)) => match slowmo::apply(&mut app.doc, &job, engine) {
            Ok(_) => {
                kit::push(ctx, crate::i18n::tf(format!("Smooth slow motion: {}× the frames", job.factor)), None, Some(Icon::Sparkle));
            }
            Err(e) => {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
        },
        Some(Err(studio_ai::Error::Cancelled)) => {
            kit::push(ctx, "Smooth slow motion stopped", None, Some(Icon::Info));
        }
        Some(Err(e)) => {
            kit::push(ctx, crate::i18n::tf(format!("Smooth slow motion failed: {e}")), None, Some(Icon::Warning));
        }
        None => {}
    }
}

/// The Inspector's "Slow motion" row, on a slowed or smoothed clip.
pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let ctx = ui.ctx().clone();
    poll(app, &ctx);
    let Some(vc) = app.doc.project.clip(id).cloned() else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let slowed = (0..8).any(|k| mc.speed.rate_at(mc.duration * k / 8) < 0.95);
    let smoothed = slowmo::describe(&mc);
    if !matches!(vc.kind, ClipKind::Video) || (!slowed && smoothed.is_none()) {
        return;
    }
    let seq = app.doc.seq();
    Section::new("slow-motion", "Slow motion").icon(Icon::Timer).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        if let Some(said) = &smoothed {
            ui.label(egui::RichText::new(said).font(theme::caption()).color(t.text_2));
            if Button::ghost("Remove").icon(Icon::X).compact().show(ui).clicked()
                && let Some(c) = slowmo::restored(&mc)
            {
                app.doc.apply("Remove smooth slow motion", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
            return;
        }
        if let Some(p) = running().lock().ok().and_then(|r| r.as_ref().filter(|(j, _, _)| j.clip == id).map(|(_, h, _)| h.progress())) {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(crate::i18n::tf(format!("Making in-between frames… {:.0}%", p * 100.0))).font(theme::body()).color(t.text_2));
                if Button::ghost("Stop").show(ui).clicked()
                    && let Ok(r) = running().lock()
                    && let Some((_, h, _)) = r.as_ref()
                {
                    h.cancel();
                }
            });
            return;
        }
        ui.label(egui::RichText::new(crate::i18n::shown("Slowed footage repeats frames. Make the in-betweens: optical flow is sharp, a cross-fade is instant.")).font(theme::caption()).color(t.text_2));
        ui.horizontal_wrapped(|ui| {
            for (label, method) in [("Optical flow", Method::Flow), ("Cross-fade", Method::Blend)] {
                if Button::secondary(label).icon(Icon::Sparkle).compact().show(ui).clicked() {
                    match start(app, &ctx, id, method) {
                        Ok(msg) => {
                            kit::push(&ctx, msg, None, Some(Icon::Sparkle));
                        }
                        Err(e) => {
                            kit::push(&ctx, e, None, Some(Icon::Warning));
                        }
                    }
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tracking::test_movie::{self, FPS, H, W};
    use serde_json::json;

    /// The whole editor with a slowed clip selected draws the row; a
    /// cross-fade run lands on the clip, and the row then offers Remove.
    #[test]
    fn the_slow_motion_row_runs_and_lands() {
        let _guard = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Slow", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": id, "end": 0.4}), json!({"op": "set", "clip": id, "speed": 0.5})], "slow", false).unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![id];
        let mut frame = eframe::Frame::_new_kittest();
        let mut draw = |app: &mut StudioApp, t: f64| {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), time: Some(t), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(app, ui, &mut frame));
        };
        draw(&mut app, 0.0);
        start(&app, &ctx, id, Method::Blend).unwrap();
        for i in 0..15000 {
            if !busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            poll(&mut app, &ctx);
            if i % 10 == 0 {
                draw(&mut app, 1.0 + i as f64 / 60.0);
            }
        }
        poll(&mut app, &ctx);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap();
        assert!(slowmo::describe(mc).is_some(), "smoothed");
        draw(&mut app, 20.0);
    }
}
