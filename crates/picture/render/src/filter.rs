// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small image filters the compositor needs itself: a separable Gaussian
//! blur (motion-preset blur) and 90° rotations (source display rotation).

use crate::frame::{Frame, Px};

/// Gaussian kernel for `sigma`, truncated at three sigma, normalised.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as usize;
    let mut k: Vec<f32> = (0..=2 * r).map(|i| (-((i as f32 - r as f32).powi(2)) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k
}

/// Separable Gaussian blur with transparent borders.
pub fn gaussian_blur(src: &Frame, sigma: f32) -> Frame {
    if sigma <= 0.05 || src.width == 0 || src.height == 0 {
        return src.clone();
    }
    let k = gaussian_kernel(sigma);
    let r = (k.len() / 2) as i64;
    let (w, h) = (src.width as usize, src.height as usize);
    // Horizontal pass.
    let mut tmp = vec![[0.0f32; 4]; w * h];
    studio_media::par::par_rows(&mut tmp, w, 8, |y, out| {
        let row = &src.pixels[y * w..y * w + w];
        for (x, o) in out.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for (i, kv) in k.iter().enumerate() {
                let sx = x as i64 + i as i64 - r;
                if sx >= 0 && sx < w as i64 {
                    let p = row[sx as usize];
                    for c in 0..4 {
                        acc[c] += p[c] * kv;
                    }
                }
            }
            *o = acc;
        }
    });
    // Vertical pass.
    let mut out = vec![[0.0f32; 4]; w * h];
    studio_media::par::par_rows(&mut out, w, 8, |y, dst| {
        for (x, o) in dst.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for (i, kv) in k.iter().enumerate() {
                let sy = y as i64 + i as i64 - r;
                if sy >= 0 && sy < h as i64 {
                    let p = tmp[sy as usize * w + x];
                    for c in 0..4 {
                        acc[c] += p[c] * kv;
                    }
                }
            }
            *o = acc;
        }
    });
    Frame { width: src.width, height: src.height, pixels: out }
}

/// Rotates by a multiple of 90° clockwise.
pub fn rotate(src: &Frame, degrees: u32) -> Frame {
    let (w, h) = (src.width, src.height);
    match degrees % 360 {
        90 => Frame::from_fn(h, w, |x, y| src.get(y, h - 1 - x)),
        180 => Frame::from_fn(w, h, |x, y| src.get(w - 1 - x, h - 1 - y)),
        270 => Frame::from_fn(h, w, |x, y| src.get(w - 1 - y, x)),
        _ => src.clone(),
    }
}

/// Multiplies a premultiplied pixel by a factor.
#[inline]
pub fn scale_px(p: Px, k: f32) -> Px {
    [p[0] * k, p[1] * k, p[2] * k, p[3] * k]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_preserves_energy_inside() {
        let f = Frame::filled(32, 32, [1.0, 0.5, 0.25, 1.0]);
        let b = gaussian_blur(&f, 1.5);
        assert!((b.get(16, 16)[0] - 1.0).abs() < 1e-4);
        assert!(b.get(0, 0)[0] < 1.0);
        assert_eq!(gaussian_blur(&f, 0.0), f);
    }

    #[test]
    fn rotations() {
        let f = Frame::from_fn(3, 2, |x, y| [x as f32, y as f32, 0.0, 1.0]);
        let r = rotate(&f, 90);
        assert_eq!((r.width, r.height), (2, 3));
        // Top-left of the source goes to the top-right.
        assert_eq!(r.get(1, 0), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(rotate(&rotate(&f, 90), 270), f);
        assert_eq!(rotate(&rotate(&f, 180), 180), f);
    }
}
