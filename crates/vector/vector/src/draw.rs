// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A shape document worked through like a recipe and drawn.
//!
//! Inside a group, items run in order: shapes add outlines; operators
//! (merge, trim, offset, wiggle, zig-zag, round corners, pucker & bloat,
//! twist, morph, repeater) change the outlines made so far; a fill or a
//! stroke paints the outlines as they are at that point, over what was
//! painted before. A group with paint of its own draws itself; a group
//! without hands its outlines to the group around it.

use crate::ops::{self, Merge};
use crate::path::{self, ID, M, Path, Sub, V, mul};
use crate::value::Ctx;
use serde_json::Value;
use studio_vector_raster::kurbo::{Affine, Point};
use studio_vector_raster::{BlendMode, Cap as LineCap, Color, FillRule, Gradient, Join as LineJoin, Paint, Scene, Stop, StrokeStyle};

/// What paints: a solid colour or a gradient (in the shape's own space).
#[derive(Clone, Debug)]
enum Ink {
    Solid([f64; 4]),
    Linear { from: [f64; 2], to: [f64; 2], stops: Vec<(f64, [f64; 4])> },
    Radial { centre: [f64; 2], focal: [f64; 2], radius: f64, stops: Vec<(f64, [f64; 4])> },
}

#[derive(Clone, Debug)]
enum Style {
    Fill { even_odd: bool },
    Stroke { width: f64, cap: LineCap, join: LineJoin, miter: f64, dash: Vec<f64>, dash_offset: f64 },
}

/// One painted outline.
#[derive(Clone, Debug)]
struct Draw {
    path: Path,
    ink: Ink,
    style: Style,
    alpha: f64,
    blend: BlendMode,
    /// From the outline's space to the document's.
    m: M,
}

#[derive(Default)]
struct State {
    paths: Vec<Path>,
    draws: Vec<Draw>,
    painted: bool,
}

/// Problems worth telling the author about (unknown item types…).
#[derive(Default)]
pub struct Notes {
    pub warnings: Vec<String>,
}

const MAX_DEPTH: usize = 16;
const MAX_COPIES: usize = 400;

fn items_of(v: &Value) -> Option<&Vec<Value>> {
    match v {
        Value::Array(a) => Some(a),
        Value::Object(o) => o.get("items").and_then(Value::as_array),
        _ => None,
    }
}

