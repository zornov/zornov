// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generated pictures: media thumbnails and frames for the sample projects, the
//! sample photo used by Library previews, and effect / look variants of it.
//!
//! Everything is produced on background threads (spec §7: never block the
//! UI thread) and uploaded to textures on the UI thread when ready. Until a
//! picture arrives, callers draw a neutral placeholder.

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureId, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex};
use crate::catalogue;
use studio_fx::{EffectContext, ParamValue, effects, transitions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pic {
    /// A frame of footage. Generated footage: `seed` with `t` in
    /// thousandths of its length. Files (`seed` has [`FILE_BIT`]): `t` in
    /// milliseconds.
    Scene { seed: u32, t: u32 },
    /// A processed viewer frame. `grade` is a hash of the processing (see
    /// [`GradeParams::key`]).
    Graded { seed: u32, t: u32, grade: u64 },
    /// Library effect preview (index into `catalogue::effects`), frame
    /// `frame` of [`PREVIEW_FRAMES`].
    Effect { index: u8, frame: u8 },
    /// Library look preview (index into `catalogue::looks`).
    Look { index: u8 },
    /// Library transition preview (index into `catalogue::cuts`).
    Cut { index: u8, frame: u8 },
    /// Thumbnail of a content library entry (index into `Content::entries`).
    Content { index: u32 },
    /// A text style preset rendered by the text engine; `sample` shows its
    /// sample line instead of "Aa".
    TextStyle { index: u16, sample: bool },
    /// The plain sample photo.
    Sample { frame: u8 },
}

pub const PREVIEW_FRAMES: u8 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub pic: Pic,
    pub w: u16,
    pub h: u16,
}

/// How a viewer frame is processed: the clip's grade (UI units, see
/// `bridge`), its look and its effects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GradeParams {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    /// Index into `catalogue::looks`.
    pub look: Option<u8>,
    /// `fx` effect ids with their numeric parameters.
    pub effects: Vec<(String, Vec<(String, f32)>)>,
}

impl GradeParams {
    pub fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in [self.exposure, self.contrast, self.saturation, self.temperature, self.tint] {
            ((v * 10.0).round() as i32).hash(&mut h);
        }
        self.look.hash(&mut h);
        for (id, ps) in &self.effects {
            id.hash(&mut h);
            for (k, v) in ps {
                k.hash(&mut h);
                ((v * 100.0).round() as i32).hash(&mut h);
            }
        }
        h.finish()
    }
}

type Done = Arc<Mutex<Vec<(Key, ColorImage)>>>;

/// Work waiting for a thumbnail worker. Workers take the newest job first,
/// so what the person is looking at now comes before stale requests; each
/// frame, jobs nobody asked for again are dropped (see [`Thumbs::pump`]).
#[derive(Default)]
struct Queue {
    jobs: Mutex<Vec<(Key, GradeParams)>>,
    cv: Condvar,
}

/// Texture memory kept for thumbnails before the least recently drawn ones
/// are freed.
const TEXTURE_BUDGET: usize = 384 << 20;

/// Smooth when scaled down: thumbnails are drawn at many sizes, and plain
/// linear filtering without mipmaps shimmers on moving pictures.
const THUMB_OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

/// Marks a seed that refers to a registered file rather than generated
/// footage.
pub const FILE_BIT: u32 = 1 << 31;
/// Marks a seed that refers to a sample photo (`content::SAMPLE_PHOTOS`),
/// shown with a slow push-in so it plays like footage.
pub const PHOTO_BIT: u32 = 1 << 30;

/// Files registered for decoding: path and length in seconds.
type Files = Arc<Mutex<Vec<(std::path::PathBuf, f64)>>>;

