// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Vectors, boxes, matrices and view frusta for the world.
//!
//! World space is in metres: x to the right (east), y up, z forward
//! (north). Studio's older 3D layers use pixels with y down; the
//! conversions live at the edges ([`from_studio`], [`to_studio`]).

pub type V3 = [f32; 3];
/// Column-major 4x4: `m[column][row]`.
pub type Mat4 = [[f32; 4]; 4];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-20 { [0.0, 1.0, 0.0] } else { mul(a, 1.0 / l) }
}
pub fn lerp(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
pub fn dist(a: V3, b: V3) -> f32 {
    len(sub(a, b))
}
/// Horizontal distance.
pub fn dist_xz(a: V3, b: V3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Wraps degrees into -180..180.
pub fn wrap_deg(d: f32) -> f32 {
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w == -180.0 { 180.0 } else { w }
}

/// Studio's pixel world (y down) to the world's metres (y up), at
/// `metres_per_unit` metres per unit.
pub fn from_studio(p: V3, metres_per_unit: f32) -> V3 {
    [p[0] * metres_per_unit, -p[1] * metres_per_unit, p[2] * metres_per_unit]
}
pub fn to_studio(p: V3, metres_per_unit: f32) -> V3 {
    let k = 1.0 / metres_per_unit.max(1e-9);
    [p[0] * k, -p[1] * k, p[2] * k]
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: V3,
    pub max: V3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3] };

    pub fn new(min: V3, max: V3) -> Aabb {
        Aabb { min, max }
    }
    pub fn around(c: V3, half: V3) -> Aabb {
        Aabb { min: sub(c, half), max: add(c, half) }
    }
    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
    pub fn grow(&mut self, p: V3) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: std::array::from_fn(|i| self.min[i].min(o.min[i])), max: std::array::from_fn(|i| self.max[i].max(o.max[i])) }
    }
    pub fn centre(&self) -> V3 {
        mul(add(self.min, self.max), 0.5)
    }
    pub fn half(&self) -> V3 {
        mul(sub(self.max, self.min), 0.5)
    }
    pub fn radius(&self) -> f32 {
        len(self.half())
    }
    pub fn contains(&self, p: V3) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
    pub fn overlaps(&self, o: &Aabb) -> bool {
        (0..3).all(|i| self.min[i] <= o.max[i] && self.max[i] >= o.min[i])
    }
    /// The eight corners.
    pub fn corners(&self) -> [V3; 8] {
        std::array::from_fn(|i| [if i & 1 == 0 { self.min[0] } else { self.max[0] }, if i & 2 == 0 { self.min[1] } else { self.max[1] }, if i & 4 == 0 { self.min[2] } else { self.max[2] }])
    }
    /// The box of this box after a placement (yaw about y, uniform scale).
    pub fn placed(&self, at: V3, yaw_deg: f32, scale: f32) -> Aabb {
        let mut b = Aabb::EMPTY;
        let (s, c) = yaw_deg.to_radians().sin_cos();
        for p in self.corners() {
            let p = mul(p, scale);
            b.grow(add(at, [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]));
        }
        b
    }
    /// Ray hit distance, if any (slab test).
    pub fn ray(&self, o: V3, d: V3, max: f32) -> Option<f32> {
        let (mut t0, mut t1) = (0.0f32, max);
        for i in 0..3 {
            if d[i].abs() < 1e-12 {
                if o[i] < self.min[i] || o[i] > self.max[i] {
                    return None;
                }
            } else {
                let inv = 1.0 / d[i];
                let (mut a, mut b) = ((self.min[i] - o[i]) * inv, (self.max[i] - o[i]) * inv);
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                t0 = t0.max(a);
                t1 = t1.min(b);
                if t0 > t1 {
                    return None;
                }
            }
        }
        Some(t0)
    }
}

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}
pub fn mat_vec(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|r| (0..4).map(|k| m[k][r] * v[k]).sum())
}
pub const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// A view from `eye` along `forward` with `up` roughly up (left-handed,
/// +z forward in view space).
pub fn look_to(eye: V3, forward: V3, up: V3) -> Mat4 {
    let f = norm(forward);
    let mut r = cross(up, f);
    if len(r) < 1e-6 {
        r = cross([0.0, 0.0, 1.0], f);
    }
    let r = norm(r);
    let u = cross(f, r);
    [[r[0], u[0], f[0], 0.0], [r[1], u[1], f[1], 0.0], [r[2], u[2], f[2], 0.0], [-dot(r, eye), -dot(u, eye), -dot(f, eye), 1.0]]
}

