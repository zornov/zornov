// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `simulate`: physics for 3D models, baked so it plays and scrubs.
//!
//! - `rigid`: model clips fall, tumble, bounce and knock each other over;
//!   the result is ordinary position and rotation keys on each clip.
//! - `shatter`: a model breaks into pieces at a moment (or when another
//!   clip hits it) that fly apart and land.
//! - `cloth`: a sheet (made here: `make` cape, flag, curtain, tablecloth,
//!   banner; or an existing model) hangs from pins, drapes over models,
//!   blows in the wind; a cape is pinned to a character's shoulders and
//!   follows the character's moves.
//! - `soft`: a model wobbles, squashes and springs back (jelly, rubber).
//! - `hair`: strands grown on part of a model that swing as it moves.
//! - `particles`: sparks, rain, snow, dust, confetti, embers, magic,
//!   debris, bubbles, a fountain, bouncing off models.
//! - `smoke` / `fire`: a plume on a grid, lit and glowing.
//! - `water`: a pond that ripples from drops and wakes; `ocean`: swell.
//!
//! Each simulation runs in the moving model's own frame (gravity, wind and
//! the colliders carried into it), so a cape on a walking character swings
//! with the walk. Results go to `<project>/Simulations/*.scache` files
//! named by their content; the clip plays one through its `cache` setting.

use super::*;
use std::sync::Arc;
use studio_mesh::cache::{Cache, Frames};
use studio_mesh::{Mesh, V3};
use studio_sim::Collider;

type M34 = [[f64; 4]; 3];

/// A clip's model → world matrix (as the renderer places model layers,
/// without motion presets): scale, turn (orientation, then X, Y, Z),
/// anchor, position.
pub(super) fn model_world(tr: &m::Transform) -> M34 {
    let r1 = studio_sim::rigid::studio_rotation_matrix(tr.orientation[0] as f32, tr.orientation[1] as f32, tr.orientation[2] as f32);
    let r2 = studio_sim::rigid::studio_rotation_matrix(tr.rotation_x as f32, tr.rotation_y as f32, tr.rotation as f32);
    let r: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| r1[i][k] as f64 * r2[k][j] as f64).sum()));
    let k = [tr.scale_x * if tr.flip_x { -1.0 } else { 1.0 }, tr.scale_y * if tr.flip_y { -1.0 } else { 1.0 }, tr.scale_z];
    let pos = [tr.position.x, tr.position.y, tr.position_z];
    let anchor = [tr.anchor.x, tr.anchor.y, tr.anchor_z];
    std::array::from_fn(|row| {
        let m = [r[row][0] * k[0], r[row][1] * k[1], r[row][2] * k[2]];
        [m[0], m[1], m[2], pos[row] - (m[0] * anchor[0] + m[1] * anchor[1] + m[2] * anchor[2])]
    })
}

pub(super) fn apply(m: &M34, p: V3) -> V3 {
    std::array::from_fn(|r| (m[r][0] * p[0] as f64 + m[r][1] * p[1] as f64 + m[r][2] * p[2] as f64 + m[r][3]) as f32)
}

fn apply_dir(m: &M34, v: V3) -> V3 {
    std::array::from_fn(|r| (m[r][0] * v[0] as f64 + m[r][1] * v[1] as f64 + m[r][2] * v[2] as f64) as f32)
}

