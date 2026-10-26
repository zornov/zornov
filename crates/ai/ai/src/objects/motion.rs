// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Camera motion against object motion. The camera's path comes from the
//! picture as a whole (the background outvotes moving things); an object's
//! own movement is what is left of its path across the frame once the
//! camera's part is taken out: an object that glides across the picture
//! because the camera pans after it has not moved through the place, one
//! that holds still on screen while the camera tracks it has.

use super::{CamPose, CameraMotion, Movement, Obs};
use crate::features::track::{self, Mat3, Pose, IDENTITY};

/// The camera's pose at every sample from the motion between neighbouring
/// samples (`steps[i]` takes sample `i` to sample `i + 1`, picture pixels
/// at `width` x `height`).
pub fn camera_path(times: &[f32], steps: &[Mat3], width: f32, height: f32) -> Vec<CamPose> {
    let c = (width as f64 / 2.0, height as f64 / 2.0);
    let mut cum = IDENTITY;
    let mut out = Vec::with_capacity(times.len());
    for (i, &t) in times.iter().enumerate() {
        if i > 0 {
            if let Some(s) = steps.get(i - 1) {
                cum = track::mul(s, &cum);
            }
        }
        let p = Pose::of(&cum, c);
        out.push(CamPose { t, x: (p.x / width as f64) as f32, y: (p.y / width as f64) as f32, angle: p.angle as f32, log_scale: p.log_scale as f32 });
    }
    out
}

/// The pose at `t`, interpolated.
pub fn pose_at(path: &[CamPose], t: f32) -> CamPose {
    match path {
        [] => CamPose { t, ..Default::default() },
        [one] => CamPose { t, ..*one },
        _ => {
            let i = path.partition_point(|p| p.t <= t);
            if i == 0 {
                return CamPose { t, ..path[0] };
            }
            if i >= path.len() {
                return CamPose { t, ..path[path.len() - 1] };
            }
            let (a, b) = (path[i - 1], path[i]);
            let k = if b.t > a.t { (t - a.t) / (b.t - a.t) } else { 0.0 };
            let m = |p: f32, q: f32| p + (q - p) * k;
            CamPose { t, x: m(a.x, b.x), y: m(a.y, b.y), angle: m(a.angle, b.angle), log_scale: m(a.log_scale, b.log_scale) }
        }
    }
}

fn matrix(p: &CamPose, width: f32, height: f32) -> Mat3 {
    Pose { x: p.x as f64 * width as f64, y: p.y as f64 * width as f64, angle: p.angle as f64, log_scale: p.log_scale as f64 }.matrix((width as f64 / 2.0, height as f64 / 2.0))
}

/// Where the normalised point `(x, y)` seen at time `t` would be if the
/// camera had stayed where it was at the first sample.
pub fn stabilised(path: &[CamPose], t: f32, x: f32, y: f32, width: f32, height: f32) -> (f32, f32) {
    let m = matrix(&pose_at(path, t), width, height);
    let Some(inv) = track::invert(&m) else { return (x, y) };
    let q = track::apply(&inv, (x as f64 * width as f64, y as f64 * height as f64));
    ((q.0 / width as f64) as f32, (q.1 / height as f64) as f32)
}

