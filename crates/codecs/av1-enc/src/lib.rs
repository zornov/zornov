// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-av1-enc`: a clean-room AV1 encoder, written from the AV1 specification only.
//!
//! This is the wave-0 skeleton: every module exists with its public interface and returns
//! `Error::Unsupported` or panics with `unimplemented!` until the module that owns it is built.
//! Ownership (one worktree per module, no file is shared):
//!
//! | module | owner | files |
//! |---|---|---|
//! | `types`, `error`, `lib.rs` | core | this file, `types.rs`, `error.rs`, `Cargo.toml` |
//! | `bits`, `obu` | M1 | `src/bits/`, `src/obu/` |
//! | `entropy` | M2 | `src/entropy/` |
//! | `transform` | M3 | `src/transform/` |
//! | `syntax` | M4 (`inter_info.rs`: M7) | `src/syntax/` |
//! | `recon` | M5 | `src/recon/` |
//! | `mode` | M6 | `src/mode/` |
//! | `motion` | M7 | `src/motion/`, `src/syntax/inter_info.rs` |
//! | `rc` | M8 | `src/rc/` |
//! | `filters` | M9 | `src/filters/` |
//! | `tiles` | M10 | `src/tiles/` |
//! | `color`, `encoder` | facade | `src/color/`, `src/encoder.rs` |
//!
//! A module uses another module's items only if they appear in [`types`] or in that module's
//! public `mod.rs` API.

pub mod bits;
pub mod color;
pub mod encoder;
pub mod entropy;
pub mod error;
pub mod filters;
pub mod mode;
pub mod motion;
pub mod obu;
pub mod rc;
pub mod recon;
pub mod syntax;
pub mod tiles;
pub mod transform;
pub mod types;

pub use encoder::Encoder;
pub use error::{Error, Result};
pub use types::{Config, Frame, Packet, RateMode};
