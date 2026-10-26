// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Glyph rasterising: outlines → coverage masks, colour glyphs → images,
//! plus the small image utilities the painter needs (blur, rounded
//! rectangles, affine image sampling, compositing).
//!
//! Coverage is grayscale anti-aliasing from zeno's analytic rasteriser at
//! the final pixel size, with sub-pixel positioning kept in the path
//! transform (nothing is snapped to the grid). There is no LCD filtering:
//! video frames are not screens.

use std::collections::HashMap;
use std::sync::Arc;

use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Command, Fill, Mask, PathData, Stroke, Style, Vector};

use crate::font::FontMatch;
use crate::geom::{Affine, Rect};
use crate::Image;

/// A coverage mask placed relative to some origin (y down).
#[derive(Clone, Debug, Default)]
pub struct CoverageMask {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Row-major coverage 0..1.
    pub data: Vec<f32>,
}

impl CoverageMask {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.left as f32, self.top as f32, self.width as f32, self.height as f32)
    }
}

/// A colour glyph image (straight-alpha sRGB 8-bit) placed like a mask.
#[derive(Clone, Debug)]
pub struct ColourGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// An outline in pixels, y down, origin at the glyph origin on the
/// baseline (already scaled to the glyph's size).
#[derive(Clone, Debug, Default)]
pub struct GlyphPath {
    pub commands: Vec<Command>,
    pub bounds: Rect,
}

impl GlyphPath {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// The same path through an affine transform.
    pub fn transformed(&self, t: &Affine) -> GlyphPath {
        let map = |p: Vector| {
            let q = t.apply([p.x, p.y]);
            Vector::new(q[0], q[1])
        };
        let commands: Vec<Command> = self
            .commands
            .iter()
            .map(|c| match *c {
                Command::MoveTo(p) => Command::MoveTo(map(p)),
                Command::LineTo(p) => Command::LineTo(map(p)),
                Command::QuadTo(a, p) => Command::QuadTo(map(a), map(p)),
                Command::CurveTo(a, b, p) => Command::CurveTo(map(a), map(b), map(p)),
                Command::Close => Command::Close,
            })
            .collect();
        let bounds = t.bounds(&self.bounds);
        GlyphPath { commands, bounds }
    }
}

