// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Frame-by-frame animation and rotoscoping.
//!
//! A flipbook is an ordinary picture with a group called `Frames` holding
//! one group per drawing (bottom first), shown one at a time; an optional
//! group `Reference` holds a video frame per drawing to paint over (it is
//! never part of the rendered animation). Layers outside both groups (a
//! painted background, a title) show under every frame. Because the
//! frames are plain layers, a flipbook saves losslessly, opens as layers
//! anywhere that reads PSD, and every paint and retouch tool works on a
//! frame.
//!
//! Timing is per drawing: `hold` is how many frames of the animation it
//! stays (1 = on ones, 2 = on twos). Onion skin shows the neighbouring
//! drawings tinted (earlier red, later green) under the current one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use studio_pixel_doc::{Document, Layer, LayerContent, LayerId};
use studio_pixel_raster::Rgba8Image;

use crate::Canvas;

pub const FRAMES: &str = "Frames";
pub const REFERENCE: &str = "Reference";

/// Onion skin settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Onion {
    pub before: u32,
    pub after: u32,
    /// Opacity of the nearest neighbour; further ones fade.
    pub opacity: f32,
    pub on: bool,
}

impl Default for Onion {
    fn default() -> Self {
        Onion { before: 1, after: 1, opacity: 0.35, on: true }
    }
}

/// Timing and view state of a flipbook (the drawings live in the document).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Flipbook {
    pub fps: f64,
    /// Exposures per drawing, in order.
    pub holds: Vec<u32>,
    pub current: usize,
    pub onion: Onion,
    #[serde(default = "yes")]
    pub looping: bool,
    /// Where the reference video came from (`{"media": …, "at": seconds}`), for the record.
    #[serde(default)]
    pub source: Option<Value>,
}

fn yes() -> bool {
    true
}

fn find_top<'a>(doc: &'a Document, name: &str) -> Option<&'a Layer> {
    doc.layers.iter().find(|l| l.is_group() && l.name == name)
}

fn top_mut<'a>(doc: &'a mut Document, name: &str) -> Option<&'a mut Layer> {
    doc.layers.iter_mut().find(|l| l.is_group() && l.name == name)
}

/// The drawings' group ids, in order.
pub fn frame_ids(doc: &Document) -> Vec<LayerId> {
    find_top(doc, FRAMES).and_then(Layer::children).map(|c| c.iter().map(|l| l.id).collect()).unwrap_or_default()
}

fn blank_frame(doc: &Document, n: usize) -> Layer {
    let ink = Layer::raster("Ink", doc.pixel_format());
    Layer::group(format!("Frame {n}"), vec![ink])
}

fn err(e: impl std::fmt::Display) -> studio_pixel_engine::EngineError {
    studio_pixel_engine::EngineError::Other(e.to_string())
}

impl Flipbook {
    pub fn describe(&self) -> Value {
        let total: u32 = self.holds.iter().sum();
        json!({"drawings": self.holds.len(), "frames": total, "fps": self.fps, "seconds": (total as f64 / self.fps * 100.0).round() / 100.0, "current": self.current + 1, "holds": self.holds, "onion": self.onion, "loop": self.looping})
    }

    /// Turn the active picture into a flipbook of `drawings` blank drawings
    /// (or keep the frames it already has), each held `hold` frames.
    pub fn start(c: &mut Canvas, drawings: usize, fps: f64, hold: u32) -> Result<Value, String> {
        let drawings = drawings.clamp(1, 2000);
        let existing = c.doc().map(|d| frame_ids(d).len()).ok_or("no picture open")?;
        if existing == 0 {
            c.engine
                .edit("New Animation", |doc, active| {
                    let frames: Vec<Layer> = (1..=drawings).map(|i| blank_frame(doc, i)).collect();
                    let first_ink = frames[0].children().and_then(|c| c.first()).map(|l| l.id);
                    doc.layers.push(Layer::group(FRAMES, frames));
                    *active = first_ink;
                    Ok(())
                })
                .map_err(crate::err)?;
        }
        let n = c.doc().map(|d| frame_ids(d).len()).unwrap_or(0);
        let fb = Flipbook { fps: fps.clamp(1.0, 120.0), holds: vec![hold.clamp(1, 240); n], current: 0, onion: Onion::default(), looping: true, source: None };
        if let Some(m) = c.meta_mut() {
            m.flipbook = Some(fb);
        }
        Self::show(c, 0)?;
        Ok(c.meta().and_then(|m| m.flipbook.as_ref()).map(Flipbook::describe).unwrap_or(Value::Null))
    }

