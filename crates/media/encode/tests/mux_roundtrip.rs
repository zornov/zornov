// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mux H.264 + AAC into MP4 / MOV and read the files back with the
//! workspace's hand-written demuxer and decoders.

use studio_encode::mux::{Brand, Muxer, Options};
use studio_encode::yuv::Yuv420;
use studio_encode::{aac, h264};
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("studio-encode-mux-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Written {
    video: Vec<(Vec<u8>, bool)>,
    audio: Vec<Vec<u8>>,
    avcc: Vec<u8>,
    asc: Vec<u8>,
}

fn write(path: &std::path::Path, opts: Options, frames: u32, fps: (u32, u32)) -> Written {
    let (w, h) = (160, 96);
    let mut venc = h264::Encoder::new(h264::Config { gop: 10, max_slices: 1, ..h264::Config::new(w, h, fps.0, fps.1) }).unwrap();
    let mut aenc = aac::Encoder::new(aac::Config { sample_rate: 48_000, channels: 2, bitrate: 128_000 }).unwrap();
    let mut mux = Muxer::create(path, opts).unwrap();
    let vt = mux.add_video(venc.track_info()).unwrap();
    let at = mux.add_audio(aenc.track_info()).unwrap();
    let mut out = Written { video: Vec::new(), audio: Vec::new(), avcc: venc.avcc(), asc: aenc.audio_specific_config() };
    let fps_f = fps.0 as f64 / fps.1 as f64;
    let mut samples = 0u64;
    for i in 0..frames {
        let mut pic = Yuv420::filled(w, h, 60, 128, 128);
        for (k, p) in pic.y.iter_mut().enumerate() {
            *p = ((k as u32 % w + i * 3) % 200 + 16) as u8;
        }
        let f = venc.encode(&pic);
        mux.write_sample(vt, &f.data, f.keyframe).unwrap();
        out.video.push((f.data, f.keyframe));
        let end = ((i + 1) as f64 * 48_000.0 / fps_f).round() as u64;
        let n = (end - samples) as usize;
        let pcm: Vec<f32> = (0..n * 2).map(|k| (((samples as usize + k / 2) as f32) * 0.05).sin() * 0.3).collect();
        samples = end;
        for au in aenc.push(&pcm) {
            mux.write_sample(at, &au, true).unwrap();
            out.audio.push(au);
        }
    }
    for au in aenc.flush() {
        mux.write_sample(at, &au, true).unwrap();
        out.audio.push(au);
    }
    mux.set_audio_samples(at, samples);
    mux.finish().unwrap();
    out
}

fn top_level_boxes(path: &std::path::Path) -> Vec<String> {
    let data = std::fs::read(path).unwrap();
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 8 <= data.len() {
        let mut size = u32::from_be_bytes(data[p..p + 4].try_into().unwrap()) as u64;
        let kind = String::from_utf8_lossy(&data[p + 4..p + 8]).to_string();
        if size == 1 {
            size = u64::from_be_bytes(data[p + 8..p + 16].try_into().unwrap());
        }
        out.push(kind);
        p += size as usize;
    }
    assert_eq!(p, data.len(), "boxes cover the file exactly");
    out
}

fn check(path: &std::path::Path, opts: Options, fps: (u32, u32)) {
    let frames = 45;
    let written = write(path, opts, frames, fps);
    let boxes = top_level_boxes(path);
    if opts.fast_start {
        assert_eq!(boxes, ["ftyp", "moov", "mdat"]);
    } else {
        assert_eq!(boxes, ["ftyp", "mdat", "moov"]);
    }
    let mut d = studio_mp4::open(path).unwrap();
    let movie = d.movie().clone();
    assert_eq!(movie.tracks.len(), 2);
    let v = movie.tracks.iter().find(|t| t.kind == studio_mp4::TrackKind::Video).unwrap().clone();
    let a = movie.tracks.iter().find(|t| t.kind == studio_mp4::TrackKind::Audio).unwrap().clone();
    // Video: config, sample count, sync samples, timing, colour.
    match &v.codec {
        studio_mp4::Codec::Avc { config } => {
            assert_eq!(config.nal_length_size, 4);
            assert_eq!(config.sps.len(), 1);
        }
        c => panic!("video codec {c:?}"),
    }
    assert_eq!((v.width, v.height), (160, 96));
    assert_eq!(v.samples.len(), frames as usize);
    let syncs: Vec<usize> = v.samples.iter().enumerate().filter(|(_, s)| s.sync).map(|(i, _)| i).collect();
    assert_eq!(syncs, vec![0, 10, 20, 30, 40]);
    let fps_f = fps.0 as f64 / fps.1 as f64;
    assert!((v.duration_secs - frames as f64 / fps_f).abs() < 0.002, "video {}", v.duration_secs);
    assert_eq!(v.color, Some(studio_mp4::ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: false }));
    // Audio: AAC config, AU count, priming hidden by the edit list.
    match &a.codec {
        studio_mp4::Codec::Aac { asc, object_type, sample_rate, channels } => {
            assert_eq!(asc, &written.asc);
            assert_eq!((*object_type, *sample_rate, *channels), (2, 48_000, 2));
        }
        c => panic!("audio codec {c:?}"),
    }
    assert_eq!(a.samples.len(), written.audio.len());
    assert_eq!(a.edit_offset, 1024);
    assert!((a.duration_secs - frames as f64 / fps_f).abs() < 0.002, "audio {}", a.duration_secs);
    // Sample bytes come back unchanged and decode.
    let mut dec = studio_h264::Decoder::from_avcc_bytes(&written.avcc).unwrap();
    let mut decoded = 0;
    for (s, (data, key)) in v.samples.iter().zip(&written.video) {
        let got = d.read_sample(s).unwrap();
        assert_eq!(&got, data);
        assert_eq!(s.sync, *key);
        decoded += dec.decode(&got).unwrap().len();
    }
    assert_eq!(decoded + dec.flush().len(), frames as usize);
    let mut adec = studio_aac::Decoder::new(&written.asc).unwrap();
    for (s, au) in a.samples.iter().zip(&written.audio) {
        let got = d.read_sample(s).unwrap();
        assert_eq!(&got, au);
        adec.decode_frame(&got).unwrap();
    }
}

