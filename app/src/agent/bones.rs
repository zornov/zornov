// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skeletons for agents: bones that bend a character picture and carry
//! its part layers (`services::character`, `services::rigging`).
//!
//! Points are canvas pixels from the centre (`[x, y]`), `{"u", "v"}`
//! across the layer, or a bone's name (its tip, where it is posed at
//! `at`). Bones are named by role (`hips`, `spine`, `head`,
//! `upper_arm.front`, `forearm.back`, `thigh.front`, `shin.back`,
//! `front_leg.upper`, `tail.2`, `trunk.3`…) and found by name or id.
//!
//! * `skeleton`: `build` a kind from the silhouette, draw `bones`, change
//!   one (`bone` + `min` / `max` / `depth` / `deform` / `rename` /
//!   `parent`), `remove`, `act` (motions baked into bone keys).
//! * `pose`: bone rotations now or as keys, the pose library (`save`,
//!   `apply`), `copy` / `paste`, `mirror`, `reset`, pose-to-pose `keys`.
//! * `ik`: reach a hand or foot to a point (the limb follows), baked as
//!   rotations, or `live` on an IK handle whose target is keyed.
//! * `parts`: the picture split into part layers on the bones.
//! * `swap`: swap sets (mouth shapes, hand poses, eye states).
//! * `mocap`: a filmed person drives the bones.

use super::{Session, f, micros, secs};
use crate::editor::pins::LayerMap;
use crate::services::{character as ch, rigging as rg};
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_model as m;

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

