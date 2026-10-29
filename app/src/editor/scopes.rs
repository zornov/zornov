// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Video scopes over the viewer: luma waveform, RGB parade, vectorscope
//! (with the colour-bar targets and the skin line) and histogram, read
//! from the frame the viewer is showing, plus the numbers a colourist
//! checks (clipping, black level, cast, saturation).

use crate::app::StudioApp;
use crate::kit::{self, Segment};
use crate::theme;
use eframe::egui::{self, Align2, Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use studio_colour::image::Image;
use studio_colour::scopes::{self, Readings, ScopeSettings};
use studio_colour::transfer::Transfer;
use studio_colour::ycbcr::YCbCrMatrix;

const KINDS: [&str; 4] = ["Waveform", "Parade", "Vector", "Histogram"];

fn on_id() -> egui::Id {
    egui::Id::unique("scopes-on")
}

/// Whether the scopes are showing.
pub fn is_on(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| *d.get_persisted_mut_or_default::<bool>(on_id()))
}

/// Show or hide the scopes (a workspace does this).
pub fn set_on(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_persisted(on_id(), on));
}

/// Settings for frames that are already display-encoded 8-bit RGB.
pub fn settings(max_width: u32) -> ScopeSettings {
    ScopeSettings { max_width, transfer: Transfer::Linear, matrix: YCbCrMatrix::Bt709, levels: 128, vectorscope_size: 160 }
}

/// An encoded RGBA8 frame sampled down to at most `max_w` wide (each
/// sample the mean of its cell's corners and centre: cheap, and steady
/// on fine detail).
pub fn sample(rgba: &[u8], w: u32, h: u32, max_w: u32) -> Image {
    let ow = w.min(max_w).max(1);
    let oh = ((h as u64 * ow as u64 / w.max(1) as u64) as u32).max(1);
    let px = |x: u32, y: u32| -> [f32; 4] {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        [rgba[i] as f32 / 255.0, rgba[i + 1] as f32 / 255.0, rgba[i + 2] as f32 / 255.0, 1.0]
    };
    Image::from_fn(ow, oh, |x, y| {
        let (x0, x1) = (x * w / ow, ((x + 1) * w / ow).max(x * w / ow + 1) - 1);
        let (y0, y1) = (y * h / oh, ((y + 1) * h / oh).max(y * h / oh + 1) - 1);
        let pts = [px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1), px((x0 + x1) / 2, (y0 + y1) / 2)];
        let mut s = [0.0; 4];
        for p in pts {
            for k in 0..4 {
                s[k] += p[k] / 5.0;
            }
        }
        s
    })
}

/// Counts as brightness: a log curve so a few pixels still show.
fn glow(count: u32, total: u32, width: u32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let per_column = (total as f32 / width.max(1) as f32).max(1.0);
    ((count as f32).ln_1p() / (per_column * 0.25).ln_1p()).clamp(0.08, 1.0)
}

