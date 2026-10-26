// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scene cut detection (split long clips at camera changes), without a
//! model.
//!
//! Each frame is reduced to a 64×36 thumbnail plus luma/chroma histograms.
//! A cut is a frame whose difference from the previous one (a mix of
//! histogram distance and pixel difference) is high both absolutely and
//! relative to the recent level of change, so fast camera motion does not
//! fire. One-frame flashes (the frame after is like the frame before) are
//! ignored, and fades through black produce a cut in the dark middle.

use crate::cache::{Cacheable, Reader, Writer};
use crate::error::Result;
use crate::frame::{Frame, FrameSource};
use crate::jobs::JobCtx;

const TW: usize = 64;
const TH: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutKind {
    /// Instant change of shot.
    Hard,
    /// Fade out and in through black.
    Fade,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneCut {
    /// First frame of the new scene.
    pub frame: usize,
    /// Its time in seconds.
    pub time: f64,
    /// Strength of the change (0..1).
    pub score: f32,
    pub kind: CutKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneSettings {
    /// 0..1; higher finds more (subtler) cuts.
    pub sensitivity: f32,
    /// Shortest scene in seconds; closer cuts are merged.
    pub min_scene: f64,
    /// Look at every n-th frame (1 = all). Cuts are then exact to n frames.
    pub stride: usize,
}

impl Default for SceneSettings {
    fn default() -> Self {
        SceneSettings { sensitivity: 0.5, min_scene: 0.5, stride: 1 }
    }
}

impl SceneSettings {
    pub fn cache_params(&self) -> String {
        format!("v2 sens={:.3} min={:.3} stride={}", self.sensitivity, self.min_scene, self.stride)
    }
}

/// The per-frame summary the detector compares.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameSignature {
    thumb: Vec<[f32; 3]>,
    hist: Vec<f32>,
    pub mean_luma: f32,
}

impl FrameSignature {
    pub fn of(frame: &Frame) -> FrameSignature {
        // area-average downscale
        let mut thumb = vec![[0.0f32; 3]; TW * TH];
        let mut count = vec![0u32; TW * TH];
        for y in 0..frame.height {
            let ty = y * TH / frame.height;
            for x in 0..frame.width {
                let tx = x * TW / frame.width;
                let p = &frame.rgba[(y * frame.width + x) * 4..][..3];
                let t = &mut thumb[ty * TW + tx];
                for c in 0..3 {
                    t[c] += p[c] as f32;
                }
                count[ty * TW + tx] += 1;
            }
        }
        // 16 luma + 8 + 8 chroma bins (BT.709 YCbCr)
        let mut hist = vec![0.0f32; 32];
        let mut luma = 0.0;
        for (t, &n) in thumb.iter_mut().zip(&count) {
            let n = n.max(1) as f32;
            for v in t.iter_mut() {
                *v /= n * 255.0;
            }
            let y = 0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2];
            let cb = ((t[2] - y) / 1.8556 + 0.5).clamp(0.0, 0.999);
            let cr = ((t[0] - y) / 1.5748 + 0.5).clamp(0.0, 0.999);
            hist[(y.clamp(0.0, 0.999) * 16.0) as usize] += 1.0;
            hist[16 + (cb * 8.0) as usize] += 1.0;
            hist[24 + (cr * 8.0) as usize] += 1.0;
            luma += y;
        }
        let total = (TW * TH) as f32;
        hist.iter_mut().for_each(|h| *h /= total);
        FrameSignature { thumb, hist, mean_luma: luma / total }
    }

    /// Difference to another frame in 0..1.
    pub fn distance(&self, o: &FrameSignature) -> f32 {
        // 1 − histogram intersection, averaged over the three channels
        let inter: f32 = self.hist.iter().zip(&o.hist).map(|(a, b)| a.min(*b)).sum::<f32>() / 3.0;
        let pix: f32 = self.thumb.iter().zip(&o.thumb).map(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()).sum::<f32>() / (3 * TW * TH) as f32;
        0.5 * (1.0 - inter) + 0.5 * (pix * 2.5).min(1.0)
    }
}

/// Finds cuts in a clip.
pub fn detect_scenes(src: &mut dyn FrameSource, settings: &SceneSettings, ctx: &JobCtx) -> Result<Vec<SceneCut>> {
    let n = src.frame_count();
    let stride = settings.stride.max(1);
    let mut sigs: Vec<(usize, FrameSignature)> = Vec::new();
    let mut i = 0;
    while i < n {
        ctx.check()?;
        ctx.set_progress(i as f32 / n.max(1) as f32);
        ctx.set_message(format!("Detecting scenes: frame {} / {n}", i + 1));
        sigs.push((i, FrameSignature::of(&src.frame(i)?)));
        i += stride;
    }
    Ok(find_cuts(&sigs, src.fps(), settings))
}

