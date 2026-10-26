// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Lottie player for Studio: reads a Lottie animation (`.json`, or a
//! `.lottie` archive) and draws any frame of it, with its transparency, on
//! tiny-skia.
//!
//! Drawn: precomposition, solid, image (embedded or beside the file),
//! null and shape layers; parenting; transforms with anchor, split
//! position, skew and opacity; keyframes with Bezier easing, holds and
//! spatial curves; groups, rectangles (rounded), ellipses, stars and
//! polygons, paths and path morphs; fills, strokes (caps, joins, dashes)
//! and linear or radial gradient fills and strokes; trim paths; masks
//! (add, subtract, intersect, inverted, opacity); alpha and luma track
//! mattes; blend modes; time stretch, start offsets and time remapping.
//! Not drawn (and listed in [`Animation::warnings`]): text layers, layer
//! effects, expressions, repeaters and the rarer path modifiers.
//!
//! Written from the public lottie-spec 1.0 format description, with
//! EffectCraft's Lottie importer (<https://github.com/storytold/effectcraft>,
//! `crates/picture/lottie`, MIT OR Apache-2.0, Copyright (c) 2026 ArtCraft Team and
//! the EffectCraft contributors) as the reference for how keyframe times,
//! `ip` / `op` / `st` / `sr` and the dotLottie archive fit together.

mod draw;
mod model;
mod value;
mod zip;

pub use value::ease;

use model::Layer;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

/// A composition: the root, or a precomp asset.
#[derive(Clone, Debug)]
struct Comp {
    layers: Vec<Layer>,
}

/// A parsed animation, ready to draw.
#[derive(Clone)]
pub struct Animation {
    pub width: u32,
    pub height: u32,
    /// Frames per second.
    pub fps: f64,
    /// First and last (exclusive) frame.
    pub in_frame: f64,
    pub out_frame: f64,
    pub name: String,
    /// What this player does not draw, in words.
    pub warnings: Vec<String>,
    root: Comp,
    comps: BTreeMap<String, Comp>,
    images: BTreeMap<String, Arc<tiny_skia::Pixmap>>,
}

impl std::fmt::Debug for Animation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Animation({}x{} @ {} fps, frames {}..{})", self.width, self.height, self.fps, self.in_frame, self.out_frame)
    }
}

/// Whether `bytes` look like a Lottie document or a dotLottie archive.
pub fn sniff(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"PK\x03\x04") {
        return zip::entries(bytes).is_ok_and(|e| e.iter().any(|(n, _)| n.ends_with("manifest.json") || (n.starts_with("animations/") && n.ends_with(".json"))));
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let t = head.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('{') && head.contains("\"layers\"") && (head.contains("\"fr\"") || head.contains("\"ip\"")) || (t.starts_with('{') && head.contains("\"v\"") && head.contains("\"fr\""))
}

fn decode_image(bytes: &[u8]) -> Option<tiny_skia::Pixmap> {
    let img = if bytes.starts_with(b"\x89PNG") { studio_media::png::decode(bytes).ok()? } else { studio_media::jpeg::decode(bytes).ok()? };
    let mut px = tiny_skia::Pixmap::new(img.width, img.height)?;
    for (d, s) in px.pixels_mut().iter_mut().zip(img.pixels.chunks_exact(4)) {
        *d = tiny_skia::ColorU8::from_rgba(s[0], s[1], s[2], s[3]).premultiply();
    }
    Some(px)
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * k);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

