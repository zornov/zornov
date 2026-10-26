// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Image building blocks shared by effects and transitions: separable
//! blurs, warps and size conversions.

use crate::image::{Edge, Image, Px};

/// Converts a length in percent of frame height to pixels.
#[inline]
pub fn pct_h(img: &Image, pct: f32) -> f32 {
    pct * 0.01 * img.height as f32
}

/// Gaussian blur with separate horizontal and vertical standard
/// deviations in pixels. Small sigmas use an exact kernel; large ones use
/// three box passes, which cost the same at any radius and are within a
/// few percent of a true gaussian.
pub fn gaussian_blur(img: &Image, sigma_x: f32, sigma_y: f32, edge: Edge) -> Image {
    let mut out = img.clone();
    blur_axis(&mut out, sigma_x, edge, true);
    blur_axis(&mut out, sigma_y, edge, false);
    out
}

fn blur_axis(img: &mut Image, sigma: f32, edge: Edge, horizontal: bool) {
    if !(sigma > 0.25) || img.width == 0 || img.height == 0 {
        return;
    }
    let sigma = sigma.min(4096.0);
    if sigma <= 6.0 {
        let kernel = gaussian_kernel(sigma);
        for_each_line(img, horizontal, |line, scratch| convolve_line(line, scratch, &kernel, edge));
    } else {
        // Three equal box passes have a variance of 3 * ((2r+1)^2 - 1) / 12.
        let r = ((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) * 0.5;
        for _ in 0..3 {
            for_each_line(img, horizontal, |line, scratch| box_line(line, scratch, r, edge));
        }
    }
}

/// Box blur with radius in pixels (fractional radii blend the end taps).
pub fn box_blur(img: &Image, rx: f32, ry: f32, edge: Edge) -> Image {
    let mut out = img.clone();
    if rx > 0.0 {
        for_each_line(&mut out, true, |line, scratch| box_line(line, scratch, rx.min(1.0e5), edge));
    }
    if ry > 0.0 {
        for_each_line(&mut out, false, |line, scratch| box_line(line, scratch, ry.min(1.0e5), edge));
    }
    out
}

fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil() as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// Runs `f` on every row (or column) with the line copied to a buffer, so
/// the same code handles both directions. Lines are spread over the cores.
fn for_each_line(img: &mut Image, horizontal: bool, f: impl Fn(&mut [Px], &mut Vec<Px>) + Sync) {
    let (w, h) = (img.width, img.height);
    if horizontal {
        crate::image::rows(&mut img.pixels, w, |_, row| {
            let mut scratch = Vec::new();
            f(row, &mut scratch);
        });
    } else {
        // Columns as rows of the transposed picture, then back.
        let mut t = vec![[0.0f32; 4]; w * h];
        for y in 0..h {
            for x in 0..w {
                t[x * h + y] = img.pixels[y * w + x];
            }
        }
        crate::image::rows(&mut t, h, |_, col| {
            let mut scratch = Vec::new();
            f(col, &mut scratch);
        });
        for x in 0..w {
            for y in 0..h {
                img.pixels[y * w + x] = t[x * h + y];
            }
        }
    }
}

#[inline]
fn line_fetch(src: &[Px], i: isize, edge: Edge) -> Px {
    let n = src.len() as isize;
    match edge {
        Edge::Clamp => src[i.clamp(0, n - 1) as usize],
        Edge::Transparent => {
            if i < 0 || i >= n {
                [0.0; 4]
            } else {
                src[i as usize]
            }
        }
        Edge::Wrap => src[i.rem_euclid(n) as usize],
        Edge::Mirror => {
            let m = i.rem_euclid(2 * n);
            src[if m < n { m } else { 2 * n - 1 - m } as usize]
        }
    }
}

fn convolve_line(line: &mut [Px], scratch: &mut Vec<Px>, kernel: &[f32], edge: Edge) {
    scratch.clear();
    scratch.extend_from_slice(line);
    let r = (kernel.len() / 2) as isize;
    for (i, out) in line.iter_mut().enumerate() {
        let mut acc = [0.0f32; 4];
        for (k, &w) in kernel.iter().enumerate() {
            let p = line_fetch(scratch, i as isize + k as isize - r, edge);
            for c in 0..4 {
                acc[c] += p[c] * w;
            }
        }
        *out = acc;
    }
}

/// Sliding-window box filter with a fractional radius: whole taps inside
/// `floor(r)` count fully and the next tap on each side counts by the
/// fractional part.
fn box_line(line: &mut [Px], scratch: &mut Vec<Px>, r: f32, edge: Edge) {
    let n = line.len() as isize;
    // Radii past four times the line length do not change the result much
    // for clamp edges and would only cost time.
    let r = r.min(4.0 * n as f32 + 4.0);
    let ri = r.floor() as isize;
    let frac = r - ri as f32;
    if ri == 0 && frac < 1e-4 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(line);
    let norm = 1.0 / (2 * ri + 1) as f32 / (1.0 + 2.0 * frac / (2 * ri + 1) as f32);
    let mut sum = [0.0f64; 4];
    for k in -ri..=ri {
        let p = line_fetch(scratch, k, edge);
        for c in 0..4 {
            sum[c] += p[c] as f64;
        }
    }
    for i in 0..n {
        let a = line_fetch(scratch, i - ri - 1, edge);
        let b = line_fetch(scratch, i + ri + 1, edge);
        let mut out = [0.0f32; 4];
        for c in 0..4 {
            out[c] = (sum[c] as f32 + frac * (a[c] + b[c])) * norm;
        }
        line[i as usize] = out;
        // Slide the window one step to the right.
        let leaving = line_fetch(scratch, i - ri, edge);
        let entering = line_fetch(scratch, i + ri + 1, edge);
        for c in 0..4 {
            sum[c] += entering[c] as f64 - leaving[c] as f64;
        }
    }
}

/// Resamples `img` through a coordinate mapping: output pixel centre
/// `(x, y)` (in pixels) takes the colour at `f(x, y)`.
pub fn warp(img: &Image, edge: Edge, f: impl Fn(f32, f32) -> (f32, f32) + Sync) -> Image {
    Image::from_fn(img.width, img.height, |x, y| {
        let (sx, sy) = f(x as f32 + 0.5, y as f32 + 0.5);
        img.sample(sx, sy, edge)
    })
}

/// Mixes two same-sized images: `t = 0` gives `a`, `t = 1` gives `b`.
pub fn mix(a: &Image, b: &Image, t: f32) -> Image {
    if t <= 0.0 {
        return a.clone();
    }
    if t >= 1.0 {
        return b.clone();
    }
    Image {
        width: a.width,
        height: a.height,
        pixels: a.pixels.iter().zip(&b.pixels).map(|(&p, &q)| crate::image::lerp_px(p, q, t)).collect(),
    }
}

/// Per-pixel mix with a mask (0 keeps `a`, 1 takes `b`).
pub fn mix_mask(a: &Image, b: &Image, mask: impl Fn(usize, usize) -> f32 + Sync) -> Image {
    a.map_xy(|x, y, p| crate::image::lerp_px(p, b.get(x, y), mask(x, y).clamp(0.0, 1.0)))
}

/// A single-channel `f32` image, used for luma, mattes and tracking.
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height] }
    }

    /// Perceptual-ish luma (square root of linear luminance) so feature
    /// detectors see shadows and highlights with similar contrast.
    pub fn from_image(img: &Image) -> Self {
        Self {
            width: img.width,
            height: img.height,
            data: img.pixels.iter().map(|&p| crate::image::luma(p).max(0.0).sqrt()).collect(),
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    #[inline]
    pub fn at(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.data[y * self.width + x]
    }

    /// Bilinear sample in pixel units where pixel (0, 0) covers 0..1, so
    /// its centre is at 0.5. Clamps at the edges.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(-1.0e6, 1.0e6) - 0.5;
        let y = y.clamp(-1.0e6, 1.0e6) - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (xi, yi) = (x0 as isize, y0 as isize);
        let a = self.at(xi, yi);
        let b = self.at(xi + 1, yi);
        let c = self.at(xi, yi + 1);
        let d = self.at(xi + 1, yi + 1);
        let top = a + (b - a) * fx;
        let bottom = c + (d - c) * fx;
        top + (bottom - top) * fy
    }

    /// Halves the size with a 2x2 average (odd edges are dropped).
    pub fn half(&self) -> Gray {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut out = Gray::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = ((2 * x) as isize, (2 * y) as isize);
                out.data[y * w + x] =
                    0.25 * (self.at(sx, sy) + self.at(sx + 1, sy) + self.at(sx, sy + 1) + self.at(sx + 1, sy + 1));
            }
        }
        out
    }

    /// Gaussian blur, done through the RGBA path to share code.
    pub fn blurred(&self, sigma: f32) -> Gray {
        let img = Image {
            width: self.width,
            height: self.height,
            pixels: self.data.iter().map(|&v| [v, 0.0, 0.0, 0.0]).collect(),
        };
        let b = gaussian_blur(&img, sigma, sigma, Edge::Clamp);
        Gray { width: self.width, height: self.height, data: b.pixels.iter().map(|p| p[0]).collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse(n: usize) -> Image {
        let mut img = Image::new(n, n);
        img.set(n / 2, n / 2, [1.0; 4]);
        img
    }

    #[test]
    fn blur_preserves_energy() {
        for sigma in [1.0, 3.0, 9.0] {
            let b = gaussian_blur(&impulse(81), sigma, sigma, Edge::Transparent);
            let s: f32 = b.pixels.iter().map(|p| p[0]).sum();
            assert!((s - 1.0).abs() < 1e-3, "sigma {sigma}: {s}");
        }
    }

    #[test]
    fn box_blur_flat_stays_flat() {
        let img = Image::filled(20, 10, [0.3, 0.2, 0.1, 1.0]);
        let b = box_blur(&img, 3.4, 2.2, Edge::Clamp);
        assert!(b.max_abs_diff(&img) < 1e-5);
    }

    #[test]
    fn large_sigma_matches_variance() {
        let n = 301;
        let mut img = Image::new(n, 1);
        img.set(n / 2, 0, [1.0; 4]);
        let b = gaussian_blur(&img, 20.0, 0.0, Edge::Transparent);
        let var: f32 = b.pixels.iter().enumerate().map(|(i, p)| p[0] * (i as f32 - (n / 2) as f32).powi(2)).sum();
        assert!((var.sqrt() - 20.0).abs() < 1.0, "{}", var.sqrt());
    }
}
