// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Point and box tracking that produce keyframes for a clip's transform.
//!
//! [`PointTracker`] follows a single feature with pyramidal KLT.
//! [`BoxTracker`] follows a rectangle by normalised cross-correlation
//! against a slowly updated template, searching a few scales so it copes
//! with objects that approach or recede.

use crate::features::{klt, Pyramid};
use crate::image::Image;
use crate::ops::Gray;

/// One tracked frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackSample {
    /// Centre of the tracked point or box, in pixels.
    pub position: [f32; 2],
    /// Size relative to the first frame (always 1 for points).
    pub scale: f32,
    /// 0..1: how sure the tracker is.
    pub confidence: f32,
    /// True when the target was lost; position and scale then repeat the
    /// last good values.
    pub lost: bool,
}

/// Follows one point from frame to frame.
pub struct PointTracker {
    pyr: Pyramid,
    point: [f32; 2],
    velocity: [f32; 2],
}

impl PointTracker {
    /// Starts tracking `point` (pixels) in `frame`.
    pub fn new(frame: &Image, point: [f32; 2]) -> Self {
        PointTracker { pyr: Pyramid::new(&Gray::from_image(frame), 4), point, velocity: [0.0; 2] }
    }

    /// Finds the point in the next frame.
    pub fn track(&mut self, frame: &Image) -> TrackSample {
        let next = Pyramid::new(&Gray::from_image(frame), 4);
        let half = 10;
        let result = klt(&self.pyr, &next, self.point, self.velocity, half).and_then(|f| {
            let back = klt(&next, &self.pyr, f.position, [self.point[0] - f.position[0], self.point[1] - f.position[1]], half)?;
            let fb = ((back.position[0] - self.point[0]).powi(2) + (back.position[1] - self.point[1]).powi(2)).sqrt();
            Some((f, fb))
        });
        let sample = match result {
            Some((f, fb)) if fb < 2.0 => {
                let conf = (-fb).exp() * (-f.residual * 10.0).exp();
                self.velocity = [f.position[0] - self.point[0], f.position[1] - self.point[1]];
                self.point = f.position;
                TrackSample { position: f.position, scale: 1.0, confidence: conf.clamp(0.0, 1.0), lost: false }
            }
            _ => TrackSample { position: self.point, scale: 1.0, confidence: 0.0, lost: true },
        };
        self.pyr = next;
        sample
    }
}

/// Follows a rectangle from frame to frame.
pub struct BoxTracker {
    template: Gray,
    /// Frame pixels per template pixel at scale 1.
    step: f32,
    center: [f32; 2],
    scale: f32,
}

/// Template resolution: the box is resampled to at most this many pixels
/// on its longer side, which keeps the search fast at any frame size.
const TEMPLATE_MAX: f32 = 40.0;

impl BoxTracker {
    /// Starts tracking the box `[x, y, width, height]` (pixels) in `frame`.
    pub fn new(frame: &Image, rect: [f32; 4]) -> Self {
        let gray = Gray::from_image(frame);
        let (w, h) = (rect[2].max(4.0), rect[3].max(4.0));
        let step = (w.max(h) / TEMPLATE_MAX).max(1.0);
        let (tw, th) = ((w / step).round().max(4.0) as usize, (h / step).round().max(4.0) as usize);
        let center = [rect[0] + w * 0.5, rect[1] + h * 0.5];
        let template = sample_patch(&gray, center, step, tw, th);
        BoxTracker { template, step, center, scale: 1.0 }
    }

    /// Current box as `[x, y, width, height]`.
    pub fn rect(&self) -> [f32; 4] {
        let (w, h) = (self.template.width as f32 * self.step * self.scale, self.template.height as f32 * self.step * self.scale);
        [self.center[0] - w * 0.5, self.center[1] - h * 0.5, w, h]
    }

