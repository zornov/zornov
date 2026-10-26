// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D camera tracking (matchmove): from footage to a moving camera, its
//! lens and a cloud of 3D points, so 3D layers, models and text stick to
//! the scene.
//!
//! 1. [`track_features`] follows a few hundred corners through the shot
//!    (FAST + pyramidal Lucas–Kanade, checked forwards and backwards,
//!    topped up as they leave), skipping areas a matte marks as moving.
//! 2. [`solve`] picks keyframes, starts from the pair with the most
//!    parallax (essential matrix, eight-point with RANSAC), adds keyframes
//!    by resection and triangulation, bundle-adjusts everything, then
//!    places the frames in between. With no parallax (a pan on a tripod)
//!    it solves a nodal camera that only turns. An unknown focal length
//!    is found by trying a range of lenses and refining the best.
//! 3. [`orient::orient`] puts the result in Studio's world: the ground
//!    plane level, a sensible origin and scale.
//!
//! Positions are in pixels from the top-left of the frame (pixel centres
//! at +0.5); the solver works from the picture centre.

pub mod ba;
pub mod linalg;
pub mod orient;

use std::collections::HashMap;

use crate::features::{Pyramid, fast_corners, klt};
use crate::image::Image;
use crate::ops::Gray;
use linalg::{self as la, M3, V3};

/// A camera pose: world → camera is `x_cam = r · x + t`. Camera axes are
/// right, down and forward (as Studio's 3D cameras).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub r: M3,
    pub t: V3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose { r: la::I3, t: [0.0; 3] };

    pub fn apply(&self, x: V3) -> V3 {
        la::add(la::mv(&self.r, x), self.t)
    }

    /// Lens position in the world.
    pub fn centre(&self) -> V3 {
        la::scale(la::mv(&la::tr(&self.r), self.t), -1.0)
    }

    pub fn from_centre(r: M3, c: V3) -> Pose {
        Pose { r, t: la::scale(la::mv(&r, c), -1.0) }
    }
}

/// One corner followed through the shot.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureTrack {
    pub id: usize,
    /// `(frame, [x, y])` in pixels, frames ascending.
    pub points: Vec<(usize, [f64; 2])>,
}

/// How to follow corners.
#[derive(Clone, Copy, Debug)]
pub struct TrackSettings {
    /// Corners kept alive at once.
    pub features: usize,
    /// Pictures wider than this are tracked at reduced size.
    pub max_width: usize,
    /// Tracks shorter than this are dropped.
    pub min_length: usize,
}

impl Default for TrackSettings {
    fn default() -> Self {
        TrackSettings { features: 300, max_width: 960, min_length: 5 }
    }
}

fn small(img: &Image, max_width: usize) -> (Gray, f32) {
    let g = Gray::from_image(img);
    let mut k = 1.0f32;
    let mut g2 = g;
    while g2.width > max_width.max(64) {
        g2 = g2.blurred(0.8).half();
        k *= 2.0;
    }
    (g2, k)
}

