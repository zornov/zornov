// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Super-resolution: a picture made 2 to 4 times larger with real detail,
//! with blur, noise and compression blocks cleaned up on the way, by
//! Real-ESRGAN's general model (`realesr-general-x4v3`, the small
//! SRVGGNetCompact network). It works in overlapping tiles, so any size
//! fits in memory and the seams never show. Without the model a sharpened
//! bicubic enlargement stands in.
//!
//! Wang et al., "Real-ESRGAN: Training Real-World Blind Super-Resolution
//! with Pure Synthetic Data" (ICCVW 2021). Weights: BSD-3-Clause.

use crate::engine;
use crate::error::{invalid, Result};
use crate::frame::Frame;
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::{crop_rgba, tiles, Norm, Rgba};
use studio_nn::Session;

/// The network's own factor.
pub const NATIVE: usize = 4;
/// Core tile size and the context around it (model pixels).
const TILE: usize = 192;
const OVERLAP: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpscaleEngine {
    RealEsrgan,
    /// Bicubic with an unsharp mask.
    Bicubic,
}

pub struct Upscaler {
    session: Option<Arc<Session>>,
}

impl Upscaler {
    /// Real-ESRGAN if installed, else the bicubic fallback.
    pub fn load(store: &ModelStore) -> Result<Upscaler> {
        let session = match store.require(ModelId::RealEsrganGeneralX4v3) {
            Ok(dir) => Some(engine::session(&dir.join("model.onnx"))?),
            Err(_) => None,
        };
        Ok(Upscaler { session })
    }

    pub fn fallback() -> Upscaler {
        Upscaler { session: None }
    }

    pub fn engine(&self) -> UpscaleEngine {
        if self.session.is_some() { UpscaleEngine::RealEsrgan } else { UpscaleEngine::Bicubic }
    }

    /// `f` at `w`×`h` (larger than `f`, at most 4× each side; smaller sizes
    /// are scaled down from the 4× result, as Real-ESRGAN's `--outscale`
    /// does).
    pub fn upscale_to(&self, f: &Frame, w: usize, h: usize, ctx: &JobCtx) -> Result<Frame> {
        if w == 0 || h == 0 || w > f.width * NATIVE || h > f.height * NATIVE {
            return invalid(format!("{}×{} is not within 4× of {}×{}", w, h, f.width, f.height));
        }
        let Some(s) = &self.session else { return Ok(sharpen(&bicubic(f, w, h), 0.35)) };
        let big = self.x4(s, f, ctx)?;
        Ok(if (big.width, big.height) == (w, h) { big } else { area(&big, w, h) })
    }

    /// `f` `scale` times larger (1 to 4).
    pub fn upscale(&self, f: &Frame, scale: f32, ctx: &JobCtx) -> Result<Frame> {
        let scale = scale.clamp(1.0, NATIVE as f32);
        let (w, h) = (((f.width as f32 * scale).round() as usize).max(1), ((f.height as f32 * scale).round() as usize).max(1));
        self.upscale_to(f, w, h, ctx)
    }

    /// Cleans `f` at its own size: the 4× pass scaled back down, which takes
    /// out noise, blur and compression blocks and keeps real edges.
    pub fn clean(&self, f: &Frame, ctx: &JobCtx) -> Result<Frame> {
        let Some(s) = &self.session else { return Ok(f.clone()) };
        let big = self.x4(s, f, ctx)?;
        Ok(area(&big, f.width, f.height))
    }

