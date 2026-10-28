// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where two shapes touch: contact manifolds (a shared normal and up to
//! four points with their separations).
//!
//! Convex pairs use the separating axis test: the face normals of both
//! and the cross products of edge pairs that form a face of their
//! Minkowski difference (the Gauss map test, so only real edge-edge
//! contacts count). A face contact clips the most opposed face of one
//! against the side planes of the other's face, which gives the corners a
//! box rests on. Points closer than `margin` but not yet touching are kept
//! too (speculative contacts), so the solver stops fast things before they
//! pass through instead of after.

use super::hull::Hull;
use super::math::*;
use super::shape::{Shape, TriMesh};

/// A point where two shapes touch, in the world: midway between the two
/// surfaces; `sep` < 0 is overlap.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub at: V3,
    pub sep: f32,
}

/// Contacts sharing a normal, which points from the first shape to the
/// second.
#[derive(Clone, Debug)]
pub struct Manifold {
    pub normal: V3,
    pub points: Vec<Point>,
}

/// A convex polyhedron in the world, for the separating axis test.
struct Poly<'a> {
    verts: Vec<V3>,
    planes: Vec<(V3, f32)>,
    loops: Loops<'a>,
    /// Corners and the normals of the two faces (arcs on the Gauss map).
    edges: Vec<(u32, u32, V3, V3)>,
}

enum Loops<'a> {
    Hull(&'a [super::hull::Face]),
    Tri,
}

const TRI_FRONT: [u32; 3] = [0, 1, 2];
const TRI_BACK: [u32; 3] = [0, 2, 1];

impl<'a> Poly<'a> {
    fn of_hull(h: &'a Hull, pose: &Pose) -> Poly<'a> {
        let verts: Vec<V3> = h.verts.iter().map(|v| pose.apply(*v)).collect();
        let planes: Vec<(V3, f32)> = h
            .faces
            .iter()
            .map(|f| {
                let n = pose.rotate(f.n);
                (n, f.d + dot(n, pose.p))
            })
            .collect();
        let edges = h.edges.iter().map(|e| (e.a, e.b, planes[e.f0 as usize].0, planes[e.f1 as usize].0)).collect();
        Poly { verts, planes, loops: Loops::Hull(&h.faces), edges }
    }

    /// A two-sided triangle: two faces, and each edge's Gauss arc split in
    /// two at the edge's outward direction (front to side to back).
    fn of_tri(t: [V3; 3]) -> Poly<'static> {
        let n = normalise_or(cross(sub(t[1], t[0]), sub(t[2], t[0])), [0.0, -1.0, 0.0]);
        let d = dot(n, t[0]);
        let mut edges = Vec::with_capacity(6);
        for i in 0..3 {
            let (a, b) = (i as u32, ((i + 1) % 3) as u32);
            let side = normalise_or(cross(sub(t[(i + 1) % 3], t[i]), n), [1.0, 0.0, 0.0]);
            edges.push((a, b, n, side));
            edges.push((a, b, side, neg(n)));
        }
        Poly { verts: t.to_vec(), planes: vec![(n, d), (neg(n), -d)], loops: Loops::Tri, edges }
    }

    fn face_loop(&self, f: usize) -> &[u32] {
        match &self.loops {
            Loops::Hull(faces) => &faces[f].verts,
            Loops::Tri => {
                if f == 0 {
                    &TRI_FRONT
                } else {
                    &TRI_BACK
                }
            }
        }
    }

    fn support(&self, d: V3) -> f32 {
        self.verts.iter().map(|v| dot(*v, d)).fold(f32::NEG_INFINITY, f32::max)
    }

    fn lowest(&self, d: V3) -> f32 {
        self.verts.iter().map(|v| dot(*v, d)).fold(f32::INFINITY, f32::min)
    }
}

