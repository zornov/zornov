// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Number field, text field and the inspector field row (spec §3).

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, TextEdit, Vec2, WidgetInfo, Role};
use std::ops::RangeInclusive;

// --- expressions ---------------------------------------------------------

/// Evaluate what the user typed into a number field. A leading `+`, `*` or
/// `/` applies to the current value (`+10`, `*2`); otherwise the text is a
/// plain expression (`12.5`, `1920/2`, `-3`). Units after the number are
/// ignored so `50%` or `12 px` work.
pub fn eval_expression(input: &str, current: f32) -> Option<f32> {
    let s: String = input
        .trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c == '%' || c == '°' || c == '×' || c.is_whitespace())
        .replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let expr = match s.chars().next()? {
        '+' | '*' | '/' => format!("({current}){s}"),
        _ => s,
    };
    let mut p = Parser { s: expr.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && v.is_finite()).then_some(v as f32)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(c @ (b'+' | b'-')) = self.peek() {
            self.i += 1;
            let r = self.term()?;
            v = if c == b'+' { v + r } else { v - r };
        }
        Some(v)
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        while let Some(c @ (b'*' | b'/')) = self.peek() {
            self.i += 1;
            let r = self.factor()?;
            v = if c == b'*' { v * r } else { v / r };
        }
        Some(v)
    }
    fn factor(&mut self) -> Option<f64> {
        match self.peek()? {
            b'-' => {
                self.i += 1;
                Some(-self.factor()?)
            }
            b'(' => {
                self.i += 1;
                let v = self.expr()?;
                (self.peek()? == b')').then(|| self.i += 1)?;
                Some(v)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
            }
        }
    }
}

// --- number field --------------------------------------------------------

pub struct NumberField<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    decimals: usize,
    unit: &'a str,
    name: &'a str,
    width: f32,
    forced: Option<Forced>,
    /// Pixels per unit when dragging the field itself.
    drag_speed: f32,
}

impl<'a> NumberField<'a> {
    pub fn new(name: &'a str, value: &'a mut f32) -> Self {
        Self {
            value,
            range: f32::MIN..=f32::MAX,
            decimals: 0,
            unit: "",
            name,
            width: 64.0,
            forced: None,
            drag_speed: 1.0,
        }
    }
    pub fn range(mut self, r: RangeInclusive<f32>) -> Self {
        self.range = r;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
    pub fn unit(mut self, u: &'a str) -> Self {
        self.unit = u;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn forced(mut self, f: Forced) -> Self {
        self.forced = Some(f);
        self
    }
    pub fn drag_speed(mut self, s: f32) -> Self {
        self.drag_speed = s;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let size = Vec2::new(self.width, theme::space::CONTROL_H);
        let id = ui.next_auto_id().with(self.name);
        let edit_id = id.with("edit");
        let editing: Option<String> = ui.data(|d| d.get_temp(edit_id));
        let cr = CornerRadius::same(theme::radius::CONTROL);

        if let Some(mut text) = editing {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect_filled(rect, cr, field_fill(ui));
            let te = TextEdit::singleline(&mut text)
                .id(edit_id.with("te"))
                .font(theme::number())
                .align(Align2::RIGHT_CENTER)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .desired_width(rect.width() - 16.0);
            let out = ui.put(rect.shrink2(Vec2::new(8.0, 0.0)), te);
            // What follows in a row goes after the whole field.
            ui.advance_cursor_after_rect(rect);
            focus_ring(ui.painter(), rect, cr, &t);
            let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            let mut resp = out.clone();
            if esc {
                ui.data_mut(|d| d.remove::<String>(edit_id));
            } else if enter || out.lost_focus() {
                if let Some(v) = eval_expression(&text, *self.value) {
                    let v = v.clamp(*self.range.start(), *self.range.end());
                    if v != *self.value {
                        *self.value = v;
                        resp.mark_changed();
                    }
                }
                ui.data_mut(|d| d.remove::<String>(edit_id));
            } else {
                ui.data_mut(|d| d.insert_temp(edit_id, text));
            }
            return resp;
        }

        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let st = State::from_response(ui, &resp, self.forced);
        let value = *self.value;
        let name = tl(self.name).into_owned();
        resp.widget_info(|| WidgetInfo::text_edit(!st.disabled, format!("{value}"), format!("{value}"), &name));
        if self.forced.is_none() && ui.is_enabled() {
            if resp.dragged() {
                let m = ui.input(|i| i.modifiers);
                let k = if m.shift { 10.0 } else if m.alt { 0.1 } else { 1.0 };
                let dv = resp.drag_delta().x * k / self.drag_speed.max(0.001);
                if dv != 0.0 {
                    *self.value = (*self.value + dv).clamp(*self.range.start(), *self.range.end());
                    resp.mark_changed();
                }
            }
            let start_edit = resp.clicked() || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if start_edit {
                let text = format!("{:.*}", self.decimals, *self.value);
                ui.data_mut(|d| d.insert_temp(edit_id, text));
                ui.memory_mut(|m| m.request_focus(edit_id.with("te")));
                // Select everything so typing replaces the value.
                let te_id = edit_id.with("te");
                let mut state = egui::text_edit::TextEditState::load(ui.ctx(), te_id).unwrap_or_default();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(64),
                )));
                state.store(ui.ctx(), te_id);
                ui.ctx().request_repaint();
            }
        }
        if ui.is_rect_visible(rect) {
            let fill = field_fill(ui);
            let fill = if st.disabled { fill } else { theme::mix(fill, t.hover(fill), st.hover) };
            let p = ui.painter();
            p.rect_filled(rect, cr, fill);
            let text = format_value(*self.value, self.decimals, self.unit);
            let color = if st.disabled { t.disabled() } else { t.text };
            paint_text_clipped(p, egui::pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, &text, theme::number(), color, rect.width() - 12.0);
            if st.focused || self.forced == Some(Forced::Focused) {
                focus_ring(p, rect, cr, &t);
            }
        }
        resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
    }
}

