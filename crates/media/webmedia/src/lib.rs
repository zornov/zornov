// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-webmedia`: the media a web page openly exposes, found and
//! downloaded in Rust, with no outside programs.
//!
//! * [`find`] reads a page: `<video>` / `<audio>` / `<source>` elements,
//!   Open Graph and Twitter player tags, JSON-LD `VideoObject`,
//!   `<link rel="alternate">`, oEmbed discovery, and HLS (`.m3u8`) and DASH
//!   (`.mpd`) addresses in the page or its scripts. Embedded players are
//!   followed a step or two.
//! * [`fetch`] takes the best of what was found. A plain file is
//!   downloaded as it is. HLS (master playlist, variant choice, TS or fMP4
//!   segments, AES-128 with an openly given key) and DASH (SegmentTemplate,
//!   SegmentList, SegmentBase; best video and sound representations) are
//!   fetched in parallel with resume and written as one MP4.
//!
//! The line: no DRM or Widevine / FairPlay / PlayReady circumvention, no
//! getting around a site's signature or cipher, no signing in. Protected or
//! unexposed media is reported plainly as such.

mod aes;
mod assemble;
mod dash;
mod dl;
mod error;
mod fmp4;
mod hls;
mod html;
mod http;
mod mp4w;
mod page;
mod track;
mod ts;
mod types;
mod url;
mod xml;

#[cfg(test)]
mod tests;

pub use error::{Error, Result};
pub use page::{CKind, Candidate};
pub use types::{quiet, Options, Progress, Quality, Report, Saved};

use std::path::Path;

/// What a link turned out to offer, best first.
#[derive(Debug, Clone)]
pub struct Found {
    /// The address after redirects.
    pub page_url: String,
    pub title: Option<String>,
    /// Where the media is, in the order to try them.
    pub candidates: Vec<Candidate>,
    pub notes: Vec<String>,
}

const PAGE_LIMIT: u64 = 8 << 20;
const FOLLOW_LIMIT: usize = 6;

fn looks_like_mpd(s: &str) -> bool {
    let h: String = s.chars().take(2048).collect();
    h.contains("<MPD")
}

fn is_manifest_text(s: &str) -> Option<CKind> {
    let t = s.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("#EXTM3U") {
        Some(CKind::Hls)
    } else if (t.starts_with("<?xml") || t.starts_with("<MPD") || t.starts_with("<!--")) && looks_like_mpd(t) {
        Some(CKind::Dash)
    } else {
        None
    }
}

fn is_json_ld_or_script(c: &Candidate) -> bool {
    c.origin == "an address in the page"
}

/// Lower is tried first.
fn rank(c: &Candidate, q: Quality) -> (u8, i64) {
    let wants_choice = q != Quality::Best;
    let r = match c.kind {
        CKind::File if is_json_ld_or_script(c) => 3,
        CKind::File if wants_choice => 2,
        CKind::File => 0,
        CKind::Hls => 1,
        CKind::Dash => 2,
        CKind::Embed => 5,
    };
    // Among files, the tallest first (or the closest to what was asked for).
    let h = c.height.unwrap_or(0) as i64;
    let pref = match q {
        Quality::Lowest => h,
        Quality::Height(want) => (h - want as i64).abs(),
        _ => -h,
    };
    (r, pref)
}

fn parse_oembed(base: &str, body: &str, scan: &mut Vec<Candidate>, title: &mut Option<String>) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) else { return false };
    if title.is_none() {
        *title = v.get("title").and_then(|t| t.as_str()).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    }
    if let Some(h) = v.get("html").and_then(|h| h.as_str()) {
        scan.extend(page::from_embed_html(base, h));
    }
    true
}