fn invert(m: &M34) -> M34 {
    let a: [[f64; 3]; 3] = std::array::from_fn(|r| [m[r][0], m[r][1], m[r][2]]);
    let cross = |x: [f64; 3], y: [f64; 3]| [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let c0 = cross(a[1], a[2]);
    let det = dot(a[0], c0);
    if det.abs() < 1e-18 {
        return [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    }
    let c1 = cross(a[2], a[0]);
    let c2 = cross(a[0], a[1]);
    let inv = [[c0[0] / det, c1[0] / det, c2[0] / det], [c0[1] / det, c1[1] / det, c2[1] / det], [c0[2] / det, c1[2] / det, c2[2] / det]];
    let t = [m[0][3], m[1][3], m[2][3]];
    std::array::from_fn(|r| [inv[r][0], inv[r][1], inv[r][2], -dot(inv[r], t)])
}

pub(super) fn v3(v: &Value) -> Option<V3> {
    let a = v.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

pub(super) fn num(op: &Value, k: &str) -> Option<f32> {
    op.get(k).and_then(Value::as_f64).map(|v| v as f32)
}

/// A clip by id or by name (exact, then any case).
pub(super) fn id_of(doc: &crate::doc::Doc, v: &Value) -> Option<u32> {
    if let Some(n) = v.as_u64() {
        return Some(n as u32);
    }
    let name = v.as_str()?;
    let clips = || doc.model.sequences.iter().flat_map(|q| q.clips.iter());
    clips().find(|c| c.name == name).or_else(|| clips().find(|c| c.name.eq_ignore_ascii_case(name))).map(|c| c.id.0 as u32)
}

pub(super) fn ids(doc: &crate::doc::Doc, op: &Value, k: &str) -> Vec<u32> {
    match op.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| id_of(doc, v)).collect(),
        Some(v) => id_of(doc, v).map(|x| vec![x]).unwrap_or_default(),
        None => Vec::new(),
    }
}

/// The model of a clip as it is at sequence time `t` (posed or played).
fn posed(mc: &m::Clip, mesh: &Arc<Mesh>, t: i64) -> Arc<Mesh> {
    #[cfg(feature = "render")]
    {
        let rel = (t - mc.start).clamp(0, mc.duration.max(0));
        let params = studio_render::model_scene::convert_params(&mc.generator_params_at(rel));
        let file = crate::mesh_store::file_of(mc).unwrap_or_default();
        studio_render::model_anim::animate(mesh.clone(), &file, &params, rel as f32 / 1e6).0
    }
    #[cfg(not(feature = "render"))]
    {
        let _ = (mc, t);
        mesh.clone()
    }
}

impl Session {
    /// Where simulations are kept.
    fn sim_dir(&self) -> std::path::PathBuf {
        match self.doc.folder.as_ref() {
            Some(f) => f.root().join("Simulations"),
            None => std::env::temp_dir().join("Zornov Simulations"),
        }
    }

    pub(super) fn save_cache(&self, c: &Cache) -> Result<String, String> {
        let bytes = c.to_bytes();
        let path = self.sim_dir().join(format!("{}.scache", studio_mesh::io::content_hash(&bytes)));
        if !path.exists() {
            c.save(&path)?;
        }
        Ok(path.to_string_lossy().into_owned())
    }

    pub(super) fn fps(&self) -> f32 {
        (self.doc.project.fps as f32).clamp(1.0, 120.0)
    }

    /// A new model clip showing `mesh` (a lane of its own, like `model`).
    pub(super) fn add_model_clip(&mut self, mesh: &Mesh, name: &str, at: i64, dur: i64, tr: Option<&m::Clip>) -> Result<u32, String> {
        let file = crate::mesh_store::save(&self.doc, mesh)?;
        let lane = self.room(None, at, dur, None)?;
        let id = self.doc.alloc_id() as u32;
        let mut c = crate::mesh_store::new_clip(id as u64, lane as u64, at, dur, file, name);
        if let Some(src) = tr {
            // Same place and moves as `src` (hair, a cape: in its frame).
            c.transform = src.transform.clone();
            for (k, v) in &src.keyframes {
                if k.starts_with("transform.") {
                    let mut v = v.clone();
                    v.shift(src.start - at);
                    c.keyframes.insert(k.clone(), v);
                }
            }
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply(&format!("Add {}", name.to_lowercase()), m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(id)
    }

    /// Points a clip at a cache starting at clip time `start` (seconds).
    pub(super) fn play_cache(&mut self, id: u32, file: String, start: f64, extra: &[(&str, m::Value)], what: &str) -> Result<(), String> {
        let mut mc = self.model_clip(id).ok_or("that clip is gone")?;
        if let m::ClipSource::Generator(g) = &mut mc.source {
            g.params.insert("cache".into(), m::Value::Text(file));
            g.params.insert("cache_start".into(), m::Value::Scalar(start));
            for (k, v) in extra {
                g.params.insert((*k).into(), v.clone());
            }
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply(what, m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "").map(|_| ())
    }

    /// Colliders in the world at time `t`: the floor and these clips'
    /// models (posed, placed).
    fn colliders_at(&self, ids: &[u32], floor: Option<f32>, t: i64) -> Result<Vec<Collider>, String> {
        let mut out = Vec::new();
        if let Some(y) = floor {
            out.push(Collider::Ground { y, friction: 0.6 });
        }
        for &id in ids {
            let (mc, mesh) = self.model_of(id)?;
            let mesh = posed(&mc, &Arc::new(mesh), t);
            let mw = model_world(&mc.transform_at((t - mc.start).clamp(0, mc.duration.max(0))));
            let tris: Vec<[V3; 3]> = (0..mesh.triangles()).map(|k| mesh.tri(k).map(|v| apply(&mw, mesh.positions[v]))).collect();
            if !tris.is_empty() {
                out.push(Collider::mesh(tris));
            }
        }
        Ok(out)
    }

    /// The floor: false for none, a number (world y), else the bottom of
    /// the frame.
    fn floor(&self, op: &Value) -> Option<f32> {
        match op.get("floor") {
            Some(Value::Bool(false)) => None,
            Some(v) if v.is_number() => v.as_f64().map(|y| y as f32),
            _ => Some(self.doc.project.height as f32 / 2.0),
        }
    }

    pub(super) fn simulate(&mut self, op: &Value) -> Result<String, String> {
        let kind = op.get("kind").and_then(Value::as_str).ok_or("simulate needs \"kind\": rigid, shatter, cloth, soft, hair, particles, smoke, fire, water or ocean")?;
        match kind {
            "rigid" | "rigid_body" | "bodies" | "drop" => self.sim_rigid(op),
            "shatter" | "fracture" | "break" | "destroy" => {
                // The physics engine's break: pieces fly with the scene.
                let mut v = op.clone();
                if let Some(o) = v.as_object_mut() {
                    if let Some(p) = o.remove("impact") {
                        o.insert("point".into(), p);
                    }
                    o.entry("at").or_insert(json!(0.5));
                }
                self.break_op(&v)
            }
            "cloth" | "cape" | "flag" | "curtain" => self.sim_cloth(op),
            "soft" | "soft_body" | "jelly" => self.sim_soft(op),
            "hair" | "fur" => self.sim_hair(op),
            "particles" | "particle" => self.sim_particles(op),
            "smoke" | "fire" => self.sim_smoke(op, kind == "fire"),
            "water" | "pond" | "ripples" => self.sim_water(op),
            "ocean" | "sea" | "waves" => self.sim_ocean(op),
            other => Err(format!("kind \"{other}\": rigid, shatter, cloth, soft, hair, particles, smoke, fire, water or ocean")),
        }
    }

    /// Rigid bodies on the physics engine: `clips` move, `static` and
    /// `colliders` stay put (everything else on screen collides as it is).
    fn sim_rigid(&mut self, op: &Value) -> Result<String, String> {
        let bodies = ids(&self.doc, op, "clips");
        if bodies.is_empty() {
            return Err("rigid needs \"clips\": [model clip ids] that fall and collide".into());
        }
        // Bounce and friction asked for here become the clips' own settings.
        if op.get("bounce").is_some() || op.get("friction").is_some() {
            let mut edits = Vec::new();
            for &id in &bodies {
                let mut c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
                let mut p = crate::physics3d::Props::of(&c);
                if let Some(b) = num(op, "bounce") {
                    p.bounce = Some(b.clamp(0.0, 1.0));
                }
                if let Some(fr) = num(op, "friction") {
                    p.friction = Some(fr.clamp(0.0, 2.0));
                }
                p.write(&mut c);
                edits.push(m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
            }
            let ok = self.doc.apply("Physics settings", m::Edit::Batch { edits });
            self.check(ok, "")?;
        }
        let mut statics = ids(&self.doc, op, "static");
        statics.extend(ids(&self.doc, op, "colliders"));
        if !statics.is_empty() {
            let mut edits = Vec::new();
            for &id in &statics {
                let mut c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
                let mut p = crate::physics3d::Props::of(&c);
                if p.body == crate::physics3d::BodyType::Auto {
                    p.body = crate::physics3d::BodyType::Static;
                }
                p.write(&mut c);
                edits.push(m::Edit::SetClip { seq: self.doc.seq(), clip: Box::new(c) });
            }
            let ok = self.doc.apply("Physics settings", m::Edit::Batch { edits });
            self.check(ok, "")?;
        }
        // Per-clip pushes and spins, or one for all.
        let at = f(op, "at").unwrap_or(0.0);
        let per = |key: &str, id: u32| op.get(key).and_then(|x| x.get(id.to_string()).or(if x.is_array() { Some(x) } else { None })).and_then(v3);
        let mut b = crate::physics3d::scene::Build::at(self.doc.seq(), micros(at));
        b.dynamic = bodies.iter().map(|&i| i as u64).collect();
        b.floor = match op.get("floor") {
            Some(Value::Bool(false)) => crate::physics3d::scene::Floor::Off,
            Some(x) if x.is_number() => crate::physics3d::scene::Floor::At(x.as_f64().unwrap_or(0.0) as f32),
            _ => crate::physics3d::scene::Floor::Auto,
        };
        let scene = crate::physics3d::scene::Scene::build(&self.doc.model, &b)?;
        let mut r = crate::physics3d::run::Run::new(micros(at), f(op, "duration").unwrap_or(3.0).clamp(0.1, 60.0) as f32, self.fps());
        for &id in &bodies {
            let (vel, spin) = (per("push", id), per("spin", id));
            if vel.is_some() || spin.is_some() {
                r.kicks.push(crate::physics3d::run::Kick { clip: id as u64, at: micros(at), velocity: vel, spin, ..Default::default() });
            }
        }
        let baked = crate::physics3d::run::simulate(&self.doc.model, scene, &r);
        let save_model = |mm: &Mesh| crate::mesh_store::save(&self.doc, mm);
        let save_cache = |c: &Cache| self.save_cache(c);
        let (edits, said) = crate::physics3d::run::bake(&self.doc.model, self.doc.seq(), &baked, &save_model, &save_cache)?;
        if !edits.is_empty() {
            let ok = self.doc.apply("Rigid bodies", m::Edit::Batch { edits });
            self.check(ok, "")?;
        }
        Ok(format!("simulated {:.1} s from {at:.2} s: {}", r.seconds, if said.is_empty() { "nothing moved".to_string() } else { said.join(", ") }))
    }

    fn sim_cloth(&mut self, op: &Value) -> Result<String, String> {
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or(4.0).clamp(0.2, 60.0);
        let fabric_name = op.get("fabric").and_then(Value::as_str).unwrap_or("cotton");
        let fabric = studio_sim::cloth::Fabric::named(fabric_name).ok_or("fabric is silk, cotton, denim, canvas, leather, rubber or flag")?;
        let make = op.get("make").and_then(Value::as_str).or(match op.get("kind").and_then(Value::as_str) {
            Some(k @ ("cape" | "flag" | "curtain")) => Some(k),
            _ => None,
        });
        let wearer = op.get("on").or(op.get("attach")).and_then(|v| id_of(&self.doc, v));
        let colour = op.get("colour").and_then(Value::as_str);
        // The cloth: made here, or an existing model.
        let (id, mut said) = match (op.get("clip").and_then(|v| id_of(&self.doc, v)).map(u64::from), make) {
            (Some(id), _) => (id as u32, Vec::new()),
            (None, Some(kind)) => {
                let h = self.doc.project.height as f32;
                let (mesh, name, follow) = match (kind, wearer) {
                    ("cape", Some(who)) => {
                        let (wc, wm) = self.model_of(who)?;
                        let (lo, hi) = wm.bounds();
                        let height = hi[1] - lo[1];
                        let width = (hi[0] - lo[0]) * 0.55;
                        let mut sheet = studio_sim::cloth::sheet(width, height * 0.55, 28);
                        // Behind the shoulders, hanging down the back.
                        let top = lo[1] + height * 0.19;
                        let back = wm.positions.iter().filter(|p| (p[1] - top).abs() < height * 0.05).map(|p| p[2]).fold(f32::NEG_INFINITY, f32::max);
                        studio_mesh::ops::transform(&mut sheet, &studio_mesh::region::Region::All, [0.0, top + height * 0.275, back + height * 0.015], [0.0; 3], [1.0; 3], [0.0; 3]);
                        (sheet, "Cape", Some(wc))
                    }
                    ("cape", None) => return Err("a cape needs \"on\": the character clip".into()),
                    ("flag" | "banner", _) => (studio_sim::cloth::sheet(h * 0.5, h * 0.3, 30), "Flag", None),
                    ("tablecloth", _) => {
                        let mut s = studio_sim::cloth::sheet(h * 0.6, h * 0.6, 36);
                        studio_mesh::ops::transform(&mut s, &studio_mesh::region::Region::All, [0.0; 3], [90.0, 0.0, 0.0], [1.0; 3], [0.0; 3]);
                        (s, "Tablecloth", None)
                    }
                    _ => (studio_sim::cloth::sheet(h * 0.6, h * 0.8, 32), "Curtain", None),
                };
                let mut mesh = mesh;
                if let Some(c) = colour.and_then(super::craft::colour) {
                    mesh.materials[0].base_colour = [studio_mesh::srgb_to_linear(c.r()), studio_mesh::srgb_to_linear(c.g()), studio_mesh::srgb_to_linear(c.b()), 1.0];
                }
                mesh.materials[0].name = name.into();
                let end = self.doc.project.duration().max(micros(at + dur));
                let id = self.add_model_clip(&mesh, name, micros(at), end - micros(at), follow.as_ref())?;
                // Placement for things made here: where asked, in the frame.
                if follow.is_none() {
                    let mut mc = self.model_clip(id).ok_or("gone")?;
                    if let Some(p) = op.get("position").and_then(v3) {
                        mc.transform.position = m::Vec2::new(p[0] as f64, p[1] as f64);
                        mc.transform.position_z = p[2] as f64;
                    }
                    let seq = self.doc.seq();
                    let ok = self.doc.apply("Place cloth", m::Edit::SetClip { seq, clip: Box::new(mc) });
                    self.check(ok, "")?;
                }
                (id, vec![format!("{} made as clip {id}", name.to_lowercase())])
            }
            (None, None) => return Err("cloth needs \"clip\" (a model to simulate) or \"make\": cape, flag, curtain or tablecloth".into()),
        };
        let (mc, mesh) = self.model_of(id)?;
        let mut cloth = studio_sim::cloth::Cloth::new(&mesh, fabric);
        // Pins.
        let band = {
            let (lo, hi) = mesh.bounds();
            ((hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.012).max(0.5)
        };
        let pin = op.get("pin").and_then(Value::as_str).unwrap_or(match make {
            Some("flag" | "banner") => "left",
            Some("curtain" | "cape") => "top",
            _ => "",
        });
        if !pin.is_empty() && pin != "none" {
            let n = cloth.pin_side(pin, band);
            said.push(format!("{n} points pinned along the {pin}"));
        }
        if let Some(Value::Array(points)) = op.get("pins") {
            for p in points.iter().filter_map(v3) {
                cloth.pin_near(p, num(op, "pin_radius").unwrap_or(band * 2.0));
            }
        }
        // A wearer: pins ride on the nearest skin, the body collides.
        let wearer_clip = wearer.map(|w| self.model_of(w)).transpose()?;
        let pin_rides: Vec<(usize, usize, V3)> = match &wearer_clip {
            Some((_, wm)) => {
                let grid = studio_mesh::rig::PointGrid::new(&wm.positions);
                cloth
                    .pins
                    .iter()
                    .map(|(i, p)| {
                        let j = grid.nearest(&wm.positions, *p);
                        (*i as usize, j, studio_mesh::sub(*p, wm.positions[j]))
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let colliders = ids(&self.doc, op, "colliders");
        let floor = self.floor(op);
        let wind_world = op.get("wind").and_then(v3).unwrap_or(if make == Some("flag") { [400.0, 0.0, 60.0] } else { [0.0; 3] });
        let gusts = num(op, "gusts").unwrap_or(0.4);
        let frames = (dur as f32 * fps).round() as usize;
        let mut data: Vec<V3> = Vec::with_capacity((frames + 1) * mesh.positions.len());
        let mut prev_world: Option<(M34, M34)> = None;
        for fr in 0..=frames {
            let t = mc.start.max(micros(at)) + micros(fr as f64 / fps as f64);
            let mw = model_world(&mc.transform_at((t - mc.start).clamp(0, mc.duration.max(0))));
            let inv = invert(&mw);
            // Gravity, wind and the frame's own acceleration, in model terms.
            let mut g = [0.0, num(op, "gravity").unwrap_or(self.doc.project.height as f32 * 2.2), 0.0];
            if let Some((p1, p0)) = prev_world {
                let a: [f64; 3] = std::array::from_fn(|i| (mw[i][3] - 2.0 * p1[i][3] + p0[i][3]) * (fps as f64) * (fps as f64));
                g = [g[0] - a[0] as f32, g[1] - a[1] as f32, g[2] - a[2] as f32];
            }
            prev_world = Some((mw, prev_world.map_or(mw, |p| p.0)));
            let g_local = apply_dir(&inv, g);
            let wind = studio_sim::cloth::Wind { velocity: apply_dir(&inv, wind_world), gusts, seed: 3 };
            let mut cols: Vec<Collider> = Vec::new();
            for c in self.colliders_at(&colliders, None, t)? {
                if let Collider::Mesh { bvh, .. } = &c {
                    cols.push(Collider::mesh(bvh.tris.iter().map(|tr| tr.map(|p| apply(&inv, p))).collect()));
                }
            }
            if let Some(y) = floor {
                let s = 1e5;
                let quad = [[-s, y, -s], [s, y, -s], [s, y, s], [-s, y, s]].map(|p| apply(&inv, p));
                cols.push(Collider::mesh(vec![[quad[0], quad[2], quad[1]], [quad[0], quad[3], quad[2]]]));
            }
            if let Some((wc, wm)) = &wearer_clip {
                // The wearer posed now, in the cloth's frame (same place).
                let posed_now = posed(wc, &Arc::new(wm.clone()), t);
                let ww = model_world(&wc.transform_at((t - wc.start).clamp(0, wc.duration.max(0))));
                let to_here = |p: V3| apply(&inv, apply(&ww, p));
                cols.push(Collider::mesh((0..posed_now.triangles()).map(|k| posed_now.tri(k).map(|v| to_here(posed_now.positions[v]))).collect()));
                cloth.pins = pin_rides.iter().map(|(i, j, d)| (*i as u32, studio_mesh::add(to_here(posed_now.positions[*j]), *d))).collect();
            }
            if fr > 0 {
                cloth.step(1.0 / fps, 10, g_local, &wind, fr as f32 / fps, &cols);
            }
            data.extend(cloth.vertices());
        }
        let cache = Cache { fps, frames: frames + 1, data: Frames::Vertices { count: mesh.positions.len(), data } };
        let file = self.save_cache(&cache)?;
        let start = ((micros(at) - mc.start) as f64 / 1e6).max(0.0);
        self.play_cache(id, file, start, &[], "Cloth")?;
        said.push(format!("{} cloth simulated {dur:.1} s", fabric_name));
        Ok(format!("clip {id}: {}", said.join("; ")))
    }

    fn sim_soft(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).map(u64::from).ok_or("soft needs \"clip\" (the model that wobbles)")? as u32;
        let (mc, mesh) = self.model_of(id)?;
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or(3.0).clamp(0.2, 60.0);
        let jelly = studio_sim::soft::Jelly {
            stiffness: num(op, "stiffness").unwrap_or(0.3),
            squash: num(op, "squash").unwrap_or(0.4),
            damping: num(op, "damping").unwrap_or(0.05),
            clusters: op.get("clusters").and_then(Value::as_u64).unwrap_or(1) as usize,
            ..Default::default()
        };
        let mut s = studio_sim::soft::Soft::new(&mesh, jelly);
        let mw = model_world(&mc.transform_at((micros(at) - mc.start).clamp(0, mc.duration.max(0))));
        let inv = invert(&mw);
        if let Some(v) = op.get("push").and_then(v3) {
            s.push(apply_dir(&inv, v));
        }
        let g = apply_dir(&inv, [0.0, num(op, "gravity").unwrap_or(self.doc.project.height as f32 * 2.2), 0.0]);
        let mut cols: Vec<Collider> = Vec::new();
        for c in self.colliders_at(&ids(&self.doc, op, "colliders"), None, micros(at))? {
            if let Collider::Mesh { bvh, .. } = &c {
                cols.push(Collider::mesh(bvh.tris.iter().map(|tr| tr.map(|p| apply(&inv, p))).collect()));
            }
        }
        if let Some(y) = self.floor(op) {
            let sz = 1e5;
            let quad = [[-sz, y, -sz], [sz, y, -sz], [sz, y, sz], [-sz, y, sz]].map(|p| apply(&inv, p));
            cols.push(Collider::mesh(vec![[quad[0], quad[2], quad[1]], [quad[0], quad[3], quad[2]]]));
        }
        let frames = (dur as f32 * fps).round() as usize;
        let mut data = Vec::with_capacity((frames + 1) * mesh.positions.len());
        for fr in 0..=frames {
            if fr > 0 {
                s.step(1.0 / fps, 6, g, &cols);
            }
            data.extend(s.vertices());
        }
        let cache = Cache { fps, frames: frames + 1, data: Frames::Vertices { count: mesh.positions.len(), data } };
        let file = self.save_cache(&cache)?;
        self.play_cache(id, file, ((micros(at) - mc.start) as f64 / 1e6).max(0.0), &[], "Soft body")?;
        Ok(format!("clip {id}: soft body simulated {dur:.1} s"))
    }

    fn sim_hair(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| id_of(&self.doc, v)).map(u64::from).ok_or("hair needs \"clip\" (the model it grows on)")? as u32;
        let (mc, mesh) = self.model_of(id)?;
        let region = super::models::region(op.get("region"))?;
        let faces = region.faces(&mesh);
        if faces.is_empty() {
            return Err("the region holds no faces to grow hair on".into());
        }
        let (lo, hi) = mesh.bounds();
        let size = (hi[1] - lo[1]).max(hi[0] - lo[0]);
        let colour = match op.get("colour").and_then(Value::as_str) {
            Some(c) => {
                let k = super::craft::colour(c).ok_or_else(|| format!("colour \"{c}\" is not #rrggbb or a basic name"))?;
                [studio_mesh::srgb_to_linear(k.r()), studio_mesh::srgb_to_linear(k.g()), studio_mesh::srgb_to_linear(k.b())]
            }
            None => [0.08, 0.05, 0.03],
        };
        let groom = studio_sim::hair::Groom {
            count: op.get("count").and_then(Value::as_u64).unwrap_or(2000).min(100_000) as usize,
            length: num(op, "length").unwrap_or(size * 0.15),
            segments: op.get("segments").and_then(Value::as_u64).unwrap_or(6) as usize,
            width: num(op, "width").unwrap_or((size * 0.004).max(0.6)),
            curl: num(op, "curl").unwrap_or(0.0),
            stiffness: num(op, "stiffness").unwrap_or(0.5),
            comb: num(op, "comb").unwrap_or(0.6),
            seed: op.get("seed").and_then(Value::as_u64).unwrap_or(1),
            colour,
        };
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or((mc.duration as f64 / 1e6 - at).max(0.5)).clamp(0.2, 120.0);
        let mw0 = model_world(&mc.transform_at(0));
        let g0 = apply_dir(&invert(&mw0), [0.0, self.doc.project.height as f32 * 2.2, 0.0]);
        let mut hair = studio_sim::hair::Hair::grow(&mesh, &faces, groom, g0);
        let geometry = hair.geometry();
        let base = Arc::new(mesh.clone());
        let frames = (dur as f32 * fps).round() as usize;
        let mut data = Vec::with_capacity((frames + 1) * geometry.positions.len());
        let wind_world = op.get("wind").and_then(v3).unwrap_or([0.0; 3]);
        let mut prev: Option<(M34, M34)> = None;
        for fr in 0..=frames {
            let t = mc.start + micros(at + fr as f64 / fps as f64);
            let mw = model_world(&mc.transform_at((t - mc.start).clamp(0, mc.duration.max(0))));
            let inv = invert(&mw);
            let mut g = [0.0, self.doc.project.height as f32 * 2.2, 0.0];
            if let Some((p1, p0)) = prev {
                let a: [f64; 3] = std::array::from_fn(|i| (mw[i][3] - 2.0 * p1[i][3] + p0[i][3]) * (fps as f64).powi(2));
                g = [g[0] - a[0] as f32, g[1] - a[1] as f32, g[2] - a[2] as f32];
            }
            prev = Some((mw, prev.map_or(mw, |p| p.0)));
            let now = posed(&mc, &base, t);
            let head = studio_sim::Collider::mesh((0..now.triangles()).map(|k| now.tri(k).map(|v| now.positions[v])).collect());
            if fr > 0 {
                hair.step(&now.positions, &mesh, 1.0 / fps, 3, apply_dir(&inv, g), &studio_sim::cloth::Wind { velocity: apply_dir(&inv, wind_world), gusts: 0.3, seed: 5 }, fr as f32 / fps, std::slice::from_ref(&head));
            } else {
                // Settle the grown strands for a moment first.
                for _ in 0..20 {
                    hair.step(&now.positions, &mesh, 1.0 / fps, 2, apply_dir(&inv, g), &studio_sim::cloth::Wind::default(), 0.0, std::slice::from_ref(&head));
                }
            }
            data.extend(hair.vertices());
        }
        let end = self.doc.project.duration().max(mc.start + micros(at + dur));
        let hid = self.add_model_clip(&geometry, "Hair", mc.start, end - mc.start, Some(&mc))?;
        let cache = Cache { fps, frames: frames + 1, data: Frames::Vertices { count: geometry.positions.len(), data } };
        let file = self.save_cache(&cache)?;
        self.play_cache(hid, file, at, &[], "Hair")?;
        Ok(format!("hair clip {hid}: {} strands on clip {id}, simulated {dur:.1} s", hair.roots.len()))
    }

    fn sim_particles(&mut self, op: &Value) -> Result<String, String> {
        let preset = op.get("preset").and_then(Value::as_str).unwrap_or("sparks");
        let at_pos = op.get("position").and_then(v3).unwrap_or([0.0; 3]);
        let size = num(op, "size").unwrap_or(self.doc.project.height as f32 * 0.6);
        let mut e = studio_sim::particles::preset(preset, at_pos, size).ok_or_else(|| format!("preset \"{preset}\": {}", studio_sim::particles::PRESETS.join(", ")))?;
        if let Some(r) = num(op, "rate") {
            e.rate = r.max(0.0);
        }
        if let Some(n) = op.get("burst").and_then(Value::as_u64) {
            e.burst = n as usize;
        }
        if let Some(l) = num(op, "life") {
            e.life = l.max(0.05);
        }
        if let Some(v) = op.get("velocity").and_then(v3) {
            e.velocity = v;
        }
        if let Some(c) = op.get("colour").and_then(Value::as_str).and_then(super::craft::colour) {
            e.colour = [c.r(), c.g(), c.b(), c.a()];
            e.colour_end = [c.r(), c.g(), c.b(), if preset == "dust" || preset == "magic" { 0 } else { c.a() }];
        }
        if let Some(w) = op.get("wind").and_then(v3) {
            e.wind = w;
        }
        e.seed = op.get("seed").and_then(Value::as_u64).unwrap_or(1);
        if let Some(src) = op.get("emit_from").and_then(|v| id_of(&self.doc, v)) {
            let (sc, sm) = self.model_of(src as u32)?;
            let mw = model_world(&sc.transform_at(0));
            let tris: Vec<[V3; 3]> = (0..sm.triangles()).map(|k| sm.tri(k).map(|v| apply(&mw, sm.positions[v]))).collect();
            let normals = tris.iter().map(|t| studio_mesh::normalise(studio_mesh::cross(studio_mesh::sub(t[1], t[0]), studio_mesh::sub(t[2], t[0])))).collect();
            e.source = studio_sim::particles::Source::Surface { tris, normals };
        }
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or(4.0).clamp(0.1, 120.0);
        if let Some(d) = f(op, "emit_for") {
            e.duration = d as f32;
        }
        let cols = self.colliders_at(&ids(&self.doc, op, "colliders"), self.floor(op), micros(at))?;
        let frames = studio_sim::particles::run(&e, [0.0, self.doc.project.height as f32 * 2.2, 0.0], &cols, dur as f32, fps);
        let most = frames.iter().map(Vec::len).max().unwrap_or(0);
        let cache = Cache { fps, frames: frames.len(), data: Frames::Points(frames) };
        let file = self.save_cache(&cache)?;
        // Each particle is a copy of the clip's model: a little ball, or a
        // flake for confetti.
        let shape = match preset {
            "confetti" => {
                let mut q = studio_mesh::prim::cube([1.0, 0.6, 0.05]);
                q.materials[0].roughness = 0.6;
                q
            }
            "rain" => studio_mesh::prim::capsule(0.25, 1.0, 5, 2),
            _ => {
                let mut b = studio_mesh::prim::ico_sphere(0.5, 1);
                b.materials[0].roughness = if preset == "bubbles" { 0.05 } else { 0.5 };
                if preset == "bubbles" || preset == "fountain" {
                    b.materials[0].transmission = 0.9;
                    b.materials[0].ior = 1.33;
                }
                b
            }
        };
        let mut shape = shape;
        if preset != "bubbles" {
            shape.materials[0].transmission = 0.0;
        }
        let end = micros(at + dur);
        // Particles are already in the world: the clip sits at the origin.
        let id = self.add_model_clip(&shape, &format!("{}{}", &preset[..1].to_uppercase(), &preset[1..]), micros(at), end - micros(at), None)?;
        let mut mc = self.model_clip(id).ok_or("gone")?;
        mc.transform.position = m::Vec2::new(0.0, 0.0);
        mc.transform.position_z = 0.0;
        if let Some(mat) = mc.material.as_mut() {
            // Thousands of tiny shadows only add noise.
            mat.casts_shadows = matches!(preset, "debris" | "confetti");
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Place particles", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        self.play_cache(id, file, 0.0, &[], "Particles")?;
        Ok(format!("{preset} particles as clip {id}: up to {most} at once over {dur:.1} s"))
    }

    fn sim_smoke(&mut self, op: &Value, fire: bool) -> Result<String, String> {
        let pos = op.get("position").and_then(v3).unwrap_or([0.0, self.doc.project.height as f32 * 0.3, 0.0]);
        let size = num(op, "size").unwrap_or(self.doc.project.height as f32 * 0.7);
        let res = op.get("resolution").and_then(Value::as_u64).unwrap_or(48).clamp(12, 128) as usize;
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or(4.0).clamp(0.2, 30.0);
        // A tall box: the source near its bottom, room to rise.
        let half = [size * 0.35, size * 0.5, size * 0.35];
        let centre = [pos[0], pos[1] - size * 0.38, pos[2]];
        let min = studio_mesh::sub(centre, half);
        let max = studio_mesh::add(centre, half);
        let mut grid = studio_sim::smoke::Grid::new(min, max, res);
        let blockers = ids(&self.doc, op, "colliders");
        if !blockers.is_empty() {
            let cols = self.colliders_at(&blockers, None, micros(at))?;
            grid.block(|p| cols.iter().any(|c| c.push_out(p, 0.0).is_some_and(|k| studio_mesh::dot(studio_mesh::sub(k.point, p), k.normal) > 0.0)));
        }
        let r = num(op, "radius").unwrap_or(size * 0.06);
        let src = studio_sim::smoke::Source {
            centre: pos,
            radius: r,
            smoke: num(op, "smoke").unwrap_or(if fire { 0.6 } else { 1.2 }),
            heat: num(op, "heat").unwrap_or(if fire { 9.0 } else { 3.0 }),
            velocity: op.get("velocity").and_then(v3).unwrap_or([0.0, -size * 0.2, 0.0]),
            from: 0.0,
            to: f(op, "emit_for").map_or(f32::INFINITY, |v| v as f32),
        };
        let air = studio_sim::smoke::Air { wind: op.get("wind").and_then(v3).unwrap_or([0.0; 3]), vorticity: num(op, "swirl").unwrap_or(2.5), ..Default::default() };
        let cache = studio_sim::smoke::bake(grid, &[src], &air, dur as f32, fps);
        let file = self.save_cache(&cache)?;
        let empty = {
            let mut m = Mesh::default();
            m.materials.push(studio_mesh::Material { name: if fire { "Fire" } else { "Smoke" }.into(), ..Default::default() });
            m
        };
        let end = micros(at + dur);
        let id = self.add_model_clip(&empty, if fire { "Fire" } else { "Smoke" }, micros(at), end - micros(at), None)?;
        let colour = match op.get("colour").and_then(Value::as_str).and_then(super::craft::colour) {
            Some(c) => m::Value::Colour(m::Rgba::new(c.r() as f64 / 255.0, c.g() as f64 / 255.0, c.b() as f64 / 255.0, 1.0)),
            None => m::Value::Colour(if fire { m::Rgba::new(0.25, 0.22, 0.2, 1.0) } else { m::Rgba::new(0.75, 0.75, 0.78, 1.0) }),
        };
        let mut mc = self.model_clip(id).ok_or("gone")?;
        mc.transform.position = m::Vec2::new(0.0, 0.0);
        mc.transform.position_z = 0.0;
        let seq = self.doc.seq();
        let ok = self.doc.apply("Place smoke", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let fire_k = num(op, "fire").unwrap_or(if fire { 1.5 } else { 0.0 }) as f64;
        self.play_cache(id, file, 0.0, &[("smoke_colour", colour), ("smoke_density", m::Value::Scalar(num(op, "density").unwrap_or(if fire { 0.03 } else { 0.05 }) as f64)), ("fire", m::Value::Scalar(fire_k))], if fire { "Fire" } else { "Smoke" })?;
        Ok(format!("{} as clip {id}: {:?} grid, {dur:.1} s", if fire { "fire" } else { "smoke" }, cache_dims(&cache)))
    }

    fn sim_water(&mut self, op: &Value) -> Result<String, String> {
        let size = num(op, "size").unwrap_or(self.doc.project.width as f32 * 1.5);
        let cells = op.get("cells").and_then(Value::as_u64).unwrap_or(120) as usize;
        let surface = studio_sim::water::surface([size, size], cells);
        let mut pond = studio_sim::water::Pond::new(&surface, [size, size]);
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or(4.0).clamp(0.2, 60.0);
        let mut touches = Vec::new();
        if let Some(Value::Array(drops)) = op.get("drops") {
            for d in drops {
                let p = d.get("at").and_then(Value::as_array).map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32]).unwrap_or([0.0, 0.0]);
                let t = d.get("t").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                touches.push(studio_sim::water::Disturbance { at: p, to: p, from_t: t, until: t + 0.05, radius: num(d, "radius").unwrap_or(size * 0.02), depth: num(d, "depth").unwrap_or(size * 0.01) });
            }
        }
        if let Some(Value::Array(wakes)) = op.get("wakes") {
            for w in wakes {
                let pt = |k: &str| w.get(k).and_then(Value::as_array).map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32]).unwrap_or([0.0, 0.0]);
                touches.push(studio_sim::water::Disturbance { at: pt("from"), to: pt("to"), from_t: w.get("t0").and_then(Value::as_f64).unwrap_or(0.0) as f32, until: w.get("t1").and_then(Value::as_f64).unwrap_or(dur) as f32, radius: num(w, "radius").unwrap_or(size * 0.015), depth: num(w, "depth").unwrap_or(size * 0.006) });
            }
        }
        if touches.is_empty() {
            touches.push(studio_sim::water::Disturbance { at: [0.0, 0.0], to: [0.0, 0.0], from_t: 0.2, until: 0.25, radius: size * 0.02, depth: size * 0.01 });
        }
        let frames = (dur as f32 * fps).round() as usize;
        let mut data = Vec::with_capacity((frames + 1) * surface.positions.len());
        for fr in 0..=frames {
            if fr > 0 {
                pond.step(1.0 / fps, fr as f32 / fps, &touches);
            }
            data.extend(pond.vertices(&surface));
        }
        self.water_clip(&surface, data, frames, fps, at, dur, op, "Water")
    }

    fn sim_ocean(&mut self, op: &Value) -> Result<String, String> {
        let size = num(op, "size").unwrap_or(self.doc.project.width as f32 * 4.0);
        let surface = studio_sim::water::surface([size, size], op.get("cells").and_then(Value::as_u64).unwrap_or(160) as usize);
        let height = num(op, "height").unwrap_or(size * 0.01);
        let length = num(op, "wavelength").unwrap_or(size * 0.15);
        let wind = op.get("wind").and_then(Value::as_array).map(|a| [a.first().and_then(Value::as_f64).unwrap_or(1.0) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32]).unwrap_or([0.0, 1.0]);
        let waves = studio_sim::water::sea(wind, height, length, op.get("seed").and_then(Value::as_u64).unwrap_or(1));
        let fps = self.fps();
        let at = f(op, "at").unwrap_or(0.0);
        let dur = f(op, "duration").unwrap_or(6.0).clamp(0.2, 120.0);
        let frames = (dur as f32 * fps).round() as usize;
        let g = self.doc.project.height as f32 * 2.2;
        let mut data = Vec::with_capacity((frames + 1) * surface.positions.len());
        for fr in 0..=frames {
            let t = fr as f32 / fps;
            data.extend(surface.positions.iter().map(|p| studio_sim::water::swell_at(*p, t, &waves, g)));
        }
        self.water_clip(&surface, data, frames, fps, at, dur, op, "Ocean")
    }

    #[allow(clippy::too_many_arguments)]
    fn water_clip(&mut self, surface: &Mesh, data: Vec<V3>, frames: usize, fps: f32, at: f64, dur: f64, op: &Value, name: &str) -> Result<String, String> {
        let cache = Cache { fps, frames: frames + 1, data: Frames::Vertices { count: surface.positions.len(), data } };
        let file = self.save_cache(&cache)?;
        let end = self.doc.project.duration().max(micros(at + dur));
        let id = self.add_model_clip(surface, name, micros(at), end - micros(at), None)?;
        let mut mc = self.model_clip(id).ok_or("gone")?;
        let p = op.get("position").and_then(v3).unwrap_or([0.0, self.doc.project.height as f32 * 0.3, self.doc.project.width as f32 * 0.5]);
        mc.transform.position = m::Vec2::new(p[0] as f64, p[1] as f64);
        mc.transform.position_z = p[2] as f64;
        let seq = self.doc.seq();
        let ok = self.doc.apply("Place water", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        self.play_cache(id, file, 0.0, &[], name)?;
        Ok(format!("{} as clip {id}: {} × {} px surface, {dur:.1} s", name.to_lowercase(), surface.bounds().1[0] * 2.0, surface.bounds().1[2] * 2.0))
    }
}

fn cache_dims(c: &Cache) -> [usize; 3] {
    match &c.data {
        Frames::Volume { dims, .. } => *dims,
        _ => [0; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session() -> Session {
        Session::new(services::project::new_custom("Sim", 640, 360, 24.0))
    }

    fn last(s: &Session) -> u32 {
        s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
    }

    #[test]
    fn a_ball_drops_onto_the_floor_as_keys() {
        let mut s = session();
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 60, "detail": 1, "position": [0, -100, 0]})], "add", false).unwrap();
        let id = last(&s);
        let r = s.edit(&[json!({"op": "simulate", "kind": "rigid", "clips": [id], "duration": 2, "spin": [0, 0, 90]})], "drop", false).unwrap();
        assert!(r.to_string().contains("keys"), "{r}");
        let mc = s.model_clip(id).unwrap();
        let tr = mc.transform_at(micros(2.0));
        // Landed on the bottom of the frame (y = 180) with its centre a
        // half-size above.
        assert!((tr.position.y - 150.0).abs() < 12.0, "{}", tr.position.y);
    }

    #[test]
    fn particles_cloth_and_smoke_make_clips_that_play_caches() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "simulate", "kind": "particles", "preset": "sparks", "duration": 1})], "sparks", false).unwrap();
        assert!(r.to_string().contains("sparks particles"), "{r}");
        let mc = s.model_clip(last(&s)).unwrap();
        assert!(matches!(&mc.source, m::ClipSource::Generator(g) if g.params.contains_key("cache")));
        s.edit(&[json!({"op": "simulate", "kind": "cloth", "make": "flag", "duration": 1})], "flag", false).unwrap();
        s.edit(&[json!({"op": "simulate", "kind": "smoke", "duration": 0.5, "resolution": 16})], "smoke", false).unwrap();
        let bad = s.edit(&[json!({"op": "simulate", "kind": "lava"})], "x", false);
        assert!(bad.is_err());
    }
}
