// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a page openly says about its media: `<video>` / `<audio>` /
//! `<source>`, Open Graph and Twitter player tags, JSON-LD VideoObject,
//! `<link rel="alternate">`, oEmbed discovery, and HLS / DASH manifest
//! addresses sitting in the page or its scripts.

use crate::html::{self, Tag, Tok};
use crate::url::{ext_of, join};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CKind {
    File,
    Hls,
    Dash,
    /// Another page that plays it (an embed), to be looked at in turn.
    Embed,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub url: String,
    pub kind: CKind,
    /// Where on the page it was found, in words.
    pub origin: &'static str,
    /// Picture height, when the page says.
    pub height: Option<u32>,
    pub audio: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Scan {
    /// The title the page gives the video (Open Graph, VideoObject).
    pub title: Option<String>,
    /// The document's own `<title>`, a weaker name.
    pub doc_title: Option<String>,
    pub candidates: Vec<Candidate>,
    /// oEmbed endpoint addresses.
    pub oembed: Vec<String>,
    /// `<iframe>` addresses.
    pub frames: Vec<String>,
    /// Signs of copy protection on the page.
    pub drm: Vec<&'static str>,
    /// A player that builds its video in the browser (`blob:` sources).
    pub blob_player: bool,
}

const AV_EXT: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "mp3", "m4a", "aac", "wav", "ogg", "oga", "ogv", "opus", "flac"];
const AUDIO_EXT: &[&str] = &["mp3", "m4a", "aac", "wav", "oga", "opus", "flac"];

pub fn is_av_ext(e: &str) -> bool {
    AV_EXT.contains(&e)
}

/// What kind of thing `url` (served as `mime`, if known) is.
pub fn classify(url: &str, mime: Option<&str>, unknown: CKind) -> CKind {
    let m = mime.unwrap_or("").to_ascii_lowercase();
    if m.contains("mpegurl") {
        return CKind::Hls;
    }
    if m.contains("dash+xml") {
        return CKind::Dash;
    }
    if m.starts_with("video/") && !m.contains("mp2t") || m.starts_with("audio/") {
        return CKind::File;
    }
    if m.contains("html") || m.contains("shockwave") {
        return CKind::Embed;
    }
    match ext_of(url).as_deref() {
        Some("m3u8") => CKind::Hls,
        Some("mpd") => CKind::Dash,
        Some(e) if is_av_ext(e) => CKind::File,
        _ => unknown,
    }
}

/// A picture height from words like "720", "1080p", "HD 480p", or from a
/// path like ".../video_720p.mp4".
pub fn height_hint(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let st = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let n: u32 = s[st..i].parse().ok()?;
            let after = b.get(i).copied();
            let whole = st == 0 && i == b.len();
            if (n == 2160 || n == 1440 || n == 1080 || n == 720 || n == 480 || n == 360 || n == 240 || n == 144 || n == 576 || n == 540 || n == 4320) && (after == Some(b'p') || after == Some(b'P') || whole) {
                return Some(n);
            }
        } else {
            i += 1;
        }
    }
    None
}

/// `\/`, `&`, `\x2F`, `&amp;` and friends undone, for addresses buried in scripts.
pub fn unescape_script(s: &str) -> String {
    let s = html::decode_entities(s);
    if !s.contains('\\') {
        return s;
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'/' => {
                    out.push('/');
                    i += 2;
                    continue;
                }
                b'u' if i + 6 <= b.len() => {
                    if let Some(c) = s.get(i + 2..i + 6).and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32) {
                        out.push(c);
                        i += 6;
                        continue;
                    }
                }
                b'x' if i + 4 <= b.len() => {
                    if let Some(c) = s.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        out.push(c as char);
                        i += 4;
                        continue;
                    }
                }
                b'"' | b'\'' => {
                    out.push(b[i + 1] as char);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        // Copy one whole character.
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '{' | '}' | '[' | ']' | '|' | '^' | '\\' | ',')
}

