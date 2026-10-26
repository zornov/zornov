// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `objects`: what is in the footage and the pictures, as a queryable map.
//!
//! Each piece of footage (or picture) is read once into an object map
//! (`studio_ai::objects`): every nameable thing in it with an id, a label
//! and a score, a box per sampled frame, how far away and how big, its
//! colours, any text on it, faces and body points of people, the way
//! animals face, when it comes in and goes out, how it moves apart from
//! the camera, and what the camera does. The map is cached by the footage's
//! content hash, so asking again is instant and a second project using the
//! same footage reads nothing twice.
//!
//! * `list` (the default): what is in a shot, most prominent first.
//! * `where {object, t}`, `when {object}`, `around {object, t}`: place,
//!   time and neighbours (in front, behind, on, holding, near).
//! * `find {query}`: the shot or moment that matches a description (picture-text
//!   embeddings of the frames).
//! * `space {t, to}`: where a title or a placed thing is clear of everything.
//! * `text`: words read in the picture.
//! * `mask {object, t}`: SAM cuts the object; the answer hands the pick to
//!   `cut_out` / `roto` / `track` as ready ops (`apply: true` runs the cut).
//!
//! `see` adds the object list to its lines.

use super::{Session, f, secs};
use crate::editor::xform;
use crate::services::cutout::RgbaFrames;
use crate::services::tracking::{self, Footage};
use crate::view::{self, ClipKind, SEC};
use eframe::egui::Vec2;
use serde_json::{Value, json};
use std::ops::Range;
use studio_ai::features::embed::PictureTextEmbedder;
use studio_ai::objects::build::{self, BuildOptions, Models};
use studio_ai::objects::moments::{self, FrameIndex};
use studio_ai::objects::space::{self, SpaceOptions};
use studio_ai::objects::words::{self, ListOptions, View};
use studio_ai::objects::{Object, ObjectMap};
use studio_ai::registry::ModelId;
use studio_ai::{JobCtx, ModelStore, ResultCache};
use studio_model as m;

/// Longest side of the pictures the models read.
const ANALYSIS: usize = 960;
/// Smaller pictures are enough for embeddings.
const EMBED_SIDE: usize = 448;

// ----------------------------------------------------------------- models

/// Fetches a model the request needs (when downloads are allowed), saying
/// what it is when it cannot.
fn ensure(store: &ModelStore, id: ModelId, download: bool) -> Result<(), String> {
    if store.is_installed(id) {
        return Ok(());
    }
    let spec = id.spec();
    let what = format!("{} ({}, {})", spec.name, studio_ai::registry::human_size(spec.download_size()), spec.licence);
    if !download || cfg!(test) {
        return Err(format!("{what} is not installed; get it in Settings → AI models"));
    }
    store.download(id, &JobCtx::detached()).map_err(|e| format!("could not download {what}: {e}; get it in Settings → AI models"))
}

/// The models the object map reads with. The detector is required; the
/// helpers (depth, text, faces, body points) are fetched too when
/// downloads are allowed, and left out when they cannot be had.
fn load_models(download: bool) -> Result<Models, String> {
    let store = ModelStore::default();
    ensure(&store, ModelId::OwlV2Base, download)?;
    for id in [ModelId::DepthAnythingV2Small, ModelId::PaddleOcrLatin, ModelId::FaceDetector, ModelId::PoseLandmarker] {
        let _ = ensure(&store, id, download);
    }
    Models::load(&store).map_err(|e| format!("the object model is not ready: {e}"))
}

fn cache() -> ResultCache {
    // Tests never write into the person's Studio folder.
    // (and each test its own folder: they run side by side).
    if cfg!(test) {
        let who: String = std::thread::current().name().unwrap_or("t").chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        ResultCache::new(std::env::temp_dir().join("studio-tests").join("objects").join(who))
    } else {
        ResultCache::new(ResultCache::default_dir())
    }
}

// ------------------------------------------------------------------- shots

/// How footage time (seconds from the first frame read) and timeline time
/// go into each other for one clip.
#[derive(Clone, Copy, Debug)]
struct Clock {
    start: i64,
    dur: i64,
    a: i64,
    b: i64,
    first_s: f64,
    still: bool,
}

impl Clock {
    /// Timeline seconds of a moment of the footage.
    fn tl(&self, map_t: f32) -> f32 {
        if self.still || self.b == self.a {
            return secs(self.start) as f32;
        }
        let src = (self.first_s + map_t as f64) * 1e6;
        let k = (src - self.a as f64) / (self.b - self.a) as f64;
        secs((self.start as f64 + k * (self.dur - 1).max(0) as f64).round() as i64).clamp(secs(self.start), secs(self.start + self.dur)) as f32
    }
}

/// One clip's footage, ready to read.
struct Shot {
    id: u32,
    mc: m::Clip,
    footage: Footage,
    range: Range<u64>,
    clock: Clock,
    hash: String,
    size: (usize, usize),
    still: bool,
}

impl Shot {
    fn frames(&self) -> Result<RgbaFrames, String> {
        RgbaFrames::open(&self.footage, self.size, &self.range)
    }

    /// Footage seconds of a timeline moment.
    fn footage_time(&self, tl: f64) -> f32 {
        if self.still {
            return 0.0;
        }
        let src = self.mc.source_time_at((tl * SEC as f64).round() as i64);
        ((src as f64 / 1e6 - self.range.start as f64 / self.footage.fps).max(0.0)) as f32
    }

