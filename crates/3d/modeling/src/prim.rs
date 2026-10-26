// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Starting shapes built from quads, laid out for texturing: what a
//! modeler blocks out with before extruding, beveling and subdividing.
//! Sizes are in Studio's pixels; y points down, so "up" is −y.

use crate::poly::Poly;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use studio_mesh::{V3, add, cross, normalise, scale};

/// Welds grid points that land in the same place.
struct Welder {
    map: HashMap<[i64; 3], u32>,
}

impl Welder {
    fn new() -> Welder {
        Welder { map: HashMap::new() }
    }
    fn at(&mut self, p: &mut Poly, v: V3) -> u32 {
        let k = [(v[0] * 1e4).round() as i64, (v[1] * 1e4).round() as i64, (v[2] * 1e4).round() as i64];
        *self.map.entry(k).or_insert_with(|| p.add_vertex(v))
    }
}

/// A box `size` across with `segments` cuts per side, its six sides laid
/// out in a 3×2 texture grid.
pub fn cube(size: V3, segments: usize) -> Poly {
    let s = segments.max(1);
    let mut p = Poly::new();
    p.uvs = Vec::new();
    let mut w = Welder::new();
    let h = scale(size, 0.5);
    // (normal axis, sign) with in-plane axes u × v = normal.
    let sides: [(V3, V3, V3); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    for (k, (n, u, v)) in sides.iter().enumerate() {
        // u × v must equal n; flip v when it doesn't.
        let c = cross(*u, *v);
        let v = if c[0] * n[0] + c[1] * n[1] + c[2] * n[2] < 0.0 { scale(*v, -1.0) } else { *v };
        let pt = |i: usize, j: usize| -> V3 {
            let (a, b) = (i as f32 / s as f32 - 0.5, j as f32 / s as f32 - 0.5);
            let q = add(add(*n, scale(*u, 2.0 * a)), scale(v, 2.0 * b));
            [q[0] * h[0], q[1] * h[1], q[2] * h[2]]
        };
        let (cu, cv) = ((k % 3) as f32 / 3.0, (k / 3) as f32 / 2.0);
        let uv = |i: usize, j: usize| [cu + (i as f32 / s as f32) / 3.0, cv + (j as f32 / s as f32) / 2.0];
        for i in 0..s {
            for j in 0..s {
                let ids = [w.at(&mut p, pt(i, j)), w.at(&mut p, pt(i + 1, j)), w.at(&mut p, pt(i + 1, j + 1)), w.at(&mut p, pt(i, j + 1))];
                let uvs = [uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)];
                p.add_face_like(&ids, Some(&uvs), 0, 0, true);
            }
        }
    }
    p.ensure_materials();
    p
}

/// A flat sheet facing up (−y), `width` along x and `depth` along z.
pub fn plane(width: f32, depth: f32, cuts_x: usize, cuts_z: usize) -> Poly {
    let (nx, nz) = (cuts_x.max(1), cuts_z.max(1));
    let mut p = Poly::new();
    for j in 0..=nz {
        for i in 0..=nx {
            p.add_vertex([(i as f32 / nx as f32 - 0.5) * width, 0.0, (j as f32 / nz as f32 - 0.5) * depth]);
        }
    }
    let id = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
    let uv = |i: usize, j: usize| [i as f32 / nx as f32, j as f32 / nz as f32];
    for j in 0..nz {
        for i in 0..nx {
            // Wound so the normal points to −y (up).
            let v = [id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)];
            let u = [uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)];
            p.add_face_like(&v, Some(&u), 0, 0, true);
        }
    }
    p.ensure_materials();
    p
}

/// How a cylinder's ends are closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    None,
    /// One many-sided face (clean for booleans and bevels).
    Ngon,
    /// Triangles to a centre point.
    Fan,
    /// Quads (an inner ring of quads around a smaller n-gon, so it
    /// subdivides round).
    Quads,
}

