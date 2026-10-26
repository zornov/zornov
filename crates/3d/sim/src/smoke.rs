// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Smoke and fire on a grid: Stam's stable fluids ("Stable Fluids", 1999)
//! with heat. Each step sources add smoke, heat and a push; hot air rises
//! and heavy smoke sinks (buoyancy); vorticity confinement keeps the
//! curls lively; the flow carries itself (semi-Lagrangian advection) and
//! is made incompressible (a pressure solve); smoke and heat are carried
//! along and fade. Solid models in the box block the flow.
//!
//! Each frame is kept as a volume of density and heat (0–255 per cell)
//! that the renderer draws by marching rays: smoke scatters light, heat
//! glows as fire.

use crate::{V3, add, scale};
use studio_mesh::cache::{Cache, Frames};

/// A puff, chimney, torch or campfire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub centre: V3,
    pub radius: f32,
    /// Smoke added per second (cell densities).
    pub smoke: f32,
    /// Heat added per second (fire is hot).
    pub heat: f32,
    /// Push (px/s) given to the air there.
    pub velocity: V3,
    /// Seconds it runs from, and to.
    pub from: f32,
    pub to: f32,
}

/// How the air behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    /// How strongly heat lifts (px/s² per unit heat).
    pub buoyancy: f32,
    /// How strongly smoke sinks.
    pub weight: f32,
    /// Swirl strength.
    pub vorticity: f32,
    /// Fraction of smoke gone per second.
    pub fade: f32,
    /// Fraction of heat gone per second.
    pub cool: f32,
    pub wind: V3,
}

impl Default for Air {
    fn default() -> Air {
        Air { buoyancy: 900.0, weight: 8.0, vorticity: 2.0, fade: 0.15, cool: 0.6, wind: [0.0; 3] }
    }
}

/// The grid.
pub struct Grid {
    pub dims: [usize; 3],
    pub min: V3,
    pub cell: f32,
    vel: Vec<V3>,
    density: Vec<f32>,
    heat: Vec<f32>,
    solid: Vec<bool>,
}

impl Grid {
    /// A grid over the box `min..max` with about `resolution` cells along
    /// its longest side.
    pub fn new(min: V3, max: V3, resolution: usize) -> Grid {
        let ext: V3 = std::array::from_fn(|i| (max[i] - min[i]).max(1e-3));
        let big = ext[0].max(ext[1]).max(ext[2]);
        let cell = big / resolution.clamp(8, 256) as f32;
        let dims: [usize; 3] = std::array::from_fn(|i| ((ext[i] / cell).ceil() as usize).clamp(4, 256));
        let n = dims[0] * dims[1] * dims[2];
        Grid { dims, min, cell, vel: vec![[0.0; 3]; n], density: vec![0.0; n], heat: vec![0.0; n], solid: vec![false; n] }
    }

