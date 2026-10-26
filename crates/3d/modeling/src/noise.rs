// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Procedural patterns in 3D: gradient noise, fractal sums, ridges,
//! cells (Worley), and the natural surfaces built from them — pores,
//! scales, wrinkles, cracks, weave, wood grain, stone. Sculpt alphas and
//! procedural materials both draw on these, so detail sculpted into a
//! surface and detail painted onto it line up.

use studio_mesh::V3;

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

fn hash3(i: i32, j: i32, k: i32, seed: u32) -> u32 {
    hash((i as u32).wrapping_mul(73_856_093) ^ (j as u32).wrapping_mul(19_349_663) ^ (k as u32).wrapping_mul(83_492_791) ^ seed.wrapping_mul(2_654_435_761))
}

/// A pseudo-random number in 0–1 for a lattice point.
pub fn rand3(i: i32, j: i32, k: i32, seed: u32) -> f32 {
    hash3(i, j, k, seed) as f32 / u32::MAX as f32
}

fn grad(h: u32, x: f32, y: f32, z: f32) -> f32 {
    // Twelve edge directions of a cube.
    match h % 12 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise, about −1…1, smooth, one feature per unit.
pub fn perlin(p: V3, seed: u32) -> f32 {
    let (xi, yi, zi) = (p[0].floor(), p[1].floor(), p[2].floor());
    let (x, y, z) = (p[0] - xi, p[1] - yi, p[2] - zi);
    let (i, j, k) = (xi as i32, yi as i32, zi as i32);
    let (u, v, w) = (fade(x), fade(y), fade(z));
    let g = |a: i32, b: i32, c: i32, dx: f32, dy: f32, dz: f32| grad(hash3(i + a, j + b, k + c, seed), x - dx, y - dy, z - dz);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = l(g(0, 0, 0, 0.0, 0.0, 0.0), g(1, 0, 0, 1.0, 0.0, 0.0), u);
    let x10 = l(g(0, 1, 0, 0.0, 1.0, 0.0), g(1, 1, 0, 1.0, 1.0, 0.0), u);
    let x01 = l(g(0, 0, 1, 0.0, 0.0, 1.0), g(1, 0, 1, 1.0, 0.0, 1.0), u);
    let x11 = l(g(0, 1, 1, 0.0, 1.0, 1.0), g(1, 1, 1, 1.0, 1.0, 1.0), u);
    0.7 * l(l(x00, x10, v), l(x01, x11, v), w)
}

/// Fractal sum of `octaves` layers (each half the size, `gain` the
/// strength), about −1…1.
pub fn fbm(p: V3, octaves: usize, gain: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves.clamp(1, 12) {
        sum += amp * perlin([p[0] * freq, p[1] * freq, p[2] * freq], seed.wrapping_add(o as u32 * 101));
        norm += amp;
        amp *= gain;
        freq *= 2.0;
    }
    sum / norm
}

/// Sharp crests where the noise crosses zero (mountain ridges, veins,
/// wrinkles), 0…1.
pub fn ridged(p: V3, octaves: usize, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut weight = 1.0f32;
    let mut norm = 0.0;
    for o in 0..octaves.clamp(1, 12) {
        let n = 1.0 - perlin([p[0] * freq, p[1] * freq, p[2] * freq], seed.wrapping_add(o as u32 * 131)).abs() * 1.4;
        let n = n.clamp(0.0, 1.0);
        let n = n * n * weight;
        weight = (n * 2.0).clamp(0.0, 1.0);
        sum += n * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    (sum / norm).clamp(0.0, 1.0)
}

/// Cells: distance to the nearest and second-nearest scattered point (in
/// cell units) and the nearest cell's random id (0–1).
pub fn cells(p: V3, jitter: f32, seed: u32) -> (f32, f32, f32) {
    let (xi, yi, zi) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
    let mut f1 = f32::INFINITY;
    let mut f2 = f32::INFINITY;
    let mut id = 0.0;
    for dk in -1..=1 {
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j, k) = (xi + di, yi + dj, zi + dk);
                let q = [i as f32 + 0.5 + jitter * (rand3(i, j, k, seed) - 0.5), j as f32 + 0.5 + jitter * (rand3(i, j, k, seed ^ 0x9e37) - 0.5), k as f32 + 0.5 + jitter * (rand3(i, j, k, seed ^ 0x51ed) - 0.5)];
                let d = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt();
                if d < f1 {
                    f2 = f1;
                    f1 = d;
                    id = rand3(i, j, k, seed ^ 0x7777);
                } else if d < f2 {
                    f2 = d;
                }
            }
        }
    }
    (f1, f2, id)
}

fn scale3(p: V3, k: f32) -> V3 {
    [p[0] * k, p[1] * k, p[2] * k]
}

/// Natural surface detail by name, as a height about −1…1 at point `p`
/// with features `size` px across.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Soft lumps.
    Noise,
    /// Fine pits (skin, leather, citrus).
    Pores,
    /// Overlapping plates (reptiles, fish, armour).
    Scales,
    /// Fine lines along one direction (skin creases, bark, fabric folds).
    Wrinkles,
    /// Dry cracks (mud, old paint, rock).
    Cracks,
    /// A woven grid (cloth, wicker).
    Weave,
    /// Rough broken stone.
    Rock,
    /// Growth rings and grain.
    Wood,
    /// Sharp crests.
    Ridges,
    /// Flat cobbles or pebbles.
    Cells,
    /// Hammered metal dents.
    Hammered,
}

