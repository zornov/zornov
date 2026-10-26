// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `match`: find the moments in the footage that look like a given frame,
//! for match cuts (a wheel into a jet intake, a headlight into the sun)
//! and for finding "more of this". Compares shape (where the strong edges
//! are) and look (colour and light layout); returns ranked moments with a
//! contact sheet of them.

use super::{Content, Session};
use crate::view::{self, MediaKind, SEC};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const G: usize = 16; // grid for shape and look

/// What a frame looks like, reduced to compare quickly.
#[derive(Clone)]
struct Sig {
    /// Edge strength per cell, normalised to unit length.
    edges: Vec<f32>,
    /// Mean colour per cell (0..1).
    colour: Vec<[f32; 3]>,
}

fn sig_of(img: &eframe::egui::ColorImage) -> Sig {
    let (w, h) = (img.size[0], img.size[1]);
    let lum = |x: usize, y: usize| {
        let c = img.pixels[y.min(h - 1) * w + x.min(w - 1)];
        0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
    };
    let mut edges = vec![0f32; G * G];
    let mut colour = vec![[0f32; 3]; G * G];
    let mut count = vec![0f32; G * G];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let cell = (y * G / h) * G + x * G / w;
            let gx = lum(x + 1, y) - lum(x - 1, y);
            let gy = lum(x, y + 1) - lum(x, y - 1);
            edges[cell] += (gx * gx + gy * gy).sqrt();
            let c = img.pixels[y * w + x];
            colour[cell][0] += c.r() as f32 / 255.0;
            colour[cell][1] += c.g() as f32 / 255.0;
            colour[cell][2] += c.b() as f32 / 255.0;
            count[cell] += 1.0;
        }
    }
    for (c, n) in colour.iter_mut().zip(&count) {
        for v in c.iter_mut() {
            *v /= n.max(1.0);
        }
    }
    let norm = edges.iter().map(|e| e * e).sum::<f32>().sqrt().max(1e-6);
    edges.iter_mut().for_each(|e| *e /= norm);
    Sig { edges, colour }
}

/// 0 (unlike) … 1 (alike).
fn score(a: &Sig, b: &Sig, mode: &str) -> f32 {
    let shape: f32 = a.edges.iter().zip(&b.edges).map(|(x, y)| x * y).sum();
    let look = 1.0 - a.colour.iter().zip(&b.colour).map(|(p, q)| (p[0] - q[0]).abs() + (p[1] - q[1]).abs() + (p[2] - q[2]).abs()).sum::<f32>() / (3 * G * G) as f32 * 2.5;
    match mode {
        "shape" => shape,
        "look" => look.max(0.0),
        _ => 0.65 * shape + 0.35 * look.max(0.0),
    }
}