/// The scope picture for `kind` (0 waveform, 1 parade, 2 vectorscope).
pub fn picture(img: &Image, kind: usize, s: &ScopeSettings) -> ColorImage {
    match kind {
        0 | 1 => {
            let waves: Vec<(scopes::Waveform, [f32; 3])> = if kind == 0 {
                vec![(scopes::waveform_luma(img, s), [0.75, 1.0, 0.8])]
            } else {
                let [r, g, b] = scopes::parade(img, s);
                vec![(r, [1.0, 0.35, 0.3]), (g, [0.35, 1.0, 0.4]), (b, [0.4, 0.55, 1.0])]
            };
            let cols: u32 = waves.iter().map(|w| w.0.width).sum::<u32>() + 2 * (waves.len() as u32 - 1);
            let rows = s.levels;
            let mut out = ColorImage::filled([cols as usize, rows as usize], Color32::TRANSPARENT);
            let mut x0 = 0;
            for (w, tint) in &waves {
                let total: u32 = w.data.iter().sum();
                for x in 0..w.width {
                    for l in 0..w.levels {
                        let g = glow(w.get(x, l), total, w.width);
                        if g > 0.0 {
                            let c = tint.map(|t| (t * g * 255.0) as u8);
                            out[((x0 + x) as usize, (rows - 1 - l) as usize)] = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (g * 255.0) as u8);
                        }
                    }
                }
                x0 += w.width + 2;
            }
            out
        }
        _ => {
            let v = scopes::vectorscope(img, s);
            let n = v.size as usize;
            let total: u32 = v.data.iter().sum();
            let mut out = ColorImage::filled([n, n], Color32::TRANSPARENT);
            for row in 0..n {
                for col in 0..n {
                    let g = glow(v.data[row * n + col], total, (n as u32) * 2);
                    if g > 0.0 {
                        // Tinted by where it sits: the hue it measures.
                        let (cb, cr) = (col as f32 / n as f32 - 0.5, 0.5 - row as f32 / n as f32);
                        let rgb = YCbCrMatrix::Bt709.from_ycbcr([0.6, cb, cr]).map(|c| (c.clamp(0.0, 1.0) * 0.6 + 0.4) * g * 255.0);
                        out[(col, row)] = Color32::from_rgba_unmultiplied(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, (g * 255.0) as u8);
                    }
                }
            }
            out
        }
    }
}

/// The readings in a few words, for the panel and the AI.
pub fn words(r: &Readings) -> Vec<String> {
    let pct = |v: f32| format!("{:.0}%", v * 100.0);
    let mut out = vec![crate::i18n::tf(format!("Black {} · mid {} · white {}", pct(r.luma[0]), pct(r.luma[2]), pct(r.luma[4])))];
    let mut warn = Vec::new();
    if r.clipped > 0.01 {
        warn.push(format!("{} blown out", pct(r.clipped)));
    }
    if r.crushed > 0.05 {
        warn.push(format!("{} crushed", pct(r.crushed)));
    }
    if r.oversaturated > 0.01 {
        warn.push(format!("{} oversaturated", pct(r.oversaturated)));
    }
    if !warn.is_empty() {
        out.push(warn.join(" · "));
    }
    for (name, c) in ["shadows", "mids", "highlights"].iter().zip(r.cast) {
        if let Some(c) = c
            && let Some(t) = cast_name(c)
        {
            out.push(format!("{t} {name}"));
        }
    }
    out
}

/// The tint a zone's (R, G, B) offsets show, if it is strong enough to
/// see (about 2 % of the range).
pub fn cast_name(c: [f32; 3]) -> Option<&'static str> {
    let [r, g, b] = c;
    let spread = r.max(g).max(b) - r.min(g).min(b);
    if spread < 0.02 {
        return None;
    }
    Some(if r > g && r > b {
        if g > b { "warm (orange)" } else { "magenta-red" }
    } else if b > r && b > g {
        if g > r { "cool (cyan-blue)" } else { "blue-violet" }
    } else if r > b {
        "yellow-green"
    } else {
        "green"
    })
}

#[derive(Clone)]
struct Cache {
    stamp: u64,
    kind: usize,
    tex: Option<TextureHandle>,
    hist: Option<scopes::Histogram>,
    readings: Readings,
}

/// Draws the scopes over the viewer's `area` when they are on and not
/// docked as a panel of their own.
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect) {
    let on = is_on(ui.ctx());
    let docked = super::dock_ui::docked(ui.ctx(), crate::dock::Panel::Scopes);
    #[cfg(feature = "render")]
    if let Some(rv) = app.render.as_mut() {
        rv.keep_pixels = on || docked;
    }
    if !on || docked {
        return;
    }
    let size = Vec2::new(300.0f32.min(area.width() * 0.5), 236.0f32.min(area.height() * 0.7));
    let panel = Rect::from_min_size(area.right_bottom() - size - Vec2::new(10.0, 52.0), size);
    ui.interact(panel, egui::Id::unique("scopes-panel"), Sense::click_and_drag());
    draw(app, ui, panel);
}

