// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The painter: a [`Layout`] plus a [`TextStyle`] → a premultiplied,
//! linear-light RGBA layer.
//!
//! Draw order, bottom to top: background box → extrusion → shadow → glow →
//! strokes (outermost first) → face (fill, colour glyphs, underlines) →
//! block opacity. Every stage works on coverage planes the size of the
//! layer, so per-character animation (offset/scale/rotation/opacity/colour
//! through [`Hooks`]) affects every stage consistently.

use std::collections::HashMap;
use std::sync::Arc;

use crate::emoji::EmojiSet;
use crate::geom::{Affine, Rect};
use crate::layout::Layout;
use crate::raster::{self, ColourGlyph, CoverageMask, GlyphPath, Plane, Rasteriser};
use crate::rgba::Rgba;
use crate::style::{Fill, GradientScope, TextStyle, TextUnit};
use crate::Image;

/// How one animation unit (character, word or line) is displaced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitPose {
    /// Pixels.
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    /// Degrees, clockwise.
    pub rotation: f32,
    pub opacity: f32,
    /// Replaces the fill colour for this unit.
    pub colour: Option<Rgba>,
}

impl Default for UnitPose {
    fn default() -> Self {
        UnitPose { offset: [0.0, 0.0], scale: [1.0, 1.0], rotation: 0.0, opacity: 1.0, colour: None }
    }
}

impl UnitPose {
    pub const REST: UnitPose = UnitPose { offset: [0.0, 0.0], scale: [1.0, 1.0], rotation: 0.0, opacity: 1.0, colour: None };

    pub fn hidden() -> UnitPose {
        UnitPose { opacity: 0.0, ..UnitPose::REST }
    }

    pub fn is_rest(&self) -> bool {
        *self == UnitPose::REST
    }

    pub fn offset(mut self, x: f32, y: f32) -> UnitPose {
        self.offset = [x, y];
        self
    }

    pub fn scale(mut self, s: f32) -> UnitPose {
        self.scale = [s, s];
        self
    }

    pub fn opacity(mut self, o: f32) -> UnitPose {
        self.opacity = o;
        self
    }

    pub fn rotation(mut self, r: f32) -> UnitPose {
        self.rotation = r;
        self
    }

    pub fn colour(mut self, c: Rgba) -> UnitPose {
        self.colour = Some(c);
        self
    }

    /// Blend between two poses.
    pub fn lerp(&self, o: &UnitPose, t: f32) -> UnitPose {
        let l = |a: f32, b: f32| a + (b - a) * t;
        UnitPose {
            offset: [l(self.offset[0], o.offset[0]), l(self.offset[1], o.offset[1])],
            scale: [l(self.scale[0], o.scale[0]), l(self.scale[1], o.scale[1])],
            rotation: l(self.rotation, o.rotation),
            opacity: l(self.opacity, o.opacity),
            colour: match (self.colour, o.colour) {
                (Some(a), Some(b)) => Some(a.lerp(b, t)),
                (None, Some(b)) => Some(b),
                (a, None) => a,
            },
        }
    }
}

/// Per-unit pose callbacks: `(index, count)` → pose. `fx` and the
/// template player drive these.
#[derive(Default, Clone, Copy)]
pub struct Hooks<'a> {
    pub chars: Option<&'a dyn Fn(usize, usize) -> UnitPose>,
    pub words: Option<&'a dyn Fn(usize, usize) -> UnitPose>,
    pub lines: Option<&'a dyn Fn(usize, usize) -> UnitPose>,
}

impl<'a> Hooks<'a> {
    pub const NONE: Hooks<'static> = Hooks { chars: None, words: None, lines: None };

