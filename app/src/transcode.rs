// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Video Studio's players do not read, made into H.264 + AAC MP4 in Rust,
//! with Studio's own code only.
//!
//! Reads WebM / MKV (VP9, AV1 or ProRes video; Opus or AAC sound) and MP4 /
//! MOV holding AV1, VP9 or ProRes (what video sites, screen recorders and
//! cameras hand over), decodes with the clean-room decoders vendored from
//! FilmCraft (`studio-vp9`, `studio-av1`, `studio-prores`, `studio-opus`,
//! the Matroska demuxer from craft-libs; see NOTICE), and writes the same
//! MP4 the older converter made: even size, constant frame rate,
//! BT.709, a sync frame every second, AAC sound.
//!
//! [`handles`] says whether every stream of a file is covered; anything
//! else is refused with a plain message.

use std::path::Path;
use studio_craft_media::matroska as mkv;
use studio_encode::{aac, h264, mux, yuv::Yuv420};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VCodec {
    Vp9,
    Av1,
    ProRes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ACodec {
    /// The `OpusHead`.
    Opus(Vec<u8>),
    /// AAC copied as it is: its AudioSpecificConfig, rate, channels.
    Aac(Vec<u8>, u32, u16),
}

/// One sample: when it is shown (seconds) and where its bytes are.
#[derive(Clone, Copy, Debug)]
struct At {
    secs: f64,
    index: usize,
}

/// A source file, its chosen streams and how to read their samples.
struct Source {
    file: std::fs::File,
    kind: Container,
    video: Option<(VCodec, usize, Vec<At>, f64)>,
    audio: Option<(ACodec, usize, Vec<At>)>,
    duration: f64,
}

enum Container {
    Mkv(mkv::MkvFile),
    Mp4(studio_mp4::Demuxer<std::io::BufReader<std::fs::File>>),
}

impl Source {
    fn read(&mut self, track: usize, index: usize) -> Result<Vec<u8>, String> {
        match &mut self.kind {
            Container::Mkv(m) => m.read_sample(&self.file, track, index).map_err(|e| e.to_string()),
            Container::Mp4(d) => {
                let s = d.movie().tracks[track].samples[index];
                d.read_sample(&s).map_err(|e| e.to_string())
            }
        }
    }
}

fn is_mkv(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    use std::io::Read;
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == [0x1A, 0x45, 0xDF, 0xA3]
}

/// Opens `path` and picks its first video and sound streams, if Rust can
/// decode every one Studio would keep.
fn open(path: &Path) -> Result<Source, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if is_mkv(path) {
        let m = mkv::open(&file).map_err(|e| format!("not a readable WebM / MKV file: {e}"))?;
        let mut video = None;
        let mut audio = None;
        for (ti, t) in m.tracks.iter().enumerate() {
            let (num, den) = t.timebase;
            let secs = |pts: i64| pts as f64 * num as f64 / den.max(1) as f64;
            match (&t.kind, &t.codec) {
                (mkv::TrackKind::Video, c) if video.is_none() => {
                    let codec = match c {
                        mkv::Codec::Vp9 { .. } => VCodec::Vp9,
                        mkv::Codec::Av1 { .. } => VCodec::Av1,
                        mkv::Codec::ProRes { .. } => VCodec::ProRes,
                        other => return Err(format!("video in {} is not decoded in Rust yet", other.name())),
                    };
                    if !t.encodings.is_empty() {
                        return Err("compressed or encrypted Matroska tracks are not read in Rust".into());
                    }
                    let ats: Vec<At> = t.samples.iter().enumerate().map(|(i, s)| At { secs: secs(s.pts), index: i }).collect();
                    let fps = t.default_duration_ns.filter(|d| *d > 0).map(|d| 1e9 / d as f64);
                    video = Some((codec, ti, ats, fps.unwrap_or(0.0)));
                }
                (mkv::TrackKind::Audio, c) if audio.is_none() => {
                    let codec = match c {
                        mkv::Codec::Opus { head } => ACodec::Opus(head.clone()),
                        mkv::Codec::Aac { asc } => {
                            let a = t.audio.as_ref();
                            ACodec::Aac(asc.clone(), a.map_or(48_000, |a| a.sampling_frequency.round() as u32), a.map_or(2, |a| a.channels as u16))
                        }
                        other => return Err(format!("sound in {} is not decoded in Rust yet", other.name())),
                    };
                    if !t.encodings.is_empty() {
                        return Err("compressed or encrypted Matroska tracks are not read in Rust".into());
                    }
                    let ats = t.samples.iter().enumerate().map(|(i, s)| At { secs: secs(s.pts), index: i }).collect();
                    audio = Some((codec, ti, ats));
                }
                _ => {}
            }
        }
        let duration = m.duration_ns().map(|d| d as f64 / 1e9).unwrap_or(0.0);
        return Ok(Source { file, kind: Container::Mkv(m), video, audio, duration });
    }
    let d = studio_mp4::open(path).map_err(|e| format!("not a readable MP4 / MOV file: {e}"))?;
    let mut video = None;
    let mut audio = None;
    for (ti, t) in d.movie().tracks.iter().enumerate() {
        let secs = |pts: i64| (pts - t.edit_offset) as f64 / t.timescale.max(1) as f64;
        match t.kind {
            studio_mp4::demux::TrackKind::Video if video.is_none() => {
                let fourcc = String::from_utf8_lossy(&t.fourcc).into_owned();
                let codec = match fourcc.as_str() {
                    "vp09" => VCodec::Vp9,
                    "av01" => VCodec::Av1,
                    "apch" | "apcn" | "apcs" | "apco" | "ap4h" | "ap4x" => VCodec::ProRes,
                    other => return Err(format!("video in {other} is not decoded in Rust yet")),
                };
                let ats = t.samples.iter().enumerate().map(|(i, s)| At { secs: secs(s.pts), index: i }).collect();
                let fps = if t.duration_secs > 0.0 { t.samples.len() as f64 / t.duration_secs } else { 0.0 };
                video = Some((codec, ti, ats, fps));
            }
            studio_mp4::demux::TrackKind::Audio if audio.is_none() => {
                let codec = match &t.codec {
                    studio_mp4::Codec::Aac { asc, sample_rate, channels, .. } => ACodec::Aac(asc.clone(), *sample_rate, *channels),
                    _ => return Err(format!("sound in {} is not decoded in Rust yet", String::from_utf8_lossy(&t.fourcc))),
                };
                let ats = t.samples.iter().enumerate().map(|(i, s)| At { secs: secs(s.pts), index: i }).collect();
                audio = Some((codec, ti, ats));
            }
            _ => {}
        }
    }
    let duration = d.movie().duration_secs;
    Ok(Source { file, kind: Container::Mp4(d), video, audio, duration })
}

