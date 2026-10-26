// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Physical 3D scenes for the AI (see `crate::physics3d`).
//!
//! - `body`: what a model is physically: `type` (dynamic, static,
//!   kinematic, none, auto), `material`, `mass`, `collider` (auto, box,
//!   sphere, capsule, hull, parts, mesh), `friction`, `bounce`,
//!   `breakable`; says the mass and collider it worked out.
//! - `joint`: a hinge, ball, slider, fixed, spring, rope or rod from a clip
//!   to another (`to`) or to the world, `at` a point, about an `axis`, with
//!   `limits`, a `motor`, a `spring`, `breaks` (N).
//! - `drop`, `throw`, `push`, `break`, `settle`: the everyday physical
//!   actions, simulated with everything else in the scene and baked into
//!   keys (scrubbable, editable, the same every time).
//! - `ragdoll`: a character goes loose (falls, is knocked over), from a
//!   strength that holds its pose a moment to fully limp.
//! - `jiggle`: secondary motion on a character or creature: tails, ears,
//!   antennae, hair, coat tails and feathers on bones follow through and
//!   settle; `flesh` shakes loose skin.
//! - `bake_physics`: the whole scene simulated from its physics settings.
//! - `physics_world`: the scene's scale (px to the metre), gravity, and
//!   whether new things rest on what is under them.

use super::sim3d::{id_of, ids, num, v3};
use super::*;
use crate::physics3d::scene::{Build, Floor, Role, Scene, character_place, load_mesh, posed_rig};
use crate::physics3d::{self as p3, BodyType, JointSpec, Props, run};
use studio_sim::dynamics::decompose::Fit;
use studio_sim::dynamics::math::{V3, length, scale, sub};
use studio_sim::dynamics::secondary::{Blocker, Chains, Feel, Jiggle, find_chains, looseness};

fn clips_of(doc: &crate::doc::Doc, op: &Value) -> Vec<u32> {
    let mut v = ids(doc, op, "clips");
    if v.is_empty() {
        v = ids(doc, op, "clip");
    }
    v
}

fn floor_of(op: &Value) -> Floor {
    match op.get("floor") {
        Some(Value::Bool(false)) => Floor::Off,
        Some(v) if v.is_number() => Floor::At(v.as_f64().unwrap_or(0.0) as f32),
        _ => Floor::Auto,
    }
}

impl Session {
    fn seconds_at(&self, op: &Value, default: f64) -> view::Time {
        micros(f(op, "at").unwrap_or(default).max(0.0))
    }

    /// Runs a scene and puts the result on the timeline as one undo step.
    fn bake_run(&mut self, scene: Scene, r: &run::Run, label: &str) -> Result<(run::Baked, Vec<String>), String> {
        let seq = scene.seq;
        let baked = run::simulate(&self.doc.model, scene, r);
        let save_model = |m: &studio_mesh::Mesh| crate::mesh_store::save(&self.doc, m);
        let save_cache = |c: &studio_mesh::cache::Cache| self.save_cache(c);
        let (edits, said) = run::bake(&self.doc.model, seq, &baked, &save_model, &save_cache)?;
        if !edits.is_empty() {
            let ok = self.doc.apply(label, m::Edit::Batch { edits });
            self.check(ok, "")?;
        }
        Ok((baked, said))
    }

    fn describe_run(&self, baked: &run::Baked, said: Vec<String>, seconds: f32) -> String {
        let mut parts = said;
        if parts.is_empty() {
            parts.push("nothing moved".into());
        }
        parts.extend(baked.notes.iter().cloned());
        match baked.rested_at {
            Some(t) => parts.push(format!("all at rest by {t:.1} s")),
            None => parts.push(format!("still moving at {seconds:.1} s")),
        }
        let hits = baked.impacts.iter().filter(|i| i.3 > 2.0).count();
        if hits > 0 {
            parts.push(format!("{hits} hard hits (sounds go there)"));
        }
        format!("{} px to the metre; {}", r1(baked.ppm), parts.join("; "))
    }

