// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `generate` with `task: "model3d"` and pictures: a real image-to-3D model
//! (TripoSR, on this Mac) turns one clean picture of an object into a
//! textured model, for any kind of object. It runs as a background job:
//! the call returns a job number, `generate {job}` shows progress (`wait`
//! blocks, `cancel` stops it) and the finished model lands as a model clip,
//! kept in My Models, announced after the next tool call.
//!
//! The model reads one picture. When several views are given, the front
//! one (or the first) is the reference. The surface it finds is rebuilt as
//! clean quads, unwrapped, and its colours baked into a texture with a PBR
//! material.

use super::models::read_picture;
use super::*;
use std::time::Instant;
use studio_ai::features::image23d::{self, ImageTo3d, Params};
use studio_mesh::Mesh;
use studio_ai::{JobCtx, JobHandle, Jobs, ModelId, ModelStore};
use studio_modeling::gen3d::Side;
use studio_quality::scan::{self, ModelOptions};
use studio_quality::score::Class;

/// What a finished job hands back.
pub(super) struct Made {
    mesh: Mesh,
    notes: Vec<String>,
}

pub(super) struct ModelJob {
    handle: JobHandle<Made>,
    /// The request, for placing the clip.
    op: Value,
    name: String,
    started: Instant,
    /// Once the job is over: the line for the clip it made, or why not.
    outcome: Option<Result<String, String>>,
    told: bool,
}

#[cfg(test)]
thread_local! {
    /// A models folder for tests (and no downloading into it).
    pub(super) static TEST_STORE: std::cell::RefCell<Option<ModelStore>> = const { std::cell::RefCell::new(None) };
}

/// The models folder, and whether a missing model may be downloaded.
fn store_for() -> (ModelStore, bool) {
    #[cfg(test)]
    if let Some(s) = TEST_STORE.with(|t| t.borrow().clone()) {
        return (s, false);
    }
    (ModelStore::default(), true)
}

fn jobs() -> &'static Jobs {
    static J: std::sync::OnceLock<Jobs> = std::sync::OnceLock::new();
    J.get_or_init(|| Jobs::new(1))
}

/// Alpha for a picture: its own when it has one, else a plain background
/// cut away, else the object found by the cut-out model.
fn subject(store: &ModelStore, w: usize, h: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    image23d::isolate(store, w, h, rgba).map(|(p, _)| p).ok_or_else(|| "no clear object in the picture: use one object on a plain background (generate it with transparent: true), or install the object cut-out model".to_string())
}