// --- text field ----------------------------------------------------------

/// Single-line text field with an optional leading icon (search, names).
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, icon: Option<Icon>, width: f32, forced: Option<Forced>) -> Response {
    text_field_ex(ui, text, hint, icon, width, forced, false)
}

#[allow(dead_code)] // the owners switch to it this round
/// A [`text_field`] whose text shows as dots (stream keys, tokens).
pub fn secret_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    text_field_ex(ui, text, hint, None, width, None, true)
}

fn text_field_ex(ui: &mut Ui, text: &mut String, hint: &str, icon: Option<Icon>, width: f32, forced: Option<Forced>, masked: bool) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(48.0), theme::space::CONTROL_H), Sense::hover());
    let cr = CornerRadius::same(theme::radius::CONTROL);
    let left = if icon.is_some() { 30 } else { 10 };
    let right = if text.is_empty() { 10 } else { 28 };
    let id = ui.scope_id().with(("text-field", hint));
    let te = TextEdit::singleline(text)
        .id(id)
        .hint_text(egui::RichText::new(shown(hint)).color(t.text_2).font(theme::control()))
        .font(theme::control())
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .align(crate::i18n::start_center())
        .password(masked)
        .desired_width(rect.width() - (left + right) as f32);
    // Paint the fill first, then the editor on top. The editor gets an
    // inset rect so text clears the icon and the clear button.
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = Rect::from_min_max(rect.min + Vec2::new(left as f32, 0.0), rect.max - Vec2::new(right as f32, 0.0));
    let resp = ui.put(inner, te);
    let st = State::from_response(ui, &resp, forced);
    // A screen reader needs the field's purpose (the hint) as its name.
    let (name, current) = (tl(hint).into_owned(), text.clone());
    resp.widget_info(|| WidgetInfo::text_edit(!st.disabled, current.clone(), current.clone(), name.clone()));
    ui.ctx().accesskit_node_builder(resp.id, |b| b.set_label(name.as_str()));
    let focused = resp.has_focus() || forced == Some(Forced::Focused);
    let fill = field_fill(ui);
    let fill = if st.disabled { fill } else { theme::mix(fill, t.hover(fill), st.hover) };
    ui.painter().set(bg, egui::epaint::RectShape::filled(rect, cr, fill));
    if let Some(icon) = icon {
        let r = Rect::from_center_size(egui::pos2(rect.left() + 17.0, rect.center().y), Vec2::splat(icons::SIZE));
        icons::paint(ui.painter(), r, icon, icons::SIZE, t.text_2);
    }
    if focused {
        focus_ring(ui.painter(), rect, cr, &t);
    }
    if !text.is_empty() {
        let r = Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), Vec2::splat(20.0));
        if IconButton::new(Icon::X, "Clear").size(20.0, 12.0).show_at(ui, r).clicked() {
            text.clear();
        }
    }
    // The inset editor moved the row's cursor back inside the field: what
    // follows in a row goes after the whole field (its clear button too).
    ui.advance_cursor_after_rect(rect);
    resp
}

