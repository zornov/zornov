// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MSB-first bit reader over an RBSP (emulation prevention bytes already removed),
//! with Exp-Golomb helpers (ITU-T H.264 clause 9.1).
//!
//! Reads past the end of the buffer return zero bits; callers detect truncation
//! with [`BitReader::overrun`] at convenient synchronisation points, which keeps
//! the hot paths free of per-read error handling.

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Current position in bits.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    #[inline]
    pub fn bits_total(&self) -> usize {
        self.data.len() * 8
    }

    #[inline]
    pub fn bits_left(&self) -> isize {
        self.bits_total() as isize - self.pos as isize
    }

    /// True once more bits have been consumed than the buffer holds.
    #[inline]
    pub fn overrun(&self) -> bool {
        self.pos > self.bits_total()
    }

    #[inline]
    pub fn check_overrun(&self) -> Result<()> {
        if self.overrun() { Err(Error::Bitstream("unexpected end of data")) } else { Ok(()) }
    }

    /// 64 bits starting at the byte containing `pos`, zero padded past the end.
    #[inline(always)]
    fn load64(&self, byte: usize) -> u64 {
        if byte + 8 <= self.data.len() {
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
        }
    }

    /// Peek up to 32 bits without consuming them.
    #[inline(always)]
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let byte = self.pos >> 3;
        let v = self.load64(byte) << (self.pos & 7);
        (v >> (64 - n)) as u32
    }

    #[inline(always)]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    /// Read up to 32 bits.
    #[inline(always)]
    pub fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    #[inline(always)]
    pub fn read_bit(&mut self) -> bool {
        let byte = self.pos >> 3;
        let b = match self.data.get(byte) {
            Some(&x) => (x >> (7 - (self.pos & 7))) & 1,
            None => 0,
        };
        self.pos += 1;
        b != 0
    }

    #[inline(always)]
    pub fn flag(&mut self) -> bool {
        self.read_bit()
    }

    /// ue(v). Returns `u32::MAX` for codes longer than 32 bits (callers range-check).
    #[inline]
    pub fn ue(&mut self) -> u32 {
        let p = self.peek(32);
        if p == 0 {
            // 32 or more leading zeros: not representable in 32 bits.
            if self.bits_left() <= 32 {
                self.pos = self.bits_total() + 1;
                return u32::MAX;
            }
            let mut lz = 32u32;
            self.pos += 32;
            while !self.read_bit() {
                lz += 1;
                if lz > 64 || self.overrun() {
                    self.pos = self.bits_total() + 1;
                    return u32::MAX;
                }
            }
            let _ = lz;
            return u32::MAX;
        }
        let lz = p.leading_zeros();
        if lz < 16 {
            // Whole code fits in the 32-bit peek.
            let len = 2 * lz + 1;
            self.pos += len as usize;
            return (p >> (32 - len)) - 1;
        }
        self.pos += lz as usize;
        let v = self.read(lz + 1) as u64;
        (v - 1).min(u32::MAX as u64) as u32
    }

    /// se(v).
    #[inline]
    pub fn se(&mut self) -> i32 {
        let k = self.ue();
        if k == u32::MAX {
            return i32::MIN;
        }
        let k = k as i64;
        let v = if k & 1 == 1 { (k + 1) / 2 } else { -(k / 2) };
        v as i32
    }

    /// te(v) with range `max` (9.1).
    #[inline]
    pub fn te(&mut self, max: u32) -> u32 {
        if max == 1 { (!self.read_bit()) as u32 } else { self.ue() }
    }

    /// ue(v) checked against an inclusive maximum.
    #[inline]
    pub fn ue_max(&mut self, max: u32, what: &'static str) -> Result<u32> {
        let v = self.ue();
        if v > max { Err(Error::Bitstream(what)) } else { Ok(v) }
    }

    /// se(v) checked against an inclusive range.
    #[inline]
    pub fn se_range(&mut self, min: i32, max: i32, what: &'static str) -> Result<i32> {
        let v = self.se();
        if v < min || v > max { Err(Error::Bitstream(what)) } else { Ok(v) }
    }

    #[inline]
    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// more_rbsp_data() (7.2): true if there is more data before the rbsp_stop_one_bit.
    pub fn more_rbsp_data(&self) -> bool {
        // Find the last set bit in the buffer: that is the rbsp_stop_one_bit.
        let mut last = self.data.len();
        while last > 0 && self.data[last - 1] == 0 {
            last -= 1;
        }
        if last == 0 {
            return false;
        }
        let b = self.data[last - 1];
        let stop_bit_pos = (last - 1) * 8 + (7 - b.trailing_zeros() as usize);
        self.pos < stop_bit_pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        // 1 | 010 | 011 | 00100 | 00101 -> ue: 0, 1, 2, 3, 4
        let data = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.ue(), 0);
        assert_eq!(r.ue(), 1);
        assert_eq!(r.ue(), 2);
        assert_eq!(r.ue(), 3);
        assert_eq!(r.ue(), 4);
        assert!(!r.overrun());
        let mut r = BitReader::new(&data);
        assert_eq!(r.se(), 0);
        assert_eq!(r.se(), 1);
        assert_eq!(r.se(), -1);
        assert_eq!(r.se(), 2);
        assert_eq!(r.se(), -2);
    }

    #[test]
    fn long_codes_and_overrun() {
        // 20 leading zeros then 1 then 20 bits of value 5 -> 2^20 - 1 + 5
        let mut bits = String::new();
        bits.push_str(&"0".repeat(20));
        bits.push('1');
        bits.push_str(&format!("{:020b}", 5));
        while bits.len() % 8 != 0 {
            bits.push('1');
        }
        let bytes: Vec<u8> =
            bits.as_bytes().chunks(8).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap()).collect();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.ue(), (1 << 20) - 1 + 5);
        let zeros = [0u8; 3];
        let mut r = BitReader::new(&zeros);
        assert_eq!(r.ue(), u32::MAX);
        assert!(r.overrun());
    }

    #[test]
    fn more_rbsp() {
        let data = [0b1100_0000];
        let mut r = BitReader::new(&data);
        assert!(r.more_rbsp_data());
        r.read(1);
        assert!(!r.more_rbsp_data());
    }
}
