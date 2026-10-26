// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Face detection and tracking (for face-following crops and auto reframe),
//! and body pose landmarks.
//!
//! Faces: BlazeFace (short range), a single-shot detector over a fixed set
//! of anchor boxes; each anchor regresses a box and six keypoints (eyes,
//! nose, mouth, ears) and a score; overlapping hits are merged by weighted
//! non-maximum suppression. Bazarevsky et al., "BlazeFace: Sub-millisecond
//! Neural Face Detection on Mobile GPUs" (2019).
//!
//! Pose: BlazePose. A detector finds a person and two alignment points (hip
//! centre and a point above the head) which define a rotated square around
//! the body; a landmark model then predicts 33 body points inside it. On
//! video the next frame's square comes from the previous landmarks, so the
//! detector only runs when the person is lost. Bazarevsky et al., "BlazePose:
//! On-device Real-time Body Pose tracking" (2020).
//!
//! Both models: Apache-2.0 (MediaPipe).

use crate::engine::{self, Tensor};
use crate::error::{invalid, Result};
use crate::frame::{Frame, Roi};
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::Session;

/// An axis-aligned box in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }
    pub fn iou(&self, o: &Rect) -> f32 {
        let ix = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let iy = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        let i = ix.max(0.0) * iy.max(0.0);
        let u = self.area() + o.area() - i;
        if u > 0.0 { i / u } else { 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub bbox: Rect,
    pub score: f32,
    /// Right eye, left eye, nose tip, mouth centre, right ear, left ear
    /// (the subject's right/left), in pixels.
    pub keypoints: [(f32, f32); 6],
}

// ------------------------------------------------------------ SSD helpers

/// Anchor centres (normalised) for a MediaPipe-style SSD with fixed-size
/// anchors: consecutive layers with the same stride share one grid, two
/// anchors per layer per cell.
fn ssd_anchors(input: usize, strides: &[usize]) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < strides.len() {
        let s = strides[i];
        let mut layers = 0;
        while i < strides.len() && strides[i] == s {
            layers += 1;
            i += 1;
        }
        let g = input.div_ceil(s);
        for y in 0..g {
            for x in 0..g {
                for _ in 0..2 * layers {
                    out.push(((x as f32 + 0.5) / g as f32, (y as f32 + 0.5) / g as f32));
                }
            }
        }
    }
    out
}

/// A raw detection in normalised input coordinates.
#[derive(Clone, Debug)]
struct Det {
    score: f32,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    kps: Vec<(f32, f32)>,
}

impl Det {
    fn rect(&self) -> Rect {
        Rect { x: self.cx - self.w / 2.0, y: self.cy - self.h / 2.0, w: self.w, h: self.h }
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-100.0, 100.0)).exp())
}

/// Decodes SSD regressors (`[n, coords]`) against anchors.
fn decode(reg: &[f32], cls: &[f32], anchors: &[(f32, f32)], coords: usize, n_kps: usize, scale: f32, thresh: f32) -> Vec<Det> {
    let mut out = Vec::new();
    for (i, &(ax, ay)) in anchors.iter().enumerate() {
        let score = sigmoid(cls[i]);
        if score < thresh {
            continue;
        }
        let r = &reg[i * coords..(i + 1) * coords];
        let kps = (0..n_kps).map(|k| (r[4 + 2 * k] / scale + ax, r[5 + 2 * k] / scale + ay)).collect();
        out.push(Det { score, cx: r[0] / scale + ax, cy: r[1] / scale + ay, w: r[2] / scale, h: r[3] / scale, kps });
    }
    out
}

/// Weighted non-maximum suppression: overlapping detections are averaged,
/// weighted by score, which steadies boxes between frames.
fn weighted_nms(mut dets: Vec<Det>, iou_thresh: f32) -> Vec<Det> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out = Vec::new();
    while !dets.is_empty() {
        let top = dets[0].rect();
        let (group, rest): (Vec<Det>, Vec<Det>) = dets.into_iter().partition(|d| d.rect().iou(&top) >= iou_thresh);
        dets = rest;
        let wsum: f32 = group.iter().map(|d| d.score).sum();
        let avg = |f: &dyn Fn(&Det) -> f32| group.iter().map(|d| f(d) * d.score).sum::<f32>() / wsum;
        let nk = group[0].kps.len();
        out.push(Det {
            score: group[0].score,
            cx: avg(&|d| d.cx),
            cy: avg(&|d| d.cy),
            w: avg(&|d| d.w),
            h: avg(&|d| d.h),
            kps: (0..nk).map(|k| (avg(&|d| d.kps[k].0), avg(&|d| d.kps[k].1))).collect(),
        });
    }
    out
}

