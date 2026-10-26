// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! DEFLATE (RFC 1951) and zlib (RFC 1950).
//!
//! The decoder handles stored, fixed-Huffman and dynamic-Huffman blocks. The
//! encoder does LZ77 matching with hash chains and emits fixed-Huffman blocks,
//! which compresses typical image rows well without building per-block trees.

use crate::png::adler32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    Truncated,
    BadBlockType,
    BadStoredLength,
    BadCodeLengths,
    BadSymbol,
    BadDistance,
    BadZlibHeader,
    ChecksumMismatch,
    TooLarge,
}

impl std::fmt::Display for InflateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deflate stream error: {self:?}")
    }
}

impl std::error::Error for InflateError {}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

// --- decoder ----------------------------------------------------------------

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, acc: 0, n: 0 }
    }
    fn need(&mut self, k: u32) -> Result<(), InflateError> {
        while self.n < k {
            let b = *self.data.get(self.pos).ok_or(InflateError::Truncated)?;
            self.pos += 1;
            self.acc |= u64::from(b) << self.n;
            self.n += 8;
        }
        Ok(())
    }
    /// LSB-first read of `k` bits.
    fn get(&mut self, k: u32) -> Result<u32, InflateError> {
        if k == 0 {
            return Ok(0);
        }
        self.need(k)?;
        let v = (self.acc & ((1u64 << k) - 1)) as u32;
        self.acc >>= k;
        self.n -= k;
        Ok(v)
    }
    fn align(&mut self) {
        let drop = self.n % 8;
        self.acc >>= drop;
        self.n -= drop;
    }
}

/// Canonical Huffman decoding table: (count per length, symbols sorted by code).
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, InflateError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut left: i32 = 1;
        for len in 1..16 {
            left = left * 2 - i32::from(counts[len]);
            if left < 0 {
                return Err(InflateError::BadCodeLengths);
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, bits: &mut Bits) -> Result<u16, InflateError> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.get(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::BadSymbol)
    }
}

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit = [0u8; 288];
    lit[..144].fill(8);
    lit[144..256].fill(9);
    lit[256..280].fill(7);
    lit[280..].fill(8);
    (Huffman::new(&lit).unwrap(), Huffman::new(&[5u8; 30]).unwrap())
}

fn dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = bits.get(5)? as usize + 257;
    let hdist = bits.get(5)? as usize + 1;
    let hclen = bits.get(4)? as usize + 4;
    let mut cl = [0u8; 19];
    for &i in CL_ORDER.iter().take(hclen) {
        cl[i] = bits.get(3)? as u8;
    }
    let clh = Huffman::new(&cl)?;
    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = clh.decode(bits)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => (*lengths.get(i.wrapping_sub(1)).ok_or(InflateError::BadCodeLengths)?, 3 + bits.get(2)? as usize),
            17 => (0, 3 + bits.get(3)? as usize),
            18 => (0, 11 + bits.get(7)? as usize),
            _ => return Err(InflateError::BadCodeLengths),
        };
        if i + repeat > lengths.len() {
            return Err(InflateError::BadCodeLengths);
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(InflateError::BadCodeLengths);
    }
    Ok((Huffman::new(&lengths[..hlit])?, Huffman::new(&lengths[hlit..])?))
}

/// Raw DEFLATE decode; `limit` caps output size (decompression-bomb guard).
pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
    let mut bits = Bits::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = bits.get(1)? == 1;
        match bits.get(2)? {
            0 => {
                bits.align();
                let len = bits.get(16)? as usize;
                let nlen = bits.get(16)? as usize;
                if len != !nlen & 0xffff {
                    return Err(InflateError::BadStoredLength);
                }
                for _ in 0..len {
                    out.push(bits.get(8)? as u8);
                }
            }
            kind @ (1 | 2) => {
                let (lit, dist) = if kind == 1 { fixed_tables() } else { dynamic_tables(&mut bits)? };
                loop {
                    let sym = lit.decode(&mut bits)? as usize;
                    if sym < 256 {
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let li = sym - 257;
                        if li >= 29 {
                            return Err(InflateError::BadSymbol);
                        }
                        let len = LEN_BASE[li] as usize + bits.get(u32::from(LEN_EXTRA[li]))? as usize;
                        let di = dist.decode(&mut bits)? as usize;
                        if di >= 30 {
                            return Err(InflateError::BadDistance);
                        }
                        let d = DIST_BASE[di] as usize + bits.get(u32::from(DIST_EXTRA[di]))? as usize;
                        if d > out.len() {
                            return Err(InflateError::BadDistance);
                        }
                        let from = out.len() - d;
                        for k in 0..len {
                            out.push(out[from + k]);
                        }
                    }
                    if out.len() > limit {
                        return Err(InflateError::TooLarge);
                    }
                }
            }
            _ => return Err(InflateError::BadBlockType),
        }
        if out.len() > limit {
            return Err(InflateError::TooLarge);
        }
        if last {
            return Ok(out);
        }
    }
}

pub fn zlib_decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
    if data.len() < 6 || (u16::from(data[0]) << 8 | u16::from(data[1])) % 31 != 0 || data[0] & 0x0f != 8 || data[1] & 0x20 != 0 {
        return Err(InflateError::BadZlibHeader);
    }
    let out = inflate(&data[2..], limit)?;
    let n = data.len();
    let want = u32::from_be_bytes([data[n - 4], data[n - 3], data[n - 2], data[n - 1]]);
    if adler32(&out) != want {
        return Err(InflateError::ChecksumMismatch);
    }
    Ok(out)
}