/// Every manifold between `a` at `pa` and `b` at `pb` (normals from a to
/// b), counting points up to `margin` apart.
pub fn collide(a: &Shape, pa: &Pose, b: &Shape, pb: &Pose, margin: f32, out: &mut Vec<Manifold>) {
    let rank = |s: &Shape| match s {
        Shape::Sphere { .. } => 0,
        Shape::Capsule { .. } => 1,
        Shape::Hull(_) => 2,
        Shape::Mesh(_) => 3,
        Shape::Plane { .. } => 4,
    };
    if rank(a) > rank(b) {
        let first = out.len();
        collide(b, pb, a, pa, margin, out);
        for m in &mut out[first..] {
            m.normal = neg(m.normal);
        }
        return;
    }
    match (a, b) {
        (Shape::Sphere { r: ra }, Shape::Sphere { r: rb }) => push(out, ball_ball(pa.p, *ra, pb.p, *rb, margin)),
        (Shape::Sphere { r }, Shape::Capsule { half, r: rc }) => {
            let (p0, p1) = ends(pb, *half);
            let q = closest_on_segment(p0, p1, pa.p);
            push(out, ball_ball(pa.p, *r, q, *rc, margin));
        }
        (Shape::Sphere { r }, Shape::Hull(h)) => push(out, ball_poly(pa.p, *r, &Poly::of_hull(h, pb), margin)),
        (Shape::Sphere { r }, Shape::Mesh(m)) => against_mesh(m, pb, &a.bounds(pa), margin, out, |poly| ball_poly(pa.p, *r, poly, margin)),
        (Shape::Sphere { r }, Shape::Plane { n, d }) => {
            let (n, d) = plane_world(pb, *n, *d);
            let sep = dot(n, pa.p) - d - r;
            if sep <= margin {
                push(out, Some(Manifold { normal: neg(n), points: vec![Point { at: mad(pa.p, n, -(r + sep * 0.5)), sep }] }));
            }
        }
        (Shape::Capsule { half: ha, r: ra }, Shape::Capsule { half: hb, r: rb }) => push(out, capsule_capsule(ends(pa, *ha), *ra, ends(pb, *hb), *rb, margin)),
        (Shape::Capsule { half, r }, Shape::Hull(h)) => push(out, capsule_poly(ends(pa, *half), *r, &Poly::of_hull(h, pb), margin)),
        (Shape::Capsule { half, r }, Shape::Mesh(m)) => against_mesh(m, pb, &a.bounds(pa), margin, out, |poly| capsule_poly(ends(pa, *half), *r, poly, margin)),
        (Shape::Capsule { half, r }, Shape::Plane { n, d }) => {
            let (n, d) = plane_world(pb, *n, *d);
            let (p0, p1) = ends(pa, *half);
            let pts: Vec<Point> = [p0, p1].iter().filter_map(|p| {
                let sep = dot(n, *p) - d - r;
                (sep <= margin).then(|| Point { at: mad(*p, n, -(r + sep * 0.5)), sep })
            }).collect();
            if !pts.is_empty() {
                push(out, Some(Manifold { normal: neg(n), points: pts }));
            }
        }
        (Shape::Hull(ha), Shape::Hull(hb)) => push(out, poly_poly(&Poly::of_hull(ha, pa), &Poly::of_hull(hb, pb), margin)),
        (Shape::Hull(h), Shape::Mesh(m)) => {
            let poly = Poly::of_hull(h, pa);
            against_mesh(m, pb, &a.bounds(pa), margin, out, |tri| poly_poly(&poly, tri, margin));
        }
        (Shape::Hull(h), Shape::Plane { n, d }) => {
            let (n, d) = plane_world(pb, *n, *d);
            let pts: Vec<Point> = h.verts.iter().filter_map(|v| {
                let p = pa.apply(*v);
                let sep = dot(n, p) - d;
                (sep <= margin).then(|| Point { at: mad(p, n, -sep * 0.5), sep })
            }).collect();
            if !pts.is_empty() {
                push(out, Some(Manifold { normal: neg(n), points: reduce(pts, neg(n)) }));
            }
        }
        _ => {}
    }
}

fn push(out: &mut Vec<Manifold>, m: Option<Manifold>) {
    if let Some(m) = m.filter(|m| !m.points.is_empty()) {
        out.push(m);
    }
}

fn ends(pose: &Pose, half: f32) -> (V3, V3) {
    (pose.apply([0.0, half, 0.0]), pose.apply([0.0, -half, 0.0]))
}

fn plane_world(pose: &Pose, n: V3, d: f32) -> (V3, f32) {
    let nw = pose.rotate(n);
    (nw, d + dot(nw, pose.p))
}

fn ball_ball(ca: V3, ra: f32, cb: V3, rb: f32, margin: f32) -> Option<Manifold> {
    let d = sub(cb, ca);
    let l = length(d);
    let sep = l - ra - rb;
    if sep > margin {
        return None;
    }
    let n = normalise_or(d, [0.0, 1.0, 0.0]);
    Some(Manifold { normal: n, points: vec![Point { at: mad(ca, n, ra + sep * 0.5), sep }] })
}

fn capsule_capsule((a0, a1): (V3, V3), ra: f32, (b0, b1): (V3, V3), rb: f32, margin: f32) -> Option<Manifold> {
    let da = sub(a1, a0);
    let db = sub(b1, b0);
    let la = length(da);
    let lb = length(db);
    // Lying side by side: two points over the shared stretch.
    if la > 1e-6 && lb > 1e-6 {
        let ua = scale(da, 1.0 / la);
        let ub = scale(db, 1.0 / lb);
        if dot(ua, ub).abs() > 0.995 {
            let t0 = dot(sub(b0, a0), ua).clamp(0.0, la);
            let t1 = dot(sub(b1, a0), ua).clamp(0.0, la);
            if (t1 - t0).abs() > la * 0.05 {
                let mut pts = Vec::new();
                let mut normal = [0.0; 3];
                for t in [t0, t1] {
                    let pa = mad(a0, ua, t);
                    let pb = closest_on_segment(b0, b1, pa);
                    if let Some(m) = ball_ball(pa, ra, pb, rb, margin) {
                        normal = add(normal, m.normal);
                        pts.extend(m.points);
                    }
                }
                if !pts.is_empty() {
                    return Some(Manifold { normal: normalise_or(normal, [0.0, 1.0, 0.0]), points: pts });
                }
            }
        }
    }
    let (s, t) = segment_segment(a0, a1, b0, b1);
    ball_ball(mad(a0, da, s), ra, mad(b0, db, t), rb, margin)
}

/// The nearest point of a convex polygon (`loop_` corners, plane `n`) to `p`.
fn closest_on_face(poly: &Poly, f: usize, p: V3) -> V3 {
    let (n, d) = poly.planes[f];
    let lp = poly.face_loop(f);
    let proj = mad(p, n, -(dot(n, p) - d));
    let mut inside = true;
    for i in 0..lp.len() {
        let a = poly.verts[lp[i] as usize];
        let b = poly.verts[lp[(i + 1) % lp.len()] as usize];
        if dot(cross(sub(b, a), n), sub(proj, a)) > 0.0 {
            inside = false;
            break;
        }
    }
    if inside {
        return proj;
    }
    let mut best = proj;
    let mut bd = f32::INFINITY;
    for i in 0..lp.len() {
        let a = poly.verts[lp[i] as usize];
        let b = poly.verts[lp[(i + 1) % lp.len()] as usize];
        let q = closest_on_segment(a, b, p);
        let dd = len2(sub(q, p));
        if dd < bd {
            bd = dd;
            best = q;
        }
    }
    best
}

