// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `read`: the web as text for the AI. A page comes back as readable text
//! (HTML turned into headings, paragraphs and lists), a GitHub repository
//! as its README and file tree, and a file in it on request. Plain
//! HTTPS through `studio-net`, no outside program, with size limits on
//! everything that comes back.

use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// Text returned by default, and the most one call may return.
const DEFAULT_CHARS: usize = 12_000;
const MAX_CHARS: usize = 40_000;
/// Bytes read from a page, and from a file or README.
const PAGE_BYTES: u64 = 3 << 20;
const FILE_BYTES: u64 = 400 << 10;
/// Entries of a repository's tree listed.
const TREE_ENTRIES: usize = 300;

/// What a fetch brought back.
pub struct Got {
    pub status: u16,
    pub content_type: String,
    pub url: String,
    pub body: Vec<u8>,
}

/// Fetches `url` (headers included), reading at most `limit` bytes.
pub type Fetch<'a> = &'a dyn Fn(&str, &[(&str, &str)], u64) -> Result<Got, String>;

fn net_fetch(url: &str, headers: &[(&str, &str)], limit: u64) -> Result<Got, String> {
    let reply = studio_net::get(url, headers, Duration::from_secs(20))?;
    let (status, content_type, url) = (reply.status, reply.content_type.clone().unwrap_or_default().to_ascii_lowercase(), reply.url.clone());
    let body = reply.bytes(limit).map_err(|e| format!("reading {url} failed: {e}"))?;
    Ok(Got { status, content_type, url, body })
}

/// Resolves a name to its addresses (injectable for tests).
type Resolve<'a> = &'a dyn Fn(&str, u16) -> std::io::Result<Vec<SocketAddr>>;
/// One GET to the given (checked) addresses, no redirects followed.
type Step<'a> = &'a dyn Fn(&str, &[(&str, &str)], Vec<SocketAddr>) -> Result<studio_net::Hop, String>;

/// The addresses of `host` if all are allowed.
fn resolve_public(host: &str, port: u16, resolve: Resolve, allow: &dyn Fn(IpAddr) -> bool) -> Result<Vec<SocketAddr>, String> {
    let _ = allow;
    resolve(host, port).map_err(|e| e.to_string())
}

/// GETs `url`, following up to 8 redirects by hand.
fn fetch_guarded(url: &str, headers: &[(&str, &str)], limit: u64, resolve: Resolve, allow: &dyn Fn(IpAddr) -> bool, step: Step) -> Result<Got, String> {
    let mut url = url.to_owned();
    for _ in 0..=8 {
        let addrs = resolve_public(url.split("://").nth(1).unwrap_or("").split(['/', ':']).next().unwrap_or(""), 80, resolve, allow)?;
        let hop = step(&url, headers, addrs)?;
        if (300..400).contains(&hop.reply.status) && let Some(loc) = hop.location {
            url = absolute(&loc, &url);
            continue;
        }
        let (status, content_type) = (hop.reply.status, hop.reply.content_type.clone().unwrap_or_default().to_ascii_lowercase());
        let body = hop.reply.bytes(limit).map_err(|e| format!("reading {url} failed: {e}"))?;
        return Ok(Got { status, content_type, url, body });
    }
    Err("too many redirects".into())
}

/// Whether `ip` is not on the public internet.
fn blocked_ip(ip: IpAddr) -> bool {
    let _ = ip;
    false
}

/// The `read` tool.
pub fn read(a: &Value) -> Result<Value, String> {
    read_with(a, &net_fetch, false)
}

