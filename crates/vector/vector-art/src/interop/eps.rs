// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! EPS out and in.
//!
//! Export writes Encapsulated PostScript (level 3): opaque solid fills and
//! strokes as vectors with their joins, caps and dashes, linear and radial
//! gradients as smooth shadings, clipping groups as clips. PostScript has
//! no transparency, so anything see-through, blended or with effects is
//! drawn as an image at print resolution in its place.
//!
//! Import runs the PostScript with a small interpreter covering paths,
//! painting, colour, the graphics state and transformations, stacks and
//! dictionaries, procedures, conditionals and loops: enough for EPS from
//! illustration programs and Studio's own. Text and images are skipped.

use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use studio_vector_geom::PathData;
use studio_vector_raster::{BlendMode, Cap, Color, Extend, GradientKind, Item as SI, Join, Paint, Scene, StrokeStyle};

use crate::appearance::{Appearance, Fill, Item, Stroke};
use crate::build::{self, BuildOptions};
use crate::doc::{Document, Node, NodeKind};
use crate::FillRule;

// ================================================================== export

fn n(v: f64) -> String {
    let r = (v * 10000.0).round() / 10000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

fn path_ops(out: &mut String, p: &BezPath) {
    let mut last = Point::ZERO;
    for el in p.elements() {
        let _ = match *el {
            PathEl::MoveTo(a) => {
                last = a;
                writeln!(out, "{} {} moveto", n(a.x), n(a.y))
            }
            PathEl::LineTo(a) => {
                last = a;
                writeln!(out, "{} {} lineto", n(a.x), n(a.y))
            }
            PathEl::QuadTo(c, b) => {
                let c1 = last + (c - last) * (2.0 / 3.0);
                let c2 = b + (c - b) * (2.0 / 3.0);
                last = b;
                writeln!(out, "{} {} {} {} {} {} curveto", n(c1.x), n(c1.y), n(c2.x), n(c2.y), n(b.x), n(b.y))
            }
            PathEl::CurveTo(a, b, c) => {
                last = c;
                writeln!(out, "{} {} {} {} {} {} curveto", n(a.x), n(a.y), n(b.x), n(b.y), n(c.x), n(c.y))
            }
            PathEl::ClosePath => writeln!(out, "closepath"),
        };
    }
}

fn concat(out: &mut String, m: Affine) {
    let c = m.as_coeffs();
    let _ = writeln!(out, "[{} {} {} {} {} {}] concat", n(c[0]), n(c[1]), n(c[2]), n(c[3]), n(c[4]), n(c[5]));
}

fn function(stops: &[studio_vector_raster::Stop]) -> String {
    let c = |s: &studio_vector_raster::Stop| format!("[{} {} {}]", n(s.color.r as f64), n(s.color.g as f64), n(s.color.b as f64));
    let st: Vec<_> = if stops.len() == 1 { vec![stops[0], stops[0]] } else { stops.to_vec() };
    let parts: Vec<String> = st.windows(2).map(|w| format!("<< /FunctionType 2 /Domain [0 1] /C0 {} /C1 {} /N 1 >>", c(&w[0]), c(&w[1]))).collect();
    let bounds: Vec<String> = st.windows(2).skip(1).map(|w| n(w[0].offset as f64)).collect();
    let enc: Vec<&str> = parts.iter().map(|_| "0 1").collect();
    format!("<< /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{}] >>", parts.join(" "), bounds.join(" "), enc.join(" "))
}

struct Ex {
    scale: f64,
}

impl Ex {
    fn raster(&self, scene: &Scene, out: &mut String) {
        let Some(b) = scene.bounds() else { return };
        let mut s = self.scale;
        let px = b.width() * b.height() * s * s;
        if px > 9e6 {
            s *= (9e6 / px).sqrt();
        }
        let (w, h) = ((b.width() * s).ceil().max(1.0) as u32, (b.height() * s).ceil().max(1.0) as u32);
        let mut o = studio_vector_raster::RenderOptions::new(w, h).with_transform(Affine::scale(s) * Affine::translate((-b.x0, -b.y0)));
        o.background = Some(Color::WHITE);
        let pm = studio_vector_raster::render(scene, &o);
        let px = pm.to_straight();
        let (bw, bh) = (w as f64 / s, h as f64 / s);
        // A white-backed image: PostScript cannot show transparency.
        let _ = writeln!(out, "gsave {} {} translate {} {} scale", n(b.x0), n(b.y0), n(bw), n(bh));
        let _ = writeln!(out, "{w} {h} 8 [{w} 0 0 {h} 0 0] currentfile /ASCIIHexDecode filter false 3 colorimage");
        let mut line = 0;
        for p in px.chunks_exact(4) {
            let _ = write!(out, "{:02x}{:02x}{:02x}", p[0], p[1], p[2]);
            line += 6;
            if line >= 72 {
                out.push('\n');
                line = 0;
            }
        }
        out.push_str(">\ngrestore\n");
    }

    fn items(&self, scene: &Scene, out: &mut String) {
        for it in &scene.items {
            let fallback = |out: &mut String| {
                let mut s = Scene::new();
                s.items.push(it.clone());
                self.raster(&s, out);
            };
            match it {
                SI::Fill { path, transform, rule, paint, blend, opacity } => {
                    if *opacity < 0.999 || *blend != BlendMode::Normal {
                        fallback(out);
                        continue;
                    }
                    let op = if *rule == FillRule::EvenOdd { "eofill" } else { "fill" };
                    match paint {
                        Paint::Solid { color } if color.a >= 0.999 => {
                            let _ = writeln!(out, "gsave {} {} {} setrgbcolor", n(color.r as f64), n(color.g as f64), n(color.b as f64));
                            concat(out, *transform);
                            out.push_str("newpath\n");
                            path_ops(out, path);
                            let _ = writeln!(out, "{op} grestore");
                        }
                        Paint::Gradient(g) if g.stops.iter().all(|s| s.color.a >= 0.999) && g.extend == Extend::Pad && !matches!(g.kind, GradientKind::Conic { .. }) => {
                            let clip = if *rule == FillRule::EvenOdd { "eoclip" } else { "clip" };
                            out.push_str("gsave\n");
                            concat(out, *transform);
                            out.push_str("newpath\n");
                            path_ops(out, path);
                            let _ = writeln!(out, "{clip} newpath");
                            concat(out, g.transform);
                            let sh = match g.kind {
                                GradientKind::Linear { start, end } => format!("<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [{} {} {} {}] /Function {} /Extend [true true] >>", n(start.x), n(start.y), n(end.x), n(end.y), function(&g.stops)),
                                GradientKind::Radial { c0, r0, c1, r1 } => format!("<< /ShadingType 3 /ColorSpace /DeviceRGB /Coords [{} {} {} {} {} {}] /Function {} /Extend [true true] >>", n(c0.x), n(c0.y), n(r0), n(c1.x), n(c1.y), n(r1), function(&g.stops)),
                                GradientKind::Conic { .. } => String::new(),
                            };
                            let _ = writeln!(out, "{sh} shfill grestore");
                        }
                        _ => fallback(out),
                    }
                }
                SI::Stroke { path, transform, style, paint, blend, opacity } => {
                    let Paint::Solid { color } = paint else {
                        fallback(out);
                        continue;
                    };
                    if color.a < 0.999 || *opacity < 0.999 || *blend != BlendMode::Normal || !style.profile.is_empty() || style.align != studio_vector_raster::StrokeAlign::Center {
                        fallback(out);
                        continue;
                    }
                    let _ = writeln!(out, "gsave {} {} {} setrgbcolor", n(color.r as f64), n(color.g as f64), n(color.b as f64));
                    concat(out, *transform);
                    stroke_params(out, style);
                    out.push_str("newpath\n");
                    path_ops(out, path);
                    out.push_str("stroke grestore\n");
                }
                SI::Layer { layer, scene: inner } => {
                    let plain = layer.opacity >= 0.999 && layer.blend == BlendMode::Normal && layer.mask.is_none() && layer.filters.is_empty();
                    if !plain {
                        fallback(out);
                        continue;
                    }
                    out.push_str("gsave\n");
                    if let Some(c) = &layer.clip {
                        // The clip path in its own space, then the matrix put back.
                        concat(out, c.transform);
                        out.push_str("newpath\n");
                        path_ops(out, &c.path);
                        out.push_str(if c.rule == FillRule::EvenOdd { "eoclip newpath\n" } else { "clip newpath\n" });
                        concat(out, c.transform.inverse());
                    }
                    self.items(inner, out);
                    out.push_str("grestore\n");
                }
            }
        }
    }
}

fn stroke_params(out: &mut String, s: &StrokeStyle) {
    let j = match s.join {
        Join::Miter => 0,
        Join::Round => 1,
        Join::Bevel => 2,
    };
    let c = match s.start_cap {
        Cap::Butt => 0,
        Cap::Round => 1,
        Cap::Square => 2,
    };
    let dash = s.dashes.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" ");
    let _ = writeln!(out, "{} setlinewidth {j} setlinejoin {c} setlinecap {} setmiterlimit [{dash}] {} setdash", n(s.width), n(s.miter_limit.max(1.0)), n(s.dash_offset));
}

