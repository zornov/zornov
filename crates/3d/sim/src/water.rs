// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Water surfaces: a pond that ripples where drops fall and wakes trail
//! behind things moving through it (the wave equation on a height grid),
//! and an open sea that swells (a sum of Gerstner waves: crests sharpen and
//! the surface rolls, as real swell does). Both move the vertices of a
//! flat grid model frame by frame.

use crate::{V3, add, scale};
use studio_mesh::Mesh;

/// A flat grid `size` px square at height `y`, `cells` across, facing up.
pub fn surface(size: [f32; 2], cells: usize) -> Mesh {
    let n = cells.clamp(4, 400);
    let big = size[0].max(size[1]).max(1.0);
    let nx = ((size[0] / big * n as f32).round() as usize).max(2);
    let nz = ((size[1] / big * n as f32).round() as usize).max(2);
    let mut m = Mesh::default();
    for z in 0..=nz {
        for x in 0..=nx {
            let (u, v) = (x as f32 / nx as f32, z as f32 / nz as f32);
            m.positions.push([(u - 0.5) * size[0], 0.0, (v - 0.5) * size[1]]);
            m.normals.push([0.0, -1.0, 0.0]);
            m.uvs.push([u, v]);
        }
    }
    let at = |x: usize, z: usize| (z * (nx + 1) + x) as u32;
    for z in 0..nz {
        for x in 0..nx {
            // Wound so the faces look up (−y).
            m.indices.extend_from_slice(&[at(x, z), at(x + 1, z), at(x + 1, z + 1), at(x, z), at(x + 1, z + 1), at(x, z + 1)]);
        }
    }
    m.materials.push(studio_mesh::Material { name: "Water".into(), base_colour: [0.02, 0.07, 0.09, 1.0], roughness: 0.05, transmission: 0.9, ior: 1.33, ..Default::default() });
    m.single_group();
    m
}

/// One train of swell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swell {
    /// Direction it travels in the ground plane (x, z).
    pub direction: [f32; 2],
    /// Crest to crest (px).
    pub length: f32,
    /// Height of a crest above the mean (px).
    pub height: f32,
    /// 0 round – 1 sharp crests.
    pub steepness: f32,
}

/// A sea: several trains of swell, a wind sea from `wind_dir` with crests
/// up to `height` px over waves around `length` px long.
pub fn sea(wind_dir: [f32; 2], height: f32, length: f32, seed: u64) -> Vec<Swell> {
    let mut rng = crate::Rng::new(seed);
    let base = wind_dir[1].atan2(wind_dir[0]);
    (0..6)
        .map(|k| {
            let a = base + rng.range(-0.6, 0.6);
            let l = length * [1.0, 0.6, 0.37, 0.25, 0.16, 0.1][k] * rng.range(0.85, 1.15);
            Swell { direction: [a.cos(), a.sin()], length: l, height: height * [1.0, 0.55, 0.32, 0.2, 0.12, 0.07][k], steepness: 0.7 }
        })
        .collect()
}

/// Where a point of the still surface is at time `t` on a swelling sea
/// (deep-water speeds; y is down, so crests have smaller y).
pub fn swell_at(rest: V3, t: f32, waves: &[Swell], gravity: f32) -> V3 {
    let mut p = rest;
    let n = waves.len().max(1) as f32;
    for w in waves {
        let k = std::f32::consts::TAU / w.length.max(1.0);
        let c = (gravity / k).sqrt();
        let d = w.direction;
        let phase = k * (d[0] * rest[0] + d[1] * rest[2]) - k * c * t;
        let q = w.steepness / (k * w.height.max(1e-3) * n);
        let (s, cos) = phase.sin_cos();
        p[0] += q * w.height * d[0] * cos;
        p[2] += q * w.height * d[1] * cos;
        p[1] -= w.height * s;
    }
    p
}

/// A drop, splash or object touching a pond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Disturbance {
    pub at: [f32; 2],
    /// Where it moves to by `until` (a wake), or the same place (a drop).
    pub to: [f32; 2],
    pub from_t: f32,
    pub until: f32,
    pub radius: f32,
    /// How hard it pushes the surface down (px).
    pub depth: f32,
}

