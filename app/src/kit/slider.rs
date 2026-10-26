// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slider (spec §3): 4 px track filled from the default value, a 12 px thumb
//! that grows to 14 on hover, a value bubble while dragging, and a soft snap
//! to the default within 2 % of the range.

use super::*;
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo};
use std::ops::RangeInclusive;

pub struct Slider<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    default: f32,
    step: f32,
    decimals: usize,
    unit: &'a str,
    name: &'a str,
    forced: Option<Forced>,
    width: Option<f32>,
    log: bool,
    show_label: bool,
    show_value: bool,
}

impl<'a> Slider<'a> {
    pub fn new(name: &'a str, value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        let default = *range.start();
        Self { value, range, default, step: 0.0, decimals: 0, unit: "", name, forced: None, width: None, log: false, show_label: false, show_value: false }
    }
    pub fn default(mut self, d: f32) -> Self {
        self.default = d;
        self
    }
    pub fn step(mut self, s: f32) -> Self {
        self.step = s;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
    pub fn unit(mut self, u: &'a str) -> Self {
        self.unit = u;
        self
    }
    pub fn forced(mut self, f: Forced) -> Self {
        self.forced = Some(f);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    /// Logarithmic response (needs a range that starts above zero).
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.log = on;
        self
    }
    /// Draw the name to the left of the track.
    pub fn with_label(mut self) -> Self {
        self.show_label = true;
        self
    }
    /// Draw the current value (with its unit) to the right of the track.
    pub fn with_value(mut self) -> Self {
        self.show_value = true;
        self
    }

    pub fn show(mut self, ui: &mut Ui) -> Response {
        if !(self.show_label || self.show_value) {
            return self.track(ui);
        }
        let t = theme::tokens(ui.ctx());
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let (decimals, unit) = (self.decimals, self.unit);
        let text_of = move |v: f32| format_value(v, decimals, unit);
        let measure = |ui: &Ui, s: String| ui.painter().layout_no_wrap(s, theme::body(), t.text).size().x;
        let value_w = if self.show_value { measure(ui, text_of(lo)).max(measure(ui, text_of(hi))).max(measure(ui, text_of(*self.value))) + 4.0 } else { 0.0 };
        let name = tl(self.name).into_owned();
        let gap = ui.spacing().item_spacing.x;
        let total = self.width.unwrap_or_else(|| ui.available_width().clamp(80.0, 200.0));
        ui.horizontal(|ui| {
            let mut used = 0.0;
            if self.show_label {
                let r = ui.label(egui::RichText::new(name).font(theme::body()).color(t.text_2));
                used += r.rect.width() + gap;
            }
            if self.show_value {
                used += value_w + gap;
            }
            self.width = Some((total - used).max(40.0));
            let resp = self.track(ui);
            if value_w > 0.0 {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(value_w, theme::space::CONTROL_H), Sense::hover());
                ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, text_of(*self.value), theme::body(), t.text);
            }
            resp
        })
        .inner
    }

    fn track(&mut self, ui: &mut Ui) -> Response {
        let map = Map { log: self.log, lo: *self.range.start(), hi: *self.range.end() };
        let t = theme::tokens(ui.ctx());
        let w = self.width.unwrap_or_else(|| ui.available_width()).max(40.0);
        let (rect, mut resp) =
            ui.allocate_exact_size(Vec2::new(w, theme::space::CONTROL_H), Sense::click_and_drag());
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let span = (hi - lo).max(f32::EPSILON);
        // Keep the thumb fully inside the rect.
        let track = rect.shrink2(Vec2::new(7.0, 0.0));
        let to_x = |v: f32| egui::lerp(track.left()..=track.right(), map.norm(v));
        let from_x = |x: f32| map.denorm((x - track.left()) / track.width());

        if self.forced.is_none() && ui.is_enabled() {
            let mut new = None;
            if let Some(pos) = resp.interact_pointer_pos() {
                if resp.dragged() || resp.clicked() || resp.drag_started() {
                    let fine = ui.input(|i| i.modifiers.alt);
                    let v = if fine && resp.dragged() {
                        map.denorm(map.norm(*self.value) + resp.drag_delta().x / track.width() * 0.1)
                    } else {
                        from_x(pos.x)
                    };
                    new = Some(v);
                }
            }
            if resp.has_focus() {
                let step = if self.step > 0.0 { self.step } else { span / 100.0 };
                let (l, r, shift) = ui.input(|i| {
                    (i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown),
                     i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp),
                     i.modifiers.shift)
                });
                let k = if shift { 10.0 } else { 1.0 };
                let log = self.log && lo > 0.0;
                let nudge = |dir: f32| {
                    if log { map.denorm(map.norm(*self.value) + dir * 0.01 * k) } else { *self.value + dir * step * k }
                };
                if l {
                    new = Some(nudge(-1.0));
                }
                if r {
                    new = Some(nudge(1.0));
                }
            }
            if let Some(mut v) = new {
                // Soft snap to the default.
                if (map.norm(v) - map.norm(self.default)).abs() <= 0.02 {
                    v = self.default;
                }
                if self.step > 0.0 {
                    v = (v / self.step).round() * self.step;
                }
                v = v.clamp(lo, hi);
                if v != *self.value {
                    *self.value = v;
                    resp.mark_changed();
                }
            }
        }

        let st = State::from_response(ui, &resp, self.forced);
        let value = *self.value;
        let name = self.name.to_owned();
        resp.widget_info(|| WidgetInfo::slider(!st.disabled, value as f64, &name));
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let p = ui.painter();
        let cy = rect.center().y;
        let track_rect = Rect::from_min_max(egui::pos2(track.left(), cy - 2.0), egui::pos2(track.right(), cy + 2.0));
        let track_fill = if t.dark { theme::mix(t.raised, Color32::WHITE, 0.07) } else { theme::mix(t.sunken, Color32::BLACK, 0.05) };
        p.rect_filled(track_rect, CornerRadius::same(2), track_fill);
        let accent = if st.disabled { t.disabled() } else { t.accent };
        let x0 = to_x(self.default);
        let x1 = to_x(value);
        if (x1 - x0).abs() > 0.5 {
            let fill = Rect::from_min_max(egui::pos2(x0.min(x1), cy - 2.0), egui::pos2(x0.max(x1), cy + 2.0));
            p.rect_filled(fill, CornerRadius::same(2), accent);
        }
        // Default tick, so the snap point is visible.
        if self.default > lo && self.default < hi {
            p.rect_filled(
                Rect::from_center_size(egui::pos2(x0, cy), Vec2::new(1.5, 8.0)),
                CornerRadius::same(1),
                theme::alpha(t.text_2, 0.6),
            );
        }
        let dragging = resp.dragged() || st.pressed;
        let r = 6.0 + st.hover.max(dragging as u8 as f32);
        let thumb = egui::pos2(x1, cy);
        let warm_white = theme::DARK.text;
        p.circle_filled(thumb, r, if st.disabled { t.disabled() } else { warm_white });

        if dragging && self.forced.is_none() {
            // Value bubble above the thumb, drawn on a foreground layer.
            let text = format_value(value, self.decimals, self.unit);
            let layer = egui::LayerId::new(egui::Order::Tooltip, resp.id.with("bubble"));
            let fp = ui.ctx().layer_painter(layer);
            let galley = fp.layout_no_wrap(text, theme::body_medium(), t.text);
            let size = galley.size() + Vec2::new(14.0, 8.0);
            let b = Rect::from_center_size(thumb - Vec2::new(0.0, r + 6.0 + size.y / 2.0), size);
            fp.add(egui::epaint::RectShape::new(b, CornerRadius::same(6), t.panel, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside).with_shadow_hint());
            fp.galley(b.center() - galley.size() / 2.0, galley, t.text);
        } else if self.forced == Some(Forced::Pressed) {
            let text = format_value(value, self.decimals, self.unit);
            let galley = p.layout_no_wrap(text, theme::body_medium(), t.text);
            let size = galley.size() + Vec2::new(14.0, 8.0);
            let b = Rect::from_center_size(thumb - Vec2::new(0.0, r + 2.0 + size.y / 2.0), size);
            p.rect(b, CornerRadius::same(6), t.panel, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
            p.galley(b.center() - galley.size() / 2.0, galley, t.text);
        }
        let _ = Align2::CENTER_CENTER;
        resp
    }
}

