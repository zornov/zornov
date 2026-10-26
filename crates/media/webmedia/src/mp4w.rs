// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writes the gathered tracks as one ordinary MP4: `moov` first (so it
//! plays while it is still being read), samples interleaved about a second
//! at a time, composition offsets and edit lists kept. Sample entries from
//! the source (H.264, HEVC, AAC, AC-3, AV1...) are copied as they are.

use crate::error::{Error, Result};
use crate::track::{Sample, TrackData};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub fn bx(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    v.extend_from_slice(t);
    v.extend_from_slice(body);
    v
}

pub fn fbx(t: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(t, &b)
}

fn u32s(vals: &[u32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_be_bytes()).collect()
}

// ---- sample entries built for transport stream sources ----

fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

fn parse_sps(sps: &[u8]) -> Option<studio_h264::Sps> {
    if sps.len() < 4 {
        return None;
    }
    studio_h264::Sps::parse(&unescape(&sps[1..])).ok()
}

/// The picture size an H.264 SPS NAL unit describes.
pub fn h264_size(sps: &[u8]) -> Option<(u32, u32)> {
    let s = parse_sps(sps)?;
    let (w, h) = s.cropped_size();
    (w > 0 && h > 0).then_some((w, h))
}

/// An `avc1` sample entry from an SPS and PPS NAL units.
pub fn avc1_entry(sps: &[u8], pps: &[Vec<u8>], w: u32, h: u32) -> Option<Vec<u8>> {
    if sps.len() < 4 || pps.is_empty() {
        return None;
    }
    let parsed = parse_sps(sps)?;
    let mut c = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
    c.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    c.extend_from_slice(sps);
    c.push(pps.len() as u8);
    for p in pps {
        c.extend_from_slice(&(p.len() as u16).to_be_bytes());
        c.extend_from_slice(p);
    }
    if matches!(sps[1], 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        c.push(0xFC | parsed.chroma_format_idc as u8);
        c.push(0xF8 | (parsed.bit_depth_luma.saturating_sub(8) as u8 & 7));
        c.push(0xF8 | (parsed.bit_depth_chroma.saturating_sub(8) as u8 & 7));
        c.push(0);
    }
    let mut e = vec![0u8; 6];
    e.extend_from_slice(&1u16.to_be_bytes());
    e.extend_from_slice(&[0; 16]);
    e.extend_from_slice(&(w as u16).to_be_bytes());
    e.extend_from_slice(&(h as u16).to_be_bytes());
    e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    e.extend_from_slice(&0u32.to_be_bytes());
    e.extend_from_slice(&1u16.to_be_bytes());
    e.extend_from_slice(&[0; 32]);
    e.extend_from_slice(&0x18u16.to_be_bytes());
    e.extend_from_slice(&0xFFFFu16.to_be_bytes());
    e.extend_from_slice(&bx(b"avcC", &c));
    Some(bx(b"avc1", &e))
}

fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len();
    let mut v = vec![tag, 0x80 | ((n >> 21) & 0x7F) as u8, 0x80 | ((n >> 14) & 0x7F) as u8, 0x80 | ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8];
    v.extend_from_slice(body);
    v
}

/// An `mp4a` sample entry for AAC with the given AudioSpecificConfig.
pub fn mp4a_entry(asc: &[u8], channels: u16, rate: u32) -> Vec<u8> {
    let mut dc = vec![0x40, 0x15, 0, 0, 0];
    dc.extend_from_slice(&0u32.to_be_bytes());
    dc.extend_from_slice(&0u32.to_be_bytes());
    dc.extend_from_slice(&descriptor(5, asc));
    let mut es = vec![0, 0, 0];
    es.extend_from_slice(&descriptor(4, &dc));
    es.extend_from_slice(&descriptor(6, &[2]));
    let esds = fbx(b"esds", 0, 0, &descriptor(3, &es));
    let mut e = vec![0u8; 6];
    e.extend_from_slice(&1u16.to_be_bytes());
    e.extend_from_slice(&[0; 8]);
    e.extend_from_slice(&channels.to_be_bytes());
    e.extend_from_slice(&16u16.to_be_bytes());
    e.extend_from_slice(&[0; 4]);
    e.extend_from_slice(&(rate.min(65535) << 16).to_be_bytes());
    e.extend_from_slice(&esds);
    bx(b"mp4a", &e)
}

