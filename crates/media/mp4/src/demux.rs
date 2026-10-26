// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The public demuxer: file scanning, track assembly, edit-list timing and
//! seeking helpers.
//!
//! Timing model: [`Sample::dts`] / [`Sample::pts`] are raw media times in the
//! track timescale. The edit list maps media time onto the presentation
//! timeline exactly like players do; for the common single-edit case the
//! presented time is simply `pts - edit_offset`. Samples that fall outside
//! every edit (encoder delay, AAC priming) are decoded but not presented.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::r#box::{find, read_top_header, require, Boxes, FourCC, TopBox};
use crate::boxes::{self, Trex};
use crate::codec::{AvcConfig, HevcConfig};
use crate::error::{Error, Result};
use crate::fragment::{parse_moof, FragTarget};
use crate::stbl::SampleTable;
use crate::stsd::parse_stsd;

/// Largest metadata box (`moov`, `moof`) the demuxer will load.
const MAX_META_BOX: u64 = 512 << 20;

/// What a track carries, from the media handler type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Other,
}

/// Codec and decoder configuration of a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    /// H.264 (`avc1` / `avc3`).
    Avc { config: AvcConfig },
    /// H.265 (`hvc1` / `hev1`).
    Hevc { config: HevcConfig },
    /// AAC from `mp4a` + `esds`. `object_type` is 2 for LC, 5 for HE (SBR),
    /// 29 for HE v2 (PS); `sample_rate` / `channels` describe the decoded
    /// output (SBR doubles the core rate, PS upmixes mono to stereo).
    /// See [`crate::AudioSpecificConfig`] for the full parse of `asc`.
    Aac { asc: Vec<u8>, object_type: u8, sample_rate: u32, channels: u16 },
    /// Anything else, identified by its sample entry four-cc.
    Other(FourCC),
}

impl Codec {
    /// NAL length prefix size for AVC / HEVC.
    pub fn nal_length_size(&self) -> Option<u8> {
        match self {
            Codec::Avc { config } => Some(config.nal_length_size),
            Codec::Hevc { config } => Some(config.nal_length_size),
            _ => None,
        }
    }

    /// Parameter sets as Annex-B, for AVC / HEVC.
    pub fn annex_b_parameter_sets(&self) -> Option<Vec<u8>> {
        match self {
            Codec::Avc { config } => Some(config.annex_b_parameter_sets()),
            Codec::Hevc { config } => Some(config.annex_b_parameter_sets()),
            _ => None,
        }
    }
}

/// One access unit / audio frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Absolute file offset of the sample data.
    pub offset: u64,
    pub size: u32,
    /// Decode timestamp in track timescale (media time).
    pub dts: i64,
    /// Composition timestamp in track timescale (media time, before edits).
    pub pts: i64,
    /// Duration in track timescale.
    pub duration: u32,
    /// Random access point (`stss`, or fragment sample flags).
    pub sync: bool,
}

/// One `elst` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    /// Segment length in movie timescale (0 in fragmented files = "all").
    pub segment_duration: u64,
    /// Media start time in track timescale, or -1 for an empty edit.
    pub media_time: i64,
    /// Playback rate, 16.16 fixed point (0 = dwell).
    pub rate: i32,
}

/// `btrt` (or `esds` DecoderConfigDescriptor) bit rates, in bits per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bitrate {
    pub buffer_size: u32,
    pub max: u32,
    pub avg: u32,
}

/// `colr` contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorInfo {
    /// `nclx` (ISO) or `nclc` (QuickTime; `full_range` always false).
    Nclx { primaries: u16, transfer: u16, matrix: u16, full_range: bool },
    /// ICC profile (`rICC` / `prof`).
    Icc(Vec<u8>),
}

/// QuickTime `chan` channel layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelLayout {
    pub tag: u32,
    pub bitmap: u32,
    /// Channel labels of explicit channel descriptions.
    pub labels: Vec<u32>,
}

