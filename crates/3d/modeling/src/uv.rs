// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Texture layout (UVs): cutting the surface into pieces along seams (the
//! ones marked, hard edges, or found automatically), flattening each with
//! least-squares conformal maps (angles kept, so a painted circle stays
//! round), and packing the pieces into the square with even texel density
//! and a margin between them for mip-maps and baking.

use crate::poly::{Edge, Poly, Topology, ekey, triangulate};
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, cross, dot, length, normalise, scale, sub};

/// How to choose seams.
#[derive(Clone, Debug, PartialEq)]
pub enum Seams {
    /// The seams marked on the mesh only.
    Marked,
    /// Marked seams plus edges sharper than `degrees`.
    Angle(f32),
    /// Pieces grown from faces facing alike (within `degrees`), for hard
    /// surfaces; marked seams are kept.
    Smart(f32),
}

/// Settings for an unwrap.
#[derive(Clone, Debug, PartialEq)]
pub struct Unwrap {
    pub seams: Seams,
    /// Space between pieces, as a fraction of the square (0.004 ≈ 4 px at
    /// 1024).
    pub margin: f32,
    /// Turn pieces to pack tighter.
    pub rotate: bool,
}

impl Default for Unwrap {
    fn default() -> Unwrap {
        Unwrap { seams: Seams::Smart(66.0), margin: 0.006, rotate: true }
    }
}

/// How well a layout came out.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Quality {
    pub charts: usize,
    /// Share of the square covered by pieces (0–1).
    pub coverage: f32,
    /// Mean area stretch (1 = none; above 1.3 shows in textures).
    pub stretch: f32,
    /// Worst angle distortion as the ratio of a triangle's stretch
    /// directions (1 = conformal).
    pub worst_shear: f32,
    pub flipped: usize,
}

/// Face groups (charts) split by seams.
pub fn charts(p: &Poly, seams: &Seams) -> (Vec<u32>, HashSet<Edge>) {
    let t = Topology::of(p);
    let nf = p.faces();
    let mut cut: HashSet<Edge> = p.seams.clone();
    let normals: Vec<V3> = (0..nf).map(|f| p.face_normal(f)).collect();
    if let Seams::Angle(deg) = seams {
        let c = deg.to_radians().cos();
        for (e, fs) in &t.edge_faces {
            if fs.len() == 2 && dot(normals[fs[0] as usize], normals[fs[1] as usize]) < c {
                cut.insert(*e);
            }
        }
    }
    for (e, fs) in &t.edge_faces {
        if fs.len() != 2 {
            cut.insert(*e);
        }
    }
    let mut chart = vec![u32::MAX; nf];
    let mut next = 0u32;
    let neighbours = |f: usize| -> Vec<(usize, Edge)> {
        let c = p.face(f);
        (0..c.len())
            .filter_map(|i| {
                let e = ekey(c[i], c[(i + 1) % c.len()]);
                t.edge_faces.get(&e).and_then(|fs| fs.iter().copied().find(|&g| g as usize != f)).map(|g| (g as usize, e))
            })
            .collect()
    };
    // Smart: grow from the biggest unassigned face while normals stay near
    // the piece's average.
    let mut order: Vec<usize> = (0..nf).collect();
    if matches!(seams, Seams::Smart(_)) {
        order.sort_by(|a, b| p.face_area(*b).total_cmp(&p.face_area(*a)));
    }
    for &s in &order {
        if chart[s] != u32::MAX {
            continue;
        }
        chart[s] = next;
        let mut sum = scale(normals[s], p.face_area(s));
        let mut stack = vec![s];
        while let Some(f) = stack.pop() {
            for (g, e) in neighbours(f) {
                if chart[g] != u32::MAX || cut.contains(&e) {
                    continue;
                }
                if let Seams::Smart(deg) = seams
                    && dot(normals[g], normalise(sum)) < deg.to_radians().cos()
                {
                    continue;
                }
                chart[g] = next;
                sum = studio_mesh::add(sum, scale(normals[g], p.face_area(g)));
                stack.push(g);
            }
        }
        next += 1;
    }
    // The cuts actually used: edges between different charts.
    let used: HashSet<Edge> = t.edge_faces.iter().filter(|(_, fs)| fs.len() != 2 || chart[fs[0] as usize] != chart[fs[1] as usize]).map(|(e, _)| *e).collect();
    (chart, used)
}

