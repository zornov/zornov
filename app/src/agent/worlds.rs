// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Walkable 3D worlds for the AI: real geometry you can walk through and
//! shoot films in.
//!
//! - `build_world`: a world from a description (and, if wanted, a
//!   reference picture whose sky, ground and foliage colours choose the
//!   look): a heightfield terrain, vegetation and rocks scattered as
//!   instances, buildings and rooms from modular kits, roads, props,
//!   characters, water, lights and a sky for the time of day. Never
//!   billboards: every object is a model.
//! - `edit_world`: add or remove things, sculpt and paint the ground,
//!   change the sky, the fog, the water, the hour.
//! - `walk`: a character walks a route through it (round walls, through
//!   doors, up stairs) and the walk is recorded into a camera's path,
//!   smoothed, as a timeline shot.
//! - `stand`: a camera placed where a person would stand, looking where
//!   told.
//! - `world_info`: what is in the world, ground heights, whether there is a
//!   way from here to there.
//!
//! A world is a `.sworld` file (its description as JSON, built the same
//! every time), shown by a model clip whose `world` setting names the file;
//! the project's cameras and the path tracer see it like any 3D layer.

use super::*;
use crate::editor::viewport::{self as vp, nav::Nav};
use std::sync::Arc;
use studio_world::build::{FileLibrary, WorldSpec};
use studio_world::walk::{Explorer, Pose, Route};

/// Metres to Studio pixels (a hundred to the metre).
const PX: f64 = 100.0;

use crate::editor::world_edit::WorldJob;
use vp::world::{read_spec, save_spec, world_file_of};

fn pair(v: Option<&Value>) -> Option<[f32; 2]> {
    let a = v?.as_array()?;
    match a.len() {
        2 => Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32]),
        3 => Some([a[0].as_f64()? as f32, a[2].as_f64()? as f32]),
        _ => None,
    }
}

