// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M12: tests for the public `Encoder`: configuration checks, packet shape, the `av1C` record,
//! and that the packets decode with `studio-av1` to the encoder's own reconstruction.

use studio_av1::{Decoder, SequenceHeader};
use studio_av1_enc::color::from_rgba8;
use studio_av1_enc::{Config, Encoder, Error, Frame, RateMode};

fn cfg(w: u32, h: u32, q: u8) -> Config {
    Config { rate: RateMode::Cqp(q), ..Config::new(w, h) }
}

fn picture(w: u32, h: u32, shift: u32) -> Frame {
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| [((i % w) * 3 + shift) as u8, ((i / w) * 5) as u8, (shift * 40) as u8, 255]).collect();
    from_rgba8(&rgba, w, h, 8, false)
}

/// `(type, whole OBU)` pairs of a low-overhead OBU stream.
fn obus(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let h = data[pos];
        assert!(h & 2 != 0, "OBU without a size field");
        let hdr = 1 + (h & 4 != 0) as usize;
        let (mut size, mut n) = (0usize, 0);
        for i in 0..8 {
            let b = data[pos + hdr + i];
            size |= ((b & 0x7f) as usize) << (7 * i);
            n += 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        out.push(((h >> 3) & 0xf, &data[pos..pos + hdr + n + size]));
        pos += hdr + n + size;
    }
    out
}

#[test]
fn bad_configurations_are_refused() {
    assert_eq!(Encoder::new(cfg(0, 64, 100)).err(), Some(Error::Invalid("zero size")));
    assert!(matches!(Encoder::new(Config { bit_depth: 12, ..cfg(64, 64, 100) }), Err(Error::Invalid(_))));
    assert!(matches!(Encoder::new(Config { fps: (0, 1), ..cfg(64, 64, 100) }), Err(Error::Invalid(_))));
    // not built yet
    assert!(matches!(Encoder::new(Config { bit_depth: 10, ..cfg(64, 64, 100) }), Err(Error::Unsupported(_))));
    assert!(matches!(Encoder::new(cfg(64, 64, 0)), Err(Error::Unsupported(_))));
    assert!(matches!(Encoder::new(Config { rate: RateMode::Crf(64), ..cfg(64, 64, 0) }), Err(Error::Invalid(_))));
    assert!(matches!(Encoder::new(Config { rate: RateMode::Bitrate(0), ..cfg(64, 64, 0) }), Err(Error::Invalid(_))));
    assert!(Encoder::new(Config { rate: RateMode::Crf(30), ..cfg(64, 64, 0) }).is_ok());
    assert!(Encoder::new(Config { rate: RateMode::Bitrate(2000), ..cfg(64, 64, 0) }).is_ok());
    assert!(matches!(Encoder::new(Config { tiles: (1, 0), ..cfg(64, 64, 100) }), Err(Error::Unsupported(_))));
}

#[test]
fn frames_must_match_the_configuration() {
    let mut e = Encoder::new(cfg(64, 48, 100)).unwrap();
    assert!(matches!(e.push_frame(&picture(32, 48, 0), 0), Err(Error::Invalid(_))));
    let mut wrong_depth = picture(64, 48, 0);
    wrong_depth.bit_depth = 10;
    assert!(matches!(e.push_frame(&wrong_depth, 0), Err(Error::Invalid(_))));
    let mut ragged = picture(64, 48, 0);
    ragged.planes[1].pop();
    assert!(matches!(e.push_frame(&ragged, 0), Err(Error::Invalid(_))));
    assert!(e.reconstruction().is_none());
}

#[test]
fn av1c_matches_the_sequence_header() {
    let e = Encoder::new(cfg(130, 70, 100)).unwrap();
    let c = e.av1c();
    assert_eq!(c[0], 0x81, "marker and version");
    assert_eq!(c[2] & 0x4c, 0x0c, "8-bit, 4:2:0");
    let o = obus(&c[4..]);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].0, 1, "configOBUs is one sequence header");
    let payload = &o[0].1[2..];
    let s = SequenceHeader::parse(payload).unwrap();
    assert_eq!((s.max_frame_width, s.max_frame_height), (130, 70));
}

#[test]
fn key_frames_hold_a_sequence_header_and_a_frame_and_decode_to_the_reconstruction() {
    for speed in [6u8, 9] {
        // keyint 1: every frame is a key frame
        let mut e = Encoder::new(Config { speed, keyint: 1, ..cfg(130, 70, 90) }).unwrap();
        let mut dec = Decoder::new();
        for i in 0..3u32 {
            let src = picture(130, 70, i);
            let pkts = e.push_frame(&src, 1000 + i as i64).unwrap();
            assert_eq!(pkts.len(), 1);
            let p = &pkts[0];
            assert!(p.keyframe);
            assert_eq!(p.pts, 1000 + i as i64);
            let types: Vec<u8> = obus(&p.data).iter().map(|o| o.0).collect();
            assert_eq!(types, [1, 6], "a sequence header, then the frame; no temporal delimiter");
            let mut pics = dec.decode_pts(&p.data, p.pts).unwrap();
            pics.extend(dec.flush());
            assert_eq!(pics.len(), 1);
            let want = e.reconstruction().unwrap();
            assert_eq!(pics[0].pts, p.pts);
            assert_eq!(pics[0].planes, want.planes, "speed {speed} frame {i}");
        }
        assert!(e.flush().unwrap().is_empty());
    }
}

#[test]
fn inter_frames_follow_the_key_frame_every_keyint_frames() {
    for speed in [3u8, 6, 9] {
        let mut e = Encoder::new(Config { speed, keyint: 4, ..cfg(130, 70, 90) }).unwrap();
        let mut dec = Decoder::new();
        for i in 0..9u32 {
            // a pan: each frame is the picture shifted by a few samples
            let src = picture(130, 70, 3 * i);
            let p = e.push_frame(&src, i as i64).unwrap().remove(0);
            let key = i % 4 == 0;
            assert_eq!(p.keyframe, key, "frame {i}");
            let types: Vec<u8> = obus(&p.data).iter().map(|o| o.0).collect();
            assert_eq!(types, if key { vec![1, 6] } else { vec![6] }, "frame {i}: only key frames carry the sequence header");
            let mut pics = dec.decode_pts(&p.data, p.pts).unwrap();
            pics.extend(dec.flush());
            assert_eq!(pics.len(), 1, "frame {i}");
            assert_eq!(pics[0].planes, e.reconstruction().unwrap().planes, "speed {speed} frame {i}");
        }
    }
}

#[test]
fn a_higher_quantiser_makes_smaller_packets() {
    let src = picture(128, 96, 1);
    let size = |q: u8| Encoder::new(cfg(128, 96, q)).unwrap().push_frame(&src, 0).unwrap()[0].data.len();
    assert!(size(200) < size(100) && size(100) < size(30), "{} {} {}", size(30), size(100), size(200));
}

#[test]
fn levels_follow_the_picture_size() {
    // seq_level_idx is the 5 bits after the operating point in the sequence header
    let level = |w, h| {
        let c = Encoder::new(cfg(w, h, 100)).unwrap().av1c();
        c[1] & 0x1f
    };
    assert_eq!(level(64, 64), 0);
    assert_eq!(level(640, 360), 1); // 2.1
    assert_eq!(level(1920, 1080), 8); // 4.0
    assert_eq!(level(3840, 2160), 12); // 5.0
}