/// Media addresses in free text (a script, an attribute): manifests, and
/// plain files when `files` is true.
fn urls_in_text(text: &str, base: &str, files: bool, out: &mut Vec<Candidate>) {
    let lower = text.to_ascii_lowercase();
    let mut exts: Vec<&str> = vec![".m3u8", ".mpd"];
    if files {
        exts.extend([".mp4", ".webm", ".m4a", ".mp3", ".mov"]);
    }
    for ext in exts {
        let mut from = 0;
        while let Some(at) = lower[from..].find(ext) {
            let idx = from + at;
            from = idx + ext.len();
            // Not part of a longer extension or word (".mpd" in ".mpdf").
            if lower[from..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let start = text[..idx].rfind(delim).map_or(0, |p| p + text[p..].chars().next().map_or(1, char::len_utf8));
            let end = text[from..].find(delim).map_or(text.len(), |p| from + p);
            let token = text[start..end].trim_end_matches(['.', ';']);
            if token.contains("${") || token.contains("{{") || token.contains("%s") || token.contains('$') || token.contains('*') || token.len() < 6 {
                continue;
            }
            let Some(url) = join(base, token) else { continue };
            // A bare word like "a.mp4" in prose is not an address worth trying.
            if !token.contains('/') && !token.starts_with("http") {
                continue;
            }
            let kind = classify(&url, None, CKind::File);
            if !files && kind == CKind::File {
                continue;
            }
            if !out.iter().any(|c| c.url == url) {
                out.push(Candidate { height: height_hint(&url), audio: ext_of(&url).is_some_and(|e| AUDIO_EXT.contains(&e.as_str())), url, kind, origin: "an address in the page" });
            }
        }
    }
}

fn add(out: &mut Vec<Candidate>, base: &str, raw: &str, mime: Option<&str>, unknown: CKind, origin: &'static str, height: Option<u32>, audio: bool) {
    let Some(url) = join(base, &html::decode_entities(raw)) else { return };
    let kind = classify(&url, mime, unknown);
    if let Some(c) = out.iter_mut().find(|c| c.url == url) {
        if c.height.is_none() {
            c.height = height;
        }
        return;
    }
    let audio = audio || mime.is_some_and(|m| m.starts_with("audio/")) || ext_of(&url).is_some_and(|e| AUDIO_EXT.contains(&e.as_str()));
    out.push(Candidate { height: height.or_else(|| height_hint(&url)), audio, url, kind, origin });
}

fn json_ld(v: &serde_json::Value, base: &str, scan: &mut Scan) {
    use serde_json::Value;
    match v {
        Value::Array(a) => a.iter().for_each(|x| json_ld(x, base, scan)),
        Value::Object(o) => {
            let is_media = match o.get("@type") {
                Some(Value::String(t)) => matches!(t.as_str(), "VideoObject" | "AudioObject" | "MediaObject" | "Clip" | "Movie" | "MusicRecording"),
                Some(Value::Array(a)) => a.iter().any(|t| matches!(t.as_str(), Some("VideoObject" | "AudioObject" | "MediaObject" | "Clip"))),
                _ => false,
            };
            if is_media {
                let audio = matches!(o.get("@type"), Some(Value::String(t)) if t == "AudioObject" || t == "MusicRecording");
                if scan.title.is_none() {
                    scan.title = o.get("name").and_then(Value::as_str).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
                }
                let height = o.get("height").and_then(|h| h.as_u64().or_else(|| h.as_str().and_then(|s| s.parse().ok()))).map(|h| h as u32);
                for (key, unknown) in [("contentUrl", CKind::File), ("embedUrl", CKind::Embed)] {
                    match o.get(key) {
                        Some(Value::String(s)) => add(&mut scan.candidates, base, s, o.get("encodingFormat").and_then(Value::as_str).filter(|_| key == "contentUrl"), unknown, "the page's VideoObject data", height, audio),
                        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).for_each(|s| add(&mut scan.candidates, base, s, None, unknown, "the page's VideoObject data", height, audio)),
                        _ => {}
                    }
                }
            }
            o.values().for_each(|x| json_ld(x, base, scan));
        }
        _ => {}
    }
}

const DRM_SIGNS: &[(&str, &str)] = &[
    ("signaturecipher", "a signature cipher that hides the video's address"),
    ("com.widevine.alpha", "Widevine"),
    ("com.microsoft.playready", "PlayReady"),
    ("com.apple.fps", "FairPlay"),
    ("com.apple.streamingkeydelivery", "FairPlay"),
    ("requestmediakeysystemaccess", "browser DRM (encrypted media)"),
    ("<contentprotection", "content protection (DRM)"),
];