    fn get(c: &Canvas) -> Result<Flipbook, String> {
        c.meta().and_then(|m| m.flipbook.clone()).ok_or_else(|| "this picture is not an animation; start one with {\"op\": \"animate\", \"drawings\": n}".into())
    }

    fn put(c: &mut Canvas, fb: Flipbook) {
        if let Some(m) = c.meta_mut() {
            m.flipbook = Some(fb);
        }
    }

    /// Keep `holds` in step with the drawings in the document (after undo, or edits in the layers).
    fn sync(c: &mut Canvas) -> Result<Flipbook, String> {
        let mut fb = Self::get(c)?;
        let n = c.doc().map(|d| frame_ids(d).len()).unwrap_or(0);
        fb.holds.resize(n, 1);
        fb.current = fb.current.min(n.saturating_sub(1));
        Self::put(c, fb.clone());
        Ok(fb)
    }

    /// Show drawing `i` (and its reference frame), make its top pixel layer
    /// the one painting goes to. No history step: it is like turning a page.
    pub fn show(c: &mut Canvas, i: usize) -> Result<Value, String> {
        let mut fb = Self::sync(c)?;
        let n = fb.holds.len();
        if n == 0 {
            return Err("the animation has no drawings".into());
        }
        let i = i.min(n - 1);
        fb.current = i;
        Self::put(c, fb);
        let st = c.engine.active_mut().ok_or("no picture open")?;
        let doc = Arc::make_mut(&mut st.doc);
        let mut target = None;
        for name in [FRAMES, REFERENCE] {
            if let Some(kids) = top_mut(doc, name).and_then(Layer::children_mut) {
                for (k, l) in kids.iter_mut().enumerate() {
                    l.visible = k == i;
                    if k == i && name == FRAMES {
                        target = l.children().and_then(|c| c.iter().rev().find(|x| matches!(x.content, LayerContent::Raster(_)))).map(|x| x.id).or(Some(l.id));
                    }
                }
            }
        }
        if target.is_some() {
            st.active_layer = target;
            st.selected_layers = target.into_iter().collect();
        }
        st.revision += 1;
        st.view_revision += 1;
        Ok(json!({"drawing": i + 1, "of": n}))
    }

    /// Add a drawing after `after` (0-based; default the current one): blank,
    /// or a copy of it with `copy` (to trace and change).
    pub fn add(c: &mut Canvas, after: Option<usize>, copy: bool, hold: Option<u32>) -> Result<Value, String> {
        let mut fb = Self::sync(c)?;
        let at = after.unwrap_or(fb.current).min(fb.holds.len().saturating_sub(1));
        c.engine
            .edit(if copy { "Duplicate Drawing" } else { "New Drawing" }, |doc, _| {
                let n = frame_ids(doc).len();
                let fresh = blank_frame(doc, n + 1);
                let kids = top_mut(doc, FRAMES).and_then(Layer::children_mut).ok_or_else(|| err("no Frames group"))?;
                let new = if copy {
                    let mut d = kids.get(at).ok_or_else(|| err("no such drawing"))?.duplicate();
                    d.name = format!("Frame {}", n + 1);
                    d
                } else {
                    fresh
                };
                kids.insert((at + 1).min(kids.len()), new);
                Ok(())
            })
            .map_err(crate::err)?;
        let h = hold.unwrap_or_else(|| fb.holds.get(at).copied().unwrap_or(1));
        fb.holds.insert((at + 1).min(fb.holds.len()), h);
        Self::put(c, fb);
        Self::show(c, at + 1)
    }

    /// Remove drawing `i`.
    pub fn delete(c: &mut Canvas, i: usize) -> Result<Value, String> {
        let mut fb = Self::sync(c)?;
        if fb.holds.len() <= 1 {
            return Err("an animation keeps at least one drawing".into());
        }
        let i = i.min(fb.holds.len() - 1);
        c.engine
            .edit("Delete Drawing", |doc, active| {
                let kids = top_mut(doc, FRAMES).and_then(Layer::children_mut).ok_or_else(|| err("no Frames group"))?;
                kids.remove(i);
                *active = None;
                if let Some(r) = top_mut(doc, REFERENCE).and_then(Layer::children_mut) {
                    if i < r.len() {
                        r.remove(i);
                    }
                }
                Ok(())
            })
            .map_err(crate::err)?;
        fb.holds.remove(i);
        Self::put(c, fb);
        Self::show(c, i.saturating_sub(1))
    }

