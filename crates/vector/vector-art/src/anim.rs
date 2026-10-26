// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animating vector art on the timeline. Any property of any node can be
//! keyframed: a track names the node and a JSON pointer into it
//! (`/opacity`, `/appearance/items/0/paint`, `/shape/radius`, `/path`…).
//! Numbers, colours, points, transforms and paths interpolate (paths
//! morph, anchors added to the one with fewer); other values hold. Three
//! extra properties move a node about its centre: `@move` (`[dx, dy]`),
//! `@rotate` (degrees) and `@scale` (`[sx, sy]` or one number).
//!
//! Bindings tie a property to a parameter of the host clip instead, so the
//! timeline's own keyframes and graph editor drive it.

use std::collections::BTreeMap;

use kurbo::Affine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use studio_vector_geom::{PathData, Point, SubPath};
use studio_vector_raster::Color;

use crate::doc::{Document, Node, NodeId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Jumps to the next key's value when it is reached.
    Hold,
    /// A cubic timing curve through (0,0), (x1,y1), (x2,y2), (1,1).
    Bezier(f64, f64, f64, f64),
    /// Overshoots and settles (a damped spring).
    Spring,
}

impl Ease {
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::EaseIn => t * t * t,
            Ease::EaseOut => 1.0 - (1.0 - t).powi(3),
            Ease::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Ease::Hold => 0.0,
            Ease::Spring => 1.0 - (-6.0 * t).exp() * (12.0 * t).cos(),
            Ease::Bezier(x1, y1, x2, y2) => {
                // Solve x(s) = t by Newton then bisection, return y(s).
                let bez = |a: f64, b: f64, s: f64| 3.0 * a * s * (1.0 - s) * (1.0 - s) + 3.0 * b * s * s * (1.0 - s) + s * s * s;
                let (mut lo, mut hi) = (0.0, 1.0);
                let mut s = t;
                for _ in 0..40 {
                    let x = bez(x1, x2, s);
                    if (x - t).abs() < 1e-7 {
                        break;
                    }
                    if x < t {
                        lo = s;
                    } else {
                        hi = s;
                    }
                    s = (lo + hi) / 2.0;
                }
                bez(y1, y2, s)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// Seconds from the start of the art's clip.
    pub t: f64,
    pub value: Value,
    /// The easing from this key to the next.
    #[serde(default)]
    pub ease: Ease,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub node: NodeId,
    pub property: String,
    pub keys: Vec<Key>,
}

/// A property driven by a parameter of the clip that shows the art.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub node: NodeId,
    pub property: String,
    pub param: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Animation {
    /// Length in seconds (0 = as long as the last key).
    #[serde(default)]
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    /// Repeat after `duration`.
    #[serde(default)]
    pub looping: bool,
}

impl Animation {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.bindings.is_empty() && self.duration == 0.0
    }

    pub fn length(&self) -> f64 {
        if self.duration > 0.0 {
            return self.duration;
        }
        self.tracks.iter().flat_map(|t| t.keys.iter().map(|k| k.t)).fold(0.0, f64::max)
    }

    /// Sets (or replaces) a key.
    pub fn set_key(&mut self, node: NodeId, property: &str, t: f64, value: Value, ease: Ease) {
        let track = match self.tracks.iter_mut().position(|tr| tr.node == node && tr.property == property) {
            Some(i) => &mut self.tracks[i],
            None => {
                self.tracks.push(Track { node, property: property.to_string(), keys: Vec::new() });
                self.tracks.last_mut().unwrap_or_else(|| unreachable!())
            }
        };
        track.keys.retain(|k| (k.t - t).abs() > 1e-9);
        track.keys.push(Key { t, value, ease });
        track.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    }
}

