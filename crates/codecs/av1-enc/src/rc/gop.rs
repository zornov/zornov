// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M8 (phase c): key-frame placement, scene cuts, reference structure.

use crate::types::FrameType;

pub struct GopPlanner {}

impl GopPlanner {
    pub fn new(keyint: u32) -> GopPlanner {
        GopPlanner {}
    }
    /// The type of the next frame.
    pub fn next_type(&mut self) -> FrameType {
        unimplemented!("M8")
    }
}
