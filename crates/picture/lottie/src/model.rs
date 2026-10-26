// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Lottie document read once into layers and shape items, per the
//! lottie-spec 1.0 description (layer types, transforms, shape items,
//! masks and mattes). What a player here cannot draw is named in
//! [`crate::Animation::warnings`].

use crate::value::{PathProp, Prop};
use serde_json::Value;
use std::collections::BTreeSet;

/// A layer or group transform.
#[derive(Clone, Debug, Default)]
pub struct Tf {
    pub anchor: Prop,
    pub position: Prop,
    /// Position given as separate x and y (`"p": {"s": true, ...}`).
    pub split: Option<(Prop, Prop)>,
    pub scale: Prop,
    pub rotation: Prop,
    pub opacity: Prop,
    pub skew: Prop,
    pub skew_axis: Prop,
}

impl Tf {
    pub fn parse(v: Option<&Value>) -> Tf {
        let Some(v) = v else { return Tf::default() };
        let p = v.get("p");
        let split = p.filter(|p| p.get("s").and_then(Value::as_bool) == Some(true)).map(|p| (Prop::parse(p.get("x")), Prop::parse(p.get("y"))));
        Tf {
            anchor: Prop::parse(v.get("a")),
            position: if split.is_some() { Prop::default() } else { Prop::parse(p) },
            split,
            scale: Prop::parse(v.get("s")),
            rotation: Prop::parse(v.get("r").or_else(|| v.get("rz"))),
            opacity: Prop::parse(v.get("o")),
            skew: Prop::parse(v.get("sk")),
            skew_axis: Prop::parse(v.get("sa")),
        }
    }

    /// The matrix at `frame`: translate(p) · rotate(r) · skew · scale(s) ·
    /// translate(-a), as [a, b, c, d, e, f] (x' = a x + c y + e).
    pub fn matrix(&self, frame: f64) -> [f64; 6] {
        let a = self.anchor.pair(frame, [0.0, 0.0]);
        let p = match &self.split {
            Some((x, y)) => [x.num(frame, 0.0), y.num(frame, 0.0)],
            None => self.position.pair(frame, [0.0, 0.0]),
        };
        let s = self.scale.pair(frame, [100.0, 100.0]);
        let r = self.rotation.num(frame, 0.0).to_radians();
        let sk = self.skew.num(frame, 0.0).to_radians();
        let sa = self.skew_axis.num(frame, 0.0).to_radians();
        let mut m = translate(-a[0], -a[1]);
        m = mul(scale(s[0] / 100.0, s[1] / 100.0), m);
        if sk.abs() > 1e-9 {
            // Skew along the axis at `sa`: rotate to it, shear, rotate back.
            m = mul(rotate(-sa), m);
            m = mul([1.0, 0.0, (-sk).tan(), 1.0, 0.0, 0.0], m);
            m = mul(rotate(sa), m);
        }
        m = mul(rotate(r), m);
        mul(translate(p[0], p[1]), m)
    }

    /// 0..1.
    pub fn opacity(&self, frame: f64) -> f64 {
        (self.opacity.num(frame, 100.0) / 100.0).clamp(0.0, 1.0)
    }
}

pub fn translate(x: f64, y: f64) -> [f64; 6] {
    [1.0, 0.0, 0.0, 1.0, x, y]
}
pub fn scale(x: f64, y: f64) -> [f64; 6] {
    [x, 0.0, 0.0, y, 0.0, 0.0]
}
pub fn rotate(r: f64) -> [f64; 6] {
    let (s, c) = r.sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}
