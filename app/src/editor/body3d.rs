// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Physics" section on a 3D model: what it is physically
//! (body type, material, mass, collider, friction, bounce, breakable), its
//! joints, the everyday actions (drop, settle, push, break, ragdoll,
//! secondary motion) and simulating the whole scene from the playhead.
//! Every control runs the same op the AI uses (`body`, `joint`, `drop`,
//! `settle`, `push`, `break`, `ragdoll`, `jiggle`, `bake_physics`), one
//! undo step each.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, Section};
use crate::physics3d::{BodyType, Props};
use crate::theme;
use eframe::egui;
use serde_json::{Value, json};
use studio_model as m;
use studio_sim::dynamics::decompose::Fit;
use studio_sim::dynamics::material::TABLE;

fn run(app: &mut StudioApp, ctx: &egui::Context, ops: Vec<Value>, label: &str) -> Option<String> {
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let r = s.edit(&ops, label, false);
    app.doc = s.doc;
    match r {
        Ok(v) => Some(v.to_string()),
        Err(e) => {
            kit::push(ctx, format!("{label}: {e}"), None, Some(Icon::Warning));
            None
        }
    }
}

const BODIES: [&str; 5] = ["Auto", "Moves (dynamic)", "Fixed (static)", "Animated (kinematic)", "Not physical"];
const FITS: [(&str, Fit); 7] = [("Auto", Fit::Auto), ("Box", Fit::Box), ("Ball", Fit::Sphere), ("Capsule", Fit::Capsule), ("Convex hull", Fit::Hull), ("Convex parts", Fit::Parts(16)), ("Exact triangles", Fit::Mesh)];