    /// The frame (relative to the range) at a timeline moment.
    fn frame_at(&self, tl: f64) -> usize {
        let n = (self.range.end - self.range.start) as usize;
        ((self.footage_time(tl) as f64 * self.footage.fps).round() as usize).min(n.saturating_sub(1))
    }

    fn key(&self, extra: &str) -> String {
        format!("{}:{}-{}@{:.3}{extra}", self.hash, self.range.start, self.range.end, self.footage.fps)
    }
}

fn analysis_size(w: u32, h: u32, side: usize) -> (usize, usize) {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let k = (side as f64 / w.max(h) as f64).min(1.0);
    (((w as f64 * k).round() as usize).max(16), ((h as f64 * k).round() as usize).max(16))
}

/// The picture behind a clip: footage, or a still.
fn footage_of(proj: &view::Project, mc: &m::Clip) -> Result<Footage, String> {
    if let m::ClipSource::Media(r) = &mc.source
        && let Some(mi) = proj.media(r.media.0 as u32).filter(|mi| mi.kind == view::MediaKind::Image)
    {
        let path = mi.path.clone().ok_or("sample media has no file to read")?;
        return Ok(Footage { path, fps: 30.0, frames: 1, width: mi.width.max(1), height: mi.height.max(1) });
    }
    tracking::footage(proj, mc)
}

impl Session {
    fn read_shot(&self, id: u32) -> Result<Shot, String> {
        let vc = self.clip(id)?;
        if vc.kind != ClipKind::Video {
            return Err(format!("clip {id} is not a picture or video clip (titles, sounds and effects have no objects)"));
        }
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let footage = footage_of(&self.doc.project, &mc)?;
        let still = footage.frames <= 1;
        let range = if still { 0..1 } else { tracking::used_frames(&mc, &footage) };
        let hash = studio_ai::cache::media_hash(&footage.path).unwrap_or_else(|_| footage.path.display().to_string());
        let a = mc.source_time_at(mc.start);
        let b = mc.source_time_at(mc.start + mc.duration - 1);
        let clock = Clock { start: mc.start, dur: mc.duration, a, b, first_s: range.start as f64 / footage.fps, still };
        let size = analysis_size(footage.width, footage.height, ANALYSIS);
        Ok(Shot { id, mc, footage, range, clock, hash, size, still })
    }

    /// The picture clip showing at timeline second `t`: the main lane's, or
    /// the top one.
    pub(super) fn object_clip_at(&self, t: f64) -> Option<u32> {
        let p = &self.doc.project;
        let at = (t * SEC as f64) as view::Time;
        let spine = p.spine();
        let live: Vec<&view::Clip> = p.clips.iter().filter(|c| c.kind == ClipKind::Video && c.start <= at && at < c.end()).collect();
        live.iter().find(|c| Some(c.lane) == spine).or_else(|| live.first()).map(|c| c.id)
    }

    /// Which clips a request is about: `clip`, `clips`, the one at `t`, or
    /// every main-lane shot.
    fn object_clips(&self, a: &Value) -> Result<Vec<u32>, String> {
        let ids = |v: &Value| -> Vec<u32> {
            match v {
                Value::Array(x) => x.iter().filter_map(Value::as_u64).map(|n| n as u32).collect(),
                v => v.as_u64().map(|n| vec![n as u32]).unwrap_or_default(),
            }
        };
        if let Some(v) = a.get("clip").or(a.get("clips")) {
            let l = ids(v);
            if l.is_empty() {
                return Err("clip is a clip id (see summary), or clips a list of ids".into());
            }
            return Ok(l);
        }
        if let Some(t) = f(a, "t").or(f(a, "at")) {
            return self.object_clip_at(t).map(|c| vec![c]).ok_or_else(|| format!("no picture is showing at {t:.2}s; give a clip"));
        }
        let p = &self.doc.project;
        let spine = p.spine();
        let mut cs: Vec<&view::Clip> = p.clips.iter().filter(|c| c.kind == ClipKind::Video && Some(c.lane) == spine).collect();
        cs.sort_by_key(|c| c.start);
        if cs.is_empty() {
            cs = p.clips.iter().filter(|c| c.kind == ClipKind::Video).collect();
            cs.sort_by_key(|c| c.start);
        }
        if cs.is_empty() {
            return Err("there is no picture or video on the timeline to read".into());
        }
        Ok(cs.iter().map(|c| c.id).collect())
    }

    fn object_options(a: &Value) -> BuildOptions {
        let mut o = BuildOptions::default();
        match a.get("names").or(a.get("look_for")) {
            Some(Value::Array(v)) => o.names = v.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            Some(Value::String(s)) => o.names = s.split(',').map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect(),
            _ => {}
        }
        if a.get("quick").and_then(Value::as_bool) == Some(true) {
            o.max_keyframes = 2;
            o.read_text = false;
        }
        if let Some(h) = f(a, "fov") {
            o.hfov_deg = h as f32;
        }
        o
    }

