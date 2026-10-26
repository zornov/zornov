// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Convex hulls: the shape most solid things collide as.
//!
//! Built by quickhull (start from a tetrahedron of extreme points, then
//! keep adding the point furthest outside a face, replacing the faces it
//! sees with a fan to their horizon). Triangles lying in one plane are
//! merged into polygon faces, which is what lets a box rest on four
//! corners instead of rocking on triangle edges. Flat or straight point
//! sets are given a little thickness so even a sheet of paper is a solid.

use super::math::*;
use std::collections::HashSet;

/// A face of a hull: its outward plane (`dot(n, x) = d`) and its corners,
/// anticlockwise seen from outside.
#[derive(Clone, Debug)]
pub struct Face {
    pub n: V3,
    pub d: f32,
    pub verts: Vec<u32>,
}

/// An edge between two faces.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub f0: u32,
    pub f1: u32,
}

/// A convex solid.
#[derive(Clone, Debug)]
pub struct Hull {
    pub verts: Vec<V3>,
    pub faces: Vec<Face>,
    pub edges: Vec<Edge>,
    /// The middle of its corners and the radius of a ball around it that
    /// holds every corner.
    pub centre: V3,
    pub radius: f32,
}

/// Volume, centre of mass and the second moment `∫ x xᵀ dV` about the
/// centre of mass, of a closed surface.
#[derive(Clone, Copy, Debug)]
pub struct Moments {
    pub volume: f32,
    pub com: V3,
    pub cov: M3,
}

impl Moments {
    /// The inertia tensor for a density.
    pub fn inertia(&self, density: f32) -> M3 {
        let c = self.cov;
        let tr = c[0][0] + c[1][1] + c[2][2];
        std::array::from_fn(|r| std::array::from_fn(|k| density * (if r == k { tr } else { 0.0 } - c[r][k])))
    }
}

/// The moments of a closed, outward-wound triangle surface (tetrahedra to
/// a reference point, summed with their signs).
pub fn moments(tris: impl Iterator<Item = [V3; 3]>, reference: V3) -> Moments {
    let mut vol = 0.0f64;
    let mut first = [0.0f64; 3];
    let mut cov = [[0.0f64; 3]; 3];
    for t in tris {
        let a = sub(t[0], reference).map(f64::from);
        let b = sub(t[1], reference).map(f64::from);
        let c = sub(t[2], reference).map(f64::from);
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let v = det / 6.0;
        vol += v;
        let s = [a[0] + b[0] + c[0], a[1] + b[1] + c[1], a[2] + b[2] + c[2]];
        for i in 0..3 {
            first[i] += v * s[i] / 4.0;
            for j in 0..3 {
                cov[i][j] += det / 120.0 * (a[i] * a[j] + b[i] * b[j] + c[i] * c[j] + s[i] * s[j]);
            }
        }
    }
    if vol.abs() < 1e-30 {
        return Moments { volume: 0.0, com: reference, cov: M_ZERO };
    }
    let com = first.map(|x| x / vol);
    let cov: M3 = std::array::from_fn(|i| std::array::from_fn(|j| (cov[i][j] - vol * com[i] * com[j]) as f32));
    Moments { volume: vol as f32, com: add(reference, com.map(|x| x as f32)), cov }
}

/// Unit directions spread evenly over the sphere (a Fibonacci spiral).
pub fn sphere_directions(n: usize) -> Vec<V3> {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let a = golden * i as f32;
            [r * a.cos(), y, r * a.sin()]
        })
        .collect()
}

struct Tri {
    v: [u32; 3],
    n: V3,
    d: f32,
    outside: Vec<u32>,
    alive: bool,
}

fn tri_of(pts: &[V3], v: [u32; 3]) -> Tri {
    let (a, b, c) = (pts[v[0] as usize], pts[v[1] as usize], pts[v[2] as usize]);
    let n = normalise_or(cross(sub(b, a), sub(c, a)), [0.0, 1.0, 0.0]);
    Tri { v, n, d: dot(n, a), outside: Vec::new(), alive: true }
}

impl Hull {
    /// A box with these half sizes about the origin.
    pub fn cuboid(half: V3) -> Hull {
        let pts: Vec<V3> = (0..8).map(|i| [if i & 1 == 0 { -half[0] } else { half[0] }, if i & 2 == 0 { -half[1] } else { half[1] }, if i & 4 == 0 { -half[2] } else { half[2] }]).collect();
        Hull::from_points(&pts, 8, 0.0).expect("a box has a hull")
    }