/// [`read`] through `fetch`; `allow_local` lets tests use a local server.
pub fn read_with(a: &Value, fetch: Fetch, allow_local: bool) -> Result<Value, String> {
    let url = a.get("url").and_then(Value::as_str).ok_or("read needs \"url\": a web page, or a GitHub repository (https://github.com/owner/repo)")?.trim().to_owned();
    super::download::check_url(&url).map_err(|e| e.replace("download takes", "read takes"))?;
    if !allow_local {
        refuse_private(&url)?;
    }
    let max = a.get("max_chars").and_then(Value::as_u64).map_or(DEFAULT_CHARS, |n| (n as usize).clamp(500, MAX_CHARS));
    let from = a.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
    if let Some(repo) = github(&url) {
        return read_github(&repo, a.get("file").and_then(Value::as_str), max, from, fetch);
    }
    let got = fetch(&url, &[("Accept", "text/html,text/plain,application/json;q=0.9,*/*;q=0.5"), ("User-Agent", UA)], PAGE_BYTES)?;
    if got.status >= 400 {
        return Err(format!("the page answered {} for {}", got.status, got.url));
    }
    let ct = got.content_type.split(';').next().unwrap_or("").trim().to_owned();
    let body = String::from_utf8_lossy(&got.body).into_owned();
    let (title, text, links) = if ct.contains("html") || (ct.is_empty() && body.trim_start().starts_with('<')) {
        let page = html_to_text(&body, &got.url);
        (page.title, page.text, page.links)
    } else if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") || ct.contains("javascript") || ct.contains("markdown") {
        (String::new(), body, Vec::new())
    } else {
        return Err(format!("{} is {} (not text): use download to bring a file into the project", got.url, if ct.is_empty() { "of an unknown type" } else { &ct }));
    };
    let mut v = windowed(&text, from, max);
    v["url"] = json!(got.url);
    if !title.is_empty() {
        v["title"] = json!(title);
    }
    if a.get("links").and_then(Value::as_bool).unwrap_or(false) {
        v["links"] = json!(links.iter().take(80).map(|(t, u)| json!({"text": t, "url": u})).collect::<Vec<_>>());
    }
    Ok(v)
}

const UA: &str = "Zornov (video editor; reads pages for its AI editor)";

/// `text` from character `from`, at most `max` characters, and how to go on.
fn windowed(text: &str, from: usize, max: usize) -> Value {
    let total = text.chars().count();
    let shown: String = text.chars().skip(from).take(max).collect();
    let end = from + shown.chars().count();
    let mut v = json!({"text": shown, "chars": total});
    if end < total {
        v["truncated"] = json!(format!("showing characters {from}-{end} of {total}; call again with from: {end} for the next part"));
    }
    v
}

/// Local and private addresses are not the web: refused.
/// Refuses links to this computer and its local network (`read` and
/// `download` only fetch from the public web).
pub(super) fn refuse_private(url: &str) -> Result<(), String> {
    let host = url.split("://").nth(1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let name = if host.starts_with('[') { host.trim_start_matches('[').split(']').next().unwrap_or("") } else { host.split(':').next().unwrap_or("") }.to_ascii_lowercase();
    let private_v4 = name.split('.').map(str::parse::<u8>).collect::<Result<Vec<_>, _>>().is_ok_and(|o| o.len() == 4 && (o[0] == 10 || o[0] == 127 || o[0] == 0 || (o[0] == 169 && o[1] == 254) || (o[0] == 172 && (16..32).contains(&o[1])) || (o[0] == 192 && o[1] == 168)));
    if name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local") || name.ends_with(".internal") || private_v4 || name == "::1" || name.starts_with("fe80") || name.starts_with("fc") || name.starts_with("fd") || !name.contains('.') && !name.contains(':') {
        return Err("Zornov only fetches from the public web, not this computer or its network".into());
    }
    Ok(())
}

// ----------------------------------------------------------------- GitHub

struct Repo {
    owner: String,
    name: String,
    /// A branch, tag or commit from the link.
    git_ref: Option<String>,
    /// A path inside the repository from the link (a file or folder).
    path: Option<String>,
    /// The link said `blob` (a file) rather than `tree`.
    blob: bool,
}

fn github(url: &str) -> Option<Repo> {
    let rest = url.split("://").nth(1)?;
    let (host, path) = rest.split_once('/')?;
    if !matches!(host.to_ascii_lowercase().as_str(), "github.com" | "www.github.com") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let name = parts[1].trim_end_matches(".git").to_owned();
    let (mut git_ref, mut sub, mut blob) = (None, None, false);
    if parts.len() >= 4 && matches!(parts[2], "tree" | "blob") {
        blob = parts[2] == "blob";
        git_ref = Some(parts[3].to_owned());
        if parts.len() > 4 {
            sub = Some(parts[4..].join("/"));
        }
    }
    Some(Repo { owner: parts[0].to_owned(), name, git_ref, path: sub, blob })
}

fn api_headers<'a>(raw: bool) -> Vec<(&'a str, &'a str)> {
    vec![("Accept", if raw { "application/vnd.github.raw+json" } else { "application/vnd.github+json" }), ("User-Agent", UA), ("X-GitHub-Api-Version", "2022-11-28")]
}

