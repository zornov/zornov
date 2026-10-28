// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `see`: the app looks for the agent and says what it saw, in words and
//! numbers, so most looking needs no picture at all. Per moment: light
//! (brightness, clipping, contrast), colour (dominant colours, how vivid),
//! people (faces: how many, where, how close), where the subject sits,
//! camera motion (still, pans, how fast), sharpness, bars, text on screen,
//! and on the timeline what is playing there. A line costs a few dozen
//! tokens; a picture costs hundreds. Measurements are cached by scene.

#[path = "ground.rs"]
pub(crate) mod ground;

use super::{Session, secs};
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};
use studio_media::rgba::RgbaImage;

/// What one frame (and the one just after it) looks like.
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Seen {
    pub brightness: f64,
    pub clipped_high: f64,
    pub crushed_low: f64,
    pub contrast: f64,
    pub colours: Vec<String>,
    pub vivid: f64,
    pub faces: Vec<[f64; 3]>, // centre x, centre y (0..1), height fraction
    pub subject: [f64; 2],
    pub motion: String,
    pub sharp: f64,
    pub bars: bool,
    pub lettering: bool,
}

fn lum(p: &[u8]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

fn colour_name(r: f64, g: f64, b: f64) -> String {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0 / 255.0;
    let s = if mx - mn < 1.0 { 0.0 } else { (mx - mn) / 255.0 / (1.0 - (2.0 * l - 1.0).abs()).max(1e-3) };
    if s < 0.18 || mx - mn < 18.0 {
        return (if l < 0.12 { "black" } else if l < 0.35 { "dark grey" } else if l < 0.7 { "grey" } else { "white" }).into();
    }
    let h = {
        let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
        let d = (mx - mn) / 255.0;
        let h = if mx / 255.0 == r { ((g - b) / d).rem_euclid(6.0) } else if mx / 255.0 == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
        h * 60.0
    };
    let hue = match h as u32 {
        0..=14 | 345..=360 => "red",
        15..=40 => "orange",
        41..=65 => "yellow",
        66..=160 => "green",
        161..=195 => "teal",
        196..=255 => "blue",
        256..=290 => "purple",
        _ => "pink",
    };
    let shade = if l < 0.25 { "deep " } else if l > 0.72 { "pale " } else { "" };
    format!("{shade}{hue}")
}

/// Global shift between two small grey frames (a rough camera motion).
fn shift(a: &[f64], b: &[f64], w: usize, h: usize) -> (i32, i32, f64) {
    let mut best = (0, 0, f64::MAX);
    for dy in -4i32..=4 {
        for dx in -6i32..=6 {
            let mut s = 0.0f64;
            let mut n = 0.0f64;
            for y in 6..h.saturating_sub(6) {
                for x in 8..w.saturating_sub(8) {
                    let (x2, y2) = ((x as i32 + dx) as usize, (y as i32 + dy) as usize);
                    s += (a[y * w + x] - b[y2 * w + x2]).abs();
                    n += 1.0;
                }
            }
            let e = s / n.max(1.0);
            if e < best.2 {
                best = (dx, dy, e);
            }
        }
    }
    best
}

pub fn look_at(img: &RgbaImage, next: Option<&RgbaImage>, faces: &[studio_ai::features::faces::Face]) -> Seen {
    let (w, h) = (img.width as usize, img.height as usize);
    let n = (w * h).max(1) as f64;
    let ls: Vec<f64> = img.pixels.chunks_exact(4).map(lum).collect();
    let mean = ls.iter().sum::<f64>() / n;
    let sd = (ls.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
    // Colours: a coarse histogram of named colours, by area.
    let mut names: std::collections::HashMap<String, f64> = Default::default();
    let mut sat = 0.0;
    for q in img.pixels.chunks_exact(4).step_by(3) {
        let (r, g, b) = (q[0] as f64, q[1] as f64, q[2] as f64);
        *names.entry(colour_name(r, g, b)).or_default() += 1.0;
        sat += (r.max(g).max(b) - r.min(g).min(b)) / 255.0;
    }
    let total: f64 = names.values().sum();
    let mut cs: Vec<(String, f64)> = names.into_iter().collect();
    cs.sort_by(|a, b| b.1.total_cmp(&a.1));
    let colours: Vec<String> = cs.iter().filter(|(_, v)| *v / total > 0.08).take(3).map(|(k, _)| k.clone()).collect();
    // Subject: where the detail is (edges), pulled to faces when there are any.
    let (mut sx, mut sy, mut se) = (0.0, 0.0, 0.0);
    let mut lap = 0.0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = ls[y * w + x];
            let e = (ls[y * w + x + 1] - ls[y * w + x - 1]).abs() + (ls[(y + 1) * w + x] - ls[(y - 1) * w + x]).abs();
            sx += e * x as f64;
            sy += e * y as f64;
            se += e;
            let l = 4.0 * c - ls[y * w + x + 1] - ls[y * w + x - 1] - ls[(y + 1) * w + x] - ls[(y - 1) * w + x];
            lap += l * l;
        }
    }
    // Where the detail gathers: the third of the frame (3×3) with the most
    // edge energy, if it clearly stands out; else the centre of it all.
    let mut grid = [0f64; 9];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let e = (ls[y * w + x + 1] - ls[y * w + x - 1]).abs() + (ls[(y + 1) * w + x] - ls[(y - 1) * w + x]).abs();
            grid[(y * 3 / h) * 3 + x * 3 / w] += e;
        }
    }
    let gm = grid.iter().sum::<f64>() / 9.0;
    let (gi, gv) = grid.iter().cloned().enumerate().fold((4, 0.0), |a, (i, v)| if v > a.1 { (i, v) } else { a });
    let mut subject = if gv > gm * 1.6 { [((gi % 3) as f64 + 0.5) / 3.0, ((gi / 3) as f64 + 0.5) / 3.0] } else { [sx / se.max(1e-6) / w as f64, sy / se.max(1e-6) / h as f64] };
    let fs: Vec<[f64; 3]> = faces.iter().map(|f| [(f.bbox.x + f.bbox.w / 2.0) as f64 / w as f64, (f.bbox.y + f.bbox.h / 2.0) as f64 / h as f64, f.bbox.h as f64 / h as f64]).collect();
    if let Some(f) = fs.iter().max_by(|a, b| a[2].total_cmp(&b[2])) {
        subject = [f[0], f[1]];
    }
    // Bars: black rows top and bottom (or columns at the sides).
    let row_dark = |y: usize| ls[y * w..(y + 1) * w].iter().all(|&v| v < 14.0);
    let col_dark = |x: usize| (0..h).all(|y| ls[y * w + x] < 14.0);
    let bars = (row_dark(1) && row_dark(h - 2)) || (col_dark(1) && col_dark(w - 2));
    // Lettering: thin light strokes on a mostly dark frame.
    let dark = ls.iter().filter(|&&v| v < 25.0).count() as f64 / n;
    let (mut lit, mut edge) = (0usize, 0usize);
    for y in 2..h.saturating_sub(2) {
        for x in 2..w.saturating_sub(2) {
            if ls[y * w + x] > 150.0 {
                lit += 1;
                if [ls[y * w + x - 2], ls[y * w + x + 2], ls[(y - 2) * w + x], ls[(y + 2) * w + x]].iter().any(|&v| v < 60.0) {
                    edge += 1;
                }
            }
        }
    }
    let lettering = dark > 0.6 && lit as f64 / n > 0.002 && lit as f64 / n < 0.15 && edge as f64 / lit.max(1) as f64 > 0.45;
    // Motion against the frame a moment later.
    let motion = match next {
        None => String::new(),
        Some(nx) if nx.width == img.width && nx.height == img.height => {
            let ln: Vec<f64> = nx.pixels.chunks_exact(4).map(lum).collect();
            let diff = ls.iter().zip(&ln).map(|(a, b)| (a - b).abs()).sum::<f64>() / n;
            let (dx, dy, resid) = shift(&ls, &ln, w, h);
            if diff < 2.0 {
                "still".into()
            } else if (dx != 0 || dy != 0) && resid < diff * 0.7 {
                let dir = match (dx.signum(), dy.signum()) {
                    (1, 0) => "pans left",
                    (-1, 0) => "pans right",
                    (0, 1) => "tilts up",
                    (0, -1) => "tilts down",
                    (1, _) => "moves left",
                    _ => "moves right",
                };
                let speed = if dx.abs().max(dy.abs()) >= 4 { "fast" } else { "slow" };
                format!("{dir}, {speed}")
            } else if diff > 25.0 {
                "a lot of movement (action or a cut)".into()
            } else {
                "things move in frame".into()
            }
        }
        _ => String::new(),
    };
    Seen {
        brightness: mean / 255.0,
        clipped_high: ls.iter().filter(|&&v| v > 250.0).count() as f64 / n,
        crushed_low: ls.iter().filter(|&&v| v < 5.0).count() as f64 / n,
        contrast: sd / 255.0,
        colours,
        vivid: sat / (n / 3.0),
        faces: fs,
        subject,
        motion,
        sharp: (lap / n).sqrt(),
        bars,
        lettering,
    }
}

