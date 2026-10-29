// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M9: in-loop filters. The encoder's reconstruction must equal the decoder's output with each
//! filter on, and each filter must pay for itself.

use studio_av1::Decoder;
use studio_av1_enc::color::from_rgba8;
use studio_av1_enc::filters::FilterSet;
use studio_av1_enc::{Config, Encoder, Frame, RateMode};

fn picture(w: u32, h: u32, shift: u32) -> Frame {
    // smooth gradients with a moving edge: blocking shows at coarse quantisers
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let edge = if (x + shift * 3) % 40 < 20 { 60 } else { 0 };
            [((x * 2 + y + shift * 2) % 256) as u8, (y * 3 % 200 + edge) as u8, (x * y / 20 % 256) as u8, 255]
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

#[test]
fn deblocked_frames_decode_to_the_encoders_reconstruction_and_beat_unfiltered() {
    let off = run(FilterSet { deblock: false }, 220, 6);
    let on = run(FilterSet { deblock: true }, 220, 6);
    println!("deblock off {off:?} on {on:?}");
    assert!(on.1 < off.1, "deblocking did not reduce the error: {on:?} vs {off:?}");
}
