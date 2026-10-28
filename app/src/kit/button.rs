// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Buttons: text buttons in four kinds, icon-only buttons (always with a
//! tooltip naming the shortcut) and the on/off switch.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Accent fill. One per view at most.
    Primary,
    /// Raised fill.
    Secondary,
    /// No fill until hovered.
    Ghost,
    /// Destructive.
    Danger,
}

pub struct Button<'a> {
    label: &'a str,
    kind: Kind,
    icon: Option<Icon>,
    trailing: Option<Icon>,
    busy: bool,
    compact: bool,
    min_width: f32,
    forced: Option<Forced>,
    shortcut: Option<&'a str>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, kind: Kind) -> Self {
        Self {
            label,
            kind,
            icon: None,
            trailing: None,
            busy: false,
            compact: false,
            min_width: 0.0,
            forced: None,
            shortcut: None,
        }
    }
    pub fn primary(label: &'a str) -> Self {
        Self::new(label, Kind::Primary)
    }
    pub fn secondary(label: &'a str) -> Self {
        Self::new(label, Kind::Secondary)
    }
    pub fn ghost(label: &'a str) -> Self {
        Self::new(label, Kind::Ghost)
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn trailing(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }
    /// Show a spinner in place of the icon (spec §7: progress in place).
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn forced(mut self, f: Forced) -> Self {
        self.forced = Some(f);
        self
    }
    /// The keys shown in its tooltip (none when empty: an action with no keys).
    pub fn shortcut(mut self, s: &'a str) -> Self {
        self.shortcut = Some(s).filter(|s| !s.is_empty());
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let font = theme::control_medium();
        let h = if self.compact { theme::space::CONTROL_H_COMPACT } else { theme::space::CONTROL_H };
        let pad = if self.compact { 10.0 } else { 12.0 };
        let has_icon = self.icon.is_some() || self.busy;
        let text_w = if self.label.is_empty() { 0.0 } else { text_width(ui, self.label, font.clone()) };
        let mut w = pad * 2.0 + text_w;
        if has_icon {
            w += icons::SIZE + if text_w > 0.0 { 6.0 } else { 0.0 };
        }
        if self.trailing.is_some() {
            w += icons::SIZE + 4.0;
        }
        let w = w.max(self.min_width).max(h);
        let forced_disabled = self.forced == Some(Forced::Disabled);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let st = State::from_response(ui, &resp, self.forced);
        let enabled = !(st.disabled || forced_disabled);
        let label = tl(self.label).into_owned();
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &label));
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let (base, fg) = match self.kind {
            Kind::Primary => (t.accent_fill, t.on_accent()),
            // On raised cards, buttons take the inset field colour.
            Kind::Secondary => (if field_fill(ui) == t.panel { t.panel } else { t.raised }, t.text),
            Kind::Ghost => (Color32::TRANSPARENT, t.text),
            Kind::Danger => (if field_fill(ui) == t.panel { t.panel } else { t.raised }, t.danger),
        };
        let fill = if !enabled {
            if self.kind == Kind::Ghost { Color32::TRANSPARENT } else { theme::mix(base, t.panel, 0.5) }
        } else if st.pressed {
            match self.kind {
                Kind::Primary => theme::mix(t.accent_fill, Color32::BLACK, 0.12),
                Kind::Ghost => t.pressed(t.raised),
                _ => t.pressed(base),
            }
        } else {
            match self.kind {
                Kind::Primary => theme::mix(t.accent_fill, Color32::WHITE, 0.08 * st.hover),
                Kind::Ghost => theme::alpha(t.raised, st.hover),
                _ => theme::mix(base, t.hover(base), st.hover),
            }
        };
        let fg = if enabled { fg } else { t.disabled() };
        let cr = CornerRadius::same(theme::radius::CONTROL);
        let p = ui.painter();
        p.rect_filled(rect, cr, fill);
        if st.focused {
            focus_ring(p, rect, cr, &t);
        }
        let content_w = w - pad * 2.0;
        let mut x = rect.center().x - content_w.min(w) / 2.0;
        if self.min_width > 0.0 {
            // Centre the content in wide buttons.
            let natural = (has_icon as u8 as f32) * (icons::SIZE + if text_w > 0.0 { 6.0 } else { 0.0 })
                + text_w
                + self.trailing.map_or(0.0, |_| icons::SIZE + 4.0);
            x = rect.center().x - natural / 2.0;
        }
        let cy = rect.center().y;
        if self.busy {
            spinner(p, egui::pos2(x + icons::SIZE / 2.0, cy), 6.0, fg);
            x += icons::SIZE + if text_w > 0.0 { 6.0 } else { 0.0 };
        } else if let Some(icon) = self.icon {
            let r = Rect::from_center_size(egui::pos2(x + icons::SIZE / 2.0, cy), Vec2::splat(icons::SIZE));
            icons::paint(p, r, icon, icons::SIZE, fg);
            x += icons::SIZE + if text_w > 0.0 { 6.0 } else { 0.0 };
        }
        if text_w > 0.0 {
            p.text(egui::pos2(x, cy), Align2::LEFT_CENTER, shown(self.label), font, fg);
            x += text_w;
        }
        if let Some(icon) = self.trailing {
            let r = Rect::from_center_size(egui::pos2(x + 4.0 + icons::SIZE / 2.0, cy), Vec2::splat(icons::SIZE));
            icons::paint(p, r, icon, icons::SIZE, if enabled { t.text_2 } else { fg });
        }
        let resp = if let Some(s) = self.shortcut { tip(resp, self.label, Some(s)) } else { resp };
        if enabled { resp } else { resp.on_disabled_hover_text(shown("Not available right now")) }
    }
}

