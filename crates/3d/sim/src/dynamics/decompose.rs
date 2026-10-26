// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collision shapes made from a model.
//!
//! The model is first turned into a solid on a voxel grid: every cell its
//! surface passes through, plus every cell the outside cannot reach
//! (flooding in from the border). That makes any model solid, even an
//! open shell, a single-sided plane or a sheet of paper, and gives its
//! true volume. Then, in order:
//!
//! - a ball, if every corner is about as far from the middle;
//! - a box, if the solid fills its turned bounding box;
//! - one convex hull, if the solid fills most of its hull;
//! - otherwise convex parts: the solid is split (separate pieces first,
//!   then by planes across the part whose hull wastes the most space)
//!   until each part is nearly convex, and each part becomes a hull of the
//!   model's own surface inside it. A chair becomes its seat, back and
//!   legs; a mug its body and handle.
//!
//! Fixed scenery can use the triangles themselves instead ([`Fit::Mesh`]).

use super::hull::{Hull, sphere_directions};
use super::math::*;
use super::shape::{Shape, TriMesh};
use std::collections::BinaryHeap;
use std::sync::Arc;

/// How to make the shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    /// The simplest of ball, box, hull and parts that fits.
    Auto,
    Sphere,
    Box,
    Capsule,
    Hull,
    /// Convex parts, at most this many.
    Parts(usize),
    /// The triangles as they are (fixed scenery only).
    Mesh,
}

impl Fit {
    pub fn parse(s: &str) -> Option<Fit> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" | "" => Fit::Auto,
            "sphere" | "ball" => Fit::Sphere,
            "box" | "cube" => Fit::Box,
            "capsule" | "pill" | "cylinder" => Fit::Capsule,
            "hull" | "convex" | "convex_hull" => Fit::Hull,
            "parts" | "decomposed" | "compound" | "convex_parts" => Fit::Parts(16),
            "mesh" | "triangles" | "exact" => Fit::Mesh,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Fit::Auto => "auto",
            Fit::Sphere => "sphere",
            Fit::Box => "box",
            Fit::Capsule => "capsule",
            Fit::Hull => "hull",
            Fit::Parts(_) => "parts",
            Fit::Mesh => "mesh",
        }
    }
}

/// What was made.
#[derive(Clone, Debug)]
pub struct Shapes {
    /// Each shape in the model's frame.
    pub parts: Vec<(Shape, Pose)>,
    /// Which fit was used.
    pub fit: Fit,
    /// The solid's volume (m³ when the model is in metres).
    pub volume: f32,
    /// How much of the bounding box the solid fills (0..1).
    pub fill: f32,
}

/// A solid on a grid.
pub struct Voxels {
    pub lo: V3,
    pub cell: f32,
    pub dims: [usize; 3],
    /// 0 empty, 1 surface, 2 inside.
    pub cells: Vec<u8>,
    /// Points on the surface (for hulls of parts).
    pub samples: Vec<V3>,
}

