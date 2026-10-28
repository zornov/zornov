// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Forms that grow by rules: L-systems drawn by a turtle in 3D (trees,
//! ferns, bushes, weeds), phyllotaxis (seeds and scales set by the golden
//! angle: sunflowers, pine cones, cacti), and the trails of strange
//! attractors (Lorenz's butterfly, Rössler, Aizawa…) integrated from
//! their equations written as formulas.
//!
//! All work in formula space (z up); the turtle starts at the origin
//! heading up, and turns in the x–z plane facing the viewer.

use super::Program;

/// The most symbols an L-system may grow to; growing stops before it.
pub const MAX_SYMBOLS: usize = 200_000;
/// The most points the turtle draws.
pub const MAX_POINTS: usize = 60_000;
/// The most seeds phyllotaxis places.
pub const MAX_SEEDS: usize = 20_000;
/// The most steps an attractor runs.
pub const MAX_STEPS: usize = 200_000;

/// An L-system: an axiom and rules, each symbol to one or more choices
/// (picked at random by the seed when there are several).
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub axiom: String,
    pub rules: Vec<(char, Vec<String>)>,
}

/// Grows `sys` `iterations` times (at most 12); the string and whether it
/// stopped early at [`MAX_SYMBOLS`].
pub fn expand(sys: &System, iterations: usize, seed: u32) -> (String, bool) {
    let mut s = sys.axiom.clone();
    let mut h = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
    for _ in 0..iterations.min(12) {
        let mut next = String::with_capacity(s.len() * 3);
        for c in s.chars() {
            match sys.rules.iter().find(|(k, _)| *k == c) {
                Some((_, choices)) if !choices.is_empty() => {
                    h ^= h << 13;
                    h ^= h >> 7;
                    h ^= h << 17;
                    next.push_str(&choices[(h % choices.len() as u64) as usize]);
                }
                _ => next.push(c),
            }
            if next.len() > MAX_SYMBOLS {
                return (s, true);
            }
        }
        s = next;
    }
    (s, false)
}

/// How the turtle draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turtle {
    /// Degrees per turn.
    pub angle: f64,
    /// Length of a step.
    pub step: f64,
    /// Radius of the trunk.
    pub width: f64,
    /// Radius kept at each `!`, and at each branch (`[`) not already
    /// thinned by a `!` before it.
    pub taper: f64,
    /// Step length kept at each branch.
    pub shrink: f64,
    /// Bends every step towards the ground (+) or the sky (−), as weight
    /// or light would.
    pub droop: f64,
}

impl Default for Turtle {
    fn default() -> Self {
        Turtle { angle: 25.0, step: 1.0, width: 0.12, taper: 0.7, shrink: 1.0, droop: 0.0 }
    }
}

/// One drawn branch: points and the radius at each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branch {
    pub points: Vec<[f64; 3]>,
    pub radii: Vec<f64>,
}

/// A leaf: where, which way the branch heads there, its side and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leaf {
    pub at: [f64; 3],
    pub heading: [f64; 3],
    pub side: [f64; 3],
    pub size: f64,
}

/// What the turtle drew.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grown {
    pub branches: Vec<Branch>,
    pub leaves: Vec<Leaf>,
    /// Stopped at [`MAX_POINTS`].
    pub capped: bool,
}

fn rot(v: [f64; 3], axis: [f64; 3], a: f64) -> [f64; 3] {
    // Rodrigues: v cos a + (k × v) sin a + k (k·v)(1 − cos a).
    let (s, c) = a.sin_cos();
    let d = axis[0] * v[0] + axis[1] * v[1] + axis[2] * v[2];
    let kx = [axis[1] * v[2] - axis[2] * v[1], axis[2] * v[0] - axis[0] * v[2], axis[0] * v[1] - axis[1] * v[0]];
    [0, 1, 2].map(|i| v[i] * c + kx[i] * s + axis[i] * d * (1.0 - c))
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-300);
    v.map(|x| x / l)
}

