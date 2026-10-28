// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rigid bodies in 3D: things that fall, tumble, bounce, slide, stack and
//! knock each other over.
//!
//! Each body's shape is filled with a clump of spheres (the cells of a grid
//! inside it; a thin shell for open surfaces), which gives its mass, its
//! inertia and what it touches. Contacts between spheres of different
//! bodies, the floor and fixed models are solved with sequential impulses
//! (restitution, Coulomb friction, a push apart for overlap) at 240 steps a
//! second; bodies that stop moving go to sleep until something wakes them.

use crate::{Collider, V3, add, cross, dot, length, normalise, scale, sub};
use studio_mesh::Mesh;
use studio_mesh::rig::{Quat, q_mul, q_normalise, q_rotate, q_to_mat};

/// One rigid body.
#[derive(Clone, Debug)]
pub struct Body {
    /// Spheres in the body's own frame (centre of mass at the origin).
    pub spheres: Vec<(V3, f32)>,
    pub pos: V3,
    pub rot: Quat,
    pub vel: V3,
    /// Radians per second about each world axis.
    pub spin: V3,
    pub inv_mass: f32,
    /// Inverse inertia in the body's frame (rows).
    pub inv_inertia: [V3; 3],
    pub bounce: f32,
    pub friction: f32,
    pub asleep: bool,
    /// Held still until this time (seconds): a shattered piece waits for its
    /// moment, a stack waits to be hit.
    pub wait_until: f32,
    /// Where the body's frame was at rest relative to the model it came
    /// from: model point = rest_offset + p (with no turn).
    pub rest_offset: V3,
    still_for: f32,
}

/// A body's shape from a model: spheres, mass properties and its centre.
pub fn clump(mesh: &Mesh, detail: usize, density: f32) -> Body {
    let (lo, hi) = mesh.bounds();
    let ext: V3 = std::array::from_fn(|i| (hi[i] - lo[i]).max(1e-3));
    let big = ext[0].max(ext[1]).max(ext[2]);
    let n = detail.clamp(3, 24) as f32;
    let cell = (big / n).max(ext[0].min(ext[1]).min(ext[2]) / 3.0).max(1e-3);
    let dims: [usize; 3] = std::array::from_fn(|i| ((ext[i] / cell).ceil() as usize).max(1));
    let bvh = studio_mesh::bvh::Bvh::of_mesh(mesh);
    let centre_of = |x: usize, y: usize, z: usize| [lo[0] + (x as f32 + 0.5) * ext[0] / dims[0] as f32, lo[1] + (y as f32 + 0.5) * ext[1] / dims[1] as f32, lo[2] + (z as f32 + 0.5) * ext[2] / dims[2] as f32];
    // Inside: an odd number of surface crossings along +x.
    let inside = |p: V3| -> bool {
        let mut count = 0;
        let mut t0 = 0.0f32;
        let dir = normalise([1.0, 0.000_37, 0.000_21]);
        while let Some(h) = bvh.ray(p, dir, t0, f32::INFINITY) {
            count += 1;
            t0 = h.t + 1e-3;
            if count > 64 {
                break;
            }
        }
        count % 2 == 1
    };
    let mut filled = vec![false; dims[0] * dims[1] * dims[2]];
    let idx = |x: usize, y: usize, z: usize| (z * dims[1] + y) * dims[0] + x;
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                filled[idx(x, y, z)] = inside(centre_of(x, y, z));
            }
        }
    }
    let half = [ext[0] / dims[0] as f32, ext[1] / dims[1] as f32, ext[2] / dims[2] as f32].map(|v| v * 0.5);
    let r = half[0].max(half[1]).max(half[2]) * 0.98;
    let mut mass_points: Vec<V3> = Vec::new();
    let mut shell: Vec<V3> = Vec::new();
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                if !filled[idx(x, y, z)] {
                    continue;
                }
                let c = centre_of(x, y, z);
                mass_points.push(c);
                let edge = x == 0 || y == 0 || z == 0 || x + 1 == dims[0] || y + 1 == dims[1] || z + 1 == dims[2] || !filled[idx(x - 1, y, z)] || !filled[idx(x + 1, y, z)] || !filled[idx(x, y - 1, z)] || !filled[idx(x, y + 1, z)] || !filled[idx(x, y, z - 1)] || !filled[idx(x, y, z + 1)];
                if edge {
                    shell.push(c);
                }
            }
        }
    }
    let radius;
    if mass_points.is_empty() {
        // Open or flat: spheres on the surface itself.
        let step = (mesh.triangles() / 400).max(1);
        for t in (0..mesh.triangles()).step_by(step) {
            mass_points.push(mesh.centroid(t));
        }
        shell = mass_points.clone();
        radius = (big / n * 0.5).max(1e-3);
    } else {
        radius = r;
    }
    let count = mass_points.len().max(1) as f32;
    let com = scale(mass_points.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / count);
    let cell_mass = density * (2.0 * half[0]) * (2.0 * half[1]) * (2.0 * half[2]);
    let mass = (cell_mass * count).max(1e-6);
    // Inertia from the cells as small spheres (plus each cell's own).
    let mut i = [[0.0f32; 3]; 3];
    let own = 0.4 * cell_mass * radius * radius;
    for p in &mass_points {
        let d = sub(*p, com);
        let d2 = dot(d, d);
        for a in 0..3 {
            for b in 0..3 {
                i[a][b] += cell_mass * (if a == b { d2 } else { 0.0 } - d[a] * d[b]);
            }
            i[a][a] += own;
        }
    }
    let inv_inertia = invert3(&i);
    // Keep the shell to a few hundred spheres.
    let keep = (shell.len() / 400).max(1);
    let spheres: Vec<(V3, f32)> = shell.iter().step_by(keep).map(|p| (sub(*p, com), radius * if keep > 1 { (keep as f32).cbrt() } else { 1.0 })).collect();
    Body { spheres, pos: com, rot: studio_mesh::rig::Q_ID, vel: [0.0; 3], spin: [0.0; 3], inv_mass: 1.0 / mass, inv_inertia, bounce: 0.3, friction: 0.5, asleep: false, wait_until: 0.0, rest_offset: com, still_for: 0.0 }
}

