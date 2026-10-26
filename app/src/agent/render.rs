// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Headless pictures for agents: one frame, or a contact sheet of many,
//! rendered by the same GPU compositor as the viewer and returned as JPEG
//! (small, so looking at the work stays cheap).

use crate::doc::Doc;
use crate::view::Time;

/// Columns of a contact sheet with `n` pictures.
use studio_media::rgba::RgbaImage;

/// Pictures for the AI to look at. Two levels:
///
/// * light (the default, for checks while working): small and compressed,
///   because every picture stays in the AI's context for the rest of the
///   job. A frame is 512 px wide at JPEG 70 and never more than
///   [`LIGHT_SIDE`] on its longer side; a contact sheet is 1024 px wide and
///   never wider than 1536. Several moments go into one contact sheet, not
///   many separate frames, and `region` looks closely at one part;
/// * full (`detail: "full"`, or `detail: true` as before): 1280 px frames,
///   1920 px sheets, JPEG 85, for judging how it looks (the final look
///   before delivering, small text, edges, colour).
///
/// Only the AI's view; renders and exports never come through here.
pub const LIGHT_QUALITY: u8 = 70;
pub const DETAIL_QUALITY: u8 = 85;
/// (light default, full default, light limit) widths.
pub const FRAME_WIDTH: (u64, u64, u64) = (512, 1280, 1024);
pub const SHEET_WIDTH: (u64, u64, u64) = (1024, 1920, 1536);
/// A light frame's longer side, whatever the canvas shape.
pub const LIGHT_SIDE: u64 = 1024;
/// The longer side of a `region` picture: native pixels up to this.
pub const REGION_SIDE: (u64, u64) = (1024, 1920);

/// Whether the call asks for full detail: `detail: "full"` or `true`.
pub fn is_full(a: &serde_json::Value) -> bool {
    match a.get("detail") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("full"),
        _ => false,
    }
}

/// (width, JPEG quality) for a picture tool's arguments. A light picture
/// asked wider than the light limit is brought down to it.
pub fn picture_size(a: &serde_json::Value, widths: (u64, u64, u64)) -> (u64, u8) {
    let full = is_full(a);
    let w = a.get("width").and_then(serde_json::Value::as_u64).unwrap_or(if full { widths.1 } else { widths.0 });
    (if full { w } else { w.min(widths.2) }, if full { DETAIL_QUALITY } else { LIGHT_QUALITY })
}

/// Width for a light frame of a `pw` x `ph` canvas: its longer side stays
/// within [`LIGHT_SIDE`] (a 9:16 canvas is limited by its height).
pub fn light_width(w: u64, pw: u64, ph: u64) -> u64 {
    let (pw, ph) = (pw.max(1), ph.max(1));
    let max_w = if ph > pw { LIGHT_SIDE * pw / ph } else { LIGHT_SIDE };
    w.min(max_w.max(64))
}

/// A part of a picture, as fractions of its width and height from the top
/// left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// `region`: `{"x", "y", "w", "h"}` or `[x, y, w, h]`, fractions of the
/// frame (0 to 1, from the top left).
pub fn region_of(a: &serde_json::Value) -> Result<Option<Region>, String> {
    let Some(v) = a.get("region").filter(|v| !v.is_null()) else { return Ok(None) };
    let nums: Vec<f64> = match v {
        serde_json::Value::Array(l) => l.iter().filter_map(serde_json::Value::as_f64).collect(),
        serde_json::Value::Object(_) => ["x", "y", "w", "h"].iter().filter_map(|k| v.get(*k).and_then(serde_json::Value::as_f64)).collect(),
        _ => Vec::new(),
    };
    let bad = || "`region` is {\"x\", \"y\", \"w\", \"h\"} (or [x, y, w, h]) as fractions of the frame from its top left, e.g. [0.5, 0, 0.5, 0.5] for the top right quarter".to_string();
    let [x, y, w, h] = nums[..] else { return Err(bad()) };
    if ![x, y, w, h].iter().all(|n| n.is_finite()) || x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x >= 1.0 || y >= 1.0 || w > 1.0 || h > 1.0 {
        return Err(bad());
    }
    Ok(Some(Region { x, y, w: w.min(1.0 - x), h: h.min(1.0 - y) }))
}

