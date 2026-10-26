// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collision shapes: balls, capsules, convex hulls (boxes are hulls), a
//! fixed triangle surface for scenery and terrain, and the ground plane.
//! Each knows its bounds, its mass for a density and how a ray hits it.

use super::hull::Hull;
use super::math::*;
use std::sync::Arc;
use studio_mesh::bvh::Bvh;

/// Triangles that never move (a room, a terrain, a statue), with a tree
/// for finding the ones near a body.
#[derive(Clone, Debug)]
pub struct TriMesh {
    pub bvh: Bvh,
    pub normals: Vec<V3>,
}

impl TriMesh {
    pub fn new(tris: Vec<[V3; 3]>) -> TriMesh {
        let normals = tris.iter().map(|t| normalise_or(cross(sub(t[1], t[0]), sub(t[2], t[0])), [0.0, -1.0, 0.0])).collect();
        TriMesh { bvh: Bvh::new(tris), normals }
    }

    /// Triangles whose boxes meet `b` (in the mesh's frame).
    pub fn query(&self, b: &Aabb, out: &mut Vec<u32>) {
        out.clear();
        if self.bvh.tris.is_empty() {
            return;
        }
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.bvh.nodes[i as usize];
            if (0..3).any(|k| node.lo[k] > b.hi[k] || node.hi[k] < b.lo[k]) {
                continue;
            }
            if node.count > 0 {
                for k in node.index..node.index + node.count {
                    out.push(self.bvh.order[k as usize]);
                }
            } else {
                stack.push(node.index);
                stack.push(i + 1);
            }
        }
        out.sort_unstable();
    }

    pub fn bounds(&self) -> Aabb {
        match self.bvh.nodes.first() {
            Some(n) if !self.bvh.tris.is_empty() => Aabb { lo: n.lo, hi: n.hi },
            _ => Aabb::EMPTY,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Sphere { r: f32 },
    /// A rounded stick along the local y axis: centres of the end balls at
    /// ±`half`.
    Capsule { half: f32, r: f32 },
    Hull(Arc<Hull>),
    /// Fixed scenery only.
    Mesh(Arc<TriMesh>),
    /// Everything on the far side of `dot(n, x) = d` is solid (a floor);
    /// local frame, `n` points out of the solid.
    Plane { n: V3, d: f32 },
}

/// Mass, centre and inertia of a shape at a density (the centre in the
/// shape's frame, the inertia about it).
#[derive(Clone, Copy, Debug)]
pub struct MassProps {
    pub mass: f32,
    pub com: V3,
    pub inertia: M3,
    pub volume: f32,
}

impl Shape {
    pub fn cuboid(half: V3) -> Shape {
        Shape::Hull(Arc::new(Hull::cuboid(half)))
    }

    pub fn is_static_only(&self) -> bool {
        matches!(self, Shape::Mesh(_) | Shape::Plane { .. })
    }

    pub fn volume(&self) -> f32 {
        match self {
            Shape::Sphere { r } => 4.0 / 3.0 * std::f32::consts::PI * r * r * r,
            Shape::Capsule { half, r } => std::f32::consts::PI * r * r * (2.0 * half + 4.0 / 3.0 * r),
            Shape::Hull(h) => h.volume(),
            _ => 0.0,
        }
    }

    pub fn mass(&self, density: f32) -> MassProps {
        use std::f32::consts::PI;
        match self {
            Shape::Sphere { r } => {
                let v = self.volume();
                let m = v * density;
                MassProps { mass: m, com: [0.0; 3], inertia: m_diag([0.4 * m * r * r; 3]), volume: v }
            }
            Shape::Capsule { half, r } => {
                // A cylinder and two half balls.
                let h = 2.0 * half;
                let mc = density * PI * r * r * h;
                let mh = density * 2.0 / 3.0 * PI * r * r * r;
                let iy = mc * r * r / 2.0 + 2.0 * mh * 0.4 * r * r;
                let ix = mc * (h * h / 12.0 + r * r / 4.0) + 2.0 * mh * (0.4 * r * r + half * half + 3.0 * h / 8.0 * r);
                MassProps { mass: mc + 2.0 * mh, com: [0.0; 3], inertia: m_diag([ix, iy, ix]), volume: self.volume() }
            }
            Shape::Hull(h) => {
                let mo = h.moments();
                MassProps { mass: mo.volume * density, com: mo.com, inertia: mo.inertia(density), volume: mo.volume }
            }
            _ => MassProps { mass: 0.0, com: [0.0; 3], inertia: M_ZERO, volume: 0.0 },
        }
    }

    /// The bounds at a pose.
    pub fn bounds(&self, pose: &Pose) -> Aabb {
        match self {
            Shape::Sphere { r } => Aabb::around(pose.p, *r),
            Shape::Capsule { half, r } => {
                let a = pose.apply([0.0, *half, 0.0]);
                let b = pose.apply([0.0, -*half, 0.0]);
                Aabb { lo: vmin(a, b), hi: vmax(a, b) }.fattened(*r)
            }
            Shape::Hull(h) => {
                let mut b = Aabb::EMPTY;
                for v in &h.verts {
                    b.grow(pose.apply(*v));
                }
                b
            }
            Shape::Mesh(m) => {
                let lb = m.bounds();
                let mut b = Aabb::EMPTY;
                for i in 0..8 {
                    b.grow(pose.apply([if i & 1 == 0 { lb.lo[0] } else { lb.hi[0] }, if i & 2 == 0 { lb.lo[1] } else { lb.hi[1] }, if i & 4 == 0 { lb.lo[2] } else { lb.hi[2] }]));
                }
                b
            }
            Shape::Plane { .. } => Aabb { lo: [-1e9; 3], hi: [1e9; 3] },
        }
    }

    /// How far from its origin the shape reaches (for speed limits and
    /// sleeping).
    pub fn reach(&self) -> f32 {
        match self {
            Shape::Sphere { r } => *r,
            Shape::Capsule { half, r } => half + r,
            Shape::Hull(h) => h.verts.iter().map(|v| length(*v)).fold(0.0, f32::max),
            Shape::Mesh(m) => {
                let b = m.bounds();
                length(vmax(b.lo.map(f32::abs), b.hi.map(f32::abs)))
            }
            Shape::Plane { .. } => 0.0,
        }
    }

    /// The first hit of `o + t·d` (`d` unit) for `t` in 0..`max`: the
    /// distance and the surface normal, in the frame of `pose`'s parent.
    pub fn raycast(&self, pose: &Pose, o: V3, d: V3, max: f32) -> Option<(f32, V3)> {
        let lo = pose.unapply(o);
        let ld = pose.unrotate(d);
        let hit = match self {
            Shape::Sphere { r } => ray_sphere(lo, ld, [0.0; 3], *r).map(|t| (t, normalise(mad(lo, ld, t)))),
            Shape::Capsule { half, r } => ray_capsule(lo, ld, *half, *r),
            Shape::Hull(h) => ray_hull(h, lo, ld),
            Shape::Mesh(m) => m.bvh.ray(lo, ld, 0.0, max).map(|h| (h.t, m.normals[h.tri as usize])),
            Shape::Plane { n, d: off } => {
                let den = dot(*n, ld);
                let t = (off - dot(*n, lo)) / den;
                (den < -1e-9 && t >= 0.0).then_some((t, *n))
            }
        }?;
        (hit.0 >= 0.0 && hit.0 <= max).then(|| (hit.0, pose.rotate(hit.1)))
    }
}

fn ray_sphere(o: V3, d: V3, c: V3, r: f32) -> Option<f32> {
    let m = sub(o, c);
    let b = dot(m, d);
    let cc = dot(m, m) - r * r;
    if cc > 0.0 && b > 0.0 {
        return None;
    }
    let disc = b * b - cc;
    if disc < 0.0 {
        return None;
    }
    Some((-b - disc.sqrt()).max(0.0))
}

fn ray_capsule(o: V3, d: V3, half: f32, r: f32) -> Option<(f32, V3)> {
    let mut best: Option<(f32, V3)> = None;
    let mut keep = |t: f32, n: V3| {
        if best.is_none_or(|b| t < b.0) {
            best = Some((t, n));
        }
    };
    // The side: a cylinder about y.
    let a = d[0] * d[0] + d[2] * d[2];
    if a > 1e-12 {
        let b = o[0] * d[0] + o[2] * d[2];
        let c = o[0] * o[0] + o[2] * o[2] - r * r;
        let disc = b * b - a * c;
        if disc >= 0.0 {
            let t = (-b - disc.sqrt()) / a;
            let y = o[1] + t * d[1];
            if t >= 0.0 && y.abs() <= half {
                let p = mad(o, d, t);
                keep(t, normalise([p[0], 0.0, p[2]]));
            }
        }
    }
    for cy in [half, -half] {
        if let Some(t) = ray_sphere(o, d, [0.0, cy, 0.0], r) {
            let p = mad(o, d, t);
            keep(t, normalise(sub(p, [0.0, cy, 0.0])));
        }
    }
    best
}

fn ray_hull(h: &Hull, o: V3, d: V3) -> Option<(f32, V3)> {
    let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
    let mut n0 = [0.0, -1.0, 0.0];
    for f in &h.faces {
        let den = dot(f.n, d);
        let dist = f.d - dot(f.n, o);
        if den.abs() < 1e-12 {
            if dist < 0.0 {
                return None;
            }
            continue;
        }
        let t = dist / den;
        if den < 0.0 {
            if t > t0 {
                t0 = t;
                n0 = f.n;
            }
        } else {
            t1 = t1.min(t);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, n0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_each_shape_on_its_near_side() {
        let pose = Pose::at([0.0, 0.0, 5.0]);
        for s in [Shape::Sphere { r: 1.0 }, Shape::Capsule { half: 1.0, r: 1.0 }, Shape::cuboid([1.0; 3])] {
            let (t, n) = s.raycast(&pose, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 100.0).unwrap();
            assert!((t - 4.0).abs() < 1e-3, "{s:?} {t}");
            assert!(n[2] < -0.99, "{n:?}");
        }
        assert!(Shape::Sphere { r: 1.0 }.raycast(&pose, [3.0, 0.0, 0.0], [0.0, 0.0, 1.0], 100.0).is_none());
    }

    #[test]
    fn capsule_mass_is_a_cylinder_and_a_ball() {
        let c = Shape::Capsule { half: 1.0, r: 0.5 };
        let m = c.mass(1000.0);
        let v = std::f32::consts::PI * 0.25 * 2.0 + 4.0 / 3.0 * std::f32::consts::PI * 0.125;
        assert!((m.mass - v * 1000.0).abs() < 1.0);
        assert!(m.inertia[0][0] > m.inertia[1][1], "harder to tumble end over end than to spin on its length");
    }
}
