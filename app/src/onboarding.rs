// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! First-run tour and contextual tips.
//!
//! The tour walks through the editor's regions once, the first time an
//! editor opens (Settings ▸ Accessibility ▸ "Replay the welcome tour"
//! brings it back). It works from the keyboard (`→` / `Enter` next, `←`
//! back, `Esc` skip) and every step is announced to a screen reader.
//!
//! Tips are one-line hints, shown one at a time when they apply and never
//! twice; they can be switched off in Settings.

use crate::app::{Overlay, StudioApp};
use crate::cli::Screen;
use crate::i18n::{tr, tr_args};
use crate::kit::{self, Button};
use crate::theme;
use eframe::egui::{self, Align2, Context, Id, Order, Rect, Vec2};

/// What a tour step points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The whole window: a welcome card in the middle.
    None,
    Region(crate::a11y::Region),
    /// Something registered with [`note_target`] (the workspace tabs, Export).
    Named(&'static str),
}

pub struct Step {
    pub id: &'static str,
    pub target: Target,
}

pub const STEPS: &[Step] = &[
    Step { id: "welcome", target: Target::None },
    Step { id: "library", target: Target::Region(crate::a11y::Region::Library) },
    Step { id: "viewer", target: Target::Region(crate::a11y::Region::Viewer) },
    Step { id: "inspector", target: Target::Region(crate::a11y::Region::Inspector) },
    Step { id: "timeline", target: Target::Region(crate::a11y::Region::Timeline) },
    Step { id: "workspaces", target: Target::Named("workspace-tabs") },
    Step { id: "ai", target: Target::None },
    Step { id: "export", target: Target::Named("export") },
    Step { id: "help", target: Target::None },
];

/// Message ids the tour uses.
pub fn message_ids() -> Vec<String> {
    STEPS.iter().flat_map(|s| [format!("tour-{}-title", s.id), format!("tour-{}-body", s.id)]).collect()
}

fn rects_id() -> Id {
    Id::new("onboarding-targets")
}