/// Draws `s` with a turtle: `F` and `G` step and draw, `f` steps without,
/// `+ -` turn left and right, `& ^` pitch down and up, `\ /` roll,
/// `|` turns round, `[ ]` branch and come back, `!` thins, `L` (or `*`)
/// puts a leaf; other symbols do nothing.
pub fn turtle(s: &str, how: &Turtle) -> Grown {
    #[derive(Clone, Copy)]
    struct State {
        p: [f64; 3],
        h: [f64; 3],
        l: [f64; 3],
        u: [f64; 3],
        w: f64,
        step: f64,
        branch: Option<usize>,
        /// A `!` has thinned the limb since it last drew: the forks that
        /// follow are already thinner and `[` does not thin them again.
        thinned: bool,
    }
    let a = how.angle.to_radians();
    let mut st = State { p: [0.0; 3], h: [0.0, 0.0, 1.0], l: [1.0, 0.0, 0.0], u: [0.0, 1.0, 0.0], w: how.width, step: how.step, branch: None, thinned: false };
    let mut stack: Vec<State> = Vec::new();
    let mut out = Grown::default();
    let mut points = 0usize;
    for c in s.chars() {
        match c {
            'F' | 'G' => {
                if points >= MAX_POINTS {
                    out.capped = true;
                    break;
                }
                if how.droop != 0.0 {
                    // Lean the heading towards straight down a little,
                    // never past it (a hanging limb turned the same angle
                    // every step swung from side to side of straight
                    // down), nor past straight up when it lifts.
                    let down = [0.0, 0.0, -1.0];
                    let tilt = unit([st.h[1] * down[2] - st.h[2] * down[1], st.h[2] * down[0] - st.h[0] * down[2], st.h[0] * down[1] - st.h[1] * down[0]]);
                    if tilt.iter().all(|x| x.is_finite()) && tilt.iter().any(|x| x.abs() > 1e-9) {
                        let to_down = (-st.h[2]).clamp(-1.0, 1.0).acos();
                        let turn = if how.droop > 0.0 { (how.droop * 0.1).min(to_down) } else { (how.droop * 0.1).max(to_down - std::f64::consts::PI) };
                        st.h = unit(rot(st.h, tilt, turn));
                        st.l = unit(rot(st.l, tilt, turn));
                        st.u = unit(rot(st.u, tilt, turn));
                    }
                }
                let b = match st.branch {
                    Some(b) => b,
                    None => {
                        out.branches.push(Branch { points: vec![st.p], radii: vec![st.w] });
                        points += 1;
                        st.branch = Some(out.branches.len() - 1);
                        out.branches.len() - 1
                    }
                };
                st.p = [0, 1, 2].map(|i| st.p[i] + st.h[i] * st.step);
                out.branches[b].points.push(st.p);
                out.branches[b].radii.push(st.w);
                points += 1;
                st.thinned = false;
            }
            'f' => {
                st.p = [0, 1, 2].map(|i| st.p[i] + st.h[i] * st.step);
                st.branch = None;
            }
            '+' => (st.h, st.l) = (rot(st.h, st.u, a), rot(st.l, st.u, a)),
            '-' => (st.h, st.l) = (rot(st.h, st.u, -a), rot(st.l, st.u, -a)),
            '&' => (st.h, st.u) = (rot(st.h, st.l, a), rot(st.u, st.l, a)),
            '^' => (st.h, st.u) = (rot(st.h, st.l, -a), rot(st.u, st.l, -a)),
            '\\' => (st.l, st.u) = (rot(st.l, st.h, a), rot(st.u, st.h, a)),
            '/' => (st.l, st.u) = (rot(st.l, st.h, -a), rot(st.u, st.h, -a)),
            '|' => (st.h, st.l) = (rot(st.h, st.u, std::f64::consts::PI), rot(st.l, st.u, std::f64::consts::PI)),
            '!' => {
                st.w *= how.taper;
                st.thinned = true;
            }
            '[' => {
                if stack.len() < 256 {
                    stack.push(st);
                }
                // One taper a fork: `![…][…]` (the tree's rule) thinned
                // each generation by taper² before, so twigs came out a
                // fiftieth of the trunk six forks up.
                if !st.thinned {
                    st.w *= how.taper;
                }
                st.thinned = false;
                st.step *= how.shrink;
                st.branch = None;
            }
            ']' => {
                if let Some(back) = stack.pop() {
                    st = back;
                }
            }
            'L' | '*' => out.leaves.push(Leaf { at: st.p, heading: st.h, side: st.l, size: st.step }),
            _ => {}
        }
    }
    out.branches.retain(|b| b.points.len() >= 2);
    out
}

