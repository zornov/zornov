// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `mesh` with hand-modelling steps: every operation a person has in the
//! 3D workspace's edit mode (`studio_modeling::tools`), on the model's
//! polygon source, with what to work on said by `select`. The pick carries
//! from step to step; modifier and material steps change the stack and
//! the slots. The whole op is one undo step, and the model stays editable
//! by hand afterwards (same `.smodel` source the person's edit mode uses).
//!
//! A step is `{"do": "<operation>", "select": {...}, <settings>}`.
//! `select` (all keys optional; `add: true` keeps the earlier pick):
//! `mode` (vertex, edge, face), `faces` (a word: top, bottom, left, right,
//! front, back, all; a list of numbers; `{"facing": [x, y, z], "degrees"}`,
//! `{"box": {"min", "max"}}`, `{"side": {"axis", "above"|"below"}}`,
//! `{"material": n}`, `{"nearest": [x, y, z]}`, `{"grow": n}` with
//! another key), `edges` (`sharp`, `boundary`, `seams`, `creased`, `all`;
//! `[[a, b], …]`; `{"sharper": deg}`, `{"loop": [x, y, z]}`, `{"ring": [x,
//! y, z]}`, `{"nearest": [x, y, z]}`, `{"outline": <faces>}`), `verts`
//! (`all`, a list, `{"near": [x, y, z], "radius": r}`, `{"box": …}`).

use super::*;
use studio_modeling::asset::Asset;
use studio_modeling::modifiers::Modifier;
use studio_modeling::poly::Poly;
use studio_modeling::select::{Edges, Faces};
use studio_modeling::selection::{Combine, Mode, Selection};
use studio_modeling::tools::{self, Ctx};
use studio_mesh::V3;

/// Steps of this kind (besides the operations of the table). `material`
/// is also a triangle step: it is hand modelling with a `select` or the
/// op's `"edit": true`.
pub const EXTRA: &[&str] = &["select", "modifier", "describe", "operations", "part"];

/// Whether a `mesh` op is hand modelling (else the triangle steps).
pub fn is_edit(op: &Value, steps: &[Value]) -> bool {
    if op.get("edit").and_then(Value::as_bool) == Some(true) || op.get("select").is_some() {
        return true;
    }
    steps.iter().any(|s| {
        let d = s.get("do").and_then(Value::as_str).unwrap_or("");
        s.get("select").is_some() || EXTRA.contains(&d) || (tools::find(d).is_some() && !super::models::STEPS.contains(&d))
    })
}

fn v3(v: &Value) -> Option<V3> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32])
}

fn axis_of(v: Option<&Value>) -> usize {
    match v {
        Some(Value::String(s)) => match s.as_str() {
            "y" => 1,
            "z" => 2,
            _ => 0,
        },
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0).min(2) as usize,
        _ => 0,
    }
}

/// Faces in words, numbers or shapes.
fn faces_spec(v: &Value) -> Result<Faces, String> {
    Ok(match v {
        Value::String(s) => Faces::named(s).ok_or_else(|| format!("faces \"{s}\": top, bottom, left, right, front, back, all, none, a list of face numbers, or an object (facing, box, side, material, nearest)"))?,
        Value::Array(a) => Faces::List(a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect()),
        Value::Object(o) => {
            let base = if let Some(d) = o.get("facing") {
                Faces::Facing { direction: v3(d).ok_or("facing is [x, y, z]")?, degrees: o.get("degrees").and_then(Value::as_f64).unwrap_or(45.0) as f32 }
            } else if let Some(b) = o.get("box") {
                Faces::Box { min: b.get("min").and_then(v3).ok_or("box needs min [x, y, z]")?, max: b.get("max").and_then(v3).ok_or("box needs max [x, y, z]")? }
            } else if let Some(s) = o.get("side") {
                let (above, at) = match (s.get("above").and_then(Value::as_f64), s.get("below").and_then(Value::as_f64)) {
                    (Some(a), _) => (true, a),
                    (_, Some(b)) => (false, b),
                    _ => return Err("side needs \"above\" or \"below\" (a coordinate)".into()),
                };
                Faces::Side { axis: axis_of(s.get("axis")), at: at as f32, above }
            } else if let Some(m) = o.get("material").and_then(Value::as_u64) {
                Faces::Material(m as u32)
            } else if let Some(p) = o.get("nearest").and_then(v3) {
                Faces::Nearest(p)
            } else if let Some(c) = o.get("sphere") {
                Faces::Sphere { centre: c.get("at").and_then(v3).ok_or("sphere needs at")?, radius: c.get("radius").and_then(Value::as_f64).unwrap_or(10.0) as f32 }
            } else {
                Faces::All
            };
            match o.get("grow").and_then(Value::as_i64) {
                Some(n) => Faces::Grow(Box::new(base), n as i32),
                None => base,
            }
        }
        _ => return Err("faces is a word, a list or an object".into()),
    })
}

