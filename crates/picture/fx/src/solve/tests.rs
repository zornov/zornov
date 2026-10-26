// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::linalg as la;
use super::orient::{OrientSettings, camera_keys, orient};
use super::*;

const W: f64 = 960.0;
const H: f64 = 540.0;

/// A synthetic shot: ground points, a back wall and scattered points; the
/// camera dollies sideways and forward while turning a little.
fn scene(frames: usize, f: f64, travel: bool, seed: u32) -> (Vec<FeatureTrack>, Vec<Pose>, Vec<V3>) {
    let mut rng = crate::math::Rng::new(seed);
    let mut pts = Vec::new();
    for _ in 0..140 {
        // Ground: y = 2 below the camera, z 6..30.
        pts.push([rng.next_f32() as f64 * 16.0 - 8.0, 2.0, 6.0 + rng.next_f32() as f64 * 24.0]);
    }
    for _ in 0..80 {
        pts.push([rng.next_f32() as f64 * 20.0 - 10.0, rng.next_f32() as f64 * 6.0 - 4.0, 28.0]);
    }
    for _ in 0..60 {
        pts.push([rng.next_f32() as f64 * 10.0 - 5.0, rng.next_f32() as f64 * 4.0 - 2.0, 8.0 + rng.next_f32() as f64 * 15.0]);
    }
    let mut poses = Vec::new();
    for i in 0..frames {
        let s = i as f64 / frames as f64;
        let c = if travel { [3.0 * s - 1.5, -0.2 * s, 2.0 * s] } else { [0.0, 0.0, 0.0] };
        let r = la::rodrigues([0.02 * s, if travel { -0.15 * s } else { 0.5 * s - 0.25 }, 0.01 * s]);
        poses.push(Pose::from_centre(r, c));
    }
    let mut tracks = Vec::new();
    for (id, x) in pts.iter().enumerate() {
        let mut t = FeatureTrack { id, points: Vec::new() };
        for (fi, p) in poses.iter().enumerate() {
            let Some(uv) = ba::project(f, p.apply(*x)) else { continue };
            let px = [uv[0] + W * 0.5 + (rng.next_f32() as f64 - 0.5) * 0.4, uv[1] + H * 0.5 + (rng.next_f32() as f64 - 0.5) * 0.4];
            if px[0] < 2.0 || px[1] < 2.0 || px[0] > W - 2.0 || px[1] > H - 2.0 {
                continue;
            }
            t.points.push((fi, px));
        }
        if t.points.len() >= 5 {
            tracks.push(t);
        }
    }
    (tracks, poses, pts)
}

/// Best similarity (scale, rotation, translation) from `a` to `b`
/// (Umeyama), and the RMS residual.
fn align(a: &[V3], b: &[V3]) -> f64 {
    let n = a.len() as f64;
    let ca = la::scale(a.iter().fold([0.0; 3], |s, p| la::add(s, *p)), 1.0 / n);
    let cb = la::scale(b.iter().fold([0.0; 3], |s, p| la::add(s, *p)), 1.0 / n);
    let mut cov = [[0.0; 3]; 3];
    let mut va = 0.0;
    for (p, q) in a.iter().zip(b) {
        let (da, db) = (la::sub(*p, ca), la::sub(*q, cb));
        va += la::dot(da, da);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += db[i] * da[j];
            }
        }
    }
    let (u, s, v) = la::svd3(&cov);
    let mut d = [1.0, 1.0, 1.0];
    if la::det3(&u) * la::det3(&v) < 0.0 {
        d[2] = -1.0;
    }
    let r = la::mul(&la::mul(&u, &[[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]]), &la::tr(&v));
    let k = (s[0] * d[0] + s[1] * d[1] + s[2] * d[2]) / va;
    let mut e = 0.0;
    for (p, q) in a.iter().zip(b) {
        let m = la::add(la::scale(la::mv(&r, la::sub(*p, ca)), k), cb);
        e += la::dot(la::sub(m, *q), la::sub(m, *q));
    }
    (e / n).sqrt()
}