/// Flattens one chart: its faces' corners get coordinates. Returns local
/// vertex → 2D.
fn flatten(p: &Poly, faces: &[usize]) -> HashMap<u32, [f32; 2]> {
    // Local vertices.
    let mut index: HashMap<u32, usize> = HashMap::new();
    let mut verts: Vec<u32> = Vec::new();
    for &f in faces {
        for &v in p.face(f) {
            index.entry(v).or_insert_with(|| {
                verts.push(v);
                verts.len() - 1
            });
        }
    }
    let n = verts.len();
    // Starting guess: projection onto the chart's average plane.
    let mut an = [0.0; 3];
    for &f in faces {
        an = studio_mesh::add(an, p.face_area_vector(f));
    }
    let an = normalise(an);
    let ax = normalise(if an[0].abs() < 0.9 { cross(an, [1.0, 0.0, 0.0]) } else { cross(an, [0.0, 1.0, 0.0]) });
    let ay = cross(an, ax);
    let mut x: Vec<f32> = Vec::with_capacity(2 * n);
    for &v in &verts {
        let q = p.positions[v as usize];
        x.push(dot(q, ax));
        x.push(dot(q, ay));
    }
    if n <= 3 {
        return verts.iter().enumerate().map(|(i, &v)| (v, [x[2 * i], x[2 * i + 1]])).collect();
    }
    // Triangles with their conformal rows.
    struct Row {
        idx: [usize; 3],
        wr: [f32; 3],
        wi: [f32; 3],
    }
    let mut rows = Vec::new();
    for &f in faces {
        let c = p.face(f);
        for t in triangulate(&p.positions, c) {
            let ids = [index[&c[t[0]]], index[&c[t[1]]], index[&c[t[2]]]];
            let (p0, p1, p2) = (p.positions[c[t[0]] as usize], p.positions[c[t[1]] as usize], p.positions[c[t[2]] as usize]);
            let e1 = sub(p1, p0);
            let e2 = sub(p2, p0);
            let l1 = length(e1);
            if l1 < 1e-12 {
                continue;
            }
            let u = scale(e1, 1.0 / l1);
            let nn = normalise(cross(e1, e2));
            if length(nn) < 0.5 {
                continue;
            }
            let w = cross(nn, u);
            let z = [[0.0f32, 0.0], [l1, 0.0], [dot(e2, u), dot(e2, w)]];
            let area = 0.5 * l1 * dot(e2, w);
            if area <= 1e-12 {
                continue;
            }
            let d = (2.0 * area).sqrt();
            let wr = [(z[2][0] - z[1][0]) / d, (z[0][0] - z[2][0]) / d, (z[1][0] - z[0][0]) / d];
            let wi = [(z[2][1] - z[1][1]) / d, (z[0][1] - z[2][1]) / d, (z[1][1] - z[0][1]) / d];
            rows.push(Row { idx: ids, wr, wi });
        }
    }
    // Pin the two vertices farthest apart (along the starting layout).
    let (mut pa, mut pb) = (0usize, 0usize);
    let mut best = -1.0f32;
    let probe = (0..n).max_by(|&a, &b| x[2 * a].total_cmp(&x[2 * b])).unwrap_or(0);
    for k in 0..n {
        let d = (x[2 * k] - x[2 * probe]).powi(2) + (x[2 * k + 1] - x[2 * probe + 1]).powi(2);
        if d > best {
            best = d;
            pa = k;
        }
    }
    best = -1.0;
    for k in 0..n {
        let d = (x[2 * k] - x[2 * pa]).powi(2) + (x[2 * k + 1] - x[2 * pa + 1]).powi(2);
        if d > best {
            best = d;
            pb = k;
        }
    }
    let pinned = |k: usize| k == pa || k == pb;
    // A x: two equations per triangle (real and imaginary parts).
    let apply = |x: &[f32], out: &mut Vec<f32>| {
        out.clear();
        for r in &rows {
            let mut re = 0.0;
            let mut im = 0.0;
            for j in 0..3 {
                let (u, v) = (x[2 * r.idx[j]], x[2 * r.idx[j] + 1]);
                re += r.wr[j] * u - r.wi[j] * v;
                im += r.wi[j] * u + r.wr[j] * v;
            }
            out.push(re);
            out.push(im);
        }
    };
    let apply_t = |y: &[f32], out: &mut Vec<f32>| {
        out.iter_mut().for_each(|o| *o = 0.0);
        for (k, r) in rows.iter().enumerate() {
            let (re, im) = (y[2 * k], y[2 * k + 1]);
            for j in 0..3 {
                let i = r.idx[j];
                out[2 * i] += r.wr[j] * re + r.wi[j] * im;
                out[2 * i + 1] += -r.wi[j] * re + r.wr[j] * im;
            }
        }
    };
    // Conjugate gradients on AᵀA x = 0 with the pinned unknowns fixed.
    let mut ax_buf = Vec::with_capacity(rows.len() * 2);
    let mut g = vec![0.0f32; 2 * n];
    let grad = |x: &[f32], ax_buf: &mut Vec<f32>, g: &mut Vec<f32>| {
        apply(x, ax_buf);
        apply_t(ax_buf, g);
        for k in [pa, pb] {
            g[2 * k] = 0.0;
            g[2 * k + 1] = 0.0;
        }
    };
    grad(&x, &mut ax_buf, &mut g);
    let mut r: Vec<f32> = g.iter().map(|v| -v).collect();
    let mut d = r.clone();
    let mut rr: f64 = r.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    let r0 = rr.max(1e-30);
    let mut ad = vec![0.0f32; 2 * n];
    for _ in 0..(4 * n).clamp(50, 3000) {
        if rr < r0 * 1e-10 {
            break;
        }
        grad(&d, &mut ax_buf, &mut ad);
        let dad: f64 = d.iter().zip(&ad).map(|(a, b)| (*a as f64) * (*b as f64)).sum();
        if dad.abs() < 1e-30 {
            break;
        }
        let alpha = (rr / dad) as f32;
        for i in 0..2 * n {
            if pinned(i / 2) {
                continue;
            }
            x[i] += alpha * d[i];
            r[i] -= alpha * ad[i];
        }
        let rr2: f64 = r.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        let beta = (rr2 / rr) as f32;
        for i in 0..2 * n {
            d[i] = if pinned(i / 2) { 0.0 } else { r[i] + beta * d[i] };
        }
        rr = rr2;
    }
    verts.iter().enumerate().map(|(i, &v)| (v, [x[2 * i], x[2 * i + 1]])).collect()
}

