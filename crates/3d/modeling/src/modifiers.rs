// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The modifier stack: changes laid over a model that stay editable —
//! mirror, arrays, solidify, bevel, subdivision, booleans, smoothing,
//! weld, displacement, and deformers (bend, twist, taper, wave, cast,
//! lattice, shrink-wrap). The base mesh is never touched; the stack is
//! run top to bottom each time the model is drawn or exported.

use crate::edit::{self, Plane};
use crate::poly::{Edge, Poly, Topology, ekey};
use crate::select::{Edges, Faces};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn half() -> f32 {
    0.5
}

/// One entry in a model's stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Modifier {
    /// Mirror across the model's own x, y and/or z middle, joined along
    /// the seam (`bisect`: cut away what's past the middle first).
    Mirror {
        #[serde(default)]
        axes: [bool; 3],
        #[serde(default)]
        bisect: bool,
        #[serde(default)]
        weld: f32,
    },
    /// Copies in a row, each `offset` px from the last plus `relative`
    /// times the model's size; `merge` welds touching copies.
    Array {
        count: usize,
        #[serde(default)]
        offset: V3,
        #[serde(default)]
        relative: V3,
        #[serde(default)]
        merge: bool,
    },
    /// Copies round an axis (spokes, bolts on a flange, petals).
    Radial {
        count: usize,
        #[serde(default)]
        axis: usize,
        #[serde(default = "full_turn")]
        degrees: f32,
        #[serde(default)]
        centre: V3,
    },
    /// Gives a sheet thickness (`offset` −1 inward … 1 outward).
    Solidify {
        thickness: f32,
        #[serde(default)]
        offset: f32,
        #[serde(default = "yes")]
        rim: bool,
    },
    /// Bevels edges sharper than `angle` degrees (or marked ones).
    Bevel {
        width: f32,
        #[serde(default = "one_seg")]
        segments: usize,
        #[serde(default = "half")]
        profile: f32,
        #[serde(default = "thirty")]
        angle: f32,
        #[serde(default)]
        only_marked: bool,
    },
    /// Catmull–Clark levels (creases respected).
    Subdivide {
        levels: usize,
    },
    /// Combine with another part of the model (by name).
    Boolean {
        op: String,
        with: String,
    },
    /// Relaxes the surface without shrinking it.
    Smooth {
        #[serde(default = "five")]
        iterations: usize,
        #[serde(default = "half")]
        factor: f32,
    },
    Weld {
        distance: f32,
    },
    Triangulate,
    /// Fewer faces (a fraction of what there is).
    Decimate {
        ratio: f32,
    },
    /// Pushes the surface in and out by a noise pattern.
    Displace {
        amount: f32,
        #[serde(default = "hundred")]
        size: f32,
        #[serde(default = "four")]
        octaves: usize,
        #[serde(default)]
        seed: u32,
    },
    /// Curves the model along `axis` toward `toward` by `degrees`.
    Bend {
        axis: usize,
        toward: usize,
        degrees: f32,
    },
    /// Turns the model progressively round `axis`.
    Twist {
        axis: usize,
        degrees: f32,
    },
    /// Narrows (or widens) the model along `axis`: `factor` at the far end.
    Taper {
        axis: usize,
        factor: f32,
    },
    /// Ripples along `axis`, pushing toward `toward`.
    Wave {
        axis: usize,
        toward: usize,
        amplitude: f32,
        wavelength: f32,
        #[serde(default)]
        phase: f32,
    },
    /// Pulls toward a sphere (`factor` 1 = fully round).
    Cast {
        #[serde(default = "one")]
        factor: f32,
    },
    /// Free-form deformation by a cage of control points.
    Lattice(Lattice),
    /// Snaps onto another part's surface (`offset` px above it).
    Shrinkwrap {
        target: String,
        #[serde(default)]
        offset: f32,
    },
    /// Hard-surface shading: big faces win the vertex normals.
    WeightedNormals,
    /// Rebuilds the surface from voxels `size` px across (even, closed,
    /// ready to sculpt).
    Remesh {
        size: f32,
    },
}

