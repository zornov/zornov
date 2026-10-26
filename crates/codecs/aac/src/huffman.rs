// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Table-driven Huffman decoding.
//!
//! A [`Vlc`] is built from `(length, code)` pairs indexed by symbol. Decoding
//! peeks 32 bits, looks up a root table of `ROOT` bits and, for longer codes,
//! one second-level table. Code words that are not in the book decode to an
//! error instead of a symbol.

use crate::bits::BitReader;
use crate::error::{Error, Result};
use crate::huffman_tables as t;
use std::sync::OnceLock;

#[derive(Clone, Copy, Default)]
struct Entry {
    /// Symbol, or offset of the sub table when `len < 0`.
    sym: u16,
    /// Code length (> 0), `-(sub table bits)` for a link, 0 for invalid.
    len: i8,
}

pub(crate) struct Vlc {
    root_bits: u32,
    table: Vec<Entry>,
}

impl Vlc {
    /// Build from per-symbol code lengths and code words (MSB-first).
    pub fn new(lens: &[u8], codes: &[u32], root_bits: u32) -> Vlc {
        assert_eq!(lens.len(), codes.len());
        let root = 1usize << root_bits;
        let mut table = vec![Entry::default(); root];
        // First pass: short codes and the needed size of each sub table.
        let mut sub_bits = vec![0u32; root];
        for (sym, (&l, &c)) in lens.iter().zip(codes).enumerate() {
            let l = l as u32;
            if l == 0 {
                continue;
            }
            if l <= root_bits {
                let first = (c << (root_bits - l)) as usize;
                for e in &mut table[first..first + (1usize << (root_bits - l))] {
                    *e = Entry { sym: sym as u16, len: l as i8 };
                }
            } else {
                let prefix = (c >> (l - root_bits)) as usize;
                sub_bits[prefix] = sub_bits[prefix].max(l - root_bits);
            }
        }
        // Allocate sub tables.
        for prefix in 0..root {
            if sub_bits[prefix] > 0 {
                let off = table.len();
                table[prefix] = Entry { sym: off as u16, len: -(sub_bits[prefix] as i8) };
                table.resize(off + (1usize << sub_bits[prefix]), Entry::default());
            }
        }
        for (sym, (&l, &c)) in lens.iter().zip(codes).enumerate() {
            let l = l as u32;
            if l <= root_bits {
                continue;
            }
            let prefix = (c >> (l - root_bits)) as usize;
            let sb = sub_bits[prefix];
            let off = table[prefix].sym as usize;
            let rest = l - root_bits;
            let low = (c & ((1u32 << rest) - 1)) as usize;
            let first = low << (sb - rest);
            for e in &mut table[off + first..off + first + (1usize << (sb - rest))] {
                *e = Entry { sym: sym as u16, len: rest as i8 };
            }
        }
        Vlc { root_bits, table }
    }

    /// Decode one symbol.
    #[inline]
    pub fn decode(&self, br: &mut BitReader) -> Result<usize> {
        let v = br.peek32();
        let e = self.table[(v >> (32 - self.root_bits)) as usize];
        if e.len > 0 {
            br.skip(e.len as usize)?;
            return Ok(e.sym as usize);
        }
        if e.len == 0 {
            return Err(Error::InvalidData("invalid Huffman code word"));
        }
        let sb = (-e.len) as u32;
        let idx = ((v << self.root_bits) >> (32 - sb)) as usize;
        let e2 = self.table[e.sym as usize + idx];
        if e2.len <= 0 {
            return Err(Error::InvalidData("invalid Huffman code word"));
        }
        br.skip(self.root_bits as usize + e2.len as usize)?;
        Ok(e2.sym as usize)
    }
}

/// Shape of a spectral code book.
#[derive(Clone, Copy)]
pub(crate) struct SpecBook {
    pub dim: usize,
    pub signed: bool,
    /// Values per coefficient (`2 * lav + 1` signed, `lav + 1` unsigned).
    pub modulo: usize,
    /// Subtracted from each digit (lav for signed books, 0 otherwise).
    pub offset: i32,
}

pub(crate) const SPEC_BOOKS: [SpecBook; 11] = [
    SpecBook { dim: 4, signed: true, modulo: 3, offset: 1 },
    SpecBook { dim: 4, signed: true, modulo: 3, offset: 1 },
    SpecBook { dim: 4, signed: false, modulo: 3, offset: 0 },
    SpecBook { dim: 4, signed: false, modulo: 3, offset: 0 },
    SpecBook { dim: 2, signed: true, modulo: 9, offset: 4 },
    SpecBook { dim: 2, signed: true, modulo: 9, offset: 4 },
    SpecBook { dim: 2, signed: false, modulo: 8, offset: 0 },
    SpecBook { dim: 2, signed: false, modulo: 8, offset: 0 },
    SpecBook { dim: 2, signed: false, modulo: 13, offset: 0 },
    SpecBook { dim: 2, signed: false, modulo: 13, offset: 0 },
    SpecBook { dim: 2, signed: false, modulo: 17, offset: 0 },
];

pub(crate) struct CoreBooks {
    pub spectral: [Vlc; 11],
    /// Decoded tuple per code word index of each spectral book.
    pub values: [Vec<[i8; 4]>; 11],
    pub scalefactor: Vlc,
}

fn tuples(b: &SpecBook, n: usize) -> Vec<[i8; 4]> {
    (0..n)
        .map(|mut idx| {
            let mut v = [0i8; 4];
            for d in (0..b.dim).rev() {
                v[d] = ((idx % b.modulo) as i32 - b.offset) as i8;
                idx /= b.modulo;
            }
            v
        })
        .collect()
}