/// Remember where something is, so the tour can point at it.
pub fn note_target(ctx: &Context, name: &'static str, rect: Rect) {
    ctx.data_mut(|d| {
        let m = d.get_temp_mut_or_insert_with(rects_id(), Vec::<(&'static str, Rect)>::new);
        m.retain(|(n, _)| *n != name);
        m.push((name, rect));
    });
}

fn target_rect(ctx: &Context, t: Target) -> Option<Rect> {
    let name = match t {
        Target::None => return None,
        Target::Region(r) => r.name(),
        Target::Named(n) => n,
    };
    ctx.data(|d| d.get_temp::<Vec<(&'static str, Rect)>>(rects_id())).and_then(|v| v.into_iter().find(|(n, _)| *n == name).map(|(_, r)| r))
}

#[derive(Clone, Copy, Default)]
struct Tour {
    step: usize,
    /// The step last announced, so it is spoken once.
    said: Option<usize>,
}

fn tour_id() -> Id {
    Id::new("onboarding-tour")
}

/// Is the tour on screen?
pub fn tour_running(app: &StudioApp) -> bool {
    !app.prefs.tour_done && app.screen == Screen::Editor && app.overlay == Overlay::None && app.cli.shot.is_none()
}

/// Draw the tour. Call after everything else, once per frame.
pub fn show_tour(app: &mut StudioApp, ctx: &Context) {
    if !tour_running(app) {
        return;
    }
    let mut tour: Tour = ctx.data(|d| d.get_temp(tour_id())).unwrap_or_default();
    // Steps whose target is not on screen (a closed panel) are skipped.
    let usable = |i: usize| match STEPS[i].target {
        Target::None => true,
        t => target_rect(ctx, t).is_some(),
    };
    while tour.step < STEPS.len() - 1 && !usable(tour.step) {
        tour.step += 1;
    }
    let step = &STEPS[tour.step.min(STEPS.len() - 1)];
    let t = theme::tokens(ctx);
    let screen = ctx.content_rect();
    let target = target_rect(ctx, step.target);

    // Dim everything but the target.
    let dim = ctx.layer_painter(egui::LayerId::new(Order::Middle, Id::new("tour-dim")));
    let veil = egui::Color32::from_black_alpha(if crate::access::high_contrast() { 200 } else { 150 });
    match target {
        Some(r) => {
            let r = r.expand(4.0).intersect(screen);
            dim.rect_filled(Rect::from_min_max(screen.min, egui::pos2(screen.right(), r.top())), 0, veil);
            dim.rect_filled(Rect::from_min_max(egui::pos2(screen.left(), r.bottom()), screen.max), 0, veil);
            dim.rect_filled(Rect::from_min_max(egui::pos2(screen.left(), r.top()), egui::pos2(r.left(), r.bottom())), 0, veil);
            dim.rect_filled(Rect::from_min_max(egui::pos2(r.right(), r.top()), egui::pos2(screen.right(), r.bottom())), 0, veil);
            dim.rect_stroke(r, 8, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
        }
        None => {
            dim.rect_filled(screen, 0, veil);
        }
    }

    // The card: beside the target when there is room, else centred.
    let card_w = 340.0;
    let est_h = 230.0;
    let pos = match target {
        Some(r) if r.center().x < screen.center().x && r.right() + card_w + 24.0 < screen.right() => egui::pos2(r.right() + 16.0, (r.center().y - est_h / 2.0).clamp(screen.top() + 12.0, screen.bottom() - est_h - 12.0)),
        Some(r) if r.left() - card_w - 24.0 > screen.left() => egui::pos2(r.left() - card_w - 16.0, (r.center().y - est_h / 2.0).clamp(screen.top() + 12.0, screen.bottom() - est_h - 12.0)),
        Some(r) if r.bottom() + est_h + 24.0 < screen.bottom() => egui::pos2((r.center().x - card_w / 2.0).clamp(screen.left() + 12.0, screen.right() - card_w - 12.0), r.bottom() + 16.0),
        Some(r) => egui::pos2((r.center().x - card_w / 2.0).clamp(screen.left() + 12.0, screen.right() - card_w - 12.0), (r.top() - est_h - 16.0).max(screen.top() + 12.0)),
        None => egui::pos2(screen.center().x - card_w / 2.0, screen.center().y - est_h / 2.0),
    };
    let n = STEPS.len();
    let title = tr(&format!("tour-{}-title", step.id)).into_owned();
    let body = tr(&format!("tour-{}-body", step.id)).into_owned();
    if tour.said != Some(tour.step) {
        crate::a11y::announce(ctx, format!("{} {}", title, body));
        tour.said = Some(tour.step);
    }

    let mut go: i32 = 0;
    let mut skip = false;
    let mut ours: Vec<Id> = Vec::new();
    let mut next_id: Option<Id> = None;
    // Nothing behind the tour can be clicked while it is open.
    egui::Area::new(Id::new("tour-block")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
    });
    egui::Area::new(Id::new("tour-card")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(if crate::access::high_contrast() { 2.0 } else { 1.0 }, t.line)).corner_radius(10).inner_margin(16).show(ui, |ui| {
            ui.set_width(card_w - 32.0);
            ui.vertical(|ui| {
                let step_text = tr_args("tour-step", &[("n", (tour.step + 1).into()), ("total", n.into())]).into_owned();
                kit::label_a11y(ui, egui::Label::new(egui::RichText::new(crate::i18n::visual(&step_text)).font(theme::caption()).color(t.text_2)), &step_text);
                ui.add_space(4.0);
                kit::label_a11y(ui, egui::Label::new(egui::RichText::new(crate::i18n::visual(&title)).font(theme::section_title()).color(t.text)), &title);
                ui.add_space(6.0);
                let font = theme::body();
                let wrapped = kit::wrapped(ui.painter(), &body, font.clone(), card_w - 32.0);
                let rtl_text = studio_i18n::rtl::has_rtl(&body);
                let mut lab = egui::Label::new(egui::RichText::new(wrapped).font(font).color(t.text));
                lab = if rtl_text { lab.extend() } else { lab.wrap() };
                kit::label_a11y(ui, lab, &body);
                ui.add_space(12.0);
                ui.with_layout(crate::i18n::row_layout(), |ui| {
                    let last = tour.step + 1 >= n;
                    let next = Button::primary(if last { "Done" } else { "Next" }).show(ui);
                    ours.push(next.id);
                    next_id = Some(next.id);
                    if next.clicked() {
                        go = 1;
                    }
                    if tour.step > 0 {
                        let b = Button::ghost("Back").show(ui);
                        ours.push(b.id);
                        if b.clicked() {
                            go = -1;
                        }
                    }
                    if !last {
                        let b = Button::ghost("Skip tour").show(ui);
                        ours.push(b.id);
                        if b.clicked() {
                            skip = true;
                        }
                    }
                });
            });
        });
    });
    // Keyboard focus stays on the tour's own buttons (Tab cannot leave it).
    if let Some(id) = next_id {
        let focused = ctx.memory(|m| m.focused());
        if focused.is_none_or(|f| !ours.contains(&f)) {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    // Consumed here so the editor's own shortcuts do not also act on them.
    let (right, left, esc) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft), i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)));
    let (next_key, prev_key) = if crate::i18n::is_rtl() { (left, right) } else { (right, left) };
    if next_key {
        go = 1;
    }
    if prev_key {
        go = -1;
    }
    if esc {
        skip = true;
    }
    if skip {
        finish(app, ctx);
        return;
    }
    if go == 1 {
        if tour.step + 1 >= n {
            finish(app, ctx);
            return;
        }
        tour.step += 1;
    } else if go == -1 && tour.step > 0 {
        tour.step -= 1;
        // Back over skipped steps.
        while tour.step > 0 && !usable(tour.step) {
            tour.step -= 1;
        }
    }
    ctx.data_mut(|d| d.insert_temp(tour_id(), tour));
    if go != 0 {
        ctx.request_repaint();
    }
}

