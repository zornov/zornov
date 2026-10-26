// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Picking faces, edges and vertices of a polygon mesh in words a person
//! or an AI can say: "the faces looking up", "inside this box", "face set
//! 2", "the edge loop nearest this point", "edges sharper than 40°".

use crate::poly::{Edge, Poly, Topology, edge_slot, ekey, other_face};
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, dot, length, lerp, normalise, sub};

/// Which faces.
#[derive(Clone, Debug, PartialEq)]
pub enum Faces {
    All,
    None,
    /// By number.
    List(Vec<u32>),
    /// Whose normal is within `degrees` of `direction`.
    Facing { direction: V3, degrees: f32 },
    /// Whose centre is within `radius` of `centre`.
    Sphere { centre: V3, radius: f32 },
    /// Whose centre is in the box.
    Box { min: V3, max: V3 },
    /// Whose centre is above (or below) `at` on `axis`.
    Side { axis: usize, at: f32, above: bool },
    Material(u32),
    FaceSet(u32),
    /// The face nearest a point.
    Nearest(V3),
    /// Faces joined (by edges) to any of these.
    Linked(Box<Faces>),
    /// These, grown by `rings` rings of neighbours (negative shrinks).
    Grow(Box<Faces>, i32),
    Not(Box<Faces>),
    And(Vec<Faces>),
    Or(Vec<Faces>),
}

impl Faces {
    /// The words "top", "bottom", "left", "right", "front", "back", "all".
    pub fn named(s: &str) -> Option<Faces> {
        let facing = |d: V3| Some(Faces::Facing { direction: d, degrees: 45.0 });
        match s {
            "" | "all" | "everything" => Some(Faces::All),
            "none" => Some(Faces::None),
            "top" | "up" => facing([0.0, -1.0, 0.0]),
            "bottom" | "down" => facing([0.0, 1.0, 0.0]),
            "left" => facing([-1.0, 0.0, 0.0]),
            "right" => facing([1.0, 0.0, 0.0]),
            "front" => facing([0.0, 0.0, -1.0]),
            "back" => facing([0.0, 0.0, 1.0]),
            _ => None,
        }
    }

    /// One yes/no per face.
    pub fn mask(&self, p: &Poly) -> Vec<bool> {
        let n = p.faces();
        match self {
            Faces::All => vec![true; n],
            Faces::None => vec![false; n],
            Faces::List(l) => {
                let mut m = vec![false; n];
                for &f in l {
                    if (f as usize) < n {
                        m[f as usize] = true;
                    }
                }
                m
            }
            Faces::Facing { direction, degrees } => {
                let d = normalise(*direction);
                let c = degrees.clamp(0.0, 180.0).to_radians().cos() - 1e-5;
                (0..n).map(|f| dot(p.face_normal(f), d) >= c).collect()
            }
            Faces::Sphere { centre, radius } => (0..n).map(|f| length(sub(p.face_centre(f), *centre)) <= *radius).collect(),
            Faces::Box { min, max } => (0..n)
                .map(|f| {
                    let c = p.face_centre(f);
                    (0..3).all(|i| c[i] >= min[i].min(max[i]) && c[i] <= max[i].max(min[i]))
                })
                .collect(),
            Faces::Side { axis, at, above } => (0..n)
                .map(|f| {
                    let v = p.face_centre(f)[(*axis).min(2)];
                    if *above { v >= *at } else { v <= *at }
                })
                .collect(),
            Faces::Material(m) => p.face_material.iter().map(|x| x == m).collect(),
            Faces::FaceSet(s) => p.face_set.iter().map(|x| x == s).collect(),
            Faces::Nearest(at) => {
                let mut m = vec![false; n];
                if let Some(best) = (0..n).min_by(|&a, &b| length(sub(p.face_centre(a), *at)).total_cmp(&length(sub(p.face_centre(b), *at)))) {
                    m[best] = true;
                }
                m
            }
            Faces::Linked(inner) => linked(p, &inner.mask(p)),
            Faces::Grow(inner, rings) => grow(p, inner.mask(p), *rings),
            Faces::Not(inner) => inner.mask(p).into_iter().map(|b| !b).collect(),
            Faces::And(list) => {
                let mut m = vec![true; n];
                for s in list {
                    for (a, b) in m.iter_mut().zip(s.mask(p)) {
                        *a &= b;
                    }
                }
                m
            }
            Faces::Or(list) => {
                let mut m = vec![false; n];
                for s in list {
                    for (a, b) in m.iter_mut().zip(s.mask(p)) {
                        *a |= b;
                    }
                }
                m
            }
        }
    }

    pub fn list(&self, p: &Poly) -> Vec<u32> {
        self.mask(p).iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i as u32).collect()
    }
}