    pub fn for_unit(unit: TextUnit, f: &'a dyn Fn(usize, usize) -> UnitPose) -> Hooks<'a> {
        match unit {
            TextUnit::Character => Hooks { chars: Some(f), ..Default::default() },
            TextUnit::Word => Hooks { words: Some(f), ..Default::default() },
            TextUnit::Line => Hooks { lines: Some(f), ..Default::default() },
        }
    }

    pub fn is_none(&self) -> bool {
        self.chars.is_none() && self.words.is_none() && self.lines.is_none()
    }
}

/// A painted text layer and where it sits relative to the layout box.
#[derive(Clone, Debug)]
pub struct TextRaster {
    pub image: Image,
    /// Offset of the image's top-left from the layout box's top-left.
    pub left: i32,
    pub top: i32,
}

impl TextRaster {
    pub fn empty() -> TextRaster {
        TextRaster { image: Image::new(0, 0), left: 0, top: 0 }
    }

    /// Composites onto `canvas` with the layout box's top-left at (x, y).
    pub fn blit(&self, canvas: &mut Image, x: f32, y: f32) {
        if self.image.width == 0 {
            return;
        }
        raster::blit_over(canvas, &self.image, x.round() as i32 + self.left, y.round() as i32 + self.top);
    }
}

struct GlyphDraw {
    path: Option<Arc<GlyphPath>>,
    colour: Option<ColourGlyph>,
    transform: Affine,
    opacity: f32,
    colour_override: Option<Rgba>,
    glyph: usize,
}

/// Paints `layout` in `style`. `images` resolves [`Fill::Image`] keys.
pub fn paint(r: &mut Rasteriser, layout: &Layout, style: &TextStyle, hooks: &Hooks, images: &HashMap<String, Image>, emoji: Option<&EmojiSet>) -> TextRaster {
    let ppu = layout.px_per_unit;
    let px = |u: f32| u * ppu;
    let line_boxes = layout.line_boxes();

    // Unit poses, evaluated once per unit.
    let char_poses: Vec<UnitPose> = (0..layout.char_count).map(|i| hooks.chars.map(|f| f(i, layout.char_count)).unwrap_or(UnitPose::REST)).collect();
    let word_poses: Vec<UnitPose> = (0..layout.word_count).map(|i| hooks.words.map(|f| f(i, layout.word_count)).unwrap_or(UnitPose::REST)).collect();
    let line_poses: Vec<UnitPose> = (0..layout.lines.len()).map(|i| hooks.lines.map(|f| f(i, layout.lines.len())).unwrap_or(UnitPose::REST)).collect();
    let pose_affine = |p: &UnitPose, b: &Rect| -> Affine {
        if p.is_rest() { Affine::IDENTITY } else { Affine::about(b.centre(), p.scale, p.rotation, p.offset) }
    };
    // Combined transform / opacity / colour for a (char, word, line) triple.
    let unit_chain = |ci: usize, wi: usize, li: usize| -> (Affine, f32, Option<Rgba>) {
        let mut t = Affine::IDENTITY;
        let mut op = 1.0;
        let mut col = None;
        if li < line_poses.len() {
            let p = &line_poses[li];
            t = t.then(&pose_affine(p, &line_boxes[li]));
            op *= p.opacity;
            col = p.colour.or(col);
        }
        if wi != usize::MAX && wi < word_poses.len() {
            let p = &word_poses[wi];
            // Word first, then line: the line transform wraps the word's.
            t = pose_affine(p, &layout.word_boxes[wi]).then(&t);
            op *= p.opacity;
            col = p.colour.or(col);
        }
        if ci != usize::MAX && ci < char_poses.len() {
            let p = &char_poses[ci];
            t = pose_affine(p, &layout.char_boxes[ci]).then(&t);
            op *= p.opacity;
            col = p.colour.or(col);
        }
        (t, op, col)
    };

    // 1. Glyph transforms and bounds.
    let mut draws: Vec<GlyphDraw> = Vec::with_capacity(layout.glyphs.len());
    let mut bounds = Rect::EMPTY;
    for (gi, g) in layout.glyphs.iter().enumerate() {
        if g.is_space {
            continue;
        }
        let (unit_t, opacity, colour_override) = unit_chain(g.char_index, g.word_index, g.line_index);
        if opacity <= 0.0 {
            continue;
        }
        let base = if g.rotation.abs() > 1e-4 { Affine::rotate(g.rotation).then(&Affine::translate(g.x, g.y)) } else { Affine::translate(g.x, g.y) };
        let transform = base.then(&unit_t);
        let m = &layout.fonts[g.font];
        if g.emoji {
            let cluster_end = layout.glyphs.iter().filter(|o| o.cluster > g.cluster).map(|o| o.cluster).min().unwrap_or(layout.text.len());
            let key = &layout.text[g.cluster..cluster_end.max(g.cluster)];
            if let Some(im) = emoji.and_then(|set| set.image(key.trim())) {
                // The image fills an em-ish square sitting on the baseline
                // with a little descent, like most emoji fonts' bitmaps.
                let side = g.size_px * crate::layout::EMOJI_ADVANCE * 1.12;
                let scale = side / im.width.max(1) as f32;
                let cell = g.size_px * crate::layout::EMOJI_ADVANCE;
                let t = Affine::scale(scale, scale)
                    .then(&Affine::translate((cell - side) * 0.5, -side * 0.78))
                    .then(&transform);
                let b = t.bounds(&Rect::new(0.0, 0.0, im.width as f32, im.height as f32));
                bounds = bounds.union(&b);
                draws.push(GlyphDraw {
                    path: None,
                    colour: Some(ColourGlyph { left: 0, top: 0, width: im.width, height: im.height, rgba: im.rgba.clone() }),
                    transform: t,
                    opacity,
                    colour_override,
                    glyph: gi,
                });
                continue;
            }
        }
        if g.colour {
            if let Some(cg) = r.colour_glyph(m, g.glyph, g.size_px) {
                let t = Affine::translate(cg.left as f32, cg.top as f32).then(&transform);
                let b = t.bounds(&Rect::new(0.0, 0.0, cg.width as f32, cg.height as f32));
                bounds = bounds.union(&b);
                draws.push(GlyphDraw { path: None, colour: Some(cg), transform: t, opacity, colour_override, glyph: gi });
                continue;
            }
        }
        let path = r.path(m, g.glyph, g.size_px);
        if path.is_empty() {
            continue;
        }
        let b = transform.bounds(&path.bounds);
        bounds = bounds.union(&b);
        draws.push(GlyphDraw { path: Some(path), colour: None, transform, opacity, colour_override, glyph: gi });
    }
    // Decorations follow their word's and line's pose.
    let mut deco_paths: Vec<(GlyphPath, usize, Option<Rgba>, f32)> = Vec::new();
    for d in &layout.decorations {
        let (t, op, col) = unit_chain(usize::MAX, d.word_index, d.line_index);
        if op <= 0.0 {
            continue;
        }
        let p = raster::rect_path(&d.rect).transformed(&t);
        bounds = bounds.union(&p.bounds);
        deco_paths.push((p, d.span, col, op));
    }
    // Background boxes.
    let mut bg_rects: Vec<Rect> = Vec::new();
    if let Some(bg) = &style.background {
        let sources: Vec<Rect> = if bg.per_line {
            layout
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !l.glyphs.is_empty() && l.width > 0.0)
                .map(|(i, l)| {
                    let r = l.rect();
                    let t = pose_affine(&line_poses[i], &r);
                    t.bounds(&r)
                })
                .collect()
        } else if layout.ink.w > 0.0 {
            // One box around the typographic extent of all lines.
            let mut r = Rect::EMPTY;
            for (i, l) in layout.lines.iter().enumerate() {
                if l.glyphs.is_empty() {
                    continue;
                }
                let lr = l.rect();
                let t = pose_affine(&line_poses[i], &lr);
                r = r.union(&t.bounds(&lr));
            }
            if r.x.is_finite() { vec![r] } else { vec![] }
        } else {
            vec![]
        };
        for s in sources {
            let extra_w = layout.size_px * bg.width;
            let extra_h = layout.size_px * bg.height;
            let r = Rect::new(
                s.x - px(bg.padding_x) - extra_w * 0.5 + px(bg.offset[0]),
                s.y - px(bg.padding_y) - extra_h * 0.5 + px(bg.offset[1]),
                s.w + 2.0 * px(bg.padding_x) + extra_w,
                s.h + 2.0 * px(bg.padding_y) + extra_h,
            );
            bounds = bounds.union(&r);
            bg_rects.push(r);
        }
    }
    if !bounds.x.is_finite() || bounds.is_empty() {
        return TextRaster::empty();
    }

