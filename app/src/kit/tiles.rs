// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tile grid and list for the Library (spec §3): thumbnail (radius 8) + one
//! line label + optional badge; 1 px accent ring on hover with a live
//! preview, 2 px ring when selected, a 60 % opacity drag ghost, and a "+"
//! button on hover that adds the item without selecting it.

use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align2, Sense, Vec2, WidgetInfo, Role};
use std::any::Any;

pub struct TileInfo<'a> {
    pub label: &'a str,
    pub badge: Option<&'a str>,
    /// Optional second line in caption size (e.g. "Edited today").
    pub sub: Option<&'a str>,
    pub selected: bool,
    /// Show the "+" button on hover.
    pub addable: bool,
    /// Thin progress bar along the bottom of the thumbnail (0..1).
    pub progress: Option<f32>,
    /// Longer description, shown as the tooltip.
    pub tip: Option<&'a str>,
    /// Already used in the project: a small ember check badge.
    pub added: bool,
}

#[derive(Default, Debug)]
pub struct TileOutput {
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
    pub added: Option<usize>,
    pub hovered: Option<usize>,
    pub secondary: Option<usize>,
}

/// Paints a thumbnail into `rect`. `hover` is the animated hover amount and
/// `live` is true while the preview should animate.
pub type ThumbPainter<'a> = dyn FnMut(&egui::Painter, Rect, usize, f32, bool) + 'a;

pub struct TileGrid<'a> {
    pub id_salt: &'a str,
    pub aspect: f32,
    pub min_width: f32,
    pub forced: Option<(usize, Forced)>,
    /// Colour behind the grid, used to round thumbnail corners.
    pub surface: Option<Color32>,
    /// Show the name under each thumbnail (off for sets whose names are
    /// just numbered, like sticker packs; the name stays in the tooltip).
    pub labels: bool,
    /// Names centred under the thumbnail in the small caption size (the
    /// library grids); otherwise left-aligned body text.
    pub centred: bool,
}

