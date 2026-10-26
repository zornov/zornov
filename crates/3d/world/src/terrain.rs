// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terrain as a heightfield: generated in styles (plains, hills,
//! mountains, dunes, canyon, island, valley, flat), sculpted with brushes,
//! painted in four layers (a weight per layer per sample), and cut into
//! square chunks that each come in several levels of detail with skirts,
//! so a big landscape draws only what is near in full.
//!
//! Heights are metres, up. The grid has `cells[0] x cells[1]` cells of
//! `cell` metres, `cells + 1` samples a side, its corner `origin` (x, z).
//! Each cell is split along the diagonal from its (x, z) corner to the
//! far one; [`Terrain::height_at`] follows the same triangles as the
//! drawn mesh, so a character stands exactly on what is drawn.

use crate::math::{Aabb, V3, smoothstep};
use crate::noise;

/// Cells along a chunk's side.
pub const CHUNK: usize = 32;
/// Cell steps of each level of detail.
pub const LOD_STRIDES: [usize; 4] = [1, 2, 4, 8];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Flat,
    Plains,
    Hills,
    Mountains,
    Dunes,
    Canyon,
    Island,
    Valley,
}

pub const STYLES: &[&str] = &["flat", "plains", "hills", "mountains", "dunes", "canyon", "island", "valley"];

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        Some(match s.trim().to_lowercase().as_str() {
            "flat" | "city" | "level" | "none" => Style::Flat,
            "plains" | "meadow" | "field" | "prairie" | "savanna" => Style::Plains,
            "hills" | "rolling" | "forest" | "woods" => Style::Hills,
            "mountains" | "mountain" | "alps" | "peaks" | "highlands" => Style::Mountains,
            "dunes" | "desert" | "sand" => Style::Dunes,
            "canyon" | "mesa" | "badlands" => Style::Canyon,
            "island" | "coast" | "beach" | "archipelago" => Style::Island,
            "valley" | "river" | "gorge" => Style::Valley,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        STYLES[self as usize]
    }
}

/// How to generate a landscape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSpec {
    /// Metres across (x) and deep (z).
    pub size: [f32; 2],
    /// Metres per cell (the size is rounded up to whole chunks).
    pub cell: f32,
    pub style: Style,
    /// Highest rise in metres.
    pub height: f32,
    pub seed: u32,
}

impl Default for TerrainSpec {
    fn default() -> TerrainSpec {
        TerrainSpec { size: [512.0, 512.0], cell: 1.0, style: Style::Hills, height: 40.0, seed: 1 }
    }
}

/// One paintable ground material.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Linear RGB.
    pub colour: V3,
    pub roughness: f32,
    /// Metres per repeat of the detail pattern.
    pub tile: f32,
}

pub fn default_layers(style: Style) -> [Layer; 4] {
    let l = |n: &str, c: V3, r: f32, t: f32| Layer { name: n.into(), colour: c, roughness: r, tile: t };
    match style {
        Style::Dunes => [l("sand", [0.52, 0.38, 0.2], 0.95, 6.0), l("rock", [0.2, 0.17, 0.14], 0.9, 8.0), l("dune", [0.62, 0.45, 0.24], 0.95, 4.0), l("dry", [0.4, 0.3, 0.2], 0.95, 5.0)],
        Style::Canyon => [l("dust", [0.4, 0.22, 0.12], 0.95, 6.0), l("rock", [0.3, 0.14, 0.08], 0.9, 8.0), l("sand", [0.55, 0.38, 0.22], 0.95, 5.0), l("scrub", [0.24, 0.2, 0.1], 0.9, 6.0)],
        _ => [l("grass", [0.085, 0.16, 0.045], 0.9, 3.0), l("rock", [0.2, 0.18, 0.16], 0.88, 8.0), l("dirt", [0.2, 0.13, 0.08], 0.95, 4.0), l("snow", [0.8, 0.82, 0.86], 0.6, 6.0)],
    }
}

/// A sculpting or painting brush shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Brush {
    Raise,
    Lower,
    Smooth,
    /// Towards the height under the brush's centre.
    Flatten,
    /// Towards a given height.
    Level(f32),
    /// Adds rough bumps.
    Noise,
}