/// One track with its fully expanded sample table.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: u32,
    pub kind: TrackKind,
    /// Media handler type (`vide`, `soun`, `text`, ...).
    pub handler: FourCC,
    /// Sample entry type (`avc1`, `hvc1`, `mp4a`, ...).
    pub fourcc: FourCC,
    pub timescale: u32,
    /// Movie timescale, the unit of [`Edit::segment_duration`].
    pub movie_timescale: u32,
    pub codec: Codec,
    /// Display size from `tkhd` (falls back to the coded size).
    pub width: u32,
    pub height: u32,
    /// Size stored in the visual sample entry.
    pub coded_width: u32,
    pub coded_height: u32,
    /// Clockwise display rotation: 0, 90, 180 or 270.
    pub rotation: u16,
    /// Horizontal mirror applied before `rotation`.
    pub flip: bool,
    /// Raw `tkhd` matrix (a, b, u, c, d, v, x, y, w).
    pub matrix: [i32; 9],
    pub pixel_aspect: Option<(u32, u32)>,
    pub color: Option<ColorInfo>,
    pub bitrate: Option<Bitrate>,
    /// Sound sample entry fields (0 for non-audio tracks).
    pub sample_rate: u32,
    pub channels: u16,
    /// Audio sample size, or visual depth.
    pub bits_per_sample: u16,
    pub channel_layout: Option<ChannelLayout>,
    /// Samples in decode order.
    pub samples: Vec<Sample>,
    /// `sdtp` byte per sample, or empty when not signalled.
    pub sdtp: Vec<u8>,
    /// Edit list (from `elst`, or synthesized from iTunes gapless info).
    pub edits: Vec<Edit>,
    /// Media time (track timescale) shown at presentation time 0 under the
    /// first edit: `presented = pts - edit_offset`. Negative when the track
    /// starts with an empty edit (delayed start).
    pub edit_offset: i64,
    /// Media duration in track timescale.
    pub media_duration: u64,
    /// Presented duration in seconds, honouring the edit list.
    pub duration_secs: f64,
    /// ISO-639-2/T language code (`und` if unknown).
    pub language: String,
}

/// Seek result: decode from `sync_index` up to `target_index` (both indices
/// into [`Track::samples`]) to present `target_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekPoint {
    pub sync_index: usize,
    pub target_index: usize,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    pres_start: i64,
    /// `None` for an empty edit.
    media_start: Option<i64>,
    /// `None` = unbounded.
    len: Option<i64>,
    dwell: bool,
}

impl Track {
    fn to_media(&self, movie_units: u64) -> i64 {
        let v = movie_units as i128 * self.timescale as i128 / self.movie_timescale.max(1) as i128;
        v.min(i64::MAX as i128) as i64
    }

    /// Edit list as segments in track timescale.
    fn segments(&self) -> Vec<Segment> {
        let mut out = Vec::with_capacity(self.edits.len());
        let mut pres = 0i64;
        for (i, e) in self.edits.iter().enumerate() {
            let len = self.to_media(e.segment_duration);
            let media_start = (e.media_time >= 0).then_some(e.media_time);
            let unbounded = e.segment_duration == 0 && media_start.is_some()
                && i + 1 == self.edits.len();
            out.push(Segment {
                pres_start: pres,
                media_start,
                len: if unbounded { None } else { Some(len) },
                dwell: e.rate == 0,
            });
            pres = pres.saturating_add(len);
        }
        out
    }

    /// Maps a media time (e.g. [`Sample::pts`]) to presentation time in track
    /// timescale; `None` when no edit presents it.
    pub fn media_to_presentation(&self, media_time: i64) -> Option<i64> {
        if self.edits.is_empty() {
            return Some(media_time);
        }
        for s in self.segments() {
            let Some(m) = s.media_start else { continue };
            if s.dwell {
                if media_time == m {
                    return Some(s.pres_start);
                }
            } else if media_time >= m && s.len.is_none_or(|l| media_time - m < l) {
                return Some(s.pres_start.saturating_add(media_time - m));
            }
        }
        None
    }