/// Reads a page. `base` is the address it was fetched from.
pub fn scan(base: &str, page: &str) -> Scan {
    let mut s = Scan::default();
    let toks = html::tokenize(page);
    let mut in_media: Vec<bool> = Vec::new(); // true = audio element
    let (mut og_title, mut doc_title, mut og_type, mut tw_type, mut og_height) = (None::<String>, None::<String>, None::<String>, None::<String>, None::<u32>);
    let mut og_urls: Vec<(String, bool)> = Vec::new();
    let mut tw_stream: Vec<String> = Vec::new();
    for t in &toks {
        match t {
            Tok::Open(tag) => match tag.name.as_str() {
                "video" | "audio" => {
                    let audio = tag.name == "audio";
                    in_media.push(audio);
                    if let Some(src) = tag.first(&["src", "data-src", "data-video-src", "data-mp4", "data-hls", "data-hls-src", "data-stream"]) {
                        media_src(&mut s, base, tag, src, audio);
                    }
                }
                "source" => {
                    if let Some(src) = tag.first(&["src", "data-src"]) {
                        media_src(&mut s, base, tag, src, in_media.last().copied().unwrap_or(false));
                    }
                }
                "meta" => {
                    let key = tag.first(&["property", "name", "itemprop"]).unwrap_or("").to_ascii_lowercase();
                    let Some(content) = tag.get("content").map(str::trim).filter(|c| !c.is_empty()) else { continue };
                    match key.as_str() {
                        "og:title" | "twitter:title" if og_title.is_none() => og_title = Some(content.to_owned()),
                        "og:video:type" | "og:audio:type" => og_type = Some(content.to_owned()),
                        "twitter:player:stream:content_type" => tw_type = Some(content.to_owned()),
                        "og:video:height" => og_height = content.parse().ok(),
                        "og:video" | "og:video:url" | "og:video:secure_url" => og_urls.push((content.to_owned(), false)),
                        "og:audio" | "og:audio:url" | "og:audio:secure_url" => og_urls.push((content.to_owned(), true)),
                        "twitter:player:stream" => tw_stream.push(content.to_owned()),
                        "twitter:player" => add(&mut s.candidates, base, content, None, CKind::Embed, "the page's Twitter player tag", None, false),
                        "contenturl" => add(&mut s.candidates, base, content, None, CKind::File, "the page's data", None, false),
                        "embedurl" => add(&mut s.candidates, base, content, None, CKind::Embed, "the page's data", None, false),
                        _ => {}
                    }
                }
                "link" => {
                    let rel = tag.get("rel").unwrap_or("").to_ascii_lowercase();
                    let ty = tag.get("type").unwrap_or("").to_ascii_lowercase();
                    let Some(href) = tag.get("href") else { continue };
                    if ty.contains("oembed") {
                        if let Some(u) = join(base, href) {
                            if !s.oembed.contains(&u) {
                                s.oembed.push(u);
                            }
                        }
                    } else if rel.split_whitespace().any(|r| r == "alternate") && (ty.starts_with("video/") || ty.starts_with("audio/") || ty.contains("mpegurl") || ty.contains("dash+xml")) {
                        add(&mut s.candidates, base, href, Some(&ty), CKind::File, "a link to an alternate version", tag.first(&["title", "media"]).and_then(height_hint), false);
                    }
                }
                "iframe" => {
                    if let Some(u) = tag.first(&["src", "data-src"]).and_then(|u| join(base, u)) {
                        if !s.frames.contains(&u) {
                            s.frames.push(u);
                        }
                    }
                }
                _ => {}
            },
            Tok::Close(n) if n == "video" || n == "audio" => {
                in_media.pop();
            }
            Tok::Close(_) => {}
            Tok::Raw(tag, text) => match tag.name.as_str() {
                "title" => doc_title = Some(html::decode_entities(text.trim()).split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty()),
                "script" => {
                    let ty = tag.get("type").unwrap_or("").to_ascii_lowercase();
                    if ty.contains("ld+json") {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                            json_ld(&v, base, &mut s);
                        }
                    }
                }
                _ => {}
            },
        }
    }
    for (u, audio) in og_urls {
        add(&mut s.candidates, base, &u, og_type.as_deref(), CKind::Embed, "the page's Open Graph tags", og_height, audio);
    }
    for u in tw_stream {
        add(&mut s.candidates, base, &u, tw_type.as_deref(), CKind::File, "the page's Twitter player tag", None, false);
    }
    // Manifests (and, as a last resort, files) named anywhere in the page or its scripts.
    let text = unescape_script(page);
    urls_in_text(&text, base, true, &mut s.candidates);
    // Those from scripts rank below what the page declares.
    let lower = page.to_ascii_lowercase();
    for (needle, what) in DRM_SIGNS {
        if lower.contains(needle) && !s.drm.contains(what) {
            s.drm.push(what);
        }
    }
    s.blob_player = lower.contains("src=\"blob:") || lower.contains("src='blob:");
    let tidy = |t: Option<String>| t.map(|t| t.chars().take(120).collect::<String>().trim().to_owned()).filter(|t| !t.is_empty());
    s.title = tidy(og_title.or(s.title.take()));
    s.doc_title = tidy(doc_title);
    s
}

fn media_src(s: &mut Scan, base: &str, tag: &Tag, src: &str, audio: bool) {
    let hint = tag.first(&["size", "res", "data-res", "data-quality", "label", "title", "data-height", "height"]).and_then(|v| v.trim().parse::<u32>().ok().filter(|n| *n >= 144).or_else(|| height_hint(v)));
    add(&mut s.candidates, base, src, tag.get("type"), CKind::File, if tag.name == "source" { "a <source> element" } else if audio { "an <audio> element" } else { "a <video> element" }, hint, audio);
}