fn invert3(m: &[V3; 3]) -> [V3; 3] {
    let c0 = cross(m[1], m[2]);
    let det = dot(m[0], c0);
    if det.abs() < 1e-20 {
        return [[0.0; 3]; 3];
    }
    let c1 = cross(m[2], m[0]);
    let c2 = cross(m[0], m[1]);
    let k = 1.0 / det;
    // Rows of the inverse are the cofactor columns.
    [[c0[0] * k, c1[0] * k, c2[0] * k], [c0[1] * k, c1[1] * k, c2[1] * k], [c0[2] * k, c1[2] * k, c2[2] * k]]
}

fn mat_vec(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

impl Body {
    /// Holds the body still (a floor, a wall, a shelf).
    pub fn fix(&mut self) {
        self.inv_mass = 0.0;
        self.inv_inertia = [[0.0; 3]; 3];
    }

    pub fn fixed(&self) -> bool {
        self.inv_mass == 0.0
    }

    /// Inverse inertia in the world.
    fn inv_inertia_world(&self) -> [V3; 3] {
        let r = q_to_mat(self.rot);
        // R · I⁻¹ · Rᵀ
        // Column c of I⁻¹·Rᵀ is I⁻¹ · (row c of R).
        let a: [V3; 3] = std::array::from_fn(|c| mat_vec(&self.inv_inertia, r[c]));
        let mut out = [[0.0f32; 3]; 3];
        for row in 0..3 {
            for col in 0..3 {
                out[row][col] = dot(r[row], a[col]);
            }
        }
        out
    }

    fn velocity_at(&self, p: V3) -> V3 {
        add(self.vel, cross(self.spin, sub(p, self.pos)))
    }

    fn apply_impulse(&mut self, at: V3, j: V3, iw: &[V3; 3]) {
        if self.fixed() {
            return;
        }
        self.vel = add(self.vel, scale(j, self.inv_mass));
        self.spin = add(self.spin, mat_vec(iw, cross(sub(at, self.pos), j)));
    }

    /// The body's spheres in the world.
    pub fn world_spheres(&self) -> impl Iterator<Item = (V3, f32)> + '_ {
        self.spheres.iter().map(|(c, r)| (add(self.pos, q_rotate(self.rot, *c)), *r))
    }

    /// The turn and shift that carry the model's rest points to where the
    /// body is now (`model point ↦ rot·(p − rest_offset) + pos`).
    pub fn placement(&self) -> (Quat, V3) {
        (self.rot, sub(self.pos, q_rotate(self.rot, self.rest_offset)))
    }
}

