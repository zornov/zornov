// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Colour well and its picker: saturation/value square, hue strip, hex
//! field, eyedropper and recent colours.

use super::*;
use crate::icons::Icon;
use eframe::egui::{ecolor::Hsva, Sense, Vec2, WidgetInfo, Role};

fn recent_id() -> egui::Id {
    egui::Id::unique("studio-recent-colours")
}

pub fn recent_colours(ctx: &egui::Context) -> Vec<Color32> {
    ctx.data_mut(|d| d.get_persisted::<Vec<[u8; 4]>>(recent_id()).unwrap_or_default())
        .into_iter()
        .map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a))
        .collect()
}

fn push_recent(ctx: &egui::Context, c: Color32) {
    let mut v = recent_colours(ctx);
    v.retain(|x| *x != c);
    v.insert(0, c);
    v.truncate(8);
    let raw: Vec<[u8; 4]> = v.iter().map(|c| c.to_srgba_unmultiplied()).collect();
    ctx.data_mut(|d| d.insert_persisted(recent_id(), raw));
}

/// Set when the user presses the eyedropper; the viewer reads it and picks
/// the next clicked pixel.
pub fn eyedropper_id() -> egui::Id {
    egui::Id::unique("studio-eyedropper-active")
}

pub fn to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

pub fn from_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
    match s.len() {
        6 => Some(Color32::from_rgb(p(0)?, p(2)?, p(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(p(0)?, p(2)?, p(4)?, p(6)?)),
        3 => {
            let q = |i: usize| u8::from_str_radix(&s[i..i + 1].repeat(2), 16).ok();
            Some(Color32::from_rgb(q(0)?, q(1)?, q(2)?))
        }
        _ => None,
    }
}

fn checker(painter: &egui::Painter, rect: Rect, cr: CornerRadius) {
    painter.rect_filled(rect, cr, Color32::from_gray(200));
    let s = 5.0;
    let p = painter.with_clip_rect(rect.shrink(1.0));
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { s };
        while x < rect.right() {
            p.rect_filled(Rect::from_min_size(egui::pos2(x, y), Vec2::splat(s)), 0, Color32::from_gray(140));
            x += s * 2.0;
        }
        y += s;
        row += 1;
    }
}

/// A 28 px swatch that opens the picker.
pub fn colour_well(ui: &mut Ui, id_salt: &str, colour: &mut Color32, forced: Option<Forced>) -> Response {
    let t = theme::tokens(ui.ctx());
    let id = ui.scope_id().with(("colour-well", id_salt));
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(theme::space::CONTROL_H), Sense::click());
    let st = State::from_response(ui, &resp, forced);
    let hex = to_hex(*colour);
    resp.widget_info(|| WidgetInfo::labeled(Role::ColorWell, !st.disabled, &hex));
    let cr = CornerRadius::same(theme::radius::CONTROL);
    let p = ui.painter();
    if colour.a() < 255 {
        checker(p, rect, cr);
    }
    p.rect_filled(rect, cr, *colour);
    p.rect_stroke(rect, cr, egui::Stroke::new(1.0, theme::alpha(if t.dark { Color32::WHITE } else { Color32::BLACK }, 0.12 + 0.1 * st.hover)), egui::StrokeKind::Inside);
    let open = is_open(ui.ctx(), id);
    if st.focused || open {
        focus_ring(p, rect, cr, &t);
    }
    if resp.clicked() && forced.is_none() {
        toggle_open(ui.ctx(), id);
        if !open {
            push_recent(ui.ctx(), *colour);
        }
    }
    if forced.is_none() {
        let before = *colour;
        popover(ui.ctx(), id, rect, Side::Below, 232.0, |ui| picker(ui, id, colour));
        if *colour != before {
            resp.mark_changed();
        }
    }
    tip(resp, &hex, None)
}