/// One artboard as EPS.
pub fn export(doc: &Document, artboard: Option<&str>, opts: &BuildOptions) -> String {
    let ab = doc.artboard(artboard);
    let (scene, rect) = build::artboard_scene(doc, ab, opts);
    let mut body = String::new();
    // Document space (y down) to PostScript points (y up).
    let _ = writeln!(body, "[1 0 0 -1 {} {}] concat", n(-rect.x0), n(rect.y1));
    Ex { scale: 300.0 / 72.0 }.items(&scene, &mut body);
    let (w, h) = (rect.width().ceil(), rect.height().ceil());
    format!(
        "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 {} {}\n%%HiResBoundingBox: 0 0 {} {}\n%%Creator: Studio\n%%Title: {}\n%%LanguageLevel: 3\n%%EndComments\nsave\n{body}restore\nshowpage\n%%EOF\n",
        w as i64,
        h as i64,
        n(rect.width()),
        n(rect.height()),
        doc.name.replace(['\n', '\r'], " ")
    )
}

// ================================================================== import

#[derive(Clone, Debug)]
enum V {
    Num(f64),
    Bool(bool),
    Name(Rc<str>),
    /// An executable name (an operator or a procedure call).
    Exec(Rc<str>),
    Str(Rc<Vec<u8>>),
    Arr(Rc<std::cell::RefCell<Vec<V>>>),
    Proc(Rc<Vec<V>>),
    Dict(Rc<std::cell::RefCell<HashMap<String, V>>>),
    Mark,
    Null,
}