/// Rasterises paths and loads colour glyphs; owns the swash scale context
/// and a small mask cache.
pub struct Rasteriser {
    ctx: ScaleContext,
    cache: HashMap<MaskKey, Arc<CoverageMask>>,
    path_cache: HashMap<PathKey, Arc<GlyphPath>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct PathKey {
    face: u32,
    glyph: u16,
    size_q: u32,
    weight_q: i32,
    bold: bool,
    italic: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct MaskKey {
    path: PathKey,
    sub_x: u8,
    sub_y: u8,
    stroke_q: u32,
}

impl Default for Rasteriser {
    fn default() -> Self {
        Self::new()
    }
}

impl Rasteriser {
    pub fn new() -> Rasteriser {
        Rasteriser { ctx: ScaleContext::new(), cache: HashMap::new(), path_cache: HashMap::new() }
    }

    fn path_key(m: &FontMatch, glyph: u16, size_px: f32) -> PathKey {
        PathKey {
            face: m.id().0,
            glyph,
            size_q: (size_px * 64.0).round() as u32,
            weight_q: m.weight_variation.map(|w| w.round() as i32).unwrap_or(-1),
            bold: m.synthetic_bold,
            italic: m.synthetic_italic,
        }
    }

    /// The outline of a glyph at `size_px`, y down, origin on the baseline.
    pub fn path(&mut self, m: &FontMatch, glyph: u16, size_px: f32) -> Arc<GlyphPath> {
        let key = Self::path_key(m, glyph, size_px);
        if let Some(p) = self.path_cache.get(&key) {
            return p.clone();
        }
        if self.path_cache.len() > 4096 {
            self.path_cache.clear();
        }
        let font = m.face.swash();
        let mut builder = self.ctx.builder(font).size(size_px).hint(false);
        if let Some(w) = m.weight_variation {
            builder = builder.variations(&[("wght", w)]);
        }
        let mut scaler = builder.build();
        let mut path = GlyphPath::default();
        if let Some(mut outline) = scaler.scale_outline(glyph) {
            if m.synthetic_bold {
                let s = size_px * 0.03;
                outline.embolden(s, s);
            }
            let skew = if m.synthetic_italic { (-12f32).to_radians().tan() } else { 0.0 };
            // Font space is y up; the painter works y down.
            let t = Affine { a: 1.0, b: 0.0, c: skew, d: -1.0, e: 0.0, f: 0.0 };
            let mut bounds = Rect::EMPTY;
            let map = |p: Vector, bounds: &mut Rect| {
                let q = t.apply([p.x, p.y]);
                *bounds = bounds.include_point(q[0], q[1]);
                Vector::new(q[0], q[1])
            };
            let mut cmds = Vec::new();
            for c in outline.path().commands() {
                cmds.push(match c {
                    Command::MoveTo(p) => Command::MoveTo(map(p, &mut bounds)),
                    Command::LineTo(p) => Command::LineTo(map(p, &mut bounds)),
                    Command::QuadTo(a, p) => Command::QuadTo(map(a, &mut bounds), map(p, &mut bounds)),
                    Command::CurveTo(a, b, p) => Command::CurveTo(map(a, &mut bounds), map(b, &mut bounds), map(p, &mut bounds)),
                    Command::Close => Command::Close,
                });
            }
            if !bounds.x.is_finite() {
                bounds = Rect::new(0.0, 0.0, 0.0, 0.0);
            }
            path = GlyphPath { commands: cmds, bounds };
        }
        let p = Arc::new(path);
        self.path_cache.insert(key, p.clone());
        p
    }

    /// Fill or stroke coverage of a glyph whose only transform is a
    /// translation to `(x, y)`; cached by quarter-pixel phase. `stroke_px`
    /// 0 fills; otherwise a centred stroke of that width.
    pub fn mask_at(&mut self, m: &FontMatch, glyph: u16, size_px: f32, x: f32, y: f32, stroke_px: f32) -> (Arc<CoverageMask>, i32, i32) {
        let (ix, iy) = (x.floor(), y.floor());
        let (fx, fy) = (x - ix, y - iy);
        let sub_x = ((fx * 4.0).round() as u8).min(3);
        let sub_y = ((fy * 4.0).round() as u8).min(3);
        let key = MaskKey { path: Self::path_key(m, glyph, size_px), sub_x, sub_y, stroke_q: (stroke_px * 16.0).round() as u32 };
        if let Some(mk) = self.cache.get(&key) {
            return (mk.clone(), ix as i32, iy as i32);
        }
        if self.cache.len() > 8192 {
            self.cache.clear();
        }
        let path = self.path(m, glyph, size_px);
        let t = Affine::translate(sub_x as f32 * 0.25, sub_y as f32 * 0.25);
        let mk = Arc::new(rasterise_path(&path.transformed(&t), stroke_px));
        self.cache.insert(key, mk.clone());
        (mk, ix as i32, iy as i32)
    }

    /// A colour glyph (emoji) at `size_px`, if the face has one.
    pub fn colour_glyph(&mut self, m: &FontMatch, glyph: u16, size_px: f32) -> Option<ColourGlyph> {
        let font = m.face.swash();
        let mut scaler = self.ctx.builder(font).size(size_px).hint(false).build();
        let img = Render::new(&[Source::ColorBitmap(StrikeWith::BestFit), Source::ColorOutline(0)]).render(&mut scaler, glyph)?;
        if img.placement.width == 0 || img.placement.height == 0 {
            return None;
        }
        match img.content {
            swash::scale::image::Content::Color => Some(ColourGlyph {
                left: img.placement.left,
                // swash reports `top` as the distance above the baseline.
                top: -img.placement.top,
                width: img.placement.width,
                height: img.placement.height,
                rgba: img.data,
            }),
            _ => None,
        }
    }
}

/// Rasterises a y-down path (pixels, any transform already applied) into
/// a mask placed relative to the path's origin.
pub fn rasterise_path(path: &GlyphPath, stroke_px: f32) -> CoverageMask {
    if path.is_empty() {
        return CoverageMask::default();
    }
    let cmds: &[Command] = &path.commands;
    let mut mask = Mask::new(cmds);
    let stroke = Stroke::new(stroke_px);
    if stroke_px > 0.0 {
        mask.style(Style::Stroke(stroke));
    } else {
        mask.style(Style::Fill(Fill::NonZero));
    }
    let (data, placement) = mask.render();
    CoverageMask {
        left: placement.left,
        top: placement.top,
        width: placement.width,
        height: placement.height,
        data: data.iter().map(|&v| v as f32 / 255.0).collect(),
    }
}

/// Rasterises several paths together (one mask, so overlaps do not double).
pub fn rasterise_paths(paths: &[GlyphPath], stroke_px: f32) -> CoverageMask {
    let mut all: Vec<Command> = Vec::new();
    let mut bounds = Rect::EMPTY;
    for p in paths {
        all.extend_from_slice(&p.commands);
        bounds = bounds.union(&p.bounds);
    }
    rasterise_path(&GlyphPath { commands: all, bounds }, stroke_px)
}

/// A rounded rectangle path.
pub fn rounded_rect_path(r: &Rect, radius: f32) -> GlyphPath {
    let rad = radius.max(0.0).min(r.w * 0.5).min(r.h * 0.5);
    let k = 0.5522848 * rad;
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let v = |x: f32, y: f32| Vector::new(x, y);
    let commands = if rad <= 0.01 {
        vec![Command::MoveTo(v(x0, y0)), Command::LineTo(v(x1, y0)), Command::LineTo(v(x1, y1)), Command::LineTo(v(x0, y1)), Command::Close]
    } else {
        vec![
            Command::MoveTo(v(x0 + rad, y0)),
            Command::LineTo(v(x1 - rad, y0)),
            Command::CurveTo(v(x1 - rad + k, y0), v(x1, y0 + rad - k), v(x1, y0 + rad)),
            Command::LineTo(v(x1, y1 - rad)),
            Command::CurveTo(v(x1, y1 - rad + k), v(x1 - rad + k, y1), v(x1 - rad, y1)),
            Command::LineTo(v(x0 + rad, y1)),
            Command::CurveTo(v(x0 + rad - k, y1), v(x0, y1 - rad + k), v(x0, y1 - rad)),
            Command::LineTo(v(x0, y0 + rad)),
            Command::CurveTo(v(x0, y0 + rad - k), v(x0 + rad - k, y0), v(x0 + rad, y0)),
            Command::Close,
        ]
    };
    GlyphPath { commands, bounds: *r }
}

/// A plain rectangle path.
pub fn rect_path(r: &Rect) -> GlyphPath {
    rounded_rect_path(r, 0.0)
}

// ---------------------------------------------------------------------------
// Float plane utilities

/// A single-channel f32 plane the size of a layer.
#[derive(Clone, Debug)]
pub struct Plane {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(width: u32, height: u32) -> Plane {
        Plane { width, height, data: vec![0.0; width as usize * height as usize] }
    }

    pub fn is_blank(&self) -> bool {
        self.data.iter().all(|&v| v <= 0.0)
    }

    /// Adds a mask (times `k`) with saturation, at layer offset (ox, oy).
    pub fn accumulate(&mut self, m: &CoverageMask, ox: i32, oy: i32, k: f32) {
        for row in 0..m.height as i32 {
            let y = oy + m.top + row;
            if y < 0 || y >= self.height as i32 {
                continue;
            }
            for col in 0..m.width as i32 {
                let x = ox + m.left + col;
                if x < 0 || x >= self.width as i32 {
                    continue;
                }
                let v = m.data[(row * m.width as i32 + col) as usize] * k;
                if v > 0.0 {
                    let i = (y * self.width as i32 + x) as usize;
                    // Union of coverages: a + b - ab keeps overlaps from
                    // exceeding full coverage without a hard clamp.
                    let a = self.data[i];
                    self.data[i] = a + v - a * v;
                }
            }
        }
    }

    pub fn get(&self, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            0.0
        } else {
            self.data[(y * self.width as i32 + x) as usize]
        }
    }

    /// `max(0, self - other)` per pixel.
    pub fn subtract(&self, other: &Plane) -> Plane {
        let data = self.data.iter().zip(&other.data).map(|(a, b)| (a - b).max(0.0)).collect();
        Plane { width: self.width, height: self.height, data }
    }

    pub fn union(&self, other: &Plane) -> Plane {
        let data = self.data.iter().zip(&other.data).map(|(a, b)| a + b - a * b).collect();
        Plane { width: self.width, height: self.height, data }
    }

    pub fn scaled(&self, k: f32) -> Plane {
        Plane { width: self.width, height: self.height, data: self.data.iter().map(|v| (v * k).min(1.0)).collect() }
    }

    /// Shifted by whole pixels (fractional parts are applied by linear
    /// interpolation).
    pub fn shifted(&self, dx: f32, dy: f32) -> Plane {
        let mut out = Plane::new(self.width, self.height);
        let (ix, iy) = (dx.floor() as i32, dy.floor() as i32);
        let (fx, fy) = (dx - dx.floor(), dy - dy.floor());
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let sx = x - ix;
                let sy = y - iy;
                let v00 = self.get(sx, sy);
                let v10 = self.get(sx - 1, sy);
                let v01 = self.get(sx, sy - 1);
                let v11 = self.get(sx - 1, sy - 1);
                let v = v00 * (1.0 - fx) * (1.0 - fy) + v10 * fx * (1.0 - fy) + v01 * (1.0 - fx) * fy + v11 * fx * fy;
                out.data[(y * self.width as i32 + x) as usize] = v;
            }
        }
        out
    }

    /// Gaussian blur with standard deviation `sigma` pixels, approximated
    /// by three box blurs (accurate to a few percent, much faster for the
    /// large radii glows use).
    pub fn blurred(&self, sigma: f32) -> Plane {
        if sigma < 0.3 {
            return self.clone();
        }
        let boxes = boxes_for_gauss(sigma, 3);
        let mut cur = self.clone();
        for r in boxes {
            cur = cur.box_blur(r);
        }
        cur
    }

    fn box_blur(&self, r: usize) -> Plane {
        let (w, h) = (self.width as usize, self.height as usize);
        let norm = 1.0 / (2 * r + 1) as f32;
        let mut tmp = vec![0.0f32; w * h];
        for y in 0..h {
            let row = &self.data[y * w..(y + 1) * w];
            let mut acc: f32 = (0..=r.min(w.saturating_sub(1))).map(|i| row[i]).sum::<f32>() + r as f32 * 0.0;
            // Sliding window with transparent edges.
            for x in 0..w {
                tmp[y * w + x] = acc * norm;
                let add = x + r + 1;
                if add < w {
                    acc += row[add];
                }
                if x >= r {
                    acc -= row[x - r];
                }
            }
        }
        let mut out = vec![0.0f32; w * h];
        for x in 0..w {
            let mut acc: f32 = (0..=r.min(h.saturating_sub(1))).map(|i| tmp[i * w + x]).sum();
            for y in 0..h {
                out[y * w + x] = acc * norm;
                let add = y + r + 1;
                if add < h {
                    acc += tmp[add * w + x];
                }
                if y >= r {
                    acc -= tmp[(y - r) * w + x];
                }
            }
        }
        Plane { width: self.width, height: self.height, data: out }
    }
}