fn ball_poly(c: V3, r: f32, poly: &Poly, margin: f32) -> Option<Manifold> {
    let (mut fbest, mut sbest) = (0usize, f32::NEG_INFINITY);
    for (i, (n, d)) in poly.planes.iter().enumerate() {
        let s = dot(*n, c) - d;
        if s > sbest {
            sbest = s;
            fbest = i;
        }
    }
    if sbest - r > margin {
        return None;
    }
    if sbest <= 0.0 {
        // The centre is inside: out through the nearest face.
        let n = poly.planes[fbest].0;
        let sep = sbest - r;
        return Some(Manifold { normal: neg(n), points: vec![Point { at: mad(c, n, -(r + sep * 0.5)), sep }] });
    }
    let mut best: Option<(V3, f32)> = None;
    for (i, (n, d)) in poly.planes.iter().enumerate() {
        if dot(*n, c) - d <= 0.0 {
            continue;
        }
        let q = closest_on_face(poly, i, c);
        let dist = length(sub(q, c));
        if best.is_none_or(|b| dist < b.1) {
            best = Some((q, dist));
        }
    }
    let (q, dist) = best?;
    let sep = dist - r;
    if sep > margin {
        return None;
    }
    let normal = normalise_or(sub(q, c), neg(poly.planes[fbest].0));
    Some(Manifold { normal, points: vec![Point { at: mad(c, normal, r + sep * 0.5), sep }] })
}

fn capsule_poly((p0, p1): (V3, V3), r: f32, poly: &Poly, margin: f32) -> Option<Manifold> {
    let seg = sub(p1, p0);
    let slen = length(seg);
    if slen < 1e-7 {
        return ball_poly(p0, r, poly, margin);
    }
    let u = scale(seg, 1.0 / slen);
    // Faces of the solid.
    let (mut fbest, mut fsep) = (0usize, f32::NEG_INFINITY);
    for (i, (n, d)) in poly.planes.iter().enumerate() {
        let s = dot(*n, p0).min(dot(*n, p1)) - d - r;
        if s > fsep {
            fsep = s;
            fbest = i;
        }
    }
    if fsep > margin {
        return None;
    }
    // The stick across its edges.
    let (mut ebest, mut esep, mut eaxis) = (usize::MAX, f32::NEG_INFINITY, [0.0; 3]);
    for (k, e) in poly.edges.iter().enumerate() {
        let ed = sub(poly.verts[e.1 as usize], poly.verts[e.0 as usize]);
        let ax = cross(u, ed);
        let l = length(ax);
        if l < 1e-6 * length(ed).max(1e-9) {
            continue;
        }
        let ax = scale(ax, 1.0 / l);
        // Either side: the axis points from the solid to the stick.
        let pos = dot(ax, p0).min(dot(ax, p1)) - poly.support(ax);
        let negs = poly.lowest(ax) - dot(ax, p0).max(dot(ax, p1));
        let (ax, s) = if pos >= negs { (ax, pos - r) } else { (neg(ax), negs - r) };
        if s > esep {
            esep = s;
            ebest = k;
            eaxis = ax;
        }
    }
    if esep > margin {
        return None;
    }
    let tol = 0.02 * r + 1e-4;
    if ebest == usize::MAX || fsep >= esep - tol {
        let (n, d) = poly.planes[fbest];
        // The stick clipped to the face's outline.
        let lp = poly.face_loop(fbest);
        let (mut a, mut b) = (p0, p1);
        let mut kept = true;
        for i in 0..lp.len() {
            let v0 = poly.verts[lp[i] as usize];
            let v1 = poly.verts[lp[(i + 1) % lp.len()] as usize];
            let side = normalise_or(cross(sub(v1, v0), n), [0.0; 3]);
            let off = dot(side, v0);
            let (da, db) = (dot(side, a) - off, dot(side, b) - off);
            if da > 0.0 && db > 0.0 {
                kept = false;
                break;
            }
            if da > 0.0 {
                a = mad(a, sub(b, a), da / (da - db));
            } else if db > 0.0 {
                b = mad(a, sub(b, a), da / (da - db));
            }
        }
        let mut pts = Vec::new();
        if kept {
            for p in [a, b] {
                let sep = dot(n, p) - d - r;
                if sep <= margin {
                    pts.push(Point { at: mad(p, n, -(r + sep * 0.5)), sep });
                }
            }
            if length(sub(a, b)) < 1e-6 {
                pts.truncate(1);
            }
        }
        if !pts.is_empty() {
            return Some(Manifold { normal: neg(n), points: pts });
        }
        // Over the face's plane but beside it: the nearest corner or edge.
        let q = closest_on_face(poly, fbest, closest_on_segment(p0, p1, poly.verts[lp[0] as usize]));
        let c = closest_on_segment(p0, p1, q);
        return ball_poly(c, r, poly, margin);
    }
    let e = poly.edges[ebest];
    let (e0, e1) = (poly.verts[e.0 as usize], poly.verts[e.1 as usize]);
    let (s, t) = segment_segment(p0, p1, e0, e1);
    let c1 = mad(p0, seg, s);
    let _c2 = mad(e0, sub(e1, e0), t);
    let normal = neg(eaxis);
    Some(Manifold { normal, points: vec![Point { at: mad(c1, normal, r + esep * 0.5), sep: esep }] })
}