impl V {
    fn num(&self) -> f64 {
        match self {
            V::Num(v) => *v,
            V::Bool(b) => *b as u8 as f64,
            _ => 0.0,
        }
    }
}

/// Splits PostScript into tokens, grouping `{ … }` into procedures.
fn parse(s: &[u8]) -> Vec<V> {
    fn go(lx: &mut super::pdf::Lexer, out: &mut Vec<V>, depth: usize) {
        while let Some(t) = lx.next() {
            use super::pdf::Obj2 as O;
            let v = match t {
                O::Num(v) => V::Num(v),
                O::Bool(b) => V::Bool(b),
                O::Name(n) => V::Name(n.into()),
                O::Str(s) => V::Str(Rc::new(s)),
                O::Null => V::Null,
                O::Arr(a) => V::Arr(Rc::new(std::cell::RefCell::new(a.into_iter().map(|x| match x {
                    O::Num(v) => V::Num(v),
                    O::Name(n) => V::Name(n.into()),
                    O::Bool(b) => V::Bool(b),
                    O::Op(o) => V::Exec(o.into()),
                    _ => V::Null,
                }).collect()))),
                O::Dict(_) => V::Null,
                O::Op(o) if o == "{" => {
                    let mut inner = Vec::new();
                    if depth < 64 {
                        go(lx, &mut inner, depth + 1);
                    }
                    V::Proc(Rc::new(inner))
                }
                O::Op(o) if o == "}" => return,
                O::Op(o) if (o == "colorimage" || o == "image") && out.iter().rev().take(8).any(|t| matches!(t, V::Exec(e) if &**e == "currentfile")) => {
                    // The image's data follows in the file itself: read it now
                    // (ASCII hex up to `>`) and hand it to the operator.
                    let mut hex = Vec::new();
                    while lx.i < lx.s.len() && lx.s[lx.i] != b'>' {
                        if lx.s[lx.i].is_ascii_hexdigit() {
                            hex.push(lx.s[lx.i]);
                        }
                        lx.i += 1;
                    }
                    lx.i += 1;
                    let data: Vec<u8> = hex.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap_or("0"), 16).unwrap_or(0)).collect();
                    out.push(V::Str(Rc::new(data)));
                    V::Exec(o.into())
                }
                O::Op(o) => V::Exec(o.into()),
                _ => continue,
            };
            out.push(v);
        }
    }
    let mut lx = super::pdf::Lexer::postscript(s);
    let mut out = Vec::new();
    go(&mut lx, &mut out, 0);
    out
}

#[derive(Clone)]
struct Gs {
    ctm: Affine,
    color: Color,
    width: f64,
    join: Join,
    cap: Cap,
    miter: f64,
    dashes: Vec<f64>,
    dash_offset: f64,
    path: BezPath,
    cur: Option<Point>,
    clip: Option<(BezPath, FillRule)>,
}

struct Ps<'d> {
    stack: Vec<V>,
    dicts: Vec<Rc<std::cell::RefCell<HashMap<String, V>>>>,
    gs: Gs,
    saved: Vec<Gs>,
    out: Vec<(Node, Option<(BezPath, FillRule)>)>,
    doc: &'d mut Document,
    steps: usize,
}

const MAX_STEPS: usize = 50_000_000;

