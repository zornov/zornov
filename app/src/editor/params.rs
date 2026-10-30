// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Manual controls for what the AI makes and changes, so nothing it can set
//! is out of a person's reach.
//!
//! * "Clip": name, on/off and label colour of any clip.
//! * "Crop & flip" for pictures.
//! * "Fill & shape" for solid, gradient and shape layers (cards).
//! * "All settings": every parameter of the selected generated layer, with a
//!   control by type (number, angle, switch, colour, point, choice, text),
//!   and the style of each text run. This is the fallback that keeps a
//!   generator kind from ever being uneditable.
//! * "Data": the clip's free-form `meta` (what the AI's tools remember on a
//!   clip) as typed controls, for the keys no dedicated section owns.
//!
//! [`GENERATOR_SECTIONS`] and [`META_OWNERS`] list who edits what; the tests
//! at the bottom fail when a generator kind or an op-written `meta` key has no
//! entry, or an entry names a section no Inspector tab shows.

use super::three_d::{self, same};
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, NumberField, Section};
use crate::theme;
use eframe::egui::{self, Color32};
use serde_json::Value;
use std::ops::RangeInclusive;
use studio_model as m;

fn mclip(app: &StudioApp, id: u32) -> Option<m::Clip> {
    app.doc.model.active().clip(m::ClipId(id as u64)).cloned()
}

/// Applies `f` to the model clip as one undo step per gesture (dragging a
/// number or a colour is one step, not hundreds).
fn put(app: &mut StudioApp, ids: &[u32], key: &str, label: &str, f: impl Fn(&mut m::Clip)) {
    let seq = app.doc.seq();
    let edits: Vec<m::Edit> = ids
        .iter()
        .filter_map(|id| mclip(app, *id))
        .filter_map(|old| {
            let mut mc = old.clone();
            f(&mut mc);
            (mc != old).then(|| m::Edit::SetClip { seq, clip: Box::new(mc) })
        })
        .collect();
    if !edits.is_empty() {
        app.doc.apply_gesture(&format!("manual-{key}-{ids:?}"), label, m::Edit::Batch { edits });
    }
}

/// "stroke_width" as "Stroke width".
pub fn humanise(key: &str) -> String {
    let s = key.replace(['_', '.'], " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn generator(mc: &m::Clip) -> Option<&m::Generator> {
    match &mc.source {
        m::ClipSource::Generator(g) => Some(g),
        _ => None,
    }
}

/// The section kinds this module draws (the tabs must list them).
pub const SECTIONS: &[&str] = &["clip", "crop", "shape", "generator", "data"];

/// Every generator kind and the Inspector sections that edit its settings.
/// A kind missing here fails the test below.
pub fn generator_sections(kind: m::GeneratorKind) -> &'static [&'static str] {
    use m::GeneratorKind::*;
    match kind {
        Solid | Gradient => &["shape", "generator"],
        Shape => &["shape", "mograph-shape", "generator"],
        Text => &["text", "mograph-text", "generator"],
        Adjustment => &["colour", "effects", "generator"],
        Camera => &["camera", "render3d", "generator"],
        Light => &["light", "sky3d", "generator"],
        Model => &["model", "character3d", "sim3d", "generator"],
    }
}

/// All generator kinds (the test checks this against the enum).
pub const ALL_KINDS: [m::GeneratorKind; 8] = [
    m::GeneratorKind::Solid,
    m::GeneratorKind::Gradient,
    m::GeneratorKind::Text,
    m::GeneratorKind::Shape,
    m::GeneratorKind::Adjustment,
    m::GeneratorKind::Camera,
    m::GeneratorKind::Light,
    m::GeneratorKind::Model,
];

/// Clip `meta` keys the tools write, and the Inspector section that edits
/// each. `data` is the typed fallback (every key shows there unless a
/// dedicated section owns it). A key written without an entry fails the test.
pub const META_OWNERS: &[(&str, &str)] = &[
    ("template", "mograph-template"),
    ("shadow", "in-scene"),
    ("light_match", "in-scene"),
    ("foreground", "in-scene"),
    ("part", "in-scene"),
    ("tracks", "tracking"),
    ("stabilize", "tracking"),
    ("follow", "tracking"),
    ("matchmove", "tracking"),
    ("slowmo", "slow-motion"),
    ("matte", "transform"),
    ("generated", "data"),
    ("multicam", "data"),
    ("swap", "skeleton"),
    ("rig", "skeleton"),
    ("dub", "data"),
    ("adr", "data"),
    ("edl_event", "data"),
    ("id", "data"),
    ("controls", "data"),
    ("sim", "sim-settings"),
];

fn owned_elsewhere(key: &str) -> bool {
    META_OWNERS.iter().any(|(k, s)| *k == key && *s != "data")
}

/// Name, on/off and label (any clip), then the fill or shape of a solid,
/// gradient or shape layer.
/// The label column of a `FieldRow` at this width: panels under 260 px give the label less room.
// TODO(kit): `kit::labeled_row` should take its column from the same place as `FieldRow`.
pub(crate) fn label_col(ui: &egui::Ui) -> f32 {
    if ui.available_width() < 260.0 { 80.0 } else { kit::label_width(ui.ctx()) }
}

/// A row with a label and arbitrary content, on the same label column as `FieldRow`.
// TODO(kit): replace with `kit::labeled_row` once it uses the narrow label column.
pub(crate) fn labeled_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = theme::tokens(ui.ctx());
    let lw = label_col(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.set_min_height(theme::space::CONTROL_H);
        let (r, resp) = ui.allocate_exact_size(egui::Vec2::new(lw, theme::space::CONTROL_H), egui::Sense::hover());
        let cut = kit::paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(r, 0.0), r.center().y), crate::i18n::start_center(), label, theme::body(), t.text_2, lw - 8.0);
        kit::tip_if_cut(resp, cut, label);
        ui.spacing_mut().item_spacing.x = 6.0;
        add(ui)
    })
    .inner
}

pub fn top(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    clip_section(app, ui, ids);
    if let [id] = ids
        && let Some(mc) = mclip(app, *id)
        && let Some(g) = generator(&mc)
        && matches!(g.kind, m::GeneratorKind::Solid | m::GeneratorKind::Gradient | m::GeneratorKind::Shape)
    {
        shape_section(app, ui, ids, &mc, g);
    }
}

