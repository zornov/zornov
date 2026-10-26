// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Smooth slow motion: a clip slowed down (a constant speed under 1, or a
//! speed ramp that dips) shows each source frame several times and judders.
//! This makes the in-between frames with RIFE optical-flow interpolation
//! (a cross-fade when RIFE is not installed, or when asked), writes the
//! clip's footage `factor` times over into a new video beside the project,
//! and points the clip at it with its speed (and ramp) and in point scaled
//! by the same factor, so it plays the same moments at the same times with
//! a fresh picture on every frame. Frames the clip plays at normal speed or
//! faster are copied, not interpolated. The sound stays the original's
//! (detached first). `meta["slowmo"]` keeps what to put back.

use std::ops::Range;
use std::path::PathBuf;

use serde_json::{Value, json};
use studio_ai::features::interpolate::{FrameInterpolator, InterpEngine};
use studio_ai::{JobCtx, ModelStore};
use studio_model as m;

use super::tracking::{self, Footage};
use crate::view::SEC;

/// Most in-between frames per source frame.
pub const MAX_FACTOR: u32 = 8;

/// How the in-betweens are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// RIFE optical flow (sharp motion; about half a second a frame at 720p).
    Flow,
    /// A cross-fade between neighbours (instant, ghosts on fast motion).
    Blend,
}

/// A bake worked out ahead.
#[derive(Clone, Debug)]
pub struct SlowJob {
    pub clip: u32,
    pub footage: Footage,
    pub range: Range<u64>,
    pub factor: u32,
    /// Per source frame of `range`: whether the clip plays it slower than
    /// normal (only those get in-betweens).
    pub slow: Vec<bool>,
    /// The new footage's size: the source's own (even).
    pub size: (usize, usize),
    /// The size the in-betweens are worked out at (smaller than `size`
    /// unless asked for full quality: optical flow is slow).
    pub flow_size: (usize, usize),
    pub method: Method,
    pub file: PathBuf,
}

impl SlowJob {
    /// In-between frames to make.
    pub fn to_make(&self) -> usize {
        self.slow.iter().take(self.slow.len().saturating_sub(1)).filter(|&&s| s).count() * (self.factor as usize - 1)
    }

    /// Seconds it will roughly take (for the person and the AI).
    pub fn estimate_secs(&self) -> f64 {
        // Measured on an M-series Mac: RIFE 0.6 s a frame at 720p (rising a
        // little faster than the pixel count), the full-size scaling and
        // the H.264 encode about 0.6 s per written frame at 1080p.
        let flow_px = (self.flow_size.0 * self.flow_size.1) as f64;
        let px = (self.size.0 * self.size.1) as f64 / (1920.0 * 1080.0);
        let per = match self.method {
            Method::Flow => 0.58 * (flow_px / (1280.0 * 720.0)).powf(1.3),
            Method::Blend => 0.0,
        };
        let written = (self.range.end - self.range.start) as f64 * self.factor as f64;
        self.to_make() as f64 * per + written * 0.6 * px.max(0.1)
    }
}

/// Sets up smoothing clip `id`. `full`: interpolate at the footage's own
/// size (slower) instead of at most 1280 px wide.
pub fn plan(doc: &crate::doc::Doc, id: u32, method: Method, full: bool) -> Result<SlowJob, String> {
    let mc = doc.model.active().clip(m::ClipId(id as u64)).ok_or_else(|| format!("no clip {id}"))?;
    if mc.meta.contains_key("slowmo") {
        return Err(format!("clip {id} is already smoothed; remove that first to change it"));
    }
    let f = tracking::footage(&doc.project, mc)?;
    let fps_t = (doc.project.fps as f64).max(1.0);
    // The slowest the clip plays, and which source frames play slowly.
    // The frames the clip shows, and two more as a handle: the retimed clip
    // reads to the end of its last frame, which the new file must reach.
    let used = tracking::used_frames(mc, &f);
    let range = used.start..(used.end + 2).min(f.frames.max(used.end));
    let n = (range.end - range.start) as usize;
    let mut slow = vec![false; n];
    let mut min_rate = f64::INFINITY;
    let step = ((SEC as f64 / fps_t / 2.0) as m::Micros).max(1);
    let mut t = 0;
    while t < mc.duration {
        let r = mc.speed.rate_at(t);
        min_rate = min_rate.min(r);
        if r < 0.95 {
            let fr = tracking::frame_at(mc, f.fps, mc.start + t).floor() as i64 - range.start as i64;
            for k in [fr - 1, fr, fr + 1] {
                if k >= 0 && (k as usize) < n {
                    slow[k as usize] = true;
                }
            }
        }
        t += step;
    }
    if !(min_rate < 0.95) {
        return Err(format!("clip {id} plays at normal speed or faster: nothing to smooth (slow it first: set speed 0.25, or animate speed for a ramp)"));
    }
    if min_rate <= 0.0 {
        return Err("a freeze frame has no motion to make in-betweens of".into());
    }
    // Each frame from the source at the clip's footage rate: enough
    // in-betweens that the slowest part shows a new picture every frame.
    // A timeline frame moves `rate · fps / fps_t` source frames: under one
    // repeats them.
    let need = (fps_t / f.fps) / min_rate;
    if need <= 1.05 {
        return Err(format!("clip {id} already shows a new frame on every frame (its footage is {:.0} fps): it needs no in-betweens", f.fps));
    }
    let factor = (need.ceil() as u32).clamp(2, MAX_FACTOR);
    let (w, h) = (f.width as usize, f.height as usize);
    let size = ((w & !1).max(16), (h & !1).max(16));
    let k = if full || method == Method::Blend { 1.0 } else { (1280.0 / w.max(h) as f64).min(1.0) };
    let flow_size = ((((w as f64 * k) as usize) & !1).max(16), (((h as f64 * k) as usize) & !1).max(16));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    // Tests never write into the person's Studio folder.
    let home = if cfg!(test) { std::env::temp_dir().join("studio-tests") } else { studio_paths::studio() };
    let dir = doc.folder.as_ref().map(|f| f.root().join("Slow motion")).unwrap_or_else(|| home.join("Slow motion"));
    let file = dir.join(format!("clip-{id}-x{factor}-{stamp}.mp4"));
    Ok(SlowJob { clip: id, footage: f, range, factor, slow, size, flow_size, method, file })
}

