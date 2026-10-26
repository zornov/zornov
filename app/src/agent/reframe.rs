// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Subject-following framing: when a clip is cropped (a wide shot in a
//! vertical frame, a punch-in), the reframer finds what matters in each
//! moment (faces when that model is installed, visual saliency and motion
//! otherwise), plans a calm camera path, and the path becomes position
//! keyframes on the clip so the subject stays in frame.

use super::analyse::FileFrames;
use crate::view::{self, SEC};
use studio_ai::features::reframe::{ReframeSettings, Reframer};
use studio_ai::frame::FrameSource;

/// Analysis width: enough for faces and saliency, cheap to scan.
const THUMB: usize = 320;

/// The source frames a clip plays, decoded small.
struct RangeFrames {
    inner: FileFrames,
    first: usize,
    count: usize,
}

impl FrameSource for RangeFrames {
    fn frame_count(&self) -> usize {
        self.count
    }
    fn fps(&self) -> f64 {
        self.inner.fps()
    }
    fn frame(&mut self, index: usize) -> studio_ai::error::Result<studio_ai::frame::Frame> {
        self.inner.frame(self.first + index)
    }
}

/// Position keyframes (time into the clip in seconds, x, y in canvas
/// pixels) that keep `c`'s subject in frame. Only the free axis moves: a
/// clip cropped at the sides pans, one cropped top and bottom tilts.
pub fn plan(p: &view::Project, c: &view::Clip, smoothing: f32) -> Result<Vec<(f64, f64, f64)>, String> {
    let mi = c.media.and_then(|m| p.media(m)).ok_or("the clip has no media")?;
    let path = mi.path.as_ref().ok_or("sample media has no file to analyse")?;
    let src = studio_render::source::open_path(path)?;
    let info = src.info();
    let fps = info.frame_rate.unwrap_or(30.0).max(1.0);
    let total = info.frame_count.unwrap_or(0) as usize;
    let speed = (c.speed as f64 / 100.0).max(0.01);
    let first = ((c.src_in as f64 / SEC as f64) * fps).round() as usize;
    let count = (((c.dur as f64 / SEC as f64) * speed * fps).round() as usize).max(1).min(total.saturating_sub(first));
    if count == 0 {
        return Err("the clip is outside its media".into());
    }
    // How the media sits on the canvas at the clip's scale.
    let (cw, ch) = (p.width as f64, p.height as f64);
    let (mw, mh) = (mi.width.max(1) as f64, mi.height.max(1) as f64);
    let fit = (cw / mw).min(ch / mh) * c.transform.scale as f64 / 100.0;
    let (dw, dh) = (mw * fit, mh * fit);
    let (slack_x, slack_y) = (((dw - cw) / 2.0).max(0.0), ((dh - ch) / 2.0).max(0.0));
    if slack_x < 1.0 && slack_y < 1.0 {
        return Err("the clip fills the canvas exactly; there is nothing to reframe (scale it up first)".into());
    }
    // The visible window as a crop of the source.
    let aspect = ((cw / dw).min(1.0) * mw / ((ch / dh).min(1.0) * mh)) as f32;
    let mut frames = RangeFrames { inner: FileFrames { src, count: total, fps, width: THUMB }, first, count };
    let settings = ReframeSettings { aspect: aspect.clamp(0.05, 20.0), zoom: 1.0, smoothing: smoothing.clamp(0.0, 1.0), samples_per_second: 8.0 };
    let reframer = Reframer::new(&studio_ai::store::ModelStore::default());
    let keys = reframer.plan(&mut frames, &settings, &[], &studio_ai::jobs::JobCtx::detached()).map_err(|e| e.to_string())?;
    let thumb_w = THUMB as f64;
    let thumb_h = thumb_w * mh / mw;
    let step = ((fps / 6.0).round() as usize).max(1);
    let mut out = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        if i % step != 0 && i + 1 != keys.len() {
            continue;
        }
        let u = (k.x as f64 + k.w as f64 / 2.0) / thumb_w;
        let v = (k.y as f64 + k.h as f64 / 2.0) / thumb_h;
        let x = if slack_x >= 1.0 { ((0.5 - u) * dw).clamp(-slack_x, slack_x) } else { c.transform.x as f64 };
        let y = if slack_y >= 1.0 { ((0.5 - v) * dh).clamp(-slack_y, slack_y) } else { c.transform.y as f64 };
        let t = (k.frame as f64 / fps) / speed;
        out.push(((t * 1000.0).round() / 1000.0, (x * 10.0).round() / 10.0, (y * 10.0).round() / 10.0));
    }
    Ok(out)
}
