// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Curves from mathematics, for shape layers (the `curve` item) and motion
//! paths: roses, Lissajous figures, hearts, spirals, spirographs, Gielis's
//! superformula; any parametric curve x(u), y(u), polar r(theta) or
//! implicit f(x, y) = 0 written as a formula; L-systems (Koch snowflake,
//! dragon, Hilbert, Sierpinski, Gosper); and Fourier epicycles that draw
//! any outline with turning circles.
//!
//! Formulas are Studio's expression language (safe, bounded), with `t`
//! the layer's time, so a curve can morph as it plays. They work in their
//! own units, y up; the result is fitted to `size` px (its largest side)
//! or scaled by `scale` px per unit, in canvas pixels (y down).
//!
//! Item fields: `preset` with its settings by name (any of them keyed or a
//! `=formula` like every shape value), or `x` + `y` (formulas of `u`, `t`),
//! or `polar` (r of `theta`, `t`), with `u` (the range, numbers or
//! formulas: `[0, "tau"]`) and `samples`; or `implicit` (f of `x`, `y`,
//! `t`, `r`) with `bounds` and `resolution`; or `lsystem` (`axiom`,
//! `rules`, `iterations`, `angle`, `draw`); and `fourier` (`of`: points,
//! an SVG path, or a curve; `terms`; `epicycles`; `period`).

use serde_json::{Map, Value};
use studio_model::modifier::formula::Program;

