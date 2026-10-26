// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shape documents to and from Lottie shape layers.
//!
//! Out: every frame's painted outlines ([`crate::draw::baked`]) become
//! the shapes of one Lottie shape layer, each outline a group whose path,
//! colour, opacity and width are keyed per frame (held), so trims, morphs,
//! wiggles and repeaters play in any Lottie player exactly as in Studio.
//!
//! In: a Lottie shape layer's groups, shapes, fills, strokes, gradients,
//! trims, repeaters, round corners and merges become a shape document
//! Studio can edit, keyframes included (times in seconds).

use crate::draw::Baked;
use crate::path::{Path, Sub};
use serde_json::{Value, json};

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn bezier(s: &Sub) -> Value {
    let v: Vec<Value> = s.v.iter().map(|q| json!([r3(q.p[0]), r3(q.p[1])])).collect();
    let i: Vec<Value> = s.v.iter().map(|q| json!([r3(q.i[0] - q.p[0]), r3(q.i[1] - q.p[1])])).collect();
    let o: Vec<Value> = s.v.iter().map(|q| json!([r3(q.o[0] - q.p[0]), r3(q.o[1] - q.p[1])])).collect();
    json!({"c": s.closed, "v": v, "i": i, "o": o})
}

/// A property keyed per frame with holds, or static when it never
/// changes. `frames` are (frame, value).
fn held(frames: &[(u64, Value)]) -> Value {
    let mut keys: Vec<(u64, &Value)> = Vec::new();
    for (f, v) in frames {
        if keys.last().is_none_or(|(_, last)| *last != v) {
            keys.push((*f, v));
        }
    }
    match keys.as_slice() {
        [] => json!({"a": 0, "k": 0}),
        [(_, v)] => json!({"a": 0, "k": v}),
        _ => json!({"a": 1, "k": keys.iter().map(|(f, v)| {
            let s = if v.is_array() { (*v).clone() } else { json!([v]) };
            json!({"t": f, "s": s, "h": 1})
        }).collect::<Vec<_>>()}),
    }
}

fn empty_shape() -> Value {
    json!({"c": false, "v": [[0, 0]], "i": [[0, 0]], "o": [[0, 0]]})
}

