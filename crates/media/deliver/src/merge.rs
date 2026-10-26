// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Joins video-only MP4 pieces into one file without re-encoding: every
//! compressed picture is copied as it is, so the join is lossless and
//! costs seconds. Sound (mixed once, for the whole programme) is encoded
//! as AAC here; captions go in as timed-text tracks.
//!
//! Every piece must have been written by the same encoder with the same
//! settings (same parameter sets, each starting on a keyframe); that is
//! checked and reported otherwise.

use studio_encode::aac;
use studio_encode::h264::{ColorDesc, TrackInfo};
use studio_encode::mux::{Muxer, Options, SubtitleInfo};
use std::path::{Path, PathBuf};

/// One sound track of the finished file.
pub struct AudioTrackIn {
    /// Interleaved float samples for the whole programme.
    pub pcm: Vec<f32>,
    pub channels: usize,
    pub rate: u32,
    pub bitrate: u32,
    /// ISO 639-2/T ("eng", "hin").
    pub lang: String,
    pub title: String,
}

/// One caption or subtitle track.
pub struct SubtitleTrackIn {
    pub lang: String,
    pub title: String,
    /// (start ms, end ms, text), times from the start of the programme.
    pub cues: Vec<(u64, u64, String)>,
}

#[derive(Default)]
pub struct MergeOptions {
    /// Sound tracks; the first plays by default.
    pub audio: Vec<AudioTrackIn>,
    pub subtitles: Vec<SubtitleTrackIn>,
}

