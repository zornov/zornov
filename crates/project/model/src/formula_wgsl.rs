// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Formulas as WGSL, so the GPU can work a shape out itself (sphere tracing
//! a distance field per pixel, with no mesh in between). [`Program::to_wgsl`]
//! writes one function per formula; [`PRELUDE`] holds the language's
//! functions written again in WGSL, bounded the same way (fractals cap
//! their iterations, there are no other loops). The shape functions (3D
//! noise, fbm, Voronoi, the distances, the minimal surfaces, the fractals)
//! give what the CPU gives, to f32 precision, so a shape drawn directly
//! matches its mesh; the 1D `noise` and `random` of property formulas use
//! another hash on the GPU and only look alike.

use super::expr::Expr;
use super::formula::Program;

/// The language's functions in WGSL: put once in front of the functions
/// [`Program::to_wgsl`] writes.
pub const PRELUDE: &str = r#"
fn fm_nan() -> f32 { return bitcast<f32>(0x7fc00000u); }
fn fm_sq(v: f32) -> f32 { return v * v; }
fn fm_mod(a: f32, b: f32) -> f32 { let m = abs(b); return a - m * floor(a / m); }
fn fm_pow(a: f32, b: f32) -> f32 {
    if (a >= 0.0) { return pow(a, b); }
    if (fract(b) == 0.0) {
        let p = pow(-a, b);
        return select(p, -p, fm_mod(b, 2.0) == 1.0);
    }
    return fm_nan();
}
fn fm_round(x: f32) -> f32 { return select(ceil(x - 0.5), floor(x + 0.5), x >= 0.0); }
fn fm_cbrt(x: f32) -> f32 { return sign(x) * pow(abs(x), 1.0 / 3.0); }
fn fm_b(c: bool) -> f32 { return select(0.0, 1.0, c); }
fn fm_unit(x: f32, a: f32, b: f32) -> f32 {
    if (abs(b - a) < 1e-12) { return fm_b(x >= b); }
    return clamp((x - a) / (b - a), 0.0, 1.0);
}
fn fm_smoothstep(e0: f32, e1: f32, x: f32) -> f32 { let u = fm_unit(x, e0, e1); return u * u * (3.0 - 2.0 * u); }
fn fm_map(kind: i32, x: f32, a: f32, b: f32, v1: f32, v2: f32) -> f32 {
    let u = fm_unit(x, a, b);
    var k = u;
    if (kind == 1) { k = u * u * u * (u * (u * 6.0 - 15.0) + 10.0); }
    if (kind == 2) { k = u * u * u; }
    if (kind == 3) { k = 1.0 - pow(1.0 - u, 3.0); }
    return v1 + (v2 - v1) * k;
}
fn fm_posterize(x: f32, n: f32) -> f32 { return select(floor(x * n) / n, x, n <= 0.0); }
fn fm_smin(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 0.0) { return min(a, b); }
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return b + (a - b) * h - k * h * (1.0 - h);
}
fn fm_smax(a: f32, b: f32, k: f32) -> f32 { return -fm_smin(-a, -b, k); }
fn fm_seed(s: f32) -> u32 { return u32(max(s, 0.0)); }
fn fm_hash(i: vec3<i32>, seed: u32) -> u32 {
    var h = (bitcast<u32>(i.x) * 0x8da6b343u) ^ (bitcast<u32>(i.y) * 0xd8163841u) ^ (bitcast<u32>(i.z) * 0xcb1ab31fu) ^ (seed * 0x9e3779b9u);
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    return h ^ (h >> 16u);
}
fn fm_noise1(x: f32, seed: u32) -> f32 {
    let i = floor(x);
    let f = x - i;
    let ga = f32(fm_hash(vec3<i32>(i32(i), 0, 0), seed)) / 4294967296.0 * 2.0 - 1.0;
    let gb = f32(fm_hash(vec3<i32>(i32(i) + 1, 0, 0), seed)) / 4294967296.0 * 2.0 - 1.0;
    let a = ga * f;
    let b = gb * (f - 1.0);
    let s = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    return 2.0 * (a + (b - a) * s);
}
fn fm_rand01(x: f32) -> f32 { return f32(fm_hash(vec3<i32>(i32(fm_round(x * 1000.0)), 99, 0), 99u)) / 4294967296.0; }
fn fm_grad(h: u32) -> vec3<f32> {
    switch (h % 12u) {
        case 0u: { return vec3<f32>(1.0, 1.0, 0.0); }
        case 1u: { return vec3<f32>(-1.0, 1.0, 0.0); }
        case 2u: { return vec3<f32>(1.0, -1.0, 0.0); }
        case 3u: { return vec3<f32>(-1.0, -1.0, 0.0); }
        case 4u: { return vec3<f32>(1.0, 0.0, 1.0); }
        case 5u: { return vec3<f32>(-1.0, 0.0, 1.0); }
        case 6u: { return vec3<f32>(1.0, 0.0, -1.0); }
        case 7u: { return vec3<f32>(-1.0, 0.0, -1.0); }
        case 8u: { return vec3<f32>(0.0, 1.0, 1.0); }
        case 9u: { return vec3<f32>(0.0, -1.0, 1.0); }
        case 10u: { return vec3<f32>(0.0, 1.0, -1.0); }
        default: { return vec3<f32>(0.0, -1.0, -1.0); }
    }
}
fn fm_fade(f: f32) -> f32 { return f * f * f * (f * (f * 6.0 - 15.0) + 10.0); }
fn fm_corner(ii: vec3<i32>, f: vec3<f32>, d: vec3<i32>, seed: u32) -> f32 { return dot(fm_grad(fm_hash(ii + d, seed)), f - vec3<f32>(d)); }
fn fm_noise3(p: vec3<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let ii = vec3<i32>(i);
    let u = fm_fade(f.x);
    let v = fm_fade(f.y);
    let w = fm_fade(f.z);
    let x00 = mix(fm_corner(ii, f, vec3<i32>(0, 0, 0), seed), fm_corner(ii, f, vec3<i32>(1, 0, 0), seed), u);
    let x10 = mix(fm_corner(ii, f, vec3<i32>(0, 1, 0), seed), fm_corner(ii, f, vec3<i32>(1, 1, 0), seed), u);
    let x01 = mix(fm_corner(ii, f, vec3<i32>(0, 0, 1), seed), fm_corner(ii, f, vec3<i32>(1, 0, 1), seed), u);
    let x11 = mix(fm_corner(ii, f, vec3<i32>(0, 1, 1), seed), fm_corner(ii, f, vec3<i32>(1, 1, 1), seed), u);
    return mix(mix(x00, x10, v), mix(x01, x11, v), w);
}
fn fm_fbm(p: vec3<f32>, octaves: f32, seed: u32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var freq = 1.0;
    let n = u32(clamp(fm_round(octaves), 1.0, 8.0));
    for (var o = 0u; o < n; o++) {
        sum += amp * fm_noise3(p * freq, seed + o * 101u);
        amp *= 0.5;
        freq *= 2.0;
    }
    return sum * 2.0;
}
fn fm_voronoi(p: vec3<f32>, flat: bool) -> vec2<f32> {
    let c = vec3<i32>(floor(p));
    var f1 = 3.0e38;
    var f2 = 3.0e38;
    let z0 = select(-1, 0, flat);
    let z1 = select(1, 0, flat);
    for (var dz = z0; dz <= z1; dz++) {
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let cell = c + vec3<i32>(dx, dy, dz);
                let h = fm_hash(cell, 7u);
                let j = vec3<f32>(f32(h & 1023u), f32((h >> 10u) & 1023u), f32((h >> 20u) & 1023u)) / 1023.0;
                let q = vec3<f32>(f32(cell.x) + j.x, f32(cell.y) + j.y, select(f32(cell.z) + j.z, p.z, flat));
                let d = length(q - p);
                if (d < f1) { f2 = f1; f1 = d; } else if (d < f2) { f2 = d; }
            }
        }
    }
    return vec2<f32>(f1, f2);
}
fn fm_sphere(p: vec3<f32>, r: f32) -> f32 { return length(p) - r; }
fn fm_box(p: vec3<f32>, b: vec3<f32>) -> f32 {
    let q = abs(p) - b;
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}
fn fm_torus(p: vec3<f32>, big: f32, r: f32) -> f32 { return length(vec2<f32>(length(p.xy) - big, p.z)) - r; }
fn fm_cylinder(p: vec3<f32>, r: f32, h: f32) -> f32 {
    let dx = length(p.xy) - r;
    let dz = abs(p.z) - h;
    return min(max(dx, dz), 0.0) + length(vec2<f32>(max(dx, 0.0), max(dz, 0.0)));
}
fn fm_gyroid(p: vec3<f32>) -> f32 { return sin(p.x) * cos(p.y) + sin(p.y) * cos(p.z) + sin(p.z) * cos(p.x); }
fn fm_schwarz_p(p: vec3<f32>) -> f32 { return cos(p.x) + cos(p.y) + cos(p.z); }
fn fm_schwarz_d(p: vec3<f32>) -> f32 {
    let s = sin(p);
    let c = cos(p);
    return s.x * s.y * s.z + s.x * c.y * c.z + c.x * s.y * c.z + c.x * c.y * s.z;
}
fn fm_neovius(p: vec3<f32>) -> f32 { return 3.0 * (cos(p.x) + cos(p.y) + cos(p.z)) + 4.0 * cos(p.x) * cos(p.y) * cos(p.z); }
fn fm_lidinoid(p: vec3<f32>) -> f32 {
    let x = p.x;
    let y = p.y;
    let z = p.z;
    return 0.5 * (sin(2.0 * x) * cos(y) * sin(z) + sin(2.0 * y) * cos(z) * sin(x) + sin(2.0 * z) * cos(x) * sin(y))
        - 0.5 * (cos(2.0 * x) * cos(2.0 * y) + cos(2.0 * y) * cos(2.0 * z) + cos(2.0 * z) * cos(2.0 * x)) + 0.15;
}
fn fm_iters(v: f32) -> i32 { return i32(clamp(fm_round(v), 1.0, 30.0)); }
fn fm_mandelbulb(p: vec3<f32>, power0: f32, it: f32) -> f32 {
    let power = clamp(power0, 2.0, 16.0);
    let n = fm_iters(it);
    var z = p;
    var dr = 1.0;
    for (var k = 0; k < n; k++) {
        let r = length(z);
        if (r > 2.0) { break; }
        if (r < 1e-12) { return -1e-3; }
        let theta = acos(clamp(z.z / r, -1.0, 1.0)) * power;
        let phi = atan2(z.y, z.x) * power;
        let zr = pow(r, power);
        dr = pow(r, power - 1.0) * power * dr + 1.0;
        z = zr * vec3<f32>(sin(theta) * cos(phi), sin(theta) * sin(phi), cos(theta)) + p;
    }
    let r = max(length(z), 1e-12);
    return 0.5 * log(r) * r / dr;
}
fn fm_mandelbox(p: vec3<f32>, scale: f32, it: f32) -> f32 {
    let n = fm_iters(it);
    var z = p;
    var dr = 1.0;
    var escaped = false;
    for (var k = 0; k < n; k++) {
        z = clamp(z, vec3<f32>(-1.0), vec3<f32>(1.0)) * 2.0 - z;
        let r2 = dot(z, z);
        var m = 1.0;
        if (r2 < 0.25) { m = 4.0; } else if (r2 < 1.0) { m = 1.0 / r2; }
        z = z * m * scale + p;
        dr = dr * m * abs(scale) + 1.0;
        if (length(z) > 100.0) { escaped = true; break; }
    }
    let d = length(z) / abs(dr);
    return select(-d, d, escaped);
}
fn fm_menger(p: vec3<f32>, it: f32) -> f32 {
    let n = min(fm_iters(it), 7);
    var d = fm_box(p, vec3<f32>(1.0));
    var s = 1.0;
    for (var k = 0; k < n; k++) {
        let a = vec3<f32>(fm_mod(p.x * s, 2.0), fm_mod(p.y * s, 2.0), fm_mod(p.z * s, 2.0)) - 1.0;
        s *= 3.0;
        let r = abs(1.0 - 3.0 * abs(a));
        let da = max(r.x, r.y);
        let db = max(r.y, r.z);
        let dc = max(r.z, r.x);
        let c = (min(da, min(db, dc)) - 1.0) / s;
        d = max(d, c);
    }
    return d;
}
fn fm_sierpinski(p: vec3<f32>, it: f32) -> f32 {
    let n = fm_iters(it);
    var z = p;
    for (var k = 0; k < n; k++) {
        if (z.x + z.y < 0.0) { z = vec3<f32>(-z.y, -z.x, z.z); }
        if (z.x + z.z < 0.0) { z = vec3<f32>(-z.z, z.y, -z.x); }
        if (z.y + z.z < 0.0) { z = vec3<f32>(z.x, -z.z, -z.y); }
        z = z * 2.0 - vec3<f32>(1.0);
    }
    let t = max(max(z.x + z.y + z.z - 1.0, -z.x - z.y + z.z - 1.0), max(-z.x + z.y - z.z - 1.0, z.x - z.y - z.z - 1.0)) / sqrt(3.0);
    return t * pow(2.0, -f32(n));
}
fn fm_superformula(angle: f32, m: f32, n1: f32, n2: f32, n3: f32, a: f32, b: f32) -> f32 {
    let k = m * angle / 4.0;
    let t = pow(abs(cos(k) / a), n2) + pow(abs(sin(k) / b), n3);
    if (t <= 0.0 || n1 == 0.0) { return 0.0; }
    return pow(t, -1.0 / n1);
}
"#;

