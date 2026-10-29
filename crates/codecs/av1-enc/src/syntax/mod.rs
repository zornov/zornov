// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M4: block-level syntax writers (spec 5.11), the inverse of the decoder's tile parsing. Keeps its
//! own above/left context state and mirrors the decoder's context derivation.

pub mod coef;
pub mod ctx;
pub mod inter_info;
pub mod intra_info;

use crate::entropy::cdf_ctx::CdfContext;
use crate::types::{BlockDecision, SeqParams};

/// Writes one tile's symbols.
pub struct TileWriter {
    cdfs: CdfContext,
}

impl TileWriter {
    pub fn new(seq: &SeqParams, base_q_idx: u8, mi_cols: u32, mi_rows: u32) -> TileWriter {
        unimplemented!("M4")
    }
    /// The partition symbol at a block of `bsize` (spec 5.11.4).
    pub fn write_partition(&mut self, mi_row: u32, mi_col: u32, bsize: usize, partition: usize) {
        unimplemented!("M4")
    }
    pub fn write_block(&mut self, dec: &BlockDecision) {
        unimplemented!("M4")
    }
    /// The finished tile data.
    pub fn finish(self) -> Vec<u8> {
        unimplemented!("M4")
    }
}
