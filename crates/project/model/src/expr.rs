// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's expression language: the formulas of expression modifiers and
//! drivers. Safe by construction: no loops, no I/O, no memory but a few
//! numbers, a bounded program size and nesting depth, so a formula always
//! finishes in a few microseconds and can never touch anything outside
//! the frame it works out.
//!
//! A formula is worked out once per component of a property (x, then y),
//! with `i` saying which. It is a list of statements separated by `;` or
//! new lines; every statement but the last is a binding `name = …`
//! (`let`, `var` and `const` in front are allowed and ignored, so After
//! Effects habits work); the last is the result:
//!
//! ```text
//! amp = 30; freq = 2
//! t < 1 ? value : wiggle(freq, amp)
//! ```
//!
//! * Numbers, `+ - * / % ^` (also `**`), comparisons `< > <= >= == !=`,
//!   `&& || !`, `c ? a : b`, brackets, `// comments`.
//! * Names: `value` (the property as keyed), `value[0]` / `value[1]`
//!   (one component of it), `t` / `time` (seconds), `i` (the component),
//!   `num_keys`, `pi`, `tau`, `e`, the bindings, and a driver's variables.
//! * `[a, b]`: the element for the component being worked out (so
//!   `[value[0], value[1] + 20]` moves only y).
//! * Maths: sin cos tan asin acos atan atan2 abs sqrt exp ln log10 floor
//!   ceil round fract sign degrees radians min max (2–8 numbers) pow mod
//!   hypot clamp lerp step smoothstep if(c, a, b) posterize(x, steps).
//! * Randomness: `noise(x)` / `noise(x, seed)` (smooth, -1..1),
//!   `random(x)` (0..1), `random(x, max)`, `random(x, min, max)`: the same
//!   for the same x, so frames repeat.
//! * Mapping: `linear(x, a, b, v1, v2)` (x from a..b to v1..v2, held at
//!   the ends), `ease`, `ease_in`, `ease_out` the same with a curve, and
//!   their 3-argument forms over 0..1.
//! * Animation: `wiggle(per_second, amount)` (+ `octaves`, `seed`): the
//!   value plus a smooth wobble; `value_at(time)`, `velocity()`,
//!   `key_time(n)`, `key_value(n)` (keys from 1); `loop_out(kind, n)` and
//!   `loop_in(kind, n)` with kind `"cycle"`, `"pingpong"`, `"offset"` or
//!   `"continue"` (n: how many keys from the end / start loop, 0 all);
//!   `inertia(amount, per_second, decay)`: an overshoot that rings out
//!   after each key.
//! * Links: `prop(clip, "path")` reads another clip's property (`clip` a
//!   name in quotes or an id number), `prop(clip, "path", component)`,
//!   `prop(clip, "path", component, time)`. Drivers resolve them; in a
//!   plain modifier they read as 0.
//!
//! Aliases for common expression spellings: `loopOut`, `loopIn`,
//! `valueAtTime`, `easeIn`, `easeOut`, `Math.sin` and friends.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A parsed formula.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A number.
    Num(f64),
    /// A string (only as some functions' arguments).
    Str(String),
    /// A name looked up when evaluated.
    Var(String),
    /// `name[k]`: one component of a named value.
    Index(String, Box<Expr>),
    /// `[a, b, …]`: the element for the component being worked out.
    List(Vec<Expr>),
    /// `-x`.
    Neg(Box<Expr>),
    /// `!x`.
    Not(Box<Expr>),
    /// `a op b` with op one of `+ - * / % ^ < > l(<=) g(>=) =(==) !(!=)
    /// &(&&) |(||)`.
    Bin(char, Box<Expr>, Box<Expr>),
    /// `c ? a : b`.
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `name(args)`.
    Call(String, Vec<Expr>),
    /// `name = expr; …; result`.
    Let(Vec<(String, Expr)>, Box<Expr>),
}

/// A clip named in `prop(…)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClipRef<'a> {
    /// By id.
    Id(u64),
    /// By name (any case).
    Name(&'a str),
}

/// What a formula can see. Only `var` is required; the rest have
/// "not available" defaults.
pub trait Context {
    /// A name: `value`, `t`, `i`, a driver's variables…
    fn var(&self, name: &str) -> Option<f64>;
    /// Component `k` of a named value (`value[1]`).
    fn component(&self, name: &str, k: usize) -> Option<f64> {
        let _ = (name, k);
        None
    }
    /// The property's own keyed value at time `t` (the same clock as `t`),
    /// in the component being worked out.
    fn value_at(&self, t: f64) -> Option<f64> {
        let _ = t;
        None
    }
    /// How many keys the property has.
    fn num_keys(&self) -> usize {
        0
    }
    /// Key `n` (from 1): its time (the clock of `t`) and value.
    fn key(&self, n: usize) -> Option<(f64, f64)> {
        let _ = n;
        None
    }
    /// Another clip's property, one component (`None`: the one being
    /// worked out), at a time (`None`: now).
    fn prop(&self, clip: ClipRef, path: &str, component: Option<usize>, t: Option<f64>) -> Option<f64> {
        let _ = (clip, path, component, t);
        None
    }
}

/// A plain name lookup as a [`Context`].
pub struct Vars<'a>(pub &'a dyn Fn(&str) -> Option<f64>);

impl Context for Vars<'_> {
    fn var(&self, name: &str) -> Option<f64> {
        (self.0)(name)
    }
}

