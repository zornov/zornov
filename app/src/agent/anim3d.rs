// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D character animation for the agent:
//! - `rig`: an armature for a model (humanoid, quadruped, chain, or bones
//!   given one by one), skin weights bound automatically, face shape keys,
//!   constraints (IK, look-at, limits, copied rotations); or the rig in
//!   words.
//! - `pose`: bone turns, shape-key weights, IK reaches and where the head
//!   looks, keyed at times in the clip (pose to pose, Studio fills in
//!   between).
//! - `act`: motion: a procedural walk, run, idle, wave, jump, nod, head
//!   shake, talk, dance or sneak; an action the model brought with it
//!   (glTF); motion capture (BVH or a rigged glTF/GLB) retargeted onto the
//!   rig; a blend from one motion into another; walking that carries the
//!   layer so the feet do not slide.
//!
//! Rigs live in the model file, so each change writes a new file (undo
//! goes back to the one before); poses and motion are ordinary keyframes
//! and settings on the model clip (`generator.bone.<bone>.x`,
//! `generator.shape.<key>`, `generator.action`…), so they show and edit
//! in the timeline and graph editor like any other animation.

use super::*;
use std::sync::Arc;
use studio_mesh::rig::{Bone, Constraint, Rig, Target};
use studio_mesh::{Mesh, V3};