/// How hard it is to change the relative speed of two bodies at a point
/// along `dir` (the inverse effective mass).
fn eff_mass(bodies: &[Body], iws: &[[V3; 3]], a: usize, b: Option<usize>, point: V3, ra: V3, dir: V3) -> f32 {
    let mut k = bodies[a].inv_mass + dot(dir, cross(mat_vec(&iws[a], cross(ra, dir)), ra));
    if let Some(b) = b {
        let rb = sub(point, bodies[b].pos);
        k += bodies[b].inv_mass + dot(dir, cross(mat_vec(&iws[b], cross(rb, dir)), rb));
    }
    k
}

/// A contact between two bodies (`b` = None for the world).
struct Contact {
    a: usize,
    b: Option<usize>,
    point: V3,
    /// From b towards a.
    normal: V3,
    depth: f32,
    friction: f32,
    bounce: f32,
    /// Accumulated impulses (normal, two tangents).
    jn: f32,
    jt: [f32; 2],
    target_vn: f32,
}

/// A world of bodies and fixed colliders.
#[derive(Clone, Debug)]
pub struct World {
    pub bodies: Vec<Body>,
    pub colliders: Vec<Collider>,
    pub gravity: V3,
    pub time: f32,
    /// Seconds per solver step.
    pub step: f32,
    pub iterations: usize,
    /// Time asked for but not yet stepped (less than one step): a frame
    /// that is not a whole number of steps (1/25 s is 9.6 of them) carries
    /// the rest to the next, so the world keeps the clock's pace.
    owed: f64,
}

impl World {
    pub fn new(gravity: V3) -> World {
        World { bodies: Vec::new(), colliders: Vec::new(), gravity, time: 0.0, step: 1.0 / 240.0, iterations: 12, owed: 0.0 }
    }

    /// Runs `seconds` forward, in whole solver steps; what is left over
    /// is run with the next call.
    pub fn advance(&mut self, seconds: f32) {
        let step = f64::from(self.step.max(1e-6));
        self.owed += f64::from(seconds.max(0.0));
        // A hair of slack so 1/30 s at 240 a second is 8 steps, not 7.
        let n = (self.owed / step + 1e-6).floor();
        self.owed = (self.owed - n * step).max(0.0);
        for _ in 0..n as usize {
            self.substep(self.step);
        }
    }

    fn substep(&mut self, h: f32) {
        self.time += h;
        let now = self.time;
        for b in &mut self.bodies {
            if b.fixed() || b.asleep || now < b.wait_until {
                continue;
            }
            b.vel = add(b.vel, scale(self.gravity, h));
            // A touch of air drag keeps spins from running forever.
            b.spin = scale(b.spin, 1.0 - 0.02 * h);
        }
        let mut contacts = self.contacts();
        let iws: Vec<[V3; 3]> = self.bodies.iter().map(Body::inv_inertia_world).collect();
        // Bounce targets from the velocities before solving.
        for c in &mut contacts {
            let va = self.bodies[c.a].velocity_at(c.point);
            let vb = c.b.map_or([0.0; 3], |b| self.bodies[b].velocity_at(c.point));
            let vn = dot(sub(va, vb), c.normal);
            c.target_vn = if vn < -1.0 * self.gravity.iter().map(|g| g.abs()).fold(0.0, f32::max) * h * 4.0 { -c.bounce * vn } else { 0.0 };
        }
        for _ in 0..self.iterations {
            for c in &mut contacts {
                let (a, b) = (c.a, c.b);
                let ra = sub(c.point, self.bodies[a].pos);
                let va = self.bodies[a].velocity_at(c.point);
                let vb = b.map_or([0.0; 3], |b| self.bodies[b].velocity_at(c.point));
                let rel = sub(va, vb);
                let n = c.normal;
                let mass_along = |bodies: &[Body], dir: V3| eff_mass(bodies, &iws, a, b, c.point, ra, dir);
                // Normal: stop approach, bounce, push apart gently.
                let kn = mass_along(&self.bodies, n);
                if kn <= 1e-12 {
                    continue;
                }
                let bias = 0.2 / h * (c.depth - 0.02).max(0.0);
                let vn = dot(rel, n);
                let mut dj = (c.target_vn.max(bias) - vn) / kn;
                let old = c.jn;
                c.jn = (old + dj).max(0.0);
                dj = c.jn - old;
                let jv = scale(n, dj);
                self.bodies[a].apply_impulse(c.point, jv, &iws[a]);
                if let Some(b) = b {
                    self.bodies[b].apply_impulse(c.point, scale(jv, -1.0), &iws[b]);
                }
                // Friction in the contact plane, up to μ times the push.
                let va = self.bodies[a].velocity_at(c.point);
                let vb = b.map_or([0.0; 3], |b| self.bodies[b].velocity_at(c.point));
                let rel = sub(va, vb);
                let vt = sub(rel, scale(n, dot(rel, n)));
                let speed = length(vt);
                if speed > 1e-6 {
                    let t = scale(vt, 1.0 / speed);
                    let kt = mass_along(&self.bodies, t);
                    if kt > 1e-12 {
                        let limit = c.friction * c.jn;
                        let old = c.jt[0];
                        c.jt[0] = (old - speed / kt).clamp(-limit, limit);
                        let dj = c.jt[0] - old;
                        let jv = scale(t, dj);
                        self.bodies[a].apply_impulse(c.point, jv, &iws[a]);
                        if let Some(b) = b {
                            self.bodies[b].apply_impulse(c.point, scale(jv, -1.0), &iws[b]);
                        }
                    }
                }
            }
        }
        // Move, and sleep what has stopped.
        for b in &mut self.bodies {
            if b.fixed() || b.asleep || now < b.wait_until {
                continue;
            }
            b.pos = add(b.pos, scale(b.vel, h));
            let w = b.spin;
            let dq = q_mul([w[0], w[1], w[2], 0.0], b.rot);
            b.rot = q_normalise([b.rot[0] + 0.5 * h * dq[0], b.rot[1] + 0.5 * h * dq[1], b.rot[2] + 0.5 * h * dq[2], b.rot[3] + 0.5 * h * dq[3]]);
            let size = b.spheres.iter().map(|s| length(s.0) + s.1).fold(1.0f32, f32::max);
            let calm = length(b.vel) < size * 0.08 && length(b.spin) < 0.12;
            b.still_for = if calm { b.still_for + h } else { 0.0 };
            if b.still_for > 0.6 {
                b.asleep = true;
                b.vel = [0.0; 3];
                b.spin = [0.0; 3];
            }
        }
    }

