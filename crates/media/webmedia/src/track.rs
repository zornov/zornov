// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One track gathered from segments, ready to be written into an MP4: its
//! sample entry, its samples (where each one's bytes are in `store`) and
//! its timing.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Offset of the sample's bytes in the track's store file.
    pub offset: u64,
    pub size: u32,
    /// Decode and presentation time in the track's timescale (the source's
    /// own clock; not shifted to start at zero).
    pub dts: i64,
    pub pts: i64,
    pub dur: u32,
    pub sync: bool,
}

#[derive(Debug, Clone)]
pub struct TrackData {
    /// `vide` or `soun`.
    pub handler: [u8; 4],
    pub timescale: u32,
    /// The whole sample entry box (`avc1`, `hvc1`, `mp4a`, `ec-3`, `av01`...).
    pub entry: Vec<u8>,
    /// Display size (video).
    pub width: u32,
    pub height: u32,
    /// ISO 639-2 language, three letters.
    pub lang: String,
    /// `tkhd` matrix (rotation), identity when the source had none.
    pub matrix: [i32; 9],
    /// Media time (in `timescale`) at which playback starts, when the
    /// source's edit list says so (hides AAC priming); 0 = at the first
    /// presented sample.
    pub edit_offset: i64,
    pub samples: Vec<Sample>,
    /// The file the samples' bytes are read from.
    pub store: PathBuf,
}

pub const IDENTITY: [i32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

impl TrackData {
    pub fn is_video(&self) -> bool {
        &self.handler == b"vide"
    }
    pub fn is_audio(&self) -> bool {
        &self.handler == b"soun"
    }
}
