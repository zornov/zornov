// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CABAC arithmetic decoding engine (9.3.4.3).
//!
//! The implementation keeps the 9-bit `ivlOffset` of the specification scaled by
//! 2^7 together with up to 8 look-ahead bits in `value` (the classic HM layout),
//! which allows byte-wise refills while remaining bit-exact. The exact
//! bitstream position at a terminating bin is recoverable (needed for
//! end_of_subset_one_bit / pcm_flag handling).

use super::tables::{NUM_CTX, RANGE_TAB_LPS, TRANS_IDX_LPS, TRANS_IDX_MPS};
use super::tables::INIT_VALUES;

/// Context model state: (pStateIdx << 1) | valMps.
pub type Ctx = u8;

/// Initialises all context variables for a slice (9.3.2.2).
pub fn init_contexts(slice_qp: i32, init_type: usize) -> [Ctx; NUM_CTX] {
    let mut out = [0u8; NUM_CTX];
    let qp = slice_qp.clamp(0, 51);
    for (i, &v) in INIT_VALUES[init_type].iter().enumerate() {
        out[i] = init_ctx(v, qp);
    }
    out
}

#[inline]
pub fn init_ctx(init_value: u8, qp: i32) -> Ctx {
    let slope = (init_value >> 4) as i32;
    let offset = (init_value & 15) as i32;
    let m = slope * 5 - 45;
    let n = (offset << 3) - 16;
    let pre = (((m * qp) >> 4) + n).clamp(1, 126);
    if pre <= 63 {
        (((63 - pre) as u8) << 1) | 0
    } else {
        (((pre - 64) as u8) << 1) | 1
    }
}

/// Arithmetic decoder. `value` holds the specification's 9-bit `ivlOffset`
/// followed by `k` look-ahead bits (`value < range << k`); the bitstream is
/// loaded 32 bits at a time and `k` is kept >= 16 between operations, so a
/// renormalisation is a decrement of `k`. The exact bitstream position at a
/// terminating bin stays recoverable (end_of_subset_one_bit / pcm_flag).
pub struct CabacEngine<'a> {
    data: &'a [u8],
    /// Next byte to load.
    pos: usize,
    /// Byte position the engine was (re)initialised at.
    start: usize,
    value: u64,
    range: u32,
    k: u32,
}

/// Look-ahead bits kept available between operations.
const MIN_K: u32 = 16;

impl<'a> CabacEngine<'a> {
    /// Initialises the arithmetic decoder at byte `start` of `data` (9.3.2.5).
    pub fn new(data: &'a [u8], start: usize) -> Self {
        let mut e = CabacEngine { data, pos: start, start, value: 0, range: 510, k: 0 };
        e.reinit(start);
        e
    }

    /// Re-initialises the arithmetic decoder at byte `start`.
    pub fn reinit(&mut self, start: usize) {
        self.pos = start;
        self.start = start;
        self.range = 510;
        self.value = 0;
        // 9 bits of ivlOffset, the remaining loaded bits are look-ahead.
        self.k = 0;
        self.refill();
        self.refill();
        self.k -= 9;
    }

    /// Appends 32 bits (zeros past the end of the data) to `value`.
    #[inline(always)]
    fn refill(&mut self) {
        let p = self.pos;
        let w = if let Some(b) = self.data.get(p..p.wrapping_add(4)) {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            let mut w = 0u32;
            for i in 0..4 {
                w = (w << 8) | self.data.get(p.wrapping_add(i)).copied().unwrap_or(0) as u32;
            }
            w
        };
        self.pos = p.wrapping_add(4);
        self.value = (self.value << 32) | w as u64;
        self.k += 32;
    }

    /// Number of bits consumed by the specification's 9-bit decoder model
    /// (absolute bit position in `data`).
    pub fn consumed_bits(&self) -> usize {
        self.pos * 8 - self.k as usize
    }

    /// Number of bytes read past the end of the data (corruption indicator),
    /// counted as a byte-wise reader that reads each byte when its first bit
    /// is needed would count them.
    pub fn overrun(&self) -> usize {
        let byte_pos = self.consumed_bits().div_ceil(8);
        byte_pos.saturating_sub(self.data.len().max(self.start))
    }

    /// Byte position following the alignment after a terminating bin equal to 1.
    pub fn aligned_byte_after_terminate(&self) -> usize {
        self.consumed_bits().div_ceil(8)
    }

    #[inline(always)]
    pub fn decode_bin(&mut self, ctx: &mut Ctx) -> u32 {
        let s = *ctx;
        let p = (s >> 1) as usize;
        let mps = (s & 1) as u32;
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        let scaled = (self.range as u64) << self.k;
        if self.value < scaled {
            *ctx = (TRANS_IDX_MPS[p] << 1) | mps as u8;
            if self.range < 256 {
                self.range <<= 1;
                self.k -= 1;
                if self.k < MIN_K {
                    self.refill();
                }
            }
            mps
        } else {
            let n = lps.leading_zeros() - 23;
            self.value -= scaled;
            self.range = lps << n;
            self.k -= n;
            let new_mps = if p == 0 { 1 - mps } else { mps };
            *ctx = (TRANS_IDX_LPS[p] << 1) | new_mps as u8;
            if self.k < MIN_K {
                self.refill();
            }
            1 - mps
        }
    }