fn blend_of(s: &str) -> BlendMode {
    match s {
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "add" | "plus" => BlendMode::Plus,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "soft_light" => BlendMode::SoftLight,
        "hard_light" => BlendMode::HardLight,
        "colour_dodge" | "color_dodge" => BlendMode::ColorDodge,
        "colour_burn" | "color_burn" => BlendMode::ColorBurn,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "colour" | "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

/// A group's own transform (`position`, `anchor`, `scale`, `rotation`,
/// `skew`, `skew_axis`).
fn transform_of(o: &serde_json::Map<String, Value>, cx: &Ctx) -> M {
    let has = ["position", "anchor", "scale", "rotation", "skew"].iter().any(|k| o.contains_key(*k));
    if !has {
        return ID;
    }
    path::layer(cx.pt(o.get("position"), [0.0, 0.0]), cx.pt(o.get("anchor"), [0.0, 0.0]), cx.pt(o.get("scale"), [1.0, 1.0]), cx.num(o.get("rotation"), 0.0), cx.num(o.get("skew"), 0.0), cx.num(o.get("skew_axis"), 0.0))
}

fn stops_of(v: Option<&Value>, cx: &Ctx, opacity: f64) -> Vec<(f64, [f64; 4])> {
    let Some(Value::Array(list)) = v.map(|v| cx.resolve(v)) else { return vec![(0.0, [1.0; 4]), (1.0, [0.0, 0.0, 0.0, 1.0])] };
    let n = list.len();
    let mut out: Vec<(f64, [f64; 4])> = list
        .iter()
        .enumerate()
        .filter_map(|(k, s)| {
            let even = if n > 1 { k as f64 / (n - 1) as f64 } else { 0.0 };
            // "#colour", [at, "#colour"], or {"at", "colour"}.
            let (at, c) = match s {
                Value::Array(a) if a.len() == 2 && a[0].is_number() => (a[0].as_f64()?, crate::value::parse_colour(&a[1])?),
                Value::Object(o) => (o.get("at").and_then(Value::as_f64).unwrap_or(even), crate::value::parse_colour(o.get("colour").or(o.get("color"))?)?),
                other => (even, crate::value::parse_colour(other)?),
            };
            Some((at.clamp(0.0, 1.0), [c[0], c[1], c[2], c[3] * opacity]))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    if out.is_empty() {
        out.push((0.0, [1.0; 4]));
    }
    out
}

fn ink_of(o: &serde_json::Map<String, Value>, cx: &Ctx, opacity: f64) -> Ink {
    if let Some(Value::Object(g)) = o.get("gradient") {
        let stops = stops_of(g.get("stops").or(g.get("colours")).or(g.get("colors")), cx, opacity);
        return match g.get("type").and_then(Value::as_str).unwrap_or("linear") {
            "radial" => {
                let centre = cx.pt(g.get("centre").or(g.get("center")), [0.0, 0.0]);
                Ink::Radial { centre, focal: cx.pt(g.get("focal"), centre), radius: cx.num(g.get("radius"), 100.0).max(1e-3), stops }
            }
            _ => Ink::Linear { from: cx.pt(g.get("from"), [-100.0, 0.0]), to: cx.pt(g.get("to"), [100.0, 0.0]), stops },
        };
    }
    let c = cx.colour(o.get("colour").or(o.get("color")), [1.0, 1.0, 1.0, 1.0]);
    Ink::Solid([c[0], c[1], c[2], c[3] * opacity])
}

/// The paths of shape items (`rect`, `ellipse`, `polygon`, `star`,
/// `path`, `line`, `arc`, `pie`) in a list, for morph targets.
fn shapes_in(v: &Value, cx: &Ctx, notes: &mut Notes) -> Vec<Path> {
    let list: Vec<Value> = match v {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    list.iter().filter_map(|it| it.as_object().and_then(|o| shape(o, cx, notes))).collect()
}

/// One shape item as a path, if it is one.
fn shape(o: &serde_json::Map<String, Value>, cx: &Ctx, notes: &mut Notes) -> Option<Path> {
    let kind = o.get("type").and_then(Value::as_str)?;
    let at = cx.pt(o.get("position").or(o.get("centre")).or(o.get("center")), [0.0, 0.0]);
    if kind == "curve" {
        // A curve from mathematics, worked out at this frame's time.
        return match crate::curve::make(o, cx.t, &|k, d| cx.num(o.get(k), d)) {
            Ok(l) => {
                let subs: Vec<Sub> = l.lines.iter().map(|(pts, closed)| {
                    let pts: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] + at[0], p[1] + at[1]]).collect();
                    if cx.flag(o.get("smooth"), false) { path::smooth(&pts, *closed, 1.0) } else { Sub::polyline(&pts, *closed) }
                }).collect();
                Some(Path::new(subs))
            }
            Err(e) => {
                notes.warnings.push(format!("curve: {e}"));
                None
            }
        };
    }
    let sub = match kind {
        "rect" | "rectangle" => ops::rect(at, cx.pt(o.get("size"), [100.0, 100.0]), cx.num(o.get("roundness").or(o.get("radius")), 0.0)),
        "ellipse" | "circle" => {
            let size = match (o.get("size"), o.get("radius")) {
                (Some(s), _) => cx.pt(Some(s), [100.0, 100.0]),
                (None, Some(r)) => {
                    let r = cx.num(Some(r), 50.0) * 2.0;
                    [r, r]
                }
                _ => [100.0, 100.0],
            };
            ops::ellipse(at, size)
        }
        "polygon" | "star" => {
            let points = cx.num(o.get("points").or(o.get("sides")), if kind == "star" { 5.0 } else { 6.0 });
            let outer = cx.num(o.get("radius").or(o.get("outer")), 100.0);
            let inner = (kind == "star").then(|| cx.num(o.get("inner"), outer * 0.45));
            ops::star(at, points, outer, inner, cx.num(o.get("rotation"), 0.0), cx.num(o.get("roundness").or(o.get("outer_roundness")), 0.0), cx.num(o.get("inner_roundness"), 0.0))
        }
        "line" => {
            let (a, b) = (cx.pt(o.get("from"), [-100.0, 0.0]), cx.pt(o.get("to"), [100.0, 0.0]));
            Sub::polyline(&[a, b], false)
        }
        "arc" | "pie" => {
            // Degrees, 0 straight up, clockwise.
            let r = cx.num(o.get("radius"), 100.0).max(0.0);
            let inner = cx.num(o.get("inner"), 0.0).clamp(0.0, r);
            let (a0, a1) = (cx.num(o.get("start"), 0.0), cx.num(o.get("end"), 90.0));
            let arc_of = |rad: f64, from: f64, to: f64| -> Vec<path::Cubic> {
                let sweep = to - from;
                let n = (sweep.abs() / 90.0).ceil().max(1.0) as usize;
                let step = sweep / n as f64;
                let k = 4.0 / 3.0 * (step.to_radians() / 4.0).tan() * rad;
                let point = |deg: f64| {
                    let a = (deg - 90.0).to_radians();
                    ([at[0] + rad * a.cos(), at[1] + rad * a.sin()], [-a.sin(), a.cos()])
                };
                (0..n)
                    .map(|j| {
                        let (p0, d0) = point(from + step * j as f64);
                        let (p3, d3) = point(from + step * (j + 1) as f64);
                        path::Cubic(p0, [p0[0] + d0[0] * k, p0[1] + d0[1] * k], [p3[0] - d3[0] * k, p3[1] - d3[1] * k], p3)
                    })
                    .collect()
            };
            if (a1 - a0).abs() < 1e-9 {
                return Some(Path::default());
            }
            if kind == "arc" {
                Sub::from_segments(&arc_of(r, a0, a1), false)
            } else {
                // A wedge, or a ring piece with an inner radius.
                let full = (a1 - a0).abs() >= 360.0 - 1e-9;
                let mut segs = arc_of(r, a0, a1);
                if inner > 0.0 {
                    let mut back = arc_of(inner, a1, a0);
                    if full {
                        // A whole ring: two loops, the inner one reversed.
                        let outer = Sub::from_segments(&segs, true);
                        let hole = Sub::from_segments(&back, true);
                        return Some(Path::new(vec![outer, hole]));
                    }
                    let (e, s) = (segs[segs.len() - 1].3, back[0].0);
                    segs.push(path::Cubic(e, e, s, s));
                    segs.append(&mut back);
                    // Back out to the outer edge where it began.
                    let (e, s) = (segs[segs.len() - 1].3, segs[0].0);
                    segs.push(path::Cubic(e, e, s, s));
                } else if !full {
                    let e = segs[segs.len() - 1].3;
                    segs.push(path::Cubic(e, e, at, at));
                    let s = segs[0].0;
                    segs.push(path::Cubic(at, at, s, s));
                }
                Sub::from_segments(&segs, true)
            }
        }
        "path" => {
            if let Some(d) = o.get("d").and_then(Value::as_str) {
                return match path::parse_svg(d) {
                    Ok(p) => Some(p.transform(&path::translate(at[0], at[1]))),
                    Err(e) => {
                        notes.warnings.push(e);
                        None
                    }
                };
            }
            let closed = cx.flag(o.get("closed"), false);
            if let Some(Value::Array(vs)) = o.get("vertices").map(|v| cx.resolve(v)) {
                // [{"p": [x, y], "in": [dx, dy], "out": [dx, dy]}]: handles
                // relative to their point.
                let v = vs
                    .iter()
                    .filter_map(|x| {
                        let p = [x.get("p")?.get(0)?.as_f64()? + at[0], x.get("p")?.get(1)?.as_f64()? + at[1]];
                        let rel = |k: &str| x.get(k).and_then(|h| Some([h.get(0)?.as_f64()?, h.get(1)?.as_f64()?])).unwrap_or([0.0, 0.0]);
                        let (i, out) = (rel("in"), rel("out"));
                        Some(V { p, i: [p[0] + i[0], p[1] + i[1]], o: [p[0] + out[0], p[1] + out[1]] })
                    })
                    .collect();
                Sub { v, closed }
            } else {
                let pts: Vec<[f64; 2]> = cx.pts(o.get("points")).into_iter().map(|p| [p[0] + at[0], p[1] + at[1]]).collect();
                if cx.flag(o.get("smooth"), false) { path::smooth(&pts, closed, cx.num(o.get("tension"), 1.0)) } else { Sub::polyline(&pts, closed) }
            }
        }
        _ => return None,
    };
    let mut p = Path::new(vec![sub]);
    if cx.flag(o.get("reverse"), false) {
        p.subs = p.subs.iter().map(Sub::reversed).collect();
    }
    Some(p)
}

/// Works through a group's items.
fn group(items: &[Value], cx: &mut Ctx, notes: &mut Notes, depth: usize, tol: f64) -> State {
    let mut st = State::default();
    if depth > MAX_DEPTH {
        notes.warnings.push("groups nest too deeply".into());
        return st;
    }
    for it in items {
        let Some(o) = it.as_object() else { continue };
        if o.get("enabled").is_some_and(|e| !cx.flag(Some(e), true)) || o.get("hidden").is_some_and(|h| cx.flag(Some(h), false)) {
            continue;
        }
        let kind = o.get("type").and_then(Value::as_str).unwrap_or("");
        if let Some(p) = shape(o, cx, notes) {
            st.paths.push(p);
            continue;
        }
        match kind {
            "group" => {
                let inner = o.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                let child = group(&inner, cx, notes, depth + 1, tol);
                let m = transform_of(o, cx);
                let opacity = cx.num(o.get("opacity"), 1.0).clamp(0.0, 1.0);
                let blend = o.get("blend").and_then(Value::as_str).map(blend_of);
                if child.painted {
                    st.painted = true;
                    st.draws.extend(child.draws.into_iter().map(|mut d| {
                        d.m = mul(m, d.m);
                        d.alpha *= opacity;
                        if let Some(b) = blend {
                            d.blend = b;
                        }
                        d
                    }));
                } else {
                    st.paths.extend(ops::transform_all(&child.paths, &m));
                }
            }
            "fill" | "stroke" => {
                let opacity = cx.num(o.get("opacity"), 1.0).clamp(0.0, 1.0);
                let ink = ink_of(o, cx, opacity);
                let style = if kind == "fill" {
                    Style::Fill { even_odd: cx.text(o.get("rule"), "nonzero") == "evenodd" }
                } else {
                    let cap = match cx.text(o.get("cap"), "round").as_str() {
                        "butt" | "flat" => LineCap::Butt,
                        "square" => LineCap::Square,
                        _ => LineCap::Round,
                    };
                    let join = match cx.text(o.get("join"), "round").as_str() {
                        "miter" | "mitre" => LineJoin::Miter,
                        "bevel" => LineJoin::Bevel,
                        _ => LineJoin::Round,
                    };
                    Style::Stroke { width: cx.num(o.get("width"), 4.0).max(0.0), cap, join, miter: cx.num(o.get("miter"), 4.0), dash: cx.nums(o.get("dash")), dash_offset: cx.num(o.get("dash_offset"), 0.0) }
                };
                if matches!(style, Style::Stroke { width, .. } if width <= 0.0) {
                    continue;
                }
                let blend = blend_of(&cx.text(o.get("blend"), "normal"));
                st.painted = true;
                // Pushed even when there is nothing to paint yet (a trim at
                // zero), so each paint item keeps its place frame to frame.
                let all = Path { subs: st.paths.iter().flat_map(|p| p.subs.iter().cloned()).collect() };
                st.draws.push(Draw { path: all, ink, style, alpha: 1.0, blend, m: ID });
            }
            "merge" => {
                let mode = match cx.text(o.get("mode"), "union").as_str() {
                    "subtract" | "difference" | "minus" => Merge::Subtract,
                    "intersect" | "intersection" => Merge::Intersect,
                    "exclude" | "xor" | "exclusion" => Merge::Exclude,
                    _ => Merge::Union,
                };
                let merged = ops::merge(&st.paths, mode, tol);
                st.paths = vec![merged];
            }
            "trim" => {
                let together = !matches!(cx.text(o.get("mode"), "together").as_str(), "sequence" | "individually" | "one_line");
                st.paths = ops::trim(&st.paths, cx.num(o.get("start"), 0.0), cx.num(o.get("end"), 1.0), cx.num(o.get("offset"), 0.0), together);
            }
            "offset" => {
                st.paths = vec![ops::offset(&st.paths, cx.num(o.get("amount"), 10.0), &cx.text(o.get("join"), "round"), cx.num(o.get("miter"), 4.0), tol)];
            }
            "wiggle" => {
                let seed = cx.num(o.get("seed"), 0.0) as u32;
                st.paths = ops::wiggle(&st.paths, cx.num(o.get("size"), 10.0), cx.num(o.get("detail"), 3.0), cx.num(o.get("per_second"), 2.0), cx.num(o.get("correlation"), 0.5), seed, cx.t + cx.num(o.get("phase"), 0.0), cx.text(o.get("points"), "smooth") != "corner");
            }
            "zigzag" | "zig_zag" => {
                st.paths = ops::zigzag(&st.paths, cx.num(o.get("size"), 10.0), cx.num(o.get("ridges"), 10.0), cx.text(o.get("points"), "corner") == "smooth");
            }
            "round_corners" | "round" => {
                st.paths = ops::round_corners(&st.paths, cx.num(o.get("radius"), 20.0));
            }
            "pucker" | "bloat" | "pucker_bloat" => {
                st.paths = ops::pucker_bloat(&st.paths, cx.num(o.get("amount"), 0.3));
            }
            "twist" => {
                let c = cx.pt(o.get("centre").or(o.get("center")), ops::centre_of(&st.paths));
                st.paths = ops::twist(&st.paths, cx.num(o.get("angle"), 90.0), c);
            }
            "morph" => {
                let to = o.get("to").map(|t| shapes_in(t, cx, notes)).unwrap_or_default();
                st.paths = ops::morph(&st.paths, &to, cx.num(o.get("amount"), 1.0));
            }
            "transform" => {
                // Moves what is made so far (outlines and paint).
                let m = transform_of(o, cx);
                st.paths = ops::transform_all(&st.paths, &m);
                for d in &mut st.draws {
                    d.m = mul(m, d.m);
                }
            }
            "repeater" | "repeat" => {
                let copies = cx.num(o.get("copies"), 3.0).clamp(0.0, MAX_COPIES as f64);
                let offset = cx.num(o.get("offset"), 0.0);
                let (o0, o1) = (cx.num(o.get("start_opacity"), 1.0), cx.num(o.get("end_opacity"), 1.0));
                let below = cx.text(o.get("composite"), "above") == "below";
                let whole = copies.ceil() as usize;
                let (paths, draws) = (std::mem::take(&mut st.paths), std::mem::take(&mut st.draws));
                let mut order: Vec<usize> = (0..whole).collect();
                if below {
                    order.reverse();
                }
                for k in order {
                    let j = k as f64 + offset;
                    cx.copy = k as f64;
                    let m = power(o, cx, j);
                    let frac = if k as f64 + 1.0 > copies { copies - k as f64 } else { 1.0 };
                    let op = if whole > 1 { o0 + (o1 - o0) * k as f64 / (whole - 1) as f64 } else { o0 } * frac.clamp(0.0, 1.0);
                    st.paths.extend(ops::transform_all(&paths, &m));
                    st.draws.extend(draws.iter().map(|d| Draw { m: mul(m, d.m), alpha: d.alpha * op, ..d.clone() }));
                }
                cx.copy = 0.0;
            }
            "" => notes.warnings.push("an item has no \"type\"".into()),
            other => notes.warnings.push(format!("unknown item type \"{other}\"")),
        }
    }
    st
}

/// The repeater's transform applied `j` times (a fractional `j` blends
/// the last step).
fn power(o: &serde_json::Map<String, Value>, cx: &Ctx, j: f64) -> M {
    let pos = cx.pt(o.get("position"), [100.0, 0.0]);
    let anchor = cx.pt(o.get("anchor"), [0.0, 0.0]);
    let sc = cx.pt(o.get("scale"), [1.0, 1.0]);
    let rot = cx.num(o.get("rotation"), 0.0);
    let step = |f: f64| path::layer([pos[0] * f + anchor[0], pos[1] * f + anchor[1]], anchor, [1.0 + (sc[0] - 1.0) * f, 1.0 + (sc[1] - 1.0) * f], rot * f, 0.0, 0.0);
    let whole = j.trunc();
    let mut m = ID;
    let one = step(1.0);
    let inv = if whole < 0.0 { invert(&one) } else { one };
    for _ in 0..(whole.abs() as usize).min(MAX_COPIES) {
        m = mul(inv, m);
    }
    let f = j - whole;
    if f.abs() > 1e-9 {
        let part = step(f.abs());
        m = mul(if f < 0.0 { invert(&part) } else { part }, m);
    }
    m
}

fn invert(m: &M) -> M {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return ID;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    [a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])]
}

