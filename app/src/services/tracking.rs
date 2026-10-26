// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion tracking and stabilization, shared by the canvas tools, the
//! Inspector and the AI editor.
//!
//! A clip's tracks live in its `meta["tracks"]`: per source frame, where
//! the tracked point (with two nearby points that carry its turn and
//! scale) or the four corners of a plane are, in 0..1 coordinates of the
//! source picture, with how sure the tracker was. Being in source terms,
//! a track survives trims, moves, speed changes and the clip's own
//! transform.
//!
//! Another layer follows a track (`meta["follow"]` on it): its position,
//! its whole transform, its corner pin (an insert on a screen or a sign) or
//! one of its masks gets a key on every frame. A correction or re-track
//! updates the followers.
//!
//! Stabilizing measures the camera's own motion, plans a smoothed or locked
//! path ([`studio_ai::features::track::plan_stabilize`]) and writes the
//! correction as keys on the clip's position, rotation and scale, with the
//! zoom that hides the moving edges. `meta["stabilize"]` keeps the settings
//! and the transform it started from so it can be redone or removed.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use eframe::egui::Vec2;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use studio_ai::features::track::{self as tk, Gray, Mat3, Motion, Region, StabilizeSettings, TrackSettings};
use studio_ai::JobCtx;
use studio_model as m;

use crate::editor::pins::LayerMap;
use crate::editor::xform;
use crate::view::{self, ClipKind, Time, SEC};

/// Longest side tracking looks at (pixels): enough for sub-pixel accuracy
/// on the canvas, cheap to follow.
pub const ANALYSIS: usize = 960;

/// What a track follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A point and the picture around it (moves, turns and scales).
    Point,
    /// A flat surface's four corners (perspective).
    Plane,
}

/// One frame of a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Points in 0..1 source coordinates: a point track's point and two
    /// helpers (to its right and below it), or a plane's four corners.
    pub p: Vec<[f32; 2]>,
    /// 0..1 confidence.
    pub c: f32,
    /// Not found in this frame (the points are where it was last seen).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lost: bool,
}

/// A tracked point or plane on one clip's footage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    pub kind: Kind,
    /// Source frame rate the frames are counted at.
    pub fps: f64,
    /// Source frame of `samples[0]`.
    pub first: u64,
    /// A point track's region radius, as a fraction of the source width.
    #[serde(default)]
    pub radius: f64,
    /// How the region may change (`translation`, `similarity`,
    /// `perspective`).
    #[serde(default = "similarity")]
    pub motion: String,
    pub samples: Vec<Sample>,
    /// Source frames placed by hand (corrections).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<u64>,
}

fn similarity() -> String {
    "similarity".into()
}

impl Track {
    pub fn last(&self) -> u64 {
        self.first + self.samples.len().saturating_sub(1) as u64
    }

    fn sample(&self, frame: u64) -> Option<&Sample> {
        self.samples.get(frame.checked_sub(self.first)? as usize)
    }

    /// The points at a fractional source frame (between two samples,
    /// interpolated; outside the track, the nearest end), and the
    /// confidence there.
    pub fn points_at(&self, f: f64) -> Option<(Vec<[f64; 2]>, f32, bool)> {
        if self.samples.is_empty() {
            return None;
        }
        let rel = (f - self.first as f64).clamp(0.0, (self.samples.len() - 1) as f64);
        let (i, k) = (rel.floor() as usize, rel - rel.floor());
        let a = &self.samples[i];
        let b = self.samples.get(i + 1).unwrap_or(a);
        let p = a.p.iter().zip(&b.p).map(|(x, y)| [x[0] as f64 + (y[0] - x[0]) as f64 * k, x[1] as f64 + (y[1] - x[1]) as f64 * k]).collect();
        let lost = if k < 0.5 { a.lost } else { b.lost };
        Some((p, a.c.min(b.c), lost))
    }

    /// Mean confidence and how many frames were lost.
    pub fn quality(&self) -> (f32, usize) {
        let n = self.samples.len().max(1) as f32;
        (self.samples.iter().map(|s| s.c).sum::<f32>() / n, self.samples.iter().filter(|s| s.lost).count())
    }

    fn motion(&self) -> Motion {
        match self.motion.as_str() {
            "translation" => Motion::Translation,
            "perspective" => Motion::Perspective,
            _ => Motion::Similarity,
        }
    }
}