/// A cylinder standing along y (its top at −height/2), `segments` around,
/// `rings` cuts along its length.
pub fn cylinder(radius_top: f32, radius_bottom: f32, height: f32, segments: usize, rings: usize, cap: Cap) -> Poly {
    let n = segments.max(3);
    let r = rings.max(1);
    let mut p = Poly::new();
    p.uvs = Vec::new();
    for j in 0..=r {
        let t = j as f32 / r as f32;
        let y = -height / 2.0 + t * height;
        let rad = radius_top + (radius_bottom - radius_top) * t;
        for i in 0..n {
            let a = TAU * i as f32 / n as f32;
            p.add_vertex([rad * a.cos(), y, rad * a.sin()]);
        }
    }
    let id = |i: usize, j: usize| (j * n + i % n) as u32;
    for j in 0..r {
        for i in 0..n {
            let v = [id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)];
            let (u0, u1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            let (v0, v1) = (0.5 * j as f32 / r as f32, 0.5 * (j + 1) as f32 / r as f32);
            p.add_face_like(&v, Some(&[[u0, v0], [u1, v0], [u1, v1], [u0, v1]]), 0, 0, true);
        }
    }
    for (ring, top, rad) in [(0usize, true, radius_top), (r, false, radius_bottom)] {
        if rad <= 1e-6 {
            continue;
        }
        let y = if top { -height / 2.0 } else { height / 2.0 };
        let cx = if top { 0.25 } else { 0.75 };
        let disc = |x: f32, z: f32, rr: f32| [cx + 0.24 * x / rr.max(1e-6), 0.75 + 0.24 * z / rr.max(1e-6)];
        let ids: Vec<u32> = (0..n).map(|i| id(i, ring)).collect();
        match cap {
            Cap::None => {}
            Cap::Ngon => {
                let mut v = ids.clone();
                let mut uv: Vec<[f32; 2]> = (0..n).map(|i| {
                    let a = TAU * i as f32 / n as f32;
                    disc(a.cos() * rad, a.sin() * rad, rad)
                }).collect();
                if !top {
                    v.reverse();
                    uv.reverse();
                }
                p.add_face_like(&v, Some(&uv), 0, 0, true);
            }
            Cap::Fan => {
                let c = p.add_vertex([0.0, y, 0.0]);
                for i in 0..n {
                    let (a, b) = (ids[i], ids[(i + 1) % n]);
                    let (aa, ab) = (TAU * i as f32 / n as f32, TAU * (i + 1) as f32 / n as f32);
                    let (ua, ub, uc) = (disc(aa.cos() * rad, aa.sin() * rad, rad), disc(ab.cos() * rad, ab.sin() * rad, rad), disc(0.0, 0.0, rad));
                    if top {
                        p.add_face_like(&[c, b, a], Some(&[uc, ub, ua]), 0, 0, true);
                    } else {
                        p.add_face_like(&[c, a, b], Some(&[uc, ua, ub]), 0, 0, true);
                    }
                }
            }
            Cap::Quads => {
                let inner_r = rad * 0.55;
                let inner: Vec<u32> = (0..n).map(|i| {
                    let a = TAU * i as f32 / n as f32;
                    p.add_vertex([inner_r * a.cos(), y, inner_r * a.sin()])
                }).collect();
                for i in 0..n {
                    let (aa, ab) = (TAU * i as f32 / n as f32, TAU * (i + 1) as f32 / n as f32);
                    let q = [ids[i], ids[(i + 1) % n], inner[(i + 1) % n], inner[i]];
                    let uv = [disc(aa.cos() * rad, aa.sin() * rad, rad), disc(ab.cos() * rad, ab.sin() * rad, rad), disc(ab.cos() * inner_r, ab.sin() * inner_r, rad), disc(aa.cos() * inner_r, aa.sin() * inner_r, rad)];
                    if top {
                        let mut q = q;
                        let mut uv = uv;
                        q.reverse();
                        uv.reverse();
                        p.add_face_like(&q, Some(&uv), 0, 0, true);
                    } else {
                        p.add_face_like(&q, Some(&uv), 0, 0, true);
                    }
                }
                let mut v = inner.clone();
                let mut uv: Vec<[f32; 2]> = (0..n).map(|i| {
                    let a = TAU * i as f32 / n as f32;
                    disc(a.cos() * inner_r, a.sin() * inner_r, rad)
                }).collect();
                if !top {
                    v.reverse();
                    uv.reverse();
                }
                p.add_face_like(&v, Some(&uv), 0, 0, true);
            }
        }
    }
    p.ensure_materials();
    p.orient_outward();
    p
}

