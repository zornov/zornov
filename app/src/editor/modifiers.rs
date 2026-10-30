// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Inspector's "Curve modifiers" section: procedural layers on a
//! property's keys (noise, loop, steps, limits, a formula), with a small
//! graph of what the property does over the clip.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, FieldRow, IconButton, Section};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use studio_model as m;

/// Properties offered for a modifier: (label, path, shown units per
/// stored unit, unit, component).
const PROPS: &[(&str, &str, f32, &str, Option<u8>)] = &[
    ("Position", "transform.position", 1.0, "px", None),
    ("Scale", "transform.scale_x+y", 100.0, "%", None),
    ("Rotation", "transform.rotation", 1.0, "°", None),
    ("Opacity", "transform.opacity", 100.0, "%", None),
    ("Depth (Z)", "transform.position_z", 1.0, "px", None),
    ("Tilt (X rotation)", "transform.rotation_x", 1.0, "°", None),
    ("Turn (Y rotation)", "transform.rotation_y", 1.0, "°", None),
];

/// The kinds a person can add, as the menu says them.
const KINDS: [(&str, &str); 5] = [("Noise (wiggle)", "noise"), ("Loop (cycles)", "cycles"), ("Steps (on twos)", "stepped"), ("Limits", "limits"), ("Formula", "expression")];

const REPEATS: [&str; 4] = ["Hold", "Repeat", "Offset", "Mirror"];

fn repeat_index(r: m::Repeat) -> usize {
    match r {
        m::Repeat::Hold => 0,
        m::Repeat::Repeat => 1,
        m::Repeat::Offset => 2,
        m::Repeat::Mirror => 3,
    }
}

fn repeat_of(i: usize) -> m::Repeat {
    [m::Repeat::Hold, m::Repeat::Repeat, m::Repeat::Offset, m::Repeat::Mirror][i.min(3)]
}

/// The model paths a property row stands for.
fn paths(path: &str) -> Vec<String> {
    match path {
        "transform.scale_x+y" => vec!["transform.scale_x".into(), "transform.scale_y".into()],
        p => vec![p.into()],
    }
}

/// A new modifier of `kind` with settings that read well at once.
pub fn fresh(kind: &str, scale: f32, axis: Option<u8>, seed: u32) -> m::Modifier {
    match kind {
        "noise" => m::Modifier::Noise { amount: (10.0 / scale) as f64, per_second: 2.0, octaves: 1, seed, axis, from: None, to: None },
        "cycles" => m::Modifier::Cycles { before: m::Repeat::Hold, after: m::Repeat::Repeat, count: 0 },
        "stepped" => m::Modifier::Stepped { per_second: 12.0, offset: 0 },
        "limits" => m::Modifier::Limits { min: None, max: None },
        _ => m::Modifier::Expression { expr: "value + sin(t * tau) * 10".into() },
    }
}

/// Adds (or replaces) `md` on each of `paths` of `mc`, giving a property
/// without keys one key holding its value now. Time modifiers go first.
pub fn put(mc: &mut m::Clip, paths: &[String], md: &m::Modifier) {
    for path in paths {
        let Some(now) = mc.get_property(path) else { continue };
        let tr = mc.keyframes.entry(path.clone()).or_insert_with(|| m::Track::new(vec![m::Keyframe::new(0, now, m::Interp::Linear)]));
        place(tr, md);
    }
}

/// The axis a noise modifier is confined to.
fn axis_of(md: &m::Modifier) -> Option<u8> {
    match md {
        m::Modifier::Noise { axis, .. } => *axis,
        _ => None,
    }
}

/// Puts `md` on `tr`: one of each kind (noise: one per axis, and one for
/// all axes replaces them), time modifiers first since they pick the
/// moment the others read.
pub fn place(tr: &mut m::Track, md: &m::Modifier) {
    if matches!(md, m::Modifier::Noise { axis: None, .. }) {
        tr.modifiers.retain(|x| !matches!(x, m::Modifier::Noise { axis: Some(_), .. }));
    }
    match tr.modifiers.iter_mut().find(|x| x.name() == md.name() && axis_of(x) == axis_of(md)) {
        Some(slot) => *slot = md.clone(),
        None if matches!(md, m::Modifier::Cycles { .. } | m::Modifier::Stepped { .. }) => tr.modifiers.insert(0, md.clone()),
        None => tr.modifiers.push(md.clone()),
    }
}

