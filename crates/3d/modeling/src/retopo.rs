// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic retopology: a dense or messy surface (a sculpt, a scan, a
//! generated model) rebuilt as clean, even quads that follow the form —
//! what animation, UVs and subdivision need.
//!
//! Field-aligned quad meshing after Jakob, Tarini, Panozzo and
//! Sorkine-Hornung, "Instant Field-Aligned Meshes" (2015): a smooth
//! four-way direction field over the surface (aligned to sharp edges), a
//! position field that places a square lattice of the wanted size along
//! it, and quads read off where the lattice crosses the surface. Where the
//! lattice can't be read cleanly the result falls back to even voxel quads.

use crate::poly::{Poly, Topology};
use crate::spatial::Tree;
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, add, cross, dot, length, normalise, scale, sub};

/// The surface samples the fields live on.
struct Graph {
    pos: Vec<V3>,
    nrm: Vec<V3>,
    adj: Vec<Vec<u32>>,
    /// Directions sharp edges pin the field to (per vertex), if any.
    pin: Vec<Option<V3>>,
}

fn graph_of(p: &Poly, feature_degrees: f32) -> Graph {
    let t = Topology::of(p);
    let n = p.positions.len();
    let mut nrm = vec![[0.0; 3]; n];
    for f in 0..p.faces() {
        let a = p.face_area_vector(f);
        for &v in p.face(f) {
            nrm[v as usize] = add(nrm[v as usize], a);
        }
    }
    let nrm: Vec<V3> = nrm.into_iter().map(normalise).collect();
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    let c = feature_degrees.to_radians().cos();
    let mut pin: Vec<Option<V3>> = vec![None; n];
    for (e, fs) in &t.edge_faces {
        adj[e.0 as usize].push(e.1);
        adj[e.1 as usize].push(e.0);
        let sharp = fs.len() != 2 || dot(p.face_normal(fs[0] as usize), p.face_normal(fs[1] as usize)) < c || p.sharp.contains(e);
        if sharp {
            let d = normalise(sub(p.positions[e.1 as usize], p.positions[e.0 as usize]));
            for v in [e.0, e.1] {
                pin[v as usize] = Some(d);
            }
        }
    }
    Graph { pos: p.positions.clone(), nrm, adj, pin }
}

/// The closest of the four turns of `o2` (about `n2`) to `o1`, carried
/// into `n1`'s tangent plane.
fn rosy4(o1: V3, n1: V3, o2: V3, n2: V3) -> V3 {
    let t2 = cross(n2, o2);
    let cands = [o2, t2, scale(o2, -1.0), scale(t2, -1.0)];
    let mut best = cands[0];
    let mut bd = f32::NEG_INFINITY;
    for c in cands {
        let d = dot(c, o1);
        if d > bd {
            bd = d;
            best = c;
        }
    }
    normalise(sub(best, scale(n1, dot(best, n1))))
}

fn tangent(n: V3, d: V3) -> V3 {
    let t = sub(d, scale(n, dot(d, n)));
    if length(t) < 1e-6 {
        let h = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        normalise(cross(n, h))
    } else {
        normalise(t)
    }
}

fn orientation_field(g: &Graph, init: Option<Vec<V3>>, iterations: usize) -> Vec<V3> {
    let n = g.pos.len();
    // Start along the "latitude" of an up axis: a reasonable global guess.
    let mut o: Vec<V3> = if let Some(o) = init { o } else { (0..n).map(|i| g.pin[i].map(|d| tangent(g.nrm[i], d)).unwrap_or_else(|| tangent(g.nrm[i], cross([0.0, 1.0, 0.0], g.nrm[i])))).collect() };
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = 0x2545_f491u64;
    for it in 0..iterations {
        // Shuffle a little each pass so the sweep has no direction.
        if it % 8 == 0 {
            for i in (1..n).rev() {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                order.swap(i, (rng % (i as u64 + 1)) as usize);
            }
        }
        for &i in &order {
            if let Some(d) = g.pin[i] {
                o[i] = tangent(g.nrm[i], d);
                continue;
            }
            let mut acc = o[i];
            let mut w = 1.0;
            for &j in &g.adj[i] {
                let j = j as usize;
                let c = rosy4(acc, g.nrm[i], o[j], g.nrm[j]);
                acc = normalise(add(scale(acc, w), c));
                w += 1.0;
            }
            o[i] = tangent(g.nrm[i], acc);
        }
    }
    o
}

