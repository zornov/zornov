// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The visual effects department, shared by the compositor panel and the
//! AI: node graphs on clips (a `comp` effect holding a
//! `studio_fx::comp::Graph`), camera solves (matchmove) that become Studio
//! cameras, clean plates and depth passes baked to files the graphs read.
//!
//! A clip's graph sees the clip's own picture (source pixels) at the
//! `input` node; on an adjustment clip it sees everything below (canvas
//! pixels). Coordinates in node data are in those pixels.

use std::ops::Range;
use std::path::{Path, PathBuf};

use serde_json::Value;
use studio_ai::JobCtx;
use studio_fx::comp::{Graph, InputMedia};
use studio_fx::solve::orient::{self, CameraFile, CameraKey, Frame3, OrientSettings};
use studio_fx::solve::{self, Solved, SolveSettings, TrackSettings};
use studio_model as m;

use super::tracking::{self, Footage};
use crate::doc::Doc;
use crate::view::{self, SEC, Time};

/// The effect kind holding a graph.
pub const KIND: &str = studio_render::comp::KIND;

/// The model clip `id`.
pub fn model_clip(doc: &Doc, id: u32) -> Option<m::Clip> {
    doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).cloned()
}

/// A clip's compositing graph and the effect that holds it.
pub fn graph_of(mc: &m::Clip) -> Option<(m::EffectId, Graph)> {
    let e = mc.effects.iter().find(|e| e.kind == KIND)?;
    let text = match e.params.get("graph") {
        Some(m::Value::Text(t)) => t.as_str(),
        _ => "",
    };
    Some((e.id, if text.is_empty() { Graph::passthrough() } else { Graph::from_json(text).ok()? }))
}

/// Writes `g` as clip `id`'s graph (adding the comp effect, last in the
/// stack, when the clip has none), as one undo step.
pub fn store_graph(doc: &mut Doc, id: u32, g: &Graph, label: &str) -> Result<(), String> {
    g.check()?;
    let mc = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let seq = doc.seq();
    let text = m::Value::Text(g.to_json());
    let ok = match mc.effects.iter().find(|e| e.kind == KIND) {
        Some(e) => {
            let mut e = e.clone();
            e.params.insert("graph".into(), text);
            doc.apply(label, m::Edit::SetEffect { seq, clip: m::ClipId(id as u64), effect: e })
        }
        None => {
            let mut e = m::Effect::new(m::EffectId(doc.model.alloc_id()), KIND);
            e.params.insert("graph".into(), text);
            doc.apply(label, m::Edit::AddEffect { seq, clip: m::ClipId(id as u64), stack: m::Stack::Video, effect: e, index: None })
        }
    };
    if ok { Ok(()) } else { Err(doc.error.take().unwrap_or_else(|| "the graph could not be stored".into())) }
}

/// Like [`store_graph`] for a value being dragged: one undo step per
/// gesture.
pub fn store_graph_live(doc: &mut Doc, id: u32, g: &Graph, label: &str) -> Result<(), String> {
    g.check()?;
    let mc = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let Some(e) = mc.effects.iter().find(|e| e.kind == KIND) else { return store_graph(doc, id, g, label) };
    let mut e = e.clone();
    e.params.insert("graph".into(), m::Value::Text(g.to_json()));
    let seq = doc.seq();
    if doc.apply_gesture(&format!("comp-{id}"), label, m::Edit::SetEffect { seq, clip: m::ClipId(id as u64), effect: e }) { Ok(()) } else { Err(doc.error.take().unwrap_or_default()) }
}

/// Removes clip `id`'s graph.
pub fn remove_graph(doc: &mut Doc, id: u32) -> Result<(), String> {
    let mc = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let e = mc.effects.iter().find(|e| e.kind == KIND).ok_or_else(|| format!("clip {id} has no compositing graph"))?;
    let seq = doc.seq();
    if doc.apply("Remove comp", m::Edit::RemoveEffect { seq, clip: m::ClipId(id as u64), effect: e.id }) { Ok(()) } else { Err(doc.error.take().unwrap_or_default()) }
}

/// Inserts an adjustment clip on `lane` from `start` for `dur`: a comp
/// over everything below.
pub fn add_comp_layer(doc: &mut Doc, lane: u32, start: Time, dur: Time, name: &str) -> Result<u32, String> {
    let id = doc.alloc_id() as u32;
    let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), start, dur, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)));
    c.name = name.to_owned();
    let seq = doc.seq();
    if !doc.apply("Comp layer", m::Edit::InsertClip { seq, clip: Box::new(c) }) {
        return Err(doc.error.take().unwrap_or_else(|| "the comp layer could not be added".into()));
    }
    store_graph(doc, id, &Graph::passthrough(), "Comp layer")?;
    Ok(id)
}

