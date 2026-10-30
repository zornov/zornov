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

/// Which trace the waveform draws: the luma, or the three channels
/// laid over each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trace {
    Luma,
    Rgb,
}

/// The scope picture for `kind` (0 waveform, 1 parade, 2 vectorscope).
/// Colours are premultiplied: add them over the plot.
pub fn picture(img: &Image, kind: usize, s: &ScopeSettings) -> ColorImage {
    picture_with(img, kind, s, Trace::Luma)
}

/// [`picture`] with the waveform's trace chosen.
pub fn picture_with(img: &Image, kind: usize, s: &ScopeSettings, trace: Trace) -> ColorImage {
    match kind {
        0 | 1 => {
            let luma = [0.75, 1.0, 0.8];
            let (red, green, blue) = ([1.0, 0.3, 0.25], [0.3, 1.0, 0.35], [0.35, 0.5, 1.0]);
            let overlay = kind == 0 && trace == Trace::Rgb;
            let waves: Vec<(scopes::Waveform, [f32; 3])> = if kind == 0 && !overlay {
                vec![(scopes::waveform_luma(img, s), luma)]
            } else {
                let [r, g, b] = scopes::parade(img, s);
                vec![(r, red), (g, green), (b, blue)]
            };
            let side_by_side = !overlay;
            let gap = 2;
            let cols: u32 = if side_by_side { waves.iter().map(|w| w.0.width).sum::<u32>() + gap * (waves.len() as u32 - 1) } else { waves[0].0.width };
            let rows = s.levels;
            let mut buf = vec![[0.0f32; 3]; (cols * rows) as usize];
            let mut x0 = 0;
            for (w, tint) in &waves {
                let per_column = w.per_column();
                for x in 0..w.width {
                    for l in 0..w.levels {
                        let g = scopes::trace_intensity(w.get(x, l), per_column);
                        if g > 0.0 {
                            // A neighbour row at half strength joins the dots into a line.
                            for (dy, k) in [(0i32, 1.0), (-1, 0.45), (1, 0.45)] {
                                let y = (rows - 1 - l) as i32 + dy;
                                if (0..rows as i32).contains(&y) {
                                    let px = &mut buf[y as usize * cols as usize + (x0 + x) as usize];
                                    for i in 0..3 {
                                        px[i] = (px[i] + tint[i] * g * k).min(1.0);
                                    }
                                }
                            }
                        }
                    }
                }
                if side_by_side {
                    x0 += w.width + gap;
                }
            }
            premultiplied(&buf, cols as usize, rows as usize)
        }
        _ => {
            let v = scopes::vectorscope(img, s);
            let n = v.size as usize;
            let total: u32 = v.data.iter().sum();
            let mut buf = vec![[0.0f32; 3]; n * n];
            for row in 0..n {
                for col in 0..n {
                    let g = scopes::trace_intensity(v.data[row * n + col], total / 50);
                    if g > 0.0 {
                        // Tinted by where it sits: the hue it measures.
                        let (cb, cr) = (col as f32 / n as f32 - 0.5, 0.5 - row as f32 / n as f32);
                        let rgb = YCbCrMatrix::Bt709.from_ycbcr([0.6, cb, cr]).map(|c| c.clamp(0.0, 1.0) * 0.6 + 0.4);
                        for (dx, dy, k) in [(0i32, 0i32, 1.0f32), (-1, 0, 0.5), (1, 0, 0.5), (0, -1, 0.5), (0, 1, 0.5)] {
                            let (x, y) = (col as i32 + dx, row as i32 + dy);
                            if (0..n as i32).contains(&x) && (0..n as i32).contains(&y) {
                                let px = &mut buf[y as usize * n + x as usize];
                                for i in 0..3 {
                                    px[i] = px[i].max(rgb[i] * g * k);
                                }
                            }
                        }
                    }
                }
            }
            premultiplied(&buf, n, n)
        }
    }
}

/// Light that adds onto the plot: the colour as it is, the alpha a share
/// of it so the graticule still shows through a thin trace.
fn premultiplied(buf: &[[f32; 3]], w: usize, h: usize) -> ColorImage {
    let mut out = ColorImage::filled([w, h], Color32::TRANSPARENT);
    for (px, c) in out.pixels.iter_mut().zip(buf) {
        let m = c[0].max(c[1]).max(c[2]);
        if m > 0.0 {
            *px = Color32::from_rgba_premultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (m * 0.6 * 255.0) as u8);
        }
    }
    out
}

