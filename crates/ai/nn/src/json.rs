// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal JSON parser (enough for safetensors headers and config files).

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(kk, _)| kk == k).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }
}

pub fn parse(s: &str) -> Result<Json> {
    let b = s.as_bytes();
    let mut p = 0;
    let v = value(b, &mut p, 0)?;
    ws(b, &mut p);
    if p != b.len() {
        return Err(Error(format!("json: trailing data at {p}")));
    }
    Ok(v)
}

fn ws(b: &[u8], p: &mut usize) {
    while *p < b.len() && matches!(b[*p], b' ' | b'\t' | b'\n' | b'\r') {
        *p += 1;
    }
}

fn value(b: &[u8], p: &mut usize, depth: usize) -> Result<Json> {
    if depth > 128 {
        return Err(Error("json: nesting too deep".into()));
    }
    ws(b, p);
    let Some(&c) = b.get(*p) else { return Err(Error("json: unexpected end".into())) };
    match c {
        b'{' => {
            *p += 1;
            let mut items = Vec::new();
            ws(b, p);
            if b.get(*p) == Some(&b'}') {
                *p += 1;
                return Ok(Json::Obj(items));
            }
            loop {
                ws(b, p);
                let k = string(b, p)?;
                ws(b, p);
                if b.get(*p) != Some(&b':') {
                    return Err(Error(format!("json: expected ':' at {p}", p = *p)));
                }
                *p += 1;
                let v = value(b, p, depth + 1)?;
                items.push((k, v));
                ws(b, p);
                match b.get(*p) {
                    Some(b',') => *p += 1,
                    Some(b'}') => {
                        *p += 1;
                        return Ok(Json::Obj(items));
                    }
                    _ => return Err(Error(format!("json: expected ',' or '}}' at {}", *p))),
                }
            }
        }
        b'[' => {
            *p += 1;
            let mut items = Vec::new();
            ws(b, p);
            if b.get(*p) == Some(&b']') {
                *p += 1;
                return Ok(Json::Arr(items));
            }
            loop {
                items.push(value(b, p, depth + 1)?);
                ws(b, p);
                match b.get(*p) {
                    Some(b',') => *p += 1,
                    Some(b']') => {
                        *p += 1;
                        return Ok(Json::Arr(items));
                    }
                    _ => return Err(Error(format!("json: expected ',' or ']' at {}", *p))),
                }
            }
        }
        b'"' => Ok(Json::Str(string(b, p)?)),
        b't' if b[*p..].starts_with(b"true") => {
            *p += 4;
            Ok(Json::Bool(true))
        }
        b'f' if b[*p..].starts_with(b"false") => {
            *p += 5;
            Ok(Json::Bool(false))
        }
        b'n' if b[*p..].starts_with(b"null") => {
            *p += 4;
            Ok(Json::Null)
        }
        _ => {
            let start = *p;
            while *p < b.len() && matches!(b[*p], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *p += 1;
            }
            let t = std::str::from_utf8(&b[start..*p]).unwrap_or("");
            t.parse::<f64>().map(Json::Num).map_err(|_| Error(format!("json: bad value at {start}")))
        }
    }
}

fn string(b: &[u8], p: &mut usize) -> Result<String> {
    if b.get(*p) != Some(&b'"') {
        return Err(Error(format!("json: expected string at {}", *p)));
    }
    *p += 1;
    let mut out: Vec<u8> = Vec::new();
    loop {
        let Some(&c) = b.get(*p) else { return Err(Error("json: unterminated string".into())) };
        *p += 1;
        match c {
            b'"' => break,
            b'\\' => {
                let Some(&e) = b.get(*p) else { return Err(Error("json: bad escape".into())) };
                *p += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'u' => {
                        let hex = std::str::from_utf8(b.get(*p..*p + 4).unwrap_or(b"")).unwrap_or("");
                        let mut cp = u32::from_str_radix(hex, 16).map_err(|_| Error("json: bad \\u escape".into()))?;
                        *p += 4;
                        if (0xd800..0xdc00).contains(&cp) && b.get(*p) == Some(&b'\\') && b.get(*p + 1) == Some(&b'u') {
                            let hex2 = std::str::from_utf8(b.get(*p + 2..*p + 6).unwrap_or(b"")).unwrap_or("");
                            if let Ok(lo) = u32::from_str_radix(hex2, 16) {
                                cp = 0x10000 + ((cp - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff);
                                *p += 6;
                            }
                        }
                        let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    other => out.push(other),
                }
            }
            _ => out.push(c),
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        let v = super::parse(r#"{"a": [1, 2.5e1, -3], "b": {"c": "x\"yA"}, "d": true, "e": null}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_arr().unwrap()[1].as_f64(), Some(25.0));
        assert_eq!(v.get("b").unwrap().get("c").unwrap().as_str(), Some("x\"yA"));
    }
}