    fn x4(&self, s: &Session, f: &Frame, ctx: &JobCtx) -> Result<Frame> {
        let (ow, oh) = (f.width * NATIVE, f.height * NATIVE);
        let mut out = vec![0u8; ow * oh * 4];
        let img = Rgba { w: f.width, h: f.height, px: &f.rgba };
        let list = tiles(f.width, f.height, TILE, OVERLAP);
        for (k, t) in list.iter().enumerate() {
            ctx.check()?;
            let px = crop_rgba(img, t.x, t.y, t.w, t.h);
            let input = studio_nn::image::to_nchw(Rgba { w: t.w, h: t.h, px: &px }, t.w, t.h, Norm::UNIT);
            let mut o = engine::run(s, vec![("input", input)])?;
            let y = o.take("output")?;
            let (yh, yw) = (y.shape[y.rank() - 2], y.shape[y.rank() - 1]);
            if (yw, yh) != (t.w * NATIVE, t.h * NATIVE) {
                return invalid(format!("the upscaler returned {yw}×{yh} for a {}×{} tile", t.w, t.h));
            }
            let v = y.f32s();
            let plane = yw * yh;
            let (cx, cy) = ((t.core_x - t.x) * NATIVE, (t.core_y - t.y) * NATIVE);
            for yy in 0..t.core_h * NATIVE {
                for xx in 0..t.core_w * NATIVE {
                    let si = (cy + yy) * yw + cx + xx;
                    let di = ((t.core_y * NATIVE + yy) * ow + t.core_x * NATIVE + xx) * 4;
                    for c in 0..3 {
                        out[di + c] = (v[c * plane + si] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            ctx.set_progress((k + 1) as f32 / list.len() as f32);
        }
        // Alpha: bilinear (the model is RGB only).
        if f.rgba.chunks_exact(4).any(|p| p[3] != 255) {
            let a = bicubic(f, ow, oh);
            for (o, s) in out.chunks_exact_mut(4).zip(a.rgba.chunks_exact(4)) {
                o[3] = s[3];
            }
        } else {
            for o in out.chunks_exact_mut(4) {
                o[3] = 255;
            }
        }
        Frame::new(ow, oh, out)
    }
}

/// Catmull-Rom bicubic resize (any size).
pub fn bicubic(f: &Frame, w: usize, h: usize) -> Frame {
    let k = |x: f32| {
        let x = x.abs();
        if x < 1.0 {
            1.5 * x * x * x - 2.5 * x * x + 1.0
        } else if x < 2.0 {
            -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0
        } else {
            0.0
        }
    };
    let (sx, sy) = (f.width as f32 / w as f32, f.height as f32 / h as f32);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let fy = (y as f32 + 0.5) * sy - 0.5;
        let y0 = fy.floor() as isize;
        let wy: [f32; 4] = std::array::from_fn(|i| k(fy - (y0 - 1 + i as isize) as f32));
        for x in 0..w {
            let fx = (x as f32 + 0.5) * sx - 0.5;
            let x0 = fx.floor() as isize;
            let wx: [f32; 4] = std::array::from_fn(|i| k(fx - (x0 - 1 + i as isize) as f32));
            let mut acc = [0f32; 4];
            let mut wsum = 0.0;
            for (j, wyj) in wy.iter().enumerate() {
                for (i, wxi) in wx.iter().enumerate() {
                    let p = f.px(x0 - 1 + i as isize, y0 - 1 + j as isize);
                    let wgt = wxi * wyj;
                    wsum += wgt;
                    for c in 0..4 {
                        acc[c] += p[c] as f32 * wgt;
                    }
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / wsum + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    Frame { width: w, height: h, rgba: out }
}

/// Box-filter (area) downscale; bicubic when not smaller.
pub fn area(f: &Frame, w: usize, h: usize) -> Frame {
    if w >= f.width || h >= f.height {
        return bicubic(f, w, h);
    }
    let mut out = vec![0u8; w * h * 4];
    let (sx, sy) = (f.width as f64 / w as f64, f.height as f64 / h as f64);
    for y in 0..h {
        let (y0, y1) = ((y as f64 * sy) as usize, (((y + 1) as f64 * sy).ceil() as usize).min(f.height));
        for x in 0..w {
            let (x0, x1) = ((x as f64 * sx) as usize, (((x + 1) as f64 * sx).ceil() as usize).min(f.width));
            let mut acc = [0u32; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * f.width + xx) * 4;
                    for c in 0..4 {
                        acc[c] += f.rgba[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u32;
            let o = (y * w + x) * 4;
            for c in 0..4 {
                out[o + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    Frame { width: w, height: h, rgba: out }
}

/// Unsharp mask on RGB: `amount` of the difference from a 3×3 blur.
pub fn sharpen(f: &Frame, amount: f32) -> Frame {
    let mut out = f.clone();
    for y in 0..f.height {
        for x in 0..f.width {
            let o = (y * f.width + x) * 4;
            for c in 0..3 {
                let mut s = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        s += f.px(x as isize + dx, y as isize + dy)[c] as f32;
                    }
                }
                let v = f.rgba[o + c] as f32;
                out.rgba[o + c] = (v + amount * (v - s / 9.0)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_enlarges_and_keeps_flat_colour() {
        let up = Upscaler::fallback();
        assert_eq!(up.engine(), UpscaleEngine::Bicubic);
        let f = Frame::filled(10, 6, [40, 120, 200, 255]);
        let g = up.upscale(&f, 2.0, &JobCtx::detached()).unwrap();
        assert_eq!((g.width, g.height), (20, 12));
        assert!(g.rgba.chunks_exact(4).all(|p| p == [40, 120, 200, 255]));
        assert!(up.upscale_to(&f, 50, 12, &JobCtx::detached()).is_err());
    }

    #[test]
    fn area_and_bicubic_sizes() {
        let mut f = Frame::filled(8, 8, [0, 0, 0, 255]);
        for p in f.rgba.chunks_exact_mut(8) {
            p[..4].copy_from_slice(&[255, 255, 255, 255]);
        }
        let s = area(&f, 4, 4);
        assert_eq!((s.width, s.height), (4, 4));
        assert!(s.rgba.chunks_exact(4).all(|p| (p[0] as i32 - 128).abs() < 2), "{:?}", &s.rgba[..8]);
        let b = bicubic(&f, 16, 3);
        assert_eq!(b.rgba.len(), 16 * 3 * 4);
    }

    /// The real model, when installed: a soft edge comes back sharper than
    /// bicubic gives it.
    #[test]
    fn real_esrgan_sharpens_when_installed() {
        let store = ModelStore::default();
        if !store.is_installed(ModelId::RealEsrganGeneralX4v3) {
            eprintln!("realesr-general-x4v3 not installed; skipping");
            return;
        }
        let up = Upscaler::load(&store).unwrap();
        let mut f = Frame::filled(40, 30, [30, 30, 30, 255]);
        for y in 0..30 {
            for x in 20..40 {
                let i = (y * 40 + x) * 4;
                f.rgba[i..i + 3].copy_from_slice(&[220, 210, 200]);
            }
        }
        let f = bicubic(&area(&f, 20, 15), 40, 30);
        let g = up.upscale(&f, 2.0, &JobCtx::detached()).unwrap();
        assert_eq!((g.width, g.height), (80, 60));
        let b = bicubic(&f, 80, 60);
        let edge = |fr: &Frame| (0..fr.width - 1).map(|x| (fr.px(x as isize + 1, 30)[0] as i32 - fr.px(x as isize, 30)[0] as i32).abs()).max().unwrap();
        assert!(edge(&g) > edge(&b), "esrgan {} vs bicubic {}", edge(&g), edge(&b));
    }
}