    /// Maps presentation time (track timescale) to the media time to show.
    /// Times inside an empty edit map to the start of the next real edit;
    /// times past the end clamp to the last presented instant.
    pub fn presentation_to_media(&self, t: i64) -> Option<i64> {
        if self.edits.is_empty() {
            return Some(t);
        }
        let segs = self.segments();
        let mut last = None;
        for s in &segs {
            let Some(m) = s.media_start else { continue };
            let end = s.len.map(|l| s.pres_start.saturating_add(l));
            if end.is_none_or(|e| t < e) {
                let into = if s.dwell { 0 } else { (t - s.pres_start).max(0) };
                return Some(m.saturating_add(into));
            }
            if s.len.is_some_and(|l| l > 0) {
                last = Some(m.saturating_add(if s.dwell { 0 } else { s.len.unwrap_or(1) - 1 }));
            }
        }
        last
    }

    /// Presentation time of a sample in seconds, if it is presented at all.
    pub fn presentation_secs(&self, sample: &Sample) -> Option<f64> {
        self.media_to_presentation(sample.pts).map(|t| t as f64 / self.timescale as f64)
    }

    /// Number of samples the edit list actually presents.
    pub fn presented_samples(&self) -> usize {
        self.samples.iter().filter(|s| self.media_to_presentation(s.pts).is_some()).count()
    }

    /// Sample indices sorted by presentation (composition) time.
    pub fn presentation_order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.samples.len()).collect();
        idx.sort_by_key(|&i| (self.samples[i].pts, i));
        idx
    }

    /// Indices of sync samples.
    pub fn sync_samples(&self) -> Vec<usize> {
        (0..self.samples.len()).filter(|&i| self.samples[i].sync).collect()
    }

    /// The sample presented at `secs` and the sync sample to start decoding
    /// from. See [`sample_for_time`].
    pub fn sample_for_time(&self, secs: f64) -> Option<SeekPoint> {
        let first = self.samples.iter().min_by_key(|s| s.pts)?;
        let secs = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
        let t = (secs * self.timescale as f64).round().min(i64::MAX as f64 / 2.0) as i64;
        let m = self.presentation_to_media(t).unwrap_or(first.pts);

        let mut target: Option<usize> = None;
        for (i, s) in self.samples.iter().enumerate() {
            if s.pts <= m && target.is_none_or(|j| s.pts > self.samples[j].pts) {
                target = Some(i);
            }
        }
        let target = target.unwrap_or_else(|| {
            (0..self.samples.len()).min_by_key(|&i| self.samples[i].pts).unwrap_or(0)
        });
        let tpts = self.samples[target].pts;
        // Latest sync sample that precedes the target in both decode and
        // presentation order (safe for open-GOP leading pictures).
        let sync = (0..=target)
            .rev()
            .find(|&i| self.samples[i].sync && self.samples[i].pts <= tpts)
            .or_else(|| (0..=target).rev().find(|&i| self.samples[i].sync))
            .unwrap_or(0);
        Some(SeekPoint { sync_index: sync, target_index: target })
    }
}

/// Free-function form of [`Track::sample_for_time`]: the sample presented at
/// `secs` (presentation timeline, edit list applied) and the sync sample to
/// start decoding from. `None` for an empty track.
pub fn sample_for_time(track: &Track, secs: f64) -> Option<SeekPoint> {
    track.sample_for_time(secs)
}

/// Movie-level description.
#[derive(Debug, Clone)]
pub struct Movie {
    pub tracks: Vec<Track>,
    /// Presented duration (longest track, edit lists applied).
    pub duration_secs: f64,
    /// `ftyp` major brand (zeros when the file has no `ftyp`).
    pub brand: FourCC,
    pub compatible_brands: Vec<FourCC>,
    pub timescale: u32,
    /// True when samples came from `moof` fragments.
    pub fragmented: bool,
}

