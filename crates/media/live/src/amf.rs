// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AMF0, the small value format RTMP commands are written in.

#[derive(Clone, Debug, PartialEq)]
pub enum Amf {
    Num(f64),
    Bool(bool),
    Str(String),
    Obj(Vec<(String, Amf)>),
    Null,
    /// ECMA array (named values, as `onMetaData` uses).
    Map(Vec<(String, Amf)>),
}

impl Amf {
    pub fn str(s: &str) -> Amf {
        Amf::Str(s.into())
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Amf::Str(s) = self { Some(s) } else { None }
    }

    pub fn as_num(&self) -> Option<f64> {
        if let Amf::Num(n) = self { Some(*n) } else { None }
    }

    pub fn get(&self, key: &str) -> Option<&Amf> {
        match self {
            Amf::Obj(v) | Amf::Map(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Amf::Num(n) => {
                out.push(0x00);
                out.extend_from_slice(&n.to_be_bytes());
            }
            Amf::Bool(b) => out.extend_from_slice(&[0x01, *b as u8]),
            Amf::Str(s) => {
                out.push(0x02);
                out.extend_from_slice(&(s.len() as u16).to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Amf::Null => out.push(0x05),
            Amf::Obj(v) => {
                out.push(0x03);
                pairs(v, out);
            }
            Amf::Map(v) => {
                out.push(0x08);
                out.extend_from_slice(&(v.len() as u32).to_be_bytes());
                pairs(v, out);
            }
        }
    }

    /// One value from the front of `b`; the rest is returned.
    pub fn decode(b: &[u8]) -> Result<(Amf, &[u8]), String> {
        let (&t, r) = b.split_first().ok_or("AMF: ends early")?;
        match t {
            0x00 => {
                let n = r.get(..8).ok_or("AMF: short number")?;
                Ok((Amf::Num(f64::from_be_bytes(n.try_into().unwrap())), &r[8..]))
            }
            0x01 => Ok((Amf::Bool(*r.first().ok_or("AMF: short bool")? != 0), &r[1..])),
            0x02 => {
                let (s, rest) = string(r)?;
                Ok((Amf::Str(s), rest))
            }
            0x03 => {
                let (v, rest) = read_pairs(r)?;
                Ok((Amf::Obj(v), rest))
            }
            0x05 | 0x06 => Ok((Amf::Null, r)),
            0x08 => {
                let r = r.get(4..).ok_or("AMF: short map")?;
                let (v, rest) = read_pairs(r)?;
                Ok((Amf::Map(v), rest))
            }
            0x0a => {
                let n = u32::from_be_bytes(r.get(..4).ok_or("AMF: short array")?.try_into().unwrap());
                let mut rest = &r[4..];
                for _ in 0..n {
                    let (_, x) = Amf::decode(rest)?;
                    rest = x;
                }
                Ok((Amf::Null, rest))
            }
            t => Err(format!("AMF: unsupported type {t}")),
        }
    }

    /// Every value in `b`.
    pub fn decode_all(mut b: &[u8]) -> Result<Vec<Amf>, String> {
        let mut out = Vec::new();
        while !b.is_empty() {
            let (v, r) = Amf::decode(b)?;
            out.push(v);
            b = r;
        }
        Ok(out)
    }
}

fn pairs(v: &[(String, Amf)], out: &mut Vec<u8>) {
    for (k, x) in v {
        out.extend_from_slice(&(k.len() as u16).to_be_bytes());
        out.extend_from_slice(k.as_bytes());
        x.encode(out);
    }
    out.extend_from_slice(&[0, 0, 9]);
}

fn string(r: &[u8]) -> Result<(String, &[u8]), String> {
    let len = u16::from_be_bytes(r.get(..2).ok_or("AMF: short string")?.try_into().unwrap()) as usize;
    let s = r.get(2..2 + len).ok_or("AMF: short string")?;
    Ok((String::from_utf8_lossy(s).into_owned(), &r[2 + len..]))
}

fn read_pairs(mut r: &[u8]) -> Result<(Vec<(String, Amf)>, &[u8]), String> {
    let mut v = Vec::new();
    loop {
        if r.len() >= 3 && r[0] == 0 && r[1] == 0 && r[2] == 9 {
            return Ok((v, &r[3..]));
        }
        let (k, rest) = string(r)?;
        let (x, rest) = Amf::decode(rest)?;
        v.push((k, x));
        r = rest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let v = vec![Amf::str("connect"), Amf::Num(1.0), Amf::Obj(vec![("app".into(), Amf::str("live")), ("n".into(), Amf::Num(7.5)), ("ok".into(), Amf::Bool(true))]), Amf::Null, Amf::Map(vec![("w".into(), Amf::Num(1920.0))])];
        let mut b = Vec::new();
        for x in &v {
            x.encode(&mut b);
        }
        assert_eq!(Amf::decode_all(&b).unwrap(), v);
        assert_eq!(v[2].get("app").unwrap().as_str(), Some("live"));
        // Known bytes: number 1.0.
        let mut one = Vec::new();
        Amf::Num(1.0).encode(&mut one);
        assert_eq!(one, [0, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0]);
    }
}