fn full_turn() -> f32 {
    360.0
}
fn one_seg() -> usize {
    1
}
fn thirty() -> f32 {
    30.0
}
fn five() -> usize {
    5
}
fn four() -> usize {
    4
}
fn hundred() -> f32 {
    100.0
}

/// A cage of control points over a box; moving them bends what's inside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lattice {
    /// Points per side (2 to 6).
    pub res: [usize; 3],
    /// The cage's box (empty: the model's own bounds).
    #[serde(default)]
    pub min: Option<V3>,
    #[serde(default)]
    pub max: Option<V3>,
    /// Moves of control points: ([i, j, k], offset in px).
    #[serde(default)]
    pub moves: Vec<([usize; 3], V3)>,
}

/// What a stack can look up while it runs (other parts, by name).
pub trait Parts {
    fn part(&self, name: &str) -> Option<Poly>;
}

impl Parts for HashMap<String, Poly> {
    fn part(&self, name: &str) -> Option<Poly> {
        self.get(name).cloned()
    }
}

pub struct NoParts;

impl Parts for NoParts {
    fn part(&self, _: &str) -> Option<Poly> {
        None
    }
}

/// What a stack leaves: the mesh and how its normals should be weighed.
#[derive(Clone, Debug)]
pub struct Evaluated {
    pub poly: Poly,
    pub weighted_normals: bool,
}

/// Runs a stack over a mesh.
pub fn apply(base: &Poly, stack: &[Modifier], parts: &dyn Parts) -> Result<Evaluated, String> {
    let mut p = base.clone();
    let mut weighted = false;
    for (i, m) in stack.iter().enumerate() {
        apply_one(&mut p, m, parts, &mut weighted).map_err(|e| format!("modifier {} ({}): {e}", i + 1, m.name()))?;
        if p.faces() > 4_000_000 {
            return Err(format!("modifier {} ({}) made {} faces; that's more than a model should hold", i + 1, m.name(), p.faces()));
        }
    }
    Ok(Evaluated { poly: p, weighted_normals: weighted })
}

impl Modifier {
    pub fn name(&self) -> &'static str {
        match self {
            Modifier::Mirror { .. } => "mirror",
            Modifier::Array { .. } => "array",
            Modifier::Radial { .. } => "radial",
            Modifier::Solidify { .. } => "solidify",
            Modifier::Bevel { .. } => "bevel",
            Modifier::Subdivide { .. } => "subdivide",
            Modifier::Boolean { .. } => "boolean",
            Modifier::Smooth { .. } => "smooth",
            Modifier::Weld { .. } => "weld",
            Modifier::Triangulate => "triangulate",
            Modifier::Decimate { .. } => "decimate",
            Modifier::Displace { .. } => "displace",
            Modifier::Bend { .. } => "bend",
            Modifier::Twist { .. } => "twist",
            Modifier::Taper { .. } => "taper",
            Modifier::Wave { .. } => "wave",
            Modifier::Cast { .. } => "cast",
            Modifier::Lattice(_) => "lattice",
            Modifier::Shrinkwrap { .. } => "shrinkwrap",
            Modifier::WeightedNormals => "weighted_normals",
            Modifier::Remesh { .. } => "remesh",
        }
    }
}

fn axis_ok(a: usize) -> Result<usize, String> {
    if a < 3 { Ok(a) } else { Err("axis is 0 (x), 1 (y) or 2 (z)".into()) }
}