/// A track's value at `t`.
pub fn sample(track: &Track, t: f64) -> Option<Value> {
    let k = &track.keys;
    if k.is_empty() {
        return None;
    }
    if t <= k[0].t {
        return Some(k[0].value.clone());
    }
    for w in k.windows(2) {
        if t < w[1].t {
            let span = (w[1].t - w[0].t).max(1e-12);
            let e = w[0].ease.apply((t - w[0].t) / span);
            if w[0].ease == Ease::Hold {
                return Some(w[0].value.clone());
            }
            return Some(lerp(&w[0].value, &w[1].value, e));
        }
    }
    k.last().map(|k| k.value.clone())
}

fn color_of(v: &Value) -> Option<Color> {
    serde_json::from_value::<Color>(v.clone()).ok().filter(|_| v.is_string() && v.as_str().is_some_and(|s| s.starts_with('#')))
}

fn path_of(v: &Value) -> Option<PathData> {
    v.get("subpaths")?;
    serde_json::from_value(v.clone()).ok()
}

/// Interpolates two JSON values: numbers, arrays and objects key by key,
/// colours in sRGB, paths morphing; anything else holds `a` until `t = 1`.
pub fn lerp(a: &Value, b: &Value, t: f64) -> Value {
    if t <= 0.0 {
        return a.clone();
    }
    if t >= 1.0 {
        return b.clone();
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            json!(x + (y - x) * t)
        }
        (Value::String(_), Value::String(_)) => match (color_of(a), color_of(b)) {
            (Some(p), Some(q)) => {
                let f = t as f32;
                json!(Color::rgba(p.r + (q.r - p.r) * f, p.g + (q.g - p.g) * f, p.b + (q.b - p.b) * f, p.a + (q.a - p.a) * f).to_hex())
            }
            _ => a.clone(),
        },
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => Value::Array(x.iter().zip(y).map(|(p, q)| lerp(p, q, t)).collect()),
        (Value::Object(_), Value::Object(_)) => {
            if let (Some(p), Some(q)) = (path_of(a), path_of(b)) {
                return serde_json::to_value(morph(&p, &q, t)).unwrap_or_else(|_| a.clone());
            }
            let (Value::Object(x), Value::Object(y)) = (a, b) else { return a.clone() };
            let mut out = serde_json::Map::new();
            for (k, va) in x {
                out.insert(k.clone(), match y.get(k) { Some(vb) => lerp(va, vb, t), None => va.clone() });
            }
            for (k, vb) in y {
                if !out.contains_key(k) {
                    out.insert(k.clone(), vb.clone());
                }
            }
            Value::Object(out)
        }
        _ => a.clone(),
    }
}

/// A path flattened into outlines for the morph engine.
pub fn contours(p: &PathData) -> Vec<studio_morph::Contour> {
    let size = p.bounds().map(|r| r.width().hypot(r.height())).unwrap_or(1.0).max(1e-6);
    let tol = size / 4000.0;
    p.subpaths
        .iter()
        .filter(|s| !s.anchors.is_empty())
        .map(|s| {
            let mut bp = kurbo::BezPath::new();
            s.to_bezpath_into(&mut bp);
            let mut pts = Vec::new();
            kurbo::flatten(bp.iter(), tol, |el| {
                if let kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) = el {
                    pts.push([q.x, q.y]);
                }
            });
            if pts.is_empty() {
                pts.push([s.anchors[0].p.x, s.anchors[0].p.y]);
            }
            studio_morph::Contour::new(pts, s.closed)
        })
        .collect()
}

/// Outlines back into a path (corner points).
pub fn from_contours(c: Vec<studio_morph::Contour>) -> PathData {
    PathData::new(c.into_iter().map(|c| SubPath::polyline(&c.pts.iter().map(|p| Point::new(p[0], p[1])).collect::<Vec<_>>(), c.closed)).collect())
}

