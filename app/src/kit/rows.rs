// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The list row: icon, title, secondary line, and trailing actions that
//! appear on hover.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo, Role};

pub struct ListRow<'a> {
    title: &'a str,
    secondary: Option<&'a str>,
    icon: Option<Icon>,
    selected: bool,
    trailing: Vec<(Icon, &'a str)>,
    forced: Option<Forced>,
}

/// What a [`ListRow`] reported: the row itself and the index of the trailing
/// action that was clicked.
pub struct RowOutput {
    pub response: Response,
    pub action: Option<usize>,
}

impl<'a> ListRow<'a> {
    pub fn new(title: &'a str) -> Self {
        Self { title, secondary: None, icon: None, selected: false, trailing: Vec::new(), forced: None }
    }
    pub fn secondary(mut self, s: &'a str) -> Self {
        self.secondary = Some(s).filter(|s| !s.is_empty());
        self
    }
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    /// An icon action shown on hover, with its tooltip name.
    pub fn action(mut self, icon: Icon, name: &'a str) -> Self {
        self.trailing.push((icon, name));
        self
    }
    pub fn forced(mut self, f: Forced) -> Self {
        self.forced = Some(f);
        self
    }

    pub fn show(self, ui: &mut Ui) -> RowOutput {
        let t = theme::tokens(ui.ctx());
        let h = if self.secondary.is_some() { theme::space::ROW_H_BIN } else { theme::space::ROW_H };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
        let st = State::from_response(ui, &resp, self.forced);
        let title = tl(self.title).into_owned();
        let selected = self.selected;
        resp.widget_info(|| WidgetInfo::selected(Role::ListItem, !st.disabled, selected, &title));
        let cr = CornerRadius::same(theme::radius::CONTROL);
        let fill = if self.selected {
            t.accent_soft_on(t.panel)
        } else {
            theme::alpha(t.alt, st.hover)
        };
        let p = ui.painter();
        p.rect_filled(rect, cr, fill);
        if st.focused {
            focus_ring(p, rect, cr, &t);
        }
        let mut x = rect.left() + theme::space::SM;
        if let Some(icon) = self.icon {
            icons::paint(p, Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), Vec2::splat(16.0)), icon, 16.0, t.text_2);
            x += 16.0 + theme::space::SM;
        }
        let show_actions = !self.trailing.is_empty() && (st.hover > 0.0 || st.focused || resp.hovered());
        let actions_w = if show_actions { self.trailing.len() as f32 * (theme::space::CONTROL_H + theme::space::XS) } else { 0.0 };
        let room = (rect.right() - theme::space::SM - actions_w - x).max(1.0);
        let fg = if st.disabled { t.disabled() } else { t.text };
        let ty = if self.secondary.is_some() { rect.top() + 11.0 } else { rect.center().y };
        paint_text_clipped(p, egui::pos2(x, ty), Align2::LEFT_CENTER, self.title, theme::control(), fg, room);
        if let Some(s) = self.secondary {
            paint_text_clipped(p, egui::pos2(x, rect.bottom() - 9.0), Align2::LEFT_CENTER, s, theme::caption(), t.text_2, room);
        }
        let mut action = None;
        if show_actions {
            let mut ax = rect.right() - theme::space::XS;
            for (i, (icon, name)) in self.trailing.iter().enumerate().rev() {
                let r = Rect::from_min_size(egui::pos2(ax - theme::space::CONTROL_H, rect.center().y - theme::space::CONTROL_H / 2.0), Vec2::splat(theme::space::CONTROL_H));
                if IconButton::new(*icon, name).compact().show_at(ui, r).clicked() {
                    action = Some(i);
                }
                ax -= theme::space::CONTROL_H + theme::space::XS;
            }
        }
        RowOutput { response: resp, action }
    }
}

#[allow(dead_code)] // the owners switch to it this round
/// The longest help line a row carries; the rest is in its tooltip.
pub const HELP_MAX: usize = 64;

#[allow(dead_code)] // the owners switch to it this round
/// `text` cut to one help line, and the whole of it when it was cut.
pub fn one_line(text: &str) -> (String, Option<String>) {
    if text.chars().count() <= HELP_MAX {
        return (text.to_owned(), None);
    }
    let cut: String = text.chars().take(HELP_MAX - 1).collect();
    (format!("{}…", cut.trim_end()), Some(text.to_owned()))
}

#[allow(dead_code)] // the owners switch to it this round
/// One line of help under a row, in the control column. `more` (or the
/// whole text, when it had to be cut) shows on hover.
pub fn help_row(ui: &mut Ui, text: &str, more: Option<&str>) {
    let t = theme::tokens(ui.ctx());
    let (shown, cut) = one_line(text);
    let tip = more.map(str::to_owned).or(cut);
    ui.add_space(-4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 14.0;
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(label_width(ui.ctx()));
        let r = ui.add(egui::Label::new(egui::RichText::new(shown).font(theme::caption()).color(t.text_2)).truncate());
        if let Some(tip) = tip {
            r.on_hover_text(tip);
        }
    });
}

#[cfg(test)]
mod help_tests {
    use super::*;

    #[test]
    fn a_help_line_is_cut_to_one_line_with_the_whole_text_kept() {
        assert_eq!(one_line("Short."), ("Short.".to_owned(), None));
        let long = "x".repeat(HELP_MAX + 20);
        let (shown, whole) = one_line(&long);
        assert_eq!(shown.chars().count(), HELP_MAX);
        assert!(shown.ends_with('…'));
        assert_eq!(whole.as_deref(), Some(long.as_str()));
    }
}
