// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Media from the internet, for the `download` tool (and the Media panel's
//! "Download from link").
//!
//! A link straight to a video, audio or picture file is fetched with the
//! Studio's own HTTPS client (OS certificates, redirects, a size and a
//! time limit). A web page is read by `studio-webmedia`: the media it
//! openly exposes (video and audio elements, Open Graph and player tags,
//! VideoObject data, alternate links, oEmbed, and HLS or DASH playlists in
//! the page or its scripts) is found and downloaded in Rust, a playlist
//! becoming one MP4. Protected media (DRM), media a site hides behind a
//! signature or a sign-in, and pages that expose nothing are refused with
//! a plain message.
//! Whatever arrives in a format Studio cannot decode (AV1, VP9…) is
//! converted in Rust (`crate::convert`). What arrives is checked to be
//! media by the import that follows; anything else is deleted.
//!
//! Files land in the project's `Downloads` folder (or Studio's own when
//! the project is not saved yet), named after the page or file.

use std::path::{Path, PathBuf};
use studio_model as m;

/// Longest a download may take, seconds.
const MAX_SECS: &str = "1800";

const MEDIA_EXT: &[&str] = &["mp4", "mov", "m4v", "webm", "mkv", "avi", "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg", "opus", "jpg", "jpeg", "png", "gif", "webp", "heic", "tif", "tiff", "bmp"];

/// Other files a link may name outright: 3D models and what they need,
/// documents, data, fonts, subtitles, colour tables, archives.
const FILE_EXT: &[&str] = &[
    "glb", "gltf", "obj", "mtl", "fbx", "stl", "ply", "usdz", "usd", "usda", "usdc", "dae", "3ds", "blend", "abc", "3mf", "off", "smesh", "bin", "hdr", "exr", "avif", "svg", "lottie", "pdf", "txt", "md", "csv", "json", "srt", "vtt", "ass", "cube", "3dl", "ttf", "otf", "woff", "woff2", "zip", "gz", "tar", "7z", "bvh",
];

/// Where downloads go for a project in `home` (or none yet).
pub fn folder_for(home: Option<&Path>) -> PathBuf {
    folder_for_kind(home, m::file::MediaKind::Other)
}

/// Where a download of `kind` goes for a project in `home` (or none yet): a
/// package sorts it with the rest of its media.
pub fn folder_for_kind(home: Option<&Path>, kind: m::file::MediaKind) -> PathBuf {
    match home {
        Some(h) => m::file::ProjectFolder::new(h).download_dir(kind),
        None => studio_paths::studio().join("Downloads"),
    }
}

/// Where a download from `url` goes for a project in `home`: a package sorts
/// it by the file type the link names, earlier folders use `Downloads`.
pub fn folder_for_url(home: Option<&Path>, url: &str) -> PathBuf {
    let kind = url_ext(url).map_or(m::file::MediaKind::Other, |e| m::file::MediaKind::for_extension(&e));
    folder_for_kind(home, kind)
}

/// The path part of a link (after the host, before `?` and `#`).
fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    rest.find('/').map_or("", |i| &rest[i..])
}

/// The extension a link's file name carries (lower case), if it looks like
/// one: letters and digits, at least one letter.
fn url_ext(url: &str) -> Option<String> {
    let last = url_path(url).rsplit('/').next().unwrap_or("");
    let (_, ext) = last.rsplit_once('.')?;
    (ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) && ext.chars().any(|c| c.is_ascii_alphabetic())).then(|| ext.to_ascii_lowercase())
}

/// Whether `url` names a file itself (by its path's extension) rather than
/// a page: media, a 3D model, a document, an archive.
pub fn is_direct(url: &str) -> bool {
    url_ext(url).is_some_and(|e| MEDIA_EXT.contains(&e.as_str()) || FILE_EXT.contains(&e.as_str()))
}

/// A link to a page that shows a file, turned into the link to the file
/// itself: a GitHub or Hugging Face "blob" page, a Dropbox share.
pub fn normalise_url(url: &str) -> String {
    let u = url.trim();
    let (host, path_and_query) = match u.split_once("://").and_then(|(scheme, r)| r.split_once('/').map(|(h, p)| (scheme, h, p))) {
        Some((_, h, p)) => (h.to_ascii_lowercase(), p),
        None => return u.to_owned(),
    };
    let (path, query) = path_and_query.split_once('?').map_or((path_and_query, ""), |(p, q)| (p, q));
    let segs: Vec<&str> = path.split('/').collect();
    match host.trim_start_matches("www.").as_ref() {
        // github.com/o/r/blob/ref/path -> github.com/o/r/raw/ref/path (which
        // redirects to the raw file, and to Git LFS storage when it is there).
        "github.com" if segs.len() > 4 && segs[2] == "blob" => format!("https://github.com/{}/{}/raw/{}", segs[0], segs[1], segs[3..].join("/")),
        "huggingface.co" if segs.iter().position(|s| *s == "blob").is_some_and(|i| i >= 2) => {
            let i = segs.iter().position(|s| *s == "blob").unwrap_or(0);
            let mut v = segs.clone();
            v[i] = "resolve";
            format!("https://huggingface.co/{}", v.join("/"))
        }
        "dropbox.com" if query.contains("dl=0") => format!("https://www.dropbox.com/{path}?{}", query.replace("dl=0", "dl=1")),
        _ => u.to_owned(),
    }
}

/// Whether `s` is a web link.
pub fn is_url(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("https://") || t.starts_with("http://")
}

