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

/// Record that a popover, dialog or sheet is up on this pass.
fn mark_shown(ctx: &egui::Context) {
    // The pass number first: asking the context for it while its data
    // is locked would wait on itself forever.
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(Id::new("kit-popover-shown"), pass));
}

/// Take Escape if it was pressed, so the layer below never sees it too: the
/// top-most surface closes first, one press at a time.
fn take_escape(ctx: &egui::Context) -> bool {
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
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
        mark_shown(ctx);
        // Arrow keys step into a menu that holds no focus yet.
        if ctx.memory(|m| m.focused()).is_none() {
            let down = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
            if down {
                ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::Next));
            }
        }
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
        if clicked_outside || take_escape(ctx) {
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

/// The three dialog sizes. A dialog asks for one of these instead of its own
/// numbers, so every dialog of a kind has the same width.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogSize {
    /// A question and two buttons.
    Small,
    /// A short form.
    Medium,
    /// A workspace of its own (settings, help).
    Large,
}

impl DialogSize {
    pub fn size(self) -> Vec2 {
        match self {
            Self::Small => Vec2::new(440.0, 200.0),
            Self::Medium => Vec2::new(560.0, 380.0),
            Self::Large => Vec2::new(900.0, 620.0),
        }
    }
}

/// Height of the title row.
pub const DIALOG_HEAD_H: f32 = 56.0;
/// Height of the button row at the foot of a dialog.
pub const DIALOG_FOOT_H: f32 = 56.0;
/// Space between a dialog's edge and its content.
pub const DIALOG_PAD: f32 = theme::space::XL;

/// What a modal reported this frame.
pub struct ModalOutput {
    /// Esc, the close button or a click on the backdrop.
    pub close: bool,
    /// Enter, with no field or button holding the keyboard: the primary
    /// button's answer.
    pub confirm: bool,
}

/// What the keyboard asks of a dialog: Esc closes, Enter confirms unless a
/// field is being edited or a button holds focus (it takes Enter itself).
pub fn dialog_keys(esc: bool, enter: bool, typing: bool, focus_held: bool) -> (bool, bool) {
    (esc && !typing, enter && !typing && !focus_held)
}

/// How dark the backdrop under a dialog is, from 0 (closed) to 1 (open).
pub fn scrim_alpha(dark: bool, k: f32) -> u8 {
    ((if dark { 120.0 } else { 70.0 }) * k.clamp(0.0, 1.0)) as u8
}

fn prev_focus_id(id: Id) -> Id {
    id.with("dialog-prev-focus")
}

/// On the first pass of a dialog: remember what held focus and let go of it,
/// so the keys go to the dialog.
fn take_focus_in(ctx: &egui::Context, id: Id) {
    let key = id.with("dialog-seen");
    if ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(key, true));
    let prev = ctx.memory(|m| m.focused());
    ctx.data_mut(|d| d.insert_temp(prev_focus_id(id), prev));
    if let Some(f) = prev {
        ctx.memory_mut(|m| m.surrender_focus(f));
    }
}

/// Give focus back to where it came from. [`modal`] and [`sheet`] do it when
/// they report a close; call it when a dialog ends some other way (its own
/// Save button).
pub fn return_focus(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| d.insert_temp(id.with("dialog-seen"), false));
    if let Some(Some(f)) = ctx.data(|d| d.get_temp::<Option<Id>>(prev_focus_id(id))) {
        ctx.memory_mut(|m| m.request_focus(f));
    }
}

fn read_keys(ctx: &egui::Context) -> (bool, bool) {
    let typing = ctx.egui_wants_keyboard_input();
    let held = ctx.memory(|m| m.focused()).is_some();
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let (close, confirm) = dialog_keys(esc, enter, typing, held);
    if close {
        take_escape(ctx);
    }
    (close, confirm)
}

