// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 1080p performance + bit-exactness regression (ignored by default).
//!
//! `cargo test --release -p studio-hevc --test perf_1080p -- --ignored --nocapture`
//! Set `HEVC_PERF_LOOPS=n` to decode the file n times (for profiling).

mod common;

use common::mp4mini::read_hevc_track;
use studio_hevc::{Decoder, Frame, PlaneData};
use std::path::PathBuf;
use std::time::Instant;

fn fnv1a(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

fn frame_hash(f: &Frame) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in [&f.y, &f.u, &f.v] {
        match p {
            PlaneData::U8(v) => fnv1a(&mut h, v),
            PlaneData::U16(v) => {
                for s in v {
                    fnv1a(&mut h, &s.to_le_bytes());
                }
            }
        }
    }
    fnv1a(&mut h, &f.poc.to_le_bytes());
    h
}

/// Per-picture hashes (output order) recorded before any optimisation.
const BASELINE: &[u64] = &[
    0x2adf9e649ac4c519, 0x2b21b012db9f6b8b, 0xcfb4a8b6d192ad99, 0xe5c2ec69b9148bc5,
    0xa6a6ded3d6dedff2, 0x622645a6151f1840, 0xe11f1838a521d86f, 0x99633bcd18d085d9,
    0x093afb602739f699, 0xf14a8a56657691dc, 0x4ed1a10de16402a5, 0xc61406e0a41b61d7,
    0xe63e83b4e7a62157, 0x88e8a47f7e02fefb, 0x5159887eadb895ad, 0xd41b84ce750a3cfc,
    0xa2ef8606693b5cc0, 0x73b2b994211e355b, 0xd39e8cb8bf60875e, 0x29b0b3f08d00222c,
    0x5668d588b711416b, 0x9bb81110bad228b9, 0x45d4ab09c2fd1991, 0x333f54822055e3f3,
    0x66bbfe570b7398ee, 0x361274e7cfd2fadb, 0x71202e971edf40ac, 0x9e54e1143710127a,
    0x95c6597dfd3d273f, 0x9f5c0322e083d484, 0x084439c8f8c424d8, 0xa581b89a822c6e99,
    0x23d055e94c672c06, 0xeb0acbb69eace0be, 0xc88f921c03f09fc8, 0x73e34a7d5faa0ca3,
    0x73aba50ebe77d6da, 0xf17eabca271e197a, 0xa09a5ea90da08da3, 0xf5f2777c46ac2429,
    0x2ffb843466096595, 0x5a5d9dea3c109bbe, 0xfa3fb5db19c4a0dc, 0x9e17db0326333246,
    0xc4e0cea58a7aa1e8, 0x34bf74514fee6f44, 0xc179518b9434a027, 0x130879e4f748cb79,
    0x7fc5becb5bddce4b, 0x55ac74b03d5fcbb8, 0xb89b2ba12df8b1ee, 0xaea9e54f27e6e733,
    0xdb90511da0a5d4f7, 0xf900a32e4e4d2f17, 0xfc20184adcc9c1f2, 0xe0ab174a8a1aa956,
    0x5b077a1f0bce48ec, 0x62a55134a7026682, 0x253cf0963aa3b009, 0x3ec3b7aaaac504d9,
    0x95c34e8f1e1674e6, 0x4a0c2a1f4105e250, 0x5f02a4e5b01ac7d0, 0xefddbed6cfcfb2c2,
    0x3e6061db397d1bc9, 0x3b3299f4fad95284, 0xc81ae3e871594a32, 0x503b77f2f474336e,
    0xcd7b814022475e8d, 0xb5907ff7f6e5596e, 0xe49fd6bf93af9d1c, 0xf99dff5f601e3eaa,
    0x4fe46d9539db26dd, 0xe2ea0e95cad06424, 0xde1bd127c42d85a2, 0xb7554a7e89878060,
    0x3b8c71301290dbf6, 0x79a7e8a0bc92b1a4, 0x8ad9e0e03be052dc, 0x6a99d024d3c8b665,
    0x3e96432c833849df, 0xbfb58dece0e37d56, 0x822eed27b4a7c4eb, 0x2e8b88cb03207f31,
    0x2bd6de31328675af, 0xbd467a52396572b6, 0x33b6d2256baf44f8, 0x66218cd8120884f8,
    0xc632397849d01e87, 0xf7cafd2891f86217, 0x58bbdf221bf9ff47, 0xa01657fa66a95eb8,
    0xffd0d2a007ab24de, 0x788d92a6d8d621b9, 0x17b16111e280e58d, 0x04492bd959c06589,
    0x5d25f4e938fa4cd0, 0x21bdf8a16177cdcb, 0xecbb715266b7a11f, 0x56ca788d54dfd3aa,
    0x7b9f09f1cdb7f247, 0x9448ba3febfddaf7, 0x39195465e7a3c2ca, 0x114c310a51ac6fca,
    0x105de9a872fca021, 0x9152ceda948de17e, 0x0a031fef2ec1b448, 0x78cf74d895d93db5,
    0x26928d945668b8d3, 0xa729040165b42d97, 0x481ad5cdd157f940, 0x7f75e1aaa8134651,
    0x6d393493d8f503f8, 0x97780f8b073cbd1e, 0x322d1cf9c6a0fd5c, 0x64577e920306b2df,
    0xf5975beb9b422df3, 0x499aaf057e0efc88, 0xc267edd0ee31e181, 0x9c2be227afa8cd47,
    0x4da356399d8d0593, 0xb453f20f6c588eb2, 0x59cd8d0ea7d11049, 0x3d623a67aa82f2b1,
    0xfb3d3c258d2d454e, 0xba3abbc392b9d513, 0xc2783b56ebb94a87, 0x1b48c9277943e185,
    0x0dbe763cab91e797, 0x628e786f3de173cb, 0xdac43814a9f88ffe, 0x0cc2fc02354ef39f,
    0x8e9b6a867a3be7c0, 0x4780a4159a0e26f3, 0xe226b338467b7c7e, 0xa4b5d3e988d93c4b,
    0xe31e3de05a223b05, 0xf8c4a3bb97e0c7e8, 0x47970d4c99847e96, 0x1f78f9883953c583,
    0x31f9e90eea4a2c0b, 0xa88b6dca11a42ff5, 0xb5a8b1291545a38f, 0xc8ba93ce2b88aa2d,
    0x1f1e3bd3bfc95756, 0x733b4edf23154b8c, 0xc57c4a8851ff2b15, 0x5a124161c6dc8413,
    0xaf8c49ba59d275a4, 0x4ba19b9c7e9ef1c7, 0x917b7fdc9672cbcf, 0xfcde0e58383f1e98,
    0x9ce98ec94f893a2c, 0x66ba65123455ff53, 0x5bc6bde8f63192b2, 0x8e2157ab33f455af,
    0x9d541e1266544eaf, 0xa54f2221b26ee05e, 0xd78feb5f32f624d8, 0x46727071e802c0ed,
    0xc3ed3f6216fff1de, 0x4266122dd08f6cff, 0x1c0193d546f95abf, 0x79e185b5a98f55cf,
    0x9cb7ea487324b830, 0xe3c8f3bd76db5d97, 0x2df38e7d1d57bb7e, 0xb4e6cc9251cc1804,
    0x881a161b8769ce0f, 0x9e4271cfe40aba39, 0x41343c5b6d57e1d1, 0x1018d002fe157e65,
    0xd5cdf669fd9679c3, 0x16706fef7589b32b, 0xb4a1b3a42a837c8c, 0xf9c196e10d041543,
    0xb254d69c071f085b, 0x22b13a03795e3fc1, 0x6bb66ba79b780397, 0xd6a5fefc2a30ae41,
];

