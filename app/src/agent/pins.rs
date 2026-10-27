// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Puppet pins, pinned masks and corner pins for agents: the same
//! geometry the canvas's pin tool uses (`editor::pins`).
//!
//! A puppet pin's `[x, y]` is in the drawing's own pixels by default (its
//! top-left corner 0, 0, the way the picture was drawn), or canvas pixels
//! from the centre (y down) with `"space": "canvas"`, read through the
//! layer's placement at the moment `at` (seconds on the timeline; default
//! the clip's start), so a pin lands on the part of the picture that
//! shows there. Without `space`, a point that falls outside the drawing is
//! taken as canvas pixels, and the report says so. Other points (masks,
//! corner pins, `by`) are canvas pixels. A name (`top_left`, `centre`…) is
//! a point of the layer's box; `{"u": 0.3, "v": 0.6}` is a point of the
//! layer in its own 0..1 coordinates.
//!
//! * `puppet`: `pins` places pins; `pin` with `to` / `by` / `rotate` /
//!   `scale` / `amount` / `extent` changes one; `pin` with `keys`
//!   animates it; `remove` takes pins (or `"all"`) away.
//! * `mask_puppet`: the same on a mask's own pins (the mask becomes a
//!   path first), plus `point` (1-based) with `to` / `by` / `keys` to
//!   move or animate one point of its path.
//! * `corner_pin`: the four corners (`top_left`…), or `skew` in degrees;
//!   `keys` per corner; `reset`, `remove`.

use super::{Session, f, micros, secs};
use crate::editor::pins::{self as canvas_pins, LayerMap};
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_model as m;

#[path = "rig.rs"]
mod rig;
pub(super) use rig::{ACTS, contacts, known_act as is_act};
#[path = "stage_check.rs"]
mod stage_check;
#[path = "talk.rs"]
mod talk;

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn kind_of(v: Option<&Value>) -> Result<m::PinKind, String> {
    match v.and_then(Value::as_str).map(str::to_lowercase).as_deref() {
        None | Some("position" | "pin" | "deform") => Ok(m::PinKind::Position),
        Some("bend" | "rotate") => Ok(m::PinKind::Bend),
        Some("starch" | "stiff" | "stiffen") => Ok(m::PinKind::Starch),
        Some("overlap" | "front" | "in_front") => Ok(m::PinKind::Overlap),
        Some(other) => Err(format!("pin kind \"{other}\": position (holds and moves its point), bend (turns and scales around it), starch (stiffens around it), overlap (brings that part in front)")),
    }
}

/// What a pin's `[x, y]` is measured in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PinSpace {
    /// The drawing's own pixels, said outright.
    Drawing,
    /// Canvas pixels from the centre.
    Canvas,
    /// Not said: the drawing's pixels, or the canvas's for a point outside
    /// the drawing.
    Either,
}

fn space_of(op: &Value) -> Result<PinSpace, String> {
    match op.get("space").and_then(Value::as_str).map(str::to_lowercase).as_deref() {
        None => Ok(PinSpace::Either),
        Some("drawing" | "picture" | "image" | "art" | "source" | "local" | "layer") => Ok(PinSpace::Drawing),
        Some("canvas" | "screen" | "frame" | "comp") => Ok(PinSpace::Canvas),
        Some(other) => Err(format!("space \"{other}\": drawing (the picture's own pixels, top-left 0, 0; the default) or canvas (pixels from the canvas centre)")),
    }
}

fn kind_name(k: m::PinKind) -> &'static str {
    match k {
        m::PinKind::Position => "position",
        m::PinKind::Bend => "bend",
        m::PinKind::Starch => "starch",
        m::PinKind::Overlap => "overlap",
    }
}

/// Whose pins an op works on.
#[derive(Clone, Copy)]
enum Owner {
    Puppet,
    Mask(u64),
}

impl Owner {
    fn prefix(self, pin: u32) -> String {
        match self {
            Owner::Puppet => format!("puppet.{pin}."),
            Owner::Mask(mk) => format!("mask.{mk}.pin.{pin}."),
        }
    }

    fn pins(self, mc: &m::Clip) -> Vec<m::Pin> {
        match self {
            Owner::Puppet => mc.puppet.as_ref().map(|p| p.pins.clone()).unwrap_or_default(),
            Owner::Mask(mk) => mc.masks.iter().find(|k| k.id.0 == mk).map(|k| k.pins.clone()).unwrap_or_default(),
        }
    }

    fn pins_mut(self, mc: &mut m::Clip) -> Option<&mut Vec<m::Pin>> {
        match self {
            Owner::Puppet => Some(&mut mc.puppet.get_or_insert_with(m::Puppet::default).pins),
            Owner::Mask(mk) => mc.masks.iter_mut().find(|k| k.id.0 == mk).map(|k| &mut k.pins),
        }
    }
}

/// A key as an agent writes it: `[t, [x, y]]` or `{"t", "to" | "by",
/// "rotate", "scale", "amount", "extent", "ease"}`.
struct KeySpec {
    t: f64,
    to: Option<Value>,
    by: Option<[f64; 2]>,
    rotate: Option<f64>,
    scale: Option<f64>,
    amount: Option<f64>,
    extent: Option<f64>,
    ease: String,
}

fn pair(v: &Value) -> Option<[f64; 2]> {
    match v {
        Value::Array(a) if a.len() == 2 => Some([a[0].as_f64()?, a[1].as_f64()?]),
        _ => None,
    }
}

fn key_specs(v: &Value, ease: &str) -> Result<Vec<KeySpec>, String> {
    let Value::Array(keys) = v else { return Err("keys is a list: [[t, [x, y]], …] or [{\"t\": 0.5, \"to\": [x, y], \"rotate\": 20, \"ease\": \"overshoot\"}, …]".into()) };
    let mut out = Vec::new();
    for k in keys {
        let spec = match k {
            Value::Array(a) if a.len() == 2 => KeySpec { t: a[0].as_f64().ok_or("a key's time is seconds")?, to: Some(a[1].clone()), by: None, rotate: None, scale: None, amount: None, extent: None, ease: ease.to_owned() },
            Value::Object(o) => KeySpec {
                t: o.get("t").and_then(Value::as_f64).ok_or("each key needs \"t\" (seconds from the clip's start)")?,
                to: o.get("to").cloned(),
                by: o.get("by").and_then(pair),
                rotate: o.get("rotate").or(o.get("rotation")).and_then(Value::as_f64),
                scale: o.get("scale").and_then(Value::as_f64),
                amount: o.get("amount").and_then(Value::as_f64),
                extent: o.get("extent").and_then(Value::as_f64),
                ease: o.get("ease").and_then(Value::as_str).unwrap_or(ease).to_owned(),
            },
            _ => return Err("a key is [t, [x, y]] or {\"t\": …, \"to\": [x, y]}".into()),
        };
        if super::craft::curve(&spec.ease).is_none() {
            return Err(format!("ease \"{}\": smooth, linear, hold, ease_in_out, ease_in, ease_out, overshoot, anticipate, snap, whip, or a GSAP curve (elastic.out, back(2)…)", spec.ease));
        }
        out.push(spec);
    }
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    Ok(out)
}