/// The pixels of `img` inside `r`, copied exactly (no resampling), and the
/// rectangle `(x, y, width, height)` they came from.
pub fn crop(img: &RgbaImage, r: Region) -> (RgbaImage, (u32, u32, u32, u32)) {
    let (iw, ih) = (img.width.max(1), img.height.max(1));
    let x0 = ((r.x * iw as f64).round() as u32).min(iw - 1);
    let y0 = ((r.y * ih as f64).round() as u32).min(ih - 1);
    let x1 = (((r.x + r.w) * iw as f64).round() as u32).clamp(x0 + 1, iw);
    let y1 = (((r.y + r.h) * ih as f64).round() as u32).clamp(y0 + 1, ih);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y1 {
        let from = ((y * iw + x0) * 4) as usize;
        pixels.extend_from_slice(&img.pixels[from..from + (w * 4) as usize]);
    }
    (RgbaImage { width: w, height: h, pixels }, (x0, y0, w, h))
}

/// `img` with its longer side brought down to `side` by averaging (only
/// when it is longer).
pub fn shrink(img: RgbaImage, side: u32) -> RgbaImage {
    let long = img.width.max(img.height);
    if long <= side || side == 0 {
        return img;
    }
    let k = side as f64 / long as f64;
    let (w, h) = (((img.width as f64 * k).round() as u32).max(1), ((img.height as f64 * k).round() as u32).max(1));
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let (sy0, sy1) = ((y as f64 / k) as u32, (((y + 1) as f64 / k).ceil() as u32).clamp(1, img.height));
        for x in 0..w {
            let (sx0, sx1) = ((x as f64 / k) as u32, (((x + 1) as f64 / k).ceil() as u32).clamp(1, img.width));
            let mut sum = [0u32; 4];
            let mut n = 0;
            for sy in sy0..sy1.max(sy0 + 1).min(img.height) {
                for sx in sx0..sx1.max(sx0 + 1).min(img.width) {
                    let i = ((sy * img.width + sx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += img.pixels[i + c] as u32;
                    }
                    n += 1;
                }
            }
            pixels.extend(sum.iter().map(|s| (s / n.max(1)) as u8));
        }
    }
    RgbaImage { width: w, height: h, pixels }
}

pub fn sheet_columns(n: usize) -> usize {
    match n {
        0..=3 => n.max(1),
        4 => 2,
        5..=9 => 3,
        10..=16 => 4,
        17..=30 => 5,
        _ => 6,
    }
}

#[cfg(feature = "render")]
#[derive(Default)]
pub struct Frames {
    state: Option<State>,
    /// Pictures rendered ahead in parallel by `prefetch`, by (time, width,
    /// clips left out).
    pre: std::collections::HashMap<(Time, u32, Vec<u32>), studio_media::rgba::RgbaImage>,
}

#[cfg(feature = "render")]
struct State {
    revision: u64,
    registry: std::sync::Arc<studio_render::source::SourceRegistry>,
    renderer: Box<dyn studio_render::FrameRenderer>,
    scene: std::sync::Arc<studio_render::model_scene::ModelScene>,
}