#[test]
#[ignore]
fn hevc_1080p30_perf_and_hash() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media/perf/hevc_1080p30.mp4");
    let data = std::fs::read(&path).expect("perf fixture");
    let track = read_hevc_track(&data).expect("hevc track");
    let loops: usize = std::env::var("HEVC_PERF_LOOPS").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut best = f64::MAX;
    for _ in 0..loops {
        // Only decoding is timed (hashing is excluded).
        let mut secs = 0.0f64;
        let mut dec = Decoder::from_hvcc(&track.hvcc).expect("decoder");
        let mut hashes = Vec::new();
        for (i, s) in track.samples.iter().enumerate() {
            let t0 = Instant::now();
            let frames = dec.decode(s).unwrap_or_else(|e| panic!("sample {i}: {e}"));
            secs += t0.elapsed().as_secs_f64();
            hashes.extend(frames.iter().map(frame_hash));
        }
        let t0 = Instant::now();
        let frames = dec.flush();
        secs += t0.elapsed().as_secs_f64();
        hashes.extend(frames.iter().map(frame_hash));
        let ms = secs * 1000.0 / hashes.len().max(1) as f64;
        best = best.min(ms);
        eprintln!("hevc_1080p30: {} pictures in {:.3}s = {:.2} ms/picture", hashes.len(), secs, ms);
        if BASELINE.is_empty() {
            let list: Vec<String> = hashes.iter().map(|h| format!("0x{h:016x}")).collect();
            eprintln!("BASELINE = [{}]", list.join(", "));
        } else {
            assert_eq!(hashes.len(), BASELINE.len(), "picture count");
            for (i, (a, b)) in hashes.iter().zip(BASELINE).enumerate() {
                assert_eq!(a, b, "picture {i} hash mismatch");
            }
        }
    }
    eprintln!("best: {best:.2} ms/picture");
}
