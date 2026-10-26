// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scattering: copies of a model spread over a surface the way nature or a
//! set dresser spreads them — rocks on a slope, trees on a hill, rivets on
//! a hull, debris round a crash — spaced so they don't pile up (Poisson
//! disc), turned and sized at random within limits, standing up or lying
//! along the ground, kept off slopes that are too steep or heights that
//! are wrong.

use crate::groom::Rng;
use crate::spatial::PointGrid;
use serde::{Deserialize, Serialize};
use studio_mesh::{Mesh, V3, add, cross, dot, length, lerp, normalise, scale, sub};

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

/// How copies are spread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scatter {
    /// The part they sit on.
    pub on: String,
    /// What is scattered: another part of the asset, or a library model
    /// (`.smodel` / `.glb` path).
    pub source: String,
    /// How many (at most; spacing may allow fewer).
    pub count: usize,
    /// Least distance between copies (px; 0: from the count).
    #[serde(default)]
    pub spacing: f32,
    /// Size range (1 = as the source is).
    #[serde(default = "scale_range")]
    pub scale: [f32; 2],
    /// Random turn about the surface normal, degrees (360 any way).
    #[serde(default = "full_turn")]
    pub turn: f32,
    /// Random tilt off the normal, degrees.
    #[serde(default)]
    pub tilt: f32,
    /// 1 stands along the surface normal, 0 stands straight up (trees on
    /// a hillside grow up, not out).
    #[serde(default = "one")]
    pub align: f32,
    /// Steepest slope allowed (degrees from flat ground; 90 any).
    #[serde(default = "ninety")]
    pub max_slope: f32,
    /// Allowed heights (y, px; y is down).
    #[serde(default)]
    pub height: Option<[f32; 2]>,
    /// Sink copies into the surface by this share of their height.
    #[serde(default)]
    pub sink: f32,
    /// Fewer copies where a pattern is low (patchy growth): pattern size
    /// px, 0 none.
    #[serde(default)]
    pub clumps: f32,
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "yes")]
    pub visible: bool,
}

fn scale_range() -> [f32; 2] {
    [0.8, 1.2]
}
fn full_turn() -> f32 {
    360.0
}
fn ninety() -> f32 {
    90.0
}

/// Where each copy goes: position, its three axes (rows: right, up-ish,
/// forward) and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub at: V3,
    pub axes: [V3; 3],
    pub size: f32,
}

/// Placements on a surface (triangles).
pub fn place(surface: &Mesh, s: &Scatter, source_height: f32) -> Vec<Instance> {
    let mut rng = Rng::new(s.seed.wrapping_add(17));
    let tris = surface.triangles();
    let up = [0.0f32, -1.0, 0.0];
    let max_cos = s.max_slope.clamp(0.0, 90.0).to_radians().cos();
    // Area of the faces allowed, for spacing and picking.
    let mut acc = Vec::with_capacity(tris);
    let mut total = 0.0f32;
    for t in 0..tris {
        let [a, b, c] = surface.tri(t);
        let (pa, pb, pc) = (surface.positions[a], surface.positions[b], surface.positions[c]);
        let n = normalise(cross(sub(pb, pa), sub(pc, pa)));
        let ok_slope = dot(n, up) >= max_cos - 1e-4;
        let ctr = scale(add(add(pa, pb), pc), 1.0 / 3.0);
        let ok_h = s.height.is_none_or(|h| ctr[1] >= h[0].min(h[1]) && ctr[1] <= h[0].max(h[1]));
        if ok_slope && ok_h {
            total += 0.5 * length(cross(sub(pb, pa), sub(pc, pa)));
        }
        acc.push(total);
    }
    if total <= 0.0 || s.count == 0 {
        return Vec::new();
    }
    let count = s.count.min(200_000);
    let spacing = if s.spacing > 0.0 { s.spacing } else { (total / count as f32).sqrt() * 0.7 };
    let mut grid = PointGrid::new(&[], spacing.max(1e-3));
    let mut placed: Vec<V3> = Vec::new();
    let mut out = Vec::new();
    let tries = count * 30;
    for _ in 0..tries {
        if out.len() >= count {
            break;
        }
        let x = rng.next() * total;
        let t = acc.partition_point(|&v| v < x).min(tris - 1);
        let [a, b, c] = surface.tri(t);
        let (pa, pb, pc) = (surface.positions[a], surface.positions[b], surface.positions[c]);
        let n = normalise(cross(sub(pb, pa), sub(pc, pa)));
        if dot(n, up) < max_cos - 1e-4 {
            continue;
        }
        let (mut u, mut v) = (rng.next(), rng.next());
        if u + v > 1.0 {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        let p = add(pa, add(scale(sub(pb, pa), u), scale(sub(pc, pa), v)));
        if let Some(h) = s.height
            && !(p[1] >= h[0].min(h[1]) && p[1] <= h[0].max(h[1]))
        {
            continue;
        }
        if s.clumps > 0.0 {
            let keep = 0.5 + 0.5 * crate::noise::fbm(scale(p, 1.0 / s.clumps), 3, 0.5, s.seed as u32);
            if rng.next() > keep * keep {
                continue;
            }
        }
        if !grid.within(&placed, p, spacing).is_empty() {
            continue;
        }
        grid.insert(placed.len() as u32, p);
        placed.push(p);
        // Up axis: between straight up and the surface normal (outward).
        let nn = if dot(n, up) < 0.0 { scale(n, -1.0) } else { n };
        let mut axis_up = normalise(lerp(up, nn, s.align.clamp(0.0, 1.0)));
        if s.tilt > 0.0 {
            let side = normalise(cross(axis_up, if axis_up[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] }));
            let a = rng.range(-s.tilt, s.tilt).to_radians();
            axis_up = normalise(add(scale(axis_up, a.cos()), scale(side, a.sin())));
        }
        let turn = rng.range(-s.turn / 2.0, s.turn / 2.0).to_radians();
        let base = normalise(cross(axis_up, if axis_up[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] }));
        let fwd0 = cross(base, axis_up);
        let right = normalise(add(scale(base, turn.cos()), scale(fwd0, turn.sin())));
        let fwd = cross(axis_up, right);
        let size = rng.range(s.scale[0].min(s.scale[1]), s.scale[0].max(s.scale[1]));
        let at = sub(p, scale(axis_up, -s.sink * source_height * size));
        // Studio's y is down: the model's −y (its top) goes along axis_up.
        out.push(Instance { at, axes: [right, scale(axis_up, -1.0), fwd], size });
    }
    out
}