/// A square ROI covering the whole frame (letterboxing for the detectors).
fn full_square(frame: &Frame) -> Roi {
    let s = frame.width.max(frame.height) as f32;
    Roi { cx: frame.width as f32 / 2.0, cy: frame.height as f32 / 2.0, w: s, h: s, angle: 0.0 }
}

/// NHWC input tensor sampled from `roi`, values mapped to `[lo, hi]`.
fn roi_input(frame: &Frame, roi: &Roi, size: usize, lo: f32, hi: f32) -> Tensor {
    let v: Vec<f32> = roi.sample_rgb(frame, size, size).into_iter().map(|x| lo + x * (hi - lo)).collect();
    Tensor::f32(&[1, size, size, 3], v)
}

// ------------------------------------------------------------- faces

pub struct FaceDetector {
    session: Arc<Session>,
    anchors: Vec<(f32, f32)>,
    /// Minimum score (0..1).
    pub threshold: f32,
}

impl FaceDetector {
    pub fn load(store: &ModelStore) -> Result<FaceDetector> {
        let dir = store.require(ModelId::FaceDetector)?;
        Ok(FaceDetector { session: engine::session(&dir.join("blaze_face_short_range.tflite"))?, anchors: ssd_anchors(128, &[8, 16, 16, 16]), threshold: 0.5 })
    }

    /// Faces in a frame, highest score first. The short-range model is made
    /// for faces filling at least ~1/8 of the frame (vlogs, talking heads);
    /// for small faces in a wide shot, call on a crop.
    pub fn detect(&self, frame: &Frame) -> Result<Vec<Face>> {
        if frame.width < 8 || frame.height < 8 {
            return invalid("frame too small");
        }
        let roi = full_square(frame);
        let mut o = engine::run(&self.session, vec![("input", roi_input(frame, &roi, 128, -1.0, 1.0))])?;
        let reg = o.take("regressors")?.into_f32_vec();
        let cls = o.take("classificators")?.into_f32_vec();
        let dets = weighted_nms(decode(&reg, &cls, &self.anchors, 16, 6, 128.0, self.threshold), 0.3);
        Ok(dets
            .into_iter()
            .map(|d| {
                let (x0, y0) = roi.to_source(d.cx - d.w / 2.0, d.cy - d.h / 2.0);
                let (x1, y1) = roi.to_source(d.cx + d.w / 2.0, d.cy + d.h / 2.0);
                let mut kp = [(0.0, 0.0); 6];
                for (k, p) in d.kps.iter().enumerate().take(6) {
                    kp[k] = roi.to_source(p.0, p.1);
                }
                Face { bbox: Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }, score: d.score, keypoints: kp }
            })
            .collect())
    }
}

/// Follows faces across frames: detections are matched to existing tracks
/// by overlap, and each track's box is smoothed so crops that follow it do
/// not jitter. Tracks survive a few missed frames.
#[derive(Default)]
pub struct FaceTracker {
    tracks: Vec<Track>,
    next_id: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: u32,
    pub bbox: Rect,
    pub score: f32,
    /// Frames since the face was last seen (0 = this frame).
    pub missed: u32,
    /// Frames the track has existed.
    pub age: u32,
}

impl FaceTracker {
    /// Smoothing: 0 = raw boxes, close to 1 = very steady.
    pub const SMOOTHING: f32 = 0.6;
    const MAX_MISSED: u32 = 10;

    pub fn update(&mut self, faces: &[Face]) -> &[Track] {
        let mut used = vec![false; faces.len()];
        for t in &mut self.tracks {
            let best = faces.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(i, f)| (i, f.bbox.iou(&t.bbox))).max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, iou)) if iou > 0.2 => {
                    used[i] = true;
                    let k = Self::SMOOTHING;
                    let f = &faces[i].bbox;
                    t.bbox = Rect { x: t.bbox.x * k + f.x * (1.0 - k), y: t.bbox.y * k + f.y * (1.0 - k), w: t.bbox.w * k + f.w * (1.0 - k), h: t.bbox.h * k + f.h * (1.0 - k) };
                    t.score = faces[i].score;
                    t.missed = 0;
                }
                _ => t.missed += 1,
            }
            t.age += 1;
        }
        self.tracks.retain(|t| t.missed <= Self::MAX_MISSED);
        for (i, f) in faces.iter().enumerate() {
            if !used[i] {
                self.tracks.push(Track { id: self.next_id, bbox: f.bbox, score: f.score, missed: 0, age: 1 });
                self.next_id += 1;
            }
        }
        &self.tracks
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn reset(&mut self) {
        self.tracks.clear();
    }
}

// -------------------------------------------------------------- pose

