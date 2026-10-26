// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hair and fur grooming: guide strands grown from part of a surface,
//! shaped by a list of grooming steps (comb, gravity, curl, wave, frizz,
//! clump, twist, cut, lift, part, noise), then filled in with thousands
//! of child strands that follow the guides and clump around them. Length
//! and density can vary over the surface. Colour comes from melanin (as
//! real hair does) or from root and tip colours.
//!
//! Strands come out as roots (a triangle of the surface and a place on it)
//! and a fixed number of points each — the layout the hair simulation
//! takes — and as ribbons for drawing, in a material marked as hair.

use crate::noise;
use serde::{Deserialize, Serialize};
use studio_mesh::{Material, Mesh, V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// A small, fast random number source.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// One grooming step, applied to the guides in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    /// Lay strands toward a direction (`strength` 0–1), along the surface.
    Comb { direction: V3, #[serde(default = "one")] strength: f32 },
    /// Let strands hang under their weight (`amount` 0–1; long hair more).
    Gravity { #[serde(default = "half")] amount: f32 },
    /// Spiral curls: `radius` px, `turns` along the strand.
    Curl { radius: f32, turns: f32, #[serde(default)] random: f32 },
    /// Flat S-waves.
    Wave { amplitude: f32, waves: f32 },
    /// Fine random flyaways, 0–1.
    Frizz { amount: f32 },
    /// Gather children around their guide toward the tip (`amount` 0–1;
    /// `shape` > 1 clumps late, < 1 early).
    Clump { amount: f32, #[serde(default = "one")] shape: f32 },
    /// Twist strands round their clump (degrees along the length).
    Twist { degrees: f32 },
    /// Shorten: to `length` px, or by `fraction`.
    Cut { #[serde(default)] length: Option<f32>, #[serde(default)] fraction: Option<f32> },
    /// Stand the roots up off the surface (0 flat – 1 straight out).
    Lift { amount: f32 },
    /// Comb away from a parting line through `at` running along `along`.
    Part { at: V3, along: V3, #[serde(default = "one")] strength: f32 },
    /// Large soft bends.
    Noise { amount: f32, #[serde(default = "noise_size")] size: f32 },
}

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}
fn noise_size() -> f32 {
    80.0
}

/// How a groom grows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Groom {
    pub name: String,
    /// Strands drawn (guides plus children).
    pub count: usize,
    /// Guide strands the steps shape (children follow them).
    #[serde(default = "guides")]
    pub guides: usize,
    pub length: f32,
    /// Random length spread, 0–1.
    #[serde(default = "spread")]
    pub length_random: f32,
    /// Length scaled by a pattern over the surface (patchy fur).
    #[serde(default)]
    pub length_noise: f32,
    #[serde(default = "segments")]
    pub segments: usize,
    #[serde(default = "root_width")]
    pub root_width: f32,
    #[serde(default)]
    pub tip_width: f32,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// How dark: 0 blond/white … 1 black (eumelanin).
    #[serde(default = "melanin")]
    pub melanin: f32,
    /// Red pigment 0–1 (pheomelanin: auburn, ginger).
    #[serde(default)]
    pub redness: f32,
    /// A dye or fur colour over the pigment (linear RGB), or none.
    #[serde(default)]
    pub colour: Option<V3>,
    #[serde(default)]
    pub tip_colour: Option<V3>,
    /// Shine: 0 matte – 1 glossy.
    #[serde(default = "shine")]
    pub shine: f32,
    #[serde(default)]
    pub seed: u64,
}

fn guides() -> usize {
    200
}
fn spread() -> f32 {
    0.2
}
fn segments() -> usize {
    8
}
fn root_width() -> f32 {
    0.6
}
fn melanin() -> f32 {
    0.6
}
fn shine() -> f32 {
    0.6
}

impl Groom {
    /// A starting groom by name.
    pub fn preset(name: &str, scale_px: f32) -> Option<Groom> {
        let s = scale_px.max(1.0);
        let base = |count: usize, length: f32, steps: Vec<Step>| Groom { name: name.to_string(), count, guides: 300, length, length_random: 0.15, length_noise: 0.0, segments: 10, root_width: 0.35, tip_width: 0.05, steps, melanin: 0.55, redness: 0.1, colour: None, tip_colour: None, shine: 0.6, seed: 1 };
        let down = [0.0, 1.0, 0.0];
        Some(match name {
            "short_hair" => base(40_000, 0.08 * s, vec![Step::Lift { amount: 0.4 }, Step::Comb { direction: [0.0, 0.3, 1.0], strength: 0.6 }, Step::Clump { amount: 0.3, shape: 1.0 }, Step::Frizz { amount: 0.05 }]),
            "long_straight" => base(60_000, 0.6 * s, vec![Step::Comb { direction: down, strength: 0.7 }, Step::Gravity { amount: 0.8 }, Step::Clump { amount: 0.45, shape: 1.5 }, Step::Noise { amount: 0.08, size: 0.3 * s }, Step::Frizz { amount: 0.03 }]),
            "wavy" => base(55_000, 0.4 * s, vec![Step::Comb { direction: down, strength: 0.6 }, Step::Gravity { amount: 0.6 }, Step::Wave { amplitude: 0.025 * s, waves: 3.0 }, Step::Clump { amount: 0.5, shape: 1.2 }, Step::Frizz { amount: 0.06 }]),
            "curly" => base(50_000, 0.35 * s, vec![Step::Lift { amount: 0.5 }, Step::Gravity { amount: 0.3 }, Step::Curl { radius: 0.02 * s, turns: 5.0, random: 0.3 }, Step::Clump { amount: 0.6, shape: 0.8 }, Step::Frizz { amount: 0.1 }]),
            "coily" | "afro" => base(70_000, 0.18 * s, vec![Step::Lift { amount: 0.9 }, Step::Curl { radius: 0.008 * s, turns: 10.0, random: 0.4 }, Step::Clump { amount: 0.3, shape: 0.6 }, Step::Frizz { amount: 0.2 }]),
            "beard" => base(25_000, 0.06 * s, vec![Step::Comb { direction: down, strength: 0.8 }, Step::Curl { radius: 0.004 * s, turns: 2.0, random: 0.5 }, Step::Clump { amount: 0.3, shape: 1.0 }, Step::Frizz { amount: 0.1 }]),
            "eyebrows" => Groom { segments: 5, root_width: 0.25, ..base(2_500, 0.025 * s, vec![Step::Comb { direction: [1.0, -0.2, 0.0], strength: 0.9 }, Step::Lift { amount: 0.1 }, Step::Clump { amount: 0.2, shape: 1.0 }]) },
            "eyelashes" => Groom { segments: 5, root_width: 0.3, melanin: 0.9, ..base(300, 0.02 * s, vec![Step::Lift { amount: 0.8 }, Step::Curl { radius: 0.01 * s, turns: 0.25, random: 0.1 }]) },
            "fur_short" => Groom { length_noise: 0.3, root_width: 0.4, ..base(120_000, 0.03 * s, vec![Step::Comb { direction: [0.0, 0.5, 1.0], strength: 0.7 }, Step::Clump { amount: 0.4, shape: 1.0 }, Step::Frizz { amount: 0.1 }]) },
            "fur_long" | "pelt" => Groom { length_noise: 0.35, ..base(150_000, 0.1 * s, vec![Step::Comb { direction: [0.0, 0.6, 1.0], strength: 0.7 }, Step::Gravity { amount: 0.5 }, Step::Clump { amount: 0.6, shape: 1.3 }, Step::Noise { amount: 0.1, size: 0.15 * s }, Step::Frizz { amount: 0.08 }]) },
            "mane" => base(40_000, 0.4 * s, vec![Step::Comb { direction: down, strength: 0.5 }, Step::Gravity { amount: 0.7 }, Step::Wave { amplitude: 0.02 * s, waves: 2.0 }, Step::Clump { amount: 0.7, shape: 1.4 }, Step::Noise { amount: 0.15, size: 0.2 * s }]),
            "grass" => Groom { melanin: 0.0, colour: Some([0.06, 0.16, 0.03]), tip_colour: Some([0.22, 0.3, 0.08]), root_width: 1.2, tip_width: 0.0, segments: 4, shine: 0.3, ..base(80_000, 0.05 * s, vec![Step::Lift { amount: 0.95 }, Step::Noise { amount: 0.25, size: 0.1 * s }, Step::Gravity { amount: 0.15 }]) },
            "feathers_down" => Groom { root_width: 0.8, melanin: 0.1, length_noise: 0.2, ..base(60_000, 0.03 * s, vec![Step::Lift { amount: 0.6 }, Step::Frizz { amount: 0.4 }, Step::Clump { amount: 0.2, shape: 0.7 }]) },
            _ => return None,
        })
    }

    pub const PRESETS: &'static [&'static str] = &["short_hair", "long_straight", "wavy", "curly", "coily", "beard", "eyebrows", "eyelashes", "fur_short", "fur_long", "mane", "grass", "feathers_down"];

    /// Linear RGB at root and tip.
    pub fn colours(&self) -> (V3, V3) {
        let root = self.colour.unwrap_or_else(|| hair_colour(self.melanin, self.redness));
        let tip = self.tip_colour.unwrap_or_else(|| scale(root, 1.15));
        (root, tip)
    }
}

/// Hair colour from pigment: light passing through a fibre with this much
/// eumelanin (brown–black) and pheomelanin (red), as linear RGB.
pub fn hair_colour(melanin: f32, redness: f32) -> V3 {
    let eu = melanin.clamp(0.0, 1.0).powf(1.5) * 12.0;
    let pheo = redness.clamp(0.0, 1.0) * 3.0;
    // Absorption per unit of each pigment (red, green, blue).
    let sigma = [0.419 * eu + 0.187 * pheo, 0.697 * eu + 0.4 * pheo, 1.37 * eu + 1.05 * pheo];
    let k = 0.6;
    [(-sigma[0] * k).exp() * 0.9, (-sigma[1] * k).exp() * 0.85, (-sigma[2] * k).exp() * 0.8]
}

/// Where a strand grows: a triangle and a place on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Root {
    pub tri: u32,
    pub bary: [f32; 3],
}

/// Grown strands: every strand `per` points long, strand after strand.
#[derive(Clone, Debug, PartialEq)]
pub struct Strands {
    pub roots: Vec<Root>,
    pub points: Vec<V3>,
    pub per: usize,
    /// Each strand's colour scale (natural variation, 0.8–1.2).
    pub tint: Vec<f32>,
}

impl Strands {
    pub fn count(&self) -> usize {
        self.roots.len()
    }
    pub fn strand(&self, i: usize) -> &[V3] {
        &self.points[i * self.per..(i + 1) * self.per]
    }
}

fn surface_point(m: &Mesh, r: &Root) -> (V3, V3) {
    let [a, b, c] = m.tri(r.tri as usize);
    let p = add(add(scale(m.positions[a], r.bary[0]), scale(m.positions[b], r.bary[1])), scale(m.positions[c], r.bary[2]));
    let n = if m.normals.len() == m.positions.len() {
        normalise(add(add(scale(m.normals[a], r.bary[0]), scale(m.normals[b], r.bary[1])), scale(m.normals[c], r.bary[2])))
    } else {
        m.face_normal(r.tri as usize)
    };
    (p, n)
}

/// Roots spread over `faces` by area (blue-noise-ish: jittered and
/// thinned so they don't bunch).
fn scatter(m: &Mesh, faces: &[usize], count: usize, rng: &mut Rng) -> Vec<Root> {
    let areas: Vec<f32> = faces.iter().map(|&t| {
        let [a, b, c] = m.tri(t);
        0.5 * length(cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a])))
    }).collect();
    let total: f32 = areas.iter().sum();
    if total <= 0.0 || faces.is_empty() {
        return Vec::new();
    }
    let mut acc = Vec::with_capacity(areas.len());
    let mut s = 0.0;
    for a in &areas {
        s += a;
        acc.push(s);
    }
    // Stratified: one root per equal slice of the total area.
    (0..count)
        .map(|i| {
            let x = (i as f32 + rng.next()) / count as f32 * total;
            let k = acc.partition_point(|&v| v < x).min(faces.len() - 1);
            let (mut u, mut v) = (rng.next(), rng.next());
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            Root { tri: faces[k] as u32, bary: [1.0 - u - v, u, v] }
        })
        .collect()
}

