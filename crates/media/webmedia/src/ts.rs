// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MPEG-2 transport stream segments (HLS `.ts`): packets, PAT/PMT, PES with
//! timestamps; H.264 access units and ADTS AAC frames are collected into
//! tracks. Nothing is decoded.

use crate::error::{Error, Result};
use crate::track::{Sample, TrackData, IDENTITY};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub const CLOCK: i64 = 90_000;
const WRAP: i64 = 1 << 33;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    H264,
    Aac,
    /// A stream type Studio does not take apart (named for the message).
    Other(&'static str),
}

fn kind_of(stream_type: u8) -> Option<Kind> {
    Some(match stream_type {
        0x1B => Kind::H264,
        0x0F => Kind::Aac,
        0x24 => Kind::Other("H.265 video in a transport stream"),
        0x02 | 0x10 => Kind::Other("MPEG-2 or MPEG-4 Part 2 video in a transport stream"),
        0x03 | 0x04 => Kind::Other("MP3 audio in a transport stream"),
        0x11 => Kind::Other("LATM AAC audio in a transport stream"),
        0x81 | 0x87 => Kind::Other("AC-3 or E-AC-3 audio in a transport stream"),
        _ => return None,
    })
}

/// Where the demuxer hands what it finds.
pub trait Sink {
    fn video(&mut self, pts: Option<i64>, dts: Option<i64>, es: &[u8]);
    fn audio(&mut self, pts: Option<i64>, es: &[u8]);
    fn note(&mut self, text: &str);
}

struct Stream {
    kind: Kind,
    buf: Vec<u8>,
}

fn read_ts(b: &[u8]) -> i64 {
    (((b[0] as i64 >> 1) & 7) << 30) | ((b[1] as i64) << 22) | (((b[2] as i64) >> 1) << 15) | ((b[3] as i64) << 7) | ((b[4] as i64) >> 1)
}

fn flush(s: &mut Stream, sink: &mut dyn Sink) {
    let buf = std::mem::take(&mut s.buf);
    if buf.len() < 9 || buf[0] != 0 || buf[1] != 0 || buf[2] != 1 {
        return;
    }
    let flags = buf[7] >> 6;
    let start = 9 + buf[8] as usize;
    if start > buf.len() {
        return;
    }
    let pts = (flags & 2 != 0 && buf.len() >= 14).then(|| read_ts(&buf[9..14]));
    let dts = (flags == 3 && buf.len() >= 19).then(|| read_ts(&buf[14..19]));
    let es = &buf[start..];
    match s.kind {
        Kind::H264 => sink.video(pts, dts, es),
        Kind::Aac => sink.audio(pts, es),
        Kind::Other(_) => {}
    }
}

fn find_sync(data: &[u8]) -> Option<usize> {
    (0..188.min(data.len())).find(|&o| data[o] == 0x47 && (o + 188 >= data.len() || data[o + 188] == 0x47) && (o + 376 >= data.len() || data[o + 376] == 0x47))
}

/// Whether `data` looks like a transport stream.
pub fn looks_like_ts(data: &[u8]) -> bool {
    data.len() >= 188 && data[0] == 0x47 && (data.len() < 376 || data[188] == 0x47)
}

