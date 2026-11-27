// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A model's sections in the Inspector: its parts, the modifier stack
//! (add, switch off, reorder, apply, remove, and every setting), material
//! slots with a material editor (faces get a slot in edit mode), the UV
//! layout (see it, pick and move pieces, unwrap, pack), shading, and the
//! object tools (origin, apply transform, join, keep in My Models).

use super::{Stage, with};
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, MenuItem, Section};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::{Map, Value, json};
use studio_mesh::V3;
use studio_model as m;
use studio_modeling::asset::{Asset, Part};
use studio_modeling::material::{self, MaterialDef};
use studio_modeling::modifiers::Modifier;

/// Changes a model's source (the open one, or the clip's own) as one
/// undo step, and redraws it.
pub fn edit_source(app: &mut StudioApp, clip: u64, label: &str, f: impl FnOnce(&mut Asset) -> Result<(), String>) -> Result<(), String> {
    if super::open_clip() == Some(clip) {
        let mut a = with(|s| s.as_ref().map(|s| s.asset.clone())).ok_or("no model")?;
        f(&mut a)?;
        with(|s| {
            if let Some(s) = s {
                s.asset = a;
                s.part = s.part.min(s.asset.parts.len().saturating_sub(1));
                let p = s.poly().clone();
                s.sel.fit(&p);
                s.sel.sync(&p);
                s.changed();
            }
        });
        return super::commit(app, label);
    }
    let mc = app.doc.model.active().clip(m::ClipId(clip)).cloned().ok_or("that clip is gone")?;
    let (mut a, mesh) = super::load_source(&mc)?;
    f(&mut a)?;
    let mut out = a.to_mesh(false)?;
    if let Some(rig) = &mesh.rig {
        out.rig = Some(std::sync::Arc::new(rig.transferred(&mesh, &out)));
    }
    let file = crate::mesh_store::save(&app.doc, &out)?;
    let source = crate::mesh_store::save_source(&app.doc, &a)?;
    let mut c = mc;
    crate::mesh_store::set_file(&mut c, file);
    crate::mesh_store::set_source(&mut c, Some(source));
    let seq = app.doc.seq();
    if !app.doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(c) }) {
        return Err(app.doc.error.take().unwrap_or_else(|| "refused".into()));
    }
    Ok(())
}

thread_local! {
    /// Sources read for the Inspector, by their (content-named) file: a
    /// model is read and joined into quads once, not every frame.
    static READ: std::cell::RefCell<Vec<(String, Asset)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The model's asset as it is now (open or on disk).
fn asset_of(_app: &StudioApp, mc: &m::Clip) -> Option<(Asset, usize)> {
    if super::open_clip() == Some(mc.id.0) {
        return with(|s| s.as_ref().map(|s| (s.asset.clone(), s.part)));
    }
    let key = crate::mesh_store::source_of(mc).or_else(|| crate::mesh_store::file_of(mc))?;
    if let Some(a) = READ.with(|r| r.borrow().iter().find(|(k, _)| *k == key).map(|(_, a)| a.clone())) {
        return Some((a, 0));
    }
    let a = super::load_source(mc).ok()?.0;
    READ.with(|r| {
        let mut r = r.borrow_mut();
        r.push((key, a.clone()));
        if r.len() > 8 {
            r.remove(0);
        }
    });
    Some((a, 0))
}

fn note(ctx: &egui::Context, r: Result<(), String>) {
    if let Err(e) = r {
        kit::push(ctx, e, None, Some(Icon::Warning));
    }
}

fn size_of(a: &Asset, part: usize) -> f32 {
    let (lo, hi) = a.parts.get(part).map(|p| p.mesh.bounds()).unwrap_or(([0.0; 3], [100.0; 3]));
    studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0)
}

/// The modifier kinds a person can add, with sensible starting settings.
pub fn new_modifier(kind: &str, size: f32, other_part: Option<&str>) -> Option<Modifier> {
    let v = match kind {
        "subdivide" => json!({"kind": "subdivide", "levels": 1}),
        "mirror" => json!({"kind": "mirror", "axes": [true, false, false], "bisect": false, "weld": 0.0}),
        "array" => json!({"kind": "array", "count": 3, "offset": [0.0, 0.0, 0.0], "relative": [1.1, 0.0, 0.0], "merge": false}),
        "radial" => json!({"kind": "radial", "count": 6, "axis": 1, "degrees": 360.0, "centre": [0.0, 0.0, 0.0]}),
        "bevel" => json!({"kind": "bevel", "width": size * 0.02, "segments": 2, "profile": 0.5, "angle": 30.0, "only_marked": false}),
        "solidify" => json!({"kind": "solidify", "thickness": size * 0.02, "offset": -1.0, "rim": true}),
        "boolean" => json!({"kind": "boolean", "op": "subtract", "with": other_part.unwrap_or("")}),
        "displace" => json!({"kind": "displace", "amount": size * 0.02, "size": size * 0.2, "octaves": 4, "seed": 0}),
        "smooth" => json!({"kind": "smooth", "iterations": 5, "factor": 0.5}),
        "weld" => json!({"kind": "weld", "distance": size * 0.001}),
        "triangulate" => json!({"kind": "triangulate"}),
        "decimate" => json!({"kind": "decimate", "ratio": 0.5}),
        "remesh" => json!({"kind": "remesh", "size": size / 40.0}),
        "shrinkwrap" => json!({"kind": "shrinkwrap", "target": other_part.unwrap_or(""), "offset": 0.0}),
        "lattice" => json!({"kind": "lattice", "res": [3, 3, 3], "moves": []}),
        "bend" => json!({"kind": "bend", "axis": 0, "toward": 1, "degrees": 45.0}),
        "twist" => json!({"kind": "twist", "axis": 1, "degrees": 90.0}),
        "taper" => json!({"kind": "taper", "axis": 1, "factor": 0.5}),
        "wave" => json!({"kind": "wave", "axis": 0, "toward": 1, "amplitude": size * 0.03, "wavelength": size * 0.3, "phase": 0.0}),
        "cast" => json!({"kind": "cast", "factor": 1.0}),
        "weighted_normals" => json!({"kind": "weighted_normals"}),
        _ => return None,
    };
    serde_json::from_value(v).ok()
}

