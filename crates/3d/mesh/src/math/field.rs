// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Implicit solids: the points where a formula f(x, y, z) is 0 or less.
//! A [`Field`] is a part of the blended mix ([`crate::sdf`]), so a gyroid
//! lattice can fill a box, a blob can melt into a capsule, a fractal can
//! carve a sphere. The formula works in its own units over a domain box
//! (by default -1..1 on each axis) that the part's `size` spans, and the
//! solid is cut to that box (or the ball inside it), so even an endless
//! lattice comes out a closed solid.
//!
//! A formula's values need not be distances: each field measures how
//! steep its formula is near its surface once, and scales it to about
//! pixels, so blends and carving behave as with the other parts.
//!
//! Formulas see `x y z` (formula space: z up), `t` (seconds, for shapes
//! that move) and `r` (distance from the domain's centre). Several
//! formulas with a `weight` morph from one to the next as the weight runs
//! from 0 to the number of formulas less one.

use super::{Program, from_studio, size_from_studio};
use crate::V3;
use crate::sdf::FieldFn;

/// The names a field's formula reads, in order.
pub const NAMES: &[&str] = &["x", "y", "z", "t", "r"];

/// What the solid is cut to: its domain box, or the ball in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clip {
    Box,
    Ball,
}

impl Clip {
    pub fn parse(s: &str) -> Option<Clip> {
        Some(match s {
            "box" | "cube" => Clip::Box,
            "ball" | "sphere" | "round" => Clip::Ball,
            _ => return None,
        })
    }
}

/// An implicit solid.
pub struct Field {
    formulas: Vec<Program>,
    weight: Option<Program>,
    t: f64,
    lo: [f64; 3],
    hi: [f64; 3],
    clip: Clip,
    /// 1 / how steep the formula is at its surface: values × this ≈
    /// distances in formula units.
    norm: f64,
}

impl Field {
    /// A field from `formulas` (over [`NAMES`]; several with a `weight`
    /// morph between them) over the domain `lo..hi`, at time `t`.
    pub fn new(formulas: Vec<Program>, weight: Option<Program>, lo: [f64; 3], hi: [f64; 3], clip: Clip, t: f64) -> Result<Field, String> {
        if formulas.is_empty() {
            return Err("a field needs a formula".into());
        }
        if (0..3).any(|i| !(hi[i] > lo[i]) || !lo[i].is_finite() || !hi[i].is_finite()) {
            return Err("the domain's low corner must be below its high corner on every axis".into());
        }
        let mut f = Field { formulas, weight, t, lo, hi, clip, norm: 1.0 };
        f.norm = 1.0 / f.steepness();
        Ok(f)
    }

    /// The same field at another time (its scale kept, so frames match).
    pub fn at_time(&self, t: f64) -> Field {
        Field { formulas: self.formulas.clone(), weight: self.weight.clone(), t, lo: self.lo, hi: self.hi, clip: self.clip, norm: self.norm }
    }

    /// Whether it changes with `t`.
    pub fn moves(&self) -> bool {
        self.formulas.iter().chain(self.weight.iter()).any(|p| p.uses(3))
    }

    /// Formula evaluations per sample.
    pub fn cost(&self) -> u64 {
        (self.formulas.len().min(2) + self.weight.is_some() as usize) as u64
    }

    /// The domain, low and high corners (formula space).
    pub fn domain(&self) -> ([f64; 3], [f64; 3]) {
        (self.lo, self.hi)
    }