#[test]
fn a_dolly_shot_solves_with_an_unknown_lens() {
    let f = 1.25 * W;
    let (tracks, truth, _) = scene(60, f, true, 3);
    let s = solve(&tracks, W, H, 60, SolveSettings::default()).unwrap();
    assert!(!s.nodal);
    assert!(s.error < 0.6, "rms {}", s.error);
    assert!((s.focal / f - 1.0).abs() < 0.05, "focal {} vs {f}", s.focal);
    assert_eq!(s.solved_frames(), 60);
    let got: Vec<V3> = s.poses.iter().map(|p| p.unwrap().centre()).collect();
    let want: Vec<V3> = truth.iter().map(|p| p.centre()).collect();
    let path: f64 = want.windows(2).map(|w| la::norm(la::sub(w[1], w[0]))).sum();
    let e = align(&got, &want);
    assert!(e < 0.02 * path, "path residual {e} of {path}");
}

#[test]
fn a_known_lens_is_kept() {
    let f = 0.9 * W;
    let (tracks, _, _) = scene(40, f, true, 5);
    let s = solve(&tracks, W, H, 40, SolveSettings { focal: Some(f), ..Default::default() }).unwrap();
    assert_eq!(s.focal, f);
    assert!(s.error < 0.6);
}

#[test]
fn a_tripod_pan_solves_nodal() {
    let f = 1.1 * W;
    let (tracks, truth, _) = scene(50, f, false, 8);
    let s = solve(&tracks, W, H, 50, SolveSettings::default()).unwrap();
    assert!(s.nodal);
    assert!(s.error < 0.6, "rms {}", s.error);
    assert!((s.focal / f - 1.0).abs() < 0.05, "focal {}", s.focal);
    // Relative pan angle between first and last frame.
    let rel = |a: &Pose, b: &Pose| la::norm(la::log_rot(&la::mul(&b.r, &la::tr(&a.r))));
    let got = rel(&s.poses[0].unwrap(), &s.poses[49].unwrap());
    let want = rel(&truth[0], &truth[49]);
    assert!((got - want).abs() < 0.01, "{got} vs {want}");
}

#[test]
fn orientation_levels_the_ground() {
    let f = 1.25 * W;
    let (tracks, _, pts) = scene(60, f, true, 3);
    let s = solve(&tracks, W, H, 60, SolveSettings::default()).unwrap();
    let fr = orient(&s, &OrientSettings { canvas_width: 1920.0, ..Default::default() });
    // The 140 ground points (ids 0..140) end up at y ≈ 0.
    let ys: Vec<f64> = s.points.iter().filter(|p| p.track < 140).map(|p| fr.to_world(p.position)[1]).collect();
    let spread = s.points.iter().map(|p| la::norm(fr.to_world(p.position))).fold(0.0f64, f64::max);
    let worst = ys.iter().map(|y| y.abs()).fold(0.0f64, f64::max);
    assert!(worst < 0.02 * spread, "ground off by {worst} (scene {spread})");
    let keys = camera_keys(&s, &fr, 2.0);
    // The camera stands above the ground (negative y is up).
    assert!(keys.iter().all(|k| k.position[1] < 0.0));
    // The first camera is the default distance from the origin.
    let d = la::norm(keys[0].position);
    assert!((d - 1920.0 * 1.388_888_9).abs() < 1.0, "{d}");
    assert!((keys[0].zoom - 2.0 * s.focal).abs() < 1e-9);
    let _ = pts;
}

#[test]
fn corners_are_followed_through_moving_pictures() {
    let (w, h) = (160usize, 120usize);
    let frame = |i: usize| {
        let img = crate::image::Image::from_fn(w, h, |x, y| {
            let (u, v) = (x as f32 - 1.5 * i as f32, y as f32 - 0.5 * i as f32);
            let k = 0.2 + 0.6 * crate::math::value_noise(u / 6.0, v / 6.0, 4);
            [k, k, k, 1.0]
        });
        Some((img, None))
    };
    let tracks = track_features(12, frame, TrackSettings { features: 80, max_width: 960, min_length: 5 }, |_| true);
    assert!(tracks.len() > 20, "{} tracks", tracks.len());
    // Every track moves by the picture's motion.
    for t in &tracks {
        let (f0, p0) = t.points[0];
        let (f1, p1) = *t.points.last().unwrap();
        let df = (f1 - f0) as f64;
        assert!((p1[0] - p0[0] - 1.5 * df).abs() < 0.5 && (p1[1] - p0[1] - 0.5 * df).abs() < 0.5, "{t:?}");
    }
}