/// Modifier kinds in the Add menu, with their names.
pub const KINDS: &[(&str, &str)] = &[
    ("subdivide", "Subdivision surface"),
    ("mirror", "Mirror"),
    ("array", "Array"),
    ("radial", "Radial array"),
    ("bevel", "Bevel"),
    ("solidify", "Solidify"),
    ("boolean", "Boolean"),
    ("displace", "Displace"),
    ("smooth", "Smooth"),
    ("weld", "Weld"),
    ("triangulate", "Triangulate"),
    ("decimate", "Decimate"),
    ("remesh", "Remesh"),
    ("shrinkwrap", "Shrinkwrap"),
    ("lattice", "Lattice"),
    ("bend", "Bend"),
    ("twist", "Twist"),
    ("taper", "Taper"),
    ("wave", "Wave"),
    ("cast", "Cast"),
    ("weighted_normals", "Weighted normals"),
];

fn kind_label(m: &Modifier) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == m.name()).map_or("Modifier", |(_, l)| l)
}

/// Every section for a model clip.
pub fn sections(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip) {
    let Some((asset, part)) = asset_of(app, mc) else { return };
    parts_section(app, ui, mc, &asset, part);
    modifiers_section(app, ui, mc, &asset, part);
    materials_section(app, ui, mc, &asset, part);
    uv_section(app, ui, mc, &asset, part);
    object_section(app, ui, mc, &asset, part);
}

fn parts_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, asset: &Asset, part: usize) {
    if asset.parts.len() < 2 {
        return;
    }
    let ctx = ui.ctx().clone();
    Section::new("model-parts", "Parts").icon(Icon::Stack).show(ui, |ui| {
        for (i, p) in asset.parts.iter().enumerate() {
            let label = if p.hidden { crate::i18n::tf(format!("{} (hidden)", p.name)) } else { p.name.clone() };
            if MenuItem::new(&label).checked(i == part && super::open_clip() == Some(mc.id.0)).show(ui).on_hover_text(p.mesh.stats()).clicked() {
                if super::open_clip() != Some(mc.id.0) {
                    app.editor.selection = vec![mc.id.0 as u32];
                    note(&ctx, super::enter(app, &ctx, Stage::Edit));
                }
                with(|s| {
                    if let Some(s) = s {
                        s.part = i.min(s.asset.parts.len() - 1);
                        s.sel = studio_modeling::selection::Selection::new(s.poly(), s.sel.mode);
                        s.last = None;
                        s.changed();
                    }
                });
            }
        }
    });
}

fn modifiers_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, asset: &Asset, part: usize) {
    let ctx = ui.ctx().clone();
    let id = mc.id.0;
    let Some(p) = asset.parts.get(part) else { return };
    let others: Vec<String> = asset.parts.iter().enumerate().filter(|(i, _)| *i != part).map(|(_, p)| p.name.clone()).collect();
    let size = size_of(asset, part);
    enum Act {
        Add(&'static str),
        Toggle(usize),
        Up(usize),
        Down(usize),
        Apply(usize),
        Remove(usize),
        Set(usize, Modifier),
        Cutter(u64),
        Cage(usize),
    }
    let mut act: Option<Act> = None;
    Section::new("model-modifiers", "Modifiers").icon(Icon::Stack).show(ui, |ui| {
        let add_id = egui::Id::unique(("model-add-modifier", id));
        let resp = kit::Button::secondary("Add modifier").icon(Icon::Plus).compact().show(ui);
        if resp.clicked() {
            kit::toggle_open(&ctx, add_id);
        }
        kit::popover(&ctx, add_id, resp.rect, kit::Side::Below, 220.0, |ui| {
            for (k, l) in KINDS {
                if MenuItem::new(l).show(ui).clicked() {
                    act = Some(Act::Add(k));
                }
            }
            // A cutter from another model in the project.
            let models: Vec<(u64, String)> = app.doc.model.active().clips.iter().filter(|c| c.id.0 != id && crate::mesh_store::file_of(c).is_some()).map(|c| (c.id.0, c.name.clone())).collect();
            if !models.is_empty() {
                kit::menu_separator(ui);
                kit::menu_heading(ui, "Boolean with an object");
                for (cid, name) in models {
                    if MenuItem::new(&name).show(ui).on_hover_text(crate::i18n::t("Subtracts a hidden copy as a cutter")).clicked() {
                        act = Some(Act::Cutter(cid));
                    }
                }
            }
        });
        if act.is_some() {
            kit::set_open(&ctx, add_id, false);
        }
        if p.modifiers.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::t("No modifiers: the model is drawn as modelled.")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        }
        for (i, md) in p.modifiers.iter().enumerate() {
            let on = !p.disabled.get(i).copied().unwrap_or(false);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut o = on;
                if kit::toggle(ui, &mut o, "Show this modifier", None).changed() {
                    act = Some(Act::Toggle(i));
                }
                ui.label(egui::RichText::new(crate::i18n::t(kind_label(md))).font(theme::body_medium()).color(theme::tokens(ui.ctx()).text));
                if kit::IconButton::new(Icon::CaretUp, "Move up").compact().show(ui).clicked() {
                    act = Some(Act::Up(i));
                }
                if kit::IconButton::new(Icon::CaretDown, "Move down").compact().show(ui).clicked() {
                    act = Some(Act::Down(i));
                }
                if kit::Button::ghost("Apply").compact().show(ui).on_hover_text(crate::i18n::t("Make this modifier's change part of the mesh")).clicked() {
                    act = Some(Act::Apply(i));
                }
                if kit::IconButton::new(Icon::Trash, "Remove").compact().show(ui).clicked() {
                    act = Some(Act::Remove(i));
                }
            });
            if matches!(md, Modifier::Lattice(_)) {
                let editing = with(|s| s.as_ref().and_then(|s| s.lattice.as_ref()).is_some_and(|e| e.modifier == i));
                let b = if editing { kit::Button::secondary("Editing cage") } else { kit::Button::ghost("Edit cage") };
                if b.compact().show(ui).on_hover_text(crate::i18n::t("Move the lattice's control points in the 3D workspace")).clicked() {
                    act = Some(Act::Cage(i));
                }
            }
            if let Some(nm) = modifier_fields(ui, i, md, &others) {
                act = Some(Act::Set(i, nm));
            }
        }
    });
    let Some(a) = act else { return };
    let pname = p.name.clone();
    let r = match a {
        Act::Add(k) => match new_modifier(k, size, others.first().map(String::as_str)) {
            Some(md) => edit_source(app, id, &crate::i18n::tf(format!("Add {} modifier", crate::i18n::t(kind_label(&md)).to_lowercase())), |a| {
                a.parts[part].modifiers.push(md);
                Ok(())
            }),
            None => Ok(()),
        },
        Act::Toggle(i) => edit_source(app, id, "Switch modifier", |a| {
            let p = &mut a.parts[part];
            p.disabled.resize(p.modifiers.len(), false);
            p.disabled[i] = !p.disabled[i];
            Ok(())
        }),
        Act::Up(i) | Act::Down(i) => {
            let down = matches!(a, Act::Down(_));
            edit_source(app, id, "Reorder modifiers", |a| {
                let p = &mut a.parts[part];
                p.disabled.resize(p.modifiers.len(), false);
                let j = if down { i + 1 } else { i.wrapping_sub(1) };
                if j < p.modifiers.len() {
                    p.modifiers.swap(i, j);
                    p.disabled.swap(i, j);
                }
                Ok(())
            })
        }
        Act::Remove(i) => edit_source(app, id, "Remove modifier", |a| {
            let p = &mut a.parts[part];
            p.modifiers.remove(i);
            if i < p.disabled.len() {
                p.disabled.remove(i);
            }
            Ok(())
        }),
        Act::Set(i, nm) => edit_source(app, id, "Modifier settings", |a| {
            a.parts[part].modifiers[i] = nm;
            Ok(())
        }),
        Act::Apply(i) => edit_source(app, id, "Apply modifier", |a| apply_modifier(a, part, i)),
        Act::Cutter(cid) => add_cutter(app, id, cid, part),
        Act::Cage(i) => {
            if with(|s| s.as_ref().and_then(|s| s.lattice.as_ref()).is_some_and(|e| e.modifier == i)) {
                super::lattice::stop();
                Ok(())
            } else {
                if super::open_clip() != Some(id) {
                    app.editor.selection = vec![id as u32];
                }
                super::lattice::start(app, &ctx, i)
            }
        }
    };
    let _ = pname;
    note(&ctx, r);
}