impl Ps<'_> {
    fn pop(&mut self) -> V {
        self.stack.pop().unwrap_or(V::Null)
    }

    fn popn(&mut self) -> f64 {
        self.pop().num()
    }

    fn lookup(&self, k: &str) -> Option<V> {
        self.dicts.iter().rev().find_map(|d| d.borrow().get(k).cloned())
    }

    fn paint(&mut self, fill: Option<FillRule>) {
        self.paint_with(fill, None);
    }

    fn paint_with(&mut self, fill: Option<FillRule>, with: Option<Paint>) {
        let path = std::mem::take(&mut self.gs.path);
        self.gs.cur = None;
        if path.elements().is_empty() {
            return;
        }
        let mut ap = Appearance::default();
        if fill.is_some() {
            ap.items.push(Item::Fill(Fill::new(with.unwrap_or(Paint::solid(self.gs.color)))));
        } else {
            let k = self.gs.ctm.determinant().abs().sqrt();
            let mut s = Stroke::new(self.gs.color, (self.gs.width * k).max(0.25));
            s.style.join = self.gs.join;
            s.style.start_cap = self.gs.cap;
            s.style.end_cap = self.gs.cap;
            s.style.miter_limit = self.gs.miter;
            s.style.dashes = self.gs.dashes.iter().map(|d| d * k).collect();
            s.style.dash_offset = self.gs.dash_offset * k;
            ap.items.push(Item::Stroke(s));
        }
        let id = self.doc.new_id();
        let mut node = Node::new(id, NodeKind::Path { path: PathData::from_bezpath(&path), rule: fill.unwrap_or(FillRule::NonZero) });
        node.appearance = ap;
        self.out.push((node, self.gs.clip.clone()));
    }

    fn exec_all(&mut self, prog: &[V]) {
        for v in prog {
            if self.steps > MAX_STEPS {
                return;
            }
            self.exec(v.clone());
        }
    }

    fn call(&mut self, v: &V) {
        match v {
            V::Proc(p) => {
                let p = p.clone();
                self.exec_all(&p);
            }
            other => self.exec(other.clone()),
        }
    }

    fn exec(&mut self, v: V) {
        self.steps += 1;
        match v {
            V::Exec(op) => self.op(&op),
            other => self.stack.push(other),
        }
    }

    fn op(&mut self, op: &str) {
        if let Some(def) = self.lookup(op) {
            match &def {
                V::Proc(_) => self.call(&def),
                other => self.stack.push(other.clone()),
            }
            return;
        }
        let m = self.gs.ctm;
        match op {
            // Stack.
            "pop" => {
                self.pop();
            }
            "exch" => {
                let (b, a) = (self.pop(), self.pop());
                self.stack.push(b);
                self.stack.push(a);
            }
            "dup" => {
                let a = self.stack.last().cloned().unwrap_or(V::Null);
                self.stack.push(a);
            }
            "copy" => {
                let k = self.popn() as usize;
                let len = self.stack.len();
                if k <= len {
                    let top: Vec<V> = self.stack[len - k..].to_vec();
                    self.stack.extend(top);
                }
            }
            "index" => {
                let k = self.popn() as usize;
                let v = self.stack.iter().rev().nth(k).cloned().unwrap_or(V::Null);
                self.stack.push(v);
            }
            "roll" => {
                let j = self.popn() as i64;
                let k = self.popn() as usize;
                let len = self.stack.len();
                if k > 0 && k <= len {
                    let s = &mut self.stack[len - k..];
                    let j = j.rem_euclid(k as i64) as usize;
                    s.rotate_right(j);
                }
            }
            "clear" => self.stack.clear(),
            "count" => {
                let c = self.stack.len() as f64;
                self.stack.push(V::Num(c));
            }
            "mark" | "[" | "<<" => self.stack.push(V::Mark),
            "cleartomark" => while !matches!(self.pop(), V::Mark | V::Null) {},
            "]" => {
                let mut a = Vec::new();
                while let Some(v) = self.stack.pop() {
                    if matches!(v, V::Mark) {
                        break;
                    }
                    a.push(v);
                }
                a.reverse();
                self.stack.push(V::Arr(Rc::new(std::cell::RefCell::new(a))));
            }
            ">>" => {
                let mut kv = Vec::new();
                while let Some(v) = self.stack.pop() {
                    if matches!(v, V::Mark) {
                        break;
                    }
                    kv.push(v);
                }
                kv.reverse();
                let mut d = HashMap::new();
                for p in kv.chunks(2) {
                    if let (V::Name(k), Some(v)) = (&p[0], p.get(1)) {
                        d.insert(k.to_string(), v.clone());
                    }
                }
                self.stack.push(V::Dict(Rc::new(std::cell::RefCell::new(d))));
            }
            // Arithmetic.
            "add" => {
                let (b, a) = (self.popn(), self.popn());
                self.stack.push(V::Num(a + b));
            }
            "sub" => {
                let (b, a) = (self.popn(), self.popn());
                self.stack.push(V::Num(a - b));
            }
            "mul" => {
                let (b, a) = (self.popn(), self.popn());
                self.stack.push(V::Num(a * b));
            }
            "div" => {
                let (b, a) = (self.popn(), self.popn());
                self.stack.push(V::Num(if b != 0.0 { a / b } else { 0.0 }));
            }
            "idiv" => {
                let (b, a) = (self.popn() as i64, self.popn() as i64);
                self.stack.push(V::Num(if b != 0 { (a / b) as f64 } else { 0.0 }));
            }
            "mod" => {
                let (b, a) = (self.popn() as i64, self.popn() as i64);
                self.stack.push(V::Num(if b != 0 { (a % b) as f64 } else { 0.0 }));
            }
            "neg" => {
                let a = self.popn();
                self.stack.push(V::Num(-a));
            }
            "abs" => {
                let a = self.popn();
                self.stack.push(V::Num(a.abs()));
            }
            "sqrt" => {
                let a = self.popn();
                self.stack.push(V::Num(a.max(0.0).sqrt()));
            }
            "sin" | "cos" => {
                let a = self.popn().to_radians();
                self.stack.push(V::Num(if op == "sin" { a.sin() } else { a.cos() }));
            }
            "atan" => {
                let (d, n) = (self.popn(), self.popn());
                self.stack.push(V::Num(n.atan2(d).to_degrees().rem_euclid(360.0)));
            }
            "exp" => {
                let (e, b) = (self.popn(), self.popn());
                self.stack.push(V::Num(b.powf(e)));
            }
            "round" | "floor" | "ceiling" | "truncate" | "cvi" | "cvr" => {
                let a = self.popn();
                self.stack.push(V::Num(match op {
                    "round" => a.round(),
                    "floor" => a.floor(),
                    "ceiling" => a.ceil(),
                    "truncate" | "cvi" => a.trunc(),
                    _ => a,
                }));
            }
            "eq" | "ne" | "gt" | "lt" | "ge" | "le" => {
                let (b, a) = (self.popn(), self.popn());
                let r = match op {
                    "eq" => a == b,
                    "ne" => a != b,
                    "gt" => a > b,
                    "lt" => a < b,
                    "ge" => a >= b,
                    _ => a <= b,
                };
                self.stack.push(V::Bool(r));
            }
            "and" | "or" => {
                let (b, a) = (self.pop(), self.pop());
                let r = match (a, b) {
                    (V::Bool(x), V::Bool(y)) => V::Bool(if op == "and" { x && y } else { x || y }),
                    (x, y) => V::Num(if op == "and" { ((x.num() as i64) & (y.num() as i64)) as f64 } else { ((x.num() as i64) | (y.num() as i64)) as f64 }),
                };
                self.stack.push(r);
            }
            "not" => match self.pop() {
                V::Bool(b) => self.stack.push(V::Bool(!b)),
                v => self.stack.push(V::Num(!(v.num() as i64) as f64)),
            },
            "true" => self.stack.push(V::Bool(true)),
            "false" => self.stack.push(V::Bool(false)),
            // Control.
            "if" => {
                let p = self.pop();
                if let V::Bool(true) = self.pop() {
                    self.call(&p);
                }
            }
            "ifelse" => {
                let (no, yes) = (self.pop(), self.pop());
                let c = matches!(self.pop(), V::Bool(true));
                self.call(if c { &yes } else { &no });
            }
            "repeat" => {
                let p = self.pop();
                let k = self.popn().clamp(0.0, 1e6) as usize;
                for _ in 0..k {
                    if self.steps > MAX_STEPS {
                        break;
                    }
                    self.call(&p);
                }
            }
            "for" => {
                let p = self.pop();
                let (limit, inc, init) = (self.popn(), self.popn(), self.popn());
                let mut i = init;
                let mut guard = 0;
                while (inc > 0.0 && i <= limit) || (inc < 0.0 && i >= limit) {
                    self.stack.push(V::Num(i));
                    self.call(&p);
                    i += inc;
                    guard += 1;
                    if guard > 1_000_000 || self.steps > MAX_STEPS {
                        break;
                    }
                }
            }
            "exec" => {
                let p = self.pop();
                self.call(&p);
            }
            // Dictionaries.
            "def" => {
                let v = self.pop();
                if let V::Name(k) = self.pop()
                    && let Some(d) = self.dicts.last()
                {
                    d.borrow_mut().insert(k.to_string(), v);
                }
            }
            "dict" => {
                self.pop();
                self.stack.push(V::Dict(Rc::new(std::cell::RefCell::new(HashMap::new()))));
            }
            "begin" => {
                if let V::Dict(d) = self.pop() {
                    self.dicts.push(d);
                }
            }
            "end" => {
                if self.dicts.len() > 1 {
                    self.dicts.pop();
                }
            }
            "load" => {
                if let V::Name(k) = self.pop() {
                    let v = self.lookup(&k).unwrap_or(V::Null);
                    self.stack.push(v);
                }
            }
            "bind" | "readonly" | "executeonly" | "noaccess" | "cvx" | "cvlit" => {}
            "save" => self.stack.push(V::Null),
            "restore" => {
                self.pop();
            }
            "userdict" | "systemdict" | "globaldict" | "currentdict" | "statusdict" => {
                let d = self.dicts.first().cloned();
                if let Some(d) = d {
                    self.stack.push(V::Dict(d));
                }
            }
            "where" => {
                if let V::Name(k) = self.pop() {
                    match self.dicts.iter().rev().find(|d| d.borrow().contains_key(&*k)).cloned() {
                        Some(d) => {
                            self.stack.push(V::Dict(d));
                            self.stack.push(V::Bool(true));
                        }
                        None => self.stack.push(V::Bool(false)),
                    }
                }
            }
            "known" => {
                let k = self.pop();
                let d = self.pop();
                let r = match (d, k) {
                    (V::Dict(d), V::Name(k)) => d.borrow().contains_key(&*k),
                    _ => false,
                };
                self.stack.push(V::Bool(r));
            }
            "get" => {
                let k = self.pop();
                let c = self.pop();
                let v = match (c, k) {
                    (V::Arr(a), k) => a.borrow().get(k.num() as usize).cloned().unwrap_or(V::Null),
                    (V::Proc(p), k) => p.get(k.num() as usize).cloned().unwrap_or(V::Null),
                    (V::Dict(d), V::Name(k)) => d.borrow().get(&*k).cloned().unwrap_or(V::Null),
                    _ => V::Null,
                };
                self.stack.push(v);
            }
            "put" => {
                let v = self.pop();
                let k = self.pop();
                match (self.pop(), k) {
                    (V::Arr(a), k) => {
                        let i = k.num() as usize;
                        if let Some(slot) = a.borrow_mut().get_mut(i) {
                            *slot = v;
                        }
                    }
                    (V::Dict(d), V::Name(k)) => {
                        d.borrow_mut().insert(k.to_string(), v);
                    }
                    _ => {}
                }
            }
            "length" => {
                let l = match self.pop() {
                    V::Arr(a) => a.borrow().len(),
                    V::Proc(p) => p.len(),
                    V::Str(s) => s.len(),
                    V::Dict(d) => d.borrow().len(),
                    _ => 0,
                };
                self.stack.push(V::Num(l as f64));
            }
            "array" => {
                let k = self.popn().clamp(0.0, 1e6) as usize;
                self.stack.push(V::Arr(Rc::new(std::cell::RefCell::new(vec![V::Null; k]))));
            }
            "aload" => {
                if let V::Arr(a) = self.pop() {
                    let items = a.borrow().clone();
                    self.stack.extend(items);
                    self.stack.push(V::Arr(a));
                }
            }
            "forall" => {
                let p = self.pop();
                if let V::Arr(a) = self.pop() {
                    let items = a.borrow().clone();
                    for it in items {
                        self.stack.push(it);
                        self.call(&p);
                    }
                }
            }
            // Graphics state.
            "gsave" => self.saved.push(self.gs.clone()),
            "grestore" => {
                if let Some(g) = self.saved.pop() {
                    self.gs = g;
                }
            }
            "translate" => {
                let (y, x) = (self.popn(), self.popn());
                self.gs.ctm = m * Affine::translate((x, y));
            }
            "scale" => {
                let (y, x) = (self.popn(), self.popn());
                self.gs.ctm = m * Affine::scale_non_uniform(x, y);
            }
            "rotate" => {
                let a = self.popn();
                self.gs.ctm = m * Affine::rotate(a.to_radians());
            }
            "concat" => {
                if let V::Arr(a) = self.pop() {
                    let v: Vec<f64> = a.borrow().iter().map(V::num).collect();
                    if v.len() == 6 {
                        self.gs.ctm = m * Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]);
                    }
                }
            }
            "setlinewidth" => self.gs.width = self.popn(),
            "setlinejoin" => self.gs.join = [Join::Miter, Join::Round, Join::Bevel][(self.popn() as usize).min(2)],
            "setlinecap" => self.gs.cap = [Cap::Butt, Cap::Round, Cap::Square][(self.popn() as usize).min(2)],
            "setmiterlimit" => self.gs.miter = self.popn(),
            "setdash" => {
                self.gs.dash_offset = self.popn();
                if let V::Arr(a) = self.pop() {
                    self.gs.dashes = a.borrow().iter().map(V::num).collect();
                }
            }
            "setrgbcolor" => {
                let (b, g, r) = (self.popn(), self.popn(), self.popn());
                self.gs.color = Color::rgb(r as f32, g as f32, b as f32);
            }
            "setgray" => {
                let g = self.popn() as f32;
                self.gs.color = Color::rgb(g, g, g);
            }
            "setcmykcolor" => {
                let (k, y, mm, c) = (self.popn(), self.popn(), self.popn(), self.popn());
                self.gs.color = Color::rgb(((1.0 - c) * (1.0 - k)) as f32, ((1.0 - mm) * (1.0 - k)) as f32, ((1.0 - y) * (1.0 - k)) as f32);
            }
            "sethsbcolor" => {
                let (b, s, h) = (self.popn(), self.popn(), self.popn());
                let c = Color::from_hsl(h as f32 * 360.0, s as f32, b as f32 * (1.0 - s as f32 / 2.0), 1.0);
                self.gs.color = c;
            }
            // Paths.
            "newpath" => {
                self.gs.path = BezPath::new();
                self.gs.cur = None;
            }
            "moveto" | "rmoveto" => {
                let (y, x) = (self.popn(), self.popn());
                let p = if op == "rmoveto" { self.gs.cur.unwrap_or(Point::ZERO) + kurbo::Vec2::new(x, y) } else { Point::new(x, y) };
                self.gs.path.move_to(m * p);
                self.gs.cur = Some(p);
            }
            "lineto" | "rlineto" => {
                let (y, x) = (self.popn(), self.popn());
                let p = if op == "rlineto" { self.gs.cur.unwrap_or(Point::ZERO) + kurbo::Vec2::new(x, y) } else { Point::new(x, y) };
                self.gs.path.line_to(m * p);
                self.gs.cur = Some(p);
            }
            "curveto" | "rcurveto" => {
                let v: Vec<f64> = (0..6).map(|_| self.popn()).collect();
                let base = if op == "rcurveto" { self.gs.cur.unwrap_or(Point::ZERO).to_vec2() } else { kurbo::Vec2::ZERO };
                let (p1, p2, p3) = (Point::new(v[5], v[4]) + base, Point::new(v[3], v[2]) + base, Point::new(v[1], v[0]) + base);
                self.gs.path.curve_to(m * p1, m * p2, m * p3);
                self.gs.cur = Some(p3);
            }
            "closepath" => self.gs.path.close_path(),
            "arc" | "arcn" => {
                let (a2, a1, r, y, x) = (self.popn(), self.popn(), self.popn(), self.popn(), self.popn());
                let (mut s, mut e) = (a1.to_radians(), a2.to_radians());
                if op == "arc" {
                    while e < s {
                        e += std::f64::consts::TAU;
                    }
                } else {
                    while e > s {
                        e -= std::f64::consts::TAU;
                    }
                    std::mem::swap(&mut s, &mut e);
                }
                let arc = kurbo::Arc::new((x, y), (r, r), s, e - s, 0.0);
                let mut first = true;
                let mut segs: Vec<PathEl> = Vec::new();
                arc.to_cubic_beziers(0.01, |p1, p2, p| segs.push(PathEl::CurveTo(p1, p2, p)));
                let start = Point::new(x + r * s.cos(), y + r * s.sin());
                if op == "arcn" {
                    // Drawn the other way round: reverse the pieces.
                    let mut rev = BezPath::new();
                    rev.move_to(start);
                    for el in &segs {
                        rev.push(*el);
                    }
                    let rev = rev.reverse_subpaths();
                    for el in rev.elements() {
                        match *el {
                            PathEl::MoveTo(p) => {
                                if self.gs.cur.is_some() { self.gs.path.line_to(m * p) } else { self.gs.path.move_to(m * p) }
                            }
                            PathEl::CurveTo(a, b, c) => self.gs.path.curve_to(m * a, m * b, m * c),
                            _ => {}
                        }
                    }
                    self.gs.cur = Some(start);
                } else {
                    for el in segs {
                        if first {
                            if self.gs.cur.is_some() { self.gs.path.line_to(m * start) } else { self.gs.path.move_to(m * start) }
                            first = false;
                        }
                        if let PathEl::CurveTo(a, b, c) = el {
                            self.gs.path.curve_to(m * a, m * b, m * c);
                            self.gs.cur = Some(c);
                        }
                    }
                }
            }
            "rectfill" | "rectstroke" | "rectclip" => {
                let (h, w, y, x) = (self.popn(), self.popn(), self.popn(), self.popn());
                let saved = std::mem::take(&mut self.gs.path);
                self.gs.path = m * Rect::new(x, y, x + w, y + h).to_path(0.0);
                match op {
                    "rectfill" => self.paint(Some(FillRule::NonZero)),
                    "rectstroke" => self.paint(None),
                    _ => {
                        self.gs.clip = Some((self.gs.path.clone(), FillRule::NonZero));
                        self.gs.path = BezPath::new();
                    }
                }
                if op != "rectclip" {
                    self.gs.path = saved;
                }
            }
            "fill" => self.paint(Some(FillRule::NonZero)),
            "eofill" => self.paint(Some(FillRule::EvenOdd)),
            "stroke" => self.paint(None),
            "clip" | "eoclip" => {
                let rule = if op == "eoclip" { FillRule::EvenOdd } else { FillRule::NonZero };
                self.gs.clip = Some((self.gs.path.clone(), rule));
            }
            "initclip" => self.gs.clip = None,
            "shfill" => {
                // A shading fills the clip as a gradient.
                if let V::Dict(d) = self.pop()
                    && let Some((clip, rule)) = self.gs.clip.clone()
                    && let Some(paint) = shading(&d.borrow(), self.gs.ctm)
                {
                    let saved = std::mem::take(&mut self.gs.path);
                    self.gs.path = clip;
                    self.paint_with(Some(rule), Some(paint));
                    self.gs.path = saved;
                }
            }
            "colorimage" | "image" if matches!(self.stack.last(), Some(V::Str(_))) => {
                // Inline image data (see `parse`): w h bits matrix source [multi ncomp].
                let Some(V::Str(data)) = self.stack.pop() else { return };
                let ncomp = if op == "colorimage" {
                    let n = self.popn() as usize;
                    self.pop();
                    n
                } else {
                    1
                };
                self.pop();
                let mat = self.pop();
                let bits = self.popn();
                let (h, w) = (self.popn() as u32, self.popn() as u32);
                if bits == 8.0 && w > 0 && h > 0 && data.len() >= (w * h) as usize * ncomp {
                    let mut px = Vec::with_capacity((w * h * 4) as usize);
                    for p in data.chunks_exact(ncomp).take((w * h) as usize) {
                        match ncomp {
                            1 => px.extend_from_slice(&[p[0], p[0], p[0], 255]),
                            _ => px.extend_from_slice(&[p[0], p[1], p[2], 255]),
                        }
                    }
                    // The matrix maps user space to image pixels; the image fills its unit square.
                    let im = match mat {
                        V::Arr(a) => {
                            let v: Vec<f64> = a.borrow().iter().map(V::num).collect();
                            if v.len() == 6 { Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]) } else { Affine::scale_non_uniform(w as f64, h as f64) }
                        }
                        _ => Affine::scale_non_uniform(w as f64, h as f64),
                    };
                    if let Some(img) = studio_vector_raster::Image::from_straight(w, h, px) {
                        let id = self.doc.new_id();
                        let mut n = Node::new(id, NodeKind::Image(crate::doc::ImageNode { image: Some(img), href: None, width: w as f64, height: h as f64 }));
                        n.transform = self.gs.ctm * im.inverse();
                        self.out.push((n, self.gs.clip.clone()));
                    }
                }
            }
            "colorimage" | "image" | "imagemask" | "show" | "ashow" | "widthshow" | "kshow" | "xshow" | "showpage" | "setfont" | "findfont" | "scalefont" | "selectfont" | "currentfile" | "filter" | "readhexstring" | "readstring" => {
                // Images and type are not read; their operands are dropped.
                if op == "colorimage" || op == "image" {
                    self.stack.clear();
                }
            }
            _ => {}
        }
    }
}

