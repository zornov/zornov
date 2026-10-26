//! Vendored from PhotoCraft (<https://github.com/storytold/photocraft>, `crates/dev/testkit`,
//! revision 486afd82cce99255e999224bcfe28de00fd766aa), unchanged apart from crate names and
//! asset paths.
//! Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors; MIT OR Apache-2.0
//! (`LICENSE-MIT`, `LICENSE-APACHE` in this folder).
//!
//! studio-pixel-testkit
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod gpu;
#[cfg(not(target_arch = "wasm32"))]
pub mod perf;
pub mod sweep;

pub use gpu::gpu_lock;
pub use sweep::Sweep;