// --- keyframe diamond ----------------------------------------------------

/// Keyframe toggle: outline diamond when the property has no key at the
/// playhead, filled accent when it does.
pub fn keyframe_toggle(ui: &mut Ui, on: &mut bool, forced: Option<Forced>) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(20.0, theme::space::CONTROL_H), Sense::click());
    if resp.clicked() && forced.is_none() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = theme::tokens(ui.ctx());
    let st = State::from_response(ui, &resp, forced);
    let on_v = *on;
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, !st.disabled, on_v, tl("Keyframe")));
    let p = ui.painter();
    if st.hover > 0.0 {
        p.rect_filled(Rect::from_center_size(rect.center(), Vec2::splat(20.0)), CornerRadius::same(4), theme::alpha(t.raised, st.hover));
    }
    let (icon, color) = if *on {
        (Icon::DiamondFill, t.accent)
    } else {
        (Icon::Diamond, theme::mix(t.text_2, t.text, st.hover))
    };
    icons::paint(p, rect, icon, 12.0, if st.disabled { t.disabled() } else { color });
    if st.focused {
        focus_ring(p, Rect::from_center_size(rect.center(), Vec2::splat(20.0)), CornerRadius::same(4), &t);
    }
    tip(resp, if on_v { "Remove keyframe" } else { "Add keyframe" }, None)
}

// --- field row -----------------------------------------------------------

/// What happened in a field row this frame.
#[derive(Default, Clone, Copy)]
pub struct RowOutput {
    pub changed: bool,
    /// The user is mid-gesture (dragging the label, slider or field).
    pub dragging: bool,
    pub keyframe_toggled: bool,
}

/// Label column width in field rows.
pub const LABEL_W: f32 = 96.0;
/// The label column of a settings page, where the room is there.
pub const LABEL_W_WIDE: f32 = 160.0;

/// The label column in force: [`LABEL_W`], or what a page set with
/// [`set_label_width`].
pub fn label_width(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::new("kit-label-w"))).unwrap_or(LABEL_W)
}

pub fn set_label_width(ctx: &egui::Context, w: f32) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("kit-label-w"), w));
}

/// Inspector row: label · slider · value field · reset · keyframe.
pub struct FieldRow<'a> {
    label: &'a str,
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    default: f32,
    decimals: usize,
    unit: &'a str,
    step: f32,
    slider: bool,
    keyframe: Option<&'a mut bool>,
    forced: Option<Forced>,
    field_w: f32,
    /// Label scrub speed: units per pixel.
    scrub: f32,
}

