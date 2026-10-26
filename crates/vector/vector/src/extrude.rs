// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Flat outlines made solid: 3D text and extruded shapes. Outlines are
//! cleaned into shapes with holes, triangulated (ear clipping with holes
//! bridged in), and given a front, a back, sides, and an optional bevel.
//!
//! Units are canvas pixels, x right, y down, z away from the viewer (the
//! front faces the camera at z = -depth / 2), as Studio's 3D models are.

use crate::path::P;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// A solid ready to become a model: triangles in three parts (the faces,
/// the bevel, the sides), each a run of triangles for its own material.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Solid {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Triangle counts of the face, bevel and side parts, in that order.
    pub parts: [u32; 3],
}

fn area(c: &[P]) -> f64 {
    let n = c.len();
    (0..n).map(|k| c[k][0] * c[(k + 1) % n][1] - c[(k + 1) % n][0] * c[k][1]).sum::<f64>() * 0.5
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn inside_tri(p: P, a: P, b: P, c: P) -> bool {
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Shapes (outer outline first, then its holes) from any outlines, with
/// overlaps resolved by the non-zero rule (as fonts draw).
pub fn shapes(contours: &[Vec<P>]) -> Vec<Vec<Vec<P>>> {
    let clean: Vec<Vec<P>> = contours.iter().filter(|c| c.len() >= 3).cloned().collect();
    if clean.is_empty() {
        return Vec::new();
    }
    let out: Vec<Vec<Vec<P>>> = clean.overlay(&Vec::<Vec<P>>::new(), OverlayRule::Subject, FillRule::NonZero);
    out.into_iter()
        .map(|shape| {
            shape
                .into_iter()
                .enumerate()
                .map(|(k, mut c)| {
                    // Outer outlines one way round, holes the other.
                    let a = area(&c);
                    if (k == 0 && a < 0.0) || (k > 0 && a > 0.0) {
                        c.reverse();
                    }
                    c
                })
                .filter(|c| c.len() >= 3)
                .collect::<Vec<_>>()
        })
        .filter(|s: &Vec<Vec<P>>| !s.is_empty())
        .collect()
}

/// One polygon with its holes joined in by bridges: the vertex order to
/// clip, as (contour, vertex) pairs.
fn bridge(shape: &[Vec<P>]) -> Vec<(usize, usize)> {
    let pt = |q: (usize, usize)| shape[q.0][q.1];
    let mut poly: Vec<(usize, usize)> = (0..shape[0].len()).map(|k| (0, k)).collect();
    let mut holes: Vec<usize> = (1..shape.len()).collect();
    // Rightmost holes first.
    let max_x = |h: usize| shape[h].iter().map(|p| p[0]).fold(f64::MIN, f64::max);
    holes.sort_by(|a, b| max_x(*b).total_cmp(&max_x(*a)));
    for h in holes {
        let hv = &shape[h];
        let mi = (0..hv.len()).max_by(|a, b| hv[*a][0].total_cmp(&hv[*b][0])).unwrap_or(0);
        let m = hv[mi];
        // The nearest edge hit by a ray from M to the right.
        let n = poly.len();
        let mut best: Option<(f64, usize)> = None;
        for k in 0..n {
            let (a, b) = (pt(poly[k]), pt(poly[(k + 1) % n]));
            if (a[1] > m[1]) == (b[1] > m[1]) {
                continue;
            }
            let x = a[0] + (m[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if x >= m[0] - 1e-12 && best.is_none_or(|(bx, _)| x < bx) {
                best = Some((x, k));
            }
        }
        let Some((ix, k)) = best else { continue };
        let (a, b) = (pt(poly[k]), pt(poly[(k + 1) % n]));
        let mut pick = if a[0] > b[0] { k } else { (k + 1) % n };
        let i = [ix, m[1]];
        let p = pt(poly[pick]);
        // A reflex vertex inside (M, I, P) blocks the view: take the one
        // closest in angle to the ray.
        let mut best_ang = f64::MAX;
        for j in 0..n {
            if j == pick {
                continue;
            }
            let q = pt(poly[j]);
            let prev = pt(poly[(j + n - 1) % n]);
            let next = pt(poly[(j + 1) % n]);
            let reflex = cross(prev, q, next) <= 0.0;
            if reflex && q[0] >= m[0] && inside_tri(q, m, i, p) && q != p {
                let ang = ((q[1] - m[1]).abs()).atan2(q[0] - m[0]);
                if ang < best_ang {
                    best_ang = ang;
                    pick = j;
                }
            }
        }
        let mut merged = Vec::with_capacity(poly.len() + hv.len() + 2);
        merged.extend_from_slice(&poly[..=pick]);
        for s in 0..=hv.len() {
            merged.push((h, (mi + s) % hv.len()));
        }
        merged.push(poly[pick]);
        merged.extend_from_slice(&poly[pick + 1..]);
        poly = merged;
    }
    poly
}

/// Triangles over `order` (indices into it), by ear clipping.
fn clip(order: &[(usize, usize)], pt: &dyn Fn((usize, usize)) -> P) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..order.len()).collect();
    let mut tris = Vec::with_capacity(order.len());
    let p = |i: usize| pt(order[i]);
    let mut guard = 0;
    while idx.len() > 3 && guard < 100_000 {
        guard += 1;
        let n = idx.len();
        let mut ear = None;
        for k in 0..n {
            let (a, b, c) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            let (pa, pb, pc) = (p(a), p(b), p(c));
            if cross(pa, pb, pc) <= 1e-12 {
                continue;
            }
            let blocked = idx.iter().any(|&j| {
                if j == a || j == b || j == c {
                    return false;
                }
                let q = p(j);
                q != pa && q != pb && q != pc && inside_tri(q, pa, pb, pc)
            });
            if !blocked {
                ear = Some(k);
                break;
            }
        }
        // Nothing clean to cut (rounding on a bridge): cut the flattest.
        let k = ear.unwrap_or_else(|| {
            (0..n)
                .min_by(|x, y| {
                    let f = |k: usize| cross(p(idx[(k + n - 1) % n]), p(idx[k]), p(idx[(k + 1) % n])).abs();
                    f(*x).total_cmp(&f(*y))
                })
                .unwrap_or(0)
        });
        let (a, b, c) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
        if cross(p(a), p(b), p(c)).abs() > 1e-12 {
            tris.push([a, b, c]);
        }
        idx.remove(k);
    }
    if idx.len() == 3 && cross(p(idx[0]), p(idx[1]), p(idx[2])).abs() > 1e-12 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

/// Flat triangles covering `contours` (non-zero), in the plane.
pub fn triangulate(contours: &[Vec<P>]) -> (Vec<P>, Vec<[u32; 3]>) {
    let mut pts = Vec::new();
    let mut tris = Vec::new();
    for shape in shapes(contours) {
        let order = bridge(&shape);
        let base = pts.len() as u32;
        let pt = |q: (usize, usize)| shape[q.0][q.1];
        for q in &order {
            pts.push(pt(*q));
        }
        for t in clip(&order, &pt) {
            tris.push([base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]);
        }
    }
    (pts, tris)
}

fn norm3(v: [f64; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [(v[0] / l) as f32, (v[1] / l) as f32, (v[2] / l) as f32]
}

/// Builds the solid: `depth` thick, edges bevelled by `bevel` (0 for
/// square edges), sides smooth where they turn less than `smooth_deg`.
pub fn extrude(contours: &[Vec<P>], depth: f64, bevel: f64, smooth_deg: f64) -> Solid {
    let depth = depth.max(0.0);
    let shapes = shapes(contours);
    let mut s = Solid::default();
    let (zf, zb) = (-depth / 2.0, depth / 2.0);
    // The bevel can't eat more than a third of the depth.
    let b = bevel.clamp(0.0, depth / 3.0);
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    for sh in &shapes {
        for c in sh {
            for p in c {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
    }
    let span = [(hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6)];
    let uv = |p: P| [((p[0] - lo[0]) / span[0]) as f32, ((p[1] - lo[1]) / span[1]) as f32];
    let push = |s: &mut Solid, p: [f64; 3], n: [f32; 3], t: [f32; 2]| -> u32 {
        s.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        s.normals.push(n);
        s.uvs.push(t);
        (s.positions.len() - 1) as u32
    };
    // Outward (away from the material) unit normal of each edge and the
    // inset of each vertex, per contour of each shape. Outer outlines
    // have positive area (clockwise on screen): the material is on the
    // right of travel.
    struct Ring {
        pts: Vec<P>,
        inset: Vec<P>,
        edge_n: Vec<P>,
    }
    let rings: Vec<Vec<Ring>> = shapes
        .iter()
        .map(|sh| {
            sh.iter()
                .map(|c| {
                    let n = c.len();
                    let edge_n: Vec<P> = (0..n)
                        .map(|k| {
                            let (a, bb) = (c[k], c[(k + 1) % n]);
                            let (dx, dy) = (bb[0] - a[0], bb[1] - a[1]);
                            let l = (dx * dx + dy * dy).sqrt().max(1e-12);
                            // Left of travel on screen (y down) is (dy, -dx).
                            [dy / l, -dx / l]
                        })
                        .collect();
                    let inset: Vec<P> = (0..n)
                        .map(|k| {
                            let (n0, n1) = (edge_n[(k + n - 1) % n], edge_n[k]);
                            let m = [n0[0] + n1[0], n0[1] + n1[1]];
                            let ml = (m[0] * m[0] + m[1] * m[1]).sqrt();
                            if ml < 1e-9 {
                                return c[k];
                            }
                            let m = [m[0] / ml, m[1] / ml];
                            let cosh = (m[0] * n1[0] + m[1] * n1[1]).max(0.4);
                            let d = b / cosh;
                            [c[k][0] - m[0] * d, c[k][1] - m[1] * d]
                        })
                        .collect();
                    Ring { pts: c.clone(), inset, edge_n }
                })
                .collect()
        })
        .collect();
    // 1. Faces: front (normal towards the viewer) and back.
    for (si, sh) in shapes.iter().enumerate() {
        let order = bridge(sh);
        let pt = |q: (usize, usize)| sh[q.0][q.1];
        let tris = clip(&order, &pt);
        let face_pt = |q: (usize, usize)| if b > 0.0 { rings[si][q.0].inset[q.1] } else { pt(q) };
        for (z, nz) in [(zf, -1.0f32), (zb, 1.0f32)] {
            let base = s.positions.len() as u32;
            for q in &order {
                let p = face_pt(*q);
                push(&mut s, [p[0], p[1], z], [0.0, 0.0, nz], uv(p));
            }
            for t in &tris {
                let (a, bb, c) = (face_pt(order[t[0]]), face_pt(order[t[1]]), face_pt(order[t[2]]));
                // cross(b - a, c - a) along z must match the face normal.
                let cz = cross(a, bb, c);
                let flip = (cz > 0.0) != (nz > 0.0);
                let (i0, i1, i2) = (base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
                if flip {
                    s.indices.extend_from_slice(&[i0, i2, i1]);
                } else {
                    s.indices.extend_from_slice(&[i0, i1, i2]);
                }
                s.parts[0] += 1;
            }
        }
    }
    // A quad between two rows of points with normals, wound outwards.
    let quad = |s: &mut Solid, p: [[f64; 3]; 4], n: [[f32; 3]; 4], t: [[f32; 2]; 4]| -> u32 {
        let base = s.positions.len() as u32;
        for k in 0..4 {
            s.positions.push([p[k][0] as f32, p[k][1] as f32, p[k][2] as f32]);
            s.normals.push(n[k]);
            s.uvs.push(t[k]);
        }
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let nn = [n[0][0] + n[1][0] + n[2][0] + n[3][0], n[0][1] + n[1][1] + n[2][1] + n[3][1], n[0][2] + n[1][2] + n[2][2] + n[3][2]];
        let along = c[0] * nn[0] as f64 + c[1] * nn[1] as f64 + c[2] * nn[2] as f64;
        if along >= 0.0 {
            s.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            s.indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
        2
    };
    let smooth_cos = smooth_deg.to_radians().cos();
    // Vertex normals for the sides: averaged where the outline turns gently.
    let side_n = |r: &Ring, k: usize, edge: usize, sign: f64| -> P {
        let n = r.pts.len();
        let (a, bb) = (r.edge_n[(k + n - 1) % n], r.edge_n[k % n]);
        let e = r.edge_n[edge];
        let dot = a[0] * bb[0] + a[1] * bb[1];
        let v = if dot >= smooth_cos {
            let m = [a[0] + bb[0], a[1] + bb[1]];
            let l = (m[0] * m[0] + m[1] * m[1]).sqrt().max(1e-12);
            [m[0] / l, m[1] / l]
        } else {
            e
        };
        [v[0] * sign, v[1] * sign]
    };
    // 2. Bevels, then 3. sides.
    for pass in 0..2 {
        for rs in &rings {
            for r in rs {
                let n = r.pts.len();
                // Outer outlines run clockwise on screen and holes the
                // other way, so left of travel is always out of the
                // material.
                let sign = 1.0;
                let mut along = 0.0f64;
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    let (a, c) = (r.pts[k], r.pts[k1]);
                    let len = ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt();
                    let (u0, u1) = ((along / 200.0) as f32, ((along + len) / 200.0) as f32);
                    along += len;
                    let (na, nc) = (side_n(r, k, k, sign), side_n(r, k1, k, sign));
                    let flat = |v: P, z: f64| [v[0], v[1], z];
                    if pass == 0 {
                        if b <= 0.0 {
                            continue;
                        }
                        let (ia, ic) = (r.inset[k], r.inset[k1]);
                        for (zi, zo, nz) in [(zf, zf + b, -1.0f64), (zb, zb - b, 1.0f64)] {
                            // Half way between the face and the side.
                            let bn = |v: P| norm3([v[0], v[1], nz]);
                            quad(&mut s, [flat(ia, zi), flat(ic, zi), flat(c, zo), flat(a, zo)], [bn(na), bn(nc), bn(nc), bn(na)], [[u0, 0.0], [u1, 0.0], [u1, 0.1], [u0, 0.1]]);
                            s.parts[1] += 2;
                        }
                    } else {
                        let sn = |v: P| norm3([v[0], v[1], 0.0]);
                        quad(&mut s, [flat(a, zf + b), flat(c, zf + b), flat(c, zb - b), flat(a, zb - b)], [sn(na), sn(nc), sn(nc), sn(na)], [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]]);
                        s.parts[2] += 2;
                    }
                }
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(c: P, h: f64) -> Vec<P> {
        vec![[c[0] - h, c[1] - h], [c[0] + h, c[1] - h], [c[0] + h, c[1] + h], [c[0] - h, c[1] + h]]
    }

    /// A hole runs the other way round, as fonts draw them.
    fn hole(c: P, h: f64) -> Vec<P> {
        let mut v = square(c, h);
        v.reverse();
        v
    }

    fn tri_area(pts: &[P], t: &[u32; 3]) -> f64 {
        cross(pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]).abs() * 0.5
    }

    #[test]
    fn a_square_with_a_hole_triangulates_to_its_area() {
        let (pts, tris) = triangulate(&[square([0.0, 0.0], 100.0), hole([0.0, 0.0], 40.0)]);
        let total: f64 = tris.iter().map(|t| tri_area(&pts, t)).sum();
        assert!((total - (200.0 * 200.0 - 80.0 * 80.0)).abs() < 1e-6, "{total}");
        // An L and two separate pieces.
        let l = vec![[0.0, 0.0], [100.0, 0.0], [100.0, 30.0], [30.0, 30.0], [30.0, 100.0], [0.0, 100.0]];
        let (pts, tris) = triangulate(&[l, square([300.0, 300.0], 10.0)]);
        let total: f64 = tris.iter().map(|t| tri_area(&pts, t)).sum();
        assert!((total - (100.0 * 30.0 + 30.0 * 70.0 + 400.0)).abs() < 1e-6, "{total}");
    }

    #[test]
    fn extrusions_face_out_and_close_up() {
        for bevel in [0.0, 6.0] {
            let s = extrude(&[square([0.0, 0.0], 100.0), hole([0.0, 0.0], 40.0)], 60.0, bevel, 30.0);
            assert!(s.parts[0] > 0 && s.parts[2] > 0);
            assert_eq!(s.parts[1] > 0, bevel > 0.0);
            assert_eq!(s.indices.len() as u32, 3 * (s.parts[0] + s.parts[1] + s.parts[2]));
            // Every triangle's winding agrees with its normals.
            for t in s.indices.chunks_exact(3) {
                let p = |i: u32| s.positions[i as usize];
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let x = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let n = s.normals[t[0] as usize];
                assert!(x[0] * n[0] + x[1] * n[1] + x[2] * n[2] > 0.0, "a triangle faces in");
            }
            // The outer sides face away from the centre, the hole's into it.
            let mut outward = 0;
            for (p, n) in s.positions.iter().zip(&s.normals) {
                if n[2].abs() < 0.5 {
                    let r = p[0] * n[0] + p[1] * n[1];
                    let on_outer = p[0].abs() > 90.0 || p[1].abs() > 90.0;
                    if on_outer {
                        assert!(r > 0.0, "outer side faces in at {p:?}");
                    } else {
                        assert!(r < 0.0, "hole side faces out at {p:?}");
                    }
                    outward += 1;
                }
            }
            assert!(outward > 0);
            // The front is at -depth/2 and faces the viewer.
            let front = s.positions.iter().zip(&s.normals).find(|(_, n)| n[2] < -0.99).unwrap();
            assert!((front.0[2] + 30.0).abs() < 1e-4);
        }
    }
}