impl<'a> TileGrid<'a> {
    pub fn new(id_salt: &'a str) -> Self {
        Self { id_salt, aspect: 16.0 / 9.0, min_width: 96.0, forced: None, surface: None, labels: true, centred: false }
    }
    pub fn aspect(mut self, a: f32) -> Self {
        self.aspect = a;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn centred(mut self) -> Self {
        self.centred = true;
        self
    }
    pub fn labels(mut self, on: bool) -> Self {
        self.labels = on;
        self
    }
    pub fn surface(mut self, c: Color32) -> Self {
        self.surface = Some(c);
        self
    }

    pub fn show<P: Any + Send + Sync>(
        self,
        ui: &mut Ui,
        count: usize,
        info: impl Fn(usize) -> TileInfo<'a>,
        payload: impl Fn(usize) -> P,
        thumb: &mut ThumbPainter<'_>,
    ) -> TileOutput {
        let t = theme::tokens(ui.ctx());
        let mut out = TileOutput::default();
        let gap = 8.0;
        let avail = ui.available_width();
        let cols = (((avail + gap) / (self.min_width + gap)).floor() as usize).max(1);
        let tile_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let thumb_h = (tile_w / self.aspect).round();
        let has_sub = count > 0 && info(0).sub.is_some();
        let label_h = if !self.labels { 0.0 } else if has_sub { 40.0 } else { 22.0 };
        let tile_h = thumb_h + label_h;
        let rows = count.div_ceil(cols);
        let total_h = rows as f32 * (tile_h + gap) - gap;
        let (area, _) = ui.allocate_exact_size(Vec2::new(avail, total_h.max(0.0)), Sense::hover());
        let clip = ui.clip_rect();
        let base = ui.scope_id().with(("tiles", self.id_salt));
        for i in 0..count {
            let (row, col) = (i / cols, i % cols);
            let min = area.min + Vec2::new(col as f32 * (tile_w + gap), row as f32 * (tile_h + gap));
            let rect = Rect::from_min_size(min, Vec2::new(tile_w, tile_h));
            if !clip.intersects(rect) {
                continue;
            }
            let ti = info(i);
            let id = base.with(i);
            let resp = ui.interact(rect, id, Sense::click_and_drag());
            let forced = self.forced.filter(|(fi, _)| *fi == i).map(|(_, f)| f);
            let st = State::from_response(ui, &resp, forced);
            let label = tl(ti.label).into_owned();
            let sel = ti.selected;
            resp.widget_info(|| WidgetInfo::selected(Role::Button, true, sel, &label));
            let thumb_rect = Rect::from_min_size(rect.min, Vec2::new(tile_w, thumb_h));
            let cr = CornerRadius::same(theme::radius::CARD);
            let p = ui.painter().with_clip_rect(clip);
            p.rect_filled(thumb_rect, cr, t.sunken);
            let live = st.hover > 0.5 || forced == Some(Forced::Hover);
            {
                let tp = p.with_clip_rect(thumb_rect.intersect(clip));
                thumb(&tp, thumb_rect, i, st.hover, live);
            }
            // Re-round the corners over the thumbnail with a frame in the
            // surface colour, so arbitrary thumbnails get radius 8.
            round_corners(&p, thumb_rect, theme::radius::CARD as f32, self.surface.unwrap_or(ui.visuals().panel_fill));
            if ti.added {
                // A plain dark "Added" label in the bottom-left corner.
                let g = p.layout_no_wrap(shown("Added").into_owned(), theme::caption(), Color32::WHITE);
                let br = Rect::from_min_size(thumb_rect.left_bottom() + Vec2::new(4.0, -g.size().y - 8.0), g.size() + Vec2::new(10.0, 4.0));
                p.rect_filled(br, CornerRadius::same(3), Color32::from_black_alpha(150));
                p.galley(br.center() - g.size() / 2.0, g, Color32::WHITE);
            }
            if let Some(b) = ti.badge {
                let g = p.layout_no_wrap(shown(b).into_owned(), theme::caption(), Color32::WHITE);
                let br = Rect::from_min_size(thumb_rect.right_bottom() - g.size() - Vec2::new(12.0, 8.0) - Vec2::new(4.0, 4.0), g.size() + Vec2::new(12.0, 6.0));
                p.rect_filled(br, CornerRadius::same(4), Color32::from_black_alpha(170));
                p.galley(br.center() - g.size() / 2.0, g, Color32::WHITE);
            }
            if let Some(pr) = ti.progress {
                let bar = Rect::from_min_size(thumb_rect.left_bottom() - Vec2::new(0.0, 3.0), Vec2::new(tile_w * pr.clamp(0.0, 1.0), 3.0));
                p.rect_filled(bar, CornerRadius::same(1), t.accent);
            }
            if ti.selected {
                p.rect_stroke(thumb_rect, cr, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
            } else if st.hover > 0.0 {
                let _ = st.hover;
            }
            if st.focused {
                focus_ring(&p, thumb_rect, cr, &t);
            }
            let color = if ti.selected { t.text } else { theme::mix(t.text_2, t.text, st.hover) };
            let truncated = !self.labels
                || if self.centred {
                    paint_text_clipped(&p, egui::pos2(rect.center().x, thumb_rect.bottom() + 6.0), Align2::CENTER_TOP, ti.label, theme::caption(), if ti.selected { color } else { t.text }, tile_w - 4.0)
                } else {
                    paint_text_clipped(&p, egui::pos2(rect.left() + 2.0, thumb_rect.bottom() + 6.0), Align2::LEFT_TOP, ti.label, theme::body(), color, tile_w - 4.0)
                };

            if let Some(sub) = ti.sub {
                paint_text_clipped(&p, egui::pos2(rect.left() + 2.0, thumb_rect.bottom() + 24.0), Align2::LEFT_TOP, sub, theme::caption(), t.text_2, tile_w - 4.0);
            }
            let mut add_clicked = false;
            if ti.addable && (st.hover > 0.01 || forced == Some(Forced::Hover)) {
                let r = Rect::from_min_size(thumb_rect.right_top() + Vec2::new(-28.0, 4.0), Vec2::splat(24.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                child.set_opacity(st.hover.max((forced == Some(Forced::Hover)) as u8 as f32));
                let add = add_button(&mut child, r, id.with("add"));
                if add.clicked() {
                    add_clicked = true;
                    out.added = Some(i);
                }
            }
            if resp.clicked() && !add_clicked {
                out.clicked = Some(i);
            }
            if resp.double_clicked() {
                out.double_clicked = Some(i);
            }
            if resp.secondary_clicked() {
                out.secondary = Some(i);
            }
            if resp.hovered() {
                out.hovered = Some(i);
            }
            if resp.drag_started() {
                resp.dnd_set_drag_payload(payload(i));
            }
            if resp.dragged() {
                drag_ghost(ui, id, Vec2::new(tile_w, thumb_h), |p, r| thumb(p, r, i, 0.0, false));
            }
            match (truncated, ti.tip) {
                (_, Some(tip)) if !tip.is_empty() => {
                    let label = tl(ti.label).into_owned();
                    let tip = tip.to_owned();
                    resp.on_hover_ui(move |ui| {
                        ui.set_max_width(240.0);
                        ui.label(egui::RichText::new(label).font(theme::body_medium()));
                        ui.label(egui::RichText::new(tip).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
                    });
                }
                (true, _) => {
                    resp.on_hover_text(ti.label);
                }
                _ => {}
            }
        }
        out
    }
}

/// Small round "+" button that floats over thumbnails.
fn add_button(ui: &mut Ui, rect: Rect, id: egui::Id) -> Response {
    let t = theme::tokens(ui.ctx());
    let resp = ui.interact(rect, id, Sense::click());
    let st = State::from_response(ui, &resp, None);
    let p = ui.painter();
    p.circle_filled(rect.center(), 12.0, theme::mix(t.accent, Color32::WHITE, 0.1 * st.hover));
    icons::paint(p, rect, Icon::Plus, 14.0, t.on_accent());
    tip(resp, "Add to timeline", None)
}

/// Mask the four corners of `rect` with `bg` so the content looks rounded
/// with radius `r`.
pub fn round_corners(p: &egui::Painter, rect: Rect, r: f32, bg: Color32) {
    let n = 6;
    for (corner, sx, sy) in [
        (rect.left_top(), 1.0, 1.0),
        (rect.right_top(), -1.0, 1.0),
        (rect.left_bottom(), 1.0, -1.0),
        (rect.right_bottom(), -1.0, -1.0),
    ] {
        let c = corner + Vec2::new(sx * r, sy * r);
        let mut pts = vec![corner];
        for k in 0..=n {
            let a = std::f32::consts::FRAC_PI_2 * k as f32 / n as f32;
            // From the edge point on the x axis round to the y axis.
            pts.push(c + Vec2::new(-sx * r * a.cos(), -sy * r * a.sin()));
        }
        p.add(egui::Shape::convex_polygon(pts, bg, egui::Stroke::NONE));
    }
}

/// Paint a drag ghost (the thumbnail at 60 % opacity) at the pointer.
pub fn drag_ghost(ui: &Ui, id: egui::Id, size: Vec2, paint: impl FnOnce(&egui::Painter, Rect)) {
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let layer = egui::LayerId::new(egui::Order::Tooltip, id.with("ghost"));
    let mut p = ui.ctx().layer_painter(layer);
    let size = size * (120.0 / size.x).min(1.0);
    let rect = Rect::from_min_size(pos + Vec2::new(10.0, 10.0), size);
    p.set_opacity(0.6);
    paint(&p.with_clip_rect(rect), rect);
    let t = theme::tokens(ui.ctx());
    p.rect_stroke(rect, CornerRadius::same(theme::radius::CARD), egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// List variant: 32 px rows with a small thumbnail, the name, and the badge
/// on the right.
pub fn tile_list<'a, P: Any + Send + Sync>(
    ui: &mut Ui,
    id_salt: &str,
    count: usize,
    info: impl Fn(usize) -> TileInfo<'a>,
    payload: impl Fn(usize) -> P,
    thumb: &mut ThumbPainter<'_>,
) -> TileOutput {
    let t = theme::tokens(ui.ctx());
    let mut out = TileOutput::default();
    let w = ui.available_width();
    let base = ui.scope_id().with(("tile-list", id_salt));
    for i in 0..count {
        let ti = info(i);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, theme::space::ROW_H), Sense::click_and_drag());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let st = State::from_response(ui, &resp, None);
        let p = ui.painter();
        let cr = CornerRadius::same(theme::radius::CONTROL);
        if ti.selected {
            p.rect_filled(rect, cr, t.accent_soft);
        } else if st.hover > 0.0 {
            p.rect_filled(rect, cr, theme::alpha(t.raised, st.hover));
        }
        let th = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(42.0, 24.0));
        p.rect_filled(th, CornerRadius::same(4), t.sunken);
        thumb(&p.with_clip_rect(th), th, i, st.hover, st.hover > 0.5);
        round_corners(p, th, 4.0, if ti.selected { t.accent_soft_on(ui.visuals().panel_fill) } else if st.hover > 0.0 { theme::mix(ui.visuals().panel_fill, t.raised, st.hover) } else { ui.visuals().panel_fill });
        let bw = ti.badge.map_or(0.0, |b| text_width(ui, b, theme::caption()) + 12.0);
        let truncated = paint_text_clipped(p, egui::pos2(th.right() + 10.0, rect.center().y), Align2::LEFT_CENTER, ti.label, theme::body(), t.text, rect.right() - th.right() - 18.0 - bw);
        if let Some(b) = ti.badge {
            p.text(egui::pos2(crate::i18n::end_x(rect, 8.0), rect.center().y), crate::i18n::end_center(), shown(b), theme::caption(), t.text_2);
        }
        if st.focused {
            focus_ring(p, rect, cr, &t);
        }
        if resp.clicked() {
            out.clicked = Some(i);
        }
        if resp.double_clicked() {
            out.double_clicked = Some(i);
        }
        if resp.drag_started() {
            resp.dnd_set_drag_payload(payload(i));
        }
        if resp.dragged() {
            drag_ghost(ui, base.with(i), Vec2::new(112.0, 63.0), |p, r| thumb(p, r, i, 0.0, false));
        }
        if truncated {
            resp.on_hover_text(ti.label);
        }
    }
    out
}