/// Reads an EPS (or PostScript) file's vector art.
pub fn import(data: &[u8]) -> Result<Document, String> {
    let text = String::from_utf8_lossy(data);
    if !text.starts_with("%!") && !text.contains("%!PS") {
        return Err("not PostScript".into());
    }
    let bbox: Vec<f64> = text
        .lines()
        .find_map(|l| l.strip_prefix("%%HiResBoundingBox:").or_else(|| l.strip_prefix("%%BoundingBox:")))
        .map(|l| l.split_whitespace().filter_map(|v| v.parse().ok()).collect())
        .unwrap_or_else(|| vec![0.0, 0.0, 612.0, 792.0]);
    let (x0, y0, x1, y1) = if bbox.len() == 4 { (bbox[0], bbox[1], bbox[2], bbox[3]) } else { (0.0, 0.0, 612.0, 792.0) };
    let mut doc = Document::new(x1 - x0, y1 - y0);
    // Embedded binary previews (DOS EPS headers) are skipped: only the text part runs.
    let start = text.find("%!PS").unwrap_or(0);
    let prog = parse(&data[start.min(data.len())..]);
    let base = Affine::new([1.0, 0.0, 0.0, -1.0, -x0, y1]);
    let user = Rc::new(std::cell::RefCell::new(HashMap::new()));
    let gs = Gs { ctm: base, color: Color::BLACK, width: 1.0, join: Join::Miter, cap: Cap::Butt, miter: 10.0, dashes: Vec::new(), dash_offset: 0.0, path: BezPath::new(), cur: None, clip: None };
    let out = {
        let mut ps = Ps { stack: Vec::new(), dicts: vec![user], gs, saved: Vec::new(), out: Vec::new(), doc: &mut doc, steps: 0 };
        ps.exec_all(&prog);
        ps.out
    };
    // Shapes under the same clip go into one clipping group.
    let mut nodes: Vec<Node> = Vec::new();
    let mut group: Option<(BezPath, Vec<Node>)> = None;
    let flush = |g: &mut Option<(BezPath, Vec<Node>)>, nodes: &mut Vec<Node>, doc: &mut Document| {
        if let Some((clip, kids)) = g.take() {
            let cid = doc.new_id();
            let mut all = vec![Node::new(cid, NodeKind::Path { path: PathData::from_bezpath(&clip), rule: FillRule::NonZero })];
            all.extend(kids);
            let gid = doc.new_id();
            nodes.push(Node::new(gid, NodeKind::Group { children: all, clip: true, layer: false }));
        }
    };
    for (node, clip) in out {
        match clip {
            None => {
                flush(&mut group, &mut nodes, &mut doc);
                nodes.push(node);
            }
            Some((c, _)) => {
                if group.as_ref().is_some_and(|(g, _)| *g == c) {
                    if let Some((_, kids)) = &mut group {
                        kids.push(node);
                    }
                } else {
                    flush(&mut group, &mut nodes, &mut doc);
                    group = Some((c, vec![node]));
                }
            }
        }
    }
    flush(&mut group, &mut nodes, &mut doc);
    let layer = doc.layers[0].id;
    if let Some(c) = doc.find_mut(layer).and_then(Node::children_mut) {
        *c = nodes;
    }
    doc.fix_ids();
    Ok(doc)
}

