// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-mp4`: a std-only ISO-BMFF (MP4 / MOV / M4A) demuxer.
//!
//! The parser walks top-level boxes over any `Read + Seek` source, reading only
//! box headers plus the metadata payloads it needs (`ftyp`, `moov`, `moof`);
//! media data (`mdat`) is never loaded until [`Demuxer::read_sample`] asks for a
//! single sample. Both classic sample tables and fragmented MP4 are expanded into
//! one flat, decode-ordered [`Sample`] list per [`Track`].
//!
//! ```no_run
//! let mut demuxer = studio_mp4::open("clip.mp4")?;
//! let track = demuxer.movie().tracks[0].clone();
//! let seek = studio_mp4::sample_for_time(&track, 1.5).expect("non-empty track");
//! for sample in &track.samples[seek.sync_index..=seek.target_index] {
//!     let data = demuxer.read_sample(sample)?;
//!     # let _ = data;
//! }
//! # Ok::<(), studio_mp4::Error>(())
//! ```
//!
//! Module map:
//! - `box`: box headers and bounds-checked byte readers.
//! - `boxes`: `moov`-level boxes (`mvhd`, `tkhd`, `elst`, `mdhd`, `hdlr`, `trex`).
//! - `stbl`: sample tables and their expansion into [`Sample`]s.
//! - `stsd`: sample entries (visual / sound) and their child boxes.
//! - [`codec`]: `avcC` / `hvcC` / `av1C` decoder configuration records.
//! - [`aac`]: `esds` descriptors and `AudioSpecificConfig`.
//! - `fragment`: `moof` / `traf` / `trun` expansion.
//! - [`demux`]: the public [`Demuxer`], [`Movie`], [`Track`] and timing helpers.
//! - [`nal`]: length-prefixed NAL unit splitting and Annex-B conversion.

pub mod aac;
mod r#box;
mod boxes;
pub mod codec;
pub mod demux;
mod error;
mod fragment;
pub mod nal;
mod stbl;
mod stsd;

pub use aac::AudioSpecificConfig;
pub use boxes::matrix_rotation;
pub use r#box::FourCC;
pub use codec::{Av1Config, AvcConfig, HevcConfig, HevcNalArray};
pub use demux::{
    open, sample_for_time, Bitrate, ChannelLayout, Codec, ColorInfo, Demuxer, Edit, Movie,
    Sample, SeekPoint, Track, TrackKind,
};
pub use error::{Error, Result};
pub use nal::{nals_to_annex_b, split_nals, to_annex_b, NalIter};
