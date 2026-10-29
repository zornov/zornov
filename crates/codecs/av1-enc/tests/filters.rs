// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M9: in-loop filters. The encoder's reconstruction must equal the decoder's output with each
//! filter on, and each filter must pay for itself.

use studio_av1::Decoder;
use studio_av1_enc::color::from_rgba8;
use studio_av1_enc::filters::FilterSet;
use studio_av1_enc::{Config, Encoder, Frame, RateMode};

fn noise(x: u32, y: u32, t: u32) -> i32 {
    let mut v = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca6b) ^ t.wrapping_mul(0xc2b2_ae35);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2c1b_3c6d);
    v ^= v >> 12;
    (v % 21) as i32 - 10
}

fn picture(w: u32, h: u32, shift: u32) -> Frame {
    // smooth gradients with a moving edge: blocking shows at coarse quantisers
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let edge = if (x + shift * 3) % 40 < 20 { 60 } else { 0 };
            [((x * 2 + y + shift * 2) as i32 % 256 + noise(x, y, shift)).clamp(0, 255) as u8, ((y * 3 % 200 + edge) as i32 + noise(y, x, shift)).clamp(0, 255) as u8, ((x * y / 20 % 256) as i32 + noise(x + 7, y, shift)).clamp(0, 255) as u8, 255]
        })
        .collect();
    from_rgba8(&rgba, w, h, 8, false)
}

fn sse(a: &[u16], b: &[u16]) -> u64 {
    a.iter().zip(b).map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64).sum()
}

/// Encodes `n` frames; (packet bytes, squared error against the source, reconstruction == decode).
fn run(filters: FilterSet, q: u8, n: u32) -> (usize, u64) {
    let (w, h) = (130, 70);
    let mut e = Encoder::new(Config { speed: 6, keyint: 4, rate: RateMode::Cqp(q), ..Config::new(w, h) }).unwrap().with_filters(filters);
    let mut dec = Decoder::new();
    let (mut bytes, mut err) = (0, 0u64);
    for i in 0..n {
        let src = picture(w, h, i);
        let p = e.push_frame(&src, i as i64).unwrap().remove(0);
        bytes += p.data.len();
        let mut out = dec.decode_pts(&p.data, p.pts).unwrap();
        out.extend(dec.flush());
        assert_eq!(out[0].planes, e.reconstruction().unwrap().planes, "frame {i}");
        err += (0..3).map(|c| sse(&out[0].planes[c], &src.planes[c])).sum::<u64>();
    }
    (bytes, err)
}

const OFF: FilterSet = FilterSet { deblock: false, cdef: false, restoration: false };
const DEBLOCK: FilterSet = FilterSet { deblock: true, cdef: false, restoration: false };
const BOTH: FilterSet = FilterSet { deblock: true, cdef: true, restoration: false };
const ALL: FilterSet = FilterSet { deblock: true, cdef: true, restoration: true };

#[test]
fn deblocked_frames_decode_to_the_encoders_reconstruction_and_beat_unfiltered() {
    let off = run(OFF, 220, 6);
    let on = run(DEBLOCK, 220, 6);
    println!("deblock off {off:?} on {on:?}");
    assert!(on.1 < off.1, "deblocking did not reduce the error: {on:?} vs {off:?}");
}

#[test]
fn cdef_frames_decode_to_the_encoders_reconstruction_and_beat_deblocking_alone() {
    for q in [120u8, 200] {
        let deblock = run(DEBLOCK, q, 6);
        let both = run(BOTH, q, 6);
        println!("q {q}: deblock {deblock:?} deblock+cdef {both:?}");
        assert!(both.1 < deblock.1, "CDEF did not reduce the error at q {q}: {both:?} vs {deblock:?}");
    }
}

#[test]
fn restoration_frames_decode_to_the_encoders_reconstruction_and_beat_cdef_alone() {
    let mut gained = false;
    for q in [120u8, 200] {
        let both = run(BOTH, q, 6);
        let all = run(ALL, q, 6);
        println!("q {q}: deblock+cdef {both:?} +restoration {all:?}");
        // a unit only keeps a filter when it pays for its bits, so the error never gets worse
        assert!(all.1 <= both.1, "restoration made the error worse at q {q}: {all:?} vs {both:?}");
        gained |= all.1 < both.1;
    }
    assert!(gained, "restoration never ran");
}
