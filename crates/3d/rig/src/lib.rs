// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Character rigging math shared by Studio's 2D skeletons
//! (`studio_model::Skeleton`, bones on a picture) and 3D armatures
//! (`studio_mesh::rig`, bones in a model). The same solver runs for both:
//! everything here works on points of any dimension (`[f64; N]`, N = 2 or
//! 3), and each side turns solved joint positions back into its own bone
//! rotations.
//!
//! - [`ik`]: analytic two-bone IK with a pole (bend direction), FABRIK
//!   and positional CCD for longer chains, soft IK (no snap at full
//!   reach), stretch with a limit, and the pole that matches an FK pose
//!   (IK/FK snapping).
//! - [`path`]: a smooth path through points, walked by arc length (path
//!   follow constraints).
//! - [`pin`]: contacts found in motion (a foot on the ground) and the
//!   pinned targets that keep planted feet from sliding.
//! - [`weights`]: skin weights on any mesh graph: automatic weights
//!   measured through the shape (geodesic), smoothing, normalising,
//!   brushes for painting, mirroring.
//! - [`dq`]: dual-quaternion skinning (no "candy wrapper" collapse at a
//!   twisted joint), with stretch blended beside it.
//! - [`names`]: sides and roles in bone names (`.L` / `.R`, `left`,
//!   `front` / `back`), mirrored names, and the shared role vocabulary
//!   that lets a motion move between 2D and 3D rigs.
//! - [`retarget`]: a motion from one rig onto another of other
//!   proportions (rest-relative bone directions, scaled root motion).
//! - [`scissor`]: where to cut a picture so its parts move apart: narrow
//!   gaps and notches from the silhouette, the lips from face landmarks,
//!   and the borders between parts.
//! - [`reader`]: pose readers that drive corrective shapes from a pose
//!   (cone and angle readers, RBF interpolation).
//!
//! The methods are the textbook ones (law of cosines, FABRIK from Aristidou
//! and Lasenby, soft IK as animators use it, Kavan's dual quaternions,
//! Gaussian RBFs); no engine's code is copied.

pub mod dq;
pub mod ik;
pub mod names;
pub mod path;
pub mod pin;
pub mod reader;
pub mod retarget;
pub mod scissor;
pub mod v;
pub mod weights;

pub use v::P;
