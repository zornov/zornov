// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Geometry the critics and the pipelines share: the nearest point on a
//! mesh (a uniform grid of triangles), even samples over a surface, and
//! welded topology (vertices that share a position are one).

use std::collections::HashMap;
use studio_mesh::{Mesh, V3, add, cross, dot, length, scale, sub};

/// The closest point to `p` on triangle `a b c` (Ericson, Real-Time
/// Collision Detection 5.1.5), and its barycentric weights.
pub fn closest_on_triangle(p: V3, a: V3, b: V3, c: V3) -> (V3, [f32; 3]) {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (add(a, scale(ab, v)), [1.0 - v, v, 0.0]);
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (add(a, scale(ac, w)), [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (add(b, scale(sub(c, b), w)), [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (add(a, add(scale(ab, v), scale(ac, w))), [1.0 - v - w, v, w])
}

/// What [`Nearest::find`] found.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub point: V3,
    pub triangle: usize,
    pub bary: [f32; 3],
    pub distance: f32,
}

/// Nearest-point queries on a mesh: triangles bucketed in a uniform grid,
/// searched in growing shells of cells until no closer one can exist.
pub struct Nearest<'m> {
    pub mesh: &'m Mesh,
    lo: V3,
    cell: f32,
    dims: [usize; 3],
    cells: HashMap<[i32; 3], Vec<u32>>,
}

impl<'m> Nearest<'m> {
    pub fn new(mesh: &'m Mesh) -> Nearest<'m> {
        let (lo, hi) = mesh.bounds();
        let n = mesh.triangles().max(1) as f32;
        let ext = sub(hi, lo);
        let vol = (ext[0].max(1e-3) * ext[1].max(1e-3) * ext[2].max(1e-3)).max(1e-9);
        // About two triangles per cell over the box.
        let cell = (vol / (n * 0.5)).cbrt().max(length(ext) / 512.0).max(1e-4);
        let dims = std::array::from_fn(|i| ((ext[i] / cell).ceil() as usize).max(1));
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for t in 0..mesh.triangles() {
            let [a, b, c] = mesh.tri(t);
            let (pa, pb, pc) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
            let tlo: [i32; 3] = std::array::from_fn(|i| ((pa[i].min(pb[i]).min(pc[i]) - lo[i]) / cell).floor() as i32);
            let thi: [i32; 3] = std::array::from_fn(|i| ((pa[i].max(pb[i]).max(pc[i]) - lo[i]) / cell).floor() as i32);
            for z in tlo[2]..=thi[2] {
                for y in tlo[1]..=thi[1] {
                    for x in tlo[0]..=thi[0] {
                        cells.entry([x, y, z]).or_default().push(t as u32);
                    }
                }
            }
        }
        Nearest { mesh, lo, cell, dims, cells }
    }

    fn key(&self, p: V3) -> [i32; 3] {
        std::array::from_fn(|i| ((p[i] - self.lo[i]) / self.cell).floor() as i32)
    }

    /// The nearest point on the surface (None for an empty mesh).
    pub fn find(&self, p: V3) -> Option<Hit> {
        if self.mesh.triangles() == 0 {
            return None;
        }
        let k = self.key(p);
        let mut best: Option<Hit> = None;
        let span = self.dims.iter().copied().max().unwrap_or(1) as i32 + 2;
        // Distance from p to the box outside: shells start where triangles can be.
        for r in 0..=span * 2 {
            if let Some(h) = &best
                && h.distance < (r as f32 - 1.0).max(0.0) * self.cell
            {
                break;
            }
            let mut visit = |c: [i32; 3]| {
                if let Some(list) = self.cells.get(&c) {
                    for &t in list {
                        let [a, b, cc] = self.mesh.tri(t as usize);
                        let (q, bary) = closest_on_triangle(p, self.mesh.positions[a], self.mesh.positions[b], self.mesh.positions[cc]);
                        let d = length(sub(q, p));
                        if best.as_ref().is_none_or(|h| d < h.distance) {
                            best = Some(Hit { point: q, triangle: t as usize, bary, distance: d });
                        }
                    }
                }
            };
            if r == 0 {
                visit(k);
                continue;
            }
            for dz in -r..=r {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        visit([k[0] + dx, k[1] + dy, k[2] + dz]);
                    }
                }
            }
        }
        best
    }
}

pub fn triangle_area(m: &Mesh, t: usize) -> f32 {
    let [a, b, c] = m.tri(t);
    0.5 * length(cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a])))
}

pub fn surface_area(m: &Mesh) -> f32 {
    (0..m.triangles()).map(|t| triangle_area(m, t)).sum()
}

/// A small fast deterministic random source.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// `n` points spread over the surface by area (the same for the same
/// mesh), with the triangle each lies on.
pub fn sample_surface(m: &Mesh, n: usize, seed: u64) -> Vec<(V3, usize)> {
    let tris = m.triangles();
    if tris == 0 {
        return Vec::new();
    }
    let mut cdf = Vec::with_capacity(tris);
    let mut total = 0.0f64;
    for t in 0..tris {
        total += triangle_area(m, t) as f64;
        cdf.push(total);
    }
    let mut rng = Rng(seed | 1);
    (0..n)
        .map(|_| {
            let x = rng.next() as f64 * total;
            let t = cdf.partition_point(|&c| c < x).min(tris - 1);
            let (mut u, mut v) = (rng.next(), rng.next());
            if u + v > 1.0 {
                (u, v) = (1.0 - u, 1.0 - v);
            }
            let [a, b, c] = m.tri(t);
            let (pa, pb, pc) = (m.positions[a], m.positions[b], m.positions[c]);
            (add(pa, add(scale(sub(pb, pa), u), scale(sub(pc, pa), v))), t)
        })
        .collect()
}

/// Welded triangles: corners as position ids, and the positions.
pub struct Welded {
    pub tris: Vec<[u32; 3]>,
    pub points: Vec<V3>,
    /// Position id of each original vertex.
    pub ids: Vec<u32>,
}

pub fn weld(m: &Mesh) -> Welded {
    let (ids, points) = m.position_ids();
    let tris = (0..m.triangles())
        .map(|t| {
            let [a, b, c] = m.tri(t);
            [ids[a], ids[b], ids[c]]
        })
        .collect();
    Welded { tris, points, ids }
}

pub fn diagonal(m: &Mesh) -> f32 {
    let (lo, hi) = m.bounds();
    length(sub(hi, lo)).max(1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_matches_brute_force() {
        let m = studio_mesh::prim::by_name("torus", 200.0, 3).unwrap();
        let near = Nearest::new(&m);
        let mut rng = Rng(7);
        for _ in 0..200 {
            let p = [rng.next() * 400.0 - 200.0, rng.next() * 400.0 - 200.0, rng.next() * 400.0 - 200.0];
            let got = near.find(p).unwrap().distance;
            let want = (0..m.triangles())
                .map(|t| {
                    let [a, b, c] = m.tri(t);
                    length(sub(closest_on_triangle(p, m.positions[a], m.positions[b], m.positions[c]).0, p))
                })
                .fold(f32::INFINITY, f32::min);
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }
}