type P = [f64; 2];
/// Polylines, each with whether it closes.
type Polys = Vec<(Vec<P>, bool)>;
/// An implicit preset: name, what it is, its settings, f(x, y).
type ImplicitPreset = (&'static str, &'static str, &'static [(&'static str, f64)], &'static str);
/// An L-system preset: name, what it is, axiom, rules, angle, iterations, the symbols that draw.
type LsysPreset = (&'static str, &'static str, &'static str, &'static [(char, &'static str)], f64, usize, &'static str);

/// The most points one curve may have.
pub const MAX_SAMPLES: usize = 20_000;
/// The most cells an implicit curve samples along a side.
pub const MAX_RESOLUTION: usize = 600;
/// The most symbols an L-system grows to.
pub const MAX_SYMBOLS: usize = 100_000;
/// The most Fourier terms.
pub const MAX_TERMS: usize = 500;

/// The polylines of a curve (each with whether it closes), in canvas px
/// (y down) about the item's position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lines {
    pub lines: Vec<(Vec<P>, bool)>,
    /// Formula evaluations made.
    pub evals: u64,
    /// The sampling: points, or cells across.
    pub samples: usize,
}

/// A named curve: its settings, x and y (or r for polar) as formulas of
/// `u` (or `theta`) over a range.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    pub params: &'static [(&'static str, f64)],
    /// `[x, y]` or, for polar ones, r.
    pub formula: &'static str,
    pub polar: bool,
    pub u: [&'static str; 2],
    pub closed: bool,
}

/// The curve presets.
pub const PRESETS: &[Preset] = &[
    Preset { name: "rose", about: "a rose r = cos(k theta), k = n / d petals' worth", params: &[("n", 5.0), ("d", 1.0)], formula: "cos(n / d * theta)", polar: true, u: ["0", "2 * pi * d"], closed: true },
    Preset { name: "lissajous", about: "a Lissajous figure: x = sin(a u + delta), y = sin(b u)", params: &[("a", 3.0), ("b", 2.0), ("delta", std::f64::consts::FRAC_PI_2)], formula: "[sin(a * u + delta), sin(b * u)]", polar: false, u: ["0", "tau"], closed: true },
    Preset { name: "heart", about: "the classic heart curve", params: &[], formula: "[16 * sin(u)^3 / 17, (13 * cos(u) - 5 * cos(2 * u) - 2 * cos(3 * u) - cos(4 * u)) / 17]", polar: false, u: ["0", "tau"], closed: true },
    Preset { name: "spiral", about: "Archimedes' spiral, `turns` turns", params: &[("turns", 4.0)], formula: "theta / (2 * pi * turns)", polar: true, u: ["0", "2 * pi * turns"], closed: false },
    Preset { name: "log_spiral", about: "the logarithmic (growth) spiral of shells and galaxies", params: &[("growth", 0.18), ("turns", 4.0)], formula: "exp(growth * (theta - 2 * pi * turns))", polar: true, u: ["0", "2 * pi * turns"], closed: false },
    Preset { name: "fermat_spiral", about: "Fermat's spiral, both arms", params: &[("turns", 3.0)], formula: "[sign(u) * sqrt(abs(u)) * cos(u), sign(u) * sqrt(abs(u)) * sin(u)]", polar: false, u: ["-2 * pi * turns", "2 * pi * turns"], closed: false },
    Preset { name: "superformula", about: "Gielis's superformula: stars, flowers, shells by m and n1..n3", params: &[("m", 6.0), ("n1", 1.0), ("n2", 7.0), ("n3", 8.0)], formula: "superformula(theta, m, n1, n2, n3)", polar: true, u: ["0", "tau"], closed: true },
    Preset { name: "spirograph", about: "a hypotrochoid: a pen `pen` out on a wheel `small` rolling inside a ring `big`", params: &[("big", 5.0), ("small", 3.0), ("pen", 5.0)], formula: "k = (big - small) / small; [(big - small) * cos(u) + pen * cos(k * u), (big - small) * sin(u) - pen * sin(k * u)]", polar: false, u: ["0", "2 * pi * small"], closed: true },
    Preset { name: "epitrochoid", about: "a wheel rolling round the outside of a ring", params: &[("big", 3.0), ("small", 1.0), ("pen", 0.5)], formula: "k = (big + small) / small; [(big + small) * cos(u) - pen * cos(k * u), (big + small) * sin(u) - pen * sin(k * u)]", polar: false, u: ["0", "2 * pi * small"], closed: true },
    Preset { name: "cardioid", about: "the heart-shaped curve a point on a rolling circle traces", params: &[], formula: "1 - cos(theta)", polar: true, u: ["0", "tau"], closed: true },
    Preset { name: "astroid", about: "a four-cusped star", params: &[], formula: "[cos(u)^3, sin(u)^3]", polar: false, u: ["0", "tau"], closed: true },
    Preset { name: "deltoid", about: "a three-cusped curve", params: &[], formula: "[2 * cos(u) + cos(2 * u), 2 * sin(u) - sin(2 * u)]", polar: false, u: ["0", "tau"], closed: true },
    Preset { name: "butterfly", about: "Fay's butterfly curve", params: &[], formula: "exp(sin(theta)) - 2 * cos(4 * theta) + sin((2 * theta - pi) / 24)^5", polar: true, u: ["0", "24 * pi"], closed: false },
    Preset { name: "lemniscate", about: "Bernoulli's lemniscate: a figure of eight, infinity", params: &[], formula: "[cos(u) / (1 + sin(u)^2), sin(u) * cos(u) / (1 + sin(u)^2)]", polar: false, u: ["0", "tau"], closed: true },
    Preset { name: "maurer_rose", about: "a Maurer rose: straight lines between points of a rose, `step` degrees apart", params: &[("n", 6.0), ("step", 71.0)], formula: "a = u * step * pi / 180; [sin(n * a) * cos(a), sin(n * a) * sin(a)]", polar: false, u: ["0", "360"], closed: true },
    Preset { name: "wave", about: "a sine wave, `cycles` of it, travelling with `speed`", params: &[("cycles", 3.0), ("amount", 0.25), ("speed", 1.0)], formula: "[u, amount * sin(cycles * pi * u - t * speed * tau)]", polar: false, u: ["-1", "1"], closed: false },
];

/// The implicit presets: f(x, y) = 0.
pub const IMPLICIT: &[ImplicitPreset] = &[
    ("cassini", "Cassini's ovals: one loop, a figure of eight, or two eggs as a passes b", &[("a", 0.7), ("b", 0.72)], "(x^2 + y^2)^2 - 2 * a^2 * (x^2 - y^2) - (b^4 - a^4)"),
    ("folium", "Descartes' folium", &[("a", 0.6)], "x^3 + y^3 - 3 * a * x * y"),
    ("devil", "the devil's curve", &[("a", 0.8), ("b", 0.9)], "y^2 * (y^2 - a^2) - x^2 * (x^2 - b^2)"),
    ("heart_implicit", "the heart (x² + y² − 1)³ = x² y³", &[], "(x^2 + y^2 - 1)^3 - x^2 * y^3"),
];

/// The L-system presets: axiom, rules, angle, iterations, and the symbols
/// that draw.
pub const LSYSTEMS: &[LsysPreset] = &[
    ("koch", "Koch's snowflake", "F--F--F", &[('F', "F+F--F+F")], 60.0, 4, "F"),
    ("dragon", "the Heighway dragon", "FX", &[('X', "X+YF+"), ('Y', "-FX-Y")], 90.0, 11, "F"),
    ("hilbert", "Hilbert's space-filling curve", "A", &[('A', "+BF-AFA-FB+"), ('B', "-AF+BFB-FA+")], 90.0, 5, "F"),
    ("sierpinski", "Sierpinski's arrowhead curve", "A", &[('A', "B-A-B"), ('B', "A+B+A")], 60.0, 7, "AB"),
    ("gosper", "Gosper's flowsnake", "A", &[('A', "A-B--B+A++AA+B-"), ('B', "+A-BB--B-A++A+B")], 60.0, 4, "AB"),
    ("levy", "the Lévy C curve", "F", &[('F', "+F--F+")], 45.0, 12, "F"),
    ("plant", "a plant (Lindenmayer's fig. 1.24f)", "X", &[('X', "F+[[X]-X]-F[-FX]+X"), ('F', "FF")], 25.0, 5, "F"),
    ("fern", "a fern frond", "X", &[('X', "F[+X]F[-X]+X"), ('F', "FF")], 22.0, 6, "F"),
    ("square_tiles", "the quadratic Koch island", "F-F-F-F", &[('F', "F-F+F+FF-F-F+F")], 90.0, 3, "F"),
];

/// Every preset name, for messages.
pub fn names() -> String {
    let mut n: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
    n.extend(IMPLICIT.iter().map(|p| p.0));
    n.extend(LSYSTEMS.iter().map(|p| p.0));
    n.join(", ")
}

fn compile(what: &str, src: &str, names: &[&str]) -> Result<Program, String> {
    Program::compile(src, names).map_err(|e| format!("{what} \"{src}\": {e}"))
}

/// Settings bound in front of a formula (`;` between, so a body starting
/// with `-` is its own statement).
fn bind(params: &[(&str, f64)], num: &dyn Fn(&str, f64) -> f64, body: &str) -> String {
    let mut s: String = params.iter().map(|(n, d)| format!("{n} = {:?}; ", num(n, *d))).collect();
    s.push_str(body);
    s
}

/// A range end: a number or a formula of the settings.
fn end(v: Option<&Value>, default: &str, params: &[(&str, f64)], num: &dyn Fn(&str, f64) -> f64) -> Result<f64, String> {
    match v {
        Some(Value::Number(n)) => Ok(n.as_f64().unwrap_or(0.0)),
        Some(Value::String(s)) => Ok(compile("range", &bind(params, num, s), &[])?.eval(&[])),
        Some(other) => Err(format!("a range end is a number or a formula, not {other}")),
        None => Ok(compile("range", &bind(params, num, default), &[])?.eval(&[])),
    }
}

/// The curve an item (or an `animate` op's `curve`) describes, at time
/// `t`; `num` reads a setting (animated, perhaps) by name.
pub fn make(o: &Map<String, Value>, t: f64, num: &dyn Fn(&str, f64) -> f64) -> Result<Lines, String> {
    let samples = (num("samples", 720.0).round().max(8.0) as usize).min(MAX_SAMPLES);
    let mut evals = 0u64;
    let mut used = samples;
    let (lines, px, frame): (Polys, bool, Option<Vec<P>>) = if let Some(f) = o.get("fourier") {
        let (lines, frame, px, n) = fourier(f, t, num)?;
        evals += n;
        (lines, px, Some(frame))
    } else if let Some(l) = o.get("lsystem") {
        (lsystem(l, num)?, false, None)
    } else {
        let preset = o.get("preset").and_then(Value::as_str);
        if let Some(name) = preset
            && let Some(p) = LSYSTEMS.iter().find(|p| p.0 == name)
        {
            let mut spec = Map::new();
            spec.insert("preset".into(), Value::from(p.0));
            for k in ["iterations", "angle"] {
                if let Some(v) = o.get(k) {
                    spec.insert(k.into(), v.clone());
                }
            }
            (lsystem(&Value::Object(spec), num)?, false, None)
        } else if let Some(p) = preset.and_then(|n| IMPLICIT.iter().find(|p| p.0 == n)) {
            let (l, n, res) = implicit(&bind(p.2, num, p.3), o, t, num)?;
            evals += n;
            used = res;
            (l, false, None)
        } else if let Some(src) = o.get("implicit").and_then(Value::as_str) {
            let (l, n, res) = implicit(src, o, t, num)?;
            evals += n;
            used = res;
            (l, false, None)
        } else {
            let (prog, polar, range, closed) = match preset {
                Some(name) => {
                    let p = PRESETS.iter().find(|p| p.name == name).ok_or_else(|| format!("curve preset \"{name}\": {}", names()))?;
                    let names = if p.polar { &["theta", "t", "u", "i"][..] } else { &["u", "t", "theta", "i"][..] };
                    let prog = compile(name, &bind(p.params, num, p.formula), names)?;
                    let a = end(o.get("u").and_then(|u| u.get(0)), p.u[0], p.params, num)?;
                    let b = end(o.get("u").and_then(|u| u.get(1)), p.u[1], p.params, num)?;
                    (vec![prog], p.polar, [a, b], p.closed)
                }
                None => {
                    if let Some(r) = o.get("polar").and_then(Value::as_str) {
                        let prog = compile("polar", r, &["theta", "t", "u", "i"])?;
                        let a = end(o.get("u").and_then(|u| u.get(0)), "0", &[], num)?;
                        let b = end(o.get("u").and_then(|u| u.get(1)), "tau", &[], num)?;
                        (vec![prog], true, [a, b], o.get("closed").and_then(Value::as_bool).unwrap_or(true))
                    } else if let (Some(x), Some(y)) = (o.get("x").and_then(Value::as_str), o.get("y").and_then(Value::as_str)) {
                        let names = &["u", "t", "theta", "i"];
                        let a = end(o.get("u").and_then(|u| u.get(0)), "0", &[], num)?;
                        let b = end(o.get("u").and_then(|u| u.get(1)), "tau", &[], num)?;
                        (vec![compile("x", x, names)?, compile("y", y, names)?], false, [a, b], o.get("closed").and_then(Value::as_bool).unwrap_or(false))
                    } else {
                        return Err(format!("a curve is a \"preset\" ({}), \"x\" and \"y\" (formulas of u and t), \"polar\" (r of theta), \"implicit\" (f(x, y) = 0), an \"lsystem\" or a \"fourier\" of an outline", names()));
                    }
                }
            };
            let mut pts = Vec::with_capacity(samples);
            for k in 0..samples {
                let u = range[0] + (range[1] - range[0]) * k as f64 / (samples - 1) as f64;
                let p = if polar {
                    let r = prog[0].eval(&[u, t, u, 0.0]);
                    evals += 1;
                    [r * u.cos(), r * u.sin()]
                } else if prog.len() == 2 {
                    evals += 2;
                    [prog[0].eval(&[u, t, u, 0.0]), prog[1].eval(&[u, t, u, 0.0])]
                } else {
                    evals += 2;
                    [prog[0].eval(&[u, t, u, 0.0]), prog[0].eval(&[u, t, u, 1.0])]
                };
                if p[0].is_finite() && p[1].is_finite() {
                    pts.push(p);
                }
            }
            if closed && pts.len() > 2 && dist(pts[0], pts[pts.len() - 1]) < 1e-9 * (1.0 + span(&pts)) {
                pts.pop();
            }
            (vec![(pts, closed)], false, None)
        }
    };
    let mut lines = lines;
    lines.retain(|(l, _)| l.len() >= 2);
    if lines.is_empty() {
        return Err("the curve has no points (its formula is undefined over the range?)".into());
    }
    // Fit: `size` px across, centred; or `scale` px per unit about the
    // formula's origin; outlines given in pixels stay as they are.
    let all: Vec<P> = frame.unwrap_or_else(|| lines.iter().flat_map(|(l, _)| l.iter().copied()).collect());
    let (lo, hi) = bounds(&all);
    let given_size = o.get("size").map(|_| num("size", 400.0));
    let lsys = o.get("lsystem").is_some() || o.get("preset").and_then(Value::as_str).is_some_and(|n| LSYSTEMS.iter().any(|p| p.0 == n));
    let (k, c) = match given_size {
        Some(s) => (s / (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-12), [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]),
        None if lsys => (400.0 / (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-12), [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]),
        None => (num("scale", if px { 1.0 } else { 100.0 }), [0.0, 0.0]),
    };
    let lines = lines.into_iter().map(|(l, closed)| (l.into_iter().map(|p| [(p[0] - c[0]) * k, -(p[1] - c[1]) * k]).collect(), closed)).collect();
    Ok(Lines { lines, evals, samples: used })
}

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn span(p: &[P]) -> f64 {
    let (lo, hi) = bounds(p);
    (hi[0] - lo[0]).max(hi[1] - lo[1])
}

fn bounds(p: &[P]) -> (P, P) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for q in p {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    if p.is_empty() { ([0.0; 2], [0.0; 2]) } else { (lo, hi) }
}

/// f(x, y) = 0 by marching squares over `bounds` (half side, or
/// `[[x0, y0], [x1, y1]]`), `resolution` cells along the longer side.
fn implicit(src: &str, o: &Map<String, Value>, t: f64, num: &dyn Fn(&str, f64) -> f64) -> Result<(Polys, u64, usize), String> {
    let f = compile("implicit", src, &["x", "y", "t", "r"])?;
    let (lo, hi) = match o.get("bounds") {
        Some(Value::Array(a)) if a.len() == 2 && a[0].is_array() => {
            let p = |v: &Value| [v.get(0).and_then(Value::as_f64).unwrap_or(-1.0), v.get(1).and_then(Value::as_f64).unwrap_or(-1.0)];
            (p(&a[0]), p(&a[1]))
        }
        _ => {
            let h = num("bounds", 1.5).abs().max(1e-9);
            ([-h, -h], [h, h])
        }
    };
    if !(hi[0] > lo[0] && hi[1] > lo[1]) {
        return Err("bounds: the low corner must be below the high corner".into());
    }
    let res = (num("resolution", 240.0).round().max(8.0) as usize).min(MAX_RESOLUTION);
    let step = (hi[0] - lo[0]).max(hi[1] - lo[1]) / res as f64;
    let (nx, ny) = (((hi[0] - lo[0]) / step).ceil() as usize + 1, ((hi[1] - lo[1]) / step).ceil() as usize + 1);
    let at = |i: usize, j: usize| [lo[0] + i as f64 * step, lo[1] + j as f64 * step];
    let v: Vec<f64> = (0..nx * ny)
        .map(|k| {
            let p = at(k % nx, k / nx);
            let x = f.eval(&[p[0], p[1], t, p[0].hypot(p[1])]);
            if x.is_finite() { x } else { 1e30 }
        })
        .collect();
    let val = |i: usize, j: usize| v[j * nx + i];
    // Crossings named by their edge: (i, j, 0) along x, (i, j, 1) along y.
    let cross = |e: (usize, usize, u8)| {
        let (a, b) = if e.2 == 0 { ((e.0, e.1), (e.0 + 1, e.1)) } else { ((e.0, e.1), (e.0, e.1 + 1)) };
        let (va, vb) = (val(a.0, a.1), val(b.0, b.1));
        let s = (va / (va - vb)).clamp(0.0, 1.0);
        let (pa, pb) = (at(a.0, a.1), at(b.0, b.1));
        [pa[0] + (pb[0] - pa[0]) * s, pa[1] + (pb[1] - pa[1]) * s]
    };
    type Edge = (usize, usize, u8);
    let mut segs: Vec<(Edge, Edge)> = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let c = [val(i, j), val(i + 1, j), val(i + 1, j + 1), val(i, j + 1)];
            let inside = c.map(|x| x < 0.0);
            let case = inside.iter().enumerate().fold(0u8, |m, (k, b)| m | ((*b as u8) << k));
            let (bottom, right, top, left) = ((i, j, 0u8), (i + 1, j, 1u8), (i, j + 1, 0u8), (i, j, 1u8));
            let centre_in = (c[0] + c[1] + c[2] + c[3]) < 0.0;
            match case {
                0 | 15 => {}
                1 | 14 => segs.push((left, bottom)),
                2 | 13 => segs.push((bottom, right)),
                3 | 12 => segs.push((left, right)),
                4 | 11 => segs.push((right, top)),
                6 | 9 => segs.push((bottom, top)),
                7 | 8 => segs.push((left, top)),
                5 => {
                    if centre_in {
                        segs.push((left, top));
                        segs.push((bottom, right));
                    } else {
                        segs.push((left, bottom));
                        segs.push((right, top));
                    }
                }
                10 => {
                    if centre_in {
                        segs.push((left, bottom));
                        segs.push((right, top));
                    } else {
                        segs.push((left, top));
                        segs.push((bottom, right));
                    }
                }
                _ => {}
            }
        }
    }
    // Join the pieces end to end into lines.
    let mut by_edge: std::collections::HashMap<(usize, usize, u8), Vec<usize>> = std::collections::HashMap::new();
    for (k, (a, b)) in segs.iter().enumerate() {
        by_edge.entry(*a).or_default().push(k);
        by_edge.entry(*b).or_default().push(k);
    }
    let mut used = vec![false; segs.len()];
    let mut lines = Vec::new();
    for s0 in 0..segs.len() {
        if used[s0] {
            continue;
        }
        used[s0] = true;
        let mut chain = vec![segs[s0].0, segs[s0].1];
        // Forward from the end, then back from the start.
        for dir in 0..2 {
            loop {
                let tip = if dir == 0 { *chain.last().expect("two") } else { chain[0] };
                let Some(&next) = by_edge.get(&tip).and_then(|l| l.iter().find(|&&k| !used[k])) else { break };
                used[next] = true;
                let other = if segs[next].0 == tip { segs[next].1 } else { segs[next].0 };
                if dir == 0 {
                    chain.push(other);
                } else {
                    chain.insert(0, other);
                }
            }
        }
        let closed = chain.len() > 3 && chain[0] == chain[chain.len() - 1];
        if closed {
            chain.pop();
        }
        lines.push((chain.into_iter().map(cross).collect(), closed));
    }
    Ok((lines, (nx * ny) as u64, res))
}