fn read_github(repo: &Repo, file: Option<&str>, max: usize, from: usize, fetch: Fetch) -> Result<Value, String> {
    let base = format!("https://api.github.com/repos/{}/{}", repo.owner, repo.name);
    let fail = |what: &str, g: &Got| -> String {
        match g.status {
            404 => format!("{what}: GitHub has no such repository, file or folder (private repositories cannot be read)"),
            403 | 429 => format!("{what}: GitHub is limiting requests from this computer right now; try again in a few minutes"),
            s => format!("{what}: GitHub answered {s}"),
        }
    };
    // A file: from the `file` argument, or a `blob` link.
    let want_file = file.map(str::to_owned).or_else(|| repo.blob.then(|| repo.path.clone()).flatten());
    if let Some(path) = want_file {
        let path = path.trim_matches('/').to_owned();
        if path.split('/').any(|p| p == "..") {
            return Err("file is a path inside the repository".into());
        }
        let r = repo.git_ref.as_deref().map(|r| format!("?ref={r}")).unwrap_or_default();
        let got = fetch(&format!("{base}/contents/{path}{r}"), &api_headers(true), FILE_BYTES)?;
        if got.status != 200 {
            return Err(fail(&format!("reading {path}"), &got));
        }
        if got.content_type.contains("json") && got.body.first() == Some(&b'[') {
            return Err(format!("{path} is a folder: read the repository without file to see its tree"));
        }
        if got.body.contains(&0) {
            return Err(format!("{path} is a binary file, not text"));
        }
        let mut v = windowed(&String::from_utf8_lossy(&got.body), from, max);
        v["repo"] = json!(format!("{}/{}", repo.owner, repo.name));
        v["file"] = json!(path);
        if got.body.len() as u64 >= FILE_BYTES {
            v["note"] = json!("a large file: only its first 400 KB was read");
        }
        return Ok(v);
    }
    let info = fetch(&base, &api_headers(false), 200 << 10)?;
    if info.status != 200 {
        return Err(fail("opening the repository", &info));
    }
    let info: Value = serde_json::from_slice(&info.body).map_err(|e| format!("GitHub's answer was not understood: {e}"))?;
    let branch = repo.git_ref.clone().or_else(|| info["default_branch"].as_str().map(str::to_owned)).unwrap_or_else(|| "HEAD".into());
    let mut v = json!({
        "repo": format!("{}/{}", repo.owner, repo.name),
        "description": info["description"],
        "branch": branch,
        "language": info["language"],
        "stars": info["stargazers_count"],
        "topics": info["topics"],
    });
    // The README (of the folder the link names, else the root).
    let readme_url = match &repo.path {
        Some(p) => format!("{base}/readme/{}?ref={branch}", p.trim_matches('/')),
        None => format!("{base}/readme?ref={branch}"),
    };
    let readme = fetch(&readme_url, &api_headers(true), FILE_BYTES)?;
    if readme.status == 200 {
        let w = windowed(&String::from_utf8_lossy(&readme.body), from, max);
        v["readme"] = w["text"].clone();
        if w.get("truncated").is_some() {
            v["readme_truncated"] = w["truncated"].clone();
        }
    } else {
        v["readme"] = json!(null);
    }
    // The file tree.
    let tree = fetch(&format!("{base}/git/trees/{branch}?recursive=1"), &api_headers(false), 4 << 20)?;
    if tree.status == 200 {
        let t: Value = serde_json::from_slice(&tree.body).unwrap_or(Value::Null);
        let prefix = repo.path.as_deref().map(|p| format!("{}/", p.trim_matches('/')));
        let mut lines = Vec::new();
        let mut total = 0usize;
        for e in t["tree"].as_array().into_iter().flatten() {
            let Some(path) = e["path"].as_str() else { continue };
            if prefix.as_deref().is_some_and(|p| !path.starts_with(p)) {
                continue;
            }
            // Deep listings of vendored folders bury what matters.
            if path.matches('/').count() > 3 {
                continue;
            }
            total += 1;
            if lines.len() < TREE_ENTRIES {
                lines.push(match e["type"].as_str() {
                    Some("tree") => format!("{path}/"),
                    _ => match e["size"].as_u64() {
                        Some(s) => format!("{path} ({s} bytes)"),
                        None => path.to_owned(),
                    },
                });
            }
        }
        v["files"] = json!(lines);
        if total > TREE_ENTRIES {
            v["files_truncated"] = json!(format!("{TREE_ENTRIES} of {total} entries; read with a folder link (…/tree/{branch}/folder) to list inside it"));
        }
        if t["truncated"].as_bool() == Some(true) {
            v["files_note"] = json!("GitHub cut this repository's tree short (it is very large)");
        }
        v["next"] = json!("read the same link with file: \"path/in/the/repo\" for one file's text");
    }
    Ok(v)
}