impl Movie {
    /// First track of the given kind.
    pub fn track(&self, kind: TrackKind) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == kind)
    }

    pub fn video(&self) -> Option<&Track> {
        self.track(TrackKind::Video)
    }

    pub fn audio(&self) -> Option<&Track> {
        self.track(TrackKind::Audio)
    }
}

/// An opened file: parsed metadata plus the reader for sample data.
#[derive(Debug)]
pub struct Demuxer<R = BufReader<File>> {
    reader: R,
    movie: Movie,
    file_len: u64,
}

/// Opens and parses a file from disk.
pub fn open(path: impl AsRef<Path>) -> Result<Demuxer> {
    Demuxer::new(BufReader::new(File::open(path)?))
}

impl<R: Read + Seek> Demuxer<R> {
    /// Parses all metadata from `reader`; media data is not read.
    pub fn new(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let mut pos = 0;
        let mut ftyp = None;
        let mut moov = None;
        let mut moofs: Vec<TopBox> = Vec::new();
        while let Some(b) = read_top_header(&mut reader, pos, file_len)? {
            match &b.kind {
                b"ftyp" if ftyp.is_none() => {
                    ftyp = Some(boxes::parse_ftyp(&read_payload(&mut reader, &b, 1 << 20)?)?)
                }
                b"moov" => {
                    if moov.is_some() {
                        return Err(Error::malformed("more than one moov"));
                    }
                    moov = Some(read_payload(&mut reader, &b, MAX_META_BOX)?);
                }
                b"moof" => moofs.push(b),
                _ => {}
            }
            pos = b.end;
        }
        let moov = moov.ok_or(Error::Missing("moov"))?;
        let mut parsed = parse_moov(&moov, file_len)?;
        drop(moov);

        for m in &moofs {
            let data = read_payload(&mut reader, m, MAX_META_BOX)?;
            let mut targets: Vec<FragTarget<'_>> = parsed
                .tracks
                .iter_mut()
                .map(|t| FragTarget {
                    track_id: t.id,
                    defaults: parsed
                        .trex
                        .iter()
                        .find(|x| x.track_id == t.id)
                        .copied()
                        .unwrap_or(Trex { track_id: t.id, ..Trex::default() }),
                    samples: &mut t.samples,
                    sdtp: &mut t.sdtp,
                })
                .collect();
            parse_moof(&data, m.start, file_len, &mut targets)?;
        }

        for t in &mut parsed.tracks {
            finish_track(t);
        }
        let longest = parsed.tracks.iter().map(|t| t.duration_secs).fold(0.0, f64::max);
        let header = parsed.duration.max(parsed.fragment_duration) as f64 / parsed.timescale as f64;
        let (brand, compatible_brands) = ftyp.unwrap_or(([0; 4], Vec::new()));
        let movie = Movie {
            duration_secs: if longest > 0.0 { longest } else { header },
            tracks: parsed.tracks,
            brand,
            compatible_brands,
            timescale: parsed.timescale,
            fragmented: !moofs.is_empty(),
        };
        Ok(Demuxer { reader, movie, file_len })
    }

    pub fn movie(&self) -> &Movie {
        &self.movie
    }

    pub fn into_movie(self) -> Movie {
        self.movie
    }

    /// Reads one sample's bytes (still length-prefixed for AVC / HEVC).
    pub fn read_sample(&mut self, sample: &Sample) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.read_sample_into(sample, &mut buf)?;
        Ok(buf)
    }

    /// Like [`Demuxer::read_sample`] but reuses `buf`.
    pub fn read_sample_into(&mut self, sample: &Sample, buf: &mut Vec<u8>) -> Result<()> {
        if sample.offset.checked_add(sample.size as u64).is_none_or(|e| e > self.file_len) {
            return Err(Error::malformed("sample lies outside the file"));
        }
        buf.clear();
        buf.resize(sample.size as usize, 0);
        self.reader.seek(SeekFrom::Start(sample.offset))?;
        self.reader.read_exact(buf)?;
        Ok(())
    }
}

