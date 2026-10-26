// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Big-endian bit packing (ST 2019-1 section 7.3.1, Figure 28).

/// Something codewords can be written to: a real writer or a bit counter.
pub trait Put {
    fn put(&mut self, code: u32, len: u32);
}

/// Counts bits instead of storing them (rate control).
#[derive(Default)]
pub struct Counter(pub u32);

impl Put for Counter {
    #[inline]
    fn put(&mut self, _code: u32, len: u32) {
        self.0 += len;
    }
}

/// Packs values most significant bit first.
#[derive(Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Put for BitWriter {
    #[inline]
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (code as u64 & ((1u64 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push((self.acc >> self.n) as u8);
        }
    }
}

impl BitWriter {
    pub fn new() -> BitWriter {
        BitWriter::default()
    }

    /// Bits written so far.
    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.n as usize
    }

    /// Pads with zero bits to the next multiple of 32 and returns the bytes.
    pub fn finish_aligned(mut self) -> Vec<u8> {
        let rem = self.bit_len() % 32;
        if rem != 0 {
            let pad = 32 - rem as u32;
            let mut left = pad;
            while left > 0 {
                let k = left.min(16);
                self.put(0, k);
                left -= k;
            }
        }
        self.buf
    }
}

/// Reads values most significant bit first. Reading past the end yields zeros;
/// `overrun` reports it.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader { data, pos: 0 }
    }

    pub fn overrun(&self) -> bool {
        self.pos > self.data.len() * 8
    }

    /// The next 16 bits without consuming them.
    #[inline]
    pub fn peek16(&self) -> u32 {
        let byte = self.pos / 8;
        let mut v: u32 = 0;
        for i in 0..4 {
            v = (v << 8) | self.data.get(byte + i).copied().unwrap_or(0) as u32;
        }
        (v << (self.pos % 8)) >> 16
    }

    #[inline]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    /// Reads up to 16 bits.
    #[inline]
    pub fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek16() >> (16 - n);
        self.pos += n as usize;
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0xABC, 12);
        w.put(1, 1);
        let bytes = w.finish_aligned();
        assert_eq!(bytes.len(), 4);
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.get(3), 0b101);
        assert_eq!(r.get(12), 0xABC);
        assert_eq!(r.get(1), 1);
        assert_eq!(r.get(16), 0);
    }
}
