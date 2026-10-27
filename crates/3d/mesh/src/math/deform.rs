// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Space warps for any model: every point moves by a function of where it
//! is, so the triangles, materials, texture coordinates and rig weights
//! stay as they were. Normals follow the warp exactly (by its Jacobian),
//! so sharp edges stay sharp and smooth ones smooth.
//!
//! Axes here are the model's own (Studio's: x right, y down, z away), and
//! distances pixels; a formula warp works in formula space (z up) with the
//! model's largest half side as 1. A coarse model is refined first
//! ([`refine`]) so a bend has vertices to bend.

use super::{Program, from_studio, to_studio};
use crate::{Mesh, V3, add, cross, length, normalise, scale, sub};
use std::collections::HashMap;

/// The names a formula warp reads; it gives the offset `[dx, dy, dz]`.
pub const NAMES: &[&str] = &["x", "y", "z", "t", "r", "i"];

/// Moves every vertex by `f`, the normals following.
pub fn warp(m: &mut Mesh, f: &(dyn Fn(V3) -> V3 + Sync)) {
    let (lo, hi) = m.bounds();
    let eps = (length(sub(hi, lo)) * 1e-4).max(1e-4);
    if m.normals.len() != m.positions.len() {
        crate::ops::smooth_normals_in_place(m);
    }
    let moved: Vec<(V3, V3)> = m
        .positions
        .iter()
        .zip(&m.normals)
        .map(|(p, n)| {
            let col = |k: usize| {
                let mut a = *p;
                let mut b = *p;
                a[k] += eps;
                b[k] -= eps;
                scale(sub(f(a), f(b)), 0.5 / eps)
            };
            let (a, b, c) = (col(0), col(1), col(2));
            // The cofactor of the Jacobian turns normals: (J⁻¹)ᵀ up to scale.
            let nn = add(add(scale(cross(b, c), n[0]), scale(cross(c, a), n[1])), scale(cross(a, b), n[2]));
            (f(*p), if length(nn) > 1e-20 { normalise(nn) } else { *n })
        })
        .collect();
    for (k, (p, n)) in moved.into_iter().enumerate() {
        m.positions[k] = p;
        m.normals[k] = n;
    }
}

/// An axis by name: x, y or z (y is up and down).
pub fn axis(s: &str) -> Option<usize> {
    Some(match s {
        "x" | "across" | "sideways" => 0,
        "y" | "up" | "vertical" | "height" => 1,
        "z" | "depth" | "forward" => 2,
        _ => return None,
    })
}

fn others(a: usize) -> (usize, usize) {
    match a {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    }
}

fn centre(m: &Mesh) -> (V3, V3, V3) {
    let (lo, hi) = m.bounds();
    (scale(add(lo, hi), 0.5), lo, hi)
}

/// Twists `degrees` from one end of the model to the other about `axis`
/// through its middle (the middle stays still, the ends turn half each way).
pub fn twist(m: &mut Mesh, axis: usize, degrees: f32) {
    let (c, lo, hi) = centre(m);
    let len = (hi[axis] - lo[axis]).max(1e-6);
    let (a, b) = others(axis);
    let k = degrees.to_radians() / len;
    warp(m, &|p| {
        let ang = (p[axis] - c[axis]) * k;
        let (s, co) = ang.sin_cos();
        let (x, y) = (p[a] - c[a], p[b] - c[b]);
        let mut q = p;
        q[a] = c[a] + x * co - y * s;
        q[b] = c[b] + x * s + y * co;
        q
    });
}

/// Bends the model along `axis` into an arc of `degrees`, curling towards
/// `toward` (another axis, its positive side; negative degrees curl the
/// other way). The middle stays put.
pub fn bend(m: &mut Mesh, axis: usize, toward: usize, degrees: f32) {
    let (c, lo, hi) = centre(m);
    let len = (hi[axis] - lo[axis]).max(1e-6);
    let theta = degrees.to_radians();
    if theta.abs() < 1e-6 || axis == toward {
        return;
    }
    let r = len / theta;
    warp(m, &|p| {
        let s = p[axis] - c[axis];
        let d = p[toward] - c[toward];
        let a = s / r;
        let mut q = p;
        q[axis] = c[axis] + (r - d) * a.sin();
        q[toward] = c[toward] + r - (r - d) * a.cos();
        q
    });
}