/// The most arguments any function takes.
const MAX_ARGS: usize = 8;

/// Function names, their argument counts (least, most), and whether some
/// arguments are words in quotes.
const FUNCS: &[(&str, usize, usize)] = &[
    ("sin", 1, 1),
    ("cos", 1, 1),
    ("tan", 1, 1),
    ("asin", 1, 1),
    ("acos", 1, 1),
    ("atan", 1, 1),
    ("atan2", 2, 2),
    ("abs", 1, 1),
    ("sqrt", 1, 1),
    ("exp", 1, 1),
    ("ln", 1, 1),
    ("log10", 1, 1),
    ("floor", 1, 1),
    ("ceil", 1, 1),
    ("round", 1, 1),
    ("fract", 1, 1),
    ("sign", 1, 1),
    ("degrees", 1, 1),
    ("radians", 1, 1),
    ("noise", 1, 2),
    ("random", 1, 3),
    ("min", 2, 8),
    ("max", 2, 8),
    ("pow", 2, 2),
    ("mod", 2, 2),
    ("hypot", 2, 2),
    ("step", 2, 2),
    ("posterize", 2, 2),
    ("clamp", 3, 3),
    ("lerp", 3, 3),
    ("smoothstep", 3, 3),
    ("if", 3, 3),
    ("linear", 3, 5),
    ("ease", 3, 5),
    ("ease_in", 3, 5),
    ("ease_out", 3, 5),
    ("wiggle", 2, 4),
    ("value_at", 1, 1),
    ("velocity", 0, 0),
    ("key_time", 1, 1),
    ("key_value", 1, 1),
    ("loop_out", 0, 2),
    ("loop_in", 0, 2),
    ("inertia", 0, 3),
    ("prop", 2, 4),
    // Shape maths (see `formula`).
    ("sinh", 1, 1),
    ("cosh", 1, 1),
    ("tanh", 1, 1),
    ("cbrt", 1, 1),
    ("length", 1, 8),
    ("mix", 3, 3),
    ("smin", 3, 3),
    ("smax", 3, 3),
    ("noise2", 2, 3),
    ("noise3", 3, 4),
    ("fbm", 3, 5),
    ("voronoi", 2, 3),
    ("voronoi_edge", 2, 3),
    ("sphere", 4, 4),
    ("box", 6, 6),
    ("torus", 5, 5),
    ("cylinder", 5, 5),
    ("gyroid", 3, 3),
    ("schwarz_p", 3, 3),
    ("schwarz_d", 3, 3),
    ("neovius", 3, 3),
    ("lidinoid", 3, 3),
    ("mandelbulb", 3, 5),
    ("mandelbox", 3, 5),
    ("menger", 3, 4),
    ("sierpinski", 3, 4),
    ("superformula", 5, 7),
];

/// Common (JavaScript-style) spellings, lower-cased.
fn alias(n: &str) -> &str {
    let n = n.strip_prefix("math.").unwrap_or(n);
    match n {
        "loopout" => "loop_out",
        "loopin" => "loop_in",
        "valueattime" => "value_at",
        "easein" => "ease_in",
        "easeout" => "ease_out",
        "key_count" | "numkeys" => "num_keys",
        "radianstodegrees" => "degrees",
        "degreestoradians" => "radians",
        "log" => "ln",
        "time" => "t",
        "v" => "value",
        _ => n,
    }
}

/// Names every formula has.
const BUILTIN: &[&str] = &["value", "t", "i", "num_keys"];

/// Words a binding or a variable may not be called.
pub const KEYWORDS: &[&str] = &["value", "v", "t", "time", "i", "num_keys", "pi", "tau", "e", "let", "var", "const", "true", "false"];

/// Parses `src`, or says what is wrong with it.
pub fn parse(src: &str) -> Result<Expr, String> {
    parse_with(src, &[])
}

/// Parses `src` where `names` (lower case) are variables too (a driver's
/// variables).
pub fn parse_with(src: &str, names: &[&str]) -> Result<Expr, String> {
    if src.len() > 4000 {
        return Err("formula is too long".into());
    }
    let toks = lex(src)?;
    let mut p = Parser { toks, at: 0, depth: 0, names: names.iter().map(|n| n.to_ascii_lowercase()).collect(), bound: Vec::new() };
    let e = p.program()?;
    if p.at < p.toks.len() {
        return Err(format!("unexpected {}", p.toks[p.at]));
    }
    Ok(e)
}

/// How many parsed formulas [`parse_cached`] keeps (per thread).
const CACHE_SIZE: usize = 512;

/// A remembered parse, with when it was last asked for.
struct Cached {
    src: String,
    names: Vec<String>,
    parsed: Rc<Result<Expr, String>>,
    used: u64,
}

