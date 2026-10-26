// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Booleans: union, subtract and intersect of two closed models, by
//! binary space partitioning (each model's polygons clip the other's).
//! Texture coordinates and normals are interpolated where polygons are
//! cut; each side keeps its materials.

use crate::{Mesh, V3, cross, dot, lerp, normalise, scale, sub};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Subtract,
    Intersect,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "union" | "add" | "join" | "combine" => Op::Union,
            "subtract" | "difference" | "cut" | "minus" | "carve" => Op::Subtract,
            "intersect" | "intersection" | "common" => Op::Intersect,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Vert {
    p: V3,
    n: V3,
    uv: [f32; 2],
}

impl Vert {
    fn lerp(&self, o: &Vert, t: f32) -> Vert {
        Vert { p: lerp(self.p, o.p, t), n: normalise(lerp(self.n, o.n, t)), uv: [self.uv[0] + (o.uv[0] - self.uv[0]) * t, self.uv[1] + (o.uv[1] - self.uv[1]) * t] }
    }
}

#[derive(Clone, Copy, Debug)]
struct Plane {
    n: V3,
    w: f32,
}

const EPS: f32 = 1e-3;

impl Plane {
    fn from(a: V3, b: V3, c: V3) -> Option<Plane> {
        let n = normalise(cross(sub(b, a), sub(c, a)));
        (n != [0.0; 3]).then(|| Plane { n, w: dot(n, a) })
    }
    fn flip(&mut self) {
        self.n = scale(self.n, -1.0);
        self.w = -self.w;
    }

