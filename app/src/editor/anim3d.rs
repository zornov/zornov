// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's 3D animation, simulation and rendering sections:
//! - on a model: "Character" (rig it, play a walk, a run, a wave…, face
//!   expressions keyed at the playhead) and "Simulation" (drop it, shatter
//!   it, make it wobble, grow hair on it, add particles, smoke or fire);
//! - on a camera: "Render" (path tracing, samples, motion blur);
//! - on a sky light: "Sky" (time of day, haze, clearness);
//! - on any 3D selection: "World" (add a sky, terrain with trees).
//!
//! Every button runs the same op the AI uses (`rig`, `act`, `pose`,
//! `simulate`, `render3d`, `world`), as one undo step.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, Section};
use crate::theme;
use crate::view::Clip;
use eframe::egui;
use serde_json::{Value, json};
use studio_model as m;

/// Runs agent ops on the open document (one undo step); says what failed.
fn run(app: &mut StudioApp, ctx: &egui::Context, ops: Vec<Value>, label: &str) {
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let r = s.edit(&ops, label, false);
    app.doc = s.doc;
    if let Err(e) = r {
        kit::push(ctx, format!("{label}: {e}"), None, Some(Icon::Warning));
    }
}

fn model_clip(app: &StudioApp, id: u32) -> Option<m::Clip> {
    app.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).cloned()
}

fn generator(c: &m::Clip) -> Option<&m::Generator> {
    match &c.source {
        m::ClipSource::Generator(g) => Some(g),
        _ => None,
    }
}

fn num(g: &m::Generator, k: &str, d: f64) -> f64 {
    g.params.get(k).and_then(|v| v.as_f64()).unwrap_or(d)
}

#[derive(Clone, Copy)]
struct Lip {
    intensity: f32,
    offset: f32,
}