fn point3(v: Option<&Value>) -> Option<[f32; 3]> {
    let a = v?.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

/// A route point: `[x, z]`, `{"at": [x, z]}` or `{"x": .., "z": ..}`.
fn route_point(v: &Value) -> Option<[f32; 2]> {
    pair(Some(v)).or_else(|| pair(v.get("at"))).or_else(|| Some([v.get("x")?.as_f64()? as f32, v.get("z")?.as_f64()? as f32]))
}

/// The pose of a camera as a Studio navigation state (pixels, y down).
fn nav_of(p: &Pose, canvas_w: f64, canvas_h: f64) -> Nav {
    vp::world::nav_of(p, canvas_w, canvas_h, 1.0 / PX as f32)
}

impl Session {
    /// The world clip asked for (`clip`), or the first one in the project.
    fn world_clip(&self, op: &Value) -> Result<(m::Clip, String), String> {
        if let Some(id) = op.get("clip").and_then(|v| super::sim3d::id_of(&self.doc, v)) {
            let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let f = world_file_of(&c).ok_or_else(|| format!("clip {id} is not a world (build_world makes one)"))?;
            return Ok((c, f));
        }
        for seq in self.doc.model.sequences.iter() {
            for c in &seq.clips {
                if let Some(f) = world_file_of(c) {
                    return Ok((c.clone(), f));
                }
            }
        }
        Err("there is no world in the project yet (build_world makes one)".into())
    }

    fn loaded_world(&self, file: &str) -> Result<Arc<studio_world::world::World>, String> {
        studio_render::world_scene::load(file).ok_or_else(|| format!("the world file {file} could not be built"))
    }

    /// A camera clip: the one asked for, the one in the project, or a new one.
    fn world_camera(&mut self, op: &Value, said: &mut Vec<String>) -> Result<u64, String> {
        if let Some(id) = op.get("camera").and_then(|v| super::sim3d::id_of(&self.doc, v)) {
            let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            if !matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera) {
                return Err(format!("clip {id} is not a camera"));
            }
            return Ok(id as u64);
        }
        if let Some(c) = self.find_generator(m::GeneratorKind::Camera, |_| true) {
            return Ok(c.id.0);
        }
        let made = vp::add_camera(&mut self.doc).ok_or("could not add a camera")?;
        said.push(format!("added camera {made}"));
        Ok(made)
    }

    pub(super) fn build_world(&mut self, op: &Value) -> Result<String, String> {
        let mut v = op.clone();
        let obj = v.as_object_mut().ok_or("build_world takes an object")?;
        for k in ["op", "clip", "at", "duration", "lane", "reference", "why", "comment", "note_to_self", "inside"] {
            obj.remove(k);
        }
        let mut spec: WorldSpec = serde_json::from_value(v).map_err(|e| format!("build_world: {e}"))?;
        let mut said = Vec::new();
        // The look of a picture: a file, or a picture in the project (made with `generate`).
        let reference: Option<String> = match op.get("reference") {
            Some(Value::String(p)) if std::path::Path::new(p).is_file() => Some(p.clone()),
            Some(v @ (Value::Number(_) | Value::String(_))) => Some(self.media_item(v).map_err(|e| format!("reference: {e}"))?.path.map(|p| p.to_string_lossy().into_owned()).ok_or("that picture has no file")?),
            _ => None,
        };
        if let Some(path) = reference.as_deref() {
            let bytes = std::fs::read(path).map_err(|e| format!("reference {path}: {e}"))?;
            let img = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { studio_media::png::decode(&bytes).map_err(|e| format!("reference {path}: {e:?}"))? } else { studio_media::jpeg::decode(&bytes).map_err(|e| format!("reference {path}: {e}"))? };
            let hints = studio_world::look::hints_from_rgba(&img.pixels, img.width as usize, img.height as usize).ok_or("the reference picture is too small to read")?;
            said.push(format!("look from the reference: sky hour about {:.0}, haze {:.0}%, foliage {}", hints.time, hints.haze * 100.0, if hints.foliage.is_some() { "found" } else { "none" }));
            let look = spec.look.get_or_insert_with(Default::default);
            look.hints = Some(studio_world::build::HintsJ::from(&hints));
        }
        let (path, world) = self.make_world(&spec)?;
        let at = op.get("at").and_then(Value::as_f64).map_or(0, micros);
        let dur = op.get("duration").and_then(Value::as_f64).map_or_else(|| super::new_clip_end(self.doc.project.duration(), at) - at, micros);
        // Replace the file of an existing world clip, or add a new one.
        let existing = op.get("clip").and_then(|v| super::sim3d::id_of(&self.doc, v));
        let id = match existing {
            Some(id) => {
                let mut c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
                set_world_file(&mut c, &path);
                let seq = self.doc.seq();
                let ok = self.doc.apply("Rebuild world", m::Edit::SetClip { seq, clip: Box::new(c) });
                self.check(ok, "")?;
                id
            }
            None => {
                let lane = self.room(None, at, dur, None)?;
                let id = self.doc.alloc_id() as u32;
                let mut g = m::Generator::new(m::GeneratorKind::Model);
                g.params.insert("world".into(), m::Value::Text(path.clone()));
                g.params.insert("metres_per_px".into(), m::Value::Scalar(0.01));
                g.params.insert("quality".into(), m::Value::Choice("balanced".into()));
                let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), at, dur, m::ClipSource::Generator(g));
                c.name = if spec.name.is_empty() || spec.name == "World" { "World".into() } else { spec.name.clone() };
                c.transform.three_d = true;
                let seq = self.doc.seq();
                let ok = self.doc.apply("Build world", m::Edit::InsertClip { seq, clip: Box::new(c) });
                self.check(ok, "")?;
                id
            }
        };
        said.insert(0, format!("world as clip {id}: {}", studio_world::build::describe(&world)));
        // A camera where a person would start, if the project has none.
        if self.find_generator(m::GeneratorKind::Camera, |_| true).is_none() {
            let cam = vp::add_camera(&mut self.doc).ok_or("could not add a camera")?;
            let sp0 = world.spawn;
            let pose = Pose { pos: [sp0.position[0], sp0.position[1] + 1.65, sp0.position[2]], yaw: sp0.yaw, pitch: 0.0, fov: 55.0 };
            let (w, h) = (self.doc.project.width as f64, self.doc.project.height as f64);
            vp::record_path(&mut self.doc, cam, &[(at, nav_of(&pose, w, h))])?;
            said.push(format!("camera {cam} at the start, eye height"));
        }
        said.push(format!("starts at ({:.1}, {:.1}) facing {:.0} degrees; sun {:.0} degrees up", world.spawn.position[0], world.spawn.position[2], world.spawn.yaw, world.sky.sun_elevation()));
        Ok(said.join("; "))
    }

    /// Saves and builds a spec: the file and the world.
    fn make_world(&mut self, spec: &WorldSpec) -> Result<(String, Arc<studio_world::world::World>), String> {
        let path = save_spec(&self.doc, spec)?;
        let world = build_saved(&path, spec)?;
        Ok((path, world))
    }

    /// Replaces the spec a world clip shows with an edited one: saved,
    /// built, and set on the clip. The Inspector does the same in steps
    /// (`stage_world`, `build_staged` on a worker, `apply_world`).
    pub(crate) fn replace_world(&mut self, clip: u32, spec: &WorldSpec) -> Result<String, String> {
        let job = Self::stage_world(&self.doc, clip, spec)?;
        let summary = Self::build_staged(&job)?;
        Self::apply_world(&mut self.doc, clip, &job.path)?;
        Ok(job.path + &format!("\n{summary}"))
    }

    /// The cheap first step of a world edit, for the UI thread: the spec is
    /// saved and the file it will be known by is named.
    pub(crate) fn stage_world(doc: &Doc, clip: u32, spec: &WorldSpec) -> Result<WorldJob, String> {
        if !doc.model.sequences.iter().any(|q| q.clip(m::ClipId(clip as u64)).is_some()) {
            return Err(format!("no clip {clip}"));
        }
        Ok(WorldJob { path: save_spec(doc, spec)?, spec: spec.clone() })
    }

    /// The slow step, which needs no document and can run on any thread:
    /// builds the world and keeps it for the viewers. Says what is in it.
    pub(crate) fn build_staged(job: &WorldJob) -> Result<String, String> {
        Ok(studio_world::build::describe(&*build_saved(&job.path, &job.spec)?))
    }

    /// The last step, for the UI thread: the clip shows the built world, as
    /// one undo step. The clip is read as it is now, so nothing else about
    /// it is touched. `false` if the clip is gone (nothing was done).
    pub(crate) fn apply_world(doc: &mut Doc, clip: u32, path: &str) -> Result<bool, String> {
        let Some(mut mc) = doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(clip as u64))).cloned() else { return Ok(false) };
        set_world_file(&mut mc, path);
        let seq = doc.seq();
        doc.apply("Edit world", m::Edit::SetClip { seq, clip: Box::new(mc) });
        match doc.error.take() {
            Some(e) => Err(e),
            None => Ok(true),
        }
    }

    pub(super) fn edit_world(&mut self, op: &Value) -> Result<String, String> {
        let (clip, file) = self.world_clip(op)?;
        // The preset's own things become items of the lists, so they can be removed by number.
        let mut spec = studio_world::build::materialize(&read_spec(&file)?)?;
        let mut said = Vec::new();
        let get = |k: &str| op.get(k).filter(|v| !v.is_null());
        if get("list").and_then(Value::as_bool) == Some(true) {
            let j = serde_json::to_value(&spec).map_err(|e| e.to_string())?;
            return Ok(serde_json::to_string(&j).unwrap_or_default());
        }
        // Lists grow.
        macro_rules! grow {
            ($field:ident, $name:literal) => {
                if let Some(Value::Array(items)) = get($name) {
                    for it in items {
                        spec.$field.push(serde_json::from_value(it.clone()).map_err(|e| format!(concat!($name, ": {}"), e))?);
                    }
                    said.push(format!(concat!("added {} to ", $name), items.len()));
                }
            };
        }
        grow!(scatter, "scatter");
        grow!(buildings, "buildings");
        grow!(props, "props");
        grow!(characters, "characters");
        grow!(lights, "lights");
        grow!(roads, "roads");
        grow!(sculpt, "sculpt");
        grow!(paint, "paint");
        // Things that are one each are replaced (the sky key by key).
        if let Some(sky) = get("sky") {
            let mut cur = serde_json::to_value(spec.sky.clone().unwrap_or_default()).unwrap_or(json!({}));
            if let (Some(c), Some(n)) = (cur.as_object_mut(), sky.as_object()) {
                for (k, v) in n {
                    c.insert(k.clone(), v.clone());
                }
            }
            spec.sky = Some(serde_json::from_value(cur).map_err(|e| format!("sky: {e}"))?);
            said.push("sky".into());
        }
        if let Some(v) = get("water") {
            spec.water = if v.as_bool() == Some(false) { Some(studio_world::build::WaterJ { kind: "none".into(), ..Default::default() }) } else { Some(serde_json::from_value(v.clone()).map_err(|e| format!("water: {e}"))?) };
            said.push("water".into());
        }
        if let Some(v) = get("terrain") {
            spec.terrain = Some(serde_json::from_value(v.clone()).map_err(|e| format!("terrain: {e}"))?);
            said.push("terrain".into());
        }
        if let Some(v) = get("fog").and_then(Value::as_f64) {
            spec.fog = Some(v as f32);
            said.push("fog".into());
        }
        if let Some(v) = get("look") {
            let mut cur = serde_json::to_value(spec.look.clone().unwrap_or_default()).unwrap_or(json!({}));
            if let (Some(c), Some(n)) = (cur.as_object_mut(), v.as_object()) {
                for (k, x) in n {
                    c.insert(k.clone(), x.clone());
                }
            }
            spec.look = Some(serde_json::from_value(cur).map_err(|e| format!("look: {e}"))?);
            said.push("look".into());
        }
        if let Some(v) = point3(get("spawn")) {
            spec.spawn = Some(v);
            said.push("start point".into());
        }
        if let Some(q) = get("quality").and_then(Value::as_str) {
            spec.quality = q.into();
        }
        // Removing by list and number (see `list`).
        if let Some(rm) = get("remove").and_then(Value::as_object) {
            for (name, which) in rm {
                let idx: Vec<usize> = match which {
                    Value::Array(a) => a.iter().filter_map(|x| x.as_u64().map(|i| i as usize)).collect(),
                    Value::Number(n) => n.as_u64().map(|i| vec![i as usize]).unwrap_or_default(),
                    Value::String(s) if s == "all" => usize::MAX.checked_sub(0).map(|_| Vec::new()).unwrap_or_default(),
                    _ => Vec::new(),
                };
                let all = which.as_str() == Some("all");
                macro_rules! drop_from {
                    ($v:expr) => {{
                        let before = $v.len();
                        if all {
                            $v.clear();
                        } else {
                            let mut i = 0;
                            $v.retain(|_| {
                                let keep = !idx.contains(&i);
                                i += 1;
                                keep
                            });
                        }
                        before - $v.len()
                    }};
                }
                let n = match name.as_str() {
                    "scatter" => drop_from!(spec.scatter),
                    "buildings" => drop_from!(spec.buildings),
                    "props" => drop_from!(spec.props),
                    "characters" => drop_from!(spec.characters),
                    "lights" => drop_from!(spec.lights),
                    "roads" => drop_from!(spec.roads),
                    "sculpt" => drop_from!(spec.sculpt),
                    "paint" => drop_from!(spec.paint),
                    other => return Err(format!("remove: \"{other}\": scatter, buildings, props, characters, lights, roads, sculpt, paint")),
                };
                said.push(format!("removed {n} from {name}"));
            }
        }
        let mut mc = clip.clone();
        // The hour is a setting of the layer so it can be animated.
        if let Some(t) = op.get("time_of_day") {
            if t.is_null() {
                if let m::ClipSource::Generator(g) = &mut mc.source {
                    g.params.remove("time_of_day");
                }
                said.push("time of day follows the world's sky".into());
            } else {
                let h = serde_json::from_value::<studio_world::build::Time>(t.clone()).map_err(|e| e.to_string())?.hours()?;
                if let m::ClipSource::Generator(g) = &mut mc.source {
                    g.params.insert("time_of_day".into(), m::Value::Scalar(h as f64));
                }
                said.push(format!("hour {h:.1} (key \"generator.time_of_day\" to run the day)"));
            }
        }
        if said.is_empty() {
            return Err("edit_world: give scatter, buildings, props, characters, lights, roads, sculpt, paint (added), sky, water, terrain, fog, look, spawn, quality (changed), remove: {list: indexes | \"all\"}, time_of_day or list: true".into());
        }
        let touched_spec = said.iter().any(|s| !s.starts_with("hour") && !s.starts_with("time of day"));
        let mut summary = String::new();
        if touched_spec {
            let (path, world) = self.make_world(&spec)?;
            set_world_file(&mut mc, &path);
            summary = studio_world::build::describe(&world);
        }
        let id = mc.id.0;
        let seq = self.doc.seq();
        let ok = self.doc.apply("Edit world", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("world clip {id}: {}{}", said.join("; "), if summary.is_empty() { String::new() } else { format!(" ({summary})") }))
    }

    pub(super) fn walk(&mut self, op: &Value) -> Result<String, String> {
        let (_, file) = self.world_clip(op)?;
        let world = self.loaded_world(&file)?;
        let mut said = Vec::new();
        let camera = self.world_camera(op, &mut said)?;
        let route_in = op.get("route").and_then(Value::as_array).ok_or("walk needs \"route\": [[x, z], ...] in metres (world_info reports where the ground is)")?;
        let mut points: Vec<[f32; 3]> = Vec::new();
        let mut pauses: Vec<f32> = Vec::new();
        for p in route_in {
            let q = route_point(p).ok_or("each route point is [x, z], {\"at\": [x, z]} or {\"x\", \"z\"}")?;
            points.push([q[0], world.ground_y(q[0], q[1]), q[1]]);
            pauses.push(p.get("pause").and_then(Value::as_f64).unwrap_or(0.0) as f32);
        }
        if points.is_empty() {
            return Err("walk needs at least one route point".into());
        }
        let mut ex = Explorer::new(&world);
        let start2 = pair(op.get("from")).unwrap_or([world.spawn.position[0], world.spawn.position[2]]);
        let first = points[0];
        let yaw0 = (first[0] - start2[0]).atan2(first[2] - start2[1]).to_degrees();
        ex.teleport(&world, [start2[0], world.ground_y(start2[0], start2[1]), start2[1]], if start2 == [first[0], first[2]] { world.spawn.yaw } else { yaw0 });
        let speed = f(op, "speed").map(|v| v as f32);
        if let Some(s) = speed {
            ex.params.walk_speed = s.clamp(0.3, 12.0);
            ex.params.sprint_speed = ex.params.walk_speed * 1.8;
        }
        let eye = f(op, "eye").map_or(1.65, |v| v as f32).clamp(0.3, 3.0);
        ex.params.eye = eye;
        ex.fov = f(op, "fov").map_or(55.0, |v| v as f32).clamp(10.0, 120.0);
        let sprint = op.get("sprint").and_then(Value::as_bool).unwrap_or(false);
        let look_at = point3(op.get("look_at"));
        let fps = 30.0;
        let mut rec = studio_world::record::Recorder::default();
        rec.start();
        let mut dist = 0.0f32;
        let mut last: Option<[f32; 3]> = None;
        // Walk to each point in turn (pausing where asked), recording the view.
        let mut clock = 0.0f32;
        let mut reached = true;
        for (i, p) in points.iter().enumerate() {
            let route = Route { points: vec![*p], sprint, pause: pauses[i], arrive: 0.8 };
            let base = clock;
            let (secs, ok) = studio_world::walk::walk_route(&world, &mut ex, &route, fps, |t, pose| {
                let mut pose = *pose;
                if let Some(l) = look_at {
                    pose = Pose::looking_at(pose.pos, l, ex_fov(&pose));
                }
                if let Some(q) = last {
                    dist += studio_world::math::dist_xz(q, pose.pos);
                }
                last = Some(pose.pos);
                rec.push(base + t, pose);
            });
            clock = base + secs;
            reached &= ok;
            if !ok {
                said.push(format!("could not reach route point {} ({:.1}, {:.1}); stopped there", i + 1, p[0], p[2]));
                break;
            }
        }
        rec.stop();
        let amount = f(op, "smooth").unwrap_or(1.0).clamp(0.0, 3.0) as f32;
        let sm = if amount <= 0.0 { studio_world::record::Smoothing::none() } else { studio_world::record::Smoothing { position: 0.35 * amount, look: 0.25 * amount, ..Default::default() } };
        let path = rec.finish(&sm);
        let start = f(op, "start").map_or(0, micros);
        let fr = (1e6 / self.doc.project.fps.max(1.0) as f64) as view::Time;
        let (w, h) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let mut samples = Vec::new();
        let n = (path.duration() as f64 * self.doc.project.fps.max(1.0) as f64).ceil() as i64;
        for i in 0..=n {
            let t = (i as f32 / self.doc.project.fps.max(1.0)).min(path.duration());
            if let Some(p) = path.at(t) {
                samples.push((start + i * fr, nav_of(&p, w, h)));
            }
        }
        let keys = vp::record_path(&mut self.doc, camera, &samples)?;
        // The world shows as long as the shot.
        self.extend_world_clip(start, start + (path.duration() as f64 * 1e6) as view::Time)?;
        said.push(format!("walked {:.1} m in {:.1} s ({} camera keys on clip {camera}, smoothed, from {:.1} s){}", dist, path.duration(), keys, start as f64 / 1e6, if reached { "" } else { "; the route was cut short" }));
        Ok(said.join("; "))
    }

    /// Lengthens the world layers showing at `from` to cover `to`, if they
    /// are shorter, never past the next clip on their lane: lengthening
    /// every world clip ran one into the next on the same lane, and every
    /// later `camera`, `walk` and `stand` failed with an overlap.
    fn extend_world_clip(&mut self, from: view::Time, to: view::Time) -> Result<(), String> {
        let seq = self.doc.model.active();
        let ids: Vec<(u64, view::Time)> = seq
            .clips
            .iter()
            .filter(|c| world_file_of(c).is_some() && c.start <= from && c.end() > from)
            .map(|c| (c.id.0, seq.clips.iter().filter(|o| o.lane == c.lane && o.id != c.id && o.start >= c.end()).map(|o| o.start).min().unwrap_or(view::Time::MAX)))
            .collect();
        for (id, next) in ids {
            let Some(mut c) = self.model_clip(id as u32) else { continue };
            let to = to.min(next);
            if c.start + c.duration < to {
                c.duration = to - c.start;
                let seq = self.doc.seq();
                let ok = self.doc.apply("Lengthen world", m::Edit::SetClip { seq, clip: Box::new(c) });
                self.check(ok, "")?;
            }
        }
        Ok(())
    }

    pub(super) fn stand(&mut self, op: &Value) -> Result<String, String> {
        let (_, file) = self.world_clip(op)?;
        let world = self.loaded_world(&file)?;
        let mut said = Vec::new();
        let camera = self.world_camera(op, &mut said)?;
        let at = pair(op.get("at")).ok_or("stand needs \"at\": [x, z] metres")?;
        let ground = world.ground_at(at[0], at[1], f32::MAX).map_or(0.0, |g| g.y);
        // The highest floor under a roof is the roof; stand on the one a person reaches.
        let ground = world.ground_at(at[0], at[1], world.ground_y(at[0], at[1]) + 0.7).map_or(ground, |g| g.y);
        let eye = f(op, "eye").or_else(|| f(op, "height")).map_or(1.65, |v| v as f32);
        let pos = [at[0], ground + eye, at[1]];
        let fov = f(op, "fov").map_or(55.0, |v| v as f32);
        let mut pose = Pose { pos, yaw: f(op, "yaw").unwrap_or(0.0) as f32, pitch: f(op, "pitch").unwrap_or(0.0) as f32, fov };
        if let Some(l) = point3(op.get("look_at")) {
            pose = Pose::looking_at(pos, l, fov);
        } else if let Some(l) = pair(op.get("look_at")) {
            let y = world.ground_y(l[0], l[1]) + eye;
            pose = Pose::looking_at(pos, [l[0], y, l[1]], fov);
        }
        let t = f(op, "t").map_or(0, micros);
        let (w, h) = (self.doc.project.width as f64, self.doc.project.height as f64);
        vp::record_path(&mut self.doc, camera, &[(t, nav_of(&pose, w, h))])?;
        said.push(format!("camera {camera} at ({:.1}, {:.1}) eye {:.2} m up, looking {:.0} degrees yaw, {:.0} pitch, at {:.1} s", at[0], at[1], eye, pose.yaw, pose.pitch, t as f64 / 1e6));
        Ok(said.join("; "))
    }

    pub(super) fn world_info(&mut self, op: &Value) -> Result<String, String> {
        let (clip, file) = self.world_clip(op)?;
        let world = self.loaded_world(&file)?;
        let b = world.bounds();
        let mut out = json!({
            "clip": clip.id.0,
            "summary": studio_world::build::describe(&world),
            "bounds_m": {"min": b.min, "max": b.max},
            "spawn": {"at": [world.spawn.position[0], world.spawn.position[2]], "yaw": world.spawn.yaw},
            "sky": {"hour": world.sky.time, "sun_elevation_deg": world.sky.sun_elevation()},
            "water_levels": world.water.iter().map(|w| w.level).collect::<Vec<_>>(),
            "objects": world.objects.iter().enumerate().take(80).map(|(i, o)| json!({"index": i, "name": o.name, "at": [o.inst.pos[0], o.inst.pos[2]], "kind": format!("{:?}", o.kind).to_lowercase()})).collect::<Vec<_>>(),
        });
        if let Some(Value::Array(pts)) = op.get("ground") {
            let hs: Vec<Value> = pts.iter().filter_map(route_point).map(|p| json!({"at": p, "ground": (world.ground_y(p[0], p[1]) * 100.0).round() / 100.0, "wet": world.water_at(p[0], p[1]).is_some_and(|l| world.ground_y(p[0], p[1]) < l)})).collect();
            out["ground"] = json!(hs);
        }
        if let Some(p) = op.get("path") {
            let (a, c) = (pair(p.get("from")).ok_or("path needs from: [x, z]")?, pair(p.get("to")).ok_or("path needs to: [x, z]")?);
            let params = studio_world::walk::Params::default();
            let from = [a[0], world.ground_y(a[0], a[1]), a[1]];
            let to = [c[0], world.ground_y(c[0], c[1]), c[1]];
            out["path"] = match studio_world::nav::find_path(&world, from, to, &params, &Default::default()) {
                Some(p) => {
                    let len: f32 = p.windows(2).map(|s| studio_world::math::dist(s[0], s[1])).sum();
                    json!({"found": true, "metres": (len * 10.0).round() / 10.0, "via": p.iter().map(|q| [q[0], q[2]]).collect::<Vec<_>>()})
                }
                None => json!({"found": false}),
            };
        }
        Ok(serde_json::to_string(&out).unwrap_or_default())
    }
}

