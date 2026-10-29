// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D for agents: layers in space, cameras, camera moves, lights and
//! parallax. The same model the Inspector and the viewer's 3D tools edit
//! (`studio_model::space`).
//!
//! Units: x and y are canvas pixels from the centre (y down) as
//! everywhere; z is pixels away from the viewer (negative comes closer).
//! The default camera stands at z = −zoom (a 50 mm lens: about 1.39 ×
//! the canvas width) looking at the canvas, so a layer at z = 0 shows at
//! its own size, at z = zoom at half its size.
//!
//! * `three_d`: the 3D switch and a layer's depth, X/Y/Z rotation,
//!   orientation, depth scale and anchor, auto-orient, `look_at`, and its
//!   material (lights, shadows, extrusion).
//! * `camera`: adds or changes a camera (one-node or aimed at a point of
//!   interest), its lens (zoom, focal length or field of view), depth of
//!   field, and camera `move`s: orbit, pan, tilt, dolly / push_in /
//!   pull_out, truck, pedestal, roll, zoom, dolly_zoom, shake.
//! * `light`: adds or changes a parallel, spot, point or ambient light.
//! * `parallax`: spreads layers in depth (keeping their size on screen)
//!   and moves a camera past them.

use super::{Session, f, micros, secs};
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_model as m;
use studio_model::space as sp;

pub(super) fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub(super) fn v3(v: &Value) -> Option<[f64; 3]> {
    match v {
        Value::Array(a) if a.len() == 2 || a.len() == 3 => Some([a[0].as_f64()?, a[1].as_f64()?, a.get(2).and_then(Value::as_f64).unwrap_or(0.0)]),
        _ => None,
    }
}

pub(super) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(super) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(super) fn mul(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub(super) fn len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// A camera's (or light's) state at one moment, as keyed properties.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rig {
    pub(super) pos: [f64; 3],
    pub(super) poi: [f64; 3],
    pub(super) roll: f64,
    pub(super) rx: f64,
    pub(super) ry: f64,
    pub(super) zoom: f64,
}

/// The paths a [`Rig`] lives on.
pub(super) const RIG_PATHS: [&str; 7] = ["transform.position", "transform.position_z", "generator.poi", "generator.poi_z", "transform.rotation", "transform.rotation_x", "transform.rotation_y"];

pub(super) fn rig_at(mc: &m::Clip, rel: m::Micros, width: f64) -> Rig {
    let num = |p: &str, d: f64| mc.property_at(p, rel).and_then(|v| v.as_f64()).unwrap_or(d);
    let pt = |p: &str| match mc.property_at(p, rel) {
        Some(m::Value::Point(q)) => [q.x, q.y],
        _ => [0.0, 0.0],
    };
    let pos = pt("transform.position");
    let poi = pt("generator.poi");
    Rig {
        pos: [pos[0], pos[1], num("transform.position_z", 0.0)],
        poi: [poi[0], poi[1], num("generator.poi_z", 0.0)],
        roll: num("transform.rotation", 0.0),
        rx: num("transform.rotation_x", 0.0),
        ry: num("transform.rotation_y", 0.0),
        zoom: num("generator.zoom", sp::default_zoom(width)),
    }
}

pub(super) fn rig_value(r: &Rig, path: &str) -> m::Value {
    match path {
        "transform.position" => m::Value::Point(m::Vec2::new(r.pos[0], r.pos[1])),
        "transform.position_z" => m::Value::Scalar(r.pos[2]),
        "generator.poi" => m::Value::Point(m::Vec2::new(r.poi[0], r.poi[1])),
        "generator.poi_z" => m::Value::Scalar(r.poi[2]),
        "transform.rotation" => m::Value::Angle(r.roll),
        "transform.rotation_x" => m::Value::Angle(r.rx),
        "transform.rotation_y" => m::Value::Angle(r.ry),
        "generator.zoom" => m::Value::Scalar(r.zoom),
        _ => m::Value::Scalar(0.0),
    }
}

/// Writes one key, replacing any at that moment; the curve leaving it is
/// `ease` (a name `animate` knows).
pub(super) fn key(mc: &mut m::Clip, path: &str, t: m::Micros, v: m::Value, ease: &str) -> Result<(), String> {
    let (interp, out, inn, shape) = super::craft::curve(ease).ok_or_else(|| format!("ease \"{ease}\": {}", super::craft::CURVES))?;
    let mut k = m::Keyframe::new(t, v, interp);
    k.curve = shape;
    if let Some(o) = out {
        k.ease_out = o;
    }
    if let Some(i) = inn {
        k.ease_in = i;
    }
    let track = mc.keyframes.entry(path.to_owned()).or_insert_with(|| m::Track::new(Vec::new()));
    track.set_key(k);
    Ok(())
}

/// The direction a rig looks (unit) and its right and down axes.
pub(super) fn axes(r: &Rig, two_node: bool) -> [[f64; 3]; 3] {
    if two_node { sp::look_at(r.pos, r.poi, r.roll) } else { sp::transpose(&sp::rotation([0.0; 3], r.rx, r.ry, r.roll)) }
}

pub(super) fn turn_about(p: [f64; 3], centre: [f64; 3], axis: [f64; 3], deg: f64) -> [f64; 3] {
    // Rodrigues' rotation of p about an axis through centre.
    let k = mul(axis, 1.0 / len(axis).max(1e-12));
    let v = sub(p, centre);
    let (s, c) = deg.to_radians().sin_cos();
    let kxv = [k[1] * v[2] - k[2] * v[1], k[2] * v[0] - k[0] * v[2], k[0] * v[1] - k[1] * v[0]];
    let kv = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
    add(centre, add(add(mul(v, c), mul(kxv, s)), mul(k, kv * (1.0 - c))))
}

impl Session {
    pub(super) fn width(&self) -> f64 {
        self.doc.project.width as f64
    }

    pub(super) fn apply_clip(&mut self, what: &str, mc: m::Clip) -> Result<(), String> {
        let seq = self.doc.seq();
        let ok = self.doc.apply(what, m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "").map(|_| ())
    }