impl Brush {
    pub fn parse(s: &str, level: Option<f32>) -> Option<Brush> {
        Some(match s {
            "raise" | "up" | "hill" => Brush::Raise,
            "lower" | "down" | "dig" | "carve" => Brush::Lower,
            "smooth" => Brush::Smooth,
            "flatten" | "flat" => Brush::Flatten,
            "level" => Brush::Level(level?),
            "noise" | "rough" => Brush::Noise,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Terrain {
    pub cell: f32,
    pub cells: [usize; 2],
    pub origin: [f32; 2],
    pub heights: Vec<f32>,
    /// Layer weights per sample, summing to 255.
    pub splat: Vec<[u8; 4]>,
    pub layers: [Layer; 4],
    pub seed: u32,
    /// Bumps on every edit; chunks have their own too.
    pub revision: u64,
    chunk_rev: Vec<u64>,
    /// Lowest and highest sample in each chunk.
    ranges: Vec<[f32; 2]>,
}

impl Terrain {
    /// A level plane of `size` metres at height 0, all grass.
    pub fn flat(size: [f32; 2], cell: f32) -> Terrain {
        let cell = cell.max(0.05);
        let cx = ((size[0] / cell / CHUNK as f32).ceil() as usize).max(1);
        let cz = ((size[1] / cell / CHUNK as f32).ceil() as usize).max(1);
        let cells = [cx * CHUNK, cz * CHUNK];
        let n = (cells[0] + 1) * (cells[1] + 1);
        let mut t = Terrain {
            cell,
            cells,
            origin: [-(cells[0] as f32) * cell * 0.5, -(cells[1] as f32) * cell * 0.5],
            heights: vec![0.0; n],
            splat: vec![[255, 0, 0, 0]; n],
            layers: default_layers(Style::Hills),
            seed: 1,
            revision: 1,
            chunk_rev: vec![1; cx * cz],
            ranges: vec![[0.0, 0.0]; cx * cz],
        };
        t.refresh_ranges();
        t
    }

    /// A landscape in a style, painted by slope and height.
    pub fn generate(spec: &TerrainSpec) -> Terrain {
        let mut t = Terrain::flat(spec.size, spec.cell);
        t.seed = spec.seed;
        t.layers = default_layers(spec.style);
        let (nx, nz) = (t.cells[0], t.cells[1]);
        let big = (nx.max(nz) as f32 * t.cell).max(1.0);
        let seed = spec.seed;
        for iz in 0..=nz {
            for ix in 0..=nx {
                let (x, z) = (t.origin[0] + ix as f32 * t.cell, t.origin[1] + iz as f32 * t.cell);
                t.heights[iz * (nx + 1) + ix] = style_height(spec.style, x, z, big, spec.height, seed, [nx as f32 * t.cell, nz as f32 * t.cell]);
            }
        }
        if matches!(spec.style, Style::Hills | Style::Mountains | Style::Canyon) {
            t.erode(12);
        }
        t.refresh_ranges();
        t.autopaint(spec.style);
        t.revision += 1;
        t
    }

    pub fn size(&self) -> [f32; 2] {
        [self.cells[0] as f32 * self.cell, self.cells[1] as f32 * self.cell]
    }
    pub fn chunks(&self) -> [usize; 2] {
        [self.cells[0] / CHUNK, self.cells[1] / CHUNK]
    }
    fn at(&self, ix: usize, iz: usize) -> usize {
        iz * (self.cells[0] + 1) + ix
    }
    pub fn contains(&self, x: f32, z: f32) -> bool {
        let s = self.size();
        x >= self.origin[0] && z >= self.origin[1] && x <= self.origin[0] + s[0] && z <= self.origin[1] + s[1]
    }
    /// Sample height by grid index (clamped).
    pub fn sample(&self, ix: i64, iz: i64) -> f32 {
        let ix = ix.clamp(0, self.cells[0] as i64) as usize;
        let iz = iz.clamp(0, self.cells[1] as i64) as usize;
        self.heights[self.at(ix, iz)]
    }

    /// Height at (x, z) on the drawn triangles; the edge height outside.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let gx = ((x - self.origin[0]) / self.cell).clamp(0.0, self.cells[0] as f32 - 1e-4);
        let gz = ((z - self.origin[1]) / self.cell).clamp(0.0, self.cells[1] as f32 - 1e-4);
        let (ix, iz) = (gx.floor() as usize, gz.floor() as usize);
        let (fx, fz) = (gx - ix as f32, gz - iz as f32);
        let ha = self.heights[self.at(ix, iz)];
        let hb = self.heights[self.at(ix + 1, iz)];
        let hc = self.heights[self.at(ix, iz + 1)];
        let hd = self.heights[self.at(ix + 1, iz + 1)];
        if fx >= fz { ha + fx * (hb - ha) + fz * (hd - hb) } else { ha + fz * (hc - ha) + fx * (hd - hc) }
    }

    /// Surface normal (unit, up) from the full-resolution grid.
    pub fn normal_at(&self, x: f32, z: f32) -> V3 {
        let e = self.cell;
        let dx = self.height_at(x + e, z) - self.height_at(x - e, z);
        let dz = self.height_at(x, z + e) - self.height_at(x, z - e);
        crate::math::norm([-dx / (2.0 * e), 1.0, -dz / (2.0 * e)])
    }

    /// Steepness in degrees.
    pub fn slope_at(&self, x: f32, z: f32) -> f32 {
        self.normal_at(x, z)[1].clamp(-1.0, 1.0).acos().to_degrees()
    }

    pub fn bounds(&self) -> Aabb {
        let s = self.size();
        let (lo, hi) = self.heights.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |a, h| (a.0.min(*h), a.1.max(*h)));
        Aabb::new([self.origin[0], lo, self.origin[1]], [self.origin[0] + s[0], hi, self.origin[1] + s[1]])
    }

    /// Where a ray first meets the ground (distance along `dir`), by
    /// marching then bisecting.
    pub fn raycast(&self, o: V3, dir: V3, max: f32) -> Option<f32> {
        let d = crate::math::norm(dir);
        let step = (self.cell * 0.5).max(0.05);
        let mut t = 0.0;
        let mut prev_above = o[1] >= self.height_at(o[0], o[2]);
        if !prev_above {
            return Some(0.0);
        }
        while t < max {
            let nt = (t + step).min(max);
            let p = [o[0] + d[0] * nt, o[1] + d[1] * nt, o[2] + d[2] * nt];
            let above = !self.contains(p[0], p[2]) || p[1] >= self.height_at(p[0], p[2]);
            if prev_above && !above {
                let (mut lo, mut hi) = (t, nt);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    let q = [o[0] + d[0] * mid, o[1] + d[1] * mid, o[2] + d[2] * mid];
                    if q[1] >= self.height_at(q[0], q[2]) { lo = mid } else { hi = mid }
                }
                return Some(hi);
            }
            prev_above = above;
            t = nt;
            if nt >= max {
                break;
            }
        }
        None
    }

    fn refresh_ranges(&mut self) {
        let [cx, cz] = self.chunks();
        self.ranges.resize(cx * cz, [0.0, 0.0]);
        for j in 0..cz {
            for i in 0..cx {
                self.refresh_range(i, j);
            }
        }
    }

    fn refresh_range(&mut self, i: usize, j: usize) {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for iz in j * CHUNK..=(j + 1) * CHUNK {
            for ix in i * CHUNK..=(i + 1) * CHUNK {
                let h = self.heights[self.at(ix, iz)];
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        let k = j * self.chunks()[0] + i;
        self.ranges[k] = [lo, hi];
    }

    /// Mark the samples in a grid rectangle changed.
    fn touch(&mut self, x0: usize, z0: usize, x1: usize, z1: usize) {
        self.revision += 1;
        let [cx, cz] = self.chunks();
        let (i0, i1) = (x0.saturating_sub(1) / CHUNK, (x1 / CHUNK).min(cx - 1));
        let (j0, j1) = (z0.saturating_sub(1) / CHUNK, (z1 / CHUNK).min(cz - 1));
        for j in j0..=j1 {
            for i in i0..=i1 {
                self.refresh_range(i, j);
                let k = j * cx + i;
                self.chunk_rev[k] = self.revision;
            }
        }
    }

    pub fn chunk_revision(&self, i: usize, j: usize) -> u64 {
        self.chunk_rev[j * self.chunks()[0] + i]
    }

    pub fn chunk_bounds(&self, i: usize, j: usize) -> Aabb {
        let r = self.ranges[j * self.chunks()[0] + i];
        let cs = CHUNK as f32 * self.cell;
        let (x, z) = (self.origin[0] + i as f32 * cs, self.origin[1] + j as f32 * cs);
        Aabb::new([x, r[0], z], [x + cs, r[1], z + cs])
    }

    /// The level of detail for a chunk at `dist` metres, given the
    /// distances where each coarser level starts.
    pub fn lod_for(&self, dist: f32, starts: &[f32; 3]) -> usize {
        starts.iter().filter(|s| dist >= **s).count()
    }

    /// Grid rectangle touched by a disc, clamped.
    fn disc(&self, c: [f32; 2], r: f32) -> (usize, usize, usize, usize) {
        let to = |v: f32, o: f32, n: usize, lo: bool| {
            let g = (v - o) / self.cell;
            (if lo { g.floor() } else { g.ceil() }).clamp(0.0, n as f32) as usize
        };
        (to(c[0] - r, self.origin[0], self.cells[0], true), to(c[1] - r, self.origin[1], self.cells[1], true), to(c[0] + r, self.origin[0], self.cells[0], false), to(c[1] + r, self.origin[1], self.cells[1], false))
    }

    /// Sculpts under a round brush: `strength` is metres per stroke at the
    /// centre (a fraction of the way for smooth/flatten/level).
    pub fn sculpt(&mut self, brush: Brush, centre: [f32; 2], radius: f32, strength: f32) {
        let radius = radius.max(self.cell);
        let (x0, z0, x1, z1) = self.disc(centre, radius);
        let target = match brush {
            Brush::Flatten => self.height_at(centre[0], centre[1]),
            Brush::Level(h) => h,
            _ => 0.0,
        };
        let src = self.heights.clone();
        let nx = self.cells[0];
        for iz in z0..=z1 {
            for ix in x0..=x1 {
                let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
                let d = ((x - centre[0]).powi(2) + (z - centre[1]).powi(2)).sqrt() / radius;
                if d >= 1.0 {
                    continue;
                }
                let w = (1.0 - d * d).powi(2);
                let i = iz * (nx + 1) + ix;
                let h = src[i];
                self.heights[i] = match brush {
                    Brush::Raise => h + strength * w,
                    Brush::Lower => h - strength * w,
                    Brush::Smooth => {
                        let mut s = 0.0;
                        for (dx, dz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                            let (jx, jz) = ((ix as i64 + dx).clamp(0, nx as i64) as usize, (iz as i64 + dz).clamp(0, self.cells[1] as i64) as usize);
                            s += src[jz * (nx + 1) + jx];
                        }
                        let avg = (s + h) / 9.0;
                        h + (avg - h) * (strength.abs().min(1.0) * w)
                    }
                    Brush::Flatten | Brush::Level(_) => h + (target - h) * (strength.abs().min(1.0) * w).min(1.0),
                    Brush::Noise => h + strength * w * noise::fbm(x * 0.7, z * 0.7, 3, self.seed ^ 0x51),
                };
            }
        }
        self.touch(x0, z0, x1, z1);
    }

    /// Flattens a round pad to `height` with a soft skirt `blend` metres
    /// wide (for buildings, roads, camps).
    pub fn pad(&mut self, centre: [f32; 2], radius: f32, blend: f32, height: f32) {
        let (x0, z0, x1, z1) = self.disc(centre, radius + blend);
        let nx = self.cells[0];
        for iz in z0..=z1 {
            for ix in x0..=x1 {
                let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
                let d = ((x - centre[0]).powi(2) + (z - centre[1]).powi(2)).sqrt();
                let k = 1.0 - smoothstep(radius, radius + blend.max(1e-3), d);
                let i = iz * (nx + 1) + ix;
                self.heights[i] += (height - self.heights[i]) * k;
            }
        }
        self.touch(x0, z0, x1, z1);
    }

    /// A rectangle pad (axis aligned) with soft edges.
    pub fn pad_rect(&mut self, min: [f32; 2], max: [f32; 2], blend: f32, height: f32) {
        let (x0, z0, x1, z1) = self.disc([(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5], ((max[0] - min[0]).max(max[1] - min[1])) * 0.71 + blend);
        let nx = self.cells[0];
        for iz in z0..=z1 {
            for ix in x0..=x1 {
                let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
                let dx = (min[0] - x).max(x - max[0]).max(0.0);
                let dz = (min[1] - z).max(z - max[1]).max(0.0);
                let d = (dx * dx + dz * dz).sqrt();
                let k = 1.0 - smoothstep(0.0, blend.max(1e-3), d);
                let i = iz * (nx + 1) + ix;
                self.heights[i] += (height - self.heights[i]) * k;
            }
        }
        self.touch(x0, z0, x1, z1);
    }

    /// Digs a round basin (a pond or lake bed).
    pub fn basin(&mut self, centre: [f32; 2], radius: f32, depth: f32) {
        let (x0, z0, x1, z1) = self.disc(centre, radius * 1.4);
        let nx = self.cells[0];
        for iz in z0..=z1 {
            for ix in x0..=x1 {
                let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
                let d = ((x - centre[0]).powi(2) + (z - centre[1]).powi(2)).sqrt() / radius;
                if d < 1.4 {
                    let k = (1.0 - smoothstep(0.0, 1.4, d)).powf(1.5);
                    self.heights[iz * (nx + 1) + ix] -= depth * k;
                }
            }
        }
        self.touch(x0, z0, x1, z1);
    }

    /// Paints layer `layer` under a round brush; others give way.
    pub fn paint(&mut self, layer: usize, centre: [f32; 2], radius: f32, strength: f32) {
        let layer = layer.min(3);
        let radius = radius.max(self.cell);
        let (x0, z0, x1, z1) = self.disc(centre, radius);
        let nx = self.cells[0];
        for iz in z0..=z1 {
            for ix in x0..=x1 {
                let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
                let d = ((x - centre[0]).powi(2) + (z - centre[1]).powi(2)).sqrt() / radius;
                if d >= 1.0 {
                    continue;
                }
                let k = ((1.0 - d * d) * strength).clamp(0.0, 1.0);
                let s = &mut self.splat[iz * (nx + 1) + ix];
                let mut w = s.map(|v| v as f32 / 255.0);
                for (i, v) in w.iter_mut().enumerate() {
                    *v = if i == layer { *v + (1.0 - *v) * k } else { *v * (1.0 - k) };
                }
                let sum: f32 = w.iter().sum();
                let mut out = [0u8; 4];
                let mut used = 0i32;
                for i in 0..4 {
                    out[i] = (w[i] / sum.max(1e-6) * 255.0).round() as u8;
                    used += out[i] as i32;
                }
                // Rounding goes to the strongest layer.
                let big = (0..4).max_by(|a, b| out[*a].cmp(&out[*b])).unwrap_or(0);
                out[big] = (out[big] as i32 + 255 - used).clamp(0, 255) as u8;
                *s = out;
            }
        }
        self.touch(x0, z0, x1, z1);
    }

    /// Thermal erosion: steep faces crumble towards their feet.
    pub fn erode(&mut self, iterations: usize) {
        let (nx, nz) = (self.cells[0], self.cells[1]);
        let talus = self.cell * 0.9;
        let row = nx + 1;
        for _ in 0..iterations {
            let src = self.heights.clone();
            for iz in 1..nz {
                for ix in 1..nx {
                    let i = iz * row + ix;
                    let h = src[i];
                    let nb = [i - 1, i + 1, i - row, i + row];
                    let mut drops = [0.0f32; 4];
                    let mut total = 0.0;
                    let mut biggest = 0.0f32;
                    for k in 0..4 {
                        let d = h - src[nb[k]] - talus;
                        if d > 0.0 {
                            drops[k] = d;
                            total += d;
                            biggest = biggest.max(d);
                        }
                    }
                    if total > 0.0 {
                        let amount = 0.3 * biggest;
                        self.heights[i] -= amount;
                        for k in 0..4 {
                            self.heights[nb[k]] += amount * drops[k] / total;
                        }
                    }
                }
            }
        }
        self.touch(0, 0, nx, nz);
    }

    /// Sets the layer weights from slope, height and noise: rock on the
    /// steeps, snow up high, sand/dirt low and by the water.
    pub fn autopaint(&mut self, style: Style) {
        let (nx, nz) = (self.cells[0], self.cells[1]);
        let b = self.bounds();
        let range = (b.max[1] - b.min[1]).max(1.0);
        for iz in 0..=nz {
            for ix in 0..=nx {
                let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
                let h = (self.heights[iz * (nx + 1) + ix] - b.min[1]) / range;
                let slope = self.slope_at(x, z);
                let n = noise::fbm(x * 0.05, z * 0.05, 3, self.seed ^ 0x77) * 0.5 + 0.5;
                let n2 = noise::fbm(x * 0.4, z * 0.4, 2, self.seed ^ 0x99) * 0.5 + 0.5;
                let rock = smoothstep(30.0, 46.0, slope + (n - 0.5) * 10.0);
                let mut w = match style {
                    Style::Flat => [1.0, 0.0, 0.0, 0.0],
                    Style::Dunes => [1.0 - rock - 0.3 * n, rock, 0.3 * n + 0.2 * n2, 0.0],
                    Style::Canyon => {
                        let band = ((h * 9.0 + n * 2.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                        [(1.0 - rock) * band, rock, (1.0 - rock) * (1.0 - band) * 0.6, (1.0 - rock) * 0.2 * n2]
                    }
                    _ => {
                        let snow = if matches!(style, Style::Mountains) { smoothstep(0.62, 0.78, h + (n - 0.5) * 0.12) * (1.0 - rock * 0.85) } else { 0.0 };
                        let low = 1.0 - smoothstep(0.0, 0.07, h + (n - 0.5) * 0.04);
                        let dirt = (low * 0.8 + smoothstep(0.55, 0.9, n2) * 0.25 * (1.0 - rock)).clamp(0.0, 1.0);
                        let grass = ((1.0 - rock) * (1.0 - snow) * (1.0 - dirt)).max(0.0);
                        [grass, rock, dirt * (1.0 - rock), snow]
                    }
                };
                let sum: f32 = w.iter().map(|v| v.max(0.0)).sum::<f32>().max(1e-6);
                for v in w.iter_mut() {
                    *v = v.max(0.0) / sum;
                }
                let mut out = w.map(|v| (v * 255.0).round() as u8);
                let used: i32 = out.iter().map(|v| *v as i32).sum();
                let big = (0..4).max_by(|a, c| out[*a].cmp(&out[*c])).unwrap_or(0);
                out[big] = (out[big] as i32 + 255 - used).clamp(0, 255) as u8;
                self.splat[iz * (nx + 1) + ix] = out;
            }
        }
        self.revision += 1;
        let r = self.revision;
        self.chunk_rev.iter_mut().for_each(|c| *c = r);
    }

    /// Layer weights (0..1) at (x, z), blended between samples.
    pub fn weights_at(&self, x: f32, z: f32) -> [f32; 4] {
        let gx = ((x - self.origin[0]) / self.cell).clamp(0.0, self.cells[0] as f32 - 1e-4);
        let gz = ((z - self.origin[1]) / self.cell).clamp(0.0, self.cells[1] as f32 - 1e-4);
        let (ix, iz) = (gx.floor() as usize, gz.floor() as usize);
        let (fx, fz) = (gx - ix as f32, gz - iz as f32);
        let mut out = [0.0; 4];
        for (dx, dz, w) in [(0, 0, (1.0 - fx) * (1.0 - fz)), (1, 0, fx * (1.0 - fz)), (0, 1, (1.0 - fx) * fz), (1, 1, fx * fz)] {
            let s = self.splat[self.at(ix + dx, iz + dz)];
            for k in 0..4 {
                out[k] += s[k] as f32 / 255.0 * w;
            }
        }
        out
    }

    /// The triangles of one chunk at one level of detail, with skirts.
    pub fn chunk_mesh(&self, ci: usize, cj: usize, lod: usize) -> ChunkMesh {
        let stride = LOD_STRIDES[lod.min(LOD_STRIDES.len() - 1)];
        let n = CHUNK / stride;
        let (x0, z0) = (ci * CHUNK, cj * CHUNK);
        let mut positions = Vec::with_capacity((n + 1) * (n + 1) + 4 * (n + 1));
        let mut normals = Vec::with_capacity(positions.capacity());
        let vertex = |ix: usize, iz: usize| {
            let (x, z) = (self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell);
            let e = self.cell;
            let dx = self.sample(ix as i64 + 1, iz as i64) - self.sample(ix as i64 - 1, iz as i64);
            let dz = self.sample(ix as i64, iz as i64 + 1) - self.sample(ix as i64, iz as i64 - 1);
            ([x, self.heights[self.at(ix, iz)], z], crate::math::norm([-dx / (2.0 * e), 1.0, -dz / (2.0 * e)]))
        };
        for k in 0..=n {
            for m in 0..=n {
                let (p, nrm) = vertex(x0 + m * stride, z0 + k * stride);
                positions.push(p);
                normals.push(nrm);
            }
        }
        let mut indices = Vec::with_capacity(n * n * 6 + 4 * n * 6);
        let v = |m: usize, k: usize| (k * (n + 1) + m) as u32;
        for k in 0..n {
            for m in 0..n {
                let (a, b, c, d) = (v(m, k), v(m + 1, k), v(m, k + 1), v(m + 1, k + 1));
                indices.extend_from_slice(&[a, c, d, a, d, b]);
            }
        }
        // Skirts: each edge sample copied downwards.
        let drop = stride as f32 * self.cell * 0.9 + 0.5;
        let mut skirt = |edge: Vec<u32>, flip: bool| {
            let base = positions.len() as u32;
            for &e in &edge {
                let (mut p, nrm) = (positions[e as usize], normals[e as usize]);
                p[1] -= drop;
                positions.push(p);
                normals.push(nrm);
            }
            for s in 0..edge.len() - 1 {
                let (a, b, c, d) = (edge[s], edge[s + 1], base + s as u32, base + s as u32 + 1);
                if flip {
                    indices.extend_from_slice(&[a, c, d, a, d, b]);
                } else {
                    indices.extend_from_slice(&[a, d, c, a, b, d]);
                }
            }
        };
        skirt((0..=n).map(|m| v(m, 0)).collect(), false);
        skirt((0..=n).map(|m| v(m, n)).collect(), true);
        skirt((0..=n).map(|k| v(0, k)).collect(), true);
        skirt((0..=n).map(|k| v(n, k)).collect(), false);
        ChunkMesh { positions, normals, indices }
    }
}

/// A chunk's drawable triangles.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkMesh {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub indices: Vec<u32>,
}

fn style_height(style: Style, x: f32, z: f32, big: f32, height: f32, seed: u32, size: [f32; 2]) -> f32 {
    let (u, v) = (x / big * 4.0, z / big * 4.0);
    let n = |s: f32, oct: usize, off: u32| noise::fbm(u * s, v * s, oct, seed.wrapping_add(off));
    let unit = match style {
        Style::Flat => 0.012 * (0.5 + 0.5 * n(6.0, 2, 0)),
        Style::Plains => 0.12 + 0.1 * n(0.8, 4, 0),
        Style::Hills => 0.35 + 0.35 * n(1.0, 5, 0),
        Style::Mountains => {
            let base = 0.25 + 0.15 * n(0.7, 3, 4);
            base + 0.75 * noise::ridged(u * 0.7, v * 0.7, 5, seed) * (0.55 + 0.45 * (n(0.5, 2, 9) * 0.5 + 0.5))
        }
        Style::Dunes => {
            let warp = n(0.5, 2, 5) * 1.6;
            let w = ((u * 1.5 + warp + v * 0.4).sin() * 0.5 + 0.5).powf(1.6);
            0.25 + 0.55 * w + 0.08 * n(2.0, 2, 9)
        }
        Style::Canyon => {
            let base = 0.5 + 0.5 * n(0.7, 4, 0);
            let terr = (base * 6.0).floor() / 6.0 + (base * 6.0).fract().powf(3.0) / 6.0;
            let river = n(0.45, 3, 21).abs();
            terr * (river * 6.0).clamp(0.0, 1.0)
        }
        Style::Island => {
            let r = ((x / (size[0] * 0.5)).powi(2) + (z / (size[1] * 0.5)).powi(2)).sqrt();
            let fall = (1.0 - r * 1.1).clamp(0.0, 1.0);
            (0.6 + 0.4 * n(1.0, 5, 0)) * fall.powf(0.8) - 0.12
        }
        Style::Valley => {
            // A winding river bed with slopes rising on both sides.
            let bend = noise::fbm(z / big * 3.0, 0.3, 2, seed ^ 0x13) * 0.18 * big;
            let d = ((x - bend).abs() / (big * 0.5)).clamp(0.0, 1.0);
            0.02 + 0.8 * d.powf(1.4) + 0.15 * (n(1.4, 4, 0) * 0.5 + 0.5) * d
        }
    };
    unit * height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(style: Style) -> Terrain {
        Terrain::generate(&TerrainSpec { size: [128.0, 128.0], cell: 1.0, style, height: 30.0, seed: 3 })
    }

    #[test]
    fn every_style_makes_land_with_its_own_relief() {
        for name in STYLES {
            let style = Style::parse(name).unwrap();
            let t = small(style);
            let b = t.bounds();
            assert!(b.max[1] - b.min[1] > 0.05, "{name}");
            assert_eq!(t.splat.len(), t.heights.len());
            assert!(t.splat.iter().all(|s| s.iter().map(|v| *v as u32).sum::<u32>() == 255), "{name}");
        }
        assert!(small(Style::Mountains).bounds().max[1] > small(Style::Plains).bounds().max[1]);
        assert!(small(Style::Flat).bounds().max[1] < 1.0);
    }

    #[test]
    fn height_follows_the_drawn_triangles() {
        let t = small(Style::Hills);
        // The mesh of a chunk at full detail agrees with height_at at
        // every triangle's centre.
        let m = t.chunk_mesh(1, 1, 0);
        let mut worst = 0.0f32;
        for tri in m.indices.chunks(3).take(32 * 32 * 2) {
            let p: Vec<V3> = tri.iter().map(|i| m.positions[*i as usize]).collect();
            let c = [(p[0][0] + p[1][0] + p[2][0]) / 3.0, 0.0, (p[0][2] + p[1][2] + p[2][2]) / 3.0];
            let h = (p[0][1] + p[1][1] + p[2][1]) / 3.0;
            worst = worst.max((h - t.height_at(c[0], c[2])).abs());
        }
        assert!(worst < 1e-3, "{worst}");
        // Every full-detail triangle faces up.
        for tri in m.indices.chunks(3).take(32 * 32 * 2) {
            let p: Vec<V3> = tri.iter().map(|i| m.positions[*i as usize]).collect();
            let n = crate::math::cross(crate::math::sub(p[1], p[0]), crate::math::sub(p[2], p[0]));
            assert!(n[1] > 0.0);
        }
    }

    #[test]
    fn coarser_levels_have_fewer_triangles_and_chunks_have_skirts() {
        let t = small(Style::Hills);
        let counts: Vec<usize> = (0..4).map(|l| t.chunk_mesh(0, 0, l).indices.len() / 3).collect();
        assert!(counts.windows(2).all(|w| w[0] > w[1]), "{counts:?}");
        let m = t.chunk_mesh(0, 0, 3);
        let low = m.positions.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let (lo, _) = t.chunk_bounds(0, 0).min.iter().fold((f32::INFINITY, 0.0), |a, v| (a.0.min(*v), 0.0));
        assert!(low < lo.max(t.chunk_bounds(0, 0).min[1]) + 0.01 || low < t.chunk_bounds(0, 0).min[1], "skirt hangs below");
    }

    #[test]
    fn sculpting_raises_flattens_and_dirties_only_nearby_chunks() {
        let mut t = Terrain::flat([128.0, 128.0], 1.0);
        let r0 = t.chunk_revision(0, 0);
        let r_far = t.chunk_revision(3, 3);
        t.sculpt(Brush::Raise, [-60.0, -60.0], 10.0, 5.0);
        assert!(t.height_at(-60.0, -60.0) > 4.0);
        assert!(t.chunk_revision(0, 0) > r0);
        assert_eq!(t.chunk_revision(3, 3), r_far);
        t.sculpt(Brush::Level(0.0), [-60.0, -60.0], 10.0, 1.0);
        assert!(t.height_at(-60.0, -60.0).abs() < 0.2);
        t.pad([10.0, 10.0], 8.0, 6.0, 3.0);
        assert!((t.height_at(10.0, 10.0) - 3.0).abs() < 1e-3);
        assert!(t.height_at(40.0, 40.0).abs() < 1e-3);
        t.basin([0.0, 30.0], 6.0, 4.0);
        assert!(t.height_at(0.0, 30.0) < -3.0);
    }

    #[test]
    fn painting_shifts_weight_to_the_layer_and_keeps_the_sum() {
        let mut t = Terrain::flat([64.0, 64.0], 1.0);
        t.paint(2, [0.0, 0.0], 6.0, 1.0);
        let w = t.weights_at(0.0, 0.0);
        assert!(w[2] > 0.9, "{w:?}");
        assert!(t.weights_at(25.0, 25.0)[0] > 0.99);
        assert!(t.splat.iter().all(|s| s.iter().map(|v| *v as u32).sum::<u32>() == 255));
    }

    #[test]
    fn rays_land_on_the_ground_and_slopes_read_right() {
        let mut t = Terrain::flat([128.0, 128.0], 1.0);
        t.sculpt(Brush::Level(5.0), [0.0, 0.0], 30.0, 1.0);
        let d = t.raycast([0.0, 20.0, 0.0], [0.0, -1.0, 0.0], 100.0).unwrap();
        assert!((20.0 - d - t.height_at(0.0, 0.0)).abs() < 0.05);
        assert!(t.raycast([0.0, 20.0, 0.0], [0.0, 1.0, 0.0], 100.0).is_none());
        assert!(t.slope_at(50.0, 50.0) < 1.0);
        let m = small(Style::Mountains);
        let steep = (0..60).map(|i| m.slope_at(-60.0 + i as f32 * 2.0, 10.0)).fold(0.0f32, f32::max);
        assert!(steep > 15.0, "{steep}");
    }
}
