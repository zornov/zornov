// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Opens every reference file in `testdata/media` and checks the demuxed
//! metadata against `index.json`.

mod common;

use studio_mp4::{open, sample_for_time, split_nals, AudioSpecificConfig, Codec, Track, TrackKind};

fn check_common(track: &Track, file_len: u64) {
    assert!(!track.samples.is_empty());
    let total: u64 = track.samples.iter().map(|s| s.size as u64).sum();
    assert!(total <= file_len, "sample sizes exceed file size");
    for s in &track.samples {
        assert!(s.offset + s.size as u64 <= file_len);
        assert!(s.size > 0);
    }
    // Decode order: monotonically increasing dts.
    assert!(track.samples.windows(2).all(|w| w[0].dts < w[1].dts), "dts not increasing");
    // Presentation order: unique, increasing pts; presented times monotonic.
    let order = track.presentation_order();
    let pts: Vec<i64> = order.iter().map(|&i| track.samples[i].pts).collect();
    assert!(pts.windows(2).all(|w| w[0] < w[1]), "pts not unique/increasing");
    let pres: Vec<f64> =
        order.iter().filter_map(|&i| track.presentation_secs(&track.samples[i])).collect();
    assert!(pres.windows(2).all(|w| w[0] < w[1]));
    if track.kind == TrackKind::Video {
        assert!(pres.first().is_some_and(|&t| t.abs() < 1e-9), "first frame must show at 0");
    }
    assert!(track.samples[0].sync, "first sample must be a sync sample");
    assert!(!track.sync_samples().is_empty());
}

