// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Capture and live production.
//!
//! * [`source`]: pictures and sound to record or switch between, with test
//!   sources that need no device; real cameras, microphones, screens and
//!   windows through [`device`] (permission asked only on the person's act).
//! * [`ingest`] / [`sync`]: cards in with SHA-256 verified copies to two
//!   drives, and separately recorded sound lined up with picture.

pub mod amf;
pub mod av;
pub mod cursor;
pub mod device;
pub mod frame;
pub mod hash;
pub mod ingest;
pub mod production;
pub mod prompter;
pub mod record;
pub mod replay;
pub mod rtmp;
pub mod sink;
pub mod srt;
pub mod stream;
pub mod source;
pub mod switcher;
pub mod text;
pub mod ts;
#[cfg(test)]
mod testutil;
pub mod vo;
pub mod wavout;
pub mod sync;
