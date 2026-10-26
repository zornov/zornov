// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A loaded model: ONNX metadata plus the graph, with op-coverage reporting.

use crate::error::{Error, Result};
use crate::onnx::{self, Graph, OpsetId, ValueInfo};
use crate::tensor::Tensor;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub ir_version: i64,
    pub opset_import: Vec<OpsetId>,
    pub producer_name: String,
    pub producer_version: String,
    pub domain: String,
    pub model_version: i64,
    pub doc_string: String,
    pub metadata: Vec<(String, String)>,
    pub graph: Graph,
}

impl Model {
    /// Loads `.onnx` (external data resolved next to the file), `.tflite`, or
    /// a MediaPipe `.task` bundle (its first/largest `.tflite`; see
    /// [`crate::tflite::load_task`] to pick another member).
    pub fn load(path: impl AsRef<Path>) -> Result<Model> {
        let path = path.as_ref();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "tflite" => crate::tflite::load(path),
            "task" => {
                let models = crate::tflite::load_task(path)?;
                models.into_iter().map(|(_, m)| m).next().ok_or_else(|| Error(format!("{}: no .tflite inside", path.display())))
            }
            _ => {
                let bytes = std::fs::read(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
                if crate::tflite::is_tflite(&bytes) {
                    return crate::tflite::from_bytes(&bytes);
                }
                onnx::decode_model(&bytes, path.parent())
            }
        }
    }

    /// Parses ONNX bytes (external data is an error: use [`Model::from_bytes_with_dir`]).
    pub fn from_bytes(bytes: &[u8]) -> Result<Model> {
        if crate::tflite::is_tflite(bytes) {
            return crate::tflite::from_bytes(bytes);
        }
        onnx::decode_model(bytes, None)
    }

    pub fn from_bytes_with_dir(bytes: &[u8], dir: &Path) -> Result<Model> {
        onnx::decode_model(bytes, Some(dir))
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        onnx::encode_model(self, None)
    }

    /// Opset version for a domain ("" = default ONNX domain).
    pub fn opset_version(&self, domain: &str) -> i64 {
        let d = crate::ops::normalize_domain(domain);
        self.opset_import.iter().find(|o| crate::ops::normalize_domain(&o.domain) == d).map(|o| o.version).unwrap_or(if d.is_empty() { 13 } else { 1 })
    }

    pub fn inputs(&self) -> Vec<&ValueInfo> {
        let inits: std::collections::HashSet<&str> = self.graph.initializers.iter().map(|(n, _)| n.as_str()).collect();
        self.graph.inputs.iter().filter(|v| !inits.contains(v.name.as_str())).collect()
    }

    pub fn outputs(&self) -> &[ValueInfo] {
        &self.graph.outputs
    }

    /// `(op, count)` for every op type in the graph (and subgraphs), sorted by name.
    /// Ops outside the default domain are written `domain::Op`.
    pub fn op_histogram(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = Default::default();
        fn walk(g: &Graph, m: &mut std::collections::BTreeMap<String, usize>) {
            for n in &g.nodes {
                let d = crate::ops::normalize_domain(&n.domain);
                let key = if d.is_empty() { n.op_type.clone() } else { format!("{d}::{}", n.op_type) };
                *m.entry(key).or_default() += 1;
                for a in &n.attrs {
                    match &a.value {
                        onnx::AttrValue::Graph(sg) => walk(sg, m),
                        onnx::AttrValue::Graphs(gs) => gs.iter().for_each(|sg| walk(sg, m)),
                        _ => {}
                    }
                }
            }
        }
        walk(&self.graph, &mut m);
        m.into_iter().collect()
    }

    /// Op types the engine cannot run (empty = fully covered).
    pub fn unsupported_ops(&self) -> Vec<String> {
        self.op_histogram()
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| {
                let (d, o) = k.split_once("::").unwrap_or(("", k));
                !crate::ops::is_supported(d, o)
            })
            .collect()
    }

    pub fn initializer(&self, name: &str) -> Option<&Tensor> {
        self.graph.initializers.iter().find(|(n, _)| n == name).map(|(_, t)| t)
    }

    /// Replaces or adds initializers by name (e.g. weights from safetensors).
    pub fn bind_initializers<'a>(&mut self, weights: impl IntoIterator<Item = (&'a str, Tensor)>) {
        for (name, t) in weights {
            if let Some(slot) = self.graph.initializers.iter_mut().find(|(n, _)| n == name) {
                slot.1 = t;
            } else {
                self.graph.initializers.push((name.to_string(), t));
            }
        }
    }

    /// Total parameter count over all initializers.
    pub fn parameter_count(&self) -> usize {
        self.graph.initializers.iter().map(|(_, t)| t.len()).sum()
    }

    /// Human-readable metadata and op-coverage report.
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "producer: {} {}  ir_version: {}", self.producer_name, self.producer_version, self.ir_version);
        for o in &self.opset_import {
            let _ = writeln!(s, "opset: {} v{}", if o.domain.is_empty() { "ai.onnx" } else { &o.domain }, o.version);
        }
        for (k, v) in &self.metadata {
            let _ = writeln!(s, "meta: {k} = {v}");
        }
        let _ = writeln!(s, "nodes: {}  initializers: {}  parameters: {}", self.graph.nodes.len(), self.graph.initializers.len(), self.parameter_count());
        let dims = |v: &ValueInfo| match &v.shape {
            None => "?".to_string(),
            Some(d) => format!(
                "[{}]",
                d.iter()
                    .map(|x| match x {
                        onnx::Dim::Value(v) => v.to_string(),
                        onnx::Dim::Param(p) => p.clone(),
                        onnx::Dim::Unknown => "?".into(),
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        };
        for v in self.inputs() {
            let _ = writeln!(s, "input: {} {:?} {}", v.name, v.dtype(), dims(v));
        }
        for v in self.outputs() {
            let _ = writeln!(s, "output: {} {:?} {}", v.name, v.dtype(), dims(v));
        }
        let hist = self.op_histogram();
        let _ = writeln!(s, "ops ({} types): {}", hist.len(), hist.iter().map(|(k, n)| format!("{k}×{n}")).collect::<Vec<_>>().join(" "));
        let un = self.unsupported_ops();
        let _ = writeln!(s, "unsupported: {}", if un.is_empty() { "none".to_string() } else { un.join(", ") });
        s
    }
}
