// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Character performance for any rigged 3D character: walking with
//! planted feet, hand and body poses, looks, reaches and acting beats,
//! the pose of a character in world space, and the critic's notes.
//!
//! - `act` with a gait (`walk`, `run`, `sneak`, `limp`, `trot`...) and a
//!   `route`, a target (`to`) or a `distance`: the body travels the path,
//!   its speed set by the stride, each foot put down and left there. The
//!   result is baked: an action for the bones, keys for the clip's place and
//!   turn. See `studio_mesh::perform::gait`.
//! - `pose` with `hand`, `body`, `beat`, `recipe`, `look_at` (eyes lead,
//!   head and chest follow) and `reach` (never stretches; leans, or steps
//!   with `step: true`): additive layers on the bones, so a look plays
//!   over a walk.
//! - `inspect` with `pose`: where bones, soles, hands and gaze are at a
//!   moment, which feet are down or sliding, the reach; or the findings
//!   over a span. The same rig evaluation the renderer draws from.
//! - The critic: `character_problems` for `review` and `check`.

use super::anim3d::{key, set_param, v3};
use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;
use studio_mesh::perform::body::{Body, Side};
use studio_mesh::perform::check::{self, Intent, Sample};
use studio_mesh::perform::gait::{self, Plan, Style, Waypoint};
use studio_mesh::perform::hands;
use studio_mesh::perform::pose as pz;
use studio_mesh::perform::query::{self, ClipXf};
use studio_mesh::perform::solve::Solver;
use studio_mesh::rig::{Action, BonePose, Rig};

/// Gaits the new locomotion plays (the older cycles stay for the rest).
fn is_gait(name: &str) -> bool {
    gait::GAITS.contains(&name) || ["stride", "amble", "creep", "sprint", "shuffle"].contains(&name)
}

/// Whether a clip key is part of a character's performance: bones, IK, looks,
/// shapes, the baked action and the 3D place and turn. The critic and the
/// reviews count all of these as acting.
pub(super) fn is_performance_key(path: &str) -> bool {
    ["transform.", "puppet.", "bone.", "ik.", "generator.bone.", "generator.ik.", "generator.look.", "generator.shape.", "generator.aim.", "generator.action", "generator.blend"].iter().any(|p| path.starts_with(p))
}

fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|x| x as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    }
}

fn point2(v: &Value) -> Option<[f32; 2]> {
    match v {
        Value::Array(a) if a.len() == 2 => Some([num(&a[0])?, num(&a[1])?]),
        Value::Object(o) => {
            if let Some(at) = o.get("at") {
                return point2(at);
            }
            Some([num(o.get("x")?)?, num(o.get("z")?)?])
        }
        _ => None,
    }
}

fn style_of(op: &Value) -> Result<Style, String> {
    let mut s = Style::default();
    let words = |s: &mut Style, w: &str| {
        for word in w.split(|c: char| !c.is_alphabetic()).map(str::to_lowercase) {
            match word.as_str() {
                "weary" | "tired" | "exhausted" | "old" => s.weary = 1.0,
                "brisk" | "quick" | "lively" | "energetic" | "spry" => s.brisk = 1.0,
                "cautious" | "careful" | "wary" | "nervous" => s.cautious = 1.0,
                "heavy" | "hefty" | "lumbering" => s.heavy = 1.0,
                _ => {}
            }
        }
    };
    match op.get("style") {
        Some(Value::String(w)) => words(&mut s, w),
        Some(Value::Array(a)) => {
            for w in a.iter().filter_map(Value::as_str) {
                words(&mut s, w);
            }
        }
        Some(Value::Object(o)) => {
            for (k, v) in o {
                let x = num(v).unwrap_or(0.0).clamp(0.0, 1.0);
                match k.as_str() {
                    "weary" => s.weary = x,
                    "brisk" => s.brisk = x,
                    "cautious" => s.cautious = x,
                    "heavy" => s.heavy = x,
                    "energy" => s.energy = num(v).unwrap_or(1.0),
                    "limp" => s.limp = side_of(v),
                    other => return Err(format!("style \"{other}\": weary, brisk, cautious, heavy (each 0..1), energy (around 1), limp (\"L\" or \"R\")")),
                }
            }
        }
        _ => {}
    }
    if let Some(e) = op.get("energy").and_then(num) {
        s.energy = e;
    }
    if let Some(l) = op.get("limp") {
        s.limp = side_of(l).or(Some(Side::L)).filter(|_| l.as_bool() != Some(false));
    }
    Ok(s)
}

fn side_of(v: &Value) -> Option<Side> {
    match v.as_str()?.to_lowercase().as_str() {
        "l" | "left" => Some(Side::L),
        "r" | "right" => Some(Side::R),
        _ => None,
    }
}

/// Ground heights (px up from the level the character starts on) at world
/// points, from a number, scene models or a world.
type Ground = Box<dyn Fn(f32, f32) -> f32>;

struct Surface {
    bvh: studio_mesh::bvh::Bvh,
    pos: [f32; 3],
    k: f32,
    yaw: f32,
}

impl Surface {
    /// The height up of the surface under (x, z) below `from_up`.
    fn height(&self, x: f32, z: f32, from_up: f32) -> Option<f32> {
        let (s, c) = self.yaw.to_radians().sin_cos();
        let (dx, dz) = (x - self.pos[0], z - self.pos[2]);
        let (lx, lz) = ((dx * c + dz * s) / self.k, (-dx * s + dz * c) / self.k);
        let oy = (-from_up - self.pos[1]) / self.k;
        let hit = self.bvh.ray([lx, oy, lz], [0.0, 1.0, 0.0], 0.0, 1e7)?;
        Some(-(self.pos[1] + self.k * (oy + hit.t)))
    }
}

/// Which clip's mesh files hold a rig (so are characters, not ground).
fn is_character_file(file: &str) -> bool {
    rig_cache(file).is_some_and(|r| !r.bones.is_empty())
}

fn rig_cache(file: &str) -> Option<Arc<Rig>> {
    static C: OnceLock<Mutex<HashMap<String, Option<Arc<Rig>>>>> = OnceLock::new();
    let cache = C.get_or_init(Default::default);
    if let Some(r) = cache.lock().ok()?.get(file) {
        return r.clone();
    }
    let rig = studio_mesh::io::load(std::path::Path::new(file)).ok().and_then(|m| m.rig.clone());
    let mut c = cache.lock().ok()?;
    if c.len() > 300 {
        c.clear();
    }
    c.insert(file.to_owned(), rig.clone());
    rig
}

impl Session {
    /// The model clip's own placement now, as the renderer's world needs it.
    fn clip_xf(mc: &m::Clip, rel: m::Micros) -> ClipXf {
        let tr = mc.transform_at(rel.clamp(0, mc.duration.max(0)));
        ClipXf { pos: [tr.position.x as f32, tr.position.y as f32, tr.position_z as f32], yaw_deg: tr.rotation_y as f32, scale: (tr.scale_x as f32).abs().max(1e-3) }
    }