/// An f32 literal.
fn lit(v: f64) -> String {
    let f = v as f32;
    if f.is_nan() {
        return "fm_nan()".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "3.0e38".into() } else { "(-3.0e38)".into() };
    }
    let mut s = format!("{f:?}");
    if !s.contains('.') && !s.contains('e') {
        s.push_str(".0");
    }
    if f < 0.0 { format!("({s})") } else { s }
}

struct Gen<'a> {
    body: String,
    scope: Vec<(&'a str, String)>,
    next: usize,
}

impl<'a> Gen<'a> {
    fn name(&self, n: &str) -> Option<String> {
        self.scope.iter().rev().find(|(k, _)| *k == n).map(|(_, v)| v.clone())
    }

    fn ex(&mut self, e: &'a Expr) -> Result<String, String> {
        Ok(match e {
            Expr::Num(v) => lit(*v),
            Expr::Str(s) => return Err(format!("\"{s}\": shape formulas take numbers, not words")),
            Expr::Var(n) | Expr::Index(n, _) => self.name(n).unwrap_or_else(|| "0.0".into()),
            Expr::List(items) => {
                let i = self.name("i").unwrap_or_else(|| "0.0".into());
                let mut out = self.ex(&items[items.len() - 1])?;
                for k in (0..items.len() - 1).rev() {
                    out = format!("select({out}, {}, {i} < {})", self.ex(&items[k])?, lit(k as f64 + 0.5));
                }
                out
            }
            Expr::Neg(x) => format!("(-{})", self.ex(x)?),
            Expr::Not(x) => format!("fm_b({} == 0.0)", self.ex(x)?),
            Expr::Bin(op, l, r) => {
                let (a, c) = (self.ex(l)?, self.ex(r)?);
                match op {
                    '+' | '-' | '*' | '/' => format!("({a} {op} {c})"),
                    '%' => format!("fm_mod({a}, {c})"),
                    '^' => format!("fm_pow({a}, {c})"),
                    '<' | '>' => format!("fm_b({a} {op} {c})"),
                    'l' => format!("fm_b({a} <= {c})"),
                    'g' => format!("fm_b({a} >= {c})"),
                    '=' => format!("fm_b(abs({a} - {c}) < 1e-6)"),
                    '&' => format!("fm_b({a} != 0.0 && {c} != 0.0)"),
                    '|' => format!("fm_b({a} != 0.0 || {c} != 0.0)"),
                    _ => format!("fm_b(abs({a} - {c}) >= 1e-6)"),
                }
            }
            Expr::Cond(c, a, b) => format!("select({}, {}, {} != 0.0)", self.ex(b)?, self.ex(a)?, self.ex(c)?),
            Expr::Let(binds, body) => {
                let mark = self.scope.len();
                for (n, x) in binds {
                    let v = self.ex(x)?;
                    let id = format!("v_{}_{}", n, self.next);
                    self.next += 1;
                    self.body.push_str(&format!("    let {id}: f32 = {v};\n"));
                    self.scope.push((n.as_str(), id));
                }
                let out = self.ex(body)?;
                self.scope.truncate(mark);
                out
            }
            Expr::Call(f, args) => {
                let a = args.iter().map(|x| self.ex(x)).collect::<Result<Vec<_>, _>>()?;
                call(f, &a)?
            }
        })
    }
}