/// Gregorius' test: do arcs (a, b) and (c, d) on the unit sphere cross?
fn arcs_cross(a: V3, b: V3, c: V3, d: V3) -> bool {
    let bxa = cross(b, a);
    let dxc = cross(d, c);
    let cba = dot(c, bxa);
    let dba = dot(d, bxa);
    let adc = dot(a, dxc);
    let bdc = dot(b, dxc);
    cba * dba < 0.0 && adc * bdc < 0.0 && cba * bdc > 0.0
}

fn face_query(a: &Poly, b: &Poly) -> (usize, f32) {
    let mut best = (0usize, f32::NEG_INFINITY);
    for (i, (n, d)) in a.planes.iter().enumerate() {
        let s = b.lowest(*n) - d;
        if s > best.1 {
            best = (i, s);
        }
    }
    best
}

fn poly_poly(a: &Poly, b: &Poly, margin: f32) -> Option<Manifold> {
    let (fa, sa) = face_query(a, b);
    if sa > margin {
        return None;
    }
    let (fb, sb) = face_query(b, a);
    if sb > margin {
        return None;
    }
    // Edge pairs that make a face of the Minkowski difference.
    let ca = scale(a.verts.iter().fold([0.0; 3], |s, v| add(s, *v)), 1.0 / a.verts.len() as f32);
    let (mut ebest, mut esep, mut eaxis) = (None, f32::NEG_INFINITY, [0.0; 3]);
    for (i, ea) in a.edges.iter().enumerate() {
        let da = sub(a.verts[ea.1 as usize], a.verts[ea.0 as usize]);
        for (j, eb) in b.edges.iter().enumerate() {
            if !arcs_cross(ea.2, ea.3, neg(eb.2), neg(eb.3)) {
                continue;
            }
            let db = sub(b.verts[eb.1 as usize], b.verts[eb.0 as usize]);
            let ax = cross(da, db);
            let l = length(ax);
            if l < 1e-6 * length(da) * length(db) {
                continue;
            }
            let mut ax = scale(ax, 1.0 / l);
            let pa = a.verts[ea.0 as usize];
            // Out of a at its edge.
            if dot(ax, add(ea.2, ea.3)) < 0.0 {
                ax = neg(ax);
            }
            let s = dot(ax, sub(b.verts[eb.0 as usize], pa));
            if s > margin {
                return None;
            }
            if s > esep {
                esep = s;
                ebest = Some((i, j));
                eaxis = ax;
            }
        }
    }
    let scale_len = a.verts.iter().map(|v| length(sub(*v, ca))).fold(0.0f32, f32::max);
    let tol = 0.005 * scale_len + 1e-5;
    let face_sep = sa.max(sb);
    if let Some((i, j)) = ebest
        && esep > face_sep + tol
    {
        let ea = a.edges[i];
        let eb = b.edges[j];
        let (a0, a1) = (a.verts[ea.0 as usize], a.verts[ea.1 as usize]);
        let (b0, b1) = (b.verts[eb.0 as usize], b.verts[eb.1 as usize]);
        let (s, t) = segment_segment(a0, a1, b0, b1);
        let pa = mad(a0, sub(a1, a0), s);
        let pb = mad(b0, sub(b1, b0), t);
        return Some(Manifold { normal: eaxis, points: vec![Point { at: scale(add(pa, pb), 0.5), sep: esep }] });
    }
    // Face contact: the reference face is the one that separates most
    // (a, unless b's is clearly better).
    let (ref_poly, rf, inc_poly, flip) = if sb > sa + tol * 0.5 { (b, fb, a, true) } else { (a, fa, b, false) };
    let (n, d) = ref_poly.planes[rf];
    let inc = (0..inc_poly.planes.len()).min_by(|&x, &y| dot(inc_poly.planes[x].0, n).total_cmp(&dot(inc_poly.planes[y].0, n))).unwrap();
    let mut pts: Vec<V3> = inc_poly.face_loop(inc).iter().map(|&v| inc_poly.verts[v as usize]).collect();
    let lp = ref_poly.face_loop(rf);
    for i in 0..lp.len() {
        let v0 = ref_poly.verts[lp[i] as usize];
        let v1 = ref_poly.verts[lp[(i + 1) % lp.len()] as usize];
        let side = normalise_or(cross(sub(v1, v0), n), [0.0; 3]);
        pts = clip(&pts, side, dot(side, v0));
        if pts.is_empty() {
            break;
        }
    }
    let normal = if flip { neg(n) } else { n };
    let points: Vec<Point> = pts
        .into_iter()
        .filter_map(|p| {
            let sep = dot(n, p) - d;
            (sep <= margin).then(|| Point { at: mad(p, n, -sep * 0.5), sep })
        })
        .collect();
    if points.is_empty() {
        // The faces miss each other: the closest pair of edges, if near.
        if let Some((i, j)) = ebest
            && esep <= margin
        {
            let ea = a.edges[i];
            let eb = b.edges[j];
            let (a0, a1) = (a.verts[ea.0 as usize], a.verts[ea.1 as usize]);
            let (b0, b1) = (b.verts[eb.0 as usize], b.verts[eb.1 as usize]);
            let (s, t) = segment_segment(a0, a1, b0, b1);
            let pa = mad(a0, sub(a1, a0), s);
            let pb = mad(b0, sub(b1, b0), t);
            // Apart along a face axis, so never nearer than that: an edge axis
            // that cuts through the other shape is no measure of the gap.
            return Some(Manifold { normal: eaxis, points: vec![Point { at: scale(add(pa, pb), 0.5), sep: esep.max(face_sep) }] });
        }
        return None;
    }
    Some(Manifold { normal, points: reduce(points, normal) })
}

