// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Solid pieces the place generators build with: boxes (with a small
//! bevel so edges catch light), prisms from an outline, tubes between two
//! points, and hulls between two rectangles. Positions are Studio pixels
//! with y down; the generators think in "up" heights and [`pt`] flips them.

use crate::poly::Poly;
use std::cell::Cell;
use studio_mesh::{V3, add, cross, dot, length, normalise, scale, sub};

thread_local! {
    /// Every piece gets at least a small bevel (set while a place is built).
    static BEVEL_ALL: Cell<bool> = const { Cell::new(false) };
}

/// Bevel even the smallest pieces from now on (on this thread).
pub fn bevel_everything(on: bool) {
    BEVEL_ALL.with(|b| b.set(on));
}

fn bevel_all() -> bool {
    BEVEL_ALL.with(|b| b.get())
}

pub type V = V3;

/// A point from x, a height above the ground, and z.
pub fn pt(x: f32, up: f32, z: f32) -> V {
    [x, -up, z]
}

fn newell(p: &Poly, ids: &[u32]) -> V {
    let mut n = [0.0f32; 3];
    for i in 0..ids.len() {
        let a = p.positions[ids[i] as usize];
        let b = p.positions[ids[(i + 1) % ids.len()] as usize];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// Adds a face turned to look away from `inside`.
fn put_out(p: &mut Poly, ids: &[u32], inside: V) {
    let n = newell(p, ids);
    let mut c = [0.0f32; 3];
    for &i in ids {
        c = add(c, p.positions[i as usize]);
    }
    c = scale(c, 1.0 / ids.len() as f32);
    if dot(n, sub(c, inside)) < 0.0 {
        let r: Vec<u32> = ids.iter().rev().copied().collect();
        p.add_face(&r, 0);
    } else {
        p.add_face(ids, 0);
    }
}

/// Adds a face whose normal faces `want`.
fn put_dir(p: &mut Poly, ids: &[u32], want: V) {
    if dot(newell(p, ids), want) < 0.0 {
        let r: Vec<u32> = ids.iter().rev().copied().collect();
        p.add_face(&r, 0);
    } else {
        p.add_face(ids, 0);
    }
}

/// A box from `x`, `u` (heights above the ground, low to high) and `z`
/// ranges. `ch`: the bevel in px (`< 0`: chosen from the thinnest side;
/// `0`: none).
pub fn bx(p: &mut Poly, x: [f32; 2], u: [f32; 2], z: [f32; 2], ch: f32) {
    let (x0, x1) = (x[0].min(x[1]), x[0].max(x[1]));
    let (u0, u1) = (u[0].min(u[1]), u[0].max(u[1]));
    let (z0, z1) = (z[0].min(z[1]), z[0].max(z[1]));
    let lo = [x0, -u1, z0];
    let hi = [x1, -u0, z1];
    let thin = (hi[0] - lo[0]).min(hi[1] - lo[1]).min(hi[2] - lo[2]);
    if thin <= 0.01 {
        return;
    }
    let all = bevel_all();
    let ch = if ch < 0.0 {
        if thin < if all { 1.0 } else { 2.0 } { 0.0 } else { (thin * 0.14).clamp(0.4, 5.0) }
    } else if ch == 0.0 && all && thin >= 1.0 {
        (thin * 0.2).clamp(0.3, 1.6)
    } else {
        ch.min(thin * 0.45)
    };
    let centre = scale(add(lo, hi), 0.5);
    if ch <= 0.0 {
        let c = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        let base = p.positions.len() as u32;
        for i in 0..8 {
            p.add_vertex(c(i));
        }
        for q in [[0u32, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
            put_out(p, &[base + q[0], base + q[1], base + q[2], base + q[3]], centre);
        }
        return;
    }
    // Chamfered: three vertices per corner, one on each face that meets there.
    let corner = |sx: usize, sy: usize, sz: usize| sx | (sy << 1) | (sz << 2);
    let outer = |s: usize, a: usize| if s == 0 { lo[a] } else { hi[a] };
    let inset = |s: usize, a: usize| if s == 0 { lo[a] + ch } else { hi[a] - ch };
    let base = p.positions.len() as u32;
    // vertex index: base + corner * 3 + axis (the face of that axis)
    for k in 0..8 {
        let s = [k & 1, (k >> 1) & 1, (k >> 2) & 1];
        for a in 0..3 {
            let v: V = std::array::from_fn(|i| if i == a { outer(s[i], i) } else { inset(s[i], i) });
            p.add_vertex(v);
        }
    }
    let vid = |k: usize, a: usize| base + (k * 3 + a) as u32;
    // Six faces.
    for a in 0..3 {
        for side in 0..2 {
            let ks: Vec<usize> = (0..8).filter(|k| (k >> a) & 1 == side).collect();
            // Order the four corners round the face.
            let (b, c) = match a {
                0 => (1usize, 2usize),
                1 => (0, 2),
                _ => (0, 1),
            };
            let at = |sb: usize, sc: usize| *ks.iter().find(|&&k| (k >> b) & 1 == sb && (k >> c) & 1 == sc).unwrap();
            put_out(p, &[vid(at(0, 0), a), vid(at(1, 0), a), vid(at(1, 1), a), vid(at(0, 1), a)], centre);
        }
    }
    // Twelve edge strips: along axis c between faces of axes a and b.
    for (a, b, c) in [(0usize, 1usize, 2usize), (0, 2, 1), (1, 2, 0)] {
        for sa in 0..2 {
            for sb in 0..2 {
                let mut k0 = [0usize; 3];
                k0[a] = sa;
                k0[b] = sb;
                k0[c] = 0;
                let mut k1 = k0;
                k1[c] = 1;
                let (i0, i1) = (corner(k0[0], k0[1], k0[2]), corner(k1[0], k1[1], k1[2]));
                put_out(p, &[vid(i0, a), vid(i1, a), vid(i1, b), vid(i0, b)], centre);
            }
        }
    }
    // Eight corners.
    for k in 0..8 {
        put_out(p, &[vid(k, 0), vid(k, 1), vid(k, 2)], centre);
    }
}

/// Extrudes an outline (pairs of `a`, `b` in a plane) between two depths.
/// `map(a, b, d)` places a point in the model; it must be linear.
pub fn extrude(target: &mut Poly, outline: &[[f32; 2]], d: [f32; 2], map: impl Fn(f32, f32, f32) -> V) {
    let mut piece = Poly::new();
    let p = &mut piece;
    let n = outline.len();
    if n < 3 {
        return;
    }
    let area: f32 = (0..n).map(|i| outline[i][0] * outline[(i + 1) % n][1] - outline[(i + 1) % n][0] * outline[i][1]).sum();
    if area.abs() < 1e-3 {
        return;
    }
    let ccw = area > 0.0;
    let origin = map(0.0, 0.0, 0.0);
    let towards_d = normalise(sub(map(0.0, 0.0, 1.0), origin));
    let base = p.positions.len() as u32;
    for &dd in &d {
        for o in outline {
            p.add_vertex(map(o[0], o[1], dd));
        }
    }
    let ids0: Vec<u32> = (0..n as u32).map(|i| base + i).collect();
    let ids1: Vec<u32> = (0..n as u32).map(|i| base + n as u32 + i).collect();
    let (lo, hi) = if d[0] <= d[1] { (&ids0, &ids1) } else { (&ids1, &ids0) };
    put_dir(p, lo, scale(towards_d, -1.0));
    put_dir(p, hi, towards_d);
    for i in 0..n {
        let j = (i + 1) % n;
        let (da, db) = (outline[j][0] - outline[i][0], outline[j][1] - outline[i][1]);
        let (na, nb) = if ccw { (db, -da) } else { (-db, da) };
        let want = sub(map(na, nb, 0.0), origin);
        put_dir(p, &[ids0[i], ids0[j], ids1[j], ids1[i]], want);
    }
    finish(target, piece);
}

/// A finished solid piece added to `target`, its hard edges given a small
/// bevel when everything is being bevelled.
fn finish(target: &mut Poly, mut piece: Poly) {
    if bevel_all() && piece.faces() > 0 {
        let (lo, hi) = piece.bounds();
        let thin = (hi[0] - lo[0]).min(hi[1] - lo[1]).min(hi[2] - lo[2]);
        let w = (thin * 0.2).min(1.2);
        if w >= 0.25 {
            let edges = crate::select::Edges::Sharper { degrees: 50.0 }.list(&piece);
            if !edges.is_empty() {
                crate::bevel::bevel(&mut piece, &edges, w, 1, 0.5);
            }
        }
    }
    target.merge(&piece);
}

/// An outline in x and height, extruded along z.
pub fn prism(p: &mut Poly, outline: &[[f32; 2]], z: [f32; 2]) {
    extrude(p, outline, z, |a, b, d| pt(a, b, d));
}

/// An outline in z and height, extruded along x.
pub fn prism_x(p: &mut Poly, outline: &[[f32; 2]], x: [f32; 2]) {
    extrude(p, outline, x, |a, b, d| pt(d, b, a));
}

/// A round bar from `a` to `b` with radii `r0` and `r1`, `segs` sided.
/// Thick bars get a small chamfer at each end so the ends catch light.
pub fn tube(p: &mut Poly, a: V, b: V, r0: f32, r1: f32, segs: usize) {
    let axis = sub(b, a);
    let len = length(axis);
    if len < 0.01 {
        return;
    }
    let w = scale(axis, 1.0 / len);
    let helper = if w[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let u = normalise(cross(w, helper));
    let v = cross(w, u);
    // Eight sides at least: fewer bend more than 60 degrees at every seam.
    let n = segs.max(8);
    let c0 = if r0 >= 1.5 || (bevel_all() && r0 >= 1.0) { (r0 * 0.22).clamp(0.4, 3.0).min(len * 0.2) } else { 0.0 };
    let c1 = if r1 >= 1.5 || (bevel_all() && r1 >= 1.0) { (r1 * 0.22).clamp(0.4, 3.0).min(len * 0.2) } else { 0.0 };
    // Rings: (centre, radius), the first and last are the cap rings.
    let mut rings: Vec<(V, f32)> = Vec::new();
    if c0 > 0.0 {
        rings.push((a, r0 - c0));
        rings.push((add(a, scale(w, c0)), r0));
    } else {
        rings.push((a, r0));
    }
    if c1 > 0.0 {
        rings.push((sub(b, scale(w, c1)), r1));
        rings.push((b, r1 - c1));
    } else {
        rings.push((b, r1));
    }
    let base = p.positions.len() as u32;
    for (c, r) in &rings {
        for i in 0..n {
            let t = std::f32::consts::TAU * i as f32 / n as f32;
            p.add_vertex(add(*c, add(scale(u, r * t.cos()), scale(v, r * t.sin()))));
        }
    }
    let mid = scale(add(a, b), 0.5);
    for k in 0..rings.len() - 1 {
        for i in 0..n {
            let j = (i + 1) % n;
            let q = [base + (k * n + i) as u32, base + (k * n + j) as u32, base + ((k + 1) * n + j) as u32, base + ((k + 1) * n + i) as u32];
            let on_axis = add(a, scale(w, dot(sub(p.positions[q[0] as usize], a), w)));
            // A chamfer strip's outside is away from the axis and a little to its end; the axis point beside it does for both.
            put_out(p, &q, on_axis);
        }
    }
    let last = rings.len() - 1;
    let cap0: Vec<u32> = (0..n as u32).map(|i| base + i).collect();
    let cap1: Vec<u32> = (0..n as u32).map(|i| base + (last * n) as u32 + i).collect();
    if rings[0].1 > 0.01 {
        put_out(p, &cap0, add(mid, scale(w, len)));
    }
    if rings[last].1 > 0.01 {
        put_out(p, &cap1, sub(mid, scale(w, len)));
    }
}

/// A round shaft standing up from `u0` to `u1`.
pub fn column(p: &mut Poly, x: f32, z: f32, u0: f32, u1: f32, r0: f32, r1: f32, segs: usize) {
    tube(p, pt(x, u0, z), pt(x, u1, z), r0, r1, segs);
}

/// A solid between two rectangles (x range, z range) at two heights: a
/// frustum for hip roofs, chimney caps, setbacks.
pub fn hull(target: &mut Poly, bottom: ([f32; 2], [f32; 2]), top: ([f32; 2], [f32; 2]), u: [f32; 2]) {
    let mut piece = Poly::new();
    let p = &mut piece;
    let c = |r: &([f32; 2], [f32; 2]), up: f32, i: usize| pt(if i & 1 == 0 { r.0[0] } else { r.0[1] }, up, if i & 2 == 0 { r.1[0] } else { r.1[1] });
    let base = p.positions.len() as u32;
    for i in 0..4 {
        p.add_vertex(c(&bottom, u[0], i));
    }
    for i in 0..4 {
        p.add_vertex(c(&top, u[1], i));
    }
    let centre = scale(add(c(&bottom, u[0], 0), c(&top, u[1], 3)), 0.5);
    for q in [[0u32, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
        let ids = [base + q[0], base + q[1], base + q[2], base + q[3]];
        // A ridge makes a quad with two equal corners: drop the repeat.
        let mut keep: Vec<u32> = Vec::new();
        for &i in &ids {
            if keep.iter().all(|&k| length(sub(p.positions[k as usize], p.positions[i as usize])) > 0.01) {
                keep.push(i);
            }
        }
        if keep.len() >= 3 {
            put_out(p, &keep, centre);
        }
    }
    finish(target, piece);
}

/// A half or whole ball (dome, finial, tank end).
pub fn ball(p: &mut Poly, c: V, r: f32, squash: f32, segs: usize) {
    let mut s = crate::prim::uv_sphere(r, segs.max(6), (segs / 2).max(4));
    s.map_positions(|v| add([v[0], v[1] * squash, v[2]], c));
    s.single_material();
    p.merge(&s);
}
