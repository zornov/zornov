// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MSB-first bit writer with Exp-Golomb codes, and NAL unit packaging.

/// Writes bits most significant first.
#[derive(Default, Clone)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    /// Bits pending in `acc` (always < 8 between calls).
    n: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(bytes: usize) -> Self {
        BitWriter { buf: Vec::with_capacity(bytes), acc: 0, n: 0 }
    }

    /// Append the low `bits` bits of `value` (bits <= 32).
    #[inline]
    pub fn put(&mut self, bits: u32, value: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        let v = if bits == 32 { value as u64 } else { (value as u64) & ((1u64 << bits) - 1) };
        self.acc = (self.acc << bits) | v;
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    #[inline]
    pub fn put_bit(&mut self, b: bool) {
        self.put(1, b as u32);
    }

    /// Unsigned Exp-Golomb ue(v).
    #[inline]
    pub fn ue(&mut self, v: u32) {
        let code = v as u64 + 1;
        let len = 64 - code.leading_zeros();
        if len <= 16 {
            self.put(2 * len - 1, code as u32);
        } else {
            self.put(len - 1, 0);
            if len > 32 {
                self.put(len - 32, (code >> 32) as u32);
                self.put(32, code as u32);
            } else {
                self.put(len, code as u32);
            }
        }
    }

    /// Signed Exp-Golomb se(v).
    #[inline]
    pub fn se(&mut self, v: i32) {
        let k = if v > 0 { 2 * v as i64 - 1 } else { -2 * v as i64 };
        self.ue(k as u32);
    }

    /// Bits written so far.
    pub fn bit_len(&self) -> u64 {
        self.buf.len() as u64 * 8 + self.n as u64
    }

    pub fn byte_aligned(&self) -> bool {
        self.n == 0
    }

    /// Pad with zero bits to a byte boundary.
    pub fn align_zero(&mut self) {
        if self.n > 0 {
            self.put(8 - self.n, 0);
        }
    }

    /// Pad with one bits to a byte boundary.
    pub fn align_one(&mut self) {
        if self.n > 0 {
            let k = 8 - self.n;
            self.put(k, (1 << k) - 1);
        }
    }

    /// rbsp_trailing_bits(): a one bit then zeros to the byte boundary.
    pub fn trailing(&mut self) {
        self.put_bit(true);
        self.align_zero();
    }

    /// Append whole bytes (the writer must be byte aligned).
    pub fn put_bytes(&mut self, b: &[u8]) {
        if self.n == 0 {
            self.buf.extend_from_slice(b);
        } else {
            for &x in b {
                self.put(8, x as u32);
            }
        }
    }

    /// Finished bytes; a partial last byte is padded with zeros.
    pub fn finish(mut self) -> Vec<u8> {
        self.align_zero();
        self.buf
    }

    /// Bytes completed so far (excluding a partial byte).
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }
}

/// Number of bits of ue(v).
#[inline]
pub fn ue_len(v: u32) -> u32 {
    let code = v as u64 + 1;
    2 * (64 - code.leading_zeros()) - 1
}

/// Number of bits of se(v).
#[inline]
pub fn se_len(v: i32) -> u32 {
    let k = if v > 0 { 2 * v as i64 - 1 } else { -2 * v as i64 };
    ue_len(k as u32)
}

/// A NAL unit (header byte + RBSP with emulation prevention), without a start
/// code or length prefix.
pub fn nal_unit(nal_ref_idc: u8, nal_unit_type: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 2);
    out.push((nal_ref_idc & 3) << 5 | (nal_unit_type & 31));
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

/// Append `nal` to `out` with a 4-byte big-endian length prefix (AVCC framing).
pub fn push_length_prefixed(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
    out.extend_from_slice(nal);
}

/// Convert 4-byte length-prefixed NAL units to an Annex-B byte stream.
pub fn to_annex_b(sample: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sample.len() + 16);
    let mut p = 0;
    while p + 4 <= sample.len() {
        let n = u32::from_be_bytes([sample[p], sample[p + 1], sample[p + 2], sample[p + 3]]) as usize;
        p += 4;
        let end = (p + n).min(sample.len());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[p..end]);
        p = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_codes() {
        let mut w = BitWriter::new();
        for v in [0u32, 1, 2, 3, 7, 8] {
            w.ue(v);
        }
        let bits: String = ["1", "010", "011", "00100", "0001000", "0001001"].concat();
        let bytes = w.finish();
        let s: String = bytes.iter().map(|b| format!("{b:08b}")).collect();
        assert!(s.starts_with(&bits), "{s}");
        assert_eq!(ue_len(0), 1);
        assert_eq!(ue_len(1), 3);
        assert_eq!(se_len(-1), 3);
        assert_eq!(se_len(0), 1);
    }

    #[test]
    fn large_ue() {
        let mut w = BitWriter::new();
        w.ue(1 << 20);
        assert_eq!(w.bit_len(), ue_len(1 << 20) as u64);
    }

    #[test]
    fn emulation_prevention() {
        let n = nal_unit(3, 7, &[0, 0, 1, 0, 0, 0, 5, 0, 0]);
        assert_eq!(n, vec![0x67, 0, 0, 3, 1, 0, 0, 3, 0, 5, 0, 0]);
    }
}
