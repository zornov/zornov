// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Popovers and menus: the only surfaces with a shadow (spec §1). They fade
//! and slide in over 180 ms and close on Esc or a click outside.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Area, Id, Order, Sense, Vec2, WidgetInfo, Role};

fn open_id(id: Id) -> Id {
    id.with("popover-open")
}

/// Whether any popover was open on the last pass (keyboard shortcuts leave
/// Escape to it).
pub fn any_popover_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(Id::new("kit-popover-shown"))).is_some_and(|n| n + 1 >= ctx.cumulative_pass_nr())
}

pub fn is_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp(open_id(id)).unwrap_or(false))
}

pub fn set_open(ctx: &egui::Context, id: Id, open: bool) {
    ctx.data_mut(|d| d.insert_temp(open_id(id), open));
}

pub fn toggle_open(ctx: &egui::Context, id: Id) {
    let o = is_open(ctx, id);
    set_open(ctx, id, !o);
}

/// Where the popover sits relative to its anchor.
#[derive(Clone, Copy)]
pub enum Side {
    Below,
    BelowRight,
}

/// Show a popover anchored to `anchor` while it is open. Returns the
/// content's return value when shown.
pub fn popover<R>(
    ctx: &egui::Context,
    id: Id,
    anchor: Rect,
    side: Side,
    width: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let open = is_open(ctx, id);
    if open {
        // The pass number first: asking the context for it while its data
        // is locked would wait on itself forever.
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(Id::new("kit-popover-shown"), pass));
    }
    let k = panel_t(ctx, id, open);
    if k <= 0.001 {
        return None;
    }
    let t = theme::tokens(ctx);
    let gap = 6.0;
    let slide = (1.0 - k) * 6.0;
    let (pivot, pos) = match side {
        Side::Below => (Align2::LEFT_TOP, anchor.left_bottom() + Vec2::new(0.0, gap - slide)),
        Side::BelowRight => (Align2::RIGHT_TOP, anchor.right_bottom() + Vec2::new(0.0, gap - slide)),
    };
    // Taller than the window: capped to it, and the menu scrolls (the
    // frame's 6 pt margins sit outside the scrolling part).
    let max_h = popup_max_height(ctx, anchor.bottom() + gap) - 12.0;
    // Wider than the window: as wide as it allows.
    let width = width.min(ctx.content_rect().width() - 2.0 * EDGE - 12.0).max(40.0);
    let resp = Area::new(id.with("area"))
        .order(Order::Foreground)
        .pivot(pivot)
        .fixed_pos(pos)
        .constrain(true)
        .interactable(open)
        .show(ctx, |ui| {
            ui.set_opacity(k);
            egui::Frame::new()
                .fill(t.panel)
                .stroke(egui::Stroke::new(1.0, t.line))
                .corner_radius(CornerRadius::same(theme::radius::CARD))
                .shadow(theme::popover_shadow())
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(width);
                    // The minimum too: an area lays its content out in last
                    // frame's size, which would otherwise keep a menu at the
                    // height it first had.
                    egui::ScrollArea::vertical()
                        .id_salt("popover-scroll")
                        .max_height(max_h)
                        .min_scrolled_height(max_h)
                        .auto_shrink([false, true])
                        .show_v(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            add(ui)
                        })
                        .inner
                })
                .inner
        });
    if open {
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !resp.response.rect.contains(p) && !anchor.contains(p));
        if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            set_open(ctx, id, false);
        }
    }
    Some(resp.inner)
}

/// One row in a menu: optional icon, label, optional shortcut, optional
/// check mark. Clicking closes the enclosing popover via `close`.
pub struct MenuItem<'a> {
    pub icon: Option<Icon>,
    pub label: &'a str,
    pub shortcut: Option<&'a str>,
    pub checked: Option<bool>,
    pub enabled: bool,
    pub danger: bool,
    pub reserve_icon: bool,
}

impl<'a> MenuItem<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { icon: None, label, shortcut: None, checked: None, enabled: true, danger: false, reserve_icon: false }
    }
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn shortcut(mut self, s: &'a str) -> Self {
        self.shortcut = Some(s).filter(|s| !s.is_empty());
        self
    }
    pub fn checked(mut self, c: bool) -> Self {
        self.checked = Some(c);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn reserve_icon(mut self) -> Self {
        self.reserve_icon = true;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, theme::space::CONTROL_H), if self.enabled { Sense::click() } else { Sense::hover() });
        let st = State::from_response(ui, &resp, None);
        let label = tl(self.label).into_owned();
        let enabled = self.enabled;
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &label));
        let p = ui.painter();
        if self.enabled && (st.hover > 0.0 || st.focused) {
            p.rect_filled(rect, CornerRadius::same(theme::radius::CONTROL), theme::alpha(t.raised, st.hover.max(st.focused as u8 as f32)));
        }
        let fg = if !self.enabled { t.disabled() } else if self.danger { t.danger } else { t.text };
        // Right-to-left languages start at the right edge.
        let dir = if crate::i18n::is_rtl() { -1.0 } else { 1.0 };
        let mut x = crate::i18n::start_x(rect, 8.0);
        if let Some(checked) = self.checked {
            if checked {
                icons::paint(p, Rect::from_center_size(egui::pos2(x + dir * 7.0, rect.center().y), Vec2::splat(14.0)), Icon::Check, 14.0, t.accent);
            }
            x += dir * 22.0;
        }
        if let Some(icon) = self.icon {
            icons::paint(p, Rect::from_center_size(egui::pos2(x + dir * 8.0, rect.center().y), Vec2::splat(16.0)), icon, 16.0, if self.enabled && !self.danger { t.text_2 } else { fg });
            x += dir * 26.0;
        } else if self.reserve_icon {
            x += dir * 26.0;
        }
        let sw = self.shortcut.map_or(0.0, |s| text_width(ui, s, theme::caption()) + 12.0);
        let room = if dir < 0.0 { x - rect.left() } else { rect.right() - x };
        let cut = paint_text_clipped(p, egui::pos2(x, rect.center().y), crate::i18n::start_center(), self.label, theme::control(), fg, room - sw - 8.0);
        if let Some(s) = self.shortcut {
            p.text(egui::pos2(crate::i18n::end_x(rect, 8.0), rect.center().y), crate::i18n::end_center(), shown(s), theme::caption(), t.text_2);
        }
        tip_if_cut(resp, cut, self.label)
    }
}

