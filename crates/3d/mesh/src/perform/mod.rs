// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Character performance for any rigged character: locomotion with
//! planted feet, hand and body poses, looks, reaches and acting beats,
//! the pose of a character at any moment in world space, and the checks
//! that tell stiff or broken movement from good.
//!
//! * [`body`]: which bones are legs, arms, spine, head, eyes, fingers,
//!   found by name in any common spelling.
//! * [`solve`]: posing by where things go (aim, turn, reach by IK).
//! * [`gait`]: walks, runs, limps, trots along a path with feet that stay
//!   put while they are down; starts, stops, turns and uneven ground.
//! * [`hands`]: a library of hand poses, blendable.
//! * [`pose`]: body poses (sit, lean, crouch), looks that lead with the
//!   eyes, reaches that never stretch, acting beats; all additive layers.
//! * [`query`]: where every bone and key point is at a moment.
//! * [`check`]: sliding feet, crossed feet, stretched or broken limbs,
//!   characters not looking where they were told.

pub mod body;
pub mod check;
pub mod gait;
pub mod hands;
pub mod pose;
pub mod query;
pub mod solve;

#[cfg(test)]
mod sheet;
#[cfg(test)]
mod tests;
