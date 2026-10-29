// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M1: bit writer, the inverse of `studio_av1`'s bit reader (spec 4.10: f(n), leb128, uvlc, su, ns).

#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    pub fn new() -> BitWriter {
        BitWriter::default()
    }
    /// f(n): `n` bits of `v`, most significant first.
    pub fn put(&mut self, v: u32, n: u32) {
        unimplemented!("M1")
    }
    pub fn flag(&mut self, b: bool) {
        unimplemented!("M1")
    }
    pub fn leb128(&mut self, v: u64) {
        unimplemented!("M1")
    }
    pub fn uvlc(&mut self, v: u32) {
        unimplemented!("M1")
    }
    pub fn su(&mut self, v: i32, n: u32) {
        unimplemented!("M1")
    }
    pub fn ns(&mut self, v: u32, n: u32) {
        unimplemented!("M1")
    }
    pub fn byte_align(&mut self) {
        unimplemented!("M1")
    }
    /// trailing_bits(): a one bit, then zeros to the byte boundary.
    pub fn trailing_bits(&mut self) {
        unimplemented!("M1")
    }
    pub fn bit_len(&self) -> usize {
        self.bit_len
    }
    pub fn finish(self) -> Vec<u8> {
        unimplemented!("M1")
    }
}
