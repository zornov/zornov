// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! safetensors loader: 8-byte little-endian header length, a JSON header
//! mapping names to `{dtype, shape, data_offsets}`, then the byte buffer.

use crate::error::{Error, Result};
use crate::json::{self, Json};
use crate::tensor::{Data, Tensor, bf16_to_f32};
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct SafeTensors {
    pub tensors: Vec<(String, Tensor)>,
    pub metadata: Vec<(String, String)>,
}

impl SafeTensors {
    pub fn get(&self, name: &str) -> Option<&Tensor> {
        self.tensors.iter().find(|(n, _)| n == name).map(|(_, t)| t)
    }
}

pub fn load(path: impl AsRef<Path>) -> Result<SafeTensors> {
    let bytes = std::fs::read(path.as_ref()).map_err(|e| Error(format!("{}: {e}", path.as_ref().display())))?;
    from_bytes(&bytes)
}

pub fn from_bytes(bytes: &[u8]) -> Result<SafeTensors> {
    if bytes.len() < 8 {
        return Err(Error("safetensors: file too short".into()));
    }
    let hlen = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
    if hlen > 100 << 20 || 8 + hlen > bytes.len() {
        return Err(Error("safetensors: bad header length".into()));
    }
    let header = std::str::from_utf8(&bytes[8..8 + hlen]).map_err(|_| Error("safetensors: header is not UTF-8".into()))?;
    let j = json::parse(header.trim_end_matches(' '))?;
    let Json::Obj(entries) = j else { return Err(Error("safetensors: header is not an object".into())) };
    let buf = &bytes[8 + hlen..];
    let mut out = SafeTensors::default();
    for (name, v) in entries {
        if name == "__metadata__" {
            if let Json::Obj(m) = v {
                for (k, v) in m {
                    out.metadata.push((k, v.as_str().unwrap_or("").to_string()));
                }
            }
            continue;
        }
        let dtype = v.get("dtype").and_then(|d| d.as_str()).ok_or_else(|| Error(format!("safetensors: {name} has no dtype")))?;
        let shape: Vec<usize> = v.get("shape").and_then(|s| s.as_arr()).ok_or_else(|| Error(format!("safetensors: {name} has no shape")))?.iter().map(|d| d.as_f64().unwrap_or(0.0) as usize).collect();
        let offs = v.get("data_offsets").and_then(|s| s.as_arr()).ok_or_else(|| Error(format!("safetensors: {name} has no data_offsets")))?;
        if offs.len() != 2 {
            return Err(Error(format!("safetensors: {name}: bad data_offsets")));
        }
        let (a, b) = (offs[0].as_f64().unwrap_or(0.0) as usize, offs[1].as_f64().unwrap_or(0.0) as usize);
        if a > b || b > buf.len() {
            return Err(Error(format!("safetensors: {name}: data_offsets out of range")));
        }
        let raw = &buf[a..b];
        let n: usize = shape.iter().product();
        let size = match dtype {
            "F64" | "I64" | "U64" => 8,
            "F32" | "I32" | "U32" => 4,
            "F16" | "BF16" | "I16" | "U16" => 2,
            "I8" | "U8" | "BOOL" => 1,
            other => return Err(Error(format!("safetensors: {name}: unsupported dtype {other}"))),
        };
        if raw.len() != n * size {
            return Err(Error(format!("safetensors: {name}: {} bytes for {n} elements of {dtype}", raw.len())));
        }
        let c2 = || raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
        let c4 = || raw.chunks_exact(4).map(|c| <[u8; 4]>::try_from(c).unwrap());
        let c8 = || raw.chunks_exact(8).map(|c| <[u8; 8]>::try_from(c).unwrap());
        let data = match dtype {
            "F32" => Data::F32(c4().map(f32::from_le_bytes).collect()),
            "F64" => Data::F32(c8().map(|b| f64::from_le_bytes(b) as f32).collect()),
            "F16" => Data::F16(c2().collect()),
            "BF16" => Data::F32(c2().map(bf16_to_f32).collect()),
            "I64" => Data::I64(c8().map(i64::from_le_bytes).collect()),
            "U64" => Data::I64(c8().map(|b| u64::from_le_bytes(b) as i64).collect()),
            "I32" => Data::I64(c4().map(|b| i32::from_le_bytes(b) as i64).collect()),
            "U32" => Data::I64(c4().map(|b| u32::from_le_bytes(b) as i64).collect()),
            "I16" => Data::I64(c2().map(|v| v as i16 as i64).collect()),
            "U16" => Data::I64(c2().map(|v| v as i64).collect()),
            "I8" => Data::I8(raw.iter().map(|&b| b as i8).collect()),
            "U8" => Data::U8(raw.to_vec()),
            _ => Data::Bool(raw.iter().map(|&b| b != 0).collect()),
        };
        out.tensors.push((name, Tensor { shape, data }));
    }
    Ok(out)
}

/// Writes tensors as a safetensors file (f32/f16/i64/i8/u8/bool).
pub fn to_bytes(tensors: &[(&str, &Tensor)]) -> Vec<u8> {
    let mut header = String::from("{");
    let mut body = Vec::new();
    for (i, (name, t)) in tensors.iter().enumerate() {
        let raw = crate::onnx::tensor_raw_bytes(t);
        let dt = match t.dtype() {
            crate::DType::F32 => "F32",
            crate::DType::F16 => "F16",
            crate::DType::I64 => "I64",
            crate::DType::I8 => "I8",
            crate::DType::U8 => "U8",
            crate::DType::Bool => "BOOL",
        };
        if i > 0 {
            header.push(',');
        }
        let shape: Vec<String> = t.shape.iter().map(|d| d.to_string()).collect();
        header.push_str(&format!("\"{}\":{{\"dtype\":\"{dt}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}", name.replace('"', "\\\""), shape.join(","), body.len(), body.len() + raw.len()));
        body.extend_from_slice(&raw);
    }
    header.push('}');
    while header.len() % 8 != 0 {
        header.push(' ');
    }
    let mut out = (header.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let a = Tensor::f32(&[2, 2], vec![1.0, 2.0, 3.0, 4.0]);
        let b = Tensor::f16_from_f32(&[3], &[0.5, -1.0, 2.0]);
        let c = Tensor::i64(&[1], vec![-7]);
        let bytes = to_bytes(&[("a", &a), ("b.weight", &b), ("c", &c)]);
        let st = from_bytes(&bytes).unwrap();
        assert_eq!(st.get("a"), Some(&a));
        assert_eq!(st.get("b.weight"), Some(&b));
        assert_eq!(st.get("c"), Some(&c));
    }
}