fn apply_one(p: &mut Poly, m: &Modifier, parts: &dyn Parts, weighted: &mut bool) -> Result<(), String> {
    match m {
        Modifier::Mirror { axes, bisect, weld } => {
            let w = if *weld > 0.0 { *weld } else { 1e-3 * extent(p).max(1.0) };
            for (a, on) in axes.iter().enumerate() {
                if *on {
                    if *bisect {
                        let mut n = [0.0; 3];
                        n[a] = 1.0;
                        edit::bisect(p, Plane { at: [0.0; 3], normal: n }, Some(true), false);
                    }
                    edit::mirror(p, a, w);
                }
            }
        }
        Modifier::Array { count, offset, relative, merge } => {
            let count = (*count).clamp(1, 1000);
            let (lo, hi) = p.bounds();
            let size = sub(hi, lo);
            let step = add(*offset, [relative[0] * size[0], relative[1] * size[1], relative[2] * size[2]]);
            let one = p.clone();
            for i in 1..count {
                let mut c = one.clone();
                let d = scale(step, i as f32);
                c.map_positions(|v| add(v, d));
                c.materials.clear();
                let mats = p.materials.clone();
                p.merge(&c);
                p.materials = mats;
                p.ensure_materials();
            }
            if *merge {
                edit::merge_by_distance(p, 1e-3 * extent(p).max(1.0));
            }
        }
        Modifier::Radial { count, axis, degrees, centre } => {
            let a = axis_ok(*axis)?;
            let count = (*count).clamp(1, 1000);
            let full = (degrees.abs() - 360.0).abs() < 1e-3;
            let one = p.clone();
            for i in 1..count {
                let ang = degrees.to_radians() * i as f32 / if full { count as f32 } else { (count - 1).max(1) as f32 };
                let mut c = one.clone();
                c.map_positions(|v| add(rotate_axis(sub(v, *centre), a, ang), *centre));
                c.materials.clear();
                let mats = p.materials.clone();
                p.merge(&c);
                p.materials = mats;
                p.ensure_materials();
            }
        }
        Modifier::Solidify { thickness, offset, rim } => solidify(p, *thickness, *offset, *rim),
        Modifier::Bevel { width, segments, profile, angle, only_marked } => {
            let edges = if *only_marked { Edges::Or(vec![Edges::MarkedSharp, Edges::Creased]).list(p) } else { Edges::Sharper { degrees: *angle }.list(p) };
            crate::bevel::bevel(p, &edges, *width, *segments, *profile);
        }
        Modifier::Subdivide { levels } => {
            if *levels > 6 {
                return Err("at most 6 levels".into());
            }
            *p = crate::subd::subdivide(p, *levels);
        }
        Modifier::Boolean { op, with } => {
            let o = studio_mesh::csg::Op::parse(op).ok_or("op is union, subtract or intersect")?;
            let other = parts.part(with).ok_or_else(|| format!("no part named \"{with}\""))?;
            *p = boolean(p, &other, o);
        }
        Modifier::Smooth { iterations, factor } => smooth(p, *iterations, *factor, &vec![true; p.positions.len()]),
        Modifier::Weld { distance } => {
            edit::merge_by_distance(p, *distance);
        }
        Modifier::Triangulate => {
            let all = vec![true; p.faces()];
            edit::triangulate_faces(p, &all);
        }
        Modifier::Decimate { ratio } => {
            let mut mesh = p.to_mesh(180.0, crate::NormalMode::Angle);
            let target = ((mesh.triangles() as f32) * ratio.clamp(0.01, 1.0)) as usize;
            studio_mesh::decimate::decimate(&mut mesh, target);
            let mats = p.materials.clone();
            *p = Poly::from_mesh(&mesh);
            p.materials = mats;
            p.ensure_materials();
        }
        Modifier::Displace { amount, size, octaves, seed } => {
            let normals = vertex_normals(p);
            let s = size.max(1e-3);
            for (v, q) in p.positions.iter_mut().enumerate() {
                let n = studio_mesh::ops::fbm(scale(*q, 1.0 / s), (*octaves).clamp(1, 8), *seed);
                *q = add(*q, scale(normals[v], amount * n));
            }
        }
        Modifier::Bend { axis, toward, degrees } => {
            let (a, t) = (axis_ok(*axis)?, axis_ok(*toward)?);
            if a == t {
                return Err("axis and toward must differ".into());
            }
            bend(p, a, t, degrees.to_radians());
        }
        Modifier::Twist { axis, degrees } => {
            let a = axis_ok(*axis)?;
            let (lo, hi) = p.bounds();
            let len = (hi[a] - lo[a]).max(1e-6);
            let c = scale(add(lo, hi), 0.5);
            p.map_positions(|v| {
                let s = (v[a] - lo[a]) / len - 0.5;
                add(rotate_axis(sub(v, c), a, degrees.to_radians() * s), c)
            });
        }
        Modifier::Taper { axis, factor } => {
            let a = axis_ok(*axis)?;
            let (lo, hi) = p.bounds();
            let len = (hi[a] - lo[a]).max(1e-6);
            let c = scale(add(lo, hi), 0.5);
            p.map_positions(|v| {
                let s = (v[a] - lo[a]) / len;
                let k = 1.0 + (factor - 1.0) * s;
                let mut o = v;
                for i in 0..3 {
                    if i != a {
                        o[i] = c[i] + (v[i] - c[i]) * k;
                    }
                }
                o
            });
        }
        Modifier::Wave { axis, toward, amplitude, wavelength, phase } => {
            let (a, t) = (axis_ok(*axis)?, axis_ok(*toward)?);
            let wl = wavelength.max(1e-3);
            p.map_positions(|mut v| {
                v[t] += amplitude * (std::f32::consts::TAU * v[a] / wl + phase).sin();
                v
            });
        }
        Modifier::Cast { factor } => {
            let (lo, hi) = p.bounds();
            let c = scale(add(lo, hi), 0.5);
            let r = p.positions.iter().map(|v| length(sub(*v, c))).sum::<f32>() / p.positions.len().max(1) as f32;
            p.map_positions(|v| lerp(v, add(c, scale(normalise(sub(v, c)), r)), factor.clamp(-1.0, 2.0)));
        }
        Modifier::Lattice(l) => lattice(p, l)?,
        Modifier::Shrinkwrap { target, offset } => {
            let t = parts.part(target).ok_or_else(|| format!("no part named \"{target}\""))?;
            shrinkwrap(p, &t, *offset);
        }
        Modifier::WeightedNormals => *weighted = true,
        Modifier::Remesh { size } => {
            let mats = p.materials.clone();
            *p = crate::remesh::voxel(p, *size)?;
            p.materials = mats;
            p.ensure_materials();
        }
    }
    Ok(())
}

