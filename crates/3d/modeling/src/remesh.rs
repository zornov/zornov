// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Voxel remeshing: the model's surface rebuilt as even quads from a
//! signed distance grid (surface nets), then laid back onto the original.
//! What a sculptor does before working (even detail everywhere, no thin
//! triangles, holes and overlaps gone) and the first step in cleaning up
//! a scanned or generated model.

use crate::poly::Poly;
use crate::spatial::Tree;
use std::collections::HashMap;
use studio_mesh::{V3, add, cross, dot, length, normalise, scale, sub};

/// Most cells along the longest side.
pub const MAX_CELLS: usize = 384;

/// A signed distance grid: negative inside.
pub struct Grid {
    pub origin: V3,
    pub cell: f32,
    pub dims: [usize; 3],
    pub d: Vec<f32>,
}

impl Grid {
    pub fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.dims[1] + j) * self.dims[0] + i
    }
    pub fn point(&self, i: usize, j: usize, k: usize) -> V3 {
        add(self.origin, [i as f32 * self.cell, j as f32 * self.cell, k as f32 * self.cell])
    }
}

/// The signed distance grid of a surface (triangles), `cell` apart.
pub fn distance_grid(tris: &[[V3; 3]], cell: f32, pad: usize) -> Result<Grid, String> {
    if tris.is_empty() {
        return Err("nothing to remesh".into());
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for t in tris {
        for p in t {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
    }
    let longest = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f32, f32::max);
    let cell = cell.max(longest / MAX_CELLS as f32).max(1e-4);
    let origin = sub(lo, [cell * pad as f32; 3]);
    let dims: [usize; 3] = std::array::from_fn(|i| (((hi[i] - lo[i]) / cell).ceil() as usize + 2 * pad + 1).max(2));
    let n = dims[0] * dims[1] * dims[2];
    let mut g = Grid { origin, cell, dims, d: vec![f32::INFINITY; n] };
    let mut sign = vec![0.0f32; n];
    let band = 1.5 * cell;
    for t in tris {
        let n_t = normalise(cross(sub(t[1], t[0]), sub(t[2], t[0])));
        let mut tlo = [f32::INFINITY; 3];
        let mut thi = [f32::NEG_INFINITY; 3];
        for p in t {
            for i in 0..3 {
                tlo[i] = tlo[i].min(p[i]);
                thi[i] = thi[i].max(p[i]);
            }
        }
        let r0: [usize; 3] = std::array::from_fn(|i| (((tlo[i] - band - origin[i]) / cell).floor().max(0.0)) as usize);
        let r1: [usize; 3] = std::array::from_fn(|i| ((((thi[i] + band - origin[i]) / cell).ceil()) as usize).min(dims[i] - 1));
        for k in r0[2]..=r1[2] {
            for j in r0[1]..=r1[1] {
                for i in r0[0]..=r1[0] {
                    let p = g.point(i, j, k);
                    let q = crate::spatial::closest_on_tri(p, t);
                    let dd = length(sub(p, q));
                    let at = g.index(i, j, k);
                    if dd < g.d[at] {
                        g.d[at] = dd;
                        // Side by the face normal; corners and edges are
                        // decided by the nearest face.
                        let s = dot(sub(p, q), n_t);
                        sign[at] = if s.abs() < 1e-9 { sign[at] } else { s.signum() };
                    }
                }
            }
        }
    }
    // Inside where the winding number along each +x row is above zero
    // (overlapping parts count as one solid; no leaks between them).
    let tree = Tree::new(tris.to_vec());
    let jitter = [0.0, 1.3e-4 * cell, 0.7e-4 * cell];
    for k in 0..dims[2] {
        for j in 0..dims[1] {
            let o = add(g.point(0, j, k), [-cell, jitter[1], jitter[2]]);
            let hits = tree.crossings(o, [1.0, 0.0, 0.0]);
            let mut winding = 0.0f32;
            let mut h = 0;
            for i in 0..dims[0] {
                let x = (i as f32 + 1.0) * cell;
                while h < hits.len() && hits[h].0 <= x {
                    winding -= hits[h].1;
                    h += 1;
                }
                let at = g.index(i, j, k);
                let inside = winding > 0.5;
                let d = if g.d[at].is_finite() { g.d[at] } else { band * 2.0 };
                g.d[at] = if inside { -d.max(1e-5 * cell) } else { d.max(1e-5 * cell) };
            }
        }
    }
    let _ = sign;
    Ok(g)
}

/// Quads over the zero level of a grid (surface nets).
pub fn surface_nets(g: &Grid) -> Poly {
    let [nx, ny, nz] = g.dims;
    let mut p = Poly::new();
    let mut cell_vert: HashMap<usize, u32> = HashMap::new();
    let corner = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];
    let edges = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    for k in 0..nz - 1 {
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let vals: [f32; 8] = std::array::from_fn(|c| g.d[g.index(i + corner[c][0], j + corner[c][1], k + corner[c][2])]);
                let inside = vals.iter().filter(|v| **v < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                let mut sum = [0.0; 3];
                let mut cnt = 0;
                for (a, b) in edges {
                    let (va, vb) = (vals[a], vals[b]);
                    if (va < 0.0) != (vb < 0.0) {
                        let t = va / (va - vb);
                        let pa = g.point(i + corner[a][0], j + corner[a][1], k + corner[a][2]);
                        let pb = g.point(i + corner[b][0], j + corner[b][1], k + corner[b][2]);
                        sum = add(sum, add(pa, scale(sub(pb, pa), t)));
                        cnt += 1;
                    }
                }
                let v = p.add_vertex(scale(sum, 1.0 / cnt as f32));
                cell_vert.insert((k * ny + j) * nx + i, v);
            }
        }
    }
    let cell = |i: usize, j: usize, k: usize| cell_vert.get(&((k * ny + j) * nx + i)).copied();
    // Each grid edge that crosses the surface makes a quad of the four
    // cells around it.
    for k in 1..nz - 1 {
        for j in 1..ny - 1 {
            for i in 1..nx - 1 {
                let here = g.d[g.index(i, j, k)] < 0.0;
                // +x edge
                if i + 1 < nx {
                    let there = g.d[g.index(i + 1, j, k)] < 0.0;
                    if here != there
                        && let (Some(a), Some(b), Some(c), Some(d)) = (cell(i, j - 1, k - 1), cell(i, j, k - 1), cell(i, j, k), cell(i, j - 1, k))
                    {
                        let q = [a, b, c, d];
                        add_quad(&mut p, q, here);
                    }
                }
                if j + 1 < ny {
                    let there = g.d[g.index(i, j + 1, k)] < 0.0;
                    if here != there
                        && let (Some(a), Some(b), Some(c), Some(d)) = (cell(i - 1, j, k - 1), cell(i - 1, j, k), cell(i, j, k), cell(i, j, k - 1))
                    {
                        add_quad(&mut p, [a, b, c, d], here);
                    }
                }
                if k + 1 < nz {
                    let there = g.d[g.index(i, j, k + 1)] < 0.0;
                    if here != there
                        && let (Some(a), Some(b), Some(c), Some(d)) = (cell(i - 1, j - 1, k), cell(i, j - 1, k), cell(i, j, k), cell(i - 1, j, k))
                    {
                        add_quad(&mut p, [a, b, c, d], here);
                    }
                }
            }
        }
    }
    p.ensure_materials();
    p
}

