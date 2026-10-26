// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-hevc`: a hand-written H.265 / HEVC video decoder (ITU-T H.265).
//!
//! Supported: Main and Main 10 profiles (4:2:0, 8..=12 bit sample storage),
//! progressive coding, all Main-profile coding tools including tiles,
//! wavefront parallel processing (decoded sequentially), dependent slice
//! segments, scaling lists, PCM, transform skip, transquant bypass, AMP,
//! long-term reference pictures, weighted prediction, deblocking and SAO.
//! Output frames are cropped to the conformance window and delivered in
//! display order.
//!
//! ```no_run
//! # fn demo(hvcc: &[u8], samples: &[Vec<u8>]) -> studio_hevc::Result<()> {
//! let mut dec = studio_hevc::Decoder::from_hvcc(hvcc)?;
//! for s in samples {
//!     for frame in dec.decode(s)? {
//!         let _rgba = frame.to_rgba();
//!     }
//! }
//! for _frame in dec.flush() {}
//! # Ok(()) }
//! ```

pub mod bitreader;
pub mod cabac;
mod ctu;
mod deblock;
mod debug;
mod decoder;
mod dpb;
mod error;
mod frame;
mod inter;
mod intra;
pub mod nal;
pub mod params;
mod picture;
mod pool;
#[cfg(test)]
mod mt_tests;
mod sao;
pub mod slice;
#[cfg(feature = "synth")]
pub mod synth;
mod transform;
mod wpp;

pub use decoder::Decoder;
pub use error::{Error, Result};
pub use frame::{ColorInfo, DownConvert, Frame, Matrix, PlaneData};

/// Parses an `hvcC` box payload (HEVCDecoderConfigurationRecord).
/// Returns the NAL length size and the parameter set NAL units it carries.
pub fn parse_hvcc(data: &[u8]) -> Result<(usize, Vec<Vec<u8>>)> {
    if data.len() < 23 {
        return Err(Error::UnexpectedEof);
    }
    let length_size = (data[21] & 3) as usize + 1;
    let num_arrays = data[22] as usize;
    let mut pos = 23usize;
    let mut out = Vec::new();
    for _ in 0..num_arrays {
        if pos + 3 > data.len() {
            return Err(Error::UnexpectedEof);
        }
        let n = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;
        for _ in 0..n {
            if pos + 2 > data.len() {
                return Err(Error::UnexpectedEof);
            }
            let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if pos + len > data.len() {
                return Err(Error::UnexpectedEof);
            }
            out.push(data[pos..pos + len].to_vec());
            pos += len;
        }
    }
    Ok((length_size, out))
}