// ---- the file ----

struct Plan<'a> {
    t: &'a TrackData,
    dts0: i64,
    /// Cumulative decode times from 0 (sum of durations before each sample).
    cum: Vec<i64>,
    media_dur: i64,
    /// Playback starts at this media time (relative to the first sample's decode time).
    start_rel: i64,
    /// Seconds from the movie's first moment to the track's first shown frame.
    lead_ms: i64,
    chunks: Vec<(usize, usize)>, // first sample, count
}

fn pack_lang(l: &str) -> u16 {
    let b: Vec<u8> = l.bytes().filter(u8::is_ascii_lowercase).take(3).collect();
    if b.len() != 3 {
        return 0x55C4; // und
    }
    (((b[0] - 0x60) as u16) << 10) | (((b[1] - 0x60) as u16) << 5) | (b[2] - 0x60) as u16
}

fn runs(vals: impl Iterator<Item = i64>) -> Vec<(u32, i64)> {
    let mut out: Vec<(u32, i64)> = Vec::new();
    for v in vals {
        match out.last_mut() {
            Some((n, x)) if *x == v => *n += 1,
            _ => out.push((1, v)),
        }
    }
    out
}

fn stbl(p: &Plan, co64: bool, mdat_start: u64, chunk_offsets: &[u64]) -> Vec<u8> {
    let t = p.t;
    let n = t.samples.len();
    let mut stsd_body = 1u32.to_be_bytes().to_vec();
    stsd_body.extend_from_slice(&t.entry);
    let stsd = fbx(b"stsd", 0, 0, &stsd_body);
    let stts_runs = runs(t.samples.iter().map(|s| s.dur as i64));
    let mut b = (stts_runs.len() as u32).to_be_bytes().to_vec();
    for (c, d) in &stts_runs {
        b.extend_from_slice(&c.to_be_bytes());
        b.extend_from_slice(&(*d as u32).to_be_bytes());
    }
    let stts = fbx(b"stts", 0, 0, &b);
    let offs: Vec<i64> = (0..n).map(|i| t.samples[i].pts - p.dts0 - p.cum[i]).collect();
    let ctts = if offs.iter().any(|&o| o != 0) {
        let neg = offs.iter().any(|&o| o < 0);
        let r = runs(offs.iter().copied());
        let mut b = (r.len() as u32).to_be_bytes().to_vec();
        for (c, o) in &r {
            b.extend_from_slice(&c.to_be_bytes());
            b.extend_from_slice(&(*o as i32).to_be_bytes());
        }
        fbx(b"ctts", if neg { 1 } else { 0 }, 0, &b)
    } else {
        Vec::new()
    };
    let stss = if t.samples.iter().any(|s| !s.sync) {
        let idx: Vec<u32> = t.samples.iter().enumerate().filter(|(_, s)| s.sync).map(|(i, _)| i as u32 + 1).collect();
        let mut b = (idx.len() as u32).to_be_bytes().to_vec();
        b.extend_from_slice(&u32s(&idx));
        fbx(b"stss", 0, 0, &b)
    } else {
        Vec::new()
    };
    let mut b = 0u32.to_be_bytes().to_vec();
    b.extend_from_slice(&(n as u32).to_be_bytes());
    b.extend_from_slice(&u32s(&t.samples.iter().map(|s| s.size).collect::<Vec<_>>()));
    let stsz = fbx(b"stsz", 0, 0, &b);
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for (ci, &(_, count)) in p.chunks.iter().enumerate() {
        if entries.last().map(|e| e.1) != Some(count as u32) {
            entries.push((ci as u32 + 1, count as u32));
        }
    }
    let mut b = (entries.len() as u32).to_be_bytes().to_vec();
    for (first, count) in &entries {
        b.extend_from_slice(&first.to_be_bytes());
        b.extend_from_slice(&count.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
    }
    let stsc = fbx(b"stsc", 0, 0, &b);
    let mut b = (chunk_offsets.len() as u32).to_be_bytes().to_vec();
    for &o in chunk_offsets {
        if co64 {
            b.extend_from_slice(&(mdat_start + o).to_be_bytes());
        } else {
            b.extend_from_slice(&((mdat_start + o) as u32).to_be_bytes());
        }
    }
    let stco = fbx(if co64 { b"co64" } else { b"stco" }, 0, 0, &b);
    let mut all = Vec::new();
    for part in [stsd, stts, ctts, stss, stsc, stsz, stco] {
        all.extend_from_slice(&part);
    }
    bx(b"stbl", &all)
}

fn trak(p: &Plan, id: u32, movie_ms: i64, co64: bool, mdat_start: u64, chunk_offsets: &[u64]) -> Vec<u8> {
    let t = p.t;
    let ms = |ticks: i64| ticks * 1000 / t.timescale.max(1) as i64;
    let shown = p.media_dur - p.start_rel;
    let track_ms = p.lead_ms + ms(shown.max(0));
    let mut b = Vec::new();
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&(track_ms.max(0) as u32).to_be_bytes());
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&0u16.to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes());
    b.extend_from_slice(&(if t.is_audio() { 0x0100u16 } else { 0 }).to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes());
    for m in t.matrix {
        b.extend_from_slice(&m.to_be_bytes());
    }
    b.extend_from_slice(&(t.width << 16).to_be_bytes());
    b.extend_from_slice(&(t.height << 16).to_be_bytes());
    let tkhd = fbx(b"tkhd", 0, 3, &b);
    let _ = movie_ms;
    let edts = if p.lead_ms > 0 || p.start_rel != 0 {
        let mut e = Vec::new();
        let mut count = 0u32;
        if p.lead_ms > 0 {
            e.extend_from_slice(&(p.lead_ms as u32).to_be_bytes());
            e.extend_from_slice(&(-1i32).to_be_bytes());
            e.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            count += 1;
        }
        e.extend_from_slice(&(ms(shown.max(0)) as u32).to_be_bytes());
        e.extend_from_slice(&(p.start_rel as i32).to_be_bytes());
        e.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        count += 1;
        let mut body = count.to_be_bytes().to_vec();
        body.extend_from_slice(&e);
        bx(b"edts", &fbx(b"elst", 0, 0, &body))
    } else {
        Vec::new()
    };
    let big = p.media_dur > u32::MAX as i64;
    let mut m = Vec::new();
    if big {
        m.extend_from_slice(&0u64.to_be_bytes());
        m.extend_from_slice(&0u64.to_be_bytes());
        m.extend_from_slice(&t.timescale.to_be_bytes());
        m.extend_from_slice(&(p.media_dur as u64).to_be_bytes());
    } else {
        m.extend_from_slice(&0u32.to_be_bytes());
        m.extend_from_slice(&0u32.to_be_bytes());
        m.extend_from_slice(&t.timescale.to_be_bytes());
        m.extend_from_slice(&(p.media_dur as u32).to_be_bytes());
    }
    m.extend_from_slice(&pack_lang(&t.lang).to_be_bytes());
    m.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = fbx(b"mdhd", if big { 1 } else { 0 }, 0, &m);
    let mut h = vec![0u8; 4];
    h.extend_from_slice(&t.handler);
    h.extend_from_slice(&[0; 12]);
    h.extend_from_slice(if t.is_video() { b"VideoHandler\0" } else { b"SoundHandler\0" });
    let hdlr = fbx(b"hdlr", 0, 0, &h);
    let xmhd = if t.is_video() { fbx(b"vmhd", 0, 1, &[0; 8]) } else { fbx(b"smhd", 0, 0, &[0; 4]) };
    let url = fbx(b"url ", 0, 1, &[]);
    let mut dref = 1u32.to_be_bytes().to_vec();
    dref.extend_from_slice(&url);
    let dinf = bx(b"dinf", &fbx(b"dref", 0, 0, &dref));
    let mut minf = xmhd;
    minf.extend_from_slice(&dinf);
    minf.extend_from_slice(&stbl(p, co64, mdat_start, chunk_offsets));
    let mut mdia = mdhd;
    mdia.extend_from_slice(&hdlr);
    mdia.extend_from_slice(&bx(b"minf", &minf));
    let mut all = tkhd;
    all.extend_from_slice(&edts);
    all.extend_from_slice(&bx(b"mdia", &mdia));
    bx(b"trak", &all)
}

