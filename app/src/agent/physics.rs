// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `physics`: layers that fall, bounce, tumble and stack, simulated as
//! rigid bodies and baked into ordinary position and rotation keys, so the result plays,
//! renders and edits like any other animation.
//!
//! `{"op": "physics", "clips": [3, 4, 5], "at": 1.0, "duration": 3}`
//! drops the clips onto the bottom of the frame. Settings: `gravity`
//! (px/s², default a little over two frame heights a second squared),
//! `bounce` 0..1, `friction` 0..1, `floor` (y in canvas pixels, default
//! the bottom edge; false for none), `walls` (true: the frame's sides
//! hold them in), `ceiling`, `static` (clips that collide but don't
//! move: a shelf, a logo to land on), `push` ({clip: [vx, vy]} px/s,
//! or one [vx, vy] for all), `spin` (deg/s, or {clip: deg/s}), `shape`
//! ("box" or "circle", or {clip: shape}).
//!
//! The solver is written from the textbook method (sequential impulses
//! with accumulated clamping, separating-axis box contacts clipped to
//! the reference face), not from any engine's code.

use super::{Session, f, micros, secs};
use crate::doc::Doc;
use crate::editor::xform;
use serde_json::{Value, json};
use studio_model as m;

type V = [f64; 2];

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: V, s: f64) -> V {
    [a[0] * s, a[1] * s]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: V, b: V) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
/// ω × r for a scalar ω.
fn cross_sv(w: f64, r: V) -> V {
    [-w * r[1], w * r[0]]
}
fn rot(a: f64, v: V) -> V {
    let (s, c) = a.sin_cos();
    [c * v[0] - s * v[1], s * v[0] + c * v[1]]
}
fn len(a: V) -> f64 {
    a[0].hypot(a[1])
}

/// A body's outline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Half width and half height.
    Box(V),
    Circle(f64),
}

/// One rigid body. Angles are radians, clockwise on screen (y is down).
#[derive(Clone, Debug)]
pub struct Body {
    pub pos: V,
    pub angle: f64,
    pub vel: V,
    pub spin: f64,
    pub shape: Shape,
    inv_mass: f64,
    inv_inertia: f64,
}

impl Body {
    pub fn new(pos: V, angle: f64, shape: Shape, fixed: bool) -> Body {
        // Unit density per square pixel; mass only matters relatively.
        let (mass, inertia) = match shape {
            Shape::Box([hw, hh]) => {
                let m = 4.0 * hw * hh;
                (m, m * (4.0 * hw * hw + 4.0 * hh * hh) / 12.0)
            }
            Shape::Circle(r) => {
                let m = std::f64::consts::PI * r * r;
                (m, 0.5 * m * r * r)
            }
        };
        let (im, ii) = if fixed || mass <= 0.0 { (0.0, 0.0) } else { (1.0 / mass, 1.0 / inertia) };
        Body { pos, angle, vel: [0.0, 0.0], spin: 0.0, shape, inv_mass: im, inv_inertia: ii }
    }

    fn corners(&self) -> [V; 4] {
        let Shape::Box([hw, hh]) = self.shape else { return [self.pos; 4] };
        [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(|c| add(self.pos, rot(self.angle, c)))
    }
}

/// A half-plane the bodies stay on the side `normal` points to:
/// `dot(normal, x) >= offset`.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub normal: V,
    pub offset: f64,
}

#[derive(Clone, Debug)]
struct Contact {
    a: usize,
    /// The other body, or None for a plane.
    b: Option<usize>,
    /// From a to b.
    normal: V,
    point: V,
    depth: f64,
    pn: f64,
    pt: f64,
    bounce_to: f64,
}

/// The world: bodies, planes and settings.
pub struct World {
    pub bodies: Vec<Body>,
    pub planes: Vec<Plane>,
    pub gravity: f64,
    pub bounce: f64,
    pub friction: f64,
}

impl World {
    /// Advances by `dt` seconds in `substeps` steps.
    pub fn step(&mut self, dt: f64, substeps: u32) {
        let h = dt / substeps.max(1) as f64;
        for _ in 0..substeps.max(1) {
            self.substep(h);
        }
    }