impl<'a> FieldRow<'a> {
    pub fn new(label: &'a str, value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        let default = (*range.start()).max(0.0).min(*range.end());
        Self {
            label,
            value,
            range,
            default,
            decimals: 0,
            unit: "",
            step: 0.0,
            slider: true,
            keyframe: None,
            forced: None,
            field_w: 60.0,
            scrub: 1.0,
        }
    }
    pub fn default(mut self, d: f32) -> Self {
        self.default = d;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
    pub fn unit(mut self, u: &'a str) -> Self {
        self.unit = u;
        self
    }
    pub fn step(mut self, s: f32) -> Self {
        self.step = s;
        self
    }
    pub fn no_slider(mut self) -> Self {
        self.slider = false;
        self
    }
    pub fn keyframe(mut self, k: &'a mut bool) -> Self {
        self.keyframe = Some(k);
        self
    }
    pub fn forced(mut self, f: Forced) -> Self {
        self.forced = Some(f);
        self
    }
    pub fn field_width(mut self, w: f32) -> Self {
        self.field_w = w;
        self
    }
    pub fn scrub(mut self, units_per_px: f32) -> Self {
        self.scrub = units_per_px;
        self
    }

    pub fn show(self, ui: &mut Ui) -> RowOutput {
        let t = theme::tokens(ui.ctx());
        let mut out = RowOutput::default();
        let h = theme::space::CONTROL_H;
        let full = ui.available_width();
        let (row, row_resp) = ui.allocate_exact_size(Vec2::new(full, h), Sense::hover());
        let row_hovered = row_resp.contains_pointer() || self.forced == Some(Forced::Hover);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 0.0;

        // Label: drag to scrub, double-click to reset. Narrow panels give
        // the label a little less room so the slider survives.
        let narrow = full < 260.0;
        let label_w = if narrow { 80.0 } else { label_width(ui.ctx()) };
        // Rows without a slider have room for the full field.
        let field_w = if narrow && self.slider { self.field_w.min(56.0) } else { self.field_w.max(if self.slider { 0.0 } else { 72.0 }) };
        // Never narrower than the value it shows: the slider gives way.
        let need = text_width(ui, &format_value(*self.value, self.decimals, self.unit), theme::number()) + 16.0;
        let field_w = field_w.max(need.min(full - label_w - 40.0));
        let (lrect, mut lresp) = ui.allocate_exact_size(Vec2::new(label_w, h), Sense::click_and_drag());
        let non_default = (*self.value - self.default).abs() > f32::EPSILON;
        if self.forced.is_none() && ui.is_enabled() {
            if lresp.dragged() {
                let m = ui.input(|i| i.modifiers);
                let k = if m.shift { 10.0 } else if m.alt { 0.1 } else { 1.0 };
                let dv = lresp.drag_delta().x * self.scrub * k;
                if dv != 0.0 {
                    *self.value = (*self.value + dv).clamp(*self.range.start(), *self.range.end());
                    if self.step > 0.0 {
                        *self.value = (*self.value / self.step).round() * self.step;
                    }
                    out.changed = true;
                }
                out.dragging = true;
            }
            if lresp.double_clicked() && non_default {
                *self.value = self.default;
                out.changed = true;
            }
        }
        let lst = State::from_response(ui, &lresp, self.forced);
        let label_color = if lst.disabled { t.disabled() } else { theme::mix(t.text_2, t.text, lst.hover * 0.6) };
        let truncated = paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(lrect, 0.0), lrect.center().y), crate::i18n::start_center(), self.label, theme::body(), label_color, label_w - 8.0);
        if lst.focused {
            focus_ring(ui.painter(), lrect, CornerRadius::same(4), &t);
        }
        lresp = lresp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        let label = tl(self.label).into_owned();
        lresp.widget_info(|| WidgetInfo::labeled(Role::Label, true, &label));
        if truncated {
            lresp.on_hover_text(shown(self.label));
        }

        let tail = field_w + 20.0 + 20.0;
        let middle = (full - label_w - tail).max(0.0);
        if self.slider && middle > 40.0 {
            let mut s = Slider::new(self.label, self.value, self.range.clone())
                .default(self.default)
                .decimals(self.decimals)
                .unit(self.unit)
                .step(self.step)
                .width(middle - 8.0);
            if let Some(f) = self.forced {
                s = s.forced(f);
            }
            let r = s.show(ui);
            out.changed |= r.changed();
            out.dragging |= r.dragged();
            ui.add_space(theme::space::SM);
        } else {
            ui.add_space(middle);
        }
        let mut nf = NumberField::new(self.label, self.value)
            .range(self.range.clone())
            .decimals(self.decimals)
            .unit(self.unit)
            .width(field_w)
            .drag_speed(1.0 / self.scrub.max(0.0001));
        if let Some(f) = self.forced {
            nf = nf.forced(if f == Forced::Hover { Forced::Rest } else { f });
        }
        let r = nf.show(ui);
        out.changed |= r.changed();
        out.dragging |= r.dragged();

