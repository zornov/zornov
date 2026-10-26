// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Making an object map from footage.
//!
//! The detector looks at a few **key frames** (every second or two). Between
//! them the motion tracker carries every box from frame to frame, so the
//! cost is the detector's, a few seconds per key frame, not per frame. At a
//! key frame detections are matched with the objects already followed
//! (they keep their ids), the tracker is restarted on the detected box
//! (no drift), new objects are tracked backwards to find when they came
//! in, and objects the detector stops finding are ended. On the key
//! frames the depth, text, faces and body points are read too, and the
//! camera's own motion is measured on every sampled frame so the objects'
//! motion can be told apart from it.

use super::assoc::{self, Followed, Found};
use super::colour;
use super::geometry::{self, Anchor, Camera, DepthScale};
use super::motion;
use super::{BBox, Body, Kind, Obs, Object, ObjectMap, Scale, TextItem, VERSION};
use crate::error::{invalid, Result};
use crate::features::depth::DepthEstimator;
use crate::features::detect::{self, Detection, OpenVocabDetector, DEFAULT_VOCABULARY};
use crate::features::faces::{body, Face, FaceDetector, Pose, PoseTracker};
use crate::features::ocr::{TextLine, TextReader};
use crate::features::track::{self, Gray, Mat3, Motion, Pyramid, Region, RegionTracker, TrackSettings, IDENTITY};
use crate::frame::{Frame, FrameSource, Mask};
use crate::jobs::JobCtx;
use crate::store::ModelStore;
use std::collections::HashMap;
use std::ops::Range;

/// What the builder asks of the models. Tests give it scripted answers.
pub trait Sensors {
    /// Boxes (pixels of `frame`) and scores for `names`, overlaps merged.
    fn detect(&mut self, frame: &Frame, names: &[String], threshold: f32) -> Result<Vec<Detection>>;
    /// Relative inverse depth (1 = near), if a depth model is available.
    fn depth(&mut self, frame: &Frame) -> Result<Option<Mask>>;
    /// Lines of text, if a reader is available.
    fn read_text(&mut self, frame: &Frame) -> Result<Vec<TextLine>>;
    /// Faces, if a face detector is available.
    fn faces(&mut self, frame: &Frame) -> Result<Vec<Face>>;
    /// The body points of the one person in `crop`.
    fn body(&mut self, crop: &Frame) -> Result<Option<Pose>>;
}

/// The real models. Only the detector is required.
pub struct Models {
    pub detector: OpenVocabDetector,
    pub depth: Option<DepthEstimator>,
    pub text: Option<TextReader>,
    pub faces: Option<FaceDetector>,
    pub pose: Option<PoseTracker>,
}

impl Models {
    /// Loads the detector (an error if it is not installed) and every
    /// helper model that is.
    pub fn load(store: &ModelStore) -> Result<Models> {
        Ok(Models {
            detector: OpenVocabDetector::load(store)?,
            depth: DepthEstimator::load(store).ok(),
            text: TextReader::load(store).ok(),
            faces: FaceDetector::load(store).ok(),
            pose: PoseTracker::load(store).ok(),
        })
    }
}

impl Sensors for Models {
    fn detect(&mut self, frame: &Frame, names: &[String], threshold: f32) -> Result<Vec<Detection>> {
        Ok(detect::merge(self.detector.detect(frame, names, threshold)?, 0.55))
    }
    fn depth(&mut self, frame: &Frame) -> Result<Option<Mask>> {
        match &self.depth {
            Some(d) => Ok(Some(d.estimate(frame)?)),
            None => Ok(None),
        }
    }
    fn read_text(&mut self, frame: &Frame) -> Result<Vec<TextLine>> {
        match &self.text {
            Some(t) => t.read(frame),
            None => Ok(Vec::new()),
        }
    }
    fn faces(&mut self, frame: &Frame) -> Result<Vec<Face>> {
        match &self.faces {
            Some(f) => Ok(f.detect(frame)?.into_iter().filter(|f| f.score > 0.6).collect()),
            None => Ok(Vec::new()),
        }
    }
    fn body(&mut self, crop: &Frame) -> Result<Option<Pose>> {
        match &self.pose {
            Some(p) => p.detect(crop),
            None => Ok(None),
        }
    }
}

/// How to read the footage.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildOptions {
    /// Extra things to look for ("a red kettle"), on top of the default
    /// vocabulary.
    pub names: Vec<String>,
    /// Search the default vocabulary too.
    pub default_vocabulary: bool,
    /// Most key frames per piece of footage.
    pub max_keyframes: usize,
    /// Seconds between key frames when the footage is long enough.
    pub keyframe_gap: f32,
    /// Frames per second the tracker samples.
    pub track_fps: f32,
    /// Detector score an object needs to be kept (its best score).
    pub min_score: f32,
    pub hfov_deg: f32,
    pub read_text: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { names: Vec::new(), default_vocabulary: true, max_keyframes: 8, keyframe_gap: 1.5, track_fps: 6.0, min_score: 0.18, hfov_deg: geometry::DEFAULT_HFOV_DEG, read_text: true }
    }
}

impl BuildOptions {
    /// Text for the cache key: every setting that changes the map.
    pub fn cache_params(&self) -> String {
        let mut names = self.names.clone();
        names.sort();
        format!("n={};dv={};k={};g={:.2};f={:.1};s={:.2};h={:.0};t={}", names.join("|").to_lowercase(), self.default_vocabulary as u8, self.max_keyframes, self.keyframe_gap, self.track_fps, self.min_score, self.hfov_deg, self.read_text as u8)
    }

    /// The names the detector is asked for.
    pub fn vocabulary(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for n in self.names.iter().map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()) {
            if !v.contains(&n) {
                v.push(n);
            }
        }
        if self.default_vocabulary {
            for n in DEFAULT_VOCABULARY {
                if !v.iter().any(|x| x == n) {
                    v.push((*n).to_string());
                }
            }
        }
        v.truncate(160);
        v
    }
}

// -------------------------------------------------------------- vocabulary

/// What a name asked of the detector means as an object.
#[derive(Clone, Debug, PartialEq)]
pub struct Canon {
    pub label: String,
    pub descriptor: Option<String>,
    pub kind: Kind,
    pub part: Option<Part>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Face,
    Head,
}