/// The lattice point (spacing `h`, axes `o`, `n × o`) through `p` nearest
/// `q`.
fn lattice_near(p: V3, o: V3, n: V3, q: V3, h: f32) -> V3 {
    let t = cross(n, o);
    let d = sub(q, p);
    let (a, b) = ((dot(d, o) / h).round(), (dot(d, t) / h).round());
    add(p, add(scale(o, a * h), scale(t, b * h)))
}

/// The pair of lattice points (one from each lattice) near `m` that lie
/// closest together: each lattice's cell corner below `m` and the three
/// around it are tried.
#[allow(clippy::too_many_arguments)]
fn lattice_pair(p1: V3, o1: V3, n1: V3, p2: V3, o2: V3, n2: V3, m: V3, h: f32) -> (V3, V3) {
    let corners = |p: V3, o: V3, n: V3| -> [V3; 4] {
        let t = cross(n, o);
        let d = sub(m, p);
        let (a, b) = ((dot(d, o) / h).floor(), (dot(d, t) / h).floor());
        let base = add(p, add(scale(o, a * h), scale(t, b * h)));
        [base, add(base, scale(o, h)), add(base, scale(t, h)), add(add(base, scale(o, h)), scale(t, h))]
    };
    let (c1, c2) = (corners(p1, o1, n1), corners(p2, o2, n2));
    let mut best = (f32::INFINITY, c1[0], c2[0]);
    for a in c1 {
        for b in c2 {
            let d = length(sub(a, b));
            if d < best.0 {
                best = (d, a, b);
            }
        }
    }
    (best.1, best.2)
}

fn position_field(g: &Graph, o: &[V3], h: f32, init: Option<Vec<V3>>, iterations: usize) -> Vec<V3> {
    let n = g.pos.len();
    let mut p: Vec<V3> = init.unwrap_or_else(|| g.pos.clone());
    for _ in 0..iterations {
        for i in 0..n {
            let (ni, oi) = (g.nrm[i], o[i]);
            let mut acc = p[i];
            let mut w = 1.0f32;
            for &j in &g.adj[i] {
                let j = j as usize;
                let oj = rosy4(oi, ni, o[j], g.nrm[j]);
                let m = scale(add(g.pos[i], g.pos[j]), 0.5);
                let (pi, pj) = lattice_pair(acc, oi, ni, p[j], oj, g.nrm[j], m, h);
                let cand = add(acc, sub(pj, pi));
                acc = scale(add(scale(acc, w), cand), 1.0 / (w + 1.0));
                w += 1.0;
            }
            // Back onto the tangent plane, re-centred near the vertex.
            acc = sub(acc, scale(ni, dot(sub(acc, g.pos[i]), ni)));
            p[i] = lattice_near(acc, oi, ni, g.pos[i], h);
        }
    }
    p
}

/// A coarser graph: each vertex paired with its most alike free
/// neighbour. Returns the graph and each fine vertex's coarse vertex.
fn coarsen(g: &Graph) -> (Graph, Vec<u32>) {
    let n = g.pos.len();
    let mut to = vec![u32::MAX; n];
    let mut pos = Vec::new();
    let mut nrm = Vec::new();
    let mut pin = Vec::new();
    let mut count = Vec::new();
    for i in 0..n {
        if to[i] != u32::MAX {
            continue;
        }
        let best = g.adj[i].iter().copied().filter(|&j| to[j as usize] == u32::MAX && j as usize != i).max_by(|&a, &b| dot(g.nrm[i], g.nrm[a as usize]).total_cmp(&dot(g.nrm[i], g.nrm[b as usize])));
        let k = pos.len() as u32;
        to[i] = k;
        let (mut p, mut nn, mut c) = (g.pos[i], g.nrm[i], 1.0);
        let mut pn = g.pin[i];
        if let Some(j) = best {
            to[j as usize] = k;
            p = add(p, g.pos[j as usize]);
            nn = add(nn, g.nrm[j as usize]);
            c += 1.0;
            pn = pn.or(g.pin[j as usize]);
        }
        pos.push(scale(p, 1.0 / c));
        nrm.push(normalise(nn));
        pin.push(pn);
        count.push(c);
    }
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    for i in 0..n {
        for &j in &g.adj[i] {
            let (a, b) = (to[i], to[j as usize]);
            if a != b && !adj[a as usize].contains(&b) {
                adj[a as usize].push(b);
            }
        }
    }
    (Graph { pos, nrm, adj, pin }, to)
}