// --- encoder ----------------------------------------------------------------

struct OutBits {
    bytes: Vec<u8>,
    acc: u64,
    n: u32,
}

impl OutBits {
    fn put(&mut self, v: u32, k: u32) {
        self.acc |= u64::from(v) << self.n;
        self.n += k;
        while self.n >= 8 {
            self.bytes.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// Huffman codes are sent MSB-first.
    fn put_code(&mut self, code: u32, len: u32) {
        let mut rev = 0;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(rev, len);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.bytes.push(self.acc as u8);
        }
        self.bytes
    }
}

fn fixed_lit_code(sym: usize) -> (u32, u32) {
    match sym {
        0..=143 => (0x30 + sym as u32, 8),
        144..=255 => (0x190 + (sym - 144) as u32, 9),
        256..=279 => ((sym - 256) as u32, 7),
        _ => (0xc0 + (sym - 280) as u32, 8),
    }
}

fn emit_length(out: &mut OutBits, len: usize) {
    let li = LEN_BASE.iter().rposition(|&b| b as usize <= len).unwrap();
    let (code, bits) = fixed_lit_code(257 + li);
    out.put_code(code, bits);
    out.put((len - LEN_BASE[li] as usize) as u32, u32::from(LEN_EXTRA[li]));
}

fn emit_distance(out: &mut OutBits, d: usize) {
    let di = DIST_BASE.iter().rposition(|&b| b as usize <= d).unwrap();
    out.put_code(di as u32, 5);
    out.put((d - DIST_BASE[di] as usize) as u32, u32::from(DIST_EXTRA[di]));
}

/// Raw DEFLATE with LZ77 (32 KiB window, hash chains) and fixed Huffman codes.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32_768;
    const HASH_BITS: u32 = 15;
    const MAX_CHAIN: usize = 64;
    let mut out = OutBits { bytes: Vec::with_capacity(data.len() / 2), acc: 0, n: 0 };
    out.put(1, 1); // final block
    out.put(1, 2); // fixed Huffman
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; data.len()];
    let hash = |i: usize| -> usize {
        let v = u32::from(data[i]) << 16 | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]);
        (v.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
    };
    let insert = |i: usize, head: &mut Vec<usize>, prev: &mut Vec<usize>| {
        if i + 2 < data.len() {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i;
        }
    };
    let mut i = 0;
    while i < data.len() {
        let (mut best_len, mut best_dist) = (0usize, 0usize);
        if i + 2 < data.len() {
            let mut cand = head[hash(i)];
            let mut chain = 0;
            let max = (data.len() - i).min(258);
            while cand != usize::MAX && i - cand <= WINDOW && chain < MAX_CHAIN {
                let mut l = 0;
                while l < max && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = i - cand;
                    if l == max {
                        break;
                    }
                }
                cand = prev[cand];
                chain += 1;
            }
        }
        if best_len >= 3 {
            emit_length(&mut out, best_len);
            emit_distance(&mut out, best_dist);
            for k in 0..best_len {
                insert(i + k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            let (code, bits) = fixed_lit_code(data[i] as usize);
            out.put_code(code, bits);
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    let (code, bits) = fixed_lit_code(256);
    out.put_code(code, bits);
    out.finish()
}

pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x5e];
    out.extend(deflate(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut samples: Vec<Vec<u8>> = vec![vec![], b"a".to_vec(), b"abcabcabcabcabcabc".to_vec(), vec![0u8; 100_000]];
        let mut x = 1u32;
        samples.push((0..50_000).map(|_| { x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345); (x >> 16) as u8 }).collect());
        samples.push((0..70_000u32).map(|i| (i % 251) as u8).collect());
        for s in samples {
            let z = zlib_compress(&s);
            assert_eq!(zlib_decompress(&z, 1 << 24).unwrap(), s);
        }
        let zeros = zlib_compress(&[0u8; 100_000]);
        assert!(zeros.len() < 1_500, "{}", zeros.len());
    }

    #[test]
    fn decodes_reference_streams() {
        // "hello hello hello" from zlib level 9 (fixed-Huffman block).
        let z = [0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x90, 0x00, 0x3a, 0x2e, 0x06, 0x7d];
        assert_eq!(zlib_decompress(&z, 1024).unwrap(), b"hello hello hello");
        // Dynamic-Huffman block produced by zlib level 9.
        let dynamic = include_bytes!("../testdata/dynamic.zlib");
        assert_eq!((dynamic[2] >> 1) & 3, 2);
        assert_eq!(zlib_decompress(dynamic, 1 << 20).unwrap(), include_bytes!("../testdata/dynamic.txt"));
        // stored block wrapper from png::zlib_stored
        let s = crate::png::zlib_stored(b"stored data");
        assert_eq!(zlib_decompress(&s, 1024).unwrap(), b"stored data");
    }

    #[test]
    fn rejects_corruption_and_bombs() {
        let mut z = zlib_compress(b"some text to compress, some text to compress");
        let n = z.len();
        z[n - 1] ^= 1;
        assert_eq!(zlib_decompress(&z, 1024).unwrap_err(), InflateError::ChecksumMismatch);
        assert_eq!(zlib_decompress(&zlib_compress(&[0u8; 10_000]), 100).unwrap_err(), InflateError::TooLarge);
        assert!(inflate(&[0b111], 10).is_err());
    }
}
