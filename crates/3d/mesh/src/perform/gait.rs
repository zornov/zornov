// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Locomotion with planted feet, for any rigged character.
//!
//! A walk is planned from the feet, not the body. The body travels a path
//! with a speed profile (it starts, cruises, slows and stops); the gait
//! phase advances with the distance travelled, so stride matches speed.
//! Each foot is put down once per cycle and **stays exactly there** until
//! it lifts: its place is under the body where the body will be in the
//! middle of that step. Legs then reach those places by IK, so a foot in
//! contact cannot slide, and the last steps close up beside each other
//! (feet apart, never crossed). Turns on the spot, curved paths and
//! uneven ground (a height for any point) use the same machinery.
//!
//! The result is baked: the clip's position and heading per frame, and a
//! pose for every bone, ready to play as an action.

use super::body::{Body, Kind, Limb, Side};
use super::solve::{Solver, rotated};
use crate::rig::{BonePose, Rig, q_axis_angle, q_mul};
use crate::{V3, add, cross, dot, normalise, scale, sub};
use std::f32::consts::{PI, TAU};

/// Named gaits (a quadruped's `run` is a gallop, its `jog` a trot).
pub const GAITS: &[&str] = &["walk", "stroll", "march", "sneak", "tiptoe", "trudge", "limp", "jog", "run", "trot", "gallop"];

/// How a gait is played: each 0..1 (energy around 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub weary: f32,
    pub brisk: f32,
    pub cautious: f32,
    pub heavy: f32,
    /// Size of the movements, 0.2–2 (1 = natural).
    pub energy: f32,
    /// The injured side of a limp.
    pub limp: Option<Side>,
}

impl Default for Style {
    fn default() -> Style {
        Style { weary: 0.0, brisk: 0.0, cautious: 0.0, heavy: 0.0, energy: 1.0, limp: None }
    }
}

/// The numbers of a gait. Lengths are in leg lengths (hip height).
#[derive(Clone, Debug, PartialEq)]
pub struct Gait {
    pub name: String,
    /// Leg lengths per second.
    pub speed: f32,
    /// Leg lengths per full cycle (two steps of a biped).
    pub stride: f32,
    /// Fraction of the cycle a foot is down.
    pub duty: f32,
    /// Per leg (as `Body::legs`): where in the cycle it lands.
    pub phases: Vec<f32>,
    pub clearance: f32,
    pub bob: f32,
    pub sway: f32,
    pub crouch: f32,
    pub lean: f32,
    pub arm_swing: f32,
    pub elbow: f32,
    pub twist: f32,
    pub head_drop: f32,
    pub toe_out: f32,
    /// A leg that is hurt: shorter and softer.
    pub limp: Option<usize>,
}

fn names_of(kind: Kind, name: &str) -> &'static str {
    match (kind, name) {
        (Kind::Biped, "walk" | "stride" | "amble") => "walk",
        (Kind::Biped, "creep") => "sneak",
        (Kind::Biped, "sprint") => "run",
        (Kind::Biped, "shuffle") => "trudge",
        (Kind::Biped, "stroll") => "stroll",
        (Kind::Biped, "march") => "march",
        (Kind::Biped, "sneak") => "sneak",
        (Kind::Biped, "tiptoe") => "tiptoe",
        (Kind::Biped, "trudge") => "trudge",
        (Kind::Biped, "limp") => "limp",
        (Kind::Biped, "jog" | "trot") => "jog",
        (Kind::Biped, "run" | "gallop") => "run",
        (_, "run" | "gallop" | "sprint") => "gallop",
        (_, "jog" | "trot") => "trot",
        (_, "sneak" | "creep" | "tiptoe") => "sneak",
        (_, "limp") => "limp",
        _ => "walk",
    }
}