/// A rippling pond on a grid model made by [`surface`].
pub struct Pond {
    nx: usize,
    nz: usize,
    size: [f32; 2],
    h: Vec<f32>,
    v: Vec<f32>,
    /// Wave speed (px/s) and how fast ripples die away (per second).
    pub speed: f32,
    pub damping: f32,
}

impl Pond {
    pub fn new(mesh: &Mesh, size: [f32; 2]) -> Pond {
        // The grid's own counts, from its vertex layout.
        let row = mesh.positions.iter().take_while(|p| p[2] == mesh.positions[0][2]).count().max(2);
        let nz = mesh.positions.len() / row;
        Pond { nx: row, nz, size, h: vec![0.0; row * nz], v: vec![0.0; row * nz], speed: 250.0, damping: 0.6 }
    }

    pub fn step(&mut self, dt: f32, t: f32, touches: &[Disturbance]) {
        let dx = self.size[0] / (self.nx - 1) as f32;
        // Stable steps for the explicit scheme (CFL).
        let sub = ((self.speed * dt / dx) * 2.0).ceil().max(1.0) as usize;
        let h = dt / sub as f32;
        for k in 0..sub {
            let ts = t + h * k as f32;
            for d in touches {
                if ts < d.from_t || ts > d.until {
                    continue;
                }
                let u = if d.until > d.from_t { (ts - d.from_t) / (d.until - d.from_t) } else { 0.0 };
                let c = [d.at[0] + (d.to[0] - d.at[0]) * u, d.at[1] + (d.to[1] - d.at[1]) * u];
                for z in 0..self.nz {
                    for x in 0..self.nx {
                        let p = [(x as f32 / (self.nx - 1) as f32 - 0.5) * self.size[0], (z as f32 / (self.nz - 1) as f32 - 0.5) * self.size[1]];
                        let r = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
                        if r < d.radius {
                            let i = z * self.nx + x;
                            let target = d.depth * (1.0 - r / d.radius);
                            self.h[i] += (target - self.h[i]) * 0.3;
                        }
                    }
                }
            }
            let c2 = (self.speed * h / dx).powi(2);
            let mut acc = vec![0.0f32; self.h.len()];
            for z in 1..self.nz - 1 {
                for x in 1..self.nx - 1 {
                    let i = z * self.nx + x;
                    acc[i] = c2 / (h * h) * (self.h[i - 1] + self.h[i + 1] + self.h[i - self.nx] + self.h[i + self.nx] - 4.0 * self.h[i]);
                }
            }
            for i in 0..self.h.len() {
                self.v[i] = (self.v[i] + acc[i] * h) * (1.0 - self.damping * h);
                self.h[i] += self.v[i] * h;
            }
        }
    }

    /// The grid's vertices now (positive height pushes down: +y).
    pub fn vertices(&self, rest: &Mesh) -> Vec<V3> {
        rest.positions.iter().enumerate().map(|(i, p)| add(*p, scale([0.0, 1.0, 0.0], self.h.get(i).copied().unwrap_or(0.0)))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_makes_rings_that_spread_and_the_sea_swells() {
        let m = surface([400.0, 400.0], 80);
        let mut pond = Pond::new(&m, [400.0, 400.0]);
        let drop = Disturbance { at: [0.0, 0.0], to: [0.0, 0.0], from_t: 0.0, until: 0.05, radius: 20.0, depth: 15.0 };
        for f in 0..15 {
            pond.step(1.0 / 30.0, f as f32 / 30.0, &[drop]);
        }
        let v = pond.vertices(&m);
        // The ring has travelled out: something moves 100 px from the centre.
        let ring = v.iter().zip(&m.positions).filter(|(_, r)| ((r[0].powi(2) + r[2].powi(2)).sqrt() - 100.0).abs() < 10.0).map(|(p, r)| (p[1] - r[1]).abs()).fold(0.0, f32::max);
        assert!(ring > 0.05, "{ring}");
        let waves = sea([1.0, 0.0], 20.0, 300.0, 4);
        let a = swell_at([0.0; 3], 0.0, &waves, 980.0);
        let b = swell_at([0.0; 3], 1.0, &waves, 980.0);
        assert!((a[1] - b[1]).abs() > 1.0, "the surface moves");
        assert!(a[1].abs() < 60.0);
    }
}