pub struct Thumbs {
    queue: Arc<Queue>,
    /// Keys asked for since the last pump (queued jobs not among them are
    /// dropped as stale).
    asked: HashSet<Key>,
    /// Frame each texture was last drawn, and its size in bytes, for the
    /// least-recently-used eviction.
    used: HashMap<Key, (u64, usize)>,
    bytes: usize,
    frame: u64,
    done: Done,
    textures: HashMap<Key, TextureHandle>,
    pending: HashSet<Key>,
    /// Keep the last viewer frame per seed so scrubbing never flashes empty.
    last_graded: HashMap<u32, Key>,
    /// The last ready frame of each animated picture (same picture and
    /// size, any time or frame), shown while the next frame is made so
    /// hover previews never blank or jump back to their first frame.
    last_frame: HashMap<Key, Key>,
    files: Files,
    file_ids: HashMap<std::path::PathBuf, u32>,
}

impl Thumbs {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue: Arc<Queue> = Arc::default();
        let done: Done = Arc::default();
        let files: Files = Arc::default();
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        for i in 0..workers {
            let queue = queue.clone();
            let done = done.clone();
            let files = files.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name(format!("studio-thumbs-{i}"))
                .spawn(move || {
                    loop {
                        let (key, grade) = {
                            let mut jobs = queue.jobs.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                if let Some(j) = jobs.pop() {
                                    break j;
                                }
                                jobs = queue.cv.wait(jobs).unwrap_or_else(|e| e.into_inner());
                            }
                        };
                        // A picture that panics (a damaged file) becomes a grey
                        // placeholder; the worker lives on.
                        let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(key, &grade, &files)))
                            .unwrap_or_else(|_| ColorImage::new([key.w.max(1) as usize, key.h.max(1) as usize], vec![Color32::from_gray(40); key.w.max(1) as usize * key.h.max(1) as usize]));
                        done.lock().unwrap_or_else(|e| e.into_inner()).push((key, img));
                        ctx.request_repaint();
                    }
                })
                .expect("spawn thumbnail worker");
        }
        Self { queue, asked: HashSet::new(), used: HashMap::new(), bytes: 0, frame: 0, done, textures: HashMap::new(), pending: HashSet::new(), last_graded: HashMap::new(), last_frame: HashMap::new(), files, file_ids: HashMap::new() }
    }

    /// Upload finished pictures. Call once per frame.
    pub fn pump(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        // Drop queued work nobody asked for since the last frame: the grid
        // scrolled, the hover moved on, the playhead passed.
        {
            let asked = &self.asked;
            let pending = &mut self.pending;
            let mut jobs = self.queue.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.retain(|(k, _)| {
                let keep = asked.contains(k);
                if !keep {
                    pending.remove(k);
                }
                keep
            });
        }
        self.asked.clear();
        let ready: Vec<_> = std::mem::take(&mut *self.done.lock().unwrap_or_else(|e| e.into_inner()));
        for (key, img) in ready {
            self.pending.remove(&key);
            let size = img.size[0] * img.size[1] * 4 * 4 / 3;
            // Viewer frames are shown full size; everything else is scaled.
            let opts = if matches!(key.pic, Pic::Graded { .. }) { TextureOptions::LINEAR } else { THUMB_OPTIONS };
            let tex = ctx.load_texture(format!("{key:?}"), img, opts);
            if let Some((_, old)) = self.used.insert(key, (self.frame, size)) {
                self.bytes -= old;
            }
            self.bytes += size;
            self.textures.insert(key, tex);
        }
        // Over budget: free the textures drawn longest ago (never this
        // frame's).
        if self.bytes > TEXTURE_BUDGET {
            let mut by_age: Vec<(u64, Key, usize)> = self.used.iter().map(|(k, (f, b))| (*f, *k, *b)).collect();
            by_age.sort_by_key(|e| e.0);
            for (f, k, b) in by_age {
                if self.bytes <= TEXTURE_BUDGET * 3 / 4 || f + 2 >= self.frame {
                    break;
                }
                self.textures.remove(&k);
                self.used.remove(&k);
                self.bytes -= b;
            }
            self.last_frame.retain(|_, k| self.textures.contains_key(k));
            self.last_graded.retain(|_, k| self.textures.contains_key(k));
        }
        // Bound memory: viewer frames are the only unbounded kind.
    }

    pub fn get(&mut self, key: Key) -> Option<TextureId> {
        let family = Self::family(key);
        if let Some(t) = self.textures.get(&key) {
            if let Some(u) = self.used.get_mut(&key) {
                u.0 = self.frame;
            }
            if let Some(f) = family {
                self.last_frame.insert(f, key);
            }
            return Some(t.id());
        }
        self.request(key, GradeParams::default());
        family.and_then(|f| self.last_frame.get(&f)).and_then(|k| self.textures.get(k)).map(|t| t.id())
    }

    /// Queue `key` (once) and mark it wanted this frame.
    fn request(&mut self, key: Key, grade: GradeParams) {
        self.asked.insert(key);
        if self.pending.insert(key) {
            self.queue.jobs.lock().unwrap_or_else(|e| e.into_inner()).push((key, grade));
            self.queue.cv.notify_one();
        }
    }

    /// The picture with its time or frame zeroed, for animated pictures.
    fn family(key: Key) -> Option<Key> {
        let pic = match key.pic {
            Pic::Scene { seed, .. } => Pic::Scene { seed, t: 0 },
            Pic::Effect { index, .. } => Pic::Effect { index, frame: 0 },
            Pic::Cut { index, .. } => Pic::Cut { index, frame: 0 },
            Pic::Sample { .. } => Pic::Sample { frame: 0 },
            _ => return None,
        };
        Some(Key { pic, ..key })
    }

    /// Like [`Self::get`], with the picture's pixel size.
    pub fn get_sized(&mut self, key: Key) -> Option<(TextureId, eframe::egui::Vec2)> {
        let id = self.get(key)?;
        let t = self.textures.get(&key)?;
        let [w, h] = t.size();
        Some((id, eframe::egui::vec2(w as f32, h as f32)))
    }

    pub fn get_with(&mut self, key: Key, grade: &GradeParams) -> Option<TextureId> {
        if let Some(t) = self.textures.get(&key) {
            if let Some(u) = self.used.get_mut(&key) {
                u.0 = self.frame;
            }
            return Some(t.id());
        }
        self.request(key, grade.clone());
        None
    }

    /// The seed to use for a media item: its file when it has one.
    pub fn seed_of(&mut self, m: &crate::view::MediaItem) -> u32 {
        let Some(path) = &m.path else { return m.seed & !FILE_BIT };
        self.file_seed(path, m.dur as f64 / 1e6)
    }

    /// The seed of a file on disk (registered on first use), `seconds`
    /// long; 0 for a still.
    pub fn file_seed(&mut self, path: &std::path::Path, seconds: f64) -> u32 {
        if let Some(id) = self.file_ids.get(path) {
            return *id;
        }
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files.push((path.to_path_buf(), seconds));
        let id = FILE_BIT | (files.len() as u32 - 1);
        self.file_ids.insert(path.to_path_buf(), id);
        id
    }

    /// Time key for `frac` (0..1) of the footage behind `seed`.
    fn time_key(&self, seed: u32, frac: f32) -> u32 {
        let frac = frac.clamp(0.0, 1.0);
        if seed & FILE_BIT != 0 {
            let dur = self.files.lock().unwrap_or_else(|e| e.into_inner()).get((seed & !FILE_BIT) as usize).map_or(1.0, |f| f.1);
            (frac as f64 * dur * 1000.0) as u32
        } else {
            (frac * 1000.0) as u32
        }
    }

    /// A viewer frame; falls back to the last ready frame of the same
    /// footage while the new one is being made. `frac` is the position in
    /// the footage (0..1).
    pub fn graded(&mut self, seed: u32, frac: f32, grade: &GradeParams, w: u16, h: u16) -> Option<TextureId> {
        let t = self.time_key(seed, frac);
        let key = Key { pic: Pic::Graded { seed, t, grade: grade.key() }, w, h };
        if let Some(id) = self.get_with(key, grade) {
            self.last_graded.insert(seed, key);
            return Some(id);
        }
        self.last_graded.get(&seed).and_then(|k| self.textures.get(k)).map(|t| t.id())
    }

    pub fn scene(&mut self, seed: u32, t: f32, w: u16, h: u16) -> Option<TextureId> {
        let t = self.time_key(seed, t);
        self.get(Key { pic: Pic::Scene { seed, t }, w, h })
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

// --- generators ----------------------------------------------------------

fn footage(seed: u32, t: u32, w: usize, h: usize, files: &Files, keyframe_only: bool) -> ColorImage {
    if seed & FILE_BIT != 0 {
        let file = files.lock().unwrap_or_else(|e| e.into_inner()).get((seed & !FILE_BIT) as usize).cloned();
        if let Some((path, _)) = file {
            match crate::media_io::frame(&path, t as f64 / 1000.0, w, h, keyframe_only) {
                Ok(img) => return img,
                Err(e) => eprintln!("decode {}: {e}", path.display()),
            }
        }
        // Unreadable file: a neutral picture rather than nothing.
        return ColorImage::new([w, h], vec![Color32::from_gray(40); w * h]);
    }
    sample_frame(seed, t as f32 / 1000.0, w, h)
}

/// A frame of sample footage: a sample photo with a gentle push-in and
/// drift (`PHOTO_BIT` seeds), or the generated landscape when the content
/// library is not installed.
pub fn sample_frame(seed: u32, t: f32, w: usize, h: usize) -> ColorImage {
    if seed & PHOTO_BIT != 0 {
        let i = (seed & 0xFFFF) as usize;
        if let Some(img) = sample_photo(i) {
            return ken_burns(&img, i as u32, t, w, h);
        }
    }
    scene(seed & 0xFFFF_FF, t, w, h)
}

/// Decoded sample photos, kept at most 1280 wide.
fn sample_photo(i: usize) -> Option<Arc<ColorImage>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<usize, Option<Arc<ColorImage>>>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&i) {
        return v.clone();
    }
    let img = (|| {
        let content = crate::content::get_blocking()?;
        let (name, _) = crate::content::SAMPLE_PHOTOS.get(i)?;
        let path = content.photo(name)?;
        let full = crate::media_io::read_image(path).ok()?;
        let [fw, fh] = full.size;
        let s = (1280.0 / fw as f32).min(1.0);
        Some(Arc::new(crate::media_io::fit(&full, (fw as f32 * s) as usize, (fh as f32 * s) as usize)))
    })();
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(i, img.clone());
    img
}

