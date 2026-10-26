// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pictures and sentences in one space: "find the moment where a woman
//! pours coffee" is a nearest-neighbour search between the sentence's
//! embedding and the embeddings of the frames, and a crop of an object is
//! named by the sentence it lies closest to.
//!
//! SigLIP base, patch 16, 224 pixels (Zhai et al., "Sigmoid Loss for
//! Language Image Pre-Training", 2023), Apache-2.0. The whole picture is
//! read squashed to 224 x 224, which is how the model was trained.

use crate::engine::{self, Tensor};
use crate::error::{invalid, Result};
use crate::features::faces::Rect;
use crate::features::unigram::UnigramTokenizer;
use crate::frame::Frame;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

/// Length of an embedding.
pub const DIM: usize = 768;
const SIDE: usize = 224;
/// Text tokens the model reads (always this many, padded).
const TOKENS: usize = 64;

/// The embedding of one frame: a unit vector.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameEmbedding {
    pub views: Vec<[f32; DIM]>,
}

impl FrameEmbedding {
    /// Best cosine similarity of any view with a text embedding.
    pub fn similarity(&self, text: &[f32; DIM]) -> f32 {
        self.views.iter().map(|v| dot(v, text)).fold(f32::MIN, f32::max)
    }
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Scales to unit length (a zero vector stays zero).
pub fn normalised(mut v: [f32; DIM]) -> [f32; DIM] {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-12 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// How strongly a cosine similarity says "this is a match": with this
/// model a caption of the picture scores about 0.08 to 0.20, an unrelated
/// one between -0.15 and 0.04.
pub fn match_strength(similarity: f32) -> &'static str {
    if similarity >= 0.10 {
        "strong"
    } else if similarity >= 0.055 {
        "likely"
    } else {
        "weak"
    }
}

pub struct PictureTextEmbedder {
    vision: Arc<Session>,
    text: Arc<Session>,
    tok: UnigramTokenizer,
}

impl PictureTextEmbedder {
    pub fn load(store: &ModelStore) -> Result<PictureTextEmbedder> {
        let dir = store.require(ModelId::SiglipBase224)?;
        Self::load_dir(&dir)
    }

    pub fn load_dir(dir: &std::path::Path) -> Result<PictureTextEmbedder> {
        Ok(PictureTextEmbedder {
            vision: engine::session(&dir.join("vision_model.onnx"))?,
            text: engine::session(&dir.join("text_model.onnx"))?,
            tok: UnigramTokenizer::parse(&std::fs::read_to_string(dir.join("tokenizer.json"))?)?,
        })
    }

    /// Unit-length embeddings of sentences.
    pub fn embed_texts(&self, texts: &[String]) -> Result<Vec<[f32; DIM]>> {
        let mut out = Vec::new();
        for chunk in texts.chunks(32) {
            let ids: Vec<i64> = chunk.iter().flat_map(|t| self.tok.encode(t, TOKENS)).collect();
            let mut o = engine::run(&self.text, vec![("input_ids", Tensor::i64(&[chunk.len(), TOKENS], ids))])?;
            let v = o.take("pooler_output")?.into_f32_vec();
            if v.len() != chunk.len() * DIM {
                return invalid("text embedding has the wrong size");
            }
            out.extend(v.chunks_exact(DIM).map(|c| normalised(c.try_into().expect("768"))));
        }
        Ok(out)
    }

    /// The embedding of a whole picture.
    pub fn embed_frame(&self, frame: &Frame) -> Result<FrameEmbedding> {
        let t = studio_nn::image::to_nchw(frame.view(), SIDE, SIDE, Norm::SIGNED);
        self.run_images(t.into_f32_vec(), 1)
    }

    /// The embedding of the part of the frame in `r` (a square around it,
    /// with a margin, so the thing is seen with its surroundings).
    pub fn embed_crop(&self, frame: &Frame, r: &Rect) -> Result<[f32; DIM]> {
        let sq = square_around(r, frame.width, frame.height, 1.15);
        let (x, y, s) = (sq.x.round().max(0.0) as usize, sq.y.round().max(0.0) as usize, (sq.w.round() as usize).max(1));
        let (x, y) = (x.min(frame.width - 1), y.min(frame.height - 1));
        let (w, h) = (s.min(frame.width - x), s.min(frame.height - y));
        let crop = studio_nn::image::crop_rgba(frame.view(), x, y, w, h);
        let t = studio_nn::image::to_nchw(studio_nn::image::Rgba { w, h, px: &crop }, SIDE, SIDE, Norm::SIGNED);
        Ok(self.run_images(t.into_f32_vec(), 1)?.views[0])
    }

    fn run_images(&self, data: Vec<f32>, n: usize) -> Result<FrameEmbedding> {
        let mut o = engine::run(&self.vision, vec![("pixel_values", Tensor::f32(&[n, 3, SIDE, SIDE], data))])?;
        let v = o.take("pooler_output")?.into_f32_vec();
        if v.len() != n * DIM {
            return invalid("image embedding has the wrong size");
        }
        Ok(FrameEmbedding { views: v.chunks_exact(DIM).map(|c| normalised(c.try_into().expect("768"))).collect() })
    }
}

/// A square around `r` grown by `margin`, moved inside the picture.
pub fn square_around(r: &Rect, w: usize, h: usize, margin: f32) -> Rect {
    let (wf, hf) = (w as f32, h as f32);
    let s = (r.w.max(r.h) * margin).max(16.0).min(wf.min(hf));
    let (cx, cy) = r.centre();
    Rect { x: (cx - s / 2.0).clamp(0.0, wf - s), y: (cy - s / 2.0).clamp(0.0, hf - s), w: s, h: s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_square_stays_inside_the_picture() {
        let r = Rect { x: 1800.0, y: 1000.0, w: 100.0, h: 60.0 };
        let s = square_around(&r, 1920, 1080, 1.2);
        assert!(s.x >= 0.0 && s.y >= 0.0 && s.x + s.w <= 1920.0 && s.y + s.h <= 1080.0);
        assert!(s.w >= 120.0 - 1e-3);
        let tiny = square_around(&Rect { x: 5.0, y: 5.0, w: 2.0, h: 2.0 }, 100, 50, 1.0);
        assert!(tiny.w >= 16.0 && tiny.x >= 0.0);
    }

    #[test]
    fn normalising_gives_unit_length() {
        let mut v = [0.0f32; DIM];
        v[0] = 3.0;
        v[1] = 4.0;
        let n = normalised(v);
        assert!((dot(&n, &n) - 1.0).abs() < 1e-6);
        assert_eq!(normalised([0.0; DIM]), [0.0; DIM]);
    }

    #[test]
    fn strengths_have_words() {
        assert_eq!(match_strength(0.2), "strong");
        assert_eq!(match_strength(0.07), "likely");
        assert_eq!(match_strength(0.02), "weak");
    }
}
