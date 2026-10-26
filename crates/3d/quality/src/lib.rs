// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Quality for Studio's AI-made models, pictures and vector art: critics
//! that measure a result the way a production artist would judge it, the
//! pipelines that bring a result up to that standard, and a repeatable
//! benchmark that keeps the scores over time.
//!
//! Critics:
//! - [`topo`]: closed, manifold, clean quads, poles, edge loops.
//! - [`uv`]: islands, stretch, overlap, packing, even texture detail.
//! - [`pbr`]: physical base colours, metal or not, no light baked in.
//! - [`views`]: silhouettes against a reference from every side, surface
//!   distance, symmetry.
//! - [`hero`]: whether a hero object was designed or only stacked: pieces,
//!   materials, razor edges, facets, outline, match to a reference picture.
//! - [`image`]: exposure, noise, blocks, banding, sharpness, composition,
//!   text accuracy.
//! - [`vector`]: anchors, kinks, redundant points, wiggles, fill fidelity.
//!
//! Pipelines:
//! - [`remesh`]: any model rebuilt as a closed, all-quad surface of a
//!   chosen face budget (voxel field, surface nets, projection back onto
//!   the original, relaxation); mirror-symmetric where the model is.
//! - [`unwrap`]: texture coordinates cut into charts, flattened with
//!   least-squares conformal maps and packed at even texture detail.
//! - [`transfer`]: colours and textures carried from the original model
//!   onto the new one's texture.
//! - [`repair`]: holes, specks, flipped and degenerate faces.
//! - [`production`]: the whole finish, scored, with retries until the
//!   model clears the bars, and best-of-N choice.
//!
//! [`bench`] runs the benchmark cases headlessly and keeps the history.

pub mod bench;
pub mod geo;
pub mod hero;
pub mod image;
pub mod pbr;
pub mod production;
pub mod raster;
pub mod remesh;
pub mod repair;
pub mod scan;
pub mod score;
pub mod topo;
pub mod transfer;
pub mod unwrap;
pub mod uv;
pub mod vector;
pub mod views;

pub use score::{ModelReport, critique_model};