/// The picker body; also usable inline (the gallery shows it open).
pub fn picker(ui: &mut Ui, id: egui::Id, colour: &mut Color32) {
    let t = theme::tokens(ui.ctx());
    ui.spacing_mut().item_spacing.y = 8.0;
    // Keep hue stable while saturation is zero by remembering it.
    let hsv_id = id.with("hsva");
    let mut hsva: Hsva = ui.data(|d| d.get_temp::<Hsva>(hsv_id)).filter(|h| Color32::from(*h) == *colour).unwrap_or_else(|| Hsva::from(*colour));
    let w = ui.available_width().max(200.0);

    // Saturation / value square.
    let (sq, sresp) = ui.allocate_exact_size(Vec2::new(w, 140.0), Sense::click_and_drag());
    {
        let p = ui.painter();
        let n = 24;
        let mut mesh = egui::Mesh::default();
        for j in 0..=n {
            for i in 0..=n {
                let s = i as f32 / n as f32;
                let v = 1.0 - j as f32 / n as f32;
                let c: Color32 = Hsva::new(hsva.h, s, v, 1.0).into();
                mesh.colored_vertex(egui::pos2(sq.left() + s * sq.width(), sq.top() + (1.0 - v) * sq.height()), c);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = (j * (n + 1) + i) as u32;
                let b = a + 1;
                let c = a + (n + 1) as u32;
                let d = c + 1;
                mesh.add_triangle(a, b, c);
                mesh.add_triangle(b, d, c);
            }
        }
        p.add(egui::Shape::mesh(mesh));
        p.rect_stroke(sq, CornerRadius::same(6), egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        let knob = egui::pos2(sq.left() + hsva.s * sq.width(), sq.top() + (1.0 - hsva.v) * sq.height());
        p.circle_stroke(knob, 6.0, egui::Stroke::new(2.0, Color32::WHITE));
        p.circle_stroke(knob, 7.5, egui::Stroke::new(1.0, Color32::from_black_alpha(90)));
    }
    if let Some(pos) = sresp.interact_pointer_pos() {
        if sresp.dragged() || sresp.clicked() {
            hsva.s = ((pos.x - sq.left()) / sq.width()).clamp(0.0, 1.0);
            hsva.v = 1.0 - ((pos.y - sq.top()) / sq.height()).clamp(0.0, 1.0);
        }
    }

    // Hue strip.
    let (strip, hresp) = ui.allocate_exact_size(Vec2::new(w, 12.0), Sense::click_and_drag());
    {
        let p = ui.painter();
        let n = 36;
        let mut mesh = egui::Mesh::default();
        for i in 0..=n {
            let h = i as f32 / n as f32;
            let c: Color32 = Hsva::new(h, 1.0, 1.0, 1.0).into();
            let x = strip.left() + h * strip.width();
            mesh.colored_vertex(egui::pos2(x, strip.top()), c);
            mesh.colored_vertex(egui::pos2(x, strip.bottom()), c);
        }
        for i in 0..n as u32 {
            let a = i * 2;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 1, a + 3, a + 2);
        }
        p.add(egui::Shape::mesh(mesh));
        let x = strip.left() + hsva.h * strip.width();
        let knob = Rect::from_center_size(egui::pos2(x, strip.center().y), Vec2::new(6.0, 16.0));
        p.rect(knob, CornerRadius::same(3), Color32::WHITE, egui::Stroke::new(1.0, Color32::from_black_alpha(90)), egui::StrokeKind::Outside);
    }
    if let Some(pos) = hresp.interact_pointer_pos() {
        if hresp.dragged() || hresp.clicked() {
            hsva.h = ((pos.x - strip.left()) / strip.width()).clamp(0.0, 0.9999);
        }
    }
    let new: Color32 = hsva.into();
    if new != *colour && (sresp.dragged() || sresp.clicked() || hresp.dragged() || hresp.clicked()) {
        *colour = new;
    }
    ui.data_mut(|d| d.insert_temp(hsv_id, hsva));

    // Hex + eyedropper.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let hex_id = id.with("hex");
        let mut text: String = ui.data(|d| d.get_temp(hex_id)).unwrap_or_else(|| to_hex(*colour));
        let r = text_field(ui, &mut text, "Colour code", None, w - 40.0, None);
        if r.has_focus() {
            ui.data_mut(|d| d.insert_temp(hex_id, text.clone()));
        } else {
            ui.data_mut(|d| d.remove::<String>(hex_id));
        }
        if r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            if let Some(c) = from_hex(&text) {
                *colour = c;
            }
        }
        let active = ui.data(|d| d.get_temp(eyedropper_id()).unwrap_or(false));
        if IconButton::new(Icon::Eyedropper, "Pick from viewer").active(active).show(ui).clicked() {
            ui.data_mut(|d| d.insert_temp(eyedropper_id(), !active));
        }
    });

    // Recent colours.
    let recents = recent_colours(ui.ctx());
    if !recents.is_empty() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for c in recents {
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                let st = State::from_response(ui, &resp, None);
                ui.painter().rect_filled(r, CornerRadius::same(4), c);
                ui.painter().rect_stroke(r, CornerRadius::same(4), egui::Stroke::new(1.0 + st.hover, theme::alpha(t.text, 0.15 + 0.3 * st.hover)), egui::StrokeKind::Inside);
                if resp.clicked() {
                    *colour = c;
                }
                tip(resp, &to_hex(c), None);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let c = Color32::from_rgb(0x8B, 0x6C, 0xFF);
        assert_eq!(to_hex(c), "#8B6CFF");
        assert_eq!(from_hex("#8B6CFF"), Some(c));
        assert_eq!(from_hex("fff"), Some(Color32::WHITE));
        assert_eq!(from_hex("zz"), None);
    }
}