/// Crop and flip of a picture clip.
pub fn crop(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    if let [id] = ids
        && let Some(mc) = mclip(app, *id)
        && generator(&mc).is_none_or(|g| !matches!(g.kind, m::GeneratorKind::Camera | m::GeneratorKind::Light | m::GeneratorKind::Adjustment))
    {
        crop_section(app, ui, ids, &mc);
    }
}

/// The fallbacks, last: every setting of the layer and the clip's data.
pub fn rest(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    let [id] = ids else { return };
    let Some(mc) = mclip(app, *id) else { return };
    if let Some(g) = generator(&mc) {
        settings_section(app, ui, ids, &mc, g);
    }
    data_section(app, ui, ids, &mc);
}

// --- Clip --------------------------------------------------------------

fn clip_section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    let Some(mc) = ids.first().and_then(|id| mclip(app, *id)) else { return };
    let out = Section::new("clip", "Clip").icon(Icon::Faders).resettable(mc.label.is_some() || !mc.enabled).show(ui, |ui| {
        if ids.len() == 1 {
            let key = egui::Id::new(("clip-name", mc.id.0));
            let mut name: String = ui.data(|d| d.get_temp(key)).unwrap_or_else(|| mc.name.clone());
            // Ends where the value fields and menus end: the reset and keyframe slots stay free.
            let w = ui.available_width() - label_col(ui) - 6.0 - 40.0;
            labeled_row(ui, "Name", |ui| {
                let r = kit::text_field(ui, &mut name, "Clip name", None, w, None);
                if r.changed() {
                    ui.data_mut(|d| d.insert_temp(key, name.clone()));
                }
                if r.lost_focus() {
                    ui.data_mut(|d| d.remove::<String>(key));
                    let n = name.trim().to_owned();
                    put(app, ids, "name", "Rename clip", |c| c.name = n.clone());
                }
            });
        }
        let mut on = mc.enabled;
        labeled_row(ui, "Enabled", |ui| {
            if kit::toggle(ui, &mut on, "Enabled", None).changed() {
                put(app, ids, "enabled", if on { "Enable clip" } else { "Disable clip" }, |c| c.enabled = on);
            }
        });
        labeled_row(ui, "Label", |ui| {
            let mut col = mc.label.as_ref().map_or(Color32::TRANSPARENT, crate::bridge::color32);
            let had = mc.label.is_some();
            if kit::colour_well(ui, "clip-label", &mut col, None).changed() {
                let r = crate::bridge::rgba(col);
                put(app, ids, "label", "Label colour", |c| c.label = Some(r));
            }
            if had && Button::ghost("Clear").compact().show(ui).clicked() {
                put(app, ids, "label-clear", "Clear label", |c| c.label = None);
            }
        });
    });
    if out.reset {
        put(app, ids, "clip-reset", "Reset clip", |c| {
            c.label = None;
            c.enabled = true;
        });
    }
}

// --- Crop & flip -------------------------------------------------------

fn crop_section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip) {
    let t = mc.transform_at(0);
    let (fx, fy) = (t.flip_x, t.flip_y);
    let cropped = t.crop != m::Crop::default() || fx || fy || mc.keyframes.keys().any(|k| k.starts_with("transform.crop"));
    let out = Section::new("crop", "Crop & flip").icon(Icon::Selection).resettable(cropped).start_closed().show(ui, |ui| {
        for (path, label) in [("transform.crop_left", "Left"), ("transform.crop_right", "Right"), ("transform.crop_top", "Top"), ("transform.crop_bottom", "Bottom")] {
            three_d::row(app, ui, ids, path, label, 0.0..=100.0, 0.0, "%", 1, three_d::pct, three_d::from_pct);
        }
        let (mut a, mut b) = (fx, fy);
        labeled_row(ui, "Flip", |ui| {
            let x = kit::toggle(ui, &mut a, "Flip horizontally", None).changed();
            ui.label(egui::RichText::new("Horizontal").font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
            let y = kit::toggle(ui, &mut b, "Flip vertically", None).changed();
            ui.label(egui::RichText::new("Vertical").font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
            if x || y {
                put(app, ids, "flip", "Flip", |c| {
                    c.transform.flip_x = a;
                    c.transform.flip_y = b;
                });
            }
        });
    });
    if out.reset {
        put(app, ids, "crop-reset", "Reset crop", |c| {
            c.transform.crop = m::Crop::default();
            c.transform.flip_x = false;
            c.transform.flip_y = false;
            c.keyframes.retain(|k, _| !k.starts_with("transform.crop") && !k.starts_with("transform.flip"));
        });
    }
}

// --- Fill & shape ------------------------------------------------------

fn colour_of(g: &m::Generator, key: &str, default: Color32) -> Color32 {
    match g.params.get(key) {
        Some(m::Value::Colour(c)) => crate::bridge::color32(c),
        _ => default,
    }
}

fn colour_row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], g: &m::Generator, key: &'static str, label: &str, default: Color32) {
    let mut col = colour_of(g, key, default);
    labeled_row(ui, label, |ui| {
        let r = kit::colour_well(ui, &format!("shape-{key}"), &mut col, None);
        ui.label(egui::RichText::new(kit::to_hex(col)).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
        if r.changed() {
            let v = m::Value::Colour(crate::bridge::rgba(col));
            put(app, ids, key, label, |c| three_d::set_param(c, key, v.clone()));
        }
    });
}

fn choice_row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], g: &m::Generator, key: &'static str, label: &str, options: &[&str], default: &str) {
    let cur = match g.params.get(key) {
        Some(m::Value::Choice(s) | m::Value::Text(s)) => s.as_str(),
        _ => default,
    };
    let at = options.iter().position(|o| *o == cur).unwrap_or(0);
    // The stored words are ids (`as_typed`): show them as words.
    let shown: Vec<String> = options.iter().map(|o| humanise(o)).collect();
    let shown: Vec<&str> = shown.iter().map(String::as_str).collect();
    labeled_row(ui, label, |ui| {
        if let Some(i) = kit::menu_button(ui, key, at, &shown, ui.available_width() - 40.0, None) {
            let v = m::Value::Choice(options[i].to_owned());
            put(app, ids, key, label, |c| three_d::set_param(c, key, v.clone()));
        }
    });
}

const STOPS: [&str; 16] = ["stop0", "stop1", "stop2", "stop3", "stop4", "stop5", "stop6", "stop7", "stop8", "stop9", "stop10", "stop11", "stop12", "stop13", "stop14", "stop15"];