/// The gait called `name` for this body, with a style applied.
pub fn gait(name: &str, body: &Body, style: &Style) -> Result<Gait, String> {
    let lower = name.trim().to_lowercase();
    if !GAITS.contains(&lower.as_str()) && !["stride", "amble", "creep", "sprint", "shuffle"].contains(&lower.as_str()) {
        return Err(format!("gait \"{name}\": {}", GAITS.join(", ")));
    }
    if body.legs.is_empty() {
        return Err("the character has no legs to walk on".into());
    }
    let n = names_of(body.kind, &lower);
    //            speed stride duty clear  bob    sway  crouch lean arms elbow twist
    let t: [f32; 11] = match (body.kind, n) {
        (Kind::Biped, "walk") => [1.4, 1.3, 0.62, 0.07, 0.05, 0.035, 0.02, 3.0, 20.0, 10.0, 6.0],
        (Kind::Biped, "stroll") => [0.95, 1.1, 0.66, 0.05, 0.04, 0.04, 0.02, 2.0, 12.0, 8.0, 5.0],
        (Kind::Biped, "march") => [1.6, 1.4, 0.60, 0.16, 0.05, 0.02, 0.02, 2.0, 30.0, 20.0, 4.0],
        (Kind::Biped, "sneak") => [0.75, 1.0, 0.70, 0.10, 0.03, 0.02, 0.10, 14.0, 8.0, 30.0, 3.0],
        (Kind::Biped, "tiptoe") => [0.6, 0.9, 0.72, 0.12, 0.02, 0.02, 0.12, 10.0, 6.0, 35.0, 3.0],
        (Kind::Biped, "trudge") => [0.7, 0.9, 0.68, 0.03, 0.03, 0.045, 0.05, 12.0, 6.0, 6.0, 4.0],
        (Kind::Biped, "limp") => [0.8, 1.1, 0.64, 0.07, 0.05, 0.05, 0.04, 6.0, 8.0, 8.0, 5.0],
        (Kind::Biped, "jog") => [2.6, 1.9, 0.45, 0.12, 0.08, 0.03, 0.05, 8.0, 35.0, 70.0, 10.0],
        (Kind::Biped, "run") => [4.2, 2.9, 0.36, 0.17, 0.11, 0.03, 0.07, 12.0, 50.0, 85.0, 12.0],
        (_, "walk") => [1.5, 1.9, 0.68, 0.06, 0.03, 0.03, 0.02, 0.0, 0.0, 0.0, 0.0],
        (_, "sneak") => [0.8, 1.3, 0.75, 0.08, 0.02, 0.02, 0.10, 0.0, 0.0, 0.0, 0.0],
        (_, "limp") => [1.0, 1.5, 0.68, 0.07, 0.04, 0.03, 0.04, 0.0, 0.0, 0.0, 0.0],
        (_, "trot") => [3.2, 2.7, 0.48, 0.09, 0.05, 0.02, 0.02, 0.0, 0.0, 0.0, 0.0],
        _ => [6.5, 4.2, 0.32, 0.14, 0.09, 0.02, 0.03, 0.0, 0.0, 0.0, 0.0],
    };
    let mut phases: Vec<f32> = Vec::new();
    for (i, l) in body.legs.iter().enumerate() {
        let left = l.side != Side::R;
        let p = match (body.kind, n) {
            (Kind::Biped, _) => {
                if left {
                    0.0
                } else {
                    0.5
                }
            }
            (Kind::Quadruped, "trot") => {
                // Diagonal pairs.
                if left == !l.front {
                    0.0
                } else {
                    0.5
                }
            }
            (Kind::Quadruped, "gallop") => match (l.front, left) {
                (false, true) => 0.0,
                (false, false) => 0.12,
                (true, true) => 0.55,
                (true, false) => 0.65,
            },
            (Kind::Quadruped, _) => match (l.front, left) {
                (false, true) => 0.0,
                (true, true) => 0.25,
                (false, false) => 0.5,
                (true, false) => 0.75,
            },
            // Other creatures: legs alternate down the list.
            _ => {
                if (i + usize::from(left)) % 2 == 0 {
                    0.0
                } else {
                    0.5
                }
            }
        };
        phases.push(p);
    }
    let mut g = Gait {
        name: n.to_owned(),
        speed: t[0],
        stride: t[1],
        duty: t[2],
        phases,
        clearance: t[3],
        bob: t[4],
        sway: t[5],
        crouch: t[6],
        lean: t[7],
        arm_swing: t[8],
        elbow: t[9],
        twist: t[10],
        head_drop: if n == "trudge" { 15.0 } else { 0.0 },
        toe_out: 5.0,
        limp: None,
    };
    if n == "limp" || style.limp.is_some() {
        let side = style.limp.unwrap_or(Side::L);
        g.limp = body.legs.iter().position(|l| l.side == side && (body.kind == Kind::Biped || !l.front));
    }
    let (w, b, c, h) = (style.weary.clamp(0.0, 1.0), style.brisk.clamp(0.0, 1.0), style.cautious.clamp(0.0, 1.0), style.heavy.clamp(0.0, 1.0));
    let e = style.energy.clamp(0.2, 2.0);
    g.speed *= (1.0 - 0.3 * w) * (1.0 + 0.3 * b) * (1.0 - 0.3 * c) * (1.0 - 0.15 * h) * (0.7 + 0.3 * e);
    g.stride *= (1.0 - 0.15 * w) * (1.0 + 0.1 * b) * (1.0 - 0.1 * c);
    g.clearance *= (1.0 - 0.55 * w) * (1.0 + 0.2 * b) * (1.0 + 0.3 * c) * (1.0 - 0.25 * h) * e;
    g.lean = (g.lean + 10.0 * w + 4.0 * b + 3.0 * c) * e.sqrt();
    g.head_drop += 14.0 * w;
    g.arm_swing *= (1.0 - 0.5 * w) * (1.0 + 0.35 * b) * (1.0 - 0.4 * c) * (1.0 - 0.15 * h) * e;
    g.bob *= (1.0 - 0.3 * w) * (1.0 + 0.2 * b) * (1.0 + 0.4 * h) * e;
    g.sway *= (1.0 + 0.4 * w) * (1.0 + 0.6 * h) * e;
    g.duty = (g.duty + 0.04 * w + 0.04 * c + 0.03 * h).min(0.8);
    g.crouch += 0.02 * w + 0.06 * c;
    g.twist *= e;
    // The planted foot may be at most half a leg ahead of or behind the hip.
    let cap = 1.05 / g.duty.max(0.2);
    if g.stride > cap {
        g.speed *= cap / g.stride;
        g.stride = cap;
    }
    Ok(g)
}

/// Where the character goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Waypoint {
    pub x: f32,
    pub z: f32,
    /// Stop here (always true for the last); otherwise the path flows through.
    pub stop: bool,
    /// Stand here this long (seconds).
    pub pause: f32,
    /// Turn to face this heading (radians) once here.
    pub face: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub start: [f32; 2],
    /// The way the character faces at the start (radians; 0 = toward -z).
    pub heading: f32,
    pub waypoints: Vec<Waypoint>,
    /// World px per second, overriding the gait's own.
    pub speed: Option<f32>,
    /// Seconds for the whole thing (the speed is found from it).
    pub duration: Option<f32>,
    /// Seconds to hold still at the end after the feet close up.
    pub hold: f32,
    /// Seconds before the first step.
    pub lead: f32,
}

