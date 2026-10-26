// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-encode`: hand-written encoders for standard export formats (std only).
//!
//! - [`h264`]: an ITU-T H.264 / AVC encoder: Baseline (CAVLC) and Main (CABAC)
//!   profiles, 8-bit 4:2:0, IDR + P frames, Intra 4x4 / 16x16, 16x16 motion
//!   compensation with half-sample motion search, in-loop deblocking, CBR / VBR
//!   rate control and BT.709 VUI.
//! - [`aac`]: an MPEG-4 AAC-LC encoder (long blocks, sine window, bit-rate driven
//!   quantisation, Huffman coded spectra) producing raw access units plus the
//!   `AudioSpecificConfig`.
//! - [`mux`]: an ISO-BMFF writer for `.mp4` and QuickTime `.mov` files with H.264
//!   and AAC tracks, optionally fast-start (`moov` before `mdat`).
//! - [`yuv`]: RGBA to Y'CbCr 4:2:0 conversion (BT.709, limited range).
//!
//! ```no_run
//! use studio_encode::{h264, aac, mux, yuv};
//! let mut venc = h264::Encoder::new(h264::Config::new(1280, 720, 30, 1)).unwrap();
//! let mut aenc = aac::Encoder::new(aac::Config { sample_rate: 48_000, channels: 2, bitrate: 192_000 }).unwrap();
//! let mut mp4 = mux::Muxer::create("out.mp4", mux::Options::mp4()).unwrap();
//! let v = mp4.add_video(venc.track_info()).unwrap();
//! let a = mp4.add_audio(aenc.track_info()).unwrap();
//! # let rgba = vec![0u8; 1280 * 720 * 4];
//! let pic = yuv::Yuv420::from_rgba(&rgba, 1280, 720);
//! let f = venc.encode(&pic);
//! mp4.write_sample(v, &f.data, f.keyframe).unwrap();
//! for au in aenc.push(&vec![0.0f32; 1600 * 2]) { mp4.write_sample(a, &au, true).unwrap(); }
//! for au in aenc.flush() { mp4.write_sample(a, &au, true).unwrap(); }
//! mp4.finish().unwrap();
//! ```

pub mod aac;
pub mod bits;
pub mod h264;
pub mod mux;
pub mod yuv;
