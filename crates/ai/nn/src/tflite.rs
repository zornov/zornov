// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! TFLite flatbuffer importer.
//!
//! A hand-written FlatBuffers reader (tables, vtables, vectors, strings,
//! unions) for the TFLite schema (`schema.fbs`, file identifier `TFL3`), and a
//! lowering of `subgraphs[0]` to the ONNX IR (default domain, opset 17).
//!
//! Layout: tensors stay NHWC at the graph boundary and between ops. Conv-like
//! ops (CONV_2D, DEPTHWISE_CONV_2D, TRANSPOSE_CONV, pooling, SPACE_TO_DEPTH /
//! DEPTH_TO_SPACE, MediaPipe `Convolution2DTransposeBias`) are wrapped in
//! NHWC→NCHW / NCHW→NHWC `Transpose`s, which the session cancels pairwise.
//! Weights are transposed at import (OHWI → OIHW, depthwise 1HWC → C1HW,
//! transpose-conv OHWI → IOHW). SAME padding is resolved to explicit `pads`
//! exactly as TFLite computes it (extra pixel at the end). fp16 weights behind
//! DEQUANTIZE are folded to f32 constants. Fully quantised (int8/uint8
//! activation) models are rejected. Unknown builtins / custom ops become
//! `tflite::<NAME>` nodes so [`Model::report`] lists them as unsupported.
//!
//! MediaPipe `.task` bundles are zip files of `.tflite` members ([`load_task`]).

use crate::error::{Error, Result};
use crate::model::Model;
use crate::onnx::{Attribute, Dim, Graph, Node, OpsetId, ValueInfo, attr_f, attr_i, attr_ints, attr_s};
use crate::tensor::{DType, Data, Tensor, numel};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn is_tflite(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[4..8] == b"TFL3"
}

