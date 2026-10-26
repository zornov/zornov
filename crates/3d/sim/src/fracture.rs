// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shattering: a model cut into Voronoi cells (the pieces glass, stone and
//! wood break into: each piece is the part of the model nearer one seed
//! point than any other), smaller pieces around where it is struck. The
//! broken faces get a material of their own ("Inside"). The pieces then fly
//! apart as rigid bodies at a moment, or when something hits the model.

use crate::rigid::{Body, World, clump};
use crate::{Collider, Rng, V3, add, dot, length, normalise, scale, sub};
use studio_mesh::Mesh;
use studio_mesh::cache::{Cache, Frames};

/// A convex polyhedron as polygons (each wound outwards).
type Poly = Vec<Vec<V3>>;

fn box_poly(lo: V3, hi: V3) -> Poly {
    let c = |x: usize, y: usize, z: usize| [if x == 0 { lo[0] } else { hi[0] }, if y == 0 { lo[1] } else { hi[1] }, if z == 0 { lo[2] } else { hi[2] }];
    vec![
        vec![c(0, 0, 0), c(0, 0, 1), c(0, 1, 1), c(0, 1, 0)],
        vec![c(1, 0, 0), c(1, 1, 0), c(1, 1, 1), c(1, 0, 1)],
        vec![c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)],
        vec![c(0, 1, 0), c(0, 1, 1), c(1, 1, 1), c(1, 1, 0)],
        vec![c(0, 0, 0), c(0, 1, 0), c(1, 1, 0), c(1, 0, 0)],
        vec![c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)],
    ]
}

/// Keeps the side of the plane `dot(n, p) <= d`, closing the cut.
fn clip(poly: &Poly, n: V3, d: f32) -> Poly {
    let mut out: Poly = Vec::new();
    let mut cut: Vec<V3> = Vec::new();
    for face in poly {
        let mut f = Vec::new();
        for i in 0..face.len() {
            let (a, b) = (face[i], face[(i + 1) % face.len()]);
            let (da, db) = (dot(n, a) - d, dot(n, b) - d);
            if da <= 0.0 {
                f.push(a);
            }
            if (da <= 0.0) != (db <= 0.0) {
                let t = da / (da - db);
                let p = crate::lerp(a, b, t);
                f.push(p);
                cut.push(p);
            }
        }
        if f.len() >= 3 {
            out.push(f);
        }
    }
    if cut.len() >= 3 {
        // The cap: cut points in order around their centre, wound along n.
        let c = scale(cut.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / cut.len() as f32);
        let u = normalise(sub(cut[0], c));
        let v = crate::cross(n, u);
        cut.sort_by(|a, b| {
            let (da, db) = (sub(*a, c), sub(*b, c));
            dot(da, v).atan2(dot(da, u)).total_cmp(&dot(db, v).atan2(dot(db, u)))
        });
        cut.dedup_by(|a, b| length(sub(*a, *b)) < 1e-4);
        if cut.len() >= 3 {
            out.push(cut);
        }
    }
    out
}

