// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background removal, object cut-outs and rotoscoping, shared by the
//! Inspector, the AI panel and the AI editor.
//!
//! * A **cut-out** keeps a per-frame alpha matte of the clip's footage in a
//!   matte file (`studio_ai::features::matte_file`) next to the project
//!   (`Cut-outs/`); the clip's `meta["matte"]` names it and the renderer
//!   multiplies it into the picture (CPU and GPU alike, `studio_render::
//!   matte`). People are matted with MODNet (hair-fine edges, any number
//!   of people); anything else is selected with clicks or a box and carried
//!   through the clip by SAM 2.1.
//! * **Roto** turns SAM's per-frame masks into one path mask with a key on
//!   its points every frame or few: an outline a person can then nudge
//!   point by point on the canvas, like a roto artist's.

use std::ops::Range;
use std::path::{Path, PathBuf};

use eframe::egui::Vec2;
use serde_json::{Value, json};
use studio_ai::features::matte_file;
use studio_ai::features::matting::{refine_edges, BackgroundRemover, MatteSmoother};
use studio_ai::features::roto;
use studio_ai::features::segment::{ObjectCutout, Prompt, Tracker};
use studio_ai::frame::{Frame, Mask};
use studio_ai::{JobCtx, ModelStore};
use studio_model as m;

use super::tracking::{self, Footage};
use crate::editor::xform;
use crate::view::{self, Time, SEC};

/// Longest side of the mattes kept on disk (and of the frames the models
/// see): edges stay fine on a 1080p canvas, files stay small.
pub const MATTE_SIZE: usize = 1280;

/// What picks the subject.
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    /// The people in the shot (MODNet).
    People,
    /// Whatever the prompts point at on frame `at` (SAM): clicks in and out
    /// of it, or a box around it, in matte pixels.
    Object { at: u64, prompts: Vec<Prompt> },
}

/// A cut-out or roto run worked out ahead.
#[derive(Clone, Debug)]
pub struct CutJob {
    pub clip: u32,
    pub media: u64,
    pub footage: Footage,
    pub range: Range<u64>,
    pub subject: Subject,
    /// Matte (and model frame) size.
    pub size: (usize, usize),
    /// Where the matte file goes (cut-outs).
    pub file: PathBuf,
    /// Smooth the matte over time (people), 0..1.
    pub smoothing: f32,
    /// Roto: run SAM on every `step`-th frame.
    pub step: u64,
}

/// What a cut-out run found.
#[derive(Clone, Debug, PartialEq)]
pub struct CutResult {
    pub frames: usize,
    /// Mean share of the picture kept.
    pub coverage: f32,
    /// Frames where nothing was found.
    pub empty: usize,
}

fn matte_size(f: &Footage) -> (usize, usize) {
    let (w, h) = (f.width.max(1) as usize, f.height.max(1) as usize);
    let k = (MATTE_SIZE as f64 / w.max(h) as f64).min(1.0);
    (((w as f64 * k).round() as usize).max(16), ((h as f64 * k).round() as usize).max(16))
}

/// RGBA frames of a file at the matte size, decoded in order and kept for
/// a while (SAM walks backwards too).
pub(crate) struct RgbaFrames {
    src: Box<dyn studio_render::source::MediaSource>,
    size: (usize, usize),
    cache: std::collections::BTreeMap<u64, Frame>,
    base: u64,
    count: usize,
    fps: f64,
}