fn num_param(g: &m::Generator, key: &str, default: f64) -> f64 {
    g.params.get(key).and_then(m::Value::as_f64).unwrap_or(default)
}

fn shape_section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip, g: &m::Generator) {
    // Vector-art shape layers (shape items) have their own section.
    if g.params.contains_key("vector") {
        return;
    }
    let title = if g.kind == m::GeneratorKind::Shape { "Shape" } else { "Fill" };
    Section::new("shape", title).icon(Icon::Rectangle).show(ui, |ui| match g.kind {
        m::GeneratorKind::Solid => {
            colour_row(app, ui, ids, g, "colour", "Colour", Color32::WHITE);
        }
        m::GeneratorKind::Gradient => {
            colour_row(app, ui, ids, g, "start", "From", Color32::BLACK);
            colour_row(app, ui, ids, g, "end", "To", Color32::WHITE);
            let stops: Vec<usize> = (0..16).filter(|i| g.params.contains_key(&format!("stop{i}"))).collect();
            for i in stops {
                colour_row(app, ui, ids, g, STOPS[i], &format!("Stop {}", i + 1), Color32::WHITE);
            }
            three_d::row(app, ui, ids, "generator.angle", "Angle", -360.0..=360.0, 90.0, "°", 0, same, same);
            let radial = matches!(g.params.get("radial"), Some(m::Value::Bool(true)));
            let mut r = radial;
            labeled_row(ui, "Radial", |ui| {
                if kit::toggle(ui, &mut r, "Radial", None).changed() {
                    put(app, ids, "radial", "Radial gradient", |c| three_d::set_param(c, "radial", m::Value::Bool(r)));
                }
            });
            if radial {
                three_d::row(app, ui, ids, "generator.radius", "Radius", 0.0..=3.0, 0.75, "", 2, same, same);
            }
            choice_row(app, ui, ids, g, "ease", "Curve", &["smooth", "linear"], "smooth");
        }
        _ => {
            choice_row(app, ui, ids, g, crate::bridge::SHAPE_KEY, "Shape", &["rectangle", "ellipse", "polygon", "star", "arrow", "line"], "rectangle");
            if matches!(g.params.get(crate::bridge::SHAPE_KEY), Some(m::Value::Choice(s)) if s == "polygon" || s == "star") {
                three_d::row(app, ui, ids, "generator.sides", "Sides", 3.0..=24.0, 5.0, "", 0, same, same);
            }
            colour_row(app, ui, ids, g, "fill", "Fill", Color32::WHITE);
            let size = g.params.get("size").and_then(m::Value::as_point).unwrap_or(m::Vec2 { x: 1.0, y: 1.0 });
            let (mut w, mut h) = ((size.x * 100.0) as f32, (size.y * 100.0) as f32);
            labeled_row(ui, "Size", |ui| {
                let a = NumberField::new("Width", &mut w).range(0.0..=400.0).decimals(0).unit("%").width(72.0).show(ui).changed();
                let b = NumberField::new("Height", &mut h).range(0.0..=400.0).decimals(0).unit("%").width(72.0).show(ui).changed();
                if a || b {
                    let v = m::Value::Point(m::Vec2 { x: w as f64 / 100.0, y: h as f64 / 100.0 });
                    put(app, ids, "size", "Shape size", |c| three_d::set_param(c, "size", v.clone()));
                }
            });
            three_d::row(app, ui, ids, "generator.roundness", "Roundness", 0.0..=100.0, 0.0, "%", 0, three_d::pct, three_d::from_pct);
            let has_stroke = g.params.contains_key("stroke");
            let mut on = has_stroke;
            labeled_row(ui, "Outline", |ui| {
                if kit::toggle(ui, &mut on, "Outline", None).changed() {
                    put(app, ids, "stroke-on", "Outline", |c| {
                        if on {
                            three_d::set_param(c, "stroke", m::Value::Colour(m::Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }));
                            three_d::set_param(c, "stroke_width", m::Value::Scalar(0.004));
                        } else if let m::ClipSource::Generator(g) = &mut c.source {
                            g.params.remove("stroke");
                            g.params.remove("stroke_width");
                        }
                    });
                }
            });
            if has_stroke {
                colour_row(app, ui, ids, g, "stroke", "Outline colour", Color32::WHITE);
                // Stored as a fraction of the smaller canvas side; shown in tenths of a percent.
                three_d::row(app, ui, ids, "generator.stroke_width", "Outline width", 0.0..=20.0, 0.4, "%", 1, three_d::pct, three_d::from_pct);
            }
            let _ = num_param(g, "sides", 5.0);
        }
    });
    let _ = mc;
}

// --- Text font and effects ---------------------------------------------

