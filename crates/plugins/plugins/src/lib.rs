// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's plugin host.
//!
//! Plugins are WebAssembly modules written in Rust with
//! `studio-plugin-api` and run sandboxed in wasmtime ([`runtime`]). An
//! enabled plugin's effects, generators and transitions join the `fx`
//! catalogue as `<plugin>:<item>` ([`bridge`]), so every part of Studio
//! uses them like built-in ones; importers, exporters and panels are
//! called through [`Host`]. [`catalogue`] lists what can be installed and
//! [`author`] builds plugins written inside Studio (by the AI). [`ofx`]
//! loads OpenFX image effects, each bundle in a worker process.
//!
//! The developer guide is `docs/plugins.md`.

pub mod author;
pub mod bridge;
pub mod catalogue;
pub mod manifest;
pub mod ofx;
pub mod runtime;
pub mod store;

use std::sync::OnceLock;

pub use manifest::{Kind, Manifest};
pub use store::{Host, Imported, Installed};

/// Where installed plugins live: `<Studio>/Plugins`.
pub fn plugins_dir() -> std::path::PathBuf {
    studio_paths::studio().join("Plugins")
}

/// The host for Studio's plugin folder. Its plugins start on first use;
/// call [`start`] early so projects that use plugin effects render them
/// from the first frame.
pub fn host() -> &'static Host {
    static H: OnceLock<Host> = OnceLock::new();
    H.get_or_init(|| Host::at(plugins_dir()))
}

/// Starts every enabled plugin. Cheap after the first call.
pub fn start() -> usize {
    host().list().iter().filter(|p| p.enabled).count()
}
