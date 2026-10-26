// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Record what I walk": camera poses sampled while walking become a
//! camera path with smoothing, ready to be keys of a timeline shot.
//!
//! Smoothing runs the recorded track through a zero-phase filter (forward
//! and backward, so the path is not delayed), which takes out the
//! footfall bob and the jitter of a hand on the mouse while keeping the
//! route; turns are unwrapped first so they never spin the long way. The
//! smoothed track is then thinned to as few keys as hold it within a
//! tolerance, and played back by a spline through them.

use crate::math::{V3, wrap_deg};
use crate::walk::Pose;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub t: f32,
    pub pose: Pose,
}

/// A key of a camera path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub t: f32,
    pub pos: V3,
    /// Unwrapped degrees (may run past 180 so interpolation never flips).
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CameraPath {
    pub keys: Vec<Key>,
}

/// How hard to smooth and how many keys to keep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Smoothing {
    /// Seconds over which position wobble is averaged away.
    pub position: f32,
    /// Seconds for the look direction.
    pub look: f32,
    /// Error allowed when thinning keys: metres, and degrees.
    pub tolerance_m: f32,
    pub tolerance_deg: f32,
    /// Most seconds between keys.
    pub max_gap: f32,
}

impl Default for Smoothing {
    fn default() -> Smoothing {
        Smoothing { position: 0.35, look: 0.25, tolerance_m: 0.04, tolerance_deg: 0.6, max_gap: 1.0 }
    }
}