impl Voxels {
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims[1] + y) * self.dims[0] + x
    }

    fn cell_of(&self, p: V3) -> [usize; 3] {
        std::array::from_fn(|k| (((p[k] - self.lo[k]) / self.cell) as isize).clamp(0, self.dims[k] as isize - 1) as usize)
    }

    pub fn centre(&self, x: usize, y: usize, z: usize) -> V3 {
        [self.lo[0] + (x as f32 + 0.5) * self.cell, self.lo[1] + (y as f32 + 0.5) * self.cell, self.lo[2] + (z as f32 + 0.5) * self.cell]
    }

    pub fn solid(&self, x: usize, y: usize, z: usize) -> bool {
        self.cells[self.idx(x, y, z)] != 0
    }

    pub fn count(&self) -> usize {
        self.cells.iter().filter(|&&c| c != 0).count()
    }

    pub fn volume(&self) -> f32 {
        self.count() as f32 * self.cell.powi(3)
    }

    /// The solid of a triangle surface, `res` cells along its longest side.
    pub fn of(tris: &[[V3; 3]], res: usize) -> Option<Voxels> {
        let mut b = Aabb::EMPTY;
        for t in tris {
            for v in t {
                b.grow(*v);
            }
        }
        if b.is_empty() {
            return None;
        }
        let size = b.size();
        let longest = size[0].max(size[1]).max(size[2]);
        if longest < 1e-9 {
            return None;
        }
        let res = res.clamp(8, 96);
        let cell = longest / res as f32;
        // One empty cell of border all round, for the flood.
        let lo = sub(b.lo, [cell * 1.5; 3]);
        let dims: [usize; 3] = std::array::from_fn(|k| ((size[k] / cell).ceil() as usize + 3).max(3));
        let mut v = Voxels { lo, cell, dims, cells: vec![0; dims[0] * dims[1] * dims[2]], samples: Vec::new() };
        // The surface: points spread over each triangle every half cell.
        let step = cell * 0.5;
        for t in tris {
            let e1 = sub(t[1], t[0]);
            let e2 = sub(t[2], t[0]);
            let n1 = (length(e1) / step).ceil().max(1.0) as usize;
            let n2 = (length(e2) / step).ceil().max(1.0) as usize;
            let n = n1.max(n2).min(400);
            for i in 0..=n {
                for j in 0..=(n - i) {
                    let p = add(t[0], add(scale(e1, i as f32 / n as f32), scale(e2, j as f32 / n as f32)));
                    let c = v.cell_of(p);
                    let k = v.idx(c[0], c[1], c[2]);
                    if v.cells[k] == 0 {
                        v.cells[k] = 1;
                    }
                }
            }
            v.samples.extend_from_slice(t);
            v.samples.push(scale(add(add(t[0], t[1]), t[2]), 1.0 / 3.0));
        }
        // Flood the outside from the border; what it cannot reach is inside.
        let mut outside = vec![false; v.cells.len()];
        let mut stack = vec![[0usize, 0, 0]];
        outside[0] = true;
        while let Some([x, y, z]) = stack.pop() {
            let near = [[x.wrapping_sub(1), y, z], [x + 1, y, z], [x, y.wrapping_sub(1), z], [x, y + 1, z], [x, y, z.wrapping_sub(1)], [x, y, z + 1]];
            for [a, bb, c] in near {
                if a >= dims[0] || bb >= dims[1] || c >= dims[2] {
                    continue;
                }
                let k = v.idx(a, bb, c);
                if !outside[k] && v.cells[k] == 0 {
                    outside[k] = true;
                    stack.push([a, bb, c]);
                }
            }
        }
        for k in 0..v.cells.len() {
            if v.cells[k] == 0 && !outside[k] {
                v.cells[k] = 2;
            }
        }
        Some(v)
    }
}

/// A part of the solid: its cells.
struct Region {
    cells: Vec<[u16; 3]>,
    waste: f32,
}

