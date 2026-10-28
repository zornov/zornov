// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The expression language made fast, for formulas worked out millions of
//! times: shapes from mathematics (a distance field sampled on a grid, a
//! surface sampled over u and v, a curve over its parameter). A formula is
//! parsed by [`super::expr`] and compiled to a short list of steps over
//! numbered slots, so evaluating it allocates nothing and looks nothing up
//! by name. The same safety holds: no loops, no I/O, the program's size
//! and depth bounded by the parser; the functions that iterate (the
//! fractals) cap their own iterations.
//!
//! Only the pure part of the language compiles: maths, noise, mapping and
//! the shape functions below. What reads a property's keys or another
//! clip (`value_at`, `wiggle`, `loop_out`, `prop`, …) is refused with a
//! word on why.
//!
//! Shape functions, all in the formula's own units:
//!
//! * Noise: `noise2(x, y)`, `noise3(x, y, z)` (smooth, about -1..1, an
//!   optional seed last), `fbm(x, y, z, octaves, seed)` (layers of
//!   noise), `voronoi(x, y[, z])` (distance to the nearest cell's
//!   centre), `voronoi_edge(x, y[, z])` (0 on the walls between cells).
//! * Distances: `sphere(x, y, z, r)`, `box(x, y, z, a, b, c)` (half
//!   sizes), `torus(x, y, z, R, r)` (round the z axis),
//!   `cylinder(x, y, z, r, h)` (along z, half height h), `length(…)`,
//!   `smin(a, b, k)` / `smax(a, b, k)` (smooth union / intersection).
//! * Minimal surfaces (0 on the surface): `gyroid`, `schwarz_p`,
//!   `schwarz_d`, `neovius`, `lidinoid` of `(x, y, z)`.
//! * Fractals as distance estimates: `mandelbulb(x, y, z, power, iters)`,
//!   `mandelbox(x, y, z, scale, iters)`, `menger(x, y, z, iters)`,
//!   `sierpinski(x, y, z, iters)`; iterations are capped (see
//!   [`MAX_FRACTAL_ITERATIONS`]).
//! * `superformula(angle, m, n1, n2, n3[, a, b])`: Gielis's radius.
//! * Also `sinh cosh tanh cbrt mix` beside the language's own maths.

use super::expr::{self, Expr};

/// Most iterations a fractal function runs, whatever it is asked.
pub const MAX_FRACTAL_ITERATIONS: f64 = 30.0;

/// A built-in function of numbers alone.
pub type Pure = fn(&[f64]) -> f64;

/// 0..1, the same for the same x.
pub fn rand01(x: f64) -> f64 {
    (super::grad((x * 1000.0).round() as i64, 99) + 1.0) * 0.5
}

/// Fraction of the way from `a` to `b` that `x` is, held to 0..1.
fn unit(x: f64, a: f64, b: f64) -> f64 {
    if (b - a).abs() < 1e-12 {
        return if x >= b { 1.0 } else { 0.0 };
    }
    ((x - a) / (b - a)).clamp(0.0, 1.0)
}

fn b(x: bool) -> f64 {
    if x { 1.0 } else { 0.0 }
}

