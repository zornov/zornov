// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Drivers" section: link a property of this clip to a
//! property of another (a scale and a delay, or a formula), and see,
//! pause, edit or remove the links it has.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, IconButton, Section};
use crate::theme;
use eframe::egui;
use studio_model as m;

/// Properties offered: (label, path, axis).
const PROPS: &[(&str, &str, Option<u8>)] = &[
    ("Position X", "transform.position", Some(0)),
    ("Position Y", "transform.position", Some(1)),
    ("Rotation", "transform.rotation", None),
    ("Scale X", "transform.scale_x", None),
    ("Scale Y", "transform.scale_y", None),
    ("Opacity", "transform.opacity", None),
    ("Depth (Z)", "transform.position_z", None),
    ("Tilt (X rotation)", "transform.rotation_x", None),
    ("Turn (Y rotation)", "transform.rotation_y", None),
];

#[derive(Clone)]
struct Form {
    target: usize,
    source: Option<u32>,
    prop: usize,
    multiply: f32,
    delay: f32,
}

impl Default for Form {
    fn default() -> Self {
        Form { target: 2, source: None, prop: 2, multiply: 1.0, delay: 0.0 }
    }
}

/// The driver key for an offered property.
pub fn key_of(i: usize) -> String {
    let (_, path, axis) = PROPS[i];
    match axis {
        Some(c) => format!("{path}#{c}"),
        None => path.to_owned(),
    }
}

/// The driver a link from the form makes: `a × multiply`, `a` read
/// `delay` seconds late.
pub fn link(source: u32, prop: usize, multiply: f64, delay: f64) -> m::Driver {
    let (_, path, axis) = PROPS[prop];
    let expr = if (multiply - 1.0).abs() < 1e-9 { "a".to_owned() } else { format!("a * {multiply}") };
    m::Driver { expr, vars: vec![m::DriverVar { name: "a".into(), clip: m::ClipId(source as u64), property: path.into(), component: axis, delay: (delay * 1e6).round() as m::Micros }], enabled: true }
}

