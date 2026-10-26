// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text on a frame, with Studio's own text engine (the same fonts and
//! shaping the editor uses): lower-third titles, the teleprompter, burnt-in
//! labels.

use crate::frame::Frame;
use std::sync::OnceLock;
use studio_text::{Align, Canvas, Fill, FontRequest, Hooks, LayoutParams, Rgba, TextBlock, TextEngine, TextStyle};

fn engine() -> &'static TextEngine {
    static E: OnceLock<TextEngine> = OnceLock::new();
    E.get_or_init(TextEngine::discover)
}

/// Starts loading the fonts in the background, so the first title or the
/// prompter does not stall a running show while they are found.
pub fn warm() {
    std::thread::spawn(|| {
        let _ = engine();
    });
}

#[derive(Clone, Debug)]
pub struct TextOpts {
    /// Font size in pixels of the frame.
    pub size: f32,
    pub colour: [u8; 4],
    pub weight: u16,
    pub family: Option<String>,
    pub align: Align,
    /// Wrap width in pixels (none: only at newlines).
    pub wrap: Option<f32>,
    /// A plate drawn behind the text, with this much padding in pixels.
    pub plate: Option<([u8; 4], f32)>,
}

impl Default for TextOpts {
    fn default() -> Self {
        TextOpts { size: 48.0, colour: [255, 255, 255, 255], weight: 600, family: None, align: Align::Centre, wrap: None, plate: None }
    }
}

/// Where a drawn text landed.
#[derive(Clone, Debug, Default)]
pub struct Drawn {
    /// Box of the text: left, top, width, height in frame pixels.
    pub rect: [f32; 4],
    /// Box of each word in frame pixels (left, top, width, height).
    pub words: Vec<[f32; 4]>,
    pub lines: usize,
}

fn style_of(o: &TextOpts, frame_h: u32) -> TextStyle {
    let mut st = TextStyle::new(
        FontRequest { family: o.family.clone().unwrap_or_else(|| "Inter".into()), weight: o.weight, italic: false },
        // The canvas is 1080 units tall whatever the frame is.
        (o.size * 1080.0 / frame_h.max(1) as f32).clamp(5.0, 300.0),
        Fill::Solid(Rgba::WHITE),
    );
    st.opacity = 1.0;
    st
}

fn block_of(text: &str, o: &TextOpts, frame_h: u32) -> TextBlock {
    let unit = 1080.0 / frame_h.max(1) as f32;
    TextBlock::plain(text).with_layout(LayoutParams { align: o.align, wrap_width: o.wrap.map(|w| w * unit), ..LayoutParams::default() })
}

/// Draws `text` with the middle of its box at (`cx`, `cy`).
pub fn draw_text(frame: &mut Frame, text: &str, cx: f32, cy: f32, o: &TextOpts) -> Drawn {
    if text.trim().is_empty() {
        return Drawn::default();
    }
    let canvas = Canvas::new((frame.w as f32 * 1080.0 / frame.h.max(1) as f32).round().max(1.0) as u32, 1080);
    let style = style_of(o, frame.h);
    let block = block_of(text, o, frame.h);
    let layout = engine().layout(&block, &style, &canvas);
    let raster = engine().paint(&layout, &style, &Hooks::NONE);
    // Layout units to frame pixels.
    let k = frame.h as f32 / 1080.0;
    let (bw, bh) = (layout.bounds.w * k, layout.bounds.h * k);
    let (left, top) = (cx - bw * 0.5, cy - bh * 0.5);
    if let Some((c, pad)) = o.plate {
        let ink = &layout.ink;
        let (x0, y0) = (left + ink.x * k - pad, top + ink.y * k - pad);
        frame.fill_rect_blend(x0.round() as i32, y0.round() as i32, (ink.w * k + 2.0 * pad).round() as i32, (ink.h * k + 2.0 * pad).round() as i32, c);
    }
    // The raster is at layout-box scale (1080-unit canvas px); the frame
    // is `k` times that.
    let img = &raster.image;
    let (ox, oy) = (left + raster.left as f32 * k, top + raster.top as f32 * k);
    for y in 0..img.height {
        for x in 0..img.width {
            let a = img.pixels[(y * img.width + x) as usize][3];
            if a <= 0.003 {
                continue;
            }
            let (fx0, fy0) = ((ox + x as f32 * k).round() as i32, (oy + y as f32 * k).round() as i32);
            let (fx1, fy1) = ((ox + (x + 1) as f32 * k).round() as i32, (oy + (y + 1) as f32 * k).round() as i32);
            for fy in fy0..fy1.max(fy0 + 1) {
                for fx in fx0..fx1.max(fx0 + 1) {
                    if fx >= 0 && fy >= 0 {
                        frame.blend_px(fx as u32, fy as u32, o.colour, a.min(1.0) * (o.colour[3] as f32 / 255.0));
                    }
                }
            }
        }
    }
    let words = layout.word_boxes.iter().map(|r| [left + r.x * k, top + r.y * k, r.w * k, r.h * k]).collect();
    Drawn { rect: [left, top, bw, bh], words, lines: layout.lines.len() }
}

/// The lines `text` breaks into when wrapped at `wrap_px` (and the width
/// the longest takes), without drawing.
pub fn wrap_lines(text: &str, o: &TextOpts, frame_h: u32, frame_w: u32) -> Vec<String> {
    let canvas = Canvas::new((frame_w as f32 * 1080.0 / frame_h.max(1) as f32).round().max(1.0) as u32, 1080);
    let style = style_of(o, frame_h);
    let block = block_of(text, o, frame_h);
    let layout = engine().layout(&block, &style, &canvas);
    layout.lines.iter().map(|l| layout.text[l.text.clone()].trim_end_matches('\n').to_owned()).collect()
}

impl Frame {
    /// A plate: `c` blended over a rectangle by its own alpha.
    pub fn fill_rect_blend(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                self.blend_px(xx as u32, yy as u32, c, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lands_where_asked_and_wraps() {
        let mut f = Frame::new(640, 360, [0, 0, 0, 255]);
        let d = draw_text(&mut f, "Hello world", 320.0, 180.0, &TextOpts { size: 40.0, ..Default::default() });
        assert!(d.rect[2] > 100.0, "{d:?}");
        let lit = f.rgba.chunks(4).filter(|p| p[0] > 128).count();
        assert!(lit > 200, "text drew {lit} bright pixels");
        // Centred: the lit pixels straddle the middle.
        let mut min_x = f.w;
        let mut max_x = 0;
        for y in 0..f.h {
            for x in 0..f.w {
                if f.px(x, y)[0] > 128 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        assert!(min_x < 320 && max_x > 320, "{min_x}..{max_x}");
        let o = TextOpts { size: 40.0, wrap: Some(300.0), align: Align::Left, ..Default::default() };
        let lines = wrap_lines("one two three four five six seven eight nine ten eleven twelve", &o, 360, 640);
        assert!(lines.len() >= 2, "{lines:?}");
        assert_eq!(lines.join(" ").split_whitespace().count(), 12);
    }
}

/// How big `text` draws: width and height in pixels.
pub fn measure(text: &str, o: &TextOpts, frame_w: u32, frame_h: u32) -> (f32, f32) {
    if text.trim().is_empty() {
        return (0.0, 0.0);
    }
    let canvas = Canvas::new((frame_w as f32 * 1080.0 / frame_h.max(1) as f32).round().max(1.0) as u32, 1080);
    let l = engine().layout(&block_of(text, o, frame_h), &style_of(o, frame_h), &canvas);
    let k = frame_h as f32 / 1080.0;
    (l.ink.w * k, l.ink.h * k)
}