/// Scales the model across `axis`: by `from` at its low end (smallest
/// coordinate: for y, the top) to `to` at its high end.
pub fn taper(m: &mut Mesh, axis: usize, from: f32, to: f32) {
    let (c, lo, hi) = centre(m);
    let len = (hi[axis] - lo[axis]).max(1e-6);
    let (a, b) = others(axis);
    warp(m, &|p| {
        let k = from + (to - from) * (p[axis] - lo[axis]) / len;
        let mut q = p;
        q[a] = c[a] + (p[a] - c[a]) * k;
        q[b] = c[b] + (p[b] - c[b]) * k;
        q
    });
}

/// Pushes every point by smooth noise up to about `amount` px, its
/// features about `size` px across: a wobble through the whole form,
/// not only along the surface.
pub fn noise(m: &mut Mesh, amount: f32, size: f32, seed: u32) {
    let k = 1.0 / size.max(1e-3);
    warp(m, &|p| {
        let q = scale(p, k);
        let d = [crate::ops::fbm(q, 3, seed), crate::ops::fbm(q, 3, seed.wrapping_add(17)), crate::ops::fbm(q, 3, seed.wrapping_add(41))];
        add(p, scale(d, amount * 2.0))
    });
}

/// Moves every point by a formula's offset (`[dx, dy, dz]` over [`NAMES`],
/// one formula for all three or three), in formula space with the model's
/// largest half side as 1.
pub fn formula(m: &mut Mesh, parts: &[Program], t: f64) {
    let (c, lo, hi) = centre(m);
    let half = ((hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]) * 0.5).max(1e-6) as f64;
    warp(m, &|p| {
        let q = from_studio(sub(p, c)).map(|v| v / half);
        let r = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
        let at = |k: usize| parts[k.min(parts.len() - 1)].eval(&[q[0], q[1], q[2], t, r, k as f64]);
        let d = [at(0), at(1), at(2)].map(|v| if v.is_finite() { v * half } else { 0.0 });
        add(p, to_studio(d))
    });
}

/// Splits every triangle into four (straight, the shape kept) until no
/// edge is longer than `edge` px or the next split would pass
/// `max_triangles`; how many times it split.
pub fn refine(m: &mut Mesh, edge: f32, max_triangles: usize) -> usize {
    let mut n = 0;
    loop {
        let longest = (0..m.triangles()).flat_map(|t| {
            let [a, b, c] = m.tri(t).map(|v| m.positions[v]);
            [length(sub(a, b)), length(sub(b, c)), length(sub(c, a))]
        });
        if longest.fold(0.0f32, f32::max) <= edge || m.triangles() * 4 > max_triangles || n >= 6 {
            return n;
        }
        split(m);
        n += 1;
    }
}

fn split(m: &mut Mesh) {
    let mats = m.tri_materials();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut idx = Vec::with_capacity(m.indices.len() * 4);
    let mut out_mats = Vec::with_capacity(mats.len() * 4);
    let has_uv = m.uvs.len() == m.positions.len();
    let has_n = m.normals.len() == m.positions.len();
    for t in 0..m.triangles() {
        let v = [m.indices[3 * t], m.indices[3 * t + 1], m.indices[3 * t + 2]];
        let mut e = [0u32; 3];
        for k in 0..3 {
            let (a, b) = (v[k], v[(k + 1) % 3]);
            e[k] = *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let (pa, pb) = (m.positions[a as usize], m.positions[b as usize]);
                m.positions.push(scale(add(pa, pb), 0.5));
                if has_n {
                    let n = normalise(add(m.normals[a as usize], m.normals[b as usize]));
                    m.normals.push(n);
                }
                if has_uv {
                    let (ua, ub) = (m.uvs[a as usize], m.uvs[b as usize]);
                    m.uvs.push([(ua[0] + ub[0]) * 0.5, (ua[1] + ub[1]) * 0.5]);
                }
                (m.positions.len() - 1) as u32
            });
        }
        idx.extend_from_slice(&[v[0], e[0], e[2], e[0], v[1], e[1], e[2], e[1], v[2], e[0], e[1], e[2]]);
        out_mats.extend_from_slice(&[mats[t]; 4]);
    }
    m.indices = idx;
    m.regroup(&out_mats);
}

