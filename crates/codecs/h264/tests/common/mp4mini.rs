// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tiny MP4/QuickTime sample reader for tests: finds the first video track with an
//! `avc1`/`avc3` sample entry and returns its `avcC` and samples in decode order.

#![allow(dead_code)]

pub struct Track {
    pub avcc: Vec<u8>,
    pub width: u16,
    pub height: u16,
    /// Samples in decode order.
    pub samples: Vec<Vec<u8>>,
    /// Decode timestamps (timescale units).
    pub dts: Vec<u64>,
    /// Composition offsets per sample (0 when no ctts).
    pub cts_offset: Vec<i64>,
    pub timescale: u32,
}

fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn be64(b: &[u8], i: usize) -> u64 {
    u64::from_be_bytes(b[i..i + 8].try_into().unwrap())
}

/// Iterate child boxes of `data`: (type, payload).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        let mut size = be32(data, i) as usize;
        let typ: [u8; 4] = data[i + 4..i + 8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 {
            if i + 16 > data.len() {
                break;
            }
            size = be64(data, i + 8) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - i;
        }
        if size < hdr || i + size > data.len() {
            break;
        }
        out.push((typ, &data[i + hdr..i + size]));
        i += size;
    }
    out
}

fn find<'a>(data: &'a [u8], typ: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(t, _)| t == typ).map(|(_, p)| p)
}

pub fn read_video_track(file: &[u8]) -> Result<Track, String> {
    let moov = find(file, b"moov").ok_or("no moov")?;
    for (t, trak) in boxes(moov) {
        if &t != b"trak" {
            continue;
        }
        let Some(mdia) = find(trak, b"mdia") else { continue };
        let Some(hdlr) = find(mdia, b"hdlr") else { continue };
        if hdlr.len() < 12 || &hdlr[8..12] != b"vide" {
            continue;
        }
        let mdhd = find(mdia, b"mdhd").ok_or("no mdhd")?;
        let timescale = if mdhd[0] == 1 { be32(mdhd, 20) } else { be32(mdhd, 12) };
        let minf = find(mdia, b"minf").ok_or("no minf")?;
        let stbl = find(minf, b"stbl").ok_or("no stbl")?;
        let stsd = find(stbl, b"stsd").ok_or("no stsd")?;
        // stsd: version/flags (4), entry_count (4), entries.
        let entries = boxes(&stsd[8..]);
        let (etype, entry) = entries.first().ok_or("empty stsd")?;
        if etype != b"avc1" && etype != b"avc3" {
            return Err(format!("sample entry {:?} is not avc1", std::str::from_utf8(etype)));
        }
        // VisualSampleEntry: 6 reserved + 2 dref idx + 16 predefined/reserved + w(2) h(2)
        // + 50 more bytes -> children start at 78.
        let width = u16::from_be_bytes([entry[24], entry[25]]);
        let height = u16::from_be_bytes([entry[26], entry[27]]);
        let avcc = find(&entry[78..], b"avcC").ok_or("no avcC")?.to_vec();

        let stsz = find(stbl, b"stsz").ok_or("no stsz")?;
        let uniform = be32(stsz, 4);
        let count = be32(stsz, 8) as usize;
        let sizes: Vec<u32> = if uniform != 0 { vec![uniform; count] } else { (0..count).map(|k| be32(stsz, 12 + 4 * k)).collect() };

        let chunk_offsets: Vec<u64> = if let Some(stco) = find(stbl, b"stco") {
            let n = be32(stco, 4) as usize;
            (0..n).map(|k| be32(stco, 8 + 4 * k) as u64).collect()
        } else if let Some(co64) = find(stbl, b"co64") {
            let n = be32(co64, 4) as usize;
            (0..n).map(|k| be64(co64, 8 + 8 * k)).collect()
        } else {
            return Err("no stco/co64".into());
        };

        let stsc = find(stbl, b"stsc").ok_or("no stsc")?;
        let n = be32(stsc, 4) as usize;
        let stsc_e: Vec<(u32, u32)> = (0..n).map(|k| (be32(stsc, 8 + 12 * k), be32(stsc, 12 + 12 * k))).collect();

        let mut samples = Vec::with_capacity(count);
        let mut s = 0usize;
        for (ci, &off) in chunk_offsets.iter().enumerate() {
            let chunk_no = ci as u32 + 1;
            let per = stsc_e.iter().rev().find(|e| e.0 <= chunk_no).map(|e| e.1).unwrap_or(0);
            let mut o = off as usize;
            for _ in 0..per {
                if s >= count {
                    break;
                }
                let sz = sizes[s] as usize;
                samples.push(file[o..o + sz].to_vec());
                o += sz;
                s += 1;
            }
        }

        let mut dts = Vec::with_capacity(count);
        if let Some(stts) = find(stbl, b"stts") {
            let n = be32(stts, 4) as usize;
            let mut t = 0u64;
            for k in 0..n {
                let c = be32(stts, 8 + 8 * k);
                let d = be32(stts, 12 + 8 * k) as u64;
                for _ in 0..c {
                    dts.push(t);
                    t += d;
                }
            }
        }
        let mut cts_offset = vec![0i64; count];
        if let Some(ctts) = find(stbl, b"ctts") {
            let version = ctts[0];
            let n = be32(ctts, 4) as usize;
            let mut idx = 0;
            for k in 0..n {
                let c = be32(ctts, 8 + 8 * k);
                let raw = be32(ctts, 12 + 8 * k);
                let v = if version == 1 { raw as i32 as i64 } else { raw as i64 };
                for _ in 0..c {
                    if idx < count {
                        cts_offset[idx] = v;
                    }
                    idx += 1;
                }
            }
        }
        return Ok(Track { avcc, width, height, samples, dts, cts_offset, timescale });
    }
    Err("no AVC video track".into())
}