/// Dialog over a dimmed backdrop. It fades and scales in over 180 ms (not at
/// all with reduce-motion) and carries the popover shadow and hairline border
/// (spec §1). Esc closes it, Enter confirms, focus goes back where it came
/// from.
pub fn modal(ctx: &egui::Context, id: Id, size: Vec2, title: &str, add: impl FnOnce(&mut Ui)) -> ModalOutput {
    let t = theme::tokens(ctx);
    mark_shown(ctx);
    take_focus_in(ctx, id);
    let k = panel_t(ctx, id, true);
    let screen = ctx.content_rect();
    let mut close = false;
    // Dim everything below; a click on the backdrop closes the dialog.
    let dim = ctx.layer_painter(egui::LayerId::new(Order::Middle, id.with("backdrop")));
    dim.rect_filled(screen, 0, Color32::from_black_alpha(scrim_alpha(t.dark, k)));
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
        let head = Rect::from_min_size(r.min, Vec2::new(r.width(), DIALOG_HEAD_H));
        paint_text_clipped(p, egui::pos2(crate::i18n::start_x(head, DIALOG_PAD), head.center().y), crate::i18n::start_center(), title, theme::dialog_title(), t.text, head.width() - 76.0);
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
    let (esc, confirm) = read_keys(ctx);
    close |= esc;
    if close {
        return_focus(ctx, id);
    }
    ModalOutput { close, confirm }
}

/// [`modal`] at one of the standard sizes.
pub fn modal_sized(ctx: &egui::Context, id: Id, size: DialogSize, title: &str, add: impl FnOnce(&mut Ui)) -> ModalOutput {
    modal(ctx, id, size.size(), title, add)
}

/// Which footer button was pressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    None,
    Primary,
    Secondary,
}

/// The rectangle left for content above the footer.
pub fn above_footer(ui: &Ui) -> Rect {
    let r = ui.max_rect();
    Rect::from_min_max(r.min, egui::pos2(r.max.x, r.max.y - DIALOG_FOOT_H))
}

/// The button row at the foot of a dialog: a hairline, then the primary
/// action at the right edge with the secondary one to its left. Name the
/// primary after what it does ("Delete 3 clips"), never "OK". `danger` draws
/// it as a destructive action.
pub fn dialog_footer(ui: &mut Ui, primary: &str, secondary: Option<&str>, danger: bool) -> Choice {
    let r = ui.max_rect();
    let foot = Rect::from_min_max(egui::pos2(r.left(), r.bottom() - DIALOG_FOOT_H), r.max);
    hline(ui, foot.left()..=foot.right(), foot.top());
    let mut choice = Choice::None;
    let inner = foot.shrink2(Vec2::new(DIALOG_PAD, 0.0));
    let layout = if crate::i18n::is_rtl() { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::right_to_left(egui::Align::Center) };
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(layout).id_salt("dialog-footer"));
    row.spacing_mut().item_spacing.x = theme::space::SM;
    let kind = if danger { Kind::Danger } else { Kind::Primary };
    if Button::new(primary, kind).min_width(88.0).show(&mut row).clicked() {
        choice = Choice::Primary;
    }
    if let Some(s) = secondary {
        if Button::secondary(s).min_width(88.0).show(&mut row).clicked() {
            choice = Choice::Secondary;
        }
    }
    choice
}

/// A question dialog that names its action. `Some(true)` when confirmed,
/// `Some(false)` when cancelled, `None` while it is open. Enter confirms
/// unless the action is `danger`: a destructive answer takes a click.
pub fn confirm_dialog(ctx: &egui::Context, id: Id, title: &str, message: &str, action: &str, danger: bool) -> Option<bool> {
    let mut answer = None;
    let out = modal_sized(ctx, id, DialogSize::Small, title, |ui| {
        let body = above_footer(ui).shrink2(Vec2::new(DIALOG_PAD, theme::space::LG));
        let mut text = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("confirm-text"));
        text.add(egui::Label::new(egui::RichText::new(tl(message)).font(theme::body()).color(theme::tokens(ctx).text_2)).wrap());
        match dialog_footer(ui, action, Some("Cancel"), danger) {
            Choice::Primary => answer = Some(true),
            Choice::Secondary => answer = Some(false),
            Choice::None => {}
        }
    });
    if out.close {
        answer = Some(false);
    } else if out.confirm && !danger {
        answer = Some(true);
    }
    if answer.is_some() {
        return_focus(ctx, id);
    }
    answer
}