fn colour(c: [f64; 4], alpha: f64) -> Color {
    let ch = |v: f64| v.clamp(0.0, 1.0) as f32;
    Color::rgba(ch(c[0]), ch(c[1]), ch(c[2]), ch(c[3] * alpha))
}

fn paint_of(ink: &Ink, alpha: f64) -> Paint {
    let stops = |s: &[(f64, [f64; 4])]| -> Vec<Stop> { s.iter().map(|(at, c)| Stop::new(*at as f32, colour(*c, alpha))).collect() };
    let pt = |p: [f64; 2]| Point::new(p[0], p[1]);
    match ink {
        Ink::Solid(c) => colour(*c, alpha).into(),
        Ink::Linear { from, to, stops: s } => {
            if s.len() == 1 || path::dist(*from, *to) < 1e-6 {
                return colour(s[0].1, alpha).into();
            }
            Paint::Gradient(std::sync::Arc::new(Gradient::linear(pt(*from), pt(*to), stops(s))))
        }
        Ink::Radial { centre, focal, radius, stops: s } => {
            if s.len() == 1 {
                return colour(s[0].1, alpha).into();
            }
            let mut g = Gradient::radial(pt(*centre), *radius, stops(s));
            g.kind = studio_vector_raster::GradientKind::Radial { c0: pt(*focal), r0: 0.0, c1: pt(*centre), r1: *radius };
            Paint::Gradient(std::sync::Arc::new(g))
        }
    }
}