/// Makes the smoothed footage; returns the engine that made the frames.
pub fn run(j: &SlowJob, ctx: &JobCtx) -> Result<InterpEngine, String> {
    use studio_encode::{h264, mux, yuv};
    let fi = match j.method {
        Method::Flow => FrameInterpolator::load(&ModelStore::default()).map_err(|e| e.to_string())?,
        Method::Blend => FrameInterpolator::fallback(),
    };
    let mut frames = super::cutout::RgbaFrames::open(&j.footage, j.size, &j.range)?;
    if let Some(d) = j.file.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let (w, h) = (j.size.0 as u32, j.size.1 as u32);
    // The footage's own frame rate, so frame k·i of the new file is frame i.
    let fps = j.footage.fps;
    let (num, den) = if (fps - fps.round()).abs() < 0.01 { (fps.round() as u32, 1) } else { ((fps * 1001.0).round() as u32, 1001) };
    let mut cfg = h264::Config::new(w, h, num, den);
    cfg.bitrate_kbps = ((w * h) as f64 * fps * 0.25 / 1000.0).clamp(2_000.0, 60_000.0) as u32;
    let mut enc = h264::Encoder::new(cfg).map_err(|e| e.to_string())?;
    let part = j.file.with_extension("mp4.part");
    let mut muxer = mux::Muxer::create(&part, mux::Options::mp4()).map_err(|e| e.to_string())?;
    let v = muxer.add_video(enc.track_info()).map_err(|e| e.to_string())?;
    let n = (j.range.end - j.range.start) as usize;
    let total = n * j.factor as usize;
    let mut put = |f: &studio_ai::frame::Frame, muxer: &mut mux::Muxer| -> Result<(), String> {
        let pic = yuv::Yuv420::from_rgba(&f.rgba, w, h);
        let e = enc.encode(&pic);
        muxer.write_sample(v, &e.data, e.keyframe).map_err(|e| e.to_string())
    };
    let mut done = 0usize;
    let mut next = frames.get(j.range.start)?;
    for i in 0..n {
        let cur = next;
        put(&cur, &mut muxer)?;
        done += 1;
        // The last frame is held for its whole length.
        let last = i + 1 == n;
        if !last {
            next = frames.get(j.range.start + i as u64 + 1)?;
        } else {
            next = cur.clone();
        }
        for s in 1..j.factor {
            ctx.check().map_err(|e| e.to_string())?;
            ctx.set_progress(done as f32 / total as f32);
            let between = if j.slow[i] && !last {
                let t = s as f32 / j.factor as f32;
                if j.flow_size == j.size {
                    fi.interpolate(&cur, &next, t).map_err(|e| e.to_string())?
                } else {
                    // Worked out small, laid over the full-size cross-fade
                    // where they differ little (still areas stay sharp).
                    let (fw, fh) = j.flow_size;
                    let small = fi.interpolate(&resize(&cur, fw, fh), &resize(&next, fw, fh), t).map_err(|e| e.to_string())?;
                    detail(&resize(&small, j.size.0, j.size.1), &studio_ai::features::interpolate::blend(&cur, &next, t))
                }
            } else {
                cur.clone()
            };
            put(&between, &mut muxer)?;
            done += 1;
        }
        if last {
            break;
        }
    }
    muxer.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&part, &j.file).map_err(|e| e.to_string())?;
    Ok(fi.engine())
}

