// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion graphics in the editor.
//!
//! * The viewer toolbar's motion graphics button: the template browser
//!   (search, sections, a description on hover; a click places one at
//!   the playhead), shape layer presets, 3D text, charts.
//! * The Inspector: a placed template's controls (Essential Graphics:
//!   change them and the template rebuilds in place), a shape layer's
//!   items (numbers and colours to adjust, items to add or remove), and a
//!   title's animators, path and kerning.
//!
//! Everything goes through the same ops the AI uses, so the app and the
//! AI make the same things.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, IconButton, MenuItem, NumberField, Section};
use crate::theme;
use eframe::egui::{self, Color32};
use serde_json::{Value, json};
use studio_model as m;

/// Runs ops on the document as one undo step, as the AI's edit tool
/// does. Returns the ops' notes, or why nothing changed.
pub(super) fn run(app: &mut StudioApp, label: &str, ops: Vec<Value>) -> Result<Vec<String>, String> {
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let r = s.edit(&ops, label, false);
    app.doc = s.doc;
    r.map(|v| v["notes"].as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect()).unwrap_or_default())
}

/// The first "clip N" a note names.
fn clip_named(notes: &[String]) -> Option<u32> {
    notes.iter().find_map(|n| {
        let at = n.find("clip ")?;
        n[at + 5..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    })
}

/// The toast after adding `what`: a plain sentence, never the operation's own note.
fn added(what: &str) -> String {
    crate::i18n::tf(format!("Added {what}"))
}

fn done(app: &mut StudioApp, ctx: &egui::Context, r: Result<Vec<String>, String>, icon: Icon, what: &str) {
    match r {
        Ok(notes) => {
            if let Some(id) = clip_named(&notes) {
                app.editor.selection = vec![id];
            }
            if !notes.is_empty() {
                kit::push(ctx, added(what), None, Some(icon));
            }
        }
        Err(e) => {
            kit::push(ctx, e, None, Some(Icon::Warning));
        }
    }
}

/// The motion graphics part of the viewer's Add menu: templates, shape
/// layers, charts and 3D text. `q` is the menu's search; `close` closes the
/// menu after a pick.
pub fn add_items(app: &mut StudioApp, ui: &mut egui::Ui, q: &str, close: egui::Id) {
    use super::viewer::{add_item, add_shows};
    let ctx = ui.ctx().clone();
    let mut pick: Option<&'static str> = None;
    let mut preset: Option<&'static str> = None;
    let mut chart: Option<&'static str> = None;
    let mut text_3d = false;
    let found = studio_mograph::search(q);
    for cat in studio_mograph::Category::ALL {
        let here: Vec<_> = found.iter().filter(|t| t.category == cat).collect();
        if here.is_empty() {
            continue;
        }
        kit::menu_heading(ui, cat.label());
        for t in here {
            let r = MenuItem::new(t.name).show(ui);
            let r = kit::tip(r, t.description, None);
            if r.clicked() {
                pick = Some(t.id);
                kit::set_open(&ctx, close, false);
            }
        }
    }
    if add_shows(q, "Shape layer", studio_vector::presets::PRESETS.iter().map(|p| p.0)) {
        kit::menu_heading(ui, "Shape layer");
        for (name, desc) in studio_vector::presets::PRESETS {
            let label = name.replace('_', " ");
            if !add_shows(q, "Shape layer", [label.as_str()]) {
                continue;
            }
            let r = kit::tip(MenuItem::new(&label).show(ui), desc, None);
            if r.clicked() {
                preset = Some(name);
                kit::set_open(&ctx, close, false);
            }
        }
    }
    let charts = [("Bar chart", "bar"), ("Line chart", "line"), ("Donut chart", "donut"), ("Pie chart", "pie"), ("Route map", "map")];
    if add_shows(q, "Chart from data", charts.iter().map(|c| c.0)) {
        kit::menu_heading(ui, "Chart from data");
        for (label, kind) in charts {
            if add_item(ui, q, "Chart from data", label) {
                chart = Some(kind);
                kit::set_open(&ctx, close, false);
            }
        }
    }
    if add_shows(q, "3D text", ["3D text"]) {
        kit::menu_heading(ui, "3D text");
        if kit::tip(MenuItem::new("3D text").show(ui), "Extruded, bevelled letters lit by the scene; uses the selected title's words", None).clicked() {
            text_3d = true;
            kit::set_open(&ctx, close, false);
        }
    }
    let at = app.editor.playhead as f64 / 1e6;
    if let Some(t) = pick {
        let r = run(app, "Add template", vec![json!({"op": "template", "id": t, "at": at})]);
        let name = studio_mograph::find(t).map_or_else(|| "template".to_owned(), |m| m.name.to_string());
        done(app, &ctx, r, Icon::Shapes, &name);
    }
    if let Some(p) = preset {
        let r = run(app, "Add shape layer", vec![json!({"op": "shape", "preset": p, "at": at, "duration": 4})]);
        done(app, &ctx, r, Icon::Shapes, "shape layer");
    }
    if let Some(k) = chart {
        let template = match k {
            "line" => "chart.line",
            "map" => "chart.map",
            "pie" | "donut" => "chart.pie",
            _ => "chart.bar",
        };
        let data = studio_mograph::find(template).and_then(|t| t.controls.iter().find(|c| c.id == "data").map(|c| c.default.clone())).unwrap_or(json!([]));
        let r = run(app, "Add chart", vec![json!({"op": "chart", "kind": k, "data": data, "at": at})]);
        done(app, &ctx, r, Icon::Shapes, "chart");
    }
    if text_3d {
        let words = app.editor.selection.first().and_then(|id| app.doc.project.clip(*id)).map(|c| c.text.clone()).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "STUDIO".into());
        let r = run(app, "Add 3D text", vec![json!({"op": "text_3d", "text": words, "at": at, "duration": 4})]);
        done(app, &ctx, r, Icon::Aperture, "3D text");
    }
}

