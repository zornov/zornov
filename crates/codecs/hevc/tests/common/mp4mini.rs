// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal ISO BMFF / QuickTime reader for tests: finds the first HEVC video
//! track (hvc1 / hev1 sample entry with hvcC) and returns its samples in
//! decode order, together with composition times.

#![allow(dead_code)]

pub struct HevcTrack {
    pub hvcc: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub samples: Vec<Vec<u8>>,
    /// Decode timestamps (from stts) and composition offsets (from ctts).
    pub dts: Vec<u64>,
    pub cts_offset: Vec<i64>,
    pub timescale: u32,
}

fn be32(b: &[u8], p: usize) -> u32 {
    u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}
fn be64(b: &[u8], p: usize) -> u64 {
    ((be32(b, p) as u64) << 32) | be32(b, p + 4) as u64
}

/// Iterates the boxes in `data`: yields (type, payload).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 8 <= data.len() {
        let mut size = be32(data, p) as u64;
        let ty = [data[p + 4], data[p + 5], data[p + 6], data[p + 7]];
        let mut hdr = 8usize;
        if size == 1 {
            if p + 16 > data.len() {
                break;
            }
            size = be64(data, p + 8);
            hdr = 16;
        } else if size == 0 {
            size = (data.len() - p) as u64;
        }
        let size = size as usize;
        if size < hdr || p + size > data.len() {
            break;
        }
        out.push((ty, &data[p + hdr..p + size]));
        p += size;
    }
    out
}

fn child<'a>(data: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(t, _)| t == ty).map(|(_, d)| d)
}

pub fn read_hevc_track(file: &[u8]) -> Option<HevcTrack> {
    let moov = child(file, b"moov")?;
    for (ty, trak) in boxes(moov) {
        if &ty != b"trak" {
            continue;
        }
        let mdia = child(trak, b"mdia")?;
        let mdhd = child(mdia, b"mdhd")?;
        let timescale = if mdhd[0] == 1 { be32(mdhd, 20) } else { be32(mdhd, 12) };
        let minf = child(mdia, b"minf")?;
        let stbl = child(minf, b"stbl")?;
        let stsd = child(stbl, b"stsd")?;
        // stsd: version/flags(4) entry_count(4) entries...
        let entries = boxes(&stsd[8..]);
        let (ety, entry) = entries.first()?;
        if ety != b"hvc1" && ety != b"hev1" {
            continue;
        }
        let width = u16::from_be_bytes([entry[24], entry[25]]);
        let height = u16::from_be_bytes([entry[26], entry[27]]);
        let hvcc = child(&entry[78..], b"hvcC")?.to_vec();

        // sample sizes
        let stsz = child(stbl, b"stsz")?;
        let fixed = be32(stsz, 4);
        let count = be32(stsz, 8) as usize;
        let sizes: Vec<usize> =
            (0..count).map(|i| if fixed != 0 { fixed as usize } else { be32(stsz, 12 + 4 * i) as usize }).collect();
        // chunk offsets
        let offsets: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
            let n = be32(stco, 4) as usize;
            (0..n).map(|i| be32(stco, 8 + 4 * i) as u64).collect()
        } else {
            let co64 = child(stbl, b"co64")?;
            let n = be32(co64, 4) as usize;
            (0..n).map(|i| be64(co64, 8 + 8 * i)).collect()
        };
        // sample-to-chunk
        let stsc = child(stbl, b"stsc")?;
        let n = be32(stsc, 4) as usize;
        let stsc_e: Vec<(u32, u32)> = (0..n).map(|i| (be32(stsc, 8 + 12 * i), be32(stsc, 12 + 12 * i))).collect();
        let mut samples = Vec::with_capacity(count);
        let mut si = 0usize;
        for (ci, &off) in offsets.iter().enumerate() {
            let chunk = ci as u32 + 1;
            let mut per = 0;
            for &(first, spc) in &stsc_e {
                if first <= chunk {
                    per = spc;
                }
            }
            let mut pos = off as usize;
            for _ in 0..per {
                if si >= count {
                    break;
                }
                let sz = sizes[si];
                samples.push(file[pos..pos + sz].to_vec());
                pos += sz;
                si += 1;
            }
        }
        // timing
        let mut dts = Vec::with_capacity(count);
        if let Some(stts) = child(stbl, b"stts") {
            let n = be32(stts, 4) as usize;
            let mut t = 0u64;
            for i in 0..n {
                let c = be32(stts, 8 + 8 * i);
                let d = be32(stts, 12 + 8 * i) as u64;
                for _ in 0..c {
                    dts.push(t);
                    t += d;
                }
            }
        }
        let mut cts_offset = vec![0i64; count];
        if let Some(ctts) = child(stbl, b"ctts") {
            let n = be32(ctts, 4) as usize;
            let mut k = 0usize;
            for i in 0..n {
                let c = be32(ctts, 8 + 8 * i);
                let o = be32(ctts, 12 + 8 * i) as i32 as i64;
                for _ in 0..c {
                    if k < count {
                        cts_offset[k] = o;
                    }
                    k += 1;
                }
            }
        }
        return Some(HevcTrack { hvcc, width, height, samples, dts, cts_offset, timescale });
    }
    None
}
