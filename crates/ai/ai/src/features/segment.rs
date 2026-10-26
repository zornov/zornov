// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Object cut-out: the user clicks on an object (or drags a box around it)
//! and SAM 2.1 returns its mask; the mask is then carried through the clip.
//!
//! The image encoder runs once per frame ([`ObjectCutout::embed`]); prompts
//! are cheap to re-run on the same embedding, so refining a selection with
//! extra clicks is interactive.
//!
//! Propagation through a clip re-prompts every frame with a box around the
//! previous frame's mask (grown by a margin) plus a positive point at its
//! most interior pixel. That follows objects that move a moderate amount
//! per frame; when the model reports the object gone, tracking pauses and
//! resumes when it reappears near the last known place.
//!
//! SAM 2: Ravi et al., "SAM 2: Segment Anything in Images and Videos"
//! (2024). Weights: Apache-2.0.

use crate::engine::{self, Tensor};
use crate::error::{invalid, Result};
use crate::frame::{Frame, FrameSource, Mask};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

/// A prompt in frame pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prompt {
    /// A click: `positive` = "this is the object", otherwise "not this".
    Point { x: f32, y: f32, positive: bool },
    /// A box around the object.
    Box { x0: f32, y0: f32, x1: f32, y1: f32 },
}

/// The encoded image (reuse it for several prompt sets on the same frame).
pub struct ImageEmbedding {
    width: usize,
    height: usize,
    feats: [Tensor; 3],
}

/// A mask with the model's confidence.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub mask: Mask,
    /// Predicted IoU of the mask (0..1).
    pub score: f32,
    /// False when the model thinks the object is not in the frame.
    pub present: bool,
}

const SIZE: usize = 1024;
/// Label for padding points (ignored by the prompt encoder).
const PAD_LABEL: i64 = -10;

pub struct ObjectCutout {
    enc: Arc<Session>,
    dec: Arc<Session>,
}

impl ObjectCutout {
    pub fn load(store: &ModelStore) -> Result<ObjectCutout> {
        let dir = store.require(ModelId::Sam21Tiny)?;
        Ok(ObjectCutout { enc: engine::session(&dir.join("vision_encoder.onnx"))?, dec: engine::session(&dir.join("prompt_encoder_mask_decoder.onnx"))? })
    }

    /// Runs the image encoder.
    pub fn embed(&self, frame: &Frame) -> Result<ImageEmbedding> {
        let input = studio_nn::image::to_nchw(frame.view(), SIZE, SIZE, Norm::IMAGENET);
        let mut o = engine::run(&self.enc, vec![("pixel_values", input)])?;
        Ok(ImageEmbedding { width: frame.width, height: frame.height, feats: [o.take("image_embeddings.0")?, o.take("image_embeddings.1")?, o.take("image_embeddings.2")?] })
    }