/// The clip's footage behind its graph's `input` node, so nodes that look
/// at other frames can read them (constant-speed media clips).
pub fn input_media(doc: &Doc, mc: &m::Clip) -> Option<InputMedia> {
    let m::ClipSource::Media(r) = &mc.source else { return None };
    let mi = doc.project.media(r.media.0 as u32)?;
    let path = mi.path.clone()?;
    let speed = match &mc.speed {
        m::Speed::Constant(s) => *s,
        _ => return None,
    };
    if mc.reversed {
        return None;
    }
    Some(InputMedia { path: path.to_string_lossy().into_owned(), start: mc.source_in as f64 / SEC as f64, speed })
}

/// Size of the picture a clip's graph works on: the footage's for media
/// clips, the canvas's for everything else.
pub fn picture_size(doc: &Doc, mc: &m::Clip) -> (f64, f64) {
    if let m::ClipSource::Media(r) = &mc.source {
        if let Some(mi) = doc.project.media(r.media.0 as u32) {
            if mi.width > 0 && mi.height > 0 {
                return (mi.width as f64, mi.height as f64);
            }
        }
    }
    (doc.project.width as f64, doc.project.height as f64)
}

// ------------------------------------------------------------ matte files

/// A cut-out matte or depth pass (`.smatte`) as a picture source for
/// graphs: grey, one frame per footage frame. The frame rate comes from
/// the path's `#fps=` suffix.
struct MatteSource {
    reader: studio_ai::features::matte_file::Reader,
    info: studio_render::source::SourceInfo,
    fps: f64,
}

impl studio_render::source::MediaSource for MatteSource {
    fn info(&self) -> studio_render::source::SourceInfo {
        self.info.clone()
    }

    fn frame_index(&self, time: i64) -> u64 {
        let i = (time.max(0) as f64 / SEC as f64 * self.fps).round() as u64;
        i.clamp(self.reader.first, (self.reader.first + self.reader.frames()).saturating_sub(1).max(self.reader.first))
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, studio_render::yuv::PictureRef)) -> bool {
        let (w, h) = (self.reader.width, self.reader.height);
        let Ok(plane) = self.reader.plane(index) else { return false };
        let px: Vec<u8> = plane.iter().flat_map(|&v| [v, v, v, 255]).collect();
        emit(index, std::sync::Arc::new(studio_render::yuv::DecodedPicture::from_rgba8(w, h, px)));
        true
    }
}

fn open_smatte(p: &Path) -> Option<Box<dyn studio_render::source::MediaSource>> {
    let s = p.to_string_lossy();
    let (file, opts) = s.split_once('#').unwrap_or((&s, ""));
    if !file.ends_with(".smatte") {
        return None;
    }
    let fps = opts.split('&').find_map(|kv| kv.strip_prefix("fps=")).and_then(|v| v.parse::<f64>().ok()).unwrap_or(30.0).max(1.0);
    let reader = studio_ai::features::matte_file::Reader::open(Path::new(file)).ok()?;
    let info = studio_render::source::SourceInfo {
        width: reader.width,
        height: reader.height,
        rotation: 0,
        frame_rate: Some(fps),
        duration: None,
        frame_count: Some(reader.first + reader.frames()),
        colour: studio_render::colour_space::ColourDesc::SRGB8,
    };
    Some(Box::new(MatteSource { reader, info, fps }))
}

/// Lets graphs read Studio's matte files. Call once at start-up.
pub fn register() {
    studio_render::comp::add_opener(open_smatte);
}

/// A matte file as a `read` path at a footage frame rate.
pub fn matte_path(file: &str, fps: f64) -> String {
    format!("{file}#fps={fps}")
}

/// The clip's cut-out matte file as a `read` path, if it has one.
pub fn cut_out_path(doc: &Doc, mc: &m::Clip) -> Option<String> {
    let o = super::cutout::matte_of(mc)?;
    let file = o.get("file")?.as_str()?.to_owned();
    let ft = tracking::footage(&doc.project, mc).ok()?;
    Some(matte_path(&file, ft.fps))
}

// ------------------------------------------------------------- footage