/// An L-system's lines: `{"axiom", "rules": {"F": "F+F", "X": ["…", "…"]},
/// "iterations", "angle", "draw"}` or `{"preset": name}` with changes.
fn lsystem(spec: &Value, num: &dyn Fn(&str, f64) -> f64) -> Result<Polys, String> {
    let o = spec.as_object().ok_or("lsystem is {\"axiom\", \"rules\", \"iterations\", \"angle\"} or {\"preset\": name}")?;
    let preset = o.get("preset").and_then(Value::as_str).map(|n| LSYSTEMS.iter().find(|p| p.0 == n).ok_or_else(|| format!("L-system preset \"{n}\": {}", LSYSTEMS.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")))).transpose()?;
    let axiom = o.get("axiom").and_then(Value::as_str).or(preset.map(|p| p.2)).ok_or("an lsystem needs \"axiom\" (or a \"preset\")")?;
    let mut rules: Vec<(char, Vec<String>)> = preset.map(|p| p.3.iter().map(|(c, r)| (*c, vec![r.to_string()])).collect()).unwrap_or_default();
    if let Some(r) = o.get("rules") {
        let r = r.as_object().ok_or("rules is {\"F\": \"F+F\", …} (a symbol to what it becomes, or to a list of choices)")?;
        for (k, v) in r {
            let mut cs = k.chars();
            let (Some(c), None) = (cs.next(), cs.next()) else { return Err(format!("a rule's key is one symbol, not \"{k}\"")) };
            let choices: Vec<String> = match v {
                Value::String(s) => vec![s.clone()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                _ => return Err(format!("rule {k} is a string or a list of strings")),
            };
            rules.retain(|(x, _)| *x != c);
            rules.push((c, choices));
        }
    }
    let it = o.get("iterations").and_then(Value::as_f64).unwrap_or(preset.map_or(4.0, |p| p.5 as f64)).clamp(0.0, 16.0) as usize;
    let angle = o.get("angle").and_then(Value::as_f64).unwrap_or_else(|| num("angle", preset.map_or(25.0, |p| p.4)));
    let draw = o.get("draw").and_then(Value::as_str).unwrap_or(preset.map_or("FG", |p| p.6)).to_owned();
    let mut s = axiom.to_owned();
    let mut h: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..it {
        let mut next = String::with_capacity(s.len() * 3);
        for c in s.chars() {
            match rules.iter().find(|(k, _)| *k == c) {
                Some((_, ch)) if !ch.is_empty() => {
                    h ^= h << 13;
                    h ^= h >> 7;
                    h ^= h << 17;
                    next.push_str(&ch[(h % ch.len() as u64) as usize]);
                }
                _ => next.push(c),
            }
            if next.len() > MAX_SYMBOLS {
                return Err(format!("the L-system grows past {MAX_SYMBOLS} symbols by iteration {}; use fewer iterations", it));
            }
        }
        s = next;
    }
    let a = angle.to_radians();
    let (mut p, mut dir) = ([0.0f64, 0.0], 0.0f64);
    let mut stack: Vec<(P, f64)> = Vec::new();
    let mut lines: Vec<(Vec<P>, bool)> = vec![(vec![p], false)];
    for c in s.chars() {
        if draw.contains(c) {
            p = [p[0] + dir.cos(), p[1] + dir.sin()];
            if let Some(l) = lines.last_mut() {
                l.0.push(p);
            }
            continue;
        }
        match c {
            'f' => {
                p = [p[0] + dir.cos(), p[1] + dir.sin()];
                lines.push((vec![p], false));
            }
            '+' => dir += a,
            '-' => dir -= a,
            '|' => dir += std::f64::consts::PI,
            '[' => stack.push((p, dir)),
            ']' => {
                if let Some((q, d)) = stack.pop() {
                    (p, dir) = (q, d);
                    lines.push((vec![p], false));
                }
            }
            _ => {}
        }
    }
    // A closed figure (the snowflake) comes back to where it began.
    for (l, closed) in &mut lines {
        if l.len() > 3 && dist(l[0], l[l.len() - 1]) < 1e-6 {
            l.pop();
            *closed = true;
        }
    }
    Ok(lines)
}

/// Points along an outline, `n` evenly spaced by length (closed).
fn resample(pts: &[P], n: usize) -> Vec<P> {
    let mut ring: Vec<P> = pts.to_vec();
    if let Some(f) = ring.first().copied() {
        ring.push(f);
    }
    let lens: Vec<f64> = ring.windows(2).map(|w| dist(w[0], w[1])).collect();
    let total: f64 = lens.iter().sum::<f64>().max(1e-12);
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut acc) = (0usize, 0.0f64);
    for k in 0..n {
        let want = total * k as f64 / n as f64;
        while seg + 1 < lens.len() && acc + lens[seg] < want {
            acc += lens[seg];
            seg += 1;
        }
        let s = ((want - acc) / lens[seg].max(1e-12)).clamp(0.0, 1.0);
        out.push([ring[seg][0] + (ring[seg + 1][0] - ring[seg][0]) * s, ring[seg][1] + (ring[seg + 1][1] - ring[seg][1]) * s]);
    }
    out
}

/// The terms (frequency, coefficient) of an outline's Fourier series, the
/// strongest `terms` of them.
pub fn fourier_terms(outline: &[P], terms: usize) -> Vec<(i64, [f64; 2])> {
    let n = 1024;
    let z = resample(outline, n);
    let kmax = 256i64;
    let mut c: Vec<(i64, [f64; 2])> = (-kmax..=kmax)
        .map(|k| {
            let mut s = [0.0, 0.0];
            for (j, p) in z.iter().enumerate() {
                let a = -std::f64::consts::TAU * k as f64 * j as f64 / n as f64;
                let (sa, ca) = a.sin_cos();
                s[0] += p[0] * ca - p[1] * sa;
                s[1] += p[0] * sa + p[1] * ca;
            }
            (k, [s[0] / n as f64, s[1] / n as f64])
        })
        .collect();
    // The centre first, then by strength.
    c.sort_by(|a, b| (b.0 == 0).cmp(&(a.0 == 0)).then(b.1[0].hypot(b.1[1]).total_cmp(&a.1[0].hypot(a.1[1]))));
    c.truncate(terms.clamp(1, MAX_TERMS));
    c
}

/// The series' point at `s` (0..1 round the outline).
pub fn fourier_at(c: &[(i64, [f64; 2])], s: f64) -> P {
    c.iter().fold([0.0, 0.0], |acc, (k, z)| {
        let (sa, ca) = (std::f64::consts::TAU * *k as f64 * s).sin_cos();
        [acc[0] + z[0] * ca - z[1] * sa, acc[1] + z[0] * sa + z[1] * ca]
    })
}

/// Fourier epicycles: `{"of": points | "<svg path>" | preset name | {curve}, "terms",
/// "epicycles", "period", "trace"}`. Returns the lines, the whole outline
/// (to fit by), whether it is in pixels, and the evaluations.
fn fourier(spec: &Value, t: f64, num: &dyn Fn(&str, f64) -> f64) -> Result<(Polys, Vec<P>, bool, u64), String> {
    let o = spec.as_object().ok_or("fourier is {\"of\": an outline, \"terms\": 20, \"epicycles\": true, \"period\": 4}")?;
    let of = o.get("of").ok_or("fourier needs \"of\": a list of points [[x, y], …] (px), an SVG path (\"M 0 0 L …\"), a curve preset name, or a curve {…}")?;
    let mut evals = 0;
    let (outline, px): (Vec<P>, bool) = match of {
        Value::Array(a) => (a.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, -p.get(1)?.as_f64()?])).collect(), true),
        Value::String(s) if PRESETS.iter().any(|p| p.name == s) || IMPLICIT.iter().any(|p| p.0 == s) => {
            let mut m = Map::new();
            m.insert("preset".into(), Value::from(s.as_str()));
            let l = make(&m, t, &|_, d| d)?;
            evals += l.evals;
            (l.lines.into_iter().next().map(|(l, _)| l.into_iter().map(|p| [p[0] / 100.0, -p[1] / 100.0]).collect()).unwrap_or_default(), false)
        }
        Value::String(d) => {
            let path = crate::path::parse_svg(d)?;
            (path.polyline(0.5).into_iter().map(|p| [p[0], -p[1]]).collect(), true)
        }
        Value::Object(m) => {
            let l = make(m, t, &|k, d| m.get(k).and_then(Value::as_f64).unwrap_or(d))?;
            evals += l.evals;
            (l.lines.into_iter().next().map(|(l, _)| l.into_iter().map(|p| [p[0], -p[1]]).collect()).unwrap_or_default(), true)
        }
        other => return Err(format!("fourier \"of\" is points, an SVG path, a preset name or a curve, not {other}")),
    };
    if outline.len() < 3 {
        return Err("fourier needs an outline of three or more points".into());
    }
    let terms = num("terms", 24.0).round().max(1.0) as usize;
    let c = fourier_terms(&outline, terms);
    evals += 1024 * 513;
    let full: Vec<P> = (0..720).map(|k| fourier_at(&c, k as f64 / 720.0)).collect();
    let epicycles = o.get("epicycles").and_then(Value::as_bool).unwrap_or(true);
    if !epicycles {
        return Ok((vec![(full.clone(), true)], full, px, evals));
    }
    let period = o.get("period").and_then(Value::as_f64).unwrap_or(4.0).max(0.05);
    let phase = (t / period).rem_euclid(1.0);
    let mut lines = Vec::new();
    // Circles and the arm through their centres, at this phase.
    let mut arm = vec![[0.0, 0.0]];
    let mut at = [0.0, 0.0];
    for (k, z) in &c {
        let (sa, ca) = (std::f64::consts::TAU * *k as f64 * phase).sin_cos();
        let v = [z[0] * ca - z[1] * sa, z[0] * sa + z[1] * ca];
        let r = z[0].hypot(z[1]);
        if *k != 0 && r > 1e-9 {
            lines.push(((0..48).map(|j| {
                let a = std::f64::consts::TAU * j as f64 / 48.0;
                [at[0] + r * a.cos(), at[1] + r * a.sin()]
            }).collect(), true));
        }
        at = [at[0] + v[0], at[1] + v[1]];
        arm.push(at);
    }
    // The arm starts at the centre (the first term).
    arm.remove(0);
    lines.push((arm, false));
    // The trace drawn so far.
    let upto = ((phase * 720.0).ceil() as usize).clamp(1, 720);
    let mut trace: Vec<P> = full[..upto].to_vec();
    trace.push(fourier_at(&c, phase));
    if o.get("trace").and_then(Value::as_bool).unwrap_or(true) {
        lines.push((trace, false));
    }
    Ok((lines, full, px, evals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn curve(v: Value, t: f64) -> Lines {
        let o = v.as_object().unwrap().clone();
        make(&o, t, &|k, d| o.get(k).and_then(Value::as_f64).unwrap_or(d)).unwrap_or_else(|e| panic!("{v}: {e}"))
    }

    #[test]
    fn a_rose_has_its_petals_and_fits_its_size() {
        let l = curve(json!({"preset": "rose", "n": 5, "size": 300}), 0.0);
        let pts = &l.lines[0].0;
        let (lo, hi) = bounds(pts);
        assert!(((hi[0] - lo[0]).max(hi[1] - lo[1]) - 300.0).abs() < 1.0);
        // Five tips at the full radius.
        let far = pts.iter().filter(|p| p[0].hypot(p[1]) > 140.0).count();
        assert!(far > 5, "{far}");
        assert!(l.lines[0].1 && l.evals >= 720);
    }

    #[test]
    fn parametric_polar_and_formula_curves_go_where_their_formulas_say() {
        // A circle of radius 1 at 100 px a unit; y is down on the canvas.
        let l = curve(json!({"x": "cos(u)", "y": "sin(u)", "samples": 9, "u": [0, "pi"]}), 0.0);
        let p = &l.lines[0].0;
        assert_eq!(p.len(), 9);
        assert!((p[0][0] - 100.0).abs() < 1e-9 && p[4][1] < -99.99, "{:?}", p[4]);
        let r = curve(json!({"polar": "1 + 0 * theta"}), 0.0);
        assert!(r.lines[0].0.iter().all(|q| (q[0].hypot(q[1]) - 100.0).abs() < 1e-6));
        // Time moves a wave.
        let a = curve(json!({"preset": "wave"}), 0.0);
        let b = curve(json!({"preset": "wave"}), 0.25);
        assert!(a.lines[0].0[100][1] != b.lines[0].0[100][1]);
        assert!(make(&Map::new(), 0.0, &|_, d| d).unwrap_err().contains("preset"));
    }

    #[test]
    fn an_implicit_circle_is_one_closed_loop_on_its_radius() {
        let l = curve(json!({"implicit": "x^2 + y^2 - 1", "bounds": 1.5, "resolution": 120}), 0.0);
        assert_eq!(l.lines.len(), 1);
        assert!(l.lines[0].1, "closed");
        assert!(l.lines[0].0.iter().all(|q| (q[0].hypot(q[1]) - 100.0).abs() < 0.5));
        // Cassini ovals as two eggs.
        let two = curve(json!({"preset": "cassini", "a": 0.8, "b": 0.7}), 0.0);
        assert_eq!(two.lines.len(), 2, "{}", two.lines.len());
        assert!(curve(json!({"implicit": "x", "resolution": 1e9}), 0.0).samples <= MAX_RESOLUTION);
    }

    #[test]
    fn l_systems_draw_their_fractals() {
        let k = curve(json!({"preset": "koch", "iterations": 2}), 0.0);
        // A snowflake of 3·4² edges, closed.
        assert_eq!(k.lines[0].0.len(), 48);
        assert!(k.lines[0].1);
        let p = curve(json!({"lsystem": {"axiom": "F", "rules": {"F": "F[+F]F[-F]F"}, "iterations": 2, "angle": 25}}), 0.0);
        assert!(p.lines.len() > 5, "branches are lines of their own");
        let o = json!({"lsystem": {"axiom": "F", "rules": {"F": "FFFFFFFFFF"}, "iterations": 9}});
        assert!(make(o.as_object().unwrap(), 0.0, &|_, d| d).unwrap_err().contains("symbols"));
    }

    #[test]
    fn fourier_epicycles_trace_the_outline() {
        // A square, 200 px: with enough terms the series is back on it.
        let sq = json!([[-100, -100], [100, -100], [100, 100], [-100, 100]]);
        let l = curve(json!({"fourier": {"of": sq, "terms": 120, "epicycles": false}}), 0.0);
        let worst = l.lines[0].0.iter().map(|p| (p[0].abs().max(p[1].abs()) - 100.0).abs()).fold(0.0f64, f64::max);
        assert!(worst < 6.0, "{worst}");
        // Drawn by circles: the arm's tip is on the trace at this phase.
        let e = curve(json!({"fourier": {"of": "heart", "terms": 12, "period": 4}}), 1.0);
        let arm = &e.lines[e.lines.len() - 2].0;
        let trace = &e.lines[e.lines.len() - 1].0;
        let tip = arm[arm.len() - 1];
        let end = trace[trace.len() - 1];
        assert!(dist(tip, end) < 1e-6, "{tip:?} {end:?}");
        assert!(e.lines.len() >= 12, "circles, the arm and the trace");
        assert!(trace.len() < 720 / 4 + 3, "a quarter drawn at a quarter of the period");
    }
}
