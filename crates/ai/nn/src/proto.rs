// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Protocol-buffers wire format: a zero-copy field reader and a writer.
//!
//! Only what ONNX needs: varints (wire type 0), 64-bit (1), length-delimited
//! (2) and 32-bit (5). Groups (3/4) are rejected.

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Wire<'a> {
    pub fn as_u64(&self) -> Result<u64> {
        match *self {
            Wire::Varint(v) | Wire::Fixed64(v) => Ok(v),
            Wire::Fixed32(v) => Ok(v as u64),
            Wire::Bytes(_) => Err(Error("protobuf: expected a scalar, got bytes".into())),
        }
    }
    pub fn as_i64(&self) -> Result<i64> {
        self.as_u64().map(|v| v as i64)
    }
    pub fn as_f32(&self) -> Result<f32> {
        match *self {
            Wire::Fixed32(v) => Ok(f32::from_bits(v)),
            _ => Err(Error("protobuf: expected fixed32 float".into())),
        }
    }
    pub fn as_bytes(&self) -> Result<&'a [u8]> {
        match *self {
            Wire::Bytes(b) => Ok(b),
            _ => Err(Error("protobuf: expected length-delimited field".into())),
        }
    }
    pub fn as_str(&self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.as_bytes()?).into_owned())
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let Some(&b) = self.buf.get(self.pos) else {
                return Err(Error("protobuf: truncated varint".into()));
            };
            self.pos += 1;
            if shift < 64 {
                v |= ((b & 0x7f) as u64) << shift;
            }
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 70 {
                return Err(Error("protobuf: varint too long".into()));
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.buf.len() - self.pos {
            return Err(Error(format!("protobuf: field of {n} bytes overruns buffer")));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Next `(field number, value)`, or `None` at the end of the message.
    pub fn next(&mut self) -> Result<Option<(u32, Wire<'a>)>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = (key >> 3) as u32;
        let w = match key & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => {
                let b = self.take(8)?;
                Wire::Fixed64(u64::from_le_bytes(b.try_into().unwrap()))
            }
            2 => {
                let n = self.varint()? as usize;
                Wire::Bytes(self.take(n)?)
            }
            5 => {
                let b = self.take(4)?;
                Wire::Fixed32(u32::from_le_bytes(b.try_into().unwrap()))
            }
            t => return Err(Error(format!("protobuf: unsupported wire type {t} (field {field})"))),
        };
        Ok(Some((field, w)))
    }
}

/// Appends a repeated varint field that may be packed or unpacked.
pub fn push_varints(w: &Wire, out: &mut Vec<i64>) -> Result<()> {
    match w {
        Wire::Bytes(b) => {
            let mut r = Reader::new(b);
            while r.pos < b.len() {
                out.push(r.varint()? as i64);
            }
        }
        _ => out.push(w.as_i64()?),
    }
    Ok(())
}

/// Appends a repeated float field (packed or unpacked).
pub fn push_f32s(w: &Wire, out: &mut Vec<f32>) -> Result<()> {
    match w {
        Wire::Bytes(b) => {
            if b.len() % 4 != 0 {
                return Err(Error("protobuf: packed float length not a multiple of 4".into()));
            }
            out.extend(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())));
        }
        _ => out.push(w.as_f32()?),
    }
    Ok(())
}

/// Appends a repeated double field (packed or unpacked).
pub fn push_f64s(w: &Wire, out: &mut Vec<f64>) -> Result<()> {
    match w {
        Wire::Bytes(b) => {
            if b.len() % 8 != 0 {
                return Err(Error("protobuf: packed double length not a multiple of 8".into()));
            }
            out.extend(b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())));
        }
        Wire::Fixed64(v) => out.push(f64::from_bits(*v)),
        _ => return Err(Error("protobuf: expected double".into())),
    }
    Ok(())
}

/// Protobuf writer.
#[derive(Default, Clone)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn raw_varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                break;
            }
            self.buf.push(b | 0x80);
        }
    }
    fn key(&mut self, field: u32, wt: u32) {
        self.raw_varint(((field << 3) | wt) as u64);
    }
    pub fn varint(&mut self, field: u32, v: u64) {
        self.key(field, 0);
        self.raw_varint(v);
    }
    pub fn int(&mut self, field: u32, v: i64) {
        self.varint(field, v as u64);
    }
    pub fn fixed32(&mut self, field: u32, v: u32) {
        self.key(field, 5);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn float(&mut self, field: u32, v: f32) {
        self.fixed32(field, v.to_bits());
    }
    pub fn bytes(&mut self, field: u32, b: &[u8]) {
        self.key(field, 2);
        self.raw_varint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }
    pub fn string(&mut self, field: u32, s: &str) {
        self.bytes(field, s.as_bytes());
    }
    pub fn message(&mut self, field: u32, m: &Writer) {
        self.bytes(field, &m.buf);
    }
    pub fn packed_ints(&mut self, field: u32, v: &[i64]) {
        let mut w = Writer::new();
        for &x in v {
            w.raw_varint(x as u64);
        }
        self.bytes(field, &w.buf);
    }
    pub fn packed_floats(&mut self, field: u32, v: &[f32]) {
        let mut b = Vec::with_capacity(v.len() * 4);
        for &x in v {
            b.extend_from_slice(&x.to_le_bytes());
        }
        self.bytes(field, &b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_fields() {
        let mut w = Writer::new();
        w.int(1, -5);
        w.float(2, 1.5);
        w.string(3, "hello");
        w.packed_ints(4, &[1, 300, -1]);
        w.int(4, 7); // unpacked element of the same repeated field
        let mut r = Reader::new(&w.buf);
        let (f, v) = r.next().unwrap().unwrap();
        assert_eq!((f, v.as_i64().unwrap()), (1, -5));
        let (f, v) = r.next().unwrap().unwrap();
        assert_eq!((f, v.as_f32().unwrap()), (2, 1.5));
        let (f, v) = r.next().unwrap().unwrap();
        assert_eq!((f, v.as_str().unwrap().as_str()), (3, "hello"));
        let mut ints = Vec::new();
        while let Some((f, v)) = r.next().unwrap() {
            assert_eq!(f, 4);
            push_varints(&v, &mut ints).unwrap();
        }
        assert_eq!(ints, vec![1, 300, -1, 7]);
    }
    #[test]
    fn truncated_is_error() {
        let mut w = Writer::new();
        w.string(1, "abcdef");
        let b = &w.buf[..4];
        let mut r = Reader::new(b);
        assert!(r.next().is_err());
    }
}
