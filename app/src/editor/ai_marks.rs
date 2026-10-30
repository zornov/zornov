// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pointing the AI editor at part of the picture, from the viewer.
//!
//! Two tools in the viewer's toolbar:
//! * **Area for the AI**: drag a box; it goes onto the next message as an
//!   attachment ("this" is now that box at that moment).
//! * **Note for the AI**: drag a box or click a spot, type a note there,
//!   Enter: the note goes into the chat as a quote, ready to send.
//!
//! Marks are kept in canvas pixels from the centre (y down), the units the
//! agent's tools use, with the timeline time they were made at.

use super::agent_panel::{AgentUi, Mark, MarkKind};
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit;
use crate::theme;
use crate::view::{self, SEC};
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

/// While a mark tool is on, a chip in the viewer's header says so and turns
/// it off.
pub fn mode_chip(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(mode) = app.agent.as_ref().and_then(|a| a.mark_mode) else { return };
    let label = if mode == MarkKind::Area { "Marking an area" } else { "Adding a note" };
    if kit::Button::ghost(label).icon(Icon::Selection).compact().show(ui).on_hover_text("Click to stop marking").clicked()
        && let Some(a) = app.agent.as_mut()
    {
        a.mark_mode = None;
    }
}

/// The AI marks part of the viewer's Add menu: turn on the area or the note
/// tool, then drag in the picture. `q` is the menu's search; `close` closes
/// the menu after a pick.
pub fn add_items(app: &mut StudioApp, ui: &mut egui::Ui, q: &str, close: egui::Id) {
    use super::viewer::{add_item, add_shows};
    let mode = app.agent.as_ref().and_then(|a| a.mark_mode);
    if !add_shows(q, "Mark for the AI", ["Select an area", "Add a note"]) {
        return;
    }
    kit::menu_heading(ui, "Mark for the AI");
    let mut want = None;
    if add_item(ui, q, "Mark for the AI", "Select an area") {
        want = Some(MarkKind::Area);
    }
    if add_item(ui, q, "Mark for the AI", "Add a note") {
        want = Some(MarkKind::Note);
    }
    if let Some(want) = want {
        let a = app.agent.get_or_insert_with(|| AgentUi::new(ui.ctx()));
        a.mark_mode = if mode == Some(want) { None } else { Some(want) };
        a.drawing = None;
        a.pending_note = None;
        kit::set_open(ui.ctx(), close, false);
    }
}

