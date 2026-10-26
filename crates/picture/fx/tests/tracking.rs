// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stabilisation and tracking on synthetic footage with known motion.

use studio_fx::features::{fast_corners, Similarity};
use studio_fx::image::{Edge, Image};
use studio_fx::math::fbm;
use studio_fx::ops::{warp, Gray};
use studio_fx::stabilise::{StabiliseMode, StabiliseSettings, Stabiliser};
use studio_fx::track::{to_keyframes, BoxTracker, PointTracker, TrackSample};

/// A large textured "world" we can point a virtual camera at.
fn world(w: usize, h: usize) -> Image {
    Image::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / 9.0, y as f32 / 9.0);
        let n = fbm(u, v, 0.0, 4, 11);
        let m = fbm(u * 0.3 + 40.0, v * 0.3, 0.0, 2, 3);
        let c = 0.05 + 0.9 * n * n;
        [c, c * (0.6 + 0.4 * m), c * 0.8, 1.0]
    })
}

/// Camera view of `src`: the frame centre looks at `src` centre + (dx, dy),
/// rotated by `angle` radians.
fn view(src: &Image, w: usize, h: usize, dx: f32, dy: f32, angle: f32) -> Image {
    let (sn, cs) = angle.sin_cos();
    let (cx, cy) = (src.width as f32 * 0.5 + dx, src.height as f32 * 0.5 + dy);
    let tmp = Image::new(w, h);
    warp(&tmp, Edge::Clamp, |x, y| {
        let (u, v) = (x - w as f32 * 0.5, y - h as f32 * 0.5);
        (cx + u * cs - v * sn, cy + u * sn + v * cs)
    })
    .map_xy(|x, y, _| {
        let (u, v) = (x as f32 + 0.5 - w as f32 * 0.5, y as f32 + 0.5 - h as f32 * 0.5);
        src.sample(cx + u * cs - v * sn, cy + u * sn + v * cs, Edge::Clamp)
    })
}