/// Bilinear resize of an RGBA frame.
fn resize(f: &studio_ai::frame::Frame, w: usize, h: usize) -> studio_ai::frame::Frame {
    if (f.width, f.height) == (w, h) {
        return f.clone();
    }
    let mut out = vec![0u8; w * h * 4];
    let (sx, sy) = (f.width as f32 / w as f32, f.height as f32 / h as f32);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (f.height - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(f.height - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (f.width - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(f.width - 1);
            for c in 0..4 {
                let p = |xx: usize, yy: usize| f.rgba[(yy * f.width + xx) * 4 + c] as f32;
                let v = (p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx) * (1.0 - ty) + (p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx) * ty;
                out[(y * w + x) * 4 + c] = (v + 0.5) as u8;
            }
        }
    }
    studio_ai::frame::Frame { width: w, height: h, rgba: out }
}

/// The upscaled in-between, with the full-size cross-fade's fine detail
/// where the two agree (still parts of the picture keep their sharpness;
/// moving parts take the flow).
fn detail(flow: &studio_ai::frame::Frame, fade: &studio_ai::frame::Frame) -> studio_ai::frame::Frame {
    let mut out = flow.clone();
    for (o, (a, b)) in out.rgba.chunks_exact_mut(4).zip(flow.rgba.chunks_exact(4).zip(fade.rgba.chunks_exact(4))) {
        let d = (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0) as f32;
        let k = (1.0 - (d - 6.0) / 18.0).clamp(0.0, 1.0);
        for c in 0..3 {
            o[c] = (a[c] as f32 * (1.0 - k) + b[c] as f32 * k + 0.5) as u8;
        }
    }
    out
}

/// The clip moved onto the smoothed footage `media` (already imported):
/// its in point and speed scaled by the factor.
pub fn retimed(mc: &m::Clip, j: &SlowJob, media: m::MediaId, engine: InterpEngine) -> m::Clip {
    let mut c = mc.clone();
    let k = j.factor as f64;
    let first_us = j.range.start as f64 / j.footage.fps * SEC as f64;
    let old_media = match &c.source {
        m::ClipSource::Media(r) => r.media.0,
        _ => 0,
    };
    c.meta.insert(
        "slowmo".into(),
        json!({"media": old_media, "source_in": c.source_in, "speed": serde_json::to_value(&c.speed).unwrap_or(Value::Null), "factor": j.factor, "file": j.file.to_string_lossy(), "engine": if engine == InterpEngine::Rife { "rife" } else { "blend" }}),
    );
    c.source = m::ClipSource::Media(m::MediaRef { media, video: true, audio: false, extra: m::Extra::new() });
    c.source_in = ((c.source_in as f64 - first_us).max(0.0) * k).round() as m::Micros;
    c.speed = match &c.speed {
        m::Speed::Constant(r) => m::Speed::Constant(r * k),
        m::Speed::Curve(tr) => {
            let mut t2 = tr.clone();
            for key in &mut t2.keys {
                if let Some(v) = key.value.as_f64() {
                    key.value = m::Value::Scalar(v * k);
                }
            }
            m::Speed::Curve(t2)
        }
    };
    c
}

/// The clip back on its own footage, as it was before smoothing.
pub fn restored(mc: &m::Clip) -> Option<m::Clip> {
    let o = mc.meta.get("slowmo")?.as_object()?.clone();
    let mut c = mc.clone();
    c.source = m::ClipSource::Media(m::MediaRef { media: m::MediaId(o.get("media")?.as_u64()?), video: true, audio: false, extra: m::Extra::new() });
    c.source_in = o.get("source_in")?.as_i64()?;
    c.speed = serde_json::from_value(o.get("speed")?.clone()).ok()?;
    c.meta.remove("slowmo");
    Some(c)
}

/// Plain words about a smoothed clip.
pub fn describe(mc: &m::Clip) -> Option<String> {
    let o = mc.meta.get("slowmo")?.as_object()?;
    Some(format!("smooth slow motion: {}× the frames, made by {}", o.get("factor")?.as_u64()?, if o.get("engine").and_then(Value::as_str) == Some("rife") { "RIFE optical flow" } else { "cross-fades" }))
}

/// Puts a finished bake on the timeline: the new footage imported, the
/// clip's sound detached (it stays the original's, linked), the clip
/// retimed onto the new footage. Returns the new media id.
pub fn apply(doc: &mut crate::doc::Doc, j: &SlowJob, engine: InterpEngine) -> Result<u32, String> {
    let report = super::project::import_media(doc, std::slice::from_ref(&j.file));
    let media = *report.ids.first().ok_or_else(|| format!("the smoothed footage could not be imported: {}", report.failed.join(", ")))?;
    let seq = doc.seq();
    let mc = doc.model.active().clip(m::ClipId(j.clip as u64)).cloned().ok_or("the clip is gone")?;
    let mut edits = Vec::new();
    let sound = match &mc.source {
        m::ClipSource::Media(r) => r.audio && doc.model.media_item(r.media).is_some_and(|i| i.has_audio()),
        _ => false,
    };
    if sound {
        edits.push(m::Edit::DetachAudio { seq, clip: mc.id, new_clip: m::ClipId(doc.alloc_id()) });
    }
    edits.push(m::Edit::SetClip { seq, clip: Box::new(retimed(&mc, j, m::MediaId(media as u64), engine)) });
    if !doc.apply("Smooth slow motion", m::Edit::Batch { edits }) {
        return Err(doc.error.take().unwrap_or_else(|| "the clip could not be moved onto the smoothed footage".into()));
    }
    Ok(media)
}
