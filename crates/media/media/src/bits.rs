// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MSB-first bit packing with Exp-Golomb codes.

#[derive(Default)]
pub struct BitWriter {
    pub bytes: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes the low `count` bits of `value` (count ≤ 32).
    pub fn put(&mut self, value: u32, count: u32) {
        debug_assert!(count <= 32);
        if count == 0 {
            return;
        }
        self.acc = (self.acc << count) | u64::from(value & (u32::MAX >> (32 - count)));
        self.n += count;
        while self.n >= 8 {
            self.n -= 8;
            self.bytes.push((self.acc >> self.n) as u8);
        }
    }

    pub fn bit(&mut self, b: bool) {
        self.put(u32::from(b), 1);
    }

    /// Unsigned Exp-Golomb.
    pub fn ue(&mut self, v: u32) {
        let x = u64::from(v) + 1;
        let len = 64 - x.leading_zeros();
        self.put(0, len - 1);
        if len > 32 {
            self.put((x >> 32) as u32, len - 32);
            self.put(x as u32, 32);
        } else {
            self.put(x as u32, len);
        }
    }

    /// Signed Exp-Golomb: 0, 1, -1, 2, -2, ...
    pub fn se(&mut self, v: i32) {
        let m = if v > 0 { (v as u32) * 2 - 1 } else { (-(v as i64) as u32) * 2 };
        self.ue(m);
    }

    /// Pads to a byte boundary with zero bits and returns the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(0, pad);
        }
        self.bytes
    }
}

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfData;

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn get(&mut self, count: u32) -> Result<u32, OutOfData> {
        let mut v = 0u32;
        for _ in 0..count {
            let byte = *self.data.get(self.pos >> 3).ok_or(OutOfData)?;
            let bit = (byte >> (7 - (self.pos & 7))) & 1;
            v = (v << 1) | u32::from(bit);
            self.pos += 1;
        }
        Ok(v)
    }

    pub fn bit(&mut self) -> Result<bool, OutOfData> {
        Ok(self.get(1)? == 1)
    }

    pub fn ue(&mut self) -> Result<u32, OutOfData> {
        let mut zeros = 0;
        while !self.bit()? {
            zeros += 1;
            if zeros > 32 {
                return Err(OutOfData);
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let rest = if zeros > 31 { (u64::from(self.get(zeros - 32)?) << 32) | u64::from(self.get(32)?) } else { u64::from(self.get(zeros)?) };
        let v = (1u64 << zeros) + rest - 1;
        u32::try_from(v).map_err(|_| OutOfData)
    }

    pub fn se(&mut self) -> Result<i32, OutOfData> {
        let m = self.ue()?;
        Ok(if m % 2 == 1 { m.div_ceil(2) as i32 } else { -((m / 2) as i32) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_round_trip() {
        let mut w = BitWriter::new();
        let us = [0u32, 1, 2, 3, 7, 8, 255, 65_535, 1 << 20, u32::MAX - 1];
        let ss = [0i32, 1, -1, 2, -2, 1000, -1000, i32::MAX / 2, -(i32::MAX / 2)];
        for &u in &us {
            w.ue(u);
        }
        for &s in &ss {
            w.se(s);
        }
        w.put(0b101, 3);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for &u in &us {
            assert_eq!(r.ue().unwrap(), u);
        }
        for &s in &ss {
            assert_eq!(r.se().unwrap(), s);
        }
        assert_eq!(r.get(3).unwrap(), 0b101);
    }

    #[test]
    fn known_codes() {
        let mut w = BitWriter::new();
        w.ue(0); // 1
        w.ue(1); // 010
        w.ue(4); // 00101
        assert_eq!(w.finish(), vec![0b1010_0010, 0b1000_0000]);
    }

    #[test]
    fn reading_past_end_fails() {
        let mut r = BitReader::new(&[0]);
        assert!(r.ue().is_err());
    }
}
