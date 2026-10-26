// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's shared test kit.
//!
//! Everything a test suite needs that is not the thing under test:
//!
//! - [`rng`]: a seeded generator so every test is deterministic.
//! - [`fixtures`]: generated test media kept in `target-qa/fixtures`, with a
//!   committed manifest of hashes so a changed generator is noticed.
//! - [`movie`]: fake movies (colour bars with a burnt-in frame counter,
//!   moving shapes, a ticking tone) written with the `encode` crate, each
//!   with a JSON ground truth; [`movie::read_counter`] reads the frame
//!   index back out of any decoded picture.
//! - [`images`]: PNG and JPEG files with known pixel values in every colour
//!   type, bit depth and interlace mode, EXIF rotation, and truncation.
//! - [`audio`]: analytic signals and measurements.
//! - [`mocks`]: `MockDecoder`, `MockAudioOut`, `MockNn`, `MockClock`, `MockFs`.
//! - [`golden`]: the tolerant image comparator with `UPDATE_GOLDEN=1` and an
//!   HTML diff report.
//! - [`strategies`]: `proptest` strategies for projects, clips, keyframes
//!   and effect parameter sets.
//!
//! Nothing here is copied from any other product; every fixture is
//! synthetic and produced by our own code.

#![forbid(unsafe_code)]

pub mod audio;
pub mod fixtures;
pub mod golden;
pub mod images;
pub mod mocks;
pub mod movie;
pub mod rng;
pub mod sha256;
pub mod strategies;

pub use fixtures::{fixture, fixtures_dir, qa_target_dir, scratch_dir};
pub use golden::{Rgba8, Tolerance, check_golden};
pub use mocks::{MockAudioOut, MockClock, MockDecoder, MockFs, MockNn};
pub use rng::Rng;