/// Sutherland–Hodgman: the part of a polygon with `dot(n, p) <= off`.
fn clip(pts: &[V3], n: V3, off: f32) -> Vec<V3> {
    let mut out = Vec::with_capacity(pts.len() + 2);
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        let (da, db) = (dot(n, a) - off, dot(n, b) - off);
        if da <= 0.0 {
            out.push(a);
        }
        if (da <= 0.0) != (db <= 0.0) {
            out.push(mad(a, sub(b, a), da / (da - db)));
        }
    }
    out
}

/// At most four points: the deepest, the one furthest from it, then the
/// ones that span the most area (what a box needs to rest flat).
pub fn reduce(mut pts: Vec<Point>, normal: V3) -> Vec<Point> {
    if pts.len() <= 4 {
        return pts;
    }
    let deepest = (0..pts.len()).min_by(|&x, &y| pts[x].sep.total_cmp(&pts[y].sep)).unwrap();
    let p0 = pts[deepest];
    let far = (0..pts.len()).max_by(|&x, &y| len2(sub(pts[x].at, p0.at)).total_cmp(&len2(sub(pts[y].at, p0.at)))).unwrap();
    let p1 = pts[far];
    let area = |a: V3, b: V3, c: V3| dot(cross(sub(b, a), sub(c, a)), normal);
    let third = (0..pts.len()).max_by(|&x, &y| area(p0.at, p1.at, pts[x].at).abs().total_cmp(&area(p0.at, p1.at, pts[y].at).abs())).unwrap();
    let p2 = pts[third];
    let s = area(p0.at, p1.at, p2.at).signum();
    // The fourth: outside the triangle of the first three, spanning most.
    let gain = |q: V3| (-s * area(p0.at, p1.at, q)).max(-s * area(p1.at, p2.at, q)).max(-s * area(p2.at, p0.at, q));
    let fourth = (0..pts.len()).filter(|&k| k != deepest && k != far && k != third && gain(pts[k].at) > 0.0).max_by(|&x, &y| gain(pts[x].at).total_cmp(&gain(pts[y].at)));
    let mut out = vec![p0, p1, p2];
    if let Some(k) = fourth {
        out.push(pts[k]);
    }
    pts.clear();
    out
}

/// A shape against the triangles of a fixed surface: one manifold per
/// direction it is pushed (a floor, a wall), each of at most four points.
fn against_mesh(m: &TriMesh, pose: &Pose, world_bounds: &Aabb, margin: f32, out: &mut Vec<Manifold>, mut each: impl FnMut(&Poly) -> Option<Manifold>) {
    // The bounds in the mesh's frame.
    let wb = world_bounds.fattened(margin);
    let mut lb = Aabb::EMPTY;
    for i in 0..8 {
        lb.grow(pose.unapply([if i & 1 == 0 { wb.lo[0] } else { wb.hi[0] }, if i & 2 == 0 { wb.lo[1] } else { wb.hi[1] }, if i & 4 == 0 { wb.lo[2] } else { wb.hi[2] }]));
    }
    let mut hits = Vec::new();
    m.query(&lb, &mut hits);
    let mut found: Vec<Manifold> = Vec::new();
    // Points this close are one contact met twice (a shared edge). The
    // range is the margin's, but a wide margin (a look for what is far
    // below) must not fold a surface into the one behind it: a slab's top
    // and bottom faces are a hand's breadth apart, and the near one counts.
    let same = margin.min(0.05);
    // What each triangle touches, and whether it is the triangle's own
    // face (the contact runs along its normal) or only its edge or corner.
    let mut touched: Vec<(bool, Manifold)> = Vec::new();
    for t in hits {
        let tri = m.bvh.tris[t as usize].map(|v| pose.apply(v));
        if let Some(mf) = each(&Poly::of_tri(tri)) {
            let face = dot(mf.normal, pose.rotate(m.normals[t as usize])).abs() > 0.999;
            touched.push((face, mf));
        }
    }
    // An edge or corner touched while a face of the surface is as near is
    // the seam to the next triangle of the same flat (a ball rolling over a
    // floor's diagonal), not something to push against sideways.
    let nearest_face = touched.iter().filter(|(face, _)| *face).flat_map(|(_, mf)| mf.points.iter().map(|p| p.sep)).fold(f32::INFINITY, f32::min);
    for (face, mf) in touched {
        if !face && mf.points.iter().all(|p| p.sep >= nearest_face - 1e-5) {
            continue;
        }
        match found.iter_mut().find(|f| dot(f.normal, mf.normal) > 0.95) {
            Some(f) => {
                let wa = f.points.len() as f32;
                f.normal = normalise_or(add(scale(f.normal, wa), scale(mf.normal, mf.points.len() as f32)), f.normal);
                for p in mf.points {
                    if !f.points.iter().any(|q| len2(sub(q.at, p.at)) < same * same * 0.01 + 1e-12) {
                        f.points.push(p);
                    }
                }
            }
            None => found.push(mf),
        }
    }
    for mut f in found {
        let n = f.normal;
        f.points = reduce(f.points, n);
        out.push(f);
    }
}