    /// Wakes bodies touching moving ones (so a stack falls when hit).
    fn wake_touching(&mut self, pairs: &[(usize, usize)]) {
        for &(a, b) in pairs {
            let (wa, wb) = (self.bodies[a].asleep, self.bodies[b].asleep);
            let moving = |x: &Body| !x.asleep && !x.fixed() && (length(x.vel) > 1.0 || length(x.spin) > 0.2);
            if wa && moving(&self.bodies[b]) {
                self.bodies[a].asleep = false;
                self.bodies[a].still_for = 0.0;
            }
            if wb && moving(&self.bodies[a]) {
                self.bodies[b].asleep = false;
                self.bodies[b].still_for = 0.0;
            }
        }
    }

    fn contacts(&mut self) -> Vec<Contact> {
        let now = self.time;
        let mut out = Vec::new();
        // Body boxes for the pairs worth testing.
        let boxes: Vec<(V3, V3)> = self
            .bodies
            .iter()
            .map(|b| {
                let mut lo = [f32::INFINITY; 3];
                let mut hi = [f32::NEG_INFINITY; 3];
                for (c, r) in b.world_spheres() {
                    for i in 0..3 {
                        lo[i] = lo[i].min(c[i] - r);
                        hi[i] = hi[i].max(c[i] + r);
                    }
                }
                (lo, hi)
            })
            .collect();
        let active = |b: &Body| !b.fixed() && !b.asleep && now >= b.wait_until;
        let mut touching = Vec::new();
        for i in 0..self.bodies.len() {
            for j in i + 1..self.bodies.len() {
                let (bi, bj) = (&self.bodies[i], &self.bodies[j]);
                if !active(bi) && !active(bj) {
                    continue;
                }
                if (0..3).any(|k| boxes[i].1[k] < boxes[j].0[k] || boxes[j].1[k] < boxes[i].0[k]) {
                    continue;
                }
                touching.push((i, j));
                let sj: Vec<(V3, f32)> = bj.world_spheres().collect();
                // Spheres of j near each sphere of i (j's in a grid).
                let cell = sj.iter().map(|s| s.1).fold(1e-3f32, f32::max) * 2.0 + bi.spheres.iter().map(|s| s.1).fold(0.0f32, f32::max);
                let mut grid: std::collections::HashMap<[i32; 3], Vec<usize>> = std::collections::HashMap::new();
                for (k, (c, _)) in sj.iter().enumerate() {
                    grid.entry(c.map(|v| (v / cell).floor() as i32)).or_default().push(k);
                }
                let mut found: Vec<(V3, V3, f32)> = Vec::new();
                for (ca, ra) in bi.world_spheres() {
                    let key = ca.map(|v| (v / cell).floor() as i32);
                    for dz in -1..=1 {
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                let Some(list) = grid.get(&[key[0] + dx, key[1] + dy, key[2] + dz]) else { continue };
                                for &k in list {
                                    let (cb, rb) = sj[k];
                                    let d = sub(ca, cb);
                                    let l = length(d);
                                    if l < ra + rb && l > 1e-6 {
                                        let n = scale(d, 1.0 / l);
                                        found.push((add(cb, scale(n, rb - (ra + rb - l) * 0.5)), n, ra + rb - l));
                                    }
                                }
                            }
                        }
                    }
                }
                for (p, n, depth) in reduce(found) {
                    out.push(Contact { a: i, b: Some(j), point: p, normal: n, depth, friction: (bi.friction * bj.friction).sqrt(), bounce: bi.bounce.max(bj.bounce), jn: 0.0, jt: [0.0; 2], target_vn: 0.0 });
                }
            }
        }
        self.wake_touching(&touching);
        for (i, b) in self.bodies.iter().enumerate() {
            if !active(b) {
                continue;
            }
            for col in &self.colliders {
                let mut found = Vec::new();
                for (c, r) in b.world_spheres() {
                    if let Some(k) = col.push_out(c, r) {
                        let depth = length(sub(k.point, c));
                        found.push((sub(c, scale(k.normal, r)), k.normal, depth));
                    }
                }
                let fr = match col {
                    Collider::Ground { friction, .. } => *friction,
                    _ => 0.5,
                };
                for (p, n, depth) in reduce(found) {
                    out.push(Contact { a: i, b: None, point: p, normal: n, depth, friction: (b.friction * fr).sqrt(), bounce: b.bounce, jn: 0.0, jt: [0.0; 2], target_vn: 0.0 });
                }
            }
        }
        out
    }
}

