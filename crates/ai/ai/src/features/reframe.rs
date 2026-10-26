// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Auto reframe: turns a 16:9 clip into 9:16, 1:1, 4:5 … by moving a crop
//! window that follows what matters, like a camera operator would.
//!
//! 1. Every few frames the subject is located: faces (searched in square
//!    tiles across a wide frame too, so small faces in wide shots are found)
//!    when the face model is installed; otherwise the people in shot (the
//!    person matte's centre of mass, head weighted) when the matting model
//!    is installed; otherwise a saliency map (spectral residual, Hou & Zhang
//!    2007, combined with motion), which follows the most striking area.
//! 2. Per shot (cuts reset the camera), the target positions go through a
//!    dead zone — the camera holds still while the subject stays near the
//!    middle of the crop — and then a zero-phase smoothing pass, so the path
//!    has no jitter and no lag.
//! 3. The crop is clamped to the frame and returned for every frame.
//!
//! The result is a proposal: the UI shows it and the user accepts it as
//! keyframes on the clip's crop.

use crate::cache::{Cacheable, Reader, Writer};
use crate::error::{invalid, Result};
use crate::features::faces::{Face, FaceDetector};
use crate::features::matting::BackgroundRemover;
use crate::frame::{Frame, FrameSource};
use crate::jobs::JobCtx;
use crate::store::ModelStore;
use studio_nn::audio::Fft;

#[derive(Clone, Debug, PartialEq)]
pub struct ReframeSettings {
    /// Target width / height (9/16 for vertical video).
    pub aspect: f32,
    /// 1 = the largest crop of that aspect; 1.5 = 1.5× tighter.
    pub zoom: f32,
    /// 0 = follow tightly, 1 = very calm camera.
    pub smoothing: f32,
    /// Subject samples per second.
    pub samples_per_second: f64,
}

impl Default for ReframeSettings {
    fn default() -> Self {
        ReframeSettings { aspect: 9.0 / 16.0, zoom: 1.0, smoothing: 0.5, samples_per_second: 6.0 }
    }
}

impl ReframeSettings {
    pub fn cache_params(&self, faces: bool) -> String {
        format!("v1 aspect={:.4} zoom={:.3} smooth={:.3} sps={:.2} faces={faces}", self.aspect, self.zoom, self.smoothing, self.samples_per_second)
    }
}

/// The crop rectangle for one frame, in source pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropKey {
    pub frame: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What steered the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectSource {
    Faces,
    People,
    Saliency,
}

pub struct Reframer {
    faces: Option<FaceDetector>,
    people: Option<BackgroundRemover>,
}

impl Reframer {
    /// Uses the face detector when installed (better framing of people),
    /// saliency otherwise. Never fails for a missing model.
    pub fn new(store: &ModelStore) -> Reframer {
        Reframer { faces: FaceDetector::load(store).ok(), people: BackgroundRemover::load(store).ok() }
    }

    pub fn without_models() -> Reframer {
        Reframer { faces: None, people: None }
    }

    pub fn source(&self) -> SubjectSource {
        if self.faces.is_some() {
            SubjectSource::Faces
        } else if self.people.is_some() {
            SubjectSource::People
        } else {
            SubjectSource::Saliency
        }
    }

    /// Faces in the whole frame and in square tiles across it (a wide frame
    /// shrunk whole leaves faces too small for the detector); tall frames
    /// are tiled top to bottom.
    pub fn find_faces(fd: &FaceDetector, f: &Frame) -> Result<Vec<Face>> {
        let mut all = fd.detect(f)?;
        if f.height > f.width * 5 / 4 {
            let side = f.width;
            let n = (f.height as f32 / side as f32).ceil() as usize + 1;
            for k in 0..n {
                let y0 = ((f.height - side) * k / (n - 1).max(1)).min(f.height - side);
                let tile = Frame { width: side, height: side, rgba: f.rgba[y0 * f.width * 4..(y0 + side) * f.width * 4].to_vec() };
                for mut face in fd.detect(&tile)? {
                    face.bbox.y += y0 as f32;
                    for kp in &mut face.keypoints {
                        kp.1 += y0 as f32;
                    }
                    match all.iter_mut().find(|o| o.bbox.iou(&face.bbox) > 0.3) {
                        Some(o) if o.score < face.score => *o = face,
                        Some(_) => {}
                        None => all.push(face),
                    }
                }
            }
        }
        if f.width > f.height * 5 / 4 {
            let side = f.height;
            let n = (f.width as f32 / side as f32).ceil() as usize + 1;
            for k in 0..n {
                let x0 = ((f.width - side) * k / (n - 1).max(1)).min(f.width - side);
                let tile = crop_square(f, x0, side);
                for mut face in fd.detect(&tile)? {
                    face.bbox.x += x0 as f32;
                    for kp in &mut face.keypoints {
                        kp.0 += x0 as f32;
                    }
                    match all.iter_mut().find(|o| o.bbox.iou(&face.bbox) > 0.3) {
                        Some(o) if o.score < face.score => *o = face,
                        Some(_) => {}
                        None => all.push(face),
                    }
                }
            }
        }
        Ok(all)
    }

