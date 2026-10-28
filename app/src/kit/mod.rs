// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared components. Each is built once here and reused by every screen,
//! so spacing, states and motion stay identical everywhere (spec §3).
//!
//! Every widget computes a [`State`] from its response and paints from that
//! state alone. The gallery passes forced states to show each variant
//! side by side.

pub mod anim;
pub mod button;
pub mod colour;
pub mod controls;
pub mod empty;
pub mod field;
pub mod fit;
pub mod popover;
pub mod section;
pub mod slider;
pub mod tiles;
pub mod toast;

pub use anim::*;
pub use button::*;
pub use colour::*;
pub use controls::*;
pub use empty::*;
pub use field::*;
pub use fit::*;
pub use popover::*;
pub use section::*;
pub use slider::*;
pub use tiles::*;
pub use toast::*;

pub use crate::i18n::{shown, t as tl};
use crate::theme::{self, Tokens};
use eframe::egui::{self, Color32, CornerRadius, Rect, Response, Stroke, StrokeKind, Ui};

/// Interaction state a widget paints from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    /// Animated hover amount, 0..1.
    pub hover: f32,
    pub pressed: bool,
    pub focused: bool,
    pub disabled: bool,
    pub selected: bool,
}

/// A state the gallery forces onto a widget instead of reading input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forced {
    Rest,
    Hover,
    Pressed,
    Focused,
    Disabled,
}

impl Forced {
    pub const ALL: [Forced; 5] =
        [Forced::Rest, Forced::Hover, Forced::Pressed, Forced::Focused, Forced::Disabled];
    pub fn label(self) -> &'static str {
        match self {
            Forced::Rest => "Rest",
            Forced::Hover => "Hover",
            Forced::Pressed => "Pressed",
            Forced::Focused => "Focus",
            Forced::Disabled => "Disabled",
        }
    }
}

impl State {
    pub fn from_response(ui: &Ui, resp: &Response, forced: Option<Forced>) -> Self {
        if let Some(f) = forced {
            return Self::forced(f);
        }
        let hovered = resp.hovered() && ui.is_enabled();
        State {
            hover: hover_t(ui.ctx(), resp.id, hovered),
            pressed: resp.is_pointer_button_down_on(),
            focused: resp.has_focus() && keyboard_focus_visible(ui.ctx()),
            disabled: !ui.is_enabled(),
            selected: false,
        }
    }
    pub fn forced(f: Forced) -> Self {
        State {
            hover: if matches!(f, Forced::Hover | Forced::Pressed) { 1.0 } else { 0.0 },
            pressed: f == Forced::Pressed,
            focused: f == Forced::Focused,
            disabled: f == Forced::Disabled,
            selected: false,
        }
    }
}

/// Whether the user has been navigating with the keyboard. Focus rings are
/// shown for keyboard focus; a mouse click on a button does not leave a ring.
pub fn keyboard_focus_visible(ctx: &egui::Context) -> bool {
    let id = egui::Id::unique("studio-kbd-focus");
    let (tab, clicked) = ctx.input(|i| {
        (
            i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, key, .. }
                if matches!(key, egui::Key::Tab | egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::ArrowLeft | egui::Key::ArrowRight))),
            i.pointer.any_pressed(),
        )
    });
    ctx.data_mut(|d| {
        let v = d.get_temp_mut_or_insert_with(id, || false);
        if tab {
            *v = true;
        } else if clicked {
            *v = false;
        }
        *v
    })
}

/// 2 px accent focus ring just outside `rect` (spec §8.2).
pub fn focus_ring(painter: &egui::Painter, rect: Rect, radius: impl Into<CornerRadius>, t: &Tokens) {
    painter.rect_stroke(rect, radius, Stroke::new(if crate::access::high_contrast() { 3.0 } else { 1.5 }, t.accent), StrokeKind::Inside);
}

/// The fill used for input fields on the current surface. Section cards set
/// this to `panel` so fields read as inset on the raised card.
pub fn field_fill(ui: &Ui) -> Color32 {
    ui.visuals().faint_bg_color
}

/// Make child widgets of `ui` sit on a raised card.
pub fn on_card(ui: &mut Ui) {
    let t = theme::tokens(ui.ctx());
    ui.visuals_mut().faint_bg_color = t.panel;
}

/// Tooltip with an optional shortcut shown after the name (spec §1: icon
/// buttons always show their shortcut).
pub fn tip(resp: Response, name: &str, shortcut: Option<&str>) -> Response {
    let name = name.to_owned();
    let shortcut = shortcut.map(str::to_owned);
    resp.on_hover_ui(move |ui| tooltip_body(ui, &name, shortcut.as_deref()))
}

pub fn tooltip_body(ui: &mut Ui, name: &str, shortcut: Option<&str>) {
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new(shown(name)).font(theme::body()).color(t.text));
        if let Some(s) = shortcut.filter(|s| !s.is_empty()) {
            shortcut_badge(ui, s);
        }
    });
}

/// A small keycap-style badge, e.g. in tooltips and menus.
pub fn shortcut_badge(ui: &mut Ui, s: &str) {
    let t = theme::tokens(ui.ctx());
    ui.label(egui::RichText::new(shown(s)).font(theme::caption()).color(t.text_2));
}

/// Paint text truncated with an ellipsis to fit `max_width`. Returns
/// whether it was truncated (callers show the full text in a tooltip).
pub fn paint_text_clipped(
    painter: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> bool {
    let mut job = egui::text::LayoutJob::simple_singleline(shown(text).into_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(1.0));
    let galley = painter.layout_job(job);
    let truncated = galley.elided;
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    truncated
}

/// Measure single-line text.
pub fn text_width(ui: &Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter().layout_no_wrap(shown(text).into_owned(), font, Color32::WHITE).size().x
}

/// Snap a coordinate to the physical pixel grid so hairlines stay sharp.
pub fn snap(ctx: &egui::Context, v: f32) -> f32 {
    let ppp = ctx.pixels_per_point();
    (v * ppp).round() / ppp
}

/// Horizontal hairline across `rect` at `y`.
pub fn hline(ui: &Ui, x: std::ops::RangeInclusive<f32>, y: f32) {
    let t = theme::tokens(ui.ctx());
    let y = snap(ui.ctx(), y) + t.hairline(ui.ctx()) / 2.0;
    ui.painter().hline(x, y, t.hair(ui.ctx()));
}

pub fn vline(ui: &Ui, x: f32, y: std::ops::RangeInclusive<f32>) {
    let t = theme::tokens(ui.ctx());
    let x = snap(ui.ctx(), x) + t.hairline(ui.ctx()) / 2.0;
    ui.painter().vline(x, y, t.hair(ui.ctx()));
}

/// Translate and break a paragraph into lines of at most `max_w`. Right-to-left
/// text is wrapped here, in reading order, because egui would otherwise wrap
/// the already reordered line at the wrong end.
pub fn wrapped(painter: &egui::Painter, text: &str, font: egui::FontId, max_w: f32) -> String {
    let text = tl(text);
    if crate::i18n::is_rtl() || studio_i18n::rtl::has_rtl(&text) {
        let fits = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x <= max_w;
        studio_i18n::rtl::wrap(&text, crate::i18n::is_rtl(), fits)
    } else {
        text.into_owned()
    }
}

/// Add `label` (its text may be in drawing order for right-to-left) and tell
/// a screen reader the logical `text` instead.
pub fn label_a11y(ui: &mut Ui, label: egui::Label, logical: &str) -> Response {
    let resp = ui.add(label);
    ui.ctx().accesskit_node_builder(resp.id, |b| b.set_value(logical));
    resp
}