/// Removes the modifier named `kind` from `paths`; a track left with one
/// key that only existed to carry it goes too.
pub fn take(mc: &mut m::Clip, paths: &[String], kind: &str) {
    for path in paths {
        if let Some(tr) = mc.keyframes.get_mut(path) {
            tr.modifiers.retain(|x| x.name() != kind);
            if tr.modifiers.is_empty() && tr.keys.len() == 1 && tr.keys[0].time == 0 {
                let v = tr.keys[0].value.clone();
                mc.keyframes.remove(path);
                mc.set_property(path, v);
            }
        }
    }
}

/// Width of the reset and keyframe slots every `FieldRow` keeps at its end.
// TODO(kit): a token for the row's end slots.
const END_SLOTS: f32 = 40.0;

/// A labelled row on the same grid as a `FieldRow`: the label column, then
/// content that stops where the value fields stop.
// TODO(kit): `labeled_row` with the narrow label and end slots.
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui, f32) -> R) -> R {
    let lw = super::params::label_col(ui);
    let full = ui.available_width();
    let w = (full - lw - END_SLOTS).max(40.0);
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.set_min_height(theme::space::CONTROL_H);
        let (r, resp) = ui.allocate_exact_size(Vec2::new(lw, theme::space::CONTROL_H), Sense::hover());
        let cut = kit::paint_text_clipped(ui.painter(), Pos2::new(crate::i18n::start_x(r, 0.0), r.center().y), crate::i18n::start_center(), label, theme::body(), t.text_2, lw - 8.0);
        kit::tip_if_cut(resp, cut, label);
        ui.scope(|ui| add(ui, w)).inner
    })
    .inner
}

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let has = mc.keyframes.values().any(|t| !t.modifiers.is_empty());
    let pick_id = egui::Id::unique(("mod-prop", id));
    let out = Section::new("modifiers", "Curve modifiers").icon(Icon::Waveform).resettable(has).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        let offered: Vec<usize> = (0..PROPS.len()).filter(|i| PROPS[*i].1 == "transform.scale_x+y" || mc.get_property(PROPS[*i].1).is_some()).filter(|i| mc.transform.three_d || !PROPS[*i].1.contains("_z") && !PROPS[*i].1.contains("rotation_")).collect();
        let mut cur = ui.data(|d| d.get_temp::<usize>(pick_id)).unwrap_or(3);
        if !offered.contains(&cur) {
            cur = offered.first().copied().unwrap_or(0);
        }
        let (label, path, scale, unit, axis) = PROPS[cur];
        let names: Vec<&str> = offered.iter().map(|i| PROPS[*i].0).collect();
        row(ui, "Property", |ui, w| {
            if let Some(i) = kit::menu_button(ui, "mod-prop", offered.iter().position(|x| *x == cur).unwrap_or(0), &names, w, None) {
                cur = offered[i];
                ui.data_mut(|d| d.insert_temp(pick_id, cur));
            }
        });
        let ps = paths(path);
        let track = mc.keyframes.get(&ps[0]).cloned();
        graph(ui, &mc, &ps[0], axis, track.as_ref());
        let mods: Vec<m::Modifier> = track.map(|t| t.modifiers).unwrap_or_default();
        let mut changed: Option<m::Clip> = None;
        let mut gesture = false;
        for (k, md) in mods.iter().enumerate() {
            ui.add_space(theme::space::SM);
            let mut next = md.clone();
            let mut remove = false;
            let title = KINDS.iter().find(|x| x.1 == md.name()).map_or(md.name(), |x| x.0);
            kit::name_row(ui, egui::RichText::new(title).font(theme::body()).color(t.text), |ui| {
                remove = IconButton::new(Icon::X, "Remove modifier").compact().show(ui).clicked();
            });
            match &mut next {
                m::Modifier::Noise { amount, per_second, octaves, seed, .. } => {
                    let mut a = *amount as f32 * scale;
                    let top = 500.0f32.max(a);
                    let r1 = FieldRow::new("Amount", &mut a, 0.0..=top).default(10.0).unit(unit).decimals(1).scrub(0.2).show(ui);
                    let mut f = *per_second as f32;
                    let r2 = FieldRow::new("Rate", &mut f, 0.1..=30.0).default(2.0).unit("/s").decimals(1).step(0.1).scrub(0.02).show(ui);
                    let mut o = *octaves as f32;
                    let r3 = FieldRow::new("Detail", &mut o, 1.0..=6.0).default(1.0).step(1.0).scrub(0.05).show(ui);
                    let mut s = *seed as f32;
                    FieldRow::new("Seed", &mut s, 0.0..=999.0).default(0.0).step(1.0).no_slider().show(ui);
                    *amount = (a / scale) as f64;
                    *per_second = f.max(0.1) as f64;
                    *octaves = o.round().clamp(1.0, 8.0) as u32;
                    *seed = s.max(0.0) as u32;
                    gesture |= r1.dragging || r2.dragging || r3.dragging;
                }
                m::Modifier::Cycles { before, after, count } => {
                    row(ui, "Before", |ui, w| {
                        if let Some(i) = kit::menu_button(ui, &format!("cyc-b-{k}"), repeat_index(*before), &REPEATS, w, None) {
                            *before = repeat_of(i);
                        }
                    });
                    row(ui, "After", |ui, w| {
                        if let Some(i) = kit::menu_button(ui, &format!("cyc-a-{k}"), repeat_index(*after), &REPEATS, w, None) {
                            *after = repeat_of(i);
                        }
                    });
                    let mut c = *count as f32;
                    FieldRow::new("Times", &mut c, 0.0..=100.0).default(0.0).step(1.0).no_slider().show(ui);
                    ui.label(egui::RichText::new(crate::i18n::shown("0 loops for ever.")).font(theme::caption()).color(t.text_2));
                    *count = c.max(0.0) as u32;
                }
                m::Modifier::Stepped { per_second, .. } => {
                    let mut f = *per_second as f32;
                    let r = FieldRow::new("Poses", &mut f, 1.0..=60.0).unit("/s").default(12.0).decimals(0).step(1.0).scrub(0.1).show(ui);
                    *per_second = f.max(1.0) as f64;
                    gesture |= r.dragging;
                }
                m::Modifier::Limits { min, max } => {
                    for (name, slot) in [("Lowest", min), ("Highest", max)] {
                        let cur = mc.get_property(&ps[0]).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        row(ui, name, |ui, w| {
                            let mut on = slot.is_some();
                            if kit::toggle(ui, &mut on, name, None).changed() {
                                *slot = on.then_some(cur);
                            }
                            if let Some(v) = slot {
                                let mut x = *v as f32 * scale;
                                let r = kit::NumberField::new(name, &mut x).range(-10_000.0..=10_000.0).decimals(1).unit(unit).width((w - 44.0).max(56.0)).show(ui);
                                *v = (x / scale) as f64;
                                gesture |= r.dragged();
                            }
                        });
                    }
                }
                m::Modifier::Expression { expr } => {
                    let mut text = expr.clone();
                    row(ui, "Formula", |ui, w| {
                        kit::text_field(ui, &mut text, "value + sin(t * tau) * 10", None, w, None);
                    });
                    let ok = m::modifier::expr::parse(&text);
                    match &ok {
                        Ok(_) => {
                            *expr = text;
                        }
                        Err(e) => {
                            ui.label(egui::RichText::new(e).font(theme::caption()).color(t.danger));
                        }
                    }
                    ui.label(egui::RichText::new(crate::i18n::shown("value is stored units (opacity and scale: 1 = 100 %), t is seconds into the clip.")).font(theme::caption()).color(t.text_2));
                }
            }
            if remove {
                let mut c = changed.take().unwrap_or_else(|| mc.clone());
                take(&mut c, &ps, md.name());
                changed = Some(c);
            } else if next != *md && next.problem().is_none() {
                let mut c = changed.take().unwrap_or_else(|| mc.clone());
                put(&mut c, &ps, &next);
                changed = Some(c);
            }
        }
        ui.add_space(theme::space::SM);
        let have: Vec<&str> = mods.iter().map(|x| x.name()).collect();
        let addable: Vec<(&str, &str)> = KINDS.iter().copied().filter(|(_, k)| !have.contains(k)).filter(|(_, k)| *k != "cycles" || mc.keyframes.get(&ps[0]).is_some_and(|t| t.keys.len() > 1)).collect();
        if !addable.is_empty() {
            let mut names: Vec<&str> = vec!["Add modifier…"];
            names.extend(addable.iter().map(|x| x.0));
            row(ui, "Add", |ui, w| {
                if let Some(i) = kit::menu_button(ui, "mod-add", 0, &names, w, None)
                    && i > 0
                {
                    let mut c = mc.clone();
                    put(&mut c, &ps, &fresh(addable[i - 1].1, scale, axis, id));
                    changed = Some(c);
                }
            });
        }
        if mods.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tf(format!("Wobble, loop or step {} without more keys. Loop needs two keys or more.", label.to_lowercase()))).font(theme::caption()).color(t.text_2));
        }
        if let Some(c) = changed {
            let seq = app.doc.seq();
            if gesture {
                app.doc.apply_gesture(&format!("modifier-{id}"), "Curve modifier", m::Edit::SetClip { seq, clip: Box::new(c) });
            } else {
                app.doc.apply("Curve modifier", m::Edit::SetClip { seq, clip: Box::new(c) });
            }
        }
    });
    if out.reset {
        let mut c = mc.clone();
        let all: Vec<(String, Vec<String>)> = c.keyframes.iter().map(|(p, t)| (p.clone(), t.modifiers.iter().map(|x| x.name().to_owned()).collect())).collect();
        for (p, kinds) in all {
            for k in kinds {
                take(&mut c, std::slice::from_ref(&p), &k);
            }
        }
        let seq = app.doc.seq();
        app.doc.apply("Remove curve modifiers", m::Edit::SetClip { seq, clip: Box::new(c) });
    }
}

