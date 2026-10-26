// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How good a model's texture coordinates are: how few pieces (islands),
//! how little the picture stretches on the surface (Sander's L2 stretch,
//! 1 = none), how square it stays (conformal error), whether pieces lie on
//! each other or are mirrored, how much of the texture they use, and
//! whether every part of the surface gets the same texture detail.

use crate::geo::triangle_area;
use serde::Serialize;
use studio_mesh::Mesh;

#[derive(Clone, Debug, Default, Serialize)]
pub struct UvQuality {
    pub islands: usize,
    /// Area-weighted L2 stretch, scaled so a perfect isometric layout is 1.
    pub stretch: f32,
    /// Mean of the larger / smaller singular value (1 = angles kept).
    pub conformal: f32,
    /// Share of the surface whose texture is mirrored against its island.
    pub flipped: f32,
    /// Share of the texture used by more than one face.
    pub overlap: f32,
    /// Share of the 0–1 square the islands cover.
    pub packing: f32,
    /// Share of the surface mapped outside 0–1.
    pub outside: f32,
    /// Spread of texture detail over the surface (standard deviation / mean).
    pub density_cov: f32,
    /// Share of the surface with no texture area at all (collapsed).
    pub collapsed: f32,
}

fn find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}

pub fn analyse(m: &Mesh) -> UvQuality {
    let n = m.triangles();
    let mut q = UvQuality::default();
    if n == 0 || m.uvs.len() != m.positions.len() {
        return q;
    }
    // Islands: faces joined by edges whose corners are the same points with
    // the same texture coordinates (a seam splits them; a hard edge in the
    // normals does not).
    let mut key_id: std::collections::HashMap<([i64; 3], [i64; 2]), u32> = std::collections::HashMap::new();
    let corner: Vec<u32> = (0..m.positions.len())
        .map(|v| {
            let p = m.positions[v];
            let u = m.uvs[v];
            let k = ([(p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64, (p[2] * 1000.0).round() as i64], [(u[0] * 1e6).round() as i64, (u[1] * 1e6).round() as i64]);
            let next = key_id.len() as u32;
            *key_id.entry(k).or_insert(next)
        })
        .collect();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut edge_of: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for t in 0..n {
        let c = m.tri(t);
        for k in 0..3 {
            let (a, b) = (corner[c[k]], corner[c[(k + 1) % 3]]);
            let key = (a.min(b), a.max(b));
            if let Some(&o) = edge_of.get(&key) {
                let (ra, rb) = (find(&mut parent, o), find(&mut parent, t as u32));
                if ra != rb {
                    parent[ra as usize] = rb;
                }
            } else {
                edge_of.insert(key, t as u32);
            }
        }
    }
    let roots: Vec<u32> = (0..n as u32).map(|t| find(&mut parent, t)).collect();
    let mut island_sign: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    let mut a3 = vec![0f32; n];
    let mut auv = vec![0f32; n];
    for t in 0..n {
        let [a, b, c] = m.tri(t);
        let (u0, u1, u2) = (m.uvs[a], m.uvs[b], m.uvs[c]);
        let s = 0.5 * ((u1[0] - u0[0]) * (u2[1] - u0[1]) - (u2[0] - u0[0]) * (u1[1] - u0[1]));
        auv[t] = s;
        a3[t] = triangle_area(m, t);
        *island_sign.entry(roots[t]).or_default() += s as f64;
    }
    q.islands = island_sign.len();
    let total3: f64 = a3.iter().map(|&a| a as f64).sum::<f64>().max(1e-12);
    let total_uv: f64 = auv.iter().map(|&a| a.abs() as f64).sum::<f64>().max(1e-12);
    // Stretch and conformality from each face's Jacobian (texture → surface).
    let mut l2 = 0.0f64;
    let mut conf = 0.0f64;
    let mut flipped = 0.0f64;
    let mut collapsed = 0.0f64;
    let mut outside = 0.0f64;
    let mut dens = Vec::with_capacity(n);
    for t in 0..n {
        let [a, b, c] = m.tri(t);
        let (p0, p1, p2) = (m.positions[a], m.positions[b], m.positions[c]);
        let (u0, u1, u2) = (m.uvs[a], m.uvs[b], m.uvs[c]);
        let area3 = a3[t] as f64;
        if area3 <= 0.0 {
            continue;
        }
        if [u0, u1, u2].iter().any(|u| u[0] < -1e-4 || u[0] > 1.0001 || u[1] < -1e-4 || u[1] > 1.0001) {
            outside += area3;
        }
        let s = auv[t] as f64;
        if s.abs() < 1e-12 {
            collapsed += area3;
            continue;
        }
        if (s < 0.0) != (island_sign[&roots[t]] < 0.0) {
            flipped += area3;
        }
        // Sander et al. 2001: Ss and St, the surface rates along u and v.
        let (s1, t1) = (u0[0] as f64, u0[1] as f64);
        let (s2, t2) = (u1[0] as f64, u1[1] as f64);
        let (s3, t3) = (u2[0] as f64, u2[1] as f64);
        let two_a = 2.0 * s;
        let ss: [f64; 3] = std::array::from_fn(|i| (p0[i] as f64 * (t2 - t3) + p1[i] as f64 * (t3 - t1) + p2[i] as f64 * (t1 - t2)) / two_a);
        let st: [f64; 3] = std::array::from_fn(|i| (p0[i] as f64 * (s3 - s2) + p1[i] as f64 * (s1 - s3) + p2[i] as f64 * (s2 - s1)) / two_a);
        let aa = ss.iter().map(|x| x * x).sum::<f64>();
        let cc = st.iter().map(|x| x * x).sum::<f64>();
        let bb = ss.iter().zip(&st).map(|(x, y)| x * y).sum::<f64>();
        let root = ((aa - cc) * (aa - cc) + 4.0 * bb * bb).sqrt();
        let g_max = ((aa + cc + root) * 0.5).max(0.0).sqrt();
        let g_min = ((aa + cc - root) * 0.5).max(0.0).sqrt();
        l2 += (aa + cc) * 0.5 * area3;
        conf += (g_max / g_min.max(1e-12)).min(50.0) * area3;
        dens.push(((s.abs() / area3).sqrt(), area3));
    }
    // Scaled so a uniform isometric layout is 1 whatever its overall size.
    q.stretch = ((l2 / total3).sqrt() * (total_uv / total3).sqrt()) as f32;
    q.conformal = (conf / (total3 - collapsed).max(1e-12)) as f32;
    q.flipped = (flipped / total3) as f32;
    q.collapsed = (collapsed / total3) as f32;
    q.outside = (outside / total3) as f32;
    let wsum: f64 = dens.iter().map(|d| d.1).sum::<f64>().max(1e-12);
    let mean = dens.iter().map(|d| d.0 * d.1).sum::<f64>() / wsum;
    let var = dens.iter().map(|d| (d.0 - mean) * (d.0 - mean) * d.1).sum::<f64>() / wsum;
    q.density_cov = (var.sqrt() / mean.max(1e-12)) as f32;
    // Coverage and overlap on a 512² grid of the unit square.
    const R: usize = 512;
    let mut hits = vec![0u8; R * R];
    for t in 0..n {
        let [a, b, c] = m.tri(t);
        let pts = [m.uvs[a], m.uvs[b], m.uvs[c]].map(|u| [u[0] * R as f32, u[1] * R as f32]);
        let x0 = pts.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let x1 = pts.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max).ceil().min(R as f32) as usize;
        let y0 = pts.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let y1 = pts.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max).ceil().min(R as f32) as usize;
        let edge = |p: [f32; 2], q: [f32; 2], x: f32, y: f32| (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0]);
        let area = edge(pts[0], pts[1], pts[2][0], pts[2][1]);
        if area.abs() < 1e-9 {
            continue;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5013, y as f32 + 0.5021);
                let w0 = edge(pts[1], pts[2], px, py) * area.signum();
                let w1 = edge(pts[2], pts[0], px, py) * area.signum();
                let w2 = edge(pts[0], pts[1], px, py) * area.signum();
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    let h = &mut hits[y * R + x];
                    *h = h.saturating_add(1);
                }
            }
        }
    }
    let covered = hits.iter().filter(|&&h| h > 0).count();
    let over = hits.iter().filter(|&&h| h > 1).count();
    q.packing = covered as f32 / (R * R) as f32;
    q.overlap = over as f32 / covered.max(1) as f32;
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_square_maps_without_stretch() {
        let mut m = studio_mesh::prim::by_name("plane", 100.0, 2).unwrap();
        let (lo, hi) = m.bounds();
        // Planar map over whichever two axes the plane spans.
        let axes: Vec<usize> = (0..3).filter(|&i| hi[i] - lo[i] > 1.0).collect();
        for (k, p) in m.positions.iter().enumerate() {
            m.uvs[k] = [(p[axes[0]] - lo[axes[0]]) / (hi[axes[0]] - lo[axes[0]]), (p[axes[1]] - lo[axes[1]]) / (hi[axes[1]] - lo[axes[1]])];
        }
        let q = analyse(&m);
        assert_eq!(q.islands, 1);
        assert!((q.stretch - 1.0).abs() < 0.01, "{q:?}");
        assert!(q.overlap < 0.01 && q.packing > 0.95, "{q:?}");
        assert!(q.density_cov < 0.01);
    }

    #[test]
    fn sphere_projection_stretches() {
        let mut m = studio_mesh::prim::by_name("sphere", 100.0, 4).unwrap();
        studio_mesh::ops::project_uvs(&mut m, "planar").unwrap();
        let q = analyse(&m);
        // Front and back lie on each other.
        assert!(q.overlap > 0.5, "{q:?}");
    }
}