/// The source frames clip `mc` shows.
pub fn frame_range(mc: &m::Clip, fps: f64) -> Range<u64> {
    let a = tracking::frame_at(mc, fps, mc.start).floor().max(0.0) as u64;
    let b = tracking::frame_at(mc, fps, mc.start + mc.duration - 1).ceil().max(0.0) as u64;
    a.min(b)..a.max(b) + 1
}

/// [`frame_range`] kept inside the footage.
fn frames_in(mc: &m::Clip, ft: &Footage) -> Range<u64> {
    let r = frame_range(mc, ft.fps);
    r.start.min(ft.frames.saturating_sub(1))..r.end.min(ft.frames).max(r.start.min(ft.frames.saturating_sub(1)) + 1)
}

/// Clip time (from the clip start) at which footage frame `f` shows.
pub fn clip_time_of_frame(doc: &Doc, mc: &m::Clip, fps: f64, f: u64) -> Option<Time> {
    match &mc.speed {
        m::Speed::Constant(s) if !mc.reversed && mc.keyframes.get(m::clip::TIME_REMAP).is_none() => {
            let src = f as f64 / fps * SEC as f64;
            let rel = ((src - mc.source_in as f64) / s.max(1e-6)).round() as Time;
            (0..mc.duration).contains(&rel).then_some(rel)
        }
        _ => tracking::time_of_frame(&doc.project, mc, fps, f).map(|t| t - mc.start),
    }
}

fn analysis(ft: &Footage, max_side: usize) -> (usize, usize) {
    let (w, h) = (ft.width.max(1) as usize, ft.height.max(1) as usize);
    let k = (max_side as f64 / w.max(h) as f64).min(1.0);
    (((w as f64 * k).round() as usize).max(16), ((h as f64 * k).round() as usize).max(16))
}

/// A matte file's plane resampled to `w`×`h` (0..1).
fn matte_plane(r: &mut studio_ai::features::matte_file::Reader, frame: u64, w: usize, h: usize) -> Option<Vec<f32>> {
    let p = r.plane(frame).ok()?;
    let (mw, mh) = (r.width as usize, r.height as usize);
    Some((0..w * h).map(|i| {
        let (x, y) = (i % w, i / w);
        p[(y * mh / h).min(mh - 1) * mw + (x * mw / w).min(mw - 1)] as f32 / 255.0
    }).collect())
}

// ------------------------------------------------------------- matchmove

/// What a camera solve found.
pub struct SolveRun {
    pub solved: Solved,
    pub frame3: Frame3,
    pub keys: Vec<CameraKey>,
    pub tracks: usize,
    pub footage: Footage,
    pub range: Range<u64>,
    /// Canvas pixels per footage pixel.
    pub zoom_scale: f64,
}

/// Solves the camera of video clip `id`. `ignore_cut_out` skips corners on
/// the clip's cut-out (moving people).
pub fn solve_clip(doc: &Doc, id: u32, settings: SolveSettings, orient_settings: OrientSettings, ignore_cut_out: bool, ctx: &JobCtx) -> Result<SolveRun, String> {
    let mc = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let ft = tracking::footage(&doc.project, &mc)?;
    let range = frames_in(&mc, &ft);
    let n = (range.end - range.start) as usize;
    if n < 8 {
        return Err("the clip is too short to solve a camera (it needs a moving shot of at least 8 frames)".into());
    }
    let size = analysis(&ft, 960);
    let k = ft.width as f64 / size.0 as f64;
    let mut frames = super::cutout::RgbaFrames::open(&ft, size, &range)?;
    let mut matte = if ignore_cut_out {
        super::cutout::matte_of(&mc).and_then(|o| o.get("file").and_then(Value::as_str).and_then(|f| studio_ai::features::matte_file::Reader::open(Path::new(f)).ok()))
    } else {
        None
    };
    let mut failed = None;
    let tracks = solve::track_features(
        n,
        |i| {
            let f = match frames.get(range.start + i as u64) {
                Ok(f) => f,
                Err(e) => {
                    failed = Some(e);
                    return None;
                }
            };
            let img = studio_fx::Image::from_srgb8(f.width, f.height, &f.rgba);
            let ignore = matte.as_mut().and_then(|r| matte_plane(r, range.start + i as u64, f.width, f.height));
            Some((img, ignore))
        },
        TrackSettings { features: 300, max_width: 960, min_length: 6 },
        |i| {
            ctx.set_progress(0.7 * i as f32 / n as f32);
            ctx.check().is_ok()
        },
    );
    if let Some(e) = failed {
        return Err(e);
    }
    ctx.check().map_err(|e| e.to_string())?;
    // Back to footage pixels.
    let tracks: Vec<_> = tracks
        .into_iter()
        .map(|mut t| {
            for p in &mut t.points {
                p.1 = [p.1[0] * k, p.1[1] * k];
            }
            t
        })
        .collect();
    let mut settings = settings;
    if let Some(f) = settings.focal.as_mut() {
        *f = f.max(1.0);
    }
    let solved = solve::solve(&tracks, ft.width as f64, ft.height as f64, n, settings)?;
    ctx.set_progress(0.95);
    let (cw, ch) = (doc.project.width as f64, doc.project.height as f64);
    // The footage fits the canvas (as a media clip does at rest).
    let zoom_scale = (cw / ft.width as f64).min(ch / ft.height as f64) * mc.transform.scale_x;
    let frame3 = orient::orient(&solved, &OrientSettings { canvas_width: cw, ..orient_settings });
    let keys = orient::camera_keys(&solved, &frame3, zoom_scale);
    Ok(SolveRun { solved, frame3, keys, tracks: tracks.len(), footage: ft, range, zoom_scale })
}