    /// The hull of `points` with at most `max_verts` corners (the extreme
    /// points in evenly spread directions when there are more). Flat or
    /// straight sets get `min_thickness` (or a hundredth of their size).
    pub fn from_points(points: &[V3], max_verts: usize, min_thickness: f32) -> Option<Hull> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in points {
            lo = vmin(lo, *p);
            hi = vmax(hi, *p);
        }
        if points.is_empty() || !lo.iter().chain(&hi).all(|v| v.is_finite()) {
            return None;
        }
        let size = length(sub(hi, lo));
        if size < 1e-9 {
            return None;
        }
        let eps = size * 2e-5;
        // Points that matter: the extremes when there are many.
        let max_verts = max_verts.max(8);
        let mut pts: Vec<V3> = if points.len() > max_verts.saturating_mul(3) {
            let mut keep: Vec<u32> = sphere_directions(max_verts.saturating_mul(2).min(4096)).iter().map(|d| (0..points.len()).max_by(|&a, &b| dot(points[a], *d).total_cmp(&dot(points[b], *d))).unwrap() as u32).collect();
            keep.sort_unstable();
            keep.dedup();
            keep.into_iter().map(|i| points[i as usize]).collect()
        } else {
            points.to_vec()
        };
        dedupe(&mut pts, eps * 4.0);
        let thick = min_thickness.max(size * 0.01);
        // Flat or straight: thicken.
        let flat = flatness(&pts);
        match flat {
            Flat::Point => return None,
            Flat::Line(u) => {
                let (a, b) = tangents(u);
                let base = pts.clone();
                pts.clear();
                for p in base {
                    for (sa, sb) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                        pts.push(add(p, add(scale(a, sa * thick * 0.5), scale(b, sb * thick * 0.5))));
                    }
                }
            }
            Flat::Plane(n) => {
                let base = pts.clone();
                pts.clear();
                for p in base {
                    pts.push(mad(p, n, thick * 0.5));
                    pts.push(mad(p, n, -thick * 0.5));
                }
            }
            Flat::Solid => {}
        }
        let hull = quickhull(&pts, eps)?;
        if hull.verts.len() > max_verts && points.len() > max_verts {
            // Still too many corners: fewer directions.
            let fewer: Vec<V3> = sphere_directions(max_verts).iter().map(|d| *hull.verts.iter().max_by(|a, b| dot(**a, *d).total_cmp(&dot(**b, *d))).unwrap()).collect();
            return Hull::from_points(&fewer, usize::MAX, min_thickness);
        }
        Some(hull)
    }

    /// The corner furthest along `dir`.
    pub fn support(&self, dir: V3) -> V3 {
        let mut best = self.verts[0];
        let mut bd = dot(best, dir);
        for v in &self.verts[1..] {
            let d = dot(*v, dir);
            if d > bd {
                bd = d;
                best = *v;
            }
        }
        best
    }

    /// The face most facing `dir`.
    pub fn face_towards(&self, dir: V3) -> usize {
        (0..self.faces.len()).max_by(|&a, &b| dot(self.faces[a].n, dir).total_cmp(&dot(self.faces[b].n, dir))).unwrap_or(0)
    }

    pub fn triangles(&self) -> impl Iterator<Item = [V3; 3]> + '_ {
        self.faces.iter().flat_map(move |f| (1..f.verts.len().saturating_sub(1)).map(move |k| [self.verts[f.verts[0] as usize], self.verts[f.verts[k] as usize], self.verts[f.verts[k + 1] as usize]]))
    }

    pub fn moments(&self) -> Moments {
        moments(self.triangles(), self.centre)
    }

    pub fn volume(&self) -> f32 {
        self.moments().volume
    }

    /// Moved by a pose (a hull in another frame).
    pub fn transformed(&self, pose: &Pose) -> Hull {
        let verts: Vec<V3> = self.verts.iter().map(|v| pose.apply(*v)).collect();
        let faces = self
            .faces
            .iter()
            .map(|f| {
                let n = pose.rotate(f.n);
                Face { n, d: dot(n, verts[f.verts[0] as usize]), verts: f.verts.clone() }
            })
            .collect();
        Hull { centre: pose.apply(self.centre), radius: self.radius, verts, faces, edges: self.edges.clone() }
    }

    /// Scaled about the origin.
    pub fn scaled(&self, k: V3) -> Option<Hull> {
        let pts: Vec<V3> = self.verts.iter().map(|v| [v[0] * k[0], v[1] * k[1], v[2] * k[2]]).collect();
        Hull::from_points(&pts, usize::MAX, 0.0)
    }

    /// Whether `p` is inside (within `tol`).
    pub fn contains(&self, p: V3, tol: f32) -> bool {
        self.faces.iter().all(|f| dot(f.n, p) - f.d <= tol)
    }
}

