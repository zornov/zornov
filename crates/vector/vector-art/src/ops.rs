// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every vector operation as JSON: `{"op": "rect", "x": 10, …}` in, a
//! result out. This is what the AI sends and what the app's tools call,
//! so both can do everything. `{"op": "help"}` lists the operations.
//!
//! Paints are written as a colour (`"#e8573f"`, `"tomato"`… CSS basics),
//! a gradient (`{"linear": {"from": [x, y], "to": [x, y], "stops":
//! ["#f00", "#00f"]}}`, `"radial"` with `"center"`, `"radius"`, optional
//! `"focal"`; `"conic"` with `"center"`, `"angle"`), `{"freeform":
//! [[x, y, "#colour"], …]}`, `{"swatch": "id", "tint": 0.5}`, or the full
//! form documents store. Points are `[x, y]`; rectangles `[x, y, w, h]`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use serde_json::{Value, json};
use studio_vector_geom::{Anchor, AnchorKind, PathData, SubPath};
use studio_vector_raster::{BlendMode, Cap, Color, Extend, FreeformGradient, FreeformPoint, Gradient, GradientKind, Interpolation, Join, Paint, Stop, StrokeAlign, WidthPoint};

use crate::anim::{Binding, Ease};
use crate::appearance::{Appearance, Effect, Fill, GraphicStyle, Item, Stroke};
use crate::build::{self, BuildOptions};
use crate::color::{self, Harmony, Swatch};
use crate::doc::{Artboard, BlendSpec, Document, Envelope, Guide, Node, NodeId, NodeKind, OpacityMask, Perspective, Plane, Repeat, Symbol, WarpKind};
use crate::edit::{self, Align, Pathfinder};
use crate::shapes::LiveShape;
use crate::text::{self, Align as TextAlign, CharProps, CharStyle, ParaStyle, Run, TextKind, TextNode};
use crate::FillRule;

/// Where file paths in operations resolve, and where renders go.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Relative paths (images to place, files to import or export) start here.
    pub base_dir: Option<PathBuf>,
    /// Renders (`look`, `render` without a path) are written here.
    pub out_dir: Option<PathBuf>,
}

impl Context {
    fn path(&self, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        match &self.base_dir {
            Some(b) if pb.is_relative() => b.join(pb),
            _ => pb,
        }
    }

    fn out(&self, name: &str) -> PathBuf {
        match &self.out_dir {
            Some(d) => d.join(name),
            None => self.path(name),
        }
    }

    fn build(&self) -> BuildOptions {
        BuildOptions { base_dir: self.base_dir.clone(), show_hidden: false }
    }
}

type R = Result<Value, String>;

// ------------------------------------------------------------------ reading arguments

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn fd(v: &Value, k: &str, d: f64) -> f64 {
    f(v, k).unwrap_or(d)
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn b(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(Value::as_bool)
}

fn need<'a>(v: &'a Value, k: &str) -> Result<&'a Value, String> {
    v.get(k).ok_or_else(|| format!("missing \"{k}\""))
}

fn point(v: &Value) -> Option<Point> {
    match v {
        Value::Array(a) if a.len() >= 2 => Some(Point::new(a[0].as_f64()?, a[1].as_f64()?)),
        Value::Object(o) => Some(Point::new(o.get("x")?.as_f64()?, o.get("y")?.as_f64()?)),
        _ => None,
    }
}

fn pt(v: &Value, k: &str) -> Option<Point> {
    v.get(k).and_then(point)
}

fn points(v: &Value) -> Vec<Point> {
    v.as_array().map(|a| a.iter().filter_map(point).collect()).unwrap_or_default()
}

/// `[x, y, w, h]`, `{x, y, w, h}` / `{x0, y0, x1, y1}`, or the op's own
/// `x`, `y`, `w`/`width`, `h`/`height`.
fn rect_of(v: &Value) -> Option<Rect> {
    match v {
        Value::Array(a) if a.len() == 4 => {
            let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            (n.len() == 4).then(|| Rect::new(n[0], n[1], n[0] + n[2], n[1] + n[3]))
        }
        Value::Object(o) if o.contains_key("x0") => Some(Rect::new(f(v, "x0")?, f(v, "y0")?, f(v, "x1")?, f(v, "y1")?)),
        Value::Object(_) => {
            let x = fd(v, "x", 0.0);
            let y = fd(v, "y", 0.0);
            let w = f(v, "w").or_else(|| f(v, "width"))?;
            let h = f(v, "h").or_else(|| f(v, "height"))?;
            Some(Rect::new(x, y, x + w, y + h))
        }
        _ => None,
    }
}

fn rect_arg(v: &Value) -> Option<Rect> {
    v.get("rect").and_then(rect_of).or_else(|| rect_of(v))
}

fn id_of(doc: &Document, v: &Value) -> Option<NodeId> {
    match v {
        Value::Number(n) => n.as_u64().filter(|id| doc.find(*id).is_some()),
        Value::String(s) => doc.lookup(s),
        _ => None,
    }
}

fn id_arg(doc: &Document, v: &Value) -> Result<NodeId, String> {
    let raw = v.get("id").or_else(|| v.get("ids").and_then(|a| a.get(0))).ok_or("missing \"id\"")?;
    id_of(doc, raw).ok_or_else(|| format!("no object {raw}"))
}

fn ids_arg(doc: &Document, v: &Value) -> Result<Vec<NodeId>, String> {
    let raw = v.get("ids").or_else(|| v.get("id")).ok_or("missing \"ids\"")?;
    let list: Vec<&Value> = match raw {
        Value::Array(a) => a.iter().collect(),
        Value::String(s) if s == "all" => {
            let mut all = Vec::new();
            for l in &doc.layers {
                for c in l.children().into_iter().flatten() {
                    all.push(c.id);
                }
            }
            return Ok(all);
        }
        other => vec![other],
    };
    list.into_iter().map(|x| id_of(doc, x).ok_or_else(|| format!("no object {x}"))).collect()
}

fn color(v: &Value) -> Result<Color, String> {
    serde_json::from_value(v.clone()).map_err(|_| format!("not a colour: {v}"))
}

fn stops(v: &Value) -> Result<Vec<Stop>, String> {
    let a = v.as_array().ok_or("stops are a list")?;
    let n = a.len();
    let mut out = Vec::with_capacity(n);
    for (i, s) in a.iter().enumerate() {
        let even = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
        out.push(match s {
            Value::String(_) => Stop::new(even, color(s)?),
            Value::Array(p) if p.len() >= 2 => {
                // [offset, colour] or [colour, offset].
                match (p[0].as_f64(), p[1].as_f64()) {
                    (Some(o), _) => Stop::new(o as f32, color(&p[1])?),
                    (None, Some(o)) => Stop::new(o as f32, color(&p[0])?),
                    _ => return Err(format!("bad stop {s}")),
                }
            }
            Value::Object(_) => {
                let mut st = Stop::new(f(s, "offset").or_else(|| f(s, "at")).unwrap_or(even as f64) as f32, color(s.get("color").or_else(|| s.get("colour")).ok_or("a stop needs a colour")?)?);
                if let Some(m) = f(s, "midpoint") {
                    st.midpoint = m as f32;
                }
                st
            }
            _ => return Err(format!("bad stop {s}")),
        });
    }
    Ok(out)
}

fn gradient_common(g: Gradient, spec: &Value) -> Gradient {
    let mut g = g;
    if let Some(e) = s(spec, "extend").or_else(|| s(spec, "spread")) {
        g = g.with_extend(match e {
            "repeat" => Extend::Repeat,
            "reflect" => Extend::Reflect,
            _ => Extend::Pad,
        });
    }
    if let Some(i) = s(spec, "interpolation").or_else(|| s(spec, "space")) {
        g = g.with_interpolation(match i {
            "linear" => Interpolation::Linear,
            "oklab" | "perceptual" => Interpolation::Oklab,
            _ => Interpolation::Srgb,
        });
    }
    g
}

/// A paint from any of the accepted forms.
pub fn paint(v: &Value) -> Result<Paint, String> {
    if let Some(l) = v.get("linear") {
        let g = Gradient::linear(pt(l, "from").ok_or("linear needs \"from\"")?, pt(l, "to").ok_or("linear needs \"to\"")?, stops(need(l, "stops")?)?);
        return Ok(Paint::gradient(gradient_common(g, l)));
    }
    if let Some(r) = v.get("radial") {
        let c = pt(r, "center").or_else(|| pt(r, "centre")).ok_or("radial needs \"center\"")?;
        let rad = f(r, "radius").ok_or("radial needs \"radius\"")?;
        let focal = pt(r, "focal").unwrap_or(c);
        let g = Gradient::new(GradientKind::Radial { c0: focal, r0: fd(r, "focal_radius", 0.0), c1: c, r1: rad }, stops(need(r, "stops")?)?);
        return Ok(Paint::gradient(gradient_common(g, r)));
    }
    if let Some(c) = v.get("conic") {
        let g = Gradient::conic(pt(c, "center").or_else(|| pt(c, "centre")).ok_or("conic needs \"center\"")?, fd(c, "angle", 0.0).to_radians(), stops(need(c, "stops")?)?);
        return Ok(Paint::gradient(gradient_common(g, c)));
    }
    if let Some(ff) = v.get("freeform").and_then(Value::as_array) {
        let mut pts = Vec::new();
        for p in ff {
            let a = p.as_array().ok_or("freeform points are [x, y, colour]")?;
            if a.len() < 3 {
                return Err("freeform points are [x, y, colour]".into());
            }
            pts.push(FreeformPoint { at: Point::new(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)), color: color(&a[2])?, spread: a.get(3).and_then(Value::as_f64).unwrap_or(1.0) as f32 });
        }
        return Ok(Paint::Freeform(Arc::new(FreeformGradient { points: pts, transform: Affine::IDENTITY })));
    }
    serde_json::from_value(v.clone()).map_err(|e| format!("not a paint ({e}): {v}"))
}

fn join_of(s: &str) -> Join {
    match s {
        "round" => Join::Round,
        "bevel" => Join::Bevel,
        _ => Join::Miter,
    }
}

fn cap_of(s: &str) -> Cap {
    match s {
        "round" => Cap::Round,
        "square" | "projecting" => Cap::Square,
        _ => Cap::Butt,
    }
}

/// A fill item from `"fill"` (paint, or `{paint, opacity, blend, swatch}`).
fn fill_item(v: &Value) -> Result<Fill, String> {
    if let Some(sw) = s(v, "swatch") {
        let mut fl = Fill::new(Color::BLACK);
        fl.swatch = Some(sw.to_string());
        fl.tint = fd(v, "tint", 1.0) as f32;
        return Ok(fl);
    }
    let p = if let Some(pp) = v.get("paint") { paint(pp)? } else { paint(v)? };
    let mut fl = Fill::new(p);
    if let Some(o) = f(v, "opacity") {
        fl.opacity = o as f32;
    }
    if let Some(bm) = s(v, "blend").and_then(BlendMode::parse) {
        fl.blend = bm;
    }
    if let Some(e) = v.get("effects") {
        fl.effects = serde_json::from_value(e.clone()).map_err(|e| e.to_string())?;
    }
    Ok(fl)
}

fn stroke_item(v: &Value, op: &Value) -> Result<Stroke, String> {
    let (pv, spec) = if v.get("paint").is_some() || v.get("swatch").is_some() { (v.get("paint").cloned().unwrap_or(json!("#000")), v) } else { (v.clone(), op) };
    let mut st = Stroke::new(paint(&pv)?, 1.0);
    if let Some(sw) = s(spec, "swatch") {
        st.swatch = Some(sw.into());
    }
    st.style.width = f(spec, "width").or_else(|| f(op, "stroke_width")).unwrap_or(1.0);
    if let Some(j) = s(spec, "join").or_else(|| s(op, "join")) {
        st.style.join = join_of(j);
    }
    if let Some(c) = s(spec, "cap").or_else(|| s(op, "cap")) {
        st.style.start_cap = cap_of(c);
        st.style.end_cap = cap_of(c);
    }
    if let Some(m) = f(spec, "miter_limit").or_else(|| f(op, "miter_limit")) {
        st.style.miter_limit = m;
    }
    if let Some(d) = spec.get("dashes").or_else(|| op.get("dashes")).and_then(Value::as_array) {
        st.style.dashes = d.iter().filter_map(Value::as_f64).collect();
    }
    if let Some(o) = f(spec, "dash_offset").or_else(|| f(op, "dash_offset")) {
        st.style.dash_offset = o;
    }
    if let Some(a) = s(spec, "align").or_else(|| s(op, "stroke_align")) {
        st.style.align = match a {
            "inside" => StrokeAlign::Inside,
            "outside" => StrokeAlign::Outside,
            _ => StrokeAlign::Center,
        };
    }
    if let Some(p) = spec.get("profile").or_else(|| op.get("profile")) {
        st.style.profile = profile(p)?;
    }
    if let Some(o) = f(spec, "opacity") {
        st.opacity = o as f32;
    }
    if let Some(bm) = s(spec, "blend").and_then(BlendMode::parse) {
        st.blend = bm;
    }
    if b(spec, "non_scaling") == Some(true) {
        st.style.non_scaling = true;
    }
    Ok(st)
}

