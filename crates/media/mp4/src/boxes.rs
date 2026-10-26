// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parsers for the movie- and track-level header boxes.

use crate::r#box::{FourCC, Reader};
use crate::demux::Edit;
use crate::error::{Error, Result};

/// `ftyp`: major brand plus compatible brands.
pub(crate) fn parse_ftyp(data: &[u8]) -> Result<(FourCC, Vec<FourCC>)> {
    let mut r = Reader::new(data, "ftyp");
    let major = r.fourcc()?;
    r.u32()?; // minor version
    let mut compat = Vec::new();
    while r.remaining() >= 4 {
        compat.push(r.fourcc()?);
    }
    Ok((major, compat))
}

/// `mvhd`: returns `(timescale, duration)`.
pub(crate) fn parse_mvhd(data: &[u8]) -> Result<(u32, u64)> {
    let mut r = Reader::new(data, "mvhd");
    let (version, _) = r.full_header()?;
    let (timescale, duration) = if version == 1 {
        r.skip(16)?;
        (r.u32()?, r.u64()?)
    } else {
        r.skip(8)?;
        (r.u32()?, r.u32()? as u64)
    };
    if timescale == 0 {
        return Err(Error::malformed("mvhd timescale is zero"));
    }
    // 0xffffffff means "unknown" in version 0.
    let duration = if version == 0 && duration == u32::MAX as u64 { 0 } else { duration };
    Ok((timescale, duration))
}

/// Fields of `tkhd` that the demuxer exposes.
#[derive(Debug, Clone)]
pub(crate) struct Tkhd {
    pub track_id: u32,
    pub matrix: [i32; 9],
    pub width: u32,
    pub height: u32,
}

pub(crate) fn parse_tkhd(data: &[u8]) -> Result<Tkhd> {
    let mut r = Reader::new(data, "tkhd");
    let (version, _) = r.full_header()?;
    let track_id = if version == 1 {
        r.skip(16)?;
        let id = r.u32()?;
        r.skip(4 + 8)?;
        id
    } else {
        r.skip(8)?;
        let id = r.u32()?;
        r.skip(4 + 4)?;
        id
    };
    r.skip(8 + 2 + 2 + 2 + 2)?; // reserved, layer, alternate group, volume, reserved
    let mut matrix = [0i32; 9];
    for m in &mut matrix {
        *m = r.i32()?;
    }
    // 16.16 fixed point; negative values are nonsense, treat them as unknown.
    let width = (r.u32()? >> 16) & 0x7fff;
    let height = (r.u32()? >> 16) & 0x7fff;
    Ok(Tkhd { track_id, matrix, width, height })
}

/// Derives `(rotation, horizontal_flip)` from a 3x3 display matrix.
///
/// With row vectors (`[x y 1] * M`) a mirrored matrix is treated as "flip
/// horizontally, then rotate clockwise by `rotation` degrees".
pub fn matrix_rotation(m: &[i32; 9]) -> (u16, bool) {
    let (a, b, c, d) = (m[0] as f64, m[1] as f64, m[3] as f64, m[4] as f64);
    let flip = a * d - b * c < 0.0;
    let (a, b) = if flip { (-a, -b) } else { (a, b) };
    if a == 0.0 && b == 0.0 {
        return (0, flip);
    }
    let deg = b.atan2(a).to_degrees();
    let snapped = ((deg / 90.0).round() as i64).rem_euclid(4) as u16 * 90;
    (snapped, flip)
}

/// `elst` entries.
pub(crate) fn parse_elst(data: &[u8]) -> Result<Vec<Edit>> {
    let mut r = Reader::new(data, "elst");
    let (version, _) = r.full_header()?;
    let entry = if version == 1 { 20 } else { 12 };
    let n = r.count(entry)?;
    let mut edits = Vec::with_capacity(n);
    for _ in 0..n {
        let (segment_duration, media_time) = if version == 1 {
            (r.u64()?, r.i64()?)
        } else {
            (r.u32()? as u64, r.i32()? as i64)
        };
        let rate = r.i32()?;
        if media_time < -1 {
            return Err(Error::malformed("elst media_time below -1"));
        }
        edits.push(Edit { segment_duration, media_time, rate });
    }
    Ok(edits)
}