/// A sphere of quads in rings (triangles at the poles).
pub fn uv_sphere(radius: f32, segments: usize, rings: usize) -> Poly {
    let (n, r) = (segments.max(3), rings.max(2));
    let mut p = Poly::new();
    p.uvs = Vec::new();
    let top = p.add_vertex([0.0, -radius, 0.0]);
    for j in 1..r {
        let phi = PI * j as f32 / r as f32;
        for i in 0..n {
            let th = TAU * i as f32 / n as f32;
            p.add_vertex([radius * phi.sin() * th.cos(), -radius * phi.cos(), radius * phi.sin() * th.sin()]);
        }
    }
    let bottom = p.add_vertex([0.0, radius, 0.0]);
    let id = |i: usize, j: usize| 1 + ((j - 1) * n + i % n) as u32;
    let uv = |i: usize, j: usize| [i as f32 / n as f32, j as f32 / r as f32];
    for i in 0..n {
        p.add_face_like(&[top, id(i + 1, 1), id(i, 1)], Some(&[[(i as f32 + 0.5) / n as f32, 0.0], uv(i + 1, 1), uv(i, 1)]), 0, 0, true);
        p.add_face_like(&[bottom, id(i, r - 1), id(i + 1, r - 1)], Some(&[[(i as f32 + 0.5) / n as f32, 1.0], uv(i, r - 1), uv(i + 1, r - 1)]), 0, 0, true);
    }
    for j in 1..r - 1 {
        for i in 0..n {
            p.add_face_like(&[id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)], Some(&[uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)]), 0, 0, true);
        }
    }
    p.ensure_materials();
    p.orient_outward();
    p
}

/// A sphere made from a subdivided cube pushed round: all quads, evenly
/// spread, no poles — the best ball to subdivide or sculpt.
pub fn quad_sphere(radius: f32, segments: usize) -> Poly {
    let mut p = cube([2.0, 2.0, 2.0], segments.max(1));
    p.map_positions(|v| {
        // Equal-area-ish spread (the cube-to-sphere map).
        let (x, y, z) = (v[0], v[1], v[2]);
        let q = [x * (1.0 - y * y / 2.0 - z * z / 2.0 + y * y * z * z / 3.0).max(0.0).sqrt(), y * (1.0 - z * z / 2.0 - x * x / 2.0 + z * z * x * x / 3.0).max(0.0).sqrt(), z * (1.0 - x * x / 2.0 - y * y / 2.0 + x * x * y * y / 3.0).max(0.0).sqrt()];
        scale(normalise(q), radius)
    });
    p
}

/// A ring: `major` from the centre to the middle of the tube, `minor`
/// the tube's radius; flat in the x–z plane.
pub fn torus(major: f32, minor: f32, segments: usize, sides: usize) -> Poly {
    let (n, s) = (segments.max(3), sides.max(3));
    let mut p = Poly::new();
    p.uvs = Vec::new();
    for i in 0..n {
        let a = TAU * i as f32 / n as f32;
        for j in 0..s {
            let b = TAU * j as f32 / s as f32;
            let rr = major + minor * b.cos();
            p.add_vertex([rr * a.cos(), minor * b.sin(), rr * a.sin()]);
        }
    }
    let id = |i: usize, j: usize| ((i % n) * s + j % s) as u32;
    let uv = |i: usize, j: usize| [i as f32 / n as f32, j as f32 / s as f32];
    for i in 0..n {
        for j in 0..s {
            p.add_face_like(&[id(i, j), id(i, j + 1), id(i + 1, j + 1), id(i + 1, j)], Some(&[uv(i, j), uv(i, j + 1), uv(i + 1, j + 1), uv(i + 1, j)]), 0, 0, true);
        }
    }
    p.ensure_materials();
    p.orient_outward();
    p
}

/// A flat disc (one n-gon) facing up.
pub fn circle(radius: f32, segments: usize) -> Poly {
    let n = segments.max(3);
    let mut p = Poly::new();
    let ids: Vec<u32> = (0..n).map(|i| {
        let a = TAU * i as f32 / n as f32;
        p.add_vertex([radius * a.cos(), 0.0, radius * a.sin()])
    }).collect();
    let uv: Vec<[f32; 2]> = (0..n).map(|i| {
        let a = TAU * i as f32 / n as f32;
        [0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()]
    }).collect();
    p.add_face_like(&ids, Some(&uv), 0, 0, true);
    p.ensure_materials();
    p
}