    /// The formula's value at `m` (formula space).
    pub fn value(&self, m: [f64; 3]) -> f64 {
        let c = [(self.lo[0] + self.hi[0]) * 0.5, (self.lo[1] + self.hi[1]) * 0.5, (self.lo[2] + self.hi[2]) * 0.5];
        let r = ((m[0] - c[0]).powi(2) + (m[1] - c[1]).powi(2) + (m[2] - c[2]).powi(2)).sqrt();
        let vars = [m[0], m[1], m[2], self.t, r];
        if self.formulas.len() == 1 {
            return self.formulas[0].eval(&vars);
        }
        let w = self.weight.as_ref().map_or(0.0, |p| p.eval(&vars)).clamp(0.0, (self.formulas.len() - 1) as f64);
        let k = (w.floor() as usize).min(self.formulas.len() - 2);
        let u = w - k as f64;
        if u <= 0.0 {
            return self.formulas[k].eval(&vars);
        }
        if u >= 1.0 {
            return self.formulas[k + 1].eval(&vars);
        }
        self.formulas[k].eval(&vars) * (1.0 - u) + self.formulas[k + 1].eval(&vars) * u
    }

    /// How steep the formula typically is where it crosses 0: the middle
    /// of the gradients at the samples closest to its surface.
    fn steepness(&self) -> f64 {
        let n = 2048;
        let h = [(self.hi[0] - self.lo[0]) * 0.5, (self.hi[1] - self.lo[1]) * 0.5, (self.hi[2] - self.lo[2]) * 0.5];
        let eps = h[0].max(h[1]).max(h[2]) * 1e-3;
        let mut found: Vec<(f64, f64)> = Vec::with_capacity(n);
        for k in 1..=n {
            // A Halton sequence fills the box evenly.
            let q = [halton(k, 2), halton(k, 3), halton(k, 5)];
            let p = [self.lo[0] + q[0] * h[0] * 2.0, self.lo[1] + q[1] * h[1] * 2.0, self.lo[2] + q[2] * h[2] * 2.0];
            let f = self.value(p);
            let mut g = 0.0;
            for i in 0..3 {
                let (mut a, mut b) = (p, p);
                a[i] += eps;
                b[i] -= eps;
                g += ((self.value(a) - self.value(b)) / (2.0 * eps)).powi(2);
            }
            let g = g.sqrt();
            if f.is_finite() && g.is_finite() && g > 0.0 {
                found.push((f.abs(), g));
            }
        }
        if found.is_empty() {
            return 1.0;
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        let keep = (n / 4).max(1).min(found.len());
        let near = &mut found[..keep];
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        near[near.len() / 2].1.clamp(1e-6, 1e6)
    }
}

fn halton(mut i: usize, base: usize) -> f64 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= base as f64;
        r += f * (i % base) as f64;
        i /= base;
    }
    r
}

impl FieldFn for Field {
    fn distance(&self, q: V3, size: V3) -> f32 {
        let s = size_from_studio(size).map(|v| v.max(1e-6) * 0.5);
        let mq = from_studio(q);
        let h = [(self.hi[0] - self.lo[0]) * 0.5, (self.hi[1] - self.lo[1]) * 0.5, (self.hi[2] - self.lo[2]) * 0.5];
        let c = [self.lo[0] + h[0], self.lo[1] + h[1], self.lo[2] + h[2]];
        let m = [c[0] + mq[0] / s[0] * h[0], c[1] + mq[1] / s[1] * h[1], c[2] + mq[2] / s[2] * h[2]];
        // Pixels per formula unit: the least, so distances are never too long.
        let k = (s[0] / h[0]).min(s[1] / h[1]).min(s[2] / h[2]);
        let f = self.value(m) * self.norm * k;
        // Undefined (a square root of less than 0, say) counts as outside.
        let f = if f.is_nan() { k } else { f };
        let cut = match self.clip {
            Clip::Box => {
                let e = [mq[0].abs() - s[0], mq[1].abs() - s[1], mq[2].abs() - s[2]];
                (e[0].max(0.0).powi(2) + e[1].max(0.0).powi(2) + e[2].max(0.0).powi(2)).sqrt() + e[0].max(e[1]).max(e[2]).min(0.0)
            }
            Clip::Ball => (((mq[0] / s[0]).powi(2) + (mq[1] / s[1]).powi(2) + (mq[2] / s[2]).powi(2)).sqrt() - 1.0) * s[0].min(s[1]).min(s[2]),
        };
        f.max(cut).clamp(-1e9, 1e9) as f32
    }
}