fn boxes_for_gauss(sigma: f32, n: usize) -> Vec<usize> {
    let w_ideal = (12.0 * sigma * sigma / n as f32 + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - (n as f32) * (wl as f32) * (wl as f32) - 4.0 * n as f32 * wl as f32 - 3.0 * n as f32) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    (0..n as i32).map(|i| if i < m { ((wl - 1) / 2).max(0) as usize } else { ((wu - 1) / 2).max(0) as usize }).collect()
}

// ---------------------------------------------------------------------------
// Image compositing (linear light, premultiplied)

/// `src` over `dst` for one pixel.
#[inline]
pub fn over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

/// Paints `colour` (premultiplied linear) through a plane onto an image
/// of the same size.
pub fn paint_plane(img: &mut Image, plane: &Plane, colour: [f32; 4]) {
    for (p, &a) in img.pixels.iter_mut().zip(&plane.data) {
        if a > 0.0 {
            *p = over(*p, [colour[0] * a, colour[1] * a, colour[2] * a, colour[3] * a]);
        }
    }
}

/// Paints a plane with a per-pixel colour function `f(x, y)`.
pub fn paint_plane_fn(img: &mut Image, plane: &Plane, mut f: impl FnMut(u32, u32) -> [f32; 4]) {
    let w = img.width;
    for (i, (p, &a)) in img.pixels.iter_mut().zip(&plane.data).enumerate() {
        if a > 0.0 {
            let c = f(i as u32 % w, i as u32 / w);
            *p = over(*p, [c[0] * a, c[1] * a, c[2] * a, c[3] * a]);
        }
    }
}

