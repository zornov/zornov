// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Sound effects" section: the clip's effect chain (EQ,
//! compressor, limiter, gate, de-esser, reverb, delay, pitch shift) with
//! every setting the `audio` op takes, plus the fade curves and whether a
//! speed change keeps the pitch. The AI's `audio` op writes the same
//! `AudioProps`.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, FieldRow, IconButton, Section};
use crate::theme;
use eframe::egui;
use studio_model as m;

/// One setting of a sound effect.
pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub decimals: usize,
}

const fn s(key: &'static str, label: &'static str, unit: &'static str, min: f32, max: f32, default: f32, decimals: usize) -> Setting {
    Setting { key, label, unit, min, max, default, decimals }
}

/// The effects a clip can carry: id, name, settings. The ids are the
/// `audio` op's; a test keeps this list and the mixer's in step.
pub const EFFECTS: &[(&str, &str, &[Setting])] = &[
    (
        "eq",
        "EQ",
        &[
            s("bass", "Bass", "dB", -24.0, 24.0, 0.0, 1),
            s("low_mid", "Low mid", "dB", -24.0, 24.0, 0.0, 1),
            s("mid", "Mid", "dB", -24.0, 24.0, 0.0, 1),
            s("presence", "Presence", "dB", -24.0, 24.0, 0.0, 1),
            s("air", "Air", "dB", -24.0, 24.0, 0.0, 1),
            s("high_pass", "High-pass", "Hz", 0.0, 2000.0, 0.0, 0),
            s("low_pass", "Low-pass", "Hz", 0.0, 20000.0, 0.0, 0),
        ],
    ),
    (
        "compressor",
        "Compressor",
        &[
            s("threshold", "Threshold", "dB", -60.0, 0.0, -18.0, 1),
            s("ratio", "Ratio", ":1", 1.0, 20.0, 3.0, 1),
            s("attack", "Attack", "ms", 0.1, 200.0, 10.0, 1),
            s("release", "Release", "ms", 5.0, 1000.0, 120.0, 0),
            s("makeup", "Makeup", "dB", 0.0, 24.0, 0.0, 1),
        ],
    ),
    ("limiter", "Limiter", &[s("ceiling", "Ceiling", "dB", -24.0, 0.0, -1.0, 1)]),
    ("gate", "Gate", &[s("threshold", "Threshold", "dB", -80.0, 0.0, -40.0, 1), s("range", "Range", "dB", 0.0, 80.0, 80.0, 0)]),
    ("de_esser", "De-esser", &[s("threshold", "Threshold", "dB", -60.0, 0.0, -30.0, 1), s("freq", "Frequency", "Hz", 2000.0, 12000.0, 6500.0, 0)]),
    (
        "reverb",
        "Reverb",
        &[
            s("size", "Size", "", 0.2, 3.0, 1.0, 2),
            s("decay", "Decay", "", 0.05, 1.0, 0.5, 2),
            s("mix", "Mix", "", 0.0, 1.0, 0.25, 2),
            s("pre_delay", "Pre-delay", "ms", 0.0, 200.0, 10.0, 0),
        ],
    ),
    ("delay", "Delay", &[s("time", "Time", "ms", 10.0, 2000.0, 375.0, 0), s("feedback", "Feedback", "", 0.0, 0.95, 0.35, 2), s("mix", "Mix", "", 0.0, 1.0, 0.3, 2)]),
    ("pitch_shift", "Pitch shift", &[s("semitones", "Semitones", "", -24.0, 24.0, 0.0, 1)]),
];

const CURVES: [(m::FadeCurve, &str); 5] = [
    (m::FadeCurve::Linear, "Linear"),
    (m::FadeCurve::EqualPower, "Equal power"),
    (m::FadeCurve::SCurve, "S-curve"),
    (m::FadeCurve::Exponential, "Slow start"),
    (m::FadeCurve::Logarithmic, "Fast start"),
];

fn mclip(app: &StudioApp, id: u32) -> Option<m::Clip> {
    app.doc.model.active().clip(m::ClipId(id as u64)).cloned()
}

fn put(app: &mut StudioApp, ids: &[u32], key: &str, label: &str, f: impl Fn(&mut m::Clip)) {
    let seq = app.doc.seq();
    let edits: Vec<m::Edit> = ids
        .iter()
        .filter_map(|id| mclip(app, *id))
        .filter_map(|old| {
            let mut c = old.clone();
            f(&mut c);
            (c != old).then(|| m::Edit::SetClip { seq, clip: Box::new(c) })
        })
        .collect();
    if !edits.is_empty() {
        app.doc.apply_gesture(&format!("audio-fx-{key}-{ids:?}"), label, m::Edit::Batch { edits });
    }
}