impl Session {
    /// Starts the job for `files` (the pictures of the request).
    pub(super) fn model3d_image(&mut self, a: &Value, name: &str, size: f32, files: &[(PathBuf, Option<Side>)]) -> Result<Value, String> {
        let (path, _) = files.iter().find(|f| f.1 == Some(Side::Front)).or_else(|| files.first()).ok_or("model3d needs \"images\"")?;
        let pic = read_picture(&path.to_string_lossy())?;
        let (w, h) = (pic.width as usize, pic.height as usize);
        let (store, download) = store_for();
        let rgba = subject(&store, w, h, &pic.rgba)?;
        let class = Class::parse(a.get("class").and_then(Value::as_str).unwrap_or("prop")).ok_or("class is prop, product, character, creature, vehicle, architecture or environment")?;
        let params = Params { resolution: a.get("resolution").and_then(Value::as_u64).unwrap_or(256) as usize, ..Default::default() };
        let opts = ModelOptions { size, quads: a.get("faces").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(10_000).clamp(500, 60_000), texture: a.get("texture").and_then(Value::as_u64).unwrap_or(1024).clamp(128, 4096) as u32, class, smoothing: 3 };
        let ignored = files.len().saturating_sub(1);
        let handle = jobs().spawn(format!("3D model: {name}"), move |ctx: &JobCtx| make(&store, download, w, h, &rgba, &params, &opts, ctx));
        let id = self.next_batch;
        self.next_batch += 1;
        self.model_jobs.push((id, ModelJob { handle, op: a.clone(), name: name.to_owned(), started: Instant::now(), outcome: None, told: false }));
        if a.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            return self.model_wait(id);
        }
        let mut v = json!({
            "job": id,
            "started": "image-to-3D model",
            "note": format!("The model is being made in the background (about a minute). Keep working; generate {{\"job\": {id}}} shows progress, add wait: true to wait for it. When it finishes it lands as a model clip, kept in My Models, and is announced after your next tool calls."),
        });
        if ignored > 0 {
            v["views_used"] = json!(format!("1 of {}: the model reads one picture (the front one)", files.len()));
        }
        Ok(v)
    }

    /// `generate {job}` for a model job.
    pub(super) fn model_job_op(&mut self, a: &Value, id: u32) -> Result<Value, String> {
        if a.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
            if let Some((_, j)) = self.model_jobs.iter().find(|(i, _)| *i == id) {
                j.handle.cancel();
            }
        }
        if a.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            return self.model_wait(id);
        }
        self.model_view(id)
    }

    fn model_wait(&mut self, id: u32) -> Result<Value, String> {
        let deadline = Instant::now() + std::time::Duration::from_secs(30 * 60);
        loop {
            let v = self.model_view(id)?;
            if v["state"] != "running" || Instant::now() > deadline {
                return Ok(v);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn model_view(&mut self, id: u32) -> Result<Value, String> {
        self.land_models();
        let j = self.model_jobs.iter_mut().find(|(i, _)| *i == id).map(|(_, j)| j).ok_or_else(|| format!("no job {id}"))?;
        let mut v = json!({"job": id, "name": j.name, "seconds": j.started.elapsed().as_secs()});
        match &j.outcome {
            None => {
                v["state"] = json!("running");
                v["progress"] = json!((j.handle.progress() * 100.0).round() / 100.0);
                v["doing"] = json!(j.handle.message());
            }
            Some(Ok(said)) => {
                j.told = true;
                v["state"] = json!("done");
                v["model"] = json!(said);
            }
            Some(Err(e)) => {
                j.told = true;
                v["state"] = json!("failed");
                v["error"] = json!(e);
            }
        }
        Ok(v)
    }

    /// Puts every finished model on the timeline.
    pub(super) fn land_models(&mut self) {
        let done: Vec<u32> = self.model_jobs.iter().filter(|(_, j)| j.outcome.is_none() && j.handle.is_finished()).map(|(i, _)| *i).collect();
        for id in done {
            let Some(pos) = self.model_jobs.iter().position(|(i, _)| *i == id) else { continue };
            let result = self.model_jobs[pos].1.handle.take_result();
            let (op, name) = (self.model_jobs[pos].1.op.clone(), self.model_jobs[pos].1.name.clone());
            let outcome = match result {
                Some(Ok(made)) => self.place_model(&op, &made.mesh, name, made.notes),
                Some(Err(e)) if !matches!(e, studio_ai::Error::Cancelled) && op.get("method").and_then(Value::as_str) != Some("image") => match self.carve_fallback(&op, &name, &e.to_string()) {
                    Ok(said) => Ok(said),
                    Err(c) => Err(format!("{e}; the carving fallback failed too: {c}")),
                },
                Some(Err(e)) => Err(e.to_string()),
                None => Err("the job's result was already taken".into()),
            };
            self.model_jobs[pos].1.outcome = Some(outcome);
        }
    }

    /// News of model jobs for a tool's reply: models that landed or
    /// failed. None when there is none.
    pub(super) fn models_note(&mut self) -> Option<Value> {
        if self.model_jobs.is_empty() {
            return None;
        }
        self.land_models();
        let (mut ready, mut failed, mut running) = (Vec::new(), Vec::new(), Vec::new());
        for (id, j) in &mut self.model_jobs {
            match &j.outcome {
                Some(Ok(said)) if !j.told => ready.push(json!({"job": id, "name": j.name, "model": said})),
                Some(Err(e)) if !j.told => failed.push(json!({"job": id, "name": j.name, "error": e})),
                None => running.push(json!({"job": id, "name": j.name, "progress": (j.handle.progress() * 100.0).round() / 100.0, "doing": j.handle.message()})),
                _ => {}
            }
            if j.outcome.is_some() {
                j.told = true;
            }
        }
        self.model_jobs.retain(|(_, j)| j.outcome.is_none() || !j.told);
        if ready.is_empty() && failed.is_empty() {
            return None;
        }
        Some(json!({"models_ready": ready, "models_failed": failed, "still_making": running}))
    }
}

/// The job: download the model when it is missing, run it, finish the surface.
fn make(store: &ModelStore, download: bool, w: usize, h: usize, rgba: &[u8], params: &Params, opts: &ModelOptions, ctx: &JobCtx) -> studio_ai::Result<Made> {
    let mut first = 0.0;
    if !store.is_installed(ModelId::TripoSr) {
        if !download {
            return Err(studio_ai::Error::ModelMissing(ModelId::TripoSr));
        }
        let spec = ModelId::TripoSr.spec();
        ctx.set_message(format!("Downloading the 3D model ({})", studio_ai::registry::human_size(spec.download_size())));
        store.download(ModelId::TripoSr, &ctx.range(0.0, 0.4))?;
        first = 0.4;
    }
    let net = ImageTo3d::load(store)?;
    let raw = net.generate(w, h, rgba, params, &ctx.range(first, 0.85))?;
    ctx.check()?;
    ctx.set_message("Rebuilding it as clean quads and painting it");
    let (mesh, mut notes) = scan::to_model(&raw.positions, &raw.colours, &raw.triangles, opts);
    ctx.set_progress(1.0);
    notes.insert(0, "made from the picture by the TripoSR image-to-3D model".into());
    Ok(Made { mesh, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    /// A fixed test picture: a shaded red ball with a stripe, see-through round it.
    fn ball(dir: &std::path::Path, name: &str) -> String {
        let n = 128u32;
        let mut px = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (u, v) = (x as f32 / n as f32 * 2.0 - 1.0, y as f32 / n as f32 * 2.0 - 1.0);
                let r2 = u * u + v * v;
                if r2 < 0.64 {
                    let shade = (1.0 - 0.6 * r2 / 0.64).clamp(0.0, 1.0);
                    let band = if (v * 5.0).sin() > 0.6 { 1.0 } else { 0.0 };
                    px.extend_from_slice(&[(230.0 * shade) as u8, (60.0 * shade + 150.0 * band) as u8, (50.0 * shade + 60.0 * band) as u8, 255]);
                } else {
                    px.extend_from_slice(&[0, 0, 0, 0]);
                }
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

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-image3d-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn said(r: Vec<crate::agent::Content>) -> Value {
        let t = r.iter().find_map(|c| if let crate::agent::Content::Text(t) = c { Some(t.clone()) } else { None }).unwrap_or_default();
        serde_json::from_str(&t).unwrap_or(Value::String(t))
    }

    /// The real model, a fixed picture: a background job makes a textured,
    /// closed-ish model clip. Skipped when the model is not installed.
    #[test]
    fn a_picture_becomes_a_textured_model_in_a_background_job() {
        if !ModelStore::default().is_installed(ModelId::TripoSr) {
            eprintln!("TripoSR not installed; skipping");
            return;
        }
        let dir = temp("real");
        let pic = ball(&dir, "ball.png");
        let mut s = Session::new(services::project::new_custom("T", 640, 360, 30.0));
        let started = said(s.run("generate", &json!({"task": "model3d", "images": [pic], "size": 100, "faces": 1500, "texture": 256, "name": "Ball"})).unwrap());
        // It is a job, not a result: no model clip yet.
        let job = started["job"].as_u64().expect("a job number");
        assert!(s.doc.project.clips.is_empty(), "the clip must not exist before the job lands: {started}");
        let seen = said(s.run("generate", &json!({"job": job})).unwrap());
        assert!(["running", "done"].contains(&seen["state"].as_str().unwrap()), "{seen}");
        let done = said(s.run("generate", &json!({"job": job, "wait": true})).unwrap());
        assert_eq!(done["state"], "done", "{done}");
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.triangles() > 300, "{} triangles", mesh.triangles());
        assert!(mesh.uvs.len() == mesh.positions.len() && mesh.materials.iter().any(|m| m.texture.is_some()), "UVs and a texture");
        let t = studio_quality::topo::analyse(&mesh);
        assert!(t.boundary_edges * 50 < t.triangles * 3, "watertight-ish: {} open edges of {} triangles", t.boundary_edges, t.triangles);
        let (lo, hi) = mesh.bounds();
        assert!((0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max) > 80.0, "sized to the request");
    }

    /// With the model unavailable, the views are carved instead and the
    /// answer says it was the fallback; `method: "image"` does not fall back.
    #[test]
    fn the_carving_fallback_still_works_when_the_model_cannot_run() {
        let dir = temp("fallback");
        let empty = ModelStore::new(dir.join("no-models"));
        TEST_STORE.with(|t| *t.borrow_mut() = Some(empty));
        let disc = |name: &str| {
            let p = ball(&dir, name);
            p
        };
        let (f, sd, t) = (disc("cup front.png"), disc("cup side.png"), disc("cup top.png"));
        let mut s = Session::new(services::project::new_custom("T", 640, 360, 30.0));
        let started = said(s.run("generate", &json!({"task": "model3d", "images": [f.clone(), sd, t], "size": 100, "cells": 40, "faces": 300, "texture": 64, "name": "Cup"})).unwrap());
        let job = started["job"].as_u64().unwrap();
        let done = said(s.run("generate", &json!({"job": job, "wait": true})).unwrap());
        assert_eq!(done["state"], "done", "{done}");
        assert!(done["model"].as_str().unwrap().contains("fallback"), "{done}");
        assert_eq!(s.doc.project.clips.len(), 1);
        // Asked for the image model only, a failure is reported as it is.
        let started = said(s.run("generate", &json!({"task": "model3d", "method": "image", "images": [f], "name": "Cup 2"})).unwrap());
        let job = started["job"].as_u64().unwrap();
        let done = said(s.run("generate", &json!({"job": job, "wait": true})).unwrap());
        assert_eq!(done["state"], "failed", "{done}");
        assert_eq!(s.doc.project.clips.len(), 1);
        TEST_STORE.with(|t| *t.borrow_mut() = None);
    }

    #[test]
    fn a_picture_with_no_object_and_a_bad_method_say_what_to_do() {
        let dir = temp("refuse");
        let mut s = Session::new(services::project::new_custom("T", 640, 360, 30.0));
        let blank = dir.join("blank.png");
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 16, 16);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&vec![255u8; 16 * 16 * 4]).unwrap();
        std::fs::write(&blank, out).unwrap();
        TEST_STORE.with(|t| *t.borrow_mut() = Some(ModelStore::new(dir.join("none"))));
        let err = s.generate(&json!({"task": "model3d", "images": [blank.display().to_string()]})).unwrap_err();
        assert!(err.contains("transparent: true"), "{err}");
        let err = s.generate(&json!({"task": "model3d", "method": "magic", "images": [blank.display().to_string()]})).unwrap_err();
        assert!(err.contains("carve"), "{err}");
        TEST_STORE.with(|t| *t.borrow_mut() = None);
    }
}