/// The file a person (or the AI) means by `s`: an absolute path, a
/// `file://` link, `~/…`, or a name or relative path, looked for in the
/// project folder, its Downloads, Models and media folders and the working
/// folder; then by file name anywhere in Downloads. When nothing matches,
/// the error says where it looked and what Downloads holds.
pub fn resolve_local(home: Option<&Path>, s: &str) -> Result<PathBuf, String> {
    let raw = s.trim().trim_matches(|c| c == '"' || c == '\'');
    let text = match raw.strip_prefix("file://") {
        Some(p) => percent_decode(p.trim_start_matches("localhost")).replace('+', " ").replace("%2B", "+"),
        None => raw.to_owned(),
    };
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|| PathBuf::from(&text)),
        None => PathBuf::from(&text),
    };
    let found = |p: &Path| -> Result<Option<PathBuf>, String> {
        if p.is_dir() {
            return Err(format!("{} is a folder, not a file", p.display()));
        }
        Ok(p.is_file().then(|| p.to_path_buf()))
    };
    let mut looked: Vec<PathBuf> = vec![expanded.clone()];
    if let Some(p) = found(&expanded)? {
        return Ok(p);
    }
    if expanded.is_relative() {
        let downloads = folder_for(home);
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(h) = home {
            roots.extend(m::file::ProjectFolder::new(h).lookup_roots());
        }
        roots.push(downloads.clone());
        for r in roots {
            let c = r.join(&expanded);
            if let Some(p) = found(&c)? {
                return Ok(p);
            }
            looked.push(c);
        }
        // By name, anywhere in Downloads (two levels down).
        if let Some(name) = expanded.file_name().map(|n| n.to_string_lossy().to_lowercase()) {
            let mut hit = None;
            // A package sorts downloads by type under Media; earlier folders kept them in Downloads.
            let search = home.map(m::file::ProjectFolder::new).filter(|pf| pf.layout() == m::file::Layout::Package).map_or_else(|| downloads.clone(), |pf| pf.media_root());
            let mut stack = vec![(search, 0)];
            while let Some((d, depth)) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() && depth < 2 {
                        stack.push((p, depth + 1));
                    } else if p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == name) {
                        hit = Some(p);
                    }
                }
            }
            if let Some(p) = hit {
                return Ok(p);
            }
        }
    }
    let dl = folder_for(home);
    let mut dirs = vec![dl.clone()];
    let models = folder_for_kind(home, m::file::MediaKind::Models3d);
    if models != dl {
        dirs.push(models);
    }
    let mut held: Vec<String> = dirs.iter().flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten()).filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    held.sort();
    held.truncate(20);
    let first = looked.first().map(|p| p.display().to_string()).unwrap_or_default();
    let place = if looked.len() > 1 { format!("{first}, and in the project folder, Downloads, Models and media") } else { first };
    Err(format!(
        "no file at \"{raw}\" (looked at {place}).{}",
        if held.is_empty() { format!(" {} is empty: download the file first (download {{url}}) or give its full path.", dl.display()) } else { format!(" {} holds: {}.", dl.display(), held.join(", ")) }
    ))
}

/// Checks a link before anything is fetched.
pub fn check_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        return Err("download takes an http(s) link".into());
    }
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("that link has spaces or control characters in it".into());
    }
    Ok(())
}

/// A file name from a link or title: safe, short, with `ext`.
fn file_name(stem: &str, ext: &str) -> String {
    let clean: String = stem.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '-' }).collect();
    let clean = clean.trim_matches(|c: char| c == '.' || c == '-' || c.is_whitespace());
    let clean: String = clean.chars().take(80).collect();
    // A name that already ends in it ("README.md") does not get it twice.
    let dot_ext = format!(".{}", ext.to_ascii_lowercase());
    let clean = if clean.to_ascii_lowercase().ends_with(&dot_ext) { clean[..clean.len() - dot_ext.len()].to_owned() } else { clean };
    format!("{}.{ext}", if clean.is_empty() { "download" } else { &clean })
}

/// A path in `dir` for `name` that does not exist yet.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (1..)
        .map(|n| if n == 1 { dir.join(name) } else { dir.join(format!("{stem} {n}.{ext}")) })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// What the person (or the AI) asked for when a page or a playlist offers
/// several versions.
#[derive(Default, Clone, Debug)]
pub struct Pick {
    /// `best` (the default), `lowest`, `audio`, or a height such as `720`.
    pub quality: Option<String>,
    /// Preferred sound language (`en`, `de`...) when a stream has several.
    pub language: Option<String>,
}

/// A finished download.
#[derive(Debug)]
pub struct Fetched {
    pub file: PathBuf,
    /// How it was found: `file`, `hls` or `dash` (a page's playlist).
    pub via: &'static str,
    /// The version taken ("1280x720, 2.4 Mbit/s"), for playlists.
    pub chosen: Option<String>,
    /// The other versions on offer.
    pub available: Vec<String>,
    /// Things worth telling the person (a live stream cut off, a part left out).
    pub notes: Vec<String>,
}

impl Fetched {
    fn plain(file: PathBuf) -> Fetched {
        Fetched { file, via: "file", chosen: None, available: Vec::new(), notes: Vec::new() }
    }
}

/// Fetches `url` into `dir`; `name` overrides the file name. Returns the
/// file.
#[cfg(test)]
pub fn fetch(url: &str, dir: &Path, name: Option<&str>) -> Result<PathBuf, String> {
    fetch_pick(url, dir, name, &Pick::default()).map(|f| f.file)
}

