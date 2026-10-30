// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Match colour" section: pick another shot and this one
//! takes on its colour and light (`services::colour_match`), with a
//! strength to blend it in.

use eframe::egui;
use studio_model as m;

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, Section};
use super::physics::cloud_feedback as feedback;
use crate::services::colour_match as cm;
use crate::theme;
use crate::view::ClipKind;

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(vc) = app.doc.project.clip(id).cloned() else { return };
    if !matches!(vc.kind, ClipKind::Video) || vc.media.is_none() {
        return;
    }
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let others: Vec<(u32, String)> = app
        .doc
        .project
        .clips
        .iter()
        .filter(|c| c.id != id && matches!(c.kind, ClipKind::Video) && c.media.is_some())
        .map(|c| (c.id, format!("{} ({:.1}s)", if c.name.is_empty() { format!("Clip {}", c.id) } else { c.name.clone() }, c.start as f64 / 1e6)))
        .collect();
    let ctx = ui.ctx().clone();
    let seq = app.doc.seq();
    Section::new("match-colour", "Match colour").icon(Icon::Palette).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        if others.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::shown("Add another video shot to match this one to")).font(theme::caption()).color(t.text_2));
            return;
        }
        let pid = egui::Id::unique("match-colour-pick").with(id);
        let (mut who, mut strength): (usize, f32) = ctx.data(|d| d.get_temp(pid)).unwrap_or((0, cm::strength(&mc).map_or(100.0, |s| (s * 100.0) as f32)));
        who = who.min(others.len() - 1);
        let names: Vec<&str> = others.iter().map(|o| o.1.as_str()).collect();
        kit::labeled_row(ui, "To", |ui| {
            if let Some(i) = kit::menu_button(ui, "match-colour-to", who, &names, ui.available_width(), None) {
                who = i;
            }
        });
        let changed = FieldRow::new("Strength", &mut strength, 0.0..=100.0).default(100.0).unit("%").decimals(0).show(ui).changed;
        ctx.data_mut(|d| d.insert_temp(pid, (who, strength)));
        // A matched clip follows the slider at once.
        if changed && cm::strength(&mc).is_some() {
            let mut c = mc.clone();
            if let Some(g) = &mut c.grade {
                g.params.insert("match_strength".into(), m::Value::Scalar((strength / 100.0) as f64));
            }
            app.doc.apply_gesture("match-colour-strength", "Match strength", m::Edit::SetClip { seq, clip: Box::new(c) });
        }
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(if cm::strength(&mc).is_some() { "Match again" } else { "Match" }).icon(Icon::Eyedropper).show(ui).clicked() {
                let rid = others[who].0;
                let p = app.doc.project.clone();
                let res = app.doc.model.active().clip(m::ClipId(rid as u64)).cloned().ok_or_else(|| "that shot is gone".to_owned()).and_then(|rc| {
                    let want = cm::stats(&p, &rc, true)?;
                    let mut c = mc.clone();
                    cm::clear(&mut c);
                    let have = cm::stats(&p, &c, false)?;
                    let said = cm::difference(&cm::stats(&p, &c, true)?, &want);
                    cm::set(&mut c, &have, &want, (strength / 100.0) as f64);
                    Ok((c, said))
                });
                match res {
                    Ok((c, said)) => {
                        app.doc.apply("Match colour", m::Edit::SetClip { seq, clip: Box::new(c) });
                        kit::push(&ctx, crate::i18n::tf(format!("Matched to {}: {said}", others[who].1)), None, Some(Icon::Palette));
                    }
                    Err(e) => feedback::failed(&ctx, &app.doc, &e),
                }
            }
            if cm::strength(&mc).is_some() && Button::ghost("Remove").icon(Icon::X).compact().show(ui).clicked() {
                let mut c = mc.clone();
                cm::clear(&mut c);
                if app.doc.apply("Remove colour match", m::Edit::SetClip { seq, clip: Box::new(c) }) {
                    feedback::done(&ctx, "Colour match removed");
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole editor, headless, with two shots and the first matched:
    /// the section draws, and the strength slider's value is the match's.
    #[test]
    fn the_editor_draws_the_match_colour_section() {
        let _guard = crate::services::tracking::test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Match", 640, 360, 30.0));
        let movie = crate::services::tracking::test_movie::movie();
        s.run("import", &serde_json::json!({"paths": [movie.to_string_lossy(), movie.to_string_lossy()], "to_timeline": true})).unwrap();
        let ids: Vec<u32> = s.doc.project.clips.iter().filter(|c| c.media.is_some() && matches!(c.kind, ClipKind::Video)).map(|c| c.id).collect();
        s.edit(&[serde_json::json!({"op": "match_colour", "clip": ids[0], "to": ids[1], "strength": 70})], "m", false).unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![ids[0]];
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..3 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), time: Some(i as f64 / 60.0), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        }
        let mc = app.doc.model.active().clip(m::ClipId(ids[0] as u64)).unwrap();
        assert_eq!(cm::strength(mc), Some(0.7));
        // At the Inspector's width: a labelled strength row (it was a bare
        // track), nothing cut or on top of anything.
        let names: Vec<String> = app.doc.project.clips.iter().map(|c| c.name.clone()).filter(|n| !n.is_empty()).collect();
        let allow: Vec<&str> = names.iter().map(String::as_str).collect();
        let drawn = crate::editor::physics::cloud_ui_check::draw(&ctx, 3, |ui| inspector(&mut app, ui, ids[0]));
        drawn.assert_tidy("match-colour", &allow);
        assert!(drawn.shows("Strength") && drawn.shows("Match again"));
    }
}
