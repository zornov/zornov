// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ground to build a set on: terrain from fractal noise (hills,
//! mountains, dunes, plains, canyons, an island in the sea), painted by
//! slope and height (grass on the flats, rock on the steeps, snow on the
//! peaks, sand by the water) into a texture of its own; and scattering:
//! trees, bushes, rocks, grass and flowers placed over it by rules (how
//! steep, how high, how crowded), merged into one model.
//!
//! The terrain lies in the x–z plane facing up (−y), centred, its heights
//! going up as negative y; its front (−z) is a level stage for the action,
//! the land rising behind it.

use crate::flora;
use crate::{Material, Mesh, Texture, V3, cross, normalise, scale, sub};

/// How a landscape is shaped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Hills,
    Mountains,
    Dunes,
    Plains,
    Canyon,
    Island,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        Some(match s {
            "hills" | "rolling" => Style::Hills,
            "mountains" | "mountain" | "alps" | "peaks" => Style::Mountains,
            "dunes" | "desert" => Style::Dunes,
            "plains" | "flat" | "meadow" | "field" => Style::Plains,
            "canyon" | "mesa" | "badlands" => Style::Canyon,
            "island" | "coast" => Style::Island,
            _ => return None,
        })
    }
}

pub const STYLES: &[&str] = &["hills", "mountains", "dunes", "plains", "canyon", "island"];

/// A terrain's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Land {
    /// Width and depth (px).
    pub size: [f32; 2],
    /// Highest rise (px).
    pub height: f32,
    pub style: Style,
    /// Grid cells along the longer side.
    pub cells: usize,
    pub seed: u32,
}

