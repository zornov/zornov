// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Open-vocabulary object detection: find anything that can be named. The
//! detector takes a picture and a list of names ("a red kettle", "dog") and
//! returns a box and a score for every match, so the vocabulary is whatever
//! the caller writes plus a general default list ([`DEFAULT_VOCABULARY`]).
//!
//! OWLv2 (Minderer et al., "Scaling Open-Vocabulary Object Detection", 2023),
//! base patch 16 ensemble checkpoint, Apache-2.0. The picture is padded to a
//! square with grey on its right and bottom, resized to 960 x 960 and read as
//! 3 600 patches; every patch predicts a box and one score per name.

use crate::engine::{self, Tensor};
use crate::error::{invalid, Result};
use crate::features::bpe::ClipTokenizer;
use crate::features::faces::Rect;
use crate::frame::Frame;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

/// CLIP's channel statistics (shared by the detector and the embedder).
pub const CLIP_NORM: Norm = Norm { scale: 1.0 / 255.0, mean: [0.481_454_66, 0.457_827_5, 0.408_210_73], std: [0.268_629_54, 0.261_302_58, 0.275_777_11] };

/// The picture side the model sees.
pub const SIZE: usize = 960;
/// Text tokens per name.
const TOKENS: usize = 16;

/// One match.
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Index into the names that were asked for.
    pub name: usize,
    /// 0..1 (a sigmoid of the model's logit; 0.2 is a fair match).
    pub score: f32,
    /// In pixels of the picture that was read.
    pub bbox: Rect,
}

/// Names a general scene is searched for when the caller gives none: the
/// things footage is most often about, one noun each (near-synonyms are
/// left out so one thing gets one name).
pub const DEFAULT_VOCABULARY: &[&str] = &[
    "person", "man", "woman", "child", "face", "hand", "animal head",
    "dog", "cat", "horse", "bird", "cow", "sheep", "bear", "elephant", "giraffe", "zebra", "monkey", "deer", "rabbit", "fish", "insect",
    "car", "truck", "bus", "motorcycle", "bicycle", "boat", "airplane", "train", "traffic light", "road sign",
    "tree", "flower", "plant", "mountain", "building", "house", "bridge", "tent",
    "table", "chair", "sofa", "bed", "desk", "shelf", "door", "window", "lamp", "mirror", "curtain", "stairs",
    "laptop", "computer monitor", "television", "phone", "camera", "keyboard", "speaker", "microphone", "headphones", "clock",
    "cup", "bottle", "glass", "plate", "bowl", "pot", "knife", "fork", "spoon", "food", "fruit", "cake", "pizza", "bread",
    "book", "paper", "bag", "backpack", "suitcase", "umbrella", "hat", "shoe", "glasses", "toy", "ball", "guitar", "piano",
    "painting", "picture frame", "sign", "flag", "poster", "box", "logo", "screen", "sculpture", "balloon",
];

pub struct OpenVocabDetector {
    session: Arc<Session>,
    tok: ClipTokenizer,
}

impl OpenVocabDetector {
    pub fn load(store: &ModelStore) -> Result<OpenVocabDetector> {
        let dir = store.require(ModelId::OwlV2Base)?;
        Ok(OpenVocabDetector { session: engine::session(&dir.join("model_fp16.onnx"))?, tok: ClipTokenizer::parse(&std::fs::read_to_string(dir.join("tokenizer.json"))?)? })
    }

    /// Everything in `frame` that matches one of `names` with at least
    /// `threshold` score, best first. Boxes of different names that cover
    /// the same thing are all returned; use [`merge`] to keep one name.
    pub fn detect(&self, frame: &Frame, names: &[String], threshold: f32) -> Result<Vec<Detection>> {
        if names.is_empty() {
            return invalid("no names to look for");
        }
        let (px, side) = pad_square(frame);
        let input = studio_nn::image::to_nchw(studio_nn::image::Rgba { w: side, h: side, px: &px }, SIZE, SIZE, CLIP_NORM);
        let (mut ids, mut mask) = (Vec::new(), Vec::new());
        for n in names {
            let (i, m) = self.tok.encode(&format!("a photo of a {n}"), TOKENS);
            ids.extend(i);
            mask.extend(m);
        }
        let q = names.len();
        let mut o = engine::run(&self.session, vec![("input_ids", Tensor::i64(&[q, TOKENS], ids)), ("attention_mask", Tensor::i64(&[q, TOKENS], mask)), ("pixel_values", input)])?;
        let logits = o.take("logits")?.into_f32_vec();
        let boxes = o.take("pred_boxes")?.into_f32_vec();
        let patches = boxes.len() / 4;
        if logits.len() != patches * q {
            return invalid(format!("detector returned {} scores for {patches} patches and {q} names", logits.len()));
        }
        let side = side as f32;
        let mut out = Vec::new();
        for p in 0..patches {
            let (cx, cy, w, h) = (boxes[p * 4], boxes[p * 4 + 1], boxes[p * 4 + 2], boxes[p * 4 + 3]);
            let row = &logits[p * q..(p + 1) * q];
            // Only the best name per patch: a patch is one thing.
            let (name, &best) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).expect("at least one name");
            let score = 1.0 / (1.0 + (-best).exp());
            if score < threshold {
                continue;
            }
            let r = Rect { x: (cx - w / 2.0) * side, y: (cy - h / 2.0) * side, w: w * side, h: h * side };
            let r = clip(r, frame.width as f32, frame.height as f32);
            if r.w >= 2.0 && r.h >= 2.0 {
                out.push(Detection { name, score, bbox: r });
            }
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(out)
    }
}

