// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `vector` tool: vector illustration documents (`.svart`, kept in the
//! project's Vector folder) drawn, edited, styled, traced, laid out,
//! animated and exported with `studio-vector-art`'s operations, looked at
//! as pictures, and placed on the timeline as clips that draw the art at
//! the output's own resolution.

use std::path::{Path, PathBuf};

use super::{Content, Session, f, micros};
use serde_json::{Value, json};
use studio_model as m;
use studio_vector_art::Document;
use studio_vector_art::ops::{self, Context};

impl Session {
    /// The open project's folder: the one the document was opened from or
    /// saved to, else the folder the session was started in. (Only the
    /// session folder was used, so after opening another project vector
    /// documents still went into the first one.)
    fn vector_root(&self) -> Option<PathBuf> {
        self.doc.folder.as_ref().map(|f| f.root().to_path_buf()).or_else(|| self.home.clone())
    }

    /// The folder vector documents live in: the project's `Vector`, or a
    /// scratch folder with no project.
    fn vector_dir(&self) -> PathBuf {
        match self.vector_root() {
            Some(h) => h.join("Vector"),
            None => std::env::temp_dir().join("studio-vector"),
        }
    }

    fn vector_path(&self, a: &Value) -> PathBuf {
        let name = a.get("doc").and_then(Value::as_str).unwrap_or("art.svart");
        let p = Path::new(name);
        let p = if p.is_absolute() { p.to_path_buf() } else if name.contains('/') { self.vector_root().unwrap_or_else(|| self.vector_dir()).join(p) } else { self.vector_dir().join(p) };
        if p.extension().is_none() { p.with_extension("svart") } else { p }
    }

    pub(super) fn vector_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let path = self.vector_path(a);
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.vector_dir());
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // Other formats open as a new document saved beside them.
        let (mut doc, save_to) = if path.extension().is_some_and(|e| e != "svart") {
            (studio_vector_art::interop::open(&path)?, path.with_extension("svart"))
        } else if path.exists() && a.get("new").is_none() {
            (studio_vector_art::interop::open(&path)?, path.clone())
        } else {
            let n = a.get("new").cloned().unwrap_or(json!({}));
            let w = f(&n, "width").unwrap_or(self.doc.project.width as f64);
            let h = f(&n, "height").unwrap_or(self.doc.project.height as f64);
            let mut d = Document::new(w, h);
            d.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Art").to_string();
            if let Some(bg) = n.get("background") {
                d.artboards[0].background = if bg.is_null() { None } else { serde_json::from_value(bg.clone()).ok() };
            }
            (d, path.clone())
        };
        let ctx = Context { base_dir: Some(self.vector_root().unwrap_or_else(|| dir.clone())), out_dir: Some(dir.join("renders")) };
        let _ = std::fs::create_dir_all(dir.join("renders"));
        let results = match a.get("ops") {
            Some(o) => ops::apply_all(&mut doc, o, &ctx)?,
            None => Value::Null,
        };
        std::fs::write(&save_to, doc.to_json()).map_err(|e| format!("{}: {e}", save_to.display()))?;
        let mut reply = json!({"doc": save_to.display().to_string(), "results": results});
        if a.get("summary").and_then(Value::as_bool).unwrap_or(a.get("ops").is_none()) {
            reply["summary"] = ops::summary(&doc);
        }
        let notes = ops::check(&doc);
        if !notes.is_empty() {
            reply["check"] = json!(notes);
        }
        if let Some(p) = a.get("place") {
            reply["clip"] = json!(self.place_vector(&save_to, &doc, p)?);
        }
        let mut out = Vec::new();
        if a.get("look").and_then(Value::as_bool).unwrap_or(false) {
            let ab = a.get("artboard").and_then(Value::as_str);
            let board = doc.artboard(ab).ok_or("the document has no artboard")?;
            let width = f(a, "width").unwrap_or(1200.0).clamp(64.0, 4096.0);
            let k = width / board.rect.width().max(1.0);
            let (w, h) = (width.round() as u32, (board.rect.height() * k).round().max(1.0) as u32);
            let pm = studio_vector_art::build::render_artboard(&doc, ab, w, h, &studio_vector_art::build::BuildOptions { base_dir: ctx.base_dir.clone(), show_hidden: false });
            // On a checkerboard where it is see-through, as editors show it.
            let mut px = pm.to_straight();
            for (i, p) in px.chunks_exact_mut(4).enumerate() {
                let (x, y) = ((i as u32 % w) / 8, (i as u32 / w) / 8);
                let bg = if (x + y) % 2 == 0 { 255.0 } else { 225.0 };
                let a = p[3] as f32 / 255.0;
                for c in &mut p[..3] {
                    *c = (*c as f32 * a + bg * (1.0 - a)).round() as u8;
                }
                p[3] = 255;
            }
            let img = studio_media::rgba::RgbaImage { width: w, height: h, pixels: px };
            out.push(Content::Jpeg(studio_content::jpeg::encode(&img, 88)));
        }
        out.insert(0, Content::Text(reply.to_string()));
        Ok(out)
    }

    /// Puts the document on the timeline as a clip drawing its artboard.
    fn place_vector(&mut self, path: &Path, doc: &Document, p: &Value) -> Result<u32, String> {
        let at = micros(f(p, "at").unwrap_or(0.0));
        let anim = doc.animation.length();
        let dur = micros(f(p, "duration").unwrap_or(if anim > 0.0 { anim } else { 5.0 })).max(micros(1.0 / 30.0));
        let want = match p.get("lane") {
            Some(v) => Some(super::craft::lane_of(&self.doc.project, v, None)?),
            None => None,
        };
        let lane = self.room(want, at, dur, None)?;
        let id = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(m::GeneratorKind::Shape);
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        g.params.insert("art".into(), m::Value::Text(abs.display().to_string()));
        if let Some(fit) = p.get("fit").and_then(Value::as_str) {
            g.params.insert("art_fit".into(), m::Value::Text(fit.into()));
        }
        if p.get("background").and_then(Value::as_bool).unwrap_or(false) {
            g.params.insert("art_background".into(), m::Value::Scalar(1.0));
        }
        if let Some(ab) = p.get("artboard").and_then(Value::as_str) {
            g.params.insert("artboard".into(), m::Value::Text(ab.into()));
        }
        // Parameters the art is bound to start at the values given.
        for (k, v) in p.get("params").and_then(Value::as_object).into_iter().flatten() {
            let val = match v {
                Value::Number(n) => m::Value::Scalar(n.as_f64().unwrap_or(0.0)),
                Value::Bool(b) => m::Value::Bool(*b),
                Value::String(s) => m::Value::Text(s.clone()),
                _ => continue,
            };
            g.params.insert(k.clone(), val);
        }
        let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), at, dur, m::ClipSource::Generator(g));
        c.name = p.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| if doc.name.is_empty() { "Vector art".into() } else { doc.name.clone() });
        c.transform.position = m::Vec2::new(f(p, "x").unwrap_or(0.0), f(p, "y").unwrap_or(0.0));
        if let Some(o) = f(p, "opacity") {
            c.transform.opacity = (o / 100.0).clamp(0.0, 1.0);
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Vector art", m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(id)
    }
}
