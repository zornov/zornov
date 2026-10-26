// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ISO base media file writer for `.mp4` (ISO/IEC 14496-12/14/15) and
//! QuickTime `.mov`, with an H.264/HEVC video track, any number of AAC audio
//! tracks (one per language or mix, the first enabled and the rest
//! alternates) and timed-text subtitle tracks (3GPP `tx3g`, one per
//! language). Tracks carry an ISO 639-2 language code and a title.
//!
//! Samples are buffered per track and written to `mdat` in interleaved chunks
//! of about half a second. With [`Options::fast_start`] the finished file has
//! `moov` in front of `mdat` (media data is first streamed to a side file and
//! copied behind the index on [`Muxer::finish`]); otherwise `moov` follows
//! `mdat`. Offsets switch to `co64` and `mdat` to a 64-bit size when needed.

use crate::h264::{self, ColorDesc};
use crate::aac;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Container flavour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brand {
    /// MPEG-4 Part 14 (`.mp4`).
    Mp4,
    /// QuickTime File Format (`.mov`).
    Mov,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub brand: Brand,
    /// Put `moov` before `mdat` so playback can start while downloading.
    pub fast_start: bool,
    /// Interleaving chunk length in seconds.
    pub chunk_secs: f64,
}

impl Options {
    pub fn mp4() -> Options {
        Options { brand: Brand::Mp4, fast_start: true, chunk_secs: 0.5 }
    }
    pub fn mov() -> Options {
        Options { brand: Brand::Mov, fast_start: true, chunk_secs: 0.5 }
    }
}

const MOVIE_TIMESCALE: u32 = 1000;

enum Media {
    Video(h264::TrackInfo),
    Audio(aac::TrackInfo),
    Text,
}

/// A timed-text track's description.
#[derive(Clone, Debug)]
pub struct SubtitleInfo {
    /// Text box size (usually the video size).
    pub width: u32,
    pub height: u32,
    /// Font size in pixels of the default style.
    pub font_size: u8,
}

/// Packs an ISO 639-2/T code (`"eng"`, `"hin"`, …) for `mdhd`; anything
/// else is `und`.
pub fn pack_language(code: &str) -> u16 {
    let b = code.as_bytes();
    if b.len() != 3 || !b.iter().all(|c| c.is_ascii_lowercase()) {
        return 0x55C4;
    }
    ((b[0] - 0x60) as u16) << 10 | ((b[1] - 0x60) as u16) << 5 | (b[2] - 0x60) as u16
}

struct Track {
    media: Media,
    timescale: u32,
    delta: u32,
    sizes: Vec<u32>,
    /// 1-based numbers of sync samples.
    sync: Vec<u32>,
    /// (file offset relative to the start of the mdat payload, samples).
    chunks: Vec<(u64, u32)>,
    pending: Vec<u8>,
    pending_samples: u32,
    /// Presented length in track timescale (audio: without priming).
    presented: Option<u64>,
    /// Per-sample durations when they vary (timed text).
    durations: Vec<u32>,
    /// Packed ISO 639-2 language.
    language: u16,
    /// Title (handler name).
    title: Option<String>,
    /// Plays by default (tkhd enabled flag); `None`: decided by kind.
    enabled: Option<bool>,
    /// Where the next timed-text sample starts, in ms.
    text_clock: u64,
}

impl Track {
    fn media_duration(&self) -> u64 {
        if !self.durations.is_empty() {
            return self.durations.iter().map(|&d| d as u64).sum();
        }
        self.sizes.len() as u64 * self.delta as u64
    }
    fn priming(&self) -> u64 {
        match &self.media {
            Media::Audio(a) => a.priming as u64,
            Media::Video(_) | Media::Text => 0,
        }
    }
    fn presented_duration(&self) -> u64 {
        self.presented.unwrap_or_else(|| self.media_duration().saturating_sub(self.priming()))
    }
}

pub struct Muxer {
    path: PathBuf,
    /// Where the mdat payload is streamed: the output itself, or a side file for fast start.
    data_path: PathBuf,
    out: Option<BufWriter<File>>,
    opts: Options,
    tracks: Vec<Track>,
    subtitle_info: Vec<(usize, SubtitleInfo)>,
    /// Bytes of mdat payload written so far.
    mdat_len: u64,
    /// File offset of the mdat payload when streaming directly into the output.
    payload_start: u64,
    done: bool,
}