fn finish(app: &mut StudioApp, ctx: &Context) {
    app.prefs.tour_done = true;
    ctx.data_mut(|d| d.insert_temp(tour_id(), Tour::default()));
    crate::a11y::announce(ctx, tr("tour-finished").into_owned());
}

// --- tips ----------------------------------------------------------------------------

pub struct Tip {
    pub id: &'static str,
    pub when: fn(&StudioApp) -> bool,
}

pub const TIPS: &[Tip] = &[
    Tip { id: "start-templates", when: |a| a.screen == Screen::Start && a.prefs.recents.is_empty() },
    Tip { id: "empty-timeline", when: |a| a.screen == Screen::Editor && a.doc.project.clips.is_empty() },
    Tip { id: "select-clip", when: |a| a.screen == Screen::Editor && !a.doc.project.clips.is_empty() && a.editor.selection.is_empty() },
    Tip { id: "split", when: |a| a.screen == Screen::Editor && a.editor.selection.len() == 1 },
    Tip { id: "undo", when: |a| a.screen == Screen::Editor && a.doc.can_undo() },
    Tip { id: "ai", when: |a| a.screen == Screen::Editor && a.doc.project.clips.len() >= 3 },
    Tip { id: "workspaces", when: |a| a.screen == Screen::Editor && !a.doc.project.clips.is_empty() && a.doc.edits >= 4 },
    Tip { id: "keyboard", when: |a| a.screen == Screen::Editor && a.doc.edits >= 8 },
];

/// Message ids the tips use.
pub fn tip_message_ids() -> Vec<String> {
    TIPS.iter().map(|t| format!("tip-{}", t.id)).collect()
}

#[derive(Clone, Copy, Default)]
struct TipState {
    /// When the current candidate first applied.
    since: f64,
    /// When the last tip was dismissed.
    last_shown: Option<f64>,
    current: Option<usize>,
}

fn tip_state_id() -> Id {
    Id::new("onboarding-tip")
}