    pub fn max(&self) -> V3 {
        std::array::from_fn(|i| self.min[i] + self.dims[i] as f32 * self.cell)
    }

    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims[1] + y) * self.dims[0] + x
    }

    fn centre(&self, x: usize, y: usize, z: usize) -> V3 {
        [self.min[0] + (x as f32 + 0.5) * self.cell, self.min[1] + (y as f32 + 0.5) * self.cell, self.min[2] + (z as f32 + 0.5) * self.cell]
    }

    /// Marks cells inside `inside` as solid (models in the way).
    pub fn block(&mut self, inside: impl Fn(V3) -> bool) {
        for z in 0..self.dims[2] {
            for y in 0..self.dims[1] {
                for x in 0..self.dims[0] {
                    let c = self.centre(x, y, z);
                    if inside(c) {
                        let i = self.idx(x, y, z);
                        self.solid[i] = true;
                    }
                }
            }
        }
    }

    /// The eight cells around a grid-space point and their weights.
    fn weights(&self, g: V3) -> [(usize, f32); 8] {
        let [nx, ny, nz] = self.dims;
        let gx = g[0].clamp(0.0, nx as f32 - 1.001);
        let gy = g[1].clamp(0.0, ny as f32 - 1.001);
        let gz = g[2].clamp(0.0, nz as f32 - 1.001);
        let (x0, y0, z0) = (gx as usize, gy as usize, gz as usize);
        let (fx, fy, fz) = (gx - x0 as f32, gy - y0 as f32, gz - z0 as f32);
        let mut out = [(0usize, 0.0f32); 8];
        let mut k = 0;
        for (dz, wz) in [(0, 1.0 - fz), (1, fz)] {
            for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
                for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                    out[k] = (self.idx(x0 + dx, y0 + dy, z0 + dz), wx * wy * wz);
                    k += 1;
                }
            }
        }
        out
    }

    fn sample(&self, f: &[f32], g: V3) -> f32 {
        self.weights(g).iter().map(|&(i, w)| f[i] * w).sum()
    }

    fn sample_v(&self, f: &[V3], g: V3) -> V3 {
        self.weights(g).iter().fold([0.0; 3], |a, &(i, w)| add(a, scale(f[i], w)))
    }

    pub fn step(&mut self, dt: f32, sources: &[Source], air: &Air, t: f32) {
        let [nx, ny, nz] = self.dims;
        let h = self.cell;
        // Sources.
        for s in sources {
            if t < s.from || t > s.to {
                continue;
            }
            for z in 0..nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let c = self.centre(x, y, z);
                        let d = crate::length(crate::sub(c, s.centre));
                        if d > s.radius {
                            continue;
                        }
                        let w = 1.0 - d / s.radius;
                        let i = self.idx(x, y, z);
                        self.density[i] += s.smoke * w * dt;
                        self.heat[i] += s.heat * w * dt;
                        self.vel[i] = add(self.vel[i], scale(crate::sub(s.velocity, self.vel[i]), (w * 4.0 * dt).min(1.0)));
                    }
                }
            }
        }
        // Buoyancy (y is down: heat pushes to −y) and wind.
        for i in 0..self.vel.len() {
            let lift = air.buoyancy * self.heat[i] - air.weight * self.density[i];
            self.vel[i][1] -= lift * dt;
            if air.wind != [0.0; 3] {
                self.vel[i] = add(self.vel[i], scale(crate::sub(air.wind, self.vel[i]), (0.5 * dt).min(1.0)));
            }
        }
        self.confine(dt, air.vorticity);
        // Carry the flow by itself.
        let old_v = self.vel.clone();
        let back = |_g: &Grid, x: usize, y: usize, z: usize, v: V3| -> V3 { [x as f32 - v[0] * dt / h, y as f32 - v[1] * dt / h, z as f32 - v[2] * dt / h] };
        let mut nv = vec![[0.0f32; 3]; self.vel.len()];
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let i = self.idx(x, y, z);
                    let g = back(self, x, y, z, old_v[i]);
                    nv[i] = self.sample_v(&old_v, g);
                }
            }
        }
        self.vel = nv;
        self.project();
        // Smoke and heat carried along, fading.
        let (od, oh) = (self.density.clone(), self.heat.clone());
        let fade = (1.0 - air.fade * dt).max(0.0);
        let cool = (1.0 - air.cool * dt).max(0.0);
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let i = self.idx(x, y, z);
                    if self.solid[i] {
                        self.density[i] = 0.0;
                        self.heat[i] = 0.0;
                        continue;
                    }
                    let g = back(self, x, y, z, self.vel[i]);
                    self.density[i] = self.sample(&od, g) * fade;
                    self.heat[i] = self.sample(&oh, g) * cool;
                }
            }
        }
    }

    /// Vorticity confinement: puts back the small swirls the grid loses.
    fn confine(&mut self, dt: f32, eps: f32) {
        if eps <= 0.0 {
            return;
        }
        let [nx, ny, nz] = self.dims;
        let h = self.cell;
        let mut curl = vec![[0.0f32; 3]; self.vel.len()];
        let mut mag = vec![0.0f32; self.vel.len()];
        for z in 1..nz - 1 {
            for y in 1..ny - 1 {
                for x in 1..nx - 1 {
                    let v = |dx: i32, dy: i32, dz: i32| self.vel[self.idx((x as i32 + dx) as usize, (y as i32 + dy) as usize, (z as i32 + dz) as usize)];
                    let c = [
                        (v(0, 1, 0)[2] - v(0, -1, 0)[2] - v(0, 0, 1)[1] + v(0, 0, -1)[1]) / (2.0 * h),
                        (v(0, 0, 1)[0] - v(0, 0, -1)[0] - v(1, 0, 0)[2] + v(-1, 0, 0)[2]) / (2.0 * h),
                        (v(1, 0, 0)[1] - v(-1, 0, 0)[1] - v(0, 1, 0)[0] + v(0, -1, 0)[0]) / (2.0 * h),
                    ];
                    let i = self.idx(x, y, z);
                    curl[i] = c;
                    mag[i] = crate::length(c);
                }
            }
        }
        for z in 1..nz - 1 {
            for y in 1..ny - 1 {
                for x in 1..nx - 1 {
                    let m = |dx: i32, dy: i32, dz: i32| mag[self.idx((x as i32 + dx) as usize, (y as i32 + dy) as usize, (z as i32 + dz) as usize)];
                    let grad = [m(1, 0, 0) - m(-1, 0, 0), m(0, 1, 0) - m(0, -1, 0), m(0, 0, 1) - m(0, 0, -1)];
                    let l = crate::length(grad);
                    if l < 1e-9 {
                        continue;
                    }
                    let n = scale(grad, 1.0 / l);
                    let i = self.idx(x, y, z);
                    let f = scale(crate::cross(n, curl[i]), eps * h);
                    self.vel[i] = add(self.vel[i], scale(f, dt));
                }
            }
        }
    }

    /// Makes the flow incompressible: solves for pressure (Jacobi) and
    /// takes its gradient away. Walls and solids let nothing through.
    fn project(&mut self) {
        let [nx, ny, nz] = self.dims;
        let n = self.vel.len();
        let mut div = vec![0.0f32; n];
        let get = |g: &Grid, x: i64, y: i64, z: i64| -> V3 {
            if x < 0 || y < 0 || z < 0 || x >= nx as i64 || y >= ny as i64 || z >= nz as i64 {
                return [0.0; 3];
            }
            let i = g.idx(x as usize, y as usize, z as usize);
            if g.solid[i] { [0.0; 3] } else { g.vel[i] }
        };
        for z in 0..nz as i64 {
            for y in 0..ny as i64 {
                for x in 0..nx as i64 {
                    let i = self.idx(x as usize, y as usize, z as usize);
                    div[i] = 0.5 * (get(self, x + 1, y, z)[0] - get(self, x - 1, y, z)[0] + get(self, x, y + 1, z)[1] - get(self, x, y - 1, z)[1] + get(self, x, y, z + 1)[2] - get(self, x, y, z - 1)[2]);
                }
            }
        }
        let mut p = vec![0.0f32; n];
        let mut q = vec![0.0f32; n];
        for _ in 0..40 {
            for z in 0..nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let i = self.idx(x, y, z);
                        if self.solid[i] {
                            q[i] = 0.0;
                            continue;
                        }
                        let mut s = 0.0;
                        let mut k = 0.0;
                        for (dx, dy, dz) in [(1i64, 0i64, 0i64), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                            let (xx, yy, zz) = (x as i64 + dx, y as i64 + dy, z as i64 + dz);
                            if xx < 0 || yy < 0 || zz < 0 || xx >= nx as i64 || yy >= ny as i64 || zz >= nz as i64 {
                                // Open sides: pressure zero beyond.
                                k += 1.0;
                                continue;
                            }
                            let j = self.idx(xx as usize, yy as usize, zz as usize);
                            if self.solid[j] {
                                // Walls: same pressure as here (no flow).
                                continue;
                            }
                            s += p[j];
                            k += 1.0;
                        }
                        q[i] = if k > 0.0 { (s - div[i]) / k } else { 0.0 };
                    }
                }
            }
            std::mem::swap(&mut p, &mut q);
        }
        let pat = |g: &Grid, p: &[f32], x: i64, y: i64, z: i64, here: f32| -> f32 {
            if x < 0 || y < 0 || z < 0 || x >= nx as i64 || y >= ny as i64 || z >= nz as i64 {
                return 0.0;
            }
            let j = g.idx(x as usize, y as usize, z as usize);
            if g.solid[j] { here } else { p[j] }
        };
        for z in 0..nz as i64 {
            for y in 0..ny as i64 {
                for x in 0..nx as i64 {
                    let i = self.idx(x as usize, y as usize, z as usize);
                    if self.solid[i] {
                        self.vel[i] = [0.0; 3];
                        continue;
                    }
                    let here = p[i];
                    let g = [
                        0.5 * (pat(self, &p, x + 1, y, z, here) - pat(self, &p, x - 1, y, z, here)),
                        0.5 * (pat(self, &p, x, y + 1, z, here) - pat(self, &p, x, y - 1, z, here)),
                        0.5 * (pat(self, &p, x, y, z + 1, here) - pat(self, &p, x, y, z - 1, here)),
                    ];
                    self.vel[i] = crate::sub(self.vel[i], g);
                }
            }
        }
    }

    /// This moment as cells (density, heat) for a cache.
    pub fn snapshot(&self) -> Vec<[u8; 2]> {
        self.density.iter().zip(&self.heat).map(|(d, h)| [(d.clamp(0.0, 1.0) * 255.0) as u8, (h.clamp(0.0, 1.0) * 255.0) as u8]).collect()
    }

    pub fn total_smoke(&self) -> f32 {
        self.density.iter().sum()
    }

    /// The centre of the smoke (density-weighted), for tests and framing.
    pub fn smoke_centre(&self) -> V3 {
        let mut acc = [0.0f32; 3];
        let mut w = 0.0;
        for z in 0..self.dims[2] {
            for y in 0..self.dims[1] {
                for x in 0..self.dims[0] {
                    let d = self.density[self.idx(x, y, z)];
                    acc = add(acc, scale(self.centre(x, y, z), d));
                    w += d;
                }
            }
        }
        if w > 0.0 { scale(acc, 1.0 / w) } else { [0.0; 3] }
    }
}

