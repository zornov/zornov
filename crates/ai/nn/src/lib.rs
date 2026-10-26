// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio NN: a hand-written neural-network inference engine.
//!
//! * [`proto`] — protobuf wire-format reader/writer.
//! * [`onnx`] — ONNX `ModelProto` parser and encoder (plus [`onnx::GraphBuilder`]
//!   for building models in code).
//! * [`safetensors`] — safetensors weight loader.
//! * [`Model`] — a loaded model with metadata and op-coverage reporting.
//! * [`Session`] — a compiled, optimised execution plan (topological schedule,
//!   constant folding, BatchNorm folding, weight pre-packing, memory reuse).
//! * [`ops`] — the operator set; [`kernels`] — GEMM / convolution kernels.
//! * [`backend`] — the [`backend::Backend`] trait, CPU and Metal backends.
//! * [`image`] / [`audio`] — pre/post-processing helpers (NCHW conversion,
//!   STFT, Whisper log-mel).
//!
//! No third-party crates are used.

pub mod audio;
pub mod backend;
pub mod error;
pub mod image;
pub mod json;
pub mod kernels;
pub mod model;
pub mod onnx;
pub mod ops;
pub mod proto;
pub mod safetensors;
pub mod session;
pub mod shape;
pub mod tensor;
pub mod tflite;
pub mod zip;

pub use backend::BackendKind;
pub use error::{Error, Result};
pub use model::Model;
pub use session::{Options, Session};
pub use tensor::{DType, Data, Tensor};
