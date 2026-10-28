// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The physics world behind Studio's 3D: rigid bodies with real collision
//! shapes, joints, sleeping and impacts, deterministic so the same scene
//! always plays the same way.
//!
//! - Bodies are dynamic (pushed by everything), static (scenery) or
//!   kinematic (moved by their animation; they push but are not pushed).
//!   Each has one or more colliders ([`shape::Shape`]); mass and inertia
//!   come from the colliders' volumes and their materials' densities.
//! - Each step finds touching pairs (boxes swept by the speed, sorted along
//!   one axis), builds contact manifolds with speculative points (so a fast
//!   ball stops at a thin wall instead of passing it), then runs sub-steps:
//!   gravity, joints and contacts as soft constraints with warm-started
//!   impulses, positions, and a relax pass without the push-apart bias;
//!   restitution last. Resting things fall asleep in islands and wake when
//!   touched.
//! - Units are SI (metres, kilograms, seconds) and directions are whatever
//!   the caller uses; gravity says which way is down.

pub mod collide;
pub mod decompose;
pub mod hull;
pub mod joint;
pub mod material;
pub mod math;
pub mod ragdoll;
pub mod secondary;
pub mod shape;

use collide::Manifold;
use joint::Joint;
use math::*;
use shape::Shape;
use std::collections::HashMap;

/// How a body moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dynamic,
    Static,
    /// Moved by its animation (a character, a door on keys): pushes, is
    /// never pushed.
    Kinematic,
}

/// A shape on a body.
#[derive(Clone, Debug)]
pub struct Collider {
    pub shape: Shape,
    /// In the body's frame.
    pub local: Pose,
    pub density: f32,
    pub friction: f32,
    pub bounce: f32,
    pub rolling: f32,
    /// Collides with colliders whose `group` shares a bit with `mask`.
    pub group: u32,
    pub mask: u32,
}

impl Collider {
    pub fn new(shape: Shape, local: Pose, stuff: &material::Stuff) -> Collider {
        Collider { shape, local, density: stuff.density, friction: stuff.friction, bounce: stuff.bounce, rolling: stuff.rolling, group: 1, mask: u32::MAX }
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub kind: Kind,
    /// The centre of mass and the body's axes.
    pub pose: Pose,
    pub v: V3,
    /// Radians per second about world axes.
    pub w: V3,
    pub inv_mass: f32,
    pub mass: f32,
    /// Inverse inertia in the body's frame.
    pub inv_inertia: M3,
    pub colliders: Vec<Collider>,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
    pub awake: bool,
    pub can_sleep: bool,
    /// Held where it is until this time (seconds): a stack waiting to be
    /// hit, a piece waiting for its shatter.
    pub hold_until: f32,
    /// A kinematic body's goal for the end of the step.
    pub target: Option<Pose>,
    /// Anything the caller wants to find the body by.
    pub user: u64,
    /// The model's origin in the body's frame (the body is at its centre
    /// of mass): model pose = pose ∘ inverse(offset).
    pub offset: Pose,
    pub(crate) inv_iw: M3,
    sleep_time: f32,
    start: Pose,
}

impl Body {
    /// A body from colliders given in the model's frame, the model at
    /// `model` in the world. Mass, centre and inertia come from the
    /// colliders and their densities.
    pub fn new(kind: Kind, colliders: Vec<Collider>, model: Pose) -> Body {
        let mut mass = 0.0f32;
        let mut first = [0.0f32; 3];
        let props: Vec<_> = colliders.iter().map(|c| c.shape.mass(c.density)).collect();
        for (c, p) in colliders.iter().zip(&props) {
            mass += p.mass;
            first = mad(first, c.local.apply(p.com), p.mass);
        }
        let com = if mass > 1e-12 { scale(first, 1.0 / mass) } else { colliders.first().map_or([0.0; 3], |c| c.local.p) };
        let mut inertia = M_ZERO;
        for (c, p) in colliders.iter().zip(&props) {
            let rot = m_rotate(c.local.q, &p.inertia);
            let d = sub(c.local.apply(p.com), com);
            let d2 = dot(d, d);
            let shift: M3 = std::array::from_fn(|r| std::array::from_fn(|k| p.mass * (if r == k { d2 } else { 0.0 } - d[r] * d[k])));
            inertia = m_add(&inertia, &m_add(&rot, &shift));
        }
        let colliders: Vec<Collider> = colliders.into_iter().map(|mut c| {
            c.local.p = sub(c.local.p, com);
            c
        }).collect();
        let pose = model.then(&Pose::at(com));
        let dynamic = kind == Kind::Dynamic && mass > 1e-12;
        let mut b = Body {
            kind: if kind == Kind::Dynamic && !dynamic { Kind::Static } else { kind },
            pose,
            v: [0.0; 3],
            w: [0.0; 3],
            inv_mass: if dynamic { 1.0 / mass } else { 0.0 },
            mass,
            inv_inertia: if mass > 1e-12 { m_inverse(&inertia) } else { M_ZERO },
            colliders,
            linear_damping: 0.0,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            awake: true,
            can_sleep: true,
            hold_until: 0.0,
            target: None,
            user: 0,
            offset: Pose::at(com),
            inv_iw: M_ZERO,
            sleep_time: 0.0,
            start: pose,
        };
        b.update_inertia();
        b
    }