/// [`fetch`] with a choice of quality and language, and the details of what
/// was taken.
pub fn fetch_pick(url: &str, dir: &Path, name: Option<&str>, pick: &Pick) -> Result<Fetched, String> {
    check_url(url)?;
    let url = normalise_url(url);
    let url = url.as_str();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    // A model that comes with other files (a .gltf with its buffers, an .obj
    // with its .mtl and pictures) gets a folder of its own for them.
    let own;
    let dir = if matches!(url_ext(url).as_deref(), Some("gltf" | "obj")) {
        let last = percent_decode(url_path(url).rsplit('/').next().unwrap_or("model"));
        let stem = name.unwrap_or(&last);
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
        own = dir.join(file_name(stem, "x").trim_end_matches(".x"));
        std::fs::create_dir_all(&own).map_err(|e| format!("cannot make {}: {e}", own.display()))?;
        own.as_path()
    } else {
        dir
    };
    if is_direct(url) {
        let file = fetch_file(url, dir, name, None)?;
        fetch_companions(url, &file);
        return Ok(Fetched::plain(file));
    }
    // A link without a file extension may still be a file (image CDNs,
    // signed links, a model served as application/octet-stream): what the
    // server says it is decides, so a photo is not handed to the video-site
    // downloader.
    match served_type(url)? {
        Served::Media(ext) => return fetch_file(url, dir, name, Some(ext)).map(Fetched::plain),
        Served::Document(ct) => {
            if let Some(ext) = document_ext(&ct, url) {
                return fetch_file(url, dir, name, Some(&ext)).map(Fetched::plain);
            }
            // Text that is not a known document type: a page (or a playlist).
        }
        // Not text: a file whatever its name; the bytes say what it is.
        Served::Binary => return fetch_file(url, dir, name, None).map(Fetched::plain),
        Served::Page => {}
    }
    fetch_page(url, dir, name, pick)
}

/// What a link serves, from one small request.
enum Served {
    /// A media file (the extension for its type).
    Media(&'static str),
    /// Text or a document, with its Content-Type.
    Document(String),
    /// Something that is not text and not media: a model, an archive...
    Binary,
    /// A web page, or the server did not say.
    Page,
}

/// Asks what `url` serves (its Content-Type). A refusal or a missing file is
/// reported here, in words, instead of being mistaken for a page.
fn served_type(url: &str) -> Result<Served, String> {
    let reply = match studio_net::get(url, &[("Range", "bytes=0-0")], std::time::Duration::from_secs(20)) {
        Ok(r) => r,
        // Could not ask (offline, a server that drops the first request):
        // the page reader tries again with retries and says what happened.
        Err(_) => return Ok(Served::Page),
    };
    if reply.status >= 400 && reply.status != 416 {
        let looks_like_page = reply.content_type.as_deref().is_some_and(|c| c.to_ascii_lowercase().contains("html"));
        // A page that says 404 is the page reader's to explain; a file that
        // is gone or refused is plain.
        if !(looks_like_page && reply.status >= 500) {
            return Err(explain_failure(&format!("the server answered {} for {url}", reply.status)));
        }
    }
    let Some(ct) = reply.content_type else { return Ok(Served::Page) };
    let ct = ct.to_ascii_lowercase();
    let ct = ct.split(';').next().unwrap_or("").trim().to_owned();
    if let Some(ext) = served_media(&ct) {
        return Ok(Served::Media(ext));
    }
    if ct.contains("html") || ct.contains("mpegurl") || ct.contains("dash+xml") || ct.is_empty() {
        return Ok(Served::Page);
    }
    if document_ext(&ct, url).is_some() {
        return Ok(Served::Document(ct));
    }
    if ct.starts_with("text/") || ct.ends_with("+xml") || ct == "application/xml" || ct == "application/json" || ct.ends_with("+json") {
        return Ok(Served::Document(ct));
    }
    Ok(Served::Binary)
}

/// The extension of a text or PDF file served as `ct`: the link's own
/// when it has one (README.md), else one for the type.
fn document_ext(ct: &str, url: &str) -> Option<String> {
    let by_type = match ct {
        "text/plain" => "txt",
        "text/markdown" | "text/x-markdown" => "md",
        "text/csv" => "csv",
        "text/vtt" => "vtt",
        "application/pdf" => "pdf",
        "application/json" => "json",
        _ => return None,
    };
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let own = path.rsplit('/').next().and_then(|l| l.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()));
    Some(own.unwrap_or_else(|| by_type.to_owned()))
}

/// What a downloaded file is when it is not media: `pdf`, or `text`
/// (prose, Markdown, data, a page's source). A Lottie animation and an
/// SVG picture are text too, but media.
pub fn document_kind(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = vec![0u8; 64 << 10];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    let head = &head[..n];
    if head.starts_with(b"%PDF") {
        return Some("pdf");
    }
    if n == 0 || head.contains(&0) {
        return None;
    }
    // Text, though the read may end inside a character.
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&head[..e.valid_up_to()]).ok()?,
        Err(_) => return None,
    };
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with('{') && t.contains("\"layers\"") || t.starts_with("<svg") || t.starts_with("<?xml") && t.contains("<svg") {
        return None;
    }
    Some("text")
}

/// A document's file named for what it is: the downloader's stand-in
/// extension (`.unknown_video`, `.bin`) taken off, or `.txt` / `.pdf`
/// put on.
pub fn settle_document(path: &Path, kind: &str) -> PathBuf {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "unknown_video" | "bin" | "") {
        return path.to_path_buf();
    }
    let stem = path.with_extension("");
    let to = if stem.extension().is_some() { stem } else { stem.with_extension(if kind == "pdf" { "pdf" } else { "txt" }) };
    let to = free_path(to.parent().unwrap_or(Path::new(".")), &to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.txt".into()));
    match std::fs::rename(path, &to) {
        Ok(()) => to,
        Err(_) => path.to_path_buf(),
    }
}

