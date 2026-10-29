// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! Facade: RGBA to planar 4:2:0 `u16` (BT.709), 8-bit and 16-bit input.

use crate::types::Frame;

/// `rgba` is `width * height * 4` bytes. `bit_depth` of the result is 8 or 10.
pub fn from_rgba8(rgba: &[u8], width: u32, height: u32, bit_depth: u8, full_range: bool) -> Frame {
    unimplemented!("facade")
}

/// `rgba` is `width * height * 4` 16-bit samples.
pub fn from_rgba16(rgba: &[u16], width: u32, height: u32, bit_depth: u8, full_range: bool) -> Frame {
    unimplemented!("facade")
}