/// Addresses of video, audio and manifests inside an oEmbed reply's `html`
/// (an iframe, a video tag): the player to look at, or the file itself.
pub fn from_embed_html(base: &str, snippet: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    for t in html::tokenize(snippet) {
        if let Tok::Open(tag) = t {
            match tag.name.as_str() {
                "iframe" | "embed" => {
                    if let Some(u) = tag.first(&["src", "data-src"]) {
                        add(&mut out, base, u, None, CKind::Embed, "the oEmbed player", None, false);
                    }
                }
                "video" | "audio" | "source" => {
                    if let Some(u) = tag.first(&["src", "data-src"]) {
                        add(&mut out, base, u, tag.get("type"), CKind::File, "the oEmbed player", None, tag.name == "audio");
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &Scan) -> Vec<(CKind, String, &'static str)> {
        s.candidates.iter().map(|c| (c.kind, c.url.clone(), c.origin)).collect()
    }

    #[test]
    fn elements_meta_links_and_json_ld() {
        let page = r#"<html><head><title> My  clip </title>
<meta property="og:title" content="The Clip">
<meta property="og:video:secure_url" content="/media/clip.mp4"><meta property="og:video:type" content="video/mp4"><meta property="og:video:height" content="720">
<meta name="twitter:player" content="https://player.example.com/embed/9">
<link rel="alternate" type="application/json+oembed" href="https://example.com/oembed?url=x&amp;format=json">
<link rel="alternate" type="application/dash+xml" href="/v/stream.mpd">
<script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"VideoObject","name":"LD name","contentUrl":"https://cdn.example.com/ld.webm","embedUrl":"https://example.com/e/1"}]}</script>
</head><body>
<video poster="p.jpg" controls><source src="hi.mp4" type="video/mp4" size="1080"><source src="lo.mp4" type="video/mp4" label="480p"><source src="live.m3u8" type="application/x-mpegURL"></video>
<audio src="song.mp3"></audio>
<video src="blob:https://example.com/1234"></video>
<script>var cfg = {"hls":"https:\/\/cdn.example.com\/a\/master.m3u8?t=1&k=2","dash":"//cdn2.example.com/x.mpd"};</script>
<iframe src="https://other.example.com/frame"></iframe>
</body></html>"#;
        let s = scan("https://example.com/watch/1", page);
        assert_eq!(s.title.as_deref(), Some("The Clip"));
        let k = kinds(&s);
        let has = |kind: CKind, url: &str| k.iter().any(|(c, u, _)| *c == kind && u == url);
        assert!(has(CKind::File, "https://example.com/media/clip.mp4"));
        assert!(has(CKind::Embed, "https://player.example.com/embed/9"));
        assert!(has(CKind::Dash, "https://example.com/v/stream.mpd"));
        assert!(has(CKind::File, "https://cdn.example.com/ld.webm"));
        assert!(has(CKind::Embed, "https://example.com/e/1"));
        assert!(has(CKind::File, "https://example.com/watch/hi.mp4"));
        assert!(has(CKind::Hls, "https://example.com/watch/live.m3u8"));
        assert!(has(CKind::File, "https://example.com/watch/song.mp3"));
        assert!(has(CKind::Hls, "https://cdn.example.com/a/master.m3u8?t=1&k=2"));
        assert!(has(CKind::Dash, "https://cdn2.example.com/x.mpd"));
        assert!(!k.iter().any(|(_, u, _)| u.starts_with("blob:")));
        assert!(s.blob_player);
        assert_eq!(s.oembed, ["https://example.com/oembed?url=x&format=json"]);
        assert_eq!(s.frames, ["https://other.example.com/frame"]);
        let hi = s.candidates.iter().find(|c| c.url.ends_with("/hi.mp4")).unwrap();
        assert_eq!(hi.height, Some(1080));
        let lo = s.candidates.iter().find(|c| c.url.ends_with("/lo.mp4")).unwrap();
        assert_eq!(lo.height, Some(480));
        assert!(s.candidates.iter().find(|c| c.url.ends_with("song.mp3")).unwrap().audio);
        assert_eq!(s.candidates.iter().find(|c| c.url.ends_with("/media/clip.mp4")).unwrap().height, Some(720));
    }

    #[test]
    fn protection_signs_and_nothing_found() {
        let s = scan("https://e.com/", r#"<script>player.setup({drm:{"com.widevine.alpha":{"licenseUrl":"x"}}})</script>"#);
        assert!(s.candidates.is_empty());
        assert_eq!(s.drm, ["Widevine"]);
        assert_eq!(height_hint("video_720p.mp4"), Some(720));
        assert_eq!(height_hint("1080"), Some(1080));
        assert_eq!(height_hint("a1080b"), None);
        let none = scan("https://e.com/", "<p>See clip.mp4 for the video, and ${url}.m3u8 too</p>");
        assert!(none.candidates.is_empty());
    }
}
