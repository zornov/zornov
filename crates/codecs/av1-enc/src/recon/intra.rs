// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M5: intra prediction + residual reconstruction through `studio_av1::intra::predict_intra`.

use super::FrameBuf;
use crate::types::{IntraMode, TxSize};

/// Predicts one transform block into `recon` and adds `residual` (clipped to the bit depth).
pub fn predict_and_reconstruct(recon: &mut FrameBuf, plane: usize, x: usize, y: usize, tx_size: TxSize, mode: IntraMode, residual: &[i32]) {
    unimplemented!("M5")
}