/// The readings in a few words, for the panel and the AI: the range the
/// picture spans, then only what is wrong with it, then any colour cast.
pub fn words(r: &Readings) -> Vec<String> {
    let pct = |v: f32| format!("{:.0}%", v * 100.0);
    let (lo, hi) = (r.luma[1], r.luma[3]);
    let range = if r.luma[4] < 0.01 {
        "Black frame".to_owned()
    } else if lo > 0.99 {
        "White frame".to_owned()
    } else if hi - lo < 0.02 {
        format!("Flat at {}", pct(r.luma[2]))
    } else {
        format!("Levels {}–{}", pct(lo), pct(hi))
    };
    let mut out = vec![crate::i18n::tf(range)];
    if r.clipped > 0.01 {
        out.push(format!("{} blown out", pct(r.clipped)));
    }
    if r.crushed > 0.05 {
        out.push(format!("{} crushed to black", pct(r.crushed)));
    }
    if r.oversaturated > 0.01 {
        out.push(format!("{} oversaturated", pct(r.oversaturated)));
    }
    if r.outside_legal > 0.02 && r.clipped <= 0.01 && r.crushed <= 0.05 {
        out.push(format!("{} outside broadcast range", pct(r.outside_legal)));
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

/// Whether a line of [`words`] is a warning (shown in the warning colour).
fn is_warning(line: &str) -> bool {
    ["blown out", "crushed", "oversaturated", "outside broadcast"].iter().any(|w| line.contains(w))
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
    trace: Trace,
    tex: Option<TextureHandle>,
    hist: Option<scopes::Histogram>,
    readings: Readings,
}

/// What the scopes show and how: the tab and the two display options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    kind: usize,
    /// Scale in 10-bit code values instead of percent.
    ten_bit: bool,
    /// The waveform as red, green and blue instead of luma.
    rgb: bool,
}

impl View {
    fn trace(&self) -> Trace {
        if self.rgb { Trace::Rgb } else { Trace::Luma }
    }
}

fn view_id() -> egui::Id {
    egui::Id::unique("scopes-view")
}

/// Where each part of the scopes panel goes. The parts never overlap:
/// scale labels have a gutter of their own beside the plot.
#[derive(Clone, Copy, Debug)]
struct Layout {
    tabs: Rect,
    options: Rect,
    /// Scale labels (waveform, parade), or the empty strip beside the plot.
    gutter: Rect,
    plot: Rect,
    /// Scale labels under the plot (histogram).
    axis: Rect,
    footer: Rect,
}

const LINE: f32 = 15.0;
const GUTTER: f32 = 30.0;

/// Splits `panel` for `kind` and `lines` lines of readings.
fn layout(panel: Rect, kind: usize, lines: usize) -> Layout {
    let inner = panel.shrink(6.0);
    let tabs = Rect::from_min_size(inner.min, Vec2::new(inner.width(), theme::space::CONTROL_H));
    let options = Rect::from_min_size(egui::pos2(inner.left(), tabs.bottom() + 3.0), Vec2::new(inner.width(), if kind <= 1 || kind == 3 { 16.0 } else { 0.0 }));
    let axis_h = if kind == 3 { 13.0 } else { 0.0 };
    // As many readings as leave a plot of at least 60 points.
    let room = (inner.bottom() - options.bottom() - axis_h - 60.0 - 8.0).max(0.0);
    let shown = lines.min(4).min((room / LINE) as usize);
    let footer_h = if shown == 0 { 0.0 } else { shown as f32 * LINE + 4.0 };
    let footer = Rect::from_min_max(egui::pos2(inner.left() + 2.0, inner.bottom() - footer_h), inner.max);
    let top = options.bottom() + 6.0;
    let gutter_w = if kind <= 1 { GUTTER } else { 0.0 };
    let bottom = (footer.top() - 10.0 - axis_h).max(top + 20.0);
    let plot = Rect::from_min_max(egui::pos2(inner.left() + gutter_w, top), egui::pos2(inner.right() - 2.0, bottom));
    let gutter = Rect::from_min_max(egui::pos2(inner.left(), top - 6.0), egui::pos2(plot.left() - 3.0, bottom + 6.0));
    let axis = Rect::from_min_max(egui::pos2(plot.left(), plot.bottom() + 1.0), egui::pos2(plot.right(), plot.bottom() + axis_h + 1.0));
    Layout { tabs, options, gutter, plot, axis, footer }
}

/// A small text button for the options row.
fn chip(ui: &mut egui::Ui, at: Pos2, text: &str, on: bool) -> (Rect, bool) {
    let t = theme::tokens(ui.ctx());
    let font = theme::caption();
    let galley = ui.painter().layout_no_wrap(crate::i18n::shown(text).to_string(), font, Color32::WHITE);
    let r = Rect::from_min_size(at, galley.size() + Vec2::new(10.0, 4.0));
    let resp = ui.interact(r, egui::Id::unique("scopes-chip").with(text), Sense::click());
    let (fill, col) = if on { (Color32::from_white_alpha(34), Color32::WHITE) } else if resp.hovered() { (Color32::from_white_alpha(14), t.text_2) } else { (Color32::TRANSPARENT, t.text_2) };
    ui.painter().rect_filled(r, 4, fill);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::i18n::shown(text), theme::caption(), col);
    (r, resp.clicked())
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

fn tab_id() -> egui::Id {
    egui::Id::unique("scopes-tab")
}

/// Whether the docked panel shows the Grade tab instead of the scopes.
pub fn grade_tab(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| *d.get_persisted_mut_or_default::<usize>(tab_id())) == 1
}