#[cfg(feature = "render")]
impl Frames {
    /// The renderer and scene for the current document. The renderer (and
    /// its decode cache) lives across edits; the scene follows the project.
    fn state(&mut self, doc: &Doc) -> &mut State {
        if self.state.is_none() {
            let registry = studio_render::source::SourceRegistry::new();
            let renderer = studio_render::best_renderer(studio_render::Sources::new(registry.clone(), studio_render::DecodeCache::new(256 << 20)));
            let scene = std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(doc.model.clone())));
            self.state = Some(State { revision: u64::MAX, registry, renderer, scene });
        }
        let st = self.state.as_mut().expect("state");
        if st.revision != doc.revision {
            crate::editor::render_view::register_sources(doc, &st.registry);
            crate::editor::render_view::install_text_renderer(doc.project.height);
            st.scene = std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(doc.model.clone())));
            st.revision = doc.revision;
        }
        st
    }

    pub(super) fn rgba(&mut self, doc: &Doc, t: Time, width: u32) -> Result<studio_media::rgba::RgbaImage, String> {
        if let Some(img) = self.pre.remove(&(t, width, Vec::new())) {
            return Ok(img);
        }
        self.rgba_with(doc, None, t, width)
    }

    /// Renders many pictures at once on several threads (each with its own
    /// decoders, each taking a stretch of time so it decodes forward); the
    /// next rgba / rgba_without for the same picture takes it from here.
    pub(super) fn prefetch(&mut self, doc: &Doc, mut jobs: Vec<(Time, u32, Vec<u32>)>) {
        jobs.retain(|j| !self.pre.contains_key(j));
        jobs.sort();
        jobs.dedup();
        if jobs.len() < 4 {
            return;
        }
        let st = self.state(doc);
        let registry = st.registry.clone();
        let base = st.scene.clone();
        // One scene per set of clips left out.
        let mut scenes: std::collections::HashMap<Vec<u32>, std::sync::Arc<studio_render::model_scene::ModelScene>> = Default::default();
        for (_, _, out) in &jobs {
            if !out.is_empty() && !scenes.contains_key(out) {
                let mut model = doc.model.clone();
                for seq in model.sequences.iter_mut() {
                    seq.clips.retain(|c| !out.contains(&(c.id.0 as u32)));
                }
                scenes.insert(out.clone(), std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(model))));
            }
        }
        let (pw, ph) = (doc.project.width.max(1), doc.project.height.max(1));
        let fps = doc.project.fps.max(1.0) as f64;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
        let per = jobs.len().div_ceil(threads);
        let done: Vec<((Time, u32, Vec<u32>), studio_media::rgba::RgbaImage)> = std::thread::scope(|sc| {
            let hs: Vec<_> = jobs
                .chunks(per.max(1))
                .map(|chunk| {
                    let (registry, base, scenes) = (registry.clone(), base.clone(), &scenes);
                    sc.spawn(move || {
                        let mut renderer = studio_render::CpuCompositor::new(studio_render::Sources::new(registry, studio_render::DecodeCache::new(96 << 20)));
                        let mut out = Vec::new();
                        for job in chunk {
                            let (t, width, ref leave) = *job;
                            let scene = if leave.is_empty() { base.clone() } else { scenes[leave].clone() };
                            let w = width.min(pw).max(16);
                            let h = ((w as f64 * ph as f64 / pw as f64).round() as u32).max(16);
                            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                studio_render::FrameRenderer::render_encoded(&mut renderer, scene.as_ref(), t.max(0), Some((w, h)), &studio_render::OutputSettings { premultiplied: false, ..Default::default() }, studio_render::OutputDepth::Eight, (t.max(0) as f64 / 1e6 * fps) as u32)
                            }));
                            if let Ok(e) = r {
                                let pixels = match e.pixels {
                                    studio_render::EncodedPixels::Rgba8(v) => v,
                                    studio_render::EncodedPixels::Rgba16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
                                };
                                out.push((job.clone(), studio_media::rgba::RgbaImage { width: e.width, height: e.height, pixels }));
                            }
                        }
                        out
                    })
                })
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        self.pre.extend(done);
    }

    pub(super) fn clear_prefetch(&mut self) {
        self.pre.clear();
    }

    /// The picture with some clips left out (a what-if, history untouched):
    /// lets checks see what lies under a title.
    pub(super) fn rgba_without(&mut self, doc: &Doc, leave_out: &[u32], t: Time, width: u32) -> Result<studio_media::rgba::RgbaImage, String> {
        if let Some(img) = self.pre.remove(&(t, width, leave_out.to_vec())) {
            return Ok(img);
        }
        let mut model = doc.model.clone();
        for seq in model.sequences.iter_mut() {
            seq.clips.retain(|c| !leave_out.contains(&(c.id.0 as u32)));
        }
        let scene = std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(model)));
        self.rgba_with(doc, Some(scene), t, width)
    }

    /// A renderer's copy of the project with some clips left out, to
    /// render many frames from ([`Frames::rgba_in`]).
    pub(super) fn scene_without(&mut self, doc: &Doc, leave_out: &[u32]) -> std::sync::Arc<studio_render::model_scene::ModelScene> {
        let mut model = doc.model.clone();
        for seq in model.sequences.iter_mut() {
            seq.clips.retain(|c| !leave_out.contains(&(c.id.0 as u32)));
        }
        std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(model)))
    }

    /// The picture at `t` of a scene from [`Frames::scene_without`].
    pub(super) fn rgba_in(&mut self, doc: &Doc, scene: &std::sync::Arc<studio_render::model_scene::ModelScene>, t: Time, width: u32) -> Result<studio_media::rgba::RgbaImage, String> {
        self.rgba_with(doc, Some(scene.clone()), t, width)
    }

    fn rgba_with(&mut self, doc: &Doc, scene: Option<std::sync::Arc<studio_render::model_scene::ModelScene>>, t: Time, width: u32) -> Result<studio_media::rgba::RgbaImage, String> {
        let (pw, ph) = (doc.project.width.max(1), doc.project.height.max(1));
        let w = width.min(pw).max(16);
        let h = ((w as f64 * ph as f64 / pw as f64).round() as u32).max(16);
        let fps = doc.project.fps.max(1.0) as f64;
        let st = self.state(doc);
        let scene = scene.unwrap_or_else(|| st.scene.clone());
        let frame_index = (t.max(0) as f64 / 1e6 * fps) as u32;
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            st.renderer.render_encoded(scene.as_ref(), t.max(0), Some((w, h)), &studio_render::OutputSettings { premultiplied: false, ..Default::default() }, studio_render::OutputDepth::Eight, frame_index)
        }))
        .map_err(|_| "rendering this frame failed".to_string())?;
        let pixels = match out.pixels {
            studio_render::EncodedPixels::Rgba8(v) => v,
            studio_render::EncodedPixels::Rgba16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
        };
        Ok(studio_media::rgba::RgbaImage { width: out.width, height: out.height, pixels })
    }

    /// The renderer's copy of the project (for the 3D workspace's card
    /// depths).
    pub(super) fn model_scene(&mut self, doc: &Doc) -> std::sync::Arc<studio_render::model_scene::ModelScene> {
        self.state(doc).scene.clone()
    }

    /// The picture seen from the 3D workspace's view (`options`), `width`
    /// pixels wide at the canvas aspect.
    pub(super) fn rgba_through(&mut self, doc: &Doc, options: studio_render::viewport::ViewOptions, t: Time, width: u32) -> Result<studio_media::rgba::RgbaImage, String> {
        let (pw, ph) = (doc.project.width.max(1), doc.project.height.max(1));
        let w = width.min(pw).max(16);
        let h = ((w as f64 * ph as f64 / pw as f64).round() as u32).max(16);
        let fps = doc.project.fps.max(1.0) as f64;
        let st = self.state(doc);
        let scene = studio_render::viewport::ViewScene::new(st.scene.clone(), options);
        let frame_index = (t.max(0) as f64 / 1e6 * fps) as u32;
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            st.renderer.render_encoded(&scene, t.max(0), Some((w, h)), &studio_render::OutputSettings { premultiplied: false, ..Default::default() }, studio_render::OutputDepth::Eight, frame_index)
        }))
        .map_err(|_| "rendering the view failed".to_string())?;
        let pixels = match out.pixels {
            studio_render::EncodedPixels::Rgba8(v) => v,
            studio_render::EncodedPixels::Rgba16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
        };
        Ok(studio_media::rgba::RgbaImage { width: out.width, height: out.height, pixels })
    }

    /// One frame at `t`, `width` pixels wide, as JPEG at `quality`.
    pub fn frame(&mut self, doc: &Doc, t: Time, width: u32, quality: u8) -> Result<Vec<u8>, String> {
        let img = self.rgba(doc, t, width)?;
        Ok(studio_content::jpeg::encode(&img, quality))
    }

    /// `n` frames evenly spread over `start..end`, tiled into one JPEG
    /// `width` pixels wide; returns the times (seconds) in reading order.
    pub fn sheet(&mut self, doc: &Doc, start: Time, end: Time, n: usize, width: u32, quality: u8) -> Result<(Vec<u8>, Vec<f64>), String> {
        let cols = sheet_columns(n);
        let rows = n.div_ceil(cols);
        let gap = 4u32;
        let tile_w = ((width - gap * (cols as u32 - 1)) / cols as u32).max(32);
        let span = (end - start).max(0);
        // Spread over the range, the last one a frame inside its end.
        let frame = (1e6 / doc.project.fps.max(1.0) as f64) as Time;
        let last = (end - frame).max(start);
        let times: Vec<Time> = (0..n).map(|i| if n == 1 { start } else { start + (last - start) * i as i64 / (n as i64 - 1) }).collect();
        let _ = span;
        let mut tiles = Vec::with_capacity(n);
        for &t in &times {
            tiles.push(self.rgba(doc, t, tile_w)?);
        }
        let sheet = tiled(&tiles, cols, tile_w);
        Ok((studio_content::jpeg::encode(&sheet, quality), times.iter().map(|&t| (t as f64 / 1e6 * 1000.0).round() / 1000.0).collect()))
    }
}