pub(super) fn v3(v: &Value) -> Option<V3> {
    let a = v.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

/// Degrees from `[x, y, z]` or `{"x":…, "y":…, "z":…}`.
pub(super) fn degrees(v: &Value) -> Option<V3> {
    if let Some(d) = v3(v) {
        return Some(d);
    }
    let o = v.as_object()?;
    let g = |k: &str| o.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    Some([g("x"), g("y"), g("z")])
}

/// The rig of a model in words.
pub(super) fn describe_rig(r: &Rig) -> Value {
    json!({
        "bones": r.bones.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
        "skinned": r.is_skinned(),
        "shape_keys": r.shapes.iter().map(|k| k.name.clone()).collect::<Vec<_>>(),
        "constraints": r.constraints.iter().map(|c| json!({"name": c.name(), "kind": c.kind(), "bone": r.bones.get(c.bone()).map(|b| b.name.clone())})).collect::<Vec<_>>(),
        "actions": r.actions.iter().map(|a| json!({"name": a.name, "seconds": (a.duration * 100.0).round() / 100.0})).collect::<Vec<_>>(),
    })
}

/// A shape key name from what an AI may say ("smile" → both smile keys).
fn shape_names(r: &Rig, name: &str) -> Vec<String> {
    if r.shape(name).is_some() {
        return vec![name.to_string()];
    }
    let a = studio_mesh::face::alias(name);
    if r.shape(a).is_some() {
        return vec![a.to_string()];
    }
    // A pair: "mouthSmile" → Left and Right.
    let pair: Vec<String> = ["Left", "Right"].iter().map(|s| format!("{a}{s}")).filter(|n| r.shape(n).is_some()).collect();
    if !pair.is_empty() {
        return pair;
    }
    Vec::new()
}

pub(super) fn interp(ease: &str) -> Result<m::Interp, String> {
    Ok(match ease {
        "smooth" | "auto" => m::Interp::Auto,
        "linear" => m::Interp::Linear,
        "hold" | "step" | "snap" => m::Interp::Hold,
        "ease" | "ease_in_out" => m::Interp::Ease,
        other => return Err(format!("ease \"{other}\": smooth, linear, ease or hold")),
    })
}

/// Sets a key on a clip's track (made if missing). A model setting that
/// is keyed gets a resting value of the same kind first (zero), as keyed
/// settings need one.
pub(super) fn key(c: &mut m::Clip, path: &str, t: i64, v: m::Value, how: m::Interp) {
    if let Some(name) = path.strip_prefix("generator.")
        && let m::ClipSource::Generator(g) = &mut c.source
        && !g.params.get(name).is_some_and(|p| p.same_kind(&v))
    {
        let rest = match &v {
            m::Value::Angle(_) => m::Value::Angle(0.0),
            m::Value::Point(_) => m::Value::Point(m::Vec2::ZERO),
            _ => m::Value::Scalar(0.0),
        };
        g.params.insert(name.to_string(), rest);
    }
    let k = m::Keyframe::new(t, v, how);
    match c.keyframes.get_mut(path) {
        Some(tr) => tr.set_key(k),
        None => {
            c.keyframes.insert(path.to_string(), m::Track::new(vec![k]));
        }
    }
}

pub(super) fn set_param(c: &mut m::Clip, k: &str, v: m::Value) {
    if let m::ClipSource::Generator(g) = &mut c.source {
        g.params.insert(k.to_string(), v);
    }
}

fn param_f(c: &m::Clip, k: &str) -> Option<f64> {
    match &c.source {
        m::ClipSource::Generator(g) => g.params.get(k).and_then(|v| v.as_f64()),
        _ => None,
    }
}

impl Session {
    /// The rig of a model clip, or an error saying how to get one.
    pub(super) fn rig_of(&self, id: u32) -> Result<(m::Clip, Mesh, Arc<Rig>), String> {
        let (mc, mesh) = self.model_of(id)?;
        let rig = mesh.rig.clone().ok_or_else(|| format!("clip {id} has no rig: rig it first ({{\"op\": \"rig\", \"clip\": {id}, \"template\": \"humanoid\"}}), or add a ready character ({{\"op\": \"model\", \"shape\": \"character\"}})"))?;
        Ok((mc, mesh, rig))
    }

    /// Whether an op's clip is a rigged 3D model (so `pose` is the 3D one).
    pub(super) fn is_rigged_model(&self, op: &Value) -> bool {
        let Some(id) = op.get("clip").and_then(Value::as_u64) else { return false };
        self.model_clip(id as u32).and_then(|mc| crate::mesh_store::file_of(&mc)).is_some()
    }

    /// Writes a changed model and points the clip at it (one undo step).
    pub(super) fn save_rigged(&mut self, mut mc: m::Clip, mesh: &Mesh, what: &str) -> Result<(), String> {
        let file = crate::mesh_store::save(&self.doc, mesh)?;
        crate::mesh_store::set_file(&mut mc, file);
        let seq = self.doc.seq();
        let ok = self.doc.apply(what, m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "").map(|_| ())
    }

    /// `rig`: give a model bones (or change or describe them).
    pub(super) fn rig(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("rig needs \"clip\" (a model clip)")? as u32;
        let (mc, mut mesh) = self.model_of(id)?;
        if op.get("list").and_then(Value::as_bool) == Some(true) || (op.as_object().is_some_and(|o| o.len() <= 2)) {
            return Ok(match &mesh.rig {
                Some(r) => format!("clip {id} rig: {}", describe_rig(r)),
                None => format!("clip {id} has no rig; templates: humanoid, quadruped, chain"),
            });
        }
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            mesh.rig = None;
            self.save_rigged(mc, &mesh, "Remove rig")?;
            return Ok(format!("clip {id}: rig removed"));
        }
        let mut said = Vec::new();
        let template = op.get("template").and_then(Value::as_str);
        let rigid = op.get("rigid").and_then(Value::as_bool).unwrap_or(false);
        if let Some(bones) = op.get("bones").and_then(Value::as_array) {
            // Bones one by one: {"name", "parent", "head", "tail"}.
            let mut r = mesh.rig.as_deref().cloned().unwrap_or_default();
            r.joints.clear();
            r.weights.clear();
            if op.get("replace").and_then(Value::as_bool).unwrap_or(template.is_none() && r.bones.is_empty()) {
                r.bones.clear();
                r.constraints.clear();
            }
            for (i, b) in bones.iter().enumerate() {
                let bad = |w: &str| format!("bone {}: {w}", i + 1);
                let name = b.get("name").and_then(Value::as_str).ok_or_else(|| bad("needs \"name\""))?.to_string();
                let head = b.get("head").and_then(v3).ok_or_else(|| bad("needs \"head\": [x, y, z] (model px)"))?;
                let tail = b.get("tail").and_then(v3).ok_or_else(|| bad("needs \"tail\": [x, y, z]"))?;
                let parent = match b.get("parent").and_then(Value::as_str) {
                    Some(p) => Some(r.bone(p).ok_or_else(|| bad(&format!("no bone \"{p}\" to hang from (parents come first)")))?),
                    None => None,
                };
                if r.bone(&name).is_some() {
                    return Err(bad(&format!("there is already a bone \"{name}\"")));
                }
                let mut bone = Bone::new(name, parent, head, tail);
                bone.deform = b.get("deform").and_then(Value::as_bool).unwrap_or(true);
                r.bones.push(bone);
            }
            if rigid {
                let carried = studio_mesh::autorig::bind_rigid(&mesh, &mut r);
                let lines: Vec<String> = r.bones.iter().zip(&carried).filter(|(b, _)| b.deform).map(|(b, n)| format!("{} carries {n} piece{}", b.name, if *n == 1 { "" } else { "s" })).collect();
                said.push(format!("{} bones, each piece bound whole to one bone ({})", r.bones.len(), lines.join(", ")));
            } else {
                studio_mesh::autorig::bind(&mesh, &mut r);
                said.push(format!("{} bones, skin bound", r.bones.len()));
            }
            mesh.rig = Some(Arc::new(r));
        } else if rigid && template.is_none() && mesh.rig.as_ref().is_some_and(|r| !r.bones.is_empty()) {
            // Rebind the bones already there, piece by piece.
            let mut r = mesh.rig.as_deref().cloned().unwrap_or_default();
            let carried = studio_mesh::autorig::bind_rigid(&mesh, &mut r);
            let lines: Vec<String> = r.bones.iter().zip(&carried).filter(|(b, _)| b.deform).map(|(b, n)| format!("{} carries {n} piece{}", b.name, if *n == 1 { "" } else { "s" })).collect();
            said.push(format!("each piece bound whole to one bone ({})", lines.join(", ")));
            mesh.rig = Some(Arc::new(r));
        } else if let Some(t) = template {
            let t = if t == "chain" {
                let count = op.get("count").and_then(Value::as_u64).unwrap_or(6) as usize;
                let start = op.get("start").and_then(Value::as_str).unwrap_or("");
                let name = op.get("name").and_then(Value::as_str).unwrap_or("bone");
                let mut r = studio_mesh::autorig::chain(&mesh, count, start, name);
                if let Some(old) = &mesh.rig {
                    r.shapes = old.shapes.clone();
                }
                studio_mesh::autorig::bind(&mesh, &mut r);
                mesh.rig = Some(Arc::new(r));
                "chain"
            } else {
                studio_mesh::autorig::rig_mesh(&mut mesh, t)?;
                t
            };
            said.push(format!("{t} rig: {} bones, skin bound", mesh.rig.as_ref().map_or(0, |r| r.bones.len())));
        }
        // Face shape keys on a head base (default on, when there is one).
        if op.get("face").and_then(Value::as_bool).unwrap_or(template.is_some()) {
            match studio_mesh::face::find_head(&mesh) {
                Some(frame) => {
                    let keys = studio_mesh::face::head_shapes(&mesh, 0..mesh.positions.len(), frame);
                    let mut r = mesh.rig.as_deref().cloned().unwrap_or_default();
                    r.shapes.retain(|k| !studio_mesh::face::NAMES.contains(&k.name.as_str()));
                    r.shapes.extend(keys);
                    said.push(format!("{} face shape keys", studio_mesh::face::NAMES.len()));
                    mesh.rig = Some(Arc::new(r));
                }
                None if op.get("face").and_then(Value::as_bool) == Some(true) => said.push("no head base found for face keys (sculpt them, or start from shape \"head\" or \"character\")".into()),
                None => {}
            }
        }
        if let Some(c) = op.get("constraint") {
            let mut r = mesh.rig.as_deref().cloned().ok_or("add bones before constraints")?;
            said.push(add_constraint(&mut r, &mesh, c)?);
            mesh.rig = Some(Arc::new(r));
        }
        if let Some(k) = op.get("skinning").and_then(Value::as_str) {
            let mut r = mesh.rig.as_deref().cloned().ok_or("add bones before choosing the skinning")?;
            if !r.is_skinned() && !matches!(k.to_lowercase().as_str(), "linear" | "matrix" | "lbs" | "dual_quaternion" | "dual" | "dq" | "smooth" | "auto") {
                return Err("this rig has no skin yet: bind it first with {\"op\": \"rig\", \"clip\": id, \"skinning\": \"smooth\"}".into());
            }
            match k.to_lowercase().replace([' ', '-'], "_").as_str() {
                "dual_quaternion" | "dual" | "dq" => {
                    r.dual_quat = true;
                    said.push("skinning dual quaternion".into());
                }
                "linear" | "matrix" | "lbs" => {
                    r.dual_quat = false;
                    said.push("skinning linear".into());
                }
                // Hard items: every chosen vertex on one named bone.
                "rigid" | "bone" | "by_bone" if op.get("bone").is_some() => {
                    let bone = r.resolve_bone(op.get("bone").and_then(Value::as_str).unwrap_or("")).map_err(|e| format!("{e}. Example: {{\"op\": \"rig\", \"clip\": 3, \"skinning\": \"rigid\", \"bone\": \"head\", \"material\": \"Hat\"}}"))?;
                    let mask = vertex_mask(&mesh, op)?;
                    let verts: Vec<usize> = (0..mask.len()).filter(|&v| mask[v]).collect();
                    studio_mesh::rigfit::bind_to_bone(&mut r, &verts, bone);
                    said.push(format!("{} vertices rigid on {}", verts.len(), r.bones[bone].name));
                }
                // Each separate piece whole on the bone that carries most of it.
                "rigid" | "parts" | "per_part" | "pieces" | "piece" | "by_part" => {
                    let mask = vertex_mask(&mesh, op)?;
                    let used = studio_mesh::rigfit::bind_pieces(&mesh, &mut r, &mask);
                    said.push(format!("each piece rigid on one bone ({})", used.iter().map(|&b| r.bones[b].name.as_str()).collect::<Vec<_>>().join(", ")));
                }
                // The body's automatic smooth bind, for chosen vertices or all.
                "smooth" | "auto" | "rebind" | "bind" => {
                    let mask = vertex_mask(&mesh, op)?;
                    if mask.iter().all(|&m| m) {
                        studio_mesh::autorig::bind(&mesh, &mut r);
                        said.push("skin bound afresh (smooth)".into());
                    } else {
                        let n = studio_mesh::rigfit::rebind_smooth(&mesh, &mut r, &mask);
                        said.push(format!("{n} vertices skinned afresh (smooth)"));
                    }
                }
                _ => return Err("skinning is \"linear\" or \"dual_quaternion\" (how the weights blend), or what carries the surface: \"smooth\" (the automatic bind; with region/material only those vertices), \"rigid\" with \"bone\" (the chosen vertices whole on that bone: a hat on \"head\"), \"parts\" (each separate piece whole on the bone that carries most of it)".into()),
            }
            mesh.rig = Some(Arc::new(r));
        }
        if said.is_empty() {
            return Err("rig: give \"template\" (humanoid, quadruped, chain), \"bones\", \"face\": true, \"constraint\", \"skinning\", \"remove\": true or \"list\": true".into());
        }
        self.save_rigged(mc, &mesh, "Rig")?;
        Ok(format!("clip {id}: {}; {}", said.join(", "), describe_rig(mesh.rig.as_deref().unwrap_or(&Rig::default()))))
    }

    /// `pose` on a rigged 3D model: key bone turns, shape keys, reaches
    /// and looks (the 2D skeleton's `pose` takes the rest).
    pub(super) fn pose3d(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("pose needs \"clip\" (a rigged model clip)")? as u32;
        let (mut mc, _, rig) = self.rig_of(id)?;
        let keys: Vec<Value> = match op.get("keys") {
            Some(Value::Array(a)) => a.clone(),
            Some(_) => return Err("\"keys\" is a list of {\"t\", \"bones\", \"shapes\", \"reach\", \"look_at\"}".into()),
            None => vec![op.clone()],
        };
        let default_ease = op.get("ease").and_then(Value::as_str).unwrap_or("smooth");
        let mut n = 0;
        let mut said: Vec<String> = Vec::new();
        let legacy_look = studio_mesh::perform::body::Body::of(&rig).head.is_none();
        for (i, k) in keys.iter().enumerate() {
            let bad = |w: String| format!("key {}: {w}", i + 1);
            let t = k.get("t").or_else(|| k.get("time")).or_else(|| k.get("at")).and_then(Value::as_f64).unwrap_or(0.0);
            let at = micros(t);
            let how = interp(k.get("ease").and_then(Value::as_str).unwrap_or(default_ease)).map_err(bad)?;
            // Hands, body poses, beats, recipes, looks that lead with the eyes, reaches to a target.
            let (more, said_more) = self.pose_layers(id, k, t, &mut mc, &rig).map_err(bad)?;
            n += more;
            said.extend(said_more);
            if let Some(b) = k.get("bones").and_then(Value::as_object) {
                for (given, v) in b {
                    // Any case or spelling of a bone's name (upper_arm.l,
                    // UpperArm.L, left_arm) is the rig's own bone.
                    let bi = rig.resolve_bone(given).map_err(&bad)?;
                    let name = &rig.bones[bi].name;
                    let d = degrees(v).ok_or_else(|| bad(format!("bone \"{name}\" takes [x, y, z] degrees")))?;
                    for (c, x) in ["x", "y", "z"].iter().zip(d) {
                        key(&mut mc, &format!("generator.bone.{name}.{c}"), at, m::Value::Angle(x as f64), how);
                        n += 1;
                    }
                }
            }
            if let Some(s) = k.get("shapes").and_then(Value::as_object) {
                for (name, v) in s {
                    let w = v.as_f64().ok_or_else(|| bad(format!("shape \"{name}\" takes a weight 0..1")))?;
                    let names = shape_names(&rig, name);
                    if names.is_empty() {
                        return Err(bad(format!("no shape key \"{name}\"; keys: {}", rig.shapes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))));
                    }
                    for nm in names {
                        key(&mut mc, &format!("generator.shape.{nm}"), at, m::Value::Scalar(w), how);
                        n += 1;
                    }
                }
            }
            if let Some(r) = k.get("reach").and_then(Value::as_object).filter(|r| !(r.contains_key("to") || r.contains_key("hand"))) {
                for (given, v) in r {
                    let Some(handle) = rig.constraints.iter().find(|c| c.kind() == "ik" && (c.name() == given || c.name().eq_ignore_ascii_case(given) || studio_mesh::rigfit::name_key(c.name()) == studio_mesh::rigfit::name_key(given))) else {
                        let iks: Vec<&str> = rig.constraints.iter().filter(|c| c.kind() == "ik").map(|c| c.name()).collect();
                        return Err(bad(format!("no IK \"{given}\"; IK handles: {}", iks.join(", "))));
                    };
                    let name = handle.name();
                    if v.is_null() || v.as_bool() == Some(false) {
                        key(&mut mc, &format!("generator.ik.{name}.on"), at, m::Value::Scalar(0.0), how);
                        continue;
                    }
                    let p = v3(v).ok_or_else(|| bad(format!("reach \"{name}\" takes [x, y, z] (model px) or false")))?;
                    for (c, x) in ["x", "y", "z"].iter().zip(p) {
                        key(&mut mc, &format!("generator.ik.{name}.{c}"), at, m::Value::Scalar(x as f64), how);
                    }
                    key(&mut mc, &format!("generator.ik.{name}.on"), at, m::Value::Scalar(1.0), how);
                    n += 4;
                }
            }
            if let Some(v) = k.get("look_at").filter(|_| legacy_look) {
                if v.is_null() || v.as_bool() == Some(false) {
                    key(&mut mc, "generator.look.on", at, m::Value::Scalar(0.0), how);
                } else {
                    let p = match v {
                        Value::Object(o) if o.contains_key("clip") => self.model_point_of(&mc, o.get("clip").and_then(Value::as_u64).unwrap_or(0) as u32, t)?,
                        _ => v3(v).ok_or_else(|| bad("look_at takes [x, y, z] (model px), {\"clip\": id} or false".into()))?,
                    };
                    for (c, x) in ["x", "y", "z"].iter().zip(p) {
                        key(&mut mc, &format!("generator.look.{c}"), at, m::Value::Scalar(x as f64), how);
                    }
                    key(&mut mc, "generator.look.on", at, m::Value::Scalar(1.0), how);
                }
                n += 4;
            }
        }
        if n == 0 {
            return Err("pose: give \"bones\" ({name: [x, y, z]}), \"shapes\" ({name: weight}), \"hand\" (relaxed, open, fist, point, cup, pinch, grip, hold_small), \"body\" (sit, crouch, lean), \"beat\" (nod, shake_head, shrug, sigh, startle, react, glance), \"recipe\" (strike, cupped_hands), \"reach\" ({hand: L, to: [x, y, z]}) or \"look_at\"".into());
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Pose", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("clip {id}: {n} keys set over {} pose{}{}", keys.len(), if keys.len() == 1 { "" } else { "s" }, if said.is_empty() { String::new() } else { format!("; {}", said.join("; ")) }))
    }

    /// Where another clip is, in this model's own pixels at clip time `t`.
    pub(super) fn model_point_of(&self, me: &m::Clip, other: u32, t: f64) -> Result<V3, String> {
        let oc = self.model_clip(other).ok_or_else(|| format!("no clip {other}"))?;
        let rel = micros(t);
        let at_other = me.start + rel - oc.start;
        let to = oc.transform_at(at_other.clamp(0, oc.duration.max(0)));
        let mine = me.transform_at(rel.clamp(0, me.duration.max(0)));
        let world = [to.position.x as f32, to.position.y as f32, to.position_z as f32];
        let here = [mine.position.x as f32, mine.position.y as f32, mine.position_z as f32];
        let d = studio_mesh::sub(world, here);
        // Undo the model's turn about y and its scale (enough for aiming).
        let a = (mine.rotation_y as f32).to_radians();
        let (s, c) = a.sin_cos();
        let local = [d[0] * c + d[2] * s, d[1], -d[0] * s + d[2] * c];
        let k = (mine.scale_x as f32).abs().max(1e-3);
        Ok([local[0] / k, local[1] / k, local[2] / k])
    }

    /// `act`: motion for a rigged model.
    pub(super) fn act(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("act needs \"clip\" (a rigged model clip)")? as u32;
        let (mut mc, mut mesh, rig) = self.rig_of(id)?;
        let at = f(op, "at").unwrap_or(0.0);
        let speed = f(op, "speed").unwrap_or(1.0).clamp(0.05, 20.0);
        if op.get("stop").and_then(Value::as_bool) == Some(true) {
            for k in ["action", "action_start", "action_speed", "action_offset", "blend_action", "blend"] {
                if let m::ClipSource::Generator(g) = &mut mc.source {
                    g.params.remove(k);
                }
                mc.keyframes.remove(&format!("generator.{k}"));
            }
            let seq = self.doc.seq();
            let ok = self.doc.apply("Stop motion", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("clip {id}: motion stopped"));
        }
        // A gait along a route (or a turn): planted feet, driven by the stride.
        if let Some(done) = self.act_walk(op, id, &mc, &mesh, &rig)? {
            return Ok(done);
        }
        // `to` naming a motion is the older way to say blend_to.
        let blend_to = op.get("blend_to").or_else(|| op.get("to").filter(|v| v.is_string()));
        let style = studio_mesh::motion::Style { cycle: f(op, "cycle").unwrap_or(0.0) as f32, energy: f(op, "energy").unwrap_or(1.0) as f32, rest_arm_droop: studio_mesh::motion::arm_droop(&rig) };
        let mut r = (*rig).clone();
        let mut said = Vec::new();
        // Which action plays: made, captured or already there.
        let name: String = if let Some(motion) = op.get("motion").and_then(Value::as_str) {
            let a = studio_mesh::motion::make(&r, motion, &style)?;
            let nm = a.name.clone();
            r.actions.retain(|x| x.name != nm);
            r.actions.push(a);
            said.push(format!("{motion} cycle made"));
            nm
        } else if let Some(file) = op.get("mocap").and_then(Value::as_str) {
            let path = std::path::Path::new(file);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("mocap").to_string();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let height = studio_mesh::motion::rig_height(&r);
            let (src, act) = match ext.as_str() {
                "bvh" => {
                    let text = std::fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
                    studio_mesh::mocap::read_bvh(&text, height, &stem)?
                }
                "glb" | "gltf" => {
                    let got = studio_mesh::import::load(path, height)?;
                    let src = got.mesh.rig.ok_or_else(|| format!("{file} has no skeleton"))?;
                    let pick = op.get("take").and_then(Value::as_str);
                    let a = match pick {
                        Some(n) => src.action(n).cloned().ok_or_else(|| format!("{file} has no animation \"{n}\"; it has {}", src.actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")))?,
                        None => src.actions.first().cloned().ok_or_else(|| format!("{file} has no animation"))?,
                    };
                    ((*src).clone(), a)
                }
                _ => return Err("mocap reads .bvh, .glb or .gltf files".into()),
            };
            let nm = op.get("name").and_then(Value::as_str).unwrap_or(&stem).to_string();
            let moved = studio_mesh::mocap::retarget(&src, &act, &r, &nm)?;
            said.push(format!("{} s of motion capture retargeted from {} bones", (moved.duration * 10.0).round() / 10.0, src.bones.len()));
            r.actions.retain(|x| x.name != nm);
            r.actions.push(moved);
            nm
        } else if let Some(a) = op.get("action").and_then(Value::as_str) {
            if r.action(a).is_none() {
                return Err(format!("the model has no action \"{a}\"; it has {}", if r.actions.is_empty() { "none".into() } else { r.actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ") }));
            }
            a.to_string()
        } else if blend_to.is_none() {
            return Err(format!("act: give \"motion\" ({}), \"mocap\" (a .bvh or .glb file), \"action\", \"blend_to\" or \"stop\": true", studio_mesh::motion::NAMES.join(", ")));
        } else {
            String::new()
        };
        if !name.is_empty() {
            set_param(&mut mc, "action", m::Value::Text(name.clone()));
            set_param(&mut mc, "action_start", m::Value::Scalar(at));
            set_param(&mut mc, "action_speed", m::Value::Scalar(speed));
            set_param(&mut mc, "action_loop", m::Value::Bool(op.get("loop").and_then(Value::as_bool).unwrap_or(true)));
            if let Some(o) = f(op, "offset") {
                set_param(&mut mc, "action_offset", m::Value::Scalar(o));
            }
        }
        // A blend into a second motion over a span.
        if let Some(to) = blend_to.and_then(Value::as_str) {
            let target = if r.action(to).is_some() {
                to.to_string()
            } else {
                let a = studio_mesh::motion::make(&r, to, &style).map_err(|e| format!("blend_to: {e}"))?;
                let nm = a.name.clone();
                r.actions.retain(|x| x.name != nm);
                r.actions.push(a);
                nm
            };
            let (b0, b1) = match op.get("over").and_then(Value::as_array) {
                Some(a) if a.len() == 2 => (a[0].as_f64().unwrap_or(at), a[1].as_f64().unwrap_or(at + 0.5)),
                _ => (at, at + 0.5),
            };
            set_param(&mut mc, "blend_action", m::Value::Text(target.clone()));
            mc.keyframes.remove("generator.blend");
            key(&mut mc, "generator.blend", micros(b0), m::Value::Scalar(0.0), m::Interp::Ease);
            key(&mut mc, "generator.blend", micros(b1.max(b0 + 0.04)), m::Value::Scalar(1.0), m::Interp::Ease);
            said.push(format!("blends into {target} from {b0:.2} to {b1:.2} s"));
        }
        // Walking carries the layer along the way it faces.
        let motion = op.get("motion").and_then(Value::as_str).unwrap_or("");
        let travel = op.get("travel").and_then(Value::as_bool).unwrap_or(matches!(motion, "walk" | "run" | "sneak"));
        if travel && matches!(motion, "walk" | "run" | "sneak") {
            let per_cycle = studio_mesh::motion::travel_per_cycle(&r, motion, &style) as f64;
            let cycle = if style.cycle > 0.0 { style.cycle as f64 } else { studio_mesh::motion::default_cycle(motion) as f64 };
            let clip_secs = mc.duration as f64 / 1e6;
            let dur = f(op, "duration").unwrap_or(clip_secs - at).max(0.1);
            let tr = mc.transform_at(micros(at));
            let k = tr.scale_x.abs();
            let dist = per_cycle / cycle * speed * dur * k;
            let a = tr.rotation_y.to_radians();
            let dir = [a.sin(), -a.cos()];
            let (x0, z0) = (tr.position.x, tr.position_z);
            let (x1, z1) = (x0 + dir[0] * dist, z0 + dir[1] * dist);
            let y = tr.position.y;
            mc.keyframes.remove("transform.position");
            mc.keyframes.remove("transform.position_z");
            key(&mut mc, "transform.position", micros(at), m::Value::Point(m::Vec2::new(x0, y)), m::Interp::Linear);
            key(&mut mc, "transform.position", micros(at + dur), m::Value::Point(m::Vec2::new(x1, y)), m::Interp::Linear);
            key(&mut mc, "transform.position_z", micros(at), m::Value::Scalar(z0), m::Interp::Linear);
            key(&mut mc, "transform.position_z", micros(at + dur), m::Value::Scalar(z1), m::Interp::Linear);
            said.push(format!("travels {:.0} px over {dur:.1} s", dist));
        }
        let _ = param_f(&mc, "action_speed");
        if r != *rig {
            mesh.rig = Some(Arc::new(r));
            self.save_rigged(mc, &mesh, "Motion")?;
        } else {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Motion", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
        }
        Ok(format!("clip {id}: {}{}", if name.is_empty() { String::new() } else { format!("plays \"{name}\" from {at:.2} s at {speed}× ; ") }, said.join("; ")))
    }
}

/// The vertices an op means: those of a `material` (name or number) and/or
/// a `region`; every vertex when neither is given.
pub(super) fn vertex_mask(mesh: &Mesh, op: &Value) -> Result<Vec<bool>, String> {
    let n = mesh.positions.len();
    let mut mask = vec![true; n];
    if let Some(m) = op.get("material") {
        let idx = match m {
            Value::Number(k) => k.as_u64().unwrap_or(0) as u32,
            Value::String(name) => mesh.materials.iter().position(|x| x.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no material \"{name}\"; materials: {}", mesh.materials.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")))? as u32,
            _ => return Err("\"material\" is a material name or number".into()),
        };
        let mats = mesh.tri_materials();
        mask = vec![false; n];
        for t in 0..mesh.triangles() {
            if mats[t] == idx {
                for v in mesh.tri(t) {
                    mask[v] = true;
                }
            }
        }
    }
    if op.get("region").is_some() {
        let inside = super::models::region(op.get("region"))?.vertices(mesh);
        for (m, i) in mask.iter_mut().zip(inside) {
            *m = *m && i;
        }
    }
    Ok(mask)
}

const CONSTRAINT_EXAMPLES: &str = "e.g. {\"kind\": \"attach\", \"bone\": \"head\", \"material\": \"Hat\"} holds the Hat material rigid on the head; {\"kind\": \"ik\", \"bone\": \"forearm.L\", \"chain\": 2, \"target\": [x, y, z]}; {\"kind\": \"look_at\", \"bone\": \"head\", \"target\": [x, y, z]}; {\"kind\": \"limit\", \"bone\": \"forearm.L\", \"min\": [0, 0, 0], \"max\": [140, 0, 0]}; {\"kind\": \"copy_rotation\", \"bone\": \"toe.L\", \"from\": \"foot.L\"}";

/// A constraint from JSON: {"kind": "ik"|"look_at"|"limit"|"copy_rotation"|
/// "attach", "name", "bone", "chain", "target" ([x,y,z] or a bone name),
/// "pole", "min", "max", "from", "influence"}. An `attach` is not kept as a
/// rule: it binds the surface chosen by `material` or `region` whole to the
/// bone, which is what holding a hat on a head means.
fn add_constraint(r: &mut Rig, mesh: &Mesh, c: &Value) -> Result<String, String> {
    let kind = c.get("kind").and_then(Value::as_str).ok_or_else(|| format!("constraint needs \"kind\": ik, look_at, limit, copy_rotation or attach, {CONSTRAINT_EXAMPLES}"))?;
    let bone_name = c.get("bone").and_then(Value::as_str).ok_or_else(|| format!("constraint needs \"bone\", the bone it acts on (the bone names are: {}), {CONSTRAINT_EXAMPLES}", r.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")))?;
    let bone = r.resolve_bone(bone_name).map_err(|e| format!("{e}; {CONSTRAINT_EXAMPLES}"))?;
    let bone_name = r.bones[bone].name.clone();
    let kind = match kind.to_lowercase().as_str() {
        "attach" | "attachment" | "parent" | "child_of" | "hold" | "fix" | "pin" => "attach",
        "ik" => "ik",
        "look_at" | "track" | "aim" | "lookat" => "look_at",
        "limit" => "limit",
        "copy_rotation" | "copy" => "copy_rotation",
        other => return Err(format!("constraint kind \"{other}\": ik, look_at, limit, copy_rotation or attach, {CONSTRAINT_EXAMPLES}")),
    };
    if kind == "attach" {
        if !r.is_skinned() {
            return Err("this rig has no skin yet: bind it first with {\"op\": \"rig\", \"clip\": id, \"skinning\": \"smooth\"}".into());
        }
        if c.get("material").is_none() && c.get("region").is_none() {
            return Err(format!("attach needs \"material\" (the name or number of the material to hold, from the model's list) or \"region\" (e.g. \"top\", or a sphere around the part), {CONSTRAINT_EXAMPLES}"));
        }
        let mask = vertex_mask(mesh, c)?;
        let verts: Vec<usize> = (0..mask.len()).filter(|&v| mask[v]).collect();
        if verts.is_empty() {
            return Err("attach: nothing is in that material or region".into());
        }
        studio_mesh::rigfit::bind_to_bone(r, &verts, bone);
        return Ok(format!("{} vertices attached rigid to {bone_name}", verts.len()));
    }
    let name = c.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{kind}.{bone_name}"));
    let influence = c.get("influence").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let target = |r: &Rig| -> Result<Target, String> {
        match c.get("target") {
            Some(Value::String(b)) => r.resolve_bone(b).map(Target::Bone),
            Some(v) => v3(v).map(Target::Point).ok_or_else(|| "\"target\" is [x, y, z] (model px) or a bone name".to_string()),
            None => Ok(Target::Point(r.bones[bone].tail)),
        }
    };
    let made = match kind {
        "ik" => Constraint::Ik { name: name.clone(), bone, chain: c.get("chain").and_then(Value::as_u64).unwrap_or(2).clamp(1, 32) as usize, target: target(r)?, pole: c.get("pole").and_then(v3), influence },
        "look_at" => Constraint::LookAt { name: name.clone(), bone, target: target(r)?, influence },
        "limit" => Constraint::Limit { name: name.clone(), bone, min: c.get("min").and_then(v3).unwrap_or([-180.0; 3]), max: c.get("max").and_then(v3).unwrap_or([180.0; 3]) },
        _ => {
            let from = c.get("from").and_then(Value::as_str).ok_or("copy_rotation needs \"from\" (a bone)")?;
            Constraint::CopyRotation { name: name.clone(), bone, from: r.resolve_bone(from)?, influence }
        }
    };
    r.constraints.retain(|x| x.name() != name);
    r.constraints.push(made);
    Ok(format!("{kind} constraint \"{name}\" on {bone_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn a_character_walks_waves_and_smiles() {
        let mut s = Session::new(services::project::new_custom("Anim", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "detail": 1, "duration": 4})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.rig.as_ref().is_some_and(|r| r.is_skinned() && r.shape("viseme_aa").is_some()));
        let r = s.edit(&[json!({"op": "act", "clip": id, "motion": "walk", "duration": 4})], "walk", false).unwrap();
        assert!(r.to_string().contains("travels"), "{r}");
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.contains_key("transform.position_z"));
        s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 0, "shapes": {"smile": 0}}, {"t": 1, "shapes": {"smile": 1}, "bones": {"head": [0, 20, 0]}}]})], "smile", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.contains_key("generator.shape.mouthSmileLeft") && mc.keyframes.contains_key("generator.bone.head.y"));
        // The renderer poses it from those settings.
        let p = mc.generator_params_at(micros(1.0));
        assert!(matches!(p.get("action"), Some(m::Value::Text(a)) if a == "walk"));
        let bad = s.edit(&[json!({"op": "pose", "clip": id, "bones": {"tentacle": [1, 2, 3]}})], "x", false);
        assert!(bad.is_err());
        // A blend into a run, and a stop.
        s.edit(&[json!({"op": "act", "clip": id, "blend_to": "run", "over": [2, 3]})], "run", false).unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.rig.as_ref().unwrap().action("run").is_some());
    }

    #[test]
    fn rig_a_plain_model_and_retarget_a_capture() {
        let mut s = Session::new(services::project::new_custom("Anim", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "cylinder", "size": [60, 300, 60], "detail": 5})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let r = s.edit(&[json!({"op": "rig", "clip": id, "template": "chain", "count": 4, "start": "top"})], "rig", false).unwrap();
        assert!(r.to_string().contains("chain rig: 4 bones"), "{r}");
        s.edit(&[json!({"op": "rig", "clip": id, "constraint": {"kind": "ik", "bone": "bone.3", "chain": 4, "target": [80, 100, 0]}})], "ik", false).unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert_eq!(mesh.rig.as_ref().unwrap().constraints.len(), 1);
        // Undo takes the constraint back off.
        s.doc.undo();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.rig.as_ref().unwrap().constraints.is_empty());
    }

    #[test]
    fn a_part_added_to_a_rigged_character_keeps_the_rig_and_bones_match_in_any_case() {
        let mut s = Session::new(services::project::new_custom("Anim", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "detail": 1, "duration": 4})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (_, before) = s.model_of(id).unwrap();
        let top = before.bounds().0[1];
        // A part on top of the head: the model stays rigged and loads again.
        let r = s.edit(&[json!({"op": "mesh", "clip": id, "do": "add", "shape": "cylinder", "size": [80, 20, 80], "at": [0.0, top - 8.0, -10.0], "colour": "#334455", "name": "Hat"})], "hat", false).unwrap();
        assert!(r.to_string().contains("skinned"), "{r}");
        let (_, after) = s.model_of(id).unwrap();
        let rig = after.rig.clone().unwrap();
        assert!(after.rig_in_step() && rig.is_skinned());
        let head = rig.bone("head").unwrap();
        let k = before.positions.len();
        assert!((k..after.positions.len()).all(|v| rig.joints[v][0] as usize == head && rig.weights[v][0] == 1.0), "rigid on the head");
        // Bones in any case; the key is written under the rig's own name.
        s.edit(&[json!({"op": "pose", "clip": id, "bones": {"UPPER_ARM.l": [0, 0, 30], "Head": [0, 20, 0]}})], "pose", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.contains_key("generator.bone.upper_arm.L.z") && mc.keyframes.contains_key("generator.bone.head.y"));
        let err = s.edit(&[json!({"op": "pose", "clip": id, "bones": {"tentacle": [1, 2, 3]}})], "x", false).unwrap_err();
        assert!(err.contains("all bones:") && err.contains("upper_arm.L"), "{err}");
        // An attach constraint holds a material rigid on a bone; its errors carry an example.
        let err = s.edit(&[json!({"op": "rig", "clip": id, "constraint": {"kind": "attachment"}})], "x", false).unwrap_err();
        assert!(err.contains("needs \"bone\"") && err.contains("\"kind\": \"attach\""), "{err}");
        s.edit(&[json!({"op": "rig", "clip": id, "constraint": {"kind": "attachment", "bone": "HEAD", "material": "Hat"}})], "attach", false).unwrap();
        let err = s.edit(&[json!({"op": "mesh", "clip": id, "do": "add", "shape": "cube", "size": 10, "bind": "bone", "bone": "tentacle"})], "x", false).unwrap_err();
        assert!(err.contains("all bones:"), "{err}");
    }
}
