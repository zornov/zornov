// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio library content.
//!
//! This crate owns everything about the bundled and downloadable library:
//!
//! * [`item`]: the index types (`Item`, `Kind`, `Licence`, `Index`, `Library`).
//! * [`made`] and [`open`]: loaders for the two input sources — our own
//!   assets (`manifest.json` per set) and the open-licence collection
//!   (`catalog.json`).
//! * [`search`]: full-text search over names, tags, sets and categories.
//! * [`thumb`]: thumbnail generation with a high-quality downscale and an
//!   on-disk cache; [`image`] holds the pixel helpers.
//! * [`key`]: automatic chroma key for green-screen items (spill
//!   suppression, soft edges, neutrals kept opaque).
//! * [`gate`]: the quality gate and contact sheets for review.
//! * [`svg`]: a small SVG rasteriser for emoji, shapes and icons.
//! * [`video`]: a poster frame from an MP4 clip.
//! * [`jpeg`]: a baseline JPEG encoder for opaque backgrounds and photos.
//! * [`png`]: a PNG encoder with dynamic Huffman coding (smaller files than
//!   the fixed-code encoder in `studio-media`).
//! * [`pack`] and [`bundle`]: pack layout, `index.json`, `LICENSES.md` and
//!   `ATTRIBUTIONS.md` generation, and loading a built pack in the app.
//! * [`naming`]: human naming rules — no ids, prompts or hashes in any
//!   user-visible string.
//!
//! The crate has no dependencies outside the workspace apart from `serde`.

pub mod bundle;
pub mod gate;
pub mod image;
pub mod item;
pub mod jpeg;
pub mod key;
pub mod made;
pub mod naming;
pub mod open;
pub mod pack;
pub mod png;
pub mod search;
pub mod svg;
pub mod thumb;
pub mod video;

pub use item::{Index, Item, Kind, Library, Licence, Source};
pub use studio_media::rgba::RgbaImage;