/// Switches the docked panel between the scopes and the Grade tab.
pub fn set_grade_tab(ctx: &egui::Context, grade: bool) {
    ctx.data_mut(|d| d.insert_persisted(tab_id(), grade as usize));
}

/// The scopes docked as a panel in `r`, with the Grade panel as a tab
/// beside them.
pub fn docked(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    #[cfg(feature = "render")]
    if let Some(rv) = app.render.as_mut() {
        rv.keep_pixels = true;
    }
    let inner = r.shrink(6.0);
    let mut tab = grade_tab(ui.ctx()) as usize;
    let strip = Rect::from_min_size(inner.min, Vec2::new(inner.width(), theme::space::CONTROL_H));
    let mut head = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::top_down(egui::Align::Min)));
    let segs = [Segment::text("Scopes"), Segment::text("Grade")];
    if kit::segmented(&mut head, "scopes-grade-tab", &mut tab, &segs, Some(strip.width()), None).changed() {
        set_grade_tab(ui.ctx(), tab == 1);
    }
    let body = Rect::from_min_max(egui::pos2(inner.left(), strip.bottom() + 4.0), inner.max);
    if tab == 1 {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        super::grade::panel(app, &mut child);
    } else {
        draw(app, ui, body);
    }
}

/// The scope chooser, the plot and the readings in `panel`.
fn draw(app: &mut StudioApp, ui: &mut egui::Ui, panel: Rect) {
    let t = theme::tokens(ui.ctx());
    let mut view = ui.ctx().data_mut(|d| *d.get_persisted_mut_or_default::<(usize, bool, bool)>(view_id()));
    view.0 = view.0.min(3);
    let mut view = View { kind: view.0, ten_bit: view.1, rgb: view.2 };
    ui.painter().with_clip_rect(panel).rect_filled(panel, 8, Color32::from_black_alpha(215));

    #[cfg(feature = "render")]
    let frame = app.render.as_ref().and_then(|rv| rv.last_pixels.as_ref().map(|(w, h, px, stamp)| (*w, *h, px.clone(), *stamp)));
    #[cfg(not(feature = "render"))]
    let frame: Option<(u32, u32, std::sync::Arc<Vec<u8>>, u64)> = None;

    let cache_id = egui::Id::unique("scopes-cache");
    let mut cache = ui.ctx().data(|d| d.get_temp::<Cache>(cache_id));
    if let Some((w, h, px, stamp)) = frame {
        // While playing, a few updates a second are plenty.
        let now = ui.input(|i| i.time);
        let last_at = ui.ctx().data(|d| d.get_temp::<f64>(egui::Id::unique("scopes-at"))).unwrap_or(0.0);
        let changed = cache.as_ref().is_none_or(|c| c.kind != view.kind || c.trace != view.trace());
        let stale = changed || cache.as_ref().is_none_or(|c| c.stamp != stamp);
        if stale && (!app.editor.playing || now - last_at > 0.2 || changed) {
            let s = settings(256);
            let img = sample(&px, w, h, 256);
            let readings = scopes::readings(&img, &s);
            let (tex, hist) = if view.kind == 3 {
                (None, Some(scopes::histogram(&img, &ScopeSettings { levels: 128, ..s })))
            } else {
                let pic = picture_with(&img, view.kind, &s, view.trace());
                let tex = match cache.as_ref().and_then(|c| c.tex.clone()) {
                    Some(mut t) => {
                        t.set(pic, TextureOptions::LINEAR);
                        t
                    }
                    None => ui.ctx().load_texture("scopes", pic, TextureOptions::LINEAR),
                };
                (Some(tex), None)
            };
            cache = Some(Cache { stamp, kind: view.kind, trace: view.trace(), tex, hist, readings });
            ui.ctx().data_mut(|d| {
                d.insert_temp(cache_id, cache.clone().expect("just made"));
                d.insert_temp(egui::Id::unique("scopes-at"), now);
            });
        } else if stale {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
    } else {
        cache = None;
    }
    let shown = cache.as_ref().filter(|c| c.kind == view.kind).map(|c| Shown { tex: c.tex.clone(), hist: c.hist.clone(), readings: c.readings.clone() });
    let lines = shown.as_ref().map_or(0, |s| words(&s.readings).len());
    let lay = layout(panel, view.kind, lines);
    let p = ui.painter().with_clip_rect(panel);

    let segs: Vec<Segment> = KINDS.iter().map(|k| Segment::text(k)).collect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lay.tabs).layout(egui::Layout::top_down(egui::Align::Min)));
    let mut changed = kit::segmented(&mut child, "scope-kind", &mut view.kind, &segs, Some(lay.tabs.width()), None).changed();
    if lay.options.height() > 0.0 {
        let mut at = lay.options.min;
        if view.kind <= 1 {
            for (label, on) in [("%", !view.ten_bit), ("10-bit", view.ten_bit)] {
                let (r, clicked) = chip(ui, at, label, on);
                at.x = r.right() + 2.0;
                if clicked && !on {
                    view.ten_bit = !view.ten_bit;
                    changed = true;
                }
            }
        }
        if view.kind == 0 {
            at.x += 8.0;
            for (label, on) in [("Luma", !view.rgb), ("RGB", view.rgb)] {
                let (r, clicked) = chip(ui, at, label, on);
                at.x = r.right() + 2.0;
                if clicked && !on {
                    view.rgb = !view.rgb;
                    changed = true;
                }
            }
        }
    }
    if changed {
        ui.ctx().data_mut(|d| d.insert_persisted(view_id(), (view.kind, view.ten_bit, view.rgb)));
        ui.ctx().request_repaint();
    }
    match shown {
        Some(sh) => paint(&p, &t, &lay, &view, &sh),
        None if cache.is_none() => {
            p.text(lay.plot.center(), Align2::CENTER_CENTER, crate::i18n::shown("Scopes read the rendered preview"), theme::caption(), t.text_2);
        }
        None => {}
    }
}