/// A flattened piece in the layout.
struct Piece {
    faces: Vec<usize>,
    uv: HashMap<u32, [f32; 2]>,
    area3: f32,
}

fn piece_bounds(uv: &HashMap<u32, [f32; 2]>) -> ([f32; 2], [f32; 2]) {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for q in uv.values() {
        for i in 0..2 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    (lo, hi)
}

fn area2(p: &Poly, faces: &[usize], uv: &HashMap<u32, [f32; 2]>) -> f32 {
    let mut a = 0.0;
    for &f in faces {
        let c = p.face(f);
        for i in 1..c.len() - 1 {
            let (x, y, z) = (uv[&c[0]], uv[&c[i]], uv[&c[i + 1]]);
            a += 0.5 * ((y[0] - x[0]) * (z[1] - x[1]) - (y[1] - x[1]) * (z[0] - x[0]));
        }
    }
    a
}

/// Unwraps the mesh: new per-corner texture coordinates, and the seams
/// used marked on the mesh.
pub fn unwrap(p: &mut Poly, settings: &Unwrap) -> Quality {
    let (chart, cuts) = charts(p, &settings.seams);
    let nc = chart.iter().copied().max().map_or(0, |m| m as usize + 1);
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); nc];
    for (f, c) in chart.iter().enumerate() {
        groups[*c as usize].push(f);
    }
    let mut pieces: Vec<Piece> = Vec::with_capacity(nc);
    for faces in groups.into_iter().filter(|g| !g.is_empty()) {
        let mut uv = flatten(p, &faces);
        // Mirror if the flattening came out inside-out.
        if area2(p, &faces, &uv) < 0.0 {
            for q in uv.values_mut() {
                q[0] = -q[0];
            }
        }
        let area3: f32 = faces.iter().map(|&f| p.face_area(f)).sum();
        pieces.push(Piece { faces, uv, area3 });
    }
    // Scale each piece to its true size (even texel density).
    for pc in &mut pieces {
        let a2 = area2(p, &pc.faces, &pc.uv).abs().max(1e-12);
        let k = (pc.area3 / a2).sqrt();
        for q in pc.uv.values_mut() {
            *q = [q[0] * k, q[1] * k];
        }
        if settings.rotate {
            orient_min_box(&mut pc.uv);
        }
    }
    // Shelf packing, tallest first.
    let total: f32 = pieces.iter().map(|pc| pc.area3).sum::<f32>().max(1e-9);
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    let dims: Vec<[f32; 2]> = pieces.iter().map(|pc| {
        let (lo, hi) = piece_bounds(&pc.uv);
        [hi[0] - lo[0], hi[1] - lo[1]]
    }).collect();
    order.sort_by(|a, b| dims[*b][1].total_cmp(&dims[*a][1]));
    let mut side = total.sqrt() * 1.05;
    let max_w = dims.iter().map(|d| d[0]).fold(0.0f32, f32::max);
    side = side.max(max_w);
    let margin_world = |side: f32| settings.margin * side;
    let mut placed: Vec<[f32; 2]> = vec![[0.0; 2]; pieces.len()];
    let mut extent;
    loop {
        let m = margin_world(side);
        let (mut x, mut y, mut row_h) = (m, m, 0.0f32);
        let mut w_used = 0.0f32;
        for &i in &order {
            let d = dims[i];
            if x + d[0] + m > side && x > m {
                x = m;
                y += row_h + m;
                row_h = 0.0;
            }
            placed[i] = [x, y];
            x += d[0] + m;
            w_used = w_used.max(x);
            row_h = row_h.max(d[1]);
        }
        extent = (y + row_h + m).max(w_used);
        if extent <= side * 1.02 {
            break;
        }
        side *= 1.05;
    }
    let s = extent.max(side);
    // Write the corners.
    let mut uvs = vec![[0.0f32; 2]; p.corners.len()];
    for (i, pc) in pieces.iter().enumerate() {
        let (lo, _) = piece_bounds(&pc.uv);
        for &f in &pc.faces {
            for k in p.face_range(f) {
                let v = p.corners[k];
                let q = pc.uv[&v];
                uvs[k] = [(q[0] - lo[0] + placed[i][0]) / s, (q[1] - lo[1] + placed[i][1]) / s];
            }
        }
    }
    p.uvs = uvs;
    p.seams = cuts.into_iter().filter(|e| Topology::of(p).edge_faces.get(e).is_some_and(|f| f.len() == 2)).collect();
    let mut q = quality(p);
    q.charts = pieces.len();
    q
}

