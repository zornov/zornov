// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The working-space picture: linear-light, premultiplied RGBA `f32`.
//!
//! `Frame` is the renderer's own buffer type. It has the same memory layout
//! as `studio_colour::Image` and `studio_fx::Image` (a `Vec<[f32; 4]>` of
//! rows top to bottom), so moving between the three costs nothing but a
//! change of the width/height fields.

use std::path::Path;

/// One premultiplied linear RGBA pixel.
pub type Px = [f32; 4];

/// A linear-light, premultiplied RGBA `f32` picture, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Px>,
}

impl Frame {
    /// A transparent black frame.
    pub fn new(width: u32, height: u32) -> Frame {
        Frame { width, height, pixels: vec![[0.0; 4]; width as usize * height as usize] }
    }

    /// A frame filled with one (premultiplied) colour.
    pub fn filled(width: u32, height: u32, px: Px) -> Frame {
        Frame { width, height, pixels: vec![px; width as usize * height as usize] }
    }

    /// Builds a frame by calling `f(x, y)` for every pixel.
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> Px) -> Frame {
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.push(f(x, y));
            }
        }
        Frame { width, height, pixels }
    }

    /// Wraps existing pixels. Panics when the length does not match.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<Px>) -> Frame {
        assert_eq!(pixels.len(), width as usize * height as usize, "pixel count mismatch");
        Frame { width, height, pixels }
    }

    pub fn len(&self) -> usize {
        self.pixels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }

    /// Bytes used by the pixel buffer.
    pub fn byte_size(&self) -> usize {
        self.pixels.len() * 16
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Px {
        self.pixels[(y * self.width + x) as usize]
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, p: Px) {
        self.pixels[(y * self.width + x) as usize] = p;
    }

    /// Pixel at integer coordinates, transparent outside the frame.
    #[inline]
    pub fn fetch(&self, x: i64, y: i64) -> Px {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            [0.0; 4]
        } else {
            self.pixels[(y as u32 * self.width + x as u32) as usize]
        }
    }

    /// Pixel at integer coordinates, clamped to the nearest edge pixel.
    #[inline]
    pub fn fetch_clamped(&self, x: i64, y: i64) -> Px {
        if self.width == 0 || self.height == 0 {
            return [0.0; 4];
        }
        let x = x.clamp(0, self.width as i64 - 1) as u32;
        let y = y.clamp(0, self.height as i64 - 1) as u32;
        self.pixels[(y * self.width + x) as usize]
    }

    /// Bilinear sample at a position in pixel units; the centre of pixel
    /// (0, 0) is at (0.5, 0.5). Outside the frame is transparent, so edges
    /// fade over half a pixel exactly like a GPU sampler with
    /// clamp-to-transparent borders.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> Px {
        if !x.is_finite() || !y.is_finite() {
            return [0.0; 4];
        }
        let x = x.clamp(-1.0e7, 1.0e7) - 0.5;
        let y = y.clamp(-1.0e7, 1.0e7) - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let (xi, yi) = (x0 as i64, y0 as i64);
        let a = self.fetch(xi, yi);
        let b = self.fetch(xi + 1, yi);
        let c = self.fetch(xi, yi + 1);
        let d = self.fetch(xi + 1, yi + 1);
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }

    /// Bilinear sample with the edge pixels repeated outside the frame.
    #[inline]
    pub fn sample_clamped(&self, x: f32, y: f32) -> Px {
        if !x.is_finite() || !y.is_finite() {
            return [0.0; 4];
        }
        let x = x.clamp(-1.0e7, 1.0e7) - 0.5;
        let y = y.clamp(-1.0e7, 1.0e7) - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let (xi, yi) = (x0 as i64, y0 as i64);
        let a = self.fetch_clamped(xi, yi);
        let b = self.fetch_clamped(xi + 1, yi);
        let c = self.fetch_clamped(xi, yi + 1);
        let d = self.fetch_clamped(xi + 1, yi + 1);
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }

    /// Halves the frame with a 2×2 box filter (odd edges repeat the last
    /// row or column). Used as a prefilter when a layer is drawn much
    /// smaller than its source, so minification does not alias.
    pub fn half(&self) -> Frame {
        let w = (self.width / 2).max(1);
        let h = (self.height / 2).max(1);
        Frame::from_fn(w, h, |x, y| {
            let (x0, y0) = (x as i64 * 2, y as i64 * 2);
            let a = self.fetch_clamped(x0, y0);
            let b = self.fetch_clamped(x0 + 1, y0);
            let c = self.fetch_clamped(x0, y0 + 1);
            let d = self.fetch_clamped(x0 + 1, y0 + 1);
            std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25)
        })
    }

    /// Multiplies every channel (colour and alpha) by `k`.
    pub fn scale_alpha(&mut self, k: f32) {
        if k == 1.0 {
            return;
        }
        for p in &mut self.pixels {
            for c in p.iter_mut() {
                *c *= k;
            }
        }
    }

    /// Crops to a pixel rectangle, clamped to the frame.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Frame {
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        let x0 = x.min(x1);
        let y0 = y.min(y1);
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut pixels = Vec::with_capacity(cw as usize * ch as usize);
        for row in y0..y1 {
            let start = (row * self.width + x0) as usize;
            pixels.extend_from_slice(&self.pixels[start..start + cw as usize]);
        }
        Frame { width: cw, height: ch, pixels }
    }

    /// True when no channel is NaN or infinite.
    pub fn is_finite(&self) -> bool {
        self.pixels.iter().all(|p| p.iter().all(|c| c.is_finite()))
    }

    /// Converts into the colour crate's image type without copying.
    pub fn into_colour(self) -> studio_colour::Image {
        studio_colour::Image::from_pixels(self.width, self.height, self.pixels)
    }

    /// Takes a colour-crate image back without copying.
    pub fn from_colour(img: studio_colour::Image) -> Frame {
        Frame { width: img.width, height: img.height, pixels: img.pixels }
    }

    /// Converts into the effects crate's image type without copying.
    #[cfg(feature = "fx")]
    pub fn into_fx(self) -> studio_fx::Image {
        studio_fx::Image { width: self.width as usize, height: self.height as usize, pixels: self.pixels }
    }

    /// Takes an effects-crate image back without copying.
    #[cfg(feature = "fx")]
    pub fn from_fx(img: studio_fx::Image) -> Frame {
        Frame { width: img.width as u32, height: img.height as u32, pixels: img.pixels }
    }

    /// Encodes as 8-bit sRGB PNG with straight alpha (for goldens and
    /// debugging), through the output converter with dithering off.
    pub fn to_srgb_png(&self) -> Vec<u8> {
        let out = crate::output::OutputSettings { dither: false, ..Default::default() };
        let rgba = crate::output::convert_rgba8(self, &out, 0);
        let img = studio_media::RgbaImage { width: self.width, height: self.height, pixels: rgba };
        studio_media::png::encode(&img)
    }

    /// Writes [`Frame::to_srgb_png`] to disk.
    pub fn save_png(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_srgb_png())
    }

    /// Loads an 8-bit PNG as a premultiplied linear frame (sRGB decoded).
    pub fn load_png(path: &Path) -> Result<Frame, String> {
        let img = studio_media::png::read(path)?;
        Ok(Frame::from_rgba8_srgb(img.width, img.height, &img.pixels))
    }

    /// Straight-alpha 8-bit sRGB pixels → premultiplied linear frame.
    pub fn from_rgba8_srgb(width: u32, height: u32, rgba: &[u8]) -> Frame {
        let lut = crate::colour_space::srgb8_to_linear_lut();
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for p in rgba.chunks_exact(4) {
            let a = p[3] as f32 / 255.0;
            pixels.push([lut[p[0] as usize] * a, lut[p[1] as usize] * a, lut[p[2] as usize] * a, a]);
        }
        Frame { width, height, pixels }
    }
}