fn extent(p: &Poly) -> f32 {
    let (lo, hi) = p.bounds();
    length(sub(hi, lo))
}

pub fn rotate_axis(v: V3, axis: usize, a: f32) -> V3 {
    let (s, c) = a.sin_cos();
    match axis {
        0 => [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c],
        1 => [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c],
        _ => [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]],
    }
}

/// Smooth vertex normals (area weighted).
pub fn vertex_normals(p: &Poly) -> Vec<V3> {
    let mut n = vec![[0.0; 3]; p.positions.len()];
    for f in 0..p.faces() {
        let a = p.face_area_vector(f);
        for &v in p.face(f) {
            n[v as usize] = add(n[v as usize], a);
        }
    }
    n.into_iter().map(normalise).collect()
}

/// Thickens a sheet (or a closed shell, inward).
pub fn solidify(p: &mut Poly, thickness: f32, offset: f32, rim: bool) {
    let normals = vertex_normals(p);
    let t = Topology::of(p);
    // Even thickness: divide by how far the vertex normal leans from its faces.
    let mut lean = vec![1.0f32; p.positions.len()];
    for f in 0..p.faces() {
        let fnm = p.face_normal(f);
        for &v in p.face(f) {
            lean[v as usize] = lean[v as usize].min(dot(fnm, normals[v as usize]).max(0.3));
        }
    }
    let (out_k, in_k) = ((1.0 + offset) / 2.0, (1.0 - offset) / 2.0);
    let n = p.positions.len() as u32;
    let original = p.clone();
    for v in 0..n as usize {
        p.positions[v] = add(original.positions[v], scale(normals[v], thickness * out_k / lean[v]));
    }
    for v in 0..n as usize {
        p.positions.push(sub(original.positions[v], scale(normals[v], thickness * in_k / lean[v])));
    }
    for f in 0..original.faces() {
        let mut c: Vec<u32> = original.face(f).iter().map(|&v| v + n).collect();
        c.reverse();
        p.add_face_like(&c, None, original.face_material[f], original.face_set[f], original.face_smooth[f]);
    }
    if rim {
        for e in t.boundary_edges() {
            let f = t.edge_faces[&e][0] as usize;
            let c = original.face(f);
            let s = crate::poly::edge_slot(c, e).unwrap_or(0);
            let (a, b) = (c[s], c[(s + 1) % c.len()]);
            p.add_face_like(&[b, a, a + n, b + n], None, original.face_material[f], original.face_set[f], original.face_smooth[f]);
            p.sharp.insert(ekey(a, b));
            p.sharp.insert(ekey(a + n, b + n));
        }
    }
    p.uvs.clear();
}

