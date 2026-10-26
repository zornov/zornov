// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Video stabilisation: estimate the camera's frame-to-frame motion from
//! tracked corners, smooth the path, and work out a per-frame correction
//! that stays within a crop limit.
//!
//! Usage: feed frames in order with [`Stabiliser::push`], then call
//! [`Stabiliser::solve`]. Apply each [`FrameCorrection`] with
//! [`FrameCorrection::apply`] (or the renderer's own transform).

use crate::features::{fast_corners, klt, ransac_similarity, spread_corners, Pyramid, Similarity};
use crate::image::{Edge, Image};
use crate::ops::Gray;

/// How the correction treats intended camera moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StabiliseMode {
    /// Keep the intended move, remove the shake.
    Smooth,
    /// Hold the first frame's framing as if on a tripod.
    Lock,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StabiliseSettings {
    /// Width of the smoothing window in seconds. Larger is steadier.
    pub smoothness: f32,
    /// Largest fraction of the frame that may be cropped away (0..0.5).
    pub max_crop: f32,
    pub mode: StabiliseMode,
    /// Frames are analysed at about this height in pixels.
    pub analysis_height: usize,
    pub max_features: usize,
}

impl Default for StabiliseSettings {
    fn default() -> Self {
        StabiliseSettings { smoothness: 1.0, max_crop: 0.1, mode: StabiliseMode::Smooth, analysis_height: 360, max_features: 300 }
    }
}

/// The correction for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameCorrection {
    /// Maps a point of the original frame (pixels, origin at the frame
    /// centre) to where it should appear in the stabilised frame.
    pub transform: Similarity,
    /// Zoom about the centre applied after `transform` to hide the edges.
    /// The same for every frame of a clip so the framing does not pump.
    pub zoom: f32,
}

impl FrameCorrection {
    pub const NONE: FrameCorrection = FrameCorrection { transform: Similarity::IDENTITY, zoom: 1.0 };

    /// Full mapping from original-frame pixels (top-left origin) to output
    /// pixels.
    pub fn output_from_source(&self, w: f32, h: f32) -> Similarity {
        let to_c = Similarity { tx: -w * 0.5, ty: -h * 0.5, ..Similarity::IDENTITY };
        let zoom = Similarity::from_parts(self.zoom, 0.0, 0.0, 0.0);
        let back = Similarity { tx: w * 0.5, ty: h * 0.5, ..Similarity::IDENTITY };
        back.compose(&zoom.compose(&self.transform.compose(&to_c)))
    }

    /// Resamples a frame with this correction.
    pub fn apply(&self, img: &Image) -> Image {
        let inv = self.output_from_source(img.width as f32, img.height as f32).inverse();
        crate::ops::warp(img, Edge::Clamp, |x, y| {
            let s = inv.apply([x, y]);
            (s[0], s[1])
        })
    }
}

/// Collects frame-to-frame motion; see the module docs.
pub struct Stabiliser {
    settings: StabiliseSettings,
    prev: Option<(Pyramid, Vec<[f32; 2]>)>,
    /// Motion from frame k-1 to frame k (analysis pixels, centred), with
    /// `motions[0]` the identity for the first frame.
    motions: Vec<Similarity>,
    scale: f32,
    size: (usize, usize),
}

impl Stabiliser {
    pub fn new(settings: StabiliseSettings) -> Self {
        Stabiliser { settings, prev: None, motions: Vec::new(), scale: 1.0, size: (0, 0) }
    }

    /// Number of frames pushed so far.
    pub fn len(&self) -> usize {
        self.motions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.motions.is_empty()
    }

    /// Analyses the next frame of the clip.
    pub fn push(&mut self, frame: &Image) {
        let mut gray = Gray::from_image(frame);
        let mut scale = 1.0;
        while gray.height > self.settings.analysis_height.max(32) * 2 - 1 {
            gray = gray.half();
            scale *= 2.0;
        }
        self.scale = scale;
        self.size = (frame.width, frame.height);
        let pyr = Pyramid::new(&gray, 3);
        let (w, h) = (gray.width as f32, gray.height as f32);
        let motion = match &self.prev {
            None => Similarity::IDENTITY,
            Some((prev_pyr, pts)) => {
                let mut pairs = Vec::with_capacity(pts.len());
                for &p in pts {
                    let Some(f) = klt(prev_pyr, &pyr, p, [0.0, 0.0], 7) else { continue };
                    // Forward-backward check drops points that slid along
                    // edges or got occluded.
                    let Some(b) = klt(&pyr, prev_pyr, f.position, [p[0] - f.position[0], p[1] - f.position[1]], 7) else {
                        continue;
                    };
                    if (b.position[0] - p[0]).powi(2) + (b.position[1] - p[1]).powi(2) > 0.25 {
                        continue;
                    }
                    let c = |q: [f32; 2]| [q[0] - w * 0.5, q[1] - h * 0.5];
                    pairs.push((c(p), c(f.position)));
                }
                ransac_similarity(&pairs, 1.0, 300, self.motions.len() as u32)
                    .filter(|&(_, n)| n >= 8)
                    .map(|(m, _)| m)
                    // Without enough evidence assume the camera held still
                    // rather than inventing a jump.
                    .unwrap_or(Similarity::IDENTITY)
            }
        };
        self.motions.push(motion);
        let corners = fast_corners(&pyr.levels[0], 0.04);
        let pts = spread_corners(corners, gray.width, gray.height, self.settings.max_features);
        self.prev = Some((pyr, pts));
    }

