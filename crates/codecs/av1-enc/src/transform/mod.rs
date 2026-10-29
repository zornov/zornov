// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M3: forward transforms (spec 7.13 inverted), producing the 64x64-stride coefficient layout
//! `studio_av1::transform::inverse_transform_2d` reads.

pub mod quant;

use crate::types::{TxSize, TxType};

/// `residual` is `w * h` samples, row stride `w`; `out` is a 64x64 buffer (row stride 64) of
/// coefficients (only the top-left 32x32 may be non-zero).
pub fn forward_transform(residual: &[i16], tx_size: TxSize, tx_type: TxType, out: &mut [i32]) {
    unimplemented!("M3")
}