/// Small extension so the value bubble casts the popover shadow.
trait ShadowHint {
    fn with_shadow_hint(self) -> egui::Shape;
}
impl ShadowHint for egui::epaint::RectShape {
    fn with_shadow_hint(self) -> egui::Shape {
        let rect = self.rect;
        let shadow = theme::popover_shadow().as_shape(rect, CornerRadius::same(6));
        egui::Shape::Vec(vec![shadow.into(), self.into()])
    }
}

/// Format a number with a fixed number of decimals and an optional unit.
pub fn format_value(v: f32, decimals: usize, unit: &str) -> String {
    let v = if v.abs() < 0.5 * 10f32.powi(-(decimals as i32)) { 0.0 } else { v };
    if unit.is_empty() {
        format!("{v:.decimals$}")
    } else if unit == "%" || unit == "°" || unit == "×" {
        format!("{v:.decimals$}{unit}")
    } else {
        format!("{v:.decimals$} {unit}")
    }
}

#[derive(Clone, Copy)]
struct Map {
    log: bool,
    lo: f32,
    hi: f32,
}
impl Map {
    fn norm(self, v: f32) -> f32 {
        if self.log && self.lo > 0.0 && self.hi > self.lo {
            ((v.max(self.lo).ln() - self.lo.ln()) / (self.hi.ln() - self.lo.ln())).clamp(0.0, 1.0)
        } else {
            ((v - self.lo) / (self.hi - self.lo).max(f32::EPSILON)).clamp(0.0, 1.0)
        }
    }
    fn denorm(self, n: f32) -> f32 {
        let n = n.clamp(0.0, 1.0);
        if self.log && self.lo > 0.0 && self.hi > self.lo {
            (self.lo.ln() + n * (self.hi.ln() - self.lo.ln())).exp()
        } else {
            self.lo + n * (self.hi - self.lo)
        }
    }
}

/// Integer slider: same look, whole-number steps.
pub fn slider_i32(ui: &mut Ui, name: &str, value: &mut i32, range: RangeInclusive<i32>, width: Option<f32>) -> Response {
    let mut v = *value as f32;
    let mut s = Slider::new(name, &mut v, *range.start() as f32..=*range.end() as f32).step(1.0).default(*range.start() as f32).with_value();
    if let Some(w) = width {
        s = s.width(w);
    }
    let r = s.show(ui);
    *value = v.round() as i32;
    r
}

#[cfg(test)]
mod tests {
    /// Every slider in the app is the kit slider: egui's own is allowed
    /// nowhere outside this folder.
    #[test]
    fn no_egui_slider_outside_the_kit() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n == "kit") {
                        continue;
                    }
                    walk(&p, hits);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    let needle = ["egui::", "Slider::new"].concat();
                    for (i, l) in text.lines().enumerate() {
                        if l.contains(&needle) {
                            hits.push(format!("{}:{}", p.display(), i + 1));
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
        assert!(hits.is_empty(), "use kit::Slider instead of egui's slider: {hits:?}");
    }
}