fn profile(v: &Value) -> Result<Vec<WidthPoint>, String> {
    match v {
        Value::String(name) => edit::width_preset(name).ok_or_else(|| format!("no width profile \"{name}\" (uniform, taper, taper_start, taper_both, bulge, pinch, brush)")),
        Value::Array(a) => Ok(a
            .iter()
            .filter_map(|p| match p {
                Value::Array(q) if q.len() >= 2 => Some(WidthPoint { at: q[0].as_f64()?, width: q[1].as_f64()? }),
                Value::Object(_) => Some(WidthPoint { at: f(p, "at")?, width: f(p, "width")? }),
                _ => None,
            })
            .collect()),
        _ => Err("a width profile is a name or [[at, width], …]".into()),
    }
}

fn effects(v: &Value) -> Result<Vec<Effect>, String> {
    let list: Vec<Value> = match v {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    list.into_iter().map(|e| serde_json::from_value(e.clone()).map_err(|x| format!("bad effect {e}: {x}"))).collect()
}

/// The appearance an op describes with `fill`, `stroke`, `stroke_width`,
/// `fills`, `strokes`, `effects`, `appearance`.
fn appearance_of(v: &Value, default_fill: Option<Paint>) -> Result<Option<Appearance>, String> {
    if let Some(a) = v.get("appearance") {
        return serde_json::from_value(a.clone()).map(Some).map_err(|e| e.to_string());
    }
    let mut ap = Appearance::default();
    let mut any = false;
    if let Some(fs) = v.get("fills").and_then(Value::as_array) {
        for x in fs {
            ap.items.push(Item::Fill(fill_item(x)?));
        }
        any = true;
    } else if let Some(fl) = v.get("fill") {
        if !fl.is_null() && fl.as_str() != Some("none") {
            ap.items.push(Item::Fill(fill_item(fl)?));
        }
        any = true;
    } else if let Some(p) = default_fill {
        ap.items.push(Item::Fill(Fill::new(p)));
    }
    if let Some(ss) = v.get("strokes").and_then(Value::as_array) {
        for x in ss {
            ap.items.push(Item::Stroke(stroke_item(x, v)?));
        }
        any = true;
    } else if let Some(sk) = v.get("stroke") {
        if !sk.is_null() && sk.as_str() != Some("none") {
            ap.items.push(Item::Stroke(stroke_item(sk, v)?));
        }
        any = true;
    }
    if let Some(e) = v.get("effects") {
        ap.effects = effects(e)?;
        any = true;
    }
    Ok((any || !ap.items.is_empty()).then_some(ap))
}

/// Applies the common node fields an op may carry: name, opacity, blend,
/// rotation (about the centre), transform, visible, locked.
fn decorate(n: &mut Node, v: &Value) {
    if let Some(name) = s(v, "name") {
        n.name = name.to_string();
    }
    if let Some(o) = f(v, "opacity") {
        n.opacity = o.clamp(0.0, 1.0) as f32;
    }
    if let Some(bm) = s(v, "blend").and_then(BlendMode::parse) {
        n.blend = bm;
    }
    if let Some(t) = v.get("transform").and_then(|t| serde_json::from_value::<Affine>(t.clone()).ok()) {
        n.transform = t;
    }
    if let Some(r) = f(v, "rotate").or_else(|| f(v, "rotation")) {
        if let Some(c) = build::node_bounds(n).map(|b| b.center()) {
            n.transform = Affine::translate(c.to_vec2()) * Affine::rotate(r.to_radians()) * Affine::translate(-c.to_vec2()) * n.transform;
        }
    }
    if let Some(vis) = b(v, "visible") {
        n.visible = vis;
    }
    if let Some(l) = b(v, "locked") {
        n.locked = l;
    }
    if let Some(w) = f(v, "wrap") {
        n.wrap = Some(w);
    }
    if let Some(st) = s(v, "style") {
        n.style = Some(st.to_string());
    }
}

fn parent_arg(doc: &Document, v: &Value) -> Option<NodeId> {
    v.get("parent").or_else(|| v.get("layer")).and_then(|p| id_of(doc, p))
}

fn add_node(doc: &mut Document, v: &Value, kind: NodeKind, default_fill: Option<Paint>) -> R {
    let mut n = Node::new(0, kind);
    if let Some(ap) = appearance_of(v, default_fill)? {
        n.appearance = ap;
    }
    decorate(&mut n, v);
    let parent = parent_arg(doc, v);
    let id = doc.add(parent, n);
    Ok(json!({ "id": id, "bounds": bounds_json(build::world_bounds(doc, id)) }))
}

fn bounds_json(b: Option<Rect>) -> Value {
    match b {
        Some(r) => json!([round(r.x0), round(r.y0), round(r.width()), round(r.height())]),
        None => Value::Null,
    }
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn default_paint() -> Option<Paint> {
    Some(Paint::solid(Color::from_hex("#2b2b2b").unwrap_or(Color::BLACK)))
}

/// A path from an op: SVG `d`, `points` (+ `closed`, `smooth`), or `anchors`.
fn path_of(v: &Value) -> Result<PathData, String> {
    if let Some(d) = s(v, "d") {
        let bp = BezPath::from_svg(d).map_err(|e| format!("bad path data: {e:?}"))?;
        return Ok(PathData::from_bezpath(&bp));
    }
    if let Some(a) = v.get("anchors").and_then(Value::as_array) {
        let pts: Vec<edit::PenPoint> = a
            .iter()
            .filter_map(|x| match x {
                Value::Array(_) => point(x).map(|p| edit::PenPoint { at: p, out: None, h_in: None }),
                _ => serde_json::from_value::<edit::PenPoint>(json!({"at": x.get("at").cloned().unwrap_or(Value::Null), "out": x.get("out").cloned(), "in": x.get("in").cloned()})).ok(),
            })
            .collect();
        return Ok(edit::pen(&pts, b(v, "closed").unwrap_or(false)));
    }
    if let Some(p) = v.get("points") {
        let pts = points(p);
        if pts.len() < 2 {
            return Err("a path needs two or more points".into());
        }
        let closed = b(v, "closed").unwrap_or(false);
        if b(v, "smooth") == Some(true) {
            return Ok(edit::curvature(&pts, closed, &[]));
        }
        return Ok(PathData::single(SubPath::polyline(&pts, closed)));
    }
    Err("give the path as \"d\" (SVG path data), \"points\" or \"anchors\"".into())
}

// ------------------------------------------------------------------ summaries

fn node_summary(doc: &Document, n: &Node, depth: usize, out: &mut Vec<Value>) {
    let mut o = json!({ "id": n.id, "type": n.kind_name(), "bounds": bounds_json(build::world_bounds(doc, n.id)) });
    if !n.name.is_empty() {
        o["name"] = json!(n.name);
    }
    if depth > 0 {
        o["depth"] = json!(depth);
    }
    if let Some(f) = n.appearance.top_fill() {
        o["fill"] = paint_summary(&f.paint);
    }
    if let Some(s) = n.appearance.top_stroke() {
        o["stroke"] = json!(format!("{} {}", paint_summary(&s.paint).as_str().unwrap_or("paint"), s.style.width));
    }
    if n.appearance.items.len() > 2 {
        o["appearance_items"] = json!(n.appearance.items.len());
    }
    if !n.appearance.effects.is_empty() {
        o["effects"] = json!(n.appearance.effects.iter().map(|e| serde_json::to_value(e).ok().and_then(|v| v.get("type").cloned()).unwrap_or(Value::Null)).collect::<Vec<_>>());
    }
    if let NodeKind::Text(t) = &n.kind {
        o["text"] = json!(t.plain());
    }
    if let NodeKind::Shape { shape } = &n.kind {
        o["shape"] = json!(shape.kind());
    }
    if !n.visible {
        o["hidden"] = json!(true);
    }
    if n.opacity < 1.0 {
        o["opacity"] = json!(n.opacity);
    }
    out.push(o);
    if let Some(c) = n.children() {
        for k in c {
            node_summary(doc, k, depth + 1, out);
        }
    }
}

fn paint_summary(p: &Paint) -> Value {
    match p {
        Paint::Solid { color } => json!(color.to_hex()),
        Paint::Gradient(g) => json!(format!("{} gradient {}", match g.kind { GradientKind::Linear { .. } => "linear", GradientKind::Radial { .. } => "radial", GradientKind::Conic { .. } => "conic" }, g.stops.iter().map(|s| s.color.to_hex()).collect::<Vec<_>>().join("→"))),
        Paint::Freeform(_) => json!("freeform gradient"),
        Paint::Mesh(_) => json!("mesh gradient"),
        Paint::Image(_) => json!("image"),
        Paint::Pattern(_) => json!("pattern"),
    }
}

/// The document's outline as JSON: artboards, objects (id, type, bounds,
/// paint), swatches and styles.
pub fn summary(doc: &Document) -> Value {
    let mut objects = Vec::new();
    for l in &doc.layers {
        node_summary(doc, l, 0, &mut objects);
    }
    json!({
        "name": doc.name,
        "artboards": doc.artboards.iter().map(|a| json!({"id": a.id, "name": a.name, "rect": [a.rect.x0, a.rect.y0, a.rect.width(), a.rect.height()], "background": a.background.map(|c| c.to_hex())})).collect::<Vec<_>>(),
        "objects": objects,
        "swatches": doc.swatches.iter().map(|s| json!({"id": s.id, "name": s.name, "color": s.color.to_hex(), "global": s.global})).collect::<Vec<_>>(),
        "styles": doc.styles.iter().map(|s| json!({"id": s.id, "name": s.name})).collect::<Vec<_>>(),
        "symbols": doc.symbols.iter().map(|s| json!({"id": s.id, "name": s.name})).collect::<Vec<_>>(),
        "animated": !doc.animation.tracks.is_empty(),
    })
}

// ------------------------------------------------------------------ checks

/// Design checks the AI reads after drawing: art off every artboard, text
/// too small or with poor contrast, hairlines that vanish when printed,
/// and empty groups.
pub fn check(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    let boards: Vec<Rect> = doc.artboards.iter().map(|a| a.rect).collect();
    let bg = doc.artboards.first().and_then(|a| a.background).unwrap_or(Color::WHITE);
    doc.walk(&mut |n| {
        let label = if n.name.is_empty() { format!("#{} ({})", n.id, n.kind_name()) } else { format!("#{} \"{}\"", n.id, n.name) };
        if matches!(n.kind, NodeKind::Group { layer: true, .. }) {
            return;
        }
        if let Some(b) = build::world_bounds(doc, n.id)
            && !boards.iter().any(|a| a.intersect(b).area() > 0.0)
            && n.visible
        {
            out.push(format!("{label} is outside every artboard"));
        }
        if let NodeKind::Text(t) = &n.kind {
            let size = t.char.size.unwrap_or(24.0);
            if size < 6.0 {
                out.push(format!("{label}: {size}-unit type is too small to read"));
            }
            if let Some(Paint::Solid { color }) = n.appearance.top_fill().map(|f| &f.paint) {
                let c = color::contrast(*color, bg);
                if c < 3.0 {
                    out.push(format!("{label}: text contrast {c:.1}:1 against the artboard (aim for 4.5:1)"));
                }
            }
        }
        if let Some(s) = n.appearance.top_stroke()
            && s.style.width < 0.25
            && !s.style.non_scaling
        {
            out.push(format!("{label}: a {}-unit stroke disappears in print", s.style.width));
        }
        if let Some(c) = n.children()
            && c.is_empty()
            && !matches!(n.kind, NodeKind::Group { layer: true, .. })
        {
            out.push(format!("{label} is empty"));
        }
    });
    out
}

// ------------------------------------------------------------------ the operations

/// The operations, for `help`.
pub const OPS: &[(&str, &str)] = &[
    ("summary", "the document outline: artboards, objects with ids, bounds and paints, swatches, styles"),
    ("get", "id → the object's full JSON"),
    ("set", "id, path (JSON pointer into the object, e.g. /appearance/items/0/paint) and value; or props: {…} merged into it"),
    ("artboard", "add one: rect [x,y,w,h], name, background"),
    ("artboard_set", "artboard (name/id/index), rect, name, background"),
    ("layer", "add a layer: name"),
    ("rect", "x, y, w, h (or rect), radius (also rx, corner_radius) or radii [tl,tr,br,bl], corners (round/inverted_round/chamfer ×4)"),
    ("ellipse", "cx, cy, rx, ry (or rect)"),
    ("circle", "cx, cy, r"),
    ("polygon", "cx, cy, r, sides, rotation (degrees)"),
    ("triangle", "cx, cy, r, rotation (degrees; 0 points up): a polygon with three sides"),
    ("star", "cx, cy, r, inner, points, rotation"),
    ("spiral", "cx, cy, r, decay (%), segments, clockwise"),
    ("arc", "from, to, slope (-100..100), closed"),
    ("line", "from, to"),
    ("grid", "rect, rows, cols"),
    ("polar_grid", "rect, rings, spokes"),
    ("path", "d (SVG path data) | points [[x,y],…] (+ closed, smooth) | anchors [{at,in,out}] (the pen)"),
    ("curve", "points, closed, corners [indices]: the curvature tool"),
    ("pencil", "points, closed, fidelity 0..1: freehand fitted with few curves"),
    ("shaper", "points: a rough drawing becomes a clean rectangle, ellipse, polygon or line"),
    ("text", "x, y, text, size, font, weight, italic, align, tracking, leading, features [\"liga\",\"smcp\",\"-kern\"], case, fill…"),
    ("area_text", "rect, text, columns, gutter, align (left/center/right/justify), size, font…; flows around objects with wrap"),
    ("path_text", "text, path (d or points) or along (an object id), offset, flip, size…"),
    ("type_style", "id, char {…} and/or para {…} properties; run (index) to style one run"),
    ("char_style", "id, name, and character properties: a named style"),
    ("para_style", "id, name, align, indents, spacing, char_style"),
    ("glyphs", "font, limit → the characters it has (the glyph panel)"),
    ("font_features", "font → the OpenType features it offers"),
    ("fonts", "the font families available"),
    ("image", "file, x, y, w, h (placed, linked or embed: true)"),
    ("trace", "id (an image object) or file, preset (6 colors, 16 colors, black and white logo, line art, high fidelity photo, flat logo, …), rect"),
    ("style", "ids, fill, stroke, stroke_width, join, cap, dashes, dash_offset, profile, opacity, blend: sets the top fill/stroke"),
    ("appearance", "id, appearance {items: [{type: fill|stroke, …}], effects: [...]} replaces the stack"),
    ("add_fill", "id, fill (paint or {paint, opacity, blend}): adds a fill on top"),
    ("add_stroke", "id, stroke, width, …: adds a stroke on top"),
    ("effect", "ids, effect {type: drop_shadow|inner_shadow|outer_glow|inner_glow|blur|offset|roughen|pucker_bloat|zig_zag|twist|round_corners|transform|warp, …}"),
    ("clear_effects", "ids"),
    ("width_profile", "id, profile (taper, taper_both, bulge, pinch, brush, uniform or [[at,width],…])"),
    ("swatch", "id, name, color, global"),
    ("harmony", "base colour, rule (complementary, split_complementary, analogous, triadic, tetradic, square, monochromatic, shades, tints), count, swatches (add them)"),
    ("recolor", "map {\"#old\": \"#new\"}, ids (default all)"),
    ("contrast", "a, b → contrast ratio"),
    ("save_style", "name, from (an object): a graphic style"),
    ("apply_style", "ids, style"),
    ("pathfinder", "ids, op: unite, minus_front, minus_back, intersect, exclude, divide, trim, merge, crop, outline"),
    ("shape_builder", "ids, merge [[x,y],…] (regions to join), delete [[x,y],…]"),
    ("live_paint", "ids, fills [{at: [x,y], paint}]: faces of the overlapping outlines, painted"),
    ("offset_path", "id, amount, join"),
    ("outline_stroke", "id"),
    ("simplify", "id, tolerance"),
    ("smooth", "id, amount 0..1"),
    ("add_anchors", "id"),
    ("reverse", "id"),
    ("join", "ids, tolerance: open paths joined end to end"),
    ("compound", "ids, rule: one compound path (holes by even-odd or non-zero)"),
    ("anchor", "id, subpath, index, at, in, out, kind (corner/smooth): move an anchor or its handles"),
    ("insert_anchor", "id, subpath, segment, t"),
    ("delete_anchor", "id, subpath, index"),
    ("scissors", "id, at [x,y]"),
    ("knife", "ids, line [[x,y],…]"),
    ("eraser", "ids, line, width"),
    ("blob", "points, width, color: blob brush (merges with touching same-colour shapes)"),
    ("expand", "id: live shapes, type, effects, appearance, symbols, repeats, blends, envelopes → plain paths"),
    ("outline_text", "id: type → paths"),
    ("delete", "ids"),
    ("duplicate", "ids, dx, dy"),
    ("group", "ids, name"),
    ("ungroup", "id"),
    ("clip", "ids: the frontmost clips the rest"),
    ("mask", "id, art (object ids to use as the opacity mask), alpha, invert"),
    ("arrange", "id, how: front, back, forward, backward"),
    ("move_to", "ids, parent, index"),
    ("transform", "ids, translate [dx,dy], rotate (deg), scale (s or [sx,sy]), skew [x,y] deg, origin ([x,y] or center): about each object's centre unless origin"),
    ("place", "id, x, y, anchor (top_left, center…): moves the object's box there"),
    ("resize", "id, w, h (keep_ratio)"),
    ("align", "ids, how (left, center, right, top, middle, bottom), to (artboard or an object id)"),
    ("distribute", "ids, axis (horizontal/vertical), spacing"),
    ("repeat", "ids, kind (radial: count, radius, center, rotate | grid: rows, cols, gap [x,y], offset_rows | mirror: at, angle)"),
    ("blend", "ids [a, b], steps (0 = smooth), spine (path)"),
    ("envelope", "ids, warp (arc, arc_lower, arc_upper, arch, bulge, shell_lower, shell_upper, flag, wave, fish, rise, fisheye, inflate, squeeze, twist), bend, horizontal, vertical | mesh {rows, cols, points} | free [4 corners] | shape (an object id)"),
    ("perspective_grid", "left_vp, right_vp, origin, cell"),
    ("perspective", "ids, plane (left, right, floor): art drawn on a plane of the grid"),
    ("symbol", "ids, name: makes a symbol and replaces the art with an instance"),
    ("instance", "symbol, x, y, scale, rotate"),
    ("guide", "x or y"),
    ("grid_settings", "spacing, subdivisions, snap, visible"),
    ("snap", "point → the point snapped to grid, guides and art"),
    ("chart", "kind (bar, column, line, pie, donut), data [n…] or [[label, n]…], rect, colors, labels, title, font: an infographic chart as vector art"),
    ("morph", "id (the shape that changes), to (an object id or name, an op that makes one such as {\"op\": \"star\", …}, {\"text\": \"Hi\", \"size\": 200}, or path data), t, duration, ease, param (a clip parameter drives the amount instead of keys), keep_target (leave the target showing). Any outline into any other (circle into triangle, logo into word): points matched, corners kept, the colour following"),
    ("key", "id, property (JSON pointer, or @move/@rotate/@scale), t (seconds), value, ease (linear, ease_in, ease_out, ease_in_out, hold, spring, [x1,y1,x2,y2])"),
    ("bind", "id, property, param: the clip parameter drives it"),
    ("animation", "duration, looping"),
    ("clear_keys", "id, property"),
    ("check", "design checks: off-artboard art, small or low-contrast text, hairlines, empty groups"),
    ("look", "artboard, width, file: renders a PNG to look at (returns its path)"),
    ("render", "file (.png), artboard, scale or width"),
    ("export", "file (.svg, .pdf, .eps, .png, .svart), artboard, scale"),
    ("import", "file (.svg, .pdf, .eps, .svart, or an image): its art added as a group"),
    ("batch", "ops [...]: several operations in order"),
];

/// Applies one operation; returns its result.
pub fn apply(doc: &mut Document, v: &Value, ctx: &Context) -> R {
    let op = s(v, "op").or_else(|| s(v, "type")).ok_or("missing \"op\"")?;
    match op {
        "help" => Ok(json!(OPS.iter().map(|(k, d)| json!({"op": k, "args": d})).collect::<Vec<_>>())),
        "summary" | "list" | "tree" => Ok(summary(doc)),
        "get" => {
            let id = id_arg(doc, v)?;
            Ok(serde_json::to_value(doc.find(id)).unwrap_or(Value::Null))
        }
        "set" => {
            let id = id_arg(doc, v)?;
            let n = doc.find(id).ok_or("unknown object")?;
            let mut j = serde_json::to_value(n).map_err(|e| e.to_string())?;
            if let Some(props) = v.get("props") {
                merge(&mut j, props);
            }
            if let Some(p) = s(v, "path") {
                let val = need(v, "value")?.clone();
                let val = if p.ends_with("/paint") || p.ends_with("/color") { serde_json::to_value(paint_or_color(p, &val)?).map_err(|e| e.to_string())? } else { val };
                *j.pointer_mut(p).ok_or_else(|| format!("no {p} in #{id}"))? = val;
            }
            let new: Node = serde_json::from_value(j).map_err(|e| format!("that makes an invalid object: {e}"))?;
            if let Some(slot) = doc.find_mut(id) {
                *slot = new;
            }
            Ok(json!({"id": id}))
        }
        "artboard" => {
            let r = rect_arg(v).ok_or("artboard needs rect [x, y, w, h]")?;
            let id = doc.new_id();
            let name = s(v, "name").map(String::from).unwrap_or_else(|| format!("Artboard {}", doc.artboards.len() + 1));
            let bg = match v.get("background") {
                Some(Value::Null) => None,
                Some(c) => Some(color(c)?),
                None => Some(Color::WHITE),
            };
            doc.artboards.push(Artboard { id, name, rect: r, background: bg });
            Ok(json!({"id": id}))
        }
        "artboard_set" => {
            let key = v.get("artboard").map(|a| a.as_str().map(String::from).unwrap_or_else(|| a.to_string()));
            let i = match key {
                Some(k) => doc.artboards.iter().position(|a| a.name == k || a.id.to_string() == k).or_else(|| k.parse::<usize>().ok().map(|i| i.saturating_sub(1))),
                None => Some(0),
            }
            .filter(|i| *i < doc.artboards.len())
            .ok_or("no such artboard")?;
            let a = &mut doc.artboards[i];
            if let Some(r) = rect_arg(v) {
                a.rect = r;
            }
            if let Some(n) = s(v, "name") {
                a.name = n.into();
            }
            match v.get("background") {
                Some(Value::Null) => a.background = None,
                Some(c) => a.background = Some(color(c)?),
                None => {}
            }
            Ok(json!({"id": a.id}))
        }
        "layer" => {
            let id = doc.new_id();
            let mut l = Node::new(id, NodeKind::Group { children: Vec::new(), clip: false, layer: true });
            l.name = s(v, "name").map(String::from).unwrap_or_else(|| format!("Layer {}", doc.layers.len() + 1));
            doc.layers.push(l);
            Ok(json!({"id": id}))
        }
        "rect" | "rectangle" => {
            let r = rect_arg(v).ok_or("rect needs x, y, w, h")?;
            let radii = match v.get("radii").and_then(Value::as_array) {
                Some(a) if a.len() == 4 => [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0)),
                // SVG's rx and the obvious names round the corners too
                // (they were ignored, leaving square corners).
                _ => [f(v, "radius").or_else(|| f(v, "rx")).or_else(|| f(v, "ry")).or_else(|| f(v, "corner_radius")).or_else(|| f(v, "rounded")).unwrap_or(0.0); 4],
            };
            let corners = v.get("corners").and_then(|c| match c {
                Value::String(k) => serde_json::from_value(json!(k.replace("inverted_round", "invertedRound"))).ok().map(|k| [k; 4]),
                other => serde_json::from_value(other.clone()).ok(),
            });
            add_node(doc, v, NodeKind::Shape { shape: LiveShape::Rect { rect: r, radii, corners } }, default_paint())
        }
        "ellipse" => {
            let r = match (f(v, "cx"), f(v, "cy")) {
                (Some(cx), Some(cy)) => {
                    let (rx, ry) = (fd(v, "rx", 50.0), f(v, "ry").unwrap_or(fd(v, "rx", 50.0)));
                    Rect::new(cx - rx, cy - ry, cx + rx, cy + ry)
                }
                _ => rect_arg(v).ok_or("ellipse needs cx, cy, rx, ry or a rect")?,
            };
            add_node(doc, v, NodeKind::Shape { shape: LiveShape::Ellipse { rect: r } }, default_paint())
        }
        "circle" => {
            let (cx, cy, r) = (fd(v, "cx", 0.0), fd(v, "cy", 0.0), f(v, "r").or_else(|| f(v, "radius")).ok_or("circle needs r")?);
            add_node(doc, v, NodeKind::Shape { shape: LiveShape::Ellipse { rect: Rect::new(cx - r, cy - r, cx + r, cy + r) } }, default_paint())
        }
        "pie" => {
            let r = match (f(v, "cx"), f(v, "cy")) {
                (Some(cx), Some(cy)) => {
                    let rr = fd(v, "r", 50.0);
                    Rect::new(cx - rr, cy - rr, cx + rr, cy + rr)
                }
                _ => rect_arg(v).ok_or("pie needs cx, cy, r or a rect")?,
            };
            add_node(doc, v, NodeKind::Shape { shape: LiveShape::Pie { rect: r, start: fd(v, "start", 0.0), end: fd(v, "end", 90.0) } }, default_paint())
        }
        "polygon" | "triangle" => {
            // The shape keeps its own turn (editable later); the node is not
            // turned again on top of it.
            let sides = if op == "triangle" { 3.0 } else { fd(v, "sides", 6.0) };
            let rotation = f(v, "rotation").or_else(|| f(v, "rotate")).unwrap_or(0.0);
            let mut plain = v.clone();
            if let Some(o) = plain.as_object_mut() {
                o.remove("rotation");
                o.remove("rotate");
            }
            let r = f(v, "r").or_else(|| f(v, "radius")).unwrap_or(50.0);
            add_node(doc, &plain, NodeKind::Shape { shape: LiveShape::Polygon { center: Point::new(fd(v, "cx", 0.0), fd(v, "cy", 0.0)), radius: r, sides: sides as u32, rotation } }, default_paint())
        }
        "star" => {
            let c = Point::new(fd(v, "cx", 0.0), fd(v, "cy", 0.0));
            let r = fd(v, "r", 50.0);
            let n = fd(v, "points", 5.0) as u32;
            let shape = match f(v, "inner") {
                Some(i) => LiveShape::Star { center: c, radius: r, inner: i, points: n, rotation: fd(v, "rotation", 0.0) },
                None => LiveShape::straight_star(c, r, n),
            };
            add_node(doc, v, NodeKind::Shape { shape }, default_paint())
        }
        "spiral" => add_node(
            doc,
            v,
            NodeKind::Shape {
                shape: LiveShape::Spiral { center: Point::new(fd(v, "cx", 0.0), fd(v, "cy", 0.0)), radius: fd(v, "r", 50.0), decay: fd(v, "decay", 80.0), segments: fd(v, "segments", 10.0) as u32, clockwise: b(v, "clockwise").unwrap_or(true) },
            },
            None,
        )
        .and_then(|r| with_default_stroke(doc, r)),
        "arc" => add_node(
            doc,
            v,
            NodeKind::Shape { shape: LiveShape::Arc { from: pt(v, "from").ok_or("arc needs from")?, to: pt(v, "to").ok_or("arc needs to")?, slope: fd(v, "slope", 50.0), closed: b(v, "closed").unwrap_or(false) } },
            None,
        )
        .and_then(|r| with_default_stroke(doc, r)),
        "line" => add_node(doc, v, NodeKind::Shape { shape: LiveShape::Line { from: pt(v, "from").ok_or("line needs from")?, to: pt(v, "to").ok_or("line needs to")? } }, None).and_then(|r| with_default_stroke(doc, r)),
        "grid" => {
            let r = rect_arg(v).ok_or("grid needs a rect")?;
            add_node(doc, v, NodeKind::Shape { shape: LiveShape::Grid { rect: r, rows: fd(v, "rows", 4.0) as u32, cols: fd(v, "cols", 4.0) as u32, skew: (0.0, 0.0) } }, None).and_then(|r| with_default_stroke(doc, r))
        }
        "polar_grid" => {
            let r = rect_arg(v).ok_or("polar_grid needs a rect")?;
            add_node(doc, v, NodeKind::Shape { shape: LiveShape::PolarGrid { rect: r, rings: fd(v, "rings", 4.0) as u32, spokes: fd(v, "spokes", 8.0) as u32 } }, None).and_then(|r| with_default_stroke(doc, r))
        }
        "path" | "pen" => {
            let p = path_of(v)?;
            let rule = if s(v, "rule") == Some("evenodd") || s(v, "rule") == Some("even_odd") { FillRule::EvenOdd } else { FillRule::NonZero };
            let closed = p.is_closed();
            add_node(doc, v, NodeKind::Path { path: p, rule }, if closed { default_paint() } else { None }).and_then(|r| if closed { Ok(r) } else { with_default_stroke(doc, r) })
        }
        "curve" | "curvature" => {
            let pts = points(need(v, "points")?);
            let corners: Vec<usize> = v.get("corners").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_u64().map(|u| u as usize)).collect()).unwrap_or_default();
            let closed = b(v, "closed").unwrap_or(false);
            add_node(doc, v, NodeKind::Path { path: edit::curvature(&pts, closed, &corners), rule: FillRule::NonZero }, if closed { default_paint() } else { None }).and_then(|r| if closed { Ok(r) } else { with_default_stroke(doc, r) })
        }
        "pencil" => {
            let pts = points(need(v, "points")?);
            let closed = b(v, "closed").unwrap_or(false);
            add_node(doc, v, NodeKind::Path { path: edit::pencil(&pts, closed, fd(v, "fidelity", 0.5)), rule: FillRule::NonZero }, if closed { default_paint() } else { None }).and_then(|r| if closed { Ok(r) } else { with_default_stroke(doc, r) })
        }
        "shaper" => {
            let pts = points(need(v, "points")?);
            let shape = edit::recognize(&pts).ok_or("that drawing is not a shape the shaper knows")?;
            let open = shape.is_open();
            add_node(doc, v, NodeKind::Shape { shape }, if open { None } else { default_paint() }).and_then(|r| if open { with_default_stroke(doc, r) } else { Ok(r) })
        }
        "text" | "point_text" => {
            let t = text_node(v, TextKind::Point { at: Point::new(fd(v, "x", 0.0), fd(v, "y", 0.0)) })?;
            add_node(doc, v, NodeKind::Text(t), Some(Paint::solid(Color::BLACK)))
        }
        "area_text" => {
            let r = rect_arg(v).ok_or("area_text needs a rect")?;
            let t = text_node(v, TextKind::Area { rect: r, columns: fd(v, "columns", 1.0) as u32, gutter: fd(v, "gutter", 12.0) })?;
            add_node(doc, v, NodeKind::Text(t), Some(Paint::solid(Color::BLACK)))
        }
        "path_text" => {
            let path = if let Some(a) = v.get("along").and_then(|a| id_of(doc, a)) {
                let n = doc.find(a).ok_or("unknown path")?;
                let m = doc.world_transform(a).unwrap_or(Affine::IDENTITY);
                PathData::new(build::geometry(doc, n).into_iter().flat_map(|p| p.path.transformed(m).subpaths).collect())
            } else {
                path_of(v)?
            };
            let t = text_node(v, TextKind::OnPath { path, offset: fd(v, "offset", 0.0), flip: b(v, "flip").unwrap_or(false) })?;
            add_node(doc, v, NodeKind::Text(t), Some(Paint::solid(Color::BLACK)))
        }
        "type_style" => {
            let id = id_arg(doc, v)?;
            let n = doc.find_mut(id).ok_or("unknown object")?;
            let NodeKind::Text(t) = &mut n.kind else { return Err("not type".into()) };
            if let Some(c) = v.get("char") {
                let p: CharProps = serde_json::from_value(c.clone()).map_err(|e| e.to_string())?;
                match v.get("run").and_then(Value::as_u64) {
                    Some(i) => {
                        let r = t.runs.get_mut(i as usize).ok_or("no such run")?;
                        r.props = p.over(&r.props);
                    }
                    None => t.char = p.over(&t.char),
                }
            }
            if let Some(p) = v.get("para") {
                let p: crate::text::ParaProps = serde_json::from_value(p.clone()).map_err(|e| e.to_string())?;
                t.para = p.over(&t.para);
            }
            if let Some(ps) = s(v, "para_style") {
                t.para_style = Some(ps.into());
            }
            if let Some(txt) = s(v, "text") {
                t.runs = vec![Run { text: txt.into(), style: None, props: CharProps::default() }];
            }
            Ok(json!({"id": id}))
        }
        "char_style" => {
            let id = s(v, "id").or_else(|| s(v, "name")).ok_or("a style needs an id")?.to_string();
            let props: CharProps = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            doc.char_styles.retain(|c| c.id != id);
            doc.char_styles.push(CharStyle { id: id.clone(), name: s(v, "name").unwrap_or(&id).into(), props });
            Ok(json!({"id": id}))
        }
        "para_style" => {
            let id = s(v, "id").or_else(|| s(v, "name")).ok_or("a style needs an id")?.to_string();
            let props: crate::text::ParaProps = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            doc.para_styles.retain(|c| c.id != id);
            doc.para_styles.push(ParaStyle { id: id.clone(), name: s(v, "name").unwrap_or(&id).into(), props, char_style: s(v, "char_style").map(String::from) });
            Ok(json!({"id": id}))
        }
        "glyphs" => Ok(json!(text::glyphs_of(s(v, "font").unwrap_or("Helvetica"), fd(v, "limit", 400.0) as usize).into_iter().map(|(c, g)| json!([c.to_string(), g])).collect::<Vec<_>>())),
        "font_features" => Ok(json!(text::features_of(s(v, "font").unwrap_or("Helvetica")))),
        "fonts" => Ok(json!(text::fonts().families())),
        "image" => {
            let file = s(v, "file").or_else(|| s(v, "path")).ok_or("image needs a file")?;
            let p = ctx.path(file);
            let img = crate::interop::load_image(&p).ok_or_else(|| format!("{}: not an image", p.display()))?;
            let (iw, ih) = (img.width as f64, img.height as f64);
            let w = f(v, "w").or_else(|| f(v, "width")).unwrap_or_else(|| f(v, "h").map(|h| h * iw / ih).unwrap_or(iw));
            let h = f(v, "h").or_else(|| f(v, "height")).unwrap_or(w * ih / iw);
            let embed = b(v, "embed").unwrap_or(true);
            let node = crate::doc::ImageNode { image: embed.then_some(img), href: (!embed).then(|| file.to_string()), width: w, height: h };
            let mut n = Node::new(0, NodeKind::Image(node));
            n.transform = Affine::translate((fd(v, "x", 0.0), fd(v, "y", 0.0)));
            decorate(&mut n, v);
            let id = doc.add(parent_arg(doc, v), n);
            Ok(json!({"id": id, "bounds": bounds_json(build::world_bounds(doc, id))}))
        }
        "trace" => {
            let preset = s(v, "preset").unwrap_or("6 colors");
            if let Some(id) = v.get("id").and_then(|x| id_of(doc, x)) {
                let n = doc.find(id).cloned().ok_or("unknown object")?;
                let NodeKind::Image(im) = &n.kind else { return Err("trace an image object".into()) };
                let img = im.image.clone().or_else(|| im.href.as_ref().and_then(|h| crate::interop::load_image(&ctx.path(h)))).ok_or("the image's pixels are missing")?;
                let world = doc.world_transform(id).unwrap_or(Affine::IDENTITY);
                // Traced in the image's own box, then placed where the image was.
                let g = edit::trace(doc, &img, preset, Rect::new(0.0, 0.0, im.width, im.height), Some(id))?;
                if let Some(node) = doc.find_mut(g) {
                    node.transform = node.transform * world;
                }
                return Ok(json!({"id": g, "shapes": doc.find(g).and_then(Node::children).map(Vec::len)}));
            }
            let file = s(v, "file").ok_or("trace needs an image id or file")?;
            let img = crate::interop::load_image(&ctx.path(file)).ok_or("not an image")?;
            let r = rect_arg(v).unwrap_or(Rect::new(0.0, 0.0, img.width as f64, img.height as f64));
            let g = edit::trace(doc, &img, preset, r, None)?;
            Ok(json!({"id": g, "shapes": doc.find(g).and_then(Node::children).map(Vec::len)}))
        }
        "style" => {
            let ids = ids_arg(doc, v)?;
            for id in &ids {
                let n = doc.find_mut(*id).ok_or("unknown object")?;
                restyle(n, v)?;
            }
            Ok(json!({"ids": ids}))
        }
        "appearance" => {
            let id = id_arg(doc, v)?;
            let ap: Appearance = serde_json::from_value(need(v, "appearance")?.clone()).map_err(|e| e.to_string())?;
            doc.find_mut(id).ok_or("unknown object")?.appearance = ap;
            Ok(json!({"id": id}))
        }
        "add_fill" => {
            let id = id_arg(doc, v)?;
            let fl = fill_item(need(v, "fill")?)?;
            doc.find_mut(id).ok_or("unknown object")?.appearance.items.push(Item::Fill(fl));
            Ok(json!({"id": id}))
        }
        "add_stroke" => {
            let id = id_arg(doc, v)?;
            let st = stroke_item(need(v, "stroke")?, v)?;
            doc.find_mut(id).ok_or("unknown object")?.appearance.items.push(Item::Stroke(st));
            Ok(json!({"id": id}))
        }
        "effect" => {
            let ids = ids_arg(doc, v)?;
            let es = effects(need(v, "effect").or_else(|_| need(v, "effects"))?)?;
            for id in &ids {
                doc.find_mut(*id).ok_or("unknown object")?.appearance.effects.extend(es.iter().cloned());
            }
            Ok(json!({"ids": ids}))
        }
        "clear_effects" => {
            let ids = ids_arg(doc, v)?;
            for id in &ids {
                doc.find_mut(*id).ok_or("unknown object")?.appearance.effects.clear();
            }
            Ok(json!({"ids": ids}))
        }
        "width_profile" => {
            let id = id_arg(doc, v)?;
            edit::set_width_profile(doc, id, profile(need(v, "profile")?)?)?;
            Ok(json!({"id": id}))
        }
        "swatch" => {
            let c = color(need(v, "color").or_else(|_| need(v, "colour"))?)?;
            let name = s(v, "name").unwrap_or("Swatch").to_string();
            let id = s(v, "id").map(String::from).unwrap_or_else(|| name.to_ascii_lowercase().replace(' ', "_"));
            doc.set_swatch(Swatch { id: id.clone(), name, color: c, global: b(v, "global").unwrap_or(true), group: s(v, "group").unwrap_or("").into() });
            Ok(json!({"id": id}))
        }
        "harmony" => {
            let base = color(need(v, "base")?)?;
            let rule = Harmony::parse(s(v, "rule").unwrap_or("complementary")).ok_or("unknown harmony rule")?;
            let cs = color::harmony(base, rule, fd(v, "count", 5.0) as usize);
            if b(v, "swatches") == Some(true) {
                for (i, c) in cs.iter().enumerate() {
                    doc.set_swatch(Swatch { id: format!("harmony_{i}"), name: format!("Harmony {}", i + 1), color: *c, global: true, group: "Harmony".into() });
                }
            }
            Ok(json!(cs.iter().map(|c| c.to_hex()).collect::<Vec<_>>()))
        }
        "recolor" => {
            let map = need(v, "map")?.as_object().ok_or("map is {\"#old\": \"#new\"}")?;
            let pairs: Vec<(String, Color)> = map.iter().filter_map(|(k, val)| Some((Color::from_hex(k)?.to_hex(), color(val).ok()?))).collect();
            let ids = if v.get("ids").is_some() { ids_arg(doc, v)? } else { doc.layers.iter().map(|l| l.id).collect() };
            let mut changed = 0;
            for id in ids {
                if let Some(n) = doc.find_mut(id) {
                    recolor(n, &pairs, &mut changed);
                }
            }
            Ok(json!({"changed": changed}))
        }
        "contrast" => Ok(json!(round(color::contrast(color(need(v, "a")?)?, color(need(v, "b")?)?)))),
        "save_style" => {
            let from = id_of(doc, need(v, "from")?).ok_or("unknown object")?;
            let ap = doc.find(from).map(|n| n.appearance.clone()).unwrap_or_default();
            let name = s(v, "name").unwrap_or("Style").to_string();
            let id = s(v, "id").map(String::from).unwrap_or_else(|| name.to_ascii_lowercase().replace(' ', "_"));
            doc.styles.retain(|st| st.id != id);
            doc.styles.push(GraphicStyle { id: id.clone(), name, appearance: ap });
            Ok(json!({"id": id}))
        }
        "apply_style" => {
            let ids = ids_arg(doc, v)?;
            let st = s(v, "style").ok_or("which style?")?;
            let ap = doc.styles.iter().find(|x| x.id == st || x.name == st).map(|x| x.appearance.clone()).ok_or("no such style")?;
            for id in &ids {
                let n = doc.find_mut(*id).ok_or("unknown object")?;
                n.appearance = ap.clone();
                n.style = None;
            }
            Ok(json!({"ids": ids}))
        }
        "pathfinder" | "boolean" => {
            let ids = ids_arg(doc, v)?;
            let p = Pathfinder::parse(s(v, "op2").or_else(|| s(v, "mode")).or_else(|| s(v, "operation")).unwrap_or("unite")).ok_or("unknown pathfinder operation")?;
            let out = edit::pathfinder(doc, &ids, p)?;
            Ok(json!({"ids": out}))
        }
        "shape_builder" => {
            let ids = ids_arg(doc, v)?;
            let out = edit::shape_builder(doc, &ids, &points(v.get("merge").unwrap_or(&json!([]))), &points(v.get("delete").unwrap_or(&json!([]))))?;
            Ok(json!({"ids": out}))
        }
        "live_paint" => {
            let ids = ids_arg(doc, v)?;
            let mut fills = Vec::new();
            for x in v.get("fills").and_then(Value::as_array).into_iter().flatten() {
                fills.push((pt(x, "at").ok_or("each fill needs at")?, paint(need(x, "paint")?)?));
            }
            let g = edit::live_paint(doc, &ids, &fills)?;
            Ok(json!({"id": g, "faces": doc.find(g).and_then(Node::children).map(Vec::len)}))
        }
        "offset_path" => {
            let id = id_arg(doc, v)?;
            let n = edit::offset(doc, id, fd(v, "amount", 10.0), join_of(s(v, "join").unwrap_or("miter")))?;
            Ok(json!({"id": n}))
        }
        "outline_stroke" => Ok(json!({"id": edit::outline_stroke(doc, id_arg(doc, v)?)?})),
        "simplify" => {
            let id = id_arg(doc, v)?;
            let tol = fd(v, "tolerance", 1.0);
            edit::edit_path(doc, id, |p| edit::simplify(p, tol))?;
            Ok(json!({"id": id, "anchors": anchors(doc, id)}))
        }
        "smooth" => {
            let id = id_arg(doc, v)?;
            let a = fd(v, "amount", 0.5);
            edit::edit_path(doc, id, |p| edit::smooth(p, a))?;
            Ok(json!({"id": id}))
        }
        "add_anchors" => {
            let id = id_arg(doc, v)?;
            edit::edit_path(doc, id, studio_vector_pathops::add_anchor_points)?;
            Ok(json!({"id": id, "anchors": anchors(doc, id)}))
        }
        "reverse" => {
            let id = id_arg(doc, v)?;
            edit::edit_path(doc, id, |p| {
                let mut q = p.clone();
                q.reverse();
                q
            })?;
            Ok(json!({"id": id}))
        }
        "join" => {
            let ids = ids_arg(doc, v)?;
            let mut paths = Vec::new();
            for id in &ids {
                let n = doc.find(*id).ok_or("unknown object")?;
                let m = doc.world_transform(*id).unwrap_or(Affine::IDENTITY);
                for p in build::geometry(doc, n) {
                    paths.push(p.path.transformed(m));
                }
            }
            let joined = studio_vector_pathops::join(&paths, fd(v, "tolerance", 1.0));
            let look = doc.find(ids[0]).map(|n| n.appearance.clone()).unwrap_or_default();
            for id in &ids {
                doc.remove(*id);
            }
            let mut n = Node::new(0, NodeKind::Path { path: joined, rule: FillRule::NonZero });
            n.appearance = look;
            Ok(json!({"id": doc.add(None, n)}))
        }
        "compound" => {
            let ids = edit::back_to_front(doc, &ids_arg(doc, v)?);
            let mut subs = Vec::new();
            for id in &ids {
                let n = doc.find(*id).ok_or("unknown object")?;
                let m = doc.world_transform(*id).unwrap_or(Affine::IDENTITY);
                for p in build::geometry(doc, n) {
                    subs.extend(p.path.transformed(m).subpaths);
                }
            }
            let look = ids.last().and_then(|i| doc.find(*i)).map(|n| n.appearance.clone()).unwrap_or_default();
            for id in &ids {
                doc.remove(*id);
            }
            let rule = if s(v, "rule") == Some("nonzero") { FillRule::NonZero } else { FillRule::EvenOdd };
            let mut n = Node::new(0, NodeKind::Path { path: PathData::new(subs), rule });
            n.appearance = look;
            Ok(json!({"id": doc.add(None, n)}))
        }
        "anchor" => {
            let id = id_arg(doc, v)?;
            let (si, ai) = (fd(v, "subpath", 0.0) as usize, fd(v, "index", 0.0) as usize);
            let (at, hin, hout, kind) = (pt(v, "at"), pt(v, "in"), pt(v, "out"), s(v, "kind").map(String::from));
            edit::edit_path(doc, id, |p| {
                let mut q = p.clone();
                if let Some(a) = q.anchor_mut(si, ai) {
                    if let Some(at) = at {
                        let d = at - a.p;
                        a.translate(d);
                    }
                    if let Some(h) = hin {
                        a.h_in = h;
                    }
                    if let Some(h) = hout {
                        a.h_out = h;
                    }
                    match kind.as_deref() {
                        Some("corner") => a.kind = AnchorKind::Corner,
                        Some("smooth") => a.kind = AnchorKind::Smooth,
                        _ => {}
                    }
                }
                if kind.as_deref() == Some("smooth") && hin.is_none() && hout.is_none()
                    && let Some(sp) = q.subpaths.get_mut(si)
                {
                    sp.smooth_anchor(ai);
                }
                if kind.as_deref() == Some("corner") && hin.is_none() && hout.is_none()
                    && let Some(a) = q.anchor_mut(si, ai)
                {
                    *a = Anchor::corner(a.p);
                }
                q
            })?;
            Ok(json!({"id": id}))
        }
        "insert_anchor" => {
            let id = id_arg(doc, v)?;
            let (si, seg, t) = (fd(v, "subpath", 0.0) as usize, fd(v, "segment", 0.0) as usize, fd(v, "t", 0.5));
            edit::edit_path(doc, id, |p| {
                let mut q = p.clone();
                if let Some(sp) = q.subpaths.get_mut(si)
                    && seg < sp.segment_count()
                {
                    sp.insert_anchor(seg, t);
                }
                q
            })?;
            Ok(json!({"id": id, "anchors": anchors(doc, id)}))
        }
        "delete_anchor" => {
            let id = id_arg(doc, v)?;
            let (si, ai) = (fd(v, "subpath", 0.0) as usize, fd(v, "index", 0.0) as usize);
            edit::edit_path(doc, id, |p| {
                let mut q = p.clone();
                if let Some(sp) = q.subpaths.get_mut(si) {
                    studio_vector_pathops::remove_anchor(sp, ai);
                }
                q
            })?;
            Ok(json!({"id": id, "anchors": anchors(doc, id)}))
        }
        "scissors" => {
            let id = id_arg(doc, v)?;
            Ok(json!({"ids": edit::scissors(doc, id, pt(v, "at").ok_or("scissors needs at")?)?}))
        }
        "knife" => {
            let ids = ids_arg(doc, v)?;
            Ok(json!({"ids": edit::knife(doc, &ids, &points(need(v, "line")?))?}))
        }
        "eraser" => {
            let ids = ids_arg(doc, v)?;
            Ok(json!({"ids": edit::eraser(doc, &ids, &points(need(v, "line")?), fd(v, "width", 10.0))?}))
        }
        "blob" | "blob_brush" => {
            let c = color(v.get("color").or_else(|| v.get("fill")).unwrap_or(&json!("#000000")))?;
            Ok(json!({"id": edit::blob_brush(doc, &points(need(v, "points")?), fd(v, "width", 10.0), c, parent_arg(doc, v))?}))
        }
        "expand" => Ok(json!({"id": edit::expand(doc, id_arg(doc, v)?)?})),
        "outline_text" | "create_outlines" => Ok(json!({"id": edit::outline_text(doc, id_arg(doc, v)?)?})),
        "delete" | "remove" => {
            let ids = ids_arg(doc, v)?;
            for id in &ids {
                doc.remove(*id);
            }
            Ok(json!({"deleted": ids}))
        }
        "duplicate" => {
            let ids = ids_arg(doc, v)?;
            let (dx, dy) = (fd(v, "dx", 10.0), fd(v, "dy", 10.0));
            let mut out = Vec::new();
            for id in ids {
                let mut n = doc.find(id).cloned().ok_or("unknown object")?;
                let parent = doc.parent_of(id);
                let mut one = vec![n.clone()];
                doc.reid(&mut one);
                n = one.remove(0);
                n.transform = Affine::translate((dx, dy)) * n.transform;
                out.push(doc.add(parent, n));
            }
            Ok(json!({"ids": out}))
        }
        "group" => {
            let ids = ids_arg(doc, v)?;
            let g = edit::group(doc, &ids)?;
            if let (Some(name), Some(n)) = (s(v, "name"), doc.find_mut(g)) {
                n.name = name.into();
            }
            Ok(json!({"id": g}))
        }
        "ungroup" => Ok(json!({"ids": edit::ungroup(doc, id_arg(doc, v)?)?})),
        "clip" | "clipping_mask" => Ok(json!({"id": edit::clip(doc, &ids_arg(doc, v)?)?})),
        "mask" => {
            let id = id_arg(doc, v)?;
            let art_ids = match v.get("art") {
                Some(Value::Array(a)) => a.iter().filter_map(|x| id_of(doc, x)).collect::<Vec<_>>(),
                Some(x) => id_of(doc, x).into_iter().collect(),
                None => Vec::new(),
            };
            let mut art = Vec::new();
            for a in art_ids {
                let world = doc.world_transform(a).unwrap_or(Affine::IDENTITY);
                let target = doc.world_transform(id).unwrap_or(Affine::IDENTITY);
                if let Some(mut n) = doc.remove(a) {
                    // The mask art lives in the masked object's space.
                    n.transform = target.inverse() * world;
                    art.push(n);
                }
            }
            doc.find_mut(id).ok_or("unknown object")?.mask = Some(Box::new(OpacityMask { art, alpha: b(v, "alpha").unwrap_or(false), invert: b(v, "invert").unwrap_or(false) }));
            Ok(json!({"id": id}))
        }
        "arrange" => {
            let id = id_arg(doc, v)?;
            edit::arrange(doc, id, s(v, "how").unwrap_or("front"))?;
            Ok(json!({"id": id}))
        }
        "move_to" => {
            let ids = ids_arg(doc, v)?;
            let parent = parent_arg(doc, v).ok_or("move_to needs a parent")?;
            for id in &ids {
                let world = doc.world_transform(*id).unwrap_or(Affine::IDENTITY);
                let pm = doc.world_transform(parent).unwrap_or(Affine::IDENTITY);
                if let Some(mut n) = doc.remove(*id) {
                    n.transform = pm.inverse() * world;
                    let list = doc.find_mut(parent).and_then(Node::children_mut).ok_or("that is not a group or layer")?;
                    let i = f(v, "index").map(|i| i as usize).unwrap_or(list.len()).min(list.len());
                    list.insert(i, n);
                }
            }
            Ok(json!({"ids": ids}))
        }
        "transform" => {
            let ids = ids_arg(doc, v)?;
            for id in &ids {
                let b0 = build::world_bounds(doc, *id).unwrap_or(Rect::ZERO);
                let origin = match v.get("origin") {
                    Some(Value::Array(_)) => pt(v, "origin").unwrap_or(b0.center()),
                    _ => b0.center(),
                };
                let mut m = Affine::IDENTITY;
                if let Some(sv) = v.get("scale") {
                    let (sx, sy) = match sv {
                        Value::Array(a) if a.len() >= 2 => (a[0].as_f64().unwrap_or(1.0), a[1].as_f64().unwrap_or(1.0)),
                        other => (other.as_f64().unwrap_or(1.0), other.as_f64().unwrap_or(1.0)),
                    };
                    m = Affine::scale_non_uniform(sx, sy) * m;
                }
                if let Some(sk) = v.get("skew").and_then(point) {
                    m = Affine::skew(sk.x.to_radians().tan(), sk.y.to_radians().tan()) * m;
                }
                if let Some(r) = f(v, "rotate") {
                    m = Affine::rotate(r.to_radians()) * m;
                }
                if b(v, "flip_x") == Some(true) {
                    m = Affine::scale_non_uniform(-1.0, 1.0) * m;
                }
                if b(v, "flip_y") == Some(true) {
                    m = Affine::scale_non_uniform(1.0, -1.0) * m;
                }
                let about = Affine::translate(origin.to_vec2()) * m * Affine::translate(-origin.to_vec2());
                let t = pt(v, "translate").map(|p| p.to_vec2()).unwrap_or(Vec2::ZERO);
                let world = Affine::translate(t) * about;
                apply_world(doc, *id, world);
            }
            Ok(json!({"ids": ids}))
        }
        "place" | "move" => {
            let id = id_arg(doc, v)?;
            let bx = build::world_bounds(doc, id).ok_or("it has no size")?;
            let anchor = s(v, "anchor").unwrap_or("top_left");
            let at = Point::new(fd(v, "x", bx.x0), fd(v, "y", bx.y0));
            let cur = match anchor {
                "center" | "centre" => bx.center(),
                "top_right" => Point::new(bx.x1, bx.y0),
                "bottom_left" => Point::new(bx.x0, bx.y1),
                "bottom_right" => Point::new(bx.x1, bx.y1),
                "top" => Point::new(bx.center().x, bx.y0),
                "bottom" => Point::new(bx.center().x, bx.y1),
                _ => Point::new(bx.x0, bx.y0),
            };
            apply_world(doc, id, Affine::translate(at - cur));
            Ok(json!({"id": id, "bounds": bounds_json(build::world_bounds(doc, id))}))
        }
        "resize" => {
            let id = id_arg(doc, v)?;
            let bx = build::world_bounds(doc, id).ok_or("it has no size")?;
            let (mut sx, mut sy) = (f(v, "w").or_else(|| f(v, "width")).map(|w| w / bx.width().max(1e-9)), f(v, "h").or_else(|| f(v, "height")).map(|h| h / bx.height().max(1e-9)));
            if b(v, "keep_ratio").unwrap_or(sx.is_none() || sy.is_none()) {
                let k = sx.or(sy).unwrap_or(1.0);
                sx = Some(k);
                sy = Some(k);
            }
            let o = Point::new(bx.x0, bx.y0);
            let m = Affine::translate(o.to_vec2()) * Affine::scale_non_uniform(sx.unwrap_or(1.0), sy.unwrap_or(1.0)) * Affine::translate(-o.to_vec2());
            apply_world(doc, id, m);
            Ok(json!({"id": id, "bounds": bounds_json(build::world_bounds(doc, id))}))
        }
        "align" => {
            let ids = ids_arg(doc, v)?;
            let how = Align::parse(s(v, "how").unwrap_or("center")).ok_or("align how?")?;
            let to = match v.get("to") {
                Some(Value::String(k)) if k == "artboard" => doc.artboards.first().map(|a| a.rect),
                Some(x) => id_of(doc, x).and_then(|i| build::world_bounds(doc, i)).or_else(|| x.as_str().and_then(|k| doc.artboard(Some(k))).map(|a| a.rect)),
                None => None,
            };
            edit::align(doc, &ids, how, to)?;
            Ok(json!({"ids": ids}))
        }
        "distribute" => {
            let ids = ids_arg(doc, v)?;
            edit::distribute(doc, &ids, s(v, "axis").unwrap_or("horizontal") != "vertical", f(v, "spacing"))?;
            Ok(json!({"ids": ids}))
        }
        "repeat" => {
            let ids = ids_arg(doc, v)?;
            let rep = match s(v, "kind").unwrap_or("radial") {
                "grid" => Repeat::Grid { rows: fd(v, "rows", 3.0) as u32, cols: fd(v, "cols", 3.0) as u32, gap: pt(v, "gap").map(|p| (p.x, p.y)).unwrap_or((fd(v, "gap", 10.0), fd(v, "gap", 10.0))), offset_rows: b(v, "offset_rows").unwrap_or(false) },
                "mirror" => Repeat::Mirror { at: pt(v, "at"), angle: fd(v, "angle", 0.0) },
                _ => Repeat::Radial { count: fd(v, "count", 8.0) as u32, center: pt(v, "center"), radius: fd(v, "radius", 0.0), rotate: b(v, "rotate").unwrap_or(true) },
            };
            wrap_in(doc, &ids, NodeKind::Repeat { repeat: rep, children: Vec::new() }, s(v, "name"))
        }
        "blend" => {
            let ids = edit::back_to_front(doc, &ids_arg(doc, v)?);
            if ids.len() != 2 {
                return Err("blend needs exactly two objects".into());
            }
            let spine = match v.get("spine") {
                Some(sp) if sp.get("d").is_some() || sp.get("points").is_some() => Some(path_of(sp)?),
                Some(x) => id_of(doc, x).and_then(|i| doc.find(i).map(|n| (i, n.clone()))).map(|(i, n)| {
                    let m = doc.world_transform(i).unwrap_or(Affine::IDENTITY);
                    PathData::new(build::geometry(doc, &n).into_iter().flat_map(|p| p.path.transformed(m).subpaths).collect())
                }),
                None => None,
            };
            let spec = BlendSpec { steps: fd(v, "steps", 0.0) as u32, spine, align_to_path: b(v, "align_to_path").unwrap_or(false) };
            wrap_in(doc, &ids, NodeKind::Blend { blend: spec, children: Vec::new() }, s(v, "name"))
        }
        "envelope" => {
            let ids = ids_arg(doc, v)?;
            let env = if let Some(w) = s(v, "warp") {
                let style: WarpKind = serde_json::from_value(json!(w)).map_err(|_| format!("unknown warp \"{w}\""))?;
                Envelope::Warp { style, bend: fd(v, "bend", 0.5), horizontal: fd(v, "horizontal", 0.0), vertical: fd(v, "vertical", 0.0), vertical_axis: b(v, "vertical_axis").unwrap_or(false) }
            } else if let Some(m) = v.get("mesh") {
                let (rows, cols) = (fd(m, "rows", 2.0) as u32, fd(m, "cols", 2.0) as u32);
                let pts = points(m.get("points").unwrap_or(&json!([])));
                Envelope::Mesh { rows, cols, points: pts }
            } else if let Some(c) = v.get("free") {
                let p = points(c);
                if p.len() != 4 {
                    return Err("free needs four corners".into());
                }
                Envelope::Free { corners: [p[0], p[1], p[2], p[3]] }
            } else if let Some(sh) = v.get("shape").and_then(|x| id_of(doc, x)) {
                let (p, _) = edit::outline_of(doc, sh).ok_or("the envelope shape has no outline")?;
                doc.remove(sh);
                Envelope::Shape { shape: p }
            } else {
                return Err("envelope needs warp, mesh, free or shape".into());
            };
            // A mesh with no points starts as the art's own grid.
            let env = match env {
                Envelope::Mesh { rows, cols, points } if points.is_empty() => {
                    let bx = ids.iter().filter_map(|i| build::world_bounds(doc, *i)).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
                    let pts = (0..=rows).flat_map(|r| (0..=cols).map(move |c| Point::new(bx.x0 + bx.width() * c as f64 / cols as f64, bx.y0 + bx.height() * r as f64 / rows as f64))).collect();
                    Envelope::Mesh { rows, cols, points: pts }
                }
                e => e,
            };
            wrap_in(doc, &ids, NodeKind::Envelope { envelope: env, children: Vec::new() }, s(v, "name"))
        }
        "perspective_grid" => {
            let mut g = doc.perspective.unwrap_or_default();
            if let Some(p) = pt(v, "left_vp") {
                g.left_vp = p;
            }
            if let Some(p) = pt(v, "right_vp") {
                g.right_vp = p;
            }
            if let Some(p) = pt(v, "origin") {
                g.origin = p;
            }
            if let Some(c) = f(v, "cell") {
                g.cell = c;
            }
            doc.perspective = Some(g);
            Ok(serde_json::to_value(g).unwrap_or(Value::Null))
        }
        "perspective" => {
            let ids = ids_arg(doc, v)?;
            if doc.perspective.is_none() {
                doc.perspective = Some(Perspective::default());
            }
            let plane: Plane = serde_json::from_value(json!(s(v, "plane").unwrap_or("left"))).map_err(|_| "plane is left, right or floor")?;
            wrap_in(doc, &ids, NodeKind::Perspective { plane, children: Vec::new() }, s(v, "name"))
        }
        "symbol" => {
            let ids = edit::back_to_front(doc, &ids_arg(doc, v)?);
            let name = s(v, "name").unwrap_or("Symbol").to_string();
            let sid = s(v, "id").map(String::from).unwrap_or_else(|| name.to_ascii_lowercase().replace(' ', "_"));
            let bx = ids.iter().filter_map(|i| build::world_bounds(doc, *i)).reduce(|a, b| a.union(b)).ok_or("nothing to make a symbol of")?;
            let mut art = Vec::new();
            for id in &ids {
                let world = doc.world_transform(*id).unwrap_or(Affine::IDENTITY);
                if let Some(mut n) = doc.remove(*id) {
                    // Symbol art is centred on its origin.
                    n.transform = Affine::translate(-bx.center().to_vec2()) * world;
                    art.push(n);
                }
            }
            doc.symbols.retain(|x| x.id != sid);
            doc.symbols.push(Symbol { id: sid.clone(), name, art });
            let mut inst = Node::new(0, NodeKind::Instance { symbol: sid.clone(), overrides: Vec::new() });
            inst.transform = Affine::translate(bx.center().to_vec2());
            let id = doc.add(None, inst);
            Ok(json!({"symbol": sid, "id": id}))
        }
        "instance" => {
            let sym = s(v, "symbol").ok_or("which symbol?")?;
            if doc.symbol(sym).is_none() {
                return Err(format!("no symbol \"{sym}\""));
            }
            let mut inst = Node::new(0, NodeKind::Instance { symbol: doc.symbol(sym).map(|x| x.id.clone()).unwrap_or_default(), overrides: Vec::new() });
            inst.transform = Affine::translate((fd(v, "x", 0.0), fd(v, "y", 0.0))) * Affine::rotate(fd(v, "rotate", 0.0).to_radians()) * Affine::scale(fd(v, "scale", 1.0));
            decorate(&mut inst, &json!({"name": v.get("name"), "opacity": v.get("opacity")}));
            let id = doc.add(parent_arg(doc, v), inst);
            Ok(json!({"id": id}))
        }
        "guide" => {
            doc.guides.push(Guide { x: f(v, "x"), y: f(v, "y"), locked: false });
            Ok(json!({"guides": doc.guides.len()}))
        }
        "grid_settings" => {
            if let Some(x) = f(v, "spacing") {
                doc.grid.spacing = x;
            }
            if let Some(x) = f(v, "subdivisions") {
                doc.grid.subdivisions = x as u32;
            }
            if let Some(x) = b(v, "snap") {
                doc.grid.snap = x;
            }
            if let Some(x) = b(v, "visible") {
                doc.grid.visible = x;
            }
            Ok(serde_json::to_value(doc.grid).unwrap_or(Value::Null))
        }
        "snap" => {
            let p = pt(v, "point").ok_or("snap needs point")?;
            let q = edit::snap(doc, p, fd(v, "tolerance", 6.0), &[]);
            Ok(json!([q.x, q.y]))
        }
        "chart" => chart(doc, v),
        "morph" => {
            let id = id_arg(doc, v)?;
            if build::geometry(doc, doc.find(id).ok_or("unknown object")?).is_empty() {
                return Err(format!("#{id} has no outline to morph (morph shapes, paths or type)"));
            }
            // The target: an object, an op that makes one, type, or path data.
            let to = need(v, "to")?;
            let made = |doc: &mut Document, o: Value| -> Result<NodeId, String> { apply(doc, &o, ctx)?["id"].as_u64().ok_or_else(|| "the target was not made".to_string()) };
            let (target, fresh) = match to {
                x if id_of(doc, x).is_some() => (id_of(doc, x).unwrap_or(0), false),
                Value::String(d) => (made(doc, json!({"op": "path", "d": d}))?, true),
                Value::Object(o) if o.contains_key("op") => (made(doc, to.clone())?, true),
                Value::Object(o) if o.contains_key("text") => {
                    let mut t = to.clone();
                    t["op"] = json!("text");
                    if t.get("x").is_none() || t.get("y").is_none() {
                        // Centred on the object it morphs from.
                        let c = build::world_bounds(doc, id).map(|b| b.center()).unwrap_or(Point::ZERO);
                        t["x"] = json!(c.x);
                        t["y"] = json!(c.y + fd(to, "size", 24.0) * 0.35);
                        t["align"] = json!("center");
                    }
                    (made(doc, t)?, true)
                }
                Value::Object(o) if o.contains_key("d") || o.contains_key("points") => {
                    let mut t = to.clone();
                    t["op"] = json!("path");
                    (made(doc, t)?, true)
                }
                other => return Err(format!("morph to what? an object id or name, an op ({{\"op\": \"star\", …}}), {{\"text\": …}} or path data; got {other}")),
            };
            if target == id {
                return Err("an object cannot morph into itself".into());
            }
            // The target is the shape to reach, not art of its own.
            if b(v, "keep_target") != Some(true)
                && let Some(n) = doc.find_mut(target)
            {
                n.visible = false;
                if fresh && n.name.is_empty() {
                    n.name = "Morph target".into();
                }
            }
            let t0 = f(v, "t").or_else(|| f(v, "at")).unwrap_or(0.0);
            let dur = fd(v, "duration", 1.0).max(0.0);
            let ease: Ease = match v.get("ease") {
                Some(Value::Array(a)) if a.len() == 4 => Ease::Bezier(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(1.0), a[3].as_f64().unwrap_or(1.0)),
                Some(e) => serde_json::from_value(e.clone()).unwrap_or_default(),
                None => Ease::EaseInOut,
            };
            let prop = format!("@morph:{target}");
            doc.animation.tracks.retain(|t| !(t.node == id && t.property.starts_with("@morph:")));
            match s(v, "param") {
                // The clip's parameter (keyed on the timeline) drives the amount.
                Some(param) => {
                    doc.animation.bindings.retain(|bd| !(bd.node == id && bd.property.starts_with("@morph:")));
                    doc.animation.bindings.push(Binding { node: id, property: prop.clone(), param: param.into() });
                }
                None => {
                    doc.animation.set_key(id, &prop, t0, json!(0.0), ease);
                    doc.animation.set_key(id, &prop, t0 + dur, json!(1.0), ease);
                }
            }
            // The colour travels too when both are flat colours.
            let mut colour = Value::Null;
            if s(v, "param").is_none()
                && let (Some(from), Some(to_node)) = (doc.find(id), doc.find(target))
                && let (Some(fi), Some(Paint::Solid { color: ca })) = (from.appearance.items.iter().rposition(|i| matches!(i, Item::Fill(_))), from.appearance.top_fill().map(|f| f.paint.clone()))
                && let Some(Paint::Solid { color: cb }) = to_node.appearance.top_fill().map(|f| f.paint.clone())
                && ca.to_hex() != cb.to_hex()
            {
                let ptr = format!("/appearance/items/{fi}/paint");
                doc.animation.set_key(id, &ptr, t0, json!({"type": "solid", "color": ca.to_hex()}), ease);
                doc.animation.set_key(id, &ptr, t0 + dur, json!({"type": "solid", "color": cb.to_hex()}), ease);
                colour = json!([ca.to_hex(), cb.to_hex()]);
            }
            Ok(json!({"id": id, "target": target, "property": prop, "from": t0, "to": t0 + dur, "colour": colour}))
        }
        "key" => {
            let id = id_arg(doc, v)?;
            let prop = s(v, "property").ok_or("key needs property")?;
            let ease: Ease = match v.get("ease") {
                Some(Value::Array(a)) if a.len() == 4 => Ease::Bezier(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(1.0), a[3].as_f64().unwrap_or(1.0)),
                Some(e) => serde_json::from_value(e.clone()).unwrap_or_default(),
                None => Ease::EaseInOut,
            };
            let mut val = need(v, "value")?.clone();
            if prop.ends_with("/paint") {
                val = serde_json::to_value(paint(&val)?).map_err(|e| e.to_string())?;
            }
            doc.animation.set_key(id, prop, fd(v, "t", 0.0), val, ease);
            Ok(json!({"tracks": doc.animation.tracks.len(), "length": doc.animation.length()}))
        }
        "bind" => {
            let id = id_arg(doc, v)?;
            doc.animation.bindings.push(Binding { node: id, property: s(v, "property").ok_or("bind needs property")?.into(), param: s(v, "param").ok_or("bind needs param")?.into() });
            Ok(json!({"bindings": doc.animation.bindings.len()}))
        }
        "animation" => {
            if let Some(d) = f(v, "duration") {
                doc.animation.duration = d;
            }
            if let Some(l) = b(v, "looping") {
                doc.animation.looping = l;
            }
            Ok(json!({"duration": doc.animation.length(), "looping": doc.animation.looping}))
        }
        "clear_keys" => {
            let id = v.get("id").and_then(|x| id_of(doc, x));
            let prop = s(v, "property");
            doc.animation.tracks.retain(|t| !(id.is_none_or(|i| t.node == i) && prop.is_none_or(|p| t.property == p)));
            Ok(json!({"tracks": doc.animation.tracks.len()}))
        }
        "check" => Ok(json!(check(doc))),
        "look" | "render" => {
            let ab = v.get("artboard").map(|a| a.as_str().map(String::from).unwrap_or_else(|| a.to_string()));
            let board = doc.artboard(ab.as_deref()).cloned().ok_or("the document has no artboard")?;
            let scale = f(v, "scale").unwrap_or_else(|| fd(v, "width", if op == "look" { 1200.0 } else { board.rect.width() }) / board.rect.width());
            let file = s(v, "file").map(|f| ctx.path(f)).unwrap_or_else(|| ctx.out(&format!("vector-{}.png", if op == "look" { "look" } else { "render" })));
            let at = if v.get("t").is_some() { crate::anim::at_time(doc, fd(v, "t", 0.0), &|_| None) } else { doc.clone() };
            crate::interop::save(&at, &file, ab.as_deref(), scale, &ctx.build())?;
            Ok(json!({"file": file.display().to_string(), "width": (board.rect.width() * scale).round(), "height": (board.rect.height() * scale).round()}))
        }
        "export" | "save" => {
            let file = ctx.path(s(v, "file").or_else(|| s(v, "path")).ok_or("export needs a file")?);
            let ab = v.get("artboard").and_then(Value::as_str);
            crate::interop::save(doc, &file, ab, fd(v, "scale", 1.0), &ctx.build())?;
            let len = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            Ok(json!({"file": file.display().to_string(), "bytes": len}))
        }
        "import" | "place_file" => {
            let file = ctx.path(s(v, "file").or_else(|| s(v, "path")).ok_or("import needs a file")?);
            let other = crate::interop::open(&file)?;
            let mut kids: Vec<Node> = other.layers.into_iter().flat_map(|l| match l.kind {
                NodeKind::Group { children, layer: true, .. } => children,
                _ => vec![l],
            }).collect();
            doc.reid(&mut kids);
            for s in other.swatches {
                if doc.swatch(&s.id).is_none() {
                    doc.swatches.push(s);
                }
            }
            for sym in other.symbols {
                if doc.symbol(&sym.id).is_none() {
                    doc.symbols.push(sym);
                }
            }
            let mut g = Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false });
            g.name = file.file_name().and_then(|n| n.to_str()).unwrap_or("Imported").to_string();
            if let Some(p) = pt(v, "at") {
                g.transform = Affine::translate(p.to_vec2());
            }
            let id = doc.add(parent_arg(doc, v), g);
            Ok(json!({"id": id, "bounds": bounds_json(build::world_bounds(doc, id))}))
        }
        "batch" => {
            let list = need(v, "ops")?.as_array().ok_or("ops is a list")?;
            let mut out = Vec::new();
            for (i, o) in list.iter().enumerate() {
                match apply(doc, o, ctx) {
                    Ok(r) => out.push(r),
                    Err(e) => return Err(format!("op {} ({}): {e}", i + 1, s(o, "op").unwrap_or("?"))),
                }
            }
            Ok(json!(out))
        }
        other => Err(format!("unknown vector operation \"{other}\" (\"help\" lists them)")),
    }
}