/// Perspective with depth 0 at `near` and 1 at `far` (WebGPU clip space);
/// `fov_y` in radians.
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    let q = far / (far - near);
    [[f / aspect, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [0.0, 0.0, q, 1.0], [0.0, 0.0, -near * q, 0.0]]
}

/// Orthographic box in view space, depth 0..1.
pub fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Mat4 {
    [[2.0 / (r - l), 0.0, 0.0, 0.0], [0.0, 2.0 / (t - b), 0.0, 0.0], [0.0, 0.0, 1.0 / (f - n), 0.0], [-(r + l) / (r - l), -(t + b) / (t - b), -n / (f - n), 1.0]]
}

/// A plane `dot(n, p) + d >= 0` on the inside.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub n: V3,
    pub d: f32,
}

/// The six planes of a view volume, normals pointing in.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    /// From a view-projection matrix (depth 0..1).
    pub fn from_view_proj(m: &Mat4) -> Frustum {
        let row = |r: usize| [m[0][r], m[1][r], m[2][r], m[3][r]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let mk = |v: [f32; 4]| {
            let l = len([v[0], v[1], v[2]]).max(1e-20);
            Plane { n: [v[0] / l, v[1] / l, v[2] / l], d: v[3] / l }
        };
        let add4 = |a: [f32; 4], b: [f32; 4], s: f32| [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2], a[3] + s * b[3]];
        Frustum { planes: [mk(add4(r3, r0, 1.0)), mk(add4(r3, r0, -1.0)), mk(add4(r3, r1, 1.0)), mk(add4(r3, r1, -1.0)), mk(r2), mk(add4(r3, r2, -1.0))] }
    }

    pub fn sphere(&self, c: V3, r: f32) -> bool {
        self.planes.iter().all(|p| dot(p.n, c) + p.d >= -r)
    }

    pub fn aabb(&self, b: &Aabb) -> bool {
        let c = b.centre();
        let h = b.half();
        self.planes.iter().all(|p| {
            let r = h[0] * p.n[0].abs() + h[1] * p.n[1].abs() + h[2] * p.n[2].abs();
            dot(p.n, c) + p.d >= -r
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frustum_keeps_what_is_in_front_and_drops_the_rest() {
        let view = look_to([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
        let proj = perspective(60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let f = Frustum::from_view_proj(&mat_mul(&proj, &view));
        assert!(f.sphere([0.0, 0.0, 10.0], 1.0));
        assert!(!f.sphere([0.0, 0.0, -10.0], 1.0), "behind");
        assert!(!f.sphere([0.0, 0.0, 200.0], 1.0), "beyond far");
        assert!(!f.sphere([50.0, 0.0, 10.0], 1.0), "far to the side");
        assert!(f.aabb(&Aabb::around([0.0, 0.0, 5.0], [1.0; 3])));
        assert!(!f.aabb(&Aabb::around([0.0, 40.0, 5.0], [1.0; 3])));
    }

    #[test]
    fn rays_hit_boxes_and_studio_coordinates_round_trip() {
        let b = Aabb::new([-1.0; 3], [1.0; 3]);
        assert_eq!(b.ray([0.0, 0.0, -5.0], [0.0, 0.0, 1.0], 100.0), Some(4.0));
        assert!(b.ray([0.0, 3.0, -5.0], [0.0, 0.0, 1.0], 100.0).is_none());
        let p = [3.0, -4.0, 5.0];
        let q = to_studio(from_studio(p, 0.01), 0.01);
        for i in 0..3 {
            assert!((p[i] - q[i]).abs() < 1e-4);
        }
        assert_eq!(wrap_deg(190.0), -170.0);
    }
}
