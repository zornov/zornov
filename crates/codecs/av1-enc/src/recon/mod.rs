// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M5: reconstruction buffers and the reference store (spec 7.20).

pub mod inter;
pub mod intra;

use std::sync::Arc;
use studio_av1::frame::Plane;

/// The encoder's reconstruction of a frame: what the decoder will reproduce.
#[derive(Default, Clone)]
pub struct FrameBuf {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub planes: [Plane; 3],
}

impl FrameBuf {
    pub fn new(width: u32, height: u32, bit_depth: u8) -> FrameBuf {
        unimplemented!("M5")
    }
}

/// The eight reference slots.
#[derive(Default)]
pub struct RefStore {
    pub slots: [Option<Arc<FrameBuf>>; 8],
}

impl RefStore {
    /// Stores `frame` in every slot whose bit is set in `refresh_frame_flags`.
    pub fn refresh(&mut self, refresh_frame_flags: u8, frame: Arc<FrameBuf>) {
        unimplemented!("M5")
    }
}