/// Copies of the model on a grid, `counts` along x, y and z, `spacing` px
/// between their middles, the whole grid centred where the model was.
pub fn repeat(m: &mut Mesh, counts: [usize; 3], spacing: V3) {
    let one = m.clone();
    let mut out = Mesh { materials: one.materials.clone(), ..Default::default() };
    let mut mats = Vec::new();
    for k in 0..counts[2].max(1) {
        for j in 0..counts[1].max(1) {
            for i in 0..counts[0].max(1) {
                let off = [0, 1, 2].map(|a| ([i, j, k][a] as f32 - (counts[a].max(1) - 1) as f32 * 0.5) * spacing[a]);
                append(&mut out, &mut mats, &one, &|p| add(p, off), &|n| n);
            }
        }
    }
    out.regroup(&mats);
    *m = out;
}

/// `count` copies round `axis` (through the origin), each turned to face
/// out as the first does, `radius` px out from the axis.
pub fn around(m: &mut Mesh, count: usize, radius: f32, axis: usize) {
    let one = m.clone();
    let (a, b) = others(axis);
    let mut out = Mesh { materials: one.materials.clone(), ..Default::default() };
    let mut mats = Vec::new();
    for k in 0..count.max(1) {
        let ang = std::f32::consts::TAU * k as f32 / count.max(1) as f32;
        let (s, c) = ang.sin_cos();
        let turn = |p: V3| {
            let mut q = p;
            q[a] = p[a] * c - p[b] * s;
            q[b] = p[a] * s + p[b] * c;
            q
        };
        let mut out_at = [0.0; 3];
        out_at[a] = radius;
        append(&mut out, &mut mats, &one, &|p| turn(add(p, out_at)), &turn);
    }
    out.regroup(&mats);
    *m = out;
}

