// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio AI: local AI features built on the in-house `studio-nn` engine.
//!
//! * [`registry`] — every model, its upstream URL, SHA-256 and licence.
//! * [`store`] — the models folder (`~/Movies/Studio/Models`): status,
//!   verification and the explicit, user-initiated download (the crate's only
//!   network access).
//! * [`jobs`] — background jobs with progress and cancel; [`cache`] — results
//!   cached by (media hash, model, parameters).
//! * [`features`] — captions, background removal, object cut-out, voice
//!   cleanup, frame interpolation, depth, faces/pose, scene cuts, auto
//!   reframe, silence/filler removal and beat sync.
//!
//! Every feature degrades gracefully: without its model it returns
//! [`Error::ModelMissing`], whose message names the model and its size, and
//! features with a classical fallback use that instead.

pub mod cache;
pub mod engine;
pub mod error;
pub mod features;
pub mod frame;
pub mod jobs;
pub mod objects;
pub mod registry;
pub mod sha256;
pub mod store;

pub use cache::{media_hash, CacheKey, Cacheable, ResultCache};
pub use error::{Error, Result};
pub use frame::{Frame, FrameSource, Mask, VecSource};
pub use jobs::{JobCtx, JobHandle, JobInfo, JobState, Jobs};
pub use registry::{ModelId, ModelSpec};
pub use store::{ModelStatus, ModelStore};
