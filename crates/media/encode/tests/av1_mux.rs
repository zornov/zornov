// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mux AV1 temporal units into MP4 and read them back with the workspace
//! demuxer.
//!
//! `data/svtav1_160x96_15f.rs` holds a 15-frame libsvtav1 stream (made with a
//! dev-only ffmpeg) as a low-overhead OBU stream with temporal delimiters.

use studio_encode::mux::{av1c_from_sequence_header, Muxer, Options};
use std::path::PathBuf;

#[path = "data/svtav1_160x96_15f.rs"]
mod stream;
use stream::STREAM;
const FRAMES: usize = 15;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("studio-encode-av1-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn leb128(data: &[u8]) -> (usize, usize) {
    let mut v = 0usize;
    for i in 0..8 {
        v |= ((data[i] & 0x7f) as usize) << (i * 7);
        if data[i] & 0x80 == 0 {
            return (v, i + 1);
        }
    }
    panic!("leb128");
}

/// Splits an OBU stream into temporal units at the temporal delimiters.
fn temporal_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut pos = 0;
    while pos < stream.len() {
        let h = stream[pos];
        assert!(h & 2 != 0, "OBU without size field");
        let (size, n) = leb128(&stream[pos + 1 + (h & 4 != 0) as usize..]);
        if (h >> 3) & 0xf == 2 {
            starts.push(pos);
        }
        pos += 1 + (h & 4 != 0) as usize + n + size;
    }
    assert_eq!(pos, stream.len());
    starts.push(stream.len());
    starts.windows(2).map(|w| &stream[w[0]..w[1]]).collect()
}

/// (type, whole OBU) pairs.
fn obus(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let h = data[pos];
        let hdr = 1 + (h & 4 != 0) as usize;
        let (size, n) = leb128(&data[pos + hdr..]);
        out.push(((h >> 3) & 0xf, &data[pos..pos + hdr + n + size]));
        pos += hdr + n + size;
    }
    out
}

fn write(path: &std::path::Path, opts: Options, tus: &[&[u8]]) -> studio_encode::mux::SequenceInfo {
    let seq = obus(tus[0]).into_iter().find(|o| o.0 == 1).expect("sequence header in the first unit").1;
    let info = av1c_from_sequence_header(seq).unwrap();
    let mut mux = Muxer::create(path, opts).unwrap();
    let track = mux.add_av1_video(info.track_info(30, 1, 200_000)).unwrap();
    for tu in tus {
        mux.write_temporal_unit(track, tu).unwrap();
    }
    mux.finish().unwrap();
    info
}

fn check(path: &std::path::Path, opts: Options) {
    let tus = temporal_units(STREAM);
    assert_eq!(tus.len(), FRAMES);
    let info = write(path, opts, &tus);
    assert!(std::fs::metadata(path).unwrap().len() < STREAM.len() as u64 + 4096, "output size is bounded by the input");
    let mut d = studio_mp4::open(path).unwrap();
    let movie = d.movie().clone();
    assert_eq!(&movie.brand, b"isom");
    assert_eq!(movie.tracks.len(), 1);
    let v = movie.tracks[0].clone();
    assert_eq!(v.kind, studio_mp4::TrackKind::Video);
    assert_eq!(&v.fourcc, b"av01");
    assert_eq!((v.width, v.height), (160, 96));
    // The configuration record comes back byte for byte.
    let studio_mp4::Codec::Av1 { config } = &v.codec else { panic!("codec {:?}", v.codec) };
    assert_eq!(config.raw, info.av1c);
    assert_eq!((config.seq_profile, config.bit_depth(), config.monochrome), (0, 8, false));
    assert_eq!((config.chroma_subsampling_x, config.chroma_subsampling_y), (1, 1));
    assert_eq!(config.config_obus, info.av1c[4..]);
    assert_eq!(config.config_obus[0] >> 3 & 0xf, 1, "config OBUs start with the sequence header");
    // Samples: one per temporal unit, no temporal delimiters or padding,
    // everything else unchanged.
    assert_eq!(v.samples.len(), FRAMES);
    let mut syncs = Vec::new();
    for (i, (s, tu)) in v.samples.iter().zip(&tus).enumerate() {
        let got = d.read_sample(s).unwrap();
        let want: Vec<u8> = obus(tu).into_iter().filter(|o| !matches!(o.0, 2 | 15)).flat_map(|o| o.1.iter().copied()).collect();
        assert_eq!(got, want, "sample {i}");
        assert!(obus(&got).iter().all(|o| o.0 != 2 && o.0 != 15));
        // SVT-AV1 repeats the sequence header on every key frame, and only there.
        assert_eq!(s.sync, obus(&got).iter().any(|o| o.0 == 1), "sync flag of sample {i}");
        if s.sync {
            syncs.push(i);
        }
    }
    assert_eq!(syncs.first(), Some(&0));
    assert!(syncs.len() >= 2, "two GOPs, got sync samples {syncs:?}");
    assert!((v.duration_secs - FRAMES as f64 / 30.0).abs() < 0.002, "duration {}", v.duration_secs);
    assert_eq!(v.color, Some(studio_mp4::ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: false }));
}

#[test]
fn mp4_fast_start_round_trip() {
    let dir = tmp("fast");
    check(&dir.join("clip.mp4"), Options::mp4_av1());
}

#[test]
fn mp4_moov_last_round_trip() {
    let dir = tmp("tail");
    check(&dir.join("clip.mp4"), Options { fast_start: false, ..Options::mp4_av1() });
}

#[test]
fn av1_needs_the_av1_option() {
    let dir = tmp("opt");
    let tus = temporal_units(STREAM);
    let seq = obus(tus[0]).into_iter().find(|o| o.0 == 1).unwrap().1;
    let info = av1c_from_sequence_header(seq).unwrap();
    let mut mux = Muxer::create(dir.join("x.mp4"), Options::mp4()).unwrap();
    assert!(mux.add_av1_video(info.track_info(30, 1, 0)).is_err());
}

#[test]
fn malformed_temporal_unit_is_refused() {
    let dir = tmp("bad");
    let tus = temporal_units(STREAM);
    let seq = obus(tus[0]).into_iter().find(|o| o.0 == 1).unwrap().1;
    let info = av1c_from_sequence_header(seq).unwrap();
    let mut mux = Muxer::create(dir.join("x.mp4"), Options::mp4_av1()).unwrap();
    let t = mux.add_av1_video(info.track_info(30, 1, 0)).unwrap();
    assert!(mux.write_temporal_unit(t, &tus[0][..tus[0].len() - 3]).is_err());
}
