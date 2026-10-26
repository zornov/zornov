// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A procedural test picture with the features effects react to: smooth
//! gradients, saturated colours, skin-like tones, fine detail, a highlight
//! brighter than white, hard edges and a partly transparent corner. Used by
//! the tests and handy for hover previews in the effect browser.

use crate::image::{premultiply, srgb_to_linear, Image};
use crate::math::{hash2, smoothstep};

/// Renders the test card at any size. `shift` moves the foreground
/// objects right by that fraction of the width, to fake motion between
/// frames for the time effects.
pub fn test_card(width: usize, height: usize, shift: f32) -> Image {
    let (w, h) = (width as f32, height as f32);
    let lin = |r: f32, g: f32, b: f32| [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)];
    Image::from_fn(width, height, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
        let aspect = w / h;
        // Sky to ground.
        let mut c = if v < 0.62 {
            let t = v / 0.62;
            lin(0.35 + 0.4 * t, 0.55 + 0.3 * t, 0.85 + 0.05 * t)
        } else {
            // Grass with fine noise texture.
            let n = hash2(x as i32, y as i32, 3) * 0.12;
            lin(0.25 + n, 0.45 + n, 0.18 + n * 0.5)
        };
        // Sun: a highlight above 1.0 so glow and bloom have something to do.
        let (sx, sy) = (0.78 * aspect, 0.2);
        let d = ((u * aspect - sx).powi(2) + (v - sy).powi(2)).sqrt();
        let sun = smoothstep(0.075, 0.06, d);
        for ch in &mut c {
            *ch = *ch * (1.0 - sun) + 2.5 * sun;
        }
        // Moving objects: a red ball and a skin-toned face-like ellipse.
        let ox = shift * aspect;
        let ball = ((u * aspect - 0.45 * aspect - ox).powi(2) + (v - 0.7).powi(2)).sqrt();
        let k = smoothstep(0.13, 0.12, ball);
        let shade = 0.75 + 0.25 * (1.0 - ball / 0.13).max(0.0);
        let red = lin(0.85 * shade, 0.12 * shade, 0.1 * shade);
        let face = ((u * aspect - 0.25 * aspect - ox) / 0.09).powi(2) + ((v - 0.42) / 0.12).powi(2);
        let kf = smoothstep(1.05, 0.95, face);
        let skin = lin(0.92, 0.72, 0.6);
        for i in 0..3 {
            c[i] = c[i] * (1.0 - k) + red[i] * k;
            c[i] = c[i] * (1.0 - kf) + skin[i] * kf;
        }
        // Colour swatches along the bottom.
        if v > 0.9 {
            let sw = [
                lin(0.9, 0.9, 0.9),
                lin(0.9, 0.85, 0.1),
                lin(0.1, 0.8, 0.85),
                lin(0.1, 0.75, 0.15),
                lin(0.8, 0.1, 0.75),
                lin(0.85, 0.1, 0.1),
                lin(0.1, 0.15, 0.85),
                lin(0.02, 0.02, 0.02),
            ];
            c = sw[((u * 8.0) as usize).min(7)];
        }
        // Stripes for fine detail in the upper left.
        if u < 0.12 && v < 0.25 {
            let s = if (x / 2) % 2 == 0 { 0.9 } else { 0.1 };
            c = lin(s, s, s);
        }
        // Top-right corner fades to transparent.
        let alpha = 1.0 - smoothstep(0.9, 1.0, u) * smoothstep(0.1, 0.0, v);
        premultiply(c, alpha)
    })
}