/// A tall sheet over the middle of the window (the export screen): no
/// title row and no dimming; a click outside it, Esc or its close button
/// closes it. It fades in like a dialog.
pub fn sheet(ctx: &egui::Context, id: Id, rect: Rect, add: impl FnOnce(&mut Ui)) -> ModalOutput {
    let t = theme::tokens(ctx);
    mark_shown(ctx);
    take_focus_in(ctx, id);
    let k = panel_t(ctx, id, true);
    let screen = ctx.content_rect();
    let mut close = false;
    egui::Area::new(id.with("backdrop-hit")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (_, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        if resp.clicked() {
            close = true;
        }
    });
    let rect = rect.translate(Vec2::new(0.0, (1.0 - k) * 8.0));
    egui::Area::new(id.with("sheet")).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        ui.set_opacity(k);
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
    let (esc, confirm) = read_keys(ctx);
    close |= esc;
    if close {
        return_focus(ctx, id);
    }
    ModalOutput { close, confirm }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &egui::Context, size: Vec2, events: Vec<egui::Event>, frame: u32, add: impl FnMut(&egui::Context)) -> egui::FullOutput {
        run_with(ctx, size, events, frame, false, add)
    }

    fn run_with(ctx: &egui::Context, size: Vec2, events: Vec<egui::Event>, frame: u32, keep_textures: bool, add: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let mut add = add;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
            time: Some(f64::from(frame) / 60.0),
            events,
            ..Default::default()
        };
        let mut o = ctx.run_ui(raw, |ui| add(ui.ctx()));
        // Nothing here draws the atlas; the picture test applies its own.
        if !keep_textures {
            o.textures_delta.clear();
        }
        o
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        set_reduce_motion(&ctx, true);
        ctx
    }

    #[test]
    fn keys_close_on_escape_and_confirm_on_enter() {
        assert_eq!(dialog_keys(true, false, false, false), (true, false));
        assert_eq!(dialog_keys(false, true, false, false), (false, true));
        // Typing in a field: the field owns both keys.
        assert_eq!(dialog_keys(true, true, true, false), (false, false));
        // A focused button takes Enter itself.
        assert_eq!(dialog_keys(false, true, false, true), (false, false));
    }

    #[test]
    fn scrim_follows_the_open_amount() {
        assert_eq!(scrim_alpha(true, 0.0), 0);
        assert_eq!(scrim_alpha(true, 1.0), 120);
        assert!(scrim_alpha(false, 1.0) < scrim_alpha(true, 1.0));
        assert_eq!(scrim_alpha(true, 5.0), 120, "clamped");
    }

    #[test]
    fn sizes_step_up_and_fit_a_laptop_window() {
        let (s, m, l) = (DialogSize::Small.size(), DialogSize::Medium.size(), DialogSize::Large.size());
        assert!(s.x < m.x && m.x < l.x && s.y < m.y && m.y < l.y);
        assert!(l.x <= 1280.0 - 48.0 && l.y <= 800.0 - 48.0);
    }

    #[test]
    fn escape_closes_a_modal_and_enter_confirms() {
        let _serial = crate::ui_tests::serial();
        let ctx = ctx();
        let id = Id::new("t-modal");
        let size = Vec2::new(800.0, 600.0);
        let mut last = None;
        for f in 0..3 {
            run(&ctx, size, vec![], f, |c| last = Some(modal_sized(c, id, DialogSize::Small, "Title", |_| {})));
        }
        let o = last.take().unwrap();
        assert!(!o.close && !o.confirm);
        run(&ctx, size, vec![key(egui::Key::Escape)], 4, |c| last = Some(modal_sized(c, id, DialogSize::Small, "Title", |_| {})));
        assert!(last.take().unwrap().close, "Esc closes");
        run(&ctx, size, vec![key(egui::Key::Enter)], 5, |c| last = Some(modal_sized(c, id, DialogSize::Small, "Title", |_| {})));
        assert!(last.take().unwrap().confirm, "Enter confirms");
    }

    #[test]
    fn a_modal_gives_focus_back_when_it_closes() {
        let _serial = crate::ui_tests::serial();
        let ctx = ctx();
        let id = Id::new("t-focus");
        let origin = Id::new("origin-widget");
        let size = Vec2::new(800.0, 600.0);
        let widget = |c: &egui::Context, grab: bool| {
            egui::Area::new(Id::new("origin-area")).show(c, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                ui.interact(rect, origin, Sense::click());
                if grab {
                    ui.memory_mut(|m| m.request_focus(origin));
                }
            });
        };
        run(&ctx, size, vec![], 0, |c| widget(c, true));
        run(&ctx, size, vec![], 1, |c| widget(c, false));
        assert_eq!(ctx.memory(|m| m.focused()), Some(origin));
        run(&ctx, size, vec![], 2, |c| {
            widget(c, false);
            modal_sized(c, id, DialogSize::Small, "T", |_| {});
        });
        assert_eq!(ctx.memory(|m| m.focused()), None, "the dialog holds the keys");
        run(&ctx, size, vec![key(egui::Key::Escape)], 3, |c| {
            widget(c, false);
            modal_sized(c, id, DialogSize::Small, "T", |_| {});
        });
        run(&ctx, size, vec![], 4, |c| widget(c, false));
        assert_eq!(ctx.memory(|m| m.focused()), Some(origin), "back where it came from");
    }

    #[test]
    fn a_popover_stays_on_screen_and_escape_closes_it() {
        let _serial = crate::ui_tests::serial();
        let ctx = ctx();
        let id = Id::new("t-pop");
        let size = Vec2::new(600.0, 400.0);
        set_open(&ctx, id, true);
        // Anchored at the bottom-right corner, wider than the room.
        let anchor = Rect::from_min_size(egui::pos2(560.0, 370.0), Vec2::new(30.0, 24.0));
        for f in 0..4 {
            run(&ctx, size, vec![], f, |c| {
                popover(c, id, anchor, Side::Below, 280.0, |ui| {
                    for i in 0..30 {
                        MenuItem::new(&format!("Item {i}")).shortcut("Cmd+K").show(ui);
                    }
                });
            });
        }
        let r = ctx.memory(|m| m.area_rect(id.with("area"))).expect("the popover is placed");
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), size);
        assert!(screen.contains_rect(r), "{r:?} leaves {screen:?}");
        assert!(any_popover_open(&ctx));
        run(&ctx, size, vec![key(egui::Key::Escape)], 5, |c| {
            popover(c, id, anchor, Side::Below, 280.0, |ui| {
                ui.label("x");
            });
        });
        assert!(!is_open(&ctx, id), "Esc closes");
    }

    #[test]
    fn the_anatomy_draws_with_every_part() {
        let _serial = crate::ui_tests::serial();
        let ctx = ctx();
        let size = [900usize, 560usize];
        let mut textures = crate::ui_shot::Textures::default();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/ui-shots");
        std::fs::create_dir_all(&dir).unwrap();
        let mut out = None;
        for f in 0..4 {
            let mut o = run_with(&ctx, Vec2::new(size[0] as f32, size[1] as f32), vec![], f, true, |c| {
                confirm_dialog(c, Id::new("t-anatomy"), "Delete 3 clips?", "The clips leave the timeline and their media stays in the library.", "Delete 3 clips", true);
            });
            textures.apply(&o.textures_delta);
            o.textures_delta.clear();
            out = Some(o);
        }
        let out = out.unwrap();
        let rects = ctx.memory(|m| m.area_rect(Id::new("t-anatomy").with("dialog"))).unwrap();
        assert!((rects.width() - DialogSize::Small.size().x).abs() < 1.0);
        let prims = ctx.tessellate(out.shapes, 1.0);
        let rgba = crate::ui_shot::render(&prims, &textures, size, 1.0, theme::tokens(&ctx).ground);
        crate::ui_shot::save_png(&dir.join("popover-confirm-dialog.png"), size, &rgba).unwrap();
        assert_eq!(rgba.len(), size[0] * size[1] * 4);
    }

    #[test]
    fn a_confirm_dialog_names_its_action_and_enter_spares_a_destructive_one() {
        let _serial = crate::ui_tests::serial();
        let ctx = ctx();
        let size = Vec2::new(800.0, 600.0);
        let id = Id::new("t-confirm");
        let mut a = None;
        for f in 0..3 {
            run(&ctx, size, vec![], f, |c| a = confirm_dialog(c, id, "T", "m", "Save", false));
        }
        assert_eq!(a, None);
        run(&ctx, size, vec![key(egui::Key::Enter)], 4, |c| a = confirm_dialog(c, id, "T", "m", "Save", false));
        assert_eq!(a, Some(true), "Enter takes the primary answer");
        let id2 = Id::new("t-confirm-danger");
        for f in 5..8 {
            run(&ctx, size, vec![], f, |c| a = confirm_dialog(c, id2, "T", "m", "Delete 3 clips", true));
        }
        run(&ctx, size, vec![key(egui::Key::Enter)], 9, |c| a = confirm_dialog(c, id2, "T", "m", "Delete 3 clips", true));
        assert_eq!(a, None, "Enter does not delete");
        run(&ctx, size, vec![key(egui::Key::Escape)], 10, |c| a = confirm_dialog(c, id2, "T", "m", "Delete 3 clips", true));
        assert_eq!(a, Some(false), "Esc cancels");
    }
}
