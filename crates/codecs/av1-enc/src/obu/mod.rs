// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M1: OBU framing (spec 5.3).

pub mod frame_header;
pub mod seq_header;

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
    unimplemented!("M1")
}