#[test]
fn every_reference_file_matches_index() {
    let entries = common::index();
    assert_eq!(entries.len(), 10);
    for e in &entries {
        let file = e.str("file").unwrap();
        let path = common::media_dir().join(file);
        let file_len = std::fs::metadata(&path).unwrap().len();
        let mut dmx = open(&path).unwrap_or_else(|err| panic!("{file}: {err}"));
        let movie = dmx.movie().clone();
        eprintln!(
            "{file}: brand {:?} dur {:.4}s tracks {}",
            String::from_utf8_lossy(&movie.brand),
            movie.duration_secs,
            movie.tracks.len()
        );
        assert!(!movie.fragmented);
        let expect_brand: &[u8; 4] = if file.ends_with(".mov") {
            b"qt  "
        } else if file.ends_with(".m4a") {
            b"M4A "
        } else {
            b"mp42"
        };
        assert_eq!(&movie.brand, expect_brand, "{file}");

        if let Some(v) = e.get("video") {
            let t = movie.video().unwrap_or_else(|| panic!("{file}: no video track"));
            assert_eq!(t.kind, TrackKind::Video);
            let frames = v.num("frames").unwrap() as usize;
            let fps = v.num("fps").unwrap();
            assert_eq!(t.width, v.num("width").unwrap() as u32, "{file} width");
            assert_eq!(t.height, v.num("height").unwrap() as u32, "{file} height");
            assert_eq!(t.samples.len(), frames, "{file} frames");
            assert_eq!(t.presented_samples(), frames, "{file} presented frames");
            assert_eq!(t.rotation, 0);
            assert!(!t.flip);
            assert!((t.duration_secs - frames as f64 / fps).abs() < 1e-6, "{file} duration");
            check_common(t, file_len);
            eprintln!(
                "  video {}x{} ts {} edit_offset {} syncs {:?} fourcc {}",
                t.width,
                t.height,
                t.timescale,
                t.edit_offset,
                t.sync_samples(),
                String::from_utf8_lossy(&t.fourcc)
            );

            let (len_size, idr_types): (u8, &[u8]) = match &t.codec {
                Codec::Avc { config } => {
                    assert!(file.starts_with("h264_"));
                    let want = if file.contains("baseline") {
                        66
                    } else if file.contains("main") {
                        77
                    } else {
                        100
                    };
                    assert_eq!(config.profile_idc, want, "{file} profile");
                    assert_eq!(config.sps.len(), 1);
                    assert_eq!(config.pps.len(), 1);
                    assert_eq!(config.sps[0][0] & 0x1f, 7);
                    assert_eq!(config.sps[0][1], want);
                    assert_eq!(config.pps[0][0] & 0x1f, 8);
                    (config.nal_length_size, &[5])
                }
                Codec::Hevc { config } => {
                    assert!(file.starts_with("hevc_"));
                    let want = if file.contains("main10") { 2 } else { 1 };
                    assert_eq!(config.general_profile_idc, want, "{file} hevc profile");
                    assert_eq!(config.bit_depth_luma, if want == 2 { 10 } else { 8 });
                    assert_eq!(config.vps().len(), 1);
                    assert_eq!(config.sps().len(), 1);
                    assert_eq!(config.pps().len(), 1);
                    assert_eq!((config.sps()[0][0] >> 1) & 0x3f, 33);
                    (config.nal_length_size, &[16, 17, 18, 19, 20, 21])
                }
                other => panic!("{file}: unexpected codec {other:?}"),
            };
            assert_eq!(t.codec.nal_length_size(), Some(len_size));
            for (i, s) in t.samples.clone().iter().enumerate() {
                let data = dmx.read_sample(s).unwrap();
                let nals = split_nals(&data, len_size).unwrap();
                assert!(!nals.is_empty());
                let types: Vec<u8> = nals
                    .iter()
                    .map(|n| if idr_types[0] == 5 { n[0] & 0x1f } else { (n[0] >> 1) & 0x3f })
                    .collect();
                let has_irap = types.iter().any(|ty| idr_types.contains(ty));
                if i == 0 {
                    assert!(has_irap, "{file}: first sample has no IDR/IRAP ({types:?})");
                }
                if has_irap {
                    assert!(s.sync, "{file}: IRAP sample {i} not marked sync");
                }
            }

            // Seeking: every frame time maps to itself and to a sync sample at or before it.
            for k in 0..frames {
                let secs = k as f64 / fps;
                let sp = sample_for_time(t, secs).unwrap();
                let target = &t.samples[sp.target_index];
                let shown = t.presentation_secs(target).unwrap();
                assert!((shown - secs).abs() < 1e-6, "{file}: seek {secs} -> {shown}");
                let sync = &t.samples[sp.sync_index];
                assert!(sync.sync && sp.sync_index <= sp.target_index && sync.pts <= target.pts);
            }
        }

        if let Some(a) = e.get("audio") {
            let t = movie.audio().unwrap_or_else(|| panic!("{file}: no audio track"));
            assert_eq!(t.kind, TrackKind::Audio);
            check_common(t, file_len);
            let rate = a.num("rate").unwrap() as u32;
            let frames = a.num("frames").unwrap();
            let Codec::Aac { asc, object_type, sample_rate, channels } = &t.codec else {
                panic!("{file}: audio is not AAC: {:?}", t.codec)
            };
            let parsed = AudioSpecificConfig::parse(asc).unwrap();
            eprintln!(
                "  audio aot {object_type} {sample_rate} Hz {channels} ch ts {} edit_offset {} dur {:.4} {parsed:?}",
                t.timescale, t.edit_offset, t.duration_secs
            );
            let want_aot = if file.contains("he_v2") {
                29
            } else if file.contains("he_v1") {
                5
            } else {
                2
            };
            assert_eq!(*object_type, want_aot, "{file} object type");
            assert_eq!(parsed.core_object_type, 2);
            assert_eq!(*sample_rate, rate, "{file} sample rate");
            assert_eq!(*channels as f64, a.num("channels").unwrap(), "{file} channels");
            assert!((t.duration_secs - frames / rate as f64).abs() < 1e-6, "{file} audio duration");
            assert!(t.edit_offset > 0, "{file}: AAC priming must be trimmed");
            assert!(t.bitrate.is_some_and(|b| b.avg > 0));
            for s in t.samples.clone() {
                assert_eq!(dmx.read_sample(&s).unwrap().len(), s.size as usize);
            }
        }

        let longest = movie.tracks.iter().map(|t| t.duration_secs).fold(0.0, f64::max);
        assert!((movie.duration_secs - longest).abs() < 1e-9);
    }
}

#[test]
fn bframe_files_reorder_presentation() {
    let dmx = open(common::media_dir().join("h264_main_cabac_bframes.mp4")).unwrap();
    let t = dmx.movie().video().unwrap();
    let order = t.presentation_order();
    assert_ne!(order, (0..t.samples.len()).collect::<Vec<_>>(), "B-frames reorder");
    assert!(t.samples.iter().any(|s| s.pts != s.dts));
    assert_eq!(t.sdtp.len(), t.samples.len());
    // The edit list skips the composition delay so the first frame shows at 0.
    let first = &t.samples[order[0]];
    assert_eq!(first.pts, t.edit_offset);
}

#[test]
fn quicktime_files() {
    for name in ["h264_high_odd_size.mov", "hevc_main10.mov"] {
        let dmx = open(common::media_dir().join(name)).unwrap();
        let m = dmx.movie();
        assert_eq!(&m.brand, b"qt  ");
        let t = m.video().unwrap();
        assert_eq!(&t.handler, b"vide");
        assert_eq!(t.language, "und");
        // QuickTime writes negative composition offsets into version-0 ctts.
        assert!(t.samples.iter().any(|s| s.pts < s.dts));
    }
}
