// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fetched pieces into tracks: transport stream segments are taken apart,
//! MP4 fragments are joined behind their init segment and read.

use crate::aes::Aes128;
use crate::error::{Error, Result};
use crate::track::TrackData;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One fetched piece, in order.
#[derive(Debug, Clone)]
pub struct Piece {
    pub path: PathBuf,
    /// AES-128 key and IV when the piece is encrypted.
    pub key: Option<([u8; 16], [u8; 16])>,
    pub discontinuity: bool,
    /// Which init segment (index into `inits`) it belongs to.
    pub init: Option<usize>,
}

fn read_piece(p: &Piece) -> Result<Vec<u8>> {
    let data = std::fs::read(&p.path)?;
    match &p.key {
        Some((k, iv)) => Aes128::new(k).cbc_decrypt(iv, &data).map_err(Error::Bad),
        None => Ok(data),
    }
}

fn is_mp4_start(d: &[u8]) -> bool {
    d.len() >= 8 && matches!(&d[4..8], b"ftyp" | b"styp" | b"moof" | b"sidx" | b"moov" | b"mdat" | b"free" | b"skip")
}

fn is_packed_audio(d: &[u8]) -> bool {
    d.starts_with(b"ID3") || (d.len() > 2 && d[0] == 0xFF && d[1] & 0xF0 == 0xF0)
}

/// Turns the pieces of one stream (video, or sound) into tracks.
/// `notes` collects things worth telling the person.
pub fn assemble(work: &Path, tag: &str, inits: &[PathBuf], pieces: &[Piece], notes: &mut Vec<String>) -> Result<Vec<TrackData>> {
    let first = pieces.first().ok_or_else(|| Error::Bad("the stream lists no segments".into()))?;
    let head = read_piece(first)?;
    if !inits.is_empty() || is_mp4_start(&head) {
        return mp4_pieces(work, tag, inits, pieces, notes);
    }
    let mut col = crate::ts::Collector::new(work, tag);
    if crate::ts::looks_like_ts(&head) {
        for p in pieces {
            let data = if std::ptr::eq(p, first) { head.clone() } else { read_piece(p)? };
            col.begin_segment(p.discontinuity);
            col.push_segment(&data)?;
        }
    } else if is_packed_audio(&head) {
        for p in pieces {
            let data = if std::ptr::eq(p, first) { head.clone() } else { read_piece(p)? };
            col.begin_segment(p.discontinuity);
            col.push_packed_audio(&data)?;
        }
    } else {
        return Err(Error::Unsupported("the segments are in a format Studio does not read (it takes MPEG transport streams, MP4 fragments and AAC sound)".into()));
    }
    notes.extend(col.notes.clone());
    col.finish()
}

fn mp4_pieces(work: &Path, tag: &str, inits: &[PathBuf], pieces: &[Piece], notes: &mut Vec<String>) -> Result<Vec<TrackData>> {
    let joined = work.join(format!("{tag}-joined.mp4"));
    let mut out = std::io::BufWriter::new(std::fs::File::create(&joined)?);
    let mut first_init = None;
    if let Some(i) = inits.first() {
        out.write_all(&std::fs::read(i)?)?;
        first_init = Some(0usize);
    }
    let mut skipped = 0;
    for p in pieces {
        // A different init segment partway (an ad break, a new encode): its
        // pictures would not decode against the first one.
        if p.init.is_some() && first_init.is_some() && p.init != first_init {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            skipped += 1;
            continue;
        }
        out.write_all(&read_piece(p)?)?;
    }
    out.flush()?;
    drop(out);
    if skipped > 0 {
        notes.push(format!("The stream switches to a different encoding partway (an ad break, or a new part); only the part before it ({} of {} segments) was saved.", pieces.len() - skipped, pieces.len()));
    }
    crate::fmp4::tracks_from_mp4(&joined)
}