pub fn menu_separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.0), Sense::hover());
    hline(ui, r.left() + 4.0..=r.right() - 4.0, r.center().y);
}

pub fn menu_heading(ui: &mut Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
    // Group labels are small capitals.
    let text = tl(text).to_uppercase();
    paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(r, 8.0), r.center().y + 2.0), crate::i18n::start_center(), &text, theme::micro(), t.text_2, r.width() - 16.0);
}

/// What a modal reported this frame.
pub struct ModalOutput {
    /// Esc, the close button or a click on the backdrop.
    pub close: bool,
}

/// Dialog over a dimmed backdrop. It fades and scales in over 180 ms and
/// carries the popover shadow and hairline border (spec §1).
pub fn modal(ctx: &egui::Context, id: Id, size: Vec2, title: &str, add: impl FnOnce(&mut Ui)) -> ModalOutput {
    let t = theme::tokens(ctx);
    let k = panel_t(ctx, id, true);
    let screen = ctx.content_rect();
    let mut close = false;
    // Dim everything below; a click on the backdrop closes the dialog.
    let dim = ctx.layer_painter(egui::LayerId::new(Order::Middle, id.with("backdrop")));
    dim.rect_filled(screen, 0, Color32::from_black_alpha((if t.dark { 120.0 } else { 70.0 } * k) as u8));
    egui::Area::new(id.with("backdrop-hit")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (_, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        if resp.clicked() {
            close = true;
        }
    });
    let size = Vec2::new(size.x.min(screen.width() - 48.0), size.y.min(screen.height() - 48.0));
    let rect = Rect::from_center_size(screen.center() + Vec2::new(0.0, (1.0 - k) * 8.0), size);
    egui::Area::new(id.with("dialog")).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        ui.set_opacity(k);
        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.add(theme::popover_shadow().as_shape(r, CornerRadius::same(theme::radius::PANEL)));
        p.rect(r, CornerRadius::same(theme::radius::PANEL), t.panel, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        // Title row.
        let head = Rect::from_min_size(r.min, Vec2::new(r.width(), 56.0));
        paint_text_clipped(p, egui::pos2(crate::i18n::start_x(head, 20.0), head.center().y), crate::i18n::start_center(), title, theme::dialog_title(), t.text, head.width() - 76.0);
        let close_r = Rect::from_center_size(egui::pos2(crate::i18n::end_x(head, 28.0), head.center().y), Vec2::splat(28.0));
        if IconButton::new(Icon::X, "Close").shortcut("Esc").show_at(ui, close_r).clicked() {
            close = true;
        }
        hline(ui, r.left()..=r.right(), head.bottom());
        let body = Rect::from_min_max(egui::pos2(r.left(), head.bottom()), r.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("modal-body"));
        child.set_clip_rect(body);
        add(&mut child)
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.egui_wants_keyboard_input() {
        close = true;
    }
    ModalOutput { close }
}

/// A tall sheet over the middle of the window (the export screen): no
/// title row and no dimming; a click outside it, Esc or its close button
/// closes it.
pub fn sheet(ctx: &egui::Context, id: Id, rect: Rect, add: impl FnOnce(&mut Ui)) -> ModalOutput {
    let t = theme::tokens(ctx);
    let screen = ctx.content_rect();
    let mut close = false;
    egui::Area::new(id.with("backdrop-hit")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (_, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        if resp.clicked() {
            close = true;
        }
    });
    egui::Area::new(id.with("sheet")).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter();
        p.add(theme::popover_shadow().as_shape(r, CornerRadius::ZERO));
        p.rect_filled(r, 0, theme::mix(t.panel, Color32::WHITE, 0.012));
        p.vline(r.left(), r.y_range(), egui::Stroke::new(1.0, t.line));
        p.vline(r.right() - 1.0, r.y_range(), egui::Stroke::new(1.0, t.line));
        let close_r = Rect::from_center_size(egui::pos2(r.right() - 22.0, r.top() + 20.0), Vec2::splat(24.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("sheet-body"));
        child.set_clip_rect(r);
        add(&mut child);
        if IconButton::new(Icon::X, "Close").shortcut("Esc").size(24.0, 14.0).show_at(ui, close_r).clicked() {
            close = true;
        }
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.egui_wants_keyboard_input() {
        close = true;
    }
    ModalOutput { close }
}
