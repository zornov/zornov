// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tracks out of MP4 sources: a file, or an init segment followed by
//! fragments. Studio's own MP4 reader expands the sample tables; this
//! adds each track's sample entry as it was written, so any codec can be
//! copied into the new file.

use crate::error::{Error, Result};
use crate::track::{Sample, TrackData};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The boxes in `data`: (type, payload).
pub fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        let mut size = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let t = [data[i + 4], data[i + 5], data[i + 6], data[i + 7]];
        let mut hdr = 8;
        if size == 1 && i + 16 <= data.len() {
            size = u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap()) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - i;
        }
        if size < hdr || i + size > data.len() {
            break;
        }
        out.push((t, &data[i + hdr..i + size]));
        i += size;
    }
    out
}

fn child<'a>(data: &'a [u8], t: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(k, _)| k == t).map(|(_, p)| p)
}

/// Each track's id and its first sample entry box (complete, with header).
fn entries(moov: &[u8]) -> HashMap<u32, Vec<u8>> {
    let mut out = HashMap::new();
    for (t, trak) in boxes(moov) {
        if &t != b"trak" {
            continue;
        }
        let Some(tkhd) = child(trak, b"tkhd") else { continue };
        let id = if tkhd.first() == Some(&1) { tkhd.get(20..24) } else { tkhd.get(12..16) };
        let Some(id) = id.map(|b| u32::from_be_bytes(b.try_into().unwrap())) else { continue };
        let stsd = child(trak, b"mdia").and_then(|m| child(m, b"minf")).and_then(|m| child(m, b"stbl")).and_then(|m| child(m, b"stsd"));
        let Some(stsd) = stsd else { continue };
        if stsd.len() < 16 {
            continue;
        }
        let size = u32::from_be_bytes(stsd[8..12].try_into().unwrap()) as usize;
        if size >= 8 && 8 + size <= stsd.len() {
            out.insert(id, stsd[8..8 + size].to_vec());
        }
    }
    out
}

/// Reads the top-level `moov` of `path`.
fn read_moov(path: &Path) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        let n = f.read(&mut h)?;
        if n < 8 {
            break;
        }
        let mut size = u32::from_be_bytes(h[0..4].try_into().unwrap()) as u64;
        let mut hdr = 8;
        if size == 1 && n >= 16 {
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr {
            break;
        }
        if &h[4..8] == b"moov" {
            if size - hdr > (256 << 20) {
                return Err(Error::Bad("the index of the video is unreasonably large".into()));
            }
            let mut v = vec![0u8; (size - hdr) as usize];
            f.seek(SeekFrom::Start(pos + hdr))?;
            f.read_exact(&mut v)?;
            return Ok(v);
        }
        pos += size;
    }
    Err(Error::Bad("the video has no index (moov) to read".into()))
}

const PROTECTED: &str = "This video is encrypted with a content protection scheme (DRM). Zornov does not work around copy protection, so it cannot be downloaded.";

/// All the video and sound tracks of an MP4 file (whole or fragmented).
pub fn tracks_from_mp4(path: &Path) -> Result<Vec<TrackData>> {
    let moov = read_moov(path)?;
    let entry_of = entries(&moov);
    drop(moov);
    let demux = studio_mp4::open(path).map_err(|e| Error::Bad(format!("the video's index could not be read: {e}")))?;
    let mut out = Vec::new();
    for t in &demux.movie().tracks {
        let handler = match t.kind {
            studio_mp4::TrackKind::Video => *b"vide",
            studio_mp4::TrackKind::Audio => *b"soun",
            studio_mp4::TrackKind::Other => continue,
        };
        let Some(entry) = entry_of.get(&t.id) else { continue };
        if matches!(&entry[4..8], b"encv" | b"enca" | b"enct" | b"encs") {
            return Err(Error::Protected(PROTECTED.into()));
        }
        let samples: Vec<Sample> = t.samples.iter().map(|s| Sample { offset: s.offset, size: s.size, dts: s.dts, pts: s.pts, dur: s.duration, sync: s.sync }).collect();
        if samples.is_empty() {
            continue;
        }
        out.push(TrackData {
            handler,
            timescale: t.timescale.max(1),
            entry: entry.clone(),
            width: t.width,
            height: t.height,
            lang: t.language.clone(),
            matrix: t.matrix,
            edit_offset: if t.edits.is_empty() { 0 } else { t.edit_offset },
            samples,
            store: path.to_path_buf(),
        });
    }
    if out.is_empty() {
        return Err(Error::Unsupported("the file holds no video or sound track Zornov can take".into()));
    }
    Ok(out)
}