/// Words for what the camera does.
pub fn summarise(path: &[CamPose], duration: f32, aspect: f32) -> String {
    if path.len() < 2 || duration < 0.3 {
        return "static".into();
    }
    let (first, last) = (path[0], path[path.len() - 1]);
    let span = (last.t - first.t).max(0.1);
    // Scene shift to the right = camera pans left. y is in widths; the
    // picture is `aspect` (h / w) tall.
    let dx = -(last.x - first.x);
    let dy = -(last.y - first.y) / aspect.max(0.1);
    let dz = last.log_scale - first.log_scale;
    // Jitter: how far the path strays from a straight line through the ends.
    let mut jitter = 0.0f32;
    for p in path {
        let k = (p.t - first.t) / span;
        jitter += ((p.x - (first.x + (last.x - first.x) * k)).powi(2) + (p.y - (first.y + (last.y - first.y) * k)).powi(2)).sqrt();
    }
    jitter /= path.len() as f32;
    let mut parts: Vec<String> = Vec::new();
    let speed = |v: f32| {
        let s = v.abs() / span;
        if s > 0.5 { "fast" } else if s > 0.15 { "steadily" } else { "slowly" }
    };
    if dx.abs() > 0.04 && dx.abs() >= dy.abs() * 0.8 {
        parts.push(format!("pans {} {}", if dx > 0.0 { "right" } else { "left" }, speed(dx)));
    }
    if dy.abs() > 0.04 && dy.abs() > dx.abs() * 0.8 {
        parts.push(format!("tilts {} {}", if dy < 0.0 { "up" } else { "down" }, speed(dy)));
    }
    if dz.abs() > 0.06 {
        parts.push(format!("{} {}", if dz > 0.0 { "pushes in" } else { "pulls out" }, speed(dz)));
    }
    if jitter > 0.006 {
        parts.push("handheld".into());
    }
    if parts.is_empty() { "static".into() } else { parts.join(", ") }
}

pub fn describe(path: Vec<CamPose>, duration: f32, aspect: f32) -> CameraMotion {
    let summary = summarise(&path, duration, aspect);
    CameraMotion { path, summary }
}