/// Runs a grid for `seconds` at `fps` (two steps a frame) and bakes it.
pub fn bake(mut grid: Grid, sources: &[Source], air: &Air, seconds: f32, fps: f32) -> Cache {
    let frames = (seconds * fps).round().max(1.0) as usize + 1;
    let mut data = Vec::with_capacity(frames);
    for f in 0..frames {
        if f > 0 {
            let t = f as f32 / fps;
            for k in 0..2 {
                grid.step(0.5 / fps, sources, air, t - 0.5 / fps * (1 - k) as f32);
            }
        }
        data.push(grid.snapshot());
    }
    Cache { fps, frames, data: Frames::Volume { dims: grid.dims, min: grid.min, max: grid.max(), data } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_smoke_rises_and_a_ceiling_spreads_it() {
        let mut g = Grid::new([-100.0, -300.0, -100.0], [100.0, 100.0, 100.0], 24);
        let src = [Source { centre: [0.0, 60.0, 0.0], radius: 30.0, smoke: 1.0, heat: 6.0, velocity: [0.0, -60.0, 0.0], from: 0.0, to: 10.0 }];
        let air = Air::default();
        for f in 0..40 {
            g.step(1.0 / 30.0, &src, &air, f as f32 / 30.0);
        }
        // The plume's top has climbed well above the source (y up is −).
        let mut top = f32::INFINITY;
        for z in 0..g.dims[2] {
            for y in 0..g.dims[1] {
                for x in 0..g.dims[0] {
                    if g.density[g.idx(x, y, z)] > 0.01 {
                        top = top.min(g.centre(x, y, z)[1]);
                    }
                }
            }
        }
        assert!(top < -40.0, "rises over 100 px: top {top}");
        let c = g.smoke_centre();
        assert!(c[1] < 60.0, "{c:?}");
        assert!(g.total_smoke() > 1.0);
        let cache = bake(Grid::new([-50.0; 3], [50.0; 3], 12), &src, &air, 0.5, 10.0);
        assert_eq!(cache.frames, 6);
        assert!(matches!(cache.data, Frames::Volume { .. }));
    }
}
