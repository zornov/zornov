// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Toasts (spec §3): bottom-centre over the viewer, one line, gone after
//! 4 s, with at most one action ("Undo").

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Id, Sense, Vec2};

pub const LIFETIME: f64 = 4.0;

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub action: Option<String>,
    pub icon: Option<Icon>,
    pub born: f64,
    pub id: u64,
}

#[derive(Clone, Default)]
struct Queue {
    items: Vec<Toast>,
    next: u64,
}

fn qid() -> Id {
    Id::unique("studio-toasts")
}

/// Show a toast. Returns its id, so a caller can match the action click.
pub fn push(ctx: &egui::Context, text: impl Into<String>, action: Option<&str>, icon: Option<Icon>) -> u64 {
    let now = ctx.input(|i| i.time);
    let text: String = text.into();
    crate::a11y::announce(ctx, crate::i18n::t(&text).into_owned());
    ctx.data_mut(|d| {
        let q = d.get_temp_mut_or_default::<Queue>(qid());
        q.next += 1;
        let id = q.next;
        // One at a time: a new toast replaces the old one.
        q.items.clear();
        q.items.push(Toast { text, action: action.map(str::to_owned), icon, born: now, id });
        id
    })
}

/// Draw the current toast anchored to the bottom centre of `area`. Returns
/// the id of a toast whose action was clicked.
pub fn show(ctx: &egui::Context, area: Rect, bottom_offset: f32) -> Option<u64> {
    let now = ctx.input(|i| i.time);
    let mut q: Queue = ctx.data(|d| d.get_temp(qid()).unwrap_or_default());
    q.items.retain(|t| now - t.born < LIFETIME + 0.3);
    let toast = q.items.last().cloned();
    ctx.data_mut(|d| d.insert_temp(qid(), q));
    let toast = toast?;
    let age = (now - toast.born) as f32;
    let fade_in = if reduce_motion(ctx) { 1.0 } else { ease_out((age / PANEL).min(1.0)) };
    let fade_out = ((LIFETIME as f32 + 0.3 - age) / 0.3).clamp(0.0, 1.0);
    let k = fade_in.min(fade_out);
    ctx.request_repaint();

    let mut clicked = None;
    egui::Area::new(Id::unique(("toast", toast.id)))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(area.center().x, area.bottom() - bottom_offset + (1.0 - k) * 8.0))
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_opacity(k);
            if body(ui, &toast, area.width()) {
                clicked = Some(toast.id);
            }
        });
    if clicked.is_some() {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Queue>(qid()).items.clear());
    }
    clicked
}

/// The toast surface itself; shared by the live toast and the gallery.
fn body(ui: &mut Ui, toast: &Toast, area_w: f32) -> bool {
    let t = theme::tokens(ui.ctx());
    let mut clicked = false;
    let inv = if t.dark { theme::mix(t.panel, Color32::WHITE, 0.06) } else { theme::DARK.raised };
    let fg = if t.dark { t.text } else { theme::DARK.text };
    egui::Frame::new()
        .fill(inv)
        .corner_radius(CornerRadius::same(theme::radius::CARD))
        .stroke(egui::Stroke::new(1.0, if t.dark { t.line } else { inv }))
        .shadow(theme::popover_shadow())
        .inner_margin(egui::Margin { left: 14, right: 8, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.set_min_height(28.0);
                if let Some(icon) = toast.icon {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    icons::paint(ui.painter(), r, icon, 16.0, if t.dark { t.accent } else { theme::DARK.accent });
                }
                let max_w = (area_w - 160.0).max(120.0);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(text_width(ui, &toast.text, theme::control()).min(max_w), 20.0), Sense::hover());
                if paint_text_clipped(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &toast.text, theme::control(), fg, max_w) {
                    resp.on_hover_text(shown(&toast.text));
                }
                if let Some(a) = &toast.action {
                    let w = text_width(ui, a, theme::control_medium()) + 20.0;
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
                    let st = State::from_response(ui, &resp, None);
                    let accent = if t.dark { t.accent } else { theme::DARK.accent };
                    ui.painter().rect_filled(r, CornerRadius::same(6), theme::alpha(Color32::WHITE, 0.08 * st.hover));
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, shown(a), theme::control_medium(), theme::mix(accent, Color32::WHITE, 0.25));
                    if st.focused {
                        focus_ring(ui.painter(), r, CornerRadius::same(6), &t);
                    }
                    if resp.clicked() {
                        clicked = true;
                    }
                } else {
                    ui.add_space(6.0);
                }
            });
        });
    clicked
}

/// Draw a toast inline (component gallery).
pub fn preview(ui: &mut Ui, text: &str, action: Option<&str>, icon: Option<Icon>) -> bool {
    let toast = Toast { text: text.into(), action: action.map(str::to_owned), icon, born: 0.0, id: 0 };
    body(ui, &toast, 600.0)
}
