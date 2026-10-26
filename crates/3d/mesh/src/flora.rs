// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Things that grow and lie about, made from rules: pine trees (tiers of
//! needled cones on a trunk), broadleaf trees (a trunk, branches and lumpy
//! crowns of leaves), palms, bushes, rocks (broken, weathered stones),
//! tufts of grass and flowers. Each stands on the origin with its base at
//! y = 0 and rises to −y, `size` px tall, light enough to scatter hundreds.

use crate::region::Region;
use crate::{Material, Mesh, V3, ops, prim, srgb_to_linear};

fn mat(name: &str, rgb: [u8; 3], rough: f32, sss: f32) -> Material {
    Material { name: name.into(), base_colour: [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2]), 1.0], roughness: rough, subsurface: sss, ..Material::default() }
}

/// Moves `part` so its own origin goes to `at` and adds it with material
/// `m` (an index into `into`'s materials).
fn put(into: &mut Mesh, mut part: Mesh, at: V3, turn: V3, m: u32, mats: &mut Vec<u32>) {
    ops::transform(&mut part, &Region::All, [0.0; 3], turn, [1.0; 3], [0.0; 3]);
    ops::transform(&mut part, &Region::All, at, [0.0; 3], [1.0; 3], [0.0; 3]);
    let base = into.positions.len() as u32;
    into.positions.extend_from_slice(&part.positions);
    into.normals.extend_from_slice(&part.normals);
    into.uvs.extend_from_slice(&part.uvs);
    into.indices.extend(part.indices.iter().map(|i| i + base));
    mats.extend(std::iter::repeat_n(m, part.triangles()));
}

fn rand(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed >> 8) as f32 / 16_777_216.0
}