    /// Sorts `poly` into the lists by which side of the plane it lies,
    /// cutting it where it crosses.
    fn split(&self, poly: &Poly, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, front: &mut Vec<Poly>, back: &mut Vec<Poly>) {
        const COPLANAR: u8 = 0;
        const FRONT: u8 = 1;
        const BACK: u8 = 2;
        let mut kind = 0u8;
        let types: Vec<u8> = poly
            .v
            .iter()
            .map(|v| {
                let t = dot(self.n, v.p) - self.w;
                let k = if t < -EPS { BACK } else if t > EPS { FRONT } else { COPLANAR };
                kind |= k;
                k
            })
            .collect();
        match kind {
            COPLANAR => {
                if dot(self.n, poly.plane.n) > 0.0 { cf.push(poly.clone()) } else { cb.push(poly.clone()) }
            }
            FRONT => front.push(poly.clone()),
            BACK => back.push(poly.clone()),
            _ => {
                let (mut f, mut b) = (Vec::new(), Vec::new());
                let n = poly.v.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let (ti, tj) = (types[i], types[j]);
                    let (vi, vj) = (poly.v[i], poly.v[j]);
                    if ti != BACK {
                        f.push(vi);
                    }
                    if ti != FRONT {
                        b.push(vi);
                    }
                    if (ti | tj) == 3 {
                        let t = (self.w - dot(self.n, vi.p)) / dot(self.n, sub(vj.p, vi.p));
                        let v = vi.lerp(&vj, t);
                        f.push(v);
                        b.push(v);
                    }
                }
                if f.len() >= 3 {
                    front.push(Poly { v: f, plane: poly.plane, mat: poly.mat });
                }
                if b.len() >= 3 {
                    back.push(Poly { v: b, plane: poly.plane, mat: poly.mat });
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Poly {
    v: Vec<Vert>,
    plane: Plane,
    mat: u32,
}

impl Poly {
    fn flip(&mut self) {
        self.v.reverse();
        for v in &mut self.v {
            v.n = scale(v.n, -1.0);
        }
        self.plane.flip();
    }
}

#[derive(Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polys: Vec<Poly>,
}

impl Node {
    fn new(polys: Vec<Poly>) -> Node {
        let mut n = Node::default();
        n.build(polys, 0);
        n
    }

    fn invert(&mut self) {
        for p in &mut self.polys {
            p.flip();
        }
        if let Some(pl) = &mut self.plane {
            pl.flip();
        }
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// The parts of `polys` outside this solid.
    fn clip_polys(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let Some(plane) = self.plane else { return polys };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        for p in &polys {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            plane.split(p, &mut cf, &mut cb, &mut f, &mut b);
            f.extend(cf);
            b.extend(cb);
        }
        let mut f = match &self.front {
            Some(n) => n.clip_polys(f),
            None => f,
        };
        let b = match &self.back {
            Some(n) => n.clip_polys(b),
            None => Vec::new(),
        };
        f.extend(b);
        f
    }

    fn clip_to(&mut self, other: &Node) {
        self.polys = other.clip_polys(std::mem::take(&mut self.polys));
        if let Some(f) = &mut self.front {
            f.clip_to(other);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(other);
        }
    }

    fn all(&self, out: &mut Vec<Poly>) {
        out.extend(self.polys.iter().cloned());
        if let Some(f) = &self.front {
            f.all(out);
        }
        if let Some(b) = &self.back {
            b.all(out);
        }
    }

    fn build(&mut self, polys: Vec<Poly>, depth: usize) {
        if polys.is_empty() {
            return;
        }
        // Very deep trees come from degenerate input; keep what is left here.
        if depth > 4000 {
            self.polys.extend(polys);
            return;
        }
        let plane = *self.plane.get_or_insert(polys[0].plane);
        let (mut f, mut b) = (Vec::new(), Vec::new());
        for p in &polys {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            plane.split(p, &mut cf, &mut cb, &mut f, &mut b);
            self.polys.extend(cf);
            self.polys.extend(cb);
        }
        if !f.is_empty() {
            self.front.get_or_insert_with(Default::default).build(f, depth + 1);
        }
        if !b.is_empty() {
            self.back.get_or_insert_with(Default::default).build(b, depth + 1);
        }
    }
}

fn to_polys(m: &Mesh, mat_base: u32) -> Vec<Poly> {
    let mats = m.tri_materials();
    (0..m.triangles())
        .filter_map(|t| {
            let [a, b, c] = m.tri(t);
            let v = |i: usize| Vert { p: m.positions[i], n: m.normals.get(i).copied().unwrap_or([0.0; 3]), uv: m.uvs.get(i).copied().unwrap_or([0.0; 2]) };
            Plane::from(m.positions[a], m.positions[b], m.positions[c]).map(|plane| Poly { v: vec![v(a), v(b), v(c)], plane, mat: mats[t] + mat_base })
        })
        .collect()
}

fn from_polys(polys: Vec<Poly>, materials: Vec<crate::Material>) -> Mesh {
    let mut m = Mesh { materials, ..Default::default() };
    let mut mats = Vec::new();
    for p in polys {
        let base = m.positions.len() as u32;
        for v in &p.v {
            m.positions.push(v.p);
            m.normals.push(v.n);
            m.uvs.push(v.uv);
        }
        // Convex polygons fan out from their first corner.
        for k in 1..p.v.len() as u32 - 1 {
            m.indices.extend_from_slice(&[base, base + k, base + k + 1]);
            mats.push(p.mat);
        }
    }
    m.regroup(&mats);
    crate::ops::weld(&mut m);
    m
}

/// `a` combined with `b`. Both should be closed (watertight) surfaces.
/// Large models are slow (the work grows faster than the triangle count);
/// decimate first when either has more than about fifty thousand.
pub fn boolean(a: &Mesh, b: &Mesh, op: Op) -> Mesh {
    let mut materials = a.materials.clone();
    materials.extend(b.materials.iter().cloned());
    let mut na = Node::new(to_polys(a, 0));
    let mut nb = Node::new(to_polys(b, a.materials.len() as u32));
    match op {
        Op::Union => {
            na.clip_to(&nb);
            nb.clip_to(&na);
            nb.invert();
            nb.clip_to(&na);
            nb.invert();
        }
        Op::Subtract => {
            na.invert();
            na.clip_to(&nb);
            nb.clip_to(&na);
            nb.invert();
            nb.clip_to(&na);
            nb.invert();
        }
        Op::Intersect => {
            na.invert();
            nb.clip_to(&na);
            nb.invert();
            na.clip_to(&nb);
            nb.clip_to(&na);
        }
    }
    let mut all = Vec::new();
    na.all(&mut all);
    nb.all(&mut all);
    if op != Op::Union {
        for p in &mut all {
            p.flip();
        }
    }
    from_polys(all, materials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    fn volume(m: &Mesh) -> f32 {
        (0..m.triangles()).map(|t| {
            let [a, b, c] = m.tri(t);
            dot(m.positions[a], cross(m.positions[b], m.positions[c])) / 6.0
        }).sum()
    }

    #[test]
    fn subtract_a_ball_from_a_box() {
        let a = prim::cube([100.0; 3]);
        let mut b = prim::ico_sphere(40.0, 2);
        crate::ops::transform(&mut b, &crate::region::Region::All, [50.0, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        let v0 = volume(&a);
        let cut = boolean(&a, &b, Op::Subtract);
        let v1 = volume(&cut);
        assert!(v1 < v0 - 50_000.0 && v1 > v0 - 200_000.0, "{v0} {v1}");
        let joined = boolean(&a, &b, Op::Union);
        assert!(volume(&joined) > v0 + 50_000.0, "{}", volume(&joined));
        let common = boolean(&a, &b, Op::Intersect);
        assert!((volume(&common) - (v0 - v1)).abs() < 20_000.0, "{} {}", volume(&common), v0 - v1);
    }
}
