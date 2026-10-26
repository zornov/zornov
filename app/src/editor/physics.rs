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
        let label = if ids.len() > 1 { crate::i18n::tf(format!("Drop {} layers from the playhead", ids.len())) } else { "Drop from the playhead".to_owned() };
        if Button::secondary(&label).show(ui).clicked() {
            let at = app.editor.playhead as f64 / 1e6;
            // One undo step; nothing changes when it can't run.
            if let Err(e) = crate::agent::physics::run(&mut app.doc, &op(ids, at, s)) {
                app.doc.error = Some(e);
            }
        }
        ui.label(egui::RichText::new(crate::i18n::shown("Place the layers where they start (above the frame, tilted), then drop: they fall to the bottom edge and settle. The result is ordinary keys.")).font(theme::caption()).color(t.text_2));
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
}
