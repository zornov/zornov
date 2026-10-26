// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sample tables (`stts`, `ctts`, `stss`, `stsc`, `stsz`/`stz2`, `stco`/`co64`,
//! `sdtp`) and their expansion into a flat list of [`Sample`]s.

use crate::r#box::{Boxes, Reader};
use crate::demux::Sample;
use crate::error::{Error, Result};

/// Hard cap on samples per track, independent of file size.
pub(crate) const MAX_SAMPLES: usize = 1 << 25;

#[derive(Debug, Clone, Copy)]
struct StscEntry {
    first_chunk: u32,
    samples_per_chunk: u32,
}

#[derive(Debug)]
enum Sizes {
    Constant { size: u32, count: usize },
    Table(Vec<u32>),
}

impl Sizes {
    fn len(&self) -> usize {
        match self {
            Sizes::Constant { count, .. } => *count,
            Sizes::Table(v) => v.len(),
        }
    }

    fn get(&self, i: usize) -> u32 {
        match self {
            Sizes::Constant { size, .. } => *size,
            Sizes::Table(v) => v[i],
        }
    }
}

/// Raw sample-table contents of one `stbl`.
#[derive(Debug)]
pub(crate) struct SampleTable {
    stts: Vec<(u32, u32)>,
    ctts: Vec<(u32, i32)>,
    stss: Option<Vec<u32>>,
    stsc: Vec<StscEntry>,
    sizes: Sizes,
    chunks: Vec<u64>,
    pub sdtp: Vec<u8>,
}

impl SampleTable {
    pub fn parse(stbl: &[u8], file_len: u64) -> Result<Self> {
        let mut stts = None;
        let mut ctts = Vec::new();
        let mut stss = None;
        let mut stsc = None;
        let mut sizes = None;
        let mut chunks = None;
        let mut sdtp = Vec::new();
        for b in Boxes::new(stbl) {
            let b = b?;
            match &b.kind {
                b"stts" => stts = Some(parse_stts(b.data)?),
                b"ctts" => ctts = parse_ctts(b.data)?,
                b"stss" => stss = Some(parse_stss(b.data)?),
                b"stsc" => stsc = Some(parse_stsc(b.data)?),
                b"stsz" => sizes = Some(parse_stsz(b.data, file_len)?),
                b"stz2" => sizes = Some(parse_stz2(b.data)?),
                b"stco" => chunks = Some(parse_stco(b.data, false)?),
                b"co64" => chunks = Some(parse_stco(b.data, true)?),
                b"sdtp" => {
                    let mut r = Reader::new(b.data, "sdtp");
                    r.full_header()?;
                    sdtp = r.rest().to_vec();
                }
                _ => {}
            }
        }
        Ok(SampleTable {
            stts: stts.ok_or(Error::Missing("stts"))?,
            ctts,
            stss,
            stsc: stsc.ok_or(Error::Missing("stsc"))?,
            sizes: sizes.ok_or(Error::Missing("stsz/stz2"))?,
            chunks: chunks.ok_or(Error::Missing("stco/co64"))?,
            sdtp,
        })
    }

    /// Expands the tables into decode-ordered samples, validating every sample
    /// against the file length.
    pub fn samples(&self, file_len: u64) -> Result<Vec<Sample>> {
        let count = self.sizes.len();
        if count > MAX_SAMPLES {
            return Err(Error::TooLarge("sample count"));
        }
        let mut out = Vec::with_capacity(count);

        // Chunk layout: offsets and sizes.
        let mut chunk_index = 0usize; // 0-based
        let mut stsc_i = 0usize;
        'chunks: while out.len() < count {
            if chunk_index >= self.chunks.len() {
                return Err(Error::malformed("stsc/stco describe fewer samples than stsz"));
            }
            while stsc_i + 1 < self.stsc.len()
                && (self.stsc[stsc_i + 1].first_chunk as usize) <= chunk_index + 1
            {
                stsc_i += 1;
            }
            let per_chunk = match self.stsc.get(stsc_i) {
                Some(e) if e.first_chunk as usize <= chunk_index + 1 => e.samples_per_chunk,
                _ => 0,
            };
            let mut offset = self.chunks[chunk_index];
            for _ in 0..per_chunk {
                if out.len() == count {
                    break 'chunks;
                }
                let size = self.sizes.get(out.len());
                let end = offset
                    .checked_add(size as u64)
                    .filter(|&e| e <= file_len)
                    .ok_or_else(|| Error::malformed("sample lies outside the file"))?;
                out.push(Sample { offset, size, dts: 0, pts: 0, duration: 0, sync: true });
                offset = end;
            }
            chunk_index += 1;
        }

        // Decode times.
        let mut dts: i64 = 0;
        let mut i = 0usize;
        let mut last_delta = 0u32;
        for &(n, delta) in &self.stts {
            for _ in 0..n {
                if i == count {
                    break;
                }
                out[i].dts = dts;
                out[i].duration = delta;
                dts = dts.checked_add(delta as i64).ok_or(Error::TooLarge("decode time"))?;
                i += 1;
            }
            last_delta = delta;
        }
        // Lenient: extend the last delta over samples stts forgot to cover.
        for s in &mut out[i..] {
            s.dts = dts;
            s.duration = last_delta;
            dts = dts.checked_add(last_delta as i64).ok_or(Error::TooLarge("decode time"))?;
        }

