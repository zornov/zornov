// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyframes on the timeline: a mark per moment that has keys, shaped by
//! how it eases (diamond: eased, dotted diamond: elastic, bounce and the
//! other named curves, square: hold, circle: smooth, triangle: linear). Click one to go to it, drag it to retime every key at that
//! moment, right-click it to pick how it eases into the next one.

use crate::app::StudioApp;
use crate::kit;
use crate::theme;
use crate::view::{Clip, SEC, Time};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;

/// The easing curves, as the agents name them too.
pub const CURVES: [(&str, &str); 16] = [
    ("Ease in and out", "ease_in_out"),
    ("Ease in", "ease_in"),
    ("Ease out", "ease_out"),
    ("Linear", "linear"),
    ("Smooth", "smooth"),
    ("Overshoot", "overshoot"),
    ("Anticipate", "anticipate"),
    ("Snap", "snap"),
    ("Whip", "whip"),
    ("Elastic", "elastic"),
    ("Spring", "spring"),
    ("Bounce", "bounce"),
    ("Back", "back"),
    ("Expo", "expo_out"),
    ("Steps", "steps(6)"),
    ("Hold", "hold"),
];

/// Which named curve a key leaves with (its `next` key's arriving handle
/// counts too), if it is one of them.
fn curve_of(key: &m::Keyframe, next: Option<&m::Keyframe>) -> Option<&'static str> {
    let close = |a: m::Handle, b: Option<m::Handle>| b.is_none_or(|b| (a.x - b.x).abs() < 0.02 && (a.y - b.y).abs() < 0.02);
    CURVES.iter().map(|(_, n)| *n).find(|n| {
        let Some((interp, out, inn, shape)) = crate::agent::curve(n) else { return false };
        interp == key.interp && (interp != m::Interp::Ease || (close(key.ease_out, out) && next.is_none_or(|nk| close(nk.ease_in, inn)))) && (interp != m::Interp::Curve || key.curve == shape)
    })
}

/// Sets the curve leaving every key of `mc` at `time`.
fn set_curve(mc: &mut m::Clip, time: Time, name: &str) {
    let Some((interp, out, inn, shape)) = crate::agent::curve(name) else { return };
    for track in mc.keyframes.values_mut() {
        let Some(i) = track.keys.iter().position(|k| k.time == time) else { continue };
        track.keys[i].interp = interp;
        track.keys[i].curve = shape;
        if let Some(h) = out {
            track.keys[i].ease_out = h;
        }
        if let (Some(h), Some(next)) = (inn, track.keys.get_mut(i + 1)) {
            next.ease_in = h;
        }
    }
}

/// Moves every key of `mc` at `from` to `to` (a key already at `to` on
/// the same track gives way).
fn retime(mc: &mut m::Clip, from: Time, to: Time) {
    for track in mc.keyframes.values_mut() {
        if let Some(mut k) = track.remove_key(from) {
            track.remove_key(to);
            k.time = to;
            track.set_key(k);
        }
    }
}

