// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's raster painting and photo editing.
//!
//! A [`Canvas`] holds the open pictures on the pixel engine
//! (`studio-pixel-engine`: tiled, copy-on-write documents in 8/16/32-bit,
//! colour managed, with layers, groups, masks, clipping, adjustment and fill
//! layers, smart objects that keep their source, blend modes, layer styles,
//! history with snapshots, and every painting, retouching, selection,
//! transform, filter and photo command, each addressed by an id with JSON
//! parameters). Studio adds on top:
//!
//! * pictures kept in the project (`Pictures/<name>.pcraft`, lossless, with
//!   a rendered PNG beside it that the timeline uses), so a person's own
//!   files are only ever read, never written;
//! * AI selections with the local models ([`vision`]): subject, people, sky,
//!   background and the object at a point or in a box;
//! * strokes from paths and shapes ([`strokes`]), so an agent can paint;
//! * frame-by-frame animation and rotoscoping ([`flipbook`]): frames,
//!   timing, onion skin, video frames to paint over;
//! * the ops the AI editor sends ([`ops`]), which mix Studio's ops with any
//!   engine command.

pub mod files;
pub mod flipbook;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod ops;
pub mod strokes;
pub mod vision;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
pub use studio_pixel_doc::Document;
pub use studio_pixel_engine::{CommandSpec, Session, command_specs};
pub use studio_pixel_geom::Rect;
pub use studio_pixel_raster::Rgba8Image;

/// What Studio keeps about an open picture beside its engine document.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// The project's copy (`.pcraft`), written by [`Canvas::save`].
    pub file: Option<PathBuf>,
    /// The file the picture was opened from (read only).
    pub source: Option<PathBuf>,
    /// Frame-by-frame animation, when the picture is one.
    pub flipbook: Option<flipbook::Flipbook>,
}

/// The open pictures.
pub struct Canvas {
    pub engine: Session,
    /// Where pictures are kept (`<project>/Pictures`); `None` keeps them in
    /// memory until a save names a folder.
    pub dir: Option<PathBuf>,
    meta: HashMap<u64, Meta>,
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas::new(None)
    }
}

