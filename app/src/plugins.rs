// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugins in the app: starting them, the project as plugins see it, and
//! files read by plugin importers. The host is `studio-plugins`; the agent's
//! `plugins` tool and the AI library's Installed / Available lists use
//! this module.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::doc::Doc;
use crate::view::{self, LaneKind, SEC};
pub use studio_plugins::{Host, Imported, Installed, catalogue, ofx};

/// The plugin host for Studio's plugin folder.
pub fn host() -> &'static Host {
    studio_plugins::host()
}

/// Starts the enabled plugins, then (on a thread, as OFX workers take a
/// moment) OpenFX bundles when OFX is turned on. Safe to call more than
/// once.
pub fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        studio_plugins::start();
        let dir = studio_plugins::plugins_dir();
        if ofx::Settings::load(&dir).enabled {
            let _ = std::thread::Builder::new().name("ofx-load".into()).spawn(move || ofx::load(&dir));
        }
    });
}

/// Lane numbers as plugins get them: the main lane 0, overlays 1, 2, ...
/// upwards, sound lanes -1, -2, ... downwards.
fn lane_numbers(p: &view::Project) -> Vec<(u32, i32)> {
    let spine = p.lanes.iter().position(|l| l.kind == LaneKind::Spine);
    let mut out = Vec::new();
    let (mut up, mut down) = (0, 0);
    // Lanes are listed top to bottom.
    for (i, l) in p.lanes.iter().enumerate().rev() {
        let n = match l.kind {
            LaneKind::Spine => 0,
            LaneKind::Audio => {
                down -= 1;
                down
            }
            LaneKind::Visual if spine.is_some_and(|s| i > s) => {
                // An overlay drawn under the main lane is unusual; count it
                // with the sound lanes' side so numbers stay unique.
                down -= 1;
                down
            }
            LaneKind::Visual => {
                up += 1;
                up
            }
        };
        out.push((l.id, n));
    }
    out
}

/// The project as exporters and panels see it: `name`, `canvas`, `fps`,
/// `duration`, `clips` (id, name, kind, lane, lane_id, start, dur, text,
/// effects, media path), `markers` (at, note). Times in seconds.
pub fn project_json(p: &view::Project) -> Value {
    let lanes = lane_numbers(p);
    let s = |t: view::Time| t as f64 / SEC as f64;
    let mut clips: Vec<&view::Clip> = p.clips.iter().collect();
    clips.sort_by_key(|c| (c.start, c.lane));
    json!({
        "name": p.name,
        "canvas": [p.width, p.height],
        "fps": p.fps,
        "duration": s(p.duration()),
        "clips": clips.iter().map(|c| json!({
            "id": c.id,
            "name": c.name,
            "kind": format!("{:?}", c.kind).to_lowercase(),
            "lane": lanes.iter().find(|(id, _)| *id == c.lane).map_or(0, |(_, n)| *n),
            "lane_id": c.lane,
            "start": s(c.start),
            "dur": s(c.dur),
            "text": c.text,
            "effects": c.effects.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
            "media": c.media.and_then(|m| p.media.iter().find(|x| x.id == m)).and_then(|m| m.path.as_ref()).map(|p| p.display().to_string()),
        })).collect::<Vec<_>>(),
        "markers": p.markers.iter().map(|m| json!({"at": s(m.time), "note": m.note})).collect::<Vec<_>>(),
    })
}

/// Where pictures made by importers are saved: the project's folder, or
/// Studio's cache for an unsaved project.
fn imports_dir(doc: &Doc) -> PathBuf {
    match &doc.folder {
        Some(f) => f.root().join("Imported"),
        None => studio_paths::cache().join("plugin-imports"),
    }
}

/// What a plugin import did.
pub enum ImportDone {
    /// A picture saved as a PNG and added to the media bin.
    Media { id: u32, file: PathBuf, plugin: String },
    /// Edit ops for the caller to apply as one undo step.
    Ops(Value),
}

