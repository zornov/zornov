// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-h264`: a hand-written ITU-T H.264 / MPEG-4 AVC video decoder (std only).
//!
//! Supported: Baseline, Main, Extended (except SP/SI slices and data partitioning) and High
//! profile progressive streams with 8-bit 4:2:0 (or monochrome) video — CAVLC and CABAC,
//! I/P/B slices, Intra 4x4/8x8/16x16, I_PCM, all inter partitions, direct spatial and
//! temporal prediction, weighted prediction (explicit and implicit), 8x8 transforms,
//! scaling matrices, multiple slices, FMO/ASO, long-term references and all memory
//! management operations, frame_num gaps and display-order output through the DPB.
//! Interlaced streams (field pictures, MBAFF) are rejected with [`Error::Unsupported`].
//!
//! ```no_run
//! # fn demo(avcc: &[u8], samples: &[Vec<u8>]) -> studio_h264::Result<()> {
//! let mut dec = studio_h264::Decoder::from_avcc_bytes(avcc)?;
//! for s in samples {
//!     for frame in dec.decode(s)? {
//!         let _rgba = studio_h264::to_rgba(&frame);
//!     }
//! }
//! let _rest = dec.flush();
//! # Ok(()) }
//! ```

mod bitreader;
mod cabac;
mod cabac_tables;
mod cavlc;
mod color;
mod deblock;
mod dbpipe;
mod decoder;
mod error;
mod fmo;
mod inter;
mod intra;
mod mb;
mod mvpred;
mod nal;
mod params;
mod picture;
mod recon;
mod residual;
mod slice;
mod slicedec;
mod tables;
mod transform;
mod vlc;

pub use color::to_rgba;
pub use decoder::{ColorInfo, Decoder, Frame, Framing};
#[doc(hidden)]
pub use decoder::testing;
pub use error::{Error, Result};
pub use nal::{AvcConfig, split_annex_b, split_length_prefixed};
pub use params::{Pps, ScalingMatrices, Sps, Vui};