/// The `shapes` of a Lottie shape layer for frames of baked outlines
/// (bottom first in each frame), in document space (centre origin).
pub fn shapes(frames: &[(u64, Vec<Baked>)]) -> Vec<Value> {
    let count = frames.iter().map(|(_, b)| b.len()).max().unwrap_or(0);
    let mut groups = Vec::new();
    for k in 0..count {
        fn at(f: &(u64, Vec<Baked>), k: usize) -> Option<&Baked> {
            f.1.get(k)
        }
        let subs = frames.iter().filter_map(|f| at(f, k)).map(|b| b.path.subs.len()).max().unwrap_or(0);
        let mut it = Vec::new();
        for s in 0..subs {
            let ks: Vec<(u64, Value)> = frames.iter().map(|f| (f.0, at(f, k).and_then(|b| b.path.subs.get(s)).map(bezier).unwrap_or_else(empty_shape))).collect();
            let mut prop = held(&ks);
            // A path key's value is a one-item list of the bezier.
            if prop["a"] == 1
                && let Some(list) = prop["k"].as_array_mut()
            {
                for key in list {
                    key["s"] = json!([key["s"][0].clone()]);
                }
            }
            it.push(json!({"ty": "sh", "nm": format!("Path {}", s + 1), "ks": prop}));
        }
        let first = frames.iter().find_map(|f| at(f, k));
        let Some(first) = first else { continue };
        let colour: Vec<(u64, Value)> = frames.iter().map(|f| (f.0, at(f, k).map(|b| json!([r3(b.colour[0]), r3(b.colour[1]), r3(b.colour[2]), 1])).unwrap_or(json!([0, 0, 0, 1])))).collect();
        let opacity: Vec<(u64, Value)> = frames.iter().map(|f| (f.0, json!(at(f, k).map(|b| r3(b.opacity * b.colour[3] * 100.0)).unwrap_or(0.0)))).collect();
        let paint = match (&first.stroke, &first.gradient) {
            (None, None) => json!({"ty": "fl", "nm": "Fill", "c": held(&colour), "o": held(&opacity), "r": if first.even_odd { 2 } else { 1 }}),
            (Some(st), None) => {
                let width: Vec<(u64, Value)> = frames.iter().map(|f| (f.0, json!(at(f, k).and_then(|b| b.stroke.as_ref()).map(|s| r3(s.width)).unwrap_or(0.0)))).collect();
                let mut o = json!({"ty": "st", "nm": "Stroke", "c": held(&colour), "o": held(&opacity), "w": held(&width), "lc": st.cap, "lj": st.join, "ml": st.miter});
                if st.dash.len() >= 2 {
                    let mut d = Vec::new();
                    for (n, pair) in st.dash.chunks(2).enumerate() {
                        d.push(json!({"n": "d", "nm": format!("dash{n}"), "v": {"a": 0, "k": r3(pair[0])}}));
                        d.push(json!({"n": "g", "nm": format!("gap{n}"), "v": {"a": 0, "k": r3(*pair.get(1).unwrap_or(&pair[0]))}}));
                    }
                    d.push(json!({"n": "o", "nm": "offset", "v": {"a": 0, "k": r3(st.dash_offset)}}));
                    o["d"] = json!(d);
                }
                o
            }
            (stroke, Some((linear, a, b, stops))) => {
                let mut flat = Vec::new();
                for (at, c) in stops {
                    flat.extend([r3(*at), r3(c[0]), r3(c[1]), r3(c[2])]);
                }
                for (at, c) in stops {
                    flat.extend([r3(*at), r3(c[3])]);
                }
                let mut o = json!({"nm": "Gradient", "o": held(&opacity), "t": if *linear { 1 } else { 2 }, "s": {"a": 0, "k": [r3(a[0]), r3(a[1])]}, "e": {"a": 0, "k": [r3(b[0]), r3(b[1])]}, "g": {"p": stops.len(), "k": {"a": 0, "k": flat}}});
                match stroke {
                    None => {
                        o["ty"] = json!("gf");
                        o["r"] = json!(if first.even_odd { 2 } else { 1 });
                    }
                    Some(st) => {
                        o["ty"] = json!("gs");
                        o["w"] = json!({"a": 0, "k": r3(st.width)});
                        o["lc"] = json!(st.cap);
                        o["lj"] = json!(st.join);
                        o["ml"] = json!(st.miter);
                    }
                }
                o
            }
        };
        it.push(paint);
        it.push(json!({"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}}));
        groups.push(json!({"ty": "gr", "nm": format!("Shape {}", k + 1), "it": it}));
    }
    // Lottie draws its first shape on top.
    groups.reverse();
    groups
}

// ------------------------------------------------------------------ in

/// Whether a key's ease handles make a straight line (linear motion).
fn straight(key: &Value) -> bool {
    let g = |k: &str, c: &str| key.get(k).and_then(|o| o.get(c)).and_then(|v| v.as_array().and_then(|a| a.first()).and_then(Value::as_f64).or(v.as_f64()));
    match (g("o", "x"), g("o", "y"), g("i", "x"), g("i", "y")) {
        (Some(ox), Some(oy), Some(ix), Some(iy)) => (ox - oy).abs() < 0.01 && (ix - iy).abs() < 0.01,
        _ => false,
    }
}

/// A Lottie property as a document value (keys in seconds).
fn prop(p: Option<&Value>, fps: f64, map: &dyn Fn(&Value) -> Value) -> Value {
    let Some(p) = p else { return Value::Null };
    let animated = p.get("a").and_then(Value::as_i64) == Some(1);
    let k = p.get("k").cloned().unwrap_or(Value::Null);
    if !animated {
        return map(&k);
    }
    let Value::Array(keys) = k else { return Value::Null };
    let mut out = Vec::new();
    for (n, key) in keys.iter().enumerate() {
        let Some(t) = key.get("t").and_then(Value::as_f64) else { continue };
        // Old files keep the end value on the key before.
        let s = key.get("s").cloned().or_else(|| n.checked_sub(1).and_then(|p| keys[p].get("e").cloned()));
        let Some(s) = s else { continue };
        let prev_hold = n > 0 && keys[n - 1].get("h").and_then(Value::as_i64) == Some(1);
        let linear = n > 0 && straight(&keys[n - 1]);
        let ease = if prev_hold { "hold" } else if linear { "linear" } else { "smooth" };
        out.push(json!([t / fps, map(&s), ease]));
    }
    json!({"keys": out})
}

fn as_num(v: &Value) -> Value {
    match v {
        Value::Array(a) if a.len() == 1 => a[0].clone(),
        other => other.clone(),
    }
}

fn as_colour(v: &Value) -> Value {
    match v {
        Value::Array(a) if a.len() >= 3 => {
            let c: Vec<f64> = a.iter().take(4).filter_map(Value::as_f64).collect();
            let k = if c.iter().take(3).any(|x| *x > 1.0) { 1.0 / 255.0 } else { 1.0 };
            let h = |x: f64| ((x * k).clamp(0.0, 1.0) * 255.0).round() as u8;
            json!(format!("#{:02x}{:02x}{:02x}", h(c[0]), h(c[1]), h(c[2])))
        }
        other => other.clone(),
    }
}

fn as_point(v: &Value) -> Value {
    match v {
        Value::Array(a) if a.len() >= 2 => json!([a[0], a[1]]),
        other => other.clone(),
    }
}

/// Percent (0..100) to a fraction.
fn pct(v: &Value) -> Value {
    match as_num(v) {
        Value::Number(n) => json!(n.as_f64().unwrap_or(0.0) / 100.0),
        Value::Array(a) => Value::Array(a.iter().map(|x| json!(x.as_f64().unwrap_or(0.0) / 100.0)).collect()),
        other => other,
    }
}

fn vertices(bez: &Value) -> Value {
    let pts = |k: &str| bez.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let (v, i, o) = (pts("v"), pts("i"), pts("o"));
    let list: Vec<Value> = v.iter().enumerate().map(|(n, p)| json!({"p": p, "in": i.get(n).cloned().unwrap_or(json!([0, 0])), "out": o.get(n).cloned().unwrap_or(json!([0, 0]))})).collect();
    Value::Array(list)
}

fn transform_into(tr: &Value, fps: f64, g: &mut serde_json::Map<String, Value>) {
    let set = |g: &mut serde_json::Map<String, Value>, k: &str, v: Value| {
        if !v.is_null() {
            g.insert(k.into(), v);
        }
    };
    set(g, "position", prop(tr.get("p"), fps, &as_point));
    set(g, "anchor", prop(tr.get("a"), fps, &as_point));
    set(g, "scale", prop(tr.get("s"), fps, &|v| match v {
        Value::Array(a) => json!(a.iter().take(2).map(|x| x.as_f64().unwrap_or(100.0) / 100.0).collect::<Vec<_>>()),
        other => pct(other),
    }));
    set(g, "rotation", prop(tr.get("r"), fps, &as_num));
    set(g, "opacity", prop(tr.get("o"), fps, &pct));
    set(g, "skew", prop(tr.get("sk"), fps, &as_num));
    set(g, "skew_axis", prop(tr.get("sa"), fps, &as_num));
}

/// One Lottie group's items as document items, in Studio's order:
/// shapes, then what changes them, then paint (bottom first), then
/// repeaters; groups first, the last one at the bottom.
fn items(list: &[Value], fps: f64, notes: &mut Vec<String>) -> Vec<Value> {
    let (mut groups, mut shapes, mut ops, mut paint, mut repeat) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for it in list {
        if it.get("hd").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let ty = it.get("ty").and_then(Value::as_str).unwrap_or("");
        match ty {
            "gr" => {
                let inner = it.get("it").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut g = serde_json::Map::new();
                g.insert("type".into(), json!("group"));
                if let Some(tr) = inner.iter().find(|x| x.get("ty").and_then(Value::as_str) == Some("tr")) {
                    transform_into(tr, fps, &mut g);
                }
                g.insert("items".into(), Value::Array(items(&inner, fps, notes)));
                groups.push(Value::Object(g));
            }
            "rc" => shapes.push(json!({"type": "rect", "position": prop(it.get("p"), fps, &as_point), "size": prop(it.get("s"), fps, &as_point), "roundness": prop(it.get("r"), fps, &as_num)})),
            "el" => shapes.push(json!({"type": "ellipse", "position": prop(it.get("p"), fps, &as_point), "size": prop(it.get("s"), fps, &as_point)})),
            "sr" => {
                let star = it.get("sy").and_then(Value::as_i64) == Some(1);
                let mut o = json!({"type": if star { "star" } else { "polygon" }, "position": prop(it.get("p"), fps, &as_point), "points": prop(it.get("pt"), fps, &as_num), "radius": prop(it.get("or"), fps, &as_num), "rotation": prop(it.get("r"), fps, &as_num)});
                if star {
                    o["inner"] = prop(it.get("ir"), fps, &as_num);
                }
                shapes.push(o);
            }
            "sh" => {
                let closed = it.pointer("/ks/k/c").and_then(Value::as_bool).or_else(|| it.pointer("/ks/k/0/s/0/c").and_then(Value::as_bool)).unwrap_or(true);
                shapes.push(json!({"type": "path", "closed": closed, "vertices": prop(it.get("ks"), fps, &|v| match v {
                    Value::Array(a) if a.first().is_some_and(Value::is_object) => vertices(&a[0]),
                    other => vertices(other),
                })}));
            }
            "fl" => paint.push(json!({"type": "fill", "colour": prop(it.get("c"), fps, &as_colour), "opacity": prop(it.get("o"), fps, &pct), "rule": if it.get("r").and_then(Value::as_i64) == Some(2) { "evenodd" } else { "nonzero" }})),
            "st" => {
                let cap = match it.get("lc").and_then(Value::as_i64) {
                    Some(1) => "butt",
                    Some(3) => "square",
                    _ => "round",
                };
                let join = match it.get("lj").and_then(Value::as_i64) {
                    Some(1) => "miter",
                    Some(3) => "bevel",
                    _ => "round",
                };
                let mut o = json!({"type": "stroke", "colour": prop(it.get("c"), fps, &as_colour), "opacity": prop(it.get("o"), fps, &pct), "width": prop(it.get("w"), fps, &as_num), "cap": cap, "join": join});
                if let Some(Value::Array(d)) = it.get("d") {
                    let dash: Vec<Value> = d.iter().filter(|x| matches!(x.get("n").and_then(Value::as_str), Some("d" | "g"))).filter_map(|x| x.pointer("/v/k").cloned()).collect();
                    if !dash.is_empty() {
                        o["dash"] = Value::Array(dash);
                    }
                }
                paint.push(o);
            }
            "gf" | "gs" => {
                let n = it.pointer("/g/p").and_then(Value::as_u64).unwrap_or(2) as usize;
                let flat: Vec<f64> = it.pointer("/g/k/k").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                let stops: Vec<Value> = (0..n).filter(|i| flat.len() >= 4 * (i + 1)).map(|i| json!([flat[4 * i], as_colour(&json!([flat[4 * i + 1], flat[4 * i + 2], flat[4 * i + 3]]))])).collect();
                let (a, b) = (prop(it.get("s"), fps, &as_point), prop(it.get("e"), fps, &as_point));
                let gradient = if it.get("t").and_then(Value::as_i64) == Some(2) {
                    let r = match (a.as_array(), b.as_array()) {
                        (Some(x), Some(y)) => ((y[0].as_f64().unwrap_or(0.0) - x[0].as_f64().unwrap_or(0.0)).hypot(y[1].as_f64().unwrap_or(0.0) - x[1].as_f64().unwrap_or(0.0))).max(1.0),
                        _ => 100.0,
                    };
                    json!({"type": "radial", "centre": a, "radius": r, "stops": stops})
                } else {
                    json!({"type": "linear", "from": a, "to": b, "stops": stops})
                };
                let mut o = json!({"type": if ty == "gf" { "fill" } else { "stroke" }, "gradient": gradient, "opacity": prop(it.get("o"), fps, &pct)});
                if ty == "gs" {
                    o["width"] = prop(it.get("w"), fps, &as_num);
                }
                paint.push(o);
            }
            "tm" => ops.push(json!({"type": "trim", "start": prop(it.get("s"), fps, &pct), "end": prop(it.get("e"), fps, &pct), "offset": prop(it.get("o"), fps, &|v| match as_num(v) { Value::Number(n) => json!(n.as_f64().unwrap_or(0.0) / 360.0), o => o }), "mode": if it.get("m").and_then(Value::as_i64) == Some(2) { "sequence" } else { "together" }})),
            "rd" => ops.push(json!({"type": "round_corners", "radius": prop(it.get("r"), fps, &as_num)})),
            "mm" => ops.push(json!({"type": "merge", "mode": match it.get("mm").and_then(Value::as_i64) { Some(3) => "subtract", Some(4) => "intersect", Some(5) => "exclude", _ => "union" }})),
            "pb" => ops.push(json!({"type": "pucker", "amount": prop(it.get("a"), fps, &pct)})),
            "tw" => ops.push(json!({"type": "twist", "angle": prop(it.get("a"), fps, &as_num)})),
            "zz" => ops.push(json!({"type": "zigzag", "size": prop(it.get("s"), fps, &as_num), "ridges": prop(it.get("r"), fps, &as_num), "points": if it.pointer("/pt/k").and_then(Value::as_i64) == Some(2) { "smooth" } else { "corner" }})),
            "op" => ops.push(json!({"type": "offset", "amount": prop(it.get("a"), fps, &as_num), "join": match it.get("lj").and_then(Value::as_i64) { Some(1) => "miter", Some(3) => "bevel", _ => "round" }})),
            "rp" => {
                let tr = it.get("tr").cloned().unwrap_or(Value::Null);
                let mut o = serde_json::Map::new();
                o.insert("type".into(), json!("repeater"));
                o.insert("copies".into(), prop(it.get("c"), fps, &as_num));
                o.insert("offset".into(), prop(it.get("o"), fps, &as_num));
                transform_into(&tr, fps, &mut o);
                o.insert("start_opacity".into(), prop(tr.get("so"), fps, &pct));
                o.insert("end_opacity".into(), prop(tr.get("eo"), fps, &pct));
                o.remove("opacity");
                o.retain(|_, v| !v.is_null());
                if it.get("m").and_then(Value::as_i64) == Some(2) {
                    o.insert("composite".into(), json!("below"));
                }
                repeat.push(Value::Object(o));
            }
            "tr" => {}
            other => {
                let n = format!("shape item \"{other}\"");
                if !notes.contains(&n) {
                    notes.push(n);
                }
            }
        }
    }
    groups.reverse();
    paint.reverse();
    let mut out = groups;
    out.extend(shapes);
    out.extend(ops);
    out.extend(paint);
    out.extend(repeat);
    // Nulls (missing properties) are dropped so defaults apply.
    for it in &mut out {
        if let Value::Object(o) = it {
            o.retain(|_, v| !v.is_null());
        }
    }
    out
}

/// A Lottie shape layer (`ty` 4) as a shape document, its own transform
/// as an outer group, positions moved so the composition's centre is the
/// origin. Returns the document and what it could not bring over.
pub fn import(layer: &Value, fps: f64, comp: [f64; 2]) -> Option<(Value, Vec<String>)> {
    if layer.get("ty").and_then(Value::as_i64) != Some(4) {
        return None;
    }
    let list = layer.get("shapes").and_then(Value::as_array)?;
    let mut notes = Vec::new();
    let inner = items(list, fps, &mut notes);
    let mut g = serde_json::Map::new();
    g.insert("type".into(), json!("group"));
    if let Some(ks) = layer.get("ks") {
        transform_into(ks, fps, &mut g);
    }
    // Layer positions are from the top left of the composition.
    let shift = |v: &mut Value| match v {
        Value::Array(a) if a.len() >= 2 => {
            *v = json!([a[0].as_f64().unwrap_or(0.0) - comp[0] / 2.0, a[1].as_f64().unwrap_or(0.0) - comp[1] / 2.0]);
        }
        _ => {}
    };
    match g.get_mut("position") {
        Some(Value::Object(o)) => {
            if let Some(Value::Array(keys)) = o.get_mut("keys") {
                for k in keys {
                    if let Some(v) = k.get_mut(1) {
                        shift(v);
                    }
                }
            }
        }
        Some(v) => shift(v),
        None => {
            g.insert("position".into(), json!([-comp[0] / 2.0, -comp[1] / 2.0]));
        }
    }
    if layer.get("ks").and_then(|k| k.get("p")).is_some_and(|p| p.get("s").and_then(Value::as_bool) == Some(true)) {
        notes.push("split position (x and y keyed apart) came in still".into());
        if let (Some(x), Some(y)) = (layer.pointer("/ks/p/x/k").and_then(Value::as_f64), layer.pointer("/ks/p/y/k").and_then(Value::as_f64)) {
            g.insert("position".into(), json!([x - comp[0] / 2.0, y - comp[1] / 2.0]));
        }
    }
    g.insert("items".into(), Value::Array(inner));
    Some((json!({"items": [Value::Object(g)]}), notes))
}

/// The paths of one baked frame, for a test or a thumbnail.
pub fn paths(frame: &[Baked]) -> Vec<Path> {
    frame.iter().map(|b| b.path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Ctx;

    #[test]
    fn baked_frames_become_held_lottie_shapes() {
        let doc = json!([{"type": "rect", "size": [200, 100]}, {"type": "trim", "end": {"keys": [[0, 0], [1, 1, "linear"]]}}, {"type": "stroke", "colour": "#ff0000", "width": 8, "dash": [10, 5]}, {"type": "circle", "radius": 30}, {"type": "fill", "colour": "#00ff00"}]);
        let frames: Vec<(u64, Vec<Baked>)> = (0..=10).map(|f| (f, crate::draw::baked(&doc, &Ctx::at(f as f64 / 10.0, 1.0)))).collect();
        let sh = shapes(&frames);
        assert_eq!(sh.len(), 2, "one group per painted outline");
        // The fill is on top, so it comes first.
        let top = &sh[0]["it"];
        assert!(top.as_array().unwrap().iter().any(|i| i["ty"] == "fl"));
        let stroke_group = &sh[1]["it"];
        let path = stroke_group.as_array().unwrap().iter().find(|i| i["ty"] == "sh").unwrap();
        assert_eq!(path["ks"]["a"], 1, "the trimmed path changes over time");
        let st = stroke_group.as_array().unwrap().iter().find(|i| i["ty"] == "st").unwrap();
        assert_eq!(st["d"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn lottie_shape_layers_come_in_editable() {
        let layer = json!({"ty": 4, "ks": {"p": {"a": 0, "k": [960, 540, 0]}, "s": {"a": 0, "k": [100, 100, 100]}},
            "shapes": [{"ty": "gr", "it": [
                {"ty": "rc", "p": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [200, 100]}, "r": {"a": 0, "k": 10}},
                {"ty": "fl", "c": {"a": 0, "k": [1, 0, 0, 1]}, "o": {"a": 0, "k": 100}},
                {"ty": "tm", "s": {"a": 0, "k": 0}, "e": {"a": 1, "k": [{"t": 0, "s": [0]}, {"t": 30, "s": [100]}]}, "o": {"a": 0, "k": 0}},
                {"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}},
            ]}]});
        let (doc, notes) = import(&layer, 30.0, [1920.0, 1080.0]).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let outer = &doc["items"][0];
        assert_eq!(outer["position"], json!([0.0, 0.0]));
        let g = &outer["items"][0]["items"];
        assert_eq!(g[0]["type"], "rect");
        assert_eq!(g[1]["type"], "trim", "operators before paint");
        assert_eq!(g[1]["end"]["keys"][1][0], json!(1.0));
        assert_eq!(g[2]["colour"], "#ff0000");
        assert!(crate::check(&doc).is_empty(), "{:?}", crate::check(&doc));
        let p = crate::render(&doc, &Ctx::at(1.0, 1.0), 192, 108, [1920.0, 1080.0]);
        assert!(p.rgba.chunks_exact(4).filter(|q| q[0] > 200 && q[3] > 200).count() > 50);
    }
}
