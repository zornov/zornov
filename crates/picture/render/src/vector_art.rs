// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Vector illustration documents (`studio-vector-art`) as clips: a shape
//! generator whose `art` parameter is a `.svart` file (or the document's
//! JSON) draws that document's artboard, exactly, at the output's own
//! resolution, so art stays crisp in 4K and 8K exports.
//!
//! The artboard sits on the canvas centre at one document unit per canvas
//! pixel (`art_fit`: `contain` or `cover` scale it to the canvas instead).
//! Its animation plays on the clip's time, and properties bound to clip
//! parameters follow those parameters' keyframes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use studio_vector_art::Document;
use studio_vector_art::raster::kurbo::Affine;

use crate::frame::Frame;
use crate::scene::{Param, Params, ParamsExt};

type Key = (String, Option<SystemTime>);

/// Documents read once and kept while their file is unchanged.
fn load(src: &str) -> Option<Arc<Document>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<Document>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let inline = src.trim_start().starts_with('{');
    let key: Key = if inline {
        (src.to_string(), None)
    } else {
        (src.to_string(), std::fs::metadata(src).and_then(|m| m.modified()).ok())
    };
    if let Some(d) = cache.lock().ok()?.get(&key) {
        return Some(d.clone());
    }
    let doc = if inline { Document::from_json(src).ok()? } else { studio_vector_art::interop::open(std::path::Path::new(src)).ok()? };
    let doc = Arc::new(doc);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, doc.clone());
    }
    Some(doc)
}

fn param_json(p: &Param) -> Option<serde_json::Value> {
    use serde_json::json;
    Some(match p {
        Param::Bool(b) => json!(b),
        Param::Float(v) => json!(v),
        Param::Point(q) => json!(q),
        Param::Colour(c) => {
            let e = |v: f64| studio_vector_art::raster::color::linear_to_srgb(v as f32);
            json!(studio_vector_art::raster::Color::rgba(e(c[0]), e(c[1]), e(c[2]), c[3] as f32).to_hex())
        }
        Param::Text(t) | Param::Choice(t) => json!(t),
        Param::Curve(_) => return None,
    })
}

/// The vector GPU renderer, opened once (`None` without a GPU).
fn gpu() -> Option<Arc<studio_vector_art::raster::gpu::GpuRaster>> {
    static GPU: OnceLock<Option<Arc<studio_vector_art::raster::gpu::GpuRaster>>> = OnceLock::new();
    GPU.get_or_init(studio_vector_art::raster::gpu::GpuRaster::new).clone()
}

/// Draws the `art` document of a shape generator at `width` × `height`, or
/// `None` when the generator has no art.
pub fn draw(p: &Params, width: u32, height: u32) -> Option<Frame> {
    let src = p.text("art", "");
    if src.is_empty() {
        return None;
    }
    let Some(doc) = load(src) else { return Some(Frame::new(width, height)) };
    let t = p.f("clip_time", 0.0) as f64;
    let at = studio_vector_art::anim::at_time(&doc, t, &|name| p.get(name).and_then(param_json));
    let ab = at.artboard(Some(p.text("artboard", ""))).or_else(|| at.artboards.first()).cloned();
    let rect = ab.as_ref().map(|a| a.rect).unwrap_or(studio_vector_art::raster::kurbo::Rect::new(0.0, 0.0, width as f64, height as f64));
    let (cw, ch) = (p.f("canvas_width", width as f32) as f64, p.f("canvas_height", height as f32) as f64);
    let k = width as f64 / cw.max(1.0);
    let fit = match p.text("art_fit", "none") {
        "contain" => (cw / rect.width()).min(ch / rect.height()),
        "cover" => (cw / rect.width()).max(ch / rect.height()),
        _ => 1.0,
    };
    let opts = studio_vector_art::build::BuildOptions::default();
    let scene = if p.f("art_background", 0.0) > 0.5 { studio_vector_art::build::artboard_scene(&at, ab.as_ref(), &opts).0 } else { studio_vector_art::build::scene(&at, &opts) };
    let m = Affine::scale(k) * Affine::translate((cw / 2.0, ch / 2.0)) * Affine::scale(fit) * Affine::translate(-rect.center().to_vec2());
    let mut ro = studio_vector_art::raster::RenderOptions::new(width, height).with_transform(m);
    ro.dither = false;
    // Big frames (4K and up) draw on the GPU when there is one.
    let srgb: Vec<[f32; 4]> = match (width as u64 * height as u64 >= 4_000_000).then(gpu).flatten().and_then(|g| g.render(&scene, &ro).ok()) {
        Some(pm) => pm.data.chunks_exact(4).map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]).collect(),
        None => studio_vector_art::raster::render_f32(&scene, &ro),
    };
    // sRGB-encoded premultiplied to linear premultiplied.
    let px = srgb
        .into_iter()
        .map(|q| {
            let a = q[3];
            if a <= 1e-7 {
                return [0.0; 4];
            }
            let l = |c: f32| studio_vector_art::raster::color::srgb_to_linear((c / a).clamp(0.0, 1.0)) * a;
            [l(q[0]), l(q[1]), l(q[2]), a]
        })
        .collect();
    Some(Frame::from_pixels(width, height, px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_draws_at_output_resolution_and_animates() {
        let mut d = Document::new(1920.0, 1080.0);
        let mut ctx = studio_vector_art::ops::Context::default();
        ctx.base_dir = None;
        let r = studio_vector_art::ops::apply(&mut d, &serde_json::json!({"op": "circle", "cx": 960, "cy": 540, "r": 200, "fill": "#ff0000"}), &ctx).unwrap();
        let id = r["id"].clone();
        studio_vector_art::ops::apply(&mut d, &serde_json::json!({"op": "key", "id": id, "property": "@move", "t": 0, "value": [0, 0], "ease": "linear"}), &ctx).unwrap();
        studio_vector_art::ops::apply(&mut d, &serde_json::json!({"op": "key", "id": id, "property": "@move", "t": 1, "value": [400, 0], "ease": "linear"}), &ctx).unwrap();
        let mut p = Params::new();
        p.insert("art".into(), Param::Text(d.to_json()));
        p.insert("canvas_width".into(), Param::Float(1920.0));
        p.insert("canvas_height".into(), Param::Float(1080.0));
        // A 4K render of a 1080p canvas: the circle's centre at 2× canvas pixels.
        let f = draw(&p, 3840, 2160).unwrap();
        let c = f.get(1920, 1080);
        assert!((c[0] - 1.0).abs() < 1e-3 && c[1] < 1e-3 && (c[3] - 1.0).abs() < 1e-3, "{c:?}");
        assert!(f.get(1920 + 395, 1080)[3] > 0.99 && f.get(1920 + 405, 1080)[3] < 0.01);
        // Half way through the move the circle is 200 canvas pixels right.
        p.insert("clip_time".into(), Param::Float(0.5));
        let f = draw(&p, 1920, 1080).unwrap();
        assert!(f.get(960 + 200, 540)[3] > 0.99 && f.get(960 - 100, 540)[3] < 0.01);
    }
}
