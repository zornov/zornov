// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The image type every effect works on: linear-light, premultiplied RGBA
//! stored as `f32`. Values above 1.0 are allowed (highlights), negative
//! values are not produced by any effect in this crate.

/// One premultiplied linear RGBA pixel.
pub type Px = [f32; 4];

/// What to return when sampling outside the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Repeat the nearest edge pixel. Good for blurs and distortions of
    /// full-frame footage, which should not darken at the borders.
    Clamp,
    /// Fully transparent. Good for moving layers around.
    Transparent,
    /// Tile the image.
    Wrap,
    /// Tile with every other copy mirrored, so there are no seams.
    Mirror,
}

/// A linear premultiplied RGBA `f32` image, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Px>,
}

impl Image {
    /// A fully transparent image.
    pub fn new(width: usize, height: usize) -> Self {
        Self::filled(width, height, [0.0; 4])
    }

    pub fn filled(width: usize, height: usize, px: Px) -> Self {
        Self { width, height, pixels: vec![px; width * height] }
    }

    /// Builds an image by calling `f(x, y)` for every pixel.
    pub fn from_fn(width: usize, height: usize, f: impl Fn(usize, usize) -> Px + Sync) -> Self {
        let mut pixels = vec![[0.0; 4]; width * height];
        rows(&mut pixels, width, |y, row| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = f(x, y);
            }
        });
        Self { width, height, pixels }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> Px {
        self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, px: Px) {
        self.pixels[y * self.width + x] = px;
    }

    /// Pixel lookup with integer coordinates that may lie outside.
    #[inline]
    pub fn fetch(&self, x: isize, y: isize, edge: Edge) -> Px {
        let (w, h) = (self.width as isize, self.height as isize);
        if w == 0 || h == 0 {
            return [0.0; 4];
        }
        let (x, y) = match edge {
            Edge::Clamp => (x.clamp(0, w - 1), y.clamp(0, h - 1)),
            Edge::Transparent => {
                if x < 0 || y < 0 || x >= w || y >= h {
                    return [0.0; 4];
                }
                (x, y)
            }
            Edge::Wrap => (x.rem_euclid(w), y.rem_euclid(h)),
            Edge::Mirror => (mirror(x, w), mirror(y, h)),
        };
        self.pixels[(y * w + x) as usize]
    }

    /// Bilinear sample at a position in pixel units, where the centre of
    /// pixel (0, 0) is at (0.5, 0.5).
    #[inline]
    pub fn sample(&self, x: f32, y: f32, edge: Edge) -> Px {
        // Non-finite coordinates come from degenerate parameter values;
        // answering transparent keeps them from poisoning the output.
        if !x.is_finite() || !y.is_finite() {
            return [0.0; 4];
        }
        // Far-away coordinates would overflow isize casts.
        let x = x.clamp(-1.0e7, 1.0e7) - 0.5;
        let y = y.clamp(-1.0e7, 1.0e7) - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let (xi, yi) = (x0 as isize, y0 as isize);
        let a = self.fetch(xi, yi, edge);
        let b = self.fetch(xi + 1, yi, edge);
        let c = self.fetch(xi, yi + 1, edge);
        let d = self.fetch(xi + 1, yi + 1, edge);
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }

    /// Sample with coordinates normalised to 0..1 across the image.
    #[inline]
    pub fn sample_uv(&self, u: f32, v: f32, edge: Edge) -> Px {
        self.sample(u * self.width as f32, v * self.height as f32, edge)
    }

    pub fn map(&self, mut f: impl FnMut(Px) -> Px) -> Image {
        Image { width: self.width, height: self.height, pixels: self.pixels.iter().map(|&p| f(p)).collect() }
    }

    /// Calls `f(x, y, pixel)` for every pixel and collects the results.
    pub fn map_xy(&self, f: impl Fn(usize, usize, Px) -> Px + Sync) -> Image {
        let w = self.width;
        let mut pixels = self.pixels.clone();
        rows(&mut pixels, w, |y, row| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = f(x, y, *p);
            }
        });
        Image { width: w, height: self.height, pixels }
    }

    /// True when no channel is NaN or infinite.
    pub fn is_finite(&self) -> bool {
        self.pixels.iter().all(|p| p.iter().all(|c| c.is_finite()))
    }

    /// Replaces any non-finite channel with zero.
    pub fn sanitise(&mut self) {
        for p in &mut self.pixels {
            for c in p.iter_mut() {
                if !c.is_finite() {
                    *c = 0.0;
                }
            }
        }
    }

    /// Largest per-channel absolute difference. Images of different size
    /// compare as infinitely different.
    pub fn max_abs_diff(&self, other: &Image) -> f32 {
        if self.width != other.width || self.height != other.height {
            return f32::INFINITY;
        }
        let mut m = 0.0f32;
        for (a, b) in self.pixels.iter().zip(&other.pixels) {
            for i in 0..4 {
                m = m.max((a[i] - b[i]).abs());
            }
        }
        m
    }

    /// Converts to straight-alpha sRGB-encoded bytes, for previews and
    /// golden files. Values are clamped to 0..1.
    pub fn to_srgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            let a = p[3].clamp(0.0, 1.0);
            for c in &p[..3] {
                let straight = if a > 1e-6 { c / a } else { 0.0 };
                out.push((linear_to_srgb(straight.clamp(0.0, 1.0)) * 255.0).round() as u8);
            }
            out.push((a * 255.0).round() as u8);
        }
        out
    }

    /// Builds an image from straight-alpha sRGB-encoded RGBA bytes.
    pub fn from_srgb8(width: usize, height: usize, bytes: &[u8]) -> Image {
        assert_eq!(bytes.len(), width * height * 4, "byte count does not match size");
        let pixels = bytes
            .chunks_exact(4)
            .map(|c| {
                let a = c[3] as f32 / 255.0;
                [
                    srgb_to_linear(c[0] as f32 / 255.0) * a,
                    srgb_to_linear(c[1] as f32 / 255.0) * a,
                    srgb_to_linear(c[2] as f32 / 255.0) * a,
                    a,
                ]
            })
            .collect();
        Image { width, height, pixels }
    }
}