    /// The mask for `prompts` on an embedded frame.
    pub fn segment(&self, emb: &ImageEmbedding, prompts: &[Prompt]) -> Result<Selection> {
        if prompts.is_empty() {
            return invalid("no prompt");
        }
        let (sx, sy) = (SIZE as f32 / emb.width as f32, SIZE as f32 / emb.height as f32);
        let mut pts: Vec<f32> = Vec::new();
        let mut labels: Vec<i64> = Vec::new();
        let mut boxes: Vec<f32> = Vec::new();
        for p in prompts {
            match *p {
                Prompt::Point { x, y, positive } => {
                    pts.extend([x * sx, y * sy]);
                    labels.push(positive as i64);
                }
                Prompt::Box { x0, y0, x1, y1 } if boxes.is_empty() => boxes.extend([x0.min(x1) * sx, y0.min(y1) * sy, x0.max(x1) * sx, y0.max(y1) * sy]),
                Prompt::Box { .. } => return invalid("only one box per selection"),
            }
        }
        if labels.is_empty() {
            // The decoder always takes points; a padding point adds nothing.
            pts.extend([0.0, 0.0]);
            labels.push(PAD_LABEL);
        }
        let n = labels.len();
        let nb = boxes.len() / 4;
        let mut inputs = vec![
            ("input_points", Tensor::f32(&[1, 1, n, 2], pts)),
            ("input_labels", Tensor::i64(&[1, 1, n], labels)),
            ("input_boxes", Tensor::f32(&[1, nb, 4], boxes)),
        ];
        let [e0, e1, e2] = &emb.feats;
        inputs.push(("image_embeddings.0", e0.clone()));
        inputs.push(("image_embeddings.1", e1.clone()));
        inputs.push(("image_embeddings.2", e2.clone()));
        let mut o = engine::run(&self.dec, inputs)?;
        let iou = o.take("iou_scores")?.into_f32_vec();
        let masks = o.take("pred_masks")?;
        let obj = o.take("object_score_logits")?.into_f32_vec();
        let (nm, mh, mw) = (masks.shape[2], masks.shape[3], masks.shape[4]);
        // This export returns SAM's three alternative masks (part / object /
        // whole) without the single-mask output; take the one the model
        // rates best.
        let k = (0..nm).max_by(|&a, &b| iou[a].total_cmp(&iou[b])).unwrap_or(0);
        let logits = masks.f32s();
        let plane = &logits[k * mh * mw..(k + 1) * mh * mw];
        let up = studio_nn::image::resize_plane(plane, mw, mh, emb.width, emb.height);
        // Logits → a soft edge one or two pixels wide instead of a hard step.
        let data = up.into_iter().map(|l| 1.0 / (1.0 + (-l * 4.0).exp())).collect();
        Ok(Selection { mask: Mask::new(emb.width, emb.height, data), score: iou[k].clamp(0.0, 1.0), present: obj.first().is_none_or(|&s| s > 0.0) })
    }

    /// Embeds and segments in one go.
    pub fn cut_out(&self, frame: &Frame, prompts: &[Prompt]) -> Result<Selection> {
        let emb = self.embed(frame)?;
        self.segment(&emb, prompts)
    }

    /// Carries a selection made on frame `start` through the rest of the
    /// clip (forwards) and back to its beginning (backwards). `sink`
    /// receives every frame's selection, in processing order.
    pub fn track_clip(&self, src: &mut dyn FrameSource, start: usize, prompts: &[Prompt], ctx: &JobCtx, mut sink: impl FnMut(usize, &Selection) -> Result<()>) -> Result<()> {
        let n = src.frame_count();
        if start >= n {
            return invalid("start frame out of range");
        }
        let first = self.cut_out(&src.frame(start)?, prompts)?;
        sink(start, &first)?;
        let mut done = 1usize;
        for dir in [1isize, -1] {
            let mut tracker = Tracker::new(&first);
            let mut i = start as isize + dir;
            while i >= 0 && (i as usize) < n {
                ctx.check()?;
                ctx.set_progress(done as f32 / n as f32);
                ctx.set_message(format!("Tracking object: frame {} / {n}", done + 1));
                let f = src.frame(i as usize)?;
                let sel = match tracker.prompts() {
                    Some(p) => {
                        let s = self.cut_out(&f, &p)?;
                        tracker.update(&s);
                        s
                    }
                    None => Selection { mask: Mask::zeros(f.width, f.height), score: 0.0, present: false },
                };
                sink(i as usize, &sel)?;
                done += 1;
                i += dir;
            }
        }
        Ok(())
    }
}

/// Builds the next frame's prompts from the last good mask.
pub struct Tracker {
    last_box: Option<(f32, f32, f32, f32)>,
    point: Option<(f32, f32)>,
    size: (usize, usize),
}

impl Tracker {
    pub fn new(sel: &Selection) -> Tracker {
        let mut t = Tracker { last_box: None, point: None, size: (sel.mask.width, sel.mask.height) };
        t.update(sel);
        t
    }

    /// Remembers a frame's result (lost frames keep the last good box).
    pub fn update(&mut self, sel: &Selection) {
        if !sel.present || sel.mask.coverage() < 1e-5 {
            return;
        }
        if let Some((x0, y0, x1, y1)) = sel.mask.bbox() {
            self.last_box = Some((x0 as f32, y0 as f32, x1 as f32, y1 as f32));
            self.point = interior_point(&sel.mask);
        }
    }