/// Icon-only button: 28 px square (24 compact), ghost until hovered. `active`
/// shows the toggled-on look (accent icon on a soft accent fill).
pub struct IconButton<'a> {
    icon: Icon,
    name: &'a str,
    shortcut: Option<&'a str>,
    active: bool,
    size: f32,
    icon_size: f32,
    forced: Option<Forced>,
    tint: Option<Color32>,
    filled: bool,
}

impl<'a> IconButton<'a> {
    pub fn new(icon: Icon, name: &'a str) -> Self {
        Self {
            icon,
            name,
            shortcut: None,
            active: false,
            size: theme::space::CONTROL_H,
            icon_size: icons::SIZE,
            forced: None,
            tint: None,
            filled: false,
        }
    }
    /// The keys shown in its tooltip (none when empty: an action with no keys).
    pub fn shortcut(mut self, s: &'a str) -> Self {
        self.shortcut = Some(s).filter(|s| !s.is_empty());
        self
    }
    pub fn active(mut self, a: bool) -> Self {
        self.active = a;
        self
    }
    pub fn compact(mut self) -> Self {
        self.size = theme::space::CONTROL_H_COMPACT;
        self
    }
    pub fn size(mut self, size: f32, icon_size: f32) -> Self {
        self.size = size;
        self.icon_size = icon_size;
        self
    }
    pub fn forced(mut self, f: Forced) -> Self {
        self.forced = Some(f);
        self
    }
    pub fn tint(mut self, c: Color32) -> Self {
        self.tint = Some(c);
        self
    }
    /// Raised fill at rest (for buttons floating over content).
    pub fn filled(mut self) -> Self {
        self.filled = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(self.size), Sense::click());
        self.paint_at(ui, rect, resp)
    }

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let id = ui.scope_id().with(("icon-button", self.name, rect.min.x as i32, rect.min.y as i32));
        let resp = ui.interact(rect, id, Sense::click());
        self.paint_at(ui, rect, resp)
    }

    fn paint_at(self, ui: &mut Ui, rect: Rect, resp: Response) -> Response {
        let t = theme::tokens(ui.ctx());
        let st = State::from_response(ui, &resp, self.forced);
        let name = tl(self.name).into_owned();
        let active = self.active;
        resp.widget_info(|| {
            if active {
                WidgetInfo::selected(Role::Button, !st.disabled, true, &name)
            } else {
                WidgetInfo::labeled(Role::Button, !st.disabled, &name)
            }
        });
        if ui.is_rect_visible(rect) {
            let cr = CornerRadius::same(theme::radius::CONTROL);
            let rest = if self.filled { t.raised } else { Color32::TRANSPARENT };
            let fill = if st.disabled {
                rest
            } else if self.active {
                if st.pressed { theme::mix(t.accent_soft, t.accent, 0.2) } else { theme::mix(t.accent_soft, t.accent, 0.1 * st.hover) }
            } else if st.pressed {
                t.pressed(t.raised)
            } else if self.filled {
                theme::mix(t.raised, t.hover(t.raised), st.hover)
            } else {
                theme::alpha(t.raised, st.hover)
            };
            let p = ui.painter();
            p.rect_filled(rect, cr, fill);
            let fg = if st.disabled {
                t.disabled()
            } else if self.active {
                t.accent
            } else if let Some(c) = self.tint {
                c
            } else {
                theme::mix(t.text_2, t.text, st.hover.max(st.pressed as u8 as f32))
            };
            if self.active {
                icons::paint_fill(p, rect, self.icon, self.icon_size, fg);
            } else {
                icons::paint(p, rect, self.icon, self.icon_size, fg);
            }
            if st.focused {
                focus_ring(p, rect, cr, &t);
            }
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        tip(resp, self.name, self.shortcut)
    }
}

/// On/off switch, 28×16.
pub fn toggle(ui: &mut Ui, on: &mut bool, name: &str, forced: Option<Forced>) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(28.0, 16.0), Sense::click());
    let rect = Rect::from_center_size(rect.center(), Vec2::new(28.0, 16.0));
    if resp.clicked() && forced.is_none() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = theme::tokens(ui.ctx());
    let st = State::from_response(ui, &resp, forced);
    let value = *on;
    let label = tl(name).into_owned();
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, !st.disabled, value, &label));
    let k = if forced.is_some() { value as u8 as f32 } else { ui.ctx().animate_bool_with_time(resp.id.with("on"), value, if reduce_motion(ui.ctx()) { 0.0 } else { HOVER }) };
    let off_fill = if t.dark { theme::mix(t.raised, Color32::WHITE, 0.08) } else { theme::mix(t.sunken, Color32::BLACK, 0.06) };
    let mut fill = theme::mix(off_fill, t.accent, k);
    if st.disabled {
        fill = theme::mix(fill, t.panel, 0.5);
    }
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(255), fill);
    let knob_x = egui::lerp(rect.left() + 8.0..=rect.right() - 8.0, k);
    let knob_r = 6.0 + 0.5 * st.hover;
    p.circle_filled(egui::pos2(knob_x, rect.center().y), knob_r, if st.disabled { t.disabled() } else { Color32::WHITE });
    if st.focused {
        focus_ring(p, rect, CornerRadius::same(255), &t);
    }
    tip(resp, name, None)
}
