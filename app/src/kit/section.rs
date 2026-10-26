// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Section card (spec §3): header with collapse chevron, title, optional
//! enable switch and reset; body of field rows. The collapse state is
//! remembered per section *type* ("transform", "colour", …), so collapsing
//! Transform on one clip collapses it for every clip.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo, Role};

pub struct Section<'a> {
    kind: &'a str,
    title: &'a str,
    icon: Option<Icon>,
    enabled: Option<&'a mut bool>,
    resettable: bool,
    forced_open: Option<bool>,
    forced: Option<Forced>,
    fixed: bool,
    start_closed: bool,
}

#[derive(Default)]
pub struct SectionOutput {
    pub reset: bool,
    pub enable_changed: bool,
    pub open: bool,
}

pub const HEADER_H: f32 = 32.0;

/// Temp-data key holding the section kinds a tab shows.
pub const SECTION_FILTER: &str = "section-filter";

impl<'a> Section<'a> {
    pub fn new(kind: &'a str, title: &'a str) -> Self {
        Self { kind, title, icon: None, enabled: None, resettable: false, forced_open: None, forced: None, fixed: false, start_closed: false }
    }
    /// Advanced sections start collapsed until the person opens them.
    pub fn start_closed(mut self) -> Self {
        self.start_closed = true;
        self
    }
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn enable(mut self, on: &'a mut bool) -> Self {
        self.enabled = Some(on);
        self
    }
    pub fn resettable(mut self, r: bool) -> Self {
        self.resettable = r;
        self
    }
    pub fn force_open(mut self, open: bool) -> Self {
        self.forced_open = Some(open);
        self
    }
    /// Always open, without a collapse chevron (settings pages).
    pub fn fixed(mut self) -> Self {
        self.forced_open = Some(true);
        self.fixed = true;
        self
    }