fn append(out: &mut Mesh, mats: &mut Vec<u32>, one: &Mesh, at: &dyn Fn(V3) -> V3, turn: &dyn Fn(V3) -> V3) {
    let base = out.positions.len() as u32;
    out.positions.extend(one.positions.iter().map(|p| at(*p)));
    out.normals.extend(one.normals.iter().map(|n| turn(*n)));
    out.uvs.extend_from_slice(&one.uvs);
    out.indices.extend(one.indices.iter().map(|i| i + base));
    mats.extend(one.tri_materials());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{compile, open_edges, volume};

    fn column() -> Mesh {
        let mut m = crate::prim::cube([60.0, 200.0, 60.0]);
        refine(&mut m, 12.0, 400_000);
        m
    }

    #[test]
    fn a_twist_keeps_the_triangles_the_volume_and_each_point_s_distance_from_the_axis() {
        let before = column();
        let mut m = before.clone();
        twist(&mut m, 1, 120.0);
        assert_eq!(m.indices, before.indices);
        assert_eq!(m.positions.len(), before.positions.len());
        for (a, b) in m.positions.iter().zip(&before.positions) {
            assert!((a[1] - b[1]).abs() < 1e-3, "a twist about y keeps y");
            assert!((a[0].hypot(a[2]) - b[0].hypot(b[2])).abs() < 1e-2);
        }
        assert!(((volume(&m) - volume(&before)) / volume(&before)).abs() < 0.01, "a twist keeps the volume");
        assert_eq!(open_edges(&m), (0, 0));
        // A top corner turned 60 degrees; the middle did not move.
        let top = m.positions.iter().zip(&before.positions).find(|(_, b)| b[1] < -99.0 && b[0] > 29.0 && b[2] > 29.0).unwrap();
        let ang = top.0[2].atan2(top.0[0]) - top.1[2].atan2(top.1[0]);
        assert!((ang.to_degrees().abs() - 60.0).abs() < 0.5, "{}", ang.to_degrees());
        // Normals stay unit and still face out of the column.
        assert!(m.normals.iter().all(|n| (length(*n) - 1.0).abs() < 1e-3));
        // No twist is no change.
        let mut same = before.clone();
        twist(&mut same, 1, 0.0);
        assert!(same.positions.iter().zip(&before.positions).all(|(a, b)| length(sub(*a, *b)) < 1e-4));
    }

    #[test]
    fn a_bend_curls_the_length_into_an_arc_and_a_taper_narrows_one_end() {
        let before = column();
        let mut m = before.clone();
        bend(&mut m, 1, 0, 90.0);
        assert_eq!(m.indices, before.indices);
        // The axis line keeps its length: a quarter circle of 200 px.
        let end = |y: f32| {
            let at: Vec<V3> = m.positions.iter().zip(&before.positions).filter(|(_, b)| (b[1] - y).abs() < 0.1).map(|(a, _)| *a).collect();
            scale(at.iter().fold([0.0; 3], |s, p| add(s, *p)), 1.0 / at.len() as f32)
        };
        // A quarter circle of 200 px spans 2R sin 45° = 180 px.
        let chord = length(sub(end(-100.0), end(100.0)));
        assert!((chord - 180.0).abs() < 3.0, "{chord}");
        assert_eq!(open_edges(&m), (0, 0));
        let mut t = before.clone();
        taper(&mut t, 1, 0.25, 1.0);
        let (lo, hi) = t.bounds();
        let width_at = |y: f32| t.positions.iter().filter(|p| (p[1] - y).abs() < 1.0).map(|p| p[0].abs()).fold(0.0f32, f32::max);
        assert!((width_at(lo[1]) - 7.5).abs() < 0.5 && (width_at(hi[1]) - 30.0).abs() < 0.5, "{} {}", width_at(lo[1]), width_at(hi[1]));
    }

    #[test]
    fn a_formula_warp_moves_by_its_offset_and_noise_is_bounded() {
        let before = column();
        let mut m = before.clone();
        // Up (formula z) by a tenth of the half side: 10 px, everywhere.
        formula(&mut m, &[compile("w", "[0, 0, 0.1]", NAMES).unwrap()], 0.0);
        assert!(m.positions.iter().zip(&before.positions).all(|(a, b)| (a[1] - (b[1] - 10.0)).abs() < 1e-3));
        let mut n = before.clone();
        noise(&mut n, 8.0, 50.0, 1);
        let most = n.positions.iter().zip(&before.positions).map(|(a, b)| length(sub(*a, *b))).fold(0.0f32, f32::max);
        assert!(most > 1.0 && most < 8.0 * 2.0 * 1.8, "{most}");
        assert_eq!(open_edges(&n), (0, 0));
    }

    #[test]
    fn repeats_make_copies_in_a_grid_and_a_ring() {
        let one = crate::prim::cube([10.0; 3]);
        let mut g = one.clone();
        repeat(&mut g, [3, 1, 2], [20.0, 0.0, 30.0]);
        assert_eq!(g.triangles(), one.triangles() * 6);
        let (lo, hi) = g.bounds();
        assert!((hi[0] - lo[0] - 50.0).abs() < 1e-3 && (hi[2] - lo[2] - 40.0).abs() < 1e-3);
        let mut r = one.clone();
        around(&mut r, 8, 100.0, 1);
        assert_eq!(r.triangles(), one.triangles() * 8);
        assert!(r.positions.iter().all(|p| (p[0].hypot(p[2]) - 100.0).abs() < 10.0));
        assert_eq!(open_edges(&r), (0, 0));
    }

    #[test]
    fn refining_keeps_the_shape_and_its_seams() {
        let mut m = crate::prim::cube([100.0; 3]);
        let v = volume(&m);
        let n = refine(&mut m, 10.0, 1_000_000);
        assert!(n >= 3 && m.triangles() >= 12 * 64);
        assert!((volume(&m) - v).abs() < 1e-1 && open_edges(&m) == (0, 0));
        let mut big = crate::prim::cube([100.0; 3]);
        assert_eq!(refine(&mut big, 0.001, 40), 0, "the cap holds");
    }
}
