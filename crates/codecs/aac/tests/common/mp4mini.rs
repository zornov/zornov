// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal ISO-BMFF reader for the tests: finds the first audio track and
//! returns its AudioSpecificConfig, the raw access units and the edit list.
//! Path: moov > trak > mdia > (hdlr, mdhd, minf > stbl > (stsd, stsz, stsc,
//! stco | co64)), trak > edts > elst.

#![allow(dead_code)]

pub struct AudioTrack {
    pub asc: Vec<u8>,
    pub samples: Vec<Vec<u8>>,
    pub timescale: u32,
    /// media_time of the first edit (priming to skip), in `timescale` units.
    pub edit_media_time: i64,
    /// Whether an edit list with a non-empty edit was found.
    pub has_edit: bool,
    /// segment_duration of the first edit in movie timescale units.
    pub edit_duration: u64,
    pub movie_timescale: u32,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Iterate the boxes in `data`: (type, payload).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 8 <= data.len() {
        let mut size = be32(&data[p..]) as usize;
        let ty = [data[p + 4], data[p + 5], data[p + 6], data[p + 7]];
        let mut hdr = 8;
        if size == 1 {
            size = be64(&data[p + 8..]) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - p;
        }
        if size < hdr || p + size > data.len() {
            break;
        }
        out.push((ty, &data[p + hdr..p + size]));
        p += size;
    }
    out
}

fn find<'a>(data: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(t, _)| t == ty).map(|(_, b)| b)
}

/// Read an MPEG-4 descriptor length (up to 4 bytes of 7 bits).
fn desc_len(b: &[u8], p: &mut usize) -> usize {
    let mut len = 0;
    for _ in 0..4 {
        let c = b[*p];
        *p += 1;
        len = (len << 7) | (c & 0x7f) as usize;
        if c & 0x80 == 0 {
            break;
        }
    }
    len
}

/// Extract DecSpecificInfo from an esds payload.
fn esds_asc(esds: &[u8]) -> Option<Vec<u8>> {
    let b = &esds[4..]; // version/flags
    let mut p = 0;
    while p < b.len() {
        let tag = b[p];
        p += 1;
        let len = desc_len(b, &mut p);
        match tag {
            3 => {
                // ES_Descriptor: ES_ID(2), flags(1) [+ optional fields]
                let flags = b[p + 2];
                p += 3;
                if flags & 0x80 != 0 {
                    p += 2;
                }
                if flags & 0x40 != 0 {
                    p += 1 + b[p] as usize;
                }
                if flags & 0x20 != 0 {
                    p += 2;
                }
            }
            4 => p += 13, // DecoderConfigDescriptor fixed part
            5 => return Some(b[p..p + len].to_vec()),
            _ => p += len,
        }
    }
    None
}

pub fn read_audio_track(file: &[u8]) -> Option<AudioTrack> {
    let moov = find(file, b"moov")?;
    let mvhd = find(moov, b"mvhd")?;
    let movie_timescale = if mvhd[0] == 1 { be32(&mvhd[20..]) } else { be32(&mvhd[12..]) };
    for (ty, trak) in boxes(moov) {
        if &ty != b"trak" {
            continue;
        }
        let mdia = find(trak, b"mdia")?;
        let hdlr = find(mdia, b"hdlr")?;
        if &hdlr[8..12] != b"soun" {
            continue;
        }
        let mdhd = find(mdia, b"mdhd")?;
        let timescale = if mdhd[0] == 1 { be32(&mdhd[20..]) } else { be32(&mdhd[12..]) };
        let stbl = find(find(mdia, b"minf")?, b"stbl")?;
        let stsd = find(stbl, b"stsd")?;
        // stsd: version/flags(4), count(4), then sample entries
        let entry = boxes(&stsd[8..]).into_iter().find(|(t, _)| t == b"mp4a")?.1;
        // AudioSampleEntry: 6 reserved + 2 dref + 8 + 2 ch + 2 bits + 4 + 4 rate = 28 bytes
        let mut ext = &entry[28..];
        let version = u16::from_be_bytes([entry[8], entry[9]]);
        if version == 1 {
            ext = &entry[28 + 16..];
        } else if version == 2 {
            ext = &entry[28 + 36..];
        }
        let esds = find(ext, b"esds").or_else(|| find(find(ext, b"wave")?, b"esds"))?;
        let asc = esds_asc(esds)?;

        let stsz = find(stbl, b"stsz")?;
        let fixed = be32(&stsz[4..]);
        let count = be32(&stsz[8..]) as usize;
        let sizes: Vec<usize> =
            (0..count).map(|i| if fixed != 0 { fixed as usize } else { be32(&stsz[12 + 4 * i..]) as usize }).collect();
        let offsets: Vec<u64> = if let Some(stco) = find(stbl, b"stco") {
            (0..be32(&stco[4..]) as usize).map(|i| be32(&stco[8 + 4 * i..]) as u64).collect()
        } else {
            let co64 = find(stbl, b"co64")?;
            (0..be32(&co64[4..]) as usize).map(|i| be64(&co64[8 + 8 * i..])).collect()
        };
        let stsc = find(stbl, b"stsc")?;
        let runs: Vec<(usize, usize)> = (0..be32(&stsc[4..]) as usize)
            .map(|i| (be32(&stsc[8 + 12 * i..]) as usize, be32(&stsc[12 + 12 * i..]) as usize))
            .collect();
        let mut samples = Vec::with_capacity(count);
        let mut s = 0;
        for (ci, &off) in offsets.iter().enumerate() {
            let chunk = ci + 1;
            let per = runs.iter().rev().find(|r| r.0 <= chunk).map(|r| r.1).unwrap_or(0);
            let mut p = off as usize;
            for _ in 0..per {
                if s >= count {
                    break;
                }
                samples.push(file[p..p + sizes[s]].to_vec());
                p += sizes[s];
                s += 1;
            }
        }
        let (mut edit_media_time, mut edit_duration, mut has_edit) = (0i64, 0u64, false);
        if let Some(elst) = find(trak, b"edts").and_then(|e| find(e, b"elst")) {
            let n = be32(&elst[4..]) as usize;
            let entry = if elst[0] == 1 { 20 } else { 12 };
            for i in 0..n {
                let e = &elst[8 + i * entry..];
                let (dur, mt) = if elst[0] == 1 {
                    (be64(e), be64(&e[8..]) as i64)
                } else {
                    (be32(e) as u64, be32(&e[4..]) as i32 as i64)
                };
                if mt >= 0 {
                    edit_duration = dur;
                    edit_media_time = mt;
                    has_edit = true;
                    break;
                }
            }
        }
        return Some(AudioTrack { asc, samples, timescale, edit_media_time, has_edit, edit_duration, movie_timescale });
    }
    None
}