impl Session {
    /// The clip, its view, and the moment `at` (default its start).
    fn rig_target(&self, op: &Value, what: &str) -> Result<(u32, m::Clip, view::Clip, view::Time), String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or_else(|| format!("{what} needs \"clip\""))? as u32;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let vc = self.clip(id)?;
        let at = f(op, "at").map(micros).unwrap_or(vc.start);
        if at < vc.start || at > vc.start + vc.dur {
            return Err(format!("\"at\" {:.2}s is outside clip {id} ({:.2}–{:.2}s)", secs(at), secs(vc.start), secs(vc.start + vc.dur)));
        }
        Ok((id, mc, vc, at))
    }

    fn rig_map(&self, id: u32, t: view::Time) -> Result<LayerMap, String> {
        LayerMap::at(&self.doc.project, &self.doc.model, id, t).ok_or_else(|| format!("clip {id} draws nothing on the canvas at {:.2}s", secs(t)))
    }

    /// The skeleton of `mc`, posed at timeline time `t`.
    fn posed_at(mc: &m::Clip, vc: &view::Clip, t: view::Time) -> Option<(m::Skeleton, m::Posed)> {
        let sk = mc.skeleton_at((t - vc.start).clamp(0, vc.dur))?;
        let p = sk.forward();
        Some((sk, p))
    }

    /// A bone by name or id.
    fn find_bone(sk: &m::Skeleton, v: &Value) -> Result<u32, String> {
        let found = match v {
            Value::Number(n) => n.as_u64().and_then(|id| sk.bone(id as u32)),
            Value::String(s) => sk.named(s).or_else(|| s.parse::<u32>().ok().and_then(|id| sk.bone(id))).or_else(|| {
                // "front hand", "left foot": the bone ending there.
                let n = s.to_lowercase().replace(' ', "_");
                let want = |b: &&m::Bone| {
                    let bn = b.name.to_lowercase();
                    let side = if n.contains("back") { "back" } else { "front" };
                    (n.contains("hand") && bn.starts_with("forearm") && bn.contains(side)) || (n.contains("foot") && (bn.starts_with("shin") || bn.ends_with(".lower")) && (bn.contains(side) || bn.starts_with(side)))
                };
                sk.bones.iter().find(want)
            }),
            _ => None,
        };
        found.map(|b| b.id).ok_or_else(|| format!("no bone {v}; bones: {}", sk.bones.iter().map(|b| if b.name.is_empty() { b.id.to_string() } else { format!("{} \"{}\"", b.id, b.name) }).collect::<Vec<_>>().join(", ")))
    }

    /// A point in layer coordinates: canvas `[x, y]`, `{"u", "v"}`, or a
    /// bone's name (its tip as posed).
    fn rig_point(map: &LayerMap, sk: Option<(&m::Skeleton, &m::Posed)>, v: &Value) -> Result<m::Vec2, String> {
        match v {
            Value::Array(a) if a.len() == 2 => {
                let c = eframe::egui::Vec2::new(a[0].as_f64().ok_or("[x, y] are numbers")? as f32, a[1].as_f64().ok_or("[x, y] are numbers")? as f32);
                map.layer(c).ok_or_else(|| "the layer has no size there".to_owned())
            }
            Value::Object(o) if o.contains_key("u") || o.contains_key("v") => Ok(m::Vec2::new(o.get("u").and_then(Value::as_f64).unwrap_or(0.5), o.get("v").and_then(Value::as_f64).unwrap_or(0.5))),
            Value::String(_) | Value::Number(_) => {
                let (s, p) = sk.ok_or("there is no skeleton to name a bone of")?;
                let id = Self::find_bone(s, v)?;
                Ok(p.bone(id).map(|b| b.tail).unwrap_or(m::Vec2::ZERO))
            }
            _ => Err("a point is [x, y] canvas pixels from the centre, {\"u\": 0..1, \"v\": 0..1} across the layer, or a bone's name".into()),
        }
    }

    /// The skeleton as a report: each bone's place on the canvas at `t`.
    pub(super) fn skeleton_said(&self, id: u32, t: view::Time) -> Option<Value> {
        let mc = self.model_clip(id)?;
        let vc = self.clip(id).ok()?;
        let (sk, posed) = Self::posed_at(&mc, &vc, t)?;
        let map = self.rig_map(id, t).ok()?;
        let at = |p: m::Vec2| {
            let c = map.canvas(p);
            json!([r1(c.x as f64), r1(c.y as f64)])
        };
        let bones: Vec<Value> = sk
            .bones
            .iter()
            .filter_map(|b| {
                let pb = posed.bone(b.id)?;
                let mut o = json!({"bone": b.id, "name": b.name, "rotation": r1(b.rotation), "head": at(pb.head), "tip": at(pb.tail)});
                if let Some(p) = b.parent.and_then(|p| sk.bone(p)) {
                    o["parent"] = json!(p.name);
                }
                if b.min.is_some() || b.max.is_some() {
                    o["limits"] = json!([b.min.map(r1), b.max.map(r1)]);
                }
                if b.depth != 0.0 {
                    o["depth"] = json!(b.depth);
                }
                if !b.deform {
                    o["deform"] = json!(false);
                }
                if mc.keyframes.keys().any(|k| k.starts_with(&format!("bone.{}.", b.id))) {
                    o["animated"] = json!(true);
                }
                Some(o)
            })
            .collect();
        let mut out = json!({"bones": bones, "faces": match sk.faces_right { Some(true) => "right", Some(false) => "left", None => "camera" }});
        if !sk.ik.is_empty() {
            out["ik"] = json!(sk.ik.iter().map(|h| json!({"handle": h.id, "name": h.name, "bone": sk.bone(h.bone).map(|b| b.name.clone()), "chain": h.chain, "live": h.enabled, "target": at(h.target)})).collect::<Vec<_>>());
        }
        if !sk.poses.is_empty() {
            out["poses"] = json!(sk.poses.iter().map(|p| p.name.clone()).collect::<Vec<_>>());
        }
        let parts: Vec<Value> = self
            .doc
            .model
            .active()
            .clips
            .iter()
            .filter(|c| c.meta.get("part").and_then(|p| p.get("rig")).and_then(Value::as_u64) == Some(id as u64))
            .map(|c| json!({"clip": c.id.0, "name": c.name, "bone": c.parent.as_ref().and_then(|p| p.bone.as_ref()).and_then(|l| sk.bone(l.bone)).map(|b| b.name.clone())}))
            .collect();
        if !parts.is_empty() {
            out["parts"] = json!(parts);
        }
        let sets = ch::swap_sets(&self.doc, id);
        if !sets.is_empty() {
            out["swap_sets"] = json!(sets.into_iter().map(|(s, o)| (s, json!(o))).collect::<serde_json::Map<_, _>>());
        }
        Some(out)
    }

    fn rig_said(&self, id: u32, t: view::Time) -> String {
        self.skeleton_said(id, t).map_or(String::new(), |v| format!("; skeleton now (canvas px at {:.2}s): {v}", secs(t)))
    }

    /// `skeleton`: bones for a character.
    pub(super) fn skeleton(&mut self, op: &Value) -> Result<String, String> {
        let (id, _, vc, at) = self.rig_target(op, "skeleton")?;
        let mut said = Vec::new();
        if let Some(kind) = op.get("build").or_else(|| op.get("kind")) {
            let kind = kind.as_str().ok_or_else(|| format!("build names a kind: {}", rg::KINDS))?;
            let mc = self.model_clip(id).ok_or("clip gone")?;
            // `facing` is how it looks on the canvas now; a flip mirrors
            // the picture.
            let facing = match op.get("facing").and_then(Value::as_str) {
                Some("right") => Some(Some(!mc.transform.flip_x)),
                Some("left") => Some(Some(mc.transform.flip_x)),
                Some("camera" | "front") => Some(None),
                Some(other) => return Err(format!("facing \"{other}\": left, right or camera (as it looks on the canvas now)")),
                None => None,
            };
            said.push(ch::build(&mut self.doc, id, kind, facing, at)?);
        }
        let mut mc = self.model_clip(id).ok_or("clip gone")?;
        // A rigged character gets the cuts that let its legs, arms and
        // mouth move apart.
        let built = op.get("build").or_else(|| op.get("kind")).and_then(Value::as_str).map(str::to_lowercase);
        let cut_said = self.cut_work(op, id, &mut mc, at, built.as_deref())?;
        let cuts_changed = !cut_said.is_empty();
        said.extend(cut_said);
        if let Some(Value::Array(list)) = op.get("bones") {
            let map = self.rig_map(id, at)?;
            let px = map.pixels();
            let sk = mc.skeleton.get_or_insert_with(|| m::Skeleton { aspect: (px.x / px.y.max(1.0)) as f64, ..m::Skeleton::default() });
            for b in list {
                let name = b.get("name").and_then(Value::as_str).unwrap_or_default();
                let parent = match b.get("parent") {
                    Some(v) => Some(Self::find_bone(sk, v)?),
                    None => None,
                };
                let head = Self::rig_point(&map, None, b.get("head").ok_or("each bone needs \"head\" (where it turns) and \"tail\"")?)?;
                let tail = Self::rig_point(&map, None, b.get("tail").ok_or("each bone needs \"tail\"")?)?;
                let bid = sk.add(name, parent, head, tail);
                if let Some(bone) = sk.bone_mut(bid) {
                    bone.min = f(b, "min");
                    bone.max = f(b, "max");
                    bone.depth = f(b, "depth").unwrap_or(0.0).clamp(-1.0, 1.0);
                    bone.deform = b.get("deform").and_then(Value::as_bool).unwrap_or(true);
                }
                said.push(format!("bone {bid} \"{name}\""));
            }
        }
        if let Some(r) = op.get("remove") {
            let sk = mc.skeleton.as_mut().ok_or_else(|| format!("clip {id} has no skeleton"))?;
            if r.as_str() == Some("all") {
                mc.skeleton = None;
                mc.keyframes.retain(|k, _| !k.starts_with("bone.") && !k.starts_with("ik."));
                said.push("skeleton removed".into());
            } else {
                let bid = Self::find_bone(sk, r)?;
                // Its children hang from its parent.
                let up = sk.bone(bid).and_then(|b| b.parent);
                for b in &mut sk.bones {
                    if b.parent == Some(bid) {
                        b.parent = up;
                    }
                }
                sk.bones.retain(|b| b.id != bid);
                sk.ik.retain(|h| h.bone != bid);
                mc.keyframes.retain(|k, _| !k.starts_with(&format!("bone.{bid}.")));
                said.push(format!("bone {bid} removed"));
            }
        }
        if let Some(bv) = op.get("bone") {
            let sk = mc.skeleton.as_mut().ok_or_else(|| format!("clip {id} has no skeleton"))?;
            let bid = Self::find_bone(sk, bv)?;
            let parent = match op.get("parent") {
                Some(Value::Null) => Some(None),
                Some(v) => Some(Some(Self::find_bone(sk, v)?)),
                None => None,
            };
            let b = sk.bone_mut(bid).expect("found");
            if let Some(v) = f(op, "min") {
                b.min = Some(v);
            }
            if let Some(v) = f(op, "max") {
                b.max = Some(v);
            }
            if let Some(v) = f(op, "depth") {
                b.depth = v.clamp(-1.0, 1.0);
            }
            if let Some(v) = op.get("deform").and_then(Value::as_bool) {
                b.deform = v;
            }
            if let Some(n) = op.get("rename").and_then(Value::as_str) {
                b.name = n.to_owned();
            }
            if let Some(p) = parent {
                b.parent = p;
            }
            sk.sort();
            said.push(format!("bone {bid} changed"));
        }
        if let Some(v) = op.get("enabled").and_then(Value::as_bool) {
            mc.skeleton.as_mut().ok_or_else(|| format!("clip {id} has no skeleton"))?.enabled = v;
            said.push(if v { "skeleton on".into() } else { "skeleton off (bones kept, the picture unbent)".into() });
        }
        if let Some(v) = f(op, "falloff") {
            mc.skeleton.as_mut().ok_or_else(|| format!("clip {id} has no skeleton"))?.falloff = if v > 1.0 { v / 100.0 } else { v }.clamp(0.005, 0.5);
        }
        if cuts_changed || op.get("bones").is_some() || op.get("remove").is_some() || op.get("bone").is_some() || op.get("enabled").is_some() || op.get("falloff").is_some() {
            self.set_rig(mc, "Skeleton")?;
        }
        if let Some(a) = op.get("act") {
            said.push(self.bone_act(id, &vc, a, op)?);
        }
        if said.is_empty() {
            return Err(format!("skeleton does nothing without build (a kind: {}), bones, bone + a change, remove, enabled or act", rg::KINDS));
        }
        Ok(format!("{}{}", said.join("; "), self.rig_said(id, at)))
    }

    /// `puppet` with `act` on a character that has bones: the act goes on
    /// its bones (pins' acts are for pictures without a skeleton), and
    /// whatever else the op asks (a voice, blinks, pins) runs after.
    pub(super) fn bones_first(&mut self, op: &Value) -> Option<Result<String, String>> {
        let a = op.get("act")?;
        let id = op.get("clip").and_then(Value::as_u64)? as u32;
        if op.get("rig").is_some() || self.model_clip(id)?.skeleton.is_none() {
            return None;
        }
        let vc = match self.clip(id) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let mut said = match self.bone_act(id, &vc, a, op) {
            Ok(s) => s,
            Err(e) => return Some(Err(e)),
        };
        let mut rest = op.clone();
        if let Some(o) = rest.as_object_mut() {
            for k in ["act", "travel", "speed"] {
                o.remove(k);
            }
            let left = o.keys().any(|k| !matches!(k.as_str(), "op" | "clip" | "at" | "from" | "duration" | "facing"));
            if left {
                match self.puppet(&rest) {
                    Ok(more) => said = format!("{said}; {more}"),
                    Err(e) => return Some(Err(e)),
                }
            }
        }
        let at = f(op, "at").map(micros).unwrap_or(vc.start);
        Some(Ok(format!("{said}{}", self.rig_said(id, at))))
    }

    fn set_rig(&mut self, mc: m::Clip, label: &str) -> Result<(), String> {
        let seq = self.doc.seq();
        let ok = self.doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "").map(|_| ())
    }

    /// Acts on a skeleton (from `skeleton` or `puppet` with `act`).
    pub(super) fn bone_act(&mut self, id: u32, vc: &view::Clip, a: &Value, op: &Value) -> Result<String, String> {
        let acts: Vec<String> = match a {
            Value::String(s) => vec![s.to_lowercase()],
            Value::Array(l) => l.iter().filter_map(Value::as_str).map(str::to_lowercase).collect(),
            _ => Vec::new(),
        };
        if acts.is_empty() {
            return Err(format!("act names one motion or a list of them: {}", rg::ACTS));
        }
        let len = secs(vc.dur);
        let from = f(op, "from").unwrap_or(0.0).clamp(0.0, len);
        let dur = f(op, "duration").unwrap_or(len - from).min(len - from);
        if dur < 0.5 {
            return Err(format!("act needs at least half a second of the clip ({:.2}s from {from:.2}s)", dur.max(0.0)));
        }
        let speed = f(op, "speed").unwrap_or(1.0).clamp(0.2, 4.0);
        let done = ch::act(&mut self.doc, id, &acts, from, dur, speed)?;
        let mut said = format!("{} on the bones from {from:.2}s for {dur:.2}s ({})", acts.join(" + "), done.bones.join(", "));
        // Travel: a number of pixels, or "auto" for the gait's own pace.
        let travel = match op.get("travel") {
            Some(Value::String(s)) if s == "auto" => done.travel.map(|v| {
                let mc = self.model_clip(id);
                let flip = mc.is_some_and(|c| c.transform.flip_x);
                v * dur * if done.right != flip { 1.0 } else { -1.0 }
            }),
            Some(v) => v.as_f64(),
            None => None,
        };
        if let Some(dx) = travel {
            let mc = self.model_clip(id).ok_or("clip gone")?;
            let prop = super::props::resolve(&mc, "x")?;
            let path = prop.paths[0].clone();
            let x0 = super::props::value_or_default(&mc, &path).map(|v| super::props::to_friendly(&prop, &path, &v)).and_then(|v| v.as_f64()).unwrap_or(0.0);
            self.animate(&json!({"clip": id, "property": "x", "keys": [[from, x0], [from + dur, x0 + dx]], "ease": "linear"}))?;
            said.push_str(&format!("; travels {dx:+.0} px across"));
        } else if let Some(v) = done.travel {
            said.push_str(&format!("; to keep the feet from sliding it travels about {v:.0} px a second toward the way it faces (\"travel\": \"auto\")"));
        }
        Ok(said)
    }

    /// `pose`: bone rotations, the pose library and pose keys.
    pub(super) fn pose(&mut self, op: &Value) -> Result<String, String> {
        let (id, mc, vc, at) = self.rig_target(op, "pose")?;
        let sk = mc.skeleton.clone().ok_or_else(|| format!("clip {id} has no skeleton: skeleton {{\"build\": kind}} first"))?;
        let mut said = Vec::new();
        let map = self.rig_map(id, at)?;
        let key = op.get("key").and_then(Value::as_bool).unwrap_or(false);
        // Pose-to-pose: a pose at each of several times.
        if let Some(Value::Array(keys)) = op.get("keys") {
            for k in keys {
                let t = f(k, "t").ok_or("each pose key needs \"t\" (seconds from the clip's start) and \"bones\" or \"pose\"")?;
                let mut one = k.clone();
                if let Some(o) = one.as_object_mut() {
                    o.remove("t");
                    o.insert("clip".into(), json!(id));
                    o.insert("at".into(), json!(secs(vc.start) + t));
                    o.insert("key".into(), json!(true));
                }
                self.pose(&one)?;
            }
            said.push(format!("{} pose keys", keys.len()));
        }
        let mut rotations: Vec<(u32, f64)> = Vec::new();
        let mut offsets: Vec<(u32, m::Vec2)> = Vec::new();
        if let Some(Value::Object(bones)) = op.get("bones") {
            for (name, v) in bones {
                let bid = Self::find_bone(&sk, &json!(name))?;
                rotations.push((bid, v.as_f64().ok_or_else(|| format!("bones maps a bone to degrees from rest (\"{name}\": 30)"))?));
            }
        }
        if let Some(Value::Object(offs)) = op.get("offset") {
            let px = map.pixels();
            for (name, v) in offs {
                let bid = Self::find_bone(&sk, &json!(name))?;
                let d = v.as_array().filter(|a| a.len() == 2).ok_or("offset maps a bone to [dx, dy] canvas pixels")?;
                // Canvas pixels → layer units, through the layer's scale.
                let o = map.canvas(m::Vec2::ZERO);
                let ux = map.canvas(m::Vec2::new(1.0, 0.0)) - o;
                let vy = map.canvas(m::Vec2::new(0.0, 1.0)) - o;
                let (dx, dy) = (d[0].as_f64().unwrap_or(0.0), d[1].as_f64().unwrap_or(0.0));
                let _ = px;
                let off = sk.bone(bid).map_or(m::Vec2::ZERO, |b| b.offset);
                offsets.push((bid, m::Vec2::new(off.x + dx / (ux.length() as f64).max(1e-6), off.y + dy / (vy.length() as f64).max(1e-6))));
            }
        }
        let mut library = sk.poses.clone();
        if let Some(name) = op.get("apply").and_then(Value::as_str) {
            let p = library.iter().find(|p| p.name.eq_ignore_ascii_case(name)).cloned().or_else(|| self.library_pose(name)).ok_or_else(|| format!("no pose \"{name}\"; poses: {}", library.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")))?;
            for (b, r, o) in sk.pose_targets(&p) {
                rotations.push((b, r));
                if let Some(o) = o {
                    offsets.push((b, o));
                }
            }
            said.push(format!("pose \"{name}\""));
        }
        if let Some(v) = op.get("paste") {
            let p: m::Pose = serde_json::from_value(if v.get("rotations").is_some() { v.clone() } else { json!({"name": "pasted", "rotations": v}) }).map_err(|e| format!("paste takes a copied pose ({{\"rotations\": {{bone: degrees}}}}): {e}"))?;
            for (b, r, o) in sk.pose_targets(&p) {
                rotations.push((b, r));
                if let Some(o) = o {
                    offsets.push((b, o));
                }
            }
            said.push("pose pasted".into());
        }
        if op.get("reset").and_then(Value::as_bool).unwrap_or(false) {
            rotations = sk.bones.iter().map(|b| (b.id, 0.0)).collect();
            offsets = sk.bones.iter().filter(|b| b.offset != m::Vec2::ZERO).map(|b| (b.id, m::Vec2::ZERO)).collect();
            said.push("rest pose".into());
        }
        let now = Self::posed_at(&mc, &vc, at).map(|p| p.0).unwrap_or(sk.clone());
        if op.get("mirror").and_then(Value::as_bool).unwrap_or(false) {
            // Front and back limbs trade poses.
            for b in &now.bones {
                let other = if b.name.contains("front") { b.name.replace("front", "back") } else if b.name.contains("back") { b.name.replace("back", "front") } else { continue };
                if let Some(o) = now.named(&other) {
                    rotations.push((b.id, o.rotation));
                }
            }
            said.push("mirrored front and back".into());
        }
        if !rotations.is_empty() || !offsets.is_empty() {
            ch::pose(&mut self.doc, id, &rotations, &offsets, at, key)?;
            said.push(format!("{} bone(s) posed{}", rotations.len().max(offsets.len()), if key { " as keys" } else { "" }));
        }
        if let Some(name) = op.get("save").and_then(Value::as_str) {
            let mc = self.model_clip(id).ok_or("clip gone")?;
            let (now, _) = Self::posed_at(&mc, &vc, at).ok_or("no skeleton")?;
            library.retain(|p| !p.name.eq_ignore_ascii_case(name));
            library.push(now.pose_now(name));
            let mut mc = mc;
            if let Some(s) = &mut mc.skeleton {
                s.poses = library.clone();
            }
            self.set_rig(mc, "Save pose")?;
            said.push(format!("saved as \"{name}\""));
        }
        let mut copied = None;
        if op.get("copy").and_then(Value::as_bool).unwrap_or(false) {
            let mc = self.model_clip(id).ok_or("clip gone")?;
            let (now, _) = Self::posed_at(&mc, &vc, at).ok_or("no skeleton")?;
            copied = Some(serde_json::to_value(now.pose_now("copied")).unwrap_or_default());
            said.push("copied".into());
        }
        if said.is_empty() {
            return Err("pose does nothing without bones ({name: degrees}), offset, apply, paste, save, copy, mirror, reset or keys".into());
        }
        Ok(format!("{}{}{}", said.join("; "), copied.map_or(String::new(), |c| format!("; pose: {c} (paste it with \"paste\")")), self.rig_said(id, at)))
    }

    /// `ik`: a hand or foot to a point, the limb following.
    pub(super) fn ik(&mut self, op: &Value) -> Result<String, String> {
        let (id, mc, vc, at) = self.rig_target(op, "ik")?;
        let sk = mc.skeleton.clone().ok_or_else(|| format!("clip {id} has no skeleton: skeleton {{\"build\": kind}} first"))?;
        let map = self.rig_map(id, at)?;
        let posed = Self::posed_at(&mc, &vc, at);
        let handle = match op.get("handle") {
            Some(v) => Some(sk.ik.iter().find(|h| Some(h.id as u64) == v.as_u64() || v.as_str().is_some_and(|s| h.name.eq_ignore_ascii_case(s))).ok_or_else(|| format!("no IK handle {v}; handles: {}", sk.ik.iter().map(|h| format!("{} \"{}\"", h.id, h.name)).collect::<Vec<_>>().join(", ")))?.clone()),
            None => None,
        };
        let bone = match (op.get("bone"), &handle) {
            (Some(v), _) => Self::find_bone(&sk, v)?,
            (None, Some(h)) => h.bone,
            (None, None) => return Err("ik needs \"bone\" (the last bone of the limb: forearm.front, shin.back… or \"front hand\") or \"handle\"".into()),
        };
        let chain = op.get("chain").and_then(Value::as_u64).map(|c| c as u32).or(handle.as_ref().map(|h| h.chain)).unwrap_or(2);
        let flip = op.get("flip").and_then(Value::as_bool).or(handle.as_ref().map(|h| h.flip)).unwrap_or(false);
        let pin = op.get("pin").and_then(Value::as_bool);
        let extras = ["pole", "stretch", "soft"].iter().any(|k| op.get(*k).is_some()) || pin == Some(true);
        let live = op.get("live").and_then(Value::as_bool).unwrap_or(op.get("keys").is_some() || extras);
        let name = sk.bone(bone).map(|b| b.name.clone()).unwrap_or_default();
        if op.get("off").and_then(Value::as_bool).unwrap_or(false) {
            let mut mc = mc;
            if let Some(s) = &mut mc.skeleton {
                for h in s.ik.iter_mut().filter(|h| h.bone == bone) {
                    h.enabled = false;
                }
            }
            self.set_rig(mc, "IK off")?;
            return Ok(format!("IK on {name} off: the bones keep their own rotations{}", self.rig_said(id, at)));
        }
        let pt = |v: &Value| Self::rig_point(&map, posed.as_ref().map(|(s, p)| (s, p)), v);
        if !live {
            let to = op.get("to").ok_or("ik needs \"to\": where the hand or foot goes ([x, y], {u, v} or a bone's name)")?;
            let target = match op.get("by").and_then(Value::as_array) {
                Some(d) if d.len() == 2 => {
                    let tip = posed.as_ref().and_then(|(_, p)| p.bone(bone)).map(|b| b.tail).ok_or("no bone")?;
                    let c = map.canvas(tip);
                    pt(&json!([c.x as f64 + d[0].as_f64().unwrap_or(0.0), c.y as f64 + d[1].as_f64().unwrap_or(0.0)]))?
                }
                _ => pt(to)?,
            };
            let rot = ch::reach(&self.doc, id, bone, chain, target, flip, at)?;
            let key = op.get("key").and_then(Value::as_bool).unwrap_or(false);
            ch::pose(&mut self.doc, id, &rot, &[], at, key)?;
            let mc = self.model_clip(id).ok_or("clip gone")?;
            let tip = Self::posed_at(&mc, &vc, at).and_then(|(_, p)| p.bone(bone).map(|b| b.tail)).unwrap_or(target);
            let miss = (map.canvas(tip) - map.canvas(target)).length();
            return Ok(format!(
                "{name} reaches{}{} ({} bone(s) turned{}){}",
                if miss > 3.0 { format!(" toward the point, {miss:.0} px short (out of reach or held by joint limits)") } else { " the point".into() },
                if key { " as a key" } else { "" },
                rot.len(),
                if flip { ", bent the other way" } else { "" },
                self.rig_said(id, at)
            ));
        }
        // A live handle: the target is a property, keyed over time.
        let mut mc = mc;
        let sk = mc.skeleton.as_mut().expect("checked");
        let hid = match sk.ik.iter().find(|h| h.bone == bone) {
            Some(h) => h.id,
            None => {
                let target = sk.bone(bone).map_or(m::Vec2::ZERO, |b| b.tail);
                let hid = sk.next_ik_id();
                sk.ik.push(m::IkHandle { id: hid, name: name.clone(), bone, chain, target, blend: 1.0, flip, enabled: true, ..Default::default() });
                hid
            }
        };
        let pole = match op.get("pole") {
            Some(Value::Null | Value::Bool(false)) => Some(None),
            Some(v) => Some(Some(pt(v)?)),
            None => None,
        };
        let stretch = match op.get("stretch") {
            Some(Value::Bool(false) | Value::Null) => Some(1.0),
            Some(Value::Bool(true)) => Some(1.5),
            Some(v) => Some(v.as_f64().ok_or("stretch is true, false or the longest factor of the chain's length (1.5)")?.clamp(1.0, 4.0)),
            None => None,
        };
        let soft = f(op, "soft").map(|v| v.clamp(0.0, 0.5));
        if let Some(h) = sk.handle_mut(hid) {
            h.enabled = pin != Some(false);
            h.chain = chain;
            h.flip = flip;
            if let Some(p) = pole {
                h.pole = p;
            }
            if let Some(k) = stretch {
                h.stretch = k;
            }
            if let Some(v) = soft {
                h.soft = v;
            }
        }
        let local = at - vc.start;
        let frame = (SEC as f64 / (self.doc.project.fps as f64).max(1.0)) as view::Time;
        if pin == Some(true) && op.get("to").is_none() {
            // Pinned: the end stays where it is now while the body moves.
            let tip = posed.as_ref().and_then(|(_, p)| p.bone(bone)).map(|b| b.tail).ok_or("no bone")?;
            crate::editor::xform::put(&mut mc, &format!("ik.{hid}.target"), m::Value::Point(tip), local, frame);
        }
        if let Some(to) = op.get("to") {
            let target = pt(to)?;
            crate::editor::xform::put(&mut mc, &format!("ik.{hid}.target"), m::Value::Point(target), local, frame);
        }
        if let Some(Value::Array(keys)) = op.get("keys") {
            let mut ks = Vec::new();
            for k in keys {
                let (t, v) = match k {
                    Value::Array(a) if a.len() == 2 => (a[0].as_f64().ok_or("a key's time is seconds")?, a[1].clone()),
                    Value::Object(o) => (o.get("t").and_then(Value::as_f64).ok_or("each key needs \"t\"")?, o.get("to").cloned().ok_or("each key needs \"to\"")?),
                    _ => return Err("keys are [[t, point], …] (t seconds from the clip's start)".into()),
                };
                let map_t = self.rig_map(id, vc.start + micros(t)).unwrap_or_else(|_| map.clone());
                let target = Self::rig_point(&map_t, posed.as_ref().map(|(s, p)| (s, p)), &v)?;
                ks.push(m::Keyframe::new(micros(t).clamp(0, vc.dur), m::Value::Point(target), m::Interp::Ease));
            }
            mc.keyframes.insert(format!("ik.{hid}.target"), m::Track::new(ks));
        }
        if let Some(b) = f(op, "blend") {
            crate::editor::xform::put(&mut mc, &format!("ik.{hid}.blend"), m::Value::Scalar(b.clamp(0.0, 1.0)), local, frame);
        }
        self.set_rig(mc, "IK")?;
        let sk_now = self.model_clip(id).and_then(|c| c.skeleton).and_then(|s| s.handle(hid).cloned());
        let extra = sk_now.map_or(String::new(), |h| {
            let mut v = Vec::new();
            if h.pole.is_some() {
                v.push("bends toward its pole".to_owned());
            }
            if h.stretch > 1.0 {
                v.push(format!("stretches up to {:.2}x", h.stretch));
            }
            if h.soft > 0.0 {
                v.push(format!("eases into straight over the last {:.0}% of its reach", h.soft * 100.0));
            }
            if pin == Some(true) {
                v.push(format!("pinned where it is at {:.2}s (keep the body moving: it stays)", secs(at)));
            }
            if v.is_empty() { String::new() } else { format!("; {}", v.join(", ")) }
        });
        Ok(format!("IK handle {hid} on {name} ({chain} bones) is live: the limb reaches for its target as it moves{extra}{}", self.rig_said(id, at)))
    }

    /// `aim`: a bone (a head, an eye, a gun arm) turns to point at a spot
    /// or another bone: a one-bone IK handle, live when keyed.
    pub(super) fn aim(&mut self, op: &Value) -> Result<String, String> {
        let mut o = op.clone();
        let map = o.as_object_mut().ok_or("aim needs an object")?;
        if !map.contains_key("bone") {
            return Err("aim needs \"bone\" (the bone that turns: head, …) and \"to\" (a point, or a bone's name)".into());
        }
        map.insert("chain".into(), json!(1));
        if let Some(limit) = map.remove("limit").and_then(|v| v.as_f64()) {
            let (id, mc, _, _) = self.rig_target(op, "aim")?;
            let sk = mc.skeleton.as_ref().ok_or_else(|| format!("clip {id} has no skeleton"))?;
            let bone = Self::find_bone(sk, &op["bone"])?;
            let mut mc = mc;
            if let Some(b) = mc.skeleton.as_mut().and_then(|s| s.bone_mut(bone)) {
                b.min = Some(-limit.abs());
                b.max = Some(limit.abs());
            }
            self.set_rig(mc, "Aim limit")?;
        }
        self.ik(&o)
    }

    /// `retarget`: the animation of one rig onto another.
    pub(super) fn retarget(&mut self, op: &Value) -> Result<String, String> {
        let from = op.get("from").and_then(Value::as_u64).ok_or("retarget needs \"from\" (the animated rig clip) and \"clip\" (the rig that takes the motion)")? as u32;
        let to = op.get("clip").and_then(Value::as_u64).ok_or("retarget needs \"clip\": the rig that takes the motion")? as u32;
        self.clip(from)?;
        self.clip(to)?;
        let map: Vec<(String, String)> = match op.get("map") {
            Some(Value::Object(o)) => o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect(),
            Some(_) => return Err("map is {\"target bone\": \"source bone\"}".into()),
            None => Vec::new(),
        };
        let done = ch::retarget(&mut self.doc, from, to, f(op, "start").unwrap_or(0.0).max(0.0), f(op, "duration"), &map)?;
        Ok(format!(
            "clip {from}'s motion now drives clip {to}: {} bone(s) keyed ({}), {} keys, each bone pointing the way its partner does and the travel scaled to the leg length. The old keys of those bones were replaced",
            done.bones.len(),
            done.bones.join(", "),
            done.keys
        ))
    }

    /// `parts`: the picture split into part layers on its bones.
    pub(super) fn parts(&mut self, op: &Value) -> Result<String, String> {
        let (id, _, _, at) = self.rig_target(op, "parts")?;
        let face = op.get("face").and_then(Value::as_bool).unwrap_or(true);
        let (made, notes) = ch::split_layers_noted(&mut self.doc, id, at, face)?;
        Ok(format!(
            "split into {} part layers, each on its own lane above clip {id} and following its bone: {}; clip {id} is now the hidden rig (opacity 0) that carries them: pose, act and ik on it move the parts. The face's layers are swap sets (swap) and are named for lip sync and blinks. {}. Check the cut with parts_check (original, parts and error side by side); polygon_mask draws a part's outline by hand in source pixels{}",
            made.len(),
            made.iter().map(|(c, n)| format!("{c} {n}")).collect::<Vec<_>>().join(", "),
            notes.join(". "),
            self.rig_said(id, at)
        ))
    }

    /// `swap`: swap sets on a rig (mouths, hands, eyes).
    pub(super) fn swap(&mut self, op: &Value) -> Result<String, String> {
        let (id, mc, vc, at) = self.rig_target(op, "swap")?;
        let set = op.get("set").and_then(Value::as_str).ok_or("swap needs \"set\" (mouth, eye.front, hand.front, …)")?.to_lowercase();
        let mut said = Vec::new();
        if let Some(Value::Object(add)) = op.get("add") {
            let sk = mc.skeleton.clone().ok_or_else(|| format!("clip {id} has no skeleton to hang the options on"))?;
            let bone = match op.get("bone") {
                Some(v) => Self::find_bone(&sk, v)?,
                None => {
                    let want = if set.starts_with("mouth") || set.starts_with("eye") || set.starts_with("brow") { "head" } else { set.trim_start_matches("hand.") };
                    let guess = if set.starts_with("hand") { sk.bones.iter().find(|b| b.name.starts_with("forearm") && b.name.contains(if want.contains("back") { "back" } else { "front" })) } else { sk.named(want) };
                    guess.or(sk.bones.first()).map(|b| b.id).ok_or("the skeleton has no bones")?
                }
            };
            let map = self.rig_map(id, at)?;
            let posed = Self::posed_at(&mc, &vc, at);
            let centre = match op.get("place") {
                Some(v) => Some(Self::rig_point(&map, posed.as_ref().map(|(s, p)| (s, p)), v)?),
                None => None,
            };
            for (option, pic) in add {
                let path = match pic {
                    Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => std::path::PathBuf::from(s),
                    v => self.media_item(v)?.path.ok_or("that media has no file")?,
                };
                let cid = ch::add_swap(&mut self.doc, id, &set, option, &path, bone, centre, f(op, "width"), at)?;
                said.push(format!("{option} = clip {cid}"));
            }
        }
        if let Some(Value::Array(keys)) = op.get("keys") {
            let mut ks = Vec::new();
            for k in keys {
                let (t, o) = match k {
                    Value::Array(a) if a.len() == 2 => (a[0].as_f64().ok_or("a key's time is seconds")?, a[1].as_str().ok_or("a key is [t, option]")?.to_owned()),
                    Value::Object(o) => (o.get("t").and_then(Value::as_f64).ok_or("each key needs \"t\"")?, o.get("option").and_then(Value::as_str).ok_or("each key needs \"option\"")?.to_owned()),
                    _ => return Err("keys are [[t, option], …] (t seconds from the rig clip's start)".into()),
                };
                ks.push((secs(vc.start) + t, o));
            }
            ks.sort_by(|a, b| a.0.total_cmp(&b.0));
            ch::swap(&mut self.doc, id, &set, &ks)?;
            said.push(format!("{} switch(es) of {set}", ks.len()));
        }
        if let Some(o) = op.get("show").and_then(Value::as_str) {
            let t = secs(at);
            ch::swap(&mut self.doc, id, &set, &[(t, o.to_owned())])?;
            said.push(format!("{set} shows {o} from {t:.2}s"));
        }
        let sets = ch::swap_sets(&self.doc, id);
        if said.is_empty() && op.get("list").is_none() {
            return Err("swap does nothing without add ({option: picture}), keys ([[t, option]…]), show (an option) or list".into());
        }
        Ok(format!("{}{}swap sets: {}", said.join("; "), if said.is_empty() { "" } else { "; " }, json!(sets.into_iter().map(|(s, o)| (s, json!(o))).collect::<serde_json::Map<_, _>>())))
    }

    /// `mocap`: a filmed person drives a skeleton.
    #[cfg(feature = "render")]
    pub(super) fn mocap(&mut self, op: &Value) -> Result<String, String> {
        use crate::services::mocap;
        let (id, mc, vc, at) = self.rig_target(op, "mocap")?;
        let sk = mc.skeleton.clone().ok_or_else(|| format!("clip {id} has no skeleton: skeleton {{\"build\": kind}} first"))?;
        let video = op.get("video").ok_or("mocap needs \"video\": a video clip id, media, or a file path of a person moving")?;
        // The footage and the stretch of it to read.
        let (path, from, len) = match video {
            Value::Number(n) if self.clip(n.as_u64().unwrap_or(0) as u32).is_ok() => {
                let vcid = n.as_u64().unwrap_or(0) as u32;
                let vmc = self.model_clip(vcid).ok_or("clip gone")?;
                let foot = crate::services::tracking::footage(&self.doc.project, &vmc)?;
                let a = vmc.source_time_at(vmc.start) as f64 / SEC as f64;
                (foot.path, a, vmc.duration as f64 / SEC as f64)
            }
            Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => (std::path::PathBuf::from(s), 0.0, f64::INFINITY),
            v => {
                let mi = self.media_item(v)?;
                (mi.path.clone().ok_or("that media has no file")?, 0.0, secs(mi.dur))
            }
        };
        let from = f(op, "from").unwrap_or(from);
        let room = secs(vc.start + vc.dur - at);
        let dur = f(op, "duration").unwrap_or(len).min(room);
        if dur < 0.3 {
            return Err(format!("mocap needs at least a third of a second ({dur:.2}s fits clip {id} from {:.2}s)", secs(at)));
        }
        let rate = f(op, "rate").unwrap_or(15.0).clamp(4.0, 60.0);
        let cap = mocap::capture(&path, from, dur, rate, &mut |_| true)?;
        let mut o = mocap::Options { smooth: f(op, "smooth").unwrap_or(0.5), mirror: op.get("mirror").and_then(Value::as_bool).unwrap_or(false), swap_sides: op.get("swap_sides").and_then(Value::as_bool).unwrap_or(false), strength: f(op, "strength").unwrap_or(1.0).clamp(0.0, 2.0), root: op.get("root").and_then(Value::as_str).unwrap_or("full").to_owned(), ..Default::default() };
        if let Some(Value::Object(map)) = op.get("map") {
            for (k, v) in map {
                o.map.insert(k.clone(), v.as_str().ok_or("map is {bone: limb}")?.to_owned());
            }
        }
        // The rest pose is what the capture retargets onto.
        let mut rest = sk.clone();
        for b in &mut rest.bones {
            b.rotation = 0.0;
            b.offset = m::Vec2::ZERO;
        }
        let start = secs(at - vc.start);
        let r = mocap::retarget(&cap, &rest, &o, start)?;
        let end = start + cap.times.last().copied().unwrap_or(0.0);
        let mut mc = self.model_clip(id).ok_or("clip gone")?;
        let local = |s: f64| micros(s).clamp(0, vc.dur);
        let mut write = |path: String, keys: Vec<(f64, m::Value)>| {
            let mut tr = mc.keyframes.remove(&path).unwrap_or_else(|| m::Track::new(Vec::new()));
            tr.keys.retain(|k| k.time < local(start) - 1000 || k.time > local(end) + 1000);
            for (t, v) in keys {
                tr.set_key(m::Keyframe::new(local(t), v, m::Interp::Linear));
            }
            mc.keyframes.insert(path, tr);
        };
        for (b, ks) in &r.rotations {
            write(format!("bone.{b}.rotation"), ks.iter().map(|(t, v)| (*t, m::Value::Angle(*v))).collect());
        }
        if let (Some(root), false) = (r.root, r.offsets.is_empty()) {
            write(format!("bone.{root}.offset"), r.offsets.iter().map(|(t, v)| (*t, m::Value::Point(*v))).collect());
        }
        if let Some(s) = &mut mc.skeleton {
            for h in &mut s.ik {
                h.enabled = h.enabled && !r.rotations.contains_key(&h.bone);
            }
        }
        self.set_rig(mc, "Motion capture")?;
        Ok(format!(
            "captured {} of {} frames with a person ({:.1}s at {rate:.0} fps) onto clip {id} from {:.2}s: {}{}",
            r.found.0,
            r.found.1,
            cap.times.last().copied().unwrap_or(0.0),
            secs(at),
            r.driven.iter().map(|(b, how, limb)| format!("{b} ← {limb} ({how})")).collect::<Vec<_>>().join(", "),
            if r.found.0 * 2 < r.found.1 { ". Under half the frames found a person: film the whole body, side on or facing the camera, in good light" } else { "" }
        ))
    }

    /// A pose from the project's character library ("Mia/wave").
    fn library_pose(&self, name: &str) -> Option<m::Pose> {
        let (who, pose) = name.split_once('/')?;
        let home = self.cast_home().ok()?;
        crate::services::cast::load(&home, who).ok()?.poses.into_iter().find(|p| p.name.eq_ignore_ascii_case(pose))
    }

    /// The folder the character library lives in: the project's.
    fn cast_home(&self) -> Result<std::path::PathBuf, String> {
        self.doc.folder.as_ref().map(|f| f.root().to_path_buf()).or_else(|| self.home.clone()).ok_or_else(|| "the character library lives in the project's folder: save the project first".into())
    }

    /// `character`: the project's character library.
    pub(super) fn character(&mut self, op: &Value) -> Result<String, String> {
        use crate::services::cast;
        let home = self.cast_home()?;
        if op.get("list").and_then(Value::as_bool).unwrap_or(false) {
            let names = cast::names(&home).unwrap_or_default();
            return Ok(if names.is_empty() { "the character library is empty: character {\"create\": name, \"from\": a picture or clip, \"description\": its look}".into() } else { format!("characters: {}", names.join(", ")) });
        }
        let name = op.get("create").or_else(|| op.get("name")).and_then(Value::as_str).ok_or("character needs \"name\" (or \"create\": a new name, or \"list\": true)")?.to_owned();
        let mut said = Vec::new();
        let mut c = if op.get("create").is_some() {
            if cast::load(&home, &name).is_ok() {
                return Err(format!("there is a character \"{name}\" already: use \"name\" to change it"));
            }
            let from = op.get("from").ok_or("create needs \"from\": the reference picture (a clip id, media or a file)")?;
            let (path, clip) = match from {
                Value::Number(n) if self.model_clip(n.as_u64().unwrap_or(0) as u32).is_some() => {
                    let mc = self.model_clip(n.as_u64().unwrap_or(0) as u32).expect("checked");
                    (ch::picture_path(&self.doc, &mc)?, Some(mc))
                }
                Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => (std::path::PathBuf::from(s), None),
                v => (self.media_item(v)?.path.ok_or("that media has no file")?, None),
            };
            let mut c = cast::Character { name: name.clone(), kind: op.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned(), description: op.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(), ..Default::default() };
            c.reference = cast::add_picture(&home, &mut c, "view", "front", &path)?;
            if let Some(sk) = clip.and_then(|mc| mc.skeleton) {
                c.poses = sk.poses.clone();
                c.skeleton = Some(sk);
            }
            said.push(format!("{name} is in the library{}", if c.skeleton.is_some() { " with its rig" } else { "" }));
            c
        } else {
            cast::load(&home, &name)?
        };
        if let Some(d) = op.get("description").and_then(Value::as_str).filter(|_| op.get("create").is_none()) {
            c.description = d.to_owned();
            said.push("description kept".into());
        }
        if let Some(cid) = op.get("save_rig").and_then(Value::as_u64) {
            let mc = self.model_clip(cid as u32).ok_or_else(|| format!("no clip {cid}"))?;
            let sk = mc.skeleton.ok_or_else(|| format!("clip {cid} has no skeleton"))?;
            c.poses = sk.poses.clone();
            c.skeleton = Some(sk);
            said.push(format!("rig of clip {cid} saved (with {} pose(s))", c.poses.len()));
        }
        if let Some(Value::Object(add)) = op.get("add") {
            let kind = add.get("kind").and_then(Value::as_str).ok_or_else(|| format!("add needs \"kind\": {}", cast::KINDS))?;
            let what = add.get("name").and_then(Value::as_str).ok_or("add needs \"name\"")?;
            let pic = add.get("picture").ok_or("add needs \"picture\"")?;
            let path = match pic {
                Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => std::path::PathBuf::from(s),
                v => self.media_item(v)?.path.ok_or("that media has no file")?,
            };
            cast::add_picture(&home, &mut c, kind, what, &path)?;
            said.push(format!("{kind} \"{what}\" added"));
        }
        // New pictures made from the reference, so they keep its look.
        if let Some(g) = op.get("generate") {
            let mut wanted: Vec<(String, String)> = Vec::new();
            match g {
                Value::Object(o) => {
                    for (key, kind) in [("views", "view"), ("poses", "pose"), ("expressions", "expression"), ("mouths", "mouth"), ("hands", "hand"), ("eyes", "eyes")] {
                        if let Some(Value::Array(l)) = o.get(key) {
                            wanted.extend(l.iter().filter_map(Value::as_str).map(|n| (kind.to_owned(), n.to_owned())));
                        }
                    }
                }
                Value::String(s) if s == "turnaround" => wanted.extend(["side", "back", "three_quarter"].map(|n| ("view".to_owned(), n.to_owned()))),
                _ => return Err("generate is \"turnaround\" or {\"views\": [side, back, three_quarter], \"poses\": […], \"expressions\": […], \"mouths\": [closed, open, wide, round], \"hands\": […]}".into()),
            }
            let reference = cast::path_of(&home, &c, &c.reference);
            let estimate = op.get("estimate").and_then(Value::as_bool).unwrap_or(false);
            let mut made = Vec::new();
            for (kind, what) in &wanted {
                let mut ask = json!({"make": "picture", "prompt": cast::prompt(&c, kind, what), "images": [reference.display().to_string()], "transparent": true, "name": format!("{}-{kind}-{what}", c.name), "estimate": estimate});
                for k in ["provider", "aspect"] {
                    if let Some(v) = op.get(k) {
                        ask[k] = v.clone();
                    }
                }
                let v = self.generate(&ask)?;
                if estimate {
                    made.push(format!("{kind} {what}"));
                    continue;
                }
                let file = v["files"].as_array().and_then(|f| f.first()).and_then(Value::as_str).ok_or_else(|| format!("no picture came back for {kind} {what}"))?;
                cast::add_picture(&home, &mut c, kind, what, std::path::Path::new(file))?;
                made.push(format!("{kind} {what}"));
            }
            said.push(if estimate { format!("would make (nothing sent): {}", made.join(", ")) } else { format!("made from the reference: {}", made.join(", ")) });
        }
        cast::save(&home, &c)?;
        // A picture of the character onto the timeline.
        if let Some(u) = op.get("use") {
            let what = u.as_str().unwrap_or("front");
            let pic = c.pictures.iter().find(|p| p.name.eq_ignore_ascii_case(what) || format!("{}/{}", p.kind, p.name).eq_ignore_ascii_case(what)).cloned().ok_or_else(|| format!("{name} has no picture \"{what}\"; pictures: {}", c.pictures.iter().map(|p| format!("{}/{}", p.kind, p.name)).collect::<Vec<_>>().join(", ")))?;
            let path = cast::path_of(&home, &c, &pic.file);
            let report = crate::services::project::import_media(&mut self.doc, &[path]);
            let media = *report.ids.first().ok_or_else(|| report.failed.join("; "))?;
            let before: Vec<u32> = self.doc.project.clips.iter().map(|x| x.id).collect();
            let mut place = json!({"media": media, "lane": op.get("lane").cloned().unwrap_or(json!("overlay")), "name": format!("{name} {}", pic.name)});
            for k in ["at", "duration", "x", "y"] {
                if let Some(v) = op.get(k) {
                    place[k] = v.clone();
                }
            }
            self.place(&place)?;
            let id = self.doc.project.clips.iter().map(|x| x.id).find(|i| !before.contains(i)).ok_or("the picture was not placed")?;
            // The rig goes on with the picture it was drawn on.
            let mut rigged = false;
            if let (Some(sk), true) = (&c.skeleton, pic.file == c.reference) {
                let mut mc = self.model_clip(id).ok_or("clip gone")?;
                mc.skeleton = Some(sk.clone());
                self.set_rig(mc, "Character")?;
                rigged = true;
            }
            said.push(format!("clip {id}: {name} ({}){}", pic.name, if rigged { ", rigged" } else { "" }));
        }
        // Mouth pictures as a rig's mouth swap set.
        if let Some(rig) = op.get("mouths_on").and_then(Value::as_u64) {
            let mouths: Vec<cast::Picture> = c.pictures.iter().filter(|p| p.kind == "mouth").cloned().collect();
            if mouths.is_empty() {
                return Err(format!("{name} has no mouth pictures: generate {{\"mouths\": [closed, open, wide, round]}} first"));
            }
            let mut add = serde_json::Map::new();
            for p in &mouths {
                add.insert(p.name.clone(), json!(cast::path_of(&home, &c, &p.file).display().to_string()));
            }
            let mut sw = json!({"clip": rig, "set": "mouth", "add": add});
            for k in ["place", "width", "bone", "at"] {
                if let Some(v) = op.get(k) {
                    sw[k] = v.clone();
                }
            }
            said.push(self.swap(&sw)?);
        }
        if said.is_empty() {
            said.push(format!("{name}: {}", serde_json::to_string(&json!({"kind": c.kind, "description": c.description, "pictures": c.pictures.iter().map(|p| format!("{}/{}", p.kind, p.name)).collect::<Vec<_>>(), "rig": c.skeleton.as_ref().map(|s| s.bones.len()), "poses": c.poses.iter().map(|p| p.name.clone()).collect::<Vec<_>>()})).unwrap_or_default()));
        }
        Ok(said.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> (Session, u32) {
        let (doc, id) = crate::services::character::tests::figure_doc();
        let mut s = Session::new(doc);
        s.home = Some(std::env::temp_dir().join(format!("studio-tests/cast-session-{}-{id}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        (s, id)
    }

    /// What the op said (its notes as plain text).
    fn run(s: &mut Session, op: Value) -> Result<String, String> {
        s.edit(&[op], "t", false).map(|v| v["notes"].as_array().map(|n| n.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default())
    }

    #[test]
    fn build_pose_reach_act_and_parts_through_ops() {
        let (mut s, id) = session();
        let r = run(&mut s, json!({"op": "skeleton", "clip": id, "build": "person", "facing": "right"})).unwrap();
        assert!(r.contains("thigh.front") && r.contains("\"tip\""), "{r}");
        let r = run(&mut s, json!({"op": "pose", "clip": id, "bones": {"upper_arm.front": 60}, "save": "reach", "copy": true})).unwrap();
        assert!(r.contains("saved") && r.contains("rotations"), "{r}");
        let r = run(&mut s, json!({"op": "pose", "clip": id, "reset": true})).unwrap();
        assert!(r.contains("rest pose"), "{r}");
        let r = run(&mut s, json!({"op": "pose", "clip": id, "apply": "reach", "key": true})).unwrap();
        assert!(r.contains("as keys"), "{r}");
        let r = run(&mut s, json!({"op": "pose", "clip": id, "keys": [{"t": 0.5, "bones": {"head": 20}}, {"t": 1.5, "bones": {"head": -20}}]})).unwrap();
        assert!(r.contains("2 pose keys"), "{r}");
        let r = run(&mut s, json!({"op": "ik", "clip": id, "bone": "front foot", "to": {"u": 0.75, "v": 0.85}})).unwrap();
        assert!(r.contains("reaches"), "{r}");
        let r = run(&mut s, json!({"op": "ik", "clip": id, "bone": "forearm.front", "keys": [[0, {"u": 0.9, "v": 0.3}], [1, {"u": 0.7, "v": 0.05}]]})).unwrap();
        assert!(r.contains("live"), "{r}");
        let r = run(&mut s, json!({"op": "ik", "clip": id, "bone": "front foot", "pole": [60, 0], "stretch": 1.4, "soft": 0.1})).unwrap();
        assert!(r.contains("pole") && r.contains("stretches up to 1.40x") && r.contains("eases"), "{r}");
        let r = run(&mut s, json!({"op": "ik", "clip": id, "bone": "forearm.back", "pin": true})).unwrap();
        assert!(r.contains("pinned"), "{r}");
        let r = run(&mut s, json!({"op": "aim", "clip": id, "bone": "head", "to": "front hand", "limit": 40, "key": true})).unwrap();
        assert!(r.contains("live"), "{r}");
        let r = run(&mut s, json!({"op": "skeleton", "clip": id, "act": "walk", "travel": "auto"})).unwrap();
        assert!(r.contains("travels"), "{r}");
        let r = run(&mut s, json!({"op": "retarget", "clip": id, "from": id, "duration": 1.0})).unwrap();
        assert!(r.contains("now drives"), "{r}");
        let r = run(&mut s, json!({"op": "parts", "clip": id})).unwrap();
        assert!(r.contains("mouth") && r.contains("hidden rig"), "{r}");
        let r = run(&mut s, json!({"op": "swap", "clip": id, "set": "mouth", "list": true})).unwrap();
        assert!(r.contains("closed"), "{r}");
        assert!(run(&mut s, json!({"op": "swap", "clip": id, "set": "mouth", "keys": [[0.5, "open"]]})).is_err());
        assert!(run(&mut s, json!({"op": "skeleton", "clip": id, "build": "cloud"})).is_err());
        assert!(run(&mut s, json!({"op": "pose", "clip": id, "bones": {"wing": 4}})).is_err());
    }

    #[test]
    fn the_character_library_keeps_a_look_and_its_rig() {
        let (mut s, id) = session();
        run(&mut s, json!({"op": "skeleton", "clip": id, "build": "biped", "facing": "right"})).unwrap();
        run(&mut s, json!({"op": "pose", "clip": id, "bones": {"upper_arm.front": 120}, "save": "wave"})).unwrap();
        let r = s.character(&json!({"create": "Ada", "from": id, "kind": "biped", "description": "a girl in a red coat"})).unwrap();
        assert!(r.contains("with its rig"), "{r}");
        assert!(s.character(&json!({"create": "Ada", "from": id})).is_err());
        let r = s.character(&json!({"name": "Ada", "generate": "turnaround", "estimate": true})).unwrap();
        assert!(r.contains("would make") && r.contains("view side"), "{r}");
        let r = s.character(&json!({"name": "Ada", "use": "front", "at": 0.5, "duration": 2})).unwrap();
        assert!(r.contains("rigged"), "{r}");
        let placed = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let mc = s.model_clip(placed).unwrap();
        assert!(mc.skeleton.as_ref().is_some_and(|k| k.poses.iter().any(|p| p.name == "wave")));
        let r = run(&mut s, json!({"op": "pose", "clip": placed, "apply": "Ada/wave"})).unwrap();
        assert!(r.contains("pose \"Ada/wave\""), "{r}");
        assert!(s.character(&json!({"list": true})).unwrap().contains("Ada"));
        assert!(s.character(&json!({"name": "Ada", "mouths_on": id})).is_err(), "no mouths yet");
    }
}
