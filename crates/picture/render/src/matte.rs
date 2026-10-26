// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cut-out mattes: a per-frame alpha picture (8 bits, in the red channel of
//! an RGBA8 picture, any size) multiplied into a media clip's picture right
//! after it is converted, before crop, effects and masks. The GPU's
//! `matte.wgsl` does the same sum: a bilinear sample of the matte at each
//! pixel's centre, edges clamped.

use crate::frame::Frame;
use crate::yuv::{DecodedPicture, PixelData};

/// The matte's coverage at picture pixel (`x`, `y`) of a `w`×`h` picture.
#[inline]
pub fn sample(matte: &[u8], mw: u32, mh: u32, x: u32, y: u32, w: u32, h: u32) -> f32 {
    let fx = (x as f32 + 0.5) * mw as f32 / w as f32 - 0.5;
    let fy = (y as f32 + 0.5) * mh as f32 / h as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |xi: f32, yi: f32| -> f32 {
        let xi = (xi as i64).clamp(0, mw as i64 - 1) as usize;
        let yi = (yi as i64).clamp(0, mh as i64 - 1) as usize;
        matte[(yi * mw as usize + xi) * 4] as f32 / 255.0
    };
    let top = at(x0, y0) + (at(x0 + 1.0, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y0 + 1.0) + (at(x0 + 1.0, y0 + 1.0) - at(x0, y0 + 1.0)) * tx;
    top + (bottom - top) * ty
}

/// Multiplies `frame` (premultiplied) by the matte; `invert` keeps what
/// the matte leaves out instead.
pub fn apply(frame: &mut Frame, matte: &DecodedPicture, invert: bool) {
    let PixelData::Rgba8(m) = &matte.data else { return };
    let (w, h) = (frame.width, frame.height);
    let (mw, mh) = (matte.width.max(1), matte.height.max(1));
    studio_media::par::par_rows(&mut frame.pixels, w as usize, 32, |row, out| {
        for (x, px) in out.iter_mut().enumerate() {
            let a = sample(m, mw, mh, x as u32, row as u32, w, h);
            let a = if invert { 1.0 - a } else { a };
            for c in px.iter_mut() {
                *c *= a;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_size_matte_cuts_on_its_edge() {
        // Left half opaque, at half the picture's size.
        let (mw, mh) = (8u32, 4u32);
        let mut px = vec![0u8; (mw * mh * 4) as usize];
        for y in 0..mh {
            for x in 0..mw / 2 {
                px[((y * mw + x) * 4) as usize] = 255;
            }
        }
        let matte = DecodedPicture::from_rgba8(mw, mh, px);
        let mut f = Frame { width: 16, height: 8, pixels: vec![[0.5, 0.4, 0.3, 1.0]; 128] };
        apply(&mut f, &matte, false);
        assert_eq!(f.pixels[0], [0.5, 0.4, 0.3, 1.0]);
        assert_eq!(f.pixels[15], [0.0; 4]);
        // At the edge, between.
        let a = f.pixels[8][3];
        assert!(a > 0.0 && a < 1.0, "{a}");
        let mut g = Frame { width: 16, height: 8, pixels: vec![[1.0; 4]; 128] };
        apply(&mut g, &matte, true);
        assert_eq!(g.pixels[0], [0.0; 4]);
        assert_eq!(g.pixels[15], [1.0; 4]);
    }
}