    /// `body`: a clip's physical properties.
    pub(super) fn body_op(&mut self, op: &Value) -> Result<String, String> {
        let targets = clips_of(&self.doc, op);
        if targets.is_empty() {
            return Err("body needs \"clip\" (or \"clips\"): the models to set".into());
        }
        let mut edits = Vec::new();
        let mut said = Vec::new();
        let ppm = p3::px_per_metre(&self.doc.model);
        for id in targets {
            let mut c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            if !crate::physics3d::scene::is_model(&c) {
                return Err(format!("clip {id} is not a 3D model"));
            }
            let mut p = Props::of(&c);
            if let Some(t) = op.get("type").or(op.get("body")).and_then(Value::as_str) {
                p.body = BodyType::parse(t).ok_or_else(|| format!("type \"{t}\": dynamic, static, kinematic, none or auto"))?;
            }
            if let Some(mat) = op.get("material").and_then(Value::as_str) {
                if studio_sim::dynamics::material::named(mat).is_none() {
                    let names: Vec<&str> = studio_sim::dynamics::material::TABLE.iter().map(|s| s.name).collect();
                    return Err(format!("material \"{mat}\" is none of: {}", names.join(", ")));
                }
                p.material = Some(mat.to_string());
            }
            if let Some(v) = num(op, "mass") {
                p.mass = (v > 0.0).then_some(v);
            }
            if let Some(cl) = op.get("collider").and_then(Value::as_str) {
                p.fit = Fit::parse(cl).ok_or_else(|| format!("collider \"{cl}\": auto, box, sphere, capsule, hull, parts or mesh"))?;
            }
            if let Some(v) = num(op, "friction") {
                p.friction = Some(v.clamp(0.0, 2.0));
            }
            if let Some(v) = num(op, "bounce") {
                p.bounce = Some(v.clamp(0.0, 1.0));
            }
            if let Some(b) = op.get("breakable").and_then(Value::as_bool) {
                p.breakable = Some(b);
            }
            p.write(&mut c);
            // What that makes it.
            let mesh = load_mesh(&c).ok_or_else(|| format!("clip {id}'s model could not be read"))?;
            let stuff = p.stuff(Some(&mesh), &c.name);
            let tris = crate::physics3d::scene::physics_tris(&mesh, &c.transform, ppm);
            let fit = if p.fit == Fit::Mesh && p.body == BodyType::Dynamic { Fit::Auto } else { p.fit };
            let shapes = studio_sim::dynamics::decompose::shapes_for(&tris, fit, 32);
            let what = match &shapes {
                Some(s) => {
                    let vol: f32 = s.parts.iter().map(|(sh, _)| sh.volume()).sum::<f32>().max(s.volume);
                    let mass = p.mass.unwrap_or(vol * stuff.density);
                    format!("{} {} ({}), {} kg, collider {} ({} part{}), {:.0}% of its box solid", p.body.name(), c.name, stuff.name, r2(mass), s.fit.name(), s.parts.len(), if s.parts.len() == 1 { "" } else { "s" }, s.fill * 100.0)
                }
                None => format!("{} {} ({})", p.body.name(), c.name, stuff.name),
            };
            said.push(format!("clip {id}: {what}"));
            edits.push(m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
        }
        let ok = self.doc.apply("Physics settings", m::Edit::Batch { edits });
        self.check(ok, "")?;
        Ok(said.join("; "))
    }

    /// `joint`: joins a clip to another or to the world.
    pub(super) fn joint(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).ok_or("joint needs \"clip\": the model that hangs, swings or slides")?;
        let mut c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let mut p = Props::of(&c);
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let n = p.joints.len();
            p.joints.clear();
            p.write(&mut c);
            let ok = self.doc.apply("Remove joints", m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
            self.check(ok, "")?;
            return Ok(format!("clip {id}: {n} joints removed"));
        }
        let kind = op.get("kind").or(op.get("type")).and_then(Value::as_str).unwrap_or("hinge").to_ascii_lowercase();
        let known = ["hinge", "ball", "slider", "fixed", "spring", "rope", "rod", "chain", "elastic", "door", "axle", "pin", "socket", "rail", "drawer", "weld", "glue", "bungee", "revolute", "prismatic", "spherical"];
        if !known.contains(&kind.as_str()) {
            return Err(format!("joint kind \"{kind}\": hinge, ball, slider, fixed, spring, rope or rod"));
        }
        let to = op.get("to").and_then(|v| id_of(&self.doc, v)).map(u64::from);
        if let Some(t) = to {
            self.model_clip(t as u32).ok_or_else(|| format!("no clip {t} to join to"))?;
        }
        let pair = |k: &str| -> Option<[f32; 2]> {
            let a = op.get(k)?;
            if let Some(arr) = a.as_array() {
                let v: Vec<f32> = arr.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
                return (v.len() >= 2).then(|| [v[0], v[1]]);
            }
            a.as_f64().map(|x| [x as f32, if k == "motor" { 50.0 } else { 0.3 }])
        };
        let spec = JointSpec { kind: kind.clone(), to, at: op.get("at_point").or(op.get("point")).or(op.get("anchor")).and_then(v3), axis: op.get("axis").and_then(v3), limits: pair("limits"), motor: pair("motor"), spring: pair("spring"), breaks: num(op, "breaks") };
        p.joints.push(spec);
        if p.body == BodyType::Auto {
            p.body = BodyType::Dynamic;
        }
        p.write(&mut c);
        let ok = self.doc.apply("Add joint", m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(format!("clip {id}: {kind} joint to {}; it is dynamic now (bake_physics, drop, push or throw make it move)", to.map_or("the world".to_string(), |t| format!("clip {t}"))))
    }

    fn scene_for(&self, op: &Value, at: view::Time, dynamic: &[u32]) -> Result<Scene, String> {
        let mut b = Build::at(self.doc.seq(), at);
        b.dynamic = dynamic.iter().map(|&d| d as u64).collect();
        b.floor = floor_of(op);
        // Loose props join in: what is hit falls over.
        b.props_move = op.get("props").and_then(Value::as_bool).unwrap_or(true);
        Scene::build(&self.doc.model, &b)
    }

    fn run_for(&self, op: &Value, at: view::Time, default_secs: f64) -> run::Run {
        let mut r = run::Run::new(at, f(op, "duration").unwrap_or(default_secs).clamp(0.1, 60.0) as f32, self.fps());
        r.react = op.get("react").and_then(Value::as_bool).unwrap_or(true);
        r.pieces = op.get("pieces").and_then(Value::as_u64).unwrap_or(18) as usize;
        r
    }

    /// `drop`: clips fall from where they are (lifted `height` px first).
    pub(super) fn drop_op(&mut self, op: &Value) -> Result<String, String> {
        let targets = clips_of(&self.doc, op);
        if targets.is_empty() {
            return Err("drop needs \"clip\" or \"clips\": the models that fall".into());
        }
        let at = self.seconds_at(op, 0.0);
        if let Some(h) = num(op, "height") {
            // Lift first (a key at the start), then fall.
            let mut edits = Vec::new();
            for &id in &targets {
                let mut c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
                c.transform.position.y -= h as f64;
                c.keyframes.remove("transform.position");
                edits.push(m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
            }
            let ok = self.doc.apply("Lift", m::Edit::Batch { edits });
            self.check(ok, "")?;
        }
        let scene = self.scene_for(op, at, &targets)?;
        let mut r = self.run_for(op, at, 4.0);
        if let Some(s) = op.get("spin").and_then(v3) {
            for &id in &targets {
                r.kicks.push(run::Kick { clip: id as u64, at, spin: Some(s), ..Default::default() });
            }
        }
        let secs = r.seconds;
        let (baked, said) = self.bake_run(scene, &r, "Drop")?;
        Ok(self.describe_run(&baked, said, secs))
    }

    /// `throw`: a clip flies off with a speed (`velocity` px/s) or at a
    /// target (`toward`: a clip or point, landing there in `time` s).
    pub(super) fn throw_op(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).ok_or("throw needs \"clip\": what is thrown")?;
        let at = self.seconds_at(op, 0.0);
        let scene = self.scene_for(op, at, &[id])?;
        let ppm = scene.ppm;
        let i = scene.item(id as u64).ok_or("the thrown clip is not on screen at that moment")?;
        let from = scene.items[i].bounds.centre();
        let g = scale(scene.world.gravity, ppm);
        let velocity = match (op.get("velocity").and_then(v3), op.get("toward").or(op.get("at_target")).or(op.get("target"))) {
            (Some(v), _) => v,
            (None, Some(t)) => {
                let target = match v3(t) {
                    Some(p) => p,
                    None => {
                        let tid = id_of(&self.doc, t).ok_or("toward is a clip or a point [x, y, z]")?;
                        let ti = scene.item(tid as u64).ok_or("the target is not in the scene then")?;
                        let b = scene.items[ti].bounds;
                        [b.centre()[0], b.lo[1], b.centre()[2]]
                    }
                };
                let d = sub(target, from);
                let time = num(op, "time").unwrap_or_else(|| (length(d) / ppm / 6.0).clamp(0.35, 2.5));
                // Lands on the target after `time` under gravity.
                sub(scale(d, 1.0 / time), scale(g, 0.5 * time))
            }
            (None, None) => {
                let speed = num(op, "speed").unwrap_or(ppm * 5.0);
                let dir = op.get("direction").and_then(v3).unwrap_or([1.0, -0.6, 0.0]);
                scale(studio_sim::dynamics::math::normalise_or(dir, [1.0, 0.0, 0.0]), speed)
            }
        };
        let mut r = self.run_for(op, at, 3.0);
        r.kicks.push(run::Kick { clip: id as u64, at, velocity: Some(velocity), spin: Some(op.get("spin").and_then(v3).unwrap_or([0.0, 0.0, 180.0])), ..Default::default() });
        let secs = r.seconds;
        let (baked, said) = self.bake_run(scene, &r, "Throw")?;
        Ok(format!("thrown at [{:.0}, {:.0}, {:.0}] px/s; {}", velocity[0], velocity[1], velocity[2], self.describe_run(&baked, said, secs)))
    }

    /// `push`: a sudden shove (`direction` and `strength` px/s, or `from`
    /// a clip or point) at a moment, at a `point` on it (its middle).
    pub(super) fn push_op(&mut self, op: &Value) -> Result<String, String> {
        let targets = clips_of(&self.doc, op);
        let id = *targets.first().ok_or("push needs \"clip\": what is pushed")?;
        let at = self.seconds_at(op, 0.0);
        let scene = self.scene_for(op, at, &targets)?;
        let ppm = scene.ppm;
        let i = scene.item(id as u64).ok_or("the pushed clip is not on screen at that moment")?;
        let b = scene.items[i].bounds;
        let point = op.get("point").and_then(v3).unwrap_or([b.centre()[0], b.lo[1] + (b.hi[1] - b.lo[1]) * 0.25, b.centre()[2]]);
        let dir = match (op.get("direction").and_then(v3), op.get("from")) {
            (Some(d), _) => d,
            (None, Some(f)) => {
                let from = match v3(f) {
                    Some(p) => p,
                    None => {
                        let fid = id_of(&self.doc, f).ok_or("from is a clip or a point")?;
                        let fi = scene.item(fid as u64).ok_or("from is not in the scene then")?;
                        scene.items[fi].bounds.centre()
                    }
                };
                sub(point, from)
            }
            (None, None) => [1.0, 0.0, 0.0],
        };
        let strength = num(op, "strength").or(num(op, "speed")).unwrap_or(ppm * 1.5);
        let shove = scale(studio_sim::dynamics::math::normalise_or(dir, [1.0, 0.0, 0.0]), strength);
        let mut r = self.run_for(op, at, 3.0);
        r.kicks.push(run::Kick { clip: id as u64, at, shove: Some(shove), point: Some(point), ..Default::default() });
        let secs = r.seconds;
        let (baked, said) = self.bake_run(scene, &r, "Push")?;
        Ok(self.describe_run(&baked, said, secs))
    }

    /// `break`: shatters a model at a moment (`at`), from a `point`, or
    /// when something hits it hard enough (`when_hit: true`).
    pub(super) fn break_op(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).ok_or("break needs \"clip\": the model that breaks")?;
        let at = self.seconds_at(op, 0.0);
        let when_hit = op.get("when_hit").and_then(Value::as_bool).unwrap_or(false) || op.get("by").is_some();
        let mut dynamic = vec![id];
        dynamic.extend(ids(&self.doc, op, "by"));
        if when_hit {
            // Breakable from now on, whatever it is made of.
            let mut c = self.model_clip(id).ok_or("gone")?;
            let mut p = Props::of(&c);
            p.breakable = Some(true);
            p.write(&mut c);
            let ok = self.doc.apply("Breakable", m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
            self.check(ok, "")?;
        }
        let scene = self.scene_for(op, at, &dynamic)?;
        let mut r = self.run_for(op, at, 3.0);
        if !when_hit {
            r.breaks.push((id as u64, at, op.get("point").and_then(v3)));
        }
        let secs = r.seconds;
        let (baked, said) = self.bake_run(scene, &r, "Break")?;
        if baked.broken.is_empty() {
            let why = baked.notes.iter().find(|n| n.contains("could not break")).cloned().unwrap_or_else(|| "nothing hit it hard enough".into());
            return Ok(format!("clip {id} did not break: {why}; {}", self.describe_run(&baked, said, secs)));
        }
        Ok(self.describe_run(&baked, said, secs))
    }

    /// `settle`: drops models for real until they stop and leaves them
    /// there (no keys: their place at that moment).
    pub(super) fn settle_op(&mut self, op: &Value) -> Result<String, String> {
        let at = self.seconds_at(op, 0.0);
        let mut targets = clips_of(&self.doc, op);
        if targets.is_empty() {
            // Everything that would not stay as it is.
            targets = crate::physics3d::rest::check(&self.doc.model, self.doc.seq(), at).into_iter().map(|f| f.clip as u32).collect();
            if targets.is_empty() {
                return Ok("everything already rests where it is".into());
            }
        }
        let ends = crate::physics3d::rest::settle(&self.doc.model, self.doc.seq(), &targets.iter().map(|&t| t as u64).collect::<Vec<_>>(), at, f(op, "duration").unwrap_or(6.0) as f32)?;
        let mut edits = Vec::new();
        let mut said = Vec::new();
        for (id, p, r) in ends {
            let mut c = self.model_clip(id as u32).ok_or("gone")?;
            let rel = at - c.start;
            let set = |c: &mut m::Clip, path: &str, v: m::Value| {
                if let Some(tr) = c.keyframes.get_mut(path) {
                    tr.keys.retain(|k| k.time != rel);
                    tr.keys.push(m::Keyframe::new(rel, v, m::Interp::Linear));
                    tr.keys.sort_by_key(|k| k.time);
                    true
                } else {
                    false
                }
            };
            if !set(&mut c, "transform.position", m::Value::Point(m::Vec2::new(p[0], p[1]))) {
                c.transform.position = m::Vec2::new(p[0], p[1]);
            }
            if !set(&mut c, "transform.position_z", m::Value::Scalar(p[2])) {
                c.transform.position_z = p[2];
            }
            for (k, path) in ["transform.rotation_x", "transform.rotation_y", "transform.rotation"].iter().enumerate() {
                if !set(&mut c, path, m::Value::Angle(r[k])) {
                    match k {
                        0 => c.transform.rotation_x = r[0],
                        1 => c.transform.rotation_y = r[1],
                        _ => c.transform.rotation = r[2],
                    }
                }
            }
            c.transform.orientation = [0.0; 3];
            said.push(format!("clip {id} settled at [{:.0}, {:.0}, {:.0}] turned [{:.0}, {:.0}, {:.0}]°", p[0], p[1], p[2], r[0], r[1], r[2]));
            edits.push(m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
        }
        if edits.is_empty() {
            return Ok("they already rest where they are".into());
        }
        let ok = self.doc.apply("Settle", m::Edit::Batch { edits });
        self.check(ok, "")?;
        Ok(said.join("; "))
    }

    /// `ragdoll`: a character goes loose at `at`.
    pub(super) fn ragdoll_op(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).ok_or("ragdoll needs \"clip\": the character")?;
        let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let mesh = load_mesh(&c).ok_or("not a model")?;
        if !mesh.rig.as_ref().is_some_and(|r| r.is_skinned()) {
            return Err(format!("clip {id} has no skinned rig (rig it first, or use drop for a rigid thing)"));
        }
        let at = self.seconds_at(op, 0.0);
        let scene = self.scene_for(op, at, &[])?;
        let mut r = self.run_for(op, at, 3.0);
        r.limp.push((id as u64, at));
        r.strength = num(op, "strength").unwrap_or(0.3).clamp(0.0, 2.0);
        r.fade = num(op, "fade").unwrap_or(0.6).max(0.05);
        if let Some(pu) = op.get("push").and_then(v3) {
            r.kicks.push(run::Kick { clip: id as u64, at, shove: Some(pu), point: op.get("point").and_then(v3), ..Default::default() });
        }
        let secs = r.seconds;
        let (baked, said) = self.bake_run(scene, &r, "Ragdoll")?;
        Ok(self.describe_run(&baked, said, secs))
    }

    /// `bake_physics`: the whole scene from its physics settings.
    pub(super) fn bake_physics(&mut self, op: &Value) -> Result<String, String> {
        let at = self.seconds_at(op, 0.0);
        let extra = clips_of(&self.doc, op);
        let scene = self.scene_for(op, at, &extra)?;
        let dynamic = scene.items.iter().filter(|i| matches!(i.role, Role::Dynamic)).count();
        if dynamic == 0 {
            return Err("nothing in the scene is dynamic: mark props with body {\"type\": \"dynamic\"} (or name them in \"clips\")".into());
        }
        let r = self.run_for(op, at, 5.0);
        let secs = r.seconds;
        let (baked, said) = self.bake_run(scene, &r, "Bake physics")?;
        Ok(format!("{dynamic} dynamic bodies; {}", self.describe_run(&baked, said, secs)))
    }

    /// `physics_world`: scale, gravity and resting on placement.
    pub(super) fn physics_world(&mut self, op: &Value) -> Result<String, String> {
        let mut w = p3::World::of(&self.doc.model);
        if let Some(v) = num(op, "px_per_metre") {
            w.px_per_metre = (v > 0.0).then_some(v);
        }
        if let Some(g) = num(op, "gravity") {
            w.gravity = g;
        }
        if let Some(b) = op.get("rest_on_place").and_then(Value::as_bool) {
            w.rest_on_place = b;
        }
        let mut settings = self.doc.model.settings.clone();
        w.write(&mut settings);
        let ok = self.doc.apply("Physics world", m::Edit::SetSettings { settings: Box::new(settings) });
        self.check(ok, "")?;
        Ok(format!("{} px to the metre{}; gravity {} m/s²; new models {} on what is under them", r1(p3::px_per_metre(&self.doc.model)), if w.px_per_metre.is_none() { " (worked out from the scene)" } else { "" }, w.gravity, if w.rest_on_place { "rest" } else { "stay where put, not resting" }))
    }

    /// `jiggle`: secondary motion baked onto a rigged model.
    pub(super) fn jiggle_op(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).ok_or("jiggle needs \"clip\": the character or creature")?;
        let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let mesh = load_mesh(&c).ok_or("not a model")?;
        let rig = mesh.rig.clone().ok_or_else(|| format!("clip {id} has no rig: rig it first (rig op), or use simulate soft for a whole wobble"))?;
        let at = self.seconds_at(op, 0.0).max(c.start);
        let dur = f(op, "duration").map(micros).unwrap_or(c.start + c.duration - at).max(SEC / 10);
        let fps = self.fps();
        let ppm = p3::px_per_metre(&self.doc.model);
        let only: Vec<String> = match op.get("bones") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            Some(Value::String(s)) if s != "auto" => vec![s.clone()],
            _ => Vec::new(),
        };
        let chains = find_chains(&rig, &only);
        let flesh = num(op, "flesh").unwrap_or(0.0).clamp(0.0, 1.0);
        if chains.is_empty() && flesh <= 0.0 {
            let names: Vec<&str> = rig.bones.iter().map(|b| b.name.as_str()).collect();
            return Err(format!("no loose bones found (tails, ears, antennae, hair, coat tails, feathers...); name some in \"bones\" (this rig has: {}) or set \"flesh\" 0..1 to shake loose skin", names.join(", ")));
        }
        let feel = if op.get("stiffness").is_some() || op.get("damping").is_some() || op.get("gravity").is_some() || op.get("drag").is_some() {
            Some(Feel { stiffness: num(op, "stiffness").unwrap_or(0.25).clamp(0.0, 1.0), damping: num(op, "damping").unwrap_or(0.1).clamp(0.0, 1.0), gravity: num(op, "gravity").unwrap_or(0.5), drag: num(op, "drag").unwrap_or(1.5) })
        } else {
            None
        };
        let g = [0.0, p3::World::of(&self.doc.model).gravity, 0.0];
        let mut said = Vec::new();
        // Bones on springs.
        let step_dt = 1.0 / 120.0;
        let frames_n = ((dur as f32 / 1e6) * fps).round() as usize;
        let mut ch = Chains::new(&rig, chains.clone(), feel, character_place(&c.transform, ppm).k);
        let loose_bones = ch.bones();
        let deform: Vec<usize> = (0..rig.bones.len()).filter(|&i| rig.bones[i].deform && !loose_bones.contains(&i)).collect();
        let mut out_frames: Vec<(view::Time, Vec<studio_mesh::rig::BonePose>)> = Vec::new();
        let substeps = ((1.0 / fps) / step_dt).round().max(1.0) as usize;
        let mut jig = (flesh > 0.0).then(|| Jiggle::new(looseness(&rig, &mesh.positions, &loose_bones, flesh), num(op, "stiffness_hz").unwrap_or(3.0), num(op, "damping").unwrap_or(0.15), 0.08 * mesh_size(&mesh) / ppm));
        let mut verts: Vec<V3> = Vec::new();
        for fr in 0..=frames_n {
            let t = at + micros(fr as f64 / fps as f64);
            let mut last_world: Option<(Vec<studio_mesh::rig::Xf>, studio_sim::dynamics::ragdoll::Placement, studio_mesh::rig::Posed)> = None;
            for s in 0..substeps {
                let ts = t - micros((substeps - 1 - s) as f64 * step_dt as f64);
                let tr = c.transform_at((ts - c.start).clamp(0, c.duration.max(0)));
                let place = character_place(&tr, ppm);
                let posed = posed_rig(&c, &rig, ts - c.start);
                let world: Vec<studio_mesh::rig::Xf> = posed.world.iter().map(|x| {
                    let p = place.frame(x);
                    studio_mesh::rig::Xf { rot: p.q, pos: p.p, scale: 1.0 }
                }).collect();
                let tails: Vec<V3> = rig.bones.iter().enumerate().map(|(i, b)| place.point(posed.world[i].apply([0.0, b.length(), 0.0]))).collect();
                let blockers: Vec<Blocker> = deform.iter().map(|&i| {
                    let b = &rig.bones[i];
                    Blocker { a: world[i].pos, b: tails[i], r: b.length() * place.k * 0.18 }
                }).collect();
                if !ch.is_empty() {
                    ch.step(&world, &tails, g, step_dt, &blockers);
                }
                last_world = Some((world, place, posed));
            }
            let Some((world, place, posed)) = last_world else { continue };
            if !ch.is_empty() {
                let tails: Vec<V3> = rig.bones.iter().enumerate().map(|(i, b)| place.point(posed.world[i].apply([0.0, b.length(), 0.0]))).collect();
                let turns = ch.turns(&world, &tails);
                // World turns back into the model, then into local poses.
                let mut rots: Vec<studio_mesh::rig::Quat> = posed.world.iter().map(|x| x.rot).collect();
                for (b, q) in turns {
                    rots[b] = studio_mesh::rig::q_normalise(studio_mesh::rig::q_mul(studio_mesh::rig::q_conj(place.place.q), q));
                }
                let mut locals = rig.local_from_world(&rots, None);
                // Bones that are not loose keep exactly their animated pose.
                for i in 0..rig.bones.len() {
                    if !loose_bones.contains(&i) {
                        locals[i] = posed.local[i];
                    }
                }
                out_frames.push((t, locals));
            }
            if let Some(j) = jig.as_mut() {
                let (skinned, _) = rig.deform(&mesh, &posed);
                let w: Vec<V3> = skinned.iter().map(|p| place.point(*p)).collect();
                let moved = j.step(&w, g, 1.0 / fps);
                verts.extend(moved.iter().map(|p| {
                    let local = place.place.unapply(*p);
                    scale(local, 1.0 / place.k.max(1e-9))
                }));
            }
        }
        let mut mc = c.clone();
        let mut edits = Vec::new();
        if !out_frames.is_empty() {
            let bt = run::BoneTrack { clip: id as u64, from: at, frames: out_frames };
            let n = run::bone_keys(&mut mc, &rig, &bt, Some(&loose_bones));
            said.push(format!("{} chain{} ({n} bones) follow through: {}", chains.len(), if chains.len() == 1 { "" } else { "s" }, chains.iter().map(|c| rig.bones[c[0]].name.clone()).collect::<Vec<_>>().join(", ")));
        }
        if jig.is_some() && !verts.is_empty() {
            let cache = studio_mesh::cache::Cache { fps, frames: frames_n + 1, data: studio_mesh::cache::Frames::Vertices { count: mesh.positions.len(), data: verts } };
            let file = self.save_cache(&cache)?;
            if let m::ClipSource::Generator(gp) = &mut mc.source {
                gp.params.insert("cache".into(), m::Value::Text(file));
                gp.params.insert("cache_start".into(), m::Value::Scalar((at - c.start) as f64 / 1e6));
            }
            said.push(format!("loose skin shakes ({:.0}% flesh)", flesh * 100.0));
        }
        edits.push(m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(mc) });
        let ok = self.doc.apply("Secondary motion", m::Edit::Batch { edits });
        self.check(ok, "")?;
        Ok(format!("clip {id}: {} from {:.2} s for {:.1} s", said.join("; "), at as f64 / 1e6, dur as f64 / 1e6))
    }

    /// After a model is placed: down onto what is under it (unless the
    /// scene says not to, or it was asked to stay put).
    pub(super) fn rest_placed(&mut self, op: &Value, id: u32) -> Option<String> {
        if op.get("rest").and_then(Value::as_bool) == Some(false) || !p3::World::of(&self.doc.model).rest_on_place {
            return None;
        }
        let c = self.model_clip(id)?;
        let at = c.start;
        let to = crate::physics3d::rest::rest_on_support(&self.doc.model, self.doc.seq(), id as u64, at)?;
        let mut c2 = c.clone();
        let dy = to.y - c.transform.position.y;
        c2.transform.position = to;
        let ok = self.doc.apply("Rest on support", m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c2) });
        if !ok {
            self.doc.error.take();
            return None;
        }
        Some(format!("rests on what is under it ({} {:.0} px; \"rest\": false keeps it where put)", if dy > 0.0 { "down" } else { "up" }, dy.abs()))
    }
}

fn mesh_size(m: &studio_mesh::Mesh) -> f32 {
    let (lo, hi) = m.bounds();
    length(sub(hi, lo)).max(1.0)
}

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests;