/// Relaxes picked vertices (Taubin's shrink-free smoothing); open rims
/// and creased edges hold their line.
pub fn smooth(p: &mut Poly, iterations: usize, factor: f32, pick: &[bool]) {
    let t = Topology::of(p);
    let nb: Vec<Vec<u32>> = (0..p.positions.len() as u32).map(|v| t.neighbours(p, v)).collect();
    let pinned: Vec<bool> = {
        let mut pin = vec![false; p.positions.len()];
        for e in t.boundary_edges() {
            pin[e.0 as usize] = true;
            pin[e.1 as usize] = true;
        }
        for (e, w) in &p.creases {
            if *w >= 0.99 {
                pin[e.0 as usize] = true;
                pin[e.1 as usize] = true;
            }
        }
        pin
    };
    let (lambda, mu) = (factor.clamp(0.0, 1.0) * 0.6307, -factor.clamp(0.0, 1.0) * 0.6732);
    for _ in 0..iterations.min(500) {
        for k in [lambda, mu] {
            let cur = p.positions.clone();
            for v in 0..cur.len() {
                if !pick.get(v).copied().unwrap_or(false) || pinned[v] || nb[v].is_empty() {
                    continue;
                }
                let mut c = [0.0; 3];
                for &u in &nb[v] {
                    c = add(c, cur[u as usize]);
                }
                c = scale(c, 1.0 / nb[v].len() as f32);
                p.positions[v] = add(cur[v], scale(sub(c, cur[v]), k));
            }
        }
    }
}

fn bend(p: &mut Poly, a: usize, t: usize, angle: f32) {
    if angle.abs() < 1e-6 {
        return;
    }
    let (lo, hi) = p.bounds();
    let len = (hi[a] - lo[a]).max(1e-6);
    let c = scale(add(lo, hi), 0.5);
    let r = len / angle;
    p.map_positions(|v| {
        let s = v[a] - c[a];
        let d = v[t] - c[t];
        let th = s / r;
        let mut o = v;
        o[a] = c[a] + (r - d) * th.sin();
        o[t] = c[t] + r - (r - d) * th.cos();
        o
    });
}

fn bernstein(n: usize, i: usize, t: f32) -> f32 {
    let mut c = 1.0f32;
    for k in 0..i {
        c = c * (n - k) as f32 / (k + 1) as f32;
    }
    c * t.powi(i as i32) * (1.0 - t).powi((n - i) as i32)
}

fn lattice(p: &mut Poly, l: &Lattice) -> Result<(), String> {
    let res = [l.res[0].clamp(2, 6), l.res[1].clamp(2, 6), l.res[2].clamp(2, 6)];
    let (blo, bhi) = p.bounds();
    let (lo, hi) = (l.min.unwrap_or(blo), l.max.unwrap_or(bhi));
    let size: V3 = std::array::from_fn(|i| (hi[i] - lo[i]).max(1e-6));
    let mut moves: HashMap<[usize; 3], V3> = HashMap::new();
    for (ijk, d) in &l.moves {
        if (0..3).any(|i| ijk[i] >= res[i]) {
            return Err(format!("lattice point {ijk:?} is outside a {res:?} cage"));
        }
        let e = moves.entry(*ijk).or_insert([0.0; 3]);
        *e = add(*e, *d);
    }
    p.map_positions(|v| {
        let s: V3 = std::array::from_fn(|i| ((v[i] - lo[i]) / size[i]).clamp(0.0, 1.0));
        let mut off = [0.0; 3];
        for (ijk, d) in &moves {
            let w = bernstein(res[0] - 1, ijk[0], s[0]) * bernstein(res[1] - 1, ijk[1], s[1]) * bernstein(res[2] - 1, ijk[2], s[2]);
            off = add(off, scale(*d, w));
        }
        add(v, off)
    });
    Ok(())
}

