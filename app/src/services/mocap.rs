// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion capture: a person filmed on video drives a character's
//! skeleton. The body points come from the pose landmarker (MediaPipe
//! BlazePose, `studio_ai::features::faces::PoseTracker`), frame by frame;
//! they are smoothed (a One Euro filter, the filter motion-capture rigs
//! use: steady when still, quick when moving) and gaps where a point is
//! hidden are bridged; then each limb's direction is retargeted onto the
//! bones by role.
//!
//! **Retargeting.** A bone whose rest direction matches the person's limb
//! in the first frame (an arm hanging, a leg standing) takes the limb's
//! direction as it is; one that does not (a dog's spine against a
//! person's upright back, a quadruped's front legs from a person's arms)
//! takes the limb's turn from that first frame. Limb mapping: arms drive
//! a quadruped's front legs and legs its back legs; `strength` scales the
//! turns (an animal's stiffer legs), and the body's travel is scaled by
//! the character's leg length against the person's.

use super::rigging::{Role, Side, role};
use std::collections::BTreeMap;
use studio_model as m;

/// BlazePose points used here.
pub mod point {
    pub const NOSE: usize = 0;
    pub const LEFT_EAR: usize = 7;
    pub const RIGHT_EAR: usize = 8;
    pub const LEFT_SHOULDER: usize = 11;
    pub const RIGHT_SHOULDER: usize = 12;
    pub const LEFT_ELBOW: usize = 13;
    pub const RIGHT_ELBOW: usize = 14;
    pub const LEFT_WRIST: usize = 15;
    pub const RIGHT_WRIST: usize = 16;
    pub const LEFT_HIP: usize = 23;
    pub const RIGHT_HIP: usize = 24;
    pub const LEFT_KNEE: usize = 25;
    pub const RIGHT_KNEE: usize = 26;
    pub const LEFT_ANKLE: usize = 27;
    pub const RIGHT_ANKLE: usize = 28;
    pub const COUNT: usize = 33;
}

/// Body points of a filmed person over time.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    /// Frame times, seconds from the start of the capture.
    pub times: Vec<f64>,
    /// Per frame: each point `(x, y, visibility)` in pixels of the video,
    /// or `None` where no person was found.
    pub frames: Vec<Option<Vec<[f32; 3]>>>,
    /// The video's height in pixels (points are smoothed in heights).
    pub height: f32,
}

/// How a capture is put onto a skeleton.
#[derive(Clone, Debug)]
pub struct Options {
    /// 0 (raw) .. 1 (very smooth).
    pub smooth: f64,
    /// Mirror the person left to right (they face the other way).
    pub mirror: bool,
    /// The person's left side drives the character's back limbs instead
    /// of its front ones.
    pub swap_sides: bool,
    /// How much of each turn the bones take (1 = all).
    pub strength: f64,
    /// Move the body with the person's hips: "full", "vertical" or "none".
    pub root: String,
    /// Bones to drive by name → a person's limb (`left_forearm`,
    /// `right_shin`, `spine`, `head`…), over the role-based choice.
    pub map: BTreeMap<String, String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { smooth: 0.5, mirror: false, swap_sides: false, strength: 1.0, root: "full".into(), map: BTreeMap::new() }
    }
}

/// A limb of the person: from point to point (a midpoint is two).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limb(pub [usize; 2], pub [usize; 2]);