/// One baked instant.
#[derive(Clone, Debug)]
pub struct WalkFrame {
    pub t: f32,
    /// The clip's place on the ground and its turn about up (degrees,
    /// `rotation_y`), and how high the ground is under it.
    pub x: f32,
    pub z: f32,
    pub yaw_deg: f32,
    pub ground: f32,
    pub poses: Vec<BonePose>,
    /// Per leg: planted.
    pub planted: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct Walk {
    pub fps: f32,
    pub frames: Vec<WalkFrame>,
    pub duration: f32,
    /// Metres of path walked, in world px.
    pub distance: f32,
    pub steps: usize,
    pub notes: Vec<String>,
    /// How long it takes the feet to close up at the end.
    pub settle: f32,
}

/// The turn from the model's forward to -z, radians: the clip is turned by
/// `heading - this` to face `heading`.
pub fn heading_offset(body: &Body) -> f32 {
    body.fwd[0].atan2(-body.fwd[2])
}

fn wrap(a: f32) -> f32 {
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    }
    if a <= -PI {
        a += TAU;
    }
    a
}

fn fwd2(h: f32) -> [f32; 2] {
    [h.sin(), -h.cos()]
}

fn left2(h: f32) -> [f32; 2] {
    [h.cos(), h.sin()]
}

fn smooth(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

// ----------------------------------------------------------------- path

struct Path {
    pts: Vec<[f32; 2]>,
    cum: Vec<f32>,
}

impl Path {
    fn new(pts: Vec<[f32; 2]>) -> Path {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
            cum.push(cum.last().copied().unwrap_or(0.0) + d);
        }
        Path { pts, cum }
    }

    fn len(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// The point and heading at arc length `s`.
    fn at(&self, s: f32) -> ([f32; 2], f32) {
        let s = s.clamp(0.0, self.len());
        let i = self.cum.partition_point(|&c| c <= s).clamp(1, self.pts.len().saturating_sub(1).max(1)) - 1;
        let (a, b) = (self.pts[i], self.pts[(i + 1).min(self.pts.len() - 1)]);
        let seg = (self.cum[(i + 1).min(self.cum.len() - 1)] - self.cum[i]).max(1e-6);
        let u = ((s - self.cum[i]) / seg).clamp(0.0, 1.0);
        let p = [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u];
        let h = (b[0] - a[0]).atan2(-(b[1] - a[1]));
        (p, h)
    }
}

/// The path with its corners rounded by quadratic curves of up to `r`.
fn rounded(points: &[[f32; 2]], r: f32) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for i in 1..points.len() - 1 {
        let (a, p, b) = (points[i - 1], points[i], points[i + 1]);
        let da = ((a[0] - p[0]).powi(2) + (a[1] - p[1]).powi(2)).sqrt();
        let db = ((b[0] - p[0]).powi(2) + (b[1] - p[1]).powi(2)).sqrt();
        let rr = r.min(da * 0.5).min(db * 0.5);
        if rr < 1e-3 {
            out.push(p);
            continue;
        }
        let q = [p[0] + (a[0] - p[0]) / da * rr, p[1] + (a[1] - p[1]) / da * rr];
        let e = [p[0] + (b[0] - p[0]) / db * rr, p[1] + (b[1] - p[1]) / db * rr];
        for k in 0..=8 {
            let u = k as f32 / 8.0;
            let w = [(1.0 - u).powi(2), 2.0 * u * (1.0 - u), u * u];
            out.push([q[0] * w[0] + p[0] * w[1] + e[0] * w[2], q[1] * w[0] + p[1] * w[1] + e[1] * w[2]]);
        }
    }
    out.push(points[points.len() - 1]);
    out
}

// ------------------------------------------------------------- timeline

/// One stretch of motion from rest to rest, sampled at `dt`.
struct Seg {
    x: Vec<f32>,
    z: Vec<f32>,
    head: Vec<f32>,
    v: Vec<f32>,
    phase: Vec<f32>,
    /// Samples where the path and turns end and the feet close up.
    moved_to: usize,
}

struct Stance {
    land: f32,
    lift: f32,
    pos: [f32; 2],
    h: f32,
    yaw: f32,
}

struct Ctx<'a> {
    rig: &'a Rig,
    body: &'a Body,
    gait: &'a Gait,
    k: f32,
    ell: f32,
    alpha: f32,
    /// Per leg: (forward, left) of the ankle from the model's origin, world px.
    neutral: Vec<[f32; 2]>,
    ankle_rest: Vec<V3>,
    bend: Vec<V3>,
    cruise: f32,
    stride: f32,
    dt: f32,
    ground: &'a dyn Fn(f32, f32) -> f32,
}

