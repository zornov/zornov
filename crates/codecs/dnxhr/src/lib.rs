// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! DNxHR (VC-3) encoder, written from SMPTE ST 2019-1:2016.
//!
//! * [`Encoder`] writes one progressive frame as a CBR coding unit: the 640-byte
//!   header (grown by 4 bytes per 16 lines above 1088), the macroblock scan-line
//!   index table, 8x8 DCT macroblocks with the Annex E codes, zero padding up to
//!   the frame size of equation 7.1, and the 4-byte end-of-frame signature.
//! * [`decode_frame`] is a reference decoder of the same format, used to check
//!   the encoder's output.
//!
//! Profiles: DNxHR LB (CID 1274), SQ (1273), HQ (1272), all 8-bit 4:2:2;
//! HQX (1271), 10-bit 4:2:2; 444 (1270), 10-bit 4:4:4. Any size from 1x1 to
//! 16384x16384 (4:2:2 needs an even width); edges are padded by replication.
//! The 444 profile is signalled as RGB with every macroblock coded as Y'CbCr (the
//! macroblock ACF flag); its [`Frame`] holds Y', Cb and Cr like the others, and the
//! reference decoder returns them unconverted. Not written: interlaced coding,
//! alpha, 12-bit, 4:2:0, variable bit rate.

mod bits;
mod dct;
mod decode;
mod encode;
mod header;
mod layout;
mod tables;
mod vlc;

pub use decode::{decode_frame, parse_header};
pub use encode::{Encoder, EncoderConfig};
pub use header::Header;

use vlc::Set;

/// Errors from encoding or decoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("truncated DNxHR data")]
    Truncated,
    #[error("invalid DNxHR data: {0}")]
    Invalid(&'static str),
    #[error("unsupported DNxHR feature: {0}")]
    Unsupported(&'static str),
    #[error("invalid encoder input: {0}")]
    Input(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Chroma sampling of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chroma {
    /// Chroma planes are half width, full height.
    Yuv422,
    /// Chroma planes are full size.
    Yuv444,
}

/// DNxHR profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Low bandwidth, CID 1274, 8-bit 4:2:2.
    Lb,
    /// Standard quality, CID 1273, 8-bit 4:2:2.
    Sq,
    /// High quality, CID 1272, 8-bit 4:2:2.
    Hq,
    /// High quality 10-bit, CID 1271, 4:2:2.
    Hqx,
    /// 4:4:4 10-bit, CID 1270.
    P444,
}

pub(crate) struct Params {
    pub set: Set,
    pub luma: &'static [u8; 64],
    pub chroma: &'static [u8; 64],
    /// Reference frame size C0 for 8160 macroblocks (Table C.2).
    pub c0: u64,
}

impl Profile {
    pub const ALL: [Profile; 5] = [Profile::Lb, Profile::Sq, Profile::Hq, Profile::Hqx, Profile::P444];

    /// Compression ID (Table C.2).
    pub fn cid(self) -> u32 {
        match self {
            Profile::Lb => 1274,
            Profile::Sq => 1273,
            Profile::Hq => 1272,
            Profile::Hqx => 1271,
            Profile::P444 => 1270,
        }
    }

    pub fn from_cid(cid: u32) -> Option<Profile> {
        Profile::ALL.into_iter().find(|p| p.cid() == cid)
    }

    pub fn bit_depth(self) -> u8 {
        match self {
            Profile::Hqx | Profile::P444 => 10,
            _ => 8,
        }
    }

    pub fn chroma(self) -> Chroma {
        if self == Profile::P444 { Chroma::Yuv444 } else { Chroma::Yuv422 }
    }

    pub fn name(self) -> &'static str {
        match self {
            Profile::Lb => "DNxHR LB",
            Profile::Sq => "DNxHR SQ",
            Profile::Hq => "DNxHR HQ",
            Profile::Hqx => "DNxHR HQX",
            Profile::P444 => "DNxHR 444",
        }
    }

    pub(crate) fn params(self) -> Params {
        use tables::*;
        match self {
            Profile::Lb => Params { set: Set::E4, luma: &W_D2_LUMA, chroma: &W_D2_CHROMA, c0: 188_416 },
            Profile::Sq => Params { set: Set::E4, luma: &W_D2_LUMA, chroma: &W_D2_CHROMA, c0: 606_208 },
            Profile::Hq => Params { set: Set::E7, luma: &W_D3_LUMA, chroma: &W_D3_CHROMA, c0: 917_504 },
            Profile::Hqx => Params { set: Set::E1, luma: &W_D1_LUMA, chroma: &W_D1_CHROMA, c0: 917_504 },
            Profile::P444 => Params { set: Set::E1, luma: &W_D11, chroma: &W_D11, c0: 1_835_008 },
        }
    }

    /// Compressed frame size in bytes for a raster (equation 7.1, no alpha).
    pub fn frame_size(self, width: u32, height: u32) -> usize {
        let nt = (width.div_ceil(16) as u64) * (height.div_ceil(16) as u64);
        let mut size = self.params().c0 * nt / 8160;
        let rem = size % 4096;
        if rem >= 2048 {
            size += 4096 - rem;
        } else {
            size -= rem;
        }
        size.max(8192) as usize
    }
}

/// Header size in bytes for a raster height (section 7.2).
pub fn header_size(height: u32) -> usize {
    if height <= 1088 { 640 } else { 640 + 4 * (height.div_ceil(16) as usize - 68) }
}

/// A planar limited-range Y'CbCr picture. Samples are `bit_depth` bits wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub chroma: Chroma,
    pub bit_depth: u8,
    pub y: Vec<u16>,
    pub cb: Vec<u16>,
    pub cr: Vec<u16>,
}

impl Frame {
    /// A frame of mid-grey.
    pub fn new(width: u32, height: u32, chroma: Chroma, bit_depth: u8) -> Frame {
        let cw = Frame::chroma_width_of(chroma, width) as usize;
        let mid = 1u16 << (bit_depth - 1);
        Frame {
            width,
            height,
            chroma,
            bit_depth,
            y: vec![mid; (width * height) as usize],
            cb: vec![mid; cw * height as usize],
            cr: vec![mid; cw * height as usize],
        }
    }

    pub fn chroma_width_of(chroma: Chroma, width: u32) -> u32 {
        match chroma {
            Chroma::Yuv422 => width.div_ceil(2),
            Chroma::Yuv444 => width,
        }
    }

    pub fn chroma_width(&self) -> u32 {
        Frame::chroma_width_of(self.chroma, self.width)
    }
}
