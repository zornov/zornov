// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M1: tests for the `bits` module. `Reader` below is a straight transcription of the descriptors
//! in spec 4.10 and is independent of the writer.

use studio_av1_enc::bits::BitWriter;

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    pub fn bit(&mut self) -> u32 {
        let b = (self.data[self.pos >> 3] >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        b as u32
    }
    pub fn f(&mut self, n: u32) -> u32 {
        (0..n).fold(0u64, |x, _| (x << 1) | self.bit() as u64) as u32
    }
    pub fn leb128(&mut self) -> u64 {
        let mut v = 0u64;
        for i in 0..8 {
            let b = self.f(8) as u64;
            v |= (b & 0x7f) << (i * 7);
            if b & 0x80 == 0 {
                break;
            }
        }
        v
    }
    pub fn uvlc(&mut self) -> u32 {
        let mut lz = 0;
        while self.bit() == 0 {
            lz += 1;
            if lz >= 32 {
                return u32::MAX;
            }
        }
        self.f(lz) + ((1u64 << lz) - 1) as u32
    }
    pub fn su(&mut self, n: u32) -> i32 {
        let v = self.f(n) as i64;
        let sign = 1i64 << (n - 1);
        (if v & sign != 0 { v - 2 * sign } else { v }) as i32
    }
    pub fn ns(&mut self, n: u32) -> u32 {
        let w = 32 - n.leading_zeros();
        let m = (1u32 << w) - n;
        let v = self.f(w - 1);
        if v < m {
            return v;
        }
        let extra = self.bit();
        (v << 1) - m + extra
    }
    pub fn le(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |t, i| t | self.f(8) << (8 * i))
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
}

#[test]
fn f_n_msb_first() {
    let mut w = BitWriter::new();
    w.put(0b101, 3);
    w.put(0b1, 1);
    w.put(0xA5, 8);
    assert_eq!(w.bit_len(), 12);
    assert_eq!(w.finish(), vec![0b1011_1010, 0b0101_0000]);
}

#[test]
fn f_n_random_widths_round_trip() {
    let mut rng = Lcg(1);
    let mut items = Vec::new();
    let mut w = BitWriter::new();
    for _ in 0..5000 {
        let n = (rng.next() % 33) as u32;
        let v = if n == 0 {
            0
        } else {
            (rng.next() as u32) >> (32 - n)
        };
        w.put(v, n);
        items.push((v, n));
    }
    let bytes = w.finish();
    let mut r = Reader::new(&bytes);
    for (v, n) in items {
        assert_eq!(r.f(n), v, "f({n})");
    }
}

#[test]
fn flag_and_zero_width() {
    let mut w = BitWriter::new();
    w.flag(true);
    w.put(0, 0);
    w.flag(false);
    w.flag(true);
    assert_eq!(w.finish(), vec![0b1010_0000]);
}

#[test]
fn leb128_known_and_round_trip() {
    let enc = |v: u64| {
        let mut w = BitWriter::new();
        w.leb128(v);
        w.finish()
    };
    assert_eq!(enc(0), [0x00]);
    assert_eq!(enc(127), [0x7f]);
    assert_eq!(enc(128), [0x80, 0x01]);
    assert_eq!(enc(300), [0xac, 0x02]);
    assert_eq!(enc((1 << 56) - 1).len(), 8);
    let mut rng = Lcg(2);
    for _ in 0..2000 {
        let v = rng.next() >> (rng.next() % 40);
        let b = enc(v);
        assert_eq!(Reader::new(&b).leb128(), v);
    }
}

#[test]
fn uvlc_known_and_round_trip() {
    let enc = |v: u32| {
        let mut w = BitWriter::new();
        w.uvlc(v);
        (w.bit_len(), w.finish())
    };
    assert_eq!(enc(0), (1, vec![0x80]));
    assert_eq!(enc(1), (3, vec![0b0100_0000]));
    assert_eq!(enc(2), (3, vec![0b0110_0000]));
    assert_eq!(enc(3), (5, vec![0b0010_0000]));
    let mut vals: Vec<u32> = (0..2000).collect();
    let mut rng = Lcg(3);
    vals.extend((0..2000).map(|_| (rng.next() as u32) >> (rng.next() % 32)));
    vals.extend([u32::MAX - 1, u32::MAX, 1 << 31, (1 << 31) - 1]);
    for v in vals {
        let (_, b) = enc(v);
        assert_eq!(Reader::new(&b).uvlc(), v, "uvlc({v})");
    }
}

#[test]
fn su_round_trip_all_small_widths() {
    for n in 1..=16u32 {
        let (lo, hi) = (-(1i32 << (n - 1)), (1i32 << (n - 1)) - 1);
        let mut w = BitWriter::new();
        let vals: Vec<i32> = if n <= 8 {
            (lo..=hi).collect()
        } else {
            vec![lo, lo + 1, -1, 0, 1, hi - 1, hi]
        };
        for &v in &vals {
            w.su(v, n);
        }
        let b = w.finish();
        let mut r = Reader::new(&b);
        for &v in &vals {
            assert_eq!(r.su(n), v, "su({n})");
        }
    }
    let mut w = BitWriter::new();
    w.su(i32::MIN, 32);
    w.su(i32::MAX, 32);
    let b = w.finish();
    let mut r = Reader::new(&b);
    assert_eq!((r.su(32), r.su(32)), (i32::MIN, i32::MAX));
}

#[test]
fn ns_round_trip_every_value() {
    for n in 1..=300u32 {
        let mut w = BitWriter::new();
        for v in 0..n {
            w.ns(v, n);
        }
        let b = w.finish();
        let mut r = Reader::new(&b);
        for v in 0..n {
            assert_eq!(r.ns(n), v, "ns({n})");
        }
    }
    // n = 5: w = 3, m = 3: values 0..3 use two bits, 3 and 4 use three.
    let mut w = BitWriter::new();
    w.ns(2, 5);
    w.ns(3, 5);
    w.ns(4, 5);
    assert_eq!(w.bit_len(), 2 + 3 + 3);
}

#[test]
fn le_is_little_endian_bytes() {
    let mut w = BitWriter::new();
    w.le(0x0102_0304, 4);
    w.le(0x0506, 2);
    assert_eq!(w.as_bytes(), [4, 3, 2, 1, 6, 5]);
    let b = w.finish();
    let mut r = Reader::new(&b);
    assert_eq!((r.le(4), r.le(2)), (0x0102_0304, 0x0506));
}

#[test]
fn trailing_bits_and_alignment() {
    let mut w = BitWriter::new();
    w.put(0b11, 2);
    w.trailing_bits();
    assert!(w.is_byte_aligned());
    assert_eq!(w.as_bytes(), [0b1110_0000]);
    // Already aligned: trailing_bits is a whole 0x80 byte.
    w.trailing_bits();
    assert_eq!(w.as_bytes(), [0b1110_0000, 0x80]);

    let mut w = BitWriter::new();
    w.put(1, 1);
    w.byte_align();
    w.byte_align();
    w.put_bytes(&[0xAB]);
    assert_eq!(w.bit_len(), 16);
    assert_eq!(w.finish(), vec![0x80, 0xAB]);
}

#[test]
fn finish_pads_partial_byte_with_zeros() {
    let mut w = BitWriter::new();
    w.put(0b111, 3);
    assert_eq!(w.finish(), vec![0b1110_0000]);
    assert!(BitWriter::new().finish().is_empty());
}