/// A pipe: a cylinder with a hole down its middle.
pub fn tube(outer: f32, inner: f32, height: f32, segments: usize) -> Poly {
    let n = segments.max(3);
    let mut p = Poly::new();
    let ring = |p: &mut Poly, r: f32, y: f32| -> Vec<u32> {
        (0..n).map(|i| {
            let a = TAU * i as f32 / n as f32;
            p.add_vertex([r * a.cos(), y, r * a.sin()])
        }).collect()
    };
    let (ot, ob, it, ib) = (ring(&mut p, outer, -height / 2.0), ring(&mut p, outer, height / 2.0), ring(&mut p, inner, -height / 2.0), ring(&mut p, inner, height / 2.0));
    for i in 0..n {
        let j = (i + 1) % n;
        p.add_face(&[ot[i], ot[j], ob[j], ob[i]], 0);
        p.add_face(&[it[j], it[i], ib[i], ib[j]], 0);
        p.add_face(&[ot[j], ot[i], it[i], it[j]], 0);
        p.add_face(&[ob[i], ob[j], ib[j], ib[i]], 0);
    }
    p.ensure_materials();
    p.orient_outward();
    p
}

/// A cone standing on its base (the tip up, at −height/2).
pub fn cone(radius: f32, height: f32, segments: usize) -> Poly {
    let n = segments.max(3);
    let mut p = Poly::new();
    let tip = p.add_vertex([0.0, -height / 2.0, 0.0]);
    let ring: Vec<u32> = (0..n).map(|i| {
        let a = TAU * i as f32 / n as f32;
        p.add_vertex([radius * a.cos(), height / 2.0, radius * a.sin()])
    }).collect();
    for i in 0..n {
        p.add_face(&[tip, ring[(i + 1) % n], ring[i]], 0);
    }
    let mut base = ring.clone();
    base.reverse();
    p.add_face(&base, 0);
    p.ensure_materials();
    p.orient_outward();
    p
}

/// A sphere of near-even triangles: an icosahedron split `subdivisions`
/// times (1–6).
pub fn ico_sphere(radius: f32, subdivisions: usize) -> Poly {
    let m = studio_mesh::prim::ico_sphere(radius, subdivisions.clamp(1, 6));
    let mut p = Poly::from_mesh(&m);
    p.uvs.clear();
    for s in &mut p.face_smooth {
        *s = true;
    }
    p.orient_outward();
    p.ensure_materials();
    p
}

/// A generic human head (and neck, with eyes), `size` px from chin to
/// crown, face toward −z: a base to model or sculpt a character from.
/// `detail` 1–4 sets how many faces (quads where triangles lie flat
/// together).
pub fn head(size: f32, detail: usize) -> Poly {
    let mut dense = studio_mesh::sdf::head(size, 1);
    let target = [1500, 3000, 6000, 12000][detail.clamp(1, 4) - 1];
    studio_mesh::decimate::decimate(&mut dense, target);
    let mut p = Poly::from_mesh(&dense);
    crate::edit::tris_to_quads(&mut p, 40.0);
    p.uvs.clear();
    for s in &mut p.face_smooth {
        *s = true;
    }
    p.orient_outward();
    p.ensure_materials();
    p
}

/// A round tube along a curve: `kind` "bezier" (an S), "arc", "circle",
/// "helix" or "line", `size` px long, `radius` the tube's, `resolution`
/// samples along it.
pub fn curve(kind: &str, size: f32, radius: f32, resolution: usize) -> Option<Poly> {
    use crate::curves::{Along, BezierKnot, Curve, Profile, sweep};
    let h = size / 2.0;
    let path = match kind {
        "bezier" | "curve" | "s" => Curve::Bezier {
            knots: vec![
                BezierKnot { at: [-h, 0.0, 0.0], before: None, after: Some([-h * 0.4, -h * 0.6, 0.0]) },
                BezierKnot { at: [h, 0.0, 0.0], before: Some([h * 0.4, h * 0.6, 0.0]), after: None },
            ],
            closed: false,
        },
        "arc" => Curve::Circle { radius: h, axis: 2, centre: [0.0; 3], degrees: 180.0 },
        "circle" | "ring" => Curve::Circle { radius: h, axis: 1, centre: [0.0; 3], degrees: 360.0 },
        "helix" | "spring" | "coil" => Curve::Helix { radius: h * 0.4, height: size, turns: 4.0, axis: 1 },
        "line" | "path" | "straight" => Curve::Polyline { points: vec![[-h, 0.0, 0.0], [h, 0.0, 0.0]], closed: false },
        _ => return None,
    };
    let closed = path.closed();
    let mut p = sweep(&path, &Profile::Circle { radius: radius.max(0.01), segments: 12 }, &Along { caps: !closed, ..Along::default() }, resolution.clamp(4, 512));
    for s in &mut p.face_smooth {
        *s = true;
    }
    p.ensure_materials();
    Some(p)
}