/// Seconds between 1904-01-01 and 1970-01-01.
const MAC_EPOCH: u64 = 2_082_844_800;

fn now_mac() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) + MAC_EPOCH
}

impl Muxer {
    /// Create `path` (overwriting it).
    pub fn create(path: impl AsRef<Path>, opts: Options) -> io::Result<Muxer> {
        let path = path.as_ref().to_path_buf();
        let data_path = if opts.fast_start {
            let mut p = path.clone().into_os_string();
            p.push(".mdat.part");
            PathBuf::from(p)
        } else {
            path.clone()
        };
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&data_path)?);
        let mut payload_start = 0;
        if !opts.fast_start {
            let ftyp = ftyp(opts.brand);
            out.write_all(&ftyp)?;
            // mdat with a 64-bit size patched on finish.
            out.write_all(&1u32.to_be_bytes())?;
            out.write_all(b"mdat")?;
            out.write_all(&0u64.to_be_bytes())?;
            payload_start = ftyp.len() as u64 + 16;
        }
        Ok(Muxer { path, data_path, out: Some(out), opts, tracks: Vec::new(), subtitle_info: Vec::new(), mdat_len: 0, payload_start, done: false })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Add the H.264 track; returns its index for [`Muxer::write_sample`].
    pub fn add_video(&mut self, info: h264::TrackInfo) -> io::Result<usize> {
        let (timescale, delta) = if info.fps_den == 1 { (info.fps_num * 1000, 1000) } else { (info.fps_num, info.fps_den) };
        self.push_track(Media::Video(info), timescale, delta)
    }

    /// Add the AAC track; returns its index.
    pub fn add_audio(&mut self, info: aac::TrackInfo) -> io::Result<usize> {
        let rate = info.sample_rate;
        self.push_track(Media::Audio(info), rate, aac::FRAME as u32)
    }

    /// Add a timed-text subtitle track (timescale 1 ms); fill it with
    /// [`Muxer::write_subtitle`] in time order.
    pub fn add_subtitles(&mut self, info: SubtitleInfo) -> io::Result<usize> {
        let t = self.push_track(Media::Text, 1000, 1)?;
        self.subtitle_info.push((t, info));
        Ok(t)
    }

    fn push_track(&mut self, media: Media, timescale: u32, delta: u32) -> io::Result<usize> {
        if !self.tracks.iter().all(|t| t.sizes.is_empty()) {
            return Err(io::Error::other("tracks must be added before samples"));
        }
        self.tracks.push(Track { media, timescale, delta, sizes: Vec::new(), sync: Vec::new(), chunks: Vec::new(), pending: Vec::new(), pending_samples: 0, presented: None, durations: Vec::new(), language: 0x55C4, title: None, enabled: None, text_clock: 0 });
        Ok(self.tracks.len() - 1)
    }

    /// Sets a track's language (ISO 639-2/T, e.g. `"eng"`, `"spa"`, `"hin"`).
    pub fn set_language(&mut self, track: usize, code: &str) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.language = pack_language(code);
        }
    }

    /// Sets a track's title, shown by players in the track menu.
    pub fn set_title(&mut self, track: usize, title: &str) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.title = Some(title.to_owned());
        }
    }

    /// Whether a track plays by default. Without a call: video, the first
    /// audio track and no subtitles are enabled.
    pub fn set_enabled(&mut self, track: usize, on: bool) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.enabled = Some(on);
        }
    }

    /// Adds one subtitle from `start_ms` to `end_ms` (after the previous
    /// one; gaps become empty samples, overlaps are cut).
    pub fn write_subtitle(&mut self, track: usize, start_ms: u64, end_ms: u64, text: &str) -> io::Result<()> {
        let clock = self.tracks.get(track).ok_or_else(|| io::Error::other("no such track"))?.text_clock;
        if start_ms > clock {
            self.text_sample(track, start_ms - clock, "")?;
        }
        let start = start_ms.max(clock);
        if end_ms > start {
            self.text_sample(track, end_ms - start, text)?;
        }
        Ok(())
    }

    fn text_sample(&mut self, track: usize, dur_ms: u64, text: &str) -> io::Result<()> {
        let bytes = text.as_bytes();
        let n = bytes.len().min(u16::MAX as usize);
        let mut data = Vec::with_capacity(n + 2);
        data.extend_from_slice(&(n as u16).to_be_bytes());
        data.extend_from_slice(&bytes[..n]);
        let d = dur_ms.min(u32::MAX as u64) as u32;
        {
            let t = &mut self.tracks[track];
            t.durations.push(d);
            t.text_clock += d as u64;
        }
        self.write_sample(track, &data, true)
    }

    /// The presented length of an audio track in samples (the rest of the
    /// last access unit, and the priming, are hidden by the edit list).
    pub fn set_audio_samples(&mut self, track: usize, samples: u64) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.presented = Some(samples);
        }
    }

    /// Append one sample (an AVCC framed access unit, or a raw AAC frame).
    pub fn write_sample(&mut self, track: usize, data: &[u8], sync: bool) -> io::Result<()> {
        let chunk_ticks;
        {
            let t = self.tracks.get_mut(track).ok_or_else(|| io::Error::other("no such track"))?;
            t.sizes.push(data.len() as u32);
            if sync {
                t.sync.push(t.sizes.len() as u32);
            }
            t.pending.extend_from_slice(data);
            t.pending_samples += 1;
            chunk_ticks = if t.durations.is_empty() { (t.pending_samples as u64 * t.delta as u64) as f64 / t.timescale as f64 } else { t.pending_samples as f64 / 4.0 };
        }
        if chunk_ticks >= self.opts.chunk_secs {
            self.flush_track(track)?;
        }
        Ok(())
    }

    fn flush_track(&mut self, track: usize) -> io::Result<()> {
        let t = &mut self.tracks[track];
        if t.pending_samples == 0 {
            return Ok(());
        }
        t.chunks.push((self.mdat_len, t.pending_samples));
        self.out.as_mut().ok_or_else(|| io::Error::other("muxer closed"))?.write_all(&t.pending)?;
        self.mdat_len += t.pending.len() as u64;
        t.pending.clear();
        t.pending_samples = 0;
        Ok(())
    }

    /// Write the index and close the file. Returns the file size.
    pub fn finish(mut self) -> io::Result<u64> {
        for i in 0..self.tracks.len() {
            self.flush_track(i)?;
        }
        let out = self.out.take().ok_or_else(|| io::Error::other("muxer closed"))?;
        self.done = true;
        let brand = self.opts.brand;
        if !self.opts.fast_start {
            let moov = self.moov(self.payload_start, self.payload_start + self.mdat_len > u32::MAX as u64);
            let mut f = out.into_inner().map_err(|e| e.into_error())?;
            f.write_all(&moov)?;
            f.seek(SeekFrom::Start(self.payload_start - 8))?;
            f.write_all(&(self.mdat_len + 16).to_be_bytes())?;
            f.sync_all()?;
            return Ok(self.payload_start + self.mdat_len + moov.len() as u64);
        }
        // Fast start: ftyp + moov + mdat header, then the streamed payload.
        let ftyp = ftyp(brand);
        let large = self.mdat_len + 8 > u32::MAX as u64;
        let mdat_header = if large { 16 } else { 8 };
        let probe = self.moov(0, false);
        let co64 = ftyp.len() as u64 + probe.len() as u64 + mdat_header + self.mdat_len > u32::MAX as u64;
        let len = self.moov(0, co64).len() as u64;
        let base = ftyp.len() as u64 + len + mdat_header;
        let moov = self.moov(base, co64);
        debug_assert_eq!(moov.len() as u64, len);
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        let mut dst = BufWriter::with_capacity(1 << 20, File::create(&self.path)?);
        dst.write_all(&ftyp)?;
        dst.write_all(&moov)?;
        if large {
            dst.write_all(&1u32.to_be_bytes())?;
            dst.write_all(b"mdat")?;
            dst.write_all(&(self.mdat_len + 16).to_be_bytes())?;
        } else {
            dst.write_all(&((self.mdat_len + 8) as u32).to_be_bytes())?;
            dst.write_all(b"mdat")?;
        }
        let mut src = File::open(&self.data_path)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])?;
        }
        let f = dst.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        drop(src);
        let _ = std::fs::remove_file(&self.data_path);
        Ok(base + self.mdat_len)
    }

    /// Stop and delete everything written.
    pub fn abandon(self) {
        // Drop removes the partial files.
    }

    fn moov(&self, base: u64, co64: bool) -> Vec<u8> {
        let mov = self.opts.brand == Brand::Mov;
        let now = now_mac();
        let mut b = Boxes::default();
        b.open(b"moov");
        let movie_dur = self.tracks.iter().map(|t| to_movie(t.presented_duration(), t.timescale)).max().unwrap_or(0);
        // mvhd
        let v1 = movie_dur > u32::MAX as u64;
        b.open_full(b"mvhd", v1 as u8, 0);
        times(&mut b, v1, now);
        b.u32(MOVIE_TIMESCALE);
        dur(&mut b, v1, movie_dur);
        b.u32(0x0001_0000); // rate
        b.u16(0x0100); // volume
        b.zeros(10);
        matrix(&mut b);
        b.zeros(24);
        b.u32(self.tracks.len() as u32 + 1);
        b.close();
        for (i, t) in self.tracks.iter().enumerate() {
            self.trak(&mut b, t, i as u32 + 1, base, co64, mov, now);
        }
        b.close();
        b.buf
    }

    #[allow(clippy::too_many_arguments)]
    fn trak(&self, b: &mut Boxes, t: &Track, id: u32, base: u64, co64: bool, mov: bool, now: u64) {
        let video = matches!(t.media, Media::Video(_));
        let text = matches!(t.media, Media::Text);
        let presented = t.presented_duration();
        let movie_dur = to_movie(presented, t.timescale);
        let first_audio = self.tracks.iter().position(|x| matches!(x.media, Media::Audio(_)));
        let index = id as usize - 1;
        let enabled = t.enabled.unwrap_or(match t.media {
            Media::Video(_) => true,
            Media::Audio(_) => first_audio == Some(index),
            Media::Text => false,
        });
        b.open(b"trak");
        // tkhd: enabled (1) | in movie (2)
        let v1 = movie_dur > u32::MAX as u64;
        b.open_full(b"tkhd", v1 as u8, if enabled { 0x3 } else { 0x2 });
        times(b, v1, now);
        b.u32(id);
        b.u32(0);
        dur(b, v1, movie_dur);
        b.zeros(8);
        b.u16(if text { 0xFFFF } else { 0 }); // layer (text in front)
        b.u16(match t.media { Media::Video(_) => 0, Media::Audio(_) => 1, Media::Text => 2 }); // alternate_group
        b.u16(if matches!(t.media, Media::Audio(_)) { 0x0100 } else { 0 }); // volume
        b.u16(0);
        matrix(b);
        match &t.media {
            Media::Video(v) => {
                b.u32(v.width << 16);
                b.u32(v.height << 16);
            }
            Media::Audio(_) => {
                b.u32(0);
                b.u32(0);
            }
            Media::Text => {
                let info = self.subtitle_info.iter().find(|(i, _)| *i == index).map(|(_, s)| s);
                b.u32(info.map_or(0, |s| s.width) << 16);
                b.u32(info.map_or(0, |s| s.height) << 16);
            }
        }
        b.close();
        // edts / elst: hide the AAC priming; video starts at 0.
        b.open(b"edts");
        b.open_full(b"elst", 0, 0);
        b.u32(1);
        b.u32(movie_dur.min(u32::MAX as u64) as u32);
        b.u32(t.priming() as u32);
        b.u32(0x0001_0000);
        b.close();
        b.close();
        b.open(b"mdia");
        let media_dur = t.media_duration();
        let v1 = media_dur > u32::MAX as u64;
        b.open_full(b"mdhd", v1 as u8, 0);
        times(b, v1, now);
        b.u32(t.timescale);
        dur(b, v1, media_dur);
        b.u16(t.language);
        b.u16(0);
        b.close();
        let (handler, name): (&[u8; 4], &str) = match t.media {
            Media::Video(_) => (b"vide", "VideoHandler"),
            Media::Audio(_) => (b"soun", "SoundHandler"),
            Media::Text => (b"sbtl", "SubtitleHandler"),
        };
        hdlr(b, if mov { b"mhlr" } else { &[0; 4] }, handler, t.title.as_deref().unwrap_or(name), mov);
        b.open(b"minf");
        match t.media {
            Media::Video(_) => {
                b.open_full(b"vmhd", 0, 1);
                b.zeros(8);
                b.close();
            }
            Media::Audio(_) => {
                b.open_full(b"smhd", 0, 0);
                b.zeros(4);
                b.close();
            }
            Media::Text => {
                b.open_full(b"nmhd", 0, 0);
                b.close();
            }
        }
        if mov {
            hdlr(b, b"dhlr", b"url ", "DataHandler", true);
        }
        b.open(b"dinf");
        b.open_full(b"dref", 0, 0);
        b.u32(1);
        b.open_full(b"url ", 0, 1);
        b.close();
        b.close();
        b.close();
        b.open(b"stbl");
        b.open_full(b"stsd", 0, 0);
        b.u32(1);
        match &t.media {
            Media::Video(v) => self.avc1(b, t, v, mov),
            Media::Audio(a) => self.mp4a(b, t, a, mov),
            Media::Text => {
                let info = self.subtitle_info.iter().find(|(i, _)| *i == index).map(|(_, s)| s.clone()).unwrap_or(SubtitleInfo { width: 0, height: 0, font_size: 24 });
                tx3g(b, &info);
            }
        }
        b.close();
        // stts
        b.open_full(b"stts", 0, 0);
        if t.durations.is_empty() {
            b.u32(1);
            b.u32(t.sizes.len() as u32);
            b.u32(t.delta);
        } else {
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for &d in &t.durations {
                match runs.last_mut() {
                    Some((n, x)) if *x == d => *n += 1,
                    _ => runs.push((1, d)),
                }
            }
            b.u32(runs.len() as u32);
            for (n, d) in runs {
                b.u32(n);
                b.u32(d);
            }
        }
        b.close();
        // stss (only when some samples are not sync samples)
        if video && t.sync.len() != t.sizes.len() {
            b.open_full(b"stss", 0, 0);
            b.u32(t.sync.len() as u32);
            for &s in &t.sync {
                b.u32(s);
            }
            b.close();
        }
        // stsc
        b.open_full(b"stsc", 0, 0);
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (i, &(_, n)) in t.chunks.iter().enumerate() {
            if runs.last().is_none_or(|r| r.1 != n) {
                runs.push((i as u32 + 1, n));
            }
        }
        b.u32(runs.len() as u32);
        for (first, n) in runs {
            b.u32(first);
            b.u32(n);
            b.u32(1);
        }
        b.close();
        // stsz
        b.open_full(b"stsz", 0, 0);
        b.u32(0);
        b.u32(t.sizes.len() as u32);
        for &s in &t.sizes {
            b.u32(s);
        }
        b.close();
        // stco / co64
        if co64 {
            b.open_full(b"co64", 0, 0);
            b.u32(t.chunks.len() as u32);
            for &(off, _) in &t.chunks {
                b.u64(base + off);
            }
        } else {
            b.open_full(b"stco", 0, 0);
            b.u32(t.chunks.len() as u32);
            for &(off, _) in &t.chunks {
                b.u32((base + off) as u32);
            }
        }
        b.close();
        if matches!(t.media, Media::Audio(_)) {
            // AAC pre-roll: every access unit needs the previous one (sample group 'roll', -1).
            b.open_full(b"sgpd", 1, 0);
            b.bytes(b"roll");
            b.u32(2); // default_length
            b.u32(1);
            b.u16(0xFFFF); // roll_distance -1
            b.close();
            b.open_full(b"sbgp", 0, 0);
            b.bytes(b"roll");
            b.u32(1);
            b.u32(t.sizes.len() as u32);
            b.u32(1);
            b.close();
        }
        b.close(); // stbl
        b.close(); // minf
        b.close(); // mdia
        b.close(); // trak
    }

    fn bitrates(&self, t: &Track) -> (u32, u32, u32) {
        let secs = t.media_duration() as f64 / t.timescale.max(1) as f64;
        let total: u64 = t.sizes.iter().map(|&s| s as u64).sum();
        let avg = if secs > 0.0 { (total as f64 * 8.0 / secs) as u32 } else { 0 };
        let per_sec = ((t.timescale as f64 / t.delta.max(1) as f64).round() as usize).max(1);
        let mut max = 0u64;
        let mut win = 0u64;
        for (i, &s) in t.sizes.iter().enumerate() {
            win += s as u64;
            if i >= per_sec {
                win -= t.sizes[i - per_sec] as u64;
            }
            max = max.max(win);
        }
        let buffer = t.sizes.iter().copied().max().unwrap_or(0);
        (buffer, (max * 8).min(u32::MAX as u64) as u32, avg)
    }

    fn avc1(&self, b: &mut Boxes, t: &Track, v: &h264::TrackInfo, mov: bool) {
        b.open(if v.hevc { b"hvc1" } else { b"avc1" });
        b.zeros(6);
        b.u16(1); // data_reference_index
        b.u16(0);
        b.u16(0);
        b.zeros(12);
        b.u16(v.width as u16);
        b.u16(v.height as u16);
        b.u32(0x0048_0000);
        b.u32(0x0048_0000);
        b.u32(0);
        b.u16(1); // frame_count
        let name: &[u8] = if v.hevc { b"HEVC" } else { b"H.264" };
        let mut comp = [0u8; 32];
        comp[0] = name.len() as u8;
        comp[1..1 + name.len()].copy_from_slice(name);
        b.bytes(&comp);
        b.u16(0x0018);
        b.u16(0xFFFF);
        b.open(if v.hevc { b"hvcC" } else { b"avcC" });
        b.bytes(&v.avcc);
        b.close();
        colr(b, &v.color, mov);
        b.open(b"pasp");
        b.u32(1);
        b.u32(1);
        b.close();
        let (buffer, max, avg) = self.bitrates(t);
        b.open(b"btrt");
        b.u32(buffer);
        b.u32(max);
        b.u32(avg);
        b.close();
        b.close();
    }

    fn mp4a(&self, b: &mut Boxes, t: &Track, a: &aac::TrackInfo, mov: bool) {
        let (buffer, max, avg) = self.bitrates(t);
        b.open(b"mp4a");
        b.zeros(6);
        b.u16(1);
        b.u16(if mov { 1 } else { 0 }); // sound description version
        b.u16(0);
        b.u32(0);
        b.u16(a.channels);
        b.u16(16);
        b.u16(if mov { 0xFFFE } else { 0 }); // compression id
        b.u16(0);
        b.u32(a.sample_rate.min(65535) << 16);
        if mov {
            b.u32(aac::FRAME as u32); // samples per packet
            b.u32(0); // bytes per packet (variable)
            b.u32(0); // bytes per frame
            b.u32(2); // bytes per sample
            b.open(b"wave");
            b.open(b"frma");
            b.bytes(b"mp4a");
            b.close();
            b.open(b"mp4a");
            b.u32(0);
            b.close();
            esds(b, a, buffer, max, avg);
            b.u32(8); // terminator atom
            b.u32(0);
            b.close();
        } else {
            esds(b, a, buffer, max, avg);
        }
        b.close();
    }
}

