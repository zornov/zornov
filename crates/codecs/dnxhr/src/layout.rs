// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! DCT block order inside a macroblock (ST 2019-1 Tables 5 and 6).

use crate::Chroma;

/// Component type of a block; also the DC predictor it uses.
pub const Y: usize = 0;
pub const CB: usize = 1;
pub const CR: usize = 2;

/// (component type, x offset, y offset) of each block, in bit-stream order.
/// Offsets are in samples of the block's own plane, from the macroblock origin.
pub fn blocks(chroma: Chroma) -> &'static [(usize, usize, usize)] {
    match chroma {
        Chroma::Yuv422 => &[(Y, 0, 0), (Y, 8, 0), (CB, 0, 0), (CR, 0, 0), (Y, 0, 8), (Y, 8, 8), (CB, 0, 8), (CR, 0, 8)],
        Chroma::Yuv444 => &[
            (Y, 0, 0),
            (Y, 8, 0),
            (CB, 0, 0),
            (CB, 8, 0),
            (CR, 0, 0),
            (CR, 8, 0),
            (Y, 0, 8),
            (Y, 8, 8),
            (CB, 0, 8),
            (CB, 8, 8),
            (CR, 0, 8),
            (CR, 8, 8),
        ],
    }
}

/// Width of a macroblock in samples of a component's plane.
pub fn mb_width(chroma: Chroma, ct: usize) -> usize {
    if ct == Y || chroma == Chroma::Yuv444 { 16 } else { 8 }
}