/// The scopes docked as a panel in `r`.
pub fn docked(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    #[cfg(feature = "render")]
    if let Some(rv) = app.render.as_mut() {
        rv.keep_pixels = true;
    }
    draw(app, ui, r.shrink(6.0));
}

/// The scope chooser, the plot and the readings in `panel`.
fn draw(app: &mut StudioApp, ui: &mut egui::Ui, panel: Rect) {
    let t = theme::tokens(ui.ctx());
    let kind_id = egui::Id::unique("scopes-kind");
    let mut kind = ui.ctx().data_mut(|d| *d.get_persisted_mut_or_default::<usize>(kind_id)).min(3);
    let p = ui.painter().with_clip_rect(panel);
    p.rect_filled(panel, 8, Color32::from_black_alpha(215));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel.shrink(6.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let segs: Vec<Segment> = KINDS.iter().map(|k| Segment::text(*k)).collect();
    if kit::segmented(&mut child, "scope-kind", &mut kind, &segs, Some(panel.width() - 12.0), None).changed() {
        ui.ctx().data_mut(|d| d.insert_persisted(kind_id, kind));
    }
    let plot = Rect::from_min_max(child.cursor().min + Vec2::new(0.0, 4.0), panel.max - Vec2::new(6.0, 46.0));
    let footer = Rect::from_min_max(egui::pos2(panel.left() + 8.0, plot.bottom() + 4.0), panel.max - Vec2::new(6.0, 4.0));

    #[cfg(feature = "render")]
    let frame = app.render.as_ref().and_then(|rv| rv.last_pixels.as_ref().map(|(w, h, px, stamp)| (*w, *h, px.clone(), *stamp)));
    #[cfg(not(feature = "render"))]
    let frame: Option<(u32, u32, std::sync::Arc<Vec<u8>>, u64)> = None;
    let Some((w, h, px, stamp)) = frame else {
        p.text(plot.center(), Align2::CENTER_CENTER, crate::i18n::shown("Scopes read the rendered preview"), theme::caption(), t.text_2);
        return;
    };
    let cache_id = egui::Id::unique("scopes-cache");
    let mut cache = ui.ctx().data(|d| d.get_temp::<Cache>(cache_id));
    // While playing, a few updates a second are plenty.
    let now = ui.input(|i| i.time);
    let last_at = ui.ctx().data(|d| d.get_temp::<f64>(egui::Id::unique("scopes-at"))).unwrap_or(0.0);
    let stale = cache.as_ref().is_none_or(|c| c.stamp != stamp || c.kind != kind);
    if stale && (!app.editor.playing || now - last_at > 0.2 || cache.as_ref().is_none_or(|c| c.kind != kind)) {
        let s = settings(256);
        let img = sample(&px, w, h, 256);
        let readings = scopes::readings(&img, &s);
        let (tex, hist) = if kind == 3 {
            (None, Some(scopes::histogram(&img, &ScopeSettings { levels: 128, ..s })))
        } else {
            let pic = picture(&img, kind, &s);
            let tex = match cache.as_ref().and_then(|c| c.tex.clone()) {
                Some(mut t) => {
                    t.set(pic, TextureOptions::LINEAR);
                    t
                }
                None => ui.ctx().load_texture("scopes", pic, TextureOptions::LINEAR),
            };
            (Some(tex), None)
        };
        cache = Some(Cache { stamp, kind, tex, hist, readings });
        ui.ctx().data_mut(|d| {
            d.insert_temp(cache_id, cache.clone().expect("just made"));
            d.insert_temp(egui::Id::unique("scopes-at"), now);
        });
    } else if stale {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    let Some(c) = cache else { return };
    let grid = Stroke::new(1.0, Color32::from_white_alpha(28));
    match kind {
        0 | 1 => {
            // 0, 25, 50, 75, 100 % lines, and broadcast range dashed.
            for k in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let y = egui::lerp(plot.bottom()..=plot.top(), k);
                p.hline(plot.x_range(), y, grid);
                p.text(egui::pos2(plot.left() + 2.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{}", (k * 100.0) as u32), theme::caption(), Color32::from_white_alpha(90));
            }
            if let Some(tex) = &c.tex {
                p.image(tex.id(), plot, Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        2 => {
            let side = plot.width().min(plot.height());
            let sq = Rect::from_center_size(plot.center(), Vec2::splat(side));
            p.circle_stroke(sq.center(), side / 2.0, grid);
            p.circle_stroke(sq.center(), side * 0.375, grid);
            p.line_segment([egui::pos2(sq.left(), sq.center().y), egui::pos2(sq.right(), sq.center().y)], grid);
            p.line_segment([egui::pos2(sq.center().x, sq.top()), egui::pos2(sq.center().x, sq.bottom())], grid);
            let at = |cb: f32, cr: f32| {
                let (u, v) = scopes::vectorscope_position(cb, cr);
                egui::pos2(sq.left() + u * sq.width(), sq.top() + v * sq.height())
            };
            // The skin line: faces of every complexion sit along it.
            let (sb, sr) = scopes::skin_line(0.5);
            p.line_segment([sq.center(), at(sb, sr)], Stroke::new(1.0, Color32::from_rgb(220, 160, 120)));
            for tg in scopes::vectorscope_targets(YCbCrMatrix::Bt709) {
                let c0 = at(tg.cb, tg.cr);
                p.rect_stroke(Rect::from_center_size(c0, Vec2::splat(7.0)), 0, Stroke::new(1.0, Color32::from_white_alpha(110)), egui::StrokeKind::Middle);
                p.text(c0 + Vec2::new(6.0, -6.0), Align2::LEFT_BOTTOM, tg.name, theme::caption(), Color32::from_white_alpha(120));
            }
            if let Some(tex) = &c.tex {
                p.image(tex.id(), sq, Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        _ => {
            if let Some(hg) = &c.hist {
                let most = hg.red.iter().chain(&hg.green).chain(&hg.blue).copied().max().unwrap_or(1).max(1) as f32;
                for (bins, col) in [(&hg.red, Color32::from_rgb(240, 90, 80)), (&hg.green, Color32::from_rgb(90, 220, 110)), (&hg.blue, Color32::from_rgb(100, 140, 250)), (&hg.luma, Color32::from_white_alpha(200))] {
                    let n = bins.len().max(2);
                    let pts: Vec<Pos2> = bins.iter().enumerate().map(|(i, v)| egui::pos2(egui::lerp(plot.x_range(), i as f32 / (n - 1) as f32), plot.bottom() - plot.height() * (*v as f32 / most).sqrt())).collect();
                    p.add(egui::Shape::line(pts, Stroke::new(1.2, col)));
                }
            }
        }
    }
    let lines = words(&c.readings);
    for (i, l) in lines.iter().take(3).enumerate() {
        let col = if i == 1 && (c.readings.clipped > 0.01 || c.readings.crushed > 0.05 || c.readings.oversaturated > 0.01) { t.warn } else { Color32::from_white_alpha(190) };
        p.text(footer.left_top() + Vec2::new(0.0, i as f32 * 13.0), Align2::LEFT_TOP, l, theme::caption(), col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_samples_down_and_reads_back() {
        // A left half black, right half white 640x360 frame.
        let (w, h) = (640u32, 360u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in w / 2..w {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let img = sample(&px, w, h, 256);
        assert_eq!((img.width, img.height), (256, 144));
        let r = scopes::readings(&img, &settings(256));
        assert!((r.clipped - 0.5).abs() < 0.02 && (r.crushed - 0.5).abs() < 0.02, "{r:?}");
        let pic = picture(&img, 0, &settings(256));
        assert_eq!(pic.size, [256, 128]);
        // Bright at the bottom on the left, at the top on the right.
        assert!(pic[(10, 127)].a() > 0 && pic[(250, 0)].a() > 0 && pic[(10, 0)].a() == 0);
        let words = words(&r);
        assert!(words.iter().any(|l| l.contains("blown out")), "{words:?}");
        assert_eq!(cast_name([0.05, 0.0, -0.05]), Some("warm (orange)"));
        assert_eq!(cast_name([0.0, 0.005, 0.0]), None);
    }
}