/// Whether [`to_mp4`] can convert `path`: a video stream it decodes and no
/// sound it cannot.
pub fn handles(path: &Path) -> bool {
    open(path).is_ok_and(|s| s.video.is_some())
}

/// The frame rate to write: the container's, snapped to a standard rate.
fn rate(fps: f64) -> (u32, u32) {
    const STANDARD: [(u32, u32); 10] = [(24000, 1001), (24, 1), (25, 1), (30000, 1001), (30, 1), (48, 1), (50, 1), (60000, 1001), (60, 1), (120, 1)];
    let off = |(n, d): (u32, u32)| (fps - f64::from(n) / f64::from(d)).abs() / fps;
    if let Some(best) = STANDARD.into_iter().filter(|r| off(*r) < 0.01).min_by(|a, b| off(*a).total_cmp(&off(*b))) {
        return best;
    }
    if fps.is_finite() && fps >= 1.0 { ((fps.round() as u32).clamp(1, 240), 1) } else { (30, 1) }
}

/// A decoded picture, any depth and chroma layout, as samples to read.
struct Planes<'a> {
    w: usize,
    h: usize,
    depth: u32,
    /// Chroma is halved across / down.
    ss: (bool, bool),
    full_range: bool,
    sample: Box<dyn Fn(usize, usize, usize) -> u16 + 'a>,
}