fn add_quad(p: &mut Poly, q: [u32; 4], inside_first: bool) {
    if inside_first {
        p.add_face(&q, 0);
    } else {
        p.add_face(&[q[3], q[2], q[1], q[0]], 0);
    }
}

/// The model rebuilt from voxels `size` px across, laid back on its
/// original surface.
pub fn voxel(p: &Poly, size: f32) -> Result<Poly, String> {
    let m = p.to_mesh(180.0, crate::NormalMode::Angle);
    let tris: Vec<[V3; 3]> = (0..m.triangles()).map(|t| m.tri(t).map(|v| m.positions[v])).collect();
    let g = distance_grid(&tris, size, 2)?;
    let mut out = surface_nets(&g);
    if out.faces() == 0 {
        return Err("the remesh found no surface (is the voxel size bigger than the model?)".into());
    }
    out.orient_outward();
    let tree = Tree::new(tris);
    relax_onto(&mut out, &tree, 3);
    Ok(out)
}

/// Evens out vertex spacing while keeping the points on a surface.
pub fn relax_onto(p: &mut Poly, tree: &Tree, iterations: usize) {
    let t = crate::poly::Topology::of(p);
    let nb: Vec<Vec<u32>> = (0..p.positions.len() as u32).map(|v| t.neighbours(p, v)).collect();
    for _ in 0..iterations {
        let cur = p.positions.clone();
        for v in 0..cur.len() {
            if nb[v].is_empty() {
                continue;
            }
            let mut c = [0.0; 3];
            for &u in &nb[v] {
                c = add(c, cur[u as usize]);
            }
            c = scale(c, 1.0 / nb[v].len() as f32);
            let moved = add(cur[v], scale(sub(c, cur[v]), 0.5));
            p.positions[v] = tree.nearest(moved, f32::INFINITY).map_or(moved, |h| h.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn a_sphere_remeshes_to_closed_quads_on_its_surface() {
        let s = prim::uv_sphere(20.0, 24, 12);
        let r = voxel(&s, 2.0).unwrap();
        assert!(crate::poly::Topology::of(&r).boundary_edges().is_empty(), "{}", r.stats());
        assert!((0..r.faces()).all(|f| r.face(f).len() == 4));
        let err = r.positions.iter().map(|v| (length(*v) - 20.0).abs()).fold(0.0f32, f32::max);
        assert!(err < 0.6, "{err}");
        for f in 0..r.faces() {
            assert!(dot(r.face_normal(f), normalise(r.face_centre(f))) > 0.0);
        }
    }

    #[test]
    fn overlapping_parts_merge_into_one_shell() {
        let mut a = prim::cube([10.0; 3], 1);
        let mut b = prim::quad_sphere(6.0, 4);
        b.map_positions(|v| add(v, [5.0, 0.0, 0.0]));
        a.merge(&b);
        let r = voxel(&a, 1.0).unwrap();
        assert!(crate::poly::Topology::of(&r).boundary_edges().is_empty());
        // No inner walls: nothing near the cube's centre plane inside the ball.
        assert!(r.positions.iter().all(|v| !(length(sub(*v, [5.0, 0.0, 0.0])) < 4.0)));
    }
}
