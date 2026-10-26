// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The operator registry. Every op is a plain function
//! `fn(&OpCtx, &Node, &[Option<&Tensor>]) -> Result<Vec<Tensor>>`; optional
//! inputs are `None`. Ops compute in f32 (f16 storage is widened on read).

pub mod elementwise;
pub mod misc;
pub mod nn;
pub mod quant;
pub mod reduce;
pub mod resize;
pub mod rnn;
pub mod shape_ops;

use crate::error::{Error, Result};
use crate::kernels::gemm::{PackedA, PackedB, pack_a, pack_b};
use crate::onnx::Node;
use crate::tensor::Tensor;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Weights pre-packed at session build time.
#[derive(Debug)]
pub enum Prepacked {
    /// Packed B operand of MatMul / Gemm.
    B(PackedB),
    /// f32 conv weights plus one packed A per group (empty for depthwise).
    Conv { w: Vec<f32>, packed: Vec<PackedA> },
}

pub struct OpCtx<'a> {
    /// Opset version of the node's domain.
    pub opset: i64,
    pub threads: usize,
    pub cache: Option<&'a Prepacked>,
}

pub type OpFn = fn(&OpCtx, &Node, &[Option<&Tensor>]) -> Result<Vec<Tensor>>;

pub fn inp<'a>(inputs: &[Option<&'a Tensor>], i: usize) -> Result<&'a Tensor> {
    inputs.get(i).copied().flatten().ok_or_else(|| Error(format!("missing required input {i}")))
}

pub fn opt<'a>(inputs: &[Option<&'a Tensor>], i: usize) -> Option<&'a Tensor> {
    inputs.get(i).copied().flatten()
}