fn edges_spec(v: &Value) -> Result<Edges, String> {
    Ok(match v {
        Value::String(s) => match s.as_str() {
            "sharp" => Edges::Sharper { degrees: 30.0 },
            "marked_sharp" => Edges::MarkedSharp,
            "boundary" | "open" | "rim" => Edges::Boundary,
            "seams" => Edges::Seams,
            "creased" => Edges::Creased,
            "all" => Edges::All,
            _ => return Err(format!("edges \"{s}\": sharp, boundary, seams, creased, all, or a list / object")),
        },
        Value::Array(a) => Edges::List(a.iter().filter_map(|e| e.as_array().filter(|e| e.len() == 2).and_then(|e| Some((e[0].as_u64()? as u32, e[1].as_u64()? as u32)))).collect()),
        Value::Object(o) => {
            if let Some(d) = o.get("sharper").and_then(Value::as_f64) {
                Edges::Sharper { degrees: d as f32 }
            } else if let Some(p) = o.get("loop").and_then(v3) {
                Edges::Loop(p)
            } else if let Some(p) = o.get("ring").and_then(v3) {
                Edges::Ring(p)
            } else if let Some(p) = o.get("nearest").and_then(v3) {
                Edges::Nearest(p)
            } else if let Some(f) = o.get("outline") {
                Edges::Outline(faces_spec(f)?)
            } else if let Some(f) = o.get("inside") {
                Edges::Inside(faces_spec(f)?)
            } else {
                return Err("edges object: sharper, loop, ring, nearest, outline or inside".into());
            }
        }
        _ => return Err("edges is a word, a list of [a, b] or an object".into()),
    })
}

/// Applies a `select` spec to the pick.
pub fn select(p: &Poly, s: &mut Selection, spec: &Value) -> Result<(), String> {
    if let Some(m) = spec.get("mode").and_then(Value::as_str) {
        let mode = Mode::parse(m).ok_or_else(|| format!("mode \"{m}\": vertex, edge or face"))?;
        s.set_mode(p, mode);
    }
    let add = spec.get("add").and_then(Value::as_bool).unwrap_or(false);
    let before = s.clone();
    let mut any = false;
    if let Some(f) = spec.get("faces") {
        let list = faces_spec(f)?.list(p);
        if spec.get("mode").is_none() {
            s.mode = Mode::Face;
        }
        let mode = s.mode;
        s.mode = Mode::Face;
        s.with_faces(p, &list);
        s.mode = mode;
        s.sync(p);
        any = true;
    }
    if let Some(e) = spec.get("edges") {
        let list = edges_spec(e)?.list(p);
        if spec.get("mode").is_none() {
            s.mode = Mode::Edge;
        }
        let mode = s.mode;
        s.mode = Mode::Edge;
        s.with_edges(p, &list);
        s.mode = mode;
        s.sync(p);
        any = true;
    }
    if let Some(v) = spec.get("verts").or_else(|| spec.get("vertices")) {
        let list: Vec<u32> = match v {
            Value::String(w) if w == "all" => (0..p.positions.len() as u32).collect(),
            Value::Array(a) => a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect(),
            Value::Object(o) if o.get("near").is_some() => {
                let c = o.get("near").and_then(v3).ok_or("near is [x, y, z]")?;
                let r = o.get("radius").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                (0..p.positions.len() as u32).filter(|&v| studio_mesh::length(studio_mesh::sub(p.positions[v as usize], c)) <= r).collect()
            }
            Value::Object(o) if o.get("box").is_some() => {
                let b = &o["box"];
                let (lo, hi) = (b.get("min").and_then(v3).ok_or("box needs min")?, b.get("max").and_then(v3).ok_or("box needs max")?);
                (0..p.positions.len() as u32).filter(|&v| (0..3).all(|k| p.positions[v as usize][k] >= lo[k].min(hi[k]) && p.positions[v as usize][k] <= hi[k].max(lo[k]))).collect()
            }
            _ => return Err("verts is \"all\", a list of numbers, {near, radius} or {box}".into()),
        };
        if spec.get("mode").is_none() {
            s.mode = Mode::Vertex;
        }
        let mode = s.mode;
        s.mode = Mode::Vertex;
        s.with_vertices(p, &list);
        s.mode = mode;
        s.sync(p);
        any = true;
    }
    if spec.get("all").and_then(Value::as_bool) == Some(true) {
        s.all(p);
        any = true;
    }
    if add && any {
        // The earlier pick joins the new one.
        let mut joined = before;
        joined.mode = s.mode;
        for v in s.vert_list() {
            joined.pick(p, studio_modeling::selection::Elem::Vertex(v), Combine::Add);
        }
        for f in s.face_list() {
            joined.pick(p, studio_modeling::selection::Elem::Face(f), Combine::Add);
        }
        for e in s.edge_list() {
            joined.pick(p, studio_modeling::selection::Elem::Edge(e), Combine::Add);
        }
        *s = joined;
    }
    Ok(())
}

