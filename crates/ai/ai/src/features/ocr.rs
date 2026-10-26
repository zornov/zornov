// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text in the picture: signs, labels, screens, lettering. A detector finds
//! the lines of text as a probability map (differentiable binarisation, Liao
//! et al. 2020) and a recogniser reads each line with a connected-temporal-
//! classification head (Shi et al. 2016).
//!
//! PaddleOCR PP-OCRv3 detector and PP-OCRv5 Latin recogniser, Apache-2.0.
//! Latin script only (English and about thirty other languages); lines are
//! read left to right, so rotated text is found but read poorly.

use crate::engine;
use crate::error::{invalid, Error, Result};
use crate::features::faces::Rect;
use crate::frame::Frame;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

/// One line of text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub text: String,
    /// Mean character confidence, 0..1.
    pub score: f32,
    pub bbox: Rect,
}

pub struct TextReader {
    det: Arc<Session>,
    rec: Arc<Session>,
    dict: Vec<String>,
}

/// Longest side the detector reads.
const DET_SIDE: usize = 960;
/// Height the recogniser reads a line at.
const REC_HEIGHT: usize = 48;
const REC_MAX_WIDTH: usize = 1280;

impl TextReader {
    pub fn load(store: &ModelStore) -> Result<TextReader> {
        let dir = store.require(ModelId::PaddleOcrLatin)?;
        Self::load_dir(&dir)
    }

    pub fn load_dir(dir: &std::path::Path) -> Result<TextReader> {
        let dict: Vec<String> = std::fs::read_to_string(dir.join("dict.txt"))?.lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).collect();
        if dict.len() < 90 {
            return Err(Error::Inference("dict.txt: too few characters".into()));
        }
        Ok(TextReader { det: engine::session(&dir.join("det.onnx"))?, rec: engine::session(&dir.join("rec.onnx"))?, dict })
    }

    /// Every line of text in the picture, top to bottom.
    pub fn read(&self, frame: &Frame) -> Result<Vec<TextLine>> {
        let boxes = self.find(frame)?;
        let mut out = Vec::new();
        for b in boxes {
            if let Some((text, score)) = self.recognise(frame, &b)?
                && score >= 0.5
                && !text.trim().is_empty()
            {
                out.push(TextLine { text, score, bbox: b });
            }
        }
        sort_reading_order(&mut out);
        Ok(out)
    }

    /// Where the lines of text are (pixels of `frame`).
    pub fn find(&self, frame: &Frame) -> Result<Vec<Rect>> {
        let (w, h) = det_size(frame.width, frame.height);
        let swapped = swap_rb(&frame.rgba);
        let input = studio_nn::image::to_nchw(studio_nn::image::Rgba { w: frame.width, h: frame.height, px: &swapped }, w, h, Norm::IMAGENET);
        let inp = self.det.input_names().first().map(|s| s.to_string()).unwrap_or_else(|| "x".into());
        let mut o = engine::run(&self.det, vec![(inp.as_str(), input)])?;
        let name = o.first_name().ok_or_else(|| Error::Inference("text detector returned nothing".into()))?;
        let t = o.take(&name)?;
        let prob = t.into_f32_vec();
        if prob.len() != w * h {
            return invalid(format!("text detector returned {} values for {w}x{h}", prob.len()));
        }
        let (sx, sy) = (frame.width as f32 / w as f32, frame.height as f32 / h as f32);
        Ok(boxes_from_probability(&prob, w, h, 0.3, 0.6, 1.6).into_iter().map(|r| Rect { x: r.x * sx, y: r.y * sy, w: r.w * sx, h: r.h * sy }).collect())
    }

    fn recognise(&self, frame: &Frame, b: &Rect) -> Result<Option<(String, f32)>> {
        let (x, y) = (b.x.floor().max(0.0) as usize, b.y.floor().max(0.0) as usize);
        let (w, h) = ((b.w.ceil() as usize).min(frame.width.saturating_sub(x)), (b.h.ceil() as usize).min(frame.height.saturating_sub(y)));
        if w < 4 || h < 4 {
            return Ok(None);
        }
        let crop = swap_rb(&studio_nn::image::crop_rgba(frame.view(), x, y, w, h));
        let ow = ((REC_HEIGHT as f32 * w as f32 / h as f32).ceil() as usize).clamp(16, REC_MAX_WIDTH);
        let input = studio_nn::image::to_nchw(studio_nn::image::Rgba { w, h, px: &crop }, ow, REC_HEIGHT, Norm::SIGNED);
        let inp = self.rec.input_names().first().map(|s| s.to_string()).unwrap_or_else(|| "x".into());
        let mut o = engine::run(&self.rec, vec![(inp.as_str(), input)])?;
        let name = o.first_name().ok_or_else(|| Error::Inference("text recogniser returned nothing".into()))?;
        let t = o.take(&name)?;
        let classes = *t.shape.last().unwrap_or(&0);
        if classes != self.dict.len() + 2 && classes != self.dict.len() + 1 {
            return invalid(format!("recogniser has {classes} classes for a dictionary of {}", self.dict.len()));
        }
        Ok(Some(ctc_decode(&t.into_f32_vec(), classes, &self.dict)))
    }
}