/// A named L-system with how its turtle draws.
#[derive(Clone, Copy, Debug)]
pub struct Plant {
    pub name: &'static str,
    pub about: &'static str,
    pub axiom: &'static str,
    pub rules: &'static [(char, &'static [&'static str])],
    pub iterations: usize,
    pub turtle: Turtle,
}

/// The L-system presets.
pub const PLANTS: &[Plant] = &[
    Plant { name: "tree", about: "a branching tree in 3D, with leaves", axiom: "FFFA", rules: &[('A', &["![&FFLA]/////[&FFLA]///////[&FFLA]", "![&FFLA]//////[&FLA]////[&FFLA]"]), ('F', &["F"])], iterations: 6, turtle: Turtle { angle: 28.0, step: 1.0, width: 0.22, taper: 0.72, shrink: 0.86, droop: 0.15 } },
    Plant { name: "bush", about: "a dense, round bush", axiom: "A", rules: &[('A', &["[&FL!A]/////[&FL!A]///////[&FL!A]"]), ('F', &["S/////F"]), ('S', &["FL"])], iterations: 6, turtle: Turtle { angle: 22.5, step: 1.0, width: 0.12, taper: 0.8, shrink: 1.0, droop: 0.0 } },
    Plant { name: "plant", about: "a weed-like plant (Lindenmayer's fig. 1.24f), flat", axiom: "X", rules: &[('X', &["F+[[X]-X]-F[-FX]+X"]), ('F', &["FF"])], iterations: 5, turtle: Turtle { angle: 25.0, step: 1.0, width: 0.25, taper: 0.75, shrink: 1.0, droop: 0.0 } },
    Plant { name: "fern", about: "a fern frond, flat, its leaflets in pairs", axiom: "X", rules: &[('X', &["F[+XL]F[-XL]+X"]), ('F', &["FF"])], iterations: 6, turtle: Turtle { angle: 22.0, step: 1.0, width: 0.3, taper: 0.7, shrink: 1.0, droop: 0.0 } },
    Plant { name: "weed", about: "a sprawling, random weed (each grow picks among rules)", axiom: "F", rules: &[('F', &["F[+F]F[-F]F", "F[+F]F", "F[-F]F"])], iterations: 5, turtle: Turtle { angle: 25.7, step: 1.0, width: 0.3, taper: 0.7, shrink: 1.0, droop: 0.0 } },
    Plant { name: "pine", about: "a conifer: whorls of drooping branches round a trunk", axiom: "FFFFA", rules: &[('A', &["F[&&B]/////[&&B]/////[&&B]/////[&&B]A"]), ('B', &["F[-L][+L]F[-L][+L]FL"])], iterations: 9, turtle: Turtle { angle: 22.0, step: 1.0, width: 0.3, taper: 0.6, shrink: 0.92, droop: 0.4 } },
];

/// The L-system called `name`.
pub fn plant(name: &str) -> Option<&'static Plant> {
    PLANTS.iter().find(|p| p.name == name)
}

impl Plant {
    pub fn system(&self) -> System {
        System { axiom: self.axiom.into(), rules: self.rules.iter().map(|(k, v)| (*k, v.iter().map(|s| s.to_string()).collect())).collect() }
    }
}

/// How phyllotaxis spreads its seeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    /// A flat disc (a sunflower's head).
    Disc,
    /// A ball (a dandelion clock, a pollen grain).
    Ball,
    /// An egg standing up (a pine cone).
    Cone,
    /// A dome (a cactus, a cauliflower).
    Dome,
}

