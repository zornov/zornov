// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Neural-network engine benchmark. The GPU rows use Metal, so the full
//! benchmark runs on macOS; elsewhere it says so and exits.

#[cfg(target_os = "macos")]
include!("bench_metal.rs.inc");

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The nn benchmark needs Metal (macOS).");
}
