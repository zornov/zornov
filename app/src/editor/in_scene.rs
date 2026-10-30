// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "In the scene" section: a character over a backdrop
//! made to sit in it (`services::backdrop_fit`). For a character, a ground
//! shadow (read from the backdrop's light, then by hand) and a light match;
//! for a backdrop, the pieces a character could stand behind, each laid
//! over the characters with a click; for such a piece, a way to take it
//! off.

use eframe::egui;
use serde_json::{Value, json};
use studio_model as m;

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, Section};
use crate::services::backdrop_fit as fit;
use crate::theme;
use crate::view::ClipKind;

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(vc) = app.doc.project.clip(id).cloned() else { return };
    if !matches!(vc.kind, ClipKind::Video) || vc.media.is_none() {
        return;
    }
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let on_spine = app.doc.project.spine() == Some(vc.lane);
    let at = app.editor.playhead.clamp(vc.start, vc.start + vc.dur - 1);
    let ctx = ui.ctx().clone();
    let seq = app.doc.seq();
    Section::new("in-scene", "In the scene").icon(Icon::Sun).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        let note = |ui: &mut egui::Ui, s: &str| {
            ui.label(egui::RichText::new(s).font(theme::caption()).color(t.text_2));
        };
        if fit::is_piece(&mc) {
            note(ui, "A piece of the backdrop laid over the characters, so they can stand behind it.");
            if Button::ghost("Remove piece").icon(Icon::X).compact().show(ui).clicked() {
                app.doc.apply("Remove foreground piece", m::Edit::Lift { seq, clips: vec![m::ClipId(id as u64)] });
            }
            return;
        }
        if on_spine {
            pieces(app, ui, &ctx, id, at);
            return;
        }
        // The shadow: on / off, read from the light, then by hand.
        let shadow = mc.meta.get("shadow").and_then(Value::as_object).cloned();
        let mut on = shadow.as_ref().is_some_and(|o| o.get("enabled").and_then(Value::as_bool) != Some(false));
        if kit::toggle(ui, &mut on, "Ground shadow", None).changed() {
            let mut c = mc.clone();
            match (&shadow, on) {
                (Some(o), _) => {
                    let mut o = o.clone();
                    o.insert("enabled".into(), json!(on));
                    c.meta.insert("shadow".into(), Value::Object(o));
                    app.doc.apply("Shadow", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
                (None, true) => match from_light(app, &mut c, at) {
                    Ok(said) => {
                        app.doc.apply("Shadow", m::Edit::SetClip { seq, clip: Box::new(c) });
                        let _ = kit::push(&ctx, said, None, Some(Icon::Sun));
                    }
                    Err(e) => {
                        let _ = kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                },
                (None, false) => {}
            }
        }
        if let Some(o) = shadow.filter(|_| on) {
            let mut c = mc.clone();
            let mut o2 = o.clone();
            let mut changed = false;
            for (key, label, range, default, unit, k) in [
                ("angle", "Falls towards", 0.0..=360.0, 60.0, "°", 1.0),
                ("length", "Length", 0.0..=150.0, 45.0, "%", 100.0),
                ("softness", "Softness", 0.0..=30.0, 4.0, "%", 100.0),
                ("opacity", "Darkness", 0.0..=100.0, 45.0, "%", 100.0),
                ("contact", "Under the feet", 0.0..=100.0, 60.0, "%", 100.0),
            ] {
                let mut v = o.get(key).and_then(Value::as_f64).map_or(default, |v| (v * k) as f32);
                if kit::Slider::new(label, &mut v, range).default(default).unit(unit).decimals(0).show(ui).changed() {
                    o2.insert(key.into(), json!((v as f64) / k));
                    changed = true;
                }
            }
            if changed {
                c.meta.insert("shadow".into(), Value::Object(o2));
                app.doc.apply_gesture("in-scene-shadow", "Shadow", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
            if Button::secondary("Read the light again").icon(Icon::Sun).show(ui).clicked() {
                let mut c = mc.clone();
                match from_light(app, &mut c, at) {
                    Ok(said) => {
                        app.doc.apply("Shadow", m::Edit::SetClip { seq, clip: Box::new(c) });
                        let _ = kit::push(&ctx, said, None, Some(Icon::Sun));
                    }
                    Err(e) => {
                        let _ = kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
        }
        // The light: matched to where it stands.
        let matched = mc.meta.contains_key("light_match");
        let pid = egui::Id::unique("in-scene-amount").with(id);
        let mut amount: f32 = ctx.data(|d| d.get_temp(pid)).unwrap_or_else(|| mc.meta.get("light_match").and_then(|o| o.get("amount")).and_then(Value::as_f64).map_or(100.0, |a| (a * 100.0) as f32));
        kit::Slider::new("Light match", &mut amount, 0.0..=200.0).default(100.0).unit("%").decimals(0).show(ui);
        ctx.data_mut(|d| d.insert_temp(pid, amount));
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(if matched { "Match again" } else { "Match the light" }).icon(Icon::Eyedropper).show(ui).clicked() {
                let mut c = mc.clone();
                let rim_id = app.doc.alloc_id();
                let res = fit::backdrop_picture(&app.doc, at, 512).and_then(|img| fit::light_match(&app.doc, &mut c, at, &img, amount / 100.0, 1.0, rim_id));
                match res {
                    Ok(said) => {
                        app.doc.apply("Light match", m::Edit::SetClip { seq, clip: Box::new(c) });
                        let _ = kit::push(&ctx, format!("Lit like its place: {said}"), None, Some(Icon::Sun));
                    }
                    Err(e) => {
                        let _ = kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
            if matched && Button::ghost("Remove").icon(Icon::X).compact().show(ui).clicked() {
                let mut c = mc.clone();
                fit::clear_light_match(&mut c);
                app.doc.apply("Remove light match", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
        });
        note(ui, "Read from the backdrop at the playhead, with the characters left out.");
    });
}

/// `c`'s shadow read from the backdrop's light at `at`.
fn from_light(app: &StudioApp, c: &mut m::Clip, at: crate::view::Time) -> Result<String, String> {
    let img = fit::backdrop_picture(&app.doc, at, 512)?;
    let light = fit::read_light(&img);
    fit::auto_shadow(&app.doc, c, at, &light, fit::horizon(&img))
}

/// The backdrop's pieces a character could stand behind: found on demand
/// (the depth model reads the picture), each laid over the characters.
fn pieces(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context, id: u32, at: crate::view::Time) {
    use crate::agent::sight::ground::DepthGrid;
    let t = theme::tokens(ui.ctx());
    let key = egui::Id::unique("in-scene-pieces").with(id);
    let found: Option<(crate::view::Time, Vec<([f32; 4], [f32; 2], [f32; 2], String)>)> = ctx.data(|d| d.get_temp(key));
    ui.label(egui::RichText::new("Backdrop pieces (rocks, trunks, grass) to stand behind").font(theme::caption()).color(t.text_2));
    if Button::secondary("Find pieces").icon(Icon::Eyedropper).show(ui).clicked() {
        let res = (|| -> Result<Vec<([f32; 4], [f32; 2], [f32; 2], String)>, String> {
            let store = studio_ai::store::ModelStore::default();
            if !store.is_installed(studio_ai::registry::ModelId::DepthAnythingV2Small) {
                return Err("Finding pieces reads depth: get the depth model in Settings → AI models".into());
            }
            let img = fit::backdrop_picture(&app.doc, at, 512)?;
            let est = studio_ai::features::depth::DepthEstimator::load(&store).map_err(|e| e.to_string())?;
            let frame = studio_ai::frame::Frame::new(img.width as usize, img.height as usize, img.pixels.clone()).map_err(|e| e.to_string())?;
            let d = est.estimate(&frame).map_err(|e| e.to_string())?;
            Ok(DepthGrid::shrink(d.width, d.height, &d.data, 96).pieces().into_iter().map(|p| (p.bbox, p.inside, p.land, p.short().to_string())).collect())
        })();
        match res {
            Ok(v) => {
                ctx.data_mut(|d| d.insert_temp(key, (at, v)));
            }
            Err(e) => {
                let _ = kit::push(ctx, e, None, Some(Icon::Warning));
            }
        }
    }
    let Some((when, list)) = found else { return };
    if list.is_empty() {
        ui.label(egui::RichText::new("Nothing stands out from the ground here.").font(theme::caption()).color(t.text_2));
        return;
    }
    let (w, h) = (app.doc.project.width as f32, app.doc.project.height as f32);
    let to_canvas = |q: [f32; 2]| egui::vec2((q[0] - 0.5) * w, (q[1] - 0.5) * h);
    for (i, (bbox, inside, land, kind)) in list.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("{}. a {kind}", i + 1)).font(theme::caption()));
            if Button::ghost("Lay over characters").compact().show(ui).clicked() {
                let cut = fit::PieceCut {
                    backdrop: id,
                    at: when,
                    pick: crate::services::cutout::Pick { inside: vec![to_canvas(*inside)], outside: Vec::new(), corners: Some([to_canvas([bbox[0], bbox[1]]), to_canvas([bbox[2], bbox[3]])]) },
                    behind: Some(*land),
                    kind: kind.clone(),
                    hides: None,
                };
                match fit::lay_piece(&mut app.doc, &cut) {
                    Ok((_, said)) => {
                        let _ = kit::push(ctx, said, None, Some(Icon::Sun));
                    }
                    Err(e) => {
                        let _ = kit::push(ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
        });
    }
}