fn poly_mesh(p: &Poly, material: studio_mesh::Material) -> Mesh {
    let mut m = Mesh { materials: vec![material], ..Default::default() };
    for face in p {
        let base = m.positions.len() as u32;
        let n = normalise(crate::cross(sub(face[1], face[0]), sub(face[2], face[0])));
        for v in face {
            m.positions.push(*v);
            m.normals.push(n);
            m.uvs.push([0.0, 0.0]);
        }
        for k in 1..face.len() as u32 - 1 {
            m.indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
    }
    m.single_group();
    m
}

/// How to break a model.
#[derive(Clone, Debug)]
pub struct Shatter {
    pub pieces: usize,
    /// Where it is struck (model px); pieces are smaller near it.
    pub impact: Option<V3>,
    pub seed: u64,
    /// The colour and finish of the broken faces.
    pub inside: studio_mesh::Material,
}

impl Default for Shatter {
    fn default() -> Shatter {
        Shatter { pieces: 16, impact: None, seed: 1, inside: studio_mesh::Material { name: "Inside".into(), base_colour: [0.6, 0.58, 0.55, 1.0], roughness: 0.9, ..studio_mesh::Material::default() } }
    }
}

/// The model broken into pieces: one mesh holding every piece (each a run
/// of vertices), the runs, and each piece's own mesh.
pub fn shatter(model: &Mesh, s: &Shatter) -> (Mesh, Vec<(u32, u32)>, Vec<Mesh>) {
    let (lo, hi) = model.bounds();
    let ext = sub(hi, lo);
    let pad = scale(ext, 0.02);
    let bvh = studio_mesh::bvh::Bvh::of_mesh(model);
    let inside = |p: V3| -> bool {
        let dir = normalise([1.0, 0.000_37, 0.000_21]);
        let mut t0 = 0.0;
        let mut n = 0;
        while let Some(h) = bvh.ray(p, dir, t0, f32::INFINITY) {
            n += 1;
            t0 = h.t + 1e-3;
            if n > 64 {
                break;
            }
        }
        n % 2 == 1
    };
    let mut rng = Rng::new(s.seed);
    let mut seeds: Vec<V3> = Vec::new();
    let want = s.pieces.clamp(2, 200);
    let mut tries = 0;
    while seeds.len() < want && tries < want * 400 {
        tries += 1;
        let p = match s.impact {
            // Half the seeds crowd around the impact.
            Some(c) if seeds.len() % 2 == 0 => add(c, scale(rng.ball(), length(ext) * 0.25)),
            _ => [rng.range(lo[0], hi[0]), rng.range(lo[1], hi[1]), rng.range(lo[2], hi[2])],
        };
        if (0..3).any(|i| p[i] < lo[i] || p[i] > hi[i]) {
            continue;
        }
        // Open models (no inside): any point in the box.
        if inside(p) || tries > want * 200 {
            seeds.push(p);
        }
    }
    let mut materials = model.materials.clone();
    materials.push(s.inside.clone());
    let mut all = Mesh { materials: materials.clone(), ..Default::default() };
    let mut tri_mats: Vec<u32> = Vec::new();
    let mut ranges = Vec::new();
    let mut parts = Vec::new();
    for (i, si) in seeds.iter().enumerate() {
        let mut cell = box_poly(sub(lo, pad), add(hi, pad));
        for (j, sj) in seeds.iter().enumerate() {
            if i == j {
                continue;
            }
            let n = normalise(sub(*sj, *si));
            let mid = scale(add(*si, *sj), 0.5);
            cell = clip(&cell, n, dot(n, mid));
            if cell.is_empty() {
                break;
            }
        }
        if cell.len() < 4 {
            continue;
        }
        let cell_mesh = poly_mesh(&cell, s.inside.clone());
        let piece = studio_mesh::csg::boolean(model, &cell_mesh, studio_mesh::csg::Op::Intersect);
        if piece.triangles() < 4 {
            continue;
        }
        let start = all.positions.len() as u32;
        let mats = piece.tri_materials();
        all.positions.extend_from_slice(&piece.positions);
        all.normals.extend_from_slice(&piece.normals);
        all.uvs.extend_from_slice(&piece.uvs);
        all.indices.extend(piece.indices.iter().map(|k| k + start));
        // The piece's own list is the model's then the cell's (Inside).
        tri_mats.extend(mats.iter().map(|m| (*m).min(materials.len() as u32 - 1)));
        ranges.push((start, piece.positions.len() as u32));
        parts.push(piece);
    }
    all.regroup(&tri_mats);
    all.normals.resize(all.positions.len(), [0.0; 3]);
    all.uvs.resize(all.positions.len(), [0.0; 2]);
    (all, ranges, parts)
}

/// When and how the pieces break loose.
#[derive(Clone, Debug)]
pub struct Break {
    /// Seconds in: the pieces hold together until then (unless struck).
    pub at: f32,
    /// Where the blow lands (model px; pieces fly from it).
    pub from: V3,
    /// Speed the nearest pieces leave at (px/s); further ones less.
    pub strength: f32,
    /// A push for everything (px/s), as the blow carries on.
    pub push: V3,
}

/// Runs the shattered pieces for `seconds` at `fps` and bakes their moves.
/// `place` puts a piece's model points in the world; `colliders` are the
/// floor and fixed models; `impactors` are moving bodies (a thrown ball)
/// that break it on contact. Returns the cache (piece moves in the
/// model's own frame) and when it broke.
#[allow(clippy::too_many_arguments)]
pub fn bake(parts: &[Mesh], ranges: &[(u32, u32)], b: &Break, gravity: V3, colliders: Vec<Collider>, mut impactors: Vec<Body>, seconds: f32, fps: f32, seed: u64) -> (Cache, f32) {
    let mut w = World::new(gravity);
    w.colliders = colliders;
    let mut rng = Rng::new(seed ^ 0x5eed);
    for p in parts {
        let mut body = clump(p, 5, 1.0);
        body.wait_until = f32::INFINITY;
        body.bounce = 0.25;
        body.friction = 0.6;
        w.bodies.push(body);
    }
    let n_parts = w.bodies.len();
    w.bodies.append(&mut impactors);
    let frames = (seconds * fps).round().max(1.0) as usize + 1;
    let mut data = Vec::with_capacity(frames * n_parts);
    let mut broke_at = f32::INFINITY;
    let release = |w: &mut World, at: f32, from: V3, rng: &mut Rng| {
        for body in w.bodies.iter_mut().take(n_parts) {
            let d = sub(body.pos, from);
            let dist = length(d).max(1.0);
            let reach = (1.0 / (1.0 + dist / 60.0)).max(0.15);
            body.vel = add(scale(normalise(add(d, scale(rng.dir(), dist * 0.3))), b.strength * reach), b.push);
            body.spin = scale(rng.dir(), rng.range(2.0, 9.0) * reach.sqrt());
            body.wait_until = at;
            body.asleep = false;
        }
    };
    for f in 0..frames {
        let t = f as f32 / fps;
        if f > 0 {
            w.advance(1.0 / fps);
        }
        // Broken by the clock, or by something reaching it.
        if broke_at.is_infinite() {
            let struck = (n_parts..w.bodies.len()).find_map(|k| {
                let imp = &w.bodies[k];
                let hits = w.bodies[..n_parts].iter().any(|piece| piece.world_spheres().any(|(c, r)| imp.world_spheres().any(|(c2, r2)| length(sub(c, c2)) < r + r2)));
                hits.then_some(imp.pos)
            });
            if t >= b.at || struck.is_some() {
                broke_at = t;
                let from = struck.unwrap_or(b.from);
                release(&mut w, t, from, &mut rng);
            }
        }
        for body in w.bodies.iter().take(n_parts) {
            data.push(body.placement());
        }
    }
    (Cache { fps, frames, data: Frames::Pieces { ranges: ranges[..n_parts].to_vec(), data } }, broke_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_shatters_into_pieces_that_fill_it_and_fall() {
        let model = studio_mesh::prim::cube([120.0, 120.0, 40.0]);
        let (all, ranges, parts) = shatter(&model, &Shatter { pieces: 8, impact: Some([0.0, 0.0, -20.0]), ..Default::default() });
        assert!(parts.len() >= 6, "{} pieces", parts.len());
        assert_eq!(ranges.len(), parts.len());
        // Together the pieces span the box; inside faces got the Inside material.
        let (lo, hi) = all.bounds();
        assert!((lo[0] + 60.0).abs() < 1.0 && (hi[1] - 60.0).abs() < 1.0);
        assert_eq!(all.materials.last().unwrap().name, "Inside");
        assert!(all.groups.iter().any(|g| g.material as usize == all.materials.len() - 1));
        let (cache, at) = bake(&parts, &ranges, &Break { at: 0.2, from: [0.0, 0.0, -20.0], strength: 300.0, push: [0.0; 3] }, [0.0, 980.0, 0.0], vec![Collider::Ground { y: 200.0, friction: 0.6 }], vec![], 1.5, 30.0, 3);
        assert!((at - 0.2).abs() < 0.05);
        // Before the break nothing moves; at the end pieces lie low.
        let still = cache.pieces_at(0.1).unwrap();
        assert!(still.iter().all(|(_, d)| length(*d) < 1e-3));
        let end = cache.pieces_at(1.5).unwrap();
        let moved = end.iter().filter(|(_, d)| length(*d) > 20.0).count();
        assert!(moved * 2 >= end.len(), "{moved} of {} moved", end.len());
    }
}