/// A strand straight out from the surface, `len` long.
fn straight(p: V3, n: V3, len: f32, per: usize) -> Vec<V3> {
    (0..per).map(|i| add(p, scale(n, len * i as f32 / (per - 1) as f32))).collect()
}

/// Keeps each segment its length after a step bent the strand.
fn restore_lengths(s: &mut [V3], seg: f32) {
    for i in 1..s.len() {
        let d = sub(s[i], s[i - 1]);
        let l = length(d);
        if l > 1e-9 {
            s[i] = add(s[i - 1], scale(d, seg / l));
        }
    }
}

fn shape_guide(s: &mut [V3], n: V3, steps: &[Step], seed: u32, rng: &mut Rng) {
    let per = s.len();
    let len: f32 = s.windows(2).map(|w| length(sub(w[1], w[0]))).sum();
    let mut seg = len / (per - 1).max(1) as f32;
    let root = s[0];
    // A frame round the root normal.
    let side = normalise(cross(n, if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] }));
    let side2 = cross(n, side);
    for st in steps {
        match st {
            Step::Comb { direction, strength } => {
                // Lay toward the direction projected onto the surface.
                let d = normalise(sub(*direction, scale(n, dot(*direction, n))));
                if length(d) < 1e-6 {
                    continue;
                }
                let k = strength.clamp(0.0, 1.0);
                for i in 1..per {
                    let t = i as f32 / (per - 1) as f32;
                    let cur = normalise(sub(s[i], s[i - 1]));
                    let want = normalise(add(scale(cur, 1.0 - k * (0.5 + 0.5 * t)), scale(add(d, scale(n, 0.15)), k * (0.5 + 0.5 * t))));
                    s[i] = add(s[i - 1], scale(want, seg));
                }
            }
            Step::Gravity { amount } => {
                let down = [0.0, 1.0, 0.0];
                for i in 1..per {
                    let t = i as f32 / (per - 1) as f32;
                    let cur = normalise(sub(s[i], s[i - 1]));
                    let k = (amount * t).clamp(0.0, 1.0);
                    s[i] = add(s[i - 1], scale(normalise(lerp(cur, down, k)), seg));
                }
            }
            Step::Curl { radius, turns, random } => {
                let ph = rng.range(0.0, std::f32::consts::TAU);
                let r = radius * (1.0 + random * rng.range(-1.0, 1.0));
                let tu = turns * (1.0 + random * rng.range(-0.5, 0.5));
                let base: Vec<V3> = s.to_vec();
                for i in 1..per {
                    let t = i as f32 / (per - 1) as f32;
                    let a = ph + std::f32::consts::TAU * tu * t;
                    let dir = normalise(sub(base[i.min(per - 1)], base[i - 1]));
                    let u = normalise(cross(dir, side2));
                    let w = cross(dir, u);
                    let ramp = (t * 4.0).min(1.0);
                    s[i] = add(base[i], scale(add(scale(u, a.cos()), scale(w, a.sin())), r * ramp));
                }
                // Curls shorten the strand: keep the segment count, lose reach.
                seg *= 1.0;
            }
            Step::Wave { amplitude, waves } => {
                let ph = rng.range(0.0, std::f32::consts::TAU);
                for (i, p) in s.iter_mut().enumerate().skip(1) {
                    let t = i as f32 / (per - 1) as f32;
                    *p = add(*p, scale(side, amplitude * (ph + std::f32::consts::TAU * waves * t).sin() * t.sqrt()));
                }
            }
            Step::Frizz { amount } => {
                for (i, p) in s.iter_mut().enumerate().skip(1) {
                    let t = i as f32 / (per - 1) as f32;
                    let j = [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)];
                    *p = add(*p, scale(j, amount * seg * t * 1.5));
                }
            }
            Step::Twist { degrees } => {
                let axis = normalise(sub(s[per - 1], root));
                for (i, p) in s.iter_mut().enumerate().skip(1) {
                    let t = i as f32 / (per - 1) as f32;
                    let a = degrees.to_radians() * t;
                    let d = sub(*p, root);
                    // Rodrigues about the strand's axis.
                    let (sn, cs) = a.sin_cos();
                    let rot = add(add(scale(d, cs), scale(cross(axis, d), sn)), scale(axis, dot(axis, d) * (1.0 - cs)));
                    *p = add(root, rot);
                }
            }
            Step::Cut { length: l, fraction } => {
                let target = l.unwrap_or(len) * fraction.unwrap_or(1.0);
                seg = (target / (per - 1) as f32).min(seg);
            }
            Step::Lift { amount } => {
                for i in 1..per {
                    let t = i as f32 / (per - 1) as f32;
                    let cur = normalise(sub(s[i], s[i - 1]));
                    let k = (amount * (1.0 - t * 0.7)).clamp(0.0, 1.0);
                    s[i] = add(s[i - 1], scale(normalise(lerp(cur, n, k)), seg));
                }
            }
            Step::Part { at, along, strength } => {
                let a = normalise(*along);
                let off = sub(root, *at);
                let away = normalise(sub(off, scale(a, dot(off, a))));
                let away = normalise(sub(away, scale(n, dot(away, n))));
                if length(away) < 1e-6 {
                    continue;
                }
                for i in 1..per {
                    let cur = normalise(sub(s[i], s[i - 1]));
                    s[i] = add(s[i - 1], scale(normalise(lerp(cur, away, strength.clamp(0.0, 1.0) * 0.6)), seg));
                }
            }
            Step::Noise { amount, size } => {
                for i in 1..per {
                    let p = s[i];
                    let q = scale(p, 1.0 / size.max(1.0));
                    let d = [noise::perlin(q, seed), noise::perlin(q, seed + 1), noise::perlin(q, seed + 2)];
                    let t = i as f32 / (per - 1) as f32;
                    s[i] = add(p, scale(d, amount * len * t));
                }
            }
            Step::Clump { .. } => {}
        }
        if !matches!(st, Step::Curl { .. } | Step::Wave { .. } | Step::Frizz { .. } | Step::Twist { .. } | Step::Noise { .. }) {
            restore_lengths(s, seg);
        } else {
            // Bends that add sideways reach keep the strand from growing.
            let l2: f32 = s.windows(2).map(|w| length(sub(w[1], w[0]))).sum();
            if l2 > len * 1.6 {
                restore_lengths(s, seg * 1.6 * len / l2);
            }
        }
    }
    let _ = side2;
}