/// Follows corners through `frames` pictures. `frame(i)` gives picture `i`
/// and, optionally, a plane (1 = moving, ignore) of the same size.
pub fn track_features(frames: usize, mut frame: impl FnMut(usize) -> Option<(Image, Option<Vec<f32>>)>, settings: TrackSettings, mut progress: impl FnMut(usize) -> bool) -> Vec<FeatureTrack> {
    let mut done: Vec<FeatureTrack> = Vec::new();
    let mut live: Vec<FeatureTrack> = Vec::new();
    let mut next_id = 0usize;
    let mut prev: Option<(Pyramid, f32)> = None;
    for i in 0..frames {
        if !progress(i) {
            break;
        }
        let Some((img, ignore)) = frame(i) else { break };
        let (g, k) = small(&img, settings.max_width);
        let levels = ((g.width.min(g.height) as f32 / 24.0).log2().floor() as usize).clamp(1, 5);
        let pyr = Pyramid::new(&g, levels);
        let (gw, gh) = (g.width, g.height);
        let blocked = |x: f32, y: f32| -> bool {
            let Some(ig) = &ignore else { return false };
            let (fx, fy) = ((x * k) as usize, (y * k) as usize);
            ig.get(fy.min(img.height - 1) * img.width + fx.min(img.width - 1)).is_some_and(|&v| v > 0.5)
        };
        if let Some((pp, _)) = &prev {
            let mut keep = Vec::with_capacity(live.len());
            for mut t in live.drain(..) {
                let last = t.points.last().unwrap().1;
                let p = [(last[0] as f32) / k, (last[1] as f32) / k];
                let ok = klt(pp, &pyr, p, [0.0, 0.0], 5).and_then(|fwd| {
                    // Forward-backward check.
                    let back = klt(&pyr, pp, fwd.position, [p[0] - fwd.position[0], p[1] - fwd.position[1]], 5)?;
                    let err = (back.position[0] - p[0]).hypot(back.position[1] - p[1]);
                    let q = fwd.position;
                    (err < 0.6 && fwd.residual < 0.1 && q[0] > 4.0 && q[1] > 4.0 && q[0] < gw as f32 - 4.0 && q[1] < gh as f32 - 4.0 && !blocked(q[0], q[1])).then_some(q)
                });
                match ok {
                    Some(q) => {
                        t.points.push((i, [(q[0] * k) as f64, (q[1] * k) as f64]));
                        keep.push(t);
                    }
                    None => done.push(t),
                }
            }
            live = keep;
        }
        // Top up in empty areas.
        if live.len() < settings.features * 4 / 5 {
            let cell = ((gw * gh) as f32 / settings.features as f32).sqrt().max(8.0);
            let (cw, ch) = ((gw as f32 / cell).ceil() as usize, (gh as f32 / cell).ceil() as usize);
            let mut occupied = vec![false; cw * ch];
            for t in &live {
                let p = t.points.last().unwrap().1;
                let (cx, cy) = (((p[0] as f32 / k) / cell) as usize, ((p[1] as f32 / k) / cell) as usize);
                if cx < cw && cy < ch {
                    occupied[cy * cw + cx] = true;
                }
            }
            let mut corners = fast_corners(&g, 0.02);
            corners.sort_by(|a, b| b.2.total_cmp(&a.2));
            for (x, y, _) in corners {
                if live.len() >= settings.features {
                    break;
                }
                let (cx, cy) = ((x / cell) as usize, (y / cell) as usize);
                if cx >= cw || cy >= ch || occupied[cy * cw + cx] || x < 6.0 || y < 6.0 || x > gw as f32 - 6.0 || y > gh as f32 - 6.0 || blocked(x, y) {
                    continue;
                }
                occupied[cy * cw + cx] = true;
                live.push(FeatureTrack { id: next_id, points: vec![(i, [(x * k) as f64, (y * k) as f64])] });
                next_id += 1;
            }
        }
        prev = Some((pyr, k));
    }
    done.extend(live);
    done.retain(|t| t.points.len() >= settings.min_length);
    done.sort_by_key(|t| t.id);
    done
}

/// How to solve.
#[derive(Clone, Copy, Debug, Default)]
pub struct SolveSettings {
    /// The focal length in pixels when known (from the lens and sensor).
    pub focal: Option<f64>,
    /// Force a nodal (tripod pan) solve, or forbid one.
    pub nodal: Option<bool>,
    /// Frames between keyframes (default: about 40 keyframes).
    pub keyframe_step: Option<usize>,
}

/// One solved 3D point.
#[derive(Clone, Debug, PartialEq)]
pub struct SolvedPoint {
    pub track: usize,
    pub position: V3,
    /// RMS reprojection error over the frames that see it, pixels.
    pub error: f64,
}

/// A solved camera.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    pub width: f64,
    pub height: f64,
    /// Focal length in pixels of the footage.
    pub focal: f64,
    /// Per frame, `None` where it could not be solved.
    pub poses: Vec<Option<Pose>>,
    pub points: Vec<SolvedPoint>,
    /// RMS reprojection error over every observation, pixels.
    pub error: f64,
    /// The camera only turns (tripod), so points have no real depth.
    pub nodal: bool,
    pub keyframes: Vec<usize>,
}

impl Solved {
    /// Projects a world point into frame `i`, in footage pixels.
    pub fn project(&self, frame: usize, x: V3) -> Option<[f64; 2]> {
        let pose = self.poses.get(frame).copied().flatten()?;
        let p = ba::project(self.focal, pose.apply(x))?;
        Some([p[0] + self.width * 0.5, p[1] + self.height * 0.5])
    }

