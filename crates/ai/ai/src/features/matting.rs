// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background removal for video: a portrait matting model (MODNet) predicts
//! an alpha matte per frame at a modest resolution; the matte is upscaled,
//! its edges are refined against the full-resolution frame with a guided
//! filter, and it is smoothed over time so it does not flicker.
//!
//! MODNet: Ke et al., "MODNet: Real-Time Trimap-Free Portrait Matting via
//! Objective Decomposition" (AAAI 2022). Weights: Apache-2.0.

use crate::engine;
use crate::error::{invalid, Result};
use crate::frame::{guided_filter, Frame, FrameSource, Mask};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

/// User-facing settings ("Remove background" inspector).
#[derive(Clone, Debug, PartialEq)]
pub struct MattingSettings {
    /// Short side of the image the model sees (rounded to a multiple of 32).
    /// 512 is what MODNet was trained for; larger is slower, not always
    /// better.
    pub inference_size: usize,
    /// Snap the matte's edges to the frame (hair, fingers).
    pub edge_refine: bool,
    /// 0 = no temporal smoothing, 1 = strongest. Moving areas are always
    /// smoothed less so fast motion does not leave ghosts.
    pub temporal_smoothing: f32,
    /// Softens the matte edge by this many pixels (at output resolution).
    pub feather: f32,
}

impl Default for MattingSettings {
    fn default() -> Self {
        MattingSettings { inference_size: 512, edge_refine: true, temporal_smoothing: 0.6, feather: 0.0 }
    }
}

impl MattingSettings {
    pub fn cache_params(&self) -> String {
        format!("v1 size={} refine={} smooth={:.3} feather={:.2}", self.inference_size, self.edge_refine, self.temporal_smoothing, self.feather)
    }
}

/// The background remover. Keep one per clip: it carries the temporal state.
pub struct BackgroundRemover {
    session: Arc<Session>,
    pub settings: MattingSettings,
    smoother: MatteSmoother,
}

impl BackgroundRemover {
    pub fn load(store: &ModelStore) -> Result<BackgroundRemover> {
        let dir = store.require(ModelId::Modnet)?;
        Ok(BackgroundRemover { session: engine::session(&dir.join("model.onnx"))?, settings: MattingSettings::default(), smoother: MatteSmoother::default() })
    }

    /// The raw model matte of one image at full resolution (no temporal
    /// smoothing; edge refinement and feathering per the settings).
    pub fn matte(&self, frame: &Frame) -> Result<Mask> {
        if frame.width < 8 || frame.height < 8 {
            return invalid("frame too small for matting");
        }
        let (w, h) = studio_nn::image::fit_short_side(frame.width, frame.height, self.settings.inference_size.max(64), 32);
        let input = studio_nn::image::to_nchw(frame.view(), w, h, Norm::SIGNED);
        let mut o = engine::run(&self.session, vec![("input", input)])?;
        let small = o.take("output")?;
        let m = Mask::new(w, h, small.into_f32_vec()).resized(frame.width, frame.height);
        Ok(self.finish(frame, m))
    }

    fn finish(&self, frame: &Frame, mut m: Mask) -> Mask {
        if self.settings.edge_refine {
            m = refine_edges(frame, &m);
        }
        if self.settings.feather > 0.5 {
            let r = self.settings.feather.round() as usize;
            m.data = crate::frame::box_mean(&m.data, m.width, m.height, r);
        }
        m.data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
        m
    }

    /// The matte of the next frame of a clip (temporal smoothing uses the
    /// previous call's result). Call [`BackgroundRemover::reset`] at cuts
    /// and when seeking.
    pub fn next_frame(&mut self, frame: &Frame) -> Result<Mask> {
        let m = self.matte(frame)?;
        Ok(self.smoother.push(frame, m, self.settings.temporal_smoothing))
    }

    pub fn reset(&mut self) {
        self.smoother = MatteSmoother::default();
    }

    /// Mattes a whole clip, passing each frame's matte to `sink`.
    pub fn process_clip(&mut self, src: &mut dyn FrameSource, ctx: &JobCtx, mut sink: impl FnMut(usize, Mask) -> Result<()>) -> Result<()> {
        self.reset();
        let n = src.frame_count();
        for i in 0..n {
            ctx.check()?;
            ctx.set_progress(i as f32 / n.max(1) as f32);
            ctx.set_message(format!("Removing background: frame {} / {n}", i + 1));
            let f = src.frame(i)?;
            let m = self.next_frame(&f)?;
            sink(i, m)?;
        }
        Ok(())
    }
}

/// Refines a matte's transition band against the image: a guided filter
/// over the luma, only where the matte is neither clearly in nor out, so
/// solid regions keep their values.
pub fn refine_edges(frame: &Frame, m: &Mask) -> Mask {
    let (w, h) = (frame.width, frame.height);
    let luma = frame.luma();
    // Radius scales with resolution: ~4 px at 1080p.
    let r = ((w.min(h) as f32 / 270.0).round() as usize).clamp(1, 16);
    let g = guided_filter(&luma, &m.data, w, h, r, 1e-3);
    let band = crate::frame::box_mean(&m.data.iter().map(|&a| if a > 0.02 && a < 0.98 { 1.0 } else { 0.0 }).collect::<Vec<_>>(), w, h, r * 2);
    let data = m.data.iter().zip(&g).zip(&band).map(|((&a, &gv), &b)| a + (gv - a) * (b * 2.0).min(1.0)).collect();
    Mask::new(w, h, data)
}

