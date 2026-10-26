// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Look transfer: a picture (or every frame of a clip) given the colour
//! and contrast of a reference, by matching the mean and spread of each
//! channel in a perceptual opponent space (Reinhard et al., "Color
//! Transfer between Images", 2001, in their lαβ space). Statistics are
//! measured once for a clip, so every frame moves the same way and the
//! look holds steady. This moves colour and tone only; Codex (from the app)
//! redraws brush strokes, materials and drawing style.

use crate::frame::Frame;

/// Mean and spread of the three lαβ channels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub mean: [f32; 3],
    pub sd: [f32; 3],
}

fn to_lab(p: &[u8]) -> [f32; 3] {
    let (r, g, b) = (p[0] as f32 / 255.0 + 1e-3, p[1] as f32 / 255.0 + 1e-3, p[2] as f32 / 255.0 + 1e-3);
    let l = (0.3811 * r + 0.5783 * g + 0.0402 * b).ln();
    let m = (0.1967 * r + 0.7244 * g + 0.0782 * b).ln();
    let s = (0.0241 * r + 0.1288 * g + 0.8444 * b).ln();
    let k = std::f32::consts::FRAC_1_SQRT_2;
    [(l + m + s) / 3f32.sqrt(), (l + m - 2.0 * s) / 6f32.sqrt(), (l - m) * k]
}

fn from_lab(v: [f32; 3]) -> [u8; 3] {
    let (a, b, c) = (v[0] / 3f32.sqrt(), v[1] / 6f32.sqrt(), v[2] * std::f32::consts::FRAC_1_SQRT_2);
    let (l, m, s) = ((a + b + c).exp(), (a + b - c).exp(), (a - 2.0 * b).exp());
    let r = 4.4679 * l - 3.5873 * m + 0.1193 * s;
    let g = -1.2186 * l + 2.3809 * m - 0.1624 * s;
    let bb = 0.0497 * l - 0.2439 * m + 1.2045 * s;
    let q = |x: f32| ((x - 1e-3) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    [q(r), q(g), q(bb)]
}

/// The statistics of several frames together (every 4th pixel).
pub fn stats(frames: &[&Frame]) -> Stats {
    let (mut sum, mut sq, mut n) = ([0f64; 3], [0f64; 3], 0f64);
    for f in frames {
        for p in f.rgba.chunks_exact(4).step_by(4) {
            if p[3] < 128 {
                continue;
            }
            let v = to_lab(p);
            for c in 0..3 {
                sum[c] += v[c] as f64;
                sq[c] += (v[c] * v[c]) as f64;
            }
            n += 1.0;
        }
    }
    let n = n.max(1.0);
    let mean: [f32; 3] = std::array::from_fn(|c| (sum[c] / n) as f32);
    let sd: [f32; 3] = std::array::from_fn(|c| ((sq[c] / n - (sum[c] / n).powi(2)).max(0.0).sqrt() as f32).max(1e-4));
    Stats { mean, sd }
}

/// `f` moved from `from` (its own statistics, or its clip's) towards `to`
/// by `amount` (0..1).
pub fn transfer(f: &Frame, from: &Stats, to: &Stats, amount: f32) -> Frame {
    let a = amount.clamp(0.0, 1.0);
    let mut out = f.clone();
    // Spreads are not stretched more than 3× either way (a flat reference
    // would otherwise crush the picture).
    let k: [f32; 3] = std::array::from_fn(|c| (to.sd[c] / from.sd[c]).clamp(0.33, 3.0));
    for p in out.rgba.chunks_exact_mut(4) {
        let v = to_lab(p);
        let w: [f32; 3] = std::array::from_fn(|c| {
            let moved = (v[c] - from.mean[c]) * k[c] + to.mean[c];
            v[c] + (moved - v[c]) * a
        });
        let rgb = from_lab(w);
        p[..3].copy_from_slice(&rgb);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_takes_a_warm_reference() {
        let mut f = Frame::filled(8, 8, [100, 100, 100, 255]);
        for p in f.rgba.chunks_exact_mut(8) {
            p[..3].copy_from_slice(&[160, 160, 160]);
        }
        let mut warm = Frame::filled(8, 8, [200, 120, 60, 255]);
        for p in warm.rgba.chunks_exact_mut(8) {
            p[..3].copy_from_slice(&[240, 170, 90]);
        }
        let g = transfer(&f, &stats(&[&f]), &stats(&[&warm]), 1.0);
        let p = g.px(0, 0);
        assert!(p[0] > p[1] && p[1] > p[2], "{p:?}");
        let same = transfer(&f, &stats(&[&f]), &stats(&[&warm]), 0.0).px(1, 0);
        assert!(same.iter().zip(f.px(1, 0)).all(|(a, b)| (*a as i32 - b as i32).abs() <= 1), "{same:?}");
        let back = from_lab(to_lab(&[37, 150, 222, 255]));
        assert!(back.iter().zip([37u8, 150, 222]).all(|(a, b)| (*a as i32 - b as i32).abs() <= 1), "{back:?}");
    }
}