fn call(f: &str, a: &[String]) -> Result<String, String> {
    let arg = |k: usize, d: &str| a.get(k).cloned().unwrap_or_else(|| d.to_owned());
    let v3 = |k: usize| format!("vec3<f32>({}, {}, {})", a[k], a[k + 1], a[k + 2]);
    Ok(match f {
        "sin" | "cos" | "tan" | "atan" | "abs" | "sqrt" | "exp" | "floor" | "ceil" | "fract" | "sign" | "degrees" | "radians" | "sinh" | "cosh" | "tanh" => format!("{f}({})", a[0]),
        "asin" | "acos" => format!("{f}(clamp({}, -1.0, 1.0))", a[0]),
        "atan2" => format!("atan2({}, {})", a[0], a[1]),
        "ln" => format!("log({})", a[0]),
        "log10" => format!("(log({}) * 0.4342944819)", a[0]),
        "cbrt" => format!("fm_cbrt({})", a[0]),
        "round" => format!("fm_round({})", a[0]),
        "noise" => format!("fm_noise1({}, fm_seed({}))", a[0], arg(1, "0.0")),
        "random" => match a.len() {
            1 => format!("fm_rand01({})", a[0]),
            2 => format!("(fm_rand01({}) * {})", a[0], a[1]),
            _ => format!("({} + fm_rand01({}) * ({} - {}))", a[1], a[0], a[2], a[1]),
        },
        "min" | "max" => a[1..].iter().fold(a[0].clone(), |acc, x| format!("{f}({acc}, {x})")),
        "length" => format!("sqrt({})", a.iter().map(|x| format!("fm_sq({x})")).collect::<Vec<_>>().join(" + ")),
        "pow" => format!("fm_pow({}, {})", a[0], a[1]),
        "mod" => format!("fm_mod({}, {})", a[0], a[1]),
        "hypot" => format!("sqrt(fm_sq({}) + fm_sq({}))", a[0], a[1]),
        "step" => format!("fm_b({} >= {})", a[1], a[0]),
        "posterize" => format!("fm_posterize({}, {})", a[0], a[1]),
        "clamp" => format!("min(max({}, {}), {})", a[0], a[1], a[2]),
        "lerp" | "mix" => format!("({0} + ({1} - {0}) * {2})", a[0], a[1], a[2]),
        "smoothstep" => format!("fm_smoothstep({}, {}, {})", a[0], a[1], a[2]),
        "smin" | "smax" => format!("fm_{f}({}, {}, {})", a[0], a[1], a[2]),
        "linear" | "ease" | "ease_in" | "ease_out" => {
            let kind = ["linear", "ease", "ease_in", "ease_out"].iter().position(|k| *k == f).unwrap_or(0);
            match a.len() {
                5 => format!("fm_map({kind}, {}, {}, {}, {}, {})", a[0], a[1], a[2], a[3], a[4]),
                4 => format!("fm_map({kind}, {}, 0.0, 1.0, {}, {})", a[0], a[2], a[3]),
                _ => format!("fm_map({kind}, {}, 0.0, 1.0, {}, {})", a[0], a[1], a[2]),
            }
        }
        "if" => format!("select({}, {}, {} != 0.0)", a[2], a[1], a[0]),
        "noise2" => format!("(fm_noise3(vec3<f32>({}, {}, 0.5), fm_seed({})) * 1.4)", a[0], a[1], arg(2, "0.0")),
        "noise3" => format!("fm_noise3({}, fm_seed({}))", v3(0), arg(3, "0.0")),
        "fbm" => format!("fm_fbm({}, {}, fm_seed({}))", v3(0), arg(3, "4.0"), arg(4, "0.0")),
        "voronoi" | "voronoi_edge" => {
            let flat = a.len() < 3;
            let p = if flat { format!("vec3<f32>({}, {}, 0.0)", a[0], a[1]) } else { v3(0) };
            let v = format!("fm_voronoi({p}, {flat})");
            if f == "voronoi" { format!("{v}.x") } else { format!("({v}.y - {v}.x)") }
        }
        "sphere" => format!("fm_sphere({}, {})", v3(0), a[3]),
        "box" => format!("fm_box({}, vec3<f32>({}, {}, {}))", v3(0), a[3], a[4], a[5]),
        "torus" => format!("fm_torus({}, {}, {})", v3(0), a[3], a[4]),
        "cylinder" => format!("fm_cylinder({}, {}, {})", v3(0), a[3], a[4]),
        "gyroid" | "schwarz_p" | "schwarz_d" | "neovius" | "lidinoid" => format!("fm_{f}({})", v3(0)),
        "mandelbulb" => format!("fm_mandelbulb({}, {}, {})", v3(0), arg(3, "8.0"), arg(4, "10.0")),
        "mandelbox" => format!("fm_mandelbox({}, {}, {})", v3(0), arg(3, "2.0"), arg(4, "12.0")),
        "menger" => format!("fm_menger({}, {})", v3(0), arg(3, "4.0")),
        "sierpinski" => format!("fm_sierpinski({}, {})", v3(0), arg(3, "8.0")),
        "superformula" => format!("fm_superformula({}, {}, {}, {}, {}, {}, {})", a[0], a[1], a[2], a[3], a[4], arg(5, "1.0"), arg(6, "1.0")),
        other => return Err(format!("{other} can't be drawn on the GPU")),
    })
}