impl Ctx<'_> {
    fn leg_duty(&self, leg: usize, k: usize) -> f32 {
        let g = self.gait;
        let first = (0.62 - 0.13 * k as f32).max(g.duty);
        let d = first.max(g.duty);
        match g.limp {
            Some(j) if j == leg => (d - 0.12).max(0.3),
            Some(_) => (d + 0.05).min(0.85),
            None => d,
        }
    }

    /// The root over a stretch from rest to rest.
    fn root(&self, plan: &Plan, from: [f32; 2], heading: f32, wp: &[&Waypoint], cruise: f32) -> (Seg, [f32; 2], f32) {
        let dt = self.dt;
        // Animals with four beats to the step get going more slowly: each leg waits its turn to lift.
        let acc = cruise / (0.9 + 0.35 * (self.body.legs.len() as f32 - 2.0).max(0.0));
        let dec = cruise / 0.7;
        let lat = 3.0 * self.ell; // sideways acceleration allowed
        let mut pts = vec![from];
        for w in wp {
            let p = [w.x, w.z];
            if ((p[0] - pts[pts.len() - 1][0]).powi(2) + (p[1] - pts[pts.len() - 1][1]).powi(2)).sqrt() > 1e-3 {
                pts.push(p);
            }
        }
        let path = Path::new(rounded(&pts, 0.9 * self.ell));
        let mut x = vec![from[0]];
        let mut z = vec![from[1]];
        let mut head = vec![heading];
        let mut v = vec![0.0f32];
        let mut cur = heading;
        let turn_in_place = |to: f32, x: &mut Vec<f32>, z: &mut Vec<f32>, head: &mut Vec<f32>, v: &mut Vec<f32>, cur: &mut f32| {
            let d = wrap(to - *cur);
            if d.abs() < 0.12 {
                *cur = to;
                return;
            }
            let secs = (d.abs() / (4.0 * (0.6 + 0.4 * cruise / (1.5 * self.ell)))).max(0.35);
            let n = (secs / dt).ceil() as usize;
            let (p, h0) = (*x.last().unwrap_or(&0.0), *cur);
            let pz = *z.last().unwrap_or(&0.0);
            for i in 1..=n {
                let u = smooth(i as f32 / n as f32);
                x.push(p);
                z.push(pz);
                head.push(h0 + d * u);
                v.push(0.0);
            }
            *cur = h0 + d;
        };
        let s_total = path.len();
        if s_total > 1e-3 {
            let (_, h_first) = path.at(0.0);
            turn_in_place(h_first, &mut x, &mut z, &mut head, &mut v, &mut cur);
            let mut s = 0.0f32;
            let mut vel = 0.0f32;
            let mut guard = 0usize;
            while s_total - s > 0.4 && guard < 400_000 {
                guard += 1;
                let remaining = s_total - s;
                let v_stop = (2.0 * dec * remaining).sqrt();
                // The bend ahead limits the speed.
                let (_, h0) = path.at(s);
                let (_, h1) = path.at(s + 0.5 * self.ell);
                let kappa = wrap(h1 - h0).abs() / (0.5 * self.ell);
                let v_curve = if kappa > 1e-4 { (lat / kappa).sqrt() } else { f32::INFINITY };
                let target = cruise.min(v_stop).min(v_curve).max(0.06 * cruise);
                vel += (target - vel).clamp(-2.0 * dec * dt, acc * dt);
                s = (s + vel * dt).min(s_total);
                let (p, th) = path.at(s + 0.1 * self.ell * (vel / cruise.max(1e-3)));
                let (pp, _) = path.at(s);
                let _ = p;
                // Heading follows the path a little ahead, smoothed.
                cur += wrap(th - cur) * (1.0 - (-dt / 0.12).exp());
                x.push(pp[0]);
                z.push(pp[1]);
                head.push(cur);
                v.push(vel);
            }
            // Arrive exactly.
            let (pe, _) = path.at(s_total);
            if let (Some(lx), Some(lz)) = (x.last_mut(), z.last_mut()) {
                *lx = pe[0];
                *lz = pe[1];
            }
        }
        // Turn to face, if asked, standing there.
        if let Some(face) = wp.last().and_then(|w| w.face) {
            turn_in_place(face, &mut x, &mut z, &mut head, &mut v, &mut cur);
        }
        let _ = plan;
        let moved_to = x.len();
        let end = [*x.last().unwrap_or(&from[0]), *z.last().unwrap_or(&from[1])];
        let end_head = *head.last().unwrap_or(&heading);
        (Seg { x, z, head, v, phase: Vec::new(), moved_to }, end, end_head)
    }

    /// Phase along the stretch, then the extra steps that close the feet up.
    /// `down` are where the feet stand at the start. Returns the foot tracks.
    fn plant(&self, seg: &mut Seg, down: &[Stance]) -> Vec<Vec<Stance>> {
        let dt = self.dt;
        let g = self.gait;
        let n = seg.x.len();
        let cycle_rate = self.cruise / self.stride;
        let mut ph = vec![0.0f32; n];
        // All feet down at the start; the first lift comes soon.
        let nlegs = g.phases.len();
        let first_lift = |p: f32, d: f32| -> f32 {
            let mut f = p + d;
            while f < 0.0 {
                f += 1.0;
            }
            f
        };
        let phi0 = (0..nlegs).map(|i| first_lift(g.phases[i], self.leg_duty(i, 0))).fold(f32::INFINITY, f32::min) - 0.10;
        ph[0] = phi0;
        let mut yaw_prev = seg.head[0];
        for i in 1..n {
            let w = wrap(seg.head[i] - yaw_prev).abs() / dt;
            yaw_prev = seg.head[i];
            let in_motion = i < seg.moved_to && (seg.v[i] > 1e-3 || w > 0.05);
            let rate = if in_motion { (seg.v[i] / self.stride).max(w / (PI * 0.5)).max(cycle_rate) } else { 0.0 };
            ph[i] = ph[i - 1] + rate * dt;
        }
        seg.phase = ph;
        // Extend with the closing steps: slow cycles of a stationary body.
        let rate_settle = 0.9;
        let (ex, ez, eh) = (*seg.x.last().unwrap_or(&0.0), *seg.z.last().unwrap_or(&0.0), *seg.head.last().unwrap_or(&0.0));
        let mut extra = 0usize;
        let mut tracks = self.tracks(seg, down);
        let max_extra = (2.5 / (rate_settle * dt)) as usize;
        loop {
            // Done when every foot stands (not mid-swing) near its neutral place.
            let t_end = (seg.x.len() - 1) as f32 * dt;
            let ok = tracks.iter().enumerate().all(|(li, tr)| {
                let Some(s) = tr.iter().rev().find(|s| s.land <= t_end + 1e-6) else { return false };
                let down = s.lift > t_end;
                let want = [ex + fwd2(eh)[0] * self.neutral[li][0] + left2(eh)[0] * self.neutral[li][1], ez + fwd2(eh)[1] * self.neutral[li][0] + left2(eh)[1] * self.neutral[li][1]];
                down && ((s.pos[0] - want[0]).powi(2) + (s.pos[1] - want[1]).powi(2)).sqrt() < 0.07 * self.ell
            });
            if ok || extra >= max_extra {
                break;
            }
            for _ in 0..4 {
                seg.x.push(ex);
                seg.z.push(ez);
                seg.head.push(eh);
                seg.v.push(0.0);
                let last = *seg.phase.last().unwrap_or(&0.0);
                seg.phase.push(last + rate_settle * dt);
                extra += 1;
            }
            tracks = self.tracks(seg, down);
        }
        tracks
    }

    /// When the phase first reaches `value` (seconds into the stretch).
    fn time_of(seg: &Seg, dt: f32, value: f32) -> f32 {
        let i = seg.phase.partition_point(|&p| p < value);
        if i == 0 {
            return 0.0;
        }
        if i >= seg.phase.len() {
            return (seg.phase.len() - 1) as f32 * dt;
        }
        let (a, b) = (seg.phase[i - 1], seg.phase[i]);
        let u = if b > a { (value - a) / (b - a) } else { 1.0 };
        (i as f32 - 1.0 + u) * dt
    }

    fn at(seg: &Seg, dt: f32, t: f32) -> ([f32; 2], f32) {
        let f = (t / dt).clamp(0.0, (seg.x.len() - 1) as f32);
        let i = f.floor() as usize;
        let j = (i + 1).min(seg.x.len() - 1);
        let u = f - i as f32;
        ([seg.x[i] + (seg.x[j] - seg.x[i]) * u, seg.z[i] + (seg.z[j] - seg.z[i]) * u], seg.head[i] + wrap(seg.head[j] - seg.head[i]) * u)
    }

    fn tracks(&self, seg: &Seg, down: &[Stance]) -> Vec<Vec<Stance>> {
        let g = self.gait;
        let dt = self.dt;
        let phi_end = *seg.phase.last().unwrap_or(&0.0);
        let phi0 = seg.phase[0];
        let mut out = Vec::new();
        for (li, d0) in down.iter().enumerate() {
            let p = g.phases[li];
            let mut list: Vec<Stance> = Vec::new();
            // Standing at the start until the first lift.
            let mut n_int = (phi0 - p - 1.0).floor() as i32;
            // The first lift at or after the start.
            let mut lift_phase;
            let mut k = 0usize;
            loop {
                let land = p + n_int as f32;
                let duty = self.leg_duty(li, k);
                if land + duty >= phi0 + 0.02 {
                    lift_phase = land + duty;
                    break;
                }
                n_int += 1;
            }
            let mut first = Stance { land: f32::NEG_INFINITY, lift: Self::time_of(seg, dt, lift_phase), pos: d0.pos, h: d0.h, yaw: d0.yaw };
            if lift_phase > phi_end {
                first.lift = f32::INFINITY;
            }
            let mut done = !first.lift.is_finite();
            list.push(first);
            while !done {
                // Next landing after the lift.
                let mut land = p + n_int as f32;
                while land <= lift_phase {
                    land += 1.0;
                    n_int += 1;
                }
                if land > phi_end {
                    break;
                }
                k += 1;
                let duty = self.leg_duty(li, k);
                let land_t = Self::time_of(seg, dt, land);
                let mid_t = Self::time_of(seg, dt, (land + 0.5 * duty).min(phi_end));
                let (c, h) = Self::at(seg, dt, mid_t);
                let (f, l) = (fwd2(h), left2(h));
                let pos = [c[0] + f[0] * self.neutral[li][0] + l[0] * self.neutral[li][1], c[1] + f[1] * self.neutral[li][0] + l[1] * self.neutral[li][1]];
                let side = self.body.legs[li].side.sign();
                let lift_phase_next = land + duty;
                let lift_t = if lift_phase_next > phi_end { f32::INFINITY } else { Self::time_of(seg, dt, lift_phase_next) };
                list.push(Stance { land: land_t, lift: lift_t, pos, h: (self.ground)(pos[0], pos[1]), yaw: h + side * g.toe_out.to_radians() });
                if !lift_t.is_finite() {
                    done = true;
                }
                lift_phase = lift_phase_next;
                n_int += 1;
            }
            out.push(list);
        }
        out
    }

    /// A foot at time `t`: where, how high above its ground, its turn,
    /// whether it is down, and how far through its swing (0 when down).
    fn foot(&self, tr: &[Stance], t: f32, leg: usize) -> (V3, f32, bool, f32) {
        let g = self.gait;
        let idx = tr.iter().rposition(|s| s.land <= t).unwrap_or(0);
        let s = &tr[idx];
        if t < s.lift || idx + 1 >= tr.len() {
            return ([s.pos[0], s.h, s.pos[1]], s.yaw, true, 0.0);
        }
        let n = &tr[idx + 1];
        let span = (n.land - s.lift).max(1e-4);
        let u = ((t - s.lift) / span).clamp(0.0, 1.0);
        let e = smooth(u);
        let pos = [s.pos[0] + (n.pos[0] - s.pos[0]) * e, s.pos[1] + (n.pos[1] - s.pos[1]) * e];
        let clear = g.clearance * self.ell * if g.limp == Some(leg) { 0.8 } else { 1.0 };
        let arch = (u * PI).sin();
        let hy = s.h + (n.h - s.h) * e + clear * arch.powf(0.8) + 0.4 * (n.h - s.h).abs().min(self.ell * 0.5) * arch;
        let yaw = s.yaw + wrap(n.yaw - s.yaw) * e;
        ([pos[0], hy, pos[1]], yaw, false, u)
    }
}