#[derive(Default)]
struct Cache {
    /// By a hash of the source and names; a hash shared by two formulas
    /// holds both.
    by_hash: HashMap<u64, Vec<Cached>>,
    len: usize,
    clock: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

fn cache_hash(src: &str, names: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    names.hash(&mut h);
    h.finish()
}

/// [`parse_with`], remembered: frames evaluate the same formulas over and
/// over. A look that finds its formula copies nothing; when the cache is
/// full the formula asked for longest ago goes (a project with more
/// formulas than the cache holds keeps its busiest ones instead of losing
/// them all at once).
pub fn parse_cached(src: &str, names: &[&str]) -> Rc<Result<Expr, String>> {
    let h = cache_hash(src, names);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.clock += 1;
        let now = c.clock;
        if let Some(hit) = c.by_hash.get_mut(&h).and_then(|v| v.iter_mut().find(|e| e.src == src && e.names.iter().map(String::as_str).eq(names.iter().copied()))) {
            hit.used = now;
            return hit.parsed.clone();
        }
        let parsed = Rc::new(parse_with(src, names));
        if c.len >= CACHE_SIZE {
            let oldest = c.by_hash.iter().flat_map(|(k, v)| v.iter().enumerate().map(move |(i, e)| (e.used, *k, i))).min();
            if let Some((_, k, i)) = oldest {
                let v = c.by_hash.get_mut(&k).expect("found above");
                v.swap_remove(i);
                if v.is_empty() {
                    c.by_hash.remove(&k);
                }
                c.len -= 1;
            }
        }
        c.by_hash.entry(h).or_default().push(Cached { src: src.to_owned(), names: names.iter().map(|n| (*n).to_owned()).collect(), parsed: parsed.clone(), used: now });
        c.len += 1;
        parsed
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Str(String),
    Op(&'static str),
    /// `;` or a line break between statements.
    Sep,
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Num(v) => write!(f, "{v}"),
            Tok::Name(n) => write!(f, "{n}"),
            Tok::Str(s) => write!(f, "\"{s}\""),
            Tok::Op(o) => write!(f, "'{o}'"),
            Tok::Sep => write!(f, "end of statement"),
        }
    }
}

const OPS2: &[&str] = &["<=", ">=", "==", "!=", "**", "&&", "||"];

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    // A line break ends a statement unless the line plainly goes on.
    let continues = |out: &[Tok], rest: &str| -> bool {
        let last_open = match out.last() {
            None | Some(Tok::Sep) => true,
            Some(Tok::Op(o)) => !matches!(*o, ")" | "]"),
            _ => false,
        };
        let next = rest.trim_start();
        let next_op = next.starts_with("==") || next.starts_with(|c: char| "+-*/%^?:)],|&<>".contains(c));
        last_open || next_op || next.is_empty()
    };
    while i < b.len() {
        let c = b[i] as char;
        if c == '\n' || c == ';' {
            let rest = &src[i + 1..];
            if c == ';' || !continues(&out, rest) {
                if !matches!(out.last(), None | Some(Tok::Sep)) {
                    out.push(Tok::Sep);
                }
            }
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == '/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c.is_ascii_digit() || (c == '.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') && i + 1 < b.len() && ((b[i + 1] as char).is_ascii_digit() || b[i + 1] == b'-' || b[i + 1] == b'+') {
                i += 2;
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                }
            }
            out.push(Tok::Num(src[s..i].parse().map_err(|_| format!("bad number {}", &src[s..i]))?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let s = i;
            // `Math.sin`, `thisComp`…: a dotted name is one word.
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || (b[i] == b'.' && b.get(i + 1).is_some_and(|d| (*d as char).is_ascii_alphabetic()))) {
                i += 1;
            }
            out.push(Tok::Name(src[s..i].to_ascii_lowercase()));
        } else if c == '"' || c == '\'' {
            let s = i + 1;
            i += 1;
            while i < b.len() && b[i] as char != c {
                i += 1;
            }
            if i >= b.len() {
                return Err("a string has no closing quote".into());
            }
            out.push(Tok::Str(src[s..i].to_owned()));
            i += 1;
        } else {
            let two = src.get(i..i + 2).unwrap_or("");
            if let Some(o) = OPS2.iter().find(|o| **o == two) {
                out.push(Tok::Op(if *o == "**" { "^" } else { o }));
                i += 2;
                continue;
            }
            let one: &'static str = match c {
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '%' => "%",
                '^' => "^",
                '(' => "(",
                ')' => ")",
                '[' => "[",
                ']' => "]",
                ',' => ",",
                '<' => "<",
                '>' => ">",
                '!' => "!",
                '?' => "?",
                ':' => ":",
                '=' => "=",
                _ => return Err(format!("unexpected character '{c}'")),
            };
            out.push(Tok::Op(one));
            i += 1;
        }
        if out.len() > 2000 {
            return Err("formula is too long".into());
        }
    }
    while out.last() == Some(&Tok::Sep) {
        out.pop();
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
    depth: u32,
    names: Vec<String>,
    bound: Vec<String>,
}