/// `a` after `b` (a · b).
pub fn mul(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [a[0] * b[0] + a[2] * b[1], a[1] * b[0] + a[3] * b[1], a[0] * b[2] + a[2] * b[3], a[1] * b[2] + a[3] * b[3], a[0] * b[4] + a[2] * b[5] + a[4], a[1] * b[4] + a[3] * b[5] + a[5]]
}
pub fn apply(m: &[f64; 6], p: [f64; 2]) -> [f64; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

/// A gradient's stops: `{"p": count, "k": [pos, r, g, b, …, pos, a, …]}`.
#[derive(Clone, Debug)]
pub struct Stops {
    pub count: usize,
    pub values: Prop,
}

#[derive(Clone, Debug)]
pub struct Gradient {
    pub radial: bool,
    pub start: Prop,
    pub end: Prop,
    pub stops: Stops,
}

/// One shape item.
#[derive(Clone, Debug)]
pub enum Item {
    Group { items: Vec<Item>, tf: Tf, hidden: bool },
    Rect { position: Prop, size: Prop, roundness: Prop },
    Ellipse { position: Prop, size: Prop },
    Star { polygon: bool, position: Prop, points: Prop, rotation: Prop, outer: Prop, inner: Prop },
    Path(PathProp),
    Fill { colour: Prop, opacity: Prop, even_odd: bool },
    Stroke { colour: Prop, opacity: Prop, width: Prop, cap: u8, join: u8, miter: f64, dash: Vec<(char, Prop)> },
    GradientFill { g: Gradient, opacity: Prop, even_odd: bool },
    GradientStroke { g: Gradient, opacity: Prop, width: Prop, cap: u8, join: u8, miter: f64 },
    Trim { start: Prop, end: Prop, offset: Prop, together: bool },
    Transform(Tf),
}

fn bool_of(v: Option<&Value>) -> bool {
    v.is_some_and(|v| v.as_bool() == Some(true) || v.as_i64() == Some(1))
}

fn gradient(v: &Value) -> Gradient {
    Gradient {
        radial: v.get("t").and_then(Value::as_i64) == Some(2),
        start: Prop::parse(v.get("s")),
        end: Prop::parse(v.get("e")),
        stops: Stops { count: v["g"].get("p").and_then(Value::as_u64).unwrap_or(0) as usize, values: Prop::parse(v["g"].get("k")) },
    }
}

/// Shape items; unknown ones are named in `unsupported`.
pub fn items(v: Option<&Value>, unsupported: &mut BTreeSet<String>) -> Vec<Item> {
    let Some(list) = v.and_then(Value::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    for it in list {
        if bool_of(it.get("hd")) {
            continue;
        }
        let ty = it.get("ty").and_then(Value::as_str).unwrap_or("");
        let p = |k: &str| Prop::parse(it.get(k));
        let cap = it.get("lc").and_then(Value::as_u64).unwrap_or(2) as u8;
        let join = it.get("lj").and_then(Value::as_u64).unwrap_or(2) as u8;
        let miter = it.get("ml").and_then(Value::as_f64).unwrap_or(4.0);
        out.push(match ty {
            "gr" => {
                let mut inner = items(it.get("it"), unsupported);
                // The group's own transform is its `tr` item.
                let pos = inner.iter().rposition(|i| matches!(i, Item::Transform(_)));
                let tf = pos.map(|k| match inner.remove(k) {
                    Item::Transform(t) => t,
                    _ => Tf::default(),
                });
                Item::Group { items: inner, tf: tf.unwrap_or_default(), hidden: false }
            }
            "rc" => Item::Rect { position: p("p"), size: p("s"), roundness: p("r") },
            "el" => Item::Ellipse { position: p("p"), size: p("s") },
            "sr" => Item::Star { polygon: it.get("sy").and_then(Value::as_i64) == Some(2), position: p("p"), points: p("pt"), rotation: p("r"), outer: p("or"), inner: p("ir") },
            "sh" => Item::Path(PathProp::parse(it.get("ks"))),
            "fl" => Item::Fill { colour: p("c"), opacity: p("o"), even_odd: it.get("r").and_then(Value::as_i64) == Some(2) },
            "st" => Item::Stroke {
                colour: p("c"),
                opacity: p("o"),
                width: p("w"),
                cap,
                join,
                miter,
                dash: it.get("d").and_then(Value::as_array).map(|d| d.iter().map(|e| (e.get("n").and_then(Value::as_str).and_then(|s| s.chars().next()).unwrap_or('d'), Prop::parse(e.get("v")))).collect()).unwrap_or_default(),
            },
            "gf" => Item::GradientFill { g: gradient(it), opacity: p("o"), even_odd: it.get("r").and_then(Value::as_i64) == Some(2) },
            "gs" => Item::GradientStroke { g: gradient(it), opacity: p("o"), width: p("w"), cap, join, miter },
            "tm" => Item::Trim { start: p("s"), end: p("e"), offset: p("o"), together: it.get("m").and_then(Value::as_i64) == Some(1) },
            "tr" => Item::Transform(Tf::parse(Some(it))),
            other => {
                unsupported.insert(match other {
                    "rp" => "repeaters".into(),
                    "rd" => "rounded corners".into(),
                    "mm" => "merge paths".into(),
                    "pb" => "pucker and bloat".into(),
                    "tw" => "twist".into(),
                    "zz" => "zig zag".into(),
                    "op" => "offset paths".into(),
                    o => format!("shape item \"{o}\""),
                });
                continue;
            }
        });
    }
    out
}

/// What a layer draws.
#[derive(Clone, Debug)]
pub enum Content {
    Null,
    Solid { colour: [f64; 3], width: f64, height: f64 },
    Image(String),
    Precomp { id: String, width: f64, height: f64, remap: Option<Prop> },
    Shapes(Vec<Item>),
    /// Text and other kinds: nothing drawn.
    Nothing,
}

#[derive(Clone, Debug)]
pub struct Mask {
    /// `a` add, `s` subtract, `i` intersect, `n` none (and others as add).
    pub mode: char,
    pub path: PathProp,
    pub opacity: Prop,
    pub inverted: bool,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub index: Option<i64>,
    pub parent: Option<i64>,
    pub content: Content,
    pub tf: Tf,
    pub in_frame: f64,
    pub out_frame: f64,
    pub start: f64,
    pub stretch: f64,
    pub hidden: bool,
    /// Used as the matte of another layer (not drawn itself).
    pub is_matte: bool,
    /// 1 alpha, 2 inverted alpha, 3 luma, 4 inverted luma.
    pub matte: u8,
    /// The matte's layer index (`tp`), when given; else the layer above.
    pub matte_parent: Option<i64>,
    pub blend: u8,
    pub masks: Vec<Mask>,
}

fn colour_hex(s: &str) -> [f64; 3] {
    let h = s.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f64 / 255.0;
    [c(0), c(2), c(4)]
}

pub fn layers(v: Option<&Value>, unsupported: &mut BTreeSet<String>) -> Vec<Layer> {
    let Some(list) = v.and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .map(|l| {
            let f = |k: &str, d: f64| l.get(k).and_then(Value::as_f64).unwrap_or(d);
            let content = match l.get("ty").and_then(Value::as_i64).unwrap_or(-1) {
                0 => Content::Precomp { id: l.get("refId").and_then(Value::as_str).unwrap_or("").to_owned(), width: f("w", 0.0), height: f("h", 0.0), remap: l.get("tm").map(|t| Prop::parse(Some(t))) },
                1 => Content::Solid { colour: colour_hex(l.get("sc").and_then(Value::as_str).unwrap_or("#000000")), width: f("sw", 0.0), height: f("sh", 0.0) },
                2 => Content::Image(l.get("refId").and_then(Value::as_str).unwrap_or("").to_owned()),
                3 => Content::Null,
                4 => Content::Shapes(items(l.get("shapes"), unsupported)),
                5 => {
                    unsupported.insert("text layers (shown as nothing; convert text to shapes before exporting)".into());
                    Content::Nothing
                }
                _ => Content::Nothing,
            };
            if l.get("ef").and_then(Value::as_array).is_some_and(|e| !e.is_empty()) {
                unsupported.insert("layer effects".into());
            }
            if l.to_string().contains("\"x\":\"") {
                unsupported.insert("expressions".into());
            }
            Layer {
                index: l.get("ind").and_then(Value::as_i64),
                parent: l.get("parent").and_then(Value::as_i64),
                content,
                tf: Tf::parse(l.get("ks")),
                in_frame: f("ip", 0.0),
                out_frame: f("op", f64::INFINITY),
                start: f("st", 0.0),
                stretch: f("sr", 1.0).max(1e-6),
                hidden: bool_of(l.get("hd")),
                is_matte: bool_of(l.get("td")),
                matte: l.get("tt").and_then(Value::as_u64).unwrap_or(0) as u8,
                matte_parent: l.get("tp").and_then(Value::as_i64),
                blend: l.get("bm").and_then(Value::as_u64).unwrap_or(0) as u8,
                masks: l
                    .get("masksProperties")
                    .and_then(Value::as_array)
                    .map(|ms| ms.iter().map(|m| Mask { mode: m.get("mode").and_then(Value::as_str).and_then(|s| s.chars().next()).unwrap_or('a'), path: PathProp::parse(m.get("pt")), opacity: Prop::parse(m.get("o")), inverted: bool_of(m.get("inv")) }).collect())
                    .unwrap_or_default(),
            }
        })
        .collect()
}