    fn substep(&mut self, h: f64) {
        for b in self.bodies.iter_mut().filter(|b| b.inv_mass > 0.0) {
            b.vel[1] += self.gravity * h;
        }
        let mut contacts = self.contacts();
        for c in &mut contacts {
            let vn = dot(self.relative_velocity(c), c.normal);
            // Only real impacts bounce: resting contacts stay put.
            c.bounce_to = if vn < -60.0 { -self.bounce * vn } else { 0.0 };
        }
        for _ in 0..12 {
            for c in &mut contacts {
                self.solve(c, h);
            }
        }
        for b in self.bodies.iter_mut().filter(|b| b.inv_mass > 0.0) {
            b.pos = add(b.pos, mul(b.vel, h));
            b.angle += b.spin * h;
        }
    }

    fn velocity_at(&self, i: usize, p: V) -> V {
        let b = &self.bodies[i];
        add(b.vel, cross_sv(b.spin, sub(p, b.pos)))
    }

    /// Velocity of b relative to a at the contact (positive along the
    /// normal when they separate).
    fn relative_velocity(&self, c: &Contact) -> V {
        let va = self.velocity_at(c.a, c.point);
        let vb = c.b.map_or([0.0, 0.0], |b| self.velocity_at(b, c.point));
        sub(vb, va)
    }

    fn apply(&mut self, i: usize, p: V, imp: V) {
        let b = &mut self.bodies[i];
        b.vel = add(b.vel, mul(imp, b.inv_mass));
        b.spin += b.inv_inertia * cross(sub(p, b.pos), imp);
    }

    fn solve(&mut self, c: &mut Contact, h: f64) {
        let ra = sub(c.point, self.bodies[c.a].pos);
        let rb = c.b.map(|b| sub(c.point, self.bodies[b].pos));
        let (ima, iia) = (self.bodies[c.a].inv_mass, self.bodies[c.a].inv_inertia);
        let (imb, iib) = c.b.map_or((0.0, 0.0), |b| (self.bodies[b].inv_mass, self.bodies[b].inv_inertia));
        let k = |d: V| {
            let ka = ima + iia * cross(ra, d).powi(2);
            let kb = rb.map_or(0.0, |rb| imb + iib * cross(rb, d).powi(2));
            ka + kb
        };
        let n = c.normal;
        let kn = k(n);
        if kn <= 0.0 {
            return;
        }
        // Normal: stop them closing, push out what overlaps, bounce.
        let vn = dot(self.relative_velocity(c), n);
        let bias = 0.2 / h * (c.depth - 0.5).max(0.0);
        let want = (bias.max(c.bounce_to)) - vn;
        let d = want / kn;
        let old = c.pn;
        c.pn = (old + d).max(0.0);
        let imp = mul(n, c.pn - old);
        self.apply(c.a, c.point, mul(imp, -1.0));
        if let Some(b) = c.b {
            self.apply(b, c.point, imp);
        }
        // Friction, at most `friction` times the normal push.
        let t = [-n[1], n[0]];
        let kt = k(t);
        if kt <= 0.0 {
            return;
        }
        let vt = dot(self.relative_velocity(c), t);
        let limit = self.friction * c.pn;
        let old = c.pt;
        c.pt = (old - vt / kt).clamp(-limit, limit);
        let imp = mul(t, c.pt - old);
        self.apply(c.a, c.point, mul(imp, -1.0));
        if let Some(b) = c.b {
            self.apply(b, c.point, imp);
        }
    }

    fn contacts(&self) -> Vec<Contact> {
        let mut out = Vec::new();
        let mk = |a, b, normal, point, depth| Contact { a, b, normal, point, depth, pn: 0.0, pt: 0.0, bounce_to: 0.0 };
        for (i, b) in self.bodies.iter().enumerate() {
            if b.inv_mass == 0.0 {
                continue;
            }
            for pl in &self.planes {
                // The plane's normal points into free space; from the body
                // to the plane is the other way.
                let n = mul(pl.normal, -1.0);
                match b.shape {
                    Shape::Box(_) => {
                        for p in b.corners() {
                            let depth = pl.offset - dot(pl.normal, p);
                            if depth > -1.0 {
                                out.push(mk(i, None, n, p, depth));
                            }
                        }
                    }
                    Shape::Circle(r) => {
                        let depth = pl.offset - (dot(pl.normal, b.pos) - r);
                        if depth > -1.0 {
                            out.push(mk(i, None, n, sub(b.pos, mul(pl.normal, r)), depth));
                        }
                    }
                }
            }
        }
        for i in 0..self.bodies.len() {
            for j in i + 1..self.bodies.len() {
                let (a, b) = (&self.bodies[i], &self.bodies[j]);
                if a.inv_mass == 0.0 && b.inv_mass == 0.0 {
                    continue;
                }
                for (normal, point, depth) in collide(a, b) {
                    out.push(mk(i, Some(j), normal, point, depth));
                }
            }
        }
        out
    }
}