/// Makes one modifier's change part of the base mesh.
pub fn apply_modifier(a: &mut Asset, part: usize, i: usize) -> Result<(), String> {
    a.apply_modifier(part, i)
}

/// Copies another model clip in as a hidden part (in this model's space)
/// and subtracts it.
fn add_cutter(app: &mut StudioApp, id: u64, other: u64, part: usize) -> Result<(), String> {
    let seq = app.doc.model.active();
    let (Some(me), Some(them)) = (seq.clip(m::ClipId(id)).cloned(), seq.clip(m::ClipId(other)).cloned()) else { return Err("that clip is gone".into()) };
    let w = app.doc.project.width as f64;
    let t = super::draw::time_of(app);
    let mx_me = crate::editor::viewport::model_matrix(&studio_model::space::oriented_transform(seq, &me, t.clamp(me.start, me.start + me.duration - 1), w));
    let mx_them = crate::editor::viewport::model_matrix(&studio_model::space::oriented_transform(seq, &them, t.clamp(them.start, them.start + them.duration - 1), w));
    let inv = super::draw::invert(&mx_me).ok_or("this model is scaled flat")?;
    let (their, _) = super::load_source(&them)?;
    let mut poly = studio_modeling::poly::Poly::from_mesh(&their.to_mesh(true)?);
    let to = |p: V3| -> V3 {
        let wpos: [f64; 3] = std::array::from_fn(|r| mx_them[r][0] * p[0] as f64 + mx_them[r][1] * p[1] as f64 + mx_them[r][2] * p[2] as f64 + mx_them[r][3]);
        std::array::from_fn(|r| (inv[r][0] * wpos[0] + inv[r][1] * wpos[1] + inv[r][2] * wpos[2] + inv[r][3]) as f32)
    };
    poly.map_positions(to);
    edit_source(app, id, "Boolean with an object", |a| {
        let name = a.unique_name(&them.name);
        let mut cut = Part::new(&name, poly);
        cut.hidden = true;
        a.parts.push(cut);
        a.parts[part].modifiers.push(Modifier::Boolean { op: "subtract".into(), with: name });
        Ok(())
    })
}

