// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI's side of the generative media department
//! (`services::genmedia`): `generate` with a `task` (animate, between,
//! restyle, relight, time_of_day, season, outpaint, inpaint, upscale,
//! enhance), the `enhance` tool, looks that keep a character or place the
//! same, and making a generation again or in variations from its recipe.
//!
//! Pictures come from files, media, a clip (a still clip's picture, or a
//! video clip's frames) or the program frame at a moment. A result made
//! from a clip goes back into that clip (its original kept in
//! `meta["generated"]`, `restore` puts it back); otherwise it lands in the
//! media bin and, when asked, on the timeline.

use super::{Content, Session, f, generate as gm};
use crate::services::genmedia::{self as g, ClipIn, Provider, Request, Task};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_ai::JobCtx;
use studio_model as m;

/// Keys of `generate` that are not task settings.
const NOT_SETTINGS: &[&str] = &["task", "provider", "prompt", "image", "images", "end_image", "clip", "style_ref", "refs", "reference", "look", "seed", "name", "estimate", "long", "to_timeline", "at", "replace", "count", "vary"];

impl Session {
    fn project_dir(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()))
    }

    fn gen_dir(&self) -> PathBuf {
        gm::generated_dir(self.project_dir().as_deref())
    }

    /// A picture as a file: a path, an https link (fetched), media, or the
    /// program frame at `{"frame": seconds}` (rendered).
    fn picture_file(&mut self, v: &Value) -> Result<PathBuf, String> {
        if let Some(t) = v.get("frame").and_then(Value::as_f64) {
            let w = self.doc.project.width.clamp(256, 3840);
            let jpeg = self.frames.frame(&self.doc, super::micros(t.max(0.0)), w, 95)?;
            let p = std::env::temp_dir().join(format!("studio-frame-{}-{}.jpg", std::process::id(), (t * 1000.0).round() as i64));
            std::fs::write(&p, jpeg).map_err(|e| e.to_string())?;
            return Ok(p);
        }
        match v {
            Value::String(s) if s.starts_with("https://") => {
                let dir = std::env::temp_dir().join(format!("studio-refs-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&dir);
                let name = s.rsplit('/').next().and_then(|n| n.split(['?', '#']).next()).filter(|n| n.contains('.')).unwrap_or("ref.png");
                let p = dir.join(name);
                let reply = studio_net::get(s, &[], std::time::Duration::from_secs(60))?;
                std::fs::write(&p, reply.bytes(64 << 20).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                Ok(p)
            }
            Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => {
                let p = PathBuf::from(s);
                if p.is_file() { Ok(p) } else { Err(format!("{s}: no such file")) }
            }
            _ => {
                let mi = self.media_item(v).map_err(|e| format!("a picture is a file path, an https link, a media id or {{\"frame\": seconds}} ({e})"))?;
                mi.path.ok_or_else(|| "that media has no file on disk".into())
            }
        }
    }

    fn pictures(&mut self, v: Option<&Value>) -> Result<Vec<PathBuf>, String> {
        match v {
            None => Ok(Vec::new()),
            Some(Value::Array(list)) => list.iter().map(|x| self.picture_file(x)).collect(),
            Some(x) => Ok(vec![self.picture_file(x)?]),
        }
    }

    /// The clip's input: a video clip's frames, or a still clip's picture.
    /// Anything else (a title, a card, a shape, a nested clip) is rendered
    /// over its span to footage first (the third value says so).
    fn clip_input(&mut self, id: u32, task: Task) -> Result<(Option<ClipIn>, Option<PathBuf>, bool), String> {
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let m::ClipSource::Media(r) = &mc.source else {
            let (c, p) = self.render_clip(id, task)?;
            return Ok((c, p, true));
        };
        let mi = self.doc.project.media(r.media.0 as u32).ok_or("the clip's media is gone")?;
        match mi.kind {
            crate::view::MediaKind::Image => Ok((None, Some(mi.path.clone().ok_or("that picture has no file on disk")?), false)),
            crate::view::MediaKind::Video => {
                let footage = crate::services::tracking::footage(&self.doc.project, &mc)?;
                let used = crate::services::tracking::used_frames(&mc, &footage);
                // One more frame: the clip reads to the end of its last.
                let range = used.start..(used.end + 1).min(footage.frames.max(used.end));
                Ok((Some(ClipIn { footage, range }), None, false))
            }
            _ => Err("a sound has no picture".into()),
        }
    }

    /// A clip that is not footage or a still, as footage: the clip and what
    /// lies under it (lanes above left out, so its transparency shows what
    /// it sits on), every frame of its span with its animation, at the
    /// canvas size. A picture task gets the frame in the middle.
    fn render_clip(&mut self, id: u32, task: Task) -> Result<(Option<ClipIn>, Option<PathBuf>), String> {
        let vc = self.doc.project.clips.iter().find(|c| c.id == id).cloned().ok_or_else(|| format!("no clip {id}"))?;
        if !vc.is_visual() {
            return Err("a sound has no picture".into());
        }
        let lanes: Vec<u32> = self.doc.project.lanes.iter().map(|l| l.id).collect();
        let pos = |lane: u32| lanes.iter().position(|&l| l == lane).unwrap_or(usize::MAX);
        let mine = pos(vc.lane);
        let above: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.id != id && c.is_visual() && pos(c.lane) < mine && c.start < vc.end() && c.end() > vc.start).map(|c| c.id).collect();
        let dir = self.gen_dir().join("Sources");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let width = self.doc.project.width.clamp(16, 1920);
        let scene = self.frames.scene_without(&self.doc, &above);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let to_frame = |img: studio_media::rgba::RgbaImage| studio_ai::Frame::new(img.width as usize, img.height as usize, img.pixels).map_err(|e| e.to_string());
        if matches!(task, Task::Outpaint | Task::Inpaint | Task::Animate | Task::Between) {
            let img = self.frames.rgba_in(&self.doc, &scene, vc.start + vc.dur / 2, width)?;
            let p = dir.join(format!("clip {id} {stamp}.png"));
            g::save_png(&to_frame(img)?, &p)?;
            return Ok((None, Some(p)));
        }
        let fps = (self.doc.project.fps as f64).max(1.0);
        let n = ((vc.dur as f64 / crate::view::SEC as f64) * fps).round().max(1.0) as usize;
        if n > 60 * 60 {
            return Err(format!("clip {id} is over a minute long: split it, then work on the part that needs it"));
        }
        let file = dir.join(format!("clip {id} {stamp}.mp4"));
        let mut out: Option<g::VideoOut> = None;
        for i in 0..n {
            let t = vc.start + (i as f64 / fps * crate::view::SEC as f64).round() as crate::view::Time;
            let f = to_frame(self.frames.rgba_in(&self.doc, &scene, t, width)?)?;
            let o = match &mut out {
                Some(o) => o,
                None => out.insert(g::VideoOut::create(&file, f.width, f.height, fps)?),
            };
            o.put(&f)?;
        }
        let file = out.ok_or("the clip has no frames")?.finish()?;
        let footage = g::footage_of(&file)?;
        let range = 0..footage.frames;
        Ok((Some(ClipIn { footage, range }), None))
    }

    /// The project as JSON (for the story bible's cast and places).
    fn project_json(&self) -> Value {
        serde_json::to_value(&self.doc.model).unwrap_or(Value::Null)
    }

    /// `generate` with a task, looks, `again`; None for a plain generation
    /// (`generate` then runs it and this writes its recipe).
    pub(super) fn generative(&mut self, a: &Value) -> Result<Option<Value>, String> {
        if a.get("_inner").is_some() {
            return Ok(None);
        }
        if a.get("looks").is_some() || a.get("save_look").is_some() || a.get("forget_look").is_some() {
            return self.looks_op(a).map(Some);
        }
        if a.get("again").is_some() {
            return self.again(a).map(Some);
        }
        if a.get("restore").is_some() {
            let id = a["restore"].as_u64().ok_or("restore is a clip id")? as u32;
            return self.restore_generated(id).map(|s| Some(json!({"restored": id, "note": s})));
        }
        match a.get("task").and_then(Value::as_str) {
            Some("make") | None => {
                if a.get("list").is_some() {
                    let mut v = self.generate(&with(a, "_inner", json!(true)))?;
                    v["tasks"] = tasks_list();
                    v["providers_here"] = json!({"codex": g::Available::now().codex, "local": true});
                    return Ok(Some(v));
                }
                // A look adds its words, pictures and seed.
                let mut args = a.clone();
                if let Some(name) = a.get("look").and_then(Value::as_str) {
                    let l = g::find_look(&self.gen_dir(), &self.project_json(), name).ok_or_else(|| self.no_look(name))?;
                    let prompt = a.get("prompt").and_then(Value::as_str).unwrap_or("");
                    args["prompt"] = json!(if l.words.is_empty() { prompt.to_owned() } else { format!("{prompt}. {}: {}", l.name, l.words) });
                    let mut images: Vec<Value> = match a.get("images").or_else(|| a.get("image")) {
                        Some(Value::Array(v)) => v.clone(),
                        Some(v) => vec![v.clone()],
                        None => Vec::new(),
                    };
                    // Pictures of a still follow the look's pictures; a clip
                    // starts from the first.
                    let video = a.get("make").and_then(Value::as_str).and_then(gm::Kind::parse) == Some(gm::Kind::Video);
                    if !(video && !images.is_empty()) {
                        images.extend(l.refs.iter().filter(|r| Path::new(r).is_file()).take(if video { 1 } else { 4 }).map(|r| json!(r)));
                    }
                    if let Some(o) = args.as_object_mut() {
                        o.remove("image");
                        o.remove("look");
                    }
                    if !images.is_empty() {
                        args["images"] = json!(images);
                    }
                    if a.get("seed").is_none() {
                        args["seed"] = json!(l.seed.unwrap_or_else(|| g::seed_for(&l.name)));
                    }
                }
                let mut v = self.generate(&with(&args, "_inner", json!(true)))?;
                let by = v["generated"]["by"].as_str().or(v["generated"]["model"].as_str()).unwrap_or("").to_owned();
                for f in v["files"].as_array().cloned().unwrap_or_default() {
                    if let Some(p) = f.as_str() {
                        g::write_make_recipe(Path::new(p), &args, &by, None);
                    }
                }
                if v["files"].as_array().is_some_and(|f| !f.is_empty()) {
                    v["recipe"] = json!("kept beside each file: generate {\"again\": <media id>} makes it again, with count for variations");
                }
                Ok(Some(v))
            }
            Some(t) => {
                let task = Task::parse(t).ok_or_else(|| format!("task \"{t}\": make, {}", Task::ALL.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ")))?;
                self.gen_task(task, a).map(Some)
            }
        }
    }

    fn no_look(&self, name: &str) -> String {
        let known: Vec<String> = g::looks(&self.gen_dir()).into_iter().chain(g::story_looks(&self.project_json())).map(|l| l.name).collect();
        format!("no look \"{name}\"{}; save one with generate {{\"save_look\": \"{name}\", \"refs\": [pictures], \"words\": \"what they look like\"}}", if known.is_empty() { String::new() } else { format!(" (looks: {})", known.join(", ")) })
    }

    fn looks_op(&mut self, a: &Value) -> Result<Value, String> {
        let dir = self.gen_dir();
        if let Some(name) = a.get("forget_look").and_then(Value::as_str) {
            return Ok(json!({"forgotten": g::forget_look(&dir, name)}));
        }
        if let Some(name) = a.get("save_look").and_then(Value::as_str) {
            let name = name.trim();
            if name.is_empty() {
                return Err("save_look is the look's name".into());
            }
            let mut refs: Vec<String> = Vec::new();
            for p in self.pictures(a.get("refs").or_else(|| a.get("images")).or_else(|| a.get("image")))? {
                // Kept with the project, so the look outlives temp files.
                let keep = dir.join("Looks");
                std::fs::create_dir_all(&keep).map_err(|e| e.to_string())?;
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
                let dest = gm::free_path(&keep, &format!("{name} {}", refs.len() + 1), &ext);
                if !p.starts_with(&keep) {
                    std::fs::copy(&p, &dest).map_err(|e| format!("{}: {e}", p.display()))?;
                    refs.push(dest.display().to_string());
                } else {
                    refs.push(p.display().to_string());
                }
            }
            let old = g::looks(&dir).into_iter().find(|l| l.name.eq_ignore_ascii_case(name));
            let l = g::Look {
                name: name.to_owned(),
                words: a.get("words").and_then(Value::as_str).map(str::to_owned).or_else(|| old.as_ref().map(|o| o.words.clone())).unwrap_or_default(),
                refs: if refs.is_empty() { old.as_ref().map(|o| o.refs.clone()).unwrap_or_default() } else { refs },
                seed: a.get("seed").and_then(Value::as_u64).or_else(|| old.as_ref().and_then(|o| o.seed)).or(Some(g::seed_for(name))),
                from: "looks",
            };
            g::save_look(&dir, &l)?;
            return Ok(json!({"saved": l.to_json(), "use": format!("generate {{\"look\": \"{}\", ...}}: its pictures, words and seed go with every generation", l.name)}));
        }
        let all: Vec<Value> = g::looks(&dir).into_iter().chain(g::story_looks(&self.project_json())).map(|l| l.to_json()).collect();
        Ok(json!({"looks": all}))
    }

    /// A task, from the tool's arguments.
    fn gen_task(&mut self, task: Task, a: &Value) -> Result<Value, String> {
        let req = self.request_for(task, a)?;
        let out = g::run(&req, &JobCtx::detached(), &mut |_| {})?;
        self.land_task(&req, &out, a)
    }

    /// The request for a task (inputs resolved to files and frames).
    pub(crate) fn request_for(&mut self, task: Task, a: &Value) -> Result<Request, String> {
        let mut req = Request::new(task, self.gen_dir());
        req.provider = match a.get("provider").and_then(Value::as_str) {
            None | Some("auto") => None,
            Some(p) => Some(Provider::parse(p).ok_or_else(|| format!("provider is codex, local or auto, not \"{p}\""))?),
        };
        req.prompt = a.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
        let clip = a.get("clip").and_then(Value::as_u64).map(|c| c as u32);
        let mut rendered = false;
        if let Some(id) = clip {
            let (c, pic, r) = self.clip_input(id, task)?;
            rendered = r;
            req.clip = c;
            req.image = pic;
            if task.makes_video() && req.clip.is_some() {
                return Err(format!("{} starts from a picture: pass image (a still, or {{\"frame\": seconds}} for a frame of the clip)", task.name()));
            }
        }
        if let Some(v) = a.get("image").or_else(|| a.get("images")) {
            let mut p = self.pictures(Some(v))?;
            if req.image.is_none() && !p.is_empty() {
                req.image = Some(p.remove(0));
            }
            req.refs.extend(p);
        }
        if let Some(v) = a.get("end_image") {
            req.end_image = Some(self.picture_file(v)?);
        }
        for k in ["style_ref", "refs", "reference"] {
            let p = self.pictures(a.get(k))?;
            req.refs.extend(p);
        }
        req.seed = a.get("seed").and_then(Value::as_u64);
        if let Some(name) = a.get("look").and_then(Value::as_str) {
            let l = g::find_look(&self.gen_dir(), &self.project_json(), name).ok_or_else(|| self.no_look(name))?;
            if !l.words.is_empty() {
                req.prompt = if req.prompt.is_empty() { l.words.clone() } else { format!("{}. {}: {}", req.prompt, l.name, l.words) };
            }
            req.refs.extend(l.refs.iter().map(PathBuf::from).filter(|p| p.is_file()));
            req.seed = req.seed.or(l.seed).or(Some(g::seed_for(&l.name)));
        }
        let mut settings = serde_json::Map::new();
        for (k, v) in a.as_object().into_iter().flatten() {
            if !NOT_SETTINGS.contains(&k.as_str()) {
                settings.insert(k.clone(), v.clone());
            }
        }
        if rendered {
            settings.insert("rendered_clip".into(), json!(true));
        }
        req.settings = Value::Object(settings);
        req.name = a.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        if req.name.is_empty() && req.prompt.is_empty() {
            req.name = format!("{} {}", req.image.as_deref().or(req.clip.as_ref().map(|c| c.footage.path.as_path())).and_then(|p| p.file_stem()).and_then(|s| s.to_str()).unwrap_or("generated"), task.name().replace('_', " "));
        }
        req.estimate = a.get("estimate").and_then(Value::as_bool).unwrap_or(false);
        req.long = a.get("long").and_then(Value::as_bool).unwrap_or(false);
        Ok(req)
    }

    /// Puts what a task made in the project: back into its clip (`clip`
    /// in `a`, unless `replace: false`), or into the media bin and, when
    /// asked, on the timeline.
    pub(crate) fn land_task(&mut self, req: &Request, out: &g::Outcome, a: &Value) -> Result<Value, String> {
        let task = req.task;
        let clip = a.get("clip").and_then(Value::as_u64).map(|c| c as u32);
        let mut v = json!({"task": task.name(), "provider": out.provider.map(|p| p.name()), "by": out.by, "details": out.details});
        if let Some(n) = &out.note {
            v["note"] = json!(n);
        }
        if out.estimate {
            v["estimate"] = json!(true);
            v["sent"] = json!(false);
            return Ok(v);
        }
        v["files"] = json!(out.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
        let replace = clip.is_some() && a.get("replace").and_then(Value::as_bool).unwrap_or(true);
        if replace {
            let id = clip.expect("checked");
            let media = self.import_one(&out.files[0])?;
            self.swap_into(id, media, req, out)?;
            v["clip"] = json!(id);
            v["media"] = json!([media]);
            v["replaced"] = json!(format!("clip {id} now shows the result; generate {{\"restore\": {id}}} puts the original back"));
        } else {
            // The result lands on the timeline unless asked not to: at `at`,
            // at the moment its frame was taken from, else after the edit.
            let frame = a.get("image").and_then(|v| v.get("frame")).and_then(Value::as_f64).filter(|_| f(a, "at").is_none());
            let at = f(a, "at").or(frame);
            let to_timeline = a.get("to_timeline").and_then(Value::as_bool).unwrap_or(true);
            let parts = self.call("import", &json!({"paths": v["files"], "to_timeline": to_timeline && at.is_none()}))?;
            let r: Value = parts.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).unwrap_or(json!({}));
            v["media"] = r["media"].clone();
            v["clips"] = r["clips"].clone();
            if let Some(at) = at.filter(|_| to_timeline) {
                let ops: Vec<Value> = r["media"].as_array().cloned().unwrap_or_default().iter().map(|m| {
                    // Made from a frame of the edit, it sits over that moment
                    // (the main lane would snap it to the nearest cut).
                    let mut op = json!({"op": "place", "media": m, "at": at.max(0.0)});
                    if frame.is_some() {
                        op["lane"] = json!("overlay");
                    }
                    op
                }).collect();
                if !ops.is_empty() {
                    v["placed"] = self.edit(&ops, "Place generated media", false)?;
                }
            }
        }
        v["recipe"] = json!("kept beside the file: generate {\"again\": <media id>} makes it again, count for variations");
        Ok(v)
    }

    fn import_one(&mut self, file: &Path) -> Result<u32, String> {
        let report = crate::services::project::import_media(&mut self.doc, &[file.to_path_buf()]);
        report.ids.first().copied().ok_or_else(|| format!("the result could not be imported: {}", report.failed.join(", ")))
    }

    /// Puts `media` into clip `id` in place of what it showed, keeping how
    /// it sits in the frame (an upscale keeps its framing, an extension
    /// keeps the original part where it was).
    fn swap_into(&mut self, id: u32, media: u32, req: &Request, out: &g::Outcome) -> Result<(), String> {
        let seq = self.doc.seq();
        let mc = self.model_clip(id).ok_or("the clip is gone")?;
        if req.settings.get("rendered_clip").and_then(Value::as_bool) == Some(true) {
            return self.replace_rendered(mc, media, req, out);
        }
        let m::ClipSource::Media(r) = &mc.source else { return Err("the clip is not media".into()) };
        let old = self.doc.project.media(r.media.0 as u32).cloned().ok_or("the clip's media is gone")?;
        let new = self.doc.project.media(media).cloned().ok_or("the result is not in the project")?;
        let mut c = mc.clone();
        if !c.meta.contains_key("generated") {
            c.meta.insert("generated".into(), json!({"media": r.media.0, "source_in": c.source_in, "transform": serde_json::to_value(&c.transform).unwrap_or(Value::Null), "keyframes": serde_json::to_value(&c.keyframes).unwrap_or(Value::Null)}));
        }
        if let Some(gm) = c.meta.get_mut("generated").and_then(Value::as_object_mut) {
            gm.insert("task".into(), json!(req.task.name()));
            gm.insert("by".into(), json!(out.by));
            gm.insert("file".into(), json!(out.files[0].display().to_string()));
        }
        let mut edits = Vec::new();
        if let Some(cin) = &req.clip {
            // Footage: the new file starts at the first frame used.
            let first_us = (cin.range.start as f64 / cin.footage.fps * crate::view::SEC as f64).round() as m::Micros;
            c.source_in = (c.source_in - first_us).max(0);
            let sound = r.audio && self.doc.model.media_item(r.media).is_some_and(|i| i.has_audio());
            if sound {
                edits.push(m::Edit::DetachAudio { seq, clip: mc.id, new_clip: m::ClipId(self.doc.alloc_id()) });
            }
            c.source = m::ClipSource::Media(m::MediaRef { media: m::MediaId(media as u64), video: true, audio: false, extra: m::Extra::new() });
        } else {
            c.source = m::ClipSource::Media(m::MediaRef { media: m::MediaId(media as u64), video: true, audio: r.audio, extra: m::Extra::new() });
        }
        // Framing: content scale k (new pixels per old pixel) and where the
        // old picture sits in the new one.
        let (ow, oh) = (old.width.max(1) as f64, old.height.max(1) as f64);
        let (nw, nh) = (new.width.max(1) as f64, new.height.max(1) as f64);
        let (k, at) = match (req.task, out.details.get("original_at").and_then(Value::as_array)) {
            (Task::Outpaint, Some(xy)) => (1.0, (xy[0].as_f64().unwrap_or(0.0), xy[1].as_f64().unwrap_or(0.0))),
            // A model's canvas without a placement: the original centred.
            (Task::Outpaint, None) => (1.0, ((nw - ow) / 2.0, (nh - oh) / 2.0)),
            _ => (nw / ow, (0.0, 0.0)),
        };
        let (cw, ch) = (self.doc.project.width.max(1) as f64, self.doc.project.height.max(1) as f64);
        let fit = |w: f64, h: f64| (cw / w).min(ch / h);
        let ratio = fit(ow, oh) / (k * fit(nw, nh));
        let off = (at.0 + ow * k / 2.0 - nw / 2.0, at.1 + oh * k / 2.0 - nh / 2.0);
        if (ratio - 1.0).abs() > 1e-6 || off.0.abs() > 0.01 || off.1.abs() > 0.01 || (k - 1.0).abs() > 1e-6 {
            c.transform.scale_x *= ratio;
            c.transform.scale_y *= ratio;
            c.transform.anchor.x = c.transform.anchor.x * k + off.0;
            c.transform.anchor.y = c.transform.anchor.y * k + off.1;
            // A crop stays on the sides that did not grow; a side that grew
            // opens up, so a pan past the old edge shows the new picture.
            let crop = c.transform.crop.clone();
            let grew = |pad: f64| pad > 0.5;
            c.transform.crop.left = if grew(at.0) { 0.0 } else { crop.left * ow * k / nw };
            c.transform.crop.top = if grew(at.1) { 0.0 } else { crop.top * oh * k / nh };
            c.transform.crop.right = if grew(nw - at.0 - ow * k) { 0.0 } else { crop.right * ow * k / nw };
            c.transform.crop.bottom = if grew(nh - at.1 - oh * k) { 0.0 } else { crop.bottom * oh * k / nh };
            for (path, track) in c.keyframes.iter_mut() {
                for key in &mut track.keys {
                    match (path.as_str(), &mut key.value) {
                        ("transform.scale_x" | "transform.scale_y", m::Value::Scalar(s)) => *s *= ratio,
                        ("transform.anchor", m::Value::Point(p)) => {
                            p.x = p.x * k + off.0;
                            p.y = p.y * k + off.1;
                        }
                        _ => {}
                    }
                }
            }
        }
        edits.push(m::Edit::SetClip { seq, clip: Box::new(c) });
        let label = format!("{} clip", req.task.name().replace('_', " "));
        if !self.doc.apply(&label, m::Edit::Batch { edits }) {
            return Err(self.doc.error.take().unwrap_or_else(|| "the clip could not take the result".into()));
        }
        Ok(())
    }

    /// A clip that was rendered to footage (a title, a card, a nested
    /// clip) takes the result as its picture in one step: same lane, start
    /// and length, fitted to the canvas, with what was baked into the
    /// render (its effects, grade, masks, motion) taken off. The whole
    /// original clip is kept for `restore`.
    fn replace_rendered(&mut self, mc: m::Clip, media: u32, req: &Request, out: &g::Outcome) -> Result<(), String> {
        let seq = self.doc.seq();
        let mut c = mc.clone();
        if !c.meta.contains_key("generated") {
            c.meta.insert("generated".into(), json!({"clip": serde_json::to_value(&mc).unwrap_or(Value::Null)}));
        }
        if let Some(gm) = c.meta.get_mut("generated").and_then(Value::as_object_mut) {
            gm.insert("task".into(), json!(req.task.name()));
            gm.insert("by".into(), json!(out.by));
            gm.insert("file".into(), json!(out.files[0].display().to_string()));
        }
        c.source = m::ClipSource::Media(m::MediaRef { media: m::MediaId(media as u64), video: true, audio: false, extra: m::Extra::new() });
        c.source_in = 0;
        c.speed = m::Speed::Constant(1.0);
        c.reversed = false;
        c.transform = m::Transform::default();
        c.effects.clear();
        c.grade = None;
        c.masks.clear();
        c.puppet = None;
        c.corner_pin = None;
        c.keyframes.clear();
        c.drivers.clear();
        c.blend = Default::default();
        let label = format!("{} clip", req.task.name().replace('_', " "));
        let ok = self.doc.apply(&label, m::Edit::SetClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(())
    }

    /// Clip `id` back on what it showed before a generation changed it.
    pub(super) fn restore_generated(&mut self, id: u32) -> Result<String, String> {
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let o = mc.meta.get("generated").and_then(Value::as_object).cloned().ok_or_else(|| format!("clip {id} shows its own media (nothing generated replaced it)"))?;
        // A rendered clip comes back whole.
        if let Some(orig) = o.get("clip").and_then(|c| serde_json::from_value::<m::Clip>(c.clone()).ok()) {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Restore original", m::Edit::SetClip { seq, clip: Box::new(orig) });
            self.check(ok, "")?;
            return Ok(format!("clip {id} is its original again"));
        }
        let mut c = mc.clone();
        let media = o.get("media").and_then(Value::as_u64).ok_or("the original is not recorded")?;
        let audio = matches!(&mc.source, m::ClipSource::Media(r) if r.audio);
        c.source = m::ClipSource::Media(m::MediaRef { media: m::MediaId(media), video: true, audio, extra: m::Extra::new() });
        c.source_in = o.get("source_in").and_then(Value::as_i64).unwrap_or(c.source_in);
        if let Some(t) = o.get("transform").and_then(|t| serde_json::from_value(t.clone()).ok()) {
            c.transform = t;
        }
        if let Some(k) = o.get("keyframes").and_then(|k| serde_json::from_value(k.clone()).ok()) {
            c.keyframes = k;
        }
        c.meta.remove("generated");
        let seq = self.doc.seq();
        let ok = self.doc.apply("Restore original", m::Edit::SetClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(format!("clip {id} shows its original media again (a sound detached when it was replaced stays its own clip)"))
    }

    /// `again`: a generation made again from its recipe; `count` makes
    /// variations (new seeds), `seed` a chosen one; other keys override.
    fn again(&mut self, a: &Value) -> Result<Value, String> {
        let file = match &a["again"] {
            Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => PathBuf::from(s),
            v => self.media_item(v)?.path.ok_or("that media has no file")?,
        };
        let r = g::recipe(&file).ok_or_else(|| format!("{} has no recipe (only what Zornov generated has one)", file.display()))?;
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 8);
        let vary = count > 1 || a.get("vary").and_then(Value::as_bool).unwrap_or(false);
        let mut args = if r["task"] == "make" { r["args"].clone() } else { recipe_args(&r) };
        for (k, v) in a.as_object().into_iter().flatten() {
            if !matches!(k.as_str(), "again" | "count" | "vary") {
                args[k.as_str()] = v.clone();
            }
        }
        if let Some(o) = args.as_object_mut() {
            o.remove("_inner");
            // Again means a new file next to it, not over the clip.
            o.remove("clip");
        }
        let base = args.get("seed").and_then(Value::as_u64).unwrap_or_else(|| g::seed_for(&file.display().to_string()));
        let mut made = Vec::new();
        for i in 0..count {
            let mut one = args.clone();
            if vary && a.get("seed").is_none() {
                one["seed"] = json!(base.wrapping_add(1 + i * 7919) % 4_294_967_291);
            }
            made.push(self.generative(&one)?.ok_or("nothing made")?);
        }
        Ok(if made.len() == 1 { made.remove(0) } else { json!({"variations": made}) })
    }

    /// The `enhance` tool: restoration, upscaling and frame rate for a clip
    /// or a picture, in one pass.
    pub(super) fn enhance(&mut self, a: &Value) -> Result<Value, String> {
        if let Some(id) = a.get("remove").and_then(Value::as_u64).or_else(|| if a.get("remove").and_then(Value::as_bool) == Some(true) { a.get("clip").and_then(Value::as_u64) } else { None }) {
            let s = self.restore_generated(id as u32)?;
            return Ok(json!({"restored": id, "note": s}));
        }
        if a.get("clip").is_none() && a.get("image").is_none() {
            return Err("enhance needs clip (a clip id) or image (a picture: file, media id or {\"frame\": seconds})".into());
        }
        let task = if a.get("steps").is_none() && a.get("old_film").is_none() && a.get("fps").is_none() && (a.get("scale").is_some() || a.get("height").is_some()) { "upscale" } else { "enhance" };
        self.gen_task(Task::parse(task).expect("a task"), a)
    }
}

/// `v` with `k` set.
fn with(v: &Value, k: &str, x: Value) -> Value {
    let mut v = v.clone();
    if !v.is_object() {
        v = json!({});
    }
    v[k] = x;
    v
}

/// A task recipe back as `generate` arguments.
pub(crate) fn recipe_args(r: &Value) -> Value {
    let mut a = r["settings"].clone();
    if !a.is_object() {
        a = json!({});
    }
    a["task"] = r["task"].clone();
    for k in ["prompt", "seed", "image", "end_image"] {
        if !r[k].is_null() {
            a[k] = r[k].clone();
        }
    }
    if r["refs"].as_array().is_some_and(|x| !x.is_empty()) {
        a["refs"] = r["refs"].clone();
    }
    if let Some(p) = r["provider"].as_str() {
        a["provider"] = json!(p);
    }
    a
}

/// The tasks and what each provider does, for `list`.
fn tasks_list() -> Value {
    json!(Task::ALL.iter().map(|t| json!({"task": t.name(), "on_pictures": g::providers(*t, false).iter().map(|p| p.name()).collect::<Vec<_>>(), "on_clips": g::providers(*t, true).iter().map(|p| p.name()).collect::<Vec<_>>()})).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use crate::agent::Session;
    use crate::services;
    use crate::services::tracking::test_movie;
    use serde_json::json;
    use studio_model as m;

    fn png(path: &std::path::Path, w: u32, h: u32) {
        let mut f = studio_ai::Frame::filled(w as usize, h as usize, [80, 150, 60, 255]);
        for (i, p) in f.rgba.chunks_exact_mut(4).enumerate() {
            if (i % w as usize / 8 + i / w as usize / 8) % 2 == 0 {
                p[..3].copy_from_slice(&[210, 190, 160]);
            }
        }
        services::genmedia::save_png(&f, path).unwrap();
    }

    fn session(tag: &str) -> (Session, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("studio-generative-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(services::project::new_custom("Gen", 320, 180, 30.0));
        s.home = Some(dir.clone());
        (s, dir)
    }

    #[test]
    fn a_backdrop_is_extended_in_place_and_restored() {
        let (mut s, dir) = session("outpaint");
        let pic = dir.join("backdrop.png");
        png(&pic, 320, 180);
        s.run("import", &json!({"paths": [pic.display().to_string()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        let before = s.model_clip(id).unwrap();
        let v = s.run("generate", &json!({"task": "outpaint", "clip": id, "right": 0.5, "provider": "local"})).unwrap();
        let v: serde_json::Value = match &v[0] {
            crate::agent::Content::Text(t) => serde_json::from_str(t).unwrap(),
            _ => panic!(),
        };
        assert_eq!(v["provider"], "local");
        let after = s.model_clip(id).unwrap();
        let m::ClipSource::Media(r) = &after.source else { panic!() };
        let mi = s.doc.project.media(r.media.0 as u32).unwrap();
        assert_eq!((mi.width, mi.height), (480, 180));
        // The old picture keeps its size and place: scaled up by 1.5 (the
        // wider picture fits smaller) and anchored on the old centre.
        assert!((after.transform.scale_x - before.transform.scale_x * 1.5).abs() < 1e-6, "{}", after.transform.scale_x);
        assert!((after.transform.anchor.x - (-80.0)).abs() < 1e-6, "{:?}", after.transform.anchor);
        assert_eq!(after.transform.crop.right, 0.0, "the new side is open for a pan");
        s.run("generate", &json!({"restore": id})).unwrap();
        let back = s.model_clip(id).unwrap();
        assert_eq!(back.transform, before.transform);
        assert_eq!(back.source, before.source);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_clip_is_enhanced_to_double_frame_rate_and_its_timing_kept() {
        let (mut s, dir) = session("enhance");
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": id, "end": 0.5})], "trim", false).unwrap();
        let before = s.model_clip(id).unwrap();
        let shown: Vec<i64> = (0..4).map(|k| before.source_time_at(before.start + k * before.duration / 4)).collect();
        let v = s.enhance(&json!({"clip": id, "steps": ["denoise"], "fps": test_movie::FPS * 2})).unwrap();
        assert_eq!(v["task"], "enhance");
        assert_eq!(v["details"]["steps"], json!(["denoise"]));
        let after = s.model_clip(id).unwrap();
        assert_eq!(after.duration, before.duration);
        let m::ClipSource::Media(r) = &after.source else { panic!() };
        let mi = s.doc.model.media_item(r.media).unwrap();
        assert!(mi.path.contains("Generated/"), "{}", mi.path);
        let first_us = shown[0] - after.source_time_at(after.start);
        for (k, t) in shown.iter().enumerate() {
            let now = after.source_time_at(after.start + k as i64 * after.duration / 4);
            assert!((now + first_us - t).abs() < 2_000, "{k}: {now} + {first_us} vs {t}");
        }
        s.enhance(&json!({"remove": id})).unwrap();
        assert_eq!(s.model_clip(id).unwrap().source, before.source);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn looks_follow_generations_and_recipes_make_variations() {
        let (mut s, dir) = session("looks");
        let pic = dir.join("fern.png");
        png(&pic, 64, 36);
        let saved = s.generative(&json!({"save_look": "Fern", "refs": [pic.display().to_string()], "words": "a red fox"})).unwrap().unwrap();
        assert_eq!(saved["saved"]["name"], "Fern");
        assert!(std::path::Path::new(saved["saved"]["refs"][0].as_str().unwrap()).is_file());
        let list = s.generative(&json!({"looks": true})).unwrap().unwrap();
        assert_eq!(list["looks"].as_array().unwrap().len(), 1);
        // A look's reference is the picture to restyle from; its seed rides along.
        let v = s.generative(&json!({"task": "season", "image": pic.display().to_string(), "look": "fern", "season": "autumn", "provider": "local"})).unwrap().unwrap();
        let file = v["files"][0].as_str().unwrap().to_owned();
        let rec = services::genmedia::recipe(std::path::Path::new(&file)).unwrap();
        assert_eq!(rec["seed"], services::genmedia::seed_for("Fern"));
        assert!(rec["prompt"].as_str().unwrap().contains("red fox"));
        let media = v["media"][0].as_u64().unwrap();
        let again = s.generative(&json!({"again": media, "count": 2, "season": "winter"})).unwrap().unwrap();
        let vars = again["variations"].as_array().unwrap();
        assert_eq!(vars.len(), 2);
        let r0 = services::genmedia::recipe(std::path::Path::new(vars[0]["files"][0].as_str().unwrap())).unwrap();
        let r1 = services::genmedia::recipe(std::path::Path::new(vars[1]["files"][0].as_str().unwrap())).unwrap();
        assert_ne!(r0["seed"], r1["seed"]);
        assert_eq!(r0["settings"]["season"], "winter");
        assert!(s.generative(&json!({"task": "relight", "look": "nobody"})).unwrap_err().contains("Fern"));
        let _ = std::fs::remove_dir_all(dir);
    }
    /// A title (no media) is rendered to footage, relit, and put back in
    /// its place in one step; restore brings the title back.
    #[test]
    fn a_title_is_rendered_relit_and_put_back_in_place() {
        let (mut s, dir) = session("title");
        s.edit(&[json!({"op": "card", "colour": "#d8c8a0", "at": 0.0, "duration": 0.4, "width": 200, "height": 100}), json!({"op": "text", "text": "THE END", "at": 0.0, "duration": 0.4})], "x", false).unwrap();
        let title = s.doc.project.clips.iter().find(|c| c.media.is_none() && c.kind != crate::view::ClipKind::Audio).unwrap().id;
        let before = s.model_clip(title).unwrap();
        let v = s.generative(&json!({"task": "relight", "clip": title, "direction": "top-left", "provider": "local"})).unwrap().unwrap();
        assert_eq!(v["clip"], title, "{v}");
        let after = s.model_clip(title).unwrap();
        assert!(matches!(after.source, m::ClipSource::Media(_)), "{:?}", after.source);
        assert_eq!((after.start, after.duration, after.lane), (before.start, before.duration, before.lane));
        assert!(after.effects.is_empty() && after.keyframes.is_empty());
        let m::ClipSource::Media(r) = &after.source else { panic!() };
        let mi = s.doc.project.media(r.media.0 as u32).unwrap();
        assert_eq!(mi.kind, crate::view::MediaKind::Video);
        s.generative(&json!({"restore": title})).unwrap().unwrap();
        assert_eq!(s.model_clip(title).unwrap().source, before.source);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A task on a frame of the edit lands on the timeline at that moment
    /// by itself (no import by hand); to_timeline false keeps it in the bin.
    #[test]
    fn a_result_from_a_frame_lands_on_the_timeline() {
        let (mut s, dir) = session("lands");
        let pic = dir.join("bg.png");
        png(&pic, 320, 180);
        s.run("import", &json!({"paths": [pic.display().to_string()], "to_timeline": true})).unwrap();
        let n = s.doc.project.clips.len();
        let v = s.generative(&json!({"task": "season", "image": {"frame": 0.5}, "season": "winter", "provider": "local"})).unwrap().unwrap();
        let media = v["media"][0].as_u64().unwrap() as u32;
        assert_eq!(s.doc.project.clips.len(), n + 1, "{v}");
        let placed = s.doc.project.clips.iter().find(|c| c.media == Some(media)).expect("on the timeline");
        assert_eq!(placed.start, 500_000);
        let v = s.generative(&json!({"task": "season", "image": {"frame": 0.5}, "season": "autumn", "provider": "local", "to_timeline": false})).unwrap().unwrap();
        let media = v["media"][0].as_u64().unwrap() as u32;
        assert!(s.doc.project.clips.iter().all(|c| c.media != Some(media)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