/// Shape `a` turning into shape `b` (`studio-morph`): subpaths pair by place
/// and size (extras grow from a point), outlines resampled to equal counts
/// with corners kept, winding matched, the start point that moves least.
pub fn morph(a: &PathData, b: &PathData, t: f64) -> PathData {
    if t <= 0.0 || b.is_empty() {
        return a.clone();
    }
    if t >= 1.0 || a.is_empty() {
        return b.clone();
    }
    from_contours(studio_morph::Morph::new(&contours(a), &contours(b), studio_morph::Options::default()).at(t))
}

/// Sets the value at `pointer` inside `v`, creating objects on the way.
fn set_pointer(v: &mut Value, pointer: &str, value: Value) -> bool {
    if pointer.is_empty() || pointer == "/" {
        *v = value;
        return true;
    }
    let mut cur = v;
    let parts: Vec<String> = pointer.trim_start_matches('/').split('/').map(|p| p.replace("~1", "/").replace("~0", "~")).collect();
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        match cur {
            Value::Object(m) => {
                if last {
                    m.insert(p.clone(), value);
                    return true;
                }
                cur = m.entry(p.clone()).or_insert_with(|| json!({}));
            }
            Value::Array(a) => {
                let Ok(k) = p.parse::<usize>() else { return false };
                let Some(slot) = a.get_mut(k) else { return false };
                if last {
                    *slot = value;
                    return true;
                }
                cur = slot;
            }
            _ => return false,
        }
    }
    false
}

/// The node's bounding box centre in its parent's space.
fn centre(n: &Node) -> Point {
    crate::build::node_bounds(n).map(|r| r.center()).unwrap_or(Point::ZERO)
}

fn apply_virtual(n: &mut Node, prop: &str, v: &Value) {
    let num = |v: &Value| v.as_f64().unwrap_or(0.0);
    let pair = |v: &Value| -> (f64, f64) {
        match v {
            Value::Array(a) if a.len() >= 2 => (num(&a[0]), num(&a[1])),
            Value::Number(_) => (num(v), num(v)),
            Value::Object(o) => (o.get("x").map(num).unwrap_or(0.0), o.get("y").map(num).unwrap_or(0.0)),
            _ => (0.0, 0.0),
        }
    };
    let c = centre(n);
    let about = |m: Affine| Affine::translate(c.to_vec2()) * m * Affine::translate(-c.to_vec2());
    let m = match prop {
        "@move" => {
            let (x, y) = pair(v);
            Affine::translate((x, y))
        }
        "@rotate" => about(Affine::rotate(num(v).to_radians())),
        "@scale" => {
            let (x, y) = pair(v);
            about(Affine::scale_non_uniform(x, y))
        }
        _ => return,
    };
    n.transform = m * n.transform;
}

