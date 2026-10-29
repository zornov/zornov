// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M1: uncompressed frame header writer (spec 5.9).

use crate::bits::BitWriter;
use crate::types::{FrameParams, SeqParams};

pub fn write_frame_header(seq: &SeqParams, fh: &FrameParams, w: &mut BitWriter) {
    unimplemented!("M1")
}
