// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The OFX worker process: `studio-ofx-worker ofx-worker <bundle>`. The
//! Studio app runs the same code as `studio ofx-worker <bundle>`.

fn main() {
    studio_plugins::ofx::worker_if_asked();
    eprintln!("usage: studio-ofx-worker ofx-worker <path to .ofx.bundle>");
    std::process::exit(64);
}
