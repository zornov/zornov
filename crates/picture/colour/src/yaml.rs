// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The YAML subset OpenColorIO configs are written in: block mappings and
//! sequences by indentation, flow `{…}` / `[…]` collections, `!<Tag>`
//! tags, plain / single / double quoted scalars, `|` and `>` block
//! scalars and `#` comments. Anchors, aliases and multi-document streams
//! are not used by OCIO and are not supported.

/// A parsed node.
#[derive(Clone, Debug, PartialEq)]
pub enum Yaml {
    Null,
    Str(String),
    Seq(Vec<Yaml>),
    /// Keys in document order.
    Map(Vec<(String, Yaml)>),
    /// A `!<Tag>` and the node it applies to.
    Tagged(String, Box<Yaml>),
}

impl Yaml {
    /// The node without its tag.
    pub fn untagged(&self) -> &Yaml {
        match self {
            Yaml::Tagged(_, v) => v.untagged(),
            v => v,
        }
    }

    pub fn tag(&self) -> Option<&str> {
        match self {
            Yaml::Tagged(t, _) => Some(t),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self.untagged() {
            Yaml::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.untagged() {
            Yaml::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_str()?.trim().parse().ok()
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.as_str()?.trim() {
            "true" | "True" | "yes" => Some(true),
            "false" | "False" | "no" => Some(false),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> &[Yaml] {
        match self.untagged() {
            Yaml::Seq(s) => s,
            _ => &[],
        }
    }

    pub fn as_map(&self) -> &[(String, Yaml)] {
        match self.untagged() {
            Yaml::Map(m) => m,
            _ => &[],
        }
    }

    /// A number or a list of numbers.
    pub fn numbers(&self) -> Vec<f64> {
        match self.untagged() {
            Yaml::Seq(s) => s.iter().filter_map(Yaml::as_f64).collect(),
            v => v.as_f64().into_iter().collect(),
        }
    }

    /// A string or a list of strings.
    pub fn strings(&self) -> Vec<String> {
        match self.untagged() {
            Yaml::Seq(s) => s.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
            Yaml::Str(s) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    }
}

struct Line<'a> {
    indent: usize,
    text: &'a str,
    no: usize,
}

/// Parses a YAML document.
pub fn parse(text: &str) -> Result<Yaml, String> {
    let raw: Vec<&str> = text.lines().collect();
    let mut lines = Vec::new();
    for (i, l) in raw.iter().enumerate() {
        let body = strip_comment(l);
        if body.trim().is_empty() || body.trim() == "---" || body.trim() == "..." {
            lines.push(Line { indent: usize::MAX, text: "", no: i + 1 });
            continue;
        }
        let indent = body.len() - body.trim_start().len();
        lines.push(Line { indent, text: body.trim_end().trim_start(), no: i + 1 });
    }
    let mut p = Parser { lines, raw, pos: 0 };
    p.skip_blank();
    if p.pos >= p.lines.len() {
        return Ok(Yaml::Null);
    }
    let indent = p.lines[p.pos].indent;
    p.block(indent)
}

fn strip_comment(l: &str) -> &str {
    let (mut sq, mut dq, mut prev_space) = (false, false, true);
    for (i, c) in l.char_indices() {
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            '#' if !sq && !dq && prev_space => return &l[..i],
            _ => {}
        }
        prev_space = c.is_whitespace();
    }
    l
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    raw: Vec<&'a str>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_blank(&mut self) {
        while self.pos < self.lines.len() && self.lines[self.pos].indent == usize::MAX {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<(usize, &'a str)> {
        self.skip_blank();
        self.lines.get(self.pos).map(|l| (l.indent, l.text))
    }

    fn err(&self, m: &str) -> String {
        let no = self.lines.get(self.pos).map(|l| l.no).unwrap_or(self.raw.len());
        format!("line {no}: {m}")
    }

    fn block(&mut self, indent: usize) -> Result<Yaml, String> {
        let Some((_, text)) = self.peek() else { return Ok(Yaml::Null) };
        if text.starts_with('{') || text.starts_with('[') || text.starts_with("!<") {
            self.pos += 1;
            return self.item(text, indent, indent.saturating_sub(1));
        }
        if text == "-" || text.starts_with("- ") {
            self.seq(indent)
        } else {
            self.map(indent, None)
        }
    }

    fn seq(&mut self, indent: usize) -> Result<Yaml, String> {
        let mut items = Vec::new();
        while let Some((ind, text)) = self.peek() {
            if ind != indent || !(text == "-" || text.starts_with("- ")) {
                break;
            }
            let rest = text[1..].trim_start();
            let item_indent = indent + (text.len() - rest.len());
            self.pos += 1;
            items.push(self.item(rest, item_indent, indent)?);
        }
        Ok(Yaml::Seq(items))
    }

    /// The value written after `- ` (or after `key:`), possibly continued
    /// by a deeper block. `inline_indent` is the column `rest` starts at.
    fn item(&mut self, rest: &str, inline_indent: usize, parent: usize) -> Result<Yaml, String> {
        if rest.is_empty() {
            return self.nested(parent);
        }
        if let Some((tag, after)) = split_tag(rest) {
            let after = after.trim();
            let v = if after.is_empty() { self.nested(parent)? } else { self.item(after, inline_indent, parent)? };
            return Ok(Yaml::Tagged(tag.to_string(), Box::new(v)));
        }
        if rest.starts_with('{') || rest.starts_with('[') {
            return self.flow_multiline(rest);
        }
        if let Some((k, v)) = split_key(rest) {
            // A mapping that starts on the dash line.
            let first = self.value_after_key(v, inline_indent)?;
            return self.map(inline_indent, Some((k, first)));
        }
        if rest == "|" || rest == ">" || rest.starts_with("|-") || rest.starts_with("|+") || rest.starts_with(">-") {
            return Ok(Yaml::Str(self.block_scalar(parent, rest.starts_with('>'))));
        }
        Ok(Yaml::Str(scalar(rest)))
    }

    fn nested(&mut self, parent: usize) -> Result<Yaml, String> {
        match self.peek() {
            Some((ind, _)) if ind > parent => self.block(ind),
            _ => Ok(Yaml::Null),
        }
    }

    fn value_after_key(&mut self, v: &str, key_indent: usize) -> Result<Yaml, String> {
        if v.is_empty() {
            // "key:" then a deeper block, or a sequence at the same indent.
            return match self.peek() {
                Some((ind, _)) if ind > key_indent => self.block(ind),
                Some((ind, t)) if ind == key_indent && (t == "-" || t.starts_with("- ")) => self.seq(ind),
                _ => Ok(Yaml::Null),
            };
        }
        if v == "|" || v == ">" || v.starts_with("|-") || v.starts_with("|+") || v.starts_with(">-") {
            return Ok(Yaml::Str(self.block_scalar(key_indent, v.starts_with('>'))));
        }
        self.item(v, key_indent + 2, key_indent)
    }

    fn map(&mut self, indent: usize, first: Option<(&str, Yaml)>) -> Result<Yaml, String> {
        let mut out = Vec::new();
        if let Some((k, v)) = first {
            out.push((k.to_string(), v));
        }
        while let Some((ind, text)) = self.peek() {
            if ind != indent || text.starts_with("- ") || text == "-" {
                break;
            }
            let Some((k, v)) = split_key(text) else { return Err(self.err(&format!("expected \"key: value\", found {text:?}"))) };
            self.pos += 1;
            let val = self.value_after_key(v, indent)?;
            out.push((k.to_string(), val));
        }
        Ok(Yaml::Map(out))
    }

    fn block_scalar(&mut self, parent: usize, folded: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut base = None;
        while self.pos < self.lines.len() {
            let raw = self.raw[self.lines[self.pos].no - 1];
            let ind = raw.len() - raw.trim_start().len();
            if !raw.trim().is_empty() && ind <= parent {
                break;
            }
            let b = *base.get_or_insert(if raw.trim().is_empty() { usize::MAX } else { ind });
            parts.push(if raw.len() > b { raw[b.min(raw.len())..].to_string() } else { String::new() });
            self.pos += 1;
        }
        while parts.last().is_some_and(|p| p.is_empty()) {
            parts.pop();
        }
        parts.join(if folded { " " } else { "\n" })
    }

    /// A flow collection, possibly spread over several lines.
    fn flow_multiline(&mut self, first: &str) -> Result<Yaml, String> {
        let mut text = first.to_string();
        loop {
            let mut f = Flow { s: text.as_bytes(), i: 0 };
            match f.value() {
                Ok(v) => return Ok(v),
                Err(e) if e == "eof" && self.pos < self.lines.len() => {
                    self.skip_blank();
                    if let Some(l) = self.lines.get(self.pos) {
                        text.push(' ');
                        text.push_str(l.text);
                        self.pos += 1;
                    } else {
                        return Err(self.err("unclosed flow collection"));
                    }
                }
                Err(e) => return Err(self.err(&e)),
            }
        }
    }
}

fn split_tag(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("!<")?;
    let end = rest.find('>')?;
    Some((&rest[..end], &rest[end + 1..]))
}

/// `key: value` outside quotes and brackets; returns the key unquoted.
fn split_key(s: &str) -> Option<(&str, &str)> {
    if s.starts_with('{') || s.starts_with('[') || s.starts_with("!<") {
        return None;
    }
    let bytes = s.as_bytes();
    let (mut sq, mut dq) = (false, false);
    for i in 0..bytes.len() {
        match bytes[i] {
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b':' if !sq && !dq && (i + 1 == bytes.len() || bytes[i + 1] == b' ') => {
                let k = s[..i].trim();
                let k = k.trim_matches('"').trim_matches('\'');
                return Some((k, s[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

fn scalar(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return unescape(&s[1..s.len() - 1]);
    }
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return s[1..s.len() - 1].replace("''", "'");
    }
    s.to_string()
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

struct Flow<'a> {
    s: &'a [u8],
    i: usize,
}

impl Flow<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<Yaml, String> {
        self.ws();
        if self.i >= self.s.len() {
            return Err("eof".into());
        }
        if self.s[self.i..].starts_with(b"!<") {
            let end = self.s[self.i..].iter().position(|b| *b == b'>').ok_or("unclosed tag")? + self.i;
            let tag = String::from_utf8_lossy(&self.s[self.i + 2..end]).into_owned();
            self.i = end + 1;
            let v = self.value()?;
            return Ok(Yaml::Tagged(tag, Box::new(v)));
        }
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                loop {
                    self.ws();
                    if self.i >= self.s.len() {
                        return Err("eof".into());
                    }
                    if self.s[self.i] == b'}' {
                        self.i += 1;
                        return Ok(Yaml::Map(m));
                    }
                    let key = self.plain(true)?;
                    self.ws();
                    if self.i < self.s.len() && self.s[self.i] == b':' {
                        self.i += 1;
                    }
                    self.ws();
                    let v = if self.i < self.s.len() && (self.s[self.i] == b',' || self.s[self.i] == b'}') { Yaml::Null } else { self.value()? };
                    m.push((key, v));
                    self.ws();
                    if self.i < self.s.len() && self.s[self.i] == b',' {
                        self.i += 1;
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.i >= self.s.len() {
                        return Err("eof".into());
                    }
                    if self.s[self.i] == b']' {
                        self.i += 1;
                        return Ok(Yaml::Seq(v));
                    }
                    v.push(self.value()?);
                    self.ws();
                    if self.i < self.s.len() && self.s[self.i] == b',' {
                        self.i += 1;
                    }
                }
            }
            _ => Ok(Yaml::Str(self.plain(false)?)),
        }
    }

    /// A scalar inside a flow collection: quoted, or plain up to `,` `}`
    /// `]` (and `:` followed by a space when it is a key).
    fn plain(&mut self, key: bool) -> Result<String, String> {
        self.ws();
        if self.i >= self.s.len() {
            return Err("eof".into());
        }
        let q = self.s[self.i];
        if q == b'"' || q == b'\'' {
            let start = self.i + 1;
            let mut j = start;
            while j < self.s.len() {
                if self.s[j] == b'\\' && q == b'"' {
                    j += 2;
                    continue;
                }
                if self.s[j] == q {
                    if q == b'\'' && j + 1 < self.s.len() && self.s[j + 1] == b'\'' {
                        j += 2;
                        continue;
                    }
                    break;
                }
                j += 1;
            }
            if j >= self.s.len() {
                return Err("eof".into());
            }
            self.i = j + 1;
            let body = String::from_utf8_lossy(&self.s[start..j]).into_owned();
            return Ok(if q == b'"' { unescape(&body) } else { body.replace("''", "'") });
        }
        let start = self.i;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c == b',' || c == b'}' || c == b']' {
                break;
            }
            if key && c == b':' && (self.i + 1 >= self.s.len() || self.s[self.i + 1] == b' ') {
                break;
            }
            self.i += 1;
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.i]).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocio_shapes() {
        let doc = r#"
ocio_profile_version: 2   # a comment
name: "test config"
roles:
  scene_linear: ACEScg
  color_timing: ACEScct
description: |
  Line one
  Line two
displays:
  sRGB - Display:
    - !<View> {name: Raw, colorspace: Raw}
    - !<Views> [ACES 1.0 - SDR Video, Un-tone-mapped]
colorspaces:
  - !<ColorSpace>
    name: ACEScg
    aliases: [lin_ap1, "ACES - ACEScg"]
    to_scene_reference: !<BuiltinTransform> {style: ACEScg_to_ACES2065-1}
  - !<ColorSpace>
    name: Group
    from_scene_reference: !<GroupTransform>
      children:
        - !<MatrixTransform> {matrix: [1, 0, 0, 0,
                                       0, 1, 0, 0,
                                       0, 0, 1, 0, 0, 0, 0, 1]}
        - !<ExponentWithLinearTransform> {gamma: 2.4, offset: 0.055, direction: inverse}
"#;
        let y = parse(doc).unwrap();
        assert_eq!(y.get("ocio_profile_version").unwrap().as_f64(), Some(2.0));
        assert_eq!(y.get("name").unwrap().as_str(), Some("test config"));
        assert_eq!(y.get("roles").unwrap().get("color_timing").unwrap().as_str(), Some("ACEScct"));
        assert_eq!(y.get("description").unwrap().as_str(), Some("Line one\nLine two"));
        let d = y.get("displays").unwrap().get("sRGB - Display").unwrap().as_seq();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].tag(), Some("View"));
        assert_eq!(d[0].get("colorspace").unwrap().as_str(), Some("Raw"));
        assert_eq!(d[1].strings(), vec!["ACES 1.0 - SDR Video".to_string(), "Un-tone-mapped".to_string()]);
        let cs = y.get("colorspaces").unwrap().as_seq();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].tag(), Some("ColorSpace"));
        assert_eq!(cs[0].get("aliases").unwrap().strings()[1], "ACES - ACEScg");
        let t = cs[0].get("to_scene_reference").unwrap();
        assert_eq!(t.tag(), Some("BuiltinTransform"));
        assert_eq!(t.get("style").unwrap().as_str(), Some("ACEScg_to_ACES2065-1"));
        let kids = cs[1].get("from_scene_reference").unwrap().get("children").unwrap().as_seq();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].get("matrix").unwrap().numbers().len(), 16);
        assert_eq!(kids[1].get("direction").unwrap().as_str(), Some("inverse"));
    }

    #[test]
    fn v1_style() {
        let doc = "colorspaces:\n- !<ColorSpace>\n  name: linear\n  to_reference: !<FileTransform> {src: a b.spi1d, interpolation: linear}\n  isdata: false\n";
        let y = parse(doc).unwrap();
        let cs = y.get("colorspaces").unwrap().as_seq();
        assert_eq!(cs[0].get("name").unwrap().as_str(), Some("linear"));
        assert_eq!(cs[0].get("to_reference").unwrap().get("src").unwrap().as_str(), Some("a b.spi1d"));
        assert_eq!(cs[0].get("isdata").unwrap().as_bool(), Some(false));
    }
}