    /// Plans the crop for every frame. `cuts` are first frames of new shots
    /// (from [`crate::features::scenes`]); the camera jumps there instead of
    /// panning across.
    pub fn plan(&self, src: &mut dyn FrameSource, settings: &ReframeSettings, cuts: &[usize], ctx: &JobCtx) -> Result<Vec<CropKey>> {
        let n = src.frame_count();
        if n == 0 {
            return Ok(Vec::new());
        }
        let first = src.frame(0)?;
        let (fw, fh) = (first.width as f32, first.height as f32);
        let (cw, ch) = crop_size(fw, fh, settings.aspect, settings.zoom)?;
        let step = ((src.fps().max(1.0) / settings.samples_per_second.max(0.1)).round() as usize).max(1);
        let mut samples: Vec<(usize, (f32, f32))> = Vec::new();
        let mut prev_thumb: Option<Vec<f32>> = None;
        let mut i = 0;
        while i < n {
            ctx.check()?;
            ctx.set_progress(0.9 * i as f32 / n as f32);
            ctx.set_message(format!("Finding the subject: frame {} / {n}", i + 1));
            let f = if i == 0 { first.clone() } else { src.frame(i)? };
            let thumb = small_luma(&f);
            let p = self.subject(&f, &thumb, prev_thumb.as_deref(), cuts.contains(&i))?;
            samples.push((i, p));
            prev_thumb = Some(thumb);
            // always sample the first frame of every shot
            let next = i + step;
            i = cuts.iter().copied().filter(|&c| c > i && c < next).min().unwrap_or(next);
        }
        let mut keys = Vec::with_capacity(n);
        // split samples into shots and plan each
        let mut bounds: Vec<usize> = cuts.iter().copied().filter(|&c| c > 0 && c < n).collect();
        bounds.sort_unstable();
        bounds.dedup();
        bounds.push(n);
        let mut start = 0;
        for &end in &bounds {
            let shot: Vec<(usize, (f32, f32))> = samples.iter().copied().filter(|(f, _)| *f >= start && *f < end).collect();
            let xs = camera_path(&shot.iter().map(|s| (s.0, s.1 .0)).collect::<Vec<_>>(), cw, fw, settings.smoothing);
            let ys = camera_path(&shot.iter().map(|s| (s.0, s.1 .1)).collect::<Vec<_>>(), ch, fh, settings.smoothing);
            for f in start..end {
                let cx = sample_at(&xs, f).unwrap_or(fw / 2.0);
                let cy = sample_at(&ys, f).unwrap_or(fh / 2.0);
                keys.push(CropKey { frame: f, x: (cx - cw / 2.0).clamp(0.0, fw - cw), y: (cy - ch / 2.0).clamp(0.0, fh - ch), w: cw, h: ch });
            }
            start = end;
        }
        ctx.set_progress(1.0);
        Ok(keys)
    }