#[test]
fn mp4_fast_start_round_trip() {
    let dir = tmp("mp4");
    let path = dir.join("clip.mp4");
    check(&path, Options::mp4(), (30, 1));
    assert!(!dir.join("clip.mp4.mdat.part").exists());
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[4..12], b"ftypisom");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mov_moov_at_end_round_trip() {
    let dir = tmp("mov");
    let path = dir.join("clip.mov");
    check(&path, Options { brand: Brand::Mov, fast_start: false, chunk_secs: 0.25 }, (30000, 1001));
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[4..12], b"ftypqt  ");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn abandoned_files_are_removed() {
    let dir = tmp("abandon");
    let path = dir.join("x.mp4");
    let mut m = Muxer::create(&path, Options::mp4()).unwrap();
    let enc = h264::Encoder::new(h264::Config::new(64, 64, 25, 1)).unwrap();
    let t = m.add_video(enc.track_info()).unwrap();
    m.write_sample(t, &[0, 0, 0, 1, 0x65], true).unwrap();
    m.abandon();
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn languages_alternate_audio_and_subtitles() {
    let d = tmp("langs");
    let path = d.join("dub.mp4");
    let mut mux = Muxer::create(&path, Options::mp4()).unwrap();
    let mut tracks = Vec::new();
    for (lang, title) in [("eng", "English"), ("hin", "Hindi (dub)")] {
        let enc = aac::Encoder::new(aac::Config { sample_rate: 48_000, channels: 2, bitrate: 128_000 }).unwrap();
        let t = mux.add_audio(enc.track_info()).unwrap();
        mux.set_language(t, lang);
        mux.set_title(t, title);
        tracks.push((t, enc));
    }
    let st = mux.add_subtitles(studio_encode::mux::SubtitleInfo { width: 1920, height: 1080, font_size: 48 }).unwrap();
    mux.set_language(st, "spa");
    mux.set_title(st, "Español");
    for (t, enc) in &mut tracks {
        let pcm = vec![0.1f32; 48_000 * 2];
        let mut units = enc.push(&pcm);
        units.extend(enc.flush());
        for u in &units {
            mux.write_sample(*t, u, true).unwrap();
        }
        mux.set_audio_samples(*t, 48_000);
    }
    mux.write_subtitle(st, 200, 900, "¡Hola!").unwrap();
    mux.write_subtitle(st, 900, 1000, "Adiós").unwrap();
    mux.finish().unwrap();
    let movie = studio_mp4::demux::open(&path).unwrap().into_movie();
    let langs: Vec<(&str, String)> = movie.tracks.iter().map(|t| (std::str::from_utf8(&t.handler).unwrap_or("?"), t.language.clone())).collect();
    assert_eq!(langs.len(), 3, "{langs:?}");
    assert_eq!(langs[0].1, "eng");
    assert_eq!(langs[1].1, "hin");
    assert_eq!(langs[2], ("sbtl", "spa".to_owned()));
    let sub = &movie.tracks[2];
    // A gap sample, then the two cues: 200 ms empty, 700 ms, 100 ms.
    assert_eq!(sub.samples.len(), 3);
    let mut dm = studio_mp4::demux::open(&path).unwrap();
    let s1 = dm.read_sample(&sub.samples[1]).unwrap();
    assert_eq!(u16::from_be_bytes([s1[0], s1[1]]) as usize, "¡Hola!".len());
    assert_eq!(std::str::from_utf8(&s1[2..]).unwrap(), "¡Hola!");
    assert!((sub.duration_secs - 1.0).abs() < 1e-6, "{}", sub.duration_secs);
    let _ = std::fs::remove_dir_all(&d);
}