/// Every face connected to a picked one.
pub fn linked(p: &Poly, seed: &[bool]) -> Vec<bool> {
    let t = Topology::of(p);
    let mut out = seed.to_vec();
    let mut stack: Vec<usize> = (0..seed.len()).filter(|&f| seed[f]).collect();
    while let Some(f) = stack.pop() {
        for &v in p.face(f) {
            for &g in &t.vertex_faces[v as usize] {
                if !out[g as usize] {
                    out[g as usize] = true;
                    stack.push(g as usize);
                }
            }
        }
    }
    out
}

/// A face pick grown by rings of neighbours sharing a vertex (or shrunk).
pub fn grow(p: &Poly, mut m: Vec<bool>, rings: i32) -> Vec<bool> {
    let t = Topology::of(p);
    for _ in 0..rings.unsigned_abs() {
        let mut next = m.clone();
        for f in 0..p.faces() {
            let touches = |want: bool| p.face(f).iter().any(|&v| t.vertex_faces[v as usize].iter().any(|&g| m[g as usize] == want));
            if rings > 0 && !m[f] && touches(true) {
                next[f] = true;
            }
            if rings < 0 && m[f] && touches(false) {
                next[f] = false;
            }
        }
        m = next;
    }
    m
}

/// Which edges.
#[derive(Clone, Debug, PartialEq)]
pub enum Edges {
    All,
    List(Vec<Edge>),
    /// Where the faces meet at more than `degrees` (the creases a hard
    /// surface shows).
    Sharper { degrees: f32 },
    /// Open edges (one face).
    Boundary,
    /// The rim of a face pick (edges between picked and other faces, or
    /// open edges of picked faces).
    Outline(Faces),
    /// Edges with both ends in the face pick.
    Inside(Faces),
    /// The loop running through the edge nearest a point.
    Loop(V3),
    /// The ring of edges across the quads, through the edge nearest a
    /// point.
    Ring(V3),
    /// The edge nearest a point.
    Nearest(V3),
    /// Edges marked sharp, creased, or as seams.
    MarkedSharp,
    Creased,
    Seams,
    Or(Vec<Edges>),
}

impl Edges {
    pub fn list(&self, p: &Poly) -> Vec<Edge> {
        let t = Topology::of(p);
        let mut out: Vec<Edge> = match self {
            Edges::All => p.edges(),
            Edges::List(l) => l.iter().map(|e| ekey(e.0, e.1)).filter(|e| t.edge_faces.contains_key(e)).collect(),
            Edges::Sharper { degrees } => {
                let c = degrees.to_radians().cos();
                t.edge_faces.iter().filter(|(_, f)| f.len() == 2 && dot(p.face_normal(f[0] as usize), p.face_normal(f[1] as usize)) < c).map(|(e, _)| *e).collect()
            }
            Edges::Boundary => t.boundary_edges(),
            Edges::Outline(faces) => {
                let m = faces.mask(p);
                t.edge_faces.iter().filter(|(_, f)| {
                    let picked = f.iter().filter(|&&g| m[g as usize]).count();
                    picked >= 1 && picked < f.len() || (f.len() == 1 && picked == 1)
                }).map(|(e, _)| *e).collect()
            }
            Edges::Inside(faces) => {
                let m = faces.mask(p);
                let mut vs = HashSet::new();
                for f in 0..p.faces() {
                    if m[f] {
                        vs.extend(p.face(f).iter().copied());
                    }
                }
                t.edge_faces.keys().filter(|e| vs.contains(&e.0) && vs.contains(&e.1)).copied().collect()
            }
            Edges::Nearest(at) => nearest_edge(p, *at).into_iter().collect(),
            Edges::Loop(at) => nearest_edge(p, *at).map(|e| edge_loop(p, &t, e)).unwrap_or_default(),
            Edges::Ring(at) => nearest_edge(p, *at).map(|e| edge_ring(p, &t, e).into_iter().map(|(e, _)| e).collect()).unwrap_or_default(),
            Edges::MarkedSharp => p.sharp.iter().copied().collect(),
            Edges::Creased => p.creases.iter().filter(|(_, w)| **w > 0.0).map(|(e, _)| *e).collect(),
            Edges::Seams => p.seams.iter().copied().collect(),
            Edges::Or(list) => list.iter().flat_map(|s| s.list(p)).collect(),
        };
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Distance from a point to a segment.
pub fn segment_distance(p: V3, a: V3, b: V3) -> f32 {
    let ab = sub(b, a);
    let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-12)).clamp(0.0, 1.0);
    length(sub(p, lerp(a, b, t)))
}