/// The property over the clip's length (with its modifiers), with the
/// keys marked.
fn graph(ui: &mut egui::Ui, mc: &m::Clip, path: &str, axis: Option<u8>, track: Option<&m::Track>) {
    let t = theme::tokens(ui.ctx());
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 4, t.sunken);
    let n = 160usize;
    // Points draw a line per axis (x in the accent, y softer).
    let comp = |v: &m::Value, d: usize| -> Option<f64> {
        match (v, axis) {
            (m::Value::Point(q), None) => Some(if d == 0 { q.x } else { q.y }),
            (m::Value::Point(q), Some(a)) => (d == 0).then_some(if a == 1 { q.y } else { q.x }),
            (v, _) => (d == 0).then(|| v.as_f64().unwrap_or(0.0)),
        }
    };
    let dur = mc.duration.max(1);
    let samples: Vec<m::Value> = (0..=n).filter_map(|i| mc.property_at(path, dur * i as i64 / n as i64)).collect();
    if samples.len() < 2 {
        return;
    }
    let lines: Vec<Vec<f64>> = (0..2).map(|d| samples.iter().filter_map(|v| comp(v, d)).collect::<Vec<f64>>()).filter(|l| !l.is_empty()).collect();
    let (lo, hi) = lines.iter().flatten().fold((f64::MAX, f64::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
    let span = (hi - lo).max(1e-6);
    let inner = r.shrink2(Vec2::new(4.0, 8.0));
    let at = |i: usize, y: f64| Pos2::new(inner.left() + inner.width() * i as f32 / n as f32, inner.bottom() - inner.height() * ((y - lo) / span) as f32);
    for (d, ys) in lines.iter().enumerate() {
        let pts: Vec<Pos2> = ys.iter().enumerate().map(|(i, y)| at(i, *y)).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.5, if d == 0 { t.accent } else { t.text_2 })));
    }
    if let Some(tr) = track {
        for k in &tr.keys {
            let x = inner.left() + inner.width() * (k.time as f32 / dur as f32);
            if x >= inner.left() - 0.5 && x <= inner.right() + 0.5 {
                let y = at(0, comp(&k.value, 0).unwrap_or(0.0)).y;
                let c = Pos2::new(x, y);
                p.add(egui::Shape::convex_polygon(vec![c + Vec2::new(0.0, -3.5), c + Vec2::new(3.5, 0.0), c + Vec2::new(0.0, 3.5), c + Vec2::new(-3.5, 0.0)], Color32::WHITE, Stroke::new(1.0, Color32::from_black_alpha(120))));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_and_removing_leaves_a_still_property_as_it_was() {
        let mut mc = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 2_000_000, m::ClipSource::Gap);
        let before = mc.clone();
        let ps = paths("transform.rotation");
        put(&mut mc, &ps, &fresh("noise", 1.0, None, 3));
        assert!(mc.keyframes["transform.rotation"].varies());
        let a = mc.property_at("transform.rotation", 700_000).unwrap().as_f64().unwrap();
        assert!(a.abs() > 1e-6 && a.abs() <= 10.0, "{a}");
        take(&mut mc, &ps, "noise");
        assert_eq!(mc, before);
        // Scale is both axes, wobbling together.
        put(&mut mc, &paths("transform.scale_x+y"), &fresh("noise", 100.0, None, 3));
        assert_eq!(mc.property_at("transform.scale_x", 900_000), mc.property_at("transform.scale_y", 900_000));
    }
}
