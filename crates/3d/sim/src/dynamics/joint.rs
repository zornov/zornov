// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Joints between two bodies: ball (with a swing cone and a twist range,
//! and an optional drive towards a pose, which is what holds a ragdoll up
//! as a character), hinge (doors, wheels, elbows; limits, a motor, a
//! spring), slider (drawers, pistons), fixed (glued) and spring (a rope or
//! a bungee with a rest length).
//!
//! Each joint has a frame on each body; the frame's x axis is the hinge
//! axis, the slide direction or the twist axis. All rows are soft
//! constraints solved in the same sub-steps as the contacts.

use super::math::*;
use super::{Body, Soft};

/// A motor: a target speed (rad/s or m/s) with the most torque or force
/// it may use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motor {
    pub speed: f32,
    pub max: f32,
}

/// A spring towards a target angle (radians) or offset (metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub target: f32,
    pub hertz: f32,
    pub damping: f32,
    pub max: f32,
}

/// A drive turning a ball joint towards a relative pose (B's frame in A's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drive {
    pub target: Quat,
    pub hertz: f32,
    pub damping: f32,
    /// The most torque it may use (N·m).
    pub max: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JointKind {
    /// `cone`: the most the x axes may swing apart (radians);
    /// `twist`: the range of turn about the x axis.
    Ball { cone: Option<f32>, twist: Option<(f32, f32)>, drive: Option<Drive> },
    Hinge { limits: Option<(f32, f32)>, motor: Option<Motor>, spring: Option<Spring> },
    Slider { limits: Option<(f32, f32)>, motor: Option<Motor>, spring: Option<Spring> },
    Fixed,
    /// Keeps the frame origins `rest` apart; `hertz` 0 is rigid (a rod);
    /// `min`/`max` make it a rope or a bungee.
    Spring { rest: f32, hertz: f32, damping: f32, min: f32, max: f32 },
}