/// Font family, weight, italic, case, line spacing, outline and shadow of a
/// text layer, in the Inspector's Text section. They are generator settings
/// (`font`, `weight`, `outline`, `shadow`…) the renderer applies over the
/// style preset, so they show up in "All settings" too.
pub fn text_style_rows(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    let Some(mc) = ids.first().and_then(|id| mclip(app, *id)) else { return };
    let Some(g) = generator(&mc).filter(|g| g.kind == m::GeneratorKind::Text) else { return };
    let tk = theme::tokens(ui.ctx());
    let has = |k: &str| g.params.contains_key(k);
    // Font family: the library's families, "Style default" first.
    let families = crate::text_look::engine().fonts.families_shared();
    let mut names: Vec<&str> = vec!["Style default"];
    names.extend(families.iter().map(String::as_str));
    let cur = match g.params.get("font") {
        Some(m::Value::Choice(f) | m::Value::Text(f)) => families.iter().position(|x| x == f).map_or(0, |i| i + 1),
        _ => 0,
    };
    labeled_row(ui, "Font", |ui| {
        if let Some(i) = kit::menu_button(ui, "text-font", cur, &names, ui.available_width() - 40.0, None) {
            let pick = (i > 0).then(|| families[i - 1].clone());
            put(app, ids, "font", "Font", |c| match &pick {
                Some(f) => three_d::set_param(c, "font", m::Value::Choice(f.clone())),
                None => {
                    if let m::ClipSource::Generator(g) = &mut c.source {
                        g.params.remove("font");
                    }
                }
            });
        }
    });
    three_d::row(app, ui, ids, "generator.weight", "Weight", 100.0..=900.0, 700.0, "", 0, same, same);
    let mut italic = matches!(g.params.get("italic"), Some(m::Value::Bool(true)));
    labeled_row(ui, "Italic", |ui| {
        if kit::toggle(ui, &mut italic, "Italic", None).changed() {
            put(app, ids, "italic", "Italic", |c| three_d::set_param(c, "italic", m::Value::Bool(italic)));
        }
    });
    choice_row(app, ui, ids, g, "case", "Case", &["as_typed", "upper", "lower", "title"], "as_typed");
    three_d::row(app, ui, ids, "generator.line_spacing", "Line spacing", -50.0..=200.0, 0.0, "%", 0, same, same);

    // Outline.
    let mut on = has("outline");
    labeled_row(ui, "Outline", |ui| {
        if kit::toggle(ui, &mut on, "Outline", None).changed() {
            put(app, ids, "outline-on", "Outline", |c| {
                if on {
                    three_d::set_param(c, "outline", m::Value::Colour(m::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
                    three_d::set_param(c, "outline_width", m::Value::Scalar(4.0));
                } else if let m::ClipSource::Generator(g) = &mut c.source {
                    g.params.remove("outline");
                    g.params.remove("outline_width");
                }
            });
        }
    });
    if has("outline") {
        colour_row(app, ui, ids, g, "outline", "Outline colour", Color32::BLACK);
        three_d::row(app, ui, ids, "generator.outline_width", "Outline width", 0.0..=40.0, 4.0, "", 1, same, same);
    }
    // Shadow.
    let mut on = has("shadow");
    labeled_row(ui, "Shadow", |ui| {
        if kit::toggle(ui, &mut on, "Shadow", None).changed() {
            put(app, ids, "shadow-on", "Shadow", |c| {
                if on {
                    three_d::set_param(c, "shadow", m::Value::Colour(m::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
                } else if let m::ClipSource::Generator(g) = &mut c.source {
                    for k in ["shadow", "shadow_blur", "shadow_distance", "shadow_angle", "shadow_opacity"] {
                        g.params.remove(k);
                    }
                }
            });
        }
    });
    if has("shadow") {
        colour_row(app, ui, ids, g, "shadow", "Shadow colour", Color32::BLACK);
        three_d::row(app, ui, ids, "generator.shadow_opacity", "Shadow opacity", 0.0..=100.0, 60.0, "%", 0, three_d::pct, three_d::from_pct);
        three_d::row(app, ui, ids, "generator.shadow_blur", "Shadow blur", 0.0..=60.0, 8.0, "", 0, same, same);
        three_d::row(app, ui, ids, "generator.shadow_distance", "Shadow distance", 0.0..=100.0, 6.0, "", 0, same, same);
        three_d::row(app, ui, ids, "generator.shadow_angle", "Shadow angle", -180.0..=180.0, 90.0, "°", 0, same, same);
    }
    let _ = tk;
}

// --- All settings ------------------------------------------------------

/// Choices of settings with a fixed list.
fn choices_of(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "shape" => &["rectangle", "ellipse", "polygon", "star", "arrow", "line"],
        "align" => &["left", "centre", "right"],
        "case" => &["as_typed", "upper", "lower", "title"],
        "ease" => &["smooth", "linear"],
        _ => return None,
    })
}

fn scalar_hint(key: &str, v: f64) -> (RangeInclusive<f32>, f32, usize) {
    let fraction = matches!(key, "opacity" | "roundness" | "metallic" | "roughness" | "emissive" | "smooth" | "stroke_width" | "radius" | "strength" | "amount" | "softness" | "mix");
    if fraction && v.abs() <= 4.0 {
        return (0.0..=(v.abs() as f32 * 4.0).max(1.0), 0.005, 3);
    }
    let big = (v.abs() as f32 * 10.0).max(100.0);
    (-big..=big, (v.abs() as f32 / 200.0).max(0.01), if v.fract() == 0.0 && v.abs() >= 10.0 { 0 } else { 2 })
}

/// One typed row for a `Value`; returns whether it changed.
pub fn value_row(ui: &mut egui::Ui, salt: &str, key: &str, v: &mut m::Value) -> bool {
    let label = humanise(key);
    let mut changed = false;
    let angle = matches!(v, m::Value::Angle(_));
    match v {
        m::Value::Bool(b) => labeled_row(ui, &label, |ui| changed = kit::toggle(ui, b, &label, None).changed()),
        m::Value::Scalar(x) | m::Value::Angle(x) => {
            let (range, speed, dec) = scalar_hint(key, *x);
            let mut f = *x as f32;
            labeled_row(ui, &label, |ui| {
                let r = NumberField::new(&label, &mut f).range(range).decimals(dec).drag_speed(speed).unit(if angle { "°" } else { "" }).width(72.0).show(ui);
                if r.changed() {
                    *x = f as f64;
                    changed = true;
                }
            });
        }
        m::Value::Point(p) => {
            let (mut a, mut b) = (p.x as f32, p.y as f32);
            labeled_row(ui, &label, |ui| {
                let (_, sp, dec) = scalar_hint(key, p.x.abs().max(p.y.abs()));
                let x = NumberField::new("X", &mut a).decimals(dec).drag_speed(sp).width(72.0).show(ui).changed();
                let y = NumberField::new("Y", &mut b).decimals(dec).drag_speed(sp).width(72.0).show(ui).changed();
                if x || y {
                    p.x = a as f64;
                    p.y = b as f64;
                    changed = true;
                }
            });
        }
        m::Value::Colour(c) => {
            let mut col = crate::bridge::color32(c);
            labeled_row(ui, &label, |ui| {
                if kit::colour_well(ui, &format!("{salt}-{key}"), &mut col, None).changed() {
                    *c = crate::bridge::rgba(col);
                    changed = true;
                }
                ui.label(egui::RichText::new(kit::to_hex(col)).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
            });
        }
        m::Value::Choice(s) | m::Value::Text(s) => {
            if let Some(options) = choices_of(key) {
                let at = options.iter().position(|o| *o == s.as_str()).unwrap_or(0);
                labeled_row(ui, &label, |ui| {
                    if let Some(i) = kit::menu_button(ui, &format!("{salt}-{key}"), at, options, ui.available_width() - 40.0, None) {
                        *s = options[i].to_owned();
                        changed = true;
                    }
                });
            } else if let Ok(mut json) = serde_json::from_str::<Value>(s)
                && (json.is_object() || json.is_array())
            {
                // A setting stored as JSON text (a path, a motion): typed controls.
                ui.label(egui::RichText::new(label.clone()).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
                if json_value(ui, &format!("{salt}-{key}"), &mut json, 1) {
                    *s = json.to_string();
                    changed = true;
                }
            } else {
                let tk = egui::Id::new(("param-text", salt, key));
                let mut edit: String = ui.data(|d| d.get_temp(tk)).unwrap_or_else(|| s.clone());
                labeled_row(ui, &label, |ui| {
                    let w = ui.available_width();
                    let r = kit::text_field(ui, &mut edit, &label, None, w, None);
                    if r.changed() {
                        ui.data_mut(|d| d.insert_temp(tk, edit.clone()));
                    }
                    if r.lost_focus() {
                        ui.data_mut(|d| d.remove::<String>(tk));
                        if edit != *s {
                            *s = edit.clone();
                            changed = true;
                        }
                    }
                });
            }
        }
        m::Value::Curve(points) => {
            labeled_row(ui, &label, |ui| {
                ui.label(egui::RichText::new(crate::i18n::tf(format!("Curve, {} points", points.len()))).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
            });
        }
    }
    changed
}

fn settings_section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip, g: &m::Generator) {
    // Cameras and lights show their settings in their own sections; their
    // raw list is still here, collapsed.
    let runs_styled = g.text.len() > 1 || g.text.iter().any(|r| !r.style.is_empty());
    if g.params.is_empty() && !runs_styled {
        return;
    }
    let mut params = g.params.clone();
    let mut runs = g.text.clone();
    let mut changed: Option<String> = None;
    let salt = format!("gen-{}", mc.id.0);
    Section::new("generator", "All settings").icon(Icon::Faders).start_closed().show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        ui.label(egui::RichText::new("Every setting of this layer, including what the AI set that has no control of its own.").font(theme::caption()).color(tk.text_2));
        for (k, v) in params.iter_mut() {
            // A vector document is edited in the Shape layer section.
            if k == "vector" {
                continue;
            }
            if value_row(ui, &salt, k, v) {
                changed = Some(k.clone());
            }
        }
        if runs_styled {
            for (i, run) in runs.iter_mut().enumerate() {
                let head: String = run.text.chars().take(24).collect();
                ui.label(egui::RichText::new(crate::i18n::tf(format!("Text run {}: {head}", i + 1))).font(theme::caption()).color(tk.text_2));
                let tkey = egui::Id::new(("run-text", mc.id.0, i));
                let mut edit: String = ui.data(|d| d.get_temp(tkey)).unwrap_or_else(|| run.text.clone());
                labeled_row(ui, "Text", |ui| {
                    let w = ui.available_width();
                    let r = kit::text_field(ui, &mut edit, "Text of this run", None, w, None);
                    if r.changed() {
                        ui.data_mut(|d| d.insert_temp(tkey, edit.clone()));
                    }
                    if r.lost_focus() {
                        ui.data_mut(|d| d.remove::<String>(tkey));
                        if edit != run.text {
                            run.text = edit.clone();
                            changed = Some("text".into());
                        }
                    }
                });
                for (k, v) in run.style.iter_mut() {
                    if value_row(ui, &format!("{salt}-run{i}"), k, v) {
                        changed = Some(format!("run{i}.{k}"));
                    }
                }
            }
        }
    });
    if let Some(k) = changed {
        put(app, ids, &format!("gen-{k}"), "Layer setting", |c| {
            if let m::ClipSource::Generator(g) = &mut c.source {
                g.params = params.clone();
                g.text = runs.clone();
            }
        });
    }
}

// --- Data (meta) --------------------------------------------------------

fn looks_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Typed controls for a JSON value; returns whether it changed.
pub fn json_value(ui: &mut egui::Ui, salt: &str, v: &mut Value, depth: usize) -> bool {
    let mut changed = false;
    let tk = theme::tokens(ui.ctx());
    match v {
        Value::Object(map) => {
            ui.indent(salt, |ui| {
                for (k, x) in map.iter_mut() {
                    changed |= json_row(ui, &format!("{salt}/{k}"), k, x, depth + 1);
                }
            });
        }
        Value::Array(items) => {
            let numeric_pair = items.len() == 2 && items.iter().all(Value::is_number);
            if numeric_pair {
                let (mut a, mut b) = (items[0].as_f64().unwrap_or(0.0) as f32, items[1].as_f64().unwrap_or(0.0) as f32);
                ui.horizontal(|ui| {
                    let x = NumberField::new("X", &mut a).decimals(2).width(72.0).show(ui).changed();
                    let y = NumberField::new("Y", &mut b).decimals(2).width(72.0).show(ui).changed();
                    if x || y {
                        items[0] = serde_json::json!(a);
                        items[1] = serde_json::json!(b);
                        changed = true;
                    }
                });
            } else {
                let shown = items.len().min(64);
                ui.indent(salt, |ui| {
                    for (i, x) in items.iter_mut().take(shown).enumerate() {
                        changed |= json_row(ui, &format!("{salt}/{i}"), &format!("{}", i + 1), x, depth + 1);
                    }
                    if items.len() > shown {
                        ui.label(egui::RichText::new(crate::i18n::tf(format!("and {} more", items.len() - shown))).font(theme::caption()).color(tk.text_2));
                    }
                });
            }
        }
        _ => {
            changed |= json_leaf(ui, salt, "", v);
        }
    }
    changed
}

fn json_row(ui: &mut egui::Ui, salt: &str, key: &str, v: &mut Value, depth: usize) -> bool {
    let label = humanise(key);
    match v {
        Value::Object(_) | Value::Array(_) if !matches!(v, Value::Array(a) if a.len() == 2 && a.iter().all(Value::is_number)) => {
            ui.label(egui::RichText::new(label).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
            if depth > 5 {
                return false;
            }
            json_value(ui, salt, v, depth)
        }
        Value::Array(_) => {
            let mut changed = false;
            labeled_row(ui, &label, |ui| changed = json_value(ui, salt, v, depth));
            changed
        }
        _ => json_leaf(ui, salt, &label, v),
    }
}

/// A hook a caller sets (in the context's temporary data) to give string
/// settings a list of choices, by their path (`world/scatter/0/kind`).
pub type ChoiceFn = fn(&str) -> Option<&'static [&'static str]>;

fn json_choices(ui: &egui::Ui, path: &str) -> Option<&'static [&'static str]> {
    let f = ui.data(|d| d.get_temp::<ChoiceFn>(egui::Id::new("json-choices")))?;
    f(path)
}

pub fn set_json_choices(ui: &egui::Ui, f: Option<ChoiceFn>) {
    ui.data_mut(|d| match f {
        Some(f) => {
            d.insert_temp(egui::Id::new("json-choices"), f);
        }
        None => {
            d.remove::<ChoiceFn>(egui::Id::new("json-choices"));
        }
    });
}

fn json_leaf(ui: &mut egui::Ui, salt: &str, label: &str, v: &mut Value) -> bool {
    let tk = theme::tokens(ui.ctx());
    let mut changed = false;
    labeled_row(ui, label, |ui| match v {
        Value::Null => {
            ui.label(egui::RichText::new("Not set").font(theme::body()).color(tk.text_2));
            // An optional setting that is not set: give it a value to edit.
            if Button::ghost("Set").compact().show(ui).clicked() {
                let key = salt.rsplit('/').next().unwrap_or("");
                *v = if matches!(key, "on" | "colour" | "hdri" | "top" | "skin" | "style" | "foliage" | "ground" | "fog_colour") { Value::String(String::new()) } else { serde_json::json!(0.0) };
                changed = true;
            }
        }
        Value::String(s) if json_choices(ui, salt).is_some() => {
            let options = json_choices(ui, salt).unwrap_or(&[]);
            let at = options.iter().position(|o| *o == s.as_str());
            let mut shown: Vec<&str> = options.to_vec();
            if at.is_none() {
                shown.push(s.as_str());
            }
            if let Some(i) = kit::menu_button(ui, salt, at.unwrap_or(options.len()), &shown, ui.available_width() - 40.0, None)
                && let Some(o) = shown.get(i)
            {
                *s = (*o).to_owned();
                changed = true;
            }
        }
        Value::Bool(b) => changed = kit::toggle(ui, b, label, None).changed(),
        Value::Number(n) => {
            let int = n.is_i64() || n.is_u64();
            let mut f = n.as_f64().unwrap_or(0.0) as f32;
            let speed = (f.abs() / 200.0).max(if int { 0.1 } else { 0.01 });
            let r = NumberField::new(label, &mut f).decimals(if int { 0 } else { 2 }).drag_speed(speed).width(72.0).show(ui);
            if r.changed() {
                *v = if int { serde_json::json!(f.round() as i64) } else { serde_json::json!(f) };
                changed = true;
            }
        }
        Value::String(s) if looks_hex(s) => {
            let mut col = kit::from_hex(s).unwrap_or(Color32::WHITE);
            if kit::colour_well(ui, salt, &mut col, None).changed() {
                *s = kit::to_hex(col);
                changed = true;
            }
            ui.label(egui::RichText::new(s.clone()).font(theme::body()).color(tk.text_2));
        }
        Value::String(s) => {
            let key = egui::Id::new(("json-text", salt));
            let mut edit: String = ui.data(|d| d.get_temp(key)).unwrap_or_else(|| s.clone());
            let w = ui.available_width();
            let r = kit::text_field(ui, &mut edit, label, None, w, None);
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(key, edit.clone()));
            }
            if r.lost_focus() {
                ui.data_mut(|d| d.remove::<String>(key));
                if edit != *s {
                    *s = edit;
                    changed = true;
                }
            }
        }
        _ => {}
    });
    changed
}