/// The function called `name` if it needs only its numbers (argument
/// counts are checked by the parser).
pub fn lookup(name: &str) -> Option<Pure> {
    let f: Pure = match name {
        "sin" => |a| a[0].sin(),
        "cos" => |a| a[0].cos(),
        "tan" => |a| a[0].tan(),
        "asin" => |a| a[0].clamp(-1.0, 1.0).asin(),
        "acos" => |a| a[0].clamp(-1.0, 1.0).acos(),
        "atan" => |a| a[0].atan(),
        "atan2" => |a| a[0].atan2(a[1]),
        "sinh" => |a| a[0].sinh(),
        "cosh" => |a| a[0].cosh(),
        "tanh" => |a| a[0].tanh(),
        "abs" => |a| a[0].abs(),
        "sqrt" => |a| a[0].sqrt(),
        "cbrt" => |a| a[0].cbrt(),
        "exp" => |a| a[0].exp(),
        "ln" => |a| a[0].ln(),
        "log10" => |a| a[0].log10(),
        "floor" => |a| a[0].floor(),
        "ceil" => |a| a[0].ceil(),
        "round" => |a| a[0].round(),
        "fract" => |a| a[0] - a[0].floor(),
        "degrees" => |a| a[0].to_degrees(),
        "radians" => |a| a[0].to_radians(),
        "sign" => |a| {
            if a[0] > 0.0 {
                1.0
            } else if a[0] < 0.0 {
                -1.0
            } else {
                0.0
            }
        },
        "noise" => |a| super::noise(a[0], a.get(1).map_or(0, |s| *s as u32)),
        "random" => |a| match a {
            [x] => rand01(*x),
            [x, hi] => rand01(*x) * hi,
            [x, lo, hi, ..] => lo + rand01(*x) * (hi - lo),
            _ => 0.0,
        },
        "min" => |a| a.iter().copied().fold(f64::INFINITY, f64::min),
        "max" => |a| a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "length" => |a| a.iter().map(|x| x * x).sum::<f64>().sqrt(),
        "pow" => |a| a[0].powf(a[1]),
        "mod" => |a| a[0].rem_euclid(a[1]),
        "hypot" => |a| a[0].hypot(a[1]),
        "step" => |a| b(a[1] >= a[0]),
        "posterize" => |a| if a[1] <= 0.0 { a[0] } else { (a[0] * a[1]).floor() / a[1] },
        "clamp" => |a| a[0].max(a[1]).min(a[2]),
        "lerp" | "mix" => |a| a[0] + (a[1] - a[0]) * a[2],
        "smoothstep" => |a| {
            let u = unit(a[2], a[0], a[1]);
            u * u * (3.0 - 2.0 * u)
        },
        "smin" => |a| smin(a[0], a[1], a[2]),
        "smax" => |a| -smin(-a[0], -a[1], a[2]),
        "linear" => |a| mapped("linear", a),
        "ease" => |a| mapped("ease", a),
        "ease_in" => |a| mapped("ease_in", a),
        "ease_out" => |a| mapped("ease_out", a),
        "noise2" => |a| noise2(a[0], a[1], a.get(2).map_or(0, |s| *s as u32)),
        "noise3" => |a| noise3([a[0], a[1], a[2]], a.get(3).map_or(0, |s| *s as u32)),
        "fbm" => |a| fbm([a[0], a[1], a[2]], a.get(3).map_or(4.0, |o| *o), a.get(4).map_or(0, |s| *s as u32)),
        "voronoi" => |a| voronoi(a).0,
        "voronoi_edge" => |a| {
            let (f1, f2) = voronoi(a);
            f2 - f1
        },
        "sphere" => |a| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() - a[3],
        "box" => |a| {
            let q = [a[0].abs() - a[3], a[1].abs() - a[4], a[2].abs() - a[5]];
            let out = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
            out + q[0].max(q[1]).max(q[2]).min(0.0)
        },
        "torus" => |a| ((a[0].hypot(a[1]) - a[3]).hypot(a[2])) - a[4],
        "cylinder" => |a| {
            let (dx, dz) = (a[0].hypot(a[1]) - a[3], a[2].abs() - a[4]);
            dx.max(dz).min(0.0) + dx.max(0.0).hypot(dz.max(0.0))
        },
        "gyroid" => |a| a[0].sin() * a[1].cos() + a[1].sin() * a[2].cos() + a[2].sin() * a[0].cos(),
        "schwarz_p" => |a| a[0].cos() + a[1].cos() + a[2].cos(),
        "schwarz_d" => |a| {
            let (s, c) = ([a[0].sin(), a[1].sin(), a[2].sin()], [a[0].cos(), a[1].cos(), a[2].cos()]);
            s[0] * s[1] * s[2] + s[0] * c[1] * c[2] + c[0] * s[1] * c[2] + c[0] * c[1] * s[2]
        },
        "neovius" => |a| 3.0 * (a[0].cos() + a[1].cos() + a[2].cos()) + 4.0 * a[0].cos() * a[1].cos() * a[2].cos(),
        "lidinoid" => |a| {
            let (x, y, z) = (a[0], a[1], a[2]);
            0.5 * ((2.0 * x).sin() * y.cos() * z.sin() + (2.0 * y).sin() * z.cos() * x.sin() + (2.0 * z).sin() * x.cos() * y.sin())
                - 0.5 * ((2.0 * x).cos() * (2.0 * y).cos() + (2.0 * y).cos() * (2.0 * z).cos() + (2.0 * z).cos() * (2.0 * x).cos())
                + 0.15
        },
        "mandelbulb" => |a| mandelbulb([a[0], a[1], a[2]], a.get(3).map_or(8.0, |p| *p), iterations(a.get(4), 10.0)),
        "mandelbox" => |a| mandelbox([a[0], a[1], a[2]], a.get(3).map_or(2.0, |s| *s), iterations(a.get(4), 12.0)),
        "menger" => |a| menger([a[0], a[1], a[2]], iterations(a.get(3), 4.0).min(7)),
        "sierpinski" => |a| sierpinski([a[0], a[1], a[2]], iterations(a.get(3), 8.0)),
        "superformula" => |a| superformula(a[0], a[1], a[2], a[3], a[4], a.get(5).copied().unwrap_or(1.0), a.get(6).copied().unwrap_or(1.0)),
        _ => return None,
    };
    Some(f)
}