/// The tracks stored on a clip.
pub fn tracks(mc: &m::Clip) -> Vec<Track> {
    mc.meta.get("tracks").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

pub fn set_tracks(mc: &mut m::Clip, list: &[Track]) {
    if list.is_empty() {
        mc.meta.remove("tracks");
    } else {
        mc.meta.insert("tracks".into(), serde_json::to_value(list).unwrap_or(Json::Null));
    }
}

pub fn track_of(mc: &m::Clip, id: u32) -> Option<Track> {
    tracks(mc).into_iter().find(|t| t.id == id)
}

/// The footage behind a clip.
#[derive(Clone, Debug)]
pub struct Footage {
    pub path: PathBuf,
    pub fps: f64,
    pub frames: u64,
    /// Source picture size.
    pub width: u32,
    pub height: u32,
}

pub fn footage(proj: &view::Project, mc: &m::Clip) -> Result<Footage, String> {
    let m::ClipSource::Media(r) = &mc.source else { return Err("only a video clip can be tracked (this one is a title, shape or nested clip)".into()) };
    let mi = proj.media(r.media.0 as u32).ok_or("the clip's media is gone")?;
    match mi.kind {
        view::MediaKind::Audio => return Err("a sound has no picture to track".into()),
        view::MediaKind::Image => return Err("a still picture does not move: nothing to track".into()),
        view::MediaKind::Video => {}
    }
    let path = mi.path.clone().ok_or("sample media has no file to track")?;
    let src = studio_render::source::open_path(&path)?;
    let info = src.info();
    let fps = info.frame_rate.unwrap_or(30.0).max(1.0);
    let frames = info.frame_count.unwrap_or(1).max(1);
    Ok(Footage { path, fps, frames, width: mi.width.max(1), height: mi.height.max(1) })
}

/// The source frames a clip shows (first..=last), from its timeline span.
pub fn used_frames(mc: &m::Clip, f: &Footage) -> Range<u64> {
    let a = mc.source_time_at(mc.start);
    let b = mc.source_time_at(mc.start + mc.duration - 1);
    let (lo, hi) = (a.min(b), a.max(b));
    let fr = |t: m::Micros| (t as f64 / SEC as f64 * f.fps).floor().max(0.0) as u64;
    let first = fr(lo).min(f.frames - 1);
    let last = fr(hi).min(f.frames - 1).max(first);
    first..last + 1
}

/// The fractional source frame a clip shows at timeline time `t`.
pub fn frame_at(mc: &m::Clip, fps: f64, t: Time) -> f64 {
    mc.source_time_at(t) as f64 / SEC as f64 * fps
}

/// Analysis picture size for footage `w`×`h`.
pub fn analysis_size(w: u32, h: u32) -> (usize, usize) {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let k = (ANALYSIS as f64 / w.max(h) as f64).min(1.0);
    (((w as f64 * k).round() as usize).max(16), ((h as f64 * k).round() as usize).max(16))
}

/// Box-filtered luma of a decoded picture at `w`×`h`.
pub fn luma(pic: &studio_render::yuv::DecodedPicture, w: usize, h: usize) -> Gray {
    use studio_render::yuv::{PixelData, Planes};
    let (pw, ph) = (pic.width.max(1) as usize, pic.height.max(1) as usize);
    let full: Vec<f32> = match &pic.data {
        PixelData::Yuv { y, .. } => {
            let max = match y {
                Planes::U8(_) => 255.0,
                Planes::U16(_) => ((1u32 << pic.colour.bit_depth.clamp(8, 16)) - 1) as f32,
            };
            (0..pw * ph).map(|i| y.get(i) / max).collect()
        }
        PixelData::Rgba8(px) => px.chunks_exact(4).map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).collect(),
    };
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y * ph / h, ((y + 1) * ph / h).max(y * ph / h + 1).min(ph));
        for x in 0..w {
            let (x0, x1) = (x * pw / w, ((x + 1) * pw / w).max(x * pw / w + 1).min(pw));
            let mut s = 0.0;
            for yy in y0..y1 {
                let row = &full[yy * pw..yy * pw + pw];
                s += row[x0..x1].iter().sum::<f32>();
            }
            out[y * w + x] = s / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    Gray { width: w, height: h, data: out }
}

/// Luma frames of a file, decoded in order and kept for a while (the
/// tracker walks backwards too).
pub struct Frames {
    src: Box<dyn studio_render::source::MediaSource>,
    size: (usize, usize),
    cache: BTreeMap<u64, Gray>,
    keep: usize,
}

impl Frames {
    pub fn open(f: &Footage) -> Result<Frames, String> {
        let src = studio_render::source::open_path(&f.path)?;
        Ok(Frames { src, size: analysis_size(f.width, f.height), cache: BTreeMap::new(), keep: 96 })
    }

    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    pub fn get(&mut self, i: u64) -> Result<Gray, String> {
        if let Some(g) = self.cache.get(&i) {
            return Ok(g.clone());
        }
        let (w, h) = self.size;
        let lo = i.saturating_sub(self.keep as u64 - 1);
        let mut got: BTreeMap<u64, Gray> = BTreeMap::new();
        let ok = self.src.decode_to(i, &mut |k, pic| {
            if k >= lo && k <= i {
                got.insert(k, luma(&pic, w, h));
            }
        });
        if !ok && !got.contains_key(&i) {
            return Err(format!("frame {i} could not be decoded"));
        }
        self.cache.extend(got);
        while self.cache.len() > self.keep * 2 {
            // Drop the frames farthest from the one asked for.
            let far = *self.cache.keys().max_by_key(|k| k.abs_diff(i)).expect("not empty");
            self.cache.remove(&far);
        }
        self.cache.get(&i).cloned().ok_or_else(|| format!("frame {i} could not be decoded"))
    }
}

/// A tracking run worked out ahead (all it needs, off the UI thread).
#[derive(Clone, Debug)]
pub struct TrackJob {
    pub clip: u32,
    /// Track to write into (`None`: a new one).
    pub track: Option<u32>,
    pub kind: Kind,
    pub name: String,
    pub footage: Footage,
    /// Source frames to track over.
    pub range: Range<u64>,
    /// Frame the region is drawn on.
    pub start: u64,
    /// The region in analysis pixels.
    pub region: Region,
    pub motion: Motion,
    pub backwards: bool,
}

/// Samples worked out by a [`TrackJob`], by source frame.
pub type Samples = BTreeMap<u64, Sample>;

