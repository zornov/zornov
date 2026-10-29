// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M8 (phase c): rate control.

pub mod gop;

use crate::types::{Config, FrameType};

pub struct RateControl {}

impl RateControl {
    pub fn new(cfg: &Config) -> RateControl {
        RateControl {}
    }
    /// The `base_q_idx` for the next frame.
    pub fn frame_qidx(&mut self, frame_type: FrameType) -> u8 {
        unimplemented!("M8")
    }
    /// Reports the size of the frame just coded.
    pub fn update(&mut self, bytes: usize) {
        unimplemented!("M8")
    }
}