    /// Estimated motion from the previous frame to frame `k`, in full-size
    /// pixels with the origin at the frame centre.
    pub fn motion(&self, k: usize) -> Similarity {
        let m = self.motions[k];
        Similarity { tx: m.tx * self.scale, ty: m.ty * self.scale, ..m }
    }

    /// Computes the correction for every frame pushed.
    pub fn solve(&self, frame_rate: f32) -> Vec<FrameCorrection> {
        let n = self.motions.len();
        if n == 0 {
            return Vec::new();
        }
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        // Camera path: where frame 0's content sits in frame k.
        let mut path = Vec::with_capacity(n);
        let mut c = Similarity::IDENTITY;
        for k in 0..n {
            c = self.motion(k).compose(&c);
            path.push(c);
        }
        // Smooth the path's parameters (translation, angle, log scale).
        let params: Vec<[f32; 4]> = path.iter().map(|s| [s.tx, s.ty, s.angle(), s.scale().max(1e-6).ln()]).collect();
        let smooth: Vec<[f32; 4]> = match self.settings.mode {
            StabiliseMode::Lock => vec![[0.0; 4]; n],
            StabiliseMode::Smooth => {
                let sigma = (self.settings.smoothness.max(0.0) * frame_rate.max(1.0)).max(1e-3);
                let r = (sigma * 3.0).ceil() as isize;
                (0..n as isize)
                    .map(|k| {
                        let mut acc = [0.0f32; 4];
                        let mut wsum = 0.0;
                        for d in -r..=r {
                            // Mirror at the ends so the path is not dragged
                            // towards the first or last frame.
                            let mut j = k + d;
                            if j < 0 {
                                j = -j;
                            }
                            if j >= n as isize {
                                j = 2 * (n as isize - 1) - j;
                            }
                            let j = j.clamp(0, n as isize - 1) as usize;
                            let wgt = (-(d * d) as f32 / (2.0 * sigma * sigma)).exp();
                            for i in 0..4 {
                                acc[i] += params[j][i] * wgt;
                            }
                            wsum += wgt;
                        }
                        acc.map(|v| v / wsum)
                    })
                    .collect()
            }
        };
        let zoom_limit = 1.0 / (1.0 - self.settings.max_crop.clamp(0.0, 0.5));
        let mut corrections: Vec<Similarity> = (0..n)
            .map(|k| {
                let s = smooth[k];
                let target = Similarity::from_parts(s[3].exp(), s[2], s[0], s[1]);
                // Undo the measured path, then follow the smooth one.
                let full = target.compose(&path[k].inverse());
                // Scale back corrections that would need more than the crop
                // limit to hide their edges.
                if required_zoom(&full, w, h) <= zoom_limit {
                    return full;
                }
                let (mut lo, mut hi) = (0.0f32, 1.0f32);
                for _ in 0..20 {
                    let mid = 0.5 * (lo + hi);
                    if required_zoom(&blend(&full, mid), w, h) <= zoom_limit {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                blend(&full, lo)
            })
            .collect();
        let zoom = corrections.iter().map(|c| required_zoom(c, w, h)).fold(1.0f32, f32::max).min(zoom_limit);
        corrections.iter_mut().map(|c| FrameCorrection { transform: *c, zoom }).collect()
    }
}

/// A fraction `t` of the way from identity to `s`, parameter-wise.
fn blend(s: &Similarity, t: f32) -> Similarity {
    let scale = s.scale().max(1e-6).ln() * t;
    Similarity::from_parts(scale.exp(), s.angle() * t, s.tx * t, s.ty * t)
}

/// Smallest zoom (about the centre, applied after `s`) at which every
/// output pixel maps back inside the original frame.
fn required_zoom(s: &Similarity, w: f32, h: f32) -> f32 {
    let inv = s.inverse();
    let fits = |z: f32| {
        [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].iter().all(|&(sx, sy)| {
            let p = inv.apply([sx * w * 0.5 / z, sy * h * 0.5 / z]);
            p[0].abs() <= w * 0.5 + 1e-3 && p[1].abs() <= h * 0.5 + 1e-3
        })
    };
    if fits(1.0) {
        return 1.0;
    }
    let (mut lo, mut hi) = (1.0f32, 64.0f32);
    if !fits(hi) {
        return hi;
    }
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_for_shift() {
        let s = Similarity { tx: 10.0, ..Similarity::IDENTITY };
        // Shifting 10 px on a 200 px wide frame needs 200 / 180 zoom.
        assert!((required_zoom(&s, 200.0, 100.0) - 200.0 / 180.0).abs() < 1e-3);
        assert_eq!(required_zoom(&Similarity::IDENTITY, 200.0, 100.0), 1.0);
    }
}