/// Contacts between two bodies: (normal from a to b, point, depth).
fn collide(a: &Body, b: &Body) -> Vec<(V, V, f64)> {
    match (a.shape, b.shape) {
        (Shape::Circle(ra), Shape::Circle(rb)) => {
            let d = sub(b.pos, a.pos);
            let l = len(d);
            if l >= ra + rb + 1.0 || l < 1e-9 {
                return Vec::new();
            }
            let n = mul(d, 1.0 / l);
            vec![(n, add(a.pos, mul(n, ra)), ra + rb - l)]
        }
        (Shape::Box(_), Shape::Circle(_)) => box_circle(a, b),
        (Shape::Circle(_), Shape::Box(_)) => box_circle(b, a).into_iter().map(|(n, p, d)| (mul(n, -1.0), p, d)).collect(),
        (Shape::Box(_), Shape::Box(_)) => box_box(a, b),
    }
}

fn box_circle(bx: &Body, c: &Body) -> Vec<(V, V, f64)> {
    let (Shape::Box([hw, hh]), Shape::Circle(r)) = (bx.shape, c.shape) else { return Vec::new() };
    let local = rot(-bx.angle, sub(c.pos, bx.pos));
    let closest = [local[0].clamp(-hw, hw), local[1].clamp(-hh, hh)];
    let d = sub(local, closest);
    let l = len(d);
    if l > r + 1.0 {
        return Vec::new();
    }
    let (n_local, depth) = if l > 1e-9 {
        (mul(d, 1.0 / l), r - l)
    } else {
        // The centre is inside: out through the nearest side.
        let (dx, dy) = (hw - local[0].abs(), hh - local[1].abs());
        if dx < dy { ([local[0].signum(), 0.0], dx + r) } else { ([0.0, local[1].signum()], dy + r) }
    };
    let n = rot(bx.angle, n_local);
    vec![(n, add(bx.pos, rot(bx.angle, closest)), depth)]
}

/// Separating axes over both boxes' edge normals; contact points are
/// the incident edge clipped to the reference face's sides.
fn box_box(a: &Body, b: &Body) -> Vec<(V, V, f64)> {
    let (Shape::Box(ha), Shape::Box(hb)) = (a.shape, b.shape) else { return Vec::new() };
    let axes = |bd: &Body| [rot(bd.angle, [1.0, 0.0]), rot(bd.angle, [0.0, 1.0])];
    let extent = |h: V, ax: [V; 2], n: V| h[0] * dot(ax[0], n).abs() + h[1] * dot(ax[1], n).abs();
    let (aa, ab) = (axes(a), axes(b));
    let d = sub(b.pos, a.pos);
    let mut best: Option<(f64, V, bool)> = None;
    for (n, of_a) in [(aa[0], true), (aa[1], true), (ab[0], false), (ab[1], false)] {
        let overlap = extent(ha, aa, n) + extent(hb, ab, n) - dot(d, n).abs();
        if overlap < -1.0 {
            return Vec::new();
        }
        // Prefer a's faces a little, so the choice doesn't flicker.
        let score = if of_a { overlap } else { overlap * 1.02 };
        if best.is_none_or(|(s, _, _)| score < s) {
            let n = if dot(d, n) < 0.0 { mul(n, -1.0) } else { n };
            best = Some((score, n, of_a));
        }
    }
    let (_, n, of_a) = best.expect("four axes");
    // Reference body r (whose face the normal is), incident body i.
    let (r, i, nr) = if of_a { (a, b, n) } else { (b, a, mul(n, -1.0)) };
    let (Shape::Box(hr), Shape::Box(hi)) = (r.shape, i.shape) else { return Vec::new() };
    let ar = axes(r);
    let ai = axes(i);
    // The incident face: the one most against nr.
    let faces = [(ai[0], hi[0], hi[1], ai[1]), (mul(ai[0], -1.0), hi[0], hi[1], ai[1]), (ai[1], hi[1], hi[0], ai[0]), (mul(ai[1], -1.0), hi[1], hi[0], ai[0])];
    let (fnorm, fdist, fhalf, fside) = faces.into_iter().min_by(|x, y| dot(x.0, nr).total_cmp(&dot(y.0, nr))).expect("four faces");
    let centre = add(i.pos, mul(fnorm, fdist));
    let mut pts = vec![add(centre, mul(fside, fhalf)), sub(centre, mul(fside, fhalf))];
    // The reference face's sides: along the axis across nr.
    let (side, side_half, face_half) = if dot(ar[0], nr).abs() > dot(ar[1], nr).abs() { (ar[1], hr[1], hr[0]) } else { (ar[0], hr[0], hr[1]) };
    for s in [1.0, -1.0] {
        let sn = mul(side, s);
        let off = dot(sn, r.pos) + side_half;
        pts = clip(&pts, sn, off);
        if pts.len() < 2 {
            return Vec::new();
        }
    }
    let face_off = dot(nr, r.pos) + face_half;
    let mut out = Vec::new();
    for p in pts {
        let depth = face_off - dot(nr, p);
        if depth > -1.0 {
            out.push((n, p, depth));
        }
    }
    out
}