fn affine(m: &M) -> Affine {
    Affine::new([m[0], m[1], m[2], m[3], m[4], m[5]])
}

/// The document as a scene for Studio's vector rasterizer, in document
/// units. Returns what it could not do as well.
pub fn scene(doc: &Value, cx: &Ctx, scale: f64) -> (Scene, Notes) {
    let mut notes = Notes::default();
    let mut out = Scene::new();
    let Some(items) = items_of(doc) else {
        notes.warnings.push("a shape is {\"items\": [...]} or a list of items".into());
        return (out, notes);
    };
    // Operators flatten finely enough for the picture's scale.
    let tol = 0.3 / scale.max(1e-6);
    let mut cx = cx.clone();
    let st = group(items, &mut cx, &mut notes, 0, tol);
    for d in &st.draws {
        let Some(p) = d.path.to_bez() else { continue };
        let paint = paint_of(&d.ink, 1.0);
        let ts = affine(&d.m);
        match &d.style {
            Style::Fill { even_odd } => {
                out.fill(p, ts, if *even_odd { FillRule::EvenOdd } else { FillRule::NonZero }, paint);
            }
            Style::Stroke { width, cap, join, miter, dash, dash_offset } => {
                let mut style = StrokeStyle::new(*width).with_join(*join).with_caps(*cap);
                style.miter_limit = *miter;
                if dash.len() >= 2 && dash.iter().all(|x| *x >= 0.0) && dash.iter().sum::<f64>() > 0.0 {
                    let mut ds: Vec<f64> = dash.iter().map(|x| x.max(0.01)).collect();
                    if ds.len() % 2 == 1 {
                        ds.extend(ds.clone());
                    }
                    style = style.with_dashes(ds, *dash_offset);
                }
                out.stroke(p, ts, style, paint);
            }
        }
        out.blended(d.blend, d.alpha.clamp(0.0, 1.0) as f32);
    }
    (out, notes)
}

