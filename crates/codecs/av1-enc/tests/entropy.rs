// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M2: the symbol encoder against `studio_av1`'s symbol decoder.

use studio_av1::cdf::Cdfs;
use studio_av1::symbol::SymbolDecoder;
use studio_av1_enc::entropy::cdf_ctx::CdfContext;
use studio_av1_enc::entropy::SymbolEncoder;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() >> 11) as usize % n
    }
}

/// A random valid CDF over `n` symbols (increasing, ending at 32768) plus the counter.
fn random_cdf(rng: &mut Rng, n: usize) -> Vec<u16> {
    let mut cuts: Vec<u16> = (0..n - 1).map(|_| 1 + rng.below(32767) as u16).collect();
    cuts.sort_unstable();
    for i in 1..cuts.len() {
        if cuts[i] <= cuts[i - 1] {
            cuts[i] = cuts[i - 1] + 1;
        }
    }
    // keep room for the last symbol
    for i in (0..cuts.len()).rev() {
        let max = 32767 - (cuts.len() - 1 - i) as u16;
        cuts[i] = cuts[i].min(max);
    }
    cuts.push(32768);
    cuts.push(0);
    cuts
}

/// Picks a symbol; `skew` makes one symbol overwhelmingly likely (long runs, carry chains).
fn pick(rng: &mut Rng, n: usize, skew: bool) -> usize {
    if skew && rng.below(64) != 0 { n - 1 } else { rng.below(n) }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Sym(usize, usize), // cdf index, symbol
    Bool(bool),
    Lit(u32, u32),
    Ns(u32, u32),
}

fn run_case(seed: u64, alphabet: usize, count: usize, adapt: bool, skew: bool) {
    let mut rng = Rng(seed | 1);
    let cdfs0: Vec<Vec<u16>> = (0..4).map(|_| random_cdf(&mut rng, alphabet)).collect();
    let mut ops = Vec::with_capacity(count);
    for _ in 0..count {
        ops.push(match rng.below(8) {
            0 => Op::Bool(rng.below(2) == 1),
            1 => {
                let n = 1 + rng.below(16) as u32;
                Op::Lit(rng.next() as u32 & ((1u32 << n) - 1), n)
            }
            2 => {
                let n = 2 + rng.below(300) as u32;
                Op::Ns(rng.below(n as usize) as u32, n)
            }
            _ => Op::Sym(rng.below(4), pick(&mut rng, alphabet, skew)),
        });
    }
    let mut enc_cdfs = cdfs0.clone();
    let mut enc = SymbolEncoder::with_disable_update(!adapt);
    for op in &ops {
        match *op {
            Op::Sym(c, s) => enc.write_symbol(&mut enc_cdfs[c], s),
            Op::Bool(b) => enc.write_bool(b),
            Op::Lit(v, n) => enc.write_literal(v, n),
            Op::Ns(v, n) => enc.write_ns(v, n),
        }
    }
    let bytes = enc.finish();
    assert!(bytes.len() <= count * 4 + 16, "output unexpectedly large: {}", bytes.len());
    let mut dec_cdfs = cdfs0;
    let mut dec = SymbolDecoder::new(&bytes, !adapt);
    for (i, op) in ops.iter().enumerate() {
        match *op {
            Op::Sym(c, s) => assert_eq!(dec.read_symbol(&mut dec_cdfs[c]), s, "seed {seed} n {alphabet} op {i}"),
            Op::Bool(b) => assert_eq!(dec.read_bool() == 1, b, "seed {seed} op {i}"),
            Op::Lit(v, n) => assert_eq!(dec.read_literal(n), v, "seed {seed} op {i}"),
            Op::Ns(v, n) => assert_eq!(dec.read_ns(n), v, "seed {seed} op {i}"),
        }
    }
    assert_eq!(enc_cdfs, dec_cdfs, "adapted cdf state, seed {seed} n {alphabet}");
}

#[test]
fn random_sequences_every_alphabet_size() {
    for alphabet in 2..=16 {
        for adapt in [true, false] {
            for skew in [false, true] {
                for rep in 0..10u64 {
                    run_case(0x9E3779B97F4A7C15 ^ (alphabet as u64 * 1000 + rep), alphabet, 25_000, adapt, skew);
                }
            }
        }
    }
}

#[test]
fn many_short_streams() {
    // Short tiles stress finish(): padding, carry into the first byte, empty and one-symbol streams.
    for seed in 0..20_000u64 {
        let alphabet = 2 + (seed % 15) as usize;
        run_case(seed.wrapping_mul(0x2545F4914F6CDD1D), alphabet, (seed % 12) as usize, seed % 3 != 0, seed % 2 == 0);
    }
}

#[test]
fn empty_stream_is_one_trailing_byte() {
    let bytes = SymbolEncoder::new().finish();
    assert_eq!(bytes, vec![0x80]);
}