fn hex_of(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn colour_of(s: &str) -> Color32 {
    let h = s.trim().trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    if h.len() >= 6 { Color32::from_rgb(b(0), b(2), b(4)) } else { Color32::WHITE }
}

/// The Inspector sections for motion graphics clips.
pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    if mc.meta.contains_key("template") {
        template_section(app, ui, &mc);
    }
    if let m::ClipSource::Generator(g) = &mc.source {
        match g.kind {
            m::GeneratorKind::Shape if g.params.contains_key("vector") => shape_section(app, ui, &mc, g),
            m::GeneratorKind::Text => text_section(app, ui, &mc, g),
            _ => {}
        }
    }
}

fn template_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip) {
    let meta = &mc.meta["template"];
    let Some(t) = meta.get("id").and_then(Value::as_str).and_then(studio_mograph::find) else { return };
    let ctx = ui.ctx().clone();
    let id = mc.id.0 as u32;
    let saved = meta.get("controls").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut changed: Option<(String, Value)> = None;
    Section::new("mograph-template", "Template").icon(Icon::Shapes).show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        ui.label(egui::RichText::new(format!("{} — {}", t.name, t.description)).font(theme::caption()).color(tk.text_2));
        template_controls(ui, id, t, &saved, &mut changed);
    });
    if let Some((k, v)) = changed {
        let r = run(app, "Template control", vec![json!({"op": "template", "clip": id, "controls": {k: v}})]);
        if let Err(e) = r {
            kit::push(&ctx, e, None, Some(Icon::Warning));
        }
    }
}