    pub fn is_dynamic(&self) -> bool {
        self.kind == Kind::Dynamic
    }

    /// Turns a kinematic body loose (a character hit hard), or holds a
    /// dynamic one on its animation.
    pub fn set_kind(&mut self, kind: Kind) {
        let kind = if kind == Kind::Dynamic && self.mass <= 1e-12 { Kind::Static } else { kind };
        self.kind = kind;
        self.inv_mass = if kind == Kind::Dynamic { 1.0 / self.mass } else { 0.0 };
        if kind != Kind::Kinematic {
            self.target = None;
        }
        self.update_inertia();
        self.wake();
    }

    /// Where the model is: its origin and turn.
    pub fn model_pose(&self) -> Pose {
        self.pose.then(&self.offset.inverse())
    }

    /// Moves the body so the model is at `model`.
    pub fn set_model_pose(&mut self, model: Pose) {
        self.pose = model.then(&self.offset);
        self.update_inertia();
    }

    fn update_inertia(&mut self) {
        self.inv_iw = if self.is_dynamic() { m_rotate(self.pose.q, &self.inv_inertia) } else { M_ZERO };
    }

    pub fn velocity_at(&self, p: V3) -> V3 {
        add(self.v, cross(self.w, sub(p, self.pose.p)))
    }

    /// A sudden push (N·s) at a world point.
    pub fn apply_impulse(&mut self, j: V3, at: V3) {
        if !self.is_dynamic() {
            return;
        }
        self.v = mad(self.v, j, self.inv_mass);
        self.w = add(self.w, m_vec(&self.inv_iw, cross(sub(at, self.pose.p), j)));
        self.wake();
    }

    pub fn wake(&mut self) {
        self.awake = true;
        self.sleep_time = 0.0;
    }

    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for c in &self.colliders {
            b = b.union(&c.shape.bounds(&self.pose.then(&c.local)));
        }
        b
    }

    /// How far any of it reaches from its centre of mass.
    pub fn reach(&self) -> f32 {
        self.colliders.iter().map(|c| length(c.local.p) + c.shape.reach()).fold(0.0, f32::max)
    }

    fn held(&self, time: f32) -> bool {
        time < self.hold_until
    }
}

/// A soft constraint's response for a stiffness (Hz) and damping ratio
/// at a sub-step.
#[derive(Clone, Copy, Debug)]
pub struct Soft {
    pub bias_rate: f32,
    pub mass_scale: f32,
    pub impulse_scale: f32,
}

impl Soft {
    pub fn new(hertz: f32, zeta: f32, h: f32) -> Soft {
        if hertz <= 0.0 {
            return Soft { bias_rate: 0.0, mass_scale: 1.0, impulse_scale: 0.0 };
        }
        let omega = 2.0 * std::f32::consts::PI * hertz;
        let a1 = 2.0 * zeta + h * omega;
        let a2 = h * omega * a1;
        let a3 = 1.0 / (1.0 + a2);
        Soft { bias_rate: omega / a1, mass_scale: a2 * a3, impulse_scale: a3 }
    }
}

/// Tuning of the solver.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Seconds per step and sub-steps per step.
    pub dt: f32,
    pub substeps: usize,
    pub contact_hertz: f32,
    pub contact_damping: f32,
    pub joint_hertz: f32,
    pub joint_damping: f32,
    /// The fastest overlap is pushed apart (m/s).
    pub push_out: f32,
    /// Overlap allowed to stay (m).
    pub slop: f32,
    /// Below this approach speed (m/s) things land without bouncing.
    pub bounce_threshold: f32,
    /// A body slower than this (m/s at its furthest point) for
    /// `sleep_time` seconds sleeps.
    pub sleep_speed: f32,
    pub sleep_time: f32,
    pub max_speed: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { dt: 1.0 / 120.0, substeps: 4, contact_hertz: 30.0, contact_damping: 10.0, joint_hertz: 60.0, joint_damping: 5.0, push_out: 3.0, slop: 0.003, bounce_threshold: 0.8, sleep_speed: 0.04, sleep_time: 0.5, max_speed: 300.0 }
    }
}