/// A short error from the engine.
pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Canvas {
    pub fn new(dir: Option<PathBuf>) -> Canvas {
        let mut engine = Session::new();
        // Printing is not part of this build; filters run inline so a
        // headless call returns with its result applied.
        studio_pixel_engine::print_cmds::disable_printing();
        engine.authorize = None;
        Canvas { engine, dir, meta: HashMap::new() }
    }

    /// The open pictures' names, in order.
    pub fn names(&self) -> Vec<String> {
        self.engine.documents().iter().map(|d| d.doc.name.clone()).collect()
    }

    /// The active picture's document.
    pub fn doc(&self) -> Option<&Document> {
        self.engine.active().map(|d| &*d.doc)
    }

    fn active_id(&self) -> Option<u64> {
        self.doc().map(|d| d.id.0)
    }

    /// Studio's notes about the active picture.
    pub fn meta(&self) -> Option<&Meta> {
        self.active_id().and_then(|id| self.meta.get(&id))
    }

    pub fn meta_mut(&mut self) -> Option<&mut Meta> {
        let id = self.active_id()?;
        Some(self.meta.entry(id).or_default())
    }

    /// Make the picture called `name` (or at that index) the active one.
    pub fn activate(&mut self, name: &str) -> Result<(), String> {
        let docs = self.engine.documents();
        let want = name.trim().to_lowercase();
        let i = docs
            .iter()
            .position(|d| d.doc.name.to_lowercase() == want || stem(&d.doc.name).to_lowercase() == want)
            .or_else(|| name.parse::<usize>().ok().filter(|&i| i < docs.len()))
            .or_else(|| docs.iter().position(|d| d.doc.name.to_lowercase().contains(&want)))
            .ok_or_else(|| format!("no picture \"{name}\"; open: {}", self.names().join(", ")))?;
        self.engine.set_active(i);
        Ok(())
    }

    /// A name no open picture has (`Painting`, `Painting 2`, …).
    fn fresh_name(&self, base: &str) -> String {
        let names = self.names();
        let taken = |n: &str| names.iter().any(|m| stem(m).eq_ignore_ascii_case(n)) || self.dir.as_ref().is_some_and(|d| d.join(format!("{n}.{}", files::NATIVE)).exists());
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_owned())
    }

    /// A new picture (`params` as the engine's `file.new`: width, height,
    /// depth 8/16/32, mode, background, resolution …).
    pub fn new_picture(&mut self, name: &str, params: Value) -> Result<Value, String> {
        let name = self.fresh_name(if name.trim().is_empty() { "Painting" } else { name.trim() });
        let mut p = params;
        if let Value::Object(m) = &mut p {
            m.insert("name".into(), json!(name));
        }
        let r = self.engine.execute("file.new", p).map_err(err)?;
        let file = self.dir.as_ref().map(|d| d.join(format!("{name}.{}", files::NATIVE)));
        self.rename_active(&name);
        if let Some(id) = self.active_id() {
            self.meta.insert(id, Meta { file, ..Meta::default() });
        }
        Ok(json!({"picture": name, "new": r}))
    }

    fn rename_active(&mut self, name: &str) {
        if let Some(d) = self.engine.active_mut() {
            if d.doc.name != name {
                std::sync::Arc::make_mut(&mut d.doc).name = name.to_owned();
            }
        }
    }

    /// Open a picture from a file. The file is only read: the picture
    /// becomes a copy kept in the project, and saves go there.
    pub fn open(&mut self, path: &Path) -> Result<Value, String> {
        let o = files::open(path)?;
        let base = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Picture".into());
        let name = self.fresh_name(&base);
        let mut doc = o.document;
        doc.name = name.clone();
        studio_pixel_engine::file_cmds::raw_defaults(&self.engine, &o.warnings, &mut doc);
        self.engine.add_document(doc, None);
        let file = self.dir.as_ref().map(|d| d.join(format!("{name}.{}", files::NATIVE)));
        if let Some(id) = self.active_id() {
            self.meta.insert(id, Meta { file, source: Some(path.to_path_buf()), flipbook: None });
        }
        let d = self.doc().ok_or("the picture did not open")?;
        Ok(json!({"picture": name, "size": [d.size.width, d.size.height], "depth": format!("{:?}", d.depth), "mode": format!("{:?}", d.mode), "layers": d.layers.len(), "warnings": o.warnings}))
    }

    /// Open a picture from RGBA8 pixels (a video frame, a render).
    pub fn open_rgba(&mut self, name: &str, width: u32, height: u32, rgba: &[u8]) -> Result<Value, String> {
        let r = self.new_picture(name, json!({"width": width, "height": height, "background": "transparent"}))?;
        self.put_rgba(width, height, rgba)?;
        Ok(r)
    }

    /// Replace the active layer's pixels with RGBA8 (top-left at 0,0).
    pub fn put_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err("the pixels do not match the size".into());
        }
        let rect = studio_pixel_geom::Rect::from_xywh(0, 0, width, height);
        self.engine
            .edit("Place Pixels", |doc, active| {
                let id = active.ok_or_else(|| studio_pixel_engine::EngineError::Other("no active layer".into()))?;
                let l = doc.layer_mut(*active.as_ref().unwrap_or(&id)).ok_or(studio_pixel_engine::EngineError::NoLayer(id))?;
                let s = l.surface_mut().ok_or_else(|| studio_pixel_engine::EngineError::Other("not a pixel layer".into()))?;
                let fmt = s.format();
                let src = studio_pixel_raster::Surface::from_interleaved(studio_pixel_color::PixelFormat::RGBA8, rect, rgba);
                *s = if fmt == studio_pixel_color::PixelFormat::RGBA8 { src } else { src.convert(fmt) };
                Ok(())
            })
            .map_err(err)
    }

    /// Where the active picture is kept, if it has a home yet.
    pub fn file(&self) -> Option<PathBuf> {
        self.meta().and_then(|m| m.file.clone())
    }

    /// Save the active picture into the project (lossless), with a rendered
    /// PNG beside it for the timeline, and the flipbook's frames when it is
    /// one. Returns the files written.
    pub fn save(&mut self) -> Result<Value, String> {
        let name = self.doc().map(|d| stem(&d.name).to_owned()).ok_or("no picture open")?;
        let file = match self.file() {
            Some(f) => f,
            None => {
                let dir = self.dir.clone().ok_or("this picture has no folder yet; save it to a path")?;
                let f = dir.join(format!("{name}.{}", files::NATIVE));
                if let Some(m) = self.meta_mut() {
                    m.file = Some(f.clone());
                }
                f
            }
        };
        if self.meta().and_then(|m| m.source.as_deref()).is_some_and(|s| same_file(s, &file)) {
            return Err("refusing to write over the file the picture was opened from".into());
        }
        let doc = self.doc().ok_or("no picture open")?.clone();
        files::save(&doc, &file, None, &studio_pixel_io::ExportOptions::default())?;
        let png = file.with_extension("png");
        files::save(&doc, &png, None, &studio_pixel_io::ExportOptions::default())?;
        let mut out = json!({"picture": name, "file": file.display().to_string(), "png": png.display().to_string()});
        if let Some(fb) = self.meta().and_then(|m| m.flipbook.clone()) {
            let frames = fb.render_frames(self, &file.with_extension("frames"))?;
            fb.save(&file.with_extension("flipbook.json"))?;
            out["frames"] = json!(frames.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
        }
        if let Some(st) = self.engine.active_mut() {
            st.saved_revision = st.revision;
        }
        Ok(out)
    }

    /// Write the active picture to `path` in any format the extension (or
    /// `format`) names: PSD/PSB with layers, TIFF (layers with `layers`),
    /// PNG, JPEG, WebP, EXR, GIF, BMP, TGA, or the native bundle.
    pub fn export(&self, path: &Path, format: Option<&str>, opts: &Value) -> Result<Value, String> {
        if self.meta().and_then(|m| m.source.as_deref()).is_some_and(|s| same_file(s, path)) {
            return Err("refusing to write over the file the picture was opened from; pick another name".into());
        }
        let doc = self.doc().ok_or("no picture open")?;
        let mut o = studio_pixel_io::ExportOptions::default();
        o.tiff_layers = opts.get("layers").and_then(Value::as_bool).unwrap_or(true);
        if let Some(q) = opts.get("quality").and_then(Value::as_f64) {
            let q = q.clamp(1.0, 100.0) as u8;
            o.encode.jpeg_quality = q;
            o.encode.webp_quality = q;
        }
        let warnings = files::save(doc, path, format, &o)?;
        Ok(json!({"file": path.display().to_string(), "warnings": warnings}))
    }

    /// Run an engine command on the active picture (any id `commands`
    /// lists), waiting for background work to land.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.engine.execute(id, params).map_err(err)?;
        // Commands that start a job answer `{"job": id}`; wait for it.
        if let Some(job) = r.get("job").and_then(Value::as_u64) {
            let v = self.engine.wait_job(studio_pixel_engine::jobs::JobId(job)).map_err(err)?;
            return Ok(v);
        }
        self.engine.poll_jobs();
        Ok(r)
    }

    /// The composite of the active picture, at most `max_side` pixels on its
    /// longer side (0 = full size), straight RGBA8.
    pub fn render(&self, max_side: u32) -> Result<Rgba8Image, String> {
        let doc = self.doc().ok_or("no picture open")?;
        let side = if max_side == 0 { doc.size.width.max(doc.size.height) } else { max_side };
        Ok(studio_pixel_compose::thumbnail(doc, side.max(1)))
    }

    /// Part of the composite: `rect` (x, y, width, height in document
    /// pixels) reduced (area-averaged) to `w × h`, straight RGBA8. The
    /// Paint workspace draws what is on screen with it.
    pub fn view(&self, rect: (i32, i32, u32, u32), w: u32, h: u32) -> Result<Rgba8Image, String> {
        let doc = self.doc().ok_or("no picture open")?;
        let r = studio_pixel_geom::Rect::from_xywh(rect.0, rect.1, rect.2.max(1), rect.3.max(1));
        Ok(studio_pixel_compose::render_reduced_rect(doc, r, w.max(1), h.max(1)).to_rgba8())
    }

    /// A short description of the active picture: size, depth, layers (top
    /// first) with their kind, blend, opacity and mask, and the selection.
    pub fn summary(&self) -> Result<Value, String> {
        let st = self.engine.active().ok_or("no picture open")?;
        let d = &st.doc;
        let mut layers = Vec::new();
        fn walk(ls: &[studio_pixel_doc::Layer], depth: usize, active: Option<studio_pixel_doc::LayerId>, out: &mut Vec<Value>) {
            for l in ls.iter().rev() {
                let mut v = json!({"id": l.id.0, "name": l.name, "kind": l.content.kind_name(), "blend": format!("{:?}", l.blend), "opacity": (l.opacity * 100.0).round()});
                if !l.visible {
                    v["hidden"] = json!(true);
                }
                if l.mask.is_some() {
                    v["mask"] = json!(true);
                }
                if l.clipped {
                    v["clipped"] = json!(true);
                }
                if depth > 0 {
                    v["in_group"] = json!(depth);
                }
                if Some(l.id) == active {
                    v["active"] = json!(true);
                }
                out.push(v);
                if let Some(c) = l.children() {
                    walk(c, depth + 1, active, out);
                }
            }
        }
        walk(&d.layers, 0, st.active_layer, &mut layers);
        let sel = d.selection.as_ref().map(|s| {
            let b = s.content_bounds();
            json!([b.x0, b.y0, b.width(), b.height()])
        });
        let mut v = json!({
            "picture": d.name, "size": [d.size.width, d.size.height], "depth": format!("{:?}", d.depth), "mode": format!("{:?}", d.mode),
            "layers": layers, "selection": sel, "can_undo": st.history.can_undo(), "revision": st.revision,
        });
        if let Some(m) = self.meta() {
            if let Some(f) = &m.file {
                v["file"] = json!(f.display().to_string());
            }
            if let Some(fb) = &m.flipbook {
                v["flipbook"] = fb.describe();
            }
        }
        Ok(v)
    }

    /// Engine commands whose id or label contains every word of `query`
    /// (all with an empty query), with their parameters.
    pub fn commands(query: &str) -> Vec<Value> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        command_specs()
            .iter()
            .filter(|c| !c.id.starts_with("print") && c.id != "file.print" && c.id != "file.printOneCopy")
            .filter(|c| {
                let hay = format!("{} {} {}", c.id, c.label, c.menu.join(" ")).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|c| json!({"id": c.id, "label": c.label, "params": c.params}))
            .collect()
    }
}