/// A decoded picture as RGBA8 at `w`×`h` (BT.709, box filtered).
fn rgba(pic: &studio_render::yuv::DecodedPicture, w: usize, h: usize) -> Frame {
    use studio_render::yuv::{PixelData, Planes};
    let (pw, ph) = (pic.width.max(1) as usize, pic.height.max(1) as usize);
    let mut out = vec![0u8; w * h * 4];
    let max = |p: &Planes| match p {
        Planes::U8(_) => 255.0,
        Planes::U16(_) => ((1u32 << pic.colour.bit_depth.clamp(8, 16)) - 1) as f32,
    };
    for y in 0..h {
        let (y0, y1) = (y * ph / h, ((y + 1) * ph / h).max(y * ph / h + 1).min(ph));
        for x in 0..w {
            let (x0, x1) = (x * pw / w, ((x + 1) * pw / w).max(x * pw / w + 1).min(pw));
            let mut acc = [0f32; 4];
            let n = ((y1 - y0) * (x1 - x0)) as f32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let px = match &pic.data {
                        PixelData::Yuv { y: yp, u, v, chroma_width, .. } => {
                            let cw = *chroma_width as usize;
                            let xdiv = if cw >= pw { 1 } else { 2 };
                            let ydiv = if (yp.len() / pw.max(1)) <= (u.len() / cw.max(1)) { 1 } else { 2 };
                            let ci = (sy / ydiv) * cw + sx / xdiv;
                            let (m1, m2) = (max(yp), max(u));
                            let yy = (yp.get(sy * pw + sx) / m1 - 16.0 / 255.0) * (255.0 / 219.0);
                            let (cb, cr) = ((u.get(ci) / m2 - 0.5) * (255.0 / 224.0), (v.get(ci) / m2 - 0.5) * (255.0 / 224.0));
                            [yy + 1.5748 * cr, yy - 0.1873 * cb - 0.4681 * cr, yy + 1.8556 * cb, 1.0]
                        }
                        PixelData::Rgba8(p) => {
                            let i = (sy * pw + sx) * 4;
                            [p[i] as f32 / 255.0, p[i + 1] as f32 / 255.0, p[i + 2] as f32 / 255.0, p[i + 3] as f32 / 255.0]
                        }
                    };
                    for c in 0..4 {
                        acc[c] += px[c];
                    }
                }
            }
            let i = (y * w + x) * 4;
            for c in 0..4 {
                out[i + c] = ((acc[c] / n).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
    Frame { width: w, height: h, rgba: out }
}

impl RgbaFrames {
    pub(crate) fn open(f: &Footage, size: (usize, usize), range: &Range<u64>) -> Result<RgbaFrames, String> {
        Ok(RgbaFrames { src: studio_render::source::open_path(&f.path)?, size, cache: Default::default(), base: range.start, count: (range.end - range.start) as usize, fps: f.fps })
    }

    pub(crate) fn get(&mut self, i: u64) -> Result<Frame, String> {
        if let Some(f) = self.cache.get(&i) {
            return Ok(f.clone());
        }
        let (w, h) = self.size;
        let lo = i.saturating_sub(23);
        let mut got = std::collections::BTreeMap::new();
        self.src.decode_to(i, &mut |k, pic| {
            if k >= lo && k <= i {
                got.insert(k, rgba(&pic, w, h));
            }
        });
        self.cache.extend(got);
        while self.cache.len() > 64 {
            let far = *self.cache.keys().max_by_key(|k| k.abs_diff(i)).expect("not empty");
            self.cache.remove(&far);
        }
        self.cache.get(&i).cloned().ok_or_else(|| format!("frame {i} could not be decoded"))
    }
}

impl studio_ai::frame::FrameSource for RgbaFrames {
    fn frame_count(&self) -> usize {
        self.count
    }
    fn fps(&self) -> f64 {
        self.fps
    }
    fn frame(&mut self, index: usize) -> studio_ai::Result<Frame> {
        self.get(self.base + index as u64).map_err(studio_ai::Error::Io)
    }
}

/// The picture behind a clip: footage, or a still (a photo or a
/// generated picture: one frame, cut out like any other; the tracker's
/// loader refuses stills because they have nothing to track).
fn footage(proj: &view::Project, mc: &m::Clip) -> Result<Footage, String> {
    if let m::ClipSource::Media(r) = &mc.source {
        if let Some(mi) = proj.media(r.media.0 as u32).filter(|mi| mi.kind == view::MediaKind::Image) {
            let path = mi.path.clone().ok_or("sample media has no file to cut out")?;
            return Ok(Footage { path, fps: 30.0, frames: 1, width: mi.width.max(1), height: mi.height.max(1) });
        }
    }
    tracking::footage(proj, mc)
}

/// Where cut-out files go: the project's `Cut-outs` folder, or Studio's own
/// for a project not saved yet.
pub fn cut_dir(doc: &crate::doc::Doc) -> PathBuf {
    // Tests never write into the person's Studio folder.
    let home = if cfg!(test) { std::env::temp_dir().join("studio-tests") } else { studio_paths::studio() };
    doc.folder.as_ref().map(|f| f.root().join("Cut-outs")).unwrap_or_else(|| home.join("Cut-outs"))
}

/// Canvas points at timeline time `t` → pixels of the matte frame.
fn to_matte(proj: &view::Project, model: &m::Project, id: u32, t: Time, size: (usize, usize), c: Vec2) -> Result<(f32, f32), String> {
    let geo = xform::geo(proj, model, id, t).ok_or("the clip draws nothing there")?;
    let s = geo.to_source(c).ok_or("that point is not on the clip's picture")?;
    let src = geo.source();
    let (x, y) = (s.x / src.x * size.0 as f32, s.y / src.y * size.1 as f32);
    if x < 0.0 || y < 0.0 || x > size.0 as f32 || y > size.1 as f32 {
        return Err("that point is off the clip's picture".into());
    }
    Ok((x, y))
}

/// What a selection on the canvas says: clicks in (`inside`) and out of
/// (`outside`) the object, or a box (two corners), canvas pixels at `t`.
#[derive(Clone, Debug, Default)]
pub struct Pick {
    pub inside: Vec<Vec2>,
    pub outside: Vec<Vec2>,
    pub corners: Option<[Vec2; 2]>,
}

impl Pick {
    pub fn is_empty(&self) -> bool {
        self.inside.is_empty() && self.corners.is_none()
    }
}

/// Sets up a cut-out (or roto) of clip `id`: `pick` `None` for people.
pub fn plan(doc: &crate::doc::Doc, id: u32, pick: Option<(&Pick, Time)>, smoothing: f32, step: u64) -> Result<CutJob, String> {
    let (proj, model) = (&doc.project, &doc.model);
    let mc = model.active().clip(m::ClipId(id as u64)).ok_or_else(|| format!("no clip {id}"))?;
    let m::ClipSource::Media(r) = &mc.source else { return Err("only a video clip can be cut out".into()) };
    let f = footage(proj, mc)?;
    let range = tracking::used_frames(mc, &f);
    let size = matte_size(&f);
    let subject = match pick {
        None => Subject::People,
        Some((p, t)) => {
            let vc = proj.clip(id).ok_or("no such clip")?;
            if t < vc.start || t >= vc.start + vc.dur {
                return Err("pick the object at a moment the clip is on screen".into());
            }
            if p.is_empty() {
                return Err("say what to cut out: click on it (points) or draw a box around it".into());
            }
            let mut prompts = Vec::new();
            for c in &p.inside {
                let (x, y) = to_matte(proj, model, id, t, size, *c)?;
                prompts.push(Prompt::Point { x, y, positive: true });
            }
            for c in &p.outside {
                let (x, y) = to_matte(proj, model, id, t, size, *c)?;
                prompts.push(Prompt::Point { x, y, positive: false });
            }
            if let Some([a, b]) = p.corners {
                let (x0, y0) = to_matte(proj, model, id, t, size, a)?;
                let (x1, y1) = to_matte(proj, model, id, t, size, b)?;
                prompts.push(Prompt::Box { x0, y0, x1, y1 });
            }
            let at = (tracking::frame_at(mc, f.fps, t).round() as u64).clamp(range.start, range.end - 1);
            Subject::Object { at, prompts }
        }
    };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let file = cut_dir(doc).join(format!("clip-{id}-{stamp}.smatte"));
    Ok(CutJob { clip: id, media: r.media.0, footage: f, range, subject, size, file, smoothing: smoothing.clamp(0.0, 1.0), step: step.max(1) })
}

/// The picture of clip `id` at timeline time `at` as RGBA8 (sRGB), its
/// longer side at most `max_side` pixels: for reading a clip's colours
/// (a backdrop to key, a colour to match) without rendering the project.
pub fn clip_frame(doc: &crate::doc::Doc, id: u32, at: Time, max_side: usize) -> Result<(usize, usize, Vec<u8>), String> {
    let (proj, model) = (&doc.project, &doc.model);
    let mc = model.active().clip(m::ClipId(id as u64)).ok_or_else(|| format!("no clip {id}"))?;
    if !matches!(mc.source, m::ClipSource::Media(_)) {
        return Err(format!("clip {id} is not footage or a picture"));
    }
    let f = footage(proj, mc)?;
    let range = tracking::used_frames(mc, &f);
    let scale = (max_side.max(32) as f64 / f.width.max(f.height).max(1) as f64).min(1.0);
    let size = (((f.width as f64 * scale).round() as usize).max(2), ((f.height as f64 * scale).round() as usize).max(2));
    let index = (tracking::frame_at(mc, f.fps, at).round().max(0.0) as u64).clamp(range.start, range.end.saturating_sub(1).max(range.start));
    let frame = RgbaFrames::open(&f, size, &range)?.get(index)?;
    Ok((frame.width, frame.height, frame.rgba.clone()))
}

/// Models the job needs, checked before it starts (a clear message, not a
/// half-done job).
pub fn models_ready(j: &CutJob) -> Result<(), String> {
    let store = ModelStore::default();
    let id = match j.subject {
        Subject::People => studio_ai::ModelId::Modnet,
        Subject::Object { .. } => studio_ai::ModelId::Sam21Tiny,
    };
    store.require(id).map(|_| ()).map_err(|e| e.to_string())
}

impl CutJob {
    /// One source frame at the matte size (for a preview of a pick).
    pub fn frame(&self, index: u64) -> Result<Frame, String> {
        RgbaFrames::open(&self.footage, self.size, &self.range)?.get(index)
    }

    /// Mattes every frame into the matte file.
    pub fn run_matte(&self, ctx: &JobCtx) -> Result<CutResult, String> {
        let mut frames = RgbaFrames::open(&self.footage, self.size, &self.range)?;
        let n = (self.range.end - self.range.start) as usize;
        let (w, h) = self.size;
        let mut masks: Vec<Option<Mask>> = vec![None; n];
        let store = ModelStore::default();
        match &self.subject {
            Subject::People => {
                let mut bg = BackgroundRemover::load(&store).map_err(|e| e.to_string())?;
                bg.settings.temporal_smoothing = self.smoothing;
                for i in 0..n {
                    ctx.check().map_err(|e| e.to_string())?;
                    ctx.set_progress(i as f32 / n as f32);
                    let f = frames.get(self.range.start + i as u64)?;
                    masks[i] = Some(bg.next_frame(&f).map_err(|e| e.to_string())?);
                }
            }
            Subject::Object { at, prompts } => {
                let sam = ObjectCutout::load(&store).map_err(|e| e.to_string())?;
                let mut smoother = MatteSmoother::default();
                let start = (*at - self.range.start) as usize;
                let mut done = 0usize;
                sam.track_clip(&mut frames, start, prompts, &JobCtx::detached(), |i, sel| {
                    done += 1;
                    ctx.set_progress(done as f32 / n as f32);
                    ctx.check()?;
                    masks[i] = Some(if sel.present { sel.mask.clone() } else { Mask::zeros(sel.mask.width, sel.mask.height) });
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
                // Edges against the picture, and a touch of smoothing over
                // time, in frame order.
                for i in 0..n {
                    if let Some(mk) = masks[i].take() {
                        let f = frames.get(self.range.start + i as u64)?;
                        let refined = refine_edges(&f, &mk);
                        masks[i] = Some(smoother.push(&f, refined, self.smoothing * 0.5));
                    }
                }
            }
        }
        let mut wr = matte_file::Writer::create(&self.file, w as u32, h as u32, self.range.start).map_err(|e| e.to_string())?;
        let (mut cov, mut empty) = (0.0f32, 0usize);
        for mk in &masks {
            let mk = mk.clone().unwrap_or_else(|| Mask::zeros(w, h));
            let c = mk.coverage();
            cov += c;
            if c < 1e-4 {
                empty += 1;
            }
            wr.push(&mk).map_err(|e| e.to_string())?;
        }
        wr.finish().map_err(|e| e.to_string())?;
        Ok(CutResult { frames: n, coverage: cov / n.max(1) as f32, empty })
    }

    /// The object's outline on every `step`-th frame (roto), `points` a
    /// frame, in matte pixels, by source frame.
    pub fn run_roto(&self, points: usize, ctx: &JobCtx) -> Result<Vec<(u64, Vec<(f32, f32)>)>, String> {
        let Subject::Object { at, prompts } = &self.subject else { return Err("roto follows an object picked with clicks or a box".into()) };
        let store = ModelStore::default();
        let sam = ObjectCutout::load(&store).map_err(|e| e.to_string())?;
        let mut frames = RgbaFrames::open(&self.footage, self.size, &self.range)?;
        // The frames SAM looks at: every step-th, from the picked frame out.
        let step = self.step.max(1);
        let mut picks: Vec<u64> = Vec::new();
        let mut f = *at;
        while f < self.range.end {
            picks.push(f);
            f += step;
        }
        if picks.last() != Some(&(self.range.end - 1)) {
            picks.push(self.range.end - 1);
        }
        let mut back = Vec::new();
        let mut f = *at;
        while f >= self.range.start + step {
            f -= step;
            back.push(f);
        }
        if *at > self.range.start && back.last() != Some(&self.range.start) {
            back.push(self.range.start);
        }
        let total = picks.len() + back.len();
        let mut found: std::collections::BTreeMap<u64, Option<Mask>> = Default::default();
        let first = sam.cut_out(&frames.get(*at)?, prompts).map_err(|e| e.to_string())?;
        found.insert(*at, first.present.then(|| first.mask.clone()));
        let mut done = 1;
        for seq in [&picks[1..], &back[..]] {
            let mut tracker = Tracker::new(&first);
            for &fr in seq {
                ctx.check().map_err(|e| e.to_string())?;
                ctx.set_progress(done as f32 / total as f32);
                let img = frames.get(fr)?;
                let sel = match tracker.prompts() {
                    Some(p) => {
                        let s = sam.cut_out(&img, &p).map_err(|e| e.to_string())?;
                        tracker.update(&s);
                        s.present.then_some(s.mask)
                    }
                    None => None,
                };
                found.insert(fr, sel);
                done += 1;
            }
        }
        let keys: Vec<u64> = found.keys().copied().collect();
        let masks: Vec<Option<Mask>> = found.into_values().collect();
        let mut outlines = roto::outlines(&masks, points);
        roto::smooth(&mut outlines, if step == 1 { 1.0 } else { 0.6 });
        Ok(keys.into_iter().zip(outlines).filter_map(|(k, o)| Some((k, o?))).collect())
    }
}

/// The clip's cut-out, as stored.
pub fn matte_of(mc: &m::Clip) -> Option<serde_json::Map<String, Value>> {
    mc.meta.get("matte").and_then(Value::as_object).cloned()
}

/// The clip with a finished cut-out on it.
pub fn store_matte(mc: &mut m::Clip, j: &CutJob, r: &CutResult, invert: bool) {
    let method = match j.subject {
        Subject::People => "people",
        Subject::Object { .. } => "object",
    };
    mc.meta.insert(
        "matte".into(),
        json!({"file": j.file.to_string_lossy(), "media": j.media, "method": method, "first": j.range.start, "frames": r.frames, "width": j.size.0, "height": j.size.1, "invert": invert, "coverage": (r.coverage * 1000.0).round() / 1000.0, "empty": r.empty}),
    );
}

/// Sets `key` of the clip's cut-out (`invert`, `enabled`).
pub fn set_flag(mc: &mut m::Clip, key: &str, on: bool) -> bool {
    match mc.meta.get_mut("matte").and_then(Value::as_object_mut) {
        Some(o) => {
            o.insert(key.into(), json!(on));
            true
        }
        None => false,
    }
}

/// Plain words about a cut-out.
pub fn describe(o: &serde_json::Map<String, Value>) -> String {
    let frames = o.get("frames").and_then(Value::as_u64).unwrap_or(0);
    let empty = o.get("empty").and_then(Value::as_u64).unwrap_or(0);
    let cov = o.get("coverage").and_then(Value::as_f64).unwrap_or(0.0);
    let what = if o.get("method").and_then(Value::as_str) == Some("people") { "people" } else { "the object" };
    let mut s = format!("cut out {what} over {frames} frames, keeping {:.0}% of the picture on average", cov * 100.0);
    if empty > 0 {
        s += &format!("; nothing found on {empty} frame(s) (they show nothing)");
    }
    if o.get("invert").and_then(Value::as_bool) == Some(true) {
        s += "; inverted (the subject is removed, the rest kept)";
    }
    if o.get("enabled").and_then(Value::as_bool) == Some(false) {
        s += "; off";
    }
    s
}

/// The clip with a roto mask from `outlines` (matte pixels by source
/// frame): a path mask with a key on each point at every outline's moment.
pub fn store_roto(proj: &view::Project, model: &m::Project, id: u32, j: &CutJob, outlines: &[(u64, Vec<(f32, f32)>)], feather: f64, invert: bool) -> Result<(m::Clip, u64), String> {
    let mut mc = model.active().clip(m::ClipId(id as u64)).ok_or("the clip is gone")?.clone();
    let vc = proj.clip(id).ok_or("the clip is gone")?.clone();
    if outlines.is_empty() {
        return Err("the object was not found on any frame: pick it again with more clicks, or a box".into());
    }
    let (w, h) = (j.size.0 as f64, j.size.1 as f64);
    // Layer coordinates (0..1 across the picture after crop) at clip time.
    let to_layer = |p: (f32, f32), local: m::Micros| {
        let c = mc.transform_at(local).crop;
        let (u, v) = (p.0 as f64 / w, p.1 as f64 / h);
        m::Vec2::new((u - c.left) / (1.0 - c.left - c.right).max(1e-6), (v - c.top) / (1.0 - c.top - c.bottom).max(1e-6))
    };
    let mid = mc.masks.iter().map(|k| k.id.0).max().map_or(1, |x| x + 1);
    let mut keys: Vec<Vec<m::Keyframe>> = vec![Vec::new(); outlines[0].1.len()];
    for (fr, pts) in outlines {
        let Some(t) = tracking::time_of_frame(proj, &mc, j.footage.fps, *fr) else { continue };
        let local = t - vc.start;
        for (k, p) in pts.iter().enumerate() {
            keys[k].push(m::Keyframe::new(local, m::Value::Point(to_layer(*p, local)), m::Interp::Linear));
        }
    }
    if keys[0].is_empty() {
        return Err("none of the tracked frames is on the timeline".into());
    }
    let mut mask = m::Mask::new(m::MaskId(mid), m::MaskShape::Path);
    mask.path = keys.iter().map(|k| m::PathPoint { point: match &k[0].value { m::Value::Point(p) => *p, _ => m::Vec2::new(0.5, 0.5) }, handle_in: m::Vec2::ZERO, handle_out: m::Vec2::ZERO }).collect();
    mask.invert = invert;
    mask.params.insert("feather".into(), m::Value::Scalar(feather));
    mask.extra.insert("roto".into(), json!(true));
    mc.masks.push(mask);
    for (k, ks) in keys.into_iter().enumerate() {
        if ks.len() > 1 {
            mc.keyframes.insert(format!("mask.{mid}.point.{k}"), m::Track::new(ks));
        }
    }
    Ok((mc, mid))
}

/// Registers every clip's cut-out matte with the renderer's sources: the
/// matte file read a frame at a time, its value in the red channel.
pub fn register_mattes(model: &m::Project, registry: &studio_render::source::SourceRegistry) {
    use studio_render::source::{ProceduralSource, SourceSpec};
    use studio_render::yuv::DecodedPicture;
    for seq in &model.sequences {
        for c in &seq.clips {
            let Some(o) = matte_of(c) else { continue };
            let Some(file) = o.get("file").and_then(Value::as_str) else { continue };
            let key = studio_render::scene::matte_key(file);
            if registry.spec(key).is_some() {
                continue;
            }
            let Ok(r) = matte_file::Reader::open(Path::new(file)) else { continue };
            let (w, h, first, n) = (r.width, r.height, r.first, r.frames());
            let reader = std::sync::Mutex::new(r);
            let source = ProceduralSource::new(w, h, 30.0, first + n, move |i| {
                let plane = reader.lock().ok().and_then(|mut r| r.plane(i).ok()).unwrap_or_else(|| vec![255; (w * h) as usize]);
                let px: Vec<u8> = plane.iter().flat_map(|&v| [v, v, v, 255]).collect();
                DecodedPicture::from_rgba8(w, h, px)
            });
            registry.register(key, SourceSpec::Procedural(source));
        }
    }
}

/// Seconds a clip spends on screen (for messages).
pub fn seconds(t: Time) -> f64 {
    t as f64 / SEC as f64
}