/// Turns a piece so its bounding box is smallest (trying each hull edge's
/// direction).
fn orient_min_box(uv: &mut HashMap<u32, [f32; 2]>) {
    let pts: Vec<[f32; 2]> = uv.values().copied().collect();
    let hull = convex_hull(&pts);
    let mut best = (f32::INFINITY, 0.0f32);
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let ang = (b[1] - a[1]).atan2(b[0] - a[0]);
        let (s, c) = (-ang).sin_cos();
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for q in &hull {
            let r = [q[0] * c - q[1] * s, q[0] * s + q[1] * c];
            for k in 0..2 {
                lo[k] = lo[k].min(r[k]);
                hi[k] = hi[k].max(r[k]);
            }
        }
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        // Prefer tall pieces lying flat (wider than high) for shelves.
        let a = w * h + if h > w { 1e-6 } else { 0.0 };
        if a < best.0 {
            best = (a, if h > w { -ang + std::f32::consts::FRAC_PI_2 } else { -ang });
        }
    }
    let (s, c) = best.1.sin_cos();
    for q in uv.values_mut() {
        *q = [q[0] * c - q[1] * s, q[0] * s + q[1] * c];
    }
}

fn convex_hull(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p = pts.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross2 = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<[f32; 2]> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross2(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<[f32; 2]> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross2(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Measures a layout's stretch and coverage.
pub fn quality(p: &Poly) -> Quality {
    let mut q = Quality::default();
    if !p.has_uvs() {
        return q;
    }
    let mut a3 = 0.0f32;
    let mut a2 = 0.0f32;
    let mut stretch_sum = 0.0f32;
    let mut worst = 1.0f32;
    let mut tris_info = Vec::new();
    for f in 0..p.faces() {
        let c = p.face(f);
        let u = p.face_uvs(f).unwrap();
        for t in triangulate(&p.positions, c) {
            let (p0, p1, p2) = (p.positions[c[t[0]] as usize], p.positions[c[t[1]] as usize], p.positions[c[t[2]] as usize]);
            let (u0, u1, u2) = (u[t[0]], u[t[1]], u[t[2]]);
            let area3 = 0.5 * length(cross(sub(p1, p0), sub(p2, p0)));
            let area_uv = 0.5 * ((u1[0] - u0[0]) * (u2[1] - u0[1]) - (u1[1] - u0[1]) * (u2[0] - u0[0]));
            if area_uv < 0.0 {
                q.flipped += 1;
            }
            a3 += area3;
            a2 += area_uv.abs();
            tris_info.push((p0, p1, p2, u0, u1, u2, area3, area_uv.abs()));
        }
    }
    q.coverage = a2.min(1.0);
    let ratio = a2 / a3.max(1e-12);
    for (p0, p1, p2, u0, u1, u2, area3, area_uv) in tris_info {
        if area3 <= 1e-12 || area_uv <= 1e-14 {
            continue;
        }
        // Jacobian singular values of the 3D -> 2D map.
        let e1 = sub(p1, p0);
        let e2 = sub(p2, p0);
        let l1 = length(e1);
        let ux = scale(e1, 1.0 / l1);
        let n = normalise(cross(e1, e2));
        let uy = cross(n, ux);
        let (x1, x2, y2) = (l1, dot(e2, ux), dot(e2, uy));
        let (s1, s2) = ([u1[0] - u0[0], u1[1] - u0[1]], [u2[0] - u0[0], u2[1] - u0[1]]);
        // d(uv)/dx, d(uv)/dy.
        let ss = [s1[0] / x1, s1[1] / x1];
        let st = [(s2[0] - ss[0] * x2) / y2, (s2[1] - ss[1] * x2) / y2];
        let a = ss[0] * ss[0] + ss[1] * ss[1];
        let b = ss[0] * st[0] + ss[1] * st[1];
        let c = st[0] * st[0] + st[1] * st[1];
        let disc = ((a - c) * (a - c) + 4.0 * b * b).sqrt();
        let big = ((a + c + disc) / 2.0).sqrt();
        let small = ((a + c - disc) / 2.0).max(1e-20).sqrt();
        worst = worst.max(big / small);
        let local = (area_uv / area3) / ratio;
        stretch_sum += area3 * if local > 1.0 { local } else { 1.0 / local.max(1e-6) };
    }
    q.stretch = stretch_sum / a3.max(1e-12);
    q.worst_shear = worst;
    q
}

/// Simple projections (for quick layouts, tiling materials, or terrain).
pub fn project(p: &mut Poly, kind: &str, size: f32) -> Result<(), String> {
    let s = size.max(1e-6);
    let (lo, hi) = p.bounds();
    let c = scale(studio_mesh::add(lo, hi), 0.5);
    let mut uvs = Vec::with_capacity(p.corners.len());
    for f in 0..p.faces() {
        let n = p.face_normal(f);
        for &v in p.face(f) {
            let q = p.positions[v as usize];
            let d = sub(q, c);
            let uv = match kind {
                "box" | "cube" | "triplanar" => {
                    let a = [n[0].abs(), n[1].abs(), n[2].abs()];
                    if a[0] >= a[1] && a[0] >= a[2] {
                        [q[2] / s, q[1] / s]
                    } else if a[1] >= a[2] {
                        [q[0] / s, q[2] / s]
                    } else {
                        [q[0] / s, q[1] / s]
                    }
                }
                "planar" | "top" => [q[0] / s, q[2] / s],
                "front" => [q[0] / s, q[1] / s],
                "cylinder" => [0.5 + d[2].atan2(d[0]) / std::f32::consts::TAU, q[1] / s],
                "sphere" => [0.5 + d[2].atan2(d[0]) / std::f32::consts::TAU, 0.5 + (d[1] / length(d).max(1e-9)).clamp(-1.0, 1.0).asin() / std::f32::consts::PI],
                _ => return Err(format!("projection \"{kind}\": box, planar, front, cylinder or sphere")),
            };
            uvs.push(uv);
        }
    }
    p.uvs = uvs;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn a_cube_unwraps_into_six_square_pieces_without_stretch() {
        let mut c = prim::cube([2.0; 3], 2);
        let q = unwrap(&mut c, &Unwrap::default());
        assert_eq!(q.charts, 6);
        assert_eq!(q.flipped, 0);
        assert!(q.stretch < 1.01, "{q:?}");
        assert!(q.worst_shear < 1.01, "{q:?}");
        assert!(q.coverage > 0.4, "{q:?}");
        assert!(c.uvs.iter().all(|u| u[0] >= 0.0 && u[0] <= 1.0 && u[1] >= 0.0 && u[1] <= 1.0));
    }

    #[test]
    fn a_sphere_with_seams_unwraps_with_low_distortion() {
        let mut s = prim::uv_sphere(10.0, 32, 16);
        let q = unwrap(&mut s, &Unwrap { seams: Seams::Smart(50.0), margin: 0.01, rotate: true });
        assert_eq!(q.flipped, 0, "{q:?}");
        assert!(q.stretch < 1.25, "{q:?}");
        // Pieces don't overlap: sample the layout.
        let mut cover = vec![0u8; 128 * 128];
        let mut overlaps = 0;
        for f in 0..s.faces() {
            let c = s.face(f);
            let u = s.face_uvs(f).unwrap();
            for t in triangulate(&s.positions, c) {
                let (a, b, d) = (u[t[0]], u[t[1]], u[t[2]]);
                for y in 0..128 {
                    for x in 0..128 {
                        let pnt = [(x as f32 + 0.5) / 128.0, (y as f32 + 0.5) / 128.0];
                        let e = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
                        let (w0, w1, w2) = (e(a, b, pnt), e(b, d, pnt), e(d, a, pnt));
                        if w0 > 1e-6 && w1 > 1e-6 && w2 > 1e-6 {
                            if cover[y * 128 + x] > 0 {
                                overlaps += 1;
                            }
                            cover[y * 128 + x] += 1;
                        }
                    }
                }
            }
        }
        assert!(overlaps < 20, "{overlaps} overlapping samples");
    }

    #[test]
    fn projections_cover_the_mesh() {
        let mut c = prim::cylinder(5.0, 5.0, 10.0, 16, 2, prim::Cap::Ngon);
        project(&mut c, "cylinder", 10.0).unwrap();
        assert!(c.has_uvs());
        assert!(project(&mut c, "spiral", 1.0).is_err());
    }
}