fn data_section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip) {
    let keys: Vec<String> = mc.meta.keys().filter(|k| !owned_elsewhere(k)).cloned().collect();
    if keys.is_empty() {
        return;
    }
    let mut meta = mc.meta.clone();
    let mut changed = false;
    Section::new("data", "Data").icon(Icon::List).start_closed().show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        ui.label(egui::RichText::new("What the AI's tools remember on this clip. Change a value and the tool picks it up.").font(theme::caption()).color(tk.text_2));
        for k in &keys {
            if let Some(v) = meta.get_mut(k) {
                ui.label(egui::RichText::new(humanise(k)).font(theme::section_title()).color(tk.text));
                changed |= json_value(ui, &format!("meta-{}-{k}", mc.id.0), v, 0);
            }
        }
    });
    if changed {
        put(app, ids, "meta", "Clip data", |c| c.meta = meta.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The variants of `GeneratorKind`, read from the model's source.
    fn variants() -> Vec<String> {
        let src = include_str!("../../../crates/project/model/src/clip.rs");
        let body = &src[src.find("pub enum GeneratorKind").expect("enum")..];
        let body = &body[..body.find("\n}").expect("end")];
        body.lines().filter_map(|l| l.strip_prefix("    ")).filter(|l| l.chars().next().is_some_and(char::is_uppercase)).map(|l| l.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_owned()).collect()
    }

    #[test]
    fn every_generator_kind_has_manual_controls() {
        let named = variants();
        assert_eq!(named.len(), ALL_KINDS.len(), "GeneratorKind has {named:?}; list the new kind in ALL_KINDS and generator_sections");
        let shown = super::super::inspector::all_tab_kinds();
        for k in ALL_KINDS {
            let sections = generator_sections(k);
            assert!(!sections.is_empty(), "{k:?} has no Inspector section");
            for s in sections {
                assert!(shown.contains(s), "{k:?}: section \"{s}\" is in no Inspector tab, so it can never be seen");
            }
            assert!(sections.contains(&"generator"), "{k:?} lacks the all-settings fallback");
        }
    }

    #[test]
    fn this_modules_sections_are_in_tabs() {
        let shown = super::super::inspector::all_tab_kinds();
        for s in SECTIONS {
            assert!(shown.contains(s), "section \"{s}\" is in no Inspector tab");
        }
    }

    /// Every `meta` key a tool writes has an owner here.
    #[test]
    fn every_meta_key_a_tool_writes_has_a_manual_control() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut keys = std::collections::BTreeSet::new();
        let mut stack = vec![root.join("agent"), root.join("services"), root.join("editor")];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.file_stem().and_then(|f| f.to_str()).is_some_and(|f| f == "tests" || f.ends_with("_tests")) {
                    // Test files set meta up as fixtures; only the tools' own writes count.
                    let src = std::fs::read_to_string(&p).unwrap_or_default();
                    for pat in ["meta.insert(\"", "meta.entry(\"", "meta.insert(META.into()"] {
                        let mut rest = src.as_str();
                        while let Some(at) = rest.find(pat) {
                            rest = &rest[at + pat.len()..];
                            if pat.ends_with("META.into()") {
                                keys.insert("template".to_owned());
                            } else if let Some(end) = rest.find('"') {
                                keys.insert(rest[..end].to_owned());
                            }
                        }
                    }
                }
            }
        }
        assert!(keys.len() >= 10, "found only {keys:?}");
        let shown = super::super::inspector::all_tab_kinds();
        for k in &keys {
            let owner = META_OWNERS.iter().find(|(n, _)| n == k);
            let Some((_, section)) = owner else { panic!("the tools write clip meta \"{k}\" but META_OWNERS (editor/params.rs) has no control for it") };
            assert!(*section == "data" || shown.contains(section), "meta \"{k}\": section \"{section}\" is in no Inspector tab");
        }
    }

    #[test]
    fn generic_rows_edit_every_value_type() {
        use eframe::egui::{Context, RawInput};
        let ctx = Context::default();
        crate::theme::apply_style(&ctx);
        let mut vals = vec![
            ("flag", m::Value::Bool(false)),
            ("amount", m::Value::Scalar(0.5)),
            ("turn", m::Value::Angle(30.0)),
            ("spot", m::Value::Point(m::Vec2 { x: 1.0, y: 2.0 })),
            ("tint", m::Value::Colour(m::Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 })),
            ("note", m::Value::Text("hi".into())),
            ("shape", m::Value::Choice("star".into())),
            ("ramp", m::Value::Curve(vec![[0.0, 0.0], [1.0, 1.0]])),
        ];
        let mut rows = 0;
        let mut out = ctx.run_ui(RawInput::default(), |ui| {
            for (k, v) in vals.iter_mut() {
                let before = ui.min_rect().height();
                value_row(ui, "t", k, v);
                if ui.min_rect().height() > before {
                    rows += 1;
                }
            }
            let mut j = serde_json::json!({"a": 1, "b": [1.5, 2.5], "c": "#ff0000", "d": {"e": true, "f": "x"}, "g": [1, 2, 3]});
            json_value(ui, "j", &mut j, 0);
        });
        out.textures_delta.clear();
        assert_eq!(rows, vals.len(), "every value type draws a row");
    }

    /// Text painted in a frame.
    fn painted(out: &egui::FullOutput) -> String {
        out.shapes.iter().filter_map(|c| if let egui::epaint::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    /// The case that started this: a lower third's name and role are fields
    /// in the Inspector of the placed clip, not only words to the AI.
    #[test]
    fn a_placed_lower_third_shows_its_text_controls_in_the_inspector() {
        use crate::agent::Session;
        use crate::ui_tests::Harness;
        let mut h = Harness::new(&["--screen", "editor"]);
        h.frames(3);
        let doc = std::mem::replace(&mut h.app.doc, Session::default().doc);
        let mut s = Session::new(doc);
        s.edit(&[serde_json::json!({"op": "template", "id": "lower_third.bar", "at": 1.0})], "template", false).expect("place a lower third");
        h.app.doc = s.doc;
        let id = h.app.doc.model.active().clips.iter().find(|c| c.meta.contains_key("template")).map(|c| c.id.0 as u32).expect("the template clip");
        h.app.editor.selection = vec![id];
        h.frames(5);
        let text = painted(h.last.as_ref().unwrap());
        let t = studio_mograph::find("lower_third.bar").unwrap();
        assert!(text.contains("Template"), "the Template section is drawn:\n{text}");
        for c in &t.controls {
            assert!(text.contains(c.label), "control \"{}\" is in the Inspector:\n{text}", c.label);
        }
    }

    /// A solid card, a gradient and a shape each show their fill controls.
    #[test]
    fn cards_show_their_fill_and_shape_controls() {
        use crate::agent::Session;
        use crate::ui_tests::Harness;
        let mut h = Harness::new(&["--screen", "editor"]);
        h.frames(3);
        let doc = std::mem::replace(&mut h.app.doc, Session::default().doc);
        let mut s = Session::new(doc);
        s.edit(
            &[
                serde_json::json!({"op": "card", "colour": "#ff2d9b", "at": 1.0, "duration": 2.0, "lane": "overlay"}),
                serde_json::json!({"op": "card", "colour": "#222222", "gradient": ["#000000", "#ffffff"], "at": 4.0, "duration": 2.0, "lane": "overlay"}),
                serde_json::json!({"op": "card", "colour": "#00aa00", "at": 7.0, "duration": 2.0, "lane": "overlay", "width": 400, "height": 120, "roundness": 0.3}),
            ],
            "cards",
            false,
        )
        .expect("cards");
        h.app.doc = s.doc;
        let cards: Vec<(u32, m::GeneratorKind)> = h
            .app
            .doc
            .model
            .active()
            .clips
            .iter()
            .filter_map(|c| match &c.source {
                m::ClipSource::Generator(g) if matches!(g.kind, m::GeneratorKind::Solid | m::GeneratorKind::Gradient | m::GeneratorKind::Shape) => Some((c.id.0 as u32, g.kind)),
                _ => None,
            })
            .collect();
        assert_eq!(cards.len(), 3);
        for (id, kind) in cards {
            h.app.editor.selection = vec![id];
            h.frames(5);
            let text = painted(h.last.as_ref().unwrap());
            let want: &[&str] = match kind {
                m::GeneratorKind::Solid => &["Fill", "Colour"],
                m::GeneratorKind::Gradient => &["Fill", "From", "To", "Angle"],
                _ => &["Shape", "Fill", "Size", "Roundness"],
            };
            for w in want {
                assert!(text.contains(w), "{kind:?}: \"{w}\" is in the Inspector:\n{text}");
            }
            // Name, enabled and label of every clip.
            for w in ["Name", "Enabled", "Label"] {
                assert!(text.contains(w), "{kind:?}: \"{w}\" is in the Inspector:\n{text}");
            }
        }
    }

    /// A text layer shows its font, weight, outline and shadow controls.
    #[test]
    fn a_text_layer_shows_font_outline_and_shadow() {
        use crate::agent::Session;
        use crate::ui_tests::Harness;
        let mut h = Harness::new(&["--screen", "editor"]);
        h.frames(3);
        let doc = std::mem::replace(&mut h.app.doc, Session::default().doc);
        let mut s = Session::new(doc);
        s.edit(&[serde_json::json!({"op": "text", "text": "HELLO", "at": 1.0, "duration": 2.0})], "text", false).expect("text");
        h.app.doc = s.doc;
        let id = h.app.doc.model.active().clips.iter().find(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Text)).map(|c| c.id.0 as u32).expect("text clip");
        h.app.editor.selection = vec![id];
        h.frames(5);
        let text = painted(h.last.as_ref().unwrap());
        for w in ["Font", "Weight", "Italic", "Case", "Outline", "Shadow"] {
            assert!(text.contains(w), "\"{w}\" is in the Text section:\n{text}");
        }
    }
    /// Draws `rows` field rows and menu rows in a column `width` wide; returns
    /// (content x of each labelled row, right edge of each menu, right edge
    /// of each field, the column's right edge) and what is cut or overlapped.
    fn grid(width: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>, f32, usize, usize) {
        let _serial = crate::ui_tests::serial();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        crate::theme::install_fonts(&ctx);
        let mut starts = vec![];
        let mut menus = vec![];
        let mut right = 0.0;
        let mut out = None;
        for frame in 0..3 {
            starts.clear();
            menus.clear();
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 400.0))), time: Some(frame as f64 / 60.0), ..Default::default() };
            let mut frame_out = ctx.run_ui(raw, |ui| {
                let col = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 400.0));
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(col));
                let ui = &mut ui;
                right = ui.max_rect().right();
                let (mut a, mut b, mut c) = (0.0f32, 12.5f32, 100.0f32);
                kit::FieldRow::new("Position X", &mut a, -100.0..=100.0).unit("px").show(ui);
                kit::FieldRow::new("Rotation", &mut b, -360.0..=360.0).unit("°").decimals(1).show(ui);
                for (label, id) in [("Blend", "g-blend"), ("Style", "g-style")] {
                    labeled_row(ui, label, |ui| {
                        starts.push(ui.cursor().min.x);
                        let w = ui.available_width() - 40.0;
                        kit::menu_button(ui, id, 0, &["Normal", "Multiply"], w, None);
                        menus.push(ui.min_rect().right());
                    });
                }
                kit::FieldRow::new("Opacity", &mut c, 0.0..=100.0).unit("%").show(ui);
            });
            // Taken as a renderer takes them; dropping them unhandled would panic.
            frame_out.textures_delta.clear();
            out = Some(frame_out);
        }
        let out = out.unwrap();
        let names: std::collections::HashMap<u64, String> = out.platform_output.accesskit_update.as_ref().map(|u| u.nodes.iter().map(|(id, n)| (id.0, n.label().or(n.value()).unwrap_or("").to_owned())).collect()).unwrap_or_default();
        // A field row's value box is named by its value, so find it by the row
        // its label opens: the rightmost draggable widget on that row.
        let fields: Vec<f32> = ["Position X", "Rotation", "Opacity"]
            .iter()
            .map(|want| {
                ctx.viewport(|v| {
                    let all: Vec<_> = v.this_pass.widgets.layers().flat_map(|(_, l)| l.iter()).collect();
                    let top = all.iter().find(|w| w.interact_rect.left() == 0.0 && names.get(&w.id.value()).is_some_and(|n| n == want)).map(|w| w.interact_rect.top()).expect("the row's label");
                    all.iter().filter(|w| w.sense.is_focusable() && (w.interact_rect.top() - top).abs() < 0.5).map(|w| w.interact_rect.right()).fold(f32::MIN, f32::max)
                })
            })
            .collect();
        let cuts = crate::ui_shot::cut_text(&ctx, &out, &[], 1.0).len();
        let over = crate::ui_shot::overlaps(&ctx, &names, 0.2).len();
        (starts, menus, fields, right, cuts, over)
    }

    #[test]
    fn labels_share_a_column_and_values_a_right_edge_at_narrow_and_wide_widths() {
        for width in [240.0f32, 280.0, 480.0] {
            let (starts, menus, fields, right, cuts, over) = grid(width);
            let want_x = if width < 260.0 { 80.0 } else { kit::label_width(&egui::Context::default()) };
            assert!(starts.iter().all(|x| (x - starts[0]).abs() < 0.5), "{width}: labelled rows start together: {starts:?}");
            assert!((starts[0] - want_x).abs() < 1.0, "{width}: content starts after the label column ({want_x}): {starts:?}");
            for m in &menus {
                assert!((m - (right - 40.0)).abs() < 1.5, "{width}: menu ends at the value edge {}: {menus:?}", right - 40.0);
            }
            for f in &fields {
                assert!((f - (right - 40.0)).abs() < 1.5, "{width}: field ends at the value edge {}: {fields:?}", right - 40.0);
            }
            assert_eq!(cuts, 0, "{width}: text cut off");
            assert_eq!(over, 0, "{width}: widgets overlap");
        }
    }

    /// Clearing a label is its own undo step, and Reset crop is undoable.
    #[test]
    fn clear_label_and_reset_crop_undo() {
        use crate::agent::Session;
        use crate::ui_tests::Harness;
        let mut h = Harness::new(&["--screen", "editor"]);
        h.frames(3);
        let doc = std::mem::replace(&mut h.app.doc, Session::default().doc);
        let mut s = Session::new(doc);
        s.edit(&[serde_json::json!({"op": "card", "colour": "#1030e0", "at": 0.0, "duration": 3.0, "lane": "overlay"})], "card", false).expect("a card");
        let id = s.doc.model.active().clips.iter().map(|c| c.id.0).max().unwrap();
        h.app.doc = s.doc;
        let ids = [id as u32];
        let label = |h: &Harness| mclip(&h.app, ids[0]).unwrap().label;
        put(&mut h.app, &ids, "label", "Label colour", |c| c.label = Some(crate::bridge::rgba(Color32::RED)));
        put(&mut h.app, &ids, "label-clear", "Clear label", |c| c.label = None);
        assert!(label(&h).is_none());
        h.app.doc.undo();
        assert!(label(&h).is_some(), "the colour is still there: clearing was a step of its own");
        put(&mut h.app, &ids, "crop-reset", "Reset crop", |c| c.transform.crop = m::Crop::default());
        put(&mut h.app, &ids, "flip", "Flip", |c| c.transform.flip_x = true);
        put(&mut h.app, &ids, "crop-reset", "Reset crop", |c| c.transform.flip_x = false);
        h.app.doc.undo();
        assert!(mclip(&h.app, ids[0]).unwrap().transform.flip_x, "undo brings the flip back");
    }
}