    /// Box grown by 15% of its size (min 8 px) per side, clamped to the
    /// frame, plus the interior point.
    pub fn prompts(&self) -> Option<Vec<Prompt>> {
        let (x0, y0, x1, y1) = self.last_box?;
        let (mx, my) = (((x1 - x0) * 0.15).max(8.0), ((y1 - y0) * 0.15).max(8.0));
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let mut p = vec![Prompt::Box { x0: (x0 - mx).max(0.0), y0: (y0 - my).max(0.0), x1: (x1 + mx).min(w), y1: (y1 + my).min(h) }];
        if let Some((x, y)) = self.point {
            p.push(Prompt::Point { x, y, positive: true });
        }
        Some(p)
    }
}

/// The mask pixel farthest from the mask's border (two-pass chamfer
/// distance on a downscaled copy), a robust "inside the object" click.
pub fn interior_point(m: &Mask) -> Option<(f32, f32)> {
    let scale = (m.width.max(m.height) as f32 / 256.0).max(1.0);
    let (w, h) = (((m.width as f32 / scale) as usize).max(1), ((m.height as f32 / scale) as usize).max(1));
    let small = m.resized(w, h);
    let big = (w + h) as f32;
    let mut d: Vec<f32> = small.data.iter().map(|&v| if v > 0.5 { big } else { 0.0 }).collect();
    let at = |d: &Vec<f32>, x: isize, y: isize| if x < 0 || y < 0 || x >= w as isize || y >= h as isize { 0.0 } else { d[y as usize * w + x as usize] };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            if d[i] > 0.0 {
                d[i] = d[i].min(at(&d, x - 1, y) + 1.0).min(at(&d, x, y - 1) + 1.0).min(at(&d, x - 1, y - 1) + 1.4).min(at(&d, x + 1, y - 1) + 1.4);
            }
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let i = y as usize * w + x as usize;
            if d[i] > 0.0 {
                d[i] = d[i].min(at(&d, x + 1, y) + 1.0).min(at(&d, x, y + 1) + 1.0).min(at(&d, x + 1, y + 1) + 1.4).min(at(&d, x - 1, y + 1) + 1.4);
            }
        }
    }
    let (i, &best) = d.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    (best > 0.0).then(|| (((i % w) as f32 + 0.5) * m.width as f32 / w as f32, ((i / w) as f32 + 0.5) * m.height as f32 / h as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_mask() -> Mask {
        // a thick "C": the centroid falls outside the object, the interior
        // point must not
        let (w, h) = (60, 60);
        let mut m = Mask::zeros(w, h);
        for y in 0..h {
            for x in 0..w {
                let r = ((x as f32 - 30.0).powi(2) + (y as f32 - 30.0).powi(2)).sqrt();
                if (15.0..28.0).contains(&r) && x < 45 {
                    m.data[y * w + x] = 1.0;
                }
            }
        }
        m
    }

    #[test]
    fn interior_point_is_inside() {
        let m = ring_mask();
        let (x, y) = interior_point(&m).unwrap();
        assert!(m.data[y as usize * m.width + x as usize] > 0.5, "({x},{y})");
        let (cx, cy) = m.centroid().unwrap();
        assert!(m.data[cy as usize * m.width + cx as usize] < 0.5, "test premise: centroid is outside");
        assert_eq!(interior_point(&Mask::zeros(5, 5)), None);
    }

    #[test]
    fn tracker_grows_box_and_survives_loss() {
        let m = ring_mask();
        let sel = Selection { mask: m.clone(), score: 0.9, present: true };
        let mut t = Tracker::new(&sel);
        let p = t.prompts().unwrap();
        let Prompt::Box { x0, y0, x1, y1 } = p[0] else { panic!() };
        let (bx0, by0, bx1, by1) = m.bbox().unwrap();
        assert!(x0 < bx0 as f32 && y0 < by0 as f32 && x1 > bx1 as f32 && y1 > by1 as f32);
        assert!(x0 >= 0.0 && y1 <= 60.0);
        assert!(matches!(p[1], Prompt::Point { positive: true, .. }));
        t.update(&Selection { mask: Mask::zeros(60, 60), score: 0.0, present: false });
        assert_eq!(t.prompts().unwrap(), p, "a lost frame keeps the last box");
    }

    #[test]
    fn needs_the_model() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        assert!(matches!(ObjectCutout::load(&store), Err(crate::Error::ModelMissing(ModelId::Sam21Tiny))));
    }
}
