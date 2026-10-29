// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Toasts (spec §3): bottom-centre over the viewer, one line, gone after
//! 4 s, with at most one action ("Undo").

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Id, Sense, Vec2};

pub const LIFETIME: f64 = 4.0;
/// Most warnings kept on screen at once; older ones move to the recent list.
const MAX_PERSISTENT: usize = 3;
/// Size of the recent-notifications list.
const HISTORY: usize = 20;
/// Vertical step between stacked toasts.
const STEP: f32 = 40.0;

/// How long a toast stays. Warnings and errors stay until read (dismissed
/// or their action taken) so the next toast cannot wipe them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub action: Option<String>,
    pub icon: Option<Icon>,
    pub born: f64,
    pub id: u64,
    pub level: Level,
    /// How many identical pushes this toast stands for.
    pub count: u32,
}

#[derive(Clone, Default)]
struct Queue {
    items: Vec<Toast>,
    history: Vec<Toast>,
    next: u64,
}

impl Queue {
    fn push(&mut self, now: f64, text: String, action: Option<&str>, icon: Option<Icon>, level: Level) -> u64 {
        // A repeat of a toast already showing collapses into it.
        if let Some(t) = self.items.iter_mut().find(|t| t.text == text && t.level == level && t.action.as_deref() == action) {
            t.count += 1;
            t.born = now;
            return t.id;
        }
        self.next += 1;
        let id = self.next;
        if level == Level::Info {
            // Info toasts replace each other, as before.
            self.items.retain(|t| t.level != Level::Info);
        }
        self.items.push(Toast { text: text.clone(), action: action.map(str::to_owned), icon, born: now, id, level, count: 1 });
        self.remember(id);
        while self.items.iter().filter(|t| t.level == Level::Warning).count() > MAX_PERSISTENT {
            let i = self.items.iter().position(|t| t.level == Level::Warning).unwrap();
            self.items.remove(i);
        }
        id
    }

    fn remember(&mut self, id: u64) {
        if let Some(t) = self.items.iter().find(|t| t.id == id) {
            self.history.push(t.clone());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
    }

    fn expire(&mut self, now: f64) {
        self.items.retain(|t| t.level == Level::Warning || now - t.born < LIFETIME + 0.3);
    }

    fn dismiss(&mut self, id: u64) {
        self.items.retain(|t| t.id != id);
    }
}

fn qid() -> Id {
    Id::unique("studio-toasts")
}

/// Show a toast. Returns its id, so a caller can match the action click.
/// A warning icon marks a warning or error: it stays until dismissed.
pub fn push(ctx: &egui::Context, text: impl Into<String>, action: Option<&str>, icon: Option<Icon>) -> u64 {
    let level = if icon == Some(Icon::Warning) { Level::Warning } else { Level::Info };
    push_as(ctx, text, action, icon, level)
}

/// Like [`push`] with the level chosen by the caller.
pub fn push_as(ctx: &egui::Context, text: impl Into<String>, action: Option<&str>, icon: Option<Icon>, level: Level) -> u64 {
    let now = ctx.input(|i| i.time);
    let text: String = text.into();
    crate::a11y::announce(ctx, crate::i18n::t(&text).into_owned());
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Queue>(qid()).push(now, text, action, icon, level))
}

/// The last notifications, newest first (including ones already gone).
pub fn recent(ctx: &egui::Context) -> Vec<Toast> {
    ctx.data(|d| d.get_temp::<Queue>(qid()).map(|q| q.history.iter().rev().cloned().collect()).unwrap_or_default())
}

/// Forget the recent list.
pub fn clear_recent(ctx: &egui::Context) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Queue>(qid()).history.clear());
}

/// Draw the toasts stacked above the bottom centre of `area`, newest at the
/// bottom. Returns the id of a toast whose action was clicked.
pub fn show(ctx: &egui::Context, area: Rect, bottom_offset: f32) -> Option<u64> {
    let now = ctx.input(|i| i.time);
    let mut q: Queue = ctx.data(|d| d.get_temp(qid()).unwrap_or_default());
    q.expire(now);
    let items = q.items.clone();
    ctx.data_mut(|d| d.insert_temp(qid(), q));
    if items.is_empty() {
        return None;
    }
    ctx.request_repaint();

    let mut clicked = None;
    let mut dismissed = None;
    let n = items.len();
    for (i, toast) in items.iter().enumerate() {
        let age = (now - toast.born) as f32;
        let fade_in = if reduce_motion(ctx) { 1.0 } else { ease_out((age / PANEL).min(1.0)) };
        let fade_out = if toast.level == Level::Warning { 1.0 } else { ((LIFETIME as f32 + 0.3 - age) / 0.3).clamp(0.0, 1.0) };
        let k = fade_in.min(fade_out);
        let lift = (n - 1 - i) as f32 * STEP;
        egui::Area::new(Id::unique(("toast", toast.id)))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(egui::pos2(area.center().x, area.bottom() - bottom_offset - lift + (1.0 - k) * 8.0))
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_opacity(k);
                match body(ui, toast, area.width()) {
                    Hit::Action => clicked = Some(toast.id),
                    Hit::Dismiss => dismissed = Some(toast.id),
                    Hit::None => {}
                }
            });
    }
    if let Some(id) = clicked.or(dismissed) {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Queue>(qid()).dismiss(id));
    }
    clicked
}