impl Smoothing {
    pub fn none() -> Smoothing {
        Smoothing { position: 0.0, look: 0.0, tolerance_m: 0.01, tolerance_deg: 0.1, max_gap: 0.5 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Recorder {
    pub samples: Vec<Sample>,
    pub recording: bool,
}

impl Recorder {
    pub fn start(&mut self) {
        self.samples.clear();
        self.recording = true;
    }
    pub fn stop(&mut self) {
        self.recording = false;
    }
    /// Notes the camera at `t` seconds since the start (ignored when not
    /// recording; a pose at a time already passed is dropped).
    pub fn push(&mut self, t: f32, pose: Pose) {
        if !self.recording {
            return;
        }
        if let Some(l) = self.samples.last()
            && t <= l.t + 1e-6
        {
            return;
        }
        self.samples.push(Sample { t, pose });
    }
    pub fn duration(&self) -> f32 {
        self.samples.last().map_or(0.0, |s| s.t) - self.samples.first().map_or(0.0, |s| s.t)
    }
    pub fn finish(&self, s: &Smoothing) -> CameraPath {
        smooth(&self.samples, s)
    }
}

const CHANNELS: usize = 6;
const RATE: f32 = 60.0;

fn channels(p: &Pose, prev_yaw: Option<f32>) -> [f32; CHANNELS] {
    // Unwrap yaw against the previous value.
    let yaw = match prev_yaw {
        Some(py) => py + wrap_deg(p.yaw - py),
        None => p.yaw,
    };
    [p.pos[0], p.pos[1], p.pos[2], yaw, p.pitch, p.fov]
}

/// Exponential smoothing run both ways: zero lag. The track is extended
/// past both ends by reflecting it about its end points, so a steady move
/// stays steady right up to the ends.
fn zero_phase(track: &mut Vec<[f32; CHANNELS]>, seconds: [f32; CHANNELS]) {
    let n = track.len();
    if n < 3 {
        return;
    }
    let pad_max = n - 1;
    for c in 0..CHANNELS {
        if seconds[c] <= 1e-4 {
            continue;
        }
        let a = 1.0 - (-1.0 / (RATE * seconds[c])).exp();
        let pad = ((seconds[c] * RATE * 4.0) as usize).clamp(1, pad_max);
        let mut v: Vec<f32> = Vec::with_capacity(n + 2 * pad);
        for k in (1..=pad).rev() {
            v.push(2.0 * track[0][c] - track[k][c]);
        }
        v.extend(track.iter().map(|t| t[c]));
        for k in 1..=pad {
            v.push(2.0 * track[n - 1][c] - track[n - 1 - k][c]);
        }
        let mut y = v[0];
        for x in v.iter_mut() {
            y += (*x - y) * a;
            *x = y;
        }
        let mut y = v[v.len() - 1];
        for x in v.iter_mut().rev() {
            y += (*x - y) * a;
            *x = y;
        }
        for (i, t) in track.iter_mut().enumerate() {
            t[c] = v[pad + i];
        }
    }
}

/// Smooths and thins recorded poses into a path. The first and last poses
/// are kept exactly.
pub fn smooth(samples: &[Sample], s: &Smoothing) -> CameraPath {
    if samples.is_empty() {
        return CameraPath::default();
    }
    let t0 = samples[0].t;
    let t1 = samples[samples.len() - 1].t;
    if samples.len() == 1 || t1 - t0 < 1e-4 {
        let c = channels(&samples[0].pose, None);
        return CameraPath { keys: vec![Key { t: 0.0, pos: [c[0], c[1], c[2]], yaw: c[3], pitch: c[4], fov: c[5] }] };
    }
    // Resample evenly (yaw unwrapped as we go).
    let n = (((t1 - t0) * RATE).ceil() as usize).max(2) + 1;
    let mut track: Vec<[f32; CHANNELS]> = Vec::with_capacity(n);
    let mut prev_yaw = None;
    let mut j = 0;
    for i in 0..n {
        let t = (t0 + i as f32 / RATE).min(t1);
        while j + 2 < samples.len() && samples[j + 1].t < t {
            j += 1;
        }
        let (a, b) = (&samples[j], &samples[j + 1]);
        let u = ((t - a.t) / (b.t - a.t).max(1e-6)).clamp(0.0, 1.0);
        let ca = channels(&a.pose, prev_yaw);
        let mut cb = channels(&b.pose, Some(ca[3]));
        cb[3] = ca[3] + wrap_deg(b.pose.yaw - ca[3]);
        let c: [f32; CHANNELS] = std::array::from_fn(|k| ca[k] + (cb[k] - ca[k]) * u);
        prev_yaw = Some(c[3]);
        track.push(c);
    }
    let (raw_first, raw_last) = (track[0], track[n - 1]);
    zero_phase(&mut track, [s.position, s.position, s.position, s.look, s.look, s.look.max(s.position)]);
    // Thin: keep a key where the line between the kept neighbours is
    // furthest off, recursively (Douglas-Peucker over time).
    let scale = [1.0 / s.tolerance_m.max(1e-4), 1.0 / s.tolerance_m.max(1e-4), 1.0 / s.tolerance_m.max(1e-4), 1.0 / s.tolerance_deg.max(1e-3), 1.0 / s.tolerance_deg.max(1e-3), 1.0 / (s.tolerance_deg.max(1e-3) * 2.0)];
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let max_gap = (s.max_gap * RATE) as usize;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let u = (i - a) as f32 / (b - a) as f32;
            let mut e = 0.0f32;
            for c in 0..CHANNELS {
                let line = track[a][c] + (track[b][c] - track[a][c]) * u;
                e = e.max((track[i][c] - line).abs() * scale[c]);
            }
            if e > worst.0 {
                worst = (e, i);
            }
        }
        if worst.0 > 1.0 || (b - a) > max_gap.max(2) {
            let split = if worst.0 > 1.0 { worst.1 } else { (a + b) / 2 };
            keep[split] = true;
            stack.push((a, split));
            stack.push((split, b));
        }
    }
    let keys = (0..n)
        .filter(|i| keep[*i])
        .map(|i| {
            let c = if i == 0 { raw_first } else if i == n - 1 { raw_last } else { track[i] };
            Key { t: i as f32 / RATE, pos: [c[0], c[1], c[2]], yaw: c[3], pitch: c[4], fov: c[5] }
        })
        .collect();
    CameraPath { keys }
}

impl CameraPath {
    pub fn duration(&self) -> f32 {
        self.keys.last().map_or(0.0, |k| k.t)
    }

