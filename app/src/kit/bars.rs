// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tab bar, toolbar group and menu button group.

use super::*;
use crate::icons::Icon;
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo, Role};

/// The one tab bar: text tabs in a row, the active one in `text` over a 2 px
/// accent underline, a hairline under the whole bar. Use it for the tabs of a
/// panel or screen; use [`segmented`] for a mode switch inside a tab.
/// Returns true when the selection changed.
pub fn tab_bar(ui: &mut Ui, id_salt: &str, selected: &mut usize, labels: &[&str]) -> bool {
    let t = theme::tokens(ui.ctx());
    let h = theme::space::CONTROL_H_LARGE;
    let base_id = ui.scope_id().with(("tab-bar", id_salt));
    let mut changed = false;
    let start = ui.cursor().min;
    let mut right = start.x;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::XS;
        for (i, label) in labels.iter().enumerate() {
            let w = text_width(ui, label, theme::control_medium()) + 2.0 * theme::space::MD;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
            let st = State::from_response(ui, &resp, None);
            let is_sel = i == *selected;
            let l = tl(label).into_owned();
            resp.widget_info(|| WidgetInfo::selected(Role::Tab, !st.disabled, is_sel, &l));
            let fg = if st.disabled {
                t.disabled()
            } else if is_sel {
                t.text
            } else {
                theme::mix(t.text_2, t.text, st.hover)
            };
            let p = ui.painter();
            p.text(rect.center() - Vec2::new(0.0, 1.0), Align2::CENTER_CENTER, shown(label), theme::control_medium(), fg);
            if is_sel {
                p.rect_filled(Rect::from_min_max(egui::pos2(rect.left() + 4.0, rect.bottom() - 2.0), egui::pos2(rect.right() - 4.0, rect.bottom())), CornerRadius::same(1), t.accent);
            }
            if st.focused {
                focus_ring(p, rect, CornerRadius::same(theme::radius::CONTROL), &t);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !is_sel {
                *selected = i;
                changed = true;
            }
            right = rect.right();
        }
    });
    let _ = base_id;
    let y = start.y + h;
    let line_to = ui.max_rect().right().max(right);
    ui.painter().hline(start.x..=line_to, y, egui::Stroke::new(t.hairline(ui.ctx()), t.line));
    // The accent underline sits over the hairline.
    ui.add_space(theme::space::XS);
    changed
}

/// A group of related toolbar controls with a small caption under or beside
/// them (shown as a tooltip on the whole group when `compact`). Groups are
/// separated by a hairline, so a toolbar reads as a few labelled clusters
/// instead of one long strip of icons.
pub fn toolbar_group<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::tokens(ui.ctx());
    let out = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::XS;
        add(ui)
    });
    let r = out.response.rect;
    if !label.is_empty() {
        ui.interact(r, ui.id().with(("toolbar-group", label)), Sense::hover()).on_hover_text(shown(label));
    }
    // Divider after the group.
    let x = r.right() + theme::space::SM / 2.0;
    ui.painter().vline(x, (r.top() + 2.0)..=(r.bottom() - 2.0), t.hair(ui.ctx()));
    ui.add_space(theme::space::SM);
    out.inner
}

/// A labelled section inside a menu: a capital micro heading, then the items
/// `add` draws.
pub fn menu_group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    menu_heading(ui, title);
    add(ui)
}

/// A button that opens a menu of arbitrary [`MenuItem`]s (unlike
/// [`menu_button`], which picks one string from a list). `add` draws the
/// items and calls `close_menu` when one is chosen. Use for "Add", "More"
/// and similar command menus.
pub fn menu_group_button(ui: &mut Ui, id_salt: &str, label: &str, icon: Option<Icon>, add: impl FnOnce(&mut Ui)) -> Response {
    let id = ui.scope_id().with(("menu-group-button", id_salt));
    let mut b = Button::secondary(label).trailing(Icon::CaretDown);
    if let Some(i) = icon {
        b = b.icon(i);
    }
    let resp = b.show(ui);
    if resp.clicked() {
        toggle_open(ui.ctx(), id);
    }
    let w = (resp.rect.width()).max(180.0);
    popover(ui.ctx(), id, resp.rect, Side::Below, w, add);
    resp
}

/// Close the menu opened by [`menu_group_button`] with the same `id_salt`.
pub fn close_menu(ui: &Ui, id_salt: &str) {
    set_open(ui.ctx(), ui.scope_id().with(("menu-group-button", id_salt)), false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    fn frame(ctx: &egui::Context, events: Vec<Event>, mut run: impl FnMut(&mut Ui)) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            run(ui);
        });
        out.textures_delta.clear();
    }

    /// A click on the second tab selects it; the other kit rows and groups
    /// draw without panicking and report no action until clicked.
    #[test]
    fn tab_bar_selects_and_rows_draw() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut sel = 0usize;
        let draw = |ui: &mut Ui, sel: &mut usize| {
            tab_bar(ui, "t", sel, &["Media", "Effects"]);
            let out = ListRow::new("Interview").secondary("00:12").icon(Icon::FilmStrip).action(Icon::Check, "Use").show(ui);
            assert!(out.action.is_none());
            toolbar_group(ui, "Edit", |ui| {
                Button::ghost("Cut").show(ui);
            });
            menu_group_button(ui, "m", "Add", None, |ui| {
                menu_group(ui, "Media", |ui| {
                    MenuItem::new("Import").show(ui);
                });
            });
        };
        frame(&ctx, vec![], |ui| draw(ui, &mut sel));
        // The second tab sits right of the first (about 60 px each at the top left).
        let pos = egui::pos2(100.0, 14.0);
        let press = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, vec![Event::PointerMoved(pos)], |ui| draw(ui, &mut sel));
        frame(&ctx, vec![press(true)], |ui| draw(ui, &mut sel));
        frame(&ctx, vec![press(false)], |ui| draw(ui, &mut sel));
        assert_eq!(sel, 1, "clicking the second tab selects it");
    }
}
