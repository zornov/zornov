// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Surfaces sampled over a grid: parametric surfaces (u, v) → (x, y, z),
//! tubes along curves, height fields z = f(x, y).
//!
//! A [`Sheet`] is the samples and which grid cells to keep (a cut-out
//! pattern drops the others). [`build`] turns it into triangles: where the
//! grid meets itself (a torus's seam, a sphere's poles, a Möbius strip's
//! twist) the edges join, so a closed surface comes out closed; with a
//! thickness the sheet becomes a shell, two faces and the walls round
//! every edge and hole, a closed solid. Texture coordinates are u and v
//! over 0..1; normals are smooth over the faces and sharp at the walls.

use super::Program;
use crate::{Material, Mesh, V3, add, cross, dot, length, normalise, scale, sub};
use std::collections::HashMap;

/// The names a surface's formulas read: the parameters, time, and `i`
/// (0, 1, 2) for one formula giving all three as `[x, y, z]`.
pub const NAMES: &[&str] = &["u", "v", "t", "i"];

/// The names a cut-out formula reads: `u` and `v` over 0..1 (the texture
/// coordinates), the point, the raw parameters `pu` and `pv`, and `t`.
pub const CUT_NAMES: &[&str] = &["u", "v", "x", "y", "z", "pu", "pv", "t"];

/// The names a height field's formula reads.
pub const HEIGHT_NAMES: &[&str] = &["x", "y", "t", "r"];

/// The most samples a sheet may have (u × v).
pub const MAX_SAMPLES: usize = 512 * 512;

/// What the three formulas of a surface give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coords {
    /// x, y, z.
    Cartesian,
    /// r (from the z axis), theta (round it), z.
    Cylindrical,
    /// r, theta (round the z axis), phi (up from the x–y plane).
    Spherical,
}

impl Coords {
    pub fn parse(s: &str) -> Option<Coords> {
        Some(match s {
            "cartesian" | "xyz" => Coords::Cartesian,
            "cylindrical" | "cylinder" | "polar" => Coords::Cylindrical,
            "spherical" | "sphere" => Coords::Spherical,
            _ => return None,
        })
    }

    pub fn to_xyz(self, p: [f64; 3]) -> [f64; 3] {
        match self {
            Coords::Cartesian => p,
            Coords::Cylindrical => [p[0] * p[1].cos(), p[0] * p[1].sin(), p[2]],
            Coords::Spherical => [p[0] * p[2].cos() * p[1].cos(), p[0] * p[2].cos() * p[1].sin(), p[0] * p[2].sin()],
        }
    }
}

/// A surface's formulas: one giving `[x, y, z]` (worked out for i = 0, 1,
/// 2) or three, in some coordinates.
pub struct Formulas {
    pub parts: Vec<Program>,
    pub coords: Coords,
}

impl Formulas {
    pub fn point(&self, u: f64, v: f64, t: f64) -> [f64; 3] {
        let p = if self.parts.len() >= 3 {
            [self.parts[0].eval(&[u, v, t, 0.0]), self.parts[1].eval(&[u, v, t, 1.0]), self.parts[2].eval(&[u, v, t, 2.0])]
        } else {
            let f = &self.parts[0];
            [f.eval(&[u, v, t, 0.0]), f.eval(&[u, v, t, 1.0]), f.eval(&[u, v, t, 2.0])]
        };
        self.coords.to_xyz(p)
    }

    /// Whether the surface changes with `t`.
    pub fn moves(&self) -> bool {
        self.parts.iter().any(|p| p.uses(2))
    }

    /// Evaluations per sample.
    pub fn cost(&self) -> u64 {
        3
    }
}

/// A grid of samples: `nu` along u by `nv` along v, point `j * nu + i`;
/// cell `j * (nu - 1) + i` kept or not.
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub nu: usize,
    pub nv: usize,
    pub points: Vec<[f64; 3]>,
    pub keep: Vec<bool>,
}

impl Sheet {
    /// Samples `f` at `nu` × `nv` points over the ranges (ends included).
    pub fn sample(f: &(dyn Fn(f64, f64) -> [f64; 3] + Sync), u: [f64; 2], v: [f64; 2], nu: usize, nv: usize) -> Sheet {
        let (nu, nv) = (nu.max(2), nv.max(2));
        let mut points = vec![[0.0; 3]; nu * nv];
        let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16);
        let rows = nv.div_ceil(threads);
        std::thread::scope(|s| {
            for (c, chunk) in points.chunks_mut(rows * nu).enumerate() {
                s.spawn(move || {
                    for (k, p) in chunk.iter_mut().enumerate() {
                        let (i, j) = (k % nu, c * rows + k / nu);
                        let pu = u[0] + (u[1] - u[0]) * i as f64 / (nu - 1) as f64;
                        let pv = v[0] + (v[1] - v[0]) * j as f64 / (nv - 1) as f64;
                        let q = f(pu, pv);
                        *p = if q.iter().all(|x| x.is_finite()) { q } else { [0.0; 3] };
                    }
                });
            }
        });
        Sheet { nu, nv, points, keep: vec![true; (nu - 1) * (nv - 1)] }
    }

    /// Drops the cells where `cut` (over [`CUT_NAMES`], at each cell's
    /// middle) is not 0; how many were dropped.
    pub fn cut(&mut self, cut: &Program, u: [f64; 2], v: [f64; 2], t: f64) -> usize {
        let mut n = 0;
        for j in 0..self.nv - 1 {
            for i in 0..self.nu - 1 {
                let (fu, fv) = ((i as f64 + 0.5) / (self.nu - 1) as f64, (j as f64 + 0.5) / (self.nv - 1) as f64);
                let c = [self.points[j * self.nu + i], self.points[j * self.nu + i + 1], self.points[(j + 1) * self.nu + i], self.points[(j + 1) * self.nu + i + 1]];
                let m = [0, 1, 2].map(|a| (c[0][a] + c[1][a] + c[2][a] + c[3][a]) * 0.25);
                let vars = [fu, fv, m[0], m[1], m[2], u[0] + (u[1] - u[0]) * fu, v[0] + (v[1] - v[0]) * fv, t];
                if cut.eval(&vars) != 0.0 {
                    self.keep[j * (self.nu - 1) + i] = false;
                    n += 1;
                }
            }
        }
        n
    }

    /// The smallest and largest point.
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &self.points {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        (lo, hi)
    }
}