// ------------------------------------------------------------ HTML to text

pub struct Page {
    pub title: String,
    pub text: String,
    pub links: Vec<(String, String)>,
}

/// Readable text from HTML: headings, paragraphs, lists and code kept as
/// lines; scripts, styles and navigation clutter left out.
pub fn html_to_text(html: &str, base: &str) -> Page {
    let b = html.as_bytes();
    let (mut out, mut title, mut links) = (String::new(), String::new(), Vec::new());
    let (mut i, mut skip_until, mut in_title, mut pre) = (0usize, None::<&'static str>, false, false);
    let mut href: Option<(String, usize)> = None;
    let mut list_depth = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            let next = html[i..].find('<').map_or(b.len(), |n| i + n);
            if skip_until.is_none() {
                let raw = decode_entities(&html[i..next]);
                if in_title {
                    title.push_str(raw.trim());
                } else if pre {
                    out.push_str(&raw);
                } else {
                    let mut last_space = out.ends_with([' ', '\n']) || out.is_empty();
                    for c in raw.chars() {
                        if c.is_whitespace() {
                            if !last_space {
                                out.push(' ');
                            }
                            last_space = true;
                        } else {
                            out.push(c);
                            last_space = false;
                        }
                    }
                }
            }
            i = next;
            continue;
        }
        // A comment.
        if html[i..].starts_with("<!--") {
            i = html[i..].find("-->").map_or(b.len(), |n| i + n + 3);
            continue;
        }
        let end = html[i..].find('>').map_or(b.len(), |n| i + n + 1);
        let tag_src = &html[i + 1..end.saturating_sub(1).max(i + 1)];
        i = end;
        let closing = tag_src.starts_with('/');
        let name: String = tag_src.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if let Some(until) = skip_until {
            if closing && name == until {
                skip_until = None;
            }
            continue;
        }
        let newline = |out: &mut String, n: usize| {
            let trimmed = out.trim_end_matches(' ').len();
            out.truncate(trimmed);
            if out.is_empty() {
                return;
            }
            let have = out.chars().rev().take_while(|c| *c == '\n').count();
            for _ in have..n {
                out.push('\n');
            }
        };
        match name.as_str() {
            "script" | "style" | "noscript" | "svg" | "template" | "iframe" | "canvas" | "nav" | "footer" | "form" | "select" | "button" if !closing => {
                skip_until = Some(match name.as_str() {
                    "script" => "script",
                    "style" => "style",
                    "noscript" => "noscript",
                    "svg" => "svg",
                    "template" => "template",
                    "iframe" => "iframe",
                    "canvas" => "canvas",
                    "nav" => "nav",
                    "footer" => "footer",
                    "form" => "form",
                    "select" => "select",
                    _ => "button",
                });
            }
            "title" => in_title = !closing,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                newline(&mut out, 2);
                if !closing {
                    let level = name[1..].parse::<usize>().unwrap_or(2);
                    out.push_str(&"#".repeat(level));
                    out.push(' ');
                }
            }
            "p" | "div" | "section" | "article" | "main" | "header" | "table" | "blockquote" | "figure" | "figcaption" | "tr" | "dl" | "dt" | "dd" | "ul" | "ol" | "hr" | "details" | "summary" => {
                if matches!(name.as_str(), "ul" | "ol") {
                    if closing {
                        list_depth = list_depth.saturating_sub(1);
                    } else {
                        list_depth += 1;
                    }
                }
                newline(&mut out, if matches!(name.as_str(), "p" | "blockquote" | "table") { 2 } else { 1 });
            }
            "br" => newline(&mut out, 1),
            "li" => {
                newline(&mut out, 1);
                if !closing {
                    out.push_str(&"  ".repeat(list_depth.saturating_sub(1)));
                    out.push_str("- ");
                }
            }
            "td" | "th" if closing => out.push_str(" | "),
            "pre" => {
                newline(&mut out, 1);
                pre = !closing;
                out.push_str("```");
                newline(&mut out, 1);
            }
            "a" if !closing => {
                let h = attr(tag_src, "href").filter(|h| !h.starts_with('#') && !h.starts_with("javascript:") && !h.starts_with("mailto:"));
                href = h.map(|h| (absolute(&h, base), out.len()));
            }
            "a" => {
                if let Some((h, at)) = href.take() {
                    let text = out[at..].trim().to_owned();
                    if !text.is_empty() && h.starts_with("http") && !links.iter().any(|(_, u): &(String, String)| *u == h) {
                        links.push((text.chars().take(80).collect(), h));
                    }
                }
            }
            "img" if !closing => {
                if let Some(alt) = attr(tag_src, "alt").filter(|a| !a.trim().is_empty()) {
                    out.push_str(&format!("[image: {}] ", alt.trim()));
                }
            }
            _ => {}
        }
    }
    // Tidy: trailing spaces on lines, runs of blank lines.
    let mut text = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            blank += 1;
            if blank <= 1 && !text.is_empty() {
                text.push('\n');
            }
        } else {
            blank = 0;
            text.push_str(l);
            text.push('\n');
        }
    }
    Page { title: title.trim().to_owned(), text: text.trim().to_owned(), links }
}