#[test]
fn trailing_bit_is_a_one_then_zeros() {
    // Spec exit process: after the symbol data, a 1 bit and zero padding up to the byte boundary.
    let mut rng = Rng(7);
    for _ in 0..2000 {
        let mut enc = SymbolEncoder::new();
        for _ in 0..rng.below(200) {
            enc.write_bool(rng.below(2) == 1);
        }
        let bytes = enc.finish();
        let last = *bytes.last().unwrap();
        assert_ne!(last, 0, "last byte must hold the trailing one bit");
    }
}

#[test]
fn long_ff_runs_carry() {
    // A near-certain symbol coded many times drives the interval into long carry chains.
    for n in [2usize, 3, 8, 16] {
        for last in [false, true] {
            let mut cdf: Vec<u16> = (1..n as u16).map(|i| if last { 32768 - (n as u16 - i) } else { i }).collect();
            cdf.push(32768);
            cdf.push(0);
            let mut ops = Vec::new();
            let mut rng = Rng(99 + n as u64);
            for _ in 0..200_000 {
                ops.push(if rng.below(5000) == 0 { rng.below(n) } else if last { n - 1 } else { 0 });
            }
            let mut e = cdf.clone();
            let mut enc = SymbolEncoder::with_disable_update(true);
            for &s in &ops {
                enc.write_symbol(&mut e, s);
            }
            let bytes = enc.finish();
            let mut d = cdf.clone();
            let mut dec = SymbolDecoder::new(&bytes, true);
            for (i, &s) in ops.iter().enumerate() {
                assert_eq!(dec.read_symbol(&mut d), s, "n {n} last {last} op {i}");
            }
        }
    }
}

macro_rules! real_cdf_fields {
    ($($f:ident),*) => {
        /// Flat list of every innermost cdf slice of the chosen fields.
        fn flat(c: &mut Cdfs) -> Vec<&mut [u16]> {
            let mut v: Vec<&mut [u16]> = Vec::new();
            $( flatten(&mut c.$f, &mut v); )*
            v
        }
    };
}
trait Flatten { fn flat<'a>(&'a mut self, out: &mut Vec<&'a mut [u16]>); }
impl<const N: usize> Flatten for [u16; N] {
    fn flat<'a>(&'a mut self, out: &mut Vec<&'a mut [u16]>) { out.push(&mut self[..]); }
}
impl<T: Flatten, const M: usize> Flatten for [T; M] {
    fn flat<'a>(&'a mut self, out: &mut Vec<&'a mut [u16]>) { for e in self.iter_mut() { e.flat(out); } }
}
fn flatten<'a, T: Flatten>(t: &'a mut T, out: &mut Vec<&'a mut [u16]>) { t.flat(out); }
real_cdf_fields!(
    y_mode, uv_mode_cfl_allowed, angle_delta, partition_w8, partition_w16, partition_w64, partition_w128, segment_id,
    tx_16x16, txfm_split, filter_intra_mode, eob_pt_1024, eob_extra, dc_sign, coeff_base_eob, coeff_base, coeff_br
);

#[test]
fn real_default_cdfs_round_trip_and_adapt_identically() {
    for (qidx, seed) in [(0u32, 1u64), (40, 2), (100, 3), (200, 4)] {
        let mut enc_set = CdfContext::new(qidx);
        let mut dec_set = CdfContext::new(qidx);
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15));
        let lens: Vec<usize> = flat(&mut enc_set.cdfs).iter().map(|c| c.len() - 1).collect();
        let nslices = lens.len();
        let mut ops = Vec::new();
        for _ in 0..200_000 {
            let c = rng.below(nslices);
            let n = lens[c];
            // Skew towards low symbols like real data, with the odd outlier.
            let s = if rng.below(4) == 0 { rng.below(n) } else { rng.below(n).min(rng.below(n)) };
            ops.push((c, s));
        }
        // flat() borrows the whole set, so index per op through a single Vec of slices.
        let mut enc = SymbolEncoder::new();
        {
            let mut slices = flat(&mut enc_set.cdfs);
            for &(c, s) in &ops {
                enc.write_symbol(slices[c], s);
            }
        }
        let bytes = enc.finish();
        assert!(bytes.len() < 1_000_000);
        let mut dec = SymbolDecoder::new(&bytes, false);
        {
            let mut slices = flat(&mut dec_set.cdfs);
            for (i, &(c, s)) in ops.iter().enumerate() {
                assert_eq!(dec.read_symbol(slices[c]), s, "qidx {qidx} op {i}");
            }
        }
        assert_eq!(flat(&mut enc_set.cdfs), flat(&mut dec_set.cdfs), "adapted state, qidx {qidx}");
    }
}

#[test]
fn cdf_context_save_load_clears_counters() {
    let mut ctx = CdfContext::new(60);
    let mut enc = SymbolEncoder::new();
    for i in 0..50 {
        enc.write_symbol(&mut ctx.cdfs.y_mode[0], i % 13);
    }
    let saved = ctx.save();
    assert_ne!(saved.y_mode[0][13], 0);
    let loaded = CdfContext::load(&saved);
    assert_eq!(loaded.cdfs.y_mode[0][13], 0);
    assert_eq!(loaded.cdfs.y_mode[0][..13], saved.y_mode[0][..13]);
}