    /// The object map of a shot: from the cache, or read now.
    fn object_map(&mut self, shot: &Shot, o: &BuildOptions, download: bool, refresh: bool) -> Result<ObjectMap, String> {
        let c = cache();
        let key = ObjectMap::cache_key(&shot.key(""), &format!("{}|{}x{}", o.cache_params(), shot.size.0, shot.size.1));
        if !refresh && let Some(mp) = ObjectMap::load(&c, &key) {
            return Ok(mp);
        }
        let mut models = load_models(download)?;
        let n = (shot.range.end - shot.range.start) as usize;
        let mut src = shot.frames()?;
        let started = std::time::Instant::now();
        let mp = build::build(&mut src, 0..n, &mut models, o, &JobCtx::detached()).map_err(|e| format!("could not read the objects: {e}"))?;
        if std::env::var_os("STUDIO_PROFILE").is_some() {
            eprintln!("objects: clip {} read in {:.1}s ({} key frames, {} objects)", shot.id, started.elapsed().as_secs_f64(), mp.keyframes.len(), mp.objects.len());
        }
        let _ = mp.store(&c, &key);
        Ok(mp)
    }

    /// The map for one moment of a shot only (one key frame): what `see`
    /// uses. Cached apart from the whole-shot map; a whole-shot map that is
    /// already there is used instead.
    fn object_map_at(&mut self, shot: &Shot, tl: f64, o: &BuildOptions, download: bool) -> Result<ObjectMap, String> {
        let c = cache();
        let whole = ObjectMap::cache_key(&shot.key(""), &format!("{}|{}x{}", o.cache_params(), shot.size.0, shot.size.1));
        if let Some(mp) = ObjectMap::load(&c, &whole) {
            return Ok(mp);
        }
        let fr = shot.frame_at(tl);
        let key = ObjectMap::cache_key(&shot.key(&format!("#frame{fr}")), &format!("{}|{}x{}", o.cache_params(), shot.size.0, shot.size.1));
        if let Some(mp) = ObjectMap::load(&c, &key) {
            return Ok(mp);
        }
        let mut models = load_models(download)?;
        let mut src = shot.frames()?;
        let mp = build::build(&mut src, fr..fr + 1, &mut models, o, &JobCtx::detached()).map_err(|e| format!("could not read the objects: {e}"))?;
        let _ = mp.store(&c, &key);
        Ok(mp)
    }

    /// A cached map for `see`, without reading anything.
    fn object_map_cached(&self, shot: &Shot, tl: f64, o: &BuildOptions) -> Option<ObjectMap> {
        let c = cache();
        let params = format!("{}|{}x{}", o.cache_params(), shot.size.0, shot.size.1);
        ObjectMap::load(&c, &ObjectMap::cache_key(&shot.key(""), &params)).or_else(|| ObjectMap::load(&c, &ObjectMap::cache_key(&shot.key(&format!("#frame{}", shot.frame_at(tl))), &params)))
    }

    /// The line `see` adds for a shot: what is in it. `compute` reads the
    /// shot if its map is not there yet (and `download` may fetch the
    /// models); otherwise only a cached map speaks.
    pub(super) fn objects_line(&mut self, clip: u32, tl: f64, compute: bool, download: bool) -> Option<String> {
        let shot = self.read_shot(clip).ok()?;
        let o = BuildOptions::default();
        let mp = match self.object_map_cached(&shot, tl, &o) {
            Some(mp) => mp,
            None if compute => self.object_map_at(&shot, tl, &o, download).ok()?,
            None => return None,
        };
        let b = words::brief(&mp, 8);
        (!b.is_empty()).then(|| format!("objects: {b}"))
    }

    // ------------------------------------------------------------- the tool

    pub(super) fn objects(&mut self, a: &Value) -> Result<String, String> {
        let action = a.get("action").or(a.get("do")).and_then(Value::as_str).map(str::to_lowercase);
        let action = match action.as_deref() {
            Some(x) => x.to_owned(),
            None if a.get("query").is_some() => "find".into(),
            None if a.get("object").is_some() && (a.get("t").is_some() || a.get("at").is_some()) => "where".into(),
            None if a.get("object").is_some() => "when".into(),
            None => "list".into(),
        };
        let o = Self::object_options(a);
        let refresh = a.get("refresh").and_then(Value::as_bool).unwrap_or(false);
        match action.as_str() {
            "find" | "search" => return self.objects_find(a),
            "list" | "where" | "when" | "around" | "space" | "text" | "mask" | "cutout" => {}
            other => return Err(format!("action \"{other}\": list (what is in a shot), where {{object, t}}, when {{object}}, around {{object, t}}, find {{query}}, space {{t, to}}, text, mask {{object, t}}")),
        }
        let clips = self.object_clips(a)?;
        // How many shots may be read now: a read costs seconds a shot.
        let limit = a.get("max_shots").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(if action == "list" || action == "when" { 4 } else { 8 });
        let mut out: Vec<String> = Vec::new();
        let mut unread = 0usize;
        let mut read = 0usize;
        let many = clips.len() > 1;
        for &id in &clips {
            let shot = self.read_shot(id)?;
            // Cached shots are free; unread ones count against the limit.
            let cached = self.object_map_cached(&shot, 0.0, &o).is_some() && !refresh;
            if !cached && read >= limit {
                unread += 1;
                continue;
            }
            let map = self.object_map(&shot, &o, true, refresh)?;
            if !cached {
                read += 1;
            }
            let part = self.objects_answer(&action, a, &shot, &map, many)?;
            if let Some(p) = part {
                out.push(p);
            }
        }
        if unread > 0 {
            out.push(format!("{unread} more shot(s) not read yet (each takes seconds): ask for them with clips: [ids], or raise max_shots"));
        }
        if out.is_empty() {
            return Err(match action.as_str() {
                "where" | "around" | "mask" | "cutout" => "no such object in that shot: list what is there first".into(),
                _ => "nothing to report".into(),
            });
        }
        Ok(out.join("\n"))
    }