enum Hit {
    None,
    Action,
    Dismiss,
}

/// The toast surface itself; shared by the live toast and the gallery.
fn body(ui: &mut Ui, toast: &Toast, area_w: f32) -> Hit {
    let t = theme::tokens(ui.ctx());
    let mut hit = Hit::None;
    let label = if toast.count > 1 { format!("{} (×{})", toast.text, toast.count) } else { toast.text.clone() };
    let inv = if t.dark { theme::mix(t.panel, Color32::WHITE, 0.06) } else { theme::DARK.raised };
    let fg = if t.dark { t.text } else { theme::DARK.text };
    egui::Frame::new()
        .fill(inv)
        .corner_radius(CornerRadius::same(theme::radius::CARD))
        .stroke(egui::Stroke::new(1.0, if t.dark { t.line } else { inv }))
        .shadow(theme::popover_shadow())
        .inner_margin(egui::Margin { left: 14, right: 8, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.set_min_height(28.0);
                if let Some(icon) = toast.icon {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    icons::paint(ui.painter(), r, icon, 16.0, if t.dark { t.accent } else { theme::DARK.accent });
                }
                let max_w = (area_w - 160.0).max(120.0);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(text_width(ui, &label, theme::control()).min(max_w), 20.0), Sense::hover());
                if paint_text_clipped(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &label, theme::control(), fg, max_w) {
                    resp.on_hover_text(shown(&label));
                }
                if let Some(a) = &toast.action {
                    let w = text_width(ui, a, theme::control_medium()) + 20.0;
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
                    let st = State::from_response(ui, &resp, None);
                    let accent = if t.dark { t.accent } else { theme::DARK.accent };
                    ui.painter().rect_filled(r, CornerRadius::same(6), theme::alpha(Color32::WHITE, 0.08 * st.hover));
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, shown(a), theme::control_medium(), theme::mix(accent, Color32::WHITE, 0.25));
                    if st.focused {
                        focus_ring(ui.painter(), r, CornerRadius::same(6), &t);
                    }
                    if resp.clicked() {
                        hit = Hit::Action;
                    }
                } else if toast.level == Level::Info {
                    ui.add_space(theme::space::SM);
                }
                if toast.level == Level::Warning {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                    let st = State::from_response(ui, &resp, None);
                    ui.painter().rect_filled(r, CornerRadius::same(6), theme::alpha(Color32::WHITE, 0.08 * st.hover));
                    icons::paint(ui.painter(), r.shrink(7.0), Icon::X, 14.0, fg);
                    if st.focused {
                        focus_ring(ui.painter(), r, CornerRadius::same(6), &t);
                    }
                    if resp.on_hover_text("Dismiss").clicked() {
                        hit = Hit::Dismiss;
                    }
                }
            });
        });
    hit
}

/// Draw a toast inline (component gallery).
pub fn preview(ui: &mut Ui, text: &str, action: Option<&str>, icon: Option<Icon>) -> bool {
    let level = if icon == Some(Icon::Warning) { Level::Warning } else { Level::Info };
    let toast = Toast { text: text.into(), action: action.map(str::to_owned), icon, born: 0.0, id: 0, level, count: 1 };
    matches!(body(ui, &toast, 600.0), Hit::Action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> Queue {
        Queue::default()
    }

    #[test]
    fn an_error_survives_the_next_toast() {
        let mut q = q();
        q.push(0.0, "Could not save".into(), None, Some(Icon::Warning), Level::Warning);
        q.push(1.0, "Saved".into(), None, None, Level::Info);
        q.expire(20.0);
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].text, "Could not save");
    }

    #[test]
    fn info_replaces_info_and_expires() {
        let mut q = q();
        q.push(0.0, "One".into(), None, None, Level::Info);
        q.push(1.0, "Two".into(), None, None, Level::Info);
        assert_eq!(q.items.len(), 1);
        q.expire(1.0 + LIFETIME + 1.0);
        assert!(q.items.is_empty());
    }

    #[test]
    fn duplicates_collapse() {
        let mut q = q();
        let a = q.push(0.0, "Could not save".into(), None, None, Level::Warning);
        let b = q.push(5.0, "Could not save".into(), None, None, Level::Warning);
        assert_eq!(a, b);
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].count, 2);
        assert_eq!(q.history.len(), 1);
    }

    #[test]
    fn warnings_are_capped_and_kept_in_history() {
        let mut q = q();
        for i in 0..30 {
            q.push(i as f64, format!("Problem {i}"), None, None, Level::Warning);
        }
        assert_eq!(q.items.len(), MAX_PERSISTENT);
        assert_eq!(q.items.last().unwrap().text, "Problem 29");
        assert_eq!(q.history.len(), HISTORY);
    }

    #[test]
    fn dismiss_removes_one() {
        let mut q = q();
        let a = q.push(0.0, "A".into(), None, None, Level::Warning);
        q.push(0.0, "B".into(), None, None, Level::Warning);
        q.dismiss(a);
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].text, "B");
    }

    #[test]
    fn warning_icon_means_persistent() {
        let ctx = egui::Context::default();
        push(&ctx, "Could not open the file", None, Some(Icon::Warning));
        push(&ctx, "Saved", None, Some(Icon::CheckCircle));
        let r = recent(&ctx);
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].level, Level::Warning);
        assert_eq!(r[0].level, Level::Info);
    }
}