/// Studio's exchange file for a solve.
pub fn camera_file(run: &SolveRun) -> CameraFile {
    CameraFile {
        format: "studio-camera-1".into(),
        width: run.footage.width as f64,
        height: run.footage.height as f64,
        frame_rate: run.footage.fps,
        focal_pixels: run.solved.focal,
        error_pixels: (run.solved.error * 1000.0).round() / 1000.0,
        nodal: run.solved.nodal,
        keys: run.keys.clone(),
        points: run.solved.points.iter().map(|p| {
            let w = run.frame3.to_world(p.position);
            [w[0], w[1], w[2], p.track as f64]
        }).collect(),
    }
}

/// Where camera solves are kept: the project's `Cameras` folder, or
/// Studio's own folder.
pub fn cameras_dir(doc: &Doc) -> PathBuf {
    // Tests never write into the person's Studio folder.
    let home = if cfg!(test) { std::env::temp_dir().join("studio-tests") } else { studio_paths::studio() };
    let d = doc.folder.as_ref().map(|f| f.generated_dir(m::file::MadeKind::Cameras)).unwrap_or_else(|| home.join("Cameras"));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Puts camera keys on a camera clip: position, rotation and zoom per
/// footage frame (clip times from footage clip `mc`).
pub fn key_camera(doc: &Doc, cam: &mut m::Clip, footage_clip: &m::Clip, fps: f64, range: &Range<u64>, keys: &[CameraKey]) {
    let mut pos = Vec::new();
    let mut z = Vec::new();
    let mut rx = Vec::new();
    let mut ry = Vec::new();
    let mut rz = Vec::new();
    let mut zoom = Vec::new();
    for k in keys {
        let f = range.start + k.frame as u64;
        let Some(rel) = clip_time_of_frame(doc, footage_clip, fps, f) else { continue };
        // The camera clip starts where the footage clip does.
        let t = rel + footage_clip.start - cam.start;
        if t < 0 || t >= cam.duration {
            continue;
        }
        let key = |v: m::Value| m::Keyframe::new(t, v, m::Interp::Linear);
        pos.push(key(m::Value::Point(m::Vec2::new(k.position[0], k.position[1]))));
        z.push(key(m::Value::Scalar(k.position[2])));
        rx.push(key(m::Value::Angle(k.rotation[0])));
        ry.push(key(m::Value::Angle(k.rotation[1])));
        rz.push(key(m::Value::Angle(k.rotation[2])));
        zoom.push(key(m::Value::Scalar(k.zoom)));
    }
    if let Some(first) = keys.first() {
        cam.transform.position = m::Vec2::new(first.position[0], first.position[1]);
        cam.transform.position_z = first.position[2];
        (cam.transform.rotation_x, cam.transform.rotation_y, cam.transform.rotation) = (first.rotation[0], first.rotation[1], first.rotation[2]);
        cam.transform.orientation = [0.0; 3];
        if let m::ClipSource::Generator(g) = &mut cam.source {
            g.params.insert("zoom".into(), m::Value::Scalar(first.zoom));
            g.params.insert("two_node".into(), m::Value::Bool(false));
        }
    }
    for (path, keys) in [("transform.position", pos), ("transform.position_z", z), ("transform.rotation_x", rx), ("transform.rotation_y", ry), ("transform.rotation", rz), ("generator.zoom", zoom)] {
        if keys.len() > 1 {
            cam.keyframes.insert(path.into(), m::Track::new(keys));
        }
    }
}

/// Where a ray from `origin` along `dir` meets the ground (world y = 0),
/// if ahead.
pub fn hit_ground(origin: [f64; 3], dir: [f64; 3]) -> Option<[f64; 3]> {
    if dir[1].abs() < 1e-9 {
        return None;
    }
    let t = -origin[1] / dir[1];
    (t > 0.0).then(|| [origin[0] + dir[0] * t, origin[1] + dir[1] * t, origin[2] + dir[2] * t])
}

// ------------------------------------------------------- clean plate, depth

/// Bakes a clean plate of clip `id`: the per-pixel median of `samples`
/// frames spread over the clip (aligned to the middle one when the camera
/// moves), ignoring its cut-out if it has one. Returns the PNG's path.
pub fn bake_clean_plate(doc: &Doc, id: u32, samples: usize, moving: bool, ctx: &JobCtx) -> Result<PathBuf, String> {
    let mc = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let ft = tracking::footage(&doc.project, &mc)?;
    let range = frames_in(&mc, &ft);
    let size = analysis(&ft, 1920);
    let mut frames = super::cutout::RgbaFrames::open(&ft, size, &range)?;
    let n = (range.end - range.start) as usize;
    let count = samples.clamp(3, 60).min(n);
    let mid = range.start + (n / 2) as u64;
    let reference = frames.get(mid)?;
    let ref_img = studio_fx::Image::from_srgb8(reference.width, reference.height, &reference.rgba);
    let (w, h) = (ref_img.width, ref_img.height);
    let mut matte = super::cutout::matte_of(&mc).and_then(|o| o.get("file").and_then(Value::as_str).and_then(|f| studio_ai::features::matte_file::Reader::open(Path::new(f)).ok()));
    let mut warped: Vec<(studio_fx::Image, Vec<f32>)> = Vec::new();
    let ref_hole = matte.as_mut().and_then(|r| matte_plane(r, mid, w, h)).unwrap_or_else(|| vec![0.0; w * h]);
    for k in 0..count {
        ctx.check().map_err(|e| e.to_string())?;
        ctx.set_progress(k as f32 / count as f32);
        let f = range.start + ((k as f64 + 0.5) / count as f64 * n as f64) as u64;
        let fr = frames.get(f.min(range.end - 1))?;
        let img = studio_fx::Image::from_srgb8(fr.width, fr.height, &fr.rgba);
        let hole = matte.as_mut().and_then(|r| matte_plane(r, f, w, h)).unwrap_or_else(|| vec![0.0; w * h]);
        let m = if moving { studio_fx::comp::clean::align(&img, &ref_img, &ref_hole, Some(&hole)) } else { Some(studio_fx::comp::geo::IDENTITY) };
        let Some(m) = m else { continue };
        let Some(inv) = studio_fx::comp::geo::inverse3(&m) else { continue };
        let wimg = studio_fx::comp::geo::warp_h(&img, &inv, studio_fx::image::Edge::Transparent);
        let mut usable = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let s = studio_fx::comp::geo::apply_h(&inv, [x as f64 + 0.5, y as f64 + 0.5]);
                if s[0] < 0.5 || s[1] < 0.5 || s[0] > w as f64 - 0.5 || s[1] > h as f64 - 0.5 {
                    continue;
                }
                let blocked = studio_fx::comp::geo::sample_plane(&hole, w, h, s[0] as f32, s[1] as f32) > 0.1;
                usable[y * w + x] = if blocked { 0.0 } else { 1.0 };
            }
        }
        warped.push((wimg, usable));
    }
    if warped.is_empty() {
        return Err("no frame could be lined up with the middle of the clip".into());
    }
    let mut hole = vec![false; w * h];
    let out = studio_fx::Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let mut v: Vec<[f32; 4]> = warped.iter().filter(|(_, u)| u[i] > 0.5).map(|(img, _)| img.pixels[i]).collect();
        if v.is_empty() {
            return [0.0; 4];
        }
        let mut px = [0.0; 4];
        for (c, o) in px.iter_mut().enumerate() {
            v.sort_by(|a, b| a[c].total_cmp(&b[c]));
            *o = v[v.len() / 2][c];
        }
        px
    });
    for i in 0..w * h {
        hole[i] = warped.iter().all(|(_, u)| u[i] <= 0.5);
    }
    let out = if hole.iter().any(|&b| b) { studio_fx::comp::clean::inpaint_image(&out, &hole, 7, 4) } else { out };
    let dir = match &doc.folder {
        Some(f) => f.generated_dir(m::file::MadeKind::CleanPlates),
        None => cameras_dir(doc).with_file_name("Clean plates"),
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("clip-{id}-{}.png", stamp()));
    let png = studio_media::png::encode(&studio_media::RgbaImage { width: w as u32, height: h as u32, pixels: out.to_srgb8() });
    std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn stamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Bakes an AI depth pass of clip `id` (white near) to a matte file;
