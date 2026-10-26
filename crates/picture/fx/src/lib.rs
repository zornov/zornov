// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio effects, transitions, motion presets, speed ramps, stabilisation
//! and tracking.
//!
//! Everything works on [`Image`], a linear-light premultiplied RGBA `f32`
//! image. Each catalogue item carries a declarative parameter list
//! ([`ParamDef`]) so the editor can build its controls from data.
//!
//! - [`effects`]: blur, light, stylise, distort, retro, time, keying and
//!   generator effects ([`effects::all`], [`effects::find`]).
//! - [`transitions`]: two-input transitions, exact at both ends.
//! - [`motion`]: entry/exit/loop motion presets as poses, text stagger.
//! - [`speed`]: Bézier speed ramps, presets and time remapping.
//! - [`stabilise`] and [`track`]: global motion estimation and smoothing,
//!   point and box tracking that produce transform keyframes.
//! - [`external`]: effects and transitions registered at run time
//!   (plugins), found by the same lookups as the built-in ones.
//! - [`particles`]: stateless particle systems (rain, snow, leaves, sparks,
//!   mist...) behind the `particles` generator.
//! - [`comp`]: the node compositor (merge, keyers, roto, channels, depth
//!   of field, cleanup, paint) and [`flow`], dense optical flow.
//! - [`solve`]: 3D camera tracking (matchmove) from 2D feature tracks.
//! - [`shaders`]: WGSL compute shaders for the per-pixel items.
//!
//! See the crate README for conventions (units, GPU bindings, tests).

pub mod comp;
pub mod easing;
pub mod external;
pub mod effects;
pub mod features;
pub mod flow;
pub mod image;
pub mod keycolour;
pub mod math;
pub mod motion;
pub mod ops;
pub mod params;
pub mod particles;
pub mod shaders;
pub mod shapes;
pub mod solve;
pub mod speed;
pub mod stabilise;
pub mod testcard;
pub mod track;
pub mod transitions;

pub use effects::{Category, EffectContext, EffectDef};
pub use image::{Edge, Image, Px};
pub use params::{ParamDef, ParamKind, ParamSet, ParamValue, Unit};
pub use motion::{MotionDef, Phase, Pose};
pub use transitions::{TransitionContext, TransitionDef};