/// How a driver reads, in a line.
pub fn summary(proj: &crate::view::Project, mc: &m::Clip, key: &str, d: &m::Driver) -> String {
    let (path, axis) = m::driver::split_key(key);
    let mut name = super::graph::label(mc, path);
    if let Some(a) = axis {
        name = format!("{name} {}", if a == 0 { "X" } else { "Y" });
    }
    let from: Vec<String> = d
        .vars
        .iter()
        .map(|v| {
            let who = proj.clip(v.clip.0 as u32).map_or(format!("#{}", v.clip.0), |c| c.name.clone());
            format!("{} = {who} {}", v.name, v.property.rsplit('.').next().unwrap_or(&v.property).replace('_', " "))
        })
        .collect();
    format!("{name} ← {}  ({})", d.expr, from.join(", "))
}

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let fid = egui::Id::unique(("driver-form", id));
    let mut form: Form = ui.data(|d| d.get_temp(fid)).unwrap_or_default();
    let mut changed: Option<m::Clip> = None;
    let out = Section::new("drivers", "Drivers").icon(Icon::LinkSimple).resettable(!mc.drivers.is_empty()).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        for (k, d) in &mc.drivers {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(summary(&app.doc.project, &mc, k, d)).font(theme::caption()).color(if d.enabled { t.text } else { t.text_2 }));
            let mut on = d.enabled;
            let mut expr = d.expr.clone();
            let mut remove = false;
            ui.horizontal(|ui| {
                kit::toggle(ui, &mut on, "Driver on", None);
                // The field leaves its remove button room.
                let w = (ui.available_width() - theme::space::CONTROL_H_COMPACT - ui.spacing().item_spacing.x - 2.0).max(60.0);
                kit::text_field(ui, &mut expr, "a * 0.5", None, w, None);
                remove = IconButton::new(Icon::X, "Remove driver").compact().show(ui).clicked();
            });
            let mut nd = d.clone();
            nd.enabled = on;
            nd.expr = expr;
            if let Some(p) = nd.problem() {
                ui.label(egui::RichText::new(p).font(theme::caption()).color(t.danger));
            } else if remove || nd != *d {
                let mut c = changed.take().unwrap_or_else(|| mc.clone());
                if remove {
                    c.drivers.remove(k);
                } else {
                    c.drivers.insert(k.clone(), nd);
                }
                changed = Some(c);
            }
        }
        if !mc.drivers.is_empty() {
            kit::menu_separator(ui);
        }
        // A new link.
        let names: Vec<&str> = PROPS.iter().map(|p| p.0).collect();
        kit::labeled_row(ui, "Drive", |ui| {
            if let Some(i) = kit::menu_button(ui, "drv-target", form.target, &names, ui.available_width(), None) {
                form.target = i;
            }
        });
        let others: Vec<(u32, String)> = app.doc.project.clips.iter().filter(|c| c.id != id && c.is_visual()).map(|c| (c.id, if c.name.is_empty() { format!("#{}", c.id) } else { c.name.clone() })).collect();
        let mut src_names: Vec<&str> = vec!["Pick a clip…"];
        src_names.extend(others.iter().map(|o| o.1.as_str()));
        let cur = form.source.and_then(|s| others.iter().position(|o| o.0 == s)).map_or(0, |i| i + 1);
        kit::labeled_row(ui, "From", |ui| {
            if let Some(i) = kit::menu_button(ui, "drv-source", cur, &src_names, ui.available_width(), None) {
                form.source = (i > 0).then(|| others[i - 1].0);
            }
        });
        kit::labeled_row(ui, "Its", |ui| {
            if let Some(i) = kit::menu_button(ui, "drv-prop", form.prop, &names, ui.available_width(), None) {
                form.prop = i;
            }
        });
        FieldRow::new("Multiply", &mut form.multiply, -10.0..=10.0).default(1.0).decimals(2).step(0.05).scrub(0.01).show(ui);
        FieldRow::new("Delay", &mut form.delay, 0.0..=5.0).default(0.0).unit("s").decimals(2).step(0.01).scrub(0.005).show(ui);
        let ready = form.source.is_some() && mc.get_property(PROPS[form.target].1).is_some();
        ui.add_enabled_ui(ready, |ui| {
            if Button::secondary("Link").show(ui).clicked()
                && let Some(src) = form.source
            {
                let mut c = changed.take().unwrap_or_else(|| mc.clone());
                c.drivers.insert(key_of(form.target), link(src, form.prop, form.multiply as f64, form.delay as f64));
                changed = Some(c);
            }
        });
        ui.label(egui::RichText::new(crate::i18n::shown("The property follows the other clip's live: move that one and this one moves. Formulas use stored units (scale and opacity: 1 = 100 %).")).font(theme::caption()).color(t.text_2));
    });
    if out.reset {
        let mut c = mc.clone();
        c.drivers.clear();
        changed = Some(c);
    }
    if let Some(c) = changed {
        let seq = app.doc.seq();
        app.doc.apply("Driver", m::Edit::SetClip { seq, clip: Box::new(c) });
    }
    ui.data_mut(|d| d.insert_temp(fid, form));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_reads_the_chosen_axis_late_and_scaled() {
        let d = link(4, 1, 0.5, 0.25);
        assert_eq!(d.expr, "a * 0.5");
        assert_eq!(d.vars[0].component, Some(1));
        assert_eq!(d.vars[0].delay, 250_000);
        assert!(d.problem().is_none());
        assert_eq!(key_of(0), "transform.position#0");
        assert_eq!(key_of(2), "transform.rotation");
    }

    /// The whole editor, headless, with a driven clip selected and its
    /// Animation tab open: the section draws, the canvas places the
    /// driven layer, nothing changes the project.
    #[test]
    fn the_editor_draws_a_driven_clip_and_its_drivers() {
        use eframe::egui::{Pos2, Rect, Vec2};
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Drivers", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2, "width": 100, "height": 100}), serde_json::json!({"op": "card", "colour": "#3366ff", "at": 0, "duration": 2, "width": 100, "height": 100})], "cards", false).unwrap();
        let mut ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        ids.sort();
        let (lead, follow) = (ids[ids.len() - 2], ids[ids.len() - 1]);
        s.edit(&[serde_json::json!({"op": "set", "clip": lead, "x": 150, "rotation": 30}), serde_json::json!({"op": "driver", "clip": follow, "property": "x", "from": {"clip": lead, "property": "x"}, "multiply": -1}), serde_json::json!({"op": "driver", "clip": follow, "property": "rotation", "from": {"clip": lead, "property": "rotation"}})], "link", false).unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![follow];
        let before = app.doc.model.clone();
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..4 {
            // The Animation tab of a title (cards are titles).
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(("insp-tab-of", "Text")), 1usize));
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(i as f64 / 60.0), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        }
        assert_eq!(app.doc.model, before);
        assert!(ctx.data(|d| d.get_temp::<Form>(egui::Id::unique(("driver-form", follow)))).is_some(), "the Drivers section drew");
        // The canvas puts it where the driver says.
        let g = super::super::xform::geo(&app.doc.project, &app.doc.model, follow, 0).unwrap();
        let q = g.quad();
        let cx = q.iter().map(|p| p.x).sum::<f32>() / 4.0;
        assert!((cx + 150.0).abs() < 1.0, "mirrored to x = -150: {cx}");
    }
}
