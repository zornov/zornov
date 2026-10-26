// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `search_media`: look for real pictures, video, sound and 3D before making
//! any. It asks the library first, then the open web like a search engine
//! (no key, no account), and adds openly published collections as extra
//! results. A chosen result is brought into the project through the same
//! path as `download`, and the page it came from is kept quietly with the
//! file (the asset's source and link), nothing more.
//!
//! Sources, all keyless:
//! * the web: Bing image and video result pages (read as HTML);
//! * Openverse (pictures and sound), Wikimedia Commons (pictures, video,
//!   sound) and Poly Haven (3D models, textures, HDRIs), through their
//!   public JSON interfaces.
//!
//! Parsing is separate from fetching so it is tested on recorded replies.

use super::{Content, Session};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36 Studio";
const DEFAULT_MAX: usize = 8;
const MOST: usize = 20;
/// Thumbnails shown in the contact sheet.
const SHEET_MAX: usize = 12;
const PAGE_BYTES: u64 = 3 << 20;

/// One result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hit {
    /// Which source answered (`web`, `openverse`, `commons`, `polyhaven`).
    pub source: &'static str,
    /// `image`, `video`, `audio`, `3d`, `texture` or `hdri`.
    pub kind: &'static str,
    pub title: String,
    /// What `download` is given (a file or a page); `polyhaven:type:id` for Poly Haven.
    pub url: String,
    pub thumb: String,
    /// The page it was found on.
    pub page: String,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub author: String,
}

impl Hit {
    fn new(source: &'static str, kind: &'static str) -> Hit {
        Hit { source, kind, ..Default::default() }
    }
}

/// The last answer, so `fetch: 3` means result 3.
static LAST: Mutex<Vec<Hit>> = Mutex::new(Vec::new());

// ------------------------------------------------------------- text helpers

/// `&quot;`-style entities back to characters.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "quot" => Some('"'),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent.strip_prefix('#').and_then(|n| match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok(),
                None => n.parse().ok(),
            }).and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text without tags, entities decoded.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else if b == b' ' {
            o.push('+');
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The values of every `attr="..."` in `html`, entities decoded.
fn attr_values(html: &str, attr: &str) -> Vec<String> {
    let key = format!(" {attr}=\"");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = html[at..].find(&key) {
        let start = at + i + key.len();
        let Some(len) = html[start..].find('"') else { break };
        out.push(unescape(&html[start..start + len]));
        at = start + len;
    }
    out
}

fn s_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_owned()
}

fn n_of(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

// ------------------------------------------------------------------ parsers

/// Openverse: `/v1/images/` or `/v1/audio/`.
pub fn parse_openverse(body: &str, kind: &'static str) -> Vec<Hit> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    v["results"].as_array().into_iter().flatten().filter_map(|r| {
        let url = s_of(r, "url");
        if url.is_empty() {
            return None;
        }
        let mut h = Hit::new("openverse", kind);
        h.title = s_of(r, "title");
        h.thumb = s_of(r, "thumbnail");
        h.page = s_of(r, "foreign_landing_url");
        h.author = s_of(r, "creator");
        h.width = n_of(r, "width") as u32;
        h.height = n_of(r, "height") as u32;
        h.seconds = n_of(r, "duration") / 1000.0;
        h.url = url;
        Some(h)
    }).collect()
}

/// Wikimedia Commons `action=query` with `prop=imageinfo`; only files whose
/// type starts with one of `mimes`.
pub fn parse_commons(body: &str, kind: &'static str, mimes: &[&str]) -> Vec<Hit> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    let mut pages: Vec<&Value> = v["query"]["pages"].as_object().into_iter().flat_map(|m| m.values()).collect();
    pages.sort_by_key(|p| p["index"].as_i64().unwrap_or(0));
    pages.into_iter().filter_map(|p| {
        let info = p["imageinfo"].get(0)?;
        let mime = s_of(info, "mime");
        if !mimes.iter().any(|m| mime.starts_with(m)) {
            return None;
        }
        let mut h = Hit::new("commons", kind);
        h.title = s_of(p, "title").trim_start_matches("File:").to_owned();
        h.url = s_of(info, "url");
        h.thumb = s_of(info, "thumburl");
        h.page = s_of(info, "descriptionurl");
        h.width = n_of(info, "width") as u32;
        h.height = n_of(info, "height") as u32;
        h.author = plain(info["extmetadata"]["Artist"]["value"].as_str().unwrap_or(""));
        (!h.url.is_empty()).then_some(h)
    }).collect()
}