pub fn nearest_edge(p: &Poly, at: V3) -> Option<Edge> {
    p.edges().into_iter().min_by(|a, b| {
        let da = segment_distance(at, p.positions[a.0 as usize], p.positions[a.1 as usize]);
        let db = segment_distance(at, p.positions[b.0 as usize], p.positions[b.1 as usize]);
        da.total_cmp(&db)
    })
}

/// The edges of each vertex.
fn vertex_edges(t: &Topology) -> HashMap<u32, Vec<Edge>> {
    let mut m: HashMap<u32, Vec<Edge>> = HashMap::new();
    for e in t.edge_faces.keys() {
        m.entry(e.0).or_default().push(*e);
        m.entry(e.1).or_default().push(*e);
    }
    m
}

/// The edge loop through `start`: across every four-edge vertex it carries
/// on along the edge that shares no face with the one it came in on; on
/// an open rim it follows the rim.
pub fn edge_loop(p: &Poly, t: &Topology, start: Edge) -> Vec<Edge> {
    let ve = vertex_edges(t);
    let boundary = t.is_boundary(start);
    let mut out = vec![start];
    let mut seen: HashSet<Edge> = HashSet::from([start]);
    for (mut from, mut at) in [(start.0, start.1), (start.1, start.0)] {
        loop {
            let cur = ekey(from, at);
            let Some(es) = ve.get(&at) else { break };
            let next = if boundary {
                es.iter().copied().filter(|e| *e != cur && t.is_boundary(*e)).collect::<Vec<_>>()
            } else {
                if es.len() != 4 || es.iter().any(|e| t.is_boundary(*e)) {
                    break;
                }
                let faces_of = |e: &Edge| t.edge_faces.get(e).cloned().unwrap_or_default();
                let mine = faces_of(&cur);
                es.iter().copied().filter(|e| *e != cur && !faces_of(e).iter().any(|f| mine.contains(f))).collect()
            };
            let [n] = next[..] else { break };
            if !seen.insert(n) {
                break;
            }
            out.push(n);
            let other = if n.0 == at { n.1 } else { n.0 };
            from = at;
            at = other;
        }
    }
    let _ = p;
    out
}

/// The ring of edges across quads from `start`: each with the quad it was
/// reached through (and the first with the quad it leaves into).
pub fn edge_ring(p: &Poly, t: &Topology, start: Edge) -> Vec<(Edge, Option<u32>)> {
    let mut out = vec![(start, None)];
    let mut seen: HashSet<Edge> = HashSet::from([start]);
    let faces = t.edge_faces.get(&start).cloned().unwrap_or_default();
    for (side, &first) in faces.iter().enumerate() {
        let mut f = first;
        let mut e = start;
        loop {
            let c = p.face(f as usize);
            if c.len() != 4 {
                break;
            }
            let Some(s) = edge_slot(c, e) else { break };
            let opp = ekey(c[(s + 2) % 4], c[(s + 3) % 4]);
            if !seen.insert(opp) {
                break;
            }
            if side == 0 {
                out.push((opp, Some(f)));
            } else {
                out.insert(0, (opp, Some(f)));
            }
            match other_face(t, opp, f) {
                Some(g) => {
                    f = g;
                    e = opp;
                }
                None => break,
            }
        }
    }
    out
}

/// Which vertices: those used by picked faces.
pub fn face_vertices(p: &Poly, m: &[bool]) -> Vec<bool> {
    let mut v = vec![false; p.positions.len()];
    for f in 0..p.faces() {
        if m[f] {
            for &i in p.face(f) {
                v[i as usize] = true;
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn loops_and_rings_go_round_a_cylinder() {
        let c = prim::cylinder(10.0, 10.0, 20.0, 12, 4, prim::Cap::Ngon);
        let t = Topology::of(&c);
        // A horizontal edge in the middle ring: its loop goes all round.
        let at = [10.0, 0.0, 0.0];
        let ring_edge = nearest_edge(&c, at).unwrap();
        let lp = edge_loop(&c, &t, ring_edge);
        let ring = edge_ring(&c, &t, ring_edge);
        let (lp_n, ring_n) = (lp.len(), ring.len());
        // One is the 12 around, the other the 4 up the side (+1 end).
        assert!(lp_n == 12 || ring_n == 12, "loop {lp_n} ring {ring_n}");
    }

    #[test]
    fn faces_by_words() {
        let c = prim::cube([10.0; 3], 2);
        assert_eq!(Faces::named("top").unwrap().list(&c).len(), 4);
        assert_eq!(Edges::Sharper { degrees: 30.0 }.list(&c).len(), 24);
        let g = Faces::Grow(Box::new(Faces::Nearest([0.0, -5.0, 0.0])), 1).list(&c);
        assert_eq!(g.len(), 8);
    }
}