    /// Set how many frames drawings hold (`which`: one index, or all).
    pub fn set_hold(c: &mut Canvas, which: Option<usize>, hold: u32) -> Result<Value, String> {
        let mut fb = Self::sync(c)?;
        let h = hold.clamp(1, 240);
        match which {
            Some(i) => *fb.holds.get_mut(i).ok_or("no such drawing")? = h,
            None => fb.holds.iter_mut().for_each(|x| *x = h),
        }
        let d = fb.describe();
        Self::put(c, fb);
        Ok(d)
    }

    /// Change speed, onion skin or looping.
    pub fn set(c: &mut Canvas, op: &Value) -> Result<Value, String> {
        let mut fb = Self::sync(c)?;
        if let Some(f) = op.get("fps").and_then(Value::as_f64) {
            fb.fps = f.clamp(1.0, 120.0);
        }
        if let Some(o) = op.get("onion") {
            match o {
                Value::Bool(b) => fb.onion.on = *b,
                Value::Object(_) => {
                    if let Some(b) = o.get("before").and_then(Value::as_u64) {
                        fb.onion.before = b.min(10) as u32;
                    }
                    if let Some(a) = o.get("after").and_then(Value::as_u64) {
                        fb.onion.after = a.min(10) as u32;
                    }
                    if let Some(p) = o.get("opacity").and_then(Value::as_f64) {
                        fb.onion.opacity = (if p > 1.0 { p / 100.0 } else { p }).clamp(0.0, 1.0) as f32;
                    }
                    if let Some(b) = o.get("on").and_then(Value::as_bool) {
                        fb.onion.on = b;
                    }
                }
                _ => return Err("`onion` is true/false or {before, after, opacity}".into()),
            }
        }
        if let Some(l) = op.get("loop").and_then(Value::as_bool) {
            fb.looping = l;
        }
        let d = fb.describe();
        Self::put(c, fb);
        Ok(d)
    }

    /// Put video frames (RGBA8, one per drawing, in order) under the
    /// drawings as the reference to trace, making more drawings if needed.
    pub fn set_reference(c: &mut Canvas, frames: Vec<(u32, u32, Vec<u8>)>, source: Option<Value>) -> Result<Value, String> {
        if frames.is_empty() {
            return Err("no video frames to trace".into());
        }
        let n = frames.len();
        let mut fb = Self::sync(c)?;
        let (dw, dh) = c.doc().map(|d| (d.size.width, d.size.height)).ok_or("no picture open")?;
        c.engine
            .edit("Reference Video", |doc, _| {
                let fmt = doc.pixel_format();
                let mut layers = Vec::with_capacity(n);
                for (k, (w, h, px)) in frames.iter().enumerate() {
                    let rgba = if (*w, *h) == (dw, dh) { px.clone() } else { scale_rgba(px, *w, *h, dw, dh) };
                    let src = studio_pixel_raster::Surface::from_interleaved(studio_pixel_color::PixelFormat::RGBA8, studio_pixel_geom::Rect::from_xywh(0, 0, dw, dh), &rgba);
                    let mut l = Layer::raster(format!("Video {}", k + 1), fmt);
                    if let Some(s) = l.surface_mut() {
                        *s = if fmt == studio_pixel_color::PixelFormat::RGBA8 { src } else { src.convert(fmt) };
                    }
                    l.locks.all = true;
                    layers.push(l);
                }
                doc.layers.retain(|l| !(l.is_group() && l.name == REFERENCE));
                let mut g = Layer::group(REFERENCE, layers);
                g.opacity = 0.6;
                doc.layers.insert(0, g);
                // As many drawings as video frames.
                let have = frame_ids(doc).len();
                for i in have..n {
                    let f = blank_frame(doc, i + 1);
                    if let Some(kids) = top_mut(doc, FRAMES).and_then(Layer::children_mut) {
                        kids.push(f);
                    }
                }
                Ok(())
            })
            .map_err(crate::err)?;
        let have = c.doc().map(|d| frame_ids(d).len()).unwrap_or(0);
        fb.holds.resize(have, 1);
        fb.source = source;
        Self::put(c, fb);
        Self::show(c, 0)?;
        Ok(json!({"reference_frames": n, "drawings": have}))
    }

    /// The document with only drawing `i` showing (and no reference unless `with_reference`).
    fn doc_at(doc: &Document, i: usize, with_reference: bool) -> Document {
        let mut d = doc.clone();
        for l in d.layers.iter_mut() {
            if l.is_group() && l.name == REFERENCE {
                l.visible = with_reference;
                if let Some(k) = l.children_mut() {
                    for (j, x) in k.iter_mut().enumerate() {
                        x.visible = j == i;
                    }
                }
            }
            if l.is_group() && l.name == FRAMES {
                l.visible = true;
                if let Some(k) = l.children_mut() {
                    for (j, x) in k.iter_mut().enumerate() {
                        x.visible = j == i;
                    }
                }
            }
        }
        d
    }