/// The object's own movement through the place: its path across the frame
/// with the camera's part taken out, and its change of distance.
pub fn movement(obs: &[Obs], path: &[CamPose], width: f32, height: f32) -> Option<Movement> {
    if obs.len() < 2 {
        return None;
    }
    let span = obs[obs.len() - 1].t - obs[0].t;
    if span < 0.4 {
        return None;
    }
    let stab: Vec<(f32, f32, f32)> = obs.iter().map(|o| { let (x, y) = stabilised(path, o.t, o.bbox.cx(), o.bbox.cy(), width, height); (o.t, x, y) }).collect();
    let screen: Vec<(f32, f32, f32)> = obs.iter().map(|o| (o.t, o.bbox.cx(), o.bbox.cy())).collect();
    let end = |v: &[(f32, f32, f32)]| {
        let k = (v.len() / 3).max(1);
        let mean = |s: &[(f32, f32, f32)]| (s.iter().map(|p| p.1).sum::<f32>() / s.len() as f32, s.iter().map(|p| p.2).sum::<f32>() / s.len() as f32, s.iter().map(|p| p.0).sum::<f32>() / s.len() as f32);
        (mean(&v[..k]), mean(&v[v.len() - k..]))
    };
    let travel = |v: &[(f32, f32, f32)]| {
        let (a, b) = end(v);
        let dt = (b.2 - a.2).max(0.1);
        let (dx, dy) = ((b.0 - a.0), (b.1 - a.1) * height / width);
        ((dx * dx + dy * dy).sqrt() / dt, dy.atan2(dx).to_degrees())
    };
    let (speed, heading) = travel(&stab);
    let (screen_speed, _) = travel(&screen);
    let zs: Vec<(f32, f32)> = obs.iter().filter_map(|o| Some((o.t, o.z?))).collect();
    let depth_speed = if zs.len() >= 2 && zs[zs.len() - 1].0 - zs[0].0 >= 0.5 { Some((zs[zs.len() - 1].1 - zs[0].1) / (zs[zs.len() - 1].0 - zs[0].0)) } else { None };
    // Still in the place: it does not travel, whatever the camera does.
    let _ = screen_speed;
    Some(Movement { speed, heading: if heading < 0.0 { heading + 360.0 } else { heading }, depth_speed, still: speed < 0.03 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift(dx: f64, dy: f64) -> Mat3 {
        [1.0, 0.0, dx, 0.0, 1.0, dy, 0.0, 0.0, 1.0]
    }

    fn times(n: usize, dt: f32) -> Vec<f32> {
        (0..n).map(|i| i as f32 * dt).collect()
    }

    #[test]
    fn a_steady_pan_is_summarised_and_accumulates() {
        // The scene slides 30 px right every half second: the camera pans left.
        let t = times(9, 0.5);
        let steps = vec![shift(30.0, 0.0); 8];
        let path = camera_path(&t, &steps, 640.0, 360.0);
        assert!((path[8].x - 240.0 / 640.0).abs() < 1e-3, "{:?}", path[8]);
        let s = summarise(&path, 4.0, 360.0 / 640.0);
        assert!(s.contains("pans left"), "{s}");
        assert_eq!(summarise(&camera_path(&t, &vec![IDENTITY; 8], 640.0, 360.0), 4.0, 0.56), "static");
    }

    #[test]
    fn a_push_in_and_a_tilt_are_named() {
        let t = times(5, 1.0);
        let zoom = Pose { x: 0.0, y: 0.0, angle: 0.0, log_scale: 0.06 }.matrix((320.0, 180.0));
        let path = camera_path(&t, &vec![zoom; 4], 640.0, 360.0);
        assert!(summarise(&path, 4.0, 0.5625).contains("pushes in"));
        let path = camera_path(&t, &vec![shift(0.0, 40.0); 4], 640.0, 360.0);
        assert!(summarise(&path, 4.0, 0.5625).contains("tilts up"), "{}", summarise(&path, 4.0, 0.5625));
    }

    #[test]
    fn handheld_shake_is_noticed() {
        let t = times(13, 0.25);
        let steps: Vec<Mat3> = (0..12).map(|i| if i % 2 == 0 { shift(9.0, 4.0) } else { shift(-9.0, -4.0) }).collect();
        let path = camera_path(&t, &steps, 640.0, 360.0);
        assert!(summarise(&path, 3.0, 0.5625).contains("handheld"));
    }

    #[test]
    fn stabilising_cancels_the_pan() {
        let t = times(5, 1.0);
        let path = camera_path(&t, &vec![shift(64.0, 0.0); 4], 640.0, 360.0);
        // A point fixed in the world shows 64 px (0.1) further right each second.
        let (x, y) = stabilised(&path, 2.0, 0.2 + 0.2, 0.5, 640.0, 360.0);
        assert!((x - 0.2).abs() < 1e-3 && (y - 0.5).abs() < 1e-3, "{x} {y}");
    }

    fn walker(f: impl Fn(f32) -> (f32, f32)) -> Vec<Obs> {
        (0..9).map(|i| { let t = i as f32 * 0.5; let (x, y) = f(t); super::super::testing::obs(t, x - 0.05, y - 0.1, 0.1, 0.2) }).collect()
    }

    #[test]
    fn a_walker_moves_and_a_panned_wall_does_not() {
        let t = times(9, 0.5);
        let path = camera_path(&t, &vec![shift(32.0, 0.0); 8], 640.0, 360.0);
        // A wall fixture slides right on screen with the pan.
        let wall = walker(|t| (0.2 + 0.1 * t, 0.5));
        let m = movement(&wall, &path, 640.0, 360.0).unwrap();
        assert!(m.still, "{m:?}");
        // A walker who holds still on screen while the camera follows moves with it.
        let followed = walker(|_| (0.5, 0.5));
        let m = movement(&followed, &path, 640.0, 360.0).unwrap();
        assert!(!m.still && m.speed > 0.05, "{m:?}");
        // With a static camera, the same drift on screen is the object's.
        let still_path = camera_path(&t, &vec![IDENTITY; 8], 640.0, 360.0);
        let m = movement(&wall, &still_path, 640.0, 360.0).unwrap();
        assert!(!m.still && (m.speed - 0.1).abs() < 0.02 && m.heading.abs() < 5.0, "{m:?}");
    }

    #[test]
    fn approach_is_read_from_distance() {
        let mut o = walker(|_| (0.5, 0.5));
        for (i, ob) in o.iter_mut().enumerate() {
            ob.z = Some(8.0 - i as f32 * 0.5);
        }
        let m = movement(&o, &[], 640.0, 360.0).unwrap();
        assert!((m.depth_speed.unwrap() + 1.0).abs() < 1e-3, "{m:?}");
    }
}