/// `mdhd`: returns `(timescale, duration, language)`.
pub(crate) fn parse_mdhd(data: &[u8]) -> Result<(u32, u64, String)> {
    let mut r = Reader::new(data, "mdhd");
    let (version, _) = r.full_header()?;
    let (timescale, duration) = if version == 1 {
        r.skip(16)?;
        (r.u32()?, r.u64()?)
    } else {
        r.skip(8)?;
        (r.u32()?, r.u32()? as u64)
    };
    if timescale == 0 {
        return Err(Error::malformed("mdhd timescale is zero"));
    }
    let duration = if version == 0 && duration == u32::MAX as u64 { 0 } else { duration };
    let lang = r.u16()?;
    Ok((timescale, duration, decode_language(lang)))
}

/// ISO-639-2/T packed language, or a QuickTime Macintosh language code.
fn decode_language(code: u16) -> String {
    if code < 0x400 {
        // Macintosh language codes; only the common ones are worth mapping.
        return match code {
            0 => "eng", 1 => "fra", 2 => "deu", 3 => "ita", 4 => "nld", 5 => "swe",
            6 => "spa", 7 => "dan", 8 => "por", 9 => "nor", 10 => "heb", 11 => "jpn",
            12 => "ara", 13 => "fin", 14 => "ell", 19 => "zho", 23 => "kor", 32 => "rus",
            _ => "und",
        }
        .to_string();
    }
    let chars = [(code >> 10) & 0x1f, (code >> 5) & 0x1f, code & 0x1f];
    if chars.iter().any(|&c| !(1..=26).contains(&c)) {
        return "und".to_string();
    }
    chars.iter().map(|&c| (c as u8 + 0x60) as char).collect()
}

/// `hdlr`: the handler type (`vide`, `soun`, ...).
pub(crate) fn parse_hdlr(data: &[u8]) -> Result<FourCC> {
    let mut r = Reader::new(data, "hdlr");
    r.full_header()?;
    r.u32()?; // pre_defined / QuickTime component type
    r.fourcc()
}

/// `trex` defaults for fragments.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Trex {
    pub track_id: u32,
    pub duration: u32,
    pub size: u32,
    pub flags: u32,
}

pub(crate) fn parse_trex(data: &[u8]) -> Result<Trex> {
    let mut r = Reader::new(data, "trex");
    r.full_header()?;
    let track_id = r.u32()?;
    r.u32()?; // default_sample_description_index
    Ok(Trex {
        track_id,
        duration: r.u32()?,
        size: r.u32()?,
        flags: r.u32()?,
    })
}

/// `mehd`: total fragment duration in movie timescale.
pub(crate) fn parse_mehd(data: &[u8]) -> Result<u64> {
    let mut r = Reader::new(data, "mehd");
    let (version, _) = r.full_header()?;
    if version == 1 { r.u64() } else { Ok(r.u32()? as u64) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i32 = 0x10000;
    const W: i32 = 0x4000_0000;

    #[test]
    fn rotation_from_matrix() {
        assert_eq!(matrix_rotation(&[ONE, 0, 0, 0, ONE, 0, 0, 0, W]), (0, false));
        assert_eq!(matrix_rotation(&[0, ONE, 0, -ONE, 0, 0, 0, 0, W]), (90, false));
        assert_eq!(matrix_rotation(&[-ONE, 0, 0, 0, -ONE, 0, 0, 0, W]), (180, false));
        assert_eq!(matrix_rotation(&[0, -ONE, 0, ONE, 0, 0, 0, 0, W]), (270, false));
        assert_eq!(matrix_rotation(&[-ONE, 0, 0, 0, ONE, 0, 0, 0, W]), (0, true));
        assert_eq!(matrix_rotation(&[ONE, 0, 0, 0, -ONE, 0, 0, 0, W]), (180, true));
        assert_eq!(matrix_rotation(&[0, -ONE, 0, -ONE, 0, 0, 0, 0, W]), (90, true));
    }

    #[test]
    fn languages() {
        assert_eq!(decode_language(0x55c4), "und");
        assert_eq!(decode_language(0x15c7), "eng");
        assert_eq!(decode_language(0), "eng");
    }
}