impl PartialEq for Region {
    fn eq(&self, o: &Region) -> bool {
        self.waste == o.waste
    }
}
impl Eq for Region {}
impl PartialOrd for Region {
    fn partial_cmp(&self, o: &Region) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Region {
    fn cmp(&self, o: &Region) -> std::cmp::Ordering {
        self.waste.total_cmp(&o.waste).then(o.cells.len().cmp(&self.cells.len()))
    }
}

/// The hull volume of cells (their corners in many directions).
fn cells_hull_volume(v: &Voxels, cells: &[[u16; 3]], dirs: &[V3]) -> f32 {
    hull_of_cells(v, cells, dirs, 48).map_or(0.0, |h| h.volume())
}

fn hull_of_cells(v: &Voxels, cells: &[[u16; 3]], dirs: &[V3], max_verts: usize) -> Option<Hull> {
    let h = v.cell * 0.5;
    let mut pts: Vec<V3> = Vec::with_capacity(dirs.len());
    for d in dirs {
        let mut best = f32::NEG_INFINITY;
        let mut bp = [0.0; 3];
        for c in cells {
            let ctr = v.centre(c[0] as usize, c[1] as usize, c[2] as usize);
            // The cell's corner furthest along d.
            let p = [ctr[0] + h * d[0].signum(), ctr[1] + h * d[1].signum(), ctr[2] + h * d[2].signum()];
            let s = dot(p, *d);
            if s > best {
                best = s;
                bp = p;
            }
        }
        pts.push(bp);
    }
    Hull::from_points(&pts, max_verts, v.cell)
}

/// Splits the solid into its connected pieces.
fn pieces(v: &Voxels) -> Vec<Vec<[u16; 3]>> {
    let mut seen = vec![false; v.cells.len()];
    let mut out = Vec::new();
    for z in 0..v.dims[2] {
        for y in 0..v.dims[1] {
            for x in 0..v.dims[0] {
                let k = v.idx(x, y, z);
                if v.cells[k] == 0 || seen[k] {
                    continue;
                }
                seen[k] = true;
                let mut part = Vec::new();
                let mut stack = vec![[x, y, z]];
                while let Some([a, b, c]) = stack.pop() {
                    part.push([a as u16, b as u16, c as u16]);
                    for dz in -1i32..=1 {
                        for dy in -1i32..=1 {
                            for dx in -1i32..=1 {
                                let (na, nb, nc) = (a as i32 + dx, b as i32 + dy, c as i32 + dz);
                                if na < 0 || nb < 0 || nc < 0 || na >= v.dims[0] as i32 || nb >= v.dims[1] as i32 || nc >= v.dims[2] as i32 {
                                    continue;
                                }
                                let kk = v.idx(na as usize, nb as usize, nc as usize);
                                if v.cells[kk] != 0 && !seen[kk] {
                                    seen[kk] = true;
                                    stack.push([na as usize, nb as usize, nc as usize]);
                                }
                            }
                        }
                    }
                }
                out.push(part);
            }
        }
    }
    out
}

/// The solid in nearly convex parts, at most `max_parts`.
pub fn convex_parts(v: &Voxels, max_parts: usize, tolerance: f32) -> Vec<Hull> {
    let dirs = sphere_directions(64);
    let cell_vol = v.cell.powi(3);
    let total = v.volume().max(1e-12);
    let waste_of = |cells: &[[u16; 3]]| (cells_hull_volume(v, cells, &dirs) - cells.len() as f32 * cell_vol).max(0.0) / total;
    let mut heap = BinaryHeap::new();
    let mut done: Vec<Vec<[u16; 3]>> = Vec::new();
    for p in pieces(v) {
        if p.len() < 2 && (v.cells.len() > 8) {
            // Specks: kept, but not split.
            done.push(p);
            continue;
        }
        let w = waste_of(&p);
        heap.push(Region { cells: p, waste: w });
    }
    let max_parts = max_parts.max(1);
    while let Some(r) = heap.pop() {
        if r.waste <= tolerance || heap.len() + done.len() + 1 >= max_parts || r.cells.len() < 8 {
            done.push(r.cells);
            continue;
        }
        // Planes across each axis; the cut that leaves the least waste.
        let mut lo = [u16::MAX; 3];
        let mut hi = [0u16; 3];
        for c in &r.cells {
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        let mut best: Option<(f32, usize, u16)> = None;
        for k in 0..3 {
            let span = hi[k] - lo[k];
            if span < 1 {
                continue;
            }
            let tries = span.min(10);
            for i in 1..=tries {
                let at = lo[k] + ((span as u32 * i as u32) / (tries as u32 + 1)) as u16 + 1;
                if at <= lo[k] || at > hi[k] {
                    continue;
                }
                let (a, b): (Vec<[u16; 3]>, Vec<[u16; 3]>) = r.cells.iter().partition(|c| c[k] < at);
                if a.is_empty() || b.is_empty() {
                    continue;
                }
                // Cuts near the middle of the part are slightly preferred.
                let balance = (a.len() as f32 / r.cells.len() as f32 - 0.5).abs() * 0.01;
                let cost = waste_of(&a) + waste_of(&b) + balance;
                if best.is_none_or(|x| cost < x.0) {
                    best = Some((cost, k, at));
                }
            }
        }
        match best {
            Some((cost, k, at)) if cost < r.waste => {
                let (a, b): (Vec<[u16; 3]>, Vec<[u16; 3]>) = r.cells.into_iter().partition(|c| c[k] < at);
                for p in [a, b] {
                    // A cut may leave pieces apart from each other.
                    let w = waste_of(&p);
                    heap.push(Region { cells: p, waste: w });
                }
            }
            _ => done.push(r.cells),
        }
    }
    // Each part: a hull of the surface points inside it, with the cut
    // faces filled from its cells.
    let mut owner = vec![u32::MAX; v.cells.len()];
    for (i, p) in done.iter().enumerate() {
        for c in p {
            owner[v.idx(c[0] as usize, c[1] as usize, c[2] as usize)] = i as u32;
        }
    }
    let mut points: Vec<Vec<V3>> = vec![Vec::new(); done.len()];
    for s in &v.samples {
        let c = v.cell_of(*s);
        let o = owner[v.idx(c[0], c[1], c[2])];
        if o != u32::MAX {
            points[o as usize].push(*s);
        }
    }
    let mut hulls = Vec::new();
    for (i, p) in done.iter().enumerate() {
        let mut pts = points[i].clone();
        // Cells on the part's boundary that are not on the model's surface
        // (where it was cut) contribute their centres.
        for c in p {
            let (x, y, z) = (c[0] as usize, c[1] as usize, c[2] as usize);
            if v.cells[v.idx(x, y, z)] != 2 {
                continue;
            }
            let edge = [[x.wrapping_sub(1), y, z], [x + 1, y, z], [x, y.wrapping_sub(1), z], [x, y + 1, z], [x, y, z.wrapping_sub(1)], [x, y, z + 1]].iter().any(|n| n[0] >= v.dims[0] || n[1] >= v.dims[1] || n[2] >= v.dims[2] || owner[v.idx(n[0], n[1], n[2])] != i as u32);
            if edge {
                pts.push(v.centre(x, y, z));
            }
        }
        let hull = if pts.len() >= 4 { Hull::from_points(&pts, 32, v.cell) } else { None };
        if let Some(h) = hull.or_else(|| hull_of_cells(v, p, &dirs, 32)) {
            hulls.push(h);
        }
    }
    hulls
}

/// Collision shapes for a model given as triangles in its own frame.
pub fn shapes_for(tris: &[[V3; 3]], fit: Fit, res: usize) -> Option<Shapes> {
    if tris.is_empty() {
        return None;
    }
    let mut b = Aabb::EMPTY;
    for t in tris {
        for v in t {
            b.grow(*v);
        }
    }
    let size = b.size();
    let box_vol = (size[0] * size[1] * size[2]).max(1e-12);
    if fit == Fit::Mesh {
        return Some(Shapes { parts: vec![(Shape::Mesh(Arc::new(TriMesh::new(tris.to_vec()))), Pose::IDENTITY)], fit, volume: 0.0, fill: 0.0 });
    }
    let vox = Voxels::of(tris, res)?;
    let solid = vox.volume();
    let fill = (solid / box_vol).min(1.0);
    let corners: Vec<V3> = tris.iter().flatten().copied().collect();
    let hull = Hull::from_points(&corners, 48, vox.cell)?;
    let hull_vol = hull.volume().max(1e-12);
    let centre = scale(add(b.lo, b.hi), 0.5);
    let done = |parts: Vec<(Shape, Pose)>, fit| Some(Shapes { parts, fit, volume: solid, fill });
    let sphere = || {
        let r = corners.iter().map(|p| length(sub(*p, centre))).fold(0.0f32, f32::max);
        (Shape::Sphere { r }, Pose::at(centre))
    };
    let oriented_box = || {
        // The box along the hull's principal axes.
        let mo = hull.moments();
        let (axes, _) = eigen_sym(&mo.cov);
        let q = q_from_mat(&axes);
        let frame = Pose::new(mo.com, q);
        let mut lb = Aabb::EMPTY;
        for p in &hull.verts {
            lb.grow(frame.unapply(*p));
        }
        let half = scale(lb.size(), 0.5);
        let mid = frame.apply(lb.centre());
        let half = half.map(|h| h.max(vox.cell * 0.5));
        (Shape::cuboid(half), Pose::new(mid, q), 8.0 * half[0] * half[1] * half[2])
    };
    let capsule = || {
        let mo = hull.moments();
        let (axes, vals) = eigen_sym(&mo.cov);
        // The long axis: the largest spread.
        let k = (0..3).max_by(|&a, &b| vals[a].total_cmp(&vals[b])).unwrap();
        let along = [axes[0][k], axes[1][k], axes[2][k]];
        let (lo_t, hi_t) = hull.verts.iter().map(|p| dot(sub(*p, mo.com), along)).fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), t| (a.min(t), b.max(t)));
        let mid_t = (lo_t + hi_t) * 0.5;
        let axis_pt = mad(mo.com, along, mid_t);
        let r = hull.verts.iter().map(|p| {
            let d = sub(*p, axis_pt);
            length(sub(d, scale(along, dot(d, along))))
        }).fold(0.0f32, f32::max).max(vox.cell * 0.5);
        let half = ((hi_t - lo_t) * 0.5 - r).max(0.0);
        let q = studio_mesh::rig::q_from_to([0.0, 1.0, 0.0], along);
        (Shape::Capsule { half, r }, Pose::new(axis_pt, q))
    };
    match fit {
        Fit::Sphere => return done(vec![sphere()], fit),
        Fit::Box => {
            let (s, p, _) = oriented_box();
            return done(vec![(s, p)], fit);
        }
        Fit::Capsule => return done(vec![capsule()], fit),
        Fit::Hull => return done(vec![(Shape::Hull(Arc::new(hull)), Pose::IDENTITY)], fit),
        Fit::Parts(n) => {
            let parts = convex_parts(&vox, n, 0.02).into_iter().map(|h| (Shape::Hull(Arc::new(h)), Pose::IDENTITY)).collect();
            return done(parts, fit);
        }
        _ => {}
    }
    // Auto: the simplest that fits.
    let dists: Vec<f32> = hull.verts.iter().map(|p| length(sub(*p, hull.moments().com))).collect();
    let mean = dists.iter().sum::<f32>() / dists.len() as f32;
    let spread = (dists.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / dists.len() as f32).sqrt();
    let even = size[0].min(size[1]).min(size[2]) > size[0].max(size[1]).max(size[2]) * 0.9;
    // Round, not just evenly cornered (a cube's corners are all as far
    // out too): its hull nearly fills the ball through its corners.
    let ball = 4.0 / 3.0 * std::f32::consts::PI * mean.powi(3);
    if spread < mean * 0.04 && even && hull.verts.len() >= 16 && hull_vol / ball > 0.75 && solid / hull_vol > 0.8 {
        let mo = hull.moments();
        return done(vec![(Shape::Sphere { r: mean.max(vox.cell * 0.5) }, Pose::at(mo.com))], Fit::Sphere);
    }
    let (bs, bp, bvol) = oriented_box();
    if hull_vol / bvol > 0.97 && solid / bvol > 0.85 {
        return done(vec![(bs, bp)], Fit::Box);
    }
    if solid / hull_vol > 0.88 {
        return done(vec![(Shape::Hull(Arc::new(hull)), Pose::IDENTITY)], Fit::Hull);
    }
    let parts: Vec<(Shape, Pose)> = convex_parts(&vox, 24, 0.02).into_iter().map(|h| (Shape::Hull(Arc::new(h)), Pose::IDENTITY)).collect();
    if parts.is_empty() {
        return done(vec![(Shape::Hull(Arc::new(hull)), Pose::IDENTITY)], Fit::Hull);
    }
    done(parts, Fit::Parts(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tris_of(m: &studio_mesh::Mesh) -> Vec<[V3; 3]> {
        (0..m.triangles()).map(|t| m.tri(t).map(|v| m.positions[v])).collect()
    }

    #[test]
    fn a_cube_is_a_box_and_a_ball_is_a_ball() {
        let cube = studio_mesh::prim::cube([2.0, 1.0, 0.5]);
        let s = shapes_for(&tris_of(&cube), Fit::Auto, 32).unwrap();
        assert_eq!(s.fit, Fit::Box, "{:?}", s.fit);
        assert!((s.volume - 1.0).abs() < 0.25, "{}", s.volume);
        let ball = studio_mesh::prim::ico_sphere(1.0, 3);
        let s = shapes_for(&tris_of(&ball), Fit::Auto, 32).unwrap();
        assert_eq!(s.fit, Fit::Sphere);
        // A cube has every corner equally far out, and is still a box.
        let s = shapes_for(&tris_of(&studio_mesh::prim::cube([1.0; 3])), Fit::Auto, 32).unwrap();
        assert_eq!(s.fit, Fit::Box);
        match &s.parts[0].0 {
            Shape::Hull(h) => assert!((h.volume() - 1.0).abs() < 0.02, "{}", h.volume()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_l_shape_comes_apart_into_two_convex_parts() {
        // Two boxes as one model: an L.
        let mut tris = tris_of(&studio_mesh::prim::cube([3.0, 1.0, 1.0]));
        let up: Vec<[V3; 3]> = tris_of(&studio_mesh::prim::cube([1.0, 3.0, 1.0])).into_iter().map(|t| t.map(|v| add(v, [-1.0, -2.0, 0.0]))).collect();
        tris.extend(up);
        let s = shapes_for(&tris, Fit::Auto, 40).unwrap();
        assert!(matches!(s.fit, Fit::Parts(_)), "{:?}", s.fit);
        assert!(s.parts.len() >= 2 && s.parts.len() <= 6, "{}", s.parts.len());
        let vol: f32 = s.parts.iter().map(|(p, _)| p.volume()).sum();
        assert!(vol > 4.0 && vol < 7.5, "parts cover the L: {vol}");
    }

    #[test]
    fn a_single_sided_plane_becomes_a_thin_solid() {
        let tris = vec![[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]], [[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]];
        let s = shapes_for(&tris, Fit::Auto, 24).unwrap();
        assert!(s.volume > 0.0);
        assert!(s.parts.iter().map(|(p, _)| p.volume()).sum::<f32>() > 0.0);
    }

    #[test]
    fn an_open_cup_is_solid_where_its_walls_are() {
        // A cube with its top missing: hollow inside, open.
        let mut tris = tris_of(&studio_mesh::prim::cube([1.0; 3]));
        tris.retain(|t| !t.iter().all(|v| (v[1] + 0.5).abs() < 1e-4));
        let v = Voxels::of(&tris, 24).unwrap();
        // The inside is reached by the flood through the open top: not solid.
        assert!(v.volume() < 0.6, "{}", v.volume());
    }
}
