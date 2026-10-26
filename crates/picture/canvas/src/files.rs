// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Adapted from the file helpers of the automation crate of PhotoCraft
// (github.com/storytold/photocraft, crates/automation/src/files.rs),
// Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors,
// MIT OR Apache-2.0. See NOTICE.

//! Opening and saving pictures: every format the pixel engine reads (PSD and
//! PSB with their layers, TIFF, PNG, JPEG, WebP, HEIC, EXR, camera raw, GIF,
//! BMP, TGA, OpenRaster, Krita, XCF, Paint.NET …) and Studio's own lossless
//! picture bundle (`.pcraft`: every layer, mask, adjustment and smart object
//! with its source).

use std::path::Path;

use studio_pixel_doc::Document;
use studio_pixel_format::{PcraftWriter, SaveOptions};
use studio_pixel_io::ExportOptions;

/// The lossless native picture bundle's extension.
pub const NATIVE: &str = studio_pixel_format::EXTENSION;

/// A document read from a file, with notes about anything approximated.
pub struct Opened {
    pub document: Document,
    pub warnings: Vec<String>,
}

/// Decode a picture from its bytes; `name` gives the format by its extension.
pub fn open_bytes(name: &str, bytes: &[u8]) -> Result<Opened, String> {
    if Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case(NATIVE)) {
        let document = studio_pixel_format::load_from_bytes(bytes).map_err(|e| e.to_string())?;
        return Ok(Opened { warnings: studio_pixel_io::open_warnings(&document), document });
    }
    let r = studio_pixel_io::import(name, bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok(Opened { document: r.document, warnings: r.warnings })
}

/// Open a picture from disk (a native bundle folder or any supported file).
pub fn open(path: &Path) -> Result<Opened, String> {
    if path.is_dir() {
        let document = studio_pixel_format::load_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(Opened { warnings: studio_pixel_io::open_warnings(&document), document });
    }
    let bytes = studio_pixel_format::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    open_bytes(&name, &bytes)
}

fn previews(doc: &Document) -> SaveOptions {
    SaveOptions { thumbnail: Some(studio_pixel_compose::thumbnail(doc, 256)), composite: Some(studio_pixel_compose::thumbnail(doc, 1024)) }
}

/// The extension `path` (or `format`, like `"png"`) asks for, lower case.
pub fn format_of(path: &Path, format: Option<&str>) -> Result<String, String> {
    format
        .map(|f| f.trim_start_matches('.').to_ascii_lowercase())
        .or_else(|| path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()))
        .ok_or_else(|| format!("cannot tell the format of `{}`; give a format", path.display()))
}

/// Save or export `doc` to `path`, the format from the extension (or
/// `format`). Returns notes about anything the format cannot hold. The
/// write is atomic: a failed save never destroys the previous file.
pub fn save(doc: &Document, path: &Path, format: Option<&str>, opts: &ExportOptions) -> Result<Vec<String>, String> {
    let ext = format_of(path, format)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if ext == NATIVE {
        let mut named = doc.clone();
        if let Some(n) = path.file_name() {
            named.name = n.to_string_lossy().into_owned();
        }
        let bytes = studio_pixel_format::save_to_bytes(&named, &previews(&named)).map_err(|e| e.to_string())?;
        studio_pixel_format::atomic_write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(Vec::new());
    }
    let r = studio_pixel_io::export(doc, &ext, opts).map_err(|e| e.to_string())?;
    studio_pixel_format::atomic_write(path, &r.bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(r.warnings)
}

/// A reusable writer is not needed for Studio's saves; kept for incremental
/// saves of very large pictures.
pub fn save_incremental(doc: &Document, path: &Path, writer: &mut PcraftWriter) -> Result<(), String> {
    writer.save_path(doc, path, &previews(doc)).map(|_| ()).map_err(|e| e.to_string())
}

/// Every extension `open` reads, for file pickers.
pub const READS: &[&str] = &[
    "psd", "psb", "pcraft", "tif", "tiff", "png", "jpg", "jpeg", "webp", "heic", "heif", "exr", "gif", "bmp", "tga", "qoi", "hdr", "ora", "kra", "xcf", "pdn", "svg", "dng", "cr2",
    "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "srw",
];

/// The formats `save` writes, for file pickers.
pub const WRITES: &[&str] = &["psd", "psb", "pcraft", "tif", "png", "jpg", "webp", "exr", "gif", "bmp", "tga"];