pub fn load(path: &Path) -> Result<Model> {
    let bytes = std::fs::read(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    from_bytes(&bytes).map_err(|e| Error(format!("{}: {}", path.display(), e.0)))
}

/// Every `.tflite` member of a MediaPipe `.task` bundle (a zip), largest
/// first (ties keep archive order). Nested `.task` members are expanded as
/// `outer.task/inner.tflite`.
pub fn load_task(path: &Path) -> Result<Vec<(String, Model)>> {
    let bytes = std::fs::read(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    task_from_bytes(bytes).map_err(|e| Error(format!("{}: {}", path.display(), e.0)))
}

/// [`load_task`] on in-memory bytes.
pub fn task_from_bytes(bytes: Vec<u8>) -> Result<Vec<(String, Model)>> {
    if is_tflite(&bytes) {
        return Ok(vec![("model.tflite".into(), from_bytes(&bytes)?)]);
    }
    let mut found: Vec<(String, u64, Vec<u8>)> = Vec::new();
    collect_task(bytes, "", &mut found, 0)?;
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found.into_iter().map(|(n, _, b)| from_bytes(&b).map(|m| (n.clone(), m)).map_err(|e| Error(format!("{n}: {}", e.0)))).collect()
}

fn collect_task(bytes: Vec<u8>, prefix: &str, out: &mut Vec<(String, u64, Vec<u8>)>, depth: usize) -> Result<()> {
    let z = crate::zip::ZipArchive::open(bytes)?;
    for e in z.entries() {
        let lower = e.name.to_ascii_lowercase();
        if lower.ends_with(".tflite") {
            out.push((format!("{prefix}{}", e.name), e.size, z.read_entry(e)?));
        } else if lower.ends_with(".task") && depth < 4 {
            let inner = z.read_entry(e)?;
            collect_task(inner, &format!("{prefix}{}/", e.name), out, depth + 1)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ flatbuffers

fn rd<const N: usize>(b: &[u8], p: usize) -> Result<[u8; N]> {
    p.checked_add(N).and_then(|e| b.get(p..e)).map(|s| s.try_into().unwrap()).ok_or_else(|| Error("tflite: truncated flatbuffer".into()))
}

/// A FlatBuffers table: position plus its vtable.
#[derive(Clone, Copy)]
struct Table<'a> {
    b: &'a [u8],
    pos: usize,
    vt: usize,
    vt_len: usize,
}

impl<'a> Table<'a> {
    fn at(b: &'a [u8], pos: usize) -> Result<Table<'a>> {
        let so = i32::from_le_bytes(rd(b, pos)?) as i64;
        let vt = pos as i64 - so;
        crate::ensure!(vt >= 0 && (vt as usize) < b.len(), "tflite: bad vtable offset");
        let vt = vt as usize;
        let vt_len = u16::from_le_bytes(rd(b, vt)?) as usize;
        crate::ensure!(vt_len >= 4 && vt + vt_len <= b.len(), "tflite: bad vtable");
        Ok(Table { b, pos, vt, vt_len })
    }
    fn root(b: &'a [u8]) -> Result<Table<'a>> {
        Table::at(b, u32::from_le_bytes(rd(b, 0)?) as usize)
    }
    fn field(&self, id: usize) -> Option<usize> {
        let o = 4 + 2 * id;
        if o + 2 > self.vt_len {
            return None;
        }
        let off = u16::from_le_bytes([self.b[self.vt + o], self.b[self.vt + o + 1]]) as usize;
        if off == 0 { None } else { Some(self.pos + off) }
    }
    fn u8(&self, id: usize, def: u8) -> Result<u8> {
        self.field(id).map(|p| rd::<1>(self.b, p).map(|v| v[0])).unwrap_or(Ok(def))
    }
    fn i32(&self, id: usize, def: i32) -> Result<i32> {
        self.field(id).map(|p| rd(self.b, p).map(i32::from_le_bytes)).unwrap_or(Ok(def))
    }
    fn u32(&self, id: usize, def: u32) -> Result<u32> {
        self.field(id).map(|p| rd(self.b, p).map(u32::from_le_bytes)).unwrap_or(Ok(def))
    }
    fn u64(&self, id: usize, def: u64) -> Result<u64> {
        self.field(id).map(|p| rd(self.b, p).map(u64::from_le_bytes)).unwrap_or(Ok(def))
    }
    fn f32(&self, id: usize, def: f32) -> Result<f32> {
        self.field(id).map(|p| rd(self.b, p).map(f32::from_le_bytes)).unwrap_or(Ok(def))
    }
    fn indirect(&self, id: usize) -> Result<Option<usize>> {
        match self.field(id) {
            None => Ok(None),
            Some(p) => {
                let t = p.checked_add(u32::from_le_bytes(rd(self.b, p)?) as usize).filter(|&t| t < self.b.len());
                t.map(Some).ok_or_else(|| Error("tflite: offset out of range".into()))
            }
        }
    }
    fn table(&self, id: usize) -> Result<Option<Table<'a>>> {
        self.indirect(id)?.map(|p| Table::at(self.b, p)).transpose()
    }
    /// `(first element position, length)` of a vector of `elem`-byte items.
    fn vector(&self, id: usize, elem: usize) -> Result<Option<(usize, usize)>> {
        let Some(p) = self.indirect(id)? else { return Ok(None) };
        let n = u32::from_le_bytes(rd(self.b, p)?) as usize;
        let end = n.checked_mul(elem).and_then(|s| s.checked_add(p + 4));
        crate::ensure!(end.is_some_and(|e| e <= self.b.len()), "tflite: vector out of range");
        Ok(Some((p + 4, n)))
    }
    fn bytes(&self, id: usize) -> Result<&'a [u8]> {
        Ok(match self.vector(id, 1)? {
            Some((p, n)) => &self.b[p..p + n],
            None => &[],
        })
    }
    fn string(&self, id: usize) -> Result<Option<String>> {
        Ok(self.vector(id, 1)?.map(|(p, n)| String::from_utf8_lossy(&self.b[p..p + n]).into_owned()))
    }
    fn i32s(&self, id: usize) -> Result<Vec<i32>> {
        Ok(match self.vector(id, 4)? {
            Some((p, n)) => (0..n).map(|i| i32::from_le_bytes(self.b[p + 4 * i..p + 4 * i + 4].try_into().unwrap())).collect(),
            None => Vec::new(),
        })
    }
    fn f32s(&self, id: usize) -> Result<Vec<f32>> {
        Ok(self.i32s(id)?.into_iter().map(|v| f32::from_bits(v as u32)).collect())
    }
    fn i64s(&self, id: usize) -> Result<Vec<i64>> {
        Ok(match self.vector(id, 8)? {
            Some((p, n)) => (0..n).map(|i| i64::from_le_bytes(self.b[p + 8 * i..p + 8 * i + 8].try_into().unwrap())).collect(),
            None => Vec::new(),
        })
    }
    fn tables(&self, id: usize) -> Result<Vec<Table<'a>>> {
        let Some((p, n)) = self.vector(id, 4)? else { return Ok(Vec::new()) };
        (0..n)
            .map(|i| {
                let e = p + 4 * i;
                Table::at(self.b, e + u32::from_le_bytes(rd(self.b, e)?) as usize)
            })
            .collect()
    }
}

/// Options table accessor that falls back to defaults when absent.
#[derive(Clone, Copy)]
struct Opts<'a>(Option<Table<'a>>);

impl Opts<'_> {
    fn i(&self, id: usize, def: i32) -> Result<i32> {
        self.0.map(|t| t.i32(id, def)).unwrap_or(Ok(def))
    }
    fn u8(&self, id: usize, def: u8) -> Result<u8> {
        self.0.map(|t| t.u8(id, def)).unwrap_or(Ok(def))
    }
    fn b(&self, id: usize) -> Result<bool> {
        Ok(self.u8(id, 0)? != 0)
    }
    fn f(&self, id: usize, def: f32) -> Result<f32> {
        self.0.map(|t| t.f32(id, def)).unwrap_or(Ok(def))
    }
    fn ints(&self, id: usize) -> Result<Option<Vec<i32>>> {
        match self.0 {
            Some(t) if t.field(id).is_some() => Ok(Some(t.i32s(id)?)),
            _ => Ok(None),
        }
    }
}

// ------------------------------------------------------------------ schema

struct TTensor {
    name: String,
    shape: Vec<i64>,
    sig: Vec<i64>,
    ty: u8,
    buffer: usize,
    scale: Vec<f32>,
    zero_point: Vec<i64>,
    qdim: i32,
    /// Sparse constant (DENSIFY input): the buffer holds only the stored values.
    sparsity: Option<Sparsity>,
}

/// TFLite `SparsityParameters`: the traversal order of the (original +
/// block) dimensions, which original dimension each block dimension splits,
/// and per traversed dimension either its dense size or CSR segments/indices.
#[derive(Clone, Debug)]
struct Sparsity {
    order: Vec<usize>,
    block_map: Vec<usize>,
    dims: Vec<SparseDim>,
}

#[derive(Clone, Debug)]
enum SparseDim {
    Dense(usize),
    Csr { segments: Vec<usize>, indices: Vec<usize> },
}

/// A `SparseIndexVector` union (Int32Vector / Uint16Vector / Uint8Vector).
fn index_vector(t: &Table, ty_id: usize, val_id: usize) -> Result<Vec<usize>> {
    let ty = t.u8(ty_id, 0)?;
    let Some(v) = t.table(val_id)? else { return Ok(Vec::new()) };
    Ok(match ty {
        1 => v.i32s(0)?.into_iter().map(|x| x.max(0) as usize).collect(),
        2 => match v.vector(0, 2)? {
            Some((p, n)) => (0..n).map(|i| u16::from_le_bytes([v.b[p + 2 * i], v.b[p + 2 * i + 1]]) as usize).collect(),
            None => Vec::new(),
        },
        3 => v.bytes(0)?.iter().map(|&x| x as usize).collect(),
        _ => Vec::new(),
    })
}

fn read_sparsity(t: &Table) -> Result<Option<Sparsity>> {
    let Some(s) = t.table(6)? else { return Ok(None) };
    let dims = s
        .tables(2)?
        .iter()
        .map(|d| {
            Ok(if d.u8(0, 0)? == 0 {
                SparseDim::Dense(d.i32(1, 0)?.max(0) as usize)
            } else {
                SparseDim::Csr { segments: index_vector(d, 2, 3)?, indices: index_vector(d, 4, 5)? }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let us = |v: Vec<i32>| v.into_iter().map(|x| x.max(0) as usize).collect::<Vec<_>>();
    Ok(Some(Sparsity { order: us(s.i32s(0)?), block_map: us(s.i32s(1)?), dims }))
}

/// Dense tensor of `shape` from the stored values `src` (TFLite
/// `FormatConverter::SparseToDense`); unstored entries are zero.
fn densify<T: Copy + Default>(src: &[T], shape: &[usize], sp: &Sparsity) -> Result<Vec<T>> {
    let (n, levels) = (shape.len(), sp.order.len());
    crate::ensure!(levels >= n && sp.dims.len() == levels && sp.block_map.len() == levels - n, "tflite: bad sparsity parameters");
    crate::ensure!(sp.order.iter().all(|&o| o < levels) && sp.block_map.iter().all(|&b| b < n), "tflite: bad sparsity parameters");
    let block: Vec<usize> = (n..levels).map(|k| match sp.dims[k] { SparseDim::Dense(s) => s, _ => 1 }).collect();
    let mut out = vec![T::default(); numel(shape)];
    struct W<'a, T> {
        src: &'a [T],
        out: &'a mut [T],
        shape: &'a [usize],
        sp: &'a Sparsity,
        block: &'a [usize],
        idx: Vec<usize>,
        ptr: usize,
    }
    fn rec<T: Copy>(w: &mut W<T>, level: usize, prev: usize) -> Result<()> {
        let n = w.shape.len();
        if level == w.idx.len() {
            let mut orig = vec![0usize; n];
            for i in 0..w.idx.len() {
                let d = w.sp.order[i];
                if d < n {
                    orig[d] = w.idx[i];
                } else {
                    let b = d - n;
                    let od = w.sp.block_map[b];
                    orig[od] = orig[od] * w.block[b] + w.idx[i];
                }
            }
            let mut flat = 0usize;
            for (k, &o) in orig.iter().enumerate() {
                crate::ensure!(o < w.shape[k], "tflite: sparse index out of range");
                flat = flat * w.shape[k] + o;
            }
            let v = *w.src.get(w.ptr).ok_or_else(|| Error("tflite: sparse tensor has too few values".into()))?;
            w.out[flat] = v;
            w.ptr += 1;
            return Ok(());
        }
        match &w.sp.dims[level] {
            SparseDim::Dense(size) => {
                for i in 0..*size {
                    w.idx[level] = i;
                    rec(w, level + 1, prev * size + i)?;
                }
            }
            SparseDim::Csr { segments, indices } => {
                let (a, b) = (segments.get(prev).copied(), segments.get(prev + 1).copied());
                let (Some(a), Some(b)) = (a, b) else { crate::bail!("tflite: sparse segments out of range") };
                for i in a..b {
                    w.idx[level] = *indices.get(i).ok_or_else(|| Error("tflite: sparse indices out of range".into()))?;
                    rec(w, level + 1, i)?;
                }
            }
        }
        Ok(())
    }
    let mut w = W { src, out: &mut out, shape, sp, block: &block, idx: vec![0; levels], ptr: 0 };
    rec(&mut w, 0, 0)?;
    Ok(out)
}

/// A sparse constant decoded and expanded to its dense shape.
fn decode_sparse(t: &TTensor, data: &[u8], sp: &Sparsity) -> Result<Tensor> {
    let size = match t.ty {
        0 | 2 | 15 => 4,
        1 | 7 | 16 => 2,
        4 | 10 | 12 => 8,
        3 | 9 | 6 => 1,
        ty => crate::bail!("tflite: sparse constant '{}' has unsupported type {}", t.name, type_name(ty)),
    };
    let stored = TTensor { name: t.name.clone(), shape: vec![(data.len() / size) as i64], sig: Vec::new(), ty: t.ty, buffer: t.buffer, scale: Vec::new(), zero_point: Vec::new(), qdim: 0, sparsity: None };
    let v = decode_const(&stored, data)?;
    let shape: Vec<usize> = t.shape.iter().map(|&d| d.max(0) as usize).collect();
    let d = match v.data {
        Data::F32(x) => Data::F32(densify(&x, &shape, sp)?),
        Data::F16(x) => Data::F16(densify(&x, &shape, sp)?),
        Data::I64(x) => Data::I64(densify(&x, &shape, sp)?),
        Data::U8(x) => Data::U8(densify(&x, &shape, sp)?),
        Data::I8(x) => Data::I8(densify(&x, &shape, sp)?),
        Data::Bool(x) => Data::Bool(densify(&x, &shape, sp)?),
    };
    Tensor::new(shape, d)
}

struct TOp<'a> {
    code: i32,
    custom: String,
    inputs: Vec<i32>,
    outputs: Vec<i32>,
    opts: Opts<'a>,
    custom_opts: &'a [u8],
}

fn type_name(t: u8) -> &'static str {
    match t {
        0 => "FLOAT32",
        1 => "FLOAT16",
        2 => "INT32",
        3 => "UINT8",
        4 => "INT64",
        5 => "STRING",
        6 => "BOOL",
        7 => "INT16",
        8 => "COMPLEX64",
        9 => "INT8",
        10 => "FLOAT64",
        11 => "COMPLEX128",
        12 => "UINT64",
        13 => "RESOURCE",
        14 => "VARIANT",
        15 => "UINT32",
        16 => "UINT16",
        17 => "INT4",
        _ => "UNKNOWN",
    }
}

fn storage_dtype(t: u8) -> Option<DType> {
    Some(match t {
        0 | 10 => DType::F32,
        1 => DType::F16,
        2 | 4 | 7 | 12 | 15 | 16 => DType::I64,
        3 => DType::U8,
        9 => DType::I8,
        6 => DType::Bool,
        _ => return None,
    })
}

fn builtin_name(code: i32) -> String {
    let n = match code {
        0 => "ADD",
        1 => "AVERAGE_POOL_2D",
        2 => "CONCATENATION",
        3 => "CONV_2D",
        4 => "DEPTHWISE_CONV_2D",
        5 => "DEPTH_TO_SPACE",
        6 => "DEQUANTIZE",
        7 => "EMBEDDING_LOOKUP",
        8 => "FLOOR",
        9 => "FULLY_CONNECTED",
        11 => "L2_NORMALIZATION",
        12 => "L2_POOL_2D",
        13 => "LOCAL_RESPONSE_NORMALIZATION",
        14 => "LOGISTIC",
        16 => "LSTM",
        17 => "MAX_POOL_2D",
        18 => "MUL",
        19 => "RELU",
        20 => "RELU_N1_TO_1",
        21 => "RELU6",
        22 => "RESHAPE",
        23 => "RESIZE_BILINEAR",
        24 => "RNN",
        25 => "SOFTMAX",
        26 => "SPACE_TO_DEPTH",
        28 => "TANH",
        31 => "CALL",
        32 => "CUSTOM",
        34 => "PAD",
        36 => "GATHER",
        37 => "BATCH_TO_SPACE_ND",
        38 => "SPACE_TO_BATCH_ND",
        39 => "TRANSPOSE",
        40 => "MEAN",
        41 => "SUB",
        42 => "DIV",
        43 => "SQUEEZE",
        44 => "UNIDIRECTIONAL_SEQUENCE_LSTM",
        45 => "STRIDED_SLICE",
        47 => "EXP",
        48 => "TOPK_V2",
        49 => "SPLIT",
        50 => "LOG_SOFTMAX",
        53 => "CAST",
        54 => "PRELU",
        55 => "MAXIMUM",
        56 => "ARG_MAX",
        57 => "MINIMUM",
        58 => "LESS",
        59 => "NEG",
        60 => "PADV2",
        61 => "GREATER",
        62 => "GREATER_EQUAL",
        63 => "LESS_EQUAL",
        64 => "SELECT",
        65 => "SLICE",
        66 => "SIN",
        67 => "TRANSPOSE_CONV",
        69 => "TILE",
        70 => "EXPAND_DIMS",
        71 => "EQUAL",
        72 => "NOT_EQUAL",
        73 => "LOG",
        74 => "SUM",
        75 => "SQRT",
        76 => "RSQRT",
        77 => "SHAPE",
        78 => "POW",
        79 => "ARG_MIN",
        81 => "REDUCE_PROD",
        82 => "REDUCE_MAX",
        83 => "PACK",
        84 => "LOGICAL_OR",
        85 => "ONE_HOT",
        86 => "LOGICAL_AND",
        87 => "LOGICAL_NOT",
        88 => "UNPACK",
        89 => "REDUCE_MIN",
        90 => "FLOOR_DIV",
        92 => "SQUARE",
        93 => "ZEROS_LIKE",
        94 => "FILL",
        95 => "FLOOR_MOD",
        96 => "RANGE",
        97 => "RESIZE_NEAREST_NEIGHBOR",
        98 => "LEAKY_RELU",
        99 => "SQUARED_DIFFERENCE",
        100 => "MIRROR_PAD",
        101 => "ABS",
        102 => "SPLIT_V",
        104 => "CEIL",
        105 => "REVERSE_V2",
        106 => "ADD_N",
        107 => "GATHER_ND",
        108 => "COS",
        109 => "WHERE",
        111 => "ELU",
        114 => "QUANTIZE",
        116 => "ROUND",
        117 => "HARD_SWISH",
        118 => "IF",
        119 => "WHILE",
        122 => "SCATTER_ND",
        123 => "SELECT_V2",
        124 => "DENSIFY",
        126 => "BATCH_MATMUL",
        128 => "CUMSUM",
        130 => "BROADCAST_TO",
        150 => "GELU",
        152 => "RELU_0_TO_1",
        _ => return format!("BUILTIN_{code}"),
    };
    n.to_string()
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-') { c } else { '_' }).collect()
}

/// TFLite SAME padding for one spatial dim: `(before, after)`.
fn same_pad(inp: i64, k: i64, s: i64, d: i64) -> (i64, i64) {
    let out = (inp + s - 1) / s;
    let eff = (k - 1) * d + 1;
    let total = ((out - 1) * s + eff - inp).max(0);
    (total / 2, total - total / 2)
}

fn decode_const(t: &TTensor, data: &[u8]) -> Result<Tensor> {
    let shape: Vec<usize> = t.shape.iter().map(|&d| d.max(0) as usize).collect();
    let n = numel(&shape);
    let need = |sz: usize| -> Result<&[u8]> {
        data.get(..n * sz).ok_or_else(|| Error(format!("tflite: buffer of '{}' too small ({} bytes for {n} elements)", t.name, data.len())))
    };
    macro_rules! le {
        ($ty:ty, $sz:expr) => {
            need($sz)?.chunks_exact($sz).map(|c| <$ty>::from_le_bytes(c.try_into().unwrap()))
        };
    }
    let d = match t.ty {
        0 => Data::F32(le!(f32, 4).collect()),
        1 => Data::F16(le!(u16, 2).collect()),
        10 => Data::F32(le!(f64, 8).map(|v| v as f32).collect()),
        2 => Data::I64(le!(i32, 4).map(|v| v as i64).collect()),
        4 => Data::I64(le!(i64, 8).collect()),
        7 => Data::I64(le!(i16, 2).map(|v| v as i64).collect()),
        16 => Data::I64(le!(u16, 2).map(|v| v as i64).collect()),
        15 => Data::I64(le!(u32, 4).map(|v| v as i64).collect()),
        12 => Data::I64(le!(u64, 8).map(|v| v as i64).collect()),
        3 => Data::U8(need(1)?.to_vec()),
        9 => Data::I8(need(1)?.iter().map(|&v| v as i8).collect()),
        6 => Data::Bool(need(1)?.iter().map(|&v| v != 0).collect()),
        ty => crate::bail!("tflite: constant '{}' has unsupported type {}", t.name, type_name(ty)),
    };
    Tensor::new(shape, d)
}

// ------------------------------------------------------------------ importer

struct Imp<'a> {
    t: Vec<TTensor>,
    bufs: Vec<&'a [u8]>,
    names: Vec<String>,
    consts: HashMap<usize, Tensor>,
    emitted: HashSet<usize>,
    weights: HashMap<(usize, Vec<usize>), String>,
    used: HashSet<String>,
    g: Graph,
    mediapipe: bool,
}

const NHWC_TO_NCHW: [i64; 4] = [0, 3, 1, 2];
const NCHW_TO_NHWC: [i64; 4] = [0, 2, 3, 1];

impl<'a> Imp<'a> {
    fn fresh(&mut self, hint: &str) -> String {
        let base = sanitize(hint);
        let mut name = base.clone();
        let mut k = 1;
        while self.used.contains(&name) {
            name = format!("{base}_{k}");
            k += 1;
        }
        self.used.insert(name.clone());
        name
    }

    fn tensor(&self, i: i32) -> Result<&TTensor> {
        usize::try_from(i).ok().and_then(|i| self.t.get(i)).ok_or_else(|| Error(format!("tflite: tensor index {i} out of range")))
    }

    fn cst(&self, i: i32) -> Option<&Tensor> {
        if i < 0 { None } else { self.consts.get(&(i as usize)) }
    }

    fn cst_i64(&self, i: i32) -> Option<Vec<i64>> {
        self.cst(i).filter(|t| !t.dtype().is_float()).map(|t| t.to_i64_vec())
    }

    fn need_i64(&self, i: i32, what: &str) -> Result<Vec<i64>> {
        self.cst_i64(i).ok_or_else(|| Error(format!("tflite: {what} must be a constant integer tensor")))
    }

    /// Static shape (`-1` for dynamic dims), `None` if the index is absent.
    fn dims(&self, i: i32) -> Option<Vec<i64>> {
        let t = self.tensor(i).ok()?;
        Some(t.shape.iter().enumerate().map(|(k, &d)| if t.sig.get(k).is_some_and(|&s| s < 0) && k != 0 { -1 } else { d }).collect())
    }

    /// Name for tensor `i` as an op input (emits constants on first use).
    fn val(&mut self, i: i32) -> Result<String> {
        if i < 0 {
            return Ok(String::new());
        }
        let u = i as usize;
        self.tensor(i)?;
        if let Some(t) = self.consts.get(&u)
            && self.emitted.insert(u)
        {
            let t = t.clone();
            self.g.initializers.push((self.names[u].clone(), t));
        }
        Ok(self.names[u].clone())
    }

    fn out(&mut self, i: i32) -> Result<String> {
        if i < 0 {
            return Ok(self.fresh("unused"));
        }
        self.tensor(i)?;
        Ok(self.names[i as usize].clone())
    }

    fn node_d(&mut self, domain: &str, op: &str, inputs: Vec<String>, outputs: Vec<String>, attrs: Vec<Attribute>) {
        let name = format!("{}_{}", op, outputs.first().map(|s| s.as_str()).unwrap_or(""));
        self.g.nodes.push(Node { name, op_type: op.into(), domain: domain.into(), inputs, outputs, attrs });
    }

    fn node(&mut self, op: &str, inputs: Vec<String>, outputs: Vec<String>, attrs: Vec<Attribute>) {
        self.node_d("", op, inputs, outputs, attrs);
    }

    /// Single-output node writing a fresh tensor.
    fn tmp(&mut self, op: &str, hint: &str, inputs: Vec<String>, attrs: Vec<Attribute>) -> String {
        let o = self.fresh(hint);
        self.node(op, inputs, vec![o.clone()], attrs);
        o
    }

    fn init(&mut self, hint: &str, t: Tensor) -> String {
        let n = self.fresh(hint);
        self.g.initializers.push((n.clone(), t));
        n
    }

    fn i64s(&mut self, hint: &str, v: &[i64]) -> String {
        self.init(hint, Tensor::i64(&[v.len()], v.to_vec()))
    }

    fn scalar(&mut self, hint: &str, v: f32) -> String {
        self.init(hint, Tensor::scalar_f32(v))
    }

    fn nchw(&mut self, x: String) -> String {
        let h = format!("{x}__nchw");
        self.tmp("Transpose", &h, vec![x], vec![attr_ints("perm", &NHWC_TO_NCHW)])
    }

    /// Weight tensor `i` permuted by `perm` (folded for constants).
    fn weight(&mut self, i: i32, perm: &[usize]) -> Result<String> {
        let u = i as usize;
        if let Some(n) = self.weights.get(&(u, perm.to_vec())) {
            return Ok(n.clone());
        }
        let name = if let Some(t) = self.cst(i) {
            crate::ensure!(t.rank() == perm.len(), "tflite: weight '{}' has rank {} (expected {})", self.names[u], t.rank(), perm.len());
            let t = crate::ops::shape_ops::transpose_tensor(t, perm);
            let hint = format!("{}__w", self.names[u]);
            self.init(&hint, t)
        } else {
            let x = self.val(i)?;
            let p: Vec<i64> = perm.iter().map(|&v| v as i64).collect();
            let hint = format!("{x}__w");
            self.tmp("Transpose", &hint, vec![x], vec![attr_ints("perm", &p)])
        };
        self.weights.insert((u, perm.to_vec()), name.clone());
        Ok(name)
    }

    /// Fused activation from `x` into `out`.
    fn act(&mut self, x: String, out: String, act: u8) -> Result<()> {
        let clip = |s: &mut Self, lo: f32, hi: f32| {
            let a = s.scalar("clip_min", lo);
            let b = s.scalar("clip_max", hi);
            s.node("Clip", vec![x.clone(), a, b], vec![out.clone()], vec![]);
        };
        match act {
            0 => self.node("Identity", vec![x.clone()], vec![out.clone()], vec![]),
            1 => self.node("Relu", vec![x.clone()], vec![out.clone()], vec![]),
            2 => clip(self, -1.0, 1.0),
            3 => clip(self, 0.0, 6.0),
            4 => self.node("Tanh", vec![x.clone()], vec![out.clone()], vec![]),
            5 => {
                // SIGN_BIT: 1 where x < 0
                let z = self.scalar("zero", 0.0);
                let l = self.tmp("Less", &format!("{x}__neg"), vec![x.clone(), z], vec![]);
                self.node("Cast", vec![l], vec![out.clone()], vec![attr_i("to", 1)]);
            }
            a => crate::bail!("tflite: unknown fused activation {a}"),
        }
        Ok(())
    }

    /// Where an op with fused activation `act` should write before the activation.
    fn act_dst(&mut self, out: &str, act: u8) -> String {
        if act == 0 { out.to_string() } else { self.fresh(&format!("{out}__preact")) }
    }

    fn finish_act(&mut self, dst: String, out: String, act: u8) -> Result<()> {
        if act == 0 { Ok(()) } else { self.act(dst, out, act) }
    }

    /// NCHW result `y` → activation → NHWC `out`.
    fn conv_tail(&mut self, y: String, out: String, act: u8) -> Result<()> {
        let z = if act == 0 {
            y
        } else {
            let z = self.fresh(&format!("{out}__act"));
            self.act(y, z.clone(), act)?;
            z
        };
        self.node("Transpose", vec![z], vec![out], vec![attr_ints("perm", &NCHW_TO_NHWC)]);
        Ok(())
    }

    /// Explicit `pads` for a 2-D window op over NHWC input `x`.
    fn window_pads(&self, x: i32, k: [i64; 2], s: [i64; 2], d: [i64; 2], same: bool) -> Attribute {
        if !same {
            return attr_ints("pads", &[0, 0, 0, 0]);
        }
        match self.dims(x) {
            Some(sh) if sh.len() == 4 && sh[1] > 0 && sh[2] > 0 => {
                let (t, b) = same_pad(sh[1], k[0], s[0], d[0]);
                let (l, r) = same_pad(sh[2], k[1], s[1], d[1]);
                attr_ints("pads", &[t, l, b, r])
            }
            // SAME_UPPER resolves identically at run time
            _ => attr_s("auto_pad", "SAME_UPPER"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transpose_conv(&mut self, x: i32, w: i32, b: i32, stride: [i64; 2], same: bool, out_hw: Option<[i64; 2]>, act: u8, out: String) -> Result<()> {
        let xs = self.dims(x).filter(|s| s.len() == 4 && s[1] > 0 && s[2] > 0).ok_or_else(|| Error(format!("tflite: transpose conv '{out}' needs a static input shape")))?;
        let ws = self.tensor(w)?.shape.clone();
        crate::ensure!(ws.len() == 4, "tflite: transpose conv weight must be OHWI");
        let k = [ws[1], ws[2]];
        let mut pads = [0i64; 4];
        let mut opad = [0i64; 2];
        for i in 0..2 {
            let full = (xs[1 + i] - 1) * stride[i] + k[i];
            let o = match out_hw {
                Some(o) => o[i],
                None if same => xs[1 + i] * stride[i],
                None => full,
            };
            let total = full - o;
            if total >= 0 {
                let before = if same { total / 2 } else { 0 };
                pads[i] = before;
                pads[2 + i] = total - before;
            } else {
                opad[i] = -total;
            }
        }
        let xt = self.val(x)?;
        let xt = self.nchw(xt);
        let wt = self.weight(w, &[3, 0, 1, 2])?;
        let mut ins = vec![xt, wt];
        if b >= 0 {
            ins.push(self.val(b)?);
        }
        let mut attrs = vec![attr_ints("kernel_shape", &k), attr_ints("strides", &stride), attr_ints("pads", &pads)];
        if opad != [0, 0] {
            attrs.push(attr_ints("output_padding", &opad));
        }
        let y = self.tmp("ConvTranspose", &format!("{out}__nchw"), ins, attrs);
        self.conv_tail(y, out, act)
    }

    fn unary(&mut self, op: &str, o: &TOp, attrs: Vec<Attribute>) -> Result<()> {
        let x = self.val(o.inputs[0])?;
        let out = self.out(o.outputs[0])?;
        self.node(op, vec![x], vec![out], attrs);
        Ok(())
    }

    fn binary(&mut self, op: &str, o: &TOp, act: u8) -> Result<()> {
        let a = self.val(o.inputs[0])?;
        let b = self.val(o.inputs[1])?;
        let out = self.out(o.outputs[0])?;
        let dst = self.act_dst(&out, act);
        self.node(op, vec![a, b], vec![dst.clone()], vec![]);
        self.finish_act(dst, out, act)
    }

    fn reduce(&mut self, op: &str, o: &TOp) -> Result<()> {
        let keep = o.opts.b(0)?;
        let x = self.val(o.inputs[0])?;
        let out = self.out(o.outputs[0])?;
        let keep_a = attr_i("keepdims", keep as i64);
        match self.cst_i64(o.inputs[1]) {
            Some(ax) if op != "ReduceSum" => self.node(op, vec![x], vec![out], vec![attr_ints("axes", &ax), keep_a]),
            Some(ax) => {
                let a = self.i64s(&format!("{out}__axes"), &ax);
                self.node(op, vec![x, a], vec![out], vec![keep_a]);
            }
            None => {
                let a = self.val(o.inputs[1])?;
                self.node(op, vec![x, a], vec![out], vec![keep_a]);
            }
        }
        Ok(())
    }

    fn import_op(&mut self, o: &TOp) -> Result<()> {
        let inp = |k: usize| o.inputs.get(k).copied().unwrap_or(-1);
        crate::ensure!(!o.outputs.is_empty(), "tflite: operator without outputs");
        let o0 = o.outputs[0];
        let opts = o.opts;
        match o.code {
            // ---------------------------------------------------- convolution
            3 | 4 => {
                let dw = o.code == 4;
                let same = opts.u8(0, 0)? == 0;
                let s = [opts.i(2, 1)?.max(1) as i64, opts.i(1, 1)?.max(1) as i64];
                let (act, dil) = if dw {
                    (opts.u8(4, 0)?, [opts.i(6, 1)?.max(1) as i64, opts.i(5, 1)?.max(1) as i64])
                } else {
                    (opts.u8(3, 0)?, [opts.i(5, 1)?.max(1) as i64, opts.i(4, 1)?.max(1) as i64])
                };
                let ws = self.tensor(inp(1))?.shape.clone();
                crate::ensure!(ws.len() == 4, "tflite: conv weight must be rank 4");
                let k = [ws[1], ws[2]];
                let cin = self.dims(inp(0)).and_then(|d| d.get(3).copied()).filter(|&c| c > 0);
                let group = if dw {
                    match cin {
                        Some(c) => c,
                        None => ws[3] / (opts.i(3, 1)?.max(1) as i64),
                    }
                } else {
                    cin.map(|c| (c / ws[3].max(1)).max(1)).unwrap_or(1)
                };
                let pads = self.window_pads(inp(0), k, s, dil, same);
                let x = self.val(inp(0))?;
                let xt = self.nchw(x);
                let wt = self.weight(inp(1), if dw { &[3, 0, 1, 2] } else { &[0, 3, 1, 2] })?;
                let mut ins = vec![xt, wt];
                if inp(2) >= 0 {
                    ins.push(self.val(inp(2))?);
                }
                let out = self.out(o0)?;
                let attrs = vec![attr_ints("kernel_shape", &k), attr_ints("strides", &s), attr_ints("dilations", &dil), attr_i("group", group), pads];
                let y = self.tmp("Conv", &format!("{out}__nchw"), ins, attrs);
                self.conv_tail(y, out, act)?;
            }
            67 => {
                // TRANSPOSE_CONV: (output_shape, weights OHWI, input, bias?)
                let same = opts.u8(0, 0)? == 0;
                let s = [opts.i(2, 1)?.max(1) as i64, opts.i(1, 1)?.max(1) as i64];
                let act = opts.u8(3, 0)?;
                let os = self.cst_i64(inp(0)).or_else(|| self.dims(o0)).filter(|v| v.len() == 4 && v[1] > 0 && v[2] > 0);
                let os = os.ok_or_else(|| Error("tflite: TRANSPOSE_CONV needs a constant output_shape".into()))?;
                let out = self.out(o0)?;
                self.transpose_conv(inp(2), inp(1), inp(3), s, same, Some([os[1], os[2]]), act, out)?;
            }
            9 => {
                // FULLY_CONNECTED
                let act = opts.u8(0, 0)?;
                crate::ensure!(opts.u8(1, 0)? == 0, "tflite: shuffled FULLY_CONNECTED weights are not supported");
                let keep = opts.b(2)?;
                let ws = self.tensor(inp(1))?.shape.clone();
                crate::ensure!(ws.len() == 2, "tflite: FULLY_CONNECTED weight must be rank 2");
                let out = self.out(o0)?;
                let dst = self.act_dst(&out, act);
                let x = self.val(inp(0))?;
                if keep {
                    let wt = self.weight(inp(1), &[1, 0])?;
                    if inp(2) >= 0 {
                        let m = self.tmp("MatMul", &format!("{out}__mm"), vec![x, wt], vec![]);
                        let b = self.val(inp(2))?;
                        self.node("Add", vec![m, b], vec![dst.clone()], vec![]);
                    } else {
                        self.node("MatMul", vec![x, wt], vec![dst.clone()], vec![]);
                    }
                } else {
                    let xd = self.dims(inp(0));
                    let xr = if xd.as_ref().is_some_and(|d| d.len() == 2 && d[1] == ws[1]) {
                        x
                    } else {
                        let s = self.i64s(&format!("{out}__flat"), &[-1, ws[1]]);
                        self.tmp("Reshape", &format!("{x}__2d"), vec![x, s], vec![])
                    };
                    let w = self.val(inp(1))?;
                    let mut ins = vec![xr, w];
                    if inp(2) >= 0 {
                        ins.push(self.val(inp(2))?);
                    }
                    self.node("Gemm", ins, vec![dst.clone()], vec![attr_i("transB", 1)]);
                }
                self.finish_act(dst, out, act)?;
            }
            126 => {
                // BATCH_MATMUL
                let (adj_x, adj_y) = (opts.b(0)?, opts.b(1)?);
                let mut ab = Vec::new();
                for (k, adj) in [(0, adj_x), (1, adj_y)] {
                    let v = self.val(inp(k))?;
                    if adj {
                        let r = self.dims(inp(k)).map(|d| d.len()).filter(|&r| r >= 2).ok_or_else(|| Error("tflite: BATCH_MATMUL adjoint needs a known rank".into()))?;
                        let mut p: Vec<i64> = (0..r as i64).collect();
                        p.swap(r - 1, r - 2);
                        ab.push(self.tmp("Transpose", &format!("{v}__adj"), vec![v], vec![attr_ints("perm", &p)]));
                    } else {
                        ab.push(v);
                    }
                }
                let out = self.out(o0)?;
                self.node("MatMul", ab, vec![out], vec![]);
            }
            // ---------------------------------------------------- pooling
            1 | 17 => {
                let same = opts.u8(0, 0)? == 0;
                let s = [opts.i(2, 1)?.max(1) as i64, opts.i(1, 1)?.max(1) as i64];
                let k = [opts.i(4, 1)?.max(1) as i64, opts.i(3, 1)?.max(1) as i64];
                let act = opts.u8(5, 0)?;
                let pads = self.window_pads(inp(0), k, s, [1, 1], same);
                let x = self.val(inp(0))?;
                let xt = self.nchw(x);
                let out = self.out(o0)?;
                let op = if o.code == 17 { "MaxPool" } else { "AveragePool" };
                let y = self.tmp(op, &format!("{out}__nchw"), vec![xt], vec![attr_ints("kernel_shape", &k), attr_ints("strides", &s), pads]);
                self.conv_tail(y, out, act)?;
            }
            // ---------------------------------------------------- elementwise
            0 => self.binary("Add", o, opts.u8(0, 0)?)?,
            41 => self.binary("Sub", o, opts.u8(0, 0)?)?,
            18 => self.binary("Mul", o, opts.u8(0, 0)?)?,
            42 => self.binary("Div", o, opts.u8(0, 0)?)?,
            78 => self.binary("Pow", o, 0)?,
            55 => self.binary("Max", o, 0)?,
            57 => self.binary("Min", o, 0)?,
            54 => self.binary("PRelu", o, 0)?,
            58 => self.binary("Less", o, 0)?,
            61 => self.binary("Greater", o, 0)?,
            62 => self.binary("GreaterOrEqual", o, 0)?,
            63 => self.binary("LessOrEqual", o, 0)?,
            71 => self.binary("Equal", o, 0)?,
            84 => self.binary("Or", o, 0)?,
            86 => self.binary("And", o, 0)?,
            72 => {
                let a = self.val(inp(0))?;
                let b = self.val(inp(1))?;
                let out = self.out(o0)?;
                let e = self.tmp("Equal", &format!("{out}__eq"), vec![a, b], vec![]);
                self.node("Not", vec![e], vec![out], vec![]);
            }
            99 => {
                // SQUARED_DIFFERENCE
                let a = self.val(inp(0))?;
                let b = self.val(inp(1))?;
                let out = self.out(o0)?;
                let d = self.tmp("Sub", &format!("{out}__diff"), vec![a, b], vec![]);
                self.node("Mul", vec![d.clone(), d], vec![out], vec![]);
            }
            92 => {
                let a = self.val(inp(0))?;
                let out = self.out(o0)?;
                self.node("Mul", vec![a.clone(), a], vec![out], vec![]);
            }
            76 => {
                let a = self.val(inp(0))?;
                let out = self.out(o0)?;
                let s = self.tmp("Sqrt", &format!("{out}__sqrt"), vec![a], vec![]);
                self.node("Reciprocal", vec![s], vec![out], vec![]);
            }
            106 => {
                let ins = o.inputs.iter().map(|&i| self.val(i)).collect::<Result<Vec<_>>>()?;
                let out = self.out(o0)?;
                self.node("Sum", ins, vec![out], vec![]);
            }
            64 | 123 => {
                let ins = (0..3).map(|k| self.val(inp(k))).collect::<Result<Vec<_>>>()?;
                let out = self.out(o0)?;
                self.node("Where", ins, vec![out], vec![]);
            }
            19 => self.unary("Relu", o, vec![])?,
            20 | 21 | 152 => {
                let (lo, hi) = match o.code {
                    20 => (-1.0, 1.0),
                    21 => (0.0, 6.0),
                    _ => (0.0, 1.0),
                };
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let a = self.scalar("clip_min", lo);
                let b = self.scalar("clip_max", hi);
                self.node("Clip", vec![x, a, b], vec![out], vec![]);
            }
            98 => self.unary("LeakyRelu", o, vec![attr_f("alpha", opts.f(0, 0.0)?)])?,
            117 => self.unary("HardSwish", o, vec![])?,
            14 => self.unary("Sigmoid", o, vec![])?,
            28 => self.unary("Tanh", o, vec![])?,
            150 => {
                let approx = opts.b(0)?;
                self.unary("Gelu", o, if approx { vec![attr_s("approximate", "tanh")] } else { vec![] })?
            }
            111 => self.unary("Elu", o, vec![attr_f("alpha", 1.0)])?,
            75 => self.unary("Sqrt", o, vec![])?,
            47 => self.unary("Exp", o, vec![])?,
            73 => self.unary("Log", o, vec![])?,
            101 => self.unary("Abs", o, vec![])?,
            59 => self.unary("Neg", o, vec![])?,
            8 => self.unary("Floor", o, vec![])?,
            104 => self.unary("Ceil", o, vec![])?,
            116 => self.unary("Round", o, vec![])?,
            66 => self.unary("Sin", o, vec![])?,
            108 => self.unary("Cos", o, vec![])?,
            87 => self.unary("Not", o, vec![])?,
            11 => {
                let act = opts.u8(0, 0)?;
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let dst = self.act_dst(&out, act);
                self.node("LpNormalization", vec![x], vec![dst.clone()], vec![attr_i("axis", -1), attr_i("p", 2)]);
                self.finish_act(dst, out, act)?;
            }
            25 | 50 => {
                let beta = if o.code == 25 && opts.0.and_then(|t| t.field(0)).is_some() { opts.f(0, 1.0)? } else { 1.0 };
                let mut x = self.val(inp(0))?;
                let out = self.out(o0)?;
                if beta != 1.0 {
                    let b = self.scalar("softmax_beta", beta);
                    x = self.tmp("Mul", &format!("{out}__beta"), vec![x, b], vec![]);
                }
                self.node(if o.code == 25 { "Softmax" } else { "LogSoftmax" }, vec![x], vec![out], vec![attr_i("axis", -1)]);
            }
            // ---------------------------------------------------- reductions
            40 => self.reduce("ReduceMean", o)?,
            74 => self.reduce("ReduceSum", o)?,
            82 => self.reduce("ReduceMax", o)?,
            89 => self.reduce("ReduceMin", o)?,
            81 => self.reduce("ReduceProd", o)?,
            56 | 79 => {
                let axis = self.need_i64(inp(1), "ARG_MAX/ARG_MIN axis")?;
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let op = if o.code == 56 { "ArgMax" } else { "ArgMin" };
                self.node(op, vec![x], vec![out], vec![attr_i("axis", axis[0]), attr_i("keepdims", 0)]);
            }
            // ---------------------------------------------------- padding
            34 | 60 | 100 => {
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let pads = match self.cst_i64(inp(1)) {
                    Some(p) => {
                        let r = p.len() / 2;
                        let mut v: Vec<i64> = (0..r).map(|i| p[2 * i]).collect();
                        v.extend((0..r).map(|i| p[2 * i + 1]));
                        self.i64s(&format!("{out}__pads"), &v)
                    }
                    None => {
                        let p = self.val(inp(1))?;
                        let t = self.tmp("Transpose", &format!("{out}__padsT"), vec![p], vec![attr_ints("perm", &[1, 0])]);
                        let s = self.i64s(&format!("{out}__flat"), &[-1]);
                        let f = self.tmp("Reshape", &format!("{out}__pads"), vec![t, s], vec![]);
                        self.tmp("Cast", &format!("{out}__pads64"), vec![f], vec![attr_i("to", 7)])
                    }
                };
                let mut ins = vec![x, pads];
                let mut attrs = Vec::new();
                if o.code == 60 && inp(2) >= 0 {
                    ins.push(self.val(inp(2))?);
                }
                if o.code == 100 {
                    if opts.u8(0, 0)? == 0 {
                        attrs.push(attr_s("mode", "reflect"));
                    } else {
                        // SYMMETRIC equals edge padding when every pad is <= 1
                        let p = self.cst_i64(inp(1)).unwrap_or_default();
                        crate::ensure!(!p.is_empty() && p.iter().all(|&v| v <= 1), "tflite: MIRROR_PAD SYMMETRIC with pads > 1 is not supported");
                        attrs.push(attr_s("mode", "edge"));
                    }
                }
                self.node("Pad", ins, vec![out], attrs);
            }
            // ---------------------------------------------------- shape ops
            22 => {
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let target = self.cst_i64(inp(1)).or(opts.ints(0)?.map(|v| v.into_iter().map(|d| d as i64).collect())).or_else(|| self.dims(o0));
                let shape = match target {
                    Some(t) if inp(1) < 0 || self.cst(inp(1)).is_some() => self.i64s(&format!("{out}__shape"), &t),
                    _ => self.val(inp(1))?,
                };
                self.node("Reshape", vec![x, shape], vec![out], vec![]);
            }
            43 => {
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let dims = opts.ints(0)?.unwrap_or_default();
                let mut ins = vec![x];
                if !dims.is_empty() {
                    let d: Vec<i64> = dims.iter().map(|&v| v as i64).collect();
                    ins.push(self.i64s(&format!("{out}__axes"), &d));
                }
                self.node("Squeeze", ins, vec![out], vec![]);
            }
            70 => {
                let axis = self.need_i64(inp(1), "EXPAND_DIMS axis")?;
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let a = self.i64s(&format!("{out}__axes"), &axis[..1]);
                self.node("Unsqueeze", vec![x, a], vec![out], vec![]);
            }
            2 => {
                let axis = opts.i(0, 0)? as i64;
                let act = opts.u8(1, 0)?;
                let ins = o.inputs.iter().map(|&i| self.val(i)).collect::<Result<Vec<_>>>()?;
                let out = self.out(o0)?;
                let dst = self.act_dst(&out, act);
                self.node("Concat", ins, vec![dst.clone()], vec![attr_i("axis", axis)]);
                self.finish_act(dst, out, act)?;
            }
            49 | 102 => {
                // SPLIT: (axis, x); SPLIT_V: (x, size_splits, axis)
                let (xi, ai) = if o.code == 49 { (inp(1), inp(0)) } else { (inp(0), inp(2)) };
                let axis = self.need_i64(ai, "SPLIT axis")?[0];
                let x = self.val(xi)?;
                let outs = o.outputs.iter().map(|&i| self.out(i)).collect::<Result<Vec<_>>>()?;
                let mut ins = vec![x];
                if o.code == 102 {
                    let mut sz = self.need_i64(inp(1), "SPLIT_V size_splits")?;
                    if let Some(neg) = sz.iter().position(|&v| v < 0) {
                        let d = self.dims(xi).ok_or_else(|| Error("tflite: SPLIT_V -1 needs a static shape".into()))?;
                        let r = d.len() as i64;
                        let total = d[(if axis < 0 { axis + r } else { axis }) as usize];
                        let known: i64 = sz.iter().filter(|&&v| v >= 0).sum();
                        sz[neg] = total - known;
                    }
                    ins.push(self.i64s(&format!("{}__split", outs[0]), &sz));
                }
                self.node("Split", ins, outs, vec![attr_i("axis", axis)]);
            }
            65 => {
                let begin = self.need_i64(inp(1), "SLICE begin")?;
                let size = self.need_i64(inp(2), "SLICE size")?;
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let ends: Vec<i64> = begin.iter().zip(&size).map(|(&b, &s)| if s < 0 { i64::MAX } else { b + s }).collect();
                let axes: Vec<i64> = (0..begin.len() as i64).collect();
                let st = self.i64s(&format!("{out}__starts"), &begin);
                let en = self.i64s(&format!("{out}__ends"), &ends);
                let ax = self.i64s(&format!("{out}__axes"), &axes);
                self.node("Slice", vec![x, st, en, ax], vec![out], vec![]);
            }
            45 => self.strided_slice(o)?,
            36 | 107 => {
                if o.code == 36 {
                    crate::ensure!(opts.i(1, 0)? == 0, "tflite: GATHER batch_dims is not supported");
                }
                let x = self.val(inp(0))?;
                let idx = self.val(inp(1))?;
                let out = self.out(o0)?;
                if o.code == 36 {
                    self.node("Gather", vec![x, idx], vec![out], vec![attr_i("axis", opts.i(0, 0)? as i64)]);
                } else {
                    self.node("GatherND", vec![x, idx], vec![out], vec![]);
                }
            }
            39 => {
                let perm = self.need_i64(inp(1), "TRANSPOSE perm")?;
                self.unary("Transpose", o, vec![attr_ints("perm", &perm)])?;
            }
            69 | 130 => {
                let x = self.val(inp(0))?;
                let r = self.val(inp(1))?;
                let out = self.out(o0)?;
                self.node(if o.code == 69 { "Tile" } else { "Expand" }, vec![x, r], vec![out], vec![]);
            }
            77 => self.unary("Shape", o, vec![])?,
            83 => {
                let axis = opts.i(1, 0)? as i64;
                let out = self.out(o0)?;
                let ax = self.i64s(&format!("{out}__axes"), &[axis]);
                let mut parts = Vec::new();
                for &i in &o.inputs {
                    let v = self.val(i)?;
                    parts.push(self.tmp("Unsqueeze", &format!("{v}__u"), vec![v, ax.clone()], vec![]));
                }
                self.node("Concat", parts, vec![out], vec![attr_i("axis", axis)]);
            }
            88 => {
                let axis = opts.i(1, 0)? as i64;
                let x = self.val(inp(0))?;
                let outs = o.outputs.iter().map(|&i| self.out(i)).collect::<Result<Vec<_>>>()?;
                let parts: Vec<String> = outs.iter().map(|n| self.fresh(&format!("{n}__part"))).collect();
                self.node("Split", vec![x], parts.clone(), vec![attr_i("axis", axis)]);
                let ax = self.i64s(&format!("{}__axes", outs[0]), &[axis]);
                for (p, n) in parts.into_iter().zip(outs) {
                    self.node("Squeeze", vec![p, ax.clone()], vec![n], vec![]);
                }
            }
            5 | 26 => {
                let bs = opts.i(0, 1)?.max(1) as i64;
                let x = self.val(inp(0))?;
                let xt = self.nchw(x);
                let out = self.out(o0)?;
                let (op, mut attrs) = if o.code == 5 { ("DepthToSpace", vec![attr_s("mode", "DCR")]) } else { ("SpaceToDepth", vec![]) };
                attrs.push(attr_i("blocksize", bs));
                let y = self.tmp(op, &format!("{out}__nchw"), vec![xt], attrs);
                self.conv_tail(y, out, 0)?;
            }
            // ---------------------------------------------------- resize
            23 | 97 => {
                let bilinear = o.code == 23;
                let (align, half) = if bilinear { (opts.b(2)?, opts.b(3)?) } else { (opts.b(0)?, opts.b(1)?) };
                let x = self.val(inp(0))?;
                let out = self.out(o0)?;
                let xd = self.dims(inp(0)).filter(|d| d.len() == 4 && d[0] > 0 && d[3] > 0);
                let (sizes, axes) = match (self.cst_i64(inp(1)), xd) {
                    (Some(hw), Some(d)) => (self.i64s(&format!("{out}__sizes"), &[d[0], hw[0], hw[1], d[3]]), None),
                    (Some(hw), None) => (self.i64s(&format!("{out}__sizes"), &hw), Some(attr_ints("axes", &[1, 2]))),
                    (None, _) => {
                        let s = self.val(inp(1))?;
                        (self.tmp("Cast", &format!("{out}__sizes"), vec![s], vec![attr_i("to", 7)]), Some(attr_ints("axes", &[1, 2])))
                    }
                };
                let mut attrs = vec![];
                if bilinear {
                    attrs.push(attr_s("mode", "linear"));
                    attrs.push(attr_s("coordinate_transformation_mode", if align { "align_corners" } else if half { "half_pixel" } else { "asymmetric" }));
                } else {
                    attrs.push(attr_s("mode", "nearest"));
                    let (coord, nearest) = if align {
                        ("align_corners", "round_prefer_ceil")
                    } else if half {
                        ("tf_half_pixel_for_nearest", "floor")
                    } else {
                        ("asymmetric", "floor")
                    };
                    attrs.push(attr_s("coordinate_transformation_mode", coord));
                    attrs.push(attr_s("nearest_mode", nearest));
                }
                attrs.extend(axes);
                self.node("Resize", vec![x, String::new(), String::new(), sizes], vec![out], attrs);
            }
            // ---------------------------------------------------- types
            6 => {
                // DEQUANTIZE: fold constants (fp16 or weight-only int8/uint8)
                let i = inp(0);
                if let Some(t) = self.cst(i) {
                    let src = self.tensor(i)?;
                    let v: Vec<f32> = match t.dtype() {
                        DType::F16 | DType::F32 => t.to_f32_vec(),
                        _ => {
                            let q = t.to_i64_vec();
                            crate::ensure!(!src.scale.is_empty(), "tflite: DEQUANTIZE of '{}' without quantization parameters", src.name);
                            let per_axis = src.scale.len() > 1;
                            let ax = src.qdim.max(0) as usize;
                            let inner: usize = t.shape.get(ax + 1..).map(|s| s.iter().product()).unwrap_or(1);
                            let dim = t.shape.get(ax).copied().unwrap_or(1).max(1);
                            q.iter()
                                .enumerate()
                                .map(|(k, &qv)| {
                                    let c = if per_axis { (k / inner) % dim } else { 0 };
                                    let s = src.scale.get(c).copied().unwrap_or(1.0);
                                    let z = src.zero_point.get(c).copied().unwrap_or(0);
                                    (qv - z) as f32 * s
                                })
                                .collect()
                        }
                    };
                    let shape = t.shape.clone();
                    self.consts.insert(o0 as usize, Tensor::f32(&shape, v));
                } else {
                    let src = self.tensor(i)?;
                    crate::ensure!(src.ty == 1 || src.ty == 0, "tflite: quantised models (DEQUANTIZE of {} activations) are not supported", type_name(src.ty));
                    self.unary("Cast", o, vec![attr_i("to", 1)])?;
                }
            }
            124 => {
                // DENSIFY: sparse constants are expanded at load; fold to the dense value.
                let t = self.cst(inp(0)).cloned().ok_or_else(|| Error("tflite: DENSIFY of a non-constant tensor".into()))?;
                self.consts.insert(o0 as usize, t);
            }
            114 => {
                let ot = self.tensor(o0)?.ty;
                crate::ensure!(ot == 0 || ot == 1, "tflite: quantised models (QUANTIZE to {}) are not supported", type_name(ot));
                self.unary("Identity", o, vec![])?;
            }
            53 => {
                let ot = self.tensor(o0)?.ty;
                let to = storage_dtype(ot).map(|d| if d == DType::F16 { DType::F32 } else { d }).ok_or_else(|| Error(format!("tflite: CAST to {} is not supported", type_name(ot))))?;
                self.unary("Cast", o, vec![attr_i("to", to.to_onnx() as i64)])?;
            }
            // ---------------------------------------------------- custom
            32 => self.custom(o)?,
            code => {
                // unknown builtin: keep it visible in the op report
                let ins = o.inputs.iter().map(|&i| self.val(i)).collect::<Result<Vec<_>>>()?;
                let outs = o.outputs.iter().map(|&i| self.out(i)).collect::<Result<Vec<_>>>()?;
                self.node_d("tflite", &builtin_name(code), ins, outs, vec![]);
            }
        }
        Ok(())
    }

    fn strided_slice(&mut self, o: &TOp) -> Result<()> {
        let opts = o.opts;
        let (bm, em, elm, nam, sam) = (opts.i(0, 0)?, opts.i(1, 0)?, opts.i(2, 0)?, opts.i(3, 0)?, opts.i(4, 0)?);
        let offset = opts.b(5)?;
        crate::ensure!(elm == 0 && nam == 0, "tflite: STRIDED_SLICE ellipsis/new_axis masks are not supported");
        let begin = self.need_i64(o.inputs[1], "STRIDED_SLICE begin")?;
        let end = self.need_i64(o.inputs[2], "STRIDED_SLICE end")?;
        let strides = match o.inputs.get(3).copied().filter(|&i| i >= 0) {
            Some(i) => self.need_i64(i, "STRIDED_SLICE strides")?,
            None => vec![1; begin.len()],
        };
        let r = begin.len();
        let (mut st, mut en, mut sp) = (vec![0i64; r], vec![0i64; r], vec![1i64; r]);
        let mut shrink = Vec::new();
        for i in 0..r {
            let s = strides[i];
            crate::ensure!(s != 0, "tflite: STRIDED_SLICE stride 0");
            let mut b = begin[i];
            let mut e = if offset { begin[i] + end[i] } else { end[i] };
            if bm >> i & 1 != 0 {
                b = if s > 0 { 0 } else { i64::MAX };
            }
            if em >> i & 1 != 0 {
                e = if s > 0 { i64::MAX } else { i64::MIN };
            }
            let mut s = s;
            if sam >> i & 1 != 0 {
                b = begin[i];
                e = if b == -1 { i64::MAX } else { b + 1 };
                s = 1;
                shrink.push(i as i64);
            }
            st[i] = b;
            en[i] = e;
            sp[i] = s;
        }
        let x = self.val(o.inputs[0])?;
        let out = self.out(o.outputs[0])?;
        let axes: Vec<i64> = (0..r as i64).collect();
        let a = self.i64s(&format!("{out}__starts"), &st);
        let b = self.i64s(&format!("{out}__ends"), &en);
        let c = self.i64s(&format!("{out}__axes"), &axes);
        let d = self.i64s(&format!("{out}__steps"), &sp);
        if shrink.is_empty() {
            self.node("Slice", vec![x, a, b, c, d], vec![out], vec![]);
        } else {
            let y = self.tmp("Slice", &format!("{out}__slice"), vec![x, a, b, c, d], vec![]);
            let ax = self.i64s(&format!("{out}__squeeze"), &shrink);
            self.node("Squeeze", vec![y, ax], vec![out], vec![]);
        }
        Ok(())
    }

    fn custom(&mut self, o: &TOp) -> Result<()> {
        // MediaPipe custom ops store a raw C struct in custom_options:
        // TfLiteTransposeConvParams {padding, stride_w, stride_h} and
        // TfLitePoolParams {padding, stride_w, stride_h, filter_w, filter_h, activation}
        // with TfLitePadding: 1 = SAME, 2 = VALID.
        let c = o.custom_opts;
        let word = |k: usize, def: i32| -> i32 { c.get(4 * k..4 * k + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap())).unwrap_or(def) };
        let inp = |k: usize| o.inputs.get(k).copied().unwrap_or(-1);
        match o.custom.as_str() {
            "Convolution2DTransposeBias" => {
                let same = word(0, 1) != 2;
                let s = [word(2, 1).max(1) as i64, word(1, 1).max(1) as i64];
                let out = self.out(o.outputs[0])?;
                let os = self.dims(o.outputs[0]).filter(|d| d.len() == 4 && d[1] > 0 && d[2] > 0).map(|d| [d[1], d[2]]);
                self.transpose_conv(inp(0), inp(1), inp(2), s, same, os, 0, out)?;
            }
            "MaxPoolingWithArgmax2D" | "MaxUnpooling2D" => {
                let same = word(0, 2) == 1;
                let attrs = vec![
                    attr_ints("kernel_shape", &[word(4, 2).max(1) as i64, word(3, 2).max(1) as i64]),
                    attr_ints("strides", &[word(2, 2).max(1) as i64, word(1, 2).max(1) as i64]),
                    attr_s("padding", if same { "SAME" } else { "VALID" }),
                ];
                let ins = o.inputs.iter().map(|&i| self.val(i)).collect::<Result<Vec<_>>>()?;
                let outs = o.outputs.iter().map(|&i| self.out(i)).collect::<Result<Vec<_>>>()?;
                self.node_d("mediapipe", &o.custom, ins, outs, attrs);
                self.mediapipe = true;
            }
            other => {
                let ins = o.inputs.iter().map(|&i| self.val(i)).collect::<Result<Vec<_>>>()?;
                let outs = o.outputs.iter().map(|&i| self.out(i)).collect::<Result<Vec<_>>>()?;
                let name = if other.is_empty() { "CUSTOM".to_string() } else { sanitize(other) };
                self.node_d("tflite", &name, ins, outs, vec![]);
            }
        }
        Ok(())
    }

    fn value_info(&self, i: usize) -> ValueInfo {
        let t = &self.t[i];
        let elem_type = storage_dtype(t.ty).map(|d| d.to_onnx()).unwrap_or(0);
        let shape = t
            .shape
            .iter()
            .enumerate()
            .map(|(k, &d)| if t.sig.get(k).is_some_and(|&s| s < 0) || d < 0 { Dim::Param(format!("{}_d{k}", self.names[i])) } else { Dim::Value(d) })
            .collect();
        ValueInfo { name: self.names[i].clone(), elem_type, shape: Some(shape) }
    }
}

fn is_quantized_activation(t: &TTensor) -> bool {
    matches!(t.ty, 3 | 9 | 7) && !t.scale.is_empty()
}

/// Parses TFLite flatbuffer bytes into a [`Model`] (first subgraph).
pub fn from_bytes(bytes: &[u8]) -> Result<Model> {
    crate::ensure!(is_tflite(bytes), "tflite: missing TFL3 file identifier");
    let root = Table::root(bytes)?;
    let version = root.u32(0, 0)?;
    // operator codes
    let mut codes: Vec<(i32, String)> = Vec::new();
    for oc in root.tables(1)? {
        let dep = oc.u8(0, 0)? as i8 as i32;
        let b = oc.i32(3, 0)?;
        codes.push((dep.max(b), oc.string(1)?.unwrap_or_default()));
    }
    // buffers
    let mut bufs: Vec<&[u8]> = Vec::new();
    for b in root.tables(4)? {
        let data = b.bytes(0)?;
        if data.is_empty() {
            let (off, size) = (b.u64(1, 0)? as usize, b.u64(2, 0)? as usize);
            if off > 1 && size > 0 {
                let end = off.checked_add(size).filter(|&e| e <= bytes.len()).ok_or_else(|| Error("tflite: external buffer out of range".into()))?;
                bufs.push(&bytes[off..end]);
                continue;
            }
        }
        bufs.push(data);
    }
    let subgraphs = root.tables(2)?;
    let sg = *subgraphs.first().ok_or_else(|| Error("tflite: no subgraphs".into()))?;
    // tensors
    let mut tensors = Vec::new();
    for (k, t) in sg.tables(0)?.into_iter().enumerate() {
        let q = t.table(4)?;
        let (scale, zero_point, qdim) = match q {
            Some(q) => (q.f32s(2)?, q.i64s(3)?, q.i32(6, 0)?),
            None => (Vec::new(), Vec::new(), 0),
        };
        tensors.push(TTensor {
            name: t.string(3)?.filter(|s| !s.is_empty()).unwrap_or_else(|| format!("t{k}")),
            shape: t.i32s(0)?.into_iter().map(|v| v as i64).collect(),
            sig: t.i32s(7)?.into_iter().map(|v| v as i64).collect(),
            ty: t.u8(1, 0)?,
            buffer: t.u32(2, 0)? as usize,
            scale,
            zero_point,
            qdim,
            sparsity: read_sparsity(&t)?,
        });
    }
    let inputs = sg.i32s(1)?;
    let outputs = sg.i32s(2)?;
    let mut ops = Vec::new();
    for op in sg.tables(3)? {
        let idx = op.u32(0, 0)? as usize;
        let (code, custom) = codes.get(idx).cloned().ok_or_else(|| Error(format!("tflite: opcode index {idx} out of range")))?;
        let mut custom_opts = op.bytes(5)?;
        if custom_opts.is_empty() {
            let (off, size) = (op.u64(9, 0)? as usize, op.u64(10, 0)? as usize);
            if off > 0 && size > 0 && off.checked_add(size).is_some_and(|e| e <= bytes.len()) {
                custom_opts = &bytes[off..off + size];
            }
        }
        ops.push(TOp { code, custom, inputs: op.i32s(1)?, outputs: op.i32s(2)?, opts: Opts(op.table(4)?), custom_opts });
    }

    let mut imp = Imp { t: tensors, bufs, names: Vec::new(), consts: HashMap::new(), emitted: HashSet::new(), weights: HashMap::new(), used: HashSet::new(), g: Graph::default(), mediapipe: false };
    imp.g.name = sg.string(4)?.filter(|s| !s.is_empty()).unwrap_or_else(|| "main".into());
    for k in 0..imp.t.len() {
        let n = imp.t[k].name.clone();
        let n = imp.fresh(&n);
        imp.names.push(n);
    }
    // constants
    let input_set: HashSet<i32> = inputs.iter().copied().collect();
    for k in 0..imp.t.len() {
        let t = &imp.t[k];
        let data = imp.bufs.get(t.buffer).copied().unwrap_or(&[]);
        if t.buffer == 0 || data.is_empty() || input_set.contains(&(k as i32)) {
            continue;
        }
        let c = match &t.sparsity {
            Some(sp) => Some(decode_sparse(t, data, sp)?),
            None => decode_const(t, data).ok(),
        };
        if let Some(c) = c {
            imp.consts.insert(k, c);
        }
    }
    // graph inputs
    for &i in &inputs {
        let t = imp.tensor(i)?;
        crate::ensure!(!is_quantized_activation(t), "tflite: quantised (int8/uint8) models are not supported (input '{}' is {})", t.name, type_name(t.ty));
        crate::ensure!(storage_dtype(t.ty).is_some(), "tflite: input '{}' has unsupported type {}", t.name, type_name(t.ty));
        let vi = imp.value_info(i as usize);
        imp.g.inputs.push(vi);
    }
    for op in &ops {
        for &i in &op.outputs {
            if i >= 0 {
                let t = imp.tensor(i)?;
                crate::ensure!(!is_quantized_activation(t), "tflite: quantised (int8/uint8) models are not supported (tensor '{}' is {})", t.name, type_name(t.ty));
            }
        }
        imp.import_op(op).map_err(|e| Error(format!("{e} (op {} -> '{}')", if op.code == 32 { op.custom.clone() } else { builtin_name(op.code) }, op.outputs.first().and_then(|&i| imp.t.get(i as usize)).map(|t| t.name.as_str()).unwrap_or("?"))))?;
    }
    for &i in &outputs {
        imp.val(i)?;
        let vi = imp.value_info(i as usize);
        imp.g.outputs.push(vi);
    }

    let mut opset_import = vec![OpsetId { domain: String::new(), version: 17 }];
    if imp.mediapipe {
        opset_import.push(OpsetId { domain: "mediapipe".into(), version: 1 });
    }
    let mut metadata = vec![("tflite.schema_version".to_string(), version.to_string())];
    if subgraphs.len() > 1 {
        metadata.push(("tflite.subgraphs".into(), subgraphs.len().to_string()));
    }
    for m in root.tables(6)? {
        let name = m.string(0)?.unwrap_or_default();
        let b = m.u32(1, 0)? as usize;
        let data = imp.bufs.get(b).copied().unwrap_or(&[]);
        let text = if data.len() <= 64 && !data.is_empty() && data.iter().all(|&c| c.is_ascii_graphic() || c == b' ') { String::from_utf8_lossy(data).into_owned() } else { format!("<{} bytes>", data.len()) };
        metadata.push((format!("tflite.metadata.{name}"), text));
    }
    Ok(Model {
        ir_version: 8,
        opset_import,
        producer_name: "tflite".into(),
        producer_version: version.to_string(),
        doc_string: root.string(3)?.unwrap_or_default(),
        metadata,
        graph: imp.g,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_padding_matches_tflite() {
        // in 10, k 3, s 2 -> out 5, total 1 -> (0, 1)
        assert_eq!(same_pad(10, 3, 2, 1), (0, 1));
        assert_eq!(same_pad(9, 3, 2, 1), (1, 1));
        assert_eq!(same_pad(5, 1, 1, 1), (0, 0));
        assert_eq!(same_pad(7, 3, 1, 2), (2, 2));
        assert_eq!(same_pad(4, 2, 2, 1), (0, 0));
    }

    #[test]
    fn densify_matches_tflite_format_converter() {
        // Dense × CSR (TFLite sparsity_format_converter_test SimpleTestD0S1).
        let sp = Sparsity { order: vec![0, 1], block_map: vec![], dims: vec![SparseDim::Dense(3), SparseDim::Csr { segments: vec![0, 3, 3, 5], indices: vec![0, 2, 3, 0, 3] }] };
        assert_eq!(densify(&[6.0f32, 9.0, 8.0, 5.0, 7.0], &[3, 4], &sp).unwrap(), vec![6.0, 0.0, 9.0, 8.0, 0.0, 0.0, 0.0, 0.0, 5.0, 0.0, 0.0, 7.0]);
        // 2×2 blocks, block rows CSR (BlockTestD0S1).
        let sp = Sparsity {
            order: vec![0, 1, 2, 3],
            block_map: vec![0, 1],
            dims: vec![SparseDim::Dense(2), SparseDim::Csr { segments: vec![0, 2, 3], indices: vec![0, 1, 1] }, SparseDim::Dense(2), SparseDim::Dense(2)],
        };
        let got = densify(&[1i64, 0, 0, 4, 2, 3, 0, 0, 5, 0, 0, 6], &[4, 4], &sp).unwrap();
        assert_eq!(got, vec![1, 0, 2, 3, 0, 4, 0, 0, 0, 0, 5, 0, 0, 0, 0, 6]);
        // Malformed parameters are errors, not panics.
        let bad = Sparsity { order: vec![0, 1], block_map: vec![], dims: vec![SparseDim::Dense(3), SparseDim::Csr { segments: vec![0, 9], indices: vec![0] }] };
        assert!(densify(&[1.0f32], &[3, 4], &bad).is_err());
    }

    #[test]
    fn rejects_non_tflite() {
        assert!(from_bytes(b"not a model").is_err());
        assert!(from_bytes(&[8, 0, 0, 0, b'T', b'F', b'L', b'3']).is_err());
    }
}