    /// The ray (origin, unit direction) through footage pixel `px` at
    /// frame `i`.
    pub fn ray(&self, frame: usize, px: [f64; 2]) -> Option<(V3, V3)> {
        let pose = self.poses.get(frame).copied().flatten()?;
        let d = [(px[0] - self.width * 0.5) / self.focal, (px[1] - self.height * 0.5) / self.focal, 1.0];
        Some((pose.centre(), la::unit(la::mv(&la::tr(&pose.r), d))))
    }

    /// How many frames solved.
    pub fn solved_frames(&self) -> usize {
        self.poses.iter().filter(|p| p.is_some()).count()
    }
}

/// Observations per track, centred: frame → (u, v).
type Centred = Vec<(usize, HashMap<usize, [f64; 2]>)>;

fn centred(tracks: &[FeatureTrack], w: f64, h: f64) -> Centred {
    tracks.iter().map(|t| (t.id, t.points.iter().map(|(f, p)| (*f, [p[0] - w * 0.5, p[1] - h * 0.5])).collect())).collect()
}

/// Essential matrix from normalised correspondences by the eight-point
/// algorithm with RANSAC; returns it with the inlier mask.
fn essential(pairs: &[([f64; 2], [f64; 2])], threshold: f64, seed: u32) -> Option<(M3, Vec<bool>)> {
    if pairs.len() < 8 {
        return None;
    }
    let fit = |idx: &[usize]| -> Option<M3> {
        let rows: Vec<Vec<f64>> = idx
            .iter()
            .map(|&i| {
                let ([x1, y1], [x2, y2]) = pairs[i];
                vec![x2 * x1, x2 * y1, x2, y2 * x1, y2 * y1, y2, x1, y1, 1.0]
            })
            .collect();
        let e = la::null_vector(&rows);
        let m: M3 = [[e[0], e[1], e[2]], [e[3], e[4], e[5]], [e[6], e[7], e[8]]];
        // Project to an essential matrix: two equal singular values.
        let (u, s, v) = la::svd3(&m);
        let k = 0.5 * (s[0] + s[1]);
        Some(la::mul(&la::mul(&u, &[[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, 0.0]]), &la::tr(&v)))
    };
    let sampson = |e: &M3, a: [f64; 2], b: [f64; 2]| -> f64 {
        let x1 = [a[0], a[1], 1.0];
        let x2 = [b[0], b[1], 1.0];
        let ex1 = la::mv(e, x1);
        let etx2 = la::mv(&la::tr(e), x2);
        let num = la::dot(x2, ex1).powi(2);
        num / (ex1[0] * ex1[0] + ex1[1] * ex1[1] + etx2[0] * etx2[0] + etx2[1] * etx2[1]).max(1e-30)
    };
    let mut rng = crate::math::Rng::new(seed);
    let mut best: Option<(M3, usize)> = None;
    let t2 = threshold * threshold;
    for _ in 0..300 {
        let mut idx = Vec::with_capacity(8);
        while idx.len() < 8 {
            let i = rng.below(pairs.len());
            if !idx.contains(&i) {
                idx.push(i);
            }
        }
        let Some(e) = fit(&idx) else { continue };
        let n = pairs.iter().filter(|(a, b)| sampson(&e, *a, *b) < t2).count();
        if best.as_ref().is_none_or(|b| n > b.1) {
            best = Some((e, n));
        }
    }
    let (e, _) = best?;
    let inl: Vec<usize> = (0..pairs.len()).filter(|&i| sampson(&e, pairs[i].0, pairs[i].1) < t2).collect();
    let e = if inl.len() >= 8 { fit(&inl)? } else { e };
    let mask = pairs.iter().map(|(a, b)| sampson(&e, *a, *b) < t2 * 4.0).collect();
    Some((e, mask))
}

