// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `picture`: raster painting and photo editing for the AI editor.
//!
//! Pictures are the Paint workspace's (see [`crate::paint`]): the same
//! open pictures the window shows. A call runs a list of ops on the active
//! picture (all or nothing), saves it into the project's `Pictures`
//! folder, and with `look` returns how it looks now. Ops are Studio's
//! (select subject / sky / people / an object with the local models, paint
//! strokes from paths, remove, adjust, layers, frame-by-frame animation)
//! or any of the engine's commands (`commands` finds them, with their
//! parameters). These ops reach the rest of Studio:
//!
//! * `to_timeline` — the picture (or the animation's drawings, timed)
//!   onto the timeline; placing it again after more painting updates it;
//! * `from_video` — a new picture from a frame of footage (`media`, `at`)
//!   or of the edit (`program`, `at`);
//! * `trace` — rotoscoping: the footage's frames from `at` under an
//!   animation's drawings, one per drawing (`frames`, `step`);
//! * `generative_fill` — paint the selection from a prompt with
//!   Codex image generation, the result as a masked layer.

use super::{Content, Session, f};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_canvas::flipbook::Flipbook;
use studio_canvas::ops;

/// Ops that reach past the picture into the project.
const PROJECT_OPS: &[&str] = &["to_timeline", "from_video", "trace", "generative_fill"];

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

