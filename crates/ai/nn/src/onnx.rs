// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ONNX `ModelProto` decoding and encoding on top of [`crate::proto`].
//!
//! Field numbers follow onnx/onnx.proto (IR version 10). Supported tensor
//! payloads: `raw_data`, `float_data`, `int32_data`, `int64_data`,
//! `double_data`, `uint64_data` and external data files
//! (`data_location = EXTERNAL`, keys `location` / `offset` / `length`).

use crate::error::{Error, Result};
use crate::model::Model;
use crate::proto::{Reader, Writer, push_f32s, push_f64s, push_varints};
use crate::tensor::{DType, Data, Tensor, bf16_to_f32};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpsetId {
    pub domain: String,
    pub version: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Dim {
    Value(i64),
    Param(String),
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValueInfo {
    pub name: String,
    /// ONNX element type (`TensorProto.DataType`), 0 if unknown/not a tensor.
    pub elem_type: i32,
    /// `None` when the shape is not given at all.
    pub shape: Option<Vec<Dim>>,
}

impl ValueInfo {
    pub fn dtype(&self) -> Option<DType> {
        DType::from_onnx(self.elem_type)
    }
    /// Static dims, `None` for symbolic/unknown ones.
    pub fn dims(&self) -> Option<Vec<Option<usize>>> {
        self.shape.as_ref().map(|s| s.iter().map(|d| if let Dim::Value(v) = d { if *v >= 0 { Some(*v as usize) } else { None } } else { None }).collect())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    Float(f32),
    Int(i64),
    Str(Vec<u8>),
    Tensor(Tensor),
    Graph(Box<Graph>),
    Floats(Vec<f32>),
    Ints(Vec<i64>),
    Strings(Vec<Vec<u8>>),
    Tensors(Vec<Tensor>),
    Graphs(Vec<Graph>),
    /// An attribute type we do not decode (sparse tensors, type protos).
    Unsupported(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: AttrValue,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub op_type: String,
    pub domain: String,
    /// Input value names; an empty string marks an omitted optional input.
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attrs: Vec<Attribute>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&AttrValue> {
        self.attrs.iter().find(|a| a.name == name).map(|a| &a.value)
    }
    pub fn has(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }
    pub fn attr_i(&self, name: &str, default: i64) -> i64 {
        match self.attr(name) {
            Some(AttrValue::Int(v)) => *v,
            Some(AttrValue::Float(v)) => *v as i64,
            _ => default,
        }
    }
    pub fn attr_f(&self, name: &str, default: f32) -> f32 {
        match self.attr(name) {
            Some(AttrValue::Float(v)) => *v,
            Some(AttrValue::Int(v)) => *v as f32,
            _ => default,
        }
    }
    pub fn attr_s(&self, name: &str) -> Option<String> {
        match self.attr(name) {
            Some(AttrValue::Str(v)) => Some(String::from_utf8_lossy(v).into_owned()),
            _ => None,
        }
    }
    pub fn attr_s_or(&self, name: &str, default: &str) -> String {
        self.attr_s(name).unwrap_or_else(|| default.to_string())
    }
    pub fn attr_ints(&self, name: &str) -> Option<Vec<i64>> {
        match self.attr(name) {
            Some(AttrValue::Ints(v)) => Some(v.clone()),
            Some(AttrValue::Int(v)) => Some(vec![*v]),
            _ => None,
        }
    }
    pub fn attr_floats(&self, name: &str) -> Option<Vec<f32>> {
        match self.attr(name) {
            Some(AttrValue::Floats(v)) => Some(v.clone()),
            Some(AttrValue::Float(v)) => Some(vec![*v]),
            _ => None,
        }
    }
    pub fn attr_strings(&self, name: &str) -> Option<Vec<String>> {
        match self.attr(name) {
            Some(AttrValue::Strings(v)) => Some(v.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect()),
            Some(AttrValue::Str(v)) => Some(vec![String::from_utf8_lossy(v).into_owned()]),
            _ => None,
        }
    }
    pub fn attr_t(&self, name: &str) -> Option<&Tensor> {
        match self.attr(name) {
            Some(AttrValue::Tensor(t)) => Some(t),
            _ => None,
        }
    }
    /// Input name `i` if present and non-empty.
    pub fn input(&self, i: usize) -> Option<&str> {
        self.inputs.get(i).map(|s| s.as_str()).filter(|s| !s.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub name: String,
    pub nodes: Vec<Node>,
    pub initializers: Vec<(String, Tensor)>,
    pub inputs: Vec<ValueInfo>,
    pub outputs: Vec<ValueInfo>,
    pub value_info: Vec<ValueInfo>,
    pub doc: String,
}

// ------------------------------------------------------------------ decoding

/// Where external-data files are looked up, plus an open-file cache.
pub struct LoadCtx {
    pub dir: Option<PathBuf>,
    files: HashMap<String, std::fs::File>,
}

impl LoadCtx {
    pub fn new(dir: Option<&Path>) -> Self {
        LoadCtx { dir: dir.map(|d| d.to_path_buf()), files: HashMap::new() }
    }

    fn read_external(&mut self, location: &str, offset: u64, length: Option<u64>) -> Result<Vec<u8>> {
        let Some(dir) = &self.dir else {
            return Err(Error(format!("tensor uses external data '{location}' but the model was loaded from bytes without a base directory")));
        };
        // Refuse paths escaping the model directory.
        let rel = Path::new(location);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(Error(format!("external data location '{location}' must be relative to the model directory")));
        }
        let path = dir.join(rel);
        if !self.files.contains_key(location) {
            let f = std::fs::File::open(&path).map_err(|e| Error(format!("external data {}: {e}", path.display())))?;
            self.files.insert(location.to_string(), f);
        }
        let f = self.files.get_mut(location).unwrap();
        let total = f.metadata()?.len();
        let len = match length {
            Some(l) => l,
            None => total.saturating_sub(offset),
        };
        if offset + len > total {
            return Err(Error(format!("external data {}: range {}..{} beyond file size {}", path.display(), offset, offset + len, total)));
        }
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
}

pub fn decode_model(bytes: &[u8], dir: Option<&Path>) -> Result<Model> {
    let mut ctx = LoadCtx::new(dir);
    let mut m = Model::default();
    let mut r = Reader::new(bytes);
    let mut have_graph = false;
    while let Some((f, w)) = r.next()? {
        match f {
            1 => m.ir_version = w.as_i64()?,
            2 => m.producer_name = w.as_str()?,
            3 => m.producer_version = w.as_str()?,
            4 => m.domain = w.as_str()?,
            5 => m.model_version = w.as_i64()?,
            6 => m.doc_string = w.as_str()?,
            7 => {
                m.graph = decode_graph(w.as_bytes()?, &mut ctx)?;
                have_graph = true;
            }
            8 => {
                let mut id = OpsetId::default();
                let mut rr = Reader::new(w.as_bytes()?);
                while let Some((f, w)) = rr.next()? {
                    match f {
                        1 => id.domain = w.as_str()?,
                        2 => id.version = w.as_i64()?,
                        _ => {}
                    }
                }
                m.opset_import.push(id);
            }
            14 => m.metadata.push(decode_kv(w.as_bytes()?)?),
            _ => {}
        }
    }
    if !have_graph {
        return Err(Error("ONNX: model has no graph (is this an ONNX file?)".into()));
    }
    Ok(m)
}

fn decode_kv(b: &[u8]) -> Result<(String, String)> {
    let mut r = Reader::new(b);
    let (mut k, mut v) = (String::new(), String::new());
    while let Some((f, w)) = r.next()? {
        match f {
            1 => k = w.as_str()?,
            2 => v = w.as_str()?,
            _ => {}
        }
    }
    Ok((k, v))
}

pub fn decode_graph(b: &[u8], ctx: &mut LoadCtx) -> Result<Graph> {
    let mut g = Graph::default();
    let mut r = Reader::new(b);
    while let Some((f, w)) = r.next()? {
        match f {
            1 => g.nodes.push(decode_node(w.as_bytes()?, ctx)?),
            2 => g.name = w.as_str()?,
            5 => g.initializers.push(decode_tensor(w.as_bytes()?, ctx)?),
            10 => g.doc = w.as_str()?,
            11 => g.inputs.push(decode_value_info(w.as_bytes()?)?),
            12 => g.outputs.push(decode_value_info(w.as_bytes()?)?),
            13 => g.value_info.push(decode_value_info(w.as_bytes()?)?),
            15 => return Err(Error("ONNX: sparse initializers are not supported".into())),
            _ => {}
        }
    }
    Ok(g)
}

fn decode_node(b: &[u8], ctx: &mut LoadCtx) -> Result<Node> {
    let mut n = Node::default();
    let mut r = Reader::new(b);
    while let Some((f, w)) = r.next()? {
        match f {
            1 => n.inputs.push(w.as_str()?),
            2 => n.outputs.push(w.as_str()?),
            3 => n.name = w.as_str()?,
            4 => n.op_type = w.as_str()?,
            5 => n.attrs.push(decode_attr(w.as_bytes()?, ctx)?),
            7 => n.domain = w.as_str()?,
            _ => {}
        }
    }
    if n.domain == "ai.onnx" {
        n.domain.clear();
    }
    Ok(n)
}

fn decode_attr(b: &[u8], ctx: &mut LoadCtx) -> Result<Attribute> {
    let mut name = String::new();
    let mut ty = 0i64;
    let (mut f_, mut i_, mut s_, mut t_, mut g_) = (None, None, None, None, None);
    let (mut floats, mut ints, mut strings, mut tensors, mut graphs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut r = Reader::new(b);
    while let Some((f, w)) = r.next()? {
        match f {
            1 => name = w.as_str()?,
            20 => ty = w.as_i64()?,
            2 => f_ = Some(w.as_f32()?),
            3 => i_ = Some(w.as_i64()?),
            4 => s_ = Some(w.as_bytes()?.to_vec()),
            5 => t_ = Some(decode_tensor(w.as_bytes()?, ctx)?.1),
            6 => g_ = Some(decode_graph(w.as_bytes()?, ctx)?),
            7 => push_f32s(&w, &mut floats)?,
            8 => push_varints(&w, &mut ints)?,
            9 => strings.push(w.as_bytes()?.to_vec()),
            10 => tensors.push(decode_tensor(w.as_bytes()?, ctx)?.1),
            11 => graphs.push(decode_graph(w.as_bytes()?, ctx)?),
            _ => {}
        }
    }
    // Old exporters omit `type`; infer it from the populated field.
    if ty == 0 {
        ty = if f_.is_some() {
            1
        } else if i_.is_some() {
            2
        } else if s_.is_some() {
            3
        } else if t_.is_some() {
            4
        } else if g_.is_some() {
            5
        } else if !floats.is_empty() {
            6
        } else if !ints.is_empty() {
            7
        } else if !strings.is_empty() {
            8
        } else if !tensors.is_empty() {
            9
        } else {
            10
        };
    }
    let value = match ty {
        1 => AttrValue::Float(f_.unwrap_or(0.0)),
        2 => AttrValue::Int(i_.unwrap_or(0)),
        3 => AttrValue::Str(s_.unwrap_or_default()),
        4 => AttrValue::Tensor(t_.ok_or_else(|| Error(format!("attribute {name}: missing tensor")))?),
        5 => AttrValue::Graph(Box::new(g_.unwrap_or_default())),
        6 => AttrValue::Floats(floats),
        7 => AttrValue::Ints(ints),
        8 => AttrValue::Strings(strings),
        9 => AttrValue::Tensors(tensors),
        10 => AttrValue::Graphs(graphs),
        t => AttrValue::Unsupported(t as i32),
    };
    Ok(Attribute { name, value })
}

fn decode_value_info(b: &[u8]) -> Result<ValueInfo> {
    let mut vi = ValueInfo { name: String::new(), elem_type: 0, shape: None };
    let mut r = Reader::new(b);
    while let Some((f, w)) = r.next()? {
        match f {
            1 => vi.name = w.as_str()?,
            2 => {
                // TypeProto
                let mut tr = Reader::new(w.as_bytes()?);
                while let Some((f, w)) = tr.next()? {
                    if f == 1 {
                        // TypeProto.Tensor
                        let mut rr = Reader::new(w.as_bytes()?);
                        while let Some((f, w)) = rr.next()? {
                            match f {
                                1 => vi.elem_type = w.as_i64()? as i32,
                                2 => {
                                    let mut dims = Vec::new();
                                    let mut sr = Reader::new(w.as_bytes()?);
                                    while let Some((f, w)) = sr.next()? {
                                        if f == 1 {
                                            let mut d = Dim::Unknown;
                                            let mut dr = Reader::new(w.as_bytes()?);
                                            while let Some((f, w)) = dr.next()? {
                                                match f {
                                                    1 => d = Dim::Value(w.as_i64()?),
                                                    2 => d = Dim::Param(w.as_str()?),
                                                    _ => {}
                                                }
                                            }
                                            dims.push(d);
                                        }
                                    }
                                    vi.shape = Some(dims);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(vi)
}

/// Decodes a `TensorProto` into `(name, tensor)`.
pub fn decode_tensor(b: &[u8], ctx: &mut LoadCtx) -> Result<(String, Tensor)> {
    let mut dims: Vec<i64> = Vec::new();
    let mut dt = 0i32;
    let mut name = String::new();
    let mut raw: Option<Vec<u8>> = None;
    let mut floats = Vec::new();
    let mut i32s = Vec::new();
    let mut i64s = Vec::new();
    let mut f64s = Vec::new();
    let mut u64s = Vec::new();
    let mut external: Vec<(String, String)> = Vec::new();
    let mut location = 0i64;
    let mut r = Reader::new(b);
    while let Some((f, w)) = r.next()? {
        match f {
            1 => push_varints(&w, &mut dims)?,
            2 => dt = w.as_i64()? as i32,
            4 => push_f32s(&w, &mut floats)?,
            5 => push_varints(&w, &mut i32s)?,
            6 => return Err(Error("ONNX: string tensors are not supported".into())),
            7 => push_varints(&w, &mut i64s)?,
            8 => name = w.as_str()?,
            9 => raw = Some(w.as_bytes()?.to_vec()),
            10 => push_f64s(&w, &mut f64s)?,
            11 => push_varints(&w, &mut u64s)?,
            13 => external.push(decode_kv(w.as_bytes()?)?),
            14 => location = w.as_i64()?,
            3 => return Err(Error("ONNX: segmented tensors are not supported".into())),
            _ => {}
        }
    }
    if location == 1 {
        let get = |k: &str| external.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        let loc = get("location").ok_or_else(|| Error(format!("tensor {name}: external data without location")))?;
        let off = get("offset").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let len = get("length").and_then(|v| v.parse::<u64>().ok());
        raw = Some(ctx.read_external(&loc, off, len)?);
    }
    let shape: Vec<usize> = dims.iter().map(|&d| d.max(0) as usize).collect();
    let n: usize = shape.iter().product();
    let data = if let Some(raw) = raw {
        decode_raw(dt, &raw, n).map_err(|e| Error(format!("tensor {name}: {}", e.0)))?
    } else {
        match dt {
            1 => Data::F32(floats),
            11 => Data::F32(f64s.iter().map(|&x| x as f32).collect()),
            10 => Data::F16(i32s.iter().map(|&x| x as u16).collect()),
            16 => Data::F32(i32s.iter().map(|&x| bf16_to_f32(x as u16)).collect()),
            2 => Data::U8(i32s.iter().map(|&x| x as u8).collect()),
            3 => Data::I8(i32s.iter().map(|&x| x as i8).collect()),
            9 => Data::Bool(i32s.iter().map(|&x| x != 0).collect()),
            4 | 5 | 6 => Data::I64(i32s.iter().map(|&x| if dt == 4 { x as u16 as i64 } else { x as i32 as i64 }).collect()),
            7 => Data::I64(i64s),
            12 | 13 => Data::I64(u64s),
            t => return Err(Error(format!("tensor {name}: unsupported data type {t}"))),
        }
    };
    if data.len() != n {
        return Err(Error(format!("tensor {name}: shape {shape:?} needs {n} values, payload has {}", data.len())));
    }
    Ok((name, Tensor { shape, data }))
}

fn decode_raw(dt: i32, raw: &[u8], n: usize) -> Result<Data> {
    let size = match dt {
        1 | 6 | 12 => 4,
        11 | 7 | 13 => 8,
        10 | 16 | 4 | 5 => 2,
        2 | 3 | 9 => 1,
        t => return Err(Error(format!("unsupported data type {t}"))),
    };
    if raw.len() != n * size {
        return Err(Error(format!("raw_data has {} bytes, expected {}", raw.len(), n * size)));
    }
    let c4 = || raw.chunks_exact(4).map(|c| <[u8; 4]>::try_from(c).unwrap());
    let c8 = || raw.chunks_exact(8).map(|c| <[u8; 8]>::try_from(c).unwrap());
    let c2 = || raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
    Ok(match dt {
        1 => Data::F32(c4().map(f32::from_le_bytes).collect()),
        11 => Data::F32(c8().map(|b| f64::from_le_bytes(b) as f32).collect()),
        10 => Data::F16(c2().collect()),
        16 => Data::F32(c2().map(bf16_to_f32).collect()),
        2 => Data::U8(raw.to_vec()),
        3 => Data::I8(raw.iter().map(|&b| b as i8).collect()),
        9 => Data::Bool(raw.iter().map(|&b| b != 0).collect()),
        4 => Data::I64(c2().map(|v| v as i64).collect()),
        5 => Data::I64(c2().map(|v| v as i16 as i64).collect()),
        6 => Data::I64(c4().map(|b| i32::from_le_bytes(b) as i64).collect()),
        12 => Data::I64(c4().map(|b| u32::from_le_bytes(b) as i64).collect()),
        7 => Data::I64(c8().map(i64::from_le_bytes).collect()),
        13 => Data::I64(c8().map(|b| u64::from_le_bytes(b) as i64).collect()),
        _ => unreachable!(),
    })
}

// ------------------------------------------------------------------ encoding

/// Writes tensors of at least `min_bytes` into `dir/file` as external data.
pub struct ExternalData {
    pub dir: PathBuf,
    pub file: String,
    pub min_bytes: usize,
}

struct EncCtx {
    external: Option<(ExternalData, std::fs::File, u64)>,
}

pub fn tensor_raw_bytes(t: &Tensor) -> Vec<u8> {
    let mut b = Vec::with_capacity(t.len() * t.dtype().size_bytes());
    match &t.data {
        Data::F32(v) => v.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes())),
        Data::F16(v) => v.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes())),
        Data::I64(v) => v.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes())),
        Data::I8(v) => v.iter().for_each(|x| b.push(*x as u8)),
        Data::U8(v) => b.extend_from_slice(v),
        Data::Bool(v) => v.iter().for_each(|x| b.push(*x as u8)),
    }
    b
}

fn encode_tensor(name: &str, t: &Tensor, ctx: &mut EncCtx) -> Result<Writer> {
    let mut w = Writer::new();
    w.packed_ints(1, &t.shape.iter().map(|&d| d as i64).collect::<Vec<_>>());
    w.int(2, t.dtype().to_onnx() as i64);
    if !name.is_empty() {
        w.string(8, name);
    }
    let raw = tensor_raw_bytes(t);
    match &mut ctx.external {
        Some((ext, file, offset)) if raw.len() >= ext.min_bytes && raw.len() > 0 => {
            file.write_all(&raw)?;
            for (k, v) in [("location", ext.file.clone()), ("offset", offset.to_string()), ("length", raw.len().to_string())] {
                let mut kv = Writer::new();
                kv.string(1, k);
                kv.string(2, &v);
                w.message(13, &kv);
            }
            w.int(14, 1);
            *offset += raw.len() as u64;
        }
        _ => w.bytes(9, &raw),
    }
    Ok(w)
}

fn encode_value_info(vi: &ValueInfo) -> Writer {
    let mut w = Writer::new();
    w.string(1, &vi.name);
    let mut tt = Writer::new();
    tt.int(1, vi.elem_type as i64);
    if let Some(shape) = &vi.shape {
        let mut sh = Writer::new();
        for d in shape {
            let mut dw = Writer::new();
            match d {
                Dim::Value(v) => dw.int(1, *v),
                Dim::Param(p) => dw.string(2, p),
                Dim::Unknown => {}
            }
            sh.message(1, &dw);
        }
        tt.message(2, &sh);
    }
    let mut ty = Writer::new();
    ty.message(1, &tt);
    w.message(2, &ty);
    w
}

fn encode_attr(a: &Attribute, ctx: &mut EncCtx) -> Result<Writer> {
    let mut w = Writer::new();
    w.string(1, &a.name);
    let ty = match &a.value {
        AttrValue::Float(v) => {
            w.float(2, *v);
            1
        }
        AttrValue::Int(v) => {
            w.int(3, *v);
            2
        }
        AttrValue::Str(v) => {
            w.bytes(4, v);
            3
        }
        AttrValue::Tensor(t) => {
            let tw = encode_tensor("", t, ctx)?;
            w.message(5, &tw);
            4
        }
        AttrValue::Graph(g) => {
            let gw = encode_graph(g, ctx)?;
            w.message(6, &gw);
            5
        }
        AttrValue::Floats(v) => {
            w.packed_floats(7, v);
            6
        }
        AttrValue::Ints(v) => {
            w.packed_ints(8, v);
            7
        }
        AttrValue::Strings(v) => {
            for s in v {
                w.bytes(9, s);
            }
            8
        }
        AttrValue::Tensors(v) => {
            for t in v {
                let tw = encode_tensor("", t, ctx)?;
                w.message(10, &tw);
            }
            9
        }
        AttrValue::Graphs(v) => {
            for g in v {
                let gw = encode_graph(g, ctx)?;
                w.message(11, &gw);
            }
            10
        }
        AttrValue::Unsupported(t) => *t,
    };
    w.int(20, ty as i64);
    Ok(w)
}

fn encode_graph(g: &Graph, ctx: &mut EncCtx) -> Result<Writer> {
    let mut w = Writer::new();
    for n in &g.nodes {
        let mut nw = Writer::new();
        for i in &n.inputs {
            nw.string(1, i);
        }
        for o in &n.outputs {
            nw.string(2, o);
        }
        if !n.name.is_empty() {
            nw.string(3, &n.name);
        }
        nw.string(4, &n.op_type);
        for a in &n.attrs {
            let aw = encode_attr(a, ctx)?;
            nw.message(5, &aw);
        }
        if !n.domain.is_empty() {
            nw.string(7, &n.domain);
        }
        w.message(1, &nw);
    }
    w.string(2, &g.name);
    for (name, t) in &g.initializers {
        let tw = encode_tensor(name, t, ctx)?;
        w.message(5, &tw);
    }
    if !g.doc.is_empty() {
        w.string(10, &g.doc);
    }
    for vi in &g.inputs {
        w.message(11, &encode_value_info(vi));
    }
    for vi in &g.outputs {
        w.message(12, &encode_value_info(vi));
    }
    for vi in &g.value_info {
        w.message(13, &encode_value_info(vi));
    }
    Ok(w)
}

/// Serialises a model. With `external`, large tensors go to a side file.
pub fn encode_model(m: &Model, external: Option<ExternalData>) -> Result<Vec<u8>> {
    let mut ctx = EncCtx { external: None };
    if let Some(ext) = external {
        let f = std::fs::File::create(ext.dir.join(&ext.file))?;
        ctx.external = Some((ext, f, 0));
    }
    let mut w = Writer::new();
    w.int(1, m.ir_version);
    if !m.producer_name.is_empty() {
        w.string(2, &m.producer_name);
    }
    if !m.producer_version.is_empty() {
        w.string(3, &m.producer_version);
    }
    if !m.domain.is_empty() {
        w.string(4, &m.domain);
    }
    if m.model_version != 0 {
        w.int(5, m.model_version);
    }
    if !m.doc_string.is_empty() {
        w.string(6, &m.doc_string);
    }
    let g = encode_graph(&m.graph, &mut ctx)?;
    w.message(7, &g);
    for id in &m.opset_import {
        let mut o = Writer::new();
        if !id.domain.is_empty() {
            o.string(1, &id.domain);
        }
        o.int(2, id.version);
        w.message(8, &o);
    }
    for (k, v) in &m.metadata {
        let mut kv = Writer::new();
        kv.string(1, k);
        kv.string(2, v);
        w.message(14, &kv);
    }
    Ok(w.buf)
}

// ------------------------------------------------------------------ builder

pub fn attr_i(name: &str, v: i64) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Int(v) }
}
pub fn attr_f(name: &str, v: f32) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Float(v) }
}
pub fn attr_s(name: &str, v: &str) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Str(v.as_bytes().to_vec()) }
}
pub fn attr_ints(name: &str, v: &[i64]) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Ints(v.to_vec()) }
}
pub fn attr_floats(name: &str, v: &[f32]) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Floats(v.to_vec()) }
}
pub fn attr_strings(name: &str, v: &[&str]) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Strings(v.iter().map(|s| s.as_bytes().to_vec()).collect()) }
}
pub fn attr_t(name: &str, t: Tensor) -> Attribute {
    Attribute { name: name.into(), value: AttrValue::Tensor(t) }
}

