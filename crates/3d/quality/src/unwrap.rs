// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Texture coordinates a texture artist would accept: the surface cut
//! into a few charts, each flattened with as little distortion as
//! possible, packed tightly at one texture detail everywhere.
//!
//! 1. Charts grow from seeds over faces that look the same way (within a
//!    cone round the chart's mean direction) and never across a material
//!    border, so seams fall where the surface turns or the material
//!    changes; tiny charts are merged into their neighbours.
//! 2. Each chart is flattened with least-squares conformal maps (Lévy et
//!    al. 2002), solved by conjugate gradients from the projection onto
//!    the chart's plane. A chart that comes out folded or stretched is
//!    split in two and flattened again.
//! 3. Charts are scaled to their real size (the same texture detail
//!    everywhere), turned to their smallest box and packed by a skyline
//!    (lowest place first), then the layout is scaled into the unit
//!    square with a margin round every chart.

use crate::geo::weld;
use std::collections::{BinaryHeap, HashMap, HashSet};
use studio_mesh::{Mesh, V3, add, cross, dot, length, normalise, scale, sub};

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Faces join a chart while they turn less than this from its mean (degrees).
    pub cone: f32,
    /// The texture size the margin is measured for.
    pub texture: u32,
    /// Margin round every chart, in texture pixels.
    pub margin: f32,
    /// Charts stretched more than this are split (L2 stretch).
    pub max_stretch: f32,
}

impl Default for Params {
    fn default() -> Params {
        Params { cone: 66.0, texture: 2048, margin: 4.0, max_stretch: 1.2 }
    }
}

fn face_adjacency(tris: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (t, c) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut adj = vec![Vec::new(); tris.len()];
    for fs in by_edge.values() {
        for i in 0..fs.len() {
            for j in 0..fs.len() {
                if i != j && fs[i] != fs[j] && !adj[fs[i]].contains(&fs[j]) {
                    adj[fs[i]].push(fs[j]);
                }
            }
        }
    }
    adj
}

#[derive(PartialEq)]
struct Item(f32, usize, usize);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // Lowest cost first.
        o.0.total_cmp(&self.0)
    }
}

/// Faces into charts (a chart id per face).
fn charts(normals: &[V3], areas: &[f32], mats: &[u32], adj: &[Vec<usize>], cone: f32) -> Vec<usize> {
    let n = normals.len();
    let cos = cone.to_radians().cos();
    let mut chart = vec![usize::MAX; n];
    let mut sums: Vec<V3> = Vec::new();
    // Seeds: the largest unassigned face each time.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| areas[b].total_cmp(&areas[a]));
    for &seed in &order {
        if chart[seed] != usize::MAX {
            continue;
        }
        let c = sums.len();
        sums.push(scale(normals[seed], areas[seed]));
        chart[seed] = c;
        let mut heap = BinaryHeap::new();
        for &o in &adj[seed] {
            heap.push(Item(0.0, o, seed));
        }
        while let Some(Item(_, f, _)) = heap.pop() {
            if chart[f] != usize::MAX || mats[f] != mats[seed] {
                continue;
            }
            let mean = normalise(sums[c]);
            // Faces with no area go wherever they touch.
            if dot(normals[f], normals[f]) > 0.5 && dot(normals[f], mean) < cos {
                continue;
            }
            chart[f] = c;
            sums[c] = add(sums[c], scale(normals[f], areas[f]));
            let mean = normalise(sums[c]);
            for &o in &adj[f] {
                if chart[o] == usize::MAX {
                    heap.push(Item(1.0 - dot(normals[o], mean), o, f));
                }
            }
        }
    }
    // Merge charts under 0.5% of the area into the neighbour they share most edge with.
    let total: f32 = areas.iter().sum();
    loop {
        let k = sums.len();
        let mut area = vec![0f32; k];
        for f in 0..n {
            area[chart[f]] += areas[f];
        }
        let mut merged = false;
        for c in 0..k {
            if area[c] == 0.0 || area[c] >= total * 0.005 {
                continue;
            }
            let mut shared: HashMap<usize, usize> = HashMap::new();
            for f in 0..n {
                if chart[f] == c {
                    for &o in &adj[f] {
                        if chart[o] != c && mats[o] == mats[f] {
                            *shared.entry(chart[o]).or_default() += 1;
                        }
                    }
                }
            }
            if let Some((&to, _)) = shared.iter().max_by_key(|(_, n)| **n) {
                for x in chart.iter_mut() {
                    if *x == c {
                        *x = to;
                    }
                }
                merged = true;
            }
        }
        if !merged {
            break;
        }
    }
    // Dense ids.
    let mut remap: HashMap<usize, usize> = HashMap::new();
    for c in chart.iter_mut() {
        let next = remap.len();
        *c = *remap.entry(*c).or_insert(next);
    }
    chart
}