/// Picks cuts from per-frame signatures `(frame index, signature)`.
pub fn find_cuts(sigs: &[(usize, FrameSignature)], fps: f64, settings: &SceneSettings) -> Vec<SceneCut> {
    let fps = if fps > 0.0 { fps } else { 30.0 };
    let s = settings.sensitivity.clamp(0.0, 1.0);
    let abs_thresh = 0.55 - 0.35 * s; // 0.55 (few) .. 0.20 (many)
    let rel = 3.0 - 1.5 * s;
    // The smallest change a lone spike may be (0.12 at the default).
    let soft_floor = 0.16 - 0.08 * s;
    let d: Vec<f32> = sigs.windows(2).map(|w| w[0].1.distance(&w[1].1)).collect();
    let mut cuts: Vec<SceneCut> = Vec::new();
    let push = |cuts: &mut Vec<SceneCut>, c: SceneCut| {
        if let Some(last) = cuts.last_mut()
            && (c.time - last.time) < settings.min_scene
        {
            if c.score > last.score {
                *last = c;
            }
            return;
        }
        if c.time < settings.min_scene {
            return;
        }
        cuts.push(c);
    };
    for k in 0..d.len() {
        // d[k] is the change from sigs[k] to sigs[k+1]
        let lo = k.saturating_sub(15);
        let mut recent: Vec<f32> = d[lo..k].to_vec();
        recent.sort_by(f32::total_cmp);
        let level = recent.get(recent.len() / 2).copied().unwrap_or(0.0);
        let strong = d[k] >= abs_thresh && d[k] >= level * rel + 0.05;
        // A cut between two shots that look alike is a small change, but a
        // lone spike: far above everything around it and the biggest of its
        // neighbours. Those count too (camera motion rises and falls
        // together, so it does not spike).
        let spike = d[k] >= soft_floor && {
            let (a, b) = (k.saturating_sub(8), (k + 9).min(d.len()));
            let mut around: Vec<f32> = (a..b).filter(|&j| (j as isize - k as isize).abs() > 1).map(|j| d[j]).collect();
            around.sort_by(f32::total_cmp);
            let typical = around.get(around.len() / 2).copied().unwrap_or(0.0);
            let local_max = (k.saturating_sub(2)..(k + 3).min(d.len())).all(|j| d[j] <= d[k]);
            local_max && d[k] >= typical * 5.0 + 0.08
        };
        if !strong && !spike {
            continue;
        }
        // flash: the frame after the change looks like the one before it
        // (we are entering the flash) or the one before looks like the one
        // after (we are leaving it)
        let flash_like = abs_thresh.min(d[k]) * 0.5;
        let similar = |a: usize, b: usize| sigs[a].1.distance(&sigs[b].1) < flash_like;
        if (k + 2 < sigs.len() && similar(k, k + 2)) || (k >= 1 && similar(k - 1, k + 1)) {
            continue;
        }
        let f = sigs[k + 1].0;
        push(&mut cuts, SceneCut { frame: f, time: f as f64 / fps, score: d[k].min(1.0), kind: CutKind::Hard });
    }
    // fades through black: a dark run between brighter frames
    const DARK: f32 = 0.06;
    let mut k = 0;
    while k < sigs.len() {
        if sigs[k].1.mean_luma < DARK {
            let a = k;
            while k < sigs.len() && sigs[k].1.mean_luma < DARK {
                k += 1;
            }
            let b = k; // first bright frame after the run
            if a > 0 && b < sigs.len() {
                let mid = sigs[(a + b) / 2].0;
                let t = mid as f64 / fps;
                if !cuts.iter().any(|c| (c.time - t).abs() < settings.min_scene) {
                    cuts.push(SceneCut { frame: mid, time: t, score: 1.0, kind: CutKind::Fade });
                }
            }
        } else {
            k += 1;
        }
    }
    cuts.sort_by_key(|c| c.frame);
    cuts
}