/// A track of `(time, value, ease)` with each key's ease shaping the
/// move that starts at it (as `animate` builds them).
fn track(keys: &[(m::Micros, m::Value, String)]) -> m::Track {
    let mut out = Vec::new();
    for (i, (t, v, e)) in keys.iter().enumerate() {
        let (interp, o, _, shape) = super::craft::curve(e).expect("checked");
        let mut key = m::Keyframe::new(*t, v.clone(), interp);
        key.curve = shape;
        if let Some(o) = o {
            key.ease_out = o;
        }
        if i > 0 {
            if let Some((_, _, Some(inn), _)) = super::craft::curve(&keys[i - 1].2) {
                key.ease_in = inn;
            }
        }
        out.push(key);
    }
    m::Track::new(out)
}

impl Session {
    fn layer_map(&self, id: u32, t: view::Time) -> Result<LayerMap, String> {
        LayerMap::at(&self.doc.project, &self.doc.model, id, t).ok_or_else(|| format!("clip {id} draws nothing on the canvas at {:.2}s (a sound, an adjustment layer, or empty text)", secs(t)))
    }

    /// A point given as canvas pixels, a name of the layer's box, or
    /// layer fractions, in layer coordinates (`flat`: before the corner
    /// pin, as corners are).
    pub(super) fn layer_point(map: &LayerMap, spec: &Value, flat: bool) -> Result<m::Vec2, String> {
        let canvas = |x: f64, y: f64| {
            let c = eframe::egui::Vec2::new(x as f32, y as f32);
            if flat { map.flat_layer(c) } else { map.layer(c) }.ok_or_else(|| "the layer has no size there (scale 0?)".to_owned())
        };
        match spec {
            Value::Array(a) if a.len() == 2 => canvas(a[0].as_f64().ok_or("[x, y] are numbers")?, a[1].as_f64().ok_or("[x, y] are numbers")?),
            Value::String(s) => {
                let (u, v) = crate::editor::xform::named_point(s).ok_or_else(|| format!("point \"{s}\": [x, y] canvas pixels from the centre, a name (centre, top_left, top, top_right, left, right, bottom_left, bottom, bottom_right) or {{\"u\": 0..1, \"v\": 0..1}} across the layer"))?;
                Ok(map.content(u, v))
            }
            Value::Object(o) if o.contains_key("u") || o.contains_key("v") => Ok(m::Vec2::new(o.get("u").and_then(Value::as_f64).unwrap_or(0.5), o.get("v").and_then(Value::as_f64).unwrap_or(0.5))),
            Value::Object(o) if o.contains_key("x") && o.contains_key("y") => canvas(o["x"].as_f64().unwrap_or(0.0), o["y"].as_f64().unwrap_or(0.0)),
            _ => Err("a point is [x, y] canvas pixels from the centre, a name (top_left, centre…) or {\"u\": 0..1, \"v\": 0..1} across the layer".into()),
        }
    }

    /// A puppet pin's point in layer coordinates: `[x, y]` (or `{"x", "y"}`)
    /// in the drawing's own pixels or the canvas's, as `space` says (see
    /// the module notes); names and `{"u", "v"}` as [`Session::layer_point`].
    /// The flag says the point was taken as canvas pixels for want of a
    /// `space`.
    fn pin_point(map: &LayerMap, spec: &Value, space: PinSpace) -> Result<(m::Vec2, bool), String> {
        let xy = match spec {
            Value::Array(a) if a.len() == 2 => Some((a[0].as_f64().ok_or("[x, y] are numbers")?, a[1].as_f64().ok_or("[x, y] are numbers")?)),
            Value::Object(o) if o.contains_key("x") && o.contains_key("y") && !o.contains_key("u") => Some((o["x"].as_f64().unwrap_or(0.0), o["y"].as_f64().unwrap_or(0.0))),
            _ => None,
        };
        let Some((x, y)) = xy.filter(|_| space != PinSpace::Canvas) else { return Self::layer_point(map, spec, false).map(|p| (p, false)) };
        let size = map.drawing_size();
        let inside = x >= -0.5 && y >= -0.5 && x <= size.x as f64 + 0.5 && y <= size.y as f64 + 0.5;
        if space == PinSpace::Either && !inside {
            return Self::layer_point(map, spec, false).map(|p| (p, true));
        }
        Ok((map.drawing(x as f32, y as f32), false))
    }

    /// Canvas pixels moved by `by` from where `p` (layer coordinates)
    /// shows.
    pub(super) fn moved_by(map: &LayerMap, p: m::Vec2, by: [f64; 2], flat: bool) -> Result<m::Vec2, String> {
        let c = if flat { map.flat_canvas(p) } else { map.canvas(p) };
        Self::layer_point(map, &json!([c.x as f64 + by[0], c.y as f64 + by[1]]), flat)
    }