/// Motion-adaptive temporal smoothing of mattes: an exponential moving
/// average whose weight drops where the image itself changed, so still
/// areas stop flickering while moving edges follow without lag.
#[derive(Default)]
pub struct MatteSmoother {
    prev: Option<(Vec<f32>, Mask)>,
}

impl MatteSmoother {
    /// Smooths `m` (the matte of `frame`) given the previous frame.
    pub fn push(&mut self, frame: &Frame, m: Mask, strength: f32) -> Mask {
        let luma = frame.luma();
        let strength = strength.clamp(0.0, 1.0);
        let out = match &self.prev {
            Some((pl, pm)) if pm.width == m.width && pm.height == m.height && strength > 0.0 => {
                // per-pixel image change, blurred so noise does not count as motion
                let diff: Vec<f32> = luma.iter().zip(pl).map(|(a, b)| (a - b).abs()).collect();
                let motion = crate::frame::box_mean(&diff, m.width, m.height, 2);
                let keep_max = 0.85 * strength;
                let data = m
                    .data
                    .iter()
                    .zip(&pm.data)
                    .zip(&motion)
                    .map(|((&cur, &prev), &mo)| {
                        // 0.06 luma change (≈15 levels) means "this pixel moved"
                        let keep = keep_max * (1.0 - (mo / 0.06).min(1.0));
                        prev * keep + cur * (1.0 - keep)
                    })
                    .collect();
                Mask::new(m.width, m.height, data)
            }
            _ => m,
        };
        self.prev = Some((luma, out.clone()));
        out
    }
}

/// Composites `frame` over a solid colour using `matte` (preview helper).
pub fn composite_over(frame: &Frame, matte: &Mask, bg: [u8; 3]) -> Frame {
    let mut out = frame.clone();
    for (p, &a) in out.rgba.chunks_exact_mut(4).zip(&matte.data) {
        for c in 0..3 {
            p[c] = (p[c] as f32 * a + bg[c] as f32 * (1.0 - a)).round() as u8;
        }
        p[3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoother_holds_still_areas_and_follows_motion() {
        let (w, h) = (16, 8);
        let still = Frame::filled(w, h, [100, 100, 100, 255]);
        let mut s = MatteSmoother::default();
        let first = s.push(&still, Mask::new(w, h, vec![1.0; w * h]), 1.0);
        assert_eq!(first.data[0], 1.0);
        // the model flickers to 0 on an unchanged frame: smoothing damps it
        let damped = s.push(&still, Mask::new(w, h, vec![0.0; w * h]), 1.0);
        assert!(damped.data[0] > 0.8, "{}", damped.data[0]);
        // the image changes everywhere: the new matte wins immediately
        let moved = Frame::filled(w, h, [200, 200, 200, 255]);
        let followed = s.push(&moved, Mask::new(w, h, vec![0.0; w * h]), 1.0);
        assert!(followed.data[0] < 0.05, "{}", followed.data[0]);
        // strength 0 disables smoothing
        let mut s0 = MatteSmoother::default();
        s0.push(&still, Mask::new(w, h, vec![1.0; w * h]), 0.0);
        assert_eq!(s0.push(&still, Mask::new(w, h, vec![0.0; w * h]), 0.0).data[0], 0.0);
    }

    #[test]
    fn refine_keeps_solid_regions() {
        let (w, h) = (40, 20);
        let mut f = Frame::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 20..w {
                f.rgba[(y * w + x) * 4..][..3].copy_from_slice(&[255, 255, 255]);
            }
        }
        // a blurry matte whose edge sits on the image edge
        let m = Mask::new(w, h, (0..w * h).map(|i| (((i % w) as f32 - 16.0) / 8.0).clamp(0.0, 1.0)).collect());
        let r = refine_edges(&f, &m);
        assert_eq!(r.data[5 * w + 2], 0.0);
        assert!((r.data[5 * w + 38] - 1.0).abs() < 1e-3);
        // the refined edge is steeper than the input at the image edge
        let slope = |m: &Mask| m.data[5 * w + 21] - m.data[5 * w + 18];
        assert!(slope(&r) > slope(&m), "{} vs {}", slope(&r), slope(&m));
    }

    #[test]
    fn composite_blends() {
        let f = Frame::filled(2, 1, [200, 0, 0, 255]);
        let c = composite_over(&f, &Mask::new(2, 1, vec![1.0, 0.0]), [0, 255, 0]);
        assert_eq!(&c.rgba[..4], &[200, 0, 0, 255]);
        assert_eq!(&c.rgba[4..], &[0, 255, 0, 255]);
    }

    #[test]
    fn needs_the_model() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        assert_eq!(BackgroundRemover::load(&store).err(), Some(crate::Error::ModelMissing(ModelId::Modnet)));
    }
}