/// Plan and bake a walk.
pub fn walk(rig: &Rig, body: &Body, gait: &Gait, scale_k: f32, plan: &Plan, fps: f32, ground: &dyn Fn(f32, f32) -> f32) -> Result<Walk, String> {
    if let Some(e) = body.missing_for_legs(rig) {
        return Err(e);
    }
    if !body.upright() {
        return Err(format!("the rig stands along {:?}, not up the y axis: rotate it upright before walking it", body.up));
    }
    if plan.waypoints.is_empty() {
        return Err("a walk needs at least one place to go (route: [[x, z], ...] or to)".into());
    }
    let k = scale_k.abs().max(1e-3);
    let mut solver = Solver::new(rig);
    let sole = body.sole_height(rig);
    let hip_y = dot(solver.head(body.legs[0].upper), body.up);
    let ell = ((hip_y - sole).abs() * k).max(1e-3);
    let origin = [0.0f32; 3];
    let f = body.fwd;
    let l = body.left();
    let mut neutral = Vec::new();
    let mut ankle_rest = Vec::new();
    let mut bend = Vec::new();
    for leg in &body.legs {
        let a = solver.head(leg.end);
        ankle_rest.push(a);
        let d = sub(a, origin);
        neutral.push([dot(d, f) * k, dot(d, l) * k]);
        bend.push(super::pose::bend_of_leg(body, rig, leg));
    }
    let alpha = heading_offset(body);
    let dt = 1.0 / (fps * 4.0);
    // The speed: asked for, or the gait's, or found from a duration.
    let mut cruise = plan.speed.unwrap_or(gait.speed * ell).max(0.05 * ell);
    let mut ctx = Ctx { rig, body, gait, k, ell, alpha, neutral, ankle_rest, bend, cruise, stride: 1.0, dt, ground };
    let stride_for = |c: f32, ctx: &Ctx| -> f32 {
        let nominal = ctx.gait.speed * ctx.ell;
        let ratio = (c / nominal.max(1e-3)).clamp(0.2, 4.0);
        (ctx.gait.stride * ctx.ell * ratio.powf(0.6)).clamp(0.5 * ctx.gait.stride * ctx.ell, 1.7 * ctx.gait.stride * ctx.ell)
    };
    // Stretches: a stop (or the end) closes each.
    let mut groups: Vec<Vec<&Waypoint>> = vec![Vec::new()];
    for (i, w) in plan.waypoints.iter().enumerate() {
        groups.last_mut().map(|g| g.push(w));
        if w.stop || w.pause > 0.0 || w.face.is_some() || i + 1 == plan.waypoints.len() {
            groups.push(Vec::new());
        }
    }
    groups.retain(|g| !g.is_empty());
    let run = |ctx: &mut Ctx, cruise: f32| -> (Vec<(Seg, Vec<Vec<Stance>>)>, f32) {
        ctx.cruise = cruise;
        ctx.stride = stride_for(cruise, ctx);
        let mut pos = plan.start;
        let mut head = plan.heading;
        // Feet start at their neutral places under the first position.
        let mut feet: Vec<Stance> = (0..ctx.body.legs.len())
            .map(|li| {
                let (fw, lf) = (fwd2(head), left2(head));
                let p = [pos[0] + fw[0] * ctx.neutral[li][0] + lf[0] * ctx.neutral[li][1], pos[1] + fw[1] * ctx.neutral[li][0] + lf[1] * ctx.neutral[li][1]];
                Stance { land: f32::NEG_INFINITY, lift: f32::INFINITY, pos: p, h: (ctx.ground)(p[0], p[1]), yaw: head + ctx.body.legs[li].side.sign() * ctx.gait.toe_out.to_radians() }
            })
            .collect();
        let mut segs = Vec::new();
        let mut total = 0.0;
        for g in &groups {
            let (mut seg, end, end_head) = ctx.root(plan, pos, head, g, cruise);
            let tracks = ctx.plant(&mut seg, &feet);
            // Pad to whole frames.
            while seg.x.len() % 4 != 1 {
                let (ex, ez, eh, ph) = (*seg.x.last().unwrap_or(&0.0), *seg.z.last().unwrap_or(&0.0), *seg.head.last().unwrap_or(&0.0), *seg.phase.last().unwrap_or(&0.0));
                seg.x.push(ex);
                seg.z.push(ez);
                seg.head.push(eh);
                seg.v.push(0.0);
                seg.phase.push(ph);
            }
            // The feet at the end stand where the tracks left them.
            feet = tracks
                .iter()
                .map(|tr| {
                    let s = tr.last().expect("a track has a stance");
                    Stance { land: f32::NEG_INFINITY, lift: f32::INFINITY, pos: s.pos, h: s.h, yaw: s.yaw }
                })
                .collect();
            pos = end;
            head = end_head;
            total += (seg.x.len() - 1) as f32 * ctx.dt;
            // A pause: standing, feet planted.
            let pause = g.last().map_or(0.0, |w| w.pause);
            if pause > 0.0 {
                let n = ((pause / ctx.dt) as usize / 4) * 4;
                let ph = *seg.phase.last().unwrap_or(&0.0);
                for _ in 0..n {
                    seg.x.push(end[0]);
                    seg.z.push(end[1]);
                    seg.head.push(end_head);
                    seg.v.push(0.0);
                    seg.phase.push(ph);
                }
                total += n as f32 * ctx.dt;
            }
            segs.push((seg, tracks));
        }
        (segs, total)
    };
    let mut result = run(&mut ctx, cruise);
    if let (Some(want), None) = (plan.duration, plan.speed) {
        let want = (want - plan.lead - plan.hold).max(0.5);
        for _ in 0..4 {
            let moving = result.1;
            if (moving - want).abs() < 0.04 * want {
                break;
            }
            cruise = (cruise * (moving / want).powf(0.9)).clamp(0.08 * ell, 8.0 * ell);
            result = run(&mut ctx, cruise);
        }
    }
    let (segs, _) = result;
    // Bake frames at fps: lead, the stretches, the hold.
    let lead_frames = (plan.lead * fps).round() as usize;
    let hold_frames = (plan.hold * fps).round() as usize;
    let mut raw: Vec<(usize, usize, f32)> = Vec::new(); // (segment, sample, time)
    for (si, (seg, _)) in segs.iter().enumerate() {
        let frames = (seg.x.len() - 1) / 4;
        for fr in 0..=frames {
            if si > 0 && fr == 0 {
                continue;
            }
            raw.push((si, fr * 4, 0.0));
        }
    }
    let mut frames: Vec<WalkFrame> = Vec::new();
    let mut notes = Vec::new();
    let mut steps = 0usize;
    let mut distance = 0.0f32;
    let mut prev_xz: Option<[f32; 2]> = None;
    let total_frames = lead_frames + raw.len() + hold_frames;
    // Smoothed ground under the body: the mean of the feet's ground heights.
    let mut g_raw: Vec<f32> = Vec::with_capacity(total_frames);
    let mut state: Vec<(usize, usize)> = Vec::with_capacity(total_frames);
    for i in 0..total_frames {
        if i < lead_frames {
            state.push((0, 0));
        } else if i < lead_frames + raw.len() {
            let (s, k2, _) = raw[i - lead_frames];
            state.push((s, k2));
        } else {
            let (s, _) = (segs.len() - 1, 0);
            state.push((s, segs[s].0.x.len() - 1));
        }
    }
    for (si, sample) in &state {
        let (seg, tracks) = &segs[*si];
        let t = *sample as f32 * dt;
        let mut gsum = 0.0;
        for (li, tr) in tracks.iter().enumerate() {
            let (p, _, _, _) = ctx.foot(tr, t, li);
            gsum += ground(p[0], p[2]);
        }
        let _ = seg;
        g_raw.push(gsum / tracks.len().max(1) as f32);
    }
    let win = (0.3 * fps) as usize;
    let g_root: Vec<f32> = (0..g_raw.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(win), (i + win).min(g_raw.len() - 1));
            g_raw[a..=b].iter().sum::<f32>() / (b - a + 1) as f32
        })
        .collect();
    let mut last_down = vec![true; body.legs.len()];
    let mut unreached = 0usize;
    for (i, (si, sample)) in state.iter().enumerate() {
        let (seg, tracks) = &segs[*si];
        let t = *sample as f32 * dt;
        let (x, z, h) = (seg.x[*sample], seg.z[*sample], seg.head[*sample]);
        let feet: Vec<(V3, f32, bool, f32)> = tracks.iter().enumerate().map(|(li, tr)| ctx.foot(tr, t, li)).collect();
        for (li, fo) in feet.iter().enumerate() {
            if fo.2 && !last_down[li] {
                steps += 1;
            }
            last_down[li] = fo.2;
        }
        if let Some(p) = prev_xz {
            distance += ((x - p[0]).powi(2) + (z - p[1]).powi(2)).sqrt();
        }
        prev_xz = Some([x, z]);
        let phase = seg.phase[*sample];
        let q = (seg.v[*sample] / ctx.cruise.max(1e-3)).clamp(0.0, 1.0);
        let (poses, missed) = pose_frame(&ctx, [x, z], h, g_root[i], phase, q, &feet)?;
        unreached += missed;
        frames.push(WalkFrame { t: i as f32 / fps, x, z, yaw_deg: (h - alpha).to_degrees(), ground: g_root[i], poses, planted: feet.iter().map(|f| f.2).collect() });
    }
    let settle = {
        // From the last moment the body moved to the last foot landing.
        let (seg, _) = &segs[segs.len() - 1];
        (seg.x.len() - seg.moved_to) as f32 * dt
    };
    if frames.len() < 2 {
        notes.push("the walk is shorter than two frames".into());
    }
    if unreached > 0 {
        notes.push(format!("{unreached} foot places were past the legs' reach and were stretched toward"));
    }
    Ok(Walk { fps, duration: (frames.len().saturating_sub(1)) as f32 / fps, frames, distance, steps, notes, settle })
}