/// Both fields, solved coarse to fine.
fn solve_fields(g: &Graph, h: f32) -> (Vec<V3>, Vec<V3>) {
    let mut levels: Vec<(Graph, Vec<u32>)> = Vec::new();
    let mut cur = Graph { pos: g.pos.clone(), nrm: g.nrm.clone(), adj: g.adj.clone(), pin: g.pin.clone() };
    while cur.pos.len() > 64 && levels.len() < 24 {
        let (next, map) = coarsen(&cur);
        if next.pos.len() as f32 > cur.pos.len() as f32 * 0.85 {
            break;
        }
        levels.push((cur, map));
        cur = next;
    }
    // Orientation, coarsest first.
    let mut o = orientation_field(&cur, None, 60);
    for (fine, map) in levels.iter().rev() {
        let init: Vec<V3> = (0..fine.pos.len()).map(|i| tangent(fine.nrm[i], o[map[i] as usize])).collect();
        o = orientation_field(fine, Some(init), 16);
    }
    // Position, coarsest first: the coarse graph needs the coarse
    // orientations again.
    let mut coarse_o: Vec<Vec<V3>> = vec![o.clone()];
    for (fine, map) in &levels {
        let prev = coarse_o.last().unwrap();
        let n_coarse = map.iter().copied().max().map_or(0, |m| m as usize + 1);
        let mut acc = vec![[0.0f32; 3]; n_coarse];
        for i in 0..fine.pos.len() {
            let k = map[i] as usize;
            acc[k] = if acc[k] == [0.0; 3] { prev[i] } else { add(acc[k], rosy4(acc[k], [0.0, 0.0, 0.0], prev[i], fine.nrm[i])) };
        }
        coarse_o.push(acc);
    }
    let coarse_graphs: Vec<&Graph> = levels.iter().map(|(g, _)| g).chain(std::iter::once(&cur)).collect();
    let mut p = position_field(&cur, &orient_on(&cur, &coarse_o[levels.len()]), h, None, 40);
    for (li, (fine, map)) in levels.iter().enumerate().rev() {
        let of = orient_on(coarse_graphs[li], &coarse_o[li]);
        let init: Vec<V3> = (0..fine.pos.len())
            .map(|i| {
                let q = p[map[i] as usize];
                let q = sub(q, scale(fine.nrm[i], dot(sub(q, fine.pos[i]), fine.nrm[i])));
                lattice_near(q, of[i], fine.nrm[i], fine.pos[i], h)
            })
            .collect();
        p = position_field(fine, &of, h, Some(init), if li == 0 { 30 } else { 12 });
    }
    (o, p)
}

/// Orientations made tangent to a graph's normals.
fn orient_on(g: &Graph, o: &[V3]) -> Vec<V3> {
    (0..g.pos.len()).map(|i| tangent(g.nrm[i], o.get(i).copied().unwrap_or([1.0, 0.0, 0.0]))).collect()
}