/// returns its `read` path.
pub fn bake_depth(doc: &Doc, id: u32, ctx: &JobCtx) -> Result<String, String> {
    let mc = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let ft = tracking::footage(&doc.project, &mc)?;
    let range = frames_in(&mc, &ft);
    let size = analysis(&ft, 960);
    let store = studio_ai::store::ModelStore::default();
    let mut est = studio_ai::features::depth::DepthEstimator::load(&store).map_err(|e| format!("the depth model is not available: {e}"))?;
    let mut frames = super::cutout::RgbaFrames::open(&ft, size, &range)?;
    let dir = match &doc.folder {
        Some(f) => f.generated_dir(m::file::MadeKind::Depth),
        None => cameras_dir(doc).with_file_name("Depth"),
    };
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("clip-{id}-{}.smatte", stamp()));
    let n = (range.end - range.start) as usize;
    let mut wr: Option<studio_ai::features::matte_file::Writer> = None;
    for i in 0..n {
        ctx.check().map_err(|e| e.to_string())?;
        ctx.set_progress(i as f32 / n as f32);
        let f = frames.get(range.start + i as u64)?;
        let d = est.next_frame(&f).map_err(|e| e.to_string())?;
        let w = match &mut wr {
            Some(w) => w,
            None => wr.insert(studio_ai::features::matte_file::Writer::create(&file, d.width as u32, d.height as u32, range.start).map_err(|e| e.to_string())?),
        };
        w.push(&d).map_err(|e| e.to_string())?;
    }
    wr.ok_or("the clip has no frames")?.finish().map_err(|e| e.to_string())?;
    Ok(matte_path(&file.to_string_lossy(), ft.fps))
}