    #[inline(always)]
    pub fn decode_bypass(&mut self) -> u32 {
        self.k -= 1;
        let scaled = (self.range as u64) << self.k;
        let bin = if self.value >= scaled {
            self.value -= scaled;
            1
        } else {
            0
        };
        if self.k < MIN_K {
            self.refill();
        }
        bin
    }

    /// Decodes `n` (<= 32) bypass bins, MSB first. Successive bypass bins are
    /// the binary long division of the offset (extended by `n` bits) by the
    /// range, done here in chunks of up to 16 bins.
    #[inline]
    pub fn decode_bypass_bits(&mut self, mut n: u32) -> u32 {
        let mut bins = 0u32;
        while n > 0 {
            let m = n.min(MIN_K);
            let sh = self.k - m;
            let t = self.value >> sh;
            let r = self.range as u64;
            let q = t / r;
            let rem = t - q * r;
            self.value = (rem << sh) | (self.value & ((1u64 << sh) - 1));
            self.k = sh;
            bins = (bins << m) | q as u32;
            if self.k < MIN_K {
                self.refill();
            }
            n -= m;
        }
        bins
    }

    pub fn decode_terminate(&mut self) -> u32 {
        self.range -= 2;
        let scaled = (self.range as u64) << self.k;
        if self.value >= scaled {
            1
        } else {
            if self.range < 256 {
                self.range <<= 1;
                self.k -= 1;
                if self.k < MIN_K {
                    self.refill();
                }
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cabac::specenc::SpecEncoder;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    #[test]
    fn init_ctx_values() {
        // initValue 154 gives pStateIdx 1? (m = 0, n = 64 => preCtxState 64 => mps 1, state 0)
        assert_eq!(init_ctx(154, 26), 1);
        // 139 at qp 26: slope 8 -> m=-5, offset 11 -> n=72; (-5*26)>>4 = -9 ; 63 -> mps0, state 0
        assert_eq!(init_ctx(139, 26), 0);
    }

    #[test]
    fn roundtrip_random_bins() {
        for seed in 0..20u64 {
            let mut rng = Lcg(seed * 7919 + 1);
            let nctx = 8;
            let mut enc_ctx: Vec<Ctx> = (0..nctx).map(|i| init_ctx(100 + i as u8 * 13, 30)).collect();
            let mut dec_ctx = enc_ctx.clone();
            let mut ops = Vec::new();
            let mut enc = SpecEncoder::new();
            let n = 2000 + (rng.next() % 3000) as usize;
            for _ in 0..n {
                let kind = rng.next() % 10;
                if kind < 7 {
                    let c = (rng.next() % nctx as u32) as usize;
                    // skewed bins
                    let bin = if rng.next() % 100 < 15 + c as u32 * 8 { 1 } else { 0 };
                    enc.encode(&mut enc_ctx[c], bin);
                    ops.push((0u8, c, bin));
                } else if kind < 8 {
                    let bin = rng.next() & 1;
                    enc.encode_bypass(bin);
                    ops.push((1, 0, bin));
                } else if kind < 9 {
                    // group of bypass bins decoded with decode_bypass_bits
                    let nbits = 1 + (rng.next() % 32) as usize;
                    let mut v: u32 = 0;
                    for _ in 0..nbits {
                        let b = rng.next() & 1;
                        enc.encode_bypass(b);
                        v = (v << 1) | b;
                    }
                    ops.push((3, nbits, v));
                } else {
                    enc.encode_terminate(0);
                    ops.push((2, 0, 0));
                }
            }
            enc.encode_terminate(1);
            let total_bits = enc.bits.len();
            let mut bytes = enc.bytes();
            bytes.extend_from_slice(&[0xAB, 0xCD]); // trailing data must not matter
            let mut dec = CabacEngine::new(&bytes, 0);
            for (i, &(k, c, bin)) in ops.iter().enumerate() {
                let got = match k {
                    0 => dec.decode_bin(&mut dec_ctx[c]),
                    1 => dec.decode_bypass(),
                    3 => dec.decode_bypass_bits(c as u32),
                    _ => dec.decode_terminate(),
                };
                assert_eq!(got, bin, "seed {seed} op {i}");
            }
            assert_eq!(dec.decode_terminate(), 1);
            let _ = &mut rng;
            assert_eq!(dec.consumed_bits(), total_bits, "seed {seed}");
            assert_eq!(dec.aligned_byte_after_terminate(), total_bits.div_ceil(8));
            assert_eq!(dec_ctx, enc_ctx);
        }
    }
}