    /// The current drawing with its onion skin and reference, for the screen.
    pub fn onion_view(&self, doc: &Document, max_side: u32) -> Rgba8Image {
        let cur = self.current;
        let base = studio_pixel_compose::thumbnail(&Self::doc_at(doc, cur, true), max_side);
        if !self.onion.on {
            return base;
        }
        let mut out = base.clone();
        let n = self.holds.len();
        let mut layers: Vec<(usize, [f32; 3], f32)> = Vec::new();
        for k in 1..=self.onion.before as usize {
            if let Some(j) = cur.checked_sub(k).or_else(|| (self.looping && n > 2).then(|| (cur + n - k) % n)).filter(|&j| j != cur) {
                layers.push((j, [0.9, 0.2, 0.2], self.onion.opacity / k as f32));
            }
        }
        for k in 1..=self.onion.after as usize {
            let j = cur + k;
            let j = if j < n { Some(j) } else if self.looping && n > 2 { Some(j % n) } else { None };
            if let Some(j) = j.filter(|&j| j != cur) {
                layers.push((j, [0.2, 0.75, 0.3], self.onion.opacity / k as f32));
            }
        }
        // Only the drawing itself (no background, no reference) for the ghosts.
        for (j, tint, a) in layers {
            let ghost = studio_pixel_compose::thumbnail(&Self::only_drawing(doc, j), max_side);
            if ghost.width != out.width || ghost.height != out.height {
                continue;
            }
            for (o, g) in out.pixels.chunks_exact_mut(4).zip(ghost.pixels.chunks_exact(4)) {
                let ga = g[3] as f32 / 255.0 * a;
                if ga <= 0.0 {
                    continue;
                }
                let lum = (0.3 * g[0] as f32 + 0.59 * g[1] as f32 + 0.11 * g[2] as f32) / 255.0;
                for c in 0..3 {
                    let ink = tint[c] * (0.35 + 0.65 * (1.0 - lum));
                    o[c] = ((o[c] as f32 / 255.0) * (1.0 - ga) * 255.0 + ink * ga * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                o[3] = o[3].max((ga * 255.0) as u8);
            }
        }
        out
    }

    fn only_drawing(doc: &Document, i: usize) -> Document {
        let mut d = Self::doc_at(doc, i, false);
        for l in d.layers.iter_mut() {
            if !(l.is_group() && l.name == FRAMES) {
                l.visible = false;
            }
        }
        d
    }

    /// Render every drawing (with what lies outside the Frames group, never
    /// the reference) to `dir/0001.png …`. Returns the files in order.
    pub fn render_frames(&self, c: &Canvas, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let doc = c.doc().ok_or("no picture open")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // Old frames from a longer version go.
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "png") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let n = frame_ids(doc).len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let d = Self::doc_at(doc, i, false);
            let img = studio_pixel_compose::thumbnail(&d, d.size.width.max(d.size.height));
            let p = dir.join(format!("{:04}.png", i + 1));
            std::fs::write(&p, crate::png(&img)?).map_err(|e| format!("{}: {e}", p.display()))?;
            out.push(p);
        }
        Ok(out)
    }

    /// The drawings as (start, duration) seconds, from their holds.
    pub fn timing(&self) -> Vec<(f64, f64)> {
        let mut t = 0u32;
        self.holds
            .iter()
            .map(|h| {
                let s = t as f64 / self.fps;
                t += h;
                (s, *h as f64 / self.fps)
            })
            .collect()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Option<Flipbook> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
}

/// Nearest-neighbour-free bilinear RGBA8 resize.
pub fn scale_rgba(px: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let (w, h, nw, nh) = (w as usize, h as usize, nw as usize, nh as usize);
    let mut out = vec![0u8; nw * nh * 4];
    if w == 0 || h == 0 {
        return out;
    }
    for y in 0..nh {
        let sy = ((y as f32 + 0.5) * h as f32 / nh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, fy) = (sy as usize, sy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..nw {
            let sx = ((x as f32 + 0.5) * w as f32 / nw as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, fx) = (sx as usize, sx.fract());
            let x1 = (x0 + 1).min(w - 1);
            for c in 0..4 {
                let g = |xx: usize, yy: usize| px[(yy * w + xx) * 4 + c] as f32;
                let v = (g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx) * (1.0 - fy) + (g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx) * fy;
                out[(y * nw + x) * 4 + c] = v.round() as u8;
            }
        }
    }
    out
}
