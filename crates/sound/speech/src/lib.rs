// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Speech to mouth.
//!
//! 1. [`align`]: the words of a line (from speech recognition or the
//!    narration's own text) become phonemes with times, laid on the audio.
//! 2. [`viseme`] and [`coart`]: phonemes become mouth channels (jaw, lip
//!    closure, rounding, stretch, tongue, blinks, brows, gaze) that blend
//!    neighbours, lead the sound, hold lip closures and soften fast speech.
//! 3. [`drive`]: the channels become keys for whatever character speaks:
//!    3D shape keys and bones, 2D puppet mouth poses, a drawn mouth chart.
//! 4. [`check`]: finds a mouth moving in silence, still while speaking,
//!    missing a closure, or out of sync, each with the fix.
//!
//! Pronunciations come from CMUdict (bundled) with letter-to-sound rules
//! for words it lacks. English only.

pub mod align;
pub mod check;
pub mod coart;
pub mod dict;
pub mod drive;
pub mod extras;
pub mod features;
pub mod lts;
pub mod phoneme;
pub mod viseme;

pub use align::{Alignment, Method, WordIn, align, estimate};
pub use coart::{Curves, Emotion, Style, curves};
pub use phoneme::{Ph, Timed};
pub use viseme::{Channel, Viseme};