/// The document as it looks at `t` seconds, with bound properties read
/// from `params`.
pub fn at_time(doc: &Document, t: f64, params: &dyn Fn(&str) -> Option<Value>) -> Document {
    let a = &doc.animation;
    if a.tracks.is_empty() && a.bindings.is_empty() {
        return doc.clone();
    }
    let len = a.length();
    let t = if a.looping && len > 0.0 { t.rem_euclid(len) } else { t };
    let mut by_node: BTreeMap<NodeId, Vec<(String, Value)>> = BTreeMap::new();
    for tr in &a.tracks {
        if let Some(v) = sample(tr, t) {
            by_node.entry(tr.node).or_default().push((tr.property.clone(), v));
        }
    }
    for b in &a.bindings {
        if let Some(v) = params(&b.param) {
            by_node.entry(b.node).or_default().push((b.property.clone(), v));
        }
    }
    let mut out = doc.clone();
    for (id, props) in by_node {
        let Some(node) = out.find_mut(id) else { continue };
        let (virt, real): (Vec<_>, Vec<_>) = props.into_iter().partition(|(p, _)| p.starts_with('@'));
        if !real.is_empty()
            && let Ok(mut v) = serde_json::to_value(&*node)
        {
            for (p, val) in real {
                set_pointer(&mut v, &p, val);
            }
            if let Ok(n) = serde_json::from_value::<Node>(v) {
                // Children keep their own (possibly also animated) state.
                let kids = node.children().cloned();
                *node = n;
                if let (Some(k), Some(c)) = (kids, node.children_mut()) {
                    *c = k;
                }
            }
        }
        // `@morph:<target>`: the node's outline becomes the target's by the
        // amount (the target drawn in this node's space).
        if let Some((tid, amount)) = virt.iter().rev().find_map(|(p, v)| Some((p.strip_prefix("@morph:")?.parse::<NodeId>().ok()?, v.as_f64()?)))
            && amount != 0.0
            && let (Some(own), Some(target)) = (doc.find(id), doc.find(tid))
        {
            let merge = |n: &Node| PathData::new(crate::build::geometry(doc, n).into_iter().flat_map(|p| p.path.subpaths).collect());
            let to_own = doc.world_transform(id).unwrap_or(Affine::IDENTITY).inverse() * doc.world_transform(tid).unwrap_or(Affine::IDENTITY);
            let path = morph(&merge(own), &merge(target).transformed(to_own), amount);
            let was_type = matches!(node.kind, crate::doc::NodeKind::Text(_));
            node.kind = crate::doc::NodeKind::Path { path, rule: Default::default() };
            if was_type && node.appearance.items.is_empty() {
                node.appearance = crate::appearance::Appearance::fill(Color::BLACK);
            }
        }
        for (p, val) in virt {
            apply_virtual(node, &p, &val);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::NodeKind;
    use crate::shapes::LiveShape;
    use kurbo::Rect;

    #[test]
    fn numbers_colours_and_paths_interpolate() {
        assert_eq!(lerp(&json!(0.0), &json!(10.0), 0.25), json!(2.5));
        assert_eq!(lerp(&json!("#000000"), &json!("#ffffff"), 0.5), json!("#808080"));
        let a = crate::shapes::LiveShape::Rect { rect: Rect::new(0.0, 0.0, 10.0, 10.0), radii: [0.0; 4], corners: None }.to_path();
        let b = crate::shapes::LiveShape::Ellipse { rect: Rect::new(0.0, 0.0, 20.0, 20.0) }.to_path();
        let m = morph(&a, &b, 0.5);
        // One outline, resampled finely, its square corners kept as points.
        assert_eq!(m.subpaths.len(), 1);
        assert!(m.subpaths[0].anchors.len() >= 32);
        assert!(Ease::EaseInOut.apply(0.5) - 0.5 < 1e-9);
        assert!((Ease::Bezier(0.42, 0.0, 0.58, 1.0).apply(0.5) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn tracks_drive_node_properties() {
        let mut d = Document::new(100.0, 100.0);
        let mut n = Node::new(0, NodeKind::Shape { shape: LiveShape::Ellipse { rect: Rect::new(0.0, 0.0, 10.0, 10.0) } });
        n.appearance = crate::appearance::Appearance::fill(Color::BLACK);
        let id = d.add(None, n);
        d.animation.set_key(id, "/opacity", 0.0, json!(0.0), Ease::Linear);
        d.animation.set_key(id, "/opacity", 2.0, json!(1.0), Ease::Linear);
        d.animation.set_key(id, "@move", 0.0, json!([0, 0]), Ease::Linear);
        d.animation.set_key(id, "@move", 2.0, json!([50, 0]), Ease::Linear);
        d.animation.bindings.push(Binding { node: id, property: "/appearance/items/0/paint".into(), param: "tint".into() });
        let at = at_time(&d, 1.0, &|p| (p == "tint").then(|| json!("#ff0000")));
        let n = at.find(id).unwrap();
        assert!((n.opacity - 0.5).abs() < 1e-6);
        assert!((n.transform.translation().x - 25.0).abs() < 1e-9);
        assert_eq!(n.appearance.top_fill().map(|f| serde_json::to_value(&f.paint).unwrap()), Some(json!({"type": "solid", "color": "#ff0000"})));
    }
}
