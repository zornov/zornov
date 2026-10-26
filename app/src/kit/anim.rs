// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion timing (spec §7): 120 ms for hover/press, 180 ms for panels and
//! popovers, never longer than 240 ms, and a "reduce motion" switch that
//! turns all of it off.

use eframe::egui::{self, Color32, Id, Pos2, Stroke};

pub const HOVER: f32 = 0.12;
pub const PANEL: f32 = 0.18;
pub const LONGEST: f32 = 0.24;

fn reduce_id() -> Id {
    Id::unique("studio-reduce-motion")
}

pub fn set_reduce_motion(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(reduce_id(), on));
    ctx.global_style_mut(|s| s.animation_time = if on { 0.0 } else { HOVER });
    ctx.all_styles_mut(|s| s.animation_time = if on { 0.0 } else { HOVER });
}

pub fn reduce_motion(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(reduce_id()).unwrap_or(false))
}

fn dur(ctx: &egui::Context, d: f32) -> f32 {
    if reduce_motion(ctx) { 0.0 } else { d.min(LONGEST) }
}

/// Animated 0..1 hover amount for a widget.
pub fn hover_t(ctx: &egui::Context, id: Id, hovered: bool) -> f32 {
    ctx.animate_bool_with_time(id.with("hover"), hovered, dur(ctx, HOVER))
}

/// Ease-out 0..1 for panels and popovers.
pub fn panel_t(ctx: &egui::Context, id: Id, open: bool) -> f32 {
    let t = ctx.animate_bool_with_time(id.with("panel"), open, dur(ctx, PANEL));
    ease_out(t)
}

pub fn value(ctx: &egui::Context, id: Id, target: f32, d: f32) -> f32 {
    ctx.animate_value_with_time(id, target, dur(ctx, d))
}

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Time in seconds since start, for looping previews. Requests a repaint so
/// the caller keeps animating.
pub fn clock(ctx: &egui::Context) -> f64 {
    ctx.request_repaint();
    ctx.input(|i| i.time)
}

/// A 3/4 arc spinner. Reduce-motion shows a static arc.
pub fn spinner(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let ctx = painter.ctx();
    let t = if reduce_motion(ctx) { 0.0 } else { clock(ctx) as f32 };
    let start = t * std::f32::consts::TAU * 1.1;
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + (i as f32 / n as f32) * std::f32::consts::TAU * 0.72;
            center + egui::vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.75, color)));
}