    pub fn show(self, ui: &mut Ui, body: impl FnOnce(&mut Ui)) -> SectionOutput {
        let t = theme::tokens(ui.ctx());
        let mut out = SectionOutput::default();
        // A tabbed host (the clip inspector) lists the kinds its active tab
        // shows; other sections stay hidden.
        if let Some(allowed) = ui.data(|d| d.get_temp::<Vec<&'static str>>(egui::Id::new(SECTION_FILTER))) {
            if !allowed.contains(&self.kind) {
                return out;
            }
        }
        let key = egui::Id::unique(("section-collapsed", self.kind));
        let mut open = self.forced_open.unwrap_or_else(|| !ui.data_mut(|d| *d.get_persisted_mut_or_insert_with::<bool>(key, || self.start_closed)));
        let width = ui.available_width();

        // Body height is measured on the previous frame to animate the card.
        let height_id = ui.scope_id().with(("section-h", self.kind, self.title));
        let open_t = if self.forced_open.is_some() { open as u8 as f32 } else { panel_t(ui.ctx(), height_id, open) };
        let body_h: f32 = ui.data(|d| d.get_temp(height_id)).unwrap_or(0.0);

        let total_h = HEADER_H + body_h * open_t;
        let (card, _) = ui.allocate_exact_size(Vec2::new(width, total_h), Sense::hover());
        let cr = CornerRadius::same(theme::radius::CARD);
        // Flat sections: a hairline under each one, no card.
        ui.painter().hline(card.x_range(), card.bottom() - 0.5, egui::Stroke::new(1.0, t.line));

        // Header.
        let header = Rect::from_min_size(card.min, Vec2::new(width, HEADER_H));
        let hresp = ui.interact(header, height_id.with("header"), if self.fixed { Sense::hover() } else { Sense::click() });
        let hst = State::from_response(ui, &hresp, self.forced);
        let title = tl(self.title).into_owned();
        let open_now = open;
        hresp.widget_info(|| WidgetInfo::selected(Role::Button, true, open_now, &title));
        if hresp.clicked() && self.forced_open.is_none() {
            open = !open;
            ui.data_mut(|d| d.insert_persisted(key, !open));
        }
        if hst.hover > 0.0 && !self.fixed {
            let mut hr = header;
            if open_t > 0.01 {
                hr.max.y = hr.max.y.min(card.max.y);
            }
            let r = if open_t > 0.01 {
                CornerRadius { nw: theme::radius::CARD, ne: theme::radius::CARD, sw: 0, se: 0 }
            } else {
                cr
            };
            ui.painter().rect_filled(hr, r, theme::alpha(t.alt, hst.hover));
        }
        if hst.focused {
            focus_ring(ui.painter(), header, cr, &t);
        }
        let p = ui.painter().clone();
        let chevron_c = egui::pos2(header.left() + 20.0, header.center().y);
        // Rotate the chevron by swapping icons at the halfway point; a caret
        // pointing right reads as collapsed.
        let chev = if open_t > 0.5 { Icon::CaretDown } else { Icon::CaretRight };
        // Right-to-left languages mirror the header: chevron and title start
        // at the right, the switch and reset sit at the left.
        let dir = if crate::i18n::is_rtl() { -1.0 } else { 1.0 };
        let mut x = crate::i18n::start_x(header, 2.0);
        let chevron_c = egui::pos2(crate::i18n::start_x(header, 10.0), header.center().y);
        if !self.fixed {
            icons::paint(&p, Rect::from_center_size(chevron_c, Vec2::splat(14.0)), chev, 12.0, t.text_2);
            x += dir * 20.0;
        }
        if let Some(icon) = self.icon {
            icons::paint(&p, Rect::from_center_size(egui::pos2(x + dir * 8.0, header.center().y), Vec2::splat(16.0)), icon, 16.0, t.text_2);
            x += dir * 24.0;
        }
        let enabled_now = self.enabled.as_ref().map_or(true, |e| **e);
        // The title ends in an ellipsis before the switch and reset.
        let end_room = 4.0 + if self.enabled.is_some() { 36.0 } else { 0.0 } + if self.resettable { 28.0 } else { 0.0 };
        let title_room = if dir < 0.0 { x - header.left() - end_room } else { header.right() - end_room - x };
        let cut = paint_text_clipped(&p, egui::pos2(x, header.center().y), crate::i18n::start_center(), self.title, theme::section_title(), if enabled_now { t.text } else { t.text_2 }, title_room);
        if cut {
            hresp.clone().on_hover_text(shown(self.title));
        }

        // End side: reset, then enable switch.
        let mut end = 2.0;
        if let Some(on) = self.enabled {
            let r = Rect::from_two_pos(egui::pos2(crate::i18n::end_x(header, end), header.top()), egui::pos2(crate::i18n::end_x(header, end + 28.0), header.bottom()));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(crate::i18n::row_layout()));
            if toggle(&mut child, on, if *on { "Turn off" } else { "Turn on" }, self.forced.filter(|f| *f == Forced::Disabled)).changed() {
                out.enable_changed = true;
            }
            end += 36.0;
        }
        if self.resettable {
            let r = Rect::from_center_size(egui::pos2(crate::i18n::end_x(header, end + 12.0), header.center().y), Vec2::splat(24.0));
            if IconButton::new(Icon::ArrowCounterClockwise, "Reset section").size(24.0, 14.0).show_at(ui, r).clicked() {
                out.reset = true;
            }
        }

        // Body.
        if open_t > 0.0 {
            let body_rect = Rect::from_min_max(
                egui::pos2(card.left() + 2.0, header.bottom()),
                egui::pos2(card.right() - 2.0, header.bottom() + body_h.max(1.0) + 1000.0),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min))
                    .id_salt(("section-body", self.kind, self.title)),
            );
            child.set_clip_rect(card.intersect(ui.clip_rect()));
            on_card(&mut child);
            child.spacing_mut().item_spacing.y = 4.0;
            if !enabled_now {
                child.multiply_opacity(0.55);
            }
            body(&mut child);
            child.add_space(theme::space::PANEL_PAD - 4.0);
            let measured = child.min_rect().height();
            if (measured - body_h).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(height_id, measured));
                ui.ctx().request_repaint();
            }
        } else if body_h == 0.0 {
            // Measure once even when collapsed so opening animates.
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(card.min, Vec2::new(width - 24.0, 4000.0))).sizing_pass().invisible());
            on_card(&mut child);
            child.spacing_mut().item_spacing.y = 4.0;
            body(&mut child);
            child.add_space(theme::space::PANEL_PAD - 4.0);
            ui.data_mut(|d| d.insert_temp(height_id, child.min_rect().height()));
        }
        out.open = open;
        out
    }
}