/// The curve shapes [`curve`] makes.
pub const CURVES: &[&str] = &["bezier", "arc", "circle", "helix", "line"];

/// The shape names [`by_name`] knows.
pub const NAMES: &[&str] = &["cube", "box", "plane", "grid", "cylinder", "sphere", "uv_sphere", "quad_sphere", "ico_sphere", "torus", "circle", "tube", "cone", "head"];

/// A starting shape by name, `size` across, `detail` segments-ish.
pub fn by_name(name: &str, size: f32, detail: usize) -> Option<Poly> {
    let d = detail.max(1);
    Some(match name {
        "cube" | "box" => cube([size; 3], d.min(64)),
        "plane" | "grid" => plane(size, size, d, d),
        "cylinder" => cylinder(size / 2.0, size / 2.0, size, (8 * d).clamp(8, 128), 1, Cap::Ngon),
        "uv_sphere" => uv_sphere(size / 2.0, (8 * d).clamp(8, 128), (4 * d).clamp(4, 64)),
        "sphere" | "quad_sphere" | "ball" => quad_sphere(size / 2.0, (2 * d).clamp(2, 64)),
        "torus" | "ring" | "donut" => torus(size * 0.35, size * 0.15, (8 * d).clamp(8, 128), (4 * d).clamp(4, 64)),
        "circle" | "disc" | "disk" => circle(size / 2.0, (8 * d).clamp(8, 256)),
        "tube" | "pipe" => tube(size / 2.0, size * 0.4, size, (8 * d).clamp(8, 128)),
        "cone" => cone(size / 2.0, size, (8 * d).clamp(8, 128)),
        "ico_sphere" | "icosphere" => ico_sphere(size / 2.0, d.clamp(1, 6)),
        "head" | "generic_head" => head(size, d.clamp(1, 4)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poly::Topology;
    use studio_mesh::dot;

    fn closed(p: &Poly) -> bool {
        Topology::of(p).boundary_edges().is_empty()
    }

    fn outward(p: &Poly) -> bool {
        let (lo, hi) = p.bounds();
        let c = scale(add(lo, hi), 0.5);
        (0..p.faces()).all(|f| dot(p.face_normal(f), studio_mesh::sub(p.face_centre(f), c)) > -1e-3)
    }

    #[test]
    fn closed_shapes_are_watertight_and_face_out() {
        for (name, p) in [("cube", cube([10.0; 3], 3)), ("cyl", cylinder(5.0, 5.0, 10.0, 16, 2, Cap::Quads)), ("fan", cylinder(5.0, 3.0, 10.0, 12, 1, Cap::Fan)), ("uv", uv_sphere(5.0, 16, 8)), ("quad", quad_sphere(5.0, 4)), ("torus", torus(5.0, 2.0, 16, 8)), ("tube", tube(5.0, 4.0, 3.0, 12)), ("cone", cone(5.0, 8.0, 12))] {
            assert!(closed(&p), "{name} has holes");
            if name != "torus" && name != "tube" {
                assert!(outward(&p), "{name} faces in");
            }
        }
    }

    #[test]
    fn plane_faces_up_and_quad_sphere_is_all_quads() {
        let p = plane(10.0, 10.0, 2, 2);
        assert!(p.face_normal(0)[1] < -0.99);
        let s = quad_sphere(5.0, 3);
        assert!((0..s.faces()).all(|f| s.face(f).len() == 4));
        assert!(s.positions.iter().all(|v| (studio_mesh::length(*v) - 5.0).abs() < 1e-3));
    }
}