/// The first tip that applies and has not been seen.
pub fn next_tip(app: &StudioApp) -> Option<usize> {
    TIPS.iter().position(|t| !app.prefs.tips_seen.iter().any(|s| s == t.id) && (t.when)(app))
}

/// Draw the current tip (a small card at the bottom left). Call once per frame.
pub fn show_tip(app: &mut StudioApp, ctx: &Context) {
    if !app.prefs.tips_on || tour_running(app) || app.overlay != Overlay::None || app.cli.shot.is_some() {
        return;
    }
    let now = ctx.input(|i| i.time);
    let mut st: TipState = ctx.data(|d| d.get_temp(tip_state_id())).unwrap_or_default();
    let cand = next_tip(app);
    if st.current.is_none() {
        match cand {
            Some(c) => {
                if st.since == 0.0 || st.current != Some(c) && st.since < 0.0 {
                    st.since = now;
                }
                // A condition has to hold for a moment, and tips are spaced out.
                if now - st.since >= 2.5 && st.last_shown.is_none_or(|t| now - t >= 12.0) {
                    st.current = Some(c);
                    st.since = now;
                    crate::a11y::announce(ctx, tr(&format!("tip-{}", TIPS[c].id)).into_owned());
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(600));
            }
            None => st.since = 0.0,
        }
    }
    let mut dismiss: Option<bool> = None;
    if let Some(c) = st.current {
        // The condition stopped applying (the person did the thing): drop it quietly.
        if !(TIPS[c].when)(app) {
            dismiss = Some(false);
        } else {
            let t = theme::tokens(ctx);
            let screen = ctx.content_rect();
            let w = 380.0f32.min(screen.width() - 32.0);
            let pos = egui::pos2(if crate::i18n::is_rtl() { screen.right() - w - 16.0 } else { screen.left() + 16.0 }, screen.bottom() - 88.0);
            egui::Area::new(Id::new("tip-card")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
                egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(if crate::access::high_contrast() { 2.0 } else { 1.0 }, t.line)).corner_radius(8).inner_margin(12).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    let msg = tr(&format!("tip-{}", TIPS[c].id)).into_owned();
                    let font = theme::body();
                    let wrapped = kit::wrapped(ui.painter(), &msg, font.clone(), w - 24.0);
                    let lab = egui::Label::new(egui::RichText::new(wrapped).font(font).color(t.text));
                    kit::label_a11y(ui, if studio_i18n::rtl::has_rtl(&msg) { lab.extend() } else { lab.wrap() }, &msg);
                    ui.add_space(8.0);
                    ui.with_layout(crate::i18n::row_layout(), |ui| {
                        if Button::secondary("Got it").compact().show(ui).clicked() {
                            dismiss = Some(true);
                        }
                        if Button::ghost("Turn off tips").compact().show(ui).clicked() {
                            app.prefs.tips_on = false;
                            dismiss = Some(true);
                        }
                    });
                });
            });
        }
    }
    if dismiss.is_some() {
        if let Some(c) = st.current.take() {
            app.prefs.tips_seen.push(TIPS[c].id.to_owned());
        }
        st.last_shown = Some(now);
        st.since = 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(tip_state_id(), st));
    let _ = Align2::CENTER_CENTER;
    let _ = Vec2::ZERO;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_has_text() {
        for id in message_ids().into_iter().chain(tip_message_ids()) {
            assert!(studio_i18n::catalogue(studio_i18n::Lang::En).contains(&id), "{id} missing from en.ftl");
        }
    }

    #[test]
    fn steps_cover_the_regions_in_order() {
        let regions: Vec<_> = STEPS.iter().filter_map(|s| if let Target::Region(r) = s.target { Some(r) } else { None }).collect();
        use crate::a11y::Region::*;
        assert_eq!(regions, [Library, Viewer, Inspector, Timeline]);
    }

    #[test]
    fn tip_ids_are_unique() {
        let mut ids: Vec<_> = TIPS.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TIPS.len());
    }
}
