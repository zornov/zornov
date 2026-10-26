// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reference CABAC *encoder* following the arithmetic encoding process of
//! the specification (9.3.4.x of H.264/H.265). Used by unit tests to validate
//! the decoding engine and by the `synth` feature to generate test bitstreams.

use super::engine::Ctx;
use super::tables::{RANGE_TAB_LPS, TRANS_IDX_LPS, TRANS_IDX_MPS};

/// Reference CABAC encoder following 9.3.4.x of the specification (used to
/// validate the decoder in unit tests).
#[derive(Clone)]
pub struct SpecEncoder {
    low: u32,
    range: u32,
    first_bit: bool,
    outstanding: u32,
    pub bits: Vec<u8>, // one entry per bit
}

impl SpecEncoder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        SpecEncoder { low: 0, range: 510, first_bit: true, outstanding: 0, bits: Vec::new() }
    }
    /// Starts a new arithmetic codeword (after a flush), keeping the written bits.
    pub fn restart(&mut self) {
        self.low = 0;
        self.range = 510;
        self.first_bit = true;
        self.outstanding = 0;
    }
    /// Appends zero bits up to the next byte boundary.
    pub fn align_zero(&mut self) {
        while self.bits.len() % 8 != 0 {
            self.bits.push(0);
        }
    }
    /// Appends raw bits (MSB first), e.g. PCM samples.
    pub fn put_raw(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bits.push(((v >> i) & 1) as u8);
        }
    }
    fn put_bit(&mut self, b: u8) {
        if self.first_bit {
            self.first_bit = false;
        } else {
            self.bits.push(b);
        }
        while self.outstanding > 0 {
            self.bits.push(1 - b);
            self.outstanding -= 1;
        }
    }
    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }
    pub fn encode(&mut self, ctx: &mut Ctx, bin: u32) {
        let p = (*ctx >> 1) as usize;
        let mps = (*ctx & 1) as u32;
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            let new_mps = if p == 0 { 1 - mps } else { mps };
            *ctx = (TRANS_IDX_LPS[p] << 1) | new_mps as u8;
        } else {
            *ctx = (TRANS_IDX_MPS[p] << 1) | mps as u8;
        }
        self.renorm();
    }
    pub fn encode_bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put_bit(1);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put_bit(0);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }
    pub fn encode_terminate(&mut self, bin: u32) {
        self.range -= 2;
        if bin != 0 {
            self.low += self.range;
            self.flush();
        } else {
            self.renorm();
        }
    }
    fn flush(&mut self) {
        self.range = 2;
        self.renorm();
        let b = ((self.low >> 9) & 1) as u8;
        self.put_bit(b);
        let two = ((self.low >> 7) & 3) | 1;
        self.bits.push(((two >> 1) & 1) as u8);
        self.bits.push((two & 1) as u8);
    }
    /// Byte-aligns with zero bits and returns the bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.bits.len().div_ceil(8)];
        for (i, &b) in self.bits.iter().enumerate() {
            out[i / 8] |= b << (7 - (i % 8));
        }
        out
    }
}

