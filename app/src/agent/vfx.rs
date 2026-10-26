// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The visual effects department for agents (`services::vfx`).
//!
//! * `comp`: the node compositor on a clip (`clip`) or over the whole shot
//!   (`shot: true` adds a comp layer above everything): `add` nodes,
//!   `set` them, `remove`, `output`, `reset`, `delete`. Node references
//!   are names (or ids); positions in node data and point parameters are
//!   canvas pixels from the centre (converted to the clip's picture),
//!   keyframe times are timeline seconds.
//! * Ready-made graphs: `keying` (screen key with core/edge mattes,
//!   spill, clean plate, light wrap over a background), `remove_object`
//!   (temporal fill or inpainting under a cut-out, box or outline),
//!   `clean_plate` (bake the background alone), `sky` (sky replacement),
//!   `wire` (wire and rig removal), `paint` (strokes over time),
//!   `depth_of_field` (AI depth pass + lens blur), `grain_match`, `lens`
//!   (distortion), `motion_blur` (vector blur).
//! * `matchmove`: solve the camera of a moving shot into a Studio camera;
//!   `place` stands a 3D layer on the solved ground; `export_camera` and
//!   `import_camera` (.chan, .json, .obj).

use super::{Session, f, micros, secs};
use crate::services::vfx;
use crate::view;
use eframe::egui::Vec2;
use serde_json::{Map, Value, json};
use studio_ai::JobCtx;
use studio_fx::comp::{Graph, Key, NodeId};
use studio_model as m;

fn xy(v: &Value) -> Option<[f64; 2]> {
    match v {
        Value::Array(a) if a.len() >= 2 => Some([a[0].as_f64()?, a[1].as_f64()?]),
        Value::Object(o) => Some([o.get("x")?.as_f64()?, o.get("y")?.as_f64()?]),
        _ => None,
    }
}

/// How a clip's graph maps canvas pixels (from the centre) into its
/// picture pixels.
struct Mapper {
    geo: Option<crate::editor::xform::Geo>,
    size: (f64, f64),
    canvas: (f64, f64),
}

impl Mapper {
    fn point(&self, c: [f64; 2]) -> [f64; 2] {
        match &self.geo {
            Some(g) => g.to_source(Vec2::new(c[0] as f32, c[1] as f32)).map_or(c, |s| [s.x as f64, s.y as f64]),
            // A shot comp works on the canvas, origin top-left.
            None => [c[0] + self.canvas.0 * 0.5, c[1] + self.canvas.1 * 0.5],
        }
    }

    fn vector(&self, at: [f64; 2], d: [f64; 2]) -> [f64; 2] {
        let a = self.point(at);
        let b = self.point([at[0] + d[0], at[1] + d[1]]);
        [b[0] - a[0], b[1] - a[1]]
    }

    fn length(&self, l: f64) -> f64 {
        let v = self.vector([0.0, 0.0], [l, 0.0]);
        v[0].hypot(v[1])
    }

    /// A point parameter: fractions of the picture stay, canvas pixels
    /// become fractions.
    fn param_point(&self, v: [f64; 2]) -> [f64; 2] {
        if v.iter().all(|c| (-1.0..=2.0).contains(c)) {
            return v;
        }
        let p = self.point(v);
        [p[0] / self.size.0, p[1] / self.size.1]
    }
}

/// Rewrites every `[x, y, …]` list in `v` (a points list) through `map`,
/// with handles (items 2..6) as vectors.
fn map_points(v: &mut Value, map: &Mapper) {
    let Some(a) = v.as_array_mut() else { return };
    for p in a.iter_mut() {
        let Some(q) = p.as_array_mut() else { continue };
        let (Some(x), Some(y)) = (q.first().and_then(Value::as_f64), q.get(1).and_then(Value::as_f64)) else { continue };
        let s = map.point([x, y]);
        q[0] = json!(s[0]);
        q[1] = json!(s[1]);
        for h in [2usize, 4] {
            if let (Some(hx), Some(hy)) = (q.get(h).and_then(Value::as_f64), q.get(h + 1).and_then(Value::as_f64)) {
                let d = map.vector([x, y], [hx, hy]);
                q[h] = json!(d[0]);
                q[h + 1] = json!(d[1]);
            }
        }
    }
}

/// Node data in canvas pixels → the graph's picture pixels.
fn map_data(kind: &str, data: &mut Value, map: &Mapper, clip_start: f64) {
    let rel = |t: &mut Value| {
        if let Some(s) = t.as_f64() {
            *t = json!(s - clip_start);
        }
    };
    match kind {
        "roto" => {
            for s in data.get_mut("shapes").and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(p) = s.get_mut("points") {
                    map_points(p, map);
                }
                for k in s.get_mut("keys").and_then(Value::as_array_mut).into_iter().flatten() {
                    if let Some(p) = k.get_mut("points") {
                        map_points(p, map);
                    }
                    if let Some(t) = k.get_mut("t") {
                        rel(t);
                    }
                }
                if let Some(fe) = s.get("feather").and_then(Value::as_f64) {
                    s["feather"] = json!(map.length(fe));
                }
                for k in ["from", "to"] {
                    if let Some(t) = s.get_mut(k) {
                        rel(t);
                    }
                }
            }
        }
        "paint" => {
            for s in data.get_mut("strokes").and_then(Value::as_array_mut).into_iter().flatten() {
                let first = s.get("points").and_then(Value::as_array).and_then(|a| a.first()).and_then(xy).unwrap_or([0.0, 0.0]);
                if let Some(p) = s.get_mut("points") {
                    map_points(p, map);
                }
                if let Some(o) = s.get("offset").and_then(xy) {
                    s["offset"] = json!(map.vector(first, o));
                }
                if let Some(r) = s.get("radius").and_then(Value::as_f64) {
                    s["radius"] = json!(map.length(r));
                }
                for k in ["from", "to"] {
                    if let Some(t) = s.get_mut(k) {
                        rel(t);
                    }
                }
            }
        }
        "wire_remove" => {
            if let Some(p) = data.get_mut("points") {
                map_points(p, map);
            }
        }
        _ => {}
    }
}

