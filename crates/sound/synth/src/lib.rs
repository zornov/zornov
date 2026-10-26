// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Local sound creation for Studio. Everything here is synthesised in Rust:
//! no key, no network and no recorded samples.
//!
//! - [`music`]: a composition engine (structure, harmony, melody, bass,
//!   drums, arrangement) that writes original music in a genre, fitted to a
//!   length and to cue points.
//! - [`instrument`]: the instruments it plays (physical-model strings,
//!   additive piano, FM, source-filter winds, synths, a drum kit).
//! - [`sfx`]: foley and sound effects from physical models, chosen from a
//!   description.
//! - [`animal`]: animal voices by source and formant filter.
//! - [`foley`]: footsteps on many surfaces.
//! - [`sing`]: a formant singing voice, from notes and syllables.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod animal;
pub mod dsp;
pub mod instrument;
pub mod foley;
pub mod level;
pub mod music;
pub mod sfx;
pub mod sing;
pub mod style;
pub mod theory;
