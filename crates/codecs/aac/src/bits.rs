// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MSB-first bit reader with strict bounds checking.
//!
//! Every read checks the remaining length, so a truncated or corrupt access
//! unit produces [`Error::Truncated`] instead of reading garbage or panicking.
//! Peeks past the end return zero bits (used by the Huffman decoder, which
//! then validates the consumed length with [`BitReader::skip`]).

use crate::error::{Error, Result};

#[derive(Clone)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, end: data.len() * 8 }
    }

    /// Current position in bits from the start of the buffer.
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Number of bits that may still be read.
    #[inline]
    pub fn bits_left(&self) -> usize {
        self.end - self.pos
    }

    /// The next 32 bits (MSB first), zero padded past the end.
    #[inline]
    pub fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let shift = (self.pos & 7) as u32;
        let v = if byte + 8 <= self.data.len() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&self.data[byte..byte + 8]);
            u64::from_be_bytes(b)
        } else {
            let mut v = 0u64;
            for i in 0..8 {
                v <<= 8;
                if let Some(&x) = self.data.get(byte + i) {
                    v |= x as u64;
                }
            }
            v
        };
        let mut r = ((v << shift) >> 32) as u32;
        // Hide bits that belong past the logical end (sub-readers).
        let left = self.bits_left();
        if left < 32 {
            r &= if left == 0 { 0 } else { !0u32 << (32 - left) };
        }
        r
    }

    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if n > self.bits_left() {
            self.pos = self.end;
            return Err(Error::Truncated);
        }
        self.pos += n;
        Ok(())
    }

    /// Read `n` bits (0..=32) as an unsigned integer.
    #[inline]
    pub fn read(&mut self, n: u32) -> Result<u32> {
        debug_assert!(n <= 32);
        if n == 0 {
            return Ok(0);
        }
        if n as usize > self.bits_left() {
            self.pos = self.end;
            return Err(Error::Truncated);
        }
        let v = self.peek32() >> (32 - n);
        self.pos += n as usize;
        Ok(v)
    }

    #[inline]
    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read(1)? != 0)
    }

    /// Read `n` bits as usize (convenience for counts and indices).
    #[inline]
    pub fn read_usize(&mut self, n: u32) -> Result<usize> {
        Ok(self.read(n)? as usize)
    }

    /// Skip to the next multiple of 8 bits measured from the buffer start.
    pub fn byte_align(&mut self) -> Result<()> {
        let pad = (8 - (self.pos & 7)) & 7;
        self.skip(pad)
    }

    /// A reader over the next `nbits` bits; `self` is advanced past them.
    pub fn take(&mut self, nbits: usize) -> Result<BitReader<'a>> {
        if nbits > self.bits_left() {
            self.pos = self.end;
            return Err(Error::Truncated);
        }
        let sub = BitReader { data: self.data, pos: self.pos, end: self.pos + nbits };
        self.pos += nbits;
        Ok(sub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_and_bounds() {
        let data = [0b1010_1100, 0b0101_0011, 0xff];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read(1).unwrap(), 1);
        assert_eq!(br.read(3).unwrap(), 0b010);
        assert_eq!(br.read(8).unwrap(), 0b1100_0101);
        assert_eq!(br.bits_left(), 12);
        let mut sub = br.take(6).unwrap();
        assert_eq!(sub.read(4).unwrap(), 0b0011);
        assert_eq!(sub.peek32(), 0b11 << 30);
        assert!(sub.read(3).is_err());
        assert_eq!(br.read(6).unwrap(), 0b111111);
        assert!(br.read(1).is_err());
    }

    #[test]
    fn align_and_long_reads() {
        let data = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11];
        let mut br = BitReader::new(&data);
        br.read(3).unwrap();
        br.byte_align().unwrap();
        assert_eq!(br.read(32).unwrap(), 0x3456789a);
        assert_eq!(br.read(32).unwrap(), 0xbcdef011);
        assert_eq!(br.bits_left(), 0);
    }
}