pub fn canonical(name: &str) -> Canon {
    let n = name.trim().to_lowercase();
    let n = n.trim_start_matches("a ").trim_start_matches("an ").trim_start_matches("the ").to_string();
    let c = |label: &str, kind: Kind| Canon { label: label.to_string(), descriptor: None, kind, part: None };
    match n.as_str() {
        "man" | "woman" | "boy" | "girl" | "child" => Canon { label: "person".into(), descriptor: Some(n), kind: Kind::Person, part: None },
        "person" | "people" => c("person", Kind::Person),
        "face" => Canon { part: Some(Part::Face), ..c("face", Kind::Thing) },
        "animal head" => Canon { part: Some(Part::Head), ..c("head", Kind::Thing) },
        "dog" | "cat" | "horse" | "bird" | "cow" | "sheep" | "bear" | "elephant" | "giraffe" | "zebra" | "monkey" | "deer" | "rabbit" | "fish" | "insect" => c(&n, Kind::Animal),
        "car" | "truck" | "bus" | "motorcycle" | "bicycle" | "boat" | "airplane" | "train" => c(&n, Kind::Vehicle),
        "tree" | "mountain" | "building" | "house" | "bridge" | "tent" | "stairs" | "window" | "curtain" => c(&n, Kind::Scenery),
        _ => c(&n, Kind::Thing),
    }
}

// ---------------------------------------------------------------- sampling

/// Frame indices (relative) where the detector looks.
pub fn keyframe_indices(n: usize, fps: f64, o: &BuildOptions) -> Vec<usize> {
    if n <= 1 {
        return vec![0];
    }
    let secs = n as f64 / fps;
    let k = ((secs / o.keyframe_gap as f64).round() as usize).clamp(1, o.max_keyframes.max(1));
    let mut v: Vec<usize> = (0..k).map(|i| (((i as f64 + 0.5) / k as f64) * n as f64) as usize).map(|i| i.min(n - 1)).collect();
    v.dedup();
    v
}

/// Frame indices the tracker samples: every few frames plus the key frames.
pub fn sample_indices(n: usize, fps: f64, keys: &[usize], o: &BuildOptions) -> Vec<usize> {
    if n <= 1 {
        return vec![0];
    }
    // About six samples a second, and never more than 150 in all (their
    // pictures are kept for tracking backwards).
    let step = ((fps / o.track_fps.max(0.5) as f64).round() as usize).max(1).max(n.div_ceil(MAX_SAMPLES));
    let mut v: Vec<usize> = (0..n).step_by(step).collect();
    v.extend_from_slice(keys);
    // The very last frame ends the shot for enter/leave decisions.
    v.sort_unstable();
    v.dedup();
    v
}

const TRACK_SIDE: usize = 320;
/// Most frames the tracker samples in one piece of footage.
const MAX_SAMPLES: usize = 150;

fn gray_small(f: &Frame) -> Gray {
    let k = (TRACK_SIDE as f32 / f.width.max(f.height) as f32).min(1.0);
    let (w, h) = (((f.width as f32 * k).round() as usize).max(16), ((f.height as f32 * k).round() as usize).max(16));
    let luma = f.luma();
    let data = if (w, h) == (f.width, f.height) { luma } else { studio_nn::image::resize_plane(&luma, f.width, f.height, w, h) };
    Gray { width: w, height: h, data }
}

/// Busyness of 16 x 9 cells: mean gradient strength scaled to 0..255.
fn detail_grid(g: &Gray) -> Vec<u8> {
    let mut sum = vec![0.0f32; 16 * 9];
    let mut cnt = vec![0.0f32; 16 * 9];
    for y in 1..g.height - 1 {
        for x in 1..g.width - 1 {
            let gx = g.data[y * g.width + x + 1] - g.data[y * g.width + x - 1];
            let gy = g.data[(y + 1) * g.width + x] - g.data[(y - 1) * g.width + x];
            let c = (y * 9 / g.height) * 16 + x * 16 / g.width;
            sum[c] += (gx.abs() + gy.abs()) * 0.5;
            cnt[c] += 1.0;
        }
    }
    sum.iter().zip(&cnt).map(|(s, c)| ((s / c.max(1.0)) * 1200.0).clamp(0.0, 255.0) as u8).collect()
}

// ------------------------------------------------------------------ tracks

#[derive(Default)]
struct Votes {
    /// label -> (score sum, count, kind, descriptor votes)
    labels: HashMap<String, (f32, u32, Option<Kind>)>,
    descriptors: HashMap<String, u32>,
    best: HashMap<String, f32>,
}

struct Tr {
    obs: Vec<Obs>,
    votes: Votes,
    tracker: Option<RegionTracker>,
    ref_box: BBox,
    alive: bool,
    missed: usize,
    low: usize,
    last_det: f32,
    detections: u32,
    facing: Vec<String>,
    colours: Vec<String>,
    texts: Vec<String>,
}

impl Tr {
    fn label(&self) -> (String, Kind) {
        let (l, v) = self.votes.labels.iter().max_by(|a, b| a.1 .0.total_cmp(&b.1 .0).then(b.0.cmp(a.0))).expect("a track has a detection");
        (l.clone(), v.2.unwrap_or(Kind::Thing))
    }
}

fn settings() -> TrackSettings {
    TrackSettings { motion: Motion::Similarity, features: 60, fb_tol: 1.2, min_confidence: 0.2 }
}

/// A tracker on `b` (inner 80 % of the box, where the thing is rather than
/// its surroundings) in `pyr`.
fn start_tracker(pyr: &Pyramid, b: &BBox) -> Option<RegionTracker> {
    let g = pyr.base();
    let inner = b.grown(0.8);
    let (w, h) = (g.width as f64, g.height as f64);
    let (x0, y0, x1, y1) = (inner.x as f64 * w, inner.y as f64 * h, inner.right() as f64 * w, inner.bottom() as f64 * h);
    if x1 - x0 < 6.0 || y1 - y0 < 6.0 {
        return None;
    }
    RegionTracker::new(pyr.clone(), Region::Plane([(x0, y0), (x1, y0), (x1, y1), (x0, y1)]), settings()).ok()
}

/// Where the box `b` (drawn on the tracker's reference frame) is under `h`.
fn moved_box(b: &BBox, h: &Mat3, g: &Gray) -> BBox {
    let (w, hh) = (g.width as f64, g.height as f64);
    let corners = [(b.x as f64 * w, b.y as f64 * hh), (b.right() as f64 * w, b.y as f64 * hh), (b.right() as f64 * w, b.bottom() as f64 * hh), (b.x as f64 * w, b.bottom() as f64 * hh)];
    let q: Vec<(f64, f64)> = corners.iter().map(|c| track::apply(h, *c)).collect();
    let x0 = q.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let x1 = q.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let y0 = q.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let y1 = q.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    BBox::new((x0 / w) as f32, (y0 / hh) as f32, ((x1 - x0) / w) as f32, ((y1 - y0) / hh) as f32)
}

/// A box that has mostly left the picture.
fn outside(b: &BBox) -> bool {
    b.w <= 0.0 || b.h <= 0.0 || b.clamped().area() < 0.3 * b.area() || b.cx() < -0.05 || b.cx() > 1.05 || b.cy() < -0.05 || b.cy() > 1.05
}