/// `linear`, `ease`, `ease_in`, `ease_out`.
fn mapped(f: &str, xs: &[f64]) -> f64 {
    let (u, v1, v2) = match xs {
        [x, v1, v2] => (x.clamp(0.0, 1.0), *v1, *v2),
        [x, a, b2, v1, v2] => (unit(*x, *a, *b2), *v1, *v2),
        // Four numbers: from 0..1 like the three-number form, with the
        // third and fourth the range.
        [x, _, v1, v2] => (x.clamp(0.0, 1.0), *v1, *v2),
        _ => return 0.0,
    };
    let k = match f {
        "ease" => u * u * u * (u * (u * 6.0 - 15.0) + 10.0),
        "ease_in" => u * u * u,
        "ease_out" => 1.0 - (1.0 - u).powi(3),
        _ => u,
    };
    v1 + (v2 - v1) * k
}

fn iterations(v: Option<&f64>, default: f64) -> u32 {
    v.copied().unwrap_or(default).round().clamp(1.0, MAX_FRACTAL_ITERATIONS) as u32
}

/// Smooth minimum over `k` (Quilez's polynomial); `k` ≤ 0 is the plain one.
pub fn smin(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// A lattice point's hash: 32-bit, so the GPU (WGSL has no 64-bit
/// integers) works out the same noise as here.
fn hash(i: [i64; 3], seed: u32) -> u32 {
    let mut h = (i[0] as i32 as u32).wrapping_mul(0x8da6_b343) ^ (i[1] as i32 as u32).wrapping_mul(0xd816_3841) ^ (i[2] as i32 as u32).wrapping_mul(0xcb1a_b31f) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

fn fade(f: f64) -> f64 {
    f * f * f * (f * (f * 6.0 - 15.0) + 10.0)
}

/// 3D gradient noise: smooth, 0 at whole points, about -1..1.
pub fn noise3(p: [f64; 3], seed: u32) -> f64 {
    const G: [[f64; 3]; 12] = [[1.0, 1.0, 0.0], [-1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0], [1.0, 0.0, -1.0], [-1.0, 0.0, -1.0], [0.0, 1.0, 1.0], [0.0, -1.0, 1.0], [0.0, 1.0, -1.0], [0.0, -1.0, -1.0]];
    let i = [p[0].floor(), p[1].floor(), p[2].floor()];
    let f = [p[0] - i[0], p[1] - i[1], p[2] - i[2]];
    let corner = |dx: i64, dy: i64, dz: i64| {
        let g = G[(hash([i[0] as i64 + dx, i[1] as i64 + dy, i[2] as i64 + dz], seed) % 12) as usize];
        g[0] * (f[0] - dx as f64) + g[1] * (f[1] - dy as f64) + g[2] * (f[2] - dz as f64)
    };
    let (u, v, w) = (fade(f[0]), fade(f[1]), fade(f[2]));
    let l = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = l(corner(0, 0, 0), corner(1, 0, 0), u);
    let x10 = l(corner(0, 1, 0), corner(1, 1, 0), u);
    let x01 = l(corner(0, 0, 1), corner(1, 0, 1), u);
    let x11 = l(corner(0, 1, 1), corner(1, 1, 1), u);
    l(l(x00, x10, v), l(x01, x11, v), w)
}

/// 2D gradient noise, about -1..1.
pub fn noise2(x: f64, y: f64, seed: u32) -> f64 {
    noise3([x, y, 0.5], seed) * 1.4
}

/// Octaves of 3D noise, each twice as fine and half as strong (up to 8).
pub fn fbm(p: [f64; 3], octaves: f64, seed: u32) -> f64 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for o in 0..octaves.round().clamp(1.0, 8.0) as u32 {
        sum += amp * noise3([p[0] * freq, p[1] * freq, p[2] * freq], seed.wrapping_add(o * 101));
        amp *= 0.5;
        freq *= 2.0;
    }
    sum * 2.0
}

/// Distances to the nearest and second nearest cell centres of a jittered
/// grid (cells one unit across), in 2D (two numbers) or 3D.
pub fn voronoi(a: &[f64]) -> (f64, f64) {
    let p = [a[0], a[1], if a.len() > 2 { a[2] } else { 0.0 }];
    let flat = a.len() < 3;
    let c = [p[0].floor() as i64, p[1].floor() as i64, p[2].floor() as i64];
    let (mut f1, mut f2) = (f64::INFINITY, f64::INFINITY);
    let zs: &[i64] = if flat { &[0] } else { &[-1, 0, 1] };
    for dz in zs {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let cell = [c[0] + dx, c[1] + dy, c[2] + dz];
                let h = hash(cell, 7);
                let j = |k: u32| ((h >> (k * 10)) & 1023) as f64 / 1023.0;
                let q = [cell[0] as f64 + j(0), cell[1] as f64 + j(1), if flat { p[2] } else { cell[2] as f64 + j(2) }];
                let d = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt();
                if d < f1 {
                    f2 = f1;
                    f1 = d;
                } else if d < f2 {
                    f2 = d;
                }
            }
        }
    }
    (f1, f2)
}

