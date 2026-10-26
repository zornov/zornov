// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Simulation settings" section on a 3D model: pick a
//! simulation (drop, shatter, cloth, jelly, hair, particles, smoke, fire,
//! pond, ocean), set every parameter the AI's `simulate` op takes, and
//! bake it. The settings used are kept on the clip (`meta.sim`) so they can
//! be changed and baked again.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, FieldRow, Section};
use crate::theme;
use eframe::egui;
use serde_json::{Value, json};
use studio_model as m;

/// One number the op takes: key, label, unit, min, max, default.
#[derive(Clone, Copy)]
pub struct Num(pub &'static str, pub &'static str, pub &'static str, pub f32, pub f32, pub f32);

/// A choice the op takes: key, label, options (first is the default).
pub struct Pick(pub &'static str, pub &'static str, pub &'static [&'static str]);

/// A kind of simulation and its settings. `vectors` are x, y, z triples.
pub struct Sim {
    pub kind: &'static str,
    pub name: &'static str,
    pub nums: &'static [Num],
    pub picks: &'static [Pick],
    pub vectors: &'static [(&'static str, &'static str)],
}

const TIMING: [Num; 2] = [Num("at", "Start", "s", 0.0, 600.0, 0.0), Num("duration", "Length", "s", 0.1, 120.0, 3.0)];

pub const SIMS: &[Sim] = &[
    Sim {
        kind: "rigid",
        name: "Drop and bounce",
        nums: &[TIMING[0], TIMING[1], Num("gravity", "Gravity", "", 0.0, 8000.0, 2000.0), Num("bounce", "Bounce", "", 0.0, 1.0, 0.35), Num("friction", "Friction", "", 0.0, 2.0, 0.5), Num("detail", "Detail", "", 2.0, 32.0, 8.0)],
        picks: &[],
        vectors: &[("push", "Push"), ("spin", "Spin")],
    },
    Sim { kind: "shatter", name: "Shatter", nums: &[Num("at", "Break at", "s", 0.0, 600.0, 0.5), Num("duration", "Length", "s", 0.2, 30.0, 3.0), Num("pieces", "Pieces", "", 4.0, 200.0, 20.0), Num("strength", "Strength", "", 0.0, 3000.0, 350.0), Num("gravity", "Gravity", "", 0.0, 8000.0, 2000.0), Num("seed", "Seed", "", 0.0, 9999.0, 1.0)], picks: &[], vectors: &[] },
    Sim {
        kind: "cloth",
        name: "Cloth",
        nums: &[Num("at", "Start", "s", 0.0, 600.0, 0.0), Num("duration", "Length", "s", 0.2, 60.0, 4.0), Num("gusts", "Gusts", "", 0.0, 2.0, 0.4), Num("gravity", "Gravity", "", 0.0, 8000.0, 2000.0)],
        picks: &[Pick("make", "Make", &["cape", "flag", "curtain", "tablecloth", "banner"]), Pick("fabric", "Fabric", &["cotton", "silk", "denim", "leather", "wool"])],
        vectors: &[("wind", "Wind")],
    },
    Sim { kind: "soft", name: "Jelly", nums: &[TIMING[0], Num("duration", "Length", "s", 0.2, 60.0, 3.0), Num("stiffness", "Stiffness", "", 0.0, 1.0, 0.3), Num("squash", "Squash", "", 0.0, 1.0, 0.4), Num("damping", "Damping", "", 0.0, 1.0, 0.05), Num("clusters", "Clusters", "", 1.0, 16.0, 1.0), Num("gravity", "Gravity", "", 0.0, 8000.0, 2000.0)], picks: &[], vectors: &[] },
    Sim {
        kind: "hair",
        name: "Hair and fur",
        nums: &[Num("at", "Start", "s", 0.0, 600.0, 0.0), Num("count", "Strands", "", 100.0, 20000.0, 2000.0), Num("segments", "Segments", "", 2.0, 16.0, 6.0), Num("curl", "Curl", "", 0.0, 1.0, 0.0), Num("stiffness", "Stiffness", "", 0.0, 1.0, 0.5), Num("comb", "Comb", "", 0.0, 1.0, 0.6), Num("length", "Strand length (0 auto)", "", 0.0, 2000.0, 0.0), Num("width", "Strand width (0 auto)", "", 0.0, 50.0, 0.0), Num("seed", "Seed", "", 0.0, 9999.0, 1.0)],
        picks: &[Pick("region", "Grow on", &["top", "all", "front", "back"])],
        vectors: &[],
    },
    Sim {
        kind: "particles",
        name: "Particles",
        nums: &[TIMING[0], Num("duration", "Length", "s", 0.1, 120.0, 4.0), Num("size", "Size", "", 1.0, 4000.0, 650.0), Num("rate", "Rate", "/s", 0.0, 5000.0, 100.0), Num("burst", "Burst", "", 0.0, 5000.0, 0.0), Num("life", "Life", "s", 0.05, 30.0, 1.2), Num("emit_for", "Emit for (0 all)", "s", 0.0, 120.0, 0.0), Num("seed", "Seed", "", 0.0, 9999.0, 1.0)],
        picks: &[Pick("preset", "Preset", &["sparks", "rain", "snow", "dust", "confetti", "embers", "magic", "debris", "bubbles", "fountain"])],
        vectors: &[("position", "Position"), ("velocity", "Velocity"), ("wind", "Wind")],
    },
    Sim { kind: "smoke", name: "Smoke", nums: &[TIMING[0], Num("duration", "Length", "s", 0.2, 30.0, 4.0), Num("size", "Size", "", 10.0, 4000.0, 750.0), Num("resolution", "Detail", "", 16.0, 128.0, 48.0), Num("radius", "Source size", "", 1.0, 400.0, 45.0), Num("smoke", "Smoke", "", 0.0, 4.0, 1.2), Num("heat", "Heat", "", 0.0, 20.0, 3.0), Num("swirl", "Swirl", "", 0.0, 10.0, 2.5), Num("density", "Density", "", 0.0, 0.2, 0.05)], picks: &[], vectors: &[] },
    Sim { kind: "fire", name: "Fire", nums: &[TIMING[0], Num("duration", "Length", "s", 0.2, 30.0, 4.0), Num("size", "Size", "", 10.0, 4000.0, 750.0), Num("resolution", "Detail", "", 16.0, 128.0, 48.0), Num("radius", "Source size", "", 1.0, 400.0, 45.0), Num("smoke", "Smoke", "", 0.0, 4.0, 0.6), Num("heat", "Heat", "", 0.0, 20.0, 9.0), Num("fire", "Flame", "", 0.0, 4.0, 1.5), Num("swirl", "Swirl", "", 0.0, 10.0, 2.5), Num("density", "Density", "", 0.0, 0.2, 0.03)], picks: &[], vectors: &[] },
    Sim { kind: "water", name: "Pond", nums: &[TIMING[0], Num("duration", "Length", "s", 0.2, 60.0, 4.0), Num("size", "Size", "", 10.0, 6000.0, 2000.0), Num("cells", "Detail", "", 32.0, 400.0, 120.0), Num("drops", "Drops", "", 0.0, 50.0, 3.0), Num("wakes", "Wakes", "", 0.0, 20.0, 0.0)], picks: &[], vectors: &[] },
    Sim { kind: "ocean", name: "Ocean", nums: &[TIMING[0], Num("duration", "Length", "s", 0.2, 120.0, 6.0), Num("size", "Size", "", 100.0, 20000.0, 7000.0), Num("cells", "Detail", "", 32.0, 400.0, 160.0), Num("height", "Wave height", "", 0.0, 800.0, 70.0), Num("wavelength", "Wavelength", "", 10.0, 4000.0, 1000.0), Num("seed", "Seed", "", 0.0, 9999.0, 1.0)], picks: &[], vectors: &[] },
];