/// What a scope draws: the trace picture (or the histogram) and the readings.
struct Shown {
    tex: Option<TextureHandle>,
    hist: Option<scopes::Histogram>,
    readings: Readings,
}

/// Paints the graticule, the scale, the trace and the readings.
fn paint(p: &egui::Painter, t: &theme::Tokens, lay: &Layout, view: &View, c: &Shown) {
    let plot = lay.plot;
    let grid = Stroke::new(1.0, Color32::from_white_alpha(34));
    let faint = Stroke::new(1.0, Color32::from_white_alpha(16));
    let label = Color32::from_white_alpha(120);
    p.rect_filled(plot, 2, Color32::from_black_alpha(150));
    let full = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    match view.kind {
        0 | 1 => {
            // Scale beside the plot, never on it.
            let marks: [(f32, String); 5] = if view.ten_bit {
                [0.0, 0.25, 0.5, 0.75, 1.0].map(|k| (k, format!("{}", scopes::code_10bit(k))))
            } else {
                [0.0, 0.25, 0.5, 0.75, 1.0].map(|k| (k, format!("{}", (k * 100.0) as u32)))
            };
            for (k, text) in marks {
                let y = egui::lerp(plot.bottom()..=plot.top(), k);
                p.hline(plot.x_range(), y, grid);
                p.text(egui::pos2(lay.gutter.right(), y), Align2::RIGHT_CENTER, text, theme::caption(), label);
            }
            // Broadcast range: below it and above it is not legal.
            for v in [scopes::LEGAL_RANGE.0, scopes::LEGAL_RANGE.1] {
                let y = egui::lerp(plot.bottom()..=plot.top(), v);
                let mut x = plot.left();
                while x < plot.right() {
                    p.hline(x..=(x + 3.0).min(plot.right()), y, faint);
                    x += 7.0;
                }
            }
            if view.kind == 1 {
                // The three channel panes of the parade are told apart by a rule.
                for i in 1..3 {
                    let x = plot.left() + plot.width() * i as f32 / 3.0;
                    p.vline(x, plot.y_range(), faint);
                }
            }
            if let Some(tex) = &c.tex {
                p.image(tex.id(), plot, full, Color32::WHITE);
            }
        }
        2 => {
            let side = plot.width().min(plot.height());
            let sq = Rect::from_center_size(plot.center(), Vec2::splat(side));
            p.circle_stroke(sq.center(), side / 2.0, grid);
            p.circle_stroke(sq.center(), side * 0.25, faint);
            p.line_segment([egui::pos2(sq.left(), sq.center().y), egui::pos2(sq.right(), sq.center().y)], faint);
            p.line_segment([egui::pos2(sq.center().x, sq.top()), egui::pos2(sq.center().x, sq.bottom())], faint);
            let at = |cb: f32, cr: f32| {
                let (u, v) = scopes::vectorscope_position(cb, cr);
                egui::pos2(sq.left() + u * sq.width(), sq.top() + v * sq.height())
            };
            // The skin line: faces of every complexion sit along it.
            let skin = Color32::from_rgb(230, 170, 120);
            let (sb, sr) = scopes::skin_line(0.5);
            p.line_segment([sq.center(), at(sb, sr)], Stroke::new(1.0, skin));
            p.text(at(sb, sr) + Vec2::new(3.0, 4.0), Align2::LEFT_TOP, "Skin", theme::caption(), skin);
            if let Some(tex) = &c.tex {
                p.image(tex.id(), sq, full, Color32::WHITE);
            }
            // 75 % boxes (the usual bars) with names, 100 % boxes small.
            for tg in scopes::vectorscope_targets_at(YCbCrMatrix::Bt709, 1.0) {
                p.rect_stroke(Rect::from_center_size(at(tg.cb, tg.cr), Vec2::splat(5.0)), 0, Stroke::new(1.0, Color32::from_white_alpha(50)), egui::StrokeKind::Middle);
            }
            for tg in scopes::vectorscope_targets(YCbCrMatrix::Bt709) {
                let c0 = at(tg.cb, tg.cr);
                p.rect_stroke(Rect::from_center_size(c0, Vec2::splat(9.0)), 0, Stroke::new(1.0, Color32::from_white_alpha(130)), egui::StrokeKind::Middle);
                let out = (c0 - sq.center()).normalized();
                p.text(c0 + out * 13.0, Align2::CENTER_CENTER, tg.name, theme::caption(), label);
            }
        }
        _ => {
            for k in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let x = egui::lerp(plot.x_range(), k);
                p.vline(x, plot.y_range(), if k == 0.0 || k == 1.0 { grid } else { faint });
            }
            for (k, text) in [(0.0, Align2::LEFT_TOP), (0.5, Align2::CENTER_TOP), (1.0, Align2::RIGHT_TOP)].map(|(k, a)| (k, a)) {
                let txt = if view.ten_bit { format!("{}", scopes::code_10bit(k)) } else { format!("{}", (k * 100.0) as u32) };
                p.text(egui::pos2(egui::lerp(lay.axis.x_range(), k), lay.axis.top() + 1.0), text, txt, theme::caption(), label);
            }
            if let Some(hg) = &c.hist {
                // Scaled to the body of the picture, so a clipped edge does not flatten it.
                let inner = |v: &Vec<u32>| v.iter().skip(1).take(v.len().saturating_sub(2)).copied().max().unwrap_or(0);
                let most = inner(&hg.red).max(inner(&hg.green)).max(inner(&hg.blue)).max(1) as f32;
                for (bins, col) in [(&hg.red, Color32::from_rgb(240, 90, 80)), (&hg.green, Color32::from_rgb(90, 220, 110)), (&hg.blue, Color32::from_rgb(100, 140, 250)), (&hg.luma, Color32::from_white_alpha(210))] {
                    let n = bins.len().max(2);
                    let pts: Vec<Pos2> = bins.iter().enumerate().map(|(i, v)| egui::pos2(egui::lerp(plot.x_range(), i as f32 / (n - 1) as f32), plot.bottom() - plot.height() * (*v as f32 / most).sqrt().min(1.0))).collect();
                    if col != Color32::from_white_alpha(210) {
                        let w = plot.width() / n as f32;
                        for pt in &pts {
                            p.rect_filled(Rect::from_min_max(egui::pos2(pt.x - w * 0.5, pt.y), egui::pos2(pt.x + w * 0.5, plot.bottom())).intersect(plot), 0, col.gamma_multiply(0.18));
                        }
                    }
                    p.add(egui::Shape::line(pts, Stroke::new(1.2, col)));
                }
                // Clip warnings at the edges: a bar where the picture runs out of range.
                let (low, high) = hg.edge_shares();
                if low > 0.005 {
                    p.rect_filled(Rect::from_min_size(plot.left_top(), Vec2::new(3.0, plot.height())), 0, t.warn);
                }
                if high > 0.005 {
                    p.rect_filled(Rect::from_min_max(egui::pos2(plot.right() - 3.0, plot.top()), plot.right_bottom()), 0, t.warn);
                }
            }
        }
    }
    p.rect_stroke(plot, 2, Stroke::new(1.0, Color32::from_white_alpha(26)), egui::StrokeKind::Inside);
    let lines = words(&c.readings);
    let rows = (lay.footer.height() / LINE) as usize;
    for (i, l) in lines.iter().take(rows).enumerate() {
        let col = if is_warning(l) { t.warn } else { Color32::from_white_alpha(190) };
        p.text(lay.footer.left_top() + Vec2::new(0.0, i as f32 * LINE), Align2::LEFT_TOP, l, theme::caption(), col);
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

    fn frame(f: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let (w, h) = (320u32, 180u32);
        let mut px = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&f(x, y));
            }
        }
        sample(&px, w, h, 256)
    }

    fn lit(pic: &ColorImage) -> usize {
        pic.pixels.iter().filter(|p| p.a() > 0).count()
    }

    #[test]
    fn a_normal_frame_gives_a_trace_you_can_read() {
        // A ramp with some texture: every column has a trace.
        let img = frame(|x, y| {
            let v = (x * 255 / 319) as i32 + ((y as i32 % 7) - 3) * 3;
            [v.clamp(0, 255) as u8; 3]
        });
        let pic = picture(&img, 0, &settings(256));
        let strong = pic.pixels.iter().filter(|p| p.r().max(p.g()).max(p.b()) > 100).count();
        assert!(strong > 256, "a visible trace in most columns, got {strong}");
        // Even a single outlier pixel shows.
        let one = frame(|x, y| if (x, y) == (160, 90) { [255; 3] } else { [20; 3] });
        let pic = picture(&one, 0, &settings(256));
        assert!(pic.pixels.iter().any(|p| p.a() > 0 && p.r() > 30 && p.y_top_half(&pic)), "outlier visible");
    }

    trait TopHalf {
        fn y_top_half(&self, pic: &ColorImage) -> bool;
    }
    impl TopHalf for Color32 {
        fn y_top_half(&self, pic: &ColorImage) -> bool {
            // The outlier is the only bright thing in the top half.
            let top = &pic.pixels[..pic.pixels.len() / 2];
            top.iter().any(|p| p == self)
        }
    }

    #[test]
    fn known_frames_land_on_the_scale() {
        let s = settings(256);
        // Black: only the bottom rows. White: only the top rows.
        let black = picture(&frame(|_, _| [0; 3]), 0, &s);
        let white = picture(&frame(|_, _| [255; 3]), 0, &s);
        let row_lit = |pic: &ColorImage, y: usize| (0..pic.size[0]).any(|x| pic[(x, y)].a() > 0);
        assert!(row_lit(&black, 127) && !row_lit(&black, 0) && !row_lit(&black, 64));
        assert!(row_lit(&white, 0) && !row_lit(&white, 127) && !row_lit(&white, 64));
        // The parade tints each pane: pure red lights only the first third in red.
        let red = picture(&frame(|_, _| [255, 0, 0]), 1, &s);
        let pane = red.size[0] / 3;
        let at = |x: usize| red[(x, 0)];
        assert!(at(pane / 2).r() > 150 && at(pane / 2).g() < 100, "{:?}", at(pane / 2));
        assert!(row_lit(&red, 127), "green and blue sit at zero");
        assert!(red[(pane * 2, 0)].a() == 0 || red[(pane * 2 + 10, 0)].a() == 0);
        // RGB overlay draws the three on the same width.
        let over = picture_with(&frame(|_, _| [255, 0, 0]), 0, &s, Trace::Rgb);
        assert_eq!(over.size, [256, 128]);
        assert!(over[(10, 0)].r() > 150 && over[(10, 127)].g() > 100, "red on top, green and blue at the bottom");
        // Bars: the vectorscope lights the 75 % yellow target's cell.
        let bars = frame(|x, _| [[191, 191, 191], [191, 191, 0], [0, 191, 191], [0, 191, 0], [191, 0, 191], [191, 0, 0], [0, 0, 191]][(x * 7 / 320) as usize]);
        let v = picture(&bars, 2, &s);
        let yl = scopes::vectorscope_targets(YCbCrMatrix::Bt709).into_iter().find(|t| t.name == "Yl").unwrap();
        let (u, w) = scopes::vectorscope_position(yl.cb, yl.cr);
        let (c, r) = ((u * 159.0).round() as usize, (w * 159.0).round() as usize);
        assert!((r - 1..=r + 1).any(|r| (c - 1..=c + 1).any(|c| v[(c, r)].a() > 0)), "yellow bar on its target");
        assert!(lit(&v) < 160 * 160 / 10, "a sharp scope, not a smear");
    }

    #[test]
    fn the_words_say_what_matters_and_stay_quiet_otherwise() {
        let img = frame(|_, _| [0; 3]);
        let r = scopes::readings(&img, &settings(256));
        let w = words(&r);
        assert_eq!(w, vec!["Black frame".to_owned(), "100% crushed to black".to_owned()], "{w:?}");
        assert!(!w.iter().any(|l| l.contains("mid")));
        let ramp = frame(|x, _| [(20 + x * 210 / 319) as u8; 3]);
        let r = scopes::readings(&ramp, &settings(256));
        let w = words(&r);
        assert!(w[0].starts_with("Levels ") && w[0].contains('–'), "{w:?}");
        assert_eq!(w.len(), 1, "a clean ramp has nothing to warn about: {w:?}");
        let r = scopes::readings(&frame(|_, _| [255; 3]), &settings(256));
        assert!(words(&r).iter().any(|l| l == "White frame") && words(&r).iter().any(|l| l.contains("blown out")));
    }

    /// One frame of the scopes panel in a window of `size`, as texts and the plot.
    fn lay_out(size: Vec2, kind: usize, ten_bit: bool, img: &Image) -> (Vec<(String, Rect)>, Layout, Vec<u8>) {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::apply_style(&ctx);
        let s = settings(256);
        let tex = if kind == 3 { None } else { Some(ctx.load_texture("scopes-test", picture(img, kind, &s), TextureOptions::LINEAR)) };
        let hist = (kind == 3).then(|| scopes::histogram(img, &ScopeSettings { levels: 128, ..s }));
        let sh = Shown { tex, hist, readings: scopes::readings(img, &s) };
        let view = View { kind, ten_bit, rgb: false };
        let mut lay = None;
        let mut out = None;
        let mut textures = crate::ui_shot::Textures::default();
        for _ in 0..2 {
            let mut o = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() }, |ui| {
                let panel = Rect::from_min_size(Pos2::ZERO, size);
                let l = layout(panel, kind, words(&sh.readings).len());
                let p = ui.painter().with_clip_rect(panel);
                paint(&p, &theme::tokens(ui.ctx()), &l, &view, &sh);
                lay = Some(l);
            });
            textures.apply(&o.textures_delta);
            o.textures_delta.clear();
            out = Some(o);
        }
        let out = out.unwrap();
        let rgba = crate::ui_shot::render(&ctx.tessellate(out.shapes.clone(), 1.0), &textures, [size.x as usize, size.y as usize], 1.0, Color32::BLACK);
        let mut texts = vec![];
        fn walk(s: &egui::epaint::Shape, f: &mut Vec<(String, Rect)>) {
            match s {
                egui::epaint::Shape::Text(t) => f.push((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()))),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, f)),
                _ => {}
            }
        }
        for cs in &out.shapes {
            walk(&cs.shape, &mut texts);
        }
        (texts, lay.unwrap(), rgba)
    }

    #[test]
    fn the_layout_keeps_labels_off_the_plot_and_off_each_other() {
        let _guard = crate::ui_tests::serial();
        let dark = frame(|x, y| [(x / 4) as u8 / 4, (y / 3) as u8 / 4, 5]);
        for size in [Vec2::new(300.0, 236.0), Vec2::new(220.0, 190.0), Vec2::new(520.0, 420.0)] {
            for kind in 0..4 {
                for ten_bit in [false, true] {
                    let (texts, lay, _) = lay_out(size, kind, ten_bit, &dark);
                    let panel = Rect::from_min_size(Pos2::ZERO, size);
                    for (name, r) in [("tabs", lay.tabs), ("plot", lay.plot), ("footer", lay.footer)] {
                        assert!(panel.contains_rect(r) || r.height() <= 0.0, "{name} {r:?} inside {panel:?} ({kind}, {size:?})");
                    }
                    assert!(lay.plot.height() >= 60.0 - 0.5 && lay.plot.width() > 50.0, "{lay:?}");
                    assert!(lay.tabs.bottom() <= lay.options.top() && lay.options.bottom() <= lay.plot.top(), "{lay:?}");
                    assert!(lay.plot.bottom() <= lay.footer.top() || lay.footer.height() == 0.0, "{lay:?}");
                    if kind <= 1 {
                        // Scale labels sit in the gutter, wholly outside the plot.
                        let scale: Vec<_> = texts.iter().filter(|(t, _)| t.chars().all(|c| c.is_ascii_digit())).collect();
                        assert_eq!(scale.len(), 5, "{texts:?}");
                        for (t, r) in scale {
                            assert!(r.right() <= lay.plot.left() && r.left() >= panel.left(), "label {t} {r:?} vs plot {:?}", lay.plot);
                        }
                    }
                    for (i, (ta, ra)) in texts.iter().enumerate() {
                        for (tb, rb) in &texts[i + 1..] {
                            let x = ra.intersect(*rb);
                            assert!(!(x.width() > 0.5 && x.height() > 0.5), "{ta:?} {ra:?} overlaps {tb:?} {rb:?} (kind {kind}, {size:?})");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_waveform_frame_draws_a_bright_trace_and_clip_bars() {
        let _guard = crate::ui_tests::serial();
        let ramp = frame(|x, _| [(x * 255 / 319) as u8; 3]);
        let (_, lay, rgba) = lay_out(Vec2::new(300.0, 236.0), 0, false, &ramp);
        let bright = |x: usize, y: usize| rgba[(y * 300 + x) * 4..][..3].iter().map(|v| *v as u32).sum::<u32>();
        // Along the diagonal of the plot the trace is clearly brighter than the grid.
        let (p, mut best) = (lay.plot, 0);
        for i in 0..40 {
            let f = 0.1 + i as f32 / 50.0;
            let (x, y) = ((p.left() + p.width() * f) as usize, (p.bottom() - p.height() * f) as usize);
            best = best.max((y.saturating_sub(2)..=y + 2).map(|y| bright(x, y)).max().unwrap());
        }
        assert!(best > 300, "trace peak brightness {best}");
        // A clipped frame marks the right edge of the histogram.
        let (_, lay, rgba) = lay_out(Vec2::new(300.0, 236.0), 3, false, &frame(|_, _| [255; 3]));
        let (x, y) = ((lay.plot.right() - 2.0) as usize, lay.plot.center().y as usize);
        let px = &rgba[(y * 300 + x) * 4..][..3];
        assert!(px[0] > 150, "warning bar at the right edge: {px:?}");
    }
}