    /// The camera at `t` seconds: a Catmull-Rom spline through the keys.
    pub fn at(&self, t: f32) -> Option<Pose> {
        let k = &self.keys;
        if k.is_empty() {
            return None;
        }
        if k.len() == 1 || t <= k[0].t {
            return Some(pose_of(&k[0]));
        }
        if t >= k[k.len() - 1].t {
            return Some(pose_of(&k[k.len() - 1]));
        }
        let i = k.partition_point(|key| key.t <= t).saturating_sub(1).min(k.len() - 2);
        let (p1, p2) = (&k[i], &k[i + 1]);
        let p0 = if i > 0 { &k[i - 1] } else { p1 };
        let p3 = if i + 2 < k.len() { &k[i + 2] } else { p2 };
        let u = (t - p1.t) / (p2.t - p1.t).max(1e-6);
        // Non-uniform Catmull-Rom: tangents scaled by the segment's time.
        let h = p2.t - p1.t;
        let tan = |a: &Key, b: &Key, f: fn(&Key) -> [f32; 6]| -> [f32; 6] {
            let (fa, fb) = (f(a), f(b));
            let dt = (b.t - a.t).max(1e-6);
            std::array::from_fn(|c| (fb[c] - fa[c]) / dt * h)
        };
        let f: fn(&Key) -> [f32; 6] = |k| [k.pos[0], k.pos[1], k.pos[2], k.yaw, k.pitch, k.fov];
        let m1 = if i > 0 { tan(p0, p2, f) } else { tan(p1, p2, f) };
        let m2 = if i + 2 < k.len() { tan(p1, p3, f) } else { tan(p1, p2, f) };
        let (a, b) = (f(p1), f(p2));
        let (u2, u3) = (u * u, u * u * u);
        let (h00, h10, h01, h11) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, -2.0 * u3 + 3.0 * u2, u3 - u2);
        let v: [f32; 6] = std::array::from_fn(|c| h00 * a[c] + h10 * m1[c] + h01 * b[c] + h11 * m2[c]);
        Some(Pose { pos: [v[0], v[1], v[2]], yaw: wrap_deg(v[3]), pitch: v[4], fov: v[5] })
    }
}