    /// Everything the canvas needs for the view of a shot at timeline second
    /// `tl`: a function from footage fractions to canvas pixels from the
    /// centre.
    fn object_canvas(&self, shot: &Shot, tl: f64) -> impl Fn(f32, f32) -> (f32, f32) + 'static {
        let geo = xform::geo(&self.doc.project, &self.doc.model, shot.id, (tl * SEC as f64) as i64);
        let (cw, ch) = (self.doc.project.width.max(1) as f32, self.doc.project.height.max(1) as f32);
        let src = geo.as_ref().map(|g| g.source()).unwrap_or(Vec2::new(cw, ch));
        move |x, y| match &geo {
            Some(g) => {
                let c = g.to_canvas(Vec2::new(x * src.x, y * src.y));
                (c.x, c.y)
            }
            None => ((x - 0.5) * cw, (y - 0.5) * ch),
        }
    }

    /// The footage seconds an answer is about: `t` (timeline), else the
    /// middle of the clip.
    fn object_time(&self, a: &Value, shot: &Shot) -> (f64, f32) {
        let vc = self.doc.project.clip(shot.id);
        let (s, e) = vc.map(|c| (secs(c.start), secs(c.end()))).unwrap_or((0.0, 0.0));
        let tl = f(a, "t").or(f(a, "at")).unwrap_or((s + e) / 2.0).clamp(s, (e - 0.001).max(s));
        (tl, shot.footage_time(tl))
    }

    fn objects_answer(&mut self, action: &str, a: &Value, shot: &Shot, map: &ObjectMap, many: bool) -> Result<Option<String>, String> {
        let (tl, ft) = self.object_time(a, shot);
        let canvas = self.object_canvas(shot, tl);
        let clock = shot.clock;
        let time = move |t: f32| clock.tl(t);
        let foot = {
            let (mc, fps, first) = (shot.mc.clone(), shot.footage.fps, shot.range.start);
            let still = shot.still;
            move |tl: f32| if still { 0.0 } else { ((mc.source_time_at((tl as f64 * SEC as f64) as i64) as f64 / 1e6 - first as f64 / fps).max(0.0)) as f32 }
        };
        let v = View { canvas: &canvas, time: &time, footage_time: &foot };
        let head = if many { format!("clip {}: ", shot.id) } else { String::new() };
        let detail = a.get("detail").and_then(Value::as_bool).unwrap_or(false);
        let max = a.get("max").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(if many { 6 } else { 12 });
        let pick = |a: &Value| -> Result<Vec<&Object>, String> { resolve(map, a.get("object")) };
        let text = match action {
            "list" => {
                let at = (a.get("t").is_some() || a.get("at").is_some()).then_some(ft);
                format!("{head}{}", words::list(map, &v, &ListOptions { max, detail, at }))
            }
            "where" => {
                let objs = pick(a)?;
                objs.iter().take(3).map(|o| words::where_at(map, o, ft, &v)).collect::<Vec<_>>().join("\n")
            }
            "when" => {
                let objs = if a.get("object").is_some() { pick(a)? } else { map.objects.iter().collect() };
                format!("{head}{}", words::when(&objs, map, &v))
            }
            "around" => {
                let objs = pick(a)?;
                words::around(map, objs[0], ft, &v)
            }
            "space" => {
                let to_tl = f(a, "to").unwrap_or(tl + f(a, "for").unwrap_or(0.0));
                let from_f = if f(a, "from").is_some() { shot.footage_time(f(a, "from").unwrap_or(tl)) } else { ft };
                let to_f = shot.footage_time(to_tl.max(tl));
                let opts = SpaceOptions { from: from_f.min(to_f), to: to_f.max(from_f), margin: f(a, "margin").map(|m| m as f32).unwrap_or(0.05), min_w: f(a, "min_width").map(|w| w as f32).unwrap_or(0.2), min_h: f(a, "min_height").map(|w| w as f32).unwrap_or(0.1), max: 4 };
                format!("{head}{}", words::space(&space::free_space(map, &opts), &v, tl as f32, (tl as f32).max(to_tl as f32)))
            }
            "text" => format!("{head}{}", words::texts(map, &v)),
            "mask" | "cutout" => {
                let objs = pick(a)?;
                self.object_mask(a, shot, map, objs[0], tl, ft)?
            }
            _ => return Ok(None),
        };
        Ok(Some(text))
    }

    // --------------------------------------------------------------- find

    fn objects_find(&mut self, a: &Value) -> Result<String, String> {
        let q = a.get("query").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty()).ok_or("find needs \"query\": what to look for, in words (\"a woman pouring coffee\", \"a red car at night\")")?;
        let clips = self.object_clips(a)?;
        let store = ModelStore::default();
        ensure(&store, ModelId::SiglipBase224, true)?;
        let clip = PictureTextEmbedder::load(&store).map_err(|e| format!("the picture-text model is not ready: {e}"))?;
        let text = clip.embed_texts(&[format!("a photo of {}", strip_article(q)), q.to_owned()]).map_err(|e| e.to_string())?;
        // The sentence as written and as a caption; the better of the two.
        let top = a.get("max").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(5).clamp(1, 12);
        let c = cache();
        let mut hits: Vec<(f32, u32, f32, f32)> = Vec::new(); // score, clip, footage t, timeline t
        let (mut indexed, mut skipped) = (0usize, 0usize);
        let limit = a.get("max_shots").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(12);
        for &id in &clips {
            let shot = self.read_shot(id)?;
            let key = FrameIndex::cache_key(&shot.key(""), &format!("{}x{}", EMBED_SIDE, EMBED_SIDE));
            let ix = match FrameIndex::load(&c, &key) {
                Some(ix) => ix,
                None if indexed >= limit => {
                    skipped += 1;
                    continue;
                }
                None => {
                    indexed += 1;
                    let mut src = RgbaFrames::open(&shot.footage, analysis_size(shot.footage.width, shot.footage.height, EMBED_SIDE), &shot.range)?;
                    let n = (shot.range.end - shot.range.start) as usize;
                    let ix = FrameIndex::build(&mut src, 0..n, &clip, &JobCtx::detached()).map_err(|e| format!("could not read the frames: {e}"))?;
                    let _ = ix.store(&c, &key);
                    ix
                }
            };
            for te in &text {
                for mo in ix.search(te, 3, 1.0) {
                    hits.push((mo.score, id, mo.t, shot.clock.tl(mo.t)));
                }
            }
        }
        hits.sort_by(|x, y| y.0.total_cmp(&x.0));
        let mut kept: Vec<(f32, u32, f32, f32)> = Vec::new();
        for h in hits {
            if kept.len() >= top {
                break;
            }
            if kept.iter().all(|k| k.1 != h.1 || (k.3 - h.3).abs() >= 1.0) {
                kept.push(h);
            }
        }
        if kept.is_empty() {
            return Err("no frames to search".into());
        }
        let mut out = vec![format!("moments matching \"{q}\" (timeline seconds; cosine similarity, 0.10 and above is a strong match):")];
        for (score, id, _, tl) in &kept {
            let strength = studio_ai::features::embed::match_strength(*score);
            out.push(format!("  {tl:.1}s · clip {id} · {strength} ({score:.2})"));
        }
        // Objects of that name where a map is already there.
        let o = BuildOptions::default();
        let word = strip_article(q).to_lowercase();
        let mut named: Vec<String> = Vec::new();
        for &id in &clips {
            if let Ok(shot) = self.read_shot(id)
                && let Some(mp) = self.object_map_cached(&shot, 0.0, &o)
            {
                let objs = mp.named(&word);
                if let Some(first) = objs.first() {
                    named.push(format!("clip {id} #{} {} ({:.1}-{:.1}s)", first.id, words::name_of(first), shot.clock.tl(first.enter), shot.clock.tl(first.leave)));
                }
            }
        }
        if !named.is_empty() {
            out.push(format!("objects with that name: {}", named.join("; ")));
        }
        if skipped > 0 {
            out.push(format!("{skipped} shot(s) not searched yet (each takes seconds the first time): clips: [ids] or max_shots"));
        }
        Ok(out.join("\n"))
    }

    // --------------------------------------------------------------- mask

    /// SAM cuts the object at `ft`; the answer carries the pick for the
    /// cut-out and tracking tools.
    fn object_mask(&mut self, a: &Value, shot: &Shot, map: &ObjectMap, o: &Object, tl: f64, ft: f32) -> Result<String, String> {
        let ob = o.at(ft).ok_or_else(|| format!("#{} {} is not in view at {tl:.2}s (it is in view {:.1}-{:.1}s)", o.id, words::name_of(o), shot.clock.tl(o.enter), shot.clock.tl(o.leave)))?;
        let store = ModelStore::default();
        ensure(&store, ModelId::Sam21Tiny, true)?;
        let sam = studio_ai::features::segment::ObjectCutout::load(&store).map_err(|e| e.to_string())?;
        let mut frames = shot.frames()?;
        let fr = shot.frame_at(tl);
        let frame = frames.get(shot.range.start + fr as u64)?;
        let (w, h) = (frame.width as f32, frame.height as f32);
        let emb = sam.embed(&frame).map_err(|e| e.to_string())?;
        let b = ob.bbox.grown(1.04).clamped();
        let sel = sam.segment(&emb, &[studio_ai::features::segment::Prompt::Box { x0: b.x * w, y0: b.y * h, x1: b.right() * w, y1: b.bottom() * h }]).map_err(|e| e.to_string())?;
        let canvas = self.object_canvas(shot, tl);
        let cov = sel.mask.coverage();
        let name = format!("#{} {}", o.id, words::name_of(o));
        if !sel.present || cov < 0.0005 {
            return Ok(format!("{name}: the cutting model found nothing to cut at {tl:.2}s inside its box; try another moment (where shows the box)"));
        }
        let (x0, y0, x1, y1) = sel.mask.bbox().ok_or("empty mask")?;
        let (mw, mh) = (sel.mask.width as f32, sel.mask.height as f32);
        let corner = |x: f32, y: f32| {
            let c = canvas(x / mw, y / mh);
            [c.0.round(), c.1.round()]
        };
        let (c0, c1) = (corner(x0 as f32, y0 as f32), corner(x1 as f32, y1 as f32));
        let (px, py) = interior(&sel.mask);
        let p = corner(px as f32 + 0.5, py as f32 + 0.5);
        let at = (tl * 1000.0).round() / 1000.0;
        let cut = json!({"op": "cut_out", "clip": shot.id, "subject": "object", "at": at, "point": p, "box": [[c0[0].min(c1[0]), c0[1].min(c1[1])], [c0[0].max(c1[0]), c0[1].max(c1[1])]]});
        let track = json!({"op": "track", "clip": shot.id, "point": p, "at": at, "name": format!("{}", words::name_of(o))});
        let mut s = format!(
            "{name} at {tl:.2}s: cut {:.1}% of the picture, model confidence {:.0}%, tight box [{}, {}]-[{}, {}], a point inside it at [{}, {}] (canvas px from the centre).",
            cov * 100.0,
            sel.score * 100.0,
            c0[0].min(c1[0]),
            c0[1].min(c1[1]),
            c0[0].max(c1[0]),
            c0[1].max(c1[1]),
            p[0],
            p[1]
        );
        let _ = map;
        if a.get("apply").and_then(Value::as_bool) == Some(true) {
            let r = self.edit(std::slice::from_ref(&cut), &format!("Cut out {}", o.label), false)?;
            let note = r.get("notes").map(|n| n.to_string()).unwrap_or_default();
            s += &format!(" Cut out applied to clip {}.{}", shot.id, if note.is_empty() { String::new() } else { format!(" {note}") });
        } else {
            s += &format!("\nTo cut it out: edit ops [{cut}] (add \"invert\": true to keep the background). To follow it: {track}. To draw an editable outline: the same fields with op \"roto\". `apply: true` here runs the cut-out.");
        }
        Ok(s)
    }
}

