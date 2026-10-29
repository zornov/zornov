// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M1: bit writer, the inverse of the decoder's bit reader (spec 4.10: f(n), leb128, uvlc, su, ns,
//! le, and the trailing/alignment bits of 5.3.4 and 5.3.5).

/// MSB-first bit writer. Bits are appended; the byte buffer is zero-padded on the last byte.
#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    pub fn new() -> BitWriter {
        BitWriter::default()
    }

    fn push_bit(&mut self, b: bool) {
        if self.bit_len & 7 == 0 {
            self.bytes.push(0);
        }
        if b {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> (self.bit_len & 7);
        }
        self.bit_len += 1;
    }

    /// f(n): the low `n` bits of `v`, most significant first. `n` is at most 32.
    pub fn put(&mut self, v: u32, n: u32) {
        assert!(n <= 32, "f(n) with n > 32");
        debug_assert!(n == 32 || v >> n == 0, "value {v} does not fit in {n} bits");
        for i in (0..n).rev() {
            self.push_bit((v >> i) & 1 != 0);
        }
    }

    pub fn flag(&mut self, b: bool) {
        self.push_bit(b);
    }

    /// leb128(): 7 bits per byte, least significant group first, at most 8 bytes.
    pub fn leb128(&mut self, mut v: u64) {
        assert!(v < 1 << 56, "leb128 value needs more than 8 bytes");
        loop {
            let b = (v & 0x7f) as u32;
            v >>= 7;
            if v == 0 {
                self.put(b, 8);
                return;
            }
            self.put(b | 0x80, 8);
        }
    }

    /// uvlc(): `leadingZeros` zeros, a one, then `leadingZeros` bits of `v + 1`.
    pub fn uvlc(&mut self, v: u32) {
        let x = v as u64 + 1;
        let lz = 63 - x.leading_zeros();
        if lz >= 32 {
            // Only u32::MAX gets here: the reader saturates at 32 leading zeros and reads no value.
            for _ in 0..32 {
                self.push_bit(false);
            }
            self.push_bit(true);
            return;
        }
        for _ in 0..lz {
            self.push_bit(false);
        }
        self.push_bit(true);
        self.put((x & ((1u64 << lz) - 1)) as u32, lz);
    }

    /// su(n): `v` as an `n`-bit two's complement number.
    pub fn su(&mut self, v: i32, n: u32) {
        assert!((1..=32).contains(&n), "su(n) needs 1 <= n <= 32");
        let lim = 1i64 << (n - 1);
        debug_assert!(
            (-lim..lim).contains(&(v as i64)),
            "value {v} does not fit in su({n})"
        );
        self.put((v as u32) & (u32::MAX >> (32 - n)), n);
    }

    /// ns(n): non-symmetric unsigned code for `v` in `0..n`.
    pub fn ns(&mut self, v: u32, n: u32) {
        assert!(v < n, "ns(n) value out of range");
        let w = 32 - n.leading_zeros(); // FloorLog2(n) + 1
        let m = ((1u64 << w) - n as u64) as u32;
        if v < m {
            self.put(v, w - 1);
        } else {
            let t = v + m;
            self.put(t >> 1, w - 1);
            self.put(t & 1, 1);
        }
    }

    /// le(n): `n` bytes of `v`, least significant first (tile sizes).
    pub fn le(&mut self, v: u32, n: u32) {
        assert!(
            n <= 4 && (n == 4 || v >> (8 * n) == 0),
            "le(n) value does not fit"
        );
        for i in 0..n {
            self.put((v >> (8 * i)) & 0xff, 8);
        }
    }

    /// Whole bytes at the current position (the position must be byte aligned).
    pub fn put_bytes(&mut self, data: &[u8]) {
        assert!(self.is_byte_aligned(), "put_bytes on an unaligned writer");
        self.bytes.extend_from_slice(data);
        self.bit_len += 8 * data.len();
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.bit_len & 7 == 0
    }

    /// byte_alignment(): zero bits up to the next byte boundary.
    pub fn byte_align(&mut self) {
        self.bit_len = self.bytes.len() * 8;
    }

    /// trailing_bits(): a one bit, then zeros to the byte boundary.
    pub fn trailing_bits(&mut self) {
        self.push_bit(true);
        self.byte_align();
    }

    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes written; a partial last byte is zero padded.
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
