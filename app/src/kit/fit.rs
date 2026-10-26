// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Content that runs out of room. Popups and menus are capped to the window
//! and scroll; panels scroll down; strips that must stay one line scroll
//! sideways; single-line names end in an ellipsis and show in full on
//! hover. When everything fits, none of this changes what is drawn (the
//! scroll bars float and only show while they are needed).
//!
//! Every scroll area notes where its viewport is this frame, so tests can
//! tell text cut by a scroll viewport (fine: it scrolls into view) from
//! text cut with no way to see it.

use super::*;
use eframe::egui::{scroll_area::ScrollAreaOutput, Id, ScrollArea};

/// Room kept between a popup and the window's edge.
pub const EDGE: f32 = 8.0;

/// Note a scroll viewport for this frame. `horizontal` is the axis it scrolls.
pub fn note_scroll(ctx: &egui::Context, rect: Rect, horizontal: bool) {
    // The pass number first: asking for it while the data is locked would
    // wait forever.
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let e = d.get_temp_mut_or_default::<(u64, Vec<(Rect, bool)>)>(Id::unique("kit-scrolled"));
        if e.0 != pass {
            *e = (pass, vec![]);
        }
        e.1.push((rect, horizontal));
    });
}

/// The scroll viewports of the last frame, with the axis each scrolls.
pub fn scrolled_rects(ctx: &egui::Context) -> Vec<(Rect, bool)> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Vec<(Rect, bool)>)>(Id::unique("kit-scrolled"))).filter(|(p, _)| p + 1 >= pass).map(|(_, v)| v).unwrap_or_default()
}

/// `show` for a scroll area that also notes its viewport (see [`note_scroll`]).
pub trait ShowNoted {
    /// A vertical scroll area.
    fn show_v<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R>;
    /// A horizontal scroll area.
    fn show_h<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R>;
}

impl ShowNoted for ScrollArea {
    fn show_v<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R> {
        let out = self.show(ui, add);
        note_scroll(ui.ctx(), out.inner_rect, false);
        out
    }
    fn show_h<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R> {
        let out = self.show(ui, add);
        note_scroll(ui.ctx(), out.inner_rect, true);
        out
    }
}

/// Inside an egui drop-down list (`ComboBox::show_ui`): notes the list's
/// own scroll viewport, which egui keeps to itself.
pub fn note_list(ui: &Ui) {
    note_scroll(ui.ctx(), ui.clip_rect(), false);
}

/// The height a popup that starts at `top` may take before it scrolls: down
/// to the window's bottom edge; when that leaves little room, the whole
/// window's height (the popup then moves up to stay on screen).
pub fn popup_max_height(ctx: &egui::Context, top: f32) -> f32 {
    let screen = ctx.content_rect();
    let below = screen.bottom() - EDGE - top;
    if below >= 240.0 { below } else { (screen.height() - 2.0 * EDGE).max(40.0) }
}

/// The content of a menu, sub-menu or popup that egui draws (`menu_button`,
/// `Popup`): capped to the window and scrolling when taller.
pub fn popup_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let max_h = popup_max_height(ui.ctx(), ui.cursor().top());
    // The minimum too: an area lays its content out in last frame's size,
    // which would otherwise keep a menu at the height it first had.
    ScrollArea::vertical().id_salt("kit-popup-body").max_height(max_h).min_scrolled_height(max_h).auto_shrink([true, true]).show_v(ui, add).inner
}

/// A panel's content scrolling down when taller than the panel. The width
/// stays the panel's.
pub fn scroll_panel<R>(ui: &mut Ui, id_salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui) -> R) -> R {
    ScrollArea::vertical().id_salt(id_salt).auto_shrink([false, true]).show_v(ui, add).inner
}

/// A row that must stay one line (a toolbar, a tab strip): it scrolls
/// sideways, with the wheel too, when wider than its space.
pub fn scroll_strip<R>(ui: &mut Ui, id_salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui) -> R) -> R {
    // The chat's thin bar, floating: no height added to a row that fits.
    let before = ui.spacing().scroll;
    crate::theme::thin_scroll(&mut ui.spacing_mut().scroll);
    let out = ScrollArea::horizontal().id_salt(id_salt).auto_shrink([true, true]).show_h(ui, |ui| ui.horizontal(add).inner).inner;
    ui.spacing_mut().scroll = before;
    out
}

/// Something wider than its space that must keep its shape (a table, a
/// row of strips): it scrolls sideways.
pub fn scroll_wide<R>(ui: &mut Ui, id_salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui) -> R) -> R {
    ScrollArea::horizontal().id_salt(id_salt).auto_shrink([false, true]).show_h(ui, add).inner
}

/// A single-line label that ends in an ellipsis when it has no room and
/// shows in full on hover.
pub fn label_trunc(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(egui::Label::new(text).truncate())
}

/// Content kept within `max_w` across, taking only the width it uses (so
/// what follows sits right after it): a name that must leave room for a tag.
pub fn within<R>(ui: &mut Ui, max_w: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let h = ui.available_height().max(ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(egui::vec2(max_w.max(16.0), h), egui::Layout::left_to_right(egui::Align::Center), add).inner
}

/// A row with a name at the start and controls at the end: the controls
/// keep their room and the name ends in an ellipsis before them (whole on
/// hover). When even that leaves the name no room, the name takes a line of
/// its own and the controls wrap under it, still at the end and in the same
/// order. Returns what `end` returns.
pub fn name_row<R>(ui: &mut Ui, name: impl Into<egui::WidgetText>, end: impl FnOnce(&mut Ui) -> R) -> R {
    let name: egui::WidgetText = name.into();
    // The controls' width on one line, from the last frame that had them so.
    let key = ui.id().with(("name-row", name.text().to_owned()));
    let end_w: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(0.0);
    if end_w > 0.0 && end_w + 64.0 > ui.available_width() {
        label_trunc(ui, name);
        return ui.with_layout(egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true), end).inner;
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = end(ui);
            let w = ui.min_rect().width();
            ui.data_mut(|d| d.insert_temp(key, w));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| label_trunc(ui, name));
            r
        })
        .inner
    })
    .inner
}

/// Hover text with the whole of a name that was painted cut short.
pub fn tip_if_cut(resp: Response, cut: bool, full: &str) -> Response {
    if cut { resp.on_hover_text(shown(full)) } else { resp }
}