/// Keeps the part of segment `pts` with dot(n, p) <= off.
fn clip(pts: &[V], n: V, off: f64) -> Vec<V> {
    let d: Vec<f64> = pts.iter().map(|p| dot(n, *p) - off).collect();
    let mut out: Vec<V> = pts.iter().zip(&d).filter(|(_, d)| **d <= 0.0).map(|(p, _)| *p).collect();
    if d[0] * d[1] < 0.0 {
        let t = d[0] / (d[0] - d[1]);
        out.push(add(pts[0], mul(sub(pts[1], pts[0]), t)));
    }
    out
}

/// Fewer keys for the same move: drops samples a straight line between
/// their neighbours already passes within `tol` of.
pub fn reduce(samples: &[(m::Micros, [f64; 3])], tol: [f64; 3]) -> Vec<usize> {
    if samples.len() <= 2 {
        return (0..samples.len()).collect();
    }
    let mut keep = vec![0usize];
    let mut anchor = 0;
    let mut i = 2;
    while i < samples.len() {
        // Could the line from the anchor to i stand in for every sample
        // in between?
        let (t0, a) = samples[anchor];
        let (t1, b) = samples[i];
        let fits = (anchor + 1..i).all(|j| {
            let (t, v) = samples[j];
            let u = (t - t0) as f64 / (t1 - t0).max(1) as f64;
            (0..3).all(|k| (a[k] + (b[k] - a[k]) * u - v[k]).abs() <= tol[k])
        });
        if !fits {
            keep.push(i - 1);
            anchor = i - 1;
        }
        i += 1;
    }
    keep.push(samples.len() - 1);
    keep
}

fn per_clip<'a>(op: &'a Value, key: &str, id: u32) -> Option<&'a Value> {
    match op.get(key)? {
        Value::Object(o) => o.get(&id.to_string()),
        v => Some(v),
    }
}

fn pair(v: &Value) -> Option<V> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

impl Session {
    pub(super) fn physics(&mut self, op: &Value) -> Result<String, String> {
        run(&mut self.doc, op)
    }
}

fn view_clip(doc: &Doc, id: u32) -> Result<crate::view::Clip, String> {
    doc.project.clip(id).cloned().ok_or_else(|| format!("No clip {id} (see summary for ids)"))
}

fn model_clip(doc: &Doc, id: u32) -> Option<m::Clip> {
    doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).cloned()
}