/// One painted outline as it ends up, for exporting: its geometry in
/// document space and its paint.
#[derive(Clone, Debug, PartialEq)]
pub struct Baked {
    pub path: Path,
    /// Straight RGBA 0..1 (a gradient gives its stops' average).
    pub colour: [f64; 4],
    /// Gradient stops when the paint is a gradient: (linear?, from or
    /// centre, to or radius point, stops).
    pub gradient: Option<(bool, [f64; 2], [f64; 2], Vec<(f64, [f64; 4])>)>,
    pub opacity: f64,
    /// `None` for a fill.
    pub stroke: Option<BakedStroke>,
    pub even_odd: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BakedStroke {
    pub width: f64,
    /// 1 butt, 2 round, 3 square (Lottie's numbers).
    pub cap: u8,
    /// 1 miter, 2 round, 3 bevel.
    pub join: u8,
    pub miter: f64,
    pub dash: Vec<f64>,
    pub dash_offset: f64,
}

/// What the document paints at this moment, bottom first.
pub fn baked(doc: &Value, cx: &Ctx) -> Vec<Baked> {
    let mut notes = Notes::default();
    let Some(items) = items_of(doc) else { return Vec::new() };
    let mut cx = cx.clone();
    let st = group(items, &mut cx, &mut notes, 0, 0.25);
    st.draws
        .iter()
        .map(|d| {
            let scale = (d.m[0] * d.m[3] - d.m[1] * d.m[2]).abs().sqrt();
            let path = d.path.transform(&d.m);
            let tp = |p: [f64; 2]| path::apply(&d.m, p);
            let (colour, gradient) = match &d.ink {
                Ink::Solid(c) => (*c, None),
                Ink::Linear { from, to, stops } => (avg(stops), Some((true, tp(*from), tp(*to), stops.clone()))),
                Ink::Radial { centre, radius, stops, .. } => (avg(stops), Some((false, tp(*centre), tp([centre[0] + radius, centre[1]]), stops.clone()))),
            };
            let stroke = match &d.style {
                Style::Fill { .. } => None,
                Style::Stroke { width, cap, join, miter, dash, dash_offset } => Some(BakedStroke {
                    width: width * scale,
                    cap: match cap {
                        LineCap::Butt => 1,
                        LineCap::Round => 2,
                        LineCap::Square => 3,
                    },
                    join: match join {
                        LineJoin::Miter => 1,
                        LineJoin::Round => 2,
                        LineJoin::Bevel => 3,
                    },
                    miter: *miter,
                    dash: dash.iter().map(|x| x * scale).collect(),
                    dash_offset: dash_offset * scale,
                }),
            };
            Baked { path, colour, gradient, opacity: d.alpha, stroke, even_odd: matches!(d.style, Style::Fill { even_odd: true }) }
        })
        .collect()
}

fn avg(stops: &[(f64, [f64; 4])]) -> [f64; 4] {
    let n = stops.len().max(1) as f64;
    let mut c = [0.0; 4];
    for (_, s) in stops {
        for k in 0..4 {
            c[k] += s[k] / n;
        }
    }
    c
}

/// The outlines the document ends with, in document space (painted or
/// not), for text on a path and for measuring.
pub fn outlines(doc: &Value, cx: &Ctx) -> Vec<Path> {
    let mut notes = Notes::default();
    let Some(items) = items_of(doc) else { return Vec::new() };
    let mut cx = cx.clone();
    let st = group(items, &mut cx, &mut notes, 0, 0.25);
    if st.painted { st.draws.iter().map(|d| d.path.transform(&d.m)).collect() } else { st.paths }
}