/// Paints a mask at an offset with a per-pixel colour.
pub fn paint_mask_fn(img: &mut Image, m: &CoverageMask, ox: i32, oy: i32, k: f32, mut f: impl FnMut(i32, i32) -> [f32; 4]) {
    for row in 0..m.height as i32 {
        let y = oy + m.top + row;
        if y < 0 || y >= img.height as i32 {
            continue;
        }
        for col in 0..m.width as i32 {
            let x = ox + m.left + col;
            if x < 0 || x >= img.width as i32 {
                continue;
            }
            let a = m.data[(row * m.width as i32 + col) as usize] * k;
            if a > 0.0 {
                let c = f(x, y);
                let i = (y * img.width as i32 + x) as usize;
                img.pixels[i] = over(img.pixels[i], [c[0] * a, c[1] * a, c[2] * a, c[3] * a]);
            }
        }
    }
}

/// Draws a straight-alpha sRGB 8-bit RGBA bitmap through an affine
/// transform (bitmap pixel → layer pixel), bilinear, premultiplied linear
/// output. `alpha_out` collects the drawn alpha if given.
#[allow(clippy::too_many_arguments)]
pub fn draw_rgba8_affine(img: &mut Image, rgba: &[u8], w: u32, h: u32, t: &Affine, opacity: f32, tint: Option<[f32; 4]>, alpha_out: Option<&mut Plane>) {
    // Invert the affine.
    let det = t.a * t.d - t.b * t.c;
    if det.abs() < 1e-9 || opacity <= 0.0 {
        return;
    }
    let inv = Affine { a: t.d / det, b: -t.b / det, c: -t.c / det, d: t.a / det, e: (t.c * t.f - t.d * t.e) / det, f: (t.b * t.e - t.a * t.f) / det };
    let dst_bounds = t.bounds(&Rect::new(0.0, 0.0, w as f32, h as f32)).round_out();
    let x0 = (dst_bounds.x as i32).max(0);
    let y0 = (dst_bounds.y as i32).max(0);
    let x1 = (dst_bounds.right() as i32 + 1).min(img.width as i32);
    let y1 = (dst_bounds.bottom() as i32 + 1).min(img.height as i32);
    let lin: Vec<f32> = (0..256).map(|i| crate::rgba::srgb_to_linear(i as f32 / 255.0)).collect();
    let sample = |sx: f32, sy: f32| -> [f32; 4] {
        let fx = sx - 0.5;
        let fy = sy - 0.5;
        let ix = fx.floor() as i32;
        let iy = fy.floor() as i32;
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let px = |x: i32, y: i32| -> [f32; 4] {
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                return [0.0; 4];
            }
            let i = ((y as u32 * w + x as u32) * 4) as usize;
            let a = rgba[i + 3] as f32 / 255.0;
            [lin[rgba[i] as usize] * a, lin[rgba[i + 1] as usize] * a, lin[rgba[i + 2] as usize] * a, a]
        };
        let p00 = px(ix, iy);
        let p10 = px(ix + 1, iy);
        let p01 = px(ix, iy + 1);
        let p11 = px(ix + 1, iy + 1);
        let mut out = [0.0; 4];
        for c in 0..4 {
            out[c] = p00[c] * (1.0 - tx) * (1.0 - ty) + p10[c] * tx * (1.0 - ty) + p01[c] * (1.0 - tx) * ty + p11[c] * tx * ty;
        }
        out
    };
    let mut alpha_out = alpha_out;
    for y in y0..y1 {
        for x in x0..x1 {
            let s = inv.apply([x as f32 + 0.5, y as f32 + 0.5]);
            if s[0] < -1.0 || s[1] < -1.0 || s[0] > w as f32 + 1.0 || s[1] > h as f32 + 1.0 {
                continue;
            }
            let mut c = sample(s[0], s[1]);
            if c[3] <= 0.0 {
                continue;
            }
            if let Some(t) = tint {
                // Tint keeps the glyph's alpha and replaces its colour.
                c = [t[0] * c[3], t[1] * c[3], t[2] * c[3], c[3]];
            }
            let c = [c[0] * opacity, c[1] * opacity, c[2] * opacity, c[3] * opacity];
            let i = (y as u32 * img.width + x as u32) as usize;
            img.pixels[i] = over(img.pixels[i], c);
            if let Some(a) = alpha_out.as_deref_mut() {
                let v = a.data[i];
                a.data[i] = v + c[3] - v * c[3];
            }
        }
    }
}