    /// `three_d`: the 3D switch and a layer's place in space.
    pub(super) fn three_d(&mut self, op: &Value) -> Result<String, String> {
        let mut ids: Vec<u32> = op.get("clip").and_then(Value::as_u64).map(|c| c as u32).into_iter().collect();
        ids.extend(super::ids(op, "clips"));
        if ids.is_empty() {
            return Err("three_d needs \"clip\" or \"clips\"".into());
        }
        let on = op.get("on").and_then(Value::as_bool).unwrap_or(true);
        let mut said = Vec::new();
        for id in ids {
            let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            if mc.is_camera_or_light() {
                return Err(format!("clip {id} is a camera or light: it is always in 3D; move it with x, y, z or the camera op"));
            }
            if matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Adjustment) {
                return Err(format!("clip {id} is an adjustment layer: it has no picture to put in space"));
            }
            mc.transform.three_d = on;
            let t = &mut mc.transform;
            for (k, slot) in [("z", &mut t.position_z), ("rotation_x", &mut t.rotation_x), ("rotation_y", &mut t.rotation_y), ("anchor_z", &mut t.anchor_z)] {
                if let Some(v) = f(op, k) {
                    *slot = v;
                }
            }
            if let Some(v) = f(op, "rotation").or_else(|| f(op, "rotation_z")) {
                t.rotation = v;
            }
            if let Some(v) = f(op, "scale_z") {
                t.scale_z = v / 100.0;
            }
            if let Some(o) = op.get("orientation") {
                t.orientation = v3(o).ok_or("orientation is [x, y, z] degrees")?;
            }
            if let Some(a) = op.get("auto_orient").and_then(Value::as_str) {
                t.auto_orient = sp::AutoOrient::parse(a).ok_or("auto_orient is off, path (turns along its motion path) or camera (always faces the camera)")?;
            }
            if let Some(target) = op.get("look_at") {
                // Turn so its face points at a point (or another clip).
                let to = self.point_of(target)?;
                let from = [t.position.x, t.position.y, t.position_z];
                let face = sp::transpose(&sp::look_at(to, from, 0.0));
                let [rx, ry, rz] = sp::euler(&sp::mul(&face, &sp::rot_z(t.rotation)));
                (t.rotation_x, t.rotation_y, t.rotation, t.orientation) = (rx, ry, rz, [0.0; 3]);
            }
            let mut mat = mc.material.clone().unwrap_or_default();
            let mut touched = false;
            if let Some(Value::Object(o)) = op.get("material") {
                for (k, v) in o {
                    let mv = match v {
                        Value::Bool(b) => m::Value::Bool(*b),
                        Value::Number(n) => m::Value::Scalar(n.as_f64().unwrap_or(0.0) / if matches!(k.as_str(), "ambient" | "diffuse" | "specular" | "side_shade") { 100.0 } else { 1.0 }),
                        _ => return Err(format!("material {k} is a number or true/false")),
                    };
                    if !mat.set(k, &mv) {
                        return Err(format!("no material setting \"{k}\"; accepts_lights, casts_shadows, accepts_shadows, ambient, diffuse, specular, shininess (percent for the first three), extrude (pixels), side_shade (percent)"));
                    }
                    touched = true;
                }
            }
            for k in ["casts_shadows", "accepts_lights", "accepts_shadows"] {
                if let Some(b) = op.get(k).and_then(Value::as_bool) {
                    mat.set(k, &m::Value::Bool(b));
                    touched = true;
                }
            }
            if let Some(e) = f(op, "extrude") {
                mat.extrude = e.max(0.0);
                touched = true;
            }
            if touched {
                mc.material = Some(mat);
            }
            let t = mc.transform.clone();
            self.apply_clip("3D layer", mc)?;
            // Within 15° of side-on, text is a sliver (a floor or wall is
            // meant to be: the camera sees it from above or the side).
            let edge_on = |a: f64| (a.rem_euclid(180.0) - 90.0).abs() < 15.0;
            let text = self.clip(id).is_ok_and(|c| !c.text.is_empty());
            let sliver = if on && text && (edge_on(t.rotation_x) || edge_on(t.rotation_y)) { " (seen almost edge-on: it all but disappears; keep text within ±60° while it must be read)" } else { "" };
            said.push(if on {
                format!("clip {id} in 3D: z {}, rotation x {} y {} z {}{}{sliver}", r1(t.position_z), r1(t.rotation_x), r1(t.rotation_y), r1(t.rotation), if t.auto_orient.is_off() { String::new() } else { format!(", auto-orient {:?}", t.auto_orient).to_lowercase() })
            } else {
                format!("clip {id} back in 2D (its 3D settings are kept for when it is switched on again)")
            });
        }
        Ok(said.join("; "))
    }

    /// A point in space: `[x, y]`, `[x, y, z]`, or a clip (where it is).
    pub(super) fn point_of(&self, v: &Value) -> Result<[f64; 3], String> {
        if let Some(p) = v3(v) {
            return Ok(p);
        }
        let id = v.get("clip").and_then(Value::as_u64).or_else(|| v.as_u64()).ok_or("a point is [x, y, z] canvas pixels, or a clip")? as u32;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let vc = self.clip(id)?;
        let seq = self.doc.model.sequence(self.doc.seq()).ok_or("no sequence")?;
        let tr = seq.placed_transform(&mc, vc.start);
        Ok([tr.position.x, tr.position.y, tr.position_z])
    }

    /// The camera that looks at `t`, if any (topmost camera clip).
    pub(super) fn camera_at(&self, t: view::Time) -> Option<u32> {
        let seq = self.doc.model.sequence(self.doc.seq())?;
        seq.clips
            .iter()
            .filter(|c| c.enabled && t >= c.start && t < c.end() && matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera))
            .max_by_key(|c| (seq.lane_position(c.lane).unwrap_or(0), c.start))
            .map(|c| c.id.0 as u32)
    }

    /// A new camera or light clip on the top lane.
    pub(super) fn add_rig(&mut self, kind: m::GeneratorKind, op: &Value, name: &str, params: m::ParamMap, pos: [f64; 3]) -> Result<u32, String> {
        let at = micros(f(op, "at").unwrap_or(0.0));
        let end = super::new_clip_end(self.doc.project.duration(), at);
        let dur = f(op, "duration").map(micros).unwrap_or(end - at).max(SEC / 10);
        let lane = match op.get("lane") {
            Some(v) => self.room(Some(super::craft::lane_of(&self.doc.project, v, None)?), at, dur, None)?,
            None => self.room(None, at, dur, None)?,
        };
        let id = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(kind);
        g.params = params;
        let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), at, dur, m::ClipSource::Generator(g));
        c.name = name.to_owned();
        c.transform.position = m::Vec2::new(pos[0], pos[1]);
        c.transform.position_z = pos[2];
        let seq = self.doc.seq();
        let ok = self.doc.apply(name, m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(id)
    }

    /// `camera`: add or change a camera, its lens and focus, and move it.
    pub(super) fn camera(&mut self, op: &Value) -> Result<String, String> {
        let w = self.width();
        let mut said = Vec::new();
        let id = match op.get("clip").and_then(Value::as_u64) {
            Some(id) => id as u32,
            None => match self.camera_at(micros(f(op, "at").unwrap_or(0.0))).filter(|_| op.get("new").and_then(Value::as_bool) != Some(true)) {
                Some(id) => id,
                None => {
                    let zoom = sp::default_zoom(w);
                    let mut params = sp::CameraSettings::defaults(w);
                    params.insert("two_node".into(), m::Value::Bool(true));
                    let id = self.add_rig(m::GeneratorKind::Camera, op, "Camera", params, [0.0, 0.0, -zoom])?;
                    said.push(format!("camera added as clip {id} (aimed at the canvas centre from {} px in front)", r1(zoom)));
                    id
                }
            },
        };
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if !matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera) {
            return Err(format!("clip {id} is not a camera"));
        }
        let vc = self.clip(id)?;
        let set_param = |mc: &mut m::Clip, k: &str, v: m::Value| {
            if let m::ClipSource::Generator(g) = &mut mc.source {
                g.params.insert(k.into(), v);
            }
        };
        if let Some(t) = op.get("type").and_then(Value::as_str) {
            let two = match t {
                "two_node" | "two-node" | "aimed" | "poi" => true,
                "one_node" | "one-node" | "free" => false,
                _ => return Err("type is one_node (turns by its rotation) or two_node (aims at its point of interest)".into()),
            };
            if !two {
                // Keep looking the same way: its aim becomes its rotation.
                let r = rig_at(&mc, 0, w);
                let [rx, ry, _] = sp::euler(&sp::transpose(&sp::look_at(r.pos, r.poi, 0.0)));
                (mc.transform.rotation_x, mc.transform.rotation_y) = (rx, ry);
            }
            set_param(&mut mc, "two_node", m::Value::Bool(two));
        }
        if let Some(p) = op.get("position") {
            let p = v3(p).ok_or("position is [x, y, z]")?;
            mc.transform.position = m::Vec2::new(p[0], p[1]);
            mc.transform.position_z = p[2];
        }
        if let Some(target) = op.get("look_at") {
            let p = self.point_of(target)?;
            set_param(&mut mc, "poi", m::Value::Point(m::Vec2::new(p[0], p[1])));
            set_param(&mut mc, "poi_z", m::Value::Scalar(p[2]));
            set_param(&mut mc, "two_node", m::Value::Bool(true));
        }
        // The lens: focal length on the sensor, kit, squeeze, character.
        let (lens_said, lens_character) = self.lens_setup(&mut mc, op)?;
        said.extend(lens_said);
        if let Some(b) = op.get("dof").and_then(Value::as_bool) {
            set_param(&mut mc, "dof", m::Value::Bool(b));
        }
        if let Some(fv) = op.get("focus") {
            // A distance, or a point / clip to focus on.
            let d = match fv.as_f64() {
                Some(d) => d,
                None => {
                    let p = self.point_of(fv)?;
                    let r = rig_at(&mc, 0, w);
                    let fwd = axes(&r, sp::CameraSettings::read(&gen_params(&mc), w).two_node)[2];
                    let v = sub(p, r.pos);
                    (v[0] * fwd[0] + v[1] * fwd[1] + v[2] * fwd[2]).max(1.0)
                }
            };
            set_param(&mut mc, "focus_distance", m::Value::Scalar(d));
            set_param(&mut mc, "dof", m::Value::Bool(true));
        }
        if let Some(a) = f(op, "aperture") {
            set_param(&mut mc, "aperture", m::Value::Scalar(a.max(0.0)));
        }
        if let Some(b) = f(op, "blur_level") {
            set_param(&mut mc, "blur_level", m::Value::Scalar(b / 100.0));
        }
        if let Some(a) = op.get("auto_orient").and_then(Value::as_str) {
            mc.transform.auto_orient = sp::AutoOrient::parse(a).ok_or("auto_orient is off or path (the camera looks where it travels)")?;
        }
        if let Some(r) = f(op, "roll") {
            mc.transform.rotation = r;
        }
        let moves: Vec<Value> = match (op.get("move"), op.get("moves")) {
            (Some(Value::String(s)), _) => vec![json!({"type": s})],
            (Some(o @ Value::Object(_)), _) => vec![o.clone()],
            (_, Some(Value::Array(a))) => a.clone(),
            (None, None) => Vec::new(),
            _ => return Err("move is a name (orbit, pan, tilt, dolly, push_in, pull_out, truck, pedestal, roll, zoom, dolly_zoom, shake) or {\"type\", \"by\", \"from\", \"duration\", \"ease\"}; moves is a list of those".into()),
        };
        for mv in &moves {
            // Rigs and focus pulls (cine), else the moves here.
            let s = match self.rig_move(&mut mc, &vc, mv)? {
                Some(s) => s,
                None => self.camera_move(&mut mc, &vc, mv)?,
            };
            said.push(s);
        }
        self.apply_clip("Camera", mc)?;
        if lens_character {
            if let Some(s) = self.lens_layer(id)? {
                said.push(s);
            }
        }
        if let Some(s) = self.bake_bokeh(id)? {
            said.push(s);
        }
        // Backdrops (3D layers filling the frame when the shot starts) grow
        // just enough that the move never shows their edges: only parallax
        // did this, so a plain truck or orbit could show the edge of the
        // picture behind.
        if !moves.is_empty() {
            let (t0, t1) = (vc.start, vc.start + vc.dur);
            let backdrops: Vec<u32> = self
                .doc
                .model
                .sequence(self.doc.seq())
                .map(|q| {
                    q.clips
                        .iter()
                        // Pictures and full-canvas colour can be backdrops;
                        // shapes, titles, cameras and lights cannot.
                        .filter(|c| c.transform.three_d && c.start < t1 && c.end() > t0)
                        .filter(|c| match &c.source {
                            m::ClipSource::Media(_) => true,
                            m::ClipSource::Generator(g) => matches!(g.kind, m::GeneratorKind::Solid | m::GeneratorKind::Gradient),
                            _ => false,
                        })
                        .map(|c| c.id.0 as u32)
                        .collect()
                })
                .unwrap_or_default();
            for b in backdrops {
                let Ok(bc) = self.clip(b) else { continue };
                let (a, z) = (t0.max(bc.start), t1.min(bc.start + bc.dur));
                if self.need(b, a, a).is_some_and(|k| k <= 1.0) && self.need(b, a, z).is_some_and(|k| k > 1.0) {
                    if let Some(k) = self.cover(b, a, z) {
                        said.push(format!("backdrop {b} scaled up {k:.2}× so the move never shows its edges"));
                    }
                }
            }
        }
        // A 2.5D move must not run past the pictures.
        if !moves.is_empty() {
            if let Some((t, place)) = self.gaps(vc.start, vc.start + vc.dur) {
                said.push(format!("warning: the empty canvas shows at {place} at {:.2}s (nothing in space covers it there): move less, start the backdrop full-frame, or use a wider backdrop", secs(t)));
            }
        }
        let mc = self.model_clip(id).ok_or("camera gone")?;
        let r = rig_at(&mc, 0, w);
        let s = sp::CameraSettings::read(&gen_params(&mc), w);
        let lens = super::cine::lens_of(&gen_params(&mc));
        said.push(format!(
            "camera clip {id} ({:.2}–{:.2}s): at [{}, {}, {}], {}, {} lens ({:.0}° across){}",
            secs(vc.start),
            secs(vc.start + vc.dur),
            r1(r.pos[0]),
            r1(r.pos[1]),
            r1(r.pos[2]),
            if s.two_node { format!("looking at [{}, {}, {}]", r1(r.poi[0]), r1(r.poi[1]), r1(r.poi[2])) } else { format!("rotation x {} y {}", r1(r.rx), r1(r.ry)) },
            lens.words(s.zoom, w),
            sp::fov_from_zoom(s.zoom, w),
            if s.dof || gen_params(&mc).get("bokeh_dof").and_then(m::Value::as_bool) == Some(true) { format!(", depth of field focused at {} px", r1(s.focus_distance)) } else { String::new() }
        ));
        if !self.doc.model.sequence(self.doc.seq()).is_some_and(|q| q.clips.iter().any(|c| c.transform.three_d)) {
            said.push("note: only 3D layers are seen through the camera; switch layers into 3D with three_d".into());
        }
        Ok(said.join("; "))
    }

    /// One camera move: keys on the rig from `from` for `duration`.
    fn camera_move(&mut self, mc: &mut m::Clip, vc: &view::Clip, mv: &Value) -> Result<String, String> {
        let w = self.width();
        let kind = mv.get("type").or(mv.get("kind")).and_then(Value::as_str).ok_or("a move needs \"type\"")?.to_lowercase();
        // An unknown field ("degrees", "seconds") would silently give the
        // default amount or the whole clip.
        if let Some(k) = mv.as_object().and_then(|o| o.keys().find(|k| !matches!(k.as_str(), "type" | "kind" | "by" | "amount" | "to" | "from" | "duration" | "ease" | "vertical" | "around"))) {
            return Err(format!("a camera move has no field \"{k}\"; it takes type, by (degrees for orbit, pan, tilt, roll; px for dolly, push_in, truck, pedestal; a factor for zoom), from, duration (s), ease, vertical and around (orbit), to (zoom, mm)"));
        }
        let from = micros(f(mv, "from").unwrap_or(0.0)).clamp(0, vc.dur);
        let dur = f(mv, "duration").map(micros).unwrap_or(vc.dur - from).clamp(SEC / 30, (vc.dur - from).max(SEC / 30));
        let ease = mv.get("ease").and_then(Value::as_str).unwrap_or("ease_in_out").to_owned();
        super::craft::curve(&ease).ok_or_else(|| format!("ease \"{ease}\": {}", super::craft::CURVES))?;
        let two_node = sp::CameraSettings::read(&gen_params(mc), w).two_node;
        let r0 = rig_at(mc, from, w);
        let ax = axes(&r0, two_node);
        let by = f(mv, "by").or_else(|| f(mv, "amount"));
        let dist = len(sub(r0.poi, r0.pos)).max(1.0);
        // The state at progress u (0..1) of the move.
        let state: Box<dyn Fn(f64) -> Rig> = match kind.as_str() {
            "dolly" | "push_in" | "push" | "pull_out" | "pull" => {
                // A quarter of the distance, as the craft book says (it was
                // 35%, and the book and the default disagreed).
                let d = by.unwrap_or(0.25 * dist) * if kind.starts_with("pull") { -1.0 } else { 1.0 };
                let d = if two_node { d.min(dist - 10.0) } else { d };
                Box::new(move |u| Rig { pos: add(r0.pos, mul(ax[2], d * u)), ..r0 })
            }
            "truck" | "track" => {
                let d = by.unwrap_or(0.25 * w);
                Box::new(move |u| Rig { pos: add(r0.pos, mul(ax[0], d * u)), poi: add(r0.poi, mul(ax[0], d * u)), ..r0 })
            }
            "pedestal" | "boom" | "crane" => {
                // Positive goes up.
                let d = by.unwrap_or(0.15 * w);
                Box::new(move |u| Rig { pos: add(r0.pos, mul(ax[1], -d * u)), poi: add(r0.poi, mul(ax[1], -d * u)), ..r0 })
            }
            "pan" | "tilt" => {
                let deg = by.unwrap_or(if kind == "pan" { 20.0 } else { 10.0 });
                let horizontal = kind == "pan";
                Box::new(move |u| {
                    if two_node {
                        let axis = if horizontal { [0.0, 1.0, 0.0] } else { ax[0] };
                        Rig { poi: turn_about(r0.poi, r0.pos, axis, if horizontal { deg } else { -deg } * u), ..r0 }
                    } else if horizontal {
                        Rig { ry: r0.ry + deg * u, ..r0 }
                    } else {
                        Rig { rx: r0.rx - deg * u, ..r0 }
                    }
                })
            }
            "orbit" | "arc" => {
                let deg = by.unwrap_or(45.0);
                let up = f(mv, "vertical").unwrap_or(0.0);
                let around = match mv.get("around") {
                    Some(v) => self.point_of(v)?,
                    None if two_node => r0.poi,
                    None => [0.0, 0.0, 0.0],
                };
                if !two_node {
                    // Orbiting keeps its eye on the centre: aim it there.
                    if let m::ClipSource::Generator(g) = &mut mc.source {
                        g.params.insert("two_node".into(), m::Value::Bool(true));
                    }
                }
                let side = ax[0];
                Box::new(move |u| {
                    let p = turn_about(r0.pos, around, [0.0, 1.0, 0.0], -deg * u);
                    let p = turn_about(p, around, turn_about(side, [0.0; 3], [0.0, 1.0, 0.0], -deg * u), -up * u);
                    Rig { pos: p, poi: around, ..r0 }
                })
            }
            "roll" => {
                let deg = by.unwrap_or(10.0);
                Box::new(move |u| Rig { roll: r0.roll + deg * u, ..r0 })
            }
            "zoom" => {
                // `to` focal length (mm), or `by` a factor.
                let end = match (f(mv, "to"), by) {
                    (Some(mm), _) => sp::zoom_from_focal_length(mm, w),
                    (None, Some(k)) => r0.zoom * k,
                    _ => r0.zoom * 1.5,
                };
                Box::new(move |u| Rig { zoom: r0.zoom + (end - r0.zoom) * u, ..r0 })
            }
            "dolly_zoom" | "vertigo" => {
                // Moves in while widening the lens: what it looks at keeps
                // its size, the background stretches away.
                let d = by.unwrap_or(0.4 * dist).min(dist - 10.0);
                Box::new(move |u| {
                    let now = dist - d * u;
                    Rig { pos: add(r0.pos, mul(ax[2], d * u)), zoom: r0.zoom * now / dist, ..r0 }
                })
            }
            "shake" | "handheld" => {
                let amount = by.unwrap_or(6.0);
                let seed = mc.id.0;
                Box::new(move |u| {
                    let n = |k: f64| ((u * 37.0 + k + seed as f64 * 0.37).sin() * 0.6 + (u * 91.0 + k * 2.1).sin() * 0.4) * amount;
                    let off = add(mul(ax[0], n(0.0)), mul(ax[1], n(5.0)));
                    Rig { pos: add(r0.pos, off), poi: add(r0.poi, off), roll: r0.roll + n(9.0) * 0.05, ..r0 }
                })
            }
            other => return Err(format!("no camera move \"{other}\": orbit, pan, tilt, dolly, push_in, pull_out, truck, pedestal, roll, zoom, dolly_zoom, shake")),
        };
        let r1_ = state(1.0);
        // Paths that change.
        let mut paths: Vec<&str> = RIG_PATHS.iter().copied().filter(|p| rig_value(&r0, p) != rig_value(&r1_, p)).collect();
        if r1_.zoom != r0.zoom {
            paths.push("generator.zoom");
        }
        // Curved moves get a key every tenth, eased along; straight ones
        // one key at each end with the curve between.
        let (steps, inner) = match kind.as_str() {
            "orbit" | "arc" | "dolly_zoom" | "vertigo" => (10, true),
            "pan" | "tilt" if two_node => (10, true),
            "shake" | "handheld" => (((secs(dur) * 8.0).round() as usize).clamp(4, 400), false),
            _ => (1, false),
        };
        let curve = m::Track::new(vec![
            {
                let (interp, out, _, shape) = super::craft::curve(&ease).expect("checked");
                let mut k = m::Keyframe::new(0, 0.0, interp);
                k.curve = shape;
                if let Some(o) = out {
                    k.ease_out = o;
                }
                k
            },
            {
                let mut k = m::Keyframe::new(1_000_000, 1.0, m::Interp::Linear);
                if let Some((_, _, Some(i), _)) = super::craft::curve(&ease) {
                    k.ease_in = i;
                }
                k
            },
        ]);
        let eased = |u: f64| curve.eval((u * 1e6).round() as m::Micros).and_then(|v| v.as_f64()).unwrap_or(u);
        for i in 0..=steps {
            let u = i as f64 / steps as f64;
            let t = from + (dur as f64 * u).round() as m::Micros;
            let (rig, e) = if inner || steps > 1 { (state(eased(u)), "spline") } else { (state(u), ease.as_str()) };
            for p in &paths {
                key(mc, p, t, rig_value(&rig, p), e)?;
            }
        }
        // Say how far it went: a default amount should not be a surprise.
        let travel = len(sub(r1_.pos, r0.pos));
        let far = match kind.as_str() {
            "orbit" | "arc" => format!(" {}°", r1(by.unwrap_or(45.0))),
            "pan" | "tilt" | "roll" => format!(" {}°", r1(by.unwrap_or(if kind == "pan" { 20.0 } else { 10.0 }))),
            "zoom" => format!(" to {} mm", r1(sp::focal_length_from_zoom(r1_.zoom, w))),
            "shake" | "handheld" => String::new(),
            _ => format!(" {} px", r1(travel)),
        };
        Ok(format!("{kind}{far} over {:.2}s from {:.2}s into the camera clip", secs(dur), secs(from)))
    }

    /// `light`: add or change a light.
    pub(super) fn light(&mut self, op: &Value) -> Result<String, String> {
        let w = self.width();
        let id = match op.get("clip").and_then(Value::as_u64) {
            Some(id) => id as u32,
            None => {
                let t = op.get("type").and_then(Value::as_str).unwrap_or("point");
                let kind = sp::LightKind::parse(t).ok_or("type is parallel (sun), spot, point (bulb) or ambient")?;
                let pos = match kind {
                    sp::LightKind::Parallel => [-0.5 * w, -0.6 * w, -0.8 * w],
                    sp::LightKind::Ambient => [0.0; 3],
                    _ => [-0.3 * w, -0.3 * w, -0.5 * w],
                };
                let name = match kind {
                    sp::LightKind::Parallel => "Sun light",
                    sp::LightKind::Spot => "Spot light",
                    sp::LightKind::Point => "Point light",
                    sp::LightKind::Ambient => "Ambient light",
                };
                self.add_rig(m::GeneratorKind::Light, op, name, sp::LightSettings::defaults(kind), pos)?
            }
        };
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let m::ClipSource::Generator(g) = &mut mc.source else { return Err(format!("clip {id} is not a light")) };
        if g.kind != m::GeneratorKind::Light {
            return Err(format!("clip {id} is not a light"));
        }
        if let Some(t) = op.get("type").and_then(Value::as_str).filter(|_| op.get("clip").is_some()) {
            let k = sp::LightKind::parse(t).ok_or("type is parallel, spot, point or ambient")?;
            g.params.insert("light_type".into(), m::Value::Choice(k.name().into()));
        }
        if let Some(c) = op.get("colour").and_then(Value::as_str) {
            let k = super::craft::colour(c).ok_or_else(|| format!("colour \"{c}\" is not #rrggbb or a basic name"))?;
            g.params.insert("colour".into(), m::Value::Colour(m::Rgba::new(k.r() as f64 / 255.0, k.g() as f64 / 255.0, k.b() as f64 / 255.0, 1.0)));
        }
        for (k, path, scale) in [("intensity", "intensity", 100.0), ("cone_angle", "cone_angle", 1.0), ("cone_feather", "cone_feather", 100.0), ("radius", "radius", 1.0), ("falloff", "falloff", 1.0), ("shadow_darkness", "shadow_darkness", 100.0)] {
            if let Some(v) = f(op, k) {
                g.params.insert(path.into(), if k == "cone_angle" { m::Value::Angle(v) } else { m::Value::Scalar(v / scale) });
            }
        }
        if let Some(b) = op.get("shadows").or(op.get("casts_shadows")).and_then(Value::as_bool) {
            g.params.insert("casts_shadows".into(), m::Value::Bool(b));
        }
        let kind_now = sp::LightSettings::read(&g.params).kind;
        let look_notes = super::air::set_light_look(op, &mut g.params, kind_now)?;
        let aim = match op.get("look_at") {
            Some(v) => Some(self.point_of(v)?),
            None => None,
        };
        let m::ClipSource::Generator(g) = &mut mc.source else { unreachable!() };
        if let Some(p) = aim {
            g.params.insert("poi".into(), m::Value::Point(m::Vec2::new(p[0], p[1])));
            g.params.insert("poi_z".into(), m::Value::Scalar(p[2]));
        }
        if let Some(p) = op.get("position") {
            let p = v3(p).ok_or("position is [x, y, z]")?;
            mc.transform.position = m::Vec2::new(p[0], p[1]);
            mc.transform.position_z = p[2];
        }
        self.apply_clip("Light", mc)?;
        let mc = self.model_clip(id).ok_or("light gone")?;
        let s = sp::LightSettings::read(&gen_params(&mc));
        let t = &mc.transform;
        let mut note = String::new();
        let any_3d = self.doc.model.sequence(self.doc.seq()).is_some_and(|q| q.clips.iter().any(|c| c.transform.three_d));
        if !any_3d {
            note = "; note: lights fall only on 3D layers (three_d)".into();
        }
        if f(op, "intensity").is_some_and(|v| v > 0.0 && v <= 2.0) {
            note.push_str("; note: intensity is percent (100 is full): this light is all but off");
        }
        for n in &look_notes {
            note.push_str("; note: ");
            note.push_str(n);
        }
        let looks: Vec<String> = ["glow", "flare", "beam", "flicker"].iter().filter_map(|k| f(op, k).filter(|v| *v > 0.0).map(|v| format!("{k} {v}"))).chain(op.get("flame").and_then(|v| v.as_bool().or_else(|| v.as_f64().map(|x| x > 0.0))).filter(|b| *b).map(|_| "flame".to_owned())).collect();
        if !looks.is_empty() {
            note.push_str(&format!("; shows itself: {}", looks.join(", ")));
        }
        let shadows_note = if s.casts_shadows && !self.doc.model.sequence(self.doc.seq()).is_some_and(|q| q.clips.iter().any(|c| c.material.as_ref().is_some_and(|m| m.casts_shadows))) {
            "; shadows need a layer that casts them (three_d casts_shadows: true)"
        } else {
            ""
        };
        Ok(format!(
            "{} light clip {id} at [{}, {}, {}]{} intensity {}%{}{}{}",
            s.kind.name(),
            r1(t.position.x),
            r1(t.position.y),
            r1(t.position_z),
            if matches!(s.kind, sp::LightKind::Spot | sp::LightKind::Parallel) { format!(" aimed at [{}, {}, {}],", r1(s.poi[0]), r1(s.poi[1]), r1(s.poi[2])) } else { ",".into() },
            r1(s.intensity * 100.0),
            if s.casts_shadows { ", casting shadows" } else { "" },
            shadows_note,
            note
        ))
    }

    /// `parallax`: layers spread in depth (the first furthest back), each
    /// scaled so it still fills the frame as before, and a camera moving
    /// past them.
    pub(super) fn parallax(&mut self, op: &Value) -> Result<String, String> {
        let ids = super::ids(op, "clips");
        if ids.len() < 2 {
            return Err("parallax needs \"clips\": two or more layers, furthest back first".into());
        }
        // Listed front to back by mistake, the backdrop ends up in front
        // and covers the rest.
        let order = {
            let p = &self.doc.project;
            let idx = |l: u32| p.lanes.iter().position(|x| x.id == l).unwrap_or(0);
            let lanes: Vec<(u32, usize)> = ids.iter().filter_map(|i| self.clip(*i).ok()).map(|c| (c.id, idx(c.lane))).collect();
            match lanes.windows(2).find(|w| w[1].1 > w[0].1) {
                Some(w) => format!("; note: clips go furthest back first, but {} is listed before {}, which is on a lane under it: {} is now behind it in space; list the backdrop first", w[0].0, w[1].0, w[0].0),
                None => String::new(),
            }
        };
        let w = self.width();
        let zoom = self.camera_at(self.clip(ids[0])?.start).and_then(|c| self.model_clip(c)).map_or(sp::default_zoom(w), |c| sp::CameraSettings::read(&gen_params(&c), w).zoom);
        let depth = f(op, "depth").unwrap_or(0.8 * zoom).max(1.0);
        let n = ids.len();
        let mut said = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
            // The front layer a little in front of the canvas plane, the
            // back one far behind it.
            let z = depth * (0.8 - i as f64 / (n - 1) as f64);
            // The backmost layer is the backdrop: sized after the move so
            // the frame never runs off it (`overscan` adds a margin).
            let overscan = if i == 0 { f(op, "overscan").map_or(1.0, |o| o / 100.0).max(1.0) } else { 1.0 };
            let grow = (zoom + z) / (zoom + mc.transform.position_z) * overscan;
            mc.transform.three_d = true;
            mc.transform.position_z = z;
            mc.transform.scale_x *= grow;
            mc.transform.scale_y *= grow;
            mc.transform.position = m::Vec2::new(mc.transform.position.x * grow, mc.transform.position.y * grow);
            mc.keyframes.retain(|k, _| !matches!(k.as_str(), "transform.position_z"));
            self.apply_clip("Parallax", mc)?;
            said.push(format!("{id} at z {}", r1(z)));
        }
        let start = ids.iter().filter_map(|i| self.clip(*i).ok()).map(|c| c.start).min().unwrap_or(0);
        let end = ids.iter().filter_map(|i| self.clip(*i).ok()).map(|c| c.end()).max().unwrap_or(start + SEC);
        let mv = op.get("move").cloned().unwrap_or(json!("truck"));
        let mut mv = match mv {
            Value::String(s) => json!({"type": s}),
            v => v,
        };
        if mv.get("by").is_none() && mv.get("type").and_then(Value::as_str) == Some("truck") {
            mv["by"] = json!(f(op, "amount").unwrap_or(0.12 * w));
        } else if let Some(a) = f(op, "amount") {
            mv["by"] = json!(a);
        }
        if let Some(e) = op.get("ease") {
            mv["ease"] = e.clone();
        }
        let cam = match self.camera_at(start) {
            Some(c) => json!({"op": "camera", "clip": c, "move": mv}),
            None => json!({"op": "camera", "at": secs(start), "duration": secs(end - start), "move": mv}),
        };
        let moved = self.camera(&cam)?;
        let cover = match self.cover(ids[0], start, end) {
            Some(k) if k > 1.0 => format!("; the backdrop {} scaled up {:.2}× so the move never shows its edges", ids[0], k),
            _ => String::new(),
        };
        Ok(format!("parallax: {} (each scaled to keep its size on screen); {moved}{cover}{order}", said.join(", ")))
    }

    /// Grows a 3D layer about its anchor until it covers the whole frame
    /// at every moment from `t0` to `t1` (a backdrop behind a camera
    /// move). Returns the factor.
    pub(super) fn cover(&mut self, id: u32, t0: view::Time, t1: view::Time) -> Option<f64> {
        let need = self.need(id, t0, t1)?;
        if need <= 1.0 {
            return Some(1.0);
        }
        let k = (need * 1.02).min(4.0);
        let mut mc = self.doc.model.sequence(self.doc.seq())?.clip(m::ClipId(id as u64))?.clone();
        mc.transform.scale_x *= k;
        mc.transform.scale_y *= k;
        self.apply_clip("Backdrop overscan", mc).ok()?;
        Some(k)
    }

    /// How much a 3D layer must grow to cover the whole frame at every
    /// moment from `t0` to `t1` (1 or less: it already does). `None` when
    /// it cannot (keyed scale, or a frame corner misses it entirely).
    pub(super) fn need(&self, id: u32, t0: view::Time, t1: view::Time) -> Option<f64> {
        let (cw, ch) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let vc = self.clip(id).ok()?;
        let source = match vc.media.and_then(|m| self.doc.project.media(m)) {
            Some(mi) => m::Vec2::new(mi.width.max(1) as f64, mi.height.max(1) as f64),
            None => m::Vec2::new(cw, ch),
        };
        let fit = (cw / source.x).min(ch / source.y);
        let seq = self.doc.model.sequence(self.doc.seq())?;
        let mc = seq.clip(m::ClipId(id as u64))?.clone();
        if mc.keyframes.contains_key("transform.scale_x") {
            return None;
        }
        let (hx, hy) = (source.x / 2.0, source.y / 2.0);
        let mut need: f64 = 1.0;
        for k in 0..=12 {
            let t = (t0 + (t1 - t0) * k / 12).clamp(vc.start, vc.start + vc.dur - 1);
            let tr = sp::oriented_transform(seq, &mc, t, cw);
            let plane = sp::Plane::of(&tr, source, fit);
            let view = sp::view_at(seq, t, cw);
            for (x, y) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                match plane.hit(view.position, view.ray(m::Vec2::new(x * cw, y * ch))) {
                    Some((u, v, d)) if d > 0.0 => need = need.max(((u - hx) / hx).abs()).max(((v - hy) / hy).abs()),
                    _ => return None,
                }
            }
        }
        Some(need)
    }

    /// A clip's 3D state for `inspect`.
    pub(super) fn space_of(&self, id: u32) -> Option<Value> {
        let mc = self.model_clip(id)?;
        let w = self.width();
        if let m::ClipSource::Generator(g) = &mc.source {
            match g.kind {
                m::GeneratorKind::Camera => {
                    let s = sp::CameraSettings::read(&gen_params(&mc), w);
                    let r = rig_at(&mc, 0, w);
                    return Some(json!({
                        "camera": if s.two_node { "two_node" } else { "one_node" },
                        "position": [r1(r.pos[0]), r1(r.pos[1]), r1(r.pos[2])],
                        "look_at": s.two_node.then(|| json!([r1(r.poi[0]), r1(r.poi[1]), r1(r.poi[2])])),
                        "zoom": r1(s.zoom), "focal_length_mm": r1(super::cine::lens_of(&gen_params(&mc)).focal(s.zoom, w)), "fov_degrees": r1(sp::fov_from_zoom(s.zoom, w)), "lens": super::cine::lens_of(&gen_params(&mc)).words(s.zoom, w),
                        "dof": s.dof, "focus_distance": r1(s.focus_distance), "aperture": r1(s.aperture),
                        "auto_orient": format!("{:?}", mc.transform.auto_orient).to_lowercase(),
                    }));
                }
                m::GeneratorKind::Light => {
                    let s = sp::LightSettings::read(&gen_params(&mc));
                    let t = &mc.transform;
                    return Some(json!({
                        "light": s.kind.name(), "position": [r1(t.position.x), r1(t.position.y), r1(t.position_z)],
                        "look_at": [r1(s.poi[0]), r1(s.poi[1]), r1(s.poi[2])], "intensity": r1(s.intensity * 100.0),
                        "casts_shadows": s.casts_shadows,
                    }));
                }
                _ => {}
            }
        }
        let t = &mc.transform;
        if !t.three_d {
            return None;
        }
        let mat = mc.material.clone().unwrap_or_default();
        Some(json!({
            "three_d": true, "z": r1(t.position_z), "rotation": [r1(t.rotation_x), r1(t.rotation_y), r1(t.rotation)],
            "orientation": t.orientation.map(r1), "scale_z": r1(t.scale_z * 100.0), "anchor_z": r1(t.anchor_z),
            "auto_orient": format!("{:?}", t.auto_orient).to_lowercase(),
            "material": {"accepts_lights": mat.accepts_lights, "casts_shadows": mat.casts_shadows, "accepts_shadows": mat.accepts_shadows, "extrude": r1(mat.extrude), "specular": r1(mat.specular * 100.0), "shininess": r1(mat.shininess)},
        }))
    }
}

