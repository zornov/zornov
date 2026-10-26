// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Primitive shapes, centred on the origin, sized in canvas pixels, every
//! triangle's normal pointing outwards. Curved shapes have smooth normals;
//! flat sides (a cube, caps) have their own.

use crate::{Mesh, V3, add, cross, normalise, scale, sub};
use std::f32::consts::{PI, TAU};

/// Builds a mesh from a grid of `(u, v)` samples: `f(u, v)` gives the
/// position and normal; `u` wraps when `wrap_u`.
fn grid(nu: usize, nv: usize, mut f: impl FnMut(f32, f32) -> (V3, V3)) -> Mesh {
    let mut m = Mesh::default();
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f32 / nu as f32, j as f32 / nv as f32);
            let (p, n) = f(u, v);
            m.positions.push(p);
            m.normals.push(n);
            m.uvs.push([u, v]);
        }
    }
    let row = nu as u32 + 1;
    for j in 0..nv as u32 {
        for i in 0..nu as u32 {
            let (a, b, c, d) = (j * row + i, j * row + i + 1, (j + 1) * row + i, (j + 1) * row + i + 1);
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    orient_outwards(&mut m);
    m.single_group();
    m
}

/// Flips triangles whose face normal disagrees with their vertex normals
/// (so every primitive is wound outwards whatever grid order made it).
fn orient_outwards(m: &mut Mesh) {
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t);
        let face = cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a]));
        let vn = add(add(m.normals[a], m.normals[b]), m.normals[c]);
        if crate::dot(face, vn) < 0.0 {
            m.indices.swap(3 * t + 1, 3 * t + 2);
        }
    }
}

/// A sphere made of rings and segments (texture coordinates wrap around).
pub fn uv_sphere(radius: f32, segments: usize, rings: usize) -> Mesh {
    let (s, r) = (segments.max(3), rings.max(2));
    let mut m = grid(s, r, |u, v| {
        let (th, ph) = (u * TAU, v * PI);
        let n = [ph.sin() * th.cos(), -ph.cos(), ph.sin() * th.sin()];
        (scale(n, radius), n)
    });
    drop_degenerate(&mut m);
    m
}

/// A sphere of near-equal triangles (best for sculpting): an icosahedron
/// split `level` times.
pub fn ico_sphere(radius: f32, level: usize) -> Mesh {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut pos: Vec<V3> = [[-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0], [0.0, -1.0, t], [0.0, 1.0, t], [0.0, -1.0, -t], [0.0, 1.0, -t], [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0]].iter().map(|p| normalise(*p)).collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    for _ in 0..level.min(7) {
        let mut mid: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut half = |a: u32, b: u32, pos: &mut Vec<V3>| -> u32 {
            let k = (a.min(b), a.max(b));
            *mid.entry(k).or_insert_with(|| {
                pos.push(normalise(add(pos[a as usize], pos[b as usize])));
                (pos.len() - 1) as u32
            })
        };
        for [a, b, c] in tris {
            let (ab, bc, ca) = (half(a, b, &mut pos), half(b, c, &mut pos), half(c, a, &mut pos));
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    let mut m = Mesh::default();
    for p in &pos {
        m.positions.push(scale(*p, radius));
        m.normals.push(*p);
        m.uvs.push([0.5 + p[2].atan2(p[0]) / TAU, 0.5 + p[1].asin() / PI]);
    }
    m.indices = tris.into_iter().flatten().collect();
    orient_outwards(&mut m);
    m.single_group();
    m
}

/// A box `size` wide, high and deep, each face with its own normal and
/// the whole picture as its texture.
pub fn cube(size: V3) -> Mesh {
    let h = scale(size, 0.5);
    let mut m = Mesh::default();
    // (normal, u axis, v axis)
    let faces: [(V3, V3, V3); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    for (n, u, v) in faces {
        let base = m.positions.len() as u32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let p: V3 = std::array::from_fn(|i| (n[i] + u[i] * su + v[i] * sv) * h[i]);
            m.positions.push(p);
            m.normals.push(n);
            m.uvs.push([(su + 1.0) / 2.0, (sv + 1.0) / 2.0]);
        }
        m.indices.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    }
    orient_outwards(&mut m);
    m.single_group();
    m
}

/// A disc cap at height `y` facing `ny` (±1), `segments` around.
fn cap(m: &mut Mesh, radius: f32, y: f32, ny: f32, segments: usize) {
    let centre = m.positions.len() as u32;
    m.positions.push([0.0, y, 0.0]);
    m.normals.push([0.0, ny, 0.0]);
    m.uvs.push([0.5, 0.5]);
    for i in 0..=segments {
        let th = i as f32 / segments as f32 * TAU;
        m.positions.push([radius * th.cos(), y, radius * th.sin()]);
        m.normals.push([0.0, ny, 0.0]);
        m.uvs.push([0.5 + 0.5 * th.cos(), 0.5 + 0.5 * th.sin()]);
    }
    for i in 0..segments as u32 {
        m.indices.extend_from_slice(&[centre, centre + 1 + i, centre + 2 + i]);
    }
}

/// A cylinder (or, with different radii, a cone or frustum) along y,
/// `height` tall, with caps.
pub fn cylinder(radius_top: f32, radius_bottom: f32, height: f32, segments: usize) -> Mesh {
    let s = segments.max(3);
    let slope = (radius_bottom - radius_top) / height.max(1e-6);
    let mut m = grid(s, 1, |u, v| {
        let th = u * TAU;
        let r = radius_top + (radius_bottom - radius_top) * v;
        let y = -height / 2.0 + height * v;
        // y is down: the top (v = 0) is at -height/2.
        (([r * th.cos(), y, r * th.sin()]), normalise([th.cos(), -slope, th.sin()]))
    });
    if radius_top > 1e-6 {
        cap(&mut m, radius_top, -height / 2.0, -1.0, s);
    }
    if radius_bottom > 1e-6 {
        cap(&mut m, radius_bottom, height / 2.0, 1.0, s);
    }
    orient_outwards(&mut m);
    m.single_group();
    m
}

pub fn cone(radius: f32, height: f32, segments: usize) -> Mesh {
    cylinder(0.0, radius, height, segments)
}

/// A ring: `major` from the centre to the tube's middle, `minor` the
/// tube's radius; it lies flat (around y).
pub fn torus(major: f32, minor: f32, segments: usize, sides: usize) -> Mesh {
    grid(segments.max(3), sides.max(3), |u, v| {
        let (th, ph) = (u * TAU, v * TAU);
        let n = [ph.cos() * th.cos(), ph.sin(), ph.cos() * th.sin()];
        let c = [major * th.cos(), 0.0, major * th.sin()];
        (add(c, scale(n, minor)), n)
    })
}

/// A cylinder with round ends: `height` from end to end.
pub fn capsule(radius: f32, height: f32, segments: usize, rings: usize) -> Mesh {
    let half = (height / 2.0 - radius).max(0.0);
    let r = rings.max(2) * 2;
    let mut m = grid(segments.max(3), r + 1, |u, v| {
        let th = u * TAU;
        // v in 0..1 covers top hemisphere, the straight part, the bottom.
        let k = v * (r + 1) as f32;
        let (ph, y_off) = if k <= r as f32 / 2.0 {
            (k / r as f32 * PI, -half)
        } else if k >= r as f32 / 2.0 + 1.0 {
            ((k - 1.0) / r as f32 * PI, half)
        } else {
            (PI / 2.0, -half + 2.0 * half * (k - r as f32 / 2.0))
        };
        let n = [ph.sin() * th.cos(), -ph.cos(), ph.sin() * th.sin()];
        (add(scale(n, radius), [0.0, y_off, 0.0]), n)
    });
    drop_degenerate(&mut m);
    m
}

/// A flat grid `width` × `depth` lying in x/z (facing up, -y), split
/// `subdivisions` times each way: a floor, or a sheet to sculpt.
pub fn plane(width: f32, depth: f32, subdivisions: usize) -> Mesh {
    let n = subdivisions.max(1);
    grid(n, n, |u, v| (([(u - 0.5) * width, 0.0, (v - 0.5) * depth]), [0.0, -1.0, 0.0]))
}

/// Removes zero-area triangles (sphere poles).
fn drop_degenerate(m: &mut Mesh) {
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t);
        let area = crate::length(cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a])));
        if area > 1e-6 {
            idx.extend_from_slice(&[a as u32, b as u32, c as u32]);
        }
    }
    m.indices = idx;
    m.single_group();
}

