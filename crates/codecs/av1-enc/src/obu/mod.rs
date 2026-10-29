// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M1: OBU framing (spec 5.3).

pub mod frame_header;
pub mod seq_header;

use crate::bits::BitWriter;

pub use frame_header::{frame_header_obu, write_frame_header};
pub use seq_header::{av1c, write_sequence_header};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObuType {
    SequenceHeader = 1,
    TemporalDelimiter = 2,
    FrameHeader = 3,
    TileGroup = 4,
    Metadata = 5,
    Frame = 6,
    Padding = 15,
}

/// One OBU with `obu_has_size_field = 1`: header byte, leb128 size, payload.
pub fn obu(kind: ObuType, payload: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(0, 1); // obu_forbidden_bit
    w.put(kind as u32, 4);
    w.put(0, 1); // obu_extension_flag
    w.put(1, 1); // obu_has_size_field
    w.put(0, 1); // obu_reserved_1bit
    w.leb128(payload.len() as u64);
    w.put_bytes(payload);
    w.finish()
}

/// A temporal delimiter OBU (empty payload). Not part of MP4 samples; used for raw `.obu` streams
/// and by tests.
pub fn temporal_delimiter() -> Vec<u8> {
    obu(ObuType::TemporalDelimiter, &[])
}
