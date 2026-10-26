// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's light, air and look controls for 3D scenes: how a lamp
//! shows itself (glow, streak, beam, flicker, flame), the scene's air and
//! look on the light clip that carries them (fog, weather, exposure, tone
//! map, bloom, grade, vignette, grain, halation), and a model's wetness and
//! glow strength. Every value is a parameter of the clip, so each row
//! keyframes like any other, and the AI's `light`, `atmosphere` and
//! `model` ops set the same ones.

use super::*;

/// The model rows that go with the surface: how wet it is, how strongly it glows.
pub(super) fn surface_rows(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    row(app, ui, ids, "generator.wetness", "Wetness", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
    row(app, ui, ids, "generator.emissive_strength", "Glow strength", 0.0..=20.0, 1.0, "×", 1, same, same);
}

fn colour_param(mc: &m::Clip, key: &str, default: Color32) -> Color32 {
    match &mc.source {
        m::ClipSource::Generator(g) => match g.params.get(key) {
            Some(m::Value::Colour(k)) => Color32::from_rgb((k.r.clamp(0.0, 1.0) * 255.0) as u8, (k.g.clamp(0.0, 1.0) * 255.0) as u8, (k.b.clamp(0.0, 1.0) * 255.0) as u8),
            _ => default,
        },
        _ => default,
    }
}

fn colour_row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip, key: &'static str, label: &str, default: Color32) {
    let mut col = colour_param(mc, key, default);
    kit::labeled_row(ui, label, |ui| {
        if kit::colour_well(ui, key, &mut col, None).changed() {
            let rgba = m::Value::Colour(m::Rgba::new(col.r() as f64 / 255.0, col.g() as f64 / 255.0, col.b() as f64 / 255.0, 1.0));
            change(app, ids, label, |c| set_param(c, key, rgba.clone()));
        }
    });
}