/// One body landmark in pixels. `z` is depth relative to the hips, in
/// roughly the same scale as `x`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Landmark {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Probability the point is visible (not occluded).
    pub visibility: f32,
    /// Probability the point is inside the frame.
    pub presence: f32,
}

/// Indices of the 33 BlazePose body points.
pub mod body {
    pub const NOSE: usize = 0;
    pub const LEFT_EYE: usize = 2;
    pub const RIGHT_EYE: usize = 5;
    pub const LEFT_SHOULDER: usize = 11;
    pub const RIGHT_SHOULDER: usize = 12;
    pub const LEFT_WRIST: usize = 15;
    pub const RIGHT_WRIST: usize = 16;
    pub const LEFT_HIP: usize = 23;
    pub const RIGHT_HIP: usize = 24;
    pub const LEFT_ANKLE: usize = 27;
    pub const RIGHT_ANKLE: usize = 28;
    pub const COUNT: usize = 33;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    /// [`body::COUNT`] points.
    pub landmarks: Vec<Landmark>,
    pub score: f32,
    /// The region the landmark model looked at.
    pub roi: Roi,
}

impl Pose {
    /// Bounding box of the confidently visible points.
    pub fn bbox(&self) -> Option<Rect> {
        let pts: Vec<_> = self.landmarks.iter().filter(|l| l.visibility > 0.5).collect();
        if pts.is_empty() {
            return None;
        }
        let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |a, l| (a.0.min(l.x), a.1.max(l.x)));
        let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |a, l| (a.0.min(l.y), a.1.max(l.y)));
        Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

pub struct PoseTracker {
    detector: Arc<Session>,
    landmarks: Arc<Session>,
    anchors: Vec<(f32, f32)>,
    /// Region for the next frame (from the last landmarks).
    next_roi: Option<Roi>,
}

impl PoseTracker {
    pub fn load(store: &ModelStore) -> Result<PoseTracker> {
        let dir = store.require(ModelId::PoseLandmarker)?;
        let task = dir.join("pose_landmarker_lite.task");
        Ok(PoseTracker {
            detector: engine::task_session(&task, "pose_detector.tflite")?,
            landmarks: engine::task_session(&task, "pose_landmarks_detector.tflite")?,
            anchors: ssd_anchors(224, &[8, 16, 32, 32, 32]),
            next_roi: None,
        })
    }

    /// The square around a body from two alignment points (hip centre and
    /// the point above the head): side 2.5 × their distance, rotated so the
    /// body is upright in the crop.
    fn roi_from(p0: (f32, f32), p1: (f32, f32)) -> Roi {
        let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
        let side = 2.0 * (dx * dx + dy * dy).sqrt() * 1.25;
        let angle = std::f32::consts::FRAC_PI_2 - (-dy).atan2(dx);
        Roi { cx: p0.0, cy: p0.1, w: side, h: side, angle }
    }

    /// Runs the person detector; the best person's region.
    fn detect_roi(&self, frame: &Frame) -> Result<Option<Roi>> {
        let roi = full_square(frame);
        // Unlike the face detector, this one takes [0, 1] input (measured:
        // with [-1, 1] its alignment points shrink towards the face).
        let mut o = engine::run(&self.detector, vec![("input_1", roi_input(frame, &roi, 224, 0.0, 1.0))])?;
        let reg = o.take("Identity")?.into_f32_vec();
        let cls = o.take("Identity_1")?.into_f32_vec();
        let dets = weighted_nms(decode(&reg, &cls, &self.anchors, 12, 4, 224.0, 0.5), 0.3);
        Ok(dets.first().map(|d| Self::roi_from(roi.to_source(d.kps[0].0, d.kps[0].1), roi.to_source(d.kps[1].0, d.kps[1].1))))
    }

    /// Landmarks inside a region, or `None` when no person is there.
    fn landmarks_in(&self, frame: &Frame, roi: Roi) -> Result<Option<(Pose, Roi)>> {
        let mut o = engine::run(&self.landmarks, vec![("input_1", roi_input(frame, &roi, 256, 0.0, 1.0))])?;
        let lm = o.take("Identity")?.into_f32_vec();
        let flag = o.take("Identity_1")?.into_f32_vec();
        let score = flag.first().map(|&f| if (0.0..=1.0).contains(&f) { f } else { sigmoid(f) }).unwrap_or(0.0);
        if score < 0.5 || lm.len() < 39 * 5 {
            return Ok(None);
        }
        let pt = |i: usize| {
            let v = &lm[i * 5..i * 5 + 5];
            let (x, y) = roi.to_source(v[0] / 256.0, v[1] / 256.0);
            Landmark { x, y, z: v[2] / 256.0 * roi.w, visibility: sigmoid(v[3]), presence: sigmoid(v[4]) }
        };
        let landmarks: Vec<Landmark> = (0..body::COUNT).map(pt).collect();
        // points 33 and 34 are the alignment points for the next frame
        let (a, b) = (pt(33), pt(34));
        let next = Self::roi_from((a.x, a.y), (b.x, b.y));
        Ok(Some((Pose { landmarks, score, roi }, next)))
    }