/// A built surface and, for every vertex, which sample it came from (so
/// the same triangles can follow the formula as `t` runs: see
/// [`Built::follow`]).
pub struct Built {
    pub mesh: Mesh,
    /// Per vertex: `side * samples + sample`, side 0 the front (or only)
    /// face, 1 the back.
    pub source: Vec<u32>,
    /// Per vertex: whether it is on a wall (flat shaded).
    wall: Vec<bool>,
    pub closed: bool,
    thickness: f32,
    nu: usize,
    nv: usize,
}

/// Welds points closer than `tol`: an id per point, the same for points
/// at one place.
fn weld(points: &[V3], tol: f32) -> Vec<u32> {
    let key = |p: V3| [(p[0] / tol).floor() as i64, (p[1] / tol).floor() as i64, (p[2] / tol).floor() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut ids = vec![0u32; points.len()];
    for (k, p) in points.iter().enumerate() {
        let c = key(*p);
        let mut found = None;
        'look: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(list) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &o in list {
                            if length(sub(points[o as usize], *p)) <= tol {
                                found = Some(ids[o as usize]);
                                break 'look;
                            }
                        }
                    }
                }
            }
        }
        ids[k] = match found {
            Some(id) => id,
            None => {
                grid.entry(c).or_default().push(k as u32);
                k as u32
            }
        };
    }
    ids
}

/// Normals of a grid's points from the cells round them, averaged over the
/// points that weld together, each turned to agree with the point's own
/// (so a Möbius strip's seam does not cancel out).
fn grid_normals(nu: usize, nv: usize, p: &[V3], ids: &[u32]) -> Vec<V3> {
    let mut own = vec![[0.0f32; 3]; p.len()];
    for j in 0..nv - 1 {
        for i in 0..nu - 1 {
            let (a, b, c, d) = (j * nu + i, j * nu + i + 1, (j + 1) * nu + i + 1, (j + 1) * nu + i);
            let n = cross(sub(p[c], p[a]), sub(p[d], p[b]));
            for k in [a, b, c, d] {
                own[k] = add(own[k], n);
            }
        }
    }
    let mut members: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, id) in ids.iter().enumerate() {
        members.entry(*id).or_default().push(k);
    }
    (0..p.len())
        .map(|k| {
            let mine = own[k];
            let mut sum = [0.0f32; 3];
            for &o in &members[&ids[k]] {
                let n = own[o];
                sum = add(sum, if dot(n, mine) < 0.0 { scale(n, -1.0) } else { n });
            }
            let n = normalise(sum);
            if length(n) > 0.5 { n } else { [0.0, -1.0, 0.0] }
        })
        .collect()
}

