// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's vector rasterizer.
//!
//! Build a [`Scene`] of fills, strokes and layers in user space, then draw
//! it at any size with [`render`] (CPU) or [`gpu::GpuRaster`] (WGSL
//! compute). Both renderers share one preparation step and one coverage
//! model, the exact area of each pixel inside the shape, so they agree to
//! within rounding and art stays crisp at 8K and beyond:
//!
//! * fills with the non-zero or even-odd rule;
//! * strokes with miter, round and bevel joins, miter limits, butt, round
//!   and square caps, dashes, variable width profiles, inside/outside
//!   alignment and non-scaling hairlines;
//! * linear, radial (two-point conical, with focal points), conic,
//!   freeform and mesh gradients with pad, repeat and reflect, stop
//!   midpoints and sRGB, linear or Oklab interpolation;
//! * images (trilinear when shrunk, bicubic when enlarged) and vector
//!   patterns;
//! * the sixteen W3C blend modes and additive `Plus`, group opacity,
//!   clipping paths and luminance or alpha masks;
//! * blur, drop shadow, inner shadow, outer and inner glow, measured in
//!   user units.

mod blend;
pub mod color;
mod cpu;
mod filter;
#[cfg(feature = "gpu")]
pub mod gpu;
mod mesh;
pub mod paint;
mod prepare;
pub mod scene;
mod shade;
pub mod stroke;

pub use color::Color;
pub use kurbo;
pub use paint::{CoonsPatch, Extend, FreeformGradient, FreeformPoint, Gradient, GradientKind, Image, ImagePaint, ImageQuality, Interpolation, MeshGradient, Paint, Pattern, Stop};
pub use prepare::covered_area;
pub use scene::{BlendMode, Cap, Clip, FillRule, Filter, Item, Join, Layer, Mask, MaskKind, Scene, StrokeAlign, StrokeStyle, WidthPoint, solid};
pub use stroke::stroke_outline;

use kurbo::Affine;

/// How to draw a scene.
#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    /// Scene space to device pixels (e.g. `Affine::scale(4.0)` draws a
    /// 1920-unit artboard at 7680 pixels).
    pub transform: Affine,
    /// Painted under everything; `None` leaves the canvas transparent.
    pub background: Option<Color>,
    /// Largest distance, in device pixels, between a curve and the lines
    /// that stand in for it. The default (0.01 px) is far below what eyes
    /// or 8-bit output can see.
    pub tolerance: f64,
    /// Ordered dither when converting to 8 bits, so gradients don't band.
    pub dither: bool,
    /// Worker threads for the CPU renderer; 0 = one per core (up to 16).
    pub threads: usize,
}

impl RenderOptions {
    pub fn new(width: u32, height: u32) -> RenderOptions {
        RenderOptions { width, height, transform: Affine::IDENTITY, background: None, tolerance: 0.01, dither: true, threads: 0 }
    }

    pub fn with_transform(mut self, t: Affine) -> RenderOptions {
        self.transform = t;
        self
    }

    pub fn with_background(mut self, c: Color) -> RenderOptions {
        self.background = Some(c);
        self
    }

    /// Fits `area` (scene units) into the canvas, centred, keeping its
    /// proportions.
    pub fn fit(mut self, area: kurbo::Rect) -> RenderOptions {
        let s = (self.width as f64 / area.width()).min(self.height as f64 / area.height());
        if s.is_finite() && s > 0.0 {
            let dx = (self.width as f64 - area.width() * s) * 0.5;
            let dy = (self.height as f64 - area.height() * s) * 0.5;
            self.transform = Affine::translate((dx, dy)) * Affine::scale(s) * Affine::translate((-area.x0, -area.y0));
        }
        self
    }
}

/// RGBA8 pixels, premultiplied, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Pixmap {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// Straight (unpremultiplied) RGBA8, as PNG files store.
    pub fn to_straight(&self) -> Vec<u8> {
        let mut out = self.data.clone();
        for p in out.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }

    /// The image as a paint source.
    pub fn to_image(&self) -> Option<Image> {
        Image::from_premul(self.width, self.height, self.data.clone())
    }
}

pub(crate) fn threads_for(n: usize) -> usize {
    if n > 0 {
        return n;
    }
    std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4).min(16)
}

/// 8×8 Bayer matrix, centred on zero, in 1/255 steps.
fn dither_at(x: i32, y: i32) -> f32 {
    const B: [u8; 64] = [
        0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51,
        19, 59, 27, 49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
    ];
    (B[((y & 7) * 8 + (x & 7)) as usize] as f32 + 0.5) / 64.0 - 0.5
}

#[inline]
pub(crate) fn to_u8(v: f32, d: f32) -> u8 {
    (v * 255.0 + d + 0.5).clamp(0.0, 255.0) as u8
}

/// Draws `scene` on the CPU.
pub fn render(scene: &Scene, opts: &RenderOptions) -> Pixmap {
    let (w, h) = (opts.width as usize, opts.height as usize);
    let out = std::sync::Mutex::new(vec![0u8; w * h * 4]);
    let prep = prepare::prepare(scene, opts);
    let dither = opts.dither;
    cpu::render(&prep, opts.threads, &|band, px| {
        let mut bytes = vec![0u8; px.len() * 4];
        for y in band.y0..band.y1 {
            for x in 0..w as i32 {
                let p = px[((y - band.y0) as usize) * w + x as usize];
                let d = if dither { dither_at(x, y) } else { 0.0 };
                let a = to_u8(p[3], d);
                let i = (((y - band.y0) as usize) * w + x as usize) * 4;
                // Colour never exceeds alpha in premultiplied form.
                bytes[i] = to_u8(p[0], d).min(a);
                bytes[i + 1] = to_u8(p[1], d).min(a);
                bytes[i + 2] = to_u8(p[2], d).min(a);
                bytes[i + 3] = a;
            }
        }
        let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
        let off = band.y0 as usize * w * 4;
        o[off..off + bytes.len()].copy_from_slice(&bytes);
    });
    Pixmap { width: opts.width, height: opts.height, data: out.into_inner().unwrap_or_else(|e| e.into_inner()) }
}

/// Draws `scene` on the CPU into premultiplied floating-point pixels
/// (sRGB-encoded), for compositing at full precision.
pub fn render_f32(scene: &Scene, opts: &RenderOptions) -> Vec<[f32; 4]> {
    let w = opts.width as usize;
    let out = std::sync::Mutex::new(vec![[0.0f32; 4]; w * opts.height as usize]);
    let prep = prepare::prepare(scene, opts);
    cpu::render(&prep, opts.threads, &|band, px| {
        let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
        let off = band.y0 as usize * w;
        o[off..off + px.len()].copy_from_slice(px);
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}