impl Drop for Muxer {
    fn drop(&mut self) {
        if !self.done {
            drop(self.out.take());
            let _ = std::fs::remove_file(&self.data_path);
            if self.data_path != self.path {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

fn to_movie(ticks: u64, timescale: u32) -> u64 {
    (ticks as u128 * MOVIE_TIMESCALE as u128 / timescale.max(1) as u128) as u64
}

fn ftyp(brand: Brand) -> Vec<u8> {
    let mut b = Boxes::default();
    b.open(b"ftyp");
    match brand {
        Brand::Mp4 => {
            b.bytes(b"isom");
            b.u32(0x200);
            for c in [b"isom", b"iso2", b"avc1", b"mp41", b"mp42"] {
                b.bytes(c);
            }
        }
        Brand::Mov => {
            b.bytes(b"qt  ");
            b.u32(0x2005_0300);
            b.bytes(b"qt  ");
        }
    }
    b.close();
    b.buf
}

fn times(b: &mut Boxes, v1: bool, now: u64) {
    if v1 {
        b.u64(now);
        b.u64(now);
    } else {
        b.u32(now as u32);
        b.u32(now as u32);
    }
}

fn dur(b: &mut Boxes, v1: bool, d: u64) {
    if v1 {
        b.u64(d);
    } else {
        b.u32(d as u32);
    }
}

fn matrix(b: &mut Boxes) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        b.u32(v);
    }
}

fn hdlr(b: &mut Boxes, pre: &[u8; 4], handler: &[u8; 4], name: &str, pascal: bool) {
    b.open_full(b"hdlr", 0, 0);
    b.bytes(pre);
    b.bytes(handler);
    b.zeros(12);
    if pascal {
        b.u8(name.len() as u8);
        b.bytes(name.as_bytes());
    } else {
        b.bytes(name.as_bytes());
        b.u8(0);
    }
    b.close();
}

fn colr(b: &mut Boxes, c: &ColorDesc, mov: bool) {
    b.open(b"colr");
    b.bytes(if mov { b"nclc" } else { b"nclx" });
    b.u16(c.primaries as u16);
    b.u16(c.transfer as u16);
    b.u16(c.matrix as u16);
    if !mov {
        b.u8(if c.full_range { 0x80 } else { 0 });
    }
    b.close();
}

/// 3GPP timed-text sample entry (TS 26.245): bottom-centred white text on
/// a transparent background, one font.
fn tx3g(b: &mut Boxes, s: &SubtitleInfo) {
    b.open(b"tx3g");
    b.zeros(6);
    b.u16(1); // data_reference_index
    b.u32(0); // displayFlags
    b.u8(1); // horizontal justification: centre
    b.u8(0xFF); // vertical justification: bottom (-1)
    b.bytes(&[0, 0, 0, 0]); // background RGBA
    // Default text box: top, left, bottom, right.
    b.u16(0);
    b.u16(0);
    b.u16(s.height.min(0x7FFF) as u16);
    b.u16(s.width.min(0x7FFF) as u16);
    // Style record: chars 0..0, font 1, plain, size, white.
    b.u16(0);
    b.u16(0);
    b.u16(1);
    b.u8(0);
    b.u8(s.font_size);
    b.bytes(&[0xFF, 0xFF, 0xFF, 0xFF]);
    b.open(b"ftab");
    b.u16(1);
    b.u16(1);
    let font = b"Sans-Serif";
    b.u8(font.len() as u8);
    b.bytes(font);
    b.close();
    b.close();
}

/// MPEG-4 descriptor with a 4-byte expandable length.
fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len() as u32;
    let mut v = vec![tag, 0x80 | ((n >> 21) & 0x7F) as u8, 0x80 | ((n >> 14) & 0x7F) as u8, 0x80 | ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8];
    v.extend_from_slice(body);
    v
}

fn esds(b: &mut Boxes, a: &aac::TrackInfo, buffer: u32, max: u32, avg: u32) {
    let mut dc = vec![0x40, 0x15];
    dc.extend_from_slice(&buffer.min(0xFF_FFFF).to_be_bytes()[1..]);
    dc.extend_from_slice(&max.max(a.bitrate).to_be_bytes());
    dc.extend_from_slice(&avg.to_be_bytes());
    dc.extend_from_slice(&descriptor(0x05, &a.asc));
    let mut es = vec![0, 0, 0]; // ES_ID, flags
    es.extend_from_slice(&descriptor(0x04, &dc));
    es.extend_from_slice(&descriptor(0x06, &[0x02]));
    b.open_full(b"esds", 0, 0);
    b.bytes(&descriptor(0x03, &es));
    b.close();
}

/// Minimal nested box builder.
#[derive(Default)]
struct Boxes {
    buf: Vec<u8>,
    stack: Vec<usize>,
}

impl Boxes {
    fn open(&mut self, kind: &[u8; 4]) {
        self.stack.push(self.buf.len());
        self.buf.extend_from_slice(&[0, 0, 0, 0]);
        self.buf.extend_from_slice(kind);
    }
    fn open_full(&mut self, kind: &[u8; 4], version: u8, flags: u32) {
        self.open(kind);
        self.u32((version as u32) << 24 | (flags & 0xFF_FFFF));
    }
    fn close(&mut self) {
        let start = self.stack.pop().expect("open box");
        let size = (self.buf.len() - start) as u32;
        self.buf[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn zeros(&mut self, n: usize) {
        self.buf.resize(self.buf.len() + n, 0);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
}