fn dedupe(pts: &mut Vec<V3>, eps: f32) {
    let cell = eps.max(1e-12);
    let mut seen = HashSet::new();
    pts.retain(|p| seen.insert(p.map(|v| (v / cell).round() as i64)));
}

enum Flat {
    Point,
    Line(V3),
    Plane(V3),
    Solid,
}

/// Whether a point set has volume, by its spread along principal axes.
fn flatness(pts: &[V3]) -> Flat {
    let n = pts.len() as f32;
    let c = scale(pts.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / n);
    let mut cov = M_ZERO;
    for p in pts {
        let d = sub(*p, c);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j] / n;
            }
        }
    }
    let (v, e) = eigen_sym(&cov);
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| e[b].total_cmp(&e[a]));
    let col = |k: usize| [v[0][k], v[1][k], v[2][k]];
    let big = e[order[0]].max(0.0).sqrt();
    if big < 1e-9 {
        return Flat::Point;
    }
    let mid = e[order[1]].max(0.0).sqrt();
    let small = e[order[2]].max(0.0).sqrt();
    if mid < big * 1e-4 {
        return Flat::Line(col(order[0]));
    }
    // Exactly flat (within rounding) along the smallest axis.
    let axis = col(order[2]);
    let spread = pts.iter().map(|p| dot(sub(*p, c), axis).abs()).fold(0.0f32, f32::max);
    if small < big * 1e-4 || spread < big * 1e-4 {
        return Flat::Plane(axis);
    }
    Flat::Solid
}

fn quickhull(pts: &[V3], eps: f32) -> Option<Hull> {
    let n = pts.len();
    if n < 4 {
        return None;
    }
    // A starting tetrahedron of extreme points.
    let ex: Vec<usize> = (0..3).flat_map(|k| [(0..n).min_by(|&a, &b| pts[a][k].total_cmp(&pts[b][k])).unwrap(), (0..n).max_by(|&a, &b| pts[a][k].total_cmp(&pts[b][k])).unwrap()]).collect();
    let (mut i0, mut i1, mut best) = (ex[0], ex[1], -1.0f32);
    for &a in &ex {
        for &b in &ex {
            let d = len2(sub(pts[a], pts[b]));
            if d > best {
                best = d;
                i0 = a;
                i1 = b;
            }
        }
    }
    let line = sub(pts[i1], pts[i0]);
    let i2 = (0..n).max_by(|&a, &b| len2(cross(line, sub(pts[a], pts[i0]))).total_cmp(&len2(cross(line, sub(pts[b], pts[i0]))))).unwrap();
    let pn = normalise_or(cross(line, sub(pts[i2], pts[i0])), [0.0, 1.0, 0.0]);
    let i3 = (0..n).max_by(|&a, &b| dot(pn, sub(pts[a], pts[i0])).abs().total_cmp(&dot(pn, sub(pts[b], pts[i0])).abs())).unwrap();
    if dot(pn, sub(pts[i3], pts[i0])).abs() < eps {
        return None;
    }
    let [a, b, c, d] = [i0 as u32, i1 as u32, i2 as u32, i3 as u32];
    let mut tris: Vec<Tri> = if dot(pn, sub(pts[i3], pts[i0])) < 0.0 {
        // d is below abc (abc's normal points away from d): abc faces out.
        vec![tri_of(pts, [a, b, c]), tri_of(pts, [a, d, b]), tri_of(pts, [b, d, c]), tri_of(pts, [c, d, a])]
    } else {
        vec![tri_of(pts, [a, c, b]), tri_of(pts, [a, b, d]), tri_of(pts, [b, c, d]), tri_of(pts, [c, a, d])]
    };
    let seed = [a, b, c, d];
    for p in 0..n as u32 {
        if seed.contains(&p) {
            continue;
        }
        if let Some(t) = tris.iter_mut().find(|t| dot(t.n, pts[p as usize]) - t.d > eps) {
            t.outside.push(p);
        }
    }
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 4 * n + 64 {
            break;
        }
        let Some(fi) = tris.iter().position(|t| t.alive && !t.outside.is_empty()) else { break };
        let p = *tris[fi].outside.iter().max_by(|&&x, &&y| (dot(tris[fi].n, pts[x as usize]) - tris[fi].d).total_cmp(&(dot(tris[fi].n, pts[y as usize]) - tris[fi].d))).unwrap();
        let pp = pts[p as usize];
        let visible: Vec<usize> = (0..tris.len()).filter(|&k| tris[k].alive && dot(tris[k].n, pp) - tris[k].d > eps).collect();
        if visible.is_empty() {
            tris[fi].outside.retain(|&x| x != p);
            continue;
        }
        let mut edges = HashSet::new();
        for &k in &visible {
            let v = tris[k].v;
            for e in 0..3 {
                edges.insert((v[e], v[(e + 1) % 3]));
            }
        }
        let mut horizon = Vec::new();
        for &k in &visible {
            let v = tris[k].v;
            for e in 0..3 {
                let (x, y) = (v[e], v[(e + 1) % 3]);
                if !edges.contains(&(y, x)) {
                    horizon.push((x, y));
                }
            }
        }
        let mut orphans = Vec::new();
        for &k in &visible {
            tris[k].alive = false;
            orphans.append(&mut tris[k].outside);
        }
        let first_new = tris.len();
        for (x, y) in horizon {
            tris.push(tri_of(pts, [x, y, p]));
        }
        for o in orphans {
            if o == p {
                continue;
            }
            let q = pts[o as usize];
            if let Some(t) = tris[first_new..].iter_mut().filter(|t| dot(t.n, q) - t.d > eps).max_by(|a, b| (dot(a.n, q) - a.d).total_cmp(&(dot(b.n, q) - b.d))) {
                t.outside.push(o);
            }
        }
    }
    let live: Vec<&Tri> = tris.iter().filter(|t| t.alive).collect();
    merge_faces(pts, &live, eps)
}