    /// The point to keep in frame.
    fn subject(&self, f: &Frame, thumb: &[f32], prev: Option<&[f32]>, at_cut: bool) -> Result<(f32, f32)> {
        if let Some(fd) = &self.faces {
            let faces = Self::find_faces(fd, f)?;
            if !faces.is_empty() {
                // weight by area: the main speaker dominates, but a group
                // stays centred between its members
                let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
                for face in &faces {
                    let w = face.bbox.area() * face.score;
                    let (cx, cy) = face.bbox.centre();
                    sx += cx * w;
                    sy += (cy + face.bbox.h * 0.3) * w; // aim a little below the eyes: head and shoulders
                    sw += w;
                }
                return Ok((sx / sw, sy / sw));
            }
        }
        // People: aim at heads, like a camera operator. Find the band of
        // rows the people occupy, then centre on the top part of it (head
        // and shoulders) - between everyone's heads when there are several.
        if let Some(pm) = &self.people {
            if let Ok(m) = pm.matte(f) {
                let (w, h) = (m.width, m.height);
                let row_cover: Vec<usize> = (0..h).map(|y| m.data[y * w..(y + 1) * w].iter().filter(|a| **a > 0.4).count()).collect();
                let on: usize = row_cover.iter().sum();
                let min_row = (w / 50).max(1);
                let top = row_cover.iter().position(|&c| c >= min_row);
                let bottom = row_cover.iter().rposition(|&c| c >= min_row);
                if let (Some(top), Some(bottom)) = (top, bottom) {
                    if on * 100 > w * h && bottom > top {
                        let span = bottom - top;
                        let head_end = top + (span as f32 * 0.3) as usize + 1;
                        let (mut sx, mut sw) = (0.0f64, 0.0f64);
                        for y in top..head_end.min(h) {
                            for x in 0..w {
                                let a = m.data[y * w + x];
                                if a > 0.4 {
                                    sx += x as f64 * a as f64;
                                    sw += a as f64;
                                }
                            }
                        }
                        if sw > 0.0 {
                            let (kx, ky) = (f.width as f64 / w as f64, f.height as f64 / h as f64);
                            let aim_y = top as f64 + span as f64 * 0.28;
                            return Ok(((sx / sw * kx) as f32, (aim_y * ky) as f32));
                        }
                    }
                }
            }
        }
        let motion = if at_cut { None } else { prev };
        let (u, v) = saliency_centre(thumb, motion);
        Ok((u * f.width as f32, v * f.height as f32))
    }
}

/// Largest `aspect` crop inside `fw × fh`, shrunk by `zoom`.
pub fn crop_size(fw: f32, fh: f32, aspect: f32, zoom: f32) -> Result<(f32, f32)> {
    if !(aspect > 0.0 && zoom >= 1.0) {
        return invalid("aspect must be positive and zoom at least 1");
    }
    let (w, h) = if fw / fh > aspect { (fh * aspect, fh) } else { (fw, fw / aspect) };
    Ok((w / zoom, h / zoom))
}

/// Smooth camera centre positions for one axis of one shot from subject
/// samples `(frame, position)`: dead zone, then zero-phase smoothing,
/// clamped so the crop (size `crop`) stays inside `extent`.
fn camera_path(samples: &[(usize, f32)], crop: f32, extent: f32, smoothing: f32) -> Vec<(usize, f32)> {
    if samples.is_empty() {
        return Vec::new();
    }
    let (lo, hi) = (crop / 2.0, extent - crop / 2.0);
    if hi <= lo {
        return samples.iter().map(|&(f, _)| (f, extent / 2.0)).collect();
    }
    let s = smoothing.clamp(0.0, 1.0);
    // dead zone: the camera only moves once the subject leaves the middle
    let dz = crop * (0.05 + 0.2 * s);
    let mut cam = samples[0].1.clamp(lo, hi);
    let mut held: Vec<f32> = Vec::with_capacity(samples.len());
    for &(_, p) in samples {
        let d = p - cam;
        if d.abs() > dz {
            cam += d - dz * d.signum();
        }
        held.push(cam.clamp(lo, hi));
    }
    // forward-backward exponential smoothing (no lag)
    let a = 0.15 + 0.8 * s;
    for i in 1..held.len() {
        held[i] = held[i - 1] * a + held[i] * (1.0 - a);
    }
    for i in (0..held.len().saturating_sub(1)).rev() {
        held[i] = held[i + 1] * a + held[i] * (1.0 - a);
    }
    samples.iter().zip(held).map(|(&(f, _), c)| (f, c.clamp(lo, hi))).collect()
}