/// The extension of the media file served as `ct`, or `None` for a web
/// page or anything else.
fn served_media(ct: &str) -> Option<&'static str> {
    Some(match ct {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/heic" | "image/heif" => "heic",
        "image/tiff" => "tiff",
        "image/bmp" => "bmp",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/ogg" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => return None,
    })
}

fn fetch_file(url: &str, dir: &Path, name: Option<&str>, served: Option<&str>) -> Result<PathBuf, String> {
    let last = url_path(url).rsplit('/').next().filter(|l| !l.is_empty()).unwrap_or("download");
    // The extension: what the server said it serves, else the link's own.
    let (stem, own_ext) = last.rsplit_once('.').map_or((last, None), |(s, e)| (s, Some(e)));
    let ext = served.or(own_ext.filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))).unwrap_or("bin");
    let stem = percent_decode(stem);
    let target = free_path(dir, &file_name(name.unwrap_or(&stem), &ext.to_ascii_lowercase()));
    let part = target.with_extension(format!("{}.part", ext.to_ascii_lowercase()));
    // A stalled transfer (nothing for 30 s) gives up, so it never holds the
    // agent's other calls waiting behind it.
    let opts = studio_net::Download { resume: false, max_bytes: 4 << 30, stall: std::time::Duration::from_secs(30), total: std::time::Duration::from_secs(MAX_SECS.parse().unwrap_or(1800)), retries: 2 };
    let get = |from: &str| -> Result<(), String> {
        studio_net::download(from, &part, opts, &mut |bytes| {
            super::jobs::report(0.0, &format!("downloaded {:.1} MB", bytes as f64 / 1e6));
            true
        })
        .map_err(|e| format!("the download failed: {}", explain_failure(&e.to_string())))
    };
    let mut res = get(url);
    // A file kept with Git LFS comes from raw.githubusercontent.com as a few
    // lines that point at it; its real bytes are on the media host.
    if res.is_ok() {
        if let Some(pointer) = lfs_pointer(&part) {
            res = match lfs_media_url(url) {
                Some(media) => get(&media),
                None => Err(format!("{url} is a Git LFS pointer ({pointer}), not the file. Use the file's github.com \"raw\" or \"Download\" link, which hands over the real file")),
            };
        }
    }
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if std::fs::metadata(&part).is_ok_and(|m| m.len() == 0) {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{url} sent an empty file"));
    }
    std::fs::rename(&part, &target).map_err(|e| format!("cannot save {}: {e}", target.display()))?;
    name_by_content(target, dir)
}

/// What a Git LFS pointer file says it stands for ("sha256:ab12…, 5813324 bytes"), if `path` is one.
fn lfs_pointer(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = [0u8; 300];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    let t = std::str::from_utf8(&head[..n]).ok()?;
    if !t.starts_with("version https://git-lfs.github.com/spec") {
        return None;
    }
    let field = |k: &str| t.lines().find_map(|l| l.strip_prefix(k)).unwrap_or("?").trim().to_owned();
    Some(format!("{}, {} bytes", field("oid "), field("size ")))
}

/// The LFS storage address for a raw.githubusercontent.com link.
fn lfs_media_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://raw.githubusercontent.com/")?;
    let rest = rest.split(['?', '#']).next()?;
    let (owner_repo, tail) = rest.match_indices('/').nth(1).map(|(i, _)| (&rest[..i], &rest[i + 1..]))?;
    let tail = tail.strip_prefix("refs/heads/").unwrap_or(tail);
    Some(format!("https://media.githubusercontent.com/media/{owner_repo}/{tail}"))
}

/// The files a downloaded model needs beside it: a .gltf's buffers and
/// pictures, an .obj's .mtl and its pictures. Fetched next to it by the
/// same relative names; what cannot be had is left out (the import says so).
fn fetch_companions(url: &str, file: &Path) {
    let Some(dir) = file.parent() else { return };
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut wanted: Vec<String> = Vec::new();
    let mut next_mtl = false;
    match ext.as_str() {
        "gltf" => {
            if let Ok(v) = std::fs::read(file).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string())) {
                for key in ["buffers", "images"] {
                    for item in v[key].as_array().into_iter().flatten() {
                        if let Some(u) = item["uri"].as_str().filter(|u| !u.starts_with("data:")) {
                            wanted.push(u.to_owned());
                        }
                    }
                }
            }
        }
        "obj" => {
            for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
                if let Some(n) = line.trim().strip_prefix("mtllib ") {
                    wanted.push(n.trim().to_owned());
                    next_mtl = true;
                }
            }
        }
        _ => return,
    }
    let base = url.split(['?', '#']).next().unwrap_or(url);
    let base = base.rsplit_once('/').map_or(base, |(b, _)| b);
    let query = url.split_once('?').map(|(_, q)| format!("?{}", q.split('#').next().unwrap_or(""))).unwrap_or_default();
    let mut i = 0;
    while i < wanted.len() {
        let rel = wanted[i].clone();
        i += 1;
        if rel.contains("://") || rel.starts_with('/') || rel.contains("..") {
            continue;
        }
        let dest = dir.join(percent_decode(&rel));
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let opts = studio_net::Download { resume: false, max_bytes: 1 << 30, stall: std::time::Duration::from_secs(30), total: std::time::Duration::from_secs(600), retries: 1 };
        let ok = studio_net::download(&format!("{base}/{rel}{query}"), &dest, opts, &mut |_| true).is_ok();
        if !ok {
            let _ = std::fs::remove_file(&dest);
        } else if next_mtl && rel.to_ascii_lowercase().ends_with(".mtl") {
            // Its pictures too.
            for line in std::fs::read_to_string(&dest).unwrap_or_default().lines() {
                if line.trim_start().to_ascii_lowercase().starts_with("map_") {
                    if let Some(n) = line.split_whitespace().last() {
                        wanted.push(n.to_owned());
                    }
                }
            }
        }
    }
}

