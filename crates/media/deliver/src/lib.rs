// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finishing and delivery.
//!
//! Everything here is pure (no window, no project model): the app and the
//! headless agent feed it pictures, sound and file names.
//!
//! - [`qc`]: automated quality control with a report.
//! - [`specs`]: the delivery specifications of platforms and mastering.
pub mod qc;
pub mod specs;
pub mod sha256;
pub mod chunks;
pub mod wire;
pub mod merge;
#[cfg(test)]
mod testutil;
pub mod filecheck;
pub mod queue;
pub mod j2k;
pub mod mxf;
pub mod sha1;
pub mod dcp;
pub mod imf;
pub mod bundle;
pub mod cut;