/// The pose of the second camera from an essential matrix: the one of four
/// with the most points in front of both cameras.
fn pose_from_essential(e: &M3, pairs: &[([f64; 2], [f64; 2])]) -> Option<Pose> {
    let (mut u, _, mut v) = la::svd3(e);
    if la::det3(&u) < 0.0 {
        for r in u.iter_mut() {
            r[2] = -r[2];
        }
    }
    if la::det3(&v) < 0.0 {
        for r in v.iter_mut() {
            r[2] = -r[2];
        }
    }
    let w: M3 = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let r1 = la::mul(&la::mul(&u, &w), &la::tr(&v));
    let r2 = la::mul(&la::mul(&u, &la::tr(&w)), &la::tr(&v));
    let t = [u[0][2], u[1][2], u[2][2]];
    let mut best: Option<(Pose, usize)> = None;
    for (r, tt) in [(r1, t), (r1, la::scale(t, -1.0)), (r2, t), (r2, la::scale(t, -1.0))] {
        let p = Pose { r, t: tt };
        let n = pairs
            .iter()
            .filter(|(a, b)| ba::triangulate(&[(Pose::IDENTITY, *a), (p, *b)], 1.0).is_some())
            .count();
        if best.as_ref().is_none_or(|b| n > b.1) {
            best = Some((p, n));
        }
    }
    best.filter(|b| b.1 >= 6).map(|b| b.0)
}

/// Common observations of two frames, centred.
fn common(obs: &Centred, fa: usize, fb: usize) -> Vec<(usize, [f64; 2], [f64; 2])> {
    obs.iter().enumerate().filter_map(|(i, (_, m))| Some((i, *m.get(&fa)?, *m.get(&fb)?))).collect()
}

/// Median residual of a homography fit between two frames: how much
/// parallax (camera travel) there is beyond a pure pan.
fn homography_residual(pairs: &[([f64; 2], [f64; 2])]) -> f64 {
    let Some((h, _)) = crate::comp::geo::ransac_homography(pairs, 2.0, 200, 5) else { return 0.0 };
    let mut r: Vec<f64> = pairs
        .iter()
        .map(|(a, b)| {
            let p = crate::comp::geo::apply_h(&h, *a);
            (p[0] - b[0]).hypot(p[1] - b[1])
        })
        .collect();
    r.sort_by(f64::total_cmp);
    r[r.len() / 2]
}