/// Everything about one instant, as bone poses.
fn pose_frame(c: &Ctx, root: [f32; 2], heading: f32, g_root: f32, phase: f32, q: f32, feet: &[(V3, f32, bool, f32)]) -> Result<(Vec<BonePose>, usize), String> {
    let (rig, body, g, k) = (c.rig, c.body, c.gait, c.k);
    let a = heading - c.alpha;
    let (ca, sa) = (a.cos(), a.sin());
    let up = body.up;
    let uy = up[1].signum();
    let f = body.fwd;
    let left = body.left();
    let ell = c.ell;
    // A world point in the model's own axes (px of the model).
    let to_model = |x: f32, hgt: f32, z: f32, rest_y: f32| -> V3 {
        let (dx, dz) = (x - root[0], z - root[1]);
        [(dx * ca + dz * sa) / k, rest_y + uy * (hgt - g_root) / k, (-dx * sa + dz * ca) / k]
    };
    let nl = body.legs.len();
    // Stance weight and forwardness of each leg in the cycle.
    let mut weight = vec![0.0f32; nl];
    let mut fwdness = vec![0.0f32; nl];
    for i in 0..nl {
        let psi = (phase - g.phases[i]).rem_euclid(1.0);
        let duty = g.duty;
        weight[i] = if psi < duty { (PI * psi / duty).sin() } else { 0.0 };
        fwdness[i] = (TAU * (phase - g.phases[i])).cos();
    }
    let side_sum = |v: &[f32]| -> f32 { body.legs.iter().zip(v).map(|(l, x)| l.side.sign() * x).sum::<f32>() };
    let norm = (nl as f32 / 2.0).max(1.0);
    // The cycle's lowest instants.
    let kk = if body.kind == Kind::Biped { 2.0 } else { 2.0 };
    let low = if g.duty < 0.5 { g.duty / (2.0 * kk) } else { 0.0 };
    let bob = g.bob * ell * q * 0.5 * (1.0 + (TAU * kk * (phase - low)).cos());
    let sway = g.sway * ell * q * side_sum(&weight) / norm;
    // The body above the legs: pelvis place (lowered as far as asked), torso,
    // head, tail and arms. Legs come last, to the feet.
    let torso: Vec<usize> = body.spine.iter().copied().chain(body.chest).collect();
    let build = |extra: f32| -> Solver {
        let mut s = Solver::new(rig);
        let drop = g.crouch * ell * (0.3 + 0.7 * q) + bob + extra;
        // Up by how much higher the planted feet stand than the clip's ground, down by the crouch.
        let raise = (mean_ground_height(feet) - g_root) - drop;
        s.shift_root(body.root, add(scale(up, raise / k), scale(left, sway / k)));
        let twist = -g.twist * q * side_sum(&fwdness) / norm;
        if body.kind == Kind::Biped && twist.abs() > 1e-3 {
            s.rotate(body.root, up, twist);
            if let Some(b) = body.spine.first().copied().or(body.chest) {
                s.rotate(b, up, -1.6 * twist);
            }
        }
        let lean = g.lean * q;
        if body.kind == Kind::Biped {
            for &b in &torso {
                s.tilt_toward(b, f, lean / torso.len().max(1) as f32);
            }
            if let Some(n) = body.neck {
                s.tilt_toward(n, f, g.head_drop * 0.4);
            }
            if let Some(h) = body.head {
                s.tilt_toward(h, f, g.head_drop * 0.6 - lean * 0.7);
            }
            // Arms swing against the legs; hands hang loose.
            let swing_axis = cross(f, up);
            for arm in &body.arms {
                let opp = body.legs.iter().position(|l| l.side != arm.side && l.side != Side::Mid);
                let fw = opp.map_or(0.0, |i| fwdness[i]);
                let theta = g.arm_swing * q * fw;
                let out = if arm.side == Side::L { 1.0 } else { -1.0 };
                let hang = normalise(add(scale(up, -(9.0f32.to_radians().cos())), scale(left, out * 9.0f32.to_radians().sin())));
                let upper_dir = rotated(hang, swing_axis, theta);
                let bend = 8.0 + g.elbow * (0.35 + 0.65 * q) + theta.max(0.0) * 0.5;
                let fore_dir = rotated(upper_dir, swing_axis, bend);
                let hand_dir = rotated(fore_dir, swing_axis, 0.25 * bend);
                s.aim(arm.upper, upper_dir);
                s.aim(arm.lower, fore_dir);
                s.aim(arm.end, hand_dir);
            }
        } else {
            // Animals: the spine undulates, the head nods, the tail swings.
            for (i, &b) in torso.iter().enumerate() {
                s.rotate(b, up, 4.0 * q * (TAU * (phase - 0.08 * i as f32)).sin());
            }
            if let Some(h) = body.head {
                s.tilt_toward(h, f, 4.0 * q * (TAU * 2.0 * phase).cos());
            }
            for (i, &b) in body.tail.iter().enumerate() {
                s.rotate(b, up, 10.0 * (0.3 + 0.7 * q) * (TAU * (phase - 0.12 * i as f32)).sin());
            }
        }
        s
    };
    let mut s = build(0.0);
    // The longest reach any leg is asked for: lower the pelvis until none is stretched.
    let mut need = 0.0f32;
    for (li, leg) in body.legs.iter().enumerate() {
        let fo = &feet[li];
        let target = to_model(fo.0[0], fo.0[1], fo.0[2], c.ankle_rest[li][1]);
        let root_j = s.head(leg.upper);
        let len = Body::limb_length(rig, leg) * 0.985;
        let d = sub(target, root_j);
        let along = dot(d, up);
        let flat = (dot(d, d) - along * along).max(0.0).sqrt();
        let max_h = (len * len - flat * flat).max(0.0).sqrt();
        // The hip is `-along` above the ankle; it may be at most max_h.
        let over = -along - max_h;
        if over > 0.0 {
            need = need.max(over * k);
        }
    }
    if need > 1e-4 {
        s = build(need);
    }
    // Legs: ankles on their planted (or swinging) places.
    let mut missed = 0;
    for (li, leg) in body.legs.iter().enumerate() {
        let fo = &feet[li];
        let target = to_model(fo.0[0], fo.0[1], fo.0[2], c.ankle_rest[li][1]);
        if !s.limb_to(leg, target, c.bend[li]).on_target {
            missed += 1;
        }
        orient_foot(&mut s, c, leg, fo, heading, a);
    }
    Ok((s.poses(), missed))
}