fn limb_named(name: &str) -> Option<Limb> {
    use point::*;
    let one = |a: usize, b: usize| Limb([a, a], [b, b]);
    Some(match name.to_lowercase().replace([' ', '-'], "_").as_str() {
        "spine" | "torso" | "back" => Limb([LEFT_HIP, RIGHT_HIP], [LEFT_SHOULDER, RIGHT_SHOULDER]),
        "head" | "neck" => Limb([LEFT_SHOULDER, RIGHT_SHOULDER], [LEFT_EAR, RIGHT_EAR]),
        "left_upper_arm" | "left_arm" => one(LEFT_SHOULDER, LEFT_ELBOW),
        "right_upper_arm" | "right_arm" => one(RIGHT_SHOULDER, RIGHT_ELBOW),
        "left_forearm" => one(LEFT_ELBOW, LEFT_WRIST),
        "right_forearm" => one(RIGHT_ELBOW, RIGHT_WRIST),
        "left_thigh" => one(LEFT_HIP, LEFT_KNEE),
        "right_thigh" => one(RIGHT_HIP, RIGHT_KNEE),
        "left_shin" | "left_leg" => one(LEFT_KNEE, LEFT_ANKLE),
        "right_shin" | "right_leg" => one(RIGHT_KNEE, RIGHT_ANKLE),
        _ => return None,
    })
}

pub const LIMBS: &str = "spine, head, left_upper_arm, left_forearm, right_upper_arm, right_forearm, left_thigh, left_shin, right_thigh, right_shin";

/// The person's limb that drives a bone, by its role.
fn limb_for(r: Role, swap: bool) -> Option<Limb> {
    let left = |s: Side| (s == Side::Front) != swap;
    let pick = |s: Side, l: &str, rr: &str| limb_named(if left(s) { l } else { rr });
    match r {
        Role::Spine => limb_named("spine"),
        Role::Head | Role::Neck => limb_named("head"),
        Role::UpperArm(s) => pick(s, "left_upper_arm", "right_upper_arm"),
        Role::Forearm(s) => pick(s, "left_forearm", "right_forearm"),
        Role::Thigh(s) => pick(s, "left_thigh", "right_thigh"),
        Role::Shin(s) => pick(s, "left_shin", "right_shin"),
        _ => None,
    }
}

/// The One Euro filter (Casiez, Roussel & Vogel 2012) on one value.
struct OneEuro {
    min_cutoff: f64,
    beta: f64,
    x: Option<f64>,
    dx: f64,
}

impl OneEuro {
    fn alpha(cutoff: f64, dt: f64) -> f64 {
        let tau = 1.0 / (std::f64::consts::TAU * cutoff);
        1.0 / (1.0 + tau / dt)
    }

    fn next(&mut self, v: f64, dt: f64) -> f64 {
        let Some(prev) = self.x else {
            self.x = Some(v);
            return v;
        };
        let d = (v - prev) / dt.max(1e-6);
        let ad = Self::alpha(1.0, dt);
        self.dx += ad * (d - self.dx);
        let cutoff = self.min_cutoff + self.beta * self.dx.abs();
        let a = Self::alpha(cutoff, dt);
        let out = prev + a * (v - prev);
        self.x = Some(out);
        out
    }
}

/// The capture's points smoothed, with hidden points bridged from the
/// frames around them: per frame, every point `(x, y)` in video heights.
fn cleaned(cap: &Capture, smooth: f64, mirror: bool) -> Vec<Option<Vec<[f64; 2]>>> {
    let n = cap.frames.len();
    let h = cap.height.max(1.0) as f64;
    let mut out: Vec<Option<Vec<[f64; 2]>>> = vec![None; n];
    for p in 0..point::COUNT {
        // Seen positions, then bridged.
        let seen: Vec<Option<[f64; 2]>> = cap.frames.iter().map(|f| f.as_ref().and_then(|pts| pts.get(p)).filter(|q| q[2] >= 0.4).map(|q| [if mirror { -q[0] as f64 } else { q[0] as f64 } / h, q[1] as f64 / h])).collect();
        let mut filled = seen.clone();
        for i in 0..n {
            if filled[i].is_some() {
                continue;
            }
            let before = (0..i).rev().find_map(|j| seen[j].map(|v| (j, v)));
            let after = (i + 1..n).find_map(|j| seen[j].map(|v| (j, v)));
            filled[i] = match (before, after) {
                (Some((a, va)), Some((b, vb))) => {
                    let k = (i - a) as f64 / (b - a) as f64;
                    Some([va[0] + (vb[0] - va[0]) * k, va[1] + (vb[1] - va[1]) * k])
                }
                (Some((_, v)), None) | (None, Some((_, v))) => Some(v),
                (None, None) => None,
            };
        }
        // 0: raw; 1: a slow, steady filter.
        let raw = smooth <= 0.0;
        let min_cutoff = 8.0 * (1.0 - smooth.clamp(0.0, 1.0)).powi(2) + 0.3;
        let (mut fx, mut fy) = (OneEuro { min_cutoff, beta: 0.4, x: None, dx: 0.0 }, OneEuro { min_cutoff, beta: 0.4, x: None, dx: 0.0 });
        for i in 0..n {
            let Some(v) = filled[i] else { continue };
            let dt = if i == 0 { 1.0 / 30.0 } else { (cap.times[i] - cap.times[i - 1]).max(1e-3) };
            let s = if raw { v } else { [fx.next(v[0], dt), fy.next(v[1], dt)] };
            if cap.frames[i].is_some() || filled[i].is_some() {
                out[i].get_or_insert_with(|| vec![[f64::NAN; 2]; point::COUNT])[p] = s;
            }
        }
    }
    out
}