pub fn demux(data: &[u8], sink: &mut dyn Sink) -> Result<()> {
    let start = find_sync(data).ok_or_else(|| Error::Bad("a segment is not a transport stream".into()))?;
    let mut pmt_pids: Vec<u16> = Vec::new();
    let mut streams: BTreeMap<u16, Stream> = BTreeMap::new();
    let mut noted: Vec<&'static str> = Vec::new();
    for pkt in data[start..].chunks_exact(188) {
        if pkt[0] != 0x47 || pkt[1] & 0x80 != 0 {
            continue;
        }
        let pusi = pkt[1] & 0x40 != 0;
        let pid = (((pkt[1] & 0x1F) as u16) << 8) | pkt[2] as u16;
        let afc = (pkt[3] >> 4) & 3;
        if afc & 1 == 0 {
            continue;
        }
        let mut off = 4;
        if afc & 2 != 0 {
            off += 1 + pkt[4] as usize;
        }
        if off >= 188 {
            continue;
        }
        let payload = &pkt[off..];
        if pid == 0 {
            if pusi && payload.len() > 8 {
                let p = 1 + payload[0] as usize;
                if p + 8 <= payload.len() && payload[p] == 0 {
                    let len = (((payload[p + 1] & 0x0F) as usize) << 8) | payload[p + 2] as usize;
                    let end = (p + 3 + len).min(payload.len());
                    let mut i = p + 8;
                    while i + 4 <= end.saturating_sub(4) {
                        let prog = ((payload[i] as u16) << 8) | payload[i + 1] as u16;
                        let ppid = (((payload[i + 2] & 0x1F) as u16) << 8) | payload[i + 3] as u16;
                        if prog != 0 && !pmt_pids.contains(&ppid) {
                            pmt_pids.push(ppid);
                        }
                        i += 4;
                    }
                }
            }
        } else if pmt_pids.contains(&pid) {
            if pusi && payload.len() > 12 {
                let p = 1 + payload[0] as usize;
                if p + 12 <= payload.len() && payload[p] == 2 {
                    let len = (((payload[p + 1] & 0x0F) as usize) << 8) | payload[p + 2] as usize;
                    let end = (p + 3 + len).min(payload.len()).saturating_sub(4);
                    let info = (((payload[p + 10] & 0x0F) as usize) << 8) | payload[p + 11] as usize;
                    let mut i = p + 12 + info;
                    while i + 5 <= end {
                        let st = payload[i];
                        let epid = (((payload[i + 1] & 0x1F) as u16) << 8) | payload[i + 2] as u16;
                        let es_info = (((payload[i + 3] & 0x0F) as usize) << 8) | payload[i + 4] as usize;
                        if let Some(kind) = kind_of(st) {
                            streams.entry(epid).or_insert(Stream { kind, buf: Vec::new() });
                            if let Kind::Other(d) = kind {
                                if !noted.contains(&d) {
                                    noted.push(d);
                                }
                            }
                        }
                        i += 5 + es_info;
                    }
                }
            }
        } else if let Some(s) = streams.get_mut(&pid) {
            if pusi {
                flush(s, sink);
            }
            if pusi || !s.buf.is_empty() {
                s.buf.extend_from_slice(payload);
            }
        }
    }
    for s in streams.values_mut() {
        flush(s, sink);
    }
    for d in noted {
        sink.note(d);
    }
    Ok(())
}

// ---- collecting tracks ----

struct Video {
    sps: Vec<u8>,
    pps: Vec<Vec<u8>>,
    out: BufWriter<File>,
    written: u64,
    /// offset, size, dts, pts, sync (dts and pts in 90 kHz, unwrapped and shifted)
    samples: Vec<(u64, u32, i64, i64, bool)>,
    wraps: i64,
    last_raw: Option<i64>,
}

struct Audio {
    asc: [u8; 2],
    rate: u32,
    channels: u16,
    out: BufWriter<File>,
    written: u64,
    frames: Vec<(u64, u32)>,
    /// 90 kHz time of the first frame.
    start: Option<i64>,
    wraps: i64,
    last_raw: Option<i64>,
}

/// Gathers the tracks of a run of transport stream segments. Payloads go to
/// files in `dir`, so memory stays small however long the stream is.
pub struct Collector {
    dir: PathBuf,
    tag: String,
    video: Option<Video>,
    audio: Option<Audio>,
    shift: i64,
    rebase: bool,
    pub notes: Vec<String>,
    error: Option<Error>,
    ignored_video: bool,
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

impl Collector {
    pub fn new(dir: &Path, tag: &str) -> Collector {
        Collector { dir: dir.to_path_buf(), tag: tag.to_owned(), video: None, audio: None, shift: 0, rebase: false, notes: Vec::new(), error: None, ignored_video: false }
    }

    /// Call before each segment; `discontinuity` is the playlist's flag.
    pub fn begin_segment(&mut self, discontinuity: bool) {
        if discontinuity && (self.video.is_some() || self.audio.is_some()) {
            self.rebase = true;
            if let Some(v) = &mut self.video {
                v.last_raw = None;
            }
            if let Some(a) = &mut self.audio {
                a.last_raw = None;
            }
        }
    }