/// `img` scaled down to `w`×`h` (box filter).
fn shrink(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (sw, sh) = (img.width as usize, img.height as usize);
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        let (y0, y1) = (y * sh / h, ((y + 1) * sh / h).max(y * sh / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * sw / w, ((x + 1) * sw / w).max(x * sw / w + 1));
            let mut acc = [0u32; 4];
            for yy in y0..y1.min(sh) {
                for xx in x0..x1.min(sw) {
                    let i = (yy * sw + xx) * 4;
                    for k in 0..4 {
                        acc[k] += img.pixels[i + k] as u32;
                    }
                }
            }
            let n = ((y1.min(sh) - y0) * (x1.min(sw) - x0)).max(1) as u32;
            for k in 0..4 {
                px[(y * w + x) * 4 + k] = (acc[k] / n) as u8;
            }
        }
    }
    RgbaImage { width: w as u32, height: h as u32, pixels: px }
}

fn place_word(x: f64, y: f64) -> String {
    let hx = if x < 0.38 { "left" } else if x > 0.62 { "right" } else { "centre" };
    let vy = if y < 0.36 { "top " } else if y > 0.64 { "bottom " } else { "" };
    if vy.is_empty() && hx == "centre" { "centre".into() } else { format!("{vy}{hx}") }
}