/// Turns a sheet into triangles in Studio's world: `place` takes a sample
/// (formula space) to its spot (px). `thickness` > 0 makes a shell that
/// thick (px) about the surface; `back` instead gives every sample its own
/// back point (a height field's floor) and the shell runs between.
pub fn build(sheet: &Sheet, place: &dyn Fn([f64; 3]) -> V3, thickness: f32, back: Option<&[V3]>) -> Built {
    let (nu, nv) = (sheet.nu, sheet.nv);
    let front: Vec<V3> = sheet.points.iter().map(|p| place(*p)).collect();
    let (lo, hi) = bounds(&front);
    let tol = (length(sub(hi, lo)) * 1e-5).max(1e-5);
    let mid_ids = weld(&front, tol);
    let solid = thickness > 0.0 || back.is_some();
    let n = front.len();
    let mut pos: Vec<V3> = front.clone();
    if solid {
        match back {
            Some(b) => pos.extend_from_slice(b),
            None => {
                let nrm = grid_normals(nu, nv, &front, &mid_ids);
                pos = front.iter().zip(&nrm).map(|(p, d)| add(*p, scale(*d, thickness * 0.5))).collect();
                pos.extend(front.iter().zip(&nrm).map(|(p, d)| sub(*p, scale(*d, thickness * 0.5))));
            }
        }
    }
    let ids = weld(&pos, tol);
    let side_ids = |s: usize| &ids[s * n..(s + 1) * n];
    let mut normals = grid_normals(nu, nv, &pos[..n], side_ids(0));
    if solid {
        normals.extend(grid_normals(nu, nv, &pos[n..], side_ids(1)).into_iter().map(|d| scale(d, -1.0)));
    }
    let uv = |k: usize| [(k % nu) as f32 / (nu - 1) as f32, (k / nu) as f32 / (nv - 1) as f32];
    let kept = |i: isize, j: isize| i >= 0 && j >= 0 && (i as usize) < nu - 1 && (j as usize) < nv - 1 && sheet.keep[j as usize * (nu - 1) + i as usize];
    // Triangles as (vertex source, wall) corners; walls have vertices of
    // their own so they are shaded flat.
    let mut tris: Vec<[(u32, bool); 3]> = Vec::new();
    let quad_short = |q: [u32; 4], side: u32, tris: &mut Vec<[(u32, bool); 3]>| {
        let s = |k: u32| (side * n as u32 + k, false);
        let at = |k: u32| pos[(side * n as u32 + k) as usize];
        if length(sub(at(q[0]), at(q[2]))) <= length(sub(at(q[1]), at(q[3]))) {
            tris.push([s(q[0]), s(q[1]), s(q[2])]);
            tris.push([s(q[0]), s(q[2]), s(q[3])]);
        } else {
            tris.push([s(q[0]), s(q[1]), s(q[3])]);
            tris.push([s(q[1]), s(q[2]), s(q[3])]);
        }
    };
    for j in 0..nv - 1 {
        for i in 0..nu - 1 {
            if !sheet.keep[j * (nu - 1) + i] {
                continue;
            }
            let q = [(j * nu + i) as u32, (j * nu + i + 1) as u32, ((j + 1) * nu + i + 1) as u32, ((j + 1) * nu + i) as u32];
            quad_short(q, 0, &mut tris);
            if solid {
                quad_short([q[3], q[2], q[1], q[0]], 1, &mut tris);
                // Walls on the edges with no kept cell beyond.
                let (ii, jj) = (i as isize, j as isize);
                for (k, (di, dj)) in [(0, -1), (1, 0), (0, 1), (-1, 0)].into_iter().enumerate() {
                    if kept(ii + di, jj + dj) {
                        continue;
                    }
                    let (a, b) = (q[k], q[(k + 1) % 4]);
                    let nn = n as u32;
                    // Front b, front a, back a, back b: split from the
                    // corner that welds lowest, so two walls on one spot
                    // (a seam) split alike and cancel.
                    let w = [b, a, nn + a, nn + b];
                    let low = (0..4).min_by_key(|&c| ids[w[c] as usize]).unwrap_or(0);
                    let r = [w[low], w[(low + 1) % 4], w[(low + 2) % 4], w[(low + 3) % 4]];
                    tris.push([(r[0], true), (r[1], true), (r[2], true)]);
                    tris.push([(r[0], true), (r[2], true), (r[3], true)]);
                }
            }
        }
    }
    // Drop triangles that weld flat, and pairs that lie on each other.
    let key = |t: &[(u32, bool); 3]| {
        let mut k = t.map(|c| ids[c.0 as usize]);
        k.sort_unstable();
        k
    };
    let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
    for t in &tris {
        *seen.entry(key(t)).or_insert(0) += 1;
    }
    tris.retain(|t| {
        let k = key(t);
        k[0] != k[1] && k[1] != k[2] && seen[&k] == 1
    });
    // Vertices: one per (source, wall face) as needed, at the welded spot.
    let mut mesh = Mesh::default();
    let mut source = Vec::new();
    let mut wall = Vec::new();
    let mut made: HashMap<u32, u32> = HashMap::new();
    for t in &tris {
        let face = normalise(cross(sub(pos[ids[t[1].0 as usize] as usize], pos[ids[t[0].0 as usize] as usize]), sub(pos[ids[t[2].0 as usize] as usize], pos[ids[t[0].0 as usize] as usize])));
        for &(c, is_wall) in t {
            let rep = ids[c as usize];
            let v = if is_wall {
                mesh.positions.push(pos[rep as usize]);
                mesh.normals.push(face);
                mesh.uvs.push(uv(c as usize % n));
                source.push(rep);
                wall.push(true);
                (mesh.positions.len() - 1) as u32
            } else {
                *made.entry(c).or_insert_with(|| {
                    mesh.positions.push(pos[rep as usize]);
                    mesh.normals.push(normals[c as usize]);
                    mesh.uvs.push(uv(c as usize % n));
                    source.push(rep);
                    wall.push(false);
                    (mesh.positions.len() - 1) as u32
                })
            };
            mesh.indices.push(v);
        }
    }
    mesh.materials.push(Material::default());
    mesh.single_group();
    let closed = super::open_edges(&mesh).0 == 0 && mesh.triangles() > 0;
    if closed && super::volume(&mesh) < 0.0 {
        crate::ops::flip(&mut mesh);
    }
    // Smooth faces face the way their triangles wind.
    orient_normals(&mut mesh);
    Built { mesh, source, wall, closed, thickness, nu, nv }
}