impl JointKind {
    pub fn name(&self) -> &'static str {
        match self {
            JointKind::Ball { .. } => "ball",
            JointKind::Hinge { .. } => "hinge",
            JointKind::Slider { .. } => "slider",
            JointKind::Fixed => "fixed",
            JointKind::Spring { .. } => "spring",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Joint {
    pub a: usize,
    pub b: usize,
    /// The joint's frame on each body (in the body's frame).
    pub frame_a: Pose,
    pub frame_b: Pose,
    pub kind: JointKind,
    /// Whether the two bodies still collide with each other.
    pub collide: bool,
    /// Breaks when it has to hold more than this force (N); 0 = never.
    pub break_force: f32,
    pub broken: bool,
    acc_lin: V3,
    acc_ang: V3,
    acc_lo: f32,
    acc_hi: f32,
    acc_motor: f32,
    acc_spring: f32,
    acc_twist_lo: f32,
    acc_twist_hi: f32,
    acc_drive: V3,
}

impl Joint {
    /// A joint at a world point; `axis` (world) is the hinge, slide or
    /// twist axis. The bodies' poses now are the joint's rest.
    pub fn at(bodies: &[Body], a: usize, b: usize, point: V3, axis: V3, kind: JointKind) -> Joint {
        let axis = normalise_or(axis, [1.0, 0.0, 0.0]);
        let (t1, t2) = tangents(axis);
        let q = q_from_mat(&[[axis[0], t1[0], t2[0]], [axis[1], t1[1], t2[1]], [axis[2], t1[2], t2[2]]]);
        let world = Pose::new(point, q);
        let fa = bodies[a].pose.inverse().then(&world);
        let fb = bodies[b].pose.inverse().then(&world);
        Joint::with_frames(a, b, fa, fb, kind)
    }

    /// A joint from a point on `a` to a point on `b` (world), frames
    /// lined up with the world (a rope, a spring, a rod).
    pub fn between(bodies: &[Body], a: usize, b: usize, pa: V3, pb: V3, kind: JointKind) -> Joint {
        let fa = bodies[a].pose.inverse().then(&Pose::at(pa));
        let fb = bodies[b].pose.inverse().then(&Pose::at(pb));
        Joint::with_frames(a, b, fa, fb, kind)
    }

    pub fn with_frames(a: usize, b: usize, frame_a: Pose, frame_b: Pose, kind: JointKind) -> Joint {
        Joint { a, b, frame_a, frame_b, kind, collide: false, break_force: 0.0, broken: false, acc_lin: [0.0; 3], acc_ang: [0.0; 3], acc_lo: 0.0, acc_hi: 0.0, acc_motor: 0.0, acc_spring: 0.0, acc_twist_lo: 0.0, acc_twist_hi: 0.0, acc_drive: [0.0; 3] }
    }

    /// The joint's frames in the world now.
    pub fn world_frames(&self, bodies: &[Body]) -> (Pose, Pose) {
        (bodies[self.a].pose.then(&self.frame_a), bodies[self.b].pose.then(&self.frame_b))
    }

    /// The hinge angle (radians) or slide offset (metres) now.
    pub fn position(&self, bodies: &[Body]) -> f32 {
        let (wa, wb) = self.world_frames(bodies);
        match self.kind {
            JointKind::Slider { .. } => dot(sub(wb.p, wa.p), wa.rotate([1.0, 0.0, 0.0])),
            _ => hinge_angle(&wa, &wb),
        }
    }

    /// The force it held in the last sub-step (N), for breaking.
    pub fn load(&self, h: f32) -> f32 {
        length(self.acc_lin) / h.max(1e-9)
    }

    pub(super) fn warm(&mut self, bodies: &mut [Body]) {
        let (wa, wb) = self.world_frames(bodies);
        let ra = sub(wa.p, bodies[self.a].pose.p);
        let rb = sub(wb.p, bodies[self.b].pose.p);
        let ax = wa.rotate([1.0, 0.0, 0.0]);
        let mut lin = self.acc_lin;
        let mut ang = add(self.acc_ang, self.acc_drive);
        match self.kind {
            JointKind::Hinge { .. } => ang = mad(ang, ax, self.acc_lo - self.acc_hi + self.acc_motor + self.acc_spring),
            JointKind::Slider { .. } => lin = mad(lin, ax, self.acc_lo - self.acc_hi + self.acc_motor + self.acc_spring),
            JointKind::Ball { .. } => ang = mad(ang, ax, self.acc_twist_lo - self.acc_twist_hi),
            JointKind::Spring { .. } => {
                let u = normalise_or(sub(wb.p, wa.p), ax);
                lin = scale(u, self.acc_spring + self.acc_lo - self.acc_hi);
            }
            JointKind::Fixed => {}
        }
        apply(bodies, self.a, self.b, ra, rb, lin, ang);
    }

    pub(super) fn solve(&mut self, bodies: &mut [Body], soft: &Soft, h: f32, use_bias: bool) {
        if self.broken {
            return;
        }
        let (a, b) = (self.a, self.b);
        let (wa, wb) = self.world_frames(bodies);
        let ra = sub(wa.p, bodies[a].pose.p);
        let rb = sub(wb.p, bodies[b].pose.p);
        let ax = wa.rotate([1.0, 0.0, 0.0]);
        let (ms, is, rate) = if use_bias { (soft.mass_scale, soft.impulse_scale, soft.bias_rate) } else { (1.0, 0.0, 0.0) };
        let inv_h = 1.0 / h;
        let kind = self.kind.clone();
        // Angular parts first, then the point.
        match kind {
            JointKind::Fixed => {
                let e = q_log(q_mul(wb.q, q_conj(wa.q)));
                self.acc_ang = add(self.acc_ang, angular_rows(bodies, a, b, e, None, rate, ms, is, self.acc_ang));
            }
            JointKind::Slider { limits, motor, spring } => {
                let e = q_log(q_mul(wb.q, q_conj(wa.q)));
                self.acc_ang = add(self.acc_ang, angular_rows(bodies, a, b, e, None, rate, ms, is, self.acc_ang));
                let d = sub(wb.p, wa.p);
                let x = dot(d, ax);
                // Lever arm of a's side: to b's anchor.
                let ra2 = add(ra, d);
                let k = lin_mass(bodies, a, b, ra2, rb, ax);
                if let Some(m) = motor {
                    let cdot = lin_rel(bodies, a, b, ra2, rb, ax) - m.speed;
                    let old = self.acc_motor;
                    self.acc_motor = (old - k * cdot).clamp(-m.max * h, m.max * h);
                    apply(bodies, a, b, ra2, rb, scale(ax, self.acc_motor - old), [0.0; 3]);
                }
                if let Some(sp) = spring {
                    let sf = Soft::new(sp.hertz, sp.damping, h);
                    let cdot = lin_rel(bodies, a, b, ra2, rb, ax);
                    let old = self.acc_spring;
                    let lim = if sp.max > 0.0 { sp.max * h } else { f32::INFINITY };
                    self.acc_spring = (old - sf.mass_scale * k * (cdot + sf.bias_rate * (x - sp.target)) - sf.impulse_scale * old).clamp(-lim, lim);
                    apply(bodies, a, b, ra2, rb, scale(ax, self.acc_spring - old), [0.0; 3]);
                }
                if let Some((lo, hi)) = limits {
                    for (lower, c) in [(true, x - lo), (false, hi - x)] {
                        let sign = if lower { 1.0 } else { -1.0 };
                        let (bias, m2, i2) = limit_bias(c, inv_h, use_bias, soft);
                        let cdot = sign * lin_rel(bodies, a, b, ra2, rb, ax);
                        let acc = if lower { &mut self.acc_lo } else { &mut self.acc_hi };
                        let old = *acc;
                        *acc = (old - m2 * k * (cdot + bias) - i2 * old).max(0.0);
                        let dj = *acc - old;
                        apply(bodies, a, b, ra2, rb, scale(ax, sign * dj), [0.0; 3]);
                    }
                }
                // Off the slide line.
                let (p1, p2) = tangents(ax);
                for u in [p1, p2] {
                    let c = dot(d, u);
                    let kk = lin_mass(bodies, a, b, ra2, rb, u);
                    let cdot = lin_rel(bodies, a, b, ra2, rb, u);
                    let j = -ms * kk * (cdot + rate * c) - is * dot(self.acc_lin, u);
                    self.acc_lin = mad(self.acc_lin, u, j);
                    apply(bodies, a, b, ra2, rb, scale(u, j), [0.0; 3]);
                }
                return;
            }
            JointKind::Hinge { limits, motor, spring } => {
                // Keep the axes lined up.
                let axb = wb.rotate([1.0, 0.0, 0.0]);
                let e = cross(ax, axb);
                self.acc_ang = add(self.acc_ang, angular_rows(bodies, a, b, e, Some(ax), rate, ms, is, self.acc_ang));
                let ang = hinge_angle(&wa, &wb);
                let k = ang_mass(bodies, a, b, ax);
                if let Some(m) = motor {
                    let cdot = ang_rel(bodies, a, b, ax) - m.speed;
                    let old = self.acc_motor;
                    self.acc_motor = (old - k * cdot).clamp(-m.max * h, m.max * h);
                    apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], scale(ax, self.acc_motor - old));
                }
                if let Some(sp) = spring {
                    let sf = Soft::new(sp.hertz, sp.damping, h);
                    let cdot = ang_rel(bodies, a, b, ax);
                    let old = self.acc_spring;
                    let lim = if sp.max > 0.0 { sp.max * h } else { f32::INFINITY };
                    let err = wrap(ang - sp.target);
                    self.acc_spring = (old - sf.mass_scale * k * (cdot + sf.bias_rate * err) - sf.impulse_scale * old).clamp(-lim, lim);
                    apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], scale(ax, self.acc_spring - old));
                }
                if let Some((lo, hi)) = limits {
                    for (lower, c) in [(true, ang - lo), (false, hi - ang)] {
                        let sign = if lower { 1.0 } else { -1.0 };
                        let (bias, m2, i2) = limit_bias(c, inv_h, use_bias, soft);
                        let cdot = sign * ang_rel(bodies, a, b, ax);
                        let acc = if lower { &mut self.acc_lo } else { &mut self.acc_hi };
                        let old = *acc;
                        *acc = (old - m2 * k * (cdot + bias) - i2 * old).max(0.0);
                        let dj = *acc - old;
                        apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], scale(ax, sign * dj));
                    }
                }
            }
            JointKind::Ball { cone, twist, drive } => {
                if let Some(dr) = drive {
                    let sf = Soft::new(dr.hertz, dr.damping, h);
                    let goal = q_mul(wa.q, dr.target);
                    let e = q_log(q_mul(wb.q, q_conj(goal)));
                    let dw = sub(bodies[b].w, bodies[a].w);
                    let kinv = m_inverse(&m_add(&bodies[a].inv_iw, &bodies[b].inv_iw));
                    let want = m_vec(&kinv, add(dw, scale(e, sf.bias_rate)));
                    let old = self.acc_drive;
                    let mut acc = sub(scale(old, 1.0 - sf.impulse_scale), scale(want, sf.mass_scale));
                    let cap = dr.max * h;
                    let l = length(acc);
                    if dr.max > 0.0 && l > cap {
                        acc = scale(acc, cap / l);
                    }
                    self.acc_drive = acc;
                    apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], sub(acc, old));
                }
                if let Some(cone) = cone {
                    let axb = wb.rotate([1.0, 0.0, 0.0]);
                    let c = dot(ax, axb).clamp(-1.0, 1.0);
                    let swing = c.acos();
                    let n = cross(ax, axb);
                    if swing > cone - 0.1 && length(n) > 1e-6 {
                        let n = normalise(n);
                        let k = ang_mass(bodies, a, b, n);
                        let (bias, m2, i2) = limit_bias(cone - swing, inv_h, use_bias, soft);
                        let cdot = -ang_rel(bodies, a, b, n);
                        let old = self.acc_lo;
                        self.acc_lo = (old - m2 * k * (cdot + bias) - i2 * old).max(0.0);
                        apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], scale(n, -(self.acc_lo - old)));
                    } else {
                        self.acc_lo = 0.0;
                    }
                }
                if let Some((lo, hi)) = twist {
                    let rel = q_mul(q_conj(wa.q), wb.q);
                    let tw = wrap(2.0 * rel[0].atan2(rel[3]));
                    let k = ang_mass(bodies, a, b, ax);
                    for (lower, c) in [(true, tw - lo), (false, hi - tw)] {
                        let sign = if lower { 1.0 } else { -1.0 };
                        let (bias, m2, i2) = limit_bias(c, inv_h, use_bias, soft);
                        let cdot = sign * ang_rel(bodies, a, b, ax);
                        let acc = if lower { &mut self.acc_twist_lo } else { &mut self.acc_twist_hi };
                        let old = *acc;
                        *acc = (old - m2 * k * (cdot + bias) - i2 * old).max(0.0);
                        let dj = *acc - old;
                        apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], scale(ax, sign * dj));
                    }
                }
            }
            JointKind::Spring { rest, hertz, damping, min, max } => {
                let d = sub(wb.p, wa.p);
                let len = length(d);
                let u = normalise_or(d, ax);
                let k = lin_mass(bodies, a, b, ra, rb, u);
                if hertz > 0.0 {
                    let sf = Soft::new(hertz, damping, h);
                    let cdot = lin_rel(bodies, a, b, ra, rb, u);
                    let old = self.acc_spring;
                    self.acc_spring = old - sf.mass_scale * k * (cdot + sf.bias_rate * (len - rest)) - sf.impulse_scale * old;
                    apply(bodies, a, b, ra, rb, scale(u, self.acc_spring - old), [0.0; 3]);
                } else {
                    let cdot = lin_rel(bodies, a, b, ra, rb, u);
                    let j = -ms * k * (cdot + rate * (len - rest)) - is * self.acc_spring;
                    self.acc_spring += j;
                    apply(bodies, a, b, ra, rb, scale(u, j), [0.0; 3]);
                }
                for (lower, c) in [(true, len - min), (false, max - len)] {
                    if (lower && min <= 0.0) || (!lower && !max.is_finite()) {
                        continue;
                    }
                    let sign = if lower { 1.0 } else { -1.0 };
                    let (bias, m2, i2) = limit_bias(c, inv_h, use_bias, soft);
                    let cdot = sign * lin_rel(bodies, a, b, ra, rb, u);
                    let acc = if lower { &mut self.acc_lo } else { &mut self.acc_hi };
                    let old = *acc;
                    *acc = (old - m2 * k * (cdot + bias) - i2 * old).max(0.0);
                    let dj = *acc - old;
                    apply(bodies, a, b, ra, rb, scale(u, sign * dj), [0.0; 3]);
                }
                return;
            }
        }
        // The two anchors held together.
        let c = sub(wb.p, wa.p);
        let dv = sub(add(bodies[b].v, cross(bodies[b].w, rb)), add(bodies[a].v, cross(bodies[a].w, ra)));
        let kk = point_mass(bodies, a, b, ra, rb);
        let want = m_vec(&m_inverse(&kk), add(dv, scale(c, rate)));
        let j = sub(scale(want, -ms), scale(self.acc_lin, is));
        self.acc_lin = add(self.acc_lin, j);
        apply(bodies, a, b, ra, rb, j, [0.0; 3]);
    }
}