/// One line in words.
pub fn words(s: &Seen) -> String {
    let mut out = Vec::new();
    let b = s.brightness * 100.0;
    let light = if b < 12.0 { "very dark" } else if b < 30.0 { "dark" } else if b < 60.0 { "mid" } else if b < 80.0 { "bright" } else { "very bright" };
    let mut l = format!("{light} {b:.0}%");
    if s.clipped_high > 0.03 {
        l.push_str(&format!(", {:.0}% blown out", s.clipped_high * 100.0));
    }
    if s.crushed_low > 0.25 {
        l.push_str(&format!(", {:.0}% pure black", s.crushed_low * 100.0));
    }
    out.push(l);
    out.push(format!("contrast {}", if s.contrast < 0.12 { "low" } else if s.contrast < 0.25 { "medium" } else { "high" }));
    if !s.colours.is_empty() {
        out.push(format!("{} ({})", s.colours.join(", "), if s.vivid < 0.12 { "muted" } else if s.vivid < 0.3 { "natural" } else { "vivid" }));
    }
    match s.faces.len() {
        0 => {}
        n => {
            let f = s.faces.iter().max_by(|a, b| a[2].total_cmp(&b[2])).expect("one");
            let size = if f[2] > 0.35 { "close-up" } else if f[2] > 0.14 { "medium" } else { "small" };
            out.push(format!("{n} face{} ({size}, {})", if n > 1 { "s" } else { "" }, place_word(f[0], f[1])));
        }
    }
    out.push(format!("subject {}", place_word(s.subject[0], s.subject[1])));
    if !s.motion.is_empty() {
        out.push(s.motion.clone());
    }
    out.push((if s.sharp < 6.0 { "soft / blurred" } else { "sharp" }).into());
    if s.bars {
        out.push("black bars".into());
    }
    if s.lettering {
        out.push("lettering on screen".into());
    }
    out.join(" · ")
}