/// A slow push-in with a little drift: `t` is 0..1 of the clip.
pub fn ken_burns(img: &ColorImage, seed: u32, t: f32, w: usize, h: usize) -> ColorImage {
    let [sw, sh] = img.size;
    let (sw, sh) = (sw as f32, sh as f32);
    let zoom = 1.0 + 0.14 * t;
    // Crop the target aspect out of the photo, then shrink it by the zoom.
    let target = w as f32 / h.max(1) as f32;
    let (mut cw, mut ch) = if sw / sh > target { (sh * target, sh) } else { (sw, sw / target) };
    cw /= zoom;
    ch /= zoom;
    let dir = if seed % 2 == 0 { 1.0 } else { -1.0 };
    let cx = sw / 2.0 + dir * (sw - cw) * 0.35 * (t - 0.5);
    let cy = sh / 2.0 - (sh - ch) * 0.2 * (t - 0.5);
    let x0 = (cx - cw / 2.0).clamp(0.0, sw - cw);
    let y0 = (cy - ch / 2.0).clamp(0.0, sh - ch);
    let mut out = Vec::with_capacity(w * h);
    let iw = img.size[0];
    for y in 0..h {
        for x in 0..w {
            // Bilinear sample.
            let fx = x0 + (x as f32 + 0.5) / w as f32 * cw - 0.5;
            let fy = y0 + (y as f32 + 0.5) / h as f32 * ch - 0.5;
            let (ix, iy) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (ax, ay) = (fx - ix as f32, fy - iy as f32);
            let at = |xx: usize, yy: usize| img.pixels[yy.min(img.size[1] - 1) * iw + xx.min(iw - 1)];
            let (p00, p10, p01, p11) = (at(ix, iy), at(ix + 1, iy), at(ix, iy + 1), at(ix + 1, iy + 1));
            let mixc = |a: u8, b: u8, c: u8, d: u8| {
                let top = a as f32 + (b as f32 - a as f32) * ax;
                let bot = c as f32 + (d as f32 - c as f32) * ax;
                (top + (bot - top) * ay).round() as u8
            };
            out.push(Color32::from_rgb(mixc(p00.r(), p10.r(), p01.r(), p11.r()), mixc(p00.g(), p10.g(), p01.g(), p11.g()), mixc(p00.b(), p10.b(), p01.b(), p11.b())));
        }
    }
    ColorImage::new([w, h], out)
}