/// Paints and handles the key marks of clip `c` drawn in `r`.
pub fn timeline(app: &mut StudioApp, ui: &mut egui::Ui, c: &Clip, r: Rect, track: Rect) {
    if c.keys.is_empty() || r.width() < 24.0 || r.height() < 30.0 {
        return;
    }
    let Some(mc) = app.doc.model.active().clip(m::ClipId(c.id as u64)).cloned() else { return };
    let t = theme::tokens(ui.ctx());
    let mut times: Vec<Time> = mc.keyframes.values().flat_map(|tr| tr.keys.iter().map(|k| k.time)).collect();
    times.sort();
    times.dedup();
    let x_of = |kt: Time| r.left() + r.width() * (kt as f32 / c.dur.max(1) as f32);
    let y = r.bottom() - 6.0;
    let frame = app.editor.frame_dur(app.doc.project.fps).max(1);
    let p = ui.painter().with_clip_rect(r.intersect(track));
    let drag_id = egui::Id::unique(("key-drag", c.id));
    for (i, &kt) in times.iter().enumerate() {
        let at = Pos2::new(x_of(kt), y);
        // By place, not time: a key keeps its widget (and its drag) while
        // it moves.
        let id = egui::Id::unique(("key", c.id, i));
        let resp = ui.interact(Rect::from_center_size(at, Vec2::splat(12.0)), id, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        // The first track with a key here decides the mark's shape.
        let (key, next) = mc.keyframes.values().find_map(|tr| tr.keys.iter().position(|k| k.time == kt).map(|i| (tr.keys[i].clone(), tr.keys.get(i + 1).cloned()))).expect("a key is at this time");
        let here = (app.editor.playhead - c.start - kt).abs() <= frame / 2;
        let hot = resp.hovered() || resp.dragged();
        let fill = if here { t.select } else { Color32::WHITE };
        let edge = Stroke::new(1.0, Color32::from_black_alpha(120));
        let s = 4.5 + hot as u8 as f32;
        match key.interp {
            m::Interp::Hold => {
                p.rect(Rect::from_center_size(at, Vec2::splat(s * 2.0)), 0, fill, edge, egui::StrokeKind::Middle);
            }
            m::Interp::Auto => {
                p.circle(at, s, fill, edge);
            }
            m::Interp::Linear => {
                p.add(egui::Shape::convex_polygon(vec![at + Vec2::new(-s, s), at + Vec2::new(0.0, -s), at + Vec2::new(s, s)], fill, edge));
            }
            m::Interp::Ease => {
                p.add(egui::Shape::convex_polygon(vec![at + Vec2::new(0.0, -s), at + Vec2::new(s, 0.0), at + Vec2::new(0.0, s), at + Vec2::new(-s, 0.0)], fill, edge));
            }
            m::Interp::Curve => {
                // A diamond with a dot: one of the springy curves.
                p.add(egui::Shape::convex_polygon(vec![at + Vec2::new(0.0, -s), at + Vec2::new(s, 0.0), at + Vec2::new(0.0, s), at + Vec2::new(-s, 0.0)], fill, edge));
                p.circle_filled(at, 1.6, Color32::from_black_alpha(160));
            }
            m::Interp::Spline | m::Interp::Through => {
                // A circle like Auto, with a dot (spline) or a bar (through).
                p.circle(at, s, fill, edge);
                if key.interp == m::Interp::Spline {
                    p.circle_filled(at, 1.6, Color32::from_black_alpha(160));
                } else {
                    p.line_segment([at + Vec2::new(-s * 0.6, 0.0), at + Vec2::new(s * 0.6, 0.0)], Stroke::new(1.5, Color32::from_black_alpha(160)));
                }
            }
        }
        if resp.clicked() {
            app.editor.playhead = c.start + kt;
        }
        // Drag: every key at this moment moves, on the frame grid.
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag_id, (kt, mc.clone())));
        }
        if resp.dragged() {
            if let (Some((from, origin)), Some(pos)) = (ui.data(|d| d.get_temp::<(Time, m::Clip)>(drag_id)), ui.input(|i| i.pointer.interact_pos())) {
                let raw = ((pos.x - r.left()) / r.width().max(1.0) * c.dur as f32) as Time;
                let to = ((raw as f64 / frame as f64).round() as Time * frame).clamp(0, c.dur);
                let mut moved = origin.clone();
                retime(&mut moved, from, to);
                let seq = app.doc.seq();
                app.doc.apply_gesture(&format!("key-move-{}-{from}", c.id), "Move keyframe", m::Edit::SetClip { seq, clip: Box::new(moved) });
                app.editor.playhead = c.start + to;
                let tip = format!("{:.2}s", (c.start + to) as f64 / SEC as f64);
                let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, drag_id));
                lp.text(pos + Vec2::new(10.0, -14.0), egui::Align2::LEFT_BOTTOM, tip, theme::caption(), t.text);
            }
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<(Time, m::Clip)>(drag_id));
        }
        // Right-click: how this key eases into the next.
        let menu_id = egui::Id::unique(("key-menu", c.id, i));
        if resp.secondary_clicked() {
            kit::set_open(ui.ctx(), menu_id, true);
        }
        let current = curve_of(&key, next.as_ref());
        let mut pick: Option<&str> = None;
        let mut delete = false;
        kit::popover(ui.ctx(), menu_id, resp.rect, kit::Side::Below, 220.0, |ui| {
            kit::menu_heading(ui, "Into the next key");
            graph(ui, &key, next.as_ref());
            for (label, name) in CURVES {
                if kit::MenuItem::new(label).checked(current == Some(name)).show(ui).clicked() {
                    pick = Some(name);
                }
            }
            kit::menu_separator(ui);
            if kit::MenuItem::new("Delete keyframe").danger().show(ui).clicked() {
                delete = true;
            }
        });
        if pick.is_some() || delete {
            kit::set_open(ui.ctx(), menu_id, false);
            let mut changed = mc.clone();
            match pick {
                Some(name) => set_curve(&mut changed, kt, name),
                None => {
                    for track in changed.keyframes.values_mut() {
                        track.remove_key(kt);
                    }
                    changed.keyframes.retain(|_, tr| !tr.keys.is_empty());
                }
            }
            let seq = app.doc.seq();
            app.doc.apply(if delete { "Delete keyframe" } else { "Keyframe easing" }, m::Edit::SetClip { seq, clip: Box::new(changed) });
        }
    }
}