fn wrap(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut a = a % tau;
    if a > std::f32::consts::PI {
        a -= tau;
    } else if a < -std::f32::consts::PI {
        a += tau;
    }
    a
}

/// The turn of b's frame about a's x axis.
fn hinge_angle(wa: &Pose, wb: &Pose) -> f32 {
    let ax = wa.rotate([1.0, 0.0, 0.0]);
    let ya = wa.rotate([0.0, 1.0, 0.0]);
    let yb = wb.rotate([0.0, 1.0, 0.0]);
    // b's y flattened onto a's hinge plane.
    let yb = sub(yb, scale(ax, dot(yb, ax)));
    dot(cross(ya, yb), ax).atan2(dot(ya, yb))
}

/// The bias and softness of a one-sided limit `c >= 0`: approaching
/// freely while apart (speculative), soft push back once past it.
fn limit_bias(c: f32, inv_h: f32, use_bias: bool, soft: &Soft) -> (f32, f32, f32) {
    if c > 0.0 {
        (c * inv_h, 1.0, 0.0)
    } else if use_bias {
        (soft.bias_rate * c, soft.mass_scale, soft.impulse_scale)
    } else {
        (0.0, 1.0, 0.0)
    }
}

fn point_mass(bodies: &[Body], a: usize, b: usize, ra: V3, rb: V3) -> M3 {
    let (ba, bb) = (&bodies[a], &bodies[b]);
    let m = ba.inv_mass + bb.inv_mass;
    let sa = m_skew(ra);
    let sb = m_skew(rb);
    let ka = m_mul(&m_mul(&sa, &ba.inv_iw), &m_t(&sa));
    let kb = m_mul(&m_mul(&sb, &bb.inv_iw), &m_t(&sb));
    m_add(&m_add(&m_diag([m; 3]), &ka), &kb)
}