/// Composites `src` over `dst` with `src`'s top-left at `(x, y)`.
/// A painted layer blurred by `radius` pixels (three box passes, about a
/// Gaussian), grown so the blur is not cut off.
pub fn blurred(r: &crate::paint::TextRaster, radius: f32) -> crate::paint::TextRaster {
    let rad = radius.max(0.0);
    if r.image.width == 0 || rad < 0.5 {
        return r.clone();
    }
    // Box half-width for three passes to give this spread.
    let box_r = ((rad * rad / 3.0 + 0.25).sqrt()).round().max(1.0) as usize;
    let pad = box_r * 3 + 1;
    let (w, h) = (r.image.width as usize + pad * 2, r.image.height as usize + pad * 2);
    let mut px = vec![[0.0f32; 4]; w * h];
    for y in 0..r.image.height as usize {
        for x in 0..r.image.width as usize {
            px[(y + pad) * w + x + pad] = r.image.pixels[y * r.image.width as usize + x];
        }
    }
    let mut tmp = vec![[0.0f32; 4]; w * h];
    let pass = |src: &[[f32; 4]], dst: &mut [[f32; 4]], len: usize, lines: usize, stride: usize, step: usize| {
        let k = 1.0 / (2 * box_r + 1) as f32;
        for line in 0..lines {
            let base = line * stride;
            let mut acc = [0.0f32; 4];
            let at = |i: isize| -> [f32; 4] { if i < 0 || i as usize >= len { [0.0; 4] } else { src[base + i as usize * step] } };
            for i in -(box_r as isize)..=(box_r as isize) {
                let v = at(i);
                for c in 0..4 {
                    acc[c] += v[c];
                }
            }
            for i in 0..len {
                dst[base + i * step] = [acc[0] * k, acc[1] * k, acc[2] * k, acc[3] * k];
                let (add, sub) = (at(i as isize + box_r as isize + 1), at(i as isize - box_r as isize));
                for c in 0..4 {
                    acc[c] += add[c] - sub[c];
                }
            }
        }
    };
    for _ in 0..3 {
        pass(&px, &mut tmp, w, h, w, 1);
        pass(&tmp, &mut px, h, w, 1, w);
    }
    let mut image = Image::new(w as u32, h as u32);
    image.pixels = px;
    crate::paint::TextRaster { image, left: r.left - pad as i32, top: r.top - pad as i32 }
}