/// A modifier's settings from its own fields; the changed one, if any.
fn modifier_fields(ui: &mut egui::Ui, i: usize, md: &Modifier, others: &[String]) -> Option<Modifier> {
    let v = serde_json::to_value(md).ok()?;
    let obj = v.as_object()?.clone();
    let mut out = obj.clone();
    let mut changed = false;
    // Lattice keeps its fields one level down.
    for (k, val) in obj.iter() {
        if k == "kind" || k == "moves" || k == "min" || k == "max" {
            continue;
        }
        let label = super::menus::words(k);
        let salt = format!("mod-{i}-{k}");
        match val {
            Value::Number(n) => {
                let is_int = n.is_u64() || n.is_i64();
                let mut x = n.as_f64().unwrap_or(0.0) as f32;
                let axis_field = matches!(k.as_str(), "axis" | "toward");
                kit::labeled_row(ui, &label, |ui| {
                    if axis_field {
                        let idx = (x as usize).min(2);
                        if let Some(a) = kit::menu_button(ui, &salt, idx, &["X", "Y", "Z"], 70.0, None) {
                            out.insert(k.clone(), json!(a));
                            changed = true;
                        }
                    } else if kit::NumberField::new(&salt, &mut x).decimals(if is_int { 0 } else { 2 }).width(100.0).show(ui).changed() {
                        out.insert(k.clone(), if is_int { json!(x.round().max(0.0) as u64) } else { json!(x) });
                        changed = true;
                    }
                });
            }
            Value::Bool(b) => {
                let mut x = *b;
                kit::labeled_row(ui, &label, |ui| {
                    if kit::toggle(ui, &mut x, &label, None).changed() {
                        out.insert(k.clone(), json!(x));
                        changed = true;
                    }
                });
            }
            Value::String(s) => {
                kit::labeled_row(ui, &label, |ui| {
                    let options: Vec<String> = match k.as_str() {
                        "op" => vec!["union".into(), "subtract".into(), "intersect".into()],
                        _ => others.to_vec(),
                    };
                    if options.is_empty() {
                        ui.label(egui::RichText::new(crate::i18n::t("(no other part to use)")).font(theme::caption()));
                        return;
                    }
                    let refs: Vec<&str> = options.iter().map(String::as_str).collect();
                    let idx = options.iter().position(|o| o == s).unwrap_or(0);
                    if let Some(a) = kit::menu_button(ui, &salt, idx, &refs, 130.0, None) {
                        out.insert(k.clone(), json!(options[a]));
                        changed = true;
                    }
                });
            }
            Value::Array(a) if a.len() == 3 => {
                kit::labeled_row(ui, &label, |ui| {
                    let mut arr = a.clone();
                    for (j, item) in a.iter().enumerate() {
                        let name = format!("{salt}-{j}");
                        match item {
                            Value::Bool(b) => {
                                let mut x = *b;
                                if kit::toggle(ui, &mut x, ["X", "Y", "Z"][j], None).changed() {
                                    arr[j] = json!(x);
                                    changed = true;
                                }
                            }
                            Value::Number(n) => {
                                let is_int = n.is_u64() || n.is_i64();
                                let mut x = n.as_f64().unwrap_or(0.0) as f32;
                                if kit::NumberField::new(&name, &mut x).decimals(if is_int { 0 } else { 2 }).width(52.0).show(ui).changed() {
                                    arr[j] = if is_int { json!(x.round().clamp(2.0, 6.0) as u64) } else { json!(x) };
                                    changed = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    out.insert(k.clone(), Value::Array(arr));
                });
            }
            _ => {}
        }
    }
    if !changed {
        return None;
    }
    serde_json::from_value(Value::Object(out)).ok()
}

fn to_srgb(v: f32) -> u8 {
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn to_linear(v: u8) -> f32 {
    studio_mesh::srgb_to_linear(v)
}

/// A material definition for a slot (made from its flat material when it
/// has none yet).
fn def_of(a: &Asset, part: usize, slot: usize) -> MaterialDef {
    if let Some(d) = a.parts[part].materials.get(slot).and_then(|n| a.material(n)) {
        return d.clone();
    }
    let flat = a.parts[part].mesh.materials.get(slot).cloned().unwrap_or_default();
    let mut d = material::smart("plastic", 100.0, None).unwrap_or_else(|| material::smart("paint", 100.0, None).unwrap());
    d.name = if flat.name.is_empty() || flat.name == "Default" { format!("Material {}", slot + 1) } else { flat.name.clone() };
    d.base.colour = [flat.base_colour[0], flat.base_colour[1], flat.base_colour[2]];
    d.base.metallic = flat.metallic;
    d.base.roughness = flat.roughness;
    d.base.pattern = None;
    d.layers.clear();
    d.micro = None;
    d.lobes.emissive = (flat.emissive != [0.0; 3]).then_some(flat.emissive);
    d.lobes.transmission = flat.transmission;
    d.lobes.subsurface = flat.subsurface;
    d
}

/// Puts a definition on a slot (named uniquely in the asset).
fn set_def(a: &mut Asset, part: usize, slot: usize, mut d: MaterialDef) {
    let p = &mut a.parts[part];
    if p.materials.len() <= slot {
        p.materials.resize(slot + 1, String::new());
    }
    let old = p.materials[slot].clone();
    if old.is_empty() || a.materials.iter().filter(|m| m.name == old).count() == 0 {
        // A fresh name.
        let mut name = d.name.clone();
        let mut n = 2;
        while a.materials.iter().any(|m| m.name == name) {
            name = format!("{} {n}", d.name);
            n += 1;
        }
        d.name = name;
        a.parts[part].materials[slot] = d.name.clone();
        a.materials.push(d);
    } else {
        d.name = old.clone();
        if let Some(m) = a.materials.iter_mut().find(|m| m.name == old) {
            *m = d;
        }
    }
    a.parts[part].mesh.ensure_materials();
}

fn materials_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, asset: &Asset, part: usize) {
    let ctx = ui.ctx().clone();
    let id = mc.id.0;
    let p = &asset.parts[part];
    let slots = p.mesh.materials.len().max(p.materials.len()).max(p.mesh.face_material.iter().copied().max().map_or(1, |m| m as usize + 1));
    let editing = super::open_clip() == Some(id) && super::stage() == Stage::Edit;
    let slot_id = egui::Id::unique(("model-material-slot", id));
    let mut slot: usize = ctx.data(|d| d.get_temp(slot_id)).unwrap_or(0).min(slots.saturating_sub(1));
    enum Act {
        Assign(usize),
        Select(usize),
        Add,
        Remove(usize),
        Def(usize, MaterialDef),
        Smart(usize, &'static str),
    }
    let mut act: Option<Act> = None;
    Section::new("model-materials", "Materials").icon(Icon::Palette).show(ui, |ui| {
        for s in 0..slots {
            let d = def_of(asset, part, s);
            let used = p.mesh.face_material.iter().filter(|m| **m as usize == s).count();
            ui.horizontal(|ui| {
                let col = Color32::from_rgb(to_srgb(d.base.colour[0]), to_srgb(d.base.colour[1]), to_srgb(d.base.colour[2]));
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                ui.painter().rect_filled(r, 4, col);
                if s == slot {
                    ui.painter().rect_stroke(r.expand(1.5), 4, Stroke::new(1.5, theme::tokens(ui.ctx()).accent), egui::StrokeKind::Outside);
                }
                let label = crate::i18n::tf(format!("{} · {used} faces", d.name));
                if resp.clicked() || MenuItem::new(&label).checked(s == slot).show(ui).clicked() {
                    slot = s;
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            if kit::Button::ghost("Add slot").icon(Icon::Plus).compact().show(ui).clicked() {
                act = Some(Act::Add);
            }
            if slots > 1 && kit::Button::ghost("Remove slot").compact().show(ui).on_hover_text(crate::i18n::t("Its faces go to the first slot")).clicked() {
                act = Some(Act::Remove(slot));
            }
            if editing {
                if kit::Button::secondary("Assign").compact().show(ui).on_hover_text(crate::i18n::t("Give the picked faces this material")).clicked() {
                    act = Some(Act::Assign(slot));
                }
                if kit::Button::ghost("Select").compact().show(ui).on_hover_text(crate::i18n::t("Pick the faces with this material")).clicked() {
                    act = Some(Act::Select(slot));
                }
            }
        });
        // The editor for the chosen slot.
        let d = def_of(asset, part, slot);
        kit::menu_separator(ui);
        let mut nd = d.clone();
        let mut name = nd.name.clone();
        kit::labeled_row(ui, "Name", |ui| {
            if kit::text_field(ui, &mut name, "Material name", None, 150.0, None).lost_focus() && name.trim() != d.name && !name.trim().is_empty() {
                nd.name = name.trim().to_owned();
            }
        });
        let mut col = Color32::from_rgb(to_srgb(nd.base.colour[0]), to_srgb(nd.base.colour[1]), to_srgb(nd.base.colour[2]));
        kit::labeled_row(ui, "Base colour", |ui| {
            if kit::colour_well(ui, "material-base", &mut col, None).changed() {
                nd.base.colour = [to_linear(col.r()), to_linear(col.g()), to_linear(col.b())];
            }
        });
        for (label, v) in [("Metallic", &mut nd.base.metallic), ("Roughness", &mut nd.base.roughness), ("Transmission", &mut nd.lobes.transmission), ("Subsurface", &mut nd.lobes.subsurface), ("Clear coat", &mut nd.lobes.clearcoat), ("Sheen", &mut nd.lobes.sheen)] {
            kit::labeled_row(ui, label, |ui| {
                let mut pct = *v * 100.0;
                if kit::Slider::new(label, &mut pct, 0.0..=100.0).unit("%").with_value().show(ui).changed() {
                    *v = pct / 100.0;
                }
            });
        }
        let mut glow = nd.lobes.emissive.unwrap_or([0.0; 3]);
        let mut gcol = Color32::from_rgb(to_srgb(glow[0]), to_srgb(glow[1]), to_srgb(glow[2]));
        kit::labeled_row(ui, "Glow", |ui| {
            if kit::colour_well(ui, "material-glow", &mut gcol, None).changed() {
                glow = [to_linear(gcol.r()), to_linear(gcol.g()), to_linear(gcol.b())];
                nd.lobes.emissive = (glow != [0.0; 3]).then_some(glow);
            }
        });
        let shadings = ["Standard", "Skin", "Hair", "Glass", "Cloth", "Car paint", "Eye", "Foliage", "Toon"];
        let kinds = [material::Shading::Standard, material::Shading::Skin, material::Shading::Hair, material::Shading::Glass, material::Shading::Cloth, material::Shading::CarPaint, material::Shading::Eye, material::Shading::Foliage, material::Shading::Toon];
        let si = kinds.iter().position(|k| *k == nd.shading).unwrap_or(0);
        kit::labeled_row(ui, "Shading", |ui| {
            if let Some(k) = kit::menu_button(ui, "material-shading", si, &shadings, 120.0, None) {
                nd.shading = kinds[k];
            }
        });
        let smart_id = egui::Id::unique(("model-smart-material", id));
        let resp = kit::Button::ghost("Smart material…").compact().show(ui).on_hover_text(crate::i18n::t("A layered material (wear, dirt, patterns), baked into textures"));
        if resp.clicked() {
            kit::toggle_open(&ctx, smart_id);
        }
        kit::popover(&ctx, smart_id, resp.rect, kit::Side::Below, 220.0, |ui| {
            for n in material::SMART {
                if MenuItem::new(&super::menus::words(n)).show(ui).clicked() {
                    act = Some(Act::Smart(slot, n));
                }
            }
        });
        if nd != d && act.is_none() {
            act = Some(Act::Def(slot, nd));
        }
    });
    ctx.data_mut(|d| d.insert_temp(slot_id, slot));
    let Some(a) = act else { return };
    let r = match a {
        Act::Assign(s) => {
            let mut args = Map::new();
            args.insert("slot".into(), json!(s));
            super::run(app, &ctx, "assign_material", args).map(|_| ())
        }
        Act::Select(s) => {
            let mut args = Map::new();
            args.insert("slot".into(), json!(s));
            super::run(app, &ctx, "select_material", args).map(|_| ())
        }
        Act::Add => edit_source(app, id, "Add material slot", |a| {
            let p = &mut a.parts[part];
            let n = p.mesh.materials.len().max(p.materials.len()).max(1);
            p.mesh.materials.resize_with(n + 1, Default::default);
            Ok(())
        }),
        Act::Remove(s) => edit_source(app, id, "Remove material slot", |a| {
            let p = &mut a.parts[part];
            for m in &mut p.mesh.face_material {
                if *m as usize == s {
                    *m = 0;
                } else if *m as usize > s {
                    *m -= 1;
                }
            }
            if s < p.mesh.materials.len() {
                p.mesh.materials.remove(s);
            }
            if s < p.materials.len() {
                p.materials.remove(s);
            }
            p.mesh.ensure_materials();
            Ok(())
        }),
        Act::Def(s, d) => edit_source(app, id, "Material", |a| {
            set_def(a, part, s, d);
            Ok(())
        }),
        Act::Smart(s, n) => {
            let size = size_of(asset, part);
            match material::smart(n, size * 0.05, None) {
                Some(mut d) => {
                    d.name = super::menus::words(n);
                    edit_source(app, id, "Smart material", |a| {
                        set_def(a, part, s, d);
                        Ok(())
                    })
                }
                None => Ok(()),
            }
        }
    };
    note(&ctx, r);
}

fn uv_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, asset: &Asset, part: usize) {
    let ctx = ui.ctx().clone();
    let id = mc.id.0;
    let editing = super::open_clip() == Some(id) && super::stage() == Stage::Edit;
    let p = &asset.parts[part].mesh;
    let mut run: Option<(&'static str, Map<String, Value>)> = None;
    Section::new("model-uv", "UV layout").icon(Icon::GridFour).show(ui, |ui| {
        let side = ui.available_width().min(260.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
        let painter = ui.painter().with_clip_rect(rect);
        // A checker so stretch shows.
        let n = 8;
        let cell = side / n as f32;
        for j in 0..n {
            for i in 0..n {
                let c = if (i + j) % 2 == 0 { Color32::from_gray(58) } else { Color32::from_gray(44) };
                painter.rect_filled(Rect::from_min_size(rect.min + Vec2::new(i as f32 * cell, j as f32 * cell), Vec2::splat(cell)), 0, c);
            }
        }
        let to_screen = |u: [f32; 2]| Pos2::new(rect.left() + u[0] * side, rect.bottom() - u[1] * side);
        let from_screen = |q: Pos2| [(q.x - rect.left()) / side, (rect.bottom() - q.y) / side];
        if !p.has_uvs() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, crate::i18n::t("No texture layout yet: unwrap the model"), theme::caption(), Color32::from_gray(200));
        } else {
            let sel = with(|s| s.as_ref().filter(|s| s.clip == id).map(|s| s.sel.faces.clone())).unwrap_or_default();
            let mut shapes = Vec::new();
            for f in 0..p.faces() {
                let us = p.face_uvs(f).unwrap();
                let picked = sel.get(f).copied().unwrap_or(false);
                let pts: Vec<Pos2> = us.iter().map(|u| to_screen(*u)).collect();
                if picked && pts.len() >= 3 {
                    for t in 1..pts.len() - 1 {
                        shapes.push(egui::Shape::convex_polygon(vec![pts[0], pts[t], pts[t + 1]], Color32::from_rgba_unmultiplied(255, 160, 40, 60), Stroke::NONE));
                    }
                }
                let mut l = pts.clone();
                l.push(pts[0]);
                shapes.push(egui::Shape::line(l, Stroke::new(if picked { 1.4 } else { 0.7 }, if picked { super::draw::PICKED } else { Color32::from_gray(210) })));
            }
            painter.extend(shapes);
            // Pick pieces by clicking a face; drag picked faces to move them.
            if editing {
                let drag_id = egui::Id::unique(("uv-drag", id));
                if resp.clicked()
                    && let Some(q) = resp.interact_pointer_pos()
                {
                    let u = from_screen(q);
                    let hit = (0..p.faces()).find(|&f| inside_uv(p.face_uvs(f).unwrap(), u));
                    let shift = ctx.input(|i| i.modifiers.shift);
                    with(|s| {
                        if let Some(s) = s {
                            let poly = s.poly().clone();
                            let mode = s.sel.mode;
                            s.sel.mode = studio_modeling::selection::Mode::Face;
                            match hit {
                                Some(f) => s.sel.pick(&poly, studio_modeling::selection::Elem::Face(f as u32), if shift { studio_modeling::selection::Combine::Toggle } else { studio_modeling::selection::Combine::Set }),
                                None if !shift => s.sel.clear(),
                                None => {}
                            }
                            s.sel.mode = mode;
                            s.sel.sync(&poly);
                        }
                    });
                }
                if resp.dragged() {
                    let d = resp.drag_delta();
                    let du = [d.x / side, -d.y / side];
                    with(|s| {
                        if let Some(s) = s {
                            let picked = s.sel.faces.clone();
                            let poly = s.poly_mut();
                            for f in 0..poly.faces() {
                                if picked.get(f).copied().unwrap_or(false) {
                                    for k in poly.face_range(f) {
                                        poly.uvs[k][0] += du[0];
                                        poly.uvs[k][1] += du[1];
                                    }
                                }
                            }
                            s.changed();
                        }
                    });
                    ctx.data_mut(|dd| dd.insert_temp(drag_id, true));
                }
                if resp.drag_stopped() && ctx.data(|dd| dd.get_temp::<bool>(drag_id)).unwrap_or(false) {
                    ctx.data_mut(|dd| dd.remove::<bool>(drag_id));
                    note(&ctx, super::commit(app, "Move UVs"));
                }
            }
        }
        if editing {
            ui.horizontal_wrapped(|ui| {
                for (label, tid, key, val) in [("Unwrap", "unwrap", "method", "seams"), ("Smart unwrap", "unwrap", "method", "smart"), ("Project from view", "project_uvs", "kind", "view")] {
                    if kit::Button::ghost(label).compact().show(ui).clicked() {
                        let mut a = Map::new();
                        a.insert(key.into(), json!(val));
                        run = Some((tid, a));
                    }
                }
                for (label, tid) in [("Pack", "pack_uvs"), ("Reset", "reset_uvs"), ("Mark seam", "mark_seam")] {
                    if kit::Button::ghost(label).compact().show(ui).clicked() {
                        run = Some((tid, Map::new()));
                    }
                }
                for (label, how) in [("Rotate 90°", 0), ("Flip", 1), ("Scale up", 2), ("Scale down", 3)] {
                    if kit::Button::ghost(label).compact().show(ui).clicked() {
                        uv_transform(how);
                        note(&ctx, super::commit(app, label));
                    }
                }
            });
        } else {
            ui.label(egui::RichText::new(crate::i18n::t("Open the model in edit mode (Tab) to unwrap and move pieces.")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        }
    });
    if let Some((tid, a)) = run {
        let _ = super::run(app, &ctx, tid, a);
    }
}

fn inside_uv(poly: &[[f32; 2]], q: [f32; 2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Turns, flips or scales the picked faces' layout about its middle.
fn uv_transform(how: u8) {
    with(|s| {
        let Some(s) = s else { return };
        let picked = s.sel.faces.clone();
        let poly = s.poly_mut();
        if !poly.has_uvs() {
            return;
        }
        let ks: Vec<usize> = (0..poly.faces()).filter(|f| picked.get(*f).copied().unwrap_or(false)).flat_map(|f| poly.face_range(f)).collect();
        if ks.is_empty() {
            return;
        }
        let c = ks.iter().fold([0.0f32; 2], |a, &k| [a[0] + poly.uvs[k][0], a[1] + poly.uvs[k][1]]);
        let c = [c[0] / ks.len() as f32, c[1] / ks.len() as f32];
        for k in ks {
            let u = [poly.uvs[k][0] - c[0], poly.uvs[k][1] - c[1]];
            let v = match how {
                0 => [-u[1], u[0]],
                1 => [-u[0], u[1]],
                2 => [u[0] * 1.25, u[1] * 1.25],
                _ => [u[0] * 0.8, u[1] * 0.8],
            };
            poly.uvs[k] = [v[0] + c[0], v[1] + c[1]];
        }
        s.changed();
    });
}

fn object_section(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, asset: &Asset, part: usize) {
    let ctx = ui.ctx().clone();
    let id = mc.id.0;
    enum Act {
        Smooth(bool),
        Angle(f32),
        Origin(&'static str),
        Apply,
        Join,
        Keep,
    }
    let mut act: Option<Act> = None;
    let p = &asset.parts[part];
    Section::new("model-object", "Shading and object").icon(Icon::Shapes).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if kit::Button::ghost("Shade smooth").compact().show(ui).clicked() {
                act = Some(Act::Smooth(true));
            }
            if kit::Button::ghost("Shade flat").compact().show(ui).clicked() {
                act = Some(Act::Smooth(false));
            }
        });
        kit::labeled_row(ui, "Auto smooth", |ui| {
            let mut a = p.smooth_angle;
            if kit::Slider::new("Auto smooth", &mut a, 0.0..=180.0).unit("°").with_value().show(ui).on_hover_text(crate::i18n::t("Sharper edges shade hard; marked ones always do")).changed() {
                act = Some(Act::Angle(a));
            }
        });
        kit::menu_separator(ui);
        ui.horizontal_wrapped(|ui| {
            for (label, how, tip) in [("Origin to geometry", "geometry", "Moves the origin to the shape's middle"), ("Origin to bottom", "bottom", "Moves the origin to the base's middle"), ("Origin to 3D cursor", "cursor", "Moves the origin to the 3D cursor")] {
                if kit::Button::ghost(label).compact().show(ui).on_hover_text(crate::i18n::t(tip)).clicked() {
                    act = Some(Act::Origin(how));
                }
            }
            if kit::Button::ghost("Apply rotation and scale").compact().show(ui).on_hover_text(crate::i18n::t("Bakes rotation and scale into shape")).clicked() {
                act = Some(Act::Apply);
            }
            let others = app.editor.selection.iter().filter(|s| **s as u64 != id).count();
            if others > 0 && kit::Button::ghost("Join selected").compact().show(ui).on_hover_text(crate::i18n::t("Joins the other selected models into this one")).clicked() {
                act = Some(Act::Join);
            }
            if kit::Button::secondary("Keep in My Models").icon(Icon::DownloadSimple).compact().show(ui).on_hover_text(crate::i18n::t("Saves to your models, for any project")).clicked() {
                act = Some(Act::Keep);
            }
        });
    });
    let Some(a) = act else { return };
    let r = match a {
        Act::Smooth(on) => edit_source(app, id, if on { "Shade smooth" } else { "Shade flat" }, |a| {
            for f in &mut a.parts[part].mesh.face_smooth {
                *f = on;
            }
            if on && a.parts[part].smooth_angle < 1.0 {
                a.parts[part].smooth_angle = 180.0;
            }
            Ok(())
        }),
        Act::Angle(v) => edit_source(app, id, "Auto smooth", |a| {
            a.parts[part].smooth_angle = v;
            Ok(())
        }),
        Act::Origin(how) => batch(app, "Set origin", |app| set_origin(app, id, how)),
        Act::Apply => batch(app, "Apply rotation and scale", |app| apply_transform(app, id)),
        Act::Join => batch(app, "Join", |app| join(app, id)),
        Act::Keep => keep(app, mc),
    };
    note(&ctx, r);
}

/// Runs `f` as one undo step (or none, when it fails).
pub fn batch(app: &mut StudioApp, label: &str, f: impl FnOnce(&mut StudioApp) -> Result<(), String>) -> Result<(), String> {
    let mine = !app.doc.in_batch();
    if mine {
        app.doc.begin_batch(label);
    }
    let r = f(app);
    if mine {
        if r.is_ok() { app.doc.commit_batch() } else { app.doc.rollback_batch() }
    }
    r
}

/// Moves the origin: the shape shifts one way inside the object and the
/// object the other, so nothing moves on screen.
pub fn set_origin(app: &mut StudioApp, id: u64, how: &str) -> Result<(), String> {
    let mc = app.doc.model.active().clip(m::ClipId(id)).cloned().ok_or("gone")?;
    let (a, _) = asset_of(app, &mc).ok_or("no model")?;
    let mesh = a.to_mesh(true)?;
    let (lo, hi) = mesh.bounds();
    let c: V3 = match how {
        "bottom" => [(lo[0] + hi[0]) / 2.0, hi[1], (lo[2] + hi[2]) / 2.0],
        "cursor" => {
            let ws = crate::editor::viewport::snapshot();
            let w = app.doc.project.width as f64;
            let seq = app.doc.model.active();
            let mx = crate::editor::viewport::model_matrix(&studio_model::space::oriented_transform(seq, &mc, super::draw::time_of(app).clamp(mc.start, mc.start + mc.duration - 1), w));
            let inv = super::draw::invert(&mx).ok_or("this model is scaled flat")?;
            std::array::from_fn(|r| (inv[r][0] * ws.cursor[0] + inv[r][1] * ws.cursor[1] + inv[r][2] * ws.cursor[2] + inv[r][3]) as f32)
        }
        _ => [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0],
    };
    let tr = &mc.transform;
    let anchor = [tr.anchor.x as f32, tr.anchor.y as f32, tr.anchor_z as f32];
    let shift = studio_mesh::sub(c, anchor);
    // The object moves by the shift as the object turns and scales it.
    let mx = crate::editor::viewport::model_matrix(tr);
    let d: [f64; 3] = std::array::from_fn(|r| mx[r][0] * shift[0] as f64 + mx[r][1] * shift[1] as f64 + mx[r][2] * shift[2] as f64);
    edit_source(app, id, "Set origin", |a| {
        for p in &mut a.parts {
            p.position = studio_mesh::sub(p.position, shift);
        }
        Ok(())
    })?;
    let seq = app.doc.seq();
    if let Some(mut c2) = app.doc.model.active().clip(m::ClipId(id)).cloned() {
        c2.transform.position = m::Vec2::new(c2.transform.position.x + d[0], c2.transform.position.y + d[1]);
        c2.transform.position_z += d[2];
        let _ = app.doc.apply("Set origin", m::Edit::SetClip { seq, clip: Box::new(c2) });
    }
    Ok(())
}

/// Bakes the object's turn and scale into the shape (the object is left
/// unturned at scale 1, in the same place).
pub fn apply_transform(app: &mut StudioApp, id: u64) -> Result<(), String> {
    let mc = app.doc.model.active().clip(m::ClipId(id)).cloned().ok_or("gone")?;
    let tr = mc.transform.clone();
    let mx = crate::editor::viewport::model_matrix(&tr);
    let lin = |p: V3| -> V3 { std::array::from_fn(|r| (mx[r][0] * p[0] as f64 + mx[r][1] * p[1] as f64 + mx[r][2] * p[2] as f64) as f32) };
    let det = mx[0][0] * (mx[1][1] * mx[2][2] - mx[1][2] * mx[2][1]) - mx[0][1] * (mx[1][0] * mx[2][2] - mx[1][2] * mx[2][0]) + mx[0][2] * (mx[1][0] * mx[2][1] - mx[1][1] * mx[2][0]);
    let anchor = [tr.anchor.x as f32, tr.anchor.y as f32, tr.anchor_z as f32];
    edit_source(app, id, "Apply rotation and scale", |a| {
        for p in &mut a.parts {
            let place = p.clone();
            p.mesh.map_positions(|v| lin(studio_mesh::sub(place.place(v), anchor)));
            if det < 0.0 {
                for f in 0..p.mesh.faces() {
                    p.mesh.flip_face(f);
                }
            }
            p.position = [0.0; 3];
            p.rotation = [0.0; 3];
            p.scale = [1.0; 3];
        }
        Ok(())
    })?;
    let seq = app.doc.seq();
    if let Some(mut c2) = app.doc.model.active().clip(m::ClipId(id)).cloned() {
        let t = &mut c2.transform;
        (t.rotation_x, t.rotation_y, t.rotation, t.orientation) = (0.0, 0.0, 0.0, [0.0; 3]);
        (t.scale_x, t.scale_y, t.scale_z, t.flip_x, t.flip_y) = (1.0, 1.0, 1.0, false, false);
        (t.anchor, t.anchor_z) = (m::Vec2::new(0.0, 0.0), 0.0);
        let _ = app.doc.apply("Apply rotation and scale", m::Edit::SetClip { seq, clip: Box::new(c2) });
    }
    Ok(())
}

/// Joins the other selected models into this one (their parts in its
/// space); they are removed.
pub fn join(app: &mut StudioApp, id: u64) -> Result<(), String> {
    let seq = app.doc.model.active();
    let me = seq.clip(m::ClipId(id)).cloned().ok_or("gone")?;
    let w = app.doc.project.width as f64;
    let t = super::draw::time_of(app);
    let mx_me = crate::editor::viewport::model_matrix(&studio_model::space::oriented_transform(seq, &me, t.clamp(me.start, me.start + me.duration - 1), w));
    let inv = super::draw::invert(&mx_me).ok_or("this model is scaled flat")?;
    let mut extra: Vec<(Asset, [[f64; 4]; 3])> = Vec::new();
    let others: Vec<u64> = app.editor.selection.iter().map(|s| *s as u64).filter(|s| *s != id).collect();
    for o in &others {
        let Some(c) = seq.clip(m::ClipId(*o)) else { continue };
        if crate::mesh_store::file_of(c).is_none() {
            continue;
        }
        let mx = crate::editor::viewport::model_matrix(&studio_model::space::oriented_transform(seq, c, t.clamp(c.start, c.start + c.duration - 1), w));
        let (a, _) = super::load_source(c)?;
        extra.push((a, mx));
    }
    if extra.is_empty() {
        return Err("select other 3D models to join into this one".into());
    }
    let joined: Vec<u64> = others.clone();
    edit_source(app, id, "Join", |a| {
        for (other, mx) in extra {
            for mut p in other.parts {
                let place = p.clone();
                p.mesh.map_positions(|v| {
                    let q = place.place(v);
                    let wp: [f64; 3] = std::array::from_fn(|r| mx[r][0] * q[0] as f64 + mx[r][1] * q[1] as f64 + mx[r][2] * q[2] as f64 + mx[r][3]);
                    std::array::from_fn(|r| (inv[r][0] * wp[0] + inv[r][1] * wp[1] + inv[r][2] * wp[2] + inv[r][3]) as f32)
                });
                p.position = [0.0; 3];
                p.rotation = [0.0; 3];
                p.scale = [1.0; 3];
                p.name = a.unique_name(&p.name);
                a.parts.push(p);
            }
            for m in other.materials {
                if !a.materials.iter().any(|x| x.name == m.name) {
                    a.materials.push(m);
                }
            }
        }
        Ok(())
    })?;
    let seqid = app.doc.seq();
    let edits: Vec<m::Edit> = joined.iter().map(|c| m::Edit::RemoveClip { seq: seqid, clip: m::ClipId(*c) }).collect();
    let _ = app.doc.apply("Join", m::Edit::Batch { edits });
    app.editor.selection = vec![id as u32];
    Ok(())
}

/// Keeps the model in the person's own models.
fn keep(app: &StudioApp, mc: &m::Clip) -> Result<(), String> {
    let (a, _) = asset_of(app, mc).ok_or("no model")?;
    let mesh = a.to_mesh(false)?;
    let saved = crate::my_models::keep(&mc.name, &mesh, None)?;
    // The source goes with it, so it opens in edit mode as modelled.
    let tmp = std::env::temp_dir().join(format!("studio-keep-{}.smodel", std::process::id()));
    studio_modeling::asset::save(&a, &tmp)?;
    let to = crate::my_models::root().join(&saved.slug).join(format!("source-{}.smodel", saved.hash));
    std::fs::copy(&tmp, &to).map_err(|e| format!("{}: {e}", to.display()))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}