fn choice_param(mc: &m::Clip, key: &str) -> Option<String> {
    match &mc.source {
        m::ClipSource::Generator(g) => g.params.get(key).and_then(m::Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// How a lamp shows itself, and whether the light carries the scene's air
/// and look.
pub(super) fn light_look(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip, kind: sp::LightKind) {
    Section::new("light-look", "Light look").icon(Icon::Sun).show(ui, |ui| {
        if matches!(kind, sp::LightKind::Point | sp::LightKind::Spot) {
            row(app, ui, ids, "generator.glow", "Glow", 0.0..=300.0, 0.0, "%", 0, pct, from_pct);
            row(app, ui, ids, "generator.flare", "Streak", 0.0..=300.0, 0.0, "%", 0, pct, from_pct);
            row(app, ui, ids, "generator.beam", "Beam", 0.0..=300.0, 0.0, "%", 0, pct, from_pct);
            row(app, ui, ids, "generator.source_size", "Source size", 0.0..=2.0, 0.0, "m", 2, same, same);
            row(app, ui, ids, "generator.flicker", "Flicker", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
            let flame = param_bool(mc, "flame", false);
            flag_row(app, ui, ids, "Flame", flame, |c, v| {
                if v {
                    set_param(c, "flame", m::Value::Bool(true));
                } else if let m::ClipSource::Generator(g) = &mut c.source {
                    g.params.remove("flame");
                }
            });
            if flame {
                row(app, ui, ids, "generator.flame_height", "Flame height", 0.005..=3.0, 0.12, "m", 2, same, same);
                row(app, ui, ids, "generator.flame_width", "Flame width", 0.003..=2.0, 0.05, "m", 2, same, same);
                row(app, ui, ids, "generator.flame_tongues", "Flame tongues", 1.0..=12.0, 1.0, "", 0, same, same);
            }
        } else if kind == sp::LightKind::Parallel {
            row(app, ui, ids, "generator.beam", "Light shafts", 0.0..=300.0, 0.0, "%", 0, pct, from_pct);
        }
        let carries = param_bool(mc, "atmosphere", false);
        flag_row(app, ui, ids, "Carries air and look", carries, |c, v| {
            if v {
                set_param(c, "atmosphere", m::Value::Bool(true));
            } else if let m::ClipSource::Generator(g) = &mut c.source {
                g.params.remove("atmosphere");
            }
        });
    });
    if param_bool(mc, "atmosphere", false) {
        air_section(app, ui, ids, mc);
    }
}

const KINDS: [&str; 9] = ["Off", "Rain", "Snow", "Dust", "Embers", "Sparks", "Leaves", "Ash", "Smoke"];
const KIND_NAMES: [&str; 9] = ["", "rain", "snow", "dust", "embers", "sparks", "leaves", "ash", "smoke"];
const TONES: [&str; 3] = ["Filmic", "Soft", "None"];
const TONE_NAMES: [&str; 3] = ["filmic", "soft", "none"];

fn air_section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip) {
    let ground = (app.doc.project.height as f32) * 4.0;
    Section::new("air", "Air and look").icon(Icon::Aperture).show(ui, |ui| {
        row(app, ui, ids, "generator.ground", "Ground (world y)", -ground..=ground, 0.0, "px", 0, same, same);
        kit::menu_heading(ui, "Fog");
        row(app, ui, ids, "generator.fog_density", "Density", 0.0..=0.3, 0.0, "", 3, same, same);
        if mc.get_property("generator.fog_density").and_then(|v| v.as_f64()).is_some_and(|d| d > 0.0) {
            row(app, ui, ids, "generator.fog_falloff", "Thins with height", 0.0..=1.5, 0.0, "", 2, same, same);
            colour_row(app, ui, ids, mc, "fog_colour", "Fog colour", Color32::from_rgb(190, 200, 215));
            row(app, ui, ids, "generator.fog_anisotropy", "Glow toward lights", -90.0..=90.0, 40.0, "%", 0, pct, from_pct);
            row(app, ui, ids, "generator.fog_patchiness", "Patchiness", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
            row(app, ui, ids, "generator.fog_range", "Range", 5.0..=500.0, 80.0, "m", 0, same, same);
        }
        kit::menu_heading(ui, "Weather");
        for layer in 1..=3 {
            let key = format!("weather_{layer}");
            let now = choice_param(mc, &key).and_then(|k| KIND_NAMES.iter().position(|n| *n == k)).unwrap_or(0);
            kit::labeled_row(ui, &format!("Layer {layer}"), |ui| {
                if let Some(i) = kit::menu_button(ui, &format!("weather-{layer}"), now, &KINDS, ui.available_width() - 40.0, None) {
                    let key = key.clone();
                    change(app, ids, "Weather", |c| {
                        if let m::ClipSource::Generator(g) = &mut c.source {
                            if i == 0 {
                                g.params.remove(&key);
                            } else {
                                g.params.insert(key.clone(), m::Value::Choice(KIND_NAMES[i].into()));
                            }
                        }
                    });
                }
            });
            if now > 0 {
                row(app, ui, ids, &format!("generator.{key}_amount"), "Amount", 0.0..=150.0, 60.0, "%", 0, pct, from_pct);
                row(app, ui, ids, &format!("generator.{key}_wind_x"), "Wind (right)", -20.0..=20.0, 0.0, "m/s", 1, same, same);
                row(app, ui, ids, &format!("generator.{key}_size"), "Size", 0.1..=5.0, 1.0, "×", 1, same, same);
                row(app, ui, ids, &format!("generator.{key}_speed"), "Speed", 0.0..=4.0, 1.0, "×", 1, same, same);
                if now == 1 {
                    row(app, ui, ids, &format!("generator.{key}_splash"), "Splashes", 0.0..=100.0, 100.0, "%", 0, pct, from_pct);
                }
            }
        }
        kit::menu_heading(ui, "Exposure and tone");
        row(app, ui, ids, "generator.exposure", "Exposure", -4.0..=4.0, 0.0, "stops", 1, same, same);
        let auto = param_bool(mc, "auto_exposure", true);
        flag_row(app, ui, ids, "Auto exposure", auto, |c, v| set_param(c, "auto_exposure", m::Value::Bool(v)));
        let tone = choice_param(mc, "tonemap").and_then(|k| TONE_NAMES.iter().position(|n| *n == k)).unwrap_or(0);
        kit::labeled_row(ui, "Tone map", |ui| {
            if let Some(i) = kit::menu_button(ui, "air-tonemap", tone, &TONES, ui.available_width() - 40.0, None) {
                change(app, ids, "Tone map", |c| set_param(c, "tonemap", m::Value::Choice(TONE_NAMES[i].into())));
            }
        });
        row(app, ui, ids, "generator.contrast", "Contrast", 20.0..=300.0, 100.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.saturation", "Saturation", 0.0..=300.0, 100.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.richness", "Rich shadows", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.temperature", "Temperature", -100.0..=100.0, 0.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.tint", "Tint", -100.0..=100.0, 0.0, "%", 0, pct, from_pct);
        colour_row(app, ui, ids, mc, "shadow_tint", "Shadow tint", Color32::from_rgb(26, 64, 128));
        row(app, ui, ids, "generator.shadow_tint_amount", "Shadow tint amount", 0.0..=200.0, 50.0, "%", 0, pct, from_pct);
        colour_row(app, ui, ids, mc, "highlight_tint", "Highlight tint", Color32::from_rgb(255, 192, 128));
        row(app, ui, ids, "generator.highlight_tint_amount", "Highlight tint amount", 0.0..=200.0, 50.0, "%", 0, pct, from_pct);
        kit::menu_heading(ui, "Bloom and lens");
        row(app, ui, ids, "generator.bloom_strength", "Bloom", 0.0..=150.0, 0.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.bloom_threshold", "Bloom threshold", 0.0..=4.0, 0.9, "", 2, same, same);
        row(app, ui, ids, "generator.bloom_radius", "Bloom reach", 0.0..=100.0, 50.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.halation", "Halation", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.vignette", "Vignette", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.grain", "Grain", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
        row(app, ui, ids, "generator.fringe", "Colour fringe", 0.0..=100.0, 0.0, "%", 0, pct, from_pct);
    });
}