/// A file name without its extension.
pub fn stem(name: &str) -> &str {
    Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Encode RGBA8 as PNG.
pub fn png(img: &Rgba8Image) -> Result<Vec<u8>, String> {
    let image = studio_pixel_codecs::Image::from_u8(img.width, img.height, studio_pixel_codecs::ChannelLayout::Rgba, img.pixels.clone()).map_err(err)?;
    studio_pixel_codecs::encode(&image, studio_pixel_codecs::Format::Png, &Default::default()).map_err(err)
}

/// Encode RGBA8 as JPEG over white (for looking at a picture).
pub fn jpeg(img: &Rgba8Image, quality: u8) -> Result<Vec<u8>, String> {
    let mut rgb = Vec::with_capacity(img.pixels.len() / 4 * 3);
    for p in img.pixels.chunks_exact(4) {
        let a = p[3] as u32;
        // Over a light checker-free grey so transparency reads as "empty".
        for c in &p[..3] {
            rgb.push(((*c as u32 * a + 236 * (255 - a)) / 255) as u8);
        }
    }
    let image = studio_pixel_codecs::Image::from_u8(img.width, img.height, studio_pixel_codecs::ChannelLayout::Rgb, rgb).map_err(err)?;
    let mut o = studio_pixel_codecs::EncodeOptions::default();
    o.jpeg_quality = quality.clamp(1, 100);
    studio_pixel_codecs::encode(&image, studio_pixel_codecs::Format::Jpeg, &o).map_err(err)
}