        // Composition offsets.
        let mut i = 0usize;
        'ctts: for &(n, off) in &self.ctts {
            for _ in 0..n {
                if i == count {
                    break 'ctts;
                }
                out[i].pts = out[i].dts + off as i64;
                i += 1;
            }
        }
        for s in &mut out[i..] {
            s.pts = s.dts;
        }

        // Sync samples (absent table = every sample is a sync sample).
        if let Some(stss) = &self.stss {
            for s in &mut out {
                s.sync = false;
            }
            for &n in stss {
                if n >= 1 && (n as usize) <= count {
                    out[n as usize - 1].sync = true;
                }
            }
        }
        Ok(out)
    }
}

fn parse_stts(data: &[u8]) -> Result<Vec<(u32, u32)>> {
    let mut r = Reader::new(data, "stts");
    r.full_header()?;
    let n = r.count(8)?;
    (0..n).map(|_| Ok((r.u32()?, r.u32()?))).collect()
}

/// Composition offsets are read as signed in both versions: QuickTime writes
/// negative offsets into version-0 `ctts`, and players accept them.
fn parse_ctts(data: &[u8]) -> Result<Vec<(u32, i32)>> {
    let mut r = Reader::new(data, "ctts");
    r.full_header()?;
    let n = r.count(8)?;
    (0..n).map(|_| Ok((r.u32()?, r.i32()?))).collect()
}

fn parse_stss(data: &[u8]) -> Result<Vec<u32>> {
    let mut r = Reader::new(data, "stss");
    r.full_header()?;
    let n = r.count(4)?;
    (0..n).map(|_| r.u32()).collect()
}

fn parse_stsc(data: &[u8]) -> Result<Vec<StscEntry>> {
    let mut r = Reader::new(data, "stsc");
    r.full_header()?;
    let n = r.count(12)?;
    let mut out: Vec<StscEntry> = Vec::with_capacity(n);
    for _ in 0..n {
        let first_chunk = r.u32()?;
        let samples_per_chunk = r.u32()?;
        r.u32()?; // sample description index
        if first_chunk == 0 || out.last().is_some_and(|p| p.first_chunk >= first_chunk) {
            return Err(Error::malformed("stsc first_chunk not strictly increasing from 1"));
        }
        out.push(StscEntry { first_chunk, samples_per_chunk });
    }
    Ok(out)
}

fn parse_stsz(data: &[u8], file_len: u64) -> Result<Sizes> {
    let mut r = Reader::new(data, "stsz");
    r.full_header()?;
    let size = r.u32()?;
    if size != 0 {
        let count = r.u32()? as u64;
        // Constant-size samples must still fit in the file.
        if count.saturating_mul(size as u64) > file_len || count as usize > MAX_SAMPLES {
            return Err(Error::malformed("stsz constant sizes exceed the file"));
        }
        return Ok(Sizes::Constant { size, count: count as usize });
    }
    let n = r.count(4)?;
    Ok(Sizes::Table((0..n).map(|_| r.u32()).collect::<Result<_>>()?))
}

fn parse_stz2(data: &[u8]) -> Result<Sizes> {
    let mut r = Reader::new(data, "stz2");
    r.full_header()?;
    r.u24()?;
    let field = r.u8()?;
    let n = r.u32()? as usize;
    let bytes_needed = match field {
        4 => n.div_ceil(2),
        8 => n,
        16 => n.checked_mul(2).ok_or(Error::Truncated("stz2"))?,
        _ => return Err(Error::malformed(format!("stz2 field size {field}"))),
    };
    let raw = r.bytes(bytes_needed)?;
    let sizes = (0..n)
        .map(|i| match field {
            4 => {
                let b = raw[i / 2];
                (if i % 2 == 0 { b >> 4 } else { b & 0x0f }) as u32
            }
            8 => raw[i] as u32,
            _ => u16::from_be_bytes([raw[2 * i], raw[2 * i + 1]]) as u32,
        })
        .collect();
    Ok(Sizes::Table(sizes))
}

fn parse_stco(data: &[u8], wide: bool) -> Result<Vec<u64>> {
    let mut r = Reader::new(data, if wide { "co64" } else { "stco" });
    r.full_header()?;
    let n = r.count(if wide { 8 } else { 4 })?;
    (0..n).map(|_| if wide { r.u64() } else { r.u32().map(u64::from) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 12) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(body);
        v
    }

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    #[test]
    fn expands_chunks_times_and_sync() {
        let mut stbl = Vec::new();
        stbl.extend(full(b"stts", &words(&[2, 2, 10, 1, 20])));
        stbl.extend(full(b"ctts", &words(&[2, 1, 10, 1, 0xffff_fffb])));
        stbl.extend(full(b"stss", &words(&[2, 1, 3])));
        stbl.extend(full(b"stsc", &words(&[2, 1, 2, 1, 2, 1, 1])));
        stbl.extend(full(b"stz2", &[0, 0, 0, 4, 0, 0, 0, 3, 0x12, 0x30]));
        stbl.extend(full(b"stco", &words(&[2, 100, 200])));
        let t = SampleTable::parse(&stbl, 1000).unwrap();
        let s = t.samples(1000).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].offset, s[0].size), (100, 1));
        assert_eq!((s[1].offset, s[1].size), (101, 2));
        assert_eq!((s[2].offset, s[2].size), (200, 3));
        assert_eq!(s.iter().map(|x| x.dts).collect::<Vec<_>>(), [0, 10, 20]);
        assert_eq!(s.iter().map(|x| x.pts).collect::<Vec<_>>(), [10, 5, 20]);
        assert_eq!(s.iter().map(|x| x.sync).collect::<Vec<_>>(), [true, false, true]);
        assert!(t.samples(150).is_err(), "sample beyond EOF must be rejected");
    }

    #[test]
    fn constant_size_bounded_by_file() {
        let body = words(&[1, 0xffff_ffff]);
        assert!(parse_stsz(&full(b"stsz", &body)[8..], 1 << 20).is_err());
    }
}