/// Linear interpolation of a sampled path at frame `f`.
fn sample_at(path: &[(usize, f32)], f: usize) -> Option<f32> {
    let i = path.partition_point(|p| p.0 <= f);
    match (i.checked_sub(1).map(|k| path[k]), path.get(i)) {
        (Some(a), Some(b)) => Some(a.1 + (b.1 - a.1) * (f - a.0) as f32 / (b.0 - a.0).max(1) as f32),
        (Some(a), None) => Some(a.1),
        (None, Some(b)) => Some(b.1),
        (None, None) => None,
    }
}

const S: usize = 64;

/// 64×64 luma thumbnail (aspect not preserved: saliency only needs the
/// relative layout).
fn small_luma(f: &Frame) -> Vec<f32> {
    let l = f.luma();
    studio_nn::image::resize_plane(&l, f.width, f.height, S, S)
}

/// Normalised (u, v) centre of the most salient region: spectral-residual
/// saliency, plus motion relative to the previous thumbnail.
pub fn saliency_centre(thumb: &[f32], prev: Option<&[f32]>) -> (f32, f32) {
    let mut sal = spectral_residual(thumb);
    normalise(&mut sal);
    if let Some(p) = prev {
        let mut m: Vec<f32> = thumb.iter().zip(p).map(|(a, b)| (a - b).abs()).collect();
        m = crate::frame::box_mean(&m, S, S, 2);
        let peak = m.iter().copied().fold(0.0, f32::max);
        // ignore sensor noise; real motion is several grey levels
        if peak > 0.04 {
            normalise(&mut m);
            for (s, mv) in sal.iter_mut().zip(&m) {
                *s = 0.5 * *s + 0.5 * mv;
            }
        }
    }
    // centroid of the top 5 %
    let mut sorted = sal.clone();
    sorted.sort_by(f32::total_cmp);
    let cut = sorted[sorted.len() * 95 / 100];
    let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
    for (i, &v) in sal.iter().enumerate() {
        if v >= cut && v > 0.0 {
            sx += (i % S) as f32 * v;
            sy += (i / S) as f32 * v;
            sw += v;
        }
    }
    if sw <= 0.0 {
        return (0.5, 0.5);
    }
    ((sx / sw + 0.5) / S as f32, (sy / sw + 0.5) / S as f32)
}

fn normalise(v: &mut [f32]) {
    let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |a, &x| (a.0.min(x), a.1.max(x)));
    let span = (hi - lo).max(1e-9);
    v.iter_mut().for_each(|x| *x = (*x - lo) / span);
}

/// 2-D FFT in place (rows then columns) of an `S × S` complex image.
fn fft2(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let fft = Fft::new(S);
    let (mut r, mut i) = (vec![0.0f32; S], vec![0.0f32; S]);
    for y in 0..S {
        fft.run(&mut re[y * S..(y + 1) * S], &mut im[y * S..(y + 1) * S], inverse);
    }
    for x in 0..S {
        for y in 0..S {
            r[y] = re[y * S + x];
            i[y] = im[y * S + x];
        }
        fft.run(&mut r, &mut i, inverse);
        for y in 0..S {
            re[y * S + x] = r[y];
            im[y * S + x] = i[y];
        }
    }
}

/// Spectral residual saliency (Hou & Zhang, CVPR 2007): what is left of
/// the log amplitude spectrum after removing its local average marks the
/// "unexpected" parts of the image.
fn spectral_residual(img: &[f32]) -> Vec<f32> {
    let mut re = img.to_vec();
    let mut im = vec![0.0f32; S * S];
    fft2(&mut re, &mut im, false);
    let logamp: Vec<f32> = re.iter().zip(&im).map(|(a, b)| (a.hypot(*b) + 1e-9).ln()).collect();
    let avg = crate::frame::box_mean(&logamp, S, S, 1);
    for k in 0..S * S {
        let phase = im[k].atan2(re[k]);
        let mag = (logamp[k] - avg[k]).exp();
        re[k] = mag * phase.cos();
        im[k] = mag * phase.sin();
    }
    fft2(&mut re, &mut im, true);
    let sal: Vec<f32> = re.iter().zip(&im).map(|(a, b)| a * a + b * b).collect();
    // blur (≈ Gaussian, three box passes)
    let mut s = sal;
    for _ in 0..3 {
        s = crate::frame::box_mean(&s, S, S, 2);
    }
    s
}

