// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading a `.lottie` (dotLottie) archive: a ZIP of `manifest.json`,
//! `animations/*.json` and `images/*`. Stored and deflated entries, from
//! the central directory (PKWARE APPNOTE 6.3). [`write`] makes a stored
//! archive for export.

/// Largest entry read (an animation with its images).
const LIMIT: usize = 256 << 20;

fn u16le(b: &[u8], at: usize) -> Result<usize, String> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize).ok_or_else(|| "truncated .lottie archive".into())
}

fn u32le(b: &[u8], at: usize) -> Result<usize, String> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize).ok_or_else(|| "truncated .lottie archive".into())
}

/// Every file in the archive, (name, bytes).
pub fn entries(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    // End of central directory: the last "PK\5\6" within 64 KiB of the end.
    let tail = b.len().saturating_sub(65_557);
    let eocd = (tail..b.len().saturating_sub(21)).rev().find(|&i| &b[i..i + 4] == b"PK\x05\x06").ok_or("not a ZIP archive")?;
    let count = u16le(b, eocd + 10)?;
    let mut at = u32le(b, eocd + 16)?;
    let mut out = Vec::new();
    for _ in 0..count {
        if b.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err("damaged .lottie directory".into());
        }
        let method = u16le(b, at + 10)?;
        let csize = u32le(b, at + 20)?;
        let usize_ = u32le(b, at + 24)?;
        let (nlen, xlen, clen) = (u16le(b, at + 28)?, u16le(b, at + 30)?, u16le(b, at + 32)?);
        let local = u32le(b, at + 42)?;
        let name = String::from_utf8_lossy(b.get(at + 46..at + 46 + nlen).ok_or("truncated .lottie archive")?).into_owned();
        at += 46 + nlen + xlen + clen;
        if name.ends_with('/') || usize_ > LIMIT {
            continue;
        }
        if b.get(local..local + 4) != Some(b"PK\x03\x04") {
            return Err("damaged .lottie entry".into());
        }
        let data_at = local + 30 + u16le(b, local + 26)? + u16le(b, local + 28)?;
        let data = b.get(data_at..data_at + csize).ok_or("truncated .lottie entry")?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => studio_media::deflate::inflate(data, LIMIT).map_err(|e| format!("{name}: {e:?}"))?,
            m => return Err(format!("{name}: ZIP method {m} is not read")),
        };
        out.push((name, bytes));
    }
    Ok(out)
}

/// A ZIP of `files`, stored (uncompressed).
pub fn write(files: &[(&str, &[u8])]) -> Vec<u8> {
    let crc = |d: &[u8]| studio_media::png::crc32(&[d]);
    let mut out = Vec::new();
    let mut dir = Vec::new();
    let p16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let p32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    for (name, data) in files {
        let offset = out.len() as u32;
        let c = crc(data);
        out.extend_from_slice(b"PK\x03\x04");
        for v in [20u16, 0x0800, 0, 0, 0x21] {
            p16(&mut out, v);
        }
        p32(&mut out, c);
        p32(&mut out, data.len() as u32);
        p32(&mut out, data.len() as u32);
        p16(&mut out, name.len() as u16);
        p16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        dir.extend_from_slice(b"PK\x01\x02");
        for v in [20u16, 20, 0x0800, 0, 0, 0x21] {
            p16(&mut dir, v);
        }
        p32(&mut dir, c);
        p32(&mut dir, data.len() as u32);
        p32(&mut dir, data.len() as u32);
        p16(&mut dir, name.len() as u16);
        for v in [0u16, 0, 0, 0] {
            p16(&mut dir, v);
        }
        p32(&mut dir, 0);
        p32(&mut dir, offset);
        dir.extend_from_slice(name.as_bytes());
    }
    let dir_at = out.len() as u32;
    out.extend_from_slice(&dir);
    out.extend_from_slice(b"PK\x05\x06");
    for v in [0u16, 0, files.len() as u16, files.len() as u16] {
        p16(&mut out, v);
    }
    p32(&mut out, dir.len() as u32);
    p32(&mut out, dir_at);
    p16(&mut out, 0);
    out
}
