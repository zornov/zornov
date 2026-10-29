// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M2: the range encoder, the exact inverse of `studio_av1::symbol::SymbolDecoder` (spec 8.2).
//!
//! The decoder works on the bitwise inverse of the stream: a symbol `s` with cumulative bounds
//! `cur(s) <= value < cur(s - 1)` (where `cur(-1)` is the range) subtracts `cur(s)` from the value
//! and takes `cur(s - 1) - cur(s)` as the new range. The encoder therefore tracks the interval
//! `[low, low + range)` of the *inverted* stream: it adds `cur(s)` to `low`, shrinks `range` the
//! same way, and renormalises by shifting `low` and `range` left. Bits that leave the 16-bit
//! window are settled except for a possible carry, which ripples back through the bytes already
//! produced.
//!
//! `finish` picks the value in the final interval whose bit pattern is `0` followed by ones, which
//! is the spec's trailing bit (`1`) and zero padding once inverted back; the decoder reads zero
//! bytes past the end of the tile, so nothing beyond the last byte is stored.

pub mod cdf_ctx;

use studio_av1::symbol::update_cdf;

const EC_PROB_SHIFT: u32 = 6;
const EC_MIN_PROB: u32 = 4;

#[derive(Debug)]
pub struct SymbolEncoder {
    /// Whole bytes of the (inverted) stream; the last ones can still receive a carry.
    out: Vec<u8>,
    /// Interval start: window in bits 0..16, then `cnt` settled bits, then a carry bit.
    low: u64,
    /// Interval width, always in `2^15..2^16` after a renormalisation.
    range: u32,
    /// Settled bits above the window that are not in `out` yet (0..8 between calls).
    cnt: u32,
    /// Whether any renormalisation happened (bit 15 of the window is real stream data then).
    shifted: bool,
    disable_update: bool,
}

impl Default for SymbolEncoder {
    fn default() -> Self {
        SymbolEncoder { out: Vec::new(), low: 0, range: 1 << 15, cnt: 0, shifted: false, disable_update: false }
    }
}

impl SymbolEncoder {
    pub fn new() -> SymbolEncoder {
        SymbolEncoder::default()
    }

    /// Like `new`, with CDF adaptation switched off (`disable_cdf_update`).
    pub fn with_disable_update(disable_update: bool) -> SymbolEncoder {
        SymbolEncoder { disable_update, ..SymbolEncoder::default() }
    }

    /// Writes `symbol` with `cdf` (N cumulative values plus the adaptation counter, the decoder's
    /// layout) and adapts `cdf` exactly as the decoder does (`studio_av1::symbol::update_cdf`).
    pub fn write_symbol(&mut self, cdf: &mut [u16], symbol: usize) {
        let n = cdf.len() - 1;
        assert!(symbol < n, "symbol {symbol} out of range for a {n}-ary cdf");
        self.encode(cdf, n, symbol);
        if !self.disable_update {
            update_cdf(cdf, n, symbol);
        }
    }

    /// The decoder's `cur` bound for symbol `s`.
    #[inline(always)]
    fn bound(&self, cdf: &[u16], n: usize, s: usize) -> u32 {
        let f = (1u32 << 15) - cdf[s] as u32;
        (((self.range >> 8) * (f >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB * (n - s - 1) as u32
    }

    #[inline(always)]
    fn encode(&mut self, cdf: &[u16], n: usize, symbol: usize) {
        let prev = if symbol == 0 { self.range } else { self.bound(cdf, n, symbol - 1) };
        let cur = self.bound(cdf, n, symbol);
        self.low += cur as u64;
        self.range = prev - cur;
        self.carry();
        let bits = 15 - (31 - self.range.leading_zeros());
        if bits > 0 {
            self.shifted = true;
            self.range <<= bits;
            self.low <<= bits;
            self.cnt += bits;
            while self.cnt >= 8 {
                let shift = 16 + self.cnt - 8;
                self.out.push((self.low >> shift) as u8);
                self.low &= (1u64 << shift) - 1;
                self.cnt -= 8;
            }
        }
    }

    /// Moves a carry out of the settled bits into `out`.
    #[inline(always)]
    fn carry(&mut self) {
        let top = 16 + self.cnt;
        if self.low >> top != 0 {
            self.low &= (1u64 << top) - 1;
            self.propagate();
        }
    }

    #[cold]
    fn propagate(&mut self) {
        for b in self.out.iter_mut().rev() {
            if *b == 0xFF {
                *b = 0;
            } else {
                *b += 1;
                return;
            }
        }
        unreachable!("carry out of the first stream byte");
    }

    pub fn write_bool(&mut self, b: bool) {
        let cdf = [1u16 << 14, 1 << 15, 0];
        self.encode(&cdf, 2, b as usize);
    }

    /// L(n): `n` bits, most significant first.
    pub fn write_literal(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bool((v >> i) & 1 != 0);
        }
    }

    /// NS(n) for `v < n`, the inverse of the decoder's `read_ns`.
    pub fn write_ns(&mut self, v: u32, n: u32) {
        if n <= 1 {
            return;
        }
        assert!(v < n);
        let w = 32 - n.leading_zeros();
        let m = (1u32 << w) - n;
        if v < m {
            self.write_literal(v, w - 1);
        } else {
            let t = v + m;
            self.write_literal(t >> 1, w - 1);
            self.write_literal(t & 1, 1);
        }
    }

    /// Flushes the coder and returns the tile's bytes.
    pub fn finish(mut self) -> Vec<u8> {
        // Largest t such that some V in [low, low + range) reads 0 at bit t and 1 below it.
        let max_t = if self.shifted { 15 } else { 14 };
        let mut t = max_t;
        let v = loop {
            let step = 1u64 << (t + 1);
            let target = (1u64 << t) - 1;
            let part = self.low & (step - 1);
            let v = self.low - part + target + if part > target { step } else { 0 };
            if v < self.low + self.range as u64 {
                break v;
            }
            t -= 1;
        };
        self.low = v;
        self.carry();
        // The settled bits start with a virtual zero bit (the decoder's first window is 15 bits
        // wide, ours 16); it is dropped below. The tail runs from there to bit `t`, then ones
        // fill the byte (plus seven spare ones, so the one-bit shift leaves whole bytes).
        let mut nbits = self.cnt + 16 - t;
        let mut bits = self.low >> t;
        let pad = (9 - nbits % 8) % 8 + 7;
        debug_assert!((nbits + pad).is_multiple_of(8) && nbits + pad <= 64);
        bits = (bits << pad) | ((1u64 << pad) - 1);
        nbits += pad;
        assert!(nbits.is_multiple_of(8) && nbits <= 64, "finish: unaligned tail");
        while nbits > 0 {
            nbits -= 8;
            self.out.push((bits >> nbits) as u8);
            bits &= (1u64 << nbits) - 1;
        }
        // Drop the virtual bit: shift the whole stream left by one.
        let len = self.out.len();
        for i in 0..len {
            let next = if i + 1 < len { self.out[i + 1] >> 7 } else { 1 };
            self.out[i] = (self.out[i] << 1) | next;
        }
        // Whole bytes of padding ones carry nothing (the decoder reads them as zeros).
        while self.out.last() == Some(&0xFF) {
            self.out.pop();
        }
        // The decoder works on the inverted stream.
        for b in self.out.iter_mut() {
            *b = !*b;
        }
        self.out
    }
}
