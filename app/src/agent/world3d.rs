// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Building and shooting whole 3D sets for the agent:
//! - `world`: a sky with the sun where the time of day puts it (or a
//!   panoramic HDR picture lighting everything), ground (terrain: hills,
//!   mountains, dunes, plains, canyon, island), and trees, bushes, rocks,
//!   grass and flowers scattered over it by rules.
//! - `render3d`: how the camera renders models: path traced (light that
//!   bounces, soft shadows, glass, area lights, depth of field, motion
//!   blur, denoised) or raster (fast); and render passes (beauty, depth,
//!   normals, albedo, object ids) written out for compositing.

use super::*;

fn v3(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

fn param(c: &mut m::Clip, k: &str, v: m::Value) {
    if let m::ClipSource::Generator(g) = &mut c.source {
        g.params.insert(k.to_string(), v);
    }
}

fn gen_kind(c: &m::Clip) -> Option<m::GeneratorKind> {
    match &c.source {
        m::ClipSource::Generator(g) => Some(g.kind),
        _ => None,
    }
}

impl Session {
    /// A camera or light clip on a free lane over the whole sequence.
    pub(super) fn add_generator(&mut self, kind: m::GeneratorKind, name: &str, params: m::ParamMap, pos: [f64; 3]) -> Result<u32, String> {
        let dur = super::new_clip_end(self.doc.project.duration(), 0);
        let lane = self.room(None, 0, dur, None)?;
        let id = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(kind);
        g.params = params;
        let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), 0, dur, m::ClipSource::Generator(g));
        c.name = name.to_owned();
        c.transform.position = m::Vec2::new(pos[0], pos[1]);
        c.transform.position_z = pos[2];
        let seq = self.doc.seq();
        let ok = self.doc.apply(&format!("Add {}", name.to_lowercase()), m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(id)
    }

    /// The first clip whose generator is `kind` and passes `pick`.
    pub(super) fn find_generator(&self, kind: m::GeneratorKind, pick: impl Fn(&m::Clip) -> bool) -> Option<m::Clip> {
        let seq = self.doc.model.sequence(self.doc.seq())?;
        seq.clips.iter().find(|c| gen_kind(c) == Some(kind) && pick(c)).cloned()
    }

    pub(super) fn world(&mut self, op: &Value) -> Result<String, String> {
        let mut said = Vec::new();
        let (w, h) = (self.doc.project.width as f32, self.doc.project.height as f32);
        // The sky and sun.
        if let Some(sky) = op.get("sky").filter(|v| !v.is_null() && v.as_bool() != Some(false)) {
            let existing = self.find_generator(m::GeneratorKind::Light, |c| matches!(&c.source, m::ClipSource::Generator(g) if matches!(g.params.get("sky"), Some(m::Value::Bool(true)))));
            let mut mc = match existing {
                Some(c) => c,
                None => {
                    let mut p = studio_model::space::LightSettings::defaults(studio_model::space::LightKind::Parallel);
                    p.insert("casts_shadows".into(), m::Value::Bool(true));
                    p.insert("shadow_darkness".into(), m::Value::Scalar(0.85));
                    let id = self.add_generator(m::GeneratorKind::Light, "Sun & sky", p, [0.0, -(h as f64), -(w as f64)])?;
                    self.model_clip(id).ok_or("gone")?
                }
            };
            param(&mut mc, "sky", m::Value::Bool(true));
            let o = sky.as_object();
            let get = |k: &str| o.and_then(|o| o.get(k));
            let n = |k: &str, d: f64| get(k).and_then(Value::as_f64).unwrap_or(d);
            let time = match get("time") {
                Some(Value::String(s)) => match s.as_str() {
                    "dawn" | "sunrise" => 6.0,
                    "morning" => 9.0,
                    "noon" | "midday" => 12.5,
                    "afternoon" => 15.5,
                    "golden" | "golden_hour" => 18.4,
                    "sunset" | "dusk" => 19.15,
                    "twilight" | "blue_hour" => 19.7,
                    "night" => 23.0,
                    other => return Err(format!("time \"{other}\": hours (0–24) or dawn, morning, noon, afternoon, golden_hour, sunset, twilight, night")),
                },
                Some(v) => v.as_f64().unwrap_or(14.0),
                None => 14.0,
            };
            param(&mut mc, "time", m::Value::Scalar(time));
            param(&mut mc, "day", m::Value::Scalar(n("day", 172.0)));
            param(&mut mc, "latitude", m::Value::Scalar(n("latitude", 40.0)));
            param(&mut mc, "heading", m::Value::Scalar(n("heading", 0.0)));
            param(&mut mc, "turbidity", m::Value::Scalar(n("turbidity", 3.0).clamp(1.7, 12.0)));
            param(&mut mc, "sky_strength", m::Value::Scalar(n("strength", 1.0).max(0.0)));
            param(&mut mc, "haze", m::Value::Scalar(n("haze", 0.0).max(0.0)));
            param(&mut mc, "background", m::Value::Bool(get("background").and_then(Value::as_bool).unwrap_or(true)));
            if let Some(g) = get("ground").and_then(Value::as_str) {
                let c = super::craft::colour(g).ok_or_else(|| format!("ground colour \"{g}\" is not #rrggbb or a basic name"))?;
                param(&mut mc, "ground", m::Value::Colour(m::Rgba::new(c.r() as f64 / 255.0, c.g() as f64 / 255.0, c.b() as f64 / 255.0, 1.0)));
            }
            let id = mc.id.0;
            let seq = self.doc.seq();
            let ok = self.doc.apply("Sky", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            let sun = studio_render::sky::sun_for_time(time as f32, n("day", 172.0) as f32, n("latitude", 40.0) as f32, n("heading", 0.0) as f32);
            said.push(format!("sky (light clip {id}) at {time:.1} h, sun {:.0}° up", sun[1].clamp(-1.0, 1.0).asin().to_degrees()));
        }
        if let Some(path) = op.get("hdri").and_then(Value::as_str) {
            studio_render::sky::load_hdri(path)?;
            let mut p = studio_model::space::LightSettings::defaults(studio_model::space::LightKind::Ambient);
            p.insert("hdri".into(), m::Value::Text(path.to_string()));
            p.insert("hdri_turn".into(), m::Value::Scalar(op.get("hdri_turn").and_then(Value::as_f64).unwrap_or(0.0)));
            p.insert("sky_strength".into(), m::Value::Scalar(op.get("hdri_strength").and_then(Value::as_f64).unwrap_or(1.0)));
            p.insert("background".into(), m::Value::Bool(op.get("background").and_then(Value::as_bool).unwrap_or(true)));
            let id = self.add_generator(m::GeneratorKind::Light, "Environment", p, [0.0, 0.0, 0.0])?;
            said.push(format!("panorama lighting as light clip {id}"));
        }
        // Ground.
        let mut ground: Option<(u32, studio_mesh::Mesh, f32)> = None;
        if let Some(t) = op.get("terrain").filter(|v| !v.is_null() && v.as_bool() != Some(false)) {
            let o = t.as_object();
            let get = |k: &str| o.and_then(|o| o.get(k));
            let style_name = get("style").and_then(Value::as_str).unwrap_or("hills");
            let style = studio_mesh::terrain::Style::parse(style_name).ok_or_else(|| format!("terrain style \"{style_name}\": {}", studio_mesh::terrain::STYLES.join(", ")))?;
            let size = match get("size") {
                Some(Value::Array(a)) if a.len() == 2 => [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32],
                Some(v) => [v.as_f64().unwrap_or(0.0) as f32; 2],
                None => [w * 6.0, w * 6.0],
            };
            let height = get("height").and_then(Value::as_f64).map_or(size[0] * 0.08, |v| v as f32);
            let land = studio_mesh::terrain::Land { size, height, style, cells: get("cells").and_then(Value::as_u64).unwrap_or(220) as usize, seed: get("seed").and_then(Value::as_u64).unwrap_or(1) as u32 };
            let mesh = studio_mesh::terrain::terrain(&land);
            let dur = super::new_clip_end(self.doc.project.duration(), 0);
            let file = crate::mesh_store::save(&self.doc, &mesh)?;
            let lane = self.room(None, 0, dur, None)?;
            let id = self.doc.alloc_id() as u32;
            let mut c = crate::mesh_store::new_clip(id as u64, lane as u64, 0, dur, file, "Terrain");
            // The ground at the bottom of the frame, starting behind the
            // camera and running away into the distance.
            let zoom = studio_model::space::default_zoom(w as f64) as f32;
            let p = get("position").and_then(v3).unwrap_or([0.0, h * 0.5, size[1] * 0.5 - zoom * 1.2]);
            c.transform.position = m::Vec2::new(p[0] as f64, p[1] as f64);
            c.transform.position_z = p[2] as f64;
            let seq = self.doc.seq();
            let ok = self.doc.apply("Add terrain", m::Edit::InsertClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            said.push(format!("{style_name} terrain as clip {id} ({:.0} × {:.0} px, {:.0} px high)", size[0], size[1], height));
            ground = Some((id, mesh, height));
        }
        if let Some(Value::Array(list)) = op.get("scatter") {
            let (gid, gmesh, gheight) = match (ground.take(), op.get("on").and_then(|v| super::sim3d::id_of(&self.doc, v))) {
                (Some(g), _) => g,
                (None, Some(on)) => {
                    let (_, m) = self.model_of(on as u32)?;
                    let (lo, hi) = m.bounds();
                    (on as u32, m, (hi[1] - lo[1]).max(1.0))
                }
                (None, None) => return Err("scatter needs ground: \"terrain\" in the same op, or \"on\": a model clip".into()),
            };
            let mut rules = Vec::new();
            for (i, r) in list.iter().enumerate() {
                let kind = r.get("kind").and_then(Value::as_str).ok_or_else(|| format!("scatter {}: needs \"kind\" ({})", i + 1, studio_mesh::terrain::KINDS.join(", ")))?;
                if !studio_mesh::terrain::KINDS.contains(&kind) {
                    return Err(format!("scatter {}: kind \"{kind}\": {}", i + 1, studio_mesh::terrain::KINDS.join(", ")));
                }
                let default_size = match kind {
                    "grass" | "flower" => h * 0.04,
                    "rock" => h * 0.12,
                    "bush" => h * 0.15,
                    _ => h * 0.5,
                };
                rules.push(studio_mesh::terrain::Scatter {
                    kind: kind.to_string(),
                    count: r.get("count").and_then(Value::as_u64).unwrap_or(100).min(20_000) as usize,
                    size: r.get("size").and_then(Value::as_f64).map_or(default_size, |v| v as f32),
                    max_slope: r.get("max_slope").and_then(Value::as_f64).unwrap_or(if kind == "rock" { 60.0 } else { 32.0 }) as f32,
                    min_height: r.get("min_height").and_then(Value::as_f64).unwrap_or(0.04) as f32,
                    max_height: r.get("max_height").and_then(Value::as_f64).unwrap_or(0.85) as f32,
                    clumping: r.get("clumping").and_then(Value::as_f64).unwrap_or(0.3) as f32,
                    seed: r.get("seed").and_then(Value::as_u64).unwrap_or(i as u64 + 1) as u32,
                });
            }
            let mesh = studio_mesh::terrain::scatter(&gmesh, gheight, &rules);
            if mesh.triangles() == 0 {
                return Err("nothing could be placed (too steep, too high, or no ground under the area)".into());
            }
            if mesh.triangles() > 1_500_000 {
                return Err(format!("that makes {} triangles; scatter fewer (under 1.5 million)", mesh.triangles()));
            }
            let gclip = self.model_clip(gid).ok_or("the ground clip is gone")?;
            let dur = super::new_clip_end(self.doc.project.duration(), 0);
            let file = crate::mesh_store::save(&self.doc, &mesh)?;
            let lane = self.room(None, 0, dur, None)?;
            let id = self.doc.alloc_id() as u32;
            let mut c = crate::mesh_store::new_clip(id as u64, lane as u64, 0, dur, file, "Scatter");
            // Same place as the ground it stands on.
            c.transform = gclip.transform.clone();
            let seq = self.doc.seq();
            let ok = self.doc.apply("Scatter", m::Edit::InsertClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            said.push(format!("scattered {} ({} triangles) as clip {id}", rules.iter().map(|r| format!("{} {}", r.count, r.kind)).collect::<Vec<_>>().join(", "), mesh.triangles()));
        }
        if said.is_empty() {
            return Err("world: give \"sky\" ({\"time\": 17.5…} or true), \"hdri\" (a .hdr panorama), \"terrain\" ({\"style\": \"hills\"…}) and/or \"scatter\" ([{\"kind\": \"pine\", \"count\": 200}…])".into());
        }
        Ok(said.join("; "))
    }

    pub(super) fn render3d(&mut self, op: &Value) -> Result<String, String> {
        let w = self.doc.project.width as f64;
        let mut said = Vec::new();
        // The camera that shoots the models (made if there is none).
        let cam = match op.get("camera").and_then(|v| super::sim3d::id_of(&self.doc, v)) {
            Some(id) => self.model_clip(id as u32).ok_or_else(|| format!("no clip {id}"))?,
            None => match self.find_generator(m::GeneratorKind::Camera, |_| true) {
                Some(c) => c,
                None => {
                    let zoom = studio_model::space::default_zoom(w);
                    let mut p = studio_model::space::CameraSettings::defaults(w);
                    p.insert("two_node".into(), m::Value::Bool(true));
                    let id = self.add_generator(m::GeneratorKind::Camera, "Camera", p, [0.0, 0.0, -zoom])?;
                    said.push(format!("camera added as clip {id}"));
                    self.model_clip(id).ok_or("gone")?
                }
            },
        };
        if gen_kind(&cam) != Some(m::GeneratorKind::Camera) {
            return Err(format!("clip {} is not a camera", cam.id.0));
        }
        let mut mc = cam;
        let mode = op.get("mode").and_then(Value::as_str);
        if let Some(mode) = mode {
            let v = match mode {
                "path" | "path_traced" | "path_tracing" | "final" | "cycles" => "path",
                "raster" | "fast" | "preview" => "raster",
                other => return Err(format!("mode \"{other}\": path (bouncing light, final quality) or raster (fast)")),
            };
            param(&mut mc, "render", m::Value::Text(v.into()));
            said.push(format!("models render {}", if v == "path" { "path traced" } else { "raster" }));
        }
        for (k, key) in [("samples", "samples"), ("preview_samples", "preview_samples"), ("bounces", "bounces"), ("clamp", "clamp")] {
            if let Some(v) = op.get(k).and_then(Value::as_f64) {
                param(&mut mc, key, m::Value::Scalar(v));
                said.push(format!("{k} {v}"));
            }
        }
        if let Some(v) = op.get("denoise").and_then(Value::as_bool) {
            param(&mut mc, "denoise", m::Value::Bool(v));
        }
        if let Some(v) = op.get("motion_blur") {
            let s = match v {
                Value::Bool(true) => 0.5,
                Value::Bool(false) => 0.0,
                v => v.as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
            };
            param(&mut mc, "shutter", m::Value::Scalar(s));
            said.push(format!("motion blur, shutter {:.0}°", s * 360.0));
        }
        let id = mc.id.0;
        let seq = self.doc.seq();
        let ok = self.doc.apply("Render settings", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        if let Some(p) = op.get("passes") {
            said.push(self.render_passes(id as u32, p)?);
        }
        if said.is_empty() {
            return Err("render3d: give \"mode\" (path or raster), \"samples\", \"bounces\", \"denoise\", \"motion_blur\" or \"passes\": {\"at\", \"out\"}".into());
        }
        Ok(format!("camera {id}: {}", said.join("; ")))
    }

    /// Path-traces the models at one moment and writes their passes as PNG
    /// files: beauty, depth, normal, albedo, id.
    fn render_passes(&mut self, camera: u32, p: &Value) -> Result<String, String> {
        let t = micros(p.get("at").and_then(Value::as_f64).unwrap_or(0.0));
        let out = p.get("out").and_then(Value::as_str).ok_or("passes need \"out\": a folder")?;
        let dir = std::path::Path::new(out);
        std::fs::create_dir_all(dir).map_err(|e| format!("{out}: {e}"))?;
        let project = std::sync::Arc::new(self.doc.model.clone());
        let scene = studio_render::model_scene::ModelScene::for_sequence(project, self.doc.seq());
        let list = studio_render::resolve::resolve(&scene, t);
        let mut desc = studio_render::mesh_render::MeshDesc::default();
        for n in &list.nodes {
            if let studio_render::resolve::Node::Layer { clip, .. } = n
                && let studio_render::scene::SourceDesc::Mesh(m) = &clip.source
            {
                desc.instances.extend(m.instances.iter().cloned());
                desc.camera = m.camera;
                desc.lights = m.lights.clone();
            }
        }
        if desc.instances.is_empty() {
            return Err(format!("no 3D models show at {:.2} s", t as f64 / 1e6));
        }
        let cam = self.model_clip(camera).ok_or("the camera is gone")?;
        let settings = {
            let n = |k: &str, d: f64| match &cam.source {
                m::ClipSource::Generator(g) => g.params.get(k).and_then(|v| v.as_f64()).unwrap_or(d),
                _ => d,
            };
            studio_render::trace::TraceSettings { samples: n("samples", 64.0) as u32, preview_samples: 8, bounces: n("bounces", 6.0) as u32, denoise: true, clamp: n("clamp", 24.0) as f32, shutter: n("shutter", 0.0) as f32 }
        };
        let (w, h) = (list.canvas.width, list.canvas.height);
        let passes = studio_render::trace::render(&desc, &list.canvas, (w, h), &settings);
        let save = |name: &str, f: &studio_render::frame::Frame, linear: bool| -> Result<(), String> {
            let to8 = |v: f32| {
                let c = v.clamp(0.0, 1.0);
                let s = if linear { if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } } else { c };
                (s * 255.0).round() as u8
            };
            let mut px = Vec::with_capacity(f.pixels.len() * 4);
            for p in &f.pixels {
                let a = p[3];
                let un = |c: f32| if linear && a > 0.0 { c / a } else { c };
                px.extend_from_slice(&[to8(un(p[0])), to8(un(p[1])), to8(un(p[2])), (a.clamp(0.0, 1.0) * 255.0).round() as u8]);
            }
            let img = studio_media::RgbaImage { width: f.width, height: f.height, pixels: px };
            std::fs::write(dir.join(format!("{name}.png")), studio_media::png::encode(&img)).map_err(|e| format!("{name}: {e}"))
        };
        save("beauty", &passes.beauty, true)?;
        save("depth", &passes.depth_frame(w, h), false)?;
        save("normal", &passes.normal_frame(w, h), false)?;
        save("albedo", &passes.albedo_frame(w, h), true)?;
        save("id", &passes.id_frame(w, h), false)?;
        Ok(format!("passes at {:.2} s written to {out}: beauty, depth, normal, albedo, id ({} samples)", t as f64 / 1e6, settings.samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn a_set_with_sky_terrain_trees_and_a_path_traced_camera() {
        let mut s = Session::new(services::project::new_custom("World", 320, 180, 24.0));
        let r = s.edit(&[json!({"op": "world", "sky": {"time": "golden_hour", "haze": 0.3}, "terrain": {"style": "hills", "cells": 40}, "scatter": [{"kind": "pine", "count": 20}, {"kind": "rock", "count": 10}]})], "set", false).unwrap();
        let txt = r.to_string();
        assert!(txt.contains("sky") && txt.contains("terrain") && txt.contains("scattered"), "{txt}");
        let r = s.edit(&[json!({"op": "render3d", "mode": "path", "samples": 4, "motion_blur": true})], "render", false).unwrap();
        assert!(r.to_string().contains("path traced"), "{r}");
        assert!(s.edit(&[json!({"op": "world", "terrain": {"style": "volcano"}})], "x", false).is_err());
    }
}