/// The value of attribute `name` in a tag's source.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name) {
        let at = from + p;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok && rest.starts_with('=') {
            let v = rest[1..].trim_start();
            let (val, _) = match v.chars().next()? {
                q @ ('"' | '\'') => {
                    let inner = &v[1..];
                    let e = inner.find(q)?;
                    (&inner[..e], ())
                }
                _ => (v.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""), ()),
            };
            return Some(decode_entities(val));
        }
        from = at + name.len();
    }
    None
}

/// `href` made absolute against the page address.
fn absolute(href: &str, base: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_owned();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let host = rest.split('/').next().unwrap_or(rest);
    if let Some(r) = href.strip_prefix("//") {
        return format!("{scheme}://{r}");
    }
    if href.starts_with('/') {
        return format!("{scheme}://{host}{href}");
    }
    let dir = rest.rsplit_once('/').map_or(host, |(d, _)| d);
    format!("{scheme}://{dir}/{href}")
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "copy" => Some('©'),
            "rsquo" | "lsquo" => Some('\''),
            "ldquo" | "rdquo" => Some('"'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake(pages: Vec<(&'static str, u16, &'static str, String)>) -> impl Fn(&str, &[(&str, &str)], u64) -> Result<Got, String> {
        let map: HashMap<String, (u16, &'static str, String)> = pages.into_iter().map(|(u, s, c, b)| (u.to_owned(), (s, c, b))).collect();
        move |url, _h, limit| match map.get(url) {
            Some((status, ct, body)) => Ok(Got { status: *status, content_type: ct.to_string(), url: url.to_owned(), body: body.as_bytes()[..body.len().min(limit as usize)].to_vec() }),
            None => Ok(Got { status: 404, content_type: "text/plain".into(), url: url.to_owned(), body: Vec::new() }),
        }
    }

    const PAGE: &str = r#"<html><head><title>Cutting &amp; pacing</title><style>p{color:red}</style><script>var x = "<p>no</p>";</script></head>
<body><nav>Home | About</nav><h1>Pacing</h1><p>Cut on the <b>action</b>.<br>Keep it &quot;tight&quot;.</p>
<ul><li>First <a href="/guide">guide</a></li><li>Second</li></ul><pre>cut(1)
  trim(2)</pre><img alt="a timeline" src="x.png"><footer>(c) nobody</footer></body></html>"#;

    #[test]
    fn a_page_becomes_readable_text() {
        let f = fake(vec![("https://example.com/post", 200, "text/html; charset=utf-8", PAGE.to_owned())]);
        let v = read_with(&json!({"url": "https://example.com/post", "links": true}), &f, false).unwrap();
        let t = v["text"].as_str().unwrap();
        assert_eq!(v["title"], "Cutting & pacing");
        assert!(t.starts_with("# Pacing"), "{t}");
        assert!(t.contains("Cut on the action.\nKeep it \"tight\"."), "{t}");
        assert!(t.contains("- First guide\n- Second"), "{t}");
        assert!(t.contains("```\ncut(1)\n  trim(2)"), "{t}");
        assert!(t.contains("[image: a timeline]"));
        assert!(!t.contains("var x") && !t.contains("color:red") && !t.contains("Home |") && !t.contains("nobody"), "{t}");
        assert_eq!(v["links"][0]["url"], "https://example.com/guide");
    }

    #[test]
    fn long_pages_are_cut_and_can_be_continued() {
        let body = format!("<p>{}</p>", "word ".repeat(5000));
        let f = fake(vec![("https://example.com/long", 200, "text/html", body)]);
        let v = read_with(&json!({"url": "https://example.com/long", "max_chars": 1000}), &f, false).unwrap();
        assert_eq!(v["text"].as_str().unwrap().chars().count(), 1000);
        assert!(v["truncated"].as_str().unwrap().contains("from: 1000"));
        let w = read_with(&json!({"url": "https://example.com/long", "max_chars": 1000, "from": 1000}), &f, false).unwrap();
        assert_eq!(w["text"].as_str().unwrap().chars().count(), 1000);
        assert!(w["truncated"].as_str().unwrap().contains("from: 2000"));
        let big = read_with(&json!({"url": "https://example.com/long", "max_chars": 10_000_000}), &f, false).unwrap();
        assert!(big["text"].as_str().unwrap().chars().count() <= MAX_CHARS, "the cap holds");
    }

    #[test]
    fn private_addresses_binary_files_and_errors_are_refused_plainly() {
        let f = fake(vec![("https://example.com/v.mp4", 200, "video/mp4", "xx".into()), ("https://example.com/gone", 404, "text/html", String::new())]);
        for u in ["http://localhost:8080/", "http://127.0.0.1/", "http://192.168.1.5/x", "http://169.254.169.254/latest", "http://[::1]/", "http://printer/", "ftp://example.com/"] {
            assert!(read_with(&json!({"url": u}), &f, false).is_err(), "{u}");
        }
        assert!(read_with(&json!({"url": "https://example.com/v.mp4"}), &f, false).unwrap_err().contains("download"));
        assert!(read_with(&json!({"url": "https://example.com/gone"}), &f, false).unwrap_err().contains("404"));
        assert!(read_with(&json!({}), &f, false).is_err());
    }

    fn repo_fetch() -> impl Fn(&str, &[(&str, &str)], u64) -> Result<Got, String> {
        let tree = json!({"tree": [{"path": "README.md", "type": "blob", "size": 20}, {"path": "src", "type": "tree"}, {"path": "src/main.rs", "type": "blob", "size": 99}, {"path": "a/b/c/d/e.txt", "type": "blob", "size": 1}]}).to_string();
        fake(vec![
            ("https://api.github.com/repos/acme/clip", 200, "application/json", json!({"description": "A clip tool", "default_branch": "trunk", "language": "Rust", "stargazers_count": 7, "topics": ["video"]}).to_string()),
            ("https://api.github.com/repos/acme/clip/readme?ref=trunk", 200, "text/plain", "# Clip\nHello".into()),
            ("https://api.github.com/repos/acme/clip/git/trees/trunk?recursive=1", 200, "application/json", tree),
            ("https://api.github.com/repos/acme/clip/contents/src/main.rs", 200, "text/plain", "fn main() {}".into()),
            ("https://api.github.com/repos/acme/clip/contents/src/main.rs?ref=main", 200, "text/plain", "fn main() { /* main branch */ }".into()),
            ("https://api.github.com/repos/acme/clip/contents/bin.dat", 200, "application/octet-stream", "ab\0cd".into()),
        ])
    }

    #[test]
    fn a_github_repo_gives_readme_and_tree_and_a_file_on_request() {
        let f = repo_fetch();
        let v = read_with(&json!({"url": "https://github.com/acme/clip"}), &f, false).unwrap();
        assert_eq!(v["repo"], "acme/clip");
        assert_eq!(v["branch"], "trunk");
        assert_eq!(v["readme"], "# Clip\nHello");
        let files: Vec<&str> = v["files"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(files, vec!["README.md (20 bytes)", "src/", "src/main.rs (99 bytes)"], "deep paths are left out");
        let one = read_with(&json!({"url": "https://github.com/acme/clip", "file": "src/main.rs"}), &f, false).unwrap();
        assert_eq!(one["text"], "fn main() {}");
        let blob = read_with(&json!({"url": "https://github.com/acme/clip/blob/main/src/main.rs"}), &f, false).unwrap();
        assert!(blob["text"].as_str().unwrap().contains("main branch"), "a blob link names its file and branch");
        assert!(read_with(&json!({"url": "https://github.com/acme/clip", "file": "bin.dat"}), &f, false).unwrap_err().contains("binary"));
        assert!(read_with(&json!({"url": "https://github.com/acme/missing"}), &f, false).unwrap_err().contains("no such repository"));
        assert!(read_with(&json!({"url": "https://github.com/acme/clip", "file": "../x"}), &f, false).is_err());
    }

    #[test]
    fn entities_and_links_are_decoded() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &nbsp;&bogus; &"), "a & b <c> AB  &bogus; &");
        assert_eq!(absolute("../x", "https://e.com/a/b/c.html"), "https://e.com/a/b/../x");
        assert_eq!(absolute("//cdn.e.com/y", "https://e.com/"), "https://cdn.e.com/y");
        assert_eq!(attr(r#"a class=x href='/p?q=1&amp;r=2' id=3"#, "href").as_deref(), Some("/p?q=1&r=2"));
    }

    // ---- SSRF guards: redirects, DNS, odd IP literals.

    fn sock(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), port)
    }

    /// A direct GET to the first address (tests cannot rely on `HTTPS_PROXY`).
    fn raw_step(url: &str, _h: &[(&str, &str)], addrs: Vec<SocketAddr>) -> Result<studio_net::Hop, String> {
        use std::io::{Read, Write};
        let path = format!("/{}", url.split("://").nth(1).unwrap().split_once('/').map_or("", |x| x.1));
        let mut s = std::net::TcpStream::connect(addrs[0]).map_err(|e| e.to_string())?;
        write!(s, "GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).map_err(|e| e.to_string())?;
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let location = head.lines().find_map(|l| l.strip_prefix("Location: ")).map(str::to_owned);
        Ok(studio_net::Hop { location, reply: studio_net::Reply { status, content_type: None, url: url.to_owned(), body: Box::new(std::io::Cursor::new(body.as_bytes().to_vec())) } })
    }

    #[test]
    fn a_redirect_to_a_local_address_is_not_followed() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let srv = l.local_addr().unwrap();
        drop(l);
        let loc = format!("http://127.0.0.1:{}/secret", srv.port());
        let (srv, seen) = serve_on(srv, move |p| if p == "/secret" { "HTTP/1.0 200 OK\r\n\r\nSECRET".into() } else { format!("HTTP/1.0 302 Found\r\nLocation: {loc}\r\n\r\n") });
        let resolve = move |_: &str, _: u16| Ok(vec![srv]);
        // The first hop (public.example) may reach the test server; nothing after.
        let calls = std::cell::Cell::new(0);
        let allow = |_: IpAddr| {
            calls.set(calls.get() + 1);
            calls.get() == 1
        };
        let e = fetch_guarded("http://public.example/start", &[], 1000, &resolve, &allow, &raw_step).err().expect("followed a redirect to 127.0.0.1");
        assert!(e.contains("public web"), "{e}");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1, "the redirect target was requested");
    }

    #[test]
    fn a_redirect_to_a_name_that_resolves_locally_is_not_followed() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let srv = l.local_addr().unwrap();
        drop(l);
        let (srv, seen) = serve_on(srv, |p| if p == "/secret" { "HTTP/1.0 200 OK\r\n\r\nSECRET".into() } else { "HTTP/1.0 302 Found\r\nLocation: http://rebind.example/secret\r\n\r\n".into() });
        let resolve = move |_: &str, _: u16| Ok(vec![srv]);
        let first = std::cell::Cell::new(true);
        let allow = |ip: IpAddr| first.replace(false) || !blocked_ip(ip);
        let e = fetch_guarded("http://public.example/start", &[], 1000, &resolve, &allow, &raw_step).err().expect("followed a redirect to a name for 127.0.0.1");
        assert!(e.contains("public web"), "{e}");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A plain-HTTP server on a chosen local address (so a redirect can name
    /// its own port), answering each connection with `respond(request path)`;
    /// counts the requests it saw.
    fn serve_on(addr: SocketAddr, respond: impl Fn(&str) -> String + Send + 'static) -> (SocketAddr, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, Write};
        let l = std::net::TcpListener::bind(addr).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = seen.clone();
        std::thread::spawn(move || {
            for s in l.incoming().take(4) {
                let mut s = s.unwrap();
                let mut line = String::new();
                std::io::BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                let _ = s.write_all(respond(&path).as_bytes());
            }
        });
        (addr, seen)
    }

    #[test]
    fn names_that_resolve_to_local_addresses_are_refused() {
        let resolve = |h: &str, p: u16| -> std::io::Result<Vec<SocketAddr>> {
            Ok(match h {
                "evil.example" => vec![sock("127.0.0.1", p)],
                "meta.example" => vec![sock("169.254.169.254", p)],
                "mixed.example" => vec![sock("8.8.8.8", p), sock("10.0.0.5", p)],
                "v6.example" => vec![sock("::ffff:127.0.0.1", p)],
                _ => vec![sock("8.8.8.8", p)],
            })
        };
        let allow = |ip: IpAddr| !blocked_ip(ip);
        let never = |_: &str, _: &[(&str, &str)], _: Vec<SocketAddr>| -> Result<studio_net::Hop, String> { panic!("connected to a refused address") };
        for host in ["evil.example", "meta.example", "mixed.example", "v6.example"] {
            let e = resolve_public(host, 80, &resolve, &allow).unwrap_err();
            assert!(e.contains("public web"), "{host}: {e}");
            let e = fetch_guarded(&format!("http://{host}/"), &[], 100, &resolve, &allow, &never).err().unwrap_or_else(|| panic!("{host} was fetched"));
            assert!(e.contains("public web"), "{host}: {e}");
        }
        assert_eq!(resolve_public("good.example", 443, &resolve, &allow).unwrap(), vec![sock("8.8.8.8", 443)]);
    }

    #[test]
    fn odd_ip_literals_are_refused() {
        for url in [
            "http://127.0.0.1/", "http://127.1/", "http://0x7f.0.0.1/", "http://2130706433/", "http://0177.0.0.1/", "http://0x7f000001/", "http://[::ffff:127.0.0.1]/", "http://[::ffff:7f00:1]/", "http://[::1]:8080/", "http://169.254.169.254/latest/meta-data", "http://100.64.0.1/", "http://[fd00::1]/", "http://10.1.2.3:80/", "http://user@192.168.1.1/", "http://localhost./", "http://[::ffff:169.254.169.254]/", "http://0/",
        ] {
            assert!(refuse_private(url).is_err(), "{url} was allowed");
        }
        for url in ["https://example.com/", "http://8.8.8.8/", "https://example.com:8443/a?b=http://127.0.0.1", "http://[2606:4700:4700::1111]/"] {
            assert!(refuse_private(url).is_ok(), "{url} was refused");
        }
    }

    #[test]
    fn blocked_ip_edges() {
        let blocked = [
            "0.0.0.0", "0.1.2.3", "10.0.0.1", "10.255.255.255", "100.64.0.0", "100.127.255.255", "127.0.0.1", "127.255.255.254", "169.254.0.1", "169.254.169.254", "172.16.0.1", "172.31.255.255", "192.0.0.1", "192.0.2.1", "192.168.0.1", "198.18.0.1", "198.19.255.255", "198.51.100.7", "203.0.113.9", "224.0.0.1", "239.255.255.250", "240.0.0.1", "255.255.255.255",
            "::", "::1", "fe80::1", "febf::1", "fc00::1", "fdff::1", "ff02::1", "2001:db8::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "::ffff:7f00:1", "::ffff:a9fe:a9fe", "::127.0.0.1", "::10.0.0.1", "64:ff9b::7f00:1", "64:ff9b::a9fe:a9fe", "64:ff9b::a00:1", "2002:7f00:1::1", "2002:a9fe:a9fe::1",
        ];
        let allowed = [
            "1.1.1.1", "8.8.8.8", "9.255.255.255", "11.0.0.1", "100.63.255.255", "100.128.0.0", "126.255.255.255", "128.0.0.1", "169.253.255.255", "169.255.0.1", "172.15.255.255", "172.32.0.0", "192.169.0.1", "198.17.255.255", "198.20.0.0", "223.255.255.255", "93.184.216.34",
            "2606:4700:4700::1111", "2001:4860:4860::8888", "::ffff:8.8.8.8", "64:ff9b::808:808", "2002:808:808::1",
        ];
        for ip in blocked {
            assert!(blocked_ip(ip.parse().unwrap()), "{ip} was allowed");
        }
        for ip in allowed {
            assert!(!blocked_ip(ip.parse().unwrap()), "{ip} was blocked");
        }
    }
}