fn paint_or_color(pointer: &str, v: &Value) -> Result<Value, String> {
    if pointer.ends_with("/color") {
        return serde_json::to_value(color(v)?).map_err(|e| e.to_string());
    }
    serde_json::to_value(paint(v)?).map_err(|e| e.to_string())
}

fn merge(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in y {
                merge(x.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

fn anchors(doc: &Document, id: NodeId) -> usize {
    match doc.find(id).map(|n| &n.kind) {
        Some(NodeKind::Path { path, .. }) => path.anchor_count(),
        _ => 0,
    }
}

fn with_default_stroke(doc: &mut Document, r: Value) -> R {
    if let Some(id) = r.get("id").and_then(Value::as_u64)
        && let Some(n) = doc.find_mut(id)
        && !n.appearance.items.iter().any(|i| matches!(i, Item::Stroke(_)))
        && n.appearance.items.is_empty()
    {
        n.appearance.items.push(Item::Stroke(Stroke::new(Color::from_hex("#2b2b2b").unwrap_or(Color::BLACK), 2.0)));
    }
    Ok(r)
}

/// Applies a document-space transform to a node (in its parent's space).
fn apply_world(doc: &mut Document, id: NodeId, world: Affine) {
    let pm = doc.parent_of(id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
    if let Some(n) = doc.find_mut(id) {
        n.transform = pm.inverse() * world * pm * n.transform;
    }
}

/// Puts `ids` into a new node of `kind` (a container) where the topmost was.
fn wrap_in(doc: &mut Document, ids: &[NodeId], kind: NodeKind, name: Option<&str>) -> R {
    let g = edit::group(doc, ids)?;
    let n = doc.find_mut(g).ok_or("grouping failed")?;
    let kids = n.children().cloned().unwrap_or_default();
    n.kind = kind;
    if let Some(c) = n.children_mut() {
        *c = kids;
    }
    if let Some(name) = name {
        n.name = name.into();
    }
    Ok(json!({"id": g, "bounds": bounds_json(build::world_bounds(doc, g))}))
}

fn restyle(n: &mut Node, v: &Value) -> Result<(), String> {
    if let Some(fl) = v.get("fill") {
        n.appearance.items.retain(|i| !matches!(i, Item::Fill(_)));
        if !fl.is_null() && fl.as_str() != Some("none") {
            n.appearance.items.insert(0, Item::Fill(fill_item(fl)?));
        }
    }
    if let Some(sk) = v.get("stroke") {
        n.appearance.items.retain(|i| !matches!(i, Item::Stroke(_)));
        if !sk.is_null() && sk.as_str() != Some("none") {
            n.appearance.items.push(Item::Stroke(stroke_item(sk, v)?));
        }
    } else {
        // Stroke properties alone adjust the existing stroke.
        for it in n.appearance.items.iter_mut() {
            if let Item::Stroke(st) = it {
                if let Some(w) = f(v, "stroke_width") {
                    st.style.width = w;
                }
                if let Some(j) = s(v, "join") {
                    st.style.join = join_of(j);
                }
                if let Some(c) = s(v, "cap") {
                    st.style.start_cap = cap_of(c);
                    st.style.end_cap = cap_of(c);
                }
                if let Some(d) = v.get("dashes").and_then(Value::as_array) {
                    st.style.dashes = d.iter().filter_map(Value::as_f64).collect();
                }
                if let Some(o) = f(v, "dash_offset") {
                    st.style.dash_offset = o;
                }
                if let Some(p) = v.get("profile") {
                    st.style.profile = profile(p)?;
                }
            }
        }
    }
    if let Some(e) = v.get("effects") {
        n.appearance.effects = effects(e)?;
    }
    decorate(n, &json!({"opacity": v.get("opacity"), "blend": v.get("blend"), "name": v.get("name")}).as_object().map(|o| Value::Object(o.iter().filter(|(_, x)| !x.is_null()).map(|(k, x)| (k.clone(), x.clone())).collect())).unwrap_or(Value::Null));
    Ok(())
}

fn recolor(n: &mut Node, pairs: &[(String, Color)], changed: &mut usize) {
    let swap = |c: &mut Color, changed: &mut usize| {
        let key = Color::rgba(c.r, c.g, c.b, 1.0).to_hex();
        if let Some((_, to)) = pairs.iter().find(|(k, _)| *k == key) {
            *c = Color::rgba(to.r, to.g, to.b, c.a * to.a);
            *changed += 1;
        }
    };
    for it in &mut n.appearance.items {
        let p = match it {
            Item::Fill(f) => &mut f.paint,
            Item::Stroke(s) => &mut s.paint,
        };
        match p {
            Paint::Solid { color } => swap(color, changed),
            Paint::Gradient(g) => {
                let mut ng = (**g).clone();
                for s in &mut ng.stops {
                    swap(&mut s.color, changed);
                }
                *g = Arc::new(Gradient::new(ng.kind, ng.stops.clone()).with_extend(ng.extend).with_interpolation(ng.interpolation).with_transform(ng.transform));
            }
            _ => {}
        }
    }
    if let NodeKind::Text(t) = &mut n.kind {
        for r in &mut t.runs {
            if let Some(c) = &mut r.props.color {
                swap(c, changed);
            }
        }
    }
    if let Some(c) = n.children_mut() {
        for k in c {
            recolor(k, pairs, changed);
        }
    }
}

fn text_node(v: &Value, kind: TextKind) -> Result<TextNode, String> {
    let mut char_props: CharProps = serde_json::from_value(v.clone()).unwrap_or_default();
    if char_props.size.is_none() {
        char_props.size = Some(fd(v, "size", 24.0));
    }
    char_props.color = None;
    let mut para = crate::text::ParaProps::default();
    if let Some(a) = s(v, "align") {
        para.align = Some(match a {
            "center" | "centre" => TextAlign::Center,
            "right" => TextAlign::Right,
            "justify" => TextAlign::Justify,
            _ => TextAlign::Left,
        });
    }
    let runs = match v.get("runs").and_then(Value::as_array) {
        Some(rs) => rs.iter().map(|r| serde_json::from_value::<Run>(r.clone()).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?,
        None => vec![Run { text: s(v, "text").ok_or("text needs \"text\"")?.to_string(), style: s(v, "char_style").map(String::from), props: CharProps::default() }],
    };
    Ok(TextNode { kind, runs, char: char_props, para, para_style: s(v, "para_style").map(String::from) })
}

/// Charts for infographics: bars, columns, lines, pies and donuts, with
/// labels and values, as plain vector art in a group.
fn chart(doc: &mut Document, v: &Value) -> R {
    let r = rect_arg(v).ok_or("chart needs a rect")?;
    let kind = s(v, "kind").unwrap_or("column");
    let data: Vec<(String, f64)> = need(v, "data")?
        .as_array()
        .ok_or("data is a list")?
        .iter()
        .enumerate()
        .filter_map(|(i, d)| match d {
            Value::Number(n) => Some((format!("{}", i + 1), n.as_f64()?)),
            Value::Array(a) if a.len() >= 2 => Some((a[0].as_str().map(String::from).unwrap_or_else(|| a[0].to_string()), a[1].as_f64()?)),
            Value::Object(_) => Some((s(d, "label").unwrap_or("").to_string(), f(d, "value")?)),
            _ => None,
        })
        .collect();
    if data.is_empty() {
        return Err("chart needs data".into());
    }
    let palette: Vec<Color> = match v.get("colors").and_then(Value::as_array) {
        Some(c) => c.iter().filter_map(|x| color(x).ok()).collect(),
        None => color::harmony(Color::from_hex("#2b6cb0").unwrap_or(Color::BLACK), Harmony::Analogous, data.len().max(2)).into_iter().chain(color::harmony(Color::from_hex("#e8573f").unwrap_or(Color::BLACK), Harmony::Analogous, 6)).collect(),
    };
    let pal = |i: usize| palette.get(i % palette.len().max(1)).copied().unwrap_or(Color::BLACK);
    let font = s(v, "font").map(String::from);
    let labels = b(v, "labels").unwrap_or(true);
    let label_size = fd(v, "label_size", (r.height() / 18.0).clamp(8.0, 28.0));
    let ink = color(v.get("ink").unwrap_or(&json!("#333333")))?;
    let mut kids: Vec<Node> = Vec::new();
    let text = |t: &str, at: Point, size: f64, align: TextAlign, kids: &mut Vec<Node>| {
        let mut tn = TextNode::point(at, t, size);
        tn.char.font = font.clone();
        tn.para.align = Some(align);
        let mut n = Node::new(0, NodeKind::Text(tn));
        n.appearance = Appearance::fill(ink);
        kids.push(n);
    };
    let shape = |p: PathData, c: Color| {
        let mut n = Node::new(0, NodeKind::Path { path: p, rule: FillRule::NonZero });
        n.appearance = Appearance::fill(c);
        n
    };
    let max = data.iter().map(|d| d.1).fold(0.0, f64::max).max(1e-9);
    let mut plot = r;
    if let Some(t) = s(v, "title") {
        text(t, Point::new(r.x0, r.y0 + label_size * 1.4), label_size * 1.4, TextAlign::Left, &mut kids);
        plot.y0 += label_size * 2.4;
    }
    match kind {
        "pie" | "donut" => {
            let total: f64 = data.iter().map(|d| d.1.max(0.0)).sum::<f64>().max(1e-9);
            let c = plot.center();
            let rad = plot.width().min(plot.height()) / 2.0 * 0.8;
            let mut a0 = -90.0;
            for (i, (label, val)) in data.iter().enumerate() {
                let sweep = 360.0 * val.max(0.0) / total;
                let mut pie = LiveShape::Pie { rect: Rect::new(c.x - rad, c.y - rad, c.x + rad, c.y + rad), start: -a0 - sweep, end: -a0 }.to_path();
                if kind == "donut" {
                    // A real ring segment: the centre stays see-through.
                    let hole = rad * fd(v, "hole", 0.55);
                    let disc = LiveShape::Ellipse { rect: Rect::new(c.x - hole, c.y - hole, c.x + hole, c.y + hole) }.to_path();
                    let nz = studio_vector_geom::FillRule::NonZero;
                    pie = studio_vector_pathops::boolean(&pie, nz, &disc, nz, studio_vector_pathops::BoolOp::Difference);
                }
                let mut n = shape(pie, pal(i));
                n.name = label.clone();
                kids.push(n);
                if labels {
                    let mid = (a0 + sweep / 2.0).to_radians();
                    let at = c + Vec2::new(mid.cos(), mid.sin()) * (rad * 1.15);
                    text(&format!("{label} {:.0}%", val / total * 100.0), at, label_size, if mid.cos() < 0.0 { TextAlign::Right } else { TextAlign::Left }, &mut kids);
                }
                a0 += sweep;
            }
        }
        "line" => {
            let n = data.len();
            let base = plot.y1 - if labels { label_size * 1.6 } else { 0.0 };
            let pts: Vec<Point> = data.iter().enumerate().map(|(i, d)| Point::new(plot.x0 + plot.width() * i as f64 / (n.max(2) - 1) as f64, base - (base - plot.y0) * d.1 / max)).collect();
            let mut line = Node::new(0, NodeKind::Path { path: edit::curvature(&pts, false, &[]), rule: FillRule::NonZero });
            line.appearance = Appearance { items: vec![Item::Stroke({
                let mut st = Stroke::new(pal(0), fd(v, "line_width", 4.0));
                st.style.join = Join::Round;
                st.style.start_cap = Cap::Round;
                st.style.end_cap = Cap::Round;
                st
            })], effects: Vec::new() };
            kids.push(line);
            for (i, p) in pts.iter().enumerate() {
                kids.push(shape(LiveShape::Ellipse { rect: Rect::new(p.x - 5.0, p.y - 5.0, p.x + 5.0, p.y + 5.0) }.to_path(), pal(0)));
                if labels {
                    text(&data[i].0, Point::new(p.x, plot.y1), label_size, TextAlign::Center, &mut kids);
                }
            }
        }
        horizontal @ ("bar" | "column") => {
            let n = data.len() as f64;
            let gap = fd(v, "gap", 0.25);
            let label_room = if labels { label_size * 1.6 } else { 0.0 };
            for (i, (label, val)) in data.iter().enumerate() {
                let i = i as f64;
                let rr = if horizontal == "bar" {
                    let left = plot.x0 + if labels { plot.width() * 0.2 } else { 0.0 };
                    let step = plot.height() / n;
                    let y = plot.y0 + step * i + step * gap / 2.0;
                    if labels {
                        text(label, Point::new(left - label_size * 0.5, y + step * (1.0 - gap) * 0.65), label_size, TextAlign::Right, &mut kids);
                    }
                    Rect::new(left, y, left + (plot.x1 - left) * val / max, y + step * (1.0 - gap))
                } else {
                    let bottom = plot.y1 - label_room;
                    let step = plot.width() / n;
                    let x = plot.x0 + step * i + step * gap / 2.0;
                    if labels {
                        text(label, Point::new(x + step * (1.0 - gap) / 2.0, plot.y1 - label_size * 0.3), label_size, TextAlign::Center, &mut kids);
                    }
                    Rect::new(x, bottom - (bottom - plot.y0) * val / max, x + step * (1.0 - gap), bottom)
                };
                let radius = fd(v, "radius", 0.0);
                let mut n = shape(LiveShape::Rect { rect: rr, radii: [radius, radius, if horizontal == "bar" { radius } else { 0.0 }, if horizontal == "bar" { 0.0 } else { 0.0 }], corners: None }.to_path(), pal(i as usize));
                n.name = label.clone();
                kids.push(n);
            }
        }
        other => return Err(format!("chart kind \"{other}\"? (bar, column, line, pie, donut)")),
    }
    let mut g = Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false });
    g.name = s(v, "name").unwrap_or("Chart").to_string();
    let id = doc.add(parent_arg(doc, v), g);
    let mut kids = doc.find(id).and_then(|n| n.children().cloned()).unwrap_or_default();
    doc.reid(&mut kids);
    if let Some(c) = doc.find_mut(id).and_then(Node::children_mut) {
        *c = kids;
    }
    Ok(json!({"id": id, "bounds": bounds_json(build::world_bounds(doc, id))}))
}

/// Applies a list of operations (or one), stopping at the first error.
pub fn apply_all(doc: &mut Document, ops: &Value, ctx: &Context) -> R {
    match ops {
        Value::Array(list) => apply(doc, &json!({"op": "batch", "ops": list}), ctx),
        one => apply(doc, one, ctx),
    }
}

/// Opens a document file for operations (a new one if it does not exist).
pub fn open_or_new(path: &Path, width: f64, height: f64) -> Result<Document, String> {
    if path.exists() { crate::interop::open(path) } else { Ok(Document::new(width, height)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(doc: &mut Document, j: Value) -> Value {
        apply(doc, &j, &Context::default()).unwrap_or_else(|e| panic!("{j}: {e}"))
    }

    #[test]
    fn draw_style_boolean_and_summarise() {
        let mut d = Document::new(400.0, 300.0);
        let a = run(&mut d, json!({"op": "rect", "x": 20, "y": 20, "w": 120, "h": 80, "radius": 12, "fill": {"linear": {"from": [20, 0], "to": [140, 0], "stops": ["#ff512f", "#f9d423"]}}, "stroke": "#222", "stroke_width": 3}));
        let b = run(&mut d, json!({"op": "circle", "cx": 140, "cy": 100, "r": 50, "fill": "tomato", "effects": [{"type": "drop_shadow", "dx": 4, "dy": 6, "blur": 12, "color": "#00000066"}]}));
        let ids = json!([a["id"], b["id"]]);
        let u = run(&mut d, json!({"op": "pathfinder", "ids": ids, "mode": "unite"}));
        assert_eq!(u["ids"].as_array().map(Vec::len), Some(1));
        run(&mut d, json!({"op": "star", "cx": 300, "cy": 150, "r": 60, "points": 5, "fill": "gold"}));
        run(&mut d, json!({"op": "repeat", "ids": [u["ids"][0]], "kind": "radial", "count": 3, "center": [200, 150]}));
        let sum = run(&mut d, json!({"op": "summary"}));
        assert!(sum["objects"].as_array().map(Vec::len).unwrap_or(0) >= 3);
        let help = run(&mut d, json!({"op": "help"}));
        assert!(help.as_array().map(Vec::len).unwrap_or(0) > 80);
        assert!(apply(&mut d, &json!({"op": "frobnicate"}), &Context::default()).is_err());
    }

    #[test]
    fn set_patches_any_property_and_keys_animate() {
        let mut d = Document::new(100.0, 100.0);
        let r = run(&mut d, json!({"op": "rect", "x": 0, "y": 0, "w": 10, "h": 10}));
        let id = r["id"].clone();
        run(&mut d, json!({"op": "set", "id": id, "path": "/appearance/items/0/paint", "value": "#00ff00"}));
        let n = run(&mut d, json!({"op": "get", "id": id}));
        assert_eq!(n["appearance"]["items"][0]["paint"], json!({"type": "solid", "color": "#00ff00"}));
        run(&mut d, json!({"op": "key", "id": id, "property": "@rotate", "t": 0, "value": 0}));
        run(&mut d, json!({"op": "key", "id": id, "property": "@rotate", "t": 1, "value": 90}));
        assert_eq!(d.animation.tracks.len(), 1);
    }

    #[test]
    fn charts_and_harmonies() {
        let mut d = Document::new(800.0, 600.0);
        let c = run(&mut d, json!({"op": "chart", "kind": "donut", "rect": [0, 0, 400, 400], "data": [["A", 3], ["B", 2], ["C", 1]], "labels": false}));
        assert_eq!(d.find(c["id"].as_u64().unwrap()).and_then(Node::children).map(Vec::len), Some(3));
        // The donut's centre is a hole, not a disc painted over it.
        for k in d.find(c["id"].as_u64().unwrap()).and_then(Node::children).unwrap() {
            if let NodeKind::Path { path, .. } = &k.kind {
                assert!(!studio_vector_geom::hit::fill_contains(&path.to_bezpath(), studio_vector_geom::FillRule::NonZero, Point::new(200.0, 200.0)));
            }
        }
        let h = run(&mut d, json!({"op": "harmony", "base": "#d94f30", "rule": "triadic", "swatches": true}));
        assert_eq!(h.as_array().map(Vec::len), Some(3));
        assert_eq!(d.swatches.len(), 3);
    }
}

#[cfg(test)]
mod morph_tests {
    use super::*;

    fn area_at(d: &Document, id: NodeId, t: f64) -> (f64, usize) {
        let at = crate::anim::at_time(d, t, &|_| None);
        let n = at.find(id).unwrap();
        let p = PathData::new(build::geometry(&at, n).into_iter().flat_map(|p| p.path.subpaths).collect());
        (studio_vector_pathops::area(&p, studio_vector_geom::FillRule::NonZero).abs(), p.anchor_count())
    }

    #[test]
    fn a_circle_really_becomes_a_triangle() {
        let mut d = Document::new(1000.0, 1000.0);
        let ctx = Context::default();
        let c = apply(&mut d, &json!({"op": "circle", "cx": 500, "cy": 500, "r": 200, "fill": "#e8573f"}), &ctx).unwrap()["id"].as_u64().unwrap();
        let r = apply(&mut d, &json!({"op": "morph", "id": c, "to": {"op": "polygon", "cx": 500, "cy": 500, "r": 200, "sides": 3, "fill": "#2b6cb0"}, "t": 0, "duration": 2, "ease": "linear"}), &ctx).unwrap();
        let tri = r["target"].as_u64().unwrap();
        assert!(!d.find(tri).unwrap().visible, "the target is hidden");
        let circle = std::f64::consts::PI * 200.0 * 200.0;
        let triangle = 3.0 * 3.0f64.sqrt() / 4.0 * (200.0f64 * 3.0f64.sqrt()).powi(2) / 3.0 * 3.0 / 3.0;
        let (a0, _) = area_at(&d, c, 0.0);
        let (a1, _) = area_at(&d, c, 2.0);
        let (am, n1) = area_at(&d, c, 1.0);
        assert!((a0 - circle).abs() / circle < 0.01, "{a0} vs {circle}");
        assert!((a1 - triangle).abs() / triangle < 0.01, "{a1} vs {triangle}");
        assert!(am < a0 && am > a1, "{a1} < {am} < {a0}");
        assert!(n1 >= 32, "a real outline, not a scaled copy: {n1} points");
        // The colour travels with it.
        assert_eq!(r["colour"], json!(["#e8573f", "#2b6cb0"]));
        // Into a word: several outlines from one.
        let w = apply(&mut d, &json!({"op": "morph", "id": tri, "to": {"text": "OK", "size": 300}, "keep_target": true}), &ctx);
        assert!(w.is_ok() || crate::text::resolve("Helvetica", 400, false).is_none());
    }
}