/// Conformal flattening of one chart: positions → (u, v) per local vertex.
fn lscm(points: &[V3], tris: &[[usize; 3]]) -> Vec<[f64; 2]> {
    let nv = points.len();
    // Start from the projection onto the chart's best plane.
    let mut nsum = [0.0f32; 3];
    for t in tris {
        nsum = add(nsum, cross(sub(points[t[1]], points[t[0]]), sub(points[t[2]], points[t[0]])));
    }
    let nrm = normalise(nsum);
    let helper = if nrm[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let ax = normalise(cross(helper, nrm));
    let ay = cross(nrm, ax);
    let mut uv: Vec<[f64; 2]> = points.iter().map(|p| [dot(*p, ax) as f64, dot(*p, ay) as f64]).collect();
    if nv < 3 || tris.is_empty() {
        return uv;
    }
    // Pins: the two points farthest apart along the projection's long axis.
    let (mut pa, mut pb) = (0usize, 0usize);
    for v in 0..nv {
        if uv[v][0] < uv[pa][0] {
            pa = v;
        }
        if uv[v][0] > uv[pb][0] {
            pb = v;
        }
    }
    if pa == pb {
        return uv;
    }
    // Rows: two per triangle; coefficients on (u_j, v_j).
    struct Row {
        idx: [usize; 3],
        a: [f64; 3],
        b: [f64; 3],
    }
    let mut rows = Vec::with_capacity(tris.len());
    for t in tris {
        let (p0, p1, p2) = (points[t[0]], points[t[1]], points[t[2]]);
        let e1 = sub(p1, p0);
        let e2 = sub(p2, p0);
        let l1 = length(e1);
        let n = cross(e1, e2);
        let area2 = length(n) as f64;
        if l1 < 1e-12 || area2 < 1e-12 {
            continue;
        }
        let x1 = l1 as f64;
        let ex = normalise(e1);
        let ey = normalise(cross(normalise(n), ex));
        let (x2, y2) = (dot(e2, ex) as f64, dot(e2, ey) as f64);
        let xs = [0.0, x1, x2];
        let ys = [0.0, 0.0, y2];
        let w = 1.0 / area2.sqrt();
        let mut a = [0.0; 3];
        let mut b = [0.0; 3];
        for j in 0..3 {
            let (k, l) = ((j + 1) % 3, (j + 2) % 3);
            a[j] = (xs[l] - xs[k]) * w;
            b[j] = (ys[l] - ys[k]) * w;
        }
        rows.push(Row { idx: [t[0], t[1], t[2]], a, b });
    }
    // Unknowns: u, v of every vertex; pins held at their projected place.
    let pinned = |v: usize| v == pa || v == pb;
    let apply = |x: &[f64], out: &mut [f64]| {
        // out = Aᵀ A x over the free unknowns (pinned entries ignored).
        for o in out.iter_mut() {
            *o = 0.0;
        }
        for r in &rows {
            // Real row: Σ a_j u_j − b_j v_j ; imaginary row: Σ b_j u_j + a_j v_j.
            let mut re = 0.0;
            let mut im = 0.0;
            for j in 0..3 {
                let v = r.idx[j];
                let (u, vv) = (x[2 * v], x[2 * v + 1]);
                re += r.a[j] * u - r.b[j] * vv;
                im += r.b[j] * u + r.a[j] * vv;
            }
            for j in 0..3 {
                let v = r.idx[j];
                out[2 * v] += r.a[j] * re + r.b[j] * im;
                out[2 * v + 1] += -r.b[j] * re + r.a[j] * im;
            }
        }
    };
    let mut x: Vec<f64> = uv.iter().flat_map(|p| [p[0], p[1]]).collect();
    let n2 = 2 * nv;
    let mut ax_buf = vec![0.0; n2];
    // r = −A x (the system is homogeneous; pins make it not trivially zero).
    apply(&x, &mut ax_buf);
    let mut r: Vec<f64> = ax_buf.iter().map(|v| -v).collect();
    for v in [pa, pb] {
        r[2 * v] = 0.0;
        r[2 * v + 1] = 0.0;
    }
    // Jacobi preconditioner.
    let mut diag = vec![0.0f64; n2];
    for row in &rows {
        for j in 0..3 {
            let v = row.idx[j];
            diag[2 * v] += row.a[j] * row.a[j] + row.b[j] * row.b[j];
            diag[2 * v + 1] += row.a[j] * row.a[j] + row.b[j] * row.b[j];
        }
    }
    let precond = |r: &[f64]| -> Vec<f64> { r.iter().zip(&diag).enumerate().map(|(i, (v, d))| if pinned(i / 2) { 0.0 } else { v / d.max(1e-12) }).collect() };
    let mut z = precond(&r);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let r0 = rz.abs().max(1e-30);
    for _ in 0..(4 * nv).clamp(200, 4000) {
        apply(&p, &mut ax_buf);
        for v in [pa, pb] {
            ax_buf[2 * v] = 0.0;
            ax_buf[2 * v + 1] = 0.0;
        }
        let pap: f64 = p.iter().zip(&ax_buf).map(|(a, b)| a * b).sum();
        if pap.abs() < 1e-30 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n2 {
            x[i] += alpha * p[i];
            r[i] -= alpha * ax_buf[i];
        }
        z = precond(&r);
        let rz_new: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        if rz_new.abs() < r0 * 1e-14 {
            break;
        }
        let beta = rz_new / rz;
        rz = rz_new;
        for i in 0..n2 {
            p[i] = z[i] + beta * p[i];
        }
    }
    for v in 0..nv {
        uv[v] = [x[2 * v], x[2 * v + 1]];
    }
    uv
}

/// Stretch (L2, isometric-normalised) and folds of a flattened chart.
fn chart_quality(points: &[V3], tris: &[[usize; 3]], uv: &[[f64; 2]]) -> (f64, usize) {
    let (mut l2, mut a3t, mut auvt, mut flips) = (0.0, 0.0, 0.0, 0usize);
    let mut sign = 0.0;
    for t in tris {
        let (u0, u1, u2) = (uv[t[0]], uv[t[1]], uv[t[2]]);
        let s = 0.5 * ((u1[0] - u0[0]) * (u2[1] - u0[1]) - (u2[0] - u0[0]) * (u1[1] - u0[1]));
        sign += s;
    }
    for t in tris {
        let (p0, p1, p2) = (points[t[0]], points[t[1]], points[t[2]]);
        let a3 = 0.5 * length(cross(sub(p1, p0), sub(p2, p0))) as f64;
        let (u0, u1, u2) = (uv[t[0]], uv[t[1]], uv[t[2]]);
        let s = 0.5 * ((u1[0] - u0[0]) * (u2[1] - u0[1]) - (u2[0] - u0[0]) * (u1[1] - u0[1]));
        if a3 < 1e-12 {
            continue;
        }
        if s * sign <= 0.0 {
            flips += 1;
            continue;
        }
        let two_a = 2.0 * s;
        let ss: [f64; 3] = std::array::from_fn(|i| (p0[i] as f64 * (u1[1] - u2[1]) + p1[i] as f64 * (u2[1] - u0[1]) + p2[i] as f64 * (u0[1] - u1[1])) / two_a);
        let st: [f64; 3] = std::array::from_fn(|i| (p0[i] as f64 * (u2[0] - u1[0]) + p1[i] as f64 * (u0[0] - u2[0]) + p2[i] as f64 * (u1[0] - u0[0])) / two_a);
        l2 += (ss.iter().map(|x| x * x).sum::<f64>() + st.iter().map(|x| x * x).sum::<f64>()) * 0.5 * a3;
        a3t += a3;
        auvt += s.abs();
    }
    if a3t <= 0.0 {
        return (1.0, flips);
    }
    ((l2 / a3t).sqrt() * (auvt / a3t).sqrt(), flips)
}

struct Chart {
    /// Welded vertex ids.
    verts: Vec<u32>,
    /// Faces (mesh triangle ids).
    faces: Vec<usize>,
    uv: Vec<[f64; 2]>,
}

/// Flattens the faces of one chart, splitting it while it folds or stretches.
fn flatten(points: &[V3], tris: &[[u32; 3]], faces: Vec<usize>, max_stretch: f32, depth: usize, out: &mut Vec<Chart>) {
    let mut local: HashMap<u32, usize> = HashMap::new();
    let mut verts = Vec::new();
    let ltris: Vec<[usize; 3]> = faces
        .iter()
        .map(|&f| {
            tris[f].map(|v| {
                *local.entry(v).or_insert_with(|| {
                    verts.push(v);
                    verts.len() - 1
                })
            })
        })
        .collect();
    let pts: Vec<V3> = verts.iter().map(|&v| points[v as usize]).collect();
    let uv = lscm(&pts, &ltris);
    let (stretch, flips) = chart_quality(&pts, &ltris, &uv);
    if std::env::var("UNWRAP_DEBUG").is_ok() { eprintln!("chart {} faces: stretch {stretch:.3} flips {flips}", faces.len()); }
    if (stretch > max_stretch as f64 || flips > 0 || !is_disc(tris, &faces)) && depth < 6 && faces.len() > 8 {
        // Split through the middle, across whichever principal direction
        // leaves the fewest pieces that are not discs (a tube must be cut
        // along its length, a cap across).
        let cents: Vec<V3> = ltris.iter().map(|t| scale(add(add(pts[t[0]], pts[t[1]]), pts[t[2]]), 1.0 / 3.0)).collect();
        let mid = scale(cents.iter().fold([0.0; 3], |s, c| add(s, *c)), 1.0 / cents.len() as f32);
        let mut cov = [[0f32; 3]; 3];
        for c in &cents {
            let d = sub(*c, mid);
            for r in 0..3 {
                for k in 0..3 {
                    cov[r][k] += d[r] * d[k];
                }
            }
        }
        // The three principal directions (power iteration with deflation).
        let mut dirs: Vec<V3> = Vec::new();
        for seed in [[1.0, 0.7, 0.3], [0.3, 1.0, 0.6], [0.5, 0.2, 1.0]] {
            let mut dir: V3 = seed;
            for _ in 0..40 {
                dir = [dot(cov[0], dir), dot(cov[1], dir), dot(cov[2], dir)];
                for d in &dirs {
                    dir = sub(dir, scale(*d, dot(dir, *d)));
                }
                dir = normalise(dir);
            }
            if dot(dir, dir) > 0.5 {
                dirs.push(dir);
            }
        }
        let mut best: Option<(usize, Vec<Vec<usize>>)> = None;
        for dir in dirs {
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for (&f, c) in faces.iter().zip(&cents) {
                if dot(sub(*c, mid), dir) < 0.0 { a.push(f) } else { b.push(f) }
            }
            if a.is_empty() || b.is_empty() {
                continue;
            }
            let parts: Vec<Vec<usize>> = connected(tris, a).into_iter().chain(connected(tris, b)).collect();
            let cost = parts.iter().filter(|p| !is_disc(tris, p)).count() * 100 + parts.len();
            if best.as_ref().is_none_or(|b| cost < b.0) {
                best = Some((cost, parts));
            }
        }
        if let Some((_, parts)) = best {
            for part in parts {
                flatten(points, tris, part, max_stretch, depth + 1, out);
            }
            return;
        }
    }
    out.push(Chart { verts, faces, uv });
}

/// Whether faces make a disc (one piece, one border, no handles): what
/// flattens without a cut.
fn is_disc(tris: &[[u32; 3]], faces: &[usize]) -> bool {
    let mut verts = HashSet::new();
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    for &f in faces {
        let c = tris[f];
        for k in 0..3 {
            verts.insert(c[k]);
            let (a, b) = (c[k], c[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let euler = verts.len() as i64 - edges.len() as i64 + faces.len() as i64;
    euler == 1 && edges.values().any(|&n| n == 1)
}

/// Faces split into edge-connected pieces.
fn connected(tris: &[[u32; 3]], faces: Vec<usize>) -> Vec<Vec<usize>> {
    let set: HashSet<usize> = faces.iter().copied().collect();
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for &f in &faces {
        let c = tris[f];
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(f);
        }
    }
    let mut seen: HashSet<usize> = HashSet::new();
    let mut parts = Vec::new();
    for &f in &faces {
        if seen.contains(&f) {
            continue;
        }
        let mut part = Vec::new();
        let mut st = vec![f];
        seen.insert(f);
        while let Some(x) = st.pop() {
            part.push(x);
            let c = tris[x];
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                for &o in &by_edge[&(a.min(b), a.max(b))] {
                    if set.contains(&o) && seen.insert(o) {
                        st.push(o);
                    }
                }
            }
        }
        parts.push(part);
    }
    parts
}

/// Convex hull (monotone chain) of 2D points.
fn hull(pts: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut p: Vec<[f64; 2]> = pts.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross2 = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross2(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
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

/// Turns a chart to its smallest box (rotating calipers over hull edges)
/// and moves it to the origin; returns its width and height.
fn orient(uv: &mut [[f64; 2]]) -> (f64, f64) {
    let h = hull(uv);
    let mut best = (f64::INFINITY, 0.0f64);
    for k in 0..h.len().max(1) {
        let (a, b) = (h[k % h.len().max(1)], h[(k + 1) % h.len().max(1)]);
        let ang = (b[1] - a[1]).atan2(b[0] - a[0]);
        let (s, c) = (-ang).sin_cos();
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &h {
            let q = [p[0] * c - p[1] * s, p[0] * s + p[1] * c];
            for i in 0..2 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
        if area < best.0 {
            best = (area, -ang);
        }
    }
    let (s, c) = best.1.sin_cos();
    for p in uv.iter_mut() {
        *p = [p[0] * c - p[1] * s, p[0] * s + p[1] * c];
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in uv.iter() {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    // Tall boxes lie down (packs better on a skyline).
    let (w, hh) = (hi[0] - lo[0], hi[1] - lo[1]);
    if hh > w {
        for p in uv.iter_mut() {
            *p = [p[1] - lo[1], w - (p[0] - lo[0])];
        }
        (hh, w)
    } else {
        for p in uv.iter_mut() {
            *p = [p[0] - lo[0], p[1] - lo[1]];
        }
        (w, hh)
    }
}

/// Skyline packing of boxes into a strip `width` wide: lowest place
/// first. Returns each box's corner and the height used.
fn skyline(boxes: &[(f64, f64)], width: f64) -> (Vec<[f64; 2]>, f64) {
    // Skyline: (x, y, w) segments.
    let mut sky: Vec<(f64, f64, f64)> = vec![(0.0, 0.0, width)];
    let mut at = vec![[0.0; 2]; boxes.len()];
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&a, &b| (boxes[b].1 * boxes[b].0).total_cmp(&(boxes[a].1 * boxes[a].0)));
    let mut top = 0.0f64;
    for &i in &order {
        let (w, h) = boxes[i];
        let mut best: Option<(f64, f64, usize)> = None;
        for s in 0..sky.len() {
            let x = sky[s].0;
            if x + w > width + 1e-9 {
                break;
            }
            // Highest skyline under [x, x + w).
            let mut y: f64 = 0.0;
            let mut k = s;
            while k < sky.len() && sky[k].0 < x + w - 1e-12 {
                y = y.max(sky[k].1);
                k += 1;
            }
            if best.is_none_or(|b| y < b.1 || (y == b.1 && x < b.0)) {
                best = Some((x, y, s));
            }
        }
        let (x, y) = match best {
            Some((x, y, _)) => (x, y),
            None => (0.0, top),
        };
        at[i] = [x, y];
        top = top.max(y + h);
        // Raise the skyline over [x, x + w).
        let mut next: Vec<(f64, f64, f64)> = Vec::new();
        for &(sx, sy, sw) in &sky {
            let (s0, s1) = (sx, sx + sw);
            if s1 <= x || s0 >= x + w {
                next.push((sx, sy, sw));
                continue;
            }
            if s0 < x {
                next.push((s0, sy, x - s0));
            }
            if s1 > x + w {
                next.push((x + w, sy, s1 - (x + w)));
            }
        }
        next.push((x, y + h, w));
        next.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Merge level neighbours.
        let mut merged: Vec<(f64, f64, f64)> = Vec::new();
        for s in next {
            if let Some(l) = merged.last_mut()
                && (l.1 - s.1).abs() < 1e-12
                && (l.0 + l.2 - s.0).abs() < 1e-9
            {
                l.2 += s.2;
                continue;
            }
            merged.push(s);
        }
        sky = merged;
    }
    (at, top)
}

/// Gives `m` new texture coordinates (splitting vertices along the seams).
/// Returns the number of charts.
pub fn unwrap(m: &mut Mesh, p: &Params) -> usize {
    let n = m.triangles();
    if n == 0 {
        return 0;
    }
    let w = weld(m);
    let normals: Vec<V3> = (0..n).map(|t| m.face_normal(t)).collect();
    let areas: Vec<f32> = (0..n).map(|t| crate::geo::triangle_area(m, t)).collect();
    let mats = m.tri_materials();
    let adj = face_adjacency(&w.tris);
    let chart_of = charts(&normals, &areas, &mats, &adj, p.cone);
    let k = chart_of.iter().copied().max().map_or(0, |c| c + 1);
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); k];
    for (f, &c) in chart_of.iter().enumerate() {
        groups[c].push(f);
    }
    let mut done: Vec<Chart> = Vec::new();
    if std::env::var("UNWRAP_DEBUG").is_ok() { eprintln!("charts grown: {k}"); }
    for g in groups {
        for part in connected(&w.tris, g) {
            flatten(&w.points, &w.tris, part, p.max_stretch, 0, &mut done);
        }
    }
    // Real size: scale each chart so its texture area equals its surface area.
    let mut boxes = Vec::with_capacity(done.len());
    let mut total = 0.0f64;
    for c in &mut done {
        let pts: Vec<V3> = c.verts.iter().map(|&v| w.points[v as usize]).collect();
        let local: HashMap<u32, usize> = c.verts.iter().enumerate().map(|(i, &v)| (v, i)).collect();
        let (mut a3, mut auv) = (0.0f64, 0.0f64);
        for &f in &c.faces {
            let t = w.tris[f].map(|v| local[&v]);
            a3 += 0.5 * length(cross(sub(pts[t[1]], pts[t[0]]), sub(pts[t[2]], pts[t[0]]))) as f64;
            let (u0, u1, u2) = (c.uv[t[0]], c.uv[t[1]], c.uv[t[2]]);
            auv += 0.5 * ((u1[0] - u0[0]) * (u2[1] - u0[1]) - (u2[0] - u0[0]) * (u1[1] - u0[1]));
        }
        // A mirrored chart is turned over so it reads the right way.
        if auv < 0.0 {
            for q in c.uv.iter_mut() {
                q[0] = -q[0];
            }
        }
        let s = (a3 / auv.abs().max(1e-18)).sqrt();
        for q in c.uv.iter_mut() {
            q[0] *= s;
            q[1] *= s;
        }
        total += a3;
        boxes.push(orient(&mut c.uv));
    }
    // Margin in layout units: the layout ends up about √(total / 0.7) wide.
    let side_guess = (total / 0.7).sqrt();
    let margin = side_guess * (p.margin as f64 / p.texture.max(1) as f64) * 2.0;
    let padded: Vec<(f64, f64)> = boxes.iter().map(|b| (b.0 + margin, b.1 + margin)).collect();
    // Try strip widths round the square and keep the squarest tight result.
    let widest = padded.iter().map(|b| b.0).fold(0.0, f64::max);
    let mut best: Option<(f64, Vec<[f64; 2]>)> = None;
    for k in 0..8 {
        let width = (side_guess * (0.85 + 0.06 * k as f64)).max(widest);
        let (at, height) = skyline(&padded, width);
        let side = width.max(height);
        if best.as_ref().is_none_or(|b| side < b.0) {
            best = Some((side, at));
        }
    }
    let (side, at) = best.unwrap_or((1.0, vec![[0.0; 2]; done.len()]));
    // New vertices: one per (welded vertex, chart).
    // A copy keeps everything else the model carries (rig, groups, materials).
    let mut out = m.clone();
    out.positions.clear();
    out.normals.clear();
    out.uvs.clear();
    let mut first_of: Vec<u32> = vec![u32::MAX; m.positions.len()];
    for (i, &pid) in w.ids.iter().enumerate() {
        if first_of[pid as usize] == u32::MAX {
            first_of[pid as usize] = i as u32;
        }
    }
    let mut made: HashMap<(u32, usize), u32> = HashMap::new();
    let mut idx = vec![0u32; m.indices.len()];
    for (ci, c) in done.iter().enumerate() {
        let local: HashMap<u32, usize> = c.verts.iter().enumerate().map(|(i, &v)| (v, i)).collect();
        for &f in &c.faces {
            let corners = m.tri(f);
            for (k, &orig) in corners.iter().enumerate() {
                let pid = w.ids[orig];
                let li = local[&pid];
                let id = *made.entry((pid, ci)).or_insert_with(|| {
                    let src = orig;
                    out.positions.push(m.positions[src]);
                    out.normals.push(m.normals.get(src).copied().unwrap_or([0.0; 3]));
                    let q = c.uv[li];
                    out.uvs.push([((q[0] + at[ci][0] + margin * 0.5) / side) as f32, ((q[1] + at[ci][1] + margin * 0.5) / side) as f32]);
                    (out.positions.len() - 1) as u32
                });
                idx[3 * f + k] = id;
            }
        }
    }
    out.indices = idx;
    *m = out;
    done.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uv::analyse;

    #[test]
    fn sphere_unwraps_cleanly() {
        let mut m = studio_mesh::prim::by_name("sphere", 200.0, 4).unwrap();
        let charts = unwrap(&mut m, &Params::default());
        let q = analyse(&m);
        assert_eq!(q.islands, charts);
        assert!(q.islands <= 24, "{q:?}");
        assert!(q.stretch < 1.15, "{q:?}");
        assert!(q.overlap < 0.01 && q.flipped == 0.0 && q.outside == 0.0, "{q:?}");
        assert!(q.packing > 0.45, "{q:?}");
        assert!(q.density_cov < 0.3, "{q:?}");
    }

    #[test]
    fn cube_unwraps_into_its_faces() {
        let mut m = studio_mesh::prim::by_name("cube", 100.0, 0).unwrap();
        unwrap(&mut m, &Params::default());
        let q = analyse(&m);
        assert!(q.stretch < 1.01, "{q:?}");
        assert!(q.islands <= 6, "{q:?}");
    }
}