/// Un-premultiplies one pixel (alpha 0 gives black).
#[inline]
pub fn unpremultiply(p: Px) -> Px {
    if p[3] <= 0.0 {
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]]
    }
}

/// Premultiplies one straight-alpha pixel.
#[inline]
pub fn premultiply(p: Px) -> Px {
    [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
}

/// Like [`compare`] but the tolerance scales with the pixel's magnitude
/// (`tol × max(1, |channel|)`), which is what `f16` storage on the GPU
/// path warrants for HDR values above 1.
pub fn compare_relative(a: &Frame, b: &Frame, tol: f32) -> (f32, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height), "frame size mismatch");
    let mut max = 0.0f32;
    let mut bad = 0usize;
    for (p, q) in a.pixels.iter().zip(&b.pixels) {
        let scale = p.iter().fold(1.0f32, |m, v| m.max(v.abs()));
        let d = (0..4).map(|i| (p[i] - q[i]).abs()).fold(0.0f32, f32::max) / scale;
        max = max.max(d);
        if d > tol {
            bad += 1;
        }
    }
    (max, bad as f64 / a.pixels.len().max(1) as f64)
}

/// Largest absolute per-channel difference between two frames of the same
/// size, and the fraction of pixels whose largest channel difference is
/// above `tol`. Used by the golden and agreement tests.
pub fn compare(a: &Frame, b: &Frame, tol: f32) -> (f32, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height), "frame size mismatch");
    let mut max = 0.0f32;
    let mut bad = 0usize;
    for (p, q) in a.pixels.iter().zip(&b.pixels) {
        let d = (0..4).map(|i| (p[i] - q[i]).abs()).fold(0.0f32, f32::max);
        max = max.max(d);
        if d > tol {
            bad += 1;
        }
    }
    (max, bad as f64 / a.pixels.len().max(1) as f64)
}