/// Signatures of a file every `step` seconds, decoded in parallel; cached.
fn scan(path: &Path, dur: f64, w: u32, h: u32, step: f64) -> Vec<(f64, Sig)> {
    type Cache = Mutex<HashMap<(PathBuf, u64), Vec<(f64, Sig)>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let key = (path.to_path_buf(), (step * 1000.0) as u64);
    if let Some(v) = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let (tw, th) = (96usize, ((96.0 * h.max(1) as f64 / w.max(1) as f64).round() as usize).clamp(16, 192));
    let times: Vec<f64> = (0..).map(|i| 0.05 + i as f64 * step).take_while(|t| *t < dur - 0.05).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunks: Vec<Vec<f64>> = times.chunks(times.len().div_ceil(threads).max(1)).map(|c| c.to_vec()).collect();
    let mut out: Vec<(f64, Sig)> = std::thread::scope(|sc| {
        let hs: Vec<_> = chunks.iter().map(|ts| sc.spawn(move || ts.iter().filter_map(|&t| crate::media_io::frame(path, t, tw, th, false).ok().map(|img| (t, sig_of(&img)))).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).insert(key, out.clone());
    out
}

impl Session {
    /// `match`: moments that look like a frame. The frame is a clip's last
    /// (or first) frame, or `media` at `t`.
    pub(super) fn match_frames(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let p = self.doc.project.clone();
        let (qpath, qt, qmi, what) = if let Some(id) = a.get("clip").and_then(Value::as_u64) {
            let c = p.clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
            let mi = c.media.and_then(|m| p.media(m)).cloned().ok_or("that clip has no footage")?;
            let speed = c.speed as f64 / 100.0;
            let first = a.get("edge").and_then(Value::as_str) == Some("in");
            let src = c.src_in as f64 / SEC as f64 + if first { 0.0 } else { (c.dur as f64 / SEC as f64 * speed - 1.0 / 30.0).max(0.0) };
            (mi.path.clone().ok_or("no file")?, src, mi, format!("the {} frame of clip {id}", if first { "first" } else { "last" }))
        } else {
            let mi = self.media_item(a.get("media").ok_or("match needs \"clip\" (its last frame; \"edge\": \"in\" for its first) or \"media\" and \"t\"")?)?;
            let t = a.get("t").and_then(Value::as_f64).ok_or("match with media needs \"t\"")?;
            (mi.path.clone().ok_or("no file")?, t, mi.clone(), format!("{} at {t}s", mi.name))
        };
        let mode = a.get("by").and_then(Value::as_str).unwrap_or("both").to_owned();
        if !matches!(mode.as_str(), "shape" | "look" | "both") {
            return Err("by is shape, look or both".into());
        }
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(8).clamp(1, 24) as usize;
        let step = a.get("step").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.1, 5.0);
        let (qw, qh) = (96usize, ((96.0 * qmi.height.max(1) as f64 / qmi.width.max(1) as f64).round() as usize).clamp(16, 192));
        let q = sig_of(&crate::media_io::frame(&qpath, qt, qw, qh, false)?);
        // Where to look: named media, or every video file.
        let pool: Vec<view::MediaItem> = match a.get("in") {
            Some(Value::Array(names)) => names.iter().map(|n| self.media_item(n)).collect::<Result<_, _>>()?,
            Some(v) => vec![self.media_item(v)?],
            None => p.media.iter().filter(|m| m.kind == MediaKind::Video && m.path.is_some()).cloned().collect(),
        };
        let mut hits: Vec<(f32, view::MediaItem, f64)> = Vec::new();
        for mi in &pool {
            let Some(path) = &mi.path else { continue };
            for (t, s) in scan(path, mi.dur as f64 / SEC as f64, mi.width, mi.height, step) {
                // Not the query moment itself.
                if path == &qpath && (t - qt).abs() < 1.0 {
                    continue;
                }
                hits.push((score(&q, &s, &mode), mi.clone(), t));
            }
        }
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        // One hit per second of footage, so the list is not one shot eight times.
        let mut picked: Vec<(f32, view::MediaItem, f64)> = Vec::new();
        for h in hits {
            if picked.iter().any(|p| p.1.id == h.1.id && (p.2 - h.2).abs() < 1.0) {
                continue;
            }
            picked.push(h);
            if picked.len() == count {
                break;
            }
        }
        let list: Vec<Value> = picked.iter().map(|(s, mi, t)| json!({"media": mi.name, "t": (t * 100.0).round() / 100.0, "score": (*s as f64 * 100.0).round() / 100.0})).collect();
        // A sheet: the query frame, then the matches in order.
        let tile = 240u32;
        let mut tiles = Vec::new();
        let frame_of = |path: &Path, t: f64, mi: &view::MediaItem| -> Option<studio_media::rgba::RgbaImage> {
            let h = ((tile as f64 * mi.height.max(1) as f64 / mi.width.max(1) as f64) as usize).clamp(32, 480);
            let img = crate::media_io::frame(path, t, tile as usize, h, false).ok()?;
            // Letterbox into a square tile so different shapes line up.
            let side = tile as usize;
            let mut px = vec![18u8; side * side * 4];
            let (w, hh) = (img.size[0], img.size[1].min(side));
            let (ox, oy) = ((side - w.min(side)) / 2, (side - hh) / 2);
            for y in 0..hh {
                for x in 0..w.min(side) {
                    let c = img.pixels[y * img.size[0] + x];
                    let d = ((oy + y) * side + ox + x) * 4;
                    px[d..d + 4].copy_from_slice(&[c.r(), c.g(), c.b(), 255]);
                }
            }
            Some(studio_media::rgba::RgbaImage { width: tile, height: tile, pixels: px })
        };
        if let Some(t) = frame_of(&qpath, qt, &qmi) {
            tiles.push(t);
        }
        for (_, mi, t) in &picked {
            if let Some(img) = mi.path.as_ref().and_then(|p| frame_of(p, *t, mi)) {
                tiles.push(img);
            }
        }
        let cols = super::render::sheet_columns(tiles.len());
        let sheet = super::render::tiled(&tiles, cols, tile);
        Ok(vec![
            Content::Jpeg(studio_content::jpeg::encode(&sheet, super::render::LIGHT_QUALITY)),
            Content::Text(json!({"query": what, "by": mode, "matches": list, "sheet": "first tile is the query, then the matches in order; times are seconds into each file (use them as place's source_in)"}).to_string()),
        ])
    }
}