/// The height (px, positive up) at ground point (x, z).
pub fn height_at(l: &Land, x: f32, z: f32) -> f32 {
    let big = l.size[0].max(l.size[1]).max(1.0);
    let p = [x / big * 4.0, 0.0, z / big * 4.0];
    let n = |q: V3, oct: usize, seed: u32| crate::ops::fbm(q, oct, l.seed.wrapping_add(seed));
    let h = match l.style {
        Style::Hills => 0.5 + 0.5 * n(p, 4, 0),
        Style::Plains => 0.5 + 0.18 * n(scale(p, 0.6), 3, 0),
        Style::Mountains => {
            // Ridged noise: sharp crests.
            let mut acc = 0.0;
            let mut amp = 0.6;
            let mut q = scale(p, 0.8);
            for o in 0..5 {
                let r = 1.0 - n(q, 1, o * 13).abs();
                acc += r * r * amp;
                amp *= 0.5;
                q = scale(q, 2.03);
            }
            acc
        }
        Style::Dunes => {
            let warp = n(scale(p, 0.5), 2, 5) * 1.5;
            let w = ((p[0] * 1.6 + warp).sin() * 0.5 + 0.5).powf(1.6);
            0.25 + 0.6 * w + 0.1 * n(scale(p, 2.0), 2, 9)
        }
        Style::Canyon => {
            let base = 0.5 + 0.5 * n(scale(p, 0.7), 4, 0);
            // Terraces, and a river cut along the noise's zero line.
            let terr = (base * 6.0).floor() / 6.0 + ((base * 6.0).fract().powf(3.0)) / 6.0;
            let river = n(scale(p, 0.45), 3, 21).abs();
            terr * (river * 6.0).clamp(0.0, 1.0)
        }
        Style::Island => {
            let r = (x * x + z * z).sqrt() / (big * 0.5);
            let falloff = (1.0 - r * 1.15).clamp(0.0, 1.0);
            (0.55 + 0.45 * n(p, 5, 0)) * falloff.powf(0.8) - 0.12
        }
    };
    // A level stage at the front (nearest −z) where the action happens;
    // the land rises beyond it.
    let front = (z / l.size[1].max(1.0) + 0.5).clamp(0.0, 1.0);
    let stage = if l.style == Style::Island { 1.0 } else { smoothstep(0.3, 0.62, front) };
    h * l.height * stage
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The terrain model, textured by slope and height.
pub fn terrain(l: &Land) -> Mesh {
    let big = l.size[0].max(l.size[1]).max(1.0);
    let c = l.cells.clamp(8, 600);
    let nx = ((l.size[0] / big * c as f32).round() as usize).max(2);
    let nz = ((l.size[1] / big * c as f32).round() as usize).max(2);
    let mut m = Mesh::default();
    for zi in 0..=nz {
        for xi in 0..=nx {
            let (u, v) = (xi as f32 / nx as f32, zi as f32 / nz as f32);
            let (x, z) = ((u - 0.5) * l.size[0], (v - 0.5) * l.size[1]);
            m.positions.push([x, -height_at(l, x, z), z]);
            m.uvs.push([u, v]);
        }
    }
    let at = |x: usize, z: usize| (z * (nx + 1) + x) as u32;
    for z in 0..nz {
        for x in 0..nx {
            m.indices.extend_from_slice(&[at(x, z), at(x + 1, z), at(x + 1, z + 1), at(x, z), at(x + 1, z + 1), at(x, z + 1)]);
        }
    }
    m.normals = vec![[0.0; 3]; m.positions.len()];
    crate::ops::smooth_normals_in_place(&mut m);
    // Make sure the faces look up.
    if m.normals.iter().map(|n| n[1]).sum::<f32>() > 0.0 {
        crate::ops::flip(&mut m);
    }
    m.materials = vec![Material { name: "Ground".into(), base_colour: [1.0, 1.0, 1.0, 1.0], roughness: 0.92, texture: Some(paint(l, &m, nx, nz)), ..Material::default() }];
    m.single_group();
    m
}

/// The ground's colours: grass, rock, snow, sand and earth by slope and
/// height, with noise so it never looks tiled.
fn paint(l: &Land, m: &Mesh, nx: usize, nz: usize) -> Texture {
    let res = 512usize;
    let mut rgba = Vec::with_capacity(res * res * 4);
    let lin = |c: [u8; 3]| [crate::srgb_to_linear(c[0]), crate::srgb_to_linear(c[1]), crate::srgb_to_linear(c[2])];
    let grass = lin([84, 112, 52]);
    let grass2 = lin([118, 128, 62]);
    let rock = lin([112, 104, 96]);
    let snow = lin([236, 240, 246]);
    let sand = lin([196, 178, 132]);
    let earth = lin([106, 84, 60]);
    let dune = lin([214, 170, 112]);
    let to_srgb = |v: f32| {
        let c = v.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    for py in 0..res {
        for px in 0..res {
            let (u, v) = ((px as f32 + 0.5) / res as f32, (py as f32 + 0.5) / res as f32);
            let (xi, zi) = ((u * nx as f32) as usize, (v * nz as f32) as usize);
            let i = zi.min(nz) * (nx + 1) + xi.min(nx);
            let n = m.normals.get(i).copied().unwrap_or([0.0, -1.0, 0.0]);
            let h = -m.positions.get(i).map_or(0.0, |p| p[1]) / l.height.max(1e-3);
            let slope = 1.0 - (-n[1]).clamp(0.0, 1.0);
            let q = [u * 40.0, 0.0, v * 40.0];
            let detail = crate::ops::fbm(q, 3, l.seed.wrapping_add(77));
            let mut c = crate::lerp(grass, grass2, (detail * 0.5 + 0.5).clamp(0.0, 1.0));
            if l.style == Style::Dunes {
                c = crate::lerp(dune, sand, (detail * 0.5 + 0.5).clamp(0.0, 1.0));
            }
            if l.style == Style::Canyon {
                c = crate::lerp(earth, lin([170, 96, 62]), ((h * 7.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0));
            }
            // Sand near the water line, rock on the steeps, snow up high.
            c = crate::lerp(c, sand, 1.0 - smoothstep(0.02, 0.08, h + detail * 0.02));
            c = crate::lerp(c, earth, smoothstep(0.25, 0.4, slope) * 0.6);
            c = crate::lerp(c, rock, smoothstep(0.35, 0.55, slope + detail * 0.05));
            if matches!(l.style, Style::Mountains | Style::Hills | Style::Island) {
                c = crate::lerp(c, snow, smoothstep(0.72, 0.8, h + detail * 0.05) * (1.0 - smoothstep(0.5, 0.7, slope)));
            }
            let k = 0.9 + 0.2 * crate::ops::fbm(scale(q, 4.0), 2, l.seed.wrapping_add(3));
            rgba.extend_from_slice(&[to_srgb(c[0] * k), to_srgb(c[1] * k), to_srgb(c[2] * k), 255]);
        }
    }
    Texture { width: res as u32, height: res as u32, rgba }
}

/// What to scatter, how many, and where it may go.
#[derive(Clone, Debug, PartialEq)]
pub struct Scatter {
    /// "pine", "tree" (broadleaf), "palm", "bush", "rock", "grass",
    /// "flower".
    pub kind: String,
    pub count: usize,
    /// Size of each (px; ± 30 %).
    pub size: f32,
    /// Steepest ground allowed, degrees.
    pub max_slope: f32,
    /// Height band allowed (fractions of the land's height, 0..1).
    pub min_height: f32,
    pub max_height: f32,
    /// How much they bunch together, 0 even – 1 clumped.
    pub clumping: f32,
    pub seed: u32,
}

pub const KINDS: &[&str] = &["pine", "tree", "palm", "bush", "rock", "grass", "flower"];

/// Places copies over the ground mesh `ground` (made by [`terrain`] or any
/// upward-facing surface) and returns them as one model. Each copy turns
/// to a random heading and leans with the slope a little.
pub fn scatter(ground: &Mesh, land_height: f32, rules: &[Scatter]) -> Mesh {
    let bvh = crate::bvh::Bvh::of_mesh(ground);
    let (lo, hi) = ground.bounds();
    let mut out = Mesh::default();
    let mut tri_mats: Vec<u32> = Vec::new();
    for r in rules {
        let mut rng = r.seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f32 / 16_777_216.0
        };
        // A handful of variants, reused.
        let variants: Vec<Mesh> = (0..4).map(|k| flora::make(&r.kind, r.size, r.seed.wrapping_add(k))).collect();
        if variants.iter().all(|v| v.triangles() == 0) {
            continue;
        }
        // Clump centres.
        let clumps: Vec<[f32; 2]> = (0..8).map(|_| [lo[0] + next() * (hi[0] - lo[0]), lo[2] + next() * (hi[2] - lo[2])]).collect();
        let mut placed = 0;
        let mut tries = 0;
        while placed < r.count && tries < r.count * 30 {
            tries += 1;
            let (mut x, mut z) = (lo[0] + next() * (hi[0] - lo[0]), lo[2] + next() * (hi[2] - lo[2]));
            if r.clumping > 0.0 && next() < r.clumping {
                let c = clumps[(next() * clumps.len() as f32) as usize % clumps.len()];
                let spread = (hi[0] - lo[0]) * 0.08;
                x = c[0] + (next() - 0.5) * spread * 2.0;
                z = c[1] + (next() - 0.5) * spread * 2.0;
            }
            // Nothing in the first stretch in front of the camera.
            let keep_off = if matches!(r.kind.as_str(), "pine" | "tree" | "palm" | "bush") { 0.3 } else { 0.08 };
            if z < lo[2] + (hi[2] - lo[2]) * keep_off {
                continue;
            }
            // Drop onto the ground.
            let Some(h) = bvh.ray([x, lo[1] - 10.0, z], [0.0, 1.0, 0.0], 0.0, f32::INFINITY) else { continue };
            let tri = bvh.tris[h.tri as usize];
            let n = normalise(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
            let up = if n[1] > 0.0 { scale(n, -1.0) } else { n };
            let slope = (-up[1]).clamp(-1.0, 1.0).acos().to_degrees();
            let p = [x, lo[1] - 10.0 + h.t, z];
            let height = (-p[1]) / land_height.max(1e-3);
            if slope > r.max_slope || height < r.min_height || height > r.max_height {
                continue;
            }
            let v = &variants[(next() * variants.len() as f32) as usize % variants.len()];
            let k = 0.7 + next() * 0.6;
            let turn = next() * 360.0;
            let mut copy = v.clone();
            let lean = [up[2].atan2(-up[1]).to_degrees() * -0.3, 0.0, up[0].atan2(-up[1]).to_degrees() * 0.3];
            crate::ops::transform(&mut copy, &crate::region::Region::All, [0.0; 3], [0.0, turn, 0.0], [k; 3], [0.0; 3]);
            crate::ops::transform(&mut copy, &crate::region::Region::All, p, lean, [1.0; 3], [0.0; 3]);
            // Materials by name, shared across copies.
            let mats = copy.tri_materials();
            let base = out.positions.len() as u32;
            out.positions.extend_from_slice(&copy.positions);
            out.normals.extend_from_slice(&copy.normals);
            out.uvs.extend_from_slice(&copy.uvs);
            out.indices.extend(copy.indices.iter().map(|i| i + base));
            for m in mats {
                let mat = &copy.materials[m as usize];
                let idx = out.materials.iter().position(|x| x.name == mat.name).unwrap_or_else(|| {
                    out.materials.push(mat.clone());
                    out.materials.len() - 1
                });
                tri_mats.push(idx as u32);
            }
            placed += 1;
        }
    }
    if out.materials.is_empty() {
        out.materials.push(Material::default());
    }
    out.regroup(&tri_mats);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountains_rise_and_trees_keep_off_the_steeps() {
        let land = Land { size: [2000.0, 2000.0], height: 400.0, style: Style::Mountains, cells: 80, seed: 3 };
        let t = terrain(&land);
        let (lo, hi) = t.bounds();
        assert!(hi[0] - lo[0] > 1990.0 && hi[1] - lo[1] > 100.0, "{lo:?} {hi:?}");
        assert!(t.materials[0].texture.is_some());
        // Faces look up (−y).
        let up = (0..t.triangles()).filter(|&k| t.face_normal(k)[1] < 0.0).count();
        assert!(up > t.triangles() * 9 / 10);
        let trees = scatter(&t, land.height, &[Scatter { kind: "pine".into(), count: 40, size: 60.0, max_slope: 25.0, min_height: 0.0, max_height: 1.0, clumping: 0.3, seed: 1 }]);
        assert!(trees.triangles() > 40 * 20, "{}", trees.triangles());
        // Every tree stands on the ground near where it was dropped.
        let (tlo, thi) = trees.bounds();
        assert!(tlo[0] >= lo[0] - 80.0 && thi[0] <= hi[0] + 80.0);
        for s in STYLES {
            let l = Land { style: Style::parse(s).unwrap(), cells: 16, ..land };
            assert!(terrain(&l).triangles() > 0, "{s}");
        }
    }
}