impl Parser {
    fn peek_op(&self) -> Option<&'static str> {
        match self.toks.get(self.at) {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }
    fn eat(&mut self, op: &str) -> bool {
        if self.peek_op() == Some(op) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn known(&self, n: &str) -> bool {
        BUILTIN.contains(&n) || self.names.iter().any(|x| x == n) || self.bound.iter().any(|x| x == n)
    }
    fn program(&mut self) -> Result<Expr, String> {
        let mut binds: Vec<(String, Expr)> = Vec::new();
        loop {
            while self.toks.get(self.at) == Some(&Tok::Sep) {
                self.at += 1;
            }
            // `let a = …`, `a = …`.
            let mut k = self.at;
            if let Some(Tok::Name(w)) = self.toks.get(k)
                && matches!(w.as_str(), "let" | "var" | "const")
            {
                k += 1;
            }
            let binding = match (self.toks.get(k), self.toks.get(k + 1)) {
                (Some(Tok::Name(n)), Some(Tok::Op("="))) => Some(n.clone()),
                _ => None,
            };
            match binding {
                Some(n) => {
                    let n = alias(&n).to_owned();
                    if KEYWORDS.contains(&n.as_str()) || FUNCS.iter().any(|f| f.0 == n) {
                        return Err(format!("\"{n}\" is a word the language keeps; pick another name"));
                    }
                    if !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        return Err(format!("\"{n}\" can't be a name"));
                    }
                    self.at = k + 2;
                    let e = self.ternary()?;
                    if binds.len() >= 64 {
                        return Err("too many statements".into());
                    }
                    binds.push((n.clone(), e));
                    self.bound.push(n.clone());
                    match self.toks.get(self.at) {
                        None => return Ok(Expr::Let(binds, Box::new(Expr::Var(n)))),
                        Some(Tok::Sep) => continue,
                        Some(t) => return Err(format!("unexpected {t} after a binding")),
                    }
                }
                None => {
                    let e = self.ternary()?;
                    if self.toks.get(self.at) == Some(&Tok::Sep) {
                        // Statements after the result: only bindings may
                        // come before it.
                        let rest_empty = self.toks[self.at..].iter().all(|t| *t == Tok::Sep);
                        if !rest_empty {
                            return Err("only bindings (name = …) may come before the result".into());
                        }
                        self.at = self.toks.len();
                    }
                    return Ok(if binds.is_empty() { e } else { Expr::Let(binds, Box::new(e)) });
                }
            }
        }
    }
    fn ternary(&mut self) -> Result<Expr, String> {
        let c = self.or()?;
        if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err("? needs a : (c ? a : b)".into());
            }
            let b = self.ternary()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.eat("||") {
            let r = self.and()?;
            l = Expr::Bin('|', Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.compare()?;
        while self.eat("&&") {
            let r = self.compare()?;
            l = Expr::Bin('&', Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn compare(&mut self) -> Result<Expr, String> {
        let mut l = self.sum()?;
        while let Some(o) = self.peek_op().filter(|o| ["<", ">", "<=", ">=", "==", "!="].contains(o)) {
            self.at += 1;
            let r = self.sum()?;
            let c = match o {
                "<" => '<',
                ">" => '>',
                "<=" => 'l',
                ">=" => 'g',
                "==" => '=',
                _ => '!',
            };
            l = Expr::Bin(c, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn sum(&mut self) -> Result<Expr, String> {
        let mut l = self.product()?;
        while let Some(o) = self.peek_op().filter(|o| *o == "+" || *o == "-") {
            self.at += 1;
            let r = self.product()?;
            l = Expr::Bin(o.chars().next().unwrap_or('+'), Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn product(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        while let Some(o) = self.peek_op().filter(|o| ["*", "/", "%"].contains(o)) {
            self.at += 1;
            let r = self.unary()?;
            l = Expr::Bin(o.chars().next().unwrap_or('*'), Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("formula nests too deeply".into());
        }
        let out = if self.eat("-") {
            Expr::Neg(Box::new(self.unary()?))
        } else if self.eat("+") {
            self.unary()?
        } else if self.eat("!") {
            Expr::Not(Box::new(self.unary()?))
        } else {
            self.power()?
        };
        self.depth -= 1;
        Ok(out)
    }
    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        if self.eat("^") {
            // Right-associative, and binds tighter than a leading minus
            // on the exponent's own side: 2^-1 works.
            let exp = self.unary()?;
            return Ok(Expr::Bin('^', Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }
    fn args(&mut self, n: &str) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                args.push(self.ternary()?);
                if self.eat(")") {
                    break;
                }
                if !self.eat(",") {
                    return Err(format!("{n}( needs , or )"));
                }
            }
        }
        Ok(args)
    }
    fn atom(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("formula nests too deeply".into());
        }
        let out = match self.toks.get(self.at).cloned() {
            Some(Tok::Num(v)) => {
                self.at += 1;
                Expr::Num(v)
            }
            Some(Tok::Str(s)) => {
                self.at += 1;
                Expr::Str(s)
            }
            Some(Tok::Name(raw)) => {
                self.at += 1;
                // A name given (`v` of a surface) is itself, not an alias.
                let n = if self.names.contains(&raw) { raw } else { alias(&raw).to_owned() };
                if self.eat("(") {
                    let args = self.args(&n)?;
                    let Some(&(_, lo, hi)) = FUNCS.iter().find(|f| f.0 == n) else {
                        return Err(format!("unknown function {n}"));
                    };
                    if args.len() < lo || args.len() > hi {
                        return Err(if lo == hi { format!("{n} takes {lo} argument(s)") } else { format!("{n} takes {lo} to {hi} arguments") });
                    }
                    check_words(&n, &args)?;
                    Expr::Call(n, args)
                } else if self.eat("[") {
                    let k = self.ternary()?;
                    if !self.eat("]") {
                        return Err("missing ]".into());
                    }
                    if !self.known(&n) {
                        return Err(self.unknown(&n));
                    }
                    Expr::Index(n, Box::new(k))
                } else {
                    match n.as_str() {
                        "pi" => Expr::Num(std::f64::consts::PI),
                        "tau" => Expr::Num(std::f64::consts::TAU),
                        "e" => Expr::Num(std::f64::consts::E),
                        "true" => Expr::Num(1.0),
                        "false" => Expr::Num(0.0),
                        _ if self.known(&n) => Expr::Var(n),
                        _ => return Err(self.unknown(&n)),
                    }
                }
            }
            Some(Tok::Op("(")) => {
                self.at += 1;
                let e = self.ternary()?;
                if !self.eat(")") {
                    return Err("missing )".into());
                }
                e
            }
            Some(Tok::Op("[")) => {
                self.at += 1;
                let mut items = Vec::new();
                if !self.eat("]") {
                    loop {
                        items.push(self.ternary()?);
                        if self.eat("]") {
                            break;
                        }
                        if !self.eat(",") {
                            return Err("[ needs , or ]".into());
                        }
                    }
                }
                if items.is_empty() {
                    return Err("[] is empty".into());
                }
                Expr::List(items)
            }
            Some(t) => return Err(format!("unexpected {t}")),
            None => return Err("formula ends too soon".into()),
        };
        self.depth -= 1;
        Ok(out)
    }
    fn unknown(&self, n: &str) -> String {
        let mut known: Vec<&str> = self.names.iter().map(String::as_str).chain(self.bound.iter().map(String::as_str)).collect();
        known.extend(["value", "t", "i", "pi", "tau"]);
        format!("unknown name {n} (names are {})", known.join(", "))
    }
}

/// Words in quotes only where a function takes them.
fn check_words(n: &str, args: &[Expr]) -> Result<(), String> {
    for (k, a) in args.iter().enumerate() {
        let word_ok = match n {
            "loop_out" | "loop_in" => k == 0,
            "prop" => k <= 1,
            _ => false,
        };
        if let Expr::Str(s) = a {
            if !word_ok {
                return Err(format!("{n} takes numbers, not \"{s}\""));
            }
            if matches!(n, "loop_out" | "loop_in") && !matches!(s.to_ascii_lowercase().as_str(), "cycle" | "pingpong" | "offset" | "continue") {
                return Err(format!("{n}: \"{s}\" is not a loop kind (cycle, pingpong, offset, continue)"));
            }
        } else if n == "prop" && k == 1 {
            return Err("prop's second argument is a property path in quotes, e.g. \"transform.rotation\"".into());
        }
    }
    Ok(())
}

struct Eval<'a> {
    cx: &'a dyn Context,
    scope: Vec<(&'a str, f64)>,
}

/// How far apart `velocity` and the loops sample (seconds).
const H: f64 = 1e-3;

impl<'a> Eval<'a> {
    fn var(&self, n: &str) -> f64 {
        if let Some((_, v)) = self.scope.iter().rev().find(|(k, _)| *k == n) {
            return *v;
        }
        match n {
            "num_keys" => self.cx.num_keys() as f64,
            _ => self.cx.var(n).unwrap_or(0.0),
        }
    }
    fn value(&self) -> f64 {
        self.var("value")
    }
    fn at(&self, t: f64) -> f64 {
        self.cx.value_at(t).unwrap_or_else(|| self.value())
    }
    fn ev(&mut self, e: &'a Expr) -> f64 {
        let b = |x: bool| if x { 1.0 } else { 0.0 };
        match e {
            Expr::Num(v) => *v,
            Expr::Str(_) => f64::NAN,
            Expr::Var(n) => self.var(n),
            Expr::Index(n, k) => {
                let k = self.ev(k).round().max(0.0) as usize;
                if self.scope.iter().any(|(s, _)| *s == n) {
                    return self.var(n);
                }
                self.cx.component(n, k).unwrap_or_else(|| if k == 0 { self.var(n) } else { 0.0 })
            }
            Expr::List(items) => {
                let i = self.var("i").round().max(0.0) as usize;
                self.ev(&items[i.min(items.len() - 1)])
            }
            Expr::Neg(x) => -self.ev(x),
            Expr::Not(x) => b(self.ev(x) == 0.0),
            Expr::Cond(c, x, y) => {
                if self.ev(c) != 0.0 {
                    self.ev(x)
                } else {
                    self.ev(y)
                }
            }
            Expr::Let(binds, body) => {
                let mark = self.scope.len();
                for (n, x) in binds {
                    let v = self.ev(x);
                    self.scope.push((n.as_str(), v));
                }
                let out = self.ev(body);
                self.scope.truncate(mark);
                out
            }
            Expr::Bin(op, l, r) => {
                // && and || short-circuit.
                if *op == '&' {
                    return b(self.ev(l) != 0.0 && self.ev(r) != 0.0);
                }
                if *op == '|' {
                    return b(self.ev(l) != 0.0 || self.ev(r) != 0.0);
                }
                let (a, c) = (self.ev(l), self.ev(r));
                match op {
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
                    _ => b((a - c).abs() >= 1e-9),
                }
            }
            Expr::Call(f, args) => self.call(f, args),
        }
    }

    fn call(&mut self, f: &str, args: &'a [Expr]) -> f64 {
        match f {
            "if" => {
                return if self.ev(&args[0]) != 0.0 { self.ev(&args[1]) } else { self.ev(&args[2]) };
            }
            "loop_out" | "loop_in" => return self.looped(f == "loop_out", args),
            "prop" => {
                let clip = match &args[0] {
                    Expr::Str(s) => ClipRef::Name(s.as_str()),
                    other => ClipRef::Id(self.ev(other).round().max(0.0) as u64),
                };
                let Expr::Str(path) = &args[1] else { return 0.0 };
                let comp = args.get(2).map(|x| self.ev(x).round().max(0.0) as usize);
                let at = args.get(3).map(|x| self.ev(x));
                return self.cx.prop(clip, path, comp, at).unwrap_or(0.0);
            }
            _ => {}
        }
        // The parser allows at most `MAX_ARGS` arguments: worked out onto
        // the stack, not into a new vector each call.
        let mut buf = [0.0f64; MAX_ARGS];
        for (slot, e) in buf.iter_mut().zip(args) {
            *slot = self.ev(e);
        }
        let a = &buf[..args.len().min(MAX_ARGS)];
        if let Some(g) = super::formula::lookup(f) {
            return g(a);
        }
        match (f, a) {
            ("wiggle", xs) => {
                let (freq, amp) = (xs[0], xs[1]);
                let oct = xs.get(2).map_or(1, |o| o.round().clamp(1.0, 8.0) as u32);
                let seed = xs.get(3).map_or(0, |s| *s as u32);
                let i = self.var("i").round().max(0.0) as u32;
                let s = seed.wrapping_mul(7919).wrapping_add(i.wrapping_mul(104_729)).wrapping_add(31_337);
                self.value() + amp * super::fractal(self.var("t") * freq.max(0.0), oct, s)
            }
            ("value_at", [t]) => self.at(*t),
            ("velocity", []) => {
                let t = self.var("t");
                (self.at(t) - self.at(t - H)) / H
            }
            ("key_time", [n]) => self.cx.key(n.round().max(1.0) as usize).map_or(0.0, |k| k.0),
            ("key_value", [n]) => self.cx.key(n.round().max(1.0) as usize).map_or(self.value(), |k| k.1),
            ("inertia", xs) => {
                let amp = xs.first().copied().unwrap_or(0.05);
                let freq = xs.get(1).copied().unwrap_or(3.0).max(0.0);
                let decay = xs.get(2).copied().unwrap_or(6.0).max(0.0);
                let t = self.var("t");
                // The last key passed.
                let n = (1..=self.cx.num_keys()).rev().find(|&n| self.cx.key(n).is_some_and(|k| k.0 < t));
                let Some(n) = n else { return self.value() };
                let Some((tk, _)) = self.cx.key(n) else { return self.value() };
                let vel = (self.at(tk) - self.at(tk - H)) / H;
                let el = t - tk;
                self.value() + vel * amp * (freq * el * std::f64::consts::TAU).sin() / (decay * el).exp()
            }
            _ => 0.0,
        }
    }

    /// `loop_out` / `loop_in`.
    fn looped(&mut self, out: bool, args: &'a [Expr]) -> f64 {
        let kind = match args.first() {
            Some(Expr::Str(s)) => s.to_ascii_lowercase(),
            _ => "cycle".into(),
        };
        let n = args.get(1).map_or(0.0, |x| self.ev(x)).round().max(0.0) as usize;
        let k = self.cx.num_keys();
        let t = self.var("t");
        let v = self.value();
        if k < 2 {
            return v;
        }
        let (lo, hi) = if out { (if n == 0 || n >= k { 1 } else { k - n }, k) } else { (1, if n == 0 || n >= k { k } else { 1 + n }) };
        let (Some((t0, v0)), Some((t1, v1))) = (self.cx.key(lo), self.cx.key(hi)) else { return v };
        let span = t1 - t0;
        if span <= 0.0 || (out && t <= t1) || (!out && t >= t0) {
            return v;
        }
        let el = if out { t - t1 } else { t0 - t };
        let cyc = (el / span).floor();
        let local = el - cyc * span;
        let even = (cyc as i64) % 2 == 0;
        match (kind.as_str(), out) {
            ("pingpong", true) => {
                if even {
                    self.at(t1 - local)
                } else {
                    self.at(t0 + local)
                }
            }
            ("pingpong", false) => {
                if even {
                    self.at(t0 + local)
                } else {
                    self.at(t1 - local)
                }
            }
            ("offset", true) => self.at(t0 + local) + (cyc + 1.0) * (v1 - v0),
            ("offset", false) => self.at(t1 - local) - (cyc + 1.0) * (v1 - v0),
            ("continue", true) => {
                let vel = (v1 - self.at(t1 - H)) / H;
                v1 + vel * el
            }
            ("continue", false) => {
                let vel = (self.at(t0 + H) - v0) / H;
                v0 - vel * el
            }
            (_, true) => self.at(t0 + local),
            (_, false) => self.at(t1 - local),
        }
    }
}

impl Expr {
    /// The formula's value with names looked up by `var` (unknown names
    /// are 0).
    pub fn eval(&self, var: &dyn Fn(&str) -> Option<f64>) -> f64 {
        self.eval_in(&Vars(var))
    }

    /// The formula's value in a full [`Context`] (keys, other times,
    /// other clips).
    pub fn eval_in(&self, cx: &dyn Context) -> f64 {
        Eval { cx, scope: Vec::new() }.ev(self)
    }

    /// Whether the formula reads other clips (`prop`).
    pub fn links(&self) -> bool {
        match self {
            Expr::Call(f, a) => f == "prop" || a.iter().any(Expr::links),
            Expr::Index(_, k) | Expr::Neg(k) | Expr::Not(k) => k.links(),
            Expr::List(xs) => xs.iter().any(Expr::links),
            Expr::Bin(_, l, r) => l.links() || r.links(),
            Expr::Cond(c, a, b) => c.links() || a.links() || b.links(),
            Expr::Let(bs, body) => body.links() || bs.iter().any(|(_, e)| e.links()),
            Expr::Num(_) | Expr::Str(_) | Expr::Var(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A property keyed 0 → 100 over one second, then 100 → 50 at 2 s.
    struct Keys {
        t: f64,
        i: f64,
        keys: Vec<(f64, f64)>,
    }

    impl Keys {
        fn at(&self, t: f64) -> f64 {
            let k = &self.keys;
            if t <= k[0].0 {
                return k[0].1;
            }
            for w in k.windows(2) {
                if t <= w[1].0 {
                    let u = (t - w[0].0) / (w[1].0 - w[0].0);
                    return w[0].1 + (w[1].1 - w[0].1) * u;
                }
            }
            k[k.len() - 1].1
        }
    }

    impl Context for Keys {
        fn var(&self, name: &str) -> Option<f64> {
            match name {
                "value" => Some(self.at(self.t)),
                "t" => Some(self.t),
                "i" => Some(self.i),
                _ => None,
            }
        }
        fn component(&self, name: &str, k: usize) -> Option<f64> {
            (name == "value").then(|| self.at(self.t) + k as f64 * 1000.0)
        }
        fn value_at(&self, t: f64) -> Option<f64> {
            Some(self.at(t))
        }
        fn num_keys(&self) -> usize {
            self.keys.len()
        }
        fn key(&self, n: usize) -> Option<(f64, f64)> {
            self.keys.get(n.checked_sub(1)?).copied()
        }
        fn prop(&self, clip: ClipRef, path: &str, component: Option<usize>, t: Option<f64>) -> Option<f64> {
            match (clip, path) {
                (ClipRef::Name("Ball"), "transform.rotation") => Some(45.0 + t.unwrap_or(0.0) + component.unwrap_or(0) as f64),
                (ClipRef::Id(7), _) => Some(7.0),
                _ => None,
            }
        }
    }

    fn run(src: &str, t: f64) -> f64 {
        let cx = Keys { t, i: 0.0, keys: vec![(0.0, 0.0), (1.0, 100.0)] };
        parse(src).unwrap_or_else(|e| panic!("{src}: {e}")).eval_in(&cx)
    }

    #[test]
    fn statements_conditions_and_logic() {
        assert_eq!(run("a = 2; b = a * 3\nb + 1", 0.0), 7.0);
        assert_eq!(run("let amp = 4;\nconst f = 2;\namp * f", 0.0), 8.0);
        assert_eq!(run("t < 0.5 ? 1 : 2", 0.25), 1.0);
        assert_eq!(run("t < 0.5 ? 1 : t < 0.8 ? 2 : 3", 0.9), 3.0);
        assert_eq!(run("t > 0 && t < 1", 0.5), 1.0);
        assert_eq!(run("!(t > 0) || false", 0.5), 0.0);
        assert_eq!(run("if(t > 0.3, 10, 20)", 0.5), 10.0);
        // A line that goes on is one statement.
        assert_eq!(run("1 +\n 2\n * 3", 0.0), 7.0);
        assert_eq!(run("x = 5 // a comment\nx", 0.0), 5.0);
        assert_eq!(run("Math.max(1, 5, 3)", 0.0), 5.0);
        // Bindings end with the last one's value.
        assert_eq!(run("a = 3", 0.0), 3.0);
        assert!(parse("1; 2").is_err());
        assert!(parse("value = 3; value").unwrap_err().contains("keeps"));
        assert!(parse("t ? 1").is_err());
        assert!(parse("sin(\"x\")").is_err());
        assert!(parse("loop_out(\"spin\")").unwrap_err().contains("loop kind"));
        assert!(parse("q + 1").unwrap_err().contains("unknown name q"));
    }

    #[test]
    fn lists_pick_the_component_and_index_reads_one() {
        let cx = |i: f64| Keys { t: 0.5, i, keys: vec![(0.0, 0.0), (1.0, 100.0)] };
        let e = parse("[value[0] + 1, value[1] + 2]").unwrap();
        assert_eq!(e.eval_in(&cx(0.0)), 51.0);
        assert_eq!(e.eval_in(&cx(1.0)), 1052.0);
    }

    #[test]
    fn mapping_functions_hold_at_the_ends() {
        assert_eq!(run("linear(t, 0, 2, 10, 20)", 1.0), 15.0);
        assert_eq!(run("linear(t, 0, 2, 10, 20)", 5.0), 20.0);
        assert_eq!(run("linear(t, 0, 2, 10, 20)", -1.0), 10.0);
        assert_eq!(run("ease(t, 0, 1, 0, 10)", 0.5), 5.0);
        assert!(run("ease_in(t, 0, 1, 0, 10)", 0.5) < 5.0);
        assert!(run("ease_out(t, 0, 1, 0, 10)", 0.5) > 5.0);
        assert_eq!(run("linear(0.25, 0, 8)", 0.0), 2.0);
        assert_eq!(run("posterize(t, 4)", 0.6), 0.5);
        assert_eq!(run("random(3, 10, 20)", 0.0), run("random(3, 10, 20)", 9.0));
        let r = run("random(3, 10, 20)", 0.0);
        assert!((10.0..=20.0).contains(&r));
    }

    #[test]
    fn wiggle_is_smooth_bounded_and_centred_on_the_value() {
        let mut prev = run("wiggle(2, 10)", 0.0);
        let mut most: f64 = 0.0;
        for k in 1..400 {
            let t = k as f64 / 100.0;
            let v = run("wiggle(2, 10) - value", t);
            most = most.max(v.abs());
            assert!(v.abs() <= 10.0 + 1e-9);
            let w = run("wiggle(2, 10)", t);
            assert!((w - prev).abs() < 5.0, "jump at {t}");
            prev = w;
        }
        assert!(most > 2.0, "it moves: {most}");
        // Different seeds wobble differently; components too.
        assert_ne!(run("wiggle(2, 10, 1, 1)", 0.37), run("wiggle(2, 10, 1, 2)", 0.37));
    }

    #[test]
    fn loops_repeat_the_keys() {
        // Keys 0 → 100 over a second.
        assert_eq!(run("loop_out()", 0.5), 50.0);
        assert_eq!(run("loop_out(\"cycle\")", 1.25), 25.0);
        assert_eq!(run("loopOut('cycle')", 2.75), 75.0);
        assert!((run("loop_out(\"pingpong\")", 1.25) - 75.0).abs() < 1e-9);
        assert!((run("loop_out(\"pingpong\")", 2.25) - 25.0).abs() < 1e-9);
        assert!((run("loop_out(\"offset\")", 1.25) - 125.0).abs() < 1e-9);
        assert!((run("loop_out(\"offset\")", 2.5) - 250.0).abs() < 1e-9);
        assert!((run("loop_out(\"continue\")", 2.0) - 200.0).abs() < 1e-6);
        assert!((run("loop_in(\"cycle\")", -0.25) - 75.0).abs() < 1e-9);
        assert!((run("loop_in(\"offset\")", -0.25) + 25.0).abs() < 1e-9);
        assert_eq!(run("value_at(0.3)", 0.9), 30.0);
        assert!((run("velocity()", 0.5) - 100.0).abs() < 1e-6);
        assert_eq!(run("key_time(2)", 0.0), 1.0);
        assert_eq!(run("key_value(2) + num_keys", 0.0), 102.0);
        // Rings after the last key, settles, and is still at the key.
        assert_eq!(run("inertia(0.05, 2, 4)", 1.0), 100.0);
        assert!((run("inertia(0.05, 2, 4)", 1.1) - 100.0).abs() > 0.5);
        assert!((run("inertia(0.05, 2, 4)", 4.0) - 100.0).abs() < 0.05);
    }

    #[test]
    fn links_read_other_clips() {
        assert_eq!(run("prop(\"Ball\", \"transform.rotation\")", 0.0), 45.0);
        assert_eq!(run("prop(\"Ball\", \"transform.rotation\", 1, 2)", 0.0), 48.0);
        assert_eq!(run("prop(7, \"x\") * 2", 0.0), 14.0);
        assert!(parse("prop(\"a\", \"b\")").unwrap().links());
        assert!(!parse("t * 2").unwrap().links());
        assert!(parse("prop(\"a\", 3)").is_err());
        // A plain lookup has no other clips: 0.
        assert_eq!(parse("prop(\"Ball\", \"x\") + 1").unwrap().eval(&|_| None), 1.0);
    }

    #[test]
    fn cache_returns_the_same_parse() {
        let a = parse_cached("value * 2", &[]);
        let b = parse_cached("value * 2", &[]);
        assert!(Rc::ptr_eq(&a, &b));
        assert!(parse_cached("nope(", &[]).is_err());
        // The names are part of what is remembered.
        let c = parse_cached("x + 1", &["x"]);
        assert!(c.is_ok() && parse_cached("x + 1", &[]).is_err());
        assert!(Rc::ptr_eq(&c, &parse_cached("x + 1", &["x"])));
        assert!(FUNCS.iter().all(|f| f.2 <= MAX_ARGS));
    }

    /// More formulas than the cache holds: the ones asked for lately stay,
    /// instead of the whole cache emptying when it fills.
    #[test]
    fn a_full_cache_keeps_its_recent_formulas() {
        let src = |k: usize| format!("value + {k}");
        let first: Vec<_> = (0..CACHE_SIZE + 100).map(|k| parse_cached(&src(k), &[])).collect();
        for k in 100..CACHE_SIZE + 100 {
            assert!(Rc::ptr_eq(&first[k], &parse_cached(&src(k), &[])), "formula {k} was parsed again");
        }
    }

    /// Timings for the hot path (run with `--ignored --nocapture`, in a
    /// release build, to compare).
    #[test]
    #[ignore]
    fn timing_of_evaluation() {
        let mut cx = Keys { t: 0.4, i: 0.0, keys: vec![(0.0, 0.0), (1.0, 100.0)] };
        let src = "amp = 3; wiggle(2, amp) + sin(t * 3) * max(1, 2, value, 4) + clamp(t, 0, 1)";
        let n = 200_000;
        let start = std::time::Instant::now();
        let mut sum = 0.0;
        for k in 0..n {
            let e = parse_cached(src, &[]);
            cx.t = k as f64 * 1e-4;
            sum += e.as_ref().as_ref().unwrap().eval_in(&cx);
        }
        let per = start.elapsed().as_nanos() as f64 / n as f64;
        println!("cached parse + eval: {per:.0} ns a look ({sum})");
    }
}