/// Something hit something hard enough to matter (for breaking glass,
/// a character going limp, a sound).
#[derive(Clone, Copy, Debug)]
pub struct Impact {
    pub a: usize,
    pub b: usize,
    pub at: V3,
    /// From a to b.
    pub normal: V3,
    /// N·s.
    pub impulse: f32,
    /// How fast they met (m/s).
    pub speed: f32,
    pub time: f32,
}

struct CPoint {
    ra: V3,
    rb: V3,
    /// The point in a's frame, for matching next step.
    local: V3,
    adj_sep: f32,
    nmass: f32,
    tmass: [f32; 2],
    jn: f32,
    jt: [f32; 2],
    max_jn: f32,
    vn0: f32,
}

struct CManifold {
    a: usize,
    b: usize,
    key: (u32, u32, u32, u32, u8),
    n: V3,
    t: [V3; 2],
    friction: f32,
    bounce: f32,
    rolling: f32,
    roll_mass: f32,
    jroll: V3,
    soft: Soft,
    points: Vec<CPoint>,
}

/// Impulses kept from the last step, by pair and point.
struct Kept {
    n: V3,
    points: Vec<(V3, f32, [f32; 2])>,
    jroll: V3,
}

#[derive(Default)]
pub struct World {
    pub bodies: Vec<Body>,
    pub joints: Vec<Joint>,
    pub gravity: V3,
    pub time: f32,
    pub settings: Settings,
    /// Impacts of the last step.
    pub impacts: Vec<Impact>,
    kept: HashMap<(u32, u32, u32, u32, u8), Kept>,
    /// Contact pairs touching after the last step (body indices).
    pub touching: Vec<(usize, usize)>,
    /// Body pairs that never collide (parts of one ragdoll that overlap).
    pub ignore: std::collections::HashSet<(usize, usize)>,
}

impl World {
    pub fn new(gravity: V3) -> World {
        World { gravity, ..Default::default() }
    }

    pub fn add(&mut self, b: Body) -> usize {
        self.bodies.push(b);
        self.bodies.len() - 1
    }

    pub fn join(&mut self, j: Joint) -> usize {
        self.joints.push(j);
        self.joints.len() - 1
    }

    /// Runs `seconds` forward in whole steps.
    pub fn advance(&mut self, seconds: f32) {
        let n = (seconds / self.settings.dt).round().max(1.0) as usize;
        for _ in 0..n {
            self.step();
        }
    }