/// A primitive by name, for ops and menus: `size` is the main dimension
/// (diameter, edge length or height) in pixels.
pub fn by_name(name: &str, size: f32, detail: usize) -> Option<Mesh> {
    let d = detail.clamp(1, 6);
    Some(match name.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "sphere" | "uv_sphere" | "ball" => uv_sphere(size / 2.0, 16 * d, 8 * d),
        "ico_sphere" | "icosphere" | "sculpt_sphere" | "head_base" => ico_sphere(size / 2.0, d),
        "cube" | "box" => cube([size; 3]),
        "cylinder" | "tube" => cylinder(size / 4.0, size / 4.0, size, 16 * d),
        "cone" => cone(size / 3.0, size, 16 * d),
        "torus" | "ring" | "donut" => torus(size * 0.35, size * 0.12, 24 * d, 12 * d),
        "capsule" | "pill" => capsule(size / 4.0, size, 16 * d, 4 * d),
        "plane" | "floor" | "grid" | "sheet" => plane(size, size, 8 * d),
        _ => return None,
    })
}

pub const NAMES: &[&str] = &["sphere", "ico_sphere", "cube", "cylinder", "cone", "torus", "capsule", "plane"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dot;

    /// Every face points away from the centre (outwards) on closed shapes.
    #[test]
    fn primitives_face_outwards_and_are_sized() {
        for name in ["sphere", "ico_sphere", "cube", "cylinder", "torus", "capsule"] {
            let m = by_name(name, 200.0, 2).unwrap();
            assert!(m.triangles() > 10, "{name}");
            let mut outward = 0;
            for t in 0..m.triangles() {
                let c = m.centroid(t);
                let radial = if name == "torus" { sub(c, scale(normalise([c[0], 0.0, c[2]]), 70.0)) } else { c };
                if dot(m.face_normal(t), radial) > 0.0 {
                    outward += 1;
                }
            }
            assert!(outward as f32 >= 0.98 * m.triangles() as f32, "{name}: {outward} of {}", m.triangles());
            let (lo, hi) = m.bounds();
            assert!(hi[0] - lo[0] > 50.0 && hi[0] - lo[0] <= 201.0, "{name} {lo:?} {hi:?}");
            assert_eq!(m.normals.len(), m.positions.len());
            assert_eq!(m.groups.iter().map(|g| g.count).sum::<u32>() as usize, m.triangles());
        }
    }
}