/// A named implicit solid: its settings (bound by name in front of the
/// formula, so any can be changed), the formula and the domain's half
/// side.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [(&'static str, f64)],
    pub formula: &'static str,
    pub bounds: f64,
    pub clip: Clip,
}

/// The implicit presets.
pub const PRESETS: &[Preset] = &[
    Preset { name: "gyroid", about: "Schoen's gyroid: a triply periodic minimal surface lattice (sheet of `thickness`, or `solid: 1` for one of its two networks), `cells` across", params: &[("cells", 2.0), ("thickness", 0.3), ("solid", 0.0)], formula: "k = cells * pi; g = gyroid(k * x, k * y, k * z); solid > 0 ? g : abs(g) - thickness", bounds: 1.0, clip: Clip::Box },
    Preset { name: "schwarz_p", about: "Schwarz's primitive surface: a lattice of rounded cubic cells", params: &[("cells", 2.0), ("thickness", 0.3), ("solid", 0.0)], formula: "k = cells * pi; g = schwarz_p(k * x, k * y, k * z); solid > 0 ? g : abs(g) - thickness", bounds: 1.0, clip: Clip::Box },
    Preset { name: "diamond", about: "Schwarz's diamond surface: a tetrahedral lattice", params: &[("cells", 2.0), ("thickness", 0.25), ("solid", 0.0)], formula: "k = cells * pi; g = schwarz_d(k * x, k * y, k * z); solid > 0 ? g : abs(g) - thickness", bounds: 1.0, clip: Clip::Box },
    Preset { name: "neovius", about: "Neovius's surface: chunky cells joined by tubes", params: &[("cells", 1.5), ("thickness", 0.6), ("solid", 0.0)], formula: "k = cells * pi; g = neovius(k * x, k * y, k * z); solid > 0 ? g : abs(g) - thickness", bounds: 1.0, clip: Clip::Box },
    Preset { name: "lidinoid", about: "the lidinoid minimal surface: a twisting, coral-like lattice", params: &[("cells", 1.0), ("thickness", 0.15), ("solid", 0.0)], formula: "k = cells * pi; g = lidinoid(k * x, k * y, k * z); solid > 0 ? g : abs(g) - thickness", bounds: 1.0, clip: Clip::Box },
    Preset { name: "strut_lattice", about: "a cubic lattice of round struts (a scaffold, a 3D-printed infill)", params: &[("cells", 4.0), ("width", 0.12)], formula: "qx = fract(x * cells / 2 + 0.5) - 0.5; qy = fract(y * cells / 2 + 0.5) - 0.5; qz = fract(z * cells / 2 + 0.5) - 0.5\nmin(length(qx, qy), length(qy, qz), length(qz, qx)) - width", bounds: 1.0, clip: Clip::Box },
    Preset { name: "foam", about: "Voronoi foam: the walls between random cells, as in a sponge or bone", params: &[("cells", 3.0), ("width", 0.08), ("seed", 0.0)], formula: "width - voronoi_edge(x * cells + seed, y * cells, z * cells)", bounds: 1.0, clip: Clip::Ball },
    Preset { name: "blob", about: "a ball lumped by noise, slowly churning when `speed` > 0", params: &[("radius", 0.7), ("amount", 0.25), ("frequency", 2.5), ("speed", 0.0), ("seed", 0.0)], formula: "sphere(x, y, z, radius) - amount * fbm(x * frequency + seed, y * frequency, z * frequency + t * speed, 4)", bounds: 1.0, clip: Clip::Box },
    Preset { name: "rock", about: "a craggy boulder: a squashed ball with sharp noise", params: &[("radius", 0.75), ("amount", 0.35), ("frequency", 1.8), ("seed", 3.0)], formula: "d = sphere(x, y * 1.15, z * 1.4, radius); n = fbm(x * frequency + seed, y * frequency, z * frequency, 6); d + amount * abs(n) - amount * 0.3", bounds: 1.0, clip: Clip::Box },
    Preset { name: "ripple_sphere", about: "a ball with standing waves over it (`waves` round, `amount` deep), rippling with `speed`", params: &[("radius", 0.75), ("waves", 8.0), ("amount", 0.06), ("speed", 0.0)], formula: "az = atan2(y, x); el = acos(clamp(z / max(r, 1e-9), -1, 1)); r - radius - amount * sin(waves * az + t * speed) * sin(waves * el)", bounds: 1.0, clip: Clip::Box },
    Preset { name: "metaballs", about: "four soft balls orbiting and merging like mercury as t runs (`speed`)", params: &[("size", 0.32), ("speed", 1.0), ("threshold", 1.0)], formula: "a = t * speed\nb1 = size^2 / ((x - 0.4 * sin(a))^2 + (y - 0.2 * cos(a * 1.3))^2 + z^2)\nb2 = size^2 / ((x + 0.35 * cos(a * 0.7))^2 + (y + 0.3 * sin(a))^2 + (z - 0.2 * sin(a * 1.1))^2)\nb3 = size^2 / (x^2 + (y - 0.4 * sin(a * 0.9 + 2))^2 + (z + 0.35 * cos(a * 0.8))^2)\nb4 = size^2 / ((x - 0.3 * cos(a * 1.2 + 1))^2 + (y + 0.1)^2 + (z - 0.4 * sin(a * 0.6 + 4))^2)\nthreshold - (b1 + b2 + b3 + b4)", bounds: 1.0, clip: Clip::Box },
    Preset { name: "superquadric", about: "a rounded cube to a star-like solid: |x|^p + |y|^p + |z|^p (p 2 a ball, 4 a soft cube, under 1 a spiky star)", params: &[("p", 4.0), ("radius", 0.9)], formula: "(abs(x)^p + abs(y)^p + abs(z)^p)^(1 / p) - radius", bounds: 1.0, clip: Clip::Box },
    Preset { name: "heart", about: "Taubin's heart surface, upright", params: &[], formula: "a = x^2 + 2.25 * y^2 + z^2 - 1; a^3 - x^2 * z^3 - 0.1125 * y^2 * z^3", bounds: 1.3, clip: Clip::Box },
    Preset { name: "mandelbulb", about: "the Mandelbulb fractal (power 8; try 3 to 12), its detail by `iterations`", params: &[("power", 8.0), ("iterations", 10.0)], formula: "mandelbulb(x, y, z, power, iterations)", bounds: 1.2, clip: Clip::Box },
    Preset { name: "mandelbox", about: "the Mandelbox fractal: folded, boxy, architectural", params: &[("scale", -1.5), ("iterations", 12.0)], formula: "mandelbox(x, y, z, scale, iterations)", bounds: 2.1, clip: Clip::Box },
    Preset { name: "menger", about: "the Menger sponge: a cube with ever smaller square tunnels (`iterations` 1 to 7)", params: &[("iterations", 4.0)], formula: "menger(x, y, z, iterations)", bounds: 1.02, clip: Clip::Box },
    Preset { name: "sierpinski", about: "the Sierpinski tetrahedron, of ever smaller tetrahedra", params: &[("iterations", 7.0)], formula: "sierpinski(x, y, z, iterations)", bounds: 1.02, clip: Clip::Box },
];