/// Many sphere contacts between the same two things kept to a handful:
/// the deepest and the ones furthest apart (enough to rest flat).
fn reduce(mut found: Vec<(V3, V3, f32)>) -> Vec<(V3, V3, f32)> {
    if found.len() <= 4 {
        return found;
    }
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut keep = vec![found[0]];
    while keep.len() < 6 {
        let next = found.iter().filter(|f| !keep.iter().any(|k| length(sub(k.0, f.0)) < 1e-3)).max_by(|a, b| {
            let da = keep.iter().map(|k| length(sub(k.0, a.0))).fold(f32::INFINITY, f32::min);
            let db = keep.iter().map(|k| length(sub(k.0, b.0))).fold(f32::INFINITY, f32::min);
            da.total_cmp(&db)
        });
        match next {
            Some(f) => keep.push(*f),
            None => break,
        }
    }
    // The push apart is shared, so the overlap is not undone six times.
    let avg_n = normalise(keep.iter().fold([0.0; 3], |a, k| add(a, k.1)));
    keep.into_iter().map(|(p, n, d)| (p, if avg_n == [0.0; 3] { n } else { normalise(add(n, avg_n)) }, d)).collect()
}

/// Rows of Studio's 3D layer rotation for X, Y, Z degrees (orientation
/// zero): the layer turn convention of `studio_render::space::rotation`.
pub fn studio_rotation_matrix(rx: f32, ry: f32, rz: f32) -> [V3; 3] {
    let rot_x = |d: f32| {
        let (s, c) = d.to_radians().sin_cos();
        [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
    };
    let rot_y = |d: f32| {
        let (s, c) = d.to_radians().sin_cos();
        [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
    };
    let rot_z = |d: f32| {
        let (s, c) = d.to_radians().sin_cos();
        [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
    };
    let mul = |a: [V3; 3], b: [V3; 3]| -> [V3; 3] { std::array::from_fn(|r| std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c])) };
    mul(mul(rot_x(rx), rot_y(ry)), rot_z(rz))
}

/// A body turn as a 3D layer's X, Y, Z degrees.
pub fn studio_angles(q: Quat) -> V3 {
    let m = q_to_mat(q);
    // m = Rx_std(−rx) · Ry_std(−ry) · Rz_std(rz); for Rx·Ry·Rz,
    // m[0][2] = sin b, a = atan2(−m[1][2], m[2][2]), c = atan2(−m[0][1], m[0][0]).
    let b = m[0][2].clamp(-1.0, 1.0).asin();
    let (a, c) = if m[0][2].abs() < 0.9999 { ((-m[1][2]).atan2(m[2][2]), (-m[0][1]).atan2(m[0][0])) } else { (m[2][1].atan2(m[1][1]), 0.0) };
    [-a.to_degrees(), -b.to_degrees(), c.to_degrees()]
}

/// A layer's X, Y, Z degrees as a body turn.
pub fn quat_from_studio(rx: f32, ry: f32, rz: f32) -> Quat {
    let m = studio_rotation_matrix(rx, ry, rz);
    studio_mesh::rig::q_from_axes([m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_body(size: f32, at: V3) -> Body {
        let mut b = clump(&studio_mesh::prim::cube([size; 3]), 6, 1.0);
        b.pos = add(b.pos, at);
        b
    }

    #[test]
    fn layer_angles_round_trip() {
        for a in [[10.0, 20.0, 30.0], [-40.0, 15.0, 100.0], [0.0, -60.0, 0.0]] {
            let q = quat_from_studio(a[0], a[1], a[2]);
            let back = studio_angles(q);
            let m1 = studio_rotation_matrix(a[0], a[1], a[2]);
            let m2 = studio_rotation_matrix(back[0], back[1], back[2]);
            for r in 0..3 {
                for c in 0..3 {
                    assert!((m1[r][c] - m2[r][c]).abs() < 1e-3, "{a:?} -> {back:?}");
                }
            }
            // The quaternion turns points as the layer matrix does.
            let p = [1.0, 2.0, 3.0];
            let via_m = [dot(m1[0], p), dot(m1[1], p), dot(m1[2], p)];
            assert!(length(sub(q_rotate(q, p), via_m)) < 1e-3);
        }
    }

    #[test]
    fn a_box_falls_bounces_and_rests_on_the_floor() {
        let mut w = World::new([0.0, 980.0, 0.0]);
        w.colliders.push(Collider::Ground { y: 300.0, friction: 0.6 });
        let b = cube_body(100.0, [0.0, 0.0, 0.0]);
        let sphere_r = b.spheres[0].1;
        w.bodies.push(b);
        let mut lowest = f32::NEG_INFINITY;
        for _ in 0..90 {
            w.advance(1.0 / 30.0);
            lowest = lowest.max(w.bodies[0].pos[1]);
        }
        let y = w.bodies[0].pos[1];
        // Resting with its bottom on the floor (spheres reach a little past
        // the faces), not sunk through it.
        assert!((y - (300.0 - 50.0)).abs() < sphere_r + 6.0, "y {y}");
        assert!(lowest < 300.0, "never through the floor: {lowest}");
        assert!(w.bodies[0].asleep, "comes to rest");
    }

    #[test]
    fn frames_that_are_not_whole_steps_keep_the_clock() {
        // 25 frames a second is 9.6 steps of 1/240 s each: rounding every
        // frame to 10 ran the world 4% fast.
        for fps in [25.0f32, 50.0, 29.97, 23.976, 30.0] {
            let mut w = World::new([0.0, 980.0, 0.0]);
            let frames = (fps * 4.0).round() as usize;
            for _ in 0..frames {
                w.advance(1.0 / fps);
            }
            let want = frames as f32 / fps;
            assert!((w.time - want).abs() <= w.step * 1.01, "{fps} fps: {} s stepped for {want} s", w.time);
        }
    }

    #[test]
    fn a_tower_stands_and_a_ball_knocks_it_over() {
        let mut w = World::new([0.0, 980.0, 0.0]);
        w.colliders.push(Collider::Ground { y: 200.0, friction: 0.7 });
        for k in 0..3 {
            w.bodies.push(cube_body(60.0, [0.0, 200.0 - 30.0 - 61.0 * k as f32, 0.0]));
        }
        w.advance(1.0);
        let top = w.bodies[2].pos;
        assert!((top[0]).abs() < 4.0 && top[1] < 200.0 - 120.0, "the stack stands: {top:?}");
        let mut ball = clump(&studio_mesh::prim::ico_sphere(25.0, 2), 6, 3.0);
        ball.pos = [-300.0, 200.0 - 100.0, 0.0];
        ball.vel = [900.0, 0.0, 0.0];
        w.bodies.push(ball);
        w.advance(2.0);
        let top = w.bodies[2].pos;
        assert!(top[0] > 40.0 || top[1] > 200.0 - 80.0, "knocked down: {top:?}");
    }
}