fn shrinkwrap(p: &mut Poly, target: &Poly, offset: f32) {
    let mesh = target.to_mesh(180.0, crate::NormalMode::Angle);
    let tree = crate::spatial::Tree::of_mesh(&mesh);
    for q in p.positions.iter_mut() {
        if let Some((hit, tri, _)) = tree.nearest(*q, f32::INFINITY) {
            let t = &tree.tris[tri as usize];
            let n = normalise(cross(sub(t[1], t[0]), sub(t[2], t[0])));
            *q = add(hit, scale(n, offset));
        }
    }
}

/// A boolean of two polygon meshes, flat runs of the result joined back
/// into n-gons.
pub fn boolean(a: &Poly, b: &Poly, op: studio_mesh::csg::Op) -> Poly {
    let ma = a.to_mesh(180.0, crate::NormalMode::Angle);
    let mb = b.to_mesh(180.0, crate::NormalMode::Angle);
    let r = studio_mesh::csg::boolean(&ma, &mb, op);
    let mut p = Poly::from_mesh(&r);
    let (lo, hi) = p.bounds();
    let tol = 1e-4 * length(sub(hi, lo)).max(1.0);
    edit::merge_by_distance(&mut p, tol);
    edit::fix_t_junctions(&mut p, tol * 10.0);
    limited_dissolve(&mut p, 0.5);
    p
}

/// Joins neighbouring faces that lie flat together (within `degrees`)
/// and share a material into single faces.
pub fn limited_dissolve(p: &mut Poly, degrees: f32) -> usize {
    let t = Topology::of(p);
    let c = degrees.to_radians().cos();
    let edges: Vec<Edge> = t
        .edge_faces
        .iter()
        .filter(|(e, f)| f.len() == 2 && !p.sharp.contains(e) && p.face_material[f[0] as usize] == p.face_material[f[1] as usize] && dot(p.face_normal(f[0] as usize), p.face_normal(f[1] as usize)) >= c)
        .map(|(e, _)| *e)
        .collect();
    edit::dissolve_edges(p, &edges)
}

