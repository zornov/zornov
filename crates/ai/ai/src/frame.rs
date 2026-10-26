// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Frames, masks and the frame-source trait the features read clips through,
//! plus image helpers shared by several features (guided filter, affine
//! crops, luma).

use crate::error::{invalid, Result};

/// An owned straight-alpha RGBA8 image, row-major, no padding.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn new(width: usize, height: usize, rgba: Vec<u8>) -> Result<Frame> {
        if width == 0 || height == 0 || rgba.len() != width * height * 4 {
            return invalid(format!("frame {width}x{height} with {} bytes", rgba.len()));
        }
        Ok(Frame { width, height, rgba })
    }

    /// A solid colour frame.
    pub fn filled(width: usize, height: usize, px: [u8; 4]) -> Frame {
        Frame { width, height, rgba: px.repeat(width * height) }
    }

    pub fn view(&self) -> studio_nn::image::Rgba<'_> {
        studio_nn::image::Rgba { w: self.width, h: self.height, px: &self.rgba }
    }

    /// Rec. 709 luma in [0, 1] per pixel.
    pub fn luma(&self) -> Vec<f32> {
        self.rgba.chunks_exact(4).map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).collect()
    }

    /// Pixel (clamped to the edges).
    pub fn px(&self, x: isize, y: isize) -> [u8; 4] {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        let i = (y * self.width + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// Copy of this frame with `mask` as its alpha channel.
    pub fn with_alpha(&self, mask: &Mask) -> Frame {
        let mut out = self.clone();
        for (p, &a) in out.rgba.chunks_exact_mut(4).zip(&mask.data) {
            p[3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        out
    }
}

/// A single-channel float image in [0, 1] (alpha matte, object mask, depth).
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Mask {
    pub fn new(width: usize, height: usize, data: Vec<f32>) -> Mask {
        debug_assert_eq!(data.len(), width * height);
        Mask { width, height, data }
    }

    pub fn zeros(width: usize, height: usize) -> Mask {
        Mask { width, height, data: vec![0.0; width * height] }
    }

    /// Bilinear resize.
    pub fn resized(&self, width: usize, height: usize) -> Mask {
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        Mask { width, height, data: studio_nn::image::resize_plane(&self.data, self.width, self.height, width, height) }
    }

    /// As 8-bit alpha.
    pub fn to_u8(&self) -> Vec<u8> {
        studio_nn::image::mask_to_u8(&self.data)
    }

    /// Fraction of pixels above 0.5.
    pub fn coverage(&self) -> f32 {
        self.data.iter().filter(|&&v| v > 0.5).count() as f32 / self.data.len().max(1) as f32
    }

    /// Bounding box `(x0, y0, x1, y1)` (exclusive end) of pixels above 0.5.
    pub fn bbox(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.data[y * self.width + x] > 0.5 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        (x1 > 0).then_some((x0, y0, x1, y1))
    }

    /// Centre of mass of the mask values, in pixels.
    pub fn centroid(&self) -> Option<(f32, f32)> {
        let (mut sx, mut sy, mut s) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..self.height {
            for x in 0..self.width {
                let v = self.data[y * self.width + x] as f64;
                sx += v * x as f64;
                sy += v * y as f64;
                s += v;
            }
        }
        (s > 1e-6).then(|| ((sx / s) as f32 + 0.5, (sy / s) as f32 + 0.5))
    }
}

/// Where features read a clip's frames from. The render crate implements it
/// over its decoders; tests implement it over synthetic frames.
pub trait FrameSource {
    fn frame_count(&self) -> usize;
    /// Frames per second (for timestamps).
    fn fps(&self) -> f64;
    /// Decoded frame `index` (0-based, in presentation order).
    fn frame(&mut self, index: usize) -> Result<Frame>;
}

/// A [`FrameSource`] over frames held in memory.
pub struct VecSource {
    pub frames: Vec<Frame>,
    pub fps: f64,
}

impl FrameSource for VecSource {
    fn frame_count(&self) -> usize {
        self.frames.len()
    }
    fn fps(&self) -> f64 {
        self.fps
    }
    fn frame(&mut self, index: usize) -> Result<Frame> {
        match self.frames.get(index) {
            Some(f) => Ok(f.clone()),
            None => invalid(format!("frame {index} out of range")),
        }
    }
}

/// Box filter mean with radius `r` (clamped window at the borders), via a
/// summed-area table so the cost does not depend on `r`.
pub fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut sat = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += v[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0] + sat[y0 * (w + 1) + x0];
            out[y * w + x] = (s / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    out
}

/// Grey-guide guided filter (He, Sun, Tang 2010): smooths `p` while making
/// its edges follow the edges of `guide`. Used to snap low-resolution mattes
/// to hair and object outlines at full resolution.
pub fn guided_filter(guide: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let mean_i = box_mean(guide, w, h, r);
    let mean_p = box_mean(p, w, h, r);
    let ip: Vec<f32> = guide.iter().zip(p).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = guide.iter().map(|a| a * a).collect();
    let corr_ip = box_mean(&ip, w, h, r);
    let corr_ii = box_mean(&ii, w, h, r);
    let mut a = vec![0.0f32; w * h];
    let mut b = vec![0.0f32; w * h];
    for i in 0..w * h {
        let var = corr_ii[i] - mean_i[i] * mean_i[i];
        let cov = corr_ip[i] - mean_i[i] * mean_p[i];
        a[i] = cov / (var + eps);
        b[i] = mean_p[i] - a[i] * mean_i[i];
    }
    let ma = box_mean(&a, w, h, r);
    let mb = box_mean(&b, w, h, r);
    (0..w * h).map(|i| ma[i] * guide[i] + mb[i]).collect()
}

/// An affine crop: output pixel `(u, v)` of an `out_w × out_h` image samples
/// the source at `centre + R(angle) * ((u + 0.5) / out_w − 0.5, (v + 0.5) /
/// out_h − 0.5) * size`. Used for rotated face/pose regions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Roi {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// Radians, clockwise in image coordinates (y down).
    pub angle: f32,
}

impl Roi {
    /// Source position of normalised ROI coordinates `(u, v)` in [0, 1].
    pub fn to_source(&self, u: f32, v: f32) -> (f32, f32) {
        let (dx, dy) = ((u - 0.5) * self.w, (v - 0.5) * self.h);
        let (s, c) = self.angle.sin_cos();
        (self.cx + dx * c - dy * s, self.cy + dx * s + dy * c)
    }

    /// Bilinearly samples the ROI from `img` into RGB floats in [0, 1],
    /// interleaved (`out_h × out_w × 3`). Outside the image reads black.
    pub fn sample_rgb(&self, img: &Frame, out_w: usize, out_h: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; out_w * out_h * 3];
        for v in 0..out_h {
            for u in 0..out_w {
                let (sx, sy) = self.to_source((u as f32 + 0.5) / out_w as f32, (v as f32 + 0.5) / out_h as f32);
                let px = bilinear_rgb(img, sx - 0.5, sy - 0.5);
                out[(v * out_w + u) * 3..][..3].copy_from_slice(&px);
            }
        }
        out
    }
}

/// Bilinear RGB sample at pixel coordinates (centres at integers), black
/// outside.
pub fn bilinear_rgb(img: &Frame, x: f32, y: f32) -> [f32; 3] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let mut acc = [0.0f32; 3];
    for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            let (xi, yi) = (x0 as isize + dx, y0 as isize + dy);
            if xi < 0 || yi < 0 || xi >= img.width as isize || yi >= img.height as isize {
                continue;
            }
            let i = (yi as usize * img.width + xi as usize) * 4;
            for c in 0..3 {
                acc[c] += img.rgba[i + c] as f32 / 255.0 * wx * wy;
            }
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_mean_matches_brute_force() {
        let (w, h) = (7, 5);
        let v: Vec<f32> = (0..w * h).map(|i| ((i * 37) % 11) as f32).collect();
        let m = box_mean(&v, w, h, 2);
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0.0, 0);
                for yy in y.saturating_sub(2)..(y + 3).min(h) {
                    for xx in x.saturating_sub(2)..(x + 3).min(w) {
                        s += v[yy * w + xx];
                        n += 1;
                    }
                }
                assert!((m[y * w + x] - s / n as f32).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn guided_filter_snaps_to_guide_edges() {
        // Guide: hard vertical edge at x = 10. Input: a blurry ramp around it.
        let (w, h) = (20, 8);
        let guide: Vec<f32> = (0..w * h).map(|i| if i % w >= 10 { 1.0 } else { 0.0 }).collect();
        let p: Vec<f32> = (0..w * h).map(|i| ((i % w) as f32 - 6.0).clamp(0.0, 8.0) / 8.0).collect();
        let q = guided_filter(&guide, &p, w, h, 3, 1e-4);
        let row = &q[3 * w..4 * w];
        assert!(row[8] < 0.35 && row[11] > 0.65, "{row:?}");
        assert!((row[9] - row[10]).abs() > 0.3, "edge should be sharp: {row:?}");
    }

    #[test]
    fn mask_geometry() {
        let mut m = Mask::zeros(10, 10);
        for y in 2..5 {
            for x in 3..8 {
                m.data[y * 10 + x] = 1.0;
            }
        }
        assert_eq!(m.bbox(), Some((3, 2, 8, 5)));
        let (cx, cy) = m.centroid().unwrap();
        assert!((cx - 5.5).abs() < 1e-4 && (cy - 3.5).abs() < 1e-4);
        assert!((m.coverage() - 0.15).abs() < 1e-6);
        assert_eq!(Mask::zeros(4, 4).bbox(), None);
    }

    #[test]
    fn roi_rotation_maps_corners() {
        let r = Roi { cx: 50.0, cy: 50.0, w: 20.0, h: 10.0, angle: std::f32::consts::FRAC_PI_2 };
        let (x, y) = r.to_source(1.0, 0.5);
        assert!((x - 50.0).abs() < 1e-4 && (y - 60.0).abs() < 1e-4);
        let f = Frame::filled(4, 4, [255, 0, 0, 255]);
        let s = Roi { cx: 2.0, cy: 2.0, w: 4.0, h: 4.0, angle: 0.0 }.sample_rgb(&f, 2, 2);
        assert!(s.chunks(3).all(|p| (p[0] - 1.0).abs() < 1e-5 && p[1] == 0.0));
    }
}
