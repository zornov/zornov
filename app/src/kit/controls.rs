// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Segmented control, chip group and menu button.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo, Role};

/// One option in a segmented control: a label, an icon, or both.
#[derive(Clone, Copy)]
pub struct Segment<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    /// Show only the icon; the label becomes the tooltip.
    pub icon_only: bool,
}

impl<'a> Segment<'a> {
    pub fn text(label: &'a str) -> Self {
        Self { label, icon: None, icon_only: false }
    }
    pub fn icon(icon: Icon, name: &'a str) -> Self {
        Self { label: name, icon: Some(icon), icon_only: true }
    }
    pub fn icon_text(icon: Icon, label: &'a str) -> Self {
        Self { label, icon: Some(icon), icon_only: false }
    }
}

/// Segmented control. The selected thumb slides between options.
/// `icon_only` segments show the label as a tooltip.
pub fn segmented(
    ui: &mut Ui,
    id_salt: &str,
    selected: &mut usize,
    options: &[Segment<'_>],
    width: Option<f32>,
    forced: Option<Forced>,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let h = theme::space::CONTROL_H;
    let seg_w: Vec<f32> = options
        .iter()
        .map(|o| match o.icon {
            Some(_) if o.icon_only || o.label.is_empty() => 32.0,
            Some(_) => text_width(ui, o.label, theme::control_medium()) + 20.0 + 22.0,
            None => text_width(ui, o.label, theme::control_medium()) + 24.0,
        })
        .collect();
    let natural: f32 = seg_w.iter().sum::<f32>() + 4.0;
    let total = width.unwrap_or(natural);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(total, h), Sense::hover());
    let scale = (total - 4.0) / (natural - 4.0);
    let cr = CornerRadius::same(theme::radius::CONTROL);
    ui.painter().rect_filled(rect, cr, field_fill(ui));
    let mut x = rect.left() + 2.0;
    let mut rects = Vec::with_capacity(options.len());
    for w in &seg_w {
        let w = w * scale;
        rects.push(Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), Vec2::new(w, h - 4.0)));
        x += w;
    }
    // Animated thumb.
    let base_id = ui.scope_id().with(("segmented", id_salt));
    let target = rects.get(*selected).map_or(rect.left(), |r| r.left());
    let target_w = rects.get(*selected).map_or(0.0, |r| r.width());
    let tx = if forced.is_some() { target } else { value(ui.ctx(), base_id.with("x"), target, PANEL) };
    let tw = if forced.is_some() { target_w } else { value(ui.ctx(), base_id.with("w"), target_w, PANEL) };
    let thumb = Rect::from_min_size(egui::pos2(tx, rect.top() + 2.0), Vec2::new(tw, h - 4.0));
    let thumb_fill = if t.dark { theme::mix(t.raised, Color32::WHITE, 0.10) } else { t.panel };
    let on_card_fill = field_fill(ui) == t.panel;
    let thumb_fill = if on_card_fill && !t.dark { t.raised } else if on_card_fill { theme::mix(t.raised, Color32::WHITE, 0.06) } else { thumb_fill };
    if !t.dark {
        ui.painter().rect_filled(thumb.translate(Vec2::new(0.0, 0.5)), CornerRadius::same(5), Color32::from_black_alpha(18));
    }
    ui.painter().rect_filled(thumb, CornerRadius::same(5), thumb_fill);

    for (i, (o, r)) in options.iter().zip(&rects).enumerate() {
        let sid = base_id.with(i);
        let sresp = ui.interact(*r, sid, Sense::click());
        let st = State::from_response(ui, &sresp, forced.map(|f| if f == Forced::Disabled { f } else if f == Forced::Focused && is_sel_pre(i, *selected) { f } else { Forced::Rest }));
        let is_sel = i == *selected;
        let label = tl(o.label).into_owned();
        sresp.widget_info(|| WidgetInfo::selected(Role::RadioButton, !st.disabled, is_sel, &label));
        if sresp.clicked() && forced.is_none() && !is_sel {
            *selected = i;
            resp.mark_changed();
        }
        let fg = if st.disabled {
            t.disabled()
        } else if is_sel {
            t.text
        } else {
            theme::mix(t.text_2, t.text, st.hover)
        };
        let p = ui.painter();
        let show_label = !(o.icon.is_some() && (o.icon_only || o.label.is_empty()));
        match (o.icon, show_label) {
            (Some(icon), false) => icons::paint(p, *r, icon, icons::SIZE, fg),
            (Some(icon), true) => {
                let tw = text_width(ui, o.label, theme::control_medium()).min(r.width() - 30.0);
                let start = r.center().x - (tw + 22.0) / 2.0;
                icons::paint(p, Rect::from_center_size(egui::pos2(start + 8.0, r.center().y), Vec2::splat(16.0)), icon, 16.0, fg);
                paint_text_clipped(p, egui::pos2(start + 22.0, r.center().y), Align2::LEFT_CENTER, o.label, theme::control_medium(), fg, tw);
            }
            (None, _) => {
                paint_text_clipped(p, r.center(), Align2::CENTER_CENTER, o.label, theme::control_medium(), fg, r.width() - 8.0);
            }
        }
        if st.focused {
            focus_ring(p, *r, CornerRadius::same(5), &t);
        }
        if !show_label && !o.label.is_empty() {
            tip(sresp, o.label, None);
        }
    }
    resp
}