fn render(key: Key, grade: &GradeParams, files: &Files) -> ColorImage {
    let (w, h) = (key.w as usize, key.h as usize);
    match key.pic {
        Pic::Scene { seed, t } => footage(seed, t, w, h, files, true),
        Pic::Graded { seed, t, .. } => {
            let img = footage(seed, t, w, h, files, false);
            process(&img, grade)
        }
        Pic::Sample { frame } => sample_frame(SAMPLE_SEED, 0.25 + frame as f32 * 0.004, w, h),
        Pic::TextStyle { index, sample } => {
            let text = sample.then(|| studio_text::styles::catalogue().get(index as usize).map(|p| p.sample.clone())).flatten();
            let text = text.map(|s| if s.chars().count() > 14 { s.split_whitespace().take(2).collect::<Vec<_>>().join(" ") } else { s });
            crate::text_look::style_tile(index as usize, w, h, text.as_deref()).unwrap_or_else(|| ColorImage::new([w, h], vec![Color32::BLACK; w * h]))
        }
        Pic::Content { index } => content_thumb(index as usize, w.max(h)).unwrap_or_else(|| ColorImage::new([w, h], vec![Color32::TRANSPARENT; w * h])),
        Pic::Effect { index, frame } => {
            let img = sample_frame(SAMPLE_SEED, 0.25, w, h);
            let Some(item) = catalogue::effects().get(index as usize) else { return img };
            let Some(def) = effects::find(item.id) else { return img };
            let phase = frame as f32 / PREVIEW_FRAMES as f32;
            // Idle (frame 0) shows the effect at its defaults; hover frames
            // sweep in from the identity so the change is visible.
            let k = if frame == 0 { 1.0 } else { 0.2 + 0.8 * (0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()) };
            let mut params = def.defaults();
            if let Some((id, zero)) = def.identity {
                if let Some(ParamValue::Float(d)) = params.get(id) {
                    let _ = params.set(id, ParamValue::Float(zero + (d - zero) * k));
                }
            }
            let ctx = EffectContext { time: phase * 2.0, ..EffectContext::still() };
            from_fx(&def.apply(&ctx, &to_fx(&img), &params))
        }
        Pic::Look { index } => {
            let img = sample_frame(SAMPLE_SEED, 0.25, w, h);
            match studio_colour::looks::LOOKS.get(index as usize) {
                Some(look) => {
                    let mut ci = to_colour(&img);
                    (look.grade)().apply(&mut ci, studio_colour::FrameInfo::default());
                    from_colour(&ci)
                }
                None => img,
            }
        }
        Pic::Cut { index, frame } => {
            let a = sample_frame(SAMPLE_SEED, 0.25, w, h);
            let b = sample_frame(SAMPLE_SEED_B, 0.3, w, h);
            let Some(def) = catalogue::cuts().get(index as usize).and_then(|c| transitions::find(c.id)) else { return a };
            let progress = frame as f32 / (PREVIEW_FRAMES - 1) as f32;
            from_fx(&def.apply(&Default::default(), &to_fx(&a), &to_fx(&b), progress, &def.defaults()))
        }
    }
}