fn said_pick(s: &Selection) -> String {
    let (v, e, f) = s.count();
    format!("picked {v} vertices, {e} edges, {f} faces")
}

/// What the AI gets back about the model after the steps.
fn describe_poly(a: &Asset, part: usize, s: &Selection) -> Value {
    let p = &a.parts[part];
    let (lo, hi) = p.mesh.bounds();
    let r = |x: f32| (x * 10.0).round() / 10.0;
    let t = studio_modeling::poly::Topology::of(&p.mesh);
    let open = t.edge_faces.values().filter(|f| f.len() == 1).count();
    let many = t.edge_faces.values().filter(|f| f.len() > 2).count();
    json!({
        "part": p.name,
        "parts": a.parts.iter().map(|q| q.name.clone()).collect::<Vec<_>>(),
        "cage": p.mesh.stats(),
        "box_min": [r(lo[0]), r(lo[1]), r(lo[2])],
        "box_max": [r(hi[0]), r(hi[1]), r(hi[2])],
        "open_edges": open,
        "edges_with_more_than_two_faces": many,
        "picked": {"mode": s.mode.name(), "vertices": s.count().0, "edges": s.count().1, "faces": s.count().2},
        "modifiers": p.modifiers.iter().enumerate().map(|(i, m)| json!({"index": i, "kind": m.name(), "on": !p.disabled.get(i).copied().unwrap_or(false)})).collect::<Vec<_>>(),
        "material_slots": (0..p.mesh.materials.len().max(p.materials.len())).map(|i| p.materials.get(i).cloned().unwrap_or_else(|| p.mesh.materials.get(i).map(|m| m.name.clone()).unwrap_or_default())).collect::<Vec<_>>(),
        "uvs": p.mesh.has_uvs(),
    })
}