/// Whether some enabled plugin reads this file.
pub fn imports(path: &Path) -> bool {
    path.extension().is_some_and(|e| !host().importers_for(&e.to_string_lossy()).is_empty())
}

/// Whether a file should go to a plugin importer: Studio does not read
/// its kind itself and an enabled plugin does.
pub fn takes(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let own = crate::media_io::VIDEO_EXT.iter().chain(crate::media_io::AUDIO_EXT).chain(crate::media_io::IMAGE_EXT).chain(crate::media_io::LOTTIE_EXT).any(|e| *e == ext) || crate::psd_import::is_psd(path);
    !own && imports(path)
}

/// Reads a file with a plugin importer (the first for its extension, or
/// `importer` = (plugin, importer id)).
pub fn import(doc: &mut Doc, path: &Path, importer: Option<(&str, &str)>) -> Result<ImportDone, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let plugin = importer.map(|(p, _)| p.to_owned()).or_else(|| host().importers_for(&ext).first().map(|(p, _, _)| p.clone())).unwrap_or_default();
    match host().import(path, importer)? {
        Imported::Image { width, height, rgba8 } => {
            let dir = imports_dir(doc);
            std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "imported".into());
            let mut file = dir.join(format!("{stem}.png"));
            let mut n = 2;
            while file.exists() {
                file = dir.join(format!("{stem} {n}.png"));
                n += 1;
            }
            let mut out = Vec::new();
            {
                let mut e = png::Encoder::new(&mut out, width, height);
                e.set_color(png::ColorType::Rgba);
                e.set_depth(png::BitDepth::Eight);
                e.write_header().and_then(|mut w| w.write_image_data(&rgba8)).map_err(|e| format!("could not encode the picture: {e}"))?;
            }
            std::fs::write(&file, out).map_err(|e| format!("could not save {}: {e}", file.display()))?;
            let report = crate::services::project::import_media(doc, std::slice::from_ref(&file));
            let id = *report.ids.first().ok_or_else(|| report.failed.join("; "))?;
            Ok(ImportDone::Media { id, file, plugin })
        }
        Imported::Ops(ops) => Ok(ImportDone::Ops(ops)),
    }
}

/// Where plugin exports go by default: the Videos folder's Studio exports.
pub fn export_path(project: &str, name: &str, ext: &str) -> PathBuf {
    let dir = studio_paths::studio().join("Exports");
    let safe: String = format!("{project} {name}").chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    dir.join(format!("{}.{ext}", safe.trim()))
}

/// Everything installed and available, for the library and the agent.
pub fn overview() -> Value {
    let installed = host().list();
    let available = catalogue::available(host());
    json!({
        "installed": installed.iter().map(Installed::json).collect::<Vec<_>>(),
        "available": available.iter().filter(|e| !installed.iter().any(|p| p.id() == e.id && !catalogue::newer(&e.version, &p.manifest.version))).map(|e| e.json(installed.iter().find(|p| p.id() == e.id))).collect::<Vec<_>>(),
        "ofx": ofx_json(),
        "folder": host().dir().display().to_string(),
    })
}

pub fn ofx_json() -> Value {
    let s = ofx::Settings::load(host().dir());
    json!({
        "enabled": s.enabled,
        "plugins": ofx::loaded().iter().map(|l| json!({"id": l.studio_id, "name": l.label, "kind": l.kind, "group": l.grouping, "enabled": l.enabled, "bundle": l.bundle.display().to_string()})).collect::<Vec<_>>(),
        "failed": ofx::failed().iter().map(|(p, e)| json!({"bundle": p.display().to_string(), "error": e})).collect::<Vec<_>>(),
        "bundles_found": ofx::find_bundles(&s.paths, s.only_paths).len(),
    })
}

/// Turns OpenFX loading on or off and (re)loads.
pub fn set_ofx(on: bool) -> Result<Value, String> {
    let dir = host().dir().to_owned();
    let mut s = ofx::Settings::load(&dir);
    s.enabled = on;
    s.save(&dir)?;
    ofx::load(&dir);
    Ok(ofx_json())
}