impl Spread {
    pub fn parse(s: &str) -> Option<Spread> {
        Some(match s {
            "disc" | "disk" | "sunflower" | "flat" => Spread::Disc,
            "ball" | "sphere" | "globe" => Spread::Ball,
            "cone" | "pinecone" | "pine_cone" | "egg" => Spread::Cone,
            "dome" | "cactus" | "hemisphere" => Spread::Dome,
            _ => return None,
        })
    }
}

/// One seed: where, which way it faces out, and how big (0..1 of the
/// largest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seed {
    pub at: [f64; 3],
    pub normal: [f64; 3],
    pub size: f64,
}

/// `count` seeds set `angle` degrees apart round the centre (137.5…, the
/// golden angle, packs them as plants do), on a body of radius 1.
pub fn phyllotaxis(count: usize, angle: f64, spread: Spread) -> Vec<Seed> {
    let n = count.clamp(1, MAX_SEEDS);
    let a = angle.to_radians();
    (0..n)
        .map(|i| {
            let f = (i as f64 + 0.5) / n as f64;
            let th = i as f64 * a;
            let (c, s) = (th.cos(), th.sin());
            match spread {
                Spread::Disc => {
                    let r = f.sqrt();
                    Seed { at: [r * c, r * s, 0.0], normal: [0.0, 0.0, 1.0], size: 0.4 + 0.6 * r }
                }
                Spread::Ball => {
                    let z = 1.0 - 2.0 * f;
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    let p = [r * c, r * s, z];
                    Seed { at: p, normal: p, size: 1.0 }
                }
                Spread::Dome => {
                    let z = 1.0 - f;
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    let p = [r * c, r * s, z];
                    Seed { at: p, normal: p, size: 0.5 + 0.5 * r }
                }
                Spread::Cone => {
                    // An egg from z = -1 to 1, widest low down.
                    let z = 1.0 - 2.0 * f;
                    let r = 0.62 * (std::f64::consts::PI * (1.0 - z) / 2.0).sin().powf(0.8) * (1.15 - 0.3 * z);
                    let slope = 0.62 * std::f64::consts::PI / 4.0 * (std::f64::consts::PI * (1.0 - z) / 2.0).cos();
                    Seed { at: [r * c, r * s, z], normal: unit([c, s, slope]), size: (r / 0.7).clamp(0.25, 1.0) }
                }
            }
        })
        .collect()
}

/// The names an attractor's equations read.
pub const ODE_NAMES: &[&str] = &["x", "y", "z", "t"];