/// Every operation with its settings, for the AI.
pub fn operations() -> Value {
    Value::Array(
        tools::TOOLS
            .iter()
            .map(|t| {
                json!({
                    "do": t.id,
                    "menu": t.menu.label(),
                    "does": t.tip,
                    "settings": t.params.iter().map(|p| {
                        let kind = match p.kind {
                            tools::Kind::Length { .. } => "px".to_string(),
                            tools::Kind::Number { min, max, .. } => format!("{min}..{max}"),
                            tools::Kind::Angle { .. } => "degrees".to_string(),
                            tools::Kind::Percent { .. } => "percent".to_string(),
                            tools::Kind::Count { min, max, .. } => format!("whole {min}..{max}"),
                            tools::Kind::Toggle { .. } => "true/false".to_string(),
                            tools::Kind::Choice { options, .. } => options.join("|"),
                            tools::Kind::Vector { .. } => "[x, y, z]".to_string(),
                            tools::Kind::Axis { .. } => "x|y|z".to_string(),
                        };
                        json!({"name": p.key, "is": kind})
                    }).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

impl Session {
    /// `mesh` with hand-modelling steps (see the module).
    pub(super) fn mesh_edit(&mut self, op: &Value, steps: &[Value]) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("mesh needs \"clip\" (a model clip)")? as u32;
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if crate::mesh_store::file_of(&mc).is_none() {
            return Err(format!("clip {id} is not a 3D model (add one with the model or asset op)"));
        }
        let (mut asset, mesh) = crate::mesh_store::load_asset(&mc)?;
        let mut part = match op.get("part").and_then(Value::as_str) {
            Some(n) => asset.parts.iter().position(|p| p.name == n).ok_or_else(|| format!("no part \"{n}\"; parts: {}", asset.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")))?,
            None => 0,
        };
        let mut sel = Selection::new(&asset.parts[part].mesh, Mode::Face);
        if let Some(spec) = op.get("select") {
            select(&asset.parts[part].mesh, &mut sel, spec)?;
        }
        let mut said = Vec::new();
        let mut changed = false;
        let mut pieces: Vec<Poly> = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            let what = step.get("do").and_then(Value::as_str).ok_or_else(|| format!("step {}: needs \"do\" (an operation; \"operations\" lists them)", i + 1))?;
            let fail = |e: String| format!("step {} ({what}): {e}. Nothing was changed.", i + 1);
            if let Some(spec) = step.get("select") {
                select(&asset.parts[part].mesh, &mut sel, spec).map_err(fail)?;
            }
            match what {
                "select" => {
                    select(&asset.parts[part].mesh, &mut sel, step).map_err(fail)?;
                    said.push(said_pick(&sel));
                }
                "describe" => said.push(describe_poly(&asset, part, &sel).to_string()),
                "operations" => said.push(operations().to_string()),
                "part" => {
                    let n = step.get("name").and_then(Value::as_str).ok_or_else(|| fail("needs \"name\"".into()))?;
                    part = asset.parts.iter().position(|p| p.name == n).ok_or_else(|| fail(format!("no part \"{n}\"")))?;
                    sel = Selection::new(&asset.parts[part].mesh, sel.mode);
                    said.push(format!("working on part {n}"));
                }
                "modifier" => {
                    said.push(modifier_step(&mut asset, part, step).map_err(fail)?);
                    changed = true;
                }
                "material" => {
                    said.push(material_step(&mut asset, part, &mut sel, step).map_err(fail)?);
                    changed = true;
                }
                _ => {
                    let t = tools::find(what).ok_or_else(|| fail(format!("no operation \"{what}\"; send {{\"do\": \"operations\"}} for the list")))?;
                    let mut args = step.as_object().cloned().unwrap_or_default();
                    args.remove("do");
                    args.remove("select");
                    let p = &mut asset.parts[part].mesh;
                    let mut ctx = Ctx::for_poly(p);
                    if let Some(c) = step.get("cursor").and_then(v3) {
                        ctx.cursor = c;
                    }
                    args.remove("cursor");
                    if let Some(c) = step.get("view").and_then(v3) {
                        ctx.view = c;
                    }
                    args.remove("view");
                    let done = tools::run(p, &mut sel, t.id, &args, &ctx).map_err(fail)?;
                    changed |= done.changed;
                    pieces.extend(done.pieces);
                    said.push(done.said);
                }
            }
            if asset.parts[part].mesh.faces() > 2_000_000 {
                return Err(format!("step {} ({what}) made {} faces; that's more than a model should hold. Nothing was changed.", i + 1, asset.parts[part].mesh.faces()));
            }
        }
        if !changed && pieces.is_empty() {
            return Ok(format!("{}; {}", said.join("; "), describe_poly(&asset, part, &sel)));
        }
        if asset.parts.iter().all(|p| p.mesh.faces() == 0 || p.hidden) {
            return Err("that would leave the model with nothing to draw. Nothing was changed.".into());
        }
        // What was done, for whoever opens it next.
        let label: Vec<&str> = steps.iter().filter_map(|s| s.get("do").and_then(Value::as_str)).collect();
        asset.parts[part].history.push(format!("AI: {}", label.join(", ")));
        let mut out = asset.to_mesh(false)?;
        if let Some(rig) = &mesh.rig {
            out.rig = Some(std::sync::Arc::new(rig.transferred(&mesh, &out)));
        }
        let file = crate::mesh_store::save(&self.doc, &out)?;
        let source = crate::mesh_store::save_source(&self.doc, &asset)?;
        crate::mesh_store::set_file(&mut mc, file);
        crate::mesh_store::set_source(&mut mc, Some(source));
        let seq = self.doc.seq();
        let mut edits = vec![m::Edit::SetClip { seq, clip: Box::new(mc.clone()) }];
        // Separated pieces become model clips of their own on new lanes.
        let mut made = Vec::new();
        for (k, piece) in pieces.into_iter().enumerate() {
            let mut a = Asset::new(&format!("{} {}", mc.name, k + 2));
            let mut pp = asset.parts[part].clone();
            pp.mesh = piece;
            pp.name = "Body".into();
            a.parts.push(pp);
            a.materials = asset.materials.clone();
            let m2 = a.to_mesh(true)?;
            let (f2, s2) = (crate::mesh_store::save(&self.doc, &m2)?, crate::mesh_store::save_source(&self.doc, &a)?);
            let lane = self.doc.alloc_id();
            let cid = self.doc.alloc_id();
            let mut c = mc.clone();
            c.id = m::ClipId(cid);
            c.lane = m::LaneId(lane);
            c.name = a.name.clone();
            crate::mesh_store::set_file(&mut c, f2);
            crate::mesh_store::set_source(&mut c, Some(s2));
            edits.push(m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, &c.name), index: Some(0) });
            edits.push(m::Edit::InsertClip { seq, clip: Box::new(c) });
            made.push(cid);
        }
        let ok = self.doc.apply("Model by hand", m::Edit::Batch { edits });
        self.check(ok, "")?;
        let mut tail = describe_poly(&asset, part, &sel).to_string();
        if !made.is_empty() {
            tail.push_str(&format!("; separated into new model clips {}", made.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")));
        }
        Ok(format!("{}; now {}", said.join("; "), tail))
    }
}

fn modifier_step(a: &mut Asset, part: usize, s: &Value) -> Result<String, String> {
    let p = &mut a.parts[part];
    p.disabled.resize(p.modifiers.len(), false);
    if let Some(add) = s.get("add") {
        let m: Modifier = serde_json::from_value(add.clone()).map_err(|e| format!("\"add\" is a modifier {{\"kind\": …}} with its fields ({e}); kinds: mirror, array, radial, solidify, bevel, subdivide, boolean, smooth, weld, triangulate, decimate, displace, bend, twist, taper, wave, cast, lattice, shrinkwrap, weighted_normals, remesh"))?;
        let name = m.name();
        p.modifiers.push(m);
        p.disabled.push(false);
        return Ok(format!("{name} modifier added ({} in the stack)", p.modifiers.len()));
    }
    let index = |k: &str| s.get(k).and_then(Value::as_u64).map(|i| i as usize);
    if let Some(i) = index("remove") {
        if i >= p.modifiers.len() {
            return Err(format!("no modifier {i} (the stack has {})", p.modifiers.len()));
        }
        p.modifiers.remove(i);
        p.disabled.remove(i);
        return Ok(format!("modifier {i} removed"));
    }
    if let Some(i) = index("enable") {
        if i >= p.modifiers.len() {
            return Err(format!("no modifier {i}"));
        }
        let on = s.get("on").and_then(Value::as_bool).unwrap_or(true);
        p.disabled[i] = !on;
        return Ok(format!("modifier {i} {}", if on { "on" } else { "off" }));
    }
    if let (Some(i), Some(j)) = (index("move"), index("to")) {
        if i >= p.modifiers.len() || j >= p.modifiers.len() {
            return Err("move and to are places in the stack".into());
        }
        let m = p.modifiers.remove(i);
        let d = p.disabled.remove(i);
        p.modifiers.insert(j, m);
        p.disabled.insert(j, d);
        return Ok(format!("modifier moved from {i} to {j}"));
    }
    if let Some(i) = index("apply") {
        crate::mesh_store::apply_modifier(a, part, i)?;
        return Ok(format!("modifier {i} applied to the mesh"));
    }
    if let (Some(i), Some(set)) = (index("index"), s.get("set")) {
        let mut v = serde_json::to_value(&p.modifiers.get(i).ok_or_else(|| format!("no modifier {i}"))?).map_err(|e| e.to_string())?;
        if let (Some(o), Some(n)) = (v.as_object_mut(), set.as_object()) {
            for (k, x) in n {
                o.insert(k.clone(), x.clone());
            }
        }
        p.modifiers[i] = serde_json::from_value(v).map_err(|e| format!("those settings don't fit that modifier: {e}"))?;
        return Ok(format!("modifier {i} changed"));
    }
    Err("modifier step: \"add\" {kind…}, \"remove\" n, \"enable\" n with \"on\", \"move\" n \"to\" m, \"apply\" n, or \"index\" n with \"set\" {fields}".into())
}

fn material_step(a: &mut Asset, part: usize, sel: &mut Selection, s: &Value) -> Result<String, String> {
    let slot = s.get("slot").and_then(Value::as_u64).unwrap_or(0) as usize;
    let size = {
        let (lo, hi) = a.parts[part].mesh.bounds();
        studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0)
    };
    let mut def = match s.get("smart").and_then(Value::as_str) {
        Some(n) => studio_modeling::material::smart(n, size * 0.05, None).ok_or_else(|| format!("smart material \"{n}\": {}", studio_modeling::material::SMART.join(", ")))?,
        None => studio_modeling::material::smart("plastic", size * 0.05, None).ok_or("no plastic")?,
    };
    if s.get("smart").is_none() {
        def.layers.clear();
        def.base.pattern = None;
        def.micro = None;
    }
    if let Some(c) = s.get("colour") {
        let l = super::models::linear_colour(c)?;
        def.base.colour = [l[0], l[1], l[2]];
    }
    if let Some(x) = s.get("metallic").and_then(Value::as_f64) {
        def.base.metallic = x as f32;
    }
    if let Some(x) = s.get("roughness").and_then(Value::as_f64) {
        def.base.roughness = x as f32;
    }
    if let Some(x) = s.get("transmission").and_then(Value::as_f64) {
        def.lobes.transmission = x as f32;
    }
    if let Some(x) = s.get("subsurface").and_then(Value::as_f64) {
        def.lobes.subsurface = x as f32;
    }
    if let Some(c) = s.get("emissive") {
        let l = super::models::linear_colour(c)?;
        def.lobes.emissive = Some([l[0], l[1], l[2]]);
    }
    def.name = s.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{} {}", a.parts[part].name, slot + 1));
    let mut name = def.name.clone();
    let mut n = 2;
    while a.materials.iter().any(|m| m.name == name) && a.parts[part].materials.get(slot) != Some(&name) {
        name = format!("{} {n}", def.name);
        n += 1;
    }
    def.name = name.clone();
    match a.materials.iter_mut().find(|m| m.name == name) {
        Some(m) => *m = def,
        None => a.materials.push(def),
    }
    let p = &mut a.parts[part];
    if p.materials.len() <= slot {
        p.materials.resize(slot + 1, String::new());
    }
    p.materials[slot] = name.clone();
    // On the picked faces too, when there are any.
    let mut n_faces = 0;
    if s.get("assign").and_then(Value::as_bool).unwrap_or(true) {
        for f in 0..p.mesh.faces() {
            if sel.faces.get(f).copied().unwrap_or(false) {
                p.mesh.face_material[f] = slot as u32;
                n_faces += 1;
            }
        }
    }
    p.mesh.ensure_materials();
    let _ = sel;
    Ok(format!("material \"{name}\" in slot {slot}{}", if n_faces > 0 { format!(", on {n_faces} picked faces") } else { String::new() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_cube() -> (Session, u32) {
        let mut s = Session::new(crate::services::project::new_custom("Model", 1280, 720, 30.0));
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 200})], "cube", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s, id)
    }

    fn poly_of(s: &Session, id: u32) -> Poly {
        let mc = s.model_clip(id).unwrap();
        crate::mesh_store::load_asset(&mc).unwrap().0.parts[0].mesh.clone()
    }

    #[test]
    fn the_ai_models_by_hand_with_the_same_operations() {
        let (mut s, id) = session_with_cube();
        // A box from triangles: joined back into its six quads first.
        let p0 = poly_of(&s, id);
        assert_eq!(p0.faces(), 6);
        let r = s.edit(&[json!({"op": "mesh", "clip": id, "steps": [
            {"do": "inset", "select": {"faces": "top"}, "thickness": 20},
            {"do": "extrude_region", "distance": -150},
            {"do": "bevel", "select": {"edges": "sharp"}, "width": 3, "segments": 2},
            {"do": "modifier", "add": {"kind": "subdivide", "levels": 1}},
            {"do": "material", "select": {"faces": "bottom"}, "slot": 1, "colour": "#cc3322", "roughness": 0.3},
            {"do": "describe"}
        ]})], "a cup", false).unwrap().to_string();
        assert!(r.contains("now"), "{r}");
        let p = poly_of(&s, id);
        let t = studio_modeling::poly::Topology::of(&p);
        assert!(t.edge_faces.values().all(|f| f.len() == 2), "closed");
        assert!(p.faces() > 20);
        let mc = s.model_clip(id).unwrap();
        let (a, mesh) = crate::mesh_store::load_asset(&mc).unwrap();
        assert_eq!(a.parts[0].modifiers.len(), 1);
        assert!(mesh.triangles() > p.faces() * 4, "drawn subdivided");
        assert!(a.parts[0].materials.len() >= 2);
        // One undo step for the whole op.
        s.doc.undo_where(&|_| true).unwrap();
        let back = poly_of(&s, id);
        assert!(back.faces() <= 6 || back == p0, "undone: {}", back.faces());
    }

    #[test]
    fn picks_by_words_loops_and_points_and_errors_explain() {
        let (mut s, id) = session_with_cube();
        let r = s.edit(&[json!({"op": "mesh", "clip": id, "edit": true, "steps": [
            {"do": "subdivide", "select": {"edges": "all"}, "cuts": 1},
            {"do": "select", "edges": {"loop": [0, 0, -100]}},
            {"do": "describe"}
        ]})], "loop", false).unwrap().to_string();
        assert!(r.contains("\\\"edges\\\":8") || r.contains("\"edges\":8"), "the loop round the middle: {r}");
        let e = s.edit(&[json!({"op": "mesh", "clip": id, "steps": [{"do": "inset", "select": {"faces": "nowhere"}}]})], "bad", false).unwrap_err();
        assert!(e.contains("faces \"nowhere\""), "{e}");
        let e = s.edit(&[json!({"op": "mesh", "clip": id, "steps": [{"do": "bevel_vertices", "select": {"verts": [0]}, "widht": 3}]})], "bad", false).unwrap_err();
        assert!(e.contains("no setting \"widht\""), "{e}");
        let ops = s.edit(&[json!({"op": "mesh", "clip": id, "steps": [{"do": "operations"}]})], "list", false).unwrap().to_string();
        for t in tools::TOOLS {
            assert!(ops.contains(&format!("\"do\":\"{}\"", t.id)) || ops.contains(&format!("\\\"do\\\":\\\"{}\\\"", t.id)), "{} is listed", t.id);
        }
    }

    #[test]
    fn separate_by_the_ai_makes_a_new_model_clip() {
        let (mut s, id) = session_with_cube();
        let before = s.doc.project.clips.len();
        s.edit(&[json!({"op": "mesh", "clip": id, "steps": [{"do": "separate", "select": {"faces": "top"}}]})], "split", false).unwrap();
        assert_eq!(s.doc.project.clips.len(), before + 1);
        assert_eq!(poly_of(&s, id).faces(), 5);
    }
}