/// Solves the camera from feature tracks of a `width`×`height` shot with
/// `frames` frames.
pub fn solve(tracks: &[FeatureTrack], width: f64, height: f64, frames: usize, settings: SolveSettings) -> Result<Solved, String> {
    if frames < 2 {
        return Err("a camera needs at least two frames to solve".into());
    }
    let obs = centred(tracks, width, height);
    let per_frame = |f: usize| obs.iter().filter(|(_, m)| m.contains_key(&f)).count();
    let step = settings.keyframe_step.unwrap_or((frames / 40).max(1)).max(1);
    let mut keyframes: Vec<usize> = (0..frames).step_by(step).filter(|&f| per_frame(f) >= 8).collect();
    if let Some(last) = (0..frames).rev().find(|&f| per_frame(f) >= 8) {
        if keyframes.last() != Some(&last) {
            keyframes.push(last);
        }
    }
    if keyframes.len() < 2 {
        return Err("too few corners could be followed through the shot (it may be too dark, blurred or plain)".into());
    }
    // Initial pair: the first keyframe and the one after it with the most
    // parallax that still shares enough corners.
    let k0 = keyframes[0];
    let mut pair = (k0, keyframes[1], 0.0f64);
    for &kb in keyframes.iter().skip(1).take(16) {
        let c = common(&obs, k0, kb);
        if c.len() < 20 {
            break;
        }
        let pr: Vec<_> = c.iter().map(|(_, a, b)| (*a, *b)).collect();
        let res = homography_residual(&pr);
        if res > pair.2 {
            pair = (k0, kb, res);
        }
    }
    let nodal = settings.nodal.unwrap_or(pair.2 < 0.6);
    let focals: Vec<f64> = match settings.focal {
        Some(f) => vec![f],
        None => [0.55, 0.7, 0.85, 1.0, 1.2, 1.45, 1.75, 2.1, 2.6, 3.3].iter().map(|k| k * width).collect(),
    };
    // Try each lens on a thinned set of keyframes, keep the best, then
    // solve everything with it.
    let probe: Vec<usize> = if focals.len() > 1 {
        let n = keyframes.len();
        let s = (n / 12).max(1);
        let mut k: Vec<usize> = keyframes.iter().copied().step_by(s).collect();
        for f in [pair.0, pair.1] {
            if !k.contains(&f) {
                k.push(f);
            }
        }
        k.sort_unstable();
        k
    } else {
        keyframes.clone()
    };
    let mut best: Option<(f64, f64)> = None;
    let mut why = String::new();
    for &f in &focals {
        match solve_keyframes(&obs, &probe, pair, f, nodal, false, 15) {
            Ok(s) if best.is_none_or(|b| s.error < b.1) => best = Some((f, s.error)),
            Ok(_) => {}
            Err(e) => why = e,
        }
    }
    let (f0, _) = best.ok_or_else(|| format!("the camera could not be solved: {why}"))?;
    let mut s = solve_keyframes(&obs, &keyframes, pair, f0, nodal, settings.focal.is_none(), 40)?;
    // Frames between keyframes: resection from the nearest solved pose.
    let index: HashMap<usize, usize> = s.points.iter().enumerate().map(|(i, p)| (p.track, i)).collect();
    let mut poses = vec![None; frames];
    for (k, &f) in keyframes.iter().enumerate() {
        poses[f] = s.poses.get(k).copied().flatten();
    }
    let solved_pts: Vec<V3> = s.points.iter().map(|p| p.position).collect();
    for pass in 0..2 {
        let order: Vec<usize> = if pass == 0 { (0..frames).collect() } else { (0..frames).rev().collect() };
        for f in order {
            if poses[f].is_some() {
                continue;
            }
            let near = (1..frames).find_map(|d| {
                let a = f.checked_sub(d).and_then(|g| poses[g]);
                let b = poses.get(f + d).copied().flatten();
                a.or(b)
            });
            let Some(mut pose) = near else { continue };
            let corr: Vec<(V3, [f64; 2])> = obs.iter().filter_map(|(id, m)| Some((solved_pts[*index.get(id)?], *m.get(&f)?))).collect();
            if corr.len() < 6 {
                continue;
            }
            let err = ba::refine_pose(&mut pose, s.focal, &corr, 30, 3.0, nodal);
            if err < 8.0 {
                poses[f] = Some(pose);
            }
        }
    }
    // Overall error.
    let mut all = Vec::new();
    let mut cams = Vec::new();
    let mut cam_of = HashMap::new();
    for (f, p) in poses.iter().enumerate() {
        if let Some(p) = p {
            cam_of.insert(f, cams.len());
            cams.push(*p);
        }
    }
    for (id, m) in &obs {
        let Some(&pi) = index.get(id) else { continue };
        for (f, uv) in m {
            if let Some(&c) = cam_of.get(f) {
                all.push(ba::Obs { cam: c, point: pi, uv: *uv });
            }
        }
    }
    s.error = ba::rms(&cams, &solved_pts, s.focal, &all);
    s.poses = poses;
    s.keyframes = keyframes;
    s.width = width;
    s.height = height;
    Ok(s)
}