/// 8-bit 4:2:0 limited range at `w`×`h` (even, cropped from the top left).
fn to_yuv420(p: &Planes, w: u32, h: u32) -> Yuv420 {
    let (w, h) = (w as usize, h as usize);
    let shift = p.depth.saturating_sub(8);
    let to8 = |v: u16| -> u8 { (((u32::from(v)) + if shift > 0 { 1 << (shift - 1) } else { 0 }) >> shift).min(255) as u8 };
    let luma = |v: u8| -> u8 { if p.full_range { (16 + (u32::from(v) * 219 + 127) / 255) as u8 } else { v } };
    let chroma = |v: u8| -> u8 { if p.full_range { (16 + (u32::from(v) * 224 + 127) / 255) as u8 } else { v } };
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = luma(to8((p.sample)(0, c.min(p.w - 1), r.min(p.h - 1))));
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let (pw, ph) = (if p.ss.0 { p.w.div_ceil(2) } else { p.w }, if p.ss.1 { p.h.div_ceil(2) } else { p.h });
    let mut planes = [vec![0u8; cw * ch], vec![0u8; cw * ch]];
    for (k, out) in planes.iter_mut().enumerate() {
        for r in 0..ch {
            for c in 0..cw {
                // The source chroma covering this 2x2 block, averaged.
                let xs: &[usize] = if p.ss.0 { &[c] } else { &[2 * c, 2 * c + 1] };
                let ys: &[usize] = if p.ss.1 { &[r] } else { &[2 * r, 2 * r + 1] };
                let mut sum = 0u32;
                let mut n = 0u32;
                for &yy in ys {
                    for &xx in xs {
                        sum += u32::from((p.sample)(k + 1, xx.min(pw - 1), yy.min(ph - 1)));
                        n += 1;
                    }
                }
                out[r * cw + c] = chroma(to8(((sum + n / 2) / n) as u16));
            }
        }
    }
    let [u, v] = planes;
    Yuv420 { width: w as u32, height: h as u32, y, u, v }
}

enum VDec {
    Vp9(Box<studio_vp9::Decoder>),
    Av1(Box<studio_av1::Decoder>),
    ProRes,
}

impl VDec {
    /// Pictures shown by `data` (display order), as (pts in µs, 8-bit 4:2:0).
    fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<(i64, Yuv420)>, String> {
        match self {
            VDec::Vp9(d) => Ok(d.decode(data, pts).map_err(|e| format!("VP9: {e}"))?.iter().map(|p| (p.pts, vp9_yuv(p))).collect()),
            VDec::Av1(d) => Ok(d.decode_pts(data, pts).map_err(|e| format!("AV1: {e}"))?.iter().map(|p| (p.pts, av1_yuv(p))).collect()),
            VDec::ProRes => {
                let f = studio_prores::decode_frame(data).map_err(|e| format!("ProRes: {e}"))?;
                Ok(vec![(pts, prores_yuv(&f))])
            }
        }
    }

    fn flush(&mut self) -> Vec<(i64, Yuv420)> {
        match self {
            VDec::Vp9(d) => d.flush().iter().map(|p| (p.pts, vp9_yuv(p))).collect(),
            VDec::Av1(d) => d.flush().iter().map(|p| (p.pts, av1_yuv(p))).collect(),
            VDec::ProRes => Vec::new(),
        }
    }
}

fn vp9_yuv(p: &studio_vp9::Picture) -> Yuv420 {
    let (w, h) = (p.width & !1, p.height & !1);
    let planes = [(&p.y, p.y_stride), (&p.u, p.uv_stride), (&p.v, p.uv_stride)];
    to_yuv420(&Planes { w: p.width as usize, h: p.height as usize, depth: p.bit_depth, ss: (p.subsampling_x, p.subsampling_y), full_range: p.color.full_range, sample: Box::new(move |k, x, y| planes[k].0.get(y * planes[k].1 + x)) }, w, h)
}