/// Viewer processing: effects, then the manual grade, then the look.
pub fn process(img: &ColorImage, g: &GradeParams) -> ColorImage {
    if g.effects.is_empty() && g.look.is_none() && [g.exposure, g.contrast, g.saturation, g.temperature, g.tint].iter().all(|v| *v == 0.0) {
        return img.clone();
    }
    let mut fxi = to_fx(img);
    for (id, overrides) in &g.effects {
        let Some(def) = effects::find(id) else { continue };
        // Effects that need earlier frames or a clean plate wait for the
        // renderer.
        if def.history_needed(&def.defaults(), 30.0) > 0 || def.category == studio_fx::Category::Keying {
            continue;
        }
        let mut params = def.defaults();
        for (k, v) in overrides {
            let _ = match params.get(k) {
                Some(ParamValue::Int(_)) => params.set(k, ParamValue::Int(v.round() as i32)),
                Some(ParamValue::Float(_)) => params.set(k, ParamValue::Float(*v)),
                _ => Ok(()),
            };
        }
        fxi = def.apply(&EffectContext::still(), &fxi, &params);
    }
    let mut ci = studio_colour::Image::from_pixels(fxi.width as u32, fxi.height as u32, fxi.pixels);
    let mut grade = studio_colour::Grade::default();
    let num = |path: &str, v: f32| crate::bridge::model_value(path, v).as_f64().unwrap_or(0.0) as f32;
    grade.basic.exposure = num("grade.exposure", g.exposure);
    grade.basic.contrast = num("grade.contrast", g.contrast);
    grade.basic.saturation = num("grade.saturation", g.saturation);
    grade.basic.temperature = num("grade.temperature", g.temperature);
    grade.basic.tint = num("grade.tint", g.tint);
    if !grade.is_identity() {
        grade.apply(&mut ci, studio_colour::FrameInfo::default());
    }
    if let Some(look) = g.look.and_then(|i| studio_colour::looks::LOOKS.get(i as usize)) {
        (look.grade)().apply(&mut ci, studio_colour::FrameInfo::default());
    }
    from_colour(&ci)
}