/// Solves the given keyframes with focal `f` (refined when `free_focal`).
/// The result's poses are indexed like `keys`.
fn solve_keyframes(obs: &Centred, keys: &[usize], pair: (usize, usize, f64), f: f64, nodal: bool, free_focal: bool, iterations: usize) -> Result<Solved, String> {
    let mut f = f;
    let ka = keys.iter().position(|&k| k == pair.0).ok_or("internal: initial keyframe missing")?;
    let kb = keys.iter().position(|&k| k == pair.1).ok_or("internal: second keyframe missing")?;
    let mut poses: Vec<Option<Pose>> = vec![None; keys.len()];
    // Points by track index (into obs).
    let mut pts: HashMap<usize, V3> = HashMap::new();
    let c = common(obs, pair.0, pair.1);
    let norm_pairs: Vec<([f64; 2], [f64; 2])> = c.iter().map(|(_, a, b)| ([a[0] / f, a[1] / f], [b[0] / f, b[1] / f])).collect();
    poses[ka] = Some(Pose::IDENTITY);
    let depth = 1000.0;
    if nodal {
        // Rotation from the homography H = K R K⁻¹, refined below.
        let pr: Vec<([f64; 2], [f64; 2])> = c.iter().map(|(_, a, b)| (*a, *b)).collect();
        let (h, _) = crate::comp::geo::ransac_homography(&pr, 2.0, 200, 3).ok_or("no consistent pan between the first frames")?;
        let k: M3 = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let ki: M3 = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
        let r = la::orthonormalise(&la::mul(&la::mul(&ki, &h), &k));
        poses[kb] = Some(Pose { r, t: [0.0; 3] });
        for (i, a, _) in &c {
            pts.insert(*i, la::scale(la::unit([a[0] / f, a[1] / f, 1.0]), depth));
        }
    } else {
        let thr = 1.5 / f;
        let (e, mask) = essential(&norm_pairs, thr, 7).ok_or("the first frames do not agree on a camera move")?;
        let inl: Vec<([f64; 2], [f64; 2])> = norm_pairs.iter().zip(&mask).filter(|(_, m)| **m).map(|(p, _)| *p).collect();
        let pb = pose_from_essential(&e, &inl).ok_or("the first frames do not agree on a camera move")?;
        poses[kb] = Some(pb);
        for ((i, a, b), m) in c.iter().zip(&mask) {
            if !*m {
                continue;
            }
            if let Some(x) = ba::triangulate(&[(Pose::IDENTITY, *a), (pb, *b)], f) {
                if ba::parallax(&Pose::IDENTITY, &pb, x) > 0.2f64.to_radians() {
                    pts.insert(*i, x);
                }
            }
        }
    }
    if pts.len() < 8 {
        return Err("too few points could be placed in 3D from the first frames".into());
    }
    // Grow outwards from the initial pair.
    let mut order: Vec<usize> = (0..keys.len()).filter(|&k| k != ka && k != kb).collect();
    order.sort_by_key(|&k| (k as isize - ka as isize).abs().min((k as isize - kb as isize).abs()));
    let mut added = 0;
    for k in order {
        let frame = keys[k];
        // Start from the nearest solved keyframe.
        let near = (0..keys.len()).filter(|&j| poses[j].is_some()).min_by_key(|&j| (j as isize - k as isize).abs());
        let Some(mut pose) = near.and_then(|j| poses[j]) else { continue };
        let corr: Vec<(V3, [f64; 2])> = obs.iter().enumerate().filter_map(|(i, (_, m))| Some((*pts.get(&i)?, *m.get(&frame)?))).collect();
        if corr.len() < 6 {
            continue;
        }
        let err = ba::refine_pose(&mut pose, f, &corr, 40, 3.0, nodal);
        if err > 12.0 {
            continue;
        }
        poses[k] = Some(pose);
        // New points seen here and in an earlier keyframe.
        for (i, (_, m)) in obs.iter().enumerate() {
            if pts.contains_key(&i) || !m.contains_key(&frame) {
                continue;
            }
            let views: Vec<(Pose, [f64; 2])> = keys.iter().enumerate().filter_map(|(j, kf)| Some((poses[j]?, *m.get(kf)?))).collect();
            if views.len() < 2 {
                continue;
            }
            if nodal {
                let (p0, uv) = views[0];
                let d = la::mv(&la::tr(&p0.r), la::unit([uv[0] / f, uv[1] / f, 1.0]));
                pts.insert(i, la::add(p0.centre(), la::scale(d, depth)));
                continue;
            }
            let Some(x) = ba::triangulate(&views, f) else { continue };
            let (first, last) = (views[0].0, views[views.len() - 1].0);
            if ba::parallax(&first, &last, x) < 0.5f64.to_radians() {
                continue;
            }
            let ok = views.iter().all(|(p, uv)| ba::project(f, p.apply(x)).is_some_and(|q| (q[0] - uv[0]).hypot(q[1] - uv[1]) < 4.0));
            if ok {
                pts.insert(i, x);
            }
        }
        added += 1;
        if added % 6 == 0 {
            bundle(obs, keys, &mut poses, &mut pts, &mut f, ka, nodal, false, 8);
        }
    }
    let solved = poses.iter().filter(|p| p.is_some()).count();
    if solved * 2 < keys.len() {
        return Err(format!("only {solved} of {} keyframes could be solved", keys.len()));
    }
    let mut err = bundle(obs, keys, &mut poses, &mut pts, &mut f, ka, nodal, free_focal, iterations);
    // Drop points that disagree, then settle again.
    let bad = point_errors(obs, keys, &poses, &pts, f).into_iter().filter(|(_, e)| *e > (3.0 * err).max(2.0)).map(|(i, _)| i).collect::<Vec<_>>();
    if !bad.is_empty() {
        for i in bad {
            pts.remove(&i);
        }
        err = bundle(obs, keys, &mut poses, &mut pts, &mut f, ka, nodal, free_focal, iterations);
    }
    let errs: HashMap<usize, f64> = point_errors(obs, keys, &poses, &pts, f).into_iter().collect();
    let mut points: Vec<SolvedPoint> = pts.iter().map(|(&i, &x)| SolvedPoint { track: obs[i].0, position: x, error: errs.get(&i).copied().unwrap_or(0.0) }).collect();
    points.sort_by_key(|p| p.track);
    Ok(Solved { width: 0.0, height: 0.0, focal: f, poses, points, error: err, nodal, keyframes: keys.to_vec() })
}