fn is_sel_pre(i: usize, sel: usize) -> bool {
    i == sel
}

/// Chip: a pill used for categories and filters. Selected chips are filled
/// with the accent.
pub fn chip(ui: &mut Ui, label: &str, selected: bool, forced: Option<Forced>) -> Response {
    let t = theme::tokens(ui.ctx());
    let w = text_width(ui, label, theme::body_medium()) + 20.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, theme::space::CONTROL_H_COMPACT), Sense::click());
    let st = State::from_response(ui, &resp, forced);
    let l = tl(label).into_owned();
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, !st.disabled, selected, &l));
    let cr = CornerRadius::same(theme::radius::CHIP);
    let (fill, fg) = if st.disabled {
        (theme::mix(t.raised, t.panel, 0.4), t.disabled())
    } else if selected {
        // Active chip: a soft ember wash with ember text, quieter than a
        // primary button.
        let soft = t.accent_soft_on(field_fill(ui));
        (if st.pressed { theme::mix(soft, t.accent_fill, 0.2) } else { theme::mix(soft, t.accent_fill, 0.08 * st.hover) }, if t.dark { t.accent } else { t.accent_fill })
    } else if st.pressed {
        (t.pressed(field_fill(ui)), t.text)
    } else {
        let f = field_fill(ui);
        (theme::mix(f, t.hover(f), st.hover), theme::mix(t.text_2, t.text, st.hover))
    };
    let p = ui.painter();
    p.rect_filled(rect, cr, fill);
    p.text(rect.center(), Align2::CENTER_CENTER, shown(label), theme::body_medium(), fg);
    if st.focused {
        focus_ring(p, rect, cr, &t);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A wrapping row of single-select chips. Returns true if the selection
/// changed.
pub fn chip_group(ui: &mut Ui, selected: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for (i, l) in labels.iter().enumerate() {
            if chip(ui, l, *selected == i, None).clicked() && *selected != i {
                *selected = i;
                changed = true;
            }
        }
    });
    changed
}

#[allow(dead_code)] // the owners switch to it this round
/// [`menu_button`] as wide as the room left in the row, within `min..=max`.
pub fn menu_button_fill(ui: &mut Ui, id_salt: &str, selected: usize, options: &[&str], min: f32, max: f32, forced: Option<Forced>) -> Option<usize> {
    let w = (ui.available_width() - 6.0).clamp(min, max);
    menu_button(ui, id_salt, selected, options, w, forced)
}

/// Button showing the current choice with a caret; opens a menu of
/// options. Returns the newly chosen index, if any.
pub fn menu_button(
    ui: &mut Ui,
    id_salt: &str,
    selected: usize,
    options: &[&str],
    width: f32,
    forced: Option<Forced>,
) -> Option<usize> {
    let t = theme::tokens(ui.ctx());
    let id = ui.scope_id().with(("menu-button", id_salt));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, theme::space::CONTROL_H), Sense::click());
    let st = State::from_response(ui, &resp, forced);
    let current = options.get(selected).copied().unwrap_or("");
    let cur = tl(current).into_owned();
    resp.widget_info(|| WidgetInfo::labeled(Role::ComboBox, !st.disabled, &cur));
    let open = is_open(ui.ctx(), id) || forced == Some(Forced::Pressed);
    let cr = CornerRadius::same(theme::radius::CONTROL);
    let fill = field_fill(ui);
    let fill = if st.disabled { fill } else if open { t.pressed(fill) } else { theme::mix(fill, t.hover(fill), st.hover) };
    let p = ui.painter();
    p.rect_filled(rect, cr, fill);
    let fg = if st.disabled { t.disabled() } else { t.text };
    let cut = paint_text_clipped(p, egui::pos2(crate::i18n::start_x(rect, 10.0), rect.center().y), crate::i18n::start_center(), current, theme::control(), fg, rect.width() - 36.0);
    icons::paint(p, Rect::from_center_size(egui::pos2(crate::i18n::end_x(rect, 14.0), rect.center().y), Vec2::splat(14.0)), Icon::CaretDown, 12.0, if st.disabled { fg } else { t.text_2 });
    if st.focused || open {
        focus_ring(p, rect, cr, &t);
    }
    if resp.clicked() && forced.is_none() {
        toggle_open(ui.ctx(), id);
    }
    if forced.is_some() {
        return None;
    }
    if !open {
        tip_if_cut(resp, cut, current);
    }
    let mut chosen = None;
    popover(ui.ctx(), id, rect, Side::Below, width.max(160.0), |ui| {
        for (i, o) in options.iter().enumerate() {
            if MenuItem::new(o).checked(i == selected).show(ui).clicked() {
                chosen = Some(i);
                set_open(ui.ctx(), id, false);
            }
        }
    });
    chosen
}
