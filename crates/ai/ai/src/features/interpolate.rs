// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Frame interpolation for smooth slow motion: synthesises frames between
//! two real ones with RIFE, which estimates the motion between them and
//! warps both towards the in-between time. Without the model, frames are
//! cross-faded (the classic "frame blending" look).
//!
//! RIFE: Huang et al., "Real-Time Intermediate Flow Estimation for Video
//! Frame Interpolation" (ECCV 2022). Weights (Practical-RIFE 4.9): MIT.

use crate::engine::{self, Tensor};
use crate::error::{invalid, Result};
use crate::frame::{Frame, FrameSource};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpEngine {
    Rife,
    /// Cross-fade fallback.
    Blend,
}

pub struct FrameInterpolator {
    session: Option<Arc<Session>>,
}

/// RIFE downsamples by up to 32 internally and the ensemble pass flips the
/// image, so sizes are padded to a multiple of 64.
const PAD: usize = 64;

impl FrameInterpolator {
    /// RIFE if installed, else the blend fallback (never fails for a
    /// missing model).
    pub fn load(store: &ModelStore) -> Result<FrameInterpolator> {
        let session = match store.require(ModelId::Rife49) {
            Ok(dir) => Some(engine::session(&dir.join("rife49.onnx"))?),
            Err(_) => None,
        };
        Ok(FrameInterpolator { session })
    }

    pub fn fallback() -> FrameInterpolator {
        FrameInterpolator { session: None }
    }

    pub fn engine(&self) -> InterpEngine {
        if self.session.is_some() { InterpEngine::Rife } else { InterpEngine::Blend }
    }

    /// The frame at fraction `t` (0..1) of the way from `a` to `b`.
    pub fn interpolate(&self, a: &Frame, b: &Frame, t: f32) -> Result<Frame> {
        if (a.width, a.height) != (b.width, b.height) {
            return invalid("frames differ in size");
        }
        let t = t.clamp(0.0, 1.0);
        let Some(s) = &self.session else { return Ok(blend(a, b, t)) };
        let to_t = |f: &Frame| studio_nn::image::pad_to_multiple(&studio_nn::image::to_nchw(f.view(), f.width, f.height, studio_nn::image::Norm::UNIT), PAD).0;
        let mut o = engine::run(s, vec![("img0", to_t(a)), ("img1", to_t(b)), ("timestep", Tensor::f32(&[1], vec![t]))])?;
        let out = studio_nn::image::crop_nchw(&o.take("output")?, a.height, a.width);
        let (mut rgba, _, _) = studio_nn::image::nchw_to_rgba(&out, |_, v| v);
        // alpha is not part of the model: blend it
        for ((p, pa), pb) in rgba.chunks_exact_mut(4).zip(a.rgba.chunks_exact(4)).zip(b.rgba.chunks_exact(4)) {
            p[3] = (pa[3] as f32 * (1.0 - t) + pb[3] as f32 * t).round() as u8;
        }
        Frame::new(a.width, a.height, rgba)
    }

    /// Slows a clip down by `factor` (> 1): emits `(n − 1) · factor + 1`
    /// frames to `sink`, the originals at multiples of `factor` and
    /// synthesised ones between.
    pub fn slow_motion(&self, src: &mut dyn FrameSource, factor: f64, ctx: &JobCtx, mut sink: impl FnMut(usize, Frame) -> Result<()>) -> Result<()> {
        if !(factor >= 1.0 && factor.is_finite()) {
            return invalid("slow-motion factor must be at least 1");
        }
        let n = src.frame_count();
        if n == 0 {
            return Ok(());
        }
        let total = ((n - 1) as f64 * factor).floor() as usize + 1;
        let mut cache: Option<(usize, Frame, Frame)> = None;
        for i in 0..total {
            ctx.check()?;
            ctx.set_progress(i as f32 / total as f32);
            ctx.set_message(format!("Interpolating frame {} / {total}", i + 1));
            let pos = i as f64 / factor;
            let j = (pos.floor() as usize).min(n - 1);
            let t = (pos - j as f64) as f32;
            if t < 1e-4 || j + 1 >= n {
                sink(i, src.frame(j)?)?;
                continue;
            }
            if cache.as_ref().is_none_or(|c| c.0 != j) {
                let a = match cache.take() {
                    Some((k, _, b)) if k + 1 == j => b,
                    _ => src.frame(j)?,
                };
                cache = Some((j, a, src.frame(j + 1)?));
            }
            let (_, a, b) = cache.as_ref().expect("filled above");
            sink(i, self.interpolate(a, b, t)?)?;
        }
        Ok(())
    }
}

/// Linear cross-fade.
pub fn blend(a: &Frame, b: &Frame, t: f32) -> Frame {
    let rgba = a.rgba.iter().zip(&b.rgba).map(|(&x, &y)| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8).collect();
    Frame { width: a.width, height: a.height, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::VecSource;

    #[test]
    fn blend_fallback_and_slow_motion_layout() {
        let fi = FrameInterpolator::fallback();
        assert_eq!(fi.engine(), InterpEngine::Blend);
        let a = Frame::filled(4, 2, [0, 0, 0, 255]);
        let b = Frame::filled(4, 2, [200, 100, 50, 255]);
        let m = fi.interpolate(&a, &b, 0.5).unwrap();
        assert_eq!(&m.rgba[..4], &[100, 50, 25, 255]);
        let mut src = VecSource { frames: vec![a.clone(), b.clone(), a.clone()], fps: 30.0 };
        let mut got = Vec::new();
        fi.slow_motion(&mut src, 4.0, &JobCtx::detached(), |i, f| {
            got.push((i, f.rgba[0]));
            Ok(())
        })
        .unwrap();
        assert_eq!(got.len(), 9);
        assert_eq!(got[0], (0, 0));
        assert_eq!(got[4], (4, 200));
        assert_eq!(got[2].1, 100);
        assert_eq!(got[8], (8, 0));
        assert!(fi.slow_motion(&mut src, 0.5, &JobCtx::detached(), |_, _| Ok(())).is_err());
    }

    #[test]
    fn size_mismatch_is_rejected() {
        let fi = FrameInterpolator::fallback();
        assert!(fi.interpolate(&Frame::filled(2, 2, [0; 4]), &Frame::filled(3, 2, [0; 4]), 0.5).is_err());
    }
}