/// The curve from this key to the next, drawn as a small graph.
fn graph(ui: &mut egui::Ui, key: &m::Keyframe, next: Option<&m::Keyframe>) {
    let t = theme::tokens(ui.ctx());
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::hover());
    let r = r.shrink2(Vec2::new(10.0, 8.0));
    let p = ui.painter();
    p.rect_stroke(r, 4, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    let Some(next) = next else {
        p.text(r.center(), egui::Align2::CENTER_CENTER, crate::i18n::shown("the last key: nothing follows"), theme::caption(), t.text_2);
        return;
    };
    // The same easing on a plain 0 → 1 value over one second.
    let mut a = key.clone();
    a.time = 0;
    a.value = m::Value::Scalar(0.0);
    let mut b = next.clone();
    b.time = SEC;
    b.value = m::Value::Scalar(1.0);
    let track = m::Track::new(vec![a, b]);
    let pts: Vec<Pos2> = (0..=48)
        .map(|i| {
            let x = i as f32 / 48.0;
            let v = match track.eval((x as f64 * SEC as f64) as Time) {
                Some(m::Value::Scalar(v)) => v as f32,
                _ => x,
            };
            // Room above and below for overshoot and anticipation.
            Pos2::new(egui::lerp(r.x_range(), x), r.bottom() - (v + 0.25) / 1.5 * r.height())
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.5, t.accent)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> m::Clip {
        let mut c = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 2 * SEC, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        for p in ["transform.rotation", "transform.opacity"] {
            c.keyframes.insert(p.into(), m::Track::new(vec![m::Keyframe::new(0, m::Value::Scalar(0.0), m::Interp::Linear), m::Keyframe::new(SEC, m::Value::Scalar(1.0), m::Interp::Linear)]));
        }
        c
    }

    #[test]
    fn curves_apply_to_every_key_at_a_moment_and_read_back() {
        let mut c = clip();
        set_curve(&mut c, 0, "overshoot");
        for tr in c.keyframes.values() {
            assert_eq!(curve_of(&tr.keys[0], tr.keys.get(1)), Some("overshoot"));
        }
        set_curve(&mut c, 0, "hold");
        assert_eq!(curve_of(&c.keyframes["transform.opacity"].keys[0], None), Some("hold"));
    }

    #[test]
    fn retiming_moves_every_track_together() {
        let mut c = clip();
        retime(&mut c, SEC, SEC / 2);
        for tr in c.keyframes.values() {
            assert_eq!(tr.keys.iter().map(|k| k.time).collect::<Vec<_>>(), vec![0, SEC / 2]);
        }
    }
}