pub fn section(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).cloned() else { return };
    if !crate::physics3d::scene::is_model(&mc) {
        return;
    }
    let ctx = ui.ctx().clone();
    let t = theme::tokens(ui.ctx());
    let p = Props::of(&mc);
    let playhead = app.editor.playhead.max(0) as f64 / 1e6;
    let rigged = crate::physics3d::scene::load_mesh(&mc).is_some_and(|mm| mm.rig.as_ref().is_some_and(|r| r.is_skinned()));
    Section::new("physics3d", "Physics").icon(Icon::ArrowsOutCardinal).show(ui, |ui| {
        let w = ui.available_width() - kit::LABEL_W - 6.0;
        // Body type.
        let cur = BodyType::ALL.iter().position(|b| *b == p.body).unwrap_or(0);
        kit::labeled_row(ui, "Body", |ui| {
            if let Some(i) = kit::menu_button(ui, "physics-body", cur, &BODIES, w, None) {
                run(app, &ctx, vec![json!({"op": "body", "clip": id, "type": BodyType::ALL[i].name()})], "Body type");
            }
        });
        // Material.
        let mut names: Vec<&str> = vec!["From its look"];
        names.extend(TABLE.iter().map(|s| s.name));
        let cur = p.material.as_deref().and_then(|m| names.iter().position(|n| *n == m)).unwrap_or(0);
        kit::labeled_row(ui, "Material", |ui| {
            if let Some(i) = kit::menu_button(ui, "physics-material", cur, &names, w, None)
                && i > 0
            {
                run(app, &ctx, vec![json!({"op": "body", "clip": id, "material": names[i]})], "Physical material");
            }
        });
        let mut mass = p.mass.unwrap_or(0.0);
        let out = FieldRow::new("Mass", &mut mass, 0.0..=10_000.0).default(0.0).unit("kg").decimals(2).show(ui);
        if out.changed && !out.dragging {
            run(app, &ctx, vec![json!({"op": "body", "clip": id, "mass": mass})], "Mass");
        }
        let cur = FITS.iter().position(|f| std::mem::discriminant(&f.1) == std::mem::discriminant(&p.fit)).unwrap_or(0);
        let labels: Vec<&str> = FITS.iter().map(|f| f.0).collect();
        kit::labeled_row(ui, "Collider", |ui| {
            if let Some(i) = kit::menu_button(ui, "physics-collider", cur, &labels, w, None) {
                run(app, &ctx, vec![json!({"op": "body", "clip": id, "collider": FITS[i].1.name()})], "Collider");
            }
        });
        let stuff = p.stuff(crate::physics3d::scene::load_mesh(&mc).as_deref(), &mc.name);
        let mut friction = p.friction.unwrap_or(stuff.friction);
        let a = FieldRow::new("Friction", &mut friction, 0.0..=2.0).default(stuff.friction).decimals(2).show(ui);
        if a.changed && !a.dragging {
            run(app, &ctx, vec![json!({"op": "body", "clip": id, "friction": friction})], "Friction");
        }
        let mut bounce = p.bounce.unwrap_or(stuff.bounce);
        let b = FieldRow::new("Bounce", &mut bounce, 0.0..=1.0).default(stuff.bounce).decimals(2).show(ui);
        if b.changed && !b.dragging {
            run(app, &ctx, vec![json!({"op": "body", "clip": id, "bounce": bounce})], "Bounce");
        }
        let mut breakable = p.is_breakable(&stuff);
        kit::labeled_row(ui, "Breakable", |ui| {
            if kit::toggle(ui, &mut breakable, "Shatters when hit hard", None).changed() {
                run(app, &ctx, vec![json!({"op": "body", "clip": id, "breakable": breakable})], "Breakable");
            }
        });
        // Joints.
        if !p.joints.is_empty() {
            let list: Vec<String> = p.joints.iter().map(|j| format!("{} to {}", j.kind, j.to.map_or("the world".to_string(), |t| format!("clip {t}")))).collect();
            ui.label(egui::RichText::new(format!("Joints: {}", list.join(", "))).font(theme::caption()).color(t.text_2));
        }
        ui.horizontal_wrapped(|ui| {
            if Button::secondary("Hinge at top").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "joint", "clip": id, "kind": "hinge", "axis": [1, 0, 0]})], "Hinge");
            }
            if Button::secondary("Hang on a rope").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "joint", "clip": id, "kind": "rope"})], "Rope");
            }
            if !p.joints.is_empty() && Button::ghost("Remove joints").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "joint", "clip": id, "remove": true})], "Remove joints");
            }
        });
        kit::menu_separator(ui);
        // Actions from the playhead.
        ui.horizontal_wrapped(|ui| {
            if Button::secondary("Drop").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "drop", "clip": id, "at": playhead})], "Drop");
            }
            if Button::secondary("Settle").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "settle", "clip": id, "at": playhead})], "Settle");
            }
            if Button::secondary("Push").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "push", "clip": id, "at": playhead, "direction": [1, 0, 0]})], "Push");
            }
            if Button::secondary("Break here").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "break", "clip": id, "at": playhead})], "Break");
            }
            if rigged && Button::secondary("Ragdoll").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "ragdoll", "clip": id, "at": playhead})], "Ragdoll");
            }
            if rigged && Button::secondary("Secondary motion").compact().show(ui).clicked() {
                run(app, &ctx, vec![json!({"op": "jiggle", "clip": id})], "Secondary motion");
            }
        });
        let key = egui::Id::new(("physics-scene-len", id));
        let mut secs: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(5.0);
        let r = FieldRow::new("Simulate for", &mut secs, 0.5..=60.0).default(5.0).unit("s").decimals(1).show(ui);
        if r.changed {
            ui.data_mut(|d| d.insert_temp(key, secs));
        }
        ui.horizontal(|ui| {
            if Button::primary("Simulate scene").icon(Icon::Play).compact().show(ui).clicked()
                && let Some(said) = run(app, &ctx, vec![json!({"op": "bake_physics", "at": playhead, "duration": secs})], "Bake physics")
            {
                kit::push(&ctx, said.trim_matches('"').to_string(), None, Some(Icon::Check));
            }
        });
        let baked = match &mc.source {
            m::ClipSource::Generator(g) => match g.params.get("physics.baked") {
                Some(m::Value::Text(s)) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        };
        ui.label(
            egui::RichText::new(match baked {
                Some(r) => format!("Baked to keys {} s: scrub freely; simulate again to redo it.", r.replace(',', " to ")),
                None => "Simulations bake into keys (and pieces for breaks): they play and scrub without simulating again, and the same scene always gives the same result.".into(),
            })
            .font(theme::caption())
            .color(t.text_2),
        );
    });
}

/// Whether anything in the project is set to move by physics.
pub fn scene_has_bodies(app: &StudioApp) -> bool {
    app.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).any(|c| Props::of(c).body == BodyType::Dynamic || !Props::of(c).joints.is_empty())
}

/// The whole scene simulated from the playhead to the end of the film
/// (at most a minute), baked.
pub fn bake_scene(app: &mut StudioApp, ctx: &egui::Context) {
    let at = app.editor.playhead.max(0) as f64 / 1e6;
    let end = app.doc.project.clips.iter().map(|c| (c.start + c.dur) as f64 / 1e6).fold(at + 1.0, f64::max);
    if let Some(said) = run(app, ctx, vec![json!({"op": "bake_physics", "at": at, "duration": (end - at).clamp(0.5, 60.0)})], "Bake physics") {
        kit::push(ctx, said.trim_matches('"').to_string(), None, Some(Icon::Check));
    }
}
