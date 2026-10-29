// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M5: reconstruction buffers and the reference store (spec 7.20).
//!
//! [`FrameBuf`] is the encoder's copy of the picture the decoder will rebuild. Intra blocks are
//! reconstructed into it with [`FrameBuf::reconstruct_block`], which follows the decoder's
//! `residual()` / `transform_block()` order exactly (spec 5.11.34, 5.11.35, 7.11.2, 7.12.3,
//! 7.13.3) and calls the decoder's own `predict_intra` and `inverse_transform_2d`, so the pixels
//! match bit for bit.

pub mod inter;
pub mod intra;

use std::sync::Arc;
use studio_av1::frame::Plane;

pub use intra::{IntraExtra, QuantInfo};

/// The encoder's reconstruction of a frame: what the decoder will reproduce.
///
/// Planes are allocated like the decoder's (`studio_av1::frame::FrameBuf`): the coded area rounded
/// up to the 128-sample grid plus slack, so blocks that extend past the frame edge are written
/// in full. Use [`FrameBuf::to_frame`] for the cropped picture.
#[derive(Default, Clone)]
pub struct FrameBuf {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub planes: [Plane; 3],
    /// Quantiser state used to dequantise coefficients.
    pub quant: QuantInfo,
    /// `enable_intra_edge_filter` of the sequence header.
    pub enable_intra_edge_filter: bool,
    pub(crate) intra: intra::IntraState,
}

impl FrameBuf {
    /// A zeroed 4:2:0 reconstruction buffer for a `width` x `height` luma picture.
    pub fn new(width: u32, height: u32, bit_depth: u8) -> FrameBuf {
        let (w, h) = (width as usize, height as usize);
        let aw = w.div_ceil(128) * 128 + 160;
        let ah = h.div_ceil(128) * 128 + 160;
        let planes = [
            Plane::new(aw, ah),
            Plane::new(aw >> 1, ah >> 1),
            Plane::new(aw >> 1, ah >> 1),
        ];
        FrameBuf {
            width,
            height,
            bit_depth,
            planes,
            quant: QuantInfo::default(),
            enable_intra_edge_filter: true,
            intra: intra::IntraState::new(w, h),
        }
    }

    /// The reconstruction cropped to the picture size, as an encoder-side [`crate::types::Frame`].
    pub fn to_frame(&self) -> crate::types::Frame {
        let mut f = crate::types::Frame::new(self.width, self.height, self.bit_depth);
        for p in 0..3 {
            let (w, h) = if p == 0 {
                (self.width as usize, self.height as usize)
            } else {
                (
                    (self.width as usize + 1) >> 1,
                    (self.height as usize + 1) >> 1,
                )
            };
            for y in 0..h {
                f.planes[p][y * f.stride[p]..y * f.stride[p] + w]
                    .copy_from_slice(&self.planes[p].row(y)[..w]);
            }
        }
        f
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
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if refresh_frame_flags >> i & 1 == 1 {
                *slot = Some(frame.clone());
            }
        }
    }
}