#[inline]
fn mirror(i: isize, n: isize) -> isize {
    let period = 2 * n;
    let m = i.rem_euclid(period);
    if m < n { m } else { period - 1 - m }
}

/// sRGB transfer function (IEC 61966-2-1), encode direction.
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB transfer function, decode direction.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Rec. 709 luminance of a linear pixel (premultiplied in, premultiplied out).
#[inline]
pub fn luma(p: Px) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

#[inline]
pub fn lerp_px(a: Px, b: Px, t: f32) -> Px {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

#[inline]
pub fn scale_px(a: Px, s: f32) -> Px {
    [a[0] * s, a[1] * s, a[2] * s, a[3] * s]
}

#[inline]
pub fn add_px(a: Px, b: Px) -> Px {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

/// Porter-Duff source-over for premultiplied pixels.
#[inline]
pub fn over(src: Px, dst: Px) -> Px {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

/// Adds emitted light to a pixel. Light falling on a transparent area
/// makes it as opaque as the light is bright, so glows stay visible when
/// the layer is composited.
#[inline]
pub fn add_light(p: Px, l: [f32; 3]) -> Px {
    let l = [l[0].max(0.0), l[1].max(0.0), l[2].max(0.0)];
    let a = p[3].max(l[0].max(l[1]).max(l[2]).min(1.0));
    [p[0] + l[0], p[1] + l[1], p[2] + l[2], a]
}

/// Straight (un-premultiplied) colour of a pixel.
#[inline]
pub fn unpremultiply(p: Px) -> [f32; 3] {
    if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { [0.0; 3] }
}

#[inline]
pub fn premultiply(rgb: [f32; 3], a: f32) -> Px {
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_hits_pixel_centres() {
        let img = Image::from_fn(3, 2, |x, y| [x as f32, y as f32, 0.0, 1.0]);
        assert_eq!(img.sample(1.5, 0.5, Edge::Clamp), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(img.sample(2.0, 1.0, Edge::Clamp), [1.5, 0.5, 0.0, 1.0]);
    }

    #[test]
    fn mirror_edges() {
        assert_eq!(mirror(-1, 4), 0);
        assert_eq!(mirror(4, 4), 3);
        assert_eq!(mirror(9, 4), 1);
    }

    #[test]
    fn srgb_round_trip() {
        for i in 0..=255u8 {
            let v = i as f32 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }
}

/// Runs `f` on every row of a `width`-wide picture, the rows split over
/// the cores: effects are per-pixel work, and one core for a 1080p frame
/// made heavy looks (god rays, glow, blur) far slower than real time.
pub(crate) fn rows<T: Send>(pixels: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let height = if width == 0 { 0 } else { pixels.len() / width };
    if height == 0 {
        return;
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
    // Small pictures are not worth the threads.
    if cores < 2 || pixels.len() < 64 * 1024 {
        for (y, row) in pixels.chunks_mut(width).enumerate() {
            f(y, row);
        }
        return;
    }
    let per = height.div_ceil(cores * 2).max(1);
    std::thread::scope(|scope| {
        for (i, band) in pixels.chunks_mut(per * width).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (j, row) in band.chunks_mut(width).enumerate() {
                    f(i * per + j, row);
                }
            });
        }
    });
}