impl Session {
    fn mapper(&self, id: u32, t: view::Time, shot: bool) -> Result<Mapper, String> {
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let canvas = (self.doc.project.width as f64, self.doc.project.height as f64);
        let size = vfx::picture_size(&self.doc, &mc);
        let geo = if shot || matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Adjustment) {
            None
        } else {
            crate::editor::xform::geo(&self.doc.project, &self.doc.model, id, t)
        };
        Ok(Mapper { geo, size: if shot { canvas } else { size }, canvas })
    }

    /// The clip an op works on, its graph (new: a passthrough) and whether
    /// it is a shot comp.
    fn comp_target(&mut self, op: &Value) -> Result<(u32, Graph, bool), String> {
        let shot = op.get("shot").and_then(Value::as_bool) == Some(true);
        let clip = op.get("clip").and_then(Value::as_u64).map(|v| v as u32);
        let id = match (clip, shot) {
            (Some(id), true) => {
                let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
                if matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Adjustment) {
                    id
                } else {
                    let vc = self.clip(id)?;
                    let lane = self.room(None, vc.start, vc.dur, None)?;
                    vfx::add_comp_layer(&mut self.doc, lane, vc.start, vc.dur, "Comp")?
                }
            }
            (Some(id), false) => id,
            (None, true) => {
                let at = micros(f(op, "at").unwrap_or(0.0));
                let dur = f(op, "duration").map(micros).unwrap_or_else(|| (self.doc.project.duration() - at).max(view::SEC));
                let lane = self.room(None, at, dur, None)?;
                vfx::add_comp_layer(&mut self.doc, lane, at, dur, "Comp")?
            }
            (None, false) => return Err("comp needs \"clip\" (a graph on that clip) or \"shot\": true (a comp layer over everything, with at/duration or the clip's time)".into()),
        };
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let is_shot = matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Adjustment);
        let mut g = vfx::graph_of(&mc).map(|(_, g)| g).unwrap_or_else(Graph::passthrough);
        if !is_shot && g.input_media.is_none() {
            g.input_media = vfx::input_media(&self.doc, &mc);
        }
        Ok((id, g, is_shot))
    }

    /// A node by name or id.
    fn node_ref(g: &Graph, v: &Value) -> Result<NodeId, String> {
        let name = match v {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => return Err("a node is named by its name or id".into()),
        };
        g.find(&name).map(|n| n.id).ok_or_else(|| format!("no node \"{name}\"; nodes: {}", g.nodes.iter().map(|n| n.label()).collect::<Vec<_>>().join(", ")))
    }

    /// Applies one node spec (`add` or `set`) to `g`.
    fn node_spec(g: &mut Graph, id: NodeId, spec: &Value, map: &Mapper, clip_start: f64) -> Result<(), String> {
        let kind = g.node(id).map(|n| n.kind.clone()).unwrap_or_default();
        if let Some(v) = spec.get("in").or(spec.get("inputs")) {
            let list: Vec<Value> = match v {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            let mut ins = Vec::new();
            for x in &list {
                ins.push(if x.is_null() { None } else { Some(Self::node_ref(g, x)?) });
            }
            g.node_mut(id).unwrap().inputs = ins;
        }
        if let Some(mk) = spec.get("mask") {
            let m = if mk.is_null() { None } else { Some(Self::node_ref(g, mk)?) };
            g.node_mut(id).unwrap().mask = m;
        }
        let n = g.node_mut(id).unwrap();
        if let Some(name) = spec.get("name").and_then(Value::as_str) {
            n.name = name.to_owned();
        }
        if let Some(mx) = spec.get("mix").and_then(Value::as_f64) {
            n.mix = mx.clamp(0.0, 1.0) as f32;
        }
        if let Some(d) = spec.get("disabled").and_then(Value::as_bool) {
            n.disabled = d;
        }
        if let Some(d) = spec.get("data") {
            let mut d = d.clone();
            map_data(&kind, &mut d, map, clip_start);
            // Data merges, so a path can be set without the shapes.
            match (&mut n.data, d) {
                (Value::Object(cur), Value::Object(new)) => cur.extend(new),
                (cur, new) => *cur = new,
            }
        }
        let defs = n.param_defs();
        let conv = |pid: &str, v: &Value| -> Result<Value, String> {
            let def = defs.iter().find(|d| d.id == pid).ok_or_else(|| format!("{kind} has no parameter \"{pid}\"; it has: {}", defs.iter().map(|d| d.id).collect::<Vec<_>>().join(", ")))?;
            let v = match (def.kind, v) {
                (studio_fx::params::ParamKind::Point, v) if xy(v).is_some() => {
                    let p = map.param_point(xy(v).unwrap());
                    json!([p[0], p[1]])
                }
                _ => v.clone(),
            };
            studio_fx::comp::value_of(&v, def).ok_or_else(|| format!("{kind}.{pid}: {v} is not a {}", match def.kind {
                studio_fx::params::ParamKind::Float { min, max } => format!("number {min}..{max}"),
                studio_fx::params::ParamKind::Int { min, max } => format!("whole number {min}..{max}"),
                studio_fx::params::ParamKind::Bool => "true or false".into(),
                studio_fx::params::ParamKind::Choice(o) => format!("choice of {}", o.join(", ")),
                studio_fx::params::ParamKind::Colour => "colour (\"#rrggbb\" or [r, g, b, a] linear)".into(),
                studio_fx::params::ParamKind::Point => "point [x, y]".into(),
            }))?;
            Ok(v)
        };
        if let Some(Value::Object(ps)) = spec.get("params") {
            for (k, v) in ps {
                let v = conv(k, v)?;
                n.params.insert(k.clone(), v);
                n.keys.remove(k);
            }
        }
        if let Some(Value::Object(ks)) = spec.get("keys") {
            for (k, list) in ks {
                let list = list.as_array().ok_or_else(|| format!("keys.{k} is [[time, value], …] (timeline seconds)"))?;
                let mut keys = Vec::new();
                for item in list {
                    let a = item.as_array().filter(|a| a.len() >= 2).ok_or_else(|| format!("keys.{k}: each key is [time, value]"))?;
                    let t = a[0].as_f64().ok_or("a key's time is seconds")?;
                    keys.push(Key { t: t - clip_start, v: conv(k, &a[1])?, hold: a.get(2).and_then(Value::as_str) == Some("hold") });
                }
                keys.sort_by(|a, b| a.t.total_cmp(&b.t));
                n.keys.insert(k.clone(), keys);
            }
        }
        Ok(())
    }

    /// One line per node, for answers.
    fn graph_said(g: &Graph) -> String {
        let out = g.output_id();
        let mut lines = Vec::new();
        for n in &g.nodes {
            let ins: Vec<String> = n.inputs.iter().map(|i| i.and_then(|i| g.node(i)).map_or("-".into(), |x| x.label())).collect();
            let mut s = format!("{} ({})", n.label(), n.kind);
            if !ins.is_empty() {
                s += &format!(" <- {}", ins.join(", "));
            }
            if !n.params.is_empty() {
                let ps: Vec<String> = n.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
                s += &format!(" [{}]", ps.join(" "));
            }
            if !n.keys.is_empty() {
                s += &format!(" keyed: {}", n.keys.keys().cloned().collect::<Vec<_>>().join(", "));
            }
            if let Some(mk) = n.mask.and_then(|m| g.node(m)) {
                s += &format!(" mask {}", mk.label());
            }
            if n.mix < 1.0 {
                s += &format!(" mix {}", n.mix);
            }
            if n.disabled {
                s += " (off)";
            }
            if Some(n.id) == out {
                s += " => output";
            }
            lines.push(s);
        }
        lines.join("; ")
    }

    /// `comp`: build and edit a clip's node graph.
    pub(super) fn comp(&mut self, op: &Value) -> Result<String, String> {
        if op.get("delete").and_then(Value::as_bool) == Some(true) {
            let id = op.get("clip").and_then(Value::as_u64).ok_or("delete needs \"clip\"")? as u32;
            vfx::remove_graph(&mut self.doc, id)?;
            return Ok(format!("clip {id} has no compositing graph now"));
        }
        let (id, mut g, shot) = self.comp_target(op)?;
        let vc = self.clip(id)?;
        let at = f(op, "at").map(micros).unwrap_or(vc.start + vc.dur / 2).clamp(vc.start, vc.start + vc.dur - 1);
        let map = self.mapper(id, at, shot)?;
        let start = secs(vc.start);
        if op.get("reset").and_then(Value::as_bool) == Some(true) {
            let media = g.input_media.take();
            g = Graph::passthrough();
            g.input_media = media;
        }
        let mut added = Vec::new();
        for spec in op.get("add").and_then(Value::as_array).into_iter().flatten() {
            let kind = spec.get("kind").and_then(Value::as_str).ok_or("each added node needs \"kind\"; kinds: guide topic nodes")?;
            studio_fx::comp::nodes::find(kind).ok_or_else(|| format!("no node kind \"{kind}\"; kinds: {}", studio_fx::comp::nodes::names()))?;
            if let Some(name) = spec.get("name").and_then(Value::as_str) {
                if g.find(name).is_some() {
                    return Err(format!("there is already a node \"{name}\""));
                }
            }
            // Fed by the current output unless told otherwise.
            let main = g.output_id();
            let nid = g.add(kind, &[]);
            let n = g.node_mut(nid).unwrap();
            let def = n.def().unwrap();
            if !def.inputs.is_empty() && spec.get("in").or(spec.get("inputs")).is_none() {
                n.inputs = vec![main];
            }
            Self::node_spec(&mut g, nid, spec, &map, start)?;
            added.push(g.node(nid).unwrap().label());
        }
        for spec in op.get("set").and_then(Value::as_array).into_iter().flatten() {
            let nid = Self::node_ref(&g, spec.get("node").ok_or("each set needs \"node\"")?)?;
            Self::node_spec(&mut g, nid, spec, &map, start)?;
        }
        for r in op.get("remove").and_then(Value::as_array).into_iter().flatten() {
            let nid = Self::node_ref(&g, r)?;
            g.remove(nid);
        }
        if let Some(o) = op.get("output") {
            g.output = Some(Self::node_ref(&g, o)?);
        }
        g.check()?;
        vfx::store_graph(&mut self.doc, id, &g, "Comp")?;
        let mut said = format!("clip {id}{}: {}", if shot { " (comp layer over the shot)" } else { "" }, Self::graph_said(&g));
        if !added.is_empty() {
            said = format!("added {}; {said}", added.join(", "));
        }
        Ok(said)
    }

    /// Adds nodes to a clip's graph through `comp` and answers with `note`.
    fn build(&mut self, clip: u32, nodes: Vec<Value>, extra: Map<String, Value>, note: &str) -> Result<String, String> {
        let mut op = json!({"clip": clip, "add": nodes});
        for (k, v) in extra {
            op[k] = v;
        }
        let said = self.comp(&op)?;
        Ok(format!("{note}. {said}"))
    }

    fn path_of(&self, v: &Value) -> Result<(String, f64), String> {
        // A clip id (its media and where it starts), or a file.
        if let Some(cid) = v.as_u64() {
            let mc = self.model_clip(cid as u32).ok_or_else(|| format!("no clip {cid}"))?;
            let im = vfx::input_media(&self.doc, &mc).ok_or_else(|| format!("clip {cid} is not footage or a picture with a file"))?;
            return Ok((im.path, im.start));
        }
        let s = v.as_str().ok_or("give a file path or a clip id")?;
        if std::path::Path::new(s).exists() {
            return Ok((s.to_owned(), 0.0));
        }
        let mid = super::craft::media_of(&self.doc.project, v)?;
        let mi = self.doc.project.media(mid).ok_or("that media is gone")?;
        Ok((mi.path.clone().ok_or("that media has no file")?.to_string_lossy().into_owned(), 0.0))
    }

    fn uniq(g: Option<&Graph>, base: &str) -> String {
        let Some(g) = g else { return base.to_owned() };
        if g.find(base).is_none() {
            return base.to_owned();
        }
        (2..).map(|i| format!("{base}{i}")).find(|n| g.find(n).is_none()).unwrap()
    }

    fn graph_now(&self, id: u32) -> Option<Graph> {
        self.model_clip(id).and_then(|mc| vfx::graph_of(&mc)).map(|(_, g)| g)
    }

    /// `keying`: a deluxe screen key.
    pub(super) fn keying(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("keying needs \"clip\": the green or blue screen footage")? as u32;
        let g = self.graph_now(id);
        let key = Self::uniq(g.as_ref(), "key");
        let mut params = Map::new();
        match op.get("screen") {
            Some(Value::String(s)) if s.starts_with('#') => {
                params.insert("screen".into(), json!("Custom"));
                params.insert("colour".into(), json!(s));
            }
            Some(Value::String(s)) => {
                params.insert("screen".into(), json!(if s.eq_ignore_ascii_case("blue") { "Blue" } else { "Green" }));
            }
            _ => {}
        }
        for (k, p) in [("core", "core_shrink"), ("edge_band", "edge_band"), ("spill", "spill"), ("restore", "restore"), ("fix_edges", "edge_colour"), ("clip_black", "black_clip"), ("clip_white", "white_clip"), ("gain", "gain"), ("balance", "balance")] {
            if let Some(v) = op.get(k) {
                params.insert(p.into(), v.clone());
            }
        }
        if let Some(v) = op.get("despill").and_then(Value::as_str) {
            params.insert("despill".into(), json!(v));
        }
        let mut nodes = Vec::new();
        let mut key_in = vec![json!("input")];
        let mut note = vec![format!("keyed clip {id}")];
        // Clean plate: a file, "auto" (baked now), or none.
        match op.get("clean_plate") {
            Some(Value::String(s)) if s == "auto" => {
                let p = vfx::bake_clean_plate(&self.doc, id, 25, false, &JobCtx::detached())?;
                nodes.push(json!({"name": Self::uniq(g.as_ref(), "plate"), "kind": "read", "in": [], "data": {"path": p.to_string_lossy()}, "params": {"hold": true}}));
                key_in.push(json!(Self::uniq(g.as_ref(), "plate")));
                note.push(format!("clean plate baked to {}", p.display()));
            }
            Some(v @ Value::String(_)) => {
                let (p, _) = self.path_of(v)?;
                nodes.push(json!({"name": Self::uniq(g.as_ref(), "plate"), "kind": "read", "in": [], "data": {"path": p}, "params": {"hold": true}}));
                key_in.push(json!(Self::uniq(g.as_ref(), "plate")));
            }
            _ => key_in.push(Value::Null),
        }
        if let Some(Value::Array(poly)) = op.get("garbage") {
            let name = Self::uniq(g.as_ref(), "garbage");
            nodes.push(json!({"name": name, "kind": "roto", "in": [], "data": {"shapes": [{"points": poly}]}, "params": {"invert": true, "output": 1}}));
            key_in.push(json!(name));
        }
        nodes.push(json!({"name": key, "kind": "keyer", "in": key_in, "params": params}));
        if let Some(bg) = op.get("background") {
            let (path, start) = self.path_of(bg)?;
            let bgn = Self::uniq(g.as_ref(), "bg");
            nodes.push(json!({"name": bgn, "kind": "read", "in": [], "data": {"path": path}, "params": {"start": start, "fit": "Fill"}}));
            let wrap = op.get("light_wrap").and_then(Value::as_f64).unwrap_or(0.5);
            nodes.push(json!({"name": Self::uniq(g.as_ref(), "wrap"), "kind": "light_wrap", "in": [key, bgn], "params": {"intensity": wrap}}));
            note.push("over the background with light wrap".into());
        } else {
            note.push("the clip is now transparent where the screen was: put the background on the lane below (or pass background for light wrap)".into());
        }
        self.build(id, nodes, Map::new(), &note.join("; "))
    }

    /// The matte node(s) for an object: the cut-out, a box or an outline.
    fn object_matte(&mut self, id: u32, op: &Value, g: Option<&Graph>) -> Result<(Vec<Value>, String), String> {
        let name = Self::uniq(g, "object");
        if let Some(b) = op.get("box") {
            let a = b.as_array().filter(|a| a.len() == 2).ok_or("box is [[x0, y0], [x1, y1]] canvas pixels")?;
            let (p, q) = (xy(&a[0]).ok_or("box corner is [x, y]")?, xy(&a[1]).ok_or("box corner is [x, y]")?);
            let poly = json!([[p[0], p[1]], [q[0], p[1]], [q[0], q[1]], [p[0], q[1]]]);
            return Ok((vec![json!({"name": name, "kind": "roto", "in": [], "data": {"shapes": [{"points": poly}]}, "params": {"output": 1}})], name));
        }
        if let Some(poly) = op.get("outline") {
            return Ok((vec![json!({"name": name, "kind": "roto", "in": [], "data": {"shapes": [{"points": poly}]}, "params": {"output": 1}})], name));
        }
        // The clip's cut-out, switched off on the clip so it only feeds
        // the graph.
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let start = mc.source_in as f64 / view::SEC as f64;
        let path = vfx::cut_out_path(&self.doc, &mc).ok_or("give the object as box [[x0, y0], [x1, y1]] or outline [[x, y], …], or cut it out first (cut_out with subject/point)")?;
        if crate::services::cutout::set_flag(&mut mc, "enabled", false) {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Cut-out as matte", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
        }
        Ok((vec![json!({"name": name, "kind": "read", "in": [], "data": {"path": path}, "params": {"layer": "Matte", "start": start}})], name))
    }

    /// `remove_object`: paints an object out with what is behind it.
    pub(super) fn remove_object(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("remove_object needs \"clip\"")? as u32;
        let g = self.graph_now(id);
        let (mut nodes, matte) = self.object_matte(id, op, g.as_ref())?;
        // The cut-out matte is indexed by source time from 0.
        let method = op.get("method").and_then(Value::as_str).unwrap_or("auto");
        let grow = op.get("grow").and_then(Value::as_f64).unwrap_or(4.0);
        let name = Self::uniq(g.as_ref(), "remove");
        match method {
            "inpaint" | "fill" => nodes.push(json!({"name": name, "kind": "inpaint", "in": [g.as_ref().and_then(|g| g.output_id()).map_or(json!("input"), |o| json!(o)), matte], "params": {"grow": grow}})),
            _ => {
                let range = op.get("range").and_then(Value::as_u64).unwrap_or(16);
                let locked = op.get("camera").and_then(Value::as_str) == Some("locked");
                nodes.push(json!({"name": name, "kind": "temporal_fill", "in": [g.as_ref().and_then(|g| g.output_id()).map_or(json!("input"), |o| json!(o)), matte], "params": {"range": range, "step": op.get("step").and_then(Value::as_u64).unwrap_or(2), "grow": grow, "camera": if locked { "Locked off" } else { "Moving (align frames)" }}}));
            }
        }
        self.build(id, nodes, Map::new(), &format!("removing the object from clip {id} ({})", if method == "inpaint" { "inpainting" } else { "background from nearby frames, the rest inpainted" }))
    }

    /// `clean_plate`: bakes the background alone to a picture.
    pub(super) fn clean_plate(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("clean_plate needs \"clip\"")? as u32;
        let samples = op.get("samples").and_then(Value::as_u64).unwrap_or(25) as usize;
        let moving = op.get("camera").and_then(Value::as_str) == Some("moving");
        let p = vfx::bake_clean_plate(&self.doc, id, samples, moving, &JobCtx::detached())?;
        Ok(format!("clean plate of clip {id} saved to {} (median of {samples} frames{}); use it as keying clean_plate, a read node, or import it", p.display(), if moving { ", aligned" } else { "" }))
    }

    /// `sky`: sky replacement.
    pub(super) fn sky(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("sky needs \"clip\"")? as u32;
        let (path, start) = self.path_of(op.get("sky").ok_or("sky needs \"sky\": a picture or video file, media name or clip id")?)?;
        let g = self.graph_now(id);
        let skyn = Self::uniq(g.as_ref(), "sky_plate");
        let mut nodes = vec![json!({"name": skyn, "kind": "read", "in": [], "data": {"path": path}, "params": {"start": start, "fit": "Fill"}})];
        let main = g.as_ref().and_then(|g| g.output_id()).map_or(json!("input"), |o| json!(o));
        let mut ins = vec![main, json!(skyn)];
        if op.get("matte").and_then(Value::as_str) == Some("cut_out") {
            let (mn, mname) = self.object_matte(id, &json!({}), g.as_ref())?;
            nodes.extend(mn);
            ins.push(json!(mname));
        }
        let mut params = Map::new();
        for (k, p) in [("relight", "harmonise"), ("haze", "haze"), ("wrap", "wrap"), ("edge", "edge"), ("detect", "threshold"), ("feather", "feather")] {
            if let Some(v) = op.get(k) {
                params.insert(p.into(), v.clone());
            }
        }
        nodes.push(json!({"name": Self::uniq(g.as_ref(), "sky"), "kind": "sky_replace", "in": ins, "params": params}));
        self.build(id, nodes, Map::new(), &format!("new sky on clip {id}; check the edge with frame and the matte with comp set sky params output \"Sky matte\""))
    }

    /// `wire`: wire and rig removal along a line or polyline.
    pub(super) fn wire(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("wire needs \"clip\"")? as u32;
        let g = self.graph_now(id);
        let mut params = Map::new();
        for k in ["from", "to"] {
            if let Some(v) = op.get(k) {
                params.insert(k.into(), v.clone());
            }
        }
        if let Some(w) = op.get("width") {
            params.insert("width".into(), w.clone());
        }
        let mut spec = json!({"name": Self::uniq(g.as_ref(), "wire"), "kind": "wire_remove", "params": params});
        if let Some(p) = op.get("points") {
            spec["data"] = json!({"points": p});
        }
        if let Some(k) = op.get("keys") {
            spec["keys"] = k.clone();
        }
        if op.get("from").is_none() && op.get("points").is_none() {
            return Err("wire needs from and to (canvas pixels), or points [[x, y], …] along the wire".into());
        }
        self.build(id, vec![spec], Map::new(), &format!("wire removed on clip {id}; if it moves, key from/to with keys {{\"from\": [[t, [x, y]], …]}}"))
    }

    /// `paint`: strokes over time on a clip.
    pub(super) fn paint(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("paint needs \"clip\"")? as u32;
        let strokes = op.get("strokes").and_then(Value::as_array).ok_or("paint needs \"strokes\": [{tool, points, radius, …}]")?.clone();
        let g = self.graph_now(id);
        // Add to the last paint node, or start one.
        if let Some(p) = g.as_ref().and_then(|g| g.nodes.iter().rev().find(|n| n.kind == "paint")) {
            let mut all = p.data.get("strokes").and_then(Value::as_array).cloned().unwrap_or_default();
            let vc = self.clip(id)?;
            let map = self.mapper(id, vc.start + vc.dur / 2, false)?;
            let mut new = json!({"strokes": strokes});
            map_data("paint", &mut new, &map, secs(vc.start));
            all.extend(new["strokes"].as_array().cloned().unwrap_or_default());
            let name = p.label();
            // Data replaces wholesale here (already in picture pixels).
            let mut gg = g.clone().unwrap();
            let nid = gg.find(&name).unwrap().id;
            gg.node_mut(nid).unwrap().data = json!({"strokes": all});
            vfx::store_graph(&mut self.doc, id, &gg, "Paint")?;
            return Ok(format!("{} strokes on {name} of clip {id}", gg.node(nid).unwrap().data["strokes"].as_array().map_or(0, |a| a.len())));
        }
        let spec = json!({"name": Self::uniq(g.as_ref(), "paint"), "kind": "paint", "data": {"strokes": strokes}});
        self.build(id, vec![spec], Map::new(), &format!("painted on clip {id}"))
    }

    /// `depth_of_field`: an AI depth pass and a lens blur by depth.
    pub(super) fn depth_of_field(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("depth_of_field needs \"clip\"")? as u32;
        let g = self.graph_now(id);
        let path = match op.get("depth").and_then(Value::as_str) {
            Some(p) => p.to_owned(),
            None => vfx::bake_depth(&self.doc, id, &JobCtx::detached())?,
        };
        let src_start = self.model_clip(id).map_or(0.0, |mc| mc.source_in as f64 / view::SEC as f64);
        let dn = Self::uniq(g.as_ref(), "depth_pass");
        let sd = Self::uniq(g.as_ref(), "depth");
        let main = g.as_ref().and_then(|g| g.output_id()).map_or(json!("input"), |o| json!(o));
        let mut z = Map::new();
        z.insert("size".into(), json!(op.get("size").and_then(Value::as_f64).unwrap_or(12.0)));
        match op.get("focus") {
            Some(Value::Number(d)) => {
                z.insert("focus".into(), json!(d));
            }
            Some(v) if xy(v).is_some() => {
                z.insert("focus_at_point".into(), json!(true));
                z.insert("focus_point".into(), v.clone());
            }
            _ => {
                z.insert("focus_at_point".into(), json!(true));
            }
        }
        if let Some(r) = op.get("range") {
            z.insert("depth_of_field".into(), r.clone());
        }
        let nodes = vec![
            json!({"name": dn, "kind": "read", "in": [], "data": {"path": path}, "params": {"start": src_start}}),
            json!({"name": sd, "kind": "set_depth", "in": [main, dn], "params": {"near": 1.0, "far": 100.0}}),
            json!({"name": Self::uniq(g.as_ref(), "dof"), "kind": "zdefocus", "in": [sd], "params": z}),
        ];
        self.build(id, nodes, Map::new(), &format!("depth of field on clip {id} from an AI depth pass ({path})"))
    }

    /// `grain_match`: matches an insert's grain to a reference plate.
    pub(super) fn grain_match(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("grain_match needs \"clip\": the clean, CG or inserted clip")? as u32;
        let (path, start) = self.path_of(op.get("reference").ok_or("grain_match needs \"reference\": the plate (clip id or file)")?)?;
        let g = self.graph_now(id);
        let rn = Self::uniq(g.as_ref(), "grain_ref");
        let main = g.as_ref().and_then(|g| g.output_id()).map_or(json!("input"), |o| json!(o));
        let nodes = vec![
            json!({"name": rn, "kind": "read", "in": [], "data": {"path": path}, "params": {"start": start, "fit": "Fill"}}),
            json!({"name": Self::uniq(g.as_ref(), "grain"), "kind": "grain_match", "in": [main, rn], "params": {"amount": op.get("amount").and_then(Value::as_f64).unwrap_or(1.0)}}),
        ];
        self.build(id, nodes, Map::new(), &format!("clip {id} now carries the reference's grain"))
    }

    /// `lens`: lens distortion and chromatic aberration.
    pub(super) fn lens(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("lens needs \"clip\"")? as u32;
        let g = self.graph_now(id);
        let mut params = Map::new();
        for k in ["k1", "k2", "chroma", "scale"] {
            if let Some(v) = op.get(k) {
                params.insert(k.into(), v.clone());
            }
        }
        params.insert("mode".into(), json!(if op.get("mode").and_then(Value::as_str) == Some("undistort") { "Undistort" } else { "Distort" }));
        let spec = json!({"name": Self::uniq(g.as_ref(), "lens"), "kind": "lens_distort", "params": params});
        self.build(id, vec![spec], Map::new(), &format!("lens distortion on clip {id}"))
    }

    /// `motion_blur`: blur along measured motion.
    pub(super) fn motion_blur(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("motion_blur needs \"clip\"")? as u32;
        let g = self.graph_now(id);
        let mv = Self::uniq(g.as_ref(), "motion");
        let nodes = vec![
            json!({"name": mv, "kind": "motion_vectors"}),
            json!({"name": Self::uniq(g.as_ref(), "vector_blur"), "kind": "vector_blur", "in": [mv], "params": {"shutter": op.get("shutter").and_then(Value::as_f64).unwrap_or(0.5)}}),
        ];
        self.build(id, nodes, Map::new(), &format!("motion blur from measured motion on clip {id}"))
    }

    // --------------------------------------------------------- matchmove

    /// `matchmove`: solves the camera of a moving shot.
    pub(super) fn matchmove(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("matchmove needs \"clip\": the moving footage")? as u32;
        let vc = self.clip(id)?;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let ft = crate::services::tracking::footage(&self.doc.project, &mc)?;
        let focal = f(op, "focal_mm").map(|mm| ft.width as f64 * mm / f(op, "sensor_mm").unwrap_or(36.0));
        let settings = studio_fx::solve::SolveSettings { focal, nodal: op.get("nodal").and_then(Value::as_bool), keyframe_step: None };
        let ignore = op.get("ignore").and_then(Value::as_str) == Some("cut_out");
        let run = vfx::solve_clip(&self.doc, id, settings, Default::default(), ignore, &JobCtx::detached())?;
        // The camera clip over the footage clip, keyed every frame.
        let w = (self.doc.project.width as f64);
        let mut params = m::space::CameraSettings::defaults(w);
        params.insert("two_node".into(), m::Value::Bool(false));
        let lane = self.room(None, vc.start, vc.dur, None)?;
        let cid = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(m::GeneratorKind::Camera);
        g.params = params;
        let mut cam = m::Clip::new(m::ClipId(cid as u64), m::LaneId(lane as u64), vc.start, vc.dur, m::ClipSource::Generator(g));
        cam.name = "Solved camera".into();
        vfx::key_camera(&self.doc, &mut cam, &mc, ft.fps, &run.range, &run.keys);
        let file = vfx::camera_file(&run);
        let dir = vfx::cameras_dir(&self.doc);
        let path = dir.join(format!("clip-{id}-camera.json"));
        std::fs::write(&path, serde_json::to_string(&file).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))?;
        cam.meta.insert("matchmove".into(), json!({"footage": id, "file": path.to_string_lossy(), "error": file.error_pixels, "nodal": file.nodal}));
        let seq = self.doc.seq();
        let ok = self.doc.apply("Solve camera", m::Edit::InsertClip { seq, clip: Box::new(cam) });
        self.check(ok, "")?;
        let mm = run.solved.focal * 36.0 / ft.width as f64;
        let mut said = format!(
            "camera solved for clip {id}: camera clip {cid}, {} of {} frames, {} corners tracked, {} 3D points, error {:.2} px, lens {:.1} mm (36 mm frame){}; solve saved to {}",
            run.solved.solved_frames(),
            run.range.end - run.range.start,
            run.tracks,
            run.solved.points.len(),
            run.solved.error,
            mm,
            if run.solved.nodal { ", nodal (a pan or tilt from one spot: things stick at any depth, but there is no real depth)" } else { "" },
            path.display()
        );
        if run.solved.error > 1.5 {
            said += ". The error is high: the shot may have moving things (cut them out and use ignore: \"cut_out\") or lens distortion (undistort with lens first)";
        }
        said += ". Put 3D layers or models on the ground with place_on_ground (clip, point [x, y] where it stands, at)";
        if let Some(e) = op.get("export").and_then(Value::as_str) {
            said += &format!("; {}", self.export_camera(&json!({"clip": cid, "path": e}))?);
        }
        Ok(said)
    }

    fn solve_file(&self, cam: u32) -> Result<studio_fx::solve::orient::CameraFile, String> {
        let mc = self.model_clip(cam).ok_or_else(|| format!("no clip {cam}"))?;
        let path = mc.meta.get("matchmove").and_then(|v| v.get("file")).and_then(Value::as_str).ok_or_else(|| format!("clip {cam} is not a solved camera (matchmove makes one)"))?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
    }

    fn solved_camera(&self) -> Option<u32> {
        self.doc.model.sequences.iter().flat_map(|s| s.clips.iter()).find(|c| c.meta.contains_key("matchmove")).map(|c| c.id.0 as u32)
    }

    /// `place`: stands a layer on the solved ground where a screen point
    /// shows it.
    pub(super) fn place_on_ground(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("place needs \"clip\": the layer or model to place")? as u32;
        let cam = op.get("camera").and_then(Value::as_u64).map(|v| v as u32).or_else(|| self.solved_camera()).ok_or("no solved camera: run matchmove first (or give camera)")?;
        let at = micros(f(op, "at").unwrap_or_else(|| secs(self.clip(cam).map(|c| c.start).unwrap_or(0))));
        let p = op.get("point").and_then(xy).ok_or("place needs \"point\": [x, y] canvas pixels from the centre where it should stand")?;
        let w = (self.doc.project.width as f64);
        let seq = self.doc.model.sequences.iter().find(|s| s.clip(m::ClipId(cam as u64)).is_some()).ok_or("the camera is gone")?;
        let cm = seq.clip(m::ClipId(cam as u64)).ok_or("the camera is gone")?;
        let view = m::space::camera_view(seq, cm, at.clamp(cm.start, cm.start + cm.duration - 1), w);
        let dir = view.ray(m::Vec2::new(p[0], p[1]));
        let lift = f(op, "lift").unwrap_or(0.0);
        let hit = match vfx::hit_ground(view.position, dir) {
            Some(h) => h,
            None => {
                // No ground there (or a nodal solve): use the nearest point
                // of the cloud along the ray, else a default distance.
                let file = self.solve_file(cam)?;
                let d = file
                    .points
                    .iter()
                    .map(|q| {
                        let v = [q[0] - view.position[0], q[1] - view.position[1], q[2] - view.position[2]];
                        let t = v[0] * dir[0] + v[1] * dir[1] + v[2] * dir[2];
                        let off = ((v[0] - dir[0] * t).powi(2) + (v[1] - dir[1] * t).powi(2) + (v[2] - dir[2] * t).powi(2)).sqrt();
                        (off / t.max(1e-6), t)
                    })
                    .filter(|(_, t)| *t > 0.0)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map_or(m::space::default_zoom(w), |x| x.1);
                [view.position[0] + dir[0] * d, view.position[1] + dir[1] * d, view.position[2] + dir[2] * d]
            }
        };
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let geo = crate::editor::xform::geo(&self.doc.project, &self.doc.model, id, at);
        // Stand it up: the bottom of what it shows on the ground, facing
        // the camera (a layer's picture can be larger than its content).
        let half_h = geo.map_or(0.0, |g| (g.content_point(0.0, 1.0).y as f64 - (g.source().y as f64 * 0.5 + mc.transform.anchor.y)) * g.fit() as f64 * mc.transform.scale_y);
        let flat = op.get("lie").and_then(Value::as_bool) == Some(true);
        mc.transform.three_d = true;
        mc.transform.position = m::Vec2::new(hit[0], hit[1] - lift - if flat { 0.0 } else { half_h });
        mc.transform.position_z = hit[2];
        let yaw = (view.axes[2][0]).atan2(view.axes[2][2]).to_degrees();
        if flat {
            (mc.transform.rotation_x, mc.transform.rotation_y) = (90.0, -yaw);
        } else {
            mc.transform.rotation_y = -yaw;
        }
        let seq_id = self.doc.seq();
        let ok = self.doc.apply("Place on ground", m::Edit::SetClip { seq: seq_id, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!(
            "clip {id} stands at [{:.0}, {:.0}, {:.0}] in the solved scene ({}), turned to face camera {cam}; it sticks to the footage as the camera moves. Check with frame at a few times",
            hit[0],
            hit[1],
            hit[2],
            if flat { "lying on the ground" } else { "standing on the ground" }
        ))
    }

    /// `export_camera`: writes a solved (or any) camera as .chan, .json or
    /// the point cloud as .obj.
    pub(super) fn export_camera(&mut self, op: &Value) -> Result<String, String> {
        let cam = op.get("clip").and_then(Value::as_u64).map(|v| v as u32).or_else(|| self.solved_camera()).ok_or("export_camera needs \"clip\": a camera")?;
        let path = op.get("path").and_then(Value::as_str).ok_or("export_camera needs \"path\" ending .chan, .json or .obj")?;
        let file = self.solve_file(cam)?;
        let units = f(op, "units").unwrap_or(0.01);
        let text = if path.ends_with(".chan") {
            studio_fx::solve::orient::to_chan(&file.keys, self.doc.project.height as f64, f(op, "first_frame").unwrap_or(1.0) as i64, units)
        } else if path.ends_with(".obj") {
            studio_fx::solve::orient::points_obj(&file.points, units)
        } else {
            serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?
        };
        std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))?;
        let note = if path.ends_with(".json") { String::new() } else { format!(" (units: {units} per Studio pixel; Y up)") };
        Ok(format!("camera {cam} written to {path}{note}"))
    }

    /// `import_camera`: a Nuke .chan (or Studio .json) as a camera clip.
    pub(super) fn import_camera(&mut self, op: &Value) -> Result<String, String> {
        let path = op.get("path").and_then(Value::as_str).ok_or("import_camera needs \"path\" (.chan or .json)")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let w = (self.doc.project.width as f64);
        let units = f(op, "units").unwrap_or(0.01);
        let keys = if path.ends_with(".json") {
            serde_json::from_str::<studio_fx::solve::orient::CameraFile>(&text).map_err(|e| e.to_string())?.keys
        } else {
            studio_fx::solve::orient::from_chan(&text, self.doc.project.height as f64, units, m::space::default_zoom(w))?
        };
        let fps = (self.doc.project.fps as f64).max(1.0);
        let at = micros(f(op, "at").unwrap_or(0.0));
        let frames = keys.iter().map(|k| k.frame).max().unwrap_or(0) as f64 + 1.0;
        let dur = micros(frames / fps);
        let lane = self.room(None, at, dur, None)?;
        let cid = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(m::GeneratorKind::Camera);
        g.params = m::space::CameraSettings::defaults(w);
        let mut cam = m::Clip::new(m::ClipId(cid as u64), m::LaneId(lane as u64), at, dur, m::ClipSource::Generator(g));
        cam.name = "Imported camera".into();
        // Keys by project frame.
        let mut pos = Vec::new();
        let mut parts: [Vec<m::Keyframe>; 5] = Default::default();
        for k in &keys {
            let t = micros(k.frame as f64 / fps);
            let key = |v: m::Value| m::Keyframe::new(t, v, m::Interp::Linear);
            pos.push(key(m::Value::Point(m::Vec2::new(k.position[0], k.position[1]))));
            parts[0].push(key(m::Value::Scalar(k.position[2])));
            parts[1].push(key(m::Value::Angle(k.rotation[0])));
            parts[2].push(key(m::Value::Angle(k.rotation[1])));
            parts[3].push(key(m::Value::Angle(k.rotation[2])));
            parts[4].push(key(m::Value::Scalar(k.zoom)));
        }
        if let Some(k) = keys.first() {
            cam.transform.position = m::Vec2::new(k.position[0], k.position[1]);
            cam.transform.position_z = k.position[2];
            (cam.transform.rotation_x, cam.transform.rotation_y, cam.transform.rotation) = (k.rotation[0], k.rotation[1], k.rotation[2]);
            if let m::ClipSource::Generator(g) = &mut cam.source {
                g.params.insert("zoom".into(), m::Value::Scalar(k.zoom));
                g.params.insert("two_node".into(), m::Value::Bool(false));
            }
        }
        if keys.len() > 1 {
            cam.keyframes.insert("transform.position".into(), m::Track::new(pos));
            for (path, ks) in ["transform.position_z", "transform.rotation_x", "transform.rotation_y", "transform.rotation", "generator.zoom"].iter().zip(parts) {
                cam.keyframes.insert((*path).into(), m::Track::new(ks));
            }
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Import camera", m::Edit::InsertClip { seq, clip: Box::new(cam) });
        self.check(ok, "")?;
        Ok(format!("camera from {path} as clip {cid}: {} frames from {:.2}s", keys.len(), secs(at)))
    }
}

#[cfg(test)]
mod tests;