/// A failed download in words that say what to do next, so the agent does
/// not try the same link, or a guessed one, again. Of the downloads agents
/// made, more than half failed on a link that did not exist.
pub fn explain_failure(e: &str) -> String {
    let status = e.strip_prefix("the server answered ").and_then(|r| r.split_whitespace().next()).and_then(|c| c.parse::<u16>().ok());
    let fix = match status {
        Some(404 | 410) => "that file is not there, and asking again will not change it. Do not guess links: use one the person gave you, or one you read on a page (`read` the page and take the media link it shows). For a sound, picture or music clip, look in the library first (`catalogue` with a query, then `add`).",
        Some(401 | 403) => "the site does not let a program fetch this file, and a retry will not either. Do not look for a way around it: tell the person, and ask for the file or another link; or use the library (`catalogue`).",
        Some(429) => "the site is limiting how fast it is asked. Do not retry in a loop: fetch the other files first, and try this one once more later.",
        Some(500..=599) => "the site's own server is failing. Try once more later; meanwhile carry on with other work or another source.",
        _ => return e.to_owned(),
    };
    format!("{e}. {fix}")
}

/// What a file is by its first bytes: media (`convert::sniff`) or a 3D model.
pub fn sniff(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    if let Some(k) = crate::convert::sniff(path) {
        return Some(k);
    }
    let mut head = vec![0u8; 1024];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    studio_mesh::foreign::sniff(&head[..n])
}

