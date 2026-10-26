// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QuickTime masters.
//!
//! * [`mov`]: a QuickTime writer for ProRes masters: one ProRes video
//!   track (any of the six profiles, with `colr` / `fiel` / `pasp`), an
//!   optional 24-bit big-endian LPCM audio track, and an optional SMPTE
//!   timecode track carrying a reel name. Files over 4 GB use 64-bit
//!   offsets.
//! * [`prores`]: RGB(A) 16-bit pictures → ProRes frames (BT.709 or
//!   BT.2020 matrix, limited range; 10-bit 4:2:2 or 12-bit 4:4:4 with
//!   alpha).
//! * [`dnxhr`]: RGB(A) 16-bit pictures → DNxHR frames (`AVdh` video track
//!   through [`mov::MovWriter::create_dnxhr`]).
//! * [`probe`]: source timecode and reel name of a QuickTime / MP4 file
//!   (its `tmcd` track), for conform and EDLs.

pub mod dnxhr;
pub mod mov;
pub mod probe;
pub mod prores;

/// Timecode a master starts at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartTimecode {
    /// Frame number of the first frame at the nominal rate (e.g.
    /// 01:00:00:00 at 24 fps is 86400).
    pub frame: u32,
    /// Nominal frames per second of the labels (24, 25, 30, 60, …).
    pub fps: u32,
    pub drop_frame: bool,
    /// Reel / tape name written with it.
    pub reel: Option<String>,
}