fn read_payload<R: Read + Seek>(r: &mut R, b: &TopBox, limit: u64) -> Result<Vec<u8>> {
    let len = b.payload_len();
    if len > limit {
        return Err(Error::TooLarge("metadata box"));
    }
    r.seek(SeekFrom::Start(b.payload))?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

struct ParsedMoov {
    timescale: u32,
    duration: u64,
    fragment_duration: u64,
    tracks: Vec<Track>,
    trex: Vec<Trex>,
}

fn parse_moov(moov: &[u8], file_len: u64) -> Result<ParsedMoov> {
    let (timescale, duration) = boxes::parse_mvhd(require(moov, b"mvhd", "mvhd")?)?;
    let mut tracks = Vec::new();
    let mut trex = Vec::new();
    let mut fragment_duration = 0;
    let mut gapless = None;
    for b in Boxes::new(moov) {
        let b = b?;
        match &b.kind {
            b"trak" => tracks.push(parse_trak(b.data, timescale, file_len)?),
            b"mvex" => {
                for m in Boxes::new(b.data) {
                    let m = m?;
                    match &m.kind {
                        b"trex" => trex.push(boxes::parse_trex(m.data)?),
                        b"mehd" => fragment_duration = boxes::parse_mehd(m.data)?,
                        _ => {}
                    }
                }
            }
            // Metadata is optional: a damaged udta never fails the file.
            b"udta" => gapless = itunes_gapless(b.data).ok().flatten(),
            _ => {}
        }
    }
    if let Some((priming, total)) = gapless {
        for t in tracks.iter_mut().filter(|t| t.kind == TrackKind::Audio && t.edits.is_empty()) {
            let dur = total as u128 * timescale as u128 / t.timescale as u128;
            t.edits.push(Edit {
                segment_duration: dur.min(u64::MAX as u128) as u64,
                media_time: priming as i64,
                rate: 0x10000,
            });
        }
    }
    Ok(ParsedMoov { timescale, duration, fragment_duration, tracks, trex })
}

fn parse_trak(trak: &[u8], movie_timescale: u32, file_len: u64) -> Result<Track> {
    let tkhd = boxes::parse_tkhd(require(trak, b"tkhd", "tkhd")?)?;
    let edits = match find(trak, b"edts")? {
        Some(edts) => match find(edts, b"elst")? {
            Some(elst) => boxes::parse_elst(elst)?,
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    let mdia = require(trak, b"mdia", "mdia")?;
    let (timescale, media_duration, language) = boxes::parse_mdhd(require(mdia, b"mdhd", "mdhd")?)?;
    let handler = boxes::parse_hdlr(require(mdia, b"hdlr", "hdlr")?)?;
    let kind = match &handler {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        _ => TrackKind::Other,
    };
    let minf = require(mdia, b"minf", "minf")?;
    let stbl = require(minf, b"stbl", "stbl")?;
    let entry = parse_stsd(require(stbl, b"stsd", "stsd")?, kind)?;
    let table = SampleTable::parse(stbl, file_len)?;
    let samples = table.samples(file_len)?;
    let mut sdtp = table.sdtp;
    if sdtp.len() >= samples.len() {
        sdtp.truncate(samples.len());
    } else {
        sdtp.clear();
    }
    let (rotation, flip) = boxes::matrix_rotation(&tkhd.matrix);
    let (width, height) = if tkhd.width > 0 && tkhd.height > 0 {
        (tkhd.width, tkhd.height)
    } else {
        (entry.coded_width, entry.coded_height)
    };
    Ok(Track {
        id: tkhd.track_id,
        kind,
        handler,
        fourcc: entry.fourcc,
        timescale,
        movie_timescale,
        codec: entry.codec,
        width,
        height,
        coded_width: entry.coded_width,
        coded_height: entry.coded_height,
        rotation,
        flip,
        matrix: tkhd.matrix,
        pixel_aspect: entry.pixel_aspect,
        color: entry.color,
        bitrate: entry.bitrate,
        sample_rate: entry.sample_rate,
        channels: entry.channels,
        bits_per_sample: entry.bits_per_sample,
        channel_layout: entry.channel_layout,
        samples,
        sdtp,
        edits,
        edit_offset: 0,
        media_duration,
        duration_secs: 0.0,
        language,
    })
}

/// Computes the derived timing fields once all samples are known.
fn finish_track(t: &mut Track) {
    if t.sdtp.len() != t.samples.len() {
        t.sdtp.clear();
    }
    let decode_end = t.samples.last().map_or(0, |s| (s.dts + s.duration as i64).max(0) as u64);
    t.media_duration = t.media_duration.max(decode_end);
    // No edit list, yet the first picture is held back by B-frame
    // reordering (composition offsets; typical of fragmented files, which
    // cannot carry the edit that cancels it): it starts the track, as in
    // every player. Otherwise every frame came out that many frames early.
    if t.kind == TrackKind::Video && t.edits.is_empty() {
        let first_pts = t.samples.iter().map(|s| s.pts).min();
        let first_dts = t.samples.iter().map(|s| s.dts).min();
        if let (Some(p), Some(d)) = (first_pts, first_dts) {
            if p > d {
                t.edits.push(Edit { segment_duration: 0, media_time: p, rate: 0x10000 });
            }
        }
    }

    let segs = t.segments();
    t.edit_offset = segs
        .iter()
        .find_map(|s| s.media_start.map(|m| m - s.pres_start))
        .unwrap_or(0);
    let media_duration = t.media_duration.min(i64::MAX as u64) as i64;
    let presented: i64 = if segs.is_empty() {
        media_duration
    } else {
        segs.iter()
            .map(|s| match (s.len, s.media_start) {
                (Some(l), _) => l,
                (None, Some(m)) => media_duration.saturating_sub(m).max(0),
                (None, None) => 0,
            })
            .fold(0i64, |a, b| a.saturating_add(b))
    };
    t.duration_secs = presented as f64 / t.timescale as f64;
}

/// Reads iTunes gapless info (`iTunSMPB`) from `moov/udta/meta/ilst`:
/// returns `(priming_samples, original_sample_count)`.
fn itunes_gapless(udta: &[u8]) -> Result<Option<(u64, u64)>> {
    let Some(meta) = find(udta, b"meta")? else { return Ok(None) };
    // ISO 'meta' is a full box; QuickTime's is a plain container.
    let meta = if meta.len() >= 8 && &meta[4..8] == b"hdlr" { meta } else { meta.get(4..).unwrap_or(&[]) };
    let Some(ilst) = find(meta, b"ilst")? else { return Ok(None) };
    for item in Boxes::new(ilst) {
        let item = item?;
        if &item.kind != b"----" {
            continue;
        }
        let (mut name, mut value) = (None, None);
        for b in Boxes::new(item.data) {
            let b = b?;
            match &b.kind {
                b"name" => name = b.data.get(4..),
                b"data" => value = b.data.get(8..),
                _ => {}
            }
        }
        if name != Some(b"iTunSMPB".as_slice()) {
            continue;
        }
        let text = String::from_utf8_lossy(value.unwrap_or(&[])).into_owned();
        let fields: Vec<u64> = text
            .split_whitespace()
            .map(|f| u64::from_str_radix(f, 16))
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| Error::malformed("iTunSMPB"))?;
        if fields.len() >= 4 && fields[3] > 0 {
            return Ok(Some((fields[1], fields[3])));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(edits: Vec<Edit>, pts: &[i64]) -> Track {
        let samples = pts
            .iter()
            .enumerate()
            .map(|(i, &p)| Sample {
                offset: 0,
                size: 1,
                dts: i as i64 * 10,
                pts: p,
                duration: 10,
                sync: i % 3 == 0,
            })
            .collect();
        let mut t = Track {
            id: 1, kind: TrackKind::Video, handler: *b"vide", fourcc: *b"avc1",
            timescale: 100, movie_timescale: 1000, codec: Codec::Other(*b"avc1"),
            width: 0, height: 0, coded_width: 0, coded_height: 0, rotation: 0, flip: false,
            matrix: [0; 9], pixel_aspect: None, color: None, bitrate: None, sample_rate: 0,
            channels: 0, bits_per_sample: 0, channel_layout: None, samples, sdtp: Vec::new(),
            edits, edit_offset: 0, media_duration: 0, duration_secs: 0.0, language: "und".into(),
        };
        finish_track(&mut t);
        t
    }

    #[test]
    fn reorder_delay_without_edit_list_starts_at_first_picture() {
        // B-frames, no `elst` (a fragmented file): the first picture's
        // composition time is two frames in, and it is shown at 0.
        let t = track(vec![], &[20, 50, 30, 40, 80, 60, 70]);
        assert_eq!(t.media_to_presentation(20), Some(0));
        assert_eq!(t.media_to_presentation(30), Some(10));
        let sp = t.sample_for_time(0.1).unwrap();
        assert_eq!(t.samples[sp.target_index].pts, 30);
        // Nothing reordered, nothing shifted.
        let t = track(vec![], &[0, 10, 20]);
        assert!(t.edits.is_empty());
    }

    #[test]
    fn empty_edit_delays_start() {
        // 0.5 s of nothing, then media from time 20 for 0.4 s.
        let edits = vec![
            Edit { segment_duration: 500, media_time: -1, rate: 0x10000 },
            Edit { segment_duration: 400, media_time: 20, rate: 0x10000 },
        ];
        let t = track(edits, &[10, 20, 30, 40, 50, 60, 70]);
        assert_eq!(t.edit_offset, 20 - 50);
        assert!((t.duration_secs - 0.9).abs() < 1e-12);
        assert_eq!(t.media_to_presentation(10), None);
        assert_eq!(t.media_to_presentation(20), Some(50));
        assert_eq!(t.media_to_presentation(50), Some(80));
        assert_eq!(t.media_to_presentation(60), None);
        assert_eq!(t.presented_samples(), 4);
        // Seeking into the empty edit lands on the first presented frame.
        let sp = t.sample_for_time(0.1).unwrap();
        assert_eq!(t.samples[sp.target_index].pts, 20);
        let sp = t.sample_for_time(0.75).unwrap();
        assert_eq!((t.samples[sp.target_index].pts, sp.sync_index), (40, 3));
        let sp = t.sample_for_time(0.65).unwrap();
        assert_eq!((t.samples[sp.target_index].pts, sp.sync_index), (30, 0));
        // Past the end clamps to the last presented frame.
        let sp = t.sample_for_time(10.0).unwrap();
        assert_eq!(t.samples[sp.target_index].pts, 50);
    }

    #[test]
    fn multiple_edits_cut_media() {
        // Play media [0, 20) then jump to [50, 70).
        let edits = vec![
            Edit { segment_duration: 200, media_time: 0, rate: 0x10000 },
            Edit { segment_duration: 200, media_time: 50, rate: 0x10000 },
        ];
        let t = track(edits, &[0, 10, 20, 30, 40, 50, 60]);
        assert_eq!(t.media_to_presentation(10), Some(10));
        assert_eq!(t.media_to_presentation(30), None);
        assert_eq!(t.media_to_presentation(60), Some(30));
        assert_eq!(t.presentation_to_media(25), Some(55));
        assert_eq!(t.presented_samples(), 4);
    }
}
