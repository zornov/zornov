// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "World settings" section: what the AI's `build_world` and
//! `edit_world` put into a world, as controls. Sky and time of day, fog,
//! water and terrain; and every item of the lists (scatter, buildings,
//! roads, props, characters, lights, sculpt and paint strokes) with its
//! own settings, a remove button and an add button.
//!
//! The world is a spec (`studio_world::build::WorldSpec`, a `.sworld`
//! file). Edits are made on a copy; when the pointer is up the spec is
//! saved, the world rebuilt and the layer pointed at it (one undo step).

use super::params::{json_value, set_json_choices};
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, IconButton, Section};
use crate::theme;
use eframe::egui;
use serde_json::Value;
use studio_model as m;
use studio_world::build::WorldSpec;

/// The lists of a world, the field each is in, its name and a new item.
pub fn lists() -> Vec<(&'static str, &'static str, Value)> {
    use studio_world::build::*;
    fn j<T: serde::Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap_or_default()
    }
    vec![
        ("scatter", "Scatter", j(&ScatterJ::default())),
        ("buildings", "Buildings", j(&BuildingJ::default())),
        ("roads", "Roads", j(&RoadJ::default())),
        ("props", "Props", j(&PropJ::default())),
        ("characters", "Characters", j(&CharJ::default())),
        ("lights", "Lights", j(&LightJ::default())),
        ("sculpt", "Sculpt strokes", j(&SculptJ::default())),
        ("paint", "Paint strokes", j(&PaintJ::default())),
    ]
}

/// The single settings of a world (not lists), by field.
pub const SINGLES: &[(&str, &str)] = &[("sky", "Sky"), ("fog", "Fog"), ("water", "Water"), ("terrain", "Terrain"), ("look", "Look"), ("quality", "Quality"), ("spawn", "Start point")];

/// Choices of string settings, by their path in the spec.
pub fn choices(path: &str) -> Option<&'static [&'static str]> {
    let key = path.rsplit('/').next()?;
    let list = path.split('/').find(|p| matches!(*p, "scatter" | "buildings" | "props" | "lights" | "characters" | "paint" | "sculpt" | "water" | "terrain"))?;
    Some(match (list, key) {
        ("scatter", "kind") => &["pine", "tree", "palm", "bush", "rock", "boulder", "grass", "flower", "cactus", "dead_tree"],
        ("scatter", "on") | ("paint", "layer") => &["grass", "rock", "dirt", "snow", "sand"],
        ("buildings", "kind") => &["house", "apartment", "tower", "shop", "warehouse", "room", "hall"],
        ("buildings", "style") => &["classical", "art_deco", "brutalist", "modern_glass", "medieval", "oriental", "futuristic", "rustic"],
        ("props", "kind") => &["crate", "barrel", "table", "chair", "column", "stairs", "lamp", "fence", "car", "sofa", "bed", "shelf", "desk", "cactus"],
        ("lights", "kind") => &["point", "spot"],
        ("characters", "clip") => &["idle", "walk", "run", "wave", "talk", "dance"],
        ("sculpt", "brush") => &["raise", "lower", "smooth", "flatten"],
        ("water", "kind") => &["none", "sea", "lake", "river"],
        ("terrain", "style") => &["flat", "plains", "hills", "mountains", "dunes", "canyon", "island"],
        _ => return None,
    })
}

/// Edits in progress for one world file.
#[derive(Clone)]
struct Pending {
    file: String,
    spec: Value,
    dirty: bool,
}

fn item_title(list: &str, item: &Value, i: usize) -> String {
    let name = ["kind", "name", "brush", "layer"].iter().find_map(|k| item.get(k).and_then(Value::as_str)).unwrap_or("");
    let n = item.get("count").and_then(Value::as_u64).map(|c| format!(" x {c}")).unwrap_or_default();
    format!("{} {}{}", i + 1, if name.is_empty() { list } else { name }, n)
}