/// Poly Haven `/assets?t=...`: everything, filtered here by the words.
pub fn parse_polyhaven(body: &str, kind: &'static str, query: &str) -> Vec<Hit> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).filter(|w| w.len() > 1).collect();
    let mut scored: Vec<(usize, String, &Value)> = v.as_object().into_iter().flatten().filter_map(|(id, a)| {
        let hay = format!("{} {} {} {}", id, s_of(a, "name"), a["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(" "), a["categories"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).to_lowercase();
        let n = words.iter().filter(|w| hay.contains(w.as_str())).count();
        (n > 0).then(|| (n, id.clone(), a))
    }).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let ty = match kind {
        "texture" => "textures",
        "hdri" => "hdris",
        _ => "models",
    };
    scored.into_iter().map(|(_, id, a)| {
        let mut h = Hit::new("polyhaven", kind);
        h.title = s_of(a, "name");
        h.url = format!("polyhaven:{ty}:{id}");
        h.thumb = format!("https://cdn.polyhaven.com/asset_img/thumbs/{id}.png?width=256&height=256");
        h.page = format!("https://polyhaven.com/a/{id}");
        h.author = a["authors"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default();
        h
    }).collect()
}

/// Bing image results (`class="iusc"` links with a `m` JSON attribute).
pub fn parse_web_images(html: &str) -> Vec<Hit> {
    attr_values(html, "m").into_iter().filter_map(|m| {
        let v: Value = serde_json::from_str(&m).ok()?;
        let url = s_of(&v, "murl");
        if !url.starts_with("http") {
            return None;
        }
        let mut h = Hit::new("web", "image");
        h.title = s_of(&v, "t");
        h.thumb = s_of(&v, "turl");
        h.page = s_of(&v, "purl");
        h.url = url;
        Some(h)
    }).collect()
}

/// Bing video results (`vrhm` JSON attribute); the link is the page that
/// plays the video, which `download` reads.
pub fn parse_web_videos(html: &str) -> Vec<Hit> {
    let mut seen = std::collections::HashSet::new();
    attr_values(html, "vrhm").into_iter().filter_map(|m| {
        let v: Value = serde_json::from_str(&m).ok()?;
        let url = s_of(&v, "murl");
        if !url.starts_with("http") || !seen.insert(url.clone()) {
            return None;
        }
        let mut h = Hit::new("web", "video");
        h.title = s_of(&v, "vt");
        let thid = s_of(&v, "thid");
        if !thid.is_empty() {
            h.thumb = format!("https://tse1.mm.bing.net/th?id={}&pid=Api", encode(&thid).replace('+', "%2B"));
        }
        h.page = s_of(&v, "pgurl");
        h.seconds = clock(&s_of(&v, "du"));
        h.url = url;
        Some(h)
    }).collect()
}

/// "01:02:03" or "00:25" to seconds.
fn clock(s: &str) -> f64 {
    s.split(':').try_fold(0.0, |acc, p| p.parse::<f64>().ok().map(|n| acc * 60.0 + n)).unwrap_or(0.0)
}

/// What a Poly Haven asset's `/files/<id>` reply gives to download:
/// (name relative to the asset's folder, link). The first is the main file.
pub fn polyhaven_plan(ty: &str, files: &Value) -> Vec<(String, String)> {
    let url = |v: &Value| v["url"].as_str().map(str::to_owned);
    let mut out = Vec::new();
    match ty {
        "models" => {
            let g = &files["gltf"];
            let res = ["1k", "2k", "4k"].into_iter().find(|r| g[*r]["gltf"]["url"].is_string());
            if let Some(r) = res {
                let main = &g[r]["gltf"];
                if let Some(u) = url(main) {
                    out.push((u.rsplit('/').next().unwrap_or("model.gltf").to_owned(), u));
                }
                for (name, inc) in main["include"].as_object().into_iter().flatten() {
                    if let Some(u) = url(inc) {
                        out.push((name.clone(), u));
                    }
                }
            }
        }
        "textures" => {
            for (map, tag) in [("Diffuse", "color"), ("nor_gl", "normal"), ("arm", "arm")] {
                let m = &files[map];
                let res = ["1k", "2k", "4k"].into_iter().find(|r| m[*r]["jpg"]["url"].is_string());
                if let Some(u) = res.and_then(|r| url(&m[r]["jpg"])) {
                    out.push((format!("{tag}.jpg"), u));
                }
            }
        }
        _ => {
            let h = &files["hdri"];
            if let Some(u) = ["1k", "2k", "4k"].into_iter().find_map(|r| url(&h[r]["hdr"])) {
                out.push((u.rsplit('/').next().unwrap_or("sky.hdr").to_owned(), u));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- searching

/// A getter for a link's body as text; tests give one that never touches the network.
pub type Getter<'a> = &'a (dyn Fn(&str) -> Result<String, String> + Sync);

fn net_get(url: &str) -> Result<String, String> {
    let reply = studio_net::get(url, &[("User-Agent", UA), ("Accept-Language", "en"), ("Accept", "text/html,application/json;q=0.9,*/*;q=0.5")], Duration::from_secs(25))?;
    if reply.status >= 400 {
        return Err(format!("answered {}", reply.status));
    }
    let b = reply.bytes(PAGE_BYTES).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// Kinds a call may ask for.
fn norm_kind(k: &str) -> Result<&'static str, String> {
    Ok(match k.trim().to_lowercase().as_str() {
        "" | "image" | "images" | "picture" | "pictures" | "photo" | "photos" => "image",
        "video" | "videos" | "footage" | "clip" | "b-roll" | "broll" => "video",
        "audio" | "sound" | "sounds" | "music" | "sfx" => "audio",
        "3d" | "model" | "models" | "3d-model" | "mesh" => "3d",
        "texture" | "textures" => "texture",
        "hdri" | "hdris" | "sky" => "hdri",
        o => return Err(format!("search_media kind is image, video, audio, 3d, texture or hdri, not \"{o}\"")),
    })
}

type Job<'a> = (&'static str, Box<dyn FnOnce() -> Result<Vec<Hit>, String> + Send + 'a>);

/// Searches the web and the open collections for `query`; one list per
/// source, in the order they should be interleaved. Failures of a source are
/// reported by name and do not stop the others.
pub fn search(query: &str, kind: &'static str, per: usize, get: Getter) -> (Vec<Hit>, Vec<String>) {
    let q_owned = encode(query);
    let q = q_owned.as_str();
    let mut jobs: Vec<Job> = Vec::new();
    match kind {
        "image" => {
            jobs.push(("web", Box::new(move || get(&format!("https://www.bing.com/images/async?q={q}&first=1&count={}&mmasync=1", per * 2)).map(|b| parse_web_images(&b)))));
            jobs.push(("openverse", Box::new(move || get(&format!("https://api.openverse.org/v1/images/?q={q}&page_size={per}")).map(|b| parse_openverse(&b, "image")))));
            jobs.push(("commons", Box::new(move || get(&commons_url(&q, "bitmap", per)).map(|b| parse_commons(&b, "image", &["image/"])))));
        }
        "video" => {
            jobs.push(("web", Box::new(move || get(&format!("https://www.bing.com/videos/search?q={q}")).map(|b| parse_web_videos(&b)))));
            jobs.push(("commons", Box::new(move || get(&commons_url(&q, "video", per)).map(|b| parse_commons(&b, "video", &["video/"])))));
        }
        "audio" => {
            jobs.push(("openverse", Box::new(move || get(&format!("https://api.openverse.org/v1/audio/?q={q}&page_size={per}")).map(|b| parse_openverse(&b, "audio")))));
            jobs.push(("commons", Box::new(move || get(&commons_url(&q, "audio", per)).map(|b| parse_commons(&b, "audio", &["audio/", "application/ogg"])))));
        }
        _ => {
            let (t, k): (&str, &'static str) = match kind {
                "texture" => ("textures", "texture"),
                "hdri" => ("hdris", "hdri"),
                _ => ("models", "3d"),
            };
            let query = query.to_owned();
            jobs.push(("polyhaven", Box::new(move || get(&format!("https://api.polyhaven.com/assets?t={t}")).map(|b| parse_polyhaven(&b, k, &query)))));
        }
    }
    let results: Vec<(&str, Result<Vec<Hit>, String>)> = std::thread::scope(|sc| {
        let handles: Vec<_> = jobs.into_iter().map(|(name, f)| (name, sc.spawn(f))).collect();
        handles.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("crashed".into())))).collect()
    });
    let mut lists = Vec::new();
    let mut notes = Vec::new();
    for (name, r) in results {
        match r {
            Ok(v) if v.is_empty() => notes.push(format!("{name}: nothing")),
            Ok(v) => lists.push(v),
            Err(e) => notes.push(format!("{name}: {e}")),
        }
    }
    (interleave(lists, per.max(1)), notes)
}

fn commons_url(q: &str, filetype: &str, n: usize) -> String {
    format!("https://commons.wikimedia.org/w/api.php?action=query&generator=search&gsrsearch={q}+filetype%3A{filetype}&gsrnamespace=6&gsrlimit={n}&prop=imageinfo&iiprop=url%7Csize%7Cmime%7Cextmetadata&iiurlwidth=320&format=json")
}

/// One from each list in turn, so no source crowds out the others; the same
/// file link once.
fn interleave(lists: Vec<Vec<Hit>>, max: usize) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while out.len() < max && lists.iter().any(|l| i < l.len()) {
        for l in &lists {
            if let Some(h) = l.get(i) {
                if out.len() < max && seen.insert(h.url.clone()) {
                    out.push(h.clone());
                }
            }
        }
        i += 1;
    }
    out
}

/// A result as the AI reads it: short.
fn compact(n: usize, h: &Hit) -> Value {
    let mut v = json!({"n": n, "title": h.title.chars().take(70).collect::<String>(), "kind": h.kind, "from": h.source});
    if h.width > 0 {
        v["size"] = json!([h.width, h.height]);
    }
    if h.seconds > 0.0 {
        v["seconds"] = json!((h.seconds * 10.0).round() / 10.0);
    }
    if !h.author.is_empty() {
        v["by"] = json!(h.author.chars().take(40).collect::<String>());
    }
    if !h.page.is_empty() {
        v["page"] = json!(h.page.chars().take(110).collect::<String>());
    }
    v
}

// --------------------------------------------------------------- thumbnails

/// A contact sheet of the results' thumbnails, numbered, as one JPEG.
fn sheet(hits: &[Hit], get_bytes: &(dyn Fn(&str) -> Option<Vec<u8>> + Sync)) -> Option<Vec<u8>> {
    use studio_media::rgba::RgbaImage;
    const W: u32 = 220;
    const H: u32 = 150;
    let shown = &hits[..hits.len().min(SHEET_MAX)];
    let pics: Vec<Option<RgbaImage>> = std::thread::scope(|sc| {
        let hs: Vec<_> = shown.iter().enumerate().map(|(i, h)| sc.spawn(move || {
            let bytes = get_bytes(&h.thumb)?;
            let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else if bytes.starts_with(&[0xFF, 0xD8]) { "jpg" } else { return None };
            let path = std::env::temp_dir().join(format!("studio-search-{}-{i}.{ext}", std::process::id()));
            std::fs::write(&path, &bytes).ok()?;
            let img = if ext == "png" { studio_media::png::read(&path) } else { studio_media::jpeg::read(&path) };
            let _ = std::fs::remove_file(&path);
            img.ok()
        })).collect();
        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    if pics.iter().all(Option::is_none) {
        return None;
    }
    let tiles: Vec<RgbaImage> = pics.iter().enumerate().map(|(i, p)| {
        let mut tile = RgbaImage { width: W, height: H, pixels: vec![0; (W * H * 4) as usize] };
        studio_content::image::fill_rect(&mut tile, 0, 0, W, H, [40, 40, 44, 255]);
        if let Some(img) = p {
            let fit = studio_content::image::resize_to_fit(img, W.max(H));
            let fit = if fit.width > W || fit.height > H { studio_content::image::resize(&fit, fit.width.min(W), fit.height.min(H)) } else { fit };
            studio_content::image::blit(&mut tile, &fit, ((W - fit.width) / 2) as i64, ((H - fit.height) / 2) as i64);
        }
        let label = (i + 1).to_string();
        studio_content::image::fill_rect(&mut tile, 0, 0, studio_content::image::label_width(&label, 2) + 8, 20, [0, 0, 0, 255]);
        studio_content::image::draw_label(&mut tile, 4, 3, &label, 2, [255, 255, 255, 255]);
        tile
    }).collect();
    let cols = if tiles.len() <= 2 { tiles.len() } else if tiles.len() <= 4 { 2 } else { 4 };
    Some(studio_content::jpeg::encode(&super::render::tiled(&tiles, cols.max(1), W), 70))
}

fn net_bytes(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let r = studio_net::get(url, &[("User-Agent", UA)], Duration::from_secs(10)).ok()?;
    if r.status >= 400 {
        return None;
    }
    r.bytes(600_000).ok()
}

// -------------------------------------------------------------------- tool

pub fn tools() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    vec![json!({
        "name": "search_media",
        "description": "Find real pictures, video, sound and 3D before making any: library first, then the web, plus Openverse, Wikimedia Commons and Poly Haven (models, textures, HDRIs). For animals, places, objects, textures, b-roll; generate only if nothing fits. Fields: query, kind (image, video, audio, 3d, texture, hdri), max (8). Returns numbered results and a numbered thumbnail sheet to look at. fetch: N brings one in like download (name, to_timeline) and keeps its source page. More: guide {topic: \"tool:search_media\"}.",
        "inputSchema": {"type": "object", "properties": {
            "query": s("what to look for"),
            "kind": s("image (default), video, audio, 3d..."),
            "max": {"type": "integer", "description": "at most 20"},
            "thumbs": {"type": "boolean", "description": "the thumbnail sheet (default true)"},
            "fetch": {"description": "result number from the last search"},
            "name": s("file name"),
            "to_timeline": {"type": "boolean", "description": "also place it"},
        }, "required": []}
    })]
}

impl Session {
    pub(super) fn search_media_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        if let Some(f) = a.get("fetch").filter(|f| !f.is_null()) {
            return self.fetch_found(f, a);
        }
        let query = a.get("query").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty()).ok_or("search_media needs \"query\": what to look for (and kind: image, video, audio, 3d, texture or hdri)")?;
        let kind = norm_kind(a.get("kind").and_then(Value::as_str).unwrap_or(""))?;
        let max = a.get("max").and_then(Value::as_u64).map_or(DEFAULT_MAX, |n| (n as usize).clamp(1, MOST));
        // The library first.
        let mut library = Vec::new();
        if let Ok(v) = self.assets_tool(&json!({"action": "find", "query": query, "limit": 6})) {
            for x in v["assets"].as_array().into_iter().flatten().take(6) {
                library.push(json!({"asset": x["name"], "kind": x["kind"], "path": x["path"]}));
            }
            for x in v["content_library"].as_array().into_iter().flatten().take(6) {
                library.push(x.clone());
            }
        }
        let (hits, notes) = search(query, kind, max, &net_get);
        let mut out = json!({"query": query, "kind": kind});
        if !library.is_empty() {
            out["library"] = json!(library);
            out["library_note"] = json!("these are already here: add them from the library before using a result below");
        }
        out["results"] = Value::Array(hits.iter().enumerate().map(|(i, h)| compact(i + 1, h)).collect());
        if !notes.is_empty() {
            out["unavailable"] = json!(notes);
        }
        if hits.is_empty() && library.is_empty() {
            out["next"] = json!("nothing found: try other words or another kind; generate only now if the picture must exist");
        } else if !hits.is_empty() {
            out["next"] = json!("look at the sheet, then search_media {fetch: N} for the best; say which were found and which were made");
        }
        let want_sheet = a.get("thumbs").and_then(Value::as_bool).unwrap_or(true) && matches!(kind, "image" | "video" | "texture" | "3d" | "hdri" | "audio");
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = hits.clone();
        let mut parts = Vec::new();
        if want_sheet && !hits.is_empty() {
            if let Some(j) = sheet(&hits, &net_bytes) {
                parts.push(Content::Jpeg(j));
            }
        }
        parts.push(Content::Text(out.to_string()));
        Ok(parts)
    }

    fn fetch_found(&mut self, f: &Value, a: &Value) -> Result<Vec<Content>, String> {
        let last = LAST.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let hit = match f {
            Value::Number(n) => {
                let n = n.as_u64().unwrap_or(0) as usize;
                last.get(n.wrapping_sub(1)).cloned().ok_or_else(|| format!("no result {n} in the last search ({} results); search again", last.len()))?
            }
            Value::String(u) if u.trim().parse::<usize>().is_ok() => {
                let n: usize = u.trim().parse().unwrap_or(0);
                last.get(n.wrapping_sub(1)).cloned().ok_or_else(|| format!("no result {n} in the last search ({} results); search again", last.len()))?
            }
            Value::String(u) => last.iter().find(|h| h.url == *u || h.page == *u).cloned().unwrap_or_else(|| Hit { source: "web", kind: "image", url: u.clone(), ..Default::default() }),
            _ => return Err("fetch is a result number or a link".into()),
        };
        let home = self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
        if let Some(rest) = hit.url.strip_prefix("polyhaven:") {
            return self.fetch_polyhaven(rest, &hit, a, home.as_deref());
        }
        let mut args = json!({"url": hit.url});
        for k in ["name", "to_timeline"] {
            if let Some(v) = a.get(k) {
                args[k] = v.clone();
            }
        }
        if args.get("name").is_none() && !hit.title.is_empty() {
            args["name"] = json!(hit.title.chars().take(60).collect::<String>());
        }
        let mut parts = self.call("download", &args)?;
        let mut file = None;
        for p in parts.iter_mut() {
            if let Content::Text(t) = p {
                if let Ok(mut v) = serde_json::from_str::<Value>(t) {
                    file = v["file"].as_str().map(PathBuf::from);
                    v["found_by"] = json!("search_media");
                    if !hit.page.is_empty() {
                        v["source_page"] = json!(hit.page);
                    }
                    *t = v.to_string();
                }
            }
        }
        if let Some(path) = file {
            record_source(&path, &hit);
        }
        Ok(parts)
    }

    fn fetch_polyhaven(&mut self, spec: &str, hit: &Hit, a: &Value, home: Option<&Path>) -> Result<Vec<Content>, String> {
        let (ty, id) = spec.split_once(':').ok_or("bad Poly Haven link")?;
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err("bad Poly Haven name".into());
        }
        let files: Value = serde_json::from_str(&net_get(&format!("https://api.polyhaven.com/files/{id}"))?).map_err(|e| format!("Poly Haven answered something unexpected: {e}"))?;
        let plan = polyhaven_plan(ty, &files);
        if plan.is_empty() {
            return Err(format!("Poly Haven has no downloadable files for {id}"));
        }
        let dir = super::download::folder_for(home).join("PolyHaven").join(id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let mut saved = Vec::new();
        for (rel, url) in &plan {
            // A name from the reply may not leave the folder.
            if rel.split('/').any(|p| p == ".." || p.is_empty()) || rel.starts_with('/') {
                continue;
            }
            let dest = dir.join(rel);
            if let Some(p) = dest.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let r = studio_net::get(url, &[("User-Agent", UA)], Duration::from_secs(60))?;
            if r.status >= 400 {
                return Err(format!("Poly Haven answered {} for {rel}", r.status));
            }
            let bytes = r.bytes(200 << 20).map_err(|e| e.to_string())?;
            std::fs::write(&dest, bytes).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
            saved.push(dest);
        }
        let main = saved.first().cloned().ok_or("nothing was saved")?;
        record_source(&main, hit);
        let mut v = json!({"file": main.display().to_string(), "folder": dir.display().to_string(), "files": saved.len(), "from": "polyhaven", "source_page": hit.page});
        match ty {
            "models" => v["use"] = json!("a glTF model with its textures in that folder: bring it in with the 3D tools (model / asset with file), then look at it"),
            "hdris" => v["use"] = json!("an HDRI sky: use it as the world's sky / lighting"),
            _ => {
                let parts = self.call("import", &json!({"paths": [main.display().to_string()], "to_timeline": a.get("to_timeline").and_then(Value::as_bool).unwrap_or(false)}))?;
                if let Some(Content::Text(t)) = parts.into_iter().next() {
                    v["imported"] = serde_json::from_str(&t).unwrap_or(Value::Null);
                }
                v["maps"] = json!(saved.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>());
            }
        }
        Ok(vec![Content::Text(v.to_string())])
    }
}

/// Keeps the page a file came from with the file: its source and link in
/// the library, nothing more. Failures are silent: the file is already in.
pub fn record_source(path: &Path, hit: &Hit) {
    let Ok(store) = crate::assets::global() else { return };
    let Ok(id) = store.add_file(path, Some("downloaded")) else { return };
    let page = if hit.page.is_empty() { hit.url.as_str() } else { hit.page.as_str() };
    if page.starts_with("polyhaven:") {
        return;
    }
    let host = page.split("//").nth(1).and_then(|r| r.split('/').next()).unwrap_or("").trim_start_matches("www.");
    let _ = crate::rights::set(store, id, &json!({"source": host, "url": page}));
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENVERSE_IMAGES: &str = include_str!("../../tests/fixtures/search_media/openverse_images.json");
    const OPENVERSE_AUDIO: &str = include_str!("../../tests/fixtures/search_media/openverse_audio.json");
    const COMMONS: &str = include_str!("../../tests/fixtures/search_media/commons.json");
    const POLY_MODELS: &str = include_str!("../../tests/fixtures/search_media/polyhaven_models.json");
    const POLY_FILES: &str = include_str!("../../tests/fixtures/search_media/polyhaven_files.json");
    const BING_IMAGES: &str = include_str!("../../tests/fixtures/search_media/bing_images.html");
    const BING_VIDEOS: &str = include_str!("../../tests/fixtures/search_media/bing_videos.html");

    #[test]
    fn openverse_images_and_audio_are_read() {
        let v = parse_openverse(OPENVERSE_IMAGES, "image");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].title, "Rooster");
        assert!(v[0].url.starts_with("https://") && v[0].thumb.contains("/thumb/") && v[0].page.contains("flickr.com"));
        assert_eq!((v[0].width, v[0].height), (1024, 685));
        assert_eq!(v[0].author, "bagsgroove");
        let a = parse_openverse(OPENVERSE_AUDIO, "audio");
        assert!(a[0].url.ends_with(".mp3") && (a[0].seconds - 2.981).abs() < 1e-6, "{:?}", a[0]);
    }

    #[test]
    fn commons_keeps_only_the_wanted_kind() {
        let v = parse_commons(COMMONS, "image", &["image/"]);
        assert!(!v.is_empty());
        assert!(v[0].url.starts_with("https://upload.wikimedia.org/") && v[0].page.starts_with("https://commons.wikimedia.org/wiki/File:"));
        assert!(!v[0].title.starts_with("File:") && !v[0].thumb.is_empty() && v[0].width > 0);
        assert!(parse_commons(COMMONS, "video", &["video/"]).is_empty());
    }

    #[test]
    fn poly_haven_is_filtered_by_the_words() {
        let v = parse_polyhaven(POLY_MODELS, "3d", "victorian chair");
        assert_eq!(v[0].url, "polyhaven:models:ArmChair_01");
        assert_eq!(v[0].page, "https://polyhaven.com/a/ArmChair_01");
        assert!(v[0].thumb.contains("ArmChair_01") && !v[0].author.is_empty());
        assert!(parse_polyhaven(POLY_MODELS, "3d", "zzzz").is_empty());
    }

    #[test]
    fn poly_haven_files_are_planned() {
        let files: Value = serde_json::from_str(POLY_FILES).unwrap();
        let m = polyhaven_plan("models", &files);
        assert!(m[0].0.ends_with(".gltf") && m.len() > 2, "{m:?}");
        assert!(m.iter().any(|(n, _)| n.starts_with("textures/")));
        let t = polyhaven_plan("textures", &files);
        assert_eq!(t[0].0, "color.jpg");
    }

    #[test]
    fn web_pages_are_read() {
        let i = parse_web_images(BING_IMAGES);
        assert_eq!(i.len(), 3);
        assert_eq!(i[0].url, "https://wallpaperaccess.com/full/1991713.jpg");
        assert!(i[0].thumb.contains("&pid=15.1") && i[0].page.starts_with("https://") && !i[0].title.is_empty());
        let v = parse_web_videos(BING_VIDEOS);
        assert!(!v.is_empty() && v[0].url.contains("youtube.com/watch") && (v[0].seconds - 25.0).abs() < 1e-6, "{:?}", v.first());
        assert!(v[0].thumb.contains("%2B"), "{}", v[0].thumb);
    }

    #[test]
    fn entities_and_clock_work() {
        assert_eq!(unescape("a &amp; b &quot;c&quot; &#39;d&#x41;"), "a & b \"c\" 'dA");
        assert_eq!(clock("01:02:03"), 3723.0);
        assert_eq!(plain("<a href=\"x\">Jane &amp; Co</a>"), "Jane & Co");
    }

    /// A getter that answers from the recorded replies, by address.
    fn recorded(url: &str) -> Result<String, String> {
        Ok(if url.contains("api.openverse.org/v1/images") {
            OPENVERSE_IMAGES
        } else if url.contains("openverse.org/v1/audio") {
            OPENVERSE_AUDIO
        } else if url.contains("commons.wikimedia.org") {
            COMMONS
        } else if url.contains("bing.com/images") {
            BING_IMAGES
        } else if url.contains("bing.com/videos") {
            BING_VIDEOS
        } else if url.contains("polyhaven.com/assets") {
            POLY_MODELS
        } else {
            return Err("answered 503".into());
        }
        .to_owned())
    }

    /// Nothing is held back for its licence: every source's results are
    /// there, from the web and the open collections together.
    #[test]
    fn every_source_answers_and_none_is_filtered() {
        let (hits, notes) = search("rooster", "image", 20, &recorded);
        assert!(notes.is_empty(), "{notes:?}");
        for src in ["web", "openverse", "commons"] {
            assert!(hits.iter().any(|h| h.source == src), "{src} missing from {hits:?}");
        }
        // The web comes first, then the collections in turn.
        assert_eq!(hits[0].source, "web");
        let (v, _) = search("rooster crowing", "video", 5, &recorded);
        assert!(v.iter().any(|h| h.source == "web"));
        let (m, _) = search("chair", "3d", 5, &recorded);
        assert_eq!(m[0].kind, "3d");
    }

    #[test]
    fn a_failing_source_is_named_and_the_rest_answer() {
        let (hits, notes) = search("rooster", "image", 5, &|u: &str| if u.contains("bing.com") { Err("answered 429".into()) } else { recorded(u) });
        assert!(!hits.is_empty() && hits.iter().all(|h| h.source != "web"));
        assert_eq!(notes, vec!["web: answered 429".to_owned()]);
    }

    #[test]
    fn the_same_file_is_listed_once_and_max_holds() {
        let a = parse_openverse(OPENVERSE_IMAGES, "image");
        let out = interleave(vec![a.clone(), a], 5);
        assert_eq!(out.len(), 3);
        assert_eq!(interleave(vec![parse_openverse(OPENVERSE_IMAGES, "image")], 2).len(), 2);
    }

    #[test]
    fn kinds_are_understood() {
        assert_eq!(norm_kind("Photos").unwrap(), "image");
        assert_eq!(norm_kind("sfx").unwrap(), "audio");
        assert_eq!(norm_kind("3D").unwrap(), "3d");
        assert!(norm_kind("poem").is_err());
    }

    /// The page a file came from is kept with it, and nothing about its
    /// licence is recorded or checked.
    #[test]
    fn the_source_page_is_kept_with_the_file() {
        let dir = std::env::temp_dir().join(format!("studio-search-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rooster.jpg");
        std::fs::write(&file, b"x").unwrap();
        let hit = Hit { source: "web", kind: "image", url: "https://img.example.org/r.jpg".into(), page: "https://www.example.org/roosters".into(), ..Default::default() };
        record_source(&file, &hit);
        if let Ok(store) = crate::assets::global() {
            if let Ok(a) = store.resolve(&json!(file.display().to_string())) {
                let r = crate::rights::get(store, a.id).unwrap().unwrap();
                assert_eq!(r.url, "https://www.example.org/roosters");
                assert_eq!(r.source, "example.org");
                assert_eq!(r.licence, "unknown", "no licence is decided here");
                let _ = store.forget(a.id);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A search reply stays small however many results come back.
    #[test]
    fn twenty_results_stay_within_the_budget() {
        let (mut hits, _) = search("rooster", "image", 20, &recorded);
        let mut more = hits.clone();
        hits.append(&mut more);
        hits.truncate(MOST);
        while hits.len() < MOST {
            let h = hits[0].clone();
            hits.push(h);
        }
        let list: Vec<Value> = hits.iter().enumerate().map(|(i, h)| compact(i + 1, h)).collect();
        let n = json!({"query": "rooster", "kind": "image", "results": list}).to_string().len();
        assert!(n <= 7_000, "20 results are {n} characters (~{} tokens)", n / 4);
    }

    /// The decision rule is in the core prompt and the five skills.
    #[test]
    fn the_search_first_rule_is_in_the_prompt_and_the_skills() {
        assert!(super::super::brain::FINISH.contains("Search before you generate") && super::super::brain::FINISH.contains("`search_media`"));
        for (name, text) in [
            ("generated-media", include_str!("skills/generated-media.md")),
            ("picture-batches", include_str!("skills/picture-batches.md")),
            ("3d-assets-scenes", include_str!("skills/3d-assets-scenes.md")),
            ("footage-edit", include_str!("skills/footage-edit.md")),
            ("story-film", include_str!("skills/story-film.md")),
        ] {
            assert!(text.contains("**Search before") && text.contains("search_media") && text.contains("found") && text.contains(" made"), "{name}");
            assert!(text.lines().find(|l| l.starts_with("tools:")).is_some_and(|l| l.contains("search_media")), "{name}: tools");
        }
        assert!(include_str!("skills/3d-assets-scenes.md").contains("Poly Haven"));
    }

    /// Real queries against the live sources: `cargo test live_queries -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_queries() {
        for (q, k) in [("rooster", "image"), ("rooster crowing", "video"), ("rooster", "audio"), ("chair", "3d"), ("rooster", "texture")] {
            let (hits, notes) = search(q, k, 4, &net_get);
            eprintln!("== {k} \"{q}\": {} results, notes {notes:?}", hits.len());
            for (i, h) in hits.iter().enumerate() {
                eprintln!("  {}", compact(i + 1, h));
            }
            if !hits.is_empty() && k != "audio" {
                let sh = sheet(&hits, &net_bytes);
                eprintln!("  sheet: {} bytes", sh.map_or(0, |j| j.len()));
            }
        }
    }
}