/// Joins triangles lying in one plane into polygons and builds the hull.
fn merge_faces(pts: &[V3], tris: &[&Tri], eps: f32) -> Option<Hull> {
    let m = tris.len();
    if m < 4 {
        return None;
    }
    // Triangle by directed edge.
    let mut by_edge = std::collections::HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        for e in 0..3 {
            by_edge.insert((t.v[e], t.v[(e + 1) % 3]), k);
        }
    }
    let mut parent: Vec<usize> = (0..m).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let area_n = |t: &Tri| {
        let (a, b, c) = (pts[t.v[0] as usize], pts[t.v[1] as usize], pts[t.v[2] as usize]);
        cross(sub(b, a), sub(c, a))
    };
    for (k, t) in tris.iter().enumerate() {
        for e in 0..3 {
            let Some(&o) = by_edge.get(&(t.v[(e + 1) % 3], t.v[e])) else { continue };
            if o <= k {
                continue;
            }
            let u = tris[o];
            let coplanar = dot(t.n, u.n) > 1.0 - 2e-4 && u.v.iter().all(|&v| (dot(t.n, pts[v as usize]) - t.d).abs() < eps * 4.0);
            if coplanar {
                let (ra, rb) = (root(&mut parent, k), root(&mut parent, o));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for k in 0..m {
        let r = root(&mut parent, k);
        groups.entry(r).or_default().push(k);
    }
    let mut faces = Vec::new();
    for (_, members) in groups {
        let mut nsum = [0.0; 3];
        let mut next = std::collections::HashMap::new();
        let mine: HashSet<usize> = members.iter().copied().collect();
        for &k in &members {
            nsum = add(nsum, area_n(tris[k]));
            let v = tris[k].v;
            for e in 0..3 {
                let (x, y) = (v[e], v[(e + 1) % 3]);
                let other = by_edge.get(&(y, x)).copied();
                if other.is_none_or(|o| !mine.contains(&o)) {
                    next.insert(x, y);
                }
            }
        }
        let Some(&start) = next.keys().min() else { continue };
        let mut loop_ = vec![start];
        let mut cur = start;
        while let Some(&nx) = next.get(&cur) {
            if nx == start || loop_.len() > next.len() {
                break;
            }
            loop_.push(nx);
            cur = nx;
        }
        if loop_.len() < 3 {
            continue;
        }
        let n = normalise_or(nsum, tris[members[0]].n);
        // Corners where the outline runs straight on are dropped.
        let clean: Vec<u32> = (0..loop_.len())
            .filter(|&i| {
                let a = pts[loop_[(i + loop_.len() - 1) % loop_.len()] as usize];
                let b = pts[loop_[i] as usize];
                let c = pts[loop_[(i + 1) % loop_.len()] as usize];
                length(cross(sub(b, a), sub(c, b))) > eps * eps.max(length(sub(c, a))) * 4.0
            })
            .map(|i| loop_[i])
            .collect();
        let verts = if clean.len() >= 3 { clean } else { loop_ };
        let d = verts.iter().map(|&v| dot(n, pts[v as usize])).fold(f32::NEG_INFINITY, f32::max);
        faces.push(Face { n, d, verts });
    }
    // Keep only corners the faces use, renumbered.
    let mut map = std::collections::HashMap::new();
    let mut verts = Vec::new();
    for f in &mut faces {
        for v in &mut f.verts {
            let id = *map.entry(*v).or_insert_with(|| {
                verts.push(pts[*v as usize]);
                verts.len() as u32 - 1
            });
            *v = id;
        }
    }
    // Edges: each undirected outline segment with the faces either side.
    let mut owner = std::collections::HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.verts.len() {
            owner.insert((f.verts[i], f.verts[(i + 1) % f.verts.len()]), fi as u32);
        }
    }
    let mut edges = Vec::new();
    for (&(a, b), &f0) in &owner {
        if a < b {
            let f1 = owner.get(&(b, a)).copied().unwrap_or(f0);
            edges.push(Edge { a, b, f0, f1 });
        }
    }
    edges.sort_by_key(|e| (e.a, e.b));
    if faces.len() < 4 || verts.len() < 4 {
        return None;
    }
    let centre = scale(verts.iter().fold([0.0; 3], |a, v| add(a, *v)), 1.0 / verts.len() as f32);
    let radius = verts.iter().map(|v| length(sub(*v, centre))).fold(0.0f32, f32::max);
    Some(Hull { verts, faces, edges, centre, radius })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_hull_has_six_square_faces_and_its_volume() {
        let h = Hull::cuboid([1.0, 2.0, 3.0]);
        assert_eq!(h.faces.len(), 6, "{:?}", h.faces.iter().map(|f| f.verts.len()).collect::<Vec<_>>());
        assert!(h.faces.iter().all(|f| f.verts.len() == 4));
        assert_eq!(h.edges.len(), 12);
        assert_eq!(h.verts.len(), 8);
        let m = h.moments();
        assert!((m.volume - 48.0).abs() < 1e-3, "{}", m.volume);
        assert!(length(m.com) < 1e-4);
        // A solid box: I_xx = m (b² + c²) / 3 for half sizes b, c.
        let i = m.inertia(1.0);
        assert!((i[0][0] - 48.0 * (4.0 + 9.0) / 3.0).abs() < 0.05, "{:?}", i);
        assert!(i[0][1].abs() < 1e-3);
        // Faces point out.
        for f in &h.faces {
            assert!(dot(f.n, sub(h.verts[f.verts[0] as usize], h.centre)) > 0.0);
        }
    }

    #[test]
    fn a_ball_of_points_makes_a_round_hull_within_its_budget() {
        let pts: Vec<V3> = sphere_directions(500).into_iter().map(|d| scale(d, 2.0)).collect();
        let h = Hull::from_points(&pts, 48, 0.0).unwrap();
        assert!(h.verts.len() <= 48, "{}", h.verts.len());
        let v = h.volume();
        let ball = 4.0 / 3.0 * std::f32::consts::PI * 8.0;
        assert!(v > ball * 0.8 && v <= ball * 1.001, "{v} vs {ball}");
        for p in &h.verts {
            assert!(h.contains(*p, 1e-3));
        }
    }

    #[test]
    fn a_flat_sheet_gets_thickness() {
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.5, 0.0, 0.5]];
        let h = Hull::from_points(&pts, 32, 0.05).unwrap();
        let v = h.volume();
        assert!((v - 0.05).abs() < 0.01, "{v}");
    }

    #[test]
    fn edges_join_the_two_faces_they_divide() {
        let pts: Vec<V3> = sphere_directions(60);
        let h = Hull::from_points(&pts, 64, 0.0).unwrap();
        for e in &h.edges {
            assert_ne!(e.f0, e.f1);
            for f in [e.f0, e.f1] {
                let fv = &h.faces[f as usize].verts;
                assert!(fv.contains(&e.a) && fv.contains(&e.b));
            }
        }
        // Euler: V − E + F = 2.
        assert_eq!(h.verts.len() as i32 - h.edges.len() as i32 + h.faces.len() as i32, 2);
    }
}