fn ftyp(audio_only: bool) -> Vec<u8> {
    let mut b = Vec::new();
    if audio_only {
        b.extend_from_slice(b"M4A ");
        b.extend_from_slice(&0u32.to_be_bytes());
        for c in [b"M4A ", b"mp42", b"isom"] {
            b.extend_from_slice(c);
        }
    } else {
        b.extend_from_slice(b"isom");
        b.extend_from_slice(&512u32.to_be_bytes());
        for c in [b"isom", b"iso2", b"avc1", b"mp41"] {
            b.extend_from_slice(c);
        }
    }
    bx(b"ftyp", &b)
}

fn moov(plans: &[Plan], movie_ms: i64, co64: bool, mdat_start: u64, offsets: &[Vec<u64>]) -> Vec<u8> {
    let mut mv = Vec::new();
    mv.extend_from_slice(&0u32.to_be_bytes());
    mv.extend_from_slice(&0u32.to_be_bytes());
    mv.extend_from_slice(&1000u32.to_be_bytes());
    mv.extend_from_slice(&(movie_ms.max(0) as u32).to_be_bytes());
    mv.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mv.extend_from_slice(&0x0100u16.to_be_bytes());
    mv.extend_from_slice(&[0; 10]);
    for m in crate::track::IDENTITY {
        mv.extend_from_slice(&m.to_be_bytes());
    }
    mv.extend_from_slice(&[0; 24]);
    mv.extend_from_slice(&(plans.len() as u32 + 1).to_be_bytes());
    let mut all = fbx(b"mvhd", 0, 0, &mv);
    for (i, p) in plans.iter().enumerate() {
        all.extend_from_slice(&trak(p, i as u32 + 1, movie_ms, co64, mdat_start, &offsets[i]));
    }
    bx(b"moov", &all)
}

