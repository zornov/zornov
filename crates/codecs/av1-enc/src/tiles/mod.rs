// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M10 (phase c): tile layout, parallel tile encoding, tile group OBUs.

/// Tile boundaries in units of superblocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileLayout {
    pub col_starts: Vec<u32>,
    pub row_starts: Vec<u32>,
}

impl TileLayout {
    /// A uniform layout (spec 5.9.15) for the given log2 tile counts.
    pub fn uniform(sb_cols: u32, sb_rows: u32, cols_log2: u8, rows_log2: u8) -> TileLayout {
        unimplemented!("M10")
    }
}