fn pose_of(k: &Key) -> Pose {
    Pose { pos: k.pos, yaw: wrap_deg(k.yaw), pitch: k.pitch, fov: k.fov }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{dist, len, sub};

    /// A walk along a gentle curve with footfall bob and mouse jitter.
    fn bobbing_walk(secs: f32) -> Vec<Sample> {
        let mut r = crate::noise::Rng::new(9);
        let n = (secs * 30.0) as usize;
        (0..=n)
            .map(|i| {
                let t = i as f32 / 30.0;
                let along = t * 4.0;
                let x = (along * 0.15).sin() * 6.0;
                let z = along;
                let bob = (t * 2.0 * std::f32::consts::PI * 1.8).sin() * 0.035;
                let yaw = (along * 0.15).cos() * 0.15f32.to_degrees() * 6.0 / 6.0 * 0.9 + (r.next() - 0.5) * 1.6;
                Sample { t, pose: Pose { pos: [x, 1.62 + bob, z], yaw, pitch: -2.0 + (r.next() - 0.5) * 1.2, fov: 60.0 } }
            })
            .collect()
    }

    fn roughness(vals: &[f32]) -> f32 {
        vals.windows(3).map(|w| (w[2] - 2.0 * w[1] + w[0]).abs()).sum::<f32>() / vals.len() as f32
    }

    #[test]
    fn smoothing_removes_bob_and_jitter_but_keeps_the_route_and_the_ends() {
        let raw = bobbing_walk(12.0);
        let path = smooth(&raw, &Smoothing::default());
        assert!(path.keys.len() >= 4 && path.keys.len() < raw.len() / 6, "{} keys from {}", path.keys.len(), raw.len());
        let play: Vec<Pose> = (0..=360).map(|i| path.at(i as f32 / 30.0).unwrap()).collect();
        let ys_raw: Vec<f32> = raw.iter().map(|s| s.pose.pos[1]).collect();
        let ys: Vec<f32> = play.iter().map(|p| p.pos[1]).collect();
        assert!(roughness(&ys) < roughness(&ys_raw) * 0.25, "{} vs {}", roughness(&ys), roughness(&ys_raw));
        let yaw_raw: Vec<f32> = raw.iter().map(|s| s.pose.yaw).collect();
        let yaw: Vec<f32> = play.iter().map(|p| p.yaw).collect();
        assert!(roughness(&yaw) < roughness(&yaw_raw) * 0.3);
        // The route is the same to within the smoothing's own corner-cutting.
        let worst = raw.iter().map(|s| { let p = path.at(s.t).unwrap(); ((p.pos[0] - s.pose.pos[0]).powi(2) + (p.pos[2] - s.pose.pos[2]).powi(2)).sqrt() }).fold(0.0f32, f32::max);
        assert!(worst < 0.4, "{worst}");
        // Ends are exact.
        let first = path.at(0.0).unwrap();
        let last = path.at(path.duration()).unwrap();
        assert!(dist(first.pos, raw[0].pose.pos) < 1e-3);
        assert!(dist(last.pos, raw.last().unwrap().pose.pos) < 1e-3);
        assert!((path.duration() - 12.0).abs() < 0.05);
    }

    #[test]
    fn turns_go_the_short_way_across_the_seam_and_stay_continuous() {
        let samples: Vec<Sample> = (0..=60).map(|i| {
            let t = i as f32 / 30.0;
            // Yaw sweeps 170 -> -170 (a 20 degree turn through 180).
            let yaw = wrap_deg(170.0 + t * 10.0);
            Sample { t, pose: Pose { pos: [0.0, 1.6, 0.0], yaw, pitch: 0.0, fov: 60.0 } }
        }).collect();
        let path = smooth(&samples, &Smoothing::default());
        let mut prev = path.at(0.0).unwrap().yaw;
        for i in 1..=60 {
            let y = path.at(i as f32 / 30.0).unwrap().yaw;
            assert!(wrap_deg(y - prev).abs() < 1.0, "no flip: {prev} -> {y}");
            prev = y;
        }
        assert!(path.keys.iter().all(|k| k.yaw >= 169.0), "unwrapped keys run past 180: {:?}", path.keys.iter().map(|k| k.yaw).collect::<Vec<_>>());
    }

    #[test]
    fn a_recorder_ignores_poses_when_stopped_and_repeated_times() {
        let mut r = Recorder::default();
        let p = Pose { pos: [0.0; 3], yaw: 0.0, pitch: 0.0, fov: 60.0 };
        r.push(0.0, p);
        assert!(r.samples.is_empty());
        r.start();
        r.push(0.0, p);
        r.push(0.0, p);
        r.push(0.5, Pose { pos: [1.0, 0.0, 0.0], ..p });
        r.stop();
        r.push(1.0, p);
        assert_eq!(r.samples.len(), 2);
        assert!((r.duration() - 0.5).abs() < 1e-6);
        let one = Recorder { samples: vec![Sample { t: 3.0, pose: p }], recording: false }.finish(&Smoothing::default());
        assert_eq!(one.keys.len(), 1);
        assert!(len(sub(one.at(5.0).unwrap().pos, p.pos)) < 1e-6);
    }

    #[test]
    fn a_still_camera_needs_two_keys_and_a_straight_walk_needs_few() {
        let still: Vec<Sample> = (0..=90).map(|i| Sample { t: i as f32 / 30.0, pose: Pose { pos: [1.0, 1.6, 2.0], yaw: 30.0, pitch: 0.0, fov: 60.0 } }).collect();
        let p = smooth(&still, &Smoothing { max_gap: 10.0, ..Smoothing::default() });
        assert_eq!(p.keys.len(), 2);
        let straight: Vec<Sample> = (0..=300).map(|i| Sample { t: i as f32 / 30.0, pose: Pose { pos: [0.0, 1.6, i as f32 / 30.0 * 4.0], yaw: 0.0, pitch: 0.0, fov: 60.0 } }).collect();
        let p = smooth(&straight, &Smoothing { max_gap: 100.0, ..Smoothing::default() });
        assert!(p.keys.len() <= 4, "{}", p.keys.len());
        let mid = p.at(5.0).unwrap();
        assert!((mid.pos[2] - 20.0).abs() < 0.1);
    }
}