fn det_size(w: usize, h: usize) -> (usize, usize) {
    let k = (DET_SIDE as f32 / w.max(h) as f32).min(1.0);
    let r32 = |v: f32| (((v / 32.0).round() as usize).max(1)) * 32;
    (r32(w as f32 * k), r32(h as f32 * k))
}

fn swap_rb(rgba: &[u8]) -> Vec<u8> {
    let mut v = rgba.to_vec();
    for p in v.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    v
}

fn sort_reading_order(lines: &mut [TextLine]) {
    lines.sort_by(|a, b| {
        let (ca, cb) = (a.bbox.centre(), b.bbox.centre());
        let same_row = (ca.1 - cb.1).abs() < a.bbox.h.min(b.bbox.h) * 0.5;
        if same_row { ca.0.total_cmp(&cb.0) } else { ca.1.total_cmp(&cb.1) }
    });
}

/// Connected regions of a probability map above `thresh`, as boxes grown
/// the way the binarisation shrank them. Regions whose mean probability is
/// below `min_score` or that are tiny are dropped.
pub fn boxes_from_probability(prob: &[f32], w: usize, h: usize, thresh: f32, min_score: f32, unclip: f32) -> Vec<Rect> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || prob[start] <= thresh {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let (mut n, mut sum) = (0usize, 0.0f32);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            n += 1;
            sum += prob[i];
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if !seen[j] && prob[j] > thresh {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        if n < 12 || bw.min(bh) < 3.0 || (sum / n as f32) < min_score {
            continue;
        }
        // DB's unclip distance: area * ratio / perimeter, on the box.
        let d = bw * bh * unclip / (2.0 * (bw + bh));
        let (rx0, ry0) = ((x0 as f32 - d).max(0.0), (y0 as f32 - d).max(0.0));
        let (rx1, ry1) = ((x1 as f32 + 1.0 + d).min(w as f32), (y1 as f32 + 1.0 + d).min(h as f32));
        out.push(Rect { x: rx0, y: ry0, w: rx1 - rx0, h: ry1 - ry0 });
    }
    out
}

/// Greedy CTC decoding of `[steps, classes]` scores. Class 0 is the blank,
/// classes 1.. are the dictionary, and one more (if present) is the space.
/// Returns the text and the mean confidence of the characters.
pub fn ctc_decode(scores: &[f32], classes: usize, dict: &[String]) -> (String, f32) {
    let steps = scores.len() / classes.max(1);
    let (mut text, mut conf, mut n) = (String::new(), 0.0f32, 0usize);
    let mut last = 0usize;
    for s in 0..steps {
        let row = &scores[s * classes..(s + 1) * classes];
        let (best, &v) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).expect("classes");
        // The recogniser ends in a softmax; if it does not, make it one.
        let p = if (0.0..=1.0).contains(&v) && (row.iter().sum::<f32>() - 1.0).abs() < 0.05 {
            v
        } else {
            let m = v;
            1.0 / row.iter().map(|x| (x - m).exp()).sum::<f32>()
        };
        if best != 0 && best != last {
            match dict.get(best - 1) {
                Some(c) => text.push_str(c),
                None => text.push(' '),
            }
            conf += p;
            n += 1;
        }
        last = best;
    }
    (text, if n > 0 { conf / n as f32 } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_become_grown_boxes() {
        let (w, h) = (40, 20);
        let mut p = vec![0.0f32; w * h];
        for y in 8..12 {
            for x in 5..30 {
                p[y * w + x] = 0.9;
            }
        }
        // A speck and a weak patch are dropped.
        p[1] = 0.9;
        for y in 15..18 {
            for x in 2..10 {
                p[y * w + x] = 0.4;
            }
        }
        let b = boxes_from_probability(&p, w, h, 0.3, 0.6, 1.6);
        assert_eq!(b.len(), 1);
        assert!(b[0].x < 5.0 && b[0].x + b[0].w > 30.0 && b[0].y < 8.0 && b[0].y + b[0].h > 12.0, "{:?}", b[0]);
    }

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        let dict: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        // classes: blank, a, b, c, space
        let rows: [[f32; 5]; 6] = [[0.0, 0.9, 0.1, 0.0, 0.0], [0.0, 0.9, 0.1, 0.0, 0.0], [0.9, 0.1, 0.0, 0.0, 0.0], [0.0, 0.9, 0.1, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0, 0.0]];
        let flat: Vec<f32> = rows.iter().flatten().copied().collect();
        let (t, c) = ctc_decode(&flat, 5, &dict);
        assert_eq!(t, "aa c");
        assert!(c > 0.9, "{c}");
    }

    #[test]
    fn detector_size_is_a_multiple_of_32() {
        assert_eq!(det_size(1920, 1080), (960, 544));
        assert_eq!(det_size(100, 40), (96, 32));
    }

    #[test]
    fn lines_read_in_order() {
        let l = |t: &str, x: f32, y: f32| TextLine { text: t.into(), score: 1.0, bbox: Rect { x, y, w: 50.0, h: 20.0 } };
        let mut v = vec![l("c", 10.0, 100.0), l("b", 200.0, 52.0), l("a", 10.0, 50.0)];
        sort_reading_order(&mut v);
        assert_eq!(v.iter().map(|x| x.text.as_str()).collect::<String>(), "abc");
    }
}
