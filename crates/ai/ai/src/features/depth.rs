// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Depth maps (for depth blur, fog, 3D-photo parallax) from Depth Anything
//! V2 (small). The model predicts relative inverse depth; maps are scaled to
//! [0, 1] with 1 = nearest. For video the scaling range and the map itself
//! are smoothed over time, so the depth of a still scene does not pump.
//!
//! Yang et al., "Depth Anything V2" (2024). Small model weights: Apache-2.0.

use crate::engine;
use crate::error::{invalid, Result};
use crate::features::matting::MatteSmoother;
use crate::frame::{Frame, FrameSource, Mask};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

pub struct DepthEstimator {
    session: Arc<Session>,
    /// Short side the model sees (multiple of 14). 518 is the training size.
    pub inference_size: usize,
    /// Temporal smoothing strength for clips (0..1).
    pub temporal_smoothing: f32,
    range: Option<(f32, f32)>,
    smoother: MatteSmoother,
}

impl DepthEstimator {
    pub fn load(store: &ModelStore) -> Result<DepthEstimator> {
        let dir = store.require(ModelId::DepthAnythingV2Small)?;
        Ok(DepthEstimator { session: engine::session(&dir.join("model.onnx"))?, inference_size: 518, temporal_smoothing: 0.5, range: None, smoother: MatteSmoother::default() })
    }

    /// Raw relative inverse depth at model resolution.
    fn raw(&self, frame: &Frame) -> Result<Mask> {
        if frame.width < 14 || frame.height < 14 {
            return invalid("frame too small for depth");
        }
        // Scale so the short side is the inference size, keep the aspect,
        // round both sides to a multiple of 14 (the ViT patch size).
        let s = self.inference_size.max(14) as f32 / frame.width.min(frame.height) as f32;
        let r14 = |v: f32| (((v / 14.0).round() as usize).max(1)) * 14;
        let (w, h) = (r14(frame.width as f32 * s), r14(frame.height as f32 * s));
        let input = studio_nn::image::to_nchw(frame.view(), w, h, Norm::IMAGENET);
        let mut o = engine::run(&self.session, vec![("pixel_values", input)])?;
        let d = o.take("predicted_depth")?;
        let (oh, ow) = (d.shape[d.rank() - 2], d.shape[d.rank() - 1]);
        Ok(Mask::new(ow, oh, d.into_f32_vec()))
    }

    /// Depth of a single image, full resolution, 1 = near.
    pub fn estimate(&self, frame: &Frame) -> Result<Mask> {
        let raw = self.raw(frame)?;
        let (lo, hi) = percentile_range(&raw.data);
        Ok(normalise(&raw, lo, hi).resized(frame.width, frame.height))
    }

    /// Depth of the next frame of a clip. Call [`DepthEstimator::reset`] at
    /// cuts.
    pub fn next_frame(&mut self, frame: &Frame) -> Result<Mask> {
        let raw = self.raw(frame)?;
        let (lo, hi) = percentile_range(&raw.data);
        let k = 0.8 * self.temporal_smoothing.clamp(0.0, 1.0);
        let (lo, hi) = match self.range {
            Some((plo, phi)) => (plo * k + lo * (1.0 - k), phi * k + hi * (1.0 - k)),
            None => (lo, hi),
        };
        self.range = Some((lo, hi));
        let m = normalise(&raw, lo, hi).resized(frame.width, frame.height);
        Ok(self.smoother.push(frame, m, self.temporal_smoothing))
    }

    pub fn reset(&mut self) {
        self.range = None;
        self.smoother = MatteSmoother::default();
    }

    pub fn process_clip(&mut self, src: &mut dyn FrameSource, ctx: &JobCtx, mut sink: impl FnMut(usize, Mask) -> Result<()>) -> Result<()> {
        self.reset();
        let n = src.frame_count();
        for i in 0..n {
            ctx.check()?;
            ctx.set_progress(i as f32 / n.max(1) as f32);
            ctx.set_message(format!("Estimating depth: frame {} / {n}", i + 1));
            let m = self.next_frame(&src.frame(i)?)?;
            sink(i, m)?;
        }
        Ok(())
    }
}

/// 1st and 99th percentile: a few extreme pixels must not squash the range.
fn percentile_range(v: &[f32]) -> (f32, f32) {
    let mut s: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return (0.0, 1.0);
    }
    s.sort_by(f32::total_cmp);
    let at = |p: f32| s[((s.len() - 1) as f32 * p) as usize];
    (at(0.01), at(0.99))
}

fn normalise(m: &Mask, lo: f32, hi: f32) -> Mask {
    let span = (hi - lo).max(1e-6);
    Mask::new(m.width, m.height, m.data.iter().map(|&v| ((v - lo) / span).clamp(0.0, 1.0)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_ignores_outliers() {
        let mut v: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        v[0] = -100.0;
        v[999] = 1e6;
        let (lo, hi) = percentile_range(&v);
        assert!(lo > -1.0 && hi < 1.0);
        let n = normalise(&Mask::new(2, 1, vec![lo, hi]), lo, hi);
        assert_eq!(n.data, vec![0.0, 1.0]);
    }

    #[test]
    fn needs_the_model() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        assert!(matches!(DepthEstimator::load(&store), Err(crate::Error::ModelMissing(ModelId::DepthAnythingV2Small))));
    }
}