fn at(pts: &[[f64; 2]], l: Limb) -> Option<([f64; 2], [f64; 2])> {
    let mid = |a: [usize; 2]| [(pts[a[0]][0] + pts[a[1]][0]) / 2.0, (pts[a[0]][1] + pts[a[1]][1]) / 2.0];
    let (a, b) = (mid(l.0), mid(l.1));
    (a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()).then_some((a, b))
}

fn angle(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees()
}

/// Keys a capture gives a skeleton: bone id → `(seconds, rotation)`, the
/// root's offsets, and which bones were driven how.
#[derive(Clone, Debug, Default)]
pub struct Retargeted {
    pub rotations: BTreeMap<u32, Vec<(f64, f64)>>,
    pub offsets: Vec<(f64, m::Vec2)>,
    pub root: Option<u32>,
    /// Bone name → "direction" or "turn" (from the first frame), and the
    /// limb.
    pub driven: Vec<(String, &'static str, String)>,
    /// Frames where a person was found, of all.
    pub found: (usize, usize),
}

/// Puts a capture onto `sk` (its rest pose), keys from `start` seconds of
/// clip time.
pub fn retarget(cap: &Capture, sk: &m::Skeleton, o: &Options, start: f64) -> Result<Retargeted, String> {
    let pts = cleaned(cap, o.smooth, o.mirror);
    let first = pts.iter().position(Option::is_some).ok_or("no person was found in the video")?;
    let aspect = sk.aspect.max(1e-6);
    let rest_angle = |b: &m::Bone| ((b.tail.y - b.head.y)).atan2((b.tail.x - b.head.x) * aspect).to_degrees();
    // Which limb drives which bone, and how.
    let mut plan: Vec<(u32, Limb, bool, f64, f64)> = Vec::new(); // bone, limb, absolute, rest, first
    let mut out = Retargeted { found: (cap.frames.iter().filter(|f| f.is_some()).count(), cap.frames.len()), ..Retargeted::default() };
    for b in &sk.bones {
        let (limb, label) = match o.map.iter().find(|(k, _)| k.eq_ignore_ascii_case(&b.name)) {
            Some((_, l)) => (limb_named(l).ok_or_else(|| format!("map: \"{l}\" is not a limb; limbs: {LIMBS}"))?, l.clone()),
            None => match limb_for(role(&b.name), o.swap_sides) {
                Some(l) => (l, format!("{:?}", role(&b.name))),
                None => continue,
            },
        };
        let Some((a, z)) = pts[first].as_ref().and_then(|p| at(p, limb)) else { continue };
        let a0 = angle(a, z);
        let rest = rest_angle(b);
        let absolute = m::skeleton::wrap(a0 - rest).abs() < 45.0;
        plan.push((b.id, limb, absolute, rest, a0));
        out.driven.push((b.name.clone(), if absolute { "direction" } else { "turn" }, label));
    }
    if plan.is_empty() {
        return Err("none of the bones has a limb to follow: name them by role (spine, head, upper_arm.front, shin.back…) or give \"map\"".into());
    }
    // Leg lengths for the body's travel.
    let char_leg: f64 = sk.bones.iter().filter(|b| matches!(role(&b.name), Role::Thigh(Side::Front) | Role::Shin(Side::Front))).map(|b| (((b.tail.x - b.head.x) * aspect).powi(2) + (b.tail.y - b.head.y).powi(2)).sqrt()).sum();
    let person_leg = pts[first].as_ref().map_or(0.0, |p| {
        [limb_named("left_thigh"), limb_named("left_shin")].iter().flatten().filter_map(|l| at(p, *l)).map(|(a, b)| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()).sum::<f64>()
    });
    let scale = if char_leg > 0.0 && person_leg > 1e-6 { char_leg / person_leg } else { 0.5 };
    let hips = Limb([point::LEFT_HIP, point::RIGHT_HIP], [point::LEFT_HIP, point::RIGHT_HIP]);
    let hip0 = pts[first].as_ref().and_then(|p| at(p, hips)).map(|v| v.0);
    out.root = sk.bones.iter().find(|b| b.parent.is_none()).map(|b| b.id);
    for (i, f) in pts.iter().enumerate() {
        let Some(p) = f else { continue };
        let t = start + cap.times[i] - cap.times[first];
        if t < start - 1e-9 {
            continue;
        }
        // World turns, parents first.
        let mut world: BTreeMap<u32, f64> = BTreeMap::new();
        for b in &sk.bones {
            let parent = b.parent.and_then(|q| world.get(&q).copied()).unwrap_or(0.0);
            let local = match plan.iter().find(|x| x.0 == b.id) {
                Some(&(_, limb, absolute, rest, a0)) => match at(p, limb) {
                    Some((a, z)) => {
                        let now = angle(a, z);
                        let turn = if absolute { m::skeleton::wrap(now - rest) } else { m::skeleton::wrap(now - a0) };
                        b.limit(m::skeleton::wrap(o.strength * turn - parent))
                    }
                    None => 0.0,
                },
                None => 0.0,
            };
            world.insert(b.id, parent + local);
            if plan.iter().any(|x| x.0 == b.id) {
                let keys = out.rotations.entry(b.id).or_default();
                // Unwrapped against the previous key, so a turn past 180°
                // does not spin the long way round.
                let v = match keys.last() {
                    Some(&(_, prev)) => prev + m::skeleton::wrap(local - prev),
                    None => local,
                };
                keys.push((t, v));
            }
        }
        if let (Some(h0), Some((h, _)), true) = (hip0, at(p, hips), o.root != "none") {
            let dx = if o.root == "vertical" { 0.0 } else { (h[0] - h0[0]) * scale / aspect };
            out.offsets.push((t, m::Vec2::new(dx, (h[1] - h0[1]) * scale)));
        }
    }
    Ok(out)
}

/// Body points of the person in a video file, from second `from` for
/// `dur` seconds, `rate` frames a second. `progress` gets 0..1 and stops
/// the run when it returns `false`.
#[cfg(feature = "render")]
pub fn capture(path: &std::path::Path, from: f64, dur: f64, rate: f64, progress: &mut dyn FnMut(f64) -> bool) -> Result<Capture, String> {
    use studio_ai::features::faces::PoseTracker;
    let src = studio_render::source::open_path(path)?;
    let info = src.info();
    let fps = info.frame_rate.unwrap_or(30.0).max(1.0);
    let total = info.frame_count.unwrap_or(1).max(1);
    let (w, h) = if info.rotation % 180 == 90 { (info.height, info.width) } else { (info.width, info.height) };
    drop(src);
    let f = super::tracking::Footage { path: path.to_path_buf(), fps, frames: total, width: w, height: h };
    let size = super::tracking::analysis_size(w, h);
    let size = (size.0.max(256), size.1.max(256));
    let first = (from * fps).floor().max(0.0) as u64;
    let last = (((from + dur) * fps).ceil() as u64).min(total).max(first + 1);
    let mut frames = super::cutout::RgbaFrames::open(&f, size, &(first..last))?;
    let mut tracker = PoseTracker::load(&studio_ai::ModelStore::default()).map_err(|e| format!("the pose model is not ready ({e}): download \"Body tracking\" in Settings ▸ Models"))?;
    let step = (fps / rate.clamp(1.0, fps)).max(1.0);
    let mut cap = Capture { height: size.1 as f32, ..Capture::default() };
    let mut k = 0.0;
    while (first as f64 + k) < last as f64 {
        let i = first + k.round() as u64;
        if !progress((i - first) as f64 / (last - first) as f64) {
            return Err("cancelled".into());
        }
        let frame = frames.get(i)?;
        let pose = tracker.next_frame(&frame).map_err(|e| e.to_string())?;
        cap.times.push((i - first) as f64 / fps);
        cap.frames.push(pose.map(|p| p.landmarks.iter().map(|l| [l.x, l.y, l.visibility]).collect()));
        k += step;
    }
    Ok(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A person standing, then raising the left arm forward and up over a
    /// second while bending the right knee, filmed at 10 frames a second
    /// on a 1000-pixel-high picture.
    fn raising() -> Capture {
        let mut cap = Capture { height: 1000.0, ..Capture::default() };
        for i in 0..=10 {
            let k = i as f32 / 10.0;
            let mut p = vec![[500.0f32, 500.0, 1.0]; point::COUNT];
            let set = |p: &mut Vec<[f32; 3]>, i: usize, x: f32, y: f32| p[i] = [x, y, 1.0];
            set(&mut p, point::LEFT_SHOULDER, 520.0, 300.0);
            set(&mut p, point::RIGHT_SHOULDER, 480.0, 300.0);
            set(&mut p, point::LEFT_EAR, 510.0, 220.0);
            set(&mut p, point::RIGHT_EAR, 490.0, 220.0);
            set(&mut p, point::LEFT_HIP, 515.0, 550.0);
            set(&mut p, point::RIGHT_HIP, 485.0, 550.0);
            // The left arm turns from hanging (down) to pointing forward
            // (+x) as k goes 0 → 1.
            let a = (90.0 - 90.0 * k).to_radians();
            set(&mut p, point::LEFT_ELBOW, 520.0 + 120.0 * a.cos(), 300.0 + 120.0 * a.sin());
            set(&mut p, point::LEFT_WRIST, 520.0 + 240.0 * a.cos(), 300.0 + 240.0 * a.sin());
            set(&mut p, point::RIGHT_ELBOW, 480.0, 420.0);
            set(&mut p, point::RIGHT_WRIST, 480.0, 540.0);
            set(&mut p, point::LEFT_KNEE, 515.0, 750.0);
            set(&mut p, point::LEFT_ANKLE, 515.0, 950.0);
            let b = (90.0 + 40.0 * k).to_radians();
            set(&mut p, point::RIGHT_KNEE, 485.0, 750.0);
            set(&mut p, point::RIGHT_ANKLE, 485.0 + 200.0 * b.cos(), 750.0 + 200.0 * b.sin());
            // The ankle is hidden at one frame: bridged.
            if i == 5 {
                p[point::LEFT_ANKLE][2] = 0.0;
            }
            cap.times.push(i as f64 / 10.0);
            cap.frames.push(Some(p));
        }
        cap
    }

    fn biped() -> m::Skeleton {
        let mut s = m::Skeleton { aspect: 0.5, ..m::Skeleton::default() };
        let v = m::Vec2::new;
        let hips = s.add("hips", None, v(0.5, 0.55), v(0.5, 0.5));
        let spine = s.add("spine", Some(hips), v(0.5, 0.55), v(0.5, 0.3));
        s.add("head", Some(spine), v(0.5, 0.3), v(0.5, 0.15));
        let up = s.add("upper_arm.front", Some(spine), v(0.52, 0.32), v(0.52, 0.45));
        s.add("forearm.front", Some(up), v(0.52, 0.45), v(0.52, 0.58));
        let th = s.add("thigh.back", Some(hips), v(0.48, 0.55), v(0.48, 0.75));
        s.add("shin.back", Some(th), v(0.48, 0.75), v(0.48, 0.95));
        s
    }

    #[test]
    fn a_raised_arm_and_a_bent_knee_drive_the_bones() {
        let sk = biped();
        let r = retarget(&raising(), &sk, &Options { smooth: 0.0, ..Options::default() }, 2.0).unwrap();
        let up = sk.named("upper_arm.front").unwrap().id;
        let keys = &r.rotations[&up];
        assert_eq!(keys.len(), 11);
        assert!((keys[0].0 - 2.0).abs() < 1e-9);
        assert!(keys[0].1.abs() < 2.0, "starts hanging: {}", keys[0].1);
        // Pointing forward (+x) from down is a counter-clockwise quarter
        // turn.
        assert!((keys[10].1 + 90.0).abs() < 3.0, "ends forward: {}", keys[10].1);
        // The forearm follows its parent: no turn of its own.
        let fore = sk.named("forearm.front").unwrap().id;
        assert!(r.rotations[&fore].iter().all(|k| k.1.abs() < 3.0));
        let shin = sk.named("shin.back").unwrap().id;
        assert!((r.rotations[&shin][10].1 - 40.0).abs() < 3.0, "{}", r.rotations[&shin][10].1);
        assert!(r.driven.iter().all(|d| d.1 == "direction"), "{:?}", r.driven);
        // Smoothing lags a fast move but lands near it; mirroring turns
        // the arm the other way.
        let s = retarget(&raising(), &sk, &Options { smooth: 1.0, ..Options::default() }, 0.0).unwrap();
        assert!(s.rotations[&up][5].1 > keys[5].1);
        let m = retarget(&raising(), &sk, &Options { smooth: 0.0, mirror: true, ..Options::default() }, 0.0).unwrap();
        assert!((m.rotations[&up][10].1 - 90.0).abs() < 3.0, "{}", m.rotations[&up][10].1);
    }

    #[test]
    fn a_dog_takes_turns_not_directions_for_its_spine() {
        let mut s = m::Skeleton { aspect: 1.6, ..m::Skeleton::default() };
        let v = m::Vec2::new;
        let hips = s.add("hips", None, v(0.3, 0.4), v(0.35, 0.4));
        let spine = s.add("spine", Some(hips), v(0.3, 0.4), v(0.7, 0.4));
        let fl = s.add("front_leg.upper", Some(spine), v(0.7, 0.5), v(0.7, 0.7));
        s.add("front_leg.lower", Some(fl), v(0.7, 0.7), v(0.7, 0.9));
        let r = retarget(&raising(), &s, &Options { smooth: 0.0, strength: 0.5, ..Options::default() }, 0.0).unwrap();
        let d: BTreeMap<&str, &str> = r.driven.iter().map(|x| (x.0.as_str(), x.1)).collect();
        assert_eq!(d["spine"], "turn");
        assert_eq!(d["front_leg.upper"], "direction");
        // Half strength: the leg swings half as far as the arm did.
        assert!((r.rotations[&fl][10].1 + 45.0).abs() < 3.0, "{}", r.rotations[&fl][10].1);
        assert!(retarget(&Capture::default(), &s, &Options::default(), 0.0).is_err());
        let mut bad = Options::default();
        bad.map.insert("spine".into(), "wing".into());
        assert!(retarget(&raising(), &s, &bad, 0.0).is_err());
    }
}
