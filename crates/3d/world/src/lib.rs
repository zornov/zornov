// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Worlds you can walk through.
//!
//! A [`world::World`] has real depth: a heightfield [`terrain`] that is
//! sculpted and painted, shapes ([`asset`]) placed one by one or
//! scattered by the thousand, water, lights, a sky with a time of day and
//! air that fogs the distance. [`cull`] decides what is worth drawing,
//! [`collide`] and [`walk`] let a character stand, step, climb, jump and
//! bump into it, and [`record`] turns a walk into a smoothed camera path
//! for a shot. [`build`] makes a world from a description, from the
//! modular [`kit`]s; the renderer lives in `studio-render`.
//!
//! Units are metres: x right, y up, z forward. Studio's older 3D layers
//! use pixels with y down; [`math::from_studio`] converts.

pub mod asset;
pub mod build;
pub mod character;
pub mod collide;
pub mod cull;
pub mod kit;
pub mod look;
pub mod math;
pub mod nav;
pub mod noise;
pub mod record;
pub mod terrain;
pub mod walk;
pub mod world;

/// sRGB byte to linear.
pub fn look_lin(v: u8) -> f32 {
    studio_mesh::srgb_to_linear(v)
}