pub fn blit_over(dst: &mut Image, src: &Image, x: i32, y: i32) {
    for sy in 0..src.height as i32 {
        let dy = y + sy;
        if dy < 0 || dy >= dst.height as i32 {
            continue;
        }
        for sx in 0..src.width as i32 {
            let dx = x + sx;
            if dx < 0 || dx >= dst.width as i32 {
                continue;
            }
            let s = src.pixels[(sy as u32 * src.width + sx as u32) as usize];
            if s[3] > 0.0 {
                let i = (dy as u32 * dst.width + dx as u32) as usize;
                dst.pixels[i] = over(dst.pixels[i], s);
            }
        }
    }
}

/// Multiplies every pixel by `k` (uniform opacity).
pub fn scale_alpha(img: &mut Image, k: f32) {
    if (k - 1.0).abs() < 1e-6 {
        return;
    }
    for p in &mut img.pixels {
        for c in p.iter_mut() {
            *c *= k;
        }
    }
}

/// Samples an image bilinearly at a pixel position (premultiplied).
pub fn sample(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let ix = fx.floor() as i32;
    let iy = fy.floor() as i32;
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let px = |x: i32, y: i32| -> [f32; 4] {
        let x = x.clamp(0, img.width as i32 - 1);
        let y = y.clamp(0, img.height as i32 - 1);
        img.pixels[(y as u32 * img.width + x as u32) as usize]
    };
    let (p00, p10, p01, p11) = (px(ix, iy), px(ix + 1, iy), px(ix, iy + 1), px(ix + 1, iy + 1));
    let mut out = [0.0; 4];
    for c in 0..4 {
        out[c] = p00[c] * (1.0 - tx) * (1.0 - ty) + p10[c] * tx * (1.0 - ty) + p01[c] * (1.0 - tx) * ty + p11[c] * tx * ty;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_mask_covers_centre_not_corner() {
        let m = rasterise_path(&rounded_rect_path(&Rect::new(0.0, 0.0, 40.0, 20.0), 8.0), 0.0);
        assert_eq!((m.width, m.height), (40, 20));
        let at = |x: usize, y: usize| m.data[y * 40 + x];
        assert!(at(20, 10) > 0.99);
        assert!(at(0, 0) < 0.3);
        assert!(at(20, 0) > 0.99);
    }

    #[test]
    fn blur_preserves_mass_inside() {
        let mut p = Plane::new(64, 64);
        p.data[32 * 64 + 32] = 1.0;
        let b = p.blurred(3.0);
        let sum: f32 = b.data.iter().sum();
        assert!((sum - 1.0).abs() < 0.02, "{sum}");
        assert!(b.data[32 * 64 + 32] < 0.1);
    }

    #[test]
    fn plane_shift_moves_content() {
        let mut p = Plane::new(8, 8);
        p.data[3 * 8 + 3] = 1.0;
        let s = p.shifted(2.0, 1.0);
        assert_eq!(s.get(5, 4), 1.0);
        assert_eq!(s.get(3, 3), 0.0);
    }

    #[test]
    fn over_is_associative_enough() {
        let a = [0.5, 0.0, 0.0, 0.5];
        let b = [0.0, 0.5, 0.0, 0.5];
        let o = over(a, b);
        assert!((o[3] - 0.75).abs() < 1e-6);
        assert!((o[0] - 0.25).abs() < 1e-6);
    }
}

/// Draws a premultiplied linear image through an affine transform
/// (source pixel → destination pixel) with bilinear sampling.
pub fn draw_image_affine(dst: &mut Image, src: &Image, t: &Affine, opacity: f32) {
    if src.width == 0 || src.height == 0 || opacity <= 0.0 {
        return;
    }
    if t.is_translation() && (t.e - t.e.round()).abs() < 1e-3 && (t.f - t.f.round()).abs() < 1e-3 && (opacity - 1.0).abs() < 1e-6 {
        blit_over(dst, src, t.e.round() as i32, t.f.round() as i32);
        return;
    }
    let det = t.a * t.d - t.b * t.c;
    if det.abs() < 1e-9 {
        return;
    }
    let inv = Affine { a: t.d / det, b: -t.b / det, c: -t.c / det, d: t.a / det, e: (t.c * t.f - t.d * t.e) / det, f: (t.b * t.e - t.a * t.f) / det };
    let db = t.bounds(&Rect::new(0.0, 0.0, src.width as f32, src.height as f32)).round_out();
    let x0 = (db.x as i32 - 1).max(0);
    let y0 = (db.y as i32 - 1).max(0);
    let x1 = (db.right() as i32 + 2).min(dst.width as i32);
    let y1 = (db.bottom() as i32 + 2).min(dst.height as i32);
    // Supersample when minifying so downscaled layers stay smooth.
    let scale = (t.a * t.a + t.b * t.b).sqrt().min((t.c * t.c + t.d * t.d).sqrt());
    let ss = if scale < 0.9 { ((1.0 / scale).ceil() as i32).clamp(2, 4) } else { 1 };
    let (w, h) = (src.width as f32, src.height as f32);
    for y in y0..y1 {
        for x in x0..x1 {
            let mut acc = [0.0f32; 4];
            let mut n = 0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let px = x as f32 + (sx as f32 + 0.5) / ss as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / ss as f32;
                    let s = inv.apply([px, py]);
                    n += 1;
                    if s[0] < -1.0 || s[1] < -1.0 || s[0] > w + 1.0 || s[1] > h + 1.0 {
                        continue;
                    }
                    let c = sample_edge_transparent(src, s[0], s[1]);
                    for k in 0..4 {
                        acc[k] += c[k];
                    }
                }
            }
            if acc[3] <= 0.0 {
                continue;
            }
            let k = opacity / n as f32;
            let c = [acc[0] * k, acc[1] * k, acc[2] * k, acc[3] * k];
            let i = (y as u32 * dst.width + x as u32) as usize;
            dst.pixels[i] = over(dst.pixels[i], c);
        }
    }
}