const STANDARD: &[(&str, OpFn)] = &[
    // activations / unary math
    ("Relu", elementwise::unary),
    ("LeakyRelu", elementwise::unary),
    ("Sigmoid", elementwise::unary),
    ("Tanh", elementwise::unary),
    ("HardSigmoid", elementwise::unary),
    ("HardSwish", elementwise::unary),
    ("Gelu", elementwise::unary),
    ("Erf", elementwise::unary),
    ("Softplus", elementwise::unary),
    ("Softsign", elementwise::unary),
    ("Mish", elementwise::unary),
    ("Elu", elementwise::unary),
    ("Selu", elementwise::unary),
    ("Celu", elementwise::unary),
    ("ThresholdedRelu", elementwise::unary),
    ("Sqrt", elementwise::unary),
    ("Exp", elementwise::unary),
    ("Log", elementwise::unary),
    ("Abs", elementwise::unary),
    ("Neg", elementwise::unary),
    ("Reciprocal", elementwise::unary),
    ("Floor", elementwise::unary),
    ("Ceil", elementwise::unary),
    ("Round", elementwise::unary),
    ("Sign", elementwise::unary),
    ("Sin", elementwise::unary),
    ("Cos", elementwise::unary),
    ("Tan", elementwise::unary),
    ("Asin", elementwise::unary),
    ("Acos", elementwise::unary),
    ("Atan", elementwise::unary),
    ("Sinh", elementwise::unary),
    ("Cosh", elementwise::unary),
    ("Asinh", elementwise::unary),
    ("Acosh", elementwise::unary),
    ("Atanh", elementwise::unary),
    ("IsNaN", elementwise::is_nan),
    ("IsInf", elementwise::is_inf),
    ("Not", elementwise::not),
    ("Clip", elementwise::clip),
    // binary / variadic arithmetic
    ("Add", elementwise::binary),
    ("Sub", elementwise::binary),
    ("Mul", elementwise::binary),
    ("Div", elementwise::binary),
    ("Pow", elementwise::binary),
    ("Mod", elementwise::binary),
    ("PRelu", elementwise::binary),
    ("Max", elementwise::variadic),
    ("Min", elementwise::variadic),
    ("Sum", elementwise::variadic),
    ("Mean", elementwise::variadic),
    ("Equal", elementwise::compare),
    ("Less", elementwise::compare),
    ("Greater", elementwise::compare),
    ("LessOrEqual", elementwise::compare),
    ("GreaterOrEqual", elementwise::compare),
    ("And", elementwise::logic),
    ("Or", elementwise::logic),
    ("Xor", elementwise::logic),
    ("Where", elementwise::where_),
    ("Cast", elementwise::cast),
    ("CastLike", elementwise::cast_like),
    // shape / data movement
    ("Reshape", shape_ops::alias_op),
    ("Flatten", shape_ops::alias_op),
    ("Squeeze", shape_ops::alias_op),
    ("Unsqueeze", shape_ops::alias_op),
    ("Identity", shape_ops::alias_op),
    ("Dropout", shape_ops::alias_op),
    ("Transpose", shape_ops::transpose),
    ("Concat", shape_ops::concat),
    ("Split", shape_ops::split),
    ("Slice", shape_ops::slice),
    ("Gather", shape_ops::gather),
    ("GatherElements", shape_ops::gather_elements),
    ("GatherND", shape_ops::gather_nd),
    ("ScatterND", shape_ops::scatter_nd),
    ("ScatterElements", shape_ops::scatter_elements),
    ("Scatter", shape_ops::scatter_elements),
    ("Expand", shape_ops::expand),
    ("Tile", shape_ops::tile),
    ("Pad", shape_ops::pad),
    ("Shape", shape_ops::shape),
    ("Size", shape_ops::size),
    ("ConstantOfShape", shape_ops::constant_of_shape),
    ("Constant", shape_ops::constant),
    ("Range", shape_ops::range),
    ("DepthToSpace", shape_ops::depth_to_space),
    ("SpaceToDepth", shape_ops::space_to_depth),
    ("Trilu", shape_ops::trilu),
    ("OneHot", shape_ops::one_hot),
    ("NonZero", shape_ops::non_zero),
    ("CumSum", shape_ops::cum_sum),
    ("TopK", shape_ops::top_k),
    // neural network
    ("Conv", nn::conv),
    ("ConvTranspose", nn::conv_transpose_op),
    ("MaxPool", nn::pool),
    ("AveragePool", nn::pool),
    ("LpPool", nn::pool),
    ("GlobalAveragePool", nn::global_pool),
    ("GlobalMaxPool", nn::global_pool),
    ("GlobalLpPool", nn::global_pool),
    ("BatchNormalization", nn::batch_norm),
    ("InstanceNormalization", nn::instance_norm),
    ("LayerNormalization", nn::layer_norm),
    ("GroupNormalization", nn::group_norm),
    ("LpNormalization", nn::lp_norm),
    ("Softmax", nn::softmax),
    ("LogSoftmax", nn::softmax),
    ("Hardmax", nn::softmax),
    ("MatMul", nn::matmul),
    ("Gemm", nn::gemm_op),
    // reductions
    ("ReduceMean", reduce::reduce),
    ("ReduceSum", reduce::reduce),
    ("ReduceMax", reduce::reduce),
    ("ReduceMin", reduce::reduce),
    ("ReduceProd", reduce::reduce),
    ("ReduceL1", reduce::reduce),
    ("ReduceL2", reduce::reduce),
    ("ReduceLogSum", reduce::reduce),
    ("ReduceLogSumExp", reduce::reduce),
    ("ReduceSumSquare", reduce::reduce),
    ("ArgMax", reduce::arg_reduce),
    ("ArgMin", reduce::arg_reduce),
    // sampling
    ("Resize", resize::resize),
    ("Upsample", resize::resize),
    ("GridSample", resize::grid_sample),
    // recurrent
    ("LSTM", rnn::rnn_op),
    ("GRU", rnn::rnn_op),
    ("RNN", rnn::rnn_op),
    // quantization
    ("QuantizeLinear", quant::quantize_linear),
    ("DequantizeLinear", quant::dequantize_linear),
    ("DynamicQuantizeLinear", quant::dynamic_quantize_linear),
    ("MatMulInteger", quant::matmul_integer),
    ("ConvInteger", quant::conv_integer),
    ("QLinearMatMul", quant::qlinear_matmul),
    ("QLinearConv", quant::qlinear_conv),
    // misc
    ("Einsum", misc::einsum),
    ("DFT", misc::dft),
    ("DeformConv", misc::deform_conv),
];