fn mean_ground_height(feet: &[(V3, f32, bool, f32)]) -> f32 {
    // Heights of planted feet (swinging ones follow their ground).
    let down: Vec<f32> = feet.iter().filter(|f| f.2).map(|f| f.0[1]).collect();
    if down.is_empty() { feet.iter().map(|f| f.0[1]).sum::<f32>() / feet.len().max(1) as f32 } else { down.iter().sum::<f32>() / down.len() as f32 }
}

/// A foot flat on the ground facing where it was put down, with the toe
/// dipping as it leaves and the heel reaching as it lands.
fn orient_foot(s: &mut Solver, c: &Ctx, leg: &Limb, fo: &(V3, f32, bool, f32), heading: f32, _a: f32) {
    let body = c.body;
    let up = body.up;
    let left = body.left();
    let delta = wrap(fo.1 - heading).to_degrees();
    let u = fo.3;
    let pitch = if fo.2 { 0.0 } else { 22.0 * (1.0 - 3.0 * u).max(0.0) - 14.0 * ((u - 0.7) / 0.3).max(0.0) };
    let rest = s.rest_rot(leg.end);
    let q1 = q_axis_angle(left, pitch.to_radians());
    let q2 = q_axis_angle(up, delta.to_radians());
    s.set_world_rot(leg.end, q_mul(q2, q_mul(q1, rest)));
}