/// The object(s) a request names: an id (`3`, `"#3"`) or a name.
fn resolve<'a>(map: &'a ObjectMap, v: Option<&Value>) -> Result<Vec<&'a Object>, String> {
    let v = v.ok_or("name the object: its id (#3) or what it is (\"dog\")")?;
    let id = match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().trim_start_matches('#').parse::<u64>().ok(),
        _ => None,
    };
    if let Some(id) = id {
        return map.object(id as u32).map(|o| vec![o]).ok_or_else(|| format!("no object #{id} in that shot (ids: {})", map.objects.iter().map(|o| format!("#{}", o.id)).collect::<Vec<_>>().join(" ")));
    }
    let name = v.as_str().ok_or("object is an id (3 or \"#3\") or a name (\"dog\")")?;
    let mut found = map.named(name);
    if found.is_empty() {
        return Err(format!("nothing called \"{name}\" in that shot; it has: {}", words::brief(map, 10)));
    }
    found.sort_by(|a, b| b.prominence().total_cmp(&a.prominence()));
    Ok(found)
}

fn strip_article(q: &str) -> &str {
    let q = q.trim();
    for p in ["a ", "an ", "the ", "A ", "An ", "The "] {
        if let Some(r) = q.strip_prefix(p) {
            return r;
        }
    }
    q
}