/// Pulls the tracked path onto a detection: the offset `delta` found at
/// `t_end` is spread over the observations since `from` (the last
/// detection), in proportion to the time since it, so the path meets the
/// detection without a jump.
fn correct_drift(obs: &mut [Obs], from: usize, t_end: f32, delta: [f32; 4]) {
    let t0 = obs[from].t;
    if t_end <= t0 {
        return;
    }
    for o in obs[from + 1..].iter_mut() {
        let a = ((o.t - t0) / (t_end - t0)).clamp(0.0, 1.0);
        o.bbox = BBox::new(o.bbox.x + delta[0] * a, o.bbox.y + delta[1] * a, (o.bbox.w + delta[2] * a).max(0.002), (o.bbox.h + delta[3] * a).max(0.002));
    }
}

// ------------------------------------------------------------------- build

struct KeyData {
    t: f32,
    scale: DepthScale,
}

/// Reads frames `range` of `src` into an object map.
pub fn build(src: &mut dyn FrameSource, range: Range<usize>, sensors: &mut dyn Sensors, o: &BuildOptions, ctx: &JobCtx) -> Result<ObjectMap> {
    let n = range.len();
    if n == 0 || range.end > src.frame_count() {
        return invalid(format!("frames {}..{} are not in the footage ({} frames)", range.start, range.end, src.frame_count()));
    }
    let fps = src.fps().max(1.0);
    let duration = n as f32 / fps as f32;
    let keys = keyframe_indices(n, fps, o);
    let samples = sample_indices(n, fps, &keys, o);
    let names = o.vocabulary();
    if names.is_empty() {
        return invalid("nothing to look for");
    }
    let canon: Vec<Canon> = names.iter().map(|n| canonical(n)).collect();

    let mut times: Vec<f32> = Vec::with_capacity(samples.len());
    let mut pyrs: Vec<Pyramid> = Vec::with_capacity(samples.len());
    let mut cuts: Vec<bool> = Vec::with_capacity(samples.len());
    let mut steps: Vec<Mat3> = Vec::new();
    let mut tracks: Vec<Tr> = Vec::new();
    let mut key_data: Vec<KeyData> = Vec::new();
    let mut texts: Vec<TextItem> = Vec::new();
    let mut detail: Vec<Vec<u8>> = Vec::new();
    let mut key_times: Vec<f32> = Vec::new();
    let mut size = (0usize, 0usize);
    let mut track_size = (1.0f32, 1.0f32);

    for (s, &fi) in samples.iter().enumerate() {
        ctx.check()?;
        ctx.set_progress(s as f32 / samples.len() as f32);
        let is_key = keys.contains(&fi);
        ctx.set_message(format!("Looking at the footage: {} of {}", s + 1, samples.len()));
        let frame = src.frame(range.start + fi)?;
        size = (frame.width, frame.height);
        let t = fi as f32 / fps as f32;
        let gray = gray_small(&frame);
        track_size = (gray.width as f32, gray.height as f32);
        let pyr = Pyramid::new(gray.clone(), 4);

        // Camera motion and the objects being followed, frame to frame.
        let mut cut = false;
        if s > 0 {
            cut = track::is_cut(pyrs[s - 1].base(), &gray);
            steps.push(if cut { IDENTITY } else { track::frame_motion(&pyrs[s - 1], &pyr).0 });
            for tr in tracks.iter_mut().filter(|t| t.alive) {
                if cut {
                    tr.alive = false;
                    continue;
                }
                let (nb, conf, lost) = match tr.tracker.as_mut() {
                    Some(tk) => {
                        let sm = tk.step(pyr.clone(), s);
                        (moved_box(&tr.ref_box, &sm.h, &gray), sm.confidence, sm.lost)
                    }
                    // Nothing to follow in a plain patch: assume it stays.
                    None => (tr.obs.last().map(|o| o.bbox).unwrap_or(tr.ref_box), 0.3, false),
                };
                tr.low = if lost || conf < 0.15 { tr.low + 1 } else { 0 };
                if tr.low >= 2 {
                    tr.alive = false;
                    continue;
                }
                if outside(&nb) {
                    tr.alive = false;
                    continue;
                }
                tr.obs.push(Obs::new(t, nb.clamped(), false, conf));
            }
        }
        times.push(t);
        cuts.push(cut);
        pyrs.push(pyr);

        if !is_key {
            continue;
        }
        // ---- key frame: detect, match, restart, read the rest.
        key_times.push(t);
        detail.push(detail_grid(&gray));
        let (fw, fh) = (frame.width as f32, frame.height as f32);
        let dets = sensors.detect(&frame, &names, 0.12)?;
        let mut found: Vec<Found> = Vec::new();
        let mut face_parts: Vec<(BBox, f32)> = Vec::new();
        let mut head_parts: Vec<BBox> = Vec::new();
        let mut descriptors: Vec<Option<String>> = Vec::new();
        for d in &dets {
            let c = &canon[d.name.min(canon.len() - 1)];
            let b = BBox::from_pixels(&d.bbox, fw, fh);
            if b.area() < 0.0006 {
                continue;
            }
            match c.part {
                Some(Part::Face) => face_parts.push((b, d.score)),
                Some(Part::Head) => head_parts.push(b),
                None => {
                    found.push(Found { label: c.label.clone(), kind: c.kind, score: d.score, bbox: b });
                    descriptors.push(c.descriptor.clone());
                }
            }
        }
        // A face found inside a person is that person's; a lone face is a
        // thing of its own (a close-up).
        let mut lone_faces: Vec<(BBox, f32)> = Vec::new();
        for (fb, sc) in &face_parts {
            let in_person = found.iter().any(|f| f.kind == Kind::Person && fb.inside(&f.bbox) > 0.7);
            if !in_person {
                lone_faces.push((*fb, *sc));
            }
        }
        for (fb, sc) in lone_faces {
            found.push(Found { label: "face".into(), kind: Kind::Thing, score: sc, bbox: fb });
            descriptors.push(None);
        }

        let followed_idx: Vec<usize> = (0..tracks.len()).filter(|&i| tracks[i].alive).collect();
        let followed: Vec<Followed> = followed_idx
            .iter()
            .map(|&i| {
                let (label, kind) = tracks[i].label();
                Followed { label, kind, at: tracks[i].obs.last().map(|o| o.bbox).unwrap_or(tracks[i].ref_box) }
            })
            .collect();
        let matches = assoc::associate(&followed, &found);
        let mut seen = vec![false; followed.len()];
        let mut touched: Vec<usize> = Vec::new();
        for (j, f) in found.iter().enumerate() {
            let det_obs = Obs::new(t, f.bbox.clamped(), true, f.score);
            match matches[j] {
                Some(fi_) => {
                    seen[fi_] = true;
                    let ti = followed_idx[fi_];
                    let tr = &mut tracks[ti];
                    // Replace the tracker's guess for this instant, and pull
                    // the path since the last detection onto the new one.
                    let tracked_end = tr.obs.last().filter(|o| (o.t - t).abs() < 1e-4).map(|o| o.bbox);
                    if tracked_end.is_some() {
                        tr.obs.pop();
                    }
                    if let (Some(te), Some(from)) = (tracked_end, tr.obs.iter().rposition(|o| o.detected)) {
                        let d = det_obs.bbox;
                        correct_drift(&mut tr.obs, from, t, [d.x - te.x, d.y - te.y, d.w - te.w, d.h - te.h]);
                    }
                    tr.obs.push(det_obs);
                    vote(&mut tr.votes, f, descriptors[j].as_deref());
                    tr.missed = 0;
                    tr.low = 0;
                    tr.last_det = t;
                    tr.detections += 1;
                    tr.ref_box = f.bbox.clamped();
                    tr.tracker = start_tracker(&pyrs[s], &tr.ref_box);
                    touched.push(ti);
                }
                None => {
                    if f.score < o.min_score {
                        continue;
                    }
                    let mut tr = Tr { obs: vec![det_obs], votes: Votes::default(), tracker: start_tracker(&pyrs[s], &f.bbox.clamped()), ref_box: f.bbox.clamped(), alive: true, missed: 0, low: 0, last_det: t, detections: 1, facing: Vec::new(), colours: Vec::new(), texts: Vec::new() };
                    vote(&mut tr.votes, f, descriptors[j].as_deref());
                    // When did it come in? Follow it back through the frames.
                    if s > 0 {
                        if let Some(mut bt) = start_tracker(&pyrs[s], &tr.ref_box) {
                            let mut k = s;
                            while k > 0 {
                                if cuts[k] {
                                    break;
                                }
                                let sm = bt.step(pyrs[k - 1].clone(), k - 1);
                                let nb = moved_box(&tr.ref_box, &sm.h, pyrs[k - 1].base());
                                if sm.lost || sm.confidence < 0.15 || outside(&nb) {
                                    break;
                                }
                                tr.obs.insert(0, Obs::new(times[k - 1], nb.clamped(), false, sm.confidence));
                                k -= 1;
                            }
                        }
                    }
                    tracks.push(tr);
                    touched.push(tracks.len() - 1);
                }
            }
        }
        for (i, &ti) in followed_idx.iter().enumerate() {
            if !seen[i] {
                let tr = &mut tracks[ti];
                tr.missed += 1;
                if tr.missed >= 2 {
                    // Gone: it ended about half way between the last
                    // detection and the first miss.
                    let end = tr.last_det + (t - tr.last_det) * 0.5;
                    tr.obs.retain(|o| o.t <= end + 1e-4);
                    tr.alive = false;
                }
            }
        }

        // ---- read the key frame's other things for the objects on it.
        let depth = sensors.depth(&frame)?;
        let faces = sensors.faces(&frame)?;
        let lines = if o.read_text { sensors.read_text(&frame)? } else { Vec::new() };
        let cam = Camera::new(fw, fh, o.hfov_deg);
        let mut anchors: Vec<Anchor> = Vec::new();
        let mut pose_budget = 4usize;
        let mut at_key: Vec<usize> = (0..tracks.len()).filter(|&i| tracks[i].obs.last().is_some_and(|ob| (ob.t - t).abs() < 1e-4)).collect();
        // Biggest first so the pose budget goes to the people that matter.
        at_key.sort_by(|&a, &b| tracks[b].obs.last().map(|o| o.bbox.area()).partial_cmp(&tracks[a].obs.last().map(|o| o.bbox.area())).unwrap_or(std::cmp::Ordering::Equal));
        for &ti in &at_key {
            let (label, kind) = tracks[ti].label();
            let bbox = tracks[ti].obs.last().expect("at key").bbox;
            // Colours.
            let cs = colour::dominant(&frame, &bbox);
            tracks[ti].colours.extend(cs);
            // Depth under the box.
            let d = depth.as_ref().and_then(|m| geometry::box_depth(&m.data, m.width, m.height, bbox.x, bbox.y, bbox.w, bbox.h));
            tracks[ti].obs.last_mut().expect("at key").depth = d;
            if kind == Kind::Person {
                // Face: the largest inside the box.
                let face = faces.iter().filter(|f| {
                    let fb = BBox::from_pixels(&f.bbox, fw, fh);
                    fb.inside(&bbox) > 0.6
                }).max_by(|a, b| a.bbox.area().total_cmp(&b.bbox.area()));
                let face_box = face.map(|f| BBox::from_pixels(&f.bbox, fw, fh)).or_else(|| face_parts.iter().filter(|(fb, _)| fb.inside(&bbox) > 0.7).max_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0));
                tracks[ti].obs.last_mut().expect("at key").face = face_box;
                if let Some(f) = face {
                    if let Some(w) = face_facing(f) {
                        tracks[ti].facing.push(w);
                    }
                    if let (Some(dd), Some(fb)) = (depth.as_ref().and_then(|m| {
                        let fb = BBox::from_pixels(&f.bbox, fw, fh);
                        geometry::box_depth(&m.data, m.width, m.height, fb.x, fb.y, fb.w, fb.h)
                    }), face_box) {
                        if let Some(a) = Anchor::from_box(&cam, &geometry::prior("face").expect("face prior"), fb.h * fh, dd) {
                            anchors.push(a);
                        }
                    }
                }
                // Body points for the biggest few.
                let mut has_body = false;
                if pose_budget > 0 && bbox.h > 0.18 {
                    pose_budget -= 1;
                    let g = bbox.grown(1.15).clamped();
                    let (cx, cy, cw, ch) = ((g.x * fw) as usize, (g.y * fh) as usize, ((g.w * fw) as usize).max(16), ((g.h * fh) as usize).max(16));
                    let (cw, ch) = (cw.min(frame.width - cx.min(frame.width - 16)), ch.min(frame.height - cy.min(frame.height - 16)));
                    let (cx, cy) = (cx.min(frame.width - cw), cy.min(frame.height - ch));
                    let crop = Frame { width: cw, height: ch, rgba: studio_nn::image::crop_rgba(frame.view(), cx, cy, cw, ch) };
                    if let Some(p) = sensors.body(&crop)? {
                        let bd = body_of(&p, cx as f32, cy as f32, fw, fh);
                        has_body = bd.full_body();
                        if let Some(w) = body_facing(&bd, face.is_some()) {
                            tracks[ti].facing.push(w);
                        }
                        tracks[ti].obs.last_mut().expect("at key").body = Some(bd);
                    }
                }
                if let (Some(dd), Some(p)) = (d, geometry::prior(&label)) {
                    let cropped = bbox.y < 0.01 || bbox.bottom() > 0.99;
                    let upright = bbox.h > 1.4 * bbox.w;
                    if !cropped && (has_body || upright) {
                        if let Some(mut a) = Anchor::from_box(&cam, &p, bbox.h * fh, dd) {
                            if !has_body {
                                a.weight *= 0.4;
                            }
                            anchors.push(a);
                        }
                    }
                }
            } else {
                if kind == Kind::Animal {
                    let head = head_parts.iter().filter(|h| bbox.contains(h.cx(), h.cy()) && h.area() < bbox.area()).min_by(|a, b| a.area().total_cmp(&b.area()));
                    if let Some(h) = head {
                        tracks[ti].obs.last_mut().expect("at key").head = Some(*h);
                        if let Some(w) = head_facing(&bbox, h) {
                            tracks[ti].facing.push(w);
                        }
                    }
                }
                if let (Some(dd), Some(p)) = (d, geometry::prior(&label)) {
                    let edge = match p.side {
                        geometry::Side::Height => bbox.y < 0.01 || bbox.bottom() > 0.99,
                        geometry::Side::Width => bbox.x < 0.01 || bbox.right() > 0.99,
                    };
                    let px = match p.side {
                        geometry::Side::Height => bbox.h * fh,
                        geometry::Side::Width => bbox.w * fw,
                    };
                    if !edge {
                        if let Some(a) = Anchor::from_box(&cam, &p, px, dd) {
                            anchors.push(a);
                        }
                    }
                }
            }
        }
        let scale = DepthScale::fit(&anchors);
        for &ti in &at_key {
            let ob = tracks[ti].obs.last_mut().expect("at key");
            ob.z = ob.depth.map(|d| scale.metres(d));
        }
        key_data.push(KeyData { t, scale });

        // Text: attached to the smallest object it sits on.
        for l in lines {
            let tb = BBox::from_pixels(&l.bbox, fw, fh);
            let owner = at_key
                .iter()
                .filter(|&&ti| {
                    let (_, kind) = tracks[ti].label();
                    kind != Kind::Scenery && tracks[ti].obs.last().is_some_and(|ob| tb.inside(&ob.bbox) > 0.8)
                })
                .min_by(|&&a, &&b| tracks[a].obs.last().map(|o| o.bbox.area()).partial_cmp(&tracks[b].obs.last().map(|o| o.bbox.area())).unwrap_or(std::cmp::Ordering::Equal))
                .copied();
            if let Some(ti) = owner {
                tracks[ti].texts.push(l.text.clone());
            }
            texts.push(TextItem { t, text: l.text, bbox: tb, score: l.score, object: owner.map(|ti| ti as u32) });
        }
    }
    ctx.set_progress(1.0);

    // ---- finish: names, ids, distances, sizes, movement.
    let last_t = times.last().copied().unwrap_or(0.0);
    let first_t = times.first().copied().unwrap_or(0.0);
    let (iw, ih) = (size.0 as f32, size.1 as f32);
    let cam = Camera::new(iw, ih, o.hfov_deg);
    let cam_path = motion::camera_path(&times, &steps, track_size.0, track_size.1);
    let camera = motion::describe(cam_path.clone(), duration, track_size.1 / track_size.0.max(1.0));
    let multi = key_times.len() > 1;

    // Keep real objects: seen at least twice, or once and sure of it.
    let mut order: Vec<usize> = (0..tracks.len())
        .filter(|&i| {
            let tr = &tracks[i];
            let best = tr.votes.best.values().copied().fold(0.0, f32::max);
            !tr.obs.is_empty() && best >= o.min_score && (tr.detections >= 2 || !multi || best >= 0.3)
        })
        .collect();
    order.sort_by(|&a, &b| {
        let (ta, tb) = (&tracks[a], &tracks[b]);
        ta.obs[0].t.total_cmp(&tb.obs[0].t).then(ta.obs[0].bbox.cx().total_cmp(&tb.obs[0].bbox.cx()))
    });
    // Remap text owners from track index to object id.
    let id_of: HashMap<usize, u32> = order.iter().enumerate().map(|(k, &ti)| (ti, k as u32 + 1)).collect();
    for t in texts.iter_mut() {
        t.object = t.object.and_then(|ti| id_of.get(&(ti as usize)).copied());
    }

    // Distances: each key frame's depth read with the nearest calibrated
    // scale (a key frame with nothing of known size in it borrows the
    // scale of one that had).
    let scale_for = |t: f32| -> DepthScale {
        key_data.iter().filter(|k| k.scale.anchors > 0).min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs())).map(|k| k.scale).or_else(|| key_data.first().map(|k| k.scale)).unwrap_or_else(DepthScale::assumed)
    };
    for tr in tracks.iter_mut() {
        for ob in tr.obs.iter_mut().filter(|o| o.detected || o.depth.is_some()) {
            if let Some(d) = ob.depth {
                ob.z = Some(scale_for(ob.t).metres(d));
            }
        }
    }
    let mut objects = Vec::new();
    for (k, &ti) in order.iter().enumerate() {
        let tr = &mut tracks[ti];
        let (label, kind) = tr.label();
        let (score, also) = {
            let mut others: Vec<(String, f32)> = tr.votes.best.iter().filter(|(l, _)| **l != label).map(|(l, s)| (l.clone(), *s)).collect();
            others.sort_by(|a, b| b.1.total_cmp(&a.1));
            others.truncate(2);
            let n = tr.votes.labels.values().map(|v| v.1).sum::<u32>().max(1) as f32;
            let sum: f32 = tr.votes.labels.values().map(|v| v.0).sum();
            (sum / n, others)
        };
        let descriptor = tr.votes.descriptors.iter().max_by_key(|(_, n)| **n).map(|(d, _)| d.clone());
        // Distances between the key frames: interpolate.
        let known: Vec<(f32, f32)> = tr.obs.iter().filter_map(|ob| Some((ob.t, ob.z?))).collect();
        for ob in tr.obs.iter_mut() {
            if ob.z.is_none() {
                ob.z = interpolate(&known, ob.t);
            }
        }
        let sizes: Vec<[f32; 2]> = tr
            .obs
            .iter()
            .filter(|ob| ob.detected && ob.z.is_some())
            .map(|ob| [cam.metres(ob.bbox.w * iw, ob.z.unwrap_or(1.0)), cam.metres(ob.bbox.h * ih, ob.z.unwrap_or(1.0))])
            .collect();
        let size = (!sizes.is_empty()).then(|| [median(sizes.iter().map(|s| s[0]).collect()), median(sizes.iter().map(|s| s[1]).collect())]);
        let colours = top_by_count(&tr.colours, 3);
        let mut texts_o = tr.texts.clone();
        texts_o.dedup();
        texts_o.truncate(4);
        let (enter, leave) = (tr.obs.first().map_or(0.0, |o| o.t), tr.obs.last().map_or(0.0, |o| o.t));
        let mv = motion::movement(&tr.obs, &cam_path, track_size.0, track_size.1);
        let mut facing = vote_facing(&tr.facing);
        if kind == Kind::Animal {
            if let Some(m) = &mv {
                if !m.still && m.speed > 0.06 && facing.is_none() {
                    facing = Some(if m.heading.cos() >= 0.0 { "heading right" } else { "heading left" }.to_string());
                }
            }
        }
        objects.push(Object {
            id: k as u32 + 1,
            label,
            kind,
            descriptor,
            score,
            also,
            obs: std::mem::take(&mut tr.obs),
            enter,
            leave,
            enters: enter > first_t + 0.2,
            leaves: leave < last_t - 0.2,
            colours,
            texts: texts_o,
            size,
            facing,
            movement: mv,
        });
    }
    let anchors = key_data.iter().map(|k| k.scale.anchors).max().unwrap_or(0);
    Ok(ObjectMap {
        version: VERSION,
        width: size.0 as u32,
        height: size.1 as u32,
        fps: fps as f32,
        duration,
        keyframes: key_times,
        vocabulary: names,
        objects,
        texts,
        camera,
        scale: Scale { anchors, hfov_deg: o.hfov_deg },
        detail,
    })
}