/// A named strange attractor: dx/dt, dy/dt, dz/dt as formulas of x, y, z
/// (their settings bound in front), where it starts and its time step.
#[derive(Clone, Copy, Debug)]
pub struct Attractor {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [(&'static str, f64)],
    pub equations: [&'static str; 3],
    pub start: [f64; 3],
    pub dt: f64,
    pub steps: usize,
}

/// The attractor presets.
pub const ATTRACTORS: &[Attractor] = &[
    Attractor { name: "lorenz", about: "Lorenz's butterfly: weather in three numbers", params: &[("sigma", 10.0), ("rho", 28.0), ("beta", 8.0 / 3.0)], equations: ["sigma * (y - x)", "x * (rho - z) - y", "x * y - beta * z"], start: [0.1, 0.0, 0.0], dt: 0.005, steps: 12_000 },
    Attractor { name: "rossler", about: "Rössler's spiral that folds over itself", params: &[("a", 0.2), ("b", 0.2), ("c", 5.7)], equations: ["-y - z", "x + a * y", "b + z * (x - c)"], start: [1.0, 1.0, 0.0], dt: 0.02, steps: 12_000 },
    Attractor { name: "aizawa", about: "Aizawa's attractor: a ball with a tube through it", params: &[("a", 0.95), ("b", 0.7), ("c", 0.6), ("d", 3.5), ("g", 0.25), ("f", 0.1)], equations: ["(z - b) * x - d * y", "d * x + (z - b) * y", "c + a * z - z^3 / 3 - (x^2 + y^2) * (1 + g * z) + f * z * x^3"], start: [0.1, 0.0, 0.0], dt: 0.01, steps: 15_000 },
    Attractor { name: "thomas", about: "Thomas's cyclically symmetric attractor, a woven knot", params: &[("b", 0.208186)], equations: ["sin(y) - b * x", "sin(z) - b * y", "sin(x) - b * z"], start: [0.1, 0.0, 0.0], dt: 0.05, steps: 15_000 },
    Attractor { name: "halvorsen", about: "Halvorsen's three-lobed attractor", params: &[("a", 1.89)], equations: ["-a * x - 4 * y - 4 * z - y^2", "-a * y - 4 * z - 4 * x - z^2", "-a * z - 4 * x - 4 * y - x^2"], start: [-1.48, -1.51, 2.04], dt: 0.005, steps: 12_000 },
    Attractor { name: "chen", about: "Chen's double scroll", params: &[("a", 35.0), ("b", 3.0), ("c", 28.0)], equations: ["a * (y - x)", "(c - a) * x - x * z + c * y", "x * y - b * z"], start: [-0.1, 0.5, -0.6], dt: 0.002, steps: 15_000 },
];

/// The attractor called `name`.
pub fn attractor(name: &str) -> Option<&'static Attractor> {
    ATTRACTORS.iter().find(|a| a.name == name)
}

/// Follows dx/dt = `eq` (over [`ODE_NAMES`]) from `start` for `steps`
/// steps of `dt` (Runge–Kutta, 4th order), keeping every `every`th point;
/// the first `skip` steps (settling onto the attractor) are dropped. Stops
/// if it runs off to infinity.
pub fn integrate(eq: &[Program; 3], start: [f64; 3], dt: f64, steps: usize, skip: usize, every: usize) -> Vec<[f64; 3]> {
    let f = |p: [f64; 3], t: f64| {
        let v = [p[0], p[1], p[2], t];
        [eq[0].eval(&v), eq[1].eval(&v), eq[2].eval(&v)]
    };
    let mut p = start;
    let mut out = Vec::new();
    let every = every.max(1);
    for k in 0..steps.min(MAX_STEPS) {
        let t = k as f64 * dt;
        let k1 = f(p, t);
        let k2 = f([0, 1, 2].map(|i| p[i] + k1[i] * dt / 2.0), t + dt / 2.0);
        let k3 = f([0, 1, 2].map(|i| p[i] + k2[i] * dt / 2.0), t + dt / 2.0);
        let k4 = f([0, 1, 2].map(|i| p[i] + k3[i] * dt), t + dt);
        p = [0, 1, 2].map(|i| p[i] + dt / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]));
        if !p.iter().all(|x| x.is_finite() && x.abs() < 1e9) {
            break;
        }
        if k >= skip && (k - skip).is_multiple_of(every) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{compile, field::bind};

    #[test]
    fn an_l_system_grows_by_its_rules_and_stops_at_the_cap() {
        let koch = System { axiom: "F".into(), rules: vec![('F', vec!["F+F-F-F+F".into()])] };
        assert_eq!(expand(&koch, 1, 0).0, "F+F-F-F+F");
        assert_eq!(expand(&koch, 2, 0).0.matches('F').count(), 25);
        let (s, capped) = expand(&koch, 12, 0);
        assert!(capped && s.len() <= MAX_SYMBOLS);
    }

    #[test]
    fn the_turtle_draws_branches_and_comes_back_from_them() {
        let g = turtle("F[+F]F", &Turtle { angle: 90.0, ..Turtle::default() });
        // The trunk (three points) and the side branch (two).
        assert_eq!(g.branches.len(), 2);
        assert_eq!(g.branches[0].points.len(), 3);
        let tip = g.branches[0].points[2];
        assert!((tip[2] - 2.0).abs() < 1e-9 && tip[0].abs() < 1e-9, "{tip:?}");
        let side = g.branches[1].points[1];
        assert!((side[2] - 1.0).abs() < 1e-9 && (side[0].abs() - 1.0).abs() < 1e-9, "{side:?}");
        assert!(g.branches[1].radii[0] < g.branches[0].radii[0], "branches are thinner");
        for p in PLANTS {
            let (s, _) = expand(&p.system(), p.iterations, 1);
            let g = turtle(&s, &p.turtle);
            assert!(g.branches.len() > 3 && !g.capped, "{}", p.name);
        }
    }

    #[test]
    fn a_fork_thins_once_and_a_hanging_limb_hangs_straight() {
        let how = Turtle { taper: 0.5, ..Turtle::default() };
        let w = how.width;
        let side = |s: &str| turtle(s, &how).branches[1].radii[1];
        // `[` alone, `!` alone and `!` then `[`: each one taper.
        assert!((side("F[F]") - w * 0.5).abs() < 1e-12);
        assert!((side("F![F]") - w * 0.5).abs() < 1e-12, "{}", side("F![F]"));
        assert!((side("F![F][F]") - w * 0.5).abs() < 1e-12);
        assert!((turtle("F!F", &how).branches[0].radii[2] - w * 0.5).abs() < 1e-12);
        // Two `!` are two tapers; a step between `!` and `[` lets `[`
        // thin again.
        assert!((side("F!![F]") - w * 0.25).abs() < 1e-12);
        assert!((turtle("F!F[F]", &how).branches[1].radii[1] - w * 0.25).abs() < 1e-12);
        // Pitched straight down and drooping: it stays straight down.
        let g = turtle("&&&&FFFFFF", &Turtle { angle: 45.0, droop: 0.4, ..Turtle::default() });
        for p in &g.branches[0].points[1..] {
            assert!(p[0].abs() < 1e-9 && p[1].abs() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn phyllotaxis_spreads_seeds_evenly() {
        let seeds = phyllotaxis(500, 137.508, Spread::Disc);
        assert_eq!(seeds.len(), 500);
        // Nearest neighbours are about as far apart everywhere.
        let near = |k: usize| seeds.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, s)| ((s.at[0] - seeds[k].at[0]).powi(2) + (s.at[1] - seeds[k].at[1]).powi(2)).sqrt()).fold(f64::INFINITY, f64::min);
        let (a, b) = (near(100), near(450));
        assert!(a / b > 0.6 && a / b < 1.6, "{a} {b}");
        assert!(phyllotaxis(100, 137.5, Spread::Ball).iter().all(|s| ((s.at[0].powi(2) + s.at[1].powi(2) + s.at[2].powi(2)).sqrt() - 1.0).abs() < 1e-9));
        assert_eq!(phyllotaxis(10_000_000, 137.5, Spread::Cone).len(), MAX_SEEDS);
    }

    #[test]
    fn the_lorenz_attractor_stays_on_its_butterfly() {
        let a = attractor("lorenz").unwrap();
        let eq = a.equations.map(|e| compile("lorenz", &bind("lorenz", a.params, &[], e).unwrap(), ODE_NAMES).unwrap());
        let pts = integrate(&eq, a.start, a.dt, a.steps, 500, 2);
        assert!(pts.len() > 5000);
        assert!(pts.iter().all(|p| p[0].abs() < 30.0 && p[1].abs() < 35.0 && p[2] > -1.0 && p[2] < 60.0));
        // Both wings: x goes both ways.
        assert!(pts.iter().any(|p| p[0] > 10.0) && pts.iter().any(|p| p[0] < -10.0));
        assert!(integrate(&eq, a.start, a.dt, usize::MAX, 0, 1).len() <= MAX_STEPS);
        for at in ATTRACTORS {
            let eq = at.equations.map(|e| compile(at.name, &bind(at.name, at.params, &[], e).unwrap(), ODE_NAMES).unwrap());
            assert!(integrate(&eq, at.start, at.dt, at.steps, 200, 1).len() > at.steps / 2, "{}", at.name);
        }
    }
}