/// Reads `url`: a page, a playlist, a manifest or a media file.
pub fn find(url: &str, opts: &Options, report: Report) -> Result<Found> {
    if !report(&Progress { stage: "looking at the page", done: 0, total: 0, bytes: 0 }) {
        return Err(Error::Cancelled);
    }
    let doc = http::get_doc(url, "", PAGE_LIMIT)?;
    let base = doc.url.clone();
    let mut notes = Vec::new();
    // A media file, a playlist or a manifest given directly.
    let kind_by_type = page::classify(&base, Some(&doc.ct), CKind::Embed);
    if kind_by_type == CKind::File {
        return Ok(Found { page_url: base.clone(), title: None, candidates: vec![Candidate { url: base, kind: CKind::File, origin: "the link itself", height: None, audio: doc.ct.starts_with("audio/") }], notes });
    }
    // Bytes that are not text at all (a model, an archive, a font served as
    // application/octet-stream) are the file itself, not a page to read.
    if !doc.ct.contains("html") && !doc.ct.contains("xml") && !doc.ct.contains("json") && doc.body.iter().take(4096).any(|&b| b == 0) {
        return Ok(Found { page_url: base.clone(), title: None, candidates: vec![Candidate { url: base, kind: CKind::File, origin: "the link itself", height: None, audio: false }], notes });
    }
    let body = doc.text();
    if let Some(k) = is_manifest_text(&body) {
        return Ok(Found { page_url: base.clone(), title: None, candidates: vec![Candidate { url: base, kind: k, origin: "the link itself", height: None, audio: false }], notes });
    }
    let first = page::scan(&base, &body);
    let mut title = first.title.clone();
    let doc_title = first.doc_title.clone();
    let mut cands: Vec<Candidate> = first.candidates.clone();
    let mut drm = first.drm.clone();
    let mut blob = first.blob_player;
    let strong = |c: &[Candidate]| c.iter().any(|x| x.kind != CKind::Embed && !is_json_ld_or_script(x));

    // Players the page points at: oEmbed, embeds, frames. Looked at only
    // when the page itself shows nothing better.
    if !strong(&cands) {
        let mut seen: Vec<String> = vec![base.clone()];
        let mut work: Vec<(String, u8)> = Vec::new();
        let mut oembeds = first.oembed.clone();
        for e in oembeds.drain(..) {
            if seen.len() > FOLLOW_LIMIT {
                break;
            }
            seen.push(e.clone());
            let mut got = http::get_doc(&e, &base, 1 << 20).ok();
            if got.as_ref().is_none_or(|d| serde_json::from_str::<serde_json::Value>(d.text().trim()).is_err()) && !e.contains("format=json") {
                let alt = format!("{e}{}format=json", if e.contains('?') { "&" } else { "?" });
                got = http::get_doc(&alt, &base, 1 << 20).ok().or(got);
            }
            if let Some(d) = got {
                let mut extra = Vec::new();
                if parse_oembed(&d.url, &d.text(), &mut extra, &mut title) {
                    cands.extend(extra);
                }
            }
        }
        for c in cands.iter().filter(|c| c.kind == CKind::Embed) {
            work.push((c.url.clone(), 1));
        }
        for f in &first.frames {
            work.push((f.clone(), 1));
        }
        let mut i = 0;
        while i < work.len() && seen.len() <= FOLLOW_LIMIT && !strong(&cands) {
            let (u, depth) = work[i].clone();
            i += 1;
            if seen.contains(&u) {
                continue;
            }
            seen.push(u.clone());
            if !report(&Progress { stage: "looking at the player", done: 0, total: 0, bytes: 0 }) {
                return Err(Error::Cancelled);
            }
            let Ok(d) = http::get_doc(&u, &base, PAGE_LIMIT) else { continue };
            let k = page::classify(&d.url, Some(&d.ct), CKind::Embed);
            if k == CKind::File {
                cands.push(Candidate { url: d.url.clone(), kind: CKind::File, origin: "an embedded player", height: None, audio: d.ct.starts_with("audio/") });
                continue;
            }
            let text = d.text();
            if let Some(mk) = is_manifest_text(&text) {
                cands.push(Candidate { url: d.url.clone(), kind: mk, origin: "an embedded player", height: None, audio: false });
                continue;
            }
            let s = page::scan(&d.url, &text);
            if title.is_none() {
                title = s.title.clone();
            }
            for c in s.candidates {
                if c.kind == CKind::Embed && depth < 2 {
                    work.push((c.url.clone(), depth + 1));
                }
                if !cands.iter().any(|x| x.url == c.url) {
                    cands.push(c);
                }
            }
            if depth < 2 {
                for f in s.frames {
                    work.push((f, depth + 1));
                }
            }
            for dsign in s.drm {
                if !drm.contains(&dsign) {
                    drm.push(dsign);
                }
            }
            blob |= s.blob_player;
        }
    }
    let title = title.or(doc_title);
    cands.retain(|c| c.kind != CKind::Embed);
    cands.sort_by_key(|c| rank(c, opts.quality));
    if cands.is_empty() {
        return Err(nothing_found(&drm, blob));
    }
    if cands.iter().all(|c| c.kind == CKind::File && c.origin == "an address in the page") {
        notes.push("The only media addresses found were ones inside the page's scripts; Zornov tried those.".into());
    }
    Ok(Found { page_url: base, title, candidates: cands, notes })
}