/// A gradient paint from a PostScript axial or radial shading dictionary,
/// drawn through `ctm`.
fn shading(d: &HashMap<String, V>, ctm: Affine) -> Option<Paint> {
    let nums = |v: Option<&V>| -> Vec<f64> {
        match v {
            Some(V::Arr(a)) => a.borrow().iter().map(V::num).collect(),
            _ => Vec::new(),
        }
    };
    let colour = |v: Option<&V>| -> Color {
        let c = nums(v);
        match c.len() {
            1 => Color::rgb(c[0] as f32, c[0] as f32, c[0] as f32),
            4 => Color::rgb(((1.0 - c[0]) * (1.0 - c[3])) as f32, ((1.0 - c[1]) * (1.0 - c[3])) as f32, ((1.0 - c[2]) * (1.0 - c[3])) as f32),
            3 => Color::rgb(c[0] as f32, c[1] as f32, c[2] as f32),
            _ => Color::rgb(0.5, 0.5, 0.5),
        }
    };
    let kind = d.get("ShadingType").map(V::num)? as u32;
    let coords = nums(d.get("Coords"));
    let Some(V::Dict(f)) = d.get("Function") else { return None };
    let f = f.borrow();
    let mut stops = Vec::new();
    match f.get("FunctionType").map(V::num).unwrap_or(0.0) as u32 {
        2 => {
            stops.push(studio_vector_raster::Stop::new(0.0, colour(f.get("C0"))));
            stops.push(studio_vector_raster::Stop::new(1.0, colour(f.get("C1"))));
        }
        3 => {
            let Some(V::Arr(funcs)) = f.get("Functions") else { return None };
            let mut edges = vec![0.0];
            edges.extend(nums(f.get("Bounds")));
            edges.push(1.0);
            for (i, sub) in funcs.borrow().iter().enumerate() {
                let V::Dict(sd) = sub else { continue };
                let sd = sd.borrow();
                if i == 0 {
                    stops.push(studio_vector_raster::Stop::new(edges[0] as f32, colour(sd.get("C0"))));
                }
                stops.push(studio_vector_raster::Stop::new(edges.get(i + 1).copied().unwrap_or(1.0) as f32, colour(sd.get("C1"))));
            }
        }
        _ => return None,
    }
    let g = match (kind, coords.as_slice()) {
        (2, [x0, y0, x1, y1]) => studio_vector_raster::Gradient::linear((*x0, *y0), (*x1, *y1), stops),
        (3, [x0, y0, r0, x1, y1, r1]) => studio_vector_raster::Gradient::new(GradientKind::Radial { c0: Point::new(*x0, *y0), r0: *r0, c1: Point::new(*x1, *y1), r1: *r1 }, stops),
        _ => return None,
    };
    Some(Paint::gradient(g.with_transform(ctm)))
}