/// Node kinds as compact lines for the AI: id, inputs, parameters with
/// ranges and defaults, data.
pub fn catalogue() -> String {
    use studio_fx::params::{ParamKind, ParamValue};
    let mut s = String::new();
    for cat in studio_fx::comp::NodeCategory::ALL {
        s.push_str(&format!("## {}\n", cat.label()));
        for d in studio_fx::comp::nodes::all().iter().filter(|d| d.category == cat) {
            let ps: Vec<String> = d
                .params
                .iter()
                .map(|p| {
                    let def = match p.default {
                        ParamValue::Float(v) => format!("{v}"),
                        ParamValue::Int(v) => format!("{v}"),
                        ParamValue::Bool(v) => format!("{v}"),
                        ParamValue::Choice(i) => match p.kind {
                            ParamKind::Choice(o) => format!("\"{}\"", o.get(i as usize).copied().unwrap_or("")),
                            _ => String::new(),
                        },
                        ParamValue::Point(v) => format!("[{}, {}]", v[0], v[1]),
                        ParamValue::Colour(c) => format!("[{:.2}, {:.2}, {:.2}, {:.2}]", c[0], c[1], c[2], c[3]),
                    };
                    let range = match p.kind {
                        ParamKind::Float { min, max } => format!(" {min}..{max}"),
                        ParamKind::Int { min, max } => format!(" {min}..{max}"),
                        ParamKind::Choice(o) => format!(" ({})", o.join(" | ")),
                        _ => String::new(),
                    };
                    format!("{}={def}{range}", p.id)
                })
                .collect();
            s.push_str(&format!("- {} [{}]: {}", d.id, d.inputs.join(", "), d.description));
            if !ps.is_empty() {
                s.push_str(&format!(" Params: {}.", ps.join("; ")));
            }
            if !d.data.is_empty() {
                s.push_str(&format!(" Data: {}.", d.data));
            }
            s.push('\n');
        }
    }
    s
}

#[allow(dead_code)]
fn _uses(_: &view::Project) {}