/// "camera", "light" or "model" for those clips.
pub(super) fn rig_kind(mc: &m::Clip) -> Option<&'static str> {
    match &mc.source {
        m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera => Some("camera"),
        m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Light => Some("light"),
        m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model => Some("model"),
        _ => None,
    }
}

/// A generator clip's parameters (none for other clips).
pub(super) fn gen_params(mc: &m::Clip) -> m::ParamMap {
    match &mc.source {
        m::ClipSource::Generator(g) => g.params.clone(),
        _ => m::ParamMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session() -> (Session, u32, u32) {
        let mut s = Session::new(services::project::new_custom("Space", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#204060", "at": 0, "duration": 4})], "bg", false).unwrap();
        let bg = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 200, "height": 120})], "card", false).unwrap();
        let card = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s, bg, card)
    }

    /// A plain camera move (not only parallax) grows a full-frame backdrop
    /// so its edges never show, and leaves a small card alone; a push in
    /// goes a quarter of the distance by default.
    #[test]
    fn camera_moves_keep_backdrops_covering_the_frame() {
        let (mut s, bg, card) = session();
        s.edit(&[json!({"op": "three_d", "clips": [bg, card]})], "3d", false).unwrap();
        let before = s.model_clip(bg).unwrap().transform.scale_x;
        let card_before = s.model_clip(card).unwrap().transform.scale_x;
        let r = s.edit(&[json!({"op": "camera", "move": {"type": "truck", "by": 300, "duration": 3}})], "cam", false).unwrap();
        let after = s.model_clip(bg).unwrap().transform.scale_x;
        assert!(after > before * 1.05, "backdrop grew: {before} -> {after}; {r:?}");
        assert!(format!("{r:?}").contains("never shows its edges"), "{r:?}");
        assert_eq!(s.model_clip(card).unwrap().transform.scale_x, card_before, "a small card is not a backdrop");
        // The edges stay out of frame through the whole move.
        let cam = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (t0, t1) = { let c = s.clip(cam).unwrap(); (c.start, c.start + c.dur) };
        assert!(s.need(bg, t0, t1).is_some_and(|k| k <= 1.0));
    }

    #[test]
    fn push_in_defaults_to_a_quarter_of_the_distance() {
        let (mut s, bg, _) = session();
        s.edit(&[json!({"op": "three_d", "clip": bg})], "3d", false).unwrap();
        let r = s.edit(&[json!({"op": "camera", "move": "push_in"})], "cam", false).unwrap();
        let cam = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let dist = sp::default_zoom(s.width());
        let z = s.model_clip(cam).unwrap().keyframes.get("transform.position_z").map(|t| t.keys.iter().filter_map(|k| k.value.as_f64()).collect::<Vec<_>>());
        let z = z.unwrap_or_default();
        let moved = z.last().copied().unwrap_or(0.0) - z.first().copied().unwrap_or(0.0);
        assert!((moved - 0.25 * dist).abs() < 1.0, "pushed {moved} of {dist}; {r:?}");
    }

    #[test]
    fn three_d_switch_and_material() {
        let (mut s, _, id) = session();
        s.edit(&[json!({"op": "three_d", "clip": id, "rotation_y": 35, "z": 200, "extrude": 30, "casts_shadows": true})], "3d", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.transform.three_d);
        assert_eq!((mc.transform.rotation_y, mc.transform.position_z), (35.0, 200.0));
        let mat = mc.material.unwrap();
        assert!(mat.casts_shadows && mat.extrude == 30.0);
        // 3D properties animate like any other.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation_y", "keys": [[0, -90], [1, 0]], "ease": "back.out"})], "turn", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.keyframes["transform.rotation_y"].keys.len(), 2);
        let insp = s.space_of(id).unwrap();
        assert_eq!(insp["three_d"], json!(true));
        s.edit(&[json!({"op": "three_d", "clip": id, "on": false})], "2d", false).unwrap();
        assert!(!s.model_clip(id).unwrap().transform.three_d);
    }

    #[test]
    fn camera_added_and_moves_key_it() {
        let (mut s, _, id) = session();
        s.edit(&[json!({"op": "three_d", "clip": id})], "3d", false).unwrap();
        let r = s.edit(&[json!({"op": "camera", "focal_length": 35, "move": {"type": "orbit", "by": 60, "duration": 2}})], "cam", false).unwrap();
        let cam = s.camera_at(0).expect("camera made");
        let mc = s.model_clip(cam).unwrap();
        assert!(mc.keyframes["transform.position"].keys.len() >= 10, "{r:?}");
        let w = 640.0;
        let st = sp::CameraSettings::read(&gen_params(&mc), w);
        assert!((sp::focal_length_from_zoom(st.zoom, w) - 35.0).abs() < 1e-6);
        // Orbiting keeps the distance to what it looks at.
        let a = rig_at(&mc, 0, w);
        let b = rig_at(&mc, 2 * SEC, w);
        assert!((len(sub(a.pos, a.poi)) - len(sub(b.pos, b.poi))).abs() < 1.0);
        assert!((b.pos[0] - a.pos[0]).abs() > 100.0);
        for mv in ["pan", "tilt", "dolly", "truck", "pedestal", "roll", "zoom", "dolly_zoom", "push_in", "shake"] {
            s.edit(&[json!({"op": "camera", "clip": cam, "move": {"type": mv, "from": 2, "duration": 1}})], mv, false).unwrap_or_else(|e| panic!("{mv}: {e}"));
        }
        let e = s.edit(&[json!({"op": "camera", "clip": cam, "move": "spin"})], "x", false).unwrap_err();
        assert!(e.contains("orbit"), "{e}");
    }

    #[test]
    fn lights_and_parallax() {
        let (mut s, bg, id) = session();
        s.edit(&[json!({"op": "light", "type": "spot", "look_at": [0, 0, 0], "shadows": true, "intensity": 120})], "spot", false).unwrap();
        let light = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let mc = s.model_clip(light).unwrap();
        let st = sp::LightSettings::read(&gen_params(&mc));
        assert_eq!(st.kind, sp::LightKind::Spot);
        assert!(st.casts_shadows && (st.intensity - 1.2).abs() < 1e-9);
        let r = s.edit(&[json!({"op": "parallax", "clips": [bg, id], "move": "truck"})], "px", false).unwrap();
        let back = s.model_clip(bg).unwrap();
        let front = s.model_clip(id).unwrap();
        assert!(back.transform.three_d && back.transform.position_z > front.transform.position_z, "{r:?}");
        // Pushed back and scaled up by the same ratio: the same size on
        // screen (the backdrop more, to cover the frame through the move).
        let zoom = sp::default_zoom(640.0);
        assert!(back.transform.scale_x >= (zoom + back.transform.position_z) / zoom - 1e-6);
        assert!((front.transform.scale_x - (zoom + front.transform.position_z) / zoom).abs() < 1e-6, "{}", front.transform.scale_x);
        assert!(s.camera_at(0).is_some());
    }
}
