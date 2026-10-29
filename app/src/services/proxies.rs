// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Proxies: Apple ProRes 422 LT copies of heavy media at half or
//! quarter size (`studio_render::proxy`), kept in the project's `Proxies`
//! folder (or the Proxies folder in Settings), recorded on each media item
//! (`proxy_path`, and `proxy_input` in its extra fields when the proxy's
//! colour needs naming), and used by the viewer when the project's
//! `use_proxies` setting is on. Export always reads the originals; a proxy
//! stands in wherever its original file is missing.

use crate::doc::Doc;
use std::path::{Path, PathBuf};
use studio_model as m;
use studio_render::source::Proxy;

/// Project setting (in the settings' extra fields) that turns proxies on.
pub const USE_KEY: &str = "use_proxies";
const INPUT_KEY: &str = "proxy_input";

/// Whether the project plays its proxies.
pub fn enabled(p: &m::Project) -> bool {
    p.settings.extra.get(USE_KEY).and_then(|v| v.as_bool()) == Some(true)
}

/// The proxy to register for `item`: when proxies are on, or when the
/// original is missing.
pub fn proxy_for(item: &m::MediaItem, on: bool) -> Option<Proxy> {
    let path = PathBuf::from(item.proxy_path.as_deref()?);
    let original_missing = !Path::new(&item.path).exists();
    if !(on || original_missing) || !path.exists() {
        return None;
    }
    let (w, h) = item.info.video.as_ref().map_or((0, 0), |v| (v.width, v.height));
    if w == 0 || h == 0 {
        return None;
    }
    let input = item.extra.get(INPUT_KEY).and_then(|v| v.as_str()).and_then(|s| studio_render::colour_space::InputColour::parse(s, None).ok());
    Some(Proxy { path, size: (w, h), input })
}

/// The edit turning proxies on or off.
pub fn set_enabled(p: &m::Project, on: bool) -> m::Edit {
    let mut s = p.settings.clone();
    if on {
        s.extra.insert(USE_KEY.into(), serde_json::json!(true));
    } else {
        s.extra.remove(USE_KEY);
    }
    m::Edit::SetSettings { settings: Box::new(s) }
}

/// Where proxies of this project go.
pub fn folder(doc: &Doc) -> PathBuf {
    match &doc.folder {
        Some(f) => f.proxies_dir(),
        None => PathBuf::from(crate::prefs::Prefs::load(None).proxy_dir),
    }
}

/// One proxy made.
#[derive(Clone, Debug)]
pub struct Made {
    pub media: m::MediaId,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// Makes proxies for `ids` (video only; ones already made are kept unless
/// `again`) at 1/`scale` size. Returns what was made and one line per
/// file that failed; the caller applies [`record`] as one undo step.
pub fn make(doc: &Doc, ids: &[m::MediaId], scale: u32, again: bool, progress: &mut dyn FnMut(&str, u64, u64) -> bool) -> (Vec<(Made, Option<String>)>, Vec<String>) {
    let dir = folder(doc);
    let mut made = Vec::new();
    let mut failed = Vec::new();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        failed.push(format!("{}: {e}", dir.display()));
        return (made, failed);
    }
    for id in ids {
        let Some(item) = doc.model.media_item(*id) else { continue };
        if item.info.kind != m::MediaKind::Video || item.path.starts_with("offline:") || item.path.starts_with("sample:") {
            continue;
        }
        if !again && item.proxy_path.as_deref().is_some_and(|p| Path::new(p).exists()) {
            continue;
        }
        let stem = Path::new(&item.path).file_stem().map_or_else(|| format!("media{}", id.0), |s| s.to_string_lossy().replace('#', ""));
        let out = dir.join(format!("{stem} (proxy {}).mov", id.0));
        let tc = item.timecode.as_deref().and_then(|t| {
            let rate = item.timecode_rate(doc.model.settings.frame_rate);
            let tcd = m::Timecode::new(rate, t.contains(';'));
            Some(studio_mov::StartTimecode { frame: tcd.parse_lenient(t)? as u32, fps: tcd.nominal_fps() as u32, drop_frame: tcd.is_drop_frame(), reel: item.reel.clone() })
        });
        let name = item.name.clone();
        match studio_render::proxy::make_proxy(Path::new(&item.path), &out, scale, tc, &mut |d, t| progress(&name, d, t)) {
            Ok(info) => made.push((Made { media: *id, path: out, width: info.width, height: info.height }, info.input)),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    (made, failed)
}

/// The edit recording made proxies on their media items.
pub fn record(p: &m::Project, made: &[(Made, Option<String>)]) -> m::Edit {
    let mut edits = Vec::new();
    for (mk, input) in made {
        let Some(item) = p.media_item(mk.media) else { continue };
        let mut it = item.clone();
        it.proxy_path = Some(mk.path.display().to_string());
        match input {
            Some(s) => {
                it.extra.insert(INPUT_KEY.into(), serde_json::json!(s));
            }
            None => {
                it.extra.remove(INPUT_KEY);
            }
        }
        edits.push(m::Edit::SetMedia { item: Box::new(it) });
    }
    m::Edit::Batch { edits }
}

/// The edit forgetting proxies (the files stay on disk).
pub fn forget(p: &m::Project, ids: &[m::MediaId]) -> m::Edit {
    let edits = ids
        .iter()
        .filter_map(|id| p.media_item(*id))
        .filter(|i| i.proxy_path.is_some())
        .map(|i| {
            let mut it = i.clone();
            it.proxy_path = None;
            it.extra.remove(INPUT_KEY);
            m::Edit::SetMedia { item: Box::new(it) }
        })
        .collect();
    m::Edit::Batch { edits }
}