/// Runs a simulation described like the op (see the module docs) and
/// writes its keys into `doc` (the Inspector's Physics section uses it
/// too).
pub fn run(doc: &mut Doc, op: &Value) -> Result<String, String> {
    {
        let ids: Vec<u32> = op.get("clips").and_then(Value::as_array).ok_or("physics needs \"clips\": [ids] that fall and collide")?.iter().map(|c| c.as_u64().map(|c| c as u32).ok_or("clips are ids")).collect::<Result<_, _>>()?;
        let statics: Vec<u32> = op.get("static").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.as_u64().map(|c| c as u32)).collect()).unwrap_or_default();
        if ids.is_empty() {
            return Err("physics needs at least one clip".into());
        }
        let (w, h) = (doc.project.width as f64, doc.project.height as f64);
        let fps = (doc.project.fps as f64).max(1.0);
        let starts: Vec<f64> = ids.iter().map(|id| view_clip(doc, *id).map(|c| secs(c.start))).collect::<Result<_, _>>()?;
        let at = f(op, "at").unwrap_or_else(|| starts.iter().cloned().fold(f64::MIN, f64::max));
        let duration = f(op, "duration").unwrap_or(3.0).clamp(0.1, 60.0);
        let mut world = World {
            bodies: Vec::new(),
            planes: Vec::new(),
            gravity: f(op, "gravity").unwrap_or(2.2 * h),
            bounce: f(op, "bounce").unwrap_or(0.35).clamp(0.0, 1.0),
            friction: f(op, "friction").unwrap_or(0.5).clamp(0.0, 2.0),
        };
        match op.get("floor") {
            Some(Value::Bool(false)) => {}
            v => world.planes.push(Plane { normal: [0.0, -1.0], offset: -v.and_then(Value::as_f64).unwrap_or(h / 2.0) }),
        }
        if op.get("walls").and_then(Value::as_bool).unwrap_or(false) {
            world.planes.push(Plane { normal: [1.0, 0.0], offset: -w / 2.0 });
            world.planes.push(Plane { normal: [-1.0, 0.0], offset: -w / 2.0 });
        }
        if op.get("ceiling").and_then(Value::as_bool).unwrap_or(false) {
            world.planes.push(Plane { normal: [0.0, 1.0], offset: -h / 2.0 });
        }
        // Each body from the layer's box on screen at the start.
        struct Start {
            id: u32,
            body: usize,
            pos0: V,
            rot0: f64,
            anchor0: V,
            centre0: V,
            angle0: f64,
        }
        let mut starts_of = Vec::new();
        for (k, &id) in ids.iter().chain(&statics).enumerate() {
            let fixed = k >= ids.len();
            let mc = model_clip(doc, id).ok_or_else(|| format!("No clip {id} (see summary for ids)"))?;
            if mc.parent.is_some() {
                return Err(format!("clip {id} is parented; physics moves free layers (unparent it, or simulate its parent)"));
            }
            if mc.transform.three_d {
                return Err(format!("clip {id} is a 3D layer; physics works on flat layers"));
            }
            let vc = view_clip(doc, id)?;
            let t = micros(at).clamp(vc.start, vc.start + vc.dur.max(1) - 1);
            let g = xform::geo(&doc.project, &doc.model, id, t).ok_or_else(|| format!("clip {id} has no picture to collide"))?;
            let q = g.quad().map(|p| [p.x as f64, p.y as f64]);
            let centre = mul(add(add(q[0], q[1]), add(q[2], q[3])), 0.25);
            let (e1, e2) = (sub(q[1], q[0]), sub(q[3], q[0]));
            let angle = e1[1].atan2(e1[0]);
            let shape = match per_clip(op, "shape", id).and_then(Value::as_str) {
                Some("circle" | "ball" | "round") => Shape::Circle(len(e1).min(len(e2)) / 2.0),
                None | Some("box" | "rect" | "card") => Shape::Box([len(e1) / 2.0, len(e2) / 2.0]),
                Some(o) => return Err(format!("shape \"{o}\": box or circle")),
            };
            let mut body = Body::new(centre, angle, shape, fixed);
            if !fixed {
                if let Some(v) = per_clip(op, "push", id).and_then(pair) {
                    body.vel = v;
                }
                if let Some(s) = per_clip(op, "spin", id).and_then(Value::as_f64) {
                    body.spin = s.to_radians();
                }
            }
            world.bodies.push(body);
            let local = t - vc.start;
            let pos0 = match mc.property_at("transform.position", local) {
                Some(m::Value::Point(p)) => [p.x, p.y],
                _ => [0.0, 0.0],
            };
            let rot0 = mc.property_at("transform.rotation", local).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let a = g.anchor_canvas();
            if !fixed {
                starts_of.push(Start { id, body: k, pos0, rot0, anchor0: [a.x as f64, a.y as f64], centre0: centre, angle0: angle });
            }
        }
        // Run it, a sample per frame.
        let frames = (duration * fps).round() as usize;
        let mut samples: Vec<Vec<(m::Micros, [f64; 3])>> = vec![Vec::new(); starts_of.len()];
        for n in 0..=frames {
            if n > 0 {
                world.step(1.0 / fps, 8);
            }
            let t = micros(at + n as f64 / fps);
            for (s, out) in starts_of.iter().zip(samples.iter_mut()) {
                let b = &world.bodies[s.body];
                let turn = b.angle - s.angle0;
                // The anchor rides along with the body.
                let anchor = add(b.pos, rot(turn, sub(s.anchor0, s.centre0)));
                let pos = add(s.pos0, sub(anchor, s.anchor0));
                out.push((t, [pos[0], pos[1], s.rot0 + turn.to_degrees()]));
            }
        }
        let seq = doc.seq();
        let mut said = Vec::new();
        let mut edits = Vec::new();
        for (s, smp) in starts_of.iter().zip(&samples) {
            let vc = view_clip(doc, s.id)?;
            let inside: Vec<(m::Micros, [f64; 3])> = smp.iter().filter(|(t, _)| *t >= vc.start && *t <= vc.start + vc.dur).map(|(t, v)| (*t - vc.start, *v)).collect();
            if inside.is_empty() {
                said.push(format!("{} (not on screen then)", s.id));
                continue;
            }
            let keep = reduce(&inside, [0.4, 0.4, 0.25]);
            let (t0, t1) = (inside[0].0, inside[inside.len() - 1].0);
            let mc = model_clip(doc, s.id).ok_or("that clip is gone")?;
            for (path, k) in [("transform.position", 0usize), ("transform.rotation", 2)] {
                let mut tr = mc.keyframes.get(path).cloned().unwrap_or_default();
                // The run replaces what was keyed over its time.
                tr.keys.retain(|key| key.time < t0 || key.time > t1);
                for &i in &keep {
                    let (t, v) = inside[i];
                    let value = if k == 0 { m::Value::Point(m::Vec2 { x: v[0], y: v[1] }) } else { m::Value::Angle(v[2]) };
                    tr.set_key(m::Keyframe::new(t, value, m::Interp::Linear));
                }
                edits.push(m::Edit::SetTrack { seq, clip: m::ClipId(s.id as u64), property: path.into(), track: Some(tr) });
            }
            said.push(format!("{} ({} keys)", s.id, keep.len()));
        }
        // One step to undo, all or nothing.
        doc.apply("Physics", m::Edit::Batch { edits });
        if let Some(e) = doc.error.take() {
            return Err(e);
        }
        Ok(format!("simulated {:.1}s from {at:.2}s: {}", duration, said.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn a_box_falls_bounces_and_comes_to_rest_on_the_floor() {
        let mut w = World { bodies: vec![Body::new([0.0, 0.0], 0.0, Shape::Box([50.0, 25.0]), false)], planes: vec![Plane { normal: [0.0, -1.0], offset: -300.0 }], gravity: 2000.0, bounce: 0.4, friction: 0.5 };
        let mut lowest: f64 = 0.0;
        let mut went_up = false;
        let mut prev = 0.0;
        for _ in 0..180 {
            w.step(1.0 / 60.0, 8);
            let y = w.bodies[0].pos[1];
            lowest = lowest.max(y);
            went_up |= y < prev - 0.5 && prev > 200.0;
            prev = y;
        }
        let b = &w.bodies[0];
        assert!(went_up, "it bounced");
        assert!((b.pos[1] - 275.0).abs() < 2.0, "rests on the floor: {:?}", b.pos);
        assert!(lowest < 285.0, "never sank far into it: {lowest}");
        assert!(b.angle.abs() < 0.01 && len(b.vel) < 5.0);
    }

    #[test]
    fn boxes_stack_and_a_tilted_one_topples_flat() {
        let floor = Plane { normal: [0.0, -1.0], offset: -200.0 };
        let mut w = World { bodies: vec![Body::new([0.0, 150.0], 0.0, Shape::Box([60.0, 30.0]), false), Body::new([10.0, 40.0], 0.0, Shape::Box([40.0, 30.0]), false)], planes: vec![floor], gravity: 2000.0, bounce: 0.1, friction: 0.6 };
        for _ in 0..240 {
            w.step(1.0 / 60.0, 8);
        }
        assert!((w.bodies[0].pos[1] - 170.0).abs() < 3.0, "{:?}", w.bodies[0].pos);
        assert!((w.bodies[1].pos[1] - 110.0).abs() < 4.0, "the top one sits on the bottom one: {:?}", w.bodies[1].pos);
        let mut t = World { bodies: vec![Body::new([0.0, 100.0], 0.5, Shape::Box([60.0, 20.0]), false)], planes: vec![floor], gravity: 2000.0, bounce: 0.0, friction: 0.6 };
        for _ in 0..300 {
            t.step(1.0 / 60.0, 8);
        }
        let a = t.bodies[0].angle.rem_euclid(std::f64::consts::PI);
        assert!(a < 0.02 || a > std::f64::consts::PI - 0.02, "lies flat: {}", t.bodies[0].angle);
        assert!((t.bodies[0].pos[1] - 180.0).abs() < 3.0, "{:?}", t.bodies[0].pos);
    }

    #[test]
    fn balls_roll_off_each_other_and_keys_come_out_few() {
        let floor = Plane { normal: [0.0, -1.0], offset: -200.0 };
        let mut w = World { bodies: vec![Body::new([0.0, 160.0], 0.0, Shape::Circle(40.0), true), Body::new([5.0, 0.0], 0.0, Shape::Circle(30.0), false)], planes: vec![floor], gravity: 2000.0, bounce: 0.2, friction: 0.3 };
        for _ in 0..240 {
            w.step(1.0 / 60.0, 8);
        }
        assert!(w.bodies[1].pos[0] > 40.0, "rolled off the fixed ball: {:?}", w.bodies[1].pos);
        assert!((w.bodies[1].pos[1] - 170.0).abs() < 3.0);
        let line: Vec<(m::Micros, [f64; 3])> = (0..50).map(|i| (i * 1000, [i as f64 * 2.0, 0.0, 0.0])).collect();
        assert_eq!(reduce(&line, [0.1; 3]), vec![0, 49]);
    }

    #[test]
    fn a_pile_of_cards_comes_to_rest() {
        // The pile from the Codex evaluation: five 300x180 cards dropped
        // from above a 1920x1080 frame, tilted, with walls.
        let starts = [([-210.0, -720.0], -18.0), ([120.0, -760.0], 24.0), ([-120.0, -1050.0], 12.0), ([130.0, -1090.0], -28.0), ([0.0, -1390.0], 33.0f64)];
        let bodies = starts.iter().map(|(p, a)| Body::new(*p, a.to_radians(), Shape::Box([150.0, 90.0]), false)).collect();
        let planes = vec![Plane { normal: [0.0, -1.0], offset: -540.0 }, Plane { normal: [1.0, 0.0], offset: -960.0 }, Plane { normal: [-1.0, 0.0], offset: -960.0 }];
        let mut w = World { bodies, planes, gravity: 3800.0, bounce: 0.18, friction: 0.85 };
        let mut track: Vec<Vec<(V, f64)>> = vec![Vec::new(); 5];
        for _ in 0..(4.4 * 30.0) as usize {
            w.step(1.0 / 30.0, 8);
            for (k, b) in w.bodies.iter().enumerate() {
                track[k].push((b.pos, b.angle));
            }
        }
        // Over the last second nothing drifts or shivers.
        for (k, tr) in track.iter().enumerate() {
            let tail = &tr[tr.len() - 30..];
            let (p0, a0) = tail[0];
            let moved = tail.iter().map(|(p, a)| len(sub(*p, p0)) + (a - a0).abs() * 150.0).fold(0.0, f64::max);
            assert!(moved < 2.0, "card {k} still moves {moved:.1} px in the last second: {:?}", tail.last());
            assert!(tail.last().unwrap().0[1] < 540.0, "above the floor");
        }
    }

    #[test]
    fn the_op_bakes_a_drop_onto_the_bottom_of_the_frame() {
        let mut s = Session::new(services::project::new_custom("Drop", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 120, "height": 60})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "set", "clip": id, "y": -120, "rotation": 20})], "up", false).unwrap();
        let said = s.edit(&[json!({"op": "physics", "clips": [id], "at": 0, "duration": 3, "bounce": 0.3})], "drop", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let keys = mc.keyframes["transform.position"].keys.len();
        assert!(keys > 3 && keys < 90, "{keys} keys: {said}");
        // At the end its box rests on the frame's bottom edge, flat.
        let g = xform::geo(&s.doc.project, &s.doc.model, id, micros(2.9)).unwrap();
        let bottom = g.quad().iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((bottom - 180.0).abs() < 3.0, "bottom {bottom}");
        let r = mc.property_at("transform.rotation", micros(2.9)).unwrap().as_f64().unwrap();
        assert!((r.rem_euclid(90.0)).min(90.0 - r.rem_euclid(90.0)) < 1.5, "flat on a side: {r}");
        assert!(s.edit(&[json!({"op": "physics", "clips": []})], "x", false).is_err());
    }
}