/// A plant or stone by kind ("pine", "tree", "palm", "bush", "rock",
/// "grass", "flower"); an empty mesh for an unknown kind.
pub fn make(kind: &str, size: f32, seed: u32) -> Mesh {
    let s = size.max(1.0);
    let mut sd = seed.wrapping_mul(747_796_405) | 1;
    let mut m = Mesh::default();
    let mut mats: Vec<u32> = Vec::new();
    match kind {
        "pine" | "fir" | "conifer" => {
            m.materials = vec![mat("Bark", [92, 64, 44], 0.9, 0.0), mat("Needles", [38, 74, 44], 0.75, 0.3)];
            // A cone's tip is at −y from its base at its origin.
            let mut trunk = prim::cylinder(s * 0.035, s * 0.05, s * 0.3, 6);
            ops::transform(&mut trunk, &Region::All, [0.0; 3], [0.0; 3], [1.0; 3], [0.0; 3]);
            put(&mut m, trunk, [0.0, -s * 0.15, 0.0], [0.0; 3], 0, &mut mats);
            let tiers = 4;
            for k in 0..tiers {
                let f = k as f32 / tiers as f32;
                let r = s * (0.32 - 0.22 * f) * (0.85 + 0.3 * rand(&mut sd));
                let h = s * 0.42 * (1.0 - 0.4 * f);
                let cone = prim::cone(r, h, 9);
                // prim::cone points up (−y) from its centre: lift its base.
                let y = -s * (0.18 + 0.2 * k as f32);
                put(&mut m, cone, [0.0, y - h * 0.5, 0.0], [0.0, rand(&mut sd) * 40.0, 0.0], 1, &mut mats);
            }
        }
        "tree" | "oak" | "broadleaf" | "birch" => {
            m.materials = vec![mat("Bark", [96, 72, 52], 0.9, 0.0), mat("Leaves", [62, 104, 48], 0.7, 0.35)];
            let trunk_h = s * 0.45;
            put(&mut m, prim::cylinder(s * 0.035, s * 0.06, trunk_h, 7), [0.0, -trunk_h * 0.5, 0.0], [0.0; 3], 0, &mut mats);
            // Branches leaning out, crowned by lumps of leaves.
            for b in 0..4 {
                let a = b as f32 * 90.0 + rand(&mut sd) * 40.0;
                let len = s * 0.25;
                let mut br = prim::cylinder(s * 0.012, s * 0.025, len, 5);
                ops::transform(&mut br, &Region::All, [0.0, -len * 0.5, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
                put(&mut m, br, [0.0, -trunk_h * 0.8, 0.0], [0.0, a, 40.0], 0, &mut mats);
            }
            for _ in 0..7 {
                let r = s * (0.14 + 0.08 * rand(&mut sd));
                let mut blob = prim::ico_sphere(r, 1);
                ops::displace(&mut blob, &Region::All, r * 0.25, r * 0.8, 2, sd);
                let at = [(rand(&mut sd) - 0.5) * s * 0.45, -s * (0.62 + 0.22 * rand(&mut sd)), (rand(&mut sd) - 0.5) * s * 0.45];
                put(&mut m, blob, at, [0.0; 3], 1, &mut mats);
            }
        }
        "palm" => {
            m.materials = vec![mat("Bark", [128, 104, 72], 0.85, 0.0), mat("Fronds", [70, 120, 52], 0.65, 0.3)];
            let segs = 6;
            for k in 0..segs {
                let f = k as f32 / segs as f32;
                let x = (f * f) * s * 0.15;
                put(&mut m, prim::cylinder(s * 0.03, s * 0.035, s * 0.85 / segs as f32 * 1.05, 6), [x, -s * 0.85 * (f + 0.5 / segs as f32), 0.0], [0.0, 0.0, -f * 12.0], 0, &mut mats);
            }
            for k in 0..8 {
                let mut frond = prim::cube([s * 0.42, s * 0.006, s * 0.07]);
                ops::transform(&mut frond, &Region::All, [s * 0.21, 0.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
                put(&mut m, frond, [s * 0.15, -s * 0.86, 0.0], [0.0, k as f32 * 45.0, -25.0 - rand(&mut sd) * 15.0], 1, &mut mats);
            }
        }
        "bush" | "shrub" => {
            m.materials = vec![mat("Leaves", [58, 96, 46], 0.75, 0.35)];
            for _ in 0..5 {
                let r = s * (0.25 + 0.12 * rand(&mut sd));
                let mut blob = prim::ico_sphere(r, 1);
                ops::displace(&mut blob, &Region::All, r * 0.25, r * 0.7, 2, sd);
                put(&mut m, blob, [(rand(&mut sd) - 0.5) * s * 0.6, -r * 0.8, (rand(&mut sd) - 0.5) * s * 0.6], [0.0; 3], 0, &mut mats);
            }
        }
        "rock" | "stone" | "boulder" => {
            m.materials = vec![mat("Stone", [122, 118, 110], 0.85, 0.0)];
            let mut r = prim::ico_sphere(s * 0.5, 2);
            ops::transform(&mut r, &Region::All, [0.0; 3], [rand(&mut sd) * 360.0, rand(&mut sd) * 360.0, 0.0], [1.0, 0.6 + 0.3 * rand(&mut sd), 0.8 + 0.4 * rand(&mut sd)], [0.0; 3]);
            ops::displace(&mut r, &Region::All, s * 0.12, s * 0.5, 4, sd);
            ops::recompute_normals(&mut r, 40.0);
            // Sunk a little into the ground.
            let (lo, hi) = r.bounds();
            let lift = hi[1] - (hi[1] - lo[1]) * 0.2;
            put(&mut m, r, [0.0, -lift, 0.0], [0.0; 3], 0, &mut mats);
        }
        "grass" | "tuft" => {
            m.materials = vec![mat("Grass", [92, 132, 56], 0.6, 0.4)];
            for _ in 0..9 {
                let h = s * (0.6 + 0.4 * rand(&mut sd));
                let mut blade = Mesh { positions: vec![[-s * 0.02, 0.0, 0.0], [s * 0.02, 0.0, 0.0], [0.0, -h, 0.0]], normals: vec![[0.0, 0.0, -1.0]; 3], uvs: vec![[0.0, 1.0], [1.0, 1.0], [0.5, 0.0]], indices: vec![0, 2, 1, 0, 1, 2], ..Default::default() };
                blade.single_group();
                let lean = [(rand(&mut sd) - 0.5) * 50.0, rand(&mut sd) * 360.0, (rand(&mut sd) - 0.5) * 50.0];
                put(&mut m, blade, [(rand(&mut sd) - 0.5) * s * 0.3, 0.0, (rand(&mut sd) - 0.5) * s * 0.3], lean, 0, &mut mats);
            }
        }
        "flower" | "flowers" => {
            let colours = [[220, 60, 80], [240, 200, 60], [180, 90, 210], [250, 250, 250]];
            let c = colours[(rand(&mut sd) * 4.0) as usize % 4];
            m.materials = vec![mat("Stem", [70, 120, 50], 0.6, 0.3), mat("Petals", c, 0.5, 0.4)];
            put(&mut m, prim::cylinder(s * 0.012, s * 0.012, s * 0.8, 4), [0.0, -s * 0.4, 0.0], [0.0; 3], 0, &mut mats);
            let mut head = prim::ico_sphere(s * 0.1, 1);
            ops::transform(&mut head, &Region::All, [0.0; 3], [0.0; 3], [1.0, 0.45, 1.0], [0.0; 3]);
            put(&mut m, head, [0.0, -s * 0.82, 0.0], [0.0; 3], 1, &mut mats);
        }
        _ => return Mesh::default(),
    }
    m.regroup(&mats);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_stands_on_the_ground_at_its_size() {
        for k in crate::terrain::KINDS {
            let m = make(k, 100.0, 3);
            assert!(m.triangles() > 0, "{k}");
            let (lo, hi) = m.bounds();
            assert!(hi[1] < 25.0 && lo[1] < -40.0, "{k}: {lo:?} {hi:?}");
            assert!(hi[1] - lo[1] < 140.0, "{k}");
        }
        assert_eq!(make("unicorn", 10.0, 1).triangles(), 0);
    }
}
