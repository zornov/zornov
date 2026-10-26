// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Vector illustration for Studio: the document model, its live
//! constructions and type, the editing operations the app and the AI
//! use, SVG/PDF/EPS interchange, and animation on the timeline. Drawing
//! goes through `studio-vector-raster` (exact-area coverage on the CPU
//! and GPU), so art is crisp at any resolution.
//!
//! * [`doc`]: [`Document`] with artboards, layers, groups, paths, live
//!   shapes, type, images, symbols and instances, repeats, blends,
//!   envelopes and perspective; guides, grid and the perspective grid.
//! * [`appearance`]: fills and strokes stacked per object, effects,
//!   graphic styles. [`color`]: swatches, global colours, harmonies.
//! * [`build`]: document to rasterizer scene; [`edit`]: drawing tools,
//!   pathfinder, shape builder, live paint, cutting, blob brush, width
//!   profiles, expand, image trace, arrange. [`ops`]: all of it as JSON
//!   operations (what the AI sends).
//! * [`interop`]: `.svart` (this crate's JSON), SVG, PDF, EPS, PNG.
//! * [`anim`]: keyframed properties and bindings to clip parameters.

mod adapted;
pub mod anim;
pub mod appearance;
pub mod build;
pub mod color;
pub mod doc;
pub mod edit;
pub mod interop;
pub mod live;
pub mod ops;
pub mod shapes;
pub mod text;

pub use doc::{Artboard, Document, Node, NodeId, NodeKind};
pub use studio_vector_geom as geom;
pub use studio_vector_pathops as pathops;
pub use studio_vector_raster as raster;
pub use studio_vector_raster::FillRule;