/// The most interior pixel of a mask: where a box filter of the mask peaks.
fn interior(mask: &studio_ai::Mask) -> (usize, usize) {
    let (w, h) = (mask.width, mask.height);
    let r = (w.min(h) / 40).max(2);
    let on: Vec<f32> = mask.data.iter().map(|&v| if v > 0.5 { 1.0 } else { 0.0 }).collect();
    let mean = studio_ai::frame::box_mean(&on, w, h, r);
    let mut best = (0.0f32, (w / 2, h / 2));
    for y in 0..h {
        for x in 0..w {
            if on[y * w + x] > 0.5 && mean[y * w + x] > best.0 {
                best = (mean[y * w + x], (x, y));
            }
        }
    }
    best.1
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::agent::Content;
    use crate::services;
    use crate::services::tracking::test_movie::{self, FPS, H, W};
    use studio_ai::objects::{BBox, CamPose, CameraMotion, Kind, Obs, Scale};

    /// A crowded map of `n` objects over a three-second shot, as the
    /// builder would leave it.
    pub fn sample_map(n: usize) -> ObjectMap {
        let labels = ["person", "dog", "chair", "lamp", "cup", "table", "laptop", "bottle", "book", "plant"];
        let mut objects = Vec::new();
        for i in 0..n {
            let (cx, cy) = ((i % 10) as f32 * 0.09 + 0.06, (i / 10) as f32 * 0.12 + 0.1);
            let obs: Vec<Obs> = (0..4)
                .map(|k| {
                    let mut o = Obs::new(k as f32, BBox::new(cx - 0.03 + 0.01 * k as f32, cy - 0.05, 0.06, 0.1), k % 2 == 0, 0.8);
                    o.z = Some(2.0 + (i % 7) as f32);
                    o
                })
                .collect();
            objects.push(Object {
                id: i as u32 + 1,
                label: labels[i % labels.len()].into(),
                kind: if i % 10 == 0 { Kind::Person } else { Kind::Thing },
                descriptor: (i % 10 == 0).then(|| "woman".to_string()),
                score: 0.4 + (i % 6) as f32 * 0.1,
                also: vec![("thing".into(), 0.2)],
                enter: 0.0,
                leave: 3.0,
                enters: i % 4 == 1,
                leaves: i % 5 == 2,
                colours: vec!["red".into(), "white".into()],
                texts: vec!["SALE".into()],
                size: Some([0.5, 1.2]),
                facing: Some("toward the camera".into()),
                movement: None,
                obs,
            });
        }
        ObjectMap {
            version: studio_ai::objects::VERSION,
            width: W,
            height: H,
            fps: FPS as f32,
            duration: 3.0,
            keyframes: vec![0.5, 1.5, 2.5],
            vocabulary: vec!["person".into()],
            objects,
            texts: vec![studio_ai::objects::TextItem { t: 0.5, text: "SALE".into(), bbox: BBox::new(0.1, 0.1, 0.2, 0.05), score: 0.9, object: None }],
            camera: CameraMotion { path: vec![CamPose::default()], summary: "static".into() },
            scale: Scale { anchors: 1, hfov_deg: 63.0 },
            detail: Vec::new(),
        }
    }

    impl Session {
        /// Files `map` as the object map of clip `id`, as a read would.
        pub(in crate::agent) fn inject_objects(&mut self, id: u32, map: &ObjectMap) {
            let shot = self.read_shot(id).expect("a picture clip");
            let o = BuildOptions::default();
            let key = ObjectMap::cache_key(&shot.key(""), &format!("{}|{}x{}", o.cache_params(), shot.size.0, shot.size.1));
            map.store(&cache(), &key).expect("cache");
        }
    }

    /// A session with the test shot on the timeline and `map` as its objects.
    pub fn session_with_objects(map: &ObjectMap) -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Objects", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.inject_objects(id, map);
        (s, id)
    }

    fn ask(s: &mut Session, args: Value) -> String {
        match s.run("objects", &args).unwrap_or_else(|e| panic!("objects {args}: {e}")).remove(0) {
            Content::Text(t) => t,
            Content::Jpeg(_) => panic!("a picture"),
        }
    }

    #[test]
    fn the_list_names_what_is_in_the_shot() {
        let (mut s, id) = session_with_objects(&sample_map(12));
        let r = ask(&mut s, json!({"clip": id}));
        assert!(r.contains("12 objects") && r.contains("#1 person (woman)"), "{r}");
        // At a moment: times are the timeline's (the clip starts at 0).
        let r = ask(&mut s, json!({"t": 1.0}));
        assert!(r.contains("most prominent") || r.contains("12 objects"), "{r}");
        let none = ask(&mut s, json!({"clip": id, "t": 1.0, "action": "list", "max": 3}));
        assert!(none.lines().count() <= 6, "{none}");
    }

    #[test]
    fn where_gives_a_box_in_canvas_pixels() {
        let (mut s, id) = session_with_objects(&sample_map(12));
        let r = ask(&mut s, json!({"clip": id, "object": "#1", "t": 0.0}));
        // Object 1: box x 0.03..0.09, y 0.05..0.15 of a 640 x 360 picture.
        assert!(r.contains("#1 woman at 0.0s") && r.contains("box [-301, -162]-[-262, -126]"), "{r}");
        let by_name = ask(&mut s, json!({"clip": id, "object": "dog", "t": 1.0}));
        assert!(by_name.contains("dog"), "{by_name}");
        let e = s.run("objects", &json!({"clip": id, "object": "unicorn", "t": 1.0})).err().expect("an error");
        assert!(e.contains("nothing called \"unicorn\"") && e.contains("it has:"), "{e}");
        let e = s.run("objects", &json!({"clip": id, "object": 99, "t": 1.0})).err().expect("an error");
        assert!(e.contains("no object #99"), "{e}");
    }

    #[test]
    fn when_around_space_and_text_answer() {
        let (mut s, id) = session_with_objects(&sample_map(12));
        let w = ask(&mut s, json!({"clip": id, "object": "dog"}));
        assert!(w.contains("dog") && (w.contains("in view") || w.contains("whole")), "{w}");
        let a = ask(&mut s, json!({"clip": id, "action": "around", "object": 2, "t": 1.0}));
        assert!(a.starts_with("around #2"), "{a}");
        let sp = ask(&mut s, json!({"clip": id, "action": "space", "t": 0.5, "to": 2.0}));
        assert!(sp.starts_with("free space between") && sp.contains("px"), "{sp}");
        let tx = ask(&mut s, json!({"clip": id, "action": "text"}));
        assert!(tx.contains("\"SALE\""), "{tx}");
    }

    #[test]
    fn an_unknown_action_says_the_choices() {
        let (mut s, id) = session_with_objects(&sample_map(3));
        let e = s.run("objects", &json!({"clip": id, "action": "wander"})).err().expect("an error");
        assert!(e.contains("where") && e.contains("find") && e.contains("mask"), "{e}");
        let e = s.run("objects", &json!({"clip": 9999})).err().expect("an error");
        assert!(e.contains("No clip 9999"), "{e}");
    }

    #[test]
    fn find_needs_the_picture_text_model_and_says_which() {
        let (mut s, id) = session_with_objects(&sample_map(3));
        let e = s.run("objects", &json!({"clip": id, "query": "a woman pouring coffee"}));
        // Without the model installed it names it; with it installed it answers.
        match e {
            Err(e) => assert!(e.contains("SigLIP") && e.contains("Settings"), "{e}"),
            Ok(_) => assert!(studio_ai::ModelStore::default().is_installed(ModelId::SiglipBase224)),
        }
        let e = s.run("objects", &json!({"clip": id, "action": "find"})).err().expect("an error");
        assert!(e.contains("query"), "{e}");
    }

    #[test]
    fn see_adds_the_object_list_from_the_map() {
        let (mut s, id) = session_with_objects(&sample_map(12));
        let r = match s.run("see", &json!({})).unwrap().remove(0) {
            Content::Text(t) => t,
            _ => panic!(),
        };
        assert!(r.contains("objects: ") && r.contains("dog"), "{r}");
        assert!(r.contains(&format!("#{id}")), "{r}");
        let off = match s.run("see", &json!({"objects": false})).unwrap().remove(0) {
            Content::Text(t) => t,
            _ => panic!(),
        };
        assert!(!off.contains("objects:"), "{off}");
    }

    fn have_sam() -> bool {
        let ok = ModelStore::default().is_installed(ModelId::Sam21Tiny);
        if !ok {
            eprintln!("SAM 2.1 is not installed: skipped");
        }
        ok
    }

    /// The test shot's square at frame 6, as an object the map knows.
    fn square_map() -> ObjectMap {
        let (ox, oy) = test_movie::object(6);
        let (sx, sy) = test_movie::shake(6);
        let (cx, cy) = (ox + sx, oy + sy);
        let mut m = sample_map(0);
        let b = BBox::new(((cx - 34.0) / W as f64) as f32, ((cy - 34.0) / H as f64) as f32, 68.0 / W as f32, 68.0 / H as f32);
        let mut o = sample_map(1).objects.remove(0);
        o.label = "square".into();
        o.descriptor = None;
        o.kind = Kind::Thing;
        o.enter = 0.0;
        o.leave = 2.0;
        o.obs = vec![Obs::new(0.0, b, true, 0.9), Obs::new(2.0, b, true, 0.9)];
        m.objects.push(o);
        m
    }

    #[test]
    fn mask_cuts_the_object_and_hands_over_ops() {
        if !have_sam() {
            return;
        }
        let (mut s, id) = session_with_objects(&square_map());
        let r = ask(&mut s, json!({"clip": id, "action": "mask", "object": "square", "t": 6.0 / 30.0}));
        // The square is 56 x 56 of 640 x 360: about 1.4 % of the picture.
        let pct: f32 = r.split("cut ").nth(1).and_then(|x| x.split('%').next()).and_then(|x| x.trim().parse().ok()).unwrap_or(-1.0);
        assert!(pct > 0.7 && pct < 3.0, "{r}");
        assert!(r.contains("\"op\":\"cut_out\"") && r.contains("\"op\":\"track\"") && r.contains("\"box\""), "{r}");
        // The point it hands over is on the square.
        let (ox, oy) = test_movie::object(6);
        let (sx, sy) = test_movie::shake(6);
        let (want_x, want_y) = (ox + sx - W as f64 / 2.0, oy + sy - H as f64 / 2.0);
        let ops: Vec<Value> = r.lines().filter_map(|l| l.split("edit ops [").nth(1)).filter_map(|l| l.split("] (add").next()).filter_map(|l| serde_json::from_str(l).ok()).collect();
        let p = ops[0]["point"].as_array().expect("a point");
        assert!((p[0].as_f64().unwrap() - want_x).abs() < 20.0 && (p[1].as_f64().unwrap() - want_y).abs() < 20.0, "{p:?} vs {want_x},{want_y}");
    }

    #[test]
    fn the_clock_maps_footage_time_to_the_timeline() {
        let c = Clock { start: 2_000_000, dur: 4_000_000, a: 10_000_000, b: 12_000_000 - 1, first_s: 10.0, still: false };
        // Half speed: two seconds of footage over four of timeline.
        assert!((c.tl(0.0) - 2.0).abs() < 1e-3 && (c.tl(1.0) - 4.0).abs() < 1e-2 && (c.tl(2.0) - 6.0).abs() < 1e-2, "{} {} {}", c.tl(0.0), c.tl(1.0), c.tl(2.0));
        let still = Clock { still: true, ..c };
        assert_eq!(still.tl(5.0), 2.0);
        // A reversed clip runs the other way.
        let rev = Clock { a: 12_000_000 - 1, b: 10_000_000, ..c };
        assert!(rev.tl(0.0) > rev.tl(1.0));
    }

    #[test]
    fn names_and_ids_resolve() {
        let m = sample_map(12);
        assert_eq!(resolve(&m, Some(&json!(3))).unwrap()[0].id, 3);
        assert_eq!(resolve(&m, Some(&json!("#4"))).unwrap()[0].id, 4);
        assert_eq!(resolve(&m, Some(&json!("people"))).unwrap()[0].label, "person");
        assert!(resolve(&m, None).is_err());
        assert_eq!(strip_article("a red car"), "red car");
    }
}