fn build_saved(path: &str, spec: &WorldSpec) -> Result<Arc<studio_world::world::World>, String> {
    let world = Arc::new(studio_world::build::build(spec, &FileLibrary)?);
    studio_render::world_scene::remember(path, world.clone());
    Ok(world)
}

fn ex_fov(p: &Pose) -> f32 {
    p.fov
}

fn set_world_file(c: &mut m::Clip, file: &str) {
    if let m::ClipSource::Generator(g) = &mut c.source {
        g.params.insert("world".into(), m::Value::Text(file.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session() -> Session {
        Session::new(services::project::new_custom("Worlds", 320, 180, 24.0))
    }

    #[test]
    fn the_ai_builds_a_world_walks_it_and_films_it() {
        let _lock = vp::test_lock();
        let mut s = session();
        let r = s.edit(&[json!({"op": "build_world", "description": "a pine forest at dawn with a small cabin", "preset": "forest", "size": 160, "quality": "fast", "seed": 3,
            "buildings": [{"kind": "house", "at": [10, 8], "floors": 1, "width": 7, "depth": 6}],
            "characters": [{"name": "Hiker", "at": [4, 2], "clip": "walk", "walk_to": [[20, 2]]}]})], "world", false).unwrap();
        let txt = r.to_string();
        assert!(txt.contains("world as clip") && txt.contains("camera") && txt.contains("starts at"), "{txt}");
        // A model clip showing the world, and a camera.
        let world_id = s.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).find(|c| world_file_of(c).is_some()).map(|c| c.id.0).unwrap();
        assert!(world_id > 0);
        let info = s.edit(&[json!({"op": "world_info", "ground": [[0, 0], [10, 8]], "path": {"from": [0, 0], "to": [25, 15]}})], "info", false);
        let _ = info;
        // Add things and change the hour.
        let r = s.edit(&[json!({"op": "edit_world", "scatter": [{"kind": "rock", "count": 12, "size": 1.4}], "sky": {"time": "golden_hour"}, "time_of_day": 18.5})], "more", false).unwrap();
        assert!(r.to_string().contains("added 1 to scatter"), "{r}");
        // The preset's own things are numbered too: remove one, and the water.
        let listing = s.edit(&[json!({"op": "edit_world", "list": true})], "list", false).unwrap().to_string();
        assert!(listing.contains("scatter") && listing.contains("pine"), "{listing}");
        let r = s.edit(&[json!({"op": "edit_world", "remove": {"scatter": [0]}, "water": false})], "remove", false).unwrap();
        assert!(r.to_string().contains("removed 1 from scatter") && r.to_string().contains("water"), "{r}");
        // Walk a route: keys on the camera, smoothed.
        let r = s.edit(&[json!({"op": "walk", "route": [[0, 14], [14, 14], [25, 4]], "start": 0.5, "speed": 3.0})], "walk", false).unwrap();
        let t = r.to_string();
        assert!(t.contains("walked") && t.contains("camera keys"), "{t}");
        let cam = s.find_generator(m::GeneratorKind::Camera, |_| true).unwrap();
        let pos = cam.keyframes.get("transform.position").expect("position keys");
        assert!(pos.keys.len() >= 4, "{} keys", pos.keys.len());
        assert!(pos.keys.first().unwrap().time >= 0 && cam.keyframes.contains_key("generator.poi"));
        // A camera placed by hand, looking at the cabin.
        let r = s.edit(&[json!({"op": "stand", "at": [3, 14], "look_at": [10, 1.5, 8], "t": 6.0, "eye": 1.7})], "stand", false).unwrap();
        assert!(r.to_string().contains("eye 1.70"), "{r}");
        // Bad things say what is wanted.
        assert!(s.edit(&[json!({"op": "build_world", "preset": "swamp"})], "x", false).is_err());
        assert!(s.edit(&[json!({"op": "walk", "route": "to the lake"})], "x", false).is_err());
        assert!(s.edit(&[json!({"op": "edit_world"})], "x", false).is_err());
        // The layer renders through the camera (CPU or GPU).
        let project = std::sync::Arc::new(s.doc.model.clone());
        let scene = studio_render::model_scene::ModelScene::for_sequence(project, s.doc.seq());
        let mut r = studio_render::best_renderer(studio_render::source::Sources::new(Default::default(), studio_render::source::DecodeCache::new(1 << 24)));
        let f = r.render(&scene, 0, None);
        let lit = f.pixels.iter().filter(|p| p[0] + p[1] + p[2] > 0.1).count();
        assert!(lit > f.pixels.len() / 2, "the frame shows the world: {lit} of {}", f.pixels.len());
    }

    #[test]
    fn a_reference_picture_chooses_the_look_but_the_world_stays_geometry() {
        let _lock = vp::test_lock();
        // A dusk picture: orange sky over dark ground.
        let (w, h) = (64u32, 48u32);
        let mut px = Vec::new();
        for y in 0..h {
            for _ in 0..w {
                let c: [u8; 4] = if y < h / 2 { [232, 128, 66, 255] } else { [40, 34, 28, 255] };
                px.extend_from_slice(&c);
            }
        }
        let dir = std::env::temp_dir().join("studio-world-reference-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dusk.png");
        std::fs::write(&file, studio_media::png::encode(&studio_media::RgbaImage { width: w, height: h, pixels: px })).unwrap();
        let mut s = session();
        let r = s.edit(&[json!({"op": "build_world", "preset": "meadow", "size": 120, "quality": "fast", "reference": file.to_string_lossy()})], "ref", false).unwrap();
        assert!(r.to_string().contains("look from the reference"), "{r}");
        let (_, world_file) = s.world_clip(&json!({})).unwrap();
        let spec = read_spec(&world_file).unwrap();
        let hints = spec.look.and_then(|l| l.hints).expect("hints kept in the description");
        assert!(hints.time > 16.0 && hints.time < 21.0, "dusk hour: {}", hints.time);
        let world = studio_render::world_scene::load(&world_file).unwrap();
        // Lit for dusk, and still real geometry: trees and ground, not a picture.
        assert!(world.sky.sun_elevation() < 15.0, "{}", world.sky.sun_elevation());
        assert!(world.scatters.iter().map(|s| s.instances.len()).sum::<usize>() > 100 && world.terrain.is_some());
        // A project picture can be the reference too (by media id); an unknown one is refused clearly.
        assert!(s.edit(&[json!({"op": "build_world", "preset": "meadow", "reference": 9999})], "x", false).is_err());
    }

    /// The Inspector's edit in steps: while the world builds the clip keeps
    /// the old one, then it shows the new one in a single undoable step, and
    /// the document is whole throughout.
    #[test]
    fn a_world_edit_swaps_the_clip_to_the_built_world_in_one_step() {
        let _lock = vp::test_lock();
        let mut s = session();
        s.edit(&[json!({"op": "build_world", "preset": "meadow", "size": 100, "quality": "fast", "seed": 1})], "w", false).unwrap();
        let (clip, old) = s.world_clip(&json!({})).unwrap();
        let id = clip.id.0 as u32;
        let clips = |s: &Session| s.doc.model.sequences.iter().map(|q| q.clips.len()).sum::<usize>();
        let before = clips(&s);
        let mut spec = studio_world::build::materialize(&read_spec(&old).unwrap()).unwrap();
        spec.fog = Some(0.01);
        let job = Session::stage_world(&s.doc, id, &spec).unwrap();
        assert_ne!(job.path, old);
        // Staged and being built: the clip still shows the old world.
        assert_eq!(world_file_of(&s.model_clip(id).unwrap()).as_deref(), Some(old.as_str()));
        let said = Session::build_staged(&job).unwrap();
        assert!(!said.is_empty());
        assert_eq!(world_file_of(&s.model_clip(id).unwrap()).as_deref(), Some(old.as_str()));
        assert_eq!(Session::apply_world(&mut s.doc, id, &job.path), Ok(true));
        assert_eq!(world_file_of(&s.model_clip(id).unwrap()).as_deref(), Some(job.path.as_str()));
        assert_eq!(clips(&s), before);
        assert!(studio_render::world_scene::load(&job.path).is_some());
        // One undo gives the old world back.
        s.doc.undo();
        assert_eq!(world_file_of(&s.model_clip(id).unwrap()).as_deref(), Some(old.as_str()));
        // A clip that is gone is left alone, and an unknown one is not staged.
        assert_eq!(Session::apply_world(&mut s.doc, 9999, &job.path), Ok(false));
        assert!(Session::stage_world(&s.doc, 9999, &spec).is_err());
    }

    /// `cargo test --release -p studio-app -- --ignored world_edit_ui_cost --nocapture`
    #[test]
    #[ignore]
    fn world_edit_ui_cost() {
        let _lock = vp::test_lock();
        let mut s = session();
        s.edit(&[json!({"op": "build_world", "preset": "village", "seed": 1})], "w", false).unwrap();
        let id = s.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).find(|c| world_file_of(c).is_some()).map(|c| c.id.0 as u32).unwrap();
        for seed in 2..5u64 {
            let mut spec: WorldSpec = serde_json::from_value(json!({"preset": "village", "seed": seed})).unwrap();
            spec.fog = Some(seed as f32 * 0.001);
            let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
            // Before: the whole edit on the UI thread.
            let t = std::time::Instant::now();
            s.replace_world(id, &spec).unwrap();
            let inline = ms(t);
            // After: the UI thread saves the spec and later swaps the clip; a worker builds.
            spec.seed = Some(seed as u32 + 100);
            let t = std::time::Instant::now();
            let job = Session::stage_world(&s.doc, id, &spec).unwrap();
            let stage = ms(t);
            let t = std::time::Instant::now();
            Session::build_staged(&job).unwrap();
            let build = ms(t);
            let t = std::time::Instant::now();
            Session::apply_world(&mut s.doc, id, &job.path).unwrap();
            let apply = ms(t);
            eprintln!("world edit: inline replace_world {inline:.0} ms on the UI thread; staged: UI {:.1} ms (stage {stage:.1} + apply {apply:.1}), worker {build:.0} ms", stage + apply);
        }
    }
}
