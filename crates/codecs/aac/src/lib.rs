// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! studio-aac: a hand-written MPEG-4 AAC decoder (ISO/IEC 14496-3), std
//! only, for AAC LC, HE-AAC v1 (SBR) and HE-AAC v2 (Parametric Stereo).
//!
//! ```no_run
//! use studio_aac::Decoder;
//! # fn access_units() -> Vec<Vec<u8>> { Vec::new() }
//! // `asc` is the AudioSpecificConfig from the MP4 `esds` box.
//! let asc = [0x13, 0x90, 0x56, 0xe5, 0xa0];
//! let mut dec = Decoder::new(&asc)?;
//! let mut out = Vec::new();
//! for au in access_units() {
//!     let pcm: &[f32] = dec.decode_frame(&au)?; // interleaved, [-1, 1]
//!     out.extend_from_slice(pcm);
//! }
//! // Rate / channels may switch on the first frame with implicit SBR / PS.
//! let (_rate, _channels) = (dec.sample_rate(), dec.channels());
//! # Ok::<(), studio_aac::Error>(())
//! ```
//!
//! Output is interleaved `f32` at [`Decoder::sample_rate`] (twice the core
//! rate with SBR) with [`Decoder::channels`] channels (mono + PS gives 2),
//! in the element order of the channel configuration
//! ([`Decoder::channel_layout`]). Every access unit yields
//! [`Decoder::samples_per_frame`] samples per channel; the stream starts
//! with the encoder priming signalled by the container, plus
//! [`Decoder::output_delay`] samples of SBR delay.
//!
//! Modules: `bits` (bit reader), `config` (AudioSpecificConfig, PCE, ADTS),
//! `huffman` + `huffman_tables` (code books), `ics` (ics_info, sections,
//! scale factors, pulses, spectral data), `spectral` (inverse
//! quantisation), `tns`, `pns`, `stereo` (M/S, intensity), `cce`
//! (coupling), `mdct` + `fft` + `filterbank` (IMDCT, windows, overlap-add),
//! `sbr/*` (bitstream, frequency tables, QMF, HF generation and
//! adjustment), `ps/*` (parametric stereo), `decoder` (raw_data_block and
//! the reconstruction chain).

mod bits;
mod cce;
mod config;
mod decoder;
mod error;
mod fft;
mod filterbank;
mod huffman;
mod huffman_tables;
mod ics;
mod mdct;
mod pns;
mod ps;
mod sbr;
mod spectral;
mod stereo;
mod tables;
mod tns;

pub use config::{
    AdtsHeader, AudioSpecificConfig, ElementKind, ElementSlot, ProgramConfig, Signal, Speaker, SAMPLE_RATES,
};
pub use decoder::{Decoder, DecoderStats};
pub use error::{Error, Result};
