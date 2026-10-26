// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `generate` with `task: "model3d"`: a 3D model made from pictures, or
//! a raw model file, and cleaned up so it can be used. All on this Mac.
//!
//! * `images`: one clean picture of the object (or several views; the front
//!   one is used) goes through a real image-to-3D model (see `image3d`),
//!   for any kind of object, as a background job. `method: "carve"` (and
//!   the fallback when that model cannot run) carves a solid from front,
//!   side and back pictures instead (each silhouette cuts the block the way
//!   a visual hull does; a single front picture gets a relief with a
//!   mirrored back), made watertight, rebuilt as even quads, unwrapped, and
//!   painted from the pictures. That suits simple hard-surface shapes only.
//! * `file`: a raw model (a scan, a downloaded or generated .glb, .obj)
//!   goes through the production finish: repaired, rebuilt as clean quads,
//!   unwrapped, its textures carried over, judged against the quality bars
//!   and retried until it clears them.
//!
//! Either way the result is a model clip, ready for `mesh`, `rig`, `pose`.

use super::models::{num, read_picture};
use super::*;
use studio_modeling::gen3d::{self, Relief, Side, View};
use studio_quality::hero;
use studio_quality::production::{self, Options};
use studio_quality::score::Class;

/// The side a picture shows, from its name: `front`, `back`, `left`,
/// `right`, `top`, `bottom`, `side` (the left).
fn side_in_name(name: &str) -> Option<Side> {
    let n = name.to_lowercase();
    let has = |w: &str| n.split(|c: char| !c.is_alphanumeric()).any(|t| t == w);
    [("front", Side::Front), ("back", Side::Back), ("rear", Side::Back), ("left", Side::Left), ("right", Side::Right), ("side", Side::Left), ("profile", Side::Left), ("top", Side::Top), ("bottom", Side::Bottom)].iter().find(|(w, _)| has(w)).map(|(_, s)| *s)
}

