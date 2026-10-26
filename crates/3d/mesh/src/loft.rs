// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parametric surfaces: a grid of points from a formula, made into a
//! smooth surface. Hats, collars, skirts, capes, belts, scarves and straps
//! are all surfaces of revolution or lofts; a formula gives each its shape
//! and this module gives it triangles, normals and texture coordinates.

use crate::{Material, Mesh, V3, add, cross, dot, length, normalise, scale, sub};

/// A surface over `nu` × `nv` points: `f(u, v)` with `u` and `v` from 0 to 1
/// (`u` goes round, `v` along). With `wrap`, the last column joins the
/// first. The winding is such that the front is the side `outward` points
/// to at the middle of the surface: `outward(p)` is a point-wise direction
/// (away from an axis, say).
pub fn surface(nu: usize, nv: usize, wrap: bool, f: impl Fn(f32, f32) -> V3, outward: impl Fn(V3) -> V3) -> Mesh {
    let (nu, nv) = (nu.max(2), nv.max(2));
    let cols = if wrap { nu } else { nu };
    let mut m = Mesh::default();
    for j in 0..nv {
        for i in 0..cols {
            let u = if wrap { i as f32 / nu as f32 } else { i as f32 / (nu - 1) as f32 };
            let v = j as f32 / (nv - 1) as f32;
            m.positions.push(f(u, v));
            m.uvs.push([u, v]);
        }
    }
    let idx = |i: usize, j: usize| (j * cols + (i % cols)) as u32;
    let last = if wrap { cols } else { cols - 1 };
    for j in 0..nv - 1 {
        for i in 0..last {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    // Wind the front outward.
    let mut votes = 0.0;
    for t in 0..m.indices.len() / 3 {
        let (a, b, c) = (m.positions[m.indices[3 * t] as usize], m.positions[m.indices[3 * t + 1] as usize], m.positions[m.indices[3 * t + 2] as usize]);
        let n = cross(sub(b, a), sub(c, a));
        let centre = scale(add(add(a, b), c), 1.0 / 3.0);
        votes += dot(n, outward(centre));
    }
    if votes < 0.0 {
        for t in m.indices.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
    m.materials = vec![Material::default()];
    m.normals = vec![[0.0; 3]; m.positions.len()];
    crate::ops::smooth_normals_in_place(&mut m);
    m.single_group();
    m
}

/// A surface of revolution about the vertical line through `axis`: the
/// profile is `(radius, y)` pairs from the top down (y grows downward).
/// `segments` round. Closed at the top when the first radius is 0.
pub fn lathe(axis: V3, profile: &[(f32, f32)], segments: usize) -> Mesh {
    let n = profile.len().max(2);
    let seg = segments.max(6);
    surface(
        seg,
        n,
        true,
        |u, v| {
            let k = (v * (n - 1) as f32).round() as usize;
            let (r, y) = profile[k.min(n - 1)];
            let a = u * std::f32::consts::TAU;
            [axis[0] + r * a.cos(), axis[1] + y, axis[2] + r * a.sin()]
        },
        move |p| [p[0] - axis[0], 0.0, p[2] - axis[2]],
    )
}

/// A tube along `path` with a `radius` at each point, `sides` round,
/// capped ends rounded off by shrinking the first and last rings.
pub fn tube(path: &[V3], radius: &[f32], sides: usize) -> Mesh {
    let n = path.len().max(2);
    let sides = sides.max(3);
    // A frame along the path: tangent, and two normals from a reference.
    let tangent = |i: usize| normalise(sub(path[(i + 1).min(n - 1)], path[i.saturating_sub(1)]));
    let frames: Vec<(V3, V3)> = (0..n)
        .map(|i| {
            let t = tangent(i);
            let reference = if t[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
            let a = normalise(cross(t, reference));
            (a, normalise(cross(t, a)))
        })
        .collect();
    let centre = scale(path.iter().fold([0.0; 3], |s, p| add(s, *p)), 1.0 / n as f32);
    let m = surface(
        sides,
        n,
        true,
        |u, v| {
            let i = ((v * (n - 1) as f32).round() as usize).min(n - 1);
            let a = u * std::f32::consts::TAU;
            let r = radius.get(i).copied().unwrap_or(1.0);
            add(path[i], add(scale(frames[i].0, r * a.cos()), scale(frames[i].1, r * a.sin())))
        },
        |p| sub(p, centre),
    );
    m
}

/// A closed band round the vertical line through `axis`: `radius(angle)` at
/// each angle (radians, 0 along +x, towards +z), `y` its middle, `height` its
/// width, thickness `thick` outward.
pub fn band(axis: V3, y: f32, height: f32, thick: f32, radius: impl Fn(f32) -> f32, segments: usize) -> Mesh {
    let seg = segments.max(8);
    surface(
        seg,
        4,
        true,
        |u, v| {
            let a = u * std::f32::consts::TAU;
            // A rounded profile: outer face between two lips.
            let (r_off, y_off) = [(0.0, -0.5), (thick, -0.5), (thick, 0.5), (0.0, 0.5)][((v * 3.0).round() as usize).min(3)];
            let r = radius(a) + r_off;
            [axis[0] + r * a.cos(), y + y_off * height, axis[2] + r * a.sin()]
        },
        move |p| [p[0] - axis[0], 0.0, p[2] - axis[2]],
    )
}

/// Moves a mesh's vertices (a closure from position to position) and
/// refreshes its normals.
pub fn deform(m: &mut Mesh, f: impl Fn(V3) -> V3) {
    for p in &mut m.positions {
        *p = f(*p);
    }
    crate::ops::smooth_normals_in_place(m);
}

/// Appends `other` to `m`, its triangles taking material `mat` of `m`.
pub fn append(m: &mut Mesh, other: &Mesh, mat: u32) {
    let mut mats = m.tri_materials();
    mats.extend(std::iter::repeat_n(mat, other.triangles()));
    let base = m.positions.len() as u32;
    m.positions.extend_from_slice(&other.positions);
    m.normals.extend_from_slice(&other.normals);
    m.uvs.extend_from_slice(&other.uvs);
    m.indices.extend(other.indices.iter().map(|i| i + base));
    m.regroup(&mats);
}

/// Total length of a path.
pub fn path_length(p: &[V3]) -> f32 {
    p.windows(2).map(|w| length(sub(w[1], w[0]))).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lathe_is_a_closed_round_surface_facing_out() {
        let m = lathe([0.0; 3], &[(0.0, 0.0), (10.0, 2.0), (10.0, 20.0), (0.0, 22.0)], 24);
        assert!(m.triangles() > 100);
        // Outward: the side wall faces away from the axis.
        let mid = (0..m.triangles()).find(|&t| (m.centroid(t)[1] - 11.0).abs() < 4.0).unwrap();
        let c = m.centroid(mid);
        assert!(dot(m.face_normal(mid), normalise([c[0], 0.0, c[2]])) > 0.5);
    }

    #[test]
    fn a_tube_follows_its_path() {
        let path: Vec<V3> = (0..10).map(|i| [i as f32 * 5.0, (i as f32 * 0.5).sin() * 4.0, 0.0]).collect();
        let m = tube(&path, &[1.5; 10], 8);
        let (lo, hi) = m.bounds();
        assert!(hi[0] - lo[0] > 44.0 && hi[1] - lo[1] > 3.0);
    }
}
