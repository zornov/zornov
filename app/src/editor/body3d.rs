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
use super::physics::cloud_feedback as feedback;
use crate::physics3d::{BodyType, Props};
use crate::theme;
use eframe::egui;
use serde_json::{Value, json};
use studio_model as m;
use studio_sim::dynamics::decompose::Fit;
use studio_sim::dynamics::material::TABLE;

/// Runs `ops` as one undo step; says why when they can't run.
fn run(app: &mut StudioApp, ctx: &egui::Context, ops: Vec<Value>, label: &str) -> Option<String> {
    let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
    let mut s = crate::agent::Session::new(doc);
    let r = s.edit(&ops, label, false);
    app.doc = s.doc;
    match r {
        Ok(v) => Some(v.to_string()),
        Err(e) => {
            feedback::failed(ctx, &app.doc, &e);
            None
        }
    }
}

/// Runs an action and says what it did.
fn act(app: &mut StudioApp, ctx: &egui::Context, op: Value, label: &str, said: &str) {
    if run(app, ctx, vec![op], label).is_some() {
        feedback::done(ctx, said);
    }
}

const BODIES: [&str; 5] = ["Auto", "Moves", "Fixed", "Animated", "Not physical"];
const FITS: [(&str, Fit); 7] = [("Auto", Fit::Auto), ("Box", Fit::Box), ("Ball", Fit::Sphere), ("Capsule", Fit::Capsule), ("Convex hull", Fit::Hull), ("Convex parts", Fit::Parts(16)), ("Exact mesh", Fit::Mesh)];

/// The joints as a person reads them: "Hinge to the world, rope to “Lamp”".
fn joints_said(app: &StudioApp, p: &Props) -> String {
    let list: Vec<String> = p
        .joints
        .iter()
        .map(|j| {
            let to = j.to.map_or("the world".to_owned(), |t| app.doc.project.clip(t as u32).map(|c| c.name.trim().to_owned()).filter(|n| !n.is_empty()).map_or("another clip".to_owned(), |n| format!("“{n}”")));
            format!("{} to {to}", j.kind)
        })
        .collect();
    let s = list.join(", ");
    let mut c = s.chars();
    c.next().map_or(s.clone(), |f| f.to_uppercase().chain(c).collect())
}

/// The note under the section: what simulating does, or what was baked.
fn baked_note(baked: Option<&str>) -> String {
    match baked {
        Some(r) => crate::i18n::tf(format!("Baked {} s. Simulate again to redo", r.replace(',', " to "))),
        None => crate::i18n::shown("Simulations bake into keys you can scrub").into_owned(),
    }
}

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
            if let Some(i) = kit::menu_button(ui, "physics-material", cur, &names, w, None) {
                if i > 0 {
                    run(app, &ctx, vec![json!({"op": "body", "clip": id, "material": names[i]})], "Physical material");
                } else if p.material.is_some() {
                    // Back to the material its look suggests (the op only sets one).
                    let mut c = mc.clone();
                    let mut q = Props::of(&c);
                    q.material = None;
                    q.write(&mut c);
                    let seq = app.doc.seq();
                    app.doc.apply("Physical material", m::Edit::SetClip { seq, clip: Box::new(c) });
                }
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
            kit::labeled_row(ui, "Joints", |ui| {
                ui.label(egui::RichText::new(joints_said(app, &p)).font(theme::body()).color(t.text));
            });
        }
        ui.horizontal_wrapped(|ui| {
            if Button::secondary("Hinge at top").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "joint", "clip": id, "kind": "hinge", "axis": [1, 0, 0]}), "Hinge", "Hinged at the top");
            }
            if Button::secondary("Hang on a rope").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "joint", "clip": id, "kind": "rope"}), "Rope", "Hung on a rope");
            }
            if !p.joints.is_empty() && Button::ghost("Remove joints").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "joint", "clip": id, "remove": true}), "Remove joints", "Joints removed");
            }
        });
        kit::menu_separator(ui);
        // Actions from the playhead.
        ui.horizontal_wrapped(|ui| {
            if Button::secondary("Drop").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "drop", "clip": id, "at": playhead}), "Drop", "Dropped from the playhead");
            }
            if Button::secondary("Settle").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "settle", "clip": id, "at": playhead}), "Settle", "Settled at the playhead");
            }
            if Button::secondary("Push").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "push", "clip": id, "at": playhead, "direction": [1, 0, 0]}), "Push", "Pushed from the playhead");
            }
            if Button::secondary("Break here").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "break", "clip": id, "at": playhead}), "Break", "Broken at the playhead");
            }
            if rigged && Button::secondary("Ragdoll").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "ragdoll", "clip": id, "at": playhead}), "Ragdoll", "Ragdoll from the playhead");
            }
            if rigged && Button::secondary("Secondary motion").compact().show(ui).clicked() {
                act(app, &ctx, json!({"op": "jiggle", "clip": id}), "Secondary motion", "Secondary motion added");
            }
        });
        let key = egui::Id::new(("physics-scene-len", id));
        let mut secs: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(5.0);
        let r = FieldRow::new("Simulate for", &mut secs, 0.5..=60.0).default(5.0).unit("s").decimals(1).show(ui);
        if r.changed {
            ui.data_mut(|d| d.insert_temp(key, secs));
        }
        ui.horizontal(|ui| {
            if Button::primary("Simulate scene").icon(Icon::Play).compact().show(ui).clicked() && run(app, &ctx, vec![json!({"op": "bake_physics", "at": playhead, "duration": secs})], "Bake physics").is_some() {
                feedback::done(&ctx, simulated(secs as f64));
            }
        });
        let baked = match &mc.source {
            m::ClipSource::Generator(g) => match g.params.get("physics.baked") {
                Some(m::Value::Text(s)) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        };
        ui.label(egui::RichText::new(baked_note(baked.as_deref())).font(theme::caption()).color(t.text_2));
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
    let secs = (end - at).clamp(0.5, 60.0);
    if run(app, ctx, vec![json!({"op": "bake_physics", "at": at, "duration": secs})], "Bake physics").is_some() {
        feedback::done(ctx, simulated(secs));
    }
}

/// What a finished simulation tells the person.
fn simulated(secs: f64) -> String {
    crate::i18n::tf(format!("Simulated {secs:.1} s from the playhead, baked into keys"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_physics_notes_read_plainly() {
        assert_eq!(baked_note(Some("1.5,6.5")), "Baked 1.5 to 6.5 s. Simulate again to redo");
        for s in [baked_note(None), simulated(5.0), BODIES.join(" "), FITS.iter().map(|f| f.0).collect::<Vec<_>>().join(" ")] {
            assert!(!s.ends_with('.') && !s.contains('('), "{s}");
        }
        for b in BODIES.iter().chain(FITS.iter().map(|f| &f.0)) {
            assert_eq!(crate::editor::physics::cloud_ui_check::problem(b), None, "{b}");
        }
    }
}