    /// The ground for a walk: `ground` as a number (flat), `"scene"` or
    /// `{"clips": [...]}` (the models of the scene to stand on), or `{"world":
    /// clip, "metres_per_unit": 0.01}` (a world's terrain and floors).
    fn ground_for(&self, op: &Value, me: u32, start: [f32; 2], start_up: f32, ell: f32) -> Result<(Ground, Option<String>), String> {
        let flat = || -> Ground { Box::new(|_, _| 0.0) };
        let g = op.get("ground");
        let (clip_ids, world): (Option<Vec<u32>>, Option<(u32, f32)>) = match g {
            None | Some(Value::Null) | Some(Value::Number(_)) | Some(Value::Bool(_)) => (None, None),
            Some(Value::String(s)) if s.eq_ignore_ascii_case("scene") => {
                let ids: Vec<u32> = self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).filter(|c| c.id.0 as u32 != me).filter_map(|c| crate::mesh_store::file_of(c).map(|f| (c.id.0 as u32, f))).filter(|(_, f)| !is_character_file(f)).map(|(i, _)| i).collect();
                (Some(ids), None)
            }
            Some(Value::String(s)) if s.eq_ignore_ascii_case("flat") => (None, None),
            Some(Value::Array(a)) => (Some(a.iter().filter_map(super::craft::as_id).map(|x| x as u32).collect()), None),
            Some(Value::Object(o)) => {
                if let Some(w) = o.get("world").and_then(Value::as_u64) {
                    (None, Some((w as u32, o.get("metres_per_unit").and_then(num).unwrap_or(0.01))))
                } else if let Some(c) = o.get("clips") {
                    (Some(match c {
                        Value::Array(a) => a.iter().filter_map(super::craft::as_id).map(|x| x as u32).collect(),
                        x => super::craft::as_id(x).map(|v| vec![v as u32]).unwrap_or_default(),
                    }), None)
                } else {
                    return Err("ground: a number (flat), \"scene\", a list of clip ids to stand on, {\"clips\": [ids]} or {\"world\": clip, \"metres_per_unit\": 0.01}".into());
                }
            }
            _ => return Err("ground: a number (flat), \"scene\", a list of clip ids to stand on, {\"clips\": [ids]} or {\"world\": clip, \"metres_per_unit\": 0.01}".into()),
        };
        if let Some(ids) = clip_ids {
            let mut surfaces = Vec::new();
            for id in ids {
                if self.model_clip(id).is_none() {
                    return Err(format!("ground: no clip {id}"));
                }
                let Ok((mc, mesh)) = self.model_of(id) else { continue };
                if mesh.rig.is_some() {
                    continue;
                }
                let x = Self::clip_xf(&mc, 0);
                surfaces.push(Surface { bvh: studio_mesh::bvh::Bvh::of_mesh(&mesh), pos: x.pos, k: x.scale, yaw: x.yaw_deg });
            }
            if surfaces.is_empty() {
                return Ok((flat(), Some("ground: no models to stand on, so the ground is flat".into())));
            }
            let top = start_up + 3.0 * ell;
            let h0 = surfaces.iter().filter_map(|s| s.height(start[0], start[1], top)).fold(f32::NEG_INFINITY, f32::max);
            let h0 = if h0.is_finite() { h0 } else { start_up };
            return Ok((
                Box::new(move |x, z| {
                    let h = surfaces.iter().filter_map(|s| s.height(x, z, top)).fold(f32::NEG_INFINITY, f32::max);
                    if h.is_finite() { (h - h0).max(-3.0 * ell) } else { 0.0 }
                }),
                None,
            ));
        }
        if let Some((wid, mpu)) = world {
            let mc = self.model_clip(wid).ok_or_else(|| format!("ground: no clip {wid}"))?;
            let file = crate::editor::viewport::world::world_file_of(&mc).ok_or_else(|| format!("ground: clip {wid} is not a world"))?;
            let world = studio_render::world_scene::load(&file).ok_or_else(|| format!("the world file {file} could not be built"))?;
            let mpu = mpu.max(1e-6);
            let start_ground = world.ground_y(start[0] * mpu, start[1] * mpu);
            let at = move |x: f32, z: f32, near: f32| -> f32 {
                let (xm, zm) = (x * mpu, z * mpu);
                world.ground_at(xm, zm, near + 0.7).map_or_else(|| world.ground_y(xm, zm), |g| g.y)
            };
            let h0 = at(start[0], start[1], start_ground);
            return Ok((Box::new(move |x, z| (at(x, z, h0) - h0) / mpu), None));
        }
        Ok((flat(), None))
    }

    /// Where a target is on the ground, in the world: a point, or a clip.
    fn ground_point(&self, v: &Value, mc: &m::Clip, at: f64) -> Option<[f32; 2]> {
        if let Some(p) = point2(v) {
            return Some(p);
        }
        let id = v.get("clip").and_then(|c| super::sim3d::id_of(&self.doc, c))?;
        let oc = self.model_clip(id)?;
        let t = (mc.start + micros(at) - oc.start).clamp(0, oc.duration.max(0));
        let tr = oc.transform_at(t);
        Some([tr.position.x as f32, tr.position_z as f32])
    }

    /// The pose of the clip's action (or its blend) at clip time `t`
    /// seconds, as local poses; nothing else (bone keys are layers).
    pub(super) fn base_locals(mc: &m::Clip, rig: &Rig, t: f32) -> Vec<BonePose> {
        let n = rig.bones.len();
        let p = mc.generator_params_at(micros(t as f64));
        let text = |k: &str| match p.get(k) {
            Some(m::Value::Text(s)) => s.clone(),
            _ => String::new(),
        };
        let fl = |k: &str, d: f64| p.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
        let name = text("action");
        let mut local = vec![BonePose::default(); n];
        if !name.is_empty()
            && let Some(a) = rig.action(&name)
        {
            let at = (t as f64 - fl("action_start", 0.0)) * fl("action_speed", 1.0) + fl("action_offset", 0.0);
            let looped = !matches!(p.get("action_loop"), Some(m::Value::Bool(false)));
            local = a.sample(n, rig.shapes.len(), at as f32, looped).0;
        }
        let blend = text("blend_action");
        let k = fl("blend", 0.0) as f32;
        if !blend.is_empty()
            && k > 0.0
            && let Some(a) = rig.action(&blend)
        {
            let at = (t as f64 - fl("action_start", 0.0)) * fl("blend_speed", fl("action_speed", 1.0)) + fl("blend_offset", 0.0);
            let looped = !matches!(p.get("action_loop"), Some(m::Value::Bool(false)));
            let (q, _) = a.sample(n, rig.shapes.len(), at as f32, looped);
            for (l, o) in local.iter_mut().zip(&q) {
                *l = l.mix(o, k.min(1.0));
            }
        }
        local
    }

    // ------------------------------------------------------------- walking

    /// `act` with a gait: a walk with planted feet along a route. `None`
    /// when this `act` is something else (a cycle, an action, a capture).
    pub(super) fn act_walk(&mut self, op: &Value, id: u32, mc: &m::Clip, mesh: &Mesh, rig: &Arc<Rig>) -> Result<Option<String>, String> {
        let motion = op.get("motion").and_then(Value::as_str).map(|s| s.trim().to_lowercase());
        let wants_path = ["route", "to", "by", "distance", "face", "turn"].iter().any(|k| op.get(*k).is_some_and(|v| !(k == &"to" && v.is_string())));
        let gait_name = match (&motion, wants_path) {
            (Some(m), _) if is_gait(m) => m.clone(),
            (Some(m), true) if m == "turn" => "walk".into(),
            (None, true) if op.get("action").is_none() && op.get("mocap").is_none() => "walk".into(),
            _ => return Ok(None),
        };
        let body = Body::of(rig);
        if body.legs.is_empty() {
            if wants_path || op.get("ground").is_some() {
                return Err(format!("{}", body.missing_for_legs(rig).unwrap_or_default()));
            }
            return Ok(None);
        }
        if !body.upright() {
            return Ok(None);
        }
        let at = f(op, "at").unwrap_or(0.0).max(0.0);
        let rel = micros(at);
        let tr = mc.transform_at(rel);
        let k = (tr.scale_x as f32).abs().max(1e-3);
        let start = [tr.position.x as f32, tr.position_z as f32];
        let alpha = gait::heading_offset(&body);
        let heading0 = (tr.rotation_y as f32).to_radians() + alpha;
        let style = style_of(op)?;
        let g = gait::gait(&gait_name, &body, &style)?;
        let ell = check::leg_unit(rig, &body, k);
        let pace = f(op, "pace").or_else(|| f(op, "speed")).unwrap_or(1.0).clamp(0.1, 6.0) as f32;
        // Where it goes.
        let clip_secs = mc.duration as f64 / 1e6;
        let mut waypoints: Vec<Waypoint> = Vec::new();
        let near = f(op, "near").map(|x| x as f32);
        let wp = |p: [f32; 2], stop: bool, pause: f32, face: Option<f32>| Waypoint { x: p[0], z: p[1], stop, pause, face };
        let mut said: Vec<String> = Vec::new();
        let facing_dir = [heading0.sin(), -heading0.cos()];
        if let Some(route) = op.get("route").and_then(Value::as_array) {
            for (i, r) in route.iter().enumerate() {
                let p = self.ground_point(r, mc, at).ok_or_else(|| format!("route point {}: [x, z] (canvas px on the ground), {{\"at\": [x, z], \"pause\": s, \"face\": degrees}} or {{\"clip\": id}}", i + 1))?;
                let pause = r.get("pause").and_then(num).unwrap_or(0.0);
                let face = r.get("face").and_then(num).map(f32::to_radians);
                let stop = r.get("stop").and_then(Value::as_bool).unwrap_or(false);
                waypoints.push(wp(p, stop, pause, face));
            }
        } else if let Some(to) = op.get("to").filter(|v| !v.is_string()) {
            let mut p = self.ground_point(to, mc, at).ok_or("to: [x, z] (canvas px on the ground) or {\"clip\": id}")?;
            // Stop short of a thing (a person, a lamp): `near` px away along the way in.
            let stop_short = near.or_else(|| to.get("clip").map(|_| 0.9 * ell));
            if let Some(d) = stop_short {
                let v = [p[0] - start[0], p[1] - start[1]];
                let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
                if len > d + 1.0 {
                    p = [p[0] - v[0] / len * d, p[1] - v[1] / len * d];
                }
            }
            waypoints.push(wp(p, true, 0.0, None));
        } else if let Some(by) = op.get("by").and_then(point2) {
            waypoints.push(wp([start[0] + by[0], start[1] + by[1]], true, 0.0, None));
        } else if let Some(d) = f(op, "distance") {
            let d = d as f32;
            waypoints.push(wp([start[0] + facing_dir[0] * d, start[1] + facing_dir[1] * d], true, 0.0, None));
        } else if op.get("face").is_none() && op.get("turn").is_none() {
            // Just a gait: along the way it faces, for `duration` (or the rest of the clip).
            let dur = f(op, "duration").unwrap_or((clip_secs - at).min(30.0)).max(0.5) as f32;
            let d = g.speed * ell * pace * dur * 0.85;
            waypoints.push(wp([start[0] + facing_dir[0] * d, start[1] + facing_dir[1] * d], true, 0.0, None));
        } else {
            waypoints.push(wp(start, true, 0.0, None));
        }
        // Facing at the end: `face` (degrees, or toward a clip), or `turn` by degrees.
        let end_face = if let Some(fv) = op.get("face") {
            match fv {
                Value::Object(_) => {
                    let last = waypoints.last().map_or(start, |w| [w.x, w.z]);
                    self.ground_point(fv, mc, at).map(|p| (p[0] - last[0]).atan2(-(p[1] - last[1])))
                }
                v => num(v).map(f32::to_radians),
            }
        } else {
            op.get("turn").and_then(num).map(|d| heading0 + d.to_radians())
        };
        if let (Some(face), Some(last)) = (end_face, waypoints.last_mut()) {
            last.face = Some(face);
            last.stop = true;
        }
        if let Some(last) = waypoints.last_mut() {
            last.stop = true;
        }
        let (ground, ground_note) = self.ground_for(op, id, start, -(tr.position.y as f32 + k * body.sole_height(rig) * body.up[1].signum()), ell)?;
        if let Some(n) = ground_note {
            said.push(n);
        }
        let fps = self.doc.project.fps.max(1.0) as f32;
        let plan = Plan {
            start,
            heading: heading0,
            waypoints,
            speed: Some(g.speed * ell * pace),
            duration: f(op, "duration").filter(|_| op.get("route").is_some() || op.get("to").is_some() || op.get("by").is_some()).map(|d| d as f32),
            hold: f(op, "hold").unwrap_or(0.4).clamp(0.0, 10.0) as f32,
            lead: f(op, "lead").unwrap_or(0.2).clamp(0.0, 10.0) as f32,
        };
        let walk = gait::walk(rig, &body, &g, k, &plan, fps, &*ground)?;
        // Bake: one action for the bones from the clip's start to the end of the walk.
        let mut r = (**rig).clone();
        let start_frames = (at * fps as f64).round() as usize;
        let total = start_frames + walk.frames.len();
        let mut frames: Vec<Vec<BonePose>> = Vec::with_capacity(total);
        let had_perf = matches!(mc.generator_params_at(0).get("action"), Some(m::Value::Text(a)) if a.starts_with("perf."));
        for i in 0..start_frames {
            let t = i as f32 / fps;
            frames.push(if had_perf || mc.generator_params_at(0).get("action").is_some() { Self::base_locals(mc, rig, t) } else { vec![BonePose::default(); r.bones.len()] });
        }
        frames.extend(walk.frames.iter().map(|fr| fr.poses.clone()));
        let n = r.actions.iter().filter_map(|a| a.name.strip_prefix("perf.")).filter_map(|s| s.split('.').next_back()?.parse::<u32>().ok()).max().unwrap_or(0) + 1;
        let name = format!("perf.{}.{n}", g.name);
        r.actions.retain(|a| !a.name.starts_with("perf."));
        let mut action = Action::from_frames(name.clone(), fps, &frames);
        action.name = name.clone();
        r.actions.push(action);
        let mut mc = mc.clone();
        // The clip's place and turn follow the walk, frame by frame (thinned to what a line cannot stand in for).
        let t0 = at as f32;
        let times: Vec<f32> = walk.frames.iter().map(|fr| t0 + fr.t).collect();
        let y0 = tr.position.y as f32;
        let cols = [walk.frames.iter().map(|fr| fr.x).collect::<Vec<_>>(), walk.frames.iter().map(|fr| y0 - fr.ground).collect(), walk.frames.iter().map(|fr| fr.z).collect(), {
            // Unwrapped, so a long turn does not spin back.
            let mut yaw: Vec<f32> = walk.frames.iter().map(|fr| fr.yaw_deg).collect();
            let base = tr.rotation_y as f32;
            if let Some(first) = yaw.first().copied() {
                let shift = ((base - first) / 360.0).round() * 360.0;
                for y in &mut yaw {
                    *y += shift;
                }
            }
            yaw
        }];
        let keep = pz::keep_indices(&times, &cols, 0.04);
        let end = micros(t0 as f64 + walk.duration as f64);
        for path in ["transform.position", "transform.position_z", "transform.rotation_y"] {
            if let Some(tr) = mc.keyframes.get_mut(path) {
                tr.keys.retain(|k| k.time < rel || k.time > end);
            }
        }
        for &i in &keep {
            let t = micros(times[i] as f64);
            key(&mut mc, "transform.position", t, m::Value::Point(m::Vec2::new(cols[0][i] as f64, cols[1][i] as f64)), m::Interp::Linear);
            key(&mut mc, "transform.position_z", t, m::Value::Scalar(cols[2][i] as f64), m::Interp::Linear);
            key(&mut mc, "transform.rotation_y", t, m::Value::Angle(cols[3][i] as f64), m::Interp::Linear);
        }
        set_param(&mut mc, "action", m::Value::Text(name.clone()));
        set_param(&mut mc, "action_start", m::Value::Scalar(0.0));
        set_param(&mut mc, "action_speed", m::Value::Scalar(1.0));
        set_param(&mut mc, "action_loop", m::Value::Bool(false));
        if let m::ClipSource::Generator(gen_) = &mut mc.source {
            gen_.params.remove("action_offset");
        }
        mc.keyframes.remove("generator.blend");
        let last = walk.frames.last().map(|fr| (fr.x, fr.z)).unwrap_or((start[0], start[1]));
        let mut out = format!("clip {id}: {} with planted feet from {:.1} to {:.1} s: {:.0} px in {} steps, ends at ({:.0}, {:.0}) standing, feet apart ({} action frames, {} position keys)", g.name, at, at + walk.duration as f64, walk.distance, walk.steps, last.0, last.1, walk.frames.len(), keep.len());
        if end_face.is_some() {
            out.push_str(", turned as asked");
        }
        for n in walk.notes.iter().chain(said.iter()) {
            out.push_str(&format!("; {n}"));
        }
        let mut mesh = mesh.clone();
        mesh.rig = Some(Arc::new(r));
        self.save_rigged(mc, &mesh, "Walk")?;
        Ok(Some(out))
    }

    // ---------------------------------------------------------------- poses

    /// A target for a look or reach at clip time `t`: model px (`[x, y, z]`),
    /// another clip, or a world point (`{"world": [x, y, z]}`, canvas px).
    fn model_target(&self, mc: &m::Clip, v: &Value, t: f64) -> Result<V3, String> {
        if let Some(p) = v3(v) {
            return Ok(p);
        }
        if let Some(c) = v.get("clip") {
            let id = super::sim3d::id_of(&self.doc, c).ok_or("target: no such clip")?;
            let mut p = self.model_point_of(mc, id, t)?;
            if let Some(off) = v.get("offset").and_then(v3) {
                p = studio_mesh::add(p, off);
            }
            return Ok(p);
        }
        if let Some(w) = v.get("world").and_then(v3) {
            let x = Self::clip_xf(mc, micros(t));
            return Ok(x.model(w));
        }
        Err("a target is [x, y, z] (model px), {\"clip\": id} (another layer) or {\"world\": [x, y, z]} (canvas px)".into())
    }

    /// The rig, its roles and the clip, for a performance op.
    fn performer(&self, id: u32) -> Result<(m::Clip, Mesh, Arc<Rig>, Body), String> {
        let (mc, mesh, rig) = self.rig_of(id)?;
        let body = Body::of(&rig);
        Ok((mc, mesh, rig, body))
    }

    /// Writes layer channels as clip keys starting at `t0`, replacing what
    /// those channels hold from `t0` to the last new key; the first key
    /// starts from the value there already, so layers chain.
    fn write_layer(mc: &mut m::Clip, channels: &[pz::Channel], t0: f32, intents: &[(&str, Vec<(f32, f32)>)]) -> usize {
        let mut n = 0;
        for ch in channels {
            let path = format!("generator.bone.{}.{}", ch.bone, ch.axis);
            let angle = !ch.axis.starts_with('l');
            let end = ch.keys.last().map_or(t0, |k| k.0);
            let prev = mc.property_at(&path, micros(t0 as f64)).and_then(|v| v.as_f64()).unwrap_or(0.0);
            if let Some(tr) = mc.keyframes.get_mut(&path) {
                tr.keys.retain(|k| k.time < micros(t0 as f64) || k.time > micros(end as f64) + 1);
            }
            for (i, (t, v)) in ch.keys.iter().enumerate() {
                let value = if i == 0 && (*t - t0).abs() < 1e-4 { prev } else { *v as f64 };
                let mv = if angle { m::Value::Angle(value) } else { m::Value::Scalar(value) };
                key(mc, &path, micros(*t as f64), mv, m::Interp::Auto);
                n += 1;
            }
        }
        for (path, keys) in intents {
            for (t, v) in keys {
                key(mc, &format!("generator.{path}"), micros(*t as f64), m::Value::Scalar(*v as f64), m::Interp::Hold);
                n += 1;
            }
        }
        n
    }

    /// The times of a layer from `t0` to `t1` at `hz`.
    fn layer_times(t0: f32, t1: f32, hz: f32) -> Vec<f32> {
        let n = (((t1 - t0) * hz).ceil() as usize).max(2);
        (0..=n).map(|i| t0 + (t1 - t0) * i as f32 / n as f32).collect()
    }

    /// The new pose fields of one key of `pose`: hand, body, beat, recipe,
    /// look_at and a reach to a target. Returns the keys written and what to say.
    pub(super) fn pose_layers(&mut self, id: u32, k: &Value, t: f64, mc: &mut m::Clip, rig: &Arc<Rig>) -> Result<(usize, Vec<String>), String> {
        let body = Body::of(rig);
        let mut n = 0;
        let mut said: Vec<String> = Vec::new();
        let t0 = t as f32;
        let over = f(k, "over").or_else(|| f(k, "blend")).unwrap_or(0.35).clamp(0.05, 10.0) as f32;
        // `hold`: seconds the pose is held before it lets go (omitted: it stays).
        let hold = f(k, "hold").map(|h| h as f32);
        let release = f(k, "release").unwrap_or(over as f64).clamp(0.05, 10.0) as f32;
        let hz = 12.0;
        let mut snap = mc.clone();
        let envelope = |tt: f32| -> f32 {
            let up = pz::ramp(tt, t0, over);
            match hold {
                Some(h) => up * (1.0 - pz::ramp(tt, t0 + over + h, release)),
                None => up,
            }
        };
        let t_end = |dur: f32| -> f32 { t0 + over + hold.map_or(0.0, |h| h + release) + dur };
        // ---- hands
        if let Some(h) = k.get("hand") {
            let mut jobs: Vec<(Side, String, f32)> = Vec::new();
            let amount = k.get("amount").and_then(num).unwrap_or(1.0);
            match h {
                Value::String(p) => {
                    jobs.push((Side::L, p.clone(), amount));
                    jobs.push((Side::R, p.clone(), amount));
                }
                Value::Object(o) if o.contains_key("pose") => {
                    let p = o.get("pose").and_then(Value::as_str).unwrap_or("relaxed").to_owned();
                    let amt = o.get("amount").and_then(num).unwrap_or(amount);
                    match o.get("side").and_then(side_of) {
                        Some(s) => jobs.push((s, p, amt)),
                        None => {
                            jobs.push((Side::L, p.clone(), amt));
                            jobs.push((Side::R, p, amt));
                        }
                    }
                }
                Value::Object(o) => {
                    for (side, v) in o {
                        let s = side_of(&json!(side)).ok_or_else(|| format!("hand: sides are \"L\" and \"R\" (got \"{side}\"); poses: {}", hands::NAMES.join(", ")))?;
                        match v {
                            Value::String(p) => jobs.push((s, p.clone(), amount)),
                            Value::Object(po) => jobs.push((s, po.get("pose").and_then(Value::as_str).unwrap_or("relaxed").to_owned(), po.get("amount").and_then(num).unwrap_or(1.0))),
                            _ => return Err("hand: {\"L\": \"cup\", \"R\": \"pinch\"} or \"fist\" for both".into()),
                        }
                    }
                }
                _ => return Err(format!("hand: a pose name ({}), {{\"L\": pose, \"R\": pose}} or {{\"pose\": name, \"side\": \"L\", \"amount\": 0..1}}", hands::NAMES.join(", "))),
            }
            if body.fingers.is_empty() {
                return Err(format!("this character has no finger bones, so there is nothing for a hand pose to bend (bones: {}); name finger bones like thumb.1.L / index.2.R (a hand with fingers comes with the character body), or pose the hand bone with bone turns", rig.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")));
            }
            for (side, pname, amt) in &jobs {
                let pose = hands::named(pname).ok_or_else(|| format!("hand pose \"{pname}\"; poses: {}", hands::NAMES.join(", ")))?;
                let times = Self::layer_times(t0, t_end(0.0).max(t0 + over), hz);
                let base = |tt: f32| Self::base_locals(&snap, rig, tt);
                let ch = pz::layer(rig, &times, &base, &mut |s, tt| {
                    hands::apply(s, &body, rig, *side, &pose, envelope(tt) * amt.clamp(0.0, 1.0));
                }, 0.4, 0.2);
                n += Self::write_layer(mc, &ch, t0, &[]);
                said.push(format!("{} hand {pname}", side.name()));
            }
        }
        // ---- body
        if let Some(b) = k.get("body") {
            let (name, params): (String, Value) = match b {
                Value::String(s) => (s.to_lowercase(), json!({})),
                Value::Object(o) if o.len() == 1 => {
                    let (n, v) = o.iter().next().expect("one");
                    (n.to_lowercase(), if v.is_object() { v.clone() } else { json!({"amount": v.clone()}) })
                }
                _ => return Err("body: \"sit\", \"crouch\", \"stand\" or {\"lean\": {\"dir\": \"forward\", \"deg\": 12}}".into()),
            };
            let times = Self::layer_times(t0, t_end(0.0).max(t0 + over), hz);
            let amount = params.get("amount").and_then(num).or_else(|| params.get("depth").and_then(num)).unwrap_or(1.0);
            let base = |tt: f32| Self::base_locals(&snap, rig, tt);
            let ch = match name.as_str() {
                "sit" | "sitting" => {
                    let seat = params.get("seat").and_then(num);
                    pz::layer(rig, &times, &base, &mut |s, tt| pz::sit(s, &body, rig, envelope(tt), seat), 0.4, 0.2)
                }
                "crouch" | "squat" => pz::layer(rig, &times, &base, &mut |s, tt| pz::crouch(s, &body, rig, envelope(tt) * amount), 0.4, 0.2),
                "lean" => {
                    let dir = params.get("dir").and_then(Value::as_str).unwrap_or("forward").to_lowercase();
                    let deg = params.get("deg").and_then(num).unwrap_or(12.0);
                    let d = match dir.as_str() {
                        "forward" | "front" | "in" => body.fwd,
                        "back" | "backward" | "away" => studio_mesh::scale(body.fwd, -1.0),
                        "left" => body.left(),
                        "right" => studio_mesh::scale(body.left(), -1.0),
                        _ => return Err("lean dir: forward, back, left or right".into()),
                    };
                    pz::layer(rig, &times, &base, &mut |s, tt| pz::lean(s, &body, rig, d, deg * envelope(tt)), 0.4, 0.2)
                }
                "stand" | "rise" => {
                    // The lower body and spine go back to the base pose.
                    let mut names: Vec<String> = vec![rig.bones[body.root].name.clone(), rig.bones[body.hips].name.clone()];
                    for l in &body.legs {
                        names.extend([l.upper, l.lower, l.end].iter().map(|&i| rig.bones[i].name.clone()));
                    }
                    names.extend(body.spine.iter().chain(body.chest.iter()).map(|&i| rig.bones[i].name.clone()));
                    for path in mc.keyframes.keys().cloned().collect::<Vec<_>>() {
                        let Some(rest) = path.strip_prefix("generator.bone.") else { continue };
                        let Some((bone, axis)) = rest.rsplit_once('.') else { continue };
                        if !names.iter().any(|n| n == bone) {
                            continue;
                        }
                        let v = if axis.starts_with('l') { m::Value::Scalar(0.0) } else { m::Value::Angle(0.0) };
                        let was = mc.property_at(&path, micros(t)).and_then(|x| x.as_f64()).unwrap_or(0.0);
                        if let Some(tr) = mc.keyframes.get_mut(&path) {
                            tr.keys.retain(|kf| kf.time <= micros(t));
                        }
                        if was.abs() > 1e-6 {
                            key(mc, &path, micros((t0 + over) as f64), v, m::Interp::Auto);
                            n += 1;
                        }
                    }
                    Vec::new()
                }
                other => return Err(format!("body \"{other}\": sit, crouch, lean, stand")),
            };
            n += Self::write_layer(mc, &ch, t0, &[]);
            said.push(format!("body {name}"));
        }
        // ---- beats
        if let Some(bv) = k.get("beat") {
            let name = bv.as_str().ok_or_else(|| format!("beat: {}", pz::BEATS.join(", ")))?;
            let beat = pz::Beat::parse(name).ok_or_else(|| format!("beat \"{name}\": {}", pz::BEATS.join(", ")))?;
            let size = k.get("size").and_then(num).unwrap_or(1.0).clamp(0.2, 2.5);
            let secs = f(k, "seconds").unwrap_or(beat.seconds() as f64) as f32;
            let toward_v = k.get("toward").or_else(|| k.get("look_at"));
            let times = Self::layer_times(t0, t0 + secs, 30.0);
            let tgt = match toward_v {
                Some(v) if !v.is_null() => Some(self.model_target(mc, v, t + secs as f64 * 0.5)?),
                _ => None,
            };
            if matches!(beat, pz::Beat::Glance) && tgt.is_none() {
                return Err("glance needs \"toward\": [x, y, z], {\"clip\": id} or {\"world\": [x, y, z]}".into());
            }
            let base = |tt: f32| Self::base_locals(&snap, rig, tt);
            let ch = pz::layer(rig, &times, &base, &mut |s, tt| pz::beat(s, &body, beat, (tt - t0) / secs, size, tgt), 0.3, 0.2);
            if ch.is_empty() {
                return Err(format!("{name} found nothing to move: the rig needs head, neck, spine, shoulder or arm bones (found {})", body.describe(rig)));
            }
            n += Self::write_layer(mc, &ch, t0, &[]);
            said.push(format!("{name} over {secs:.1} s"));
        }
        // ---- recipes: hands working together from the primitives
        if let Some(rv) = k.get("recipe") {
            let name = rv.as_str().ok_or_else(|| format!("recipe: {}", pz::RECIPES.join(", ")))?;
            let recipe = pz::Recipe::parse(name).ok_or_else(|| format!("recipe \"{name}\": {}", pz::RECIPES.join(", ")))?;
            let dom = k.get("dominant").and_then(side_of).unwrap_or(Side::R);
            let secs = f(k, "seconds").unwrap_or(recipe.seconds() as f64) as f32;
            let times = Self::layer_times(t0, t0 + secs, 15.0);
            let base = |tt: f32| Self::base_locals(&snap, rig, tt);
            let ch = pz::layer(rig, &times, &base, &mut |s, tt| {
                pz::recipe(s, &body, rig, recipe, dom, ((tt - t0) / secs).clamp(0.0, 1.0));
            }, 0.4, 0.2);
            n += Self::write_layer(mc, &ch, t0, &[]);
            if k.get("look").and_then(Value::as_bool).unwrap_or(false) {
                let mut s = Solver::new(rig);
                let at = pz::recipe_gaze(&mut s, &body, rig);
                let times = Self::layer_times(t0, t0 + secs, 15.0);
                let base = |tt: f32| Self::base_locals(&snap, rig, tt);
                let ch = pz::layer(rig, &times, &base, &mut |s, tt| {
                    let u = ((tt - t0) / secs).clamp(0.0, 1.0);
                    let e = pz::ramp(u, 0.0, 0.25) * (1.0 - pz::ramp(u, 0.8, 0.2));
                    pz::look(s, &body, at, [e, e * 0.9, e * 0.6], 0.8);
                }, 0.3, 0.2);
                n += Self::write_layer(mc, &ch, t0, &[]);
            }
            said.push(format!("{} over {secs:.1} s ({})", recipe.name(), if body.fingers.is_empty() { "arms only: no finger bones yet" } else { "hands posed" }));
        }
        // ---- looks that lead with the eyes
        if let Some(lv) = k.get("look_at").filter(|_| k.get("beat").is_none()) {
            if lv.is_null() || lv.as_bool() == Some(false) {
                for path in ["generator.look.on", "generator.aim.look.on"] {
                    key(mc, path, micros(t), m::Value::Scalar(0.0), m::Interp::Hold);
                }
                n += 2;
                said.push("look off".into());
            } else if body.head.is_some() {
                let share = k.get("head").and_then(num).unwrap_or(0.7);
                let secs = f(k, "seconds").unwrap_or(0.0) as f32;
                let t1 = t_end(secs.max(0.0)).max(t0 + over + 0.4);
                let times = Self::layer_times(t0, t1, hz);
                // The eyes get there first, the head a beat later, the chest last.
                let lead = [0.0, 0.07, 0.15];
                let mut intent_t: Vec<(f32, f32)> = Vec::new();
                let (mut ix, mut iy, mut iz): (Vec<(f32, f32)>, Vec<(f32, f32)>, Vec<(f32, f32)>) = (Vec::new(), Vec::new(), Vec::new());
                let base = |tt: f32| Self::base_locals(&snap, rig, tt);
                let mut short = 0.0f32;
                let mut err: Option<String> = None;
                let tg: Vec<(f32, V3)> = times
                    .iter()
                    .map(|&tt| match self.model_target(mc, lv, tt as f64) {
                        Ok(p) => (tt, p),
                        Err(e) => {
                            err.get_or_insert(e);
                            (tt, [0.0; 3])
                        }
                    })
                    .collect();
                if let Some(e) = err {
                    return Err(e);
                }
                for (tt, p) in &tg {
                    let w = world_of(mc, *tt, *p);
                    ix.push((*tt, w[0]));
                    iy.push((*tt, w[1]));
                    iz.push((*tt, w[2]));
                }
                intent_t.push((t0 + over, 1.0));
                if hold.is_some() {
                    intent_t.push((t1, 0.0));
                }
                let ch = pz::layer(rig, &times, &base, &mut |s, tt| {
                    let target = tg.iter().min_by(|a, b| (a.0 - tt).abs().total_cmp(&(b.0 - tt).abs())).map_or([0.0; 3], |x| x.1);
                    let w = |d: f32| pz::ramp(tt, t0 + d, over) * hold.map_or(1.0, |h| 1.0 - pz::ramp(tt, t0 + over + h + d, release));
                    let r = pz::look(s, &body, target, [w(lead[0]), w(lead[1]), w(lead[2])], share);
                    short = short.max(r.short_by * w(0.0));
                }, 0.25, 0.2);
                // Keep only samples to simplify the intent: first, last and a few between.
                let thin = |v: Vec<(f32, f32)>| -> Vec<(f32, f32)> { let n = v.len(); v.into_iter().enumerate().filter(|(i, _)| *i == 0 || *i + 1 == n || i % 4 == 0).map(|(_, x)| x).collect() };
                n += Self::write_layer(mc, &ch, t0, &[("aim.look.wx", thin(ix)), ("aim.look.wy", thin(iy)), ("aim.look.wz", thin(iz)), ("aim.look.on", intent_t)]);
                said.push(if short > 3.0 { format!("look: eyes, head and chest turned as far as they can; the target is {short:.0} degrees further round than the neck allows, turn the whole character first (act turn / face)") } else { "look: eyes first, then head and chest".into() });
            } else {
                // No head bone found: the rig's own look-at constraint.
                let tp = self.model_target(mc, lv, t)?;
                for (c, x) in ["x", "y", "z"].iter().zip(tp) {
                    key(mc, &format!("generator.look.{c}"), micros(t), m::Value::Scalar(x as f64), m::Interp::Auto);
                }
                key(mc, "generator.look.on", micros(t), m::Value::Scalar(1.0), m::Interp::Auto);
                n += 4;
                said.push("look (no head bone found: the rig's look-at constraint)".into());
            }
        }
        // ---- a reach to a target (the older `reach` names an IK handle)
        if let Some(Value::Object(r)) = k.get("reach").filter(|v| v.get("to").is_some() || v.get("hand").is_some()) {
            let side = r.get("hand").or_else(|| r.get("side")).and_then(side_of).ok_or("reach: {\"hand\": \"L\" or \"R\", \"to\": [x, y, z] | {\"clip\": id} | {\"world\": [x, y, z]}}")?;
            let to = r.get("to").ok_or("reach needs \"to\"")?;
            let lean_in = r.get("lean").and_then(Value::as_bool).unwrap_or(true);
            let step = r.get("step").and_then(Value::as_bool).unwrap_or(false);
            let arm = body.arm(side).ok_or("this character has no arm to reach with (bones like upper_arm.L, forearm.L, hand.L)")?.clone();
            let mut t_reach = t;
            let tgt0 = self.model_target(mc, to, t)?;
            // Too far: report it, or walk closer first.
            let mut probe = Solver::with_pose(rig, Self::base_locals(&snap, rig, t0));
            let info = pz::reach(&mut probe, &body, rig, side, tgt0, 1.0, lean_in).ok_or("no arm")?;
            let arm_len = Body::limb_length(rig, &arm);
            let xf = Self::clip_xf(mc, micros(t));
            if info.short_by > 0.0 {
                if step {
                    // Walk toward it until the arm reaches: stop `0.8` arms' lengths short.
                    let dist_model = info.distance - 0.8 * arm_len;
                    let w = xf.world(tgt0);
                    let v = [w[0] - xf.pos[0], w[2] - xf.pos[2]];
                    let len = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-3);
                    let walk_px = (dist_model * xf.scale).max(0.0).min(len);
                    let to = [xf.pos[0] + v[0] / len * walk_px, xf.pos[2] + v[1] / len * walk_px];
                    // Plan it without keying (its length), then key it to arrive as the hand goes out.
                    let face = v[0].atan2(-v[1]).to_degrees();
                    let ell = check::leg_unit(rig, &body, xf.scale);
                    let pace = gait::gait("walk", &body, &Style::default()).map_or(1.4, |g| g.speed) * ell;
                    let eta = (walk_px / pace.max(1e-3) + 1.4).clamp(0.8, 30.0);
                    let start_t = (t - eta as f64).max(0.0);
                    let mut walk_op = json!({"op": "act", "clip": id, "motion": "walk", "to": [to[0], to[1]], "face": face, "at": start_t, "hold": 0.1});
                    if let Some(g) = k.get("ground") {
                        walk_op["ground"] = g.clone();
                    }
                    let said_walk = self.act(&walk_op)?;
                    // The walk rewrote the clip: take its new state.
                    let fresh = self.model_clip(id).ok_or("the clip is gone")?;
                    *mc = fresh;
                    snap = mc.clone();
                    let dur = self.act_duration_hint(mc);
                    t_reach = t.max(dur + 0.05);
                    said.push(format!("stepped closer first ({walk_px:.0} px): {said_walk}"));
                } else {
                    said.push(format!("the target is {:.0} px past the arm's reach ({:.0} px): the hand stops {:.0} px short (leaning {:.0} degrees); move the target closer or add step: true so the body walks closer first", info.short_by * xf.scale, arm_len * xf.scale, info.short_by * xf.scale, info.leaned));
                }
            }
            let t0r = t_reach as f32;
            let t1 = t0r + over + hold.map_or(0.0, |h| h + release);
            let times = Self::layer_times(t0r, t1.max(t0r + over), hz);
            let tg: Vec<(f32, V3)> = times.iter().map(|&tt| (tt, self.model_target(mc, to, tt as f64).unwrap_or(tgt0))).collect();
            let base2 = |tt: f32| Self::base_locals(&snap, rig, tt);
            let ch = pz::layer(rig, &times, &base2, &mut |s, tt| {
                let target = tg.iter().min_by(|a, b| (a.0 - tt).abs().total_cmp(&(b.0 - tt).abs())).map_or(tgt0, |x| x.1);
                let w = pz::ramp(tt, t0r, over) * hold.map_or(1.0, |h| 1.0 - pz::ramp(tt, t0r + over + h, release));
                pz::reach(s, &body, rig, side, target, w, lean_in);
            }, 0.4, 0.2);
            n += Self::write_layer(mc, &ch, t0r, &[]);
            said.push(format!("{} hand reaches (elbow natural, bones keep their length)", side.name()));
            // A hand pose can ride along with it.
            if let Some(hp) = r.get("pose").and_then(Value::as_str) {
                let mut sub = json!({"hand": {"pose": hp, "side": side.name()}, "over": over});
                if let Some(h) = hold {
                    sub["hold"] = json!(h);
                }
                let (n2, s2) = self.pose_layers(id, &sub, t_reach, mc, rig)?;
                n += n2;
                said.extend(s2);
            }
        }
        Ok((n, said))
    }

    /// When the baked action in a clip ends (seconds into the clip).
    fn act_duration_hint(&self, mc: &m::Clip) -> f64 {
        mc.keyframes.get("transform.position").and_then(|t| t.keys.last()).map_or(0.0, |k| k.time as f64 / 1e6)
    }

    // --------------------------------------------------------------- query

    /// The posed frames and the clip's place at timeline time `t` (the
    /// renderer's own rig evaluation).
    #[cfg(feature = "render")]
    fn sample_pose(&self, mc: &m::Clip, rig: &Rig, rel: m::Micros) -> (Vec<studio_mesh::rig::Xf>, ClipXf) {
        let rel = rel.clamp(0, mc.duration.max(0));
        let params = studio_render::model_scene::convert_params(&mc.generator_params_at(rel));
        let input = studio_render::model_anim::pose_input(&params, rel as f32 / 1e6);
        (rig.evaluate(&input).world, Self::clip_xf(mc, rel))
    }

    /// `inspect` with `pose`: where bones and key points are at a moment, or
    /// the findings over a span.
    #[cfg(feature = "render")]
    pub(super) fn pose_report(&self, id: u32, a: &Value) -> Result<Value, String> {
        let (mc, _mesh, rig, body) = self.performer(id)?;
        let opts = a.get("pose").cloned().unwrap_or(Value::Bool(true));
        let r1 = |v: f32| (v * 10.0).round() / 10.0;
        let p3 = |p: V3| json!([r1(p[0]), r1(p[1]), r1(p[2])]);
        let vc = self.clip(id)?;
        let rel_of = |t: f64| micros(t) - vc.start;
        let ell = check::leg_unit(&rig, &body, Self::clip_xf(&mc, 0).scale);
        let units = "canvas px: x right, y down, z away; feet and hands are world points; contact/slide from neighbouring moments";
        // Over a span: contacts, slides and every finding with its fix.
        if let Some(span) = opts.get("span").and_then(Value::as_array).filter(|s| s.len() == 2) {
            let (s0, s1) = (span[0].as_f64().unwrap_or(0.0), span[1].as_f64().unwrap_or(0.0));
            let step = opts.get("step").and_then(num).unwrap_or(0.1).clamp(0.02, 1.0) as f64;
            let mut samples = Vec::new();
            let mut t = s0;
            while t <= s1 + 1e-9 && samples.len() < 2000 {
                let (world, xf) = self.sample_pose(&mc, &rig, rel_of(t));
                samples.push(Sample { t: t as f32, xf, world });
                t += step;
            }
            let mut pts: Vec<_> = samples.iter().map(|s| query::points(&rig, &body, &s.world, &s.xf, s.t)).collect();
            query::annotate(&mut pts, ell, body.up[1].signum());
            let mut feet = Vec::new();
            let slides = check::slides(&body, &samples.iter().map(|s| s.t).collect::<Vec<_>>(), &pts, ell);
            for (li, leg) in body.legs.iter().enumerate() {
                let mut down: Vec<(f32, f32)> = Vec::new();
                for (i, p) in pts.iter().enumerate() {
                    if p.feet[li].contact {
                        match down.last_mut() {
                            Some(r) if (samples[i].t - r.1) as f64 <= step * 1.5 => r.1 = samples[i].t,
                            _ => down.push((samples[i].t, samples[i].t)),
                        }
                    }
                }
                let slide: f32 = slides.iter().filter(|x| x.leg == leg.name).map(|x| x.dist).sum();
                feet.push(json!({"leg": leg.name, "down": down.iter().map(|d| json!([r1(d.0), r1(d.1)])).collect::<Vec<_>>(), "slid_px": r1(slide)}));
            }
            let intents = self.look_intents(&mc, vc.start);
            let found = check::analyse(&rig, &body, &samples, &intents);
            let notes: Vec<Value> = found.iter().take(12).map(|f| json!({"kind": f.kind, "from": r1(f.t0), "to": r1(f.t1), "who": f.who, "problem": f.detail, "fix": f.fix})).collect();
            return Ok(json!({"span": [s0, s1], "units": units, "feet": feet, "findings": notes, "clean": found.is_empty()}));
        }
        // At one moment: the middle of the clip, or `at`.
        let t = f(a, "at").unwrap_or(secs(vc.start + vc.dur / 2));
        let around: Vec<f64> = [-0.1, -0.05, 0.0, 0.05, 0.1].iter().map(|d| (t + d).clamp(secs(vc.start), secs(vc.start + vc.dur))).collect();
        let mut pts: Vec<_> = around
            .iter()
            .map(|&tt| {
                let (world, xf) = self.sample_pose(&mc, &rig, rel_of(tt));
                query::points(&rig, &body, &world, &xf, tt as f32)
            })
            .collect();
        query::annotate(&mut pts, ell, body.up[1].signum());
        let p = &pts[2];
        let feet: Vec<Value> = p
            .feet
            .iter()
            .map(|f| json!({"leg": f.leg, "ankle": p3(f.ankle), "sole": p3(f.sole), "toe": p3(f.toe), "contact": f.contact, "sliding": f.contact && f.speed > 0.12 * ell, "speed_px_s": r1(f.speed)}))
            .collect();
        let hands: Vec<Value> = p
            .hands
            .iter()
            .map(|h| json!({"hand": h.side.name(), "wrist": p3(h.wrist), "tip": p3(h.tip), "elbow": p3(h.elbow), "reach_px": r1(h.reach), "arm_px": r1(h.arm_length), "stretch": (query::stretch_of(h) * 100.0).round() / 100.0}))
            .collect();
        let mut out = json!({"t": r1(t as f32), "units": units, "root": p3(p.root), "head": p3(p.head), "eyes": p3(p.eyes), "gaze": [(p.gaze[0] * 100.0).round() / 100.0, (p.gaze[1] * 100.0).round() / 100.0, (p.gaze[2] * 100.0).round() / 100.0], "feet": feet, "hands": hands});
        if let Some(names) = opts.get("bones").or_else(|| a.get("bones")).and_then(Value::as_array) {
            let mut o = serde_json::Map::new();
            for nm in names.iter().filter_map(Value::as_str) {
                match p.bones.iter().find(|b| b.0.eq_ignore_ascii_case(nm)) {
                    Some(b) => {
                        o.insert(b.0.clone(), json!({"head": p3(b.1), "tail": p3(b.2)}));
                    }
                    None => return Err(format!("no bone \"{nm}\"; bones: {}", p.bones.iter().map(|b| b.0.as_str()).collect::<Vec<_>>().join(", "))),
                }
            }
            out["bones"] = Value::Object(o);
        }
        if opts.get("all").and_then(Value::as_bool) == Some(true) {
            out["bones"] = Value::Object(p.bones.iter().map(|b| (b.0.clone(), json!({"head": p3(b.1), "tail": p3(b.2)}))).collect());
        }
        Ok(out)
    }

    #[cfg(not(feature = "render"))]
    pub(super) fn pose_report(&self, _id: u32, _a: &Value) -> Result<Value, String> {
        Err("the pose query needs the renderer (built without the render feature)".into())
    }

    /// Where the character was told to look, from the layer's own keys:
    /// world points over spans.
    fn look_intents(&self, mc: &m::Clip, clip_start: m::Micros) -> Vec<Intent> {
        let _ = clip_start;
        let (Some(on), Some(wx), Some(wy), Some(wz)) = (mc.keyframes.get("generator.aim.look.on"), mc.keyframes.get("generator.aim.look.wx"), mc.keyframes.get("generator.aim.look.wy"), mc.keyframes.get("generator.aim.look.wz")) else { return Vec::new() };
        let mut out = Vec::new();
        let mut open: Option<f32> = None;
        let end = mc.duration as f32 / 1e6;
        let flush = |from: f32, to: f32, out: &mut Vec<Intent>| {
            // One intent per short stretch, so a moving target is followed.
            let n = (((to - from) / 0.5).ceil() as usize).max(1);
            for i in 0..n {
                let (a, b) = (from + (to - from) * i as f32 / n as f32, from + (to - from) * (i + 1) as f32 / n as f32);
                let mid = micros(((a + b) / 2.0) as f64);
                let g = |tr: &m::Track| tr.eval(mid).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                out.push(Intent { t0: a, t1: b, target: [g(wx), g(wy), g(wz)] });
            }
        };
        for k in &on.keys {
            let t = k.time as f32 / 1e6;
            let v = k.value.as_f64().unwrap_or(0.0);
            match (v > 0.5, open) {
                (true, None) => open = Some(t),
                (false, Some(s)) => {
                    flush(s, t, &mut out);
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(s) = open {
            flush(s, end, &mut out);
        }
        out
    }

    // -------------------------------------------------------------- critic

    /// Notes on every character performance: sliding or crossing feet,
    /// stretched, torn or broken limbs, characters not looking where told.
    pub(super) fn character_problems(&self) -> Vec<CharNote> {
        #[cfg(feature = "render")]
        {
            let mut out = Vec::new();
            let Some(seq) = self.doc.model.sequence(self.doc.model.active_sequence) else { return out };
            for c in seq.clips.iter().filter(|c| c.enabled) {
                let m::ClipSource::Generator(g) = &c.source else { continue };
                if g.kind != m::GeneratorKind::Model || c.duration < SEC {
                    continue;
                }
                let performed = g.params.contains_key("action") || c.keyframes.keys().any(|k| k.starts_with("generator.bone.") || k.starts_with("generator.ik.") || k.starts_with("generator.aim."));
                if !performed {
                    continue;
                }
                let Some(file) = crate::mesh_store::file_of(c) else { continue };
                let Some(rig) = rig_cache(&file) else { continue };
                let body = Body::of(&rig);
                if body.legs.is_empty() && body.arms.is_empty() {
                    continue;
                }
                let secs_total = (c.duration as f64 / 1e6).min(120.0);
                let dt = if secs_total > 40.0 { 0.2 } else { 0.1 };
                let mut samples = Vec::new();
                let mut t = 0.0;
                while t <= secs_total + 1e-9 {
                    let (world, xf) = self.sample_pose(c, &rig, micros(t));
                    samples.push(Sample { t: t as f32, xf, world });
                    t += dt;
                }
                let intents = self.look_intents(c, c.start);
                let name = c.name.split('|').next().unwrap_or(&c.name).trim().to_owned();
                let mut per_kind: HashMap<(&'static str, String), usize> = HashMap::new();
                for f in check::analyse(&rig, &body, &samples, &intents) {
                    let n = per_kind.entry((f.kind, f.who.clone())).or_default();
                    *n += 1;
                    if *n > 2 {
                        continue;
                    }
                    out.push(CharNote { kind: f.kind, at: secs(c.start) + f.t0 as f64, clip: c.id.0 as u32, problem: format!("{name} (clip {}): {}", c.id.0, f.detail), fix: f.fix });
                }
            }
            out
        }
        #[cfg(not(feature = "render"))]
        {
            Vec::new()
        }
    }
}

/// A character note: like a 3D note, with a fix written for this case.
pub struct CharNote {
    pub kind: &'static str,
    pub at: f64,
    pub clip: u32,
    pub problem: String,
    pub fix: String,
}

/// A model point as the world sees it at clip time `t`.
fn world_of(mc: &m::Clip, t: f32, p: V3) -> V3 {
    Session::clip_xf(mc, micros(t as f64)).world(p)
}

type V3 = [f32; 3];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;
    use studio_mesh::rig::Bone;

    fn text_of(r: Vec<Content>) -> String {
        r.into_iter().map(|c| if let Content::Text(t) = c { t } else { String::new() }).collect()
    }

    fn with_character() -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Perform", 1280, 720, 30.0));
        s.edit(&[json!({"op": "model", "shape": "character", "size": 320, "detail": 1, "duration": 24})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s, id)
    }

    fn query(s: &mut Session, id: u32, pose: Value, at: f64) -> Value {
        let v: Value = serde_json::from_str(&text_of(s.run("inspect", &json!({"clip": id, "at": at, "pose": pose})).unwrap())).unwrap();
        v["pose"].clone()
    }

    fn near(a: &Value, b: [f64; 3], tol: f64) -> bool {
        (0..3).all(|i| (a[i].as_f64().unwrap() - b[i]).abs() < tol)
    }

    #[test]
    fn act_walks_a_route_with_planted_feet_and_the_query_says_so() {
        let (mut s, id) = with_character();
        let r = s.edit(&[json!({"op": "act", "clip": id, "motion": "walk", "route": [[0, -400], [300, -600]], "style": "weary"})], "walk", false).unwrap();
        assert!(r.to_string().contains("planted feet"), "{r}");
        let mc = s.model_clip(id).unwrap();
        for p in ["transform.position", "transform.position_z", "transform.rotation_y"] {
            assert!(mc.keyframes.get(p).is_some_and(|t| t.keys.len() >= 3), "{p}");
        }
        assert!(matches!(mc.generator_params_at(0).get("action"), Some(m::Value::Text(a)) if a.starts_with("perf.walk")));
        // The body arrives where it was sent.
        let end = mc.keyframes["transform.position_z"].keys.last().unwrap();
        assert!((end.value.as_f64().unwrap() + 600.0).abs() < 4.0, "{:?}", end.value);
        let last_t = end.time as f64 / 1e6;
        // Over the whole walk: feet planted, nothing sliding, no findings.
        let v = query(&mut s, id, json!({"span": [0.0, last_t], "step": 0.1}), 0.0);
        assert_eq!(v["clean"], json!(true), "{v}");
        for f in v["feet"].as_array().unwrap() {
            assert!(f["slid_px"].as_f64().unwrap() < 3.0, "{f}");
            assert!(!f["down"].as_array().unwrap().is_empty());
        }
        // At the stop both feet are down, still, apart.
        let v = query(&mut s, id, json!(true), last_t);
        let feet = v["feet"].as_array().unwrap();
        assert!(feet.iter().all(|f| f["contact"] == json!(true) && f["speed_px_s"].as_f64().unwrap() < 2.0), "{v}");
        let (a, b) = (&feet[0]["ankle"], &feet[1]["ankle"]);
        let apart = ((a[0].as_f64().unwrap() - b[0].as_f64().unwrap()).powi(2) + (a[2].as_f64().unwrap() - b[2].as_f64().unwrap()).powi(2)).sqrt();
        assert!(apart > 12.0, "feet {apart} px apart");
    }

    #[test]
    fn the_pose_query_matches_what_the_renderer_draws() {
        let (mut s, id) = with_character();
        s.edit(&[json!({"op": "act", "clip": id, "motion": "walk", "route": [[0, -500]]}), json!({"op": "pose", "clip": id, "keys": [{"t": 0, "bones": {"upper_arm.L": [0, 0, 0]}}, {"t": 2, "bones": {"upper_arm.L": [50, 20, -30], "head": [10, 25, 0]}}]})], "go", false).unwrap();
        let t = 2.4;
        let v = query(&mut s, id, json!({"bones": ["foot.L", "hand.R", "head"]}), t);
        let (mc, mesh, rig) = s.rig_of(id).unwrap();
        let rel = micros(t);
        let params = studio_render::model_scene::convert_params(&mc.generator_params_at(rel));
        let file = crate::mesh_store::file_of(&mc).unwrap();
        let (drawn, _) = studio_render::model_anim::animate(Arc::new(mesh.clone()), &file, &params, rel as f32 / 1e6);
        let xf = Session::clip_xf(&mc, rel);
        // A vertex carried wholly by a bone sits at the same distance from the bone's head as at rest:
        // the drawn model and the query agree on where that bone is.
        for name in ["foot.L", "hand.R", "head"] {
            let bi = rig.bone(name).unwrap();
            let vtx = (0..mesh.positions.len()).filter(|&i| rig.joints[i][0] as usize == bi && rig.weights[i][0] > 0.98).min_by(|&a, &b| {
                let d = |i: usize| studio_mesh::length(studio_mesh::sub(mesh.positions[i], rig.bones[bi].head));
                d(a).total_cmp(&d(b))
            });
            let Some(vtx) = vtx else { continue };
            let rest = studio_mesh::length(studio_mesh::sub(mesh.positions[vtx], rig.bones[bi].head));
            let head_world = &v["bones"][name]["head"];
            let head_model = xf.model([head_world[0].as_f64().unwrap() as f32, head_world[1].as_f64().unwrap() as f32, head_world[2].as_f64().unwrap() as f32]);
            let now = studio_mesh::length(studio_mesh::sub(drawn.positions[vtx], head_model));
            assert!((now - rest).abs() < 0.8, "{name}: {rest:.2} px from its bone at rest, {now:.2} drawn");
        }
        assert!(near(&v["bones"]["head"]["head"], [0.0, 0.0, 0.0], 5000.0));
    }

    #[test]
    fn a_look_leads_with_the_eyes_and_the_gaze_lands_on_the_target() {
        let (mut s, id) = with_character();
        // Up to the left, in model pixels, in front of the face.
        let r = s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 1.0, "look_at": [120, -260, -300]}]})], "look", false).unwrap();
        assert!(r.to_string().contains("eyes first"), "{r}");
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.contains_key("generator.aim.look.on") && mc.keyframes.keys().any(|k| k.starts_with("generator.bone.head.")) && mc.keyframes.keys().any(|k| k.starts_with("generator.bone.eye.L.")));
        // Eyes move before the head does.
        let moved = |bone: &str| -> f64 {
            ["x", "y", "z"].iter().filter_map(|a| mc.keyframes.get(&format!("generator.bone.{bone}.{a}"))).filter_map(|t| t.keys.iter().find(|k| k.value.as_f64().is_some_and(|v| v.abs() > 2.0)).map(|k| k.time as f64)).fold(f64::INFINITY, f64::min)
        };
        assert!(moved("eye.L") < moved("head") && moved("head") <= moved("chest"), "eyes {} head {} chest {}", moved("eye.L"), moved("head"), moved("chest"));
        // The gaze ends up on the target.
        let v = query(&mut s, id, json!(true), 3.0);
        let xf = Session::clip_xf(&mc, micros(3.0));
        let target = xf.world([120.0, -260.0, -300.0]);
        let eyes = [v["eyes"][0].as_f64().unwrap(), v["eyes"][1].as_f64().unwrap(), v["eyes"][2].as_f64().unwrap()];
        let want = [target[0] as f64 - eyes[0], target[1] as f64 - eyes[1], target[2] as f64 - eyes[2]];
        let wl = (want[0] * want[0] + want[1] * want[1] + want[2] * want[2]).sqrt();
        let g: Vec<f64> = (0..3).map(|i| v["gaze"][i].as_f64().unwrap()).collect();
        let cos = (0..3).map(|i| want[i] / wl * g[i]).sum::<f64>();
        assert!(cos.acos().to_degrees() < 6.0, "gaze {g:?} is {:.1} degrees off", cos.acos().to_degrees());
        // And the critic is satisfied.
        let span = query(&mut s, id, json!({"span": [0.0, 4.0]}), 0.0);
        assert!(span["findings"].as_array().unwrap().iter().all(|f| f["kind"] != "char_not_looking"), "{span}");
    }

    /// A character whose digit bones are renamed past recognition: a hand
    /// without finger bones.
    fn without_fingers(s: &mut Session, id: u32) {
        let (mc, mut mesh) = s.model_of(id).unwrap();
        let mut rig = (**mesh.rig.as_ref().unwrap()).clone();
        for (i, b) in rig.bones.iter_mut().enumerate() {
            if ["thumb", "index", "middle", "ring", "pinky"].iter().any(|d| b.name.starts_with(d)) {
                b.name = format!("unnamed_{i}");
            }
        }
        mesh.rig = Some(Arc::new(rig));
        s.save_rigged(mc, &mesh, "no fingers").unwrap();
    }

    #[test]
    fn hand_poses_close_the_fingers_and_a_hand_without_fingers_says_so() {
        let (mut s, id) = with_character();
        without_fingers(&mut s, id);
        let e = s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 1, "hand": "fist"}]})], "fist", false).unwrap_err();
        assert!(e.contains("no finger bones"), "{e}");
        // The character body comes with five fingers on each hand.
        let (mut s, id) = with_character();
        s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 1, "hand": {"L": "fist", "R": "cup"}}]})], "hands", false).unwrap();
        let spread = |s: &mut Session, t: f64, side: &str| -> f64 {
            let v = query(s, id, json!({"bones": [format!("middle.{side}.2"), format!("hand.{side}")]}), t);
            let tip = &v["bones"][format!("middle.{side}.2").as_str()]["tail"];
            let wrist = &v["bones"][format!("hand.{side}").as_str()]["head"];
            (0..3).map(|i| (tip[i].as_f64().unwrap() - wrist[i].as_f64().unwrap()).powi(2)).sum::<f64>().sqrt()
        };
        let open = spread(&mut s, 0.0, "L");
        let fist = spread(&mut s, 3.0, "L");
        let cup = spread(&mut s, 3.0, "R");
        let open_r = spread(&mut s, 0.0, "R");
        assert!(fist < open * 0.75, "fist {fist:.1} vs open {open:.1}");
        assert!(cup < open_r * 0.95 && cup > fist, "cup {cup:.1} between open {open_r:.1} and fist {fist:.1}");
    }

    #[test]
    fn a_strike_is_built_from_hand_and_arm_primitives_and_beats_play_over_a_walk() {
        let (mut s, id) = with_character();
        let r = s.edit(&[json!({"op": "act", "clip": id, "motion": "walk", "route": [[0, -300]]}), json!({"op": "pose", "clip": id, "keys": [{"t": 6, "recipe": "strike", "look": true}, {"t": 9, "beat": "shrug"}, {"t": 11, "beat": "nod"}, {"t": 12, "beat": "sigh"}]})], "acts", false).unwrap();
        assert!(r.to_string().contains("strike"), "{r}");
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.keys().any(|k| k.starts_with("generator.bone.upper_arm.R.")) && mc.keyframes.keys().any(|k| k.starts_with("generator.bone.pinky.L.0.")));
        // At the end of the strike the hands are together in front of the chest and the arms are not stretched.
        let v = query(&mut s, id, json!({"bones": ["hand.L", "hand.R"]}), 7.5);
        let (l, r) = (&v["bones"]["hand.L"]["head"], &v["bones"]["hand.R"]["head"]);
        let gap = (0..3).map(|i| (l[i].as_f64().unwrap() - r[i].as_f64().unwrap()).powi(2)).sum::<f64>().sqrt();
        assert!(gap < 60.0, "hands {gap:.0} px apart");
        for h in v["hands"].as_array().unwrap() {
            assert!(h["stretch"].as_f64().unwrap() <= 1.03, "{h}");
        }
        let span = query(&mut s, id, json!({"span": [5.0, 13.0]}), 0.0);
        assert_eq!(span["clean"], json!(true), "{span}");
    }

    #[test]
    fn a_reach_past_the_arm_says_so_and_can_step_closer_first() {
        let (mut s, id) = with_character();
        let far = [0.0, -20.0, -700.0];
        let r = s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 6, "reach": {"hand": "R", "to": far}}]})], "reach", false).unwrap();
        assert!(r.to_string().contains("past the arm's reach") && r.to_string().contains("step: true"), "{r}");
        // Never stretched, however far.
        let v = query(&mut s, id, json!(true), 8.0);
        for h in v["hands"].as_array().unwrap() {
            assert!(h["stretch"].as_f64().unwrap() <= 1.03, "{h}");
        }
        let (mut s, id) = with_character();
        let r = s.edit(&[json!({"op": "pose", "clip": id, "keys": [{"t": 9, "reach": {"hand": "R", "to": far, "step": true}}]})], "reach", false).unwrap();
        assert!(r.to_string().contains("stepped closer"), "{r}");
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes["transform.position_z"].keys.last().unwrap().value.as_f64().unwrap() < -60.0, "the body walked toward it");
        let v = query(&mut s, id, json!(true), 12.0);
        for h in v["hands"].as_array().unwrap() {
            assert!(h["stretch"].as_f64().unwrap() <= 1.03, "{h}");
        }
    }

    #[test]
    fn the_review_flags_a_sliding_hand_keyed_walk_and_passes_the_planted_one() {
        // The hand-keyed way: a sine gait on the thighs and the body keyed apart.
        let (mut s, id) = with_character();
        let mut keys = Vec::new();
        for i in 0..=48 {
            let t = i as f64 * 0.125;
            let a = 28.0 * (t * std::f64::consts::TAU / 1.1).sin();
            keys.push(json!({"t": t, "bones": {"thigh.L": [a, 0, 0], "thigh.R": [-a, 0, 0], "shin.L": [0, 0, 0]}}));
        }
        s.edit(&[json!({"op": "pose", "clip": id, "keys": keys}), json!({"op": "animate", "clip": id, "property": "position_z", "keys": [[0, 0], [6, -520]], "ease": "linear"})], "hand", false).unwrap();
        let v: Value = serde_json::from_str(&text_of(s.run("review", &json!({"only": "3d"})).unwrap())).unwrap();
        let notes = v["notes"].as_array().unwrap();
        let slide = notes.iter().find(|n| n["kind"] == "char_foot_slide").unwrap_or_else(|| panic!("no slide note: {v}"));
        assert!(slide["fix"].as_str().unwrap().contains("act"), "{slide}");
        assert!(v["verdict"].as_str().unwrap().contains("NOT DONE"));
        // The same distance walked with act: nothing char_ is open.
        let (mut s, id) = with_character();
        s.edit(&[json!({"op": "act", "clip": id, "motion": "walk", "route": [[0, -520]]}), json!({"op": "pose", "clip": id, "keys": [{"t": 2, "beat": "nod"}]})], "walk", false).unwrap();
        let v: Value = serde_json::from_str(&text_of(s.run("review", &json!({"only": "3d"})).unwrap())).unwrap();
        let chars: Vec<&Value> = v["notes"].as_array().unwrap().iter().filter(|n| n["kind"].as_str().is_some_and(|k| k.starts_with("char_"))).collect();
        assert!(chars.is_empty(), "{chars:?}");
    }

    #[test]
    fn bone_and_action_keys_count_as_acting() {
        for p in ["generator.bone.head.x", "generator.ik.hand.L.x", "generator.look.on", "generator.aim.look.on", "generator.action_start", "transform.position_z", "transform.rotation_y", "puppet.1.position"] {
            assert!(is_performance_key(p), "{p}");
        }
        assert!(!is_performance_key("generator.base_colour") && !is_performance_key("effect.1.amount"));
    }

    #[test]
    fn other_characters_walk_too_a_child_a_quadruped_and_a_limp() {
        let (mut s, id) = with_character();
        // A limp on the left and a turn to face the viewer at the end.
        let r = s.edit(&[json!({"op": "act", "clip": id, "motion": "limp", "limp": "L", "route": [[200, -300]], "face": 180})], "limp", false).unwrap();
        assert!(r.to_string().contains("limp"), "{r}");
        let mc = s.model_clip(id).unwrap();
        let yaw = mc.keyframes["transform.rotation_y"].keys.last().unwrap().value.as_f64().unwrap();
        assert!(((yaw - 180.0).rem_euclid(360.0)).min((180.0 - yaw).rem_euclid(360.0)) < 3.0 || (yaw.rem_euclid(360.0) - 180.0).abs() < 3.0, "{yaw}");
        let end_t = mc.keyframes["transform.position_z"].keys.last().unwrap().time as f64 / 1e6;
        let v = query(&mut s, id, json!({"span": [0.0, end_t]}), 0.0);
        assert_eq!(v["clean"], json!(true), "{v}");
        // A rig with no legs says what to name them.
        let mut s2 = Session::new(services::project::new_custom("Chain", 640, 360, 30.0));
        s2.edit(&[json!({"op": "model", "shape": "cylinder", "size": [60, 300, 60], "detail": 5})], "add", false).unwrap();
        let cid = s2.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s2.edit(&[json!({"op": "rig", "clip": cid, "template": "chain", "count": 4, "start": "top"})], "rig", false).unwrap();
        let e = s2.edit(&[json!({"op": "act", "clip": cid, "motion": "walk", "route": [[0, -100]]})], "walk", false).unwrap_err();
        assert!(e.contains("no legs") && e.contains("thigh"), "{e}");
    }
}
