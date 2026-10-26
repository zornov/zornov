// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Files: the document's own JSON (`.svart`), SVG, PDF, EPS and PNG, in
//! and out, picked by extension.

pub mod eps;
pub mod pdf;
pub mod svg;

use std::path::Path;

use studio_vector_raster::Image;

use crate::build::{self, BuildOptions};
use crate::doc::Document;

/// Decodes PNG, JPEG, WebP, GIF, TIFF or BMP bytes.
pub fn decode_image(bytes: &[u8]) -> Option<Image> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Image::from_straight(w, h, img.into_raw())
}

pub fn load_image(path: &Path) -> Option<Image> {
    decode_image(&std::fs::read(path).ok()?)
}

/// PNG bytes of an image (straight alpha).
pub fn encode_png(img: &Image) -> Vec<u8> {
    let px = studio_vector_raster::Pixmap { width: img.width, height: img.height, data: img.data.to_vec() };
    png_bytes(&px)
}

pub fn png_bytes(p: &studio_vector_raster::Pixmap) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, p.width, p.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        if let Ok(mut w) = e.write_header() {
            let _ = w.write_image_data(&p.to_straight());
        }
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Opens a vector document: `.svart`/`.json` (Studio's own), `.svg`,
/// `.svgz`, `.pdf`, `.eps`/`.ps`/`.ai` (PostScript-based), or a raster
/// image (placed on an artboard of its size, ready to trace).
pub fn open(path: &Path) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut doc = match ext(path).as_str() {
        "svart" | "json" => Document::from_json(&String::from_utf8_lossy(&bytes))?,
        "svg" => svg::import(&String::from_utf8_lossy(&bytes))?,
        "svgz" => {
            let raw = miniz_oxide::inflate::decompress_to_vec(bytes.get(10..).unwrap_or(&[])).map_err(|_| "a damaged .svgz".to_string())?;
            svg::import(&String::from_utf8_lossy(&raw))?
        }
        "pdf" => pdf::import(&bytes)?,
        "ai" if bytes.starts_with(b"%PDF") => pdf::import(&bytes)?,
        "eps" | "ps" | "ai" | "epsf" => eps::import(&bytes)?,
        _ => {
            let img = decode_image(&bytes).ok_or_else(|| format!("{}: not a vector file or image this reads", path.display()))?;
            let mut d = Document::new(img.width as f64, img.height as f64);
            let n = crate::doc::Node::new(0, crate::doc::NodeKind::Image(crate::doc::ImageNode { width: img.width as f64, height: img.height as f64, image: Some(img), href: None }));
            d.add(None, n);
            d
        }
    };
    if doc.name.is_empty() {
        doc.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string();
    }
    Ok(doc)
}

/// What [`save`] can write.
pub const FORMATS: &[&str] = &["svart", "svg", "pdf", "eps", "png"];

/// Writes the document (or one artboard) by extension. PNG takes `scale`
/// pixels per document unit (2 = retina, 4 = an 8K frame from a 1080p artboard).
pub fn save(doc: &Document, path: &Path, artboard: Option<&str>, scale: f64, opts: &BuildOptions) -> Result<(), String> {
    let data = match ext(path).as_str() {
        "svart" | "json" => doc.to_json().into_bytes(),
        "svg" => svg::export(doc, artboard).into_bytes(),
        "pdf" => pdf::export(doc, artboard, opts),
        "eps" | "ps" => eps::export(doc, artboard, opts).into_bytes(),
        "png" => {
            let ab = doc.artboard(artboard).ok_or("the document has no artboard")?;
            let s = scale.clamp(0.01, 64.0);
            let (w, h) = ((ab.rect.width() * s).round().max(1.0) as u32, (ab.rect.height() * s).round().max(1.0) as u32);
            if w as u64 * h as u64 > 400_000_000 {
                return Err(format!("{w} × {h} pixels is more than one image can hold"));
            }
            png_bytes(&build::render_artboard(doc, artboard, w, h, opts))
        }
        other => return Err(format!("can't write .{other} (try {})", FORMATS.join(", "))),
    };
    std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
}
