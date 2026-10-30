// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Physics" section: drop the selected layers from the
//! playhead and let them fall, bounce and stack, baked into keys.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, Section};
use crate::theme;
use eframe::egui;
use serde_json::json;

// Shared by the panels this branch looks after (body3d, bones, pins,
// scissor, sculpt, slowmo, match_colour, generate, canvas_text, air).
#[path = "cloud_feedback.rs"]
pub(crate) mod cloud_feedback;
#[cfg(test)]
#[path = "cloud_ui_check.rs"]
pub(crate) mod cloud_ui_check;

#[derive(Clone, Copy)]
struct Settings {
    bounce: f32,
    friction: f32,
    seconds: f32,
    walls: bool,
    round: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { bounce: 35.0, friction: 50.0, seconds: 2.5, walls: true, round: false }
    }
}

/// The op the section runs for `ids` from timeline time `at` (seconds).
fn op(ids: &[u32], at: f64, s: Settings) -> serde_json::Value {
    let mut o = json!({"clips": ids, "at": at, "duration": s.seconds, "bounce": s.bounce / 100.0, "friction": s.friction / 100.0, "walls": s.walls});
    if s.round {
        o["shape"] = json!("circle");
    }
    o
}

/// What a drop tells the person.
fn dropped(n: usize) -> String {
    if n > 1 { crate::i18n::tf(format!("Dropped {n} layers; undo puts them back")) } else { "Dropped; undo puts it back".to_owned() }
}

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    let key = egui::Id::unique("physics-settings");
    let mut s = ui.data(|d| d.get_temp::<Settings>(key)).unwrap_or_default();
    Section::new("physics", "Physics").icon(Icon::ArrowsOutCardinal).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        FieldRow::new("Bounce", &mut s.bounce, 0.0..=100.0).default(35.0).unit("%").show(ui);
        FieldRow::new("Friction", &mut s.friction, 0.0..=100.0).default(50.0).unit("%").show(ui);
        FieldRow::new("Run for", &mut s.seconds, 0.5..=10.0).default(2.5).unit("s").decimals(1).step(0.1).scrub(0.02).show(ui);
        kit::labeled_row(ui, "Frame edges", |ui| {
            kit::toggle(ui, &mut s.walls, "Sides hold them in", None);
        });
        kit::labeled_row(ui, "Round", |ui| {
            kit::toggle(ui, &mut s.round, "Roll like balls", None);
        });
        let label = if ids.len() > 1 { crate::i18n::tf(format!("Drop {} layers", ids.len())) } else { "Drop from the playhead".to_owned() };
        if Button::secondary(&label).show(ui).clicked() {
            let at = app.editor.playhead as f64 / 1e6;
            // One undo step; nothing changes when it can't run.
            match crate::agent::physics::run(&mut app.doc, &op(ids, at, s)) {
                Ok(_) => cloud_feedback::done(ui.ctx(), dropped(ids.len())),
                Err(e) => cloud_feedback::failed(ui.ctx(), &app.doc, &e),
            }
        }
        ui.label(egui::RichText::new(crate::i18n::shown("Start them above the frame; they land as keys")).font(theme::caption()).color(t.text_2));
    });
    ui.data_mut(|d| d.insert_temp(key, s));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_asks_for_what_its_settings_say() {
        let o = op(&[3, 4], 1.5, Settings { round: true, ..Default::default() });
        assert_eq!(o["clips"], json!([3, 4]));
        assert_eq!(o["shape"], "circle");
        assert!((o["bounce"].as_f64().unwrap() - 0.35).abs() < 1e-6);
    }

    /// The section drawn at the Inspector's width reads cleanly, and a
    /// drop says what it did.
    #[test]
    fn the_section_reads_cleanly_and_a_drop_reports() {
        let _serial = crate::ui_tests::serial();
        let (ctx, mut app) = cloud_ui_check::editor();
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Drop", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2, "width": 120, "height": 80})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        app.doc = s.doc;
        let drawn = cloud_ui_check::draw(&ctx, 3, |ui| inspector(&mut app, ui, &[id]));
        drawn.assert_tidy("physics", &[]);
        assert!(drawn.shows("Drop from the playhead"));
        assert_eq!(dropped(3), "Dropped 3 layers; undo puts them back");
    }
}