/// The sound effects, fade curves and pitch keeping of the selected clips.
pub fn section(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32]) {
    let Some(mc) = ids.first().and_then(|id| mclip(app, *id)) else { return };
    let effects = mc.audio.effects.clone();
    let mut new_id = None;
    Section::new("audio-fx", "Sound effects").icon(Icon::Faders).start_closed().show(ui, |ui| {
        let tk = theme::tokens(ui.ctx());
        for fx in &effects {
            let Some((_, name, settings)) = EFFECTS.iter().find(|e| e.0 == fx.kind) else { continue };
            let kind = fx.kind.clone();
            ui.horizontal(|ui| {
                let mut on = fx.enabled;
                if kit::toggle(ui, &mut on, name, None).changed() {
                    put(app, ids, &format!("{kind}-on"), "Sound effect on or off", |c| {
                        if let Some(e) = c.audio.effects.iter_mut().find(|e| e.kind == kind) {
                            e.enabled = on;
                        }
                    });
                }
                ui.label(egui::RichText::new(*name).font(theme::section_title()).color(tk.text));
                if IconButton::new(Icon::X, "Remove effect").compact().show(ui).clicked() {
                    put(app, ids, &format!("{kind}-rm"), "Remove sound effect", |c| c.audio.effects.retain(|e| e.kind != kind));
                }
            });
            if kind == "reverb" {
                let rooms: Vec<(&str, &str)> = studio_sound::reverb::ROOMS.iter().map(|r| (r.0, r.1)).collect();
                let names: Vec<&str> = rooms.iter().map(|r| r.1).collect();
                let cur = match fx.params.get("room") {
                    Some(m::Value::Choice(r) | m::Value::Text(r)) => rooms.iter().position(|x| x.0 == r),
                    _ => None,
                }
                .unwrap_or(0);
                kit::labeled_row(ui, "Room", |ui| {
                    if let Some(i) = kit::menu_button(ui, "audio-fx-room", cur, &names, ui.available_width() - 40.0, None) {
                        let r = rooms[i].0.to_owned();
                        put(app, ids, "room", "Reverb room", |c| {
                            if let Some(e) = c.audio.effects.iter_mut().find(|e| e.kind == "reverb") {
                                e.params.insert("room".into(), m::Value::Choice(r.clone()));
                            }
                        });
                    }
                });
            }
            for st in *settings {
                let mut v = fx.params.get(st.key).and_then(m::Value::as_f64).map_or(st.default, |x| x as f32);
                let label = format!("{}", st.label);
                let out = FieldRow::new(&label, &mut v, st.min..=st.max).default(st.default).unit(st.unit).decimals(st.decimals).scrub(((st.max - st.min) / 400.0).max(0.001)).show(ui);
                if out.changed {
                    let (k, x) = (st.key, v as f64);
                    put(app, ids, &format!("{kind}-{k}"), "Sound effect setting", |c| {
                        if let Some(e) = c.audio.effects.iter_mut().find(|e| e.kind == kind) {
                            e.params.insert(k.into(), m::Value::Scalar(x));
                        }
                    });
                }
            }
        }
        let mut names: Vec<&str> = vec!["Add an effect"];
        names.extend(EFFECTS.iter().map(|e| e.1));
        kit::labeled_row(ui, "Add", |ui| {
            if let Some(i) = kit::menu_button(ui, "audio-fx-add", 0, &names, ui.available_width() - 40.0, None)
                && i > 0
            {
                new_id = Some(EFFECTS[i - 1].0);
            }
        });
        // Fades.
        let cnames: Vec<&str> = CURVES.iter().map(|c| c.1).collect();
        for (label, fade_in) in [("Fade-in curve", true), ("Fade-out curve", false)] {
            let cur = CURVES.iter().position(|c| c.0 == if fade_in { mc.audio.fade_in.curve } else { mc.audio.fade_out.curve }).unwrap_or(0);
            kit::labeled_row(ui, label, |ui| {
                if let Some(i) = kit::menu_button(ui, if fade_in { "audio-fade-in" } else { "audio-fade-out" }, cur, &cnames, ui.available_width() - 40.0, None) {
                    let k = CURVES[i].0;
                    put(app, ids, label, label, |c| if fade_in { c.audio.fade_in.curve = k } else { c.audio.fade_out.curve = k });
                }
            });
        }
        let mut keep = mc.audio.preserve_pitch;
        kit::labeled_row(ui, "Keep pitch", |ui| {
            if kit::toggle(ui, &mut keep, "Keep pitch when the speed changes", None).changed() {
                put(app, ids, "pitch", "Keep pitch", |c| c.audio.preserve_pitch = keep);
            }
        });
    });
    if let Some(kind) = new_id {
        let id = app.doc.alloc_id();
        put(app, ids, "fx-add", "Add sound effect", |c| {
            if !c.audio.effects.iter().any(|e| e.kind == kind) {
                c.audio.effects.push(m::Effect::new(m::EffectId(id), kind));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every effect and setting the `audio` op takes has a control here, and
    /// the mixer knows each one.
    #[test]
    fn every_sound_effect_setting_has_a_control() {
        let ops = ["eq", "compressor", "limiter", "gate", "de_esser", "reverb", "delay", "pitch_shift"];
        assert_eq!(EFFECTS.len(), ops.len());
        for k in ops {
            let (_, _, settings) = EFFECTS.iter().find(|e| e.0 == k).unwrap_or_else(|| panic!("effect {k} has no controls"));
            assert!(!settings.is_empty());
            // The mixer builds it from the settings given here.
            let mut e = m::Effect::new(m::EffectId(1), k);
            for st in *settings {
                e.params.insert(st.key.into(), m::Value::Scalar(st.default as f64));
                assert!(st.min <= st.default && st.default <= st.max, "{k}.{}: default outside range", st.key);
            }
            assert!(crate::sound_bridge::audio_effect(&e).is_some(), "{k}: the mixer does not build it");
        }
        assert!(super::super::inspector::all_tab_kinds().contains(&"audio-fx"), "the section is in the Audio tabs");
        // Reverb's other setting is its room.
        assert!(!studio_sound::reverb::ROOMS.is_empty());
    }
}