pub fn section(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let Some(file) = super::viewport::world::world_file_of(&mc) else { return };
    let ctx = ui.ctx().clone();
    let key = egui::Id::new(("world-edit", id));
    let mut pending: Option<Pending> = ctx.data(|d| d.get_temp(key)).filter(|p: &Pending| p.file == file);
    if pending.is_none() {
        let spec = super::viewport::world::read_spec(&file).and_then(|s| studio_world::build::materialize(&s)).and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()));
        match spec {
            Ok(spec) => pending = Some(Pending { file: file.clone(), spec, dirty: false }),
            Err(_) => return,
        }
    }
    let Some(mut p) = pending else { return };
    let mut changed = false;
    Section::new("world-spec", "World settings").icon(Icon::Sun).start_closed().show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        set_json_choices(ui, Some(choices));
        ui.label(egui::RichText::new("Everything in this world. Changes rebuild it when you let go.").font(theme::caption()).color(tk.text_2));
        // Time of day is the layer's, so it can be animated.
        super::three_d::row(app, ui, &[id], "generator.time_of_day", "Time of day", 0.0..=24.0, 12.0, "h", 1, super::three_d::same, super::three_d::same);
        for (field, title) in SINGLES {
            let Some(v) = p.spec.get_mut(*field) else { continue };
            ui.label(egui::RichText::new(*title).font(theme::section_title()).color(tk.text));
            if v.is_null() {
                if Button::ghost("Set").compact().show(ui).clicked() {
                    *v = match *field {
                        "sky" => serde_json::to_value(studio_world::build::SkyJ::default()).unwrap_or_default(),
                        "water" => serde_json::to_value(studio_world::build::WaterJ { kind: "sea".into(), ..Default::default() }).unwrap_or_default(),
                        "terrain" => serde_json::to_value(studio_world::build::TerrainJ { style: "hills".into(), ..Default::default() }).unwrap_or_default(),
                        "look" => serde_json::to_value(studio_world::build::LookJ::default()).unwrap_or_default(),
                        "spawn" => serde_json::json!([0.0, 2.0, 0.0]),
                        _ => serde_json::json!(0.002),
                    };
                    changed = true;
                }
                continue;
            }
            changed |= json_value(ui, &format!("world/{field}"), v, 0);
        }
        for (field, title, fresh) in lists() {
            let items = p.spec.get_mut(field).and_then(Value::as_array_mut);
            let Some(items) = items else { continue };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{title} ({})", items.len())).font(theme::section_title()).color(tk.text));
                if IconButton::new(Icon::Plus, "Add one").compact().show(ui).clicked() {
                    items.push(fresh.clone());
                    changed = true;
                }
            });
            let mut remove = None;
            for (i, item) in items.iter_mut().enumerate() {
                let head = item_title(field, item, i);
                egui::CollapsingHeader::new(egui::RichText::new(head).font(theme::body()).color(tk.text_2)).id_salt(("world-item", id, field, i)).default_open(false).show(ui, |ui| {
                    changed |= json_value(ui, &format!("world/{field}/{i}"), item, 0);
                    if Button::ghost("Remove").icon(Icon::Trash).compact().show(ui).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                items.remove(i);
                changed = true;
            }
        }
        set_json_choices(ui, None);
    });
    if changed {
        p.dirty = true;
    }
    // Rebuild once the pointer is up.
    if p.dirty && !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
        match serde_json::from_value::<WorldSpec>(p.spec.clone()) {
            Ok(spec) => {
                let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
                let mut s = crate::agent::Session::new(doc);
                let r = s.replace_world(id, &spec);
                app.doc = s.doc;
                match r {
                    Ok(path) => p.file = path.lines().next().unwrap_or("").to_owned(),
                    Err(e) => {
                        kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
            Err(e) => {
                kit::push(&ctx, format!("World: {e}"), None, Some(Icon::Warning));
            }
        }
        p.dirty = false;
    }
    ctx.data_mut(|d| d.insert_temp(key, p));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every list a world spec has is editable here, and every single
    /// setting; a new field of the spec fails this until it has a place.
    #[test]
    fn every_world_setting_has_a_control() {
        let spec = serde_json::to_value(WorldSpec::default()).unwrap();
        let obj = spec.as_object().unwrap();
        let listed: Vec<&str> = lists().iter().map(|l| l.0).collect();
        let single: Vec<&str> = SINGLES.iter().map(|s| s.0).collect();
        // Fields that are the world's identity, not its content.
        let identity = ["name", "description", "preset", "seed", "size", "without"];
        for (k, v) in obj {
            if identity.contains(&k.as_str()) {
                continue;
            }
            assert!(listed.contains(&k.as_str()) || single.contains(&k.as_str()), "world setting \"{k}\" has no control in editor/world_edit.rs");
            if v.is_array() {
                assert!(listed.contains(&k.as_str()), "list \"{k}\" is not editable");
            }
        }
        for l in &listed {
            assert!(obj.get(*l).is_some_and(Value::is_array), "{l} is not a list of the spec");
        }
        // A new item of every list reads back into the spec.
        for (field, _, fresh) in lists() {
            let mut s = spec.clone();
            s[field] = serde_json::json!([fresh]);
            assert!(serde_json::from_value::<WorldSpec>(s).is_ok(), "a new {field} item is not valid");
        }
        assert!(super::super::inspector::all_tab_kinds().contains(&"world-spec"));
    }

    /// The choices offered are values the builder accepts.
    #[test]
    fn choices_are_known_to_the_builder() {
        assert!(choices("world/scatter/0/kind").unwrap().contains(&"pine"));
        assert!(choices("world/lights/0/kind").unwrap().contains(&"spot"));
        assert!(choices("world/sky/clouds").is_none());
    }
}