/// One row per control of a template (text, colour, number, choice, switch,
/// point, data table); returns the ids it drew. What the person changed goes
/// to `changed`.
pub(crate) fn template_controls(ui: &mut egui::Ui, id: u32, t: &studio_mograph::Template, saved: &serde_json::Map<String, Value>, changed: &mut Option<(String, Value)>) -> Vec<&'static str> {
    let mut drawn = Vec::new();
    {
        for c in &t.controls {
            drawn.push(c.id);
            let cur = saved.get(c.id).cloned().unwrap_or_else(|| c.default.clone());
            kit::labeled_row(ui, c.label, |ui| match &c.kind {
                studio_mograph::Kind::Text => {
                    let key = egui::Id::new(("mograph-text", id, c.id));
                    let mut s: String = ui.data(|d| d.get_temp(key)).unwrap_or_else(|| cur.as_str().map(str::to_owned).unwrap_or_else(|| cur.to_string()));
                    let r = kit::text_field(ui, &mut s, c.label, None, 180.0, None);
                    if r.changed() {
                        ui.data_mut(|d| d.insert_temp(key, s.clone()));
                    }
                    if r.lost_focus() && Some(s.as_str()) != cur.as_str() {
                        *changed = Some((c.id.to_owned(), json!(s)));
                        ui.data_mut(|d| d.remove::<String>(key));
                    }
                }
                studio_mograph::Kind::Colour => {
                    let mut col = colour_of(cur.as_str().unwrap_or("#ffffff"));
                    if kit::colour_well(ui, &format!("mograph-{id}-{}", c.id), &mut col, None).changed() {
                        *changed = Some((c.id.to_owned(), json!(hex_of(col))));
                    }
                }
                studio_mograph::Kind::Number { min, max } => {
                    let mut v = cur.as_f64().unwrap_or(0.0) as f32;
                    let r = NumberField::new(c.id, &mut v).range(*min as f32..=*max as f32).decimals(2).width(90.0).show(ui);
                    if r.changed() {
                        *changed = Some((c.id.to_owned(), json!(v)));
                    }
                }
                studio_mograph::Kind::Choice { options } => {
                    ui.horizontal_wrapped(|ui| {
                        for o in options {
                            if Button::ghost(o).compact().show(ui).clicked() && cur.as_str() != Some(*o) {
                                *changed = Some((c.id.to_owned(), json!(o)));
                            }
                        }
                    });
                }
                studio_mograph::Kind::Toggle => {
                    let mut b = cur.as_bool().unwrap_or(false);
                    if kit::toggle(ui, &mut b, c.label, None).changed() {
                        *changed = Some((c.id.to_owned(), json!(b)));
                    }
                }
                studio_mograph::Kind::Point => {
                    let mut p = [cur.get(0).and_then(Value::as_f64).unwrap_or(0.0) as f32, cur.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32];
                    ui.horizontal_wrapped(|ui| {
                        let a = NumberField::new("x", &mut p[0]).unit("px").width(70.0).show(ui).changed();
                        let b = NumberField::new("y", &mut p[1]).unit("px").width(70.0).show(ui).changed();
                        if a || b {
                            *changed = Some((c.id.to_owned(), json!([p[0], p[1]])));
                        }
                    });
                }
                studio_mograph::Kind::Data => {
                    // One row a line: "label, value" (a map: "place, lon, lat").
                    let key = egui::Id::new(("mograph-data", id, c.id));
                    let rows = studio_mograph::data::rows(&cur);
                    let text = rows.iter().map(|r| {
                        let mut cells = vec![r.label.clone()];
                        cells.extend(r.more.iter().map(|x| x.to_string()));
                        cells.push(studio_mograph::format_number(r.value, 2, false).trim_end_matches('0').trim_end_matches('.').to_owned());
                        cells.join(", ")
                    }).collect::<Vec<_>>().join("\n");
                    let mut s: String = ui.data(|d| d.get_temp(key)).unwrap_or(text);
                    let r = ui.add(egui::TextEdit::multiline(&mut s).desired_rows(4).desired_width(200.0).font(theme::body()));
                    if r.changed() {
                        ui.data_mut(|d| d.insert_temp(key, s.clone()));
                    }
                    if r.lost_focus() {
                        ui.data_mut(|d| d.remove::<String>(key));
                        if !studio_mograph::data::rows(&json!(s)).is_empty() {
                            *changed = Some((c.id.to_owned(), json!(s)));
                        }
                    }
                }
            });
        }
    }
    drawn
}