    /// The first thing a ray hits: body, distance, normal.
    pub fn raycast(&self, o: V3, d: V3, max: f32, skip: Option<usize>) -> Option<(usize, f32, V3)> {
        let d = normalise_or(d, [0.0, 1.0, 0.0]);
        let mut best: Option<(usize, f32, V3)> = None;
        for (i, b) in self.bodies.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            for c in &b.colliders {
                let pose = b.pose.then(&c.local);
                let far = best.map_or(max, |x| x.1);
                if let Some((t, n)) = c.shape.raycast(&pose, o, d, far) {
                    best = Some((i, t, n));
                }
            }
        }
        best
    }

    /// How far body `i` can move along `dir` (a unit vector) before it
    /// touches another, if it does within `reach` (see
    /// [`collide::sweep`]); negative when it is in something now.
    pub fn sweep(&self, i: usize, dir: V3, reach: f32) -> Option<f32> {
        let bi = &self.bodies[i];
        let b = bi.bounds();
        let swept = b.union(&Aabb { lo: mad(b.lo, dir, reach), hi: mad(b.hi, dir, reach) }).fattened(1e-3);
        let mut best: Option<f32> = None;
        for (j, bj) in self.bodies.iter().enumerate() {
            if j == i || !swept.overlaps(&bj.bounds()) {
                continue;
            }
            for ca in &bi.colliders {
                for cb in &bj.colliders {
                    if ca.group & cb.mask == 0 || cb.group & ca.mask == 0 {
                        continue;
                    }
                    if let Some(t) = collide::sweep(&ca.shape, &bi.pose.then(&ca.local), &cb.shape, &bj.pose.then(&cb.local), dir, best.unwrap_or(reach))
                        && best.is_none_or(|b| t < b)
                    {
                        best = Some(t);
                    }
                }
            }
        }
        best
    }

    /// How deep body `i` is in anything else now (0 when only touching);
    /// it stops looking once it is deeper than `enough`.
    pub fn overlap(&self, i: usize, enough: f32) -> f32 {
        let bi = &self.bodies[i];
        let b = bi.bounds().fattened(1e-3);
        let mut most = 0.0f32;
        for (j, bj) in self.bodies.iter().enumerate() {
            if j == i || !b.overlaps(&bj.bounds()) {
                continue;
            }
            for ca in &bi.colliders {
                for cb in &bj.colliders {
                    if ca.group & cb.mask == 0 || cb.group & ca.mask == 0 {
                        continue;
                    }
                    most = most.max(collide::overlap(&ca.shape, &bi.pose.then(&ca.local), &cb.shape, &bj.pose.then(&cb.local), enough));
                    if most > enough {
                        return most;
                    }
                }
            }
        }
        most
    }

    /// Every manifold between body `i` and anything else now (normals
    /// from `i`), with the other body.
    pub fn contacts_of(&self, i: usize, margin: f32) -> Vec<(usize, Manifold)> {
        let mut out = Vec::new();
        let bi = &self.bodies[i];
        let bounds = bi.bounds().fattened(margin);
        for (j, bj) in self.bodies.iter().enumerate() {
            if j == i || !bounds.overlaps(&bj.bounds().fattened(margin)) {
                continue;
            }
            for ca in &bi.colliders {
                for cb in &bj.colliders {
                    if ca.group & cb.mask == 0 || cb.group & ca.mask == 0 {
                        continue;
                    }
                    let mut ms = Vec::new();
                    collide::collide(&ca.shape, &bi.pose.then(&ca.local), &cb.shape, &bj.pose.then(&cb.local), margin, &mut ms);
                    out.extend(ms.into_iter().map(|m| (j, m)));
                }
            }
        }
        out
    }

    fn jointed(&self) -> std::collections::HashSet<(usize, usize)> {
        let mut set: std::collections::HashSet<(usize, usize)> = self.joints.iter().filter(|j| !j.collide && !j.broken).map(|j| (j.a.min(j.b), j.a.max(j.b))).collect();
        set.extend(self.ignore.iter().map(|&(a, b)| (a.min(b), a.max(b))));
        set
    }

    /// One step of `settings.dt`.
    pub fn step(&mut self) {
        let s = self.settings;
        let dt = s.dt;
        let n_sub = s.substeps.max(1);
        let h = dt / n_sub as f32;
        let inv_h = 1.0 / h;
        let now = self.time;
        self.impacts.clear();
        // Kinematic bodies head for their targets.
        for b in &mut self.bodies {
            if b.kind == Kind::Kinematic {
                if let Some(t) = b.target {
                    b.v = scale(sub(t.p, b.pose.p), 1.0 / dt);
                    b.w = scale(q_log(q_mul(t.q, q_conj(b.pose.q))), 1.0 / dt);
                } else {
                    b.v = [0.0; 3];
                    b.w = [0.0; 3];
                }
            }
            b.update_inertia();
            b.start = b.pose;
        }
        let moving = |b: &Body| match b.kind {
            Kind::Dynamic => b.awake && !b.held(now),
            Kind::Kinematic => len2(b.v) + len2(b.w) > 0.0,
            Kind::Static => false,
        };
        // Broad phase: every collider's swept box, sorted along x.
        struct Entry {
            body: usize,
            col: usize,
            bounds: Aabb,
        }
        let mut entries = Vec::new();
        for (bi, b) in self.bodies.iter().enumerate() {
            let sweep = s.slop * 4.0 + (length(b.v) + length(b.w) * b.reach()) * dt;
            for (ci, c) in b.colliders.iter().enumerate() {
                entries.push(Entry { body: bi, col: ci, bounds: c.shape.bounds(&b.pose.then(&c.local)).fattened(sweep) });
            }
        }
        entries.sort_by(|x, y| x.bounds.lo[0].total_cmp(&y.bounds.lo[0]).then(x.body.cmp(&y.body)).then(x.col.cmp(&y.col)));
        let jointed = self.jointed();
        let mut pairs = Vec::new();
        for i in 0..entries.len() {
            for j in i + 1..entries.len() {
                if entries[j].bounds.lo[0] > entries[i].bounds.hi[0] {
                    break;
                }
                let (ea, eb) = (&entries[i], &entries[j]);
                if ea.body == eb.body || !ea.bounds.overlaps(&eb.bounds) {
                    continue;
                }
                let (ba, bb) = (&self.bodies[ea.body], &self.bodies[eb.body]);
                if !ba.is_dynamic() && !bb.is_dynamic() {
                    continue;
                }
                if !moving(ba) && !moving(bb) {
                    continue;
                }
                let (ca, cb) = (&ba.colliders[ea.col], &bb.colliders[eb.col]);
                if ca.group & cb.mask == 0 || cb.group & ca.mask == 0 {
                    continue;
                }
                if jointed.contains(&(ea.body.min(eb.body), ea.body.max(eb.body))) {
                    continue;
                }
                // The dynamic one first.
                if ba.is_dynamic() || !bb.is_dynamic() {
                    pairs.push((ea.body, ea.col, eb.body, eb.col));
                } else {
                    pairs.push((eb.body, eb.col, ea.body, ea.col));
                }
            }
        }
        pairs.sort_unstable();
        // Narrow phase: the shapes first.
        let mut raw: Vec<((u32, u32, u32, u32, u8), Manifold)> = Vec::new();
        let mut found = Vec::new();
        for &(a, ca, b, cb) in &pairs {
            let (ba, bb) = (&self.bodies[a], &self.bodies[b]);
            let (cola, colb) = (&ba.colliders[ca], &bb.colliders[cb]);
            let rel = length(sub(ba.v, bb.v)) + length(ba.w) * ba.reach() + length(bb.w) * bb.reach();
            let margin = s.slop * 4.0 + rel * dt;
            found.clear();
            collide::collide(&cola.shape, &ba.pose.then(&cola.local), &colb.shape, &bb.pose.then(&colb.local), margin, &mut found);
            for (mi, m) in found.drain(..).enumerate() {
                raw.push(((a as u32, ca as u32, b as u32, cb as u32, mi.min(255) as u8), m));
            }
        }
        // Waking: anything moving that touches something asleep wakes it.
        for (key, m) in &raw {
            if !m.points.iter().any(|p| p.sep < s.slop * 2.0) {
                continue;
            }
            let (a, b) = (key.0 as usize, key.2 as usize);
            let (aw, bw) = (moving(&self.bodies[a]), moving(&self.bodies[b]));
            if aw && !self.bodies[b].awake && self.bodies[b].is_dynamic() {
                self.bodies[b].wake();
            }
            if bw && !self.bodies[a].awake && self.bodies[a].is_dynamic() {
                self.bodies[a].wake();
            }
        }
        let active: Vec<bool> = self.bodies.iter().map(|b| b.is_dynamic() && b.awake && !b.held(now)).collect();
        // Held or sleeping bodies act as fixed for this step.
        let saved: Vec<(f32, M3)> = self.bodies.iter().map(|b| (b.inv_mass, b.inv_iw)).collect();
        for (i, b) in self.bodies.iter_mut().enumerate() {
            if b.is_dynamic() && !active[i] {
                b.inv_mass = 0.0;
                b.inv_iw = M_ZERO;
                b.v = [0.0; 3];
                b.w = [0.0; 3];
            }
        }
        // Then the constraints.
        let contact_soft = Soft::new(s.contact_hertz.min(0.25 / h), s.contact_damping, h);
        let static_soft = Soft::new((2.0 * s.contact_hertz).min(0.25 / h), s.contact_damping, h);
        let mut manifolds: Vec<CManifold> = Vec::new();
        for (key, m) in raw {
            let (a, ca, b, cb) = (key.0 as usize, key.1 as usize, key.2 as usize, key.3 as usize);
            if !active[a] && !active[b] && !moving(&self.bodies[a]) && !moving(&self.bodies[b]) {
                continue;
            }
            let (cola, colb) = (&self.bodies[a].colliders[ca], &self.bodies[b].colliders[cb]);
            let kept = self.kept.get(&key).filter(|k| dot(k.n, m.normal) > 0.9);
            let n = m.normal;
            let t = tangents(n);
            let (ba, bb) = (&self.bodies[a], &self.bodies[b]);
            let mut points = Vec::with_capacity(m.points.len());
            for p in &m.points {
                let ra = sub(p.at, ba.pose.p);
                let rb = sub(p.at, bb.pose.p);
                let local = ba.pose.unapply(p.at);
                let kn = eff(ba, bb, ra, rb, n);
                let (mut jn, mut jt) = (0.0, [0.0; 2]);
                if let Some(k) = kept {
                    let tol = (0.02 * ba.reach()).max(s.slop * 4.0);
                    if let Some(old) = k.points.iter().find(|o| len2(sub(o.0, local)) < tol * tol) {
                        jn = old.1;
                        jt = old.2;
                    }
                }
                let vn0 = dot(sub(bb.velocity_at(p.at), ba.velocity_at(p.at)), n);
                points.push(CPoint { ra, rb, local, adj_sep: p.sep - dot(sub(rb, ra), n), nmass: inv(kn), tmass: [inv(eff(ba, bb, ra, rb, t.0)), inv(eff(ba, bb, ra, rb, t.1))], jn, jt, max_jn: 0.0, vn0 });
            }
            let rolling = (cola.rolling * colb.rolling).sqrt().max(cola.rolling.min(colb.rolling));
            let rk = {
                let k = m_add(&ba.inv_iw, &bb.inv_iw);
                (k[0][0] + k[1][1] + k[2][2]) / 3.0
            };
            let both = ba.inv_mass > 0.0 && bb.inv_mass > 0.0;
            manifolds.push(CManifold {
                a,
                b,
                key,
                n,
                t: [t.0, t.1],
                friction: (cola.friction * colb.friction).sqrt(),
                bounce: cola.bounce.max(colb.bounce),
                rolling,
                roll_mass: inv(rk),
                jroll: kept.map_or([0.0; 3], |k| k.jroll),
                soft: if both { contact_soft } else { static_soft },
                points,
            });
        }
        let joint_soft = Soft::new(s.joint_hertz.min(0.25 / h), s.joint_damping, h);
        let live_joints: Vec<usize> = (0..self.joints.len()).filter(|&k| !self.joints[k].broken && (active[self.joints[k].a] || active[self.joints[k].b])).collect();
        for _ in 0..n_sub {
            // Gravity and damping.
            for (i, b) in self.bodies.iter_mut().enumerate() {
                if !active[i] {
                    continue;
                }
                b.v = mad(b.v, self.gravity, h * b.gravity_scale);
                b.v = scale(b.v, 1.0 / (1.0 + h * b.linear_damping));
                b.w = scale(b.w, 1.0 / (1.0 + h * b.angular_damping));
            }
            // Warm start: each sub-step begins with the impulses that held
            // things last time.
            for &k in &live_joints {
                self.joints[k].warm(&mut self.bodies);
            }
            for m in &manifolds {
                let (a, b) = (m.a, m.b);
                for p in &m.points {
                    let j = add(add(scale(m.n, p.jn), scale(m.t[0], p.jt[0])), scale(m.t[1], p.jt[1]));
                    push_pair(&mut self.bodies, a, b, p.ra, p.rb, j);
                }
                rotate_pair(&mut self.bodies, a, b, m.jroll);
            }
            for &k in &live_joints {
                self.joints[k].solve(&mut self.bodies, &joint_soft, h, true);
            }
            solve_contacts(&mut self.bodies, &mut manifolds, inv_h, s.push_out, true, h);
            // Move.
            for (i, b) in self.bodies.iter_mut().enumerate() {
                if !(active[i] || (b.kind == Kind::Kinematic && moving_kin(b))) {
                    continue;
                }
                let sp = length(b.v);
                if sp > s.max_speed {
                    b.v = scale(b.v, s.max_speed / sp);
                }
                let wl = length(b.w);
                if wl > 200.0 {
                    b.w = scale(b.w, 200.0 / wl);
                }
                b.pose.p = mad(b.pose.p, b.v, h);
                b.pose.q = q_integrate(b.pose.q, b.w, h);
                if b.is_dynamic() {
                    b.inv_iw = m_rotate(b.pose.q, &b.inv_inertia);
                }
            }
            for &k in &live_joints {
                self.joints[k].solve(&mut self.bodies, &joint_soft, h, false);
            }
            solve_contacts(&mut self.bodies, &mut manifolds, inv_h, s.push_out, false, h);
        }
        // Bounces.
        for m in &mut manifolds {
            if m.bounce <= 0.0 {
                continue;
            }
            for p in &mut m.points {
                if p.vn0 > -s.bounce_threshold || p.max_jn <= 0.0 {
                    continue;
                }
                let (ba, bb) = (&self.bodies[m.a], &self.bodies[m.b]);
                let vn = dot(sub(bb.velocity_at(add(bb.pose.p, p.rb)), ba.velocity_at(add(ba.pose.p, p.ra))), m.n);
                let old = p.jn;
                p.jn = (old - p.nmass * (vn + m.bounce * p.vn0)).max(0.0);
                let j = scale(m.n, p.jn - old);
                push_pair(&mut self.bodies, m.a, m.b, p.ra, p.rb, j);
            }
        }
        // Undo the stand-in fixing of held and sleeping bodies.
        for (i, b) in self.bodies.iter_mut().enumerate() {
            if b.is_dynamic() && !active[i] {
                b.inv_mass = saved[i].0;
                b.inv_iw = saved[i].1;
            }
        }
        // Kinematic bodies land exactly on their targets.
        for b in &mut self.bodies {
            if b.kind == Kind::Kinematic
                && let Some(t) = b.target
            {
                b.pose = t;
            }
        }
        // Breaking joints.
        for j in &mut self.joints {
            if j.break_force > 0.0 && !j.broken && j.load(h) > j.break_force {
                j.broken = true;
            }
        }
        // Keep impulses, note impacts, find what touches.
        self.kept.clear();
        self.touching.clear();
        for m in &manifolds {
            let total: f32 = m.points.iter().map(|p| p.max_jn).sum::<f32>() * n_sub as f32;
            let speed = m.points.iter().map(|p| -p.vn0).fold(0.0f32, f32::max);
            if speed > s.bounce_threshold && total > 0.0 {
                let at = scale(m.points.iter().fold([0.0; 3], |acc, p| add(acc, add(self.bodies[m.a].start.p, p.ra))), 1.0 / m.points.len() as f32);
                self.impacts.push(Impact { a: m.a, b: m.b, at, normal: m.n, impulse: total, speed, time: now + dt });
            }
            if m.points.iter().any(|p| p.jn > 0.0) {
                self.touching.push((m.a, m.b));
            }
            self.kept.insert(m.key, Kept { n: m.n, points: m.points.iter().map(|p| (p.local, p.jn, p.jt)).collect(), jroll: m.jroll });
        }
        self.time = now + dt;
        self.sleep(dt);
    }

    /// Islands of touching awake bodies; an island at rest long enough
    /// sleeps as one.
    fn sleep(&mut self, dt: f32) {
        let s = self.settings;
        let n = self.bodies.len();
        for b in &mut self.bodies {
            if !b.is_dynamic() || !b.awake {
                continue;
            }
            let speed = length(b.v) + length(b.w) * b.reach();
            if !b.can_sleep || speed > s.sleep_speed {
                b.sleep_time = 0.0;
            } else {
                b.sleep_time += dt;
            }
        }
        let mut parent: Vec<usize> = (0..n).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let link = |p: &mut Vec<usize>, a: usize, b: usize| {
            let (ra, rb) = (root(p, a), root(p, b));
            if ra != rb {
                p[ra.max(rb)] = ra.min(rb);
            }
        };
        // A kinematic body that moves keeps what it touches awake.
        let mut keep_awake = vec![false; n];
        for &(a, b) in &self.touching {
            let (ka, kb) = (self.bodies[a].kind, self.bodies[b].kind);
            if ka == Kind::Dynamic && kb == Kind::Dynamic {
                link(&mut parent, a, b);
            }
            if ka == Kind::Kinematic && len2(self.bodies[a].v) + len2(self.bodies[a].w) > 0.0 {
                keep_awake[b] = true;
            }
            if kb == Kind::Kinematic && len2(self.bodies[b].v) + len2(self.bodies[b].w) > 0.0 {
                keep_awake[a] = true;
            }
        }
        for j in &self.joints {
            if !j.broken && self.bodies[j.a].is_dynamic() && self.bodies[j.b].is_dynamic() {
                link(&mut parent, j.a, j.b);
            }
        }
        let mut island_min = vec![f32::INFINITY; n];
        for i in 0..n {
            let b = &self.bodies[i];
            if !b.is_dynamic() || !b.awake {
                continue;
            }
            let r = root(&mut parent, i);
            let t = if keep_awake[i] { 0.0 } else { b.sleep_time };
            island_min[r] = island_min[r].min(t);
        }
        for i in 0..n {
            if !self.bodies[i].is_dynamic() || !self.bodies[i].awake {
                continue;
            }
            let r = root(&mut parent, i);
            if island_min[r] >= s.sleep_time {
                let b = &mut self.bodies[i];
                b.awake = false;
                b.v = [0.0; 3];
                b.w = [0.0; 3];
            }
        }
    }
}