impl Cacheable for CropKey {
    const TAG: &'static str = "crop-key-v1";
    fn encode(&self, w: &mut Writer) {
        w.u64(self.frame as u64);
        for v in [self.x, self.y, self.w, self.h] {
            w.f32(v);
        }
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        Some(CropKey { frame: r.u64()? as usize, x: r.f32()?, y: r.f32()?, w: r.f32()?, h: r.f32()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::VecSource;

    /// Plain grey with a bright textured square at (x, y).
    fn scene(w: usize, h: usize, x: usize, y: usize) -> Frame {
        let mut f = Frame::filled(w, h, [90, 90, 90, 255]);
        for yy in y..(y + 24).min(h) {
            for xx in x..(x + 24).min(w) {
                let v = if (xx / 4 + yy / 4) % 2 == 0 { 250 } else { 20 };
                f.rgba[(yy * w + xx) * 4..][..3].copy_from_slice(&[v, v / 2, 0]);
            }
        }
        f
    }

    #[test]
    fn crop_sizes() {
        assert_eq!(crop_size(1920.0, 1080.0, 9.0 / 16.0, 1.0).unwrap(), (607.5, 1080.0));
        assert_eq!(crop_size(1080.0, 1920.0, 1.0, 2.0).unwrap(), (540.0, 540.0));
        assert!(crop_size(100.0, 100.0, 1.0, 0.5).is_err());
    }

    #[test]
    fn saliency_finds_the_object() {
        let f = scene(320, 180, 240, 40);
        let (u, v) = saliency_centre(&small_luma(&f), None);
        assert!(u > 0.65 && v < 0.5, "({u}, {v})");
    }

    #[test]
    fn follows_the_subject_smoothly_and_stays_inside() {
        // the object moves from left to right over 60 frames
        let frames: Vec<Frame> = (0..60).map(|i| scene(320, 180, 10 + i * 4, 80)).collect();
        let mut src = VecSource { frames, fps: 30.0 };
        let r = Reframer::without_models();
        let keys = r.plan(&mut src, &ReframeSettings { samples_per_second: 15.0, ..Default::default() }, &[], &JobCtx::detached()).unwrap();
        assert_eq!(keys.len(), 60);
        for k in &keys {
            assert!(k.x >= 0.0 && k.x + k.w <= 320.0 + 1e-3 && (k.h - 180.0).abs() < 1e-3);
        }
        assert!(keys[59].x > keys[0].x + 80.0, "camera follows: {} → {}", keys[0].x, keys[59].x);
        // no frame-to-frame jumps
        for w in keys.windows(2) {
            assert!((w[1].x - w[0].x).abs() < 12.0, "jump at {}: {} → {}", w[1].frame, w[0].x, w[1].x);
        }
    }

    #[test]
    fn cuts_reset_the_camera() {
        let mut frames: Vec<Frame> = (0..20).map(|_| scene(320, 180, 20, 80)).collect();
        frames.extend((0..20).map(|_| scene(320, 180, 270, 80)));
        let mut src = VecSource { frames, fps: 30.0 };
        let keys = Reframer::without_models().plan(&mut src, &ReframeSettings::default(), &[20], &JobCtx::detached()).unwrap();
        assert!(keys[19].x < 60.0, "{}", keys[19].x);
        assert!(keys[20].x > 150.0, "jumps at the cut: {}", keys[20].x);
    }

    #[test]
    fn interpolation_between_samples() {
        let p = vec![(0, 0.0), (10, 10.0)];
        assert_eq!(sample_at(&p, 5), Some(5.0));
        assert_eq!(sample_at(&p, 20), Some(10.0));
        assert_eq!(sample_at(&[], 1), None);
    }
}

/// A `side` × `side` square of `f` starting at column `x0`.
fn crop_square(f: &Frame, x0: usize, side: usize) -> Frame {
    let mut rgba = Vec::with_capacity(side * side * 4);
    for y in 0..side.min(f.height) {
        let row = (y * f.width + x0) * 4;
        rgba.extend_from_slice(&f.rgba[row..row + side * 4]);
    }
    Frame { width: side, height: side.min(f.height), rgba }
}
