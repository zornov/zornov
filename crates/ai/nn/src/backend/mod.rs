// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Execution backends. The CPU backend runs every op in [`crate::ops`]; an
//! accelerator backend (Metal) claims the heavy ops it implements and returns
//! `None` for everything else, which then falls back to the CPU per op.

use crate::error::Result;
use crate::onnx::Node;
use crate::ops::OpCtx;
use crate::tensor::Tensor;

#[cfg(target_os = "macos")]
pub mod metal;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BackendKind {
    #[default]
    Cpu,
    /// Metal for GEMM / Conv / large element-wise ops, CPU for the rest.
    /// Session creation fails if no Metal device exists.
    Metal,
    /// Metal when available, else CPU.
    Auto,
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Runs `node` if this backend supports it for these inputs; `None` means
    /// "not handled here" and the caller falls back to the CPU kernel.
    fn try_run(&self, node: &Node, inputs: &[Option<&Tensor>], ctx: &OpCtx) -> Option<Result<Vec<Tensor>>>;
}

/// The CPU backend: the operator registry.
pub struct Cpu;

impl Backend for Cpu {
    fn name(&self) -> &'static str {
        "cpu"
    }
    fn try_run(&self, node: &Node, inputs: &[Option<&Tensor>], ctx: &OpCtx) -> Option<Result<Vec<Tensor>>> {
        crate::ops::lookup(&node.domain, &node.op_type).map(|f| f(ctx, node, inputs))
    }
}

/// True when a Metal device can be created.
pub fn metal_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        metal::Metal::shared().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The accelerator backend for `kind`, if any. `min_flops` is the smallest
/// GEMM/conv work (in FLOPs) worth sending to the GPU.
pub fn accelerator(kind: BackendKind, min_flops: f64) -> Result<Option<Box<dyn Backend>>> {
    match kind {
        BackendKind::Cpu => Ok(None),
        #[cfg(target_os = "macos")]
        BackendKind::Metal | BackendKind::Auto => match metal::MetalBackend::new(min_flops) {
            Some(b) => Ok(Some(Box::new(b))),
            None if kind == BackendKind::Auto => Ok(None),
            None => Err(crate::error::Error("Metal backend requested but no Metal device is available".into())),
        },
        #[cfg(not(target_os = "macos"))]
        BackendKind::Metal => Err(crate::error::Error("Metal is only available on macOS".into())),
        #[cfg(not(target_os = "macos"))]
        BackendKind::Auto => {
            let _ = min_flops;
            Ok(None)
        }
    }
}