impl Session {
    /// `see` with `place`: the backdrop at `t` (every layer above the main
    /// lane left out, or the clips in `without`) read for staging.
    fn see_place(&mut self, a: &Value) -> Result<String, String> {
        let t = a.get("t").or(a.get("at")).and_then(Value::as_f64).ok_or("see with place needs \"t\": the moment to read (seconds on the timeline)")?;
        let at = (t * SEC as f64) as view::Time;
        let p = self.doc.project.clone();
        let spine = p.spine();
        let leave: Vec<u32> = match a.get("without") {
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_u64).map(|x| x as u32).collect(),
            _ => p.clips.iter().filter(|c| Some(c.lane) != spine && c.start <= at && at < c.start + c.dur).map(|c| c.id).collect(),
        };
        let img = self.frames.rgba_without(&self.doc, &leave, at, 512)?;
        // The person asked the AI to stage the scene: fetch the depth model
        // (99 MB, once) rather than stop.
        let store = studio_ai::store::ModelStore::default();
        let id = studio_ai::registry::ModelId::DepthAnythingV2Small;
        if !store.is_installed(id) {
            store.download(id, &studio_ai::jobs::JobCtx::detached()).map_err(|e| format!("could not download the depth model ({e}); get it in Settings → AI models"))?;
        }
        let est = studio_ai::features::depth::DepthEstimator::load(&store).map_err(|e| format!("the depth model is not ready ({e}): get it in Settings → AI models"))?;
        let frame = studio_ai::frame::Frame::new(img.width as usize, img.height as usize, img.pixels.clone()).map_err(|e| e.to_string())?;
        let depth = est.estimate(&frame).map_err(|e| format!("could not read the depth: {e}"))?;
        let g = ground::DepthGrid::shrink(depth.width, depth.height, &depth.data, 96);
        let (horizon, spots) = ground::spots(&g);
        let (w, h) = (p.width as f32, p.height as f32);
        let cx = |u: f32| ((u - 0.5) * w).round();
        let cy = |v: f32| ((v - 0.5) * h).round();
        let nearest = spots.iter().filter(|s| s.ground >= 0.5).map(|s| s.depth).fold(0f32, f32::max).max(1e-3);
        let share = g.ground().iter().filter(|&&b| b).count() as f32 / (g.w * g.h) as f32;
        let mut out = vec![format!(
            "the backdrop at {t:.2}s{}: the far land ends at y = {} px; ground (surfaces running away from the camera) is {:.0}% of the frame.",
            if leave.is_empty() { String::new() } else { format!(" (leaving out clips {leave:?})") },
            cy(horizon),
            share * 100.0
        )];
        out.push("spots to stand on (feet at [x, y] canvas px; how tall a character standing there should be, so near and far read right):".into());
        for s in &spots {
            let tall = ground::height_at(s.depth, nearest, 0.5) * h;
            out.push(format!(
                "  {:13} [{}, {}]  {:.0}% ground  about {:.0} px tall{}",
                s.name,
                cx(s.u),
                cy(s.v),
                s.ground * 100.0,
                tall,
                if s.ground < 0.5 { "  (mostly not ground: a rock, water, a trunk or a wall; frame it to see what)" } else { "" }
            ));
        }
        // What the parts are, when the scene model is here already.
        if studio_ai::store::ModelStore::default().is_installed(studio_ai::registry::ModelId::UperNetConvNextTiny)
            && let Ok(r) = self.regions_at(t, None)
        {
            let mut rs = r.regions.clone();
            rs.sort_by(|a, b| b.share.total_cmp(&a.share));
            out.push(format!("made of: {} (see with regions for where).", rs.iter().map(|x| format!("{} {:.0}%", x.name, x.share * 100.0)).collect::<Vec<_>>().join(", ")));
        }
        out.push("Put each character's feet on a spot and size it to that height (scale = height wanted / its height now); keep characters on different spots so none hides another, and the ones further back smaller. Things that rise from the ground (trees, rocks, a bridge) are where to hide, lean, climb or enter from; frame the backdrop to see what they are.".into());
        let pieces = g.pieces();
        if pieces.is_empty() {
            out.push("pieces that could hide a character: none stand out from the ground here.".into());
        } else {
            out.push("pieces that can hide a character standing behind them (nearest first; foreground with piece <n> cuts one out and lays it over the characters):".into());
            for (i, pc) in pieces.iter().enumerate() {
                out.push(format!(
                    "  piece {}: {}, box [{}, {}]-[{}, {}]; to stand behind it, feet near [{}, {}] (hidden from there up to y = {}), about {:.0} px tall",
                    i + 1,
                    pc.kind,
                    cx(pc.bbox[0]),
                    cy(pc.bbox[1]),
                    cx(pc.bbox[2]),
                    cy(pc.bbox[3]),
                    cx(pc.behind[0]),
                    cy(pc.behind[1]),
                    cy(pc.bbox[1]),
                    ground::height_at(pc.behind_depth, nearest, 0.5) * h
                ));
            }
        }
        Ok(out.join("\n"))
    }

    fn seen_at(&mut self, t: view::Time, width: u32, faces: &mut Option<Option<studio_ai::features::faces::FaceDetector>>) -> Option<Seen> {
        let key = self.scene_key_pub(t, t + SEC / 5, &format!("see{width}"));
        if let Some(v) = self.memo_pub().get(&key).cloned() {
            return serde_json::from_value(v).ok();
        }
        let a = self.frames.rgba(&self.doc, t, width).ok()?;
        let b = self.frames.rgba(&self.doc, t + SEC / 5, 96).ok();
        let small = self.frames.rgba(&self.doc, t, 96).ok();
        let fd = faces.get_or_insert_with(|| studio_ai::features::faces::FaceDetector::load(&studio_ai::store::ModelStore::default()).ok());
        let found: Vec<_> = match fd {
            Some(fd) => studio_ai::features::reframe::Reframer::find_faces(fd, &studio_ai::frame::Frame { width: a.width as usize, height: a.height as usize, rgba: a.pixels.clone() }).unwrap_or_default().into_iter().filter(|f| f.score > 0.6).collect(),
            None => Vec::new(),
        };
        let mut seen = look_at(&a, None, &found);
        if let (Some(s), Some(b)) = (small, b) {
            seen.motion = look_at(&s, Some(&b), &[]).motion;
        }
        let v = serde_json::to_value(&seen).unwrap_or(Value::Null);
        self.memo_pub().insert(key, v);
        Some(seen)
    }

    /// `see`: the timeline (one line per main-lane shot, or at `t`, or for
    /// `clips`), or a media file (`media`, one line every `every` seconds).
    pub(super) fn see(&mut self, a: &Value) -> Result<String, String> {
        if a.get("regions").is_some_and(|v| v.as_bool() != Some(false)) {
            return self.see_regions(a);
        }
        if a.get("place").is_some_and(|v| v.as_bool() != Some(false)) {
            return self.see_place(a);
        }
        let mut faces = None;
        let mut out = Vec::new();
        if let Some(mv) = a.get("media") {
            // Footage as it is, before it is used: a shot list in words.
            let mi = self.media_item(mv)?;
            let path = mi.path.clone().ok_or("that media has no file")?;
            let len = mi.dur as f64 / SEC as f64;
            let from = a.get("from").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
            let to = a.get("to").and_then(Value::as_f64).unwrap_or(len).min(len);
            let every = a.get("every").and_then(Value::as_f64).unwrap_or(((to - from) / 24.0).max(0.5)).max(0.1);
            let (w, h) = (192usize, ((192.0 * mi.height.max(1) as f64 / mi.width.max(1) as f64) as usize).clamp(32, 400));
            out.push(format!("{} · {:.1} s · every {every:.1} s (times in the file)", mi.name, len));
            let mut t = from;
            while t < to - 0.05 {
                let shot = (|| -> Option<Seen> {
                    let conv = |c: eframe::egui::ColorImage| RgbaImage { width: c.size[0] as u32, height: c.size[1] as u32, pixels: c.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b(), 255]).collect() };
                    let a = conv(crate::media_io::frame(&path, t, w, h, false).ok()?);
                    let b = crate::media_io::frame(&path, (t + 0.2).min(len - 0.01), 96, (96 * h / w).max(16), false).ok().map(conv);
                    let s = crate::media_io::frame(&path, t, 96, (96 * h / w).max(16), false).ok().map(conv);
                    let fd = faces.get_or_insert_with(|| studio_ai::features::faces::FaceDetector::load(&studio_ai::store::ModelStore::default()).ok());
                    let found: Vec<_> = match fd {
                        Some(fd) => studio_ai::features::reframe::Reframer::find_faces(fd, &studio_ai::frame::Frame { width: a.width as usize, height: a.height as usize, rgba: a.pixels.clone() }).unwrap_or_default().into_iter().filter(|f| f.score > 0.6).collect(),
                        None => Vec::new(),
                    };
                    let mut seen = look_at(&a, None, &found);
                    if let (Some(s), Some(b)) = (s, b) {
                        seen.motion = look_at(&s, Some(&b), &[]).motion;
                    }
                    Some(seen)
                })();
                if let Some(s) = shot {
                    out.push(format!("{t:6.2}  {}", words(&s)));
                }
                t += every;
            }
            return Ok(out.join("\n"));
        }
        let p = self.doc.project.clone();
        let times: Vec<(view::Time, Option<u32>)> = if let Some(t) = a.get("t").and_then(Value::as_f64) {
            vec![((t * SEC as f64) as view::Time, None)]
        } else {
            let spine = p.spine();
            let ids: Vec<u32> = match a.get("clips") {
                Some(Value::Array(v)) => v.iter().filter_map(Value::as_u64).map(|x| x as u32).collect(),
                Some(Value::Number(n)) => n.as_u64().map(|x| x as u32).into_iter().collect(),
                _ => {
                    let mut cs: Vec<&view::Clip> = p.clips.iter().filter(|c| Some(c.lane) == spine).collect();
                    cs.sort_by_key(|c| c.start);
                    cs.iter().map(|c| c.id).collect()
                }
            };
            let mut moments: Vec<(view::Time, Option<u32>)> = ids.iter().filter_map(|&id| p.clip(id)).map(|c| (c.start + c.dur / 2, Some(c.id))).collect();
            // Nothing on the main lane (a 3D or vector scene on overlay
            // lanes): moments spread over the film, not an empty answer.
            if moments.is_empty() && a.get("clips").is_none() {
                let len = p.duration();
                if len <= 0 {
                    return Err("there is nothing on the timeline to look at".into());
                }
                let n = ((len / SEC) as usize).clamp(1, 8);
                moments = (0..n).map(|i| (len * (2 * i as view::Time + 1) / (2 * n as view::Time), None)).collect();
            }
            if moments.is_empty() {
                return Err("none of those clips exist; give clip ids, or t (a timeline moment in seconds)".into());
            }
            moments
        };
        let t0 = std::time::Instant::now();
        let t1 = std::time::Instant::now();
        // Render what is missing, then look at it on several threads at once
        // (face finding is the slow part), each with its own detector.
        let mut todo: Vec<(String, RgbaImage, Option<RgbaImage>, Option<RgbaImage>)> = Vec::new();
        for &(t, _) in &times {
            let key = self.scene_key_pub(t, t + SEC / 5, "see360");
            if self.memo_pub().contains_key(&key) || todo.iter().any(|j| j.0 == key) {
                continue;
            }
            let Ok(a) = self.frames.rgba(&self.doc, t, 360) else { continue };
            // The small picture for motion is the big one shrunk; only the
            // moment a fifth of a second later is rendered again.
            let b = self.frames.rgba(&self.doc, t + SEC / 5, 96).ok();
            let small = b.as_ref().map(|b| shrink(&a, b.width, b.height));
            todo.push((key, a, small, b));
        }
        let t2 = std::time::Instant::now();
        if !todo.is_empty() {
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 6).min(todo.len());
            let per = todo.len().div_ceil(threads);
            let done: Vec<(String, Seen)> = std::thread::scope(|sc| {
                let hs: Vec<_> = todo
                    .chunks(per)
                    .map(|chunk| {
                        sc.spawn(move || {
                            let fd = studio_ai::features::faces::FaceDetector::load(&studio_ai::store::ModelStore::default()).ok();
                            chunk
                                .iter()
                                .map(|(key, a, small, b)| {
                                    let found: Vec<_> = match &fd {
                                        Some(fd) => studio_ai::features::reframe::Reframer::find_faces(fd, &studio_ai::frame::Frame { width: a.width as usize, height: a.height as usize, rgba: a.pixels.clone() }).unwrap_or_default().into_iter().filter(|f| f.score > 0.6).collect(),
                                        None => Vec::new(),
                                    };
                                    let mut seen = look_at(a, None, &found);
                                    if let (Some(s), Some(b)) = (small, b) {
                                        seen.motion = look_at(s, Some(b), &[]).motion;
                                    }
                                    (key.clone(), seen)
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            });
            for (key, seen) in done {
                let v = serde_json::to_value(&seen).unwrap_or(Value::Null);
                self.memo_pub().insert(key, v);
            }
        }
        // Where the figures sit against the guides: at one moment, or
        // when guides are asked for or on in the viewer.
        let wanted = super::guides::from_arg(a.get("guides"))?;
        let framing = a.get("t").is_some() || !wanted.is_empty();
        if std::env::var_os("STUDIO_PROFILE").is_some() {
            eprintln!("see: setup {:.2}s, frames {:.2}s, looking {:.2}s ({} shots)", (t1 - t0).as_secs_f64(), (t2 - t1).as_secs_f64(), t2.elapsed().as_secs_f64(), times.len());
        }
        // What is in each shot: from the object map when it is there; read
        // now for a moment asked for (or a couple of shots) if the model is
        // installed; `objects: false` leaves it out.
        let want_objects = a.get("objects").and_then(Value::as_bool);
        let explicit = a.get("t").is_some() || want_objects == Some(true);
        let n_shots = times.len();
        for (t, id) in times {
            let Some(s) = self.seen_at(t, 360, &mut faces) else { continue };
            // What is playing there, briefly.
            let on: Vec<String> = p
                .clips
                .iter()
                .filter(|c| c.start <= t && t < c.end() && c.kind != ClipKind::Audio)
                .map(|c| if c.kind == ClipKind::Title && !c.text.is_empty() { format!("\"{}\"", c.text.chars().take(20).collect::<String>()) } else if c.kind == ClipKind::Fx { format!("#{} {}", c.id, c.name) } else { format!("#{}", c.id) })
                .collect();
            let head = match id {
                Some(id) => format!("#{id} {:6.2}", secs(t)),
                None => format!("{:6.2}", secs(t)),
            };
            out.push(format!("{head}  {}  [{}]", words(&s), on.join(" ")));
            if want_objects != Some(false)
                && let Some(c) = id.or_else(|| self.object_clip_at(secs(t)))
                && let Some(l) = self.objects_line(c, secs(t), !cfg!(test) && (explicit || n_shots <= 2), explicit)
            {
                out.push(format!("        {l}"));
            }
            if framing {
                for l in self.framing_words(t, &wanted) {
                    out.push(format!("        framing {l}"));
                }
            }
            // Where the things are, exactly: one short line for a moment.
            if a.get("t").is_some() {
                if let Some(l) = self.layout_line(t) {
                    out.push(format!("        {l}"));
                }
            }
        }
        self.frames.clear_prefetch();
        self.memo_save_pub();
        Ok(out.join("\n"))
    }
}
