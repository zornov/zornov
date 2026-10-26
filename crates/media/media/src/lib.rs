// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio media: Studio's own codec and container, WAV and PNG files,
//! text rasterising and pixel operations. No third-party code.

pub mod aiff;
pub mod bits;
pub mod clv;
pub mod codec;
pub mod deflate;
pub mod flac;
pub mod gif;
pub mod mov;
pub mod jpeg;
pub mod mp3;
pub mod ogg;
pub mod vorbis;
pub mod par;
mod mp3_tables;
pub mod png;
pub mod rgba;
pub mod ttf;
pub mod wav;

pub use rgba::RgbaImage;