fn is_model(mc: &m::Clip) -> bool {
    matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model)
}

fn stored(mc: &m::Clip) -> (usize, serde_json::Map<String, Value>) {
    let s = mc.meta.get("sim");
    let kind = s.and_then(|s| s.get("kind")).and_then(Value::as_str);
    let at = SIMS.iter().position(|x| Some(x.kind) == kind).unwrap_or(0);
    (at, s.and_then(|s| s.get("settings")).and_then(Value::as_object).cloned().unwrap_or_default())
}

pub fn section(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    if !is_model(&mc) {
        return;
    }
    let (mut at, mut set) = stored(&mc);
    let before = (at, set.clone());
    let mut bake = false;
    let playhead = (app.editor.playhead - mc.start).max(0) as f64 / 1e6 + mc.start as f64 / 1e6;
    Section::new("sim-settings", "Simulation settings").icon(Icon::ArrowsOutCardinal).start_closed().show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        ui.label(egui::RichText::new("Set up a simulation and bake it into keys. Unset values use the automatic ones for the project's size.").font(theme::caption()).color(tk.text_2));
        let names: Vec<&str> = SIMS.iter().map(|s| s.name).collect();
        kit::labeled_row(ui, "Simulation", |ui| {
            if let Some(i) = kit::menu_button(ui, "sim-kind", at, &names, ui.available_width() - 40.0, None) {
                at = i;
                set.clear();
            }
        });
        let sim = &SIMS[at];
        for n in sim.nums {
            let mut v = set.get(n.0).and_then(Value::as_f64).map_or(n.5, |x| x as f32);
            let out = FieldRow::new(n.1, &mut v, n.3..=n.4).default(n.5).unit(n.2).decimals(if n.4 <= 10.0 { 2 } else { 0 }).scrub(((n.4 - n.3) / 400.0).max(0.001)).show(ui);
            if out.changed {
                if (v - n.5).abs() < 1e-6 && n.0 != "at" {
                    set.remove(n.0);
                } else {
                    set.insert(n.0.into(), json!(v));
                }
            }
        }
        for p in sim.picks {
            let cur = set.get(p.0).and_then(Value::as_str).and_then(|s| p.2.iter().position(|o| *o == s)).unwrap_or(0);
            kit::labeled_row(ui, p.1, |ui| {
                if let Some(i) = kit::menu_button(ui, p.0, cur, p.2, ui.available_width() - 40.0, None) {
                    set.insert(p.0.into(), json!(p.2[i]));
                }
            });
        }
        for (key, label) in sim.vectors {
            let mut v: [f32; 3] = set.get(*key).and_then(Value::as_array).map(|a| std::array::from_fn(|i| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32)).unwrap_or([0.0; 3]);
            kit::labeled_row(ui, label, |ui| {
                let mut any = false;
                for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    any |= kit::NumberField::new(axis, &mut v[i]).decimals(1).width(52.0).show(ui).changed();
                }
                if any {
                    set.insert((*key).into(), json!(v));
                }
            });
        }
        ui.horizontal(|ui| {
            if Button::primary("Bake").icon(Icon::Play).compact().show(ui).clicked() {
                bake = true;
            }
            if Button::ghost("Reset settings").compact().show(ui).clicked() {
                set.clear();
            }
        });
    });
    if (at, &set) != (before.0, &before.1) {
        let seq = app.doc.seq();
        let mut c = mc.clone();
        c.meta.insert("sim".into(), json!({"kind": SIMS[at].kind, "settings": set}));
        app.doc.apply_gesture(&format!("sim-settings-{id}"), "Simulation settings", m::Edit::SetClip { seq, clip: Box::new(c) });
    }
    if bake {
        let ctx = ui.ctx().clone();
        let sim = &SIMS[at];
        let mut op = serde_json::Map::new();
        for (k, v) in &set {
            op.insert(k.clone(), v.clone());
        }
        op.insert("op".into(), json!("simulate"));
        op.insert("kind".into(), json!(sim.kind));
        match sim.kind {
            "rigid" => {
                op.insert("clips".into(), json!([id]));
            }
            "shatter" | "soft" | "hair" => {
                op.insert("clip".into(), json!(id));
            }
            "cloth" => {
                // A cape worn by the model, or a sheet of its own.
                op.insert("on".into(), json!(id));
            }
            _ => {}
        }
        op.entry("at").or_insert(json!(playhead));
        let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
        let mut s = crate::agent::Session::new(doc);
        let r = s.edit(&[Value::Object(op)], "Simulation", false);
        app.doc = s.doc;
        if let Err(e) = r {
            kit::push(&ctx, e, None, Some(Icon::Warning));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of the `simulate` op has a control here or is one of the
    /// ones the section fills in itself (which clips, where it is placed).
    #[test]
    fn every_simulate_setting_has_a_control() {
        let fields = crate::agent::args::op_fields("simulate").expect("simulate op");
        let by_section: std::collections::BTreeSet<&str> = SIMS.iter().flat_map(|s| s.nums.iter().map(|n| n.0).chain(s.picks.iter().map(|p| p.0)).chain(s.vectors.iter().map(|v| v.0))).collect();
        // Set by the section from the selection, or flags with their own place.
        let by_selection = ["kind", "clips", "clip", "on", "attach", "emit_from", "static", "colliders", "floor", "pin", "pins", "pin_radius", "impact", "by", "colour"];
        let missing: Vec<&&str> = fields.iter().filter(|f| !by_section.contains(*f) && !by_selection.contains(f)).collect();
        assert!(missing.is_empty(), "simulate settings with no control in editor/sim_edit.rs: {missing:?}");
        // Every kind the op takes is offered.
        for k in ["rigid", "shatter", "cloth", "soft", "hair", "particles", "smoke", "fire", "water", "ocean"] {
            assert!(SIMS.iter().any(|s| s.kind == k), "{k} is not offered");
        }
        assert!(super::super::inspector::all_tab_kinds().contains(&"sim-settings"));
    }
}