impl Pattern {
    pub const ALL: &'static [(&'static str, Pattern)] = &[
        ("noise", Pattern::Noise),
        ("pores", Pattern::Pores),
        ("scales", Pattern::Scales),
        ("wrinkles", Pattern::Wrinkles),
        ("cracks", Pattern::Cracks),
        ("weave", Pattern::Weave),
        ("rock", Pattern::Rock),
        ("wood", Pattern::Wood),
        ("ridges", Pattern::Ridges),
        ("cells", Pattern::Cells),
        ("hammered", Pattern::Hammered),
    ];

    pub fn parse(s: &str) -> Option<Pattern> {
        Pattern::ALL.iter().find(|(n, _)| *n == s).map(|(_, p)| *p)
    }

    pub fn name(self) -> &'static str {
        Pattern::ALL.iter().find(|(_, p)| *p == self).map_or("noise", |(n, _)| n)
    }

    /// Height at `p` (px), features `size` px across.
    pub fn height(self, p: V3, size: f32, seed: u32) -> f32 {
        let q = scale3(p, 1.0 / size.max(1e-3));
        match self {
            Pattern::Noise => fbm(q, 4, 0.5, seed),
            Pattern::Pores => {
                let (f1, _, _) = cells(scale3(q, 3.0), 0.9, seed);
                let pit = (1.0 - (f1 / 0.32).min(1.0)).powi(3);
                -2.0 * pit + 0.15 * fbm(scale3(q, 6.0), 2, 0.5, seed)
            }
            Pattern::Scales => {
                let (f1, f2, id) = cells(q, 0.6, seed);
                let edge = ((f2 - f1) / 0.35).min(1.0);
                // Domed plates with a lip.
                (edge.sqrt() * 1.6 - 0.8) + 0.2 * (id - 0.5)
            }
            Pattern::Wrinkles => {
                let warp = fbm(scale3(q, 0.5), 3, 0.5, seed) * 2.0;
                let w = ((q[1] * 3.0 + warp) * std::f32::consts::PI).sin();
                -(1.0 - w.abs()).powi(4) * 1.5 + 0.2 * fbm(scale3(q, 4.0), 2, 0.5, seed)
            }
            Pattern::Cracks => {
                let (f1, f2, _) = cells(scale3(add(q, scale3([fbm(q, 2, 0.5, seed), fbm(q, 2, 0.5, seed + 7), 0.0], 0.3)), 1.0), 1.0, seed);
                let crack = (1.0 - ((f2 - f1) / 0.08).min(1.0)).powi(2);
                -crack * 1.6 + 0.1 * fbm(scale3(q, 4.0), 3, 0.5, seed)
            }
            Pattern::Weave => {
                let (u, v) = (q[0] * 4.0, q[2] * 4.0 + q[1] * 4.0);
                let a = (u * std::f32::consts::PI).sin() * ((v * 0.5).floor().rem_euclid(2.0) * 2.0 - 1.0);
                let b = (v * std::f32::consts::PI).sin() * ((u * 0.5).floor().rem_euclid(2.0) * 2.0 - 1.0);
                0.6 * a.max(b) + 0.1 * fbm(scale3(q, 8.0), 2, 0.5, seed)
            }
            Pattern::Rock => {
                let (f1, _, id) = cells(q, 1.0, seed);
                0.6 * fbm(scale3(q, 2.0), 5, 0.55, seed) - 0.5 * f1 + 0.3 * id
            }
            Pattern::Wood => {
                let r = (q[0] * q[0] + q[2] * q[2]).sqrt() * 4.0 + fbm(scale3(q, 0.7), 3, 0.5, seed) * 1.5;
                let ring = (r * std::f32::consts::TAU).sin();
                0.5 * ring + 0.3 * fbm([q[0] * 6.0, q[1] * 0.4, q[2] * 6.0], 3, 0.5, seed)
            }
            Pattern::Ridges => ridged(q, 5, seed) * 2.0 - 1.0,
            Pattern::Cells => {
                let (f1, f2, _) = cells(q, 0.8, seed);
                ((f2 - f1) / 0.5).min(1.0) * 2.0 - 1.0
            }
            Pattern::Hammered => {
                let (f1, _, _) = cells(scale3(q, 1.5), 0.9, seed);
                -(1.0 - (f1 / 0.7).min(1.0)).powi(2) * 1.2 + 0.4
            }
        }
    }
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_and_varies() {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..2000 {
            let p = [i as f32 * 0.137, i as f32 * 0.071, i as f32 * 0.029];
            let n = perlin(p, 3);
            lo = lo.min(n);
            hi = hi.max(n);
        }
        assert!(lo > -1.0 && hi < 1.0 && hi - lo > 0.6, "{lo}..{hi}");
        for (name, pat) in Pattern::ALL {
            let vals: Vec<f32> = (0..500).map(|i| pat.height([i as f32 * 1.3, 0.7 * i as f32, 0.0], 20.0, 1)).collect();
            let (a, b) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
            assert!(b - a > 0.2, "{name} is flat");
            assert!(a > -3.0 && b < 3.0, "{name} out of range {a}..{b}");
        }
    }
}
