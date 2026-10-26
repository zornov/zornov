// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Empty states (spec §3): an icon, one sentence, one action. Never a blank
//! panel.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, Vec2};

/// Draw an empty state centred in the available space. Returns true when
/// the action button was clicked.
pub fn empty_state(ui: &mut Ui, icon: Icon, sentence: &str, action: Option<(&str, Icon)>) -> bool {
    let t = theme::tokens(ui.ctx());
    let avail = ui.available_rect_before_wrap();
    let content_h = 64.0 + 14.0 + 36.0 + if action.is_some() { 12.0 + theme::space::CONTROL_H } else { 0.0 };
    let top = avail.top() + ((avail.height() - content_h) / 2.0).max(8.0);
    let cx = avail.center().x;
    let (_, _) = ui.allocate_exact_size(avail.size().max(Vec2::new(0.0, content_h + 16.0)), Sense::hover());
    let p = ui.painter();
    illustration(p, egui::pos2(cx, top + 32.0), icon, t.text_2);
    // One sentence, wrapped to a comfortable width.
    let wrap = (avail.width() - 32.0).clamp(80.0, 260.0);
    let galley = p.layout(wrapped(p, sentence, theme::body(), wrap), theme::body(), t.text_2, if studio_i18n::rtl::has_rtl(&tl(sentence)) { f32::INFINITY } else { wrap });
    let text_top = top + 64.0 + 14.0;
    let gsize = galley.size();
    p.galley(egui::pos2(cx - gsize.x / 2.0, text_top), galley, t.text_2);
    let mut clicked = false;
    if let Some((label, icon)) = action {
        let bw = text_width(ui, label, theme::control_medium()) + 24.0 + 22.0;
        let r = Rect::from_center_size(egui::pos2(cx, text_top + gsize.y + 12.0 + theme::space::CONTROL_H / 2.0), Vec2::new(bw, theme::space::CONTROL_H));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        clicked = Button::secondary(label).icon(icon).show(&mut child).clicked();
    }
    let _ = Align2::CENTER_CENTER;
    clicked
}

/// A small line-art picture (about 96 × 64) for an empty state, drawn in
/// strokes of one colour.
pub fn illustration(p: &egui::Painter, c: egui::Pos2, icon: Icon, colour: Color32) {
    let s = egui::Stroke::new(1.5, colour);
    let faint = egui::Stroke::new(1.5, theme::alpha(colour, 0.45));
    let card = |r: Rect, st: egui::Stroke| p.rect_stroke(r, CornerRadius::same(6), st, egui::StrokeKind::Middle);
    match icon {
        Icon::FilmStrip | Icon::Image => {
            // Two stacked frames; the front one holds hills and a sun.
            card(Rect::from_center_size(c + Vec2::new(10.0, -8.0), Vec2::new(64.0, 44.0)), faint);
            let f = Rect::from_center_size(c + Vec2::new(-4.0, 4.0), Vec2::new(64.0, 44.0));
            p.rect_filled(f, CornerRadius::same(6), theme::alpha(colour, 0.0));
            card(f, s);
            let hills: Vec<egui::Pos2> = (0..=16)
                .map(|i| {
                    let u = i as f32 / 16.0;
                    egui::pos2(f.left() + 4.0 + u * (f.width() - 8.0), f.bottom() - 10.0 - (u * 7.0).sin() * 5.0 - u * 4.0)
                })
                .collect();
            p.add(egui::Shape::line(hills, s));
            p.circle_stroke(f.left_top() + Vec2::new(46.0, 13.0), 5.0, s);
        }
        Icon::MagnifyingGlass => {
            p.circle_stroke(c - Vec2::new(6.0, 6.0), 16.0, s);
            p.line_segment([c + Vec2::new(5.5, 5.5), c + Vec2::new(18.0, 18.0)], egui::Stroke::new(3.0, colour));
            for (a, b) in [(Vec2::new(26.0, -20.0), Vec2::new(26.0, -12.0)), (Vec2::new(22.0, -16.0), Vec2::new(30.0, -16.0))] {
                p.line_segment([c + a, c + b], faint);
            }
        }
        Icon::FilmSlate => {
            let body = Rect::from_center_size(c + Vec2::new(0.0, 6.0), Vec2::new(64.0, 38.0));
            card(body, s);
            let top = Rect::from_min_size(body.left_top() - Vec2::new(0.0, 14.0), Vec2::new(64.0, 10.0));
            card(top, s);
            for k in 0..4 {
                let x = top.left() + 8.0 + k as f32 * 15.0;
                p.line_segment([egui::pos2(x, top.bottom()), egui::pos2(x + 8.0, top.top())], s);
            }
            p.line_segment([body.left_top() + Vec2::new(8.0, 12.0), body.left_top() + Vec2::new(40.0, 12.0)], faint);
            p.line_segment([body.left_top() + Vec2::new(8.0, 22.0), body.left_top() + Vec2::new(30.0, 22.0)], faint);
        }
        _ => {
            let r = Rect::from_center_size(c, Vec2::new(64.0, 48.0));
            for w in [[r.left_top(), r.right_top()], [r.right_top(), r.right_bottom()], [r.right_bottom(), r.left_bottom()], [r.left_bottom(), r.left_top()]] {
                p.extend(egui::Shape::dashed_line(&w, faint, 4.0, 3.0));
            }
            icons::paint(p, Rect::from_center_size(c, Vec2::splat(24.0)), icon, 24.0, colour);
        }
    }
}