impl TrackJob {
    /// Runs the tracker (seconds for a short clip; cancellable).
    pub fn run(&self, ctx: &JobCtx) -> Result<Samples, String> {
        let mut frames = Frames::open(&self.footage)?;
        let (w, h) = frames.size();
        let norm = |p: (f64, f64)| [(p.0 / w as f64) as f32, (p.1 / h as f64) as f32];
        let region = self.region.clone();
        let helpers: Vec<(f64, f64)> = match &region {
            Region::Point { x, y, radius } => vec![(*x, *y), (x + radius, *y), (*x, y + radius)],
            Region::Plane(c) => c.to_vec(),
        };
        let mut out = Samples::new();
        let settings = TrackSettings { motion: self.motion, ..TrackSettings::default() };
        let base = self.range.start;
        let mut get = |i: usize| frames.get(base + i as u64).map_err(|e| tk::frame_error(i, e));
        let r = (self.range.start - base) as usize..(self.range.end - base) as usize;
        tk::track(&mut get, r, (self.start - base) as usize, region, settings, self.backwards, ctx, &mut |s| {
            out.insert(base + s.frame as u64, Sample { p: helpers.iter().map(|p| norm(tk::apply(&s.h, *p))).collect(), c: (s.confidence * 1000.0).round() / 1000.0, lost: s.lost });
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        Ok(out)
    }
}

/// Canvas points → source pixels of clip `id` at `t`.
fn to_source(proj: &view::Project, model: &m::Project, id: u32, t: Time, c: Vec2) -> Result<Vec2, String> {
    let geo = xform::geo(proj, model, id, t).ok_or("the clip draws nothing there")?;
    geo.to_source(c).ok_or_else(|| "that point is not on the clip's picture".to_owned())
}

/// Sets up tracking a new point or plane drawn on the canvas at `t`:
/// `points` is the point (one) or the plane's corners (four, in order
/// around it), canvas pixels from the centre; `radius` is how much picture
/// around a point moves with it (canvas pixels).
pub fn plan_new(proj: &view::Project, model: &m::Project, id: u32, kind: Kind, t: Time, points: &[Vec2], radius: f32, motion: Option<Motion>, name: &str) -> Result<TrackJob, String> {
    let mc = model.active().clip(m::ClipId(id as u64)).ok_or("no such clip")?;
    let vc = proj.clip(id).ok_or("no such clip")?;
    if t < vc.start || t >= vc.start + vc.dur {
        return Err(format!("the clip is on screen from {:.2}s to {:.2}s; track it there", vc.start as f64 / SEC as f64, (vc.start + vc.dur) as f64 / SEC as f64));
    }
    let f = footage(proj, mc)?;
    let range = used_frames(mc, &f);
    let start = (frame_at(mc, f.fps, t).round() as u64).clamp(range.start, range.end - 1);
    let (w, h) = analysis_size(f.width, f.height);
    let k = w as f64 / f.width as f64;
    let src: Vec<(f64, f64)> = points.iter().map(|c| to_source(proj, model, id, t, *c).map(|s| (s.x as f64 * k, s.y as f64 * k))).collect::<Result<_, _>>()?;
    let region = match kind {
        Kind::Point => {
            let [p] = src.as_slice() else { return Err("a point track starts from one point".into()) };
            // The radius on the canvas, in analysis pixels.
            let geo = xform::geo(proj, model, id, t).ok_or("the clip draws nothing there")?;
            let per = (geo.fit() * xform::xf_at(mc, t - vc.start).sx.abs()).max(1e-3) as f64;
            let r = (radius.max(4.0) as f64 / per * k).clamp(6.0, w.min(h) as f64 / 3.0);
            Region::Point { x: p.0, y: p.1, radius: r }
        }
        Kind::Plane => {
            let [a, b, c, d] = src.as_slice() else { return Err("a plane track starts from its four corners".into()) };
            Region::Plane([*a, *b, *c, *d])
        }
    };
    for p in region.points() {
        if p.0 < 0.0 || p.1 < 0.0 || p.0 > w as f64 || p.1 > h as f64 {
            return Err("the region is off the clip's picture".into());
        }
    }
    let motion = motion.unwrap_or(match kind {
        Kind::Point => Motion::Similarity,
        Kind::Plane => Motion::Perspective,
    });
    Ok(TrackJob { clip: id, track: None, kind, name: name.to_owned(), footage: f, range, start, region, motion, backwards: true })
}

/// Sets up a correction: track `track` is put at `points` (canvas pixels,
/// as [`plan_new`] takes them) at `t` and re-tracked from there, forwards
/// up to the next correction (and backwards to the one before when
/// `both_ways`). `points: None` re-tracks from where the track already is.
pub fn plan_fix(proj: &view::Project, model: &m::Project, id: u32, track: u32, t: Time, points: Option<&[Vec2]>, both_ways: bool) -> Result<TrackJob, String> {
    let mc = model.active().clip(m::ClipId(id as u64)).ok_or("no such clip")?;
    let tr = track_of(mc, track).ok_or_else(|| format!("clip {id} has no track {track}"))?;
    let f = footage(proj, mc)?;
    let range = used_frames(mc, &f);
    let start = (frame_at(mc, tr.fps, t).round() as u64).clamp(range.start, range.end - 1);
    let (w, h) = analysis_size(f.width, f.height);
    let k = w as f64 / f.width as f64;
    let src: Vec<(f64, f64)> = match points {
        Some(pts) => pts.iter().map(|c| to_source(proj, model, id, t, *c).map(|s| (s.x as f64 * k, s.y as f64 * k))).collect::<Result<_, _>>()?,
        None => {
            let (p, _, _) = tr.points_at(start as f64).ok_or("the track is empty")?;
            p.iter().map(|q| (q[0] * w as f64, q[1] * h as f64)).collect()
        }
    };
    let region = match tr.kind {
        Kind::Point => {
            let p = src.first().copied().ok_or("a correction needs the point")?;
            Region::Point { x: p.0, y: p.1, radius: (tr.radius * w as f64).max(6.0) }
        }
        Kind::Plane => {
            let [a, b, c, d] = src.as_slice() else { return Err("a plane is corrected with its four corners".into()) };
            Region::Plane([*a, *b, *c, *d])
        }
    };
    let next = tr.fixes.iter().copied().filter(|&x| x > start).min().unwrap_or(range.end).min(range.end);
    let prev = tr.fixes.iter().copied().filter(|&x| x < start).max().map_or(range.start, |x| x + 1).max(range.start);
    let lo = if both_ways { prev } else { start };
    Ok(TrackJob { clip: id, track: Some(track), kind: tr.kind, name: tr.name.clone(), footage: f, range: lo..next.max(start + 1), start, region, motion: tr.motion(), backwards: both_ways, })
}

/// Writes a finished run into the clip's tracks; returns the track id.
pub fn store(mc: &mut m::Clip, job: &TrackJob, samples: &Samples, fixed: bool) -> u32 {
    let mut list = tracks(mc);
    let (w, _) = analysis_size(job.footage.width, job.footage.height);
    let radius = match &job.region {
        Region::Point { radius, .. } => radius / w as f64,
        Region::Plane(_) => 0.0,
    };
    let motion = match job.motion {
        Motion::Translation => "translation",
        Motion::Similarity => "similarity",
        Motion::Perspective => "perspective",
    };
    let id = job.track.unwrap_or_else(|| list.iter().map(|t| t.id).max().unwrap_or(0) + 1);
    let mut tr = list.iter().find(|t| t.id == id).cloned().unwrap_or_else(|| Track {
        id,
        name: if job.name.is_empty() { format!("{} {id}", if job.kind == Kind::Point { "Point" } else { "Plane" }) } else { job.name.clone() },
        kind: job.kind,
        fps: job.footage.fps,
        first: job.range.start,
        radius,
        motion: motion.into(),
        samples: Vec::new(),
        fixes: Vec::new(),
    });
    // Merge by frame.
    let mut by: BTreeMap<u64, Sample> = tr.samples.iter().enumerate().map(|(i, s)| (tr.first + i as u64, s.clone())).collect();
    by.extend(samples.iter().map(|(k, v)| (*k, v.clone())));
    if let (Some(&a), Some(&b)) = (by.keys().next(), by.keys().next_back()) {
        tr.first = a;
        // Gaps (never tracked) hold the frame before.
        let mut out = Vec::with_capacity((b - a + 1) as usize);
        let mut last: Option<Sample> = None;
        for f in a..=b {
            let s = by.get(&f).cloned().or_else(|| last.clone().map(|s| Sample { lost: true, c: 0.0, ..s }));
            if let Some(s) = s {
                out.push(s.clone());
                last = Some(s);
            }
        }
        tr.samples = out;
    }
    if fixed && !tr.fixes.contains(&job.start) {
        tr.fixes.push(job.start);
        tr.fixes.sort_unstable();
    }
    if radius > 0.0 {
        tr.radius = radius;
    }
    list.retain(|t| t.id != id);
    list.push(tr);
    list.sort_by_key(|t| t.id);
    set_tracks(mc, &list);
    id
}

/// Where a track's points show on the canvas at timeline time `t`
/// (canvas pixels from the centre), with its confidence and whether it is
/// lost there.
pub fn canvas_points(proj: &view::Project, model: &m::Project, id: u32, tr: &Track, t: Time) -> Option<(Vec<Vec2>, f32, bool)> {
    let mc = model.active().clip(m::ClipId(id as u64))?;
    let geo = xform::geo(proj, model, id, t)?;
    let s = geo.source();
    let (p, c, lost) = tr.points_at(frame_at(mc, tr.fps, t))?;
    Some((p.iter().map(|q| geo.to_canvas(Vec2::new(q[0] as f32 * s.x, q[1] as f32 * s.y))).collect(), c, lost))
}

/// How a layer follows a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowMode {
    /// Moves with the tracked point.
    Position,
    /// Moves, turns and scales with it.
    Transform,
    /// Its corners land on the plane's corners (a screen insert).
    Corners,
    /// Mask `id` moves with it.
    Mask(u64),
}

impl FollowMode {
    pub fn parse(s: &str, mask: Option<u64>) -> Result<FollowMode, String> {
        Ok(match s {
            "position" | "move" => FollowMode::Position,
            "transform" | "all" | "position_rotation_scale" => FollowMode::Transform,
            "corners" | "corner_pin" | "insert" | "plane" => FollowMode::Corners,
            "mask" => FollowMode::Mask(mask.ok_or("following with a mask needs which mask")?),
            other => return Err(format!("follow \"{other}\": position (moves with it), transform (moves, turns and scales), corners (pins its corners onto a plane track: screen inserts), mask (a mask moves with it)")),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            FollowMode::Position => "position",
            FollowMode::Transform => "transform",
            FollowMode::Corners => "corners",
            FollowMode::Mask(_) => "mask",
        }
    }
}

/// A follower's link to a track (stored in its `meta["follow"]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Follow {
    pub clip: u32,
    pub track: u32,
    pub mode: FollowMode,
    /// The moment the follower was attached (timeline micros): it stays
    /// where it was then, relative to the track.
    pub at: i64,
    /// The follower's own values at that moment: position, rotation,
    /// scale x/y.
    pub base: [f64; 5],
    /// A mask's points on the canvas at that moment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
}

pub fn follow_of(mc: &m::Clip) -> Option<Follow> {
    mc.meta.get("follow").and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// The canvas motion of a track from `t0` to `t`: a similarity (point
/// tracks) or homography (planes) on canvas pixels.
fn canvas_motion(proj: &view::Project, model: &m::Project, id: u32, tr: &Track, t0: Time, t: Time) -> Option<Mat3> {
    let (a, _, _) = canvas_points(proj, model, id, tr, t0)?;
    let (b, _, _) = canvas_points(proj, model, id, tr, t)?;
    let a: Vec<(f64, f64)> = a.iter().map(|p| (p.x as f64, p.y as f64)).collect();
    let b: Vec<(f64, f64)> = b.iter().map(|p| (p.x as f64, p.y as f64)).collect();
    let motion = match tr.kind {
        Kind::Point => match tr.motion() {
            Motion::Translation => Motion::Translation,
            _ => Motion::Similarity,
        },
        Kind::Plane => Motion::Perspective,
    };
    tk::fit(motion, &a, &b)
}

fn frame_len(proj: &view::Project) -> Time {
    (SEC as f64 / (proj.fps as f64).max(1.0)).round() as Time
}

fn track_keys(keys: Vec<(m::Micros, m::Value)>) -> m::Track {
    m::Track::new(keys.into_iter().map(|(t, v)| m::Keyframe::new(t, v, m::Interp::Linear)).collect())
}

/// Makes clip `follower` follow track `track` of clip `id` from the
/// moment `at`; returns the follower with its new keys (not yet applied).
pub fn attach(proj: &view::Project, model: &m::Project, follower: u32, id: u32, track: u32, mode: FollowMode, at: Time) -> Result<m::Clip, String> {
    if follower == id && !matches!(mode, FollowMode::Mask(_)) {
        return Err("a clip cannot follow its own track (except with a mask): attach another layer, or stabilize this one".into());
    }
    let mc = model.active().clip(m::ClipId(follower as u64)).ok_or_else(|| format!("no clip {follower}"))?;
    let fv = proj.clip(follower).ok_or_else(|| format!("no clip {follower}"))?;
    if matches!(fv.kind, ClipKind::Audio | ClipKind::Fx) {
        return Err(format!("clip {follower} has no picture to move"));
    }
    if mc.parent.is_some() && matches!(mode, FollowMode::Position | FollowMode::Transform) {
        return Err(format!("clip {follower} follows a parent; unparent it first (a layer moves with one thing)"));
    }
    if mc.transform.three_d {
        return Err(format!("clip {follower} is a 3D layer; tracking moves 2D layers"));
    }
    let tracked = model.active().clip(m::ClipId(id as u64)).ok_or_else(|| format!("no clip {id}"))?;
    let tr = track_of(tracked, track).ok_or_else(|| format!("clip {id} has no track {track}"))?;
    if matches!(mode, FollowMode::Corners) && tr.kind != Kind::Plane {
        return Err("corners follow a plane track (four corners); this is a point track: use position or transform".into());
    }
    let local = (at - fv.start).clamp(0, fv.dur);
    let xf = xform::xf_at(mc, local);
    let mut points = Vec::new();
    if let FollowMode::Mask(mk) = mode {
        let mask = mc.masks.iter().find(|k| k.id.0 == mk).ok_or_else(|| format!("clip {follower} has no mask {mk}"))?;
        let map = LayerMap::at(proj, model, follower, at).ok_or("the follower draws nothing there")?;
        let outline = mc.mask_at(mask, local);
        let path = if outline.shape == m::MaskShape::Path { outline.path.clone() } else { outline.outline().ok_or("that mask has no outline to move")? };
        points = path.iter().map(|p| {
            let c = map.flat_canvas(p.point);
            [c.x as f64, c.y as f64]
        }).collect();
    }
    let f = Follow { clip: id, track, mode, at, base: [xf.pos.x as f64, xf.pos.y as f64, xf.rot as f64, xf.sx as f64, xf.sy as f64], points };
    let mut out = mc.clone();
    if let FollowMode::Mask(mk) = f.mode {
        // The mask becomes a free path (its points get the keys).
        if let Some(i) = out.masks.iter().position(|k| k.id.0 == mk)
            && out.masks[i].shape != m::MaskShape::Path
        {
            let mk2 = out.masks[i].clone();
            if let Some(p) = crate::editor::pins::as_path(&mut out, &mk2) {
                out.masks[i] = p;
            }
        }
    }
    out.meta.insert("follow".into(), serde_json::to_value(&f).map_err(|e| e.to_string())?);
    apply_follow(proj, model, out, &f)
}

/// Writes the follower's keys from the track as it is now.
pub fn apply_follow(proj: &view::Project, model: &m::Project, mut out: m::Clip, f: &Follow) -> Result<m::Clip, String> {
    let follower = out.id.0 as u32;
    let fv = proj.clip(follower).ok_or("the follower is gone")?.clone();
    let tracked = model.active().clip(m::ClipId(f.clip as u64)).ok_or("the tracked clip is gone")?;
    let tv = proj.clip(f.clip).ok_or("the tracked clip is gone")?;
    let tr = track_of(tracked, f.track).ok_or("the track is gone")?;
    let step = frame_len(proj);
    let (lo, hi) = (fv.start.max(tv.start), (fv.start + fv.dur).min(tv.start + tv.dur));
    if hi <= lo {
        return Err(format!("clip {follower} and clip {} are never on screen together", f.clip));
    }
    let at = f.at.clamp(lo, hi - 1);
    let mut times = Vec::new();
    let mut t = lo;
    while t < hi {
        times.push(t);
        t += step;
    }
    times.push(hi - 1);
    let mut keys: BTreeMap<String, Vec<(m::Micros, m::Value)>> = BTreeMap::new();
    let mut put = |path: String, t: Time, v: m::Value| keys.entry(path).or_default().push((t - fv.start, v));
    let base_pos = (f.base[0], f.base[1]);
    let p0 = canvas_points(proj, model, f.clip, &tr, at).ok_or("the tracked clip draws nothing there")?.0[0];
    for &t in &times {
        let Some(d) = canvas_motion(proj, model, f.clip, &tr, at, t) else { continue };
        match &f.mode {
            FollowMode::Position => {
                let Some((pts, _, _)) = canvas_points(proj, model, f.clip, &tr, t) else { continue };
                let dp = pts[0] - p0;
                put("transform.position".into(), t, m::Value::Point(m::Vec2::new(base_pos.0 + dp.x as f64, base_pos.1 + dp.y as f64)));
            }
            FollowMode::Transform => {
                let q = tk::apply(&d, base_pos);
                let ang = d[3].atan2(d[0]).to_degrees();
                let s = tk::scale_of(&d);
                put("transform.position".into(), t, m::Value::Point(m::Vec2::new(q.0, q.1)));
                put("transform.rotation".into(), t, m::Value::Angle(f.base[2] + ang));
                put("transform.scale_x".into(), t, m::Value::Scalar(f.base[3] * s));
                put("transform.scale_y".into(), t, m::Value::Scalar(f.base[4] * s));
            }
            FollowMode::Corners => {
                let Some((pts, _, _)) = canvas_points(proj, model, f.clip, &tr, t) else { continue };
                let Some(map) = LayerMap::at(proj, model, follower, t) else { continue };
                if out.corner_pin.is_none() {
                    out.corner_pin = Some(crate::editor::pins::new_corner_pin(&map));
                }
                for (name, c) in m::CornerPin::CORNERS.iter().zip(&pts) {
                    if let Some(l) = map.flat_layer(*c) {
                        put(format!("corner_pin.{name}"), t, m::Value::Point(l));
                    }
                }
            }
            FollowMode::Mask(mk) => {
                let Some(map) = LayerMap::at(proj, model, follower, t) else { continue };
                for (i, p) in f.points.iter().enumerate() {
                    let q = tk::apply(&d, (p[0], p[1]));
                    if let Some(l) = map.flat_layer(Vec2::new(q.0 as f32, q.1 as f32)) {
                        put(format!("mask.{mk}.point.{i}"), t, m::Value::Point(l));
                    }
                }
            }
        }
    }
    if keys.is_empty() {
        return Err("the track gave no positions where the two clips overlap".into());
    }
    for (path, k) in keys {
        out.keyframes.insert(path, track_keys(k));
    }
    Ok(out)
}

/// Clips following any track of clip `id`, with their new keys from the
/// track as it is now (to apply after a correction).
pub fn refreshed_followers(proj: &view::Project, model: &m::Project, id: u32) -> Vec<m::Clip> {
    model
        .active()
        .clips
        .iter()
        .filter_map(|c| follow_of(c).filter(|f| f.clip == id).map(|f| (c.clone(), f)))
        .filter_map(|(c, f)| apply_follow(proj, model, c, &f).ok())
        .collect()
}

/// Stops a clip following a track; its keys stay as they are unless
/// `clear`, which removes them.
pub fn detach(mc: &mut m::Clip, clear: bool) -> bool {
    let Some(f) = follow_of(mc) else { return false };
    mc.meta.remove("follow");
    if clear {
        let paths: Vec<String> = match f.mode {
            FollowMode::Position => vec!["transform.position".into()],
            FollowMode::Transform => ["position", "rotation", "scale_x", "scale_y"].iter().map(|k| format!("transform.{k}")).collect(),
            FollowMode::Corners => m::CornerPin::CORNERS.iter().map(|k| format!("corner_pin.{k}")).collect(),
            FollowMode::Mask(mk) => mc.keyframes.keys().filter(|k| k.starts_with(&format!("mask.{mk}.point."))).cloned().collect(),
        };
        for p in paths {
            mc.keyframes.remove(&p);
        }
        if matches!(f.mode, FollowMode::Corners) {
            mc.corner_pin = None;
        }
        if matches!(f.mode, FollowMode::Position | FollowMode::Transform) {
            mc.transform.position = m::Vec2::new(f.base[0], f.base[1]);
            mc.transform.rotation = f.base[2];
            mc.transform.scale_x = f.base[3];
            mc.transform.scale_y = f.base[4];
        }
    }
    true
}

/// Removes a track (and stops its followers following, keeping their keys).
pub fn remove_track(model: &m::Project, id: u32, track: u32) -> Vec<m::Clip> {
    let mut out = Vec::new();
    for c in &model.active().clips {
        let mut c2 = c.clone();
        let mut changed = false;
        if c.id.0 as u32 == id {
            let mut list = tracks(c);
            let n = list.len();
            list.retain(|t| t.id != track);
            if list.len() != n {
                set_tracks(&mut c2, &list);
                changed = true;
            }
        }
        if follow_of(c).is_some_and(|f| f.clip == id && f.track == track) {
            detach(&mut c2, false);
            changed = true;
        }
        if changed {
            out.push(c2);
        }
    }
    out
}

// --- stabilizing -----------------------------------------------------------

/// A stabilize run worked out ahead.
#[derive(Clone, Debug)]
pub struct StabilizeJob {
    pub clip: u32,
    pub footage: Footage,
    pub range: Range<u64>,
    pub settings: StabilizeSettings,
    /// Follow this track's motion instead of the whole picture's (lock on a
    /// subject).
    pub track: Option<Track>,
}

/// The camera's motion over the clip: per frame of `range` after the
/// first, a similarity in analysis pixels.
pub type CameraPath = Vec<Mat3>;

impl StabilizeJob {
    pub fn run(&self, ctx: &JobCtx) -> Result<(CameraPath, f32), String> {
        let (w, h) = analysis_size(self.footage.width, self.footage.height);
        if let Some(tr) = &self.track {
            // The tracked subject's own motion, frame to frame.
            let mut out = Vec::new();
            let pts = |f: u64| tr.points_at(f as f64).map(|(p, _, _)| p.iter().map(|q| (q[0] * w as f64, q[1] * h as f64)).collect::<Vec<_>>());
            for f in self.range.start..self.range.end.saturating_sub(1) {
                let (Some(a), Some(b)) = (pts(f), pts(f + 1)) else { out.push(tk::IDENTITY); continue };
                out.push(tk::fit(Motion::Similarity, &a, &b).unwrap_or(tk::IDENTITY));
            }
            return Ok((out, tr.quality().0));
        }
        let mut frames = Frames::open(&self.footage)?;
        let base = self.range.start;
        let mut get = |i: usize| frames.get(base + i as u64).map_err(|e| tk::frame_error(i, e));
        let n = (self.range.end - self.range.start) as usize;
        let motion = tk::camera_motion(&mut get, 0..n, ctx).map_err(|e| e.to_string())?;
        let agree = motion.iter().map(|m| m.1).sum::<f32>() / motion.len().max(1) as f32;
        Ok((motion.into_iter().map(|m| m.0).collect(), agree))
    }
}

/// Sets up stabilizing clip `id`.
pub fn plan_stabilize(proj: &view::Project, model: &m::Project, id: u32, settings: StabilizeSettings, track: Option<u32>) -> Result<StabilizeJob, String> {
    let mc = model.active().clip(m::ClipId(id as u64)).ok_or("no such clip")?;
    let f = footage(proj, mc)?;
    let range = used_frames(mc, &f);
    if range.end - range.start < 3 {
        return Err("the clip is too short to stabilize".into());
    }
    if mc.parent.is_some() {
        return Err("the clip follows a parent; unparent it to stabilize it".into());
    }
    if mc.transform.three_d {
        return Err("stabilizing works on 2D layers".into());
    }
    let track = match track {
        Some(t) => Some(track_of(mc, t).ok_or_else(|| format!("clip {id} has no track {t}"))?),
        None => None,
    };
    Ok(StabilizeJob { clip: id, footage: f, range, settings, track })
}

/// What a stabilize did, kept on the clip to redo or remove it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stabilized {
    pub lock: bool,
    pub smoothness: f64,
    pub rotation: bool,
    pub scale: bool,
    pub max_zoom: f64,
    /// The transform before (position x, y, rotation, scale x, y).
    pub base: [f64; 5],
    pub zoom: f64,
    pub strength: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<u32>,
}

pub fn stabilized(mc: &m::Clip) -> Option<Stabilized> {
    mc.meta.get("stabilize").and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// The clip with the stabilizing keys for a measured camera path.
pub fn apply_stabilize(proj: &view::Project, model: &m::Project, job: &StabilizeJob, path: &CameraPath) -> Result<(m::Clip, Stabilized), String> {
    let id = job.clip;
    let mut mc = model.active().clip(m::ClipId(id as u64)).ok_or("the clip is gone")?.clone();
    let vc = proj.clip(id).ok_or("the clip is gone")?.clone();
    // Start from the transform before any earlier stabilizing.
    let prior = stabilized(&mc);
    if let Some(p) = &prior {
        for k in ["position", "rotation", "scale_x", "scale_y"] {
            mc.keyframes.remove(&format!("transform.{k}"));
        }
        mc.transform.position = m::Vec2::new(p.base[0], p.base[1]);
        mc.transform.rotation = p.base[2];
        mc.transform.scale_x = p.base[3];
        mc.transform.scale_y = p.base[4];
    }
    if ["position", "rotation", "scale_x", "scale_y"].iter().any(|k| mc.keyframes.contains_key(&format!("transform.{k}"))) {
        return Err("the clip's position, rotation or scale is already animated; stabilize before animating it (or remove those keys)".into());
    }
    let (w, h) = analysis_size(job.footage.width, job.footage.height);
    let plan = tk::plan_stabilize(path, w as f64, h as f64, job.footage.fps, &job.settings);
    let geo = xform::geo(proj, model, id, vc.start).ok_or("the clip draws nothing")?;
    let fit = geo.fit() as f64;
    let k = job.footage.width as f64 / w as f64;
    let t0 = mc.transform.clone();
    let (sx, sy) = (t0.scale_x * if t0.flip_x { -1.0 } else { 1.0 }, t0.scale_y * if t0.flip_y { -1.0 } else { 1.0 });
    let (rs, rc) = t0.rotation.to_radians().sin_cos();
    let anchor = (t0.anchor.x, t0.anchor.y);
    let zoom = plan.zoom;
    let step = frame_len(proj);
    let mut pos = Vec::new();
    let mut rot = Vec::new();
    let mut scx = Vec::new();
    let mut scy = Vec::new();
    let mut t = vc.start;
    while t < vc.start + vc.dur {
        let f = (frame_at(&mc, job.footage.fps, t).round() as u64).clamp(job.range.start, job.range.end - 1);
        let q = plan.corrections[(f - job.range.start) as usize];
        let (s_c, a) = (q.log_scale.exp(), q.angle);
        let (asn, acs) = a.sin_cos();
        // Layer: canvas = pos + R·S·(p − C − anchor); correction p' =
        // Rₐ·s(p − C) + C + t. Folded: rotation + a, scale · s, and the
        // position moves by R·S·(t − anchor + Rₐ·s·anchor).
        let ra = (acs * anchor.0 - asn * anchor.1, asn * anchor.0 + acs * anchor.1);
        let v = (q.x * k - anchor.0 + s_c * ra.0, q.y * k - anchor.1 + s_c * ra.1);
        let v = (v.0 * fit * sx, v.1 * fit * sy);
        let d = (rc * v.0 - rs * v.1, rs * v.0 + rc * v.1);
        let p = (t0.position.x + d.0 * zoom, t0.position.y + d.1 * zoom);
        let local = t - vc.start;
        pos.push((local, m::Value::Point(m::Vec2::new(p.0, p.1))));
        rot.push((local, m::Value::Angle(t0.rotation + a.to_degrees())));
        scx.push((local, m::Value::Scalar(t0.scale_x * s_c * zoom)));
        scy.push((local, m::Value::Scalar(t0.scale_y * s_c * zoom)));
        t += step;
    }
    mc.keyframes.insert("transform.position".into(), track_keys(pos));
    if job.settings.rotation {
        mc.keyframes.insert("transform.rotation".into(), track_keys(rot));
    }
    if job.settings.scale || zoom > 1.0 + 1e-6 {
        mc.keyframes.insert("transform.scale_x".into(), track_keys(scx));
        mc.keyframes.insert("transform.scale_y".into(), track_keys(scy));
    }
    let s = &job.settings;
    let info = Stabilized { lock: s.lock, smoothness: s.smoothness, rotation: s.rotation, scale: s.scale, max_zoom: s.max_zoom, base: [t0.position.x, t0.position.y, t0.rotation, t0.scale_x, t0.scale_y], zoom: (zoom * 1000.0).round() / 1000.0, strength: (plan.strength * 100.0).round() / 100.0, track: job.track.as_ref().map(|t| t.id) };
    mc.meta.insert("stabilize".into(), serde_json::to_value(&info).map_err(|e| e.to_string())?);
    Ok((mc, info))
}

/// The clip without its stabilizing (the transform it had before).
pub fn unstabilize(mc: &m::Clip) -> Option<m::Clip> {
    let p = stabilized(mc)?;
    let mut out = mc.clone();
    for k in ["position", "rotation", "scale_x", "scale_y"] {
        out.keyframes.remove(&format!("transform.{k}"));
    }
    out.transform.position = m::Vec2::new(p.base[0], p.base[1]);
    out.transform.rotation = p.base[2];
    out.transform.scale_x = p.base[3];
    out.transform.scale_y = p.base[4];
    out.meta.remove("stabilize");
    Some(out)
}

/// A plain-words line about a track (for the Inspector and the AI).
pub fn describe(tr: &Track) -> String {
    let (c, lost) = tr.quality();
    let frames = tr.samples.len();
    let word = if c > 0.75 { "solid" } else if c > 0.5 { "fair" } else { "weak" };
    let mut s = format!("{} track {} \"{}\": {frames} frames, confidence {:.0}% ({word})", if tr.kind == Kind::Point { "point" } else { "plane" }, tr.id, tr.name, c * 100.0);
    if lost > 0 {
        s += &format!(", lost on {lost} frame(s)");
    }
    if !tr.fixes.is_empty() {
        s += &format!(", {} hand correction(s)", tr.fixes.len());
    }
    s
}

/// The frames where a track is weakest: (source frame, confidence), worst
/// first, at most `n` (a frame per stretch).
pub fn weak_spots(tr: &Track, n: usize) -> Vec<(u64, f32)> {
    let mut spots: Vec<(u64, f32)> = Vec::new();
    for (i, s) in tr.samples.iter().enumerate() {
        if s.c < 0.5 || s.lost {
            let f = tr.first + i as u64;
            if spots.last().is_some_and(|l| f - l.0 < (tr.fps / 2.0) as u64) {
                if s.c < spots.last().unwrap().1 {
                    *spots.last_mut().unwrap() = (f, s.c);
                }
                continue;
            }
            spots.push((f, s.c));
        }
    }
    spots.sort_by(|a, b| a.1.total_cmp(&b.1));
    spots.truncate(n);
    spots
}

/// Timeline time a clip shows source frame `f` (the first such moment;
/// `None` when it never does).
pub fn time_of_frame(proj: &view::Project, mc: &m::Clip, fps: f64, f: u64) -> Option<Time> {
    let step = frame_len(proj);
    let mut t = mc.start;
    let mut best: Option<(f64, Time)> = None;
    while t < mc.start + mc.duration {
        let d = (frame_at(mc, fps, t) - f as f64).abs();
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, t));
        }
        t += step;
    }
    best.filter(|b| b.0 < 1.0).map(|b| b.1)
}