fn gather(obs: &Centred, keys: &[usize], poses: &[Option<Pose>], pts: &HashMap<usize, V3>) -> (Vec<usize>, Vec<Pose>, Vec<usize>, Vec<V3>, Vec<ba::Obs>) {
    let cam_idx: Vec<usize> = (0..keys.len()).filter(|&k| poses[k].is_some()).collect();
    let cams: Vec<Pose> = cam_idx.iter().map(|&k| poses[k].unwrap()).collect();
    let mut pt_idx: Vec<usize> = pts.keys().copied().collect();
    pt_idx.sort_unstable();
    let points: Vec<V3> = pt_idx.iter().map(|i| pts[i]).collect();
    let mut o = Vec::new();
    for (pi, &i) in pt_idx.iter().enumerate() {
        for (ci, &k) in cam_idx.iter().enumerate() {
            if let Some(uv) = obs[i].1.get(&keys[k]) {
                o.push(ba::Obs { cam: ci, point: pi, uv: *uv });
            }
        }
    }
    (cam_idx, cams, pt_idx, points, o)
}

#[allow(clippy::too_many_arguments)]
fn bundle(obs: &Centred, keys: &[usize], poses: &mut [Option<Pose>], pts: &mut HashMap<usize, V3>, f: &mut f64, gauge: usize, nodal: bool, free_focal: bool, iterations: usize) -> f64 {
    let (cam_idx, mut cams, pt_idx, mut points, o) = gather(obs, keys, poses, pts);
    // The gauge camera first, held still.
    let g = cam_idx.iter().position(|&k| k == gauge).unwrap_or(0);
    cams.swap(0, g);
    let o: Vec<ba::Obs> = o.into_iter().map(|mut x| {
        x.cam = if x.cam == 0 { g } else if x.cam == g { 0 } else { x.cam };
        x
    }).collect();
    let free = ba::Free { first_free_cam: 1, focal: free_focal, rotation_only: nodal };
    let err = ba::adjust(&mut cams, &mut points, f, &o, free, iterations, 3.0);
    cams.swap(0, g);
    for (ci, &k) in cam_idx.iter().enumerate() {
        poses[k] = Some(cams[ci]);
    }
    for (pi, &i) in pt_idx.iter().enumerate() {
        pts.insert(i, points[pi]);
    }
    err
}

fn point_errors(obs: &Centred, keys: &[usize], poses: &[Option<Pose>], pts: &HashMap<usize, V3>, f: f64) -> Vec<(usize, f64)> {
    pts.iter()
        .map(|(&i, &x)| {
            let mut s = 0.0;
            let mut n = 0.0;
            for (k, kf) in keys.iter().enumerate() {
                let (Some(p), Some(uv)) = (poses[k], obs[i].1.get(kf)) else { continue };
                let e = ba::project(f, p.apply(x)).map_or(100.0, |q| (q[0] - uv[0]).hypot(q[1] - uv[1]));
                s += e * e;
                n += 1.0;
            }
            (i, if n > 0.0 { (s / n).sqrt() } else { 0.0 })
        })
        .collect()
}

#[cfg(test)]
mod tests;