/// onnxruntime `com.microsoft` contrib ops.
const CONTRIB: &[(&str, OpFn)] = &[
    ("Gelu", misc::bias_gelu),
    ("BiasGelu", misc::bias_gelu),
    ("FastGelu", misc::bias_gelu),
    ("QuickGelu", elementwise::unary),
    ("SkipLayerNormalization", misc::skip_layer_norm),
    ("MultiHeadAttention", misc::multi_head_attention),
    ("MatMulIntegerToFloat", quant::matmul_integer_to_float),
    ("DynamicQuantizeMatMul", quant::dynamic_quantize_matmul),
    ("QuantizeLinear", quant::quantize_linear),
    ("DequantizeLinear", quant::dequantize_linear),
];

/// MediaPipe custom ops (from the TFLite importer).
const MEDIAPIPE: &[(&str, OpFn)] = &[("MaxPoolingWithArgmax2D", misc::max_pool_argmax_nhwc), ("MaxUnpooling2D", misc::max_unpool_nhwc)];

/// Ops implemented by the session itself rather than a kernel.
pub const CONTROL_FLOW: &[&str] = &["If"];

fn table() -> &'static HashMap<(&'static str, &'static str), OpFn> {
    static T: OnceLock<HashMap<(&'static str, &'static str), OpFn>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = HashMap::new();
        for &(n, f) in STANDARD {
            m.insert(("", n), f);
        }
        for &(n, f) in CONTRIB {
            m.insert(("com.microsoft", n), f);
        }
        for &(n, f) in MEDIAPIPE {
            m.insert(("mediapipe", n), f);
        }
        m
    })
}

pub fn normalize_domain(d: &str) -> &str {
    if d == "ai.onnx" { "" } else { d }
}

pub fn lookup(domain: &str, op: &str) -> Option<OpFn> {
    table().get(&(normalize_domain(domain), op)).copied()
}

pub fn is_supported(domain: &str, op: &str) -> bool {
    lookup(domain, op).is_some() || (normalize_domain(domain).is_empty() && CONTROL_FLOW.contains(&op))
}

/// Every supported `(domain, op_type)`, sorted.
pub fn supported_ops() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = table().keys().map(|(d, o)| (d.to_string(), o.to_string())).collect();
    for c in CONTROL_FLOW {
        v.push((String::new(), c.to_string()));
    }
    v.sort();
    v
}

/// Ops that only move or reinterpret data; the executor can reuse the input buffer.
pub fn is_alias_op(domain: &str, op: &str) -> bool {
    normalize_domain(domain).is_empty() && matches!(op, "Reshape" | "Flatten" | "Squeeze" | "Unsqueeze" | "Identity")
}

/// Pre-packs constant weights for MatMul / Gemm / Conv.
pub fn prepack(node: &Node, inputs: &[Option<&Tensor>], threads: usize) -> Option<Prepacked> {
    if !normalize_domain(&node.domain).is_empty() {
        return None;
    }
    match node.op_type.as_str() {
        "MatMul" => {
            let b = opt(inputs, 1)?;
            if b.rank() != 2 || !b.dtype().is_float() {
                return None;
            }
            let (k, n) = (b.shape[0], b.shape[1]);
            Some(Prepacked::B(pack_b(k, n, &b.f32s(), n, 1, threads)))
        }
        "Gemm" => {
            let b = opt(inputs, 1)?;
            if b.rank() != 2 || !b.dtype().is_float() {
                return None;
            }
            Some(Prepacked::B(nn::pack_gemm_b(b, node.attr_i("transB", 0) != 0, threads)))
        }
        "Conv" => {
            let w = opt(inputs, 1)?;
            if w.rank() < 3 || !w.dtype().is_float() {
                return None;
            }
            let group = node.attr_i("group", 1).max(1) as usize;
            let wv = w.to_f32_vec();
            let depthwise = w.rank() == 4 && w.shape[1] == 1 && w.shape[0] == group;
            let packed = if depthwise {
                Vec::new()
            } else {
                let ocg = w.shape[0] / group;
                let kk: usize = w.shape[1..].iter().product();
                (0..group).map(|g| pack_a(ocg, kk, &wv[g * ocg * kk..(g + 1) * ocg * kk], kk, 1)).collect()
            };
            Some(Prepacked::Conv { w: wv, packed })
        }
        _ => None,
    }
}