/// A test shot with a known truth: a shaky camera on a textured wall and a
/// patterned square sliding across it.
#[cfg(test)]
pub(crate) mod test_movie {
    use std::path::{Path, PathBuf};
    use studio_encode::{h264, mux, yuv};

    /// The editor's tracking jobs are app-wide: tests that run them take
    /// turns.
    pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub const W: u32 = 640;
    pub const H: u32 = 360;
    pub const FPS: u32 = 30;
    pub const N: u32 = 60;

    /// Camera shake of frame `i` (source pixels the whole picture moves).
    pub fn shake(i: u32) -> (f64, f64) {
        let t = i as f64;
        ((t * 1.9).sin() * 4.0, (t * 2.7).cos() * 3.0)
    }

    /// Where the moving square's centre is in frame `i` (source pixels,
    /// before the shake).
    pub fn object(i: u32) -> (f64, f64) {
        (200.0 + 3.0 * i as f64, 150.0 + 1.0 * i as f64)
    }

    fn texture(x: f64, y: f64, s: f64) -> f64 {
        ((x / s).sin() * (y / s * 1.3).cos() + (x / s * 0.7 + y / s * 0.4).sin()) * 0.5
    }

    /// A shaky shot of a textured wall with a patterned square sliding
    /// across it.
    fn write_movie(path: &Path) {
        let mut cfg = h264::Config::new(W, H, FPS, 1);
        cfg.max_slices = 1;
        cfg.bitrate_kbps = 6000;
        let mut enc = h264::Encoder::new(cfg).unwrap();
        let mut muxer = mux::Muxer::create(path, mux::Options::mp4()).unwrap();
        let v = muxer.add_video(enc.track_info()).unwrap();
        for i in 0..N {
            let (sx, sy) = shake(i);
            let (ox, oy) = object(i);
            let mut rgba = vec![0u8; (W * H * 4) as usize];
            for y in 0..H {
                for x in 0..W {
                    // The world point seen at this pixel.
                    let (wx, wy) = (x as f64 - sx, y as f64 - sy);
                    let in_obj = (wx - ox).abs() < 28.0 && (wy - oy).abs() < 28.0;
                    let g = if in_obj {
                        0.55 + 0.4 * texture(wx - ox + 300.0, wy - oy + 300.0, 2.3)
                    } else {
                        0.45 + 0.3 * texture(wx, wy, 6.0) + 0.15 * texture(wx + 50.0, wy - 20.0, 2.7)
                    };
                    let b = (g.clamp(0.0, 1.0) * 255.0) as u8;
                    let k = ((y * W + x) * 4) as usize;
                    rgba[k..k + 4].copy_from_slice(&[b, b, if in_obj { 255 - b } else { b }, 255]);
                }
            }
            let pic = yuv::Yuv420::from_rgba(&rgba, W, H);
            let f = enc.encode(&pic);
            muxer.write_sample(v, &f.data, f.keyframe).unwrap();
        }
        muxer.finish().unwrap();
    }

    pub fn movie() -> PathBuf {
        static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("studio-track-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join("shaky.mp4");
            write_movie(&p);
            p
        })
        .clone()
    }

}