/// Turns each smooth vertex normal to agree with the faces using it.
fn orient_normals(m: &mut Mesh) {
    let mut acc = vec![0.0f32; m.positions.len()];
    for t in 0..m.triangles() {
        let f = cross(sub(m.positions[m.tri(t)[1]], m.positions[m.tri(t)[0]]), sub(m.positions[m.tri(t)[2]], m.positions[m.tri(t)[0]]));
        for v in m.tri(t) {
            acc[v] += dot(f, m.normals[v]);
        }
    }
    for (v, a) in acc.iter().enumerate() {
        if *a < 0.0 {
            m.normals[v] = scale(m.normals[v], -1.0);
        }
    }
}

fn bounds(p: &[V3]) -> (V3, V3) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for q in p {
        for a in 0..3 {
            lo[a] = lo[a].min(q[a]);
            hi[a] = hi[a].max(q[a]);
        }
    }
    if p.is_empty() { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
}

impl Built {
    /// The vertices for the same sheet sampled again (another `t`): the
    /// triangles stay, the points move. `back` as for [`build`].
    pub fn follow(&self, sheet: &Sheet, place: &dyn Fn([f64; 3]) -> V3, back: Option<&[V3]>) -> Vec<V3> {
        let front: Vec<V3> = sheet.points.iter().map(|p| place(*p)).collect();
        let n = front.len();
        let all: Vec<V3> = if let Some(b) = back {
            front.iter().chain(b).copied().collect()
        } else if self.thickness > 0.0 {
            let ids: Vec<u32> = (0..n as u32).collect();
            let nrm = grid_normals(self.nu, self.nv, &front, &ids);
            let h = self.thickness * 0.5;
            front.iter().zip(&nrm).map(|(p, d)| add(*p, scale(*d, h))).chain(front.iter().zip(&nrm).map(|(p, d)| sub(*p, scale(*d, h)))).collect()
        } else {
            front
        };
        self.source.iter().map(|s| all.get(*s as usize).copied().unwrap_or([0.0; 3])).collect()
    }

    /// How many vertices are on walls.
    pub fn wall_vertices(&self) -> usize {
        self.wall.iter().filter(|w| **w).count()
    }
}

