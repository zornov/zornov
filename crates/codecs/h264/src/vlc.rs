// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generic multi-level lookup table for prefix (variable length) codes.

use crate::bitreader::BitReader;

#[derive(Clone, Copy, Default)]
struct Entry {
    /// Symbol, or offset of the sub-table when `len < 0`.
    sym: u16,
    /// Code length (> 0), `-(sub-table bits)` (< 0) or 0 for an invalid code.
    len: i8,
}

pub struct Vlc {
    table: Vec<Entry>,
    bits: u32,
}

#[derive(Clone, Copy)]
struct Code {
    code: u32,
    len: u32,
    sym: u16,
}

impl Vlc {
    /// Build from `(code bit string, symbol)` pairs. Empty strings are ignored.
    pub fn from_strings(codes: &[(&str, u16)], root_bits: u32) -> Vlc {
        let mut list = Vec::new();
        for &(s, sym) in codes {
            if s.is_empty() {
                continue;
            }
            let code = u32::from_str_radix(s, 2).expect("binary code");
            list.push(Code { code, len: s.len() as u32, sym });
        }
        let mut vlc = Vlc { table: Vec::new(), bits: root_bits };
        vlc.build(&list, 0, root_bits);
        vlc
    }

    /// Build the table for all codes, given `consumed` prefix bits already matched.
    /// Returns the offset of the created table.
    fn build(&mut self, codes: &[Code], consumed: u32, bits: u32) -> usize {
        let base = self.table.len();
        self.table.resize(base + (1usize << bits), Entry::default());
        // Codes that terminate in this level.
        for c in codes {
            let rem = c.len - consumed;
            if rem <= bits {
                let v = c.code & ((1u32 << rem) - 1);
                let start = (v << (bits - rem)) as usize;
                for k in 0..(1usize << (bits - rem)) {
                    self.table[base + start + k] = Entry { sym: c.sym, len: rem as i8 };
                }
            }
        }
        // Longer codes: group by their next `bits` bits.
        for idx in 0..(1u32 << bits) {
            let sub: Vec<Code> = codes
                .iter()
                .copied()
                .filter(|c| {
                    let rem = c.len - consumed;
                    rem > bits && ((c.code >> (rem - bits)) & ((1 << bits) - 1)) == idx
                })
                .collect();
            if sub.is_empty() {
                continue;
            }
            let max_rem = sub.iter().map(|c| c.len - consumed - bits).max().unwrap();
            let nb = max_rem.min(self.bits);
            let off = self.build(&sub, consumed + bits, nb);
            self.table[base + idx as usize] = Entry { sym: off as u16, len: -(nb as i8) };
        }
        base
    }

    /// Decode one symbol; `None` for an invalid code.
    #[inline]
    pub fn decode(&self, r: &mut BitReader) -> Option<u16> {
        let mut bits = self.bits;
        let mut e = self.table[r.peek(bits) as usize];
        loop {
            if e.len > 0 {
                r.skip(e.len as u32);
                return Some(e.sym);
            }
            if e.len == 0 {
                return None;
            }
            r.skip(bits);
            bits = (-e.len) as u32;
            e = self.table[e.sym as usize + r.peek(bits) as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_level() {
        let codes = [("1", 0u16), ("01", 1), ("001", 2), ("0001", 3), ("00001", 4), ("000001111", 5), ("000000000001", 6)];
        let vlc = Vlc::from_strings(&codes, 3);
        // Concatenate all codes.
        let s: String = codes.iter().map(|c| c.0).collect::<Vec<_>>().join("") + "1111111";
        let bytes: Vec<u8> = s
            .as_bytes()
            .chunks(8)
            .map(|c| {
                let mut v = 0u8;
                for (i, &b) in c.iter().enumerate() {
                    if b == b'1' {
                        v |= 0x80 >> i;
                    }
                }
                v
            })
            .collect();
        let mut r = BitReader::new(&bytes);
        for c in codes {
            assert_eq!(vlc.decode(&mut r), Some(c.1));
        }
    }
}