#[derive(Debug, PartialEq)]
pub struct MergeStats {
    pub frames: u64,
    pub pieces: usize,
    pub width: u32,
    pub height: u32,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Write `out` from `parts`, in order.
pub fn merge(parts: &[PathBuf], out: &Path, opts: &MergeOptions) -> Result<MergeStats, String> {
    let first = parts.first().ok_or("nothing to merge")?;
    let io = |e: std::io::Error| e.to_string();
    let mp4 = |e: studio_mp4::Error| e.to_string();
    let demux = studio_mp4::open(first).map_err(mp4)?;
    let track = demux.movie().video().ok_or("the first piece has no video")?.clone();
    let (config, hevc) = match &track.codec {
        studio_mp4::Codec::Avc { config } => (config.raw.clone(), false),
        studio_mp4::Codec::Hevc { config } => (config.raw.clone(), true),
        _ => return Err("pieces must be H.264 or HEVC".into()),
    };
    let dur = track.samples.first().map(|s| s.duration as u64).filter(|d| *d > 0).ok_or("the first piece is empty")?;
    let g = gcd(track.timescale as u64, dur).max(1);
    let (fps_num, fps_den) = ((track.timescale as u64 / g) as u32, (dur / g) as u32);
    let bytes: u64 = track.samples.iter().map(|s| s.size as u64).sum();
    let secs = (track.samples.len() as u64 * dur) as f64 / track.timescale.max(1) as f64;
    let color = match &track.color {
        Some(studio_mp4::ColorInfo::Nclx { primaries, transfer, matrix, full_range }) => ColorDesc { primaries: *primaries as u8, transfer: *transfer as u8, matrix: *matrix as u8, full_range: *full_range },
        _ => ColorDesc::BT709,
    };
    let info = TrackInfo { width: track.coded_width, height: track.coded_height, fps_num, fps_den, avcc: config.clone(), color, bitrate: if secs > 0.0 { (bytes as f64 * 8.0 / secs) as u32 } else { 0 }, hevc };
    let (width, height) = (track.coded_width, track.coded_height);
    drop(demux);

    let tmp = out.with_extension("merging.mp4");
    let mut mux = Muxer::create(&tmp, Options::mp4()).map_err(io)?;
    let vt = mux.add_video(info).map_err(io)?;
    let mut audio = Vec::new();
    for t in &opts.audio {
        let enc = aac::Encoder::new(aac::Config { sample_rate: t.rate, channels: t.channels as u16, bitrate: t.bitrate })?;
        let at = mux.add_audio(enc.track_info()).map_err(io)?;
        mux.set_language(at, &t.lang);
        mux.set_title(at, &t.title);
        audio.push((enc, at));
    }
    let mut subs = Vec::new();
    for t in &opts.subtitles {
        let st = mux.add_subtitles(SubtitleInfo { width, height, font_size: (height / 18).clamp(12, 255) as u8 }).map_err(io)?;
        mux.set_language(st, &t.lang);
        mux.set_title(st, &t.title);
        subs.push(st);
    }

    let mut frames = 0u64;
    for (i, p) in parts.iter().enumerate() {
        let mut d = studio_mp4::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let t = d.movie().video().ok_or_else(|| format!("{} has no video", p.display()))?.clone();
        let same = match &t.codec {
            studio_mp4::Codec::Avc { config: c } => c.raw == config && !hevc,
            studio_mp4::Codec::Hevc { config: c } => c.raw == config && hevc,
            _ => false,
        };
        if !same || (t.coded_width, t.coded_height) != (width, height) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("piece {i} was encoded differently from the first (different parameter sets or size): render every piece with the same encoder, e.g. STUDIO_SOFTWARE_ENCODE=1"));
        }
        if !t.samples.first().is_some_and(|s| s.sync) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("piece {i} does not start on a keyframe"));
        }
        let mut buf = Vec::new();
        for s in &t.samples {
            d.read_sample_into(s, &mut buf).map_err(mp4)?;
            mux.write_sample(vt, &buf, s.sync).map_err(io)?;
            frames += 1;
        }
    }
    for ((mut enc, t), src) in audio.into_iter().zip(&opts.audio) {
        let mut units = enc.push(&src.pcm);
        units.extend(enc.flush());
        for u in &units {
            mux.write_sample(t, u, true).map_err(io)?;
        }
        mux.set_audio_samples(t, (src.pcm.len() / src.channels.max(1)) as u64);
    }
    for (st, src) in subs.into_iter().zip(&opts.subtitles) {
        for (a, b, text) in &src.cues {
            mux.write_subtitle(st, *a, *b, text).map_err(io)?;
        }
    }
    mux.finish().map_err(io)?;
    std::fs::rename(&tmp, out).map_err(io)?;
    Ok(MergeStats { frames, pieces: parts.len(), width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::piece;

    #[test]
    fn pieces_join_losslessly_with_sound() {
        let dir = std::env::temp_dir().join(format!("studio-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let parts: Vec<PathBuf> = (0..3).map(|i| dir.join(format!("p{i}.mp4"))).collect();
        for (i, p) in parts.iter().enumerate() {
            piece(p, 64, 48, i as u32 * 10, 10);
        }
        // Expected: the exact compressed bytes of every piece, in order.
        let mut want: Vec<Vec<u8>> = Vec::new();
        for p in &parts {
            let mut d = studio_mp4::open(p).unwrap();
            let t = d.movie().video().unwrap().clone();
            for s in &t.samples {
                want.push(d.read_sample(s).unwrap());
            }
        }
        let rate = 48_000u32;
        let pcm: Vec<f32> = (0..rate as usize * 6 / 5).flat_map(|i| { let v = 0.2 * (i as f32 * 0.05).sin(); [v, v] }).collect();
        let out = dir.join("out.mp4");
        let opts = MergeOptions {
            audio: vec![AudioTrackIn { pcm, channels: 2, rate, bitrate: 192_000, lang: "eng".into(), title: "English".into() }],
            subtitles: vec![SubtitleTrackIn { lang: "eng".into(), title: "English".into(), cues: vec![(0, 500, "Hello".into()), (600, 1100, "World".into())] }],
        };
        let stats = merge(&parts, &out, &opts).unwrap();
        assert_eq!((stats.frames, stats.pieces, stats.width, stats.height), (30, 3, 64, 48));
        let mut d = studio_mp4::open(&out).unwrap();
        let t = d.movie().video().unwrap().clone();
        assert_eq!(t.samples.len(), 30);
        let got: Vec<Vec<u8>> = t.samples.iter().map(|s| d.read_sample(s).unwrap()).collect();
        assert_eq!(got, want, "pictures are copied, not re-encoded");
        assert_eq!(t.samples.iter().filter(|s| s.sync).count(), 3);
        assert!(d.movie().audio().is_some());
        assert_eq!(d.movie().tracks.len(), 3, "video, sound and captions");
        // And the joined file decodes: 30 pictures.
        let studio_mp4::Codec::Avc { config } = &t.codec else { panic!() };
        let sps: Vec<&[u8]> = config.sps.iter().map(|s| s.as_slice()).collect();
        let pps: Vec<&[u8]> = config.pps.iter().map(|s| s.as_slice()).collect();
        let mut dec = studio_h264::Decoder::new(&sps, &pps, config.nal_length_size as usize).unwrap();
        let mut n = 0;
        for s in &t.samples {
            n += dec.decode(&d.read_sample(s).unwrap()).unwrap().len();
        }
        n += dec.flush().len();
        assert_eq!(n, 30);
        // A piece from different settings is refused.
        let odd = dir.join("odd.mp4");
        piece(&odd, 80, 48, 0, 5);
        assert!(merge(&[parts[0].clone(), odd], &dir.join("bad.mp4"), &MergeOptions::default()).unwrap_err().contains("differently"));
        assert!(!dir.join("bad.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