fn av1_yuv(p: &studio_av1::Picture) -> Yuv420 {
    let (w, h) = (p.width & !1, p.height & !1);
    let widths = [p.plane_width(0) as usize, p.plane_width(1) as usize, p.plane_width(2) as usize];
    let mid = 128u16 << (p.bit_depth.saturating_sub(8));
    let mono = p.mono_chrome;
    to_yuv420(&Planes { w: p.width as usize, h: p.height as usize, depth: u32::from(p.bit_depth), ss: (p.subsampling_x == 1, p.subsampling_y == 1), full_range: p.full_range, sample: Box::new(move |k, x, y| if mono && k > 0 { mid } else { p.planes[k][y * widths[k] + x] }) }, w, h)
}

fn prores_yuv(f: &studio_prores::Frame) -> Yuv420 {
    let (w, h) = (f.width & !1, f.height & !1);
    let cw = f.chroma_width() as usize;
    let fw = f.width as usize;
    let half = cw < fw;
    to_yuv420(&Planes { w: fw, h: f.height as usize, depth: u32::from(f.bit_depth), ss: (half, false), full_range: false, sample: Box::new(move |k, x, y| [&f.y, &f.cb, &f.cr][k][y * if k == 0 { fw } else { cw } + x]) }, w, h)
}