/// Tiles in reading order, `cols` across, on a dark ground.
pub fn tiled(tiles: &[RgbaImage], cols: usize, tile_w: u32) -> RgbaImage {
    let gap = 4u32;
    let rows = tiles.len().div_ceil(cols.max(1));
    let tile_h = tiles.first().map_or(16, |t| t.height);
    let (sw, sh) = (cols as u32 * tile_w + gap * (cols as u32 - 1), rows as u32 * tile_h + gap * (rows as u32).saturating_sub(1));
    let mut sheet = RgbaImage { width: sw, height: sh, pixels: vec![0; (sw * sh * 4) as usize] };
    for p in sheet.pixels.chunks_exact_mut(4) {
        p.copy_from_slice(&[20, 20, 21, 255]);
    }
    for (i, t) in tiles.iter().enumerate() {
        let (ox, oy) = ((i % cols) as u32 * (tile_w + gap), (i / cols) as u32 * (tile_h + gap));
        for y in 0..t.height.min(tile_h) {
            for x in 0..t.width.min(tile_w) {
                let s = ((y * t.width + x) * 4) as usize;
                let d = (((oy + y) * sw + ox + x) * 4) as usize;
                sheet.pixels[d..d + 4].copy_from_slice(&t.pixels[s..s + 4]);
                sheet.pixels[d + 3] = 255;
            }
        }
    }
    sheet
}

/// Frames of a media file itself (not the timeline): what the footage
/// holds before it is placed. Times are seconds into the file.
pub fn media_frames(mi: &crate::view::MediaItem, times: &[f64], tile_w: u32) -> Result<Vec<RgbaImage>, String> {
    let path = mi.path.as_ref().ok_or("that media has no file to look at")?;
    if mi.kind == crate::view::MediaKind::Audio {
        return Err("that media is audio; use analyse for its beats and pauses".into());
    }
    let w = tile_w as usize;
    let h = ((w as f64 * mi.height.max(1) as f64 / mi.width.max(1) as f64).round() as usize).max(8);
    let out: Vec<Result<RgbaImage, String>> = std::thread::scope(|sc| {
        let hs: Vec<_> = times.iter().map(|&t| sc.spawn(move || {
            let img = crate::media_io::frame(path, t, w, h, false)?;
            let mut pixels = Vec::with_capacity(w * h * 4);
            for c in &img.pixels {
                pixels.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
            }
            Ok(RgbaImage { width: img.size[0] as u32, height: img.size[1] as u32, pixels })
        })).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("decode failed".into()))).collect()
    });
    out.into_iter().collect()
}
