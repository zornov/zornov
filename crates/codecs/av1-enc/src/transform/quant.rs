// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M3: quantisation (spec 7.12.2 inverted).

use crate::types::{QuantParams, TxSize};

/// Quantises the 64x64-stride `coef` block; returns the levels in scan order and the end of block.
pub fn quantize(coef: &[i32], tx_size: TxSize, qp: QuantParams) -> (Vec<i32>, usize) {
    unimplemented!("M3")
}

/// The dequantised value of `level` (what the decoder reconstructs).
pub fn dequantize(level: i32, pos: usize, qp: QuantParams) -> i32 {
    unimplemented!("M3")
}