/// A picture's mask (0..1 per pixel): its alpha when it has any, else
/// the pixels unlike the plain background round its edge.
fn mask_of(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<f32>> {
    if rgba.chunks_exact(4).any(|p| p[3] < 200) {
        let share = rgba.chunks_exact(4).filter(|p| p[3] >= 128).count() as f32 / (w * h) as f32;
        return (share > 0.005).then(|| rgba.chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect());
    }
    hero::picture_mask(w, h, rgba).map(|m| m.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect())
}

impl Session {
    fn picture_path(&mut self, v: &Value) -> Result<PathBuf, String> {
        match v {
            Value::String(s) if s.starts_with("https://") => {
                let dir = std::env::temp_dir().join(format!("studio-refs-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&dir);
                let name = s.rsplit('/').next().and_then(|n| n.split(['?', '#']).next()).filter(|n| n.contains('.')).unwrap_or("view.png");
                let p = dir.join(name);
                let reply = studio_net::get(s, &[], std::time::Duration::from_secs(60))?;
                std::fs::write(&p, reply.bytes(64 << 20).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                Ok(p)
            }
            Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => {
                let p = PathBuf::from(s);
                if p.is_file() { Ok(p) } else { Err(format!("{s}: no such file")) }
            }
            _ => self.media_item(v).map_err(|e| format!("a picture is a file path, an https link or a media id ({e})"))?.path.ok_or_else(|| "that media has no file on disk".to_string()),
        }
    }

    /// The pictures of the request: each a picture value or
    /// `{"image": picture, "side": "front"}`.
    fn view_files(&mut self, a: &Value) -> Result<Vec<(PathBuf, Option<Side>)>, String> {
        let list: Vec<Value> = match a.get("images").or_else(|| a.get("image")) {
            Some(Value::Array(v)) => v.clone(),
            Some(v) => vec![v.clone()],
            None => Vec::new(),
        };
        let mut out = Vec::new();
        for v in &list {
            let (pic, side) = match v.get("image").or_else(|| v.get("file")) {
                Some(p) => (p.clone(), v.get("side").and_then(Value::as_str)),
                None => (v.clone(), None),
            };
            let side = match side {
                Some(s) => Some(Side::parse(&s.to_lowercase()).ok_or_else(|| format!("side \"{s}\": front, back, left, right, top, bottom"))?),
                None => None,
            };
            let path = self.picture_path(&pic)?;
            let side = side.or_else(|| path.file_stem().and_then(|s| s.to_str()).and_then(side_in_name));
            out.push((path, side));
        }
        Ok(out)
    }

    /// The task: see the module notes.
    pub(super) fn model3d(&mut self, a: &Value) -> Result<Value, String> {
        if let Some(p) = a.get("provider").and_then(Value::as_str).filter(|p| !matches!(*p, "local" | "auto")) {
            return Err(format!("provider \"{p}\": models are built on this Mac, from pictures (\"images\") or cleaned up from a model file (\"file\"); there is no hosted 3D generator"));
        }
        let name = a.get("name").and_then(Value::as_str).unwrap_or("Generated model").to_owned();
        let size = self.model_size(a);
        if let Some(file) = a.get("file").and_then(Value::as_str) {
            let path = self.input_file(file)?;
            return self.model3d_file(a, &name, size, &path);
        }
        let files = self.view_files(a)?;
        match a.get("method").and_then(Value::as_str).unwrap_or("auto") {
            "carve" => self.model3d_local(a, &name, size, files),
            "auto" | "image" => self.model3d_image(a, &name, size, &files),
            m => Err(format!("method \"{m}\": image (the default: a real image-to-3D model) or carve (a visual hull from front, side and back views, for simple hard-surface shapes only)")),
        }
    }

    /// The size a model is made at (`size`, else 60% of the canvas's short side).
    pub(super) fn model_size(&self, a: &Value) -> f32 {
        let short = self.doc.project.width.min(self.doc.project.height) as f32;
        num(a, "size").unwrap_or(short * 0.6).max(10.0)
    }

    /// The fallback when the image-to-3D model cannot run (not installed
    /// and not downloadable, an engine failure): the views carved into a
    /// solid, as before. For simple hard-surface shapes it is fine; the
    /// answer says it was the fallback.
    pub(super) fn carve_fallback(&mut self, a: &Value, name: &str, why: &str) -> Result<String, String> {
        let size = self.model_size(a);
        let files = self.view_files(a)?;
        let v = self.model3d_local(a, name, size, files)?;
        Ok(format!("{}; this is the fallback: the image-to-3D model could not run ({why}), so the views were carved into a solid, which only suits simple hard-surface shapes", v["model"].as_str().unwrap_or("model made")))
    }

    /// A raw model file, finished.
    fn model3d_file(&mut self, a: &Value, name: &str, size: f32, file: &std::path::Path) -> Result<Value, String> {
        let raw = studio_mesh::import::load(file, size)?;
        let mut notes = raw.notes.clone();
        notes.push(format!("{} triangles in {}", raw.mesh.triangles(), file.display()));
        let mesh = if a.get("clean_up").and_then(Value::as_bool).unwrap_or(true) {
            let class = Class::parse(a.get("class").and_then(Value::as_str).unwrap_or("prop")).ok_or("class is prop, product, character, creature, vehicle, architecture or environment")?;
            let o = Options { quads: a.get("faces").and_then(Value::as_u64).map(|n| n as usize), texture: a.get("texture").and_then(Value::as_u64).unwrap_or(1024).clamp(64, 4096) as u32, candidates: 2, retries: 1, class };
            let done = production::finish(&raw.mesh, None, &o);
            notes.push(format!("clean-up: {}", done.report.summary()));
            done.mesh
        } else {
            raw.mesh
        };
        let said = self.place_model(a, &mesh, name.to_owned(), notes)?;
        Ok(json!({"model": said, "file": file.display().to_string()}))
    }

    pub(super) fn model3d_local(&mut self, a: &Value, name: &str, size: f32, files: Vec<(PathBuf, Option<Side>)>) -> Result<Value, String> {
        if files.is_empty() {
            return Err("model3d needs \"images\": pictures of the object (front, and side, back or top for a full form) on a plain or transparent background: generate them with `transparent: true`".into());
        }
        if files.len() > 6 {
            return Err("up to six views (front, back, left, right, top, bottom)".into());
        }
        let order = [Side::Front, Side::Left, Side::Back, Side::Right, Side::Top, Side::Bottom];
        let mut taken: Vec<Side> = files.iter().filter_map(|f| f.1).collect();
        let mut views = Vec::new();
        for (path, side) in &files {
            let side = side.unwrap_or_else(|| {
                let s = order.iter().copied().find(|s| !taken.contains(s)).unwrap_or(Side::Front);
                taken.push(s);
                s
            });
            let tex = read_picture(&path.to_string_lossy())?;
            let (w, h) = (tex.width as usize, tex.height as usize);
            let mask = mask_of(w, h, &tex.rgba).ok_or_else(|| format!("{}: no clear object on a plain or transparent background; generate it again with transparent: true", path.display()))?;
            views.push(View { side, width: tex.width, height: tex.height, rgba: tex.rgba, mask: Some(mask), depth: None });
        }
        if !views.iter().any(|v| v.side == Side::Front) {
            return Err("one of the pictures must be the front view (name it front, or pass {\"image\": …, \"side\": \"front\"})".into());
        }
        let cells = a.get("cells").and_then(Value::as_u64).unwrap_or(96).clamp(24, 200) as usize;
        let relief = Relief { thickness: num(a, "thickness").unwrap_or(0.5), mirror: a.get("mirror").and_then(Value::as_bool).unwrap_or(true) };
        let carved = gen3d::carve(&views, size, cells, relief)?;
        let mut notes = vec![format!("carved from {} view{}: {}", views.len(), if views.len() == 1 { "" } else { "s" }, views.iter().map(|v| format!("{:?}", v.side).to_lowercase()).collect::<Vec<_>>().join(", "))];
        let faces = a.get("faces").and_then(Value::as_u64).unwrap_or(3000).clamp(200, 60_000) as usize;
        let cleaned = gen3d::clean_up(&carved, faces)?;
        notes.extend(cleaned.notes.clone());
        let mut mesh = cleaned.mesh.to_mesh(40.0, studio_modeling::poly::NormalMode::Angle);
        let tex = a.get("texture").and_then(Value::as_u64).unwrap_or(512).clamp(0, 2048) as u32;
        let mut material = studio_mesh::Material { name: name.into(), roughness: 0.55, ..Default::default() };
        if tex >= 16 {
            material.texture = Some(gen3d::paint_from_views(&cleaned.mesh, &views, tex).to_rgba8(true));
            notes.push(format!("painted from the pictures ({tex} px)"));
        }
        mesh.materials = vec![material];
        let n = mesh.triangles();
        mesh.regroup(&vec![0; n]);
        mesh.tidy();
        let said = self.place_model(a, &mesh, name.to_owned(), notes)?;
        Ok(json!({"model": said, "provider": "local"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    /// A picture of a disc (see-through outside), written as a PNG.
    fn disc(dir: &std::path::Path, name: &str) -> String {
        let n = 64u32;
        let mut px = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (u, v) = (x as f32 / n as f32 - 0.5, y as f32 / n as f32 - 0.5);
                let inside = (u * u + v * v).sqrt() < 0.4;
                px.extend_from_slice(&[200, 60, 40, if inside { 255 } else { 0 }]);
            }
        }
        let path = dir.join(name);
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, n, n);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&px).unwrap();
        std::fs::write(&path, out).unwrap();
        path.display().to_string()
    }

    #[test]
    fn three_views_become_a_cleaned_painted_model_clip() {
        let dir = std::env::temp_dir().join(format!("studio-model3d-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (f, sd, t) = (disc(&dir, "ball front.png"), disc(&dir, "ball side.png"), disc(&dir, "ball top.png"));
        let mut s = Session::new(services::project::new_custom("T", 640, 360, 30.0));
        let r = s.generate(&json!({"task": "model3d", "images": [f, sd, t], "method": "carve", "size": 100, "cells": 40, "faces": 300, "texture": 64, "name": "Ball", "duration": 3})).unwrap();
        assert!(r["model"].as_str().unwrap().contains("model clip"), "{r}");
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.triangles() > 100 && mesh.materials[0].texture.is_some(), "{} triangles", mesh.triangles());
        let (lo, hi) = mesh.bounds();
        assert!((hi[2] - lo[2]) > 60.0, "a full form, not a relief: {:?}", (lo, hi));
        // A view that is only background is refused with the way out.
        let blank = dir.join("blank.png");
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 16, 16);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&vec![255u8; 16 * 16 * 4]).unwrap();
        std::fs::write(&blank, out).unwrap();
        let err = s.generate(&json!({"task": "model3d", "method": "carve", "images": [blank.display().to_string()]})).unwrap_err();
        assert!(err.contains("transparent: true"), "{err}");
    }

    #[test]
    fn there_is_no_hosted_generator_and_a_raw_file_is_finished() {
        let dir = std::env::temp_dir().join(format!("studio-model3d-f-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(services::project::new_custom("T", 640, 360, 30.0));
        let err = s.generate(&json!({"task": "model3d", "provider": "trellis", "images": []})).unwrap_err();
        assert!(err.contains("no hosted 3D generator"), "{err}");
        // A raw model file (an icosphere as OBJ) comes back cleaned.
        let ball = studio_mesh::prim::by_name("sphere", 100.0, 3).unwrap();
        let (obj, _, _) = studio_mesh::export::obj(&ball, "x.mtl");
        let path = dir.join("raw.obj");
        std::fs::write(&path, obj).unwrap();
        let r = s.run("generate", &json!({"task": "model3d", "file": path.display().to_string(), "faces": 300, "texture": 64, "name": "Raw"})).unwrap();
        let said = r.iter().find_map(|c| if let crate::agent::Content::Text(t) = c { Some(t.clone()) } else { None }).unwrap_or_default();
        assert!(said.contains("clean-up"), "{said}");
        assert!(side_in_name("case reference front") == Some(Side::Front) && side_in_name("case side view") == Some(Side::Left));
    }
}
