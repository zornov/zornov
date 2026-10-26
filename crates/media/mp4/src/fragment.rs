// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fragmented MP4: expands `moof/traf` (`tfhd`, `tfdt`, `trun`) into samples
//! appended to the owning track, using `trex` defaults from `moov/mvex`.

use crate::r#box::{Boxes, Reader};
use crate::boxes::Trex;
use crate::demux::Sample;
use crate::error::{Error, Result};
use crate::stbl::MAX_SAMPLES;

/// `sample_is_non_sync_sample` bit of the sample flags.
const NON_SYNC: u32 = 0x0001_0000;

/// A track that fragments may add samples to.
pub(crate) struct FragTarget<'a> {
    pub track_id: u32,
    pub defaults: Trex,
    pub samples: &'a mut Vec<Sample>,
    /// Kept in step with `samples` only while it already is (see `Track::sdtp`).
    pub sdtp: &'a mut Vec<u8>,
}

/// Parses one `moof` payload. `moof_start` is the file offset of its header.
pub(crate) fn parse_moof(
    moof: &[u8],
    moof_start: u64,
    file_len: u64,
    targets: &mut [FragTarget<'_>],
) -> Result<()> {
    let mut prev_traf_end: Option<u64> = None;
    for b in Boxes::new(moof) {
        let b = b?;
        if &b.kind == b"traf" {
            let end = parse_traf(b.data, moof_start, prev_traf_end, file_len, targets)?;
            prev_traf_end = end.or(prev_traf_end);
        }
    }
    Ok(())
}

/// Returns the file offset just past this traf's last sample.
fn parse_traf(
    traf: &[u8],
    moof_start: u64,
    prev_traf_end: Option<u64>,
    file_len: u64,
    targets: &mut [FragTarget<'_>],
) -> Result<Option<u64>> {
    let tfhd = crate::r#box::require(traf, b"tfhd", "tfhd")?;
    let mut r = Reader::new(tfhd, "tfhd");
    let (_, flags) = r.full_header()?;
    let track_id = r.u32()?;
    let Some(t) = targets.iter_mut().find(|t| t.track_id == track_id) else {
        return Err(Error::malformed(format!("traf for unknown track {track_id}")));
    };
    let base_offset = if flags & 0x01 != 0 { Some(r.u64()?) } else { None };
    if flags & 0x02 != 0 {
        r.u32()?; // sample_description_index
    }
    let def_duration = if flags & 0x08 != 0 { r.u32()? } else { t.defaults.duration };
    let def_size = if flags & 0x10 != 0 { r.u32()? } else { t.defaults.size };
    let def_flags = if flags & 0x20 != 0 { r.u32()? } else { t.defaults.flags };
    let base = match base_offset {
        Some(o) => o,
        None if flags & 0x2_0000 != 0 => moof_start,
        None => prev_traf_end.unwrap_or(moof_start),
    };

    let mut dts = match crate::r#box::find(traf, b"tfdt")? {
        Some(tfdt) => {
            let mut r = Reader::new(tfdt, "tfdt");
            let (v, _) = r.full_header()?;
            let t = if v == 1 { r.u64()? } else { r.u32()? as u64 };
            i64::try_from(t).map_err(|_| Error::TooLarge("tfdt"))?
        }
        None => t.samples.last().map_or(0, |s| s.dts + s.duration as i64),
    };

    let mut cursor = base; // data position for truns without data_offset
    let mut end = None;
    for b in Boxes::new(traf) {
        let b = b?;
        if &b.kind != b"trun" {
            continue;
        }
        let mut r = Reader::new(b.data, "trun");
        let (_, tf) = r.full_header()?;
        let count = r.u32()? as usize;
        if tf & 0x01 != 0 {
            let off = r.i32()? as i64;
            cursor = base
                .checked_add_signed(off)
                .ok_or_else(|| Error::malformed("trun data offset before file start"))?;
        }
        let first_flags = if tf & 0x04 != 0 { Some(r.u32()?) } else { None };
        let per_sample = [0x100, 0x200, 0x400, 0x800].iter().filter(|&&f| tf & f != 0).count() * 4;
        let backed = count.checked_mul(per_sample).is_some_and(|n| n <= r.remaining());
        let bounded = per_sample != 0 || count as u64 <= file_len;
        if !backed || !bounded || t.samples.len() + count > MAX_SAMPLES {
            return Err(Error::malformed("trun sample count exceeds its data"));
        }
        t.samples.reserve(count);
        for i in 0..count {
            let duration = if tf & 0x100 != 0 { r.u32()? } else { def_duration };
            let size = if tf & 0x200 != 0 { r.u32()? } else { def_size };
            let sflags = if tf & 0x400 != 0 { r.u32()? } else { def_flags };
            let sflags = if i == 0 { first_flags.unwrap_or(sflags) } else { sflags };
            let cto = if tf & 0x800 != 0 { r.i32()? as i64 } else { 0 };
            let next = cursor
                .checked_add(size as u64)
                .filter(|&e| e <= file_len)
                .ok_or_else(|| Error::malformed("fragment sample lies outside the file"))?;
            if t.sdtp.len() == t.samples.len() {
                t.sdtp.push((sflags >> 20) as u8);
            }
            t.samples.push(Sample {
                offset: cursor,
                size,
                dts,
                pts: dts.checked_add(cto).ok_or(Error::TooLarge("composition time"))?,
                duration,
                sync: sflags & NON_SYNC == 0,
            });
            dts = dts.checked_add(duration as i64).ok_or(Error::TooLarge("decode time"))?;
            cursor = next;
        }
        end = Some(cursor);
    }
    Ok(end)
}
