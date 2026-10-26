// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small XML reader for DASH manifests: elements, attributes and text,
//! with namespace prefixes dropped.

use crate::html::{decode_entities, parse_attrs, tag_end};

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    pub text: String,
}

fn local(n: &str) -> &str {
    n.rsplit(':').next().unwrap_or(n)
}

impl Node {
    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    pub fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    pub fn kid(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
}

pub fn parse(s: &str) -> Result<Node, String> {
    let b = s.as_bytes();
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            let end = s[i..].find('<').map_or(s.len(), |x| i + x);
            if let Some(top) = stack.last_mut() {
                let t = &s[i..end];
                if !t.trim().is_empty() {
                    top.text.push_str(&decode_entities(t.trim()));
                }
            }
            i = end;
            continue;
        }
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            i = s[i + 4..].find("-->").map(|e| i + 4 + e + 3).ok_or("unterminated comment")?;
        } else if rest.starts_with("<![CDATA[") {
            let e = s[i..].find("]]>").ok_or("unterminated CDATA")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&s[i + 9..i + e]);
            }
            i += e + 3;
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            i = tag_end(s, i).ok_or("unterminated declaration")? + 1;
        } else if rest.starts_with("</") {
            let e = tag_end(s, i).ok_or("unterminated tag")?;
            let node = stack.pop().ok_or("closing tag without an opening one")?;
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => root = Some(node),
            }
            i = e + 1;
        } else {
            let e = tag_end(s, i).ok_or("unterminated tag")?;
            let self_closing = s[..e].ends_with('/');
            let inner = &s[i + 1..if self_closing { e - 1 } else { e }];
            let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
            let node = Node { name: local(&inner[..name_end]).to_owned(), attrs: parse_attrs_cased(&inner[name_end..]), children: Vec::new(), text: String::new() };
            if self_closing {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            } else {
                stack.push(node);
            }
            i = e + 1;
        }
    }
    if !stack.is_empty() {
        // A truncated manifest: close what is open.
        while let Some(node) = stack.pop() {
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => root = Some(node),
            }
        }
    }
    root.ok_or_else(|| "no elements".to_owned())
}

/// Attributes with their case kept (XML is case-sensitive) and prefixes
/// dropped; namespace declarations left out.
fn parse_attrs_cased(s: &str) -> Vec<(String, String)> {
    let lowered = parse_attrs(s);
    // Re-scan for the original spelling of each name, in order.
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() && n < lowered.len() {
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
        let raw = &s[ns..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                i += 1;
            } else {
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
            }
        }
        let (_, v) = &lowered[n];
        n += 1;
        if raw == "xmlns" || raw.starts_with("xmlns:") {
            continue;
        }
        out.push((local(raw).to_owned(), v.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_attributes_text() {
        let n = parse(r#"<?xml version="1.0"?><!-- c --><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT10S"><Period id="1"><BaseURL>http://a/&amp;b/</BaseURL><AdaptationSet mimeType="video/mp4"><Representation id="v1" bandwidth="100"/></AdaptationSet></Period></MPD>"#).unwrap();
        assert_eq!(n.name, "MPD");
        assert_eq!(n.attr("mediaPresentationDuration"), Some("PT10S"));
        assert_eq!(n.attr("xmlns"), None);
        let p = n.kid("Period").unwrap();
        assert_eq!(p.kid("BaseURL").unwrap().text, "http://a/&b/");
        let r = p.kid("AdaptationSet").unwrap().kid("Representation").unwrap();
        assert_eq!(r.attr("bandwidth"), Some("100"));
        assert!(parse("just text").is_err());
    }
}