    /// The pose in a single image (always runs the detector).
    pub fn detect(&self, frame: &Frame) -> Result<Option<Pose>> {
        let Some(roi) = self.detect_roi(frame)? else { return Ok(None) };
        Ok(self.landmarks_in(frame, roi)?.map(|p| p.0))
    }

    /// The pose in the next frame of a clip (tracks from the previous
    /// landmarks; re-detects when the person is lost). Call
    /// [`PoseTracker::reset`] at cuts.
    pub fn next_frame(&mut self, frame: &Frame) -> Result<Option<Pose>> {
        if let Some(roi) = self.next_roi
            && let Some((p, next)) = self.landmarks_in(frame, roi)?
        {
            self.next_roi = Some(next);
            return Ok(Some(p));
        }
        let found = match self.detect_roi(frame)? {
            Some(roi) => self.landmarks_in(frame, roi)?,
            None => None,
        };
        self.next_roi = found.as_ref().map(|f| f.1);
        Ok(found.map(|f| f.0))
    }

    pub fn reset(&mut self) {
        self.next_roi = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_counts_match_the_models() {
        assert_eq!(ssd_anchors(128, &[8, 16, 16, 16]).len(), 896);
        assert_eq!(ssd_anchors(224, &[8, 16, 32, 32, 32]).len(), 2254);
        let a = ssd_anchors(128, &[8, 16, 16, 16]);
        assert_eq!(a[0], (0.5 / 16.0, 0.5 / 16.0));
        assert_eq!(a[512], (0.5 / 8.0, 0.5 / 8.0));
    }

    #[test]
    fn decode_and_nms_merge_overlaps() {
        let anchors = vec![(0.5, 0.5), (0.52, 0.5), (0.1, 0.1)];
        // 3 anchors × 8 coords (box + 2 keypoints), scale 100
        let mut reg = vec![0.0f32; 3 * 8];
        for i in 0..3 {
            reg[i * 8 + 2] = 20.0;
            reg[i * 8 + 3] = 20.0;
        }
        let cls = vec![3.0, 2.0, -5.0];
        let d = decode(&reg, &cls, &anchors, 8, 2, 100.0, 0.5);
        assert_eq!(d.len(), 2);
        let m = weighted_nms(d, 0.3);
        assert_eq!(m.len(), 1);
        assert!(m[0].cx > 0.5 && m[0].cx < 0.52);
        assert!((m[0].w - 0.2).abs() < 1e-6);
    }

    #[test]
    fn rect_iou() {
        let a = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        assert_eq!(a.iou(&a), 1.0);
        assert!((a.iou(&Rect { x: 5.0, y: 0.0, w: 10.0, h: 10.0 }) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(a.iou(&Rect { x: 20.0, y: 0.0, w: 1.0, h: 1.0 }), 0.0);
    }

    #[test]
    fn tracker_keeps_ids_and_smooths() {
        let face = |x: f32| Face { bbox: Rect { x, y: 10.0, w: 50.0, h: 50.0 }, score: 0.9, keypoints: [(0.0, 0.0); 6] };
        let mut t = FaceTracker::default();
        let id = t.update(&[face(100.0)])[0].id;
        let tr = t.update(&[face(110.0)]);
        assert_eq!(tr.len(), 1);
        assert_eq!(tr[0].id, id);
        assert!(tr[0].bbox.x > 100.0 && tr[0].bbox.x < 110.0, "smoothed: {}", tr[0].bbox.x);
        for _ in 0..=FaceTracker::MAX_MISSED {
            t.update(&[]);
        }
        assert!(t.tracks().is_empty());
    }

    #[test]
    fn upright_body_roi_is_not_rotated() {
        let r = PoseTracker::roi_from((100.0, 200.0), (100.0, 100.0));
        assert!(r.angle.abs() < 1e-6);
        assert!((r.w - 250.0).abs() < 1e-3);
        // lying body, head to the right: the crop's up must point to +x
        let r = PoseTracker::roi_from((100.0, 200.0), (200.0, 200.0));
        let (x, y) = r.to_source(0.5, 0.0);
        assert!(x > 100.0 && (y - 200.0).abs() < 1e-3, "({x},{y})");
    }
}
