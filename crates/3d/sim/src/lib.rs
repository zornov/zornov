// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Physics for Studio's 3D, written from the textbook methods (no engine's
//! code): each simulation runs once and bakes what happened, frame by
//! frame, into a cache ([`studio_mesh::cache`]) or into keyframes, so it
//! plays, scrubs and renders like any other animation.
//!
//! - [`rigid`]: bodies that fall, tumble, bounce, stack and knock each
//!   other over (impulses between clumps of spheres filling each shape);
//! - [`fracture`]: a model shattered into pieces (Voronoi cells cut from
//!   it), which fly apart at a moment or when something hits it;
//! - [`cloth`]: capes, flags, curtains, tablecloths (position-based
//!   dynamics: stretch and bend constraints, pins, wind, collisions);
//! - [`soft`]: jelly, rubber, squash and stretch (shape matching);
//! - [`hair`]: strands grown from a surface that swing and settle;
//! - [`particles`]: sparks, rain, snow, dust, confetti, embers, sprays
//!   that collide with 3D models;
//! - [`smoke`]: smoke and fire on a grid (stable fluids: advection,
//!   buoyancy, vorticity, pressure projection);
//! - [`water`]: water surfaces: ripples, wakes and ocean swell.
//!
//! Units are Studio's world: pixels, y down, z away; time in seconds.

pub mod cloth;
pub mod dynamics;
pub mod fracture;
pub mod hair;
pub mod particles;
pub mod rigid;
pub mod smoke;
pub mod soft;
pub mod water;

use std::sync::Arc;
use studio_mesh::bvh::Bvh;
pub use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// Something solid things bump into.
#[derive(Clone, Debug)]
pub enum Collider {
    /// The floor: everything stays above `y` (y is down, so "above" is
    /// smaller y).
    Ground { y: f32, friction: f32 },
    Sphere { centre: V3, radius: f32 },
    /// A rounded stick from `a` to `b` (limbs of a character).
    Capsule { a: V3, b: V3, radius: f32 },
    /// A model's surface (its triangles in the world).
    Mesh { bvh: Arc<Bvh>, normals: Arc<Vec<V3>> },
}

/// A point pushed out of a collider: where it goes and the surface normal.
pub struct Contact {
    pub point: V3,
    pub normal: V3,
    pub friction: f32,
}

impl Collider {
    /// A mesh collider from triangles already in the world.
    pub fn mesh(tris: Vec<[V3; 3]>) -> Collider {
        let normals = tris.iter().map(|t| normalise(cross(sub(t[1], t[0]), sub(t[2], t[0])))).collect();
        Collider::Mesh { bvh: Arc::new(Bvh::new(tris)), normals: Arc::new(normals) }
    }

    /// If a ball of radius `r` at `p` is inside, where it should be.
    pub fn push_out(&self, p: V3, r: f32) -> Option<Contact> {
        match self {
            Collider::Ground { y, friction } => (p[1] + r > *y).then(|| Contact { point: [p[0], *y - r, p[2]], normal: [0.0, -1.0, 0.0], friction: *friction }),
            Collider::Sphere { centre, radius } => {
                let d = sub(p, *centre);
                let l = length(d);
                let reach = radius + r;
                (l < reach).then(|| {
                    let n = if l > 1e-6 { scale(d, 1.0 / l) } else { [0.0, -1.0, 0.0] };
                    Contact { point: add(*centre, scale(n, reach)), normal: n, friction: 0.4 }
                })
            }
            Collider::Capsule { a, b, radius } => {
                let q = studio_mesh::autorig::closest_on_segment(*a, *b, p);
                let d = sub(p, q);
                let l = length(d);
                let reach = radius + r;
                (l < reach).then(|| {
                    let n = if l > 1e-6 { scale(d, 1.0 / l) } else { [0.0, -1.0, 0.0] };
                    Contact { point: add(q, scale(n, reach)), normal: n, friction: 0.4 }
                })
            }
            Collider::Mesh { bvh, normals } => {
                let (q, tri, dist) = bvh.nearest(p, r.max(1e-3) * 1.0)?;
                let face = normals[tri as usize];
                let d = sub(p, q);
                // Behind the face: pushed back out the front.
                let n = if dot(d, face) < 0.0 || dist < 1e-6 { face } else { scale(d, 1.0 / dist) };
                Some(Contact { point: add(q, scale(n, r)), normal: n, friction: 0.5 })
            }
        }
    }
}

/// Smooth 3D noise in -1..1 (the mesh crate's fractal noise).
pub fn noise(p: V3, seed: u32) -> f32 {
    studio_mesh::ops::fbm(p, 3, seed)
}

/// A swirling, divergence-free flow (curl of a noise field), for wind
/// gusts and turbulence; `scale_px` is the size of the swirls.
pub fn curl(p: V3, scale_px: f32, seed: u32) -> V3 {
    let q = scale(p, 1.0 / scale_px.max(1e-3));
    let e = 0.05;
    let f = |v: V3, s: u32| noise(v, seed.wrapping_add(s));
    let d = |s: u32, axis: usize| {
        let mut a = q;
        let mut b = q;
        a[axis] += e;
        b[axis] -= e;
        (f(a, s) - f(b, s)) / (2.0 * e)
    };
    // Three potentials, one per axis.
    [d(2, 1) - d(1, 2), d(0, 2) - d(2, 0), d(1, 0) - d(0, 1)]
}

/// A small deterministic random number source.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
    /// A random direction.
    pub fn dir(&mut self) -> V3 {
        loop {
            let v = [self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0)];
            let l = length(v);
            if l > 0.05 && l <= 1.0 {
                return scale(v, 1.0 / l);
            }
        }
    }
    /// A random point in the unit ball.
    pub fn ball(&mut self) -> V3 {
        scale(self.dir(), self.next().cbrt())
    }
}