/// Converts `src` into an H.264 + AAC MP4 at `dst`.
pub fn to_mp4(src: &Path, dst: &Path) -> Result<(), String> {
    let mut s = open(src)?;
    let (vcodec, vtrack, vats, fps) = s.video.clone().ok_or("no video stream to convert")?;
    if vats.is_empty() {
        return Err("the video stream is empty".into());
    }
    // The size from the first picture: decoded before anything is written.
    let mut dec = match vcodec {
        VCodec::Vp9 => VDec::Vp9(Box::new(studio_vp9::Decoder::new())),
        VCodec::Av1 => VDec::Av1(Box::new(studio_av1::Decoder::new())),
        VCodec::ProRes => VDec::ProRes,
    };
    let fps = if fps > 0.0 {
        fps
    } else {
        let last = vats.iter().map(|a| a.secs).fold(0.0, f64::max);
        if last > 0.0 { (vats.len() - 1) as f64 / last } else { 30.0 }
    };
    let (num, den) = rate(fps);
    let step = den as f64 / num as f64;
    let t0 = vats.iter().map(|a| a.secs).fold(f64::INFINITY, f64::min);

    let mut pending: Vec<(f64, Yuv420)> = Vec::new();
    let mut size: Option<(u32, u32)> = None;
    let mut out: Option<(h264::Encoder, mux::Muxer, usize)> = None;
    let part = dst.with_extension("part.mp4");
    let _ = std::fs::remove_file(&part);
    let mut written = 0u64;
    let mut held: Option<Yuv420> = None;

    // Audio, decoded up front (small next to the picture).
    let audio = match s.audio.clone() {
        Some((codec, track, ats)) => {
            let pcm = decode_audio(&mut s, &codec, track, &ats)?;
            Some(encode_audio(codec, pcm)?)
        }
        None => None,
    };

    let mut audio_track = None;
    let mut first_err = None;
    for (i, at) in vats.iter().enumerate() {
        let data = s.read(vtrack, at.index)?;
        let pts = ((at.secs - t0) * 1e6).round() as i64;
        let pics = match dec.decode(&data, pts) {
            Ok(p) => p,
            // A damaged frame mid-stream is skipped (the last good one
            // holds); a bad first frame fails the conversion.
            Err(e) if i > 0 => {
                first_err.get_or_insert(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        for (p, pic) in pics {
            // The first picture sets the size; a stream that changes size
            // part way is refused.
            match size {
                None if pic.width < 2 || pic.height < 2 => return Err("the video is smaller than 2 pixels".into()),
                None => size = Some((pic.width, pic.height)),
                Some(sz) if sz != (pic.width, pic.height) => return Err("the picture size changes part way through".into()),
                Some(_) => {}
            }
            pending.push((p as f64 / 1e6, pic));
        }
        if out.is_none() {
            if let Some((w, h)) = size {
                // An intermediate to edit from: three times the export's usual
                // rate, so a second encode at export costs little.
                let mut cfg = h264::Config::new(w, h, num, den);
                cfg.bitrate_kbps = cfg.bitrate_kbps.saturating_mul(3);
                let enc = h264::Encoder::new(cfg)?;
                let mut m = mux::Muxer::create(&part, mux::Options::mp4()).map_err(|e| e.to_string())?;
                let v = m.add_video(enc.track_info()).map_err(|e| e.to_string())?;
                // Every track before the first sample.
                if let Some((info, _, _)) = &audio {
                    audio_track = Some(m.add_audio(info.clone()).map_err(|e| e.to_string())?);
                }
                out = Some((enc, m, v));
            }
        }
        if let Some((enc, m, v)) = out.as_mut() {
            pending.sort_by(|a, b| a.0.total_cmp(&b.0));
            emit(&mut pending, &mut held, &mut written, step, false, enc, m, *v)?;
        }
    }
    let (mut enc, mut m, v) = out.ok_or_else(|| first_err.clone().unwrap_or_else(|| "no picture could be decoded".into()))?;
    pending.extend(dec.flush().into_iter().map(|(p, pic)| (p as f64 / 1e6, pic)));
    pending.sort_by(|a, b| a.0.total_cmp(&b.0));
    emit(&mut pending, &mut held, &mut written, step, true, &mut enc, &mut m, v)?;
    // The last picture lasts to the end of the file.
    let last = vats.iter().map(|a| a.secs).fold(t0, f64::max) - t0;
    let end = (last + step).max(written as f64 * step);
    if let Some(last) = held.take() {
        while (written as f64 + 0.5) * step < end {
            let f = enc.encode(&last);
            m.write_sample(v, &f.data, f.keyframe).map_err(|e| e.to_string())?;
            written += 1;
        }
    }
    if let (Some(a), Some((_, aus, presented))) = (audio_track, audio) {
        for au in aus {
            m.write_sample(a, &au, true).map_err(|e| e.to_string())?;
        }
        if let Some(n) = presented {
            m.set_audio_samples(a, n);
        }
    }
    m.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&part, dst).map_err(|e| e.to_string())
}

/// Writes frames at the constant rate: each output slot shows the latest
/// picture shown by then (repeats fill gaps, extras are dropped).
#[allow(clippy::too_many_arguments)]
fn emit(pending: &mut Vec<(f64, Yuv420)>, held: &mut Option<Yuv420>, written: &mut u64, step: f64, all: bool, enc: &mut h264::Encoder, m: &mut mux::Muxer, v: usize) -> Result<(), String> {
    // Pictures can come out of a decoder a little late (reordering): only
    // those older than the newest one are certainly final.
    let keep = if all { 0 } else { 2.min(pending.len()) };
    while pending.len() > keep {
        let (t, pic) = pending.remove(0);
        // Slots before this picture show the one held.
        while (*written as f64 + 0.5) * step <= t {
            match held.as_ref() {
                Some(h) => {
                    let f = enc.encode(h);
                    m.write_sample(v, &f.data, f.keyframe).map_err(|e| e.to_string())?;
                    *written += 1;
                }
                None => break,
            }
        }
        *held = Some(pic);
    }
    Ok(())
}

/// Decoded sound, interleaved, at its rate; or AAC to copy.
enum Pcm {
    Interleaved { rate: u32, channels: u16, samples: Vec<f32> },
    Copy(Vec<Vec<u8>>),
}

fn decode_audio(s: &mut Source, codec: &ACodec, track: usize, ats: &[At]) -> Result<Pcm, String> {
    match codec {
        ACodec::Opus(head) => {
            let mut d = studio_opus::Decoder::new(head).map_err(|e| format!("Opus: {e}"))?;
            d.set_trim_pre_skip(true);
            let ch = d.channels().clamp(1, 8);
            let out_ch = ch.min(2);
            let mut samples = Vec::new();
            for at in ats {
                let data = s.read(track, at.index)?;
                let planes = d.decode(Some(&data)).map_err(|e| format!("Opus: {e}"))?;
                let n = planes.first().map_or(0, |p| p.len());
                for i in 0..n {
                    if out_ch == 1 {
                        samples.push(planes[0][i]);
                    } else if ch == 2 {
                        samples.push(planes[0][i]);
                        samples.push(planes[1][i]);
                    } else {
                        // Surround folded to stereo: front left / right
                        // plus half of everything else (Vorbis order: L C R…).
                        let (mut l, mut r) = (planes[0][i], planes[2.min(ch - 1)][i]);
                        for (k, p) in planes.iter().enumerate().skip(1) {
                            if k != 2 {
                                let half = p[i] * 0.5;
                                l += half;
                                r += half;
                            }
                        }
                        samples.push((l * 0.6).clamp(-1.0, 1.0));
                        samples.push((r * 0.6).clamp(-1.0, 1.0));
                    }
                }
            }
            Ok(Pcm::Interleaved { rate: 48_000, channels: out_ch as u16, samples })
        }
        ACodec::Aac(..) => {
            let mut aus = Vec::with_capacity(ats.len());
            for at in ats {
                aus.push(s.read(track, at.index)?);
            }
            Ok(Pcm::Copy(aus))
        }
    }
}

/// The sound as AAC: its track, its access units and its presented length
/// in samples (when re-encoded).
fn encode_audio(codec: ACodec, pcm: Pcm) -> Result<(aac::TrackInfo, Vec<Vec<u8>>, Option<u64>), String> {
    match (codec, pcm) {
        (_, Pcm::Interleaved { rate, channels, samples }) => {
            let mut enc = aac::Encoder::new(aac::Config { sample_rate: rate, channels, bitrate: 192_000 })?;
            let mut aus = Vec::new();
            for chunk in samples.chunks(4096 * channels as usize) {
                aus.extend(enc.push(chunk));
            }
            aus.extend(enc.flush());
            Ok((enc.track_info(), aus, Some((samples.len() / channels.max(1) as usize) as u64)))
        }
        (ACodec::Aac(asc, rate, channels), Pcm::Copy(aus)) => Ok((aac::TrackInfo { sample_rate: rate, channels, asc, bitrate: 0, priming: 0 }, aus, None)),
        _ => Err("sound codec and samples disagree".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_snap_to_standard_ones() {
        assert_eq!(rate(29.97), (30000, 1001));
        assert_eq!(rate(30.0), (30, 1));
        assert_eq!(rate(23.976), (24000, 1001));
        assert_eq!(rate(25.0), (25, 1));
        assert_eq!(rate(59.94), (60000, 1001));
        assert_eq!(rate(15.0), (15, 1));
        assert_eq!(rate(f64::NAN), (30, 1));
    }

    #[test]
    fn ten_bit_full_range_and_422_become_8_bit_limited_420() {
        // A 4x2 picture: 10-bit full-range white luma, 4:2:2 chroma.
        let p = Planes { w: 4, h: 2, depth: 10, ss: (true, false), full_range: true, sample: Box::new(|k, _, _| if k == 0 { 1023 } else { 512 }) };
        let y = to_yuv420(&p, 4, 2);
        assert!(y.y.iter().all(|v| *v == 235), "{:?}", y.y);
        assert!(y.u.iter().chain(&y.v).all(|v| *v == 128 || *v == 129), "{:?}", y.u);
        assert_eq!((y.u.len(), y.v.len()), (2, 2));
    }
}
