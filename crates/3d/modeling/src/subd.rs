// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catmull–Clark subdivision with creases: every face becomes quads and
//! the surface rounds off, except where edges are creased — fully (a sharp
//! edge that stays sharp at every level) or partly (0.25 per level stays
//! sharp, then softens: the crisp-but-not-razor edge of a machined part).
//! Open edges keep their line and corners stay put, so panels keep their
//! outline.

use crate::poly::{Edge, Poly, Topology, ekey};
use std::collections::HashMap;
use studio_mesh::{V3, add, lerp, scale};

/// Subdivides `levels` times.
pub fn subdivide(p: &Poly, levels: usize) -> Poly {
    let mut out = p.clone();
    for _ in 0..levels {
        out = once(&out);
    }
    out
}

/// A crease weight as levels of sharpness (1 = sharp for good).
fn sharpness(w: f32) -> f32 {
    if w >= 0.999 { f32::INFINITY } else { (w * 4.0).max(0.0) }
}

fn weight_of(s: f32) -> f32 {
    if s.is_infinite() { 1.0 } else { (s / 4.0).clamp(0.0, 1.0) }
}

/// One level of subdivision.
pub fn once(p: &Poly) -> Poly {
    once_with(p, false)
}

/// One level; `linear` keeps the shape (midpoints and centres only).
pub fn once_with(p: &Poly, linear: bool) -> Poly {
    let topo = Topology::of(p);
    let nv = p.positions.len();
    let nf = p.faces();
    let face_pts: Vec<V3> = (0..nf).map(|f| p.face_centre(f)).collect();
    // Edge numbering.
    let mut edge_id: HashMap<Edge, u32> = HashMap::with_capacity(topo.edge_faces.len());
    let mut edges: Vec<Edge> = Vec::with_capacity(topo.edge_faces.len());
    for f in 0..nf {
        let v = p.face(f);
        for i in 0..v.len() {
            let e = ekey(v[i], v[(i + 1) % v.len()]);
            edge_id.entry(e).or_insert_with(|| {
                edges.push(e);
                (edges.len() - 1) as u32
            });
        }
    }
    let edge_sharp: Vec<f32> = edges
        .iter()
        .map(|e| {
            let faces = topo.edge_faces.get(e).map_or(0, Vec::len);
            if faces != 2 { f32::INFINITY } else { p.creases.get(e).copied().map_or(0.0, sharpness) }
        })
        .collect();
    // Edge points.
    let edge_pts: Vec<V3> = edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (a, b) = (p.positions[e.0 as usize], p.positions[e.1 as usize]);
            let mid = lerp(a, b, 0.5);
            let s = edge_sharp[i];
            if linear || s.is_infinite() || s >= 1.0 {
                return mid;
            }
            let fs = &topo.edge_faces[e];
            let smooth = scale(add(add(a, b), add(face_pts[fs[0] as usize], face_pts[fs[1] as usize])), 0.25);
            lerp(smooth, mid, s)
        })
        .collect();
    // Vertex points.
    let mut vert_edges: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (i, e) in edges.iter().enumerate() {
        vert_edges[e.0 as usize].push(i as u32);
        vert_edges[e.1 as usize].push(i as u32);
    }
    let vert_pts: Vec<V3> = (0..nv)
        .map(|v| {
            let pv = p.positions[v];
            let es = &vert_edges[v];
            let fs = &topo.vertex_faces[v];
            if linear || es.is_empty() || fs.is_empty() {
                return pv;
            }
            let n = es.len() as f32;
            let smooth = {
                let mut q = [0.0; 3];
                for &f in fs {
                    q = add(q, face_pts[f as usize]);
                }
                q = scale(q, 1.0 / fs.len() as f32);
                let mut r = [0.0; 3];
                for &e in es {
                    let (a, b) = edges[e as usize];
                    r = add(r, lerp(p.positions[a as usize], p.positions[b as usize], 0.5));
                }
                r = scale(r, 1.0 / n);
                scale(add(add(q, scale(r, 2.0)), scale(pv, n - 3.0)), 1.0 / n)
            };
            let sharp: Vec<u32> = es.iter().copied().filter(|&e| edge_sharp[e as usize] > 0.0).collect();
            let boundary = es.iter().any(|&e| edge_sharp[e as usize].is_infinite() && topo.edge_faces.get(&edges[e as usize]).map_or(0, Vec::len) != 2);
            match sharp.len() {
                0 | 1 => smooth,
                2 => {
                    // Boundary corners (two open edges and nothing else)
                    // stay where they are.
                    if boundary && es.len() == 2 {
                        return pv;
                    }
                    let other = |e: u32| {
                        let (a, b) = edges[e as usize];
                        p.positions[if a as usize == v { b } else { a } as usize]
                    };
                    let crease = scale(add(add(other(sharp[0]), other(sharp[1])), scale(pv, 6.0)), 1.0 / 8.0);
                    let s = (edge_sharp[sharp[0] as usize] + edge_sharp[sharp[1] as usize]) / 2.0;
                    if s >= 1.0 { crease } else { lerp(smooth, crease, s) }
                }
                _ => {
                    let s = sharp.iter().map(|&e| edge_sharp[e as usize]).sum::<f32>() / sharp.len() as f32;
                    if s >= 1.0 { pv } else { lerp(smooth, pv, s) }
                }
            }
        })
        .collect();
    // New mesh: vertices, then edge points, then face points.
    let mut out = Poly::new();
    out.materials = p.materials.clone();
    out.positions = vert_pts;
    let ebase = out.positions.len() as u32;
    out.positions.extend_from_slice(&edge_pts);
    let fbase = out.positions.len() as u32;
    out.positions.extend_from_slice(&face_pts);
    let uvs = p.has_uvs();
    if uvs {
        out.uvs = Vec::with_capacity(p.corners.len() * 4);
    }
    for f in 0..nf {
        let v = p.face(f);
        let n = v.len();
        let fu = p.face_uvs(f);
        let centre_uv = fu.map(|u| {
            let mut c = [0.0f32; 2];
            for x in u {
                c[0] += x[0];
                c[1] += x[1];
            }
            [c[0] / n as f32, c[1] / n as f32]
        });
        for i in 0..n {
            let prev = (i + n - 1) % n;
            let next = (i + 1) % n;
            let e_next = ebase + edge_id[&ekey(v[i], v[next])];
            let e_prev = ebase + edge_id[&ekey(v[prev], v[i])];
            let quad = [v[i], e_next, fbase + f as u32, e_prev];
            let quv = fu.map(|u| {
                let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                [u[i], mid(u[i], u[next]), centre_uv.unwrap_or_default(), mid(u[prev], u[i])]
            });
            out.add_face_like(&quad, quv.as_ref().map(|q| &q[..]), p.face_material[f], p.face_set[f], p.face_smooth[f]);
        }
    }
    if !uvs {
        out.uvs.clear();
    }
    // Child edges keep creases (one level less), hard edges and seams.
    for (i, e) in edges.iter().enumerate() {
        let mid = ebase + i as u32;
        let halves = [ekey(e.0, mid), ekey(e.1, mid)];
        if let Some(&w) = p.creases.get(e) {
            let s = sharpness(w);
            let child = if s.is_infinite() { 1.0 } else { weight_of((s - 1.0).max(0.0)) };
            if child > 0.0 {
                for h in halves {
                    out.creases.insert(h, child);
                }
            }
        }
        if p.sharp.contains(e) {
            out.sharp.extend(halves);
        }
        if p.seams.contains(e) {
            out.seams.extend(halves);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;
    use studio_mesh::length;

    #[test]
    fn a_cube_rounds_toward_a_sphere() {
        let c = prim::cube([2.0, 2.0, 2.0], 1);
        let s = subdivide(&c, 3);
        assert_eq!(s.faces(), 6 * 64);
        let r: Vec<f32> = s.positions.iter().map(|p| length(*p)).collect();
        let (lo, hi) = r.iter().fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(hi / lo < 1.25, "{lo}..{hi}");
        assert!(crate::poly::Topology::of(&s).boundary_edges().is_empty());
    }

    #[test]
    fn creased_edges_stay_sharp() {
        let mut c = prim::cube([2.0, 2.0, 2.0], 1);
        for e in c.edges() {
            c.creases.insert(e, 1.0);
        }
        let s = subdivide(&c, 2);
        // Fully creased: still a cube (every point on the surface of the box).
        for p in &s.positions {
            let m = p[0].abs().max(p[1].abs()).max(p[2].abs());
            assert!((m - 1.0).abs() < 1e-5, "{p:?}");
        }
    }

    #[test]
    fn half_creases_are_between_round_and_sharp() {
        let corner = |w: f32| {
            let mut c = prim::cube([2.0, 2.0, 2.0], 1);
            for e in c.edges() {
                c.creases.insert(e, w);
            }
            let s = subdivide(&c, 3);
            s.positions.iter().map(|p| length(*p)).fold(0.0f32, f32::max)
        };
        let (soft, mid, hard) = (corner(0.0), corner(0.5), corner(1.0));
        assert!(soft < mid && mid < hard, "{soft} {mid} {hard}");
    }

    #[test]
    fn open_sheets_keep_their_corners() {
        let p = prim::plane(2.0, 2.0, 2, 2);
        let s = subdivide(&p, 2);
        let (lo, hi) = s.bounds();
        assert!((lo[0] + 1.0).abs() < 1e-5 && (hi[2] - 1.0).abs() < 1e-5);
        assert!(s.has_uvs());
    }
}