fn to_fx(img: &ColorImage) -> studio_fx::Image {
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    studio_fx::Image::from_srgb8(img.size[0], img.size[1], &bytes)
}

fn from_fx(img: &studio_fx::Image) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.to_srgb8())
}

fn to_colour(img: &ColorImage) -> studio_colour::Image {
    let f = to_fx(img);
    studio_colour::Image::from_pixels(f.width as u32, f.height as u32, f.pixels)
}

fn from_colour(img: &studio_colour::Image) -> ColorImage {
    let f = studio_fx::Image { width: img.width as usize, height: img.height as usize, pixels: img.pixels.clone() };
    from_fx(&f)
}

/// The photo every effect, look and cut preview is shown on.
pub const SAMPLE_SEED: u32 = PHOTO_BIT | 11;
pub const SAMPLE_SEED_B: u32 = PHOTO_BIT | 1;

/// A content item's thumbnail at about `px` pixels on its long side.
fn content_thumb(index: usize, px: usize) -> Option<ColorImage> {
    let c = crate::content::get_blocking()?;
    let e = c.entries.get(index)?;
    let t = c.thumbnailer.as_ref()?;
    let img = t.thumbnail(&e.path, e.kind, px.clamp(32, 512) as u32).ok()?;
    Some(ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels))
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn hash(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    (x & 0xFFFF) as f32 / 65535.0
}

