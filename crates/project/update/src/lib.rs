// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's updater.
//!
//! * [`config`]: the feed URL and public key (`packaging/release.conf`).
//! * [`feed`]: the signed feed, its Ed25519 check and version order.
//! * [`apply`]: unpacking an update and swapping it in on restart.
//! * [`state`]: the background checker the app shows in Settings.
//!
//! The feed lists, per platform, an archive (`.tar.gz` of the install
//! folder), its SHA-256 and size, and an Ed25519 signature over those, the
//! platform and the version. Nothing is installed unless the signature
//! checks against the public key built into the app.

pub mod apply;
pub mod config;
pub mod feed;
pub mod state;

/// The version this build was made as.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Zornov's per-user data folder (settings, staged updates, crash reports,
/// the signing key): `~/Library/Application Support/Zornov`,
/// `%LOCALAPPDATA%\Zornov` or `$XDG_STATE_HOME/zornov`. A folder left by the
/// app's earlier name (`Studio`) is moved over on first use.
pub fn data_dir() -> std::path::PathBuf {
    studio_paths::data_dir()
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bytes from hex (either case), or `None`.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}