fn vote(v: &mut Votes, f: &Found, descriptor: Option<&str>) {
    let e = v.labels.entry(f.label.clone()).or_insert((0.0, 0, None));
    e.0 += f.score;
    e.1 += 1;
    e.2 = Some(f.kind);
    let b = v.best.entry(f.label.clone()).or_insert(0.0);
    *b = b.max(f.score);
    if let Some(d) = descriptor {
        *v.descriptors.entry(d.to_string()).or_insert(0) += 1;
    }
}

fn interpolate(known: &[(f32, f32)], t: f32) -> Option<f32> {
    match known {
        [] => None,
        [(_, z)] => Some(*z),
        _ => {
            let i = known.partition_point(|k| k.0 <= t);
            if i == 0 {
                return Some(known[0].1);
            }
            if i >= known.len() {
                return Some(known[known.len() - 1].1);
            }
            let (a, b) = (known[i - 1], known[i]);
            let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
            Some(a.1 + (b.1 - a.1) * k)
        }
    }
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

fn top_by_count(items: &[String], n: usize) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for s in items {
        match counts.iter_mut().find(|c| &c.0 == s) {
            Some(c) => c.1 += 1,
            None => counts.push((s.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    // Only colours that came up in at least a third of the readings.
    let total = items.len().max(1);
    counts.into_iter().filter(|c| c.1 * 3 >= total.min(3) || c.1 * 3 >= total).take(n).map(|c| c.0).collect()
}

fn vote_facing(v: &[String]) -> Option<String> {
    let mut counts: Vec<(&String, usize)> = Vec::new();
    for s in v {
        match counts.iter_mut().find(|c| c.0 == s) {
            Some(c) => c.1 += 1,
            None => counts.push((s, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.first().map(|c| c.0.clone())
}

// -------------------------------------------------------- people and facing

/// Which way a face looks, from where the nose sits between the eyes.
pub fn face_facing(f: &Face) -> Option<String> {
    let (er, el, nose) = (f.keypoints[0], f.keypoints[1], f.keypoints[2]);
    let eye_gap = (el.0 - er.0).abs().max(1e-3);
    let mid = (er.0 + el.0) / 2.0;
    let yaw = (nose.0 - mid) / eye_gap;
    Some(if yaw.abs() < 0.28 {
        "toward the camera".to_string()
    } else if yaw.abs() < 0.7 {
        format!("toward the camera, turned {}", if yaw > 0.0 { "right" } else { "left" })
    } else {
        format!("in profile, looking {}", if yaw > 0.0 { "right" } else { "left" })
    })
}

/// From the body points: face visible (toward), only the back (away).
pub fn body_facing(b: &Body, face_seen: bool) -> Option<String> {
    if face_seen {
        return None;
    }
    match (b.nose, b.left_shoulder, b.right_shoulder) {
        (None, Some(_), Some(_)) => Some("away from the camera".to_string()),
        _ => None,
    }
}

/// Which way an animal faces, from where its head is on its body.
pub fn head_facing(body: &BBox, head: &BBox) -> Option<String> {
    let off = (head.cx() - body.cx()) / body.w.max(1e-3);
    if off.abs() > 0.2 {
        Some(format!("facing {}", if off > 0.0 { "right" } else { "left" }))
    } else if head.w > 0.4 * body.w {
        Some("facing the camera".to_string())
    } else {
        None
    }
}

fn body_of(p: &Pose, ox: f32, oy: f32, fw: f32, fh: f32) -> Body {
    let pt = |i: usize| {
        let l = p.landmarks.get(i)?;
        (l.visibility > 0.5 && l.presence > 0.3).then(|| [(l.x + ox) / fw, (l.y + oy) / fh])
    };
    Body {
        nose: pt(body::NOSE),
        left_eye: pt(body::LEFT_EYE),
        right_eye: pt(body::RIGHT_EYE),
        left_shoulder: pt(body::LEFT_SHOULDER),
        right_shoulder: pt(body::RIGHT_SHOULDER),
        left_wrist: pt(body::LEFT_WRIST),
        right_wrist: pt(body::RIGHT_WRIST),
        left_hip: pt(body::LEFT_HIP),
        right_hip: pt(body::RIGHT_HIP),
        left_ankle: pt(body::LEFT_ANKLE),
        right_ankle: pt(body::RIGHT_ANKLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::VecSource;

    const W: usize = 480;
    const H: usize = 270;
    const FPS: f64 = 30.0;

    fn hash(x: i32, y: i32, seed: u32) -> f32 {
        let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed.wrapping_mul(2_246_822_519);
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
    }

    /// Smooth value noise with detail at several scales: easy to follow.
    fn noise(x: f32, y: f32, seed: u32) -> f32 {
        let mut v = 0.0;
        let mut amp = 0.5;
        let mut cell = 12.0;
        for o in 0..3 {
            let (fx, fy) = (x / cell, y / cell);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (tx, ty) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
            let h = |dx: i32, dy: i32| hash(x0 as i32 + dx, y0 as i32 + dy, seed + o);
            let a = h(0, 0) * (1.0 - tx) + h(1, 0) * tx;
            let b = h(0, 1) * (1.0 - tx) + h(1, 1) * tx;
            v += amp * (a * (1.0 - ty) + b * ty);
            amp *= 0.5;
            cell /= 2.0;
        }
        v
    }

    struct Thing {
        label: &'static str,
        seed: u32,
        size: f32,
        /// Top-left at time t (world coordinates, before the camera).
        at: Box<dyn Fn(f32) -> (f32, f32)>,
        present: Box<dyn Fn(f32) -> bool>,
    }

    struct World {
        things: Vec<Thing>,
        /// Scene shift on screen at time t (pixels): the camera's pan.
        pan: Box<dyn Fn(f32) -> f32>,
        frames: usize,
    }

    impl World {
        fn boxes(&self, t: f32) -> Vec<(&'static str, BBox)> {
            let sh = (self.pan)(t);
            self.things
                .iter()
                .filter(|th| (th.present)(t))
                .map(|th| {
                    let (x, y) = (th.at)(t);
                    (th.label, BBox::new((x + sh) / W as f32, y / H as f32, th.size / W as f32, th.size / H as f32))
                })
                .collect()
        }

        fn render(&self, i: usize) -> Frame {
            let t = i as f32 / FPS as f32;
            let sh = (self.pan)(t);
            let mut f = Frame::filled(W, H, [0, 0, 0, 255]);
            let tops: Vec<(f32, f32, &Thing)> = self.things.iter().filter(|th| (th.present)(t)).map(|th| { let (x, y) = (th.at)(t); (x + sh, y, th) }).collect();
            for y in 0..H {
                for x in 0..W {
                    let mut v = 0.15 + 0.55 * noise(x as f32 - sh, y as f32, 7);
                    for (ox, oy, th) in &tops {
                        let (dx, dy) = (x as f32 - ox, y as f32 - oy);
                        if dx >= 0.0 && dy >= 0.0 && dx < th.size && dy < th.size {
                            v = 0.25 + 0.7 * noise(dx, dy, th.seed);
                        }
                    }
                    let g = (v.clamp(0.0, 1.0) * 255.0) as u8;
                    f.rgba[(y * W + x) * 4..][..3].copy_from_slice(&[g, g, g]);
                }
            }
            f
        }

        fn source(&self) -> VecSource {
            VecSource { frames: (0..self.frames).map(|i| self.render(i)).collect(), fps: FPS }
        }
    }

    /// Answers with the truth at the n-th key frame, a little off.
    struct Scripted<'a> {
        world: &'a World,
        key_times: Vec<f32>,
        calls: usize,
        fake_depth: bool,
    }

    impl Sensors for Scripted<'_> {
        fn detect(&mut self, _: &Frame, names: &[String], _: f32) -> Result<Vec<Detection>> {
            let t = self.key_times[self.calls];
            self.calls += 1;
            let idx = |l: &str| names.iter().position(|n| n == l).expect("label in vocabulary");
            Ok(self
                .world
                .boxes(t)
                .into_iter()
                .enumerate()
                .map(|(k, (l, b))| {
                    let jitter = if k % 2 == 0 { 2.0 } else { -2.0 };
                    Detection { name: idx(l), score: 0.7, bbox: crate::features::faces::Rect { x: b.x * W as f32 + jitter, y: b.y * H as f32, w: b.w * W as f32, h: b.h * H as f32 } }
                })
                .collect())
        }
        fn depth(&mut self, _: &Frame) -> Result<Option<Mask>> {
            if !self.fake_depth {
                return Ok(None);
            }
            // Nearer toward the bottom of the picture.
            let (w, h) = (48, 27);
            Ok(Some(Mask::new(w, h, (0..w * h).map(|i| (i / w) as f32 / (h - 1) as f32).collect())))
        }
        fn read_text(&mut self, _: &Frame) -> Result<Vec<TextLine>> {
            Ok(Vec::new())
        }
        fn faces(&mut self, _: &Frame) -> Result<Vec<Face>> {
            Ok(Vec::new())
        }
        fn body(&mut self, _: &Frame) -> Result<Option<Pose>> {
            Ok(None)
        }
    }

    fn opts() -> BuildOptions {
        BuildOptions { default_vocabulary: false, names: vec!["dog".into(), "cat".into(), "car".into()], ..Default::default() }
    }

    fn run(world: &World, o: &BuildOptions, depth: bool) -> ObjectMap {
        let mut src = world.source();
        let n = world.frames;
        let keys = keyframe_indices(n, FPS, o);
        let mut sensors = Scripted { world, key_times: keys.iter().map(|&k| k as f32 / FPS as f32).collect(), calls: 0, fake_depth: depth };
        build(&mut src, 0..n, &mut sensors, o, &JobCtx::detached()).unwrap()
    }

    fn crossing_world() -> World {
        crossing_world_with(true)
    }

    fn crossing_world_with(car: bool) -> World {
        World {
            frames: 120,
            pan: Box::new(|_| 0.0),
            things: vec![
                // A dog walks right across the whole shot.
                Thing { label: "dog", seed: 11, size: 70.0, at: Box::new(|t| (30.0 + 70.0 * t, 150.0)), present: Box::new(|_| true) },
                // A cat comes in from the right edge at 1.5 s.
                Thing { label: "cat", seed: 23, size: 60.0, at: Box::new(|t| (W as f32 - (t - 1.5) * 90.0, 60.0)), present: Box::new(|t| t >= 1.5) },
                // A car drives off to the left and is gone by 1.2 s.
                Thing { label: "car", seed: 37, size: 70.0, at: Box::new(|t| (200.0 - 150.0 * t, 20.0)), present: Box::new(move |t| car && 200.0 - 150.0 * t > -70.0) },
            ],
        }
    }

    #[test]
    fn keyframes_and_samples_cover_the_shot() {
        let o = BuildOptions::default();
        let k = keyframe_indices(120, 30.0, &o);
        assert_eq!(k.len(), 3);
        assert!(k.windows(2).all(|w| w[0] < w[1]) && k[0] > 0 && *k.last().unwrap() < 120);
        let s = sample_indices(120, 30.0, &k, &o);
        assert!(k.iter().all(|x| s.contains(x)));
        assert_eq!(s[0], 0);
        assert_eq!(keyframe_indices(1, 30.0, &o), vec![0]);
        assert!(keyframe_indices(3000, 30.0, &o).len() <= o.max_keyframes);
        // A long shot is sampled sparsely, not kept whole in memory.
        let long = keyframe_indices(54_000, 30.0, &o);
        assert!(sample_indices(54_000, 30.0, &long, &o).len() <= MAX_SAMPLES + long.len());
        // A still picture is one key frame and one sample.
        assert_eq!(sample_indices(1, 30.0, &[0], &o), vec![0]);
    }

    #[test]
    fn objects_keep_their_ids_and_the_tracker_follows_between_key_frames() {
        let world = crossing_world();
        let m = run(&world, &opts(), false);
        let dog = m.named("dog").into_iter().next().expect("the dog");
        // Numbered by first appearance: the dog and car are there at the start.
        assert!(dog.id <= 2, "ids by first appearance: {:?}", m.objects.iter().map(|o| (o.id, &o.label)).collect::<Vec<_>>());
        assert_eq!(m.named("dog").len(), 1, "one dog, not one per key frame");
        assert!(dog.obs.len() > 8, "tracked between key frames: {}", dog.obs.len());
        // Every observation is close to the truth, key frame or not.
        let mut worst = 0.0f32;
        for ob in &dog.obs {
            let truth = world.boxes(ob.t).into_iter().find(|b| b.0 == "dog").unwrap().1;
            worst = worst.max((ob.bbox.cx() - truth.cx()).abs() * W as f32).max((ob.bbox.cy() - truth.cy()).abs() * H as f32);
        }
        assert!(worst < 6.0, "worst tracking error {worst:.1} px");
        assert!(dog.enter <= 0.05 && dog.leave >= 3.8, "the dog is there the whole time: {} to {}", dog.enter, dog.leave);
        assert!(!dog.enters && !dog.leaves);
    }

    #[test]
    fn a_new_arrival_and_a_departure_are_timed() {
        let world = crossing_world();
        let m = run(&world, &opts(), false);
        let cat = m.named("cat").into_iter().next().expect("the cat");
        // It crosses the right edge at 1.5 s; it is first found at the key
        // frame at 2.0 s and followed back.
        assert!(cat.enter > 1.3 && cat.enter < 1.95, "cat came in at {}", cat.enter);
        assert!(cat.enters && !cat.leaves);
        let car = m.named("car").into_iter().next().expect("the car");
        assert!(car.enter <= 0.05);
        assert!(car.leaves, "the car drives out");
        assert!(car.leave > 0.4 && car.leave < 1.9, "car left at {}", car.leave);
        assert!(m.objects.iter().all(|o| o.id >= 1) && m.objects.iter().map(|o| o.id).collect::<std::collections::HashSet<_>>().len() == m.objects.len());
        // The map notes what was looked for.
        assert!(m.vocabulary.contains(&"dog".to_string()));
        assert_eq!(m.keyframes.len(), 3);
    }

    #[test]
    fn a_pan_is_the_cameras_and_a_fixed_thing_is_still() {
        // The camera pans: the whole scene slides left 60 px a second, and a
        // car parked in the scene slides with it.
        let world = World {
            frames: 90,
            pan: Box::new(|t| -60.0 * t),
            things: vec![Thing { label: "car", seed: 5, size: 70.0, at: Box::new(|_| (300.0, 120.0)), present: Box::new(|t| 300.0 - 60.0 * t > -70.0) }],
        };
        let m = run(&world, &opts(), false);
        assert!(m.camera.summary.contains("pans right"), "camera: {}", m.camera.summary);
        let car = m.named("car").into_iter().next().expect("car");
        let mv = car.movement.as_ref().expect("movement");
        assert!(mv.still, "it only moves because the camera does: {mv:?}");
    }

    #[test]
    fn distances_and_sizes_follow_the_depth_map() {
        // No car: nothing of known size, so the scale is assumed.
        let world = crossing_world_with(false);
        let m = run(&world, &opts(), true);
        let dog = m.named("dog").into_iter().next().unwrap();
        // The dog is lower in the picture than the cat: nearer.
        let cat = m.named("cat").into_iter().next().unwrap();
        let (zd, zc) = (dog.obs.iter().find(|o| o.detected).unwrap().z.unwrap(), cat.obs.iter().find(|o| o.detected).unwrap().z.unwrap());
        assert!(zd < zc, "dog {zd} m, cat {zc} m");
        assert!(dog.size.is_some());
        assert_eq!(m.scale.anchors, 0, "nothing of known size: the scale is assumed");
        // Every observation has a distance, key frame or not.
        assert!(dog.obs.iter().all(|o| o.z.is_some()));
        // The same depth gives the same distance at every key frame.
        let zs: Vec<f32> = dog.obs.iter().filter(|o| o.detected).map(|o| o.z.unwrap()).collect();
        assert!(zs.iter().all(|z| (z - zs[0]).abs() < 0.05), "{zs:?}");
    }

    #[test]
    fn a_car_of_known_height_calibrates_the_scene() {
        let world = crossing_world();
        let m = run(&world, &opts(), true);
        assert!(m.scale.anchors >= 1, "the car is an anchor");
        let car = m.named("car").into_iter().next().unwrap();
        // 70 px of 270 is a quarter of the picture height; a 1.5 m car that
        // fills it is about 1.5 / 0.26 x the focal ratio away.
        let size = car.size.expect("size");
        assert!((size[1] - 1.5).abs() < 0.35, "{size:?}");
        // The dog, lower in the picture, is nearer than the cat.
        let dog = m.named("dog").into_iter().next().unwrap();
        let cat = m.named("cat").into_iter().next().unwrap();
        let z = |o: &Object| o.obs.iter().find(|x| x.detected).unwrap().z.unwrap();
        assert!(z(dog) < z(cat), "{} vs {}", z(dog), z(cat));
    }

    #[test]
    fn the_cache_params_name_every_choice() {
        let a = BuildOptions::default();
        let mut b = a.clone();
        b.names.push("red kettle".into());
        assert_ne!(a.cache_params(), b.cache_params());
        let mut c = a.clone();
        c.max_keyframes = 3;
        assert_ne!(a.cache_params(), c.cache_params());
        // Order of names does not matter.
        let mut d = b.clone();
        d.names = vec!["Red Kettle".into()];
        assert_eq!(b.cache_params(), d.cache_params());
        assert!(a.vocabulary().contains(&"person".to_string()));
        assert_eq!(b.vocabulary()[0], "red kettle");
    }

    #[test]
    fn names_become_labels_and_parts() {
        assert_eq!(canonical("a woman").label, "person");
        assert_eq!(canonical("woman").descriptor.as_deref(), Some("woman"));
        assert_eq!(canonical("dog").kind, Kind::Animal);
        assert_eq!(canonical("face").part, Some(Part::Face));
        assert_eq!(canonical("animal head").part, Some(Part::Head));
        assert_eq!(canonical("red kettle").kind, Kind::Thing);
    }

    #[test]
    fn facing_is_read_from_the_nose_and_the_head() {
        let face = |nose_x: f32| Face { bbox: crate::features::faces::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }, score: 0.9, keypoints: [(7.0, 3.0), (3.0, 3.0), (nose_x, 5.0), (5.0, 7.0), (9.0, 4.0), (1.0, 4.0)] };
        assert_eq!(face_facing(&face(5.0)).unwrap(), "toward the camera");
        assert!(face_facing(&face(3.8)).unwrap().contains("turned left"));
        assert!(face_facing(&face(6.8)).unwrap().contains("turned right") || face_facing(&face(6.8)).unwrap().contains("profile"));
        let body = BBox::new(0.2, 0.4, 0.4, 0.3);
        assert_eq!(head_facing(&body, &BBox::new(0.5, 0.4, 0.1, 0.1)).unwrap(), "facing right");
        assert_eq!(head_facing(&body, &BBox::new(0.2, 0.4, 0.1, 0.1)).unwrap(), "facing left");
        assert_eq!(head_facing(&body, &BBox::new(0.3, 0.4, 0.2, 0.2)).unwrap(), "facing the camera");
    }
}