fn moving_kin(b: &Body) -> bool {
    len2(b.v) + len2(b.w) > 0.0
}

fn inv(k: f32) -> f32 {
    if k > 1e-12 { 1.0 / k } else { 0.0 }
}

/// The inverse effective mass of two bodies at a contact along `n`.
fn eff(a: &Body, b: &Body, ra: V3, rb: V3, n: V3) -> f32 {
    let ca = cross(ra, n);
    let cb = cross(rb, n);
    a.inv_mass + b.inv_mass + dot(ca, m_vec(&a.inv_iw, ca)) + dot(cb, m_vec(&b.inv_iw, cb))
}

/// `j` to b at `rb`, `−j` to a at `ra`.
fn push_pair(bodies: &mut [Body], a: usize, b: usize, ra: V3, rb: V3, j: V3) {
    {
        let x = &mut bodies[a];
        x.v = mad(x.v, j, -x.inv_mass);
        x.w = sub(x.w, m_vec(&x.inv_iw, cross(ra, j)));
    }
    let y = &mut bodies[b];
    y.v = mad(y.v, j, y.inv_mass);
    y.w = add(y.w, m_vec(&y.inv_iw, cross(rb, j)));
}

fn rotate_pair(bodies: &mut [Body], a: usize, b: usize, j: V3) {
    {
        let x = &mut bodies[a];
        x.w = sub(x.w, m_vec(&x.inv_iw, j));
    }
    let y = &mut bodies[b];
    y.w = add(y.w, m_vec(&y.inv_iw, j));
}