fn lin_mass(bodies: &[Body], a: usize, b: usize, ra: V3, rb: V3, u: V3) -> f32 {
    let (ba, bb) = (&bodies[a], &bodies[b]);
    let ca = cross(ra, u);
    let cb = cross(rb, u);
    let k = ba.inv_mass + bb.inv_mass + dot(ca, m_vec(&ba.inv_iw, ca)) + dot(cb, m_vec(&bb.inv_iw, cb));
    if k > 1e-12 { 1.0 / k } else { 0.0 }
}

fn lin_rel(bodies: &[Body], a: usize, b: usize, ra: V3, rb: V3, u: V3) -> f32 {
    let dv = sub(add(bodies[b].v, cross(bodies[b].w, rb)), add(bodies[a].v, cross(bodies[a].w, ra)));
    dot(dv, u)
}

fn ang_mass(bodies: &[Body], a: usize, b: usize, u: V3) -> f32 {
    let k = dot(u, m_vec(&bodies[a].inv_iw, u)) + dot(u, m_vec(&bodies[b].inv_iw, u));
    if k > 1e-12 { 1.0 / k } else { 0.0 }
}

fn ang_rel(bodies: &[Body], a: usize, b: usize, u: V3) -> f32 {
    dot(sub(bodies[b].w, bodies[a].w), u)
}