/// Builds ONNX graphs in code (used by tests and for hand-assembled models
/// whose weights come from safetensors).
pub struct GraphBuilder {
    pub graph: Graph,
    pub opset: i64,
    pub extra_opsets: Vec<OpsetId>,
    counter: usize,
}

impl GraphBuilder {
    pub fn new(opset: i64) -> Self {
        GraphBuilder { graph: Graph { name: "graph".into(), ..Default::default() }, opset, extra_opsets: Vec::new(), counter: 0 }
    }
    fn vi(name: &str, dtype: DType, shape: Option<&[i64]>) -> ValueInfo {
        ValueInfo {
            name: name.into(),
            elem_type: dtype.to_onnx(),
            shape: shape.map(|s| s.iter().map(|&d| if d < 0 { Dim::Param(format!("d{}", -d)) } else { Dim::Value(d) }).collect()),
        }
    }
    /// Declares a graph input; negative dims become symbolic (`d1`, `d2`, ..).
    pub fn input(&mut self, name: &str, dtype: DType, shape: &[i64]) -> &mut Self {
        self.graph.inputs.push(Self::vi(name, dtype, Some(shape)));
        self
    }
    pub fn output(&mut self, name: &str, dtype: DType) -> &mut Self {
        self.graph.outputs.push(Self::vi(name, dtype, None));
        self
    }
    pub fn output_shaped(&mut self, name: &str, dtype: DType, shape: &[i64]) -> &mut Self {
        self.graph.outputs.push(Self::vi(name, dtype, Some(shape)));
        self
    }
    pub fn init(&mut self, name: &str, t: Tensor) -> &mut Self {
        self.graph.initializers.push((name.into(), t));
        self
    }
    pub fn node(&mut self, op: &str, inputs: &[&str], outputs: &[&str], attrs: Vec<Attribute>) -> &mut Self {
        self.counter += 1;
        self.graph.nodes.push(Node {
            name: format!("{op}_{}", self.counter),
            op_type: op.into(),
            domain: String::new(),
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            attrs,
        });
        self
    }
    pub fn node_domain(&mut self, domain: &str, op: &str, inputs: &[&str], outputs: &[&str], attrs: Vec<Attribute>) -> &mut Self {
        self.node(op, inputs, outputs, attrs);
        self.graph.nodes.last_mut().unwrap().domain = domain.into();
        if !self.extra_opsets.iter().any(|o| o.domain == domain) {
            self.extra_opsets.push(OpsetId { domain: domain.into(), version: 1 });
        }
        self
    }
    pub fn build(&self) -> Model {
        let mut opset_import = vec![OpsetId { domain: String::new(), version: self.opset }];
        opset_import.extend(self.extra_opsets.iter().cloned());
        Model { ir_version: 8, opset_import, producer_name: "studio-nn".into(), graph: self.graph.clone(), ..Default::default() }
    }
}
