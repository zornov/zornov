// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M1: sequence header OBU writer (spec 5.5) and the `av1C` record.

use crate::types::SeqParams;

/// The sequence header OBU payload (without OBU framing).
pub fn write_sequence_header(seq: &SeqParams) -> Vec<u8> {
    unimplemented!("M1")
}

/// The `av1C` record for the MP4 sample entry: four fixed bytes followed by the sequence header
/// OBU (`configOBUs`).
pub fn av1c(seq: &SeqParams) -> Vec<u8> {
    unimplemented!("M1")
}