fn nothing_found(drm: &[&'static str], blob: bool) -> Error {
    if !drm.is_empty() {
        return Error::Protected(format!("This page's video is protected ({}). Zornov does not work around copy protection, or get around a site's signature or cipher, so it cannot be downloaded.", drm.join(", ")));
    }
    if blob {
        return Error::NotExposed("This page's player builds the video inside the browser and does not expose a file or a playlist address, so there is nothing for Zornov to download. Zornov only takes media a page openly links.".into());
    }
    Error::NotExposed("I looked through the page (its video and audio elements, Open Graph and player tags, VideoObject data, alternate links, oEmbed and the playlists named in its scripts) and found no media it openly exposes. If the video needs you to sign in or pay, Zornov cannot get it: it does not log in for you. Only download what you have the right to use.".into())
}

fn stem_from_url(url: &str) -> String {
    let path = url::path_of(url);
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    let stem = last.rsplit_once('.').map_or(last, |(s, _)| s);
    let low = stem.to_ascii_lowercase();
    if stem.is_empty() || matches!(low.as_str(), "index" | "master" | "playlist" | "manifest" | "stream" | "main" | "video") {
        let host = url::host_of(url).trim_start_matches("www.");
        return if host.is_empty() { "download".into() } else { host.to_owned() };
    }
    url::percent_decode(stem)
}

/// Finds the media at `url` and saves it into `dir`, named `name` (or after
/// the page).
pub fn fetch(url: &str, dir: &Path, name: Option<&str>, opts: &Options, report: Report) -> Result<Saved> {
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("cannot make {}: {e}", dir.display())))?;
    let found = find(url, opts, report)?;
    let stem = name.map(str::to_owned).or_else(|| found.title.clone()).unwrap_or_else(|| stem_from_url(&found.page_url));
    let ctx = dl::Ctx { referer: &found.page_url, opts, report, dir, stem: &stem, title: found.title.clone() };
    let mut worst: Option<Error> = None;
    for c in found.candidates.iter().take(6) {
        let r = match c.kind {
            CKind::File => dl::download_file(&c.url, &ctx),
            CKind::Hls => dl::download_hls(&c.url, &ctx),
            CKind::Dash => dl::download_dash(&c.url, &ctx),
            CKind::Embed => continue,
        };
        match r {
            Ok(mut s) => {
                s.notes.extend(found.notes.iter().cloned());
                return Ok(s);
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                if worst.as_ref().is_none_or(|w| e.weight() > w.weight()) {
                    worst = Some(e);
                }
            }
        }
    }
    Err(worst.unwrap_or_else(|| nothing_found(&[], false)))
}