/// Grows a groom on `faces` of a model.
pub fn grow(m: &Mesh, faces: &[usize], g: &Groom) -> Strands {
    let per = g.segments.clamp(2, 32) + 1;
    let mut rng = Rng::new(g.seed);
    let count = g.count.clamp(1, 2_000_000);
    let nguide = g.guides.clamp(1, count);
    let guide_roots = scatter(m, faces, nguide, &mut rng);
    if guide_roots.is_empty() {
        return Strands { roots: Vec::new(), points: Vec::new(), per, tint: Vec::new() };
    }
    let seed = g.seed as u32;
    let length_at = |p: V3, rng: &mut Rng| {
        let n = if g.length_noise > 0.0 { 1.0 + g.length_noise * noise::fbm(scale(p, 1.0 / (g.length * 4.0).max(1.0)), 3, 0.5, seed) } else { 1.0 };
        (g.length * n * (1.0 + g.length_random * rng.range(-1.0, 1.0))).max(0.0)
    };
    // Guides.
    let mut guides: Vec<Vec<V3>> = Vec::with_capacity(nguide);
    let mut guide_pos: Vec<(V3, V3)> = Vec::with_capacity(nguide);
    for r in &guide_roots {
        let (p, n) = surface_point(m, r);
        let len = length_at(p, &mut rng);
        let mut s = straight(p, n, len, per);
        shape_guide(&mut s, n, &g.steps, seed, &mut rng);
        guides.push(s);
        guide_pos.push((p, n));
    }
    let (clump, clump_shape) = g.steps.iter().find_map(|s| if let Step::Clump { amount, shape } = s { Some((*amount, *shape)) } else { None }).unwrap_or((0.0, 1.0));
    let frizz = g.steps.iter().find_map(|s| if let Step::Frizz { amount } = s { Some(*amount) } else { None }).unwrap_or(0.0);
    // Guide lookup grid.
    let (lo, hi) = m.bounds();
    let area: f32 = faces.iter().map(|&t| {
        let [a, b, c] = m.tri(t);
        0.5 * length(cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a])))
    }).sum();
    let spacing = (area / nguide as f32).sqrt().max(1e-3);
    let mut grid = crate::spatial::PointGrid::new(&[], spacing * 1.5);
    let gp: Vec<V3> = guide_pos.iter().map(|x| x.0).collect();
    for (i, p) in gp.iter().enumerate() {
        grid.insert(i as u32, *p);
    }
    let _ = (lo, hi);
    let child_roots = scatter(m, faces, count - nguide, &mut rng);
    let mut roots = guide_roots.clone();
    let mut points: Vec<V3> = guides.iter().flatten().copied().collect();
    let mut tint: Vec<f32> = (0..nguide).map(|_| rng.range(0.85, 1.15)).collect();
    for r in child_roots {
        let (p, n) = surface_point(m, &r);
        // The nearest guides, weighted (the closest wins the clump).
        let mut near = grid.within(&gp, p, spacing * 2.5);
        if near.is_empty() {
            near = vec![(0..gp.len()).min_by(|&a, &b| length(sub(gp[a], p)).total_cmp(&length(sub(gp[b], p)))).unwrap_or(0) as u32];
        }
        near.sort_by(|a, b| length(sub(gp[*a as usize], p)).total_cmp(&length(sub(gp[*b as usize], p))));
        near.truncate(3);
        let ws: Vec<f32> = near.iter().map(|&i| 1.0 / (length(sub(gp[i as usize], p)) + spacing * 0.2)).collect();
        let wsum: f32 = ws.iter().sum();
        let lscale = length_at(p, &mut rng) / g.length.max(1e-6);
        let main = near[0] as usize;
        let mut s = Vec::with_capacity(per);
        for i in 0..per {
            let t = i as f32 / (per - 1) as f32;
            // Interpolated shape: each guide's offset from its root.
            let mut off = [0.0; 3];
            for (k, &gi) in near.iter().enumerate() {
                let gs = &guides[gi as usize];
                off = add(off, scale(sub(gs[i], gs[0]), ws[k] / wsum));
            }
            let mut q = add(p, scale(off, lscale));
            // Clump toward the main guide's strand.
            if clump > 0.0 {
                let gs = &guides[main];
                let target = add(gs[i], scale(sub(p, gs[0]), 0.0));
                let k = clump * t.powf(clump_shape.max(0.05));
                q = lerp(q, target, k.clamp(0.0, 0.98));
            }
            if frizz > 0.0 && i > 0 {
                q = add(q, scale([rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)], frizz * g.length / per as f32 * t));
            }
            s.push(q);
        }
        // Lift the root off the surface along its own normal.
        let lift = n;
        s[0] = p;
        let first = sub(s[1], s[0]);
        if dot(first, lift) < 0.0 {
            s[1] = add(s[0], scale(normalise(add(first, scale(lift, -2.0 * dot(first, lift)))), length(first)));
        }
        roots.push(r);
        points.extend(s);
        tint.push(rng.range(0.85, 1.15));
    }
    Strands { roots, points, per, tint }
}