impl Cacheable for SceneCut {
    const TAG: &'static str = "scene-cut-v1";
    fn encode(&self, w: &mut Writer) {
        w.u64(self.frame as u64);
        w.f64(self.time);
        w.f32(self.score);
        w.u8(matches!(self.kind, CutKind::Fade) as u8);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        Some(SceneCut { frame: r.u64()? as usize, time: r.f64()?, score: r.f32()?, kind: if r.u8()? == 1 { CutKind::Fade } else { CutKind::Hard } })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::VecSource;

    /// A "shot": a gradient whose colours depend on `seed`, panning with `t`.
    fn shot(seed: u8, t: usize) -> Frame {
        let (w, h) = (96, 54);
        let mut f = Frame::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let u = ((x + t * 2) % w) as u32;
                let p = [(u * 2 + seed as u32 * 60) as u8, (y as u32 * 4 + seed as u32 * 90) as u8, ((u + y as u32) + seed as u32 * 30) as u8, 255];
                f.rgba[(y * w + x) * 4..][..4].copy_from_slice(&p);
            }
        }
        f
    }

    fn run(frames: Vec<Frame>) -> Vec<SceneCut> {
        let mut src = VecSource { frames, fps: 10.0 };
        detect_scenes(&mut src, &SceneSettings { min_scene: 0.3, ..Default::default() }, &JobCtx::detached()).unwrap()
    }

    #[test]
    fn finds_hard_cuts_but_not_pans() {
        let frames: Vec<Frame> = (0..30).map(|i| shot(if i < 12 { 0 } else if i < 22 { 1 } else { 2 }, i)).collect();
        let cuts = run(frames);
        let at: Vec<usize> = cuts.iter().map(|c| c.frame).collect();
        assert_eq!(at, vec![12, 22], "{cuts:?}");
        assert!(cuts.iter().all(|c| c.kind == CutKind::Hard && (c.time - c.frame as f64 / 10.0).abs() < 1e-9));
    }

    /// Another take of the same set: the same layout and palette, a little
    /// brighter and shifted. Cuts between such shots are small changes.
    fn similar(variant: u8, t: usize) -> Frame {
        let mut f = shot(0, t);
        let shift = variant as i32 * 22;
        for px in f.rgba.chunks_exact_mut(4) {
            px[0] = (px[0] as i32 + shift).clamp(0, 255) as u8;
            px[1] = (px[1] as i32 - shift / 2).clamp(0, 255) as u8;
            px[2] = (px[2] as i32 + shift / 3).clamp(0, 255) as u8;
        }
        // And a different framing: the picture shifted sideways.
        let w = f.width;
        for row in f.rgba.chunks_exact_mut(w * 4) {
            row.rotate_left((variant as usize * 6 % w) * 4);
        }
        f
    }

    /// Two cuts a couple of seconds apart between shots that look alike
    /// (the 18.3 s and 20.5 s cuts an editor reported as missed).
    #[test]
    fn finds_close_cuts_between_similar_shots() {
        let frames: Vec<Frame> = (0..300).map(|i| similar(if i < 183 { 0 } else if i < 205 { 1 } else { 2 }, i / 3)).collect();
        let mut src = VecSource { frames, fps: 10.0 };
        let cuts = detect_scenes(&mut src, &SceneSettings::default(), &JobCtx::detached()).unwrap();
        let at: Vec<usize> = cuts.iter().map(|c| c.frame).collect();
        assert_eq!(at, vec![183, 205], "{cuts:?}");
    }

    #[test]
    fn ignores_flashes() {
        let mut frames: Vec<Frame> = (0..20).map(|i| shot(0, i)).collect();
        frames[10] = Frame::filled(96, 54, [255, 255, 255, 255]);
        assert!(run(frames).is_empty());
    }

    #[test]
    fn finds_fades_through_black() {
        let mut frames: Vec<Frame> = (0..10).map(|i| shot(0, i)).collect();
        frames.extend((0..6).map(|_| Frame::filled(96, 54, [5, 5, 5, 255])));
        frames.extend((0..10).map(|i| shot(0, i)));
        let cuts = run(frames);
        assert!(cuts.iter().any(|c| c.kind == CutKind::Fade && (10..16).contains(&c.frame)), "{cuts:?}");
    }

    #[test]
    fn min_scene_merges_close_cuts() {
        let frames: Vec<Frame> = (0..30).map(|i| shot((i / 2 % 3) as u8, 0)).collect();
        let mut src = VecSource { frames, fps: 10.0 };
        let cuts = detect_scenes(&mut src, &SceneSettings { min_scene: 1.0, ..Default::default() }, &JobCtx::detached()).unwrap();
        for w in cuts.windows(2) {
            assert!(w[1].time - w[0].time >= 1.0, "{cuts:?}");
        }
    }

    #[test]
    fn cache_round_trip() {
        let c = vec![SceneCut { frame: 3, time: 0.1, score: 0.7, kind: CutKind::Fade }];
        let mut w = Writer::default();
        c.encode(&mut w);
        assert_eq!(Vec::<SceneCut>::decode(&mut Reader::new(&w.0)).unwrap(), c);
    }
}
