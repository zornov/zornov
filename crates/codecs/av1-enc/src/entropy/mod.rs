// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M2: the range encoder, the exact inverse of `studio_av1::symbol::SymbolDecoder` (spec 8.2).

pub mod cdf_ctx;

#[derive(Debug, Default)]
pub struct SymbolEncoder {
    out: Vec<u8>,
}

impl SymbolEncoder {
    pub fn new() -> SymbolEncoder {
        SymbolEncoder::default()
    }
    /// Writes `symbol` with `cdf` (N cumulative values plus the adaptation counter, the decoder's
    /// layout) and adapts `cdf` exactly as the decoder does (`studio_av1::symbol::update_cdf`).
    pub fn write_symbol(&mut self, cdf: &mut [u16], symbol: usize) {
        unimplemented!("M2")
    }
    pub fn write_bool(&mut self, b: bool) {
        unimplemented!("M2")
    }
    pub fn write_literal(&mut self, v: u32, n: u32) {
        unimplemented!("M2")
    }
    pub fn write_ns(&mut self, v: u32, n: u32) {
        unimplemented!("M2")
    }
    /// Flushes the coder and returns the tile's bytes.
    pub fn finish(self) -> Vec<u8> {
        unimplemented!("M2")
    }
}
