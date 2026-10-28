// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shapes from mathematics: forms a sculptor or a stack of primitives
//! can't reach, written as formulas in Studio's expression language
//! (compiled by `studio_model::modifier::formula`, so safe and bounded).
//!
//! * [`field`]: implicit solids, f(x, y, z) ≤ 0 (minimal-surface lattices,
//!   blobs, metaballs, fractals by their distance estimates), as parts
//!   of the blended [`crate::sdf`] mix: they melt into, carve and cut the
//!   other parts.
//! * [`surface`]: parametric surfaces (u, v) → (x, y, z), with thickness
//!   and cut-outs by a pattern over u and v; tubes along curves; height
//!   fields z = f(x, y).
//! * [`deform`]: space warps for any model: twist, bend, taper, noise and
//!   formula warps, repetition.
//! * [`growth`]: L-systems, phyllotaxis, strange-attractor trails.
//!
//! Formulas work in their own space, the mathematician's: x right, y
//! away, z up (Studio's world is y down, z away; [`to_studio`] turns one
//! into the other). Every builder caps its resolution and iterations and
//! says how many times it worked a formula out.

pub mod deform;
pub mod field;
pub mod growth;
pub mod surface;

use crate::{Mesh, V3};
pub use studio_model::modifier::formula::Program;

/// Most formula evaluations one build may make.
pub const MAX_EVALS: u64 = 60_000_000;

/// A point in formula space (x right, y away, z up) in Studio's world
/// (x right, y down, z away).
pub fn to_studio(p: [f64; 3]) -> V3 {
    [p[0] as f32, -p[2] as f32, p[1] as f32]
}

/// A point in Studio's world in formula space.
pub fn from_studio(p: V3) -> [f64; 3] {
    [p[0] as f64, p[2] as f64, -p[1] as f64]
}

/// A size in Studio's axes (x, y, z) as formula axes (x, y, z).
pub fn size_from_studio(s: V3) -> [f64; 3] {
    [s[0] as f64, s[2] as f64, s[1] as f64]
}

/// Compiles a formula, saying which one is wrong.
pub fn compile(what: &str, src: &str, names: &[&str]) -> Result<Program, String> {
    Program::compile(src, names).map_err(|e| format!("{what} \"{src}\": {e}"))
}

/// How big to make a shape: its largest side, or each side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    Largest(f32),
    Sides(V3),
}

impl Fit {
    /// Scale per axis taking a box `extent` across (Studio axes) to this
    /// size.
    pub fn scales(&self, extent: V3) -> V3 {
        let e = extent.map(|v| v.max(1e-9));
        match *self {
            Fit::Largest(s) => [s / e[0].max(e[1]).max(e[2]); 3],
            Fit::Sides(s) => [s[0] / e[0], s[1] / e[1], s[2] / e[2]],
        }
    }
}

/// Centres `m` on the origin and scales it to `fit`.
pub fn fit(m: &mut Mesh, fit: Fit) {
    let (lo, hi) = m.bounds();
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let k = fit.scales([hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]);
    for p in &mut m.positions {
        *p = [(p[0] - c[0]) * k[0], (p[1] - c[1]) * k[1], (p[2] - c[2]) * k[2]];
    }
    if k[0] != k[1] || k[1] != k[2] {
        for n in &mut m.normals {
            *n = crate::normalise([n[0] / k[0], n[1] / k[1], n[2] / k[2]]);
        }
    }
}

/// How whole a surface is: edges with one face (holes) and edges with more
/// than two (where surfaces cross through an edge), counting vertices at
/// one place as one.
pub fn open_edges(m: &Mesh) -> (usize, usize) {
    let (ids, _) = m.position_ids();
    let mut edges: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] {
            continue;
        }
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    (edges.values().filter(|n| **n == 1).count(), edges.values().filter(|n| **n > 2).count())
}

/// The volume a closed surface holds (negative when it is inside out).
pub fn volume(m: &Mesh) -> f64 {
    let mut v = 0.0f64;
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t).map(|k| m.positions[k].map(|x| x as f64));
        v += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]);
    }
    v / 6.0
}

/// Turns a closed surface right way out (its faces outward).
pub fn orient_outward(m: &mut Mesh) {
    if volume(m) < 0.0 {
        crate::ops::flip(m);
    }
}

/// Numbers for the agent about what a build made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Formula evaluations made.
    pub evals: u64,
    /// The sampling grid (u × v, or x × y × z).
    pub grid: Vec<usize>,
    /// Things worth saying (a cap reached, a coarser grid used).
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_space_and_studio_space_turn_into_each_other() {
        let p = [1.0, 2.0, 3.0];
        assert_eq!(to_studio(p), [1.0, -3.0, 2.0], "z up is -y");
        assert_eq!(from_studio(to_studio(p)), p);
        let s = crate::prim::cube([10.0, 20.0, 30.0]);
        let mut m = s.clone();
        fit(&mut m, Fit::Largest(60.0));
        let (lo, hi) = m.bounds();
        assert!((hi[2] - lo[2] - 60.0).abs() < 1e-3 && (hi[0] - lo[0] - 20.0).abs() < 1e-3);
        assert_eq!(open_edges(&m), (0, 0));
        assert!(volume(&m) > 0.0);
    }
}