/// Faces of a pick, for modifiers that take one later.
pub fn faces_of(p: &Poly, f: &Faces) -> Vec<bool> {
    f.mask(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    fn run(p: &Poly, m: Vec<Modifier>) -> Poly {
        apply(p, &m, &NoParts).unwrap().poly
    }

    #[test]
    fn mirror_array_and_radial() {
        let mut half = prim::cube([2.0; 3], 1);
        half.map_positions(|v| [v[0] + 1.0, v[1], v[2]]);
        let m = run(&half, vec![Modifier::Mirror { axes: [true, false, false], bisect: false, weld: 0.0 }]);
        let (lo, hi) = m.bounds();
        assert!((lo[0] + 2.0).abs() < 1e-5 && (hi[0] - 2.0).abs() < 1e-5);
        assert_eq!(m.positions.len(), 12, "the seam welded");
        let a = run(&prim::cube([2.0; 3], 1), vec![Modifier::Array { count: 3, offset: [0.0; 3], relative: [1.0, 0.0, 0.0], merge: true }]);
        assert_eq!(a.faces(), 18);
        assert_eq!(a.positions.len(), 16);
        let r = run(&prim::cube([1.0; 3], 1), vec![Modifier::Radial { count: 6, axis: 1, degrees: 360.0, centre: [-3.0, 0.0, 0.0] }]);
        assert_eq!(r.faces(), 36);
    }

    #[test]
    fn solidify_makes_a_sheet_closed() {
        let s = run(&prim::plane(10.0, 10.0, 4, 4), vec![Modifier::Solidify { thickness: 1.0, offset: 0.0, rim: true }]);
        assert!(Topology::of(&s).boundary_edges().is_empty());
        let (lo, hi) = s.bounds();
        assert!((hi[1] - lo[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn hard_surface_stack() {
        // A box, bevelled, subdivided: rounder corners but flat faces.
        let s = run(&prim::cube([2.0; 3], 1), vec![Modifier::Bevel { width: 0.1, segments: 2, profile: 0.5, angle: 30.0, only_marked: false }, Modifier::Subdivide { levels: 1 }, Modifier::WeightedNormals]);
        let (lo, hi) = s.bounds();
        assert!((hi[0] - lo[0] - 2.0).abs() < 0.02, "{:?}", (lo, hi));
        assert!(Topology::of(&s).boundary_edges().is_empty());
    }

    #[test]
    fn deformers_move_the_right_way() {
        let bar = prim::cube([10.0, 1.0, 1.0], 8);
        let b = run(&bar, vec![Modifier::Bend { axis: 0, toward: 1, degrees: 90.0 }]);
        assert!(b.bounds().1[1] > 2.0, "the ends rise: {:?}", b.bounds());
        let t = run(&bar, vec![Modifier::Taper { axis: 0, factor: 0.0 }]);
        let tip: Vec<&V3> = t.positions.iter().filter(|v| v[0] > 4.99).collect();
        assert!(tip.iter().all(|v| v[1].abs() < 1e-4 && v[2].abs() < 1e-4));
        let l = run(&bar, vec![Modifier::Lattice(Lattice { res: [2, 2, 2], min: None, max: None, moves: vec![([1, 0, 0], [0.0, -5.0, 0.0]), ([1, 0, 1], [0.0, -5.0, 0.0])] })]);
        assert!(l.bounds().0[1] < -5.0);
        let c = run(&prim::cube([2.0; 3], 4), vec![Modifier::Cast { factor: 1.0 }]);
        let r: Vec<f32> = c.positions.iter().map(|v| length(*v)).collect();
        assert!(r.iter().all(|x| (x - r[0]).abs() < 1e-3));
    }

    #[test]
    fn booleans_keep_flat_faces_as_ngons() {
        let mut parts = HashMap::new();
        parts.insert("hole".to_string(), prim::cylinder(0.5, 0.5, 4.0, 16, 1, prim::Cap::Ngon));
        let base = prim::cube([2.0; 3], 1);
        let r = apply(&base, &[Modifier::Boolean { op: "subtract".into(), with: "hole".into() }], &parts).unwrap().poly;
        assert!(Topology::of(&r).boundary_edges().is_empty());
        assert!(r.faces() < 120, "{}", r.stats());
    }

    #[test]
    fn smoothing_does_not_shrink_much() {
        let s = prim::quad_sphere(10.0, 6);
        let mut noisy = s.clone();
        for (i, v) in noisy.positions.iter_mut().enumerate() {
            let k = if i % 2 == 0 { 1.05 } else { 0.95 };
            *v = scale(*v, k);
        }
        let rms = |p: &Poly| {
            let r: f32 = p.positions.iter().map(|v| length(*v)).sum::<f32>() / p.positions.len() as f32;
            (r, (p.positions.iter().map(|v| (length(*v) - r).powi(2)).sum::<f32>() / p.positions.len() as f32).sqrt())
        };
        let out = run(&noisy, vec![Modifier::Smooth { iterations: 10, factor: 0.5 }]);
        let (r, after) = rms(&out);
        let (_, before) = rms(&noisy);
        assert!(r > 9.5, "{r}");
        assert!(after < before * 0.5, "{before} -> {after}");
    }

    #[test]
    fn stacks_read_from_json() {
        let j = r#"[{"kind":"mirror","axes":[true,false,false]},{"kind":"bevel","width":0.1},{"kind":"subdivide","levels":2},{"kind":"weighted_normals"}]"#;
        let s: Vec<Modifier> = serde_json::from_str(j).unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s[1], Modifier::Bevel { width: 0.1, segments: 1, profile: 0.5, angle: 30.0, only_marked: false });
    }
}