fn solve_contacts(bodies: &mut [Body], manifolds: &mut [CManifold], inv_h: f32, push_out: f32, use_bias: bool, h: f32) {
    for m in manifolds.iter_mut() {
        let (a, b) = (m.a, m.b);
        // How far each body has moved and turned since the step began.
        let (dpa, dqa) = {
            let x = &bodies[a];
            (sub(x.pose.p, x.start.p), q_mul(x.pose.q, q_conj(x.start.q)))
        };
        let (dpb, dqb) = {
            let y = &bodies[b];
            (sub(y.pose.p, y.start.p), q_mul(y.pose.q, q_conj(y.start.q)))
        };
        let mut total = 0.0;
        for p in &mut m.points {
            let pra = q_rotate(dqa, p.ra);
            let prb = q_rotate(dqb, p.rb);
            let d = add(sub(dpb, dpa), sub(prb, pra));
            let sep = dot(d, m.n) + p.adj_sep;
            let (bias, ms, is) = if sep > 0.0 {
                (sep * inv_h, 1.0, 0.0)
            } else if use_bias {
                ((m.soft.bias_rate * sep).max(-push_out), m.soft.mass_scale, m.soft.impulse_scale)
            } else {
                (0.0, 1.0, 0.0)
            };
            let (ba, bb) = (&bodies[a], &bodies[b]);
            let dv = sub(add(bb.v, cross(bb.w, p.rb)), add(ba.v, cross(ba.w, p.ra)));
            let vn = dot(dv, m.n);
            let imp = -p.nmass * ms * (vn + bias) - is * p.jn;
            let new = (p.jn + imp).max(0.0);
            let dj = new - p.jn;
            p.jn = new;
            p.max_jn = p.max_jn.max(dj);
            total += new;
            push_pair(bodies, a, b, p.ra, p.rb, scale(m.n, dj));
        }
        for p in &mut m.points {
            let (ba, bb) = (&bodies[a], &bodies[b]);
            let dv = sub(add(bb.v, cross(bb.w, p.rb)), add(ba.v, cross(ba.w, p.ra)));
            let limit = m.friction * p.jn;
            for k in 0..2 {
                let vt = dot(dv, m.t[k]);
                let old = p.jt[k];
                p.jt[k] = (old - p.tmass[k] * vt).clamp(-limit, limit);
                let dj = p.jt[k] - old;
                push_pair(bodies, a, b, p.ra, p.rb, scale(m.t[k], dj));
            }
        }
        if m.rolling > 0.0 && total > 0.0 {
            // Rolling resistance: a torque against the relative spin, up
            // to rolling × the push × a lever of the contact's size.
            let dw = sub(bodies[b].w, bodies[a].w);
            let old = m.jroll;
            let mut acc = sub(old, scale(dw, m.roll_mass));
            let cap = m.rolling * total * 0.05;
            let l = length(acc);
            if l > cap {
                acc = scale(acc, cap / l);
            }
            m.jroll = acc;
            rotate_pair(bodies, a, b, sub(acc, old));
        }
        let _ = h;
    }
}

#[cfg(test)]
mod tests;