/// Writes `tracks` to `dest` as an MP4. Returns the file size.
pub fn write_mp4(dest: &Path, tracks: &[TrackData]) -> Result<u64> {
    let tracks: Vec<&TrackData> = tracks.iter().filter(|t| !t.samples.is_empty()).collect();
    if tracks.is_empty() {
        return Err(Error::Bad("there is nothing to write: the segments held no samples".into()));
    }
    // Timing per track.
    let mut plans: Vec<Plan> = Vec::new();
    let mut abs_starts = Vec::new();
    for t in &tracks {
        let dts0 = t.samples[0].dts;
        let mut cum = Vec::with_capacity(t.samples.len());
        let mut acc = 0i64;
        for s in &t.samples {
            cum.push(acc);
            acc += s.dur as i64;
        }
        let min_pts = t.samples.iter().map(|s| s.pts).min().unwrap_or(dts0);
        let start_abs = if t.edit_offset != 0 { t.edit_offset } else { min_pts };
        let start_rel = (start_abs - dts0).clamp(0, acc.max(0));
        abs_starts.push((dts0 + start_rel) as f64 / t.timescale.max(1) as f64);
        plans.push(Plan { t, dts0, cum, media_dur: acc, start_rel, lead_ms: 0, chunks: Vec::new() });
    }
    let t0 = abs_starts.iter().cloned().fold(f64::MAX, f64::min);
    for (p, a) in plans.iter_mut().zip(&abs_starts) {
        p.lead_ms = ((a - t0) * 1000.0).round().max(0.0) as i64;
    }
    let movie_ms = plans.iter().map(|p| p.lead_ms + (p.media_dur - p.start_rel).max(0) * 1000 / p.t.timescale.max(1) as i64).max().unwrap_or(0);
    // Chunks, interleaved by time.
    let mut next = vec![0usize; plans.len()];
    let mut payload = 0u64;
    let mut order: Vec<(usize, usize, usize)> = Vec::new(); // track, first, count
    let mut offsets: Vec<Vec<u64>> = vec![Vec::new(); plans.len()];
    loop {
        let mut pick: Option<(usize, f64)> = None;
        for (ti, p) in plans.iter().enumerate() {
            if next[ti] < p.t.samples.len() {
                let at = p.lead_ms as f64 / 1000.0 + p.cum[next[ti]] as f64 / p.t.timescale.max(1) as f64;
                if pick.is_none_or(|(_, b)| at < b) {
                    pick = Some((ti, at));
                }
            }
        }
        let Some((ti, _)) = pick else { break };
        let p = &plans[ti];
        let first = next[ti];
        let mut count = 0;
        let mut ticks = 0i64;
        let mut size = 0u64;
        while first + count < p.t.samples.len() && (count == 0 || ticks < p.t.timescale as i64) {
            let s = &p.t.samples[first + count];
            ticks += s.dur as i64;
            size += s.size as u64;
            count += 1;
        }
        order.push((ti, first, count));
        offsets[ti].push(payload);
        payload += size;
        next[ti] += count;
    }
    for (ti, p) in plans.iter_mut().enumerate() {
        p.chunks = order.iter().filter(|c| c.0 == ti).map(|&(_, first, count)| (first, count)).collect();
    }
    let co64 = payload > 0xF000_0000;
    let audio_only = tracks.iter().all(|t| t.is_audio());
    let ftyp = ftyp(audio_only);
    let mdat_hdr = if payload + 8 > u32::MAX as u64 { 16 } else { 8 };
    let probe = moov(&plans, movie_ms, co64, 0, &offsets);
    let mdat_start = ftyp.len() as u64 + probe.len() as u64 + mdat_hdr;
    let moov_box = moov(&plans, movie_ms, co64, mdat_start, &offsets);
    debug_assert_eq!(moov_box.len(), probe.len());

    let mut out = BufWriter::with_capacity(1 << 20, File::create(dest)?);
    out.write_all(&ftyp)?;
    out.write_all(&moov_box)?;
    if mdat_hdr == 16 {
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&(payload + 16).to_be_bytes())?;
    } else {
        out.write_all(&(payload as u32 + 8).to_be_bytes())?;
        out.write_all(b"mdat")?;
    }
    let mut stores: Vec<(PathBuf, File)> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut written = 0u64;
    for &(ti, first, count) in &order {
        let t = plans[ti].t;
        let fi = match stores.iter().position(|(p, _)| *p == t.store) {
            Some(i) => i,
            None => {
                stores.push((t.store.clone(), File::open(&t.store)?));
                stores.len() - 1
            }
        };
        let f = &mut stores[fi].1;
        let samples: &[Sample] = &t.samples[first..first + count];
        let mut i = 0;
        while i < samples.len() {
            // A run of samples that sit next to each other in the store.
            let mut j = i + 1;
            let mut len = samples[i].size as u64;
            while j < samples.len() && samples[j].offset == samples[j - 1].offset + samples[j - 1].size as u64 && len < (8 << 20) {
                len += samples[j].size as u64;
                j += 1;
            }
            buf.resize(len as usize, 0);
            f.seek(SeekFrom::Start(samples[i].offset))?;
            f.read_exact(&mut buf).map_err(|e| Error::Io(format!("a downloaded piece is shorter than its index says: {e}")))?;
            out.write_all(&buf)?;
            written += len;
            i = j;
        }
    }
    debug_assert_eq!(written, payload);
    out.flush()?;
    Ok(ftyp.len() as u64 + moov_box.len() as u64 + mdat_hdr + payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::IDENTITY;

    fn track(handler: &[u8; 4], timescale: u32, entry: Vec<u8>, store: &Path, samples: Vec<Sample>) -> TrackData {
        TrackData { handler: *handler, timescale, entry, width: 64, height: 48, lang: "und".into(), matrix: IDENTITY, edit_offset: 0, samples, store: store.to_path_buf() }
    }

    /// Frames shown out of decode order (B-frames), a sound track that
    /// starts later than the picture, and an AAC-style priming edit all
    /// come back out of Studio's own reader as they went in.
    #[test]
    fn composition_offsets_and_start_offsets_survive() {
        let dir = std::env::temp_dir().join(format!("studio-webmedia-mp4w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vstore = dir.join("v.es");
        let astore = dir.join("a.es");
        let vdata: Vec<u8> = (0..40).collect();
        std::fs::write(&vstore, &vdata).unwrap();
        std::fs::write(&astore, vec![7u8; 30]).unwrap();
        // Decode order I P B B: shown 0, 3, 1, 2 (times 3000 apart, shifted by 1 s of clock).
        let base = 90_000;
        let mk = |i: i64, pts_slot: i64, sync: bool| Sample { offset: i as u64 * 10, size: 10, dts: base + i * 3000, pts: base + 6000 + pts_slot * 3000, dur: 3000, sync };
        let video = track(b"vide", 90_000, crate::mp4w::avc1_entry_for_tests(), &vstore, vec![mk(0, 0, true), mk(1, 3, false), mk(2, 1, false), mk(3, 2, false)]);
        // Sound begins half a second after the first picture and hides 1024 priming samples.
        let asamples: Vec<Sample> = (0..3).map(|i| Sample { offset: i as u64 * 10, size: 10, dts: 24_000 + 45_000 * 0 + i * 1024, pts: 24_000 + i * 1024, dur: 1024, sync: true }).collect();
        let mut audio = track(b"soun", 48_000, mp4a_entry(&[0x11, 0x90], 2, 48000), &astore, asamples);
        audio.edit_offset = 24_000 + 1024;
        let out = dir.join("o.mp4");
        write_mp4(&out, &[video, audio]).unwrap();
        let mut d = studio_mp4::open(&out).unwrap();
        let v = d.movie().video().unwrap().clone();
        let pts: Vec<i64> = v.samples.iter().map(|s| s.pts - v.samples[0].dts).collect();
        assert_eq!(pts, [6000, 15000, 9000, 12000]);
        assert!(v.samples[0].sync && !v.samples[1].sync);
        assert_eq!(d.read_sample(&v.samples[2]).unwrap(), vdata[20..30].to_vec());
        // Sound is first heard (its sample 1; sample 0 is priming) at the clock origin, the first
        // picture 0.5453 s later (1.0667 s against 0.5213 s on the source's clock).
        let a = d.movie().audio().unwrap().clone();
        let heard = a.presentation_secs(&a.samples[1]).unwrap();
        let seen = v.presentation_secs(&v.samples[0]).unwrap();
        assert!(heard.abs() < 0.002, "{heard}");
        assert!((seen - 0.5453).abs() < 0.003, "{seen}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
pub(crate) fn avc1_entry_for_tests() -> Vec<u8> {
    // A real 64x48 Baseline SPS/PPS from Studio's own encoder settings would need the encoder;
    // the entry's contents are not read by this test, only copied.
    let mut e = vec![0u8; 6];
    e.extend_from_slice(&1u16.to_be_bytes());
    e.extend_from_slice(&[0; 70]);
    bx(b"vp09", &e)
}
