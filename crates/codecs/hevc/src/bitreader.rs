// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RBSP extraction (emulation prevention removal) and an MSB-first bit reader
//! with Exp-Golomb support.

use crate::error::{Error, Result};

/// Removes emulation prevention bytes (`00 00 03` -> `00 00`) from a NAL unit payload.
pub fn nal_to_rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0usize;
    let mut i = 0;
    while i < nal.len() {
        let b = nal[i];
        if zeros >= 2 && b == 3 {
            // Emulation prevention byte: drop it.
            zeros = 0;
            i += 1;
            continue;
        }
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        out.push(b);
        i += 1;
    }
    out
}

/// MSB-first bit reader over an RBSP.
#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    #[inline]
    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn bits_left(&self) -> isize {
        (self.data.len() * 8) as isize - self.pos as isize
    }

    pub fn byte_aligned(&self) -> bool {
        self.pos % 8 == 0
    }

    /// Byte offset of the next unread byte (rounded up).
    pub fn byte_pos_ceil(&self) -> usize {
        self.pos.div_ceil(8)
    }

    #[inline]
    pub fn u1(&mut self) -> Result<bool> {
        Ok(self.u(1)? != 0)
    }

    #[inline]
    pub fn flag(&mut self) -> Result<bool> {
        self.u1()
    }

    /// Reads `n` (0..=32) bits.
    pub fn u(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        debug_assert!(n <= 32);
        if self.bits_left() < n as isize {
            return Err(Error::UnexpectedEof);
        }
        let mut v: u64 = 0;
        let mut need = n;
        while need > 0 {
            let byte = self.data[self.pos >> 3] as u64;
            let bit_off = (self.pos & 7) as u32;
            let avail = 8 - bit_off;
            let take = avail.min(need);
            let shifted = (byte >> (avail - take)) & ((1u64 << take) - 1);
            v = (v << take) | shifted;
            need -= take;
            self.pos += take as usize;
        }
        Ok(v as u32)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.bits_left() < n as isize {
            return Err(Error::UnexpectedEof);
        }
        self.pos += n;
        Ok(())
    }

    /// Unsigned Exp-Golomb code (values up to 2^32 - 2).
    pub fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0u32;
        while !self.u1()? {
            zeros += 1;
            if zeros > 31 {
                return Err(Error::Invalid("exp-golomb code too long"));
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let rest = self.u(zeros)? as u64;
        let v = (1u64 << zeros) - 1 + rest;
        if v > u32::MAX as u64 - 1 {
            return Err(Error::Invalid("exp-golomb value overflow"));
        }
        Ok(v as u32)
    }

    /// Unsigned Exp-Golomb with an inclusive upper bound check.
    pub fn ue_max(&mut self, max: u32, what: &'static str) -> Result<u32> {
        let v = self.ue()?;
        if v > max {
            return Err(Error::Invalid(what));
        }
        Ok(v)
    }

    /// Signed Exp-Golomb code.
    pub fn se(&mut self) -> Result<i32> {
        let k = self.ue()? as i64;
        let v = if k & 1 == 1 { (k + 1) / 2 } else { -(k / 2) };
        Ok(v as i32)
    }

    /// Signed Exp-Golomb with an inclusive range check.
    pub fn se_range(&mut self, min: i32, max: i32, what: &'static str) -> Result<i32> {
        let v = self.se()?;
        if v < min || v > max {
            return Err(Error::Invalid(what));
        }
        Ok(v)
    }

    pub fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// True if there is more RBSP data before the rbsp_trailing_bits.
    pub fn more_rbsp_data(&self) -> bool {
        // Find the last 1 bit in the data (the rbsp_stop_one_bit).
        let mut last = self.data.len();
        while last > 0 && self.data[last - 1] == 0 {
            last -= 1;
        }
        if last == 0 {
            return false;
        }
        let b = self.data[last - 1];
        let tz = b.trailing_zeros() as usize;
        let stop_bit_pos = (last - 1) * 8 + (7 - tz);
        self.pos < stop_bit_pos
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epb_removal() {
        assert_eq!(nal_to_rbsp(&[0, 0, 3, 1, 0, 0, 3, 0, 0, 3]), vec![0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(nal_to_rbsp(&[0, 3, 0, 0, 3]), vec![0, 3, 0, 0]);
    }

    #[test]
    fn exp_golomb() {
        // 1 010 011 00100 00101 -> 0 1 2 3 4
        let data = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.ue().unwrap(), 0);
        assert_eq!(r.ue().unwrap(), 1);
        assert_eq!(r.ue().unwrap(), 2);
        assert_eq!(r.ue().unwrap(), 3);
        assert_eq!(r.ue().unwrap(), 4);
        let data = [0b0100_1110, 0b1000_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.se().unwrap(), 1);
        assert_eq!(r.se().unwrap(), -1);
        assert_eq!(r.se().unwrap(), 0);
    }

    #[test]
    fn more_data() {
        let data = [0b1010_0000];
        let mut r = BitReader::new(&data);
        assert!(r.more_rbsp_data());
        r.u(1).unwrap();
        assert!(r.more_rbsp_data());
        r.u(1).unwrap();
        assert!(!r.more_rbsp_data());
    }
}