/// Holds the relative turn: `e` is the error (rotation vector, world);
/// with `free` set, turning about that axis is left alone (a hinge).
#[allow(clippy::too_many_arguments)]
fn angular_rows(bodies: &mut [Body], a: usize, b: usize, e: V3, free: Option<V3>, rate: f32, ms: f32, is: f32, acc: V3) -> V3 {
    let mut k = m_add(&bodies[a].inv_iw, &bodies[b].inv_iw);
    let dw = sub(bodies[b].w, bodies[a].w);
    let mut target = add(dw, scale(e, rate));
    let mut old = acc;
    if let Some(ax) = free {
        // Only the two directions across the axis.
        let (p1, p2) = tangents(ax);
        let k11 = dot(p1, m_vec(&k, p1));
        let k12 = dot(p1, m_vec(&k, p2));
        let k22 = dot(p2, m_vec(&k, p2));
        let det = k11 * k22 - k12 * k12;
        if det.abs() < 1e-20 {
            return [0.0; 3];
        }
        let (t1, t2) = (dot(target, p1), dot(target, p2));
        let (o1, o2) = (dot(old, p1), dot(old, p2));
        let x1 = (k22 * t1 - k12 * t2) / det;
        let x2 = (k11 * t2 - k12 * t1) / det;
        let j = add(scale(p1, -ms * x1 - is * o1), scale(p2, -ms * x2 - is * o2));
        apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], j);
        return j;
    }
    if k.iter().flatten().all(|v| v.abs() < 1e-20) {
        k = M_ID;
        target = [0.0; 3];
        old = [0.0; 3];
    }
    let j = sub(scale(m_vec(&m_inverse(&k), target), -ms), scale(old, is));
    apply(bodies, a, b, [0.0; 3], [0.0; 3], [0.0; 3], j);
    j
}

/// Applies a linear impulse `lin` at the anchors and an angular impulse
/// `ang`: + to b, − to a.
fn apply(bodies: &mut [Body], a: usize, b: usize, ra: V3, rb: V3, lin: V3, ang: V3) {
    {
        let ba = &mut bodies[a];
        if ba.is_dynamic() {
            ba.v = mad(ba.v, lin, -ba.inv_mass);
            ba.w = sub(ba.w, m_vec(&ba.inv_iw, add(cross(ra, lin), ang)));
        }
    }
    let bb = &mut bodies[b];
    if bb.is_dynamic() {
        bb.v = mad(bb.v, lin, bb.inv_mass);
        bb.w = add(bb.w, m_vec(&bb.inv_iw, add(cross(rb, lin), ang)));
    }
}