pub(crate) fn core_books() -> &'static CoreBooks {
    static BOOKS: OnceLock<CoreBooks> = OnceLock::new();
    BOOKS.get_or_init(|| {
        let src: [(&[u8], &[u32]); 11] = [
            (&t::SPEC1_LENS, &t::SPEC1_CODES),
            (&t::SPEC2_LENS, &t::SPEC2_CODES),
            (&t::SPEC3_LENS, &t::SPEC3_CODES),
            (&t::SPEC4_LENS, &t::SPEC4_CODES),
            (&t::SPEC5_LENS, &t::SPEC5_CODES),
            (&t::SPEC6_LENS, &t::SPEC6_CODES),
            (&t::SPEC7_LENS, &t::SPEC7_CODES),
            (&t::SPEC8_LENS, &t::SPEC8_CODES),
            (&t::SPEC9_LENS, &t::SPEC9_CODES),
            (&t::SPEC10_LENS, &t::SPEC10_CODES),
            (&t::SPEC11_LENS, &t::SPEC11_CODES),
        ];
        CoreBooks {
            spectral: std::array::from_fn(|i| Vlc::new(src[i].0, src[i].1, 9)),
            values: std::array::from_fn(|i| tuples(&SPEC_BOOKS[i], src[i].0.len())),
            scalefactor: Vlc::new(&t::SF_LENS, &t::SF_CODES, 9),
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Kraft sum == 1 and no code word is a prefix of another.
    pub fn assert_complete_prefix_code(lens: &[u8], codes: &[u32], name: &str) {
        let kraft: f64 = lens.iter().map(|&l| 2f64.powi(-(l as i32))).sum();
        assert!((kraft - 1.0).abs() < 1e-12, "{name}: kraft sum {kraft}");
        let mut words: Vec<String> = lens
            .iter()
            .zip(codes)
            .map(|(&l, &c)| format!("{:0width$b}", c, width = l as usize))
            .collect();
        for w in &words {
            assert!(w.len() <= 32);
        }
        words.sort();
        for p in words.windows(2) {
            assert!(!p[1].starts_with(&p[0]), "{name}: {} prefixes {}", p[0], p[1]);
        }
    }

    /// Encode every symbol of a book back to back and decode them again.
    pub fn assert_round_trip(lens: &[u8], codes: &[u32], root: u32) {
        let vlc = Vlc::new(lens, codes, root);
        let mut bits: Vec<bool> = Vec::new();
        let order: Vec<usize> = (0..lens.len()).rev().chain(0..lens.len()).collect();
        for &s in &order {
            for i in (0..lens[s]).rev() {
                bits.push((codes[s] >> i) & 1 != 0);
            }
        }
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, &b) in bits.iter().enumerate() {
            if b {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let mut br = BitReader::new(&bytes);
        for &s in &order {
            assert_eq!(vlc.decode(&mut br).unwrap(), s);
        }
        assert!(br.bits_left() < 8);
    }

    #[test]
    fn spectral_books_are_complete_and_decode() {
        let b = core_books();
        let src: [(&[u8], &[u32]); 11] = [
            (&t::SPEC1_LENS, &t::SPEC1_CODES),
            (&t::SPEC2_LENS, &t::SPEC2_CODES),
            (&t::SPEC3_LENS, &t::SPEC3_CODES),
            (&t::SPEC4_LENS, &t::SPEC4_CODES),
            (&t::SPEC5_LENS, &t::SPEC5_CODES),
            (&t::SPEC6_LENS, &t::SPEC6_CODES),
            (&t::SPEC7_LENS, &t::SPEC7_CODES),
            (&t::SPEC8_LENS, &t::SPEC8_CODES),
            (&t::SPEC9_LENS, &t::SPEC9_CODES),
            (&t::SPEC10_LENS, &t::SPEC10_CODES),
            (&t::SPEC11_LENS, &t::SPEC11_CODES),
        ];
        let sizes = [81, 81, 81, 81, 81, 81, 64, 64, 169, 169, 289];
        for (i, (l, c)) in src.iter().enumerate() {
            assert_eq!(l.len(), sizes[i]);
            assert_complete_prefix_code(l, c, &format!("spectral book {}", i + 1));
            assert_round_trip(l, c, 9);
            assert_round_trip(l, c, 5);
        }
        // Spot checks against ISO/IEC 14496-3 Table 4.A.2..4.A.12.
        assert_eq!((t::SPEC1_LENS[40], t::SPEC1_CODES[40]), (1, 0)); // (0,0,0,0)
        assert_eq!((t::SPEC11_LENS[288], t::SPEC11_CODES[288]), (5, 0x4)); // (16,16) escape pair
        assert_eq!(b.values[0][0], [-1, -1, -1, -1]);
        assert_eq!(b.values[10][16], [0, 16, 0, 0]);
        assert_eq!(b.values[4][80], [4, 4, 0, 0]);
    }

    #[test]
    fn scalefactor_book() {
        assert_complete_prefix_code(&t::SF_LENS, &t::SF_CODES, "scalefactor");
        assert_round_trip(&t::SF_LENS, &t::SF_CODES, 9);
        assert_eq!((t::SF_LENS[60], t::SF_CODES[60]), (1, 0));
        assert_eq!(t::SF_LENS[0], 18);
        // An all-ones run longer than any code word is invalid.
        let vlc = &core_books().scalefactor;
        let data = [0xffu8; 4];
        let mut br = BitReader::new(&data);
        assert!(vlc.decode(&mut br).is_ok()); // 19 ones is index 13
        let bad = Vlc::new(&[1, 2], &[0, 2], 4); // "11" unused
        let data = [0xc0u8];
        assert!(bad.decode(&mut BitReader::new(&data)).is_err());
    }
}