/// How far `a` at `pa` can move along `dir` (a unit vector) before it
/// touches `b` at `pb`, if it does within `reach`. It steps in by what
/// each contact leaves between them (the separation over how much of the
/// normal runs along `dir`) until they touch; a contact beside or above
/// (its normal under a quarter along `dir`) does not stop it. Already in
/// `b`, the distance is negative: back out to touching. A fixed surface
/// is searched down its tree nearest first, only under `a`, and no
/// further than the nearest hit so far, so a big one costs little more
/// than the few triangles in the way.
pub fn sweep(a: &Shape, pa: &Pose, b: &Shape, pb: &Pose, dir: V3, reach: f32) -> Option<f32> {
    if a.is_static_only() {
        return None;
    }
    if let Shape::Mesh(m) = b {
        let poly = if let Shape::Hull(h) = a { Some(Poly::of_hull(h, pa)) } else { None };
        return sweep_mesh(m, pb, &a.bounds(pa), dir, reach, |tri, margin| against_tri(a, pa, poly.as_ref(), tri, margin));
    }
    let mut ms = Vec::new();
    advance(dir, reach, |t, margin| {
        ms.clear();
        collide(a, &Pose { p: mad(pa.p, dir, t), q: pa.q }, b, pb, margin, &mut ms);
        ms.pop()
    })
}

/// How deep `a` at `pa` is in `b` at `pb` (0 when apart or only
/// touching). A fixed surface stops counting once it is deeper than
/// `enough`.
pub fn overlap(a: &Shape, pa: &Pose, b: &Shape, pb: &Pose, enough: f32) -> f32 {
    let deepest = |m: &Manifold| m.points.iter().fold(0.0f32, |d, p| d.max(-p.sep));
    match b {
        Shape::Mesh(m) if !a.is_static_only() => {
            let poly = if let Shape::Hull(h) = a { Some(Poly::of_hull(h, pa)) } else { None };
            let mut hits = Vec::new();
            m.query(&local_box(&a.bounds(pa), pb), &mut hits);
            let mut most = 0.0f32;
            for t in hits {
                let tri = m.bvh.tris[t as usize].map(|v| pb.apply(v));
                if let Some(mf) = against_tri(a, pa, poly.as_ref(), &Poly::of_tri(tri), 0.0) {
                    most = most.max(deepest(&mf));
                    if most > enough {
                        break;
                    }
                }
            }
            most
        }
        _ => {
            let mut ms = Vec::new();
            collide(a, pa, b, pb, 0.0, &mut ms);
            ms.iter().map(deepest).fold(0.0, f32::max)
        }
    }
}

/// A convex shape against one triangle (normal from the shape).
fn against_tri(a: &Shape, pa: &Pose, poly: Option<&Poly>, tri: &Poly, margin: f32) -> Option<Manifold> {
    match a {
        Shape::Sphere { r } => ball_poly(pa.p, *r, tri, margin),
        Shape::Capsule { half, r } => capsule_poly(ends(pa, *half), *r, tri, margin),
        Shape::Hull(_) => poly_poly(poly?, tri, margin),
        _ => None,
    }
}

/// A world box in the frame of `pose` (the box around its corners).
fn local_box(b: &Aabb, pose: &Pose) -> Aabb {
    let mut lb = Aabb::EMPTY;
    for i in 0..8 {
        lb.grow(pose.unapply([if i & 1 == 0 { b.lo[0] } else { b.hi[0] }, if i & 2 == 0 { b.lo[1] } else { b.hi[1] }, if i & 4 == 0 { b.lo[2] } else { b.hi[2] }]));
    }
    lb
}

/// Steps toward a convex thing: `contact(t, margin)` is the manifold with
/// the moving shape `t` along `dir`, counting points up to `margin` apart.
fn advance(dir: V3, reach: f32, mut contact: impl FnMut(f32, f32) -> Option<Manifold>) -> Option<f32> {
    let mut t = 0.0f32;
    // Each step closes the gap a face contact leaves, so a few do; the
    // cap stops one that creeps along an edge.
    for _ in 0..16 {
        let mf = contact(t, (reach - t).max(0.0) + 0.001)?;
        let along = dot(mf.normal, dir);
        if along < 0.25 {
            return None;
        }
        let step = mf.points.iter().map(|p| p.sep / along).fold(f32::INFINITY, f32::min);
        if !step.is_finite() {
            return None;
        }
        if step.abs() < 1e-4 {
            break;
        }
        t += step;
        if t > reach {
            return None;
        }
    }
    Some(t)
}

/// A tree node (or triangle) to look at, the nearest along the sweep first.
struct Ahead(f32, u32);