/// A small landscape: sky gradient, sun, three layers of hills and a
/// little grain. `seed` picks palette and shapes; `t` (0..1) drifts the
/// camera so filmstrips and playback change over time.
pub fn scene(seed: u32, t: f32, w: usize, h: usize) -> ColorImage {
    let hue = hash(seed * 7 + 1);
    let warm = hash(seed * 13 + 5);
    let sky_top = hsv(hue * 0.35 + 0.55, 0.45 + 0.2 * warm, 0.55 + 0.25 * hash(seed + 3));
    let sky_bot = hsv(0.05 + 0.08 * warm, 0.35 + 0.3 * warm, 0.95);
    let sun_c = hsv(0.08 + 0.05 * warm, 0.55, 1.0);
    let sun = (0.2 + 0.6 * hash(seed * 31) - t * 0.15, 0.32 + 0.15 * hash(seed * 17));
    let layers: Vec<([f32; 3], f32, f32, f32, f32)> = (0..3)
        .map(|k| {
            let kf = k as f32;
            let c = hsv(hue * 0.4 + 0.3 + kf * 0.04, 0.35 + 0.1 * kf, 0.55 - 0.16 * kf);
            let base = 0.55 + 0.12 * kf;
            let amp = 0.06 + 0.05 * hash(seed * 3 + k);
            let freq = 3.0 + 4.0 * hash(seed * 5 + k * 11);
            let phase = hash(seed * 19 + k) * 6.28 + t * (0.6 + kf * 0.9) * 3.0;
            (c, base, amp, freq, phase)
        })
        .collect();
    let aspect = w as f32 / h.max(1) as f32;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let mut c = lerp3(sky_top, sky_bot, (v / 0.65).min(1.0).powf(1.3));
            let dx = (u - sun.0) * aspect;
            let dy = v - sun.1;
            let d = (dx * dx + dy * dy).sqrt();
            let glow = (1.0 - d / 0.45).max(0.0).powi(2) * 0.45;
            c = lerp3(c, sun_c, glow);
            if d < 0.075 {
                c = lerp3(c, [1.0, 0.97, 0.9], ((0.075 - d) / 0.01).min(1.0));
            }
            for (lc, base, amp, freq, phase) in &layers {
                let yy = base + amp * (u * freq + phase).sin() + amp * 0.4 * (u * freq * 2.3 + phase * 1.7).sin();
                if v > yy {
                    let shade = ((v - yy) * 1.5).min(0.25);
                    c = [lc[0] * (1.0 - shade), lc[1] * (1.0 - shade), lc[2] * (1.0 - shade)];
                }
            }
            // Vignette and grain.
            let vx = u - 0.5;
            let vy = v - 0.5;
            let vig = 1.0 - (vx * vx + vy * vy) * 0.5;
            let g = (hash((x as u32).wrapping_mul(7919) ^ (y as u32).wrapping_mul(104_729) ^ seed) - 0.5) * 0.025;
            px.push(Color32::from_rgb(to8(c[0] * vig + g), to8(c[1] * vig + g), to8(c[2] * vig + g)));
        }
    }
    ColorImage::new([w, h], px)
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_differ_by_seed_and_time() {
        let a = scene(1, 0.0, 32, 18);
        let b = scene(2, 0.0, 32, 18);
        let c = scene(1, 0.5, 32, 18);
        assert_ne!(a.pixels, b.pixels);
        assert_ne!(a.pixels, c.pixels);
    }

    #[test]
    fn previews_change_the_picture() {
        let base = sample_frame(SAMPLE_SEED, 0.25, 48, 27);
        let k = |pic| Key { pic, w: 48, h: 27 };
        let posterise = crate::catalogue::effects().iter().position(|e| e.id == "posterise").unwrap() as u8;
        let files = Files::default();
        let fx = render(k(Pic::Effect { index: posterise, frame: 0 }), &GradeParams::default(), &files);
        assert_ne!(fx.pixels, base.pixels);
        let look = render(k(Pic::Look { index: 0 }), &GradeParams::default(), &files);
        assert_ne!(look.pixels, base.pixels);
        let graded = process(&base, &GradeParams { exposure: 40.0, ..Default::default() });
        assert_ne!(graded.pixels, base.pixels);
        assert_eq!(process(&base, &GradeParams::default()).pixels, base.pixels);
    }
}