/// Bilinear sample with transparent outside.
pub fn sample_edge_transparent(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let ix = fx.floor() as i32;
    let iy = fy.floor() as i32;
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let px = |x: i32, y: i32| -> [f32; 4] {
        if x < 0 || y < 0 || x >= img.width as i32 || y >= img.height as i32 {
            [0.0; 4]
        } else {
            img.pixels[(y as u32 * img.width + x as u32) as usize]
        }
    };
    let (p00, p10, p01, p11) = (px(ix, iy), px(ix + 1, iy), px(ix, iy + 1), px(ix + 1, iy + 1));
    let mut out = [0.0; 4];
    for c in 0..4 {
        out[c] = p00[c] * (1.0 - tx) * (1.0 - ty) + p10[c] * tx * (1.0 - ty) + p01[c] * (1.0 - tx) * ty + p11[c] * tx * ty;
    }
    out
}

/// Rasterises a filled path into its own tight image.
pub fn path_image(path: &GlyphPath, colour: [f32; 4]) -> (Image, i32, i32) {
    let m = rasterise_path(path, 0.0);
    if m.is_empty() {
        return (Image::new(0, 0), 0, 0);
    }
    let mut img = Image::new(m.width, m.height);
    for (p, &a) in img.pixels.iter_mut().zip(&m.data) {
        *p = [colour[0] * a, colour[1] * a, colour[2] * a, colour[3] * a];
    }
    (img, m.left, m.top)
}