/// Copies of `source` at the placements, as one mesh (the source's base,
/// its lowest point, sits on the surface).
pub fn build(source: &Mesh, instances: &[Instance]) -> Mesh {
    let (lo, hi) = source.bounds();
    // The base point: centre in x and z, lowest (largest y).
    let base = [(lo[0] + hi[0]) / 2.0, hi[1], (lo[2] + hi[2]) / 2.0];
    let mut out = Mesh { materials: source.materials.clone(), ..Mesh::default() };
    let mats = source.tri_materials();
    let mut tri_mats = Vec::new();
    for ins in instances {
        let b = out.positions.len() as u32;
        let to = |p: V3| -> V3 {
            let d = scale(sub(p, base), ins.size);
            add(ins.at, add(add(scale(ins.axes[0], d[0]), scale(ins.axes[1], d[1])), scale(ins.axes[2], d[2])))
        };
        let rot = |n: V3| -> V3 { normalise(add(add(scale(ins.axes[0], n[0]), scale(ins.axes[1], n[1])), scale(ins.axes[2], n[2]))) };
        for (i, p) in source.positions.iter().enumerate() {
            out.positions.push(to(*p));
            out.normals.push(rot(source.normals.get(i).copied().unwrap_or([0.0, -1.0, 0.0])));
            out.uvs.push(source.uvs.get(i).copied().unwrap_or([0.0, 0.0]));
        }
        // The axes keep handedness (right × down = forward), so winding
        // stays outward.
        out.indices.extend(source.indices.iter().map(|i| i + b));
        tri_mats.extend_from_slice(&mats);
    }
    if out.materials.is_empty() {
        out.materials.push(studio_mesh::Material::default());
    }
    out.regroup(&tri_mats);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    fn ground() -> Mesh {
        prim::plane(1000.0, 1000.0, 20, 20).to_mesh(180.0, crate::NormalMode::Angle)
    }

    #[test]
    fn copies_spread_apart_and_stand_on_the_ground() {
        let s = Scatter { on: "Ground".into(), source: "Rock".into(), count: 200, spacing: 40.0, scale: [0.5, 1.5], turn: 360.0, tilt: 10.0, align: 1.0, max_slope: 90.0, height: None, sink: 0.0, clumps: 0.0, seed: 3, visible: true };
        let inst = place(&ground(), &s, 20.0);
        assert!(inst.len() > 100, "{}", inst.len());
        for (i, a) in inst.iter().enumerate() {
            assert!(a.at[1].abs() < 1e-3, "on the ground");
            for b in &inst[i + 1..] {
                assert!(length(sub(a.at, b.at)) >= 40.0 - 1e-3);
            }
        }
        let rock = prim::quad_sphere(10.0, 2).to_mesh(180.0, crate::NormalMode::Angle);
        let m = build(&rock, &inst);
        assert_eq!(m.triangles(), rock.triangles() * inst.len());
        // Every copy sits above the ground (y up is −y).
        assert!(m.positions.iter().all(|p| p[1] < 1.0));
        // Copies keep their faces outward (no mirrored copies).
        let vol: f32 = (0..m.triangles()).map(|t| {
            let [a, b, c] = m.tri(t);
            dot(m.positions[a], cross(m.positions[b], m.positions[c])) / 6.0
        }).sum();
        assert!(vol > 0.0, "{vol}");
    }

    #[test]
    fn steep_slopes_stay_bare() {
        let mut wall = prim::plane(500.0, 500.0, 4, 4);
        wall.map_positions(|p| [p[0], -p[2], p[1]]);
        let s = Scatter { on: "Wall".into(), source: "x".into(), count: 50, spacing: 0.0, scale: [1.0, 1.0], turn: 0.0, tilt: 0.0, align: 0.0, max_slope: 30.0, height: None, sink: 0.0, clumps: 0.0, seed: 1, visible: true };
        assert!(place(&wall.to_mesh(180.0, crate::NormalMode::Angle), &s, 10.0).is_empty());
    }
}