/// Whether `path` is named like a video, sound or picture.
pub fn is_media_name(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| MEDIA_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// Whether `path` is a 3D model Studio reads (by its name, once downloaded
/// files are named by their bytes).
pub fn is_model(path: &Path) -> bool {
    crate::mesh_store::is_model_file(path)
}

/// Named by what it is: a ".bin" or a wrongly named file gets the
/// extension its bytes say.
fn name_by_content(target: PathBuf, dir: &Path) -> Result<PathBuf, String> {
    if let Some(real) = sniff(&target) {
        let has = target.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let same = has == real || (real == "jpg" && has == "jpeg") || (real == "tiff" && has == "tif") || (real == "mkv" && has == "webm");
        // A glTF binary under a ".gltf" name is read as it is; only a name
        // that is wrong is changed.
        if !same {
            let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
            let renamed = free_path(dir, &format!("{stem}.{real}"));
            std::fs::rename(&target, &renamed).map_err(|e| format!("cannot rename {}: {e}", target.display()))?;
            return Ok(renamed);
        }
    }
    Ok(target)
}

/// A web page (or a playlist): the media it openly exposes.
fn fetch_page(url: &str, dir: &Path, name: Option<&str>, pick: &Pick) -> Result<Fetched, String> {
    let mut opts = studio_webmedia::Options::default();
    if let Some(q) = pick.quality.as_deref() {
        opts.quality = studio_webmedia::Quality::parse(q)?;
    }
    opts.language = pick.language.clone().filter(|l| !l.trim().is_empty());
    let saved = studio_webmedia::fetch(url, dir, name, &opts, &studio_webmedia::quiet).map_err(|e| e.to_string())?;
    // A plain file the page linked gets the same care as a direct link.
    let file = if saved.via == "file" { name_by_content(saved.file, dir)? } else { saved.file };
    Ok(Fetched {
        file,
        via: saved.via,
        chosen: (saved.via != "file").then_some(saved.chosen),
        available: saved.available,
        notes: saved.notes,
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_download_says_what_to_do_next() {
        let nf = explain_failure("the server answered 404 for https://example.com/a.wav");
        assert!(nf.starts_with("the server answered 404 for https://example.com/a.wav. ") && nf.contains("Do not guess links") && nf.contains("catalogue"), "{nf}");
        assert!(explain_failure("the server answered 403 for https://x.test/a.mp4").contains("tell the person"));
        assert!(explain_failure("the server answered 429 for https://x.test/a.mp4").contains("loop"));
        assert!(explain_failure("the server answered 503 for https://x.test/a.mp4").contains("server is failing"));
        // Anything else is left as it was.
        assert_eq!(explain_failure("the download stalled: nothing arrived for 30 s"), "the download stalled: nothing arrived for 30 s");
        assert_eq!(explain_failure("the server answered 418 for https://x.test/t"), "the server answered 418 for https://x.test/t");
    }

    use super::*;

    /// A README fetched through the page downloader is a document, named
    /// for what it is; a Lottie file and an SVG stay media; a PNG is media.
    #[test]
    fn documents_are_told_from_media() {
        let dir = std::env::temp_dir().join(format!("studio-dl-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let readme = dir.join("onetake-README.md.unknown_video");
        std::fs::write(&readme, "# oneTake\n\nA video editor — written in Swift.\n").unwrap();
        assert_eq!(super::document_kind(&readme), Some("text"));
        let settled = super::settle_document(&readme, "text");
        assert_eq!(settled.file_name().unwrap(), "onetake-README.md");
        assert!(settled.is_file() && !readme.exists());
        let pdf = dir.join("paper.bin");
        std::fs::write(&pdf, b"%PDF-1.7\n...").unwrap();
        assert_eq!(super::document_kind(&pdf), Some("pdf"));
        assert_eq!(super::settle_document(&pdf, "pdf").file_name().unwrap(), "paper.pdf");
        let lottie = dir.join("anim.json");
        std::fs::write(&lottie, r#"{"v":"5.7.4","fr":30,"layers":[]}"#).unwrap();
        assert_eq!(super::document_kind(&lottie), None);
        let svg = dir.join("logo.svg");
        std::fs::write(&svg, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        assert_eq!(super::document_kind(&svg), None);
        let png = dir.join("p.png");
        std::fs::write(&png, studio_media::png::encode(&studio_media::RgbaImage { width: 2, height: 2, pixels: vec![255; 16] })).unwrap();
        assert_eq!(super::document_kind(&png), None);
        // A name given with its extension does not get it twice.
        assert_eq!(super::file_name("onetake-README.md", "md"), "onetake-README.md");
        assert_eq!(super::file_name("clip", "mp4"), "clip.mp4");
        assert_eq!(super::document_ext("text/plain", "https://raw.example.com/a/README.md").as_deref(), Some("md"));
        assert_eq!(super::document_ext("text/plain", "https://example.com/notes").as_deref(), Some("txt"));
        assert_eq!(super::document_ext("text/html", "https://example.com/watch?v=1"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A downloader that hangs is stopped; one that keeps writing is not.
    #[test]
    fn direct_links_are_told_from_pages() {
        assert!(is_direct("https://example.com/clips/beach.MP4"));
        assert!(is_direct("https://cdn.example.com/a/song.mp3?token=abc"));
        assert!(!is_direct("https://www.youtube.com/watch?v=abc"));
        assert!(!is_direct("https://vimeo.com/123456"));
    }

    #[test]
    fn only_web_links() {
        assert!(check_url("https://example.com/a.mp4").is_ok());
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("ftp://example.com/a.mp4").is_err());
        assert!(check_url("https://example.com/a b.mp4").is_err());
    }

    #[test]
    fn names_are_safe() {
        assert_eq!(file_name("../../etc/passwd", "mp4"), "etc-passwd.mp4");
        assert_eq!(file_name("Beach day: take 2", "mov"), "Beach day- take 2.mov");
        assert_eq!(file_name("", "mp3"), "download.mp3");
        assert_eq!(percent_decode("Beach%20day"), "Beach day");
    }

    /// Fetches from a local web server (no internet needed).
    #[test]
    fn a_direct_link_is_fetched() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = vec![7u8; 5000];
        let served = body.clone();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", served.len());
                let _ = s.write_all(&served);
            }
        });
        let dir = std::env::temp_dir().join(format!("studio-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = fetch(&format!("http://127.0.0.1:{port}/media/Beach%20day.mp4"), &dir, None).unwrap();
        assert_eq!(f.file_name().unwrap().to_string_lossy(), "Beach day.mp4");
        assert_eq!(std::fs::read(&f).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tiny web server for pages: path to (content type, body).
    fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_owned();
                    match routes.iter().find(|r| r.0 == path) {
                        Some((_, ct, body)) => {
                            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = s.write_all(body);
                        }
                        None => {
                            let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        port
    }

    /// A page link works when the page openly exposes its video; one that
    /// does not, or whose player is protected, is refused in plain words
    /// that name no other program.
    #[test]
    fn a_page_with_a_video_is_fetched_and_one_without_is_refused() {
        let mut mp4 = vec![0, 0, 0, 24];
        mp4.extend_from_slice(b"ftypisom\0\0\x02\0isomiso2");
        mp4.extend_from_slice(&[7; 3000]);
        let port = serve(vec![
            ("/watch", "text/html", br#"<html><head><title>Beach day</title></head><body><video src="/media/beach"></video></body></html>"#.to_vec()),
            ("/media/beach", "video/mp4", mp4.clone()),
            ("/empty", "text/html", b"<html><body>Nothing to see</body></html>".to_vec()),
            ("/drm", "text/html", br#"<script>setup({"com.widevine.alpha": {}})</script>"#.to_vec()),
        ]);
        let dir = std::env::temp_dir().join(format!("studio-dl-page-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let got = fetch_pick(&format!("http://127.0.0.1:{port}/watch"), &dir, None, &Pick::default()).unwrap();
        assert_eq!(got.file.file_name().unwrap().to_string_lossy(), "Beach day.mp4");
        assert_eq!(std::fs::read(&got.file).unwrap(), mp4);
        assert_eq!(got.via, "file");
        for (path, needle) in [("/empty", "openly"), ("/drm", "copy protection")] {
            let err = fetch(&format!("http://127.0.0.1:{port}{path}"), &dir, None).unwrap_err();
            assert!(err.contains(needle), "{err}");
            let low = err.to_ascii_lowercase();
            assert!(!["yt-dlp", "youtube-dl", "ffmpeg", "curl", "wget", "brew"].iter().any(|b| low.contains(b)), "{err}");
        }
        assert!(fetch_pick("http://127.0.0.1:1/x", &dir, None, &Pick { quality: Some("loud".into()), language: None }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- any file, 3D models, redirects, plain failures ----

    /// A web server with status codes and redirects: path to (status,
    /// headers, body). Anything else is a 404.
    fn serve_raw(routes: Vec<(&'static str, u16, Vec<(&'static str, String)>, Vec<u8>)>) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_owned();
                    match routes.iter().find(|r| r.0 == path) {
                        Some((_, status, headers, body)) => {
                            let mut out = format!("HTTP/1.1 {status} X\r\n");
                            for (k, v) in headers {
                                out.push_str(&format!("{k}: {v}\r\n"));
                            }
                            out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
                            let _ = s.write_all(out.as_bytes());
                            let _ = s.write_all(body);
                        }
                        None => {
                            let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        port
    }

    fn octet() -> Vec<(&'static str, String)> {
        vec![("Content-Type", "application/octet-stream".into())]
    }

    /// A small GLB with some non-ASCII bytes, as a real model has.
    fn a_glb() -> Vec<u8> {
        studio_mesh::export::glb(&studio_mesh::prim::cube([1.0, 2.0, 3.0]))
    }

    #[test]
    fn page_links_to_files_become_file_links() {
        assert_eq!(normalise_url("https://github.com/o/r/blob/main/models/a.glb"), "https://github.com/o/r/raw/main/models/a.glb");
        assert_eq!(normalise_url("https://github.com/o/r/blob/refs/heads/dev/a b.glb"), "https://github.com/o/r/raw/refs/heads/dev/a b.glb");
        assert_eq!(normalise_url("https://huggingface.co/o/m/blob/main/x.bin"), "https://huggingface.co/o/m/resolve/main/x.bin");
        assert_eq!(normalise_url("https://www.dropbox.com/s/abc/a.glb?dl=0"), "https://www.dropbox.com/s/abc/a.glb?dl=1");
        for same in ["https://github.com/o/r", "https://github.com/o/r/raw/main/a.glb", "https://raw.githubusercontent.com/o/r/main/a.glb", "https://example.com/watch?v=1"] {
            assert_eq!(normalise_url(same), same);
        }
    }

    #[test]
    fn direct_links_include_models_and_not_hosts() {
        for yes in ["https://github.com/o/r/raw/refs/heads/main/2026-0728-144611-877418-2026-0728-144609-996759.glb", "https://x.test/a.FBX?token=1", "https://x.test/a/b.ply", "https://x.test/m.usdz", "https://x.test/f.ttf", "https://x.test/a.zip"] {
            assert!(is_direct(yes), "{yes}");
        }
        // A host name is not a file extension; neither is a page.
        for no in ["https://example.com", "https://example.com/", "https://example.com/watch?v=a.b", "https://example.com/v1.2", "https://x.test/index.html", "https://x.test/live.m3u8"] {
            assert!(!is_direct(no), "{no}");
        }
        assert_eq!(url_ext("https://example.com"), None);
    }

    /// The failing case: a model behind a redirect, served as
    /// application/octet-stream, was read as a web page and crashed.
    #[test]
    fn a_model_behind_a_redirect_is_saved_whole() {
        let glb = a_glb();
        let port = serve_raw(vec![
            ("/o/r/raw/refs/heads/main/rooster.glb", 302, vec![("Location", "/files/abc123".into()), ("Content-Type", "text/html; charset=utf-8".into())], vec![]),
            ("/files/abc123", 200, octet(), glb.clone()),
        ]);
        let dir = std::env::temp_dir().join(format!("studio-dl-glb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let got = fetch_pick(&format!("http://127.0.0.1:{port}/o/r/raw/refs/heads/main/rooster.glb"), &dir, Some("rooster.glb"), &Pick::default()).unwrap();
        assert_eq!(got.file.file_name().unwrap().to_string_lossy(), "rooster.glb");
        assert_eq!(std::fs::read(&got.file).unwrap(), glb);
        assert!(is_model(&got.file));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A link with no extension that serves bytes is a file, and the bytes
    /// name it.
    #[test]
    fn a_link_without_an_extension_that_serves_a_model_is_a_file() {
        let glb = a_glb();
        let port = serve_raw(vec![("/download/42", 200, octet(), glb.clone())]);
        let dir = std::env::temp_dir().join(format!("studio-dl-noext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let got = fetch_pick(&format!("http://127.0.0.1:{port}/download/42"), &dir, None, &Pick::default()).unwrap();
        assert_eq!(got.file.extension().unwrap(), "glb", "{:?}", got.file);
        assert_eq!(std::fs::read(&got.file).unwrap(), glb);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whatever the link is called, a file with other bytes gets the name
    /// its bytes say.
    #[test]
    fn a_wrongly_named_model_is_renamed_by_its_bytes() {
        let glb = a_glb();
        let port = serve_raw(vec![("/m.bin", 200, octet(), glb)]);
        let dir = std::env::temp_dir().join(format!("studio-dl-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let got = fetch_pick(&format!("http://127.0.0.1:{port}/m.bin"), &dir, None, &Pick::default()).unwrap();
        assert_eq!(got.file.extension().unwrap(), "glb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_refused_and_pointer_files_say_why() {
        let port = serve_raw(vec![
            ("/denied.glb", 403, vec![("Content-Type", "text/html".into())], b"no".to_vec()),
            ("/lfs.glb", 200, octet(), b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 5813324\n".to_vec()),
            ("/empty.glb", 200, octet(), vec![]),
            ("/hidden", 403, vec![("Content-Type", "text/html".into())], b"no".to_vec()),
        ]);
        let dir = std::env::temp_dir().join(format!("studio-dl-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let u = |p: &str| format!("http://127.0.0.1:{port}{p}");
        let e = fetch_pick(&u("/gone.glb"), &dir, None, &Pick::default()).unwrap_err();
        assert!(e.contains("404") && e.contains("/gone.glb") && e.contains("not there"), "{e}");
        let e = fetch_pick(&u("/denied.glb"), &dir, None, &Pick::default()).unwrap_err();
        assert!(e.contains("403") && e.contains("tell the person"), "{e}");
        let e = fetch_pick(&u("/hidden"), &dir, None, &Pick::default()).unwrap_err();
        assert!(e.contains("403"), "{e}");
        let e = fetch_pick(&u("/lfs.glb"), &dir, None, &Pick::default()).unwrap_err();
        assert!(e.contains("Git LFS") && e.contains("5813324"), "{e}");
        let e = fetch_pick(&u("/empty.glb"), &dir, None, &Pick::default()).unwrap_err();
        assert!(e.contains("empty"), "{e}");
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing half-saved is left");
        assert_eq!(lfs_media_url("https://raw.githubusercontent.com/o/r/refs/heads/main/m/a.glb?x=1").as_deref(), Some("https://media.githubusercontent.com/media/o/r/main/m/a.glb"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A .gltf comes with the buffer it points at.
    #[test]
    fn a_gltf_brings_its_buffer() {
        let port = serve_raw(vec![
            ("/m/scene.gltf", 200, vec![("Content-Type", "model/gltf+json".into())], br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"scene.bin","byteLength":4},{"uri":"data:application/octet-stream;base64,AAAA","byteLength":3}]}"#.to_vec()),
            ("/m/scene.bin", 200, octet(), vec![1, 2, 3, 4]),
        ]);
        let dir = std::env::temp_dir().join(format!("studio-dl-gltf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let got = fetch_pick(&format!("http://127.0.0.1:{port}/m/scene.gltf"), &dir, None, &Pick::default()).unwrap();
        assert_eq!(got.file.parent().unwrap().file_name().unwrap(), "scene");
        assert_eq!(std::fs::read(got.file.with_file_name("scene.bin")).unwrap(), vec![1, 2, 3, 4]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_found_in_the_project_and_misses_say_where_they_looked() {
        let home = std::env::temp_dir().join(format!("studio-dl-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Downloads/sub")).unwrap();
        std::fs::write(home.join("Downloads/rooster.glb"), b"x").unwrap();
        std::fs::write(home.join("Downloads/sub/Hen.GLB"), b"x").unwrap();
        let want = home.join("Downloads/rooster.glb");
        for given in ["Downloads/rooster.glb", "rooster.glb", &format!("file://{}", want.display()), &format!("\"{}\"", want.display()), &want.display().to_string()] {
            assert_eq!(resolve_local(Some(&home), given).unwrap(), want, "{given}");
        }
        assert_eq!(resolve_local(Some(&home), "hen.glb").unwrap(), home.join("Downloads/sub/Hen.GLB"));
        let e = resolve_local(Some(&home), "chicken.glb").unwrap_err();
        assert!(e.contains("no file at \"chicken.glb\"") && e.contains("rooster.glb") && e.contains("Downloads"), "{e}");
        let e = resolve_local(Some(&home), "Downloads").unwrap_err();
        assert!(e.contains("is a folder"), "{e}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The tools, end to end: `download` adds the model, `model` takes a
    /// link or a Downloads name, `import` sends models to the model clip,
    /// a failing tool says why.
    #[test]
    fn the_tools_take_links_and_names_for_models() {
        let glb = a_glb();
        let port = serve_raw(vec![
            ("/raw/rooster.glb", 302, vec![("Location", "/files/9".into())], vec![]),
            ("/files/9", 200, octet(), glb.clone()),
            ("/pack.zip", 200, vec![("Content-Type", "application/zip".into())], b"PK\x03\x04\0\0\0\0".to_vec()),
        ]);
        let dir = std::env::temp_dir().join(format!("studio-dl-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = crate::agent::Session::at(&dir).unwrap();
        let text = |r: Vec<crate::agent::Content>| r.iter().filter_map(|p| if let crate::agent::Content::Text(t) = p { Some(t.clone()) } else { None }).collect::<Vec<_>>().join("\n");
        let link = format!("http://127.0.0.1:{port}/raw/rooster.glb");
        let r = text(s.run("download", &serde_json::json!({"url": link, "name": "rooster.glb"})).unwrap());
        assert!(r.contains("\"model\":true") && r.contains("rooster"), "{r}");
        assert!(m::file::ProjectFolder::new(&dir).download_dir(m::file::MediaKind::Models3d).join("rooster.glb").is_file());
        let models = |s: &crate::agent::Session| s.doc.project.clips.iter().filter(|c| c.name == "rooster" || c.name == "chicken" || c.name == "hen").count();
        assert_eq!(models(&s), 1);
        // The same file by its name, by a link, and through import.
        s.run("edit", &serde_json::json!({"ops": [{"op": "model", "file": "rooster.glb", "name": "hen"}]})).unwrap();
        s.run("edit", &serde_json::json!({"ops": [{"op": "model", "file": link, "name": "chicken"}]})).unwrap();
        let r = text(s.run("import", &serde_json::json!({"paths": [m::file::ProjectFolder::new(&dir).download_dir(m::file::MediaKind::Models3d).join("rooster.glb")]})).unwrap());
        assert!(r.contains("3D model clip"), "{r}");
        assert_eq!(models(&s), 4);
        // A file that is not media is kept, not deleted.
        let r = text(s.run("download", &serde_json::json!({"url": format!("http://127.0.0.1:{port}/pack.zip")})).unwrap());
        assert!(r.contains("kept_as_file") && m::file::ProjectFolder::new(&dir).download_dir(m::file::MediaKind::Other).join("pack.zip").is_file(), "{r}");
        // Misses say where they looked.
        let e = s.run("edit", &serde_json::json!({"ops": [{"op": "model", "file": "Downloads/nothing.glb"}]})).err().expect("it fails");
        assert!(e.contains("no file at") && e.contains("rooster.glb"), "{e}");
        let e = s.run("edit", &serde_json::json!({"ops": [{"op": "model", "file": format!("http://127.0.0.1:{port}/missing.glb")}]})).err().expect("it fails");
        assert!(e.contains("404") && e.contains("missing.glb"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