/// The picture on a square canvas (grey padding right and below), and its
/// side.
fn pad_square(f: &Frame) -> (Vec<u8>, usize) {
    let side = f.width.max(f.height);
    if f.width == f.height {
        return (f.rgba.clone(), side);
    }
    let mut px = vec![128u8; side * side * 4];
    for p in px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    for y in 0..f.height {
        px[y * side * 4..y * side * 4 + f.width * 4].copy_from_slice(&f.rgba[y * f.width * 4..(y + 1) * f.width * 4]);
    }
    (px, side)
}

fn clip(r: Rect, w: f32, h: f32) -> Rect {
    let (x0, y0) = (r.x.max(0.0), r.y.max(0.0));
    let (x1, y1) = ((r.x + r.w).min(w), (r.y + r.h).min(h));
    Rect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
}

/// Non-maximum suppression across all names: of boxes that overlap by more
/// than `iou`, only the best-scoring one stays. A box almost inside a better
/// one of the same name is dropped too (a part of the same thing).
pub fn merge(dets: Vec<Detection>, iou: f32) -> Vec<Detection> {
    let mut dets = dets;
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        let dup = kept.iter().any(|k| {
            let i = k.bbox.iou(&d.bbox);
            let inside = inter(&k.bbox, &d.bbox) / d.bbox.area().max(1e-6);
            i > iou || (k.name == d.name && inside > 0.85 && d.bbox.area() < k.bbox.area() * 0.7)
        });
        if !dup {
            kept.push(d);
        }
    }
    kept
}

fn inter(a: &Rect, b: &Rect) -> f32 {
    let ix = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let iy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    ix.max(0.0) * iy.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: usize, score: f32, x: f32, y: f32, w: f32, h: f32) -> Detection {
        Detection { name, score, bbox: Rect { x, y, w, h } }
    }

    #[test]
    fn merge_keeps_the_best_name_for_one_thing() {
        let out = merge(vec![d(0, 0.4, 10.0, 10.0, 100.0, 100.0), d(1, 0.6, 12.0, 8.0, 100.0, 104.0), d(2, 0.3, 300.0, 300.0, 50.0, 50.0)], 0.6);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, 1);
        assert_eq!(out[1].name, 2);
    }

    #[test]
    fn merge_drops_a_part_of_the_same_name() {
        let out = merge(vec![d(0, 0.8, 0.0, 0.0, 200.0, 200.0), d(0, 0.5, 20.0, 20.0, 60.0, 60.0), d(1, 0.5, 20.0, 20.0, 60.0, 60.0)], 0.6);
        // The small "0" is a part of the big "0"; the small "1" is another name inside it and stays.
        assert_eq!(out.iter().map(|x| x.name).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn padding_puts_the_picture_top_left() {
        let f = Frame::filled(4, 2, [10, 20, 30, 255]);
        let (px, side) = pad_square(&f);
        assert_eq!(side, 4);
        assert_eq!(&px[..4], &[10, 20, 30, 255]);
        assert_eq!(&px[2 * 4 * 4..2 * 4 * 4 + 4], &[128, 128, 128, 255]);
    }

    #[test]
    fn the_default_vocabulary_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for n in DEFAULT_VOCABULARY {
            assert!(seen.insert(*n), "{n} twice");
            assert!(n.split_whitespace().count() <= 3);
        }
        assert!(DEFAULT_VOCABULARY.len() < 128);
        assert!(DEFAULT_VOCABULARY.contains(&"person") && DEFAULT_VOCABULARY.contains(&"bear"));
    }
}