/// On the canvas: draws the marks attached to the next message and, while
/// a tool is on, takes the pointer to make a new one.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, scale: f32) {
    let t = theme::tokens(ui.ctx());
    let fps = app.doc.project.fps;
    let playhead = app.editor.playhead;
    let Some(a) = app.agent.as_mut() else { return };
    let to_canvas = |p: Pos2| -> [f32; 2] { [(p.x - canvas.center().x) / scale, (p.y - canvas.center().y) / scale] };
    let to_screen = |c: [f32; 2]| -> Pos2 { canvas.center() + Vec2::new(c[0], c[1]) * scale };
    let p = ui.painter().with_clip_rect(canvas);

    // Marks waiting to be sent: numbered boxes.
    for (i, m) in a.attachments.iter().enumerate() {
        if (m.time - playhead).abs() > SEC / 2 {
            continue;
        }
        paint_mark(&p, Rect::from_two_pos(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]])), m.kind, &format!("{}", i + 1), &t);
    }

    let Some(kind) = a.mark_mode else { return };
    // Esc puts the tool away.
    if a.pending_note.is_none() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        a.mark_mode = None;
        a.drawing = None;
        return;
    }
    if a.pending_note.is_none() {
        let resp = ui.interact(canvas, egui::Id::unique("ai-mark-canvas"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Crosshair);
        if resp.drag_started() || resp.clicked() {
            a.drawing = resp.interact_pointer_pos().map(to_canvas);
        }
        let now = ui.input(|i| i.pointer.interact_pos()).map(to_canvas);
        if let (Some(start), Some(cur)) = (a.drawing, now) {
            if resp.dragged() {
                paint_mark(&p, Rect::from_two_pos(to_screen(start), to_screen(cur)), kind, "", &t);
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            if let (Some(start), Some(end)) = (a.drawing.take(), now.or(a.drawing)) {
                let rect = [start[0].min(end[0]), start[1].min(end[1]), start[0].max(end[0]), start[1].max(end[1])];
                let mark = Mark { kind, time: playhead, rect, note: String::new() };
                match kind {
                    MarkKind::Area => {
                        a.attachments.push(mark);
                        a.mark_mode = None;
                        super::agent_panel::open(app);
                    }
                    MarkKind::Note => a.pending_note = Some(mark),
                }
            }
        }
        // A hint at the top of the picture.
        let hint = match kind {
            MarkKind::Area => "Drag over the part for the AI · Esc to cancel",
            MarkKind::Note => "Drag a box or click; type a note · Esc cancels",
        };
        let g = p.layout_no_wrap(hint.into(), theme::caption(), Color32::WHITE);
        let hr = Rect::from_center_size(egui::pos2(canvas.center().x, canvas.top() + 18.0), g.size() + Vec2::new(20.0, 10.0));
        p.rect_filled(hr, CornerRadius::same(11), Color32::from_black_alpha(170));
        p.galley(hr.min + Vec2::new(10.0, 5.0), g, Color32::WHITE);
        return;
    }

    // A note being written: its box, and a field beside it.
    let Some(m) = a.pending_note.as_mut() else { return };
    let mr = Rect::from_two_pos(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]]));
    paint_mark(&p, mr, MarkKind::Note, "", &t);
    let w = 280.0f32.min(canvas.width() - 16.0);
    let mut at = egui::pos2(mr.left(), mr.bottom() + 8.0);
    at.x = at.x.clamp(canvas.left() + 8.0, canvas.right() - w - 8.0);
    if at.y + 84.0 > canvas.bottom() {
        at.y = (mr.top() - 92.0).max(canvas.top() + 8.0);
    }
    let mut done = None;
    egui::Area::new(egui::Id::unique("ai-note-field")).order(egui::Order::Foreground).fixed_pos(at).show(ui.ctx(), |ui| {
        egui::Frame::new().fill(t.panel).stroke(Stroke::new(1.0, theme::alpha(t.accent, 0.7))).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(w - 20.0);
            ui.label(egui::RichText::new(crate::i18n::tf(format!("Note for the AI · {}", time_label(m.time, fps)))).font(theme::caption()).color(t.text_2));
            let resp = ui.add(egui::TextEdit::multiline(&mut m.note).hint_text("What should change here?").desired_rows(2).desired_width(w - 20.0).font(theme::body()).return_key(Some(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))));
            resp.request_focus();
            if ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                done = Some(true);
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                done = Some(false);
            }
            ui.label(egui::RichText::new(crate::i18n::shown("↵ adds it to the chat · Esc cancels")).font(theme::caption()).color(t.text_2));
        });
    });
    match done {
        Some(true) => {
            let mut m = a.pending_note.take().expect("note");
            m.note = m.note.trim().to_owned();
            if !m.note.is_empty() {
                a.attachments.push(m);
            }
            a.mark_mode = None;
            super::agent_panel::open(app);
        }
        Some(false) => a.pending_note = None,
        None => {}
    }
}

fn paint_mark(p: &egui::Painter, r: Rect, kind: MarkKind, label: &str, t: &theme::Tokens) {
    let colour = match kind {
        MarkKind::Area => t.accent,
        MarkKind::Note => t.warn,
    };
    if r.width() < 3.0 && r.height() < 3.0 {
        // A spot.
        p.circle_filled(r.center(), 9.0, theme::alpha(colour, 0.25));
        p.circle_stroke(r.center(), 9.0, Stroke::new(2.0, colour));
        p.circle_filled(r.center(), 2.5, colour);
    } else {
        p.rect_filled(r, CornerRadius::same(3), theme::alpha(colour, 0.12));
        p.rect_stroke(r, CornerRadius::same(3), Stroke::new(2.0, colour), egui::StrokeKind::Middle);
    }
    if !label.is_empty() {
        let c = r.left_top();
        p.circle_filled(c, 9.0, colour);
        p.text(c, Align2::CENTER_CENTER, label, theme::caption(), t.accent_ink);
    }
}

/// "0:12.4"
pub fn time_label(t: view::Time, _fps: f32) -> String {
    let s = t.max(0) as f64 / SEC as f64;
    format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0)
}