/// Points along a tube round a curve: a ring of `sides` per curve point,
/// `radius` there, turned by frames carried along the curve without
/// twisting (a closed curve's last frame meets its first). An open curve
/// gets a ring of radius 0 at each end, so its ends close.
pub fn tube(curve: &[[f64; 3]], radius: &[f64], sides: usize) -> Sheet {
    let n = curve.len();
    let sides = sides.clamp(3, 256);
    if n < 2 {
        return Sheet::default();
    }
    let d = |a: [f64; 3], b: [f64; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let span = curve.windows(2).map(|w| d(w[0], w[1])).sum::<f64>().max(1e-12);
    let closed = d(curve[0], curve[n - 1]) < span * 1e-6;
    let norm = |a: [f64; 3]| {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-300);
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let dot3 = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross3 = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let tangent = |k: usize| {
        let (a, b) = if closed {
            (curve[(k + n - 2) % (n - 1)], curve[(k + 1) % (n - 1)])
        } else {
            (curve[k.saturating_sub(1)], curve[(k + 1).min(n - 1)])
        };
        norm([b[0] - a[0], b[1] - a[1], b[2] - a[2]])
    };
    let t0 = tangent(0);
    let pick = if t0[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let mut nrm = norm(cross3(t0, pick));
    let mut frames = Vec::with_capacity(n);
    for k in 0..n {
        let t = tangent(k);
        let p = dot3(nrm, t);
        nrm = norm([nrm[0] - t[0] * p, nrm[1] - t[1] * p, nrm[2] - t[2] * p]);
        frames.push((t, nrm, cross3(t, nrm)));
    }
    // A closed curve: spread the turn between the last frame and the first.
    let twist = if closed {
        let (_, n0, b0) = frames[0];
        let (_, nl, _) = frames[n - 1];
        dot3(nl, b0).atan2(dot3(nl, n0))
    } else {
        0.0
    };
    let mut rows: Vec<([f64; 3], [f64; 3], [f64; 3], f64)> = Vec::new();
    if !closed {
        rows.push((curve[0], frames[0].1, frames[0].2, 0.0));
    }
    for k in 0..n {
        let (_, nn, bb) = frames[k];
        let a = -twist * k as f64 / (n - 1) as f64;
        let (c, s) = (a.cos(), a.sin());
        let n2 = [nn[0] * c + bb[0] * s, nn[1] * c + bb[1] * s, nn[2] * c + bb[2] * s];
        let b2 = [bb[0] * c - nn[0] * s, bb[1] * c - nn[1] * s, bb[2] * c - nn[2] * s];
        rows.push((curve[k], n2, b2, radius[k.min(radius.len() - 1)].max(0.0)));
    }
    if !closed {
        rows.push((curve[n - 1], frames[n - 1].1, frames[n - 1].2, 0.0));
    }
    if closed {
        // The last ring is the first: the same points, exactly.
        let first = rows[0];
        let last = rows.len() - 1;
        rows[last] = first;
    }
    let nu = sides + 1;
    let mut points = Vec::with_capacity(nu * rows.len());
    for (c, nn, bb, r) in &rows {
        for i in 0..nu {
            let a = std::f64::consts::TAU * (i % sides) as f64 / sides as f64;
            let (ca, sa) = (a.cos() * r, a.sin() * r);
            points.push([c[0] + nn[0] * ca + bb[0] * sa, c[1] + nn[1] * ca + bb[1] * sa, c[2] + nn[2] * ca + bb[2] * sa]);
        }
    }
    let nv = rows.len();
    Sheet { nu, nv, points, keep: vec![true; (nu - 1) * (nv - 1)] }
}

/// A height field's grid: z = `h(x, y)` over `lo..hi` (formula space),
/// and its floor `depth` below the lowest point.
pub fn height(h: &(dyn Fn(f64, f64) -> f64 + Sync), lo: [f64; 2], hi: [f64; 2], nx: usize, ny: usize, depth: f64) -> (Sheet, Vec<[f64; 3]>) {
    let s = Sheet::sample(&|x, y| [x, y, h(x, y)], [lo[0], hi[0]], [lo[1], hi[1]], nx, ny);
    let floor = s.points.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min) - depth.max(1e-9);
    let back = s.points.iter().map(|p| [p[0], p[1], floor]).collect();
    (s, back)
}

/// A named surface: settings bound in front of its formulas, which give
/// `[x, y, z]` (or r, theta, z / r, theta, phi by `coords`) over the
/// ranges of u and v.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [(&'static str, f64)],
    pub formula: &'static str,
    pub coords: Coords,
    /// The ranges of u and v as formulas of the settings (`"tau"`).
    pub u: [&'static str; 2],
    pub v: [&'static str; 2],
    /// A shell this thick (a fraction of the size) by default: for sheets
    /// that are open.
    pub thickness: f64,
}

const TAU: &str = "tau";

/// The parametric presets.
pub const PRESETS: &[Preset] = &[
    Preset { name: "torus", about: "a ring: `radius` round, `tube` thick (in the same units)", params: &[("radius", 1.0), ("tube", 0.4)], formula: "[(radius + tube * cos(v)) * cos(u), (radius + tube * cos(v)) * sin(u), tube * sin(v)]", coords: Coords::Cartesian, u: ["0", TAU], v: ["0", TAU], thickness: 0.0 },
    Preset { name: "sphere", about: "a ball by latitude and longitude", params: &[], formula: "[cos(u) * cos(v), sin(u) * cos(v), sin(v)]", coords: Coords::Cartesian, u: ["0", TAU], v: ["-pi / 2", "pi / 2"], thickness: 0.0 },
    Preset { name: "swirl_torus", about: "a cylindrical-coordinate knot of a ring: r = 3 + sin u + cos v, theta = 2u, z = sin v + 2 cos u", params: &[("a", 3.0), ("turns", 2.0)], formula: "[a + sin(u) + cos(v), turns * u, sin(v) + 2 * cos(u)]", coords: Coords::Cylindrical, u: ["0", TAU], v: ["0", TAU], thickness: 0.0 },
    Preset { name: "mobius", about: "a Möbius strip: one side, one edge (`width` across, `twists` half turns, odd ones one-sided)", params: &[("width", 0.5), ("twists", 1.0)], formula: "[(1 + v * width * cos(twists * u / 2)) * cos(u), (1 + v * width * cos(twists * u / 2)) * sin(u), v * width * sin(twists * u / 2)]", coords: Coords::Cartesian, u: ["0", TAU], v: ["-1", "1"], thickness: 0.03 },
    Preset { name: "klein", about: "a Klein bottle (the figure-8 immersion): a closed surface with no inside", params: &[("a", 2.0)], formula: "w = a + cos(u / 2) * sin(v) - sin(u / 2) * sin(2 * v)\n[w * cos(u), w * sin(u), sin(u / 2) * sin(v) + cos(u / 2) * sin(2 * v)]", coords: Coords::Cartesian, u: ["0", TAU], v: ["0", TAU], thickness: 0.0 },
    Preset { name: "seashell", about: "a spiral shell grown by a logarithmic spiral (`turns` coils)", params: &[("turns", 3.0)], formula: "g = exp(u / (6 * pi)); c = cos(v / 2)^2\n[2 * (1 - g) * cos(u) * c, 2 * (g - 1) * sin(u) * c, 1 - exp(u / (3 * pi)) - sin(v) + g * sin(v)]", coords: Coords::Cartesian, u: ["0", "turns * 2 * pi"], v: ["0", TAU], thickness: 0.0 },
    Preset { name: "nautilus", about: "a chambered spiral: a tube widening by `growth` each turn, coiled `turns` times", params: &[("turns", 3.0), ("growth", 0.18), ("open", 0.85)], formula: "k = exp(growth * u)\n[k * (1 + open * cos(v)) * cos(u), k * (1 + open * cos(v)) * sin(u), k * open * sin(v)]", coords: Coords::Cartesian, u: ["0", "turns * 2 * pi"], v: ["0", TAU], thickness: 0.0 },
    Preset { name: "horn", about: "a curling horn, thin at its tip", params: &[("curl", 2.0)], formula: "[(2 + u * cos(v)) * sin(curl * pi * u), (2 + u * cos(v)) * cos(curl * pi * u) + 2 * u, u * sin(v)]", coords: Coords::Cartesian, u: ["0", "1"], v: ["0", TAU], thickness: 0.0 },
    Preset { name: "vase", about: "a vase turned on a lathe: its radius waves `waves` times up its height, closed at the foot, open at the mouth", params: &[("waves", 1.5), ("bulge", 0.3), ("height", 2.0)], formula: "rr = (0.55 + bulge * sin(waves * pi * v / height + 0.6)) * min(1, v * 6)\n[rr * cos(u), rr * sin(u), v]", coords: Coords::Cartesian, u: ["0", TAU], v: ["0", "height"], thickness: 0.025 },
    Preset { name: "dini", about: "Dini's surface: a twisted pseudosphere, a spiralling trumpet", params: &[("a", 1.0), ("b", 0.2), ("turns", 2.0)], formula: "[a * cos(u) * sin(v), a * sin(u) * sin(v), a * (cos(v) + ln(tan(v / 2))) + b * u]", coords: Coords::Cartesian, u: ["0", "turns * 2 * pi"], v: ["0.05", "2"], thickness: 0.02 },
    Preset { name: "enneper", about: "Enneper's minimal surface: a saddle that folds over itself", params: &[("size", 1.6)], formula: "[u - u^3 / 3 + u * v^2, v - v^3 / 3 + v * u^2, u^2 - v^2]", coords: Coords::Cartesian, u: ["-size", "size"], v: ["-size", "size"], thickness: 0.02 },
    Preset { name: "helicoid", about: "a helicoid: a spiral ramp, the surface of a screw", params: &[("turns", 2.0), ("pitch", 0.3)], formula: "[v * cos(u), v * sin(u), pitch * u]", coords: Coords::Cartesian, u: ["0", "turns * 2 * pi"], v: ["0.2", "1"], thickness: 0.03 },
    Preset { name: "catenoid", about: "a catenoid: the minimal surface between two rings (a soap film)", params: &[("c", 1.0), ("height", 1.2)], formula: "[c * cosh(v / c) * cos(u), c * cosh(v / c) * sin(u), v]", coords: Coords::Cartesian, u: ["0", TAU], v: ["-height / 2", "height / 2"], thickness: 0.02 },
    Preset { name: "supershape", about: "Gielis's supershape: the superformula in two directions (stars, flowers, shells, crystals by m and n1..n3)", params: &[("m", 6.0), ("n1", 1.0), ("n2", 1.0), ("n3", 1.0), ("m2", 3.0), ("k1", 1.0), ("k2", 1.0), ("k3", 1.0)], formula: "r1 = superformula(u, m, n1, n2, n3); r2 = superformula(v, m2, k1, k2, k3)\n[r1 * cos(u) * r2 * cos(v), r1 * sin(u) * r2 * cos(v), r2 * sin(v)]", coords: Coords::Cartesian, u: ["-pi", "pi"], v: ["-pi / 2", "pi / 2"], thickness: 0.0 },
    Preset { name: "boy", about: "Boy's surface (Bryant–Kusner): the projective plane in space, three lobes round a triple point", params: &[], formula: "rr = v; th = u\nre = rr * cos(th); im = rr * sin(th)\n// w = re + i im; the powers of w\nw2r = re^2 - im^2; w2i = 2 * re * im\nw3r = w2r * re - w2i * im; w3i = w2r * im + w2i * re\nw4r = w2r^2 - w2i^2; w4i = 2 * w2r * w2i\nw6r = w3r^2 - w3i^2; w6i = 2 * w3r * w3i\ndr = w6r + sqrt(5) * w3r - 1; di = w6i + sqrt(5) * w3i\nden = dr^2 + di^2\n// g1 = -3/2 Im(w(1 - w^4)/d), g2 = -3/2 Re(w(1 + w^4)/d), g3 = Im((1 + w^6)/d) - 1/2\nn1r = re - (w4r * re - w4i * im); n1i = im - (w4r * im + w4i * re)\nn2r = re + (w4r * re - w4i * im); n2i = im + (w4r * im + w4i * re)\nn3r = 1 + w6r; n3i = w6i\ng1 = -1.5 * (n1i * dr - n1r * di) / den\ng2 = -1.5 * (n2r * dr + n2i * di) / den\ng3 = (n3i * dr - n3r * di) / den - 0.5\ns = g1^2 + g2^2 + g3^2\n[g1 / s, g2 / s, g3 / s]", coords: Coords::Cartesian, u: ["0", TAU], v: ["0", "1"], thickness: 0.0 },
];

/// The preset called `name`.
pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// A named curve for a tube (`[x, y, z]` of u over its range).
#[derive(Clone, Copy, Debug)]
pub struct CurvePreset {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [(&'static str, f64)],
    pub formula: &'static str,
    pub u: [&'static str; 2],
    /// The tube's radius in curve units.
    pub radius: f64,
}

/// The tube presets.
pub const TUBES: &[CurvePreset] = &[
    CurvePreset { name: "torus_knot", about: "a (p, q) torus knot: p times round the hole, q times through it (2, 3 is the trefoil)", params: &[("p", 2.0), ("q", 3.0), ("radius", 1.0), ("tube", 0.4)], formula: "[(radius + tube * cos(q * u)) * cos(p * u), (radius + tube * cos(q * u)) * sin(p * u), tube * sin(q * u)]", u: ["0", TAU], radius: 0.13 },
    CurvePreset { name: "trefoil", about: "the trefoil knot", params: &[], formula: "[sin(u) + 2 * sin(2 * u), cos(u) - 2 * cos(2 * u), -sin(3 * u)]", u: ["0", TAU], radius: 0.35 },
    CurvePreset { name: "figure_eight_knot", about: "the figure-eight knot", params: &[], formula: "[(2 + cos(2 * u)) * cos(3 * u), (2 + cos(2 * u)) * sin(3 * u), sin(4 * u)]", u: ["0", TAU], radius: 0.25 },
    CurvePreset { name: "lissajous_knot", about: "a Lissajous knot (frequencies a, b, c)", params: &[("a", 3.0), ("b", 2.0), ("c", 7.0), ("phase", 0.7)], formula: "[cos(a * u + phase), cos(b * u + 1.5), cos(c * u)]", u: ["0", TAU], radius: 0.05 },
    CurvePreset { name: "helix", about: "a coil spring, `turns` turns", params: &[("turns", 6.0), ("pitch", 0.35)], formula: "[cos(u), sin(u), pitch * u / (2 * pi)]", u: ["0", "turns * 2 * pi"], radius: 0.12 },
    CurvePreset { name: "spiral", about: "a flat spiral coil", params: &[("turns", 4.0)], formula: "[u / (2 * pi) * cos(u), u / (2 * pi) * sin(u), 0]", u: ["0.3", "turns * 2 * pi"], radius: 0.12 },
];

/// The tube preset called `name`.
pub fn tube_preset(name: &str) -> Option<&'static CurvePreset> {
    TUBES.iter().find(|p| p.name == name)
}

/// A named height field: z of x and y over -1..1.
#[derive(Clone, Copy, Debug)]
pub struct HeightPreset {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [(&'static str, f64)],
    pub formula: &'static str,
}

/// The height presets.
pub const HEIGHTS: &[HeightPreset] = &[
    HeightPreset { name: "waves", about: "crossing sine waves, moving with `speed`", params: &[("frequency", 6.0), ("amount", 0.08), ("speed", 0.0)], formula: "amount * sin(frequency * x + t * speed) * cos(frequency * y * 0.8 - t * speed * 0.7)" },
    HeightPreset { name: "ripples", about: "rings spreading from the middle (a drop in a pond), moving with `speed`", params: &[("frequency", 14.0), ("amount", 0.1), ("speed", 4.0)], formula: "amount * sin(frequency * r - t * speed) / (1 + 5 * r)" },
    HeightPreset { name: "dunes", about: "wind-blown dunes with ripples", params: &[("amount", 0.18), ("seed", 2.0)], formula: "b = noise2(x * 1.5 + seed, y * 0.6); d = abs(sin(x * 3 + b * 2)) * amount; d + 0.01 * sin(x * 40 + y * 6)" },
    HeightPreset { name: "terrain", about: "hills and valleys of fractal noise (`roughness` its octaves)", params: &[("amount", 0.3), ("frequency", 1.5), ("roughness", 6.0), ("seed", 0.0)], formula: "amount * fbm(x * frequency + seed, y * frequency, 0.3, roughness)" },
    HeightPreset { name: "saddle", about: "a saddle, z = x² − y²", params: &[("amount", 0.5)], formula: "amount * (x^2 - y^2)" },
    HeightPreset { name: "egg_crate", about: "an egg crate of bumps and dips", params: &[("cells", 4.0), ("amount", 0.08)], formula: "amount * sin(cells * pi * x) * sin(cells * pi * y)" },
    HeightPreset { name: "sinc", about: "the sombrero: sin(r)/r", params: &[("frequency", 12.0), ("amount", 0.4)], formula: "k = frequency * r; amount * (k < 1e-6 ? 1 : sin(k) / k)" },
];

/// The height preset called `name`.
pub fn height_preset(name: &str) -> Option<&'static HeightPreset> {
    HEIGHTS.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Fit, compile, field::bind, open_edges, to_studio};

    fn surface(p: &Preset, n: usize) -> (Sheet, Formulas) {
        let src = bind(p.name, p.params, &[], p.formula).unwrap();
        let f = Formulas { parts: vec![compile(p.name, &src, NAMES).unwrap()], coords: p.coords };
        let r = |s: &str| compile("range", &bind("r", p.params, &[], s).unwrap(), NAMES).unwrap().eval(&[]);
        let (u, v) = ([r(p.u[0]), r(p.u[1])], [r(p.v[0]), r(p.v[1])]);
        (Sheet::sample(&|a, b| f.point(a, b, 0.0), u, v, n, n), f)
    }

    fn place(k: f32) -> impl Fn([f64; 3]) -> V3 {
        move |p| scale(to_studio(p), k)
    }

    #[test]
    fn a_parametric_sphere_closes_at_its_seam_and_poles_and_sits_on_the_formula() {
        let (sheet, f) = surface(preset("sphere").unwrap(), 48);
        let b = build(&sheet, &place(50.0), 0.0, None);
        assert!(b.closed, "{:?}", open_edges(&b.mesh));
        // Every vertex back on the formula's sphere of radius 1 (50 px).
        for p in &b.mesh.positions {
            assert!((length(*p) - 50.0).abs() < 1e-3);
        }
        // Round trip: the point at (u, v) is where the formula puts it.
        let q = f.point(0.7, 0.2, 0.0);
        assert!(((q[0].powi(2) + q[1].powi(2) + q[2].powi(2)).sqrt() - 1.0).abs() < 1e-12);
        // Outward and nearly the ball's volume.
        let v = crate::math::volume(&b.mesh);
        assert!(v > 0.97 * 4.0 / 3.0 * std::f64::consts::PI * 125_000.0 && v < 4.0 / 3.0 * std::f64::consts::PI * 125_000.0, "{v}");
        assert!(b.mesh.uvs.iter().all(|t| (0.0..=1.0).contains(&t[0]) && (0.0..=1.0).contains(&t[1])));
        // Smooth normals point out of the ball.
        assert!(b.mesh.positions.iter().zip(&b.mesh.normals).all(|(p, n)| dot(normalise(*p), *n) > 0.9));
    }

    #[test]
    fn a_torus_and_a_klein_bottle_close_and_a_mobius_strip_makes_a_solid_shell() {
        for name in ["torus", "klein", "swirl_torus"] {
            let (sheet, _) = surface(preset(name).unwrap(), 64);
            let b = build(&sheet, &place(40.0), 0.0, None);
            assert!(b.closed, "{name}: {:?}", open_edges(&b.mesh));
        }
        let (sheet, _) = surface(preset("mobius").unwrap(), 64);
        let open = build(&sheet, &place(100.0), 0.0, None);
        assert!(!open.closed, "a strip has an edge");
        let shell = build(&sheet, &place(100.0), 4.0, None);
        assert!(shell.closed, "{:?}", open_edges(&shell.mesh));
        assert!(crate::math::volume(&shell.mesh) > 0.0 && shell.wall_vertices() > 0);
    }

    #[test]
    fn a_uv_pattern_cuts_holes_and_the_shell_stays_closed() {
        let (mut sheet, _) = surface(preset("sphere").unwrap(), 96);
        let cut = compile("cut", "fract(u * 12) < 0.4 && fract(v * 8) < 0.5", CUT_NAMES).unwrap();
        let n = sheet.cut(&cut, [0.0, 6.28], [-1.57, 1.57], 0.0);
        assert!(n > 1000, "{n}");
        let lattice = build(&sheet, &place(100.0), 3.0, None);
        assert!(lattice.closed, "{:?}", open_edges(&lattice.mesh));
        let whole = build(&surface(preset("sphere").unwrap(), 96).0, &place(100.0), 3.0, None);
        assert!(crate::math::volume(&lattice.mesh) < crate::math::volume(&whole.mesh) * 0.9);
    }

    #[test]
    fn tubes_round_knots_and_open_curves_are_closed() {
        let p = tube_preset("torus_knot").unwrap();
        let f = compile("knot", &bind("k", p.params, &[], p.formula).unwrap(), NAMES).unwrap();
        let curve: Vec<[f64; 3]> = (0..=300).map(|k| {
            let u = std::f64::consts::TAU * k as f64 / 300.0;
            [f.eval(&[u, 0.0, 0.0, 0.0]), f.eval(&[u, 0.0, 0.0, 1.0]), f.eval(&[u, 0.0, 0.0, 2.0])]
        }).collect();
        let b = build(&tube(&curve, &[0.13], 12), &place(80.0), 0.0, None);
        assert!(b.closed, "{:?}", open_edges(&b.mesh));
        let helix: Vec<[f64; 3]> = (0..100).map(|k| [(k as f64 * 0.2).cos(), (k as f64 * 0.2).sin(), k as f64 * 0.02]).collect();
        let b = build(&tube(&helix, &[0.1], 8), &place(80.0), 0.0, None);
        assert!(b.closed && crate::math::volume(&b.mesh) > 0.0, "{:?}", open_edges(&b.mesh));
    }

    #[test]
    fn a_height_field_is_a_closed_block_with_its_top_up() {
        let h = compile("h", "0.2 * sin(4 * x) * cos(3 * y)", HEIGHT_NAMES).unwrap();
        let (sheet, back) = height(&|x, y| h.eval(&[x, y, 0.0, 0.0]), [-1.0, -1.0], [1.0, 1.0], 40, 30, 0.1);
        let place = place(100.0);
        let floor: Vec<V3> = back.iter().map(|p| place(*p)).collect();
        let b = build(&sheet, &place, 0.0, Some(&floor));
        assert!(b.closed, "{:?}", open_edges(&b.mesh));
        assert!(crate::math::volume(&b.mesh) > 0.0);
        // The top's normals point up (−y in Studio).
        let up = b.mesh.normals.iter().filter(|n| n[1] < -0.5).count();
        assert!(up > 40 * 30 / 2, "{up}");
        // Following the field to another time keeps the triangles.
        let moved = b.follow(&sheet, &place, Some(&floor));
        assert_eq!(moved.len(), b.mesh.positions.len());
        assert!(moved.iter().zip(&b.mesh.positions).all(|(a, c)| length(sub(*a, *c)) < 1e-3));
    }

    #[test]
    fn every_surface_preset_builds() {
        for p in PRESETS {
            let (sheet, _) = surface(p, 40);
            let b = build(&sheet, &place(50.0), p.thickness as f32 * 100.0, None);
            assert!(b.mesh.triangles() > 500, "{}", p.name);
            assert!(b.mesh.positions.iter().all(|q| q.iter().all(|x| x.is_finite())), "{}", p.name);
        }
        let _ = Fit::Largest(1.0);
    }
}