/// Strands as ribbons for drawing: two crossed ribbons per strand,
/// tapering from root to tip, in one hair material.
pub fn ribbons(s: &Strands, g: &Groom) -> Mesh {
    let mut m = Mesh::default();
    let (rc, tc) = g.colours();
    let per = s.per;
    for k in 0..s.count() {
        let st = s.strand(k);
        let tint = s.tint.get(k).copied().unwrap_or(1.0);
        let mut prev_side = [1.0, 0.0, 0.0];
        for cross_i in 0..2 {
            let base = m.positions.len() as u32;
            for i in 0..per {
                let t = i as f32 / (per - 1) as f32;
                let dir = normalise(if i + 1 < per { sub(st[i + 1], st[i]) } else { sub(st[i], st[i - 1]) });
                let mut side = normalise(cross(dir, if cross_i == 0 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] }));
                if length(side) < 0.5 {
                    side = prev_side;
                }
                prev_side = side;
                let w = (g.root_width + (g.tip_width - g.root_width) * t) * 0.5;
                let n = normalise(cross(side, dir));
                for sgn in [-1.0f32, 1.0] {
                    m.positions.push(add(st[i], scale(side, w * sgn)));
                    m.normals.push(n);
                    // u across, v root (0) to tip (1); the tint rides in u.
                    m.uvs.push([if sgn < 0.0 { 0.0 } else { tint.min(1.0) }, t]);
                }
            }
            for i in 0..per as u32 - 1 {
                let (a, b, c, d) = (base + 2 * i, base + 2 * i + 1, base + 2 * i + 3, base + 2 * i + 2);
                m.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    m.materials = vec![Material { name: format!("Hair: {}", g.name), base_colour: [1.0, 1.0, 1.0, 1.0], metallic: 0.0, roughness: (1.0 - g.shine * 0.75).clamp(0.15, 1.0), subsurface: 0.0, ..Material::default() }];
    // A root-to-tip colour ramp as a small texture down v.
    let h = 32u32;
    let mut rgba = Vec::with_capacity(h as usize * 8);
    let enc = |v: f32| ((if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 }).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let c = lerp(rc, tc, t);
        for x in 0..2 {
            let k = if x == 0 { 0.9 } else { 1.1 };
            rgba.extend_from_slice(&[enc(c[0] * k), enc(c[1] * k), enc(c[2] * k), 255]);
        }
    }
    m.materials[0].texture = Some(studio_mesh::Texture { width: 2, height: h, rgba });
    m.single_group();
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    fn ball() -> Mesh {
        prim::quad_sphere(100.0, 8).to_mesh(180.0, crate::NormalMode::Angle)
    }

    #[test]
    fn every_preset_grows_on_the_top_of_a_head() {
        let m = ball();
        let top: Vec<usize> = (0..m.triangles()).filter(|&t| m.face_normal(t)[1] < -0.3).collect();
        for name in Groom::PRESETS {
            let mut g = Groom::preset(name, 400.0).unwrap();
            g.count = 800;
            g.guides = 60;
            let s = grow(&m, &top, &g);
            assert_eq!(s.count(), 800, "{name}");
            assert_eq!(s.points.len(), 800 * s.per);
            // Roots on the surface, strands of about the asked length.
            for k in 0..s.count() {
                let st = s.strand(k);
                assert!((length(st[0]) - 100.0).abs() < 2.0, "{name}: root off the surface");
                let l: f32 = st.windows(2).map(|w| length(sub(w[1], w[0]))).sum();
                assert!(l < g.length * 3.5 + 1.0, "{name}: strand {l} vs {}", g.length);
                assert!(st.iter().all(|p| p.iter().all(|v| v.is_finite())));
            }
            let r = ribbons(&s, &g);
            assert!(r.triangles() > 0);
        }
    }

    #[test]
    fn gravity_pulls_long_hair_down_and_curls_stay_close() {
        let m = ball();
        let side: Vec<usize> = (0..m.triangles()).filter(|&t| m.face_normal(t)[0] > 0.8).collect();
        let g = Groom { count: 200, guides: 30, ..Groom::preset("long_straight", 400.0).unwrap() };
        let s = grow(&m, &side, &g);
        let fall: f32 = (0..s.count()).map(|k| s.strand(k)[s.per - 1][1] - s.strand(k)[0][1]).sum::<f32>() / s.count() as f32;
        assert!(fall > g.length * 0.3, "hangs: {fall}");
        let c = Groom { count: 200, guides: 30, ..Groom::preset("coily", 400.0).unwrap() };
        let sc = grow(&m, &side, &c);
        let reach: f32 = (0..sc.count()).map(|k| length(sub(sc.strand(k)[sc.per - 1], sc.strand(k)[0]))).sum::<f32>() / sc.count() as f32;
        assert!(reach < c.length * 1.2);
    }

    #[test]
    fn melanin_makes_dark_and_red_hair() {
        let black = hair_colour(1.0, 0.0);
        let blond = hair_colour(0.15, 0.1);
        let ginger = hair_colour(0.3, 1.0);
        assert!(black[0] < 0.05 && blond[0] > 0.3);
        assert!(ginger[0] > ginger[2] * 2.0);
    }
}
