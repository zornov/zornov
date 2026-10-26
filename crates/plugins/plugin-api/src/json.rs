// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small JSON reader and writer, so plugins need no other crates. The
//! host hands panels and exporters the project as JSON and reads manifests,
//! panel views and edit ops back as JSON.

use std::collections::BTreeMap;
use std::fmt::Write;

/// A JSON value. Objects keep their keys sorted.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    /// An empty object, for building with [`Json::with`].
    pub fn obj() -> Json {
        Json::Obj(BTreeMap::new())
    }

    /// Adds a field to an object (no effect on other values).
    pub fn with(mut self, key: &str, v: impl Into<Json>) -> Json {
        if let Json::Obj(m) = &mut self {
            m.insert(key.to_owned(), v.into());
        }
        self
    }

    /// A field of an object, or `Null`.
    pub fn get(&self, key: &str) -> &Json {
        static NULL: Json = Json::Null;
        match self {
            Json::Obj(m) => m.get(key).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// An element of an array, or `Null`.
    pub fn at(&self, i: usize) -> &Json {
        static NULL: Json = Json::Null;
        match self {
            Json::Arr(a) => a.get(i).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        if let Json::Num(n) = self { Some(*n) } else { None }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self { Some(s) } else { None }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let Json::Bool(b) = self { Some(*b) } else { None }
    }

    /// The elements of an array (empty for anything else).
    pub fn items(&self) -> &[Json] {
        if let Json::Arr(a) = self { a } else { &[] }
    }

    /// Parses a JSON document.
    pub fn parse(s: &str) -> Result<Json, String> {
        let mut p = Parser { b: s.as_bytes(), i: 0 };
        let v = p.value(0)?;
        p.ws();
        if p.i != p.b.len() {
            return Err(format!("unexpected text at byte {}", p.i));
        }
        Ok(v)
    }

    /// Writes compact JSON.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) if n.is_finite() => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Json::Num(_) => out.push_str("null"),
            Json::Str(s) => quote(s, out),
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<f64> for Json {
    fn from(v: f64) -> Self {
        Json::Num(v)
    }
}
impl From<f32> for Json {
    fn from(v: f32) -> Self {
        Json::Num(v as f64)
    }
}
impl From<i32> for Json {
    fn from(v: i32) -> Self {
        Json::Num(v as f64)
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Self {
        Json::Num(v as f64)
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Self {
        Json::Num(v as f64)
    }
}
impl From<bool> for Json {
    fn from(v: bool) -> Self {
        Json::Bool(v)
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Json::Str(v.to_owned())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Self {
        Json::Str(v)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Self {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Json> + Copy, const N: usize> From<[T; N]> for Json {
    fn from(v: [T; N]) -> Self {
        Json::Arr(v.iter().map(|x| (*x).into()).collect())
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 128 {
            return Err("nested too deeply".into());
        }
        self.ws();
        let Some(&c) = self.b.get(self.i) else { return Err("unexpected end".into()) };
        match c {
            b'{' => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    let v = self.value(depth + 1)?;
                    m.insert(k, v);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return Err(format!("expected , or }} at byte {}", self.i)),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(format!("expected , or ] at byte {}", self.i)),
                    }
                }
            }
            b'"' => Ok(Json::Str(self.string()?)),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            _ => self.number(),
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(format!("unexpected text at byte {}", self.i))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|e| e.to_string())?;
        s.parse::<f64>().map(Json::Num).map_err(|_| format!("bad number at byte {start}"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).map_err(|e| e.to_string())?, 16).map_err(|e| e.to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.b[start..self.i]).map_err(|e| e.to_string())?);
            match self.b.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = *self.b.get(self.i).ok_or("unexpected end")?;
                    self.i += 1;
                    match e {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let mut c = self.hex4()?;
                            if (0xD800..0xDC00).contains(&c) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                c = 0x10000 + ((c - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                        }
                        other => out.push(other as char),
                    }
                }
                _ => return Err("unterminated string".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let src = r#"{"a":[1,2.5,-3e2,true,false,null],"b":"x\"y\né😀","c":{}}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("a").at(1).as_f64(), Some(2.5));
        assert_eq!(v.get("b").as_str(), Some("x\"y\né😀"));
        assert_eq!(Json::parse(&v.to_json()).unwrap(), v);
        assert!(Json::parse("[1,").is_err());
        assert!(Json::parse("{} x").is_err());
    }
}