/// Quads read off the fields.
fn extract(g: &Graph, o: &[V3], p: &[V3], h: f32, tree: &Tree) -> Poly {
    let n = g.pos.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(par: &mut [u32], mut x: u32) -> u32 {
        while par[x as usize] != x {
            par[x as usize] = par[par[x as usize] as usize];
            x = par[x as usize];
        }
        x
    }
    let mut unit: Vec<(u32, u32)> = Vec::new();
    let mut zero: Vec<(f32, u32, u32)> = Vec::new();
    for i in 0..n {
        for &j in &g.adj[i] {
            let j = j as usize;
            if j <= i {
                continue;
            }
            let oj = rosy4(o[i], g.nrm[i], o[j], g.nrm[j]);
            let m = scale(add(g.pos[i], g.pos[j]), 0.5);
            let (pi, pj) = lattice_pair(p[i], o[i], g.nrm[i], p[j], oj, g.nrm[j], m, h);
            // Each vertex's own lattice point, with j's lattice shifted onto i's.
            let d = add(sub(p[j], p[i]), sub(pi, pj));
            let t = cross(g.nrm[i], o[i]);
            let (a, b) = ((dot(d, o[i]) / h).round() as i32, (dot(d, t) / h).round() as i32);
            if a == 0 && b == 0 {
                zero.push((length(d), i as u32, j as u32));
            } else if a.abs() + b.abs() == 1 {
                unit.push((i as u32, j as u32));
            }
        }
    }
    // Merge the closest pairs first, and never two clusters whose middles
    // are more than half a cell apart (one bad edge must not chain whole
    // regions together).
    zero.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut mean: Vec<(V3, f32)> = p.iter().map(|q| (*q, 1.0)).collect();
    for (_, i, j) in zero {
        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
        if ri == rj {
            continue;
        }
        let (a, b) = (mean[ri as usize], mean[rj as usize]);
        let (ca, cb) = (scale(a.0, 1.0 / a.1), scale(b.0, 1.0 / b.1));
        if length(sub(ca, cb)) > 0.5 * h {
            continue;
        }
        parent[ri as usize] = rj;
        mean[rj as usize] = (add(a.0, b.0), a.1 + b.1);
    }
    // Output vertices: one per cluster.
    let mut id: HashMap<u32, u32> = HashMap::new();
    let mut sum: Vec<(V3, V3, f32)> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i as u32);
        let k = *id.entry(r).or_insert_with(|| {
            sum.push(([0.0; 3], [0.0; 3], 0.0));
            (sum.len() - 1) as u32
        });
        let s = &mut sum[k as usize];
        s.0 = add(s.0, p[i]);
        s.1 = add(s.1, g.nrm[i]);
        s.2 += 1.0;
    }
    let verts: Vec<V3> = sum.iter().map(|(q, _, c)| {
        let q = scale(*q, 1.0 / c);
        tree.nearest(q, f32::INFINITY).map_or(q, |hit| hit.0)
    }).collect();
    let vn: Vec<V3> = sum.iter().map(|(_, nn, _)| normalise(*nn)).collect();
    let mut nb: Vec<HashSet<u32>> = vec![HashSet::new(); verts.len()];
    for (i, j) in unit {
        let (a, b) = (id[&find(&mut parent, i)], id[&find(&mut parent, j)]);
        if a != b {
            nb[a as usize].insert(b);
            nb[b as usize].insert(a);
        }
    }
    // Faces: walk each directed edge, turning to the next neighbour round
    // the vertex each time (a planar embedding by angle).
    let order: Vec<Vec<u32>> = (0..verts.len())
        .map(|v| {
            let n = vn[v];
            let mut list: Vec<u32> = nb[v].iter().copied().collect();
            if list.is_empty() {
                return list;
            }
            let e0 = tangent(n, sub(verts[list[0] as usize], verts[v]));
            let e1 = cross(n, e0);
            list.sort_by(|a, b| {
                let da = sub(verts[*a as usize], verts[v]);
                let db = sub(verts[*b as usize], verts[v]);
                dot(da, e1).atan2(dot(da, e0)).total_cmp(&dot(db, e1).atan2(dot(db, e0)))
            });
            list
        })
        .collect();
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let mut out = Poly { positions: verts.clone(), ..Poly::new() };
    for v in 0..verts.len() as u32 {
        for &u in &order[v as usize] {
            if used.contains(&(v, u)) {
                continue;
            }
            let mut face = vec![v];
            let (mut a, mut b) = (v, u);
            let mut ok = false;
            for _ in 0..8 {
                used.insert((a, b));
                // At b, the neighbour just before a (clockwise) continues
                // a counter-clockwise face.
                let list = &order[b as usize];
                let Some(k) = list.iter().position(|&x| x == a) else { break };
                let next = list[(k + list.len() - 1) % list.len()];
                if next == v {
                    face.push(b);
                    used.insert((b, next));
                    ok = true;
                    break;
                }
                face.push(b);
                a = b;
                b = next;
            }
            if ok && (3..=6).contains(&face.len()) {
                let mut f = face;
                // Wind outward.
                let nf = {
                    let mut s = [0.0; 3];
                    for i in 0..f.len() {
                        let (x, y) = (verts[f[i] as usize], verts[f[(i + 1) % f.len()] as usize]);
                        s = add(s, cross(x, y));
                    }
                    s
                };
                if dot(nf, vn[f[0] as usize]) < 0.0 {
                    f.reverse();
                }
                out.add_face(&f, 0);
            }
        }
    }
    // Faces that came out both ways (an inside-out duplicate) go.
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    let mut keep = vec![true; out.faces()];
    for f in 0..out.faces() {
        let mut k = out.face(f).to_vec();
        k.sort_unstable();
        if !seen.insert(k) {
            keep[f] = false;
        }
    }
    out.retain_faces(|f| keep[f]);
    out
}