impl Animation {
    /// Reads a `.json` or `.lottie` file (images beside a `.json` are
    /// looked up next to it).
    pub fn open(path: &Path) -> Result<Animation, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Animation::from_bytes(&bytes, path.parent())
    }

    /// The Lottie document itself, from a `.json` file or the first
    /// animation of a `.lottie` archive (for importing its shapes).
    pub fn document(bytes: &[u8]) -> Result<Value, String> {
        if bytes.starts_with(b"PK\x03\x04") {
            let files: BTreeMap<String, Vec<u8>> = zip::entries(bytes)?.into_iter().collect();
            let first = files
                .get("manifest.json")
                .and_then(|m| serde_json::from_slice::<Value>(m).ok())
                .and_then(|m| m["animations"].get(0).and_then(|a| a.get("id")).and_then(Value::as_str).map(|id| format!("animations/{id}.json")))
                .filter(|n| files.contains_key(n))
                .or_else(|| files.keys().find(|n| n.starts_with("animations/") && n.ends_with(".json")).cloned())
                .ok_or("the .lottie archive holds no animation")?;
            return serde_json::from_slice(&files[&first]).map_err(|e| format!("not a Lottie document: {e}"));
        }
        serde_json::from_slice(bytes).map_err(|e| format!("not a Lottie document: {e}"))
    }

    /// Reads a Lottie document or dotLottie archive from memory; `dir` is
    /// where external images live.
    pub fn from_bytes(bytes: &[u8], dir: Option<&Path>) -> Result<Animation, String> {
        if bytes.starts_with(b"PK\x03\x04") {
            let entries = zip::entries(bytes)?;
            let files: BTreeMap<String, Vec<u8>> = entries.into_iter().collect();
            // The first animation the manifest names, else the first one.
            let first = files
                .get("manifest.json")
                .and_then(|m| serde_json::from_slice::<Value>(m).ok())
                .and_then(|m| m["animations"].get(0).and_then(|a| a.get("id")).and_then(Value::as_str).map(|id| format!("animations/{id}.json")))
                .filter(|n| files.contains_key(n))
                .or_else(|| files.keys().find(|n| n.starts_with("animations/") && n.ends_with(".json")).cloned())
                .ok_or("the .lottie archive holds no animation")?;
            let doc: Value = serde_json::from_slice(&files[&first]).map_err(|e| format!("not a Lottie document: {e}"))?;
            let images = |name: &str| files.get(&format!("images/{name}")).or_else(|| files.get(name)).cloned();
            return Animation::from_value(&doc, &images);
        }
        let doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("not a Lottie document: {e}"))?;
        let images = |name: &str| dir.and_then(|d| std::fs::read(d.join(name)).ok());
        Animation::from_value(&doc, &images)
    }

    fn from_value(doc: &Value, external: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Animation, String> {
        let num = |k: &str| doc.get(k).and_then(Value::as_f64);
        let (w, h, fps) = (num("w").ok_or("no width (\"w\")")?, num("h").ok_or("no height (\"h\")")?, num("fr").ok_or("no frame rate (\"fr\")")?);
        if !(w >= 1.0 && h >= 1.0 && w <= 16384.0 && h <= 16384.0) || !(fps > 0.0 && fps <= 1000.0) {
            return Err(format!("unusable size or rate: {w}x{h} at {fps} fps"));
        }
        if doc.get("layers").and_then(Value::as_array).is_none() {
            return Err("no layers".into());
        }
        let mut unsupported = BTreeSet::new();
        let root = Comp { layers: model::layers(doc.get("layers"), &mut unsupported) };
        let mut comps = BTreeMap::new();
        let mut images = BTreeMap::new();
        let mut missing = Vec::new();
        for a in doc.get("assets").and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = a.get("id").and_then(Value::as_str) else { continue };
            if a.get("layers").is_some() {
                comps.insert(id.to_owned(), Comp { layers: model::layers(a.get("layers"), &mut unsupported) });
                continue;
            }
            let p = a.get("p").and_then(Value::as_str).unwrap_or("");
            let bytes = if let Some(data) = p.strip_prefix("data:") {
                data.split_once(";base64,").and_then(|(_, b)| base64(b))
            } else {
                let u = a.get("u").and_then(Value::as_str).unwrap_or("");
                external(&format!("{u}{p}")).or_else(|| external(p))
            };
            match bytes.as_deref().and_then(decode_image) {
                Some(px) => {
                    images.insert(id.to_owned(), Arc::new(px));
                }
                None => missing.push(p.chars().take(40).collect::<String>()),
            }
        }
        let mut warnings: Vec<String> = unsupported.into_iter().map(|u| format!("not drawn: {u}")).collect();
        if !missing.is_empty() {
            warnings.push(format!("images not found: {}", missing.join(", ")));
        }
        let ip = num("ip").unwrap_or(0.0);
        let op = num("op").unwrap_or(ip + fps).max(ip + 1.0);
        Ok(Animation { width: w.round() as u32, height: h.round() as u32, fps, in_frame: ip, out_frame: op, name: doc.get("nm").and_then(Value::as_str).unwrap_or("").to_owned(), warnings, root, comps, images })
    }

    /// The size Studio draws it at: its own, scaled up (at most 4×) until the
    /// longer side is 1920 pixels, so a small animation stays sharp on a
    /// full HD canvas.
    pub fn picture_size(&self) -> (u32, u32) {
        let long = self.width.max(self.height) as f64;
        let k = (1920.0 / long).clamp(1.0, 4.0);
        ((self.width as f64 * k).round() as u32, (self.height as f64 * k).round() as u32)
    }

    /// Frames in the animation.
    pub fn frame_count(&self) -> u64 {
        (self.out_frame - self.in_frame).ceil().max(1.0) as u64
    }

    /// Seconds.
    pub fn duration(&self) -> f64 {
        (self.out_frame - self.in_frame) / self.fps
    }

    /// Frame `frame` (an animation frame number, `in_frame` first) at
    /// `width` × `height`, premultiplied RGBA.
    pub fn render(&self, frame: f64, width: u32, height: u32) -> tiny_skia::Pixmap {
        let mut px = tiny_skia::Pixmap::new(width.max(1), height.max(1)).expect("non-zero size");
        let k = [width as f64 / self.width as f64, height as f64 / self.height as f64];
        draw::comp(self, &self.root, frame, &mut px, model::scale(k[0], k[1]), 0);
        px
    }

    /// The same, as straight (not premultiplied) RGBA bytes.
    pub fn render_rgba(&self, frame: f64, width: u32, height: u32) -> Vec<u8> {
        let px = self.render(frame, width, height);
        px.pixels().iter().flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        }).collect()
    }
}

/// A dotLottie archive of `files` (name, bytes), stored uncompressed.
pub fn write_dotlottie(files: &[(&str, &[u8])]) -> Vec<u8> {
    zip::write(files)
}

#[cfg(test)]
mod tests;