    // 2. Layer.
    let reach = px(style.reach()) + 2.0;
    let lb = bounds.inflate(reach).round_out();
    let (w, h) = (lb.w as u32, lb.h as u32);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return TextRaster::empty();
    }
    let (ox, oy) = (-lb.x, -lb.y);
    let to_layer = Affine::translate(ox, oy);
    let mut img = Image::new(w, h);

    // 3. Coverage planes.
    // Fill coverage per glyph (kept for colouring) and the silhouette.
    let mut fill_masks: Vec<(Arc<CoverageMask>, i32, i32, usize)> = Vec::new(); // (mask, ox, oy, draw index)
    let mut silhouette = Plane::new(w, h);
    let mut outline_paths: Vec<GlyphPath> = Vec::new(); // transformed, layer space, with per-draw opacity folded in later
    let mut outline_opacity: Vec<f32> = Vec::new();
    for (di, d) in draws.iter().enumerate() {
        if let Some(p) = &d.path {
            let t = d.transform.then(&to_layer);
            let (mask, mx, my) = if t.is_translation() {
                r.mask_at(&layout.fonts[layout.glyphs[d.glyph].font], layout.glyphs[d.glyph].glyph, layout.glyphs[d.glyph].size_px, t.e, t.f, 0.0)
            } else {
                (Arc::new(raster::rasterise_path(&p.transformed(&t), 0.0)), 0, 0)
            };
            silhouette.accumulate(&mask, mx, my, d.opacity);
            fill_masks.push((mask, mx, my, di));
            outline_paths.push(p.transformed(&t));
            outline_opacity.push(d.opacity);
        }
    }
    // Colour glyph alpha joins the silhouette (for shadows and glows).
    let mut colour_alpha = Plane::new(w, h);
    for d in &draws {
        if let Some(cg) = &d.colour {
            let t = d.transform.then(&to_layer);
            let mut scratch = Image::new(w, h);
            raster::draw_rgba8_affine(&mut scratch, &cg.rgba, cg.width, cg.height, &t, d.opacity, None, Some(&mut colour_alpha));
        }
    }
    if !colour_alpha.is_blank() {
        silhouette = silhouette.union(&colour_alpha);
    }
    for (p, _, _, op) in &deco_paths {
        let m = raster::rasterise_path(&p.transformed(&to_layer), 0.0);
        silhouette.accumulate(&m, 0, 0, *op);
    }

    // Stroke rings: cumulative widths, outermost drawn first.
    let mut rings: Vec<Plane> = Vec::new();
    let mut cum = 0.0;
    let mut inner = silhouette.clone();
    for s in &style.strokes {
        cum += px(s.width).max(0.0);
        let mut plane = Plane::new(w, h);
        if cum > 0.0 {
            for (p, op) in outline_paths.iter().zip(&outline_opacity) {
                let m = raster::rasterise_path(p, cum * 2.0);
                plane.accumulate(&m, 0, 0, *op);
            }
            for (p, _, _, op) in &deco_paths {
                let m = raster::rasterise_path(&p.transformed(&to_layer), cum * 2.0);
                plane.accumulate(&m, 0, 0, *op);
            }
            // Fatten colour glyphs by blurring their alpha and thresholding
            // (bitmaps have no outline to stroke).
            if !colour_alpha.is_blank() {
                let fat = colour_alpha.blurred(cum * 0.6).scaled(2.5);
                plane = plane.union(&fat);
            }
        }
        let ring = plane.subtract(&inner);
        inner = inner.union(&plane);
        rings.push(ring);
    }
    let outer_shape: Plane = inner; // silhouette ∪ all strokes
    let shadow_source = if matches!(style.fill, Fill::None) {
        rings.iter().fold(Plane::new(w, h), |acc, r| acc.union(r))
    } else {
        outer_shape.clone()
    };

    // 4. Composite.
    if let Some(bg) = &style.background {
        let c = bg.colour.linear_premul();
        for r0 in &bg_rects {
            let p = raster::rounded_rect_path(&r0.translate(ox, oy), px(bg.radius));
            let m = raster::rasterise_path(&p, 0.0);
            let mut plane = Plane::new(w, h);
            plane.accumulate(&m, 0, 0, 1.0);
            raster::paint_plane(&mut img, &plane, c);
        }
    }
    if let Some(e) = &style.extrusion {
        let depth = px(e.depth);
        let steps = depth.ceil().max(1.0) as usize;
        let (dx, dy) = (e.angle.to_radians().cos(), e.angle.to_radians().sin());
        for i in (1..=steps).rev() {
            let d = depth * i as f32 / steps as f32;
            let t = i as f32 / steps as f32;
            let shade = if e.shade { 1.0 - 0.45 * t } else { 1.0 };
            let c = Rgba::new(e.colour.r * shade, e.colour.g * shade, e.colour.b * shade, e.colour.a).linear_premul();
            let shifted = shadow_source.shifted(dx * d, dy * d);
            raster::paint_plane(&mut img, &shifted, c);
        }
    }
    if let Some(s) = &style.shadow {
        let (dx, dy) = (s.angle.to_radians().cos() * px(s.distance), s.angle.to_radians().sin() * px(s.distance));
        let plane = shadow_source.blurred(px(s.blur) * 0.5).shifted(dx, dy);
        raster::paint_plane(&mut img, &plane, s.colour.fade(s.opacity).linear_premul());
    }
    if let Some(g) = &style.glow {
        let sigma = px(g.range) * 0.5;
        let halo = shadow_source.blurred(sigma).shifted(px(g.offset[0]), px(g.offset[1]));
        let k = 1.0 + 3.0 * g.intensity;
        let halo = halo.scaled(k);
        let c = g.colour.fade(g.intensity.clamp(0.0, 1.0)).linear_premul();
        raster::paint_plane(&mut img, &halo, c);
    }
    for (ring, s) in rings.iter().zip(&style.strokes).rev() {
        raster::paint_plane(&mut img, ring, s.colour.fade(s.opacity).linear_premul());
    }

    // Face.
    let ink_layer = layout.ink.translate(ox, oy);
    let face_colour = |d: &GlyphDraw, mask_rect: Rect| -> FaceColour {
        let g = &layout.glyphs[d.glyph];
        if let Some(c) = d.colour_override {
            // An override with alpha below one mixes the face towards it
            // (animator picks, shimmers) instead of fading the letter.
            if c.a < 0.999 {
                let base = layout.spans.get(g.span).and_then(|s| s.fill.as_ref()).unwrap_or(&style.fill).representative();
                let mixed = base.lerp(Rgba::new(c.r, c.g, c.b, base.a), c.a.clamp(0.0, 1.0));
                return FaceColour::Solid(mixed.linear_premul());
            }
            return FaceColour::Solid(c.linear_premul());
        }
        if let Some(cycle) = &style.colour_cycle {
            let idx = match cycle.unit {
                TextUnit::Character => g.char_index,
                TextUnit::Word => g.word_index,
                TextUnit::Line => g.line_index,
            };
            if idx != usize::MAX {
                return FaceColour::Solid(cycle.colour(idx).linear_premul());
            }
        }
        let fill = layout.spans.get(g.span).and_then(|s| s.fill.as_ref()).unwrap_or(&style.fill);
        match fill {
            Fill::Solid(c) => FaceColour::Solid(c.linear_premul()),
            Fill::None => FaceColour::None,
            Fill::Gradient(gr) => {
                let rect = match gr.scope {
                    GradientScope::Text => ink_layer,
                    GradientScope::Line => line_boxes[g.line_index].translate(ox, oy),
                    GradientScope::Glyph => mask_rect,
                };
                FaceColour::Gradient(gr.clone(), rect)
            }
            Fill::Image { key, opacity } => match images.get(key) {
                Some(im) => FaceColour::Image(im, ink_layer, *opacity),
                None => FaceColour::Solid(Rgba::WHITE.linear_premul()),
            },
        }
    };
    for (mask, mx, my, di) in &fill_masks {
        let d = &draws[*di];
        let mask_rect = Rect::new((mx + mask.left) as f32, (my + mask.top) as f32, mask.width as f32, mask.height as f32);
        match face_colour(d, mask_rect) {
            FaceColour::None => {}
            FaceColour::Solid(c) => raster::paint_mask_fn(&mut img, mask, *mx, *my, d.opacity, |_, _| c),
            FaceColour::Gradient(gr, rect) => {
                raster::paint_mask_fn(&mut img, mask, *mx, *my, d.opacity, |x, y| gr.at(gradient_t(&rect, gr.angle, x as f32 + 0.5, y as f32 + 0.5)).linear_premul())
            }
            FaceColour::Image(im, rect, op) => raster::paint_mask_fn(&mut img, mask, *mx, *my, d.opacity * op, |x, y| sample_cover(im, &rect, x as f32 + 0.5, y as f32 + 0.5)),
        }
    }
    for (p, span, col, op) in &deco_paths {
        let m = raster::rasterise_path(&p.transformed(&to_layer), 0.0);
        let fill = col.map(Fill::Solid).or_else(|| layout.spans.get(*span).and_then(|s| s.fill.clone())).unwrap_or_else(|| style.fill.clone());
        let c = match &fill {
            Fill::Solid(c) => *c,
            other => other.representative(),
        };
        raster::paint_mask_fn(&mut img, &m, 0, 0, *op, |_, _| c.linear_premul());
    }
    for d in &draws {
        if let Some(cg) = &d.colour {
            let t = d.transform.then(&to_layer);
            let is_emoji = layout.glyphs[d.glyph].emoji;
            let tint = if is_emoji { None } else { d.colour_override.map(|c| c.linear_premul()) };
            raster::draw_rgba8_affine(&mut img, &cg.rgba, cg.width, cg.height, &t, d.opacity, tint, None);
        }
    }
    raster::scale_alpha(&mut img, style.opacity.clamp(0.0, 1.0));
    TextRaster { image: img, left: lb.x as i32, top: lb.y as i32 }
}