/// The preset called `name`.
pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Settings bound in front of a formula: the defaults, with `given` (by
/// name) changing them. A given name the formula has no setting for is
/// an error that lists those it has.
pub fn bind(what: &str, params: &[(&str, f64)], given: &[(String, f64)], body: &str) -> Result<String, String> {
    let mut out = String::new();
    for (k, _) in given {
        if !params.iter().any(|(n, _)| n == k) {
            let names: Vec<&str> = params.iter().map(|p| p.0).collect();
            return Err(if names.is_empty() { format!("{what} has no settings (\"{k}\" given)") } else { format!("{what} has no setting \"{k}\"; its settings are {}", names.join(", ")) });
        }
    }
    for (n, d) in params {
        let v = given.iter().find(|(k, _)| k == n).map_or(*d, |g| g.1);
        if !v.is_finite() {
            return Err(format!("{what}: {n} must be a number"));
        }
        // `;` and not a line break: a body starting with `-` would go on
        // from the binding before it.
        out.push_str(&format!("{n} = {v:?}; "));
    }
    out.push_str(body);
    Ok(out)
}

/// A grid step (px) for fields spanning `extent` px at `detail` 1 to 6,
/// coarse enough that the grid costs at most [`super::MAX_EVALS`] when
/// each sample costs `cost` evaluations.
pub fn cell_for(extent: V3, detail: usize, cost: u64) -> f32 {
    let across = [48.0f32, 72.0, 100.0, 140.0, 180.0, 240.0][detail.clamp(1, 6) - 1];
    let big = extent[0].max(extent[1]).max(extent[2]).max(1.0);
    let mut cell = big / across;
    let points = |c: f32| (extent[0] / c + 3.0) * (extent[1] / c + 3.0) * (extent[2] / c + 3.0);
    while points(cell) as f64 * cost.max(1) as f64 * 1.15 > super::MAX_EVALS as f64 {
        cell *= 1.1;
    }
    cell.max(big / crate::sdf::MAX_GRID)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Material;
    use crate::math::{compile, open_edges};
    use crate::sdf::{Part, Shape, build, build_counted};
    use std::sync::Arc;

    fn field(src: &str, bounds: f64, clip: Clip) -> Arc<Field> {
        Arc::new(Field::new(vec![compile("test", src, NAMES).unwrap()], None, [-bounds; 3], [bounds; 3], clip, 0.0).unwrap())
    }

    #[test]
    fn a_sphere_formula_matches_the_ellipsoid_part() {
        // Radius 1 in a domain of half side 1.25, the part 125 px across:
        // a ball 100 px across, as Shape::Ellipsoid of size 100.
        let f = field("x^2 + y^2 + z^2 - 1", 1.25, Clip::Box);
        let made = build(&[Part::field(f, [0.0; 3], [125.0; 3])], vec![], 2.0);
        let want = build(&[Part::new(Shape::Ellipsoid, [0.0; 3], [100.0; 3])], vec![], 2.0);
        let (lo, hi) = made.bounds();
        let (wl, wh) = want.bounds();
        for i in 0..3 {
            assert!((lo[i] - wl[i]).abs() < 1.5 && (hi[i] - wh[i]).abs() < 1.5, "{lo:?} {hi:?} vs {wl:?} {wh:?}");
        }
        // Every vertex 50 px from the centre, as the ellipsoid's are.
        let worst = made.positions.iter().map(|p| (crate::length(*p) - 50.0).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1.0, "{worst}");
        assert!((crate::math::volume(&made) - crate::math::volume(&want)).abs() / crate::math::volume(&want) < 0.02);
        assert_eq!(open_edges(&made), (0, 0));
        // The distances are about pixels, so it blends like any part.
        let d = Field::new(vec![compile("t", "x^2 + y^2 + z^2 - 1", NAMES).unwrap()], None, [-1.25; 3], [1.25; 3], Clip::Box, 0.0).unwrap();
        let at = d.distance([60.0, 0.0, 0.0], [125.0; 3]);
        assert!((at - 10.0).abs() < 4.0, "{at}");
    }

    #[test]
    fn a_gyroid_lattice_is_a_whole_closed_solid() {
        let p = preset("gyroid").unwrap();
        let src = bind("gyroid", p.params, &[], p.formula).unwrap();
        let f = field(&src, p.bounds, p.clip);
        let (m, cost) = build_counted(&[Part::field(f, [0.0; 3], [200.0; 3])], vec![Material::default()], 2.5);
        assert!(m.triangles() > 5000, "{}", m.triangles());
        assert_eq!(open_edges(&m).0, 0, "no holes");
        assert!(crate::math::volume(&m) > 0.0, "faces outward");
        // A sheet lattice fills far less than its box.
        let v = crate::math::volume(&m) / (200.0f64.powi(3));
        assert!(v > 0.1 && v < 0.7, "{v}");
        assert!(cost.samples > 100_000 && cost.grid.iter().all(|&g| g <= crate::sdf::MAX_GRID as usize + 3));
    }

    #[test]
    fn fields_blend_carve_and_cut_the_other_parts() {
        let ball = Part::new(Shape::Ellipsoid, [0.0; 3], [160.0; 3]);
        let lattice = field(&bind("g", preset("gyroid").unwrap().params, &[("cells".into(), 3.0)], preset("gyroid").unwrap().formula).unwrap(), 1.0, Clip::Box);
        let carved = build(&[ball.clone(), Part { join: crate::sdf::Join::Intersect, ..Part::field(lattice, [0.0; 3], [170.0; 3]) }], vec![], 2.0);
        let whole = build(&[ball], vec![], 2.0);
        let (vc, vw) = (crate::math::volume(&carved), crate::math::volume(&whole));
        assert!(vc > 0.15 * vw && vc < 0.8 * vw, "{vc} of {vw}");
        assert_eq!(open_edges(&carved).0, 0);
    }

    #[test]
    fn every_preset_makes_a_surface_and_moving_ones_say_so() {
        for p in PRESETS {
            let src = bind(p.name, p.params, &[], p.formula).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            let f = Field::new(vec![compile(p.name, &src, NAMES).unwrap()], None, [-p.bounds; 3], [p.bounds; 3], p.clip, 0.5).unwrap();
            let m = build(&[Part::field(Arc::new(f), [0.0; 3], [100.0; 3])], vec![], 2.5);
            assert!(m.triangles() > 200, "{} made {}", p.name, m.triangles());
        }
        let f = field("sphere(x, y, z, 0.5 + 0.1 * sin(t))", 1.0, Clip::Box);
        assert!(f.moves() && !field("x", 1.0, Clip::Box).moves());
        assert!(bind("gyroid", preset("gyroid").unwrap().params, &[("bogus".into(), 1.0)], "x").unwrap_err().contains("cells, thickness, solid"));
    }

    #[test]
    fn two_formulas_morph_by_their_weight() {
        let a = compile("a", "sphere(x, y, z, 0.5)", NAMES).unwrap();
        let b = compile("b", "box(x, y, z, 0.5, 0.5, 0.5)", NAMES).unwrap();
        let w = compile("w", "t", NAMES).unwrap();
        let f = Field::new(vec![a, b], Some(w), [-1.0; 3], [1.0; 3], Clip::Box, 0.0).unwrap();
        let corner = [0.45, 0.45, 0.45];
        assert!(f.value(corner) > 0.0, "a ball misses the corner");
        assert!(f.at_time(1.0).value(corner) < 0.0, "a box holds it");
        assert!(f.moves() && f.cost() == 3);
    }

    #[test]
    fn a_fine_grid_is_made_coarse_enough_to_stay_under_the_cap() {
        let fine = cell_for([1000.0; 3], 6, 1);
        let points = (1000.0 / fine + 3.0).powi(3) as u64;
        assert!(points <= crate::math::MAX_EVALS);
        let costly = cell_for([1000.0; 3], 6, 20);
        assert!(costly > fine && (1000.0 / costly + 3.0).powi(3) as u64 * 20 <= crate::math::MAX_EVALS);
    }
}