fn len3(p: [f64; 3]) -> f64 {
    (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt()
}

/// The Mandelbulb's distance estimate (White and Nylander's power-n bulb),
/// about 1.1 across at power 8.
pub fn mandelbulb(p: [f64; 3], power: f64, iters: u32) -> f64 {
    let power = power.clamp(2.0, 16.0);
    let mut z = p;
    let mut dr = 1.0;
    for _ in 0..iters {
        let r = len3(z);
        if r > 2.0 {
            break;
        }
        if r < 1e-12 {
            return -1e-3;
        }
        let theta = (z[2] / r).clamp(-1.0, 1.0).acos() * power;
        let phi = z[1].atan2(z[0]) * power;
        let zr = r.powf(power);
        dr = r.powf(power - 1.0) * power * dr + 1.0;
        z = [zr * theta.sin() * phi.cos() + p[0], zr * theta.sin() * phi.sin() + p[1], zr * theta.cos() + p[2]];
    }
    let r = len3(z).max(1e-12);
    0.5 * r.ln() * r / dr
}

/// The Mandelbox's distance estimate (Lowe): box fold, sphere fold, scale;
/// negative where the point never escapes. About 12 across at scale 2, 4
/// at -1.5.
pub fn mandelbox(p: [f64; 3], scale: f64, iters: u32) -> f64 {
    let mut z = p;
    let mut dr = 1.0;
    let mut escaped = false;
    for _ in 0..iters {
        for c in &mut z {
            *c = c.clamp(-1.0, 1.0) * 2.0 - *c;
        }
        let r2 = z[0] * z[0] + z[1] * z[1] + z[2] * z[2];
        let k = if r2 < 0.25 {
            4.0
        } else if r2 < 1.0 {
            1.0 / r2
        } else {
            1.0
        };
        z = [z[0] * k * scale + p[0], z[1] * k * scale + p[1], z[2] * k * scale + p[2]];
        dr = dr * k * scale.abs() + 1.0;
        if len3(z) > 100.0 {
            escaped = true;
            break;
        }
    }
    // The estimate is never below 0; points that never escape are inside.
    let d = len3(z) / dr.abs();
    if escaped { d } else { -d }
}

/// The Menger sponge filling the cube -1..1 (Quilez's distance).
pub fn menger(p: [f64; 3], iters: u32) -> f64 {
    let q = [p[0].abs() - 1.0, p[1].abs() - 1.0, p[2].abs() - 1.0];
    let mut d = len3([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]) + q[0].max(q[1]).max(q[2]).min(0.0);
    let mut s = 1.0;
    for _ in 0..iters {
        let a = [(p[0] * s).rem_euclid(2.0) - 1.0, (p[1] * s).rem_euclid(2.0) - 1.0, (p[2] * s).rem_euclid(2.0) - 1.0];
        s *= 3.0;
        let r = [(1.0 - 3.0 * a[0].abs()).abs(), (1.0 - 3.0 * a[1].abs()).abs(), (1.0 - 3.0 * a[2].abs()).abs()];
        let (da, db, dc) = (r[0].max(r[1]), r[1].max(r[2]), r[2].max(r[0]));
        let c = (da.min(db).min(dc) - 1.0) / s;
        d = d.max(c);
    }
    d
}

/// The Sierpinski tetrahedron with corners at (±1, ±1, ±1), by folding.
pub fn sierpinski(p: [f64; 3], iters: u32) -> f64 {
    let mut z = p;
    let scale = 2.0;
    for _ in 0..iters {
        if z[0] + z[1] < 0.0 {
            (z[0], z[1]) = (-z[1], -z[0]);
        }
        if z[0] + z[2] < 0.0 {
            (z[0], z[2]) = (-z[2], -z[0]);
        }
        if z[1] + z[2] < 0.0 {
            (z[1], z[2]) = (-z[2], -z[1]);
        }
        z = [z[0] * scale - (scale - 1.0), z[1] * scale - (scale - 1.0), z[2] * scale - (scale - 1.0)];
    }
    // The tetrahedron the folds map onto, scaled back.
    let t = (z[0] + z[1] + z[2] - 1.0).max(-z[0] - z[1] + z[2] - 1.0).max(-z[0] + z[1] - z[2] - 1.0).max(z[0] - z[1] - z[2] - 1.0) / 3f64.sqrt();
    t * scale.powi(-(iters as i32))
}

/// Gielis's superformula: the radius at `angle` (radians).
pub fn superformula(angle: f64, m: f64, n1: f64, n2: f64, n3: f64, a: f64, b: f64) -> f64 {
    let k = m * angle / 4.0;
    let t = (k.cos() / a).abs().powf(n2) + (k.sin() / b).abs().powf(n3);
    if t <= 0.0 || n1 == 0.0 {
        return 0.0;
    }
    t.powf(-1.0 / n1)
}

/// The most slots (given names and bindings) and stack a program has.
const MAX_SLOTS: usize = 96;
const MAX_STACK: usize = 256;

#[derive(Clone, Copy, Debug)]
enum Op {
    Num(f64),
    Load(u8),
    Store(u8),
    Neg,
    Not,
    Bin(char),
    Call(Pure, u8),
    /// Jump to the step if the top (popped) is 0.
    Jz(u32),
    Jump(u32),
}

/// A compiled formula: evaluate with [`Program::eval`], the values of its
/// names in the order they were given.
#[derive(Clone, Debug)]
pub struct Program {
    ops: Vec<Op>,
    expr: Expr,
    given: Vec<String>,
    names: usize,
    used: Vec<bool>,
    src: String,
}

/// Functions that need a property's keys, a clip or time handled by the
/// player, and so mean nothing in a shape.
const CONTEXT: &[(&str, &str)] = &[
    ("value_at", "reads a property's keys"),
    ("velocity", "reads a property's keys"),
    ("key_time", "reads a property's keys"),
    ("key_value", "reads a property's keys"),
    ("loop_out", "repeats a property's keys"),
    ("loop_in", "repeats a property's keys"),
    ("inertia", "rings after a property's keys"),
    ("wiggle", "wobbles a keyed property (use noise3 or fbm here)"),
    ("prop", "reads another clip"),
];

struct Compiler<'a> {
    ops: Vec<Op>,
    scope: Vec<(&'a str, u8)>,
    slots: usize,
    depth: usize,
    most: usize,
}

impl<'a> Compiler<'a> {
    fn push(&mut self, op: Op) -> Result<(), String> {
        match op {
            Op::Num(_) | Op::Load(_) => self.depth += 1,
            Op::Store(_) | Op::Bin(_) | Op::Jz(_) => self.depth -= 1,
            Op::Call(_, n) => self.depth = self.depth + 1 - n as usize,
            Op::Neg | Op::Not | Op::Jump(_) => {}
        }
        self.most = self.most.max(self.depth);
        if self.most > MAX_STACK {
            return Err("formula is too complex".into());
        }
        self.ops.push(op);
        Ok(())
    }

    fn slot(&self, n: &str) -> Option<u8> {
        self.scope.iter().rev().find(|(k, _)| *k == n).map(|(_, s)| *s)
    }

    fn expr(&mut self, e: &'a Expr) -> Result<(), String> {
        match e {
            Expr::Num(v) => self.push(Op::Num(*v)),
            Expr::Str(s) => Err(format!("\"{s}\": shape formulas take numbers, not words")),
            // `name[k]` reads the name itself: shape names have one part.
            Expr::Var(n) | Expr::Index(n, _) => match self.slot(n) {
                Some(s) => self.push(Op::Load(s)),
                None => self.push(Op::Num(0.0)),
            },
            Expr::List(items) => {
                // The element for `i`: i < 0.5 ? a : i < 1.5 ? b : …
                let i = self.slot("i");
                let mut ends = Vec::new();
                for (k, item) in items.iter().enumerate() {
                    if k + 1 == items.len() {
                        self.expr(item)?;
                        break;
                    }
                    match i {
                        Some(s) => self.push(Op::Load(s))?,
                        None => self.push(Op::Num(0.0))?,
                    }
                    self.push(Op::Num(k as f64 + 0.5))?;
                    self.push(Op::Bin('<'))?;
                    let jz = self.ops.len();
                    self.push(Op::Jz(0))?;
                    self.expr(item)?;
                    ends.push(self.ops.len());
                    self.push(Op::Jump(0))?;
                    // The other branch starts as deep as this one did.
                    self.depth -= 1;
                    let here = self.ops.len() as u32;
                    self.ops[jz] = Op::Jz(here);
                }
                let end = self.ops.len() as u32;
                for j in ends {
                    self.ops[j] = Op::Jump(end);
                }
                Ok(())
            }
            Expr::Neg(x) => {
                self.expr(x)?;
                self.push(Op::Neg)
            }
            Expr::Not(x) => {
                self.expr(x)?;
                self.push(Op::Not)
            }
            Expr::Bin(op, l, r) => {
                self.expr(l)?;
                self.expr(r)?;
                self.push(Op::Bin(*op))
            }
            Expr::Cond(c, a, bb) => {
                self.expr(c)?;
                let jz = self.ops.len();
                self.push(Op::Jz(0))?;
                self.expr(a)?;
                let jump = self.ops.len();
                self.push(Op::Jump(0))?;
                self.depth -= 1;
                self.ops[jz] = Op::Jz(self.ops.len() as u32);
                self.expr(bb)?;
                self.ops[jump] = Op::Jump(self.ops.len() as u32);
                Ok(())
            }
            Expr::Call(f, args) => {
                if let Some((_, why)) = CONTEXT.iter().find(|(n, _)| n == f) {
                    return Err(format!("{f} {why}, so it can't be used in a shape formula"));
                }
                if f == "if" {
                    return self.expr_if(args);
                }
                let g = lookup(f).ok_or_else(|| format!("{f} can't be used in a shape formula"))?;
                for a in args {
                    self.expr(a)?;
                }
                self.push(Op::Call(g, args.len() as u8))
            }
            Expr::Let(binds, body) => {
                let mark = self.scope.len();
                for (n, x) in binds {
                    self.expr(x)?;
                    if self.slots >= MAX_SLOTS {
                        return Err("too many statements".into());
                    }
                    let s = self.slots as u8;
                    self.slots += 1;
                    self.push(Op::Store(s))?;
                    self.scope.push((n.as_str(), s));
                }
                self.expr(body)?;
                self.scope.truncate(mark);
                Ok(())
            }
        }
    }

    fn expr_if(&mut self, args: &'a [Expr]) -> Result<(), String> {
        self.expr(&args[0])?;
        let jz = self.ops.len();
        self.push(Op::Jz(0))?;
        self.expr(&args[1])?;
        let jump = self.ops.len();
        self.push(Op::Jump(0))?;
        self.depth -= 1;
        self.ops[jz] = Op::Jz(self.ops.len() as u32);
        self.expr(&args[2])?;
        self.ops[jump] = Op::Jump(self.ops.len() as u32);
        Ok(())
    }
}

impl Program {
    /// Parses and compiles `src` over `names` (lower case, at most 32): the
    /// variables it may read, given to [`Program::eval`] in this order.
    pub fn compile(src: &str, names: &[&str]) -> Result<Program, String> {
        if names.len() > 32 {
            return Err("too many names".into());
        }
        let e = expr::parse_with(src, names)?;
        let mut c = Compiler { ops: Vec::new(), scope: names.iter().enumerate().map(|(i, n)| (*n, i as u8)).collect(), slots: names.len(), depth: 0, most: 0 };
        c.expr(&e)?;
        let mut used = vec![false; names.len()];
        for op in &c.ops {
            if let Op::Load(s) = op
                && (*s as usize) < names.len()
            {
                used[*s as usize] = true;
            }
        }
        Ok(Program { ops: c.ops, names: names.len(), used, src: src.to_owned(), given: names.iter().map(|n| n.to_string()).collect(), expr: e.clone() })
    }

    /// The formula's value, `vars` the values of its names in order
    /// (missing ones are 0).
    pub fn eval(&self, vars: &[f64]) -> f64 {
        let mut slots = [0.0f64; MAX_SLOTS];
        let n = vars.len().min(self.names);
        slots[..n].copy_from_slice(&vars[..n]);
        let mut st = [0.0f64; MAX_STACK + 1];
        let mut sp = 0usize;
        let mut pc = 0usize;
        while let Some(op) = self.ops.get(pc) {
            pc += 1;
            match *op {
                Op::Num(v) => {
                    st[sp] = v;
                    sp += 1;
                }
                Op::Load(s) => {
                    st[sp] = slots[s as usize];
                    sp += 1;
                }
                Op::Store(s) => {
                    sp -= 1;
                    slots[s as usize] = st[sp];
                }
                Op::Neg => st[sp - 1] = -st[sp - 1],
                Op::Not => st[sp - 1] = b(st[sp - 1] == 0.0),
                Op::Bin(o) => {
                    sp -= 1;
                    let (a, c) = (st[sp - 1], st[sp]);
                    st[sp - 1] = match o {
                        '+' => a + c,
                        '-' => a - c,
                        '*' => a * c,
                        '/' => a / c,
                        '%' => a.rem_euclid(c),
                        '^' => a.powf(c),
                        '<' => b(a < c),
                        '>' => b(a > c),
                        'l' => b(a <= c),
                        'g' => b(a >= c),
                        '=' => b((a - c).abs() < 1e-9),
                        '&' => b(a != 0.0 && c != 0.0),
                        '|' => b(a != 0.0 || c != 0.0),
                        _ => b((a - c).abs() >= 1e-9),
                    };
                }
                Op::Call(g, k) => {
                    let k = k as usize;
                    let v = g(&st[sp - k..sp]);
                    sp -= k;
                    st[sp] = v;
                    sp += 1;
                }
                Op::Jz(to) => {
                    sp -= 1;
                    if st[sp] == 0.0 {
                        pc = to as usize;
                    }
                }
                Op::Jump(to) => pc = to as usize,
            }
        }
        if sp == 0 { 0.0 } else { st[sp - 1] }
    }

    /// Whether the formula reads its `k`th name (whether a shape moves with
    /// `t`, say).
    pub fn uses(&self, k: usize) -> bool {
        self.used.get(k).copied().unwrap_or(false)
    }

    /// The formula as written.
    pub fn source(&self) -> &str {
        &self.src
    }

    /// The formula as parsed.
    pub fn expr(&self) -> &Expr {
        &self.expr
    }

    /// The names it reads, in order.
    pub fn names(&self) -> &[String] {
        &self.given
    }

    /// How many steps it runs at most (a measure of its cost).
    pub fn steps(&self) -> usize {
        self.ops.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_as_the_tree(src: &str, names: &[&str], vals: &[f64]) {
        let p = Program::compile(src, names).unwrap_or_else(|e| panic!("{src}: {e}"));
        let tree = expr::parse_with(src, names).unwrap();
        let want = tree.eval(&|n| names.iter().position(|k| *k == n).map(|i| vals[i]));
        let got = p.eval(vals);
        assert!((want - got).abs() < 1e-9 || (want.is_nan() && got.is_nan()), "{src}: tree {want}, compiled {got}");
    }

    #[test]
    fn compiled_formulas_agree_with_the_language() {
        let n = ["x", "y", "z", "t", "i"];
        for (src, vals) in [
            ("x^2 + y^2 + z^2 - 1", [0.3, -0.4, 0.5, 0.0, 0.0]),
            ("a = 2; b = a * x\nb + 1", [1.5, 0.0, 0.0, 0.0, 0.0]),
            ("x > 0 ? sin(x) : cos(y)", [-1.0, 2.0, 0.0, 0.0, 0.0]),
            ("x > 0 ? sin(x) : cos(y)", [1.0, 2.0, 0.0, 0.0, 0.0]),
            ("if(t < 1, x, y) + min(1, 2, x, 4)", [0.25, 9.0, 0.0, 0.5, 0.0]),
            ("[x, y, z]", [1.0, 2.0, 3.0, 0.0, 2.0]),
            ("[x, y, z]", [1.0, 2.0, 3.0, 0.0, 1.0]),
            ("[x, y, z]", [1.0, 2.0, 3.0, 0.0, 0.0]),
            ("!(x > 0) || y && z", [1.0, 1.0, 0.0, 0.0, 0.0]),
            ("smoothstep(0, 1, x) * linear(y, 0, 2, 10, 20) + noise(x * 3, 2) + random(z, 1, 2)", [0.3, 1.0, 4.0, 0.0, 0.0]),
            ("-x % 3 + 2^-y", [7.0, 1.0, 0.0, 0.0, 0.0]),
        ] {
            same_as_the_tree(src, &n, &vals);
        }
    }

    #[test]
    fn shape_names_are_not_taken_for_aliases_and_context_functions_are_refused() {
        // `v` is `value` in property formulas but a name of its own here.
        let p = Program::compile("u * 10 + v", &["u", "v"]).unwrap();
        assert_eq!(p.eval(&[2.0, 3.0]), 23.0);
        assert!(p.uses(1));
        assert!(Program::compile("value_at(1)", &["x"]).unwrap_err().contains("keys"));
        assert!(Program::compile("wiggle(1, 2)", &["x"]).unwrap_err().contains("noise3"));
        assert!(Program::compile("q + 1", &["x"]).unwrap_err().contains("unknown name q"));
        assert!(!Program::compile("x + 1", &["x", "t"]).unwrap().uses(1));
    }

    #[test]
    fn shape_functions_measure_what_they_say() {
        let f = |s: &str, p: [f64; 3]| Program::compile(s, &["x", "y", "z"]).unwrap().eval(&p);
        assert!((f("sphere(x, y, z, 1)", [2.0, 0.0, 0.0]) - 1.0).abs() < 1e-12);
        assert!((f("box(x, y, z, 1, 1, 1)", [0.0, 3.0, 0.0]) - 2.0).abs() < 1e-12);
        assert!((f("torus(x, y, z, 2, 0.5)", [2.0, 0.0, 0.0]) + 0.5).abs() < 1e-12);
        assert!((f("cylinder(x, y, z, 1, 1)", [0.0, 0.0, 3.0]) - 2.0).abs() < 1e-12);
        assert!(f("gyroid(x, y, z)", [0.0, 0.0, 0.0]).abs() < 1e-12);
        // Fractals: inside is negative or near 0, far away positive.
        assert!(f("mandelbulb(x, y, z)", [0.0, 0.0, 0.0]) <= 0.0);
        assert!(f("mandelbulb(x, y, z)", [3.0, 0.0, 0.0]) > 1.0);
        assert!(f("menger(x, y, z, 3)", [0.0, 0.0, 0.0]) > 0.0, "the middle of a sponge is a hole");
        assert!(f("menger(x, y, z, 3)", [0.99, 0.99, 0.99]) < 0.0, "a corner is solid");
        assert!(f("sierpinski(x, y, z)", [1.0, 1.0, 1.0]) < 0.05);
        assert!(f("sierpinski(x, y, z)", [-2.0, -2.0, -2.0]) > 0.5);
        assert!(f("mandelbox(x, y, z)", [0.0, 0.0, 0.0]) <= 0.1);
        // Noise is smooth and bounded; voronoi edges are 0 between cells.
        let mut most: f64 = 0.0;
        for k in 0..2000 {
            let p = [k as f64 * 0.037, k as f64 * 0.011, k as f64 * 0.023];
            most = most.max(noise3(p, 1).abs()).max(fbm(p, 5.0, 2).abs());
            let (f1, f2) = voronoi(&p);
            assert!(f1 <= f2 && f1 < 1.8);
        }
        assert!(most <= 2.0 && most > 0.1, "{most}");
        assert!((superformula(0.3, 4.0, 2.0, 2.0, 2.0, 1.0, 1.0) - 1.0).abs() < 1e-9, "m=4, n=2 is a circle");
    }

    #[test]
    fn iterating_functions_cap_their_iterations() {
        let slow = Program::compile("mandelbulb(x, y, z, 8, 1e9)", &["x", "y", "z"]).unwrap();
        let capped = Program::compile("mandelbulb(x, y, z, 8, 30)", &["x", "y", "z"]).unwrap();
        let p = [0.3, 0.2, 0.1];
        assert_eq!(slow.eval(&p), capped.eval(&p));
        assert_eq!(Program::compile("menger(x, y, z, 1e9)", &["x", "y", "z"]).unwrap().eval(&p), menger(p, 7));
    }
}