impl Session {
    fn picture_root(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()))
    }

    fn pictures_home(&self) -> PathBuf {
        crate::paint::pictures_dir(self.picture_root().as_deref())
    }

    pub(super) fn picture(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        if let Some(q) = a.get("commands") {
            let q = q.as_str().unwrap_or("");
            let all = studio_canvas::Canvas::commands(q);
            let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize;
            let n = all.len();
            return Ok(vec![Content::Text(json!({"commands": all.into_iter().take(limit).collect::<Vec<_>>(), "matching": n}).to_string())]);
        }
        let dir = self.pictures_home();
        let mut out = json!({});
        {
            let mut c = crate::paint::canvas();
            crate::paint::use_dir(&mut c, dir.clone());
            if a.get("list").and_then(Value::as_bool) == Some(true) {
                let saved: Vec<String> = crate::paint::saved(&dir).iter().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).collect();
                return Ok(vec![Content::Text(json!({"open": c.names(), "saved": saved, "active": c.doc().map(|d| d.name.clone()), "folder": dir.display().to_string()}).to_string())]);
            }
            let starts_new = a.get("ops").and_then(Value::as_array).and_then(|o| o.first()).is_some_and(|o| matches!(s(o, "op"), Some("new" | "open" | "from_video")));
            if !starts_new {
                crate::paint::bring(&mut c, &dir, s(a, "picture"))?;
            }
        }
        let ops: Vec<Value> = match a.get("ops") {
            Some(Value::Array(o)) => o.clone(),
            Some(o @ Value::Object(_)) => vec![o.clone()],
            None => Vec::new(),
            _ => return Err("`ops` is a list of ops".into()),
        };
        let mut results = Vec::new();
        // Runs of picture ops go to the canvas together (all or nothing);
        // the project ops between them run on their own.
        let mut i = 0;
        while i < ops.len() {
            let name = s(&ops[i], "op").unwrap_or("");
            if PROJECT_OPS.contains(&name) {
                results.push(self.picture_project_op(&ops[i])?);
                i += 1;
                continue;
            }
            let j = (i..ops.len()).find(|&k| s(&ops[k], "op").is_some_and(|n| PROJECT_OPS.contains(&n))).unwrap_or(ops.len());
            let r = {
                let mut c = crate::paint::canvas();
                crate::paint::with_models(|v| ops::apply(&mut c, &ops[i..j], v))?
            };
            if let Some(list) = r["results"].as_array() {
                results.extend(list.iter().cloned());
            }
            i = j;
        }
        let mut c = crate::paint::canvas();
        if !ops.is_empty() && c.doc().is_some() && !ops.iter().any(|o| s(o, "op") == Some("save")) {
            // Kept in the project after every call, so the next call (or the
            // window) carries on from here.
            if let Err(e) = c.save() {
                out["not_saved"] = json!(e);
            }
        }
        if !results.is_empty() {
            out["results"] = json!(results);
        }
        if let Ok(sm) = c.summary() {
            out["picture"] = sm;
        }
        let mut parts = Vec::new();
        if let Some(lk) = a.get("look").filter(|v| !matches!(v, Value::Bool(false) | Value::Null)) {
            let side = lk.as_u64().unwrap_or(1024).clamp(64, 4096) as u32;
            let img = match c.meta().and_then(|m| m.flipbook.clone()) {
                Some(fb) if a.get("onion").and_then(Value::as_bool) != Some(false) => fb.onion_view(c.doc().ok_or("no picture open")?, side),
                _ => c.render(side)?,
            };
            parts.push(Content::Jpeg(studio_canvas::jpeg(&img, 85)?));
        }
        parts.insert(0, Content::Text(out.to_string()));
        Ok(parts)
    }

    fn picture_project_op(&mut self, op: &Value) -> Result<Value, String> {
        match s(op, "op").unwrap_or("") {
            "to_timeline" => self.picture_to_timeline(op),
            "from_video" => self.picture_from_video(op),
            "trace" => self.picture_trace(op),
            "generative_fill" => self.picture_generative_fill(op),
            other => Err(format!("unknown op {other}")),
        }
    }

    /// The picture onto the timeline: a still for `duration` seconds, or an
    /// animation's drawings in a row, each for its hold, merged into one
    /// clip named after the picture. Each placing writes a versioned copy
    /// (`Pictures/Placed/<name>-<revision>…`); a picture placed before is
    /// updated in place: its media items point at the new copy (one undo
    /// step) and every clip showing it shows the new pixels.
    fn picture_to_timeline(&mut self, op: &Value) -> Result<Value, String> {
        let (saved, fb, name, size, rev) = {
            let mut c = crate::paint::canvas();
            let saved = c.save()?;
            let fb = c.meta().and_then(|m| m.flipbook.clone());
            let st = c.engine.active().ok_or("no picture open")?;
            (saved, fb, studio_canvas::stem(&st.doc.name).to_owned(), (st.doc.size.width, st.doc.size.height), st.revision)
        };
        let placed = match self.picture_root() {
            Some(r) => studio_model::file::ProjectFolder::new(r).generated_dir(studio_model::file::MadeKind::Placed),
            None => self.pictures_home().join("Placed"),
        };
        std::fs::create_dir_all(&placed).map_err(|e| e.to_string())?;
        let tag = format!("{name}-{rev}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() % 100_000));
        let sources: Vec<PathBuf> = match (&fb, saved["frames"].as_array()) {
            (Some(_), Some(fr)) => fr.iter().filter_map(Value::as_str).map(PathBuf::from).collect(),
            _ => vec![PathBuf::from(saved["png"].as_str().ok_or("the picture was not saved")?)],
        };
        let files: Vec<PathBuf> = if sources.len() == 1 && fb.is_none() {
            vec![placed.join(format!("{tag}.png"))]
        } else {
            let d = placed.join(&tag);
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            (1..=sources.len()).map(|i| d.join(format!("{i:04}.png"))).collect()
        };
        for (src, dst) in sources.iter().zip(&files) {
            std::fs::copy(src, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
        // Placed before: the media items of this picture, in frame order.
        let prefix = placed.join(format!("{name}-")).display().to_string();
        let mut before: Vec<(String, u64)> = self
            .doc
            .model
            .media
            .iter()
            .filter(|m| m.path.starts_with(&prefix) && m.path[prefix.len()..].split(['-', '/', '.']).next().is_some_and(|r| r.chars().all(|c| c.is_ascii_digit())))
            .map(|m| (m.path.clone(), m.id.0))
            .collect();
        before.sort();
        // The latest placing's items only (by their folder or file tag).
        if let Some(last_tag) = before.last().map(|(p, _)| p[prefix.len()..].split(['/', '.']).next().unwrap_or("").to_owned()) {
            before.retain(|(p, _)| p[prefix.len()..].starts_with(&last_tag));
        }
        if !before.is_empty() && before.len() == files.len() && op.get("again").and_then(Value::as_bool) != Some(true) {
            self.doc.begin_batch(&format!("Update {name}"));
            for ((_, id), file) in before.iter().zip(&files) {
                if let Some(item) = self.doc.model.media.iter().find(|m| m.id.0 == *id).cloned() {
                    let mut item = item;
                    item.path = file.display().to_string();
                    self.doc.apply("Update picture", studio_model::Edit::SetMedia { item: Box::new(item) });
                }
            }
            self.doc.commit_batch();
            return Ok(json!({"updated": name, "media": before.iter().map(|b| b.1).collect::<Vec<_>>(), "revision": self.doc.history.revision(), "note": "every clip showing this picture shows the new version; pass again: true to place another copy"}));
        }
        let report = crate::services::project::import_media(&mut self.doc, &files);
        if report.ids.len() != files.len() {
            return Err(format!("the picture could not be added to the media bin: {}", report.failed.join("; ")));
        }
        let at = f(op, "at").unwrap_or(0.0);
        let lane = s(op, "lane").map(str::to_owned);
        let mut ops = Vec::new();
        let place = |media: u32, start: f64, dur: f64, ops: &mut Vec<Value>| {
            let mut p = json!({"op": "place", "media": media, "at": start, "duration": dur, "name": name});
            if let Some(l) = &lane {
                p["lane"] = json!(l);
            }
            ops.push(p);
        };
        match &fb {
            Some(fb) => {
                let times = fb.timing();
                let loops = f(op, "loops").unwrap_or(1.0).max(1.0) as usize;
                let total: f64 = times.iter().map(|t| t.1).sum();
                for k in 0..loops {
                    for ((t0, dur), media) in times.iter().zip(&report.ids) {
                        place(*media, at + k as f64 * total + t0, *dur, &mut ops);
                    }
                }
                let n = ops.len();
                if n > 1 {
                    let refs: Vec<String> = (1..=n).map(|i| format!("${i}")).collect();
                    ops.push(json!({"op": "merge", "clips": refs, "name": name}));
                }
            }
            None => place(report.ids[0], at, f(op, "duration").unwrap_or(5.0).max(0.1), &mut ops),
        }
        let mut v = self.edit(&ops, &format!("Place {name}"), false)?;
        v["picture"] = json!({"name": name, "size": [size.0, size.1], "files": files.len()});
        Ok(v)
    }

    /// A video frame as RGBA at its own size (or `width`).
    fn video_frame(&mut self, op: &Value) -> Result<(u32, u32, Vec<u8>, String), String> {
        let t = f(op, "at").unwrap_or(0.0);
        if op.get("program").and_then(Value::as_bool) == Some(true) || op.get("media").is_none() {
            #[cfg(feature = "render")]
            {
                let w = f(op, "width").map(|w| w as u32).unwrap_or(self.doc.project.width.max(16));
                let img = self.frames.rgba(&self.doc, super::micros(t), w)?;
                return Ok((img.width, img.height, img.pixels, format!("Frame {t:.2}s")));
            }
            #[cfg(not(feature = "render"))]
            return Err("this build cannot render the edit; give `media`".into());
        }
        let mi = self.media_item(op.get("media").ok_or("give `media` (or `program: true`)")?)?;
        let path = mi.path.clone().ok_or("that media has no file")?;
        let (w, h) = match f(op, "width") {
            Some(w) => (w as usize, ((w * mi.height.max(1) as f64 / mi.width.max(1) as f64).round() as usize).max(8)),
            None => (mi.width.max(16) as usize, mi.height.max(16) as usize),
        };
        let img = crate::media_io::frame(&path, t.max(0.0), w, h, false)?;
        let mut px = Vec::with_capacity(w * h * 4);
        for c in &img.pixels {
            px.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
        }
        let stem = Path::new(&mi.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Frame".into());
        Ok((img.size[0] as u32, img.size[1] as u32, px, format!("{stem} {t:.2}s")))
    }

    fn picture_from_video(&mut self, op: &Value) -> Result<Value, String> {
        let (w, h, px, name) = self.video_frame(op)?;
        let mut c = crate::paint::canvas();
        let r = c.open_rgba(s(op, "name").unwrap_or(&name), w, h, &px)?;
        Ok(r)
    }

    /// Rotoscoping: the footage's frames under the drawings.
    fn picture_trace(&mut self, op: &Value) -> Result<Value, String> {
        let mi = self.media_item(op.get("media").ok_or("trace needs `media` (the footage to draw over)")?)?;
        let at = f(op, "at").unwrap_or(0.0).max(0.0);
        let fps = f(op, "fps").unwrap_or(12.0).clamp(1.0, 60.0);
        let frames = f(op, "frames").unwrap_or(12.0).clamp(1.0, 600.0) as usize;
        let width = f(op, "width").map(|w| w as usize);
        let path = mi.path.clone().ok_or("that media has no file")?;
        let (w, h) = match width {
            Some(w) => (w, ((w as f64 * mi.height.max(1) as f64 / mi.width.max(1) as f64).round() as usize).max(8)),
            None => (mi.width.clamp(16, 3840) as usize, mi.height.clamp(16, 2160) as usize),
        };
        let mut shots = Vec::with_capacity(frames);
        for k in 0..frames {
            let t = at + k as f64 / fps;
            let img = crate::media_io::frame(&path, t, w, h, false)?;
            let mut px = Vec::with_capacity(w * h * 4);
            for c in &img.pixels {
                px.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
            }
            shots.push((img.size[0] as u32, img.size[1] as u32, px));
        }
        let mut c = crate::paint::canvas();
        let stem = Path::new(&mi.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Roto".into());
        if c.doc().is_none() || op.get("new").and_then(Value::as_bool) != Some(false) {
            let name = s(op, "name").map(str::to_owned).unwrap_or_else(|| format!("{stem} roto"));
            c.new_picture(&name, json!({"width": shots[0].0, "height": shots[0].1, "background": "transparent"}))?;
        }
        Flipbook::start(&mut c, frames, fps, 1)?;
        let r = Flipbook::set_reference(&mut c, shots, Some(json!({"media": mi.name, "at": at, "fps": fps})))?;
        Ok(json!({"trace": r, "fps": fps, "note": "paint each drawing over its video frame (frame {go: n}); the video is never part of the animation; to_timeline lays the drawings over the footage at `at`"}))
    }

    /// The selection painted from a prompt by Codex's image generation (the
    /// person's own ChatGPT plan, no API key); the result comes back as a
    /// new layer masked to the selection.
    fn picture_generative_fill(&mut self, op: &Value) -> Result<Value, String> {
        let prompt = s(op, "prompt").ok_or("generative_fill needs `prompt`: what to paint in the selection")?;
        if op.get("estimate").and_then(Value::as_bool) == Some(true) {
            return Ok(json!({"estimate": "would make one picture", "model": "Codex image generation"}));
        }
        if super::brain::find_codex().is_none() {
            return Err("generative fill uses the Codex app's image generation, and the codex command is not on this Mac (install Codex); without it, `remove` fills the selection from its surroundings on this Mac".into());
        }
        let (picture, mask, size) = {
            let c = crate::paint::canvas();
            let d = c.doc().ok_or("no picture open")?;
            let sel = d.selection.as_ref().ok_or("select the area to fill first (select with what / point / box, or a rect / lasso)")?;
            let (w, h) = (d.size.width, d.size.height);
            let vals = sel.read_region(studio_pixel_geom_rect(w, h));
            let ch = sel.channels().max(1);
            let mut m = studio_canvas::Rgba8Image::new(w, h);
            for (i, px) in m.pixels.chunks_exact_mut(4).enumerate() {
                let v = (vals.get(i * ch).copied().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                px.copy_from_slice(&[v, v, v, 255]);
            }
            (studio_canvas::png(&c.render(0)?)?, studio_canvas::png(&m)?, (w, h))
        };
        let work = std::env::temp_dir().join(format!("studio-genfill-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let (picture_file, mask_file) = (work.join("picture.png"), work.join("mask.png"));
        std::fs::write(&picture_file, &picture).map_err(|e| e.to_string())?;
        std::fs::write(&mask_file, &mask).map_err(|e| e.to_string())?;
        let task = format!(
            "Use your image generation tool to edit the first attached picture. The second attached picture is a mask of the same size: repaint only where the mask is white, with: {prompt}. Match the light, grain and perspective around it. Leave everything where the mask is black unchanged. The result is {}×{} like the original.",
            size.0, size.1
        );
        let made = super::generate::codex_run(&task, 1, &[picture_file, mask_file], &work.join("out"), "fill");
        let bytes = made.and_then(|m| std::fs::read(m.first().ok_or("Codex made no picture")?).map_err(|e| e.to_string()));
        let _ = std::fs::remove_dir_all(&work);
        let bytes = bytes?;
        let o = studio_canvas::files::open_bytes("fill.png", &bytes).or_else(|_| studio_canvas::files::open_bytes("fill.jpg", &bytes))?;
        let img = studio_pixel_compose_thumbnail(&o.document, size)?;
        let mut c = crate::paint::canvas();
        let r = ops::apply(&mut c, &[json!({"op": "layer", "action": "new", "name": format!("Fill: {prompt}")})], &mut studio_canvas::vision::NoVision)?;
        c.put_rgba(size.0, size.1, &img)?;
        let m = c.run("layer.layerMask.revealSelection", json!({}))?;
        Ok(json!({"layer": r["results"][0], "mask": m, "by": "Codex image generation"}))
    }
}

fn studio_pixel_geom_rect(w: u32, h: u32) -> studio_canvas::Rect {
    studio_canvas::Rect::from_xywh(0, 0, w, h)
}

/// A picture file's pixels at `size` (the model may return another size).
fn studio_pixel_compose_thumbnail(doc: &studio_canvas::Document, size: (u32, u32)) -> Result<Vec<u8>, String> {
    let img = {
        let mut c = studio_canvas::Canvas::new(None);
        c.engine.add_document(doc.clone(), None);
        c.render(0)?
    };
    Ok(if (img.width, img.height) == size { img.pixels } else { studio_canvas::flipbook::scale_rgba(&img.pixels, img.width, img.height, size.0, size.1) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(parts: &[Content]) -> Value {
        let Content::Text(t) = &parts[0] else { panic!("text first") };
        serde_json::from_str(t).unwrap()
    }

    #[test]
    fn the_ai_paints_a_picture_looks_at_it_and_puts_it_on_the_timeline() {
        let _turn = crate::paint::test_lock();
        let dir = std::env::temp_dir().join(format!("studio-picture-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::at(&dir).unwrap();
        let parts = s
            .call(
                "picture",
                &json!({"ops": [
                    {"op": "new", "name": "Sunset", "width": 320, "height": 180, "background": "#203060"},
                    {"op": "layer", "action": "new", "name": "Sun"},
                    {"op": "paint", "shape": {"kind": "circle", "center": [160, 110], "radius": 30}, "size": 20, "color": "#ffaa00", "feel": "airbrush"},
                    {"op": "adjust", "kind": "vibrance", "vibrance": 30}
                ], "look": 256}),
            )
            .unwrap();
        let v = text(&parts);
        assert_eq!(v["picture"]["picture"], "Sunset", "{v}");
        assert!(matches!(parts.get(1), Some(Content::Jpeg(j)) if j.len() > 500), "a look comes back");
        assert!(dir.join("Pictures/Sunset.pcraft").exists() && dir.join("Pictures/Sunset.png").exists(), "kept in the project");
        let v = text(&s.call("picture", &json!({"ops": [{"op": "to_timeline", "at": 1.0, "duration": 3.0}]})).unwrap());
        assert!(v["results"][0]["changed"].is_array() || v["results"][0]["added"].is_array() || !v["results"][0].is_null(), "{v}");
        assert_eq!(s.doc.project.clips.len(), 1, "one clip on the timeline");
        // Painting more and placing again updates the same media.
        let v = text(&s.call("picture", &json!({"ops": [{"op": "paint", "points": [[10, 10], [300, 170]], "size": 6, "color": "#ffffff"}, {"op": "to_timeline"}]})).unwrap());
        assert!(v["results"].as_array().unwrap().iter().any(|r| r.get("updated").is_some()), "{v}");
        assert_eq!(s.doc.project.clips.len(), 1);
        let listed = text(&s.call("picture", &json!({"list": true})).unwrap());
        assert!(listed["saved"].as_array().unwrap().iter().any(|n| n == "Sunset"));
        let found = text(&s.call("picture", &json!({"commands": "liquify"})).unwrap());
        assert!(found["commands"][0]["id"].as_str().unwrap().contains("liquify"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_animation_goes_on_the_timeline_as_its_drawings_in_time() {
        let _turn = crate::paint::test_lock();
        let dir = std::env::temp_dir().join(format!("studio-picture-anim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::at(&dir).unwrap();
        s.call(
            "picture",
            &json!({"ops": [
                {"op": "new", "name": "Ball", "width": 128, "height": 72, "background": "transparent"},
                {"op": "animate", "drawings": 1, "fps": 12, "hold": 2},
                {"op": "paint", "shape": {"kind": "circle", "center": [20, 36], "radius": 8}, "size": 6, "color": "#ff0000"},
                {"op": "frame", "action": "add"},
                {"op": "paint", "shape": {"kind": "circle", "center": [60, 36], "radius": 8}, "size": 6, "color": "#ff0000"},
                {"op": "frame", "action": "add"},
                {"op": "paint", "shape": {"kind": "circle", "center": [100, 36], "radius": 8}, "size": 6, "color": "#ff0000"}
            ]}),
        )
        .unwrap();
        let v = text(&s.call("picture", &json!({"ops": [{"op": "to_timeline", "at": 0.0}], "look": 128})).unwrap());
        assert_eq!(v["picture"]["flipbook"]["drawings"], 3, "{v}");
        assert!(dir.join("Pictures/Ball.frames/0003.png").exists());
        // Three drawings on twos at 12 fps: half a second, merged into one clip.
        let clips = &s.doc.project.clips;
        assert_eq!(clips.len(), 1, "merged into one clip");
        assert!((clips[0].dur as f64 / 1e6 - 0.5).abs() < 0.02, "{}", clips[0].dur);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