    /// The clip, its timeline start and length, and the moment `at`.
    pub(super) fn pin_target(&self, op: &Value, what: &str) -> Result<(u32, m::Clip, view::Clip, view::Time), String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or_else(|| format!("{what} needs \"clip\""))? as u32;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let vc = self.clip(id)?;
        let at = f(op, "at").map(micros).unwrap_or(vc.start);
        if at < vc.start || at > vc.start + vc.dur {
            return Err(format!("\"at\" {:.2}s is outside clip {id} ({:.2}–{:.2}s)", secs(at), secs(vc.start), secs(vc.start + vc.dur)));
        }
        Ok((id, mc, vc, at))
    }

    fn find_pin(pins: &[m::Pin], v: &Value) -> Result<u32, String> {
        let found = match v {
            Value::Number(n) => n.as_u64().and_then(|id| pins.iter().find(|p| p.id as u64 == id)),
            Value::String(s) => pins.iter().find(|p| p.name.eq_ignore_ascii_case(s)).or_else(|| s.parse::<u32>().ok().and_then(|id| pins.iter().find(|p| p.id == id))),
            _ => None,
        };
        found.map(|p| p.id).ok_or_else(|| {
            let have: Vec<String> = pins.iter().map(|p| if p.name.is_empty() { p.id.to_string() } else { format!("{} \"{}\"", p.id, p.name) }).collect();
            if have.is_empty() { "there are no pins yet: place some with \"pins\"".to_owned() } else { format!("no pin {v}; pins: {}", have.join(", ")) }
        })
    }

    /// The pins of `owner` as a line each: id, name, kind and where it
    /// shows on the canvas at `t`.
    fn pins_said(&self, id: u32, mc: &m::Clip, owner: Owner, t: view::Time) -> Vec<Value> {
        let Ok(map) = self.layer_map(id, t) else { return Vec::new() };
        let local = self.clip(id).map_or(0, |vc| t - vc.start);
        let pins = match owner {
            Owner::Puppet => mc.puppet_at(local).map(|p| p.pins).unwrap_or_default(),
            Owner::Mask(mk) => mc.masks.iter().find(|k| k.id.0 == mk).map(|k| mc.mask_at(k, local).pins).unwrap_or_default(),
        };
        pins.iter()
            .map(|p| {
                let c = map.canvas(p.position);
                let mut o = json!({"pin": p.id, "kind": kind_name(p.kind), "at": [r1(c.x), r1(c.y)]});
                if !p.name.is_empty() {
                    o["name"] = json!(p.name);
                }
                if p.kind == m::PinKind::Bend {
                    o["rotate"] = json!(p.rotation);
                    o["scale"] = json!((p.scale * 1000.0).round() / 10.0);
                }
                if mc.keyframes.keys().any(|k| k.starts_with(&owner.prefix(p.id))) {
                    o["animated"] = json!(true);
                }
                o
            })
            .collect()
    }

    /// Places, changes, animates and removes the pins of `owner` on `mc`
    /// as `op` says. Returns what was done.
    fn edit_pins(&self, op: &Value, id: u32, mc: &mut m::Clip, vc: &view::Clip, at: view::Time, owner: Owner) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        let frame = (SEC as f64 / (self.doc.project.fps as f64).max(1.0)) as view::Time;
        let map = self.layer_map(id, at)?;
        let local = at - vc.start;
        // Puppet pins read `[x, y]` in the drawing's pixels; mask pins, as
        // the mask's points, in canvas pixels.
        let space = match owner {
            Owner::Puppet => space_of(op)?,
            Owner::Mask(_) => PinSpace::Canvas,
        };
        let mut as_canvas = 0usize;
        if let Some(r) = op.get("remove") {
            let all = r.as_str() == Some("all");
            let targets: Vec<u32> = if all {
                owner.pins(mc).iter().map(|p| p.id).collect()
            } else {
                let list = match r {
                    Value::Array(a) => a.clone(),
                    v => vec![v.clone()],
                };
                list.iter().map(|v| Self::find_pin(&owner.pins(mc), v)).collect::<Result<_, _>>()?
            };
            for pid in &targets {
                if let Some(pins) = owner.pins_mut(mc) {
                    pins.retain(|p| p.id != *pid);
                }
                let prefix = owner.prefix(*pid);
                mc.keyframes.retain(|k, _| !k.starts_with(&prefix));
            }
            if matches!(owner, Owner::Puppet) && mc.puppet.as_ref().is_some_and(|p| p.pins.is_empty()) {
                mc.puppet = None;
            }
            said.push(if all { "removed every pin".to_owned() } else { format!("removed pin(s) {targets:?}") });
        }
        if let Some(list) = op.get("pins") {
            let Value::Array(list) = list else { return Err("pins is a list: [{\"name\": \"left hand\", \"at\": [x, y]}, …] or [[x, y], …]".into()) };
            for p in list {
                let (spec, name, kind) = match p {
                    Value::Object(o) => {
                        let spec = o.get("at").or(o.get("point")).cloned().or_else(|| (o.contains_key("x") || o.contains_key("u")).then(|| p.clone())).ok_or("each pin needs \"at\": [x, y] canvas pixels, a name of the layer's box or {\"u\", \"v\"}")?;
                        (spec, o.get("name").and_then(Value::as_str).unwrap_or("").to_owned(), kind_of(o.get("kind"))?)
                    }
                    other => (other.clone(), String::new(), m::PinKind::Position),
                };
                let (at_layer, fell_back) = Self::pin_point(&map, &spec, space)?;
                as_canvas += fell_back as usize;
                let pins = owner.pins_mut(mc).ok_or("that mask is gone")?;
                let pid = pins.iter().map(|q| q.id).max().map_or(1, |x| x + 1);
                let mut pin = m::Pin::new(pid, kind, at_layer);
                pin.name = name;
                if let Value::Object(o) = p {
                    for (k, key) in [("rotate", "rotation"), ("rotation", "rotation"), ("amount", "amount"), ("extent", "extent")] {
                        if let Some(x) = o.get(k).and_then(Value::as_f64) {
                            let x = if key == "extent" && x > 1.0 { x / 100.0 } else if key == "amount" && x.abs() > 1.0 { x / 100.0 } else { x };
                            pin.set(key, &if key == "rotation" { m::Value::Angle(x) } else { m::Value::Scalar(x) });
                        }
                    }
                    if let Some(x) = o.get("scale").and_then(Value::as_f64) {
                        pin.scale = x / 100.0;
                    }
                }
                pins.push(pin);
            }
            said.push(format!("placed {} pin(s){}", list.len(), match (owner, space) {
                (Owner::Mask(_), _) => "",
                (_, PinSpace::Canvas) => " (canvas pixels)",
                _ if as_canvas > 0 => " (in the drawing's pixels; points outside the drawing were read as canvas pixels: say \"space\": \"drawing\" or \"canvas\")",
                _ => " (in the drawing's own pixels; \"space\": \"canvas\" for canvas pixels)",
            }));
        }
        if let Some(which) = op.get("pin") {
            let pid = Self::find_pin(&owner.pins(mc), which)?;
            let prefix = owner.prefix(pid);
            let pin = owner.pins(mc).into_iter().find(|p| p.id == pid).expect("found");
            let now = |mc: &m::Clip, key: &str, t: view::Time| mc.property_at(&format!("{prefix}{key}"), t);
            if let Some(keys) = op.get("keys") {
                let ease = op.get("ease").and_then(Value::as_str).unwrap_or("ease_in_out");
                let timeline = op.get("timeline").and_then(Value::as_bool).unwrap_or(false);
                let specs = key_specs(keys, ease)?;
                let mut tracks: std::collections::BTreeMap<&str, Vec<(m::Micros, m::Value, String)>> = Default::default();
                for k in &specs {
                    let t = if timeline { micros(k.t) - vc.start } else { micros(k.t) };
                    if t < 0 || t > vc.dur {
                        return Err(format!("key at {:.2}s is outside the clip (0–{:.2}s from its start; \"timeline\": true takes timeline times)", k.t, secs(vc.dur)));
                    }
                    let kmap = self.layer_map(id, vc.start + t)?;
                    let place = match (&k.to, k.by) {
                        (Some(to), _) => Some(Self::pin_point(&kmap, to, space)?.0),
                        (None, Some(by)) => Some(Self::moved_by(&kmap, pin.rest, by, false)?),
                        _ => None,
                    };
                    if let Some(p) = place {
                        tracks.entry("position").or_default().push((t, m::Value::Point(p), k.ease.clone()));
                    }
                    if let Some(r) = k.rotate {
                        tracks.entry("rotation").or_default().push((t, m::Value::Angle(r), k.ease.clone()));
                    }
                    if let Some(s) = k.scale {
                        tracks.entry("scale").or_default().push((t, m::Value::Scalar(s / 100.0), k.ease.clone()));
                    }
                    if let Some(a) = k.amount {
                        tracks.entry("amount").or_default().push((t, m::Value::Scalar(if a.abs() > 1.0 { a / 100.0 } else { a }), k.ease.clone()));
                    }
                    if let Some(e) = k.extent {
                        tracks.entry("extent").or_default().push((t, m::Value::Scalar(if e > 1.0 { e / 100.0 } else { e }), k.ease.clone()));
                    }
                }
                if tracks.is_empty() {
                    return Err("keys need a value: \"to\": [x, y] (or \"by\"), \"rotate\", \"scale\", \"amount\" or \"extent\"".into());
                }
                for (key, ks) in &tracks {
                    mc.keyframes.insert(format!("{prefix}{key}"), track(ks));
                }
                said.push(format!("pin {pid}: {} key(s) on {}", specs.len(), tracks.keys().copied().collect::<Vec<_>>().join(", ")));
            }
            let mut set = |mc: &mut m::Clip, key: &str, v: m::Value| xform_put(mc, &format!("{prefix}{key}"), v, local, frame);
            if let Some(to) = op.get("to") {
                set(mc, "position", m::Value::Point(Self::pin_point(&map, to, space)?.0));
                said.push(format!("pin {pid} moved to {to}"));
            } else if let Some(by) = op.get("by").and_then(pair) {
                let cur = match now(mc, "position", local) {
                    Some(m::Value::Point(p)) => p,
                    _ => pin.position,
                };
                set(mc, "position", m::Value::Point(Self::moved_by(&map, cur, by, false)?));
                said.push(format!("pin {pid} moved by {by:?} px"));
            }
            if let Some(r) = op.get("rotate").or(op.get("rotation")).and_then(Value::as_f64) {
                set(mc, "rotation", m::Value::Angle(r));
                said.push(format!("pin {pid} turned {r}°"));
            }
            if let Some(s) = op.get("scale").and_then(Value::as_f64) {
                set(mc, "scale", m::Value::Scalar(s / 100.0));
            }
            if let Some(a) = op.get("amount").and_then(Value::as_f64) {
                set(mc, "amount", m::Value::Scalar(if a.abs() > 1.0 { a / 100.0 } else { a }));
            }
            if let Some(e) = op.get("extent").and_then(Value::as_f64) {
                set(mc, "extent", m::Value::Scalar(if e > 1.0 { e / 100.0 } else { e }));
            }
            if let Some(n) = op.get("name").and_then(Value::as_str) {
                if let Some(p) = owner.pins_mut(mc).and_then(|ps| ps.iter_mut().find(|p| p.id == pid)) {
                    p.name = n.to_owned();
                }
            }
        } else if op.get("point").is_none() {
            for k in ["to", "by", "keys", "rotate", "scale", "amount", "extent"] {
                if op.get(k).is_some() {
                    return Err(format!("\"{k}\" changes one pin: say which with \"pin\" (its id or name)"));
                }
            }
        }
        Ok(said)
    }

    /// `puppet`: pins that bend a layer like a puppet-pin tool.
    pub(super) fn puppet(&mut self, op: &Value) -> Result<String, String> {
        // A character with bones acts on them (`bones`).
        if let Some(r) = self.bones_first(op) {
            return r;
        }
        let (id, mut mc, vc, at) = self.pin_target(op, "puppet")?;
        let mut op = op.clone();
        let mut rigged = None;
        if let Some(kind) = op.get("rig") {
            let kind = kind.as_str().ok_or_else(|| format!("rig names a kind: {}", rig::KINDS))?.to_lowercase();
            if matches!(kind.as_str(), "backdrop" | "background" | "scene" | "place") {
                if let Some(o) = op.as_object_mut() {
                    if mc.puppet.as_ref().is_some_and(|p| !p.pins.is_empty()) {
                        o.insert("remove".into(), json!("all"));
                    }
                    o.insert("pins".into(), Value::Array(rig::backdrop()));
                }
                rigged = Some("rigged as a backdrop: the ground and lower half held, the top and sides free (act \"breeze\" or \"sway\")".to_owned());
            }
        }
        if let Some(kind) = op.get("rig").filter(|_| rigged.is_none()) {
            let kind = kind.as_str().unwrap_or_default().to_lowercase();
            let shape = self.shape_of(&mc)?;
            let guess = shape.facing();
            // `facing` is how it looks on the canvas now; the rig works on
            // the picture, which a flip mirrors.
            let right = match op.get("facing").and_then(Value::as_str) {
                Some("right") => true,
                Some("left") => false,
                Some(other) => return Err(format!("facing \"{other}\": left or right (as it looks on the canvas now)")),
                None => guess.unwrap_or(true) != mc.transform.flip_x,
            } != mc.transform.flip_x;
            let pins = rig::rig(&kind, &shape, right)?;
            if let Some(o) = op.as_object_mut() {
                if mc.puppet.as_ref().is_some_and(|p| !p.pins.is_empty()) {
                    o.insert("remove".into(), json!("all"));
                }
                o.insert("pins".into(), Value::Array(pins));
            }
            rigged = Some(format!(
                "rigged as {kind} from {}{}",
                shape.from,
                if op.get("facing").is_none() && guess.is_none() { " (it reads as facing the camera; say \"facing\" for a side view)" } else { "" }
            ));
        }
        let mut said = self.edit_pins(&op, id, &mut mc, &vc, at, Owner::Puppet)?;
        let did_rig = rigged.is_some();
        if let Some(r) = rigged {
            said.insert(0, r);
        }
        // A voice sets the window: the line, where it plays over this clip.
        if let Some(vid) = op.get("voice").and_then(Value::as_u64) {
            if f(&op, "from").is_none() && f(&op, "duration").is_none() {
                let (from, dur) = self.voice_window(id, vid as u32)?;
                if let Some(o) = op.as_object_mut() {
                    o.insert("from".into(), json!(from));
                    o.insert("duration".into(), json!(dur));
                }
            }
        }
        let before_acts = mc.clone();
        let mut talking = false;
        let mut travel = None;
        if let Some(a) = op.get("act") {
            let acts: Vec<String> = match a {
                Value::String(s) => vec![s.to_lowercase()],
                Value::Array(l) => l.iter().filter_map(Value::as_str).map(str::to_lowercase).collect(),
                _ => Vec::new(),
            };
            if acts.is_empty() {
                return Err(format!("act names one motion or a list of them: {}", rig::ACTS));
            }
            if let Some(bad) = acts.iter().find(|a| !rig::known_act(a)) {
                return Err(format!("act \"{bad}\": {}", rig::ACTS));
            }
            talking = acts.iter().any(|a| a == "talk");
            let pins = mc.puppet.as_ref().map(|p| p.pins.clone()).unwrap_or_default();
            if pins.is_empty() {
                return Err(format!("clip {id} has no pins to act with: add \"rig\" (e.g. \"rig\": \"bird\") or place pins first"));
            }
            let len = secs(vc.dur);
            let from = f(&op, "from").unwrap_or(0.0).clamp(0.0, len);
            let dur = f(&op, "duration").unwrap_or(len - from).min(len - from);
            if dur < 0.5 {
                return Err(format!("act needs at least half a second of the clip ({:.2}s from {from:.2}s)", dur.max(0.0)));
            }
            let speed = f(&op, "speed").unwrap_or(1.0).clamp(0.2, 4.0);
            let right = Self::picture_faces_right(&pins).or_else(|| op.get("facing").and_then(Value::as_str).map(|s| (s == "right") != mc.transform.flip_x)).unwrap_or(true);
            let px = self.layer_map(id, at)?.pixels();
            let aspect = (px.x / px.y.max(1.0)) as f64;
            let mut moved = Vec::new();
            for pin in &pins {
                let Some(ks) = rig::keys(&acts, &pin.name, (pin.rest.x, pin.rest.y), from, dur, right, aspect, speed, id as u64) else { continue };
                // Keep this pin's keys outside the window (another act).
                let path = format!("puppet.{}.position", pin.id);
                let mut list: Vec<Value> = mc
                    .keyframes
                    .get(&path)
                    .map(|tr| {
                        tr.keys
                            .iter()
                            .filter(|k| secs(k.time) < from - 1e-3 || secs(k.time) > from + dur + 1e-3)
                            .filter_map(|k| match &k.value {
                                m::Value::Point(p) => Some(json!({"t": secs(k.time), "to": {"u": p.x, "v": p.y}, "ease": "smooth"})),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                list.extend(ks.into_iter().map(|(t, u, v)| json!({"t": t, "to": {"u": u, "v": v}, "ease": "smooth"})));
                self.edit_pins(&json!({"pin": pin.id, "keys": list}), id, &mut mc, &vc, at, Owner::Puppet)?;
                moved.push(if pin.name.is_empty() { pin.id.to_string() } else { pin.name.clone() });
            }
            if moved.is_empty() {
                return Err(format!("none of the pins ({}) take part in {}: name pins head, body, tail, front_foot, hand, top… or use \"rig\"", pins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "), acts.join(" + ")));
            }
            said.push(format!("{} from {from:.2}s for {dur:.2}s of the clip on {}", acts.join(" + "), moved.join(", ")));
            super::foley::remember(&mut mc, &acts, from, dur, speed);
            // Face pins took the head's motion alone: put their own back.
            talk::refollow(&before_acts, &mut mc);
            if let Some(dx) = f(&op, "travel") {
                travel = Some((from, dur, dx));
            } else if let Some(pace) = acts.iter().find_map(|a| rig::planted_pace(a, speed)) {
                // The pace at which the planted foot stays still on the ground.
                let map = self.layer_map(id, at)?;
                let tall = (map.canvas(m::Vec2::new(0.5, 1.0)).y - map.canvas(m::Vec2::new(0.5, 0.0)).y).abs() as f64;
                let dx = (pace * tall * dur).round();
                said.push(format!("the legs step in place: add \"travel\": {} (px over the window, the sign the way it faces) and the planted foot stays still on the ground", if right != mc.transform.flip_x { dx } else { -dx }));
            }
        }
        let len = secs(vc.dur);
        let from = f(&op, "from").unwrap_or(0.0).clamp(0.0, len);
        let window = (from, f(&op, "duration").unwrap_or(len - from).min(len - from).max(0.0));
        said.extend(self.face_work(&op, id, &mut mc, &vc, at, window, talking)?);
        let mut op = op;
        if let Some(o) = op.as_object_mut() {
            o.remove("act");
        }
        // Cuts: drawn by the agent, or found from the silhouette (a rig
        // proposes the legs, arms and mouth).
        let rig_kind = op.get("rig").and_then(Value::as_str).map(str::to_lowercase).filter(|_| did_rig);
        said.extend(self.cut_work(&op, id, &mut mc, at, rig_kind.as_deref())?);
        if let Some(pp) = &mut mc.puppet {
            if let Some(d) = f(&op, "density") {
                pp.density = d.clamp(4.0, 64.0);
                said.push(format!("mesh {d}"));
            }
            if let Some(e) = f(&op, "expansion") {
                pp.expansion = if e > 1.0 { e / 100.0 } else { e };
            }
            if let Some(b) = op.get("enabled").and_then(Value::as_bool) {
                pp.enabled = b;
                said.push(if b { "puppet on".into() } else { "puppet off (pins kept)".into() });
            }
        } else if op.get("density").is_some() || op.get("enabled").is_some() {
            return Err(format!("clip {id} has no pins yet: place some with \"pins\""));
        }
        if said.is_empty() {
            return Err("puppet does nothing without pins (place), pin + to / by / keys / rotate (change one), or remove".into());
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Puppet", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let mc = self.model_clip(id).ok_or("clip gone")?;
        let faces = mc.puppet.as_ref().and_then(|p| Self::picture_faces_right(&p.pins)).map(|r| r != mc.transform.flip_x);
        if let Some((from, dur, dx)) = travel {
            let prop = super::props::resolve(&mc, "x")?;
            let path = prop.paths[0].clone();
            let x0 = super::props::value_or_default(&mc, &path).map(|v| super::props::to_friendly(&prop, &path, &v)).and_then(|v| v.as_f64()).unwrap_or(0.0);
            self.animate(&json!({"clip": id, "property": "x", "keys": [[from, x0], [from + dur, x0 + dx]], "ease": "linear"}))?;
            said.push(format!("travels {dx:+} px across"));
            if faces.is_some_and(|r| r != (dx > 0.0)) {
                said.push("it faces away from where it travels (walking backwards): flip it with set \"flip\": true, then act again".into());
            }
        }
        // Footsteps (or paddles, landings) on the frames the feet land.
        if let Some(g) = op.get("foley").filter(|g| !matches!(g, Value::Bool(false) | Value::Null)) {
            let mut fo = json!({"clip": id});
            if let Some(g) = g.as_str() {
                fo["surface"] = json!(g);
            }
            said.push(self.foley(&fo)?);
        }
        let mc = self.model_clip(id).ok_or("clip gone")?;
        let pins = self.pins_said(id, &mc, Owner::Puppet, at);
        let lone = pins.iter().filter(|p| p["kind"] == "position").count() == 1;
        Ok(format!(
            "{}; pins now (canvas px at {:.2}s): {}{}{}",
            said.join("; "),
            secs(at),
            pins_line(&pins),
            match faces {
                Some(true) => ". It reads as facing right on the canvas (from its pins: check with frame, and rig again with \"facing\" if not)",
                Some(false) => ". It reads as facing left on the canvas (from its pins: check with frame, and rig again with \"facing\" if not)",
                None => "",
            },
            if lone { ". One position pin moves the whole layer: add a second one to hold the rest of it" } else { "" }
        ))
    }

    /// Rigged characters sharing the screen with one facing away from all
    /// the others (a conversation that reads as turned backs).
    pub(super) fn facing_problems(&self) -> Vec<Value> {
        let p = &self.doc.project;
        let rigged: Vec<(u32, view::Time, view::Time)> = p
            .clips
            .iter()
            .filter(|c| self.model_clip(c.id).and_then(|mc| mc.puppet).is_some_and(|pp| pp.enabled && Self::picture_faces_right(&pp.pins).is_some()))
            .map(|c| (c.id, c.start, c.start + c.dur))
            .collect();
        // Where a character stands (its pins' middle, canvas x) and which
        // way it faces at `t`.
        let place = |id: u32, t: view::Time| -> Option<(f32, bool)> {
            let mc = self.model_clip(id)?;
            let vc = self.clip(id).ok()?;
            let local = t - vc.start;
            let pins = mc.puppet_at(local)?.pins;
            let map = self.layer_map(id, t).ok()?;
            let x = pins.iter().map(|q| map.canvas(q.position).x).sum::<f32>() / pins.len().max(1) as f32;
            let right = Self::picture_faces_right(&mc.puppet.as_ref()?.pins)? != mc.transform_at(local).flip_x;
            Some((x, right))
        };
        let mut out = Vec::new();
        for &(id, s, e) in &rigged {
            let others: Vec<u32> = rigged.iter().filter(|o| o.0 != id && o.1 < e && o.2 > s).map(|o| o.0).collect();
            let Some(t0) = rigged.iter().filter(|o| others.contains(&o.0)).map(|o| o.1.max(s)).min() else { continue };
            let t1 = rigged.iter().filter(|o| others.contains(&o.0)).map(|o| o.2.min(e)).max().unwrap_or(t0);
            let t = t0 + (t1 - t0) / 2;
            let Some((x, right)) = place(id, t) else { continue };
            let xs: Vec<f32> = others.iter().filter_map(|&o| place(o, t).map(|v| v.0)).collect();
            if xs.is_empty() {
                continue;
            }
            let side = if xs.iter().all(|&ox| ox > x + 20.0) {
                Some(true)
            } else if xs.iter().all(|&ox| ox < x - 20.0) {
                Some(false)
            } else {
                None
            };
            if side.is_some_and(|s| s != right) {
                let (faces, them) = if right { ("right", "left") } else { ("left", "right") };
                out.push(json!({
                    "kind": "faces_away",
                    "problem": format!("clip {id} faces {faces} while the characters it shares the screen with are all to its {them} (at {:.2}s): it turns its back on them", secs(t)),
                    "fix": "set \"flip\": true on it (its pins and motion turn with it) so the characters face each other",
                    "at": (secs(t) * 100.0).round() / 100.0,
                    "clip": id
                }));
            }
        }
        out
    }

    /// Which way the picture faces, read from its pins: the head sits in
    /// front of the body (`None` without both, or when level).
    fn picture_faces_right(pins: &[m::Pin]) -> Option<bool> {
        let named = |want: &[&str]| pins.iter().find(|p| want.iter().any(|w| p.name.to_lowercase().contains(w))).map(|p| p.rest.x);
        let head = named(&["head", "beak", "face"])?;
        let body = named(&["body", "chest", "back", "hips", "torso"])?;
        ((head - body).abs() > 0.03).then_some(head > body)
    }

    /// The character's silhouette: its cut-out, or the picture's own
    /// transparency or plain backdrop.
    /// The silhouette grid of a layer (width, height, cells, how it was
    /// found, which way it faces in its picture).
    pub(super) fn shape_grid(&self, mc: &m::Clip) -> Result<(usize, usize, Vec<bool>, &'static str, Option<bool>), String> {
        let s = self.shape_of(mc)?;
        let (w, h, on) = s.grid();
        Ok((w, h, on.to_vec(), s.from, s.facing()))
    }

    fn shape_of(&self, mc: &m::Clip) -> Result<rig::Shape, String> {
        let c = &mc.transform.crop;
        let cropped = |s: rig::Shape| s.cropped(c.left, c.top, c.right, c.bottom);
        self.whole_shape(mc).and_then(|s| cropped(s).ok_or_else(|| "the crop leaves nothing of the character to rig".to_owned()))
    }

    fn whole_shape(&self, mc: &m::Clip) -> Result<rig::Shape, String> {
        if let Some(mt) = crate::services::cutout::matte_of(mc) {
            let file = mt.get("file").and_then(Value::as_str).unwrap_or_default();
            let (w, h) = (mt.get("width").and_then(Value::as_u64).unwrap_or(0) as usize, mt.get("height").and_then(Value::as_u64).unwrap_or(0) as usize);
            let invert = mt.get("invert").and_then(Value::as_bool).unwrap_or(false);
            if let Ok(plane) = studio_ai::features::matte_file::Reader::open(std::path::Path::new(file)).and_then(|mut r| r.plane(0)) {
                if let Some(s) = rig::Shape::from_coverage(w, h, &plane, invert, "its cut-out") {
                    return Ok(s);
                }
            }
        }
        if let m::ClipSource::Media(r) = &mc.source {
            if let Some(path) = self.doc.project.media(r.media.0 as u32).and_then(|mi| mi.path.clone()) {
                if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png")) {
                    if let Some(s) = rig::Shape::of_png(&path) {
                        return Ok(s);
                    }
                }
            }
        }
        Err("can't find this layer's silhouette (no transparency, plain backdrop or cut-out): cut it out first (cut_out), or place pins yourself with \"pins\"".into())
    }

    /// `mask_puppet`: pins and point moves on a mask's outline.
    pub(super) fn mask_puppet(&mut self, op: &Value) -> Result<String, String> {
        let (id, mut mc, vc, at) = self.pin_target(op, "mask_puppet")?;
        let n = op.get("mask").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let mk = mc.masks.get(n - 1).cloned().ok_or_else(|| format!("clip {id} has {} mask(s); add one with the mask op first", mc.masks.len()))?;
        let mut said = Vec::new();
        if mk.shape != m::MaskShape::Path {
            let path = canvas_pins::as_path(&mut mc, &mk).ok_or("a line or mirror mask has no outline to pin; use a rectangle, ellipse, star, heart or path")?;
            mc.masks[n - 1] = path;
            said.push(format!("mask{n} is a path now ({} points)", mc.masks[n - 1].path.len()));
        }
        let mid = mk.id.0;
        said.extend(self.edit_pins(op, id, &mut mc, &vc, at, Owner::Mask(mid))?);
        if let Some(p) = op.get("point") {
            let count = mc.masks[n - 1].path.len();
            let i = p.as_u64().filter(|i| *i >= 1 && (*i as usize) <= count).ok_or_else(|| format!("point is 1 to {count} (the path's points, clockwise from the top)"))? as usize - 1;
            let path = format!("mask.{mid}.point.{i}");
            let frame = (SEC as f64 / (self.doc.project.fps as f64).max(1.0)) as view::Time;
            let map = self.layer_map(id, at)?;
            let cur = match mc.property_at(&path, at - vc.start) {
                Some(m::Value::Point(q)) => q,
                _ => mc.masks[n - 1].path[i].point,
            };
            if let Some(keys) = op.get("keys") {
                let specs = key_specs(keys, op.get("ease").and_then(Value::as_str).unwrap_or("ease_in_out"))?;
                let mut ks = Vec::new();
                for k in &specs {
                    let t = micros(k.t);
                    let kmap = self.layer_map(id, vc.start + t.clamp(0, vc.dur))?;
                    let v = match (&k.to, k.by) {
                        (Some(to), _) => Self::layer_point(&kmap, to, false)?,
                        (None, Some(by)) => Self::moved_by(&kmap, cur, by, false)?,
                        _ => return Err("a point's keys need \"to\" or \"by\"".into()),
                    };
                    ks.push((t, m::Value::Point(v), k.ease.clone()));
                }
                mc.keyframes.insert(path.clone(), track(&ks));
                said.push(format!("point {} animated ({} keys)", i + 1, ks.len()));
            } else if let Some(to) = op.get("to") {
                xform_put(&mut mc, &path, m::Value::Point(Self::layer_point(&map, to, false)?), at - vc.start, frame);
                said.push(format!("point {} moved", i + 1));
            } else if let Some(by) = op.get("by").and_then(pair) {
                xform_put(&mut mc, &path, m::Value::Point(Self::moved_by(&map, cur, by, false)?), at - vc.start, frame);
                said.push(format!("point {} moved by {by:?} px", i + 1));
            }
        }
        if said.is_empty() {
            return Err("mask_puppet does nothing without pins, pin + to / by / keys, point + to / by / keys, or remove".into());
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Mask puppet", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let mc = self.model_clip(id).ok_or("clip gone")?;
        let map = self.layer_map(id, at)?;
        let shown = mc.mask_at(&mc.masks[n - 1], at - vc.start);
        let points: Vec<[f64; 2]> = shown.path.iter().map(|p| map.canvas(p.point)).map(|c| [r1(c.x), r1(c.y)]).collect();
        Ok(format!("{}; mask{n} points (canvas px, before its pins bend them): {}; pins: {}", said.join("; "), json!(points), json!(self.pins_said(id, &mc, Owner::Mask(mid), at))))
    }

    /// `corner_pin`: the layer's corners placed anywhere (perspective), or
    /// skewed.
    pub(super) fn corner_pin(&mut self, op: &Value) -> Result<String, String> {
        let (id, mut mc, vc, at) = self.pin_target(op, "corner_pin")?;
        let map = self.layer_map(id, at)?;
        let local = at - vc.start;
        let frame = (SEC as f64 / (self.doc.project.fps as f64).max(1.0)) as view::Time;
        let mut said = Vec::new();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            mc.corner_pin = None;
            mc.keyframes.retain(|k, _| !k.starts_with("corner_pin."));
            said.push("corner pin removed".to_owned());
        } else {
            let fresh = mc.corner_pin.is_none();
            let mut cp = mc.corner_pin.clone().unwrap_or_else(|| canvas_pins::new_corner_pin(&map));
            if op.get("reset").and_then(Value::as_bool) == Some(true) {
                cp = m::CornerPin::new(cp.rest);
                mc.keyframes.retain(|k, _| !k.starts_with("corner_pin."));
                said.push("corners back on the layer's box".to_owned());
            }
            mc.corner_pin = Some(cp.clone());
            let names = m::CornerPin::CORNERS;
            if let Some(sk) = op.get("skew") {
                let [sx, sy] = pair(sk).or_else(|| sk.as_f64().map(|x| [x, 0.0])).ok_or("skew is degrees: a number (sideways) or [x, y]")?;
                // Shear the box about its centre, in canvas pixels of the
                // flat layer so the angle is what shows.
                let corners = m::CornerPin::new(cp.rest).corners().map(|c| map.flat_canvas(c));
                let mid = corners.iter().fold(eframe::egui::Vec2::ZERO, |a, c| a + *c) / 4.0;
                for (k, c) in corners.iter().enumerate() {
                    let d = *c - mid;
                    let moved = [(c.x + d.y * (sx.to_radians().tan() as f32)) as f64, (c.y + d.x * (sy.to_radians().tan() as f32)) as f64];
                    let p = Self::layer_point(&map, &json!(moved), true)?;
                    xform_put(&mut mc, &format!("corner_pin.{}", names[k]), m::Value::Point(p), local, frame);
                }
                said.push(format!("skewed {sx}° × {sy}°"));
            }
            for name in names {
                let Some(spec) = op.get(name) else { continue };
                let path = format!("corner_pin.{name}");
                let p = match spec {
                    Value::Object(o) if o.contains_key("by") => {
                        let by = o.get("by").and_then(pair).ok_or("by is [dx, dy] canvas pixels")?;
                        let cur = match mc.property_at(&path, local) {
                            Some(m::Value::Point(q)) => q,
                            _ => m::Vec2::ZERO,
                        };
                        Self::moved_by(&map, cur, by, true)?
                    }
                    s => Self::layer_point(&map, s, true)?,
                };
                xform_put(&mut mc, &path, m::Value::Point(p), local, frame);
                said.push(format!("{name} placed"));
            }
            if let Some(Value::Object(per)) = op.get("keys") {
                let ease = op.get("ease").and_then(Value::as_str).unwrap_or("ease_in_out");
                for (name, keys) in per {
                    if !names.contains(&name.as_str()) {
                        return Err(format!("keys are per corner: {}", names.join(", ")));
                    }
                    let path = format!("corner_pin.{name}");
                    let mut ks = Vec::new();
                    for k in key_specs(keys, ease)? {
                        let t = micros(k.t);
                        let kmap = self.layer_map(id, vc.start + t.clamp(0, vc.dur))?;
                        let cur = match mc.property_at(&path, t) {
                            Some(m::Value::Point(q)) => q,
                            _ => m::Vec2::ZERO,
                        };
                        let v = match (&k.to, k.by) {
                            (Some(to), _) => Self::layer_point(&kmap, to, true)?,
                            (None, Some(by)) => Self::moved_by(&kmap, cur, by, true)?,
                            _ => return Err("a corner's keys need \"to\" or \"by\"".into()),
                        };
                        ks.push((t, m::Value::Point(v), k.ease.clone()));
                    }
                    mc.keyframes.insert(path, track(&ks));
                    said.push(format!("{name} animated"));
                }
            } else if op.get("keys").is_some() {
                return Err("corner keys are per corner: {\"top_left\": [[0, [x, y]], [1, [x, y]]], …}".into());
            }
            if said.is_empty() {
                if !fresh {
                    return Err("corner_pin needs corners (top_left, top_right, bottom_right, bottom_left), skew, keys, reset or remove".into());
                }
                said.push("corner pin added on the layer's box (drag its corners in the app, or give them here)".into());
            }
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Corner pin", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let mc = self.model_clip(id).ok_or("clip gone")?;
        let corners: Value = match mc.corner_pin_at(local) {
            Some(cp) => {
                let map = self.layer_map(id, at)?;
                json!(m::CornerPin::CORNERS.iter().zip(cp.corners()).map(|(n, c)| {
                    let p = map.flat_canvas(c);
                    (n.to_string(), json!([r1(p.x), r1(p.y)]))
                }).collect::<serde_json::Map<_, _>>())
            }
            None => Value::Null,
        };
        Ok(format!("{}; corners now (canvas px at {:.2}s): {corners}", said.join("; "), secs(at)))
    }

    /// The pins, corner pin and pinned masks of a clip, for `inspect`.
    pub(super) fn pins_of(&self, id: u32) -> Option<Value> {
        let mc = self.model_clip(id)?;
        let vc = self.clip(id).ok()?;
        if mc.puppet.is_none() && mc.corner_pin.is_none() && mc.masks.iter().all(|k| k.pins.is_empty()) {
            return None;
        }
        let t = vc.start;
        let mut o = json!({"note": "canvas px at the clip's start; puppet / mask_puppet / corner_pin change them"});
        if mc.puppet.is_some() {
            o["puppet"] = json!(self.pins_said(id, &mc, Owner::Puppet, t));
        }
        if let (Some(cp), Ok(map)) = (mc.corner_pin_at(0), self.layer_map(id, t)) {
            o["corner_pin"] = json!(m::CornerPin::CORNERS.iter().zip(cp.corners()).map(|(n, c)| {
                let p = map.flat_canvas(c);
                (n.to_string(), json!([r1(p.x), r1(p.y)]))
            }).collect::<serde_json::Map<_, _>>());
        }
        for (i, mk) in mc.masks.iter().enumerate().filter(|(_, k)| !k.pins.is_empty()) {
            o[format!("mask{}_pins", i + 1)] = json!(self.pins_said(id, &mc, Owner::Mask(mk.id.0), t));
        }
        Some(o)
    }
}

fn xform_put(mc: &mut m::Clip, path: &str, value: m::Value, local: view::Time, frame: view::Time) {
    canvas_pins::put(mc, path, value, local, frame);
}

/// Pins as one line: `1 head position (477.3, 81.2) animated, 2 ...`.
fn pins_line(pins: &[Value]) -> String {
    pins.iter()
        .map(|p| {
            let at = p["at"].as_array().map(|a| format!("({}, {})", a[0], a[1])).unwrap_or_default();
            let name = p["name"].as_str().map(|n| format!(" {n}")).unwrap_or_default();
            let extra = if p["kind"] == "bend" { format!(" rotate {} scale {}", p["rotate"], p["scale"]) } else { String::new() };
            format!("{}{name} {} {at}{extra}{}", p["pin"], p["kind"].as_str().unwrap_or(""), if p["animated"] == true { " animated" } else { "" })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::services;

    fn session_with_card() -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Pins", 640, 360, 30.0));
        let r = s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2, "width": 320, "height": 180})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let _ = r;
        (s, id)
    }

    #[test]
    fn pins_place_move_animate_and_remove() {
        let (mut s, id) = session_with_card();
        s.edit(&[json!({"op": "puppet", "clip": id, "pins": [{"name": "left", "at": [-150, 0]}, {"name": "right", "at": "right"}]})], "pins", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let pins = &mc.puppet.as_ref().unwrap().pins;
        assert_eq!(pins.len(), 2);
        // The card is 320 px wide on a 640 px canvas, centred: x = -150 is
        // 5 / 320 into it.
        let map = LayerMap::at(&s.doc.project, &s.doc.model, id, 0).unwrap();
        let back = map.canvas(pins[0].position);
        assert!((back.x + 150.0).abs() < 0.5 && back.y.abs() < 0.5, "{back:?}");
        s.edit(&[json!({"op": "puppet", "clip": id, "pin": "right", "keys": [[0, "right"], {"t": 1, "by": [0, -80], "ease": "overshoot"}]})], "anim", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let tr = &mc.keyframes["puppet.2.position"];
        assert_eq!(tr.keys.len(), 2);
        let at1 = mc.puppet_at(SEC).unwrap();
        let c = map.canvas(at1.pins[1].position);
        assert!((c.y + 80.0).abs() < 1.0, "{c:?}");
        // Wrong pin: refused with the list.
        let e = s.edit(&[json!({"op": "puppet", "clip": id, "pin": "middle", "to": [0, 0]})], "x", false).unwrap_err();
        assert!(e.contains("left"), "{e}");
        s.edit(&[json!({"op": "puppet", "clip": id, "remove": "all"})], "rm", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.puppet.is_none() && !mc.keyframes.keys().any(|k| k.starts_with("puppet.")));
    }

    #[test]
    fn corner_pin_and_skew() {
        let (mut s, id) = session_with_card();
        s.edit(&[json!({"op": "corner_pin", "clip": id, "top_left": [-200, -120], "bottom_right": {"by": [40, 20]}})], "cp", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let cp = mc.corner_pin.clone().unwrap();
        assert!(!cp.is_identity());
        let map = LayerMap::at(&s.doc.project, &s.doc.model, id, 0).unwrap();
        let tl = map.flat_canvas(cp.top_left);
        assert!((tl.x + 200.0).abs() < 0.5 && (tl.y + 120.0).abs() < 0.5, "{tl:?}");
        s.edit(&[json!({"op": "corner_pin", "clip": id, "reset": true, "skew": 20})], "skew", false).unwrap();
        let cp = s.model_clip(id).unwrap().corner_pin.unwrap();
        // Skewed sideways: the top edge moved left of the bottom edge.
        assert!(map.flat_canvas(cp.top_left).x < map.flat_canvas(cp.bottom_left).x - 10.0);
    }

    #[test]
    fn mask_puppet_turns_a_shape_into_a_pinned_path() {
        let (mut s, id) = session_with_card();
        s.edit(&[json!({"op": "mask", "clip": id, "shape": "ellipse", "params": {"size": [0.6, 0.6]}})], "mask", false).unwrap();
        s.edit(&[json!({"op": "mask_puppet", "clip": id, "pins": [[-40, 0], [40, 0]], "pin": 2, "to": [90, -30]})], "mp", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.masks[0].shape, m::MaskShape::Path);
        assert_eq!(mc.masks[0].pins.len(), 2);
        s.edit(&[json!({"op": "mask_puppet", "clip": id, "point": 1, "keys": [[0, {"u": 0.5, "v": 0.2}], [1, {"u": 0.5, "v": 0.05}]]})], "pt", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let mid = mc.masks[0].id.0;
        assert_eq!(mc.keyframes[&format!("mask.{mid}.point.0")].keys.len(), 2);
        let e = s.edit(&[json!({"op": "mask_puppet", "clip": id, "point": 9, "to": [0, 0]})], "x", false).unwrap_err();
        assert!(e.contains("1 to 4"), "{e}");
    }
}