#[test]
fn fast_finds_corners_of_a_square() {
    let img = Image::from_fn(40, 40, |x, y| if (12..28).contains(&x) && (12..28).contains(&y) { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
    let corners = fast_corners(&Gray::from_image(&img), 0.2);
    assert!(corners.len() >= 4, "{corners:?}");
    for (cx, cy) in [(12.0, 12.0), (28.0, 12.0), (12.0, 28.0), (28.0, 28.0)] {
        assert!(corners.iter().any(|&(x, y, _)| (x - cx).abs() < 3.0 && (y - cy).abs() < 3.0), "missing corner near {cx},{cy}");
    }
}

#[test]
fn stabiliser_measures_and_removes_shake() {
    let src = world(420, 300);
    let (w, h) = (240, 160);
    // A slow pan to the right with hand shake on top.
    let shake = |k: usize| ((k as f32 * 2.1).sin() * 3.0, (k as f32 * 1.7).cos() * 2.5, (k as f32 * 1.3).sin() * 0.01);
    let n = 24;
    let mut stab = Stabiliser::new(StabiliseSettings { smoothness: 0.3, max_crop: 0.15, ..Default::default() });
    let mut truth = Vec::new();
    for k in 0..n {
        let (sx, sy, sa) = shake(k);
        let (dx, dy, a) = (k as f32 * 1.5 + sx, sy, sa);
        truth.push((dx, dy, a));
        stab.push(&view(&src, w, h, dx, dy, a));
    }
    // Frame-to-frame motion: the content moves opposite to the camera.
    for k in 1..n {
        let m = stab.motion(k);
        let (dx, dy, a) = (truth[k].0 - truth[k - 1].0, truth[k].1 - truth[k - 1].1, truth[k].2 - truth[k - 1].2);
        assert!((m.angle() + a).abs() < 0.003, "frame {k}: angle {} vs {}", m.angle(), -a);
        // Translation of the centre, allowing for the rotation about it.
        assert!((m.tx + dx).abs() < 0.5 && (m.ty + dy).abs() < 0.5, "frame {k}: {m:?} vs {dx},{dy}");
    }
    // After correction the path left should be much smoother: compare the
    // jerk (second difference) of the corrected and raw paths.
    let corr = stab.solve(30.0);
    assert_eq!(corr.len(), n);
    let zoom = corr[0].zoom;
    assert!(corr.iter().all(|c| c.zoom == zoom) && zoom >= 1.0 && zoom <= 1.0 / 0.85 + 1e-4);
    let mut path = Vec::new();
    let mut c = Similarity::IDENTITY;
    for k in 0..n {
        c = stab.motion(k).compose(&c);
        path.push(corr[k].transform.compose(&c));
    }
    let jerk = |xs: &[f32]| xs.windows(3).map(|w| (w[0] - 2.0 * w[1] + w[2]).abs()).sum::<f32>();
    let raw_x: Vec<f32> = truth.iter().map(|t| -t.0).collect();
    let fixed_x: Vec<f32> = path.iter().map(|s| s.tx).collect();
    assert!(jerk(&fixed_x) < jerk(&raw_x) * 0.35, "raw {} fixed {}", jerk(&raw_x), jerk(&fixed_x));

    // Lock mode cancels the motion completely within the crop allowance.
    let mut lock = Stabiliser::new(StabiliseSettings { mode: StabiliseMode::Lock, max_crop: 0.3, ..Default::default() });
    for k in 0..8 {
        let (sx, sy, _) = shake(k);
        lock.push(&view(&src, w, h, sx, sy, 0.0));
    }
    let corr = lock.solve(30.0);
    let mut c = Similarity::IDENTITY;
    for k in 0..8 {
        c = lock.motion(k).compose(&c);
        let total = corr[k].transform.compose(&c);
        assert!(total.tx.abs() < 0.5 && total.ty.abs() < 0.5, "frame {k}: {total:?}");
    }
    // Applying a correction keeps the frame size and finite values.
    let img = corr[3].apply(&view(&src, w, h, 0.0, 0.0, 0.0));
    assert_eq!((img.width, img.height), (w, h));
    assert!(img.is_finite());
}

#[test]
fn point_tracker_follows_subpixel_motion() {
    let src = world(400, 300);
    let (w, h) = (200, 150);
    let start = [100.0f32, 75.0];
    let mut tracker = PointTracker::new(&view(&src, w, h, 0.0, 0.0, 0.0), start);
    let mut samples = vec![TrackSample { position: start, scale: 1.0, confidence: 1.0, lost: false }];
    for k in 1..15 {
        let (dx, dy) = (k as f32 * 2.3, (k as f32 * 0.4).sin() * 6.0);
        let s = tracker.track(&view(&src, w, h, dx, dy, 0.0));
        assert!(!s.lost, "lost at {k}");
        // The world point moves opposite to the camera.
        let expect = [start[0] - dx, start[1] - dy];
        assert!((s.position[0] - expect[0]).abs() < 0.3 && (s.position[1] - expect[1]).abs() < 0.3, "frame {k}: {:?} vs {expect:?}", s.position);
        samples.push(s);
    }
    let keys = to_keyframes(&samples, 30.0, w, h, 0.0005);
    assert!(keys.len() >= 2 && keys.len() <= samples.len());
    assert_eq!(keys[0].offset, [0.0, 0.0]);
    let last = keys.last().unwrap();
    assert!((last.offset[0] * w as f32 + 14.0 * 2.3).abs() < 0.5, "{last:?}");
    assert!((last.time - 14.0 / 30.0).abs() < 1e-5);
}

#[test]
fn box_tracker_follows_and_scales() {
    let src = world(500, 400);
    let (w, h) = (240, 180);
    let first = view(&src, w, h, 0.0, 0.0, 0.0);
    let rect = [90.0, 60.0, 60.0, 50.0];
    let mut tracker = BoxTracker::new(&first, rect);
    for k in 1..10 {
        let (dx, dy) = (k as f32 * 3.0, -(k as f32) * 1.5);
        let s = tracker.track(&view(&src, w, h, dx, dy, 0.0));
        assert!(!s.lost && s.confidence > 0.8, "frame {k}: {s:?}");
        let expect = [120.0 - dx, 85.0 - dy];
        assert!((s.position[0] - expect[0]).abs() < 1.0 && (s.position[1] - expect[1]).abs() < 1.0, "frame {k}: {:?} vs {expect:?}", s.position);
    }
    // Zooming the camera in makes the target grow.
    let zoomed = {
        let base = view(&src, w, h, 27.0, -13.5, 0.0);
        let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
        warp(&base, Edge::Clamp, |x, y| (cx + (x - cx) / 1.05, cy + (y - cy) / 1.05))
    };
    let s = tracker.track(&zoomed);
    assert!(!s.lost && s.scale > 1.02, "{s:?}");
}
