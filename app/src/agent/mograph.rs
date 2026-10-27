// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion graphics ops: shape layers, 3D text and extruded shapes,
//! expressions, time remapping, and the text extras (rich runs, text on a
//! path, kerning) the `text` and `set` ops take.
//!
//! * `shape`: `{"op": "shape", "items": [...]}` (see `studio_vector`) or
//!   `{"preset": "ring", "colour", "accent", "size", "width", "speed"}`,
//!   with `inputs` (settings the document reads as `$name`, keyable as
//!   properties), `at`, `duration`, `lane`, `x`, `y`, `name`, `behind`,
//!   `extrude` (`{"depth", "bevel", "colour"}`: a 3D model instead). On a
//!   `clip`: `items` replaces, `add` appends, `patch` changes in place
//!   (`[{"path": "/items/2/end", "value": 0.5}]`, `"remove": true`),
//!   `inputs` sets.
//! * `text_3d`: a word as a lit, extruded, bevelled 3D model.
//! * `expression`: a formula on a property (a modifier, or a driver when
//!   it reads other clips with `prop`).
//! * `time_remap`: which source second plays when, by keys or a formula.

use super::{Session, SEC, f, micros, secs};
use serde_json::{Value, json};
use studio_model as m;

/// Text fields the `text` and `set` ops hand here.
pub const TEXT_EXTRA_FIELDS: &[&str] = &["runs", "path", "kerning", "font", "weight", "italic"];

fn rgba(s: &str) -> Option<m::Rgba> {
    let k = super::craft::colour(s)?;
    Some(m::Rgba { r: k.r() as f64 / 255.0, g: k.g() as f64 / 255.0, b: k.b() as f64 / 255.0, a: k.a() as f64 / 255.0 })
}

/// An input as a generator setting.
fn input_value(v: &Value) -> Result<m::Value, String> {
    Ok(match v {
        Value::Number(n) => m::Value::Scalar(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => m::Value::Scalar(*b as u8 as f64),
        Value::String(s) if s.starts_with('#') || super::craft::colour(s).is_some() => m::Value::Colour(rgba(s).ok_or_else(|| format!("colour \"{s}\" is not #rrggbb or a basic name"))?),
        Value::String(s) => m::Value::Text(s.clone()),
        Value::Array(a) if a.len() == 2 && a.iter().all(Value::is_number) => m::Value::Point(m::Vec2::new(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0))),
        other => return Err(format!("input {other} is a number, a colour, a point [x, y] or text")),
    })
}

/// A preset's options from the op.
fn preset_options(op: &Value) -> studio_vector::presets::Options {
    let mut o = studio_vector::presets::Options::default();
    if let Some(c) = op.get("colour").or(op.get("color")).and_then(Value::as_str) {
        o.colour = c.to_owned();
    }
    if let Some(c) = op.get("accent").and_then(Value::as_str) {
        o.accent = c.to_owned();
    }
    if let Some(v) = f(op, "size") {
        o.size = v.max(1.0);
    }
    if let Some(v) = f(op, "width").or(f(op, "stroke_width")) {
        o.width = v.max(0.5);
    }
    if let Some(v) = f(op, "speed") {
        o.speed = v.max(0.05);
    }
    o
}

/// Hex and named colours in a document as `#rrggbb` (the vector crate
/// reads hex; names the agent knows go through its own table).
fn normalise_colours(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if matches!(k.as_str(), "colour" | "color") {
                    if let Value::String(s) = x
                        && !s.starts_with('#')
                        && !s.starts_with('$')
                        && !s.starts_with('=')
                        && let Some(c) = super::craft::colour(s)
                    {
                        *x = json!(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()));
                    }
                }
                normalise_colours(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalise_colours),
        _ => {}
    }
}