impl PartialEq for Ahead {
    fn eq(&self, o: &Ahead) -> bool {
        self.0.total_cmp(&o.0).is_eq()
    }
}
impl Eq for Ahead {}
impl PartialOrd for Ahead {
    fn partial_cmp(&self, o: &Ahead) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Ahead {
    fn cmp(&self, o: &Ahead) -> std::cmp::Ordering {
        o.0.total_cmp(&self.0)
    }
}

/// [`sweep`] against the triangles of a fixed surface: `contact(tri,
/// margin)` is the moving shape against a triangle (in the world).
fn sweep_mesh(m: &TriMesh, pose: &Pose, part: &Aabb, dir: V3, reach: f32, mut contact: impl FnMut(&Poly, f32) -> Option<Manifold>) -> Option<f32> {
    if m.bvh.tris.is_empty() || part.is_empty() {
        return None;
    }
    // The column the shape sweeps through, in the mesh's frame.
    let swept = part.union(&Aabb { lo: mad(part.lo, dir, reach), hi: mad(part.hi, dir, reach) }).fattened(1e-3);
    let col = local_box(&swept, pose);
    let meets = |lo: &V3, hi: &V3| (0..3).all(|k| lo[k] <= col.hi[k] && col.lo[k] <= hi[k]);
    // How far along `dir` the shape reaches, and how far it must go to
    // meet a box: no triangle in the box is nearer.
    let front: f32 = (0..3).map(|k| if dir[k] > 0.0 { part.hi[k] * dir[k] } else { part.lo[k] * dir[k] }).sum();
    let ld = pose.unrotate(dir);
    let base = dot(dir, pose.p) - front;
    let ahead = |lo: &V3, hi: &V3| (0..3).map(|k| if ld[k] > 0.0 { lo[k] * ld[k] } else { hi[k] * ld[k] }).sum::<f32>() + base;
    let mut best: Option<f32> = None;
    let mut heap = std::collections::BinaryHeap::new();
    let root = &m.bvh.nodes[0];
    if meets(&root.lo, &root.hi) {
        heap.push(Ahead(ahead(&root.lo, &root.hi), 0));
    }
    while let Some(Ahead(d, i)) = heap.pop() {
        let limit = best.unwrap_or(reach);
        if d > limit {
            break;
        }
        let node = &m.bvh.nodes[i as usize];
        if node.count == 0 {
            for c in [node.index, i + 1] {
                let n = &m.bvh.nodes[c as usize];
                if meets(&n.lo, &n.hi) {
                    heap.push(Ahead(ahead(&n.lo, &n.hi), c));
                }
            }
            continue;
        }
        for k in node.index..node.index + node.count {
            let lt = m.bvh.tris[m.bvh.order[k as usize] as usize];
            let lo = vmin(lt[0], vmin(lt[1], lt[2]));
            let hi = vmax(lt[0], vmax(lt[1], lt[2]));
            let limit = best.unwrap_or(reach);
            if !meets(&lo, &hi) || ahead(&lo, &hi) > limit {
                continue;
            }
            let tri = lt.map(|v| pose.apply(v));
            // The triangle comes back along `dir` instead of the shape going on.
            if let Some(t) = advance(dir, limit, |t, margin| contact(&Poly::of_tri(tri.map(|v| mad(v, dir, -t))), margin))
                && best.is_none_or(|b| t < b)
            {
                best = Some(t);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn boxy(h: V3) -> Shape {
        Shape::cuboid(h)
    }

    #[test]
    fn a_box_on_a_box_touches_at_four_corners() {
        let a = boxy([0.5; 3]);
        let b = boxy([2.0, 0.5, 2.0]);
        let mut out = Vec::new();
        collide(&a, &Pose::at([0.0, -0.99, 0.0]), &b, &Pose::IDENTITY, 0.02, &mut out);
        assert_eq!(out.len(), 1);
        let m = &out[0];
        assert_eq!(m.points.len(), 4, "{m:?}");
        assert!(m.normal[1] > 0.99, "from the top box down into the lower one: {:?}", m.normal);
        for p in &m.points {
            assert!((p.sep + 0.01).abs() < 1e-3, "{p:?}");
        }
    }

    #[test]
    fn apart_beyond_the_margin_is_no_contact() {
        let mut out = Vec::new();
        collide(&boxy([0.5; 3]), &Pose::at([0.0, -1.2, 0.0]), &boxy([0.5; 3]), &Pose::IDENTITY, 0.1, &mut out);
        assert!(out.is_empty());
        collide(&boxy([0.5; 3]), &Pose::at([0.0, -1.05, 0.0]), &boxy([0.5; 3]), &Pose::IDENTITY, 0.1, &mut out);
        assert_eq!(out.len(), 1, "inside the margin: a speculative contact");
        assert!(out[0].points.iter().all(|p| p.sep > 0.0));
    }

    #[test]
    fn crossed_edges_meet_at_one_point() {
        // A box turned 45° about z resting its edge across another's
        // edge turned about x.
        let qa = studio_mesh::rig::q_axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_4);
        let qb = studio_mesh::rig::q_axis_angle([1.0, 0.0, 0.0], std::f32::consts::FRAC_PI_4);
        let r = 0.5 * 2f32.sqrt();
        let mut out = Vec::new();
        collide(&boxy([0.5; 3]), &Pose::new([0.0, -2.0 * r + 0.01, 0.0], qa), &boxy([0.5; 3]), &Pose::new([0.0; 3], qb), 0.05, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].points.len(), 1, "{:?}", out[0]);
        assert!(out[0].normal[1] > 0.99);
        assert!((out[0].points[0].sep + 0.01).abs() < 2e-3, "{:?}", out[0].points);
    }

    #[test]
    fn balls_capsules_and_planes() {
        let ground = Shape::Plane { n: [0.0, -1.0, 0.0], d: 0.0 };
        let mut out = Vec::new();
        collide(&Shape::Sphere { r: 1.0 }, &Pose::at([0.0, -0.9, 0.0]), &ground, &Pose::IDENTITY, 0.01, &mut out);
        assert!((out[0].points[0].sep + 0.1).abs() < 1e-4 && out[0].normal[1] > 0.99);
        out.clear();
        // A capsule lying on its side rests on two points.
        let lying = studio_mesh::rig::q_axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
        collide(&Shape::Capsule { half: 1.0, r: 0.5 }, &Pose::new([0.0, -0.49, 0.0], lying), &ground, &Pose::IDENTITY, 0.02, &mut out);
        assert_eq!(out[0].points.len(), 2);
        out.clear();
        collide(&Shape::Capsule { half: 1.0, r: 0.5 }, &Pose::new([0.0, -0.99, 0.0], lying), &boxy([3.0, 0.5, 3.0]), &Pose::IDENTITY, 0.02, &mut out);
        assert_eq!(out[0].points.len(), 2, "{out:?}");
        assert!(out[0].normal[1] > 0.99);
        out.clear();
        collide(&Shape::Sphere { r: 0.5 }, &Pose::at([1.2, -1.2, 0.0]), &boxy([1.0; 3]), &Pose::IDENTITY, 0.5, &mut out);
        // Near the corner edge: the normal points at the edge.
        let n = out[0].normal;
        assert!(n[0] < -0.7 && n[1] > 0.7, "{n:?}");
    }

    #[test]
    fn a_box_on_a_triangle_floor_rests_on_four_corners() {
        let tris = vec![[[-5.0, 0.0, -5.0], [5.0, 0.0, 5.0], [5.0, 0.0, -5.0]], [[-5.0, 0.0, -5.0], [-5.0, 0.0, 5.0], [5.0, 0.0, 5.0]]];
        let mesh = Shape::Mesh(Arc::new(TriMesh::new(tris)));
        let mut out = Vec::new();
        collide(&boxy([0.5; 3]), &Pose::at([0.3, -0.49, 0.2]), &mesh, &Pose::IDENTITY, 0.02, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].normal[1].abs() > 0.99);
        assert_eq!(out[0].points.len(), 4);
    }

    /// A flat square of two triangles at height `y` (down is +y).
    fn floor_at(y: f32, half: f32) -> Vec<[V3; 3]> {
        vec![[[-half, y, -half], [half, y, half], [half, y, -half]], [[-half, y, -half], [-half, y, half], [half, y, half]]]
    }

    #[test]
    fn a_sweep_stops_at_the_first_surface_under_it() {
        let mut tris = floor_at(0.0, 5.0);
        tris.extend(floor_at(2.0, 5.0));
        // A wall touching the box's side: beside it, not under it.
        tris.push([[0.5, -10.0, -5.0], [0.5, 10.0, -5.0], [0.5, 10.0, 5.0]]);
        let mesh = Shape::Mesh(Arc::new(TriMesh::new(tris)));
        let down = [0.0, 1.0, 0.0];
        let at = |y: f32| Pose::at([0.0, y, 0.0]);
        for shape in [boxy([0.5; 3]), Shape::Sphere { r: 0.5 }, Shape::Capsule { half: 0.0, r: 0.5 }] {
            let d = sweep(&shape, &at(-3.0), &mesh, &Pose::IDENTITY, down, 10.0).unwrap();
            assert!((d - 2.5).abs() < 1e-3, "{shape:?}: {d}");
            // Out of reach: nothing.
            assert!(sweep(&shape, &at(-3.0), &mesh, &Pose::IDENTITY, down, 2.0).is_none(), "{shape:?}");
            // Sunk 0.2 into the floor: back out by that much.
            let d = sweep(&shape, &at(-0.3), &mesh, &Pose::IDENTITY, down, 10.0).unwrap();
            assert!((d + 0.2).abs() < 1e-3, "{shape:?}: {d}");
            let o = overlap(&shape, &at(-0.3), &mesh, &Pose::IDENTITY, f32::INFINITY);
            assert!((o - 0.2).abs() < 1e-3, "{shape:?}: {o}");
            assert_eq!(overlap(&shape, &at(-3.0), &mesh, &Pose::IDENTITY, f32::INFINITY), 0.0);
        }
        // The same against a convex floor.
        let slab = boxy([5.0, 0.5, 5.0]);
        let d = sweep(&boxy([0.5; 3]), &at(-3.0), &slab, &at(0.5), down, 10.0).unwrap();
        assert!((d - 2.5).abs() < 1e-3, "{d}");
    }

    #[test]
    fn a_sweep_over_a_big_surface_looks_only_under_the_shape() {
        // 320 000 triangles of floor; a box above one corner of it.
        let n = 400;
        let mut tris = Vec::with_capacity(n * n * 2);
        for i in 0..n {
            for j in 0..n {
                let (x, z) = (i as f32 * 0.1 - 20.0, j as f32 * 0.1 - 20.0);
                let y = 0.05 * (x * 3.0).sin() * (z * 3.0).sin();
                tris.push([[x, y, z], [x + 0.1, y, z + 0.1], [x + 0.1, y, z]]);
                tris.push([[x, y, z], [x, y, z + 0.1], [x + 0.1, y, z + 0.1]]);
            }
        }
        let mesh = Shape::Mesh(Arc::new(TriMesh::new(tris)));
        let t0 = std::time::Instant::now();
        for k in 0..20 {
            let d = sweep(&boxy([0.5; 3]), &Pose::at([k as f32 - 10.0, -3.0, 4.0]), &mesh, &Pose::IDENTITY, [0.0, 1.0, 0.0], 100.0).unwrap();
            assert!(d > 2.4 && d < 2.6, "{d}");
        }
        assert!(t0.elapsed().as_secs_f32() < 1.0, "20 sweeps took {:?}", t0.elapsed());
    }
}
