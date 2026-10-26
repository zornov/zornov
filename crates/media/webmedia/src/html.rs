// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A forgiving HTML tokenizer: tags with their attributes, closing tags and
//! the raw text of `<script>`, `<style>`, `<title>` and `<textarea>`. It is
//! not a parser; it never fails, whatever the page looks like.

#[derive(Debug, Clone, Default)]
pub struct Tag {
    /// Lower-case element name.
    pub name: String,
    /// Lower-case attribute names with entity-decoded values.
    pub attrs: Vec<(String, String)>,
}

impl Tag {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    /// The first non-empty of several attributes.
    pub fn first(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|k| self.get(k).filter(|v| !v.trim().is_empty()))
    }
}

#[derive(Debug, Clone)]
pub enum Tok {
    Open(Tag),
    Close(String),
    /// The raw text inside a raw-text element, with that element's tag.
    Raw(Tag, String),
}

const RAW: &[&str] = &["script", "style", "title", "textarea"];

/// `&amp;` and friends, numeric references included.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // The name is at most 11 bytes; the cut must not fall inside a
        // multi-byte character (binary data read as text has plenty).
        let mut cut = rest.len().min(12);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let Some(semi) = rest[..cut].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
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

/// Parses `a=1 b="2" c='3' d` (the inside of a tag after its name).
pub fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' && b[i] != b'>' {
            i += 1;
        }
        if i == ns {
            i += 1;
            continue;
        }
        let name = s[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                val = decode_entities(&s[vs..i]);
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                val = decode_entities(&s[vs..i]);
            }
        }
        out.push((name, val));
    }
    out
}

/// The index of the `>` that ends the tag starting at `start` (a `<`),
/// skipping quoted attribute values.
pub fn tag_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = start + 1;
    let mut quote = 0u8;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if (c == b'"' || c == b'\'') && i > 0 && (b[i - 1] == b'=' || b[i - 1].is_ascii_whitespace()) {
            quote = c;
        } else if c == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn tokenize(html: &str) -> Vec<Tok> {
    let lower = html.to_ascii_lowercase();
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let Some(off) = html[i..].find('<') else { break };
        i += off;
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            i = match html[i + 4..].find("-->") {
                Some(e) => i + 4 + e + 3,
                None => break,
            };
            continue;
        }
        let next = b.get(i + 1).copied().unwrap_or(0);
        if next == b'/' {
            let Some(e) = tag_end(html, i) else { break };
            let name = html[i + 2..e].trim().split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
            if !name.is_empty() {
                out.push(Tok::Close(name));
            }
            i = e + 1;
            continue;
        }
        if !next.is_ascii_alphabetic() {
            // "<!doctype", "<?xml", a stray "<".
            i = match tag_end(html, i) {
                Some(e) if next == b'!' || next == b'?' => e + 1,
                _ => i + 1,
            };
            continue;
        }
        let Some(e) = tag_end(html, i) else { break };
        let inner = &html[i + 1..e];
        let name_end = inner.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        let tag = Tag { name: name.clone(), attrs: parse_attrs(&inner[name_end..]) };
        i = e + 1;
        if RAW.contains(&name.as_str()) && !inner.ends_with('/') {
            let close = format!("</{name}");
            let end = lower[i..].find(&close).map_or(html.len(), |x| i + x);
            let text = html[i..end].to_owned();
            out.push(Tok::Open(tag.clone()));
            out.push(Tok::Raw(tag, text));
            i = end;
        } else {
            out.push(Tok::Open(tag));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Binary data read as text (a model file mistaken for a page) must not
    /// cut a multi-byte character in two.
    #[test]
    fn entities_survive_binary_text() {
        let s = format!("ab&{}", "\u{fffd}".repeat(8));
        assert_eq!(super::decode_entities(&s), s);
        assert_eq!(super::decode_entities("a &amp; b"), "a & b");
    }

    use super::*;

    #[test]
    fn tags_attributes_and_raw_text() {
        let toks = tokenize(r#"<!doctype html><html><head><title>A &amp; B</title><meta property="og:video" content='https://x.test/v.mp4?a=1&amp;b=2'><script type="application/ld+json">{"a":"<b>"}</script></head><body><video src=a.mp4 controls><source src="b.m3u8" type="application/x-mpegURL"/></video><!-- <video src="no.mp4"> --></body>"#);
        let opens: Vec<&Tag> = toks.iter().filter_map(|t| if let Tok::Open(t) = t { Some(t) } else { None }).collect();
        let meta = opens.iter().find(|t| t.name == "meta").unwrap();
        assert_eq!(meta.get("content"), Some("https://x.test/v.mp4?a=1&b=2"));
        let video = opens.iter().find(|t| t.name == "video").unwrap();
        assert_eq!(video.get("src"), Some("a.mp4"));
        assert!(video.get("controls").is_some());
        assert_eq!(opens.iter().filter(|t| t.name == "video").count(), 1);
        let raws: Vec<&String> = toks.iter().filter_map(|t| if let Tok::Raw(_, s) = t { Some(s) } else { None }).collect();
        assert_eq!(raws[0], "A &amp; B");
        assert_eq!(raws[1], r#"{"a":"<b>"}"#);
        let source = opens.iter().find(|t| t.name == "source").unwrap();
        assert_eq!(source.get("type"), Some("application/x-mpegURL"));
        assert_eq!(decode_entities("a&#38;b&#x26;c&bogus;d"), "a&b&c&bogus;d");
    }
}