    /// A segment of packed audio: ADTS frames behind an ID3 tag that may
    /// carry the 90 kHz start time.
    pub fn push_packed_audio(&mut self, data: &[u8]) -> Result<()> {
        let mut pts = None;
        if data.starts_with(b"ID3") && data.len() >= 10 {
            let size = ((data[6] as usize & 0x7F) << 21) | ((data[7] as usize & 0x7F) << 14) | ((data[8] as usize & 0x7F) << 7) | (data[9] as usize & 0x7F);
            let tag = &data[10..(10 + size).min(data.len())];
            let owner = b"com.apple.streaming.transportStreamTimestamp\0";
            if let Some(at) = tag.windows(owner.len()).position(|w| w == owner) {
                if let Some(b) = tag.get(at + owner.len()..at + owner.len() + 8) {
                    pts = Some((u64::from_be_bytes(b.try_into().unwrap()) & ((1 << 33) - 1)) as i64);
                }
            }
        }
        self.audio(pts, data);
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    pub fn push_segment(&mut self, data: &[u8]) -> Result<()> {
        demux(data, self)?;
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn unwrap_ts(raw: i64, last: &mut Option<i64>, wraps: &mut i64) -> i64 {
        if let Some(l) = *last {
            if raw + WRAP / 2 < l {
                *wraps += 1;
            }
        }
        *last = Some(raw);
        raw + *wraps * WRAP
    }

    fn video_end(&self) -> Option<i64> {
        let v = self.video.as_ref()?;
        let n = v.samples.len();
        let last = v.samples.last()?;
        let step = if n >= 2 { (last.2 - v.samples[n - 2].2).max(1) } else { 3003 };
        Some(last.2 + step)
    }

    fn audio_end(&self) -> Option<i64> {
        let a = self.audio.as_ref()?;
        Some(a.start? + a.frames.len() as i64 * 1024 * CLOCK / a.rate.max(1) as i64)
    }

    /// Finishes the files and returns the tracks.
    pub fn finish(mut self) -> Result<Vec<TrackData>> {
        let mut tracks = Vec::new();
        if let Some(mut v) = self.video.take() {
            v.out.flush()?;
            let path = self.dir.join(format!("{}-video.es", self.tag));
            let (w, h) = crate::mp4w::h264_size(&v.sps).ok_or_else(|| Error::Bad("the video's parameter sets cannot be read".into()))?;
            let entry = crate::mp4w::avc1_entry(&v.sps, &v.pps, w, h).ok_or_else(|| Error::Bad("the video's parameter sets cannot be read".into()))?;
            let n = v.samples.len();
            let mut samples = Vec::with_capacity(n);
            for i in 0..n {
                let (off, size, dts, pts, sync) = v.samples[i];
                let dur = if i + 1 < n { (v.samples[i + 1].2 - dts).max(1) } else if i > 0 { (dts - v.samples[i - 1].2).max(1) } else { 3003 };
                samples.push(Sample { offset: off, size, dts, pts, dur: dur.min(u32::MAX as i64) as u32, sync: sync || i == 0 });
            }
            tracks.push(TrackData { handler: *b"vide", timescale: CLOCK as u32, entry, width: w, height: h, lang: "und".into(), matrix: IDENTITY, edit_offset: 0, samples, store: path });
        }
        if let Some(mut a) = self.audio.take() {
            a.out.flush()?;
            let path = self.dir.join(format!("{}-audio.es", self.tag));
            let entry = crate::mp4w::mp4a_entry(&a.asc, a.channels, a.rate);
            let start = a.start.unwrap_or(0);
            let start_ticks = start * a.rate as i64 / CLOCK;
            let samples = a.frames.iter().enumerate().map(|(i, &(off, size))| Sample { offset: off, size, dts: start_ticks + i as i64 * 1024, pts: start_ticks + i as i64 * 1024, dur: 1024, sync: true }).collect();
            tracks.push(TrackData { handler: *b"soun", timescale: a.rate, entry, width: 0, height: 0, lang: "und".into(), matrix: IDENTITY, edit_offset: 0, samples, store: path });
        }
        // The clocks of audio and video are one clock (90 kHz): the video
        // track's times are already in it; audio was converted above.
        if tracks.is_empty() {
            return Err(Error::Unsupported(match self.notes.first() {
                Some(n) => format!("The stream carries {n}, which Zornov cannot take apart yet."),
                None => "The segments hold no video or audio Zornov can read.".into(),
            }));
        }
        Ok(tracks)
    }
}

fn has_recovery_point(sei: &[u8]) -> bool {
    // First SEI message type 6 = recovery point.
    sei.len() > 1 && sei[1] == 6
}

impl Sink for Collector {
    fn video(&mut self, pts: Option<i64>, dts: Option<i64>, es: &[u8]) {
        if self.error.is_some() {
            return;
        }
        let (Some(raw_dts), raw_pts) = (dts.or(pts), pts.or(dts)) else { return };
        let raw_pts = raw_pts.unwrap_or(raw_dts);
        if self.video.is_none() {
            let path = self.dir.join(format!("{}-video.es", self.tag));
            match File::create(&path) {
                Ok(f) => self.video = Some(Video { sps: Vec::new(), pps: Vec::new(), out: BufWriter::new(f), written: 0, samples: Vec::new(), wraps: 0, last_raw: None }),
                Err(e) => {
                    self.error = Some(e.into());
                    return;
                }
            }
        }
        // Parameter sets and slices.
        let mut au = Vec::with_capacity(es.len() + 16);
        let mut sync = false;
        let mut slices = 0;
        {
            let v = self.video.as_mut().unwrap();
            for nal in studio_h264::split_annex_b(es) {
                match nal[0] & 0x1F {
                    7 => {
                        if v.sps.is_empty() {
                            v.sps = nal.to_vec();
                        } else if v.sps != nal && !self.ignored_video {
                            self.ignored_video = true;
                            self.notes.push("the video's settings change partway; the first ones are kept".into());
                        }
                    }
                    8 => {
                        if !v.pps.iter().any(|p| p == nal) && v.pps.len() < 8 {
                            v.pps.push(nal.to_vec());
                        }
                    }
                    1 | 5 | 6 => {
                        if nal[0] & 0x1F == 5 {
                            sync = true;
                        }
                        if nal[0] & 0x1F == 6 && has_recovery_point(&nal[1..]) {
                            sync = true;
                        }
                        if nal[0] & 0x1F != 6 {
                            slices += 1;
                        }
                        au.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        au.extend_from_slice(nal);
                    }
                    _ => {}
                }
            }
        }
        if slices == 0 {
            return;
        }
        if self.rebase {
            // The first sample after a discontinuity continues where the last one ended.
            let end = self.video_end().or_else(|| self.audio_end());
            if let Some(end) = end {
                let v = self.video.as_ref().unwrap();
                let wraps = if v.last_raw.is_some() { v.wraps } else { 0 };
                self.shift = end - (raw_dts + wraps * WRAP);
            }
            self.rebase = false;
        }
        let shift = self.shift;
        let v = self.video.as_mut().unwrap();
        let mut last = v.last_raw;
        let mut wraps = v.wraps;
        let dts = Self::unwrap_ts(raw_dts, &mut last, &mut wraps);
        let pts_u = dts + (raw_pts - raw_dts);
        v.last_raw = last;
        v.wraps = wraps;
        let off = v.written;
        if let Err(e) = v.out.write_all(&au) {
            self.error = Some(e.into());
            return;
        }
        v.written += au.len() as u64;
        v.samples.push((off, au.len() as u32, dts + shift, pts_u + shift, sync));
    }

    fn audio(&mut self, pts: Option<i64>, es: &[u8]) {
        if self.error.is_some() {
            return;
        }
        let mut i = 0;
        // Leading ID3 tags (packed audio).
        while es.len() >= i + 10 && &es[i..i + 3] == b"ID3" {
            let size = ((es[i + 6] as usize & 0x7F) << 21) | ((es[i + 7] as usize & 0x7F) << 14) | ((es[i + 8] as usize & 0x7F) << 7) | (es[i + 9] as usize & 0x7F);
            i += 10 + size;
        }
        let mut first = true;
        while i + 7 <= es.len() {
            if es[i] != 0xFF || es[i + 1] & 0xF0 != 0xF0 {
                i += 1;
                continue;
            }
            let hdr = if es[i + 1] & 1 == 0 { 9 } else { 7 };
            let profile = (es[i + 2] >> 6) & 3;
            let sf = ((es[i + 2] >> 2) & 0xF) as usize;
            let chan = (((es[i + 2] & 1) << 2) | (es[i + 3] >> 6)) as u16;
            let len = (((es[i + 3] & 3) as usize) << 11) | ((es[i + 4] as usize) << 3) | (es[i + 5] as usize >> 5);
            if len < hdr || i + len > es.len() || sf >= AAC_RATES.len() || chan == 0 || chan > 7 {
                i += 1;
                continue;
            }
            if self.audio.is_none() {
                let path = self.dir.join(format!("{}-audio.es", self.tag));
                let f = match File::create(&path) {
                    Ok(f) => f,
                    Err(e) => {
                        self.error = Some(e.into());
                        return;
                    }
                };
                let aot = profile + 1;
                let asc = [(aot << 3) | (sf as u8 >> 1), ((sf as u8 & 1) << 7) | ((chan as u8) << 3)];
                self.audio = Some(Audio { asc, rate: AAC_RATES[sf], channels: if chan == 7 { 8 } else { chan }, out: BufWriter::new(f), written: 0, frames: Vec::new(), start: None, wraps: 0, last_raw: None });
            }
            if first {
                first = false;
                if let Some(raw) = pts {
                    let rebase = self.rebase;
                    let end = if rebase { self.video_end().or_else(|| self.audio_end()) } else { None };
                    let a = self.audio.as_mut().unwrap();
                    let mut last = a.last_raw;
                    let mut wraps = a.wraps;
                    let t = Self::unwrap_ts(raw, &mut last, &mut wraps);
                    a.last_raw = last;
                    a.wraps = wraps;
                    if rebase {
                        if let Some(end) = end {
                            self.shift = end - t;
                        }
                        self.rebase = false;
                    }
                    if self.audio.as_ref().unwrap().start.is_none() {
                        self.audio.as_mut().unwrap().start = Some(t + self.shift);
                    }
                }
            }
            let a = self.audio.as_mut().unwrap();
            let raw = &es[i + hdr..i + len];
            let off = a.written;
            if let Err(e) = a.out.write_all(raw) {
                self.error = Some(e.into());
                return;
            }
            a.written += raw.len() as u64;
            a.frames.push((off, raw.len() as u32));
            i += len;
        }
    }

    fn note(&mut self, text: &str) {
        let t = text.to_owned();
        if !self.notes.contains(&t) {
            self.notes.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Seen {
        video: Vec<(Option<i64>, Option<i64>, usize)>,
        audio: Vec<(Option<i64>, usize)>,
    }

    impl Sink for Seen {
        fn video(&mut self, pts: Option<i64>, dts: Option<i64>, es: &[u8]) {
            self.video.push((pts, dts, es.len()));
        }
        fn audio(&mut self, pts: Option<i64>, es: &[u8]) {
            self.audio.push((pts, es.len()));
        }
        fn note(&mut self, _: &str) {}
    }

    fn stamp(prefix: u8, v: i64) -> [u8; 5] {
        [prefix | (((v >> 30) & 7) as u8) << 1 | 1, (v >> 22) as u8, ((((v >> 15) & 0x7F) as u8) << 1) | 1, (v >> 7) as u8, (((v & 0x7F) as u8) << 1) | 1]
    }

    /// Pictures shown out of decode order carry both times; one with only a
    /// PTS has no DTS; times past 2^32 survive.
    #[test]
    fn pes_times_are_read_whole() {
        let big = 0x1_2345_6789i64;
        let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0xC0, 10];
        pes.extend_from_slice(&stamp(0x30, big + 6000));
        pes.extend_from_slice(&stamp(0x10, big));
        pes.extend_from_slice(&[1, 2, 3, 4]);
        let mut s = Stream { kind: Kind::H264, buf: pes };
        let mut seen = Seen::default();
        flush(&mut s, &mut seen);
        assert_eq!(seen.video, [(Some(big + 6000), Some(big), 4)]);
        let mut pes = vec![0, 0, 1, 0xC0, 0, 0, 0x80, 0x80, 5];
        pes.extend_from_slice(&stamp(0x20, 90_000));
        pes.extend_from_slice(&[9; 7]);
        let mut s = Stream { kind: Kind::Aac, buf: pes };
        flush(&mut s, &mut seen);
        assert_eq!(seen.audio, [(Some(90_000), 7)]);
        // Garbage is not a PES packet.
        let mut s = Stream { kind: Kind::H264, buf: vec![1, 2, 3] };
        flush(&mut s, &mut seen);
        assert_eq!(seen.video.len(), 1);
    }

    /// The 33-bit clock wraps after about 26 hours: the track goes on rising.
    #[test]
    fn the_clock_wrap_is_unwrapped() {
        let mut last = None;
        let mut wraps = 0;
        let a = Collector::unwrap_ts(WRAP - 3000, &mut last, &mut wraps);
        let b = Collector::unwrap_ts(1500, &mut last, &mut wraps);
        assert_eq!(b - a, 4500);
    }
}