/// Items a shape layer can add from the Inspector.
const ADDABLE: &[(&str, &str)] = &[
    ("Rectangle", r#"{"type": "rect", "size": [300, 200], "roundness": 0}"#),
    ("Ellipse", r#"{"type": "ellipse", "size": [240, 240]}"#),
    ("Star", r#"{"type": "star", "points": 5, "radius": 150, "inner": 65}"#),
    ("Polygon", r#"{"type": "polygon", "points": 6, "radius": 150}"#),
    ("Fill", r##"{"type": "fill", "colour": "#7c5cff"}"##),
    ("Stroke", r##"{"type": "stroke", "colour": "#ffffff", "width": 8}"##),
    ("Trim paths", r#"{"type": "trim", "start": 0, "end": 1, "offset": 0}"#),
    ("Repeater", r#"{"type": "repeater", "copies": 4, "position": [120, 0], "rotation": 0}"#),
    ("Merge paths", r#"{"type": "merge", "mode": "union"}"#),
    ("Round corners", r#"{"type": "round_corners", "radius": 20}"#),
    ("Wiggle paths", r#"{"type": "wiggle", "size": 10, "detail": 3, "per_second": 2}"#),
    ("Zig zag", r#"{"type": "zigzag", "size": 10, "ridges": 12}"#),
    ("Offset paths", r#"{"type": "offset", "amount": 10}"#),
    ("Pucker & bloat", r#"{"type": "pucker", "amount": 0.3}"#),
    ("Twist", r#"{"type": "twist", "angle": 90}"#),
];

fn shape_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, g: &m::Generator) {
    let Some(m::Value::Text(src)) = g.params.get("vector") else { return };
    let Ok(doc) = serde_json::from_str::<Value>(src) else { return };
    let items: Vec<Value> = doc.get("items").and_then(Value::as_array).cloned().or_else(|| doc.as_array().cloned()).unwrap_or_default();
    let ctx = ui.ctx().clone();
    let id = mc.id.0 as u32;
    let mut patch: Vec<Value> = Vec::new();
    let mut add: Option<Value> = None;
    Section::new("mograph-shape", "Shape layer").icon(Icon::Shapes).show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        ui.label(egui::RichText::new("Items run in order: shapes, changes, paint; edit animated values with the AI or as JSON").font(theme::caption()).color(tk.text_2));
        for (k, it) in items.iter().enumerate() {
            let Some(o) = it.as_object() else { continue };
            let kind = o.get("type").and_then(Value::as_str).unwrap_or("?");
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{}. {}", k + 1, kind.replace('_', " "))).font(theme::body_medium()).color(tk.text));
                if IconButton::new(Icon::X, "Remove this item").show(ui).clicked() {
                    patch.push(json!({"path": format!("/items/{k}"), "remove": true}));
                }
            });
            for (field, v) in o {
                if field == "type" {
                    continue;
                }
                let path = format!("/items/{k}/{field}");
                match v {
                    Value::Number(n) => {
                        let mut x = n.as_f64().unwrap_or(0.0) as f32;
                        kit::labeled_row(ui, field, |ui| {
                            if NumberField::new(field, &mut x).decimals(2).width(90.0).show(ui).changed() {
                                patch.push(json!({"path": path, "value": x}));
                            }
                        });
                    }
                    Value::String(s) if matches!(field.as_str(), "colour" | "color") && s.starts_with('#') => {
                        let mut col = colour_of(s);
                        kit::labeled_row(ui, field, |ui| {
                            if kit::colour_well(ui, &format!("mograph-shape-{id}-{k}"), &mut col, None).changed() {
                                patch.push(json!({"path": path, "value": hex_of(col)}));
                            }
                        });
                    }
                    Value::Array(a) if a.len() == 2 && a.iter().all(Value::is_number) => {
                        let mut p = [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32];
                        kit::labeled_row(ui, field, |ui| {
                            let x = NumberField::new("x", &mut p[0]).width(70.0).show(ui).changed();
                            let y = NumberField::new("y", &mut p[1]).width(70.0).show(ui).changed();
                            if x || y {
                                patch.push(json!({"path": path, "value": [p[0], p[1]]}));
                            }
                        });
                    }
                    Value::String(s) if !matches!(field.as_str(), "colour" | "color") => {
                        kit::labeled_row(ui, field, |ui| ui.label(egui::RichText::new(s.chars().take(40).collect::<String>()).font(theme::caption()).color(tk.text_2)));
                    }
                    Value::Object(_) | Value::String(_) => {
                        kit::labeled_row(ui, field, |ui| ui.label(egui::RichText::new("animated").font(theme::caption()).color(tk.accent)));
                    }
                    _ => {}
                }
            }
        }
        let menu = egui::Id::new(("mograph-add-item", id));
        let r = Button::secondary("Add item").icon(Icon::Plus).compact().show(ui);
        if r.clicked() {
            kit::toggle_open(ui.ctx(), menu);
        }
        kit::popover(ui.ctx(), menu, r.rect, kit::Side::Below, 200.0, |ui| {
            for (label, item) in ADDABLE {
                if MenuItem::new(label).show(ui).clicked() {
                    add = serde_json::from_str(item).ok();
                    kit::set_open(ui.ctx(), menu, false);
                }
            }
        });
    });
    if !patch.is_empty() || add.is_some() {
        let mut op = json!({"op": "shape", "clip": id});
        if !patch.is_empty() {
            op["patch"] = Value::Array(patch);
        }
        if let Some(a) = add {
            op["add"] = json!([a]);
        }
        if let Err(e) = run(app, "Shape layer", vec![op]) {
            kit::push(&ctx, e, None, Some(Icon::Warning));
        }
    }
}

fn text_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, g: &m::Generator) {
    let ctx = ui.ctx().clone();
    let id = mc.id.0 as u32;
    let motion: studio_text::TextMotion = match g.params.get("motion") {
        Some(m::Value::Text(t)) => serde_json::from_str(t).unwrap_or_default(),
        _ => Default::default(),
    };
    let mut op: Option<Value> = None;
    Section::new("mograph-text", "Text animation").icon(Icon::Sparkle).show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        ui.label(egui::RichText::new(format!("{} animator(s). Add one: it sweeps or wobbles the letters.", motion.animators.len())).font(theme::caption()).color(tk.text_2));
        ui.horizontal_wrapped(|ui| {
            for name in studio_text::animator::PRESETS {
                if Button::ghost(&name.replace('_', " ")).compact().show(ui).clicked() {
                    op = Some(json!({"op": "set", "clip": id, "animator": {"preset": name, "start": 0.2, "duration": 1.2}}));
                }
            }
        });
        if !motion.animators.is_empty() && Button::ghost("Remove animators").icon(Icon::X).compact().show(ui).clicked() {
            op = Some(json!({"op": "set", "clip": id, "animators": []}));
        }
        kit::labeled_row(ui, "On a path", |ui| {
            ui.horizontal_wrapped(|ui| {
                let on = g.params.contains_key("text_path");
                for (label, path) in [("None", Value::Null), ("Arc", json!({"arc": 420, "degrees": 140})), ("Circle", json!({"circle": 260})), ("Wave", json!({"wave": 60, "width": 1100, "waves": 1.5}))] {
                    let current = path.is_null() && !on;
                    let b = if current { Button::secondary(label) } else { Button::ghost(label) };
                    if b.compact().show(ui).clicked() {
                        op = Some(json!({"op": "set", "clip": id, "path": path}));
                    }
                }
            });
        });
        let mut kern = !matches!(g.params.get("kerning"), Some(m::Value::Bool(false)));
        kit::labeled_row(ui, "Kerning", |ui| {
            if kit::toggle(ui, &mut kern, "Kerning", None).changed() {
                op = Some(json!({"op": "set", "clip": id, "kerning": kern}));
            }
        });
    });
    if let Some(o) = op
        && let Err(e) = run(app, "Text animation", vec![o])
    {
        kit::push(&ctx, e, None, Some(Icon::Warning));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_says_what_was_added_not_the_operation() {
        assert_eq!(added("Handle Slide"), "Added Handle Slide");
        let note = "template clip 47 (Handle Slide, 4s): handle=Name; fill in again with {\"op\": \"template\"}";
        assert!(!added("Handle Slide").contains(note) && !added("Handle Slide").contains("clip 47"));
    }

    /// Every control of every template (a lower third's name and role, a
    /// chart's data…) has a row in the Inspector, so a template the AI can
    /// fill in can also be filled in by hand.
    #[test]
    fn every_template_control_has_an_inspector_row() {
        let ctx = egui::Context::default();
        crate::theme::apply_style(&ctx);
        let mut checked = 0;
        for t in studio_mograph::all() {
            let mut drawn = Vec::new();
            let mut height = 0.0;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let before = ui.min_rect().height();
                drawn = template_controls(ui, 1, t, &serde_json::Map::new(), &mut None);
                height = ui.min_rect().height() - before;
            });
            out.textures_delta.clear();
            let want: Vec<&str> = t.controls.iter().map(|c| c.id).collect();
            assert_eq!(drawn, want, "template {}: controls without a row", t.id);
            assert!(height >= want.len() as f32 * 20.0 || want.is_empty(), "template {}: its controls take no room ({height})", t.id);
            checked += want.len();
        }
        assert!(checked > 50, "only {checked} controls found");
    }
}