        // Reset: shown on row hover when the value is not the default. The
        // slot is always reserved so columns stay aligned.
        let non_default = (*self.value - self.default).abs() > f32::EPSILON;
        let (rrect, _) = ui.allocate_exact_size(Vec2::new(20.0, h), Sense::hover());
        if non_default && (row_hovered || self.forced.is_some()) && self.forced != Some(Forced::Disabled) {
            let b = IconButton::new(Icon::ArrowCounterClockwise, "Reset to default").size(20.0, 12.0);
            if b.show_at(ui, rrect).clicked() && self.forced.is_none() {
                *self.value = self.default;
                out.changed = true;
            }
        }
        if let Some(k) = self.keyframe {
            let r = keyframe_toggle(ui, k, self.forced.filter(|f| *f == Forced::Disabled));
            out.keyframe_toggled = r.changed();
        } else {
            ui.add_space(theme::space::XL);
        }
        out
    }
}

/// The label column of a [`FieldRow`] at this width: a narrow panel gives
/// the label less room.
pub fn label_col(ui: &Ui) -> f32 {
    if ui.available_width() < 260.0 { 80.0 } else { label_width(ui.ctx()) }
}

#[allow(dead_code)] // the owners switch to it this round
/// What a row of the inspector keeps at its end for the reset and keyframe
/// buttons.
pub const ROW_END_SLOTS: f32 = 40.0;

/// A row with a label and arbitrary content, aligned with field rows (on the
/// same label column, narrow panels included).
pub fn labeled_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let lw = label_col(ui);
    labeled_row_in(ui, label, lw, add)
}

/// [`labeled_row`] with the label column given: a row that keeps slots at its
/// end passes `label_col(ui) - ROW_END_SLOTS`.
pub fn labeled_row_in<R>(ui: &mut Ui, label: &str, label_w: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.set_min_height(theme::space::CONTROL_H);
        let (r, resp) = ui.allocate_exact_size(Vec2::new(label_w, theme::space::CONTROL_H), Sense::hover());
        let cut = paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(r, 0.0), r.center().y), crate::i18n::start_center(), label, theme::body(), t.text_2, label_w - 8.0);
        tip_if_cut(resp, cut, label);
        ui.spacing_mut().item_spacing.x = 6.0;
        add(ui)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::eval_expression as e;

    #[test]
    fn expressions() {
        assert_eq!(e("12", 5.0), Some(12.0));
        assert_eq!(e("+10", 5.0), Some(15.0));
        assert_eq!(e("*2", 5.0), Some(10.0));
        assert_eq!(e("/2", 5.0), Some(2.5));
        assert_eq!(e("-3", 5.0), Some(-3.0));
        assert_eq!(e("1920/2", 0.0), Some(960.0));
        assert_eq!(e("(1+2)*3", 0.0), Some(9.0));
        assert_eq!(e("50%", 0.0), Some(50.0));
        assert_eq!(e("12 px", 0.0), Some(12.0));
        assert_eq!(e("abc", 1.0), None);
        assert_eq!(e("", 1.0), None);
    }
}

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn a_labeled_row_takes_the_narrow_label_column_in_a_narrow_panel() {
        let ctx = egui::Context::default();
        let mut got = (0.0, 0.0);
        let mut out = ctx.run_ui(Default::default(), |ui| {
            ui.scope(|ui| {
                ui.set_max_width(200.0);
                got.0 = label_col(ui);
            });
            got.1 = label_col(ui);
        });
        out.textures_delta.clear();
        assert_eq!(got.0, 80.0);
        assert_eq!(got.1, LABEL_W);
    }
}