impl Program {
    /// The formula as a WGSL function `fn {name}(n_<name>: f32, …) -> f32`
    /// over its names in order (needs [`PRELUDE`]).
    pub fn to_wgsl(&self, name: &str) -> Result<String, String> {
        let names = self.names();
        let ids: Vec<String> = names.iter().map(|n| format!("n_{n}")).collect();
        let mut g = Gen { body: String::new(), scope: names.iter().map(String::as_str).zip(ids.iter().cloned()).collect(), next: 0 };
        let out = g.ex(self.expr())?;
        let params: Vec<String> = ids.iter().map(|i| format!("{i}: f32")).collect();
        Ok(format!("fn {name}({}) -> f32 {{\n{}    return {out};\n}}\n", params.join(", "), g.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_become_wgsl_functions() {
        let p = Program::compile("k = 3 * pi; g = gyroid(k * x, k * y, k * z); t > 1 ? abs(g) - 0.3 : max(g, -sphere(x, y, z, 0.5))", &["x", "y", "z", "t", "r"]).unwrap();
        let w = p.to_wgsl("field").unwrap();
        assert!(w.starts_with("fn field(n_x: f32, n_y: f32, n_z: f32, n_t: f32, n_r: f32) -> f32"), "{w}");
        assert!(w.contains("let v_k_0: f32 = (3.0 * 3.1415927);") && w.contains("fm_gyroid(") && w.contains("select("), "{w}");
        assert_eq!(lit(-2.0), "(-2.0)");
        assert_eq!(lit(1e-7), "1e-7");
        assert!(Program::compile("[x, y, z]", &["x", "y", "z", "i"]).unwrap().to_wgsl("f").unwrap().contains("n_i < 0.5"));
    }
}