enum FaceColour<'a> {
    None,
    Solid([f32; 4]),
    Gradient(crate::style::Gradient, Rect),
    Image(&'a Image, Rect, f32),
}

/// 0..1 position of a point along a gradient axis through `rect`.
pub fn gradient_t(rect: &Rect, angle: f32, x: f32, y: f32) -> f32 {
    let (s, c) = angle.to_radians().sin_cos();
    let [cx, cy] = rect.centre();
    let extent = (rect.w * c).abs() + (rect.h * s).abs();
    if extent < 1e-6 {
        return 0.5;
    }
    let d = (x - cx) * c + (y - cy) * s;
    (d / extent + 0.5).clamp(0.0, 1.0)
}

/// Samples `im` scaled to cover `rect` at `(x, y)`.
fn sample_cover(im: &Image, rect: &Rect, x: f32, y: f32) -> [f32; 4] {
    if im.width == 0 || im.height == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return [0.0; 4];
    }
    let scale = (rect.w / im.width as f32).max(rect.h / im.height as f32);
    let (sw, sh) = (im.width as f32 * scale, im.height as f32 * scale);
    let (x0, y0) = (rect.x + (rect.w - sw) * 0.5, rect.y + (rect.h - sh) * 0.5);
    raster::sample(im, (x - x0) / scale, (y - y0) / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_axis() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert!((gradient_t(&r, 0.0, 0.0, 25.0) - 0.0).abs() < 1e-5);
        assert!((gradient_t(&r, 0.0, 100.0, 25.0) - 1.0).abs() < 1e-5);
        assert!((gradient_t(&r, 90.0, 50.0, 0.0) - 0.0).abs() < 1e-5);
        assert!((gradient_t(&r, 90.0, 50.0, 50.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn pose_lerp() {
        let a = UnitPose::REST.offset(0.0, 10.0).opacity(0.0);
        let p = a.lerp(&UnitPose::REST, 0.5);
        assert_eq!(p.offset, [0.0, 5.0]);
        assert_eq!(p.opacity, 0.5);
    }
}