    /// Finds the box in the next frame.
    pub fn track(&mut self, frame: &Image) -> TrackSample {
        let gray = Gray::from_image(frame);
        let (tw, th) = (self.template.width, self.template.height);
        let mut best = (f32::NEG_INFINITY, self.center, self.scale);
        // Coarse search on a grid of template pixels over a window as big
        // as the box, at three scales; then refine around the winner.
        let reach = tw.max(th) as i32 / 2 + 2;
        for &ds in &[0.95f32, 1.0, 1.05] {
            let s = self.scale * ds;
            let k = self.step * s;
            for gy in (-reach..=reach).step_by(2) {
                for gx in (-reach..=reach).step_by(2) {
                    let c = [self.center[0] + gx as f32 * k, self.center[1] + gy as f32 * k];
                    let score = ncc(&self.template, &sample_patch(&gray, c, k, tw, th));
                    if score > best.0 {
                        best = (score, c, s);
                    }
                }
            }
        }
        // Refine: shrinking steps around the best match.
        let mut step = self.step * best.2;
        for _ in 0..4 {
            let (score0, c0, s) = best;
            let mut local = (score0, c0, s);
            for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (1.0, -1.0)] {
                let c = [c0[0] + dx * step, c0[1] + dy * step];
                let score = ncc(&self.template, &sample_patch(&gray, c, self.step * s, tw, th));
                if score > local.0 {
                    local = (score, c, s);
                }
            }
            best = local;
            step *= 0.5;
        }
        let (score, c, s) = best;
        let confidence = score.clamp(0.0, 1.0);
        if confidence < 0.4 {
            return TrackSample { position: self.center, scale: self.scale, confidence, lost: true };
        }
        self.center = c;
        self.scale = s;
        if confidence > 0.8 {
            // Let the template follow slow changes in lighting and pose,
            // but only when the match is good, so it does not drift onto
            // the background.
            let fresh = sample_patch(&gray, c, self.step * s, tw, th);
            for (t, f) in self.template.data.iter_mut().zip(&fresh.data) {
                *t = *t * 0.9 + f * 0.1;
            }
        }
        TrackSample { position: c, scale: s, confidence, lost: false }
    }
}

fn sample_patch(g: &Gray, center: [f32; 2], step: f32, tw: usize, th: usize) -> Gray {
    let mut out = Gray::new(tw, th);
    for y in 0..th {
        for x in 0..tw {
            let px = center[0] + (x as f32 + 0.5 - tw as f32 * 0.5) * step;
            let py = center[1] + (y as f32 + 0.5 - th as f32 * 0.5) * step;
            out.data[y * tw + x] = g.sample(px, py);
        }
    }
    out
}

/// Normalised cross-correlation in -1..1 (0 for flat patches).
fn ncc(a: &Gray, b: &Gray) -> f32 {
    let n = a.data.len() as f32;
    let (ma, mb) = (a.data.iter().sum::<f32>() / n, b.data.iter().sum::<f32>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0f32, 0.0f32, 0.0f32);
    for (&x, &y) in a.data.iter().zip(&b.data) {
        let (dx, dy) = (x - ma, y - mb);
        sab += dx * dy;
        saa += dx * dx;
        sbb += dy * dy;
    }
    if saa < 1e-9 || sbb < 1e-9 {
        return 0.0;
    }
    sab / (saa * sbb).sqrt()
}

/// A keyframe for a clip's transform written by a tracker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformKey {
    /// Seconds from the first tracked frame.
    pub time: f32,
    /// Position offset from the first frame, as a fraction of the frame
    /// width (x) and height (y): add it to a layer's position to make the
    /// layer follow the target.
    pub offset: [f32; 2],
    /// Scale relative to the first frame.
    pub scale: f32,
}

/// Turns track samples into transform keyframes, dropping keys that a
/// straight line between their neighbours already explains to within
/// `tolerance` (fraction of the frame). Lost frames are skipped.
pub fn to_keyframes(samples: &[TrackSample], frame_rate: f32, width: usize, height: usize, tolerance: f32) -> Vec<TransformKey> {
    let Some(first) = samples.iter().find(|s| !s.lost) else { return Vec::new() };
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let keys: Vec<TransformKey> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.lost)
        .map(|(i, s)| TransformKey {
            time: i as f32 / frame_rate.max(1e-3),
            offset: [(s.position[0] - first.position[0]) / w, (s.position[1] - first.position[1]) / h],
            scale: s.scale / first.scale,
        })
        .collect();
    simplify(&keys, tolerance.max(0.0))
}

/// Ramer-Douglas-Peucker over time, on offset and scale together.
fn simplify(keys: &[TransformKey], tol: f32) -> Vec<TransformKey> {
    if keys.len() <= 2 {
        return keys.to_vec();
    }
    let mut keep = vec![false; keys.len()];
    keep[0] = true;
    keep[keys.len() - 1] = true;
    let mut stack = vec![(0, keys.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (ka, kb) = (keys[a], keys[b]);
        let span = (kb.time - ka.time).max(1e-9);
        let mut worst = (0.0f32, 0);
        for (i, k) in keys.iter().enumerate().take(b).skip(a + 1) {
            let t = (k.time - ka.time) / span;
            let lerp = |x: f32, y: f32| x + (y - x) * t;
            let err = (k.offset[0] - lerp(ka.offset[0], kb.offset[0]))
                .abs()
                .max((k.offset[1] - lerp(ka.offset[1], kb.offset[1])).abs())
                .max((k.scale - lerp(ka.scale, kb.scale)).abs());
            if err > worst.0 {
                worst = (err, i);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    keys.iter().zip(&keep).filter(|&(_, &k)| k).map(|(k, _)| *k).collect()
}