/// Removes faces that make an edge shared by more than two faces.
fn make_manifold(p: &mut Poly) {
    for _ in 0..4 {
        let t = Topology::of(p);
        let bad: HashSet<u32> = t.edge_faces.values().filter(|f| f.len() > 2).flat_map(|f| f[2..].iter().copied()).collect();
        if bad.is_empty() {
            break;
        }
        p.retain_faces(|f| !bad.contains(&(f as u32)));
    }
    p.compact();
}

/// A clean quad mesh of about `faces` faces over the surface of `src`.
pub fn quad_remesh(src: &Poly, faces: usize) -> Result<Poly, String> {
    quad_remesh_report(src, faces).map(|(p, _)| p)
}

/// As [`quad_remesh`], saying whether the field-aligned quads were used
/// (false: the even voxel quads it falls back to).
pub fn quad_remesh_report(src: &Poly, faces: usize) -> Result<(Poly, bool), String> {
    let faces = faces.max(24);
    let area: f32 = (0..src.faces()).map(|f| src.face_area(f)).sum();
    if area <= 0.0 {
        return Err("the surface has no area".into());
    }
    let h = (area / faces as f32).sqrt();
    let mesh = src.to_mesh(180.0, crate::NormalMode::Angle);
    let tree = Tree::of_mesh(&mesh);
    // Even samples (about six per output point) on a closed surface.
    let samples = crate::remesh::voxel(src, h / 2.5)?;
    let g = graph_of(&samples, 50.0);
    let (o, p) = solve_fields(&g, h);
    let mut out = extract(&g, &o, &p, h, &tree);
    make_manifold(&mut out);
    crate::edit::fill_holes(&mut out, 8);
    out.orient_outward();
    crate::edit::tris_to_quads(&mut out, 40.0);
    let quads = (0..out.faces()).filter(|&f| out.face(f).len() == 4).count();
    let open = Topology::of(&out).boundary_edges().len();
    let good = out.faces() > faces / 4 && quads * 10 >= out.faces() * 7 && open * 50 <= out.faces();
    if !good {
        // Fallback: even voxel quads at the wanted size.
        let mut v = crate::remesh::voxel(src, h)?;
        crate::remesh::relax_onto(&mut v, &tree, 4);
        v.ensure_materials();
        return Ok((v, false));
    }
    crate::remesh::relax_onto(&mut out, &tree, 3);
    out.ensure_materials();
    Ok((out, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    fn check(p: &Poly, want: usize, on: &dyn Fn(V3) -> f32) {
        let quads = (0..p.faces()).filter(|&f| p.face(f).len() == 4).count();
        assert!(quads * 10 >= p.faces() * 7, "mostly quads: {}", p.stats());
        assert!(p.faces() > want / 3 && p.faces() < want * 3, "about {want} faces: {}", p.stats());
        let open = Topology::of(p).boundary_edges().len();
        assert!(open * 50 <= p.faces(), "{open} open edges: {}", p.stats());
        let worst = p.positions.iter().map(|v| on(*v).abs()).fold(0.0f32, f32::max);
        assert!(worst < 2.0, "off the surface by {worst}");
    }

    #[test]
    fn a_dense_ball_becomes_even_quads() {
        let ball = crate::subd::subdivide(&prim::uv_sphere(50.0, 24, 12), 1);
        let (r, field) = quad_remesh_report(&ball, 600).unwrap();
        assert!(field, "field-aligned, not the fallback");
        check(&r, 600, &|v| length(v) - 50.0);
    }

    #[test]
    fn a_box_keeps_its_edges() {
        let mut b = prim::cube([100.0; 3], 1);
        crate::edit::subdivide_flat(&mut b, 4);
        let (r, field) = quad_remesh_report(&b, 400).unwrap();
        assert!(field, "field-aligned, not the fallback");
        check(&r, 400, &|v| v[0].abs().max(v[1].abs()).max(v[2].abs()) - 50.0);
    }
}