/// The document an op describes and the inputs it brings.
fn doc_of(op: &Value) -> Result<(Value, Vec<(String, m::Value)>), String> {
    let mut inputs = Vec::new();
    let mut doc = if let Some(name) = op.get("preset").and_then(Value::as_str) {
        let o = preset_options(op);
        let d = studio_vector::presets::make(name, &o).ok_or_else(|| format!("no shape preset \"{name}\"; presets: {}", studio_vector::presets::PRESETS.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")))?;
        for (k, v) in studio_vector::presets::inputs(name, &o) {
            inputs.push((k.to_owned(), m::Value::Scalar(v)));
        }
        d
    } else {
        match (op.get("items"), op.get("type").and_then(Value::as_str)) {
            (Some(Value::Array(a)), _) => json!({"items": a}),
            (Some(_), _) => return Err("shape \"items\" is a list of shape items".into()),
            // One plain shape, as the craft book reads ("type": "rect",
            // "width", "height", "roundness", "colour", "stroke"): made
            // into its items rather than refused.
            (None, Some(t)) if matches!(t, "rect" | "rectangle" | "ellipse" | "circle" | "polygon" | "star") => {
                let (w, h) = (f(op, "width").or_else(|| f(op, "size")).unwrap_or(200.0), f(op, "height").or_else(|| f(op, "size")).unwrap_or(200.0));
                let mut shape = json!({"type": t, "size": [w, h]});
                if let Some(r) = f(op, "roundness") {
                    // 0..1 of the shorter side, as card takes it.
                    shape["roundness"] = json!(if r <= 1.0 { r * w.min(h) / 2.0 } else { r });
                }
                for k in ["points", "radius", "inner"] {
                    if let Some(v) = op.get(k) {
                        shape[k] = v.clone();
                    }
                }
                let mut items = vec![shape, json!({"type": "fill", "colour": op.get("colour").cloned().unwrap_or(json!("#ffffff"))})];
                if let Some(c) = op.get("stroke") {
                    items.push(json!({"type": "stroke", "colour": c, "width": f(op, "stroke_width").unwrap_or(4.0)}));
                }
                json!({"items": items})
            }
            (None, Some(t)) => return Err(format!("shape \"type\" \"{t}\": rect, ellipse, circle, polygon or star for one plain shape; anything more is \"items\" (shapes, operators and paint, in order) or a \"preset\"")),
            (None, None) => return Err("shape needs \"items\" (shapes, operators and paint, in order), \"preset\", or \"type\" (rect, ellipse, polygon, star) with width, height, colour for one plain shape".into()),
        }
    };
    normalise_colours(&mut doc);
    if let Some(Value::Object(ins)) = op.get("inputs") {
        for (k, v) in ins {
            inputs.retain(|(n, _)| n != k);
            inputs.push((k.clone(), input_value(v)?));
        }
    }
    Ok((doc, inputs))
}

/// What a document covers over its clip, in canvas pixels from the
/// centre: (min, max).
fn coverage(doc: &Value, inputs: &[(String, m::Value)], dur: f64) -> Option<([f64; 2], [f64; 2])> {
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    for k in 0..=4 {
        let mut cx = studio_vector::Ctx::at(dur * k as f64 / 4.0, dur);
        for (n, v) in inputs {
            let i = match v {
                m::Value::Scalar(x) | m::Value::Angle(x) => studio_vector::Input::Num(*x),
                m::Value::Point(p) => studio_vector::Input::Point([p.x, p.y]),
                m::Value::Colour(c) => studio_vector::Input::Colour([c.r, c.g, c.b, c.a]),
                m::Value::Text(s) | m::Value::Choice(s) => studio_vector::Input::Text(s.clone()),
                _ => continue,
            };
            cx.inputs.insert(n.clone(), i);
        }
        for p in studio_vector::outlines(doc, &cx) {
            if let Some((a, b)) = p.bounds() {
                lo = [lo[0].min(a[0]), lo[1].min(a[1])];
                hi = [hi[0].max(b[0]), hi[1].max(b[1])];
            }
        }
    }
    lo[0].is_finite().then_some((lo, hi)).filter(|_| lo[0] <= hi[0])
}

fn describe(doc: &Value, inputs: &[(String, m::Value)], dur: f64, at: (f64, f64)) -> String {
    let n = doc.get("items").and_then(Value::as_array).map_or(0, Vec::len);
    let mut s = format!("{n} item(s)");
    if let Some((lo, hi)) = coverage(doc, inputs, dur) {
        s.push_str(&format!("; draws within x {:.0}..{:.0}, y {:.0}..{:.0} px on the canvas", lo[0] + at.0, hi[0] + at.0, lo[1] + at.1, hi[1] + at.1));
    }
    if !inputs.is_empty() {
        s.push_str(&format!("; inputs (keyable as generator.<name>): {}", inputs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")));
    }
    s
}

/// Flat outlines for extrusion: the document's outlines at its first
/// moment, flattened (canvas pixels).
fn flat_outlines(doc: &Value) -> Vec<Vec<[f64; 2]>> {
    studio_vector::outlines(doc, &studio_vector::Ctx::at(0.0, 1.0)).iter().flat_map(|p| p.subs.iter().filter(|s| s.closed).map(|s| s.flatten(0.5))).filter(|c| c.len() >= 3).collect()
}

/// The key a 3D text keeps its words and look under.
pub const TEXT_3D: &str = "text_3d";

/// A model mesh from a solid: face, bevel and side materials.
fn mesh_of(s: studio_vector::extrude::Solid, face: [f32; 4], side: [f32; 4], metallic: f32, roughness: f32) -> studio_mesh::Mesh {
    let mat = |name: &str, c: [f32; 4]| studio_mesh::Material { name: name.into(), base_colour: c, metallic, roughness, ..Default::default() };
    let [a, b, c] = s.parts;
    let mut groups = vec![studio_mesh::Group { start: 0, count: a, material: 0 }];
    if b > 0 {
        groups.push(studio_mesh::Group { start: a, count: b, material: 1 });
    }
    groups.push(studio_mesh::Group { start: a + b, count: c, material: 2 });
    studio_mesh::Mesh { positions: s.positions, normals: s.normals, uvs: s.uvs, indices: s.indices, groups, materials: vec![mat("Face", face), mat("Bevel", face), mat("Side", side)], ..Default::default() }
}

fn linear(c: m::Rgba) -> [f32; 4] {
    let l = c.to_linear();
    [l[0] as f32, l[1] as f32, l[2] as f32, l[3] as f32]
}

impl Session {
    /// The motion graphics ops, by name.
    pub(super) fn mograph(&mut self, name: &str, op: &Value) -> Result<String, String> {
        match name {
            "shape" => self.shape(op),
            "text_3d" => self.text_3d(op),
            "expression" => self.expression(op),
            "time_remap" => self.time_remap(op),
            "template" => self.template(op),
            "chart" => self.chart(op),
            other => Err(format!("unknown motion graphics op \"{other}\"")),
        }
    }

    /// A picture lane for a new layer, as `card` finds one.
    fn layer_lane(&mut self, op: &Value, at: i64, dur: i64) -> Result<u32, String> {
        let behind = match op.get("behind").and_then(Value::as_u64) {
            Some(b) => Some(self.clip(b as u32)?.lane),
            None => None,
        };
        match op.get("lane") {
            Some(v) => {
                let want = super::craft::lane_of(&self.doc.project, v, None)?;
                self.room(Some(want), at, dur, behind)
            }
            None => self.room(None, at, dur, behind),
        }
    }

    fn shape(&mut self, op: &Value) -> Result<String, String> {
        if let Some(id) = op.get("clip").and_then(Value::as_u64) {
            return self.shape_edit(id as u32, op);
        }
        if let Some(file) = op.get("lottie").and_then(Value::as_str) {
            return self.shape_from_lottie(file, op);
        }
        let (doc, inputs) = doc_of(op)?;
        if let Some(w) = studio_vector::check(&doc).first() {
            return Err(format!("shape: {w}"));
        }
        let at = micros(f(op, "at").unwrap_or(0.0));
        let dur = micros(f(op, "duration").unwrap_or(3.0)).max(SEC / 30);
        if let Some(ex) = op.get("extrude") {
            let contours = flat_outlines(&doc);
            if contours.is_empty() {
                return Err("nothing closed to extrude: give the shape closed outlines (rect, ellipse, star, a closed path)".into());
            }
            return self.solid_model(op, ex, &contours, at, dur, op.get("name").and_then(Value::as_str).unwrap_or("Shape 3D"));
        }
        let lane = self.layer_lane(op, at, dur)?;
        let id = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(m::GeneratorKind::Shape);
        g.params.insert("vector".into(), m::Value::Text(serde_json::to_string(&doc).map_err(|e| e.to_string())?));
        for (k, v) in &inputs {
            g.params.insert(k.clone(), v.clone());
        }
        let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), at, dur, m::ClipSource::Generator(g));
        c.name = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| op.get("preset").and_then(Value::as_str).map_or("Shape".into(), |p| format!("{p} shape")));
        let (x, y) = (f(op, "x").unwrap_or(0.0), f(op, "y").unwrap_or(0.0));
        c.transform.position = m::Vec2::new(x, y);
        if let Some(o) = f(op, "opacity") {
            c.transform.opacity = (o / 100.0).clamp(0.0, 1.0);
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Shape layer", m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(format!("shape clip {id}: {}", describe(&doc, &inputs, secs(dur), (x, y))))
    }

    /// Every shape layer of a Lottie file as an editable shape layer,
    /// timed as in the file from `at`.
    fn shape_from_lottie(&mut self, file: &str, op: &Value) -> Result<String, String> {
        let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
        let lot = studio_lottie::Animation::document(&bytes)?;
        let num = |k: &str, d: f64| lot.get(k).and_then(Value::as_f64).unwrap_or(d);
        let (fps, w, h) = (num("fr", 30.0).max(1.0), num("w", 1920.0), num("h", 1080.0));
        let (ip, op_frame) = (num("ip", 0.0), num("op", 60.0));
        let at = f(op, "at").unwrap_or(0.0);
        // The composition fitted to the canvas, as the player draws it.
        let fit = (self.doc.project.width as f64 / w).min(self.doc.project.height as f64 / h);
        let layers: Vec<Value> = lot.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut made = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        // Lottie lists the top layer first: add from the bottom.
        for layer in layers.iter().rev() {
            let Some((mut doc, more)) = studio_vector::lottie::import(layer, fps, [w, h]) else {
                if let Some(n) = layer.get("nm").and_then(Value::as_str) {
                    notes.push(format!("\"{n}\" is not a shape layer"));
                }
                continue;
            };
            notes.extend(more);
            if (fit - 1.0).abs() > 1e-6 {
                doc = json!({"items": [{"type": "group", "scale": fit, "items": doc["items"].clone()}]});
            }
            let l_in = layer.get("ip").and_then(Value::as_f64).unwrap_or(ip).max(ip);
            let l_out = layer.get("op").and_then(Value::as_f64).unwrap_or(op_frame).min(op_frame);
            if l_out <= l_in {
                continue;
            }
            // Key times count from the composition's frame 0, so the clip
            // starts there too.
            let name = layer.get("nm").and_then(Value::as_str).unwrap_or("Lottie shape").to_owned();
            let one = json!({"op": "shape", "items": doc["items"], "at": at, "duration": op_frame / fps, "name": name});
            let r = self.shape(&one)?;
            made.push(r);
        }
        if made.is_empty() {
            return Err(format!("{file} has no shape layers to bring in{}", if notes.is_empty() { String::new() } else { format!(" ({})", notes.join("; ")) }));
        }
        notes.dedup();
        Ok(format!("{} shape layer(s) from {file}: {}{}", made.len(), made.join("; "), if notes.is_empty() { String::new() } else { format!(". Not brought over: {}", notes.join("; ")) }))
    }

    fn shape_edit(&mut self, id: u32, op: &Value) -> Result<String, String> {
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let dur = secs(mc.duration);
        let at = (mc.transform.position.x, mc.transform.position.y);
        let m::ClipSource::Generator(g) = &mut mc.source else { return Err(format!("clip {id} is not a shape layer")) };
        if g.kind != m::GeneratorKind::Shape {
            return Err(format!("clip {id} is not a shape layer"));
        }
        let mut doc: Value = match g.params.get("vector") {
            Some(m::Value::Text(t)) => serde_json::from_str(t).unwrap_or(json!({"items": []})),
            _ => json!({"items": []}),
        };
        if doc.is_array() {
            doc = json!({"items": doc});
        }
        let mut said = Vec::new();
        if op.get("items").is_some() || op.get("preset").is_some() {
            let (d, ins) = doc_of(op)?;
            doc = d;
            for (k, v) in ins {
                g.params.insert(k, v);
            }
            said.push("new items".to_owned());
        }
        if let Some(Value::Array(more)) = op.get("add") {
            let mut more = Value::Array(more.clone());
            normalise_colours(&mut more);
            if let (Some(items), Value::Array(more)) = (doc.get_mut("items").and_then(Value::as_array_mut), more) {
                said.push(format!("{} item(s) added", more.len()));
                items.extend(more);
            }
        }
        if let Some(Value::Array(patches)) = op.get("patch") {
            for p in patches {
                let path = p.get("path").and_then(Value::as_str).ok_or("each patch is {\"path\": \"/items/1/colour\", \"value\": …} or {\"path\", \"remove\": true}")?;
                if p.get("remove").and_then(Value::as_bool) == Some(true) {
                    let (parent, last) = path.rsplit_once('/').ok_or("patch path starts with /")?;
                    match doc.pointer_mut(parent) {
                        Some(Value::Array(a)) => {
                            let k: usize = last.parse().map_err(|_| format!("{path}: not a list index"))?;
                            if k >= a.len() {
                                return Err(format!("{path}: no such item"));
                            }
                            a.remove(k);
                        }
                        Some(Value::Object(o)) => {
                            o.remove(last);
                        }
                        _ => return Err(format!("{path}: nothing there")),
                    }
                } else {
                    let mut v = p.get("value").cloned().ok_or_else(|| format!("{path}: needs \"value\""))?;
                    normalise_colours(&mut v);
                    // A colour set on its own: a name becomes hex too.
                    if (path.ends_with("/colour") || path.ends_with("/color"))
                        && let Value::String(c) = &v
                        && !c.starts_with(['#', '$', '='])
                        && let Some(k) = super::craft::colour(c)
                    {
                        v = json!(format!("#{:02x}{:02x}{:02x}", k.r(), k.g(), k.b()));
                    }
                    let (parent, last) = path.rsplit_once('/').ok_or("patch path starts with /")?;
                    match doc.pointer_mut(parent) {
                        Some(Value::Object(o)) => {
                            o.insert(last.to_owned(), v);
                        }
                        Some(Value::Array(a)) => {
                            let k: usize = last.parse().map_err(|_| format!("{path}: not a list index"))?;
                            if k < a.len() {
                                a[k] = v;
                            } else if k == a.len() {
                                a.push(v);
                            } else {
                                return Err(format!("{path}: past the end"));
                            }
                        }
                        _ => return Err(format!("{path}: nothing there")),
                    }
                }
            }
            said.push(format!("{} patch(es)", patches.len()));
        }
        if let Some(Value::Object(ins)) = op.get("inputs") {
            for (k, v) in ins {
                g.params.insert(k.clone(), input_value(v)?);
            }
            said.push(format!("inputs {}", ins.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
        if let Some(w) = studio_vector::check(&doc).first() {
            return Err(format!("shape: {w}"));
        }
        g.params.insert("vector".into(), m::Value::Text(serde_json::to_string(&doc).map_err(|e| e.to_string())?));
        let inputs: Vec<(String, m::Value)> = g.params.iter().filter(|(k, _)| !matches!(k.as_str(), "vector" | "colour" | "fill" | "align" | "shape" | "size")).map(|(k, v)| (k.clone(), v.clone())).collect();
        let summary = describe(&doc, &inputs, dur, at);
        let seq = self.doc.seq();
        let ok = self.doc.apply("Shape layer", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("shape clip {id}: {}; {summary}", if said.is_empty() { "unchanged".to_owned() } else { said.join(", ") }))
    }

    /// A model made from flat outlines (canvas pixels).
    fn solid_model(&mut self, op: &Value, ex: &Value, contours: &[Vec<[f64; 2]>], at: i64, dur: i64, name: &str) -> Result<String, String> {
        let get = |k: &str| ex.get(k).and_then(Value::as_f64).or_else(|| f(op, k));
        let (lo, hi) = contours.iter().flatten().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
        let tall = (hi[1] - lo[1]).max(1.0);
        let depth = get("depth").unwrap_or(tall * 0.35).max(1.0);
        let bevel = get("bevel").unwrap_or(depth * 0.08).max(0.0);
        let solid = studio_vector::extrude::extrude(contours, depth, bevel, get("smooth").unwrap_or(30.0));
        if solid.indices.is_empty() {
            return Err("the outline made no solid".into());
        }
        let col = |k: &str, d: &str| -> Result<m::Rgba, String> {
            let s = ex.get(k).or(op.get(k)).and_then(Value::as_str).unwrap_or(d);
            rgba(s).ok_or_else(|| format!("{k} \"{s}\" is not #rrggbb or a basic name"))
        };
        let face = col("colour", "#f2f2f2")?;
        let side = match ex.get("side_colour").or(op.get("side_colour")).and_then(Value::as_str) {
            Some(_) => col("side_colour", "#888888")?,
            None => m::Rgba { r: face.r * 0.55, g: face.g * 0.55, b: face.b * 0.55, a: 1.0 },
        };
        let mesh = mesh_of(solid, linear(face), linear(side), get("metallic").unwrap_or(0.0) as f32, get("roughness").unwrap_or(0.35) as f32);
        let tris = mesh.triangles();
        let file = crate::mesh_store::save(&self.doc, &mesh)?;
        // Editing the words of a 3D text: the same clip, a new mesh.
        if let Some(old) = op.get("replace").and_then(Value::as_u64) {
            let mut mc = self.model_clip(old as u32).ok_or_else(|| format!("no clip {old}"))?;
            if let m::ClipSource::Generator(g) = &mut mc.source {
                g.params.insert("file".into(), m::Value::Text(file));
            }
            mc.name = name.to_owned();
            if let Some(t) = op.get("text") {
                mc.meta.insert(TEXT_3D.into(), json!({"text": t, "style": op.clone()}));
            }
            let seq = self.doc.seq();
            let ok = self.doc.apply("3D text", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("3D model clip {old} ({name}): {tris} triangles"));
        }
        let lane = self.layer_lane(op, at, dur)?;
        let id = self.doc.alloc_id() as u32;
        let mut c = crate::mesh_store::new_clip(id as u64, lane as u64, at, dur, file, name);
        c.transform.position = m::Vec2::new(f(op, "x").unwrap_or(0.0), f(op, "y").unwrap_or(0.0));
        c.transform.position_z = f(op, "z").unwrap_or(0.0);
        if let Some(Value::Array(r)) = op.get("rotation") {
            let g = |k: usize| r.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            (c.transform.rotation_x, c.transform.rotation_y, c.transform.rotation) = (g(0), g(1), g(2));
        }
        let seq = self.doc.seq();
        // A 3D text remembers its words and look, so they can be edited.
        if let Some(t) = op.get("text").filter(|_| op.get("replace").is_none() && op.get("op").and_then(Value::as_str) == Some("text_3d")) {
            c.meta.insert(TEXT_3D.into(), json!({"text": t, "style": op.clone()}));
        }
        let ok = self.doc.apply("3D shape", m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(format!("3D model clip {id} ({name}): {tris} triangles, {:.0} px wide, {:.0} tall, {depth:.0} deep, bevel {bevel:.0}; lit by the scene's lights, turn it with rotation_x / rotation_y", hi[0] - lo[0], tall))
    }

    fn text_3d(&mut self, op: &Value) -> Result<String, String> {
        // `{"op": "text_3d", "clip": 7, "text": "New words"}` changes the words
        // of a placed 3D text and keeps its look.
        if let Some(id) = op.get("clip").and_then(Value::as_u64) {
            let old = self.model_clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
            let kept = old.meta.get(TEXT_3D).and_then(|m| m.get("style")).cloned().ok_or_else(|| format!("clip {id} is not a 3D text"))?;
            let mut merged = kept;
            if let (Some(o), Some(n)) = (merged.as_object_mut(), op.as_object()) {
                for (k, v) in n {
                    o.insert(k.clone(), v.clone());
                }
                o.insert("replace".into(), json!(id));
                o.insert("at".into(), json!(secs(old.start)));
                o.insert("duration".into(), json!(secs(old.duration)));
                o.remove("clip");
            }
            return self.text_3d(&merged);
        }
        let text = op.get("text").and_then(Value::as_str).ok_or("text_3d needs \"text\"")?.to_owned();
        let (w, h) = (self.doc.project.width.max(16), self.doc.project.height.max(16));
        let canvas = studio_text::Canvas::new(w, h);
        let ppu = canvas.px_per_unit() as f64;
        let size_px = f(op, "size").unwrap_or(h as f64 * 0.16).clamp(4.0, 4000.0);
        let family = op.get("font").and_then(Value::as_str).unwrap_or("Manrope");
        let weight = f(op, "weight").unwrap_or(800.0) as u16;
        let italic = op.get("italic").and_then(Value::as_bool).unwrap_or(false);
        let mut style = studio_text::TextStyle::new(studio_text::FontRequest::new(family, weight, italic), (size_px / ppu) as f32, studio_text::Fill::default());
        if let Some(ls) = f(op, "letter_spacing") {
            style.letter_spacing = ls as f32;
        }
        let engine = crate::text_look::engine();
        let outlines = engine.outlines(&studio_text::TextBlock::plain(text.clone()), &style, &canvas);
        let contours: Vec<Vec<[f64; 2]>> = outlines.iter().flat_map(|g| g.contours.iter().map(|c| c.iter().map(|p| [p[0] as f64 * ppu, p[1] as f64 * ppu]).collect())).collect();
        if contours.is_empty() {
            return Err(format!("\"{text}\" has no letters to make solid"));
        }
        let at = micros(f(op, "at").unwrap_or(0.0));
        let dur = micros(f(op, "duration").unwrap_or(3.0)).max(SEC / 30);
        let ex = json!({"depth": f(op, "depth").unwrap_or(size_px * 0.3), "bevel": f(op, "bevel").unwrap_or(size_px * 0.025)});
        let name = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("3D {}", text.chars().take(20).collect::<String>()));
        self.solid_model(op, &ex, &contours, at, dur, &name)
    }

    fn expression(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("expression needs \"clip\"")? as u32;
        let prop = op.get("property").and_then(Value::as_str).ok_or("expression needs \"property\" (rotation, x, opacity, scale, generator.<name>, time_remap…)")?;
        if prop == "time_remap" {
            return self.time_remap(&json!({"clip": id, "expr": op.get("expr"), "remove": op.get("remove")}));
        }
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let a = self.modifier(&json!({"clip": id, "property": prop, "remove": "expression"}));
            let b = self.driver(&json!({"clip": id, "property": prop, "remove": true}));
            return match (a, b) {
                (Ok(x), Ok(y)) => Ok(format!("{x}; {y}")),
                (Ok(x), Err(_)) | (Err(_), Ok(x)) => Ok(x),
                (Err(e), Err(_)) => Err(e),
            };
        }
        let expr = op.get("expr").and_then(Value::as_str).ok_or("expression needs \"expr\", e.g. \"wiggle(2, 15)\" or \"loop_out(\\\"pingpong\\\")\"")?;
        let parsed = m::modifier::expr::parse(expr).map_err(|e| format!("expression: {e}"))?;
        // Reading other clips needs the sequence: a driver.
        if parsed.links() || op.get("driver").and_then(Value::as_bool) == Some(true) {
            return self.driver(&json!({"clip": id, "property": prop, "expr": expr}));
        }
        self.modifier(&json!({"clip": id, "property": prop, "add": "expression", "expr": expr}))
    }

    fn time_remap(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("time_remap needs \"clip\"")? as u32;
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if !matches!(mc.source, m::ClipSource::Media(_) | m::ClipSource::Sequence(_)) {
            return Err(format!("clip {id} has no footage to remap (it is generated)"));
        }
        let seq = self.doc.seq();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            mc.keyframes.remove(m::clip::TIME_REMAP);
            mc.drivers.remove(m::clip::TIME_REMAP);
            let ok = self.doc.apply("Time remap", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("clip {id} plays at its speed again"));
        }
        let mut keys: Vec<m::Keyframe> = Vec::new();
        match op.get("keys") {
            Some(Value::Array(ks)) => {
                for k in ks {
                    let a = k.as_array().filter(|a| a.len() >= 2).ok_or("time_remap keys are [clip second, source second] (optionally a third: the curve)")?;
                    let (t, s) = (a[0].as_f64().ok_or("key time is a number")?, a[1].as_f64().ok_or("source time is a number")?);
                    if t < 0.0 || s < 0.0 {
                        return Err("time_remap times are 0 or more".into());
                    }
                    let interp = match a.get(2).and_then(Value::as_str) {
                        Some("hold") => m::Interp::Hold,
                        Some("linear") => m::Interp::Linear,
                        Some(_) => m::Interp::Ease,
                        None => m::Interp::Linear,
                    };
                    keys.push(m::Keyframe::new(micros(t), m::Value::Scalar(s), interp));
                }
            }
            Some(_) => return Err("time_remap keys are a list".into()),
            None => {
                // As it plays now, so a formula has a value to work from.
                let (a, b) = (mc.source_time_at(mc.start), mc.source_time_at(mc.start + mc.duration));
                keys.push(m::Keyframe::new(0, m::Value::Scalar(secs(a)), m::Interp::Linear));
                keys.push(m::Keyframe::new(mc.duration, m::Value::Scalar(secs(b)), m::Interp::Linear));
            }
        }
        mc.keyframes.insert(m::clip::TIME_REMAP.into(), m::Track::new(keys));
        let mut said = format!("clip {id} time remapped");
        if let Some(e) = op.get("expr").and_then(Value::as_str) {
            let d = m::Driver { expr: e.to_owned(), vars: Vec::new(), enabled: true };
            if let Some(p) = d.problem() {
                return Err(p);
            }
            mc.drivers.insert(m::clip::TIME_REMAP.into(), d);
            said.push_str(&format!(" by {e} (t: timeline seconds, value: the keyed source second)"));
        }
        let ok = self.doc.apply("Time remap", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(said)
    }

    /// Rich runs, a path and kerning on a title's generator (called by
    /// the text motion step of `text` and `set`).
    pub(super) fn text_extras(&self, g: &mut m::Generator, op: &Value) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        // The title's own font: family, weight, italic. An unknown family
        // is refused (with the closest installed names) rather than drawn
        // in a stand-in.
        let cur_text = |g: &m::Generator, k: &str| match g.params.get(k) {
            Some(m::Value::Text(t) | m::Value::Choice(t)) => Some(t.clone()),
            _ => None,
        };
        if ["font", "weight", "italic"].iter().any(|k| op.get(*k).is_some()) {
            let clear = |v: Option<&Value>| matches!(v, Some(Value::Null) | Some(Value::Bool(false)));
            if clear(op.get("font")) {
                g.params.remove("font");
                said.push("the style's own font".into());
            }
            let family = match op.get("font") {
                Some(Value::String(f)) if !f.trim().is_empty() => Some(f.trim().to_owned()),
                Some(Value::String(_)) | Some(Value::Null) | Some(Value::Bool(false)) | None => None,
                Some(_) => return Err("font is a family name, e.g. \"Playfair Display\" (`fonts` lists and searches them)".into()),
            };
            let weight = match op.get("weight") {
                Some(Value::Number(n)) => Some(n.as_f64().unwrap_or(400.0).clamp(100.0, 900.0)),
                Some(Value::String(w)) => Some(match w.to_lowercase().as_str() {
                    "thin" => 100.0,
                    "extralight" | "extra light" | "ultralight" => 200.0,
                    "light" => 300.0,
                    "regular" | "normal" => 400.0,
                    "medium" => 500.0,
                    "semibold" | "semi bold" | "demibold" => 600.0,
                    "bold" => 700.0,
                    "extrabold" | "extra bold" | "ultrabold" => 800.0,
                    "black" | "heavy" => 900.0,
                    other => return Err(format!("weight \"{other}\": 100 to 900, or thin, light, regular, medium, semibold, bold, extrabold, black")),
                }),
                Some(Value::Null) | Some(Value::Bool(false)) => {
                    g.params.remove("font_weight");
                    None
                }
                None => None,
                Some(_) => return Err("weight is 100 to 900 (400 regular, 700 bold)".into()),
            };
            let italic = match op.get("italic") {
                Some(Value::Bool(b)) => Some(*b),
                Some(Value::Null) | None => None,
                Some(_) => return Err("italic is true or false".into()),
            };
            // Report against the family that will draw: the new one, the
            // one set before, or else the style's (not checked here).
            let fam_now = family.clone().or_else(|| cur_text(g, "font"));
            let w_now = weight.or(match g.params.get("font_weight") {
                Some(m::Value::Scalar(w)) => Some(*w),
                _ => None,
            });
            let i_now = italic.or(match (g.params.get("font_italic_set"), g.params.get("font_italic")) {
                (Some(m::Value::Bool(true)), Some(m::Value::Bool(b))) => Some(*b),
                _ => None,
            });
            if let Some(fam) = &fam_now {
                said.push(format!("font {}", crate::text_look::font_report(fam, w_now.unwrap_or(400.0) as u16, i_now.unwrap_or(false))?));
            } else {
                if let Some(w) = weight {
                    said.push(format!("weight {}", w as u16));
                }
                if let Some(i) = italic {
                    said.push(if i { "italic".into() } else { "upright".into() });
                }
            }
            if let Some(f) = family {
                g.params.insert("font".into(), m::Value::Text(f));
            }
            if let Some(w) = weight {
                g.params.insert("font_weight".into(), m::Value::Scalar(w));
            }
            if let Some(i) = italic {
                g.params.insert("font_italic_set".into(), m::Value::Bool(true));
                g.params.insert("font_italic".into(), m::Value::Bool(i));
            }
        }
        if let Some(Value::Array(runs)) = op.get("runs") {
            let mut out = Vec::new();
            for r in runs {
                let text = r.get("text").and_then(Value::as_str).ok_or("each run is {\"text\", …style}")?;
                let mut style = m::ParamMap::new();
                for (k, v) in r.as_object().into_iter().flatten() {
                    let val = match (k.as_str(), v) {
                        ("text", _) => continue,
                        ("bold" | "italic" | "underline" | "strikethrough", Value::Bool(b)) => m::Value::Bool(*b),
                        ("colour" | "color", Value::String(s)) => m::Value::Colour(rgba(s).ok_or_else(|| format!("colour \"{s}\" is not #rrggbb or a basic name"))?),
                        ("size" | "weight" | "tracking" | "letter_spacing" | "baseline_shift", Value::Number(n)) => m::Value::Scalar(n.as_f64().unwrap_or(0.0)),
                        ("font", Value::String(s)) => {
                            let w = r.get("weight").and_then(Value::as_f64).unwrap_or(400.0) as u16;
                            let it = r.get("italic").and_then(Value::as_bool).unwrap_or(false);
                            crate::text_look::font_report(s, w, it).map_err(|e| format!("run \"{text}\": {e}"))?;
                            m::Value::Text(s.clone())
                        }
                        (other, _) => return Err(format!("run setting \"{other}\": bold, italic, underline, strikethrough, colour, size (× the title's), weight (100–900), tracking (%), baseline_shift (px, + up), font")),
                    };
                    let key = if k == "color" { "colour".to_owned() } else if k == "tracking" { "letter_spacing".to_owned() } else { k.clone() };
                    style.insert(key, val);
                }
                out.push(m::TextRun { text: text.to_owned(), style });
            }
            said.push(format!("{} styled run(s)", out.len()));
            g.text = out;
        }
        if let Some(p) = op.get("path") {
            match p {
                Value::Null | Value::Bool(false) => {
                    g.params.remove("text_path");
                    said.push("no path".into());
                }
                _ => {
                    let spec = path_spec(p)?;
                    if let Some(s) = p.get("start").and_then(Value::as_f64) {
                        g.params.insert("path_start".into(), m::Value::Scalar(s));
                    }
                    g.params.insert("text_path".into(), m::Value::Text(spec.to_string()));
                    said.push("on a path (key generator.path_start 0..1 to run it along)".into());
                }
            }
        }
        if let Some(k) = op.get("kerning").and_then(Value::as_bool) {
            g.params.insert("kerning".into(), m::Value::Bool(k));
            said.push(format!("kerning {}", if k { "on" } else { "off" }));
        }
        Ok(said)
    }
}

/// A text path from the op: `{"circle": r}`, `{"arc": r, "degrees"}`,
/// `{"wave": amplitude, "width", "waves"}`, `{"d": "M…"}` (SVG path data)
/// or `{"points": [[x, y], …], "smooth": true}`, all canvas pixels from
/// the title's centre; with `offset`, `reverse`, `perpendicular`.
pub(super) fn path_spec(p: &Value) -> Result<Value, String> {
    let num = |k: &str, d: f64| p.get(k).and_then(Value::as_f64).unwrap_or(d);
    let (path, closed): (studio_vector::Path, bool) = if let Some(r) = p.get("circle").and_then(Value::as_f64) {
        // From the left, clockwise over the top.
        let mut sub = studio_vector::ops::ellipse([0.0, 0.0], [r * 2.0, r * 2.0]);
        sub.v.rotate_left(3);
        (studio_vector::Path::new(vec![sub]), true)
    } else if let Some(r) = p.get("arc").and_then(Value::as_f64) {
        let deg = num("degrees", 180.0).clamp(1.0, 359.0);
        let doc = json!([{"type": "arc", "radius": r, "start": -deg / 2.0, "end": deg / 2.0, "position": [0, r * 0.5]}]);
        (studio_vector::outlines(&doc, &studio_vector::Ctx::default()).into_iter().next().unwrap_or_default(), false)
    } else if let Some(a) = p.get("wave").and_then(Value::as_f64) {
        let width = num("width", 1200.0);
        let waves = num("waves", 2.0).max(0.25);
        let pts: Vec<[f64; 2]> = (0..=128).map(|k| {
            let u = k as f64 / 128.0;
            [(u - 0.5) * width, -a * (u * waves * std::f64::consts::TAU).sin()]
        }).collect();
        (studio_vector::Path::new(vec![studio_vector::path::Sub::polyline(&pts, false)]), false)
    } else if let Some(d) = p.get("d").and_then(Value::as_str) {
        let path = studio_vector::path::parse_svg(d)?;
        let closed = path.subs.first().is_some_and(|s| s.closed);
        (path, closed)
    } else if let Some(Value::Array(pts)) = p.get("points") {
        let pts: Vec<[f64; 2]> = pts.iter().filter_map(|q| Some([q.get(0)?.as_f64()?, q.get(1)?.as_f64()?])).collect();
        if pts.len() < 2 {
            return Err("a text path needs two or more points".into());
        }
        let closed = p.get("closed").and_then(Value::as_bool).unwrap_or(false);
        let sub = if p.get("smooth").and_then(Value::as_bool).unwrap_or(true) { studio_vector::path::smooth(&pts, closed, 1.0) } else { studio_vector::path::Sub::polyline(&pts, closed) };
        (studio_vector::Path::new(vec![sub]), closed)
    } else {
        return Err("path is {\"circle\": radius}, {\"arc\": radius, \"degrees\"}, {\"wave\": height, \"width\", \"waves\"}, {\"d\": \"M…\"} or {\"points\": [[x, y], …]} (canvas pixels from the title's centre); null removes it".into());
    };
    let mut pts = path.polyline(1.0);
    if closed && pts.len() > 1 {
        pts.pop();
    }
    if pts.len() < 2 {
        return Err("that path is empty".into());
    }
    Ok(json!({
        "points": pts.iter().map(|q| [((q[0] * 10.0).round() / 10.0) as f32, ((q[1] * 10.0).round() / 10.0) as f32]).collect::<Vec<_>>(),
        "closed": closed,
        "offset": num("offset", 0.0),
        "perpendicular": p.get("perpendicular").and_then(Value::as_bool).unwrap_or(true),
        "reverse": p.get("reverse").and_then(Value::as_bool).unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session() -> Session {
        Session::new(services::project::new_custom("Mograph", 640, 360, 30.0))
    }

    fn newest(s: &Session) -> u32 {
        s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
    }

    #[test]
    fn a_3d_text_changes_its_words_and_keeps_its_clip() {
        let mut s = session();
        s.edit(&[json!({"op": "text_3d", "text": "HELLO", "at": 0, "duration": 2})], "3d", false).unwrap();
        let id = newest(&s);
        assert_eq!(s.model_clip(id).unwrap().meta[TEXT_3D]["text"], "HELLO");
        let before = s.doc.project.clips.len();
        s.edit(&[json!({"op": "text_3d", "clip": id, "text": "WORLD"})], "words", false).unwrap();
        assert_eq!(s.doc.project.clips.len(), before, "the same clip, not a new one");
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.meta[TEXT_3D]["text"], "WORLD");
        assert!(mc.name.contains("WORLD"), "{}", mc.name);
        assert!(s.edit(&[json!({"op": "text_3d", "clip": 999999, "text": "x"})], "bad", false).is_err());
    }

    #[test]
    fn shapes_are_made_patched_and_keyed() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "shape", "at": 0, "duration": 2, "items": [{"type": "rect", "size": [200, 100], "roundness": 10}, {"type": "fill", "colour": "red"}, {"type": "trim", "end": "$grow"}, {"type": "stroke", "colour": "white", "width": 6}], "inputs": {"grow": 0.5}})], "shape", false).unwrap();
        assert!(r.to_string().contains("inputs"), "{r}");
        let id = newest(&s);
        let mc = s.model_clip(id).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!() };
        assert!(matches!(g.params.get("vector"), Some(m::Value::Text(t)) if t.contains("#")), "names become hex");
        // Keyed like any property.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "generator.grow", "keys": [[0, 0], [1, 1]]})], "grow", false).unwrap();
        assert!(s.model_clip(id).unwrap().keyframes.contains_key("generator.grow"));
        s.edit(&[json!({"op": "shape", "clip": id, "patch": [{"path": "/items/1/colour", "value": "blue"}, {"path": "/items/2", "remove": true}]})], "patch", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!() };
        let Some(m::Value::Text(t)) = g.params.get("vector") else { panic!() };
        let d: Value = serde_json::from_str(t).unwrap();
        assert_eq!(d["items"].as_array().unwrap().len(), 3);
        assert!(d["items"][1]["colour"].as_str().unwrap().starts_with('#'));
        assert!(s.edit(&[json!({"op": "shape", "items": [{"type": "squiggle"}]})], "bad", false).unwrap_err().contains("squiggle"));
        assert!(s.edit(&[json!({"op": "shape", "preset": "nope"})], "bad", false).unwrap_err().contains("ring"));
        s.edit(&[json!({"op": "shape", "preset": "ring", "colour": "#22cc88", "size": 200})], "ring", false).unwrap();
        let ring = s.model_clip(newest(&s)).unwrap();
        let m::ClipSource::Generator(g) = &ring.source else { panic!() };
        assert!(g.params.contains_key("progress"));
    }

    #[test]
    fn a_title_takes_a_font_weight_and_italic_and_runs_their_own() {
        let mut s = session();
        // Manrope is bundled with the app, so the family exists on every machine.
        let r = s.edit(&[json!({"op": "text", "text": "Grand Opening", "at": 0, "duration": 2, "font": "Manrope", "weight": 500})], "t", false).unwrap();
        assert!(r.to_string().contains("Manrope"), "the reply names the face: {r}");
        let id = newest(&s);
        let mc = s.model_clip(id).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!("a title") };
        assert_eq!(g.params.get("font"), Some(&m::Value::Text("Manrope".into())));
        assert_eq!(g.params.get("font_weight"), Some(&m::Value::Scalar(500.0)));
        // A family that is not there is refused, not drawn in a stand-in.
        let e = s.edit(&[json!({"op": "set", "clip": id, "font": "Manrop"})], "typo", false).unwrap_err();
        assert!(e.contains("Manrope") && e.contains("fonts"), "{e}");
        let e = s.edit(&[json!({"op": "set", "clip": id, "runs": [{"text": "Grand ", "font": "Not A Real Font 77"}, {"text": "Opening"}]})], "run", false).unwrap_err();
        assert!(e.contains("no font"), "{e}");
        // Named weights, a run in its own font, and back to the style's font.
        s.edit(&[json!({"op": "set", "clip": id, "weight": "bold", "italic": true, "runs": [{"text": "Grand ", "font": "Manrope", "weight": 800}, {"text": "Opening"}]})], "ok", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!() };
        assert_eq!(g.params.get("font_weight"), Some(&m::Value::Scalar(700.0)));
        assert_eq!(g.params.get("font_italic"), Some(&m::Value::Bool(true)));
        assert_eq!(g.text.len(), 2);
        s.edit(&[json!({"op": "set", "clip": id, "font": null})], "reset", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!() };
        assert!(!g.params.contains_key("font"));
        // The chosen face is what draws: two weights of the same family measure differently.
        let engine = crate::text_look::engine();
        let width = |w: f32| {
            let mut ts = crate::text_look::style_of("", 120.0, 0.0, [1.0; 4], true);
            crate::text_look::set_font(&mut ts, "Manrope", w, None);
            engine.layout(&studio_text::TextBlock::plain("Grand Opening"), &ts, &studio_text::style::Canvas::new(1280, 720)).bounds.w
        };
        assert!((width(500.0) - width(700.0)).abs() > 1.0, "{} vs {}", width(500.0), width(700.0));
    }

    #[test]
    fn lottie_shape_layers_come_in_as_shape_layers() {
        let mut s = session();
        let dir = std::env::temp_dir().join(format!("mograph-lottie-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dot.json");
        let lot = json!({"v": "5.7.0", "fr": 30, "ip": 0, "op": 60, "w": 640, "h": 360, "layers": [
            {"ty": 4, "nm": "Dot", "ip": 0, "op": 60, "ks": {"p": {"a": 0, "k": [320, 180, 0]}},
             "shapes": [{"ty": "gr", "it": [{"ty": "el", "p": {"a": 0, "k": [0, 0]}, "s": {"a": 1, "k": [{"t": 0, "s": [10, 10]}, {"t": 30, "s": [120, 120]}]}}, {"ty": "fl", "c": {"a": 0, "k": [1, 0.5, 0, 1]}, "o": {"a": 0, "k": 100}}, {"ty": "tr"}]}]},
            {"ty": 1, "nm": "Solid", "ip": 0, "op": 60}]});
        std::fs::write(&file, lot.to_string()).unwrap();
        let r = s.edit(&[json!({"op": "shape", "lottie": file.to_str().unwrap()})], "lottie", false).unwrap();
        assert!(r.to_string().contains("1 shape layer"), "{r}");
        assert!(r.to_string().contains("Solid"), "{r}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn solids_from_text_and_shapes() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "shape", "items": [{"type": "star", "points": 5, "radius": 100}], "extrude": {"depth": 40, "colour": "#e8c060"}})], "star", false).unwrap();
        assert!(r.to_string().contains("3D model clip"), "{r}");
        let mc = s.model_clip(newest(&s)).unwrap();
        assert!(crate::mesh_store::file_of(&mc).is_some());
    }

    #[test]
    fn expressions_and_time_remaps() {
        let mut s = session();
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 100, "height": 100})], "card", false).unwrap();
        let lead = newest(&s);
        s.edit(&[json!({"op": "card", "colour": "#3366ff", "at": 0, "duration": 4, "width": 100, "height": 100})], "card", false).unwrap();
        let follow = newest(&s);
        s.edit(&[json!({"op": "set", "clip": lead, "rotation": 30})], "turn", false).unwrap();
        let name = s.model_clip(lead).unwrap().name;
        let r = s.edit(&[json!({"op": "expression", "clip": follow, "property": "rotation", "expr": format!("prop(\"{name}\", \"rotation\") * 2")})], "link", false).unwrap();
        // By id too.
        s.edit(&[json!({"op": "expression", "clip": follow, "property": "opacity", "expr": format!("prop({lead}, \"opacity\") * 0.5")})], "link", false).unwrap();
        assert!(r.to_string().contains("driven"), "{r}");
        let seq = s.doc.model.active();
        let c = seq.clip(m::ClipId(follow as u64)).unwrap();
        assert_eq!(seq.driven(c, 0).property_at("transform.rotation", 0).unwrap().as_f64(), Some(60.0));
        let r = s.edit(&[json!({"op": "expression", "clip": follow, "property": "x", "expr": "wiggle(2, 30)"})], "wiggle", false).unwrap();
        assert!(!r.to_string().contains("driven"), "{r}");
        assert!(s.edit(&[json!({"op": "expression", "clip": follow, "property": "x", "expr": "wiggle(2"})], "bad", false).unwrap_err().contains("expression"));
        assert!(s.edit(&[json!({"op": "time_remap", "clip": follow})], "remap", false).unwrap_err().contains("generated"));
    }
}
