// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Just enough URL handling: resolving a reference against a base.

/// `(scheme, authority, path-and-query)` of an http(s) URL.
fn split(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((scheme, &rest[..end], &rest[end..]))
}

/// Whether `url` is an absolute http(s) URL.
pub fn is_http(url: &str) -> bool {
    split(url).is_some_and(|(_, host, _)| !host.is_empty())
}

/// `url` without its `#fragment`.
pub fn no_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

/// The path part of `url` (no query, no fragment), or "".
pub fn path_of(url: &str) -> &str {
    let u = url.split(['?', '#']).next().unwrap_or(url);
    match split(u) {
        Some((_, _, p)) => p,
        None => u,
    }
}

/// The host of `url`.
pub fn host_of(url: &str) -> &str {
    split(url).map_or("", |(_, h, _)| h)
}

/// The lower-case extension of the URL's path.
pub fn ext_of(url: &str) -> Option<String> {
    let last = path_of(url).rsplit('/').next()?;
    let (_, e) = last.rsplit_once('.')?;
    (!e.is_empty() && e.len() <= 6 && e.chars().all(|c| c.is_ascii_alphanumeric())).then(|| e.to_ascii_lowercase())
}

fn dots(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segs: Vec<&str> = path.split('/').collect();
    for (i, s) in segs.iter().enumerate() {
        match *s {
            "." => {
                if i + 1 == segs.len() {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if i + 1 == segs.len() {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if joined.starts_with('/') { joined } else { format!("/{joined}") }
}

/// Resolves `rel` against `base`. Only http(s) results come back; `data:`,
/// `blob:`, `javascript:` and the like are `None`.
pub fn join(base: &str, rel: &str) -> Option<String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return None;
    }
    if rel.len() > 8 && rel[..8].eq_ignore_ascii_case("https://") || rel.len() > 7 && rel[..7].eq_ignore_ascii_case("http://") {
        return is_http(rel).then(|| no_fragment(rel).to_owned());
    }
    // Some other scheme (blob:, data:, javascript:, skd:).
    if let Some(colon) = rel.find(':') {
        let head = &rel[..colon];
        if !head.is_empty() && head.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) && !head.contains('/') {
            return None;
        }
    }
    let (scheme, host, path) = split(base)?;
    if let Some(r) = rel.strip_prefix("//") {
        let r = no_fragment(r);
        return Some(format!("{scheme}://{r}"));
    }
    let rel = no_fragment(rel);
    if rel.starts_with('/') {
        let (p, q) = rel.split_once('?').map_or((rel, None), |(p, q)| (p, Some(q)));
        return Some(format!("{scheme}://{host}{}{}", dots(p), q.map(|q| format!("?{q}")).unwrap_or_default()));
    }
    let base_path = path.split('#').next().unwrap_or(path);
    if rel.starts_with('?') {
        let p = base_path.split('?').next().unwrap_or("");
        return Some(format!("{scheme}://{host}{}{rel}", if p.is_empty() { "/" } else { p }));
    }
    let base_noq = base_path.split('?').next().unwrap_or("");
    let dir = match base_noq.rfind('/') {
        Some(i) => &base_noq[..=i],
        None => "/",
    };
    let (p, q) = rel.split_once('?').map_or((rel, None), |(p, q)| (p, Some(q)));
    let joined = format!("{dir}{p}");
    Some(format!("{scheme}://{host}{}{}", dots(&joined), q.map(|q| format!("?{q}")).unwrap_or_default()))
}

/// Percent-decodes (`%20` and the like; `+` stays).
pub fn percent_decode(s: &str) -> String {
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
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_resolve() {
        let base = "https://cdn.example.com/a/b/index.m3u8?token=1";
        assert_eq!(join(base, "seg1.ts").unwrap(), "https://cdn.example.com/a/b/seg1.ts");
        assert_eq!(join(base, "../x/seg.ts?k=2").unwrap(), "https://cdn.example.com/a/x/seg.ts?k=2");
        assert_eq!(join(base, "/root.mp4").unwrap(), "https://cdn.example.com/root.mp4");
        assert_eq!(join(base, "//other.example.org/v.mp4#t=1").unwrap(), "https://other.example.org/v.mp4");
        assert_eq!(join(base, "http://x.org/v.mp4").unwrap(), "http://x.org/v.mp4");
        assert_eq!(join(base, "./v/../w.ts").unwrap(), "https://cdn.example.com/a/b/w.ts");
        assert_eq!(join(base, "?t=2").unwrap(), "https://cdn.example.com/a/b/index.m3u8?t=2");
        assert_eq!(join("https://example.com", "v.mp4").unwrap(), "https://example.com/v.mp4");
        assert!(join(base, "blob:https://x/abc").is_none());
        assert!(join(base, "data:video/mp4;base64,AAAA").is_none());
        assert!(join(base, "skd://key").is_none());
        assert!(join("not a url", "x.ts").is_none());
        assert_eq!(ext_of("https://e.com/a/b.MP4?x=1").as_deref(), Some("mp4"));
        assert_eq!(path_of("https://e.com/a/b.m3u8?x=1"), "/a/b.m3u8");
        assert_eq!(percent_decode("a%20b%2"), "a b%2");
    }
}
