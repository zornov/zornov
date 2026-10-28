// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A model as a solid on a voxel grid, and distances measured through it.
//!
//! Every cell the surface passes through is solid, and so is every cell
//! the outside cannot reach (flooding in from the border), so any model is
//! filled, even one with small holes or parts pushed into each other.
//! Distances through the solid (Dijkstra over the cells, each step to one
//! of the 26 neighbours at its true length) go round a gap instead of
//! across it: from a bone in the forearm to the chest the way runs up the
//! arm, through the shoulder and down, never through the air between the
//! arm and the side. This is what skin weights measure by (after Dionne
//! and de Lasa, "Geodesic Voxel Binding", 2013).

use crate::{Mesh, V3, length, sub};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};

/// The solid: `n` cells a side from `origin`, each `cell` across.
#[derive(Clone, Debug)]
pub struct Solid {
    pub origin: V3,
    pub cell: f32,
    pub n: [usize; 3],
    /// Per cell: inside the model (or on its surface).
    pub inside: Vec<bool>,
}

#[derive(PartialEq)]
struct Item(f32, u32);
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(self.1.cmp(&o.1))
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Solid {
    /// Fills `mesh` on a grid of about `across` cells along its longest
    /// side (at least 8).
    pub fn of(mesh: &Mesh, across: usize) -> Solid {
        let (lo, hi) = mesh.bounds();
        let big = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).max(1e-3);
        let cell = big / across.max(8) as f32;
        // A cell of air round the model so the flood reaches all of it.
        let origin = [lo[0] - cell, lo[1] - cell, lo[2] - cell];
        let n: [usize; 3] = std::array::from_fn(|i| (((hi[i] - lo[i]) / cell).ceil() as usize + 3).max(3));
        let mut s = Solid { origin, cell, n, inside: vec![false; n[0] * n[1] * n[2]] };
        // The surface: points over each triangle no further apart than half
        // a cell.
        let mut wall = vec![false; s.inside.len()];
        for t in 0..mesh.triangles() {
            let [a, b, c] = mesh.tri(t).map(|v| mesh.positions[v]);
            let longest = length(sub(b, a)).max(length(sub(c, b))).max(length(sub(a, c)));
            let k = ((longest / (cell * 0.5)).ceil() as usize).clamp(1, 512);
            for i in 0..=k {
                for j in 0..=(k - i) {
                    let (u, v) = (i as f32 / k as f32, j as f32 / k as f32);
                    let p: V3 = std::array::from_fn(|d| a[d] + (b[d] - a[d]) * u + (c[d] - a[d]) * v);
                    if let Some(id) = s.cell_of(p) {
                        wall[id] = true;
                    }
                }
            }
        }
        // Flood the outside from the border, through cells the surface
        // does not touch.
        let mut outside = vec![false; s.inside.len()];
        let mut queue = VecDeque::new();
        for z in 0..n[2] {
            for y in 0..n[1] {
                for x in 0..n[0] {
                    if (x == 0 || y == 0 || z == 0 || x == n[0] - 1 || y == n[1] - 1 || z == n[2] - 1) && !wall[s.id(x, y, z)] {
                        let id = s.id(x, y, z);
                        outside[id] = true;
                        queue.push_back((x, y, z));
                    }
                }
            }
        }
        while let Some((x, y, z)) = queue.pop_front() {
            for (dx, dy, dz) in [(-1i64, 0i64, 0i64), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)] {
                let (nx, ny, nz) = (x as i64 + dx, y as i64 + dy, z as i64 + dz);
                if nx < 0 || ny < 0 || nz < 0 || nx >= n[0] as i64 || ny >= n[1] as i64 || nz >= n[2] as i64 {
                    continue;
                }
                let id = s.id(nx as usize, ny as usize, nz as usize);
                if !outside[id] && !wall[id] {
                    outside[id] = true;
                    queue.push_back((nx as usize, ny as usize, nz as usize));
                }
            }
        }
        for (i, o) in outside.iter().enumerate() {
            s.inside[i] = !o;
        }
        s
    }

    fn id(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.n[1] + y) * self.n[0] + x
    }

    /// The cell holding `p`, if on the grid.
    pub fn cell_of(&self, p: V3) -> Option<usize> {
        let c: [i64; 3] = std::array::from_fn(|i| ((p[i] - self.origin[i]) / self.cell).floor() as i64);
        (0..3).all(|i| c[i] >= 0 && c[i] < self.n[i] as i64).then(|| self.id(c[0] as usize, c[1] as usize, c[2] as usize))
    }

    /// The centre of cell `id`.
    pub fn centre(&self, id: usize) -> V3 {
        let x = id % self.n[0];
        let y = (id / self.n[0]) % self.n[1];
        let z = id / (self.n[0] * self.n[1]);
        [self.origin[0] + (x as f32 + 0.5) * self.cell, self.origin[1] + (y as f32 + 0.5) * self.cell, self.origin[2] + (z as f32 + 0.5) * self.cell]
    }

    /// How many cells are solid.
    pub fn volume_cells(&self) -> usize {
        self.inside.iter().filter(|x| **x).count()
    }

    /// Distance through the solid from the segment `a`–`b` to every cell
    /// (infinite for air and for solid cut off from it), stopping past
    /// `reach`. The segment starts in the solid cells it is nearest (those
    /// within a cell of its nearest), at their straight distance.
    pub fn distances_from_segment(&self, a: V3, b: V3, reach: f32) -> Vec<f32> {
        let mut d = vec![f32::INFINITY; self.inside.len()];
        let to_seg = |p: V3| length(sub(p, crate::autorig::closest_on_segment(a, b, p)));
        let solid: Vec<(usize, f32)> = self.inside.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| (i, to_seg(self.centre(i)))).collect();
        let near = solid.iter().map(|x| x.1).fold(f32::INFINITY, f32::min);
        let mut heap = BinaryHeap::new();
        for (i, s) in solid {
            if s <= near + self.cell * 0.9 {
                d[i] = s;
                heap.push(Item(s, i as u32));
            }
        }
        let n = self.n;
        let steps: Vec<(i64, i64, i64, f32)> = (-1i64..=1)
            .flat_map(|z| (-1i64..=1).flat_map(move |y| (-1i64..=1).map(move |x| (x, y, z))))
            .filter(|&(x, y, z)| (x, y, z) != (0, 0, 0))
            .map(|(x, y, z)| (x, y, z, ((x * x + y * y + z * z) as f32).sqrt() * self.cell))
            .collect();
        while let Some(Item(dc, id)) = heap.pop() {
            let id = id as usize;
            if dc > d[id] || dc > reach {
                continue;
            }
            let (x, y, z) = ((id % n[0]) as i64, ((id / n[0]) % n[1]) as i64, (id / (n[0] * n[1])) as i64);
            for &(dx, dy, dz, w) in &steps {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if nx < 0 || ny < 0 || nz < 0 || nx >= n[0] as i64 || ny >= n[1] as i64 || nz >= n[2] as i64 {
                    continue;
                }
                let m = self.id(nx as usize, ny as usize, nz as usize);
                let nd = dc + w;
                if self.inside[m] && nd < d[m] && nd <= reach {
                    d[m] = nd;
                    heap.push(Item(nd, m as u32));
                }
            }
        }
        d
    }

    /// A point's distance read from cell distances `d`: the least over its
    /// cell and the 26 round it of the cell's distance plus the way from
    /// the cell's centre to the point (infinite when none is reached).
    pub fn read(&self, d: &[f32], p: V3) -> f32 {
        let c: [i64; 3] = std::array::from_fn(|i| ((p[i] - self.origin[i]) / self.cell).floor() as i64);
        let mut best = f32::INFINITY;
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let q = [c[0] + x, c[1] + y, c[2] + z];
                    if (0..3).any(|i| q[i] < 0 || q[i] >= self.n[i] as i64) {
                        continue;
                    }
                    let id = self.id(q[0] as usize, q[1] as usize, q[2] as usize);
                    if d[id].is_finite() {
                        best = best.min(d[id] + length(sub(p, self.centre(id))));
                    }
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn a_ball_is_filled_and_the_way_round_a_gap_is_longer_than_across_it() {
        let ball = prim::ico_sphere(50.0, 3);
        let s = Solid::of(&ball, 40);
        let cells = s.volume_cells() as f32 * s.cell.powi(3);
        let true_v = 4.0 / 3.0 * std::f32::consts::PI * 50f32.powi(3);
        assert!(cells > true_v * 0.9 && cells < true_v * 1.4, "{cells} vs {true_v}");
        // A U: two bars joined at the bottom. From the left bar's top to the
        // right bar's top the way goes down and round, not across the gap.
        let mut u = prim::cube([20.0, 100.0, 20.0]);
        crate::ops::transform(&mut u, &crate::region::Region::All, [-40.0, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        let mut right = prim::cube([20.0, 100.0, 20.0]);
        crate::ops::transform(&mut right, &crate::region::Region::All, [40.0, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        let mut bottom = prim::cube([100.0, 20.0, 20.0]);
        crate::ops::transform(&mut bottom, &crate::region::Region::All, [0.0, 60.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        crate::ops::merge(&mut u, &right);
        crate::ops::merge(&mut u, &bottom);
        let s = Solid::of(&u, 60);
        let d = s.distances_from_segment([-40.0, -50.0, 0.0], [-40.0, -40.0, 0.0], f32::INFINITY);
        let across = s.read(&d, [40.0, -45.0, 0.0]);
        // Straight it is 80; round the U about 100 + 80 + 100.
        assert!(across > 220.0 && across.is_finite(), "{across}");
    }
}
