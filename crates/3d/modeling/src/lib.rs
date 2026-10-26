// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's 3D content creation, on top of [`studio_mesh`]'s triangles.
//!
//! - [`poly`]: polygon meshes (quads and n-gons) with creases, hard
//!   edges, seams and face sets, and the conversion to triangles.
//! - [`prim`]: quad starting shapes laid out for texturing.
//! - [`select`]: picking faces and edges in words (facing, inside a box,
//!   edge loops and rings, sharper than an angle).
//! - [`edit`]: extrude, inset, loop cut, knife and bisect, bridge, merge,
//!   dissolve, fill, symmetry, soft-selection moves, spin.
//! - [`selection`], [`ops`] and [`tools`]: edit mode — what a person has
//!   picked, the rest of the hand-modelling toolbox, and the one table of
//!   every operation that menus, search, shortcuts and the AI share.
//! - [`bevel`]: chamfers and rounded bevels with mitred corners.
//! - [`subd`]: Catmull–Clark subdivision with semi-sharp creases.
//! - [`modifiers`]: the non-destructive stack (mirror, array, solidify,
//!   bevel, subdivision, booleans, deformers, lattice, shrink-wrap…).
//! - [`remesh`]: voxel remeshing to even quads.
//! - [`spatial`]: ray casts and nearest points on a surface.

pub mod asset;
pub mod bake;
pub mod bevel;
pub mod curves;
pub mod edit;
pub mod formats;
pub mod gen3d;
pub mod groom;
pub mod hair;
pub mod kits;
pub mod library;
pub mod material;
pub mod modifiers;
pub mod multires;
pub mod nodes;
pub mod noise;
pub mod ops;
pub mod places;
pub mod poly;
pub mod prim;
pub mod remesh;
pub mod scatter;
pub mod retopo;
pub mod sculpt;
pub mod select;
pub mod selection;
pub mod spatial;
pub mod subd;
pub mod tools;
pub mod uv;

pub use poly::{Edge, NormalMode, Poly, ekey};