#[derive(Clone, Copy)]
struct Face {
    smile: f32,
    jaw: f32,
    brows: f32,
}

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], _c: &Clip) {
    let [id] = ids else { return };
    let Some(mc) = model_clip(app, *id) else { return };
    let Some(g) = generator(&mc).cloned() else { return };
    let ctx = ui.ctx().clone();
    let t = theme::tokens(ui.ctx());
    let playhead = (app.editor.playhead - mc.start).max(0) as f64 / 1e6;
    match g.kind {
        m::GeneratorKind::Model => {
            let rig = crate::mesh_store::load(&mc).ok().and_then(|mesh| mesh.rig.clone());
            Section::new("character3d", "Character").icon(Icon::Sparkle).show(ui, |ui| match &rig {
                None => {
                    ui.label(egui::RichText::new("Add bones to animate it; they fit the model and skin binds itself").font(theme::caption()).color(t.text_2));
                    ui.horizontal_wrapped(|ui| {
                        for (label, template) in [("Rig as person", "humanoid"), ("Rig as animal", "quadruped"), ("Rig as chain", "chain")] {
                            if Button::secondary(label).show(ui).clicked() {
                                run(app, &ctx, vec![json!({"op": "rig", "clip": id, "template": template})], "Rig");
                            }
                        }
                    });
                }
                Some(r) => {
                    ui.label(egui::RichText::new(format!("{} bones, {} shape keys{}", r.bones.len(), r.shapes.len(), if r.actions.is_empty() { String::new() } else { format!(", actions: {}", r.actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")) })).font(theme::caption()).color(t.text_2));
                    ui.horizontal_wrapped(|ui| {
                        for motion in ["walk", "run", "idle", "wave", "jump", "dance", "talk"] {
                            let mut label = motion.to_owned();
                            if let Some(c) = label.get_mut(0..1) {
                                c.make_ascii_uppercase();
                            }
                            if Button::secondary(&label).show(ui).clicked() {
                                run(app, &ctx, vec![json!({"op": "act", "clip": id, "motion": motion, "at": playhead})], "Motion");
                            }
                        }
                        if Button::secondary("Stop").show(ui).clicked() {
                            run(app, &ctx, vec![json!({"op": "act", "clip": id, "stop": true})], "Stop motion");
                        }
                    });
                    if r.shape("mouthSmileLeft").is_some() {
                        let key = egui::Id::unique(("face3d", *id));
                        let mut f = ui.data(|d| d.get_temp::<Face>(key)).unwrap_or(Face { smile: 0.0, jaw: 0.0, brows: 0.0 });
                        let a = FieldRow::new("Smile", &mut f.smile, 0.0..=100.0).unit("%").show(ui);
                        let b = FieldRow::new("Mouth open", &mut f.jaw, 0.0..=100.0).unit("%").show(ui);
                        let c = FieldRow::new("Brows up", &mut f.brows, 0.0..=100.0).unit("%").show(ui);
                        if (a.changed || b.changed || c.changed) && !(a.dragging || b.dragging || c.dragging) {
                            run(app, &ctx, vec![json!({"op": "pose", "clip": id, "t": playhead, "shapes": {"smile": f.smile / 100.0, "jawOpen": f.jaw / 100.0, "browInnerUp": f.brows / 100.0}})], "Expression");
                        }
                        ui.data_mut(|d| d.insert_temp(key, f));
                    }
                    // Speech to mouth: the same op the AI uses; intensity and offset re-lay the keys.
                    let talks = r.shape("jawOpen").is_some() || r.shapes.iter().any(|k| k.name.to_lowercase().contains("viseme")) || r.bones.iter().any(|b| b.name.to_lowercase().contains("jaw"));
                    if talks {
                        let key = egui::Id::unique(("lipsync3d", *id));
                        let saved = |k: &str, d: f64| g.params.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
                        let mut l = ui.data(|d| d.get_temp::<Lip>(key)).unwrap_or(Lip { intensity: (saved("lipsync.intensity", 1.0) * 100.0) as f32, offset: (saved("lipsync.offset", 0.0) * 1000.0) as f32 });
                        let a = FieldRow::new("Lip sync intensity", &mut l.intensity, 0.0..=200.0).unit("%").show(ui);
                        let b = FieldRow::new("Lip sync offset", &mut l.offset, -300.0..=300.0).unit("ms").show(ui);
                        let op = json!({"op": "lip_sync", "clip": id, "intensity": l.intensity / 100.0, "offset": l.offset / 1000.0});
                        let dragging = a.dragging || b.dragging;
                        ui.horizontal_wrapped(|ui| {
                            if Button::secondary("Sync mouth to voice").show(ui).clicked() || ((a.changed || b.changed) && !dragging && mc.keyframes.keys().any(|k| k.starts_with("generator.shape."))) {
                                run(app, &ctx, vec![op.clone()], "Lip sync");
                            }
                            if Button::secondary("Check mouth").show(ui).clicked() {
                                let mut chk = op.clone();
                                chk["check"] = json!(true);
                                let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
                                let mut sess = crate::agent::Session::new(doc);
                                let r = sess.edit(&[chk], "Check mouth", false);
                                app.doc = sess.doc;
                                kit::push(&ctx, match r { Ok(v) => v.to_string(), Err(e) => format!("Check mouth: {e}") }, None, Some(Icon::Sparkle));
                            }
                        });
                        ui.data_mut(|d| d.insert_temp(key, l));
                    }
                }
            });
            Section::new("sim3d", "Simulation").icon(Icon::ArrowsOutCardinal).show(ui, |ui| {
                let at = playhead + mc.start as f64 / 1e6;
                ui.horizontal_wrapped(|ui| {
                    if Button::secondary("Drop").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "rigid", "clips": [id], "at": at})], "Drop");
                    }
                    if Button::secondary("Shatter here").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "shatter", "clip": id, "at": playhead})], "Shatter");
                    }
                    if Button::secondary("Jelly").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "soft", "clip": id, "at": at})], "Soft body");
                    }
                    if Button::secondary("Hair on top").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "hair", "clip": id, "region": "top"})], "Hair");
                    }
                    if rig.is_some() && Button::secondary("Cape").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "cloth", "make": "cape", "on": id})], "Cape");
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for preset in ["sparks", "snow", "rain", "confetti", "embers"] {
                        if Button::secondary(preset).show(ui).clicked() {
                            run(app, &ctx, vec![json!({"op": "simulate", "kind": "particles", "preset": preset, "at": at, "colliders": [id]})], "Particles");
                        }
                    }
                    if Button::secondary("Smoke").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "smoke", "at": at})], "Smoke");
                    }
                    if Button::secondary("Fire").show(ui).clicked() {
                        run(app, &ctx, vec![json!({"op": "simulate", "kind": "fire", "at": at})], "Fire");
                    }
                });
                ui.label(egui::RichText::new("Runs once and is kept: drops become keys, the rest a cache the clip plays, so scrubbing never simulates again.").font(theme::caption()).color(t.text_2));
            });
            world_section(app, ui, &ctx);
        }
        m::GeneratorKind::Camera => {
            Section::new("render3d", "Render").icon(Icon::Sun).show(ui, |ui| {
                let mut path = matches!(g.params.get("render"), Some(m::Value::Text(s)) if s == "path");
                let mut samples = num(&g, "samples", 64.0) as f32;
                let mut shutter = num(&g, "shutter", 0.0) as f32 * 360.0;
                let mut changed = false;
                kit::labeled_row(ui, "Path tracing", |ui| {
                    changed |= kit::toggle(ui, &mut path, "Light that bounces (final quality)", None).changed();
                });
                let a = FieldRow::new("Samples", &mut samples, 1.0..=1024.0).default(64.0).show(ui);
                let b = FieldRow::new("Shutter", &mut shutter, 0.0..=360.0).default(0.0).unit("°").show(ui);
                changed |= (a.changed && !a.dragging) || (b.changed && !b.dragging);
                if changed {
                    run(app, &ctx, vec![json!({"op": "render3d", "camera": id, "mode": if path { "path" } else { "raster" }, "samples": samples.round(), "motion_blur": shutter / 360.0})], "Render settings");
                }
                ui.label(egui::RichText::new("Path tracing: soft shadows, bounced light, glass, glow; previews use fewer samples").font(theme::caption()).color(t.text_2));
            });
            world_section(app, ui, &ctx);
        }
        m::GeneratorKind::Light if matches!(g.params.get("sky"), Some(m::Value::Bool(true))) => {
            Section::new("sky3d", "Sky").icon(Icon::Sun).show(ui, |ui| {
                let mut time = num(&g, "time", 14.0) as f32;
                let mut haze = num(&g, "haze", 0.0) as f32 * 100.0;
                let mut turbidity = num(&g, "turbidity", 3.0) as f32;
                let a = FieldRow::new("Time of day", &mut time, 0.0..=24.0).default(14.0).unit("h").decimals(1).step(0.1).show(ui);
                let b = FieldRow::new("Haze", &mut haze, 0.0..=200.0).default(0.0).unit("%").show(ui);
                let c = FieldRow::new("Hazy air", &mut turbidity, 1.7..=10.0).default(3.0).decimals(1).show(ui);
                if (a.changed || b.changed || c.changed) && !(a.dragging || b.dragging || c.dragging) {
                    run(app, &ctx, vec![json!({"op": "world", "sky": {"time": time, "haze": haze / 100.0, "turbidity": turbidity}})], "Sky");
                }
            });
        }
        _ => {}
    }
}

fn world_section(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    Section::new("world3d", "World").icon(Icon::Sun).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if Button::secondary("Sky at golden hour").show(ui).clicked() {
                run(app, ctx, vec![json!({"op": "world", "sky": {"time": "golden_hour", "haze": 0.3}})], "Sky");
            }
            if Button::secondary("Hills with trees").show(ui).clicked() {
                run(app, ctx, vec![json!({"op": "world", "terrain": {"style": "hills"}, "scatter": [{"kind": "pine", "count": 150}, {"kind": "tree", "count": 60}, {"kind": "rock", "count": 60}, {"kind": "grass", "count": 1200}]})], "Terrain");
            }
            if Button::secondary("Mountains").show(ui).clicked() {
                run(app, ctx, vec![json!({"op": "world", "terrain": {"style": "mountains"}, "scatter": [{"kind": "pine", "count": 200, "max_height": 0.55}]})], "Terrain");
            }
            if Button::secondary("Ocean").show(ui).clicked() {
                run(app, ctx, vec![json!({"op": "simulate", "kind": "ocean"})], "Ocean");
            }
        });
    });
}
