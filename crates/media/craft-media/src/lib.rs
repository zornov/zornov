//! Bit-level I/O and a clean-room Matroska / WebM demuxer and muxer.
//!
//! Vendored from craft-libs (<https://github.com/storytold/craft-libs>,
//! `crates/media/media`, revision 6e7f28518dd1cb57e1624bdc269c3c14316b13ae):
//! `bitstream.rs` and `matroska/` unchanged apart from paths. Copyright (c)
//! 2026 ArtCraft Team and the craft-libs contributors; MIT OR Apache-2.0
//! (`LICENSE-MIT`, `LICENSE-APACHE` in this folder). Studio reads WebM and
//! MKV files with it (`studio-app`'s `transcode`).

pub mod bitstream;
pub mod matroska;
